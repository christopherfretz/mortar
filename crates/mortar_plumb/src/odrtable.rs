//! Stage 4 (`spec/plumb.md` §13): `.mason.meta` in outputs (P-M1), the
//! one-definition-rule table `.note.mason` (P-M2), and "a table adds no
//! code" (P-M3). The rules are `spec/odr.md` §12.5's; the container is
//! read by [`crate::meta`], this crate's own reader.

use std::collections::{BTreeMap, BTreeSet};

use mortar_elf::SymbolSection;
use mortar_elf::consts::{
    ET_REL, PT_INTERP, PT_NOTE, SHF_ALLOC, SHF_EXCLUDE, SHT_FINI_ARRAY, SHT_INIT_ARRAY, SHT_NOTE,
    SHT_PREINIT_ARRAY, SHT_PROGBITS, STB_LOCAL,
};

use crate::dynout::DynOut;
use crate::image::Image;
use crate::line::{Item, Line};
use crate::locate::{Obj, Place};
use crate::map::{ChunkKind, Format, Map, SectionRef};
use crate::meta::{self, KIND2_ROW, KIND3_ROW, Unit, fnv1a, u16_at, u32_at, u64_at};
use crate::opts::{OdrTable, Options};
use crate::report::{Finding, MetaCoverage, Rule};

const NT_ODR: u32 = 0x4d4f_4401;
const NT_ODR_FULL: u32 = 0x4d4f_4402;
const OWNER: &[u8] = b"MasonMeta\0";
/// O-15's flags: `VAGUE` (bit 0), `EXPLICIT_SPEC` (bit 3); O-51's
/// `NONPREEMPTIBLE` (bit 15).
const VAGUE: u16 = 1;
const EXPLICIT_SPEC: u16 = 8;
const NONPREEMPTIBLE: u16 = 0x8000;

/// `--odr-table`'s value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Table {
    Off,
    Exported,
    Full,
}

/// The options stage 4 reads (§13.1).
#[derive(Debug, Clone, Copy)]
struct Opts {
    table: Table,
    odr_off: bool,
    keep: bool,
}

/// Stage 4's options from §3.2a's reader.
fn options(o: &Options) -> Opts {
    Opts {
        table: match o.odr_table {
            OdrTable::Off => Table::Off,
            OdrTable::Exported => Table::Exported,
            OdrTable::Full => Table::Full,
        },
        odr_off: o.odr_off,
        keep: o.keep_mason_meta,
    }
}

/// A live relocatable file, with its units and its definitions.
struct Live<'o, 'a> {
    obj: &'o Obj<'a>,
    units: Vec<Unit<'a>>,
    /// Per unit: its kind 1 `lang` is `c`. O-33: a C unit has no kind 2
    /// or kind 3 rows, so the table rules read none of such a unit's
    /// (measured: mortar's table ignores them; §13.1).
    c: Vec<bool>,
    /// The kind 2 rows of the units the table rules read, by key, in
    /// (unit, row) order: (unit index, row).
    rows: BTreeMap<u64, Vec<(usize, &'a [u8])>>,
    /// Non-local definitions by name: the output address of the
    /// definition when its section is placed whole, `None` otherwise.
    defs: BTreeMap<&'a [u8], Option<u64>>,
}

impl<'a> Live<'_, 'a> {
    /// The units whose kind 2 and kind 3 rows the table rules read.
    fn odr_units(&self) -> impl Iterator<Item = &Unit<'a>> {
        self.units.iter().zip(&self.c).filter(|(_, c)| !**c).map(|(u, _)| u)
    }

    /// The first kind 2 row of this file's units that matches `name`
    /// (§13.1: key, then the name or an anchor this file defines).
    fn row(&self, name: &[u8]) -> Option<&'a [u8]> {
        let key = fnv1a(name);
        self.rows.get(&key)?.iter().find_map(|&(u, r)| {
            let n = u32_at(r, 24).unwrap_or(0);
            let named = if n == 0 {
                self.defs.contains_key(name)
            } else {
                self.units[u].string(n) == Some(name)
            };
            named.then_some(r)
        })
    }
}

fn qualifies(row: &[u8]) -> bool {
    u16_at(row, 30).unwrap_or(0) & (VAGUE | EXPLICIT_SPEC) != 0
}

/// The live relocatable files; their definitions are read only when
/// `defs` (an output with no unit and no table pays for no symbol walk).
fn live_files<'o, 'a>(
    objs: &'o [Obj<'a>],
    defs_too: bool,
    cov: &mut MetaCoverage,
) -> Vec<Live<'o, 'a>> {
    let mut out = Vec::new();
    for obj in objs {
        if obj.elf.header().e_type != ET_REL {
            continue;
        }
        let mut units = Vec::new();
        for (si, s) in
            obj.secs.iter().enumerate().filter(|(_, s)| is_container(s.name.as_str(), &s.shdr))
        {
            // MT-2a: a relocated container is not one (never read, never
            // kept). MT-1's other attributes are the reader's (MT-6): a
            // section with another alignment, `sh_entsize`, `sh_link` or
            // `sh_info` is malformed whole (measured: mortar keeps none
            // of its units; `--odr=strict` names the alignment, MT-1).
            if s.relocs.is_some() {
                continue;
            }
            let h = &s.shdr;
            if h.sh_addralign != 8 || h.sh_entsize != 0 || h.sh_link != 0 || h.sh_info != 0 {
                cov.units_malformed += 1;
                continue;
            }
            // The skeleton holds every `.mason.meta` (§8.1).
            let sec = meta::read(obj.resident(si).unwrap_or(&[]));
            cov.units_malformed += sec.problems.len() as u64;
            units.extend(sec.units);
        }
        cov.units += units.len() as u64;
        let mut defs = BTreeMap::new();
        if let Some(t) = obj.symtab.as_ref().filter(|_| defs_too) {
            for (idx, s) in t.iter() {
                if s.binding() == STB_LOCAL {
                    continue;
                }
                let Ok(name) = t.name(&s) else { continue };
                let addr = match t.section_of(idx, &s) {
                    Ok(SymbolSection::Undefined) | Err(_) => continue,
                    Ok(SymbolSection::Section(i)) => {
                        match obj.secs.get(i.0 as usize).map(|x| &x.place) {
                            Some(Place::Whole(a)) => Some(a.wrapping_add(s.st_value)),
                            _ => None,
                        }
                    }
                    Ok(_) => None,
                };
                defs.entry(name).or_insert(addr);
            }
        }
        let c: Vec<bool> = units.iter().map(is_c).collect();
        let mut rows: BTreeMap<u64, Vec<(usize, &[u8])>> = BTreeMap::new();
        for (ui, u) in units.iter().enumerate().filter(|(i, _)| !c[*i]) {
            for r in u.block(2, KIND2_ROW).into_iter().flat_map(meta::Block::rows) {
                rows.entry(u64_at(r, 0).unwrap_or(0)).or_default().push((ui, r));
            }
        }
        out.push(Live { obj, units, c, rows, defs });
    }
    out
}

/// Whether a unit's kind 1 row `lang` has the value `c` (MT-26, O-33).
fn is_c(u: &Unit<'_>) -> bool {
    u.block(1, 16).is_some_and(|b| {
        b.rows().any(|r| {
            u.string(u32_at(r, 0).unwrap_or(0)) == Some(&b"lang"[..])
                && u.string(u32_at(r, 4).unwrap_or(0)) == Some(&b"c"[..])
        })
    })
}

/// MT-6's predicate.
fn is_container(name: &str, shdr: &mortar_elf::Shdr) -> bool {
    name == ".mason.meta" && shdr.sh_type == SHT_PROGBITS && shdr.sh_flags == SHF_EXCLUDE
}

/// The live files and an index of their definitions by name, so that a
/// keeper is found in time proportional to the name's copies.
struct Lives<'o, 'a> {
    files: Vec<Live<'o, 'a>>,
    by_name: BTreeMap<&'a [u8], Vec<usize>>,
}

impl<'o, 'a> std::ops::Deref for Lives<'o, 'a> {
    type Target = [Live<'o, 'a>];
    fn deref(&self) -> &Self::Target {
        &self.files
    }
}

impl<'o, 'a> Lives<'o, 'a> {
    fn new(files: Vec<Live<'o, 'a>>) -> Self {
        let mut by_name: BTreeMap<&'a [u8], Vec<usize>> = BTreeMap::new();
        for (i, l) in files.iter().enumerate() {
            for name in l.defs.keys() {
                by_name.entry(name).or_default().push(i);
            }
        }
        Lives { files, by_name }
    }

    /// The keeper of `name` (§13.1), by the output's value of it when the
    /// output holds it. `Err(())`: not judged.
    fn keeper(&self, name: &[u8], value: Option<u64>) -> Result<Option<&Live<'o, 'a>>, ()> {
        let cands = self.by_name.get(name).map_or(&[][..], Vec::as_slice);
        match cands {
            [] => Ok(None),
            [one] => Ok(Some(&self.files[*one])),
            _ => {
                let Some(v) = value else { return Err(()) };
                let mut at = cands
                    .iter()
                    .filter(|&&i| self.files[i].defs.get(name).copied().flatten() == Some(v));
                match (at.next(), at.next()) {
                    (Some(&i), None) => Ok(Some(&self.files[i])),
                    _ => Err(()),
                }
            }
        }
    }
}

/// One note of `.note.mason`.
struct Note<'a> {
    ty: u32,
    /// Offset of the descriptor in the section.
    at: usize,
    desc: &'a [u8],
}

/// Stage 4's rules over one output. `opts` are the line's options when
/// known (§3.2a); `line` the line stage 3 read from its words.
#[allow(clippy::too_many_lines, clippy::similar_names)] // one rule after another, in §13.2's order
pub(crate) fn check(
    image: &Image<'_>,
    objs: &[Obj<'_>],
    map: &Map,
    opts: Option<&Options>,
    line: Option<&Line>,
    cov: &mut MetaCoverage,
) -> Vec<Finding> {
    let mut f = Vec::new();
    let mortar = map.format == Format::Mortar;
    // A reference linker has none of mortar's options.
    let opts = if mortar {
        opts.map(options)
    } else {
        Some(Opts { table: Table::Off, odr_off: false, keep: false })
    };
    cov.options_known = opts.is_some();
    let has_note = image.sections.iter().any(|s| s.name == ".note.mason");
    let has_container = objs.iter().any(|o| o.secs.iter().any(|s| is_container(&s.name, &s.shdr)));
    let live = Lives::new(live_files(objs, has_note || has_container, cov));
    let exec = image.header.e_type != ET_REL;
    if !exec {
        return f;
    }
    cov.outputs += 1;

    // P-M1: no container placed.
    for (fi, mf) in map.files.iter().enumerate() {
        let Some(obj) = objs.iter().find(|o| o.path == mf.path && o.member == mf.member) else {
            continue;
        };
        for (si, s) in obj.secs.iter().enumerate() {
            if !is_container(&s.name, &s.shdr) {
                continue;
            }
            let copied = matches!(s.place, Place::Whole(_) | Place::Pieces(_) | Place::Merged)
                || map.nonalloc.iter().any(|n| {
                    n.file as usize == fi
                        && match &n.section {
                            SectionRef::Index(i) => *i as usize == si,
                            SectionRef::Name(x) => x == ".mason.meta",
                        }
                });
            if copied {
                f.push(Finding::new(
                    Rule::Meta(1),
                    format!("container: {}'s .mason.meta (section {si}) is in the output; MT-31 consumes containers", obj.label()),
                ));
            }
        }
    }
    // P-M1: the kept section, or an ordinary copy.
    let kept_chunk =
        map.chunks.iter().any(|c| c.kind == ChunkKind::Synthetic && c.name == ".mason.meta");
    let out_meta: Vec<usize> =
        (0..image.sections.len()).filter(|&i| image.sections[i].name == ".mason.meta").collect();
    let expected = meta::kept(live.iter().flat_map(|l| l.units.iter()));
    if out_meta.len() > 1 {
        f.push(Finding::new(
            Rule::Meta(1),
            format!("kept section: {} sections named .mason.meta", out_meta.len()),
        ));
    }
    // A `.mason.meta` without `SHF_EXCLUDE` is copied as any section is
    // (encoding K, MT-6): refused under the option only when the output
    // gets the kept section (MT-32, as WP-099 wrote it).
    let ordinary = live.iter().any(|l| {
        l.obj.secs.iter().any(|s| s.name == ".mason.meta" && s.shdr.sh_flags & SHF_EXCLUDE == 0)
    });
    match out_meta.first() {
        Some(&i) if kept_chunk || !mortar => {
            if !kept_chunk && ordinary {
                if opts.is_some_and(|o| o.keep) && !expected.is_empty() {
                    f.push(Finding::new(Rule::Meta(1), "kept section: a .mason.meta that is not a container was copied under --keep-mason-meta (MT-32: refused by name)"));
                }
            } else {
                let s = &image.sections[i].shdr;
                if s.sh_type != SHT_PROGBITS || s.sh_flags != 0 || s.sh_addralign != 8 {
                    f.push(Finding::new(
                        Rule::Meta(1),
                        format!("kept section: type {:#x}, flags {:#x}, alignment {}; MT-32 is SHT_PROGBITS, no flags, 8", s.sh_type, s.sh_flags, s.sh_addralign),
                    ));
                }
                let got = image
                    .data
                    .get(s.sh_offset as usize..(s.sh_offset.saturating_add(s.sh_size)) as usize);
                if got != Some(expected.as_slice()) {
                    f.push(Finding::new(
                        Rule::Meta(1),
                        format!("kept section: {} bytes that are not MT-32's concatenation of the live units ({} bytes)", s.sh_size, expected.len()),
                    ));
                }
                if opts.is_some_and(|o| !o.keep) {
                    f.push(Finding::new(
                        Rule::Meta(1),
                        "kept section: present without --keep-mason-meta (MT-31)",
                    ));
                } else if expected.is_empty() {
                    f.push(Finding::new(Rule::Meta(1), "kept section: present with no live unit that parses (MT-32: no units, no section)"));
                }
                cov.kept_bytes += expected.len() as u64;
            }
        }
        Some(_) => {
            if !ordinary {
                f.push(Finding::new(
                    Rule::Meta(1),
                    "kept section: a .mason.meta the map does not write and no input copies",
                ));
            } else if opts.is_some_and(|o| o.keep) && !expected.is_empty() {
                f.push(Finding::new(Rule::Meta(1), "kept section: a .mason.meta that is not a container was copied under --keep-mason-meta (MT-32: refused by name)"));
            }
        }
        None => {
            if opts.is_some_and(|o| o.keep) && !expected.is_empty() {
                f.push(Finding::new(Rule::Meta(1), "kept section: absent under --keep-mason-meta with live units that parse (MT-32)"));
            }
        }
    }

    // P-M2.
    let interp = image.phdrs.iter().any(|p| p.p_type == PT_INTERP);
    let due = opts.map(|o| interp && o.table != Table::Off && !o.odr_off);
    let full = opts.map(|o| o.table == Table::Full);
    let note_sec = image.sections.iter().position(|s| s.name == ".note.mason");
    let Some(ni) = note_sec else {
        if due == Some(true) {
            f.push(Finding::new(
                Rule::Meta(2),
                "presence: no .note.mason, and --odr-table asks for one (O-50)",
            ));
        }
        if opts.is_some_and(|o| o.table != Table::Off) {
            pm3(image, objs, map, opts.is_some(), line, &mut f);
        }
        return f;
    };
    cov.tables += 1;
    let sh = image.sections[ni].shdr;
    if due == Some(false) {
        f.push(Finding::new(Rule::Meta(2), "presence: a .note.mason the link does not ask for (O-50: dynamic executables with --odr-table, --odr not off)").at(sh.sh_addr));
    }
    if sh.sh_type != SHT_NOTE || sh.sh_flags & SHF_ALLOC == 0 || sh.sh_addralign != 8 {
        f.push(Finding::new(Rule::Meta(2), format!("place: .note.mason has type {:#x}, flags {:#x}, alignment {}; O-52 is an allocated SHT_NOTE aligned to 8", sh.sh_type, sh.sh_flags, sh.sh_addralign)).at(sh.sh_addr));
    }
    let in_note = image.phdrs.iter().any(|p| {
        p.p_type == PT_NOTE
            && p.p_align == 8
            && p.p_offset <= sh.sh_offset
            && sh.sh_offset.saturating_add(sh.sh_size) <= p.p_offset.saturating_add(p.p_filesz)
            && p.p_vaddr.wrapping_sub(p.p_offset) == sh.sh_addr.wrapping_sub(sh.sh_offset)
    });
    if !in_note {
        f.push(
            Finding::new(
                Rule::Meta(2),
                "place: .note.mason is not inside a PT_NOTE with p_align 8 (O-52)",
            )
            .at(sh.sh_addr),
        );
    }
    let data = image
        .data
        .get(sh.sh_offset as usize..sh.sh_offset.saturating_add(sh.sh_size) as usize)
        .unwrap_or(&[]);
    let notes = match notes(data) {
        Ok(n) => n,
        Err(e) => {
            f.push(Finding::new(Rule::Meta(2), format!("place: {e}")).at(sh.sh_addr));
            return f;
        }
    };
    let firsts: Vec<&Note<'_>> = notes.iter().filter(|n| n.ty == NT_ODR).collect();
    let seconds: Vec<&Note<'_>> = notes.iter().filter(|n| n.ty == NT_ODR_FULL).collect();
    if firsts.len() != 1
        || seconds.len() > 1
        || notes.len() != firsts.len() + seconds.len()
        || notes.first().is_some_and(|n| n.ty != NT_ODR)
    {
        f.push(Finding::new(Rule::Meta(2), format!("place: {} notes, {} of type 0x4d4f4401 and {} of 0x4d4f4402; O-52 is one table note, then at most one =full note", notes.len(), firsts.len(), seconds.len())).at(sh.sh_addr));
    }
    if let Some(full) = full
        && due == Some(true)
        && full == seconds.is_empty()
    {
        f.push(
            Finding::new(
                Rule::Meta(2),
                format!(
                    "presence: the =full note is {} and --odr-table is {}",
                    if seconds.is_empty() { "absent" } else { "present" },
                    if full { "full" } else { "exported" }
                ),
            )
            .at(sh.sh_addr),
        );
    }
    let dout = DynOut::read(image);
    let mut carried: BTreeSet<Vec<u8>> = BTreeSet::new();
    if let Some(n) = firsts.first() {
        let base = sh.sh_addr + n.at as u64;
        table(n.desc, base, dout.as_ref(), &live, &mut carried, cov, &mut f);
    }
    if let Some(n) = seconds.first() {
        let base = sh.sh_addr + n.at as u64;
        full_note(n.desc, base, image, &live, &carried, cov, &mut f);
    }
    pm3(image, objs, map, opts.is_some(), line, &mut f);
    f
}

fn notes(data: &[u8]) -> Result<Vec<Note<'_>>, String> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < data.len() {
        let (Some(namesz), Some(descsz), Some(ty)) =
            (u32_at(data, at), u32_at(data, at + 4), u32_at(data, at + 8))
        else {
            return Err(format!("a note header cut short at offset {at}"));
        };
        let name = data.get(at + 12..at + 12 + namesz as usize);
        if namesz != 10 || name != Some(OWNER) {
            return Err(format!("the note at offset {at} is not owned by MasonMeta (n_namesz 10)"));
        }
        let d = at + 24;
        let desc = d
            .checked_add(descsz as usize)
            .and_then(|e| data.get(d..e))
            .ok_or(format!("the note at offset {at}: its descriptor runs past the section"))?;
        // The padding after the name (to offset 24) and after the
        // descriptor (to 8) is zero: the bytes are a function of the
        // rows (O-50), and nothing else determines a padding byte.
        let next = d + (descsz as usize).div_ceil(8) * 8;
        let pads = [(at + 22, d), (d + descsz as usize, next)];
        for (from, to) in pads {
            if data.get(from..to.min(data.len())).is_some_and(|p| p.iter().any(|&b| b != 0)) {
                return Err(format!("the note at offset {at}: a padding byte that is not zero"));
            }
        }
        out.push(Note { ty, at: d, desc });
        at = next;
    }
    Ok(out)
}

/// The first note: O-51's header and rows, against O-50's row set.
#[allow(clippy::too_many_lines, clippy::many_single_char_names)]
fn table(
    desc: &[u8],
    base: u64,
    dout: Option<&DynOut>,
    live: &Lives<'_, '_>,
    carried: &mut BTreeSet<Vec<u8>>,
    cov: &mut MetaCoverage,
    f: &mut Vec<Finding>,
) {
    let h = |o| u32_at(desc, o).unwrap_or(0);
    let (version, count, row_size, reserved) = (h(0), h(4), h(8), h(12));
    if desc.len() < 16
        || version != 1
        || row_size != 24
        || reserved != 0
        || (desc.len() - 16) as u64 != 24 * u64::from(count)
    {
        f.push(Finding::new(Rule::Meta(2), format!("header: version {version}, count {count}, row_size {row_size}, reserved {reserved}, descriptor {} bytes; O-51 is {{1, n, 24, 0}} and 16 + 24n bytes", desc.len())).at(base));
        return;
    }
    let syms = dout.map_or(&[][..], |d| d.syms.as_slice());
    let versym = dout.and_then(|d| d.versym.as_ref());
    let mut prev: Option<(u64, u32)> = None;
    let mut rows: BTreeSet<u32> = BTreeSet::new();
    let mut seen: BTreeMap<String, BTreeSet<u16>> = BTreeMap::new();
    for i in 0..count as usize {
        let r = &desc[16 + 24 * i..16 + 24 * (i + 1)];
        let at = base + 16 + 24 * i as u64;
        let (key, idx, flags, res) = (
            u64_at(r, 0).unwrap_or(0),
            u32_at(r, 16).unwrap_or(0),
            u16_at(r, 20).unwrap_or(0),
            u16_at(r, 22).unwrap_or(0),
        );
        cov.rows += 1;
        if res != 0 {
            f.push(
                Finding::new(
                    Rule::Meta(2),
                    format!("header: row {i}'s reserved field is {res:#x}, not 0"),
                )
                .at(at + 22),
            );
        }
        if prev.is_some_and(|p| p >= (key, idx)) {
            f.push(
                Finding::new(
                    Rule::Meta(2),
                    format!(
                        "order: row {i} is not after row {} by key, then .dynsym index (O-51)",
                        i - 1
                    ),
                )
                .at(at),
            );
        }
        prev = Some((key, idx));
        let Some(s) = syms.get(idx as usize).filter(|_| idx != 0) else {
            f.push(Finding::new(Rule::Meta(2), format!("row set: row {i}'s .dynsym index {idx} is not an entry of .dynsym ({} entries)", syms.len())).at(at + 16));
            continue;
        };
        rows.insert(idx);
        carried.insert(s.name.as_bytes().to_vec());
        if key != fnv1a(s.name.as_bytes()) {
            f.push(
                Finding::new(
                    Rule::Meta(2),
                    format!(
                        "key: row {i}'s key {key:#x} is not FNV-1a 64 of '{}' ({:#x})",
                        s.name,
                        fnv1a(s.name.as_bytes())
                    ),
                )
                .at(at),
            );
        }
        if let Some(v) = versym.and_then(|v| v.get(idx as usize)) {
            if *v == 0 {
                f.push(
                    Finding::new(
                        Rule::Meta(2),
                        format!(
                            "version: row {i} names '{}', whose .gnu.version index is 0 (local)",
                            s.name
                        ),
                    )
                    .at(at + 16),
                );
            }
            if !seen.entry(s.name.clone()).or_default().insert(*v & 0x7fff) {
                f.push(
                    Finding::new(
                        Rule::Meta(2),
                        format!("version: two rows of '{}' with one version index", s.name),
                    )
                    .at(at + 16),
                );
            }
        }
        match live.keeper(s.name.as_bytes(), Some(s.st_value)) {
            Err(()) => cov.not_judged += 1,
            Ok(k) => match k.and_then(|k| k.row(s.name.as_bytes())).filter(|r| qualifies(r)) {
                None => f.push(Finding::new(Rule::Meta(2), format!("row set: a row for '{}', whose keeper has no kind 2 row with VAGUE or EXPLICIT_SPEC (O-50)", s.name)).at(at)),
                Some(kr) => {
                    if r[8..16] != kr[8..16] {
                        f.push(Finding::new(Rule::Meta(2), format!("hash: row {i} ('{}') does not carry the first 8 bytes of its keeper's kind 2 hash ({})", s.name, k.map(|k| k.obj.label()).unwrap_or_default())).at(at + 8));
                    }
                    let want = u16_at(kr, 30).unwrap_or(0) | NONPREEMPTIBLE;
                    if flags != want {
                        f.push(Finding::new(Rule::Meta(2), format!("flags: row {i} ('{}') has {flags:#x}; the keeper's row's flags with NONPREEMPTIBLE are {want:#x}", s.name)).at(at + 20));
                    }
                }
            },
        }
    }
    // The other half of the row set: every entry O-50 gives a row has one.
    for (idx, s) in syms.iter().enumerate().skip(1) {
        let vis = s.st_other & 3;
        if s.st_shndx == 0 || !(vis == 0 || vis == 3) || rows.contains(&(idx as u32)) {
            continue;
        }
        match live.keeper(s.name.as_bytes(), Some(s.st_value)) {
            Err(()) => cov.not_judged += 1,
            Ok(k) => {
                if k.and_then(|k| k.row(s.name.as_bytes())).is_some_and(qualifies) {
                    f.push(Finding::new(Rule::Meta(2), format!("row set: '{}' (.dynsym {idx}) is exported and its keeper's kind 2 row has VAGUE or EXPLICIT_SPEC, and no row names it (O-50)", s.name)).at(base));
                }
            }
        }
    }
}

/// `=full`'s note (O-51, O-52; §13.2).
#[allow(clippy::too_many_lines, clippy::many_single_char_names)]
fn full_note(
    desc: &[u8],
    base: u64,
    image: &Image<'_>,
    live: &Lives<'_, '_>,
    carried: &BTreeSet<Vec<u8>>,
    cov: &mut MetaCoverage,
    f: &mut Vec<Finding>,
) {
    let h = |o| u32_at(desc, o).unwrap_or(0);
    let (version, defs, row_size, types) = (h(0), h(4), h(8), h(12));
    if desc.len() < 16
        || version != 1
        || row_size != 32
        || (desc.len() - 16) as u64 != 32 * (u64::from(defs) + u64::from(types))
    {
        f.push(Finding::new(Rule::Meta(2), format!("full: header version {version}, definitions {defs}, row_size {row_size}, types {types}, descriptor {} bytes; O-51 is {{1, d, 32, t}} and 16 + 32(d + t) bytes", desc.len())).at(base));
        return;
    }
    let row = |i: usize| &desc[16 + 32 * i..16 + 32 * (i + 1)];
    let at = |i: usize| base + 16 + 32 * i as u64;
    for (lo, n, what) in
        [(0usize, defs as usize, "definition"), (defs as usize, types as usize, "type")]
    {
        for i in lo + 1..lo + n {
            if sort_key(row(i - 1)) >= sort_key(row(i)) {
                f.push(Finding::new(Rule::Meta(2), format!("full: {what} row {} is not after the one before it (O-51: sorted by bytes as integers, no two equal)", i - lo)).at(at(i)));
            }
        }
    }
    // Definitions.
    let mut want: BTreeMap<u64, ([u8; 8], u16, String)> = BTreeMap::new();
    let mut judged_keys: BTreeSet<u64> = BTreeSet::new();
    // The output's value of each name: its first defined `.symtab` entry.
    let mut values: BTreeMap<&[u8], u64> = BTreeMap::new();
    for s in image.symbols.iter().filter(|s| s.sym.st_shndx != 0) {
        values.entry(s.name.as_bytes()).or_insert(s.sym.st_value);
    }
    for &name in live.by_name.keys() {
        if carried.contains(name) {
            continue;
        }
        let value = values.get(name).copied();
        match live.keeper(name, value) {
            Err(()) => cov.not_judged += 1,
            Ok(k) => {
                let key = fnv1a(name);
                judged_keys.insert(key);
                if let Some(r) = k.and_then(|k| k.row(name)).filter(|r| qualifies(r)) {
                    let mut hsh = [0u8; 8];
                    hsh.copy_from_slice(&r[8..16]);
                    want.insert(
                        key,
                        (
                            hsh,
                            u16_at(r, 30).unwrap_or(0),
                            String::from_utf8_lossy(name).into_owned(),
                        ),
                    );
                }
            }
        }
    }
    let carried_keys: BTreeSet<u64> = carried.iter().map(|n| fnv1a(n)).collect();
    let mut found: BTreeSet<u64> = BTreeSet::new();
    for i in 0..defs as usize {
        let r = row(i);
        cov.full_rows += 1;
        let key = u64_at(r, 0).unwrap_or(0);
        let flags = u16_at(r, 30).unwrap_or(0);
        if u64_at(r, 16) != Some(0)
            || u32_at(r, 24) != Some(0)
            || u16_at(r, 28) != Some(0)
            || flags & NONPREEMPTIBLE != 0
        {
            f.push(Finding::new(Rule::Meta(2), format!("full: definition row {i} has a layout, size or alignment, or NONPREEMPTIBLE (O-51: zero, clear)")).at(at(i) + 16));
        }
        if carried_keys.contains(&key) {
            f.push(Finding::new(Rule::Meta(2), format!("full: definition row {i} ({key:#x}) names a definition the table note carries")).at(at(i)));
        }
        found.insert(key);
        if let Some((hsh, fl, name)) = want.get(&key) {
            if r[8..16] != hsh[..] {
                f.push(
                    Finding::new(
                        Rule::Meta(2),
                        format!(
                            "full: definition row {i} ('{name}') does not carry its keeper's hash"
                        ),
                    )
                    .at(at(i) + 8),
                );
            }
            if flags != *fl {
                f.push(Finding::new(Rule::Meta(2), format!("full: definition row {i} ('{name}') has flags {flags:#x}, its keeper's row {fl:#x}")).at(at(i) + 30));
            }
        } else if judged_keys.contains(&key) {
            f.push(Finding::new(Rule::Meta(2), format!("full: definition row {i} ({key:#x}) names a definition whose keeper has no kind 2 row with VAGUE or EXPLICIT_SPEC")).at(at(i)));
        } else {
            // Not a judged name: the row must still be some live unit's.
            let justified = live
                .iter()
                .flat_map(Live::odr_units)
                .filter_map(|u| u.block(2, KIND2_ROW))
                .flat_map(meta::Block::rows)
                .any(|x| {
                    u64_at(x, 0) == Some(key)
                        && qualifies(x)
                        && x[8..16] == r[8..16]
                        && u16_at(x, 30) == Some(flags)
                });
            if !justified {
                f.push(Finding::new(Rule::Meta(2), format!("full: definition row {i} ({key:#x}) is no live unit's qualifying kind 2 row")).at(at(i)));
            }
        }
    }
    for (key, (_, _, name)) in &want {
        if !found.contains(key) {
            f.push(Finding::new(Rule::Meta(2), format!("full: '{name}' is a live definition not exported whose keeper's row has VAGUE or EXPLICIT_SPEC, and has no definition row")).at(base));
        }
    }
    // Types: one row per kind 3 group of the live units, the first.
    let mut groups: BTreeMap<(u64, Vec<u8>), [u8; 32]> = BTreeMap::new();
    for u in live.iter().flat_map(Live::odr_units) {
        let Some(b) = u.block(3, KIND3_ROW) else { continue };
        for r in b.rows() {
            let key = u64_at(r, 0).unwrap_or(0);
            let name = u.string(u32_at(r, 32).unwrap_or(0)).unwrap_or(&[]).to_vec();
            groups.entry((key, name)).or_insert_with(|| {
                let mut t = [0u8; 32];
                t[0..16].copy_from_slice(&r[0..16]);
                t[16..24].copy_from_slice(&r[24..32]);
                t[24..28].copy_from_slice(&r[36..40]);
                t[28..32].copy_from_slice(&r[40..44]);
                t
            });
        }
    }
    let mut want_types: Vec<[u8; 32]> = groups.into_values().collect();
    want_types.sort_by_key(|r| sort_key(r));
    let got: Vec<&[u8]> = (defs as usize..(defs + types) as usize).map(row).collect();
    cov.full_rows += got.len() as u64;
    if got.len() != want_types.len() {
        f.push(
            Finding::new(
                Rule::Meta(2),
                format!(
                    "full: {} type rows; the live units have {} kind 3 groups",
                    got.len(),
                    want_types.len()
                ),
            )
            .at(at(defs as usize)),
        );
    }
    for (i, (g, w)) in got.iter().zip(&want_types).enumerate() {
        if *g != &w[..] {
            let field = (0..32).find(|&b| g[b] != w[b]).map_or("", |b| match b {
                0..8 => "key",
                8..16 => "hash",
                16..24 => "layout",
                24..28 => "size",
                28..30 => "alignment",
                _ => "flags",
            });
            f.push(Finding::new(Rule::Meta(2), format!("full: type row {i}'s {field} is not that of its group's first row in (file, unit, row) order")).at(at(defs as usize + i)));
        }
    }
}

/// O-51's order: the fields as integers, key first.
fn sort_key(r: &[u8]) -> (u64, [u8; 8], u64, u32, u16, u16) {
    let mut h = [0u8; 8];
    h.copy_from_slice(&r[8..16]);
    (
        u64_at(r, 0).unwrap_or(0),
        h,
        u64_at(r, 16).unwrap_or(0),
        u32_at(r, 24).unwrap_or(0),
        u16_at(r, 28).unwrap_or(0),
        u16_at(r, 30).unwrap_or(0),
    )
}

/// P-M3: a table adds no code (§13.2).
fn pm3(
    image: &Image<'_>,
    objs: &[Obj<'_>],
    map: &Map,
    inputs: bool,
    line: Option<&Line>,
    f: &mut Vec<Finding>,
) {
    for s in &image.sections {
        let t = s.shdr.sh_type;
        if !(t == SHT_INIT_ARRAY || t == SHT_FINI_ARRAY || t == SHT_PREINIT_ARRAY) {
            continue;
        }
        let (lo, hi) = (s.shdr.sh_addr, s.shdr.sh_addr.saturating_add(s.shdr.sh_size));
        let mut spans: Vec<(u64, u64)> = objs
            .iter()
            .flat_map(|o| o.secs.iter())
            .filter(|x| {
                x.shdr.sh_type == t
                    || (x.shdr.sh_type == SHT_PROGBITS
                        && (x.name.starts_with(".ctors") || x.name.starts_with(".dtors")))
            })
            .filter_map(|x| match x.place {
                Place::Whole(a) if a >= lo && a < hi => Some((a, a.saturating_add(x.shdr.sh_size))),
                _ => None,
            })
            .collect();
        spans.sort_unstable();
        let mut cur = lo;
        for (a, e) in spans {
            if a > cur {
                break;
            }
            cur = cur.max(e);
        }
        if cur < hi {
            f.push(
                Finding::new(
                    Rule::Meta(3),
                    format!(
                        "init array: {} [{cur:#x}, {hi:#x}) holds bytes no input {} section gives",
                        s.name, s.name
                    ),
                )
                .at(cur),
            );
        }
        for c in &map.chunks {
            if let Some(a) = c.addr
                && a >= lo
                && a < hi
            {
                f.push(
                    Finding::new(
                        Rule::Meta(3),
                        format!("init array: the map's chunk '{}' lies in {}", c.name, s.name),
                    )
                    .at(a),
                );
            }
        }
    }
    if !inputs {
        return;
    }
    let Some(line) = line else { return };
    let on_line: BTreeSet<&str> = line
        .items
        .iter()
        .map(|i| match i {
            Item::Relocatable { path, .. } | Item::Shared { path, .. } => path.as_str(),
        })
        .collect();
    for o in objs.iter().filter(|o| o.elf.header().e_type == ET_REL) {
        if !on_line.contains(o.path.as_str()) {
            f.push(Finding::new(
                Rule::Meta(3),
                format!("input: {} is not on the command line", o.label()),
            ));
        }
    }
}

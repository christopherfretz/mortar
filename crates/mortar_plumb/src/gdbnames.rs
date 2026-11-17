//! `.gdb_index`, mortar's rules (`spec/plumb.md` §6.10, P-G6 to P-G9):
//! the index `spec/synth.md` §17 says mortar writes, recomputed from the
//! inputs the map names (their units, their GNU name sets, their range
//! lists, read through their relocations) and compared with the output:
//! present exactly when S-G1 says, complete against the name sets, and
//! byte for byte. Only for a map in mortar's format; the format rules
//! (`crate::gdbindex`) judge every producer.

#![allow(clippy::many_single_char_names)] // byte readers: r, p, b, v, w, s, t

use std::collections::{BTreeMap, BTreeSet};

use mortar_elf::consts::{EM_386, EM_AARCH64, EM_X86_64, SHT_PROGBITS};
use mortar_elf::{Class, SymbolIndex, SymbolSection};

use crate::gdbdwarf::{self, Addr, View};
use crate::gdbindex::{self, Entry, Index, NAME, OutUnits, finding, hash, probe, show};
use crate::hasher::HashMap;
use crate::image::Image;
use crate::locate::{Obj, Place};
use crate::map::{ChunkKind, DiscardReason, Map, SectionRef};
use crate::report::Finding;
use crate::store::Bytes;

const PUBNAMES: &str = ".debug_gnu_pubnames";
const PUBTYPES: &str = ".debug_gnu_pubtypes";

/// Whether the link had `--gdb-index` (the last of it and
/// `--no-gdb-index`) and no strip option (S-G8), from mortar's map's line
/// (§3.2a); `false` for a reference's map (P-G6 is mortar's rule).
fn option(map: &Map, line: Option<&[String]>) -> bool {
    crate::opts::of(map, line).is_some_and(|o| o.gdb_index && !o.strip_debug)
}

/// P-G6: presence, the section's header, the consumed sets.
pub(crate) fn presence(
    image: &Image<'_>,
    objs: &[Obj<'_>],
    map: &Map,
    line: Option<&[String]>,
    found: &[usize],
    out: &mut Vec<Finding>,
) {
    let chunks: Vec<u64> = map
        .chunks
        .iter()
        .filter(|c| c.kind == ChunkKind::Synthetic && c.name == NAME)
        .map(|c| c.size)
        .collect();
    let opt = option(map, line);
    if found.is_empty() && chunks.is_empty() && !opt {
        return;
    }
    let units = gdbindex::out_units(image).units.len();
    let want = opt && units > 0;
    match (want, found.len()) {
        (true, 0) => out.push(finding(
            6,
            format!("--gdb-index is in force and the output's .debug_info has {units} unit(s), and the output has no {NAME}"),
        )),
        (false, n) if n > 0 => out.push(finding(
            6,
            format!("the output has {NAME} and the link's words do not ask for one with debug information (S-G1, S-G8)"),
        )),
        (_, n) if n > 1 => out.push(finding(6, format!("the output has {n} sections named {NAME}"))),
        _ => {}
    }
    if let Some(&si) = found.first() {
        let s = &image.sections[si];
        match chunks.as_slice() {
            [z] if *z == s.shdr.sh_size => {}
            [z] => out.push(finding(
                6,
                format!("the map's chunk is {z:#x} bytes and the section {:#x}", s.shdr.sh_size),
            )),
            [] => out.push(finding(
                6,
                format!("the output has {NAME} and the map has no chunk of that name"),
            )),
            _ => out.push(finding(6, "the map has more than one .gdb_index chunk")),
        }
        let h = &s.shdr;
        if h.sh_type != SHT_PROGBITS || h.sh_flags != 0 || h.sh_addralign != 4 || h.sh_entsize != 0
        {
            out.push(finding(
                6,
                format!(
                    "the section header is type {}, flags {:#x}, alignment {}, entry size {}; S-G1 says SHT_PROGBITS, 0, 4, 0",
                    h.sh_type, h.sh_flags, h.sh_addralign, h.sh_entsize
                ),
            ));
        }
        let last_debug = image.sections.iter().rposition(|x| x.name.starts_with(".debug_"));
        let symtab = image.sections.iter().position(|x| x.name == ".symtab");
        if last_debug.is_some_and(|d| d > si) || symtab.is_some_and(|y| y < si) {
            out.push(finding(
                6,
                "the section is not after the .debug_* sections and before .symtab (S-G1)",
            ));
        }
        for x in &image.sections {
            if x.name == PUBNAMES || x.name == PUBTYPES {
                out.push(finding(
                    6,
                    format!("the output has {} beside an index (S-G6: consumed)", x.name),
                ));
            }
        }
        for o in objs {
            for (k, x) in o.secs.iter().enumerate() {
                if (x.name == PUBNAMES || x.name == PUBTYPES)
                    && matches!(x.place, Place::Whole(_) | Place::Pieces(_) | Place::Merged)
                {
                    out.push(
                        finding(
                            6,
                            format!(
                                "the map places input {} beside an index (S-G6: consumed)",
                                x.name
                            ),
                        )
                        .site(o.site(k, 0)),
                    );
                }
            }
        }
    }
}

/// One section's relocations: `r_offset` -> (symbol, type, addend).
type Relocs = HashMap<u64, (u32, u32, Option<i64>)>;

/// An input object's sections read through its relocations (S-G5).
struct InView<'o, 'a> {
    obj: &'o Obj<'a>,
    machine: u16,
    /// Per section: `r_offset` -> (symbol, type, addend).
    relocs: Vec<Option<Relocs>>,
    /// Per section: its contents, read when first asked for (§8.1).
    data: Vec<std::sync::OnceLock<Bytes<'a>>>,
}

impl<'o, 'a> InView<'o, 'a> {
    fn new(obj: &'o Obj<'a>, machine: u16) -> Self {
        let relocs = obj
            .secs
            .iter()
            .map(|s| {
                let wanted = matches!(
                    s.name.as_str(),
                    ".debug_info"
                        | ".debug_addr"
                        | ".debug_rnglists"
                        | ".debug_ranges"
                        | PUBNAMES
                        | PUBTYPES
                );
                let rs = s.relocs.filter(|_| wanted)?;
                let recs = obj.records(rs)?;
                Some(recs.into_iter().map(|(off, sym, ty, a)| (off, (sym, ty, a))).collect())
            })
            .collect();
        let data = obj.secs.iter().map(|_| std::sync::OnceLock::new()).collect();
        InView { obj, machine, relocs, data }
    }

    /// Whether `ty` is a relocation of absolute class `w` bytes wide
    /// (Some(true)), a none type (Some(false)), or another (None).
    fn class(&self, ty: u32, w: u8) -> Option<bool> {
        match (self.machine, ty, w) {
            (EM_AARCH64, 0 | 256, _) | (EM_X86_64 | EM_386, 0, _) => Some(false),
            (EM_AARCH64, 257, 8)
            | (EM_AARCH64, 258, 4)
            | (EM_X86_64, 1, 8)
            | (EM_X86_64, 10, 4)
            | (EM_386, 1, 4) => Some(true),
            _ => None,
        }
    }
}

impl View for InView<'_, '_> {
    fn bytes(&self, s: usize) -> &[u8] {
        self.data.get(s).map_or(&[], |c| c.get_or_init(|| self.obj.data(s)))
    }
    fn field(&self, s: usize, off: u64, w: u8) -> Result<Addr, String> {
        let b = self.bytes(s);
        let raw = gdbdwarf::Rd::new(b, usize::try_from(off).unwrap_or(usize::MAX)).n(w as usize)?;
        let Some(&(sym, ty, addend)) =
            self.relocs.get(s).and_then(Option::as_ref).and_then(|m| m.get(&off))
        else {
            return Ok((None, raw));
        };
        match self.class(ty, w) {
            Some(false) => return Ok((None, raw)),
            None => {
                return Err(format!(
                    "a relocation of type {ty} at {off:#x} of {}",
                    self.obj.secs[s].name
                ));
            }
            Some(true) => {}
        }
        let t = self.obj.symtab.ok_or("relocations without a symbol table")?;
        let sy = t.get(SymbolIndex(sym)).map_err(|e| e.to_string())?;
        match t.section_of(SymbolIndex(sym), &sy).map_err(|e| e.to_string())? {
            SymbolSection::Section(k) => {
                let a = addend.unwrap_or(raw as i64);
                Ok((Some(k.0 as usize), sy.st_value.wrapping_add(a as u64)))
            }
            _ => Err(format!(
                "the relocation at {off:#x} of {} names a symbol not defined in a section",
                self.obj.secs[s].name
            )),
        }
    }
    fn offset_into(&self, s: usize, off: u64, name: &str) -> Result<(usize, u64), String> {
        match self.field(s, off, 4)? {
            (Some(k), v) if self.obj.secs.get(k).is_some_and(|x| x.name == name) => Ok((k, v)),
            (Some(k), _) => Err(format!(
                "the offset at {off:#x} of {} names {}, not {name}",
                self.obj.secs[s].name,
                self.obj.secs.get(k).map_or("?", |x| x.name.as_str())
            )),
            (None, _) => Err(format!(
                "the offset into {name} at {off:#x} of {} has no relocation",
                self.obj.secs[s].name
            )),
        }
    }
    fn input(&self) -> bool {
        true
    }
}

/// A unit of the inputs: its object and section, its unit offset.
type Key = (usize, usize, u64);

/// One set's entries: (attributes, name).
type Set<'a> = Vec<(u8, &'a [u8])>;

/// The output address of `(section, offset)` of object `o` (§4.3), or
/// `None` when the section is not in the output.
fn addr_in(o: &Obj<'_>, sec: usize, off: u64) -> Result<Option<u64>, String> {
    match &o.secs.get(sec).map(|s| &s.place) {
        Some(Place::Whole(a)) => Ok(Some(a.wrapping_add(off))),
        Some(Place::Pieces(ps)) => Ok(ps
            .iter()
            .find(|p| p.input_offset <= off && off < p.input_offset.saturating_add(p.size))
            .map(|p| p.addr.wrapping_add(off - p.input_offset))),
        Some(Place::Discarded | Place::Absent) | None => Ok(None),
        Some(_) => {
            Err(format!("section {} is placed in a way that gives no address", o.secs[sec].name))
        }
    }
}

/// What the inputs say the index holds (S-G2 to S-G5).
#[derive(Default)]
struct Want<'a> {
    cus: Vec<(u64, u64)>,
    area: Vec<(u64, u64, u32)>,
    /// Names in name order, each with its sorted values.
    names: Vec<(&'a [u8], BTreeSet<u32>)>,
}

/// P-G7 and P-G8, then P-G9 when nothing else was found.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)] // the rules in order
pub(crate) fn content(
    image: &Image<'_>,
    objs: &[Obj<'_>],
    map: &Map,
    index: &Index<'_>,
    units: &OutUnits,
    entries: &[Entry<'_>],
    clean: bool,
    out: &mut Vec<Finding>,
) {
    let before = out.len();
    let machine = image.header.e_machine;
    let word: u8 = if image.class() == Class::Elf64 { 8 } else { 4 };
    let info_base = units.sec.map_or(0, |i| image.sections[i].shdr.sh_addr);
    let mut want = Want::default();
    // S-G2: the live .debug_info input sections, in output order.
    let mut live: Vec<(u64, usize, usize)> = Vec::new();
    for (oi, o) in objs.iter().enumerate() {
        for (si, s) in o.secs.iter().enumerate() {
            if s.name == ".debug_types" && matches!(s.place, Place::Whole(_) | Place::Pieces(_)) {
                out.push(
                    finding(
                        7,
                        "a live input .debug_types beside an index (S-G2: gdb-index-type-unit)",
                    )
                    .site(o.site(si, 0)),
                );
            }
            if s.name == ".debug_info"
                && !s.alloc()
                && let Place::Whole(a) = s.place
            {
                live.push((a.wrapping_sub(info_base), oi, si));
            }
        }
    }
    live.sort_unstable();
    let views: BTreeMap<usize, InView<'_, '_>> = {
        let mut set: BTreeSet<usize> = live.iter().map(|x| x.1).collect();
        for (oi, o) in objs.iter().enumerate() {
            if o.secs.iter().any(|s| s.name == PUBNAMES || s.name == PUBTYPES) {
                set.insert(oi);
            }
        }
        set.into_iter().map(|oi| (oi, InView::new(&objs[oi], machine))).collect()
    };
    let mut cu_of: HashMap<Key, u32> = HashMap::default();
    let mut bearing: Vec<(Key, bool)> = Vec::new();
    for &(base, oi, si) in &live {
        let o = &objs[oi];
        let v = &views[&oi];
        let (us, err) = gdbdwarf::units(v.bytes(si));
        if let Some((off, e)) = err {
            out.push(finding(7, format!("an input unit cannot be read at {off:#x}: {e} (S-G2: gdb-index-malformed-unit)")).site(o.site(si, off)));
        }
        for u in &us {
            let cu = want.cus.len() as u32;
            want.cus.push((base.wrapping_add(u.off), u.len));
            cu_of.insert((oi, si, u.off), cu);
            let site = o.site(si, u.off);
            if u.addr_size != word {
                out.push(
                    finding(7, format!("an input unit of address size {} (S-G2)", u.addr_size))
                        .site(site),
                );
                continue;
            }
            match gdbdwarf::ranges(v, si, u) {
                Err(e) => out.push(finding(7, format!("an input unit's root DIE or ranges cannot be read: {e} (S-G5: gdb-index-malformed-unit)")).site(site)),
                Ok((root, rs)) => {
                    if let Err(e) = gdbdwarf::kind(u, &root) {
                        out.push(finding(7, format!("an input unit is {e} (S-G2)")).site(site.clone()));
                    }
                    bearing.push(((oi, si, u.off), gdbdwarf::debug_bearing(u, &root)));
                    for r in rs {
                        let len = r.hi.wrapping_sub(r.lo);
                        if len == 0 {
                            continue;
                        }
                        let Some(k) = r.sec else {
                            out.push(finding(7, "an input unit's range has no section (S-G5 rule 3)").site(site.clone()));
                            continue;
                        };
                        match addr_in(o, k, r.lo) {
                            Ok(Some(lo)) => want.area.push((lo, lo.wrapping_add(len), cu)),
                            Ok(None) => {}
                            Err(e) => out.push(finding(7, e).site(site.clone())),
                        }
                    }
                }
            }
        }
    }
    if want.cus != index.cus {
        out.push(finding(
            7,
            format!(
                "the CU list ({} entries) is not the units of the input .debug_info sections the map places ({} units)",
                index.cus.len(),
                want.cus.len()
            ),
        ));
    }
    if index.tus != 0 {
        out.push(finding(7, format!("the TU list has {} entries (S-G2: empty)", index.tus)));
    }
    // S-G4a: the name sets.
    let file_of: HashMap<(&str, Option<&str>), u32> = map
        .files
        .iter()
        .enumerate()
        .map(|(i, f)| ((f.path.as_str(), f.member.as_deref()), i as u32))
        .collect();
    let comdat: BTreeSet<(u32, u32)> = map
        .discarded
        .iter()
        .filter(|d| d.reason == DiscardReason::Comdat)
        .filter_map(|d| match d.section {
            SectionRef::Index(i) => Some((d.file, i)),
            SectionRef::Name(_) => None,
        })
        .collect();
    // Per unit: [pubnames, pubtypes].
    let mut sets: BTreeMap<u32, [Option<Set<'_>>; 2]> = BTreeMap::new();
    for (&oi, v) in &views {
        let o = &objs[oi];
        let file = file_of.get(&(o.path.as_str(), o.member.as_deref())).copied();
        for (si, s) in o.secs.iter().enumerate() {
            let which = match s.name.as_str() {
                PUBNAMES => 0,
                PUBTYPES => 1,
                _ => continue,
            };
            if file.is_some_and(|f| comdat.contains(&(f, si as u32))) {
                continue;
            }
            let mut p = 0u64;
            while p < v.bytes(si).len() as u64 {
                match read_set(v, si, p) {
                    Err(e) => {
                        out.push(finding(7, format!("an input {} set cannot be read: {e} (S-G4a: gdb-index-malformed-name-set)", s.name)).site(o.site(si, p)));
                        break;
                    }
                    Ok((next, (k, ioff), set)) => {
                        let site = o.site(si, p);
                        p = next;
                        if !objs[oi].secs.get(k).is_some_and(|x| matches!(x.place, Place::Whole(_)))
                        {
                            out.push(finding(7, format!("an input {} set names a unit whose section is not in the output (S-G4a: gdb-index-orphan-name-set)", s.name)).site(site));
                            continue;
                        }
                        let Some(&cu) = cu_of.get(&(oi, k, ioff)) else {
                            out.push(finding(7, format!("an input {} set names offset {ioff:#x}, not the start of a unit (S-G4a)", s.name)).site(site));
                            continue;
                        };
                        let slot = &mut sets.entry(cu).or_default()[which];
                        if slot.is_some() {
                            out.push(
                                finding(
                                    7,
                                    format!("two input {} sets name unit {cu} (S-G4a)", s.name),
                                )
                                .site(site),
                            );
                            continue;
                        }
                        *slot = Some(set);
                    }
                }
            }
        }
    }
    // S-G3: every debug-bearing unit has both sets.
    for (key, b) in &bearing {
        let cu = cu_of[key];
        let have = sets.get(&cu);
        for (w, n) in [(0, PUBNAMES), (1, PUBTYPES)] {
            if *b && have.is_none_or(|x| x[w].is_none()) {
                let o = &objs[key.0];
                out.push(
                    finding(7, format!("unit {cu} has debug information and no {n} set: the index is incomplete (S-G3: mortar refuses gdb-index-incomplete)"))
                        .site(o.site(key.1, key.2)),
                );
            }
        }
    }
    // S-G4: the names in name order, their values.
    let mut order: HashMap<&[u8], usize> = HashMap::default();
    for (&cu, pair) in &sets {
        for set in pair.iter().flatten() {
            for &(attr, name) in set {
                let i = *order.entry(name).or_insert_with(|| {
                    want.names.push((name, BTreeSet::new()));
                    want.names.len() - 1
                });
                want.names[i].1.insert(cu | u32::from(attr) << 24);
            }
        }
    }
    // P-G8: the index against the inputs.
    let held: HashMap<&[u8], BTreeSet<u32>> =
        entries.iter().map(|e| (e.name, e.values.iter().copied().collect())).collect();
    let mut said = 0usize;
    let mut say = |out: &mut Vec<Finding>, m: String| {
        said += 1;
        if said <= 8 {
            out.push(finding(8, m));
        }
    };
    for (name, vals) in &want.names {
        match held.get(name) {
            None => {
                say(out, format!("'{}' of the inputs' name sets is not in the index", show(name)));
            }
            Some(h) if h != vals => say(
                out,
                format!(
                    "'{}' has values {:x?}; the inputs' name sets give {:x?}",
                    show(name),
                    h,
                    vals
                ),
            ),
            Some(_) => {}
        }
    }
    for e in entries {
        if !order.contains_key(e.name) {
            say(out, format!("'{}' is in the index and in no input name set", show(e.name)));
        }
    }
    if said > 8 {
        out.push(finding(8, format!("and {} more names", said - 8)));
    }
    if want.area != index.area {
        let i = want
            .area
            .iter()
            .zip(&index.area)
            .position(|(a, b)| a != b)
            .unwrap_or(want.area.len().min(index.area.len()));
        out.push(finding(
            8,
            format!(
                "the address area ({} entries) is not S-G5's for the inputs ({} entries): first difference at entry {i}, index {:x?}, inputs {:x?}",
                index.area.len(),
                want.area.len(),
                index.area.get(i),
                want.area.get(i)
            ),
        ));
    }
    if clean && out.len() == before {
        bytes(index, &want, out);
    }
}

/// One set at `p` of section `s`: the next set's offset, the unit it
/// names (section, offset), its entries (S-G4a).
fn read_set<'v>(
    v: &'v InView<'_, '_>,
    s: usize,
    p: u64,
) -> Result<(u64, (usize, u64), Set<'v>), String> {
    let b: &'v [u8] = v.bytes(s);
    let mut r = gdbdwarf::Rd::new(b, p as usize);
    let len = r.u32()?;
    if len == 0xffff_ffff {
        return Err("a DWARF64 set".into());
    }
    let end = (p as usize)
        .checked_add(4)
        .and_then(|e| e.checked_add(len as usize))
        .filter(|&e| e <= b.len());
    let Some(end) = end else {
        return Err(format!("unit_length {len:#x} runs past the section's end"));
    };
    let version = r.u16()?;
    if version != 2 {
        return Err(format!("version {version}"));
    }
    let at = r.p as u64;
    let unit = v.offset_into(s, at, ".debug_info")?;
    r.u32()?;
    r.u32()?;
    let mut r = gdbdwarf::Rd::new(&b[..end], r.p);
    let mut set = Vec::new();
    loop {
        let die = r.u32()?;
        if die == 0 {
            if r.p != end {
                return Err("bytes after the terminator".into());
            }
            break;
        }
        let attr = r.u8()?;
        if attr & 0x0f != 0 || (attr >> 4) & 7 >= 5 {
            return Err(format!("attribute byte {attr:#04x}"));
        }
        set.push((attr, r.cstr()?));
    }
    Ok((end as u64, unit, set))
}

/// P-G9: the bytes S-G7 gives for the content.
fn bytes(index: &Index<'_>, want: &Want<'_>, out: &mut Vec<Finding>) {
    let (c, r, n) = (want.cus.len() as u64, want.area.len() as u64, want.names.len() as u64);
    let p = (4 * n / 3 + 1).next_power_of_two();
    let t = p.max(1024);
    let sym = 24 + 16 * c + 20 * r;
    let pool_off = sym + 8 * t;
    let mut pool = Vec::new();
    let mut vec_at = Vec::with_capacity(want.names.len());
    for (_, vals) in &want.names {
        vec_at.push(pool.len() as u32);
        pool.extend_from_slice(&(vals.len() as u32).to_le_bytes());
        for v in vals {
            pool.extend_from_slice(&v.to_le_bytes());
        }
    }
    let mut slots = vec![(0u32, 0u32); t as usize];
    for (i, (name, _)) in want.names.iter().enumerate() {
        let at = pool.len() as u32;
        pool.extend_from_slice(name);
        pool.push(0);
        if let Some(s) = probe(hash(name), t as u32).find(|&s| slots[s as usize] == (0, 0)) {
            slots[s as usize] = (at, vec_at[i]);
        }
    }
    let mut b = Vec::new();
    for w in [7u64, 24, 24 + 16 * c, 24 + 16 * c, sym, pool_off] {
        b.extend_from_slice(&(w as u32).to_le_bytes());
    }
    for &(o, l) in &want.cus {
        b.extend_from_slice(&o.to_le_bytes());
        b.extend_from_slice(&l.to_le_bytes());
    }
    for &(lo, hi, cu) in &want.area {
        b.extend_from_slice(&lo.to_le_bytes());
        b.extend_from_slice(&hi.to_le_bytes());
        b.extend_from_slice(&cu.to_le_bytes());
    }
    for (a, v) in &slots {
        b.extend_from_slice(&a.to_le_bytes());
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(&pool);
    let got = index.data;
    if index.version != 7 {
        out.push(finding(9, format!("version {}; S-G7 writes 7", index.version)));
        return;
    }
    if got == b.as_slice() {
        return;
    }
    let at = got.iter().zip(&b).position(|(x, y)| x != y).unwrap_or(got.len().min(b.len())) as u64;
    let area = if at < 24 {
        "the header".to_string()
    } else if at < sym {
        "the CU list or the address area".to_string()
    } else if at < pool_off {
        format!("the symbol table (slot {})", (at - sym) / 8)
    } else {
        "the constant pool".to_string()
    };
    let mut f = finding(
        9,
        format!(
            "the bytes are not S-G7's ({} bytes, S-G7a gives {}; {t} slots for {n} names): first difference at {at:#x}, in {area}",
            got.len(),
            b.len()
        ),
    );
    f.expected = b.get(at as usize).map(|&x| u64::from(x));
    f.found = got.get(at as usize).map(|&x| u64::from(x));
    out.push(f);
}

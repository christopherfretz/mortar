//! The checks on foreign objects, W1-W4 (`spec/odr.md` §8.1): they read
//! only what resolution holds already, the symbol tables of the files that
//! keep a copy of a COMDAT group, and for W4 (`--odr=strict` only) the
//! copies' bytes and relocations.
//!
//! What is compared: every global definition in a section of a live COMDAT
//! copy (`Resolution::comdat_copies`), grouped by key, and for W2 and W3
//! the key's resolved definition (`Resolution::def`). A key no COMDAT copy
//! defines is not looked at, so a link without COMDAT groups costs one
//! test; a definition that is neither in a COMDAT copy nor the resolved
//! one (a weak definition outside any group that lost) is not compared.
//! That is §8.1's "what resolution already holds"; every finding here is
//! one `wcheck.py` (the rule text's reference) also makes.

use std::borrow::Cow;

use mortar_core::inputs::Reader;
use mortar_core::par::Pool;
use mortar_core::resolution::Def;
use mortar_core::{ComdatId, FileId, Inputs, Resolution, SymEntry, SymbolId};
use mortar_diag::{Diagnostic, Key, Kind, Place, Relation};
use mortar_elf::consts::{
    SHT_NOBITS, SHT_REL, SHT_RELA, STB_GLOBAL, STB_LOCAL, STT_COMMON, STT_FUNC, STT_GNU_IFUNC,
    STT_OBJECT, STT_SECTION, STT_TLS,
};
use mortar_elf::{Object, SectionIndex, SymbolIndex, SymbolSection, SymbolTable};

/// The phase of the one-definition-rule checks (`spec/model.md` §8).
pub const PHASE: u16 = 40;

/// The most files a class of definitions lists (§9.1); the rest are
/// counted.
const FILES_A_CLASS: usize = 3;

/// Which of W1-W4 run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools, reason = "one switch per W check")]
pub struct Checks {
    pub w1: bool,
    pub w2: bool,
    pub w3: bool,
    pub w4: bool,
}

impl Checks {
    /// Whether any runs.
    #[must_use]
    pub const fn any(self) -> bool {
        self.w1 || self.w2 || self.w3 || self.w4
    }
}

/// What the foreign checks counted, for `--odr-stats` (O-44).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    /// Files read: those that keep a copy of some COMDAT group.
    pub files: u64,
    /// Live COMDAT groups, and copies of them.
    pub groups: u64,
    pub copies: u64,
    /// Global definitions in the copies.
    pub definitions: u64,
    /// Findings of W1-W4.
    pub w: [u64; 4],
}

/// A global definition in a section of a COMDAT copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Held {
    key: SymbolId,
    group: ComdatId,
    file: FileId,
    index: SymbolIndex,
    section: SectionIndex,
    sym_type: u8,
    size: u64,
    /// The symbol covers its whole section (`st_value` 0, `st_size` the
    /// section's size): W4 compares the section.
    whole: bool,
}

/// One file's definition: where, and what kind (W2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct At {
    file: FileId,
    index: SymbolIndex,
}

/// A finding before it is a diagnostic: the classes of definitions that
/// agree, the first the one kept or used, and the notes that explain.
struct Finding {
    kind: Kind,
    classes: Vec<Vec<At>>,
    notes: Vec<String>,
}

/// Runs the checks `checks` asks for. The diagnostics are warnings
/// (O-40, O-41: no W check is an error in any mode), keyed per §9.1.
#[must_use]
pub fn check(
    inputs: &Inputs<'_>,
    resolution: &Resolution,
    checks: Checks,
    pool: &Pool,
) -> (Vec<(Key, Diagnostic)>, Counts) {
    let mut counts = Counts::default();
    if !checks.any() || resolution.comdat_copies.is_empty() {
        return (Vec::new(), counts);
    }
    let copies = &resolution.comdat_copies;
    counts.copies = copies.len() as u64;
    counts.groups = copies.windows(2).filter(|w| w[0].0 != w[1].0).count() as u64 + 1;
    let mut files: Vec<FileId> = copies.iter().map(|c| c.1).collect();
    pool.sort_by_key(&mut files, |f| *f);
    files.dedup();
    counts.files = files.len() as u64;

    let got = pool.unit_cost(2000).map_collect(files.len(), |u, out: &mut Vec<Held>| {
        definitions(inputs, resolution, files[u], out);
    });
    let mut defs = got.items;
    counts.definitions = defs.len() as u64;
    pool.sort_by_key(&mut defs, |d| (d.key, d.group, d.file, d.index));

    // The keys with a run of definitions; each is checked on its own.
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut start = 0;
    for i in 1..=defs.len() {
        if i == defs.len() || defs[i].key != defs[start].key {
            runs.push((start, i));
            start = i;
        }
    }
    let found =
        pool.unit_cost(500).map_collect(runs.len(), |u, out: &mut Vec<(usize, Finding)>| {
            let (a, b) = runs[u];
            let cx = Cx { inputs, resolution };
            for (w, finding) in cx.key(&defs[a..b], checks) {
                out.push((w, finding));
            }
        });
    let mut diagnostics = Vec::with_capacity(found.items.len());
    for (w, finding) in found.items {
        counts.w[w] += 1;
        diagnostics.push(diagnostic(inputs, finding));
    }
    (diagnostics, counts)
}

/// The global definitions of `file` in sections of its COMDAT copies.
fn definitions(inputs: &Inputs<'_>, resolution: &Resolution, file: FileId, out: &mut Vec<Held>) {
    let Some((obj, table)) = object(inputs, file) else { return };
    let keys = inputs.sym_map_of(file);
    let first = table.first_global().0;
    for (index, sym) in table.globals() {
        if sym.binding() == STB_LOCAL {
            continue;
        }
        let Ok(SymbolSection::Section(section)) = table.section_of(index, &sym) else { continue };
        let Some(row) =
            resolution.section_id(file, section).and_then(|s| resolution.sections.get(s))
        else {
            continue;
        };
        let Some(group) = row.comdat.get() else { continue };
        let Some(&key) = keys.get((index.0 - first) as usize) else { continue };
        let whole = sym.st_value == 0
            && obj.section(section).is_ok_and(|s| s.header().sh_size == sym.st_size);
        out.push(Held {
            key,
            group,
            file,
            index,
            section,
            sym_type: sym.sym_type(),
            size: sym.st_size,
            whole,
        });
    }
}

/// A live object's reader and symbol table.
fn object<'s>(inputs: &Inputs<'s>, file: FileId) -> Option<(Object<'s>, SymbolTable<'s>)> {
    let Reader::Object(obj) = inputs.files.get(file)?.reader else { return None };
    let table = obj.symbols().ok()??;
    Some((obj, table))
}

/// W2's two sides; anything else (`STT_NOTYPE`, `STT_SECTION`, OS and
/// processor types) does not take part.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Function,
    Data,
}

const fn side(sym_type: u8, common: bool) -> Option<Side> {
    if common {
        return Some(Side::Data);
    }
    match sym_type {
        STT_FUNC | STT_GNU_IFUNC => Some(Side::Function),
        STT_OBJECT | STT_TLS | STT_COMMON => Some(Side::Data),
        _ => None,
    }
}

/// Whether W1 and W2 cannot fire among the copies alone: every copy on
/// the same W2 side (or none), and in each group the data copies of one
/// size. Reads only `defs`, allocates nothing.
fn agree(defs: &[Held]) -> bool {
    let first = side(defs[0].sym_type, false);
    if defs.iter().any(|d| side(d.sym_type, false) != first) {
        return false;
    }
    defs.chunk_by(|a, b| a.group == b.group).all(|run| {
        let mut data = run.iter().filter(|d| is_data(d.sym_type));
        data.next().is_none_or(|d0| data.all(|d| d.size == d0.size))
    })
}

const fn is_data(sym_type: u8) -> bool {
    matches!(sym_type, STT_OBJECT | STT_TLS)
}

fn type_word(sym_type: u8, common: bool) -> &'static str {
    if common {
        return "a common symbol";
    }
    match sym_type {
        STT_FUNC => "STT_FUNC",
        STT_GNU_IFUNC => "STT_GNU_IFUNC",
        STT_OBJECT => "STT_OBJECT",
        STT_TLS => "STT_TLS",
        STT_COMMON => "STT_COMMON",
        _ => "another type",
    }
}

/// The resolved definition of a key, when an object defines it outside
/// any COMDAT group: its place, binding, type, whether it is common, and
/// whether it is in a section or absolute.
struct Resolved {
    at: At,
    binding: u8,
    sym_type: u8,
    common: bool,
}

struct Cx<'a, 's> {
    inputs: &'a Inputs<'s>,
    resolution: &'a Resolution,
}

impl Cx<'_, '_> {
    fn group_name(&self, group: ComdatId) -> String {
        self.inputs
            .comdats
            .get(group)
            .map_or_else(String::new, |c| String::from_utf8_lossy(c.name).into_owned())
    }

    fn kept(&self, group: ComdatId, file: FileId) -> bool {
        self.resolution.comdat.get(group).and_then(|k| k.get()) == Some(file)
    }

    /// The key's resolved definition when it is an object's, outside every
    /// COMDAT group (one inside is among the copies already).
    fn resolved(&self, key: SymbolId) -> Option<Resolved> {
        let (entry, common): (SymEntry, bool) = match *self.resolution.def.get(key)? {
            Def::Defined(e) => (e, false),
            Def::Common(e) => (e, true),
            Def::Shared(_) | Def::Internal(_) | Def::Undefined { .. } => return None,
        };
        let (_, table) = object(self.inputs, entry.file)?;
        let sym = table.get(entry.index).ok()?;
        let in_section = match table.section_of(entry.index, &sym).ok()? {
            SymbolSection::Section(s) => Some(s),
            SymbolSection::Absolute | SymbolSection::Common => None,
            SymbolSection::Undefined | SymbolSection::Reserved(_) => {
                if !common {
                    return None;
                }
                None
            }
        };
        if let Some(section) = in_section {
            let row = self
                .resolution
                .section_id(entry.file, section)
                .and_then(|s| self.resolution.sections.get(s))?;
            if row.comdat.is_some() {
                return None;
            }
        }
        Some(Resolved {
            at: At { file: entry.file, index: entry.index },
            binding: sym.binding(),
            sym_type: sym.sym_type(),
            common,
        })
    }

    fn name(&self, key: SymbolId) -> String {
        let Some(k) = self.inputs.symbols.get(key) else { return String::new() };
        let mut name = String::from_utf8_lossy(k.name).into_owned();
        if let Some(version) = k.version {
            name.push('@');
            name.push_str(&String::from_utf8_lossy(version));
        }
        name
    }

    /// The findings for one key, `defs` its COMDAT definitions sorted by
    /// (group, file); each with the index of its W.
    fn key(&self, defs: &[Held], checks: Checks) -> Vec<(usize, Finding)> {
        let mut out = Vec::new();
        let key = defs[0].key;
        // The resolved definition takes part only when it is not one of
        // the copies (a copy's is inside a group, so `resolved` would give
        // nothing): test by id before reading any symbol table.
        let outside = match self.resolution.def.get(key) {
            Some(Def::Defined(e) | Def::Common(e)) => {
                !defs.iter().any(|d| d.file == e.file && d.index == e.index)
            }
            _ => false,
        };
        // The default mode's fast path: one definition, or copies that agree
        // on W1's and W2's cheap tests, and nothing outside: no work.
        if !checks.w3 && !checks.w4 && !outside && agree(defs) {
            return out;
        }
        let resolved = if (checks.w2 || checks.w3) && outside { self.resolved(key) } else { None };
        if checks.w1 {
            for run in defs.chunk_by(|a, b| a.group == b.group) {
                if let Some(f) = self.w1(key, run) {
                    out.push((0, f));
                }
            }
        }
        if checks.w2
            && let Some(f) = self.w2(key, defs, resolved.as_ref())
        {
            out.push((1, f));
        }
        if checks.w3
            && let Some(r) = &resolved
            && let Some(f) = self.w3(key, defs, r)
        {
            out.push((2, f));
        }
        if checks.w4
            && let Some(f) = self.w4(key, defs)
        {
            out.push((3, f));
        }
        out
    }

    /// W1: the data definitions in the copies of one group, by size.
    fn w1(&self, key: SymbolId, run: &[Held]) -> Option<Finding> {
        let data: Vec<&Held> = run.iter().filter(|d| is_data(d.sym_type)).collect();
        let mut sizes: Vec<u64> = Vec::new();
        let mut classes: Vec<Vec<At>> = Vec::new();
        for d in &data {
            let at = At { file: d.file, index: d.index };
            if let Some(c) = sizes.iter().position(|s| *s == d.size) {
                classes[c].push(at);
            } else {
                sizes.push(d.size);
                classes.push(vec![at]);
            }
        }
        if classes.len() < 2 {
            return None;
        }
        let group = run[0].group;
        let words: Vec<String> = sizes.iter().map(u64::to_string).collect();
        let mut note = format!(
            "st_size {} in the order above, in the copies of COMDAT group {}",
            list(&words),
            self.group_name(group)
        );
        if self.kept(group, data[0].file) {
            note.push_str("; the first is kept");
        }
        Some(Finding {
            kind: Kind::OdrSizeMismatch { name: self.name(key) },
            classes,
            notes: vec![note],
        })
    }

    /// W2: a function on one side, data on the other, over the copies and
    /// the resolved definition (first, when it takes part).
    fn w2(&self, key: SymbolId, defs: &[Held], resolved: Option<&Resolved>) -> Option<Finding> {
        let mut sides: Vec<(Side, &'static str)> = Vec::new();
        let mut classes: Vec<Vec<At>> = Vec::new();
        let mut add = |side: Side, word: &'static str, at: At| {
            if let Some(c) = sides.iter().position(|s| s.0 == side) {
                if !classes[c].contains(&at) {
                    classes[c].push(at);
                }
            } else {
                sides.push((side, word));
                classes.push(vec![at]);
            }
        };
        let used = resolved.and_then(|r| side(r.sym_type, r.common).map(|s| (s, r)));
        if let Some((s, r)) = used {
            add(s, type_word(r.sym_type, r.common), r.at);
        }
        let mut sorted: Vec<&Held> = defs.iter().collect();
        sorted.sort_by_key(|d| (d.file, d.index));
        for d in sorted {
            if let Some(s) = side(d.sym_type, false) {
                add(s, type_word(d.sym_type, false), At { file: d.file, index: d.index });
            }
        }
        if classes.len() < 2 {
            return None;
        }
        let mut note = format!("{} in the first, {} in the second", sides[0].1, sides[1].1);
        if used.is_some() {
            note.push_str("; the first is the definition used");
        }
        Some(Finding {
            kind: Kind::OdrSymbolTypeMismatch { name: self.name(key) },
            classes,
            notes: vec![note],
        })
    }

    /// W3: a strong (`STB_GLOBAL`) definition outside every COMDAT group,
    /// the one used, and a COMDAT copy of the same key.
    fn w3(&self, key: SymbolId, defs: &[Held], resolved: &Resolved) -> Option<Finding> {
        if resolved.binding != STB_GLOBAL || resolved.common {
            return None;
        }
        let mut copies: Vec<At> =
            defs.iter().map(|d| At { file: d.file, index: d.index }).collect();
        copies.sort_by_key(|a| (a.file, a.index));
        copies.dedup_by_key(|a| a.file);
        let note = format!(
            "the first is outside any COMDAT group and is the definition used; the second is \
             in COMDAT group {}",
            self.group_name(defs[0].group)
        );
        Some(Finding {
            kind: Kind::OdrStrongAndComdat { name: self.name(key) },
            classes: vec![vec![resolved.at], copies],
            notes: vec![note],
        })
    }

    /// W4: the data definitions that cover their section, by the
    /// section's contents (bytes, or the size of `SHT_NOBITS`) and its
    /// relocations with symbols named (§8.1, `weakcheck.py`). A copy whose
    /// contents cannot be read is left out (no finding from it).
    fn w4(&self, key: SymbolId, defs: &[Held]) -> Option<Finding> {
        let mut encodings: Vec<Contents> = Vec::new();
        let mut classes: Vec<Vec<At>> = Vec::new();
        let mut groups: Vec<ComdatId> = Vec::new();
        let mut sorted: Vec<&Held> =
            defs.iter().filter(|d| is_data(d.sym_type) && d.whole).collect();
        if sorted.len() < 2 {
            return None;
        }
        sorted.sort_by_key(|d| (d.file, d.index));
        for d in sorted {
            let Some(contents) = self.contents(d.file, d.section) else { continue };
            let at = At { file: d.file, index: d.index };
            if let Some(c) = encodings.iter().position(|e| *e == contents) {
                classes[c].push(at);
            } else {
                encodings.push(contents);
                classes.push(vec![at]);
                groups.push(d.group);
            }
        }
        if classes.len() < 2 {
            return None;
        }
        let (a, b) = (&encodings[0], &encodings[1]);
        let what = match (a.bytes != b.bytes, a.relocs != b.relocs) {
            (true, true) => "bytes and relocations",
            (true, false) => "bytes",
            _ => "relocations",
        };
        let mut note = format!(
            "the second differs from the first in its {what}, in the copies of COMDAT group {}",
            self.group_name(groups[1])
        );
        if self.kept(groups[0], classes[0][0].file) {
            note.push_str("; the first is kept");
        }
        Some(Finding {
            kind: Kind::OdrContentsMismatch { name: self.name(key) },
            classes,
            notes: vec![note],
        })
    }

    /// A section's contents and its relocations, symbols by name: section
    /// symbols `[sec]<section name>`, locals `[local]<name>`.
    fn contents(&self, file: FileId, section: SectionIndex) -> Option<Contents> {
        let (obj, table) = object(self.inputs, file)?;
        let sec = obj.section(section).ok()?;
        let bytes = if sec.sh_type() == SHT_NOBITS {
            Bytes::Nobits(sec.size())
        } else {
            Bytes::Data(self.data(file, &obj, section)?.into_owned())
        };
        let mut relocs: Vec<(u64, u32, Vec<u8>, i64)> = Vec::new();
        let row = self
            .resolution
            .section_id(file, section)
            .and_then(|s| self.resolution.sections.get(s))?;
        if row.reloc.0 != 0 {
            let rsec = obj.section(row.reloc).ok()?;
            let raw = self.data(file, &obj, row.reloc)?;
            let mut push = |offset: u64, r_type: u32, r_sym: u32, addend: i64| -> Option<()> {
                let name = reloc_name(&obj, &table, r_sym)?;
                relocs.push((offset, r_type, name, addend));
                Some(())
            };
            match rsec.sh_type() {
                SHT_RELA => {
                    for r in rsec.rela_in(&raw).ok()? {
                        push(r.r_offset, r.r_type, r.r_sym, r.r_addend)?;
                    }
                }
                SHT_REL => {
                    for r in rsec.rel_in(&raw).ok()? {
                        push(r.r_offset, r.r_type, r.r_sym, 0)?;
                    }
                }
                _ => return None,
            }
        }
        relocs.sort_unstable();
        Some(Contents { bytes, relocs })
    }

    /// A section's bytes in the file: borrowed when resident, else copied
    /// from the store (`spec/input.md` §11.3).
    fn data<'o>(
        &self,
        file: FileId,
        obj: &Object<'o>,
        section: SectionIndex,
    ) -> Option<Cow<'o, [u8]>> {
        let sec = obj.section(section).ok()?;
        if let Ok(data) = sec.data() {
            return Some(Cow::Borrowed(data));
        }
        let range = sec.file_range().ok()??;
        let mut buf = vec![0; range.len()];
        self.inputs.copy_to(file, range.start as u64..range.end as u64, &mut buf).ok()?;
        Some(Cow::Owned(buf))
    }
}

/// A relocation's symbol, named so that two files compare.
fn reloc_name(obj: &Object<'_>, table: &SymbolTable<'_>, r_sym: u32) -> Option<Vec<u8>> {
    let index = SymbolIndex(r_sym);
    let sym = table.get(index).ok()?;
    let mut name = Vec::new();
    if sym.sym_type() == STT_SECTION {
        name.extend_from_slice(b"[sec]");
        if let Ok(SymbolSection::Section(s)) = table.section_of(index, &sym) {
            name.extend_from_slice(obj.section(s).ok()?.name().ok()?);
        }
    } else {
        if sym.binding() == STB_LOCAL {
            name.extend_from_slice(b"[local]");
        }
        name.extend_from_slice(table.name(&sym).ok()?);
    }
    Some(name)
}

#[derive(Debug, PartialEq, Eq)]
enum Bytes {
    Data(Vec<u8>),
    Nobits(u64),
}

#[derive(Debug, PartialEq, Eq)]
struct Contents {
    bytes: Bytes,
    relocs: Vec<(u64, u32, Vec<u8>, i64)>,
}

/// `a, b and c`.
fn list(words: &[String]) -> String {
    match words {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// The diagnostic of a finding (§9.1): the first class "defined in", the
/// others "defined differently in", at most three files a class and a
/// count of the rest, then the notes. Its key: phase 40, the first file
/// of the second class and its symbol-table index.
fn diagnostic(inputs: &Inputs<'_>, finding: Finding) -> (Key, Diagnostic) {
    let first_other = finding.classes[1][0];
    let key = Key::new(PHASE, first_other.file.0, u64::from(first_other.index.0));
    let name = finding.kind.symbol().unwrap_or_default().to_owned();
    let mut d = Diagnostic::warning(finding.kind);
    for (c, class) in finding.classes.iter().enumerate() {
        let relation = if c == 0 { Relation::DefinedIn } else { Relation::DefinedDifferentlyIn };
        for at in class.iter().take(FILES_A_CLASS) {
            let file = inputs.files.get(at.file).map(mortar_core::inputs::File::display);
            if let Some(file) = file {
                d = d.with(relation, Place::symbol(file, name.clone()));
            }
        }
        if class.len() > FILES_A_CLASS {
            let rest = class.len() - FILES_A_CLASS;
            let files = if rest == 1 { "file" } else { "files" };
            d = d.note(format!("and {rest} more {files} with the same definition"));
        }
    }
    for note in finding.notes {
        d = d.note(note);
    }
    (key, d)
}

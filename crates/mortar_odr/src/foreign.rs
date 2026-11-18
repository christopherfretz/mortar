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
use mortar_diag::{Diagnostic, FileRef, Key, Kind, Place, Relation};
use mortar_elf::consts::{
    STB_GLOBAL, STB_LOCAL, STT_COMMON, STT_FUNC, STT_GNU_IFUNC, STT_OBJECT, STT_TLS,
};
use mortar_elf::{Object, SectionIndex, SymbolIndex, SymbolSection, SymbolTable};

use crate::contents::{self, Contents};

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

/// A global definition in a section of a COMDAT copy (what [`copies`]
/// and [`definitions_in`] give; an incremental state's `COPIES` row is
/// made from one, `spec/incr.md` IN-71d, WP-220).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Definition {
    pub key: SymbolId,
    pub group: ComdatId,
    pub file: FileId,
    pub index: SymbolIndex,
    pub section: SectionIndex,
    pub sym_type: u8,
    pub size: u64,
    /// The symbol covers its whole section (`st_value` 0, `st_size` the
    /// section's size): W4 compares the section.
    pub whole: bool,
}

/// One file's definition: where (its file and symbol-table index).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct At {
    pub file: FileId,
    pub index: SymbolIndex,
}

/// What W1 and W2 read of a COMDAT copy's global definition: the full
/// link's [`Definition`] and an incremental update's stored row alike
/// (WP-220: one comparator, [`compare_copies`]). `Group` identifies the
/// copy's COMDAT group: equal for two copies of one group.
///
/// "First" is by load order: [`CopyDef::rank`], never the `FileId` alone.
/// A full link's `FileId` order is its load order; an updated state's
/// `FILES` rows are append-only, so a file that appears in an update has
/// an appended row whose `FILES.rank` is not last (`spec/incr.md`
/// IN-18e). The update passes each copy's CURRENT rank (the stored
/// `FILES.rank` composed with the update's new ranks).
pub trait CopyDef {
    type Group: Copy + Eq;
    fn group(&self) -> Self::Group;
    fn at(&self) -> At;
    /// The copy's file's load-order position: what orders copies.
    fn rank(&self) -> u64;
    fn sym_type(&self) -> u8;
    fn size(&self) -> u64;
}

impl CopyDef for Definition {
    type Group = ComdatId;
    fn group(&self) -> ComdatId {
        self.group
    }
    fn at(&self) -> At {
        At { file: self.file, index: self.index }
    }
    /// A full link's load order is its `FileId` order.
    fn rank(&self) -> u64 {
        u64::from(self.file.0)
    }
    fn sym_type(&self) -> u8 {
        self.sym_type
    }
    fn size(&self) -> u64 {
        self.size
    }
}

/// The names and the keeper the text of W1 and W2 reads: the full link's
/// from its inputs and resolution, an update's from its state.
pub trait Names<G> {
    /// The key's name as a diagnostic writes it (`name@version`).
    fn symbol(&self, key: SymbolId) -> String;
    /// The group's signature.
    fn group(&self, group: G) -> String;
    /// Whether `file` keeps `group`.
    fn kept(&self, group: G, file: FileId) -> bool;
}

/// A finding before it is a diagnostic: the key, the classes of
/// definitions that agree, the first the one kept or used, and the notes
/// that explain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub key: SymbolId,
    pub kind: Kind,
    pub classes: Vec<Vec<At>>,
    pub notes: Vec<String>,
}

/// A W diagnostic's subjects (`spec/incr.md` IN-75a, WP-220): the symbol
/// and the files the diagnostic names, sorted, each once. The one
/// assignment, for a full link's capture and an update alike.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subjects {
    pub key: Key,
    pub symbol: SymbolId,
    pub files: Vec<FileId>,
}

/// Runs the checks `checks` asks for. The diagnostics are warnings
/// (O-40, O-41: no W check is an error in any mode), keyed per §9.1.
/// `stds`: the `std` of the units of each file with a container, sorted
/// by file, for W4's note (§8.1, WP-151); the record checks give it, and
/// it is empty when they read no container.
#[must_use]
pub fn check(
    inputs: &Inputs<'_>,
    resolution: &Resolution,
    checks: Checks,
    stds: &[(FileId, Vec<u8>)],
    pool: &Pool,
) -> (Vec<(Key, Diagnostic)>, Vec<Subjects>, Counts) {
    let mut counts = Counts::default();
    if !checks.any() || resolution.comdat_copies.is_empty() {
        return (Vec::new(), Vec::new(), counts);
    }
    let copies = &resolution.comdat_copies;
    counts.copies = copies.len() as u64;
    counts.groups = copies.windows(2).filter(|w| w[0].0 != w[1].0).count() as u64 + 1;
    let (defs, files) = collect(inputs, resolution, pool);
    counts.files = files;
    counts.definitions = defs.len() as u64;

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
            let cx = Cx { inputs, resolution, stds };
            for (w, finding) in cx.key(&defs[a..b], checks) {
                out.push((w, finding));
            }
        });
    let mut diagnostics = Vec::with_capacity(found.items.len());
    let mut subjects = Vec::with_capacity(found.items.len());
    let display = |file: FileId| inputs.files.get(file).map(mortar_core::inputs::File::display);
    for (w, finding) in found.items {
        counts.w[w] += 1;
        let (key, d, s) = diagnostic(&display, finding);
        diagnostics.push((key, d));
        subjects.push(s);
    }
    (diagnostics, subjects, counts)
}

/// Every global definition in a section of a live COMDAT copy, sorted by
/// (key, group, file, index): what W1-W4 compare, and what an incremental
/// state stores of it (`COPIES`, WP-220). Reads the symbol tables of the
/// files that keep a copy.
#[must_use]
pub fn copies(inputs: &Inputs<'_>, resolution: &Resolution, pool: &Pool) -> Vec<Definition> {
    collect(inputs, resolution, pool).0
}

/// [`copies`] and the count of files read.
fn collect(inputs: &Inputs<'_>, resolution: &Resolution, pool: &Pool) -> (Vec<Definition>, u64) {
    let mut files: Vec<FileId> = resolution.comdat_copies.iter().map(|c| c.1).collect();
    pool.sort_by_key(&mut files, |f| *f);
    files.dedup();
    let got = pool.unit_cost(2000).map_collect(files.len(), |u, out: &mut Vec<Definition>| {
        definitions(inputs, resolution, files[u], out);
    });
    let mut defs = got.items;
    pool.sort_by_key(&mut defs, |d| (d.key, d.group, d.file, d.index));
    (defs, files.len() as u64)
}

/// The global definitions of one file in sections of its COMDAT copies,
/// in symbol-table order (an update's changed file, WP-220).
#[must_use]
pub fn definitions_in(
    inputs: &Inputs<'_>,
    resolution: &Resolution,
    file: FileId,
) -> Vec<Definition> {
    let mut out = Vec::new();
    definitions(inputs, resolution, file, &mut out);
    out
}

/// The global definitions of `file` in sections of its COMDAT copies.
fn definitions(
    inputs: &Inputs<'_>,
    resolution: &Resolution,
    file: FileId,
    out: &mut Vec<Definition>,
) {
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
        out.push(Definition {
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
fn agree<D: CopyDef>(defs: &[D]) -> bool {
    let Some(d0) = defs.first() else { return true };
    let first = side(d0.sym_type(), false);
    if defs.iter().any(|d| side(d.sym_type(), false) != first) {
        return false;
    }
    defs.chunk_by(|a, b| a.group() == b.group()).all(|run| {
        let mut data = run.iter().filter(|d| is_data(d.sym_type()));
        data.next().is_none_or(|d0| data.all(|d| d.size() == d0.size()))
    })
}

/// W1 and W2 for one key (`spec/odr.md` §8.1), the full link's and an
/// incremental update's one comparator (WP-220): `defs` are the key's
/// COMDAT definitions, contiguous by group and sorted by
/// ([`CopyDef::rank`], index) within a group (the order of each class's
/// files, its first file, and so §9.1's key); `outside` says the key's resolved definition is not
/// one of them, and `resolved` gives it (called only when W2 runs and
/// `outside`). The default mode's fast path ([`agree`]) is inside: copies
/// that agree and nothing outside cost no allocation. Each finding comes
/// with its W's index (0 for W1, 1 for W2).
#[must_use]
pub fn compare_copies<D: CopyDef>(
    key: SymbolId,
    defs: &[D],
    outside: bool,
    resolved: impl FnOnce() -> Option<Resolved>,
    names: &impl Names<D::Group>,
    w1: bool,
    w2: bool,
) -> Vec<(usize, Finding)> {
    let mut out = Vec::new();
    if defs.is_empty() || (!outside && agree(defs)) {
        return out;
    }
    let resolved = if w2 && outside { resolved() } else { None };
    w12(key, defs, resolved.as_ref(), names, w1, w2, &mut out);
    out
}

/// W1 per group, then W2, into `out`.
fn w12<D: CopyDef>(
    key: SymbolId,
    defs: &[D],
    resolved: Option<&Resolved>,
    names: &impl Names<D::Group>,
    w1: bool,
    w2: bool,
    out: &mut Vec<(usize, Finding)>,
) {
    if w1 {
        for run in defs.chunk_by(|a, b| a.group() == b.group()) {
            if let Some(f) = check_w1(key, run, names) {
                out.push((0, f));
            }
        }
    }
    if w2 && let Some(f) = check_w2(key, defs, resolved, names) {
        out.push((1, f));
    }
}

/// W1: the data definitions in the copies of one group, by size.
fn check_w1<D: CopyDef>(key: SymbolId, run: &[D], names: &impl Names<D::Group>) -> Option<Finding> {
    let data: Vec<&D> = run.iter().filter(|d| is_data(d.sym_type())).collect();
    let mut sizes: Vec<u64> = Vec::new();
    let mut classes: Vec<Vec<At>> = Vec::new();
    for d in &data {
        let at = d.at();
        if let Some(c) = sizes.iter().position(|s| *s == d.size()) {
            classes[c].push(at);
        } else {
            sizes.push(d.size());
            classes.push(vec![at]);
        }
    }
    if classes.len() < 2 {
        return None;
    }
    let group = run[0].group();
    let words: Vec<String> = sizes.iter().map(u64::to_string).collect();
    let mut note = format!(
        "st_size {} in the order above, in the copies of COMDAT group {}",
        list(&words),
        names.group(group)
    );
    if names.kept(group, data[0].at().file) {
        note.push_str("; the first is kept");
    }
    Some(Finding {
        key,
        kind: Kind::OdrSizeMismatch { name: names.symbol(key) },
        classes,
        notes: vec![note],
    })
}

/// W2: a function on one side, data on the other, over the copies and
/// the resolved definition (first, when it takes part).
fn check_w2<D: CopyDef>(
    key: SymbolId,
    defs: &[D],
    resolved: Option<&Resolved>,
    names: &impl Names<D::Group>,
) -> Option<Finding> {
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
    let mut sorted: Vec<&D> = defs.iter().collect();
    sorted.sort_by_key(|d| (d.rank(), d.at().index));
    for d in sorted {
        if let Some(s) = side(d.sym_type(), false) {
            add(s, type_word(d.sym_type(), false), d.at());
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
        key,
        kind: Kind::OdrSymbolTypeMismatch { name: names.symbol(key) },
        classes,
        notes: vec![note],
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
/// any COMDAT group: its place, binding, type, and whether it is common.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolved {
    pub at: At,
    pub binding: u8,
    pub sym_type: u8,
    pub common: bool,
}

struct Cx<'a, 's> {
    inputs: &'a Inputs<'s>,
    resolution: &'a Resolution,
    /// Per file with a container, the `std` of its units, sorted by file
    /// (W4's note, WP-151); empty when no container was read.
    stds: &'a [(FileId, Vec<u8>)],
}

impl Names<ComdatId> for Cx<'_, '_> {
    fn symbol(&self, key: SymbolId) -> String {
        self.name(key)
    }
    fn group(&self, group: ComdatId) -> String {
        self.group_name(group)
    }
    fn kept(&self, group: ComdatId, file: FileId) -> bool {
        self.resolution.comdat.get(group).and_then(|k| k.get()) == Some(file)
    }
}

impl Cx<'_, '_> {
    fn group_name(&self, group: ComdatId) -> String {
        self.inputs
            .comdats
            .get(group)
            .map_or_else(String::new, |c| String::from_utf8_lossy(c.name).into_owned())
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
    /// Whether the key's resolved definition takes part beside `defs`: an
    /// object's (defined or common) that is not one of the copies (a
    /// copy's is inside a group, so `resolved` would give nothing). By id,
    /// before any symbol table is read.
    fn outside(&self, key: SymbolId, defs: &[Definition]) -> bool {
        match self.resolution.def.get(key) {
            Some(Def::Defined(e) | Def::Common(e)) => {
                !defs.iter().any(|d| d.file == e.file && d.index == e.index)
            }
            _ => false,
        }
    }

    fn key(&self, defs: &[Definition], checks: Checks) -> Vec<(usize, Finding)> {
        let key = defs[0].key;
        let outside = self.outside(key, defs);
        // The default mode: W1 and W2 only, by the one comparator an
        // incremental update also calls (WP-220), its fast path inside.
        if !checks.w3 && !checks.w4 {
            return compare_copies(
                key,
                defs,
                outside,
                || self.resolved(key),
                self,
                checks.w1,
                checks.w2,
            );
        }
        let mut out = Vec::new();
        let resolved = if (checks.w2 || checks.w3) && outside { self.resolved(key) } else { None };
        w12(key, defs, resolved.as_ref(), self, checks.w1, checks.w2, &mut out);
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

    /// W3: a strong (`STB_GLOBAL`) definition outside every COMDAT group,
    /// the one used, and a COMDAT copy of the same key.
    fn w3(&self, key: SymbolId, defs: &[Definition], resolved: &Resolved) -> Option<Finding> {
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
            key,
            kind: Kind::OdrStrongAndComdat { name: self.name(key) },
            classes: vec![vec![resolved.at], copies],
            notes: vec![note],
        })
    }

    /// W4: the data definitions that cover their section, by the
    /// section's contents (bytes, or the size of `SHT_NOBITS`) and its
    /// relocations, each target a key by name and addend or a unit-local
    /// one by what it refers to (§8.1, W4's rule; `contents`). Each copy
    /// joins the first class whose first copy it is the same as. A copy
    /// whose contents cannot be read is left out (no finding from it).
    fn w4(&self, key: SymbolId, defs: &[Definition]) -> Option<Finding> {
        let mut encodings: Vec<Contents> = Vec::new();
        let mut classes: Vec<Vec<At>> = Vec::new();
        let mut groups: Vec<ComdatId> = Vec::new();
        let mut sorted: Vec<&Definition> =
            defs.iter().filter(|d| is_data(d.sym_type) && d.whole).collect();
        if sorted.len() < 2 {
            return None;
        }
        sorted.sort_by_key(|d| (d.file, d.index));
        for d in sorted {
            let Some(contents) = self.contents(d.file, d.section) else { continue };
            let at = At { file: d.file, index: d.index };
            if let Some(c) = encodings.iter().position(|e| contents::differ(e, &contents).is_none())
            {
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
        let what = contents::differ(&encodings[0], &encodings[1]).unwrap_or_default();
        let mut note = format!(
            "the second differs from the first in {what}, in the copies of COMDAT group {}",
            self.group_name(groups[1])
        );
        if self.kept(groups[0], classes[0][0].file) {
            note.push_str("; the first is kept");
        }
        let mut notes = vec![note];
        // §8.1 (WP-151), as O-32 says for rows: a static local may be
        // constant-initialized under one -std and dynamically under
        // another, which is legal.
        let std = |file: FileId| {
            self.stds.binary_search_by_key(&file, |(f, _)| *f).ok().map(|i| &self.stds[i].1)
        };
        if let (Some(a), Some(b)) = (std(classes[0][0].file), std(classes[1][0].file))
            && a != b
        {
            notes.push(format!(
                "the copies' translation units differ in -std (std={} and {}): the \
                 initialization of a static local, constant in one and dynamic in the other, may \
                 differ legally (spec/odr.md §8.1)",
                String::from_utf8_lossy(a),
                String::from_utf8_lossy(b)
            ));
        }
        Some(Finding {
            key,
            kind: Kind::OdrContentsMismatch { name: self.name(key) },
            classes,
            notes,
        })
    }

    /// A section's contents as W4 compares them (`contents::read`).
    fn contents(&self, file: FileId, section: SectionIndex) -> Option<Contents> {
        let (obj, table) = object(self.inputs, file)?;
        let row = self
            .resolution
            .section_id(file, section)
            .and_then(|s| self.resolution.sections.get(s))?;
        let reloc = (row.reloc.0 != 0).then_some(row.reloc);
        contents::read(&obj, &table, section, reloc, &|s| self.data(file, &obj, s))
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

/// What a full link gives [`compare_copies`] for `key` besides its
/// copies `defs` (the key's [`copies`]): whether its resolved definition
/// is outside them, and that definition as W2 reads it (`None` when it is
/// not outside, or not an object's). WP-220: an update derives the same
/// from its rows; this is the full link's side of that proof.
#[must_use]
pub fn resolved_of(
    inputs: &Inputs<'_>,
    resolution: &Resolution,
    key: SymbolId,
    defs: &[Definition],
) -> (bool, Option<Resolved>) {
    let cx = Cx { inputs, resolution, stds: &[] };
    let outside = cx.outside(key, defs);
    (outside, if outside { cx.resolved(key) } else { None })
}

/// The names and keepers a full link's W1 and W2 read ([`Names`] over its
/// inputs and resolution), for a caller of [`compare_copies`] outside
/// [`check`] (WP-220's proof that an update reads the same from rows).
#[must_use]
pub fn link_names<'a>(
    inputs: &'a Inputs<'_>,
    resolution: &'a Resolution,
) -> impl Names<ComdatId> + 'a {
    Cx { inputs, resolution, stds: &[] }
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
/// of the second class and its symbol-table index. `display` gives a
/// file's display name (`spec/diag.md` §2.3): the full link's from its
/// inputs, an update's from its state. With the diagnostic come its
/// [`Subjects`]: the files it names.
#[must_use]
pub fn diagnostic(
    display: &dyn Fn(FileId) -> Option<FileRef>,
    finding: Finding,
) -> (Key, Diagnostic, Subjects) {
    let first_other = finding.classes[1][0];
    let key = Key::new(PHASE, first_other.file.0, u64::from(first_other.index.0));
    let name = finding.kind.symbol().unwrap_or_default().to_owned();
    let mut d = Diagnostic::warning(finding.kind);
    let mut files = Vec::new();
    for (c, class) in finding.classes.iter().enumerate() {
        let relation = if c == 0 { Relation::DefinedIn } else { Relation::DefinedDifferentlyIn };
        for at in class.iter().take(FILES_A_CLASS) {
            if let Some(file) = display(at.file) {
                d = d.with(relation, Place::symbol(file, name.clone()));
                files.push(at.file);
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
    files.sort_unstable();
    files.dedup();
    (key, d, Subjects { key, symbol: finding.key, files })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hand-made copy: group by number, as an update's stored row is.
    #[derive(Debug, Clone, Copy)]
    struct Row {
        group: u32,
        file: u32,
        rank: u64,
        index: u32,
        sym_type: u8,
        size: u64,
    }

    impl CopyDef for Row {
        type Group = u32;
        fn group(&self) -> u32 {
            self.group
        }
        fn at(&self) -> At {
            At { file: FileId(self.file), index: SymbolIndex(self.index) }
        }
        fn rank(&self) -> u64 {
            self.rank
        }
        fn sym_type(&self) -> u8 {
            self.sym_type
        }
        fn size(&self) -> u64 {
            self.size
        }
    }

    struct Named;

    impl Names<u32> for Named {
        fn symbol(&self, key: SymbolId) -> String {
            format!("k{}", key.0)
        }
        fn group(&self, group: u32) -> String {
            format!("g{group}")
        }
        fn kept(&self, _group: u32, file: FileId) -> bool {
            file.0 == 1
        }
    }

    fn row(group: u32, file: u32, sym_type: u8, size: u64) -> Row {
        Row { group, file, rank: u64::from(file) << 20, index: 7, sym_type, size }
    }

    fn run(defs: &[Row], resolved: Option<Resolved>) -> Vec<(usize, Finding)> {
        compare_copies(SymbolId(3), defs, resolved.is_some(), || resolved, &Named, true, true)
    }

    // spec: odr.md §8.1 (W1, W2 over hand-made rows; WP-220's one comparator)
    #[test]
    fn agreeing_copies_find_nothing() {
        assert!(run(&[], None).is_empty());
        let defs = [row(1, 1, STT_OBJECT, 8), row(1, 2, STT_OBJECT, 8), row(2, 3, STT_OBJECT, 4)];
        // Sizes differ only across groups: W1 compares within a group.
        assert!(run(&defs, None).is_empty());
    }

    // spec: odr.md §8.1 (W1)
    #[test]
    fn w1_classes_by_size_in_one_group() {
        let defs = [row(1, 1, STT_OBJECT, 8), row(1, 2, STT_OBJECT, 16), row(1, 3, STT_OBJECT, 8)];
        let found = run(&defs, None);
        assert_eq!(found.len(), 1);
        let (w, f) = &found[0];
        assert_eq!(*w, 0);
        assert_eq!(f.key, SymbolId(3));
        assert_eq!(f.kind, Kind::OdrSizeMismatch { name: "k3".into() });
        let files: Vec<Vec<u32>> =
            f.classes.iter().map(|c| c.iter().map(|a| a.file.0).collect()).collect();
        assert_eq!(files, vec![vec![1, 3], vec![2]]);
        assert_eq!(
            f.notes,
            vec![
                "st_size 8 and 16 in the order above, in the copies of COMDAT group g1; the first is kept"
            ]
        );
    }

    // spec: odr.md §8.1 (W2, the resolved definition first)
    #[test]
    fn w2_sides_with_the_resolved_definition_first() {
        let defs = [row(1, 2, STT_OBJECT, 8), row(1, 3, STT_OBJECT, 8)];
        assert!(run(&defs, None).is_empty());
        let used = Resolved {
            at: At { file: FileId(5), index: SymbolIndex(2) },
            binding: STB_GLOBAL,
            sym_type: STT_FUNC,
            common: false,
        };
        let found = run(&defs, Some(used));
        assert_eq!(found.len(), 1);
        let (w, f) = &found[0];
        assert_eq!(*w, 1);
        assert_eq!(f.classes[0], vec![used.at]);
        assert_eq!(f.classes[1].len(), 2);
        assert_eq!(
            f.notes,
            vec![
                "STT_FUNC in the first, STT_OBJECT in the second; the first is the definition used"
            ]
        );
        // The subjects: the symbol and the files named, sorted.
        let names = |file: FileId| Some(FileRef::new(format!("f{}.o", file.0)));
        let (key, _, subjects) = diagnostic(&names, f.clone());
        assert_eq!(key, Key::new(PHASE, 2, 7));
        assert_eq!(subjects.symbol, SymbolId(3));
        assert_eq!(subjects.files, vec![FileId(2), FileId(3), FileId(5)]);
    }

    // spec: incr.md IN-18e (an appended FILES row with a middle rank is
    // ordered by its rank, WP-220)
    #[test]
    fn copies_are_ordered_by_rank_not_file_id() {
        // File 9 appeared in an update between files 1 and 2: its row is
        // appended, its rank is the midpoint.
        let mid = Row { file: 9, rank: (1 << 20) + (1 << 19), ..row(1, 9, STT_OBJECT, 16) };
        let defs = [row(1, 1, STT_OBJECT, 8), mid, row(1, 2, STT_OBJECT, 8)];
        let found = run(&defs, None);
        let files: Vec<Vec<u32>> =
            found[0].1.classes.iter().map(|c| c.iter().map(|a| a.file.0).collect()).collect();
        assert_eq!(files, vec![vec![1, 2], vec![9]]);
        // W2 sorts its copies by rank too.
        let func = Row { file: 9, rank: (1 << 20) + (1 << 19), ..row(1, 9, STT_FUNC, 0) };
        let defs = [row(1, 1, STT_OBJECT, 8), func, row(1, 2, STT_OBJECT, 8)];
        let w2: Vec<Vec<u32>> = run(&defs, None)
            .iter()
            .find(|(w, _)| *w == 1)
            .map(|(_, f)| f.classes.iter().map(|c| c.iter().map(|a| a.file.0).collect()).collect())
            .unwrap_or_default();
        assert_eq!(w2, vec![vec![1, 2], vec![9]]);
        // With the rank order 1, 9, 2 the second class starts at 9 when 9
        // is the odd one, and at 2 when 2 is: §9.1's key follows the rank.
        let odd = Row { size: 16, ..row(1, 2, STT_OBJECT, 8) };
        let mid8 = Row { file: 9, rank: (1 << 20) + (1 << 19), ..row(1, 9, STT_OBJECT, 8) };
        let found = run(&[row(1, 1, STT_OBJECT, 8), mid8, odd], None);
        let names = |file: FileId| Some(FileRef::new(format!("f{}.o", file.0)));
        let (key, _, _) = diagnostic(&names, found[0].1.clone());
        assert_eq!(key, Key::new(PHASE, 2, 7));
        assert_eq!(found[0].1.classes[0].iter().map(|a| a.file.0).collect::<Vec<_>>(), vec![1, 9]);
    }
}

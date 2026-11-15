//! The record checks (`spec/odr.md` §4-§9, WP-073): the rows of the Mason
//! units (`spec/meta.md`) of every live file, compared across units.
//!
//! - kind 1 (O-30, O-31, O-25): `ENFORCE` keys, `isa` of COMDAT copies,
//!   vtables emitted with and without RTTI;
//! - kind 5 (O-35, O-33): declarations against definitions and each other;
//! - kind 3 (O-17, O-18): type layouts and definitions;
//! - kind 2 (O-4 to O-16): definition hashes and flags.
//!
//! The pipeline: the containers of the live files are read (in parallel per
//! file) and parsed by `mortar_meta`'s reader; each file's global names are
//! hashed for anchoring (O-4); every compared row becomes an [`Entry`];
//! the entries are sorted by (kind, key, name) and each group is compared
//! on its own (in parallel); the findings are keyed per §9.1.
//!
//! What this module decides beyond the spec's text is marked "WP-073:"
//! and listed in `spec/landed/meta/WP-073.md`.

use std::borrow::Cow;

use mortar_core::inputs::{File, Reader};
use mortar_core::par::Pool;
use mortar_core::resolution::Def;
use mortar_core::{ComdatId, FileId, Inputs, Resolution};
use mortar_diag::{Diagnostic, FileRef, Key, Kind, Place, Relation};
use mortar_elf::consts::STB_LOCAL;
use mortar_elf::{SectionIndex, SymbolIndex, SymbolSection};
use mortar_meta::read::{
    DeclarationRow, DefinitionRow, FileReader, Finding, SectionAttrs, TypeRow, Unit,
};
use mortar_meta::{flags, fnv1a64};

use crate::foreign::PHASE;

/// The most files a class of definitions lists (§9.1); the rest are
/// counted.
const FILES_A_CLASS: usize = 3;

/// `seq` of a row without a symbol: `2^40 +` its index (§9.1, A-2).
const ROW_SEQ: u64 = 1 << 40;
/// `seq` of a COMDAT group's finding (O-25, O-31): `2^41 +` the group.
const GROUP_SEQ: u64 = 1 << 41;
/// `seq` of a container finding (`meta-malformed`, `meta-version`):
/// `2^42 +` its index among the file's findings.
const META_SEQ: u64 = 1 << 42;
/// The check's tag sits above the `seq` bases, so that findings of
/// different checks never share a key (WP-073; tag 0 is the W checks').
const TAG_SHIFT: u32 = 44;

/// The tag of each record check in a key's `seq`, in O-47's order.
mod tag {
    pub const ENFORCE: u64 = 1;
    pub const KIND5: u64 = 2;
    pub const KIND3: u64 = 3;
    pub const KIND2: u64 = 4;
    pub const INTERNAL: u64 = 5;
    pub const ISA: u64 = 6;
    pub const VTABLE: u64 = 7;
    pub const META: u64 = 8;
}

/// A record check's `seq`: its tag above `base`.
const fn seq(tag: u64, base: u64) -> u64 {
    (tag << TAG_SHIFT) | base
}

/// Kind 1 keys a note never compares: they differ between any two units.
const NOT_NOTED: [&[u8]; 2] = [b"producer", b"source"];

/// Which record checks run (O-47's rows; [`crate::register`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools, reason = "one switch per register row")]
pub struct Switches {
    /// O-30 and O-31 (one register row).
    pub abi: bool,
    /// O-25.
    pub vtable: bool,
    /// O-35, C++ linkage.
    pub kind5_cxx: bool,
    /// O-33, C linkage.
    pub kind5_c: bool,
    /// O-17/O-18: layouts, within one scheme.
    pub kind3_layout: bool,
    /// O-11's strict exception: size, alignment and the shape flags of
    /// kind 3 rows across schemes.
    pub kind3_shape: bool,
    /// O-17: definition-only differences.
    pub kind3_definition: bool,
    /// O-15: hashes and the K13 flags.
    pub kind2: bool,
    /// O-16.
    pub internal: bool,
    /// O-32 differences reported as warnings (`--odr=strict`), not only
    /// counted.
    pub system_std_reported: bool,
    /// Unanchored `INLINED_ONLY` rows are compared (O-41, `strict`).
    pub inlined_only: bool,
}

impl Switches {
    /// Whether any record check runs, so that containers are read.
    #[must_use]
    pub const fn any(self) -> bool {
        self.abi
            || self.vtable
            || self.kind5_cxx
            || self.kind5_c
            || self.kind3_layout
            || self.kind3_shape
            || self.kind3_definition
            || self.kind2
            || self.internal
    }
}

/// What the record checks counted, for `--odr-stats` (O-44).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    /// Files with a container that were read, and their units.
    pub files: u64,
    pub units: u64,
    /// Rows by kind: 1, 2, 3, 5.
    pub rows: [u64; 4],
    /// Distinct `odr.scheme` values (a unit without one counts as none).
    pub schemes: u64,
    /// Unit pairs whose kind 2 and 3 rows are never compared: different
    /// schemes, or a unit without a scheme (O-11, X13).
    pub pairs_not_compared: u64,
    /// Kind 3 groups whose shapes were compared across schemes (O-11's
    /// strict exception).
    pub cross_scheme: u64,
    /// Rows whose anchor is ambiguous (O-4).
    pub ambiguous: u64,
    /// Rows without an anchor that are not compared (O-4).
    pub unanchored: u64,
    /// O-32's differences.
    pub system_std: u64,
    /// Rows set aside: kind 2 and 3 rows of C units (O-33, O-14), rows of
    /// one entity that contradict each other in one unit.
    pub ignored: u64,
    /// Container bytes read.
    pub bytes: u64,
    /// Findings, by diagnostic.
    pub findings: u64,
}

impl Counts {
    fn add(&mut self, o: &Self) {
        self.files += o.files;
        self.units += o.units;
        for (a, b) in self.rows.iter_mut().zip(o.rows) {
            *a += b;
        }
        self.cross_scheme += o.cross_scheme;
        self.ambiguous += o.ambiguous;
        self.unanchored += o.unanchored;
        self.system_std += o.system_std;
        self.ignored += o.ignored;
        self.bytes += o.bytes;
        self.findings += o.findings;
    }
}

/// A global symbol of a file with a container, for anchoring (O-4).
#[derive(Debug, Clone, Copy)]
struct Sym<'s> {
    key: u64,
    name: &'s [u8],
    index: SymbolIndex,
    defined: bool,
}

/// One file's containers, read.
struct Loaded<'s> {
    file: FileId,
    sections: Vec<(SectionAttrs, Cow<'s, [u8]>)>,
    /// Sorted by (key, name).
    syms: Vec<Sym<'s>>,
    /// The symbol table's first global (`sym_map_of` is indexed from it).
    first_global: u32,
}

/// One file's units, parsed.
struct Parsed<'l> {
    units: Vec<Unit<'l>>,
    findings: Vec<Finding>,
}

/// What a unit's kind 1 says that the comparisons use (MT-26).
#[derive(Debug, Clone, Copy)]
struct Info<'l> {
    /// Index of the unit's file in `loaded`.
    at: usize,
    file: FileId,
    lang_c: bool,
    std: Option<&'l [u8]>,
    scheme: Option<&'l [u8]>,
}

/// A row that takes part in a comparison.
#[derive(Debug, Clone, Copy)]
struct Entry<'l> {
    kind: u8,
    key: u64,
    /// The entity's name: the anchor's, the row's own, or empty for an
    /// unanchored `INLINED_ONLY` row.
    name: &'l [u8],
    /// Kind 2's class; 0 for the other kinds.
    class: u16,
    unit: u32,
    row: u32,
    /// The anchor's symbol index, if any.
    sym: Option<SymbolIndex>,
}

impl Entry<'_> {
    /// The integer part of the group: a key collision between names is
    /// split by [`groups`] inside a run of equal `run`s.
    fn run(&self) -> (u8, u64, u16) {
        (self.kind, self.key, self.class)
    }
}

/// The context of the comparisons.
struct Cx<'a, 's, 'l> {
    inputs: &'a Inputs<'s>,
    resolution: &'a Resolution,
    loaded: &'a [Loaded<'s>],
    units: &'a [&'a Unit<'l>],
    info: &'a [Info<'l>],
    switches: Switches,
}

/// Runs the record checks `switches` asks for over the containers of the
/// live files.
#[must_use]
pub fn check(
    inputs: &Inputs<'_>,
    resolution: &Resolution,
    switches: Switches,
    pool: &Pool,
) -> (Vec<(Key, Diagnostic)>, Counts) {
    let mut counts = Counts::default();
    if !switches.any() || resolution.meta.is_empty() {
        return (Vec::new(), counts);
    }
    // The live files with a container, and their sections (sorted).
    let meta = &resolution.meta;
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut start = 0;
    for i in 1..=meta.len() {
        if i == meta.len() || meta[i].0 != meta[start].0 {
            let live = resolution.file_live.get(meta[start].0).is_some_and(|l| l.is_live());
            if live {
                runs.push((start, i));
            }
            start = i;
        }
    }
    let loaded = pool
        .unit_cost(20_000)
        .map_collect(runs.len(), |u, out: &mut Vec<Loaded<'_>>| {
            let (a, b) = runs[u];
            if let Some(l) = load(inputs, &meta[a..b]) {
                out.push(l);
            }
        })
        .items;
    counts.files = loaded.len() as u64;
    counts.bytes = loaded.iter().flat_map(|l| &l.sections).map(|(_, b)| b.len() as u64).sum();
    let parsed = pool
        .unit_cost(20_000)
        .map_collect(loaded.len(), |u, out: &mut Vec<Parsed<'_>>| {
            let mut reader = FileReader::new();
            for (attrs, bytes) in &loaded[u].sections {
                reader.section(*attrs, bytes);
            }
            out.push(Parsed { units: reader.units, findings: reader.findings });
        })
        .items;

    let mut diagnostics = Vec::new();
    // MT-14, MT-35: errors under `strict` (O-41), the only mode that reads.
    for (l, p) in loaded.iter().zip(&parsed) {
        for (i, finding) in p.findings.iter().enumerate() {
            diagnostics.push(container_finding(inputs, l.file, i, finding));
        }
    }

    let mut units: Vec<&Unit<'_>> = Vec::new();
    let mut info: Vec<Info<'_>> = Vec::new();
    for (at, (l, p)) in loaded.iter().zip(&parsed).enumerate() {
        for u in &p.units {
            units.push(u);
            info.push(info_of(at, l.file, u));
        }
    }
    counts.units = units.len() as u64;
    let (schemes, pairs) = schemes(&info);
    counts.schemes = schemes;
    counts.pairs_not_compared = pairs;

    let cx = Cx { inputs, resolution, loaded: &loaded, units: &units, info: &info, switches };

    // Entries of kinds 2, 3 and 5, per unit, then sorted into groups.
    let got = pool.unit_cost(5_000).map_collect(units.len(), |u, out: &mut Vec<Entry<'_>>| {
        cx.entries(u, out);
    });
    let mut entries = got.items;
    let per_unit = pool.unit_cost(5_000).map_collect(units.len(), |u, out: &mut Vec<Counts>| {
        let mut c = cx.unit_counts(u);
        c.add(&cx.skipped(u));
        out.push(c);
    });
    for c in &per_unit.items {
        counts.add(c);
    }
    // Integers only (WP-073: comparing names here was a third of the
    // checks' time); names split a run only on a key collision.
    pool.sort_by(&mut entries, |a, b| (a.run(), a.unit, a.row).cmp(&(b.run(), b.unit, b.row)));
    let groups = groups(&mut entries);
    let found = pool.unit_cost(2_000).map_collect(groups.len(), |g, out: &mut Vec<Found>| {
        let (a, b) = groups[g];
        cx.group(&entries[a..b], out);
    });
    for f in found.items {
        counts.add(&f.counts);
        diagnostics.extend(f.diagnostic);
    }
    if switches.abi {
        let before = diagnostics.len();
        cx.enforce(&mut diagnostics);
        cx.isa(&mut diagnostics);
        counts.findings += (diagnostics.len() - before) as u64;
    }
    if switches.vtable {
        let before = diagnostics.len();
        cx.vtables(&mut diagnostics);
        counts.findings += (diagnostics.len() - before) as u64;
    }
    diagnostics.sort_by_key(|(k, _)| *k);
    (diagnostics, counts)
}

/// Reads one live file's containers (`sections` all of that file) and
/// hashes its global names (O-4).
fn load<'s>(inputs: &Inputs<'s>, sections: &[(FileId, SectionIndex)]) -> Option<Loaded<'s>> {
    let file = sections.first()?.0;
    let Reader::Object(obj) = inputs.files.get(file)?.reader else { return None };
    let mut out = Vec::with_capacity(sections.len());
    for &(_, index) in sections {
        let Ok(sec) = obj.section(index) else { continue };
        let h = sec.header();
        let attrs = SectionAttrs {
            index: index.0,
            addralign: h.sh_addralign,
            entsize: h.sh_entsize,
            link: h.sh_link,
            info: h.sh_info,
        };
        let bytes = if let Ok(data) = sec.data() {
            Cow::Borrowed(data)
        } else {
            let Ok(Some(range)) = sec.file_range() else { continue };
            let mut buf = vec![0; range.len()];
            if inputs.copy_to(file, range.start as u64..range.end as u64, &mut buf).is_err() {
                continue;
            }
            Cow::Owned(buf)
        };
        out.push((attrs, bytes));
    }
    let mut syms = Vec::new();
    let mut first_global = 0;
    if let Ok(Some(table)) = obj.symbols() {
        first_global = table.first_global().0;
        for (index, sym) in table.globals() {
            if sym.binding() == STB_LOCAL {
                continue;
            }
            let Ok(name) = table.name(&sym) else { continue };
            if name.is_empty() {
                continue;
            }
            let defined =
                !matches!(table.section_of(index, &sym), Ok(SymbolSection::Undefined) | Err(_));
            syms.push(Sym { key: fnv1a64(name), name, index, defined });
        }
        syms.sort_unstable_by(|a, b| (a.key, a.name, a.index.0).cmp(&(b.key, b.name, b.index.0)));
    }
    Some(Loaded { file, sections: out, syms, first_global })
}

fn info_of<'l>(at: usize, file: FileId, u: &Unit<'l>) -> Info<'l> {
    let value = |name: &[u8]| u.summary_value(name).map(|(v, _)| v);
    Info {
        at,
        file,
        lang_c: value(b"lang") == Some(b"c"),
        std: value(b"std"),
        scheme: value(b"odr.scheme").filter(|s| !s.is_empty()),
    }
}

/// (name, value, file, unit, row) of an ENFORCE kind-1 row.
type EnforceRow<'a> = (&'a [u8], &'a [u8], FileId, u32, u32);

/// The number of distinct schemes, and the unit pairs never compared
/// because their schemes differ or one has none (O-11).
fn schemes(info: &[Info<'_>]) -> (u64, u64) {
    let mut sorted: Vec<Option<&[u8]>> = info.iter().map(|i| i.scheme).collect();
    sorted.sort_unstable();
    let s = &sorted;
    let units = s.len() as u64;
    let mut same = 0u64;
    let mut distinct = 0u64;
    let mut i = 0;
    while i < s.len() {
        let j = s[i..].iter().position(|x| *x != s[i]).map_or(s.len(), |p| i + p);
        let c = (j - i) as u64;
        if s[i].is_some() {
            distinct += 1;
            same += c * (c - 1) / 2;
        }
        i = j;
    }
    (distinct, (units * units.saturating_sub(1) / 2).saturating_sub(same))
}

/// The groups (kind, key, name, class) of `entries`, sorted by
/// (kind, key, class, unit, row): each group in (unit, row) order. A run
/// of one (kind, key, class) whose names differ (a key collision, or
/// unanchored rows beside named ones) is sorted by name first. An
/// unanchored group (empty name) joins the run's one named group when
/// there is exactly one (WP-073), in (unit, row) order.
fn groups(entries: &mut [Entry<'_>]) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut start = 0;
    while start < entries.len() {
        let run = entries[start].run();
        let end = entries[start..]
            .iter()
            .position(|e| e.run() != run)
            .map_or(entries.len(), |p| start + p);
        let name = entries[start].name;
        if entries[start..end].iter().all(|e| e.name == name) {
            out.push((start, end));
        } else {
            let slice = &mut entries[start..end];
            slice.sort_by(|a, b| (a.name, a.unit, a.row).cmp(&(b.name, b.unit, b.row)));
            let mut names: Vec<(usize, usize)> = Vec::new();
            let mut a = 0;
            for i in 1..=slice.len() {
                if i == slice.len() || slice[i].name != slice[a].name {
                    names.push((start + a, start + i));
                    a = i;
                }
            }
            if names.len() == 2 && entries[names[0].0].name.is_empty() {
                entries[start..end].sort_by_key(|e| (e.unit, e.row));
                out.push((start, end));
            } else {
                out.extend(names);
            }
        }
        start = end;
    }
    out
}

fn container_finding(
    inputs: &Inputs<'_>,
    file: FileId,
    i: usize,
    finding: &Finding,
) -> (Key, Diagnostic) {
    let key = Key::new(PHASE, file.0, seq(tag::META, META_SEQ + i as u64));
    let at = match finding {
        Finding::Malformed { unit, .. } => unit.unwrap_or(0),
        Finding::Version { unit, .. } => *unit,
    };
    let kind = match finding {
        Finding::Malformed { .. } => Kind::MetaMalformed { reason: finding.reason() },
        Finding::Version { major, .. } => Kind::MetaVersion { major: *major },
    };
    let mut d = Diagnostic::error(kind);
    if let Some(f) = inputs.files.get(file) {
        d = d.at(Place::section(f.display(), ".mason.meta", at));
    }
    (key, d)
}

/// The anchor of a row (O-4).
enum Anchor<'s> {
    Name(&'s [u8], SymbolIndex),
    Missing,
    Ambiguous,
}

fn anchor<'s>(syms: &[Sym<'s>], key: u64, defined_only: bool) -> Anchor<'s> {
    let start = syms.partition_point(|s| s.key < key);
    let mut found: Option<(&'s [u8], SymbolIndex)> = None;
    for s in syms[start..].iter().take_while(|s| s.key == key) {
        if defined_only && !s.defined {
            continue;
        }
        match found {
            None => found = Some((s.name, s.index)),
            Some((name, _)) if name == s.name => {}
            Some(_) => return Anchor::Ambiguous,
        }
    }
    found.map_or(Anchor::Missing, |(n, i)| Anchor::Name(n, i))
}

/// What a group's comparison gives: a diagnostic, or only counts (an
/// O-32 difference that is not reported, contradictory rows).
struct Found {
    diagnostic: Option<(Key, Diagnostic)>,
    counts: Counts,
}

impl Found {
    fn report(d: (Key, Diagnostic)) -> Self {
        Self { diagnostic: Some(d), counts: Counts { findings: 1, ..Counts::default() } }
    }

    fn count(counts: Counts) -> Self {
        Self { diagnostic: None, counts }
    }
}

/// A class of rows that agree: the rows' (unit, row) entries.
type Class<'l> = Vec<Entry<'l>>;

/// How a finding is reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Severity {
    Error,
    Warning,
}

impl<'s: 'l, 'l> Cx<'_, 's, 'l> {
    fn file_ref(&self, file: FileId) -> FileRef {
        self.inputs.files.get(file).map_or_else(|| FileRef::new(""), File::display)
    }

    /// A file as a note writes it: `a.o`, `libx.a(y.o)`.
    fn file_text(&self, file: FileId) -> String {
        let f = self.file_ref(file);
        match f.member() {
            Some(m) => format!("{}({m})", f.path()),
            None => f.path().to_owned(),
        }
    }

    /// The counts a unit adds by itself: its rows.
    fn unit_counts(&self, u: usize) -> Counts {
        let unit = self.units[u];
        let rows = [
            unit.summary().len(),
            unit.definitions().len(),
            unit.types().len(),
            unit.declarations().len(),
        ];
        Counts { rows: rows.map(|r| r as u64), ..Counts::default() }
    }

    /// The entries of unit `u`; what it sets aside is [`Cx::skipped`]'s.
    fn entries(&self, u: usize, out: &mut Vec<Entry<'l>>) {
        let unit = self.units[u];
        let info = self.info[u];
        let syms = &self.loaded[info.at].syms;
        let unit_id = u as u32;
        let s = self.switches;
        if (s.kind2 || s.internal) && !info.lang_c {
            for (i, r) in unit.definitions().iter().enumerate() {
                let (name, sym) = if r.name != 0 {
                    (unit.string(r.name), None)
                } else {
                    match anchor(syms, r.key, true) {
                        Anchor::Name(n, i) => (n, Some(i)),
                        Anchor::Missing if s.inlined_only && r.flags & flags::INLINED_ONLY != 0 => {
                            (&b""[..], None)
                        }
                        // Counted by `skipped`.
                        Anchor::Missing | Anchor::Ambiguous => continue,
                    }
                };
                out.push(Entry {
                    kind: 2,
                    key: r.key,
                    name,
                    class: r.class,
                    unit: unit_id,
                    row: i as u32,
                    sym,
                });
            }
        }
        if (s.kind3_layout || s.kind3_shape || s.kind3_definition) && !info.lang_c {
            for (i, r) in unit.types().iter().enumerate() {
                out.push(Entry {
                    kind: 3,
                    key: r.key,
                    name: unit.string(r.name),
                    class: 0,
                    unit: unit_id,
                    row: i as u32,
                    sym: None,
                });
            }
        }
        if s.kind5_cxx || s.kind5_c {
            for (i, r) in unit.declarations().iter().enumerate() {
                let (name, sym) = if r.name != 0 {
                    (unit.string(r.name), None)
                } else {
                    match anchor(syms, r.key, false) {
                        Anchor::Name(n, i) => (n, Some(i)),
                        Anchor::Missing | Anchor::Ambiguous => continue,
                    }
                };
                out.push(Entry {
                    kind: 5,
                    key: r.key,
                    name,
                    class: 0,
                    unit: unit_id,
                    row: i as u32,
                    sym,
                });
            }
        }
    }

    /// What unit `u` sets aside without comparing: unanchored and
    /// ambiguous rows, kind 2 and 3 rows of a C unit.
    fn skipped(&self, u: usize) -> Counts {
        let unit = self.units[u];
        let info = self.info[u];
        let syms = &self.loaded[info.at].syms;
        let s = self.switches;
        let mut c = Counts::default();
        if info.lang_c {
            c.ignored += (unit.definitions().len() + unit.types().len()) as u64;
        } else if s.kind2 || s.internal {
            for r in unit.definitions().iter().filter(|r| r.name == 0) {
                match anchor(syms, r.key, true) {
                    Anchor::Name(..) => {}
                    Anchor::Missing if s.inlined_only && r.flags & flags::INLINED_ONLY != 0 => {}
                    Anchor::Missing => c.unanchored += 1,
                    Anchor::Ambiguous => c.ambiguous += 1,
                }
            }
        }
        if s.kind5_cxx || s.kind5_c {
            for r in unit.declarations().iter().filter(|r| r.name == 0) {
                match anchor(syms, r.key, false) {
                    Anchor::Name(..) => {}
                    Anchor::Missing => c.unanchored += 1,
                    Anchor::Ambiguous => c.ambiguous += 1,
                }
            }
        }
        c
    }

    fn definition(&self, e: &Entry<'_>) -> Option<DefinitionRow> {
        self.units[e.unit as usize].definitions().get(e.row as usize)
    }

    fn type_row(&self, e: &Entry<'_>) -> Option<TypeRow> {
        self.units[e.unit as usize].types().get(e.row as usize)
    }

    fn declaration(&self, e: &Entry<'_>) -> Option<DeclarationRow> {
        self.units[e.unit as usize].declarations().get(e.row as usize)
    }

    fn file_of(&self, e: &Entry<'_>) -> FileId {
        self.info[e.unit as usize].file
    }

    /// The file whose definition of the group's symbol the link keeps.
    fn keeper(&self, group: &[Entry<'_>]) -> Option<FileId> {
        let e = group.iter().find(|e| e.sym.is_some())?;
        let at = self.info[e.unit as usize].at;
        let l = &self.loaded[at];
        let index = e.sym?.0.checked_sub(l.first_global)?;
        let key = *self.inputs.sym_map_of(l.file).get(index as usize)?;
        match self.resolution.def.get(key) {
            Some(Def::Defined(d) | Def::Common(d)) => Some(d.file),
            _ => None,
        }
    }

    /// One group: rows of one (kind, key, name, class).
    fn group(&self, group: &[Entry<'l>], out: &mut Vec<Found>) {
        let mut counts = Counts::default();
        let rows = self.one_per_unit(group, &mut counts);
        if counts != Counts::default() {
            out.push(Found::count(counts));
        }
        if rows.len() >= 2 {
            match group[0].kind {
                2 => self.kind2(&rows, out),
                3 => self.kind3(&rows, out),
                5 => self.kind5(&rows, out),
                _ => {}
            }
        }
    }

    /// The group's rows, one per unit: rows of one entity in one unit
    /// that are identical are one row; rows that differ contradict each
    /// other and the unit's rows of the entity are set aside (WP-073).
    fn one_per_unit(&self, group: &[Entry<'l>], counts: &mut Counts) -> Vec<Entry<'l>> {
        let mut out: Vec<Entry<'l>> = Vec::with_capacity(group.len());
        let mut i = 0;
        while i < group.len() {
            let u = group[i].unit;
            let j = group[i..].iter().position(|e| e.unit != u).map_or(group.len(), |p| i + p);
            let same = group[i + 1..j].iter().all(|e| self.raw(e) == self.raw(&group[i]));
            if same {
                out.push(group[i]);
            } else {
                counts.ignored += (j - i) as u64;
            }
            i = j;
        }
        out
    }

    /// A row's bytes, for comparing rows of one unit.
    fn raw(&self, e: &Entry<'_>) -> Option<&[u8]> {
        let u = self.units[e.unit as usize];
        let i = e.row as usize;
        match e.kind {
            2 => u.definitions().raw(i),
            3 => u.types().raw(i),
            5 => u.declarations().raw(i),
            _ => None,
        }
    }

    /// Splits `rows` by scheme (O-11): rows of units without a scheme are
    /// never compared.
    fn by_scheme(&self, rows: &[Entry<'l>]) -> Vec<Vec<Entry<'l>>> {
        let mut sorted: Vec<Entry<'l>> =
            rows.iter().copied().filter(|e| self.info[e.unit as usize].scheme.is_some()).collect();
        sorted.sort_by_key(|e| (self.info[e.unit as usize].scheme, self.file_of(e), e.unit));
        let mut out: Vec<Vec<Entry<'l>>> = Vec::new();
        for e in sorted {
            let scheme = self.info[e.unit as usize].scheme;
            match out.last_mut() {
                Some(last) if self.info[last[0].unit as usize].scheme == scheme => last.push(e),
                _ => out.push(vec![e]),
            }
        }
        out.retain(|g| g.len() >= 2);
        out
    }

    /// Whether the units of `rows` have different `std` (O-32).
    fn std_differs(&self, rows: &[Entry<'_>]) -> bool {
        let first = self.info[rows[0].unit as usize].std;
        rows.iter().any(|e| self.info[e.unit as usize].std != first)
    }

    /// Partitions `rows` into classes of equal `key`, each class in file
    /// order, the classes by their first file, the keeper's class first.
    /// `O(n log n)`: a group of `n` distinct rows stays cheap (WP-073).
    fn classes<K: Ord>(
        &self,
        rows: &[Entry<'l>],
        keeper: Option<FileId>,
        key: impl Fn(&Entry<'l>) -> K,
    ) -> Vec<Class<'l>> {
        let mut sorted = rows.to_vec();
        sorted.sort_by_key(|e| (self.file_of(e), e.unit));
        let keys: Vec<K> = sorted.iter().map(&key).collect();
        // Positions sorted by (key, position): each run is a class, and
        // its first position is the class's place in file order.
        let mut order: Vec<usize> = (0..sorted.len()).collect();
        order.sort_by(|&a, &b| keys[a].cmp(&keys[b]).then(a.cmp(&b)));
        let mut runs: Vec<Vec<usize>> = Vec::new();
        for &p in &order {
            match runs.last_mut() {
                Some(r) if keys[r[0]] == keys[p] => r.push(p),
                _ => runs.push(vec![p]),
            }
        }
        runs.sort_by_key(|r| r[0]);
        let mut classes: Vec<Class<'l>> =
            runs.into_iter().map(|r| r.into_iter().map(|p| sorted[p]).collect()).collect();
        if let Some(k) = keeper
            && let Some(at) = classes.iter().position(|c| c.iter().any(|e| self.file_of(e) == k))
        {
            let c = classes.remove(at);
            classes.insert(0, c);
        }
        classes
    }

    /// The kind 2 comparison (O-15, O-16, O-32).
    #[allow(clippy::too_many_lines)] // one rule table (O-15, O-16), read top to bottom
    fn kind2(&self, rows: &[Entry<'l>], out: &mut Vec<Found>) {
        let keeper = self.keeper(rows);
        let def = |e: &Entry<'_>| self.definition(e);
        if self.switches.internal {
            let users: Vec<Entry<'l>> = rows
                .iter()
                .copied()
                .filter(|e| def(e).is_some_and(|r| r.flags & flags::USES_INTERNAL != 0))
                .collect();
            let mut files: Vec<FileId> = users.iter().map(|e| self.file_of(e)).collect();
            files.dedup();
            if files.len() >= 2 {
                let classes = self.classes(&users, keeper, |e| e.unit);
                let name = Self::display_name(rows);
                let kind = Kind::OdrInternalReference { name };
                let note = "each translation unit's copy refers to its own entity with internal \
                            linkage (a static function or variable, or one in an unnamed \
                            namespace)"
                    .to_owned();
                out.push(Found::report(self.diagnostic(
                    kind,
                    Severity::Warning,
                    &classes,
                    keeper,
                    vec![note],
                )));
            }
        }
        if !self.switches.kind2 {
            return;
        }
        for rows in self.by_scheme(rows) {
            let flag = |e: &Entry<'_>, f: u16| def(e).is_some_and(|r| r.flags & f != 0);
            let spec = rows.iter().any(|e| flag(e, flags::EXPLICIT_SPEC));
            let implicit = rows.iter().any(|e| flag(e, flags::IMPLICIT_INST));
            let hashes = self.classes(&rows, keeper, |e| def(e).map(|r| r.hash));
            let (classes, k13) = if hashes.len() >= 2 {
                (hashes, spec && implicit)
            } else if spec && implicit {
                let c = self.classes(&rows, keeper, |e| flag(e, flags::EXPLICIT_SPEC));
                (c, true)
            } else {
                continue;
            };
            let partial = rows.iter().any(|e| flag(e, flags::PARTIAL));
            let system = rows.iter().all(|e| flag(e, flags::SYSTEM)) && self.std_differs(&rows);
            let mut notes = Vec::new();
            let severity = if system {
                if !self.switches.system_std_reported {
                    out.push(Found::count(Counts { system_std: 1, ..Counts::default() }));
                    continue;
                }
                notes.push(
                    "every definition is in a system header, and the translation units' -std \
                     differ: a warning (spec/odr.md O-32)"
                        .to_owned(),
                );
                Severity::Warning
            } else if partial {
                notes.push(partial_note());
                Severity::Warning
            } else {
                Severity::Error
            };
            if k13 {
                let which = |c: &Class<'_>| {
                    if flag(&c[0], flags::EXPLICIT_SPEC) {
                        "an explicit specialization"
                    } else if flag(&c[0], flags::IMPLICIT_INST) {
                        "an implicit instantiation"
                    } else {
                        "a definition"
                    }
                };
                notes.push(format!(
                    "{}: {}; {}: {} ([temp.expl.spec]: the specialization must be declared \
                     before every use)",
                    self.file_text(self.file_of(&classes[0][0])),
                    which(&classes[0]),
                    self.file_text(self.file_of(&classes[1][0])),
                    which(&classes[1]),
                ));
            }
            let differ = self.summary_notes(&classes[0][0], &classes[1][0]);
            let none_differ = differ.is_empty();
            notes.extend(differ);
            if none_differ && !k13 && !system {
                notes.push(format!(
                    "the definitions differ; {} and {} were compiled with different -D options? \
                     (compare their command lines)",
                    self.file_text(self.file_of(&classes[0][0])),
                    self.file_text(self.file_of(&classes[1][0])),
                ));
            }
            let name = Self::display_name(&rows);
            let kind = Kind::OdrViolation { name };
            let mut found = Found::report(self.diagnostic(kind, severity, &classes, keeper, notes));
            found.counts.system_std = u64::from(system);
            out.push(found);
        }
    }

    /// The kind 3 comparison (O-17, O-18, O-32): within one scheme the
    /// layout (with size, alignment and the shape flags) and the
    /// definition hash; across schemes, under O-11's strict exception,
    /// size, alignment and the shape flags only.
    #[allow(clippy::too_many_lines)] // O-17, O-18, O-11's exception, read top to bottom
    fn kind3(&self, rows: &[Entry<'l>], out: &mut Vec<Found>) {
        const SHAPE: u16 =
            flags::TYPE_ENUM | flags::TYPE_UNION | flags::TYPE_POLYMORPHIC | flags::TYPE_HAS_VBASES;
        let ty = |e: &Entry<'_>| self.type_row(e);
        let shape = |e: &Entry<'_>| ty(e).map(|r| (r.size, r.align, r.flags & SHAPE));
        let flag = |e: &Entry<'_>, f: u16| ty(e).is_some_and(|r| r.flags & f != 0);
        let mut found: Option<(Vec<Class<'l>>, Compared)> = None;
        for rows in &self.by_scheme(rows) {
            if self.switches.kind3_layout {
                let c = self.classes(rows, None, |e| (ty(e).map(|r| r.layout), shape(e)));
                if c.len() >= 2 {
                    found = Some((c, Compared::Layout));
                    break;
                }
            }
            if self.switches.kind3_definition {
                // O-17: hashes compared only when both are non-zero.
                let hashed: Vec<Entry<'l>> = rows
                    .iter()
                    .copied()
                    .filter(|e| ty(e).is_some_and(|r| r.hash != [0; 16]))
                    .collect();
                let c = self.classes(&hashed, None, |e| ty(e).map(|r| r.hash));
                if c.len() >= 2 {
                    found = Some((c, Compared::Definition));
                    break;
                }
            }
        }
        if found.is_none() && self.switches.kind3_shape {
            let first = self.info[rows[0].unit as usize].scheme;
            if rows.iter().any(|e| self.info[e.unit as usize].scheme != first) {
                out.push(Found::count(Counts { cross_scheme: 1, ..Counts::default() }));
                let c = self.classes(rows, None, shape);
                if c.len() >= 2 {
                    found = Some((c, Compared::Shape));
                }
            }
        }
        let Some((classes, compared)) = found else { return };
        let layout = compared != Compared::Definition;
        let all: Vec<Entry<'l>> = classes.iter().flatten().copied().collect();
        let partial = all.iter().any(|e| flag(e, flags::TYPE_PARTIAL));
        let system = all.iter().all(|e| flag(e, flags::TYPE_SYSTEM)) && self.std_differs(&all);
        let mut notes = Vec::new();
        let severity = if system {
            if !self.switches.system_std_reported {
                out.push(Found::count(Counts { system_std: 1, ..Counts::default() }));
                return;
            }
            notes.push(
                "every definition is in a system header, and the translation units' -std \
                 differ: a warning (spec/odr.md O-32)"
                    .to_owned(),
            );
            Severity::Warning
        } else if partial {
            notes.push(partial_note());
            Severity::Warning
        } else if layout {
            Severity::Error
        } else {
            Severity::Warning
        };
        let (a, b) = (&classes[0][0], &classes[1][0]);
        if compared == Compared::Shape {
            notes.push(
                "the translation units use different odr.scheme values: compared on size, \
                 alignment and kind only (spec/odr.md O-11, strict)"
                    .to_owned(),
            );
        }
        if let (Some(ra), Some(rb)) = (ty(a), ty(b)) {
            let kinds = |r: &TypeRow| r.flags & SHAPE;
            if layout && (ra.size, ra.align) == (rb.size, rb.align) && kinds(&ra) != kinds(&rb) {
                notes.push(format!(
                    "{}: {}; {}: {}",
                    self.file_text(self.file_of(a)),
                    shape_text(kinds(&ra)),
                    self.file_text(self.file_of(b)),
                    shape_text(kinds(&rb)),
                ));
            } else if layout && (ra.size, ra.align) == (rb.size, rb.align) {
                notes.push(format!(
                    "the layouts differ (members, offsets, bases or vtable) with the same size \
                     {} and alignment {}",
                    ra.size, ra.align
                ));
            } else if layout {
                notes.push(format!(
                    "{}: size {}, alignment {}; {}: size {}, alignment {}",
                    self.file_text(self.file_of(a)),
                    ra.size,
                    ra.align,
                    self.file_text(self.file_of(b)),
                    rb.size,
                    rb.align,
                ));
            } else {
                notes.push(
                    "the layouts agree; the definitions differ (members, initializers or \
                     member declarations)"
                        .to_owned(),
                );
            }
        }
        notes.extend(self.summary_notes(a, b));
        let kind = Kind::OdrTypeMismatch { name: Self::display_name(&classes[0]) };
        let mut found = Found::report(self.diagnostic(kind, severity, &classes, None, notes));
        found.counts.system_std = u64::from(system);
        out.push(found);
    }

    /// The kind 5 comparison (O-35, O-33).
    fn kind5(&self, rows: &[Entry<'l>], out: &mut Vec<Found>) {
        let decl = |e: &Entry<'_>| self.declaration(e);
        let flag = |e: &Entry<'_>, f: u16| decl(e).is_some_and(|r| r.flags & f != 0);
        let c_linkage =
            rows.iter().any(|e| flag(e, flags::DECL_EXTERN_C) || self.info[e.unit as usize].lang_c);
        let on = if c_linkage { self.switches.kind5_c } else { self.switches.kind5_cxx };
        if !on {
            return;
        }
        let keeper = self.keeper(rows);
        // Classes by type; sizes split a type's class when both are known:
        // a row without a size joins its type's first sized row in file
        // order (O-35).
        let mut sized: Vec<(u64, FileId, u32, u64)> = rows
            .iter()
            .filter_map(|e| {
                decl(e).filter(|r| r.size != 0).map(|r| (r.ty, self.file_of(e), e.unit, r.size))
            })
            .collect();
        sized.sort_unstable();
        let size_of = |e: &Entry<'_>| {
            let r = decl(e)?;
            if r.size != 0 {
                return Some((r.ty, r.size));
            }
            let at = sized.partition_point(|s| s.0 < r.ty);
            Some((r.ty, sized.get(at).filter(|s| s.0 == r.ty).map_or(0, |s| s.3)))
        };
        let classes = self.classes(rows, keeper, size_of);
        let definition = |e: &Entry<'_>| flag(e, flags::DECL_DEFINITION);
        // Two definitions are never compared (O-35): a finding needs a
        // declaration on one side of a disagreeing pair, and with two
        // classes any declaration is on one side of one.
        let disagree = classes.len() >= 2 && rows.iter().any(|e| !definition(e));
        let noexcept_only = !disagree && classes.len() == 1 && {
            let first = flag(&rows[0], flags::DECL_NOEXCEPT);
            rows.iter().any(|e| flag(e, flags::DECL_NOEXCEPT) != first)
        };
        if !disagree && !noexcept_only {
            return;
        }
        let classes = if noexcept_only {
            self.classes(rows, keeper, |e| flag(e, flags::DECL_NOEXCEPT))
        } else {
            classes
        };
        let severity = if c_linkage || noexcept_only { Severity::Warning } else { Severity::Error };
        let (first, second) = (&classes[0][0], &classes[1][0]);
        let mut notes = Vec::new();
        if let (Some(ra), Some(rb)) = (decl(first), decl(second)) {
            let fa = self.file_text(self.file_of(first));
            let fb = self.file_text(self.file_of(second));
            let what = |r: &DeclarationRow, e: &Entry<'_>| {
                let s = self.units[e.unit as usize].string(r.type_name);
                let t = if s.is_empty() { "?".into() } else { String::from_utf8_lossy(s) };
                let d = if r.flags & flags::DECL_DEFINITION != 0 { "defined" } else { "declared" };
                if r.size != 0 {
                    format!("{d} as {t}, size {}", r.size)
                } else {
                    format!("{d} as {t}")
                }
            };
            if noexcept_only {
                notes.push(format!(
                    "the declarations differ only in noexcept: {fa}: {}; {fb}: {}",
                    if ra.flags & flags::DECL_NOEXCEPT != 0 { "noexcept" } else { "not noexcept" },
                    if rb.flags & flags::DECL_NOEXCEPT != 0 { "noexcept" } else { "not noexcept" },
                ));
            } else {
                notes.push(format!("{fa}: {}; {fb}: {}", what(&ra, first), what(&rb, second)));
            }
            if c_linkage {
                notes.push(
                    "C linkage: compared as the producer's canonical C types (spec/odr.md O-33); \
                     a warning"
                        .to_owned(),
                );
            }
        }
        notes.extend(self.summary_notes(first, second));
        let kind = Kind::OdrDeclarationMismatch { name: Self::display_name(rows) };
        out.push(Found::report(self.diagnostic(kind, severity, &classes, keeper, notes)));
    }

    /// The entity's name for a message: the first anchor's or row's name
    /// among `rows`; the key when every row is unanchored.
    fn display_name(rows: &[Entry<'_>]) -> String {
        match rows.iter().find(|e| !e.name.is_empty()) {
            Some(e) => String::from_utf8_lossy(e.name).into_owned(),
            None => format!("<unnamed entity, key {:#018x}>", rows[0].key),
        }
    }

    /// §9.1's notes from kind 1: every key whose value differs between the
    /// units of `a` and `b` (not `producer` or `source`).
    fn summary_notes(&self, a: &Entry<'_>, b: &Entry<'_>) -> Vec<String> {
        let (ua, ub) = (self.units[a.unit as usize], self.units[b.unit as usize]);
        let fa = self.file_text(self.file_of(a));
        let fb = self.file_text(self.file_of(b));
        let mut names: Vec<&[u8]> = ua.summary().iter().map(|r| ua.string(r.name)).collect();
        names.extend(ub.summary().iter().map(|r| ub.string(r.name)));
        names.sort_unstable();
        names.dedup();
        let mut out = Vec::new();
        for name in names {
            if NOT_NOTED.contains(&name) {
                continue;
            }
            let va = ua.summary_value(name).map(|(v, _)| v);
            let vb = ub.summary_value(name).map(|(v, _)| v);
            if va != vb {
                let text = |v: Option<&[u8]>| {
                    v.map_or_else(
                        || "(none)".to_owned(),
                        |v| String::from_utf8_lossy(v).into_owned(),
                    )
                };
                out.push(format!(
                    "{fa}: {}={}; {fb}: {}",
                    String::from_utf8_lossy(name),
                    text(va),
                    text(vb)
                ));
            }
        }
        out
    }

    /// Which translation unit each side is, and what built it: kind 1's
    /// `source` and `producer`, where the units carry them (WP-073).
    fn unit_note(&self, a: &Entry<'_>, b: &Entry<'_>) -> Option<String> {
        if a.unit == b.unit {
            return None;
        }
        let side = |e: &Entry<'_>| {
            let u = self.units[e.unit as usize];
            let parts: Vec<String> = NOT_NOTED
                .iter()
                .rev()
                .filter_map(|k| {
                    u.summary_value(k).map(|(v, _)| {
                        format!("{}={}", String::from_utf8_lossy(k), String::from_utf8_lossy(v))
                    })
                })
                .collect();
            (!parts.is_empty())
                .then(|| format!("{}: {}", self.file_text(self.file_of(e)), parts.join(", ")))
        };
        match (side(a), side(b)) {
            (None, None) => None,
            (x, y) => Some([x, y].into_iter().flatten().collect::<Vec<_>>().join("; ")),
        }
    }

    /// §9.1's diagnostic: the first class "defined in", the others
    /// "defined differently in", at most 3 files a class.
    fn diagnostic(
        &self,
        kind: Kind,
        severity: Severity,
        classes: &[Class<'_>],
        keeper: Option<FileId>,
        notes: Vec<String>,
    ) -> (Key, Diagnostic) {
        let first_other = classes.get(1).and_then(|c| c.first()).unwrap_or(&classes[0][0]);
        let base = first_other.sym.map_or(ROW_SEQ + u64::from(first_other.row), |s| u64::from(s.0));
        let tag = match kind {
            Kind::OdrDeclarationMismatch { .. } => tag::KIND5,
            Kind::OdrTypeMismatch { .. } => tag::KIND3,
            Kind::OdrInternalReference { .. } => tag::INTERNAL,
            _ => tag::KIND2,
        };
        let key = Key::new(PHASE, self.file_of(first_other).0, seq(tag, base));
        let mut d = match severity {
            Severity::Error => Diagnostic::error(kind),
            Severity::Warning => Diagnostic::warning(kind),
        };
        for (c, class) in classes.iter().enumerate() {
            let relation =
                if c == 0 { Relation::DefinedIn } else { Relation::DefinedDifferentlyIn };
            let mut files: Vec<&Entry<'_>> = Vec::new();
            for e in class {
                if files.last().is_none_or(|l| self.file_of(l) != self.file_of(e)) {
                    files.push(e);
                }
            }
            for e in files.iter().take(FILES_A_CLASS) {
                d = d.with(relation, self.place(e, keeper));
            }
            if files.len() > FILES_A_CLASS {
                let rest = files.len() - FILES_A_CLASS;
                let noun = if rest == 1 { "file" } else { "files" };
                d = d.note(format!("and {rest} more {noun} with the same definition"));
            }
        }
        if let Some(units) = self.unit_note(&classes[0][0], first_other) {
            d = d.note(units);
        }
        for note in notes {
            d = d.note(note);
        }
        (key, d)
    }

    /// The record place of an entry (§9.1): the symbol for an anchored or
    /// named function or variable, the source position the row gives.
    fn place(&self, e: &Entry<'_>, keeper: Option<FileId>) -> Place {
        let file = self.file_of(e);
        let unit = self.units[e.unit as usize];
        let source = |path: u32, line: u32| {
            let p = unit.string(path);
            (!p.is_empty()).then(|| (String::from_utf8_lossy(p).into_owned(), line))
        };
        let (name, source) = match e.kind {
            2 => (
                (!e.name.is_empty()).then(|| String::from_utf8_lossy(e.name).into_owned()),
                self.definition(e).and_then(|r| source(r.file, r.line)),
            ),
            3 => (None, self.type_row(e).and_then(|r| source(r.file, r.line))),
            _ => ((!e.name.is_empty()).then(|| String::from_utf8_lossy(e.name).into_owned()), None),
        };
        Place::record(self.file_ref(file), name, keeper == Some(file), source)
    }

    /// O-30: `ENFORCE` keys with different values. Per key, the value of
    /// the first unit in file order is the first; each other value gives
    /// one diagnostic naming the first unit of each (WP-073: not every
    /// pair of values, which a hostile link could make quadratic).
    fn enforce(&self, out: &mut Vec<(Key, Diagnostic)>) {
        // (name, value, file, unit, row) of every ENFORCE row.
        let mut rows: Vec<EnforceRow<'_>> = Vec::new();
        for (u, unit) in self.units.iter().enumerate() {
            for (i, r) in unit.summary().iter().enumerate() {
                if r.flags & flags::ENFORCE != 0 {
                    let file = self.info[u].file;
                    rows.push((
                        unit.string(r.name),
                        unit.string(r.value),
                        file,
                        u as u32,
                        i as u32,
                    ));
                }
            }
        }
        rows.sort_unstable();
        let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
        let mut i = 0;
        while i < rows.len() {
            let name = rows[i].0;
            let j = rows[i..].iter().position(|r| r.0 != name).map_or(rows.len(), |p| i + p);
            // The first row of each value (sorted by file within a value).
            let mut firsts: Vec<EnforceRow<'_>> = Vec::new();
            for r in &rows[i..j] {
                if firsts.last().is_none_or(|f| f.1 != r.1) {
                    firsts.push(*r);
                }
            }
            if let Some(base) = firsts.iter().min_by_key(|r| (r.2, r.3)).copied() {
                for y in firsts.iter().filter(|r| r.1 != base.1) {
                    let key = Key::new(PHASE, y.2.0, seq(tag::ENFORCE, ROW_SEQ + u64::from(y.4)));
                    let d = Diagnostic::warning(Kind::OdrAbiMismatch {
                        option: text(name),
                        first: text(base.1),
                        second: text(y.1),
                    })
                    .with(Relation::In, Place::record(self.file_ref(base.2), None, false, None))
                    .with(Relation::In, Place::record(self.file_ref(y.2), None, false, None));
                    out.push((key, d));
                }
            }
            i = j;
        }
    }

    /// A file's value of a kind 1 key, when all its units agree on it and
    /// state it; `None` when the file has no unit, or they disagree.
    fn file_value(&self, units: &[usize], name: &[u8]) -> Option<&'l [u8]> {
        let mut value: Option<&'l [u8]> = None;
        for &u in units {
            let v = self.units[u].summary_value(name).map(|(v, _)| v)?;
            match value {
                None => value = Some(v),
                Some(w) if w == v => {}
                Some(_) => return None,
            }
        }
        value
    }

    /// The units of each file, by `FileId`, for the COMDAT checks.
    fn units_by_file(&self) -> Vec<(FileId, Vec<usize>)> {
        let mut out: Vec<(FileId, Vec<usize>)> = Vec::new();
        for (u, info) in self.info.iter().enumerate() {
            match out.last_mut() {
                Some((f, us)) if *f == info.file => us.push(u),
                _ => out.push((info.file, vec![u])),
            }
        }
        out
    }

    /// The live COMDAT groups' copies, each group's run with the keeper
    /// first, restricted to groups with a copy in a file with units.
    fn comdat_runs(&self, files: &[(FileId, Vec<usize>)]) -> Vec<(ComdatId, Vec<FileId>)> {
        let has = |f: FileId| files.binary_search_by_key(&f, |(g, _)| *g).is_ok();
        let copies = &self.resolution.comdat_copies;
        let mut out: Vec<(ComdatId, Vec<FileId>)> = Vec::new();
        let mut i = 0;
        while i < copies.len() {
            let g = copies[i].0;
            let j = copies[i..].iter().position(|c| c.0 != g).map_or(copies.len(), |p| i + p);
            let run: Vec<FileId> = copies[i..j].iter().map(|c| c.1).collect();
            if run.len() >= 2 && run.iter().filter(|f| has(**f)).count() >= 2 {
                out.push((g, run));
            }
            i = j;
        }
        out
    }

    /// O-31 and O-25 share this: per COMDAT group whose copies' files'
    /// values of `key` differ (each file's value known), classes by value,
    /// the keeper's first. `only` filters the groups by signature.
    fn by_value(
        &self,
        key: &[u8],
        only: impl Fn(&[u8]) -> bool,
        keeper_must_differ: bool,
        tag: u64,
        mut make: impl FnMut(&[u8], Vec<Vec<(FileId, &'l [u8])>>) -> Option<(Kind, Vec<String>)>,
        out: &mut Vec<(Key, Diagnostic)>,
    ) {
        let files = self.units_by_file();
        for (g, run) in self.comdat_runs(&files) {
            let Some(group) = self.inputs.comdats.get(g) else { continue };
            if !only(group.name) {
                continue;
            }
            let mut known: Vec<(FileId, &'l [u8])> = Vec::new();
            for f in &run {
                let Ok(at) = files.binary_search_by_key(f, |(h, _)| *h) else { continue };
                if let Some(v) = self.file_value(&files[at].1, key) {
                    known.push((*f, v));
                }
            }
            if known.len() < 2 {
                continue;
            }
            let keeper = run[0];
            let keeper_value = known.iter().find(|(f, _)| *f == keeper).map(|(_, v)| *v);
            if keeper_must_differ && keeper_value.is_none() {
                continue;
            }
            // Classes by value, each in file order, ordered by first file.
            let mut sorted = known.clone();
            sorted.sort_by_key(|(f, v)| (*v, *f));
            let mut classes: Vec<Vec<(FileId, &'l [u8])>> = Vec::new();
            for k in sorted {
                match classes.last_mut() {
                    Some(c) if c[0].1 == k.1 => c.push(k),
                    _ => classes.push(vec![k]),
                }
            }
            classes.sort_by_key(|c| c[0].0);
            if classes.len() < 2 {
                continue;
            }
            if let Some(at) = classes.iter().position(|c| c.iter().any(|(f, _)| *f == keeper)) {
                let c = classes.remove(at);
                classes.insert(0, c);
            }
            let first_other = classes[1][0].0;
            let Some((kind, notes)) = make(group.name, classes.clone()) else { continue };
            let name = String::from_utf8_lossy(group.name).into_owned();
            let mut d = Diagnostic::warning(kind);
            for (c, class) in classes.iter().enumerate() {
                let relation =
                    if c == 0 { Relation::DefinedIn } else { Relation::DefinedDifferentlyIn };
                for (f, _) in class.iter().take(FILES_A_CLASS) {
                    let place =
                        Place::record(self.file_ref(*f), Some(name.clone()), *f == keeper, None);
                    d = d.with(relation, place);
                }
                if class.len() > FILES_A_CLASS {
                    let rest = class.len() - FILES_A_CLASS;
                    let noun = if rest == 1 { "file" } else { "files" };
                    d = d.note(format!("and {rest} more {noun} with the same value"));
                }
            }
            for n in notes {
                d = d.note(n);
            }
            out.push((Key::new(PHASE, first_other.0, seq(tag, GROUP_SEQ + u64::from(g.0))), d));
        }
    }

    /// O-31: the kept copy's `isa` differs from another copy's unit's.
    fn isa(&self, out: &mut Vec<(Key, Diagnostic)>) {
        let names = |s: &[u8]| String::from_utf8_lossy(s).into_owned();
        self.by_value(
            b"isa",
            |_| true,
            true,
            tag::ISA,
            |sig, classes| {
                let note = format!(
                    "{}: isa={}; {}: isa={}",
                    self.file_text(classes[0][0].0),
                    names(classes[0][0].1),
                    self.file_text(classes[1][0].0),
                    names(classes[1][0].1),
                );
                Some((Kind::OdrIsaMismatch { name: names(sig) }, vec![note]))
            },
            out,
        );
    }

    /// O-25: a `_ZTV` group with copies from units whose `abi.rtti`
    /// differ. The message names the type (`<type>`, after `_ZTV`).
    fn vtables(&self, out: &mut Vec<(Key, Diagnostic)>) {
        let names = |s: &[u8]| String::from_utf8_lossy(s).into_owned();
        self.by_value(
            b"abi.rtti",
            |sig| sig.starts_with(b"_ZTV") && sig.len() > 4,
            false,
            tag::VTABLE,
            |sig, classes| {
                let note = format!(
                    "{}: abi.rtti={}; {}: abi.rtti={} (every user reads the kept copy's \
                     type_info slot)",
                    self.file_text(classes[0][0].0),
                    names(classes[0][0].1),
                    self.file_text(classes[1][0].0),
                    names(classes[1][0].1),
                );
                Some((Kind::OdrVtableRttiMismatch { name: names(&sig[4..]) }, vec![note]))
            },
            out,
        );
    }
}

/// What a kind 3 comparison compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Compared {
    /// The layout, within one scheme.
    Layout,
    /// The definition hash, within one scheme.
    Definition,
    /// Size, alignment and the shape flags, across schemes.
    Shape,
}

/// The shape flags of a kind 3 row in words.
fn shape_text(f: u16) -> String {
    let mut words = Vec::new();
    if f & flags::TYPE_ENUM != 0 {
        words.push("an enumeration");
    } else if f & flags::TYPE_UNION != 0 {
        words.push("a union");
    } else {
        words.push("a class");
    }
    if f & flags::TYPE_POLYMORPHIC != 0 {
        words.push("polymorphic");
    }
    if f & flags::TYPE_HAS_VBASES != 0 {
        words.push("with virtual bases");
    }
    words.join(", ")
}

fn partial_note() -> String {
    "a record is PARTIAL (its producer did not hash every construct): a warning, never an error \
     (spec/odr.md O-15)"
        .to_owned()
}

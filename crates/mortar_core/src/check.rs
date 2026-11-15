//! `Layout::check` (`spec/model.md` §4.4 "Validity", and the rules §4.1 to
//! §4.3 state): whether a layout may be given to `mortar_image`.
//!
//! Rules 1 to 3, 4b and 5 are checked in full from the tables; rule 3
//! compares the segments with [`derive_segments`]. Rule 4a needs a pass over
//! the relocations, which `fill` makes on every field it writes (§4.4): here
//! it is checked as far as the tables show it (the patch and redirect rows
//! are consistent, every redirect names a patch of its kind, an erratum
//! patch only from its own site, and a `Plt` thunk's key has an entry; an
//! unused patch is allowed). What was rule 6 is `fill`'s obligation.
//!
//! The rules run in a fixed order and the first violation found is the
//! answer, so the report is deterministic.

use mortar_diag::{Diagnostic, Kind};
use mortar_elf::Class;
use mortar_elf::consts::{
    PF_W, PF_X, PT_GNU_RELRO, PT_LOAD, PT_TLS, SHF_COMPRESSED, SHF_EXECINSTR, SHF_WRITE,
};

use crate::assignment::Assignment;
use crate::column::Column;
use crate::demands::{DemandFlags, Demands, GlobalFlags};
use crate::id::{ChunkId, FragId, Id, OptId, OutSecId, SectionId, SegmentId, SymKey, SymKind};
use crate::inputs::{Inputs, Reader};
use crate::layout::{
    AllocKey, ChunkKind, DefValue, Layout, PatchKind, SegmentRules, ThunkTarget, derive_segments,
};
use crate::liveness::Liveness;
use crate::resolution::{Def, FileLive, Resolution};
use crate::split::Split;

/// The rule a layout breaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rule {
    /// Every id a row names exists and every column has its id space's
    /// length; alignments are below 2^64.
    Range,
    /// §1: every address, offset and size fits the class of the link.
    Width,
    /// §4.1: folded sections fold into live, unfolded sections.
    Folds,
    /// §4.2: live fragments are placed, inside their merged chunk, aligned,
    /// without overlap.
    Merge,
    /// §4.3: every demand has exactly one slot of its kind, no slot is given
    /// twice.
    Slots,
    /// §4.4 rule 1: places.
    Places,
    /// §4.4 rule 2: chunks.
    Chunks,
    /// §4.4 rule 3: segments, TLS, RELRO, copy allocations.
    Segments,
    /// §4.4 rule 4, as far as the tables show it: patches.
    Patches,
    /// §4.4 rule 5: `sym_place`, `defined`, `sized`, `entry` present for
    /// exactly what needs them.
    Presence,
}

impl Rule {
    /// The rule's section of `spec/model.md`.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Range => "§4 ids in range",
            Self::Width => "§1 widths",
            Self::Folds => "§4.1",
            Self::Merge => "§4.2",
            Self::Slots => "§4.3",
            Self::Places => "§4.4 rule 1",
            Self::Chunks => "§4.4 rule 2",
            Self::Segments => "§4.4 rule 3",
            Self::Patches => "§4.4 rule 4",
            Self::Presence => "§4.4 rule 5",
        }
    }
}

/// A row a violation is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Section(SectionId),
    Chunk(ChunkId),
    Segment(SegmentId),
    Frag(FragId),
    Symbol(SymKey),
    /// A row of a list of the layout, by the list's name and position.
    Entry(&'static str, u32),
}

impl Row {
    fn text(self) -> String {
        match self {
            Self::Section(id) => format!("section {}", id.0),
            Self::Chunk(id) => format!("chunk {}", id.0),
            Self::Segment(id) => format!("segment {}", id.0),
            Self::Frag(id) => format!("fragment {}", id.0),
            Self::Symbol(key) => match key.kind() {
                SymKind::Global(id) => format!("symbol {}", id.0),
                SymKind::Local(sym) => {
                    format!("local symbol {} of file {}", sym.index.0, sym.file.0)
                }
            },
            Self::Entry(list, at) => format!("{list}[{at}]"),
        }
    }
}

/// Why a layout is not valid: the rule, what is wrong, and the rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub rule: Rule,
    pub what: String,
    pub rows: Vec<Row>,
}

impl From<Violation> for Diagnostic {
    /// `internal-error` (`spec/diag.md` §3.2): `invalid layout: <rule>:
    /// <what>`, with one note per offending row. A layout mortar computed
    /// that is not valid is mortar's bug (§4.4); a loaded one is not reused,
    /// which is the caller's decision, not a diagnostic.
    fn from(v: Violation) -> Self {
        let mut diag = Diagnostic::error(Kind::InternalError {
            what: format!("invalid layout: {}: {}", v.rule.label(), v.what),
        });
        for row in v.rows {
            diag = diag.note(row.text());
        }
        diag
    }
}

/// The size and alignment of an input section, from its header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Shape {
    pub size: u64,
    /// log2 of the alignment.
    pub align: u8,
}

/// Where the check reads input sections' sizes and alignments (they are in
/// the section headers, not in a table, §3.2).
pub trait SectionShapes {
    /// The shape of a section, or `None` if it cannot be read.
    fn shape(&self, section: SectionId) -> Option<Shape>;
}

impl SectionShapes for Column<SectionId, Shape> {
    fn shape(&self, section: SectionId) -> Option<Shape> {
        self.get(section).copied()
    }
}

/// Section shapes read through the readers of `Inputs`.
#[derive(Debug, Clone, Copy)]
pub struct InputShapes<'t, 'store> {
    pub inputs: &'t Inputs<'store>,
    pub resolution: &'t Resolution,
}

impl SectionShapes for InputShapes<'_, '_> {
    fn shape(&self, section: SectionId) -> Option<Shape> {
        let (file, index) = self.resolution.sh_index(section)?;
        let Reader::Object(object) = self.inputs.files.get(file)?.reader else { return None };
        let header = object.section(index).ok()?;
        let align = header.alignment().ok()?.max(1);
        align
            .is_power_of_two()
            .then(|| Shape { size: header.size(), align: align.trailing_zeros() as u8 })
    }
}

/// The derived tables and facts a layout is checked against.
#[derive(Clone, Copy)]
pub struct CheckContext<'t, 'n> {
    pub class: Class,
    /// The page size the loader uses (`-z max-page-size`, or the target's).
    pub page_size: u64,
    /// Whether the output has an entry address: an executable whose entry
    /// symbol is defined (none after `entry-not-found`, or for `-shared` and
    /// `-r` without `-e`: `spec/layout.md` L-103).
    pub needs_entry: bool,
    /// The facts and options [`derive_segments`] reads (rule 3,
    /// `spec/layout.md` L-125).
    pub segment_rules: SegmentRules,
    pub resolution: &'t Resolution,
    pub assignment: &'t Assignment<'n>,
    pub split: &'t Split,
    pub liveness: &'t Liveness,
    pub demands: &'t Demands,
    pub shapes: &'t dyn SectionShapes,
}

impl std::fmt::Debug for CheckContext<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CheckContext")
            .field("class", &self.class)
            .field("page_size", &self.page_size)
            .finish_non_exhaustive()
    }
}

type Check = Result<(), Violation>;

fn fail(rule: Rule, what: impl Into<String>, rows: impl IntoIterator<Item = Row>) -> Check {
    Err(Violation { rule, what: what.into(), rows: rows.into_iter().collect() })
}

fn need(
    holds: bool,
    rule: Rule,
    what: impl FnOnce() -> String,
    rows: impl IntoIterator<Item = Row>,
) -> Check {
    if holds { Ok(()) } else { fail(rule, what(), rows) }
}

/// The first position where a list is not strictly ascending by `key`.
fn unsorted<T, K: Ord>(rows: &[T], key: impl Fn(&T) -> K) -> Option<u32> {
    rows.windows(2).position(|p| key(&p[0]) >= key(&p[1])).map(|i| i as u32 + 1)
}

/// The first difference between two ascending sequences: `(item, true)`
/// when `want` has it and `have` does not, `(item, false)` the other way.
fn difference<T: Ord + Copy>(
    want: impl IntoIterator<Item = T>,
    have: impl IntoIterator<Item = T>,
) -> Option<(T, bool)> {
    let (mut want, mut have) = (want.into_iter().peekable(), have.into_iter().peekable());
    loop {
        match (want.peek().copied(), have.peek().copied()) {
            (None, None) => return None,
            (Some(w), None) => return Some((w, true)),
            (None, Some(h)) => return Some((h, false)),
            (Some(w), Some(h)) if w == h => {
                want.next();
                have.next();
            }
            (Some(w), Some(h)) => return Some(if w < h { (w, true) } else { (h, false) }),
        }
    }
}

fn aligned(value: u64, align: u8) -> bool {
    align >= 64 || value.trailing_zeros() >= u32::from(align) || value == 0
}

fn end(start: u64, len: u64) -> u128 {
    u128::from(start) + u128::from(len)
}

impl Layout {
    /// Whether the layout is valid (§4.4): `Ok`, or the first violation as
    /// an `internal-error` diagnostic naming the rule and the rows.
    #[allow(
        clippy::result_large_err,
        reason = "the signature of spec/model.md §10; fails once per link"
    )]
    pub fn check(&self, cx: &CheckContext<'_, '_>) -> Result<(), Diagnostic> {
        self.validate(cx).map_err(Diagnostic::from)
    }

    /// [`Layout::check`] with the violation as data.
    pub fn validate(&self, cx: &CheckContext<'_, '_>) -> Result<(), Violation> {
        let checker = Checker::new(self, cx)?;
        checker.width()?;
        checker.folds()?;
        checker.merge()?;
        checker.slots()?;
        checker.places()?;
        checker.chunks()?;
        checker.segments()?;
        checker.patches()?;
        checker.presence()
    }
}

struct Checker<'a, 't, 'n> {
    layout: &'a Layout,
    cx: &'a CheckContext<'t, 'n>,
    /// The merged chunk of each output section.
    merged: Column<OutSecId, OptId<ChunkId>>,
}

impl<'a, 't, 'n> Checker<'a, 't, 'n> {
    /// Checks [`Rule::Range`] and builds the index of merged chunks.
    #[allow(clippy::too_many_lines, reason = "one range check per column")]
    fn new(layout: &'a Layout, cx: &'a CheckContext<'t, 'n>) -> Result<Self, Violation> {
        let r = Rule::Range;
        let p = &layout.placement;
        let res = cx.resolution;
        let sections = res.sections.len();
        let symbols = res.def.len();
        let files = res.file_live.len();
        let chunks = p.chunks.len();
        let chunk = |id: ChunkId| id.index() < chunks;
        let key = |key: SymKey| match key.kind() {
            SymKind::Global(id) => id.index() < symbols,
            SymKind::Local(sym) => sym.file.index() < files,
        };
        need(p.place.len() == sections, r, || "place is not per section".into(), [])?;
        need(
            cx.liveness.live.len() == sections && cx.assignment.out_of.len() == sections,
            r,
            || "derived tables are not per section".into(),
            [],
        )?;
        need(
            layout.merge.frag_off.len() == cx.split.frags.len(),
            r,
            || "frag_off is not per fragment".into(),
            [],
        )?;
        need(
            cx.liveness.frag_live.len() == cx.split.frags.len(),
            r,
            || "frag_live is not per fragment".into(),
            [],
        )?;
        if let Some(folded) = &layout.folds.folded {
            need(folded.len() == sections, r, || "folded is not per section".into(), [])?;
            for (s, t) in folded.iter() {
                need(
                    t.get().is_none_or(|t| t.index() < sections),
                    r,
                    || "fold target out of range".into(),
                    [Row::Section(s)],
                )?;
            }
        }
        let mut merged = Column::filled(cx.assignment.outsecs.len(), OptId::NONE);
        for (id, c) in p.chunks.iter() {
            need(c.align < 64, r, || "alignment of 2^64 or more".into(), [Row::Chunk(id)])?;
            if let Some(out) = c.kind.out() {
                need(
                    cx.assignment.outsecs.has(out),
                    r,
                    || format!("output section {} out of range", out.0),
                    [Row::Chunk(id)],
                )?;
            }
            if let ChunkKind::Merged(out) = c.kind {
                need(
                    merged[out].is_none(),
                    r,
                    || format!("two merged chunks of output section {}", out.0),
                    [Row::Chunk(id)],
                )?;
                merged[out] = OptId::some(id);
            }
        }
        for (id, s) in p.segments.iter() {
            need(s.align < 64, r, || "alignment of 2^64 or more".into(), [Row::Segment(id)])?;
        }
        for (at, &c) in p.order.iter().enumerate() {
            need(chunk(c), r, || "chunk out of range".into(), [Row::Entry("order", at as u32)])?;
        }
        for (s, place) in p.place.iter() {
            need(
                place.get().is_none_or(|(c, _)| chunk(c)),
                r,
                || "chunk out of range".into(),
                [Row::Section(s)],
            )?;
        }
        for (at, row) in p.sym_place.iter().enumerate() {
            let ok = chunk(row.chunk)
                && row.align < 64
                && match row.key {
                    AllocKey::Common(s) => s.index() < symbols,
                    AllocKey::Copy(i) => (i as usize) < layout.slots.copies.len(),
                };
            need(ok, r, || "a field out of range".into(), [Row::Entry("sym_place", at as u32)])?;
        }
        for (at, row) in p.defined.iter().enumerate() {
            let ok = row.symbol.index() < symbols
                && !matches!(row.value, DefValue::At { chunk: c, .. } if !chunk(c));
            need(ok, r, || "a field out of range".into(), [Row::Entry("defined", at as u32)])?;
        }
        for (at, row) in p.patches.iter().enumerate() {
            let ok = chunk(row.chunk)
                && match row.kind {
                    PatchKind::Thunk(ThunkTarget::Sym { key: k, .. } | ThunkTarget::Plt(k)) => {
                        key(k)
                    }
                    PatchKind::Erratum { section, .. } => section.index() < sections,
                };
            need(ok, r, || "a field out of range".into(), [Row::Entry("patches", at as u32)])?;
        }
        for (at, row) in p.redirects.iter().enumerate() {
            let ok = row.section.index() < sections;
            need(ok, r, || "a field out of range".into(), [Row::Entry("redirects", at as u32)])?;
        }
        for (at, row) in p.sized.iter().enumerate() {
            need(
                chunk(row.chunk),
                r,
                || "chunk out of range".into(),
                [Row::Entry("sized", at as u32)],
            )?;
        }
        let slots = &layout.slots;
        for (name, list) in Self::slot_lists(layout) {
            for (at, &k) in list.iter().enumerate() {
                need(key(k), r, || "symbol out of range".into(), [Row::Entry(name, at as u32)])?;
            }
        }
        for (at, s) in slots.dynsym.iter().enumerate() {
            need(
                s.index() < symbols,
                r,
                || "symbol out of range".into(),
                [Row::Entry("dynsym", at as u32)],
            )?;
        }
        for (at, f) in slots.needed.iter().enumerate() {
            need(
                f.index() < files,
                r,
                || "file out of range".into(),
                [Row::Entry("needed", at as u32)],
            )?;
        }
        for (at, row) in slots.copies.iter().enumerate() {
            need(
                row.file.index() < files,
                r,
                || "file out of range".into(),
                [Row::Entry("copies", at as u32)],
            )?;
        }
        for (at, row) in slots.copy_of.iter().enumerate() {
            let ok = row.0.index() < symbols && (row.1 as usize) < slots.copies.len();
            need(ok, r, || "a field out of range".into(), [Row::Entry("copy_of", at as u32)])?;
        }
        for (at, row) in slots.eh_records.iter().enumerate() {
            need(
                row.section.index() < sections,
                r,
                || "section out of range".into(),
                [Row::Entry("eh_records", at as u32)],
            )?;
        }
        for (at, s) in slots.dyn_relocs.iter().enumerate() {
            need(
                s.index() < sections,
                r,
                || "section out of range".into(),
                [Row::Entry("dyn_relocs", at as u32)],
            )?;
        }
        need(slots.symtab_locals.len() == files, r, || "symtab_locals is not per file".into(), [])?;
        for (at, row) in slots.sizes.iter().enumerate() {
            need(
                chunk(row.0),
                r,
                || "chunk out of range".into(),
                [Row::Entry("sizes", at as u32)],
            )?;
        }
        Ok(Self { layout, cx, merged })
    }

    fn slot_lists(layout: &Layout) -> [(&'static str, &[SymKey]); 6] {
        let s = &layout.slots;
        [
            ("got", &s.got),
            ("plt", &s.plt),
            ("iplt", &s.iplt),
            ("tls_desc", &s.tls_desc),
            ("tls_gd", &s.tls_gd),
            ("tls_ie", &s.tls_ie),
        ]
    }

    /// §1: every value fits the class.
    fn width(&self) -> Check {
        let r = Rule::Width;
        let limit: u128 = match self.cx.class {
            Class::Elf32 => 1 << 32,
            Class::Elf64 => 1 << 64,
        };
        let p = &self.layout.placement;
        for (id, c) in p.chunks.iter() {
            let ok = end(c.offset, c.capacity) <= limit
                && c.addr.is_none_or(|a| end(a, c.capacity) <= limit);
            need(ok, r, || "the chunk does not fit the class".into(), [Row::Chunk(id)])?;
        }
        for (id, s) in p.segments.iter() {
            let ok = end(s.offset, s.filesz) <= limit && end(s.addr, s.memsz) <= limit;
            need(ok, r, || "the segment does not fit the class".into(), [Row::Segment(id)])?;
        }
        need(
            p.entry.is_none_or(|e| u128::from(e) < limit),
            r,
            || "the entry does not fit the class".into(),
            [],
        )?;
        need(
            p.tls.is_none_or(|t| end(t.start, t.size) <= limit),
            r,
            || "the TLS template does not fit the class".into(),
            [],
        )
    }

    /// §4.1.
    fn folds(&self) -> Check {
        let r = Rule::Folds;
        let Some(folded) = &self.layout.folds.folded else { return Ok(()) };
        let live = |s: SectionId| self.cx.liveness.is_live(s);
        for (s, t) in folded.iter() {
            let Some(t) = t.get() else { continue };
            need(t != s, r, || "a section folded into itself".into(), [Row::Section(s)])?;
            need(
                !self.layout.folds.is_folded(t),
                r,
                || "the target is itself folded".into(),
                [Row::Section(s), Row::Section(t)],
            )?;
            need(
                live(s) && live(t),
                r,
                || "a dead section in a fold".into(),
                [Row::Section(s), Row::Section(t)],
            )?;
        }
        Ok(())
    }

    /// §4.2.
    fn merge(&self) -> Check {
        let r = Rule::Merge;
        let cx = self.cx;
        let chunks = &self.layout.placement.chunks;
        let mut runs = Vec::new();
        for (f, row) in cx.split.frags.iter() {
            let off = self.layout.merge.offset(f);
            let live = cx.liveness.frag_live[f];
            need(
                live == off.is_some(),
                r,
                || {
                    if live {
                        "a live fragment has no offset".into()
                    } else {
                        "a dead fragment has an offset".into()
                    }
                },
                [Row::Frag(f)],
            )?;
            let Some(off) = off else { continue };
            let Some(out) = cx.assignment.out(row.section) else {
                return fail(
                    r,
                    "a live fragment of a section with no output section",
                    [Row::Frag(f), Row::Section(row.section)],
                );
            };
            let Some(chunk) = self.merged[out].get() else {
                return fail(
                    r,
                    format!("output section {} has no merged chunk", out.0),
                    [Row::Frag(f)],
                );
            };
            let c = &chunks[chunk];
            need(
                end(off, row.size.into()) <= c.size.into(),
                r,
                || "the fragment ends past its chunk".into(),
                [Row::Frag(f), Row::Chunk(chunk)],
            )?;
            need(
                aligned(off, row.align) && c.align >= row.align,
                r,
                || "the fragment is misaligned".into(),
                [Row::Frag(f), Row::Chunk(chunk)],
            )?;
            runs.push((out, off, end(off, row.size.into()), f));
        }
        runs.sort_unstable();
        let mut last: Option<(OutSecId, u128, FragId)> = None;
        for &(out, off, stop, f) in &runs {
            if let Some((o, max, g)) = last
                && o == out
                && u128::from(off) < max
                && stop > off.into()
            {
                return fail(r, "two fragments overlap", [Row::Frag(g), Row::Frag(f)]);
            }
            if last.is_none_or(|(o, max, _)| o != out || stop > max) {
                last = Some((out, stop, f));
            }
        }
        Ok(())
    }

    /// §4.3.
    #[allow(clippy::too_many_lines, reason = "one list of Slots after another")]
    fn slots(&self) -> Check {
        let r = Rule::Slots;
        let cx = self.cx;
        let slots = &self.layout.slots;
        let flags = [
            DemandFlags::GOT,
            DemandFlags::PLT | DemandFlags::CANON_PLT,
            DemandFlags::IPLT,
            DemandFlags::TLS_DESC,
            DemandFlags::TLS_GD,
            DemandFlags::GOT_TPREL,
        ];
        for ((name, list), want) in Self::slot_lists(self.layout).into_iter().zip(flags) {
            let mut have: Vec<(SymKey, u32)> =
                list.iter().enumerate().map(|(i, &k)| (k, i as u32)).collect();
            have.sort_unstable();
            if let Some(pair) = have.windows(2).find(|p| p[0].0 == p[1].0) {
                return fail(
                    r,
                    format!("a {name} slot given twice"),
                    [Row::Symbol(pair[0].0), Row::Entry(name, pair[1].1)],
                );
            }
            if let Some((key, missing)) =
                difference(cx.demands.keys_with(want), have.iter().map(|p| p.0))
            {
                let what = if missing {
                    format!("a {name} demand without a slot")
                } else {
                    format!("a {name} slot without a demand")
                };
                return fail(r, what, [Row::Symbol(key)]);
            }
        }
        let module = cx.demands.output.contains(GlobalFlags::TLS_LD);
        need(
            slots.tls_ld == module,
            r,
            || {
                if module {
                    "a TLS_LD demand without the module slot".into()
                } else {
                    "the module slot without a TLS_LD demand".into()
                }
            },
            [],
        )?;
        let mut dynsym = slots.dynsym.to_vec();
        dynsym.sort_unstable();
        if let Some(pair) = dynsym.windows(2).find(|p| p[0] == p[1]) {
            return fail(r, "a symbol twice in .dynsym", [Row::Symbol(pair[0].into())]);
        }
        let exported = cx.demands.keys_with(DemandFlags::EXPORT).filter_map(SymKey::as_global);
        if let Some((s, missing)) = difference(exported, dynsym.iter().copied()) {
            let what = if missing {
                "an export demand not in .dynsym"
            } else {
                "a .dynsym entry without an export demand"
            };
            return fail(r, what, [Row::Symbol(s.into())]);
        }
        if let Some(at) = unsorted(&slots.copies, |c| *c) {
            return fail(r, "copies not sorted or given twice", [Row::Entry("copies", at)]);
        }
        if let Some(at) = unsorted(&slots.copy_of, |c| c.0) {
            return fail(r, "copy_of not sorted or given twice", [Row::Entry("copy_of", at)]);
        }
        let copied = cx.demands.keys_with(DemandFlags::COPY).filter_map(SymKey::as_global);
        if let Some((s, missing)) = difference(copied, slots.copy_of.iter().map(|c| c.0)) {
            let what = if missing {
                "a copy demand without an allocation"
            } else {
                "a copy allocation without a demand"
            };
            return fail(r, what, [Row::Symbol(s.into())]);
        }
        let mut used = vec![false; slots.copies.len()];
        for row in &slots.copy_of {
            used[row.1 as usize] = true;
        }
        if let Some(at) = used.iter().position(|u| !u) {
            return fail(r, "a copy allocation no symbol uses", [Row::Entry("copies", at as u32)]);
        }
        let mut needed = slots.needed.to_vec();
        needed.sort_unstable();
        if let Some(pair) = needed.windows(2).find(|p| p[0] == p[1]) {
            return fail(r, format!("file {} needed twice", pair[0].0), []);
        }
        for (at, &f) in slots.needed.iter().enumerate() {
            need(
                cx.resolution.file_live[f] == FileLive::Needed,
                r,
                || "DT_NEEDED for a file that is not a needed shared object".into(),
                [Row::Entry("needed", at as u32)],
            )?;
        }
        let mut eh = slots.eh_records.to_vec();
        eh.sort_unstable();
        if let Some(pair) = eh.windows(2).find(|p| p[0] == p[1]) {
            return fail(r, "an .eh_frame record given twice", [Row::Section(pair[0].section)]);
        }
        for (at, row) in slots.eh_records.iter().enumerate() {
            let ok = cx.split.eh_pieces(row.section).is_some_and(|span| row.piece < span.len);
            need(
                ok,
                r,
                || "an .eh_frame record that is no piece".into(),
                [Row::Entry("eh_records", at as u32)],
            )?;
        }
        let mut relocs = slots.dyn_relocs.to_vec();
        relocs.sort_unstable();
        if let Some(pair) = relocs.windows(2).find(|p| p[0] == p[1]) {
            return fail(r, "a section's dynamic relocations given twice", [Row::Section(pair[0])]);
        }
        if let Some((s, missing)) =
            difference(cx.demands.dyn_relocs.iter().map(|d| d.0), relocs.iter().copied())
        {
            let what = if missing {
                "dynamic relocations with no place in the order"
            } else {
                "a section without dynamic relocations in their order"
            };
            return fail(r, what, [Row::Section(s)]);
        }
        let mut next = None;
        for (f, span) in slots.symtab_locals.iter() {
            need(
                next.is_none_or(|n| u64::from(span.start) == n),
                r,
                || "symtab_locals not contiguous".into(),
                [Row::Entry("symtab_locals", f.0)],
            )?;
            next = Some(span.end());
        }
        if let Some(at) = unsorted(&slots.sizes, |s| s.0) {
            return fail(r, "sizes not sorted or given twice", [Row::Entry("sizes", at)]);
        }
        for &(chunk, size) in &slots.sizes {
            let c = &self.layout.placement.chunks[chunk];
            need(
                matches!(c.kind, ChunkKind::Synthetic(_)),
                r,
                || "a slot size for a chunk that is not synthetic".into(),
                [Row::Chunk(chunk)],
            )?;
            need(
                c.size == size,
                r,
                || "the chunk's size is not its slots' size".into(),
                [Row::Chunk(chunk)],
            )?;
        }
        Ok(())
    }

    /// §4.4 rule 1.
    fn places(&self) -> Check {
        let r = Rule::Places;
        let cx = self.cx;
        let p = &self.layout.placement;
        let mut taken = Vec::new();
        for (s, row) in cx.resolution.sections.iter() {
            let out = cx.assignment.out(s);
            let needs =
                row.is_kept() && cx.liveness.live[s] && row.kind.is_placed() && out.is_some();
            let place = p.place[s].get();
            let Some((chunk, off)) = place else {
                need(!needs, r, || "a live section has no place".into(), [Row::Section(s)])?;
                continue;
            };
            need(needs, r, || "a section that needs no place has one".into(), [Row::Section(s)])?;
            let c = &p.chunks[chunk];
            need(
                out.is_some_and(|o| matches!(c.kind, ChunkKind::Body(part) if part.out == o)),
                r,
                || "the section is not in the body of its output section".into(),
                [Row::Section(s), Row::Chunk(chunk)],
            )?;
            if let Some(target) = self.layout.folds.target(s) {
                need(
                    p.place[target] == p.place[s],
                    r,
                    || "a folded section does not have its target's place".into(),
                    [Row::Section(s), Row::Section(target)],
                )?;
                continue;
            }
            let Some(shape) = cx.shapes.shape(s) else {
                return fail(
                    r,
                    "the section's size and alignment cannot be read",
                    [Row::Section(s)],
                );
            };
            need(
                end(off, shape.size) <= c.size.into(),
                r,
                || "the section ends past its chunk's size".into(),
                [Row::Section(s), Row::Chunk(chunk)],
            )?;
            need(
                aligned(off, shape.align) && c.align >= shape.align,
                r,
                || "the section is misaligned".into(),
                [Row::Section(s), Row::Chunk(chunk)],
            )?;
            taken.push((chunk, off, end(off, shape.size), Row::Section(s)));
        }
        for (at, row) in p.sym_place.iter().enumerate() {
            let c = &p.chunks[row.chunk];
            let me = Row::Entry("sym_place", at as u32);
            need(
                c.is_alloc() && end(row.offset, row.size) <= c.size.into(),
                r,
                || "the allocation is not inside an alloc chunk".into(),
                [me, Row::Chunk(row.chunk)],
            )?;
            need(
                aligned(row.offset, row.align) && c.align >= row.align,
                r,
                || "the allocation is misaligned".into(),
                [me, Row::Chunk(row.chunk)],
            )?;
            taken.push((row.chunk, row.offset, end(row.offset, row.size), me));
        }
        taken.sort_unstable_by_key(|t| (t.0, t.1, t.2));
        let mut last: Option<(ChunkId, u128, Row)> = None;
        for &(chunk, off, stop, row) in &taken {
            if let Some((c, max, prev)) = last
                && c == chunk
                && u128::from(off) < max
                && stop > off.into()
            {
                return fail(r, "two places overlap", [prev, row]);
            }
            if last.is_none_or(|(c, max, _)| c != chunk || stop > max) {
                last = Some((chunk, stop, row));
            }
        }
        Ok(())
    }

    /// §4.4 rule 2.
    #[allow(clippy::too_many_lines, reason = "the clauses of rule 2 in order")]
    fn chunks(&self) -> Check {
        let r = Rule::Chunks;
        let p = &self.layout.placement;
        let page = self.cx.page_size;
        let mut seen = vec![false; p.chunks.len() as usize];
        for (at, &c) in p.order.iter().enumerate() {
            need(
                !seen[c.slot()],
                r,
                || "a chunk twice in order".into(),
                [Row::Chunk(c), Row::Entry("order", at as u32)],
            )?;
            seen[c.slot()] = true;
        }
        if let Some(c) = seen.iter().position(|s| !s) {
            return fail(r, "a chunk missing from order", [Row::Chunk(ChunkId(c as u32))]);
        }
        need(
            page.is_power_of_two(),
            r,
            || format!("page size {page:#x} is not a power of two"),
            [],
        )?;
        for (id, c) in p.chunks.iter() {
            let me = [Row::Chunk(id)];
            need(c.size <= c.capacity, r, || "size exceeds capacity".into(), me)?;
            need(
                c.is_alloc() == c.addr.is_some(),
                r,
                || {
                    if c.is_alloc() {
                        "an alloc chunk has no address".into()
                    } else {
                        "a non-alloc chunk has an address".into()
                    }
                },
                me,
            )?;
            match c.addr {
                Some(addr) => {
                    need(
                        aligned(addr, c.align),
                        r,
                        || "the chunk's address is misaligned".into(),
                        me,
                    )?;
                    need(
                        !c.in_file() || addr % page == c.offset % page,
                        r,
                        || "address and offset differ modulo the page size".into(),
                        me,
                    )?;
                }
                None => need(
                    !c.in_file() || aligned(c.offset, c.align),
                    r,
                    || "the chunk's offset is misaligned".into(),
                    me,
                )?,
            }
        }
        let mut file_end: Option<(u128, ChunkId)> = None;
        for &id in &p.order {
            let c = &p.chunks[id];
            if !c.in_file() || c.capacity == 0 {
                continue;
            }
            if let Some((stop, prev)) = file_end {
                need(
                    u128::from(c.offset) >= stop,
                    r,
                    || "chunks overlap in the file, or order is not file order".into(),
                    [Row::Chunk(prev), Row::Chunk(id)],
                )?;
            }
            file_end = Some((end(c.offset, c.capacity), id));
        }
        let mut memory: Vec<(u64, u128, ChunkId)> = p
            .chunks
            .iter()
            .filter(|(_, c)| c.takes_memory())
            .filter_map(|(id, c)| c.addr.map(|a| (a, end(a, c.capacity), id)))
            .collect();
        memory.sort_unstable();
        for pair in memory.windows(2) {
            need(
                u128::from(pair[1].0) >= pair[0].1,
                r,
                || "chunks overlap in memory".into(),
                [Row::Chunk(pair[0].2), Row::Chunk(pair[1].2)],
            )?;
        }
        let mut count = 0u32;
        let mut previous: Option<u32> = None;
        let mut done = vec![false; p.chunks.len() as usize + 1];
        for &id in &p.order {
            let Some(index) = p.chunks[id].shndx else { continue };
            need(
                index != 0 && (index as usize) < done.len(),
                r,
                || format!("section index {index} out of range"),
                [Row::Chunk(id)],
            )?;
            if previous != Some(index) {
                need(
                    !done[index as usize],
                    r,
                    || format!("the chunks of section index {index} are not consecutive"),
                    [Row::Chunk(id)],
                )?;
                done[index as usize] = true;
                count = count.max(index);
            }
            previous = Some(index);
        }
        let missing = (1..=count).find(|&i| !done[i as usize]);
        need(
            missing.is_none(),
            r,
            || format!("section index {} is not used", missing.unwrap_or(0)),
            [],
        )
    }

    /// §4.4 rule 3.
    fn segments(&self) -> Check {
        let r = Rule::Segments;
        let p = &self.layout.placement;
        let page = self.cx.page_size;
        let mut loads = Vec::new();
        for (id, s) in p.segments.iter() {
            need(
                s.filesz <= s.memsz,
                r,
                || "file size exceeds memory size".into(),
                [Row::Segment(id)],
            )?;
            if s.p_type == PT_LOAD {
                let a = 1u64 << s.align;
                need(
                    s.offset % a == s.addr % a,
                    r,
                    || "offset and address differ modulo the alignment".into(),
                    [Row::Segment(id)],
                )?;
                loads.push(id);
            }
        }
        for pair in loads.windows(2) {
            let (a, b) = (&p.segments[pair[0]], &p.segments[pair[1]]);
            need(
                u128::from(b.addr) >= end(a.addr, a.memsz),
                r,
                || "PT_LOAD segments overlap or are out of order".into(),
                [Row::Segment(pair[0]), Row::Segment(pair[1])],
            )?;
        }
        for (id, c) in p.chunks.iter() {
            let Some(addr) = c.addr.filter(|_| c.takes_memory()) else { continue };
            let stop = end(addr, c.capacity);
            let holders: Vec<SegmentId> = loads
                .iter()
                .copied()
                .filter(|&l| {
                    let s = &p.segments[l];
                    s.addr <= addr && stop <= end(s.addr, s.memsz)
                })
                .collect();
            let [load] = holders[..] else {
                return fail(r, "the chunk is not in exactly one PT_LOAD", [Row::Chunk(id)]);
            };
            let s = &p.segments[load];
            let rows = [Row::Chunk(id), Row::Segment(load)];
            if c.in_file() {
                let ok = c.offset.checked_sub(s.offset) == Some(addr - s.addr)
                    && stop <= end(s.addr, s.filesz);
                need(
                    ok,
                    r,
                    || "the chunk's bytes are not where its PT_LOAD maps them".into(),
                    rows,
                )?;
            } else {
                need(
                    u128::from(addr) >= end(s.addr, s.filesz),
                    r,
                    || "a NOBITS chunk inside the file part of its PT_LOAD".into(),
                    rows,
                )?;
            }
            need(
                c.flags & SHF_WRITE == 0 || s.flags & PF_W != 0,
                r,
                || "a writable chunk in a read-only PT_LOAD".into(),
                rows,
            )?;
            need(
                c.flags & SHF_EXECINSTR == 0 || s.flags & PF_X != 0,
                r,
                || "an executable chunk in a PT_LOAD without PF_X".into(),
                rows,
            )?;
        }
        self.tls()?;
        self.relro()?;
        for (at, row) in p.sym_place.iter().enumerate() {
            if let AllocKey::Copy(_) = row.key {
                need(
                    p.chunks[row.chunk].flags & SHF_WRITE != 0,
                    r,
                    || "a copy allocation in read-only memory".into(),
                    [Row::Entry("sym_place", at as u32), Row::Chunk(row.chunk)],
                )?;
            }
        }
        need(page > 0, r, || "page size 0".into(), [])?;
        self.derived()
    }

    /// Rule 3, `spec/layout.md` L-125: the segments are exactly what
    /// [`derive_segments`] derives from the chunks and their order.
    fn derived(&self) -> Check {
        let r = Rule::Segments;
        let p = &self.layout.placement;
        let want = derive_segments(&p.chunks, &p.order, &self.cx.segment_rules);
        let have = p.segments.as_slice();
        if let Some(at) = (0..want.len().max(have.len())).find(|&i| want.get(i) != have.get(i)) {
            let what = match (want.get(at), have.get(at)) {
                (Some(w), Some(h)) => {
                    format!("segment {at} is not the derived one: {h:?}, derived {w:?}")
                }
                (Some(w), None) => format!("a derived segment is missing: {w:?}"),
                (None, _) => format!("segment {at} is not derived from the chunks"),
            };
            return fail(r, what, [Row::Segment(SegmentId(at as u32))]);
        }
        Ok(())
    }

    /// Rule 3, TLS: the initialized TLS chunks are directly followed by the
    /// uninitialized ones, `tls` describes them, and one `PT_TLS` matches.
    fn tls(&self) -> Check {
        let r = Rule::Segments;
        let p = &self.layout.placement;
        let tls: Vec<(usize, ChunkId)> = p
            .order
            .iter()
            .enumerate()
            .filter(|(_, c)| p.chunks[**c].is_tls())
            .map(|(i, &c)| (i, c))
            .collect();
        let segs: Vec<SegmentId> =
            p.segments.iter().filter(|(_, s)| s.p_type == PT_TLS).map(|(id, _)| id).collect();
        let (Some(&(first_at, first)), Some(&(last_at, last))) = (tls.first(), tls.last()) else {
            need(p.tls.is_none(), r, || "a TLS template without TLS chunks".into(), [])?;
            return need(
                segs.is_empty(),
                r,
                || "PT_TLS without TLS chunks".into(),
                segs.iter().map(|&s| Row::Segment(s)),
            );
        };
        need(
            last_at - first_at + 1 == tls.len(),
            r,
            || "the TLS chunks are split".into(),
            [Row::Chunk(first), Row::Chunk(last)],
        )?;
        let mut init_end = None;
        let mut align = 0;
        let mut prev_end: Option<u128> = None;
        let mut uninitialized = false;
        for &(_, id) in &tls {
            let c = &p.chunks[id];
            let Some(addr) = c.addr else {
                return fail(r, "a TLS chunk with no address", [Row::Chunk(id)]);
            };
            need(
                prev_end.is_none_or(|e| u128::from(addr) >= e),
                r,
                || "TLS chunks overlap or are out of order".into(),
                [Row::Chunk(id)],
            )?;
            prev_end = Some(end(addr, c.size));
            if c.in_file() {
                need(
                    !uninitialized,
                    r,
                    || "an initialized TLS chunk after an uninitialized one".into(),
                    [Row::Chunk(id)],
                )?;
                init_end = Some(end(addr, c.size));
            } else {
                uninitialized = true;
            }
            align = align.max(c.align);
        }
        let start = p.chunks[first].addr.unwrap_or(0);
        let size = prev_end.unwrap_or(0) - u128::from(start);
        let Some(tmpl) = p.tls else {
            return fail(r, "TLS chunks without a TLS template", [Row::Chunk(first)]);
        };
        need(
            tmpl.start == start && u128::from(tmpl.size) == size && tmpl.align == align,
            r,
            || "tls does not describe the TLS chunks".into(),
            [Row::Chunk(first), Row::Chunk(last)],
        )?;
        need(
            aligned(tmpl.start, tmpl.align),
            r,
            || "the TLS template's start is not a multiple of its alignment".into(),
            [Row::Chunk(first)],
        )?;
        let [seg] = segs[..] else {
            return fail(r, "not exactly one PT_TLS", segs.iter().map(|&s| Row::Segment(s)));
        };
        let s = &p.segments[seg];
        let filesz = init_end.map_or(0, |e| e - u128::from(start));
        need(
            s.addr == tmpl.start
                && s.memsz == tmpl.size
                && u128::from(s.filesz) == filesz
                && s.align == tmpl.align,
            r,
            || "PT_TLS does not match the TLS template".into(),
            [Row::Segment(seg)],
        )?;
        // spec/layout.md L-76, L-128: the template's file image starts at a
        // multiple of its alignment too.
        let c = &p.chunks[first];
        need(
            aligned(s.offset, tmpl.align) && (!c.in_file() || aligned(c.offset, tmpl.align)),
            r,
            || "the TLS template's file offset is not a multiple of its alignment".into(),
            [Row::Chunk(first), Row::Segment(seg)],
        )
    }

    /// Rule 3, RELRO: the RELRO chunks are consecutive, and one
    /// `PT_GNU_RELRO` covers exactly them, ending on a page boundary.
    #[allow(
        clippy::many_single_char_names,
        reason = "rule, placement, page, segment: the names of the rule"
    )]
    fn relro(&self) -> Check {
        let r = Rule::Segments;
        let p = &self.layout.placement;
        let page = u128::from(self.cx.page_size.max(1));
        let runs: Vec<(usize, ChunkId)> = p
            .order
            .iter()
            .enumerate()
            .filter(|(_, c)| p.chunks[**c].relro)
            .map(|(i, &c)| (i, c))
            .collect();
        let segs: Vec<SegmentId> =
            p.segments.iter().filter(|(_, s)| s.p_type == PT_GNU_RELRO).map(|(id, _)| id).collect();
        let (Some(&(first_at, first)), Some(&(last_at, last))) = (runs.first(), runs.last()) else {
            return need(
                segs.is_empty(),
                r,
                || "PT_GNU_RELRO without RELRO chunks".into(),
                segs.iter().map(|&s| Row::Segment(s)),
            );
        };
        need(
            last_at - first_at + 1 == runs.len(),
            r,
            || "the RELRO run is not contiguous".into(),
            [Row::Chunk(first), Row::Chunk(last)],
        )?;
        // spec/layout.md L-64, L-129: the padding ends the run.
        need(
            p.chunks[last].is_relro_padding(),
            r,
            || "the RELRO run does not end with .relro_padding".into(),
            [Row::Chunk(last)],
        )?;
        let [seg] = segs[..] else {
            return fail(r, "not exactly one PT_GNU_RELRO", segs.iter().map(|&s| Row::Segment(s)));
        };
        let s = &p.segments[seg];
        let seg_end = end(s.addr, s.memsz);
        let (Some(a), Some(b)) = (p.chunks[first].addr, p.chunks[last].addr) else {
            return fail(r, "a RELRO chunk with no address", [Row::Chunk(first)]);
        };
        need(
            s.addr == a && seg_end >= end(b, p.chunks[last].capacity),
            r,
            || "PT_GNU_RELRO does not cover the RELRO run".into(),
            [Row::Segment(seg)],
        )?;
        need(
            seg_end.is_multiple_of(page),
            r,
            || "PT_GNU_RELRO does not end on a page boundary".into(),
            [Row::Segment(seg)],
        )?;
        for (id, c) in p.chunks.iter() {
            if let Some(addr) = c.addr
                && !c.relro
                && c.takes_memory()
                && u128::from(addr) < seg_end
                && end(addr, c.capacity) > u128::from(s.addr)
            {
                return fail(
                    r,
                    "a chunk that is not RELRO inside PT_GNU_RELRO",
                    [Row::Chunk(id), Row::Segment(seg)],
                );
            }
        }
        Ok(())
    }

    /// §4.4 rule 4, the part the tables show.
    fn patches(&self) -> Check {
        let r = Rule::Patches;
        let p = &self.layout.placement;
        if let Some(at) = unsorted(&p.patches, |x| (x.chunk, x.offset)) {
            return fail(r, "patches not sorted or two at one place", [Row::Entry("patches", at)]);
        }
        for (at, patch) in p.patches.iter().enumerate() {
            let c = &p.chunks[patch.chunk];
            let ok = matches!(c.kind, ChunkKind::Patches(_)) && patch.offset < c.size;
            need(
                ok,
                r,
                || "a patch outside a patch chunk".into(),
                [Row::Entry("patches", at as u32), Row::Chunk(patch.chunk)],
            )?;
        }
        if let Some(at) = unsorted(&p.redirects, |x| (x.section, x.offset)) {
            return fail(
                r,
                "redirects not sorted or two for one site",
                [Row::Entry("redirects", at)],
            );
        }
        for (at, patch) in p.patches.iter().enumerate() {
            if let PatchKind::Thunk(ThunkTarget::Plt(key)) = patch.kind {
                // The Slots rule has run: demands and entries agree.
                let entry = DemandFlags::PLT | DemandFlags::CANON_PLT | DemandFlags::IPLT;
                need(
                    self.cx.demands.of(key).intersects(entry),
                    r,
                    || "a thunk to the PLT entry of a key with no PLT or IPLT entry".into(),
                    [Row::Entry("patches", at as u32), Row::Symbol(key)],
                )?;
            }
        }
        for (at, row) in p.redirects.iter().enumerate() {
            let me = Row::Entry("redirects", at as u32);
            need(
                p.place[row.section].get().is_some(),
                r,
                || "a redirected site in a section with no place".into(),
                [me, Row::Section(row.section)],
            )?;
            let size = self.cx.shapes.shape(row.section).map_or(0, |s| s.size);
            need(
                row.offset < size,
                r,
                || "a redirected site outside its section".into(),
                [me, Row::Section(row.section)],
            )?;
            let Some(patch) = p.patches.get(row.patch as usize) else {
                return fail(r, "a site redirected to a patch that does not exist", [me]);
            };
            if let PatchKind::Erratum { section, offset, .. } = patch.kind {
                need(
                    (section, offset) == (row.section, row.offset),
                    r,
                    || "a site redirected to the erratum patch of another site".into(),
                    [me, Row::Entry("patches", row.patch)],
                )?;
            }
        }
        self.islands()?;
        // A patch no site uses is allowed (M-4): the fixed point only adds.
        Ok(())
    }

    /// Rule 4, the island policy (`spec/layout.md` L-111, L-126): an
    /// erratum patch lies in the island of its site's body part; an island
    /// holds at most one thunk per target and one erratum patch per site.
    fn islands(&self) -> Check {
        let r = Rule::Patches;
        let p = &self.layout.placement;
        let mut keys: Vec<(ChunkId, u8, SymKey, i64, SectionId, u64, u32)> = Vec::new();
        for (at, patch) in p.patches.iter().enumerate() {
            let me = Row::Entry("patches", at as u32);
            let k = match patch.kind {
                PatchKind::Thunk(ThunkTarget::Sym { key, addend }) => {
                    (patch.chunk, 0, key, addend, SectionId(0), 0, at as u32)
                }
                PatchKind::Thunk(ThunkTarget::Plt(key)) => {
                    (patch.chunk, 1, key, 0, SectionId(0), 0, at as u32)
                }
                PatchKind::Erratum { section, offset, .. } => {
                    if let Some((body, _)) = p.place[section].get()
                        && let ChunkKind::Body(part) = p.chunks[body].kind
                    {
                        need(
                            p.chunks[patch.chunk].kind == ChunkKind::Patches(part),
                            r,
                            || "an erratum patch outside the island of its site's part".into(),
                            [me, Row::Section(section), Row::Chunk(patch.chunk)],
                        )?;
                    }
                    (
                        patch.chunk,
                        2,
                        SymKey::global(crate::id::SymbolId(0)),
                        0,
                        section,
                        offset,
                        at as u32,
                    )
                }
            };
            keys.push(k);
        }
        keys.sort_unstable();
        let same = |a: &(ChunkId, u8, SymKey, i64, SectionId, u64, u32),
                    b: &(ChunkId, u8, SymKey, i64, SectionId, u64, u32)| {
            (a.0, a.1, a.2, a.3, a.4, a.5) == (b.0, b.1, b.2, b.3, b.4, b.5)
        };
        if let Some(pair) = keys.windows(2).find(|w| same(&w[0], &w[1])) {
            return fail(
                r,
                "two patches of one target or site in one island",
                [Row::Entry("patches", pair[0].6), Row::Entry("patches", pair[1].6)],
            );
        }
        Ok(())
    }

    /// §4.4 rule 5.
    #[allow(clippy::too_many_lines, reason = "one part of Placement after another")]
    fn presence(&self) -> Check {
        let r = Rule::Presence;
        let cx = self.cx;
        let p = &self.layout.placement;
        if let Some(at) = unsorted(&p.sym_place, |x| x.key) {
            return fail(
                r,
                "sym_place not sorted or two for one key",
                [Row::Entry("sym_place", at)],
            );
        }
        let commons =
            cx.resolution.def.iter().filter(|(_, d)| matches!(d, Def::Common(_))).map(|(s, _)| s);
        let placed = p
            .sym_place
            .iter()
            .filter_map(|x| if let AllocKey::Common(s) = x.key { Some(s) } else { None });
        if let Some((s, missing)) = difference(commons, placed) {
            let what = if missing {
                "a common symbol with no allocation"
            } else {
                "an allocation for a symbol that is not common"
            };
            return fail(r, what, [Row::Symbol(s.into())]);
        }
        for (at, row) in p.sym_place.iter().enumerate() {
            if let AllocKey::Common(s) = row.key {
                let shape = cx.resolution.common_of(s);
                let ok = shape.is_some_and(|c| c.size == row.size && row.align >= c.align);
                need(
                    ok,
                    r,
                    || "the allocation does not have the common's size and alignment".into(),
                    [Row::Entry("sym_place", at as u32), Row::Symbol(s.into())],
                )?;
            }
        }
        let copies = 0..self.layout.slots.copies.len() as u32;
        let placed = p
            .sym_place
            .iter()
            .filter_map(|x| if let AllocKey::Copy(i) = x.key { Some(i) } else { None });
        if let Some((i, missing)) = difference(copies, placed) {
            let what = if missing {
                "a copy allocation with no place"
            } else {
                "a place for no copy allocation"
            };
            return fail(r, what, [Row::Entry("copies", i)]);
        }
        if let Some(at) = unsorted(&p.defined, |x| x.symbol) {
            return fail(
                r,
                "defined not sorted or two for one symbol",
                [Row::Entry("defined", at)],
            );
        }
        let internal =
            cx.resolution.def.iter().filter(|(_, d)| matches!(d, Def::Internal)).map(|(s, _)| s);
        if let Some((s, missing)) = difference(internal, p.defined.iter().map(|x| x.symbol)) {
            let what = if missing {
                "a linker-defined symbol with no value"
            } else {
                "a value for a symbol the linker does not define"
            };
            return fail(r, what, [Row::Symbol(s.into())]);
        }
        for (at, row) in p.defined.iter().enumerate() {
            if let DefValue::At { chunk, offset } = row.value {
                need(
                    offset <= p.chunks[chunk].size,
                    r,
                    || "a linker-defined symbol past its chunk's end".into(),
                    [Row::Entry("defined", at as u32), Row::Chunk(chunk)],
                )?;
            }
        }
        if let Some(at) = unsorted(&p.sized, |x| x.chunk) {
            return fail(r, "sized not sorted or two for one chunk", [Row::Entry("sized", at)]);
        }
        let late = p.chunks.iter().filter(|(_, c)| needs_sized(c)).map(|(id, _)| id);
        if let Some((c, missing)) = difference(late, p.sized.iter().map(|x| x.chunk)) {
            let what = if missing {
                "an address-dependent chunk with no sized row"
            } else {
                "a sized row for a chunk that does not need one"
            };
            return fail(r, what, [Row::Chunk(c)]);
        }
        for row in &p.sized {
            need(
                p.chunks[row.chunk].size == row.size,
                r,
                || "the chunk's size is not its sized row's".into(),
                [Row::Chunk(row.chunk)],
            )?;
        }
        let mut alloc_seen_after_compressed = None;
        for &id in p.order.iter().rev() {
            let c = &p.chunks[id];
            if c.is_alloc() {
                alloc_seen_after_compressed = Some(id);
            } else if c.flags & SHF_COMPRESSED != 0
                && let Some(alloc) = alloc_seen_after_compressed
            {
                return fail(
                    r,
                    "a compressed chunk before an alloc chunk",
                    [Row::Chunk(id), Row::Chunk(alloc)],
                );
            }
        }
        need(
            p.entry.is_some() == cx.needs_entry,
            r,
            || {
                if cx.needs_entry {
                    "the output needs an entry and has none".into()
                } else {
                    "an entry the output does not need".into()
                }
            },
            [],
        )
    }
}

/// Whether the chunk's size depends on addresses (§4.4 `sized`).
fn needs_sized(c: &crate::layout::Chunk) -> bool {
    matches!(c.kind, ChunkKind::Patches(_))
        || matches!(c.kind, ChunkKind::Synthetic(s) if s.address_dependent)
        || (!c.is_alloc() && c.flags & SHF_COMPRESSED != 0)
}

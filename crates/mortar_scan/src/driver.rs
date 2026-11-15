//! The scan units and the walk of one unit (`spec/image.md` §4.1 to §4.3).

use std::sync::atomic::{AtomicU16, Ordering};

use mortar_arch::seq::overlaps;
use mortar_arch::{
    DemandOut, DemandSink, DynRole, Howto, PlaceFacts, ProofRun, RefName, RelocAt, ScanCx,
    ScanError, Site, UnitWindow,
};
use mortar_core::demands::{DemandFlags, DynCounts, DynKind, GlobalFlags};
use mortar_core::par::SCAN_BLOCK;
use mortar_core::resolution::SectionKind;
use mortar_core::{FileId, LocalSym, SectionId, SymEntry, SymKey, SymKind};
use mortar_diag::{Diagnostic, FileRef, Key, Kind, Place, RefusalClass};
use mortar_elf::consts::{SHF_ALLOC, SHF_EXECINSTR, SHF_WRITE, STT_SECTION};
use mortar_elf::{SectionIndex, SymbolIndex};

use crate::events::{Acc, Event, EventClass, GroupKey, RefId, UnitEvents, sym_group};
use crate::view::{Raw, RecTable, RecTables, Recs, View, rec_table};

/// One scan unit: records `lo..hi` of the relocation section `rel` of
/// section `index` of `file` (a whole section, or one block of it).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Unit {
    pub file: FileId,
    pub section: SectionId,
    pub index: u32,
    pub rel: u32,
    pub lo: u32,
    pub hi: u32,
    /// The number of blocks of the section.
    pub parts: u32,
}

fn key_at(file: FileId, index: u32, record: u32) -> Key {
    mortar_arch::cx::scan_key(file, index, record)
}

fn malformed(what: String) -> Diagnostic {
    Diagnostic::error(Kind::Malformed { what })
}

/// The relocated sections of the link, as units in `(FileId, section
/// index)` order, a section of more than `SCAN_BLOCK` records cut into
/// blocks that never split a sequence (§4.1 step 1). A relocation section
/// that cannot be read is reported here and has no unit.
pub(crate) fn units(view: &View<'_, '_>, diags: &mut Vec<(Key, Diagnostic)>) -> Vec<Unit> {
    let mut out = Vec::new();
    let r = view.resolution;
    for (file, range) in r.section_base.iter() {
        for (k, sid) in range.iter().enumerate() {
            let row = r.sections[sid];
            if row.reloc.0 == 0 || !view.liveness.is_live(sid) {
                continue;
            }
            let index = k as u32;
            let sec = match Sec::open(view, file, sid, index, row.reloc.0) {
                Ok(s) => s,
                Err(d) => {
                    diags.push(d);
                    continue;
                }
            };
            let n = sec.recs.records.len();
            let Ok(n) = u32::try_from(n) else {
                let d = Diagnostic::refused(
                    RefusalClass::Feature,
                    "a relocation section of more than 2^32 - 1 records",
                );
                diags.push((
                    key_at(file, index, 0),
                    d.at(Place::section(sec.path(), sec.name_str(), 0)),
                ));
                continue;
            };
            if n == 0 {
                continue;
            }
            let block = SCAN_BLOCK as u32;
            let mut cuts = vec![0u32];
            let mut cut = block;
            while cut < n {
                let mut c = cut;
                while c < n && continues(view, &sec, c) {
                    c += 1;
                }
                if c >= n {
                    break;
                }
                cuts.push(c);
                cut = c.saturating_add(block);
            }
            let parts = cuts.len() as u32;
            for (part, &lo) in cuts.iter().enumerate() {
                let hi = cuts.get(part + 1).copied().unwrap_or(n);
                out.push(Unit { file, section: sid, index, rel: row.reloc.0, lo, hi, parts });
            }
        }
    }
    out
}

/// Whether record `i` belongs to a sequence whose lead is an earlier
/// record: a block may not start there (§4.1 step 1). Today through
/// `unit_window`'s unit; `spec/image.md` R-3 replaces this with
/// `sequence_extent`.
fn continues<'a, 's>(view: &View<'a, 's>, sec: &Sec<'_, 'a, 's>, i: u32) -> bool {
    let Some(raw) = sec.recs.records.raw(i) else { return false };
    let Some(key) = view.key_of(sec.file, sec.recs.table, raw.r_sym) else { return false };
    let site = Site { bytes: sec.recs.bytes, offset: raw.r_offset };
    let Ok(h) = view.arch.classify(raw.r_type, site) else { return false };
    let at = sec.at(i, &raw, key, &h);
    let names = RecTables {
        view,
        key,
        mention: SymEntry { file: sec.file, index: SymbolIndex(raw.r_sym) },
    };
    view.arch.unit_window(view.opts.output, &at, &names).is_some_and(|w| w.unit < i)
}

/// A relocated section opened for the scan.
pub(crate) struct Sec<'v, 'a, 's> {
    pub file: FileId,
    pub sid: SectionId,
    pub index: u32,
    pub recs: Recs<'v, 'a, 's>,
    pub place: PlaceFacts,
    pub kind: SectionKind,
    pub name: &'s [u8],
    pub proof: Vec<ProofRun>,
    view: &'v View<'a, 's>,
}

impl<'v, 'a, 's> Sec<'v, 'a, 's> {
    #[allow(
        clippy::result_large_err,
        reason = "the error is the diagnostic, made once per section"
    )]
    pub fn open(
        view: &'v View<'a, 's>,
        file: FileId,
        sid: SectionId,
        index: u32,
        rel: u32,
    ) -> Result<Self, (Key, Diagnostic)> {
        let fref = || view.inputs.files[file].display();
        let at = |what: String| {
            let d = malformed(what).at(Place::section(fref(), view.section_name(file, rel), 0));
            (key_at(file, index, 0), d)
        };
        let prep = view.prep(file).ok_or_else(|| at("not an object".into()))?;
        let obj = prep.obj.ok_or_else(|| at("not an object".into()))?;
        let table = match &prep.symtab {
            Ok(t) => t.as_ref(),
            Err(e) => return Err(at(e.clone())),
        };
        let (records, link, info) = rec_table(&obj, SectionIndex(rel)).map_err(at)?;
        if info.0 != index {
            return Err(at(format!(
                "relocation section {rel} applies to section {info}, not to section {index}"
            )));
        }
        if let Some(t) = table
            && link != t.section()
            && !records_all_null(&records)
        {
            return Err(at(format!(
                "relocation section {rel} uses section {link} as its symbol table, not {}",
                t.section()
            )));
        }
        let s = obj.section(SectionIndex(index)).map_err(|e| at(e.to_string()))?;
        let flags = s.flags();
        let bytes = s.data().unwrap_or_default();
        let name = s.name().unwrap_or_default();
        let place = PlaceFacts { alloc: flags & SHF_ALLOC != 0, writable: flags & SHF_WRITE != 0 };
        let exec = flags & SHF_EXECINSTR != 0;
        let mut proof = Vec::new();
        if exec {
            let mut marks = view.marks(file, index);
            view.opts.proof.runs(&mut marks, bytes.len() as u64, exec, &mut proof);
        }
        let kind = view.resolution.sections[sid].kind;
        Ok(Self {
            file,
            sid,
            index,
            recs: Recs { view, file, table, records, bytes },
            place,
            kind,
            name,
            proof,
            view,
        })
    }

    pub fn path(&self) -> FileRef {
        self.view.inputs.files[self.file].display()
    }

    pub fn name_str(&self) -> String {
        String::from_utf8_lossy(self.name).into_owned()
    }

    pub fn at<'r>(&'r self, i: u32, raw: &Raw, key: SymKey, h: &'r Howto) -> RelocAt<'r> {
        RelocAt {
            file: self.file,
            section: self.sid,
            index: i,
            r_offset: raw.r_offset,
            key,
            r_type: raw.r_type,
            addend: self.recs.addend(raw, h),
            howto: h,
            site: Site { bytes: self.recs.bytes, offset: raw.r_offset },
            records: &self.recs,
            proof: &self.proof,
        }
    }

    fn r_type_name(&self, r_type: u32) -> RefName {
        let m = self.view.inputs.target.machine.e_machine();
        mortar_elf::reloc::name(m, r_type).map_or(RefName::Hex(u64::from(r_type)), RefName::Named)
    }

    /// The diagnostic of a fault of record `i` (§4.3), its place and key.
    pub fn fault(&self, i: u32, fault: Fault) -> (Key, Diagnostic) {
        let raw = self.recs.records.raw(i);
        let off = raw.map_or(0, |r| r.r_offset);
        let path = self.path();
        let section = self.name_str();
        let place = || Place::section(path.clone(), section.clone(), off);
        match fault {
            Fault::Scan(e) => {
                let raw = raw.unwrap_or(Raw { r_offset: 0, r_sym: 0, r_type: 0, addend: Some(0) });
                let key = self.view.key_of(self.file, self.recs.table, raw.r_sym);
                let site = Site { bytes: self.recs.bytes, offset: raw.r_offset };
                let h = self.view.arch.classify(raw.r_type, site).ok();
                let r_type =
                    h.map_or_else(|| self.r_type_name(raw.r_type), |h| RefName::Named(h.name));
                let addend = h.map_or(raw.addend.unwrap_or(0), |h| self.recs.addend(&raw, &h));
                let target = key.map(|k| target_name(self.view, k, addend)).unwrap_or_default();
                let at = mortar_arch::diag::RelocPlace {
                    file: self.file,
                    path: &path,
                    section_index: self.index,
                    section: &section,
                    record: i,
                    r_offset: off,
                };
                mortar_arch::diag::scan(e, &at, r_type, &target)
            }
            Fault::BadSymbol { r_sym, count } => {
                let what = format!(
                    "relocation {i} of section {section} names symbol {r_sym}, but the symbol table has {count} entries"
                );
                (key_at(self.file, self.index, i), malformed(what).at(place()))
            }
            Fault::Unreadable => {
                let what = format!("relocation {i} of section {section} cannot be read");
                (key_at(self.file, self.index, i), malformed(what).at(place()))
            }
        }
    }
}

/// A relocation section all of whose records name symbol 0 needs no
/// symbol table (`sh_link` 0 is then accepted).
fn records_all_null(r: &RecTable<'_>) -> bool {
    (0..r.len() as u32).all(|i| r.raw(i).is_some_and(|x| x.r_sym == 0))
}

/// The target as a diagnostic names it (`mortar_arch::diag::scan`): the
/// symbol's name, or `section+0xoff` for a section symbol.
pub(crate) fn target_name(view: &View<'_, '_>, key: SymKey, addend: i64) -> String {
    let name = view.shown_name(key);
    if let SymKind::Local(e) = key.kind()
        && view.sym(e.file, e.index).is_some_and(|s| s.sym_type() == STT_SECTION)
    {
        return if addend < 0 {
            format!("{name}-{:#x}", addend.unsigned_abs())
        } else {
            format!("{name}+{addend:#x}")
        };
    }
    name
}

/// What can be wrong with one record.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Fault {
    /// `arch.scan`'s, or `classify`'s refusal.
    Scan(ScanError),
    /// The symbol index is outside the symbol table.
    BadSymbol { r_sym: u32, count: u32 },
    /// The record cannot be decoded.
    Unreadable,
}

/// What one unit hands the merge (I-15, I-17, I-20, I-29).
#[derive(Debug, Default)]
pub(crate) struct UnitOut {
    pub locals: Vec<(LocalSym, DemandFlags)>,
    pub output: GlobalFlags,
    pub counts: DynCounts,
    pub diags: Vec<(Key, Diagnostic)>,
    /// The records with a fault, overlaps aside, in record order; kept
    /// only for a section of several blocks (the overlap re-walk).
    pub faulted: Vec<u32>,
    /// The records whose window overlaps another unit's (§3.4).
    pub overlaps: Vec<u32>,
    pub events: Vec<(GroupKey, Acc)>,
    pub n_events: u64,
    /// I-29: whether `r_offset` never decreases, and the first and last.
    pub sorted: bool,
    pub first: u64,
    pub last: u64,
    /// The extent of the unit's non-empty windows.
    pub extent: Option<(u64, u64)>,
    pub records: u64,
}

/// The sink one record's `arch.scan` writes (`spec/arch.md` §3.4).
struct Sink<'u> {
    global: &'u [AtomicU16],
    locals: Vec<(LocalSym, DemandFlags)>,
    output: GlobalFlags,
    counts: [u32; 4],
    errors: Vec<(u32, ScanError)>,
    undefined: Vec<(SymKey, u32)>,
    record: u32,
    errored: bool,
    /// The reference's target is in a section that is not live: its value
    /// is a tombstone or 0 (§6.1), so it demands nothing.
    mute: bool,
}

impl DemandOut for Sink<'_> {
    fn raise(&mut self, key: SymKey, flags: DemandFlags) {
        if self.mute || flags.is_empty() {
            return;
        }
        match key.kind() {
            SymKind::Global(id) => {
                // I-16: read before or-ing, so the line stays shared.
                if let Some(cell) = self.global.get(id.0 as usize) {
                    let bits = flags.bits();
                    if cell.load(Ordering::Relaxed) & bits != bits {
                        cell.fetch_or(bits, Ordering::Relaxed);
                    }
                }
            }
            SymKind::Local(sym) => match self.locals.last_mut() {
                Some(last) if last.0 == sym => last.1 |= flags,
                _ => self.locals.push((sym, flags)),
            },
        }
    }

    fn raise_global(&mut self, flags: GlobalFlags) {
        if !self.mute {
            self.output |= flags;
        }
    }

    fn count(&mut self, role: DynRole, n: u32) {
        if self.mute {
            return;
        }
        let k = dyn_kind(role) as usize;
        self.counts[k] = self.counts[k].saturating_add(n);
    }

    fn error(&mut self, record: u32, error: ScanError) {
        if record == self.record {
            self.errored = true;
        }
        self.errors.push((record, error));
    }

    fn undefined(&mut self, key: SymKey, record: u32) {
        self.undefined.push((key, record));
    }
}

/// The kind a dynamic relocation role counts as (`Demands.dyn_relocs`).
pub(crate) const fn dyn_kind(role: DynRole) -> DynKind {
    match role {
        DynRole::Relative => DynKind::Relative,
        DynRole::Symbolic | DynRole::GlobDat | DynRole::JumpSlot | DynRole::Copy => {
            DynKind::Symbolic
        }
        DynRole::IRelative => DynKind::IRelative,
        DynRole::DtpMod | DynRole::DtpOff | DynRole::TpOff { .. } | DynRole::TlsDesc => {
            DynKind::Tls
        }
    }
}

/// The event class of an `undefined` report (§4.4, I-18): a key only
/// dropped COMDAT copies define is `ComdatMismatch`, any other
/// `Undefined`. This is where `spec/image.md` R-1's `reference(class,
/// group)` will arrive instead.
fn on_undefined(view: &View<'_, '_>, key: SymKey) -> EventClass {
    match key.kind() {
        SymKind::Global(id) if view.is_mismatch(id) => EventClass::ComdatMismatch,
        _ => EventClass::Undefined,
    }
}

/// GNU ld's debug section names (`spec/resolve.md` §6.3).
pub fn is_debug_name(name: &[u8]) -> bool {
    const PREFIXES: [&[u8]; 6] =
        [b".debug", b".zdebug", b".line", b".stab", b".gnu.linkonce.wi.", b".gnu.debuglto_.debug_"];
    name == b".gnu_debuglink" || PREFIXES.iter().any(|p| name.starts_with(p))
}

/// Whether a reference from section `sec` to a symbol of a section that
/// is not live is `discarded-section-reference` (`spec/resolve.md` §6.3
/// case (a), I-12): not from a non-alloc section with a debug name (the
/// tombstone), `.eh_frame` (the FDE is dead) or exactly
/// `.gcc_except_table` (0).
pub fn discarded_is_error(kind: SectionKind, name: &[u8], alloc: bool) -> bool {
    !(kind == SectionKind::EhFrame
        || name == b".eh_frame"
        || name == b".gcc_except_table"
        || (!alloc && is_debug_name(name)))
}

/// Walks one unit (§4.1 step 2): every record in record order.
#[allow(clippy::too_many_lines, reason = "the per-record steps of §4.1, in order")]
pub(crate) fn run_unit(view: &View<'_, '_>, u: &Unit, global: &[AtomicU16]) -> UnitOut {
    let mut out = UnitOut { sorted: true, ..UnitOut::default() };
    let sec = match Sec::open(view, u.file, u.section, u.index, u.rel) {
        Ok(s) => s,
        Err(d) => {
            // Opened once already when the units were made: unreachable
            // for a resident store, reported rather than assumed.
            out.diags.push(d);
            return out;
        }
    };
    let mut sink = Sink {
        global,
        locals: Vec::new(),
        output: GlobalFlags::empty(),
        counts: [0; 4],
        errors: Vec::new(),
        undefined: Vec::new(),
        record: 0,
        errored: false,
        mute: false,
    };
    let mut events = UnitEvents::default();
    let mut windows: Vec<UnitWindow> = Vec::new();
    let nsyms = sec.recs.table.map_or(0, mortar_elf::SymbolTable::len);
    let output = view.opts.output;
    let refid = |record| RefId { file: u.file, section: u.index, record };
    let mut faults: Vec<(u32, Fault)> = Vec::new();
    let mut prev: Option<u64> = None;
    for i in u.lo..u.hi {
        out.records += 1;
        let Some(raw) = sec.recs.records.raw(i) else {
            faults.push((i, Fault::Unreadable));
            continue;
        };
        if let Some(p) = prev {
            out.sorted &= p <= raw.r_offset;
        } else {
            out.first = raw.r_offset;
        }
        prev = Some(raw.r_offset);
        out.last = raw.r_offset;
        let Some(key) = view.key_of(u.file, sec.recs.table, raw.r_sym) else {
            faults.push((i, Fault::BadSymbol { r_sym: raw.r_sym, count: nsyms }));
            continue;
        };
        let site = Site { bytes: sec.recs.bytes, offset: raw.r_offset };
        let h = match view.arch.classify(raw.r_type, site) {
            Ok(h) => h,
            Err(r) => {
                faults.push((i, Fault::Scan(ScanError::Refused(r))));
                continue;
            }
        };
        let at = sec.at(i, &raw, key, &h);
        let tables = RecTables {
            view,
            key,
            mention: SymEntry { file: u.file, index: SymbolIndex(raw.r_sym) },
        };
        // §6.1: a local of a section that is not live.
        let dead = match key.kind() {
            SymKind::Local(e) if e.index.0 != 0 => {
                view.local_section(e).is_some_and(|s| !view.is_live(u.file, s))
            }
            _ => false,
        };
        sink.record = i;
        sink.errored = false;
        sink.mute = dead;
        let before = sink.errors.len();
        let cx = ScanCx { tables: &tables, output, place: sec.place };
        view.arch.scan(&cx, &at, &mut DemandSink::new(&mut sink));
        for (record, e) in sink.errors.drain(before..) {
            faults.push((record, Fault::Scan(e)));
        }
        for (k, record) in sink.undefined.drain(..) {
            let class = on_undefined(view, k);
            events.push(Event { class, group: sym_group(k), at: refid(record) });
        }
        let consumed = || view.arch.consumed(output, &at, &tables);
        if dead && discarded_is_error(sec.kind, sec.name, sec.place.alloc) && !consumed() {
            let group = u64::from(u.index) << 32 | u64::from(raw.r_sym);
            events.push(Event { class: EventClass::DiscardedSection, group, at: refid(i) });
        }
        if let SymKind::Global(id) = key.kind()
            && view.is_warned(id)
            && !consumed()
        {
            events.push(Event {
                class: EventClass::GnuWarning,
                group: sym_group(key),
                at: refid(i),
            });
        }
        if !sink.errored
            && let Some(w) = view.arch.unit_window(output, &at, &tables)
        {
            windows.push(w);
        }
    }
    let mut pairs = Vec::new();
    for w in &windows {
        if w.start < w.end {
            let e = out.extent.get_or_insert((w.start, w.end));
            *e = (e.0.min(w.start), e.1.max(w.end));
        }
    }
    overlaps(&mut windows, &mut pairs);
    out.overlaps = pairs.iter().map(|p| p.0).collect();
    out.overlaps.sort_unstable();
    out.overlaps.dedup();
    faults.sort_by_key(|f| f.0);
    if u.parts > 1 {
        // Decided at the merge, with the other blocks (`rewalk`).
        out.faulted = faults.iter().map(|f| f.0).collect();
        out.faulted.dedup();
    } else {
        faults.extend(out.overlaps.drain(..).map(|r| (r, Fault::Scan(ScanError::Overlap))));
        faults.sort_by_key(|f| f.0);
    }
    out.diags = faults.iter().map(|&(i, f)| sec.fault(i, f)).collect();
    out.n_events = events.events();
    out.events = events.finish();
    out.locals = sink.locals;
    out.locals.sort_by_key(|l| l.0);
    let mut merged: Vec<(LocalSym, DemandFlags)> = Vec::with_capacity(out.locals.len());
    for (s, f) in out.locals.drain(..) {
        match merged.last_mut() {
            Some(last) if last.0 == s => last.1 |= f,
            _ => merged.push((s, f)),
        }
    }
    out.locals = merged;
    out.output = sink.output;
    out.counts = DynCounts(sink.counts);
    out
}

/// The overlapping records of a section of several blocks whose blocks'
/// extents intersect (§3.4 across blocks): the windows of every record
/// of `blocks` without another fault, decided together. Each block's own
/// decision is exact for it when its extent meets no other block's, since
/// windows of disjoint extents cannot overlap.
pub(crate) fn rewalk(view: &View<'_, '_>, blocks: &[(&Unit, &UnitOut)]) -> Vec<u32> {
    let Some((u, _)) = blocks.first() else { return Vec::new() };
    let Ok(sec) = Sec::open(view, u.file, u.section, u.index, u.rel) else { return Vec::new() };
    let mut windows = Vec::new();
    for (b, o) in blocks {
        for i in b.lo..b.hi {
            if o.faulted.binary_search(&i).is_ok() {
                continue;
            }
            let Some(raw) = sec.recs.records.raw(i) else { continue };
            let Some(key) = view.key_of(b.file, sec.recs.table, raw.r_sym) else { continue };
            let site = Site { bytes: sec.recs.bytes, offset: raw.r_offset };
            let Ok(h) = view.arch.classify(raw.r_type, site) else { continue };
            let at = sec.at(i, &raw, key, &h);
            let tables = RecTables {
                view,
                key,
                mention: SymEntry { file: b.file, index: SymbolIndex(raw.r_sym) },
            };
            if let Some(w) = view.arch.unit_window(view.opts.output, &at, &tables) {
                windows.push(w);
            }
        }
    }
    let mut pairs = Vec::new();
    overlaps(&mut windows, &mut pairs);
    let mut v: Vec<u32> = pairs.iter().map(|p| p.0).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// The record indexes of relocation section `u.rel` stably sorted by
/// `r_offset` (I-29 `Permuted`).
pub(crate) fn permutation(view: &View<'_, '_>, u: &Unit) -> Box<[u32]> {
    let Some(obj) = view.prep(u.file).and_then(|p| p.obj) else { return Box::new([]) };
    let Ok((t, _, _)) = rec_table(&obj, SectionIndex(u.rel)) else { return Box::new([]) };
    let n = t.len() as u32;
    let mut v: Vec<(u64, u32)> = (0..n).map(|i| (t.raw(i).map_or(0, |r| r.r_offset), i)).collect();
    v.sort_by_key(|e| e.0);
    v.into_iter().map(|e| e.1).collect()
}

/// The diagnostic of an overlap found at the merge.
pub(crate) fn overlap_diag(
    view: &View<'_, '_>,
    u: &Unit,
    record: u32,
) -> Option<(Key, Diagnostic)> {
    let sec = Sec::open(view, u.file, u.section, u.index, u.rel).ok()?;
    Some(sec.fault(record, Fault::Scan(ScanError::Overlap)))
}

//! The scan units and the walk of one unit (`spec/image.md` §4.1 to §4.3).

use std::sync::atomic::{AtomicU16, Ordering};

use mortar_arch::seq::{disjoint_in_order, overlaps};
use mortar_arch::{
    DemandOut, DemandSink, DynRefusal, DynRole, Howto, Incompatible, PlaceFacts, ProofRun, RefName,
    RelocAt, ScanCx, ScanError, ScanTables, Site, UnitWindow,
};
use mortar_core::demands::{DemandFlags, DynCounts, DynKind, GlobalFlags};
use mortar_core::par::SCAN_BLOCK;
use mortar_core::resolution::SectionKind;
use mortar_core::{FileId, LocalSym, SectionId, SymEntry, SymKey, SymKind, SymbolId};
use mortar_diag::{Diagnostic, FileRef, Key, Kind, Place, RefusalClass};
use mortar_elf::consts::{SHF_ALLOC, SHF_EXECINSTR, SHF_WRITE, SHT_RELA, STT_SECTION};
use mortar_elf::{SectionIndex, SymbolIndex};

use crate::events::{Acc, Event, EventClass, GroupKey, RefId, UnitEvents, sym_group};
use crate::view::{
    Bufs, Raw, ReadFail, RecTable, RecTables, Recs, View, contents, rec_table, with_bufs,
};

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
    /// A section whose checks are fill's (I-14): only its records of the
    /// types of `Arch::nonalloc_demand_types` are walked, for their
    /// demands.
    pub types_only: bool,
}

pub(crate) fn key_at(file: FileId, index: u32, record: u32) -> Key {
    mortar_arch::cx::scan_key(file, index, record)
}

fn malformed(what: String) -> Diagnostic {
    Diagnostic::error(Kind::Malformed { what })
}

/// The relocated sections of the link, as units in `(FileId, section
/// index)` order, a section of more than `SCAN_BLOCK` records cut into
/// blocks that never split a sequence (§4.1 step 1). A relocation section
/// that cannot be read is reported here and has no unit.
///
/// The sections whose checks are fill's (I-14, [`Defer`]) are returned
/// apart, in the same order; they have no unit, or a `types_only` one
/// when the architecture has non-alloc demand types.
pub(crate) fn units(
    view: &View<'_, '_>,
    diags: &mut Vec<(Key, Diagnostic)>,
) -> (Vec<Unit>, Vec<SectionId>) {
    let mut out = Vec::new();
    let mut deferred = Vec::new();
    let mut bufs = Bufs::default();
    let mut defer = Defer::new(view);
    let types = !view.arch.nonalloc_demand_types().is_empty();
    let r = view.resolution;
    for (file, range) in r.section_base.iter() {
        for (k, sid) in range.iter().enumerate() {
            let row = r.sections[sid];
            if row.reloc.0 == 0 || !view.liveness.is_live(sid) {
                continue;
            }
            let index = k as u32;
            if let Some(n) = defer.count(file, index, row.reloc.0, row.kind) {
                if n > 0 {
                    deferred.push(sid);
                    if types {
                        out.push(Unit {
                            file,
                            section: sid,
                            index,
                            rel: row.reloc.0,
                            lo: 0,
                            hi: n,
                            parts: 1,
                            types_only: true,
                        });
                    }
                }
                continue;
            }
            if let Some(n) = header_count(view, file, row.reloc.0) {
                // One unit, opened and checked by its walk: the scan
                // reads the records once (`spec/image.md` I-81), and the
                // list of units is made from section headers alone.
                out.push(Unit {
                    file,
                    section: sid,
                    index,
                    rel: row.reloc.0,
                    lo: 0,
                    hi: n,
                    parts: 1,
                    types_only: false,
                });
                continue;
            }
            let sec = match Sec::open(view, file, sid, index, row.reloc.0, &mut bufs) {
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
                let c = cut_at(view, &sec, cut, n);
                if c >= n {
                    break;
                }
                cuts.push(c);
                cut = c.saturating_add(block);
            }
            let parts = cuts.len() as u32;
            for (part, &lo) in cuts.iter().enumerate() {
                let hi = cuts.get(part + 1).copied().unwrap_or(n);
                out.push(Unit {
                    file,
                    section: sid,
                    index,
                    rel: row.reloc.0,
                    lo,
                    hi,
                    parts,
                    types_only: false,
                });
            }
        }
    }
    (out, deferred)
}

/// Which relocated sections the scan leaves to fill (`spec/image.md`
/// I-14): a section not `SHF_ALLOC`, of a debug name (a dead target is
/// the tombstone there, `mortar_arch::dead_reference`), whose records
/// can therefore make no event unless their file mentions a key of
/// [`View::event_keys`]; and whose relocation section's header is one
/// the walk would accept (`SHT_RELA`, its record size, whole records,
/// `sh_info` the section, `sh_link` the file's symbol table). Such a
/// section's records can only be faults (§4.3), which fill finds as it
/// applies them; the diagnostics are then the walk's
/// ([`crate::Walker::deferred_report`]).
struct Defer<'v, 'a, 's> {
    view: &'v View<'a, 's>,
    /// [`View::event_keys`]: empty in a link without undefined,
    /// mismatched or warned keys, the common case, when no file's
    /// mentions need be looked at.
    events: Vec<SymbolId>,
    /// Per file: 0 not yet known, 1 mentions none of `events`, 2 does.
    mentions: Vec<u8>,
}

impl<'v, 'a, 's> Defer<'v, 'a, 's> {
    fn new(view: &'v View<'a, 's>) -> Self {
        Self { view, events: view.event_keys(), mentions: Vec::new() }
    }

    /// Whether `file` mentions a key of `events`: one of its globals, or
    /// what `redirect` makes of one (`View::key_of`).
    fn mentions(&mut self, file: FileId) -> bool {
        if self.events.is_empty() {
            return false;
        }
        let at = file.0 as usize;
        if self.mentions.len() <= at {
            self.mentions.resize(at + 1, 0);
        }
        if self.mentions[at] == 0 {
            let r = self.view.resolution;
            let hit = |id: SymbolId| self.events.binary_search(&id).is_ok();
            let any =
                self.view.inputs.sym_map_of(file).iter().any(|&id| {
                    hit(id) || r.redirect.get(id).is_some_and(|&to| to != id && hit(to))
                });
            self.mentions[at] = if any { 2 } else { 1 };
        }
        self.mentions[at] == 2
    }

    /// The record count of relocation section `rel` of section `index`
    /// of `file` when the section is one fill checks, from the headers
    /// alone; `None` when the scan walks it.
    fn count(&mut self, file: FileId, index: u32, rel: u32, kind: SectionKind) -> Option<u32> {
        if !matches!(kind, SectionKind::Regular | SectionKind::Debug) {
            return None;
        }
        let view = self.view;
        let prep = view.prep(file)?;
        let obj = prep.obj?;
        let table = prep.table()?;
        let s = obj.section(SectionIndex(index)).ok()?;
        if s.flags() & SHF_ALLOC != 0 {
            return None;
        }
        let name = s.name().ok()?;
        if mortar_arch::dead_reference(name, false) != mortar_arch::DeadRef::Tombstone {
            return None;
        }
        let rs = obj.section(SectionIndex(rel)).ok()?;
        let h = rs.header();
        let size = match obj.class() {
            mortar_elf::Class::Elf64 => 24,
            mortar_elf::Class::Elf32 => 12,
        };
        if rs.sh_type() != SHT_RELA
            || h.sh_entsize != size
            || h.sh_size % size != 0
            || h.sh_info != index
            || h.sh_link != table.section().0
        {
            return None;
        }
        let n = u32::try_from(h.sh_size / size).ok()?;
        if self.mentions(file) {
            return None;
        }
        Some(n)
    }
}

/// The record count of relocation section `rel` of `file` from its
/// header, when it has between 1 and `SCAN_BLOCK` records: such a section
/// is a single unit whatever its records say, so [`units`] need not open
/// it; its walk does, and reports what [`Sec::open`] finds wrong at the
/// same key. `sh_size / sh_entsize` is the record count whenever the
/// records can be read (`mortar_elf` checks `sh_entsize` against the
/// record size and `sh_size` against a multiple of it). `None`: [`units`]
/// opens it (none, or more than one block, of records; an unreadable
/// header).
fn header_count(view: &View<'_, '_>, file: FileId, rel: u32) -> Option<u32> {
    let mortar_core::inputs::Reader::Object(obj) = view.inputs.files.get(file)?.reader else {
        return None;
    };
    let s = obj.section(SectionIndex(rel)).ok()?;
    let h = s.header();
    if h.sh_entsize == 0 {
        return None;
    }
    let n = h.sh_size / h.sh_entsize;
    u32::try_from(n).ok().filter(|&n| n > 0 && n as usize <= SCAN_BLOCK)
}

/// The records before a cut that may still lead a sequence reaching
/// past it: `spec/arch.md` §3.5 bounds a sequence's look-back by the
/// architecture (AArch64 2, x86 1), so an extent is at most 3 records.
const EXTENT_BACK: u32 = 16;

/// The first block boundary at or after `c` that no sequence crosses
/// (§4.1 step 1): every record `j` before the boundary, from
/// `EXTENT_BACK` records back, ends its `sequence_extent` (R-3) at or
/// before it. A record that leads nothing has extent 1, so checking
/// every record, not only the leads, is exact.
fn cut_at(view: &View<'_, '_>, sec: &Sec<'_, '_, '_, '_>, c: u32, n: u32) -> u32 {
    let mut c = c;
    let mut j = c.saturating_sub(EXTENT_BACK);
    while j < c && c < n {
        let end = j.saturating_add(view.arch.sequence_extent(&sec.recs, j).max(1));
        c = c.max(end);
        j += 1;
    }
    c
}

/// A relocated section opened for the scan.
pub(crate) struct Sec<'v, 'a, 's, 'b> {
    pub file: FileId,
    pub sid: SectionId,
    pub index: u32,
    pub recs: Recs<'v, 'a, 's, 'b>,
    pub place: PlaceFacts,
    pub name: &'s [u8],
    pub proof: Vec<ProofRun>,
    view: &'v View<'a, 's>,
}

impl<'v, 'a, 's: 'b, 'b> Sec<'v, 'a, 's, 'b> {
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
        bufs: &'b mut Bufs,
    ) -> Result<Self, (Key, Diagnostic)> {
        let fref = || view.inputs.files[file].display();
        let at = |what: String| {
            let d = malformed(what).at(Place::section(fref(), view.section_name(file, rel), 0));
            (key_at(file, index, 0), d)
        };
        // A read of bytes that are not resident failed (or bytes the
        // store keeps resident were not): never "no records".
        let fail = |f: ReadFail, sec: u32| {
            let place = Place::section(fref(), view.section_name(file, sec), 0);
            (key_at(file, index, 0), f.diagnostic(&fref()).at(place))
        };
        let Bufs { rel: rel_buf, sec: sec_buf } = bufs;
        let prep = view.prep(file).ok_or_else(|| at("not an object".into()))?;
        let obj = prep.obj.ok_or_else(|| at("not an object".into()))?;
        let table = match &prep.symtab {
            Ok(t) => t.as_ref(),
            Err(e) => return Err(at(e.clone())),
        };
        let (records, link, info) =
            rec_table(view, file, &obj, SectionIndex(rel), rel_buf).map_err(|f| fail(f, rel))?;
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
        let bytes = contents(view, file, &s, sec_buf).map_err(|f| fail(f, index))?;
        let name = s.name().unwrap_or_default();
        let exec = flags & SHF_EXECINSTR != 0;
        let place = PlaceFacts {
            alloc: flags & SHF_ALLOC != 0,
            writable: flags & SHF_WRITE != 0,
            exec: Some(exec),
        };
        let mut proof = Vec::new();
        if exec {
            let mut marks = view.marks(file, index);
            view.opts.proof.runs(&mut marks, bytes.len() as u64, exec, &mut proof);
        }
        Ok(Self {
            file,
            sid,
            index,
            recs: Recs { view, file, table, records, bytes },
            place,
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
            addend: raw.addend,
            howto: h,
            site: Site { bytes: self.recs.bytes, offset: raw.r_offset },
            records: &self.recs,
            proof: &self.proof,
            // `spec/arch.md` §3.5: scan passes `true` and records the bit.
            leads: true,
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
    /// `records` when the section is not `SHF_ALLOC` (I-14), else 0.
    pub nonalloc: u64,
    /// Some record's type is in `Facts.seq_leads` (`RelocAt.leads`).
    pub leads: bool,
    /// The branch candidates of a code section's unit, in record order.
    pub branches: Vec<crate::BranchRec>,
    /// The records that raised `COPY` or `CANON_PLT` on a global key, in
    /// record order (`spec/dyn.md` D-18, D-42: checked per key).
    pub copy_refs: Vec<(SymbolId, u32)>,
}

/// The sink one record's `arch.scan` writes (`spec/arch.md` §3.4).
struct Sink<'u> {
    global: &'u [AtomicU16],
    locals: Vec<(LocalSym, DemandFlags)>,
    output: GlobalFlags,
    counts: [u32; 4],
    errors: Vec<(u32, ScanError)>,
    refs: Vec<(u32, EventClass, SymKey)>,
    copy_refs: Vec<(SymbolId, u32)>,
    record: u32,
    errored: bool,
}

impl DemandOut for Sink<'_> {
    fn raise(&mut self, key: SymKey, flags: DemandFlags) {
        if flags.is_empty() {
            return;
        }
        match key.kind() {
            SymKind::Global(id) => {
                if flags.intersects(DemandFlags::COPY | DemandFlags::CANON_PLT)
                    && self.copy_refs.last().is_none_or(|l| *l != (id, self.record))
                {
                    self.copy_refs.push((id, self.record));
                }
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
        self.output |= flags;
    }

    fn count(&mut self, role: DynRole, n: u32) {
        let k = dyn_kind(role) as usize;
        self.counts[k] = self.counts[k].saturating_add(n);
    }

    fn error(&mut self, record: u32, error: ScanError) {
        if record == self.record {
            self.errored = true;
        }
        self.errors.push((record, error));
    }

    fn reference(&mut self, record: u32, class: EventClass, key: SymKey) {
        self.refs.push((record, class, key));
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

/// `spec/dyn.md` D-47: in an output with `.dynamic`, a record that
/// would be dynamic at a read-only place (arch.md §4.3 `TX`, the
/// `read-only` reason of `relocation-incompatible`) is `text-relocation`.
/// A static link (static PIE included) keeps the M1 diagnostic.
pub(crate) fn dynamic_error(view: &View<'_, '_>, e: ScanError) -> ScanError {
    match e {
        ScanError::Incompatible(Incompatible::ReadOnly) if view.opts.dynamic => {
            ScanError::Dynamic(DynRefusal::TextRelocation)
        }
        e => e,
    }
}

/// Whether `key`'s target kind is `Weak0`, from the definitions alone;
/// checked against the full facts in debug builds.
fn weak0(view: &View<'_, '_>, tables: &RecTables<'_, '_, '_>, key: SymKey) -> bool {
    let w = view.is_weak0(key);
    debug_assert_eq!(w, ScanTables::sym(tables, key).kind == mortar_arch::TargetKind::Weak0);
    #[cfg(not(debug_assertions))]
    let _ = tables;
    w
}

/// Walks one unit (§4.1 step 2): every record in record order.
pub(crate) fn run_unit(view: &View<'_, '_>, u: &Unit, global: &[AtomicU16]) -> UnitOut {
    with_bufs(|bufs| walk_unit(view, u, global, bufs))
}

/// The demands of the records of a `types_only` unit (I-14) whose type
/// is one of `Arch::nonalloc_demand_types`: `arch.scan` as the walk calls
/// it, its demands kept and nothing else (the checks are fill's). The
/// section is opened only when a record has such a type.
fn walk_types(view: &View<'_, '_>, u: &Unit, global: &[AtomicU16], bufs: &mut Bufs) -> UnitOut {
    let mut out = UnitOut { sorted: true, ..UnitOut::default() };
    let types = view.arch.nonalloc_demand_types();
    let Some(obj) = view.prep(u.file).and_then(|p| p.obj) else { return out };
    // A failed read is fill's to report, as every check of the section.
    let Ok((t, _, _)) = rec_table(view, u.file, &obj, SectionIndex(u.rel), &mut bufs.rel) else {
        return out;
    };
    let n = t.len() as u32;
    if !(0..n).any(|i| t.raw(i).is_some_and(|r| types.contains(&r.r_type))) {
        return out;
    }
    let Ok(sec) = Sec::open(view, u.file, u.section, u.index, u.rel, bufs) else { return out };
    let mut sink = Sink {
        global,
        locals: Vec::new(),
        output: GlobalFlags::empty(),
        counts: [0; 4],
        errors: Vec::new(),
        refs: Vec::new(),
        copy_refs: Vec::new(),
        record: 0,
        errored: false,
    };
    for i in u.lo..u.hi {
        let Some(raw) = sec.recs.records.raw(i) else { continue };
        if !types.contains(&raw.r_type) {
            continue;
        }
        out.records += 1;
        let Some(key) = view.key_of(u.file, sec.recs.table, raw.r_sym) else { continue };
        let site = Site { bytes: sec.recs.bytes, offset: raw.r_offset };
        let Ok(h) = view.arch.classify(raw.r_type, site) else { continue };
        let at = sec.at(i, &raw, key, &h);
        let tables = RecTables {
            view,
            key,
            mention: SymEntry { file: u.file, index: SymbolIndex(raw.r_sym) },
        };
        sink.record = i;
        let cx =
            ScanCx { tables: &tables, output: view.opts.output, place: sec.place, name: sec.name };
        view.arch.scan(&cx, &at, &mut DemandSink::new(&mut sink));
        sink.errors.clear();
        sink.refs.clear();
    }
    out.locals = sink.locals;
    out.locals.sort_by_key(|l| l.0);
    out.output = sink.output;
    out.counts = DynCounts(sink.counts);
    out.nonalloc = out.records;
    out
}

/// [`run_unit`] with the worker's buffers.
#[allow(clippy::too_many_lines, reason = "the per-record steps of §4.1, in order")]
fn walk_unit(view: &View<'_, '_>, u: &Unit, global: &[AtomicU16], bufs: &mut Bufs) -> UnitOut {
    if u.types_only {
        return walk_types(view, u, global, bufs);
    }
    let mut out = UnitOut { sorted: true, ..UnitOut::default() };
    let sec = match Sec::open(view, u.file, u.section, u.index, u.rel, bufs) {
        Ok(s) => s,
        Err(d) => {
            // A section of one block is first opened here
            // (`header_count`); one of several blocks was opened when the
            // units were made.
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
        refs: Vec::new(),
        copy_refs: Vec::new(),
        record: 0,
        errored: false,
    };
    let mut events = UnitEvents::default();
    let mut windows: Vec<UnitWindow> = Vec::with_capacity((u.hi - u.lo) as usize);
    let nsyms = sec.recs.table.map_or(0, mortar_elf::SymbolTable::len);
    let output = view.opts.output;
    let refid = |record| RefId { file: u.file, section: u.index, record };
    let mut faults: Vec<(u32, Fault)> = Vec::new();
    let mut prev: Option<u64> = None;
    let seq_leads = view.arch.facts().seq_leads;
    // The branch candidates (`crate::BranchRec`): only in live code.
    let code = sec.place.alloc && sec.place.exec == Some(true);
    for i in u.lo..u.hi {
        out.records += 1;
        let Some(raw) = sec.recs.records.raw(i) else {
            faults.push((i, Fault::Unreadable));
            continue;
        };
        out.leads |= seq_leads.contains(&raw.r_type);
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
        // `spec/dyn.md` D-25: the key's refusal (made once, by the weak
        // classes) precedes what the architecture says of the record.
        if let SymKind::Global(id) = key.kind()
            && view.weak_refused.contains(id)
        {
            continue;
        }
        let at = sec.at(i, &raw, key, &h);
        let tables = RecTables {
            view,
            key,
            mention: SymEntry { file: u.file, index: SymbolIndex(raw.r_sym) },
        };
        if code
            && let Some(b) = view.arch.branch(&h)
            && b.veneer
            && let Some(site) = mortar_arch::need::branch_candidate(
                view.arch,
                output,
                &at,
                &tables,
                weak0(view, &tables, key),
            )
        {
            out.branches.push(crate::BranchRec {
                section: u.section,
                offset: raw.r_offset,
                record: i,
                site,
            });
        }
        sink.record = i;
        sink.errored = false;
        let before = sink.errors.len();
        let cx = ScanCx { tables: &tables, output, place: sec.place, name: sec.name };
        view.arch.scan(&cx, &at, &mut DemandSink::new(&mut sink));
        for (record, e) in sink.errors.drain(before..) {
            faults.push((record, Fault::Scan(dynamic_error(view, e))));
        }
        for (record, class, k) in sink.refs.drain(..) {
            let group = if class == EventClass::DiscardedSection {
                // (referring section index, symbol index): the record's.
                let r_sym = sec.recs.records.raw(record).map_or(raw.r_sym, |r| r.r_sym);
                u64::from(u.index) << 32 | u64::from(r_sym)
            } else {
                sym_group(k)
            };
            events.push(Event { class, group, at: refid(record) });
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
    if !disjoint_in_order(&windows) {
        overlaps(&mut windows, &mut pairs);
    }
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
    if !sec.place.alloc {
        out.nonalloc = out.records;
    }
    out.output = sink.output;
    out.counts = DynCounts(sink.counts);
    out.copy_refs = sink.copy_refs;
    out
}

/// The overlapping records of a section of several blocks whose blocks'
/// extents intersect (§3.4 across blocks): the windows of every record
/// of `blocks` without another fault, decided together. Each block's own
/// decision is exact for it when its extent meets no other block's, since
/// windows of disjoint extents cannot overlap.
#[allow(clippy::result_large_err, reason = "the error is the diagnostic, made once per section")]
pub(crate) fn rewalk(
    view: &View<'_, '_>,
    blocks: &[(&Unit, &UnitOut)],
) -> Result<Vec<u32>, (Key, Diagnostic)> {
    let Some((u, _)) = blocks.first() else { return Ok(Vec::new()) };
    let mut bufs = Bufs::default();
    // Opened by every block's walk: a failure now is a file that changed.
    let sec = Sec::open(view, u.file, u.section, u.index, u.rel, &mut bufs)?;
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
    if !disjoint_in_order(&windows) {
        overlaps(&mut windows, &mut pairs);
    }
    let mut v: Vec<u32> = pairs.iter().map(|p| p.0).collect();
    v.sort_unstable();
    v.dedup();
    Ok(v)
}

/// The record indexes of relocation section `u.rel` stably sorted by
/// `r_offset` (I-29 `Permuted`).
/// A read that fails now (the section was walked, so the file changed)
/// is the read's diagnostic.
#[allow(clippy::result_large_err, reason = "the error is the diagnostic, made once per section")]
pub(crate) fn permutation(view: &View<'_, '_>, u: &Unit) -> Result<Box<[u32]>, (Key, Diagnostic)> {
    let Some(obj) = view.prep(u.file).and_then(|p| p.obj) else { return Ok(Box::new([])) };
    with_bufs(|bufs| {
        let (t, _, _) =
            rec_table(view, u.file, &obj, SectionIndex(u.rel), &mut bufs.rel).map_err(|f| {
                let path = view.inputs.files[u.file].display();
                let place = Place::section(path.clone(), view.section_name(u.file, u.rel), 0);
                (key_at(u.file, u.index, 0), f.diagnostic(&path).at(place))
            })?;
        let n = t.len() as u32;
        let mut v: Vec<(u64, u32)> =
            (0..n).map(|i| (t.raw(i).map_or(0, |r| r.r_offset), i)).collect();
        v.sort_by_key(|e| e.0);
        Ok(v.into_iter().map(|e| e.1).collect())
    })
}

/// The diagnostic of an overlap found at the merge.
pub(crate) fn overlap_diag(view: &View<'_, '_>, u: &Unit, record: u32) -> (Key, Diagnostic) {
    let mut bufs = Bufs::default();
    match Sec::open(view, u.file, u.section, u.index, u.rel, &mut bufs) {
        Ok(sec) => sec.fault(record, Fault::Scan(ScanError::Overlap)),
        // Opened by the walk: a failure now is a file that changed.
        Err(d) => d,
    }
}

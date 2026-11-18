//! The read-only views the trait's functions see (§3.4, §3.6, §3.7) and
//! the sink scan writes into.
//!
//! The tables behind them (`Inputs`, `Resolution`, `Demands`, `Slots`,
//! `Placement`) are reached through small traits ([`ScanTables`],
//! [`FillTables`], [`DemandOut`]) rather than the model's types, so that
//! the architecture crates do not depend on the shape the model's
//! amendment (WP-020c) gives those tables. The image and scan phases
//! implement the traits over the model. Each lookup is one dynamic call;
//! `apply` makes one to four per relocation.

use mortar_core::{FileId, SectionId, SymKey};
use mortar_diag::Key;

use crate::class::PlaceFacts;
use crate::error::{DynRefusal, Incompatible, Refusal};
use crate::flags::{DemandFlags, GlobalFlags};
use crate::seq::{Code, Proof, ProofRun};
use crate::shapes::{DynRole, TlsBlock};
use crate::vocab::{Howto, OutputKind, Site, SlotKind, TargetKind};

/// One relocation record, its symbol resolved to a key (§3.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Reloc {
    pub r_offset: u64,
    pub key: SymKey,
    pub r_type: u32,
    /// RELA: `r_addend`. REL: what the caller read, or 0; an architecture
    /// with REL relocations reads every member's addend from the input
    /// bytes itself (§3.3, §3.5) and does not use this field.
    pub addend: i64,
}

/// The records of one relocation section, by index (§3.5).
pub trait Records {
    fn len(&self) -> u32;
    fn get(&self, i: u32) -> Option<Reloc>;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A slice: `&records` of a `&[Reloc]` is a `&dyn Records`.
impl Records for &[Reloc] {
    fn len(&self) -> u32 {
        u32::try_from(<[Reloc]>::len(self)).unwrap_or(u32::MAX)
    }

    fn get(&self, i: u32) -> Option<Reloc> {
        <[Reloc]>::get(self, i as usize).copied()
    }
}

impl Records for Vec<Reloc> {
    fn len(&self) -> u32 {
        self.as_slice().len().try_into().unwrap_or(u32::MAX)
    }

    fn get(&self, i: u32) -> Option<Reloc> {
        self.as_slice().get(i as usize).copied()
    }
}

/// One live relocation as scan and fill see it (§3.4).
#[derive(Clone, Copy)]
pub struct RelocAt<'a> {
    pub file: FileId,
    pub section: SectionId,
    /// The record's index in its relocation section.
    pub index: u32,
    pub r_offset: u64,
    pub key: SymKey,
    pub r_type: u32,
    /// `Some(r_addend)` on RELA, `None` on REL, where the architecture
    /// reads the implicit addend from the input bytes of the field
    /// (`read_addend`, §3.3): from `site` at scan, through
    /// [`SectionOut::input`] at fill.
    pub addend: Option<i64>,
    pub howto: &'a Howto,
    pub site: Site<'a>,
    /// The same relocation section (§3.5).
    pub records: &'a dyn Records,
    /// The instruction proof of the input section's bytes (§3.6 rule 1),
    /// in section offsets: for AArch64 the runs of its mapping symbols
    /// (a64.md §3.4). A word no `Insn` run covers is not proved.
    pub proof: &'a [ProofRun],
    /// Whether the relocation section may hold a record whose type is in
    /// `Facts.seq_leads` (§3.5). `false` only when no record has such a
    /// type; `true` is always correct (scan passes `true`, fill the bit
    /// scan recorded for the section). An architecture skips the patterns
    /// of those sequences when it is `false`.
    pub leads: bool,
}

impl RelocAt<'_> {
    /// The explicit addend of a RELA record (§3.3). A RELA architecture is
    /// never given a REL record (§3.1 refuses the section), so `None`
    /// cannot reach it; it reads as 0 here and is caught in debug builds.
    #[inline]
    #[must_use]
    pub fn rela_addend(&self) -> i64 {
        debug_assert!(self.addend.is_some(), "a REL record given to a RELA architecture");
        self.addend.unwrap_or(0)
    }

    /// Whether the input word at section offset `off` is a proved
    /// instruction (§3.6 rule 1).
    #[must_use]
    pub fn is_insn(&self, off: u64) -> bool {
        let code = Code { bytes: self.site.bytes, addr: 0, proof: self.proof };
        code.word_proof(off).0 == Proof::Insn
    }
}

impl std::fmt::Debug for RelocAt<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelocAt")
            .field("file", &self.file)
            .field("section", &self.section)
            .field("index", &self.index)
            .field("r_offset", &self.r_offset)
            .field("r_type", &self.r_type)
            .field("addend", &self.addend)
            .field("howto", &self.howto.name)
            .finish_non_exhaustive()
    }
}

/// What scan knows about a relocation's symbol (§3.4): input-only facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(clippy::struct_excessive_bools)] // independent input facts, each read on its own
pub struct SymFacts {
    pub kind: TargetKind,
    /// `STT_*` of the resolved definition (or of the reference when
    /// undefined).
    pub sym_type: u8,
    pub st_other: u8,
    /// `STT_TLS`.
    pub tls: bool,
    /// The target is a symbol of a section that is not live: a dropped
    /// COMDAT copy's (`spec/resolve.md` §6.3 case (a)) or, under
    /// `--gc-sections`, a collected section's (§3.4 "Scan sees only live
    /// sections").
    pub dead: bool,
    /// The key is defined only in dropped copies of a COMDAT group
    /// (`spec/resolve.md` §6.3 case (b)); its `kind` is `Undefined`.
    pub comdat_only: bool,
    /// A live file has a `.gnu.warning.<name>` section for the key
    /// (`spec/resolve.md` §10).
    pub gnu_warning: bool,
}

impl SymFacts {
    /// The facts as one `u32` (the state's `RESOLVED.facts`, `spec/incr.md`
    /// IN-18e; WP-239 §13.1, R-4a): bits 0-2 the kind (`Local` 0, `Abs`
    /// 1, `Preemptible` 2, `Shlib` 3, `Ifunc` 4, `Weak0` 5, `WeakDyn` 6,
    /// `Undefined` 7), bit 3 `Shlib.func`, bits 8-11 `sym_type`, bits
    /// 16-23 `st_other`, bits 24-26 `dead`, `comdat_only`, `gnu_warning`;
    /// every other bit 0. `tls` is not stored: [`SymFacts::unpack`] gives
    /// `sym_type == STT_TLS`, which is what every global's facts hold.
    #[must_use]
    pub const fn pack(self) -> u32 {
        let (kind, func) = match self.kind {
            TargetKind::Local => (0, false),
            TargetKind::Abs => (1, false),
            TargetKind::Preemptible => (2, false),
            TargetKind::Shlib { func } => (3, func),
            TargetKind::Ifunc => (4, false),
            TargetKind::Weak0 => (5, false),
            TargetKind::WeakDyn => (6, false),
            TargetKind::Undefined => (7, false),
        };
        kind | (func as u32) << 3
            | ((self.sym_type as u32) & 0xf) << 8
            | (self.st_other as u32) << 16
            | (self.dead as u32) << 24
            | (self.comdat_only as u32) << 25
            | (self.gnu_warning as u32) << 26
    }

    /// The facts [`SymFacts::pack`] stored; `None` for a word no `pack`
    /// gives (a bit outside the layout, or `func` on a kind other than
    /// `Shlib`): a state's reader refuses it.
    #[must_use]
    pub const fn unpack(w: u32) -> Option<Self> {
        const USED: u32 = 0x7 | 0x8 | 0xf00 | 0xff_0000 | 0x700_0000;
        if w & !USED != 0 {
            return None;
        }
        let func = w & 0x8 != 0;
        let kind = match w & 0x7 {
            0 => TargetKind::Local,
            1 => TargetKind::Abs,
            2 => TargetKind::Preemptible,
            3 => TargetKind::Shlib { func },
            4 => TargetKind::Ifunc,
            5 => TargetKind::Weak0,
            6 => TargetKind::WeakDyn,
            _ => TargetKind::Undefined,
        };
        if func && w & 0x7 != 3 {
            return None;
        }
        let sym_type = ((w >> 8) & 0xf) as u8;
        Some(Self {
            kind,
            sym_type,
            st_other: (w >> 16) as u8,
            tls: sym_type == mortar_elf::consts::STT_TLS,
            dead: w & (1 << 24) != 0,
            comdat_only: w & (1 << 25) != 0,
            gnu_warning: w & (1 << 26) != 0,
        })
    }
}

/// The tables scan reads (§3.4: `Inputs`, `Resolution`, `Liveness`),
/// as the generic phase provides them.
pub trait ScanTables: SymNames {
    fn sym(&self, key: SymKey) -> SymFacts;
}

/// Symbol names, as their symbol tables have them: a sequence's pattern
/// names a symbol (§3.5; a64.md §8.4 `__tls_get_addr`), and a diagnostic
/// names its target.
pub trait SymNames: Sync {
    fn name(&self, key: SymKey) -> &[u8];
}

/// The context of `scan` (§3.4).
#[derive(Clone, Copy)]
pub struct ScanCx<'a> {
    pub tables: &'a dyn ScanTables,
    pub output: OutputKind,
    /// The section of the places being scanned.
    pub place: PlaceFacts,
    /// That section's name: a reference to a dead target is classified
    /// by it ([`crate::dead_reference`], §3.4).
    pub name: &'a [u8],
}

impl std::fmt::Debug for ScanCx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScanCx")
            .field("output", &self.output)
            .field("place", &self.place)
            .field("name", &String::from_utf8_lossy(self.name))
            .finish_non_exhaustive()
    }
}

/// Why scan cannot accept a relocation (§3.4, §7). The generic phase turns
/// it into a diagnostic at the place, keyed by [`scan_key`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScanError {
    /// `refused` by name (a type's status, a feature).
    Refused(Refusal),
    /// `relocation-incompatible`.
    Incompatible(Incompatible),
    /// A dynamic output's refusal with a kind of its own
    /// (`spec/dyn.md` D-25, D-44, D-47).
    Dynamic(DynRefusal),
    /// `malformed-input`: the field or its window is outside the section.
    Outside,
    /// `refused`, feature `overlapping relocations` (§3.4).
    Overlap,
    /// The target kind cannot occur in this output kind (a blank cell of
    /// §4.3): the generic phase's tables are inconsistent. Never a user
    /// diagnostic.
    Internal,
}

/// The key of a scan diagnostic (§3.4, model §8): `(70, FileId, section
/// index × 2^32 + record index)`.
#[must_use]
pub fn scan_key(file: FileId, section_index: u32, record: u32) -> Key {
    Key::new(70, file.0, (u64::from(section_index) << 32) | u64::from(record))
}

/// The key of an image diagnostic (§7): `(100, FileId, section index ×
/// 2^32 + record index)`.
#[must_use]
pub fn fill_key(file: FileId, section_index: u32, record: u32) -> Key {
    Key::new(100, file.0, (u64::from(section_index) << 32) | u64::from(record))
}

/// What scan writes (§3.4): flag words merged by bitwise or, per-section
/// counts, errors.
pub trait DemandOut {
    fn raise(&mut self, key: SymKey, flags: DemandFlags);
    fn raise_global(&mut self, flags: GlobalFlags);
    /// Adds `n` dynamic relocations of `role` to the section being scanned.
    fn count(&mut self, role: DynRole, n: u32);
    /// Reports an error for the record `record` of the section being
    /// scanned.
    fn error(&mut self, record: u32, error: ScanError);
    /// Reports that the record `record` of the section being scanned is a
    /// surviving reference to `key` of event class `class` (§3.4;
    /// `spec/image.md` §4.4). The scan driver forms the event's group
    /// from `key` (the key itself for the classes grouped by key; the
    /// referring section index and `key`'s symbol index for
    /// `DiscardedSection`, whose key is a local) and merges the events of
    /// all files into one diagnostic per group, keyed by its least
    /// reference in `(FileId, section index, record index)` order.
    fn reference(&mut self, record: u32, class: EventClass, key: SymKey);
}

/// The class of a reference event (§3.4; `spec/image.md` §4.4). Ordered
/// as declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EventClass {
    /// A reference to a key whose target kind is `Undefined`:
    /// `undefined-symbol`.
    Undefined,
    /// A reference to a key defined only in dropped copies of a COMDAT
    /// group: `comdat-mismatch` (never also `Undefined`).
    ComdatMismatch,
    /// A reference to a symbol of a section that is not live, from a
    /// section `spec/resolve.md` §6.3 makes an error:
    /// `discarded-section-reference`.
    DiscardedSection,
    /// A reference to a key with a `.gnu.warning` section: `gnu-warning`.
    GnuWarning,
}

/// The sink of `scan` (§3.4).
pub struct DemandSink<'a> {
    out: &'a mut dyn DemandOut,
}

impl std::fmt::Debug for DemandSink<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DemandSink").finish_non_exhaustive()
    }
}

impl<'a> DemandSink<'a> {
    pub fn new(out: &'a mut dyn DemandOut) -> Self {
        Self { out }
    }

    pub fn raise(&mut self, key: SymKey, flags: DemandFlags) {
        if !flags.is_empty() {
            self.out.raise(key, flags);
        }
    }

    pub fn raise_global(&mut self, flags: GlobalFlags) {
        if !flags.is_empty() {
            self.out.raise_global(flags);
        }
    }

    pub fn count(&mut self, role: DynRole, n: u32) {
        self.out.count(role, n);
    }

    pub fn error(&mut self, record: u32, error: ScanError) {
        self.out.error(record, error);
    }

    pub fn reference(&mut self, record: u32, class: EventClass, key: SymKey) {
        self.out.reference(record, class, key);
    }
}

/// What fill knows about a relocation's symbol (§3.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SymValue {
    /// `S`: the address, or the PLT/IPLT entry when it is the symbol's
    /// value (`CANON_PLT`, `IPLT`); `st_value` for `SHN_ABS`; 0 for a
    /// symbol not defined in this output.
    pub s: u64,
    pub kind: TargetKind,
    /// `st_size` of the resolved definition.
    pub size: u64,
    /// Whether the symbol's section is live; a relocation in a non-alloc
    /// section against a dead target writes the tombstone (§3.11).
    pub live: bool,
    /// `STT_TLS` (`SymFacts.tls`): an `AbsWord` or `AbsNarrow` relocation
    /// at a non-alloc place against it writes `DtpRel(S + A)` (§4.3
    /// "Non-alloc places", [`crate::writes_dtprel`]).
    pub tls: bool,
}

/// The tables fill reads (§3.6): model §3 and §4, as the image phase
/// provides them.
pub trait FillTables: SymNames {
    fn sym(&self, key: SymKey) -> SymValue;
    /// The address of the key's slot of `kind`; for `SlotKind::TlsLd`, the
    /// output's module slot whatever the key.
    fn slot(&self, key: SymKey, kind: SlotKind) -> Option<u64>;
    /// The key's PLT (or IPLT) entry.
    fn plt(&self, key: SymKey) -> Option<u64>;
    fn flags(&self, key: SymKey) -> DemandFlags;
    /// The patch a site at `(section, offset)` is redirected to.
    fn redirect(&self, section: SectionId, offset: u64) -> Option<u64>;
}

/// The context of `apply` (§3.6).
#[derive(Clone, Copy)]
pub struct FillCx<'a> {
    pub tables: &'a dyn FillTables,
    pub output: OutputKind,
    /// The GOT base address (`Facts.got_base`), when the output has one.
    pub got_base: Option<u64>,
    /// `Placement.tls`, when the output has TLS.
    pub tls: Option<TlsBlock>,
}

impl FillCx<'_> {
    /// `DtpRel(S + A)` (§3.8) of `sym`, the offset in the output's TLS
    /// block, for every class that writes one (`TlsDtpRel`, and §4.3's
    /// `AbsWord`/`AbsNarrow` exception). `A` for a `Weak0` target (§7). A
    /// target only a shared library defines (`Shlib`, `WeakDyn`; §4.3
    /// "Non-alloc places", the only places where one is accepted) is not
    /// in this module's block, so its word is `0 + A` with or without a
    /// `PT_TLS` in the output (the supervisor's ruling on WP-113; lld
    /// writes 0, GNU ld 2.44 and mold 2.37 write `A − tls.start` when
    /// there is a `PT_TLS`, a number that names nothing). Any other target
    /// without a `PT_TLS` is `MissingTerm("PT_TLS")`.
    ///
    /// # Errors
    ///
    /// `FieldError::MissingTerm` when the output has no `PT_TLS` and the
    /// target is defined in it.
    pub fn dtp_rel(&self, sym: SymValue, a: i64) -> Result<i64, crate::FieldError> {
        let x = sym.s.wrapping_add(a as u64);
        match (sym.kind, self.tls) {
            (TargetKind::Weak0 | TargetKind::Shlib { .. } | TargetKind::WeakDyn, _) => Ok(a),
            (_, Some(tls)) => Ok(crate::TlsAbi::dtp_rel(x, tls)),
            (_, None) => Err(crate::FieldError::MissingTerm("PT_TLS")),
        }
    }
}

impl std::fmt::Debug for FillCx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FillCx")
            .field("output", &self.output)
            .field("got_base", &self.got_base)
            .field("tls", &self.tls)
            .finish_non_exhaustive()
    }
}

/// The output bytes of one unit (§3.6) in the clip `[start, start +
/// bytes.len())` of section offsets, pre-filled with the input bytes;
/// `P` of offset 0 is `addr`. `apply` writes only its window, through
/// [`SectionOut::write`]; the part of a write that falls outside the clip
/// is discarded, so the bytes inside the clip are those of an unclipped
/// call (`spec/image.md` I-35). It reads input bytes only through
/// [`SectionOut::input`], inside its own read window and before its unit
/// writes (§3.6; `spec/x86.md` R-13 (d)).
#[derive(Debug)]
pub struct SectionOut<'a> {
    bytes: &'a mut [u8],
    start: u64,
    /// The whole input section, when it is resident (§3.6 `input`).
    resident: Option<&'a [u8]>,
    /// The input bytes just before and just after the clip:
    /// `[start − before.len(), start)` and `[end, end + after.len())`.
    before: &'a [u8],
    after: &'a [u8],
    /// `P` of offset 0.
    pub addr: u64,
    /// The section has `SHF_ALLOC`.
    pub alloc: bool,
    /// The section's name, for the tombstone (§3.11).
    pub name: &'a [u8],
    /// The section has `SHF_EXECINSTR`, when the caller says (§3.6):
    /// x86-64 `DTPOFF32`/`DTPOFF64` write `TpRel` in code and `DtpRel`
    /// elsewhere (`spec/x64.md` X-46). `None` (the default of [`Self::new`])
    /// when unknown; an architecture that needs it refuses by name.
    pub exec: Option<bool>,
    /// The place's OUTPUT section has `SHF_WRITE` (RELRO included), as
    /// the scan's [`crate::PlaceFacts::writable`], when the caller says:
    /// `spec/dyn.md` D-25's class of an absolute word against a
    /// `WeakDyn` key in an `Exec` ([`crate::weak_use_kind`]). `None`
    /// when unknown: such a word is then taken as writable (an absolute
    /// word in a writable place).
    pub writable: Option<bool>,
}

impl<'a> SectionOut<'a> {
    /// The clip of `bytes.len()` bytes starting at section offset `start`.
    pub fn new(bytes: &'a mut [u8], start: u64, addr: u64, alloc: bool, name: &'a [u8]) -> Self {
        Self {
            bytes,
            start,
            addr,
            alloc,
            name,
            exec: None,
            writable: None,
            resident: None,
            before: &[],
            after: &[],
        }
    }

    /// The same clip, with the whole input section resident: [`Self::input`]
    /// reads from it.
    #[must_use]
    pub fn with_resident(mut self, section: &'a [u8]) -> Self {
        self.resident = Some(section);
        self
    }

    /// The same clip, with the input bytes of the two margins of a
    /// non-resident body (`spec/x86.md` R-13 (d)): `before` ends at the
    /// clip's start, `after` starts at its end.
    #[must_use]
    pub fn with_margins(mut self, before: &'a [u8], after: &'a [u8]) -> Self {
        self.before = before;
        self.after = after;
        self
    }

    /// The same clip, with the section's `SHF_EXECINSTR` known.
    #[must_use]
    pub fn with_exec(mut self, exec: bool) -> Self {
        self.exec = Some(exec);
        self
    }

    /// The same clip, with its output section's `SHF_WRITE` known.
    #[must_use]
    pub fn with_writable(mut self, writable: bool) -> Self {
        self.writable = Some(writable);
        self
    }

    /// The whole section: the clip starting at offset 0.
    pub fn whole(bytes: &'a mut [u8], addr: u64, alloc: bool, name: &'a [u8]) -> Self {
        Self::new(bytes, 0, addr, alloc, name)
    }

    /// The clip `[a, b)` in section offsets.
    #[inline]
    #[must_use]
    pub fn clip(&self) -> (u64, u64) {
        (self.start, self.start.saturating_add(self.bytes.len() as u64))
    }

    /// The input bytes at section offsets `[off, off + dst.len())` (§3.6;
    /// `spec/x86.md` R-13 (d)): from the resident section, else from the
    /// clip's buffer (pre-filled with the input bytes, and read by a unit
    /// before it writes: no unit reads a byte another unit writes, §3.4)
    /// and the clip's two margins. `false`, with `dst` unspecified, when a
    /// byte is in none of them.
    #[must_use]
    pub fn input(&self, off: u64, dst: &mut [u8]) -> bool {
        let Some(end) = off.checked_add(dst.len() as u64) else { return false };
        if let Some(sec) = self.resident {
            let got = usize::try_from(off)
                .ok()
                .and_then(|o| Some(o..usize::try_from(end).ok()?))
                .and_then(|r| sec.get(r));
            return got.is_some_and(|src| {
                dst.copy_from_slice(src);
                true
            });
        }
        let (a, b) = self.clip();
        let lo = a.saturating_sub(self.before.len() as u64);
        let hi = b.saturating_add(self.after.len() as u64);
        if off < lo || end > hi {
            return false;
        }
        // Each byte from the one source that holds it: `[lo, a)` the
        // margin before, `[a, b)` the buffer, `[b, hi)` the margin after.
        for (i, d) in dst.iter_mut().enumerate() {
            let x = off + i as u64;
            *d = if x < a {
                self.before[(x - lo) as usize]
            } else if x < b {
                self.bytes[(x - a) as usize]
            } else {
                self.after[(x - b) as usize]
            };
        }
        true
    }

    /// Writes `data` at section offset `off`: the part inside the clip.
    #[inline]
    pub fn write(&mut self, off: u64, data: &[u8]) {
        // The common case: the write lies inside the clip.
        if let Some(at) = off.checked_sub(self.start)
            && let Ok(at) = usize::try_from(at)
            && let Some(dst) = at.checked_add(data.len()).and_then(|e| self.bytes.get_mut(at..e))
        {
            dst.copy_from_slice(data);
            return;
        }
        let (a, b) = self.clip();
        let end = off.saturating_add(data.len() as u64);
        let (lo, hi) = (off.max(a), end.min(b));
        if lo >= hi {
            return;
        }
        // In range: `lo − off < data.len()` and `hi − a ≤ bytes.len()`.
        let src = &data[(lo - off) as usize..(hi - off) as usize];
        self.bytes[(lo - a) as usize..(hi - a) as usize].copy_from_slice(src);
    }
}

/// The context of `plt_variant` (§3.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OutputCx {
    pub output: OutputKind,
    /// The merged `FEATURE_1_AND` property (0 when absent).
    pub feature_1_and: u32,
    /// `-z pac-plt`.
    pub pac_plt: bool,
    /// `-z force-bti` / `-z force-ibt`.
    pub force_bti: bool,
    /// `-z now` (`spec/dyn.md` D-49, R-A6 (b)): `.got.plt` moves into
    /// RELRO and `DF_BIND_NOW` is set; the PLT shapes stay, so this is
    /// a flag beside the variant, not a variant. Always `false` in a
    /// static link.
    pub bind_now: bool,
}

/// The context of `write_plt` (§3.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SynthCx {
    /// The address of the header or entry being written.
    pub addr: u64,
    /// The PLT header's address.
    pub header: u64,
    /// The `.got.plt` word the entry uses (`.got.plt[2]` for the header).
    pub got_plt: u64,
    /// The GOT base.
    pub got_base: u64,
    /// The entry's index in `.rela.plt`.
    pub index: u32,
}

/// The context of the thunk and erratum functions (§3.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PatchCx {
    /// The patch's (thunk's) address.
    pub addr: u64,
    /// A thunk's target `T`; an erratum patch's final word `F`.
    pub target: u64,
    /// An erratum site's `W1` address `Q` (for `erratum_in_place`).
    pub site: u64,
    /// The word the function needs: `W4'` for `write_patch`, the pre-fix
    /// `W1` for `erratum_in_place`.
    pub word: u32,
    /// The output's BTI property is set.
    pub bti: bool,
    /// The word at a thunk's direct target, when it is a proved
    /// instruction (a64.md §3.4); `None` otherwise.
    pub target_word: Option<u32>,
    /// The thunk's target is `SHN_ABS`.
    pub target_abs: bool,
}

//! The GOT, `.got.plt`, the PLT and IPLT in `.plt` and `.rela.plt`
//! (`spec/synth.md` §6; `spec/dyn.md` D-32, D-41, D-45, D-46, D-48, D-49
//! for an output with `.dynamic`).
//!
//! A static output (static PIE included) takes [`GotPlan::new`]; an
//! `Exec` or `Pie` with `.dynamic` takes [`GotPlan::new_dynamic`], which
//! adds PLT0 and the PLT entries, the three reserved `.got.plt` words, the
//! lazy `JUMP_SLOT` words, the `_DYNAMIC` header word, and the GOT's
//! records for `.rela.dyn` ([`GotPlan::got_records`]).
//!
//! **Plan** ([`GotPlan::new`], phase 80): the region of each slot kind in
//! `.got` (S-63) with the header word (S-65), the `.got.plt` words (S-67),
//! the IPLT entries (S-68) and the `IRELATIVE` relocations (S-69): sizes
//! and offsets, from `Slots` and the architecture's shapes.
//!
//! **Write** (image): any range of `.got` ([`GotPlan::write_got`]), of
//! `.got.plt` ([`GotPlan::write_got_plt`]), of the IPLT entries
//! ([`GotPlan::write_plt`]) and of `.rela.plt`
//! ([`GotPlan::write_rela_plt`]), in O(range + 1), from the addresses a
//! [`SlotValues`] oracle gives.

use mortar_arch::{
    Arch, DynRole, FieldError, GotBase, IpltWord, LazyTarget, PltPart, PltVariant, RelForm,
    SlotKind, SynthCx, TargetKind, TlsAbi, TlsBlock,
};
use mortar_core::layout::{SlotIndex, Slots};
use mortar_core::{SymKey, demands::GlobalFlags};

use crate::dynsym::DynsymPlan;
use mortar_diag::Collector;
use mortar_elf::Class;

use crate::error::{Failed, SynthError, fail, feature, internal, key};
use crate::out::Window;

/// The `.got` regions of S-63, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Region {
    /// The header word (S-65).
    Header,
    /// `Slots.got`.
    Addr,
    /// `Slots.tls_ie`.
    TpOff,
    /// `Slots.tls_gd`.
    TlsGd,
    /// `Slots.tls_ld`.
    TlsLd,
    /// `Slots.tls_desc`.
    TlsDesc,
}

impl Region {
    /// The regions in `.got` order.
    pub const ALL: [Self; 6] =
        [Self::Header, Self::Addr, Self::TpOff, Self::TlsGd, Self::TlsLd, Self::TlsDesc];

    fn slot_kind(self) -> SlotKind {
        match self {
            Self::Header | Self::Addr => SlotKind::Addr,
            Self::TpOff => SlotKind::TpOff { negated: false },
            Self::TlsGd => SlotKind::TlsGd,
            Self::TlsLd => SlotKind::TlsLd,
            Self::TlsDesc => SlotKind::TlsDesc,
        }
    }
}

/// What the image phase knows when it writes the GOT: addresses.
pub trait SlotValues: Sync {
    /// The address of `key`'s definition (for an IFUNC its resolver, the
    /// symbol's own value); `None` for an undefined weak key (a64.md §10.1
    /// `weak0`).
    fn address(&self, key: SymKey) -> Option<u64>;
    /// The TLS template (model §4.4 `Placement.tls`), when there is one.
    fn tls(&self) -> Option<TlsBlock>;
    /// `_DYNAMIC`, the address of `.dynamic` (`spec/dyn.md` D-32), in an
    /// output with `.dynamic`; `None` (the default) in a static one.
    fn dynamic(&self) -> Option<u64> {
        None
    }
}

/// The target kind of a key with a slot (`spec/arch.md` §2.1), as the
/// scan decided it: what a dynamic plan needs to know which slot takes
/// which record (`spec/dyn.md` D-46, D-48).
pub trait KeyKinds: Sync {
    fn kind(&self, key: SymKey) -> TargetKind;
}

/// The facts of an output with `.dynamic` that its GOT and PLT depend on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DynGotCx {
    /// A `Pie` (else an `Exec`).
    pub pie: bool,
    /// `-z now` (D-49; `OutputCx.bind_now`): the shapes stay, so the
    /// plan's bytes do not change with it; layout reads it.
    pub bind_now: bool,
}

/// One dynamic record of a `.got` slot (`spec/dyn.md` D-46, D-48), in
/// `.rela.dyn`: `RELATIVE` (its addend the slot's link-time value, written
/// with addresses) or a record against a `.dynsym` entry (addend 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GotRecord {
    /// The slot word's offset in `.got`.
    pub offset: u64,
    pub role: DynRole,
    /// The architecture's type for `role`.
    pub r_type: u32,
    /// The slot's key.
    pub key: Option<SymKey>,
    /// `r_sym`: the key's `.dynsym` index; 0 for `RELATIVE`.
    pub r_sym: u32,
}

/// The dynamic part of a [`GotPlan`].
#[derive(Debug, Clone)]
struct DynPlt {
    /// `Slots.plt`, in PLT order (D-41), and each key's `.dynsym` index.
    plt: Box<[SymKey]>,
    plt_sym: Box<[u32]>,
    plt_of: SlotIndex,
    /// PLT0's bytes, and one PLT entry's.
    header: u64,
    entry: u64,
    lazy: LazyTarget,
    jump_slot: u32,
    /// The `.got` records, by offset.
    records: Box<[GotRecord]>,
    /// How many of `records` are `RELATIVE`.
    relative: u32,
    /// The kind of each `Slots.got` key (D-46's rows).
    got_kinds: Box<[TargetKind]>,
    /// Whether each `tls_ie`/`tls_gd`/`tls_desc` key is a shared object's.
    ie_shared: Box<[bool]>,
    gd_shared: Box<[bool]>,
    desc_shared: Box<[bool]>,
    bind_now: bool,
}

/// The addresses of the chunks themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GotAddrs {
    pub got: u64,
    pub got_plt: u64,
    /// `.plt`.
    pub plt: u64,
}

/// The plan of §6.
#[derive(Debug, Clone)]
pub struct GotPlan {
    word: u8,
    class: Class,
    rel: RelForm,
    /// Whether an IPLT word holds its resolver's address (S-69: REL's
    /// implicit addend, or the architecture's `IpltWord::Resolver`).
    iplt_resolver: bool,
    /// Per region of [`Region::ALL`]: its first byte in `.got` and its
    /// slot size in bytes; `bases[6]` is the size.
    bases: [u64; 7],
    slot: [u64; 6],
    /// The keys of each region with keys (`Addr`, `TpOff`, `TlsGd`, `TlsDesc`).
    got: Box<[SymKey]>,
    tls_ie: Box<[SymKey]>,
    tls_gd: Box<[SymKey]>,
    tls_desc: Box<[SymKey]>,
    iplt: Box<[SymKey]>,
    iplt_of: SlotIndex,
    /// Reserved words at the start of `.got.plt` (S-67).
    reserved: u64,
    variant: PltVariant,
    iplt_entry: u64,
    irelative: u32,
    tls: &'static TlsAbi,
    /// An output with `.dynamic` (`None`: a static one).
    dynamic: Option<DynPlt>,
}

impl GotPlan {
    /// The plan for a static output (S-63 to S-72). `output` is
    /// `Demands.output` (`GOT_BASE` gives AArch64 its header word, and
    /// x86 its `.got.plt`), `variant` the PLT variant chosen from the
    /// merged properties (S-97).
    pub fn new(
        arch: &dyn Arch,
        slots: &Slots,
        output: GlobalFlags,
        variant: PltVariant,
        diag: &Collector,
    ) -> Result<Self, Failed> {
        let facts = arch.facts();
        let word = u64::from(facts.word);
        if !slots.plt.is_empty() {
            // M2: a static output has no PLT entries (S-68).
            return fail(diag, key(0), feature("PLT entries in a static output"));
        }
        let shape = arch.got();
        let header = facts.got_base == GotBase::GotStart && output.contains(GlobalFlags::GOT_BASE);
        let counts = [
            u64::from(header),
            slots.got.len() as u64,
            slots.tls_ie.len() as u64,
            slots.tls_gd.len() as u64,
            u64::from(slots.tls_ld),
            slots.tls_desc.len() as u64,
        ];
        let mut bases = [0u64; 7];
        let mut slot = [0u64; 6];
        for (r, region) in Region::ALL.iter().enumerate() {
            slot[r] = u64::from((shape.slot_words)(region.slot_kind())) * word;
            bases[r + 1] = bases[r] + counts[r] * slot[r];
        }
        // S-67: x86 keeps its three reserved words whenever `.got.plt`
        // exists; it exists for `GOT_BASE` too. AArch64 reserves none in a
        // static output.
        let x86 = facts.got_base == GotBase::GotPltStart;
        let reserved = if x86 && (output.contains(GlobalFlags::GOT_BASE) || !slots.iplt.is_empty())
        {
            u64::from(facts.got_plt_reserved)
        } else {
            0
        };
        let plt = arch.plt(variant);
        let Some(irelative) = arch.dyn_type(DynRole::IRelative) else {
            return fail(diag, key(0), internal("the architecture has no IRELATIVE type"));
        };
        Ok(Self {
            word: facts.word,
            class: facts.class,
            rel: facts.rel_form,
            iplt_resolver: facts.rel_form == RelForm::Rel || facts.iplt_word == IpltWord::Resolver,
            bases,
            slot,
            got: slots.got.clone(),
            tls_ie: slots.tls_ie.clone(),
            tls_gd: slots.tls_gd.clone(),
            tls_desc: slots.tls_desc.clone(),
            iplt: slots.iplt.clone(),
            iplt_of: SlotIndex::new(&slots.iplt),
            reserved,
            variant,
            iplt_entry: u64::from(plt.iplt_entry),
            irelative,
            tls: arch.tls(),
            dynamic: None,
        })
    }

    /// The plan for an `Exec` or `Pie` with `.dynamic` (`spec/dyn.md`
    /// D-32, D-41, D-45, D-46, D-48, D-49; a64 §10.1-§10.2; synth S-107):
    ///
    /// - `.got`: on AArch64 the header word (`_DYNAMIC`) always, then the
    ///   regions of S-63; each slot's record per D-46/D-48 by its key's
    ///   kind (`kinds`): `Local` and `Ifunc` slots a `RELATIVE` in a
    ///   `Pie`, `Shlib` and `WeakDyn` slots a `GLOB_DAT` (place 0), a
    ///   shared key's TLS slots `TPOFF`, `DTPMOD`+`DTPOFF` or `TLSDESC`;
    /// - `.got.plt`: three reserved words (x86 `[0] = _DYNAMIC`), one word
    ///   per PLT entry holding its lazy target, one per IPLT entry, when
    ///   there is a PLT or IPLT entry (or x86's `GOT_BASE`);
    /// - `.plt`: PLT0 and the entries when `Slots.plt` is not empty, then
    ///   the IPLT entries;
    /// - `.rela.plt`: one `JUMP_SLOT` per PLT entry, then one `IRELATIVE`
    ///   per IPLT entry (D-41).
    ///
    /// Every `r_sym` is `dynsym`'s index of the key. Refused by name: a
    /// PLT shape of several sections (x86-64 IBT's `.plt.sec`; follow-up
    /// of the x86-64 package).
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub fn new_dynamic(
        arch: &dyn Arch,
        slots: &Slots,
        output: GlobalFlags,
        variant: PltVariant,
        cx: DynGotCx,
        kinds: &dyn KeyKinds,
        dynsym: &DynsymPlan,
        diag: &Collector,
    ) -> Result<Self, Failed> {
        let facts = arch.facts();
        let word = u64::from(facts.word);
        let shape = arch.got();
        let plt = arch.plt(variant);
        let Some(irelative) = arch.dyn_type(DynRole::IRelative) else {
            return fail(diag, key(0), internal("the architecture has no IRELATIVE type"));
        };
        if plt.chunks.len() != 1 || plt.call_chunk != 0 || plt.lazy_chunk != 0 {
            return fail(diag, key(0), feature("a PLT of several sections in a dynamic output"));
        }
        let Some(lazy) = arch.lazy_target(variant) else {
            return fail(diag, key(0), feature("a dynamic PLT on this architecture"));
        };
        let ty = |role: DynRole| {
            arch.dyn_type(role).ok_or_else(|| internal::<SynthError>("no type for a dynamic role"))
        };
        let index = |k: SymKey| -> Result<u32, SynthError> {
            k.as_global()
                .and_then(|id| dynsym.index(id))
                .ok_or_else(|| internal("a key with a dynamic record has no .dynsym entry"))
        };
        let header = facts.got_base == GotBase::GotStart;
        let counts = [
            u64::from(header),
            slots.got.len() as u64,
            slots.tls_ie.len() as u64,
            slots.tls_gd.len() as u64,
            u64::from(slots.tls_ld),
            slots.tls_desc.len() as u64,
        ];
        let mut bases = [0u64; 7];
        let mut slot = [0u64; 6];
        for (r, region) in Region::ALL.iter().enumerate() {
            slot[r] = u64::from((shape.slot_words)(region.slot_kind())) * word;
            bases[r + 1] = bases[r] + counts[r] * slot[r];
        }
        let shared = |k: SymKey| matches!(kinds.kind(k), TargetKind::Shlib { .. });
        let mut records: Vec<GotRecord> = Vec::new();
        let mut relative = 0u32;
        let mut plain = |offset: u64, role: DynRole, k: SymKey| -> Result<(), SynthError> {
            records.push(GotRecord {
                offset,
                role,
                r_type: ty(role)?,
                key: Some(k),
                r_sym: index(k)?,
            });
            Ok(())
        };
        let got_kinds: Box<[TargetKind]> = slots.got.iter().map(|&k| kinds.kind(k)).collect();
        let mut rel_rows: Vec<GotRecord> = Vec::new();
        let rel_type = ty(DynRole::Relative);
        let built = (|| -> Result<(), SynthError> {
            for (i, (&k, &kind)) in slots.got.iter().zip(got_kinds.iter()).enumerate() {
                let off = bases[1] + i as u64 * slot[1];
                match kind {
                    TargetKind::Local | TargetKind::Ifunc if cx.pie => {
                        rel_rows.push(GotRecord {
                            offset: off,
                            role: DynRole::Relative,
                            r_type: rel_type.clone()?,
                            key: Some(k),
                            r_sym: 0,
                        });
                    }
                    TargetKind::Shlib { .. } | TargetKind::WeakDyn => {
                        plain(off, DynRole::GlobDat, k)?;
                    }
                    TargetKind::Preemptible | TargetKind::Undefined => {
                        return Err(internal("a GOT slot whose key has no slot record"));
                    }
                    _ => {}
                }
            }
            for (i, &k) in slots.tls_ie.iter().enumerate() {
                if shared(k) {
                    plain(bases[2] + i as u64 * slot[2], DynRole::TpOff { negated: false }, k)?;
                }
            }
            for (i, &k) in slots.tls_gd.iter().enumerate() {
                if shared(k) {
                    let off = bases[3] + i as u64 * slot[3];
                    plain(off, DynRole::DtpMod, k)?;
                    plain(off + word, DynRole::DtpOff, k)?;
                }
            }
            for (i, &k) in slots.tls_desc.iter().enumerate() {
                if shared(k) {
                    plain(bases[5] + i as u64 * slot[5], DynRole::TlsDesc, k)?;
                }
            }
            Ok(())
        })();
        if let Err(e) = built {
            return fail(diag, key(0), e.into_diagnostic());
        }
        relative = relative.saturating_add(rel_rows.len() as u32);
        rel_rows.extend(records);
        let records = rel_rows;
        let plt_sym: Result<Box<[u32]>, SynthError> = slots.plt.iter().map(|&k| index(k)).collect();
        let plt_sym = match plt_sym {
            Ok(v) => v,
            Err(e) => return fail(diag, key(0), e.into_diagnostic()),
        };
        let jump_slot = match ty(DynRole::JumpSlot) {
            Ok(t) => t,
            Err(e) => return fail(diag, key(0), e.into_diagnostic()),
        };
        let x86 = facts.got_base == GotBase::GotPltStart;
        let entries = !slots.plt.is_empty() || !slots.iplt.is_empty();
        let reserved = if entries || (x86 && output.contains(GlobalFlags::GOT_BASE)) {
            u64::from(facts.got_plt_reserved).max(3)
        } else {
            0
        };
        let chunk = plt.chunks[0];
        let (plt_header, plt_entry) = if slots.plt.is_empty() {
            (0, u64::from(chunk.entry))
        } else {
            (u64::from(chunk.header), u64::from(chunk.entry))
        };
        let is_shared =
            |list: &[SymKey]| -> Box<[bool]> { list.iter().map(|&k| shared(k)).collect() };
        Ok(Self {
            word: facts.word,
            class: facts.class,
            rel: facts.rel_form,
            iplt_resolver: facts.rel_form == RelForm::Rel || facts.iplt_word == IpltWord::Resolver,
            bases,
            slot,
            got: slots.got.clone(),
            tls_ie: slots.tls_ie.clone(),
            tls_gd: slots.tls_gd.clone(),
            tls_desc: slots.tls_desc.clone(),
            iplt: slots.iplt.clone(),
            iplt_of: SlotIndex::new(&slots.iplt),
            reserved,
            variant,
            iplt_entry: u64::from(plt.iplt_entry),
            irelative,
            tls: arch.tls(),
            dynamic: Some(DynPlt {
                plt: slots.plt.clone(),
                plt_sym,
                plt_of: SlotIndex::new(&slots.plt),
                header: plt_header,
                entry: plt_entry,
                lazy,
                jump_slot,
                records: records.into(),
                relative,
                got_kinds,
                ie_shared: is_shared(&slots.tls_ie),
                gd_shared: is_shared(&slots.tls_gd),
                desc_shared: is_shared(&slots.tls_desc),
                bind_now: cx.bind_now,
            }),
        })
    }

    /// The `.got` records for `.rela.dyn` (D-40, D-46, D-48): the
    /// `RELATIVE`s first, then the others, each group by offset. Empty in
    /// a static output.
    #[must_use]
    pub fn got_records(&self) -> &[GotRecord] {
        self.dynamic.as_ref().map_or(&[], |d| &d.records)
    }

    /// How many of [`Self::got_records`] are `RELATIVE`.
    #[must_use]
    pub fn got_relative(&self) -> u32 {
        self.dynamic.as_ref().map_or(0, |d| d.relative)
    }

    /// Whether the plan is a dynamic output's.
    #[must_use]
    pub fn is_dynamic(&self) -> bool {
        self.dynamic.is_some()
    }

    /// `-z now` (D-49): layout places `.got.plt` in RELRO.
    #[must_use]
    pub fn bind_now(&self) -> bool {
        self.dynamic.as_ref().is_some_and(|d| d.bind_now)
    }

    /// PLT entries (`Slots.plt`); 0 in a static output.
    fn plt_count(&self) -> u64 {
        self.dynamic.as_ref().map_or(0, |d| d.plt.len() as u64)
    }

    /// The offset in `.plt` of PLT entry `n` (after PLT0).
    #[must_use]
    pub fn plt_entry(&self, n: u32) -> u64 {
        self.dynamic.as_ref().map_or(0, |d| d.header + u64::from(n) * d.entry)
    }

    /// The offset in `.got.plt` of PLT entry `n`'s word (a64 §10.2).
    #[must_use]
    pub fn plt_word(&self, n: u32) -> u64 {
        (self.reserved + u64::from(n)) * u64::from(self.word)
    }

    /// The address of `key`'s PLT entry, or of its IPLT entry: every
    /// address of a key with one (S-71; D-44's canonical PLT; a branch's
    /// target).
    #[must_use]
    pub fn plt_address(&self, at: &GotAddrs, key: SymKey) -> Option<u64> {
        if let Some(d) = &self.dynamic
            && let Some(n) = d.plt_of.find(key)
        {
            return Some(at.plt + self.plt_entry(n));
        }
        self.iplt_address(at, key)
    }

    /// The size of `.got` (S-108); 0 means no chunk (S-3).
    #[must_use]
    pub fn got_size(&self) -> u64 {
        self.bases[6]
    }

    /// The offset in `.got` of slot `index` of `region` (S-63).
    #[must_use]
    pub fn got_offset(&self, region: Region, index: u32) -> u64 {
        let r = region as usize;
        self.bases[r] + u64::from(index) * self.slot[r]
    }

    /// The number of `.got.plt` words; 0 means no chunk (S-67).
    #[must_use]
    pub fn got_plt_words(&self) -> u64 {
        self.reserved + self.plt_count() + self.iplt.len() as u64
    }

    /// The size of `.got.plt`.
    #[must_use]
    pub fn got_plt_size(&self) -> u64 {
        self.got_plt_words() * u64::from(self.word)
    }

    /// The offset in `.got.plt` of IPLT entry `n`'s word (S-67: after
    /// the PLT words).
    #[must_use]
    pub fn iplt_word(&self, n: u32) -> u64 {
        (self.reserved + self.plt_count() + u64::from(n)) * u64::from(self.word)
    }

    /// The bytes before the first IPLT entry in `.plt`: PLT0 and the PLT
    /// entries (none in a static output).
    fn iplt_base(&self) -> u64 {
        self.dynamic.as_ref().map_or(0, |d| d.header + d.plt.len() as u64 * d.entry)
    }

    /// The size of `.plt`: PLT0 and the PLT entries (dynamic), then the
    /// IPLT entries (S-68, no header in a static output).
    #[must_use]
    pub fn plt_size(&self) -> u64 {
        self.iplt_base() + self.iplt.len() as u64 * self.iplt_entry
    }

    /// The PLT variant the plan was made with (S-97).
    #[must_use]
    pub fn variant(&self) -> PltVariant {
        self.variant
    }

    /// The offset in `.plt` of IPLT entry `n`.
    #[must_use]
    pub fn iplt_entry(&self, n: u32) -> u64 {
        self.iplt_base() + u64::from(n) * self.iplt_entry
    }

    /// The bytes of one `.rela.plt` entry.
    #[must_use]
    pub fn rela_entsize(&self) -> u64 {
        match (self.rel, self.class) {
            (RelForm::Rela, Class::Elf64) => 24,
            (RelForm::Rela, Class::Elf32) => 12,
            (RelForm::Rel, Class::Elf64) => 16,
            (RelForm::Rel, Class::Elf32) => 8,
        }
    }

    /// The size of `.rela.plt` (S-108); 0 means no chunk.
    #[must_use]
    pub fn rela_plt_size(&self) -> u64 {
        (self.plt_count() + self.iplt.len() as u64) * self.rela_entsize()
    }

    /// The address of `key`'s IPLT entry, if it has one (S-71: every
    /// address of the IFUNC a relocation writes).
    #[must_use]
    pub fn iplt_address(&self, at: &GotAddrs, key: SymKey) -> Option<u64> {
        self.iplt_of.find(key).map(|n| at.plt + self.iplt_entry(n))
    }

    fn tls_block(values: &dyn SlotValues) -> Result<TlsBlock, SynthError> {
        values.tls().ok_or_else(|| internal("a TLS GOT slot without a TLS template"))
    }

    /// The static value of one slot (S-66, S-72), one or two words.
    fn slot_value(
        &self,
        region: Region,
        key: Option<SymKey>,
        at: &GotAddrs,
        values: &dyn SlotValues,
    ) -> Result<[u64; 2], SynthError> {
        let s = key.and_then(|k| values.address(k));
        Ok(match region {
            // S-65: 0 in a static output; D-32: `_DYNAMIC` in a dynamic one.
            Region::Header if self.dynamic.is_some() => {
                let d = values.dynamic().ok_or_else(|| internal::<SynthError>("no _DYNAMIC"))?;
                [d, 0]
            }
            Region::Header => [0, 0],
            Region::Addr => {
                let k = key.ok_or_else(|| internal::<SynthError>("an Addr slot without a key"))?;
                [self.iplt_address(at, k).or(s).unwrap_or(0), 0]
            }
            Region::TpOff => match s {
                Some(s) => [self.tls.tp_rel(s, Self::tls_block(values)?) as u64, 0],
                None => [0, 0],
            },
            Region::TlsGd => match s {
                Some(s) => [1, TlsAbi::dtp_rel(s, Self::tls_block(values)?) as u64],
                None => [1, 0],
            },
            // S-72: module 1 (the executable), offset 0.
            Region::TlsLd => [1, 0],
            // a64.md §10.6: relaxed away in a static output.
            Region::TlsDesc => {
                return Err(internal("a TLS descriptor slot in a static output"));
            }
        })
    }

    /// D-46, D-48: a dynamic slot whose record carries its value holds 0
    /// (`GLOB_DAT` of a `Shlib` or `WeakDyn` key; a shared key's TLS
    /// slots).
    fn zero_slot(&self, region: Region, index: u64) -> bool {
        let Some(d) = &self.dynamic else { return false };
        let i = index as usize;
        match region {
            Region::Addr => {
                matches!(d.got_kinds.get(i), Some(TargetKind::Shlib { .. } | TargetKind::WeakDyn))
            }
            Region::TpOff => d.ie_shared.get(i).copied().unwrap_or(false),
            Region::TlsGd => d.gd_shared.get(i).copied().unwrap_or(false),
            Region::TlsDesc => d.desc_shared.get(i).copied().unwrap_or(false),
            Region::Header | Region::TlsLd => false,
        }
    }

    fn region_key(&self, region: Region, index: u64) -> Option<SymKey> {
        let list: &[SymKey] = match region {
            Region::Addr => &self.got,
            Region::TpOff => &self.tls_ie,
            Region::TlsGd => &self.tls_gd,
            Region::TlsDesc => &self.tls_desc,
            Region::Header | Region::TlsLd => return None,
        };
        list.get(index as usize).copied()
    }

    /// Fills the window's range of `.got` (S-66).
    pub fn write_got(
        &self,
        at: &GotAddrs,
        values: &dyn SlotValues,
        out: &mut Window<'_>,
    ) -> Result<(), SynthError> {
        let w = u64::from(self.word);
        for (r, &region) in Region::ALL.iter().enumerate() {
            let (base, end, size) = (self.bases[r], self.bases[r + 1], self.slot[r]);
            if end <= out.start() || base >= out.end() || size == 0 {
                continue;
            }
            let first = out.start().saturating_sub(base) / size;
            let last = (out.end().min(end) - base).div_ceil(size);
            for i in first..last {
                let v = if self.zero_slot(region, i) {
                    [0, 0]
                } else {
                    self.slot_value(region, self.region_key(region, i), at, values)?
                };
                let off = base + i * size;
                for (j, word) in v.iter().enumerate().take((size / w) as usize) {
                    out.word(off + j as u64 * w, self.word, *word);
                }
            }
        }
        Ok(())
    }

    /// Fills the window's range of `.got.plt` (S-67, S-69; D-32, D-49):
    /// reserved words 0 (x86 dynamic: `[0] = _DYNAMIC`); a PLT word its
    /// lazy target; an IPLT word the resolver's address (REL, and
    /// AArch64's `S + A`), or 0 (x86-64).
    pub fn write_got_plt(
        &self,
        values: &dyn SlotValues,
        out: &mut Window<'_>,
    ) -> Result<(), SynthError> {
        self.write_got_plt_at(None, values, out)
    }

    /// [`Self::write_got_plt`] with the chunk addresses, which a dynamic
    /// output's lazy targets need (`None`: a static output's).
    pub fn write_got_plt_at(
        &self,
        at: Option<&GotAddrs>,
        values: &dyn SlotValues,
        out: &mut Window<'_>,
    ) -> Result<(), SynthError> {
        let w = u64::from(self.word);
        let first = out.start() / w;
        let last = out.end().min(self.got_plt_size()).div_ceil(w);
        let nplt = self.plt_count();
        let x86_dynamic = self.dynamic.is_some() && self.reserved > 0 && !self.got_header();
        for i in first..last {
            let v = match i.checked_sub(self.reserved) {
                None if i == 0 && x86_dynamic => {
                    values.dynamic().ok_or_else(|| internal::<SynthError>("no _DYNAMIC"))?
                }
                Some(n) if n < nplt => {
                    let d = self.dynamic.as_ref().ok_or_else(|| internal::<SynthError>("PLT"))?;
                    let at = at.ok_or_else(|| internal::<SynthError>("no PLT address"))?;
                    match d.lazy {
                        LazyTarget::Plt0 => at.plt,
                        LazyTarget::Entry { plus } => {
                            at.plt + self.plt_entry(n as u32) + u64::from(plus)
                        }
                    }
                }
                Some(n) if self.iplt_resolver => self.resolver(n - nplt, values)?,
                None | Some(_) => 0,
            };
            out.word(i * w, self.word, v);
        }
        Ok(())
    }

    /// Whether `.got` has the header word (AArch64's `_DYNAMIC`).
    fn got_header(&self) -> bool {
        self.bases[1] > 0
    }

    fn resolver(&self, n: u64, values: &dyn SlotValues) -> Result<u64, SynthError> {
        let key = self.iplt[n as usize];
        values.address(key).ok_or_else(|| internal("an IPLT entry for an undefined key"))
    }

    /// Fills the window's range of `.plt` (S-68): `arch.write_plt` per
    /// IPLT entry. An error names the entry.
    pub fn write_plt(
        &self,
        arch: &dyn Arch,
        at: &GotAddrs,
        out: &mut Window<'_>,
    ) -> Result<(), (u32, FieldError)> {
        let got_base = at.got;
        let w = u64::from(self.word);
        if let Some(d) = &self.dynamic
            && !d.plt.is_empty()
        {
            // PLT0 (index `u32::MAX` names it in an error), then the entries.
            if out.start() < d.header {
                let mut buf = vec![0u8; d.header as usize];
                let cx = SynthCx {
                    addr: at.plt,
                    header: at.plt,
                    got_plt: at.got_plt + 2 * w,
                    got_base,
                    index: 0,
                };
                arch.write_plt(self.variant, PltPart::Header(0), &cx, &mut buf)
                    .map_err(|e| (u32::MAX, e))?;
                out.put(0, &buf);
            }
            let end = d.header + d.plt.len() as u64 * d.entry;
            if out.end() > d.header && out.start() < end {
                let first = out.start().saturating_sub(d.header) / d.entry;
                let last = (out.end().min(end) - d.header).div_ceil(d.entry);
                let mut buf = vec![0u8; d.entry as usize];
                for n in first..last {
                    let n32 = n as u32;
                    let cx = SynthCx {
                        addr: at.plt + self.plt_entry(n32),
                        header: at.plt,
                        got_plt: at.got_plt + self.plt_word(n32),
                        got_base,
                        index: n32,
                    };
                    buf.fill(0);
                    arch.write_plt(
                        self.variant,
                        PltPart::Entry { chunk: 0, n: n32 },
                        &cx,
                        &mut buf,
                    )
                    .map_err(|e| (n32, e))?;
                    out.put(self.plt_entry(n32), &buf);
                }
            }
        }
        let e = self.iplt_entry;
        if e == 0 || self.iplt.is_empty() {
            return Ok(());
        }
        let base = self.iplt_base();
        let size = self.plt_size();
        if out.end() <= base || out.start() >= size {
            return Ok(());
        }
        let first = out.start().saturating_sub(base) / e;
        let last = (out.end().min(size) - base).div_ceil(e);
        let mut buf = vec![0u8; e as usize];
        let nplt = self.plt_count() as u32;
        for n in first..last {
            let n32 = n as u32;
            let cx = SynthCx {
                addr: at.plt + self.iplt_entry(n32),
                header: at.plt,
                got_plt: at.got_plt + self.iplt_word(n32),
                got_base,
                index: nplt + n32,
            };
            buf.fill(0);
            arch.write_plt(self.variant, PltPart::Iplt(n32), &cx, &mut buf)
                .map_err(|e| (n32, e))?;
            out.put(self.iplt_entry(n32), &buf);
        }
        Ok(())
    }

    /// Fills the window's range of `.rela.plt` (S-69, `spec/dyn.md`
    /// D-41): in a dynamic output first one `JUMP_SLOT` per PLT entry in
    /// PLT order (its `.got.plt` word, the key's `.dynsym` index, addend
    /// 0); then one `IRELATIVE` per IPLT entry, `r_offset` its `.got.plt`
    /// word, symbol 0, addend the resolver (RELA).
    pub fn write_rela_plt(
        &self,
        at: &GotAddrs,
        values: &dyn SlotValues,
        out: &mut Window<'_>,
    ) -> Result<(), SynthError> {
        let e = self.rela_entsize();
        let first = out.start() / e;
        let last = out.end().min(self.rela_plt_size()).div_ceil(e);
        let w = self.word;
        let ww = u64::from(w);
        let nplt = self.plt_count();
        let shift = if self.class == Class::Elf64 { 32 } else { 8 };
        for i in first..last {
            let off = i * e;
            if let Some(d) = &self.dynamic
                && i < nplt
            {
                // D-41: `JUMP_SLOT`, the entry's word, its key, addend 0.
                let n = i as u32;
                out.word(off, w, at.got_plt + self.plt_word(n));
                let r_sym = u64::from(d.plt_sym[n as usize]);
                out.word(off + ww, w, r_sym << shift | u64::from(d.jump_slot));
                if self.rel == RelForm::Rela {
                    out.word(off + 2 * ww, w, 0);
                }
                continue;
            }
            let n = i - nplt;
            let r_offset = at.got_plt + self.iplt_word(n as u32);
            out.word(off, w, r_offset);
            // r_info: symbol 0, the type in the low bits (ELF64: 32, ELF32: 8).
            out.word(off + ww, w, u64::from(self.irelative));
            if self.rel == RelForm::Rela {
                out.word(off + 2 * ww, w, self.resolver(n, values)?);
            }
        }
        Ok(())
    }
}

//! The GOT, `.got.plt`, the IPLT in `.plt` and `.rela.plt` of a static
//! output (`spec/synth.md` §6).
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
    Arch, DynRole, FieldError, GotBase, IpltWord, PltPart, PltVariant, RelForm, SlotKind, SynthCx,
    TlsAbi, TlsBlock,
};
use mortar_core::layout::{SlotIndex, Slots};
use mortar_core::{SymKey, demands::GlobalFlags};
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
        })
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
        self.reserved + self.iplt.len() as u64
    }

    /// The size of `.got.plt`.
    #[must_use]
    pub fn got_plt_size(&self) -> u64 {
        self.got_plt_words() * u64::from(self.word)
    }

    /// The offset in `.got.plt` of IPLT entry `n`'s word (S-67).
    #[must_use]
    pub fn iplt_word(&self, n: u32) -> u64 {
        (self.reserved + u64::from(n)) * u64::from(self.word)
    }

    /// The size of `.plt`: the IPLT entries (S-68, no header in a static
    /// output).
    #[must_use]
    pub fn plt_size(&self) -> u64 {
        self.iplt.len() as u64 * self.iplt_entry
    }

    /// The PLT variant the plan was made with (S-97).
    #[must_use]
    pub fn variant(&self) -> PltVariant {
        self.variant
    }

    /// The offset in `.plt` of IPLT entry `n`.
    #[must_use]
    pub fn iplt_entry(&self, n: u32) -> u64 {
        u64::from(n) * self.iplt_entry
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
        self.iplt.len() as u64 * self.rela_entsize()
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
            // S-65: 0 in a static output.
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
                let v = self.slot_value(region, self.region_key(region, i), at, values)?;
                let off = base + i * size;
                for (j, word) in v.iter().enumerate().take((size / w) as usize) {
                    out.word(off + j as u64 * w, self.word, *word);
                }
            }
        }
        Ok(())
    }

    /// Fills the window's range of `.got.plt` (S-67, S-69): reserved
    /// words 0; an IPLT word the resolver's address (REL, and AArch64's
    /// `S + A`), or 0 (x86-64).
    pub fn write_got_plt(
        &self,
        values: &dyn SlotValues,
        out: &mut Window<'_>,
    ) -> Result<(), SynthError> {
        let w = u64::from(self.word);
        let first = out.start() / w;
        let last = out.end().min(self.got_plt_size()).div_ceil(w);
        for i in first..last {
            let v = match i.checked_sub(self.reserved) {
                Some(n) if self.iplt_resolver => self.resolver(n, values)?,
                _ => 0,
            };
            out.word(i * w, self.word, v);
        }
        Ok(())
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
        let e = self.iplt_entry;
        if e == 0 {
            return Ok(());
        }
        let first = out.start() / e;
        let last = out.end().min(self.plt_size()).div_ceil(e);
        let mut buf = vec![0u8; e as usize];
        let got_base = at.got;
        for n in first..last {
            let n32 = n as u32;
            let cx = SynthCx {
                addr: at.plt + self.iplt_entry(n32),
                header: at.plt,
                got_plt: at.got_plt + self.iplt_word(n32),
                got_base,
                index: n32,
            };
            buf.fill(0);
            arch.write_plt(self.variant, PltPart::Iplt(n32), &cx, &mut buf)
                .map_err(|e| (n32, e))?;
            out.put(self.iplt_entry(n32), &buf);
        }
        Ok(())
    }

    /// Fills the window's range of `.rela.plt` (S-69): one `IRELATIVE`
    /// per IPLT entry, `r_offset` its `.got.plt` word, symbol 0, addend
    /// the resolver (RELA).
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
        for n in first..last {
            let off = n * e;
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

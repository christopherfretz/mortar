//! Facts and shapes (§3.1, §3.7 to §3.13): what an architecture states
//! about its tables, stubs and conventions, as data.

use mortar_elf::{Class, Machine};

use crate::vocab::{SlotKind, align_up};

/// The relocation section type an architecture accepts (§3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RelForm {
    Rela,
    Rel,
}

/// Where the GOT base (`_GLOBAL_OFFSET_TABLE_`) is (§3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GotBase {
    /// The start of `.got` (AArch64).
    GotStart,
    /// The start of `.got.plt` (x86-64, i386).
    GotPltStart,
}

/// The architecture's constants (§3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Facts {
    pub machine: Machine,
    pub class: Class,
    pub e_machine: u16,
    pub rel_form: RelForm,
    /// 8 or 4.
    pub word: u8,
    /// `EI_OSABI` written.
    pub osabi: u8,
    /// The default of `-z max-page-size`.
    pub max_page: u64,
    /// The default of `-z common-page-size`.
    pub common_page: u64,
    /// The non-PIE default image base; PIE and shared are 0.
    pub image_base: u64,
    /// `Page(x) = x & !page_mask`; 0 if unused.
    pub page_mask: u64,
    pub got_base: GotBase,
    /// Words reserved at the start of `.got` with `.dynamic`.
    pub got_header: u8,
    /// Words reserved at the start of `.got.plt` with `.dynamic`.
    pub got_plt_reserved: u8,
}

/// The size of each slot kind, in words (§3.7).
#[derive(Debug, Clone, Copy)]
pub struct GotShape {
    pub slot_words: fn(SlotKind) -> u8,
}

/// The PLT shape of a link (§3.7), chosen once per link by
/// `plt_variant`. Each architecture uses its own subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PltVariant {
    /// AArch64.
    Plain,
    Bti,
    Pac,
    BtiPac,
    /// x86-64.
    Lazy,
    Ibt,
    /// i386.
    Abs,
    Pic,
    IbtAbs,
    IbtPic,
}

/// One output section a PLT occupies (§3.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PltChunk {
    pub name: &'static str,
    pub header: u32,
    pub entry: u32,
    pub align: u32,
}

/// The chunks of a PLT variant, in address order (§3.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PltShape {
    pub chunks: &'static [PltChunk],
    /// The chunk whose entry is the symbol's PLT address (`L`).
    pub call_chunk: u8,
    /// The chunk whose entry the `.got.plt` word initially points to.
    pub lazy_chunk: u8,
    /// Bytes of an IPLT entry (in `call_chunk`'s section).
    pub iplt_entry: u32,
}

/// Which part of a PLT `write_plt` writes (§3.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PltPart {
    Header(u8),
    Entry { chunk: u8, n: u32 },
    Iplt(u32),
}

/// The reach of a branch: `min ≤ T − P ≤ max` (§3.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Reach {
    pub min: i64,
    pub max: i64,
}

impl Reach {
    #[must_use]
    pub const fn contains(self, d: i64) -> bool {
        self.min <= d && d <= self.max
    }

    /// Whether a branch at `from` reaches `to` (64-bit difference).
    #[must_use]
    pub const fn reaches(self, from: u64, to: u64) -> bool {
        self.contains(to.wrapping_sub(from) as i64)
    }
}

/// A branch type's reach and whether a veneer may be inserted (§3.7, §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Branch {
    pub reach: Reach,
    pub veneer: bool,
}

/// The size and alignment of a range-extension thunk (§3.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ThunkShape {
    pub size: u32,
    pub align: u32,
}

/// The errata an architecture can fix (§3.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Erratum {
    A53_843419,
}

/// The TLS variant (§3.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TlsVariant {
    /// The TCB of `tcb` bytes precedes the block (AArch64: 16).
    I { tcb: u64 },
    /// The block precedes the thread pointer (x86-64, i386).
    II,
}

/// The TLS template of the output (model §4.4 `Placement.tls`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TlsBlock {
    pub start: u64,
    pub size: u64,
    pub align: u64,
}

/// The TLS ABI of an architecture (§3.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TlsAbi {
    pub variant: TlsVariant,
    pub get_addr: &'static [u8],
    pub module_base: Option<&'static [u8]>,
}

impl TlsAbi {
    /// `TpRel(x)` (§3.8) under the validity rule `tls.start ≡ 0 (mod
    /// tls.align)`: variant I `x − start + align_up(tcb, align)`; variant
    /// II `x − start − align_up(size, align)`.
    #[must_use]
    pub const fn tp_rel(&self, x: u64, tls: TlsBlock) -> i64 {
        let off = x.wrapping_sub(tls.start);
        match self.variant {
            TlsVariant::I { tcb } => off.wrapping_add(align_up(tcb, tls.align)) as i64,
            TlsVariant::II => off.wrapping_sub(align_up(tls.size, tls.align)) as i64,
        }
    }

    /// `DtpRel(x) = x − tls.start` (§3.8).
    #[must_use]
    pub const fn dtp_rel(x: u64, tls: TlsBlock) -> i64 {
        x.wrapping_sub(tls.start) as i64
    }
}

/// The `.eh_frame_hdr` encodings (§3.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EhEncodings {
    pub hdr_ptr: u8,
    pub hdr_count: u8,
    pub hdr_table: u8,
}

/// The LSB `.eh_frame_hdr` encodings all three targets use (§3.9).
pub const EH_ENCODINGS: EhEncodings =
    EhEncodings { hdr_ptr: 0x1b, hdr_count: 0x03, hdr_table: 0x3b };

/// The role of a dynamic relocation (§3.10); `dyn_type` maps it to the
/// architecture's number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DynRole {
    Relative,
    Symbolic,
    GlobDat,
    JumpSlot,
    Copy,
    IRelative,
    DtpMod,
    DtpOff,
    TpOff { negated: bool },
    TlsDesc,
}

impl DynRole {
    /// Every role (`TpOff` both ways), in declaration order.
    pub const ALL: [Self; 11] = [
        Self::Relative,
        Self::Symbolic,
        Self::GlobDat,
        Self::JumpSlot,
        Self::Copy,
        Self::IRelative,
        Self::DtpMod,
        Self::DtpOff,
        Self::TpOff { negated: false },
        Self::TpOff { negated: true },
        Self::TlsDesc,
    ];
}

/// How an undefined weak (`Weak0`) target's value is formed (§3.12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Weak0Rule {
    /// `S = 0`.
    SZero,
    /// `S = P`.
    SIsP,
    /// `X = 0`.
    XZero,
    /// A constant `X` (AArch64 branches: 4).
    XConst(i64),
}

/// How a GNU property merges over the inputs (§3.13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PropMerge {
    /// An input without the property counts as 0; bitwise and.
    And,
    /// An input without the property counts as 0; bitwise or.
    Or,
}

//! Facts and shapes (§3.1, §3.7 to §3.13): what an architecture states
//! about its tables, stubs and conventions, as data.

use mortar_elf::{Class, Machine};

use crate::vocab::{SlotKind, Window, align_up};

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

/// What an IPLT entry's `.got.plt` word holds before start-up, beside its
/// `IRELATIVE` (`spec/synth.md` S-69; `spec/dyn.md` Q7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IpltWord {
    /// The record's `S + A`, the resolver's address: AArch64
    /// (`spec/a64.md` §6.2, §10.2) and i386 (where REL makes it the
    /// implicit addend, `spec/x86.md` §6.2).
    Resolver,
    /// 0: x86-64 (`spec/x64.md` X-39).
    Zero,
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
    /// Words reserved at the start of `.got.plt`: with `.dynamic` on every
    /// target; also in static outputs on x86 (`spec/synth.md` S-67).
    pub got_plt_reserved: u8,
    /// The initial content of an IPLT entry's `.got.plt` word (S-69).
    pub iplt_word: IpltWord,
    /// The largest `before` and `after` of any write window of the
    /// architecture, over every `Howto` window (§3.2) and every member
    /// window of a recognized form (`unit_window`, §3.5). Computed from
    /// those tables ([`max_window`]), never written by hand
    /// (`spec/image.md` I-111): AArch64 `(0, 4)`; x86-64 `(4, 15)` (the
    /// 16-byte `TLSGD` forms' `before`, the 22-byte forms' `after`,
    /// `spec/x64.md` §8.6).
    pub max_reach: Window,
    /// The trap pattern code padding is filled with, positioned by address
    /// modulo its length (`spec/layout.md` L-82): AArch64 four zero bytes
    /// (`UDF #0`), x86 `0xCC`.
    pub code_fill: &'static [u8],
    /// The no-op that pads between the inputs of `.init` and `.fini`
    /// (L-82): AArch64 `NOP` (`0xd503201f`, little-endian), x86 `0x90`.
    pub code_nop: &'static [u8],
    /// The lead types of the sequences an architecture recognizes only
    /// when `RelocAt.leads` is set (§3.5): AArch64 `[513]` (R4's
    /// `TLSGD_ADR_PAGE21`); x86-64 `[19, 20]` (`TLSGD`, `TLSLD`,
    /// `spec/x64.md` §8.3, §8.4).
    pub seq_leads: &'static [u32],
}

/// The larger `before` and the larger `after` of two windows.
#[must_use]
pub const fn max_window(a: Window, b: Window) -> Window {
    Window {
        before: if a.before > b.before { a.before } else { b.before },
        after: if a.after > b.after { a.after } else { b.after },
    }
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

/// The errata an architecture can fix (§3.7): the model's type
/// (`spec/model.md` §4.4, amendment M-7). arch.md §3.7 names the variant
/// `A53_843419`; the model names it `E843419`.
pub use mortar_core::layout::Erratum;

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

impl DynRole {
    /// What the place of a record of this role holds when the record is
    /// written, under `form` (`spec/dyn.md` D-46, D-48, D-49; R-A6 (d)).
    /// RELA: 0 for `GLOB_DAT`, symbolic, `COPY` and an IE `TPOFF`
    /// record, `S + A` for `RELATIVE`, the lazy target for `JUMP_SLOT`
    /// (D-49; D-46 as WP-040r states it). `None` where the spec states no
    /// content: every REL role (D-46's REL column is i386's, M4),
    /// `IRELATIVE` (its word is [`Facts::iplt_word`]'s, S-69),
    /// `DTPMOD`, `DTPOFF`, a negated `TPOFF` and `TLSDESC`.
    #[must_use]
    pub const fn place(self, form: RelForm) -> Option<PlaceContent> {
        if matches!(form, RelForm::Rel) {
            return None;
        }
        match self {
            Self::Relative => Some(PlaceContent::SPlusA),
            Self::Symbolic | Self::GlobDat | Self::Copy | Self::TpOff { negated: false } => {
                Some(PlaceContent::Zero)
            }
            Self::JumpSlot => Some(PlaceContent::LazyTarget),
            Self::IRelative
            | Self::DtpMod
            | Self::DtpOff
            | Self::TpOff { negated: true }
            | Self::TlsDesc => None,
        }
    }
}

/// What the place of a dynamic record holds in the output file
/// ([`DynRole::place`]; `spec/dyn.md` D-46, D-49).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlaceContent {
    /// 0.
    Zero,
    /// The record's `S + A` at the link-time address.
    SPlusA,
    /// The `JUMP_SLOT` word's lazy target ([`LazyTarget`]).
    LazyTarget,
}

/// Where a `JUMP_SLOT` word points before it is bound (`spec/dyn.md`
/// D-49; R-A6 (a)), at the link-time address: the loader adds the bias
/// in a PIE.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LazyTarget {
    /// The PLT header (AArch64, i386).
    Plt0,
    /// The entry's own address plus `plus` (x86-64: 6, the `push`).
    Entry { plus: u32 },
}

/// When an architecture's dynamic tag is written (`spec/dyn.md` D-30
/// rows 24-25; R-A6 (c)): the condition, decided by the tag writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TagWhen {
    /// The output has the BTI property (a64 §10.3).
    BtiProperty,
    /// Some `JUMP_SLOT` symbol is `STO_AARCH64_VARIANT_PCS` (a64 §10.3).
    VariantPcsJumpSlot,
}

/// One per-architecture `.dynamic` entry and its condition (`spec/dyn.md`
/// D-30 rows 24-25).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DynTag {
    pub tag: i64,
    pub value: u64,
    pub when: TagWhen,
}

/// How a live relocation uses an undefined weak key (`spec/dyn.md`
/// D-25's table): what decides `WeakDyn` against `Weak0` per key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WeakClass {
    /// A GOT-slot reference.
    Got,
    /// An absolute word of the pointer size in a writable place.
    AbsWord,
    /// A branch that may go through a PLT entry.
    Branch,
    /// A use no dynamic record can fill: a key with one stays `Weak0`
    /// (`Exec`) or is refused `weak-undefined-pc-in-pie` (`Pie`).
    Unfillable,
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

/// `property_merge` for the x86 processor range (§3.13; `spec/x64.md`
/// X-41, `spec/synth.md` S-95), shared by x86-64 and i386: binutils'
/// `GNU_PROPERTY_X86_UINT32_AND` range 0xc0000002..=0xc0007fff (with
/// `FEATURE_1_AND`) is `And`, `UINT32_OR` 0xc0008000..=0xc000ffff (with
/// `ISA_1_NEEDED`, `FEATURE_2_NEEDED`) `Or`, `UINT32_OR_AND`
/// 0xc0010000..=0xc0017fff (with `ISA_1_USED`, `FEATURE_2_USED`) `OrAll`;
/// the legacy `COMPAT_ISA_1_USED` (0xc0000000) is `OrAll` and
/// `COMPAT_ISA_1_NEEDED` (0xc0000001) `Or`, as GNU ld merges them. Any
/// other processor-range type is `None` (dropped with `unknown-property`).
#[must_use]
pub const fn x86_property_merge(pr_type: u32) -> Option<PropMerge> {
    match pr_type {
        // COMPAT_ISA_1_NEEDED and the UINT32_OR range.
        0xc000_0001 | 0xc000_8000..=0xc000_ffff => Some(PropMerge::Or),
        0xc000_0002..=0xc000_7fff => Some(PropMerge::And),
        // COMPAT_ISA_1_USED and the UINT32_OR_AND range.
        0xc000_0000 | 0xc001_0000..=0xc001_7fff => Some(PropMerge::OrAll),
        _ => None,
    }
}

/// How a processor-range GNU property (`0xc0000000..0xdfffffff`) merges
/// over the inputs (§3.13; `spec/synth.md` S-95).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PropMerge {
    /// The AND of every input's value, an input without it counting 0;
    /// omitted when 0.
    And,
    /// The OR of the inputs that have it; present if any has it.
    Or,
    /// The OR of the inputs; present only if every input has it.
    OrAll,
    /// Copied when every input that has it has the same bytes (inputs
    /// without it do not count); refused when two disagree.
    EqualOrAbsent,
}

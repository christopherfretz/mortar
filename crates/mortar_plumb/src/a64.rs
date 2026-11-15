//! AArch64 (`spec/plumb.md` §4.2, §5): the stage-1 relocation table, the
//! fields of AAELF64 "Relocation", the instruction shapes of the specified
//! rewritings, and the veneer emulator.
//!
//! Written from AAELF64 and the Arm A64 instruction encodings alone.

/// The operation of a relocation type (AAELF64 "Relocation operations").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// `R_AARCH64_NONE`: nothing.
    None,
    /// `S + A`.
    Abs,
    /// `S + A - P`.
    Prel,
    /// `Page(S + A) - Page(P)`.
    Page,
    /// `S + A - GOT`.
    GotRel,
    /// `TPREL(S + A)`: the thread-pointer offset of S+A (§4.7).
    TpRel,
    /// `DTPREL(S + A)`: the offset of S+A in its module's TLS block (§4.7).
    DtpRel,
    /// An operation on the GOT entry of a [`Slot`] kind, of a [`Form`]:
    /// `G(GDAT(S+A))`, `G(GTPREL(S+A))`, `G(GTLSDESC(S+A))`,
    /// `G(GTLSIDX(S+A))` and `G(GLDM(S))`.
    Slot(Slot, Form),
    /// `R_AARCH64_TLSDESC_CALL`: marks the `BLR` of a TLS descriptor
    /// sequence; it writes nothing (`R_X86_64_TLSDESC_CALL` too).
    DescCall,
    /// `Z + A`: the symbol's size (x86-64 `SIZE32`, `SIZE64`).
    Size,
    /// `GOT + A - P` (x86-64 `GOTPC32`, `GOTPC64`).
    GotPc,
}

/// What a GOT entry holds (AAELF64 "Relocation operations", §5.5, §4.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Slot {
    /// `GDAT(S+A)`: 8 bytes, the address S+A.
    Addr,
    /// `GTPREL(S+A)`: 8 bytes, the thread-pointer offset of S+A.
    TpRel,
    /// `GTLSDESC(S+A)`: a 16-byte TLS descriptor.
    Desc,
    /// `GTLSIDX(S+A)`: 16 bytes, the module index and the offset of S+A.
    Gd,
    /// `GLDM(S)`: 16 bytes, the module index and 0.
    Ld,
}

/// How a GOT operation uses the entry's address G.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// `G`.
    Abs,
    /// `G - P`.
    Prel,
    /// `Page(G) - Page(P)`.
    Page,
    /// `G - GOT`.
    Off,
    /// `G - Page(GOT)`.
    PageOff,
    /// `G + A - P`: the slot holds S and A is added outside it (x86-64,
    /// `spec/x64.md` X-4).
    PrelA,
    /// `G + A - GOT` (x86-64 `GOT64`).
    OffA,
}

impl Form {
    /// Whether the addend is outside the slot: the slot holds S, not S+A.
    pub const fn addend_outside(self) -> bool {
        matches!(self, Form::PrelA | Form::OffA)
    }
}

impl Op {
    /// Whether the type goes through a GOT entry (§5.5).
    pub const fn is_slot(self) -> bool {
        matches!(self, Op::Slot(..))
    }

    /// Whether the type is one of AAELF64's thread-local types (§4.7).
    pub const fn is_tls(self) -> bool {
        matches!(
            self,
            Op::TpRel
                | Op::DtpRel
                | Op::DescCall
                | Op::Slot(Slot::TpRel | Slot::Desc | Slot::Gd | Slot::Ld, _)
        )
    }
}

/// How the value is written (AAELF64 "Static AArch64 relocations").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// Little-endian data of 8, 4 or 2 bytes: the low bits of X.
    Data(u8),
    /// MOVZ/MOVK/MOVN `imm16` = X[shift+15:shift].
    Movw(u8),
    /// As [`Field::Movw`], and the instruction becomes MOVN with `!X` when
    /// X < 0, MOVZ otherwise (the signed groups).
    MovwSigned(u8),
    /// ADR `immhi:immlo` = X[20:0].
    Adr,
    /// ADRP `immhi:immlo` = X[32:12].
    AdrPage,
    /// `imm12` (bits 21:10) = X[11:scale]: ADD and the LD/ST unsigned-offset forms.
    Lo12(u8),
    /// `imm12` = X[14:3] (the `LO15` GOT forms).
    Lo15,
    /// `imm12` = X[23:12] (the `HI12` TLS forms of `ADD ..., LSL #12`).
    Hi12,
    /// No field: the type marks an instruction and writes nothing
    /// (`TLSDESC_CALL`).
    Insn,
    /// `imm19` (bits 23:5) = X[20:2]: LDR (literal), B.cond, CBZ/CBNZ.
    Imm19,
    /// `imm14` (bits 18:5) = X[15:2]: TBZ/TBNZ.
    Imm14,
    /// `imm26` = X[27:2]: B, BL.
    Imm26,
}

impl Field {
    /// The width of the relocated place in bytes.
    pub const fn width(self) -> u8 {
        match self {
            Field::Data(w) => w,
            _ => 4,
        }
    }

    /// The bits of the place the field occupies (data: all of them).
    pub const fn mask(self) -> u64 {
        match self {
            Field::Data(8) => u64::MAX,
            Field::Data(w) => (1u64 << (8 * w as u32)) - 1,
            Field::Movw(_) => 0xffff << 5,
            Field::MovwSigned(_) => 0xffff << 5 | 0x6000_0000,
            Field::Adr | Field::AdrPage => 0x6000_0000 | 0x7ffff << 5,
            Field::Lo12(_) | Field::Lo15 | Field::Hi12 => 0xfff << 10,
            Field::Insn => 0,
            Field::Imm19 => 0x7ffff << 5,
            Field::Imm14 => 0x3fff << 5,
            Field::Imm26 => 0x3ff_ffff,
        }
    }

    /// The low bits of X the field drops, which must be zero (§4.3).
    pub const fn align_mask(self) -> i64 {
        match self {
            Field::Lo12(s) => (1 << s) - 1,
            Field::Lo15 => 7,
            Field::Imm19 | Field::Imm14 | Field::Imm26 => 3,
            _ => 0,
        }
    }

    /// The place's content after writing X into `place` (§4.3).
    pub fn encode(self, place: u64, x: i64) -> u64 {
        let u = x as u64;
        let ins = |bits: u64| (place & !self.mask()) | (bits & self.mask());
        match self {
            Field::Data(_) => u & self.mask(),
            Field::Movw(s) => ins(((u >> s) & 0xffff) << 5),
            Field::MovwSigned(s) => {
                if x < 0 {
                    ins(((!u >> s) & 0xffff) << 5)
                } else {
                    ins(((u >> s) & 0xffff) << 5 | 0x4000_0000)
                }
            }
            Field::Adr => ins(adr_bits(u)),
            Field::AdrPage => ins(adr_bits(u >> 12)),
            Field::Lo12(s) => ins(((u & 0xfff) >> s) << 10),
            Field::Lo15 => ins(((u & 0x7fff) >> 3) << 10),
            Field::Hi12 => ins(((u >> 12) & 0xfff) << 10),
            Field::Insn => place,
            Field::Imm19 => ins(((u >> 2) & 0x7ffff) << 5),
            Field::Imm14 => ins(((u >> 2) & 0x3fff) << 5),
            Field::Imm26 => ins((u >> 2) & 0x3ff_ffff),
        }
    }
}

fn adr_bits(v: u64) -> u64 {
    (v & 3) << 29 | ((v >> 2) & 0x7ffff) << 5
}

/// The overflow check of a type (AAELF64), on X as a signed 64-bit value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    None,
    /// `0 <= X < 2^n`.
    Unsigned(u8),
    /// `-2^(n-1) <= X < 2^(n-1)`.
    Signed(u8),
    /// `-2^(n-1) <= X < 2^n` (the data types that may hold either).
    Either(u8),
}

impl Check {
    pub fn holds(self, x: i64) -> bool {
        let x = i128::from(x);
        match self {
            Check::None => true,
            Check::Unsigned(n) => (0..1i128 << n).contains(&x),
            Check::Signed(n) => (-(1i128 << (n - 1))..1i128 << (n - 1)).contains(&x),
            Check::Either(n) => (-(1i128 << (n - 1))..1i128 << n).contains(&x),
        }
    }

    /// The check as the spec's table writes it.
    pub fn describe(self) -> String {
        match self {
            Check::None => "none".into(),
            Check::Unsigned(n) => format!("0 <= X < 2^{n}"),
            Check::Signed(n) => format!("-2^{} <= X < 2^{}", n - 1, n - 1),
            Check::Either(n) => format!("-2^{} <= X < 2^{n}", n - 1),
        }
    }
}

/// One row of the stage-1 table (§4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Def {
    pub r_type: u32,
    pub name: &'static str,
    pub op: Op,
    pub field: Field,
    pub check: Check,
}

const fn d(r_type: u32, name: &'static str, op: Op, field: Field, check: Check) -> Def {
    Def { r_type, name, op, field, check }
}

use Form as F;
use Slot as G;

/// The table (§4.2 and §4.7), in type order.
pub const TABLE: &[Def] = &[
    d(0, "R_AARCH64_NONE", Op::None, Field::Data(8), Check::None),
    d(256, "R_AARCH64_NULL", Op::None, Field::Data(8), Check::None),
    d(257, "R_AARCH64_ABS64", Op::Abs, Field::Data(8), Check::None),
    d(258, "R_AARCH64_ABS32", Op::Abs, Field::Data(4), Check::Either(32)),
    d(259, "R_AARCH64_ABS16", Op::Abs, Field::Data(2), Check::Either(16)),
    d(260, "R_AARCH64_PREL64", Op::Prel, Field::Data(8), Check::None),
    d(261, "R_AARCH64_PREL32", Op::Prel, Field::Data(4), Check::Either(32)),
    d(262, "R_AARCH64_PREL16", Op::Prel, Field::Data(2), Check::Either(16)),
    d(263, "R_AARCH64_MOVW_UABS_G0", Op::Abs, Field::Movw(0), Check::Unsigned(16)),
    d(264, "R_AARCH64_MOVW_UABS_G0_NC", Op::Abs, Field::Movw(0), Check::None),
    d(265, "R_AARCH64_MOVW_UABS_G1", Op::Abs, Field::Movw(16), Check::Unsigned(32)),
    d(266, "R_AARCH64_MOVW_UABS_G1_NC", Op::Abs, Field::Movw(16), Check::None),
    d(267, "R_AARCH64_MOVW_UABS_G2", Op::Abs, Field::Movw(32), Check::Unsigned(48)),
    d(268, "R_AARCH64_MOVW_UABS_G2_NC", Op::Abs, Field::Movw(32), Check::None),
    d(269, "R_AARCH64_MOVW_UABS_G3", Op::Abs, Field::Movw(48), Check::None),
    d(270, "R_AARCH64_MOVW_SABS_G0", Op::Abs, Field::MovwSigned(0), Check::Signed(17)),
    d(271, "R_AARCH64_MOVW_SABS_G1", Op::Abs, Field::MovwSigned(16), Check::Signed(33)),
    d(272, "R_AARCH64_MOVW_SABS_G2", Op::Abs, Field::MovwSigned(32), Check::Signed(49)),
    d(273, "R_AARCH64_LD_PREL_LO19", Op::Prel, Field::Imm19, Check::Signed(21)),
    d(274, "R_AARCH64_ADR_PREL_LO21", Op::Prel, Field::Adr, Check::Signed(21)),
    d(275, "R_AARCH64_ADR_PREL_PG_HI21", Op::Page, Field::AdrPage, Check::Signed(33)),
    d(276, "R_AARCH64_ADR_PREL_PG_HI21_NC", Op::Page, Field::AdrPage, Check::None),
    d(277, "R_AARCH64_ADD_ABS_LO12_NC", Op::Abs, Field::Lo12(0), Check::None),
    d(278, "R_AARCH64_LDST8_ABS_LO12_NC", Op::Abs, Field::Lo12(0), Check::None),
    d(279, "R_AARCH64_TSTBR14", Op::Prel, Field::Imm14, Check::Signed(16)),
    d(280, "R_AARCH64_CONDBR19", Op::Prel, Field::Imm19, Check::Signed(21)),
    d(282, "R_AARCH64_JUMP26", Op::Prel, Field::Imm26, Check::Signed(28)),
    d(283, "R_AARCH64_CALL26", Op::Prel, Field::Imm26, Check::Signed(28)),
    d(284, "R_AARCH64_LDST16_ABS_LO12_NC", Op::Abs, Field::Lo12(1), Check::None),
    d(285, "R_AARCH64_LDST32_ABS_LO12_NC", Op::Abs, Field::Lo12(2), Check::None),
    d(286, "R_AARCH64_LDST64_ABS_LO12_NC", Op::Abs, Field::Lo12(3), Check::None),
    d(287, "R_AARCH64_MOVW_PREL_G0", Op::Prel, Field::MovwSigned(0), Check::Signed(17)),
    d(288, "R_AARCH64_MOVW_PREL_G0_NC", Op::Prel, Field::Movw(0), Check::None),
    d(289, "R_AARCH64_MOVW_PREL_G1", Op::Prel, Field::MovwSigned(16), Check::Signed(33)),
    d(290, "R_AARCH64_MOVW_PREL_G1_NC", Op::Prel, Field::Movw(16), Check::None),
    d(291, "R_AARCH64_MOVW_PREL_G2", Op::Prel, Field::MovwSigned(32), Check::Signed(49)),
    d(292, "R_AARCH64_MOVW_PREL_G2_NC", Op::Prel, Field::Movw(32), Check::None),
    d(293, "R_AARCH64_MOVW_PREL_G3", Op::Prel, Field::Movw(48), Check::None),
    d(299, "R_AARCH64_LDST128_ABS_LO12_NC", Op::Abs, Field::Lo12(4), Check::None),
    d(307, "R_AARCH64_GOTREL64", Op::GotRel, Field::Data(8), Check::None),
    d(308, "R_AARCH64_GOTREL32", Op::GotRel, Field::Data(4), Check::Signed(32)),
    d(309, "R_AARCH64_GOT_LD_PREL19", Op::Slot(G::Addr, F::Prel), Field::Imm19, Check::Signed(21)),
    d(
        310,
        "R_AARCH64_LD64_GOTOFF_LO15",
        Op::Slot(G::Addr, F::Off),
        Field::Lo15,
        Check::Unsigned(15),
    ),
    d(311, "R_AARCH64_ADR_GOT_PAGE", Op::Slot(G::Addr, F::Page), Field::AdrPage, Check::Signed(33)),
    d(312, "R_AARCH64_LD64_GOT_LO12_NC", Op::Slot(G::Addr, F::Abs), Field::Lo12(3), Check::None),
    d(
        313,
        "R_AARCH64_LD64_GOTPAGE_LO15",
        Op::Slot(G::Addr, F::PageOff),
        Field::Lo15,
        Check::Unsigned(15),
    ),
    d(314, "R_AARCH64_PLT32", Op::Prel, Field::Data(4), Check::Signed(32)),
    // §4.7: the thread-local types, AAELF64 "Relocations for thread-local storage".
    d(
        513,
        "R_AARCH64_TLSGD_ADR_PAGE21",
        Op::Slot(G::Gd, F::Page),
        Field::AdrPage,
        Check::Signed(33),
    ),
    d(514, "R_AARCH64_TLSGD_ADD_LO12_NC", Op::Slot(G::Gd, F::Abs), Field::Lo12(0), Check::None),
    d(
        518,
        "R_AARCH64_TLSLD_ADR_PAGE21",
        Op::Slot(G::Ld, F::Page),
        Field::AdrPage,
        Check::Signed(33),
    ),
    d(519, "R_AARCH64_TLSLD_ADD_LO12_NC", Op::Slot(G::Ld, F::Abs), Field::Lo12(0), Check::None),
    d(523, "R_AARCH64_TLSLD_MOVW_DTPREL_G2", Op::DtpRel, Field::MovwSigned(32), Check::Signed(49)),
    d(524, "R_AARCH64_TLSLD_MOVW_DTPREL_G1", Op::DtpRel, Field::MovwSigned(16), Check::Signed(33)),
    d(525, "R_AARCH64_TLSLD_MOVW_DTPREL_G1_NC", Op::DtpRel, Field::Movw(16), Check::None),
    d(526, "R_AARCH64_TLSLD_MOVW_DTPREL_G0", Op::DtpRel, Field::MovwSigned(0), Check::Signed(17)),
    d(527, "R_AARCH64_TLSLD_MOVW_DTPREL_G0_NC", Op::DtpRel, Field::Movw(0), Check::None),
    d(528, "R_AARCH64_TLSLD_ADD_DTPREL_HI12", Op::DtpRel, Field::Hi12, Check::Unsigned(24)),
    d(529, "R_AARCH64_TLSLD_ADD_DTPREL_LO12", Op::DtpRel, Field::Lo12(0), Check::Unsigned(12)),
    d(530, "R_AARCH64_TLSLD_ADD_DTPREL_LO12_NC", Op::DtpRel, Field::Lo12(0), Check::None),
    d(531, "R_AARCH64_TLSLD_LDST8_DTPREL_LO12", Op::DtpRel, Field::Lo12(0), Check::Unsigned(12)),
    d(532, "R_AARCH64_TLSLD_LDST8_DTPREL_LO12_NC", Op::DtpRel, Field::Lo12(0), Check::None),
    d(533, "R_AARCH64_TLSLD_LDST16_DTPREL_LO12", Op::DtpRel, Field::Lo12(1), Check::Unsigned(12)),
    d(534, "R_AARCH64_TLSLD_LDST16_DTPREL_LO12_NC", Op::DtpRel, Field::Lo12(1), Check::None),
    d(535, "R_AARCH64_TLSLD_LDST32_DTPREL_LO12", Op::DtpRel, Field::Lo12(2), Check::Unsigned(12)),
    d(536, "R_AARCH64_TLSLD_LDST32_DTPREL_LO12_NC", Op::DtpRel, Field::Lo12(2), Check::None),
    d(537, "R_AARCH64_TLSLD_LDST64_DTPREL_LO12", Op::DtpRel, Field::Lo12(3), Check::Unsigned(12)),
    d(538, "R_AARCH64_TLSLD_LDST64_DTPREL_LO12_NC", Op::DtpRel, Field::Lo12(3), Check::None),
    d(
        539,
        "R_AARCH64_TLSIE_MOVW_GOTTPREL_G1",
        Op::Slot(G::TpRel, F::Off),
        Field::MovwSigned(16),
        Check::Signed(33),
    ),
    d(
        540,
        "R_AARCH64_TLSIE_MOVW_GOTTPREL_G0_NC",
        Op::Slot(G::TpRel, F::Off),
        Field::Movw(0),
        Check::None,
    ),
    d(
        541,
        "R_AARCH64_TLSIE_ADR_GOTTPREL_PAGE21",
        Op::Slot(G::TpRel, F::Page),
        Field::AdrPage,
        Check::Signed(33),
    ),
    d(
        542,
        "R_AARCH64_TLSIE_LD64_GOTTPREL_LO12_NC",
        Op::Slot(G::TpRel, F::Abs),
        Field::Lo12(3),
        Check::None,
    ),
    d(
        543,
        "R_AARCH64_TLSIE_LD_GOTTPREL_PREL19",
        Op::Slot(G::TpRel, F::Prel),
        Field::Imm19,
        Check::Signed(21),
    ),
    d(544, "R_AARCH64_TLSLE_MOVW_TPREL_G2", Op::TpRel, Field::MovwSigned(32), Check::Signed(49)),
    d(545, "R_AARCH64_TLSLE_MOVW_TPREL_G1", Op::TpRel, Field::MovwSigned(16), Check::Signed(33)),
    d(546, "R_AARCH64_TLSLE_MOVW_TPREL_G1_NC", Op::TpRel, Field::Movw(16), Check::None),
    d(547, "R_AARCH64_TLSLE_MOVW_TPREL_G0", Op::TpRel, Field::MovwSigned(0), Check::Signed(17)),
    d(548, "R_AARCH64_TLSLE_MOVW_TPREL_G0_NC", Op::TpRel, Field::Movw(0), Check::None),
    d(549, "R_AARCH64_TLSLE_ADD_TPREL_HI12", Op::TpRel, Field::Hi12, Check::Unsigned(24)),
    d(550, "R_AARCH64_TLSLE_ADD_TPREL_LO12", Op::TpRel, Field::Lo12(0), Check::Unsigned(12)),
    d(551, "R_AARCH64_TLSLE_ADD_TPREL_LO12_NC", Op::TpRel, Field::Lo12(0), Check::None),
    d(552, "R_AARCH64_TLSLE_LDST8_TPREL_LO12", Op::TpRel, Field::Lo12(0), Check::Unsigned(12)),
    d(553, "R_AARCH64_TLSLE_LDST8_TPREL_LO12_NC", Op::TpRel, Field::Lo12(0), Check::None),
    d(554, "R_AARCH64_TLSLE_LDST16_TPREL_LO12", Op::TpRel, Field::Lo12(1), Check::Unsigned(12)),
    d(555, "R_AARCH64_TLSLE_LDST16_TPREL_LO12_NC", Op::TpRel, Field::Lo12(1), Check::None),
    d(556, "R_AARCH64_TLSLE_LDST32_TPREL_LO12", Op::TpRel, Field::Lo12(2), Check::Unsigned(12)),
    d(557, "R_AARCH64_TLSLE_LDST32_TPREL_LO12_NC", Op::TpRel, Field::Lo12(2), Check::None),
    d(558, "R_AARCH64_TLSLE_LDST64_TPREL_LO12", Op::TpRel, Field::Lo12(3), Check::Unsigned(12)),
    d(559, "R_AARCH64_TLSLE_LDST64_TPREL_LO12_NC", Op::TpRel, Field::Lo12(3), Check::None),
    d(
        562,
        "R_AARCH64_TLSDESC_ADR_PAGE21",
        Op::Slot(G::Desc, F::Page),
        Field::AdrPage,
        Check::Signed(33),
    ),
    d(563, "R_AARCH64_TLSDESC_LD64_LO12", Op::Slot(G::Desc, F::Abs), Field::Lo12(3), Check::None),
    d(564, "R_AARCH64_TLSDESC_ADD_LO12", Op::Slot(G::Desc, F::Abs), Field::Lo12(0), Check::None),
    d(569, "R_AARCH64_TLSDESC_CALL", Op::DescCall, Field::Insn, Check::None),
    d(570, "R_AARCH64_TLSLE_LDST128_TPREL_LO12", Op::TpRel, Field::Lo12(4), Check::Unsigned(12)),
    d(571, "R_AARCH64_TLSLE_LDST128_TPREL_LO12_NC", Op::TpRel, Field::Lo12(4), Check::None),
    d(572, "R_AARCH64_TLSLD_LDST128_DTPREL_LO12", Op::DtpRel, Field::Lo12(4), Check::Unsigned(12)),
    d(573, "R_AARCH64_TLSLD_LDST128_DTPREL_LO12_NC", Op::DtpRel, Field::Lo12(4), Check::None),
];

/// The row of `r_type`, if the table has it.
pub fn def(r_type: u32) -> Option<&'static Def> {
    TABLE.binary_search_by_key(&r_type, |d| d.r_type).ok().map(|i| &TABLE[i])
}

/// Whether `r_type` is a TLS type (AAELF64 "Thread-local storage"):
/// 512 to 573, and the dynamic TLS types 1028 to 1031.
pub fn is_tls(r_type: u32) -> bool {
    (512..=573).contains(&r_type) || (1028..=1031).contains(&r_type)
}

/// The width of a type the table does not have: every AArch64 static TLS
/// type is an instruction; the others are taken as 8-byte data (§6.3).
pub fn unchecked_width(r_type: u32) -> u8 {
    if (512..=573).contains(&r_type) { 4 } else { 8 }
}

/// `Page(x)` (AAELF64): x with the low 12 bits clear.
pub const fn page(x: u64) -> u64 {
    x & !0xfff
}

fn sext(v: u64, bits: u32) -> i64 {
    let shift = 64 - bits;
    ((v << shift) as i64) >> shift
}

// ---- instruction shapes (Arm A64 encodings) ----

pub const NOP: u32 = 0xd503_201f;

/// `ADRP Xd`: the destination and the page it computes at `pc`.
pub fn adrp(insn: u32, pc: u64) -> Option<(u32, u64)> {
    (insn & 0x9f00_0000 == 0x9000_0000).then(|| {
        let imm = u64::from((insn >> 29) & 3) | u64::from((insn >> 5) & 0x7ffff) << 2;
        (insn & 31, page(pc).wrapping_add((sext(imm, 21) << 12) as u64))
    })
}

/// `ADR Xd`: the destination and the address it computes at `pc`.
pub fn adr(insn: u32, pc: u64) -> Option<(u32, u64)> {
    (insn & 0x9f00_0000 == 0x1000_0000).then(|| {
        let imm = u64::from((insn >> 29) & 3) | u64::from((insn >> 5) & 0x7ffff) << 2;
        (insn & 31, pc.wrapping_add(sext(imm, 21) as u64))
    })
}

/// `ADD Xd, Xn, #imm` (64-bit, no shift): `(d, n, imm)`.
pub fn add_imm(insn: u32) -> Option<(u32, u32, u64)> {
    (insn & 0xffc0_0000 == 0x9100_0000)
        .then(|| (insn & 31, (insn >> 5) & 31, u64::from((insn >> 10) & 0xfff)))
}

/// `ADD Xd, Xn, #imm, LSL #12`: `(d, n, imm << 12)`.
fn add_imm_lsl12(insn: u32) -> Option<(u32, u32, u64)> {
    (insn & 0xffc0_0000 == 0x9140_0000)
        .then(|| (insn & 31, (insn >> 5) & 31, u64::from((insn >> 10) & 0xfff) << 12))
}

/// `LDR Xt, [Xn, #imm]` (64-bit, unsigned offset): `(t, n, imm)`.
pub fn ldr64_imm(insn: u32) -> Option<(u32, u32, u64)> {
    (insn & 0xffc0_0000 == 0xf940_0000)
        .then(|| (insn & 31, (insn >> 5) & 31, u64::from((insn >> 10) & 0xfff) << 3))
}

/// `LDR Xt, label` (64-bit literal): `(t, address)`.
fn ldr64_lit(insn: u32, pc: u64) -> Option<(u32, u64)> {
    (insn & 0xff00_0000 == 0x5800_0000).then(|| {
        (insn & 31, pc.wrapping_add((sext(u64::from((insn >> 5) & 0x7ffff), 19) << 2) as u64))
    })
}

/// `B` (`link` false) or `BL` (`link` true) and its target.
pub fn branch(insn: u32, pc: u64) -> Option<(bool, u64)> {
    let link = match insn & 0xfc00_0000 {
        0x1400_0000 => false,
        0x9400_0000 => true,
        _ => return None,
    };
    Some((link, pc.wrapping_add((sext(u64::from(insn & 0x3ff_ffff), 26) << 2) as u64)))
}

/// `BR Xn`.
fn br(insn: u32) -> Option<u32> {
    (insn & 0xffff_fc1f == 0xd61f_0000).then_some((insn >> 5) & 31)
}

/// `BTI` with any target: the one hint a veneer may start with.
fn bti(insn: u32) -> bool {
    insn & 0xffff_ff3f == 0xd503_241f
}

/// `MOVZ`/`MOVK`/`MOVN` (64-bit): `(opc, hw, imm16, d)`.
fn movw(insn: u32) -> Option<(u32, u32, u64, u32)> {
    let opc = (insn >> 29) & 3;
    (insn & 0x9f80_0000 == 0x9280_0000 && opc != 1)
        .then(|| (opc, (insn >> 21) & 3, u64::from((insn >> 5) & 0xffff), insn & 31))
}

/// A load or store with an unsigned 12-bit offset (the form erratum
/// 843419's patch moves): any size, integer or SIMD&FP.
pub fn is_ldst_unsigned_imm(insn: u32) -> bool {
    insn & 0x3b00_0000 == 0x3900_0000
}

/// Whether an instruction at `addr` may be the last of an erratum 843419
/// sequence, which starts with an `ADRP` at an address ending in `0xff8`
/// or `0xffc` and ends 8 or 12 bytes later (§5.3).
pub fn erratum_843419_site(addr: u64) -> bool {
    matches!(addr & 0xfff, 0x000 | 0x004 | 0x008)
}

/// Whether an `ADRP` at `addr` may start an erratum 843419 sequence.
pub fn erratum_843419_adrp(addr: u64) -> bool {
    matches!(addr & 0xfff, 0xff8 | 0xffc)
}

/// The decoded field of an output place: what the place says X is, given
/// the place's address (§5.6). For an `AdrPage` field the value is the
/// page; for a `Lo12`/`Lo15` field only the bits it holds.
pub fn decode(field: Field, word: u64, p: u64) -> i64 {
    let w32 = word as u32;
    match field {
        Field::Data(8) => word as i64,
        Field::Data(0) => 0,
        Field::Data(w) => sext(word, 8 * u32::from(w)),
        Field::Movw(s) | Field::MovwSigned(s) => (u64::from((w32 >> 5) & 0xffff) << s) as i64,
        Field::Adr => adr(w32 & !0x8000_0000, p).map_or(0, |(_, t)| t.wrapping_sub(p) as i64),
        Field::AdrPage => {
            let imm = u64::from((w32 >> 29) & 3) | u64::from((w32 >> 5) & 0x7ffff) << 2;
            sext(imm, 21) << 12
        }
        Field::Lo12(s) => (u64::from((w32 >> 10) & 0xfff) << s) as i64,
        Field::Lo15 => (u64::from((w32 >> 10) & 0xfff) << 3) as i64,
        Field::Hi12 => (u64::from((w32 >> 10) & 0xfff) << 12) as i64,
        Field::Insn => 0,
        Field::Imm19 => sext(u64::from((w32 >> 5) & 0x7ffff), 19) << 2,
        Field::Imm14 => sext(u64::from((w32 >> 5) & 0x3fff), 14) << 2,
        Field::Imm26 => sext(u64::from(w32 & 0x3ff_ffff), 26) << 2,
    }
}

/// `MOVZ Xd, #imm16, LSL #(16*hw)` (64-bit).
pub const fn movz(rd: u32, imm16: u64, hw: u32) -> u32 {
    0xd280_0000 | hw << 21 | ((imm16 & 0xffff) as u32) << 5 | rd
}

/// `MOVK Xd, #imm16, LSL #(16*hw)` (64-bit).
pub const fn movk(rd: u32, imm16: u64, hw: u32) -> u32 {
    0xf280_0000 | hw << 21 | ((imm16 & 0xffff) as u32) << 5 | rd
}

/// `MRS Xt, TPIDR_EL0`.
pub const fn mrs_tpidr_el0(rt: u32) -> u32 {
    0xd53b_d040 | rt
}

/// `ADD Xd, Xn, Xm` (shifted register, no shift).
pub const fn add_reg(rd: u32, rn: u32, rm: u32) -> u32 {
    0x8b00_0000 | rm << 16 | rn << 5 | rd
}

/// `ADD Xd, Xn, #imm` for `imm` < 2^24 with the low or the high 12 bits
/// zero: `ADD` (immediate), unshifted or with `LSL #12`.
pub fn add_imm_any(rd: u32, rn: u32, imm: u64) -> Option<u32> {
    if imm < 0x1000 {
        Some(0x9100_0000 | (imm as u32) << 10 | rn << 5 | rd)
    } else if imm.trailing_zeros() >= 12 && imm < 0x100_0000 {
        Some(0x9140_0000 | ((imm >> 12) as u32) << 10 | rn << 5 | rd)
    } else {
        None
    }
}

/// `BLR Xn`: the register.
pub fn blr(insn: u32) -> Option<u32> {
    (insn & 0xffff_fc1f == 0xd63f_0000).then_some((insn >> 5) & 31)
}

/// The access size of a load or store with an unsigned 12-bit offset, as
/// the power of two its `imm12` is scaled by (Arm A64 "Load/store register
/// (unsigned immediate)"): `size`, or 4 for a 128-bit SIMD&FP access.
pub fn ldst_scale(insn: u32) -> Option<u8> {
    if !is_ldst_unsigned_imm(insn) {
        return None;
    }
    let size = (insn >> 30) as u8;
    let simd = insn & (1 << 26) != 0;
    let opc_hi = insn & (1 << 23) != 0;
    Some(if simd && size == 0 && opc_hi { 4 } else { size })
}

/// What the instruction at a place of type `r_type` must be (§4.3, R8):
/// the load/store types name the access size their `imm12` is scaled by,
/// and the `LD64` GOT forms a 64-bit `LDR`. `None`: any instruction (or
/// data).
pub fn insn_class_ok(r_type: u32, insn: u32) -> bool {
    let scale = |want: u8| ldst_scale(insn) == Some(want);
    match r_type {
        // LDST8/16/32/64/128_ABS_LO12_NC and the TLS LDST forms
        278 | 531 | 532 | 552 | 553 => scale(0),
        284 | 533 | 534 | 554 | 555 => scale(1),
        285 | 535 | 536 | 556 | 557 => scale(2),
        286 | 537 | 538 | 558 | 559 => scale(3),
        299 | 570..=573 => scale(4),
        // LD64_GOTOFF_LO15, LD64_GOT_LO12_NC, LD64_GOTPAGE_LO15,
        // TLSIE_LD64_GOTTPREL_LO12_NC, TLSDESC_LD64_LO12
        310 | 312 | 313 | 542 | 563 => ldr64_imm(insn).is_some(),
        _ => true,
    }
}

/// `ADD Xd, Xn, Xm` (shifted register, `LSL #0`): (rd, rn, rm).
fn add_xreg(insn: u32) -> Option<(u32, u32, u32)> {
    (insn & 0xffe0_fc00 == 0x8b00_0000).then_some((insn & 31, (insn >> 5) & 31, (insn >> 16) & 31))
}

/// The output memory a veneer reads.
pub trait Memory {
    /// The instruction at `addr`, if it is in an executable segment.
    fn insn(&self, addr: u64) -> Option<u32>;
    /// The 64-bit little-endian word at `addr`, if mapped.
    fn u64_at(&self, addr: u64) -> Option<u64>;
}

/// The longest veneer the emulator follows (§5.4).
pub const VENEER_MAX: usize = 8;

/// Runs the instructions at `start` as a veneer (§5.4): a leading `BTI`,
/// `ADRP`, `ADR`, `ADD` (immediate, or x16/x17 registers), `LDR` (literal, 64-bit), `MOVZ`/
/// `MOVK`/`MOVN` writing x16 or x17, ending in `BR x16`/`BR x17` or a `B`.
/// Returns where control goes, or `None` if the code is not a veneer.
pub fn run_veneer(mem: &dyn Memory, start: u64) -> Option<u64> {
    let mut regs: [Option<u64>; 2] = [None, None];
    let reg = |r: u32| (r == 16 || r == 17).then(|| (r - 16) as usize);
    let mut pc = start;
    for step in 0..VENEER_MAX {
        let insn = mem.insn(pc)?;
        if step == 0 && bti(insn) {
        } else if let Some((rd, v)) = adrp(insn, pc).or_else(|| adr(insn, pc)) {
            regs[reg(rd)?] = Some(v);
        } else if let Some((rd, rn, imm)) = add_imm(insn).or_else(|| add_imm_lsl12(insn)) {
            let v = regs[reg(rn)?]?;
            regs[reg(rd)?] = Some(v.wrapping_add(imm));
        } else if let Some((rd, rn, rm)) = add_xreg(insn) {
            // GNU ld's long veneer: ADR x17, .; ADD x16, x16, x17
            // (spec/a64.md §8.7).
            let v = regs[reg(rn)?]?.wrapping_add(regs[reg(rm)?]?);
            regs[reg(rd)?] = Some(v);
        } else if let Some((rt, at)) = ldr64_lit(insn, pc) {
            let v = mem.u64_at(at)?;
            regs[reg(rt)?] = Some(v);
        } else if let Some((opc, hw, imm, rd)) = movw(insn) {
            let r = reg(rd)?;
            let shift = 16 * hw;
            regs[r] = Some(match opc {
                0 => !(imm << shift),
                2 => imm << shift,
                _ => (regs[r]? & !(0xffff << shift)) | imm << shift,
            });
        } else if let Some(rn) = br(insn) {
            return regs[reg(rn)?];
        } else if let Some((false, target)) = branch(insn, pc) {
            return Some(target);
        } else {
            return None;
        }
        pc = pc.wrapping_add(4);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec/plumb.md §4.2: the table is sorted, unique, and names its
    // types as mortar_elf does.
    #[test]
    fn s4_2_table_is_sorted_and_named_as_the_reader_names_it() {
        for w in TABLE.windows(2) {
            assert!(w[0].r_type < w[1].r_type);
        }
        for d in TABLE {
            assert_eq!(
                mortar_elf::reloc::name(mortar_elf::consts::EM_AARCH64, d.r_type),
                Some(d.name)
            );
            assert_eq!(def(d.r_type), Some(d));
        }
        assert!(def(281).is_none());
    }

    // §4.3: encodings of each field shape (Arm A64 encodings).
    #[test]
    fn s4_3_fields_encode_as_the_instruction_set_says() {
        // adr x6, .+0xfffe0 (immhi = 0x3fff8, immlo = 0) from `adr x6, .`
        assert_eq!(Field::Adr.encode(0x1000_0006, 0xf_ffe0), 0x107f_ff06);
        assert_eq!(decode(Field::Adr, 0x107f_ff06, 0), 0xf_ffe0);
        assert_eq!(decode(Field::Adr, Field::Adr.encode(0x1000_0006, -3), 0), -3);
        // adrp x0, +0x1f000 pages.
        let w = Field::AdrPage.encode(0x9000_0000, 0x1f000);
        assert_eq!(adrp(w as u32, 0x40_0000), Some((0, 0x41_f000)));
        assert_eq!(decode(Field::AdrPage, w, 0), 0x1f000);
        // add x4, x4, #0x260
        assert_eq!(Field::Lo12(0).encode(0x9100_0084, 0x43_0260), 0x9109_8084);
        // ldr x0, [x0, #4048] = imm12 506
        assert_eq!(Field::Lo12(3).encode(0xf940_0000, 0x41_ffd0), 0xf947_e800);
        // bl .+0x10
        assert_eq!(Field::Imm26.encode(0x9400_0000, 0x10), 0x9400_0004);
        assert_eq!(Field::Imm26.encode(0x9400_0000, -4), 0x97ff_ffff);
        assert_eq!(decode(Field::Imm26, 0x97ff_ffff, 0), -4);
        // movz x0, #0x1234, lsl #16 ; movn when negative.
        assert_eq!(Field::Movw(16).encode(0xd2a0_0000, 0x1234_0000), 0xd2a2_4680);
        assert_eq!(Field::MovwSigned(0).encode(0xd280_0000, -2), 0x9280_0020);
        assert_eq!(Field::MovwSigned(0).encode(0x9280_0000, 5), 0xd280_00a0);
        assert_eq!(Field::Data(4).encode(0, -1), 0xffff_ffff);
        assert_eq!(decode(Field::Data(4), 0xffff_ffff, 0), -1);
        assert_eq!(Field::Imm14.encode(0x3600_0000, -8), 0x3607_ffc0);
        assert_eq!(Field::Imm19.encode(0x5400_0000, 0x10), 0x5400_0080);
        assert_eq!(Field::Lo15.encode(0xf940_0000, 0x7ff8), 0xf97f_fc00);
    }

    // §4.3: the checks of AAELF64.
    #[test]
    fn s4_3_checks() {
        assert!(Check::Either(32).holds(-(1 << 31)) && Check::Either(32).holds((1 << 32) - 1));
        assert!(!Check::Either(32).holds(1 << 32) && !Check::Either(32).holds(-(1 << 31) - 1));
        assert!(Check::Signed(28).holds((1 << 27) - 4) && !Check::Signed(28).holds(1 << 27));
        assert!(Check::Unsigned(16).holds(0xffff) && !Check::Unsigned(16).holds(-1));
        assert!(Check::None.holds(i64::MIN));
        assert_eq!(Check::Signed(21).describe(), "-2^20 <= X < 2^20");
    }

    struct Mem(Vec<(u64, u32)>, Vec<(u64, u64)>);
    impl Memory for Mem {
        fn insn(&self, a: u64) -> Option<u32> {
            self.0.iter().find(|w| w.0 == a).map(|w| w.1)
        }
        fn u64_at(&self, a: u64) -> Option<u64> {
            self.1.iter().find(|w| w.0 == a).map(|w| w.1)
        }
    }

    // §5.4: the three veneer shapes the reference linkers emit.
    #[test]
    fn s5_4_veneers_are_followed() {
        // GNU ld / mold: adrp x16; add x16, x16, #lo; br x16.
        let gnu = Mem(
            vec![
                (0x0040_00c8, 0x900f_e010),
                (0x0040_00cc, 0x9100_0210),
                (0x0040_00d0, 0xd61f_0200),
            ],
            vec![],
        );
        assert_eq!(run_veneer(&gnu, 0x0040_00c8), Some(0x2000_0000));
        // lld: ldr x16, .+8; br x16; .xword target.
        let lld = Mem(
            vec![(0x0040_0010, 0x5800_0050), (0x0040_0014, 0xd61f_0200)],
            vec![(0x0040_0018, 0x2000_0000)],
        );
        assert_eq!(run_veneer(&lld, 0x0040_0010), Some(0x2000_0000));
        // Writing another register is not a veneer.
        let bad = Mem(vec![(0x0, 0x9000_0000), (0x4, 0xd61f_0000)], vec![]);
        assert_eq!(run_veneer(&bad, 0), None);
        // movz/movk chain.
        let mov =
            Mem(vec![(0, 0xd2a4_0010), (4, 0xf280_0010 | 0x10 << 5), (8, 0xd61f_0200)], vec![]);
        assert_eq!(run_veneer(&mov, 0), Some(0x2000_0010));
        // GNU ld's long veneer (a64.md §8.7): ldr x16, .+16; adr x17, .;
        // add x16, x16, x17; br x16; .xword target - (the adr's place).
        let long = Mem(
            vec![
                (0x1000, 0x5800_0090),
                (0x1004, 0x1000_0011),
                (0x1008, 0x8b11_0210),
                (0x100c, 0xd61f_0200),
            ],
            vec![(0x1010, 0x1000_0000)],
        );
        assert_eq!(run_veneer(&long, 0x1000), Some(0x1000_1004));
        // A register other than x16/x17 is not followed.
        let other = Mem(vec![(0, 0x1000_0011), (4, 0x8b00_0230), (8, 0xd61f_0200)], vec![]);
        assert_eq!(run_veneer(&other, 0), None);
    }
}

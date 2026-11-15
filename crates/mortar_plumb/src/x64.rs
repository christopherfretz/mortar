//! The x86-64 relocation table (`spec/plumb.md` §4.5, from `spec/x64.md`
//! §6.1 and the psABI's "Relocation Types") and the byte forms of the
//! rewritings a linker may perform (§5.9, from `spec/x64.md` §8 and §8.8).
//!
//! Every field is little-endian data at P (`spec/x64.md` §3.2). The GOT
//! forms add A outside the slot, which holds S (X-4).

use crate::a64::Form as F;
use crate::a64::{Check, Def, Field, Op, Slot};
use Slot as G;

pub const NONE: u32 = 0;
pub const R64: u32 = 1;
pub const PC32: u32 = 2;
pub const PLT32: u32 = 4;
pub const GOTPCREL: u32 = 9;
pub const DTPOFF64: u32 = 17;
pub const TPOFF64: u32 = 18;
pub const TLSGD: u32 = 19;
pub const TLSLD: u32 = 20;
pub const DTPOFF32: u32 = 21;
pub const GOTTPOFF: u32 = 22;
pub const TPOFF32: u32 = 23;
pub const PC64: u32 = 24;
pub const GOTOFF64: u32 = 25;
pub const PLTOFF64: u32 = 31;
pub const GOTPC32_TLSDESC: u32 = 34;
pub const TLSDESC_CALL: u32 = 35;
pub const GOTPCRELX: u32 = 41;
pub const REX_GOTPCRELX: u32 = 42;

const fn d(r_type: u32, name: &'static str, op: Op, field: Field, check: Check) -> Def {
    Def { r_type, name, op, field, check }
}

const D8: Field = Field::Data(8);
const D4: Field = Field::Data(4);
const S32: Check = Check::Signed(32);

/// The table, in type order: `spec/x64.md` §6.1's M1 rows. `PLTOFF64` is
/// `L + A − GOT`: [`Op::GotRel`] with S the IPLT entry when there is one.
/// The DTPOFF rows are [`Op::DtpRel`]; X-46 decides per section and
/// linker which offset they hold.
pub const TABLE: &[Def] = &[
    d(1, "R_X86_64_64", Op::Abs, D8, Check::None),
    d(2, "R_X86_64_PC32", Op::Prel, D4, S32),
    d(4, "R_X86_64_PLT32", Op::Prel, D4, S32),
    d(9, "R_X86_64_GOTPCREL", Op::Slot(G::Addr, F::PrelA), D4, S32),
    d(10, "R_X86_64_32", Op::Abs, D4, Check::Unsigned(32)),
    d(11, "R_X86_64_32S", Op::Abs, D4, S32),
    d(12, "R_X86_64_16", Op::Abs, Field::Data(2), Check::Either(16)),
    d(13, "R_X86_64_PC16", Op::Prel, Field::Data(2), Check::Signed(16)),
    d(14, "R_X86_64_8", Op::Abs, Field::Data(1), Check::Either(8)),
    d(15, "R_X86_64_PC8", Op::Prel, Field::Data(1), Check::Signed(8)),
    d(17, "R_X86_64_DTPOFF64", Op::DtpRel, D8, Check::None),
    d(18, "R_X86_64_TPOFF64", Op::TpRel, D8, Check::None),
    d(19, "R_X86_64_TLSGD", Op::Slot(G::Gd, F::PrelA), D4, S32),
    d(20, "R_X86_64_TLSLD", Op::Slot(G::Ld, F::PrelA), D4, S32),
    d(21, "R_X86_64_DTPOFF32", Op::DtpRel, D4, S32),
    d(22, "R_X86_64_GOTTPOFF", Op::Slot(G::TpRel, F::PrelA), D4, S32),
    d(23, "R_X86_64_TPOFF32", Op::TpRel, D4, S32),
    d(24, "R_X86_64_PC64", Op::Prel, D8, Check::None),
    d(25, "R_X86_64_GOTOFF64", Op::GotRel, D8, Check::None),
    d(26, "R_X86_64_GOTPC32", Op::GotPc, D4, S32),
    d(27, "R_X86_64_GOT64", Op::Slot(G::Addr, F::OffA), D8, Check::None),
    d(29, "R_X86_64_GOTPC64", Op::GotPc, D8, Check::None),
    d(31, "R_X86_64_PLTOFF64", Op::GotRel, D8, Check::None),
    d(32, "R_X86_64_SIZE32", Op::Size, D4, Check::Unsigned(32)),
    d(33, "R_X86_64_SIZE64", Op::Size, D8, Check::None),
    d(34, "R_X86_64_GOTPC32_TLSDESC", Op::Slot(G::Desc, F::PrelA), D4, S32),
    d(35, "R_X86_64_TLSDESC_CALL", Op::DescCall, Field::Data(0), Check::None),
    d(41, "R_X86_64_GOTPCRELX", Op::Slot(G::Addr, F::PrelA), D4, S32),
    d(42, "R_X86_64_REX_GOTPCRELX", Op::Slot(G::Addr, F::PrelA), D4, S32),
];

/// The row of `r_type`.
pub fn def(r_type: u32) -> Option<&'static Def> {
    TABLE.binary_search_by_key(&r_type, |d| d.r_type).ok().map(|i| &TABLE[i])
}

/// The width of a type outside the table, for S12 (§6.3): `spec/x64.md`
/// §3.2's field of every numbered type, 4 when unknown.
pub fn unchecked_width(r_type: u32) -> u8 {
    match r_type {
        0 | 35 => 0,
        5 | 6 | 7 | 8 | 16 | 28 | 30 | 36 | 37 => 8,
        12 | 13 => 2,
        14 | 15 => 1,
        _ => 4,
    }
}

/// `REX'`: the REX prefix with REX.R moved to REX.B (`spec/x64.md` §8),
/// for an instruction whose register moves from ModRM.reg to ModRM.rm.
pub const fn rex_rb(rex: u8) -> u8 {
    (rex & !0x05) | ((rex >> 2) & 1)
}

/// The `ModRM` of a RIP-relative operand (`mod` 00, `rm` 101): its `reg`.
pub const fn rip_reg(modrm: u8) -> Option<u8> {
    crate::x86op::disp32_reg(modrm)
}

/// Whether `b` is a REX.W prefix without X and B (`48` or `4c`).
pub const fn rex_w(b: u8) -> bool {
    b & 0xfb == 0x48
}

/// The LE form of a relaxed GD sequence's 16 bytes: `mov %fs:0, %rax`
/// then `lea T(%rax), %rax` (`8d 80`) or mold's `add $T, %rax` (`81 c0`).
pub fn gd_le(t: u32, add: bool) -> [u8; 16] {
    let mut v = [0x64, 0x48, 0x8b, 0x04, 0x25, 0, 0, 0, 0, 0x48, 0x8d, 0x80, 0, 0, 0, 0];
    if add {
        v[10] = 0x81;
        v[11] = 0xc0;
    }
    v[12..16].copy_from_slice(&t.to_le_bytes());
    v
}

/// `mov %fs:0, %rax` (9 bytes).
pub const MOV_FS0_RAX: [u8; 9] = [0x64, 0x48, 0x8b, 0x04, 0x25, 0, 0, 0, 0];
/// `nopw 0(%rax,%rax,1)` (6 bytes).
pub const NOPW6: [u8; 6] = [0x66, 0x0f, 0x1f, 0x44, 0x00, 0x00];
/// `cs nopw 0(%rax,%rax,1)` with three data16 prefixes before it (13
/// bytes: `66 66 66 66 2e 0f 1f 84 00 00 00 00 00`).
pub const NOP13: [u8; 13] = [0x66, 0x66, 0x66, 0x66, 0x2e, 0x0f, 0x1f, 0x84, 0, 0, 0, 0, 0];

//! The i386 relocation table (`spec/plumb.md` §4.5): `spec/x86.md` §6.1's
//! M1 rows, with the field, the check and the operation of each type.
//! Every field is data and every addend is implicit: the field's input
//! bytes, sign-extended (X86-4). `X` is computed modulo 2^32 and the
//! narrow checks apply to the reduced value (§3.1); `TLS_LE` is checked
//! before the reduction (X86-38). The record path is `x86rec.rs`.

use crate::a64::{Check, Def, Field, Form as F, Op, Slot as G};

pub const NONE: u32 = 0;
pub const R32: u32 = 1;
pub const PC32: u32 = 2;
pub const GOT32: u32 = 3;
pub const PLT32: u32 = 4;
pub const GOTOFF: u32 = 9;
pub const GOTPC: u32 = 10;
pub const TLS_IE: u32 = 15;
pub const TLS_GOTIE: u32 = 16;
pub const TLS_LE: u32 = 17;
pub const TLS_GD: u32 = 18;
pub const TLS_LDM: u32 = 19;
pub const R16: u32 = 20;
pub const PC16: u32 = 21;
pub const R8: u32 = 22;
pub const PC8: u32 = 23;
pub const TLS_LDO_32: u32 = 32;
pub const TLS_IE_32: u32 = 33;
pub const TLS_LE_32: u32 = 34;
pub const SIZE32: u32 = 38;
pub const TLS_GOTDESC: u32 = 39;
pub const TLS_DESC_CALL: u32 = 40;
pub const GOT32X: u32 = 43;

const fn d(r_type: u32, name: &'static str, op: Op, field: Field, check: Check) -> Def {
    Def { r_type, name, op, field, check }
}

const D4: Field = Field::Data(4);
/// `mod(32)`: no range; the field takes `X mod 2^32` (X86-10).
const MOD: Check = Check::None;

/// The table, in type order: `spec/x86.md` §6.1's M1 rows. `GOT32` and
/// `GOT32X` are listed with a base register (`G + A − GOT`); their
/// no-base reading is [`GOT32_NOBASE`]/[`GOT32X_NOBASE`] (X86-13).
/// `TLS_GD`, `TLS_LDM` and `TLS_GOTDESC` have no unrelaxed value in a
/// static output (X86-17): they are judged as sequences (§9).
pub const TABLE: &[Def] = &[
    d(R32, "R_386_32", Op::Abs, D4, MOD),
    d(PC32, "R_386_PC32", Op::Prel, D4, MOD),
    d(GOT32, "R_386_GOT32", Op::Slot(G::Addr, F::OffA), D4, MOD),
    d(PLT32, "R_386_PLT32", Op::Prel, D4, MOD),
    d(GOTOFF, "R_386_GOTOFF", Op::GotRel, D4, MOD),
    d(GOTPC, "R_386_GOTPC", Op::GotPc, D4, MOD),
    d(TLS_IE, "R_386_TLS_IE", Op::Slot(G::TpRel, F::AbsA), D4, MOD),
    d(TLS_GOTIE, "R_386_TLS_GOTIE", Op::Slot(G::TpRel, F::OffA), D4, MOD),
    d(TLS_LE, "R_386_TLS_LE", Op::TpRel, D4, Check::Signed(32)),
    d(TLS_GD, "R_386_TLS_GD", Op::Slot(G::Gd, F::OffA), D4, MOD),
    d(TLS_LDM, "R_386_TLS_LDM", Op::Slot(G::Ld, F::OffA), D4, MOD),
    d(R16, "R_386_16", Op::Abs, Field::Data(2), Check::Either(16)),
    d(PC16, "R_386_PC16", Op::Prel, Field::Data(2), Check::Signed(16)),
    d(R8, "R_386_8", Op::Abs, Field::Data(1), Check::Either(8)),
    d(PC8, "R_386_PC8", Op::Prel, Field::Data(1), Check::Signed(8)),
    d(TLS_LDO_32, "R_386_TLS_LDO_32", Op::DtpRel, D4, MOD),
    d(SIZE32, "R_386_SIZE32", Op::Size, D4, Check::Unsigned(32)),
    d(TLS_GOTDESC, "R_386_TLS_GOTDESC", Op::Slot(G::Desc, F::OffA), D4, MOD),
    d(TLS_DESC_CALL, "R_386_TLS_DESC_CALL", Op::DescCall, Field::Data(0), MOD),
    d(GOT32X, "R_386_GOT32X", Op::Slot(G::Addr, F::OffA), D4, MOD),
];

/// `GOT32` whose operand has no base register: `G + A` (X86-13).
pub const GOT32_NOBASE: Def = d(GOT32, "R_386_GOT32", Op::Slot(G::Addr, F::AbsA), D4, MOD);
/// `GOT32X` whose operand has no base register: `G + A` (X86-13).
pub const GOT32X_NOBASE: Def = d(GOT32X, "R_386_GOT32X", Op::Slot(G::Addr, F::AbsA), D4, MOD);

/// The row of `r_type`.
pub fn def(r_type: u32) -> Option<&'static Def> {
    TABLE.binary_search_by_key(&r_type, |d| d.r_type).ok().map(|i| &TABLE[i])
}

/// The dynamic-only types (`spec/x86.md` X-20): in an input they are a
/// finding (R8); GNU ld writes `S + A` for them (W14).
pub const fn dynamic_only(r_type: u32) -> bool {
    matches!(r_type, 5 | 6 | 7 | 8 | 14 | 35 | 36 | 37 | 41 | 42)
}

/// The width of a type outside the table, for S12 (§6.3): `spec/x86.md`
/// §3.3's field of every numbered type, 4 when unknown.
pub fn unchecked_width(r_type: u32) -> u8 {
    match r_type {
        0 | 40 => 0,
        20 | 21 => 2,
        22 | 23 => 1,
        _ => 4,
    }
}

/// The 12 bytes of a GD sequence relaxed to LE (D1–D3, X86-34):
/// `movl %gs:0, %eax; subl $(−T), %eax`, or mold's `addl $T, %eax`.
pub fn gd_le(t: u32, add: bool) -> [u8; 12] {
    let mut v = [0x65, 0xa1, 0, 0, 0, 0, 0x81, 0xe8, 0, 0, 0, 0];
    let imm = if add {
        v[7] = 0xc0;
        t
    } else {
        t.wrapping_neg()
    };
    v[8..].copy_from_slice(&imm.to_le_bytes());
    v
}

/// `movl %gs:0, %eax` (6 bytes).
pub const MOV_GS0_EAX: [u8; 6] = [0x65, 0xa1, 0, 0, 0, 0];
/// L1's NOPs after it: `nop; lea 0(%esi,%eiz,1), %esi`.
pub const NOP5_L1: [u8; 5] = [0x90, 0x8d, 0x74, 0x26, 0x00];
/// L2's 6-byte NOP: `lea 0(%esi), %esi`.
pub const NOP6_L2: [u8; 6] = [0x8d, 0xb6, 0, 0, 0, 0];
/// `endbr32`.
pub const ENDBR32: [u8; 4] = [0xf3, 0x0f, 0x1e, 0xfb];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sorted_and_complete() {
        assert!(TABLE.windows(2).all(|w| w[0].r_type < w[1].r_type));
        // spec/x86.md §6.1: 21 M1 rows, 0 (NONE) has no Def.
        assert_eq!(TABLE.len(), 20);
        assert!(def(TLS_IE_32).is_none() && def(TLS_LE_32).is_none() && def(11).is_none());
        assert_eq!(gd_le(0xffff_ff8c, false)[6..], [0x81, 0xe8, 0x74, 0, 0, 0]);
    }
}

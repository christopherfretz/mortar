//! The instruction field forms of a64.md §3.2: which instruction each
//! requires, how the bits of `X` are inserted, and how the immediate is
//! read back.
//!
//! A form is a `u16` (`Field::Insn { form }`); the constants below are the
//! only values this crate writes into a `Howto`. Every function is total
//! over all 32-bit words and all values: an unknown form requires nothing
//! and inserts nothing.

/// `adr`: `X[1:0]` → `[30:29]`, `X[20:2]` → `[23:5]`.
pub const ADR: u16 = 1;
/// `adrp`: `X[13:12]` → `[30:29]`, `X[32:14]` → `[23:5]`.
pub const ADRP: u16 = 2;
/// `add12`: `X[11:0]` → `[21:10]`, ADD/ADDS (immediate) with `sh = 0`.
pub const ADD12: u16 = 3;
/// `hi12`: `X[23:12]` → `[21:10]`, ADD/ADDS (immediate) with `sh = 1`.
pub const HI12: u16 = 4;
/// `ls12/n` is `LS12 + n`, n = 0..=4: `X[11:n]` → `[21:10]`.
pub const LS12: u16 = 10;
/// `ldx12`: `X[11:3]` → `[21:10]`, `LDR Xt, [Xn, #imm]`.
pub const LDX12: u16 = 20;
/// `ls15`: `X[14:3]` → `[21:10]`, as `ldx12`.
pub const LS15: u16 = 21;
/// `lit19`: `X[20:2]` → `[23:5]`, load register (literal).
pub const LIT19: u16 = 22;
/// `ldx19`: `X[20:2]` → `[23:5]`, `LDR Xt, label`.
pub const LDX19: u16 = 23;
/// `bc19`: `X[20:2]` → `[23:5]`, B.cond, BC.cond, CBZ, CBNZ.
pub const BC19: u16 = 24;
/// `tb14`: `X[15:2]` → `[18:5]`, TBZ/TBNZ.
pub const TB14: u16 = 25;
/// `b26`: `X[27:2]` → `[25:0]`, B/BL.
pub const B26: u16 = 26;
/// `mov/k` is `MOV + k`, k = 0..=3: `X[16k+15:16k]` → `[20:5]`.
pub const MOV: u16 = 30;
/// `movs/k` is `MOVS + k`: MOVZ of `X` or MOVN of `!X` by sign.
pub const MOVS: u16 = 40;

/// Every form, for tests: `(form, name)`.
pub const ALL: [(u16, &str); 24] = [
    (ADR, "adr"),
    (ADRP, "adrp"),
    (ADD12, "add12"),
    (HI12, "hi12"),
    (LS12, "ls12/0"),
    (LS12 + 1, "ls12/1"),
    (LS12 + 2, "ls12/2"),
    (LS12 + 3, "ls12/3"),
    (LS12 + 4, "ls12/4"),
    (LDX12, "ldx12"),
    (LS15, "ls15"),
    (LIT19, "lit19"),
    (LDX19, "ldx19"),
    (BC19, "bc19"),
    (TB14, "tb14"),
    (B26, "b26"),
    (MOV, "mov/0"),
    (MOV + 1, "mov/1"),
    (MOV + 2, "mov/2"),
    (MOV + 3, "mov/3"),
    (MOVS, "movs/0"),
    (MOVS + 1, "movs/1"),
    (MOVS + 2, "movs/2"),
    (MOVS + 3, "movs/3"),
];

/// The log2 of the access size of a load/store register (unsigned
/// immediate) word: 4 for a 128-bit SIMD&FP access (`V = 1`, `size = 0`,
/// `opc[1] = 1`), else `size` (PRFM: 3).
#[must_use]
pub const fn ls_size_log2(w: u32) -> u32 {
    let size = w >> 30;
    let v = (w >> 26) & 1;
    let opc = (w >> 22) & 3;
    if v == 1 && size == 0 && opc & 2 != 0 { 4 } else { size }
}

/// A move-wide word with `hw = k` (and `sf = 1` for `k ≥ 2`).
const fn move_wide(w: u32, k: u32) -> bool {
    w & 0x1F80_0000 == 0x1280_0000 && (w >> 21) & 3 == k && (k < 2 || w >> 31 == 1)
}

/// Whether input word `w` is the instruction form `form` requires (§3.2).
#[must_use]
pub const fn requires(form: u16, w: u32) -> bool {
    match form {
        ADR => w & 0x9F00_0000 == 0x1000_0000,
        ADRP => w & 0x9F00_0000 == 0x9000_0000,
        ADD12 => w & 0x5FC0_0000 == 0x1100_0000,
        HI12 => w & 0x5FC0_0000 == 0x1140_0000,
        LS12..=14 => w & 0x3B00_0000 == 0x3900_0000 && ls_size_log2(w) == (form - LS12) as u32,
        LDX12 | LS15 => w & 0xFFC0_0000 == 0xF940_0000,
        LIT19 => w & 0x3B00_0000 == 0x1800_0000,
        LDX19 => w & 0xFF00_0000 == 0x5800_0000,
        BC19 => w & 0xFF00_0000 == 0x5400_0000 || w & 0x7E00_0000 == 0x3400_0000,
        TB14 => w & 0x7E00_0000 == 0x3600_0000,
        B26 => w & 0x7C00_0000 == 0x1400_0000,
        MOV..=33 => move_wide(w, (form - MOV) as u32),
        MOVS..=43 => move_wide(w, (form - MOVS) as u32) && (w >> 29) & 3 != 3,
        _ => false,
    }
}

/// The 21-bit ADR/ADRP immediate field of `v`'s low 21 bits.
const fn adr_bits(v: u64) -> u32 {
    let v = v as u32;
    ((v & 3) << 29) | (((v >> 2) & 0x7_FFFF) << 5)
}

/// The bits of `form`'s field, and the bits of `w` it replaces.
const fn field(form: u16, x: i64) -> (u32, u32) {
    let u = x as u64;
    match form {
        ADR => (adr_bits(u), 0x60FF_FFE0),
        ADRP => (adr_bits(u >> 12), 0x60FF_FFE0),
        ADD12 => (((u & 0xFFF) as u32) << 10, 0x003F_FC00),
        HI12 => ((((u >> 12) & 0xFFF) as u32) << 10, 0x003F_FC00),
        LS12..=14 => ((((u & 0xFFF) >> (form - LS12)) as u32) << 10, 0x003F_FC00),
        LDX12 => ((((u & 0xFFF) >> 3) as u32) << 10, 0x003F_FC00),
        LS15 => ((((u & 0x7FFF) >> 3) as u32) << 10, 0x003F_FC00),
        LIT19 | LDX19 | BC19 => ((((u >> 2) & 0x7_FFFF) as u32) << 5, 0x00FF_FFE0),
        TB14 => ((((u >> 2) & 0x3FFF) as u32) << 5, 0x0007_FFE0),
        B26 => (((u >> 2) & 0x03FF_FFFF) as u32, 0x03FF_FFFF),
        MOV..=33 => ((((u >> (16 * (form - MOV) as u32)) & 0xFFFF) as u32) << 5, 0x001F_FFE0),
        MOVS..=43 => {
            let k = 16 * (form - MOVS) as u32;
            if x >= 0 {
                ((((u >> k) & 0xFFFF) as u32) << 5 | 0b10 << 29, 0x601F_FFE0)
            } else {
                ((((!u >> k) & 0xFFFF) as u32) << 5, 0x601F_FFE0)
            }
        }
        _ => (0, 0),
    }
}

/// `w` with the bits of `X` that `form` takes inserted, every other bit
/// kept (§3.2). The caller has checked `requires` and the value's check.
#[must_use]
pub const fn insert(form: u16, w: u32, x: i64) -> u32 {
    let (bits, mask) = field(form, x);
    (w & !mask) | bits
}

/// Sign-extends the low `n` bits of `v`.
#[must_use]
pub const fn sext(v: u64, n: u32) -> i64 {
    let s = 64 - n;
    ((v << s) as i64) >> s
}

/// The value the field of `form` in `w` encodes, scaled back to the units
/// of `X`: the inverse of [`insert`] on the bits the form keeps
/// (immediates of PC-relative forms signed; `adrp` the page distance; MOVW
/// the 16-bit chunk in place; `movs` signed by its opcode). For tests, the
/// ADR fix and a future REL reader (§3.3).
#[must_use]
pub const fn extract(form: u16, w: u32) -> i64 {
    let adr = (((w >> 5) & 0x7_FFFF) << 2 | ((w >> 29) & 3)) as u64;
    let imm12 = ((w >> 10) & 0xFFF) as u64;
    let imm16 = ((w >> 5) & 0xFFFF) as u64;
    match form {
        ADR => sext(adr, 21),
        ADRP => sext(adr, 21) << 12,
        ADD12 => imm12 as i64,
        HI12 => (imm12 << 12) as i64,
        LS12..=14 => (imm12 << (form - LS12)) as i64,
        LDX12 | LS15 => (imm12 << 3) as i64,
        LIT19 | LDX19 | BC19 => sext((((w >> 5) & 0x7_FFFF) << 2) as u64, 21),
        TB14 => sext((((w >> 5) & 0x3FFF) << 2) as u64, 16),
        B26 => sext(((w & 0x03FF_FFFF) << 2) as u64, 28),
        MOV..=33 => (imm16 << (16 * (form - MOV) as u32)) as i64,
        MOVS..=43 => {
            let k = 16 * (form - MOVS) as u32;
            if (w >> 29) & 3 == 0b10 { (imm16 << k) as i64 } else { !((imm16 << k) as i64) }
        }
        _ => 0,
    }
}

/// The name of a form, for messages and tests.
#[must_use]
pub fn name(form: u16) -> &'static str {
    ALL.iter().find(|(f, _)| *f == form).map_or("?", |(_, n)| n)
}

/// The register fields (a64.md §8): `Rd`/`Rt` `[4:0]`, `Rn` `[9:5]`.
#[must_use]
pub const fn rd(w: u32) -> u32 {
    w & 0x1F
}

#[must_use]
pub const fn rn(w: u32) -> u32 {
    (w >> 5) & 0x1F
}

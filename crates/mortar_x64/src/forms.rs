//! The instruction forms of x64.md §8 as byte patterns over the input
//! section, read only inside the form's window (X-8).
//!
//! A form is a pure function of the type, the section bytes and the
//! offset: `classify` uses it for the window, scan for kind-1 decisions,
//! fill for the rewrite, and every caller sees the same answer.

use mortar_arch::{Site, Window};

pub const PC32: u32 = 2;
pub const PLT32: u32 = 4;
pub const GOTPCREL: u32 = 9;
pub const DTPOFF64: u32 = 17;
pub const TLSGD: u32 = 19;
pub const TLSLD: u32 = 20;
pub const DTPOFF32: u32 = 21;
pub const GOTTPOFF: u32 = 22;
pub const PLTOFF64: u32 = 31;
pub const GOTPC32_TLSDESC: u32 = 34;
pub const TLSDESC_CALL: u32 = 35;
pub const GOTPCRELX: u32 = 41;
pub const REX_GOTPCRELX: u32 = 42;

/// A recognized form (§8's table). `rex` is the input REX prefix (`48` or
/// `4c`), `reg` the input `ModRM`'s reg field, `op` a G5 opcode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Form {
    /// `REX_GOTPCRELX` `48|4c 8b` `ModRM` `00 reg 101` (mov).
    G1 { rex: u8 },
    /// `GOTPCRELX` `8b` `ModRM` `00 reg 101` (32-bit mov).
    G2,
    /// `GOTPCRELX` `ff 15` (call).
    G3,
    /// `GOTPCRELX` `ff 25` (jmp).
    G4,
    /// `REX_GOTPCRELX` `48|4c op` `ModRM` `00 reg 101`, `op` a binary
    /// operation of [`BINOPS`].
    G5 { rex: u8, op: u8, reg: u8 },
    /// `REX_GOTPCRELX` `48|4c 85` `ModRM` `00 reg 101` (test).
    G6 { rex: u8, reg: u8 },
    /// `GOTTPOFF` `48|4c 8b` `ModRM` `00 reg 101` (mov).
    I1 { rex: u8, reg: u8 },
    /// `GOTTPOFF` `48|4c 03` `ModRM` `00 reg 101` (add).
    I2 { rex: u8, reg: u8 },
    /// `GOTPC32_TLSDESC` `48|4c 8d` `ModRM` `00 reg 101` (lea).
    T1 { rex: u8, reg: u8 },
    /// `TLSDESC_CALL` `ff 10` at `P`.
    T2,
    /// `66 48 8d 3d [P] 66 66 48 e8 [rel32]`.
    Gd16,
    /// `66 48 8d 3d [P] 66 48 ff 15 [rel32]`.
    Gd16x,
    /// `48 8d 3d [P] 48 b8 [imm64] (48|4c) 01 (c0 | r << 3) ff d0`.
    Gd22,
    /// `48 8d 3d [P] e8 [rel32]`.
    Ld12,
    /// `48 8d 3d [P] ff 15 [rel32]`.
    Ld13,
    /// `48 8d 3d [P] 48 b8 [imm64] (48|4c) 01 (c0 | r << 3) ff d0`.
    Ld22,
}

/// G5's opcodes: `add or adc sbb and sub xor cmp` (`r64, r/m64`).
pub const BINOPS: [u8; 8] = [0x03, 0x0b, 0x13, 0x1b, 0x23, 0x2b, 0x33, 0x3b];

/// One form per window class, for [`crate::MAX_REACH`] (§8.6).
pub const ALL_WINDOWS: [Form; 16] = [
    Form::G1 { rex: 0x48 },
    Form::G2,
    Form::G3,
    Form::G4,
    Form::G5 { rex: 0x48, op: 0x03, reg: 0 },
    Form::G6 { rex: 0x48, reg: 0 },
    Form::I1 { rex: 0x48, reg: 0 },
    Form::I2 { rex: 0x48, reg: 0 },
    Form::T1 { rex: 0x48, reg: 0 },
    Form::T2,
    Form::Gd16,
    Form::Gd16x,
    Form::Gd22,
    Form::Ld12,
    Form::Ld13,
    Form::Ld22,
];

const fn w(before: u8, after: u8) -> Window {
    Window { before, after }
}

impl Form {
    /// The form's window around its 4-byte field (§8.6; `T2` around its
    /// zero-width marker).
    #[must_use]
    pub const fn window(self) -> Window {
        match self {
            Self::G1 { .. }
            | Self::G5 { .. }
            | Self::G6 { .. }
            | Self::I1 { .. }
            | Self::I2 { .. }
            | Self::T1 { .. } => w(3, 0),
            Self::G2 | Self::G3 | Self::G4 => w(2, 0),
            Self::T2 => w(0, 2),
            Self::Gd16 | Self::Gd16x => w(4, 8),
            Self::Gd22 | Self::Ld22 => w(3, 15),
            Self::Ld12 => w(3, 5),
            Self::Ld13 => w(3, 6),
        }
    }

    /// A sequence lead's follower (§8.3, §8.4, X-30): the types allowed
    /// and the offset of the follower's field from `P`.
    #[must_use]
    pub const fn follower(self) -> Option<(&'static [u32], u64)> {
        match self {
            Self::Gd16 => Some((&[PLT32, PC32], 8)),
            Self::Gd16x => Some((&[GOTPCRELX, GOTPCREL], 8)),
            Self::Ld12 => Some((&[PLT32, PC32], 5)),
            Self::Ld13 => Some((&[GOTPCRELX, GOTPCREL], 6)),
            Self::Gd22 | Self::Ld22 => Some((&[PLTOFF64], 6)),
            _ => None,
        }
    }
}

/// The `len` input bytes from `P − before`, if they lie in the section
/// (X-8: a window outside the section does not match).
fn around<'a>(site: &Site<'a>, before: u8, len: usize) -> Option<&'a [u8]> {
    let start = usize::try_from(site.offset.checked_sub(u64::from(before))?).ok()?;
    site.bytes.get(start..start.checked_add(len)?)
}

/// `ModRM` `00 reg 101`: RIP-relative, no SIB.
const fn rip(modrm: u8) -> bool {
    modrm & 0xC7 == 0x05
}

const fn reg(modrm: u8) -> u8 {
    (modrm >> 3) & 7
}

/// `48` or `4c`: REX.W, with or without REX.R, nothing else.
const fn rex_w(b: u8) -> bool {
    b == 0x48 || b == 0x4c
}

/// `REX'` (§8): REX.R moved to REX.B (`48` → `48`, `4c` → `49`).
#[must_use]
pub const fn rex_prime(rex: u8) -> u8 {
    (rex & !0x04) | ((rex & 0x04) >> 2)
}

/// The form the bytes around `site` match for a relocation of `r_type`,
/// or `None` (the record is then applied as its §6.1 row, or refused where
/// §8 says so).
#[must_use]
pub fn form(r_type: u32, site: Site<'_>) -> Option<Form> {
    match r_type {
        REX_GOTPCRELX => {
            let b = around(&site, 3, 3)?;
            if !rex_w(b[0]) || !rip(b[2]) {
                return None;
            }
            let (rex, r) = (b[0], reg(b[2]));
            match b[1] {
                0x8b => Some(Form::G1 { rex }),
                0x85 => Some(Form::G6 { rex, reg: r }),
                op if is_binop(op) => Some(Form::G5 { rex, op, reg: r }),
                _ => None,
            }
        }
        GOTPCRELX => match around(&site, 2, 2)? {
            [0x8b, m] if rip(*m) => Some(Form::G2),
            [0xff, 0x15] => Some(Form::G3),
            [0xff, 0x25] => Some(Form::G4),
            _ => None,
        },
        GOTTPOFF => {
            let b = around(&site, 3, 3)?;
            if !rex_w(b[0]) || !rip(b[2]) {
                return None;
            }
            let (rex, r) = (b[0], reg(b[2]));
            match b[1] {
                0x8b => Some(Form::I1 { rex, reg: r }),
                0x03 => Some(Form::I2 { rex, reg: r }),
                _ => None,
            }
        }
        GOTPC32_TLSDESC => {
            let b = around(&site, 3, 3)?;
            (rex_w(b[0]) && b[1] == 0x8d && rip(b[2]))
                .then_some(Form::T1 { rex: b[0], reg: reg(b[2]) })
        }
        TLSDESC_CALL => (around(&site, 0, 2)? == [0xff, 0x10]).then_some(Form::T2),
        TLSGD => gd_form(&site),
        TLSLD => ld_form(&site),
        _ => None,
    }
}

const fn is_binop(op: u8) -> bool {
    // `00 xxx 011` with xxx any of the eight ALU operations.
    op & 0xC7 == 0x03
}

/// `48 8d 3d` (`lea x(%rip), %rdi`) just before `P`.
fn lea_rdi(site: &Site<'_>) -> bool {
    around(site, 3, 3).is_some_and(|b| b == [0x48, 0x8d, 0x3d])
}

/// The 22-byte large-model tail from `P + 4`: `48 b8 [imm64] (48|4c) 01
/// (c0 | r << 3) ff d0`.
fn large_tail(site: &Site<'_>) -> bool {
    let Some(b) = around(site, 3, 22) else { return false };
    b[7..9] == [0x48, 0xb8]
        && rex_w(b[17])
        && b[18] == 0x01
        && b[19] & 0xC7 == 0xC0
        && b[20..22] == [0xff, 0xd0]
}

fn gd_form(site: &Site<'_>) -> Option<Form> {
    if !lea_rdi(site) {
        return None;
    }
    if let Some(b) = around(site, 4, 16)
        && b[0] == 0x66
    {
        match b[8..12] {
            [0x66, 0x66, 0x48, 0xe8] => return Some(Form::Gd16),
            [0x66, 0x48, 0xff, 0x15] => return Some(Form::Gd16x),
            _ => {}
        }
    }
    large_tail(site).then_some(Form::Gd22)
}

fn ld_form(site: &Site<'_>) -> Option<Form> {
    if !lea_rdi(site) {
        return None;
    }
    if around(site, 3, 12).is_some_and(|b| b[7] == 0xe8) {
        return Some(Form::Ld12);
    }
    if around(site, 3, 13).is_some_and(|b| b[7..9] == [0xff, 0x15]) {
        return Some(Form::Ld13);
    }
    large_tail(site).then_some(Form::Ld22)
}

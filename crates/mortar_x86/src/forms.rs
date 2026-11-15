//! The instruction forms of x86.md §8 and §9 as byte patterns over the
//! input section, and the `GOT32`/`GOT32X` operand reading of X86-13.
//!
//! A form is a pure function of the type, the section bytes and the
//! offset: `classify` uses it for the window, scan for the kind-1
//! decisions, fill for the rewrite, and every caller sees the same
//! answer. Nothing here reads outside the record's read window (X86-40);
//! bytes outside the section do not exist (a pattern that needs them does
//! not match).

use mortar_arch::{Site, Window};

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
pub const SIZE32: u32 = 38;
pub const TLS_GOTDESC: u32 = 39;
pub const TLS_DESC_CALL: u32 = 40;
pub const GOT32X: u32 = 43;

/// The X86-13 reading of a `GOT32`/`GOT32X` memory operand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Operand {
    /// A base register: `G + A − GOT`.
    Base,
    /// No base: `G + A`, the absolute slot address.
    NoBase,
}

/// A recognized form (§8's and §9's tables). `reg` is the input `ModRM`'s
/// reg field (bits 5..3), `op` a G2 opcode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Form {
    /// `GOT32X` `8b` `ModRM` (mov) → `c7 (c0 | reg) imm32`.
    G1 { reg: u8 },
    /// `GOT32X` `op` `ModRM`, `op` in [`BINOPS`] → `81 (c0 | (op & 38) | reg)
    /// imm32`.
    G2 { op: u8, reg: u8 },
    /// `GOT32X` `85` `ModRM` (test) → `f7 (c0 | reg) imm32`.
    G3 { reg: u8 },
    /// `GOT32X` `ff` `ModRM` with reg = 2 (call) → `67 e8 rel32`.
    G4,
    /// `GOT32X` `ff` `ModRM` with reg = 4 (jmp) → `e9 rel32 90`.
    G5,
    /// `TLS_IE` `a1` at `P − 1` → `b8 T`.
    I1,
    /// `TLS_IE` `8b` `ModRM` `00 reg 101` → `c7 (c0 | reg) T`.
    I2 { reg: u8 },
    /// `TLS_IE` `03` `ModRM` `00 reg 101` → `81 (c0 | reg) T`.
    I3 { reg: u8 },
    /// `TLS_GOTIE` `8b` `ModRM` `10 reg rm`, rm ≠ 100 → `c7 (c0 | reg) T`.
    I4 { reg: u8 },
    /// `TLS_GOTIE` `03` `ModRM` `10 reg rm`, rm ≠ 100 → `81 (c0 | reg) T`.
    I5 { reg: u8 },
    /// `TLS_GD` `8d 04 1d [P]`, `e8` at `P + 4`.
    D1,
    /// `TLS_GD` `8d` `ModRM` `10 000 rm` `[P]`, `ff` `ModRM` `10 010 rm'` at
    /// `P + 4`.
    D2,
    /// `TLS_GD` as D2's lead, `e8` at `P + 4`, `90` at `P + 9`.
    D3,
    /// `TLS_LDM` `8d` `ModRM` `10 000 rm` `[P]`, `e8` at `P + 4`.
    L1,
    /// `TLS_LDM` as L1's lead, `ff` `ModRM` `10 010 rm'` at `P + 4`.
    L2,
    /// `TLS_GOTDESC` `8d` `ModRM` `10 reg rm`, rm ≠ 100 → `8d (05 | reg <<
    /// 3) T`.
    T1 { reg: u8 },
    /// `TLS_DESC_CALL` `ff 10` at `P` → `66 90`.
    T2,
}

/// G2's opcodes: `add or adc sbb and sub xor cmp` (`r32, r/m32`).
pub const BINOPS: [u8; 8] = [0x03, 0x0b, 0x13, 0x1b, 0x23, 0x2b, 0x33, 0x3b];

/// One form per window class, for [`crate::MAX_REACH`] (X86-40).
pub const ALL_WINDOWS: [Form; 17] = [
    Form::G1 { reg: 0 },
    Form::G2 { op: 0x03, reg: 0 },
    Form::G3 { reg: 0 },
    Form::G4,
    Form::G5,
    Form::I1,
    Form::I2 { reg: 0 },
    Form::I3 { reg: 0 },
    Form::I4 { reg: 0 },
    Form::I5 { reg: 0 },
    Form::D1,
    Form::D2,
    Form::D3,
    Form::L1,
    Form::L2,
    Form::T1 { reg: 0 },
    Form::T2,
];

/// The largest read window of an unmatched record (X86-40): `[P − 3, P +
/// 4)` of an unmatched lead, `[P, P + 2)` of `TLS_DESC_CALL`.
pub const READ_WINDOWS: Window = Window { before: 3, after: 2 };

const fn w(before: u8, after: u8) -> Window {
    Window { before, after }
}

impl Form {
    /// The form's write window around its 4-byte field (X86-40; `T2`
    /// around its zero-width marker).
    #[must_use]
    pub const fn window(self) -> Window {
        match self {
            Self::G1 { .. }
            | Self::G2 { .. }
            | Self::G3 { .. }
            | Self::G4
            | Self::G5
            | Self::I2 { .. }
            | Self::I3 { .. }
            | Self::I4 { .. }
            | Self::I5 { .. }
            | Self::T1 { .. } => w(2, 0),
            Self::I1 => w(1, 0),
            Self::D1 => w(3, 5),
            Self::D2 | Self::D3 | Self::L2 => w(2, 6),
            Self::L1 => w(2, 5),
            Self::T2 => w(0, 2),
        }
    }

    /// A sequence lead's partner (X86-31, §9): the types allowed, the
    /// offset of the partner's field from `P`, and the partner's `A`.
    #[must_use]
    pub const fn follower(self) -> Option<(&'static [u32], u64, i64)> {
        match self {
            Self::D1 | Self::D3 | Self::L1 => Some((&[PLT32, PC32], 5, -4)),
            Self::D2 | Self::L2 => Some((&[GOT32, GOT32X], 6, 0)),
            _ => None,
        }
    }
}

/// The `len` input bytes from `P − before`, if they lie in the section.
fn around<'a>(site: &Site<'a>, before: u8, len: usize) -> Option<&'a [u8]> {
    let start = usize::try_from(site.offset.checked_sub(u64::from(before))?).ok()?;
    site.bytes.get(start..start.checked_add(len)?)
}

/// The byte at `P + delta`, if it lies in the section.
fn byte(site: &Site<'_>, delta: i64) -> Option<u8> {
    let off = site.offset.checked_add_signed(delta)?;
    site.bytes.get(usize::try_from(off).ok()?).copied()
}

const fn reg(modrm: u8) -> u8 {
    (modrm >> 3) & 7
}

/// `ModRM` `00 reg 101`: a 32-bit displacement, no base.
const fn disp32(modrm: u8) -> bool {
    modrm & 0xC7 == 0x05
}

/// `ModRM` `10 reg rm`, rm ≠ 100: a base register and a 32-bit
/// displacement, no `SIB`.
const fn base_disp32(modrm: u8) -> bool {
    modrm & 0xC0 == 0x80 && modrm & 7 != 4
}

/// X86-13 steps 2 and 3: the operand of a `GOT32`/`GOT32X` at `site`, or
/// `None` when it is refused (no byte before the field, or the `ModRM` and
/// `SIB` readings disagree). Step 1 (the place is code) is the caller's.
#[must_use]
pub fn got_operand(site: Site<'_>) -> Option<Operand> {
    let b1 = byte(&site, -1)?;
    let modrm = if disp32(b1) { Operand::NoBase } else { Operand::Base };
    if let Some(b2) = byte(&site, -2)
        && b2 & 7 == 4
        && matches!(b2 >> 6, 0b00 | 0b10)
    {
        let sib = if b2 >> 6 == 0b00 && b1 & 7 == 5 { Operand::NoBase } else { Operand::Base };
        if sib != modrm {
            return None;
        }
    }
    Some(modrm)
}

/// The read window of a record (X86-40, request R-13): the write window
/// of the form its bytes match, else the bytes `classify`, `scan` and
/// `apply` read, clipped to the section (bytes outside it do not exist).
#[must_use]
pub fn read_window(r_type: u32, site: Site<'_>) -> Window {
    if let Some(f) = form(r_type, site) {
        return f.window();
    }
    let before = |n: u8| u8::try_from(site.offset.min(u64::from(n))).unwrap_or(n);
    match r_type {
        // `TLS_IE` after `a1` is I1, whose window is (1, 0).
        GOT32 | GOT32X | TLS_GOTIE | TLS_GOTDESC | TLS_IE => w(before(2), 0),
        TLS_DESC_CALL => {
            let left = (site.bytes.len() as u64).saturating_sub(site.offset);
            w(0, u8::try_from(left.min(2)).unwrap_or(2))
        }
        TLS_GD | TLS_LDM => w(before(3), 0),
        _ => w(0, 0),
    }
}

/// The form the bytes around `site` match for a relocation of `r_type`,
/// or `None`. The conditions that are not bytes (the output kind, `A`,
/// the target, the partner record) are the caller's (§8, §9).
#[must_use]
pub fn form(r_type: u32, site: Site<'_>) -> Option<Form> {
    match r_type {
        GOT32X => got_form(site),
        TLS_IE => {
            if byte(&site, -1)? == 0xa1 {
                return Some(Form::I1);
            }
            match around(&site, 2, 2)? {
                [0x8b, m] if disp32(*m) => Some(Form::I2 { reg: reg(*m) }),
                [0x03, m] if disp32(*m) => Some(Form::I3 { reg: reg(*m) }),
                _ => None,
            }
        }
        TLS_GOTIE => match around(&site, 2, 2)? {
            [0x8b, m] if base_disp32(*m) => Some(Form::I4 { reg: reg(*m) }),
            [0x03, m] if base_disp32(*m) => Some(Form::I5 { reg: reg(*m) }),
            _ => None,
        },
        TLS_GOTDESC => match around(&site, 2, 2)? {
            [0x8d, m] if base_disp32(*m) => Some(Form::T1 { reg: reg(*m) }),
            _ => None,
        },
        TLS_DESC_CALL => (around(&site, 0, 2)? == [0xff, 0x10]).then_some(Form::T2),
        TLS_GD => gd_form(&site),
        TLS_LDM => ld_form(&site),
        _ => None,
    }
}

/// G1–G5 (X86-25's byte conditions): X86-13 gives an answer, the `ModRM`
/// is `00 reg 101` (no base) or `10 reg rm`, rm ≠ 100 (base), and the
/// opcode at `P − 2` is one of the five.
fn got_form(site: Site<'_>) -> Option<Form> {
    got_operand(site)?;
    let [op, m] = *around(&site, 2, 2)? else { return None };
    if !(disp32(m) || base_disp32(m)) {
        return None;
    }
    let r = reg(m);
    match op {
        0x8b => Some(Form::G1 { reg: r }),
        0x85 => Some(Form::G3 { reg: r }),
        0xff if r == 2 => Some(Form::G4),
        0xff if r == 4 => Some(Form::G5),
        op if BINOPS.contains(&op) => Some(Form::G2 { op, reg: r }),
        _ => None,
    }
}

/// `8d` `ModRM` `10 000 rm`, rm ≠ 100, at `P − 2`: `leal x(%rm), %eax`.
fn lea_eax_base(site: &Site<'_>) -> bool {
    around(site, 2, 2).is_some_and(|b| b[0] == 0x8d && base_disp32(b[1]) && reg(b[1]) == 0)
}

/// `ff` `ModRM` `10 010 rm'` at `P + 4`: `call *x(%rm')`. rm' = 100 would
/// put a `SIB` byte where the partner's field must start (`P + 6`).
fn call_indirect(site: &Site<'_>) -> bool {
    byte(site, 4) == Some(0xff) && byte(site, 5).is_some_and(|m| base_disp32(m) && reg(m) == 2)
}

fn gd_form(site: &Site<'_>) -> Option<Form> {
    if around(site, 3, 3) == Some(&[0x8d, 0x04, 0x1d]) && around(site, 3, 12).is_some() {
        return (byte(site, 4) == Some(0xe8)).then_some(Form::D1);
    }
    if !lea_eax_base(site) || around(site, 2, 12).is_none() {
        return None;
    }
    if call_indirect(site) {
        Some(Form::D2)
    } else if byte(site, 4) == Some(0xe8) && byte(site, 9) == Some(0x90) {
        Some(Form::D3)
    } else {
        None
    }
}

fn ld_form(site: &Site<'_>) -> Option<Form> {
    if !lea_eax_base(site) {
        return None;
    }
    if around(site, 2, 12).is_some() && call_indirect(site) {
        return Some(Form::L2);
    }
    (around(site, 2, 11).is_some() && byte(site, 4) == Some(0xe8)).then_some(Form::L1)
}

/// The implicit addend of the 4-byte field at section offset `off` of the
/// input bytes (X86-4), if the field lies in them.
#[must_use]
pub fn addend32(bytes: &[u8], off: u64) -> Option<i64> {
    let start = usize::try_from(off).ok()?;
    let b: [u8; 4] = bytes.get(start..start.checked_add(4)?)?.try_into().ok()?;
    Some(i64::from(i32::from_le_bytes(b)))
}

/// The implicit addend of a `width`-byte field (X86-4, X86-6): the bytes
/// little-endian, sign-extended. 0 for a marker.
#[must_use]
pub fn implicit(field: &[u8]) -> i64 {
    match *field {
        [a] => i64::from(a.cast_signed()),
        [a, b] => i64::from(i16::from_le_bytes([a, b])),
        [a, b, c, d] => i64::from(i32::from_le_bytes([a, b, c, d])),
        _ => 0,
    }
}

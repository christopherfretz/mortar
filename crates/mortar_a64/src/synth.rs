//! The words mortar writes itself: PLT and IPLT entries (a64.md §10.3,
//! §10.4), range-extension thunks (§9), static GOT slot contents (§10.1,
//! §12.5).

use mortar_arch::vocab::verify_align;
use mortar_arch::{Check, FieldError, Refusal, TlsAbi, TlsBlock};
use mortar_diag::RefusalClass;
use mortar_elf::Class;

use crate::forms::{self, ADD12, ADRP, LS12};

pub const NOP: u32 = 0xD503_201F;
pub const BTI_C: u32 = 0xD503_245F;
pub const BTI_J: u32 = 0xD503_249F;
pub const BTI_JC: u32 = 0xD503_24DF;
pub const PACIASP: u32 = 0xD503_233F;
pub const PACIBSP: u32 = 0xD503_237F;
/// `stp x16, x30, [sp, #-16]!`.
pub const STP_X16_X30: u32 = 0xA9BF_7BF0;
/// `adrp x16, 0`.
pub const ADRP_X16: u32 = 0x9000_0010;
/// `ldr x17, [x16, #0]`.
pub const LDR_X17_X16: u32 = 0xF940_0211;
/// `add x16, x16, #0`.
pub const ADD_X16_X16: u32 = 0x9100_0210;
/// `br x17`.
pub const BR_X17: u32 = 0xD61F_0220;
/// `br x16`.
pub const BR_X16: u32 = 0xD61F_0200;

/// The value's check and alignment.
pub fn checked(check: Check, align: u8, x: i64) -> Result<i64, FieldError> {
    check.verify(x, Class::Elf64)?;
    verify_align(x, align)?;
    Ok(x)
}

/// `Page(t) − Page(q)`, checked `s(33)`.
pub fn page_delta(q: u64, t: u64) -> Result<i64, FieldError> {
    let x = (t & !0xFFF).wrapping_sub(q & !0xFFF) as i64;
    checked(Check::Signed(33), 1, x)
}

/// The three-word "address of `g` into x16" group of the PLT and of
/// thunks: `adrp x16, g` at `q`, then the low-12 word `lo` (`ldr x17` or
/// `add x16`).
fn adrp_x16(q: u64, g: u64) -> Result<u32, FieldError> {
    Ok(forms::insert(ADRP, ADRP_X16, page_delta(q, g)?))
}

fn ldr_x17(g: u64) -> Result<u32, FieldError> {
    verify_align(g as i64 & 0xFFF, 8)?;
    Ok(forms::insert(LS12 + 3, LDR_X17_X16, g as i64))
}

const fn add_x16(g: u64) -> u32 {
    forms::insert(ADD12, ADD_X16_X16, g as i64)
}

/// Writes `words` little-endian into `out`, which must hold them.
pub fn put(out: &mut [u8], words: &[u32]) -> Result<(), FieldError> {
    let need = words.len() * 4;
    let Some(dst) = out.get_mut(..need) else {
        return Err(FieldError::Outside {
            offset: 0,
            width: u8::try_from(need).unwrap_or(u8::MAX),
        });
    };
    for (chunk, w) in dst.chunks_exact_mut(4).zip(words) {
        chunk.copy_from_slice(&w.to_le_bytes());
    }
    Ok(())
}

/// The PLT header at `addr` (32 bytes), using `.got.plt[2]` at `g2`
/// (§10.3).
pub fn plt_header(addr: u64, g2: u64, bti: bool) -> Result<[u32; 8], FieldError> {
    let first = u64::from(bti) * 4;
    let q = addr.wrapping_add(first + 4);
    let body = [STP_X16_X30, adrp_x16(q, g2)?, ldr_x17(g2)?, add_x16(g2), BR_X17];
    Ok(if bti {
        [BTI_C, body[0], body[1], body[2], body[3], body[4], NOP, NOP]
    } else {
        [body[0], body[1], body[2], body[3], body[4], NOP, NOP, NOP]
    })
}

/// A PLT or IPLT entry at `addr` loading the `.got.plt` word `g` (§10.3,
/// §10.4): 4 words plain, 6 with BTI.
pub fn plt_entry(addr: u64, g: u64, bti: bool) -> Result<([u32; 6], usize), FieldError> {
    let q = addr.wrapping_add(u64::from(bti) * 4);
    let body = [adrp_x16(q, g)?, ldr_x17(g)?, add_x16(g), BR_X17];
    Ok(if bti {
        ([BTI_C, body[0], body[1], body[2], body[3], NOP], 6)
    } else {
        ([body[0], body[1], body[2], body[3], 0, 0], 4)
    })
}

/// The thunk at `q` to `t` (§9): `adrp x16, T; add x16, x16, :lo12:T; br
/// x16`.
pub fn thunk(q: u64, t: u64) -> Result<[u32; 3], FieldError> {
    Ok([adrp_x16(q, t)?, add_x16(t), BR_X16])
}

/// A word `br x16` may land on in a BTI output (§9).
#[must_use]
pub const fn is_landing_pad(w: u32) -> bool {
    matches!(w, BTI_C | BTI_J | BTI_JC | PACIASP | PACIBSP)
}

/// The refusal of a BTI thunk target without a landing pad (§9).
pub const BTI_LANDING_PAD: Refusal = Refusal::named(RefusalClass::Feature, "BTI landing pad");

/// The static content of a slot in a `static` output (§10.1, §12.5): one
/// or two words. `s` is the symbol's address, `None` for `weak0` (whose
/// address is 0 and whose TP and DTP offsets are 0, §7); `tls` the
/// template when the slot is a TLS one.
pub fn static_slot(
    kind: mortar_arch::SlotKind,
    s: Option<u64>,
    tls: Option<TlsBlock>,
    abi: &TlsAbi,
) -> Result<([u64; 2], usize), FieldError> {
    use mortar_arch::SlotKind as K;
    let tls = || tls.ok_or(FieldError::MissingTerm("PT_TLS"));
    Ok(match (kind, s) {
        (K::Addr | K::PltGot, s) => ([s.unwrap_or(0), 0], 1),
        (K::TpOff { negated: false }, Some(s)) => ([abi.tp_rel(s, tls()?) as u64, 0], 1),
        (K::TlsGd, Some(s)) => ([1, TlsAbi::dtp_rel(s, tls()?) as u64], 2),
        (K::TpOff { negated: false }, None) => ([0, 0], 1),
        // The module pair; for `weak0` its offset is 0.
        (K::TlsGd, None) | (K::TlsLd, _) => ([1, 0], 2),
        // Relaxed (R1) in a static output: never demanded.
        (K::TlsDesc | K::TpOff { negated: true }, _) => return Err(FieldError::MissingSlot),
    })
}

/// `R_AARCH64_IRELATIVE` (§6.2, §10.4).
pub const IRELATIVE: u32 = 1032;

/// The `Elf64_Rela` of an IPLT word (§10.4): `r_offset` the word's
/// address, no symbol, type `IRELATIVE`, addend the resolver's address
/// (`st_value` of the IFUNC's definition). The word itself holds the
/// resolver's address too (§10.2), so a loader that reads either agrees.
#[must_use]
pub fn irelative(word: u64, resolver: u64) -> [u8; 24] {
    let mut out = [0u8; 24];
    out[..8].copy_from_slice(&word.to_le_bytes());
    out[8..16].copy_from_slice(&u64::from(IRELATIVE).to_le_bytes());
    out[16..].copy_from_slice(&resolver.to_le_bytes());
    out
}

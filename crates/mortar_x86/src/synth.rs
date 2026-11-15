//! The bytes mortar writes itself: IPLT entries (x86.md §11.3) and the
//! static GOT slot and `.got.plt` contents (§11.1, §11.2). The lazy PLT
//! (§11.4) is M2 and refused by name.

use mortar_arch::{FieldError, Refusal, SlotKind, TlsAbi, TlsBlock};
use mortar_diag::RefusalClass;

/// `endbr32` (X86-49).
pub const ENDBR32: [u8; 4] = [0xf3, 0x0f, 0x1e, 0xfb];

/// The refusal of a lazy PLT header or entry (§11.4, M2).
pub const LAZY_PLT: Refusal =
    Refusal::named(RefusalClass::Feature, "lazy PLT (shared-library calls)");

/// The 6-byte indirect `jmp` of an IPLT entry through its `.got.plt` word
/// (X86-48): `ff 25 <word>` (absolute, `static`) or `ff a3 <word − GOT>`
/// (through `%ebx`, `static-pie`). Both are mod 2^32 (X86-10).
#[must_use]
pub fn jmp(word: u64, got: Option<u64>) -> [u8; 6] {
    let (modrm, d) = match got {
        None => (0x25, word as u32),
        Some(g) => (0xa3, word.wrapping_sub(g) as u32),
    };
    let d = d.to_le_bytes();
    [0xff, modrm, d[0], d[1], d[2], d[3]]
}

/// X86-48: the plain IPLT entry, the jump then `66 90`.
#[must_use]
pub fn iplt(word: u64, got: Option<u64>) -> [u8; 8] {
    let j = jmp(word, got);
    [j[0], j[1], j[2], j[3], j[4], j[5], 0x66, 0x90]
}

/// X86-49: the IBT IPLT entry, `endbr32`, the jump, then `66 0f 1f 44 00
/// 00`.
#[must_use]
pub fn iplt_ibt(word: u64, got: Option<u64>) -> [u8; 16] {
    let mut b = [0u8; 16];
    b[..4].copy_from_slice(&ENDBR32);
    b[4..10].copy_from_slice(&jmp(word, got));
    b[10..].copy_from_slice(&[0x66, 0x0f, 0x1f, 0x44, 0x00, 0x00]);
    b
}

/// Copies `bytes` into `out`, which must hold them.
pub fn put(out: &mut [u8], bytes: &[u8]) -> Result<(), FieldError> {
    let Some(dst) = out.get_mut(..bytes.len()) else {
        return Err(FieldError::Outside {
            offset: 0,
            width: u8::try_from(bytes.len()).unwrap_or(u8::MAX),
        });
    };
    dst.copy_from_slice(bytes);
    Ok(())
}

/// X86-46: the static content of a slot (one 4-byte word). `s` is the
/// slot's target address (an IFUNC's IPLT entry in `static`), `None` for
/// `weak0`, whose address and TP offset are 0 (X86-23, X86-24); `tls`
/// the template for a `GTPREL` slot. Every other kind is relaxed away in
/// a static output (X86-17) or M2.
pub fn static_slot(
    kind: SlotKind,
    s: Option<u64>,
    tls: Option<TlsBlock>,
    abi: &TlsAbi,
) -> Result<u32, FieldError> {
    Ok(match (kind, s) {
        (SlotKind::Addr | SlotKind::PltGot, s) => s.unwrap_or(0) as u32,
        (SlotKind::TpOff { negated: false }, Some(s)) => {
            abi.tp_rel(s, tls.ok_or(FieldError::MissingTerm("PT_TLS"))?) as u32
        }
        (SlotKind::TpOff { negated: false }, None) => 0,
        _ => return Err(FieldError::MissingSlot),
    })
}

/// X86-47, R-16: the three reserved words of `.got.plt`. All 0 in
/// `static`; in `static-pie` word 0 is `_DYNAMIC`'s link-time address
/// (`dynamic`), with no `RELATIVE`, and words 1 and 2 are 0.
#[must_use]
pub fn got_plt_reserved(dynamic: Option<u64>) -> [u32; 3] {
    [dynamic.map_or(0, |d| d as u32), 0, 0]
}

/// `R_386_RELATIVE` (§6.2).
pub const RELATIVE: u32 = 8;
/// `R_386_IRELATIVE` (§6.2, X86-50, X86-51).
pub const IRELATIVE: u32 = 42;

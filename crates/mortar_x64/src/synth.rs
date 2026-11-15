//! The bytes mortar writes itself: IPLT entries (x64.md §10.4) and the
//! static GOT slot contents (§10.1). The lazy PLT and `.plt.sec` (§10.3)
//! are M2 and refused by name.

use mortar_arch::{Check, FieldError, Refusal, SlotKind, TlsAbi, TlsBlock};
use mortar_diag::RefusalClass;
use mortar_elf::Class;

/// `endbr64`.
pub const ENDBR64: [u8; 4] = [0xf3, 0x0f, 0x1e, 0xfa];

/// The refusal of a lazy PLT header or entry (§10.3, M2).
pub const LAZY_PLT: Refusal = Refusal::named(RefusalClass::Feature, "lazy PLT (shared-library calls)");

/// `rel32 = word − (end)`, checked s(32).
fn rel32(word: u64, end: u64) -> Result<[u8; 4], FieldError> {
    let x = word.wrapping_sub(end) as i64;
    Check::Signed(32).verify(x, Class::Elf64)?;
    Ok((x as i32).to_le_bytes())
}

/// X-37: the plain IPLT entry at `entry` for the `.got.plt` word `word`,
/// `ff 25 <rel32> 66 90` (`jmp *word(%rip); xchg %ax,%ax`), `rel32 = word −
/// (entry + 6)`.
pub fn iplt(entry: u64, word: u64) -> Result<[u8; 8], FieldError> {
    let r = rel32(word, entry.wrapping_add(6))?;
    Ok([0xff, 0x25, r[0], r[1], r[2], r[3], 0x66, 0x90])
}

/// X-38: the IBT IPLT entry, `f3 0f 1e fa ff 25 <rel32> 66 0f 1f 44 00
/// 00` (`endbr64; jmp *word(%rip); nopw`), `rel32 = word − (entry + 10)`.
pub fn iplt_ibt(entry: u64, word: u64) -> Result<[u8; 16], FieldError> {
    let r = rel32(word, entry.wrapping_add(10))?;
    let mut b = [0u8; 16];
    b[..4].copy_from_slice(&ENDBR64);
    b[4..6].copy_from_slice(&[0xff, 0x25]);
    b[6..10].copy_from_slice(&r);
    b[10..].copy_from_slice(&[0x66, 0x0f, 0x1f, 0x44, 0x00, 0x00]);
    Ok(b)
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

/// X-35: the static content of a slot (one word). `s` is the slot's
/// target address (an IFUNC's IPLT entry), `None` for `weak0`, whose
/// address and TP offset are 0 (X-17, X-18); `tls` the template for a
/// `GTPREL` slot. Every other kind is relaxed away in a static output
/// (X-11) or M2.
pub fn static_slot(
    kind: SlotKind,
    s: Option<u64>,
    tls: Option<TlsBlock>,
    abi: &TlsAbi,
) -> Result<u64, FieldError> {
    Ok(match (kind, s) {
        (SlotKind::Addr | SlotKind::PltGot, s) => s.unwrap_or(0),
        (SlotKind::TpOff { negated: false }, Some(s)) => {
            abi.tp_rel(s, tls.ok_or(FieldError::MissingTerm("PT_TLS"))?) as u64
        }
        (SlotKind::TpOff { negated: false }, None) => 0,
        _ => return Err(FieldError::MissingSlot),
    })
}

/// `R_X86_64_IRELATIVE` (§6.2, X-39).
pub const IRELATIVE: u32 = 37;

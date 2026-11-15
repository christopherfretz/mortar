//! The bytes mortar writes itself: IPLT entries (x64.md §10.4), the lazy
//! PLT of dynamic executables (§10.3, M2) and the static GOT slot
//! contents (§10.1). The IBT lazy PLT and `.plt.sec` stay refused by name
//! (`spec/dyn.md` §15 Q4).

use mortar_arch::{Check, FieldError, Refusal, SlotKind, TlsAbi, TlsBlock};
use mortar_diag::RefusalClass;
use mortar_elf::Class;

/// `endbr64`.
pub const ENDBR64: [u8; 4] = [0xf3, 0x0f, 0x1e, 0xfa];

/// The refusal of a lazy PLT header or entry before M2 (§10.3).
pub const LAZY_PLT: Refusal =
    Refusal::named(RefusalClass::Feature, "lazy PLT (shared-library calls)");

/// The refusal of the IBT lazy PLT and `.plt.sec` (§10.3; `spec/dyn.md`
/// D-49, §15 Q4): refused at M2, whether `-z ibt`/`-z ibtplt` or a merged
/// `FEATURE_1_AND` with IBT chose the variant.
pub const IBT_PLT: Refusal = Refusal::named(
    RefusalClass::Feature,
    "IBT PLT (-z ibt, -z ibtplt, or every input marked IBT) in a dynamic executable",
);

/// `.got.plt[1]` and `.got.plt[2]` from the GOT base (`.got.plt`'s start,
/// X-36): the words PLT0 pushes and jumps through.
const GOT_PLT_1: u64 = 8;
const GOT_PLT_2: u64 = 16;

/// §10.3: PLT0 at `header`, `ff 35 <GOT+8>; ff 25 <GOT+16>; 0f 1f 40 00`
/// (`push .got.plt[1](%rip); jmp *.got.plt[2](%rip); nopl 0(%rax)`), the
/// rel32s relative to the end of their instruction (`header + 6`,
/// `header + 12`). GNU ld and lld write these bytes (`spec/dyn.md`
/// E-reloc §6, request R-X3).
pub fn plt0(header: u64, got_base: u64) -> Result<[u8; 16], FieldError> {
    let push = rel32(got_base.wrapping_add(GOT_PLT_1), header.wrapping_add(6))?;
    let jmp = rel32(got_base.wrapping_add(GOT_PLT_2), header.wrapping_add(12))?;
    let mut b = [0u8; 16];
    b[..2].copy_from_slice(&[0xff, 0x35]);
    b[2..6].copy_from_slice(&push);
    b[6..8].copy_from_slice(&[0xff, 0x25]);
    b[8..12].copy_from_slice(&jmp);
    b[12..].copy_from_slice(&[0x0f, 0x1f, 0x40, 0x00]);
    Ok(b)
}

/// §10.3: the lazy PLT entry at `entry` for the `.got.plt` word `word`
/// and `.rela.plt` index `n`, `ff 25 <word>; 68 <n>; e9 <PLT0>` (`jmp
/// *word(%rip); push $n; jmp PLT0`), the rel32s relative to `entry + 6`
/// and `entry + 16`. The word's lazy target is `entry + 6`, the `push`
/// ([`LAZY_PLUS`]).
pub fn plt_entry(entry: u64, word: u64, n: u32, header: u64) -> Result<[u8; 16], FieldError> {
    let jmp = rel32(word, entry.wrapping_add(6))?;
    let back = rel32(header, entry.wrapping_add(16))?;
    let mut b = [0u8; 16];
    b[..2].copy_from_slice(&[0xff, 0x25]);
    b[2..6].copy_from_slice(&jmp);
    b[6] = 0x68;
    b[7..11].copy_from_slice(&n.to_le_bytes());
    b[11] = 0xe9;
    b[12..].copy_from_slice(&back);
    Ok(b)
}

/// §10.3, D-49: a `JUMP_SLOT` word's lazy target is its entry plus 6,
/// the `push` (GNU ld and lld; `Arch::lazy_target`).
pub const LAZY_PLUS: u32 = 6;

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

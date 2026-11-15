//! `scan` and `apply` (arch.md §3.4, §3.6; x64.md §5 to §8, §12).
//!
//! `apply` is one pass over the `Howto`: no allocation; the table lookups
//! of `FillTables` are one `sym`, plus `slot`, `plt`, `flags` or `name`
//! where the value needs them.

use mortar_arch::seq::{field_window, in_section};
use mortar_arch::{
    Base, Check, ClassOutcome, DeadRef, DemandClass, DemandFlags, DemandSink, EventClass, Field,
    FieldError, FillCx, GlobalFlags, Incompatible, Minus, OutputKind, Refusal, RelocAt, ScanCx,
    ScanError, SectionOut, SymValue, TargetKind, Terms, TlsAbi, class_outcome, dead_reference,
    writes_dtprel,
};
use mortar_diag::RefusalClass;
use mortar_elf::Class;

use crate::forms::{
    self, DTPOFF32, DTPOFF64, Form, GOTPC32_TLSDESC, GOTPCRELX, GOTTPOFF, REX_GOTPCRELX,
    TLSDESC_CALL, TLSGD, TLSLD, rex_prime,
};
use crate::{TLS, relax};

/// X-26: a `TLSGD` lead that matches no form of §8.3 in an executable.
pub const UNRECOGNIZED_GD: &str = "unrecognized general-dynamic TLS sequence";
/// X-26: a `TLSLD` lead that matches no form of §8.4 in an executable.
pub const UNRECOGNIZED_LD: &str = "unrecognized local-dynamic TLS sequence";
/// X-28: a TLSDESC member that matches neither T1 nor T2.
pub const UNRECOGNIZED_DESC: &str = "unrecognized TLS descriptor sequence";
/// X-32: GD → IE and TLSDESC → IE, which need a TLS-dynamic target (M2).
pub const TLS_TO_IE: &str = "TLS relaxation to initial-exec";
/// The unrelaxed general-dynamic, local-dynamic and descriptor models
/// (§6.1's M2 values) in an output that is not an executable.
pub const DYNAMIC_TLS: &str = "dynamic TLS models in a shared object";
/// X-46 needs the place's `SHF_EXECINSTR`, which the caller did not give
/// (`SectionOut.exec`).
pub const DTPOFF_PLACE: &str = "DTPOFF relocation in an allocated section of unknown kind";
/// X-32: no relaxation is taken in a non-alloc section, and a static
/// output has no unrelaxed GD, LD or descriptor model.
pub const NON_ALLOC_TLS: &str = "TLS sequence in a non-allocated section";

const fn feature(name: &'static str) -> Refusal {
    Refusal::named(RefusalClass::Feature, name)
}

/// The input-only checks of arch.md §3.4 for one relocation: the field
/// and its window lie in the section. x86-64 has no addend rule (X-4) and
/// no instruction field (X-5); the type's status was checked by
/// `classify`.
pub fn scan_checks(at: &RelocAt<'_>) -> Result<(), ScanError> {
    let len = at.site.bytes.len() as u64;
    match field_window(at.r_offset, at.howto) {
        Some((start, end)) if in_section(start, end, len) => Ok(()),
        _ => Err(ScanError::Outside),
    }
}

/// A TLS class treats `weak0` as TLS-local, its TP and DTP offsets being
/// 0 (X-18).
fn effective(kind: TargetKind, class: DemandClass) -> TargetKind {
    if class.is_tls() && kind == TargetKind::Weak0 { TargetKind::Local } else { kind }
}

/// TLS-local in an executable: defined in this output (or `weak0`,
/// already mapped by [`effective`]).
const fn tls_local(kind: TargetKind) -> bool {
    matches!(kind, TargetKind::Local | TargetKind::Abs | TargetKind::Weak0)
}

/// Whether the value uses the GOT base (`GOTPC*`, `GOTOFF64`, `GOT64`,
/// `PLTOFF64`), which then must exist.
const fn uses_got_base(h: &mortar_arch::Howto) -> bool {
    matches!(h.value.base, Base::GotBase)
        || matches!(h.value.minus, Minus::GotBase | Minus::PageGotBase)
}

/// I1/I2 (X-23): decided from the output kind, the target kind and the
/// input bytes, so scan and fill agree (kind 1).
fn ie_relaxed(at: &RelocAt<'_>, output: OutputKind, kind: TargetKind, alloc: bool) -> Option<Form> {
    // X-32: never in a non-alloc section.
    if !alloc || !output.is_exec() || at.addend != -4 || !tls_local(kind) {
        return None;
    }
    forms::form(GOTTPOFF, at.site).filter(|f| matches!(f, Form::I1 { .. } | Form::I2 { .. }))
}

/// T1 (with `A = −4`) or T2 (X-28).
fn desc_form(at: &RelocAt<'_>) -> Option<Form> {
    match forms::form(at.r_type, at.site)? {
        f @ Form::T1 { .. } if at.addend == -4 => Some(f),
        f @ Form::T2 => Some(f),
        _ => None,
    }
}

/// `scan` (arch.md §3.4): the checks, then the class rule of §4.3 (X-10,
/// X-11), X-15 for `Pc` against `weak0`, and the TLS rules of §8.
pub fn scan(cx: &ScanCx<'_>, at: &RelocAt<'_>, out: &mut DemandSink<'_>) {
    if let Err(e) = scan_checks(at) {
        out.error(at.index, e);
        return;
    }
    let h = at.howto;
    // X-25: a consumed follower makes no reference and demands nothing.
    if cx.output.is_exec() && relax::follower_of(at, cx.tables).is_some() {
        return;
    }
    let sym = cx.tables.sym(at.key);
    if sym.gnu_warning {
        out.reference(at.index, EventClass::GnuWarning, at.key);
    }
    // arch.md §3.4: a dead target demands nothing; the referring section
    // decides whether the field is the tombstone, 0, or an error (X-53).
    if sym.dead {
        if dead_reference(cx.name, cx.place.alloc) == DeadRef::Discarded {
            out.reference(at.index, EventClass::DiscardedSection, at.key);
        }
        return;
    }
    let kind = effective(sym.kind, h.class);
    let data = matches!(h.field, Field::Data { .. });
    match class_outcome(h.class, data, cx.output, kind, cx.place, sym.tls) {
        ClassOutcome::Demand(d) => {
            out.raise(at.key, d.flags);
            let base = if uses_got_base(h) { GlobalFlags::GOT_BASE } else { GlobalFlags::empty() };
            out.raise_global(d.global | base);
            if let Some(role) = d.dynamic {
                out.count(role, 1);
            }
        }
        ClassOutcome::Incompatible(r) => out.error(at.index, ScanError::Incompatible(r)),
        ClassOutcome::Undefined => {
            let class =
                if sym.comdat_only { EventClass::ComdatMismatch } else { EventClass::Undefined };
            out.reference(at.index, class, at.key);
        }
        ClassOutcome::Impossible => out.error(at.index, ScanError::Internal),
        // X-15: `Pc` against `weak0` in `Pie` or `Shared` is `E`, with the
        // reasons of arch.md §10's `E` cells.
        ClassOutcome::ArchWeak0 => {
            let r = if cx.output == OutputKind::Pie {
                Incompatible::RecompilePie
            } else {
                Incompatible::RecompilePic
            };
            out.error(at.index, ScanError::Incompatible(r));
        }
        ClassOutcome::ArchTls => match tls_scan(cx, at, kind) {
            Ok((flags, global)) => {
                out.raise(at.key, flags);
                out.raise_global(global);
            }
            Err(r) => out.error(at.index, ScanError::Refused(r)),
        },
    }
}

/// The TLS-model demands (X-11, §8.2 to §8.5). In an executable a
/// matched GD or LD sequence and a matched TLSDESC member against a
/// TLS-local target demand nothing (kind 1); an unmatched one is refused
/// by name; IE keeps its `GTPREL` slot unless I1/I2 is taken. Outside an
/// executable only IE is accepted (M2 for the others).
fn tls_scan(
    cx: &ScanCx<'_>,
    at: &RelocAt<'_>,
    kind: TargetKind,
) -> Result<(DemandFlags, GlobalFlags), Refusal> {
    let none = (DemandFlags::empty(), GlobalFlags::empty());
    if !cx.output.is_exec() {
        return match at.r_type {
            GOTTPOFF => Ok((DemandFlags::GOT_TPREL, GlobalFlags::STATIC_TLS)),
            _ => Err(feature(DYNAMIC_TLS)),
        };
    }
    if !cx.place.alloc && matches!(at.r_type, TLSGD | TLSLD | GOTPC32_TLSDESC | TLSDESC_CALL) {
        return Err(feature(NON_ALLOC_TLS));
    }
    match at.r_type {
        TLSGD => match relax::lead_form(at, cx.tables) {
            None => Err(feature(UNRECOGNIZED_GD)),
            Some(_) if tls_local(kind) => Ok(none),
            Some(_) => Err(feature(TLS_TO_IE)),
        },
        // The LE outputs of §8.4 do not depend on the symbol.
        TLSLD => relax::lead_form(at, cx.tables).map(|_| none).ok_or(feature(UNRECOGNIZED_LD)),
        GOTTPOFF if ie_relaxed(at, cx.output, kind, cx.place.alloc).is_some() => Ok(none),
        GOTTPOFF => Ok((DemandFlags::GOT_TPREL, GlobalFlags::empty())),
        GOTPC32_TLSDESC | TLSDESC_CALL => match desc_form(at) {
            None => Err(feature(UNRECOGNIZED_DESC)),
            Some(_) if tls_local(kind) => Ok(none),
            Some(_) => Err(feature(TLS_TO_IE)),
        },
        // Every TLS-model type of §6.1 that reaches the architecture's
        // table is above; the later types were refused by `classify`.
        _ => Err(Refusal::named(RefusalClass::RelocationType, at.howto.name)),
    }
}

/// Writes `bytes` at section offset `off`, which must lie in the input
/// section (`at.site`); `out` keeps the part inside its clip (arch.md
/// §3.6).
#[inline]
fn put(
    at: &RelocAt<'_>,
    out: &mut SectionOut<'_>,
    off: u64,
    bytes: &[u8],
) -> Result<(), FieldError> {
    let width = u8::try_from(bytes.len()).unwrap_or(u8::MAX);
    match off.checked_add(bytes.len() as u64) {
        Some(end) if end <= at.site.bytes.len() as u64 => {
            out.write(off, bytes);
            Ok(())
        }
        _ => Err(FieldError::Outside { offset: off, width }),
    }
}

/// `off − n`, the start of a form's bytes before `P`.
fn back(at: &RelocAt<'_>, n: u64) -> Result<u64, FieldError> {
    at.r_offset.checked_sub(n).ok_or(FieldError::Outside { offset: at.r_offset, width: 4 })
}

fn checked(check: Check, x: i64) -> Result<i64, FieldError> {
    check.verify(x, Class::Elf64)?;
    Ok(x)
}

/// A TLS offset of `S + A` by `f` (`TpRel` or `DtpRel`), and `A` itself
/// for `weak0` (X-18: its TP and DTP offsets are 0; no template needed).
fn tls_off(
    cx: &FillCx<'_>,
    sym: SymValue,
    a: i64,
    f: impl Fn(u64, mortar_arch::TlsBlock) -> i64,
) -> Result<i64, FieldError> {
    if sym.kind == TargetKind::Weak0 {
        return Ok(a);
    }
    let tls = cx.tls.ok_or(FieldError::MissingTerm("PT_TLS"))?;
    Ok(f(sym.s.wrapping_add(a as u64), tls))
}

/// `T` of §8.2 to §8.5: `TpRel(S)`, 0 for a TLS `weak0` (X-18), checked
/// s(32) (X-45).
fn t_value(cx: &FillCx<'_>, sym: SymValue) -> Result<[u8; 4], FieldError> {
    let t = checked(Check::Signed(32), tls_off(cx, sym, 0, |x, tls| TLS.tp_rel(x, tls))?)?;
    Ok((t as i32).to_le_bytes())
}

/// `apply` (arch.md §3.6).
pub fn apply(
    cx: &FillCx<'_>,
    at: &RelocAt<'_>,
    out: &mut SectionOut<'_>,
) -> Result<(), FieldError> {
    let h = at.howto;
    let exec = cx.output.is_exec();
    // X-25: the lead writes the follower's bytes; the follower nothing.
    if exec && relax::follower_of(at, cx.tables).is_some() {
        return Ok(());
    }
    let sym = cx.tables.sym(at.key);
    // X-53 (arch.md §3.11): against a dead target a debug section gets the
    // tombstone and `.gcc_except_table` 0; scan reported any other section.
    if !sym.live {
        let x = match dead_reference(out.name, out.alloc) {
            DeadRef::Tombstone => mortar_arch::tombstone(out.name, Class::Elf64),
            DeadRef::Zero => 0,
            DeadRef::Discarded => return Err(FieldError::MissingTerm("live target")),
        };
        let width = usize::from(h.field.width());
        return put(at, out, at.r_offset, &x.to_le_bytes()[..width]);
    }
    let kind = effective(sym.kind, h.class);
    match at.r_type {
        TLSGD | TLSLD | GOTPC32_TLSDESC | TLSDESC_CALL if !exec => {
            return Err(FieldError::Refused(feature(DYNAMIC_TLS)));
        }
        TLSGD | TLSLD | GOTPC32_TLSDESC | TLSDESC_CALL if !out.alloc => {
            return Err(FieldError::Refused(feature(NON_ALLOC_TLS)));
        }
        TLSGD | TLSLD => return lead(cx, at, out, sym, kind),
        GOTPC32_TLSDESC | TLSDESC_CALL => return desc(cx, at, out, sym, kind),
        GOTTPOFF => {
            if let Some(f) = ie_relaxed(at, cx.output, kind, out.alloc) {
                return ie(cx, at, out, sym, f);
            }
        }
        GOTPCRELX | REX_GOTPCRELX => {
            // X-32: in a non-alloc section, applied as its §6.1 row.
            if out.alloc
                && let Some((off, bytes, n)) = got_relaxed(cx.output, at, out.addr, sym)
            {
                return put(at, out, off, &bytes[..n]);
            }
        }
        _ => {}
    }
    let width = usize::from(h.field.width());
    if width == 0 {
        return Ok(());
    }
    let x = value(cx, at, out, sym)?;
    put(at, out, at.r_offset, &x.to_le_bytes()[..width])
}

/// `X` of the §6.1 row (unrelaxed), checked.
#[allow(clippy::many_single_char_names)] // the spec's S, A, P, X
fn value(
    cx: &FillCx<'_>,
    at: &RelocAt<'_>,
    out: &SectionOut<'_>,
    sym: SymValue,
) -> Result<i64, FieldError> {
    let h = at.howto;
    let a = at.addend;
    let p = out.addr.wrapping_add(at.r_offset);
    // X-53 (arch.md §4.3): `64`/`32`/… against a TLS symbol at a non-alloc
    // place is the offset in the TLS block.
    if writes_dtprel(
        h.class,
        mortar_arch::PlaceFacts { alloc: out.alloc, writable: false },
        sym.tls,
    ) {
        return checked(h.check, tls_off(cx, sym, a, TlsAbi::dtp_rel)?);
    }
    // §7 (`Facts`' `weak0` rule): `S = 0` for every class.
    let s = if sym.kind == TargetKind::Weak0 { 0 } else { sym.s };
    let b = match h.value.base {
        Base::Zero => 0,
        Base::S => s,
        Base::L => {
            if cx.tables.flags(at.key).contains(DemandFlags::PLT) {
                cx.tables.plt(at.key).ok_or(FieldError::MissingTerm("PLT entry"))?
            } else {
                s
            }
        }
        Base::G(k) => cx.tables.slot(at.key, k).ok_or(FieldError::MissingSlot)?,
        Base::GotBase => cx.got_base.ok_or(FieldError::MissingTerm("GOT base"))?,
        Base::TpRel => tls_off(cx, sym, a, |x, tls| TLS.tp_rel(x, tls))? as u64,
        Base::DtpRel => dtpoff(cx, at, out, sym)? as u64,
        Base::Size => sym.size,
    };
    let got = match h.value.minus {
        Minus::GotBase | Minus::PageGotBase => {
            cx.got_base.ok_or(FieldError::MissingTerm("GOT base"))?
        }
        _ => 0,
    };
    checked(h.check, h.value.eval(Terms { b, a, p, got }, 0))
}

/// X-46: `DTPOFF32`/`DTPOFF64` are `TpRel(S + A)` in an `SHF_EXECINSTR`
/// section of an executable (where a relaxed LD sequence or a
/// `_TLS_MODULE_BASE_` descriptor leaves TP as the module base), and
/// `DtpRel(S + A)` everywhere else.
fn dtpoff(
    cx: &FillCx<'_>,
    at: &RelocAt<'_>,
    out: &SectionOut<'_>,
    sym: SymValue,
) -> Result<i64, FieldError> {
    debug_assert!(matches!(at.r_type, DTPOFF32 | DTPOFF64), "only DTPOFF* have a DtpRel base");
    let code = if out.alloc && cx.output.is_exec() {
        out.exec.ok_or(FieldError::Refused(feature(DTPOFF_PLACE)))?
    } else {
        false
    };
    if code {
        tls_off(cx, sym, at.addend, |x, tls| TLS.tp_rel(x, tls))
    } else {
        tls_off(cx, sym, at.addend, TlsAbi::dtp_rel)
    }
}

/// D16 (§8): `mov %fs:0, %rax; lea T(%rax), %rax`, `T` at byte 12.
const D16: [u8; 12] = [0x64, 0x48, 0x8b, 0x04, 0x25, 0, 0, 0, 0, 0x48, 0x8d, 0x80];
/// D22's tail after `T`: `nopw 0(%rax,%rax)`.
const NOPW6: [u8; 6] = [0x66, 0x0f, 0x1f, 0x44, 0x00, 0x00];
/// LD12, LD13, LD22 (§8.4).
pub const LD12: [u8; 12] = [0x66, 0x66, 0x66, 0x64, 0x48, 0x8b, 0x04, 0x25, 0, 0, 0, 0];
pub const LD13: [u8; 13] = [0x66, 0x66, 0x66, 0x66, 0x64, 0x48, 0x8b, 0x04, 0x25, 0, 0, 0, 0];
pub const LD22: [u8; 22] = [
    0x66, 0x66, 0x66, 0x66, 0x2e, 0x0f, 0x1f, 0x84, 0, 0, 0, 0, 0, 0x64, 0x48, 0x8b, 0x04, 0x25, 0,
    0, 0, 0,
];

/// A `TLSGD` or `TLSLD` lead in an executable (X-25, X-27): its LE output
/// over the form's window, or refused by name (X-26).
fn lead(
    cx: &FillCx<'_>,
    at: &RelocAt<'_>,
    out: &mut SectionOut<'_>,
    sym: SymValue,
    kind: TargetKind,
) -> Result<(), FieldError> {
    let gd = at.r_type == TLSGD;
    let Some(form) = relax::lead_form(at, cx.tables) else {
        return Err(FieldError::Refused(feature(if gd {
            UNRECOGNIZED_GD
        } else {
            UNRECOGNIZED_LD
        })));
    };
    let mut buf = [0u8; 22];
    let (start, n) = match form {
        Form::Gd16 | Form::Gd16x | Form::Gd22 => {
            if !tls_local(kind) {
                return Err(FieldError::Refused(feature(TLS_TO_IE)));
            }
            buf[..12].copy_from_slice(&D16);
            buf[12..16].copy_from_slice(&t_value(cx, sym)?);
            if form == Form::Gd22 {
                buf[16..22].copy_from_slice(&NOPW6);
                (back(at, 3)?, 22)
            } else {
                (back(at, 4)?, 16)
            }
        }
        Form::Ld12 => {
            buf[..12].copy_from_slice(&LD12);
            (back(at, 3)?, 12)
        }
        Form::Ld13 => {
            buf[..13].copy_from_slice(&LD13);
            (back(at, 3)?, 13)
        }
        Form::Ld22 => {
            buf.copy_from_slice(&LD22);
            (back(at, 3)?, 22)
        }
        _ => return Err(FieldError::MissingTerm("sequence form")),
    };
    put(at, out, start, &buf[..n])
}

/// A TLSDESC member in an executable (X-28, X-47): T1 → `REX' c7 (c0 |
/// reg) T`, `T = 0` for `_TLS_MODULE_BASE_`; T2 → `66 90`.
fn desc(
    cx: &FillCx<'_>,
    at: &RelocAt<'_>,
    out: &mut SectionOut<'_>,
    sym: SymValue,
    kind: TargetKind,
) -> Result<(), FieldError> {
    let Some(form) = desc_form(at) else {
        return Err(FieldError::Refused(feature(UNRECOGNIZED_DESC)));
    };
    if !tls_local(kind) {
        return Err(FieldError::Refused(feature(TLS_TO_IE)));
    }
    match form {
        Form::T1 { rex, reg } => {
            let base = TLS.module_base.is_some_and(|b| cx.tables.name(at.key) == b);
            let t = if base { [0; 4] } else { t_value(cx, sym)? };
            let b = [rex_prime(rex), 0xc7, 0xc0 | reg, t[0], t[1], t[2], t[3]];
            put(at, out, back(at, 3)?, &b)
        }
        _ => put(at, out, at.r_offset, &[0x66, 0x90]),
    }
}

/// I1/I2 (X-23): `REX' c7 (c0 | reg) T` (mov) or `REX' 81 (c0 | reg) T`
/// (add: result and flags as the input's, D1).
fn ie(
    cx: &FillCx<'_>,
    at: &RelocAt<'_>,
    out: &mut SectionOut<'_>,
    sym: SymValue,
    form: Form,
) -> Result<(), FieldError> {
    let (rex, op, reg) = match form {
        Form::I1 { rex, reg } => (rex, 0xc7, reg),
        Form::I2 { rex, reg } => (rex, 0x81, reg),
        _ => return Err(FieldError::MissingTerm("IE form")),
    };
    let t = t_value(cx, sym)?;
    put(at, out, back(at, 3)?, &[rex_prime(rex), op, 0xc0 | reg, t[0], t[1], t[2], t[3]])
}

/// G1 to G6 (X-19, X-20; kind 2): the bytes to write and where, when the
/// form matches and its condition holds; `None` keeps the GOT load.
#[allow(clippy::many_single_char_names)]
fn got_relaxed(
    output: OutputKind,
    at: &RelocAt<'_>,
    addr: u64,
    sym: SymValue,
) -> Option<(u64, [u8; 7], usize)> {
    // X-19, X-22: a `local` target (not `abs`, `ifunc`, `weak0`), `A = −4`.
    if sym.kind != TargetKind::Local || at.addend != -4 {
        return None;
    }
    let form = forms::form(at.r_type, at.site)?;
    let p = addr.wrapping_add(at.r_offset);
    let s = sym.s;
    // `S + A − P`, in s(32).
    let pc = |bias: i64| -> Option<[u8; 4]> {
        let x = s.wrapping_add(at.addend as u64).wrapping_sub(p).wrapping_add(bias as u64) as i64;
        let x = i32::try_from(x).ok()?;
        Some(x.to_le_bytes())
    };
    // X-20: `S` in [0, 2^31), static non-PIE only.
    let imm = || -> Option<[u8; 4]> {
        (output == OutputKind::Static && s < 1 << 31).then(|| (s as u32).to_le_bytes())
    };
    let mut b = [0u8; 7];
    let mut start = at.r_offset.checked_sub(u64::from(form.window().before))?;
    let n = match form {
        // `8d` at `P − 2` (lea), the ModRM kept; the field `S + A − P`.
        Form::G1 { .. } | Form::G2 => {
            let x = pc(0)?;
            start = at.r_offset.checked_sub(2)?;
            b[0] = 0x8d;
            b[1] = *at.site.bytes.get(usize::try_from(at.r_offset - 1).ok()?)?;
            b[2..6].copy_from_slice(&x);
            6
        }
        // `67 e8` (addr32 call).
        Form::G3 => {
            let x = pc(0)?;
            b[..2].copy_from_slice(&[0x67, 0xe8]);
            b[2..6].copy_from_slice(&x);
            6
        }
        // `e9` at `P − 2`, `S + A − P + 1` at `P − 1`, `90` at `P + 3`.
        Form::G4 => {
            let x = pc(1)?;
            b[0] = 0xe9;
            b[1..5].copy_from_slice(&x);
            b[5] = 0x90;
            6
        }
        Form::G5 { rex, op, reg } => {
            let x = imm()?;
            b[..3].copy_from_slice(&[rex_prime(rex), 0x81, 0xc0 | (op & 0x38) | reg]);
            b[3..7].copy_from_slice(&x);
            7
        }
        Form::G6 { rex, reg } => {
            let x = imm()?;
            b[..3].copy_from_slice(&[rex_prime(rex), 0xf7, 0xc0 | reg]);
            b[3..7].copy_from_slice(&x);
            7
        }
        _ => return None,
    };
    Some((start, b, n))
}

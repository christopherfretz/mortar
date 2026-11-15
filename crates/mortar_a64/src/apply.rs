//! `scan` and `apply` (arch.md §3.4, §3.6; a64.md §5 to §8).
//!
//! `apply` is one pass over the `Howto`: no allocation, no dynamic
//! dispatch beyond the table lookups of `FillTables` (one `sym`, plus
//! `slot`, `plt`, `flags` or `redirect` where the value needs them).

use mortar_arch::seq::{field_window, in_section};
use mortar_arch::{
    AddendRule, Base, ClassOutcome, DemandClass, DemandFlags, DemandSink, Field, FieldError,
    FillCx, Incompatible, Minus, Refusal, RelocAt, ScanCx, ScanError, SectionOut, TargetKind,
    Terms, Weak0Rule, class_outcome,
};
use mortar_diag::RefusalClass;

use crate::forms::{self, B26};
use crate::synth::{NOP, checked};
use crate::{TLS, weak0_rule};

/// A thread-local undefined weak (§7).
pub const WEAK_TLS: Refusal =
    Refusal::named(RefusalClass::Feature, "undefined weak thread-local symbol");

/// R2 (TLSDESC → IE) is M2 and not yet written (stage B of WP-023).
pub const R2_PENDING: Refusal =
    Refusal::named(RefusalClass::Feature, "TLSDESC to initial-exec relaxation");

/// R4 (traditional GD → LE/IE) is M2 and not yet written.
pub const R4_PENDING: Refusal =
    Refusal::named(RefusalClass::Feature, "traditional general-dynamic TLS in an executable");

/// The input-only checks of arch.md §3.4 for one relocation: the field and
/// its window lie in the section; the addend rule (a64.md §3.1); the
/// instruction the field form requires (§3.2). The type's status was
/// checked by `classify`.
pub fn scan_checks(at: &RelocAt<'_>) -> Result<(), ScanError> {
    let h = at.howto;
    let len = at.site.bytes.len() as u64;
    match field_window(at.r_offset, h) {
        Some((start, end)) if in_section(start, end, len) => {}
        _ => return Err(ScanError::Outside),
    }
    if h.addend == AddendRule::MustBeZero && at.addend != 0 {
        return Err(ScanError::Incompatible(Incompatible::AddendMustBeZero));
    }
    if let Field::Insn { form } = h.field {
        let w = at.site.word(0).ok_or(ScanError::Outside)?;
        if !forms::requires(form, w) {
            return Err(ScanError::Incompatible(Incompatible::InstructionMismatch));
        }
    }
    Ok(())
}

/// `scan` (arch.md §3.4): the checks, then the class rule of §4.3 and the
/// TLS table of a64.md §8 and §12.
pub fn scan(cx: &ScanCx<'_>, at: &RelocAt<'_>, out: &mut DemandSink<'_>) {
    if let Err(e) = scan_checks(at) {
        out.error(at.index, e);
        return;
    }
    let h = at.howto;
    let sym = cx.tables.sym(at.key);
    if h.class.is_tls() && sym.kind == TargetKind::Weak0 && cx.place.alloc {
        out.error(at.index, ScanError::Refused(WEAK_TLS));
        return;
    }
    let data = matches!(h.field, Field::Data { .. });
    match class_outcome(h.class, data, cx.output, sym.kind, cx.place, sym.tls) {
        ClassOutcome::Demand(d) => {
            out.raise(at.key, d.flags);
            out.raise_global(d.global);
            if let Some(role) = d.dynamic {
                out.count(role, 1);
            }
        }
        ClassOutcome::Incompatible(r) => out.error(at.index, ScanError::Incompatible(r)),
        ClassOutcome::Undefined => out.error(at.index, ScanError::Undefined),
        ClassOutcome::Impossible => out.error(at.index, ScanError::Internal),
        ClassOutcome::ArchTls => {
            let (flags, global) = tls_demand(h.class, cx.output, sym.kind);
            out.raise(at.key, flags);
            out.raise_global(global);
        }
    }
}

/// The TLS-model demands (a64.md §8.1 to §8.4, §12): exec outputs relax
/// TLSDESC against a TLS-local target to LE (nothing) and against a
/// TLS-dynamic one to IE (`GOT_TPREL`); IE always keeps its slot (R3 is
/// kind 2); GD and LD demand their pairs (R4 is stage B).
fn tls_demand(
    class: DemandClass,
    output: mortar_arch::OutputKind,
    kind: TargetKind,
) -> (DemandFlags, mortar_arch::GlobalFlags) {
    use mortar_arch::GlobalFlags as GF;
    let exec = output.is_exec();
    let local = tls_local(kind);
    match class {
        DemandClass::TlsDesc if exec && local => (DemandFlags::empty(), GF::empty()),
        DemandClass::TlsDesc | DemandClass::TlsIe if exec => (DemandFlags::GOT_TPREL, GF::empty()),
        DemandClass::TlsDesc => (DemandFlags::TLS_DESC, GF::empty()),
        DemandClass::TlsIe => (DemandFlags::GOT_TPREL, GF::STATIC_TLS),
        DemandClass::TlsGd => (DemandFlags::TLS_GD, GF::empty()),
        DemandClass::TlsLd => (DemandFlags::empty(), GF::TLS_LD),
        _ => (DemandFlags::empty(), GF::empty()),
    }
}

/// TLS-local (a64.md §8): defined in this output, its TP offset a
/// link-time constant (the caller has checked the output is an exec).
const fn tls_local(kind: TargetKind) -> bool {
    matches!(kind, TargetKind::Local | TargetKind::Abs)
}

/// The bytes of the field at `r_offset`.
fn field_mut<'o>(
    out: &'o mut SectionOut<'_>,
    off: u64,
    width: u8,
) -> Result<&'o mut [u8], FieldError> {
    let outside = FieldError::Outside { offset: off, width };
    let start = usize::try_from(off).map_err(|_| outside)?;
    let end = start.checked_add(usize::from(width)).ok_or(outside)?;
    out.bytes.get_mut(start..end).ok_or(outside)
}

fn put_word(out: &mut SectionOut<'_>, off: u64, w: u32) -> Result<(), FieldError> {
    field_mut(out, off, 4)?.copy_from_slice(&w.to_le_bytes());
    Ok(())
}

/// The input word at the place, checked against its form.
fn input_word(at: &RelocAt<'_>, form: u16) -> Result<u32, FieldError> {
    let w = at.site.word(0).ok_or(FieldError::Outside { offset: at.r_offset, width: 4 })?;
    if forms::requires(form, w) { Ok(w) } else { Err(FieldError::Instruction { word: w }) }
}

/// `apply` (arch.md §3.6).
#[allow(clippy::many_single_char_names)] // the spec's S, A, P, X, w
pub fn apply(
    cx: &FillCx<'_>,
    at: &RelocAt<'_>,
    out: &mut SectionOut<'_>,
) -> Result<(), FieldError> {
    let h = at.howto;
    let off = at.r_offset;
    let p = out.addr.wrapping_add(off);
    let sym = cx.tables.sym(at.key);
    let weak0 = sym.kind == TargetKind::Weak0;
    if h.class.is_tls() && weak0 && out.alloc {
        return Err(FieldError::Refused(WEAK_TLS));
    }
    if cx.output.is_exec() {
        match h.class {
            DemandClass::TlsDesc => return tlsdesc_exec(cx, at, out, sym.s, sym.kind),
            DemandClass::TlsGd => return Err(FieldError::Refused(R4_PENDING)),
            _ => {}
        }
    }
    let form = match h.field {
        Field::None => return Ok(()),
        Field::Data { .. } => None,
        Field::Insn { form } => Some(form),
    };
    let w = match form {
        Some(f) => input_word(at, f)?,
        None => 0,
    };

    // §13.2: a non-alloc place against a dead target gets the tombstone.
    let x = if !out.alloc && !sym.live {
        mortar_arch::tombstone(out.name) as i64
    } else {
        value(cx, at, sym, weak0, p, form)?
    };

    match form {
        None => {
            let width = h.field.width();
            let bytes = x.to_le_bytes();
            field_mut(out, off, width)?.copy_from_slice(&bytes[..usize::from(width)]);
            Ok(())
        }
        Some(f) => put_word(out, off, forms::insert(f, w, x)),
    }
}

/// `X` for the ordinary (unrelaxed) form, checked.
#[allow(clippy::many_single_char_names)]
fn value(
    cx: &FillCx<'_>,
    at: &RelocAt<'_>,
    sym: mortar_arch::SymValue,
    weak0: bool,
    p: u64,
    form: Option<u16>,
) -> Result<i64, FieldError> {
    let h = at.howto;
    let mut s = sym.s;
    if weak0 {
        match weak0_rule(h) {
            Weak0Rule::SZero => s = 0,
            Weak0Rule::SIsP => s = p,
            Weak0Rule::XZero => return Ok(0),
            Weak0Rule::XConst(c) => return Ok(c),
        }
    }
    // A branch with a veneer redirected to its thunk (§9): `X = thunk − P`.
    if form == Some(B26)
        && !weak0
        && let Some(t) = cx.tables.redirect(at.section, at.r_offset)
    {
        return checked(h.check, h.align, t.wrapping_sub(p) as i64);
    }
    let a = at.addend;
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
        Base::TpRel => {
            let tls = cx.tls.ok_or(FieldError::MissingTerm("PT_TLS"))?;
            TLS.tp_rel(s.wrapping_add(a as u64), tls) as u64
        }
        Base::DtpRel => {
            let tls = cx.tls.ok_or(FieldError::MissingTerm("PT_TLS"))?;
            mortar_arch::TlsAbi::dtp_rel(s.wrapping_add(a as u64), tls) as u64
        }
        Base::Size => sym.size,
    };
    let got = match h.value.minus {
        Minus::GotBase | Minus::PageGotBase => {
            cx.got_base.ok_or(FieldError::MissingTerm("GOT base"))?
        }
        _ => 0,
    };
    let x = h.value.eval(Terms { b, a, p, got }, crate::FACTS.page_mask);
    checked(h.check, h.align, x)
}

/// TLSDESC in an exec output (a64.md §8.1 R1, R1t, R1l; §8.2 R2 is stage
/// B): against a TLS-local target every member is rewritten on its own.
fn tlsdesc_exec(
    cx: &FillCx<'_>,
    at: &RelocAt<'_>,
    out: &mut SectionOut<'_>,
    s: u64,
    kind: TargetKind,
) -> Result<(), FieldError> {
    let h = at.howto;
    if let Field::Insn { form } = h.field {
        input_word(at, form)?;
    }
    if !tls_local(kind) {
        // R2 against a TLS-dynamic target; the large form stays a
        // descriptor (§8.2), which an exec output only has from M2.
        return Err(FieldError::Refused(R2_PENDING));
    }
    let tls = cx.tls.ok_or(FieldError::MissingTerm("PT_TLS"))?;
    let t = TLS.tp_rel(s.wrapping_add(at.addend as u64), tls);
    let word = match at.r_type {
        // ADR_PAGE21, LD_PREL19 (tiny), OFF_G1 (large): movz x0, #T[31:16], lsl #16.
        562 | 560 | 565 => {
            checked(mortar_arch::Check::Unsigned(32), 1, t)?;
            0xD2A0_0000 | (((t as u64 >> 16) & 0xFFFF) as u32) << 5
        }
        // LD64_LO12, ADR_PREL21 (tiny), OFF_G0_NC (large): movk x0, #T[15:0].
        563 | 561 | 566 => 0xF280_0000 | ((t as u64 & 0xFFFF) as u32) << 5,
        // ADD_LO12, CALL, LDR, ADD: nop.
        _ => NOP,
    };
    put_word(out, at.r_offset, word)
}

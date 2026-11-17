//! `scan` and `apply` (arch.md §3.4, §3.6; x86.md §3 to §9, §13).
//!
//! Every implicit addend is read from the input bytes (X86-5): from
//! `at.site`, which holds them at scan and at fill, else through
//! `SectionOut::input`; never from bytes a relocation has written. A unit
//! reads every byte it needs before it writes any (X86-5, R-13).

use mortar_arch::seq::{field_window, in_section};
use mortar_arch::{
    Base, Check, ClassOutcome, DeadRef, DemandClass, DemandFlags, DemandSink, EventClass, Field,
    FieldError, FillCx, GlobalFlags, Incompatible, Minus, OutputKind, Refusal, RelocAt, ScanCx,
    ScanError, SectionOut, SlotKind, SymValue, TargetKind, Terms, class_outcome, dead_reference,
    writes_dtprel,
};
use mortar_diag::RefusalClass;
use mortar_elf::Class;

use crate::forms::{
    self, Form, GOT32, GOT32X, GOTOFF, Operand, PC8, PC16, PC32, TLS_DESC_CALL, TLS_GD,
    TLS_GOTDESC, TLS_GOTIE, TLS_IE, TLS_LDM, TLS_LDO_32,
};
use crate::{TLS, relax};

/// X86-36: a `TLS_GD` lead that matches no form of §9 in an executable.
pub const UNRECOGNIZED_GD: &str = "unrecognized general-dynamic TLS sequence";
/// X86-36: a `TLS_LDM` lead that matches no form of §9 in an executable.
pub const UNRECOGNIZED_LD: &str = "unrecognized local-dynamic TLS sequence";
/// X86-36, X86-37: a descriptor member that matches neither T1 nor T2.
pub const UNRECOGNIZED_DESC: &str = "unrecognized TLS descriptor sequence";
/// GD → IE and TLSDESC → IE, which need a TLS-dynamic target (M2).
pub const TLS_TO_IE: &str = "TLS relaxation to initial-exec";
/// The unrelaxed general-dynamic, local-dynamic and descriptor models
/// (§6.1's M2 values) in an output that is not an executable.
pub const DYNAMIC_TLS: &str = "dynamic TLS models in a shared object";
/// X86-61 needs the place's `SHF_EXECINSTR`, which the caller did not give
/// (`SectionOut.exec`).
pub const DTPOFF_PLACE: &str = "DTPOFF relocation in an allocated section of unknown kind";
/// X86-13 step 1 needs the place's `SHF_EXECINSTR`, which the caller did
/// not give (`SectionOut.exec`).
pub const GOT_PLACE: &str = "GOT32 relocation in a section of unknown kind";
/// No relaxation is taken in a non-alloc section, and a static output has
/// no unrelaxed GD, LD or descriptor model.
pub const NON_ALLOC_TLS: &str = "TLS sequence in a non-allocated section";
/// X86-18, X86-33: an unrelaxed `TLS_IE` in static-pie (scan refuses it
/// as `ReadOnlySection`; `apply` never writes it).
pub const IE_TEXT_RELOCATION: &str = "R_386_TLS_IE without its relaxation in static-pie";

/// X86-50: the name of the refusal of `GOTOFF` or a `Pc` type against an
/// IFUNC in static-pie.
#[must_use]
pub const fn ifunc_pie(r_type: u32) -> &'static str {
    match r_type {
        GOTOFF => "R_386_GOTOFF against an IFUNC in static-pie",
        PC16 => "R_386_PC16 against an IFUNC in static-pie",
        PC8 => "R_386_PC8 against an IFUNC in static-pie",
        _ => "R_386_PC32 against an IFUNC in static-pie",
    }
}

const fn feature(name: &'static str) -> Refusal {
    Refusal::named(RefusalClass::Feature, name)
}

/// The input-only checks of arch.md §3.4 for one relocation: the field
/// and its read window lie in the section. The type's status was checked
/// by `classify`.
pub fn scan_checks(at: &RelocAt<'_>) -> Result<(), ScanError> {
    let len = at.site.bytes.len() as u64;
    match field_window(at.r_offset, at.howto) {
        Some((start, end)) if in_section(start, end, len) => Ok(()),
        _ => Err(ScanError::Outside),
    }
}

/// A TLS class treats `weak0` as TLS-local, its TP and DTP offsets being
/// 0 (X86-23).
fn effective(kind: TargetKind, class: DemandClass) -> TargetKind {
    if class.is_tls() && kind == TargetKind::Weak0 { TargetKind::Local } else { kind }
}

/// TLS-local in an executable: defined in this output (or `weak0`,
/// already mapped by [`effective`]).
const fn tls_local(kind: TargetKind) -> bool {
    matches!(kind, TargetKind::Local | TargetKind::Abs | TargetKind::Weak0)
}

/// The implicit addend of the record (X86-4): its field's input bytes,
/// sign-extended; 0 for a marker.
fn addend(at: &RelocAt<'_>, out: Option<&SectionOut<'_>>) -> Result<i64, FieldError> {
    let width = at.howto.field.width();
    if width == 0 {
        return Ok(0);
    }
    if let Some(f) = at.site.at(usize::from(width)) {
        return Ok(forms::implicit(f));
    }
    let mut b = [0u8; 4];
    let dst = &mut b[..usize::from(width)];
    if out.is_some_and(|o| o.input(at.r_offset, dst)) {
        return Ok(forms::implicit(dst));
    }
    Err(FieldError::Outside { offset: at.r_offset, width })
}

/// `A`, for the `A = 0` preconditions of §8 and §9 (`None` outside the
/// section, which scan reports first).
fn addend_is_zero(at: &RelocAt<'_>) -> bool {
    addend(at, None).is_ok_and(|a| a == 0)
}

/// I1–I5 (X86-32): decided from the output kind, the target kind, `A` and
/// the input bytes, so scan and fill agree (kind 1).
fn ie_relaxed(at: &RelocAt<'_>, output: OutputKind, kind: TargetKind, alloc: bool) -> Option<Form> {
    if !alloc || !output.is_exec() || !tls_local(kind) || !addend_is_zero(at) {
        return None;
    }
    forms::form(at.r_type, at.site).filter(|f| {
        matches!(
            (at.r_type, f),
            (TLS_IE, Form::I1 | Form::I2 { .. } | Form::I3 { .. })
                | (TLS_GOTIE, Form::I4 { .. } | Form::I5 { .. })
        )
    })
}

/// T1 (with `A = 0`) or T2 (X86-37).
fn desc_form(at: &RelocAt<'_>) -> Option<Form> {
    match forms::form(at.r_type, at.site)? {
        f @ Form::T1 { .. } if addend_is_zero(at) => Some(f),
        f @ Form::T2 => Some(f),
        _ => None,
    }
}

/// Whether the record makes `_GLOBAL_OFFSET_TABLE_` exist (X86-30): class
/// `Got`, `GotRel`, `GotBaseRef`, or `TLS_GOTIE`.
fn needs_got_base(at: &RelocAt<'_>) -> bool {
    matches!(at.howto.class, DemandClass::Got | DemandClass::GotRel | DemandClass::GotBaseRef)
        || at.r_type == TLS_GOTIE
}

/// `scan` (arch.md §3.4): the checks, then the class rule of §4.3 (X86-16,
/// X86-17) with the i386 cells of §7, §8 and §11.3, and the TLS rules of
/// §9.
pub fn scan(cx: &ScanCx<'_>, at: &RelocAt<'_>, out: &mut DemandSink<'_>) {
    if let Err(e) = scan_checks(at) {
        out.error(at.index, e);
        return;
    }
    let h = at.howto;
    // X86-31: a consumed partner makes no reference and demands nothing.
    if cx.output.is_exec() && relax::follower_of(at, cx.tables).is_some() {
        return;
    }
    let sym = cx.tables.sym(at.key);
    if sym.gnu_warning {
        out.reference(at.index, EventClass::GnuWarning, at.key);
    }
    // arch.md §3.4: a dead target demands nothing; the referring section
    // decides whether the field is the tombstone, 0, or an error (X86-59).
    // Outside a debug name it is a reference event: in `.eh_frame` and
    // `.gcc_except_table` the scan driver judges it by the record that
    // owns the field (WP-177, resolve.md §6.3).
    if sym.dead {
        if dead_reference(cx.name, cx.place.alloc) != DeadRef::Tombstone {
            out.reference(at.index, EventClass::DiscardedSection, at.key);
        }
        return;
    }
    let kind = effective(sym.kind, h.class);
    // X86-25 (kind 1): a `GOT32X` that relaxes demands no GOT slot.
    if got_relaxable(cx, at, kind) {
        return;
    }
    let data = matches!(h.field, Field::Data { .. });
    let base = if needs_got_base(at) { GlobalFlags::GOT_BASE } else { GlobalFlags::empty() };
    let mut outcome = class_outcome(h.class, data, cx.output, kind, cx.place, sym.tls);
    // §7, X86-22: `GOTOFF` against `weak0` in `static` is `0 + A − GOT`.
    if h.class == DemandClass::GotRel
        && cx.output == OutputKind::Static
        && kind == TargetKind::Weak0
        && cx.place.alloc
        && !sym.tls
    {
        outcome = ClassOutcome::Demand(mortar_arch::Demand {
            global: GlobalFlags::GOT_BASE,
            ..mortar_arch::Demand::default()
        });
    }
    match outcome {
        ClassOutcome::Demand(d) => {
            if let Err(e) = i386_cells(cx, at, kind) {
                out.error(at.index, e);
                return;
            }
            out.raise(at.key, d.flags);
            out.raise_global(d.global | base);
            if let Some(role) = d.dynamic {
                out.count(role, 1);
            }
        }
        ClassOutcome::Incompatible(r) => out.error(at.index, ScanError::Incompatible(r)),
        ClassOutcome::Refused(r) => out.error(at.index, ScanError::Dynamic(r)),
        ClassOutcome::Undefined => {
            let class =
                if sym.comdat_only { EventClass::ComdatMismatch } else { EventClass::Undefined };
            out.reference(at.index, class, at.key);
        }
        ClassOutcome::Impossible => out.error(at.index, ScanError::Internal),
        // X86-21: `Pc` against `weak0` in `Pie` is `E`, "recompile with
        // -fPIE" (and `-fPIC` in `Shared`, M2).
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
                out.raise_global(global | base);
            }
            Err(e) => out.error(at.index, e),
        },
    }
}

/// The i386 refusals the generic table does not have: X86-13 (step 1 where
/// the place's `SHF_EXECINSTR` is known, `PlaceFacts.exec`; fill refuses
/// the rest; steps 2 and 3), the no-base GOT form in static-pie (X86-27,
/// X86-58), and `GOTOFF` and `Pc` against an IFUNC in static-pie (X86-50).
/// X86-25 as scan decides it: a `GOT32X` in code (`exec` known), with an
/// operand form (X86-13 step 1, so [`i386_cells`] has nothing to say),
/// that [`got_relaxed`] rewrites: the same input facts, so scan and fill
/// agree, and its key demands no GOT slot for it (X86-28).
fn got_relaxable(cx: &ScanCx<'_>, at: &RelocAt<'_>, kind: TargetKind) -> bool {
    cx.output == OutputKind::Static
        && at.r_type == GOT32X
        && cx.place.alloc
        && cx.place.exec == Some(true)
        && forms::got_operand(at.site).is_some()
        && addend_is_zero(at)
        && forms::form(GOT32X, at.site).is_some_and(|f| relaxes(f, kind))
}

/// The form and target conditions of X86-25: G1–G3 against `local`,
/// `abs` or `weak0`; G4 and G5 against `local` or `abs`.
fn relaxes(form: Form, kind: TargetKind) -> bool {
    match form {
        Form::G1 { .. } | Form::G2 { .. } | Form::G3 { .. } => {
            matches!(kind, TargetKind::Local | TargetKind::Abs | TargetKind::Weak0)
        }
        Form::G4 | Form::G5 => matches!(kind, TargetKind::Local | TargetKind::Abs),
        _ => false,
    }
}

fn i386_cells(cx: &ScanCx<'_>, at: &RelocAt<'_>, kind: TargetKind) -> Result<(), ScanError> {
    let pie = cx.output == OutputKind::Pie;
    match at.r_type {
        GOT32 | GOT32X if cx.place.exec == Some(false) => {
            Err(ScanError::Incompatible(Incompatible::InstructionMismatch))
        }
        GOT32 | GOT32X => match forms::got_operand(at.site) {
            None => Err(ScanError::Incompatible(Incompatible::InstructionMismatch)),
            Some(Operand::NoBase) if pie => Err(ScanError::Incompatible(Incompatible::ReadOnly)),
            Some(_) => Ok(()),
        },
        GOTOFF | PC32 | PC16 | PC8 if pie && kind == TargetKind::Ifunc && cx.place.alloc => {
            Err(ScanError::Refused(feature(ifunc_pie(at.r_type))))
        }
        _ => Ok(()),
    }
}

/// The TLS-model demands (X86-17, §9). In an executable a matched GD or
/// LD sequence and a matched TLSDESC member against a TLS-local target
/// demand nothing (kind 1); an unmatched one is refused by name; IE keeps
/// its `GTPREL` slot unless I1–I5 is taken, and an unrelaxed `TLS_IE` in
/// static-pie is a text relocation. Outside an executable only IE is
/// accepted (M2 for the others).
fn tls_scan(
    cx: &ScanCx<'_>,
    at: &RelocAt<'_>,
    kind: TargetKind,
) -> Result<(DemandFlags, GlobalFlags), ScanError> {
    let none = (DemandFlags::empty(), GlobalFlags::empty());
    let refused = |name| Err(ScanError::Refused(feature(name)));
    if !cx.output.is_exec() {
        return match at.r_type {
            TLS_IE | TLS_GOTIE => Ok((DemandFlags::GOT_TPREL, GlobalFlags::STATIC_TLS)),
            _ => refused(DYNAMIC_TLS),
        };
    }
    if !cx.place.alloc && matches!(at.r_type, TLS_GD | TLS_LDM | TLS_GOTDESC | TLS_DESC_CALL) {
        return refused(NON_ALLOC_TLS);
    }
    match at.r_type {
        TLS_GD => match relax::lead_form(at, cx.tables) {
            None => refused(UNRECOGNIZED_GD),
            Some(_) if tls_local(kind) => Ok(none),
            Some(_) => refused(TLS_TO_IE),
        },
        // The LE outputs of L1 and L2 do not depend on the symbol.
        TLS_LDM => match relax::lead_form(at, cx.tables) {
            None => refused(UNRECOGNIZED_LD),
            Some(_) => Ok(none),
        },
        TLS_IE | TLS_GOTIE if ie_relaxed(at, cx.output, kind, cx.place.alloc).is_some() => Ok(none),
        // X86-18: the absolute slot address in code of a static-pie.
        TLS_IE if cx.output == OutputKind::Pie => {
            Err(ScanError::Incompatible(Incompatible::ReadOnly))
        }
        TLS_IE | TLS_GOTIE => Ok((DemandFlags::GOT_TPREL, GlobalFlags::empty())),
        TLS_GOTDESC | TLS_DESC_CALL => match desc_form(at) {
            None => refused(UNRECOGNIZED_DESC),
            Some(_) if tls_local(kind) => Ok(none),
            Some(_) => refused(TLS_TO_IE),
        },
        // Every TLS-model type of §6.1 that reaches the table is above;
        // the later types were refused by `classify`.
        _ => Err(ScanError::Refused(Refusal::named(RefusalClass::RelocationType, at.howto.name))),
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

/// `P − n`, the start of a form's bytes before the field.
fn back(at: &RelocAt<'_>, n: u64) -> Result<u64, FieldError> {
    at.r_offset.checked_sub(n).ok_or(FieldError::Outside { offset: at.r_offset, width: 4 })
}

/// A TLS offset of `S + A` by `f` (`TpRel` or `DtpRel`), and `A` itself
/// for `weak0` (X86-23: its TP and DTP offsets are 0).
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

/// `T` of §9: `TpRel(S)`, 0 for a TLS `weak0` (X86-23), checked s(32)
/// before any reduction (X86-38).
fn t_value(cx: &FillCx<'_>, sym: SymValue) -> Result<i32, FieldError> {
    let t = tls_off(cx, sym, 0, |x, tls| TLS.tp_rel(x, tls))?;
    Check::Signed(32).verify(t, Class::Elf64)?;
    Ok(t as i32)
}

/// The bytes of `t`, little-endian.
const fn le(t: i32) -> [u8; 4] {
    t.to_le_bytes()
}

/// `apply` (arch.md §3.6).
pub fn apply(
    cx: &FillCx<'_>,
    at: &RelocAt<'_>,
    out: &mut SectionOut<'_>,
) -> Result<(), FieldError> {
    let h = at.howto;
    let exec = cx.output.is_exec();
    // X86-31: the lead writes the partner's bytes; the partner nothing.
    if exec && relax::follower_of(at, cx.tables).is_some() {
        return Ok(());
    }
    let sym = cx.tables.sym(at.key);
    // X86-59 (arch.md §3.11, image.md I-47): against a dead target a debug
    // section gets the ELFCLASS32 tombstone, whatever the addend, and
    // `.gcc_except_table` 0; scan reported any other section.
    if !sym.live {
        let x = match dead_reference(out.name, out.alloc) {
            DeadRef::Tombstone => mortar_arch::tombstone(out.name, Class::Elf32),
            DeadRef::Zero => 0,
            DeadRef::Discarded | DeadRef::DeadFde => {
                return Err(FieldError::MissingTerm("live target"));
            }
        };
        let width = usize::from(h.field.width());
        return put(at, out, at.r_offset, &x.to_le_bytes()[..width]);
    }
    let kind = effective(sym.kind, h.class);
    match at.r_type {
        TLS_GD | TLS_LDM | TLS_GOTDESC | TLS_DESC_CALL if !exec => {
            return Err(FieldError::Refused(feature(DYNAMIC_TLS)));
        }
        TLS_GD | TLS_LDM | TLS_GOTDESC | TLS_DESC_CALL if !out.alloc => {
            return Err(FieldError::Refused(feature(NON_ALLOC_TLS)));
        }
        TLS_GD | TLS_LDM => return lead(cx, at, out, sym, kind),
        TLS_GOTDESC | TLS_DESC_CALL => return desc(cx, at, out, sym, kind),
        TLS_IE | TLS_GOTIE => {
            if let Some(f) = ie_relaxed(at, cx.output, kind, out.alloc) {
                return ie(cx, at, out, sym, f);
            }
            if at.r_type == TLS_IE && cx.output == OutputKind::Pie {
                return Err(FieldError::Refused(feature(IE_TEXT_RELOCATION)));
            }
        }
        GOT32 | GOT32X => return got(cx, at, out, sym),
        _ => {}
    }
    let width = usize::from(h.field.width());
    if width == 0 {
        return Ok(());
    }
    let x = value(cx, at, out, sym)?;
    put(at, out, at.r_offset, &x.to_le_bytes()[..width])
}

/// The class of the ELFCLASS32 check of a row (§3.1): the 32-bit reduced
/// value, except the TP offset (X86-38) and `SIZE32`'s `u(32)` (X86-12),
/// which are checked as integers.
const fn check_class(h: &mortar_arch::Howto) -> Class {
    match (h.value.base, h.check) {
        (Base::TpRel | Base::Size, _) | (_, Check::Unsigned(32)) => Class::Elf64,
        _ => Class::Elf32,
    }
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
    let a = addend(at, Some(out))?;
    let p = out.addr.wrapping_add(at.r_offset);
    // X86-62 (arch.md §4.3): `32`/`16`/`8` against a TLS symbol at a
    // non-alloc place is the offset in the TLS block.
    if writes_dtprel(
        h.class,
        mortar_arch::PlaceFacts { alloc: out.alloc, writable: false, exec: out.exec },
        sym.tls,
    ) {
        let x = cx.dtp_rel(sym, a)?;
        h.check.verify(x, Class::Elf32)?;
        return Ok(x);
    }
    // §7: `S = 0` for `weak0`.
    let s = if sym.kind == TargetKind::Weak0 { 0 } else { sym.s };
    let b = match h.value.base {
        Base::Zero => 0,
        Base::S => s,
        Base::L => {
            let flags = cx.tables.flags(at.key);
            if flags.contains(DemandFlags::PLT) {
                cx.tables.plt(at.key).ok_or(FieldError::MissingTerm("PLT entry"))?
            } else if flags.contains(DemandFlags::IPLT) && cx.output == OutputKind::Pie {
                // X86-50: in static-pie `S` is the resolved function and a
                // `PLT32` call goes to the IPLT entry.
                cx.tables.plt(at.key).ok_or(FieldError::MissingTerm("IPLT entry"))?
            } else {
                s
            }
        }
        Base::G(k) => cx.tables.slot(at.key, k).ok_or(FieldError::MissingSlot)?,
        Base::GotBase => cx.got_base.ok_or(FieldError::MissingTerm("GOT base"))?,
        Base::TpRel => tls_off(cx, sym, a, |x, tls| TLS.tp_rel(x, tls))? as u64,
        Base::DtpRel => dtpoff(cx, at, out, sym, a)? as u64,
        Base::Size => sym.size,
    };
    let got = match h.value.minus {
        Minus::GotBase | Minus::PageGotBase => {
            cx.got_base.ok_or(FieldError::MissingTerm("GOT base"))?
        }
        _ => 0,
    };
    let x = h.value.eval(Terms { b, a, p, got }, 0);
    h.check.verify(x, check_class(h))?;
    Ok(x)
}

/// X86-61: `TLS_LDO_32` is `TpRel(S + A)` in an `SHF_EXECINSTR` section of
/// an executable (where a relaxed LD sequence or a `_TLS_MODULE_BASE_`
/// descriptor leaves `%eax = TP`), and `DtpRel(S + A)` everywhere else.
fn dtpoff(
    cx: &FillCx<'_>,
    at: &RelocAt<'_>,
    out: &SectionOut<'_>,
    sym: SymValue,
    a: i64,
) -> Result<i64, FieldError> {
    debug_assert!(at.r_type == TLS_LDO_32, "only TLS_LDO_32 has a DtpRel base");
    let code = if out.alloc && cx.output.is_exec() {
        out.exec.ok_or(FieldError::Refused(feature(DTPOFF_PLACE)))?
    } else {
        false
    };
    if code { tls_off(cx, sym, a, |x, tls| TLS.tp_rel(x, tls)) } else { cx.dtp_rel(sym, a) }
}

/// The input word at the place, for an `InstructionMismatch` refusal.
fn word(at: &RelocAt<'_>) -> u32 {
    at.site.at(4).and_then(|b| b.try_into().ok()).map_or(0, u32::from_le_bytes)
}

/// `GOT32`/`GOT32X` (X86-13, X86-25 to X86-28): the operand read, the
/// place checked to be code, then G1–G5 where they hold, else `G + A −
/// GOT` (base) or `G + A` (no base).
#[allow(clippy::many_single_char_names)]
fn got(
    cx: &FillCx<'_>,
    at: &RelocAt<'_>,
    out: &mut SectionOut<'_>,
    sym: SymValue,
) -> Result<(), FieldError> {
    // Step 1: only an instruction has an operand to read.
    match out.exec {
        Some(true) => {}
        Some(false) => return Err(FieldError::Instruction { word: word(at) }),
        None => return Err(FieldError::Refused(feature(GOT_PLACE))),
    }
    let Some(operand) = forms::got_operand(at.site) else {
        return Err(FieldError::Instruction { word: word(at) });
    };
    if cx.output == OutputKind::Pie && operand == Operand::NoBase {
        // X86-27: scan refused it (`ReadOnlySection`).
        return Err(FieldError::Instruction { word: word(at) });
    }
    let a = addend(at, Some(out))?;
    if out.alloc
        && let Some((off, bytes)) = got_relaxed(cx.output, at, out.addr, sym, a)
    {
        return put(at, out, off, &bytes);
    }
    let g = cx.tables.slot(at.key, SlotKind::Addr).ok_or(FieldError::MissingSlot)?;
    let base = match operand {
        Operand::Base => cx.got_base.ok_or(FieldError::MissingTerm("GOT base"))?,
        Operand::NoBase => 0,
    };
    // mod(32) (X86-10).
    let x = g.wrapping_add(a as u64).wrapping_sub(base) as u32;
    put(at, out, at.r_offset, &x.to_le_bytes())
}

/// G1–G5 (X86-25; kind 1): the 6 bytes to write over `[P − 2, P + 4)`,
/// when the output is `static`, the type `GOT32X`, `A = 0`, the target
/// `local` or `abs` (or `weak0` for G1–G3) and the bytes a form; `None`
/// keeps the GOT load.
#[allow(clippy::many_single_char_names)]
fn got_relaxed(
    output: OutputKind,
    at: &RelocAt<'_>,
    addr: u64,
    sym: SymValue,
    a: i64,
) -> Option<(u64, [u8; 6])> {
    if output != OutputKind::Static || at.r_type != GOT32X || a != 0 {
        return None;
    }
    let form = forms::form(GOT32X, at.site)?;
    let weak0 = sym.kind == TargetKind::Weak0;
    if !relaxes(form, sym.kind) {
        return None;
    }
    let s = if weak0 { 0 } else { sym.s };
    let p = addr.wrapping_add(at.r_offset);
    // imm32 = S; rel32 mod 2^32 (X86-10).
    let imm = (s as u32).to_le_bytes();
    let rel = |end: u64| (s.wrapping_sub(end) as u32).to_le_bytes();
    let start = at.r_offset.checked_sub(2)?;
    let b = match form {
        Form::G1 { reg } => [0xc7, 0xc0 | reg, imm[0], imm[1], imm[2], imm[3]],
        Form::G2 { op, reg } => [0x81, 0xc0 | (op & 0x38) | reg, imm[0], imm[1], imm[2], imm[3]],
        Form::G3 { reg } => [0xf7, 0xc0 | reg, imm[0], imm[1], imm[2], imm[3]],
        Form::G4 => {
            let r = rel(p.wrapping_add(4));
            [0x67, 0xe8, r[0], r[1], r[2], r[3]]
        }
        Form::G5 => {
            let r = rel(p.wrapping_add(3));
            [0xe9, r[0], r[1], r[2], r[3], 0x90]
        }
        _ => return None,
    };
    Some((start, b))
}

/// `movl %gs:0, %eax` (§9).
const GS0: [u8; 6] = [0x65, 0xa1, 0, 0, 0, 0];
/// L1's tail: `nop; lea 0(%esi,%eiz,1), %esi`.
const L1_TAIL: [u8; 5] = [0x90, 0x8d, 0x74, 0x26, 0x00];
/// L2's tail: `lea 0(%esi), %esi`.
const L2_TAIL: [u8; 6] = [0x8d, 0xb6, 0, 0, 0, 0];

/// A `TLS_GD` or `TLS_LDM` lead in an executable (X86-34, X86-35): its LE
/// output over the form's window, or refused by name (X86-36). Both
/// records' addends were read by [`relax::lead_form`] before this write.
fn lead(
    cx: &FillCx<'_>,
    at: &RelocAt<'_>,
    out: &mut SectionOut<'_>,
    sym: SymValue,
    kind: TargetKind,
) -> Result<(), FieldError> {
    let Some(form) = relax::lead_form(at, cx.tables) else {
        let name = if at.r_type == TLS_GD { UNRECOGNIZED_GD } else { UNRECOGNIZED_LD };
        return Err(FieldError::Refused(feature(name)));
    };
    let mut buf = [0u8; 12];
    buf[..6].copy_from_slice(&GS0);
    let (start, n) = match form {
        Form::D1 | Form::D2 | Form::D3 => {
            if !tls_local(kind) {
                return Err(FieldError::Refused(feature(TLS_TO_IE)));
            }
            // `subl $(−T), %eax`.
            buf[6..8].copy_from_slice(&[0x81, 0xe8]);
            buf[8..].copy_from_slice(&le(t_value(cx, sym)?.wrapping_neg()));
            (if form == Form::D1 { back(at, 3)? } else { back(at, 2)? }, 12)
        }
        Form::L1 => {
            buf[6..11].copy_from_slice(&L1_TAIL);
            (back(at, 2)?, 11)
        }
        Form::L2 => {
            buf[6..].copy_from_slice(&L2_TAIL);
            (back(at, 2)?, 12)
        }
        _ => return Err(FieldError::MissingTerm("sequence form")),
    };
    put(at, out, start, &buf[..n])
}

/// A TLSDESC member in an executable (X86-37, X86-63): T1 → `8d (05 | reg
/// << 3) T`, `T = 0` for `_TLS_MODULE_BASE_`; T2 → `66 90`.
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
        Form::T1 { reg } => {
            let base = TLS.module_base.is_some_and(|b| cx.tables.name(at.key) == b);
            let t = if base { [0; 4] } else { le(t_value(cx, sym)?) };
            put(at, out, back(at, 2)?, &[0x8d, 0x05 | (reg << 3), t[0], t[1], t[2], t[3]])
        }
        _ => put(at, out, at.r_offset, &[0x66, 0x90]),
    }
}

/// I1–I5 (X86-32): `b8 T`, `c7 (c0 | reg) T` (mov) or `81 (c0 | reg) T`
/// (add: result and flags as the input's).
fn ie(
    cx: &FillCx<'_>,
    at: &RelocAt<'_>,
    out: &mut SectionOut<'_>,
    sym: SymValue,
    form: Form,
) -> Result<(), FieldError> {
    let t = le(t_value(cx, sym)?);
    match form {
        Form::I1 => put(at, out, back(at, 1)?, &[0xb8, t[0], t[1], t[2], t[3]]),
        Form::I2 { reg } | Form::I4 { reg } => {
            put(at, out, back(at, 2)?, &[0xc7, 0xc0 | reg, t[0], t[1], t[2], t[3]])
        }
        Form::I3 { reg } | Form::I5 { reg } => {
            put(at, out, back(at, 2)?, &[0x81, 0xc0 | reg, t[0], t[1], t[2], t[3]])
        }
        _ => Err(FieldError::MissingTerm("IE form")),
    }
}

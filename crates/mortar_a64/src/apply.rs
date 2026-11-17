//! `scan` and `apply` (arch.md §3.4, §3.6; a64.md §5 to §8).
//!
//! `apply` is one pass over the `Howto`: no allocation, no dynamic
//! dispatch beyond the table lookups of `FillTables` (one `sym`, plus
//! `slot`, `plt`, `flags` or `redirect` where the value needs them).

use mortar_arch::seq::{field_window, in_section};
use mortar_arch::{
    AddendRule, Base, Check, ClassOutcome, DeadRef, DemandClass, DemandFlags, DemandSink,
    EventClass, Field, FieldError, FillCx, Incompatible, Minus, OutputKind, Refusal, RelocAt,
    ScanCx, ScanError, SectionOut, SlotKind, TargetKind, Terms, Weak0Rule, class_outcome,
    dead_reference, writes_dtprel,
};
use mortar_diag::RefusalClass;

use crate::forms::{self, B26};
use crate::synth::{NOP, checked, page_delta};
use crate::{TLS, relax, weak0_rule};

/// The feature a64.md §13.2's staging note refuses: `ABS*` against a TLS
/// symbol at a non-alloc place.
pub const NON_ALLOC_TLS_ABS: &str =
    "absolute relocation against a TLS symbol in a non-alloc section";

/// The feature refused for a large TLSDESC sequence kept as a descriptor
/// (a TLS-dynamic target in an exec output, a64.md §8.2) whose
/// `TLSDESC_ADD` is not directly followed by its `TLSDESC_CALL`: §8.1
/// rewrites each member on its own, and a `TLSDESC_CALL` is the same
/// record in the small and the large form, so the call is told apart by
/// the record before it ([`relax::large_call`]); a sequence in another
/// order would lose its `blr`.
pub const LARGE_TLSDESC_DYNAMIC: &str =
    "large-model TLSDESC whose TLSDESC_ADD is not followed by its TLSDESC_CALL";

/// The feature refused for a traditional general-dynamic member (513,
/// 514) whose sequence does not match R4's pattern in a link of the
/// static tables (a static executable or a static PIE, a64.md §8.4, R4's
/// static column): such an output has no module/offset pair to keep, so
/// the sequence is either rewritten whole to LE or refused, never applied
/// half. x86-64's name for the same case (x64.md X-26).
pub const UNRECOGNIZED_GD: &str = "unrecognized general-dynamic TLS sequence";

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
    if h.addend == AddendRule::MustBeZero && at.rela_addend() != 0 {
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

/// `scan` (arch.md §3.4): the checks, then the class rule of §4.3, the
/// TLS table of a64.md §8 and §12, and the inhibitor flags of R3 and R5.
/// `m2` is the dynamic tables (a dynamic executable); without it a
/// traditional GD member is accepted only inside R4's pattern (§8.4).
pub fn scan(m2: bool, cx: &ScanCx<'_>, at: &RelocAt<'_>, out: &mut DemandSink<'_>) {
    if let Err(e) = scan_checks(at) {
        out.error(at.index, e);
        return;
    }
    let h = at.howto;
    let exec = cx.output.is_exec();
    // §8.4: R4's `CALL26` is consumed: no reference, no demand.
    if exec && at.r_type == relax::CALL26 && relax::gd_lead(at, cx.tables).is_some() {
        return;
    }
    let sym = cx.tables.sym(at.key);
    if sym.gnu_warning {
        out.reference(at.index, EventClass::GnuWarning, at.key);
    }
    // arch.md §3.4: a dead target demands nothing; the referring section
    // decides whether the field is the tombstone, 0, or an error.
    if sym.dead {
        if dead_reference(cx.name, cx.place.alloc) == DeadRef::Discarded {
            out.reference(at.index, EventClass::DiscardedSection, at.key);
        }
        return;
    }
    // §7: a TLS class treats `weak0` as TLS-local, its TP and DTP offsets
    // being `A` (§12.2).
    // `spec/dyn.md` D-25 (WP-155): an unfillable use of a split key is
    // written as `Weak0`'s (a TLS class keeps the key's kind: such a key
    // is refused, the thread-local undefined weak).
    let kind = mortar_arch::weak_use_kind(cx.output, sym.kind, cx.place.alloc, || {
        crate::weak::weak_class(at.r_type, cx.place.writable).filter(|_| !h.class.is_tls())
    });
    let kind = if h.class.is_tls() && kind == TargetKind::Weak0 {
        TargetKind::Local
    } else {
        kind
    };
    let data = matches!(h.field, Field::Data { .. });
    let outcome = class_outcome(h.class, data, cx.output, kind, cx.place, sym.tls);
    // a64.md §5's dynamic cells are arch.md §4.3's since WP-046 (D-43,
    // D-44 in `mortar_arch::class`); `TX` in a read-only place becomes
    // `text-relocation` in the scan driver, which knows whether the
    // output has `.dynamic` (D-47; a static PIE keeps M1's reason).
    match outcome {
        // a64.md §13.2's staging note: arch.md §4.3 writes `DtpRel(S + A)`
        // here, which fill cannot yet tell from an address (its `SymValue`
        // has no symbol type), so the case is refused rather than written
        // as the template address.
        ClassOutcome::Demand(_) if writes_dtprel(h.class, cx.place, sym.tls) => {
            out.error(
                at.index,
                ScanError::Refused(Refusal::named(RefusalClass::Feature, NON_ALLOC_TLS_ABS)),
            );
        }
        ClassOutcome::Demand(d) => {
            out.raise(at.key, d.flags);
            out.raise_global(d.global);
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
        // a64.md §7: `PC` against `weak0` is `S = P` in every output kind;
        // it demands nothing.
        ClassOutcome::ArchWeak0 => {}
        ClassOutcome::ArchTls
            if exec
                && h.class == DemandClass::TlsDesc
                && at.r_type == relax::TLSDESC_ADD
                && !tls_local(kind)
                && !relax::large_add_paired(at) =>
        {
            out.error(
                at.index,
                ScanError::Refused(Refusal::named(RefusalClass::Feature, LARGE_TLSDESC_DYNAMIC)),
            );
        }
        ClassOutcome::ArchTls => {
            let gd =
                exec && h.class == DemandClass::TlsGd && relax::gd_lead(at, cx.tables).is_some();
            // §8.4, R4's static column: the static tables have no
            // unrelaxed GD (no module/offset pair, no call kept).
            if !m2 && h.class == DemandClass::TlsGd && !gd {
                out.error(
                    at.index,
                    ScanError::Refused(Refusal::named(RefusalClass::Feature, UNRECOGNIZED_GD)),
                );
                return;
            }
            let large = matches!(at.r_type, 565..=568) || relax::large_call(at);
            let (flags, global) = tls_demand(h.class, large, cx.output, kind, gd);
            out.raise(at.key, flags);
            out.raise_global(global);
        }
    }
    // The inhibitors (§8.3, §8.5): input-only, raised in every output kind.
    let inhibit = match at.r_type {
        relax::ADR_GOT_PAGE | relax::LD64_GOT_LO12_NC if relax::got_pair(at).is_none() => {
            DemandFlags::GOT_NORELAX
        }
        relax::LD64_GOTTPREL_LO12_NC if relax::ie_registers_differ(at) == Some(true) => {
            DemandFlags::TLSIE_NORELAX
        }
        _ => DemandFlags::empty(),
    };
    out.raise(at.key, inhibit);
}

/// The TLS-model demands (a64.md §8.1 to §8.4, §12). In an exec output:
/// TLSDESC against a TLS-local target relaxes to LE (nothing), against a
/// TLS-dynamic one to IE (`GOT_TPREL`), except the large form, which stays
/// a descriptor (its `TLSDESC_CALL` is told apart by
/// [`relax::large_call`]); IE keeps its slot (R3 is kind 2); GD with R4's pattern
/// (`gd`) is LE or IE. Otherwise each model demands its own slots.
fn tls_demand(
    class: DemandClass,
    large: bool,
    output: OutputKind,
    kind: TargetKind,
    gd: bool,
) -> (DemandFlags, mortar_arch::GlobalFlags) {
    use mortar_arch::GlobalFlags as GF;
    let exec = output.is_exec();
    let local = tls_local(kind);
    let none = DemandFlags::empty();
    match class {
        DemandClass::TlsDesc if exec && local => (none, GF::empty()),
        DemandClass::TlsDesc if exec && !large => (DemandFlags::GOT_TPREL, GF::empty()),
        DemandClass::TlsDesc => (DemandFlags::TLS_DESC, GF::empty()),
        DemandClass::TlsIe if exec => (DemandFlags::GOT_TPREL, GF::empty()),
        DemandClass::TlsIe => (DemandFlags::GOT_TPREL, GF::STATIC_TLS),
        DemandClass::TlsGd if gd && local => (none, GF::empty()),
        DemandClass::TlsGd if gd => (DemandFlags::GOT_TPREL, GF::empty()),
        DemandClass::TlsGd => (DemandFlags::TLS_GD, GF::empty()),
        DemandClass::TlsLd => (none, GF::TLS_LD),
        _ => (none, GF::empty()),
    }
}

/// TLS-local (a64.md §8): defined in this output, or `weak0` (§7), its TP
/// offset a link-time constant (the caller has checked the output is an
/// exec).
const fn tls_local(kind: TargetKind) -> bool {
    matches!(kind, TargetKind::Local | TargetKind::Abs | TargetKind::Weak0)
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

#[inline]
fn put_word(
    at: &RelocAt<'_>,
    out: &mut SectionOut<'_>,
    off: u64,
    w: u32,
) -> Result<(), FieldError> {
    put(at, out, off, &w.to_le_bytes())
}

/// The input word at the place, checked against its form.
fn input_word(at: &RelocAt<'_>, form: u16) -> Result<u32, FieldError> {
    let w = at.site.word(0).ok_or(FieldError::Outside { offset: at.r_offset, width: 4 })?;
    if forms::requires(form, w) { Ok(w) } else { Err(FieldError::Instruction { word: w }) }
}

/// `apply` (arch.md §3.6). `m2` enables the M2 rules R3 and R5 (§8; before
/// M2 IE and GOT loads are written unrelaxed).
#[allow(clippy::many_single_char_names)] // the spec's S, A, P, X, w
pub fn apply(
    m2: bool,
    cx: &FillCx<'_>,
    at: &RelocAt<'_>,
    out: &mut SectionOut<'_>,
) -> Result<(), FieldError> {
    let h = at.howto;
    let off = at.r_offset;
    let p = out.addr.wrapping_add(off);
    let exec = cx.output.is_exec();
    // §8.4: R4's `CALL26` writes `o+8` and `o+12`.
    if exec && at.r_type == relax::CALL26 && relax::gd_lead(at, cx.tables).is_some() {
        put_word(at, out, off, MRS_X1_TPIDR)?;
        return put_word(at, out, off + 4, ADD_X0_X1_X0);
    }
    let mut sym = cx.tables.sym(at.key);
    // `spec/dyn.md` D-25 (WP-155): an unfillable use of a split key is
    // written as `Weak0`'s (an unknown place is taken as writable).
    sym.kind = mortar_arch::weak_use_kind(cx.output, sym.kind, out.alloc, || {
        crate::weak::weak_class(at.r_type, out.writable.unwrap_or(true))
            .filter(|_| !h.class.is_tls())
    });
    let weak0 = sym.kind == TargetKind::Weak0;
    if exec {
        match h.class {
            DemandClass::TlsDesc
                if tls_local(sym.kind)
                    || !(matches!(at.r_type, 565..=568) || relax::large_call(at)) =>
            {
                return tlsdesc_exec(cx, at, out, sym);
            }
            DemandClass::TlsGd => {
                if let Some((_, o)) = relax::gd_lead(at, cx.tables) {
                    return gd_exec(cx, at, out, sym, o);
                }
            }
            DemandClass::TlsIe
                if m2
                    && tls_local(sym.kind)
                    && matches!(
                        at.r_type,
                        relax::ADR_GOTTPREL_PAGE21 | relax::LD64_GOTTPREL_LO12_NC
                    )
                    && !cx.tables.flags(at.key).contains(DemandFlags::TLSIE_NORELAX) =>
            {
                return ie_to_le(cx, at, out, sym);
            }
            _ => {}
        }
    }
    if m2
        && matches!(at.r_type, relax::ADR_GOT_PAGE | relax::LD64_GOT_LO12_NC)
        && let Some(words) = got_relaxed(cx, at, out.addr, sym)
    {
        return put_word(at, out, off, words);
    }
    // `spec/dyn.md` D-46, a64.md §6.2: the place of a symbolic `ABS64`
    // record holds 0 (RELA; the loader writes `S + A`).
    if symbolic_place(cx.output, h.class, out.alloc, sym.kind, || cx.tables.flags(at.key)) {
        return put(at, out, off, &[0; 8]);
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

    // §13.2 (arch.md §3.4, §3.11): against a dead target, a debug section
    // gets the tombstone and `.gcc_except_table` 0; scan reported any
    // other section.
    let x = if sym.live {
        value(cx, at, sym, weak0, p, form)?
    } else {
        match dead_reference(out.name, out.alloc) {
            DeadRef::Tombstone => mortar_arch::tombstone(out.name, crate::FACTS.class) as i64,
            DeadRef::Zero => 0,
            // `.eh_frame` is written by `mortar_synth` (its dead FDEs
            // dropped), never field by field here.
            DeadRef::Discarded | DeadRef::DeadFde => {
                return Err(FieldError::MissingTerm("live target"));
            }
        }
    };

    match form {
        None => {
            let width = h.field.width();
            let bytes = x.to_le_bytes();
            put(at, out, off, &bytes[..usize::from(width)])
        }
        Some(f) => put_word(at, out, off, forms::insert(f, w, x)),
    }
}

/// Whether an `AW` place carries a symbolic `ABS64` record (§5, §6.2;
/// `spec/dyn.md` D-46), so that its static content is 0: in `exec` and
/// `pie`, at an alloc place, against `weakdyn` (a writable place: D-25
/// makes a key `weak0` when it has a read-only `ABS64`), and against
/// `shlib` in `pie` (every alloc `AW` there is `D`, or refused as a text
/// relocation) and in `exec` unless the key has `COPY` or `CANON_PLT`
/// (then the cell was `C`, read-only, and the value is the copy's or the
/// PLT entry's address; a writable `ABS64` against such a key is `D`
/// too, but fill cannot tell the place's `SHF_WRITE`, so it writes
/// `S + A` there: a RELA loader ignores it, see the record).
fn symbolic_place(
    output: OutputKind,
    class: DemandClass,
    alloc: bool,
    kind: TargetKind,
    flags: impl FnOnce() -> DemandFlags,
) -> bool {
    if class != DemandClass::AbsWord || !alloc {
        return false;
    }
    match (output, kind) {
        (OutputKind::Exec | OutputKind::Pie, TargetKind::WeakDyn)
        | (OutputKind::Pie, TargetKind::Shlib { .. }) => true,
        (OutputKind::Exec, TargetKind::Shlib { .. }) => {
            !flags().intersects(DemandFlags::COPY | DemandFlags::CANON_PLT)
        }
        _ => false,
    }
}

/// `mrs x1, tpidr_el0` (§8.4).
pub const MRS_X1_TPIDR: u32 = 0xD53B_D041;
/// `add x0, x1, x0` (§8.4).
pub const ADD_X0_X1_X0: u32 = 0x8B00_0020;
/// `movz x0, #0, lsl #16`, `movk x0, #0`: the LE words before `T`.
const MOVZ_X0_16: u32 = 0xD2A0_0000;
const MOVK_X0: u32 = 0xF280_0000;

/// `T = TPREL(S + A)`, checked `u(32)` when `check` (§8.1, §8.3, §8.4);
/// `A` for `weak0` (§7).
fn tprel(
    cx: &FillCx<'_>,
    sym: mortar_arch::SymValue,
    a: i64,
    check: bool,
) -> Result<u64, FieldError> {
    let t = tls_off(cx, sym, a, |x, tls| TLS.tp_rel(x, tls))?;
    if check {
        checked(Check::Unsigned(32), 1, t)?;
    }
    Ok(t as u64)
}

/// A TLS offset of `S + A` by `f` (`TPREL` or `DTPREL`), and `A` itself
/// for `weak0` (§7: its TP and DTP offsets are 0; no template needed).
fn tls_off(
    cx: &FillCx<'_>,
    sym: mortar_arch::SymValue,
    a: i64,
    f: impl Fn(u64, mortar_arch::TlsBlock) -> i64,
) -> Result<i64, FieldError> {
    if sym.kind == TargetKind::Weak0 {
        return Ok(a);
    }
    let tls = cx.tls.ok_or(FieldError::MissingTerm("PT_TLS"))?;
    Ok(f(sym.s.wrapping_add(a as u64), tls))
}

const fn hi16(t: u64) -> u32 {
    ((t >> 16) & 0xFFFF) as u32
}

const fn lo16(t: u64) -> u32 {
    (t & 0xFFFF) as u32
}

/// The `GTPREL` slot of the key (R2, R4's IE form).
fn gtprel(cx: &FillCx<'_>, at: &RelocAt<'_>) -> Result<u64, FieldError> {
    cx.tables.slot(at.key, SlotKind::TpOff { negated: false }).ok_or(FieldError::MissingSlot)
}

/// R5 (§8.5): the relaxed word of a 311 or 312 when the precondition
/// holds, else `None` (the GOT load stands).
fn got_relaxed(
    cx: &FillCx<'_>,
    at: &RelocAt<'_>,
    addr: u64,
    sym: mortar_arch::SymValue,
) -> Option<u32> {
    if sym.kind != TargetKind::Local {
        return None;
    }
    let (o, d) = relax::got_pair(at)?;
    if cx.tables.flags(at.key).contains(DemandFlags::GOT_NORELAX) {
        return None;
    }
    let x = page_delta(addr.wrapping_add(o), sym.s).ok()?;
    let w = at.site.word(0)?;
    Some(match at.r_type {
        relax::ADR_GOT_PAGE => forms::insert(forms::ADRP, w, x),
        _ => ADD_X_IMM | ((sym.s & 0xFFF) as u32) << 10 | d << 5 | d,
    })
}

/// `add xd, xn, #0`.
const ADD_X_IMM: u32 = 0x9100_0000;

/// R3 (§8.3): IE → LE on a 541 or 542, all-or-none per symbol.
fn ie_to_le(
    cx: &FillCx<'_>,
    at: &RelocAt<'_>,
    out: &mut SectionOut<'_>,
    sym: mortar_arch::SymValue,
) -> Result<(), FieldError> {
    let w = input_word(
        at,
        match at.howto.field {
            Field::Insn { form } => form,
            _ => return Err(FieldError::MissingTerm("IE field")),
        },
    )?;
    let t = tprel(cx, sym, at.rela_addend(), true)?;
    let r = w & 0x1F;
    let word = if at.r_type == relax::ADR_GOTTPREL_PAGE21 {
        MOVZ_X0_16 | hi16(t) << 5 | r
    } else {
        MOVK_X0 | lo16(t) << 5 | r
    };
    put_word(at, out, at.r_offset, word)
}

/// R4 (§8.4) on the 513 or 514 of a matched GD sequence at `o`: LE for a
/// TLS-local target, IE otherwise.
fn gd_exec(
    cx: &FillCx<'_>,
    at: &RelocAt<'_>,
    out: &mut SectionOut<'_>,
    sym: mortar_arch::SymValue,
    o: u64,
) -> Result<(), FieldError> {
    let lead = at.r_type == relax::TLSGD_ADR_PAGE21;
    let word = if tls_local(sym.kind) {
        let t = tprel(cx, sym, at.rela_addend(), true)?;
        if lead { MOVZ_X0_16 | hi16(t) << 5 } else { MOVK_X0 | lo16(t) << 5 }
    } else {
        let g = gtprel(cx, at)?;
        if lead {
            forms::insert(forms::ADRP, ADRP_X0, page_delta(out.addr.wrapping_add(o), g)?)
        } else {
            forms::insert(forms::LDX12, LDR_X0_X0, checked(Check::None, 8, g as i64)?)
        }
    };
    put_word(at, out, at.r_offset, word)
}

/// `adrp x0, 0`, `ldr x0, [x0]`, `ldr x0, .`.
const ADRP_X0: u32 = 0x9000_0000;
const LDR_X0_X0: u32 = 0xF940_0000;
const LDR_X0_LIT: u32 = 0x5800_0000;

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
    let a = at.rela_addend();
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
        // §7: `A` for `weak0`, whose TP and DTP offsets are 0.
        Base::TpRel => tls_off(cx, sym, a, |x, tls| TLS.tp_rel(x, tls))? as u64,
        Base::DtpRel => cx.dtp_rel(sym, a)? as u64,
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

/// TLSDESC in an exec output (a64.md §8.1 R1, R1t, R1l; §8.2 R2, R2t):
/// every member is rewritten on its own, to LE against a TLS-local target
/// and to IE against a TLS-dynamic one (the large form against a
/// TLS-dynamic target stays a descriptor and does not come here).
fn tlsdesc_exec(
    cx: &FillCx<'_>,
    at: &RelocAt<'_>,
    out: &mut SectionOut<'_>,
    sym: mortar_arch::SymValue,
) -> Result<(), FieldError> {
    let h = at.howto;
    if let Field::Insn { form } = h.field {
        input_word(at, form)?;
    }
    let word = if tls_local(sym.kind) {
        let t = tprel(cx, sym, at.rela_addend(), matches!(at.r_type, 562 | 560 | 565))?;
        match at.r_type {
            // ADR_PAGE21, LD_PREL19 (tiny), OFF_G1 (large): movz x0, #T[31:16], lsl #16.
            562 | 560 | 565 => MOVZ_X0_16 | hi16(t) << 5,
            // LD64_LO12, ADR_PREL21 (tiny), OFF_G0_NC (large): movk x0, #T[15:0].
            563 | 561 | 566 => MOVK_X0 | lo16(t) << 5,
            // ADD_LO12, CALL, LDR, ADD: nop.
            _ => NOP,
        }
    } else {
        let p = out.addr.wrapping_add(at.r_offset);
        match at.r_type {
            562 => forms::insert(forms::ADRP, ADRP_X0, page_delta(p, gtprel(cx, at)?)?),
            563 => forms::insert(
                forms::LDX12,
                LDR_X0_X0,
                checked(Check::None, 8, gtprel(cx, at)? as i64)?,
            ),
            560 => {
                let x = gtprel(cx, at)?.wrapping_sub(p) as i64;
                forms::insert(forms::LDX19, LDR_X0_LIT, checked(Check::Signed(21), 4, x)?)
            }
            _ => NOP,
        }
    };
    put_word(at, out, at.r_offset, word)
}

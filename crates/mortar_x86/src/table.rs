//! The table of x86.md §6.1 as data: one `Howto` per static type.
//!
//! The rows are transcribed from §6.1 in its order, with §3.1's terms,
//! §3.3's fields and §3.4's checks; `tests/table.rs` checks the names
//! against `mortar_elf` and every row against §6.1. Every field is a
//! little-endian data field at `P` (X86-9) whose implicit addend is the
//! field itself, sign-extended (X86-4, X86-6). The window of a row is
//! (0, 0) here; `classify` widens it to the record's read window (X86-40).

use mortar_arch::{
    AddendRule, Base, Check, DemandClass, Field, Howto, Implicit, Minus, SlotKind, Status, Value,
    Window,
};

const fn val(base: Base, add_a: bool, minus: Minus) -> Value {
    Value { base, add_a, page: false, negate: false, minus }
}

const ZERO: Value = Value::of(Base::Zero);
/// `S + A`.
const SA: Value = val(Base::S, true, Minus::None);
/// `S + A − P`.
const SAP: Value = val(Base::S, true, Minus::P);
/// `L + A − P`.
const LAP: Value = val(Base::L, true, Minus::P);
/// `S + A − GOT`.
const SAG: Value = val(Base::S, true, Minus::GotBase);
/// `GOT + A − P`.
const GOTP: Value = val(Base::GotBase, true, Minus::P);
/// `Z + A`.
const ZA: Value = val(Base::Size, true, Minus::None);
/// `TpRel(S + A)`: the base already holds `S + A` (arch.md §2.2).
const TP: Value = Value::of(Base::TpRel);
/// `DtpRel(S + A)`; X86-61 chooses `TpRel` in code at fill.
const DTP: Value = Value::of(Base::DtpRel);

const GDAT: SlotKind = SlotKind::Addr;
const GTPREL: SlotKind = SlotKind::TpOff { negated: false };

/// `G(k) + A` (`TLS_IE`; `GOT32`/`GOT32X` without a base, X86-13).
const fn g_a(k: SlotKind) -> Value {
    val(Base::G(k), true, Minus::None)
}
/// `G(k) + A − GOT`.
const fn g_ag(k: SlotKind) -> Value {
    val(Base::G(k), true, Minus::GotBase)
}

/// `mod(32)` (X86-10): no check; the field takes `X mod 2^32`.
const MOD32: Check = Check::None;
const S32: Check = Check::Signed(32);

const fn d(bytes: u8) -> Field {
    Field::Data { bytes }
}

use DemandClass::{
    AbsNarrow as AN, AbsWord as AW, Branch as BR, Got as GT, GotBaseRef as GB, GotRel as GR,
    None as NO, Pc as PC, Size as SZ, TlsDesc as TS, TlsDtpRel as TD, TlsGd as TG, TlsIe as TI,
    TlsLd as TL, TlsLe as TE,
};
use Status::{Later, M1};

const fn h(
    name: &'static str,
    class: DemandClass,
    value: Value,
    field: Field,
    check: Check,
    status: Status,
) -> Howto {
    // X86-4: the implicit addend is the whole field, sign-extended; a
    // marker (width 0) has none.
    let implicit = match field {
        Field::Data { bytes } => Some(Implicit { bytes, signed: true, shift: 0 }),
        _ => None,
    };
    Howto {
        name,
        value,
        field,
        check,
        align: 1,
        class,
        // §6.1 restricts no row's `A`; §8 and §9 test `A` per form.
        addend: AddendRule::Any,
        implicit,
        window: Window { before: 0, after: 0 },
        status,
    }
}

/// The largest type number of §6.1.
pub const MAX_TYPE: u32 = 43;

/// [`howto`] of every type up to the largest it lists, read by
/// `classify` per record: one load, where the `match` is an indirect
/// jump that the mix of i386 types mispredicts (WP-032i).
pub static HOWTOS: [Option<Howto>; MAX_TYPE as usize + 1] = {
    let mut t = [None; MAX_TYPE as usize + 1];
    let mut i = 0;
    while i < t.len() {
        t[i] = howto(i as u32);
        i += 1;
    }
    t
};

/// [`howto`] by [`HOWTOS`].
#[must_use]
#[inline]
pub fn howto_of(r_type: u32) -> Option<Howto> {
    HOWTOS.get(r_type as usize).copied().flatten()
}

/// The `Howto` of a static type of §6.1, or `None` for a type §6.1 does
/// not list, or lists as refused (`32PLT`, 11): the dynamic types of
/// §6.2, the refused types of §6.3, unassigned numbers.
#[must_use]
pub const fn howto(r_type: u32) -> Option<Howto> {
    let m = Field::None;
    Some(match r_type {
        0 => h("R_386_NONE", NO, ZERO, m, Check::None, M1),
        1 => h("R_386_32", AW, SA, d(4), MOD32, M1),
        2 => h("R_386_PC32", PC, SAP, d(4), MOD32, M1),
        // The base form; `classify` gives `G + A` for an operand without
        // a base (X86-13).
        3 => h("R_386_GOT32", GT, g_ag(GDAT), d(4), MOD32, M1),
        4 => h("R_386_PLT32", BR, LAP, d(4), MOD32, M1),
        9 => h("R_386_GOTOFF", GR, SAG, d(4), MOD32, M1),
        10 => h("R_386_GOTPC", GB, GOTP, d(4), MOD32, M1),
        15 => h("R_386_TLS_IE", TI, g_a(GTPREL), d(4), MOD32, M1),
        16 => h("R_386_TLS_GOTIE", TI, g_ag(GTPREL), d(4), MOD32, M1),
        // s(32) before the reduction (X86-38).
        17 => h("R_386_TLS_LE", TE, TP, d(4), S32, M1),
        // The value is M2's (an unrelaxed lead); in an executable the lead
        // is relaxed (§9) or refused (X86-36).
        18 => h("R_386_TLS_GD", TG, g_ag(SlotKind::TlsGd), d(4), MOD32, M1),
        19 => h("R_386_TLS_LDM", TL, g_ag(SlotKind::TlsLd), d(4), MOD32, M1),
        20 => h("R_386_16", AN, SA, d(2), Check::Either(16), M1),
        21 => h("R_386_PC16", PC, SAP, d(2), Check::Signed(16), M1),
        22 => h("R_386_8", AN, SA, d(1), Check::Either(8), M1),
        23 => h("R_386_PC8", PC, SAP, d(1), Check::Signed(8), M1),
        32 => h("R_386_TLS_LDO_32", TD, DTP, d(4), MOD32, M1),
        // X86-14: refused by name until a producer emits them.
        33 => h("R_386_TLS_IE_32", TI, g_ag(SlotKind::TpOff { negated: true }), d(4), MOD32, Later),
        34 => {
            let neg = Value { negate: true, ..TP };
            h("R_386_TLS_LE_32", TE, neg, d(4), S32, Later)
        }
        // u(32) (X86-12).
        38 => h("R_386_SIZE32", SZ, ZA, d(4), Check::Unsigned(32), M1),
        39 => h("R_386_TLS_GOTDESC", TS, g_ag(SlotKind::TlsDesc), d(4), MOD32, M1),
        40 => h("R_386_TLS_DESC_CALL", TS, ZERO, m, Check::None, M1),
        43 => h("R_386_GOT32X", GT, g_ag(GDAT), d(4), MOD32, M1),
        _ => return None,
    })
}

/// The status of a type (§1, §6): its row's, `Refused` for every type
/// §6.1 does not list (X86-20, §6.3).
#[must_use]
pub const fn status(r_type: u32) -> Status {
    match howto(r_type) {
        Some(h) => h.status,
        None => Status::Refused,
    }
}

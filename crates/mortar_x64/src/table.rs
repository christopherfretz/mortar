//! The table of x64.md §6.1 as data: one `Howto` per static type.
//!
//! The rows are transcribed from §6.1 in its order (with §3.1's terms,
//! §3.2's fields and §3.3's checks); `tests/table.rs` checks the names
//! against `mortar_elf` and the status counts against §1. Every field is
//! a little-endian data field at `P` (X-5); the window of a row is (0, 0)
//! here, and `classify` widens it to the window of the form the bytes
//! match (§8.6).

use mortar_arch::{
    AddendRule, Base, Check, DemandClass, Field, Howto, Minus, SlotKind, Status, Value, Window,
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
/// `L + A − GOT`.
const LAG: Value = val(Base::L, true, Minus::GotBase);
/// `S + A − GOT`.
const SAG: Value = val(Base::S, true, Minus::GotBase);
/// `GOT + A − P`.
const GOTP: Value = val(Base::GotBase, true, Minus::P);
/// `Z + A`.
const ZA: Value = val(Base::Size, true, Minus::None);
/// `TpRel(S + A)`: the base already holds `S + A` (arch.md §2.2).
const TP: Value = Value::of(Base::TpRel);
/// `DtpRel(S + A)`; X-46 chooses `TpRel` in code at fill.
const DTP: Value = Value::of(Base::DtpRel);

const GDAT: SlotKind = SlotKind::Addr;
const GTPREL: SlotKind = SlotKind::TpOff { negated: false };

/// `G(k) + A − P` (X-4: the addend is outside the slot).
const fn g_ap(k: SlotKind) -> Value {
    val(Base::G(k), true, Minus::P)
}
/// `G(k) + A − GOT`.
const fn g_ag(k: SlotKind) -> Value {
    val(Base::G(k), true, Minus::GotBase)
}

const NONE: Check = Check::None;
const S32: Check = Check::Signed(32);
const U32: Check = Check::Unsigned(32);

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
    Howto {
        name,
        value,
        field,
        check,
        align: 1,
        class,
        // X-4: every row's `A` is "any".
        addend: AddendRule::Any,
        implicit: None,
        window: Window { before: 0, after: 0 },
        status,
    }
}

/// The largest type number of §6.1.
pub const MAX_TYPE: u32 = 51;

/// The `Howto` of a static type of §6.1, or `None` for a type §6.1 does
/// not list (the dynamic types of §6.2, the refused types of §6.3,
/// unassigned numbers).
#[must_use]
pub const fn howto(r_type: u32) -> Option<Howto> {
    let m = Field::None;
    Some(match r_type {
        0 => h("R_X86_64_NONE", NO, ZERO, m, NONE, M1),
        1 => h("R_X86_64_64", AW, SA, d(8), NONE, M1),
        2 => h("R_X86_64_PC32", PC, SAP, d(4), S32, M1),
        3 => h("R_X86_64_GOT32", GT, g_ag(GDAT), d(4), S32, Later),
        4 => h("R_X86_64_PLT32", BR, LAP, d(4), S32, M1),
        9 => h("R_X86_64_GOTPCREL", GT, g_ap(GDAT), d(4), S32, M1),
        10 => h("R_X86_64_32", AN, SA, d(4), U32, M1),
        11 => h("R_X86_64_32S", AN, SA, d(4), S32, M1),
        12 => h("R_X86_64_16", AN, SA, d(2), Check::Either(16), M1),
        13 => h("R_X86_64_PC16", PC, SAP, d(2), Check::Signed(16), M1),
        14 => h("R_X86_64_8", AN, SA, d(1), Check::Either(8), M1),
        15 => h("R_X86_64_PC8", PC, SAP, d(1), Check::Signed(8), M1),
        17 => h("R_X86_64_DTPOFF64", TD, DTP, d(8), NONE, M1),
        18 => h("R_X86_64_TPOFF64", TE, TP, d(8), NONE, M1),
        // The value is M2's (an unrelaxed lead); in an executable the
        // lead is relaxed (§8.3, §8.4) or refused (X-26).
        19 => h("R_X86_64_TLSGD", TG, g_ap(SlotKind::TlsGd), d(4), S32, M1),
        20 => h("R_X86_64_TLSLD", TL, g_ap(SlotKind::TlsLd), d(4), S32, M1),
        21 => h("R_X86_64_DTPOFF32", TD, DTP, d(4), S32, M1),
        22 => h("R_X86_64_GOTTPOFF", TI, g_ap(GTPREL), d(4), S32, M1),
        23 => h("R_X86_64_TPOFF32", TE, TP, d(4), S32, M1),
        24 => h("R_X86_64_PC64", PC, SAP, d(8), NONE, M1),
        25 => h("R_X86_64_GOTOFF64", GR, SAG, d(8), NONE, M1),
        26 => h("R_X86_64_GOTPC32", GB, GOTP, d(4), S32, M1),
        27 => h("R_X86_64_GOT64", GT, g_ag(GDAT), d(8), NONE, M1),
        28 => h("R_X86_64_GOTPCREL64", GT, g_ap(GDAT), d(8), NONE, Later),
        29 => h("R_X86_64_GOTPC64", GB, GOTP, d(8), NONE, M1),
        30 => h("R_X86_64_GOTPLT64", GT, g_ag(SlotKind::PltGot), d(8), NONE, Later),
        31 => h("R_X86_64_PLTOFF64", BR, LAG, d(8), NONE, M1),
        32 => h("R_X86_64_SIZE32", SZ, ZA, d(4), U32, M1),
        33 => h("R_X86_64_SIZE64", SZ, ZA, d(8), NONE, M1),
        34 => h("R_X86_64_GOTPC32_TLSDESC", TS, g_ap(SlotKind::TlsDesc), d(4), S32, M1),
        35 => h("R_X86_64_TLSDESC_CALL", TS, ZERO, m, NONE, M1),
        41 => h("R_X86_64_GOTPCRELX", GT, g_ap(GDAT), d(4), S32, M1),
        42 => h("R_X86_64_REX_GOTPCRELX", GT, g_ap(GDAT), d(4), S32, M1),
        43 => h("R_X86_64_CODE_4_GOTPCRELX", GT, g_ap(GDAT), d(4), S32, Later),
        44 => h("R_X86_64_CODE_4_GOTTPOFF", TI, g_ap(GTPREL), d(4), S32, Later),
        45 => h("R_X86_64_CODE_4_GOTPC32_TLSDESC", TS, g_ap(SlotKind::TlsDesc), d(4), S32, Later),
        46 => h("R_X86_64_CODE_5_GOTPCRELX", GT, g_ap(GDAT), d(4), S32, Later),
        47 => h("R_X86_64_CODE_5_GOTTPOFF", TI, g_ap(GTPREL), d(4), S32, Later),
        48 => h("R_X86_64_CODE_5_GOTPC32_TLSDESC", TS, g_ap(SlotKind::TlsDesc), d(4), S32, Later),
        49 => h("R_X86_64_CODE_6_GOTPCRELX", GT, g_ap(GDAT), d(4), S32, Later),
        50 => h("R_X86_64_CODE_6_GOTTPOFF", TI, g_ap(GTPREL), d(4), S32, Later),
        51 => h("R_X86_64_CODE_6_GOTPC32_TLSDESC", TS, g_ap(SlotKind::TlsDesc), d(4), S32, Later),
        _ => return None,
    })
}

/// The status of a type (§1, §6): its row's, `Refused` for every type
/// §6.1 does not list (§6.2's dynamic types in an input, X-14; §6.3).
#[must_use]
pub const fn status(r_type: u32) -> Status {
    match howto(r_type) {
        Some(h) => h.status,
        None => Status::Refused,
    }
}

//! The table of a64.md §6.1 as data: one `Howto` per static type.
//!
//! The rows are transcribed from §6.1 in its order; `tests/table.rs`
//! checks the names against `mortar_elf` and the status counts against
//! §1's summary.

use mortar_arch::{
    AddendRule, Base, Check, DemandClass, Field, Howto, Minus, SlotKind, Status, Value, Window,
};

use crate::forms::{
    ADD12, ADR, ADRP, B26, BC19, HI12, LDX12, LDX19, LIT19, LS12, LS15, MOV, MOVS, TB14,
};

const fn val(base: Base, add_a: bool, page: bool, minus: Minus) -> Value {
    Value { base, add_a, page, negate: false, minus }
}

const ZERO: Value = Value::of(Base::Zero);
/// `S + A`.
const SA: Value = val(Base::S, true, false, Minus::None);
/// `S + A − P`.
const SAP: Value = val(Base::S, true, false, Minus::P);
/// `Page(S + A) − Page(P)`.
const PG_SAP: Value = val(Base::S, true, true, Minus::PageP);
/// `L + A − P`: `S` is the PLT/IPLT entry when the class demanded one.
const LAP: Value = val(Base::L, true, false, Minus::P);
/// `S + A − GOT`.
const SA_GOT: Value = val(Base::S, true, false, Minus::GotBase);
/// `TPREL(S + A)`, `DTPREL(S + A)`.
const TP: Value = Value::of(Base::TpRel);
const DTP: Value = Value::of(Base::DtpRel);

const GDAT: SlotKind = SlotKind::Addr;
const GTPREL: SlotKind = SlotKind::TpOff { negated: false };
const GTLSIDX: SlotKind = SlotKind::TlsGd;
const GLDM: SlotKind = SlotKind::TlsLd;
const GTLSDESC: SlotKind = SlotKind::TlsDesc;

/// `G(k)`.
const fn g(k: SlotKind) -> Value {
    val(Base::G(k), false, false, Minus::None)
}
/// `G(k) − P`.
const fn g_p(k: SlotKind) -> Value {
    val(Base::G(k), false, false, Minus::P)
}
/// `Page(G(k)) − Page(P)`.
const fn g_page(k: SlotKind) -> Value {
    val(Base::G(k), false, true, Minus::PageP)
}
/// `G(k) − GOT`.
const fn g_got(k: SlotKind) -> Value {
    val(Base::G(k), false, false, Minus::GotBase)
}

const NONE: Check = Check::None;
const fn s(n: u8) -> Check {
    Check::Signed(n)
}
const fn u(n: u8) -> Check {
    Check::Unsigned(n)
}
const fn su(n: u8) -> Check {
    Check::Either(n)
}
const fn ms(n: u8) -> Check {
    Check::MovSigned(n)
}

const fn d(bytes: u8) -> Field {
    Field::Data { bytes }
}
const fn i(form: u16) -> Field {
    Field::Insn { form }
}

use AddendRule::{Any, MustBeZero as Zero};
use DemandClass::{
    AbsNarrow as AN, AbsWord as AW, Branch as BR, Got as GT, GotRel as GR, Lo as LO, None as NO,
    Pc as PC, TlsDesc as TS, TlsDtpRel as TD, TlsGd as TG, TlsIe as TI, TlsLd as TL, TlsLe as TE,
};
use Status::{Later, M1, M2};

#[allow(clippy::too_many_arguments)]
const fn h(
    name: &'static str,
    class: DemandClass,
    value: Value,
    addend: AddendRule,
    field: Field,
    check: Check,
    align: u8,
    status: Status,
) -> Howto {
    // A marker's word may be rewritten by a relaxation (R1, R2): its
    // window is that word.
    let window = match field {
        Field::None => Window { before: 0, after: 4 },
        _ => Window { before: 0, after: 0 },
    };
    Howto { name, value, field, check, align, class, addend, implicit: None, window, status }
}

/// The `Howto` of a static type of §6.1, or `None` for a type §6.1 does
/// not list as accepted (the refused rows, the dynamic types, unknown
/// numbers).
#[must_use]
#[allow(clippy::too_many_lines)]
pub const fn howto(r_type: u32) -> Option<Howto> {
    let m = Field::None;
    Some(match r_type {
        // Data.
        0 => h("R_AARCH64_NONE", NO, ZERO, Any, m, NONE, 1, M1),
        256 => h("R_AARCH64_NULL", NO, ZERO, Any, m, NONE, 1, M1),
        257 => h("R_AARCH64_ABS64", AW, SA, Any, d(8), NONE, 1, M1),
        258 => h("R_AARCH64_ABS32", AN, SA, Any, d(4), su(32), 1, M1),
        259 => h("R_AARCH64_ABS16", AN, SA, Any, d(2), su(16), 1, M1),
        260 => h("R_AARCH64_PREL64", PC, SAP, Any, d(8), NONE, 1, M1),
        261 => h("R_AARCH64_PREL32", PC, SAP, Any, d(4), s(32), 1, M1),
        262 => h("R_AARCH64_PREL16", PC, SAP, Any, d(2), s(16), 1, M1),
        314 => h("R_AARCH64_PLT32", BR, LAP, Any, d(4), s(32), 1, M2),
        // Inline absolute (MOVW).
        263 => h("R_AARCH64_MOVW_UABS_G0", AN, SA, Any, i(MOV), u(16), 1, M1),
        264 => h("R_AARCH64_MOVW_UABS_G0_NC", AN, SA, Any, i(MOV), NONE, 1, M1),
        265 => h("R_AARCH64_MOVW_UABS_G1", AN, SA, Any, i(MOV + 1), u(32), 1, M1),
        266 => h("R_AARCH64_MOVW_UABS_G1_NC", AN, SA, Any, i(MOV + 1), NONE, 1, M1),
        267 => h("R_AARCH64_MOVW_UABS_G2", AN, SA, Any, i(MOV + 2), u(48), 1, M1),
        268 => h("R_AARCH64_MOVW_UABS_G2_NC", AN, SA, Any, i(MOV + 2), NONE, 1, M1),
        269 => h("R_AARCH64_MOVW_UABS_G3", AN, SA, Any, i(MOV + 3), NONE, 1, M1),
        270 => h("R_AARCH64_MOVW_SABS_G0", AN, SA, Any, i(MOVS), ms(16), 1, M1),
        271 => h("R_AARCH64_MOVW_SABS_G1", AN, SA, Any, i(MOVS + 1), ms(32), 1, M1),
        272 => h("R_AARCH64_MOVW_SABS_G2", AN, SA, Any, i(MOVS + 2), ms(48), 1, M1),
        // PC-relative addresses.
        273 => h("R_AARCH64_LD_PREL_LO19", PC, SAP, Any, i(LIT19), s(21), 4, M1),
        274 => h("R_AARCH64_ADR_PREL_LO21", PC, SAP, Any, i(ADR), s(21), 1, M1),
        275 => h("R_AARCH64_ADR_PREL_PG_HI21", PC, PG_SAP, Any, i(ADRP), s(33), 1, M1),
        276 => h("R_AARCH64_ADR_PREL_PG_HI21_NC", PC, PG_SAP, Any, i(ADRP), NONE, 1, M1),
        277 => h("R_AARCH64_ADD_ABS_LO12_NC", LO, SA, Any, i(ADD12), NONE, 1, M1),
        278 => h("R_AARCH64_LDST8_ABS_LO12_NC", LO, SA, Any, i(LS12), NONE, 1, M1),
        284 => h("R_AARCH64_LDST16_ABS_LO12_NC", LO, SA, Any, i(LS12 + 1), NONE, 2, M1),
        285 => h("R_AARCH64_LDST32_ABS_LO12_NC", LO, SA, Any, i(LS12 + 2), NONE, 4, M1),
        286 => h("R_AARCH64_LDST64_ABS_LO12_NC", LO, SA, Any, i(LS12 + 3), NONE, 8, M1),
        299 => h("R_AARCH64_LDST128_ABS_LO12_NC", LO, SA, Any, i(LS12 + 4), NONE, 16, M1),
        287 => h("R_AARCH64_MOVW_PREL_G0", PC, SAP, Any, i(MOVS), ms(16), 1, M2),
        288 => h("R_AARCH64_MOVW_PREL_G0_NC", PC, SAP, Any, i(MOV), NONE, 1, M2),
        289 => h("R_AARCH64_MOVW_PREL_G1", PC, SAP, Any, i(MOVS + 1), ms(32), 1, M2),
        290 => h("R_AARCH64_MOVW_PREL_G1_NC", PC, SAP, Any, i(MOV + 1), NONE, 1, M2),
        291 => h("R_AARCH64_MOVW_PREL_G2", PC, SAP, Any, i(MOVS + 2), ms(48), 1, M2),
        292 => h("R_AARCH64_MOVW_PREL_G2_NC", PC, SAP, Any, i(MOV + 2), NONE, 1, M2),
        293 => h("R_AARCH64_MOVW_PREL_G3", PC, SAP, Any, i(MOVS + 3), NONE, 1, M2),
        // Branches.
        279 => h("R_AARCH64_TSTBR14", BR, LAP, Any, i(TB14), s(16), 4, M1),
        280 => h("R_AARCH64_CONDBR19", BR, LAP, Any, i(BC19), s(21), 4, M1),
        282 => h("R_AARCH64_JUMP26", BR, LAP, Any, i(B26), s(28), 4, M1),
        283 => h("R_AARCH64_CALL26", BR, LAP, Any, i(B26), s(28), 4, M1),
        // GOT.
        300 => h("R_AARCH64_MOVW_GOTOFF_G0", GT, g_got(GDAT), Zero, i(MOVS), ms(16), 1, Later),
        301 => h("R_AARCH64_MOVW_GOTOFF_G0_NC", GT, g_got(GDAT), Zero, i(MOV), NONE, 1, Later),
        302 => h("R_AARCH64_MOVW_GOTOFF_G1", GT, g_got(GDAT), Zero, i(MOVS + 1), ms(32), 1, Later),
        303 => h("R_AARCH64_MOVW_GOTOFF_G1_NC", GT, g_got(GDAT), Zero, i(MOV + 1), NONE, 1, Later),
        304 => h("R_AARCH64_MOVW_GOTOFF_G2", GT, g_got(GDAT), Zero, i(MOVS + 2), ms(48), 1, Later),
        305 => h("R_AARCH64_MOVW_GOTOFF_G2_NC", GT, g_got(GDAT), Zero, i(MOV + 2), NONE, 1, Later),
        306 => h("R_AARCH64_MOVW_GOTOFF_G3", GT, g_got(GDAT), Zero, i(MOVS + 3), NONE, 1, Later),
        307 => h("R_AARCH64_GOTREL64", GR, SA_GOT, Any, d(8), NONE, 1, M2),
        308 => h("R_AARCH64_GOTREL32", GR, SA_GOT, Any, d(4), s(32), 1, M2),
        315 => {
            let v = val(Base::G(GDAT), true, false, Minus::P);
            h("R_AARCH64_GOTPCREL32", GT, v, Any, d(4), s(32), 1, M2)
        }
        309 => h("R_AARCH64_GOT_LD_PREL19", GT, g_p(GDAT), Zero, i(LDX19), s(21), 4, M1),
        310 => h("R_AARCH64_LD64_GOTOFF_LO15", GT, g_got(GDAT), Zero, i(LS15), u(15), 8, M2),
        311 => h("R_AARCH64_ADR_GOT_PAGE", GT, g_page(GDAT), Zero, i(ADRP), s(33), 1, M1),
        312 => h("R_AARCH64_LD64_GOT_LO12_NC", GT, g(GDAT), Zero, i(LDX12), NONE, 8, M1),
        313 => {
            let v = val(Base::G(GDAT), false, false, Minus::PageGotBase);
            h("R_AARCH64_LD64_GOTPAGE_LO15", GT, v, Zero, i(LS15), u(15), 8, M1)
        }
        // TLS, general dynamic.
        512 => h("R_AARCH64_TLSGD_ADR_PREL21", TG, g_p(GTLSIDX), Zero, i(ADR), s(21), 1, Later),
        513 => h("R_AARCH64_TLSGD_ADR_PAGE21", TG, g_page(GTLSIDX), Zero, i(ADRP), s(33), 1, M2),
        514 => h("R_AARCH64_TLSGD_ADD_LO12_NC", TG, g(GTLSIDX), Zero, i(ADD12), NONE, 1, M2),
        515 => {
            h("R_AARCH64_TLSGD_MOVW_G1", TG, g_got(GTLSIDX), Zero, i(MOVS + 1), ms(32), 1, Later)
        }
        516 => h("R_AARCH64_TLSGD_MOVW_G0_NC", TG, g_got(GTLSIDX), Zero, i(MOV), NONE, 1, Later),
        // TLS, local dynamic module.
        517 => h("R_AARCH64_TLSLD_ADR_PREL21", TL, g_p(GLDM), Zero, i(ADR), s(21), 1, Later),
        518 => h("R_AARCH64_TLSLD_ADR_PAGE21", TL, g_page(GLDM), Zero, i(ADRP), s(33), 1, Later),
        519 => h("R_AARCH64_TLSLD_ADD_LO12_NC", TL, g(GLDM), Zero, i(ADD12), NONE, 1, Later),
        520 => h("R_AARCH64_TLSLD_MOVW_G1", TL, g_got(GLDM), Zero, i(MOVS + 1), ms(32), 1, Later),
        521 => h("R_AARCH64_TLSLD_MOVW_G0_NC", TL, g_got(GLDM), Zero, i(MOV), NONE, 1, Later),
        522 => h("R_AARCH64_TLSLD_LD_PREL19", TL, g_p(GLDM), Zero, i(LDX19), s(21), 4, Later),
        // TLS, DTPREL offsets.
        523 => h("R_AARCH64_TLSLD_MOVW_DTPREL_G2", TD, DTP, Any, i(MOVS + 2), ms(48), 1, M2),
        524 => h("R_AARCH64_TLSLD_MOVW_DTPREL_G1", TD, DTP, Any, i(MOVS + 1), ms(32), 1, M2),
        525 => h("R_AARCH64_TLSLD_MOVW_DTPREL_G1_NC", TD, DTP, Any, i(MOV + 1), NONE, 1, M2),
        526 => h("R_AARCH64_TLSLD_MOVW_DTPREL_G0", TD, DTP, Any, i(MOVS), ms(16), 1, M2),
        527 => h("R_AARCH64_TLSLD_MOVW_DTPREL_G0_NC", TD, DTP, Any, i(MOV), NONE, 1, M2),
        528 => h("R_AARCH64_TLSLD_ADD_DTPREL_HI12", TD, DTP, Any, i(HI12), u(24), 1, M2),
        529 => h("R_AARCH64_TLSLD_ADD_DTPREL_LO12", TD, DTP, Any, i(ADD12), u(12), 1, M2),
        530 => h("R_AARCH64_TLSLD_ADD_DTPREL_LO12_NC", TD, DTP, Any, i(ADD12), NONE, 1, M2),
        531 => h("R_AARCH64_TLSLD_LDST8_DTPREL_LO12", TD, DTP, Any, i(LS12), u(12), 1, M2),
        532 => h("R_AARCH64_TLSLD_LDST8_DTPREL_LO12_NC", TD, DTP, Any, i(LS12), NONE, 1, M2),
        533 => h("R_AARCH64_TLSLD_LDST16_DTPREL_LO12", TD, DTP, Any, i(LS12 + 1), u(12), 2, M2),
        534 => h("R_AARCH64_TLSLD_LDST16_DTPREL_LO12_NC", TD, DTP, Any, i(LS12 + 1), NONE, 2, M2),
        535 => h("R_AARCH64_TLSLD_LDST32_DTPREL_LO12", TD, DTP, Any, i(LS12 + 2), u(12), 4, M2),
        536 => h("R_AARCH64_TLSLD_LDST32_DTPREL_LO12_NC", TD, DTP, Any, i(LS12 + 2), NONE, 4, M2),
        537 => h("R_AARCH64_TLSLD_LDST64_DTPREL_LO12", TD, DTP, Any, i(LS12 + 3), u(12), 8, M2),
        538 => h("R_AARCH64_TLSLD_LDST64_DTPREL_LO12_NC", TD, DTP, Any, i(LS12 + 3), NONE, 8, M2),
        572 => h("R_AARCH64_TLSLD_LDST128_DTPREL_LO12", TD, DTP, Any, i(LS12 + 4), u(12), 16, M2),
        573 => h("R_AARCH64_TLSLD_LDST128_DTPREL_LO12_NC", TD, DTP, Any, i(LS12 + 4), NONE, 16, M2),
        // TLS, initial exec.
        539 => h(
            "R_AARCH64_TLSIE_MOVW_GOTTPREL_G1",
            TI,
            g_got(GTPREL),
            Zero,
            i(MOVS + 1),
            ms(32),
            1,
            M2,
        ),
        540 => {
            h("R_AARCH64_TLSIE_MOVW_GOTTPREL_G0_NC", TI, g_got(GTPREL), Zero, i(MOV), NONE, 1, M2)
        }
        541 => h(
            "R_AARCH64_TLSIE_ADR_GOTTPREL_PAGE21",
            TI,
            g_page(GTPREL),
            Zero,
            i(ADRP),
            s(33),
            1,
            M1,
        ),
        542 => {
            h("R_AARCH64_TLSIE_LD64_GOTTPREL_LO12_NC", TI, g(GTPREL), Zero, i(LDX12), NONE, 8, M1)
        }
        543 => {
            h("R_AARCH64_TLSIE_LD_GOTTPREL_PREL19", TI, g_p(GTPREL), Zero, i(LDX19), s(21), 4, M1)
        }
        // TLS, local exec.
        544 => h("R_AARCH64_TLSLE_MOVW_TPREL_G2", TE, TP, Any, i(MOVS + 2), ms(48), 1, M1),
        545 => h("R_AARCH64_TLSLE_MOVW_TPREL_G1", TE, TP, Any, i(MOVS + 1), ms(32), 1, M1),
        546 => h("R_AARCH64_TLSLE_MOVW_TPREL_G1_NC", TE, TP, Any, i(MOV + 1), NONE, 1, M1),
        547 => h("R_AARCH64_TLSLE_MOVW_TPREL_G0", TE, TP, Any, i(MOVS), ms(16), 1, M1),
        548 => h("R_AARCH64_TLSLE_MOVW_TPREL_G0_NC", TE, TP, Any, i(MOV), NONE, 1, M1),
        549 => h("R_AARCH64_TLSLE_ADD_TPREL_HI12", TE, TP, Any, i(HI12), u(24), 1, M1),
        550 => h("R_AARCH64_TLSLE_ADD_TPREL_LO12", TE, TP, Any, i(ADD12), u(12), 1, M1),
        551 => h("R_AARCH64_TLSLE_ADD_TPREL_LO12_NC", TE, TP, Any, i(ADD12), NONE, 1, M1),
        552 => h("R_AARCH64_TLSLE_LDST8_TPREL_LO12", TE, TP, Any, i(LS12), u(12), 1, M1),
        553 => h("R_AARCH64_TLSLE_LDST8_TPREL_LO12_NC", TE, TP, Any, i(LS12), NONE, 1, M1),
        554 => h("R_AARCH64_TLSLE_LDST16_TPREL_LO12", TE, TP, Any, i(LS12 + 1), u(12), 2, M1),
        555 => h("R_AARCH64_TLSLE_LDST16_TPREL_LO12_NC", TE, TP, Any, i(LS12 + 1), NONE, 2, M1),
        556 => h("R_AARCH64_TLSLE_LDST32_TPREL_LO12", TE, TP, Any, i(LS12 + 2), u(12), 4, M1),
        557 => h("R_AARCH64_TLSLE_LDST32_TPREL_LO12_NC", TE, TP, Any, i(LS12 + 2), NONE, 4, M1),
        558 => h("R_AARCH64_TLSLE_LDST64_TPREL_LO12", TE, TP, Any, i(LS12 + 3), u(12), 8, M1),
        559 => h("R_AARCH64_TLSLE_LDST64_TPREL_LO12_NC", TE, TP, Any, i(LS12 + 3), NONE, 8, M1),
        570 => h("R_AARCH64_TLSLE_LDST128_TPREL_LO12", TE, TP, Any, i(LS12 + 4), u(12), 16, M1),
        571 => h("R_AARCH64_TLSLE_LDST128_TPREL_LO12_NC", TE, TP, Any, i(LS12 + 4), NONE, 16, M1),
        // TLS descriptors.
        560 => h("R_AARCH64_TLSDESC_LD_PREL19", TS, g_p(GTLSDESC), Zero, i(LDX19), s(21), 4, M2),
        561 => h("R_AARCH64_TLSDESC_ADR_PREL21", TS, g_p(GTLSDESC), Zero, i(ADR), s(21), 1, M2),
        562 => h("R_AARCH64_TLSDESC_ADR_PAGE21", TS, g_page(GTLSDESC), Zero, i(ADRP), s(33), 1, M1),
        563 => h("R_AARCH64_TLSDESC_LD64_LO12", TS, g(GTLSDESC), Zero, i(LDX12), NONE, 8, M1),
        564 => h("R_AARCH64_TLSDESC_ADD_LO12", TS, g(GTLSDESC), Zero, i(ADD12), NONE, 1, M1),
        565 => h("R_AARCH64_TLSDESC_OFF_G1", TS, g_got(GTLSDESC), Zero, i(MOVS + 1), ms(32), 1, M2),
        566 => h("R_AARCH64_TLSDESC_OFF_G0_NC", TS, g_got(GTLSDESC), Zero, i(MOV), NONE, 1, M2),
        567 => h("R_AARCH64_TLSDESC_LDR", TS, ZERO, Zero, m, NONE, 1, M2),
        568 => h("R_AARCH64_TLSDESC_ADD", TS, ZERO, Zero, m, NONE, 1, M2),
        569 => h("R_AARCH64_TLSDESC_CALL", TS, ZERO, Zero, m, NONE, 1, M1),
        // TLS data.
        1029 => h("R_AARCH64_TLS_DTPREL64", TD, DTP, Any, d(8), NONE, 1, M2),
        _ => return None,
    })
}

/// The types §6.1 and §6.2 refuse with no plan: the `PAuth` static types
/// (580-598), the Structure Protection types (316, 317), a static
/// `TLS_DTPMOD64` (1028), and the dynamic types an input never carries
/// (1024-1027, 1030-1032, 1041-1044).
#[must_use]
pub const fn refused(r_type: u32) -> bool {
    matches!(r_type, 316 | 317 | 580..=598 | 1024..=1028 | 1030..=1032 | 1041..=1044)
}

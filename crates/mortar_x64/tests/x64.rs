//! Behavioural tests of `mortar_x64` outside the relaxations, by section
//! of `spec/x64.md`: the values of §6.1 (X-12), the checks of §3.3 (X-6,
//! X-7, W1, W2), the demands of §5 (X-10, X-11), undefined weak (§7,
//! X-15 to X-18), dead targets and TLS at non-alloc places (X-53, W10),
//! and `DTPOFF*` by section kind (X-46).
#![allow(clippy::many_single_char_names)]

mod support;

use mortar_arch::{
    Arch, DemandFlags, FieldError, GlobalFlags, Incompatible, OutputKind, PlaceFacts, Range, Reloc,
    ScanError, SlotKind, TargetKind, TlsBlock,
};
use mortar_diag::RefusalClass;
use mortar_x64::{DTPOFF_PLACE, X64};
use support::{Case, Sink, Tables, key, raised, scan_all, scan_one};

/// The section `.data`/`.text` at `ADDR`; the target at `S`.
const ADDR: u64 = 0x40_2000;
const S: u64 = 0x40_1000;
const GOT: u64 = 0x40_3000;
const SLOT: u64 = 0x40_3800;
const PLT: u64 = 0x40_0800;
/// `TpRel(x) = x − 0x404010`, `DtpRel(x) = x − 0x404000` (§3.1, §12).
const TLS: TlsBlock = TlsBlock { start: 0x40_4000, size: 0x10, align: 8 };
const TP_END: u64 = 0x40_4010;

const ALLOC: PlaceFacts = PlaceFacts { alloc: true, writable: true };
const RODATA: PlaceFacts = PlaceFacts { alloc: true, writable: false };
const DEBUG: PlaceFacts = PlaceFacts { alloc: false, writable: false };

fn tables(kind: TargetKind) -> Tables {
    let mut t = Tables::default();
    t.sym(key(1), S, kind);
    t.syms.get_mut(&key(1)).unwrap().size = 0x28;
    t.slots.insert((key(1), SlotKind::Addr), SLOT);
    t.slots.insert((key(1), SlotKind::TpOff { negated: false }), SLOT + 8);
    t.plts.insert(key(1), PLT);
    t
}

fn tls_tables(kind: TargetKind, s: u64) -> Tables {
    let mut t = Tables::default();
    t.tls_sym(key(1), s, kind);
    t.slots.insert((key(1), SlotKind::TpOff { negated: false }), SLOT + 8);
    t
}

/// The field bytes one record of `r_type` writes at offset 8 of a
/// 32-byte section.
fn field(case: &Case<'_>, r_type: u32, addend: i64) -> Result<Vec<u8>, FieldError> {
    let out = case.apply(&[Reloc { r_offset: 8, key: key(1), r_type, addend }])?;
    let w = usize::from(case.howto(r_type, 8).field.width());
    Ok(out[8..8 + w].to_vec())
}

fn le(x: i64, w: usize) -> Vec<u8> {
    x.to_le_bytes()[..w].to_vec()
}

fn data_case<'a>(t: &'a Tables, input: &'a [u8]) -> Case<'a> {
    let mut c = Case::new(t, input, ADDR);
    c.name = b".data";
    c.exec = Some(false);
    c.got_base = Some(GOT);
    c.tls = Some(TLS);
    c
}

const P: i64 = (ADDR + 8) as i64;

// ---------------------------------------------------------------- §6.1 X-12

#[test]
fn x12_every_non_tls_row_writes_its_value() {
    let t = tables(TargetKind::Local);
    let input = [0u8; 32];
    let c = data_case(&t, &input);
    let s = S as i64;
    let cases: [(u32, i64, i64, usize); 17] = [
        (1, 5, s + 5, 8),
        (2, -4, s - 4 - P, 4),
        // `L` is `S` without a `PLT` flag.
        (4, -4, s - 4 - P, 4),
        (9, -4, SLOT as i64 - 4 - P, 4),
        (10, 7, s + 7, 4),
        (11, -7, s - 7, 4),
        (12, 0, 0x1000, 2),
        (13, 0, 0x1000 - 0x2008 + 0x2000 - 0x1000, 2),
        (14, 0, 0, 1),
        (15, 0, 0, 1),
        (24, 3, s + 3 - P, 8),
        (25, 2, s + 2 - GOT as i64, 8),
        (26, 3, GOT as i64 + 3 - P, 4),
        (27, 1, SLOT as i64 + 1 - GOT as i64, 8),
        (29, 0, GOT as i64 - P, 8),
        (31, 0, s - GOT as i64, 8),
        (32, 1, 0x29, 4),
    ];
    for (r_type, a, want, w) in cases {
        let got = match r_type {
            // The narrow types need targets they can reach.
            12..=15 => continue,
            _ => field(&c, r_type, a).unwrap(),
        };
        assert_eq!(got, le(want, w), "type {r_type}");
    }
    assert_eq!(field(&c, 33, -1).unwrap(), le(0x27, 8));
    assert_eq!(field(&c, 41, 0).unwrap(), le(SLOT as i64 - P, 4), "A ≠ −4: unrelaxed");
    assert_eq!(field(&c, 42, 0).unwrap(), le(SLOT as i64 - P, 4));
    // `NONE` writes nothing.
    assert_eq!(
        c.apply(&[Reloc { r_offset: 8, key: key(1), r_type: 0, addend: 9 }]).unwrap(),
        input
    );
}

#[test]
fn x12_narrow_rows() {
    let mut t = tables(TargetKind::Local);
    t.syms.get_mut(&key(1)).unwrap().s = 0x1234;
    let input = [0u8; 32];
    let mut c = data_case(&t, &input);
    c.addr = 0x1000;
    let p = 0x1008;
    assert_eq!(field(&c, 12, 1).unwrap(), le(0x1235, 2));
    assert_eq!(field(&c, 13, 0).unwrap(), le(0x1234 - p, 2));
    assert_eq!(field(&c, 14, -0x1200).unwrap(), le(0x34, 1));
    assert_eq!(field(&c, 15, -0x22c).unwrap(), le(0x1234 - 0x22c - p, 1));
}

#[test]
fn x12_branch_takes_the_plt_entry_when_flagged() {
    let mut t = tables(TargetKind::Ifunc);
    t.flags.insert(key(1), DemandFlags::PLT);
    let input = [0u8; 32];
    let c = data_case(&t, &input);
    assert_eq!(field(&c, 4, -4).unwrap(), le(PLT as i64 - 4 - P, 4));
    assert_eq!(field(&c, 31, 0).unwrap(), le(PLT as i64 - GOT as i64, 8));
}

#[test]
fn x12_tls_le_rows() {
    let t = tls_tables(TargetKind::Local, 0x40_4008);
    let input = [0u8; 32];
    let c = data_case(&t, &input);
    // TpRel(S + A), variant II.
    assert_eq!(field(&c, 23, 4).unwrap(), le(0x40_400c - TP_END as i64, 4));
    assert_eq!(field(&c, 18, 0).unwrap(), le(0x40_4008 - TP_END as i64, 8));
}

// ---------------------------------------------------------------- §3.3

fn range(r: Result<Vec<u8>, FieldError>) -> Range {
    match r {
        Err(FieldError::Range(r)) => r,
        other => panic!("not out of range: {other:?}"),
    }
}

/// A target at `s` in a section at 0 (so `P = 8`).
fn at_zero(s: u64) -> Tables {
    let mut t = tables(TargetKind::Abs);
    t.syms.get_mut(&key(1)).unwrap().s = s;
    t
}

#[test]
fn x6_w1_16_and_8_are_checked_su() {
    let t = at_zero(0);
    let input = [0u8; 32];
    let mut c = data_case(&t, &input);
    c.addr = 0;
    // `16`: [−2^15, 2^16 − 1]. GNU ld accepts −32769 (W1).
    assert_eq!(field(&c, 12, -32768).unwrap(), le(-32768, 2));
    assert_eq!(field(&c, 12, 65535).unwrap(), le(65535, 2));
    assert_eq!(range(field(&c, 12, -32769)).value, -32769);
    assert_eq!(range(field(&c, 12, 65536)).value, 65536);
    // `8`: [−2^7, 2^8 − 1]. GNU ld accepts down to −256 (W1).
    assert_eq!(field(&c, 14, -128).unwrap(), le(-128, 1));
    assert_eq!(field(&c, 14, 255).unwrap(), le(255, 1));
    assert_eq!(range(field(&c, 14, -129)).min, -128);
    assert_eq!(range(field(&c, 14, 256)).max, 255);
    assert!(field(&c, 14, -256).is_err(), "W1: GNU ld's −256 is refused");
}

#[test]
fn x6_w2_pc16_and_pc8_are_signed() {
    let t = at_zero(8);
    let input = [0u8; 32];
    let mut c = data_case(&t, &input);
    c.addr = 0;
    // `S − P = 0`: the addend is X.
    assert_eq!(field(&c, 13, 32767).unwrap(), le(32767, 2));
    assert_eq!(field(&c, 13, -32768).unwrap(), le(-32768, 2));
    assert!(field(&c, 13, 32768).is_err(), "W2: GNU ld accepts +32768");
    assert!(field(&c, 13, -32769).is_err(), "W2: GNU ld accepts −32769");
    assert_eq!(field(&c, 15, 127).unwrap(), le(127, 1));
    assert!(field(&c, 15, 128).is_err());
    assert!(field(&c, 15, -129).is_err());
}

#[test]
fn x6_x7_32_bit_checks() {
    let t = at_zero(0);
    let input = [0u8; 32];
    let mut c = data_case(&t, &input);
    c.addr = 0;
    // `32` u(32), `32S` s(32).
    assert_eq!(field(&c, 10, 0xffff_ffff).unwrap(), le(0xffff_ffff, 4));
    assert!(field(&c, 10, -1).is_err());
    assert!(field(&c, 10, 1 << 32).is_err());
    assert_eq!(field(&c, 11, -(1 << 31)).unwrap(), le(-(1 << 31), 4));
    assert!(field(&c, 11, 1 << 31).is_err());
    // X-7: `SIZE32` = −1 is refused (size 0x28, A = −0x29).
    assert!(field(&c, 32, -0x29).is_err());
    assert_eq!(field(&c, 32, -0x28).unwrap(), le(0, 4));
    // PC32 at both ends.
    assert_eq!(field(&c, 2, (1 << 31) - 1 + 8).unwrap(), le((1 << 31) - 1, 4));
    assert!(field(&c, 2, (1 << 31) + 8).is_err());
    assert!(field(&c, 2, -(1 << 31) + 7).is_err());
}

// ---------------------------------------------------------------- §5

fn scan(t: &Tables, output: OutputKind, place: PlaceFacts, r_type: u32, addend: i64) -> Sink {
    let input = [0u8; 32];
    scan_one(X64::M1, t, output, place, &input, Reloc { r_offset: 8, key: key(1), r_type, addend })
        .unwrap()
}

#[test]
fn x10_demands_static() {
    let t = tables(TargetKind::Local);
    for r_type in [9, 27, 41, 42] {
        let s = scan(&t, OutputKind::Static, RODATA, r_type, -4);
        assert_eq!(raised(&s, key(1)), DemandFlags::GOT, "{r_type}: kind 2 keeps the slot");
        assert!(s.errors.is_empty());
    }
    for r_type in [1, 2, 4, 10, 11, 24, 31, 32, 33] {
        let s = scan(&t, OutputKind::Static, RODATA, r_type, 0);
        assert_eq!(raised(&s, key(1)), DemandFlags::empty(), "{r_type}");
        assert!(s.errors.is_empty(), "{r_type}: {:?}", s.errors);
    }
    // `GOTPC*`, `GOTOFF64`, `GOT64` and `PLTOFF64` need the GOT base.
    for r_type in [25, 26, 27, 29, 31] {
        let s = scan(&t, OutputKind::Static, RODATA, r_type, 0);
        assert!(s.global.contains(GlobalFlags::GOT_BASE), "{r_type}");
    }
    for r_type in [1, 2, 4, 9, 41] {
        let s = scan(&t, OutputKind::Static, RODATA, r_type, 0);
        assert!(!s.global.contains(GlobalFlags::GOT_BASE), "{r_type}");
    }
}

#[test]
fn x10_ifunc_takes_the_iplt() {
    let t = tables(TargetKind::Ifunc);
    let s = scan(&t, OutputKind::Static, RODATA, 4, -4);
    assert!(raised(&s, key(1)).contains(DemandFlags::IPLT));
    let s = scan(&t, OutputKind::Static, RODATA, 42, -4);
    assert!(raised(&s, key(1)).contains(DemandFlags::GOT));
}

#[test]
fn a7_abs_narrow_in_pie() {
    // §5: `AbsNarrow` is `E` in `Pie` except against `Abs`.
    let t = tables(TargetKind::Local);
    for r_type in [10, 11, 12, 14] {
        let s = scan(&t, OutputKind::Pie, ALLOC, r_type, 0);
        assert_eq!(s.errors.len(), 1, "{r_type}");
        assert!(matches!(s.errors[0].1, ScanError::Incompatible(_)));
    }
    let t = tables(TargetKind::Abs);
    let s = scan(&t, OutputKind::Pie, ALLOC, 10, 0);
    assert!(s.errors.is_empty());
}

#[test]
fn x11_tls_mismatch_is_an_error_at_an_alloc_place() {
    let t = tls_tables(TargetKind::Local, 0x40_4008);
    let s = scan(&t, OutputKind::Static, RODATA, 1, 0);
    assert_eq!(s.errors[0].1, ScanError::Incompatible(Incompatible::ThreadLocal));
    let t = tables(TargetKind::Local);
    let s = scan(&t, OutputKind::Static, RODATA, 23, 0);
    assert_eq!(s.errors[0].1, ScanError::Incompatible(Incompatible::NotThreadLocal));
}

#[test]
fn x11_tls_le_and_dtprel_demand_nothing() {
    let t = tls_tables(TargetKind::Local, 0x40_4008);
    for r_type in [17, 18, 21, 23] {
        let s = scan(&t, OutputKind::Static, RODATA, r_type, 0);
        assert!(s.errors.is_empty() && s.raised.is_empty(), "{r_type}: {s:?}");
    }
}

#[test]
fn x11_unrelaxed_ie_keeps_its_slot() {
    // X-24: bytes that match neither I1 nor I2 keep the `GTPREL` slot.
    let t = tls_tables(TargetKind::Local, 0x40_4008);
    let s = scan(&t, OutputKind::Static, RODATA, 22, -4);
    assert_eq!(raised(&s, key(1)), DemandFlags::GOT_TPREL);
    let input = [0u8; 32];
    let c = data_case(&t, &input);
    assert_eq!(field(&c, 22, -4).unwrap(), le((SLOT + 8) as i64 - 4 - P, 4));
}

#[test]
fn x11_ie_in_a_shared_object_needs_static_tls() {
    let t = tls_tables(TargetKind::Local, 0x40_4008);
    let s = scan(&t, OutputKind::Shared, RODATA, 22, -4);
    assert_eq!(raised(&s, key(1)), DemandFlags::GOT_TPREL);
    assert!(s.global.contains(GlobalFlags::STATIC_TLS));
    for r_type in [19, 20, 34, 35] {
        let s = scan(&t, OutputKind::Shared, RODATA, r_type, -4);
        let ScanError::Refused(r) = s.errors[0].1 else { panic!("{r_type}: {s:?}") };
        assert_eq!(r.class, RefusalClass::Feature);
    }
}

#[test]
fn undefined_targets_report_undefined() {
    let mut t = Tables::default();
    t.sym(key(1), 0, TargetKind::Undefined);
    let s = scan(&t, OutputKind::Static, RODATA, 2, -4);
    assert_eq!(s.undefined, [(key(1), 0)]);
}

#[test]
fn outside_the_section_is_malformed() {
    let t = tables(TargetKind::Local);
    let input = [0u8; 10];
    let mut sink = Sink::default();
    let r = Reloc { r_offset: 8, key: key(1), r_type: 10, addend: 0 };
    scan_all(X64::M1, &t, OutputKind::Static, RODATA, &input, &[r], &mut sink).unwrap();
    assert_eq!(sink.errors, [(0, ScanError::Outside)]);
}

// ---------------------------------------------------------------- §7

#[test]
fn x15_pc_to_weak0() {
    let t = tables(TargetKind::Weak0);
    let s = scan(&t, OutputKind::Static, RODATA, 2, -4);
    assert!(s.errors.is_empty());
    let s = scan(&t, OutputKind::Pie, RODATA, 2, -4);
    assert_eq!(s.errors[0].1, ScanError::Incompatible(Incompatible::RecompilePie));
    let s = scan(&t, OutputKind::Shared, RODATA, 2, -4);
    assert_eq!(s.errors[0].1, ScanError::Incompatible(Incompatible::RecompilePic));
    // Static: `0 + A − P`.
    let input = [0u8; 32];
    let c = data_case(&t, &input);
    assert_eq!(field(&c, 2, -4).unwrap(), le(-4 - P, 4));
}

#[test]
fn x16_weak0_values() {
    let t = tables(TargetKind::Weak0);
    let input = [0u8; 32];
    let c = data_case(&t, &input);
    assert_eq!(field(&c, 1, 3).unwrap(), le(3, 8), "AbsWord: 0 + A");
    assert_eq!(field(&c, 4, -4).unwrap(), le(-4 - P, 4), "Branch: a call to 0");
    assert_eq!(field(&c, 31, 0).unwrap(), le(-(GOT as i64), 8), "PLTOFF64: A − GOT");
    // `GOTOFF64` against `weak0` is `E` (arch.md §4.3).
    let s = scan(&t, OutputKind::Static, RODATA, 25, 0);
    assert_eq!(s.errors.len(), 1, "{s:?}");
}

#[test]
fn x17_got_forms_against_weak0_keep_the_slot() {
    let t = tables(TargetKind::Weak0);
    for output in [OutputKind::Static, OutputKind::Pie] {
        let s = scan(&t, output, RODATA, 42, -4);
        assert_eq!(raised(&s, key(1)), DemandFlags::GOT, "{output:?}");
        assert!(s.counts.is_empty(), "no RELATIVE: {s:?}");
    }
    // Never relaxed: the GOT load stays (bytes `48 8b 05`).
    let mut input = [0u8; 32];
    input[5..8].copy_from_slice(&[0x48, 0x8b, 0x05]);
    let mut c = data_case(&t, &input);
    c.name = b".text";
    c.exec = Some(true);
    let out = c.apply(&[Reloc { r_offset: 8, key: key(1), r_type: 42, addend: -4 }]).unwrap();
    assert_eq!(out[5..8], [0x48, 0x8b, 0x05]);
    assert_eq!(out[8..12], le(SLOT as i64 - 4 - P, 4)[..]);
    assert_eq!(mortar_x64::synth::static_slot(SlotKind::Addr, None, None, X64::M1.tls()), Ok(0));
}

#[test]
fn x18_thread_local_weak0_has_offset_0() {
    let mut t = Tables::default();
    t.tls_sym(key(1), 0, TargetKind::Weak0);
    let input = [0u8; 32];
    // No PT_TLS needed.
    let mut c = data_case(&t, &input);
    c.tls = None;
    assert_eq!(field(&c, 23, 5).unwrap(), le(5, 4));
    assert_eq!(field(&c, 18, 0).unwrap(), le(0, 8));
    assert_eq!(field(&c, 21, 7).unwrap(), le(7, 4));
    assert_eq!(field(&c, 17, 0).unwrap(), le(0, 8));
    let s = scan(&t, OutputKind::Static, RODATA, 23, 0);
    assert!(s.errors.is_empty(), "{s:?}");
    let slot = mortar_x64::synth::static_slot(
        SlotKind::TpOff { negated: false },
        None,
        None,
        X64::M1.tls(),
    );
    assert_eq!(slot, Ok(0));
}

// ---------------------------------------------------------------- X-46

#[test]
fn x46_dtpoff_by_section_kind() {
    let t = tls_tables(TargetKind::Local, 0x40_4008);
    let input = [0u8; 32];
    let mut c = data_case(&t, &input);
    // A data section of an executable: DtpRel.
    assert_eq!(field(&c, 21, 4).unwrap(), le(0xc, 4));
    assert_eq!(field(&c, 17, 0).unwrap(), le(8, 8));
    // Code of an executable: TpRel (LD → LE leaves TP as the module base).
    c.name = b".text";
    c.exec = Some(true);
    assert_eq!(field(&c, 21, 4).unwrap(), le(0x40_400c - TP_END as i64, 4));
    assert_eq!(field(&c, 17, 0).unwrap(), le(0x40_4008 - TP_END as i64, 8));
    // Unknown kind: refused by name, never guessed.
    c.exec = None;
    let Err(FieldError::Refused(r)) = field(&c, 21, 0) else { panic!() };
    assert_eq!(r.name, mortar_arch::RefName::Named(DTPOFF_PLACE));
    // A debug section: DtpRel whatever `exec` says.
    c.alloc = false;
    c.name = b".debug_info";
    assert_eq!(field(&c, 21, 0).unwrap(), le(8, 4));
}

// ---------------------------------------------------------------- X-53, W10

#[test]
fn x53_w10_tls_at_a_non_alloc_place_is_dtprel() {
    // GCC `-gsplit-dwarf`: `R_X86_64_64` against a TLS symbol in
    // `.debug_addr`. GNU ld and mold write the template address (W10).
    let t = tls_tables(TargetKind::Local, 0x40_4008);
    let s = scan(&t, OutputKind::Static, DEBUG, 1, 0);
    assert!(s.errors.is_empty(), "{s:?}");
    let input = [0u8; 32];
    let mut c = data_case(&t, &input);
    c.alloc = false;
    c.name = b".debug_addr";
    assert_eq!(field(&c, 1, 0).unwrap(), le(8, 8));
    assert_eq!(field(&c, 10, 4).unwrap(), le(12, 4));
}

#[test]
fn x53_dead_targets() {
    let mut t = tables(TargetKind::Local);
    t.syms.get_mut(&key(1)).unwrap().live = false;
    t.facts.get_mut(&key(1)).unwrap().dead = true;
    let input = [0u8; 32];
    let mut c = data_case(&t, &input);
    c.alloc = false;
    c.name = b".debug_info";
    assert_eq!(field(&c, 1, 0).unwrap(), le(-1, 8));
    assert_eq!(field(&c, 10, 0).unwrap(), le(-1, 4));
    c.name = b".debug_ranges";
    assert_eq!(field(&c, 1, 0).unwrap(), le(1, 8));
    // An address paired with a length (`DW_RLE_start_length`): 0.
    for name in [&b".debug_rnglists"[..], b".debug_loclists", b".debug_addr"] {
        c.name = name;
        assert_eq!(field(&c, 1, 0).unwrap(), le(0, 8));
    }
    c.alloc = true;
    c.name = b".gcc_except_table";
    assert_eq!(field(&c, 10, 0).unwrap(), le(0, 4));
    // Elsewhere scan reports the discarded section; nothing is demanded.
    let s = scan(&t, OutputKind::Static, RODATA, 42, -4);
    assert!(s.raised.is_empty());
    assert_eq!(s.events.len(), 1);
}

#[test]
fn rel_is_refused() {
    // X-2.
    let h = mortar_x64::table::howto(2).unwrap();
    let Err(FieldError::Refused(r)) = X64::M1.write_addend(&h, &mut [0; 4], 1) else { panic!() };
    assert_eq!(r.class, RefusalClass::SectionType);
}

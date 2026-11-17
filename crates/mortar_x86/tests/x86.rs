//! Behavioural tests of `mortar_x86` outside the TLS relaxations, by
//! section of `spec/x86.md`: REL addends (§3.2), the values of §6.1
//! (X86-19), the checks of §3.4 (X86-10 to X86-12, X86-38), the GOT32
//! operand (X86-13), the demands of §5 and the i386 cells (X86-15 to
//! X86-18, X86-27, X86-50, X86-58), undefined weak (§7), dead targets and
//! TLS at non-alloc places (X86-59, X86-61, X86-62), and the W-cases of
//! §15.2 that are not relaxations.
#![allow(clippy::many_single_char_names)]

mod support;

use mortar_arch::{
    Arch, DemandFlags, FieldError, GlobalFlags, Incompatible, OutputKind, PlaceFacts, Range,
    Refusal, ScanError, SlotKind, TargetKind, TlsBlock,
};
use mortar_diag::RefusalClass;
use mortar_x86::{GOT_PLACE, X86, ifunc_pie};
use support::{Case, Sink, Tables, key, raised, rel, scan_one, set_addend};

/// The section at `ADDR`; the target at `S`.
const ADDR: u64 = 0x0804_9000;
const S: u64 = 0x0804_a000;
const GOT: u64 = 0x0804_c000;
const SLOT: u64 = 0x0804_c00c;
const TP_SLOT: u64 = 0x0804_c010;
/// `TpRel(x) = x − 0x0804b010`, `DtpRel(x) = x − 0x0804b000` (§3.1, §13).
const TLS: TlsBlock = TlsBlock { start: 0x0804_b000, size: 0x10, align: 4 };
const TP_END: u64 = 0x0804_b010;

const DATA: PlaceFacts = PlaceFacts { alloc: true, writable: true, exec: None };
const TEXT: PlaceFacts = PlaceFacts { alloc: true, writable: false, exec: None };
const DEBUG: PlaceFacts = PlaceFacts { alloc: false, writable: false, exec: None };

fn tables(kind: TargetKind, s: u64) -> Tables {
    let mut t = Tables::default();
    t.sym(key(1), s, kind);
    t.syms.get_mut(&key(1)).unwrap().size = 12;
    t.slots.insert((key(1), SlotKind::Addr), SLOT);
    t.slots.insert((key(1), SlotKind::TpOff { negated: false }), TP_SLOT);
    t
}

fn tls_tables(kind: TargetKind, s: u64) -> Tables {
    let mut t = Tables::default();
    t.tls_sym(key(1), s, kind);
    t.slots.insert((key(1), SlotKind::TpOff { negated: false }), TP_SLOT);
    t
}

fn case<'a>(t: &'a Tables, input: &'a [u8], addr: u64) -> Case<'a> {
    let mut c = Case::new(t, input, addr);
    c.got_base = Some(GOT);
    c.tls = Some(TLS);
    c
}

/// The field of one record of `r_type` at offset 8 with implicit addend
/// `a`, in a 32-byte section at `addr` (zero-extended).
fn word_at(t: &Tables, addr: u64, r_type: u32, a: i64) -> Result<u32, FieldError> {
    let input = [0u8; 32];
    case(t, &input, addr).word(r_type, 8, key(1), a)
}

fn word(t: &Tables, r_type: u32, a: i64) -> Result<u32, FieldError> {
    word_at(t, ADDR, r_type, a)
}

fn range(r: Result<u32, FieldError>) -> Range {
    match r {
        Err(FieldError::Range(r)) => r,
        other => panic!("not a range error: {other:?}"),
    }
}

const P: u64 = ADDR + 8;

#[test]
fn x86_4_x86_5_the_addend_is_read_from_the_input_field() {
    let t = tables(TargetKind::Local, S);
    // `32`: `S + A`, A from the field; the written field is X, and a
    // second application over the same input gives the same bytes.
    let mut input = [0u8; 32];
    set_addend(&mut input, 8, 4, -8);
    let c = case(&t, &input, ADDR);
    let once = c.apply(&[rel(8, key(1), 1)]).unwrap();
    assert_eq!(once[8..12], ((S - 8) as u32).to_le_bytes());
    // Junk around the field is never read (X86-9).
    let mut junk = [0xa5u8; 32];
    set_addend(&mut junk, 8, 4, -8);
    let out = case(&t, &junk, ADDR).apply(&[rel(8, key(1), 1)]).unwrap();
    assert_eq!(out[8..12], once[8..12]);
    assert_eq!(out[..8], junk[..8]);
    assert_eq!(out[12..], junk[12..]);
}

#[test]
fn x86_19_values_of_6_1() {
    let t = tables(TargetKind::Local, S);
    let a = 0x10;
    assert_eq!(word(&t, 1, a), Ok((S + 0x10) as u32));
    assert_eq!(word(&t, 2, -4), Ok((S - 4 - P) as u32));
    assert_eq!(word(&t, 4, -4), Ok((S - 4 - P) as u32), "X86-29: PLT32 direct");
    assert_eq!(word(&t, 9, -8), Ok((S as i64 - 8 - GOT as i64) as u32), "GOTOFF");
    // X86-30: `GOTPC` is `GOT + A − P` whatever its symbol.
    assert_eq!(word(&t, 10, 2), Ok((GOT + 2 - P) as u32));
    assert_eq!(word(&t, 10, 7), Ok((GOT + 7 - P) as u32));
    assert_eq!(word(&t, 38, 0), Ok(12), "X86-12: SIZE32 = Z + A");
    assert_eq!(word(&t, 38, 4), Ok(16));
    assert_eq!(word(&t, 0, 0), Ok(0), "NONE writes nothing");
}

#[test]
fn x86_10_word_size_fields_are_modular() {
    // `.long allones + 2` → 1.
    let t = tables(TargetKind::Abs, 0xffff_ffff);
    assert_eq!(word(&t, 1, 2), Ok(1));
    // `.long sym − k`, `sym < k`.
    let t = tables(TargetKind::Abs, 0x10);
    assert_eq!(word(&t, 1, -0x20), Ok(0xffff_fff0));
    // A `PC32` from `P = 0xfffffff3` to 0x10 → 0x1d.
    let input = [0u8; 8];
    let c = case(&t, &input, 0xffff_fff0);
    assert_eq!(c.word(2, 3, key(1), 0), Ok(0x1d));
    // A call across the wrap (X86-43).
    assert_eq!(c.word(4, 3, key(1), -4), Ok(0x19));
}

#[test]
fn x86_10_w15_narrow_checks_are_on_the_reduced_value() {
    // `.word top + 0x20` with `top = 0xfffffff0` → 0x0010; `.byte` → 0x10.
    let t = tables(TargetKind::Abs, 0xffff_fff0);
    assert_eq!(word(&t, 20, 0x20), Ok(0x10));
    assert_eq!(word(&t, 22, 0x20), Ok(0x10));
    // `PC16`/`PC8` from `P = 0xfffffff0` to 0x10 → 0x20.
    let t = tables(TargetKind::Abs, 0x10);
    let input = [0u8; 32];
    let c = case(&t, &input, 0xffff_ffe0);
    assert_eq!(c.word(21, 0x10, key(1), 0), Ok(0x20));
    assert_eq!(c.word(23, 0x10, key(1), 0), Ok(0x20));
    // A `16` whose reduced value is 0x10000 is refused.
    let t = tables(TargetKind::Abs, 0x1_0000);
    assert_eq!(range(word(&t, 20, 0)).value, 0x1_0000);
}

#[test]
fn x86_11_w11_narrow_overflow_is_an_error() {
    let at = |s: u64| tables(TargetKind::Abs, s);
    // `16`: su(16) = [−32768, 65535].
    assert_eq!(word(&at(0xffff_8000), 20, 0), Ok(0x8000));
    assert_eq!(word(&at(0xffff), 20, 0), Ok(0xffff));
    assert_eq!(range(word(&at(0xffff_7fff), 20, 0)).value, -32769, "W11: not 0x7fff");
    assert_eq!(range(word(&at(0x1_0000), 20, 0)).value, 65536);
    // `8`: su(8) = [−128, 255].
    assert_eq!(word(&at(0xffff_ff80), 22, 0), Ok(0x80));
    assert_eq!(word(&at(0xff), 22, 0), Ok(0xff));
    assert_eq!(range(word(&at(0xffff_ff7f), 22, 0)).value, -129, "W11: not 0x7f");
    assert_eq!(range(word(&at(0x100), 22, 0)).value, 256);
    // X86-44: `PC8` = 128 is out of range.
    let t = at(P + 128);
    let r = range(word(&t, 23, 0));
    assert_eq!((r.value, r.min, r.max), (128, -128, 127));
    assert_eq!(word(&at(P + 127), 23, 0), Ok(127));
    assert_eq!(word(&at(P - 128), 23, 0), Ok(0x80));
}

#[test]
fn x86_6_w13_pc16_and_16_read_signed() {
    // A valid `PC16` with `A = −2` (W13: mold refuses it).
    let t = tables(TargetKind::Abs, P + 2);
    assert_eq!(word(&t, 21, -2), Ok(0));
    // `.word sym − 1` with sym = 0: −1, inside su(16).
    let t = tables(TargetKind::Abs, 0);
    assert_eq!(word(&t, 20, -1), Ok(0xffff));
    assert_eq!(word(&t, 22, -1), Ok(0xff));
    // Both check edges with negative addends.
    let t = tables(TargetKind::Abs, P);
    assert_eq!(word(&t, 21, -32768), Ok(0x8000));
    assert_eq!(word(&t, 23, -128), Ok(0x80));
    let t = tables(TargetKind::Abs, P - 1);
    assert_eq!(range(word(&t, 21, -32768)).value, -32769);
}

#[test]
fn x86_12_size32_is_u32_before_reduction() {
    let mut t = tables(TargetKind::Local, S);
    t.syms.get_mut(&key(1)).unwrap().size = 0;
    assert_eq!(range(word(&t, 38, -1)).value, -1);
}

#[test]
fn x86_38_the_tp_offset_is_checked_before_reduction() {
    let t = tls_tables(TargetKind::Local, 0x0804_b004);
    // `TLS_LE` = TpRel(S + A) = −12 + A.
    assert_eq!(word(&t, 17, 0), Ok((-12i32) as u32));
    assert_eq!(word(&t, 17, 4), Ok((-8i32) as u32));
    // −12 − 2^31: its 32-bit reduction fits, the integer does not.
    let r = range(word(&t, 17, i64::from(i32::MIN)));
    assert_eq!(r.value, -12 - (1i128 << 31));
}

#[test]
fn x86_61_tls_ldo_32_by_section_kind() {
    let t = tls_tables(TargetKind::Local, 0x0804_b004);
    let input = [0u8; 32];
    // In code: TpRel(S + A).
    let c = case(&t, &input, ADDR);
    assert_eq!(c.word(32, 8, key(1), 4), Ok((0x0804_b008 - TP_END as i64) as u32));
    // In `.rodata` and `.debug_info`: DtpRel(S + A).
    let mut c = case(&t, &input, ADDR);
    c.exec = Some(false);
    c.name = b".rodata";
    assert_eq!(c.word(32, 8, key(1), 4), Ok(8));
    let mut c = case(&t, &input, 0);
    c.alloc = false;
    c.exec = None;
    c.name = b".debug_info";
    assert_eq!(c.word(32, 8, key(1), 4), Ok(8));
    // Unknown kind in an allocated section: refused by name.
    let mut c = case(&t, &input, ADDR);
    c.exec = None;
    assert!(matches!(c.word(32, 8, key(1), 0), Err(FieldError::Refused(_))));
}

#[test]
fn x86_62_w10_32_against_tls_at_a_non_alloc_place_is_dtprel() {
    let t = tls_tables(TargetKind::Local, 0x0804_b068);
    let input = [0u8; 32];
    let mut c = case(&t, &input, 0);
    c.alloc = false;
    c.exec = None;
    c.name = b".debug_addr";
    assert_eq!(c.word(1, 8, key(1), 0), Ok(0x68), "W10: not the template address");
    // And `E` at an alloc place (X86-17, Q8).
    let s = scan_one(X86::M1, &t, OutputKind::Static, DATA, &[0; 32], rel(8, key(1), 1)).unwrap();
    assert_eq!(s.errors, [(0, ScanError::Incompatible(Incompatible::ThreadLocal))]);
    let s = scan_one(X86::M1, &t, OutputKind::Static, DEBUG, &[0; 32], rel(8, key(1), 1)).unwrap();
    assert!(s.errors.is_empty());
}

#[test]
fn x86_59_dead_targets_get_the_elfclass32_tombstone() {
    let mut t = tables(TargetKind::Local, S);
    t.syms.get_mut(&key(1)).unwrap().live = false;
    let mut input = [0u8; 32];
    set_addend(&mut input, 8, 4, 0x16);
    let mut c = case(&t, &input, 0);
    c.alloc = false;
    c.exec = None;
    for (name, v) in [
        (&b".debug_ranges"[..], 1u32),
        (b".debug_loc", 1),
        (b".debug_frame", 0xffff_ffff),
        (b".debug_info", 0),
        (b".debug_rnglists", 0),
        (b".debug_addr", 0),
    ] {
        c.name = name;
        let out = c.apply(&[rel(8, key(1), 1)]).unwrap();
        // The tombstone replaces the field, whatever the addend.
        assert_eq!(out[8..12], v.to_le_bytes(), "{}", String::from_utf8_lossy(name));
    }
}

/// WP-177: a dead target from `.eh_frame` or `.gcc_except_table` (alloc
/// or not) is the event `DiscardedSection`, for the scan's driver to judge
/// by the record that owns the field; from a debug name it is not.
#[test]
fn x86_59_dead_targets_from_eh_sections_are_events() {
    let mut t = tables(TargetKind::Local, S);
    t.syms.get_mut(&key(1)).unwrap().live = false;
    t.facts.get_mut(&key(1)).unwrap().dead = true;
    let input = [0u8; 32];
    let na = PlaceFacts { alloc: false, writable: false, exec: None };
    for (place, name, n) in [
        (TEXT, &b".eh_frame"[..], 1),
        (TEXT, b".gcc_except_table", 1),
        (na, b".gcc_except_table", 1),
        (na, b".debug_info", 0),
    ] {
        let mut sink = Sink::default();
        support::scan_named(
            X86::M1,
            &t,
            OutputKind::Static,
            place,
            name,
            &input,
            &[rel(8, key(1), 2)],
            &mut sink,
        )
        .unwrap();
        assert!(sink.raised.is_empty() && sink.errors.is_empty());
        assert_eq!(sink.events.len(), n, "{}", String::from_utf8_lossy(name));
    }
}

#[test]
fn s7_undefined_weak_values() {
    let t = tables(TargetKind::Weak0, 0x1234);
    assert_eq!(word(&t, 1, 4), Ok(4), "`32`: 0 + A");
    assert_eq!(word(&t, 2, -4), Ok((-4 - P as i64) as u32), "PC32 static: A − P");
    assert_eq!(word(&t, 4, -4), Ok((-4 - P as i64) as u32), "PLT32: a call to 0");
    assert_eq!(word(&t, 9, 0), Ok((0 - GOT as i64) as u32), "GOTOFF static: −GOT");
    // X86-23: thread-local `weak0`.
    let t = tls_tables(TargetKind::Weak0, 0x1234);
    assert_eq!(word(&t, 17, 4), Ok(4), "TLS_LE writes A");
    assert_eq!(word(&t, 32, 4), Ok(4), "TLS_LDO_32 writes A");
}

fn scan(t: &Tables, output: OutputKind, place: PlaceFacts, input: &[u8], r_type: u32) -> Sink {
    scan_one(X86::M1, t, output, place, input, rel(8, key(1), r_type)).unwrap()
}

fn refused(name: &'static str) -> ScanError {
    ScanError::Refused(Refusal::named(RefusalClass::Feature, name))
}

#[test]
fn s7_x86_21_x86_22_weak0_scan_cells() {
    let t = tables(TargetKind::Weak0, 0);
    let z = [0u8; 32];
    // Static: GOTOFF against weak0 is accepted and needs the GOT base.
    let s = scan(&t, OutputKind::Static, TEXT, &z, 9);
    assert!(s.errors.is_empty(), "{:?}", s.errors);
    assert!(s.global.contains(GlobalFlags::GOT_BASE));
    for r_type in [2, 21, 23] {
        let s = scan(&t, OutputKind::Static, TEXT, &z, r_type);
        assert!(s.errors.is_empty(), "{r_type}");
    }
    // Static-pie: `Pc` and `GotRel` are E, "recompile with -fPIE".
    for r_type in [2, 9, 21, 23] {
        let s = scan(&t, OutputKind::Pie, TEXT, &z, r_type);
        assert_eq!(
            s.errors,
            [(0, ScanError::Incompatible(Incompatible::RecompilePie))],
            "{r_type}"
        );
    }
    // `32` and PLT32 are accepted in static-pie (§7).
    let s = scan(&t, OutputKind::Pie, DATA, &z, 1);
    assert!(s.errors.is_empty() && s.counts.is_empty(), "{s:?}");
    let s = scan(&t, OutputKind::Pie, TEXT, &z, 4);
    assert!(s.errors.is_empty());
}

#[test]
fn x86_50_w5_w6_w7_ifunc_cells() {
    let t = tables(TargetKind::Ifunc, S);
    let z = [0u8; 32];
    // Static: every form takes the IPLT entry (the canonical address).
    for r_type in [1, 2, 4, 9] {
        let s = scan(&t, OutputKind::Static, DATA, &z, r_type);
        assert!(s.errors.is_empty(), "{r_type}");
        assert!(raised(&s, key(1)).contains(DemandFlags::IPLT), "{r_type}");
    }
    // Static-pie: GOTOFF and PC32 against an IFUNC are refused by name.
    for r_type in [2u32, 9, 21, 23] {
        let s = scan(&t, OutputKind::Pie, TEXT, &z, r_type);
        assert_eq!(s.errors, [(0, refused(ifunc_pie(r_type)))], "{r_type}");
    }
    assert_eq!(ifunc_pie(9), "R_386_GOTOFF against an IFUNC in static-pie");
    assert_eq!(ifunc_pie(2), "R_386_PC32 against an IFUNC in static-pie");
    // PLT32 goes to the IPLT entry.
    let s = scan(&t, OutputKind::Pie, TEXT, &z, 4);
    assert!(s.errors.is_empty());
    // X86-28: a GOT form keeps its slot (and the IPLT).
    let mut code = [0u8; 32];
    code[6..8].copy_from_slice(&[0x8b, 0x83]);
    let s = scan(&t, OutputKind::Static, TEXT, &code, 43);
    assert!(raised(&s, key(1)).contains(DemandFlags::GOT | DemandFlags::IPLT));
}

#[test]
fn x86_15_x86_58_non_pic_in_static_pie() {
    let t = tables(TargetKind::Local, S);
    let z = [0u8; 32];
    // `32` in text: a text relocation.
    let s = scan(&t, OutputKind::Pie, TEXT, &z, 1);
    assert_eq!(s.errors, [(0, ScanError::Incompatible(Incompatible::ReadOnly))]);
    // In writable data: a RELATIVE.
    let s = scan(&t, OutputKind::Pie, DATA, &z, 1);
    assert!(s.errors.is_empty());
    assert_eq!(s.counts.len(), 1);
    // An absolute target needs nothing.
    let a = tables(TargetKind::Abs, 0x10);
    let s = scan(&a, OutputKind::Pie, TEXT, &z, 1);
    assert!(s.errors.is_empty() && s.counts.is_empty());
    // `16`/`8` in Pie: E except against `abs` (§5).
    let s = scan(&t, OutputKind::Pie, DATA, &z, 20);
    assert_eq!(s.errors.len(), 1);
    let s = scan(&a, OutputKind::Pie, DATA, &z, 22);
    assert!(s.errors.is_empty());
}

/// A 32-byte code section with `prefix` just before offset 8.
fn code(prefix: &[u8]) -> [u8; 32] {
    let mut b = [0u8; 32];
    b[8 - prefix.len()..8].copy_from_slice(prefix);
    b
}

/// The GOT32/GOT32X value at offset 8 after `prefix`, or the error.
fn got_word(output: OutputKind, prefix: &[u8], r_type: u32, a: i64) -> Result<u32, FieldError> {
    let t = tables(TargetKind::Local, S);
    let input = code(prefix);
    let mut c = case(&t, &input, ADDR);
    c.output = output;
    c.word(r_type, 8, key(1), a)
}

#[test]
fn x86_13_the_got32_operand() {
    let base = |a: i64| Ok((SLOT as i64 + a - GOT as i64) as u32);
    let none = |a: i64| Ok((SLOT as i64 + a) as u32);
    // `ff a3` is base under both types (static-pie: GOT32X not relaxed).
    assert_eq!(got_word(OutputKind::Static, &[0xff, 0xa3], 3, 0), base(0));
    assert_eq!(got_word(OutputKind::Pie, &[0xff, 0xa3], 43, 0), base(0));
    // `review.sh tail`: `ff a0`, `ff a2` (GOT32) are base.
    assert_eq!(got_word(OutputKind::Static, &[0xff, 0xa0], 3, 4), base(4));
    assert_eq!(got_word(OutputKind::Pie, &[0xff, 0xa2], 3, 0), base(0));
    // No base: `8b 05`, `ff 15`, `ff 25` (GOT32, never relaxed: X86-26).
    assert_eq!(got_word(OutputKind::Static, &[0x8b, 0x05], 3, 0), none(0));
    assert_eq!(got_word(OutputKind::Static, &[0xff, 0x15], 3, 0), none(0));
    assert_eq!(got_word(OutputKind::Static, &[0x8b, 0x1d], 3, 0), none(0));
    // `disp32(,%index,s)`: SIB 05 after ModRM 04 agrees: no base.
    assert_eq!(got_word(OutputKind::Static, &[0x04, 0x05], 3, 0), none(0));
    // §3.4 corners, written as GNU ld does: `a3` (moffs store, W2), `b8`,
    // `68` base; `05` none.
    assert_eq!(got_word(OutputKind::Static, &[0xa3], 3, 0), base(0));
    assert_eq!(got_word(OutputKind::Static, &[0xb8], 3, 0), base(0));
    assert_eq!(got_word(OutputKind::Static, &[0x68], 3, 0), base(0));
    assert_eq!(got_word(OutputKind::Static, &[0x05], 3, 0), none(0));
    // classify gives `G + A` without a base, `G + A − GOT` with one.
    let input = code(&[0x8b, 0x05]);
    let h = X86::M1.classify(3, mortar_arch::Site { bytes: &input, offset: 8 }).unwrap();
    assert_eq!(h.value.minus, mortar_arch::Minus::None);
    assert_eq!(h.window, mortar_arch::Window { before: 2, after: 0 });
}

#[test]
fn x86_13_w1_w8_ambiguous_operands_are_refused() {
    let t = tables(TargetKind::Local, S);
    for (prefix, r_type) in [
        (&[0x8b, 0x84, 0x0d][..], 43), // W1: movl foo@GOT(%ebp,%ecx,1)
        (&[0x8b, 0x84, 0x0d], 3),
        (&[0x03, 0x84, 0x15], 43), // W8: addl w8v@GOT(%ebp,%edx,1)
        (&[0x84, 0x0d], 3),        // testb %cl, foo@GOT
        (&[0x0f, 0xbc, 0x0d], 3),  // bsfl foo@GOT, %ecx
    ] {
        let input = code(prefix);
        for output in [OutputKind::Static, OutputKind::Pie] {
            let s = scan(&t, output, TEXT, &input, r_type);
            assert_eq!(
                s.errors,
                [(0, ScanError::Incompatible(Incompatible::InstructionMismatch))],
                "{prefix:02x?} {r_type}"
            );
            let mut c = case(&t, &input, ADDR);
            c.output = output;
            assert!(matches!(c.word(r_type, 8, key(1), 0), Err(FieldError::Instruction { .. })));
        }
    }
    // A record at section offset 0 has no ModRM byte.
    let input = [0u8; 8];
    let s = scan_one(X86::M1, &t, OutputKind::Static, TEXT, &input, rel(0, key(1), 3)).unwrap();
    assert_eq!(s.errors, [(0, ScanError::Incompatible(Incompatible::InstructionMismatch))]);
}

#[test]
fn x86_13_step_1_got32_outside_code_is_refused() {
    // `.long foo@GOT` in `.data`.
    let t = tables(TargetKind::Local, S);
    let input = [0u8; 32];
    let mut c = case(&t, &input, ADDR);
    c.exec = Some(false);
    c.name = b".data";
    assert!(matches!(c.word(3, 8, key(1), 0), Err(FieldError::Instruction { .. })));
    c.exec = None;
    assert_eq!(
        c.word(3, 8, key(1), 0),
        Err(FieldError::Refused(Refusal::named(RefusalClass::Feature, GOT_PLACE)))
    );
    // At scan, where `PlaceFacts.exec` is known (arch.md §3.4), the same
    // record is refused there; unknown, scan leaves it to fill.
    let data_known = PlaceFacts { exec: Some(false), ..DATA };
    for r_type in [3, 43] {
        let s = scan(&t, OutputKind::Static, data_known, &input, r_type);
        assert_eq!(s.errors, [(0, ScanError::Incompatible(Incompatible::InstructionMismatch))]);
        let s = scan(&t, OutputKind::Static, DATA, &input, r_type);
        assert!(s.errors.is_empty(), "{:?}", s.errors);
    }
    let text_known = PlaceFacts { exec: Some(true), ..TEXT };
    let s = scan(&t, OutputKind::Static, text_known, &code(&[0x8b, 0x83]), 43);
    assert!(s.errors.is_empty(), "{:?}", s.errors);
    // A non-alloc GOT form is E (X86-59).
    let s = scan(&t, OutputKind::Static, DEBUG, &input, 3);
    assert_eq!(s.errors, [(0, ScanError::Incompatible(Incompatible::NotInNonAlloc))]);
}

#[test]
fn x86_27_w3_no_base_got_forms() {
    let t = tables(TargetKind::Local, S);
    // W3: `call *foo@GOT` (`ff 15`) is the absolute slot address in
    // static, never `G − .got`.
    assert_eq!(got_word(OutputKind::Static, &[0xff, 0x15], 3, 0), Ok(SLOT as u32));
    // Static-pie: a text relocation, refused at scan.
    let input = code(&[0xff, 0x15]);
    for r_type in [3, 43] {
        let s = scan(&t, OutputKind::Pie, TEXT, &input, r_type);
        assert_eq!(s.errors, [(0, ScanError::Incompatible(Incompatible::ReadOnly))]);
    }
    // Every GOT form needs `_GLOBAL_OFFSET_TABLE_` (X86-30) and a slot.
    let input = code(&[0xff, 0xa3]);
    let s = scan(&t, OutputKind::Static, TEXT, &input, 3);
    assert!(s.global.contains(GlobalFlags::GOT_BASE));
    assert!(raised(&s, key(1)).contains(DemandFlags::GOT));
}

#[test]
fn x86_24_got_forms_against_weak0_keep_a_zero_slot() {
    let t = tables(TargetKind::Weak0, 0);
    let input = code(&[0xff, 0x93]);
    let s = scan(&t, OutputKind::Pie, TEXT, &input, 43);
    assert!(s.errors.is_empty());
    assert!(raised(&s, key(1)).contains(DemandFlags::GOT));
}

#[test]
fn x86_20_w14_dynamic_types_in_an_input_are_refused() {
    for t in [5, 6, 7, 8, 14, 35, 36, 37, 41, 42] {
        let r = X86::M1.classify(t, mortar_arch::Site { bytes: &[0; 16], offset: 8 }).unwrap_err();
        assert_eq!(r.class, RefusalClass::RelocationType, "{t}");
    }
}

#[test]
fn x86_17_tls_mismatch_and_le() {
    let t = tables(TargetKind::Local, S);
    let z = [0u8; 32];
    let s = scan(&t, OutputKind::Static, TEXT, &z, 17);
    assert_eq!(s.errors, [(0, ScanError::Incompatible(Incompatible::NotThreadLocal))]);
    let t = tls_tables(TargetKind::Local, 0x0804_b004);
    for r_type in [17, 32] {
        let s = scan(&t, OutputKind::Static, TEXT, &z, r_type);
        assert!(s.errors.is_empty() && raised(&s, key(1)).is_empty(), "{r_type}");
    }
}

#[test]
fn undefined_targets_report_undefined() {
    let t = tables(TargetKind::Undefined, 0);
    let s = scan(&t, OutputKind::Static, DATA, &[0; 32], 1);
    assert_eq!(s.undefined, [(key(1), 0)]);
}

#[test]
fn outside_the_section_is_malformed() {
    let t = tables(TargetKind::Local, S);
    let s = scan_one(X86::M1, &t, OutputKind::Static, DATA, &[0; 10], rel(8, key(1), 1)).unwrap();
    assert_eq!(s.errors, [(0, ScanError::Outside)]);
}

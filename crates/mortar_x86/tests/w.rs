//! The wrong-code cases of x86.md §15.2, W1–W16: for each, the input a
//! reference mislinks, and what mortar does (the right bytes, or a refusal
//! by name). W12 (`SHT_RELA` in an i386 object, X86-2) is refused before
//! the arch is called; its test is the driver's (stage B).
#![allow(clippy::many_single_char_names)]

mod support;

use mortar_arch::{
    DemandFlags, FieldError, Incompatible, OutputKind, PlaceFacts, Range, Refusal, ScanError,
    SlotKind, TargetKind, TlsBlock,
};
use mortar_diag::RefusalClass;
use mortar_x86::{X86, ifunc_pie};
use support::{Case, Sink, Tables, key, raised, rel, scan_all, scan_one};

const ADDR: u64 = 0x0804_9000;
const GOT: u64 = 0x080e_5000;
const SLOT: u64 = 0x080e_500c;
const S: u64 = 0x080e_6020;
const TLS: TlsBlock = TlsBlock { start: 0x080e_4900, size: 0x20, align: 4 };
const TEXT: PlaceFacts = PlaceFacts { alloc: true, writable: false, exec: None };
const DATA: PlaceFacts = PlaceFacts { alloc: true, writable: true, exec: None };

fn tables(kind: TargetKind, s: u64) -> Tables {
    let mut t = Tables::default();
    t.sym(key(1), s, kind);
    t.slots.insert((key(1), SlotKind::Addr), SLOT);
    t
}

/// 32 bytes with `prefix` ending at 8, the field at 8 holding `a`.
fn code(prefix: &[u8], a: i32) -> Vec<u8> {
    let mut b = vec![0xcc; 32];
    b[8 - prefix.len()..8].copy_from_slice(prefix);
    b[8..12].copy_from_slice(&a.to_le_bytes());
    b
}

fn case<'a>(t: &'a Tables, input: &'a [u8], output: OutputKind) -> Case<'a> {
    let mut c = Case::new(t, input, ADDR);
    c.output = output;
    c.got_base = Some(GOT);
    c.tls = Some(TLS);
    c
}

fn field(out: &[u8]) -> u32 {
    u32::from_le_bytes(out[8..12].try_into().unwrap())
}

fn scan(t: &Tables, output: OutputKind, place: PlaceFacts, input: &[u8], r_type: u32) -> Sink {
    scan_one(X86::M1, t, output, place, input, rel(8, key(1), r_type)).unwrap()
}

fn mismatch() -> Vec<(u32, ScanError)> {
    vec![(0, ScanError::Incompatible(Incompatible::InstructionMismatch))]
}

#[test]
fn w1_sib_0d_has_a_base_and_is_refused() {
    // `movl foo@GOT(%ebp,%ecx,1), %eax`: GNU ld and lld write `G + A`;
    // its two X86-13 readings disagree: refused, in both outputs.
    let t = tables(TargetKind::Local, S);
    let input = code(&[0x8b, 0x84, 0x0d], 0);
    for output in [OutputKind::Static, OutputKind::Pie] {
        for r_type in [3, 43] {
            assert_eq!(scan(&t, output, TEXT, &input, r_type).errors, mismatch());
            let r = case(&t, &input, output).apply(&[rel(8, key(1), r_type)]);
            assert!(matches!(r, Err(FieldError::Instruction { .. })), "{output:?} {r_type}");
        }
    }
}

#[test]
fn w2_moffs_store_is_read_as_base() {
    // `movl %eax, foo@GOT` (`a3`): as GNU ld and lld (X86-13, Q4).
    let t = tables(TargetKind::Local, S);
    let input = code(&[0xa3], 0);
    let out = case(&t, &input, OutputKind::Static).apply(&[rel(8, key(1), 3)]).unwrap();
    assert_eq!(field(&out), (SLOT - GOT) as u32);
}

#[test]
fn w3_no_base_got_forms_get_the_slot_address() {
    // `call *foo@GOT` (`ff 15`) with `GOT32` (-fno-plt, unrelaxable):
    // the absolute slot address, never mold's `G − .got`.
    let t = tables(TargetKind::Local, S);
    for prefix in [&[0xff, 0x15][..], &[0xff, 0x25], &[0x8b, 0x05], &[0x8b, 0x1d]] {
        let input = code(prefix, 0);
        let out = case(&t, &input, OutputKind::Static).apply(&[rel(8, key(1), 3)]).unwrap();
        assert_eq!(field(&out), SLOT as u32, "{prefix:02x?}");
    }
    // Against a shared target there is no relaxation; a `GOT32X` call to
    // an IFUNC keeps the slot, absolute.
    let t = tables(TargetKind::Ifunc, S);
    let input = code(&[0xff, 0x15], 0);
    let out = case(&t, &input, OutputKind::Static).apply(&[rel(8, key(1), 43)]).unwrap();
    assert_eq!(field(&out), SLOT as u32);
}

#[test]
fn w4_d3_is_rewritten_from_p_minus_2() {
    // `8d 83 <f>; e8 <rel>; 90`: the 12-byte LE output covers [P − 2,
    // P + 10), not mold's [P − 3, P + 9).
    let mut t = Tables::default();
    t.tls_sym(key(1), TLS.start + 0x10, TargetKind::Local);
    t.sym(key(2), 0, TargetKind::Undefined);
    t.names.insert(key(2), b"___tls_get_addr".to_vec());
    let mut input = vec![0x5a; 32];
    input[6..18].copy_from_slice(&[0x8d, 0x83, 0, 0, 0, 0, 0xe8, 0xfc, 0xff, 0xff, 0xff, 0x90]);
    let relocs = [rel(8, key(1), 18), rel(13, key(2), 4)];
    let out = case(&t, &input, OutputKind::Static).apply(&relocs).unwrap();
    // T = TpRel(S) = S − (start + size) = −0x10; `subl $0x10, %eax`.
    let mut want = input.clone();
    want[6..18].copy_from_slice(&[0x65, 0xa1, 0, 0, 0, 0, 0x81, 0xe8, 0x10, 0, 0, 0]);
    assert_eq!(out, want);
    assert_eq!(out[5], 0x5a, "the previous instruction is untouched");
}

#[test]
fn w5_gotoff_against_an_ifunc_in_static_is_its_iplt_entry() {
    // `S` of an IFUNC in static is its IPLT entry (X86-50); `GOTOFF` is
    // entry − GOT, not GNU ld's entry − 12 − GOT.
    let entry = 0x0804_9030;
    let t = tables(TargetKind::Ifunc, entry);
    let input = code(&[0x8d, 0x83], 0);
    let s = scan(&t, OutputKind::Static, TEXT, &input, 9);
    assert!(s.errors.is_empty() && raised(&s, key(1)).contains(DemandFlags::IPLT), "{s:?}");
    let out = case(&t, &input, OutputKind::Static).apply(&[rel(8, key(1), 9)]).unwrap();
    assert_eq!(field(&out), entry.wrapping_sub(GOT) as u32);
}

#[test]
fn w6_w7_static_pie_ifunc_addresses_are_refused() {
    // A `GOTOFF` or `PC32` against an IFUNC in static-pie would need
    // lld's `%ebx`-relative IPLT entry as the canonical address (W6) or
    // give a second address (W7): refused by name (X86-50, Q9).
    let t = tables(TargetKind::Ifunc, S);
    let input = code(&[0x8d, 0x83], 0);
    for r_type in [9, 2] {
        let s = scan(&t, OutputKind::Pie, TEXT, &input, r_type);
        let want = Refusal::named(RefusalClass::Feature, ifunc_pie(r_type));
        assert_eq!(s.errors, [(0, ScanError::Refused(want))], "{r_type}");
    }
    // The GOT form keeps one address: the slot (IRELATIVE).
    let input = code(&[0x8b, 0x83], 0);
    let s = scan(&t, OutputKind::Pie, TEXT, &input, 43);
    assert!(s.errors.is_empty() && raised(&s, key(1)).contains(DemandFlags::GOT));
}

#[test]
fn w8_two_readings_are_refused_as_such() {
    // `addl w8v@GOT(%ebp,%edx,1), %eax` (`03 84 15`): not "no base", but
    // an operand with two readings (X86-13 step 3).
    let t = tables(TargetKind::Local, S);
    let input = code(&[0x03, 0x84, 0x15], 0);
    for output in [OutputKind::Static, OutputKind::Pie] {
        assert_eq!(scan(&t, output, TEXT, &input, 43).errors, mismatch(), "{output:?}");
    }
}

#[test]
fn w9_weak0_gotoff_and_pc32_in_static_pie_are_refused() {
    // Both would give the load base for `&wf` (§7).
    let t = tables(TargetKind::Weak0, 0);
    for (place, r_type) in [(TEXT, 9), (TEXT, 2), (DATA, 2)] {
        let s = scan(&t, OutputKind::Pie, place, &code(&[0x8d, 0x83], 0), r_type);
        assert_eq!(s.errors, [(0, ScanError::Incompatible(Incompatible::RecompilePie))]);
    }
    // In static they write 0's value.
    let input = code(&[0x8d, 0x83], 0);
    let out = case(&t, &input, OutputKind::Static).apply(&[rel(8, key(1), 9)]).unwrap();
    assert_eq!(field(&out), 0u32.wrapping_sub(GOT as u32));
}

#[test]
fn w10_split_dwarf_tls_locations_are_offsets() {
    let mut t = Tables::default();
    t.tls_sym(key(1), TLS.start + 0x10, TargetKind::Local);
    let input = code(&[], 0);
    let mut c = case(&t, &input, OutputKind::Static);
    c.addr = 0;
    c.alloc = false;
    c.exec = None;
    c.name = b".debug_addr";
    let out = c.apply(&[rel(8, key(1), 1)]).unwrap();
    assert_eq!(field(&out), 0x10, "not 0x080e4910");
}

#[test]
fn w11_narrow_overflow_is_refused() {
    // −32769 in a `16` and −129 in an `8` (GNU ld writes 0x7fff, 0x7f).
    for (r_type, v) in [(20u32, -32769i64), (22, -129)] {
        let t = tables(TargetKind::Abs, u64::from(v as u32));
        let input = vec![0u8; 16];
        let r = case(&t, &input, OutputKind::Static).apply(&[rel(8, key(1), r_type)]);
        let Err(FieldError::Range(Range { value, .. })) = r else { panic!("{r_type}: {r:?}") };
        assert_eq!(value, i128::from(v));
    }
}

#[test]
fn w13_pc16_addend_is_signed() {
    // `PC16` with `A = −2` (bytes `fe ff`) and `S = P + 2`: 0.
    let t = tables(TargetKind::Abs, ADDR + 8 + 2);
    let mut input = vec![0u8; 16];
    input[8..10].copy_from_slice(&[0xfe, 0xff]);
    let out = case(&t, &input, OutputKind::Static).apply(&[rel(8, key(1), 21)]).unwrap();
    assert_eq!(out[8..10], [0, 0]);
}

#[test]
fn w14_dynamic_types_in_an_input_are_refused_by_name() {
    let t = tables(TargetKind::Local, S);
    for r_type in [5u32, 6, 7, 8, 14, 35, 36, 37, 41, 42] {
        let mut sink = Sink::default();
        let r = scan_all(
            X86::M1,
            &t,
            OutputKind::Static,
            DATA,
            &[0; 16],
            &[rel(8, key(1), r_type)],
            &mut sink,
        );
        let refusal = r.expect_err("classify refuses the type");
        assert_eq!(refusal.class, RefusalClass::RelocationType, "{r_type}");
        assert!(matches!(refusal.name, mortar_arch::RefName::Named(_)), "{r_type}");
    }
}

#[test]
fn w15_narrow_fields_are_checked_after_the_reduction() {
    // `.word top + 0x20`, `top = 0xfffffff0`: 0x10, not mold's refusal.
    let t = tables(TargetKind::Abs, 0xffff_fff0);
    for r_type in [20u32, 22] {
        let mut input = vec![0u8; 16];
        input[8] = 0x20;
        let out = case(&t, &input, OutputKind::Static).apply(&[rel(8, key(1), r_type)]).unwrap();
        assert_eq!(out[8], 0x10, "{r_type}");
    }
}

#[test]
fn w16_absolute_targets_keep_one_value_in_static_pie() {
    // In static-pie no `GOT32X` is relaxed (X86-25): every reference to
    // an absolute symbol goes through its slot, whose value is `S`.
    let t = tables(TargetKind::Abs, 0);
    let input = code(&[0x8b, 0x83], 0);
    let out = case(&t, &input, OutputKind::Pie).apply(&[rel(8, key(1), 43)]).unwrap();
    assert_eq!(out[6..8], [0x8b, 0x83], "not rewritten to lea");
    assert_eq!(field(&out), (SLOT - GOT) as u32);
    let s = scan(&t, OutputKind::Pie, TEXT, &input, 43);
    assert!(s.errors.is_empty() && raised(&s, key(1)).contains(DemandFlags::GOT));
}

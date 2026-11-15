//! The relaxations of x64.md §8, byte for byte, every register and REX
//! form: the expected bytes are built here from §8's table, and the whole
//! section is compared, so a byte written outside the form's output is a
//! failure (X-5, X-8).
#![allow(clippy::many_single_char_names)]

mod support;

use mortar_arch::{
    Arch, DemandFlags, EventClass, FieldError, OutputKind, PlaceFacts, RefName, Reloc, RelocAt,
    ScanError, Site, SlotKind, TargetKind, TlsBlock, UnitWindow, Window,
};
use mortar_core::{FileId, SectionId};
use mortar_x64::forms::BINOPS;
use mortar_x64::{
    TLS_TO_IE, UNRECOGNIZED_DESC, UNRECOGNIZED_GD, UNRECOGNIZED_LD, X64, relax, shown_bytes,
};
use support::{Case, Sink, Tables, all_code, key, raised, scan_all};

/// Section address; the section is `LEN` bytes of `int3` with the form at
/// `AT` (`P` = `AT` + the form's bytes before the field).
const ADDR: u64 = 0x40_1000;
const LEN: usize = 64;
const AT: usize = 16;
const TEXT: PlaceFacts = PlaceFacts { alloc: true, writable: false };
const TLS: TlsBlock = TlsBlock { start: 0x40_4000, size: 0x10, align: 8 };
/// The TLS target's address and `T = TpRel(S) = −8`.
const TLS_S: u64 = 0x40_4008;
const T: [u8; 4] = (-8i32).to_le_bytes();
/// A non-TLS target and its GOT slot.
const S: u64 = 0x40_2468;
const SLOT: u64 = 0x40_3800;

const SYM: u32 = 1;
const GET_ADDR: u32 = 2;

fn tables(kind: TargetKind, tls: bool, s: u64) -> Tables {
    let mut t = Tables::default();
    if tls {
        t.tls_sym(key(SYM), s, kind);
    } else {
        t.sym(key(SYM), s, kind);
    }
    t.slots.insert((key(SYM), SlotKind::Addr), SLOT);
    t.slots.insert((key(SYM), SlotKind::TpOff { negated: false }), SLOT + 8);
    t.sym(key(GET_ADDR), 0, TargetKind::Undefined);
    t.names.insert(key(GET_ADDR), b"__tls_get_addr".to_vec());
    t
}

/// `LEN` bytes of `cc` with `form` at `AT`.
fn section(form: &[u8]) -> Vec<u8> {
    let mut b = vec![0xcc; LEN];
    b[AT..AT + form.len()].copy_from_slice(form);
    b
}

fn rel(off: usize, k: u32, r_type: u32, addend: i64) -> Reloc {
    Reloc { r_offset: off as u64, key: key(k), r_type, addend }
}

fn case<'a>(t: &'a Tables, input: &'a [u8], output: OutputKind) -> Case<'a> {
    let mut c = Case::new(t, input, ADDR);
    c.output = output;
    c.tls = Some(TLS);
    c.got_base = Some(0x40_3000);
    c
}

/// The section after applying `relocs` (all of them, in order).
fn applied(t: &Tables, input: &[u8], output: OutputKind, relocs: &[Reloc]) -> Vec<u8> {
    case(t, input, output).apply(relocs).unwrap()
}

/// `input` with `bytes` at `off`.
fn with(input: &[u8], off: usize, bytes: &[u8]) -> Vec<u8> {
    let mut b = input.to_vec();
    b[off..off + bytes.len()].copy_from_slice(bytes);
    b
}

fn scan(t: &Tables, output: OutputKind, input: &[u8], relocs: &[Reloc]) -> Sink {
    let mut sink = Sink::default();
    scan_all(X64::M1, t, output, TEXT, input, relocs, &mut sink).unwrap();
    sink
}

fn refused_name(e: &ScanError) -> &'static str {
    match e {
        ScanError::Refused(r) => match r.name {
            RefName::Named(n) => n,
            other @ RefName::Hex(_) => panic!("unnamed refusal {other:?}"),
        },
        other => panic!("not a refusal: {other:?}"),
    }
}

/// `S + A − P` for a field at `p`, `A = −4`.
fn pc(s: u64, p: usize) -> [u8; 4] {
    (s.wrapping_sub(4).wrapping_sub(ADDR + p as u64) as i32).to_le_bytes()
}

const fn rex_prime(rex: u8) -> u8 {
    match rex {
        0x48 => 0x48,
        0x4c => 0x49,
        _ => panic!(),
    }
}

const REXES: [u8; 2] = [0x48, 0x4c];
const EXEC: [OutputKind; 2] = [OutputKind::Static, OutputKind::Pie];

// ---------------------------------------------------------------- §8.1 G1–G6

#[test]
fn g1_mov_becomes_lea_in_every_output() {
    let t = tables(TargetKind::Local, false, S);
    for output in [OutputKind::Static, OutputKind::Pie, OutputKind::Shared] {
        for rex in REXES {
            for reg in 0..8u8 {
                let modrm = (reg << 3) | 5;
                let input = section(&[rex, 0x8b, modrm, 0, 0, 0, 0]);
                let p = AT + 3;
                let out = applied(&t, &input, output, &[rel(p, SYM, 42, -4)]);
                let mut want = with(&input, AT + 1, &[0x8d, modrm]);
                want[p..p + 4].copy_from_slice(&pc(S, p));
                assert_eq!(out, want, "{output:?} rex {rex:#x} reg {reg}");
            }
        }
    }
}

#[test]
fn g2_g3_g4() {
    let t = tables(TargetKind::Local, false, S);
    let p = AT + 2;
    for output in [OutputKind::Static, OutputKind::Pie] {
        for reg in 0..8u8 {
            let modrm = (reg << 3) | 5;
            let input = section(&[0x8b, modrm, 0, 0, 0, 0]);
            let out = applied(&t, &input, output, &[rel(p, SYM, 41, -4)]);
            let want = with(&with(&input, AT, &[0x8d, modrm]), p, &pc(S, p));
            assert_eq!(out, want, "G2 reg {reg}");
        }
        // G3: `ff 15` → `67 e8`.
        let input = section(&[0xff, 0x15, 0, 0, 0, 0]);
        let out = applied(&t, &input, output, &[rel(p, SYM, 41, -4)]);
        assert_eq!(out, with(&with(&input, AT, &[0x67, 0xe8]), p, &pc(S, p)));
        // G4: `ff 25` → `e9 <S + A − P + 1> 90` from `P − 2`.
        let input = section(&[0xff, 0x25, 0, 0, 0, 0]);
        let out = applied(&t, &input, output, &[rel(p, SYM, 41, -4)]);
        let x = (S.wrapping_sub(4).wrapping_sub(ADDR + p as u64) as i32 + 1).to_le_bytes();
        let mut want = with(&input, AT, &[0xe9]);
        want[AT + 1..AT + 5].copy_from_slice(&x);
        want[AT + 5] = 0x90;
        assert_eq!(out, want);
    }
}

#[test]
fn g5_g6_binops_and_test_become_immediates_in_static_only() {
    let t = tables(TargetKind::Local, false, S);
    let p = AT + 3;
    let imm = (S as u32).to_le_bytes();
    for rex in REXES {
        for reg in 0..8u8 {
            let modrm = (reg << 3) | 5;
            for op in BINOPS {
                let input = section(&[rex, op, modrm, 0, 0, 0, 0]);
                let out = applied(&t, &input, OutputKind::Static, &[rel(p, SYM, 42, -4)]);
                let b = [rex_prime(rex), 0x81, 0xc0 | (op & 0x38) | reg];
                let want = with(&with(&input, AT, &b), p, &imm);
                assert_eq!(out, want, "G5 op {op:#x} rex {rex:#x} reg {reg}");
                // X-9: never in static-pie.
                let out = applied(&t, &input, OutputKind::Pie, &[rel(p, SYM, 42, -4)]);
                assert_eq!(out, with(&input, p, &pc(SLOT, p)), "G5 static-pie");
            }
            let input = section(&[rex, 0x85, modrm, 0, 0, 0, 0]);
            let out = applied(&t, &input, OutputKind::Static, &[rel(p, SYM, 42, -4)]);
            let want = with(&with(&input, AT, &[rex_prime(rex), 0xf7, 0xc0 | reg]), p, &imm);
            assert_eq!(out, want, "G6 rex {rex:#x} reg {reg}");
        }
    }
}

#[test]
fn g5_needs_s_below_2_31() {
    let p = AT + 3;
    let input = section(&[0x48, 0x03, 0x05, 0, 0, 0, 0]);
    for s in [1u64 << 31, 0x1_0000_0000] {
        let t = tables(TargetKind::Local, false, s);
        let out = applied(&t, &input, OutputKind::Static, &[rel(p, SYM, 42, -4)]);
        assert_eq!(out, with(&input, p, &pc(SLOT, p)), "S = {s:#x}");
    }
    let t = tables(TargetKind::Local, false, (1 << 31) - 1);
    let out = applied(&t, &input, OutputKind::Static, &[rel(p, SYM, 42, -4)]);
    assert_eq!(out[AT..AT + 3], [0x48, 0x81, 0xc0]);
}

#[test]
fn x19_x22_conditions_keep_the_got() {
    let p = AT + 3;
    let input = section(&[0x48, 0x8b, 0x05, 0, 0, 0, 0]);
    let unrelaxed = with(&input, p, &pc(SLOT, p));
    for kind in [TargetKind::Abs, TargetKind::Ifunc, TargetKind::Weak0] {
        let t = tables(kind, false, S);
        assert_eq!(applied(&t, &input, OutputKind::Static, &[rel(p, SYM, 42, -4)]), unrelaxed);
    }
    // A ≠ −4.
    let t = tables(TargetKind::Local, false, S);
    let out = applied(&t, &input, OutputKind::Static, &[rel(p, SYM, 42, -5)]);
    let x = (SLOT.wrapping_sub(5).wrapping_sub(ADDR + p as u64) as i32).to_le_bytes();
    assert_eq!(out, with(&input, p, &x));
    // `S + A − P` outside s(32).
    let t = tables(TargetKind::Local, false, ADDR + (1 << 32));
    assert_eq!(applied(&t, &input, OutputKind::Static, &[rel(p, SYM, 42, -4)]), unrelaxed);
    // The scan demand is the same in every case (kind 2).
    let s = scan(&t, OutputKind::Static, &input, &[rel(p, SYM, 42, -4)]);
    assert_eq!(raised(&s, key(SYM)), DemandFlags::GOT);
}

#[test]
fn x21_x31_x32_unmatched_bytes_keep_the_got() {
    let t = tables(TargetKind::Local, false, S);
    let cases: [(&[u8], u32); 6] = [
        // X-31: plain `GOTPCREL` is never relaxed.
        (&[0x48, 0x8b, 0x05], 9),
        // X-32: a 32-bit `GOTPCRELX` binop.
        (&[0x00, 0x03, 0x05], 41),
        // `pushq` `ff 35`.
        (&[0x00, 0xff, 0x35], 41),
        // SIB form, not RIP-relative.
        (&[0x48, 0x8b, 0x04], 42),
        // REX.X / REX.B set: not `48|4c`.
        (&[0x49, 0x8b, 0x05], 42),
        // `mov` with a REX that is not REX.W.
        (&[0x40, 0x8b, 0x05], 42),
    ];
    for (bytes, r_type) in cases {
        let mut form = bytes.to_vec();
        form.extend([0; 4]);
        let input = section(&form);
        let p = AT + 3;
        let out = applied(&t, &input, OutputKind::Static, &[rel(p, SYM, r_type, -4)]);
        assert_eq!(out, with(&input, p, &pc(SLOT, p)), "{bytes:x?} type {r_type}");
        let s = scan(&t, OutputKind::Static, &input, &[rel(p, SYM, r_type, -4)]);
        assert!(s.errors.is_empty(), "X-21: never refused");
    }
}

#[test]
fn x32_no_relaxation_in_a_non_alloc_section() {
    let t = tables(TargetKind::Local, false, S);
    let input = section(&[0x48, 0x8b, 0x05, 0, 0, 0, 0]);
    let p = AT + 3;
    let mut c = case(&t, &input, OutputKind::Static);
    c.alloc = false;
    c.name = b".note.x";
    let out = c.apply(&[rel(p, SYM, 42, -4)]).unwrap();
    assert_eq!(out, with(&input, p, &pc(SLOT, p)));
}

// ---------------------------------------------------------------- §8.2 I1/I2

#[test]
fn i1_i2_every_register() {
    let t = tables(TargetKind::Local, true, TLS_S);
    let p = AT + 3;
    for output in EXEC {
        for rex in REXES {
            for reg in 0..8u8 {
                let modrm = (reg << 3) | 5;
                for (op, new) in [(0x8b, 0xc7), (0x03, 0x81)] {
                    let input = section(&[rex, op, modrm, 0, 0, 0, 0]);
                    let relocs = [rel(p, SYM, 22, -4)];
                    let out = applied(&t, &input, output, &relocs);
                    let want = with(
                        &input,
                        AT,
                        &[rex_prime(rex), new, 0xc0 | reg, T[0], T[1], T[2], T[3]],
                    );
                    assert_eq!(out, want, "{output:?} op {op:#x} rex {rex:#x} reg {reg}");
                    // Kind 1: no `GTPREL` slot.
                    let s = scan(&t, output, &input, &relocs);
                    assert!(s.raised.is_empty() && s.errors.is_empty(), "{s:?}");
                }
            }
        }
    }
}

#[test]
fn x18_ie_against_thread_local_weak0_writes_0() {
    let t = tables(TargetKind::Weak0, true, 0);
    let input = section(&[0x4c, 0x03, 0x25, 0, 0, 0, 0]);
    let mut c = case(&t, &input, OutputKind::Static);
    c.tls = None;
    let out = c.apply(&[rel(AT + 3, SYM, 22, -4)]).unwrap();
    assert_eq!(out, with(&input, AT, &[0x49, 0x81, 0xc4, 0, 0, 0, 0]));
}

#[test]
fn x24_other_gottpoff_keeps_ie() {
    let t = tables(TargetKind::Local, true, TLS_S);
    // `movq x@gottpoff(%rip), %xmm0` (`66 48 0f 7e 05`) and `subq`.
    for (form, p) in [(&[0x66, 0x48, 0x0f, 0x7e, 0x05][..], AT + 5), (&[0x48, 0x2b, 0x05], AT + 3)]
    {
        let mut b = form.to_vec();
        b.extend([0; 4]);
        let input = section(&b);
        let relocs = [rel(p, SYM, 22, -4)];
        let out = applied(&t, &input, OutputKind::Static, &relocs);
        assert_eq!(out, with(&input, p, &pc(SLOT + 8, p)));
        let s = scan(&t, OutputKind::Static, &input, &relocs);
        assert_eq!(raised(&s, key(SYM)), DemandFlags::GOT_TPREL);
    }
}

// ---------------------------------------------------------------- §8.3, §8.4

const D16: [u8; 12] = [0x64, 0x48, 0x8b, 0x04, 0x25, 0, 0, 0, 0, 0x48, 0x8d, 0x80];

fn gd16(x: bool) -> Vec<u8> {
    let mut b = vec![0x66, 0x48, 0x8d, 0x3d, 0, 0, 0, 0];
    b.extend(if x { [0x66, 0x48, 0xff, 0x15] } else { [0x66, 0x66, 0x48, 0xe8] });
    b.extend([0; 4]);
    b
}

fn large(rex: u8, r: u8) -> Vec<u8> {
    let mut b = vec![0x48, 0x8d, 0x3d, 0, 0, 0, 0, 0x48, 0xb8];
    b.extend([0; 8]);
    b.extend([rex, 0x01, 0xc0 | (r << 3), 0xff, 0xd0]);
    b
}

/// The sequences of §8.3 and §8.4: (name, input form, lead type, `P`
/// offset in the form, follower type, follower offset from `P`, output
/// from the form's start).
#[allow(clippy::type_complexity)]
fn sequences() -> Vec<(&'static str, Vec<u8>, u32, usize, u32, usize, Vec<u8>)> {
    let mut d16 = D16.to_vec();
    d16.extend(T);
    let mut d22 = d16.clone();
    d22.extend([0x66, 0x0f, 0x1f, 0x44, 0x00, 0x00]);
    let ld12 = vec![0x66, 0x66, 0x66, 0x64, 0x48, 0x8b, 0x04, 0x25, 0, 0, 0, 0];
    let ld13 = vec![0x66, 0x66, 0x66, 0x66, 0x64, 0x48, 0x8b, 0x04, 0x25, 0, 0, 0, 0];
    let ld22 = vec![
        0x66, 0x66, 0x66, 0x66, 0x2e, 0x0f, 0x1f, 0x84, 0, 0, 0, 0, 0, 0x64, 0x48, 0x8b, 0x04,
        0x25, 0, 0, 0, 0,
    ];
    let mut ld12_in = vec![0x48, 0x8d, 0x3d, 0, 0, 0, 0, 0xe8];
    ld12_in.extend([0; 4]);
    let mut ld13_in = vec![0x48, 0x8d, 0x3d, 0, 0, 0, 0, 0xff, 0x15];
    ld13_in.extend([0; 4]);
    let mut v = vec![];
    for f in [4, 2] {
        v.push(("GD16", gd16(false), 19, 4, f, 8, d16.clone()));
        v.push(("LD12", ld12_in.clone(), 20, 3, f, 5, ld12.clone()));
    }
    for f in [41, 9] {
        v.push(("GD16x", gd16(true), 19, 4, f, 8, d16.clone()));
        v.push(("LD13", ld13_in.clone(), 20, 3, f, 6, ld13.clone()));
    }
    for (rex, r) in [(0x48, 0), (0x48, 7), (0x4c, 7), (0x4c, 3)] {
        v.push(("GD22", large(rex, r), 19, 3, 31, 6, d22.clone()));
        v.push(("LD22", large(rex, r), 20, 3, 31, 6, ld22.clone()));
    }
    v
}

#[test]
fn x25_x27_sequences_relax_to_le() {
    let t = tables(TargetKind::Local, true, TLS_S);
    for output in EXEC {
        for (name, form, lead, p, follower, delta, le) in sequences() {
            let input = section(&form);
            let p = AT + p;
            let relocs = [rel(p, SYM, lead, -4), rel(p + delta, GET_ADDR, follower, -4)];
            let want = with(&input, AT, &le);
            assert_eq!(applied(&t, &input, output, &relocs), want, "{name} {follower}");
            // Each record alone: the lead writes everything, the follower
            // nothing (X-25), so the order does not matter.
            let c = case(&t, &input, output);
            assert_eq!(c.apply_at(&relocs, 0).unwrap(), want, "{name}: the lead alone");
            assert_eq!(c.apply_at(&relocs, 1).unwrap(), input, "{name}: the follower alone");
            // Kind 1: no demand, no undefined `__tls_get_addr`.
            let s = scan(&t, output, &input, &relocs);
            assert!(s.raised.is_empty() && s.errors.is_empty(), "{name}: {s:?}");
            assert!(s.undefined.is_empty() && s.events.is_empty(), "{name}: {s:?}");
            assert_eq!(X64::M1.sequence_extent(&relocs.as_slice(), 0), 2, "{name}");
        }
    }
}

#[test]
fn x18_gd_against_thread_local_weak0_writes_t_0() {
    let t = tables(TargetKind::Weak0, true, 0);
    let input = section(&gd16(false));
    let relocs = [rel(AT + 4, SYM, 19, -4), rel(AT + 12, GET_ADDR, 4, -4)];
    let mut c = case(&t, &input, OutputKind::Static);
    c.tls = None;
    let mut le = D16.to_vec();
    le.extend([0; 4]);
    assert_eq!(c.apply(&relocs).unwrap(), with(&input, AT, &le));
}

#[test]
fn x26_unmatched_sequences_are_refused_by_name() {
    let t = tables(TargetKind::Local, true, TLS_S);
    let mut other = tables(TargetKind::Local, true, TLS_S);
    other.names.insert(key(GET_ADDR), b"not_tls_get_addr".to_vec());
    for (name, form, lead, p, follower, delta, _) in sequences() {
        let input = section(&form);
        let p = AT + p;
        let want = if lead == 19 { UNRECOGNIZED_GD } else { UNRECOGNIZED_LD };
        let wrong_type = if follower == 31 { 4 } else { 31 };
        let broken: [(&str, &Tables, Vec<u8>, [Reloc; 2]); 5] = [
            (
                "A",
                &t,
                input.clone(),
                [rel(p, SYM, lead, -5), rel(p + delta, GET_ADDR, follower, -4)],
            ),
            (
                "offset",
                &t,
                input.clone(),
                [rel(p, SYM, lead, -4), rel(p + delta + 1, GET_ADDR, follower, -4)],
            ),
            (
                "type",
                &t,
                input.clone(),
                [rel(p, SYM, lead, -4), rel(p + delta, GET_ADDR, wrong_type, -4)],
            ),
            (
                "name",
                &other,
                input.clone(),
                [rel(p, SYM, lead, -4), rel(p + delta, GET_ADDR, follower, -4)],
            ),
            (
                "bytes",
                &t,
                with(&input, p - 1, &[0x35]),
                [rel(p, SYM, lead, -4), rel(p + delta, GET_ADDR, follower, -4)],
            ),
        ];
        for (what, tb, input, relocs) in broken {
            let s = scan(tb, OutputKind::Static, &input, &relocs);
            let lead_err = s
                .errors
                .iter()
                .find(|(i, _)| *i == 0)
                .unwrap_or_else(|| panic!("{name} {what}: {s:?}"));
            assert_eq!(refused_name(&lead_err.1), want, "{name} {what}");
            let c = case(tb, &input, OutputKind::Static);
            let Err(FieldError::Refused(r)) = c.apply_at(&relocs, 0) else {
                panic!("{name} {what}")
            };
            assert_eq!(r.name, RefName::Named(want));
            // The follower is not consumed: it is an ordinary reference.
            assert!(
                !X64::M1.consumed(OutputKind::Static, &at(&relocs, 1, &input), tb),
                "{name} {what}"
            );
        }
    }
}

#[test]
fn x32_gd_to_ie_is_refused() {
    // A TLS target defined in a shared object (M2 `Exec`): GD → IE.
    let t = tables(TargetKind::Shlib { func: false }, true, TLS_S);
    let input = section(&gd16(false));
    let relocs = [rel(AT + 4, SYM, 19, -4), rel(AT + 12, GET_ADDR, 4, -4)];
    let s = scan(&t, OutputKind::Exec, &input, &relocs);
    assert_eq!(refused_name(&s.errors[0].1), TLS_TO_IE, "{s:?}");
}

// ---------------------------------------------------------------- §8.5

#[test]
fn x28_tlsdesc_every_register() {
    let t = tables(TargetKind::Local, true, TLS_S);
    for output in EXEC {
        for rex in REXES {
            for reg in 0..8u8 {
                let input = section(&[rex, 0x8d, (reg << 3) | 5, 0, 0, 0, 0, 0xff, 0x10]);
                let relocs = [rel(AT + 3, SYM, 34, -4), rel(AT + 7, SYM, 35, 0)];
                let mut want = with(&input, AT, &[rex_prime(rex), 0xc7, 0xc0 | reg]);
                want[AT + 3..AT + 7].copy_from_slice(&T);
                want[AT + 7..AT + 9].copy_from_slice(&[0x66, 0x90]);
                assert_eq!(applied(&t, &input, output, &relocs), want, "rex {rex:#x} reg {reg}");
                let s = scan(&t, output, &input, &relocs);
                assert!(s.raised.is_empty() && s.errors.is_empty(), "{s:?}");
            }
        }
    }
    // The E2 destinations: `%rax`, `%rbp`, `%r12`.
    for (input, out) in [
        ([0x48, 0x8d, 0x05], [0x48, 0xc7, 0xc0]),
        ([0x48, 0x8d, 0x2d], [0x48, 0xc7, 0xc5]),
        ([0x4c, 0x8d, 0x25], [0x49, 0xc7, 0xc4]),
    ] {
        let mut b = input.to_vec();
        b.extend([0; 4]);
        let input = section(&b);
        let got = applied(&t, &input, OutputKind::Static, &[rel(AT + 3, SYM, 34, -4)]);
        assert_eq!(got[AT..AT + 3], out);
    }
}

#[test]
fn x47_tls_module_base_is_t_0() {
    let mut t = tables(TargetKind::Local, true, TLS_S);
    t.names.insert(key(SYM), b"_TLS_MODULE_BASE_".to_vec());
    let input = section(&[0x48, 0x8d, 0x05, 0, 0, 0, 0]);
    let out = applied(&t, &input, OutputKind::Static, &[rel(AT + 3, SYM, 34, -4)]);
    assert_eq!(out, with(&input, AT, &[0x48, 0xc7, 0xc0, 0, 0, 0, 0]));
}

#[test]
fn x28_unmatched_descriptor_members_are_refused() {
    let t = tables(TargetKind::Local, true, TLS_S);
    let cases: [(&[u8], u32, usize, i64); 4] = [
        (&[0x48, 0x8b, 0x05, 0, 0, 0, 0], 34, 3, -4),
        (&[0x48, 0x8d, 0x05, 0, 0, 0, 0], 34, 3, -5),
        (&[0xff, 0x15], 35, 0, 0),
        (&[0xff, 0x11], 35, 0, 0),
    ];
    for (form, r_type, p, a) in cases {
        let input = section(form);
        let relocs = [rel(AT + p, SYM, r_type, a)];
        let s = scan(&t, OutputKind::Static, &input, &relocs);
        assert_eq!(refused_name(&s.errors[0].1), UNRECOGNIZED_DESC, "{form:x?}");
        let Err(FieldError::Refused(r)) = case(&t, &input, OutputKind::Static).apply(&relocs)
        else {
            panic!()
        };
        assert_eq!(r.name, RefName::Named(UNRECOGNIZED_DESC));
    }
}

// ---------------------------------------------------------------- §8.6 X-8, X-30

fn at<'a>(relocs: &'a [Reloc], i: usize, input: &'a [u8]) -> RelocAt<'a> {
    let r = relocs[i];
    let h = X64::M1.classify(r.r_type, Site { bytes: input, offset: r.r_offset }).unwrap();
    RelocAt {
        file: FileId(1),
        section: SectionId(3),
        index: u32::try_from(i).unwrap(),
        r_offset: r.r_offset,
        key: r.key,
        r_type: r.r_type,
        addend: Some(r.addend),
        howto: Box::leak(Box::new(h)),
        site: Site { bytes: input, offset: r.r_offset },
        records: Box::leak(Box::new(relocs)),
        proof: Box::leak(all_code(input.len()).into_boxed_slice()),
        leads: true,
    }
}

#[test]
fn x30_sequence_extent() {
    let x = X64::M1;
    let r = |o: u64, t: u32| Reloc { r_offset: o, key: key(1), r_type: t, addend: -4 };
    let ext = |rs: &[Reloc], i| x.sequence_extent(&rs, i);
    assert_eq!(ext(&[r(4, 19), r(12, 4)], 0), 2);
    assert_eq!(ext(&[r(4, 19), r(12, 2)], 0), 2);
    assert_eq!(ext(&[r(4, 19), r(12, 41)], 0), 2);
    assert_eq!(ext(&[r(4, 19), r(12, 9)], 0), 2);
    assert_eq!(ext(&[r(3, 19), r(9, 31)], 0), 2);
    assert_eq!(ext(&[r(3, 20), r(8, 4)], 0), 2);
    assert_eq!(ext(&[r(3, 20), r(8, 2)], 0), 2);
    assert_eq!(ext(&[r(3, 20), r(9, 41)], 0), 2);
    assert_eq!(ext(&[r(3, 20), r(9, 31)], 0), 2);
    // Wrong offset, wrong type, no follower, a follower type as a lead.
    assert_eq!(ext(&[r(4, 19), r(13, 4)], 0), 1);
    assert_eq!(ext(&[r(3, 20), r(9, 4)], 0), 1);
    assert_eq!(ext(&[r(4, 19), r(12, 42)], 0), 1);
    assert_eq!(ext(&[r(4, 19)], 0), 1);
    assert_eq!(ext(&[r(4, 4), r(12, 4)], 0), 1);
    assert_eq!(ext(&[r(4, 19), r(12, 4)], 1), 1);
    assert_eq!(ext(&[r(4, 19), r(12, 4)], 7), 1);
}

#[test]
fn x29_unit_windows() {
    let t = tables(TargetKind::Local, true, TLS_S);
    let x = X64::M1;
    let input = section(&large(0x4c, 7));
    let p = (AT + 3) as u64;
    let relocs = [rel(AT + 3, SYM, 19, -4), rel(AT + 9, GET_ADDR, 31, -4)];
    let lead = at(&relocs, 0, &input);
    let follower = at(&relocs, 1, &input);
    assert_eq!(lead.howto.window, Window { before: 3, after: 15 });
    assert_eq!(
        x.unit_window(OutputKind::Static, &lead, &t),
        Some(UnitWindow { start: p - 3, end: p + 19, unit: 0, record: 0 })
    );
    assert_eq!(
        x.unit_window(OutputKind::Static, &follower, &t),
        Some(UnitWindow { start: p + 6, end: p + 6, unit: 0, record: 1 })
    );
    assert!(x.consumed(OutputKind::Static, &follower, &t));
    assert!(!x.consumed(OutputKind::Static, &lead, &t));
    // Not an executable: nothing is consumed; the lead owns its field.
    assert!(!x.consumed(OutputKind::Shared, &follower, &t));
    assert_eq!(
        x.unit_window(OutputKind::Shared, &lead, &t),
        Some(UnitWindow { start: p, end: p + 4, unit: 0, record: 0 })
    );
    // An unmatched lead owns its field (X-26 refuses it).
    let bad = [rel(AT + 3, SYM, 19, -5), rel(AT + 9, GET_ADDR, 31, -4)];
    assert_eq!(
        x.unit_window(OutputKind::Static, &at(&bad, 0, &input), &t),
        Some(UnitWindow { start: p, end: p + 4, unit: 0, record: 0 })
    );
}

#[test]
fn x8_windows_outside_the_section_do_not_match() {
    let t = tables(TargetKind::Local, false, S);
    let x = X64::M1;
    // `REX_GOTPCRELX` with its field at offset 0, 1, 2: the REX would be
    // before the section; applied unrelaxed, never decoded.
    for p in 0..3usize {
        let mut input = vec![0x48, 0x8b, 0x05, 0, 0, 0, 0];
        input.drain(..3 - p);
        let h = x.classify(42, Site { bytes: &input, offset: p as u64 }).unwrap();
        assert_eq!(h.window, Window { before: 0, after: 0 }, "P = {p}");
        let out = case(&t, &input, OutputKind::Static).apply(&[rel(p, SYM, 42, -4)]).unwrap();
        let mut want = input.clone();
        want[p..p + 4].copy_from_slice(&pc(SLOT, p));
        assert_eq!(out, want, "P = {p}");
    }
    // GD16 with its `66` one byte before the section: refused (X-26).
    let tt = tables(TargetKind::Local, true, TLS_S);
    let mut input = gd16(false);
    input.remove(0);
    let relocs = [rel(3, SYM, 19, -4), rel(11, GET_ADDR, 4, -4)];
    let s = scan(&tt, OutputKind::Static, &input, &relocs);
    assert_eq!(refused_name(&s.errors[0].1), UNRECOGNIZED_GD);
    // GD22 cut at the section end.
    let mut input = large(0x48, 0);
    input.pop();
    let relocs = [rel(3, SYM, 19, -4), rel(9, GET_ADDR, 31, -4)];
    let s = scan(&tt, OutputKind::Static, &input, &relocs);
    assert_eq!(refused_name(&s.errors[0].1), UNRECOGNIZED_GD);
    // The full form at the section's start and end matches.
    let input = large(0x48, 0);
    let s = scan(&tt, OutputKind::Static, &input, &relocs);
    assert!(s.errors.is_empty(), "{s:?}");
}

#[test]
fn followers_are_consumed_once() {
    // Two GD16 sequences back to back: each follower belongs to its own
    // lead; a PLT32 after a follower is not taken as another follower.
    let t = tables(TargetKind::Local, true, TLS_S);
    let mut form = gd16(false);
    form.extend(gd16(false));
    let input = section(&form);
    let relocs = [
        rel(AT + 4, SYM, 19, -4),
        rel(AT + 12, GET_ADDR, 4, -4),
        rel(AT + 20, SYM, 19, -4),
        rel(AT + 28, GET_ADDR, 4, -4),
    ];
    let mut le = D16.to_vec();
    le.extend(T);
    let want = with(&with(&input, AT, &le), AT + 16, &le);
    assert_eq!(applied(&t, &input, OutputKind::Static, &relocs), want);
    let x = X64::M1;
    let consumed: Vec<bool> =
        (0..4).map(|i| x.consumed(OutputKind::Static, &at(&relocs, i, &input), &t)).collect();
    assert_eq!(consumed, [false, true, false, true]);
    let s = scan(&t, OutputKind::Static, &input, &relocs);
    assert!(s.errors.is_empty() && s.events.is_empty(), "{s:?}");
    // `leads = false` (the lead is in another unit's record set): the
    // follower is not consumed.
    let mut f = at(&relocs, 1, &input);
    f.leads = false;
    assert!(relax::follower_of(&f, &t).is_none());
}

#[test]
fn consumed_follower_against_undefined_is_not_an_error() {
    let t = tables(TargetKind::Local, true, TLS_S);
    let input = section(&gd16(false));
    let relocs = [rel(AT + 4, SYM, 19, -4), rel(AT + 12, GET_ADDR, 4, -4)];
    let s = scan(&t, OutputKind::Static, &input, &relocs);
    assert!(!s.events.iter().any(|(_, c, _)| *c == EventClass::Undefined));
}

#[test]
fn shown_bytes_are_the_reach_clipped() {
    let input: Vec<u8> = (0u8..40).collect();
    assert_eq!(
        shown_bytes(&input, 2),
        "00 01 02 03 04 05 06 07 08 09 0a 0b 0c 0d 0e 0f 10 11 12 13 14"
    );
    assert_eq!(shown_bytes(&input, 36), "20 21 22 23 24 25 26 27");
}

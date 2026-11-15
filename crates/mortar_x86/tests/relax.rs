//! The relaxations of x86.md §8 and §9, byte for byte, every register:
//! the expected bytes are built here from the spec's tables and the whole
//! section is compared, so a byte written outside the form's output is a
//! failure (X86-9, X86-40). The addends are in the input bytes (X86-4).
#![allow(clippy::many_single_char_names)]

mod support;

use mortar_arch::{
    Arch, DemandFlags, EventClass, FieldError, OutputKind, PlaceFacts, RefName, Reloc, RelocAt,
    ScanError, Site, SlotKind, TargetKind, TlsBlock, UnitWindow, Window,
};
use mortar_core::{FileId, SectionId};
use mortar_x86::forms::BINOPS;
use mortar_x86::{
    IE_TEXT_RELOCATION, TLS_TO_IE, UNRECOGNIZED_DESC, UNRECOGNIZED_GD, UNRECOGNIZED_LD, X86, relax,
    shown_bytes,
};
use support::{Case, Sink, Tables, all_code, key, raised, scan_all};

/// Section address; `LEN` bytes of `int3` with the form at `AT`.
const ADDR: u64 = 0x0804_9000;
const LEN: usize = 48;
const AT: usize = 16;
const TEXT: PlaceFacts = PlaceFacts { alloc: true, writable: false };
const GOT: u64 = 0x0804_c000;
const TLS: TlsBlock = TlsBlock { start: 0x0804_b000, size: 0x10, align: 4 };
/// The TLS target and `T = TpRel(S) = −8` (variant II, TP at the end).
const TLS_S: u64 = 0x0804_b008;
const T: [u8; 4] = (-8i32).to_le_bytes();
const NEG_T: [u8; 4] = 8i32.to_le_bytes();
/// A non-TLS target and its GOT slot.
const S: u64 = 0x0804_a468;
const SLOT: u64 = 0x0804_c00c;

const SYM: u32 = 1;
const GET_ADDR: u32 = 2;

const GOT32: u32 = 3;
const PLT32: u32 = 4;
const PC32: u32 = 2;
const TLS_IE: u32 = 15;
const TLS_GOTIE: u32 = 16;
const TLS_GD: u32 = 18;
const TLS_LDM: u32 = 19;
const TLS_GOTDESC: u32 = 39;
const TLS_DESC_CALL: u32 = 40;
const GOT32X: u32 = 43;

const EXEC: [OutputKind; 2] = [OutputKind::Static, OutputKind::Pie];

fn tables(kind: TargetKind, tls: bool, s: u64) -> Tables {
    let mut t = Tables::default();
    if tls {
        t.tls_sym(key(SYM), s, kind);
    } else {
        t.sym(key(SYM), s, kind);
    }
    t.slots.insert((key(SYM), SlotKind::Addr), SLOT);
    t.slots.insert((key(SYM), SlotKind::TpOff { negated: false }), SLOT + 4);
    t.sym(key(GET_ADDR), 0, TargetKind::Undefined);
    t.names.insert(key(GET_ADDR), b"___tls_get_addr".to_vec());
    t
}

/// `LEN` bytes of `cc` with `form` at `AT`.
fn section(form: &[u8]) -> Vec<u8> {
    let mut b = vec![0xcc; LEN];
    b[AT..AT + form.len()].copy_from_slice(form);
    b
}

fn rel(off: usize, k: u32, r_type: u32) -> Reloc {
    Reloc { r_offset: off as u64, key: key(k), r_type, addend: 0 }
}

fn case<'a>(t: &'a Tables, input: &'a [u8], output: OutputKind) -> Case<'a> {
    let mut c = Case::new(t, input, ADDR);
    c.output = output;
    c.tls = Some(TLS);
    c.got_base = Some(GOT);
    c
}

fn applied(t: &Tables, input: &[u8], output: OutputKind, relocs: &[Reloc]) -> Vec<u8> {
    case(t, input, output).apply(relocs).unwrap_or_else(|e| panic!("{output:?}: {e:?}"))
}

/// `input` with `bytes` at `off`.
fn with(input: &[u8], off: usize, bytes: &[u8]) -> Vec<u8> {
    let mut b = input.to_vec();
    b[off..off + bytes.len()].copy_from_slice(bytes);
    b
}

fn scan(t: &Tables, output: OutputKind, input: &[u8], relocs: &[Reloc]) -> Sink {
    let mut sink = Sink::default();
    scan_all(X86::M1, t, output, TEXT, input, relocs, &mut sink).unwrap();
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

fn le(x: u64) -> [u8; 4] {
    (x as u32).to_le_bytes()
}

/// `x − (ADDR + end)` mod 2^32, a rel32 whose instruction ends at `end`.
fn rel32(x: u64, end: usize) -> [u8; 4] {
    le(x.wrapping_sub(ADDR + end as u64))
}

/// A form with a `ModRM` byte: base (`10 reg 011`, `%ebx`) or no base
/// (`00 reg 101`).
const fn modrm(reg: u8, base: bool) -> u8 {
    if base { 0x80 | (reg << 3) | 3 } else { (reg << 3) | 5 }
}

// ---------------------------------------------------------------- §8 G1–G5

/// The G1–G3 outputs of `op ModRM` for every register, base and no base.
fn g123() -> Vec<(u8, u8, u8, u8)> {
    // (input op, reg, output op, output ModRM).
    let mut v = Vec::new();
    for reg in 0..8u8 {
        v.push((0x8b, reg, 0xc7, 0xc0 | reg));
        v.push((0x85, reg, 0xf7, 0xc0 | reg));
        for op in BINOPS {
            v.push((op, reg, 0x81, 0xc0 | (op & 0x38) | reg));
        }
    }
    v
}

#[test]
fn g1_g2_g3_become_immediates_in_static() {
    for kind in [TargetKind::Local, TargetKind::Abs, TargetKind::Weak0] {
        let s = if kind == TargetKind::Abs { 0x1234 } else { S };
        let t = tables(kind, false, s);
        let imm = if kind == TargetKind::Weak0 { 0 } else { s };
        for (op, reg, op2, m2) in g123() {
            for base in [true, false] {
                let input = section(&[op, modrm(reg, base), 0, 0, 0, 0]);
                let p = AT + 2;
                let out = applied(&t, &input, OutputKind::Static, &[rel(p, SYM, GOT32X)]);
                let mut want = with(&input, AT, &[op2, m2]);
                want[p..p + 4].copy_from_slice(&le(imm));
                assert_eq!(out, want, "{kind:?} {op:#x} reg {reg} base {base}");
            }
        }
    }
}

#[test]
fn g4_g5_call_and_jmp_become_direct() {
    for kind in [TargetKind::Local, TargetKind::Abs] {
        let t = tables(kind, false, S);
        let p = AT + 2;
        for base in [true, false] {
            // `call *foo@GOT(%ebx)` → `addr32 call foo`.
            let input = section(&[0xff, modrm(2, base), 0, 0, 0, 0]);
            let out = applied(&t, &input, OutputKind::Static, &[rel(p, SYM, GOT32X)]);
            let mut want = with(&input, AT, &[0x67, 0xe8]);
            want[p..p + 4].copy_from_slice(&rel32(S, p + 4));
            assert_eq!(out, want, "G4 base {base}");
            // `jmp *foo@GOT(%ebx)` → `jmp foo; nop`.
            let input = section(&[0xff, modrm(4, base), 0, 0, 0, 0]);
            let out = applied(&t, &input, OutputKind::Static, &[rel(p, SYM, GOT32X)]);
            let mut want = with(&input, AT, &[0xe9]);
            want[AT + 1..AT + 5].copy_from_slice(&rel32(S, AT + 5));
            want[AT + 5] = 0x90;
            assert_eq!(out, want, "G5 base {base}");
        }
    }
}

#[test]
fn x86_25_conditions_keep_the_got_load() {
    let p = AT + 2;
    let unrelaxed = |a: i64, base: bool| {
        let x = SLOT as i64 + a - if base { GOT as i64 } else { 0 };
        le(x as u64)
    };
    // (what, kind, output, type, A, ModRM reg, op).
    let cases: [(&str, TargetKind, OutputKind, u32, i64, u8, u8); 7] = [
        ("A ≠ 0", TargetKind::Local, OutputKind::Static, GOT32X, 4, 0, 0x8b),
        ("static-pie", TargetKind::Local, OutputKind::Pie, GOT32X, 0, 0, 0x8b),
        ("GOT32", TargetKind::Local, OutputKind::Static, GOT32, 0, 0, 0x8b),
        ("IFUNC", TargetKind::Ifunc, OutputKind::Static, GOT32X, 0, 0, 0x8b),
        ("weak0 call", TargetKind::Weak0, OutputKind::Static, GOT32X, 0, 2, 0xff),
        ("weak0 jmp", TargetKind::Weak0, OutputKind::Static, GOT32X, 0, 4, 0xff),
        ("lea", TargetKind::Local, OutputKind::Static, GOT32X, 0, 0, 0x8d),
    ];
    for (what, kind, output, r_type, a, reg, op) in cases {
        let t = tables(kind, false, S);
        let mut input = section(&[op, modrm(reg, true), 0, 0, 0, 0]);
        input[p..p + 4].copy_from_slice(&(a as i32).to_le_bytes());
        let out = applied(&t, &input, output, &[rel(p, SYM, r_type)]);
        assert_eq!(out, with(&input, p, &unrelaxed(a, true)), "{what}");
        let s = scan(&t, output, &input, &[rel(p, SYM, r_type)]);
        assert!(raised(&s, key(SYM)).contains(DemandFlags::GOT), "{what}");
    }
    // No base, unrelaxed, static: the absolute slot address (X86-27).
    let t = tables(TargetKind::Local, false, S);
    let input = section(&[0xff, modrm(2, false), 0, 0, 0, 0]);
    let out = applied(&t, &input, OutputKind::Static, &[rel(p, SYM, GOT32)]);
    assert_eq!(out, with(&input, p, &le(SLOT)));
}

#[test]
fn x86_25_no_relaxation_in_a_non_alloc_section() {
    // A GOT form at a non-alloc place is E at scan (X86-59); fill never
    // rewrites an instruction there.
    let t = tables(TargetKind::Local, false, S);
    let input = section(&[0x8b, modrm(0, true), 0, 0, 0, 0]);
    let mut sink = Sink::default();
    let debug = PlaceFacts { alloc: false, writable: false };
    scan_all(X86::M1, &t, OutputKind::Static, debug, &input, &[rel(AT + 2, SYM, GOT32X)], &mut sink)
        .unwrap();
    assert_eq!(sink.errors.len(), 1, "{sink:?}");
    let mut c = case(&t, &input, OutputKind::Static);
    c.alloc = false;
    c.name = b".debug_info";
    c.exec = Some(false);
    assert!(c.apply(&[rel(AT + 2, SYM, GOT32X)]).is_err());
}

// ---------------------------------------------------------------- §9 I1–I5

#[test]
fn i1_movl_moffs_becomes_movl_imm() {
    let t = tables(TargetKind::Local, true, TLS_S);
    for output in EXEC {
        // `movl foo@indntpoff, %eax` (`a1`).
        let input = section(&[0xa1, 0, 0, 0, 0]);
        let p = AT + 1;
        let out = applied(&t, &input, output, &[rel(p, SYM, TLS_IE)]);
        let mut want = with(&input, AT, &[0xb8]);
        want[p..p + 4].copy_from_slice(&T);
        assert_eq!(out, want, "{output:?}");
    }
}

#[test]
fn i2_to_i5_every_register() {
    let t = tables(TargetKind::Local, true, TLS_S);
    for output in EXEC {
        for reg in 0..8u8 {
            for (r_type, op, m, op2) in [
                (TLS_IE, 0x8b, modrm(reg, false), 0xc7),
                (TLS_IE, 0x03, modrm(reg, false), 0x81),
                (TLS_GOTIE, 0x8b, modrm(reg, true), 0xc7),
                (TLS_GOTIE, 0x03, modrm(reg, true), 0x81),
                // Another base register: `10 reg 110` (%esi).
                (TLS_GOTIE, 0x8b, 0x80 | (reg << 3) | 6, 0xc7),
            ] {
                let input = section(&[op, m, 0, 0, 0, 0]);
                let p = AT + 2;
                let out = applied(&t, &input, output, &[rel(p, SYM, r_type)]);
                let mut want = with(&input, AT, &[op2, 0xc0 | reg]);
                want[p..p + 4].copy_from_slice(&T);
                assert_eq!(out, want, "{output:?} {r_type} {op:#x} reg {reg}");
                let s = scan(&t, output, &input, &[rel(p, SYM, r_type)]);
                assert!(raised(&s, key(SYM)).is_empty(), "no slot: {s:?}");
            }
        }
    }
}

#[test]
fn x86_32_ie_against_thread_local_weak0_writes_0() {
    // §7: TpRel of an undefined weak TLS symbol is 0 (X86-23).
    let t = tables(TargetKind::Weak0, true, 0);
    let input = section(&[0x8b, modrm(1, true), 0, 0, 0, 0]);
    let out = applied(&t, &input, OutputKind::Static, &[rel(AT + 2, SYM, TLS_GOTIE)]);
    assert_eq!(out, with(&input, AT, &[0xc7, 0xc1, 0, 0, 0, 0]));
}

#[test]
fn x86_32_other_ie_bytes_keep_the_slot() {
    let t = tables(TargetKind::Local, true, TLS_S);
    let p = AT + 2;
    // `subl foo@gotntpoff(%ebx), %ecx` (`2b`): not a form.
    let input = section(&[0x2b, modrm(1, true), 0, 0, 0, 0]);
    let out = applied(&t, &input, OutputKind::Static, &[rel(p, SYM, TLS_GOTIE)]);
    assert_eq!(out, with(&input, p, &le((SLOT + 4).wrapping_sub(GOT))));
    let s = scan(&t, OutputKind::Static, &input, &[rel(p, SYM, TLS_GOTIE)]);
    assert!(raised(&s, key(SYM)).contains(DemandFlags::GOT_TPREL));
    // `A ≠ 0` keeps the slot too.
    let mut input = section(&[0x8b, modrm(1, true), 4, 0, 0, 0]);
    let out = applied(&t, &input, OutputKind::Static, &[rel(p, SYM, TLS_GOTIE)]);
    assert_eq!(out, with(&input, p, &le((SLOT + 8).wrapping_sub(GOT))));
    // An unrelaxed `TLS_IE` (`G + A`, absolute): static only (X86-18).
    input = section(&[0x8b, 0x84, 0x05, 0, 0, 0, 0]);
    let p = AT + 3;
    let out = applied(&t, &input, OutputKind::Static, &[rel(p, SYM, TLS_IE)]);
    assert_eq!(out, with(&input, p, &le(SLOT + 4)));
    let s = scan(&t, OutputKind::Pie, &input, &[rel(p, SYM, TLS_IE)]);
    assert!(!s.errors.is_empty(), "{s:?}");
    let c = case(&t, &input, OutputKind::Pie);
    let Err(FieldError::Refused(r)) = c.apply(&[rel(p, SYM, TLS_IE)]) else { panic!() };
    assert_eq!(r.name, RefName::Named(IE_TEXT_RELOCATION));
}

// ---------------------------------------------------------------- §9 D1–D3, L1–L2

/// `movl %gs:0, %eax; subl $(−T), %eax` (§9's GD output).
fn gd_le() -> Vec<u8> {
    let mut v = vec![0x65, 0xa1, 0, 0, 0, 0, 0x81, 0xe8];
    v.extend(NEG_T);
    v
}

/// (name, form bytes, lead type, P from `AT`, partner type, partner
/// offset from `P`, LE output from `AT`).
#[allow(clippy::type_complexity)]
fn sequences() -> Vec<(&'static str, Vec<u8>, u32, usize, u32, usize, Vec<u8>)> {
    let e8 = [0xe8, 0xfc, 0xff, 0xff, 0xff];
    let call_got = |rm: u8| [0xff, 0x90 | rm, 0, 0, 0, 0];
    let lea = |rm: u8| [0x8d, 0x80 | rm, 0, 0, 0, 0];
    let cat = |a: &[u8], b: &[u8]| [a, b].concat();
    let l1_le = vec![0x65, 0xa1, 0, 0, 0, 0, 0x90, 0x8d, 0x74, 0x26, 0x00];
    let l2_le = vec![0x65, 0xa1, 0, 0, 0, 0, 0x8d, 0xb6, 0, 0, 0, 0];
    let mut v = Vec::new();
    // D1: `leal x@tlsgd(,%ebx,1), %eax; call ___tls_get_addr@PLT`.
    for p_type in [PLT32, PC32] {
        v.push(("D1", cat(&[0x8d, 0x04, 0x1d, 0, 0, 0, 0], &e8), TLS_GD, 3, p_type, 5, gd_le()));
    }
    for rm in [3u8, 6, 7] {
        for p_type in [GOT32, GOT32X] {
            // D2: `leal x@tlsgd(%rm), %eax; call *___tls_get_addr@GOT(%rm')`.
            v.push(("D2", cat(&lea(rm), &call_got(rm)), TLS_GD, 2, p_type, 6, gd_le()));
            v.push(("L2", cat(&lea(rm), &call_got(rm)), TLS_LDM, 2, p_type, 6, l2_le.clone()));
        }
        for p_type in [PLT32, PC32] {
            // D3: `leal x@tlsgd(%rm), %eax; call ___tls_get_addr@PLT; nop`.
            let d3 = [&lea(rm)[..], &e8, &[0x90]].concat();
            v.push(("D3", d3, TLS_GD, 2, p_type, 5, gd_le()));
            v.push(("L1", cat(&lea(rm), &e8), TLS_LDM, 2, p_type, 5, l1_le.clone()));
        }
    }
    v
}

#[test]
fn x86_34_x86_35_sequences_relax_to_le() {
    let t = tables(TargetKind::Local, true, TLS_S);
    for output in EXEC {
        for (name, form, lead, p, partner, delta, le) in sequences() {
            let input = section(&form);
            let p = AT + p;
            let relocs = [rel(p, SYM, lead), rel(p + delta, GET_ADDR, partner)];
            let out = applied(&t, &input, output, &relocs);
            assert_eq!(out, with(&input, AT, &le), "{output:?} {name} {partner}");
            let s = scan(&t, output, &input, &relocs);
            assert!(s.errors.is_empty() && s.events.is_empty(), "{name}: {s:?}");
            assert!(s.raised.iter().all(|(_, f)| f.is_empty()), "{name}: {s:?}");
            // Either order of fill gives the same bytes (X86-5).
            let c = case(&t, &input, output);
            let one = c.apply_at(&relocs, 0).unwrap();
            assert_eq!(one, with(&input, AT, &le), "{name}: lead alone");
        }
    }
}

#[test]
fn x86_23_gd_against_thread_local_weak0_writes_t_0() {
    let t = tables(TargetKind::Weak0, true, 0);
    let (_, form, lead, p, partner, delta, _) = sequences().remove(0);
    let input = section(&form);
    let relocs = [rel(AT + p, SYM, lead), rel(AT + p + delta, GET_ADDR, partner)];
    let mut le = gd_le();
    le[8..].copy_from_slice(&[0; 4]);
    assert_eq!(applied(&t, &input, OutputKind::Static, &relocs), with(&input, AT, &le));
}

#[test]
fn x86_36_unmatched_sequences_are_refused_by_name() {
    let t = tables(TargetKind::Local, true, TLS_S);
    let mut other = tables(TargetKind::Local, true, TLS_S);
    other.names.insert(key(GET_ADDR), b"__tls_get_addr".to_vec());
    for (name, form, lead, p, partner, delta, _) in sequences() {
        let input = section(&form);
        let p = AT + p;
        let want = if lead == TLS_GD { UNRECOGNIZED_GD } else { UNRECOGNIZED_LD };
        let wrong_type = if partner == PLT32 || partner == PC32 { GOT32X } else { PLT32 };
        let pa = p + delta;
        let rs = |a, b| [rel(p, SYM, lead), rel(a, GET_ADDR, b)];
        let broken: [(&str, &Tables, Vec<u8>, [Reloc; 2]); 6] = [
            ("lead A", &t, with(&input, p, &[1]), rs(pa, partner)),
            ("partner A", &t, with(&input, pa, &[1]), rs(pa, partner)),
            ("offset", &t, input.clone(), rs(pa + 1, partner)),
            ("type", &t, input.clone(), rs(pa, wrong_type)),
            ("name", &other, input.clone(), rs(pa, partner)),
            ("bytes", &t, with(&input, p - 2, &[0x8b]), rs(pa, partner)),
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
            assert!(
                !X86::M1.consumed(OutputKind::Static, &at(&relocs, 1, &input), tb),
                "{name} {what}"
            );
        }
    }
}

#[test]
fn x86_36_review_shapes_are_refused() {
    let t = tables(TargetKind::Local, true, TLS_S);
    let e8 = [0xe8, 0xfc, 0xff, 0xff, 0xff];
    // (what, bytes, lead type, P from AT, partner offset from P).
    let shapes: [(&str, Vec<u8>, u32, usize, usize); 4] = [
        // Base GD with a bare `e8` (no D3 `nop`).
        ("base GD + e8", [&[0x8d, 0x83, 0, 0, 0, 0][..], &e8, &[0xcc]].concat(), TLS_GD, 2, 5),
        // `leal x@tlsldm(%ebx,%ecx,1), %eax`: a SIB.
        ("SIB LDM", [&[0x8d, 0x84, 0x0b, 0, 0, 0, 0][..], &e8].concat(), TLS_LDM, 3, 5),
        // `leal x@tlsgd(%ebx), %ecx`.
        ("GD into %ecx", [&[0x8d, 0x8b, 0, 0, 0, 0][..], &e8, &[0x90]].concat(), TLS_GD, 2, 5),
        // D2 with `call *x(%esp)` (rm' = 100, a SIB byte).
        (
            "D2 rm' = 100",
            vec![0x8d, 0x83, 0, 0, 0, 0, 0xff, 0x94, 0x24, 0, 0, 0, 0],
            TLS_GD,
            2,
            7,
        ),
    ];
    for (what, form, lead, p, delta) in shapes {
        let input = section(&form);
        let p_type = if delta == 5 { PLT32 } else { GOT32X };
        let relocs = [rel(AT + p, SYM, lead), rel(AT + p + delta, GET_ADDR, p_type)];
        let s = scan(&t, OutputKind::Static, &input, &relocs);
        let want = if lead == TLS_GD { UNRECOGNIZED_GD } else { UNRECOGNIZED_LD };
        assert_eq!(refused_name(&s.errors[0].1), want, "{what}: {s:?}");
    }
    // The partner two records away.
    let (_, form, lead, p, partner, delta, _) = sequences().remove(0);
    let input = section(&form);
    let relocs = [
        rel(AT + p, SYM, lead),
        rel(AT + 40, SYM, 1),
        rel(AT + p + delta, GET_ADDR, partner),
    ];
    let s = scan(&t, OutputKind::Static, &input, &relocs);
    assert_eq!(refused_name(&s.errors[0].1), UNRECOGNIZED_GD, "{s:?}");
}

#[test]
fn x86_32_gd_to_ie_is_refused() {
    // A TLS target the executable does not define (M2's `Shlib`): GD →
    // IE is not implemented (X86-32). In M1 the kind cannot occur (the
    // class table's impossible cell); fill refuses it by name.
    let t = tables(TargetKind::Shlib { func: false }, true, TLS_S);
    let (_, form, lead, p, partner, delta, _) = sequences().remove(0);
    let input = section(&form);
    let relocs = [rel(AT + p, SYM, lead), rel(AT + p + delta, GET_ADDR, partner)];
    let s = scan(&t, OutputKind::Static, &input, &relocs);
    assert_eq!(s.errors.len(), 1, "{s:?}");
    let Err(FieldError::Refused(r)) = case(&t, &input, OutputKind::Static).apply(&relocs) else {
        panic!()
    };
    assert_eq!(r.name, RefName::Named(TLS_TO_IE));
}

// ---------------------------------------------------------------- §9 T1, T2

#[test]
fn x86_37_tlsdesc_every_register() {
    let t = tables(TargetKind::Local, true, TLS_S);
    for output in EXEC {
        for reg in 0..8u8 {
            for rm in [3u8, 0, 1, 6] {
                // `leal x@tlsdesc(%rm), %reg; call *x@tlscall(%eax)`.
                let m = 0x80 | (reg << 3) | rm;
                let input = section(&[0x8d, m, 0, 0, 0, 0, 0xff, 0x10]);
                let relocs = [rel(AT + 2, SYM, TLS_GOTDESC), rel(AT + 6, SYM, TLS_DESC_CALL)];
                let out = applied(&t, &input, output, &relocs);
                let mut le = vec![0x8d, 0x05 | (reg << 3)];
                le.extend(T);
                le.extend([0x66, 0x90]);
                assert_eq!(out, with(&input, AT, &le), "{output:?} reg {reg} rm {rm}");
                let s = scan(&t, output, &input, &relocs);
                assert!(s.errors.is_empty() && raised(&s, key(SYM)).is_empty(), "{s:?}");
            }
        }
    }
    // T2 alone (its T1 in another section, or none).
    let input = section(&[0xff, 0x10]);
    let out = applied(&t, &input, OutputKind::Static, &[rel(AT, SYM, TLS_DESC_CALL)]);
    assert_eq!(out, with(&input, AT, &[0x66, 0x90]));
}

#[test]
fn x86_63_tls_module_base_is_t_0() {
    let mut t = tables(TargetKind::Local, true, TLS_S);
    t.names.insert(key(SYM), b"_TLS_MODULE_BASE_".to_vec());
    let input = section(&[0x8d, 0x83, 0, 0, 0, 0]);
    let out = applied(&t, &input, OutputKind::Static, &[rel(AT + 2, SYM, TLS_GOTDESC)]);
    assert_eq!(out, with(&input, AT, &[0x8d, 0x05, 0, 0, 0, 0]));
}

#[test]
fn x86_37_unmatched_descriptor_members_are_refused() {
    let t = tables(TargetKind::Local, true, TLS_S);
    for (what, bytes, off, r_type) in [
        ("T1 A ≠ 0", vec![0x8d, 0x83, 4, 0, 0, 0], 2, TLS_GOTDESC),
        ("T1 mov", vec![0x8b, 0x83, 0, 0, 0, 0], 2, TLS_GOTDESC),
        ("T1 SIB", vec![0x8d, 0x84, 0x1b, 0, 0, 0, 0], 3, TLS_GOTDESC),
        ("T2 ff 15", vec![0xff, 0x15], 0, TLS_DESC_CALL),
    ] {
        let input = section(&bytes);
        let relocs = [rel(AT + off, SYM, r_type)];
        let s = scan(&t, OutputKind::Static, &input, &relocs);
        assert_eq!(refused_name(&s.errors[0].1), UNRECOGNIZED_DESC, "{what}: {s:?}");
        let Err(FieldError::Refused(r)) = case(&t, &input, OutputKind::Static).apply(&relocs)
        else {
            panic!("{what}")
        };
        assert_eq!(r.name, RefName::Named(UNRECOGNIZED_DESC), "{what}");
    }
    // `TLS_DESC_CALL` at the section's last byte: no `ff 10` (X86-40).
    let input = vec![0xcc, 0xff];
    let s = scan(&t, OutputKind::Static, &input, &[rel(1, SYM, TLS_DESC_CALL)]);
    assert_eq!(refused_name(&s.errors[0].1), UNRECOGNIZED_DESC);
}

// ---------------------------------------------------------------- X86-31, X86-40, X86-41

fn at<'a>(relocs: &'a [Reloc], i: usize, input: &'a [u8]) -> RelocAt<'a> {
    let r = relocs[i];
    let h = X86::M1.classify(r.r_type, Site { bytes: input, offset: r.r_offset }).unwrap();
    RelocAt {
        file: FileId(1),
        section: SectionId(3),
        index: u32::try_from(i).unwrap(),
        r_offset: r.r_offset,
        key: r.key,
        r_type: r.r_type,
        addend: None,
        howto: Box::leak(Box::new(h)),
        site: Site { bytes: input, offset: r.r_offset },
        records: Box::leak(Box::new(relocs)),
        proof: Box::leak(all_code(input.len()).into_boxed_slice()),
        leads: true,
    }
}

#[test]
fn x86_41_sequence_extent() {
    let x = X86::M1;
    let r = |o: u64, t: u32| Reloc { r_offset: o, key: key(1), r_type: t, addend: 0 };
    let ext = |rs: &[Reloc], i| x.sequence_extent(&rs, i);
    // A GD/LDM with any next record: the records alone decide.
    assert_eq!(ext(&[r(3, TLS_GD), r(8, PLT32)], 0), 2);
    assert_eq!(ext(&[r(2, TLS_GD), r(8, GOT32X)], 0), 2);
    assert_eq!(ext(&[r(2, TLS_LDM), r(7, PC32)], 0), 2);
    assert_eq!(ext(&[r(2, TLS_LDM), r(30, 1)], 0), 2);
    assert_eq!(ext(&[r(2, TLS_GD)], 0), 1);
    assert_eq!(ext(&[r(2, TLS_GOTDESC), r(6, TLS_DESC_CALL)], 0), 1);
    assert_eq!(ext(&[r(2, PLT32), r(8, PLT32)], 0), 1);
    assert_eq!(ext(&[r(2, TLS_GD), r(8, PLT32)], 1), 1);
    assert_eq!(ext(&[r(2, TLS_GD), r(8, PLT32)], 7), 1);
}

#[test]
fn x86_40_unit_windows() {
    let t = tables(TargetKind::Local, true, TLS_S);
    let x = X86::M1;
    for (name, form, lead, p, partner, delta, le) in sequences() {
        let input = section(&form);
        let p = AT + p;
        let relocs = [rel(p, SYM, lead), rel(p + delta, GET_ADDR, partner)];
        let l = at(&relocs, 0, &input);
        let f = at(&relocs, 1, &input);
        let start = AT as u64;
        let end = (AT + le.len()) as u64;
        let p = p as u64;
        let w = l.howto.window;
        assert_eq!(p - u64::from(w.before), start, "{name}");
        assert_eq!(p + 4 + u64::from(w.after), end, "{name}");
        assert_eq!(
            x.unit_window(OutputKind::Static, &l, &t),
            Some(UnitWindow { start, end, unit: 0, record: 0 }),
            "{name}"
        );
        let fp = p + delta as u64;
        assert_eq!(
            x.unit_window(OutputKind::Static, &f, &t),
            Some(UnitWindow { start: fp, end: fp, unit: 0, record: 1 }),
            "{name}"
        );
        assert!(x.consumed(OutputKind::Static, &f, &t));
        assert!(!x.consumed(OutputKind::Static, &l, &t));
    }
    // An unmatched lead: its read window `[P − 3, P + 4)` (X86-40).
    let (_, form, lead, p, partner, delta, _) = sequences().remove(0);
    let input = with(&section(&form), AT + p, &[1]);
    let p = AT + p;
    let bad = [rel(p, SYM, lead), rel(p + delta, GET_ADDR, partner)];
    let p = p as u64;
    assert_eq!(
        x.unit_window(OutputKind::Static, &at(&bad, 0, &input), &t),
        Some(UnitWindow { start: p - 3, end: p + 4, unit: 0, record: 0 })
    );
}

#[test]
fn x86_40_windows_outside_the_section_do_not_match() {
    let t = tables(TargetKind::Local, false, S);
    let x = X86::M1;
    // `GOT32X` with its field at offset 0 and 1: the `ModRM` or opcode is
    // before the section. At 0 no operand can be read: refused.
    let input = vec![0x83, 0, 0, 0, 0];
    let h = x.classify(GOT32X, Site { bytes: &input, offset: 1 }).unwrap();
    assert_eq!(h.window, Window { before: 1, after: 0 });
    let out = case(&t, &input, OutputKind::Static).apply(&[rel(1, SYM, GOT32X)]).unwrap();
    assert_eq!(out, with(&input, 1, &le(SLOT.wrapping_sub(GOT))));
    let input = vec![0, 0, 0, 0];
    let h = x.classify(GOT32X, Site { bytes: &input, offset: 0 }).unwrap();
    assert_eq!(h.window, Window { before: 0, after: 0 });
    // D1 with its `8d` one byte before the section: refused (X86-36).
    let tt = tables(TargetKind::Local, true, TLS_S);
    let (_, mut form, lead, p, partner, delta, _) = sequences().remove(0);
    form.remove(0);
    let relocs = [rel(p - 1, SYM, lead), rel(p - 1 + delta, GET_ADDR, partner)];
    let s = scan(&tt, OutputKind::Static, &form, &relocs);
    assert_eq!(refused_name(&s.errors[0].1), UNRECOGNIZED_GD);
    // D1 cut at the section end.
    let (_, mut form, ..) = sequences().remove(0);
    form.pop();
    let relocs = [rel(p, SYM, lead), rel(p + delta, GET_ADDR, partner)];
    let s = scan(&tt, OutputKind::Static, &form, &relocs);
    assert!(!s.errors.is_empty());
    // The full form at the section's start and end matches.
    let (_, form, ..) = sequences().remove(0);
    let s = scan(&tt, OutputKind::Static, &form, &relocs);
    assert!(s.errors.is_empty(), "{s:?}");
}

#[test]
fn x86_31_partners_are_consumed_once() {
    let t = tables(TargetKind::Local, true, TLS_S);
    let (_, one, lead, p, partner, delta, le) = sequences().remove(0);
    let form = [one.clone(), one].concat();
    let input = section(&form);
    let n = le.len();
    let relocs = [
        rel(AT + p, SYM, lead),
        rel(AT + p + delta, GET_ADDR, partner),
        rel(AT + n + p, SYM, lead),
        rel(AT + n + p + delta, GET_ADDR, partner),
    ];
    let want = with(&with(&input, AT, &le), AT + n, &le);
    assert_eq!(applied(&t, &input, OutputKind::Static, &relocs), want);
    let x = X86::M1;
    let consumed: Vec<bool> =
        (0..4).map(|i| x.consumed(OutputKind::Static, &at(&relocs, i, &input), &t)).collect();
    assert_eq!(consumed, [false, true, false, true]);
    let s = scan(&t, OutputKind::Static, &input, &relocs);
    assert!(s.errors.is_empty() && s.events.is_empty(), "{s:?}");
    assert!(!s.events.iter().any(|(_, c, _)| *c == EventClass::Undefined));
    // `leads = false`: the partner is not consumed.
    let mut f = at(&relocs, 1, &input);
    f.leads = false;
    assert!(relax::follower_of(&f, &t).is_none());
}

#[test]
fn shown_bytes_are_the_reach_clipped() {
    let input: Vec<u8> = (0u8..40).collect();
    // `[P − 3, P + 4 + 6)`.
    assert_eq!(shown_bytes(&input, 5), "02 03 04 05 06 07 08 09 0a 0b 0c 0d 0e");
    assert_eq!(shown_bytes(&input, 1), "00 01 02 03 04 05 06 07 08 09 0a");
    assert_eq!(shown_bytes(&input, 36), "21 22 23 24 25 26 27");
}

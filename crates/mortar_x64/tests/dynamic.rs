//! WP-048: x86-64 in dynamic executables (`spec/dyn.md`, x64.md's M2
//! rules), with the M2 rules on (`X64::M2`):
//!
//! - the demand cells of `exec` and `pie` against shared functions and
//!   data and `weakdyn` keys, each refusal by its name (D-43, D-44, D-46,
//!   D-47, X-57);
//! - `weak_use` over every type of §6 (X-57, the branch-opcode test);
//! - the lazy PLT (§10.3, D-49): PLT0, the entries, the lazy target;
//! - the TLS transitions of `exec` and `pie` (D-48, X-23, X-25 to X-28,
//!   X-58), byte for byte;
//! - the GOT relaxations' scope (X-19, X-20, R-X1);
//! - what stays refused (IBT PLT, Q4) and what M1 keeps refusing.
//!
//! The byte rules are compared with GNU ld's and lld's outputs in
//! `dyndiff.rs`.
#![allow(clippy::many_single_char_names, clippy::type_complexity)]

mod support;

use mortar_arch::{
    Arch, DemandFlags, DynRefusal, DynRole, FieldError, GlobalFlags, Incompatible, LazyTarget,
    OutputCx, OutputKind, PlaceFacts, PltPart, PltVariant, RefName, Reloc, ScanError, SlotKind,
    SynthCx, TargetKind, TlsBlock, WeakClass,
};
use mortar_x64::{
    TLS_TO_IE, TLS_WEAK_DYNAMIC, UNRECOGNIZED_DESC, UNRECOGNIZED_GD, UNRECOGNIZED_LD, X64, synth,
    table,
};
use support::{Case, Sink, Tables, key, raised, scan_all};

const M2: X64 = X64::M2;
const ADDR: u64 = 0x1000;
const LEN: usize = 64;
const AT: usize = 16;
const RO: PlaceFacts = PlaceFacts { alloc: true, writable: false, exec: Some(true) };
const RW: PlaceFacts = PlaceFacts { alloc: true, writable: true, exec: Some(false) };
const TLS: TlsBlock = TlsBlock { start: 0x3000, size: 0x10, align: 8 };
const SYM: u32 = 1;
const GET_ADDR: u32 = 2;
/// The key's GOT slot, `GTPREL` slot, `GTLSIDX` pair and PLT entry.
const SLOT: u64 = 0x4000;
const TPSLOT: u64 = 0x4008;
const GDSLOT: u64 = 0x4010;
const PLT: u64 = 0x1800;
const GET_ADDR_PLT: u64 = 0x1810;
/// A copy allocation or canonical PLT entry (`S` in this output).
const COPY_S: u64 = 0x5000;

const SHLIB_DATA: TargetKind = TargetKind::Shlib { func: false };
const SHLIB_FUNC: TargetKind = TargetKind::Shlib { func: true };
const DYN: [OutputKind; 2] = [OutputKind::Exec, OutputKind::Pie];

fn tables(kind: TargetKind, tls: bool) -> Tables {
    let mut t = Tables::default();
    let s = match kind {
        TargetKind::Local | TargetKind::Ifunc => 0x2468,
        _ => 0,
    };
    if tls {
        t.tls_sym(key(SYM), if kind == TargetKind::Local { 0x3008 } else { 0 }, kind);
    } else {
        t.sym(key(SYM), s, kind);
    }
    t.slots.insert((key(SYM), SlotKind::Addr), SLOT);
    t.slots.insert((key(SYM), SlotKind::TpOff { negated: false }), TPSLOT);
    t.slots.insert((key(SYM), SlotKind::TlsGd), GDSLOT);
    t.plts.insert(key(SYM), PLT);
    t.sym(key(GET_ADDR), 0, SHLIB_FUNC);
    t.names.insert(key(GET_ADDR), b"__tls_get_addr".to_vec());
    t.plts.insert(key(GET_ADDR), GET_ADDR_PLT);
    t.flags.insert(key(GET_ADDR), DemandFlags::PLT);
    t
}

fn section(form: &[u8]) -> Vec<u8> {
    let mut b = vec![0xcc; LEN];
    b[AT..AT + form.len()].copy_from_slice(form);
    b
}

fn with(input: &[u8], off: usize, bytes: &[u8]) -> Vec<u8> {
    let mut b = input.to_vec();
    b[off..off + bytes.len()].copy_from_slice(bytes);
    b
}

fn rel(off: usize, k: u32, r_type: u32, addend: i64) -> Reloc {
    Reloc { r_offset: off as u64, key: key(k), r_type, addend }
}

fn scan(
    arch: X64,
    t: &Tables,
    output: OutputKind,
    place: PlaceFacts,
    input: &[u8],
    relocs: &[Reloc],
) -> Sink {
    let mut sink = Sink::default();
    scan_all(arch, t, output, place, input, relocs, &mut sink).unwrap();
    sink
}

fn case<'a>(t: &'a Tables, input: &'a [u8], output: OutputKind) -> Case<'a> {
    let mut c = Case::new(t, input, ADDR);
    c.arch = M2;
    c.output = output;
    c.tls = Some(TLS);
    c.got_base = Some(0x4800);
    c
}

fn refused_name(e: &ScanError) -> &'static str {
    match e {
        ScanError::Refused(r) => match r.name {
            RefName::Named(n) => n,
            other => panic!("unnamed refusal {other:?}"),
        },
        other => panic!("not a refusal: {other:?}"),
    }
}

// ---------------------------------------------------------------- the demand cells

/// What one relocation's scan gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Got {
    /// Flags on the key, the global flags, the dynamic record counted.
    Demand(DemandFlags, GlobalFlags, Option<DynRole>),
    Dyn(DynRefusal),
    Inc(Incompatible),
}

fn outcome(s: &Sink) -> Got {
    match s.errors.as_slice() {
        [] => {
            let role = match s.counts.as_slice() {
                [] => None,
                [(r, 1)] => Some(*r),
                other => panic!("counts {other:?}"),
            };
            Got::Demand(raised(s, key(SYM)), s.global, role)
        }
        [(0, ScanError::Dynamic(d))] => Got::Dyn(*d),
        [(0, ScanError::Incompatible(i))] => Got::Inc(*i),
        other => panic!("errors {other:?}"),
    }
}

/// The bytes before the field of each probe: a `mov` for the data forms
/// (no branch opcode), a `call` for the branch forms.
const MOV: [u8; 2] = [0x8b, 0x05];
const CALL: [u8; 1] = [0xe8];

fn cell(
    r_type: u32,
    output: OutputKind,
    kind: TargetKind,
    place: PlaceFacts,
    prefix: &[u8],
) -> Got {
    let t = tables(kind, false);
    let input = section(prefix);
    let s = scan(M2, &t, output, place, &input, &[rel(AT + prefix.len(), SYM, r_type, -4)]);
    outcome(&s)
}

const fn d(flags: DemandFlags) -> Got {
    Got::Demand(flags, GlobalFlags::empty(), None)
}

#[test]
fn demand_cells_against_shared_functions_and_data() {
    use DemandFlags as F;
    use OutputKind::{Exec, Pie};
    let none = GlobalFlags::empty();
    let base = GlobalFlags::GOT_BASE;
    let sym = Got::Demand(F::EXPORT, none, Some(DynRole::Symbolic));
    // D-47: the arch says `read-only`; the scan driver names it
    // `text-relocation` in an output with `.dynamic` (`mortar_scan`
    // `dynamic_error`), and a static PIE keeps the M1 diagnostic.
    let text = Got::Inc(Incompatible::ReadOnly);
    let canon = Got::Dyn(DynRefusal::CanonicalPltInPie);
    // (type, prefix, place, output, data cell, function cell)
    let rows: &[(u32, &[u8], PlaceFacts, OutputKind, Got, Got)] = &[
        // `PC32` not on a branch: copy / canonical PLT in `exec`; D-43 copy
        // and D-44 refusal in `pie`.
        (2, &MOV, RO, Exec, d(F::COPY), d(F::CANON_PLT)),
        (2, &MOV, RO, Pie, d(F::COPY), canon),
        (2, &MOV, RW, Pie, d(F::COPY), canon),
        // `PC32` on a call: the same cells (the branch-opcode test is
        // `weakdyn`'s; spec finding in the record).
        (2, &CALL, RO, Exec, d(F::COPY), d(F::CANON_PLT)),
        (2, &CALL, RO, Pie, d(F::COPY), canon),
        (13, &MOV, RO, Exec, d(F::COPY), d(F::CANON_PLT)),
        (13, &MOV, RO, Pie, d(F::COPY), canon),
        (15, &MOV, RO, Pie, d(F::COPY), canon),
        (24, &MOV, RO, Exec, d(F::COPY), d(F::CANON_PLT)),
        (24, &MOV, RO, Pie, d(F::COPY), canon),
        // Branches: PLT in both.
        (4, &CALL, RO, Exec, d(F::PLT), d(F::PLT)),
        (4, &CALL, RO, Pie, d(F::PLT), d(F::PLT)),
        (31, &[], RO, Exec, Got::Demand(F::PLT, base, None), Got::Demand(F::PLT, base, None)),
        (31, &[], RO, Pie, Got::Demand(F::PLT, base, None), Got::Demand(F::PLT, base, None)),
        // GOT forms: a slot (its `GLOB_DAT` is synth's), never relaxed.
        (9, &MOV, RO, Exec, d(F::GOT), d(F::GOT)),
        (41, &[0xff, 0x15], RO, Pie, d(F::GOT), d(F::GOT)),
        (42, &[0x48, 0x8b, 0x05], RO, Exec, d(F::GOT), d(F::GOT)),
        (42, &[0x48, 0x8b, 0x05], RO, Pie, d(F::GOT), d(F::GOT)),
        (27, &[], RO, Pie, Got::Demand(F::GOT, base, None), Got::Demand(F::GOT, base, None)),
        // `64`: a symbolic record in a writable place; read-only, a copy or
        // canonical PLT in `exec`, a text relocation in `pie` (D-47).
        (1, &[], RW, Exec, sym, sym),
        (1, &[], RW, Pie, sym, sym),
        (1, &[], RO, Exec, d(F::COPY), d(F::CANON_PLT)),
        (1, &[], RO, Pie, text, text),
        // `32`, `32S`, `16`, `8`: copy / canonical PLT in `exec`; in `pie`
        // `relocation-incompatible` against data and `canonical-plt-in-pie`
        // against a function (D-44).
        (10, &MOV, RO, Exec, d(F::COPY), d(F::CANON_PLT)),
        (11, &MOV, RO, Exec, d(F::COPY), d(F::CANON_PLT)),
        (10, &MOV, RW, Pie, Got::Inc(Incompatible::RecompilePie), canon),
        (11, &MOV, RO, Pie, Got::Inc(Incompatible::RecompilePie), canon),
        (12, &MOV, RO, Pie, Got::Inc(Incompatible::RecompilePie), canon),
        (14, &MOV, RO, Exec, d(F::COPY), d(F::CANON_PLT)),
        // `GOTOFF64` cannot reach a shared key (arch.md §4.3 `E`).
        (
            25,
            &[],
            RO,
            Exec,
            Got::Inc(Incompatible::RecompilePic),
            Got::Inc(Incompatible::RecompilePic),
        ),
        (
            25,
            &[],
            RO,
            Pie,
            Got::Inc(Incompatible::RecompilePie),
            Got::Inc(Incompatible::RecompilePie),
        ),
        // `SIZE*` and `GOTPC*`: no demand on the key.
        (32, &[], RO, Pie, d(F::empty()), d(F::empty())),
        (33, &[], RW, Exec, d(F::empty()), d(F::empty())),
        (
            26,
            &[],
            RO,
            Pie,
            Got::Demand(F::empty(), base, None),
            Got::Demand(F::empty(), base, None),
        ),
    ];
    for &(r_type, prefix, place, output, data, func) in rows {
        let name = table::howto(r_type).unwrap().name;
        assert_eq!(
            cell(r_type, output, SHLIB_DATA, place, prefix),
            data,
            "{name} {output:?} {place:?} data"
        );
        assert_eq!(
            cell(r_type, output, SHLIB_FUNC, place, prefix),
            func,
            "{name} {output:?} {place:?} func"
        );
    }
}

#[test]
fn demand_cells_against_weakdyn_keys() {
    use DemandFlags as F;
    let base = GlobalFlags::GOT_BASE;
    // X-57: the uses a `weakdyn` key can have (every other use made it
    // `weak0` or refused it before the walk, D-25).
    for output in DYN {
        let w = TargetKind::WeakDyn;
        assert_eq!(cell(42, output, w, RO, &[0x48, 0x8b, 0x05]), d(F::GOT), "{output:?}");
        assert_eq!(cell(9, output, w, RO, &[0x48, 0x83, 0x3d]), d(F::GOT), "cmpq $0 {output:?}");
        assert_eq!(cell(27, output, w, RO, &[]), Got::Demand(F::GOT, base, None));
        assert_eq!(
            cell(1, output, w, RW, &[]),
            Got::Demand(F::EXPORT, GlobalFlags::empty(), Some(DynRole::Symbolic))
        );
        assert_eq!(cell(4, output, w, RO, &CALL), d(F::PLT), "PLT32 {output:?}");
        assert_eq!(cell(31, output, w, RO, &[]), Got::Demand(F::PLT, base, None));
        // `PC32` on `e8`, `e9`, `0f 8x`: a branch, through the PLT entry.
        for b in [&[0xe8][..], &[0xe9], &[0x0f, 0x80], &[0x0f, 0x84], &[0x0f, 0x8f]] {
            assert_eq!(cell(2, output, w, RO, b), d(F::PLT), "PC32 after {b:x?} {output:?}");
        }
        // `64` in a read-only place: `read-only` in `pie`, which the
        // driver names `text-relocation` (D-47).
        if output == OutputKind::Pie {
            assert_eq!(cell(1, output, w, RO, &[]), Got::Inc(Incompatible::ReadOnly));
        }
    }
}

#[test]
fn local_and_ifunc_cells_are_the_generic_table() {
    use DemandFlags as F;
    use OutputKind::{Exec, Pie};
    let none = GlobalFlags::empty();
    let l = TargetKind::Local;
    let i = TargetKind::Ifunc;
    // D-46: a `Local` word is `RELATIVE` in `pie`, a value in `exec`.
    assert_eq!(cell(1, Pie, l, RW, &[]), Got::Demand(F::empty(), none, Some(DynRole::Relative)));
    assert_eq!(cell(1, Exec, l, RW, &[]), d(F::empty()));
    assert_eq!(cell(42, Pie, l, RO, &[0x48, 0x8b, 0x05]), d(F::GOT));
    // D-45: one IPLT entry, the address everywhere; a GOT slot holds it.
    for o in DYN {
        assert_eq!(cell(42, o, i, RO, &[0x48, 0x8b, 0x05]), d(F::GOT | F::IPLT), "{o:?}");
        assert_eq!(cell(2, o, i, RO, &MOV), d(F::IPLT), "{o:?}");
        assert_eq!(cell(4, o, i, RO, &CALL), d(F::IPLT), "{o:?}");
    }
    assert_eq!(cell(1, Pie, i, RW, &[]), Got::Demand(F::IPLT, none, Some(DynRole::Relative)));
    assert_eq!(cell(1, Exec, i, RW, &[]), d(F::IPLT));
    // The read-only `Local`/`Ifunc` word in `pie`: `read-only` from the
    // arch; the driver makes it `text-relocation` only in an output with
    // `.dynamic`, so a static PIE keeps `relocation-incompatible` (D-47).
    assert_eq!(cell(1, Pie, l, RO, &[]), Got::Inc(Incompatible::ReadOnly));
}

/// The scan rules M2 adds are only for targets a dynamic output has. The
/// one cell where M1 and M2 differ outside TLS is X-57's `PC32` on a
/// branch against a `weakdyn` key. Every shared-key cell is the shared
/// table's (D-43, D-44) under both.
#[test]
fn m1_and_m2_scan_alike_but_the_weakdyn_branch() {
    let types: &[(u32, &[u8])] = &[
        (1, &[]),
        (2, &MOV),
        (2, &CALL),
        (4, &CALL),
        (9, &MOV),
        (10, &MOV),
        (11, &MOV),
        (24, &MOV),
        (31, &[]),
        (41, &[0xff, 0x15]),
        (42, &[0x48, 0x8b, 0x05]),
    ];
    for kind in [SHLIB_DATA, SHLIB_FUNC, TargetKind::WeakDyn, TargetKind::Local, TargetKind::Ifunc]
    {
        let t = tables(kind, false);
        for &(r_type, prefix) in types {
            let input = section(prefix);
            let r = [rel(AT + prefix.len(), SYM, r_type, -4)];
            for output in [OutputKind::Static, OutputKind::Exec, OutputKind::Pie] {
                if output == OutputKind::Static
                    && !matches!(kind, TargetKind::Local | TargetKind::Ifunc)
                {
                    continue;
                }
                for place in [RO, RW] {
                    let m1 = outcome(&scan(X64::M1, &t, output, place, &input, &r));
                    let m2 = outcome(&scan(M2, &t, output, place, &input, &r));
                    if kind == TargetKind::WeakDyn && r_type == 2 && prefix == CALL {
                        assert_eq!(m2, d(DemandFlags::PLT));
                        assert_ne!(m1, m2, "X-57 is M2's");
                    } else {
                        assert_eq!(m1, m2, "{r_type} {kind:?} {output:?} {place:?}");
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------- X-57 weak_use

#[test]
fn x57_weak_use_every_type() {
    use WeakClass::{AbsWord, Branch, Got, Unfillable};
    let mov = section(&MOV);
    let p = (AT + 2) as u64;
    for r_type in 0..=60u32 {
        let want = match r_type {
            0 | 26 | 29 | 32 | 33 => None,
            9 | 27 | 41 | 42 => Some(Got),
            4 | 31 => Some(Branch),
            _ => Some(Unfillable),
        };
        for writable in [false, true] {
            let want = if r_type == 1 && writable { Some(AbsWord) } else { want };
            assert_eq!(M2.weak_use(r_type, &mov, p, writable), want, "{r_type} {writable}");
        }
        // M1: every type unfillable (every weak key `weak0`).
        assert_eq!(X64::M1.weak_use(r_type, &mov, p, true), Some(Unfillable));
    }
}

#[test]
fn x57_branch_opcode_test() {
    let cases: &[(&[u8], usize, bool)] = &[
        (&[0xe8, 0, 0, 0, 0], 1, true),
        (&[0xe9, 0, 0, 0, 0], 1, true),
        (&[0x0f, 0x80, 0, 0, 0, 0], 2, true),
        (&[0x0f, 0x85, 0, 0, 0, 0], 2, true),
        (&[0x0f, 0x8f, 0, 0, 0, 0], 2, true),
        (&[0x0f, 0x90, 0, 0, 0, 0], 2, false),
        (&[0x0e, 0x84, 0, 0, 0, 0], 2, false),
        (&[0x8b, 0x05, 0, 0, 0, 0], 2, false),
        (&[0xeb, 0, 0, 0, 0], 1, false),
        (&[0xff, 0x15, 0, 0, 0, 0], 2, false),
        // At offset 0 of its section: nothing before it.
        (&[0, 0, 0, 0], 0, false),
        // At offset 1: only the `e8`/`e9` byte can be there.
        (&[0x0f, 0, 0, 0, 0], 1, false),
        (&[0xe8, 0, 0, 0, 0], 1, true),
    ];
    for &(bytes, off, branch) in cases {
        assert_eq!(mortar_x64::on_branch(bytes, off as u64), branch, "{bytes:x?} at {off}");
        let want = if branch { WeakClass::Branch } else { WeakClass::Unfillable };
        assert_eq!(M2.weak_use(2, bytes, off as u64, false), Some(want), "{bytes:x?}");
        // Only `PC32` reads the bytes.
        assert_eq!(M2.weak_use(13, bytes, off as u64, false), Some(WeakClass::Unfillable));
    }
}

// ---------------------------------------------------------------- §10.3 PLT, D-49

/// GNU ld 2.44's and lld 22's bytes (`dyndiff.rs` compares them in their
/// own layouts): PLT0 at 0x1020 with `.got.plt` at 0x4000, the entry at
/// 0x1030 with its word at 0x4018 and index 0, the entry at 0x1040 with
/// 0x4020 and index 1.
#[test]
fn plt0_and_lazy_entries() {
    let x = M2;
    let cx =
        |addr, got_plt, index| SynthCx { addr, header: 0x1020, got_plt, got_base: 0x4000, index };
    let mut b = [0u8; 16];
    x.write_plt(PltVariant::Lazy, PltPart::Header(0), &cx(0x1020, 0x4010, 0), &mut b).unwrap();
    // push 0x4008 − 0x1026 = 0x2fe2; jmp *(0x4010 − 0x102c = 0x2fe4).
    assert_eq!(
        b,
        [0xff, 0x35, 0xe2, 0x2f, 0, 0, 0xff, 0x25, 0xe4, 0x2f, 0, 0, 0x0f, 0x1f, 0x40, 0]
    );
    x.write_plt(
        PltVariant::Lazy,
        PltPart::Entry { chunk: 0, n: 0 },
        &cx(0x1030, 0x4018, 0),
        &mut b,
    )
    .unwrap();
    // jmp *(0x4018 − 0x1036 = 0x2fe2); push $0; jmp 0x1020 − 0x1040 = −0x20.
    assert_eq!(b, [0xff, 0x25, 0xe2, 0x2f, 0, 0, 0x68, 0, 0, 0, 0, 0xe9, 0xe0, 0xff, 0xff, 0xff]);
    x.write_plt(
        PltVariant::Lazy,
        PltPart::Entry { chunk: 0, n: 1 },
        &cx(0x1040, 0x4020, 1),
        &mut b,
    )
    .unwrap();
    assert_eq!(b, [0xff, 0x25, 0xda, 0x2f, 0, 0, 0x68, 1, 0, 0, 0, 0xe9, 0xd0, 0xff, 0xff, 0xff]);
    // The IPLT entries follow the PLT entries, the shape of X-37.
    let mut i = [0u8; 8];
    x.write_plt(PltVariant::Lazy, PltPart::Iplt(0), &cx(0x1050, 0x4028, 2), &mut i).unwrap();
    assert_eq!(i, [0xff, 0x25, 0xd2, 0x2f, 0, 0, 0x66, 0x90]);
    // The shape: 16-byte header and entries in `.plt`, 8-byte IPLT
    // entries; `-z now` keeps it (a flag, not a variant).
    let s = x.plt(PltVariant::Lazy);
    assert_eq!(
        (s.chunks.len(), s.chunks[0].header, s.chunks[0].entry, s.iplt_entry),
        (1, 16, 16, 8)
    );
    // A rel32 that does not fit is `relocation-out-of-range`.
    assert!(matches!(
        x.write_plt(
            PltVariant::Lazy,
            PltPart::Header(0),
            &SynthCx { got_base: 1 << 40, ..cx(0x1020, 0, 0) },
            &mut b
        ),
        Err(FieldError::Range(_))
    ));
}

#[test]
fn lazy_target_tags_and_variants() {
    let cx = |output, feature_1_and, force_bti, bind_now| OutputCx {
        output,
        feature_1_and,
        pac_plt: false,
        force_bti,
        bind_now,
    };
    for o in DYN {
        for now in [false, true] {
            let c = cx(o, 0, false, now);
            assert_eq!(M2.plt_variant(&c), Ok(PltVariant::Lazy), "{o:?} now={now}");
            assert!(M2.dyn_tags(&c).is_empty(), "D-30: no x86-64 tag");
        }
    }
    assert_eq!(M2.lazy_target(PltVariant::Lazy), Some(LazyTarget::Entry { plus: 6 }));
    // Q4: the IBT PLT stays refused, by name, at its first header or
    // entry (the IBT IPLT of a static link is M1 and unchanged).
    assert_eq!(M2.lazy_target(PltVariant::Ibt), None);
    let s = SynthCx { addr: 0x1020, header: 0x1020, got_plt: 0x4010, got_base: 0x4000, index: 0 };
    for part in
        [PltPart::Header(0), PltPart::Entry { chunk: 0, n: 0 }, PltPart::Entry { chunk: 1, n: 0 }]
    {
        assert_eq!(
            M2.write_plt(PltVariant::Ibt, part, &s, &mut [0; 16]),
            Err(FieldError::Refused(synth::IBT_PLT)),
            "{part:?}"
        );
    }
    let mut b = [0u8; 16];
    M2.write_plt(PltVariant::Ibt, PltPart::Iplt(0), &s, &mut b).unwrap();
    assert_eq!(b[..4], [0xf3, 0x0f, 0x1e, 0xfa]);
    // M1: no dynamic PLT, no lazy target.
    assert_eq!(X64::M1.lazy_target(PltVariant::Lazy), None);
    assert_eq!(
        X64::M1.write_plt(PltVariant::Lazy, PltPart::Header(0), &s, &mut [0; 16]),
        Err(FieldError::Refused(synth::LAZY_PLT))
    );
    assert_eq!(X64::new(mortar_arch::Status::M2), M2);
    assert_eq!(X64::new(mortar_arch::Status::Later), X64::M1);
    assert_eq!(X64::default(), X64::M1);
}

// ---------------------------------------------------------------- D-48 TLS

const N16: [u8; 12] = [0x64, 0x48, 0x8b, 0x04, 0x25, 0, 0, 0, 0, 0x48, 0x03, 0x05];
const NOPW6: [u8; 6] = [0x66, 0x0f, 0x1f, 0x44, 0x00, 0x00];

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

/// `G − end` for the field ending at section offset `end`.
fn rel_to(g: u64, end: usize) -> [u8; 4] {
    (g.wrapping_sub(ADDR + end as u64) as i32).to_le_bytes()
}

#[test]
fn x58_gd_to_ie() {
    let t = tables(SHLIB_DATA, true);
    // (form, P in the form, follower type, follower offset, IE output)
    let mut forms: Vec<(Vec<u8>, usize, u32, usize, Vec<u8>)> = Vec::new();
    for (x, followers) in [(false, [4u32, 2]), (true, [41, 9])] {
        for f in followers {
            let mut ie = N16.to_vec();
            ie.extend(rel_to(TPSLOT, AT + 16));
            forms.push((gd16(x), 4, f, 8, ie));
        }
    }
    for (rex, r) in [(0x48, 0), (0x48, 3), (0x4c, 7)] {
        let mut ie = N16.to_vec();
        ie.extend(rel_to(TPSLOT, AT + 16));
        ie.extend(NOPW6);
        forms.push((large(rex, r), 3, 31, 6, ie));
    }
    for output in DYN {
        for (form, p, f, delta, ie) in &forms {
            let input = section(form);
            let p = AT + p;
            let relocs = [rel(p, SYM, 19, -4), rel(p + delta, GET_ADDR, *f, -4)];
            let c = case(&t, &input, output);
            let want = with(&input, AT, ie);
            assert_eq!(c.apply(&relocs).unwrap(), want, "{output:?} {f}");
            assert_eq!(c.apply_at(&relocs, 1).unwrap(), input, "the follower writes nothing");
            // Kind 1: `GOT_TPREL` on the key, nothing on `__tls_get_addr`.
            let s = scan(M2, &t, output, RO, &input, &relocs);
            assert!(s.errors.is_empty(), "{s:?}");
            assert_eq!(s.raised, vec![(key(SYM), DemandFlags::GOT_TPREL)], "{output:?} {f}");
            assert!(s.global.is_empty(), "D-48: never DF_STATIC_TLS");
        }
    }
    // M1 still refuses GD → IE (x64.md X-32 before M2).
    let input = section(&gd16(false));
    let relocs = [rel(AT + 4, SYM, 19, -4), rel(AT + 12, GET_ADDR, 4, -4)];
    let s = scan(X64::M1, &t, OutputKind::Exec, RO, &input, &relocs);
    assert_eq!(refused_name(&s.errors[0].1), TLS_TO_IE);
}

#[test]
fn x58_tlsdesc_to_ie_every_register() {
    let t = tables(SHLIB_DATA, true);
    for output in DYN {
        for rex in [0x48u8, 0x4c] {
            for reg in 0..8u8 {
                let modrm = (reg << 3) | 5;
                let input = section(&[rex, 0x8d, modrm, 0, 0, 0, 0, 0xff, 0x10]);
                let relocs = [rel(AT + 3, SYM, 34, -4), rel(AT + 7, SYM, 35, 0)];
                let mut want = with(&input, AT, &[rex, 0x8b, modrm]);
                // `G(GTPREL(S)) + A − P`, `A = −4`.
                want[AT + 3..AT + 7].copy_from_slice(&rel_to(TPSLOT, AT + 7));
                want[AT + 7..AT + 9].copy_from_slice(&[0x66, 0x90]);
                let c = case(&t, &input, output);
                assert_eq!(c.apply(&relocs).unwrap(), want, "rex {rex:#x} reg {reg}");
                let s = scan(M2, &t, output, RO, &input, &relocs);
                assert!(s.errors.is_empty(), "{s:?}");
                assert_eq!(s.raised, vec![(key(SYM), DemandFlags::GOT_TPREL)]);
            }
        }
    }
    // X-28: an unrecognized member is refused, against a shared key too.
    for (form, r_type, p) in [(&[0x48, 0x8b, 0x05, 0, 0, 0, 0][..], 34, 3), (&[0xff, 0x11], 35, 0)]
    {
        let input = section(form);
        let relocs = [rel(AT + p, SYM, r_type, if r_type == 34 { -4 } else { 0 })];
        let s = scan(M2, &t, OutputKind::Pie, RO, &input, &relocs);
        assert_eq!(refused_name(&s.errors[0].1), UNRECOGNIZED_DESC);
        let Err(FieldError::Refused(r)) = case(&t, &input, OutputKind::Pie).apply(&relocs) else {
            panic!()
        };
        assert_eq!(r.name, RefName::Named(UNRECOGNIZED_DESC));
    }
}

#[test]
fn x26_unmatched_gd_against_a_shared_key_keeps_its_pair() {
    let t = tables(SHLIB_DATA, true);
    // GD16 with a `PC32` follower 1 byte off: no form matches.
    let input = section(&gd16(false));
    let relocs = [rel(AT + 4, SYM, 19, -4), rel(AT + 13, GET_ADDR, 4, -4)];
    for output in DYN {
        let s = scan(M2, &t, output, RO, &input, &relocs);
        assert!(s.errors.is_empty(), "{s:?}");
        assert_eq!(raised(&s, key(SYM)), DemandFlags::TLS_GD);
        // The call is kept: a PLT reference to `__tls_get_addr`.
        assert_eq!(raised(&s, key(GET_ADDR)), DemandFlags::PLT);
        let out = case(&t, &input, output).apply(&relocs).unwrap();
        let mut want = with(&input, AT + 4, &rel_to(GDSLOT, AT + 4 + 4));
        want[AT + 13..AT + 17].copy_from_slice(&rel_to(GET_ADDR_PLT, AT + 13 + 4));
        assert_eq!(out, want, "{output:?}");
    }
    // Against the executable's own TLS it is refused (supervisor's ruling).
    let local = tables(TargetKind::Local, true);
    let s = scan(M2, &local, OutputKind::Pie, RO, &input, &relocs);
    assert_eq!(refused_name(&s.errors[0].1), UNRECOGNIZED_GD);
    // An unmatched LD is refused in both kinds.
    let mut ld = vec![0x48, 0x8d, 0x3d, 0, 0, 0, 0, 0xe8];
    ld.extend([0; 4]);
    let input = section(&ld);
    let relocs = [rel(AT + 3, SYM, 20, -4), rel(AT + 9, GET_ADDR, 4, -4)];
    for output in DYN {
        let s = scan(M2, &local, output, RO, &input, &relocs);
        assert_eq!(refused_name(&s.errors[0].1), UNRECOGNIZED_LD);
    }
}

#[test]
fn d48_exe_tls_relaxes_to_le_and_shared_ie_keeps_its_slot() {
    let local = tables(TargetKind::Local, true);
    let shared = tables(SHLIB_DATA, true);
    // `T = TpRel(0x3008) = 0x3008 − 0x3000 − 0x10 = −8`.
    let t = (-8i32).to_le_bytes();
    for output in DYN {
        for rex in [0x48u8, 0x4c] {
            for (op, out_op) in [(0x8b, 0xc7), (0x03, 0x81)] {
                let input = section(&[rex, op, 0x05 | (2 << 3), 0, 0, 0, 0]);
                let relocs = [rel(AT + 3, SYM, 22, -4)];
                // IE → LE against the executable's TLS (X-23).
                let rexp = if rex == 0x4c { 0x49 } else { 0x48 };
                let mut le = with(&input, AT, &[rexp, out_op, 0xc2]);
                le[AT + 3..AT + 7].copy_from_slice(&t);
                assert_eq!(case(&local, &input, output).apply(&relocs).unwrap(), le);
                assert!(scan(M2, &local, output, RO, &input, &relocs).raised.is_empty());
                // IE kept against a shared key (X-24): the slot, `TPOFF64`.
                let ie = with(&input, AT + 3, &rel_to(TPSLOT, AT + 7));
                assert_eq!(case(&shared, &input, output).apply(&relocs).unwrap(), ie);
                let s = scan(M2, &shared, output, RO, &input, &relocs);
                assert_eq!(s.raised, vec![(key(SYM), DemandFlags::GOT_TPREL)]);
                assert!(s.global.is_empty());
            }
        }
        // GD → LE and TLSDESC → LE against the executable's TLS (X-25,
        // X-28), in `exec` and `pie` alike.
        let input = section(&gd16(false));
        let relocs = [rel(AT + 4, SYM, 19, -4), rel(AT + 12, GET_ADDR, 4, -4)];
        let mut d16 = vec![0x64, 0x48, 0x8b, 0x04, 0x25, 0, 0, 0, 0, 0x48, 0x8d, 0x80];
        d16.extend(t);
        assert_eq!(case(&local, &input, output).apply(&relocs).unwrap(), with(&input, AT, &d16));
        // `TPOFF32` and `DTPOFF32` against a shared key: `E` (D-48).
        for r_type in [23, 21] {
            let input = section(&[0x48, 0x8b, 0x80]);
            let s = scan(M2, &shared, output, RO, &input, &[rel(AT + 3, SYM, r_type, 0)]);
            assert!(matches!(s.errors[0].1, ScanError::Incompatible(_)), "{r_type} {s:?}");
        }
    }
}

#[test]
fn d25_thread_local_weakdyn_is_refused() {
    let t = tables(TargetKind::WeakDyn, true);
    let input = section(&[0x48, 0x8b, 0x05, 0, 0, 0, 0]);
    for output in DYN {
        let s = scan(M2, &t, output, RO, &input, &[rel(AT + 3, SYM, 22, -4)]);
        assert_eq!(refused_name(&s.errors[0].1), TLS_WEAK_DYNAMIC);
    }
}

// ---------------------------------------------------------------- R-X1 GOT relaxation

#[test]
fn rx1_got_relaxation_scope() {
    let local = tables(TargetKind::Local, false);
    // G1 (mov → lea) in both kinds against a local target.
    let mov = section(&[0x48, 0x8b, 0x05, 0, 0, 0, 0]);
    let r = [rel(AT + 3, SYM, 42, -4)];
    let lea_field =
        ((0x2468u64).wrapping_sub(4).wrapping_sub(ADDR + AT as u64 + 3) as i32).to_le_bytes();
    for o in DYN {
        let out = case(&local, &mov, o).apply(&r).unwrap();
        assert_eq!(out[AT + 1], 0x8d, "{o:?}");
        assert_eq!(out[AT + 3..AT + 7], lea_field, "{o:?}");
    }
    // G5/G6 (immediates) in `exec` only (X-20).
    for (bytes, opc) in [([0x48, 0x03, 0x05], 0x81), ([0x48, 0x85, 0x05], 0xf7)] {
        let input = section(&{
            let mut b = bytes.to_vec();
            b.extend([0; 4]);
            b
        });
        let exec = case(&local, &input, OutputKind::Exec).apply(&r).unwrap();
        assert_eq!(exec[AT + 1], opc);
        assert_eq!(exec[AT + 3..AT + 7], 0x2468u32.to_le_bytes());
        let pie = case(&local, &input, OutputKind::Pie).apply(&r).unwrap();
        assert_eq!(pie[AT..AT + 3], bytes, "not in pie");
        assert_eq!(pie[AT + 3..AT + 7], rel_to(SLOT, AT + 7), "the GOT load stays");
    }
    // Never against a shared or `weakdyn` key: the GOT load stays.
    for kind in [SHLIB_DATA, SHLIB_FUNC, TargetKind::WeakDyn] {
        let t = tables(kind, false);
        for o in DYN {
            for (form, r_type) in
                [(&[0x48, 0x8b, 0x05][..], 42), (&[0xff, 0x15], 41), (&[0xff, 0x25], 41)]
            {
                let mut b = form.to_vec();
                b.extend([0; 4]);
                let input = section(&b);
                let p = AT + form.len();
                let out = case(&t, &input, o).apply(&[rel(p, SYM, r_type, -4)]).unwrap();
                assert_eq!(out, with(&input, p, &rel_to(SLOT, p + 4)), "{kind:?} {o:?} {r_type}");
            }
        }
    }
}

// ---------------------------------------------------------------- values

#[test]
fn values_against_shared_and_weakdyn_keys() {
    // X-57: a `PC32` on a call against a `weakdyn` key goes to its PLT.
    let mut w = tables(TargetKind::WeakDyn, false);
    w.flags.insert(key(SYM), DemandFlags::PLT);
    for b in [&[0xe8][..], &[0x0f, 0x84]] {
        let input = section(b);
        let p = AT + b.len();
        for o in DYN {
            let out = case(&w, &input, o).apply(&[rel(p, SYM, 2, -4)]).unwrap();
            assert_eq!(out, with(&input, p, &rel_to(PLT, p + 4)), "{b:x?}");
            let out = case(&w, &input, o).apply(&[rel(p, SYM, 4, -4)]).unwrap();
            assert_eq!(out, with(&input, p, &rel_to(PLT, p + 4)), "PLT32");
        }
    }
    // `64` against a shared or `weakdyn` key: the arch writes `S + A`
    // with `S = 0`. D-46's 0 at the place of the symbolic record is the
    // fill's (`mortar_image` `dynrel::zeroed_place`, from the roles).
    for kind in [SHLIB_DATA, SHLIB_FUNC, TargetKind::WeakDyn] {
        let t = tables(kind, false);
        let input = section(&[0xaa; 8]);
        for o in DYN {
            let out = case(&t, &input, o).apply(&[rel(AT, SYM, 1, 0x10)]).unwrap();
            assert_eq!(out, with(&input, AT, &0x10u64.to_le_bytes()), "{kind:?} {o:?}");
        }
    }
    // A copied (or canonical) key has an address here: `S + A`.
    let mut c = tables(SHLIB_DATA, false);
    c.syms.get_mut(&key(SYM)).unwrap().s = COPY_S;
    c.flags.insert(key(SYM), DemandFlags::COPY);
    let input = section(&[0; 8]);
    let out = case(&c, &input, OutputKind::Exec).apply(&[rel(AT, SYM, 1, 0x10)]).unwrap();
    assert_eq!(out, with(&input, AT, &(COPY_S + 0x10).to_le_bytes()));
    // `PC32` against a copy (D-43): `S + A − P` to the allocation.
    let input = section(&MOV);
    let out = case(&c, &input, OutputKind::Pie).apply(&[rel(AT + 2, SYM, 2, -4)]).unwrap();
    assert_eq!(out, with(&input, AT + 2, &rel_to(COPY_S, AT + 6)));
    // `PLTOFF64` against a shared function: `L + A − GOT`.
    let mut f = tables(SHLIB_FUNC, false);
    f.flags.insert(key(SYM), DemandFlags::PLT);
    let input = section(&[0; 8]);
    let out = case(&f, &input, OutputKind::Pie).apply(&[rel(AT, SYM, 31, 0)]).unwrap();
    assert_eq!(out, with(&input, AT, &(PLT.wrapping_sub(0x4800)).to_le_bytes()));
}

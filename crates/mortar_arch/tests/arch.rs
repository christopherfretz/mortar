//! Behavioural tests of `mortar_arch`, by section of `spec/arch.md`.

use mortar_arch::error::text;
use mortar_arch::seq::{field_window, overlaps};
use mortar_arch::vocab::verify_align;
use mortar_arch::{
    Base, Check, ClassOutcome, Code, Demand, DemandClass, DemandFlags, DynRole, FieldError,
    GlobalFlags, Incompatible, Minus, OutputKind, PlaceFacts, Proof, ProofRun, Range, RefName,
    TargetKind, Terms, TlsAbi, TlsBlock, TlsVariant, UnitWindow, Value, class_outcome, tombstone,
};
use mortar_elf::Class;

// ---------------------------------------------------------------- §2.2

#[test]
fn s2_2_value_page_minus_page_p() {
    // ADRP: Page(S + A) − Page(P).
    let v = Value { base: Base::S, add_a: true, page: true, negate: false, minus: Minus::PageP };
    let t = Terms { b: 0x41_2345, a: 0x10, p: 0x40_0ffc, got: 0 };
    assert_eq!(v.eval(t, 0xFFF), 0x41_2000 - 0x40_0000);
    // Negative distance.
    let t = Terms { b: 0x40_0000, a: -8, p: 0x41_0000, got: 0 };
    assert_eq!(v.eval(t, 0xFFF), 0x3F_F000 - 0x41_0000);
}

#[test]
fn s2_2_value_negate_and_got_base() {
    let v = Value { base: Base::S, add_a: true, page: false, negate: true, minus: Minus::GotBase };
    let t = Terms { b: 100, a: 5, p: 0, got: 40 };
    assert_eq!(v.eval(t, 0), -65);
    let v = Value { minus: Minus::PageGotBase, negate: false, ..v };
    let t = Terms { b: 0x2010, a: 0, p: 0, got: 0x1fff };
    assert_eq!(v.eval(t, 0xFFF), 0x2010 - 0x1000);
    assert_eq!(Value::of(Base::Size).eval(Terms { b: 24, a: 99, p: 7, got: 9 }, 0xFFF), 24);
}

#[test]
fn s2_2_value_wraps() {
    let v = Value { base: Base::S, add_a: true, page: false, negate: false, minus: Minus::P };
    let t = Terms { b: u64::MAX, a: 2, p: 0, got: 0 };
    assert_eq!(v.eval(t, 0), 1);
}

// ---------------------------------------------------------------- §2.3

#[test]
fn s2_3_check_bounds() {
    assert_eq!(Check::None.bounds(), None);
    assert_eq!(Check::Signed(21).bounds(), Some((-(1 << 20), (1 << 20) - 1)));
    assert_eq!(Check::Unsigned(12).bounds(), Some((0, 4095)));
    assert_eq!(Check::Either(16).bounds(), Some((-32768, 65535)));
    assert_eq!(Check::MovSigned(16).bounds(), Some((-65536, 65535)));
    assert_eq!(Check::Signed(64).bounds(), Some((i128::from(i64::MIN), i128::from(i64::MAX))));
}

#[test]
fn s2_3_elf64_just_outside_signed32_is_refused() {
    let c = Check::Signed(32);
    assert!(c.verify(0x7FFF_FFFF, Class::Elf64).is_ok());
    assert!(c.verify(-0x8000_0000, Class::Elf64).is_ok());
    assert_eq!(
        c.verify(0x8000_0000, Class::Elf64),
        Err(FieldError::Range(Range { value: 0x8000_0000, min: -0x8000_0000, max: 0x7FFF_FFFF }))
    );
    assert!(c.verify(-0x8000_0001, Class::Elf64).is_err());
}

#[test]
fn s2_3_elf32_wraparound_links_are_accepted() {
    // An absolute R_386_32 (`Either(32)`) whose S + A passes 2^32.
    assert!(Check::Either(32).verify(0x1_0000_0010, Class::Elf32).is_ok());
    assert!(Check::Either(32).verify(-0x1_2345_6789, Class::Elf32).is_ok());
    // A PC32 (`Signed(32)`) whose 64-bit distance is below −2^31 but whose
    // 32-bit reduction is in range: P high, S low, wrapping.
    let x = 0x10_i64 - 0xF000_0000;
    assert!(Check::Signed(32).verify(x, Class::Elf64).is_err());
    assert!(Check::Signed(32).verify(x, Class::Elf32).is_ok());
    // The reduced value is what the error carries.
    assert_eq!(
        Check::Signed(16).verify(0x1_0000_8000, Class::Elf32),
        Err(FieldError::Range(Range { value: 0x8000, min: -32768, max: 32767 }))
    );
    assert_eq!(
        Check::Unsigned(16).verify(-1, Class::Elf32),
        Err(FieldError::Range(Range { value: 0xFFFF_FFFF, min: 0, max: 65535 }))
    );
}

#[test]
fn s2_3_alignment() {
    assert!(verify_align(0x1008, 8).is_ok());
    assert!(verify_align(-16, 16).is_ok());
    assert_eq!(verify_align(0x1004, 8), Err(FieldError::Misaligned { value: 0x1004, align: 8 }));
    assert!(verify_align(3, 1).is_ok());
}

// ---------------------------------------------------------------- §4.3

const W: PlaceFacts = PlaceFacts { alloc: true, writable: true };
const RO: PlaceFacts = PlaceFacts { alloc: true, writable: false };
const NA: PlaceFacts = PlaceFacts { alloc: false, writable: false };

fn demand(flags: DemandFlags, global: GlobalFlags, dynamic: Option<DynRole>) -> ClassOutcome {
    ClassOutcome::Demand(Demand { flags, global, dynamic })
}
fn nothing() -> ClassOutcome {
    demand(DemandFlags::empty(), GlobalFlags::empty(), None)
}
fn fl(flags: DemandFlags) -> ClassOutcome {
    demand(flags, GlobalFlags::empty(), None)
}
fn inc(r: Incompatible) -> ClassOutcome {
    ClassOutcome::Incompatible(r)
}

const SHLIB_DATA: TargetKind = TargetKind::Shlib { func: false };
const SHLIB_FUNC: TargetKind = TargetKind::Shlib { func: true };

/// The table of §4.3, cell by cell, as the spec writes it (not derived from
/// the implementation's table).
#[test]
fn s4_3_class_table() {
    use DemandClass as K;
    use OutputKind::{Exec, Pie, Shared, Static};
    use TargetKind::{Abs, Ifunc, Local, Preemptible, Weak0, WeakDyn};
    let out = |k, o, t, p| class_outcome(k, true, o, t, p, false);
    let rel = Some(DynRole::Relative);
    let sym = Some(DynRole::Symbolic);
    let ex = DemandFlags::EXPORT;
    let no = GlobalFlags::empty();

    // AbsWord, Static/Exec.
    for o in [Static, Exec] {
        assert_eq!(out(K::AbsWord, o, Local, W), nothing());
        assert_eq!(out(K::AbsWord, o, Abs, RO), nothing());
        assert_eq!(out(K::AbsWord, o, Ifunc, RO), fl(DemandFlags::IPLT));
        assert_eq!(out(K::AbsWord, o, Weak0, RO), nothing());
    }
    assert_eq!(out(K::AbsWord, Exec, SHLIB_DATA, W), demand(ex, no, sym));
    assert_eq!(out(K::AbsWord, Exec, SHLIB_DATA, RO), fl(DemandFlags::COPY));
    assert_eq!(out(K::AbsWord, Exec, SHLIB_FUNC, RO), fl(DemandFlags::CANON_PLT));
    assert_eq!(out(K::AbsWord, Exec, WeakDyn, W), demand(ex, no, sym));
    assert_eq!(out(K::AbsWord, Exec, WeakDyn, RO), inc(Incompatible::RecompilePic));
    // AbsWord, Pie/Shared.
    assert_eq!(out(K::AbsWord, Pie, Local, W), demand(DemandFlags::empty(), no, rel));
    assert_eq!(out(K::AbsWord, Pie, Local, RO), inc(Incompatible::ReadOnly));
    assert_eq!(out(K::AbsWord, Shared, Abs, RO), nothing());
    assert_eq!(out(K::AbsWord, Shared, Preemptible, W), demand(ex, no, sym));
    assert_eq!(out(K::AbsWord, Shared, Preemptible, RO), inc(Incompatible::ReadOnly));
    assert_eq!(out(K::AbsWord, Pie, SHLIB_FUNC, W), demand(ex, no, sym));
    assert_eq!(out(K::AbsWord, Pie, Ifunc, W), demand(DemandFlags::IPLT, no, rel));
    assert_eq!(out(K::AbsWord, Pie, Ifunc, RO), inc(Incompatible::ReadOnly));
    assert_eq!(out(K::AbsWord, Pie, Weak0, RO), nothing());
    assert_eq!(out(K::AbsWord, Shared, WeakDyn, W), demand(ex, no, sym));

    // AbsNarrow.
    assert_eq!(out(K::AbsNarrow, Static, Local, RO), nothing());
    assert_eq!(out(K::AbsNarrow, Exec, SHLIB_DATA, RO), fl(DemandFlags::COPY));
    assert_eq!(out(K::AbsNarrow, Exec, Ifunc, RO), fl(DemandFlags::IPLT));
    assert_eq!(out(K::AbsNarrow, Exec, WeakDyn, W), inc(Incompatible::RecompilePic));
    assert_eq!(out(K::AbsNarrow, Pie, Local, W), inc(Incompatible::RecompilePie));
    assert_eq!(out(K::AbsNarrow, Shared, Local, W), inc(Incompatible::RecompilePic));
    assert_eq!(out(K::AbsNarrow, Pie, Abs, W), nothing());
    assert_eq!(out(K::AbsNarrow, Pie, Ifunc, W), inc(Incompatible::RecompilePie));
    assert_eq!(out(K::AbsNarrow, Pie, Weak0, W), nothing());

    // Pc.
    assert_eq!(out(K::Pc, Static, Abs, RO), nothing());
    assert_eq!(out(K::Pc, Exec, SHLIB_FUNC, RO), fl(DemandFlags::CANON_PLT));
    assert_eq!(out(K::Pc, Exec, WeakDyn, RO), inc(Incompatible::RecompilePic));
    assert_eq!(out(K::Pc, Pie, Local, RO), nothing());
    assert_eq!(out(K::Pc, Pie, Abs, RO), inc(Incompatible::RecompilePie));
    assert_eq!(out(K::Pc, Shared, Preemptible, RO), inc(Incompatible::RecompilePic));
    assert_eq!(out(K::Pc, Pie, Ifunc, RO), fl(DemandFlags::IPLT));
    assert_eq!(out(K::Pc, Pie, Weak0, RO), nothing());

    // Lo: as Pc, except Abs is always nothing.
    assert_eq!(out(K::Lo, Pie, Abs, RO), nothing());
    assert_eq!(out(K::Lo, Exec, SHLIB_DATA, RO), fl(DemandFlags::COPY));
    assert_eq!(out(K::Lo, Shared, Preemptible, RO), inc(Incompatible::RecompilePic));
    assert_eq!(out(K::Lo, Pie, SHLIB_DATA, RO), inc(Incompatible::RecompilePie));
    assert_eq!(out(K::Lo, Static, Ifunc, RO), fl(DemandFlags::IPLT));

    // Branch.
    assert_eq!(out(K::Branch, Static, Local, RO), nothing());
    assert_eq!(out(K::Branch, Static, Abs, RO), nothing());
    assert_eq!(out(K::Branch, Exec, SHLIB_FUNC, RO), fl(DemandFlags::PLT));
    assert_eq!(out(K::Branch, Exec, WeakDyn, RO), fl(DemandFlags::PLT));
    assert_eq!(out(K::Branch, Pie, Abs, RO), inc(Incompatible::RecompilePie));
    assert_eq!(out(K::Branch, Shared, Preemptible, RO), fl(DemandFlags::PLT));
    assert_eq!(out(K::Branch, Pie, Ifunc, RO), fl(DemandFlags::IPLT));
    assert_eq!(out(K::Branch, Shared, Weak0, RO), nothing());

    // Got, any.
    for o in [Static, Exec, Pie, Shared] {
        for t in [Local, Abs, Weak0] {
            assert_eq!(out(K::Got, o, t, RO), fl(DemandFlags::GOT), "{o:?} {t:?}");
        }
        assert_eq!(out(K::Got, o, Ifunc, RO), fl(DemandFlags::GOT | DemandFlags::IPLT));
    }
    assert_eq!(out(K::Got, Shared, Preemptible, RO), fl(DemandFlags::GOT));
    assert_eq!(out(K::Got, Exec, WeakDyn, RO), fl(DemandFlags::GOT));

    // GotRel.
    let gb = |f| demand(f, GlobalFlags::GOT_BASE, None);
    assert_eq!(out(K::GotRel, Pie, Local, RO), gb(DemandFlags::empty()));
    assert_eq!(out(K::GotRel, Static, Abs, RO), gb(DemandFlags::empty()));
    assert_eq!(out(K::GotRel, Exec, Abs, RO), gb(DemandFlags::empty()));
    assert_eq!(out(K::GotRel, Pie, Abs, RO), inc(Incompatible::RecompilePie));
    assert_eq!(out(K::GotRel, Shared, Preemptible, RO), inc(Incompatible::RecompilePic));
    assert_eq!(out(K::GotRel, Exec, SHLIB_DATA, RO), inc(Incompatible::RecompilePic));
    assert_eq!(out(K::GotRel, Shared, Ifunc, RO), gb(DemandFlags::IPLT));
    assert_eq!(out(K::GotRel, Static, Weak0, RO), inc(Incompatible::RecompilePic));

    // GotBaseRef, any.
    assert_eq!(out(K::GotBaseRef, Shared, Preemptible, RO), gb(DemandFlags::empty()));
    assert_eq!(out(K::GotBaseRef, Static, Weak0, RO), gb(DemandFlags::empty()));

    // Size.
    assert_eq!(out(K::Size, Exec, SHLIB_DATA, RO), nothing());
    assert_eq!(out(K::Size, Shared, Preemptible, RO), inc(Incompatible::RecompilePic));
    assert_eq!(out(K::Size, Exec, WeakDyn, RO), inc(Incompatible::RecompilePic));
    assert_eq!(out(K::Size, Static, Ifunc, RO), nothing());

    // TLS classes.
    let tls = |k, o, t| class_outcome(k, false, o, t, RO, true);
    for k in [K::TlsGd, K::TlsLd, K::TlsIe, K::TlsDesc] {
        assert_eq!(tls(k, Static, Local), ClassOutcome::ArchTls);
        assert_eq!(tls(k, Shared, Preemptible), ClassOutcome::ArchTls);
    }
    for o in [Static, Exec, Pie] {
        assert_eq!(tls(K::TlsLe, o, Local), nothing());
    }
    assert_eq!(tls(K::TlsLe, Exec, SHLIB_DATA), inc(Incompatible::RecompilePic));
    assert_eq!(tls(K::TlsLe, Pie, SHLIB_DATA), inc(Incompatible::RecompilePie));
    assert_eq!(tls(K::TlsLe, Shared, Local), inc(Incompatible::LocalExecInShared));
    assert_eq!(tls(K::TlsDtpRel, Shared, Preemptible), nothing());
    assert_eq!(tls(K::TlsDtpRel, Exec, SHLIB_DATA), inc(Incompatible::RecompilePic));

    // None.
    assert_eq!(out(K::None, Shared, Preemptible, RO), nothing());
}

#[test]
fn s4_3_impossible_and_undefined() {
    use DemandClass as K;
    for k in DemandClass::ALL {
        for o in [OutputKind::Static, OutputKind::Exec, OutputKind::Pie, OutputKind::Shared] {
            assert_eq!(
                class_outcome(k, true, o, TargetKind::Undefined, W, k.is_tls()),
                ClassOutcome::Undefined
            );
        }
        // Static never has Preemptible, Shlib or WeakDyn targets.
        if k != K::None {
            for t in [TargetKind::Preemptible, SHLIB_DATA, TargetKind::WeakDyn] {
                assert_eq!(
                    class_outcome(k, true, OutputKind::Static, t, W, k.is_tls()),
                    ClassOutcome::Impossible,
                    "{k:?} {t:?}"
                );
            }
        }
    }
}

#[test]
fn s4_3_tls_mismatch() {
    let o = OutputKind::Exec;
    let t = TargetKind::Local;
    assert_eq!(
        class_outcome(DemandClass::TlsLe, false, o, t, RO, false),
        inc(Incompatible::NotThreadLocal)
    );
    assert_eq!(
        class_outcome(DemandClass::Pc, false, o, t, RO, true),
        inc(Incompatible::ThreadLocal)
    );
    assert_eq!(
        class_outcome(DemandClass::Got, false, o, t, RO, true),
        inc(Incompatible::ThreadLocal)
    );
}

#[test]
fn s4_3_non_alloc_places() {
    use DemandClass as K;
    for o in [OutputKind::Static, OutputKind::Pie, OutputKind::Shared] {
        for k in [K::AbsWord, K::AbsNarrow, K::Pc, K::Size] {
            for t in [TargetKind::Local, TargetKind::Weak0, TargetKind::Ifunc] {
                assert_eq!(class_outcome(k, true, o, t, NA, false), nothing());
            }
        }
        assert_eq!(class_outcome(K::TlsDtpRel, true, o, TargetKind::Local, NA, true), nothing());
        assert_eq!(class_outcome(K::None, false, o, TargetKind::Local, NA, false), nothing());
        // An instruction form of an allowed class is not a data relocation.
        assert_eq!(
            class_outcome(K::Pc, false, o, TargetKind::Local, NA, false),
            inc(Incompatible::NotInNonAlloc)
        );
        for k in [K::Lo, K::Branch, K::Got, K::GotRel, K::TlsLe, K::TlsIe] {
            assert_eq!(
                class_outcome(k, true, o, TargetKind::Local, NA, k.is_tls()),
                inc(Incompatible::NotInNonAlloc),
                "{k:?}"
            );
        }
        // Undefined is still undefined-symbol.
        assert_eq!(
            class_outcome(K::AbsWord, true, o, TargetKind::Undefined, NA, false),
            ClassOutcome::Undefined
        );
    }
    // Shlib/Preemptible targets in a non-alloc place raise nothing.
    assert_eq!(
        class_outcome(K::AbsWord, true, OutputKind::Shared, TargetKind::Preemptible, NA, false),
        nothing()
    );
}

// ---------------------------------------------------------------- §3.4

fn w(start: u64, end: u64, unit: u32, record: u32) -> UnitWindow {
    UnitWindow { start, end, unit, record }
}

#[test]
fn s3_4_two_matched_sequences_sharing_a_byte_overlap() {
    // Sequence A (records 0, 1) writes [0, 12); sequence B (records 2, 3)
    // writes [11, 20).
    let mut ws = vec![w(0, 8, 0, 0), w(4, 12, 0, 1), w(11, 16, 2, 2), w(16, 20, 2, 3)];
    let mut out = Vec::new();
    overlaps(&mut ws, &mut out);
    assert_eq!(out, vec![(2, 1)]);
}

#[test]
fn s3_4_unmatched_sequence_is_not_an_overlap() {
    // An unmatched TLSGD + call: each record its own unit, ordinary
    // windows, disjoint.
    let mut ws = vec![w(3, 7, 0, 0), w(12, 16, 1, 1)];
    let mut out = Vec::new();
    overlaps(&mut ws, &mut out);
    assert!(out.is_empty());
    // Members of one unit are exempt from each other.
    let mut ws = vec![w(0, 16, 5, 5), w(4, 8, 5, 6), w(8, 12, 5, 7)];
    overlaps(&mut ws, &mut out);
    assert!(out.is_empty());
    // Empty windows occupy nothing.
    let mut ws = vec![w(0, 8, 0, 0), w(4, 4, 1, 1)];
    overlaps(&mut ws, &mut out);
    assert!(out.is_empty());
}

#[test]
fn s3_4_overlap_sees_past_a_same_unit_window() {
    // Unit 0 reaches 100; unit 1 reaches 50, then 60. A later window of
    // unit 0 at 58 must still see unit 1, behind unit 0's own window.
    let mut ws = vec![w(0, 100, 0, 0), w(10, 50, 1, 1), w(20, 60, 1, 2), w(58, 59, 0, 3)];
    let mut out = Vec::new();
    overlaps(&mut ws, &mut out);
    // Record 1 and 2 overlap unit 0; record 3 (unit 0) overlaps unit 1.
    assert_eq!(out, vec![(1, 0), (2, 0), (3, 2)]);
}

/// Brute force against the sweep on generated windows.
#[test]
fn s3_4_overlap_matches_brute_force() {
    let mut seed = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _ in 0..2000 {
        let n = (next() % 9) as u32;
        let mut ws: Vec<UnitWindow> = (0..n)
            .map(|i| {
                let start = next() % 40;
                let len = next() % 12;
                let unit = (next() % 4) as u32;
                w(start, start + len, unit, i)
            })
            .collect();
        let mut sorted = ws.clone();
        sorted.sort_unstable_by_key(|w| (w.start, w.record));
        let mut want = Vec::new();
        for (i, a) in sorted.iter().enumerate() {
            if a.start == a.end {
                continue;
            }
            let hit =
                sorted[..i].iter().any(|b| b.start != b.end && b.unit != a.unit && b.end > a.start);
            if hit {
                want.push(a.record);
            }
        }
        let mut out = Vec::new();
        overlaps(&mut ws, &mut out);
        let got: Vec<u32> = out.iter().map(|p| p.0).collect();
        assert_eq!(got, want, "{sorted:?}");
        for &(r, seen) in &out {
            let a = sorted.iter().find(|w| w.record == r).unwrap();
            let b = sorted.iter().find(|w| w.record == seen).unwrap();
            assert!(a.unit != b.unit && b.start <= a.start && b.end > a.start);
        }
    }
}

#[test]
fn s3_6_field_window() {
    use mortar_arch::{AddendRule, Field, Howto, Status, Window};
    let mut h = Howto {
        name: "T",
        value: Value::of(Base::S),
        field: Field::Insn { form: 0 },
        check: Check::None,
        align: 1,
        class: DemandClass::Pc,
        addend: AddendRule::Any,
        implicit: None,
        window: Window { before: 0, after: 0 },
        status: Status::M1,
    };
    assert_eq!(field_window(8, &h), Some((8, 12)));
    h.window = Window { before: 4, after: 8 };
    assert_eq!(field_window(8, &h), Some((4, 20)));
    assert_eq!(field_window(2, &h), None);
    h.field = Field::None;
    assert_eq!(field_window(8, &h), Some((4, 8)));
    h.field = Field::Data { bytes: 8 };
    h.window = Window::default();
    assert_eq!(field_window(u64::MAX - 3, &h), None);
}

// ---------------------------------------------------------------- §3.6 rule 1

fn run(start: u64, end: u64, proof: Proof, unit: u32) -> ProofRun {
    ProofRun { start, end, proof, unit, unit_offset: 0 }
}

#[test]
fn s3_6_word_proof() {
    let bytes = [0u8; 64];
    let runs = [
        run(0, 16, Proof::Insn, 0),
        run(16, 24, Proof::Data, 0),
        run(24, 32, Proof::Insn, 0),
        // 32..36: padding.
        run(36, 48, Proof::Unproved, 1),
        run(48, 56, Proof::Insn, 2),
        run(56, 64, Proof::Insn, 3),
    ];
    let code = Code { bytes: &bytes, addr: 0x1000, proof: &runs };
    assert_eq!(code.word_proof(0).0, Proof::Insn);
    assert_eq!(code.word_proof(12).0, Proof::Insn);
    assert_eq!(code.word_proof(14).0, Proof::Data); // straddles into $d
    assert_eq!(code.word_proof(16).0, Proof::Data);
    assert_eq!(code.word_proof(28).0, Proof::Insn);
    assert_eq!(code.word_proof(32).0, Proof::Data); // padding
    assert_eq!(code.word_proof(30).0, Proof::Data); // into padding
    assert_eq!(code.word_proof(40), (Proof::Unproved, Some(runs[3])));
    assert_eq!(code.word_proof(46), (Proof::Unproved, Some(runs[3])));
    // Straddling two $x runs of different units: not one $x range.
    assert_eq!(code.word_proof(54).0, Proof::Data);
    assert_eq!(code.word_proof(62).0, Proof::Data); // past the end
    assert_eq!(code.word(60), Some(0));
    assert_eq!(code.word(61), None);
    assert_eq!(code.word(u64::MAX), None);
}

// ---------------------------------------------------------------- §3.8, §3.11

#[test]
fn s3_8_tls_formulas() {
    let a = TlsAbi { variant: TlsVariant::I { tcb: 16 }, get_addr: b"", module_base: None };
    let tls = TlsBlock { start: 0x42_0000, size: 0x30, align: 64 };
    assert_eq!(a.tp_rel(0x42_0008, tls), 8 + 64);
    let tls8 = TlsBlock { align: 8, ..tls };
    assert_eq!(a.tp_rel(0x42_0008, tls8), 8 + 16);
    let b = TlsAbi { variant: TlsVariant::II, get_addr: b"", module_base: None };
    assert_eq!(b.tp_rel(0x42_0008, tls8), 8 - 0x30);
    assert_eq!(TlsAbi::dtp_rel(0x42_0010, tls), 0x10);
}

#[test]
fn s3_11_tombstone() {
    assert_eq!(tombstone(b".debug_ranges"), 1);
    assert_eq!(tombstone(b".debug_loc"), 1);
    assert_eq!(tombstone(b".debug_info"), 0);
    assert_eq!(tombstone(b".debug_rnglists"), 0);
}

// ---------------------------------------------------------------- §7, §10

#[test]
fn s10_diagnostic_texts() {
    assert_eq!(
        text::incompatible("R_AARCH64_ADR_PREL_PG_HI21", "foo", Incompatible::RecompilePic),
        "relocation R_AARCH64_ADR_PREL_PG_HI21 cannot be used against symbol 'foo'; recompile with -fPIC"
    );
    let texts: Vec<&str> = Incompatible::ALL.iter().map(Incompatible::text).collect();
    assert_eq!(
        texts,
        [
            "recompile with -fPIC",
            "recompile with -fPIE",
            "it is in a read-only section",
            "the addend must be 0",
            "it is not thread-local",
            "it is thread-local",
            "local-exec TLS in a shared object",
            "the instruction does not match the relocation",
            "not allowed in a non-allocated section",
        ]
    );
    assert_eq!(
        text::misaligned("R_AARCH64_LDST64_ABS_LO12_NC", 0x1004, 8),
        "improper alignment for relocation R_AARCH64_LDST64_ABS_LO12_NC: 4100 is not aligned to 8 bytes"
    );
    assert_eq!(
        text::out_of_range(
            "R_AARCH64_CALL26",
            Range { value: 134_217_728, min: -134_217_728, max: 134_217_727 },
            "far"
        ),
        "relocation R_AARCH64_CALL26 out of range: 134217728 is not in [-134217728, 134217727]; references 'far'"
    );
    assert_eq!(RefName::Hex(0x1234).to_string(), "0x1234");
    assert_eq!(RefName::Named("R_AARCH64_P32_ABS32").to_string(), "R_AARCH64_P32_ABS32");
}

#[test]
fn s3_4_diag_keys() {
    use mortar_core::FileId;
    let k = mortar_arch::cx::scan_key(FileId(3), 7, 9);
    assert_eq!(k, mortar_diag::Key::new(70, 3, (7 << 32) | 9));
    let k = mortar_arch::cx::fill_key(FileId(3), 7, 9);
    assert_eq!(k, mortar_diag::Key::new(100, 3, (7 << 32) | 9));
}

#[test]
fn s4_2_flags() {
    assert_eq!(DemandFlags::from_bits(1 << 11), None);
    assert_eq!(DemandFlags::from_bits(0x7FF).map(DemandFlags::bits), Some(0x7FF));
    assert_eq!(format!("{:?}", DemandFlags::GOT | DemandFlags::IPLT), "{GOT, IPLT}");
    assert!(DemandFlags::CROSS_FILE.contains(DemandFlags::GOT_NORELAX));
}

//! AArch64 in dynamic executables (`spec/dyn.md`; a64.md §5, §6.2, §7.1,
//! §8, §10.2, §10.3): the R-A6 hooks, the demand cells of `exec` and
//! `pie` with each refusal's name, D-25's class table (`weak-class-table`
//! at the unit level), the static content of a symbolic place, the TLS
//! forms against a shared object's TLS, and 843419 with PLT-bound
//! ADRPs. The comparisons with GNU ld and lld are `dynamic_ref.rs`.

#![allow(
    clippy::many_single_char_names,
    clippy::match_same_arms,
    clippy::too_many_lines,
    clippy::cast_lossless
)]

use mortar_a64::{A64, DT_AARCH64_BTI_PLT, DT_AARCH64_VARIANT_PCS, erratum, forms, synth, table};
use mortar_arch::{
    Arch, Code, DemandClass, DemandFlags, DynRefusal, DynRole, DynTag, Field, GlobalFlags,
    LazyTarget, OutputCx, OutputKind, PatchCx, PlaceFacts, PltPart, PltVariant, Proof, ProofRun,
    Reloc, ScanError, SlotKind, Status, SynthCx, TagWhen, TargetKind, WeakClass,
};
use mortar_core::SymKey;

mod support;
use support::{Case, M2, Sink, Tables, key, raised, scan_all, scan_one, words};

const ALL_KINDS: [OutputKind; 5] = OutputKind::ALL;

fn ocx(output: OutputKind, feature_1_and: u32, bind_now: bool) -> OutputCx {
    OutputCx { output, feature_1_and, pac_plt: false, force_bti: false, bind_now }
}

// ---------------------------------------------------------------- R-A6

/// `spec/dyn.md` D-49, a64.md §10.2: the lazy target is the PLT header in
/// both shapes mortar writes; no other variant is AArch64's.
#[test]
fn d49_lazy_target_is_plt0() {
    for a in [A64::M1, M2] {
        assert_eq!(a.lazy_target(PltVariant::Plain), Some(LazyTarget::Plt0));
        assert_eq!(a.lazy_target(PltVariant::Bti), Some(LazyTarget::Plt0));
        for v in [
            PltVariant::Pac,
            PltVariant::BtiPac,
            PltVariant::Lazy,
            PltVariant::Ibt,
            PltVariant::Abs,
            PltVariant::Pic,
            PltVariant::IbtAbs,
            PltVariant::IbtPic,
        ] {
            assert_eq!(a.lazy_target(v), None, "{v:?}");
        }
    }
}

/// `spec/dyn.md` D-30 rows 24-25 (a64.md §10.3): the two tags, in that
/// order, each with its condition, in every output with `.dynamic`; none
/// in a static output.
#[test]
fn d30_dyn_tags() {
    let want = [
        DynTag { tag: 0x7000_0001, value: 0, when: TagWhen::BtiProperty },
        DynTag { tag: 0x7000_0005, value: 0, when: TagWhen::VariantPcsJumpSlot },
    ];
    assert_eq!((DT_AARCH64_BTI_PLT, DT_AARCH64_VARIANT_PCS), (0x7000_0001, 0x7000_0005));
    for out in ALL_KINDS {
        for (f, now) in [(0, false), (1, true), (3, false)] {
            let tags = M2.dyn_tags(&ocx(out, f, now));
            match out {
                OutputKind::Exec | OutputKind::Pie | OutputKind::Shared => {
                    assert_eq!(tags, want, "{out:?}");
                }
                OutputKind::Static | OutputKind::Relocatable => assert!(tags.is_empty()),
            }
        }
    }
    // Never DT_AARCH64_PAC_PLT (D-30: `-z pac-plt` stays refused).
    assert!(want.iter().all(|t| t.tag != 0x7000_0003));
}

/// a64.md §10.3 and the scoped BTI rule: the BTI shape exactly when the
/// merged `FEATURE_1_AND` has the BTI bit (Debian's crt files carry BTI
/// and PAC: an all-BTI link gets `bti c` in PLT0 and every entry); the PAC
/// bit alone, or GCS, changes nothing; `-z now` keeps the shapes (D-49);
/// `-z pac-plt` and `-z force-bti` are refused by name.
#[test]
fn s10_3_plt_variant_scope() {
    for out in [OutputKind::Exec, OutputKind::Pie] {
        for now in [false, true] {
            for (f, v) in [
                (0, PltVariant::Plain),
                (1, PltVariant::Bti),
                (2, PltVariant::Plain),
                (3, PltVariant::Bti),
                (4, PltVariant::Plain),
                (7, PltVariant::Bti),
            ] {
                assert_eq!(M2.plt_variant(&ocx(out, f, now)), Ok(v), "{out:?} {f} {now}");
            }
            let pac = OutputCx { pac_plt: true, ..ocx(out, 3, now) };
            let r = M2.plt_variant(&pac).unwrap_err();
            assert_eq!(r.name.to_string(), "-z pac-plt");
            let force = OutputCx { force_bti: true, ..ocx(out, 0, now) };
            assert_eq!(M2.plt_variant(&force).unwrap_err().name.to_string(), "-z force-bti");
        }
    }
    // The first word of a BTI entry is a landing pad for `br x16` (a
    // thunk to a PLT entry, §9) and for `blr`.
    let cx = SynthCx { addr: 0x1_0020, header: 0x1_0000, got_plt: 0x3_0018, got_base: 0, index: 0 };
    let mut e = [0u8; 24];
    M2.write_plt(PltVariant::Bti, PltPart::Entry { chunk: 0, n: 0 }, &cx, &mut e).unwrap();
    assert!(synth::is_landing_pad(u32::from_le_bytes(e[..4].try_into().unwrap())));
}

// ---------------------------------------------------------------- D-25

/// The M1 and M2 types of §6.1 (`classify` accepts them at M2).
fn m1_m2_types() -> Vec<u32> {
    (0..=mortar_a64::MAX_TYPE)
        .filter(|&t| table::howto(t).is_some_and(|h| matches!(h.status, Status::M1 | Status::M2)))
        .collect()
}

/// `weak-class-table` (`spec/dyn.md` §16, D-25; a64.md §7.1): every type
/// of the M1 and M2 sets in exactly one class, transcribed from the
/// table's names, and every other number unfillable.
#[test]
fn d25_weak_class_table() {
    use mortar_elf::reloc::aarch64::name;
    let got = [
        "GOTPCREL32",
        "GOT_LD_PREL19",
        "LD64_GOTOFF_LO15",
        "ADR_GOT_PAGE",
        "LD64_GOT_LO12_NC",
        "LD64_GOTPAGE_LO15",
    ];
    let branch = ["CALL26", "JUMP26", "PLT32"];
    let types = m1_m2_types();
    let mut seen = [0u32; 5];
    for &t in &types {
        let n = name(t).unwrap_or_else(|| panic!("type {t} has no name"));
        let short = n.strip_prefix("R_AARCH64_").unwrap();
        for writable in [false, true] {
            let place = [0u8; 16];
            let c = M2.weak_use(t, &place, 0, writable);
            let want = if short == "NONE" {
                None
            } else if got.contains(&short) {
                Some(WeakClass::Got)
            } else if branch.contains(&short) {
                Some(WeakClass::Branch)
            } else if short == "ABS64" && writable {
                Some(WeakClass::AbsWord)
            } else {
                // §7.1's unfillable row: ABS64 read-only, ABS32/16,
                // PREL*, MOVW_*, LD_PREL_LO19, ADR_*, the LO12 group,
                // CONDBR19, TSTBR14, GOTREL*, every TLS type; and the
                // withdrawn 256, which §7.1 does not list.
                Some(WeakClass::Unfillable)
            };
            assert_eq!(c, want, "{n} writable={writable}");
            seen[match c {
                None => 0,
                Some(WeakClass::Got) => 1,
                Some(WeakClass::AbsWord) => 2,
                Some(WeakClass::Branch) => 3,
                Some(WeakClass::Unfillable) => 4,
            }] += 1;
        }
        // Every TLS type, CONDBR19 and TSTBR14 are unfillable, writable or not.
        let h = table::howto(t).unwrap();
        if h.class.is_tls() || matches!(t, 279 | 280) {
            assert_eq!(M2.weak_use(t, &[0; 16], 0, true), Some(WeakClass::Unfillable), "{n}");
        }
    }
    // Every name of §7.1's three filled rows is an M1 or M2 type.
    for n in got.iter().chain(&branch).chain(&["ABS64"]) {
        assert!(types.iter().any(|&t| name(t) == Some(&format!("R_AARCH64_{n}"))), "{n}");
    }
    // NONE once per writability; GOT 6×2; ABS64 once; branches 3×2.
    assert_eq!(seen[..4], [2, 12, 1, 6]);
    // Numbers outside the M1 and M2 sets are unfillable (the `Later`
    // MOVW_GOTOFF group, the dynamic types, unknown numbers).
    for t in [300, 306, 512, 517, 1024, 1026, 1028, 1032, 0xE000, 0xFFFF] {
        assert_eq!(M2.weak_use(t, &[], 0, true), Some(WeakClass::Unfillable), "{t}");
    }
}

// ---------------------------------------------------------------- §5

/// What a64.md §5's table gives one relocation in `exec` or `pie`, at an
/// alloc place: demanded flags and the dynamic record, an `E`, or a
/// dynamic refusal by name. Transcribed from §5 (with D-43, D-44, D-47),
/// not from the code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Want {
    Demand(DemandFlags, GlobalFlags, Option<DynRole>),
    E,
    Refused(DynRefusal),
    /// `refused`, feature [`mortar_a64::LARGE_TLSDESC_DYNAMIC`].
    Feature,
}

fn want(class: DemandClass, out: OutputKind, kind: TargetKind, writable: bool) -> Want {
    use DemandClass as C;
    use TargetKind as K;
    let n = DemandFlags::empty();
    let g0 = GlobalFlags::empty();
    let d = |f: DemandFlags| Want::Demand(f, g0, None);
    let pie = out == OutputKind::Pie;
    let shlib_c = |func: bool| d(if func { DemandFlags::CANON_PLT } else { DemandFlags::COPY });
    // `TX`: the record in a read-only place is a text relocation (D-47).
    let tx = |f: DemandFlags, role: DynRole| {
        if writable {
            Want::Demand(f, g0, Some(role))
        } else {
            Want::Refused(DynRefusal::TextRelocation)
        }
    };
    let sym = DynRole::Symbolic;
    match (class, kind) {
        (C::AbsWord, K::Local) if pie => tx(n, DynRole::Relative),
        (C::AbsWord, K::Local | K::Abs | K::Weak0) => d(n),
        (C::AbsWord, K::Shlib { func }) if !pie => {
            if writable {
                Want::Demand(DemandFlags::EXPORT, g0, Some(sym))
            } else {
                shlib_c(func)
            }
        }
        (C::AbsWord, K::Shlib { .. }) => tx(DemandFlags::EXPORT, sym),
        (C::AbsWord, K::Ifunc) if pie => tx(DemandFlags::IPLT, DynRole::Relative),
        (C::AbsWord, K::Ifunc) => d(DemandFlags::IPLT),
        (C::AbsWord, K::WeakDyn) if !pie && !writable => Want::E,
        (C::AbsWord, K::WeakDyn) => tx(DemandFlags::EXPORT, sym),
        (C::AbsNarrow, K::Abs | K::Weak0) => d(n),
        // D-44 (F-6): a non-GOT address use of a shared function with no
        // record of its own is `canonical-plt-in-pie`, before its width.
        (C::AbsNarrow, K::Shlib { func: true }) if pie => {
            Want::Refused(DynRefusal::CanonicalPltInPie)
        }
        (C::AbsNarrow, _) if pie => Want::E,
        (C::AbsNarrow, K::Local) => d(n),
        (C::AbsNarrow, K::Shlib { func }) => shlib_c(func),
        (C::AbsNarrow, K::Ifunc) => d(DemandFlags::IPLT),
        (C::AbsNarrow, _) => Want::E,
        // `LO` is `PC` except `abs`, which is `—` everywhere.
        (C::Pc | C::Lo, K::Local | K::Weak0) => d(n),
        (C::Pc | C::Lo, K::Abs) if class == C::Lo || !pie => d(n),
        (C::Pc | C::Lo, K::Abs) => Want::E,
        (C::Pc | C::Lo, K::Shlib { func: true }) if pie => {
            Want::Refused(DynRefusal::CanonicalPltInPie)
        }
        (C::Pc | C::Lo, K::Shlib { func }) => shlib_c(func),
        (C::Pc | C::Lo, K::Ifunc) => d(DemandFlags::IPLT),
        (C::Pc | C::Lo, _) => Want::E,
        (C::Branch, K::Local | K::Weak0) => d(n),
        (C::Branch, K::Abs) if pie => Want::E,
        (C::Branch, K::Abs) => d(n),
        (C::Branch, K::Shlib { .. } | K::WeakDyn) => d(DemandFlags::PLT),
        (C::Branch, K::Ifunc) => d(DemandFlags::IPLT),
        (C::Got, K::Ifunc) => d(DemandFlags::GOT | DemandFlags::IPLT),
        (C::Got, _) => d(DemandFlags::GOT),
        (C::GotRel, K::Local) => Want::Demand(n, GlobalFlags::GOT_BASE, None),
        (C::GotRel, K::Abs) if !pie => Want::Demand(n, GlobalFlags::GOT_BASE, None),
        (C::GotRel, K::Ifunc) => Want::Demand(DemandFlags::IPLT, GlobalFlags::GOT_BASE, None),
        (C::GotRel, _) => Want::E,
        (C::None, _) => d(n),
        _ => unreachable!("{class:?} {kind:?}"),
    }
}

/// A word each instruction form requires (the first one found from a
/// fixed seed), so that scan's instruction check passes.
fn word_for(form: u16) -> u32 {
    let mut x: u32 = 0x9E37_79B9;
    loop {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        if forms::requires(form, x) {
            return x;
        }
    }
}

fn input_for(t: u32) -> Vec<u8> {
    let h = table::howto(t).unwrap();
    match h.field {
        Field::Insn { form } => words(&[word_for(form), synth::NOP]),
        _ => vec![0; 8],
    }
}

const TARGETS: [TargetKind; 7] = [
    TargetKind::Local,
    TargetKind::Abs,
    TargetKind::Shlib { func: false },
    TargetKind::Shlib { func: true },
    TargetKind::Ifunc,
    TargetKind::Weak0,
    TargetKind::WeakDyn,
];

fn got(s: &Sink) -> Want {
    // The R3/R5 inhibitors are input-only and not §5's.
    let inhib = DemandFlags::GOT_NORELAX.bits() | DemandFlags::TLSIE_NORELAX.bits();
    let flags = DemandFlags::from_bits(raised(s, key(1)).bits() & !inhib).unwrap();
    if let [(_, e)] = s.errors[..] {
        return match e {
            ScanError::Dynamic(r) => Want::Refused(r),
            // D-47: the scan driver turns this reason into
            // `text-relocation` in an output with `.dynamic`
            // (`mortar_scan` `dynamic_error`); the architecture reports
            // the generic `TX` reason.
            ScanError::Incompatible(mortar_arch::Incompatible::ReadOnly) => {
                Want::Refused(DynRefusal::TextRelocation)
            }
            ScanError::Incompatible(_) => Want::E,
            ScanError::Refused(ref r)
                if r.name.to_string() == mortar_a64::LARGE_TLSDESC_DYNAMIC =>
            {
                Want::Feature
            }
            other => panic!("{other:?}"),
        };
    }
    assert!(s.errors.is_empty(), "{:?}", s.errors);
    let role = match s.counts[..] {
        [] => None,
        [(r, 1)] => Some(r),
        ref c => panic!("{c:?}"),
    };
    Want::Demand(flags, s.global, role)
}

/// a64.md §5 for `exec` and `pie`: every non-TLS class (every M1 and M2
/// type of it) × every target kind × writable or read-only place, with
/// each refusal's name (`text-relocation`, `canonical-plt-in-pie`) and
/// `relocation-incompatible` for the `E` cells.
#[test]
fn s5_demand_cells_exec_and_pie() {
    let mut cells = 0;
    let mut by: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
    for t in m1_m2_types() {
        let h = table::howto(t).unwrap();
        if h.class.is_tls() {
            continue;
        }
        let input = input_for(t);
        for out in [OutputKind::Exec, OutputKind::Pie] {
            for kind in TARGETS {
                for writable in [false, true] {
                    let mut tables = Tables::default();
                    tables.sym(key(1), if kind.is_defined_here() { 0x41_0000 } else { 0 }, kind);
                    let place = PlaceFacts { alloc: true, writable, exec: Some(!writable) };
                    let r = Reloc { r_offset: 0, key: key(1), r_type: t, addend: 0 };
                    let s = scan_one(M2, &tables, out, place, &input, r).unwrap();
                    // `spec/dyn.md` D-25 (WP-155): in `Exec` an unfillable
                    // use of a `WeakDyn` key (a split key) is `Weak0`'s cell.
                    let split = out == OutputKind::Exec
                        && kind == TargetKind::WeakDyn
                        && mortar_a64::weak::weak_class(t, writable)
                            == Some(mortar_arch::WeakClass::Unfillable);
                    let w = want(h.class, out, if split { TargetKind::Weak0 } else { kind }, writable);
                    assert_eq!(got(&s), w, "{} {out:?} {kind:?} writable={writable}", h.name);
                    *by.entry(format!("{w:?}")).or_default() += 1;
                    cells += 1;
                }
            }
        }
    }
    assert_eq!(cells, 1344);
    // Each refusal by name is reached.
    assert!(by.keys().any(|k| k.contains("TextRelocation")));
    assert!(by.keys().any(|k| k.contains("CanonicalPltInPie")));
    assert!(by.keys().any(|k| k.contains("COPY")));
    assert!(by.keys().any(|k| k.contains("CANON_PLT")));
}

/// A non-alloc place keeps the static rule in `exec` and `pie` (a64.md §5
/// "The table is for places in `SHF_ALLOC` sections"): an `ABS64` in
/// `.debug_info` against a shared function demands nothing and is no text
/// relocation.
#[test]
fn s5_non_alloc_places_are_not_dynamic() {
    let mut tables = Tables::default();
    tables.sym(key(1), 0, TargetKind::Shlib { func: true });
    let place = PlaceFacts { alloc: false, writable: false, exec: Some(false) };
    for out in [OutputKind::Exec, OutputKind::Pie] {
        let r = Reloc { r_offset: 0, key: key(1), r_type: 257, addend: 0 };
        let s = scan_one(M2, &tables, out, place, &[0; 8], r).unwrap();
        assert!(s.errors.is_empty() && s.counts.is_empty(), "{s:?}");
        assert_eq!(raised(&s, key(1)), DemandFlags::empty());
    }
}

/// The TLS classes in `exec` and `pie` (a64.md §8, §10.1; `spec/dyn.md`
/// D-48): against the executable's own TLS every model relaxes (R1, R3
/// is kind 2 and keeps the slot, R4 with its pattern); against a shared
/// object's TLS, IE (`GOT_TPREL`), TLSDESC relaxes to IE except the large
/// form (a `TLS_DESC` descriptor), and GD without R4's pattern keeps its
/// pair (`TLS_GD`: `DTPMOD64` + `DTPREL64`, R-A5).
#[test]
fn s8_tls_cells_exec_and_pie() {
    use TargetKind as K;
    let tp = DemandFlags::GOT_TPREL;
    for t in m1_m2_types() {
        let h = table::howto(t).unwrap();
        if !h.class.is_tls() {
            continue;
        }
        let input = input_for(t);
        for out in [OutputKind::Exec, OutputKind::Pie] {
            for kind in [K::Local, K::Shlib { func: false }] {
                let mut tables = Tables::default();
                tables.tls_sym(key(1), if kind == K::Local { 0x10 } else { 0 }, kind);
                let place = PlaceFacts { alloc: true, writable: false, exec: Some(true) };
                let r = Reloc { r_offset: 0, key: key(1), r_type: t, addend: 0 };
                let s = scan_one(M2, &tables, out, place, &input, r).unwrap();
                let local = kind == K::Local;
                let w = match h.class {
                    DemandClass::TlsDesc if local => {
                        Want::Demand(DemandFlags::empty(), GlobalFlags::empty(), None)
                    }
                    // A lone `TLSDESC_ADD` has no `CALL` after it.
                    DemandClass::TlsDesc if t == 568 => Want::Feature,
                    DemandClass::TlsDesc if matches!(t, 565..=567) => {
                        Want::Demand(DemandFlags::TLS_DESC, GlobalFlags::empty(), None)
                    }
                    DemandClass::TlsDesc | DemandClass::TlsIe => {
                        Want::Demand(tp, GlobalFlags::empty(), None)
                    }
                    // No R4 pattern in this input: the pair stays.
                    DemandClass::TlsGd => {
                        Want::Demand(DemandFlags::TLS_GD, GlobalFlags::empty(), None)
                    }
                    DemandClass::TlsLe | DemandClass::TlsDtpRel if local => {
                        Want::Demand(DemandFlags::empty(), GlobalFlags::empty(), None)
                    }
                    DemandClass::TlsLe | DemandClass::TlsDtpRel => Want::E,
                    c => unreachable!("{c:?}"),
                };
                assert_eq!(got(&s), w, "{} {out:?} {kind:?}", h.name);
            }
        }
    }
}

// ---------------------------------------------------------------- fill

/// `spec/dyn.md` D-46, a64.md §6.2: the place of a symbolic `ABS64`
/// record holds 0 (RELA); a `RELATIVE` place `S + A`; a `weak0` word `A`;
/// a copy's or a canonical PLT's word the address of the copy or entry.
#[test]
fn d46_absolute_word_place_contents() {
    let mut t = Tables::default();
    t.sym(key(1), 0, TargetKind::Shlib { func: false });
    t.sym(key(2), 0, TargetKind::WeakDyn);
    t.sym(key(3), 0x41_2340, TargetKind::Local);
    t.sym(key(4), 0, TargetKind::Weak0);
    // A copied datum (its `S` is the copy) and a canonical PLT entry.
    t.sym(key(5), 0x42_0000, TargetKind::Shlib { func: false });
    t.flags.insert(key(5), DemandFlags::COPY);
    t.sym(key(6), 0x40_1030, TargetKind::Shlib { func: true });
    t.flags.insert(key(6), DemandFlags::CANON_PLT);
    let input = [0xAAu8; 8];
    let word = |out: OutputKind, k: SymKey, alloc: bool| {
        let mut c = Case::new(&t, &input, 0x43_0000);
        c.arch = M2;
        c.output = out;
        c.alloc = alloc;
        c.name = if alloc { b".data" } else { b".debug_info" };
        let b = c.apply(&[Reloc { r_offset: 0, key: k, r_type: 257, addend: 0x10 }]).unwrap();
        u64::from_le_bytes(b.try_into().unwrap())
    };
    for out in [OutputKind::Exec, OutputKind::Pie] {
        assert_eq!(word(out, key(1), true), 0, "{out:?} shlib: symbolic");
        assert_eq!(word(out, key(2), true), 0, "{out:?} weakdyn: symbolic");
        assert_eq!(word(out, key(3), true), 0x41_2350, "{out:?} local: value / RELATIVE");
        assert_eq!(word(out, key(4), true), 0x10, "{out:?} weak0: A");
        // Not loaded: the static value.
        assert_eq!(word(out, key(1), false), 0x10, "{out:?} debug");
    }
    // `pie`: every alloc ABS64 against `shlib` is a symbolic record.
    assert_eq!(word(OutputKind::Pie, key(5), true), 0);
    // `exec`, read-only: `C`, the copy's and the entry's addresses.
    assert_eq!(word(OutputKind::Exec, key(5), true), 0x42_0010);
    assert_eq!(word(OutputKind::Exec, key(6), true), 0x40_1040);
    // Static: unchanged (M1).
    assert_eq!(word(OutputKind::Static, key(3), true), 0x41_2350);
}

/// Branches against `shlib` and `weakdyn` go to the PLT entry (a64.md §5,
/// §7.1); against `weak0` they are `X = 4` (§7) in every output kind;
/// `PLT32` takes the entry too. `ADRP`/`ADD` against `weak0` in `exec` are
/// §7's (the instruction's page, `S = 0`), as in a static link.
#[test]
fn s7_branches_and_weak0_in_dynamic_outputs() {
    let mut t = Tables::default();
    t.sym(key(1), 0, TargetKind::Shlib { func: true });
    t.sym(key(2), 0, TargetKind::WeakDyn);
    t.sym(key(3), 0, TargetKind::Weak0);
    for k in [key(1), key(2)] {
        t.plts.insert(k, 0x40_0230);
        t.flags.insert(k, DemandFlags::PLT);
    }
    let bl = words(&[0x9400_0000, synth::NOP]);
    for out in [OutputKind::Exec, OutputKind::Pie] {
        let mut c = Case::new(&t, &bl, 0x40_1000);
        c.arch = M2;
        c.output = out;
        for k in [key(1), key(2)] {
            let w = c.word(283, 0, k, 0).unwrap();
            assert_eq!(w, 0x9400_0000 | ((0x40_0230u32.wrapping_sub(0x40_1000) >> 2) & 0x3FF_FFFF));
        }
        assert_eq!(c.word(283, 0, key(3), 0).unwrap(), 0x9400_0001, "X = 4");
        let data = [0u8; 8];
        let mut c = Case::new(&t, &data, 0x40_2000);
        c.arch = M2;
        c.output = out;
        assert_eq!(c.word(314, 0, key(2), 0).unwrap(), 0x40_0230u32.wrapping_sub(0x40_2000));
    }
    // `exec`, §7: ADRP X = Page(P + A) − Page(P), ADD S = 0.
    let code = words(&[0x9000_0000, 0x9100_0000]);
    let mut c = Case::new(&t, &code, 0x40_3ffc);
    c.arch = M2;
    c.output = OutputKind::Exec;
    let adrp = c.word(275, 0, key(3), 8).unwrap();
    assert_eq!(forms::extract(forms::ADRP, adrp), 0x1000);
    assert_eq!(c.word(277, 4, key(3), 8).unwrap(), 0x9100_0000 | 8 << 10);
}

/// R5 (§8.5) relaxes a GOT load only for a `local` target: never for
/// `weakdyn` or `shlib` (D-25 "R5 is off for a `weakdyn` key"), in `exec`
/// and in `pie` (D-46's scope).
#[test]
fn s8_5_r5_scope_in_dynamic_outputs() {
    let code = words(&[0x9000_0000, 0xF940_0000]);
    let relocs = [
        Reloc { r_offset: 0, key: key(1), r_type: 311, addend: 0 },
        Reloc { r_offset: 4, key: key(1), r_type: 312, addend: 0 },
    ];
    for (kind, relaxed) in [
        (TargetKind::Local, true),
        (TargetKind::WeakDyn, false),
        (TargetKind::Shlib { func: false }, false),
    ] {
        let mut t = Tables::default();
        t.sym(key(1), if relaxed { 0x41_0010 } else { 0 }, kind);
        t.slots.insert((key(1), SlotKind::Addr), 0x42_0008);
        for out in [OutputKind::Exec, OutputKind::Pie] {
            let mut c = Case::new(&t, &code, 0x40_0000);
            c.arch = M2;
            c.output = out;
            let b = c.apply(&relocs).unwrap();
            let w1 = u32::from_le_bytes(b[4..8].try_into().unwrap());
            if relaxed {
                assert_eq!(w1, 0x9100_0000 | 0x10 << 10, "{kind:?} {out:?}");
            } else {
                assert_eq!(w1, 0xF940_0000 | (8 >> 3) << 10, "{kind:?} {out:?}");
            }
        }
    }
}

/// TLS against a shared object's TLS in `exec` and `pie` (a64.md §8.2,
/// §8.4; `spec/dyn.md` D-48): TLSDESC → IE (small and tiny), R4's GD → IE,
/// IE kept; the large TLSDESC and an unpatterned GD stay (their slots).
#[test]
fn d48_tls_against_a_shared_definition() {
    let mut t = Tables::default();
    t.tls_sym(key(1), 0, TargetKind::Shlib { func: false });
    t.sym(key(2), 0, TargetKind::Shlib { func: true });
    t.names.insert(key(2), b"__tls_get_addr".to_vec());
    let gtp = 0x42_0ff8u64;
    let desc = 0x42_1010u64;
    let gd = 0x42_1020u64;
    t.slots.insert((key(1), SlotKind::TpOff { negated: false }), gtp);
    t.slots.insert((key(1), SlotKind::TlsDesc), desc);
    t.slots.insert((key(1), SlotKind::TlsGd), gd);
    let base = 0x40_0ff0u64;
    let page = |p: u64, g: u64| ((g & !0xFFF).wrapping_sub(p & !0xFFF)) as i64;
    let adrp_x0 = |p: u64, g: u64| forms::insert(forms::ADRP, 0x9000_0000, page(p, g));
    let ldr_x0 = |g: u64| 0xF940_0000 | ((g & 0xFFF) as u32 >> 3) << 10;
    for out in [OutputKind::Exec, OutputKind::Pie] {
        // Small TLSDESC (R2).
        let code = words(&[0x9000_0000, 0xF940_0001, 0x9100_0000, 0xD63F_0020]);
        let rs: Vec<Reloc> = [562, 563, 564, 569]
            .iter()
            .enumerate()
            .map(|(i, &ty)| Reloc { r_offset: 4 * i as u64, key: key(1), r_type: ty, addend: 0 })
            .collect();
        let mut c = Case::new(&t, &code, base);
        c.arch = M2;
        c.output = out;
        let b = c.apply(&rs).unwrap();
        assert_eq!(b, words(&[adrp_x0(base, gtp), ldr_x0(gtp), synth::NOP, synth::NOP]));
        // Tiny TLSDESC (R2t): `ldr x0, =slot`; `nop`; `nop`.
        let code = words(&[0x5800_0001, 0x1000_0000, 0xD63F_0020]);
        let rs = [
            Reloc { r_offset: 0, key: key(1), r_type: 560, addend: 0 },
            Reloc { r_offset: 4, key: key(1), r_type: 561, addend: 0 },
            Reloc { r_offset: 8, key: key(1), r_type: 569, addend: 0 },
        ];
        let mut c = Case::new(&t, &code, base);
        c.arch = M2;
        c.output = out;
        let b = c.apply(&rs).unwrap();
        let lit = 0x5800_0000 | (((gtp - base) >> 2) as u32 & 0x7_FFFF) << 5;
        assert_eq!(b, words(&[lit, synth::NOP, synth::NOP]));
        // Large TLSDESC (§8.2): kept, `G(GTLSDESC) − GOT`, the markers and
        // the `blr` untouched (the `CALL` after its `TLSDESC_ADD` is the
        // large form's); scan demands the descriptor only.
        let code = words(&[0xD2A0_0000, 0xF280_0000, 0xF860_6801, 0x8B00_0000, 0xD63F_0020]);
        let rs: Vec<Reloc> = [565, 566, 567, 568, 569]
            .iter()
            .enumerate()
            .map(|(i, &ty)| Reloc { r_offset: 4 * i as u64, key: key(1), r_type: ty, addend: 0 })
            .collect();
        let mut c = Case::new(&t, &code, base);
        c.arch = M2;
        c.output = out;
        c.got_base = Some(0x42_0000);
        let b = c.apply(&rs).unwrap();
        let off = desc - 0x42_0000;
        assert_eq!(
            b,
            words(&[
                0xD2A0_0000 | ((off >> 16) as u32 & 0xFFFF) << 5,
                0xF280_0000 | (off as u32 & 0xFFFF) << 5,
                0xF860_6801,
                0x8B00_0000,
                0xD63F_0020
            ])
        );
        let place = PlaceFacts { alloc: true, writable: false, exec: Some(true) };
        let mut s = Sink::default();
        scan_all(M2, &t, out, place, &code, &rs, &mut s).unwrap();
        assert!(s.errors.is_empty(), "{:?}", s.errors);
        assert_eq!(raised(&s, key(1)), DemandFlags::TLS_DESC);
        // A `TLSDESC_ADD` not directly followed by its `CALL`: refused by
        // name (the `CALL` could not be told from a small form's).
        let mut s = Sink::default();
        let (add, call) = (rs[3], Reloc { r_offset: 0, ..rs[4] });
        scan_all(M2, &t, out, place, &code, &[rs[0], rs[1], rs[2], add, call], &mut s).unwrap();
        match s.errors[..] {
            [(3, ScanError::Refused(ref r))] => {
                assert_eq!(r.name.to_string(), mortar_a64::LARGE_TLSDESC_DYNAMIC);
            }
            ref e => panic!("{e:?}"),
        }
        // R4's GD → IE.
        let code = words(&[0x9000_0000, 0x9100_0000, 0x9400_0000, synth::NOP]);
        let rs = [
            Reloc { r_offset: 0, key: key(1), r_type: 513, addend: 0 },
            Reloc { r_offset: 4, key: key(1), r_type: 514, addend: 0 },
            Reloc { r_offset: 8, key: key(2), r_type: 283, addend: 0 },
        ];
        let mut c = Case::new(&t, &code, base);
        c.arch = M2;
        c.output = out;
        let b = c.apply(&rs).unwrap();
        assert_eq!(
            b,
            words(&[
                adrp_x0(base, gtp),
                ldr_x0(gtp),
                mortar_a64::MRS_X1_TPIDR,
                mortar_a64::ADD_X0_X1_X0
            ])
        );
        // GD without the `nop`: the pair stays, the call goes to the PLT.
        let code = words(&[0x9000_0000, 0x9100_0000, 0x9400_0000, 0xAA00_03E0]);
        t.plts.insert(key(2), 0x40_0100);
        t.flags.insert(key(2), DemandFlags::PLT);
        let mut c = Case::new(&t, &code, base);
        c.arch = M2;
        c.output = out;
        let b = c.apply(&rs).unwrap();
        let call = 0x9400_0000 | ((0x40_0100u64.wrapping_sub(base + 8) >> 2) as u32 & 0x3FF_FFFF);
        assert_eq!(
            b,
            words(&[
                adrp_x0(base, gd),
                0x9100_0000 | ((gd & 0xFFF) as u32) << 10,
                call,
                0xAA00_03E0
            ])
        );
        t.plts.remove(&key(2));
        t.flags.remove(&key(2));
        // IE against the shared TLS: kept (R3 needs a TLS-local target).
        let code = words(&[0x9000_0003, 0xF940_0063]);
        let rs = [
            Reloc { r_offset: 0, key: key(1), r_type: 541, addend: 0 },
            Reloc { r_offset: 4, key: key(1), r_type: 542, addend: 0 },
        ];
        let mut c = Case::new(&t, &code, base);
        c.arch = M2;
        c.output = out;
        let b = c.apply(&rs).unwrap();
        let adrp_x3 = forms::insert(forms::ADRP, 0x9000_0003, page(base, gtp));
        assert_eq!(b, words(&[adrp_x3, 0xF940_0063 | ((gtp & 0xFFF) as u32 >> 3) << 10]));
    }
}

// ---------------------------------------------------------------- §11

/// One output code section holding `words` at `addr`, all instructions.
fn sites_of(addr: u64, ws: &[u32]) -> usize {
    let bytes = words(ws);
    let proof = [ProofRun {
        start: 0,
        end: bytes.len() as u64,
        proof: Proof::Insn,
        unit: 0,
        unit_offset: 0,
    }];
    let code = Code { bytes: &bytes, addr, proof: &proof };
    let mut out = Vec::new();
    erratum::sites(&code, &mut out).unwrap();
    out.len()
}

/// a64.md §11, §16 ("the shapes of §9/§10 at every offset modulo 4096
/// form no site"): a dynamic `.plt` (PLT0 and entries, plain and BTI)
/// placed at every word offset of a page holds no 843419 site; and an
/// ADRP bound to a PLT entry (a canonical PLT, `ADR_PREL_PG_HI21` against
/// a shared function in `exec`) at a site takes the ADR fix to the
/// entry's page.
#[test]
fn s11_843419_and_plts() {
    for bti in [false, true] {
        let v = if bti { PltVariant::Bti } else { PltVariant::Plain };
        let shape = M2.plt(v);
        let (hdr, ent) = (shape.chunks[0].header as u64, shape.chunks[0].entry as u64);
        for off in (0..0x1000u64).step_by(4) {
            let addr = 0x41_0000 + off;
            let g = 0x43_0000u64;
            let mut ws = Vec::new();
            let mut buf = [0u8; 32];
            let cx = SynthCx { addr, header: addr, got_plt: g + 16, got_base: 0, index: 0 };
            M2.write_plt(v, PltPart::Header(0), &cx, &mut buf).unwrap();
            ws.extend(buf.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())));
            for n in 0..4u32 {
                let e = addr + hdr + ent * u64::from(n);
                let cx = SynthCx {
                    addr: e,
                    header: addr,
                    got_plt: g + 24 + 8 * u64::from(n),
                    got_base: 0,
                    index: n,
                };
                let mut buf = vec![0u8; ent as usize];
                M2.write_plt(v, PltPart::Entry { chunk: 0, n }, &cx, &mut buf).unwrap();
                ws.extend(buf.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())));
            }
            assert_eq!(sites_of(addr, &ws), 0, "bti={bti} at {addr:#x}");
        }
    }
    // A canonical-PLT ADRP at 0xFF8, then `ldr`, `ldr [x0]`: a site; its
    // relocated word decodes to the PLT entry's page, and the ADR fix
    // reaches it.
    let mut t = Tables::default();
    t.sym(key(1), 0x40_0230, TargetKind::Shlib { func: true });
    t.flags.insert(key(1), DemandFlags::CANON_PLT);
    let code = words(&[0x9000_0000]);
    let mut c = Case::new(&t, &code, 0x40_0ff8);
    c.arch = M2;
    c.output = OutputKind::Exec;
    let w1 = c.word(275, 0, key(1), 0).unwrap();
    assert_eq!(sites_of(0x40_0ff8, &[w1, 0xF940_0021, 0xF940_0000, synth::NOP]), 1);
    let adr = mortar_a64::erratum_adr(0x40_0ff8, w1).expect("in ADR reach");
    assert_eq!(forms::extract(forms::ADR, adr) as u64, 0x40_0000u64.wrapping_sub(0x40_0ff8));
}

// ---------------------------------------------------------------- §9

/// a64.md §9 with PLT targets: a range thunk to a PLT entry is
/// `adrp x16; add x16; br x16` to the entry, and in a BTI output the
/// entry's first word (`bti c`) is the landing pad `thunk_target_ok`
/// requires; a plain entry's first word (`adrp`) is not, so a plain shape
/// in a BTI output could not be reached (the variant follows the property,
/// §10.3).
#[test]
fn s9_thunks_to_plt_entries() {
    let entry = 0x40_0230u64;
    let first = |v: PltVariant| {
        let cx =
            SynthCx { addr: entry, header: 0x40_0200, got_plt: 0x43_0018, got_base: 0, index: 0 };
        let mut b = [0u8; 24];
        M2.write_plt(v, PltPart::Entry { chunk: 0, n: 0 }, &cx, &mut b).unwrap();
        u32::from_le_bytes(b[..4].try_into().unwrap())
    };
    let (bti_word, plain_word) = (first(PltVariant::Bti), first(PltVariant::Plain));
    let cx = |bti: bool, w: u32| PatchCx {
        addr: 0x800_0000,
        target: entry,
        site: 0,
        word: 0,
        bti,
        target_word: Some(w),
        target_abs: false,
    };
    assert_eq!(M2.thunk_target_ok(&cx(true, bti_word)), Ok(()));
    assert_eq!(M2.thunk_target_ok(&cx(false, plain_word)), Ok(()));
    assert!(M2.thunk_target_ok(&cx(true, plain_word)).is_err());
    let mut t = [0u8; 12];
    M2.write_thunk(&cx(true, bti_word), &mut t).unwrap();
    let w: Vec<u32> =
        t.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
    let page = (entry & !0xFFF).wrapping_sub(0x800_0000) as i64;
    assert_eq!(w[0], forms::insert(forms::ADRP, 0x9000_0010, page));
    assert_eq!(w[1], 0x9100_0210 | ((entry & 0xFFF) as u32) << 10);
    assert_eq!(w[2], 0xD61F_0200);
}

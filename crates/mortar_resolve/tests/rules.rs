//! The rules of `spec/resolve.md`, one test per behaviour, named
//! `s<section>_<what>` (§15.1), over hand-built views. Every link runs
//! under `check_schedules` (1, 2, 7, 64 threads and seeds) and its closure
//! is compared with the sequential worklist.

#![allow(
    clippy::many_single_char_names,
    clippy::similar_names,
    reason = "files, entries and indexes are short names in the rules' tests"
)]

mod support;

use mortar_core::par::{Pool, check_schedules};
use mortar_core::resolution::{DropReason, FileLive, SectionKind, SectionState};
use mortar_core::{FileId, OptId, SymEntry, SymbolId};
use mortar_elf::consts::{
    GRP_COMDAT, SHF_ALLOC, SHF_EXCLUDE, SHF_GNU_RETAIN, SHF_LINK_ORDER, SHF_MERGE, SHF_STRINGS,
    SHF_WRITE, SHT_NOBITS, SHT_NOTE, SHT_PROGBITS, SHT_RELA, STB_GLOBAL, STB_GNU_UNIQUE, STB_LOCAL,
    STB_WEAK, STT_FUNC, STT_NOTYPE, STT_OBJECT, STT_SECTION, STT_TLS, STV_DEFAULT, STV_HIDDEN,
    STV_INTERNAL, STV_PROTECTED,
};
use mortar_elf::{Machine, SectionIndex, SymbolIndex, SymbolSection};
use mortar_resolve::linker::{Provided, TABLE, When};
use mortar_resolve::options::{Defsym, DefsymValue, Entry, Named, Options, Output, Wrap};
use mortar_resolve::problem::{DefsymWhy, MalformedSymbol, What};
use mortar_resolve::resolve::flags;
use mortar_resolve::sections::SectionRefusal;
use mortar_resolve::verdict::{SymtabEntry, Unresolved, Verdict, symtab_entry, verdict};
use mortar_resolve::view::FileKind;
use mortar_resolve::{Def, EntryPoint, Props, Resolved, live, resolve};
use support::{BSS, DATA, MSection, Model, TDATA, TEXT};

const A64: Machine = Machine::AArch64;

/// Options with no entry (so that no test sees `entry-not-found` unless
/// it asks).
fn opts() -> Options {
    Options { entry: Entry::None, ..Options::default() }
}

fn named(m: &mut Model, name: &str, arg: u32) -> Named {
    Named { key: OptId::some(m.key(name)), name: name.as_bytes().into(), arg }
}

fn link_with(m: &Model, o: &Options) -> Resolved {
    let r = check_schedules(|pool| resolve(m, o, pool));
    let roots: Vec<(SymbolId, u32)> = o.roots().iter().map(|r| (r.key, r.arg)).collect();
    // The oracle is of §4 (extraction); the shared stage then sets the
    // rows of the shared objects (`spec/dyn.md` D-11).
    let mut want = live::oracle(m, &r.targets, &r.redirect, &roots);
    for (f, row) in want.iter_mut().enumerate() {
        if mortar_resolve::view::View::file(m, FileId(f as u32)).kind == FileKind::Shared {
            *row = r.file_live[f];
        }
    }
    assert_eq!(want, r.file_live, "closure != oracle");
    r
}

fn link(m: &Model) -> Resolved {
    link_with(m, &opts())
}

fn e(file: FileId, index: SymbolIndex) -> SymEntry {
    SymEntry { file, index }
}

fn def(m: &Model, r: &Resolved, name: &str) -> Def {
    r.def[m.id(name).0 as usize]
}

fn ids(r: &Resolved) -> Vec<&'static str> {
    r.problems.iter().map(mortar_resolve::Problem::id).collect()
}

fn live_of(r: &Resolved, f: FileId) -> bool {
    r.file_live[f.0 as usize].is_live()
}

// ---- §3.2 ------------------------------------------------------------------

#[test]
fn s3_2_strong_beats_weak_either_order() {
    for strong_first in [true, false] {
        let mut m = Model::new(A64);
        let a = m.obj("a.o");
        let b = m.obj("b.o");
        let (s, w) = if strong_first { (a, b) } else { (b, a) };
        let si = m.def(s, "K");
        m.weak(w, "K");
        let r = link(&m);
        assert_eq!(def(&m, &r, "K"), Def::Strong(e(s, si)));
        assert!(r.problems.is_empty(), "{:?}", r.problems);
    }
}

#[test]
fn s3_2_first_weak_wins() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    let b = m.obj("b.o");
    let c = m.obj("c.o");
    m.undef(a, "K");
    let wb = m.weak(b, "K");
    m.weak(c, "K");
    let r = link(&m);
    assert_eq!(def(&m, &r, "K"), Def::Weak(e(b, wb)));
}

#[test]
fn s3_2_common_beats_weak() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    let b = m.obj("b.o");
    m.weak_data(a, "v", 4);
    let c = m.common(b, "v", 4, 4);
    let r = link(&m);
    assert_eq!(def(&m, &r, "v"), Def::Common(e(b, c)));
    assert_eq!(
        r.common,
        vec![(m.id("v"), mortar_core::resolution::CommonShape { size: 4, align: 2, large: false })]
    );
}

#[test]
fn s3_2_lazy_files_have_no_candidates() {
    // A lazy member that stays lazy defines nothing (finding 5).
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.weak_undef(a, "K");
    let ar = m.archive();
    let x = m.member(ar, "x.o");
    m.def(x, "K");
    let r = link(&m);
    assert!(!live_of(&r, x));
    assert_eq!(def(&m, &r, "K"), Def::Undefined { weak: true });
}

#[test]
fn s3_2_order_is_total() {
    // Random candidate sets: `def` is the brute-force minimum of the
    // table (rank, then FileId, then index), whatever the file order the
    // workers visit (check_schedules permutes it).
    let mut seed = 0x1234_5678_u64;
    let mut rnd = move |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n
    };
    for _ in 0..200 {
        let mut m = Model::new(A64);
        let files = 1 + rnd(6) as usize;
        let mut expect: Option<(u8, FileId, SymbolIndex)> = None;
        let mut any_ref_strong = false;
        for i in 0..files {
            let f = m.obj(&format!("f{i}.o"));
            for _ in 0..=rnd(3) {
                let (rank, idx) = match rnd(5) {
                    0 => (1, m.def(f, "K")),
                    1 => (2, m.common(f, "K", 1 + rnd(16), 1 << rnd(4))),
                    2 => (3, m.weak(f, "K")),
                    3 => {
                        any_ref_strong = true;
                        (9, m.undef(f, "K"))
                    }
                    _ => (9, m.weak_undef(f, "K")),
                };
                if rank < 9 && expect.is_none_or(|(r, _, _)| rank < r) {
                    expect = Some((rank, f, idx));
                }
            }
        }
        let r = link_with(&m, &Options { allow_multiple_definition: true, ..opts() });
        let got = def(&m, &r, "K");
        let want = match expect {
            None => Def::Undefined { weak: !any_ref_strong },
            Some((1, f, i)) => Def::Strong(e(f, i)),
            Some((2, f, i)) => Def::Common(e(f, i)),
            Some((_, f, i)) => Def::Weak(e(f, i)),
        };
        assert_eq!(got, want);
    }
}

// ---- §3.3 ------------------------------------------------------------------

#[test]
fn s3_3_size_and_alignment_are_maxima() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    let b = m.obj("b.o");
    let first = m.common(a, "v", 4, 16);
    m.common(b, "v", 32, 4);
    let r = link(&m);
    assert_eq!(def(&m, &r, "v"), Def::Common(e(a, first)));
    assert_eq!(
        r.common[0].1,
        mortar_core::resolution::CommonShape { size: 32, align: 4, large: false }
    );
    assert!(r.problems.is_empty());
}

#[test]
fn s3_3_alignment_not_power_of_two_is_malformed() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    let i = m.common(a, "v", 4, 12);
    let r = link(&m);
    assert!(
        r.problems.iter().any(|p| p.what
            == What::Malformed { entry: e(a, i), why: MalformedSymbol::CommonAlignment(12) })
    );
}

/// Alignment 0 is alignment 1, as GNU ld and mold take it (lld refuses;
/// WP-022.md, "common alignment 0").
#[test]
fn s3_3_alignment_zero_is_one() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.common(a, "v", 3, 0);
    let r = link(&m);
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    assert_eq!(
        r.common[0].1,
        mortar_core::resolution::CommonShape { size: 3, align: 0, large: false }
    );
}

#[test]
fn s3_3_larger_common_warns_in_both_orders() {
    for common_first in [true, false] {
        let mut m = Model::new(A64);
        let a = m.obj("a.o");
        let b = m.obj("b.o");
        let (cf, df) = if common_first { (a, b) } else { (b, a) };
        let c = m.common(cf, "v", 64, 4);
        let d = m.data(df, "v", 4);
        let r = link(&m);
        assert_eq!(def(&m, &r, "v"), Def::Strong(e(df, d)));
        assert!(r.common.is_empty());
        let larger: Vec<&What> = r
            .problems
            .iter()
            .map(|p| &p.what)
            .filter(|w| matches!(w, What::CommonLarger { .. }))
            .collect();
        assert_eq!(
            larger,
            vec![&What::CommonLarger {
                key: m.id("v"),
                def: e(df, d),
                def_size: 4,
                common: e(cf, c),
                common_size: 64
            }]
        );
    }
}

#[test]
fn s3_3_less_aligned_definition_warns() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    let b = m.obj("b.o");
    m.files[b.0 as usize].sections[DATA as usize].addralign = 4;
    let c = m.common(a, "v", 4, 16);
    let d = m.data(b, "v", 4);
    let r = link(&m);
    assert!(r.problems.iter().any(|p| p.what
        == What::CommonAlignment {
            key: m.id("v"),
            def: e(b, d),
            def_align: 4,
            common: e(a, c),
            common_align: 16
        }));
}

#[test]
fn s3_3_warn_common() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    let b = m.obj("b.o");
    let c = m.obj("c.o");
    let ca = m.common(a, "v", 4, 4);
    let cb = m.common(b, "v", 4, 4);
    let cw = m.common(a, "w", 4, 4);
    let dw = m.data(c, "w", 4);
    let quiet = link(&m);
    assert!(quiet.problems.is_empty());
    let r = link_with(&m, &Options { warn_common: true, ..opts() });
    let whats: Vec<&What> = r.problems.iter().map(|p| &p.what).collect();
    assert!(whats.contains(&&What::MultipleCommon {
        key: m.id("v"),
        first: e(a, ca),
        second: e(b, cb)
    }));
    assert!(whats.contains(&&What::CommonOverridden {
        key: m.id("w"),
        common: e(a, cw),
        def: e(c, dw)
    }));
}

// ---- §3.4 ------------------------------------------------------------------

#[test]
fn s3_4_tls_mismatch_each_direction() {
    // TLS ref / non-TLS def; NOTYPE ref / TLS def; common / TLS def; TLS
    // ref / non-TLS ref with no definition.
    type Add<'a> = &'a dyn Fn(&mut Model, FileId);
    let cases: [(Add, Add); 4] = [
        (
            &|m, f| {
                m.undef_typed(f, "x", STT_TLS);
            },
            &|m, f| {
                m.data(f, "x", 4);
            },
        ),
        (
            &|m, f| {
                m.undef(f, "x");
            },
            &|m, f| {
                m.tls(f, "x");
            },
        ),
        (
            &|m, f| {
                m.common(f, "x", 4, 4);
            },
            &|m, f| {
                m.tls(f, "x");
            },
        ),
        (
            &|m, f| {
                m.undef_typed(f, "x", STT_TLS);
            },
            &|m, f| {
                m.undef_typed(f, "x", STT_OBJECT);
            },
        ),
    ];
    for (n, (one, two)) in cases.iter().enumerate() {
        for swap in [false, true] {
            let mut m = Model::new(A64);
            let a = m.obj("a.o");
            let b = m.obj("b.o");
            let (fa, fb) = if swap { (b, a) } else { (a, b) };
            one(&mut m, fa);
            two(&mut m, fb);
            let r = link(&m);
            assert_eq!(
                ids(&r).iter().filter(|&&i| i == "tls-mismatch").count(),
                1,
                "case {n} swap {swap}: {:?}",
                r.problems
            );
            assert!(!ids(&r).iter().any(|i| i.starts_with("common-")));
        }
    }
    // Agreeing TLS mentions are quiet.
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    let b = m.obj("b.o");
    m.undef_typed(a, "x", STT_TLS);
    m.tls(b, "x");
    assert!(link(&m).problems.is_empty());
}

#[test]
fn s3_4_unique_duplicates_clash() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    let b = m.obj("b.o");
    m.sym(a, "u", STB_GNU_UNIQUE, STT_OBJECT, SymbolSection::Section(SectionIndex(DATA)), 0, 4);
    m.sym(b, "u", STB_GNU_UNIQUE, STT_OBJECT, SymbolSection::Section(SectionIndex(DATA)), 0, 4);
    let r = link(&m);
    assert_eq!(ids(&r), vec!["duplicate-symbol"]);
    assert!(r.clash[m.id("u").0 as usize]);
}

#[test]
fn s3_4_equal_absolutes_do_not_clash() {
    for n in [2, 3] {
        let mut m = Model::new(A64);
        let mut first = None;
        for i in 0..n {
            let f = m.obj(&format!("a{i}.o"));
            let idx = m.abs(f, "A", 0x40);
            first.get_or_insert(e(f, idx));
        }
        let r = link(&m);
        assert!(r.problems.is_empty(), "{:?}", r.problems);
        assert!(!r.clash[m.id("A").0 as usize]);
        assert_eq!(def(&m, &r, "A"), Def::Strong(first.unwrap()));
    }
}

#[test]
fn s3_4_unequal_absolutes_clash() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    let b = m.obj("b.o");
    m.abs(a, "A", 1);
    m.abs(b, "A", 2);
    assert_eq!(ids(&link(&m)), vec!["duplicate-symbol"]);
}

#[test]
fn s3_4_absolute_and_section_definition_clash() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    let b = m.obj("b.o");
    m.abs(a, "A", 0);
    m.data(b, "A", 4);
    assert_eq!(ids(&link(&m)), vec!["duplicate-symbol"]);
}

#[test]
fn s3_4_nonlocal_section_symbol_is_malformed() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    let i =
        m.sym(a, "s", STB_GLOBAL, STT_SECTION, SymbolSection::Section(SectionIndex(TEXT)), 0, 0);
    let r = link(&m);
    assert_eq!(
        r.problems.iter().map(|p| &p.what).collect::<Vec<_>>(),
        vec![&What::Malformed { entry: e(a, i), why: MalformedSymbol::NonLocalSectionOrFile }]
    );
}

#[test]
fn s3_4_bindings_and_types_refused() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    let i = m.sym(a, "l", STB_LOCAL, STT_FUNC, SymbolSection::Section(SectionIndex(TEXT)), 0, 0);
    let j = m.sym(a, "t", STB_GLOBAL, 7, SymbolSection::Section(SectionIndex(TEXT)), 0, 0);
    let k = m.sym(a, "w", STB_WEAK, STT_OBJECT, SymbolSection::Common, 4, 4);
    let n = m.sym(a, "r", STB_GLOBAL, STT_OBJECT, SymbolSection::Reserved(0xff02), 4, 4);
    let r = link(&m);
    let whats: Vec<&What> = r.problems.iter().map(|p| &p.what).collect();
    assert_eq!(
        whats,
        vec![
            &What::SymbolBinding { entry: e(a, i), binding: STB_LOCAL },
            &What::SymbolType { entry: e(a, j), sym_type: 7 },
            &What::WeakCommon { entry: e(a, k) },
            &What::ReservedSection { entry: e(a, n), shndx: 0xff02 },
        ]
    );
    assert!(r.problems.iter().all(|p| p.id() == "refused"));
}

// ---- §3.5, §3.6 ------------------------------------------------------------

#[test]
fn s3_5_most_constraining_visibility() {
    for (vis, want) in [
        (STV_PROTECTED, STV_PROTECTED),
        (STV_HIDDEN, STV_HIDDEN),
        (STV_INTERNAL, STV_HIDDEN),
        (STV_DEFAULT, STV_DEFAULT),
    ] {
        let mut m = Model::new(A64);
        let a = m.obj("a.o");
        let b = m.obj("b.o");
        m.def(a, "K");
        let u = m.undef(b, "K");
        m.set_visibility(b, u, vis);
        let r = link(&m);
        assert_eq!(r.props[m.id("K").0 as usize].visibility, want);
    }
}

#[test]
fn s3_5_lazy_mentions_do_not_contribute() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.def(a, "K");
    let ar = m.archive();
    let x = m.member(ar, "x.o");
    let u = m.undef(x, "K");
    m.set_visibility(x, u, STV_HIDDEN);
    let r = link(&m);
    assert!(!live_of(&r, x));
    assert_eq!(r.props[m.id("K").0 as usize].visibility, STV_DEFAULT);
    assert_eq!(r.props[m.id("K").0 as usize].flags & flags::REF_REGULAR, 0);
}

#[test]
fn s3_6_ref_flags() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.weak_undef(a, "W");
    m.undef(a, "S");
    m.weak_undef(a, "S");
    let r = link(&m);
    let w = r.props[m.id("W").0 as usize].flags;
    let s = r.props[m.id("S").0 as usize].flags;
    assert_eq!(w & (flags::REF_REGULAR | flags::REF_STRONG), flags::REF_REGULAR);
    assert_eq!(
        s & (flags::REF_REGULAR | flags::REF_STRONG),
        flags::REF_REGULAR | flags::REF_STRONG
    );
    assert_eq!(def(&m, &r, "W"), Def::Undefined { weak: true });
    assert_eq!(def(&m, &r, "S"), Def::Undefined { weak: false });
}

// ---- §3.7, §5.2 ------------------------------------------------------------

#[test]
fn s3_7_output_binding_table() {
    let x = e(FileId(1), SymbolIndex(1));
    let p = |v| Props { visibility: v, flags: 0 };
    let entry = |binding, visibility| SymtabEntry::Entry { binding, visibility };
    assert_eq!(symtab_entry(Def::Strong(x), p(STV_HIDDEN), false), entry(STB_LOCAL, STV_HIDDEN));
    assert_eq!(symtab_entry(Def::Weak(x), p(STV_HIDDEN), false), entry(STB_LOCAL, STV_HIDDEN));
    assert_eq!(symtab_entry(Def::Strong(x), p(STV_DEFAULT), false), entry(STB_GLOBAL, STV_DEFAULT));
    assert_eq!(
        symtab_entry(Def::Strong(x), p(STV_DEFAULT), true),
        entry(STB_GNU_UNIQUE, STV_DEFAULT)
    );
    assert_eq!(
        symtab_entry(Def::Common(x), p(STV_PROTECTED), false),
        entry(STB_GLOBAL, STV_PROTECTED)
    );
    assert_eq!(symtab_entry(Def::Weak(x), p(STV_DEFAULT), false), entry(STB_WEAK, STV_DEFAULT));
    let provide = Def::Provide { sym: Provided::Table(0), conditional: false };
    assert_eq!(symtab_entry(provide, p(STV_DEFAULT), false), SymtabEntry::Linker);
    assert_eq!(
        symtab_entry(Def::Undefined { weak: true }, p(STV_DEFAULT), false),
        SymtabEntry::Absent
    );
}

#[test]
fn s5_2_verdict_table() {
    let k = SymbolId(3);
    let p = |v| Props { visibility: v, flags: 0 };
    let u = Def::Undefined { weak: false };
    let w = Def::Undefined { weak: true };
    let d = Def::Strong(e(FileId(1), SymbolIndex(1)));
    assert_eq!(verdict(k, d, p(STV_DEFAULT), Unresolved::Error, &[]), Verdict::Defined);
    assert_eq!(verdict(k, w, p(STV_DEFAULT), Unresolved::Error, &[]), Verdict::Zero);
    assert_eq!(verdict(k, u, p(STV_DEFAULT), Unresolved::Error, &[]), Verdict::Error);
    assert_eq!(verdict(k, u, p(STV_DEFAULT), Unresolved::Warn, &[]), Verdict::Warning);
    assert_eq!(verdict(k, u, p(STV_DEFAULT), Unresolved::Ignore, &[]), Verdict::Zero);
    assert_eq!(verdict(k, u, p(STV_DEFAULT), Unresolved::Error, &[k]), Verdict::Zero);
    // Hidden: an error whatever the options; weak hidden is zero.
    assert_eq!(verdict(k, u, p(STV_HIDDEN), Unresolved::Ignore, &[k]), Verdict::Error);
    assert_eq!(verdict(k, w, p(STV_HIDDEN), Unresolved::Error, &[]), Verdict::Zero);
}

#[test]
fn s3_5_hidden_needs_object_definition() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    let u = m.undef(a, "zz");
    m.set_visibility(a, u, STV_HIDDEN);
    let r = link(&m);
    let k = m.id("zz");
    assert_eq!(r.def[k.0 as usize], Def::Undefined { weak: false });
    assert_eq!(
        verdict(k, r.def[k.0 as usize], r.props[k.0 as usize], Unresolved::Ignore, &[]),
        Verdict::Error
    );
}

#[test]
fn s3_5_hidden_weak_undefined_is_zero() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    let u = m.weak_undef(a, "zz");
    m.set_visibility(a, u, STV_HIDDEN);
    let r = link(&m);
    let k = m.id("zz");
    assert_eq!(
        verdict(k, r.def[k.0 as usize], r.props[k.0 as usize], Unresolved::Error, &[]),
        Verdict::Zero
    );
}

#[test]
fn s5_1_unused_hidden_undefined_is_accepted() {
    // Resolution records `Undefined`; no diagnostic (the scan decides).
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    let u = m.undef(a, "zz");
    m.set_visibility(a, u, STV_HIDDEN);
    assert!(link(&m).problems.is_empty());
}

// ---- §3.8 ------------------------------------------------------------------

#[test]
fn s3_8_duplicate_lists_three_places() {
    let mut m = Model::new(A64);
    let fs: Vec<FileId> = (0..4).map(|i| m.obj(&format!("d{i}.o"))).collect();
    let idx: Vec<SymbolIndex> = fs.iter().map(|&f| m.def(f, "K")).collect();
    let r = link(&m);
    let [p] = r.problems.as_slice() else { panic!("{:?}", r.problems) };
    let What::Duplicate { key, defs } = &p.what else { panic!() };
    assert_eq!(*key, m.id("K"));
    assert_eq!(
        defs.iter().map(|d| d.entry).collect::<Vec<_>>(),
        (0..4).map(|i| e(fs[i], idx[i])).collect::<Vec<_>>()
    );
    assert_eq!(p.key, mortar_resolve::problem::DiagKey::at(e(fs[1], idx[1])));
}

#[test]
fn s3_8_muldefs_takes_first() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    let b = m.obj("b.o");
    let i = m.def(a, "K");
    m.def(b, "K");
    let r = link_with(&m, &Options { allow_multiple_definition: true, ..opts() });
    assert!(r.problems.is_empty());
    assert_eq!(def(&m, &r, "K"), Def::Strong(e(a, i)));
    assert!(r.clash[m.id("K").0 as usize]);
}

// ---- §4.1 ------------------------------------------------------------------

/// A generated corpus: `n` archives of members with references in chains
/// and cycles.
fn corpus(seed: u64, members: usize) -> Model {
    let mut s = seed | 1;
    let mut rnd = move |n: u64| {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s % n
    };
    let mut m = Model::new(A64);
    let main = m.obj("main.o");
    m.undef(main, "f0");
    let mut ar = m.archive();
    for i in 0..members {
        if rnd(16) == 0 {
            ar = m.archive();
        }
        let f = m.member(ar, &format!("m{i}.o"));
        match rnd(4) {
            0 => {
                m.weak(f, &format!("f{i}"));
            }
            1 => {
                m.common(f, &format!("f{i}"), 4, 4);
            }
            _ => {
                m.def(f, &format!("f{i}"));
            }
        }
        if rnd(3) == 0 {
            m.data(f, &format!("d{}", rnd(members as u64)), 4);
        }
        for _ in 0..rnd(4) {
            let t = rnd(members as u64);
            match rnd(5) {
                0 => {
                    m.weak_undef(f, &format!("f{t}"));
                }
                1 => {
                    m.common(f, &format!("d{t}"), 8, 8);
                }
                _ => {
                    m.undef(f, &format!("f{t}"));
                }
            }
        }
    }
    m
}

#[test]
fn s4_1_closure_is_order_independent() {
    for seed in 1..12 {
        let m = corpus(seed * 7919, 300);
        // link_with compares the rounds with the worklist under every
        // schedule; a random-order worklist gives the same set too.
        let r = link(&m);
        let mut order: Vec<u32> = (0..m.files.len() as u32).collect();
        let mut s = seed;
        for i in (1..order.len()).rev() {
            s = s.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            order.swap(i, (s >> 33) as usize % (i + 1));
        }
        let mut live = vec![false; m.files.len()];
        let mut work: Vec<u32> =
            order.iter().copied().filter(|&f| !m.files[f as usize].info.kind.is_lazy()).collect();
        for &f in &work {
            live[f as usize] = true;
        }
        while let Some(f) = work.pop() {
            for mm in &m.files[f as usize].mentions {
                let t = match mm.place {
                    SymbolSection::Undefined if mm.binding != STB_WEAK => {
                        r.targets.first[mm.key.0 as usize]
                    }
                    SymbolSection::Common => r.targets.cfirst[mm.key.0 as usize],
                    _ => continue,
                };
                if t != live::NO_FILE && !live[t as usize] {
                    live[t as usize] = true;
                    work.push(t);
                }
            }
        }
        let got: Vec<bool> = r.file_live.iter().map(|l| l.is_live()).collect();
        assert_eq!(got, live, "seed {seed}");
    }
}

#[test]
fn s4_1_first_counts_live_and_lazy() {
    // The definition in the lazy member comes first: the archive is
    // before b.o, so a.o's reference extracts x.o, and b.o's weak
    // definition does not block it (R1).
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.undef(a, "K");
    let ar = m.archive();
    let x = m.member(ar, "x.o");
    let xi = m.def(x, "K");
    let b = m.obj("b.o");
    m.weak(b, "K");
    let r = link(&m);
    assert!(live_of(&r, x));
    assert_eq!(def(&m, &r, "K"), Def::Strong(e(x, xi)));
    assert_eq!(r.targets.first[m.id("K").0 as usize], x.0);
}

#[test]
fn s4_1_definition_before_archive_blocks() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.undef(a, "K");
    let w = m.weak(a, "K2");
    let b = m.obj("b.o");
    let bi = m.weak(b, "K");
    let ar = m.archive();
    let x = m.member(ar, "x.o");
    m.def(x, "K");
    let _ = w;
    let r = link(&m);
    assert!(!live_of(&r, x));
    assert_eq!(def(&m, &r, "K"), Def::Weak(e(b, bi)));
}

#[test]
fn s4_1_weak_definition_after_archive_does_not_block() {
    // t1: t1m.o libt1k.a t1w.o: the member is extracted (2).
    let mut m = Model::new(A64);
    let t1m = m.obj("t1m.o");
    m.undef(t1m, "K");
    let ar = m.archive();
    let k = m.member(ar, "t1k.o");
    let ki = m.def(k, "K");
    let t1w = m.obj("t1w.o");
    m.weak(t1w, "K");
    let r = link(&m);
    assert_eq!(r.file_live[k.0 as usize], FileLive::Extracted(e(t1m, SymbolIndex(1))));
    assert_eq!(def(&m, &r, "K"), Def::Strong(e(k, ki)));
    assert!(r.problems.is_empty());
}

#[test]
fn s4_1_weak_reference_does_not_extract() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.weak_undef(a, "K");
    let ar = m.archive();
    let x = m.member(ar, "x.o");
    m.def(x, "K");
    let r = link(&m);
    assert!(!live_of(&r, x));
}

#[test]
fn s4_1_lazy_common_is_first() {
    // R5: mv.o libLC.a: the strong reference extracts the member whose
    // common is first.
    let mut m = Model::new(A64);
    let mv = m.obj("mv.o");
    m.undef(mv, "v");
    let ar = m.archive();
    let lvc = m.member(ar, "lvc.o");
    let c = m.common(lvc, "v", 4, 4);
    m.def(lvc, "other");
    let r = link(&m);
    assert!(live_of(&r, lvc));
    assert_eq!(def(&m, &r, "v"), Def::Common(e(lvc, c)));
}

#[test]
fn s4_1_weak_member_first_wins() {
    // R3: mk.o libKW.a libKS.a: the weak member is first(K).
    let mut m = Model::new(A64);
    let mk = m.obj("mk.o");
    m.undef(mk, "K");
    let kw_ar = m.archive();
    let kw = m.member(kw_ar, "kw.o");
    let wi = m.weak(kw, "K");
    let ks_ar = m.archive();
    let ks = m.member(ks_ar, "ks.o");
    m.def(ks, "K");
    let r = link(&m);
    assert!(live_of(&r, kw) && !live_of(&r, ks));
    assert_eq!(def(&m, &r, "K"), Def::Weak(e(kw, wi)));
}

#[test]
fn s4_1_pulling_reference_is_least() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    let b = m.obj("b.o");
    m.def(b, "pad");
    let bi = m.undef(b, "K");
    m.def(a, "pad2");
    m.def(a, "pad3");
    let ai = m.undef(a, "K");
    let ar = m.archive();
    let x = m.member(ar, "x.o");
    m.def(x, "K");
    let r = link(&m);
    let _ = bi;
    assert_eq!(r.file_live[x.0 as usize], FileLive::Extracted(e(a, ai)));
}

#[test]
fn s4_1_def_never_lazy() {
    for seed in 1..6 {
        let m = corpus(seed * 104_729, 200);
        let r = link(&m);
        for d in &r.def {
            if let Some(x) = d.entry() {
                assert!(r.file_live[x.file.0 as usize].is_live());
            }
        }
    }
}

#[test]
fn s4_1_wrap_target_is_not_a_root() {
    // --wrap=f: a.o's undefined f binds to __wrap_f; a member defining
    // only f is not extracted for it; __real_f extracts f's definer.
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.undef(a, "f");
    let ar = m.archive();
    let x = m.member(ar, "f.o");
    m.def(x, "f");
    let wrap = Wrap { sym: m.key("f"), wrap: m.key("__wrap_f"), real: m.key("__real_f") };
    let o = Options { wrap: vec![wrap], ..opts() };
    let r = link_with(&m, &o);
    assert!(!live_of(&r, x));
    assert_eq!(def(&m, &r, "__wrap_f"), Def::Undefined { weak: false });
    assert_eq!(def(&m, &r, "f"), Def::Undefined { weak: true });
}

// ---- §4.2 ------------------------------------------------------------------

#[test]
fn s4_2_each_root_option_extracts() {
    for which in 0..3 {
        let mut m = Model::new(A64);
        let a = m.obj("m0.o");
        m.def(a, "main");
        let ar = m.archive();
        let ks = m.member(ar, "ks.o");
        m.def(ks, "K");
        let n = named(&mut m, "K", 5);
        let o = match which {
            0 => Options { undefined: vec![n], ..opts() },
            1 => Options { require_defined: vec![n], ..opts() },
            _ => Options { entry: Entry::Named(n), ..opts() },
        };
        let r = link_with(&m, &o);
        assert_eq!(
            r.file_live[ks.0 as usize],
            FileLive::Extracted(e(FileId::INTERNAL, SymbolIndex(5)))
        );
        assert!(r.problems.is_empty(), "{:?}", r.problems);
        if which == 2 {
            assert_eq!(r.entry, EntryPoint::Symbol(m.id("K")));
        }
    }
}

#[test]
fn s4_2_default_start_is_a_root() {
    // R9 / t12: m0.o libst.a with no -e: the _start member is extracted.
    let mut m = Model::new(A64);
    let a = m.obj("m0.o");
    m.def(a, "main");
    let ar = m.archive();
    let st = m.member(ar, "st.o");
    m.def(st, "_start");
    let o = Options { entry: Entry::Default(OptId::some(m.key("_start"))), ..opts() };
    let r = link_with(&m, &o);
    assert!(live_of(&r, st));
    assert_eq!(r.entry, EntryPoint::Symbol(m.id("_start")));
    assert!(r.problems.is_empty());
}

#[test]
fn s4_2_require_defined_errors() {
    let mut m = Model::new(A64);
    m.obj("a.o");
    let n = named(&mut m, "nosuch", 2);
    let missing = Named { key: OptId::NONE, name: b"absent".as_slice().into(), arg: 3 };
    let r = link_with(&m, &Options { require_defined: vec![n, missing], ..opts() });
    assert_eq!(ids(&r), vec!["required-symbol", "required-symbol"]);
}

// spec: resolve.md §4.2 (WP-152): a definition only a shared object
// gives does not satisfy --require-defined (GNU ld's error; mold accepts
// it, lld has no such option): required-symbol, with the shared object's
// entry as its place. -u of the same name is no error.
#[test]
fn s4_2_require_defined_by_a_shared_object_errors() {
    let mut m = Model::new(A64);
    m.obj("a.o");
    let lib = m.shlib("libk.so", false);
    let k = m.def(lib, "k");
    let n = named(&mut m, "k", 4);
    let r = link_with(&m, &Options { require_defined: vec![n.clone()], ..opts() });
    assert!(matches!(def(&m, &r, "k"), Def::Shared(_)));
    assert!(
        r.problems.iter().any(|p| matches!(&p.what,
            What::RequiredSymbol { arg: 4, shared: Some(s), .. } if *s == e(lib, k))),
        "{:?}",
        r.problems
    );
    let r = link_with(&m, &Options { undefined: vec![n], ..opts() });
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    // An --as-needed library the root leaves out (WP-152): the same note.
    let mut m = Model::new(A64);
    m.obj("a.o");
    let lib = m.shlib("libk.so", true);
    let k = m.def(lib, "k");
    let n = named(&mut m, "k", 4);
    let r = link_with(&m, &Options { require_defined: vec![n], ..opts() });
    assert_eq!(def(&m, &r, "k"), Def::Undefined { weak: false });
    assert!(
        r.problems.iter().any(|p| matches!(&p.what,
            What::RequiredSymbol { arg: 4, shared: Some(s), .. } if *s == e(lib, k))),
        "{:?}",
        r.problems
    );
}

/// §4.2, "A root and a shared object" (WP-152): `-u k`, `k` in the
/// shared object `lib` (as-needed or not), the archive member `member`
/// after it when `ar` is set; `refs` names an object's references.
fn root_and_lib(
    as_needed: bool,
    ar: bool,
    refs: &[&str],
) -> (Model, FileId, Option<FileId>, Options) {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    for r in refs {
        m.undef(a, r);
    }
    let lib = m.shlib("libk.so", as_needed);
    m.def(lib, "k");
    m.def(lib, "j");
    let member = ar.then(|| {
        let arch = m.archive();
        let k = m.member(arch, "k.o");
        m.def(k, "k");
        k
    });
    let n = named(&mut m, "k", 4);
    let o = Options { undefined: vec![n], ..opts() };
    (m, lib, member, o)
}

fn imported(m: &Model, r: &Resolved, name: &str) -> bool {
    r.dynamic.as_ref().is_some_and(|d| d.imported[m.id(name).0 as usize])
}

#[test]
fn s4_2_a_root_makes_no_library_needed() {
    // Item 1: GNU ld and lld leave the library not needed; `k` is written
    // nowhere (no object reference, so no `FROM_OBJECT`).
    let (m, lib, _, o) = root_and_lib(true, false, &[]);
    let r = link_with(&m, &o);
    assert!(ids(&r).is_empty(), "{:?}", r.problems);
    assert_eq!(r.file_live[lib.0 as usize], FileLive::Dead);
    assert_eq!(def(&m, &r, "k"), Def::Undefined { weak: false });
    assert!(!imported(&m, &r, "k"));
    assert_eq!(r.props[m.id("k").0 as usize].flags & flags::OBJ_REF, 0);
}

#[test]
fn s4_2_a_root_imports_from_a_needed_library() {
    // Item 2: needed for itself (--no-as-needed) or for another symbol:
    // the root's name is imported (GNU ld).
    for (as_needed, refs) in [(false, &[][..]), (true, &["j"][..])] {
        let (m, lib, _, o) = root_and_lib(as_needed, false, refs);
        let r = link_with(&m, &o);
        assert!(ids(&r).is_empty(), "{:?}", r.problems);
        assert!(matches!(r.file_live[lib.0 as usize], FileLive::Shared { needed: true, .. }));
        assert!(matches!(def(&m, &r, "k"), Def::Shared(_)));
        assert!(imported(&m, &r, "k"));
    }
    // A member after the needed library is not extracted (GNU ld).
    let (m, _, member, o) = root_and_lib(true, true, &["j"]);
    let r = link_with(&m, &o);
    assert!(ids(&r).is_empty(), "{:?}", r.problems);
    assert!(!live_of(&r, member.unwrap()));
    assert!(imported(&m, &r, "k"));
}

#[test]
fn s4_2_a_root_an_unneeded_library_and_a_member_is_refused() {
    // Item 3: GNU ld extracts the member, lld does not.
    let (m, lib, member, o) = root_and_lib(true, true, &[]);
    let r = link_with(&m, &o);
    let member = member.unwrap();
    assert!(
        r.problems.iter().any(|p| matches!(p.what,
            What::RootAsNeeded { arg: 4, lib: l, member: f, .. } if l == lib && f == member)),
        "{:?}",
        r.problems
    );
    assert_eq!(ids(&r), ["refused"]);
    // An object's own reference makes the library needed: no refusal.
    let (m, lib, _, o) = root_and_lib(true, true, &["k"]);
    let r = link_with(&m, &o);
    assert!(ids(&r).is_empty(), "{:?}", r.problems);
    assert!(matches!(r.file_live[lib.0 as usize], FileLive::Shared { needed: true, .. }));
    assert!(imported(&m, &r, "k"));
    // The library under --no-as-needed: imported, no refusal.
    let (m, _, _, o) = root_and_lib(false, true, &[]);
    let r = link_with(&m, &o);
    assert!(ids(&r).is_empty(), "{:?}", r.problems);
    assert!(imported(&m, &r, "k"));
}

#[test]
fn s4_2_a_root_and_a_member_before_the_library() {
    // The archive first: first(k) is the member, extracted (all four).
    let mut m = Model::new(A64);
    m.obj("a.o");
    let arch = m.archive();
    let km = m.member(arch, "k.o");
    let ki = m.def(km, "k");
    let lib = m.shlib("libk.so", true);
    m.def(lib, "k");
    let n = named(&mut m, "k", 4);
    let r = link_with(&m, &Options { undefined: vec![n], ..opts() });
    assert!(ids(&r).is_empty(), "{:?}", r.problems);
    assert_eq!(def(&m, &r, "k"), Def::Strong(e(km, ki)));
    assert_eq!(r.file_live[lib.0 as usize], FileLive::Dead);
}

#[test]
fn s4_2_whole_archive_members_are_roots() {
    // R4: mk.o libKS.a --whole-archive libKW.a: ks.o is extracted (first)
    // and kw.o is live; K is ks.o's strong definition.
    let mut m = Model::new(A64);
    let mk = m.obj("mk.o");
    m.undef(mk, "K");
    let a1 = m.archive();
    let ks = m.member(a1, "ks.o");
    let ki = m.def(ks, "K");
    let a2 = m.archive();
    let kw = m.whole(a2, "kw.o");
    m.weak(kw, "K");
    let r = link(&m);
    assert!(live_of(&r, ks));
    assert_eq!(r.file_live[kw.0 as usize], FileLive::Always);
    assert_eq!(def(&m, &r, "K"), Def::Strong(e(ks, ki)));
}

#[test]
fn s5_5_entry_not_found_and_address() {
    let mut m = Model::new(A64);
    m.obj("a.o");
    let n = named(&mut m, "nosuch", 1);
    let r = link_with(&m, &Options { entry: Entry::Named(n), ..opts() });
    assert_eq!(ids(&r), vec!["entry-not-found"]);
    assert_eq!(r.entry, EntryPoint::Default);
    let num = Named { key: OptId::NONE, name: b"0x400100".as_slice().into(), arg: 1 };
    let r = link_with(&m, &Options { entry: Entry::Named(num), ..opts() });
    assert!(r.problems.is_empty());
    assert_eq!(r.entry, EntryPoint::Address(0x0040_0100));
}

// ---- §4.5 ------------------------------------------------------------------

#[test]
fn s4_5_common_targets() {
    // (common in main, member kind) -> extracted?
    // data definition: yes (R6 mc.o libLV.a: bfd, mold); function: no
    // (mc.o libLF.a); common in member: no (cfirst ignores commons);
    // weak data: no; a weak or function live definition does not block a
    // later data member (R6: wv.o mc.o libLV.a extracts).
    type Add = fn(&mut Model, FileId);
    let rows: [(&str, Add, bool); 4] = [
        (
            "data",
            |m, f| {
                m.data(f, "v", 4);
            },
            true,
        ),
        (
            "function",
            |m, f| {
                m.def(f, "v");
            },
            false,
        ),
        (
            "common",
            |m, f| {
                m.common(f, "v", 8, 8);
            },
            false,
        ),
        (
            "weak data",
            |m, f| {
                m.weak_data(f, "v", 4);
            },
            false,
        ),
    ];
    for (what, add, extracted) in rows {
        let mut m = Model::new(A64);
        let mc = m.obj("mc.o");
        m.common(mc, "v", 4, 4);
        let ar = m.archive();
        let lv = m.member(ar, "lv.o");
        add(&mut m, lv);
        let r = link(&m);
        assert_eq!(live_of(&r, lv), extracted, "{what}");
    }
    for blocker in [true, false] {
        let mut m = Model::new(A64);
        let wv = m.obj("wv.o");
        if blocker {
            m.weak_data(wv, "v", 4);
        } else {
            m.def(wv, "v");
        }
        let mc = m.obj("mc.o");
        m.common(mc, "v", 4, 4);
        let ar = m.archive();
        let lv = m.member(ar, "lv.o");
        m.data(lv, "v", 4);
        let r = link(&m);
        // A live strong definition (the function) is cfirst and blocks.
        assert_eq!(live_of(&r, lv), blocker, "blocker weak={blocker}");
    }
}

#[test]
fn s4_5_common_and_strong_reference_both_edges() {
    // mc.o has a common v and a.o a strong reference: first(v) is mc.o
    // (live), cfirst is the member with data.
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.undef(a, "v");
    let mc = m.obj("mc.o");
    m.common(mc, "v", 4, 4);
    let ar = m.archive();
    let lv = m.member(ar, "lv.o");
    let d = m.data(lv, "v", 4);
    let r = link(&m);
    assert!(live_of(&r, lv));
    assert_eq!(def(&m, &r, "v"), Def::Strong(e(lv, d)));
}

// ---- §4.6 ------------------------------------------------------------------

/// t2: m2.o libA.a libCB.a; t2b: m2.o libAB.a libC.a (finding 3). m2.o
/// references b; b.o references pick; a.o and c.o define pick.
fn t2(b_in_first: bool) -> (Model, FileId, FileId) {
    let mut m = Model::new(A64);
    let m2 = m.obj("m2.o");
    m.undef(m2, "b");
    let a1 = m.archive();
    let a = m.member(a1, "a.o");
    m.def(a, "pick");
    let b_member = |m: &mut Model, ar| {
        let b = m.member(ar, "b.o");
        m.def(b, "b");
        m.undef(b, "pick");
    };
    if b_in_first {
        b_member(&mut m, a1);
    }
    let a2 = m.archive();
    let c = m.member(a2, "c.o");
    m.def(c, "pick");
    if !b_in_first {
        b_member(&mut m, a2);
    }
    (m, a, c)
}

#[test]
fn s4_6_choice_fires_for_t2_not_t2b() {
    let o = Options { warn_backrefs: true, allow_multiple_definition: false, ..opts() };
    let (m, a, c) = t2(false);
    let r = link_with(&m, &o);
    assert!(live_of(&r, a) && !live_of(&r, c));
    assert_eq!(r.def[m.id("pick").0 as usize].entry().unwrap().file, a);
    let sim = r.gnu.as_ref().unwrap();
    assert!(!sim.loaded.contains(&a) && sim.loaded.contains(&c), "{sim:?}");
    assert!(ids(&r).contains(&"archive-choice"), "{:?}", r.problems);
    let (m, a, c) = t2(true);
    let r = link_with(&m, &o);
    let sim = r.gnu.as_ref().unwrap();
    assert!(sim.loaded.contains(&a) && !sim.loaded.contains(&c), "{sim:?}");
    assert!(!ids(&r).iter().any(|i| i.starts_with("archive-")), "{:?}", r.problems);
}

/// `-lgcc -lc -lgcc`: GNU ld takes `cas` from the second mention of
/// `libgcc.a`, mortar from the first. They are the same member, so there is
/// no choice to report; two different archives still get the warning.
#[test]
fn s4_6_same_member_twice_is_no_choice() {
    for same in [true, false] {
        let mut m = Model::new(A64);
        let main = m.obj("main.o");
        m.undef(main, "puts");
        let gcc1 = m.archive();
        let cas1 = m.member(gcc1, "cas.o");
        m.def(cas1, "cas");
        let libc = m.archive();
        let c = m.member(libc, "c.o");
        m.def(c, "puts");
        m.undef(c, "cas");
        let gcc2 = m.archive();
        let cas2 = m.member(gcc2, "cas.o");
        m.def(cas2, "cas");
        if same {
            m.same_source(cas2, cas1);
        }
        let r = link_with(&m, &Options { warn_backrefs: true, ..opts() });
        assert!(live_of(&r, cas1) && !live_of(&r, cas2));
        assert!(r.gnu.as_ref().unwrap().loaded.contains(&cas2));
        let fired = ids(&r).contains(&"archive-choice");
        assert_eq!(fired, !same, "{:?}", r.problems);
    }
}

#[test]
fn s4_6_nothing_runs_without_option() {
    let (m, _, _) = t2(false);
    let r = link(&m);
    assert!(r.gnu.is_none());
    assert!(!ids(&r).iter().any(|i| i.starts_with("archive-")));
}

#[test]
fn s4_6_backref_and_group() {
    // R2-like: a.o libKS.a uk.o: uk.o's reference comes after the
    // archive: GNU ld fails (archive-backref); in a group it does not.
    for grouped in [false, true] {
        let mut m = Model::new(A64);
        let a = m.obj("a.o");
        m.def(a, "main");
        let ar = m.archive();
        let ks = m.member(ar, "ks.o");
        m.def(ks, "K");
        let uk = m.obj("uk.o");
        m.undef(uk, "K");
        if grouped {
            m.set_group(ks, 0);
            m.set_group(uk, 0);
        }
        let r = link_with(&m, &Options { warn_backrefs: true, ..opts() });
        assert!(live_of(&r, ks));
        let sim = r.gnu.as_ref().unwrap();
        assert_eq!(sim.loaded.contains(&ks), grouped);
        assert_eq!(ids(&r).contains(&"archive-backref"), !grouped, "{:?}", r.problems);
    }
}

#[test]
fn s4_6_common_after_weak_makes_common() {
    // A common after a weak definition makes the key `common`: the next
    // archive pass loads a data definition.
    let mut m = Model::new(A64);
    let w = m.obj("wv.o");
    m.weak_data(w, "v", 4);
    let c = m.obj("mc.o");
    m.common(c, "v", 4, 4);
    let ar = m.archive();
    let lv = m.member(ar, "lv.o");
    m.data(lv, "v", 4);
    let r = link_with(&m, &Options { warn_backrefs: true, ..opts() });
    assert!(r.gnu.as_ref().unwrap().loaded.contains(&lv));
}

// ---- §6 --------------------------------------------------------------------

#[test]
fn s6_1_keeper_is_lowest_live_file() {
    let mut m = Model::new(A64);
    let main = m.obj("main.o");
    m.undef(main, "late");
    let ar = m.archive();
    let x = m.member(ar, "x.o");
    m.def(x, "late");
    m.comdat_def(x, "G", "tpl");
    let b = m.obj("b.o");
    let sb = m.comdat_def(b, "G", "tpl");
    let r = link(&m);
    let g = r.sections[r.section_base[x.0 as usize].start.0 as usize + 5].comdat.get().unwrap();
    assert_eq!(r.keepers.keeper[g.0 as usize], OptId::some(x));
    assert_eq!(r.keepers.copies, vec![(g, x), (g, b)]);
    let row = r.sections[(r.section_base[b.0 as usize].start.0 + sb) as usize];
    assert_eq!(row.state, SectionState::Dropped(DropReason::Comdat));
    let tpl = def(&m, &r, "tpl");
    assert_eq!(tpl.entry().unwrap().file, x);
}

#[test]
fn s6_1_section_symbol_signature_uses_section_name() {
    // The ELF adapter signs a group by its section symbol's section name;
    // two files with the same named section share the signature.
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    let b = m.obj("b.o");
    m.comdat_def(a, ".text.x", "x");
    m.comdat_def(b, ".text.x", "x");
    let r = link(&m);
    assert_eq!(r.keepers.copies.len(), 2);
    assert_eq!(r.keepers.copies[0].0, r.keepers.copies[1].0);
}

#[test]
fn s6_1_second_group_of_signature_in_one_file_dropped() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    let s1 = m.comdat_def(a, "G", "x");
    let s2 = m.comdat_def(a, "G", "y");
    let r = link(&m);
    let base = r.section_base[a.0 as usize].start.0;
    assert!(r.sections[(base + s1) as usize].is_kept());
    assert_eq!(r.sections[(base + s2) as usize].state, SectionState::Dropped(DropReason::Comdat));
    assert_eq!(def(&m, &r, "y"), Def::Undefined { weak: true });
}

#[test]
fn s6_2_demoted_mentions_bind_to_keeper() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    let b = m.obj("b.o");
    let sa = m.comdat_def(a, "G", "tpl");
    let _ = sa;
    m.comdat_def(b, "G", "tpl");
    let r = link(&m);
    let k = def(&m, &r, "tpl").entry().unwrap();
    assert_eq!(k.file, a);
    let f = r.props[m.id("tpl").0 as usize].flags;
    assert_ne!(f & flags::DEMOTED, 0);
    assert!(r.comdat_mismatch.is_empty());
}

#[test]
fn s6_3_comdat_mismatch_names_group_and_files() {
    // t10: the kept copy (gl.o) lacks hx, which only the dropped copy
    // defines.
    let mut m = Model::new(A64);
    let main = m.obj("m10.o");
    m.undef(main, "g");
    m.undef(main, "hx");
    let ar = m.archive();
    let gl = m.member(ar, "gl.o");
    m.comdat_def(gl, "G", "g");
    let gh = m.member(ar, "gh.o");
    let s = m.comdat_def(gh, "G", "g");
    m.sym(gh, "hx", STB_WEAK, STT_FUNC, SymbolSection::Section(SectionIndex(s)), 4, 4);
    m.def(gh, "only_gh");
    m.undef(gl, "only_gh");
    let r = link(&m);
    assert!(live_of(&r, gl) && live_of(&r, gh));
    assert_eq!(def(&m, &r, "hx"), Def::Undefined { weak: false });
    let [cm] = r.comdat_mismatch.as_slice() else { panic!("{:?}", r.comdat_mismatch) };
    assert_eq!(cm.key, m.id("hx"));
    assert_eq!(cm.keeper, gl);
    assert_eq!(cm.dropped.file, gh);
}

#[test]
fn s6_4_non_comdat_groups_are_not_merged() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    let b = m.obj("b.o");
    let sa = m.section(a, MSection::new(".text.g", SHT_PROGBITS, SHF_ALLOC));
    let sb = m.section(b, MSection::new(".text.g", SHT_PROGBITS, SHF_ALLOC));
    m.group(a, "G", 0, &[sa]);
    m.group(b, "G", 0, &[sb]);
    let r = link(&m);
    assert!(r.keepers.copies.is_empty());
    for (f, s) in [(a, sa), (b, sb)] {
        let row = r.sections[(r.section_base[f.0 as usize].start.0 + s) as usize];
        assert!(row.is_kept());
        assert!(row.comdat.is_none());
    }
    // An unknown flag bit is refused.
    let c = m.obj("c.o");
    let sc = m.section(c, MSection::new(".text.h", SHT_PROGBITS, SHF_ALLOC));
    m.group(c, "H", GRP_COMDAT | 2, &[sc]);
    assert_eq!(ids(&link(&m)), vec!["refused"]);
}

#[test]
fn s6_5_section_in_two_groups_is_malformed() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    let s = m.section(a, MSection::new(".text.g", SHT_PROGBITS, SHF_ALLOC));
    m.group(a, "G", GRP_COMDAT, &[s]);
    m.group(a, "H", GRP_COMDAT, &[s]);
    assert_eq!(ids(&link(&m)), vec!["malformed-input"]);
}

#[test]
fn s6_5_link_order_follows_dropped_section() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    let b = m.obj("b.o");
    m.comdat_def(a, "G", "x");
    let sb = m.comdat_def(b, "G", "x");
    let mut lo = MSection::new(".meta", SHT_PROGBITS, SHF_ALLOC | SHF_LINK_ORDER);
    lo.link = sb;
    let l = m.section(b, lo);
    let r = link(&m);
    let row = r.sections[(r.section_base[b.0 as usize].start.0 + l) as usize];
    assert_eq!(row.state, SectionState::Dropped(DropReason::Comdat));
}

/// A refusal of a section's content or fate applies to kept sections only:
/// the same unsupported section in a dropped copy is dropped with it,
/// while structural faults still refuse (§10).
#[test]
fn s10_refusal_only_for_kept_sections() {
    for keep_b in [false, true] {
        let mut m = Model::new(A64);
        let a = m.obj("a.o");
        let b = m.obj("b.o");
        if !keep_b {
            m.comdat_def(a, "G", "x");
        }
        // With `keep_b`, b's copy is the only one: kept, so it is refused.
        let sb = m.comdat_def(b, "G", "x");
        let odd = m.section(b, MSection::new(".odd", 0x6fff_4c00, SHF_ALLOC));
        let groups = &mut m.files[b.0 as usize].groups;
        groups.members.push(odd);
        groups.groups.last_mut().unwrap().len += 1;
        let r = link(&m);
        let row = r.sections[(r.section_base[b.0 as usize].start.0 + odd) as usize];
        if keep_b {
            assert_eq!(ids(&r), vec!["refused"]);
        } else {
            assert!(r.problems.is_empty(), "{:?}", r.problems);
            assert_eq!(row.state, SectionState::Dropped(DropReason::Comdat));
        }
        let _ = sb;
    }
    // `-S` drops `.zdebug*`, so it is not refused.
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.section(a, MSection::new(".zdebug_info", SHT_PROGBITS, 0));
    assert_eq!(ids(&link(&m)), vec!["refused"]);
    assert!(link_with(&m, &Options { strip_debug: true, ..opts() }).problems.is_empty());
}

/// Two relocation sections for one kept section are refused by name: no
/// reference applies both (GNU ld drops the second `SHT_RELA`, lld refuses,
/// mold asserts or drops one; WP-022.md). For a dropped section they are
/// dropped with it.
#[test]
fn s10_second_reloc_section_refused() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    let mut r1 = MSection::new(".rela.data", SHT_RELA, 0);
    r1.info = DATA;
    m.section(a, r1);
    let mut r2 = MSection::new(".rela.data.2", SHT_RELA, 0);
    r2.info = DATA;
    let second = m.section(a, r2);
    let r = link(&m);
    assert_eq!(
        r.problems.iter().map(|p| p.what.clone()).collect::<Vec<_>>(),
        vec![What::Section {
            file: a,
            section: SectionIndex(second),
            why: SectionRefusal::SecondReloc(DATA)
        }]
    );
    // In a dropped copy: nothing.
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    let b = m.obj("b.o");
    m.comdat_def(a, "G", "x");
    let sb = m.comdat_def(b, "G", "x");
    for name in [".rela.g1", ".rela.g2"] {
        let mut rel = MSection::new(name, SHT_RELA, 0);
        rel.info = sb;
        m.section(b, rel);
    }
    assert!(link(&m).problems.is_empty());
}

#[test]
fn s6_6_linkonce_is_refused() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.section(a, MSection::new(".gnu.linkonce.t.foo", SHT_PROGBITS, SHF_ALLOC));
    let r = link(&m);
    assert!(matches!(r.problems[0].what, What::Section { why: SectionRefusal::Linkonce, .. }));
}

// ---- §7 --------------------------------------------------------------------

#[test]
fn s7_1_wrap_undefined_mentions_only() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.undef(a, "f");
    let fi = m.def(a, "g");
    let b = m.obj("b.o");
    let wi = m.def(b, "__wrap_f");
    let di = m.def(b, "f");
    m.undef(b, "__real_f");
    let _ = fi;
    let o = Options {
        wrap: vec![Wrap { sym: m.key("f"), wrap: m.key("__wrap_f"), real: m.key("__real_f") }],
        ..opts()
    };
    let r = link_with(&m, &o);
    assert_eq!(r.redirect[m.id("f").0 as usize], m.id("__wrap_f"));
    assert_eq!(r.redirect[m.id("__real_f").0 as usize], m.id("f"));
    assert_eq!(def(&m, &r, "__wrap_f"), Def::Strong(e(b, wi)));
    // The definition of f stays f's (definitions are not redirected).
    assert_eq!(def(&m, &r, "f"), Def::Strong(e(b, di)));
}

#[test]
fn s7_1_real_extracts_target() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.undef(a, "__real_f");
    let ar = m.archive();
    let x = m.member(ar, "f.o");
    m.def(x, "f");
    let o = Options {
        wrap: vec![Wrap { sym: m.key("f"), wrap: m.key("__wrap_f"), real: m.key("__real_f") }],
        ..opts()
    };
    let r = link_with(&m, &o);
    assert!(live_of(&r, x));
}

#[test]
fn s7_1_wrap_of_a_dropped_comdat_copy_is_refused() {
    // WP-153: the references disagree on the calls in a file whose copy
    // of SYM's group is dropped; one copy is accepted, a second live one
    // refused, an unextracted member's copy is no copy.
    let wrap = |m: &mut Model| Options {
        wrap: vec![Wrap {
            sym: m.key("_Z1fv"),
            wrap: m.key("__wrap__Z1fv"),
            real: m.key("__real__Z1fv"),
        }],
        ..opts()
    };
    let refused = |r: &Resolved| {
        r.problems.iter().find_map(|p| match p.what {
            What::WrapComdat { dropped, kept, .. } => Some((dropped.file, kept)),
            _ => None,
        })
    };
    // One copy.
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.comdat_def(a, "_Z1fv", "_Z1fv");
    let o = wrap(&mut m);
    assert_eq!(refused(&link_with(&m, &o)), None);
    // Two live copies: b.o's is dropped.
    let b = m.obj("b.o");
    m.comdat_def(b, "_Z1fv", "_Z1fv");
    let o = wrap(&mut m);
    assert_eq!(refused(&link_with(&m, &o)), Some((b, a)));
    // Without --wrap the same link is no problem.
    assert!(link(&m).problems.iter().all(|p| !matches!(p.what, What::WrapComdat { .. })));
    // The second copy in an archive member nothing extracts.
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.comdat_def(a, "_Z1fv", "_Z1fv");
    let ar = m.archive();
    let x = m.member(ar, "b.o");
    m.comdat_def(x, "_Z1fv", "_Z1fv");
    m.def(x, "b");
    let o = wrap(&mut m);
    assert_eq!(refused(&link_with(&m, &o)), None);
    // Extracted for another symbol, it is a live copy.
    m.undef(a, "b");
    let o = wrap(&mut m);
    assert_eq!(refused(&link_with(&m, &o)), Some((x, a)));
}

fn defsym(m: &mut Model, sym: &str, value: DefsymValue, arg: u32) -> Defsym {
    Defsym { sym: m.key(sym), value, arg, text: sym.as_bytes().into() }
}

fn defsym_refusals(r: &Resolved) -> Vec<DefsymWhy> {
    r.problems
        .iter()
        .filter_map(|p| match p.what {
            What::Defsym { why, .. } => Some(why),
            _ => None,
        })
        .collect()
}

// spec: resolve.md §7.2 (WP-153): rank 0 over every definition; the
// symbol is a root; an option's symbol extracts no member defining it.
#[test]
fn s7_2_defsym_beats_every_definition_and_its_symbol_is_a_root() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.def(a, "s");
    m.undef(a, "t");
    let ar = m.archive();
    let x = m.member(ar, "b.o");
    m.def(x, "b");
    let y = m.member(ar, "t.o");
    m.def(y, "t");
    let b = m.key("b");
    let o = Options {
        defsym: vec![
            defsym(&mut m, "s", DefsymValue::Number(0x10), 1),
            defsym(&mut m, "t", DefsymValue::Alias(b), 2),
        ],
        ..opts()
    };
    let r = link_with(&m, &o);
    assert_eq!(def(&m, &r, "s"), Def::Defsym(DefsymValue::Number(0x10)));
    assert_eq!(def(&m, &r, "t"), Def::Defsym(DefsymValue::Alias(b)));
    // b.o is extracted for the root b; t.o, which defines t, is not.
    assert!(r.file_live[x.0 as usize].is_live());
    assert!(!r.file_live[y.0 as usize].is_live());
    assert!(matches!(def(&m, &r, "b"), Def::Strong(_)));
    assert!(defsym_refusals(&r).is_empty(), "{:?}", r.problems);
}

// spec: resolve.md §7.2: the symbol named must be defined in the output:
// undefined, shared-only and another --defsym's are refused by name.
#[test]
fn s7_2_defsym_of_an_undefined_shared_or_defsym_symbol_is_refused() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.undef(a, "f");
    let lib = m.shlib("libb.so", false);
    m.def(lib, "sh");
    m.undef(a, "sh");
    let (nosuch, sh, c) = (m.key("nosuch"), m.key("sh"), m.key("c"));
    let o = Options {
        defsym: vec![
            defsym(&mut m, "a1", DefsymValue::Alias(nosuch), 1),
            defsym(&mut m, "a2", DefsymValue::Offset(sh, 4), 2),
            defsym(&mut m, "c", DefsymValue::Number(1), 3),
            defsym(&mut m, "a3", DefsymValue::Alias(c), 4),
        ],
        ..opts()
    };
    let r = link_with(&m, &o);
    let mut got = defsym_refusals(&r);
    got.sort();
    assert_eq!(got, vec![DefsymWhy::Undefined(nosuch), DefsymWhy::Shared(sh), DefsymWhy::Chain(c)]);
    // A B only a shared object defines and no object mentions is still
    // named as the shared object's (a root binds no shared definition).
    let lib2 = m.shlib("libc2.so", false);
    m.def(lib2, "only");
    let only = m.key("only");
    let o = Options { defsym: vec![defsym(&mut m, "a5", DefsymValue::Alias(only), 1)], ..opts() };
    assert_eq!(defsym_refusals(&link_with(&m, &o)), vec![DefsymWhy::Shared(only)]);
    // A symbol aliasing itself is a chain too.
    let a4 = m.key("a4");
    let o = Options { defsym: vec![defsym(&mut m, "a4", DefsymValue::Alias(a4), 1)], ..opts() };
    assert_eq!(defsym_refusals(&link_with(&m, &o)), vec![DefsymWhy::Chain(a4)]);
}

// spec: resolve.md §7.2: a number wider than the target's address.
#[test]
fn s7_2_defsym_number_wider_than_the_address_is_refused_on_i386() {
    for (machine, refused) in [(Machine::I386, true), (Machine::X86_64, false)] {
        let mut m = Model::new(machine);
        m.obj("a.o");
        let wide = DefsymValue::Number(0x1_2345_6789);
        let o = Options {
            defsym: vec![
                defsym(&mut m, "k", wide, 1),
                defsym(&mut m, "j", DefsymValue::Number(0xffff_fff0), 2),
            ],
            ..opts()
        };
        let want = if refused { vec![DefsymWhy::TooWide(0x1_2345_6789)] } else { vec![] };
        assert_eq!(defsym_refusals(&link_with(&m, &o)), want, "{machine:?}");
    }
}

// ---- §8 --------------------------------------------------------------------

#[test]
fn s8_1_provided_only_when_referenced() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.undef(a, "_end");
    m.weak_undef(a, "__start_mysec");
    m.key("__bss_start");
    let r = link(&m);
    assert!(matches!(def(&m, &r, "_end"), Def::Provide { conditional: false, .. }));
    assert_eq!(
        def(&m, &r, "__start_mysec"),
        Def::Provide { sym: Provided::Start, conditional: true }
    );
    assert!(def(&m, &r, "__bss_start").is_undefined());
}

#[test]
fn s8_1_table() {
    for machine in [Machine::AArch64, Machine::X86_64, Machine::I386] {
        for (n, row) in TABLE.iter().enumerate() {
            let name = std::str::from_utf8(row.name).unwrap();
            let mut m = Model::new(machine);
            let a = m.obj("a.o");
            m.undef(a, name);
            let r = link(&m);
            let d = def(&m, &r, name);
            let want = match row.when {
                When::Always | When::Executable | When::Conditional => true,
                When::StaticExecutable => match machine {
                    Machine::I386 => name.starts_with("__rel_iplt"),
                    _ => name.starts_with("__rela_iplt"),
                },
                When::Dynamic | When::Later => false,
                When::AArch64 => machine == Machine::AArch64,
                When::X86_64Conditional => machine == Machine::X86_64,
            };
            if want {
                assert_eq!(
                    d,
                    Def::Provide {
                        sym: Provided::Table(n as u8),
                        conditional: matches!(
                            row.when,
                            When::Conditional | When::X86_64Conditional
                        )
                    },
                    "{name} on {machine:?}"
                );
            } else {
                assert!(d.is_undefined(), "{name} on {machine:?}: {d:?}");
            }
        }
    }
    // A dynamic output has _DYNAMIC.
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.undef(a, "_DYNAMIC");
    let r = link_with(&m, &Options { output: Output { pie: true, dynamic: true }, ..opts() });
    assert!(matches!(def(&m, &r, "_DYNAMIC"), Def::Provide { .. }));
    // So does a static-pie (dyn.md D-2, D-32; WP-052), and a static
    // non-PIE output does not.
    let r = link_with(&m, &Options { output: Output { pie: true, dynamic: false }, ..opts() });
    assert!(matches!(def(&m, &r, "_DYNAMIC"), Def::Provide { .. }));
    let r = link_with(&m, &Options { output: Output { pie: false, dynamic: false }, ..opts() });
    assert!(def(&m, &r, "_DYNAMIC").is_undefined());
}

/// Under `-static-pie` the IPLT range names stay undefined: the
/// `IRELATIVE` relocations are in `.rela.dyn` (GNU ld and lld leave them
/// undefined, mold makes them absolute 0; WP-022.md, "`__rela_iplt_*`").
#[test]
fn s8_1_no_iplt_range_under_static_pie() {
    for (machine, name) in [
        (Machine::AArch64, "__rela_iplt_start"),
        (Machine::X86_64, "__rela_iplt_end"),
        (Machine::I386, "__rel_iplt_start"),
    ] {
        let mut m = Model::new(machine);
        let a = m.obj("a.o");
        m.weak_undef(a, name);
        let pie = Options { output: Output { pie: true, dynamic: false }, ..opts() };
        assert!(def(&m, &link_with(&m, &pie), name).is_undefined(), "{name}");
        assert!(matches!(def(&m, &link(&m), name), Def::Provide { .. }), "{name}");
    }
}

#[test]
fn s8_2_object_definition_wins() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.undef(a, "etext");
    let b = m.obj("b.o");
    let i = m.def(b, "etext");
    let r = link(&m);
    assert_eq!(def(&m, &r, "etext"), Def::Strong(e(b, i)));
    assert!(r.problems.is_empty());
}

#[test]
fn s8_2_forced_names_refused() {
    let mut m = Model::new(A64);
    let b = m.obj("b.o");
    let i = m.data(b, "_end", 4);
    let j = m.def(b, "__bss_end__");
    let r = link(&m);
    let whats: Vec<&What> = r.problems.iter().map(|p| &p.what).collect();
    assert_eq!(
        whats,
        vec![
            &What::LinkerSymbolRedefined { key: m.id("_end"), entry: e(b, i) },
            &What::LinkerSymbolRedefined { key: m.id("__bss_end__"), entry: e(b, j) },
        ]
    );
    // __bss_end__ is AArch64's only.
    let mut m = Model::new(Machine::X86_64);
    let b = m.obj("b.o");
    m.def(b, "__bss_end__");
    assert!(link(&m).problems.is_empty());
}

// ---- §9.4 ------------------------------------------------------------------

#[test]
fn s9_4_second_lookup_in_objects() {
    // R12: a reference to foo@V2 binds to a definition foo@@V2, and to a
    // foo@V2 definition; a plain reference binds to foo@@V2 only.
    let mut m = Model::new(A64);
    let mv2 = m.obj("mv2.o");
    m.undef(mv2, "foo@V2");
    let mf = m.obj("mf.o");
    m.undef(mf, "foo");
    let dloc = m.obj("dloc.o");
    let d = m.def(dloc, "foo@@V2");
    let r = link(&m);
    assert_eq!(def(&m, &r, "foo@V2"), Def::Strong(e(dloc, d)));
    assert_eq!(def(&m, &r, "foo"), Def::Strong(e(dloc, d)));
    let mut m = Model::new(A64);
    let mf = m.obj("mf.o");
    m.undef(mf, "foo");
    let dv = m.obj("dv.o");
    m.def(dv, "foo@V2");
    let r = link(&m);
    assert!(def(&m, &r, "foo").is_undefined());
}

// ---- §10 -------------------------------------------------------------------

#[test]
#[allow(clippy::too_many_lines)] // one row per line of §10's table
fn s10_classification_table() {
    use SectionKind as K;
    type Want = Result<(SectionKind, SectionState), ()>;
    let kept = SectionState::Kept;
    let dropped = SectionState::Dropped;
    let rows: Vec<(MSection, Want)> = vec![
        (MSection::new(".rela.text", SHT_RELA, 0), Ok((K::Reloc, dropped(DropReason::Consumed)))),
        (
            MSection::new(".x", SHT_PROGBITS, SHF_ALLOC | SHF_EXCLUDE),
            Ok((K::Excluded, dropped(DropReason::Excluded))),
        ),
        (
            MSection::new(".note.GNU-stack", SHT_PROGBITS, 0),
            Ok((K::Stack, dropped(DropReason::Consumed))),
        ),
        (MSection::new(".note.gnu.property", SHT_NOTE, SHF_ALLOC), Ok((K::Note, kept))),
        // WP-135: an input's build id (`ld -r --build-id`) is discarded
        // whatever its type; the output's is the linker's own (I-69).
        (
            MSection::new(".note.gnu.build-id", SHT_NOTE, SHF_ALLOC),
            Ok((K::Excluded, dropped(DropReason::Excluded))),
        ),
        (
            MSection::new(".note.gnu.build-id", SHT_PROGBITS, 0),
            Ok((K::Excluded, dropped(DropReason::Excluded))),
        ),
        (MSection::new(".note.gnu.build-idx", SHT_NOTE, SHF_ALLOC), Ok((K::Note, kept))),
        (
            MSection::new(".llvm_addrsig", 0x6fff_4c03, 0),
            Ok((K::Excluded, dropped(DropReason::Consumed))),
        ),
        // As compilers emit it, with `SHF_EXCLUDE`: that row matches first.
        (
            MSection::new(".llvm_addrsig", 0x6fff_4c03, SHF_EXCLUDE),
            Ok((K::Excluded, dropped(DropReason::Excluded))),
        ),
        // WP-034: consumed, its specifiers not honoured (GNU ld, mold).
        (
            MSection {
                entsize: 1,
                ..MSection::new(".deplibs", 0x6fff_4c04, SHF_MERGE | SHF_STRINGS)
            },
            Ok((K::Excluded, dropped(DropReason::Consumed))),
        ),
        (MSection::new(".llvm.odrtab", 0x6fff_4c00, 0), Err(())),
        (
            MSection::new(".gnu.lto_.x", SHT_PROGBITS, SHF_EXCLUDE),
            Ok((K::Excluded, dropped(DropReason::Excluded))),
        ),
        (
            MSection::new(".gnu.warning", SHT_PROGBITS, 0),
            Ok((K::Excluded, dropped(DropReason::Consumed))),
        ),
        (
            MSection::new(".gnu.warning.gets", SHT_PROGBITS, 0),
            Ok((K::Excluded, dropped(DropReason::Consumed))),
        ),
        (MSection::new(".gnu.linkonce.t.f", SHT_PROGBITS, SHF_ALLOC), Err(())),
        (MSection::new(".zdebug_info", SHT_PROGBITS, 0), Err(())),
        (MSection::new(".eh_frame", SHT_PROGBITS, SHF_ALLOC), Ok((K::EhFrame, kept))),
        (
            MSection {
                entsize: 1,
                ..MSection::new(".rodata.str1.1", SHT_PROGBITS, SHF_ALLOC | SHF_MERGE | SHF_STRINGS)
            },
            Ok((K::Merge, kept)),
        ),
        (MSection::new(".rodata.m0", SHT_PROGBITS, SHF_ALLOC | SHF_MERGE), Ok((K::Regular, kept))),
        (MSection::new(".debug_info", SHT_PROGBITS, 0), Ok((K::Debug, kept))),
        (MSection::new(".note.x", SHT_NOTE, SHF_ALLOC), Ok((K::Note, kept))),
        (MSection::new(".tbss", SHT_NOBITS, SHF_ALLOC | SHF_WRITE), Ok((K::Nobits, kept))),
        (MSection::new(".init_array", 14, SHF_ALLOC | SHF_WRITE), Ok((K::Regular, kept))),
        // layout.md §20.7, L-35: consumed (WP-027d; refused before).
        (
            MSection::new(".ARM.attributes", 0x7000_0003, 0),
            Ok((K::Excluded, dropped(DropReason::Consumed))),
        ),
        (MSection::new(".dynamic", 6, SHF_ALLOC), Err(())),
        (MSection::new(".weird", 0x6000_0001, SHF_ALLOC), Err(())),
        (MSection::new(".flag", SHT_PROGBITS, SHF_ALLOC | 0x0800_0000), Err(())),
        (MSection::new(".purecode", SHT_PROGBITS, SHF_ALLOC | 0x2000_0000), Ok((K::Regular, kept))),
        (
            MSection::new(".retain", SHT_PROGBITS, SHF_ALLOC | SHF_GNU_RETAIN),
            Ok((K::Regular, kept)),
        ),
    ];
    for (s, want) in rows {
        let name = String::from_utf8_lossy(&s.name).into_owned();
        let mut m = Model::new(A64);
        let a = m.obj("a.o");
        let i = m.section(a, s);
        let r = link(&m);
        let row = r.sections[(r.section_base[a.0 as usize].start.0 + i) as usize];
        match want {
            Ok((kind, state)) => {
                assert_eq!((row.kind, row.state), (kind, state), "{name}: {:?}", r.problems);
                let warns = usize::from(name == ".gnu.warning");
                assert_eq!(r.problems.len(), warns, "{name}: {:?}", r.problems);
            }
            Err(()) => assert_eq!(ids(&r), vec!["refused"], "{name}"),
        }
        if name == ".retain" {
            assert_eq!(r.retained, vec![(a, SectionIndex(i))]);
        }
        if name == ".gnu.warning.gets" {
            assert!(r.gnu_warning_syms.is_empty(), "no key gets");
        }
    }
    // -S strips debug sections; a reloc section is recorded on its target.
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    let d = m.section(a, MSection::new(".debug_info", SHT_PROGBITS, 0));
    let mut rela = MSection::new(".rela.text", SHT_RELA, 0);
    rela.info = TEXT;
    let ri = m.section(a, rela);
    let r = link_with(&m, &Options { strip_debug: true, ..opts() });
    let base = r.section_base[a.0 as usize].start.0 as usize;
    assert_eq!(r.sections[base + d as usize].state, SectionState::Dropped(DropReason::Stripped));
    assert_eq!(r.sections[base + TEXT as usize].reloc, SectionIndex(ri));
    assert_eq!(r.sections[base + BSS as usize].kind, SectionKind::Nobits);
    assert_eq!(r.sections[base + TDATA as usize].kind, SectionKind::Regular);
}

/// A Mason container is `Meta`, `Dropped(Consumed)`, and listed; a
/// section of that name that is not one takes the other rows and is
/// reported once per file, never by its position in the table.
// spec: meta.md T-MT-6, MT-1, MT-4, MT-6; resolve.md §10 (amendment A-1)
#[test]
fn s10_mason_containers() {
    const META: &str = ".mason.meta";
    let consumed = SectionState::Dropped(DropReason::Consumed);
    // Two containers in one object (MT-4), not at a fixed index.
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    let i = m.section(a, MSection::new(META, SHT_PROGBITS, SHF_EXCLUDE));
    m.section(a, MSection::new(".debug_info", SHT_PROGBITS, 0));
    let j = m.section(a, MSection::new(META, SHT_PROGBITS, SHF_EXCLUDE));
    m.obj("b.o");
    let r = link(&m);
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    let base = r.section_base[a.0 as usize].start.0 as usize;
    for k in [i, j] {
        let row = r.sections[base + k as usize];
        assert_eq!((row.kind, row.state), (SectionKind::Meta, consumed));
        assert!(!row.kind.carries_output() && !row.kind.is_placed());
    }
    assert_eq!(r.meta, vec![(a, SectionIndex(i)), (a, SectionIndex(j))]);

    // The non-container variants: the rows below, one warning per file.
    let variants = [
        (SHT_PROGBITS, SHF_ALLOC, SectionKind::Regular, SectionState::Kept),
        (SHT_PROGBITS, 0, SectionKind::Regular, SectionState::Kept),
        (
            SHT_PROGBITS,
            SHF_ALLOC | SHF_EXCLUDE,
            SectionKind::Excluded,
            SectionState::Dropped(DropReason::Excluded),
        ),
        (SHT_NOTE, SHF_EXCLUDE, SectionKind::Excluded, SectionState::Dropped(DropReason::Excluded)),
        (SHT_NOTE, 0, SectionKind::Note, SectionState::Kept),
    ];
    for (sh_type, flags, kind, state) in variants {
        let mut m = Model::new(A64);
        let a = m.obj("a.o");
        let i = m.section(a, MSection::new(META, sh_type, flags));
        // A second one in the same file: still one warning.
        m.section(a, MSection::new(META, sh_type, flags));
        let b = m.obj("b.o");
        m.section(b, MSection::new(META, sh_type, flags));
        let r = link(&m);
        let row = r.sections[(r.section_base[a.0 as usize].start.0 + i) as usize];
        assert_eq!((row.kind, row.state), (kind, state), "{sh_type} {flags:#x}");
        assert!(r.meta.is_empty());
        assert_eq!(ids(&r), ["meta-malformed", "meta-malformed"], "{sh_type} {flags:#x}");
        let files: Vec<_> = r
            .problems
            .iter()
            .map(|p| match p.what {
                What::MetaNotContainer { file, .. } => file,
                _ => panic!("{p:?}"),
            })
            .collect();
        assert_eq!(files, [a, b]);
        assert!(r.problems.iter().all(|p| p.severity() == mortar_resolve::Severity::Warning));
    }
}

/// A section with MT-1's name, type and flags that a relocation section
/// applies to is not a container (MT-2): not listed, the `SHF_EXCLUDE`
/// row, its relocations dropped with it (not recorded: WP-194), one
/// warning per file; the
/// file's other container is still one.
// spec: meta.md MT-2, MT-6 (WP-099: hod writes `.rela.mason.meta` for an
// initializer with a pointer)
#[test]
fn s10_relocated_container_is_not_one() {
    const META: &str = ".mason.meta";
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    let i = m.section(a, MSection::new(META, SHT_PROGBITS, SHF_EXCLUDE));
    let mut rela = MSection::new(".rela.mason.meta", SHT_RELA, 0);
    rela.info = i;
    m.section(a, rela);
    let j = m.section(a, MSection::new(META, SHT_PROGBITS, SHF_EXCLUDE));
    let k = m.section(a, MSection::new(META, SHT_PROGBITS, SHF_EXCLUDE));
    let mut rela2 = MSection::new(".rela.mason.meta", SHT_RELA, 0);
    rela2.info = k;
    m.section(a, rela2);
    let r = link(&m);
    let base = r.section_base[a.0 as usize].start.0 as usize;
    let row = r.sections[base + i as usize];
    assert_eq!(
        (row.kind, row.state, row.reloc),
        (SectionKind::Excluded, SectionState::Dropped(DropReason::Excluded), SectionIndex(0))
    );
    // An `Excluded` target records no relocation section: its records
    // are nothing (§10, WP-194).
    assert_eq!(r.sections[base + k as usize].kind, SectionKind::Excluded);
    assert_eq!(r.meta, vec![(a, SectionIndex(j))]);
    assert_eq!(ids(&r), ["meta-malformed"]);
    assert!(matches!(
        r.problems[0].what,
        What::MetaNotContainer { file, section, relocated: true } if file == a && section.0 == i
    ));
    assert_eq!(r.problems[0].severity(), mortar_resolve::Severity::Warning);
}

#[test]
fn s10_slim_lto_refused() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.common(a, "__gnu_lto_slim", 1, 1);
    m.section(a, MSection::new(".gnu.lto_.main", SHT_PROGBITS, SHF_EXCLUDE));
    let r = link(&m);
    assert!(r.problems.iter().any(|p| p.what == What::LtoSlim { file: a }));
}

#[test]
fn s10_section_base_is_prefix_sum_over_live_objects() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    let ar = m.archive();
    let x = m.member(ar, "x.o");
    m.def(x, "unused");
    let b = m.obj("b.o");
    m.section(b, MSection::new(".extra", SHT_PROGBITS, SHF_ALLOC));
    let r = link(&m);
    assert_eq!(r.section_base[a.0 as usize].len(), 5);
    assert_eq!(r.section_base[x.0 as usize].len(), 0);
    assert_eq!(r.section_base[b.0 as usize].start.0, 5);
    assert_eq!(r.section_base[b.0 as usize].len(), 6);
    assert_eq!(r.sections.len(), 11);
    let _ = (FileKind::Object, STT_NOTYPE, BSS);
}

#[test]
fn s1_2_shared_objects_are_no_longer_refused() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.undef(a, "f");
    let lib = m.shlib("libf.so", false);
    m.def(lib, "f");
    let r = link(&m);
    assert!(ids(&r).is_empty(), "{:?}", ids(&r));
    assert!(matches!(def(&m, &r, "f"), Def::Shared(_)));
    let _ = Pool::sequential();
}

/// §10, "Relocations of a dropped section" (WP-194): a relocation section
/// of the form the machine does not use (`SHT_REL` on AArch64 and x86-64,
/// `SHT_RELA` on i386) is refused by name when its target is placed, has
/// no target, or is a consumed section whose records a phase reads
/// (`.debug_gnu_pubnames` under `--gdb-index`); one whose target has no
/// place (`SHF_EXCLUDE`: Clang's `.rel.llvm.call-graph-profile`; a
/// discarded COMDAT copy; a `.debug_*` section under `-S`) is dropped
/// with it, and an `Excluded` target records no relocation section of
/// either form: its records are nothing.
#[test]
fn s10_other_rel_form_dropped_with_a_target_without_place() {
    use mortar_elf::consts::{SHT_LLVM_CALL_GRAPH_PROFILE, SHT_REL};
    let row = |r: &Resolved, f: FileId, s: u32| {
        r.sections[(r.section_base[f.0 as usize].start.0 + s) as usize]
    };
    for (machine, other, native) in [
        (Machine::AArch64, SHT_REL, SHT_RELA),
        (Machine::X86_64, SHT_REL, SHT_RELA),
        (Machine::I386, SHT_RELA, SHT_REL),
    ] {
        let refused = |r: &Resolved, f: FileId, s: u32| {
            assert_eq!(
                r.problems.iter().map(|p| p.what.clone()).collect::<Vec<_>>(),
                vec![What::Section {
                    file: f,
                    section: SectionIndex(s),
                    why: SectionRefusal::RelForm(other)
                }],
                "{machine:?}"
            );
        };
        let rel = |name: &str, sh_type: u32, info: u32| {
            let mut s = MSection::new(name, sh_type, 0);
            s.info = info;
            s
        };
        // A placed target: refused by name.
        let mut m = Model::new(machine);
        let a = m.obj("a.o");
        let s = m.section(a, rel(".rel.data", other, DATA));
        refused(&link(&m), a, s);
        // No target (`sh_info` 0): refused.
        let mut m = Model::new(machine);
        let a = m.obj("a.o");
        let s = m.section(a, rel(".rel.none", other, 0));
        refused(&link(&m), a, s);
        // The call-graph profile (`SHF_EXCLUDE`), and any `SHF_EXCLUDE`
        // section: nothing, of either form; the target records none.
        for flags in [SHF_EXCLUDE, 0] {
            for form in [other, native] {
                let mut m = Model::new(machine);
                let a = m.obj("a.o");
                let p = m.section(
                    a,
                    MSection::new(".llvm.call-graph-profile", SHT_LLVM_CALL_GRAPH_PROFILE, flags),
                );
                m.section(a, rel(".rel.llvm.call-graph-profile", form, p));
                let x = m.section(a, MSection::new(".foo", SHT_PROGBITS, SHF_EXCLUDE | SHF_ALLOC));
                m.section(a, rel(".rel.foo", form, x));
                let r = link(&m);
                assert!(r.problems.is_empty(), "{machine:?} {flags:#x}: {:?}", r.problems);
                let reason = if flags == 0 { DropReason::Consumed } else { DropReason::Excluded };
                assert_eq!(row(&r, a, p).state, SectionState::Dropped(reason));
                assert_eq!(row(&r, a, p).reloc, SectionIndex(0));
                assert_eq!(row(&r, a, x).reloc, SectionIndex(0));
            }
        }
        // A discarded COMDAT copy: dropped with it (its relocation section
        // outside the group, as the in-group one always was).
        let mut m = Model::new(machine);
        let a = m.obj("a.o");
        let b = m.obj("b.o");
        m.comdat_def(a, "G", "x");
        let sb = m.comdat_def(b, "G", "x");
        m.section(b, rel(".rel.text.G", other, sb));
        let r = link(&m);
        assert!(r.problems.is_empty(), "{machine:?}: {:?}", r.problems);
        assert_eq!(row(&r, b, sb).reloc, SectionIndex(0));
        // `.debug_info` under `-S`: dropped with it; without, refused.
        let mut m = Model::new(machine);
        let a = m.obj("a.o");
        let d = m.section(a, MSection::new(".debug_info", SHT_PROGBITS, 0));
        let s = m.section(a, rel(".rel.debug_info", other, d));
        assert!(link_with(&m, &Options { strip_debug: true, ..opts() }).problems.is_empty());
        refused(&link(&m), a, s);
        // `.debug_gnu_pubnames` consumed by `--gdb-index`: the index reads
        // its records, so the other form stays refused.
        let mut m = Model::new(machine);
        let a = m.obj("a.o");
        let d = m.section(a, MSection::new(".debug_gnu_pubnames", SHT_PROGBITS, 0));
        let s = m.section(a, rel(".rel.debug_gnu_pubnames", other, d));
        let r = link_with(&m, &Options { consume_gnu_pubnames: true, ..opts() });
        assert_eq!(row(&r, a, d).state, SectionState::Dropped(DropReason::Consumed));
        refused(&r, a, s);
    }
}

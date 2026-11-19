//! WP-255d5: `mortar::update::file_scan::scan_of` on an EDITED file (ruling
//! item 6, "After 255d5's second hand-back").
//!
//! A v0 -> v1 pair of static C++ links (`main.cc`, `b.cc` unchanged,
//! `a.cc` edited: v1 adds GOT references (`-fPIC`) to `b.cc`'s data, a TLS
//! access, a call to `b.cc`'s function, and changes a `try`/`throw`).
//! The update's inputs come from their producers: `changed::u5` (the
//! join's U4-U5 over path (A)'s front of v1), `live`, `front_of` with
//! `Ids::Update`, `changed_facts`, `split_of`. Then:
//! - `scan_of`'s demands equal the full link's `Front::incr5.demands` of
//!   `a.cc.o`, matched by identity (a global by name, a local by its
//!   symbol index);
//! - `defined_facts` of every global `a.cc.o` defines equals, packed, the
//!   full link's `Incr5::facts` of that key, by name.
//!
//! On AArch64 and x86-64, under `-static -g --gc-sections` (the
//! operator's configuration) and plain `-static`. One test per fall-back
//! word that a static pair can reach: `wrap`, `stored-reloc`, `facts` (a
//! new key; an undefined key), `weak-dynamic`.
#![allow(clippy::too_many_lines, reason = "one comparison after the other")]

mod support;

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::PathBuf;

use mortar::front::{Env, Front, front, output_kind};
use mortar::timing::Timer;
use mortar::update::file_front::{FrontCache, Ids, front_of};
use mortar::update::file_scan::{Fail, defined_facts, scan_of_why};
use mortar::update::file_split::{SectionFact, changed_facts, split_of};
use mortar_core::demands::DemandFlags;
use mortar_core::incr::{FileDemands, OptionKeys};
use mortar_core::resolution::SectionKind;
use mortar_core::{FileId, SymKind, SymbolId};
use mortar_diag::{Collector, Policy};
use mortar_elf::SectionIndex;
use mortar_incr::format::kind;
use mortar_incr::update::live::{LiveDelta, live};
use mortar_incr::update::resolve::ResolveDelta;
use mortar_incr::update::rows::FileRow;
use mortar_incr::update::testing::MemoryState;
use mortar_incr::update::view::{ChangedFiles, StateView};
use mortar_merge::phase::ArchRelocs;
use mortar_scan::ScanOptions;
use mortar_testutil::{Scratch, tools_required};
use support::{Source, build_by, store, with_output};

/// The two 64-bit targets' C++ drivers.
const TARGETS: [(&str, &str); 2] = [("aarch64", "g++"), ("x86-64", "x86_64-linux-gnu-g++")];

const MAIN: &str = "extern \"C\" int f();\nint main() { return f(); }\n";
const B: &str = "extern \"C\" { int gv = 5; int gv2 = 6; __thread int tv = 7; \
    int ext(int x) { return x + 1; } }\n";
/// v0: every key v1 defines already defined (so none is new), and the
/// GOT, TLS and call references v1 makes made already (so the edit
/// extracts no archive member: `changed::u5` refuses one).
const A0: &str = "extern \"C\" { extern int gv; extern __thread int tv; int ext(int); \
    int g2(); int f(); }\n\
    inline __attribute__((noinline)) int twice(int x) { return ext(x) * 2; }\n\
    static int thrower(int x) { if (x > 1000) throw x; return x; }\n\
    extern \"C\" int g2() { return 4 + tv; }\n\
    extern \"C\" int f() { int r = 0; try { r = thrower(gv); } catch (int e) { r = e; } \
    return r + twice(1) + g2(); }\n";
/// v1: a GOT reference added (`gv2`, `-fPIC`), the TLS access and the
/// call moved, the bodies changed.
const A1: &str = "extern \"C\" { extern int gv; extern int gv2; extern __thread int tv; \
    int ext(int); int g2(); int f(); }\n\
    inline __attribute__((noinline)) int twice(int x) { return ext(x) * 2 + tv; }\n\
    static int thrower(int x) { if (x > 100) throw x + 1; return x; }\n\
    extern \"C\" int g2() { return gv2 + 4; }\n\
    extern \"C\" int f() { int r = 0; try { r = thrower(gv); } catch (int e) { r = e; } \
    return r + tv + twice(3) + g2(); }\n";
/// v1 with a NEW key (`fresh`) the file defines and calls.
const A1_NEW: &str = "extern \"C\" { extern int gv; extern int gv2; extern __thread int tv; \
    int ext(int); int g2(); int f(); }\n\
    inline __attribute__((noinline)) int twice(int x) { return ext(x) * 2 + tv; }\n\
    static int thrower(int x) { if (x > 100) throw x + 1; return x; }\n\
    extern \"C\" __attribute__((noinline)) int fresh(int x) { return x + gv2; }\n\
    extern \"C\" int g2() { return gv2 + 4; }\n\
    extern \"C\" int f() { int r = 0; try { r = thrower(gv); } catch (int e) { r = e; } \
    return r + tv + twice(3) + g2() + fresh(2); }\n";
/// v0 and v1 with a weak undefined reference (`wk`, defined nowhere).
const A0_WEAK: &str = "extern \"C\" { __attribute__((weak)) int wk(); int ext(int); int g2(); \
    int f(); }\n\
    inline __attribute__((noinline)) int twice(int x) { return x * 2; }\n\
    static int thrower(int x) { if (x > 1000) throw x; return x; }\n\
    extern \"C\" int g2() { return 4; }\n\
    extern \"C\" int f() { int r = 0; try { r = thrower(3); } catch (int e) { r = e; } \
    return r + twice(1) + g2() + (wk ? wk() : 0); }\n";
const A1_WEAK: &str = "extern \"C\" { __attribute__((weak)) int wk(); int ext(int); int g2(); \
    int f(); }\n\
    inline __attribute__((noinline)) int twice(int x) { return x * 3; }\n\
    static int thrower(int x) { if (x > 1000) throw x; return x; }\n\
    extern \"C\" int g2() { return 5; }\n\
    extern \"C\" int f() { int r = 0; try { r = thrower(3); } catch (int e) { r = e; } \
    return r + twice(1) + g2() + (wk ? wk() : 0); }\n";

/// The two configurations: (label, compile flags, `--gc-sections`).
const CONFIGS: [(&str, &[&str], bool); 2] = [
    ("static -g gc", &["-O1", "-fPIC", "-g", "-ffunction-sections", "-fdata-sections"], true),
    ("static", &["-O1", "-fPIC"], false),
];

struct Line {
    scratch: Scratch,
    label: String,
    dir: PathBuf,
    args: Vec<String>,
}

impl Line {
    fn args(&self, out: &str) -> Vec<OsString> {
        let mut a = with_output(&self.args, &self.dir.join(out));
        a.extend(["--incremental=full", "--incremental-state"].iter().map(OsString::from));
        a.push(format!("{out}.mortar-state").into());
        a
    }

    fn link(&self, out: &str) -> MemoryState {
        let inv = mortar::invoke_in("mortar", &self.args(out), &self.dir);
        assert!(
            matches!(inv.outcome, mortar::Outcome::Linked { .. }),
            "{}: {out}: {:?}",
            self.label,
            inv.outcome
        );
        let bytes = std::fs::read(self.dir.join(format!("{out}.mortar-state"))).expect("state");
        mortar_incr::state5::read::decode(&bytes).expect("the state decodes")
    }

    fn front<'s>(&self, store: &'s mortar_input::store::Store) -> Front<'s> {
        let parsed = mortar_cli::parse(&self.args("front"));
        assert!(!parsed.failed(), "{:?}", parsed.diagnostics);
        let diag = Collector::new(Policy::default());
        let pool = mortar_core::par::Pool::try_new(2).expect("a pool");
        let env = Env { cwd: &self.dir, pool, replay: None, slack: true, state5: true };
        let f = front(parsed.plan, env, store, &diag, &mut Timer::new(false));
        assert!(!diag.finish().failed(), "{}", self.label);
        f.expect("the front")
    }
}

fn pool() -> mortar_core::par::Pool {
    mortar_core::par::Pool::try_new(2).expect("a pool")
}

/// A pair built and linked: v0's state, v1's full front, and the update's
/// inputs for `a.cc.o` from their producers.
struct Pair {
    line: Line,
    m0: MemoryState,
}

fn pair(
    target: (&str, &str),
    config: (&str, &[&str], bool),
    a0: &'static str,
    a1: &'static str,
    tag: &str,
) -> Option<Pair> {
    let (arch, cxx) = target;
    let (cfg, flags, gc) = config;
    let label = format!("{arch} {cfg}");
    let s = Scratch::new(&format!("wp255d5-{tag}-{}", label.replace([' ', '/'], "-")));
    let v0 = [
        Source { name: "main.cc", text: MAIN },
        Source { name: "b.cc", text: B },
        Source { name: "a.cc", text: a0 },
    ];
    let Some(b) = build_by(&s, cxx, flags, &v0) else {
        assert!(!tools_required(), "{cxx} is required");
        return None;
    };
    let mut args = b.args;
    if gc {
        args.push("--gc-sections".into());
    }
    let line = Line { scratch: s, label, dir: b.dir, args };
    let m0 = line.link("out");
    let v1 = [
        Source { name: "main.cc", text: MAIN },
        Source { name: "b.cc", text: B },
        Source { name: "a.cc", text: a1 },
    ];
    build_by(&line.scratch, cxx, flags, &v1).expect("v1 builds");
    Some(Pair { line, m0 })
}

/// What the update hands `scan_of`, built by the producers.
struct Inputs {
    changed: ChangedFiles,
    delta: ResolveDelta,
    live: LiveDelta,
    options: OptionKeys,
    a: FileId,
}

fn inputs(p: &Pair, f1: &Front<'_>, m1: &MemoryState, view: &StateView<'_>) -> Inputs {
    let label = &p.line.label;
    let a = f1
        .inputs
        .files
        .iter()
        .find(|(_, x)| x.path.ends_with(b"a.cc.o"))
        .map(|(id, _)| id)
        .expect("a.cc.o");
    let (changed, delta) = mortar::update::changed::u5(view, &[a], &[], f1, m1, &mut |_| {})
        .unwrap_or_else(|e| panic!("{label}: U4-U5: {}", e.report()))
        .expect("a.cc.o changed");
    let options = mortar_resolve::tables::option_keys(&f1.plan, &f1.inputs.link.directives);
    let live = live(view, &changed, &delta, &options)
        .unwrap_or_else(|e| panic!("{label}: live: {}", e.report()));
    Inputs { changed, delta, live, options, a }
}

/// A demand's key by identity: a global by name, a local by its index.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum K {
    Global(Vec<u8>),
    Local(u32),
}

fn front_set(f: &Front<'_>, d: &FileDemands) -> BTreeSet<(K, DemandFlags)> {
    d.demands
        .iter()
        .map(|s| {
            let k = match s.key.kind() {
                SymKind::Global(id) => K::Global(f.inputs.symbols[id].name.to_vec()),
                SymKind::Local(e) => K::Local(e.index.0),
            };
            (k, s.flags)
        })
        .collect()
}

fn update_set(view: &StateView<'_>, d: &FileDemands, file: FileId) -> BTreeSet<(K, DemandFlags)> {
    let symbols = view.count(kind::SYMBOLS);
    d.demands
        .iter()
        .map(|s| {
            let k = match s.key.kind() {
                SymKind::Global(id) => {
                    assert!(u64::from(id.0) < symbols, "a demand on a new key {id:?}");
                    let r = view.symbol(id).expect("SYMBOLS");
                    K::Global(view.string(r.name).expect("name"))
                }
                SymKind::Local(e) => {
                    assert_eq!(e.file, file, "a local of another file");
                    K::Local(e.index.0)
                }
            };
            (k, s.flags)
        })
        .collect()
}

/// What a test checks: the full front, the state, `a.cc.o`'s id there,
/// `scan_of_why`'s result and `defined_facts`, packed.
type Check<'c> = dyn Fn(
        &Front<'_>,
        &StateView<'_>,
        FileId,
        Result<FileDemands, Fail>,
        Vec<(SymbolId, Result<u32, Fail>)>,
    ) + 'c;

/// `scan_of_why` and `defined_facts` on `a.cc.o` of the pair, with the
/// options as given (`edit` may change them).
fn run(p: &Pair, edit: &dyn Fn(&mut OptionKeys, &mut ScanOptions<'_>, &mut bool), check: &Check) {
    let label = &p.line.label;
    let m1 = p.line.link("out1");
    let st = store();
    let f1 = p.line.front(&st);
    let view = p.m0.view();
    let mut x = inputs(p, &f1, &m1, &view);
    let cf = x.changed.files[0].clone();
    let fresh = store();
    let mut cache = FrontCache::new(&fresh, pool(), &p.line.dir);
    let stored = view.file(cf.file).expect("FILES");
    let row = FileRow { base: cf.row.base, len: cf.row.len, hash: cf.row.hash, ..stored };
    let ids = Ids::Update { first_section: cf.first_section, row };
    let ff = front_of(&mut cache, &view, cf.file, ids)
        .unwrap_or_else(|e| panic!("{label}: front_of: {e:?}"));
    let facts = changed_facts(ff, &view, &f1.plan, &x.delta.keepers)
        .unwrap_or_else(|e| panic!("{label}: changed_facts: {e:?}"));
    let null =
        SectionFact { kind: SectionKind::Null, kept: false, out: None, reloc: SectionIndex(0) };
    let by_index = |i: SectionIndex| facts.get(i.0 as usize).copied().unwrap_or(null);
    let split = split_of(ff, &view, &by_index, &ArchRelocs(f1.arch.get()), &f1.pool)
        .unwrap_or_else(|e| panic!("{label}: split_of: {e:?}"));
    let mut opts = ScanOptions {
        output: output_kind(f1.plan.kind),
        dynamic: false,
        sections: None,
        proof: f1.arch.proof(),
    };
    let mut stored = false;
    edit(&mut x.options, &mut opts, &mut stored);
    let facts_in = if stored { None } else { Some(facts.as_slice()) };
    let got = scan_of_why(
        f1.arch.get(),
        opts,
        ff,
        facts_in,
        &split,
        &view,
        &x.changed,
        &x.options,
        &x.delta,
        &x.live,
    );
    let defined =
        defined_facts(opts, ff, facts_in, &split, &view, &x.changed, &x.options, &x.delta, &x.live)
            .map(|v| {
                v.into_iter().map(|(id, r)| (id, r.map(mortar_arch::SymFacts::pack))).collect()
            })
            .unwrap_or_default();
    check(&f1, &view, x.a, got, defined);
}

/// The comparison of an edited file: demands and defined keys' facts.
#[test]
fn scan_of_an_edited_file_equals_the_full_link() {
    let mut ran = 0;
    for target in TARGETS {
        for config in CONFIGS {
            let Some(p) = pair(target, config, A0, A1, "eq") else { continue };
            ran += 1;
            let label = p.line.label.clone();
            run(&p, &|_, _, _| {}, &|f1, view, a, got, defined| {
                let got = got.unwrap_or_else(|e| panic!("{label}: scan_of: {}", e.1));
                let want = f1
                    .incr5
                    .as_ref()
                    .and_then(|i| i.demands.iter().find(|d| d.file == a))
                    .map_or_else(BTreeSet::new, |d| front_set(f1, d));
                let have = update_set(view, &got, got.file);
                assert_eq!(have, want, "{label}: a.cc.o's demands");
                let flags = have.iter().fold(DemandFlags::empty(), |acc, d| acc | d.1);
                assert!(!have.is_empty(), "{label}: some demand");
                assert!(flags.contains(DemandFlags::GOT), "{label}: a GOT demand: {have:?}");
                let all = &f1.incr5.as_ref().expect("incr5").facts;
                let mut n = 0;
                for (id, packed) in &defined {
                    let packed =
                        packed.as_ref().unwrap_or_else(|e| panic!("{label}: key {id:?}: {}", e.1));
                    let r = view.symbol(*id).expect("SYMBOLS");
                    let name = view.string(r.name).expect("name");
                    let fid = (0..f1.inputs.symbols.len())
                        .map(SymbolId)
                        .find(|&k| f1.inputs.symbols[k].name == name.as_slice())
                        .expect("the key in the full link");
                    assert_eq!(
                        *packed,
                        all[fid.0 as usize],
                        "{label}: facts of {}",
                        String::from_utf8_lossy(&name)
                    );
                    n += 1;
                }
                assert!(n >= 3, "{label}: f, g2, twice defined: {n}");
                eprintln!("{label}: {} demands equal, {n} defined keys' facts equal", have.len());
            });
        }
    }
    assert!(ran == 4 || !tools_required(), "{ran} of 4 pairs built");
}

fn word(r: &Result<FileDemands, Fail>) -> String {
    match r {
        Ok(_) => "ok".into(),
        Err(e) => {
            assert_eq!(e.0.report().split(' ').next(), Some("update-not-implemented"), "{}", e.1);
            e.1.split(':').next().unwrap_or_default().to_owned()
        }
    }
}

fn fallback(
    tag: &str,
    a0: &'static str,
    a1: &'static str,
    edit: &dyn Fn(&mut OptionKeys, &mut ScanOptions<'_>, &mut bool),
    want: &str,
) {
    let mut ran = 0;
    for target in TARGETS {
        let Some(p) = pair(target, CONFIGS[1], a0, a1, tag) else { continue };
        ran += 1;
        let label = p.line.label.clone();
        run(&p, edit, &|_, _, _, got, _| {
            assert_eq!(word(&got), want, "{label}: {got:?}");
        });
    }
    assert!(ran == 2 || !tools_required(), "{ran} of 2 pairs built");
}

#[test]
fn a_wrap_link_falls_back_wrap() {
    fallback("wrap", A0, A1, &|o, _, _| o.wrap.push(b"ext".to_vec().into_boxed_slice()), "wrap");
}

#[test]
fn a_stored_file_falls_back_stored_reloc() {
    fallback("stored", A0, A1, &|_, _, s| *s = true, "stored-reloc");
}

#[test]
fn a_new_key_falls_back_facts() {
    fallback("new", A0, A1_NEW, &|_, _, _| {}, "facts");
}

#[test]
fn an_undefined_key_falls_back_facts() {
    fallback("undef", A0_WEAK, A1_WEAK, &|_, _, _| {}, "facts");
}

#[test]
fn a_dynamic_output_with_an_undefined_key_falls_back_weak_dynamic() {
    fallback(
        "dyn",
        A0_WEAK,
        A1_WEAK,
        &|_, o, _| o.output = mortar_arch::OutputKind::Exec,
        "weak-dynamic",
    );
}

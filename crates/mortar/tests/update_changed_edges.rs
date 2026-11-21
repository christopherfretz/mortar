//! WP-267 P-1b: U4's `INDEX` (M-4) and `EDGES` (M-5) rows of a changed
//! file without front 1 (`mortar::update::changed_edges`).
//!
//! - `changed_edges_are_the_full_links_rows`: an edited C++ object (a
//!   COMDAT group of two members: the group ring; a `__start_`/`__stop_`
//!   section; merge strings; a dynamic initializer in `.init_array`; an
//!   `SHF_LINK_ORDER` section; `.eh_frame` with an LSDA) given to
//!   P-1's `changed_files_of` over the state of the OLD inputs, U-3's
//!   `resolve`, `facts_after_resolve`, then `changed_edges_of`: its
//!   `index` and, under `--gc-sections`, its `edges` equal the rows the
//!   FULL link of the NEW inputs stores for that file (sections by index
//!   in the file, keys by name; an oracle that is not front 1's rows).
//!   Without `--gc-sections` `edges` is empty. Each column (INDEX unit,
//!   kind, target; EDGES kind, source, target, offset) mutated alone is
//!   caught.
//! - `changed_edges_refuse_by_name`: a `RESOLVED` row naming a section of
//!   the file in which no entry defines the key is `update-not-implemented
//!   (live)`, `def-entry`.
//! - `changed_edges_equal_the_fronts`: the real path under
//!   `MORTAR_UPDATE_FINDINGS`, with and without `--gc-sections`: each
//!   update's `P-1b:` line says `equal` (front 1's rows).
//!
//! On AArch64 and x86-64.
#![allow(clippy::too_many_lines, reason = "one fixture, then its checks")]

mod support;
#[path = "support/update.rs"]
#[allow(dead_code, reason = "a shared helper file; this test uses part of it")]
mod update;

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use mortar::update::changed_edges::changed_edges_of;
use mortar::update::changed_rows::{changed_files_of, facts_after_resolve};
use mortar::update::file_front::FrontCache;
use mortar_core::incr::{IncrReason, Unimplemented};
use mortar_core::{FileId, SymbolId};
use mortar_incr::format::{def_kind, index_kind, kind};
use mortar_incr::update::resolve::{Resolution, ResolveDelta};
use mortar_incr::update::rows::{EdgeRow, edge_kind};
use mortar_incr::update::view::{ChangedFiles, StateView};
use mortar_testutil::run_with;
use support::with_output;
use update::{Line, lines, state, whole};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

/// What every version shares: liveness's inputs of each kind. `p1b_f` is
/// live through the dynamic initializer (`.init_array`, a root by rule).
const P1B_HEAD: &str = "__asm__(\".section .text.p1b_g,\\\"axG\\\",%progbits,p1b_g,comdat\\n\"\n\
    \"\\t.globl p1b_g\\np1b_g:\\n\\tret\\n\"\n\
    \"\\t.section .rodata.p1b_g,\\\"aG\\\",%progbits,p1b_g,comdat\\n\\t.byte 2\\n\"\n\
    \"\\t.section .text.p1b_h,\\\"ax\\\",%progbits\\n\\t.globl p1b_h\\np1b_h:\\n\\tret\\n\"\n\
    \"\\t.section .p1b_lo,\\\"ao\\\",%progbits,.text.p1b_h\\n\\t.byte 1\\n\\t.text\\n\");\n\
    extern \"C\" void p1b_g(void);\n\
    extern \"C\" void p1b_h(void);\n\
    extern \"C\" { int p1b_data = 4; }\n\
    extern \"C\" const int __start_p1b_set[], __stop_p1b_set[];\n\
    __attribute__((section(\"p1b_set\"), used)) static const int p1b_set_a = 1;\n\
    inline __attribute__((noinline)) int p1b_inl(int x) \
    { static int calls; return x * 5 + p1b_data + ++calls; }\n\
    static const char *p1b_str(int x) { return x ? \"p1b-one\" : \"p1b-two\"; }\n\
    __attribute__((noinline)) int p1b_thrower(int x) { if (x > 1000) throw x; return x; }\n\
    static int p1b_try(int x) { try { return p1b_thrower(x); } catch (int e) { return e; } }\n";

/// The object before the edit.
fn p1b_v0() -> String {
    format!(
        "{P1B_HEAD}extern \"C\" int p1b_f(int x) {{ if (x > 1000) throw x; p1b_g(); p1b_h(); x += p1b_try(x); \
         return p1b_inl(x) + int(__stop_p1b_set - __start_p1b_set) + p1b_str(x)[0]; }}\n\
         static int p1b_init = p1b_f(1);\n\
         extern \"C\" int p1b_get(void) {{ return p1b_init; }}\n"
    )
}

/// The edit: `p1b_f` grown, a second inline with a static local (a new
/// group), a second `p1b_set` entry, a new string, an added section.
fn p1b_v1() -> String {
    format!(
        "{P1B_HEAD}__attribute__((section(\"p1b_set\"), used)) static const int p1b_set_b = 2;\n\
         inline __attribute__((noinline)) int p1b_inl2(int x) \
         {{ static int seen; return x * 3 - ++seen; }}\n\
         extern \"C\" __attribute__((section(\".text.p1b_added\"))) int p1b_added(int x) \
         {{ return x + 9; }}\n\
         extern \"C\" int p1b_f(int x) {{ if (x > 1000) throw x; p1b_g(); p1b_h(); x += p1b_try(x); int s = 0; \
         for (int i = 0; i < x; i++) s += i * x; \
         return s + p1b_inl(x) + p1b_inl2(x) + p1b_added(x) \
         + int(__stop_p1b_set - __start_p1b_set) + p1b_str(x)[0] + (x > 7 ? \"p1b-three\"[1] : 0); }}\n\
         static int p1b_init = p1b_f(1);\n\
         extern \"C\" int p1b_get(void) {{ return p1b_init; }}\n"
    )
}

/// One column of the rows changed (a mutation the check must catch).
type Mutation<'a> = dyn Fn(&mut ChangedFiles) + 'a;

#[allow(clippy::needless_pass_by_value, reason = "a `map_err` argument")]
fn report(e: IncrReason) -> String {
    e.report()
}

/// The file of `view` whose path ends in `name` (not a member).
fn file_named(view: &StateView<'_>, name: &str) -> FileId {
    let files = u32::try_from(view.count(kind::FILES)).expect("FILES");
    (0..files)
        .map(FileId)
        .find(|&id| {
            let r = view.file(id).expect("FILES");
            r.member == 0 && view.string(r.path).expect("path").ends_with(name.as_bytes())
        })
        .unwrap_or_else(|| panic!("no file {name}"))
}

/// The plan of `args` (the update's line).
fn plan_of(line: &Line, args: &[String]) -> mortar_core::plan::Plan {
    let parsed = mortar_cli::parse(&with_output(args, &line.dir.join("plan")));
    assert!(!parsed.failed(), "{:?}", parsed.diagnostics);
    parsed.plan
}

/// A target, normalized: a key by (name, version), a section by index in
/// the file (`Other` past it: never expected), else the value itself.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum T {
    Key(Vec<u8>, Vec<u8>),
    Section(u32),
    Other(u32),
    Value(u32),
}

/// The rows of one file in a form both id spaces share, as SETS: the
/// state holds them sorted and deduplicated (`tables.rs`' delta writer,
/// the capture), `ChangedFile::index` keeps a pair once per record (front
/// 1's and P-1b's alike).
#[derive(Debug, Default)]
struct Norm {
    index: Vec<(u32, u8, T)>,
    edges: Vec<(u8, u32, T, u32)>,
}

/// `cf`'s file (update ids: sections `first_section + i`, keys positions
/// in `keys`) normalized.
fn mine_norm(cf: &ChangedFiles) -> Norm {
    let c = &cf.files[0];
    let first = c.first_section.0;
    let count = u32::try_from(c.sections.len()).expect("sections");
    let section = |s: u32| match s.checked_sub(first).filter(|k| *k < count) {
        Some(k) => T::Section(k),
        None => T::Other(s),
    };
    let key = |k: u32| {
        c.keys.get(k as usize).map_or(T::Other(k), |n| T::Key(n.name.clone(), n.version.clone()))
    };
    let mut n = Norm::default();
    for r in &c.index {
        let t = match r.kind {
            index_kind::GLOBAL | index_kind::SLOT => key(r.target),
            _ => section(r.target),
        };
        let T::Section(u) = section(r.unit) else {
            panic!("an INDEX unit {} not the file's", r.unit)
        };
        n.index.push((u, r.kind, t));
    }
    for e in &c.edges {
        n.edges.push(edge_norm(e, &section, &key));
    }
    n.index.sort();
    n.index.dedup();
    n.edges.sort();
    n.edges.dedup();
    n
}

fn edge_norm(e: &EdgeRow, section: &dyn Fn(u32) -> T, key: &dyn Fn(u32) -> T) -> (u8, u32, T, u32) {
    let t = match edge_kind::target(e.kind) {
        Some(edge_kind::Target::Key) => key(e.target),
        Some(edge_kind::Target::Section) => section(e.target),
        _ => T::Value(e.target),
    };
    let s = match section(e.source) {
        T::Section(k) => k,
        _ => u32::MAX,
    };
    (e.kind, s, t, e.offset)
}

/// What `s1` (the full link of the new inputs) stores for `id`,
/// normalized.
fn full_norm(s1: &StateView<'_>, id: FileId) -> Norm {
    let e = report;
    let fi = s1.file_index(id).map_err(e).expect("FILE_INDEX");
    let first = fi.ranges[mortar_incr::update::rows::FileTable::Sections as usize].first;
    let count = u32::try_from(s1.sections_of(id).map_err(e).expect("SECTIONS").len()).expect("n");
    let section = |s: u32| match s.checked_sub(first).filter(|k| *k < count) {
        Some(k) => T::Section(k),
        None => T::Other(s),
    };
    let key = |k: u32| {
        let sym = s1.symbol(SymbolId(k)).map_err(e).expect("SYMBOLS");
        let s = |x| s1.string(x).map_err(e).expect("a string");
        T::Key(s(sym.name), s(sym.version))
    };
    let mut n = Norm::default();
    for r in &s1.index_of(id).map_err(e).expect("INDEX") {
        let t = match r.kind {
            index_kind::GLOBAL | index_kind::SLOT => key(r.target),
            _ => section(r.target),
        };
        let T::Section(u) = section(r.unit) else {
            panic!("an INDEX unit {} not the file's", r.unit)
        };
        n.index.push((u, r.kind, t));
    }
    for r in &s1.edges_of(id).map_err(e).expect("EDGES") {
        n.edges.push(edge_norm(r, &section, &key));
    }
    n.index.sort();
    n.index.dedup();
    n.edges.sort();
    n.edges.dedup();
    n
}

/// The differences of `mine` against `full`, each named (empty: equal).
fn differ(mine: &Norm, full: &Norm) -> Vec<String> {
    let mut bad = Vec::new();
    if mine.index != full.index {
        let a: Vec<_> = mine.index.iter().filter(|r| !full.index.contains(r)).collect();
        let b: Vec<_> = full.index.iter().filter(|r| !mine.index.contains(r)).collect();
        bad.push(format!(
            "INDEX ({} rows mine, {} the full link's): only mine {a:?}, only the full link's {b:?}",
            mine.index.len(),
            full.index.len()
        ));
    }
    if mine.edges != full.edges {
        let a: Vec<_> = mine.edges.iter().filter(|r| !full.edges.contains(r)).collect();
        let b: Vec<_> = full.edges.iter().filter(|r| !mine.edges.contains(r)).collect();
        bad.push(format!(
            "EDGES ({} rows mine, {} the full link's): only mine {a:?}, only the full link's {b:?}",
            mine.edges.len(),
            full.edges.len()
        ));
    }
    bad
}

/// P-1's front and post, U-3's election, over `s0` for the changed `id`:
/// (the changed files, the delta, the plan).
struct Run {
    cf: ChangedFiles,
    delta: ResolveDelta,
}

fn run_update(
    line: &Line,
    args: &[String],
    s0: &StateView<'_>,
    id: FileId,
    store: &mortar_input::store::Store,
    run: impl FnOnce(&mut Run, &FrontCache<'_>, &mortar_core::plan::Plan, &mortar::front::Front<'_>),
) {
    let mut cache = FrontCache::new(store, update::pool(), &line.dir);
    let plan = plan_of(line, args);
    let mut cf = changed_files_of(s0, &mut cache, &[id], &[], &plan, &line.dir)
        .unwrap_or_else(|e| panic!("{}: changed_files_of: {}", line.label, e.report()));
    let options = mortar_resolve::tables::option_keys(&plan, &[]);
    let ex = mortar_incr::update::extract::extract(s0, &cf, &options).expect("extract");
    let delta = match mortar_incr::update::resolve::resolve(s0, &cf, &ex, &options)
        .unwrap_or_else(|e| panic!("{}: resolve: {}", line.label, e.report()))
    {
        Resolution::Delta(d) => d,
        other @ Resolution::DrawIn(_) => panic!("{}: resolve: {other:?}", line.label),
    };
    let f = whole(line, args, store);
    facts_after_resolve(s0, &cache, &mut cf, &plan, f.arch.get(), &delta.keepers)
        .unwrap_or_else(|e| panic!("{}: facts_after_resolve: {}", line.label, e.report()));
    let mut r = Run { cf, delta: *delta };
    run(&mut r, &cache, &plan, &f);
}

// spec: WP-267 §1 rows 7f, 7g (incr.md IN-70 U4, M-4, M-5): the changed
// file's INDEX and EDGES rows from its bytes alone are the full link's.
#[test]
fn changed_edges_are_the_full_links_rows() {
    let mut ran = 0;
    for line in lines(&[("p1b", "c++", &p1b_v0())]) {
        for gc in [true, false] {
            line.compile("p1b", "c++", &p1b_v0());
            let mut args = line.base.clone();
            args.push(line.dir.join("p1b.o").to_string_lossy().into_owned());
            if gc {
                args.push("--gc-sections".into());
            }
            let label = format!("{} gc {gc}", line.label);
            let s0 = state(&line, &args, "out0");
            line.compile("p1b", "c++", &p1b_v1());
            let s1 = state(&line, &args, "out1");
            let id = file_named(&s0, "p1b.o");
            let store = support::store();
            run_update(&line, &args, &s0, id, &store, |r, cache, plan, f| {
                changed_edges_of(&s0, cache, &mut r.cf, &r.delta, plan, f.arch.get(), &f.pool)
                    .unwrap_or_else(|(what, e)| panic!("{label}: {what}: {}", e.report()));
                let mine = mine_norm(&r.cf);
                let full = full_norm(&s1, id);
                let bad = differ(&mine, &full);
                eprintln!(
                    "{label}: INDEX {} rows, EDGES {} rows, {} differ",
                    mine.index.len(),
                    mine.edges.len(),
                    bad.len()
                );
                assert!(bad.is_empty(), "{label}: {}", bad.join("\n"));
                assert!(!mine.index.is_empty(), "{label}: no INDEX row: the check cannot bite");
                let c = &r.cf.files[0];
                if !gc {
                    assert!(c.edges.is_empty(), "{label}: EDGES without --gc-sections");
                    return;
                }
                // The fixture bites: the kinds it is made for are there.
                let kinds: Vec<u8> = c.edges.iter().map(|e| e.kind).collect();
                let missing: Vec<&str> = [
                    (edge_kind::RELOC, "RELOC"),
                    (edge_kind::START_STOP, "START_STOP"),
                    (edge_kind::LOCAL, "LOCAL"),
                    (edge_kind::FDE_SECTION, "FDE_SECTION"),
                    (edge_kind::FDE_KEY, "FDE_KEY"),
                    (edge_kind::NAMED, "NAMED"),
                    (edge_kind::LINK_ORDER, "LINK_ORDER"),
                    (edge_kind::GROUP, "GROUP"),
                    (edge_kind::ROOT, "ROOT"),
                    (edge_kind::SPLIT, "SPLIT"),
                ]
                .into_iter()
                .filter(|(k, _)| !kinds.contains(k))
                .map(|(_, what)| what)
                .collect();
                assert!(missing.is_empty(), "{label}: no row of {missing:?}: {kinds:?}");
                let ikinds: Vec<u8> = c.index.iter().map(|e| e.kind).collect();
                for (k, what) in [
                    (index_kind::GLOBAL, "GLOBAL"),
                    (index_kind::SECTION, "SECTION"),
                    (index_kind::LINK_ORDER, "LINK_ORDER"),
                ] {
                    assert!(ikinds.contains(&k), "{label}: no INDEX {what} row: {ikinds:?}");
                }
                // Each column mutated alone is caught.
                // A row alone of its (unit, kind): another target is a row
                // the set does not hold (the sets are deduplicated).
                let alone = (0..c.index.len())
                    .find(|&j| {
                        let r = &c.index[j];
                        c.index.iter().filter(|x| (x.unit, x.kind) == (r.unit, r.kind)).count() == 1
                    })
                    .expect("an INDEX row alone of its unit and kind");
                let offset_at = c
                    .edges
                    .iter()
                    .position(|e| {
                        edge_kind::target(e.kind) == Some(edge_kind::Target::Section)
                            || e.kind == edge_kind::RELOC
                    })
                    .expect("an edge");
                let mutations: [(&str, &Mutation<'_>); 7] = [
                    ("INDEX unit", &|m| m.files[0].index[0].unit += 1),
                    ("INDEX kind", &|m| m.files[0].index[0].kind ^= 1),
                    ("INDEX target", &|m| m.files[0].index[alone].target ^= 1),
                    ("EDGES kind", &|m| m.files[0].edges[0].kind ^= 1),
                    ("EDGES source", &|m| m.files[0].edges[0].source += 1),
                    ("EDGES target", &|m| m.files[0].edges[0].target ^= 1),
                    ("EDGES offset", &|m| m.files[0].edges[offset_at].offset ^= 1),
                ];
                for (what, mutate) in mutations {
                    let mut bad = r.cf.clone();
                    mutate(&mut bad);
                    assert!(
                        !differ(&mine_norm(&bad), &full).is_empty(),
                        "{label}: the mutation of {what} is not caught"
                    );
                }
            });
        }
        ran += 1;
    }
    if mortar_testutil::tools_required() {
        assert_eq!(ran, 2, "both targets ran");
    }
}

// spec: WP-267 P-1b step 3 (refused by name, `update-not-implemented
// (live)` with the reason): a definition the narrow link cannot find.
#[test]
fn changed_edges_refuse_by_name() {
    let mut ran = 0;
    for line in lines(&[("p1b", "c++", &p1b_v0())]) {
        let mut args = line.base.clone();
        args.push(line.dir.join("p1b.o").to_string_lossy().into_owned());
        args.push("--gc-sections".into());
        let s0 = state(&line, &args, "out0");
        line.compile("p1b", "c++", &p1b_v1());
        let id = file_named(&s0, "p1b.o");
        let store = support::store();
        run_update(&line, &args, &s0, id, &store, |r, cache, plan, f| {
            // `p1b_f`'s RESOLVED row moved to the file's section 0, where
            // no entry defines it.
            let c = &r.cf.files[0];
            let p = c.keys.iter().position(|k| k.name == b"p1b_f").expect("p1b_f");
            let key = r.delta.key_ids[0].1[p];
            let at = r.delta.resolved.binary_search_by_key(&key, |x| x.0).expect("RESOLVED");
            let row = &mut r.delta.resolved[at].1;
            assert_eq!(row.kind, def_kind::SECTION, "{}: p1b_f", line.label);
            row.section = c.first_section.0;
            let e = changed_edges_of(&s0, cache, &mut r.cf, &r.delta, plan, f.arch.get(), &f.pool)
                .expect_err("a refusal");
            assert_eq!(
                e.1,
                IncrReason::UpdateNotImplemented(Some(Unimplemented::Live)),
                "{}: {}",
                line.label,
                e.0
            );
            assert!(e.0.starts_with("def-entry: key p1b_f"), "{}: {}", line.label, e.0);
            assert!(r.cf.files[0].edges.is_empty(), "{}: rows after a refusal", line.label);
        });
        ran += 1;
    }
    if mortar_testutil::tools_required() {
        assert_eq!(ran, 2, "both targets ran");
    }
}

/// One link: (success, stderr). `--incremental=update` takes the update
/// whatever `small-output` says (U1), so no testhook is needed.
fn link_status(line: &Line, args: &[String], extra: &[&str], findings: &Path) -> (bool, String) {
    let mut a = with_output(args, &line.dir.join("u"));
    a.extend(extra.iter().map(OsString::from));
    let mut c = Command::new(MORTAR);
    c.args(&a).current_dir(&line.dir).env_remove("MORTAR_SCHED_SEED");
    c.env_remove("SOURCE_DATE_EPOCH").env("MORTAR_UPDATE_FINDINGS", findings);
    c.env("MORTAR_INCR_SMALL_OUTPUT", "0");
    let o = run_with(&mut c, b"", Duration::from_mins(2));
    (o.success(), o.stderr_text())
}

fn link(line: &Line, args: &[String], extra: &[&str], findings: &Path) {
    let o = link_status(line, args, extra, findings);
    assert!(o.0, "{}: mortar {extra:?}: {}", line.label, o.1);
}

// spec: WP-267 P-1b step 4 (`changed_edges_equal_the_fronts`), §6 R-1: on
// the real path, P-1b's INDEX and EDGES are front 1's.
#[test]
fn changed_edges_equal_the_fronts() {
    let mut ran = 0;
    for line in lines(&[("p1b", "c++", &p1b_v0())]) {
        for gc in [true, false] {
            line.compile("p1b", "c++", &p1b_v0());
            let mut args = line.base.clone();
            args.push(line.dir.join("p1b.o").to_string_lossy().into_owned());
            if gc {
                args.push("--gc-sections".into());
            }
            let label = format!("{} gc {gc}", line.label);
            let findings = line.dir.join("findings.txt");
            link(&line, &args, &["--incremental=full"], &findings);
            for (hop, text) in [(1, p1b_v1()), (2, p1b_v0()), (3, p1b_v1())] {
                line.compile("p1b", "c++", &text);
                std::fs::write(&findings, b"").expect("findings");
                let (ok, err) = link_status(&line, &args, &["--incremental=update"], &findings);
                assert!(ok || err.contains("update-not-implemented ("), "{label}: {err}");
                let text = std::fs::read_to_string(&findings).expect("findings");
                let p1b: Vec<&str> = text
                    .lines()
                    .filter(|l| l.starts_with("P-1b") || l.starts_with("finding: P-1b"))
                    .collect();
                eprintln!("{label}: hop {hop}: {p1b:?}");
                assert_eq!(p1b.len(), 1, "{label}: hop {hop}: one P-1b line: {text}");
                assert!(
                    p1b[0].starts_with("P-1b: equal (1 files, "),
                    "{label}: hop {hop}: {}",
                    p1b[0]
                );
                let gc_word = format!("; gc {})", u8::from(gc));
                assert!(p1b[0].contains(&gc_word), "{label}: hop {hop}: {}", p1b[0]);
                if !ok {
                    link(&line, &args, &["--incremental=full"], &findings);
                }
            }
        }
        ran += 1;
    }
    if mortar_testutil::tools_required() {
        assert_eq!(ran, 2, "both targets ran");
    }
}

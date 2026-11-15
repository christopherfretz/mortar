//! End to end against GNU ld (spec/image.md §5, §6; WP-026 deliverable 3).
//!
//! A C and a C++ program, compiled by GCC with `-g -fno-pie` (C at `-O1`; C++ as two units at
//! `-O0`, sharing template instantiations), are
//! linked statically by GNU ld (`-static -no-pie -Wl,--no-relax
//! -Wl,-Map`). A plan places every input section of the two user objects
//! that has file bytes (code, data, read-only data, init arrays,
//! `.gcc_except_table`, every `.debug_*` that is not a merge section) at
//! the file offset the reference gave it, with every symbol value read
//! from the reference (`tests/support/reflink.rs`). The image fills it at
//! 1 MiB windows and at 4 KiB windows (so input sections straddle windows,
//! I-33), with 1 and 4 workers, and every byte of every placed section is
//! compared with the executable's, except the fields of the records left
//! out: GOT and TLS types (the reference allocates and relaxes those; the
//! arch's own differential test covers them), targets in merge sections
//! (`.rodata.str*`, `.debug_str`: rewritten by the reference) and
//! `STT_GNU_IFUNC` targets (IPLT). Not compared: the archive members (libc,
//! libstdc++), `.eh_frame` (synthetic), headers, padding, and every
//! synthetic section. Records against discarded sections (COMDAT copies
//! the reference dropped) are applied as dead targets; their fields are
//! checked against the I-47 table rather than GNU ld (which redirects them
//! to the kept copy: the documented difference in spec/image.md §16).

mod support;

use mortar_arch::OutputKind;
use mortar_core::par::Pool;
use mortar_image::fill::Image;
use mortar_testutil::{run, tool};
use support::reflink::{RefExe, check_tombstones, compare, parse_map, ref_world};
use support::{ARCH, TempDir, fill_mem};

const C_SRC: &str = r#"
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
static int tab[8] = {1, 2, 3, 4, 5, 6, 7, 8};
int *p = &tab[2];
const char *msg = "hello";
static const char *names[] = {"zero", "one", "two", "three"};
struct op { int (*f)(int); const char *name; };
static int twice(int x) { return 2 * x; }
static int inc(int x) { return x + 1; }
const struct op ops[] = {{twice, "twice"}, {inc, "inc"}};
long big[1024];
int f(int x) { return tab[x & 7] + (int)big[x & 1023]; }
static void __attribute__((constructor)) init(void) { big[3] = 42; }
int main(int c, char **v) {
    int s = 0;
    for (unsigned i = 0; i < sizeof ops / sizeof ops[0]; i++) s += ops[i].f(c);
    printf("%s %s %d %zu %d\n", msg, names[c & 3], f(c), strlen(v[0]), s);
    char *q = malloc(16);
    snprintf(q, 16, "%d", *p);
    puts(q);
    free(q);
    return 0;
}
"#;

const CXX_SRC: &str = r#"
#include <cstdio>
#include <map>
#include <memory>
#include <stdexcept>
#include <string>
#include <vector>
struct Base { virtual ~Base() = default; virtual int get() const = 0; };
struct A : Base { int v; explicit A(int x) : v(x) {} int get() const override { return v; } };
struct B : Base { std::string s; explicit B(std::string x) : s(std::move(x)) {} int get() const override { return (int)s.size(); } };
template <class T> T sum(const std::vector<T>& xs) { T s{}; for (auto& x : xs) s += x; return s; }
static int may_throw(int x) { if (x > 3) throw std::runtime_error("too big"); return x; }
thread_local int counter = 0;
int main(int argc, char**) {
    std::vector<std::unique_ptr<Base>> v;
    v.push_back(std::make_unique<A>(argc));
    v.push_back(std::make_unique<B>("hello"));
    std::map<std::string, int> m;
    for (auto& b : v) m[std::to_string(b->get())] = b->get();
    std::vector<int> xs{1, 2, 3, argc};
    int r = sum(xs);
    try { r += may_throw(argc + 3); } catch (const std::exception& e) { std::puts(e.what()); r += 1; }
    counter += r;
    std::printf("%d %zu %d\n", r, m.size(), counter);
    return 0;
}
"#;

/// A second C++ unit instantiating the same templates: the reference keeps
/// one COMDAT copy of each, so this unit's debug and exception records
/// against the dropped copies are tombstoned (I-47).
const CXX_SRC2: &str = r#"
#include <map>
#include <memory>
#include <string>
#include <vector>
template <class T> T sum(const std::vector<T>& xs) { T s{}; for (auto& x : xs) s += x; return s; }
int other(int n) {
    std::vector<int> xs(n, 1);
    std::map<std::string, int> m;
    m[std::to_string(n)] = sum(xs);
    auto p = std::make_unique<int>(n);
    return (int)m.size() + *p;
}
"#;

struct Built {
    exe: RefExe,
    map: String,
    objects: Vec<(String, Vec<u8>)>,
}

fn build(dir: &TempDir, lang: &str) -> Option<Built> {
    // C at -O1; C++ at -O0, so the template instantiations stay out of
    // line in both units (COMDAT groups the reference deduplicates).
    let (cc, srcs, ext, opt): (_, &[&str], _, _) = match lang {
        "c" => (tool("gcc")?, &[C_SRC], "c", "-O1"),
        _ => (tool("g++")?, &[CXX_SRC, CXX_SRC2], "cc", "-O0"),
    };
    let mut objs = Vec::new();
    for (i, src) in srcs.iter().enumerate() {
        let s = dir.path(&format!("prog{i}.{ext}"));
        std::fs::write(&s, src).unwrap();
        let o = dir.path(&format!("prog{i}.o"));
        let out = run(&cc, ["-g".as_ref(), opt.as_ref(), "-fno-pie".as_ref(), "-c".as_ref(), s.as_os_str(), "-o".as_ref(), o.as_os_str()]);
        assert!(out.success(), "{lang} compile: {}", out.stderr_text());
        objs.push(o);
    }
    let exe = dir.path("prog");
    let map = dir.path("prog.map");
    let mut args: Vec<std::ffi::OsString> =
        ["-static", "-no-pie", "-Wl,--no-relax"].iter().map(Into::into).collect();
    args.push(format!("-Wl,-Map={}", map.display()).into());
    args.extend(objs.iter().map(|o| o.clone().into_os_string()));
    args.push("-o".into());
    args.push(exe.clone().into_os_string());
    let out = run(&cc, &args);
    if !out.success() {
        // No static libraries for this language: not a mortar failure.
        assert!(!mortar_testutil::tools_required(), "{lang} static link: {}", out.stderr_text());
        eprintln!("note: {lang} static link failed; skipping: {}", out.stderr_text());
        return None;
    }
    Some(Built {
        exe: RefExe::parse(std::fs::read(&exe).unwrap()),
        map: std::fs::read_to_string(&map).unwrap(),
        objects: objs.iter().map(|o| (o.display().to_string(), std::fs::read(o).unwrap())).collect(),
    })
}

fn check(lang: &str) {
    let dir = TempDir::new(&format!("e2e-{lang}"));
    let Some(b) = build(&dir, lang) else { return };
    let map = parse_map(&b.map);
    let objects: Vec<(&str, &[u8])> = b.objects.iter().map(|(n, o)| (n.as_str(), o.as_slice())).collect();
    for window in [mortar_image::IMAGE_BLOCK, 4096] {
        let w = ref_world(&b.exe, &map, &objects, window);
        assert!(w.disagree.is_empty(), "{lang}: map placement disagrees with the symbol table: {:?}", w.disagree);
        let names: std::collections::BTreeSet<&str> = w.units.iter().map(|u| u.name.as_str()).collect();
        assert!(names.contains(".text") && names.contains(".debug_info"), "{lang}: units {names:?}");
        let records: usize = w.units.iter().map(|u| u.records).sum();
        let img = Image {
            plan: &w.plan,
            source: &w.source,
            arch: &ARCH,
            tables: &w.tables,
            output: OutputKind::Static,
            got_base: None,
            tls: b.exe.tls(),
            chunks: &[],
            build_id: None,
        };
        for threads in [1, 4] {
            let pool = Pool::new(threads);
            let ours = match fill_mem(&img, &pool) {
                Ok(o) => o,
                Err(f) => panic!("{lang}: fill failed: {:?}", f.diagnostics),
            };
            let (compared, diffs) = compare(&w, &ours, &b.exe.bytes);
            let (tombs, bad) = check_tombstones(&w, &ours);
            assert!(bad.is_empty(), "{lang}: tombstones not I-47's: {bad:?}");
            assert!(diffs.is_empty(), "{lang} window {window} threads {threads}: {} differences, first {:?}", diffs.len(), &diffs[..diffs.len().min(8)]);
            if threads == 1 && window == mortar_image::IMAGE_BLOCK {
                eprintln!(
                    "e2e {lang}: {} units, {compared} bytes compared, {records} records applied ({} to dead targets, {tombs} tombstones checked), left out {:?}",
                    w.units.len(),
                    w.dead,
                    w.left
                );
            }
            assert!(compared > 1000 && records > 50, "{lang}: too little compared ({compared} bytes, {records} records)");
            assert!(lang == "c" || w.dead > 0, "{lang}: no record against a discarded COMDAT copy");
        }
    }
}

#[test]
fn i33_e2e_c_static_g_matches_gnu_ld() {
    check("c");
}

#[test]
fn i33_e2e_cxx_static_g_matches_gnu_ld() {
    check("c++");
}

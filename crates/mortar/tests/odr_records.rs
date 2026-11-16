//! The record checks end to end (`spec/odr.md` §7-§9, WP-073), on
//! containers the canonical writer builds (`mortar_meta::write`, the test
//! producer) in objects assembled around them, so that every row is
//! exactly what the test says: the rules that compilers cannot be made to
//! exercise (K13 flags, `PARTIAL`, `USES_INTERNAL`, `isa`, O-11's
//! exception), the determinism of the report (equal keys, O-30's hostile
//! values) and robustness (WP-071's mutations, duplicate and contradictory
//! rows, unknown kinds, shared strings, forged counts, mixed minor
//! versions). The model cases with compilers are `tests/link/odr/`.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::process::Command;
use std::time::Duration;

use mortar_meta::write::{Declaration, Definition, RawBlock, Summary, Type, section};
use mortar_meta::{UnitBuilder, class, flags, fnv1a64, producer};
use mortar_testutil::{Outcome, Scratch, run_with, tool, tools_required};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

struct Cx {
    scratch: Scratch,
    gcc: std::path::PathBuf,
}

/// What a symbol of a test object is.
#[derive(Clone, Copy)]
enum Sym<'a> {
    /// A global function.
    Func(&'a str),
    /// A function in its own COMDAT group of the same name.
    Comdat(&'a str),
    /// An undefined reference.
    Undef(&'a str),
}

impl Cx {
    fn new(label: &str) -> Option<Self> {
        if std::env::consts::ARCH != "aarch64" {
            return None;
        }
        let Some(gcc) = tool("gcc") else {
            assert!(!tools_required(), "gcc is required");
            return None;
        };
        let cx = Self { scratch: Scratch::new(label), gcc };
        cx.object("start.o", &[], None);
        Some(cx)
    }

    /// Assembles `name` with `syms` and, when given, a `.mason.meta`
    /// section of exactly `meta`'s bytes.
    fn object(&self, name: &str, syms: &[Sym<'_>], meta: Option<&[u8]>) {
        let mut s = String::new();
        if name == "start.o" {
            s.push_str(
                "\t.text\n\t.globl _start\n\t.type _start, %function\n_start:\n\tmov x0, #0\n\
                 \tmov x8, #93\n\tsvc #0\n",
            );
        }
        for sym in syms {
            match sym {
                Sym::Func(n) => {
                    let _ = write!(s, "\t.text\n\t.globl {n}\n\t.type {n}, %function\n{n}:\tret\n");
                }
                Sym::Comdat(n) => {
                    let _ = write!(
                        s,
                        "\t.section .text.{n},\"axG\",%progbits,{n},comdat\n\t.weak {n}\n\
                         \t.type {n}, %function\n{n}:\tret\n"
                    );
                }
                Sym::Undef(n) => {
                    let _ = write!(s, "\t.data\n\t.quad {n}\n");
                }
            }
        }
        if let Some(meta) = meta {
            let bin = format!("{name}.meta");
            std::fs::write(self.scratch.join(&bin), meta).unwrap();
            let _ = write!(
                s,
                "\t.section .mason.meta,\"e\",%progbits\n\t.balign 8\n\t.incbin \"{bin}\"\n"
            );
        }
        s.push_str("\t.section .note.GNU-stack,\"\",%progbits\n");
        let src = self.scratch.write(&format!("{name}.s"), &s);
        let args: Vec<OsString> = vec!["-c".into(), src.into(), "-o".into(), name.into()];
        let mut c = Command::new(&self.gcc);
        c.args(&args).current_dir(self.scratch.path());
        let o = run_with(&mut c, b"", Duration::from_mins(1));
        assert!(o.success(), "gcc {name}: {}", o.stderr_text());
    }

    /// mortar on `inputs` (after `start.o`) with `extra`, to `out`.
    fn link(&self, inputs: &[&str], extra: &[&str], out: &str) -> Outcome {
        let mut args: Vec<OsString> = vec!["start.o".into()];
        args.extend(inputs.iter().map(OsString::from));
        args.extend(extra.iter().map(OsString::from));
        args.extend(["-o".into(), out.into()]);
        let mut c = Command::new(MORTAR);
        c.args(&args).current_dir(self.scratch.path()).env_remove("MORTAR_SCHED_SEED");
        run_with(&mut c, b"", Duration::from_mins(2))
    }

    /// The standard error of a strict link, which must exit 0 or 1 (never
    /// a panic) and say nothing of a panic.
    fn strict(&self, inputs: &[&str], extra: &[&str]) -> (bool, String) {
        let mut args = vec!["--odr=strict", "--odr-stats"];
        args.extend_from_slice(extra);
        let o = self.link(inputs, &args, "out");
        let e = o.stderr_text();
        assert!(!e.contains("panicked"), "{e}");
        assert!(o.success() || e.contains(": error: "), "neither success nor an error: {e}");
        (o.success(), e)
    }
}

/// A unit with the summary rows `summary` (name, value, flags).
fn unit(summary: &[(&str, &str, u32)]) -> UnitBuilder {
    let mut b = UnitBuilder::new(producer::TEST);
    for (n, v, f) in summary {
        b.summary.push(Summary {
            name: n.as_bytes().to_vec(),
            value: v.as_bytes().to_vec(),
            flags: *f,
        });
    }
    b
}

/// The summary rows every C++ test unit carries.
fn cxx(source: &str) -> UnitBuilder {
    unit(&[
        ("lang", "c++", 0),
        ("odr.scheme", "test-1", 0),
        ("producer", "test", 0),
        ("source", source, 0),
        ("std", "c++20", 0),
    ])
}

/// A named kind 2 row.
fn def(name: &str, hash: u8, flags: u16) -> Definition {
    Definition {
        key: fnv1a64(name.as_bytes()),
        hash: [hash; 16],
        name: Some(name.as_bytes().to_vec()),
        class: class::FUNCTION,
        flags,
        module: Vec::new(),
        file: b"h.h".to_vec(),
        line: 3,
    }
}

/// An anchored (or, without the symbol, unanchored) kind 2 row.
fn anchored(name: &str, hash: u8, flags: u16) -> Definition {
    Definition { name: None, ..def(name, hash, flags) }
}

fn ty(name: &str, layout: u64, size: u32, align: u16, flags: u16) -> Type {
    Type {
        layout,
        size,
        align,
        flags,
        file: b"t.h".to_vec(),
        line: 7,
        ..Type::named(name.as_bytes())
    }
}

fn decl(name: &str, ty: u64, type_name: &str, flags: u16) -> Declaration {
    Declaration {
        key: fnv1a64(name.as_bytes()),
        ty,
        size: 0,
        name: Some(name.as_bytes().to_vec()),
        type_name: type_name.as_bytes().to_vec(),
        flags,
    }
}

fn bytes(units: &[UnitBuilder]) -> Vec<u8> {
    let u: Vec<Vec<u8>> = units.iter().map(|b| b.finish().unwrap()).collect();
    section(&u, 0)
}

fn count(e: &str, needle: &str) -> usize {
    e.matches(needle).count()
}

// spec: odr.md O-15 (K13: an explicit specialization and an implicit
// instantiation are a violation whatever the hashes; PARTIAL is a
// warning), O-16 (USES_INTERNAL in two files)
#[test]
fn kind2_flags() {
    let Some(cx) = Cx::new("odr-k2") else { return };
    let mut a = cxx("a.cc");
    a.definitions.push(def("_Z2k1IiEvv", 1, flags::EXPLICIT_SPEC));
    a.definitions.push(def("_Z1pv", 1, flags::PARTIAL | flags::VAGUE));
    a.definitions.push(def("_Z1uv", 1, flags::USES_INTERNAL | flags::VAGUE));
    a.definitions.push(def("_Z1sv", 1, flags::VAGUE));
    let mut b = cxx("b.cc");
    b.definitions.push(def("_Z2k1IiEvv", 1, flags::IMPLICIT_INST | flags::VAGUE));
    b.definitions.push(def("_Z1pv", 2, flags::VAGUE));
    b.definitions.push(def("_Z1uv", 1, flags::USES_INTERNAL | flags::VAGUE));
    b.definitions.push(def("_Z1sv", 1, flags::VAGUE));
    cx.object("a.o", &[], Some(&bytes(&[a])));
    cx.object("b.o", &[], Some(&bytes(&[b])));
    let (ok, e) = cx.strict(&["a.o", "b.o"], &[]);
    assert!(!ok, "{e}");
    assert!(e.contains("error: _Z2k1IiEvv violates the one-definition rule"), "{e}");
    assert!(e.contains("a.o: an explicit specialization; b.o: an implicit instantiation"), "{e}");
    assert!(e.contains("warning: _Z1pv violates the one-definition rule"), "{e}");
    assert!(e.contains("a record is PARTIAL"), "{e}");
    assert!(
        e.contains(
            "warning: _Z1uv refers to an entity with internal linkage, which differs in each"
        ),
        "{e}"
    );
    assert!(!e.contains("_Z1sv"), "{e}");
    // §9.1: the units' source and producer, first among the notes.
    assert!(
        e.contains(">>> a.o: source=a.cc, producer=test; b.o: source=b.cc, producer=test"),
        "{e}"
    );
    assert!(e.contains(">>> defined in a.o:(_Z2k1IiEvv), at h.h:3"), "{e}");
    assert!(e.contains("record findings 3;"), "{e}");
}

// spec: odr.md O-32 (all rows SYSTEM, -std differs: a warning under
// strict), O-11 (different schemes: not compared, counted)
#[test]
fn system_std_and_schemes() {
    let Some(cx) = Cx::new("odr-sys") else { return };
    let mut a = cxx("a.cc");
    a.definitions.push(def("_Z3sysv", 1, flags::SYSTEM | flags::VAGUE));
    a.definitions.push(def("_Z3usrv", 1, flags::VAGUE));
    let mut b = unit(
        &[("lang", "c++"), ("odr.scheme", "test-1"), ("std", "c++17")].map(|(n, v)| (n, v, 0)),
    );
    b.definitions.push(def("_Z3sysv", 2, flags::SYSTEM | flags::VAGUE));
    b.definitions.push(def("_Z3usrv", 1, flags::VAGUE));
    let mut c = unit(
        &[("lang", "c++"), ("odr.scheme", "other-9"), ("std", "c++20")].map(|(n, v)| (n, v, 0)),
    );
    c.definitions.push(def("_Z3usrv", 9, flags::VAGUE));
    cx.object("a.o", &[], Some(&bytes(&[a])));
    cx.object("b.o", &[], Some(&bytes(&[b])));
    cx.object("c.o", &[], Some(&bytes(&[c])));
    let (ok, e) = cx.strict(&["a.o", "b.o", "c.o"], &[]);
    assert!(ok, "{e}");
    assert!(e.contains("warning: _Z3sysv violates the one-definition rule"), "{e}");
    assert!(e.contains("spec/odr.md O-32"), "{e}");
    assert!(e.contains(">>> a.o: std=c++20; b.o: c++17"), "{e}");
    assert!(!e.contains("_Z3usrv"), "{e}");
    assert!(e.contains("schemes 2, pairs not compared 2,"), "{e}");
    assert!(e.contains("system -std differences 1,"), "{e}");
}

// spec: odr.md O-11's strict exception, O-17, O-18 (kind 3: layout within
// a scheme; across schemes size, alignment and shape flags only)
#[test]
fn kind3_within_and_across_schemes() {
    let Some(cx) = Cx::new("odr-k3") else { return };
    let mut a = cxx("a.cc");
    a.types.push(ty("1L", 0x11, 8, 4, 0));
    a.types.push(ty("1X", 0x22, 8, 4, 0));
    a.types.push(ty("1Y", 0x33, 16, 8, flags::TYPE_POLYMORPHIC));
    let mut b = cxx("b.cc");
    b.types.push(ty("1L", 0x12, 8, 4, 0));
    let mut c = unit(&[("lang", "c++"), ("odr.scheme", "other-9")].map(|(n, v)| (n, v, 0)));
    // Another scheme: a different layout hash is never compared, a
    // different size is.
    c.types.push(ty("1X", 0x99, 8, 4, 0));
    c.types.push(ty("1Y", 0x99, 16, 8, 0));
    cx.object("a.o", &[], Some(&bytes(&[a])));
    cx.object("b.o", &[], Some(&bytes(&[b])));
    cx.object("c.o", &[], Some(&bytes(&[c])));
    let (ok, e) = cx.strict(&["a.o", "b.o", "c.o"], &[]);
    assert!(!ok, "{e}");
    assert!(e.contains("error: type 1L violates the one-definition rule"), "{e}");
    assert!(e.contains("the layouts differ (members, offsets, bases or vtable)"), "{e}");
    assert!(!e.contains("type 1X"), "{e}");
    assert!(e.contains("error: type 1Y violates the one-definition rule"), "{e}");
    assert!(e.contains("a.o: a class, polymorphic; c.o: a class"), "{e}");
    assert!(e.contains("compared on size, alignment and kind only"), "{e}");
    assert!(e.contains("type groups compared across schemes 2,"), "{e}");
}

// spec: odr.md O-35, O-33 (kind 5: two definitions never compared; C
// linkage a warning; NOEXCEPT only a warning)
#[test]
fn kind5_rules() {
    let Some(cx) = Cx::new("odr-k5") else { return };
    let mut a = cxx("a.cc");
    a.declarations.push(decl("_Z1dv", 1, "FvvE", flags::DECL_DEFINITION));
    a.declarations.push(decl(
        "cfun",
        1,
        "int (void)",
        flags::DECL_DEFINITION | flags::DECL_EXTERN_C,
    ));
    a.declarations.push(decl("_Z1nv", 1, "FvvE", flags::DECL_DEFINITION | flags::DECL_NOEXCEPT));
    a.declarations.push(decl("_Z1xv", 1, "FvvE", flags::DECL_DEFINITION));
    let mut b = cxx("b.cc");
    b.declarations.push(decl("_Z1dv", 2, "FivE", flags::DECL_DEFINITION));
    b.declarations.push(decl("cfun", 2, "long (void)", flags::DECL_EXTERN_C));
    b.declarations.push(decl("_Z1nv", 1, "FvvE", 0));
    b.declarations.push(decl("_Z1xv", 2, "FivE", 0));
    cx.object("a.o", &[], Some(&bytes(&[a])));
    cx.object("b.o", &[], Some(&bytes(&[b])));
    let (ok, e) = cx.strict(&["a.o", "b.o"], &[]);
    assert!(!ok, "{e}");
    assert!(!e.contains("_Z1dv"), "{e}");
    assert!(e.contains("warning: cfun is declared with a type that does not match"), "{e}");
    assert!(e.contains("warning: _Z1nv is declared"), "{e}");
    assert!(e.contains("differ only in noexcept"), "{e}");
    assert!(e.contains("error: _Z1xv is declared"), "{e}");
    assert!(e.contains("a.o: defined as FvvE; b.o: declared as FivE"), "{e}");
}

// spec: odr.md O-31 (isa across COMDAT copies, the keeper's known), O-25
// (_ZTV copies of units with different abi.rtti)
#[test]
fn isa_and_vtable_rtti() {
    let Some(cx) = Cx::new("odr-isa") else { return };
    let a = unit(&[("lang", "c++"), ("isa", "v8a"), ("abi.rtti", "on")].map(|(n, v)| (n, v, 0)));
    let b =
        unit(&[("lang", "c++"), ("isa", "v8a+sve"), ("abi.rtti", "off")].map(|(n, v)| (n, v, 0)));
    let syms = [Sym::Comdat("_Z1fv"), Sym::Comdat("_ZTV1S")];
    cx.object("a.o", &syms, Some(&bytes(&[a])));
    cx.object("b.o", &syms, Some(&bytes(&[b])));
    let (ok, e) = cx.strict(&["a.o", "b.o"], &[]);
    assert!(ok, "{e}");
    assert!(
        e.contains("warning: the kept copy of _Z1fv was compiled for other target features"),
        "{e}"
    );
    assert!(e.contains(">>> a.o: isa=v8a; b.o: isa=v8a+sve"), "{e}");
    assert!(
        e.contains("warning: the vtable of 1S is emitted by translation units compiled with and"),
        "{e}"
    );
    assert!(e.contains(">>> defined in a.o:(_ZTV1S) (kept)"), "{e}");
}

/// O-30's diagnostics in order, as (first, second) value pairs.
fn abi_lines(e: &str) -> Vec<&str> {
    e.lines().filter(|l| l.contains("translation units compiled with different abi.k")).collect()
}

// spec: odr.md O-30 (per name, one diagnostic per value other than the
// first unit's, naming the first unit of each; linear)
#[test]
fn enforce_three_values() {
    let Some(cx) = Cx::new("odr-o30") else { return };
    let files = [("a.o", "x"), ("b.o", "y"), ("c.o", "z"), ("d.o", "x"), ("e.o", "y")];
    for (f, v) in files {
        cx.object(
            f,
            &[],
            Some(&bytes(&[unit(&[("lang", "c++", 0), ("abi.k", v, flags::ENFORCE)])])),
        );
    }
    let names: Vec<&str> = files.iter().map(|(f, _)| *f).collect();
    let (ok, e) = cx.strict(&names, &[]);
    assert!(ok, "{e}");
    assert_eq!(
        abi_lines(&e),
        [
            "mortar: warning: translation units compiled with different abi.k: x and y",
            "mortar: warning: translation units compiled with different abi.k: x and z",
        ],
        "{e}"
    );
    assert!(e.contains(">>> in a.o\n>>> in b.o\n"), "{e}");
    assert!(e.contains(">>> in a.o\n>>> in c.o\n"), "{e}");
    // The first unit in file order decides the first value, whatever the
    // values sort as.
    let (_, e) = cx.strict(&["c.o", "a.o", "b.o"], &[]);
    assert_eq!(
        abi_lines(&e),
        [
            "mortar: warning: translation units compiled with different abi.k: z and x",
            "mortar: warning: translation units compiled with different abi.k: z and y",
        ],
        "{e}"
    );
}

// spec: odr.md O-30 (a hostile count of distinct values: one diagnostic
// per value, deterministic at any thread count)
#[test]
fn enforce_hostile_values() {
    let Some(cx) = Cx::new("odr-o30h") else { return };
    let units: Vec<UnitBuilder> = (0..400)
        .map(|i| unit(&[("lang", "c++", 0), ("abi.k", &format!("v{i:03}"), flags::ENFORCE)]))
        .collect();
    cx.object("h.o", &[], Some(&bytes(&units)));
    let (ok, one) = cx.strict(&["h.o"], &["--threads=1"]);
    assert!(ok, "{one}");
    assert_eq!(abi_lines(&one).len(), 399, "{one}");
    assert!(one.contains("different abi.k: v000 and v399"), "{one}");
    let (_, many) = cx.strict(&["h.o"], &[]);
    assert_eq!(strip_time(&one), strip_time(&many));
}

/// The report without the stats line's time.
fn strip_time(e: &str) -> String {
    e.lines().map(|l| l.split("; time ").next().unwrap_or(l)).collect::<Vec<_>>().join("\n")
}

// spec: diag.md §6.2, odr.md §9.1 (findings with equal keys print in one
// order, at 1 thread and at the default count, run after run)
#[test]
fn equal_keys_print_in_one_order() {
    let Some(cx) = Cx::new("odr-keys") else { return };
    // x.o's two units each hold row 0 of an unanchored INLINED_ONLY
    // entity; y.o (first in file order) disagrees with both, so both
    // findings have x.o, tag kind 2 and 2^40 + 0: equal keys.
    let only = flags::INLINED_ONLY | flags::VAGUE;
    let mut x0 = cxx("x0.cc");
    x0.definitions.push(anchored("_Z5firstv", 1, only));
    let mut x1 = cxx("x1.cc");
    x1.definitions.push(anchored("_Z6secondv", 1, only));
    let mut y = cxx("y.cc");
    y.definitions.push(anchored("_Z5firstv", 2, only));
    y.definitions.push(anchored("_Z6secondv", 2, only));
    cx.object("x.o", &[], Some(&bytes(&[x0, x1])));
    cx.object("y.o", &[], Some(&bytes(&[y])));
    let (ok, first) = cx.strict(&["y.o", "x.o"], &["--threads=1"]);
    assert!(!ok);
    assert_eq!(count(&first, "violates the one-definition rule"), 2, "{first}");
    let first = strip_time(&first);
    for run in 0..20 {
        let threads: &[&str] = if run % 2 == 0 { &["--threads=1"] } else { &[] };
        let (_, e) = cx.strict(&["y.o", "x.o"], threads);
        assert_eq!(first, strip_time(&e), "run {run}");
    }
}

// spec: odr.md O-41 as WP-073 reads it (strict compares unanchored rows
// that are INLINED_ONLY; an unanchored row without it describes a
// renamed or localized symbol, N12/N13, and is counted, not compared)
#[test]
fn unanchored_rows() {
    let Some(cx) = Cx::new("odr-unanch") else { return };
    let mut a = cxx("a.cc");
    a.definitions.push(anchored("_Z6inlinev", 1, flags::INLINED_ONLY | flags::VAGUE));
    a.definitions.push(anchored("_Z7renamedv", 1, flags::VAGUE));
    a.declarations.push(Declaration { name: None, ..decl("_Z7renamedv", 1, "FvvE", 0) });
    let mut b = cxx("b.cc");
    b.definitions.push(anchored("_Z6inlinev", 2, flags::INLINED_ONLY | flags::VAGUE));
    b.definitions.push(anchored("_Z7renamedv", 2, flags::VAGUE));
    b.declarations.push(Declaration { name: None, ..decl("_Z7renamedv", 2, "FivE", 0) });
    cx.object("a.o", &[], Some(&bytes(&[a])));
    cx.object("b.o", &[], Some(&bytes(&[b])));
    let (ok, e) = cx.strict(&["a.o", "b.o"], &[]);
    assert!(!ok, "{e}");
    let key = format!("<unnamed entity, key {:#018x}>", fnv1a64(b"_Z6inlinev"));
    assert!(e.contains(&format!("error: {key} violates the one-definition rule")), "{e}");
    assert!(!e.contains(&format!("{:#018x}", fnv1a64(b"_Z7renamedv"))), "{e}");
    assert!(e.contains("unanchored rows 4,"), "{e}");
}

// spec: meta.md MT-11, MT-12, MT-14 (duplicate and contradictory rows of
// one unit, a kind no header announces, shared strings), odr.md §3 N4
// (abi.exceptions differs without ENFORCE: silent)
#[test]
fn odd_but_valid_units() {
    let Some(cx) = Cx::new("odr-odd") else { return };
    let mut a = cxx("a.cc");
    // N4: a summary key without ENFORCE that differs is a note, not a check.
    let exceptions =
        |v: &str| Summary { name: b"abi.exceptions".to_vec(), value: v.into(), flags: 0 };
    a.summary.insert(0, exceptions("on"));
    // The same row twice: one row.
    a.definitions.push(def("_Z3dupv", 1, flags::VAGUE));
    a.definitions.push(def("_Z3dupv", 1, flags::VAGUE));
    // Two rows of one entity that contradict each other: set aside.
    a.definitions.push(def("_Z3conv", 1, flags::VAGUE));
    a.definitions.push(def("_Z3conv", 2, flags::VAGUE));
    // A string shared by rows of three kinds (the writer stores it once).
    a.types.push(ty("1S", 1, 4, 4, 0));
    a.declarations.push(decl("_Z3dupv", 1, "1S", 0));
    a.definitions.push(Definition { module: b"1S".to_vec(), ..def("_Z1mv", 1, flags::VAGUE) });
    // Kinds no reader of 1.0 knows: one past 5, and the experimental range.
    a.raw.push(RawBlock { kind: 6, version: 1, row_size: 8, row_count: 2, body: vec![0xab; 16] });
    a.raw.push(RawBlock { kind: 0x7f00, version: 1, row_size: 0, row_count: 0, body: vec![1; 5] });
    let mut b = cxx("b.cc");
    b.summary.insert(0, exceptions("off"));
    b.definitions.push(def("_Z3dupv", 1, flags::VAGUE));
    b.definitions.push(def("_Z3conv", 3, flags::VAGUE));
    b.types.push(ty("1S", 1, 4, 4, 0));
    b.declarations.push(decl("_Z3dupv", 1, "1S", 0));
    b.definitions.push(Definition { module: b"1S".to_vec(), ..def("_Z1mv", 1, flags::VAGUE) });
    cx.object("a.o", &[], Some(&bytes(&[a])));
    cx.object("b.o", &[], Some(&bytes(&[b])));
    let (ok, e) = cx.strict(&["a.o", "b.o"], &[]);
    assert!(ok, "{e}");
    assert!(!e.contains("warning:") && !e.contains("error:"), "{e}");
    assert!(e.contains("rows set aside 2,"), "{e}");
    assert!(e.contains("record findings 0;"), "{e}");
}

/// A unit's header field at `at` (a `u16` or `u32`) set to `v`.
fn patch(bytes: &mut [u8], at: usize, v: &[u8]) {
    bytes[at..at + v.len()].copy_from_slice(v);
}

// spec: meta.md MT-14 (a forged count is malformed, not an allocation),
// MT-35 (a later minor is read), odr.md O-41 (meta findings are errors
// under strict)
#[test]
fn forged_counts_and_minor_versions() {
    let Some(cx) = Cx::new("odr-forged") else { return };
    let mut a = cxx("a.cc");
    a.definitions.push(def("_Z1fv", 1, flags::VAGUE));
    let good = bytes(&[a]);
    // The definitions block follows the header (32) and the summary block
    // (16 + 5 rows of 16): its row_count is at 32 + 96 + 12.
    let mut rows = good.clone();
    assert_eq!(u16::from_le_bytes([rows[128], rows[129]]), 2);
    patch(&mut rows, 140, &u32::MAX.to_le_bytes());
    let mut blocks = good.clone();
    patch(&mut blocks, 16, &u32::MAX.to_le_bytes());
    let mut strings = good.clone();
    patch(&mut strings, 24, &0x7fff_fff0u32.to_le_bytes());
    for (name, bytes) in [("rows.o", &rows), ("blocks.o", &blocks), ("strings.o", &strings)] {
        cx.object(name, &[], Some(bytes));
        let started = std::time::Instant::now();
        let (ok, e) = cx.strict(&[name], &[]);
        assert!(!ok, "{name}: {e}");
        let head = format!("error: {name}:(.mason.meta+0x0): malformed Mason metadata: ");
        assert!(e.contains(&head), "{name}: {e}");
        assert!(started.elapsed() < Duration::from_secs(20), "{name}");
        // Below strict the container is not read: nothing is reported.
        let o = cx.link(&[name], &[], "out-warn");
        assert!(o.success() && o.stderr_text().is_empty(), "{name}: {}", o.stderr_text());
    }
    // A unit of minor 1 is read as 1.0 (MT-35) and compared with 1.0's.
    let mut b = cxx("b.cc");
    b.definitions.push(def("_Z1fv", 2, flags::VAGUE));
    let mut later = bytes(&[b]);
    patch(&mut later, 6, &1u16.to_le_bytes());
    cx.object("a.o", &[], Some(&good));
    cx.object("b.o", &[], Some(&later));
    let (ok, e) = cx.strict(&["a.o", "b.o"], &[]);
    assert!(!ok, "{e}");
    assert!(e.contains("error: _Z1fv violates the one-definition rule"), "{e}");
    assert!(!e.contains("Mason metadata"), "{e}");
}

// spec: odr.md O-42, O-44 (strict reads exactly the containers' bytes;
// the default and off read none)
#[test]
fn container_bytes_read() {
    let Some(cx) = Cx::new("odr-bytes") else { return };
    let mut a = cxx("a.cc");
    a.definitions.push(def("_Z1fv", 1, flags::VAGUE));
    let meta = bytes(&[a]);
    cx.object("a.o", &[], Some(&meta));
    let (_, e) = cx.strict(&["a.o"], &[]);
    assert!(e.contains(&format!("container bytes read {},", meta.len())), "{e}");
    for mode in ["--odr=warn", "--odr=off"] {
        let o = cx.link(&["a.o"], &[mode, "--odr-stats"], "out-m");
        assert!(o.stderr_text().contains("container bytes read 0,"), "{}", o.stderr_text());
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
}

/// WP-071's mutations (`crates/mortar_meta/tests/mutate.rs`) of a valid
/// section.
fn mutate(rng: &mut Rng, bytes: &mut Vec<u8>) {
    for _ in 0..=rng.below(6) {
        if bytes.is_empty() {
            return;
        }
        match rng.below(6) {
            0 => {
                let i = rng.below(bytes.len());
                bytes[i] ^= 1 << rng.below(8);
            }
            1 => {
                let i = rng.below(bytes.len() / 4) * 4;
                let v: u32 = [0, 1, 8, 16, 32, 0xffff_ffff, 0x8000_0000, 7][rng.below(8)];
                if i + 4 <= bytes.len() {
                    bytes[i..i + 4].copy_from_slice(&v.to_le_bytes());
                }
            }
            2 => {
                let at = rng.below(bytes.len() + 1);
                bytes.truncate(at);
            }
            3 => {
                let i = rng.below(bytes.len());
                bytes[i] = rng.next() as u8;
            }
            4 => {
                let at = rng.below(bytes.len() + 1);
                let extra = vec![rng.next() as u8; rng.below(24)];
                bytes.splice(at..at, extra);
            }
            _ => {
                let i = rng.below(bytes.len() / 2) * 2;
                if i + 2 <= bytes.len() {
                    bytes[i..i + 2].copy_from_slice(&(rng.next() as u16).to_le_bytes());
                }
            }
        }
    }
}

/// A unit with rows of every kind on a few shared entities.
fn rich(rng: &mut Rng, source: &str) -> UnitBuilder {
    let mut b = cxx(source);
    b.summary.push(Summary {
        name: b"abi.k".to_vec(),
        value: vec![b'a' + rng.below(3) as u8],
        flags: 1,
    });
    for i in 0..=rng.below(4) {
        let n = format!("_Z1{}v", (b'a' + i as u8) as char);
        let f = [0, flags::PARTIAL, flags::SYSTEM, flags::INLINED_ONLY, flags::USES_INTERNAL]
            [rng.below(5)];
        b.definitions.push(Definition {
            name: (i % 2 == 0).then(|| n.clone().into_bytes()),
            ..def(&n, rng.below(3) as u8, f)
        });
        b.declarations.push(decl(&n, rng.below(2) as u64, "FvvE", rng.below(16) as u16));
    }
    for i in 0..rng.below(3) {
        b.types.push(ty(&format!("1T{i}"), rng.below(2) as u64, 4 << rng.below(2), 4, 0));
    }
    b
}

// spec: meta.md MT-14 "malformed metadata never..." and WP-073 scope 4
// (the record checks never panic on WP-071's mutations: every link ends
// in success or a named error, with the same report at 1 thread and at
// the default count)
#[test]
fn mutated_containers_never_panic_the_link() {
    let Some(cx) = Cx::new("odr-mut") else { return };
    let syms = [Sym::Func("_Z1av"), Sym::Undef("_Z1bv"), Sym::Comdat("_Z1cv")];
    for seed in 1..=300u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        for (f, syms) in [("m1.o", &syms[..]), ("m2.o", &syms[2..])] {
            let n = 1 + rng.below(2);
            let units: Vec<UnitBuilder> =
                (0..n).map(|i| rich(&mut rng, &format!("{f}{i}.cc"))).collect();
            let mut b = bytes(&units);
            if rng.below(4) != 0 {
                mutate(&mut rng, &mut b);
            }
            cx.object(f, syms, Some(&b));
        }
        let (_, one) = cx.strict(&["m1.o", "m2.o"], &["--threads=1"]);
        let (_, many) = cx.strict(&["m1.o", "m2.o"], &[]);
        assert_eq!(strip_time(&one), strip_time(&many), "seed {seed}");
    }
}

/// The legal link of the mutation tests: every record check has rows,
/// and they agree. `a.o` keeps the COMDAT copies; `c.o` is another
/// scheme's unit of the same type.
fn legal_units() -> [UnitBuilder; 3] {
    let mut a = cxx("a.cc");
    let mut b = cxx("b.cc");
    for (u, i) in [(&mut a, 0u8), (&mut b, 1)] {
        let enforce = [("abi.k", "x", flags::ENFORCE), ("abi.rtti", "on", 0), ("isa", "v8a", 0)];
        for (n, v, f) in enforce {
            u.summary.push(Summary { name: n.into(), value: v.into(), flags: f });
        }
        u.definitions.push(anchored("_Z1fv", 1, flags::VAGUE));
        u.definitions.push(def("_Z1hv", 2, flags::VAGUE));
        u.definitions.push(def("_Z1pv", 3, flags::VAGUE | flags::PARTIAL));
        u.definitions.push(def(
            "_Z1gv",
            4,
            flags::VAGUE | if i == 0 { flags::USES_INTERNAL } else { 0 },
        ));
        u.definitions.push(def("_Z2k1IiEvv", 5, flags::VAGUE | flags::EXPLICIT_SPEC));
        u.definitions.push(def("_Z3sysv", 6, flags::VAGUE | flags::SYSTEM));
        u.types.push(Type { hash: [7; 16], ..ty("1S", 0x51, 16, 8, flags::TYPE_POLYMORPHIC) });
        let d = if i == 0 { flags::DECL_DEFINITION } else { 0 };
        u.declarations.push(Declaration { name: None, ..decl("_Z1fv", 1, "FvvE", d) });
        u.declarations.push(decl("cfun", 2, "int (void)", d | flags::DECL_EXTERN_C));
        u.declarations.push(decl("_Z1nv", 3, "FvvE", d | flags::DECL_NOEXCEPT));
    }
    let mut c = unit(&[("lang", "c++", 0), ("odr.scheme", "other-9", 0)]);
    c.types.push(ty("1S", 0x99, 16, 8, flags::TYPE_POLYMORPHIC));
    [a, b, c]
}

/// The files of the mutation tests, `b`'s and `c`'s units mutated by
/// `mutate`, `b.o`'s section bytes by `bytes_of_b`.
fn mutant(
    cx: &Cx,
    mutate: impl Fn(&mut UnitBuilder, &mut UnitBuilder),
    bytes_of_b: impl Fn(&mut Vec<u8>),
) {
    let [a, mut b, mut c] = legal_units();
    mutate(&mut b, &mut c);
    let syms = [Sym::Comdat("_Z1fv"), Sym::Comdat("_ZTV1S")];
    cx.object("a.o", &syms, Some(&bytes(&[a])));
    let mut bb = bytes(&[b.clone(), b]);
    bytes_of_b(&mut bb);
    cx.object("b.o", &syms, Some(&bb));
    cx.object("c.o", &[], Some(&bytes(&[c])));
}

fn set_def(u: &mut UnitBuilder, name: &str, f: impl Fn(&mut Definition)) {
    let key = fnv1a64(name.as_bytes());
    f(u.definitions.iter_mut().find(|d| d.key == key).unwrap());
}

fn set_decl(u: &mut UnitBuilder, name: &str, f: impl Fn(&mut Declaration)) {
    let key = fnv1a64(name.as_bytes());
    f(u.declarations.iter_mut().find(|d| d.key == key).unwrap());
}

fn set_summary(u: &mut UnitBuilder, name: &str, value: &str) {
    u.summary.iter_mut().find(|s| s.name == name.as_bytes()).unwrap().value = value.into();
}

/// The diagnostics' first lines.
fn heads(e: &str) -> Vec<&str> {
    e.lines()
        .filter(|l| l.starts_with("mortar: error: ") || l.starts_with("mortar: warning: "))
        .collect()
}

type Mutation = (&'static str, fn(&mut UnitBuilder, &mut UnitBuilder), &'static [&'static str]);

// spec: odr.md §12.4 (each check has a mutation of a legal link's units
// that it must report; the legal original stays silent: the kill table
// of spec/landed/meta/WP-073.md)
#[test]
#[allow(clippy::too_many_lines)] // the kill table, one row a mutation
fn mutations_are_killed() {
    let Some(cx) = Cx::new("odr-kill") else { return };
    mutant(&cx, |_, _| {}, |_| {});
    let (ok, e) = cx.strict(&["a.o", "b.o", "c.o"], &[]);
    assert!(ok && heads(&e).is_empty(), "the legal link: {e}");
    assert!(e.contains("units 4,") && e.contains("record findings 0;"), "{e}");
    let mutations: &[Mutation] = &[
        (
            "kind 2: one hash byte flipped",
            |b, _| set_def(b, "_Z1fv", |d| d.hash[3] ^= 1),
            &["error: _Z1fv violates the one-definition rule"],
        ),
        (
            "kind 2: two rows' hashes swapped",
            |b, _| {
                set_def(b, "_Z1fv", |d| d.hash = [2; 16]);
                set_def(b, "_Z1hv", |d| d.hash = [1; 16]);
            },
            &["error: _Z1fv violates", "error: _Z1hv violates"],
        ),
        (
            "kind 2: K13 flags",
            |b, _| set_def(b, "_Z2k1IiEvv", |d| d.flags = flags::VAGUE | flags::IMPLICIT_INST),
            &["error: _Z2k1IiEvv violates"],
        ),
        (
            "kind 2: PARTIAL hash",
            |b, _| set_def(b, "_Z1pv", |d| d.hash[0] ^= 1),
            &["warning: _Z1pv violates"],
        ),
        (
            "O-16: USES_INTERNAL",
            |b, _| set_def(b, "_Z1gv", |d| d.flags |= flags::USES_INTERNAL),
            &["warning: _Z1gv refers to an entity with internal linkage"],
        ),
        (
            "O-32: SYSTEM hash, -std",
            |b, _| {
                set_def(b, "_Z3sysv", |d| d.hash[0] ^= 1);
                set_summary(b, "std", "c++17");
            },
            &["warning: _Z3sysv violates"],
        ),
        ("kind 3: layout", |b, _| b.types[0].layout ^= 1, &["error: type 1S violates"]),
        ("kind 3: definition hash", |b, _| b.types[0].hash[0] ^= 1, &["warning: type 1S violates"]),
        ("kind 3 across schemes: size", |_, c| c.types[0].size = 24, &["error: type 1S violates"]),
        ("kind 5: type", |b, _| set_decl(b, "_Z1fv", |d| d.ty = 9), &["error: _Z1fv is declared"]),
        (
            "kind 5: C linkage type",
            |b, _| set_decl(b, "cfun", |d| d.ty = 9),
            &["warning: cfun is declared"],
        ),
        (
            "kind 5: noexcept",
            |b, _| set_decl(b, "_Z1nv", |d| d.flags &= !flags::DECL_NOEXCEPT),
            &["warning: _Z1nv is declared"],
        ),
        (
            "O-30: ENFORCE value",
            |b, _| set_summary(b, "abi.k", "y"),
            &["warning: translation units compiled with different abi.k: x and y"],
        ),
        (
            "O-31: isa",
            |b, _| set_summary(b, "isa", "v8a+sve"),
            &[
                "warning: the kept copy of _Z1fv was compiled",
                "warning: the kept copy of _ZTV1S was compiled",
            ],
        ),
        (
            "O-25: abi.rtti",
            |b, _| set_summary(b, "abi.rtti", "off"),
            &["warning: the vtable of 1S is emitted"],
        ),
        (
            "O-11: scheme renamed (with a hash flipped): not compared",
            |b, _| {
                set_summary(b, "odr.scheme", "test-2");
                set_def(b, "_Z1fv", |d| d.hash[3] ^= 1);
            },
            &[],
        ),
    ];
    for (what, m, expected) in mutations {
        mutant(&cx, m, |_| {});
        let (_, e) = cx.strict(&["a.o", "b.o", "c.o"], &[]);
        let got = heads(&e);
        assert_eq!(got.len(), expected.len(), "{what}: {e}");
        for (g, x) in got.iter().zip(*expected) {
            assert!(g.contains(x), "{what}: {g} is not {x}: {e}");
        }
    }
    // Bytes: the second unit of b.o truncated, the first still read.
    mutant(&cx, |b, _| set_def(b, "_Z1fv", |d| d.hash[3] ^= 1), |bb| bb.truncate(bb.len() - 8));
    let (_, e) = cx.strict(&["a.o", "b.o", "c.o"], &[]);
    let got = heads(&e);
    assert_eq!(got.len(), 2, "truncated: {e}");
    assert!(got[0].contains("error: _Z1fv violates"), "truncated: {e}");
    assert!(got[1].contains("malformed Mason metadata: unit at offset"), "truncated: {e}");
    // Rows unsorted: b.o's first unit's first two kind 2 rows swapped.
    mutant(
        &cx,
        |_, _| {},
        |bb| {
            let at = 32 + 16 + 8 * 16 + 16;
            assert_eq!(u16::from_le_bytes([bb[at - 16], bb[at - 15]]), 2);
            let (x, y) = bb[at..at + 96].split_at_mut(48);
            x.swap_with_slice(y);
        },
    );
    let (_, e) = cx.strict(&["a.o", "b.o", "c.o"], &[]);
    let got = heads(&e);
    assert_eq!(got.len(), 1, "unsorted: {e}");
    assert!(
        got[0].contains("malformed Mason metadata") && e.contains("out of order"),
        "unsorted: {e}"
    );
}

/// `tool` on `b.o` in place, with `args` before the file.
fn edit_b(cx: &Cx, tool_name: &str, args: &[&str]) -> bool {
    let Some(t) = tool(tool_name) else {
        assert!(!tools_required(), "{tool_name} is required");
        return false;
    };
    let mut words: Vec<OsString> = args.iter().map(OsString::from).collect();
    words.push("b.o".into());
    let mut c = Command::new(t);
    c.args(&words).current_dir(cx.scratch.path());
    let o = run_with(&mut c, b"", Duration::from_mins(1));
    assert!(o.success(), "{tool_name} {args:?}: {}", o.stderr_text());
    true
}

// spec: odr.md §12.4, N12 (a symbol renamed by objcopy --redefine-sym; the
// container removed by eu-strip, llvm-strip, llvm-objcopy; the symbols
// removed by strip: no report and no warning, with a hash flipped)
#[test]
fn tools_that_drop_or_rename_are_silent() {
    let Some(cx) = Cx::new("odr-tools") else { return };
    let flip = |b: &mut UnitBuilder, _: &mut UnitBuilder| set_def(b, "_Z1fv", |d| d.hash[3] ^= 1);
    let cases: [(&str, &[&str]); 6] = [
        ("objcopy", &["--redefine-sym", "_Z1fv=_Z8vendoredv"]),
        ("eu-strip", &["--remove-section=.mason.meta"]),
        ("llvm-strip", &[]),
        ("llvm-strip", &["--strip-all"]),
        ("llvm-objcopy", &["--strip-non-alloc"]),
        ("strip", &[]),
    ];
    for (t, args) in cases {
        mutant(&cx, flip, |_| {});
        if !edit_b(&cx, t, args) {
            continue;
        }
        let (ok, e) = cx.strict(&["a.o", "b.o", "c.o"], &[]);
        assert!(ok, "{t} {args:?}: {e}");
        let got = heads(&e);
        assert!(got.iter().all(|g| !g.contains("_Z1fv violates")), "{t} {args:?}: {e}");
        assert!(got.iter().all(|g| !g.contains("Mason metadata")), "{t} {args:?}: {e}");
    }
}

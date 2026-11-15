//! `spec/input.md` §1: `load` and the `Inputs` it builds (`spec/model.md`
//! §2.3, §3.2; `spec/input.md` §6, §10, §13).
mod common;

use std::ffi::OsStr;
use std::path::Path;

use mortar_core::id::{ArchiveId, FileId};
use mortar_core::inputs::{
    EdgeOutcome, FileKind, Inputs, OpenedKind, Origin, ProbeOutcome, SearchRef,
};
use mortar_core::par::Pool;
use mortar_diag::{Collector, Policy, Report};
use mortar_input::{LoadEnv, Store, StorePolicy, load};
use mortar_testutil::Scratch;

const M: &str = "-maarch64linux";
const EXE: &[&str] = &[M, "-dynamic-linker", "/lib/ld-linux-aarch64.so.1"];

fn load_in<S: AsRef<OsStr>>(
    cwd: &Path,
    args: &[S],
    threads: usize,
) -> (Option<Inputs<'static>>, Report) {
    let store: &'static Store = Box::leak(Box::new(Store::new(StorePolicy::Resident)));
    let diag = Collector::new(Policy::default());
    let env = LoadEnv { cwd: cwd.to_path_buf(), pool: Pool::new(threads) };
    let inputs = load(&common::plan(args), &env, store, &diag);
    (inputs, diag.finish())
}

fn ids(report: &Report) -> Vec<&'static str> {
    report.diagnostics().iter().map(mortar_diag::Diagnostic::id).collect()
}

/// The keys file `f` mentions, as text (`name` or `name@version`).
fn keys(inputs: &Inputs<'_>, f: u32) -> Vec<String> {
    inputs
        .sym_map_of(FileId(f))
        .iter()
        .map(|&id| {
            let k = inputs.symbols[id];
            let name = String::from_utf8_lossy(k.name).into_owned();
            match k.version {
                Some(v) => format!("{name}@{}", String::from_utf8_lossy(v)),
                None => name,
            }
        })
        .collect()
}

/// A group whose signature is the section symbol of `.text.foo`.
const GROUP: &str = "__asm__(\".section .text.foo,\\\"axG\\\",%progbits,.text.foo,comdat\\n.byte 0\\n.text\\n\");\n";

fn objects_fixture(s: &Scratch) -> bool {
    if !common::have(&["native"]) {
        return false;
    }
    let a = format!("{GROUP}extern int g(void);\nint f(void){{return g();}}\n");
    common::cc(s.path(), "native", "a.o", &a, &[]);
    common::cc(s.path(), "native", "b1.o", "int g(void){return 1;}\n", &[]);
    common::cc(s.path(), "native", "b2.o", &format!("{GROUP}int f2(void){{return 2;}}\n"), &[]);
    common::ar(s.path(), "libb.a", &["b1.o", "b2.o"], false);
    true
}

#[test]
fn s1_load_builds_inputs() {
    let s = Scratch::new("s1load");
    if !objects_fixture(&s) {
        return;
    }
    let (inputs, report) = load_in(s.path(), &[M, "a.o", "libb.a"], 1);
    assert!(report.diagnostics().is_empty(), "{report:?}");
    let inputs = inputs.expect("loaded");
    inputs.check().unwrap();
    // spec/model.md §2.1: file 0 is the internal file.
    let f0 = &inputs.files[FileId(0)];
    assert_eq!((f0.kind(), f0.origin, f0.bytes), (FileKind::Internal, Origin::Internal, None));
    assert_eq!(inputs.files.len(), 4);
    let a = &inputs.files[FileId(1)];
    assert_eq!(a.path, b"a.o");
    assert_eq!(a.origin, Origin::Input { input: 0, arg: 1, entry: None });
    assert!(a.sections > 0);
    // §11.3 through the tables: the bytes of a file, and a member's.
    assert_eq!(inputs.resident(FileId(1), 0..4), Some(&b"\x7fELF"[..]));
    let mut out = [0u8; 4];
    inputs.copy_to(FileId(3), 0..4, &mut out).unwrap();
    assert_eq!(&out, b"\x7fELF");
    assert!(inputs.resident(FileId(1), 0..u64::MAX).is_none());
    // The archive and its members (§7, §10.3).
    assert_eq!(inputs.archives.len(), 1);
    let ar = &inputs.archives[ArchiveId(0)];
    assert_eq!((ar.path, ar.members.start, ar.members.end), (&b"libb.a"[..], FileId(2), FileId(4)));
    let names: Vec<_> = (2..4).map(|f| inputs.files[FileId(f)].member.unwrap().name).collect();
    assert_eq!(names, [&b"b1.o"[..], b"b2.o"]);
    assert!(inputs.files[FileId(2)].is_lazy());
    // spec/model.md §2.3: one key per name; the reference to g in a.o and
    // the definition in b1.o are one row.
    assert!(keys(&inputs, 1).contains(&"g".to_owned()));
    assert_eq!(keys(&inputs, 2), ["g"]);
    let g_a = inputs.sym_map_of(FileId(1))[keys(&inputs, 1).iter().position(|k| k == "g").unwrap()];
    assert_eq!(inputs.sym_map_of(FileId(2))[0], g_a);
    // §6, resolve.md §6.1: the effective signature of a section-symbol
    // signature is the section's name; both copies are one ComdatId.
    let comdats: Vec<&[u8]> = inputs.comdats.iter().map(|(_, c)| c.name).collect();
    assert_eq!(comdats, [&b".text.foo"[..]]);
}

#[test]
fn s1_load_is_deterministic() {
    let s = Scratch::new("s1det");
    if !objects_fixture(&s) {
        return;
    }
    let args = [M, "libb.a", "a.o", "--whole-archive", "libb.a"];
    let digest = |inputs: &Inputs<'_>| {
        let files: Vec<_> =
            inputs.files.iter().map(|(_, f)| (f.path, f.member.map(|m| m.name), f.syms)).collect();
        let symbols: Vec<_> =
            inputs.symbols.iter().map(|(_, k)| (k.name, k.version, k.hash)).collect();
        let comdats: Vec<_> = inputs.comdats.iter().map(|(_, c)| c.name).collect();
        format!("{files:?}{:?}{symbols:?}{comdats:?}", inputs.sym_map)
    };
    let (first, _) = load_in(s.path(), &args, 1);
    let want = digest(&first.expect("loaded"));
    for threads in [2, 3, 8] {
        let (inputs, _) = load_in(s.path(), &args, threads);
        assert_eq!(digest(&inputs.expect("loaded")), want, "{threads} threads");
    }
}

#[test]
fn s1_load_shared_object_keys() {
    let s = Scratch::new("s1vers");
    if !common::have(&["native"]) {
        return;
    }
    let v = "int foo_old(void){return 1;}\nint foo_new(void){return 2;}\n__asm__(\".symver foo_old,foo@V1\");\n__asm__(\".symver foo_new,foo@@V2\");\n";
    s.write("v.map", "V1 { local: foo_*; };\nV2 { } V1;\n");
    let so = ["-shared", "-fPIC", "-nostdlib"];
    common::cc(
        s.path(),
        "native",
        "libv.so",
        v,
        &[&so[..], &["-Wl,--version-script=v.map", "-Wl,-soname,libv.so"]].concat(),
    );
    let u = "extern int foo(void);\nint u(void){return foo();}\n";
    common::cc(
        s.path(),
        "native",
        "libu.so",
        u,
        &[&so[..], &["-Wl,--no-as-needed", "-L.", "-lv", "-Wl,-soname,libu.so"]].concat(),
    );
    let mut args = EXE.to_vec();
    args.extend(["libv.so", "libu.so"]);
    let (inputs, report) = load_in(s.path(), &args, 1);
    assert!(report.diagnostics().is_empty(), "{report:?}");
    let inputs = inputs.expect("loaded");
    // resolve.md §9.2: the default version is the unversioned key, the
    // hidden one a key of its own; an undefined reference that needs V2
    // is `foo@V2`.
    let mut v = keys(&inputs, 1);
    v.sort();
    assert_eq!(v, ["V1", "V2", "foo", "foo@V1"]);
    assert!(keys(&inputs, 2).contains(&"foo@V2".to_owned()));
    assert_eq!(inputs.files[FileId(1)].sections, 0, "shared objects get no section rows");
    let units: Vec<FileId> = inputs.link.sonames.iter().map(|u| u.file).collect();
    assert_eq!(units, [FileId(1), FileId(2)]);
}

#[test]
fn s1_load_link_inputs() {
    let s = Scratch::new("s1link");
    if !common::have(&["native"]) {
        return;
    }
    let so = ["-shared", "-fPIC", "-nostdlib"];
    common::cc(
        s.path(),
        "native",
        "libh.so",
        "int h(void){return 1;}\n",
        &[&so[..], &["-Wl,-soname,libh.so"]].concat(),
    );
    common::cc(
        s.path(),
        "native",
        "libg.so",
        "extern int h(void);\nint g(void){return h();}\n",
        &[&so[..], &["-Wl,--no-as-needed", "-L.", "-lh", "-Wl,-soname,libg.so"]].concat(),
    );
    std::fs::create_dir(s.join("d")).unwrap();
    std::fs::rename(s.join("libh.so"), s.join("d/libh.so")).unwrap();
    s.write("g.ld", "ENTRY(start)\nINPUT(-lg)\n");
    let mut args = EXE.to_vec();
    args.extend(["-L.", "g.ld", "-rpath-link", "d"]);
    let (inputs, report) = load_in(s.path(), &args, 1);
    assert!(report.diagnostics().is_empty(), "{report:?}");
    let inputs = inputs.expect("loaded");
    let link = &inputs.link;
    // §10.4: the script, the library it names, the closure file.
    let kinds: Vec<OpenedKind> = link.opened.iter().map(|o| o.kind).collect();
    assert_eq!(
        kinds,
        [OpenedKind::Script, OpenedKind::Shared(FileId(1)), OpenedKind::Shared(FileId(2))]
    );
    assert_eq!(link.script_entries.len(), 1);
    assert_eq!(link.script_entries[0].script, 0);
    assert_eq!(inputs.files[FileId(1)].origin, Origin::Input { input: 0, arg: 4, entry: Some(0) });
    assert_eq!(link.directives.len(), 1);
    assert_eq!(link.directives[0].value, b"start");
    // §8.6: the closure edge and the file it found.
    assert_eq!(link.closure.len(), 1);
    assert_eq!(link.closure[0].needing, FileId(1));
    assert_eq!(link.closure[0].outcome, EdgeOutcome::Satisfied(FileId(2)));
    assert_eq!(inputs.files[FileId(2)].origin, Origin::Closure { needing: FileId(1), index: 0 });
    assert_eq!(inputs.first_closure_file(), FileId(2));
    // §4.6: the probes of the script's -lg, then of the edge.
    let entry: Vec<_> = link.probes.iter().filter(|p| p.search == SearchRef::Entry(0)).collect();
    assert_eq!(entry.last().unwrap().outcome, ProbeOutcome::Found(1));
    let edge: Vec<_> = link.probes.iter().filter(|p| p.search == SearchRef::Edge(0)).collect();
    assert_eq!(edge.last().unwrap().path, b"d/libh.so");
    assert_eq!(edge.last().unwrap().outcome, ProbeOutcome::Found(2));
}

#[test]
fn s1_load_errors_give_none() {
    let s = Scratch::new("s1err");
    s.write("junk.o", "\x01\x02 not anything\n");
    let (inputs, report) = load_in(s.path(), &[M, "nothere.o", "junk.o"], 1);
    assert!(inputs.is_none());
    // §12.3: every error of the line, in key order.
    assert_eq!(ids(&report), ["cannot-open-file", "unknown-file-type"]);
    let text = report.render(&mortar_diag::Style::plain("mortar"));
    assert_eq!(
        text,
        "mortar: error: nothere.o: cannot open nothere.o: No such file or directory\n\
         mortar: error: junk.o: file not recognized: file format not recognized\n"
    );
}

#[test]
fn s1_load_warnings_do_not_fail() {
    let s = Scratch::new("s1warn");
    if !common::have(&["native"]) {
        return;
    }
    common::cc(s.path(), "native", "f.o", "int f;\n", &[]);
    s.write("t.txt", "text\n");
    common::ar(s.path(), "libt.a", &["t.txt"], false);
    let (inputs, report) = load_in(s.path(), &[M, "f.o", "libt.a"], 1);
    assert_eq!(ids(&report), ["ignored-member"]);
    assert!(!report.failed());
    let inputs = inputs.expect("a warning does not stop the load");
    assert_eq!(
        inputs.archives[ArchiveId(0)].members.start,
        inputs.archives[ArchiveId(0)].members.end
    );
}

// ---------------------------------------------------------------- §10, §12

/// A command-line item of the generated lines of `s10_1_walk_reference`.
#[derive(Clone, Debug)]
enum Item {
    File(&'static str),
    Group(Vec<&'static str>),
    Lib(Vec<&'static str>),
}

/// The §10.1 pseudocode over the fixture of `s10_1_walk_reference`, whose
/// kinds the test knows: `(display name, group ordinal, lazy)` per row,
/// and the number of archive mentions.
#[derive(Default)]
struct Reference {
    rows: Vec<(String, Option<u32>, bool)>,
    archives: u32,
    soname_loaded: bool,
    groups: Vec<(u32, u32)>,
    script_groups: u32,
}

impl Reference {
    /// `group`: `(0, n)` the n-th command-line group, `(1, n)` the n-th
    /// script `GROUP` outside one.
    fn row(&mut self, name: String, group: Option<(u32, u32)>, lazy: bool) {
        let ordinal = group.map(|g| {
            if let Some(at) = self.groups.iter().position(|x| *x == g) {
                at as u32 + 1
            } else {
                self.groups.push(g);
                self.groups.len() as u32
            }
        });
        self.rows.push((name, ordinal, lazy));
    }

    fn visit(&mut self, name: &str, group: Option<(u32, u32)>, lazy: bool) {
        match name {
            "o1.o" | "o2.o" => self.row(name.to_owned(), group, lazy),
            "liba.a" => {
                self.archives += 1;
                self.row("liba.a(m1.o)".to_owned(), group, false);
                self.row("liba.a(m2.o)".to_owned(), group, false);
            }
            "libs1.so" | "libs2.so" if !self.soname_loaded => {
                self.soname_loaded = true;
                self.row(name.to_owned(), group, false);
            }
            "s1.ld" => {
                self.visit("o2.o", group, lazy);
                self.visit("s2.ld", group, lazy);
            }
            "s2.ld" => {
                let g = group.or_else(|| {
                    self.script_groups += 1;
                    Some((1, self.script_groups))
                });
                self.visit("liba.a", g, lazy);
            }
            _ => {}
        }
    }
}

#[test]
fn s10_1_walk_reference() {
    let s = Scratch::new("s10ref");
    if !common::have(&["native"]) {
        return;
    }
    for (o, f) in [("o1.o", "o1"), ("o2.o", "o2"), ("m1.o", "m1"), ("m2.o", "m2")] {
        common::cc(s.path(), "native", o, &format!("int {f}(void){{return 1;}}\n"), &[]);
    }
    s.write("t.txt", "not an object\n");
    common::ar(s.path(), "liba.a", &["m1.o", "t.txt", "m2.o"], false);
    let so = ["-shared", "-fPIC", "-nostdlib", "-Wl,-soname,libs.so"];
    common::cc(s.path(), "native", "libs1.so", "int s(void){return 1;}\n", &so);
    common::cc(s.path(), "native", "libs2.so", "int s(void){return 2;}\n", &so);
    s.write("s1.ld", "INPUT(o2.o s2.ld)\n");
    s.write("s2.ld", "GROUP(liba.a)\n");
    s.write("empty.o", "");
    let files = ["o1.o", "o2.o", "liba.a", "libs1.so", "libs2.so", "s1.ld", "s2.ld", "empty.o"];
    let lazy_files = ["o1.o", "o2.o", "empty.o"];
    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n as u64) as usize
    };
    for line in 0..120 {
        let mut items = Vec::new();
        for _ in 0..=next(7) {
            let item = match next(6) {
                0 => Item::Group((0..=next(3)).map(|_| files[next(files.len())]).collect()),
                1 => Item::Lib((0..=next(3)).map(|_| lazy_files[next(lazy_files.len())]).collect()),
                _ => Item::File(files[next(files.len())]),
            };
            items.push(item);
        }
        let mut args: Vec<&str> = EXE.to_vec();
        let mut want = Reference::default();
        let mut cli_groups = 0;
        for item in &items {
            match item {
                Item::File(f) => {
                    args.push(f);
                    want.visit(f, None, false);
                }
                Item::Group(fs) => {
                    cli_groups += 1;
                    args.push("--start-group");
                    for f in fs {
                        args.push(f);
                        want.visit(f, Some((0, cli_groups)), false);
                    }
                    args.push("--end-group");
                }
                Item::Lib(fs) => {
                    args.push("--start-lib");
                    for f in fs {
                        args.push(f);
                        want.visit(f, None, true);
                    }
                    args.push("--end-lib");
                }
            }
        }
        let (inputs, report) = load_in(s.path(), &args, 1 + line % 3);
        assert!(!report.failed(), "{args:?}: {report:?}");
        let inputs = inputs.expect("loaded");
        let got: Vec<(String, Option<u32>, bool)> = inputs
            .files
            .iter()
            .skip(1)
            .map(|(id, f)| {
                let path = String::from_utf8_lossy(f.path).into_owned();
                let name = match f.member {
                    Some(m) => format!("{path}({})", String::from_utf8_lossy(m.name)),
                    None => path,
                };
                let lazy = f.state.contains(mortar_core::inputs::InputFlags::LAZY);
                (name, inputs.group_of(id).map(|g| g.0), lazy)
            })
            .collect();
        assert_eq!(got, want.rows, "{args:?}");
        assert_eq!(inputs.archives.len(), want.archives, "{args:?}");
    }
}

#[test]
#[allow(clippy::many_single_char_names)]
fn s10_3_rows_are_per_mention() {
    let s = Scratch::new("s10rows");
    if !objects_fixture(&s) {
        return;
    }
    s.write("one.ld", "INPUT(libb.a)\n");
    s.write("two.ld", "INPUT(b1.o libb.a)\n");
    // The columns that are a function of the bytes and the own mention.
    let row = |inputs: &Inputs<'_>, f: u32| {
        let f = &inputs.files[FileId(f)];
        let member = f.member.map(|m| (m.name, m.ordinal, m.offset));
        format!("{:?} {member:?} {:?} {} {} {:?}", f.path, f.state, f.sections, f.syms.len, f.hash)
    };
    let (a, _) = load_in(s.path(), &[M, "a.o", "libb.a"], 1);
    let (b, _) =
        load_in(s.path(), &[M, "b2.o", "--as-needed", "a.o", "--no-as-needed", "libb.a"], 1);
    let (a, b) = (a.unwrap(), b.unwrap());
    // Another mention before them: the archive members' rows are equal.
    assert_eq!(row(&a, 2), row(&b, 3));
    assert_eq!(row(&a, 3), row(&b, 4));
    // Its own state changes its own row only.
    assert_ne!(row(&a, 1), row(&b, 2));
    // Another script naming the same archive: equal rows.
    let (c, _) = load_in(s.path(), &[M, "one.ld"], 1);
    let (d, _) = load_in(s.path(), &[M, "two.ld"], 1);
    let (c, d) = (c.unwrap(), d.unwrap());
    assert_eq!(row(&c, 1), row(&d, 2));
    assert_eq!(row(&c, 2), row(&d, 3));
}

#[test]
fn s10_4_opened_and_dependency_file() {
    let s = Scratch::new("s10opened");
    if !objects_fixture(&s) {
        return;
    }
    s.write("s.ld", "INPUT(a.o)\n");
    s.write("empty.o", "");
    let (inputs, report) = load_in(s.path(), &[M, "s.ld", "libb.a", "s.ld", "empty.o"], 1);
    assert!(report.diagnostics().is_empty(), "{report:?}");
    let inputs = inputs.unwrap();
    let opened: Vec<_> = inputs
        .link
        .opened
        .iter()
        .map(|o| (String::from_utf8_lossy(o.path).into_owned(), o.kind))
        .collect();
    assert_eq!(
        opened,
        [
            ("s.ld".to_owned(), OpenedKind::Script),
            ("a.o".to_owned(), OpenedKind::Object(FileId(1))),
            ("libb.a".to_owned(), OpenedKind::Archive(ArchiveId(0))),
            ("s.ld".to_owned(), OpenedKind::Script),
            ("a.o".to_owned(), OpenedKind::Object(FileId(4))),
            ("empty.o".to_owned(), OpenedKind::Empty),
        ]
    );
    // --dependency-file: each found path once, in this order [E21].
    let deps = mortar_input::dependency_paths(&inputs.link);
    assert_eq!(deps, [&b"s.ld"[..], b"a.o", b"libb.a", b"empty.o"]);
}

#[test]
fn s12_3_all_errors_reported() {
    let s = Scratch::new("s12all");
    if !common::have(&["native", "x86"]) {
        return;
    }
    common::cc(s.path(), "native", "usef.o", "int f(void){return 1;}\n", &[]);
    common::cc(s.path(), "x86", "xf.o", "int f;\n", &[]);
    s.write("g1.o", "garbage\x01");
    // [E20]: GNU ld stops at g1.o; every error of the line is reported.
    let line = [M, "usef.o", "nosuch1.o", "-lnosuch", "nosuch2.o", "g1.o", "xf.o"];
    let (inputs, report) = load_in(s.path(), &line, 2);
    assert!(inputs.is_none());
    assert_eq!(
        ids(&report),
        [
            "cannot-open-file",
            "library-not-found",
            "cannot-open-file",
            "unknown-file-type",
            "incompatible-file"
        ]
    );
}

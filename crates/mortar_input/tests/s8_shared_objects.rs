//! `spec/input.md` §8: shared objects.
mod common;

use mortar_testutil::Scratch;

const C: &str = "int f(void) { return 1; }\n";

fn so(dir: &std::path::Path, name: &str, src: &str, extra: &[&str]) -> bool {
    let mut flags = vec!["-shared", "-fPIC", "-nostdlib"];
    flags.extend_from_slice(extra);
    common::cc(dir, "native", name, src, &flags).is_some()
}

#[test]
fn s8_1_pie_refused() {
    let s = Scratch::new("s8pie");
    if common::cc(
        s.path(),
        "native",
        "pie",
        "int main(void){return 0;}\n",
        &["--link", "-fPIE", "-pie"],
    )
    .is_none()
    {
        return;
    }
    let out = common::run(s.path(), &["-shared", "-maarch64linux", "pie"]);
    assert_eq!(
        common::report(&out),
        [
            "error: pie: unsupported object type: DF_1_PIE\n>>> a position-independent executable cannot be linked against"
        ]
    );
    assert!(out.files.is_empty());
}

#[test]
fn s8_1_libc_so_6_accepted() {
    let s = Scratch::new("s8libc");
    if !common::have(&["native"]) {
        return;
    }
    let libc = "/lib/aarch64-linux-gnu/libc.so.6";
    if !std::path::Path::new(libc).exists() {
        assert!(!mortar_testutil::tools_required(), "{libc} is required");
        return;
    }
    let out = common::run(s.path(), &["-shared", "-maarch64linux", libc]);
    assert!(out.problems.is_empty(), "{:?}", common::report(&out));
    assert_eq!(out.files[0].soname.as_ref().map(|s| s.0.to_vec()), Some(b"libc.so.6".to_vec()));
}

#[test]
fn s8_1_no_section_headers_refused() {
    let s = Scratch::new("s8nosh");
    let Some(objcopy) = common::need("llvm-objcopy") else { return };
    if !so(s.path(), "libf.so", C, &[]) {
        return;
    }
    let o = common::tool_run(&objcopy, &["--strip-sections".into(), "libf.so".into()], s.path());
    assert!(o.success(), "{}", o.stderr_text());
    let out = common::run(s.path(), &["-shared", "-maarch64linux", "libf.so"]);
    assert_eq!(
        common::report(&out),
        ["error: libf.so: unsupported feature: shared object without section headers"]
    );
}

/// The file offset and size of `.dynamic`, from `readelf -SW`.
fn dynamic_section(dir: &std::path::Path, file: &str) -> (usize, usize) {
    let readelf = common::need("readelf").unwrap();
    let o = common::tool_run(&readelf, &["-SW".into(), file.into()], dir);
    let line = o.stdout_text().lines().find(|l| l.contains(" .dynamic ")).unwrap().to_owned();
    let w: Vec<&str> = line.split_whitespace().collect();
    let i = w.iter().position(|x| *x == ".dynamic").unwrap();
    // name type address offset size
    let off = usize::from_str_radix(w[i + 3], 16).unwrap();
    let size = usize::from_str_radix(w[i + 4], 16).unwrap();
    (off, size)
}

#[test]
fn s8_1_soname_twice() {
    let s = Scratch::new("s8soname2");
    if !so(s.path(), "libf.so", C, &["-Wl,-soname,libf.so.1", "-Wl,--no-as-needed", "-lc"]) {
        return;
    }
    let (off, size) = dynamic_section(s.path(), "libf.so");
    let mut b = std::fs::read(s.join("libf.so")).unwrap();
    let mut soname_val = None;
    for e in (off..off + size).step_by(16) {
        let tag = u64::from_le_bytes(b[e..e + 8].try_into().unwrap());
        if tag == 14 {
            soname_val = Some(b[e + 8..e + 16].to_vec());
        }
    }
    let val = soname_val.unwrap();
    // The DT_NEEDED entry becomes a second DT_SONAME.
    let e = (off..off + size)
        .step_by(16)
        .find(|&e| u64::from_le_bytes(b[e..e + 8].try_into().unwrap()) == 1)
        .unwrap();
    b[e..e + 8].copy_from_slice(&14u64.to_le_bytes());
    b[e + 8..e + 16].copy_from_slice(&val);
    s.write("libf.so", b);
    let out = common::run(s.path(), &["-shared", "-maarch64linux", "libf.so"]);
    assert_eq!(common::ids(&out), ["malformed-input"], "{:?}", common::report(&out));
    assert!(out.files.is_empty());
}

#[test]
fn s8_1_static_link_of_shared_object() {
    let s = Scratch::new("s8static");
    std::fs::create_dir_all(s.join("d1")).unwrap();
    if !so(&s.join("d1"), "libf.so", C, &[]) {
        return;
    }
    let out = common::run(s.path(), &["-shared", "-maarch64linux", "-Bstatic", "d1/libf.so"]);
    assert_eq!(
        common::report(&out),
        ["error: d1/libf.so: attempted static link of dynamic object d1/libf.so"]
    );
    // -l under -Bstatic finds only .a: not found, not this error.
    let out =
        common::run(s.path(), &["-shared", "-maarch64linux", "-Ld1", "-Bstatic", "-l:libf.so"]);
    assert_eq!(common::ids(&out), ["static-link-of-shared-object"]);
}

#[test]
fn s8_2_one_per_soname_and_duplicate_soname_warning_only_when_bytes_differ() {
    let s = Scratch::new("s8onesoname");
    std::fs::create_dir_all(s.join("d1")).unwrap();
    std::fs::create_dir_all(s.join("d2")).unwrap();
    if !so(&s.join("d1"), "libf.so", C, &["-Wl,-soname,libf.so.1"]) {
        return;
    }
    so(
        &s.join("d2"),
        "libf.so",
        "int f(void){return 2;}\nint g(void){return 3;}\n",
        &["-Wl,-soname,libf.so.1"],
    );
    std::fs::copy(s.join("d1/libf.so"), s.join("copy.so")).unwrap();
    let out = common::run(
        s.path(),
        &["-shared", "-maarch64linux", "d1/libf.so", "d1/libf.so", "copy.so", "d2/libf.so"],
    );
    assert_eq!(
        common::report(&out),
        [
            "warning: d2/libf.so: DT_SONAME 'libf.so.1' is already loaded; this file is not used\n>>> loaded from d1/libf.so"
        ]
    );
    assert_eq!(common::names(&out), ["d1/libf.so"]);
    assert_eq!(out.link.sonames.len(), 1);
    assert_eq!(out.link.sonames[0].mentions.len(), 4);
    assert_eq!(common::opened(&out), ["d1/libf.so", "d1/libf.so", "copy.so", "d2/libf.so"]);
}

#[test]
fn s8_2_as_needed_merge() {
    let s = Scratch::new("s8merge");
    if !so(s.path(), "libf.so", C, &["-Wl,-soname,libf.so.1"]) {
        return;
    }
    let mixed = common::run(
        s.path(),
        &[
            "-shared",
            "-maarch64linux",
            "--as-needed",
            "libf.so",
            "--no-as-needed",
            "--copy-dt-needed-entries",
            "libf.so",
        ],
    );
    let u = &mixed.link.sonames[0];
    assert!(!u.as_needed);
    assert!(u.copy_dt_needed);
    // The row keeps the first mention's state.
    assert!(mixed.files[0].state.as_needed);
    assert!(!mixed.files[0].state.copy_dt_needed);
    let all =
        common::run(s.path(), &["-shared", "-maarch64linux", "--as-needed", "libf.so", "libf.so"]);
    assert!(all.link.sonames[0].as_needed);
}

#[test]
fn s8_3_needed_name_table_and_s8_5_sonameless_spellings() {
    let s = Scratch::new("s8needed");
    std::fs::create_dir_all(s.join("ns2")).unwrap();
    if !so(&s.join("ns2"), "libf.so", C, &[]) {
        return;
    }
    let abs = s.join("ns2/libf.so");
    let abs = abs.to_str().unwrap();
    let out = common::run(
        s.path(),
        &[
            "-shared",
            "-maarch64linux",
            "-Lns2",
            "-lf",
            "ns2/libf.so",
            "./ns2/libf.so",
            abs,
            "ns2/libf.so",
        ],
    );
    assert!(out.problems.is_empty(), "{:?}", common::report(&out));
    let sonames: Vec<_> = out
        .files
        .iter()
        .map(|f| {
            let (n, from_dt) = f.soname.as_ref().unwrap();
            assert!(!from_dt);
            String::from_utf8_lossy(n).into_owned()
        })
        .collect();
    assert_eq!(sonames, ["libf.so", "ns2/libf.so", "./ns2/libf.so", abs]);
    assert_eq!(out.link.sonames[1].mentions.len(), 2);
}

#[test]
fn s8_3_script_names_soname_less_object() {
    let s = Scratch::new("s8scriptns");
    std::fs::create_dir_all(s.join("ns")).unwrap();
    if !so(&s.join("ns"), "libf.so", C, &[]) {
        return;
    }
    so(&s.join("ns"), "libg.so", C, &["-Wl,-soname,libg.so.1"]);
    s.write("s.ld", "INPUT(ns/libf.so ns/libg.so)\n");
    let out = common::run(s.path(), &["-shared", "-maarch64linux", "s.ld"]);
    assert_eq!(
        common::report(&out),
        [
            "error: ns/libf.so: unsupported feature: shared object without DT_SONAME named by a linker script\n>>> named by s.ld:1:7\n>>> named by argument 's.ld'"
        ]
    );
    assert_eq!(common::names(&out), ["ns/libg.so"]);
}

#[test]
fn s8_4_no_interpreter_refused() {
    let s = Scratch::new("s8nointerp");
    if !so(s.path(), "libf.so", C, &[]) {
        return;
    }
    so(s.path(), "libg.so", C, &["-Wl,-soname,libg.so"]);
    let out = common::run(s.path(), &["-maarch64linux", "libf.so", "libg.so"]);
    assert_eq!(
        common::report(&out),
        [
            "error: libf.so: unsupported feature: dynamic executable without -dynamic-linker\n>>> the shared object is libf.so"
        ]
    );
    let out = common::run(
        s.path(),
        &["-maarch64linux", "-dynamic-linker", "/lib/ld-linux-aarch64.so.1", "libf.so"],
    );
    assert!(out.problems.is_empty());
}

const EXE: &[&str] = &["-maarch64linux", "-dynamic-linker", "/lib/ld-linux-aarch64.so.1"];

fn args<'a>(extra: &[&'a str]) -> Vec<&'a str> {
    let mut v: Vec<&str> = EXE.to_vec();
    v.extend_from_slice(extra);
    v
}

/// `dg/libg.so` needs `libh.so` (no soname), which needs `libz.so.9`.
fn closure_fixture(s: &Scratch, g_flags: &[&str]) -> bool {
    std::fs::create_dir_all(s.join("dg")).unwrap();
    std::fs::create_dir_all(s.join("hid")).unwrap();
    if !so(&s.join("hid"), "libz.so.9", "int z(void){return 9;}\n", &["-Wl,-soname,libz.so.9"]) {
        return false;
    }
    let hid = s.join("hid");
    let l = format!("-L{}", hid.display());
    so(
        &hid,
        "libh.so",
        "extern int hook(void);\nint h(void){return hook();}\n",
        &[&l, "-Wl,--no-as-needed", "-l:libz.so.9"],
    );
    std::fs::rename(hid.join("libz.so.9"), s.join("libz.so.9")).unwrap();
    // hid/libz.so.9 is moved out: libh's own need must be searched for.
    let mut g = vec![l.as_str(), "-Wl,--no-as-needed", "-lh"];
    g.extend_from_slice(g_flags);
    so(&s.join("dg"), "libg.so", "extern int h(void);\nint g(void){return h();}\n", &g);
    true
}

#[test]
fn s8_6_numbering_and_rpath_link_colon_split() {
    let s = Scratch::new("s8closure");
    if !closure_fixture(&s, &[]) {
        return;
    }
    let out = common::run_closure(
        s.path(),
        &args(&["dg/libg.so", "-rpath-link", "/nonexist::hid", "-L."]),
    );
    assert!(out.problems.is_empty(), "{:?}", common::report(&out));
    assert_eq!(common::names(&out), ["dg/libg.so", "hid/libh.so", "./libz.so.9"]);
    let edges: Vec<_> = out
        .link
        .closure
        .iter()
        .map(|e| (e.needing, String::from_utf8_lossy(&e.name).into_owned(), e.satisfied))
        .collect();
    assert_eq!(edges, [(1, "libh.so".to_owned(), Some(2)), (2, "libz.so.9".to_owned(), Some(3))]);
    // A closure file is a mention with `as_needed` on, and its row says
    // which edge led to it.
    assert_eq!(out.files[1].origin.edge, Some(0));
    assert!(out.files[1].state.as_needed);
}

#[test]
fn s8_6_when_loaded() {
    let s = Scratch::new("s8closurewhen");
    if !closure_fixture(&s, &[]) {
        return;
    }
    let shared = common::run_closure(
        s.path(),
        &["-shared", "-maarch64linux", "dg/libg.so", "-rpath-link", "hid"],
    );
    assert_eq!(shared.files.len(), 1);
    let checked = common::run_closure(
        s.path(),
        &[
            "-shared",
            "-maarch64linux",
            "--no-allow-shlib-undefined",
            "dg/libg.so",
            "-rpath-link",
            "hid",
            "-L.",
        ],
    );
    assert_eq!(checked.files.len(), 3, "{:?}", common::report(&checked));
    let pie = common::run_closure(
        s.path(),
        &[
            "-pie",
            "-maarch64linux",
            "-dynamic-linker",
            "/l",
            "dg/libg.so",
            "-rpath-link",
            "hid",
            "-L.",
        ],
    );
    assert_eq!(pie.files.len(), 3, "{:?}", common::report(&pie));
    let copy = common::run_closure(
        s.path(),
        &[
            "-shared",
            "-maarch64linux",
            "--copy-dt-needed-entries",
            "dg/libg.so",
            "-rpath-link",
            "hid",
            "-L.",
        ],
    );
    assert_eq!(copy.files.len(), 3, "{:?}", common::report(&copy));
    let reloc = common::run_closure(
        s.path(),
        &["-r", "-maarch64linux", "dg/libg.so", "-rpath-link", "hid"],
    );
    assert!(reloc.link.closure.is_empty());
    // Not asked for (the driver of stage A): nothing.
    let off = common::run(s.path(), &args(&["dg/libg.so", "-rpath-link", "hid"]));
    assert!(off.link.closure.is_empty());
}

#[test]
fn s8_6_origin() {
    let s = Scratch::new("s8origin");
    if !closure_fixture(&s, &["-Wl,-rpath,$ORIGIN/../hid"]) {
        return;
    }
    std::fs::rename(s.join("libz.so.9"), s.join("hid/libz.so.9")).unwrap();
    let out = common::run_closure(s.path(), &args(&["dg/libg.so"]));
    assert!(out.problems.is_empty(), "{:?}", common::report(&out));
    // libh.so through $ORIGIN; libz.so.9 through libh's own directory.
    assert_eq!(common::names(&out), ["dg/libg.so", "dg/../hid/libh.so", "dg/../hid/libz.so.9"]);
}

#[test]
fn s8_6_not_found_warning() {
    let s = Scratch::new("s8notfound");
    if !closure_fixture(&s, &[]) {
        return;
    }
    let out = common::run_closure(s.path(), &args(&["dg/libg.so", "-Lhid"]));
    assert_eq!(
        common::report(&out),
        [
            "warning: hid/libh.so: libz.so.9, needed by hid/libh.so, not found (try using -rpath or -rpath-link)\n>>> searched hid\n>>> searched hid",
        ],
    );
}

#[test]
fn s8_6_closure_lib_is_refused() {
    let s = Scratch::new("s8lib");
    if !closure_fixture(&s, &["-Wl,-rpath,/x/$LIB"]) {
        return;
    }
    let out = common::run_closure(s.path(), &args(&["dg/libg.so", "-rpath-link", "hid", "-L."]));
    assert_eq!(common::ids(&out), ["refused"], "{:?}", common::report(&out));
    assert!(common::report(&out)[0].contains("unsupported feature: $LIB in DT_RUNPATH"));
}

#[test]
fn s8_6_closure_satisfied_by_walk() {
    let s = Scratch::new("s8satisfied");
    if !closure_fixture(&s, &[]) {
        return;
    }
    let out =
        common::run_closure(s.path(), &args(&["dg/libg.so", "libz.so.9", "-rpath-link", "hid"]));
    assert!(out.problems.is_empty(), "{:?}", common::report(&out));
    assert_eq!(common::names(&out), ["dg/libg.so", "libz.so.9", "hid/libh.so"]);
    let edges: Vec<_> =
        out.link.closure.iter().map(|e| (e.needing, e.index, e.satisfied)).collect();
    assert_eq!(edges, [(1, 0, Some(3)), (3, 0, Some(2))]);
    // §8.5: hid/libh.so named by path has soname `hid/libh.so`, so the
    // edge `libh.so` is not satisfied by it and is searched.
    let out = common::run_closure(s.path(), &args(&["dg/libg.so", "hid/libh.so", "libz.so.9"]));
    assert_eq!(common::ids(&out), ["needed-not-found"]);
}

/// `libg.so` (in `d4`, `DT_RUNPATH` `runpath`) needs `libz.so.9`; a copy
/// of `libz.so.9` in each of `d1` .. `d5`.
fn search_fixture(s: &Scratch, runpath: &str) -> bool {
    for d in ["d1", "d2", "d3", "d4", "d5", "z"] {
        std::fs::create_dir_all(s.join(d)).unwrap();
    }
    if !so(&s.join("z"), "libz.so.9", "int z(void){return 9;}\n", &["-Wl,-soname,libz.so.9"]) {
        return false;
    }
    let z = format!("-L{}", s.join("z").display());
    let rp = format!("-Wl,--enable-new-dtags,-rpath,{runpath}");
    so(
        &s.join("d4"),
        "libg.so",
        "extern int z(void);\nint g(void){return z();}\n",
        &[&z, "-Wl,--no-as-needed", "-l:libz.so.9", &rp],
    );
    for d in ["d1", "d2", "d3", "d5"] {
        std::fs::copy(s.join("z/libz.so.9"), s.join(d).join("libz.so.9")).unwrap();
    }
    std::fs::rename(s.join("z/libz.so.9"), s.join("d4/libz.so.9")).unwrap();
    true
}

/// The found path of the closure file.
fn found(out: &mortar_input::walk::Loaded<'_>) -> String {
    assert!(out.problems.is_empty(), "{:?}", common::report(out));
    common::names(out).last().cloned().unwrap_or_default()
}

#[test]
fn s8_6_closure_search_order() {
    let s = Scratch::new("s8order");
    let d3 = s.join("d3").display().to_string();
    if !search_fixture(&s, &d3) {
        return;
    }
    let line = args(&["d4/libg.so", "-rpath-link", "d1", "-rpath", "d2", "-Ld5"]);
    // Each step shadows the next: remove the file a step found, and the
    // next step finds its own.
    let mut want = vec![
        "d1/libz.so.9".to_owned(),
        "d2/libz.so.9".to_owned(),
        format!("{d3}/libz.so.9"),
        "d4/libz.so.9".to_owned(),
        "d5/libz.so.9".to_owned(),
    ];
    want.reverse();
    while let Some(w) = want.pop() {
        let out = common::run_closure(s.path(), &line);
        assert_eq!(found(&out), w);
        std::fs::remove_file(s.join(&w)).unwrap();
    }
    let out = common::run_closure(s.path(), &line);
    assert_eq!(common::ids(&out), ["needed-not-found"]);
}

#[test]
fn s8_6_sysroot_applied() {
    let s = Scratch::new("s8sysroot");
    if !search_fixture(&s, "/d3") {
        return;
    }
    let root = s.path().display().to_string();
    let sysroot = format!("--sysroot={root}");
    // An absolute DT_RUNPATH entry is a target path: inside the sysroot.
    std::fs::remove_file(s.join("d4/libz.so.9")).unwrap();
    let out = common::run_closure(s.path(), &args(&[&sysroot, "d4/libg.so"]));
    assert_eq!(found(&out), format!("{root}/d3/libz.so.9"));
    // `=` in -rpath-link is inside the sysroot; a plain directory is not.
    let out =
        common::run_closure(s.path(), &args(&[&sysroot, "d4/libg.so", "-rpath-link", "=/d1"]));
    assert_eq!(found(&out), format!("{root}/d1/libz.so.9"));
    let out = common::run_closure(s.path(), &args(&[&sysroot, "d4/libg.so", "-rpath-link", "d2"]));
    assert_eq!(found(&out), "d2/libz.so.9");
}

#[test]
fn s8_6_skip_applies() {
    let s = Scratch::new("s8skip");
    if !common::have(&["x86"]) || !search_fixture(&s, "/nonexistent") {
        return;
    }
    std::fs::remove_file(s.join("d1/libz.so.9")).unwrap();
    common::cc(
        &s.join("d1"),
        "x86",
        "libz.so.9",
        "int z(void){return 9;}\n",
        &["-shared", "-fPIC", "-nostdlib", "-Wl,-soname,libz.so.9"],
    );
    let out = common::run_closure(s.path(), &args(&["d4/libg.so", "-rpath-link", "d1:d2"]));
    assert_eq!(
        common::report(&out),
        ["warning: d4/libg.so: skipping incompatible d1/libz.so.9 when searching for libz.so.9"],
    );
    assert_eq!(common::names(&out).last().unwrap(), "d2/libz.so.9");
}

#[test]
fn s8_6_as_needed_objects_have_closures() {
    let s = Scratch::new("s8asneeded");
    if !closure_fixture(&s, &[]) {
        return;
    }
    // libg.so may turn out unneeded: its closure is loaded all the same.
    let out = common::run_closure(
        s.path(),
        &args(&["--as-needed", "dg/libg.so", "-rpath-link", "hid", "-L."]),
    );
    assert!(out.problems.is_empty(), "{:?}", common::report(&out));
    assert_eq!(common::names(&out), ["dg/libg.so", "hid/libh.so", "./libz.so.9"]);
    assert!(out.files[0].state.as_needed);
}

/// The `found N at P` lines of GNU ld's `--verbose`: `N` and `P`'s real
/// path.
fn gnu_found(
    ld: &std::path::Path,
    args: &[std::ffi::OsString],
    dir: &std::path::Path,
) -> Vec<(String, std::path::PathBuf)> {
    let mut a = args.to_vec();
    a.push("--verbose".into());
    let o = common::tool_run(ld, &a, dir);
    assert!(o.success(), "{}", o.stderr_text());
    let text = String::from_utf8_lossy(&o.stdout).into_owned();
    let mut out: Vec<(String, std::path::PathBuf)> = text
        .lines()
        .filter_map(|l| l.strip_prefix("found "))
        .filter_map(|l| l.split_once(" at "))
        .map(|(n, p)| (n.to_owned(), std::fs::canonicalize(p).unwrap()))
        .collect();
    out.sort();
    out.dedup();
    out
}

#[test]
fn s8_6_system_lines() {
    let s = Scratch::new("s8system");
    let cout = "#include <iostream>\nint main(){std::cout << 1;}\n";
    let elf =
        "extern unsigned elf_version(unsigned);\nint main(void){return (int)elf_version(1);}\n";
    // [E29]: driver, ld.bfd, source, the driver's extra arguments.
    let lines: [(&str, &str, &str, &str, &[&str]); 4] = [
        ("g++", "ld.bfd", "e.cc", cout, &[]),
        ("x86_64-linux-gnu-g++", "x86_64-linux-gnu-ld.bfd", "e.cc", cout, &[]),
        ("i686-linux-gnu-g++", "i686-linux-gnu-ld.bfd", "e.cc", cout, &[]),
        ("gcc", "ld.bfd", "e.c", elf, &["-l:libelf.so.1"]),
    ];
    let mut checked = 0;
    for (driver, bfd, src, text, extra) in lines {
        let (Some(d), Some(ld)) = (common::need(driver), common::need(bfd)) else { continue };
        let w = s.join(driver);
        std::fs::create_dir_all(&w).unwrap();
        std::fs::write(w.join(src), text).unwrap();
        let c = common::tool_run(&d, &["-c".into(), src.into(), "-o".into(), "e.o".into()], &w);
        assert!(c.success(), "{driver}: {}", c.stderr_text());
        let mut a = vec!["e.o"];
        a.extend_from_slice(extra);
        a.extend(["-o", "a.out"]);
        let Some(args) = common::driver_link_args(&d, &a, &w) else { continue };
        let gnu = gnu_found(&ld, &args, &w);
        let out = common::run_closure(&w, &args);
        assert!(!out.failed(), "{driver}: {:?}", common::report(&out));
        // Every edge GNU ld searched for is satisfied by the same file.
        // (Its `found` lines include `-l` searches through scripts, such
        // as `libm.so`: only `DT_NEEDED` names are edges.)
        let mut ours: Vec<(String, std::path::PathBuf)> = Vec::new();
        for e in &out.link.closure {
            let Some(f) = e.satisfied else { continue };
            let row = &out.files[f as usize - 1];
            ours.push((
                String::from_utf8_lossy(&e.name).into_owned(),
                std::fs::canonicalize(w.join(
                    <std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(&row.path),
                ))
                .unwrap(),
            ));
        }
        for (n, p) in gnu.iter().filter(|(n, _)| n.contains(".so.")) {
            assert!(
                ours.iter().any(|(m, q)| m == n && q == p),
                "{driver}: {n} at {}: ours {ours:?}",
                p.display()
            );
        }
        // And no closure file is one GNU ld did not find.
        for row in out.files.iter().filter(|r| r.origin.edge.is_some()) {
            let name = row
                .soname
                .as_ref()
                .map(|s| String::from_utf8_lossy(&s.0).into_owned())
                .unwrap_or_default();
            assert!(
                gnu.iter().any(|(n, _)| *n == name),
                "{driver}: closure file {name} not found by GNU ld"
            );
        }
        checked += 1;
    }
    if std::env::var_os("MORTAR_REQUIRE_TOOLS").is_some() {
        assert_eq!(checked, 4, "every line of [E29] runs");
    }
}

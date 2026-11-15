//! `spec/input.md` §6 and §7: relocatable objects and archives.
mod common;

use mortar_testutil::Scratch;

const C: &str = "int f(void) { return 1; }\n";

#[test]
fn s6_object_rows() {
    let s = Scratch::new("s6rows");
    let Some(gxx) = common::need("g++") else { return };
    s.write("t.cc", "template <class T> T id(T x) { return x; }\nint g(int x) { return id(x); }\nint h;\nstatic int l;\nint *p = &l;\n");
    assert!(common::tool_run(&gxx, &["-c".into(), "t.cc".into()], s.path()).success());
    let out = common::run(s.path(), &["-maarch64linux", "t.o"]);
    assert!(out.problems.is_empty());
    let f = &out.files[0];
    let readelf = common::need("readelf").unwrap();
    let o = common::tool_run(&readelf, &["-hsSW".into(), "t.o".into()], s.path());
    let text = o.stdout_text();
    let shnum: u32 = text
        .lines()
        .find(|l| l.contains("Number of section headers"))
        .unwrap()
        .split(':')
        .nth(1)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(f.sections, shnum);
    let globals = text.lines().filter(|l| l.contains(" GLOBAL ") || l.contains(" WEAK ")).count();
    assert_eq!(f.globals as usize, globals);
    assert_eq!(f.groups, 1, "the COMDAT group of id<int>");
}

#[test]
fn s7_1_archive_mentions_get_ids() {
    let s = Scratch::new("s7ids");
    if !common::have(&["native"]) {
        return;
    }
    common::cc(s.path(), "native", "f.o", C, &[]);
    s.write("t.txt", "text\n");
    common::ar(s.path(), "libf.a", &["f.o"], false);
    common::ar(s.path(), "libt.a", &["t.txt"], false);
    let out = common::run(s.path(), &["-maarch64linux", "libf.a", "libt.a", "-L.", "-lf"]);
    assert_eq!(out.archives.len(), 3);
    let a: Vec<_> = out.files.iter().map(|f| f.member.as_ref().unwrap().archive).collect();
    assert_eq!(a, [0, 2]);
    assert_eq!(out.files[0].identity.mention, 0);
    assert_eq!(common::names(&out), ["libf.a(f.o)", "./libf.a(f.o)"]);
}

#[test]
#[ignore = "expected-fail: Archive::parse rejects a malformed symbol index until WP-020d (spec/input.md §17)"]
fn s7_1_malformed_index_warns() {
    let s = Scratch::new("s7badidx");
    if !common::have(&["native"]) {
        return;
    }
    common::cc(s.path(), "native", "f.o", C, &[]);
    common::ar(s.path(), "libf.a", &["f.o"], false);
    let mut b = std::fs::read(s.join("libf.a")).unwrap();
    // The index's count: larger than the index.
    b[68..72].copy_from_slice(&0xffff_u32.to_be_bytes());
    std::fs::write(s.join("libf.a"), b).unwrap();
    let out = common::run(s.path(), &["-maarch64linux", "libf.a"]);
    assert_eq!(common::ids(&out), ["malformed-input"]);
    assert!(!out.failed());
    assert_eq!(out.files.len(), 1);
}

#[test]
fn s7_2_thin_relative_to_archive() {
    let s = Scratch::new("s7thin");
    if !common::have(&["native"]) {
        return;
    }
    std::fs::create_dir_all(s.join("sub/objs")).unwrap();
    common::cc(&s.join("sub/objs"), "native", "f.o", C, &[]);
    common::ar(&s.join("sub"), "libt.a", &["objs/f.o"], true);
    let out = common::run(s.path(), &["-maarch64linux", "sub/libt.a"]);
    assert!(out.problems.is_empty(), "{:?}", common::report(&out));
    assert_eq!(common::names(&out), ["sub/libt.a(sub/objs/f.o)"]);
    assert_eq!(common::opened(&out), ["sub/libt.a", "sub/objs/f.o"]);
    assert_eq!(
        out.files[0].member.as_ref().unwrap().thin_path.as_deref(),
        Some(&b"sub/objs/f.o"[..])
    );
}

#[test]
fn s7_2_thin_missing_unneeded_member() {
    let s = Scratch::new("s7thinmiss");
    if !common::have(&["native"]) {
        return;
    }
    common::cc(s.path(), "native", "f.o", C, &[]);
    common::cc(s.path(), "native", "g.o", "int g;\n", &[]);
    common::ar(s.path(), "libt.a", &["f.o", "g.o"], true);
    std::fs::remove_file(s.join("g.o")).unwrap();
    let out = common::run(s.path(), &["-maarch64linux", "libt.a"]);
    assert_eq!(
        common::report(&out),
        ["error: libt.a(g.o): cannot open g.o: No such file or directory"]
    );
}

#[test]
fn s7_2_thin_size_changed() {
    let s = Scratch::new("s7thinsize");
    if !common::have(&["native"]) {
        return;
    }
    common::cc(s.path(), "native", "f.o", C, &[]);
    common::ar(s.path(), "libt.a", &["f.o"], true);
    common::cc(
        s.path(),
        "native",
        "f.o",
        "int f(void){return 1;}\nint more(int x){return x*x+3;}\n",
        &[],
    );
    let out = common::run(s.path(), &["-maarch64linux", "libt.a"]);
    assert!(out.problems.is_empty(), "{:?}", common::report(&out));
    assert_eq!(out.files[0].len, std::fs::metadata(s.join("f.o")).unwrap().len());
}

#[test]
fn s7_2_gnu_nested_thin_member() {
    let s = Scratch::new("s7nested");
    if !common::have(&["native"]) {
        return;
    }
    common::cc(s.path(), "native", "a.o", C, &[]);
    common::ar(s.path(), "libinner.a", &["a.o"], false);
    common::ar(s.path(), "libouter.a", &["libinner.a"], true);
    let out = common::run(s.path(), &["-maarch64linux", "libouter.a"]);
    assert!(out.problems.is_empty(), "{:?}", common::report(&out));
    assert_eq!(common::names(&out), ["libouter.a(libinner.a)"]);
    assert_eq!(out.files[0].len, std::fs::metadata(s.join("a.o")).unwrap().len());
}

#[test]
fn s7_2_display_names() {
    let s = Scratch::new("s7names");
    if !common::have(&["native"]) {
        return;
    }
    common::cc(s.path(), "native", "a_long_member_name_over_sixteen.o", C, &[]);
    common::ar(s.path(), "liblong.a", &["a_long_member_name_over_sixteen.o"], false);
    let out = common::run(s.path(), &["-maarch64linux", "liblong.a"]);
    assert_eq!(common::names(&out), ["liblong.a(a_long_member_name_over_sixteen.o)"]);
}

#[test]
fn s7_3_member_table() {
    let s = Scratch::new("s7members");
    if !common::have(&["native"]) {
        return;
    }
    common::cc(s.path(), "native", "ok.o", C, &[]);
    s.write("text.txt", "INPUT(ok.o)\n");
    s.write("empty.o", b"");
    common::ar(s.path(), "libnested.a", &["ok.o"], false);
    common::cc(s.path(), "native", "libs.so", C, &["-shared", "-fPIC"]);
    common::cc(s.path(), "native", "exe", "int main(void){return 0;}\n", &["--link", "-no-pie"]);
    s.write("bc.o", b"BC\xC0\xDE\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0");
    common::cc(
        s.path(),
        "native",
        "slim.o",
        "int s(void){return 3;}\n",
        &["-flto", "-fno-fat-lto-objects"],
    );
    let mut bad = std::fs::read(s.join("ok.o")).unwrap();
    bad[5] = 2; // big-endian
    s.write("be.o", bad);
    common::ar(
        s.path(),
        "libmix.a",
        &["ok.o", "text.txt", "empty.o", "libnested.a", "libs.so", "exe", "bc.o", "slim.o", "be.o"],
        false,
    );
    let lazy = common::run(s.path(), &["-maarch64linux", "libmix.a"]);
    let whole = common::run(s.path(), &["-maarch64linux", "--whole-archive", "libmix.a"]);
    let expect = |sev: &str| {
        vec![
            format!("{sev}: libmix.a(text.txt): archive member is not a relocatable object: not an ELF file"),
            format!("{sev}: libmix.a(empty.o): archive member is not a relocatable object: empty"),
            format!("{sev}: libmix.a(libnested.a): archive member is not a relocatable object: an archive"),
            format!("{sev}: libmix.a(libs.so): archive member is not a relocatable object: a shared object"),
            format!("{sev}: libmix.a(exe): archive member is not a relocatable object: an executable (ET_EXEC)"),
            "error: libmix.a(bc.o): unsupported file format: LLVM bitcode; file not recognized: file format not recognized".to_owned(),
            "error: libmix.a(slim.o): unsupported file format: GCC LTO object\n>>> mortar does not perform link-time optimization; compile without -flto, or with -ffat-lto-objects".to_owned(),
            "error: libmix.a(be.o): unsupported file format: big-endian ELF".to_owned(),
        ]
    };
    assert_eq!(common::report(&lazy), expect("warning"));
    assert_eq!(common::report(&whole), expect("error"));
    assert_eq!(common::names(&lazy), ["libmix.a(ok.o)"]);
}

#[test]
fn s7_4_no_index() {
    let s = Scratch::new("s7noidx");
    let Some(ar) = common::need("ar") else { return };
    let Some(_) = common::cc(s.path(), "native", "f.o", C, &[]) else { return };
    assert!(
        common::tool_run(&ar, &["rcS".into(), "libf.a".into(), "f.o".into()], s.path()).success()
    );
    let out = common::run(s.path(), &["-maarch64linux", "libf.a"]);
    assert!(out.problems.is_empty());
    assert_eq!(out.files[0].globals, 1);
}

#[test]
fn s7_4_stale_index_is_ignored() {
    let s = Scratch::new("s7stale");
    let Some(ar) = common::need("ar") else { return };
    let Some(_) = common::cc(s.path(), "native", "f.o", "int old_sym(void){return 1;}\n", &[])
    else {
        return;
    };
    common::ar(s.path(), "libf.a", &["f.o"], false);
    // Replace the member without updating the index.
    common::cc(s.path(), "native", "f.o", "int new_sym(void){return 2;}\nint new_two;\n", &[]);
    assert!(
        common::tool_run(&ar, &["rS".into(), "libf.a".into(), "f.o".into()], s.path()).success()
    );
    let out = common::run(s.path(), &["-maarch64linux", "libf.a"]);
    assert!(out.problems.is_empty(), "{:?}", common::report(&out));
    // The member's own symbols, not the index's.
    assert_eq!(out.files[0].globals, 2);
}

#[test]
fn s7_5_whole_archive_state() {
    let s = Scratch::new("s7whole");
    if !common::have(&["native"]) {
        return;
    }
    common::cc(s.path(), "native", "f.o", C, &[]);
    common::ar(s.path(), "libf.a", &["f.o"], false);
    let out = common::run(
        s.path(),
        &["-maarch64linux", "--whole-archive", "libf.a", "--no-whole-archive", "libf.a"],
    );
    let w: Vec<_> = out.files.iter().map(|f| f.state.whole_archive).collect();
    assert_eq!(w, [true, false]);
}

#[test]
fn s7_6_start_lib() {
    let s = Scratch::new("s7startlib");
    if !common::have(&["native"]) {
        return;
    }
    common::cc(s.path(), "native", "k.o", C, &[]);
    common::cc(s.path(), "native", "f.o", "int g;\n", &[]);
    common::ar(s.path(), "libf.a", &["f.o"], false);
    common::cc(s.path(), "native", "libs.so", C, &["-shared", "-fPIC", "-Wl,-soname,libs.so"]);
    let out = common::run(
        s.path(),
        &[
            "-shared",
            "-maarch64linux",
            "--whole-archive",
            "--start-lib",
            "k.o",
            "libf.a",
            "libs.so",
            "--end-lib",
        ],
    );
    assert!(out.problems.is_empty(), "{:?}", common::report(&out));
    let st: Vec<_> = out
        .files
        .iter()
        .map(|f| (f.state.lazy, f.member.is_some(), f.state.whole_archive))
        .collect();
    assert_eq!(st, [(true, false, true), (false, true, true), (false, false, true)]);
}

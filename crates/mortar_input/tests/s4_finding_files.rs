//! `spec/input.md` §4: finding files.
#![allow(clippy::many_single_char_names, reason = "short names for fixtures and tallies")]

mod common;

use std::path::Path;

use mortar_input::walk::{ProbeOutcome, SearchRef};
use mortar_testutil::Scratch;

const C: &str = "int f(void) { return 1; }\n";

fn lib(dir: &Path, target: &str, name: &str) {
    std::fs::create_dir_all(dir).unwrap();
    let o = format!("{name}.o");
    common::cc(dir, target, &o, C, &[]);
    common::ar(dir, &format!("lib{name}.a"), &[&o], false);
}

fn so(dir: &Path, target: &str, name: &str) {
    std::fs::create_dir_all(dir).unwrap();
    common::cc(
        dir,
        target,
        &format!("lib{name}.so"),
        C,
        &["-shared", "-fPIC", &format!("-Wl,-soname,lib{name}.so")],
    );
}

fn probes(out: &mortar_input::walk::Loaded<'_>) -> Vec<(String, ProbeOutcome)> {
    out.link
        .probes
        .iter()
        .map(|p| (String::from_utf8_lossy(&p.path).into_owned(), p.outcome.clone()))
        .collect()
}

#[test]
fn s4_1_paths_not_searched() {
    let s = Scratch::new("s4path");
    if !common::have(&["native"]) {
        return;
    }
    lib(&s.join("dd"), "native", "q");
    let out = common::run(s.path(), &["-maarch64linux", "-Ldd", "libq.a"]);
    assert_eq!(
        common::report(&out),
        ["error: libq.a: cannot open libq.a: No such file or directory"]
    );
    assert!(out.link.probes.is_empty());
}

#[test]
fn s4_1_dash_is_a_file() {
    let s = Scratch::new("s4dash");
    let Some(_) = common::cc(s.path(), "native", "-", C, &[]) else { return };
    let out = common::run(s.path(), &["-maarch64linux", "-"]);
    assert!(out.problems.is_empty(), "{:?}", common::report(&out));
    assert_eq!(common::names(&out), ["-"]);
}

#[test]
fn s4_1_equals_path_refused() {
    let s = Scratch::new("s4eq");
    let out = common::run(s.path(), &["-maarch64linux", "--sysroot=/", "=/lib/x.o"]);
    assert_eq!(common::ids(&out), ["refused"]);
    assert!(common::report(&out)[0].starts_with(
        "error: argument '=/lib/x.o': unsupported feature: '=' prefix on an input path\n>>> "
    ));
}

#[test]
fn s4_2_l_after_l_dir() {
    let s = Scratch::new("s4lafter");
    if !common::have(&["native"]) {
        return;
    }
    lib(&s.join("d"), "native", "f");
    let out = common::run(s.path(), &["-maarch64linux", "-lf", "-Ld"]);
    assert_eq!(common::names(&out), ["d/libf.a(f.o)"]);
}

#[test]
fn s4_2_search_dir_applies_later_only() {
    let s = Scratch::new("s4sd");
    if !common::have(&["native"]) {
        return;
    }
    lib(&s.join("dd"), "native", "q");
    s.write("s.ld", "SEARCH_DIR(dd)\n");
    let out = common::run(s.path(), &["-maarch64linux", "-lq", "s.ld", "-lq"]);
    assert_eq!(common::ids(&out), ["library-not-found"]);
    assert_eq!(out.problems[0].unit, 1);
    assert_eq!(common::names(&out), ["dd/libq.a(q.o)"]);
    // Within one script, every SEARCH_DIR applies to every entry.
    s.write("own.ld", "INPUT(-lq)\nSEARCH_DIR(dd)\n");
    let out = common::run(s.path(), &["-maarch64linux", "own.ld"]);
    assert!(out.problems.is_empty(), "{:?}", common::report(&out));
    assert_eq!(common::names(&out), ["dd/libq.a(q.o)"]);
}

#[test]
fn s4_2_no_builtin_dirs() {
    let s = Scratch::new("s4nobuiltin");
    let out = common::run(s.path(), &["-maarch64linux", "-lc"]);
    assert_eq!(
        common::report(&out),
        [
            "error: argument '-lc': cannot find -lc: No such file or directory\n>>> the library search path is empty"
        ]
    );
}

#[test]
fn s4_2_nostdlib_drops_search_dir() {
    let s = Scratch::new("s4nostd");
    if !common::have(&["native"]) {
        return;
    }
    lib(&s.join("dd"), "native", "q");
    common::cc(s.path(), "native", "u.o", "int u;\n", &[]);
    s.write("s.ld", "SEARCH_DIR(dd)\n");
    s.write("own.ld", "SEARCH_DIR(dd)\nINPUT(-lq)\n");
    let out = common::run(s.path(), &["u.o", "s.ld", "-lq"]);
    assert!(out.problems.is_empty());
    for args in [&["-nostdlib", "u.o", "s.ld", "-lq"][..], &["-nostdlib", "u.o", "own.ld"]] {
        let out = common::run(s.path(), args);
        assert_eq!(common::ids(&out), ["library-not-found"], "{args:?}");
    }
}

#[test]
fn s4_3_sysroot_table() {
    let s = Scratch::new("s4sysroot");
    if !common::have(&["native"]) {
        return;
    }
    lib(&s.join("d"), "native", "f");
    lib(&s.join("S/lib"), "native", "g");
    // No sysroot: `=d` is `d`.
    let out = common::run(s.path(), &["-maarch64linux", "-L=d", "-lf"]);
    assert_eq!(common::names(&out), ["d/libf.a(f.o)"]);
    // A sysroot and an absolute directory: joined.
    let out = common::run(s.path(), &["-maarch64linux", "--sysroot=S", "-L=/lib", "-lg"]);
    assert_eq!(common::names(&out), ["S/lib/libg.a(g.o)"]);
    let out = common::run(s.path(), &["-maarch64linux", "--sysroot=S", "-L$SYSROOT/lib", "-lg"]);
    assert_eq!(common::names(&out), ["S/lib/libg.a(g.o)"]);
    // A sysroot and a relative directory: ambiguous.
    let out = common::run(s.path(), &["-maarch64linux", "--sysroot=S", "-L=lib", "-lg"]);
    assert_eq!(common::ids(&out), ["ambiguous-sysroot-path", "library-not-found"]);
    assert_eq!(
        common::report(&out)[0],
        "error: argument '-L=lib': ambiguous sysroot path '=lib'\n>>> GNU ld and mold read it as Slib\n>>> lld reads it as S/lib\n>>> write the directory with a leading '/' after '='"
    );
    // An absolute directory that is not marked is not prefixed.
    let out = common::run(s.path(), &["-maarch64linux", "--sysroot=S", "-L/lib", "-lg"]);
    assert_eq!(common::ids(&out), ["library-not-found"]);
}

#[test]
fn s4_3_sysroot_spelling() {
    let s = Scratch::new("s4spell");
    if !common::have(&["native"]) {
        return;
    }
    lib(&s.join("S/lib"), "native", "g");
    let out = common::run(s.path(), &["-maarch64linux", "--sysroot=S/", "-L=/lib", "-lg"]);
    assert_eq!(common::names(&out), ["S/lib/libg.a(g.o)"]);
    let out = common::run(s.path(), &["-maarch64linux", "--sysroot=/", "-L=/usr/lib", "-lnone"]);
    assert_eq!(probes(&out)[0].0, "/usr/lib/libnone.so");
}

#[test]
fn s4_3_empty_l_dir() {
    let s = Scratch::new("s4emptyL");
    let out = common::run(s.path(), &["-maarch64linux", "-L", "", "/dev/null"]);
    assert_eq!(common::report(&out), ["error: argument '-L': -L: invalid value ''"]);
}

#[test]
fn s4_4_shared_before_static_per_directory() {
    let s = Scratch::new("s4order");
    if !common::have(&["native"]) {
        return;
    }
    lib(&s.join("da"), "native", "f");
    so(&s.join("dso"), "native", "f");
    lib(&s.join("both"), "native", "f");
    so(&s.join("both"), "native", "f");
    let run = |args: &[&str]| {
        let mut a = vec!["-shared", "-maarch64linux"];
        a.extend_from_slice(args);
        common::names(&common::run(s.path(), &a))
    };
    assert_eq!(run(&["-Lda", "-Ldso", "-lf"]), ["da/libf.a(f.o)"]);
    assert_eq!(run(&["-Ldso", "-Lda", "-lf"]), ["dso/libf.so"]);
    assert_eq!(run(&["-Lboth", "-lf"]), ["both/libf.so"]);
}

#[test]
fn s4_4_bstatic() {
    let s = Scratch::new("s4bstatic");
    if !common::have(&["native"]) {
        return;
    }
    lib(&s.join("both"), "native", "f");
    so(&s.join("both"), "native", "f");
    let out = common::run(s.path(), &["-shared", "-maarch64linux", "-Lboth", "-Bstatic", "-lf"]);
    assert_eq!(common::names(&out), ["both/libf.a(f.o)"]);
    assert_eq!(probes(&out).len(), 1);
}

#[test]
fn s4_4_l_colon() {
    let s = Scratch::new("s4colon");
    if !common::have(&["native"]) {
        return;
    }
    so(&s.join("d"), "native", "f");
    let out =
        common::run(s.path(), &["-shared", "-maarch64linux", "-Ld", "-Bstatic", "-l:libf.so"]);
    // `-l:` ignores -Bstatic for its candidate; the shared object is then
    // a static link of a shared object (§8.1).
    assert_eq!(common::ids(&out), ["static-link-of-shared-object"]);
    let out = common::run(s.path(), &["-shared", "-maarch64linux", "-Ld", "-l:libf.so"]);
    assert_eq!(common::names(&out), ["d/libf.so"]);
}

#[test]
fn s4_4_candidate_stat_table() {
    let s = Scratch::new("s4stat");
    if !common::have(&["native"]) {
        return;
    }
    lib(&s.join("good"), "native", "f");
    std::fs::create_dir_all(s.join("dangling")).unwrap();
    std::os::unix::fs::symlink("nowhere/libf.so", s.join("dangling/libf.so")).unwrap();
    std::fs::create_dir_all(s.join("dir/libf.so")).unwrap();
    std::fs::create_dir_all(s.join("loop")).unwrap();
    std::os::unix::fs::symlink("libf.so", s.join("loop/libf.so")).unwrap();
    let out = common::run(s.path(), &["-maarch64linux", "-Ldangling", "-Lgood", "-lf"]);
    assert_eq!(common::names(&out), ["good/libf.a(f.o)"]);
    assert_eq!(probes(&out)[0], ("dangling/libf.so".to_owned(), ProbeOutcome::Absent));
    let out = common::run(s.path(), &["-maarch64linux", "-Ldir", "-Lgood", "-lf"]);
    assert_eq!(
        common::report(&out),
        ["error: argument '-lf': cannot open dir/libf.so: Is a directory"]
    );
    assert!(out.files.is_empty());
    let out = common::run(s.path(), &["-maarch64linux", "-Lloop", "-Lgood", "-lf"]);
    assert_eq!(common::ids(&out), ["cannot-open-file"]);
    assert!(common::report(&out)[0].contains("loop/libf.so"), "{:?}", common::report(&out));
    assert!(matches!(probes(&out)[0].1, ProbeOutcome::Error(Some(40))));
}

#[test]
fn s4_4_skip_table() {
    let s = Scratch::new("s4skip");
    if !common::have(&["native", "x86"]) {
        return;
    }
    let x = s.join("x");
    let n = s.join("n");
    // Candidates of another target: a shared object, an archive, an
    // object, an executable, a script with another OUTPUT_FORMAT.
    so(&x, "x86", "so");
    lib(&x, "x86", "ar");
    common::cc(&x, "x86", "obj.o", C, &[]);
    common::cc(&x, "x86", "exe", "int main(void){return 0;}\n", &[]);
    std::fs::write(x.join("libsc.so"), "OUTPUT_FORMAT(elf64-x86-64)\nGROUP(none.o)\n").unwrap();
    so(&n, "native", "so");
    lib(&n, "native", "ar");
    common::cc(&n, "native", "obj.o", C, &[]);
    common::cc(&n, "native", "exe", "int main(void){return 0;}\n", &[]);
    common::cc(&n, "native", "libsc.so", C, &["-shared", "-fPIC"]);
    let base = ["-shared", "-maarch64linux", "-Lx", "-Ln"];
    for (arg, label, skipped) in [
        ("-lso", "-lso", "x/libso.so"),
        ("-lar", "-lar", "x/libar.a"),
        ("-l:obj.o", "-l:obj.o", "x/obj.o"),
        ("-l:exe", "-l:exe", "x/exe"),
        ("-lsc", "-lsc", "x/libsc.so"),
    ] {
        let mut a = base.to_vec();
        a.push(arg);
        let out = common::run(s.path(), &a);
        let r = common::report(&out);
        assert_eq!(
            r.first().map(String::as_str),
            Some(format!("warning: argument '{arg}': skipping incompatible {skipped} when searching for {label}").as_str()),
            "{arg}: {r:?}"
        );
        assert!(out.link.probes.iter().any(|p| matches!(p.outcome, ProbeOutcome::Skipped(_))));
    }
    // A script's relative entry: through the script's directory, the
    // working directory and the search path; the text names the entry.
    std::fs::create_dir_all(s.join("sd")).unwrap();
    common::cc(&s.join("sd"), "x86", "e.o", C, &[]);
    common::cc(s.path(), "x86", "e.o", C, &[]);
    common::cc(&x, "x86", "e.o", C, &[]);
    common::cc(&n, "native", "e.o", C, &[]);
    s.write("sd/s.ld", "INPUT(e.o)\n");
    let out = common::run(s.path(), &["-maarch64linux", "-Lx", "-Ln", "sd/s.ld"]);
    let r = common::report(&out);
    assert_eq!(common::names(&out), ["n/e.o"], "{r:?}");
    let skipped: Vec<_> = probes(&out)
        .into_iter()
        .filter(|p| matches!(p.1, ProbeOutcome::Skipped(_)))
        .map(|p| p.0)
        .collect();
    assert_eq!(skipped, ["sd/e.o", "e.o", "x/e.o"]);
    assert!(r.iter().all(|l| l.ends_with(">>> named by argument 'sd/s.ld'")));
    assert!(r.contains(&"warning: sd/s.ld:1:7: skipping incompatible sd/e.o when searching for e.o\n>>> named by argument 'sd/s.ld'".to_owned()), "{r:?}");
}

#[test]
fn s4_4_not_found_notes() {
    let s = Scratch::new("s4notes");
    if !common::have(&["native", "x86"]) {
        return;
    }
    lib(&s.join("xd"), "x86", "f");
    so(&s.join("sd"), "native", "f");
    let out = common::run(s.path(), &["-maarch64linux", "-Lxd", "-Lsd", "-Bstatic", "-lf"]);
    assert_eq!(
        common::report(&out),
        [
            "error: argument '-lf': cannot find -lf: No such file or directory\n>>> searched xd\n>>> searched sd\n>>> skipped xd/libf.a: elf64-x86-64\n>>> sd/libf.so exists but -Bstatic is in effect",
            "warning: argument '-lf': skipping incompatible xd/libf.a when searching for -lf",
        ]
    );
    let out = common::run(s.path(), &["-maarch64linux", "-Lxd", "-l:none.a"]);
    assert!(common::report(&out)[0].starts_with(
        "error: argument '-l:none.a': cannot find -l:none.a: No such file or directory"
    ));
}

#[test]
fn s4_5_found_path_is_not_normalized() {
    let s = Scratch::new("s4spelling");
    if !common::have(&["native"]) {
        return;
    }
    lib(&s.join("d"), "native", "f");
    let out = common::run(s.path(), &["-maarch64linux", "-L./d/../d", "-lf"]);
    assert_eq!(common::names(&out), ["./d/../d/libf.a(f.o)"]);
    let out = common::run(s.path(), &["-maarch64linux", "-Ld/", "-lf"]);
    assert_eq!(common::names(&out), ["d/libf.a(f.o)"]);
    assert_eq!(common::opened(&out), ["d/libf.a"]);
}

#[test]
fn s4_6_probes_recorded() {
    let s = Scratch::new("s4probes");
    if !common::have(&["native"]) {
        return;
    }
    lib(&s.join("b"), "native", "f");
    std::fs::create_dir_all(s.join("a")).unwrap();
    let out = common::run(s.path(), &["-maarch64linux", "-La", "-Lb", "-Lc", "-lf"]);
    assert_eq!(
        probes(&out),
        [
            ("a/libf.so".to_owned(), ProbeOutcome::Absent),
            ("a/libf.a".to_owned(), ProbeOutcome::Absent),
            ("b/libf.so".to_owned(), ProbeOutcome::Absent),
            ("b/libf.a".to_owned(), ProbeOutcome::Found(0)),
        ]
    );
    assert!(out.link.probes.iter().all(|p| p.search == SearchRef::Input(0)));
}

#[test]
fn s4_6_new_candidate_changes_a_probe() {
    let s = Scratch::new("s4newcand");
    if !common::have(&["native"]) {
        return;
    }
    lib(&s.join("b"), "native", "f");
    std::fs::create_dir_all(s.join("a")).unwrap();
    let args = ["-shared", "-maarch64linux", "-La", "-Lb", "-lf"];
    let before = common::run(s.path(), &args);
    // A library in an earlier directory.
    so(&s.join("a"), "native", "f");
    let after = common::run(s.path(), &args);
    assert_eq!(probes(&before)[0].1, ProbeOutcome::Absent);
    assert_eq!(probes(&after)[0].1, ProbeOutcome::Found(0));
    assert_eq!(common::names(&after), ["a/libf.so"]);
    // A file in the working directory shadowing a script entry.
    std::fs::create_dir_all(s.join("sd")).unwrap();
    common::cc(&s.join("b"), "native", "e.o", C, &[]);
    s.write("sd/s.ld", "INPUT(e.o)\n");
    let before = common::run(s.path(), &["-maarch64linux", "-Lb", "sd/s.ld"]);
    common::cc(s.path(), "native", "e.o", C, &[]);
    let after = common::run(s.path(), &["-maarch64linux", "-Lb", "sd/s.ld"]);
    assert_eq!(common::names(&before), ["b/e.o"]);
    assert_eq!(common::names(&after), ["e.o"]);
    assert_eq!(probes(&before)[1], ("e.o".to_owned(), ProbeOutcome::Absent));
    assert_eq!(probes(&after)[1], ("e.o".to_owned(), ProbeOutcome::Found(1)));
}

#[test]
fn s4_7_environment_is_ignored() {
    let s = Scratch::new("s4env");
    if !common::have(&["native"]) {
        return;
    }
    lib(&s.join("envdir"), "native", "f");
    // The walk in a child process with the variables set: `-lf` is still
    // not found.
    let exe = std::env::current_exe().unwrap();
    let mut c = std::process::Command::new(exe);
    c.args(["--exact", "s4_7_child", "--ignored", "--nocapture", "--test-threads=1"]);
    c.env("MORTAR_S4_7_DIR", s.path());
    for v in ["LIBRARY_PATH", "LD_LIBRARY_PATH", "LD_RUN_PATH"] {
        c.env(v, s.join("envdir"));
    }
    let o = mortar_testutil::run_with(&mut c, b"", std::time::Duration::from_mins(1));
    assert!(o.success(), "{}{}", o.stdout_text(), o.stderr_text());
    assert!(o.stdout_text().contains("s4_7: library-not-found"), "{}", o.stdout_text());
}

#[test]
#[ignore = "run by s4_7_environment_is_ignored in a child process"]
fn s4_7_child() {
    let Some(dir) = std::env::var_os("MORTAR_S4_7_DIR") else { return };
    let out = common::run(Path::new(&dir), &["-maarch64linux", "-lf"]);
    println!("s4_7: {}", common::ids(&out).join(","));
}

#[test]
fn s4_7_rpath_link_is_not_an_l_directory() {
    let s = Scratch::new("s4rpl");
    if !common::have(&["native"]) {
        return;
    }
    lib(&s.join("rl"), "native", "f");
    let out = common::run(s.path(), &["-maarch64linux", "-rpath-link", "rl", "-lf"]);
    assert_eq!(common::ids(&out), ["library-not-found"]);
}

#[test]
fn s4_8_r_directory_is_rpath() {
    let s = Scratch::new("s4R");
    std::fs::create_dir_all(s.join("rdir")).unwrap();
    let out = common::run(s.path(), &["-maarch64linux", "-R", "rdir", "/dev/null"]);
    assert!(out.problems.is_empty(), "{:?}", common::report(&out));
    assert_eq!(out.link.rpath_from_r, [(1, (*b"rdir").into())]);
}

#[test]
fn s4_8_r_file_refused() {
    let s = Scratch::new("s4Rfile");
    s.write("syms", b"");
    let out = common::run(s.path(), &["-maarch64linux", "-R", "syms"]);
    assert_eq!(
        common::report(&out),
        ["error: argument '-R syms': unsupported feature: --just-symbols"]
    );
}

//! `spec/input.md` §5: linker scripts as inputs.
mod common;

use mortar_input::walk::OpenedKind;
use mortar_testutil::Scratch;

const C: &str = "int f(void) { return 1; }\n";

fn objs(s: &Scratch, names: &[&str]) -> bool {
    if !common::have(&["native"]) {
        return false;
    }
    for n in names {
        common::cc(
            s.path(),
            "native",
            n,
            &format!("int {}_v;\n", n.replace(['.', '/', '-'], "_")),
            &[],
        );
    }
    true
}

#[test]
fn s5_1_commands() {
    let s = Scratch::new("s5cmd");
    if !objs(&s, &["a.o", "b.o", "c.o", "d.o"]) {
        return;
    }
    s.write("inc.ld", "INPUT(d.o)\n");
    s.write(
        "s.ld",
        "INPUT(a.o)\nGROUP(b.o AS_NEEDED(c.o))\nSEARCH_DIR(.)\nINCLUDE inc.ld\nOUTPUT(o.out)\nENTRY(go)\nEXTERN(u1 u2)\nOUTPUT_FORMAT(elf64-littleaarch64)\nOUTPUT_ARCH(aarch64)\nTARGET(elf64-littleaarch64)\n",
    );
    let out = common::run(s.path(), &["-maarch64linux", "s.ld"]);
    assert!(out.problems.is_empty(), "{:?}", common::report(&out));
    assert_eq!(common::names(&out), ["a.o", "b.o", "c.o", "d.o"]);
    assert!(out.files[2].state.as_needed);
    assert!(!out.files[1].state.as_needed);
    assert_eq!(out.link.group, [None, Some(1), Some(1), None]);
    let d: Vec<_> = out
        .link
        .directives
        .iter()
        .map(|d| (d.command, String::from_utf8_lossy(&d.value).into_owned()))
        .collect();
    assert_eq!(
        d,
        [
            ("OUTPUT", "o.out".into()),
            ("ENTRY", "go".into()),
            ("EXTERN", "u1".into()),
            ("EXTERN", "u2".into())
        ]
    );
    let kinds: Vec<_> = out.link.opened.iter().map(|o| o.kind).collect();
    assert_eq!(
        kinds,
        [
            OpenedKind::Script,
            OpenedKind::Include,
            OpenedKind::Object,
            OpenedKind::Object,
            OpenedKind::Object,
            OpenedKind::Object
        ]
    );
    // STARTUP is refused.
    s.write("st.ld", "STARTUP(a.o)\n");
    let out = common::run(s.path(), &["-maarch64linux", "st.ld"]);
    assert_eq!(
        common::report(&out),
        ["error: st.ld:1:1: unsupported linker script command: STARTUP"]
    );
    // A script that does not parse contributes nothing.
    s.write("bad.ld", "INPUT(a.o\n");
    let out = common::run(s.path(), &["-maarch64linux", "bad.ld", "b.o"]);
    assert_eq!(common::ids(&out).len(), 1);
    assert!(common::report(&out)[0].contains("bad.ld"), "{:?}", common::report(&out));
    assert_eq!(common::names(&out), ["b.o"]);
    // A missing INCLUDE.
    s.write("mi.ld", "INCLUDE nowhere.ld\nINPUT(a.o)\n");
    let out = common::run(s.path(), &["-maarch64linux", "mi.ld"]);
    assert_eq!(common::ids(&out), ["cannot-open-file"]);
    assert!(out.files.is_empty());
}

/// The system scripts: `libc.so`, `libm.so`, `libgcc_s.so` through the
/// drivers of the three targets.
#[test]
fn s5_1_system_scripts() {
    let s = Scratch::new("s5sys");
    for target in ["native", "x86", "i686"] {
        let Some(gcc) = common::compiler(target) else { continue };
        let libc6 = common::libc_so_6(target).unwrap();
        let o = format!("m-{target}.o");
        common::cc(s.path(), target, &o, "int main(void){return 0;}\n", &[]);
        let args =
            common::driver_link_args(&gcc, &[&o, "-lm", "-shared-libgcc"], s.path()).unwrap();
        let out = common::run(s.path(), &args);
        assert!(out.problems.is_empty(), "{target}: {:?}", common::report(&out));
        let names = common::names(&out);
        assert!(names.contains(&libc6), "{target}: {libc6}: {names:?}");
        assert!(names.iter().any(|n| n.contains("/libm.so")), "{target}: {names:?}");
        assert!(names.iter().any(|n| n.ends_with("libgcc_s.so.1")), "{target}: {names:?}");
        assert!(names.iter().any(|n| n.contains("libc_nonshared.a(")), "{target}: {names:?}");
    }
    // The i686 line with an x86-64 -L first: libgcc_s.so.1 found past it.
    let Some(gcc) = common::compiler("i686") else { return };
    let Some(x86) = common::libc_dir("x86") else { return };
    let l = format!("-L{}", x86.display());
    let args = common::driver_link_args(&gcc, &["m-i686.o", &l, "-lm", "-shared-libgcc"], s.path())
        .unwrap();
    let out = common::run(s.path(), &args);
    assert!(
        out.problems.iter().all(|p| p.id() == "skipped-incompatible"),
        "{:?}",
        common::report(&out)
    );
    let r = common::report(&out);
    let skip = format!("skipping incompatible {}/libm.so when searching for -lm", x86.display());
    assert!(r.iter().any(|l| l.contains(&skip)), "{r:?}");
    assert!(
        common::names(&out).iter().any(|n| n.ends_with("libgcc_s.so.1") && !n.contains("x86_64"))
    );
}

#[test]
fn s5_2_state_table() {
    let s = Scratch::new("s5state");
    if !objs(&s, &["a.o", "b.o"]) {
        return;
    }
    common::cc(s.path(), "native", "m.o", C, &[]);
    common::ar(s.path(), "libw.a", &["m.o"], false);
    common::cc(s.path(), "native", "libd.so", C, &["-shared", "-fPIC", "-Wl,-soname,libd.so"]);
    s.write("st.ld", "INPUT(a.o AS_NEEDED(libd.so) b.o libw.a)\n");
    let out = common::run(s.path(), &["-shared", "-maarch64linux", "--whole-archive", "st.ld"]);
    assert!(out.problems.is_empty(), "{:?}", common::report(&out));
    let st: Vec<_> = out.files.iter().map(|f| (f.state.as_needed, f.state.whole_archive)).collect();
    assert_eq!(st, [(false, true), (true, true), (false, true), (false, true)]);
    // `-l` entries search with the including -Bstatic.
    std::fs::create_dir_all(s.join("both")).unwrap();
    common::cc(&s.join("both"), "native", "libq.so", C, &["-shared", "-fPIC"]);
    common::cc(&s.join("both"), "native", "q.o", C, &[]);
    common::ar(&s.join("both"), "libq.a", &["q.o"], false);
    s.write("lq.ld", "INPUT(-lq)\n");
    let out = common::run(s.path(), &["-shared", "-maarch64linux", "-Lboth", "-Bstatic", "lq.ld"]);
    assert_eq!(common::names(&out), ["both/libq.a(q.o)"]);
    // A script in a lib region; GROUP inside a command-line group.
    s.write("g.ld", "GROUP(a.o b.o)\n");
    let out = common::run(
        s.path(),
        &[
            "-maarch64linux",
            "--start-lib",
            "g.ld",
            "--end-lib",
            "--start-group",
            "g.ld",
            "--end-group",
            "g.ld",
        ],
    );
    let lazy: Vec<_> = out.files.iter().map(|f| f.state.lazy).collect();
    assert_eq!(lazy, [true, true, false, false, false, false]);
    assert_eq!(out.link.group, [Some(1), Some(1), Some(2), Some(2), Some(3), Some(3)]);
}

#[test]
fn s5_3_nesting() {
    let s = Scratch::new("s5nest");
    if !objs(&s, &["a.o"]) {
        return;
    }
    std::fs::create_dir_all(s.join("in")).unwrap();
    s.write("in/inner.ld", "INPUT(a.o)\n");
    s.write("outer.ld", "INPUT(in/inner.ld)\n");
    let out = common::run(s.path(), &["-maarch64linux", "outer.ld", "outer.ld"]);
    assert!(out.problems.is_empty(), "{:?}", common::report(&out));
    // Named twice, expanded twice; the inner entry is found in the cwd.
    assert_eq!(common::names(&out), ["a.o", "a.o"]);
    assert_eq!(out.link.script_entries.len(), 4);
    assert_eq!(out.link.script_entries[1].parent, Some(0));
}

#[test]
fn s5_3_cycle_two() {
    let s = Scratch::new("s5cycle2");
    s.write("c1.ld", "INPUT(c2.ld)\n");
    s.write("c2.ld", "INPUT(c1.ld)\n");
    let out = common::run(s.path(), &["-maarch64linux", "c1.ld"]);
    assert_eq!(
        common::report(&out),
        [
            "error: c2.ld:1:7: linker script names itself\n>>> named by c1.ld:1:7\n>>> named by argument 'c1.ld'"
        ]
    );
}

#[test]
fn s5_3_cycle_self() {
    let s = Scratch::new("s5cycle1");
    s.write("me.ld", "INPUT(me.ld)\n");
    let out = common::run(s.path(), &["-maarch64linux", "me.ld"]);
    assert_eq!(
        common::report(&out),
        ["error: me.ld:1:7: linker script names itself\n>>> named by argument 'me.ld'"]
    );
    // Through a symbolic link: the same file.
    std::os::unix::fs::symlink("me.ld", s.join("alias.ld")).unwrap();
    s.write("viaalias.ld", "INPUT(alias.ld)\n");
    std::fs::write(s.join("me.ld"), "INPUT(viaalias.ld)\n").unwrap();
    let out = common::run(s.path(), &["-maarch64linux", "alias.ld"]);
    assert_eq!(common::ids(&out), ["script-cycle"]);
}

#[test]
fn s5_3_depth() {
    let s = Scratch::new("s5depth");
    for i in 0..20 {
        s.write(&format!("d{i}.ld"), format!("INPUT(d{}.ld)\n", i + 1));
    }
    s.write("d20.ld", "");
    let out = common::run(s.path(), &["-maarch64linux", "d0.ld"]);
    assert_eq!(common::ids(&out), ["refused"]);
    assert!(common::report(&out)[0].starts_with(
        "error: d15.ld:1:7: unsupported feature: linker scripts nested more than 16 deep"
    ));
    // 16 deep is fine.
    let out = common::run(s.path(), &["-maarch64linux", "d4.ld"]);
    assert!(out.problems.is_empty(), "{:?}", common::report(&out));
}

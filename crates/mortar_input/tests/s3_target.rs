//! `spec/input.md` §3: the target.
mod common;

use mortar_core::par::{Pool, check_schedules};
use mortar_elf::Machine;
use mortar_input::walk::Options;
use mortar_testutil::Scratch;

const C: &str = "int f(void) { return 1; }\n";
const G: &str = "int g(void) { return 2; }\n";

fn machine(out: &mortar_input::walk::Loaded<'_>) -> Option<(Machine, u8)> {
    out.target.as_ref().map(|t| (t.target.machine, t.rule))
}

#[test]
fn s3_1_m_decides() {
    let s = Scratch::new("s3m");
    if !common::have(&["native", "x86"]) {
        return;
    }
    common::cc(s.path(), "native", "a.o", C, &[]);
    common::cc(s.path(), "x86", "x.o", C, &[]);
    let out = common::run(s.path(), &["-melf_x86_64", "a.o", "x.o"]);
    assert_eq!(machine(&out), Some((Machine::X86_64, 1)));
    assert_eq!(
        common::report(&out),
        [
            "error: a.o: incompatible file: elf64-x86-64 is expected but got elf64-littleaarch64\n>>> the target was set by -m elf_x86_64"
        ]
    );
}

#[test]
fn s3_1_first_path_input() {
    let s = Scratch::new("s3first");
    if !common::have(&["native", "x86", "i686"]) {
        return;
    }
    common::cc(s.path(), "native", "a.o", C, &[]);
    common::cc(s.path(), "x86", "libx.so", C, &["-shared", "-fPIC"]);
    let out = common::run(s.path(), &["-shared", "libx.so", "a.o"]);
    assert_eq!(machine(&out), Some((Machine::X86_64, 2)));
    assert_eq!(common::ids(&out), ["incompatible-file"]);
    // An archive first: its first relocatable member, lazy.
    s.write("note.txt", "not an object\n");
    common::cc(s.path(), "i686", "i.o", C, &[]);
    common::ar(s.path(), "libi.a", &["note.txt", "i.o"], false);
    let out = common::run(s.path(), &["libi.a", "a.o"]);
    assert_eq!(machine(&out), Some((Machine::I386, 2)));
    assert_eq!(common::ids(&out), ["ignored-member", "incompatible-file"]);
}

#[test]
fn s3_1_scripts_and_l_do_not_count_in_rule_2() {
    let s = Scratch::new("s3rule2");
    if !common::have(&["native", "x86"]) {
        return;
    }
    std::fs::create_dir(s.join("xd")).unwrap();
    std::fs::create_dir(s.join("ad")).unwrap();
    common::cc(&s.join("xd"), "x86", "f.o", C, &[]);
    common::ar(&s.join("xd"), "libf.a", &["f.o"], false);
    common::cc(&s.join("ad"), "native", "f.o", C, &[]);
    common::ar(&s.join("ad"), "libf.a", &["f.o"], false);
    common::cc(s.path(), "native", "a.o", G, &[]);
    common::cc(s.path(), "x86", "x.o", G, &[]);
    s.write("s.ld", "INPUT(x.o)\n");
    // `-lf` and the script come first; `a.o` decides.
    let out = common::run(s.path(), &["-Lxd", "-Lad", "-lf", "s.ld", "a.o"]);
    assert_eq!(machine(&out), Some((Machine::AArch64, 2)));
    assert_eq!(out.target.as_ref().unwrap().by, "a.o");
    // The script's relative entry is a search: the x86-64 `x.o` is
    // skipped (once: the script's directory is the working directory).
    assert_eq!(
        common::report(&out),
        [
            "warning: argument '-lf': skipping incompatible xd/libf.a when searching for -lf",
            "error: s.ld:1:7: cannot open x.o: No such file or directory\n>>> searched the working directory\n>>> searched xd\n>>> searched ad\n>>> skipped x.o: elf64-x86-64\n>>> named by argument 's.ld'",
            "warning: s.ld:1:7: skipping incompatible x.o when searching for x.o\n>>> named by argument 's.ld'",
        ]
    );
    assert_eq!(common::names(&out), ["ad/libf.a(f.o)", "a.o"]);
}

/// Searches before the first ELF file in load order take their first
/// candidate; after it they skip.
fn rule_3_fixture(s: &Scratch) {
    for d in ["d1", "d2"] {
        std::fs::create_dir(s.join(d)).unwrap();
    }
    common::cc(&s.join("d2"), "x86", "f.o", C, &[]);
    common::ar(&s.join("d2"), "libf.a", &["f.o"], false);
    common::cc(&s.join("d1"), "native", "g.o", G, &[]);
    common::ar(&s.join("d1"), "libg.a", &["g.o"], false);
    common::cc(&s.join("d2"), "x86", "g.o", G, &[]);
    common::ar(&s.join("d2"), "libg.a", &["g.o"], false);
}

#[test]
fn s3_1_rule_3_fallback() {
    let s = Scratch::new("s3r3");
    if !common::have(&["native", "x86"]) {
        return;
    }
    rule_3_fixture(&s);
    let out = common::run(s.path(), &["-Ld1", "-Ld2", "-lf"]);
    assert_eq!(machine(&out), Some((Machine::X86_64, 3)));
    assert_eq!(out.target.as_ref().unwrap().by, "d2/libf.a");
}

#[test]
fn s3_1_rule_3_load_order() {
    let s = Scratch::new("s3r3order");
    if !common::have(&["native", "x86"]) {
        return;
    }
    rule_3_fixture(&s);
    let plan = common::plan(&["-Ld1", "-Ld2", "-lf", "-lg"]);
    let (names, report) = check_schedules(|pool: &Pool| {
        let out = common::run_plan(s.path(), &plan, Options::default(), pool);
        (common::names(&out), common::report(&out))
    });
    assert_eq!(names, ["d2/libf.a(f.o)", "d2/libg.a(g.o)"]);
    assert_eq!(
        report,
        ["warning: argument '-lg': skipping incompatible d1/libg.a when searching for -lg"]
    );
    // `-lg` first: it takes d1's (AArch64), which sets the target.
    let out = common::run(s.path(), &["-Ld1", "-Ld2", "-lg", "-lf"]);
    assert_eq!(machine(&out), Some((Machine::AArch64, 3)));
    assert_eq!(common::names(&out), ["d1/libg.a(g.o)"]);
    // One key: the error sorts before the warning (by text, as the
    // collector does).
    assert_eq!(common::ids(&out), ["library-not-found", "skipped-incompatible"]);
}

#[test]
fn s3_1_no_target() {
    let s = Scratch::new("s3none");
    s.write("e.ld", "/* nothing */\n");
    let out = common::run(s.path(), &["e.ld", "/dev/null"]);
    assert_eq!(out.target, None);
    assert_eq!(
        common::report(&out),
        ["error: target emulation unknown: -m or at least one .o file required"]
    );
}

#[test]
#[ignore = "expected-fail: mortar_input has no dependency edge to mortar_cli for check_target (WP-021 handback)"]
fn s3_2_check_target_runs_when_no_m() {
    panic!("check_target is not called: the mortar_input -> mortar_cli edge is missing");
}

#[test]
fn s3_2_output_format_checked_late() {
    let s = Scratch::new("s3fmt");
    let Some(_) = common::cc(s.path(), "native", "a.o", C, &[]) else { return };
    s.write("x.ld", "OUTPUT_FORMAT(elf64-x86-64)\nOUTPUT_ARCH(i386:x86-64)\n");
    s.write("frob.ld", "OUTPUT_FORMAT(elf64-frob)\nTARGET(binary)\n");
    s.write("ok.ld", "OUTPUT_FORMAT(\"elf64-littleaarch64\", \"elf64-bigaarch64\", \"elf64-littleaarch64\")\nOUTPUT_ARCH(aarch64)\n");
    // No -m: the script comes before the object that sets the target.
    let out = common::run(s.path(), &["x.ld", "frob.ld", "ok.ld", "a.o"]);
    assert_eq!(
        common::report(&out),
        [
            "error: x.ld:1:1: incompatible file: elf64-littleaarch64 is expected but got elf64-x86-64\n>>> the target was set by a.o",
            "error: x.ld:2:1: incompatible file: aarch64 is expected but got i386:x86-64\n>>> the target was set by a.o",
            "error: frob.ld:1:1: unsupported file format: elf64-frob",
            "error: frob.ld:2:1: unsupported file format: binary",
        ]
    );
}

#[test]
fn s3_3_incompatible_object() {
    let s = Scratch::new("s3incompat");
    let Some(_) = common::cc(s.path(), "i686", "i.o", C, &[]) else { return };
    let out = common::run(s.path(), &["-maarch64linux", "i.o"]);
    assert_eq!(common::ids(&out), ["incompatible-file"]);
    assert!(out.files.is_empty());
}

#[test]
fn s3_3_incompatible_lazy_member() {
    let s = Scratch::new("s3lazy");
    if !common::have(&["native", "x86"]) {
        return;
    }
    common::cc(s.path(), "native", "a.o", C, &[]);
    common::cc(s.path(), "x86", "x.o", G, &[]);
    common::ar(s.path(), "libm.a", &["a.o", "x.o"], false);
    let out = common::run(s.path(), &["-maarch64linux", "libm.a"]);
    assert_eq!(
        common::report(&out),
        [
            "error: libm.a(x.o): incompatible file: elf64-littleaarch64 is expected but got elf64-x86-64\n>>> the target was set by -m aarch64linux"
        ]
    );
    assert_eq!(common::names(&out), ["libm.a(a.o)"]);
}

#[test]
fn s3_3_note_names_the_decider() {
    let s = Scratch::new("s3note");
    if !common::have(&["native", "i686"]) {
        return;
    }
    common::cc(s.path(), "native", "start.o", C, &[]);
    common::cc(s.path(), "i686", "i.o", G, &[]);
    let out = common::run(s.path(), &["start.o", "i.o"]);
    assert_eq!(
        common::report(&out),
        [
            "error: i.o: incompatible file: elf64-littleaarch64 is expected but got elf32-i386\n>>> the target was set by start.o"
        ]
    );
}

#[test]
fn s3_3_explicit_path_never_skipped() {
    let s = Scratch::new("s3explicit");
    if !common::have(&["native", "x86"]) {
        return;
    }
    common::cc(s.path(), "native", "a.o", C, &[]);
    common::cc(s.path(), "x86", "x.o", G, &[]);
    let abs = s.join("x.o");
    s.write("s.ld", format!("INPUT({})\n", abs.display()));
    let out = common::run(s.path(), &["a.o", "x.o", "s.ld"]);
    assert_eq!(common::ids(&out), ["incompatible-file", "incompatible-file"]);
    assert!(!common::ids(&out).contains(&"skipped-incompatible"));
}

#[test]
#[ignore = "expected-fail: no Arch trait yet (spec/arch.md); the e_flags hooks of §3.4 are not called (WP-021 follow-up)"]
fn s3_4_flags_hooks_called_in_fileid_order() {
    panic!("Arch::check_input_flags / merge_input_flags do not exist yet");
}

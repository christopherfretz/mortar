//! `spec/input.md` §13: every diagnostic of the phase, its kind, its
//! severity and its exact text, one case per row of the table. (Each row
//! is also tested in its own section; this is the table in one place.)
mod common;

use mortar_testutil::Scratch;

const M: &str = "-maarch64linux";

/// `(kind id, rendered)` of each problem of a line.
fn diags(s: &Scratch, args: &[&str]) -> Vec<(&'static str, String)> {
    let out = common::run(s.path(), args);
    out.problems.iter().map(|p| (p.id(), p.render())).collect()
}

/// Checks that a line gives exactly one diagnostic, of kind `id`, with
/// text `text`; a mismatch is recorded in `bad`.
fn one(bad: &mut Vec<String>, s: &Scratch, args: &[&str], id: &str, text: &str) {
    let got = diags(s, args);
    if got.len() != 1 || got[0].0 != id || got[0].1 != text {
        bad.push(format!("{args:?}:\n  got  {got:?}\n  want {id} {text:?}"));
    }
}

#[test]
fn s13_table() {
    let s = Scratch::new("s13");
    if !common::have(&["native", "x86"]) {
        return;
    }
    common::cc(s.path(), "native", "f.o", "int f(void){return 1;}\n", &[]);
    common::cc(s.path(), "x86", "x.o", "int x;\n", &[]);
    common::cc(s.path(), "native", "libf.so", "int f(void){return 1;}\n", &["-shared", "-fPIC", "-nostdlib"]);
    std::fs::create_dir(s.join("dir")).unwrap();
    std::fs::create_dir(s.join("loops")).unwrap();
    std::os::unix::fs::symlink("libl.so", s.join("loops/libl.so")).unwrap();
    s.write("junk.o", "\x01\x02\x03 not anything\n");
    s.write("bad.ld", "INPUT(\n");
    s.write("self.ld", "INPUT(self.ld)\n");
    let mut bad = Vec::new();

    one(&mut bad, &s, &[M, "nothere.o"], "cannot-open-file", "error: nothere.o: cannot open nothere.o: No such file or directory");
    one(
        &mut bad,
        &s,
        &[M, "dir"],
        "cannot-open-file",
        "error: dir: cannot open dir: Is a directory; file not recognized: file format not recognized",
    );
    one(
        &mut bad,
        &s,
        &[M, "-Lloops", "-ll"],
        "cannot-open-file",
        "error: argument '-ll': cannot open loops/libl.so: Too many levels of symbolic links",
    );
    one(
        &mut bad,
        &s,
        &[M, "-lnope"],
        "library-not-found",
        "error: argument '-lnope': cannot find -lnope: No such file or directory\n>>> the library search path is empty",
    );
    one(&mut bad, &s, &[M, "-L", ""], "invalid-option-value", "error: argument '-L': -L: invalid value ''");
    one(
        &mut bad,
        &s,
        &[M, "--sysroot=/s", "-L=rel", "f.o"],
        "ambiguous-sysroot-path",
        "error: argument '-L=rel': ambiguous sysroot path '=rel'\n>>> GNU ld and mold read it as /srel\n>>> lld reads it as /s/rel\n>>> write the directory with a leading '/' after '='",
    );
    s.write("libx.a", std::fs::read(s.join("x.o")).unwrap());
    // Skipped, and then not found.
    let got = diags(&s, &[M, "-L.", "-l:libx.a", "f.o"]);
    assert_eq!(
        got,
        [
            ("library-not-found", "error: argument '-l:libx.a': cannot find -l:libx.a: No such file or directory\n>>> searched .\n>>> skipped ./libx.a: elf64-x86-64".to_owned()),
            ("skipped-incompatible", "warning: argument '-l:libx.a': skipping incompatible ./libx.a when searching for -l:libx.a".to_owned()),
        ]
    );
    s.write("empty.o", b"");
    one(&mut bad, &s, &["empty.o"], "no-target", "error: target emulation unknown: -m or at least one .o file required");
    one(
        &mut bad,
        &s,
        &[M, "x.o"],
        "incompatible-file",
        "error: x.o: incompatible file: elf64-littleaarch64 is expected but got elf64-x86-64\n>>> the target was set by -m aarch64linux",
    );
    one(&mut bad, &s, &[M, "junk.o"], "unknown-file-type", "error: junk.o: file not recognized: file format not recognized");
    s.write("m.o", b"\xcf\xfa\xed\xfe\x0c\x00\x00\x01rest");
    one(
        &mut bad,
        &s,
        &[M, "m.o"],
        "refused",
        "error: m.o: unsupported file format: Mach-O; file not recognized: file format not recognized",
    );
    let got = diags(&s, &[M, "bad.ld"]);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, "malformed-input");
    assert!(got[0].1.contains("bad.ld:"), "{got:?}");
    s.write("short.o", &std::fs::read(s.join("f.o")).unwrap()[..100]);
    assert_eq!(diags(&s, &[M, "short.o"])[0].0, "malformed-input");
    s.write("t.txt", "x\n");
    common::ar(s.path(), "libt.a", &["t.txt"], false);
    one(
        &mut bad,
        &s,
        &[M, "libt.a"],
        "ignored-member",
        "warning: libt.a(t.txt): archive member is not a relocatable object: not an ELF file",
    );
    one(
        &mut bad,
        &s,
        &[M, "--whole-archive", "libt.a"],
        "ignored-member",
        "error: libt.a(t.txt): archive member is not a relocatable object: not an ELF file",
    );
    one(
        &mut bad,
        &s,
        &[M, "self.ld"],
        "script-cycle",
        "error: self.ld:1:7: linker script names itself\n>>> named by argument 'self.ld'",
    );
    one(
        &mut bad,
        &s,
        &[M, "-shared", "-Bstatic", "libf.so"],
        "static-link-of-shared-object",
        "error: libf.so: attempted static link of dynamic object libf.so",
    );
    one(
        &mut bad,
        &s,
        &[M, "libf.so"],
        "refused",
        "error: libf.so: unsupported feature: dynamic executable without -dynamic-linker\n>>> the shared object is libf.so",
    );
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

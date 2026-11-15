//! `spec/meta.md` T-MT-42, end to end: a static link of objects whose
//! `.mason.meta` containers hold garbage (a valid section header, random
//! contents) succeeds without a diagnostic and gives the same bytes as the
//! link of the same objects with the containers removed (`objcopy
//! --remove-section`), on the three targets, for an object of at most
//! 64 KiB and a larger one. No check reads a container today (MT-42), so
//! nothing of the garbage may reach the link; and the output has no
//! container (MT-31/33: consumed, not copied). The byte counter half of
//! T-MT-42 is `crates/mortar_input/tests/t_mt_42.rs`.
//!
//! T-MT-5 (MT-5's table of what binutils, llvm and elfutils do to a
//! container and its anchor, re-run on the harness's tools) is at the end.

mod support;

use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use mortar_testutil::{Scratch, run_with, tool, tools_required};
use support::{Source, build_by, with_output};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

/// A container of `n` garbage bytes, as a top-level `asm` statement.
fn container(n: usize, seed: u64) -> String {
    let mut x = seed | 1;
    let mut s = String::from("__asm__(\".pushsection .mason.meta,\\\"e\\\",%progbits\\n.p2align 3");
    for i in 0..n {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        s.push_str(if i % 32 == 0 { "\\n.byte " } else { "," });
        let _ = write!(s, "{}", x & 0xff);
    }
    s.push_str("\\n.popsection\");\n");
    s
}

fn sources() -> Vec<Source> {
    let main = format!(
        "{}extern int big(int);\nint main(void) {{ return big(3) == 3 * 2999 + 7 ? 42 : 1; }}\n",
        container(1000, 7)
    );
    let mut big = container(5000, 11);
    for i in 0..3000 {
        let _ = writeln!(big, "int f{i}(int x) {{ return x * {i} + 7; }}");
    }
    big.push_str("int big(int x) { return f2999(x); }\n");
    // `Source` holds static text; a test process leaks two strings.
    vec![
        Source { name: "main.c", text: Box::leak(main.into_boxed_str()) },
        Source { name: "big.c", text: Box::leak(big.into_boxed_str()) },
    ]
}

fn mortar(dir: &Path, args: &[String], out: &Path) -> (bool, String) {
    let a: Vec<OsString> = with_output(args, out);
    let mut c = Command::new(MORTAR);
    c.args(&a).current_dir(dir);
    let o = run_with(&mut c, b"", Duration::from_mins(5));
    (o.success(), o.stderr_text())
}

fn has_container(path: &Path) -> bool {
    let bytes = std::fs::read(path).unwrap();
    let h = mortar_elf::FileHeader::parse(&bytes).unwrap();
    let t = mortar_elf::section::SectionTable::parse(&bytes, &h).unwrap();
    t.iter().any(|s| s.name().is_ok_and(|n| n == b".mason.meta"))
}

// spec: meta.md T-MT-42, MT-42, MT-6, MT-31, MT-33
#[test]
fn garbage_containers_change_nothing() {
    let srcs = sources();
    let mut ran = 0;
    for cc in ["gcc", "x86_64-linux-gnu-gcc", "i686-linux-gnu-gcc"] {
        let s = Scratch::new(&format!("meta-{cc}"));
        let Some(built) = build_by(&s, cc, &["-O1"], &srcs) else {
            assert!(!tools_required(), "{cc} is required");
            continue;
        };
        // The target's binutils (the host's objcopy reads its own machine
        // only).
        let Some(objcopy) = tool(&format!("{}objcopy", cc.strip_suffix("gcc").unwrap())) else {
            assert!(!tools_required(), "the objcopy of {cc} is required");
            continue;
        };
        let objects = [s.join("main.c.o"), s.join("big.c.o")];
        assert!(std::fs::metadata(&objects[0]).unwrap().len() <= 64 << 10);
        assert!(std::fs::metadata(&objects[1]).unwrap().len() > 64 << 10);
        assert!(objects.iter().all(|o| has_container(o)), "{cc}: the fixtures have containers");

        let with = s.join("with");
        let (ok, err) = mortar(&built.dir, &built.args, &with);
        assert!(ok, "{cc}: {err}");
        assert_eq!(err, "", "{cc}: no diagnostic for an unread container");

        for o in &objects {
            let mut c = Command::new(&objcopy);
            c.arg("--remove-section=.mason.meta").arg(o);
            let r = run_with(&mut c, b"", Duration::from_mins(1));
            assert!(r.success(), "objcopy: {}", r.stderr_text());
            assert!(!has_container(o));
        }
        let without = s.join("without");
        let (ok, err) = mortar(&built.dir, &built.args, &without);
        assert!(ok, "{cc}: {err}");
        let (a, b) = (std::fs::read(&with).unwrap(), std::fs::read(&without).unwrap());
        assert!(a == b, "{cc}: the outputs differ ({} and {} bytes)", a.len(), b.len());
        assert!(!has_container(&with), "{cc}: the container is consumed (MT-31, MT-33)");
        ran += 1;
    }
    assert!(ran > 0 || !tools_required());
}

// -- T-MT-5: the table of MT-5, re-run on the harness's tools ---------------

/// One object: a container of `fill` (24 bytes, MT-3's section) and the
/// global function `anchor` its rows would anchor to.
fn mt5_source(anchor: &str, fill: u8) -> String {
    let bytes: Vec<String> = (0..24u8).map(|i| format!("{}", fill.wrapping_add(i))).collect();
    format!(
        "\t.section .mason.meta,\"e\",%progbits\n\t.p2align 3\n\t.byte {}\n\
         \t.text\n\t.globl {anchor}\n\t.type {anchor},%function\n{anchor}:\n\tret\n\
         \t.section .note.GNU-stack,\"\",%progbits\n",
        bytes.join(",")
    )
}

/// The containers of the ELF file `path`: (contents, flags, alignment).
fn mt5_containers(path: &Path) -> Vec<(Vec<u8>, u64, u64)> {
    let bytes = std::fs::read(path).unwrap();
    let h = mortar_elf::FileHeader::parse(&bytes).unwrap();
    let t = mortar_elf::section::SectionTable::parse(&bytes, &h).unwrap();
    t.iter()
        .filter(|s| s.name().is_ok_and(|n| n == b".mason.meta"))
        .map(|s| (s.data().unwrap().to_vec(), s.flags(), s.alignment().unwrap()))
        .collect()
}

/// The binding of the `.symtab` symbol `name`, `None` when there is none.
fn mt5_binding(path: &Path, name: &str) -> Option<u8> {
    let bytes = std::fs::read(path).unwrap();
    let h = mortar_elf::FileHeader::parse(&bytes).unwrap();
    let t = mortar_elf::section::SectionTable::parse(&bytes, &h).unwrap();
    let syms = t.find_symbol_table(mortar_elf::consts::SHT_SYMTAB).unwrap()?;
    syms.iter()
        .find(|(_, s)| syms.name(s).is_ok_and(|n| n == name.as_bytes()))
        .map(|(_, s)| s.binding())
}

fn mt5_run(dir: &Path, exe: &Path, args: &[&str]) {
    let mut c = Command::new(exe);
    c.args(args).current_dir(dir);
    let o = run_with(&mut c, b"", Duration::from_mins(1));
    assert!(o.success(), "{} {args:?}: {}", exe.display(), o.stderr_text());
}

/// What MT-5's table says a tool does to the container and the anchor.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Mt5 {
    /// Container byte-identical, `anchor` still global.
    Kept,
    /// Container byte-identical, `anchor` weak (`objcopy --weaken`).
    KeptWeak,
    /// Container byte-identical, `anchor` gone (removed or renamed).
    KeptUnanchored,
    /// No container left.
    Removed,
}

// spec: meta.md T-MT-5, MT-5
#[test]
#[allow(clippy::too_many_lines)] // MT-5's table, one row per line
fn mt5_tools_that_keep_it_and_tools_that_lose_it() {
    use Mt5::{Kept, KeptUnanchored, KeptWeak, Removed};
    let Some(asm) = tool("as") else { return };
    let s = Scratch::new("meta-mt5");
    let dir = s.path();
    std::fs::write(dir.join("a.s"), mt5_source("anchor_a", 0x10)).unwrap();
    std::fs::write(dir.join("b.s"), mt5_source("anchor_b", 0x80)).unwrap();
    mt5_run(dir, &asm, &["a.s", "-o", "a.o"]);
    mt5_run(dir, &asm, &["b.s", "-o", "b.o"]);
    let original = mt5_containers(&dir.join("a.o"));
    assert_eq!(original.len(), 1);
    assert_eq!(original[0].1, mortar_elf::consts::SHF_EXCLUDE);
    let (a_bytes, b_bytes) = (original[0].0.clone(), mt5_containers(&dir.join("b.o"))[0].0.clone());

    let check = |what: &str, file: &Path, expect: Mt5| {
        let c = mt5_containers(file);
        let sym = mt5_binding(file, "anchor_a");
        let got = match (c.as_slice(), sym) {
            ([], _) => Removed,
            ([(b, ..)], Some(1)) if *b == a_bytes => Kept,
            ([(b, ..)], Some(2)) if *b == a_bytes => KeptWeak,
            ([(b, ..)], None) if *b == a_bytes => KeptUnanchored,
            _ => {
                panic!("{what}: containers {c:?}, anchor binding {sym:?}: a row MT-5 does not have")
            }
        };
        assert_eq!(got, expect, "{what}: MT-5's row changed (a spec change)");
    };
    let mut n = 0;
    let mut row = |what: &str, exe: &str, args: &[&str], expect: Mt5| {
        let Some(exe) = tool(exe) else { return };
        n += 1;
        let out = format!("r{n}.o");
        std::fs::copy(dir.join("a.o"), dir.join(&out)).unwrap();
        let mut a: Vec<&str> = args.to_vec();
        a.push(&out);
        mt5_run(dir, &exe, &a);
        check(what, &dir.join(&out), expect);
    };
    row("strip -g", "strip", &["-g"], Kept);
    row("strip -x", "strip", &["-x"], Kept);
    row("strip --strip-unneeded", "strip", &["--strip-unneeded"], Kept);
    row("strip", "strip", &[], KeptUnanchored);
    row("strip --strip-all", "strip", &["--strip-all"], KeptUnanchored);
    row("objcopy --only-keep-debug", "objcopy", &["--only-keep-debug"], Kept);
    row("objcopy --localize-hidden", "objcopy", &["--localize-hidden"], Kept);
    row("objcopy --weaken", "objcopy", &["--weaken"], KeptWeak);
    row("objcopy --strip-dwo", "objcopy", &["--strip-dwo"], Kept);
    row("objcopy --prefix-symbols", "objcopy", &["--prefix-symbols=p_"], KeptUnanchored);
    row("objcopy --redefine-sym", "objcopy", &["--redefine-sym", "anchor_a=other"], KeptUnanchored);
    row("objcopy -N", "objcopy", &["-N", "anchor_a"], KeptUnanchored);
    row("objcopy --extract-dwo", "objcopy", &["--extract-dwo"], Removed);
    for v in ["19", "22"] {
        row("llvm-strip", &format!("llvm-strip-{v}"), &[], Removed);
        row("llvm-strip --strip-all", &format!("llvm-strip-{v}"), &["--strip-all"], Removed);
        row(
            "llvm-objcopy --strip-non-alloc",
            &format!("llvm-objcopy-{v}"),
            &["--strip-non-alloc"],
            Removed,
        );
    }
    row("eu-strip", "eu-strip", &[], Removed);

    // `ar` (regular and thin): the member is byte-identical; `strip
    // --strip-unneeded` on an archive keeps it in every member, `llvm-strip`
    // removes it from every member.
    let archive = |what: &str, strip: Option<(&str, &[&str])>, expect: Mt5| {
        let Some(ar) = tool("ar") else { return };
        let sub = dir.join(what.replace(' ', "_"));
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::copy(dir.join("a.o"), sub.join("a.o")).unwrap();
        std::fs::copy(dir.join("b.o"), sub.join("b.o")).unwrap();
        mt5_run(&sub, &ar, &["rcs", "lib.a", "a.o", "b.o"]);
        if let Some((exe, args)) = strip {
            let Some(exe) = tool(exe) else { return };
            let mut a = args.to_vec();
            a.push("lib.a");
            mt5_run(&sub, &exe, &a);
        }
        let x = sub.join("x");
        std::fs::create_dir_all(&x).unwrap();
        mt5_run(&x, &ar, &["x", "../lib.a"]);
        check(what, &x.join("a.o"), expect);
        let b = mt5_containers(&x.join("b.o"));
        let b_ok = match expect {
            Removed => b.is_empty(),
            _ => b.len() == 1 && b[0].0 == b_bytes,
        };
        assert!(b_ok, "{what}: the second member's container: {b:?}");
    };
    archive("ar", None, Kept);
    archive("strip --strip-unneeded archive", Some(("strip", &["--strip-unneeded"])), Kept);
    for v in ["19", "22"] {
        let exe = format!("llvm-strip-{v}");
        archive(&format!("llvm-strip-{v} archive"), Some((&exe, &[])), Removed);
    }
    if let Some(ar) = tool("ar") {
        // A thin archive names the object; its bytes are the object's.
        mt5_run(dir, &ar, &["rcsT", "thin.a", "a.o"]);
        let thin = std::fs::read(dir.join("thin.a")).unwrap();
        assert!(thin.starts_with(b"!<thin>\n") && !thin.windows(4).any(|w| w == b"MASN"));
    }

    // `ld -r`: one container, the inputs' contents concatenated in input
    // order (each 8-aligned; 24 bytes need no padding), flags `E`,
    // alignment 8; both anchors kept.
    for (exe, extra) in [("ld", &[][..]), ("ld.lld", &[][..]), ("mold", &[][..])] {
        let Some(ld) = tool(exe) else { continue };
        let out = format!("r-{exe}.o");
        let mut a: Vec<&str> = extra.to_vec();
        a.extend(["-r", "a.o", "b.o", "-o", &out]);
        mt5_run(dir, &ld, &a);
        let c = mt5_containers(&dir.join(&out));
        let want: Vec<u8> = [a_bytes.clone(), b_bytes.clone()].concat();
        assert!(
            c.len() == 1
                && c[0].0 == want
                && c[0].1 == mortar_elf::consts::SHF_EXCLUDE
                && c[0].2 == 8,
            "{exe} -r: MT-5's row changed: {c:?}"
        );
        for anchor in ["anchor_a", "anchor_b"] {
            assert_eq!(mt5_binding(&dir.join(&out), anchor), Some(1), "{exe} -r: {anchor}");
        }
    }
    assert!(n > 0 || !tools_required());
}

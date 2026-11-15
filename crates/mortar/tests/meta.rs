//! `spec/meta.md` T-MT-42, end to end: a static link of objects whose
//! `.mason.meta` containers hold garbage (a valid section header, random
//! contents) succeeds without a diagnostic and gives the same bytes as the
//! link of the same objects with the containers removed (`objcopy
//! --remove-section`), on the three targets, for an object of at most
//! 64 KiB and a larger one. No check reads a container today (MT-42), so
//! nothing of the garbage may reach the link; and the output has no
//! container (MT-31/33: consumed, not copied). The byte counter half of
//! T-MT-42 is `crates/mortar_input/tests/t_mt_42.rs`.

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

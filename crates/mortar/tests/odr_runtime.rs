//! `--odr-runtime` through a compiler driver (`spec/cli.md` §3.9,
//! `spec/odr.md` O-53a, §10.2; WP-198): the runtime input is linked, the
//! output binds now, `.note.mason` carries the table and the runtime's
//! note, the program runs; and `off` is the plain link's bytes. The
//! behaviour at load time is `tests/link/odr-runtime/`.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use mortar_testutil::{Outcome, Scratch, run_with, tool, tools_required};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

const MAIN: &str = "#include <stdio.h>\nint main(void) { printf(\"ok\\n\"); return 0; }\n";

struct Cx {
    scratch: Scratch,
    _bin: Scratch,
    b: String,
    gcc: PathBuf,
}

impl Cx {
    fn new(label: &str) -> Option<Self> {
        if !matches!(std::env::consts::ARCH, "aarch64" | "x86_64") {
            return None;
        }
        let Some(gcc) = tool("gcc") else {
            assert!(!tools_required(), "gcc is required");
            return None;
        };
        let bin = Scratch::new(&format!("{label}-bin"));
        std::os::unix::fs::symlink(MORTAR, bin.join("ld")).unwrap();
        let b = format!("-B{}/", bin.path().display());
        let cx = Self { scratch: Scratch::new(label), _bin: bin, b, gcc };
        cx.scratch.write("main.c", MAIN);
        let o = cx.gcc(&["-O1", "-fPIE", "-c", "main.c", "-o", "main.o"]);
        assert!(o.success(), "gcc: {}", o.stderr_text());
        Some(cx)
    }

    fn gcc(&self, args: &[&str]) -> Outcome {
        let mut c = Command::new(&self.gcc);
        c.args(args).current_dir(self.scratch.path()).env_remove("MORTAR_SCHED_SEED");
        run_with(&mut c, b"", Duration::from_mins(2))
    }

    fn link(&self, extra: &[&str], out: &str) -> Outcome {
        let mut a = vec![self.b.as_str(), "-pie", "main.o"];
        a.extend_from_slice(extra);
        a.extend(["-o", out]);
        self.gcc(&a)
    }

    fn bytes(&self, out: &str) -> Vec<u8> {
        std::fs::read(self.scratch.join(out)).unwrap()
    }
}

/// The `MasonMeta` notes of `image` by type: (type, descriptor), found by
/// their owner (the notes of `.note.mason` are contiguous).
fn notes(image: &[u8]) -> Vec<(u32, Vec<u8>)> {
    let owner = b"MasonMeta\0";
    let word = |o: usize| u32::from_le_bytes(image[o..o + 4].try_into().unwrap());
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = image[from..].windows(owner.len()).position(|w| w == owner) {
        let at = from + at;
        from = at + owner.len();
        let Some(head) = at.checked_sub(12) else { continue };
        if word(head) != 10 {
            continue;
        }
        let size = word(head + 4) as usize;
        out.push((word(head + 8), image[head + 24..head + 24 + size].to_vec()));
    }
    out
}

fn dynamic_flags(cx: &Cx, out: &str) -> String {
    let Some(readelf) = tool("readelf") else { return String::new() };
    let mut c = Command::new(readelf);
    c.args(["-d", out]).current_dir(cx.scratch.path());
    run_with(&mut c, b"", Duration::from_mins(1)).stdout_text()
}

// spec: cli.md §3.9, odr.md O-53a and §10.2 (WP-198): with
// `--odr-runtime=report` the output has the table's note and the
// runtime's (type 0x4d4f4403, {1, 1, 0, runtime id}), binds now, and
// runs; `=fatal` writes mode 2; `--odr=off` changes neither note; `-z
// lazy` keeps lazy binding; `=off` is the plain link's bytes; two links
// are the same bytes.
#[test]
fn runtime_input_note_and_bind_now() {
    let Some(cx) = Cx::new("odr-runtime") else { return };
    let id = if std::env::consts::ARCH == "aarch64" { 1 } else { 3 };
    for (opt, out) in [
        ("-Wl,--odr-runtime=report", "r1"),
        ("-Wl,--odr-runtime=report", "r2"),
        ("-Wl,--odr-runtime=fatal", "f"),
        ("-Wl,--odr-runtime=off", "off"),
    ] {
        let o = cx.link(&[opt], out);
        assert!(o.success(), "{opt}: {}", o.stderr_text());
    }
    let o = cx.link(&[], "plain");
    assert!(o.success(), "plain: {}", o.stderr_text());
    let o = cx.link(&["-Wl,--odr-runtime=report", "-Wl,-z,lazy"], "lazy");
    assert!(o.success(), "lazy: {}", o.stderr_text());
    assert_eq!(cx.bytes("r1"), cx.bytes("r2"), "two links differ");
    assert_eq!(cx.bytes("off"), cx.bytes("plain"), "=off is not the plain link");
    assert!(notes(&cx.bytes("plain")).is_empty(), "a note without the option");
    let le = |w: [u32; 4]| w.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let o = cx.link(&["-Wl,--odr-runtime=report", "-Wl,--odr=off"], "rt-odr-off");
    assert!(o.success(), "--odr=off: {}", o.stderr_text());
    for (out, mode) in [("r1", 1), ("f", 2), ("rt-odr-off", 1)] {
        let n = notes(&cx.bytes(out));
        let types: Vec<u32> = n.iter().map(|(t, _)| *t).collect();
        assert_eq!(types, [0x4d4f_4401, 0x4d4f_4403], "{out}");
        assert_eq!(n[1].1, le([1, mode, 0, id]), "{out}");
    }
    let d = dynamic_flags(&cx, "r1");
    if !d.is_empty() {
        assert!(d.contains("BIND_NOW"), "{d}");
        assert!(d.contains("NOW"), "{d}");
        let lazy = dynamic_flags(&cx, "lazy");
        assert!(!lazy.contains("BIND_NOW"), "{lazy}");
    }
    for out in ["r1", "f", "plain"] {
        let mut c = Command::new(cx.scratch.join(out));
        c.current_dir(cx.scratch.path()).env_remove("MASON_ODR");
        let o = run_with(&mut c, b"", Duration::from_mins(1));
        assert!(o.success(), "{out}: {}", o.stderr_text());
        assert_eq!(o.stdout_text(), "ok\n", "{out}");
    }
}

// spec: cli.md §3.9 (WP-198): a static executable is refused by name,
// `--odr-table=off` conflicts, `--incremental` is refused by name, `-r`
// is a no-op (the line's outcome without it).
#[test]
fn refusals_and_the_no_op() {
    let Some(cx) = Cx::new("odr-runtime-refusals") else { return };
    let o = cx.gcc(&[&cx.b, "-static", "main.o", "-Wl,--odr-runtime=report", "-o", "s"]);
    assert!(!o.success());
    assert!(
        o.stderr_text().contains("unsupported feature: odr-runtime-static"),
        "{}",
        o.stderr_text()
    );
    let o = cx.link(&["-Wl,--odr-runtime=fatal", "-Wl,--odr-table=off"], "c");
    assert!(!o.success());
    assert!(
        o.stderr_text().contains("--odr-runtime=fatal and --odr-table=off cannot be used together"),
        "{}",
        o.stderr_text()
    );
    let o = cx.link(&["-Wl,--odr-runtime=report", "-Wl,--incremental"], "i");
    assert!(!o.success());
    assert!(
        o.stderr_text().contains("unsupported feature: odr-runtime-incremental"),
        "{}",
        o.stderr_text()
    );
    // At `-r` the option is a no-op (spec/cli.md C-R2b; RL-7, WP-187):
    // the line links, nothing is checked, and no runtime object is added:
    // the output is the plain line's, byte for byte.
    let mut c = Command::new(MORTAR);
    c.args(["-r", "main.o", "-o", "r0.o"]).current_dir(cx.scratch.path());
    let plain = run_with(&mut c, b"", Duration::from_mins(1));
    if std::env::consts::ARCH != "aarch64" {
        // RL-7's cut is AArch64: x86-64 stays refused by RL-9's name.
        assert!(plain.stderr_text().contains("x86-64 output with -r"), "{}", plain.stderr_text());
        return;
    }
    assert!(plain.success(), "{}", plain.stderr_text());
    for mode in ["report", "fatal"] {
        let mut c = Command::new(MORTAR);
        c.args(["-r", &format!("--odr-runtime={mode}"), "main.o", "-o", "r1.o"])
            .current_dir(cx.scratch.path());
        let o = run_with(&mut c, b"", Duration::from_mins(1));
        assert!(o.success(), "{mode}: {}", o.stderr_text());
        assert_eq!(o.stderr_text(), plain.stderr_text(), "-r is a no-op");
        assert_eq!(cx.bytes("r1.o"), cx.bytes("r0.o"), "{mode}: no runtime object at -r");
    }
}

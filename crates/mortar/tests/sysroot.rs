//! `--sysroot` through the `mortar` executable (`spec/input.md` §4.3,
//! `spec/script.md` §2.4 rules 1 and 2), held to GNU ld's links of the
//! same lines: `-L=` and `-L$SYSROOT` directories under the sysroot, an
//! absolute path in a script inside the sysroot read inside it, a script
//! outside it read as written, and `--sysroot=/` (what the x86-64 and i686
//! cross drivers pass on every link) and an empty `--sysroot=` changing
//! nothing.

mod support;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mortar_testutil::{Outcome, Scratch, run_with, tool, tools_required};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

/// `_start` calls `ca` (in `liba.a`), which calls `cb` (in `libb.a`),
/// which calls `cc` (in `liba.a` again); the program exits 9.
const OBJECTS: &[(&str, &str)] = &[
    ("circ", "\t.text\n\t.globl _start\n_start:\n\tbl ca\n\tmov x8, #93\n\tsvc #0\n"),
    ("ca", "\t.text\n\t.globl ca\nca:\n\tb cb\n\t.globl cc\ncc:\n\tmov x0, #9\n\tret\n"),
    ("cb", "\t.text\n\t.globl cb\ncb:\n\tb cc\n"),
];

struct Cx {
    scratch: Scratch,
    bfd: PathBuf,
}

impl Cx {
    fn new() -> Option<Self> {
        if std::env::consts::ARCH != "aarch64" {
            return None;
        }
        let (Some(gcc), Some(bfd), Some(ar)) = (tool("gcc"), tool("ld.bfd"), tool("ar")) else {
            assert!(!tools_required(), "gcc, ld.bfd and ar are required");
            return None;
        };
        let cx = Self { scratch: Scratch::new("sysroot"), bfd };
        for (name, text) in OBJECTS {
            let src = cx.scratch.write(
                &format!("{name}.s"),
                format!("{text}\t.section .note.GNU-stack,\"\",%progbits\n"),
            );
            let o =
                cx.run(&gcc, &["-c".into(), src.into(), "-o".into(), format!("{name}.o").into()]);
            assert!(o.success(), "gcc {name}: {}", o.stderr_text());
        }
        std::fs::create_dir_all(cx.scratch.join("lib")).unwrap();
        std::fs::create_dir_all(cx.scratch.join("sr/lib")).unwrap();
        for (lib, obj) in [
            ("lib/liba.a", "ca.o"),
            ("lib/libb.a", "cb.o"),
            ("sr/lib/libsa.a", "ca.o"),
            ("sr/lib/libsb.a", "cb.o"),
        ] {
            let o = cx.run(&ar, &["rcs", lib, obj].map(OsString::from));
            assert!(o.success(), "ar: {}", o.stderr_text());
        }
        cx.scratch.write("sr/lib/g.ld", "GROUP ( /lib/libsa.a /lib/libsb.a )\n");
        cx.scratch.write("out.ld", "GROUP ( /lib/libsa.a )\n");
        Some(cx)
    }

    fn run(&self, exe: &Path, args: &[OsString]) -> Outcome {
        let mut c = Command::new(exe);
        c.args(args).current_dir(self.scratch.path()).env_remove("MORTAR_SCHED_SEED");
        run_with(&mut c, b"", Duration::from_mins(2))
    }

    /// `exe` on `line` with `-o out`. mortar also writes its map to
    /// `out.mortar-map`, and an output it writes is judged by the
    /// verifier, which must find nothing (`docs/VALIDATION.md` L3, WP-035).
    fn link(&self, exe: &Path, line: &[&str], out: &str) -> Outcome {
        let mut args: Vec<OsString> = line.iter().map(OsString::from).collect();
        args.extend(["-o".into(), out.into()]);
        let mortar = exe == Path::new(MORTAR);
        let map = self.scratch.join(format!("{out}.mortar-map"));
        if mortar {
            args.push(format!("--mortar-map={}", map.display()).into());
        }
        let o = self.run(exe, &args);
        if mortar && o.success() {
            support::plumb::clean(&self.scratch.join(out), &map, self.scratch.path());
        }
        o
    }

    fn exit(&self, out: &str) -> Option<i32> {
        run_with(&mut Command::new(self.scratch.join(out)), b"", Duration::from_mins(1))
            .status
            .and_then(|s| s.code())
    }
}

// spec: input.md §4.3 (the `-L` table: `=D` and `$SYSROOT D` are the
// sysroot joined with `D`; the last `--sysroot` wins), script.md §2.4
// rule 1 (a script inside the sysroot reads its absolute paths inside it)
#[test]
fn sysroot_directories_and_scripts_are_under_it() {
    let Some(cx) = Cx::new() else { return };
    // GNU ld reads the sysroot only from `--sysroot=DIR` (it ignores the
    // separate form); lld and mold read both, as mortar does (cli.md
    // §7.2): the separate form is held to the program alone.
    let lines: [(&[&str], bool); 4] = [
        (&["--sysroot=sr", "circ.o", "-L=/lib", "-lsa", "-lsb", "-lsa"], true),
        (&["--sysroot=sr", "circ.o", "-L$SYSROOT/lib", "-lsa", "-lsb", "-lsa"], true),
        (&["--sysroot=elsewhere", "--sysroot=sr", "circ.o", "sr/lib/g.ld"], true),
        (&["--sysroot", "sr", "circ.o", "sr/lib/g.ld"], false),
    ];
    for (line, gnu) in lines {
        let m = cx.link(Path::new(MORTAR), line, "m.out");
        assert!(m.success(), "mortar {line:?}: {}", m.stderr_text());
        assert_eq!(cx.exit("m.out"), Some(9), "mortar {line:?}");
        if gnu {
            let b = cx.link(&cx.bfd, line, "b.out");
            assert!(b.success(), "ld.bfd {line:?}: {}", b.stderr_text());
            assert_eq!(cx.exit("b.out"), Some(9), "ld.bfd {line:?}");
        }
    }
}

// spec: script.md §2.4 rule 1 (a script outside the sysroot uses its
// absolute paths as written: all three references fail to find
// `/lib/libsa.a`)
#[test]
fn a_script_outside_the_sysroot_is_not_under_it() {
    let Some(cx) = Cx::new() else { return };
    let line = ["--sysroot=sr", "circ.o", "out.ld"];
    for exe in [Path::new(MORTAR), cx.bfd.as_path()] {
        let o = cx.link(exe, &line, "o.out");
        assert!(!o.success(), "{}", exe.display());
        assert!(o.stderr_text().contains("/lib/libsa.a"), "{}", o.stderr_text());
    }
}

// spec: input.md §4.3 ("Spelling": `--sysroot=/ -L=/usr/lib` is
// `/usr/lib`), cli.md §3.3 (an empty `--sysroot=` is no sysroot): the
// output is the bytes of the line without the option
#[test]
fn sysroot_slash_and_empty_change_nothing() {
    let Some(cx) = Cx::new() else { return };
    let lib = cx.scratch.join("lib");
    let lib = lib.to_str().unwrap();
    let with_eq = format!("-L={lib}");
    let plain: &[&str] = &["circ.o", "-L", lib, "-la", "-lb", "-la"];
    let o = cx.link(Path::new(MORTAR), plain, "plain.out");
    assert!(o.success(), "{}", o.stderr_text());
    let want = std::fs::read(cx.scratch.join("plain.out")).unwrap();
    let lines: [&[&str]; 3] = [
        &["--sysroot=/", "circ.o", "-L", lib, "-la", "-lb", "-la"],
        &["--sysroot=", "circ.o", "-L", lib, "-la", "-lb", "-la"],
        &["--sysroot=/", "circ.o", with_eq.as_str(), "-la", "-lb", "-la"],
    ];
    for line in lines {
        let o = cx.link(Path::new(MORTAR), line, "s.out");
        assert!(o.success(), "{line:?}: {}", o.stderr_text());
        assert!(std::fs::read(cx.scratch.join("s.out")).unwrap() == want, "{line:?}");
        let b = cx.link(&cx.bfd, line, "b.out");
        assert!(b.success(), "ld.bfd {line:?}: {}", b.stderr_text());
    }
}

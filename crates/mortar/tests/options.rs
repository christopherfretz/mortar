//! The audit of the option table (`spec/cli.md` §4.2, WP-027d): every row
//! that is not `refused` has evidence. A `supported` or `partial` row is
//! linked with an observable effect checked here (against GNU ld's output
//! where GNU ld has the option), or names the test that checks it; a
//! `noop` row links to the same bytes as the line without it, and the
//! reason its row gives is checked where it can be. A row whose status
//! changes from `refused` must be added here: the list is compared with
//! the table.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mortar_cli::table::{Status, rows};
use mortar_testutil::{Outcome, Scratch, run_with, tool, tools_required};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

/// How a row's status is shown to hold.
#[derive(Clone, Copy)]
enum Proof {
    /// A check in this file.
    Check(fn(&Cx)),
    /// The test that checks it: a path from the repository root and the
    /// test function's name.
    Test(&'static str, &'static str),
    /// A `noop` row: the extra arguments (the option as written), and an
    /// optional check of the row's reason. The output must be the same
    /// bytes as the plain link's.
    Same(&'static [&'static str], Option<fn(&Cx)>),
}

use Proof::{Check, Same, Test};

/// Every row that is not `refused`, in table order.
const AUDIT: &[(&str, Proof)] = &[
    ("-o", Check(output_is_named)),
    ("-m", Check(emulation_sets_the_target)),
    ("-e", Check(entry_is_the_symbol)),
    ("-s", Check(strip_all)),
    ("-S", Check(strip_debug)),
    ("--strip-discarded", Same(&["--strip-discarded"], None)),
    ("-x", Check(discard_all)),
    ("-X", Check(discard_locals)),
    ("--discard-none", Check(discard_none)),
    ("--build-id", Check(build_id_styles)),
    ("--no-build-id", Check(build_id_styles)),
    ("--eh-frame-hdr", Check(eh_frame_hdr)),
    ("--no-eh-frame-hdr", Check(eh_frame_hdr)),
    ("--gc-sections", Test("crates/mortar/tests/gc.rs", "differential_c_and_cxx_against_gnu_ld")),
    ("--no-gc-sections", Test("crates/mortar/tests/pipeline.rs", "gc_sections_the_last_one_wins")),
    (
        "--print-gc-sections",
        Test("crates/mortar/tests/gc.rs", "i10_print_gc_sections_text_and_order"),
    ),
    (
        "--no-print-gc-sections",
        Test("crates/mortar/tests/gc.rs", "i10_print_gc_sections_text_and_order"),
    ),
    (
        "--gc-keep-exported",
        Test("crates/mortar/tests/gc.rs", "gc_keep_exported_roots_default_visibility_globals"),
    ),
    ("-l", Check(libraries)),
    ("-(", Check(groups)),
    ("-)", Check(groups)),
    ("--as-needed", Same(&["--as-needed"], Some(shared_objects_are_refused))),
    ("--no-as-needed", Same(&["--no-as-needed"], Some(shared_objects_are_refused))),
    ("-Bstatic", Check(bstatic)),
    ("--warn-backrefs", Check(backrefs)),
    ("--no-warn-backrefs", Check(backrefs)),
    ("-L", Check(libraries)),
    (
        "--sysroot",
        Test("crates/mortar/tests/sysroot.rs", "sysroot_directories_and_scripts_are_under_it"),
    ),
    ("--hash-style", Same(&["--hash-style=sysv"], Some(dynamic_outputs_are_refused))),
    ("--rosegment", Same(&["--rosegment"], Some(rodata_has_its_own_load))),
    ("--error-limit", Check(error_limit)),
    ("--fatal-warnings", Check(warning_switches)),
    ("--no-fatal-warnings", Check(warning_switches)),
    ("-w", Check(warning_switches)),
    ("--color-diagnostics", Check(color)),
    ("--no-color-diagnostics", Check(color)),
    ("--warn-execstack", Same(&["--warn-execstack"], Some(execstack_warns_by_default))),
    ("--version", Test("crates/mortar/tests/cli.rs", "version_goes_to_stdout")),
    ("-v", Test("crates/mortar/src/lib.rs", "requests_are_answered")),
    ("-V", Test("crates/mortar/src/lib.rs", "requests_are_answered")),
    ("--help", Test("crates/mortar/src/lib.rs", "requests_are_answered")),
    (
        "-Map",
        Test("crates/mortar/tests/pipeline.rs", "print_map_writes_the_map_to_standard_output"),
    ),
    ("-M", Test("crates/mortar/tests/pipeline.rs", "print_map_writes_the_map_to_standard_output")),
    (
        "--threads",
        Test("crates/mortar/tests/pipeline.rs", "the_output_is_a_function_of_the_inputs_alone"),
    ),
    ("--time-phases", Test("crates/mortar/tests/refusals.rs", "time_phases_lists_the_front")),
    ("-plugin", Same(&["-plugin", "/nonexistent/liblto_plugin.so"], Some(lto_is_refused))),
    ("-plugin-opt", Same(&["-plugin-opt=-fresolution=x.res"], None)),
    ("-plugin-save-temps", Same(&["-plugin-save-temps"], None)),
    ("-flto", Same(&["-flto"], None)),
    ("-flto-partition", Same(&["-flto-partition=none"], None)),
    ("-fuse-ld", Same(&["-fuse-ld=bfd"], None)),
    (
        "--fix-cortex-a53-843419",
        Test("crates/mortar/tests/pipeline.rs", "an_erratum_site_goes_through_a_patch"),
    ),
    ("-z relro", Check(relro)),
    ("-z norelro", Check(relro)),
    ("-z execstack", Check(execstack)),
    ("-z noexecstack", Check(execstack)),
    ("-z separate-code", Check(separate_code)),
    ("-z noseparate-code", Check(separate_code)),
    ("-z max-page-size", Check(max_page_size)),
    (
        "-z common-page-size",
        Same(&["-z", "common-page-size=4096"], Some(common_page_size_is_checked)),
    ),
    ("-z sectionheader", Same(&["-z", "sectionheader"], None)),
    ("-z nostart-stop-gc", Same(&["-z", "nostart-stop-gc"], None)),
    ("-z memory-seal", Check(memory_seal)),
    ("-z nomemory-seal", Check(memory_seal)),
    ("-EL", Same(&["-EL"], Some(big_endian_is_refused))),
    ("-g", Same(&["-g"], None)),
    ("-O", Same(&["-O", "1"], None)),
    ("-Qy", Same(&["-Qy"], None)),
    ("-qmagic", Same(&["-qmagic"], None)),
    ("-assert", Same(&["-assert", "definitions"], None)),
    ("--map-whole-files", Same(&["--map-whole-files"], None)),
    ("--no-map-whole-files", Same(&["--no-map-whole-files"], None)),
    ("--no-keep-memory", Same(&["--no-keep-memory"], None)),
    ("--reduce-memory-overheads", Same(&["--reduce-memory-overheads"], None)),
    ("--hash-size", Same(&["--hash-size=1021"], None)),
    ("--max-cache-size", Same(&["--max-cache-size=1048576"], None)),
];

// spec: cli.md §4.2 (every row that is not refused has its evidence; the
// list is the table's)
#[test]
fn every_row_that_is_not_refused_is_audited() {
    let honoured: Vec<String> = rows()
        .iter()
        .filter(|r| r.status != Status::Refused)
        .map(mortar_cli::table::Row::name)
        .collect();
    let audited: Vec<String> = AUDIT.iter().map(|(n, _)| (*n).to_owned()).collect();
    assert_eq!(audited, honoured, "AUDIT must list exactly the rows that are not refused");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    for (name, proof) in AUDIT {
        let row = rows().iter().find(|r| r.name() == *name).unwrap();
        match (proof, row.status) {
            (Test(file, f), _) => {
                let text = std::fs::read_to_string(root.join(file)).unwrap_or_else(|e| {
                    panic!("{name}: {file}: {e}");
                });
                assert!(text.contains(&format!("fn {f}(")), "{name}: no test {f} in {file}");
            }
            (Same(..), Status::Noop(_))
            | (Check(_), Status::Supported | Status::Partial { .. }) => {}
            (_, status) => panic!("{name}: {status:?} with the wrong kind of proof"),
        }
    }
}

// spec: cli.md §4.2 (each supported row's effect, each noop row's
// reason)
#[test]
fn every_audited_row_holds() {
    let Some(cx) = Cx::new() else { return };
    let mut done: Vec<usize> = Vec::new();
    for (name, proof) in AUDIT {
        match proof {
            Check(f) => {
                // One run per check, shared by the rows it covers.
                let id = *f as usize;
                if !done.contains(&id) {
                    done.push(id);
                    f(&cx);
                }
            }
            Same(extra, reason) => {
                let plain = cx.bytes(&[], "noop-plain");
                let with = cx.bytes(extra, "noop-with");
                assert!(plain == with, "{name}: a noop row changed the output");
                if let Some(f) = reason {
                    let id = *f as usize;
                    if !done.contains(&id) {
                        done.push(id);
                        f(&cx);
                    }
                }
            }
            Test(..) => {}
        }
    }
}

// ---- the context --------------------------------------------------------------

/// The scratch directory with the inputs every check links.
struct Cx {
    scratch: Scratch,
    gcc: PathBuf,
    bfd: PathBuf,
    readelf: PathBuf,
}

/// `_start` exits 42 through `sfn`, a local function; `.Ltemp` is a
/// temporary local kept by `as -L`; `.data` has a word; `.rodata` a
/// string; `.debug_*` from `-g`.
const START: &str = "\t.text\n\t.globl _start\n\t.type _start, %function\n_start:\n\t.cfi_startproc\n\
    \tbl sfn\n\tmov x8, #93\n\tsvc #0\n\t.cfi_endproc\n\
    \t.type sfn, %function\nsfn:\n.Ltemp:\n\tmov x0, #42\n\tret\n\
    \t.globl alt\n\t.type alt, %function\nalt:\n\tmov x0, #7\n\tmov x8, #93\n\tsvc #0\n\
    \t.data\n\t.globl word\nword:\t.quad 1\n\
    \t.section .rodata\nmsg:\t.asciz \"hello\"\n\
    \t.section .note.GNU-stack,\"\",%progbits\n";

/// Three undefined calls.
const UNDEF: &str = "\t.text\n\t.globl _start\n_start:\n\tbl u1\n\tbl u2\n\tbl u3\n\
    \t.section .note.GNU-stack,\"\",%progbits\n";

/// An object that asks for an executable stack.
const XSTACK: &str = "\t.text\n\t.globl xs\nxs:\tret\n\t.section .note.GNU-stack,\"x\",%progbits\n";

/// `_start` calls `ca` from `liba.a`, which calls `cb` from `libb.a`,
/// which calls `cc` from `liba.a` again.
const CIRC_MAIN: &str = "\t.text\n\t.globl _start\n_start:\n\tbl ca\n\tmov x8, #93\n\tsvc #0\n\
    \t.section .note.GNU-stack,\"\",%progbits\n";
const CIRC_A: &str = "\t.text\n\t.globl ca\nca:\n\tb cb\n\t.globl cc\ncc:\n\tmov x0, #9\n\tret\n\
    \t.section .note.GNU-stack,\"\",%progbits\n";
const CIRC_B: &str =
    "\t.text\n\t.globl cb\ncb:\n\tb cc\n\t.section .note.GNU-stack,\"\",%progbits\n";
/// A backward reference: `_start` calls `user` (in `libu.a`), which calls
/// `helper` (in the earlier `libh.a`); the program exits 9.
const BACK_MAIN: &str = "\t.text\n\t.globl _start\n_start:\n\tbl user\n\tmov x8, #93\n\tsvc #0\n\
    \t.section .note.GNU-stack,\"\",%progbits\n";
const BACK_USER: &str =
    "\t.text\n\t.globl user\nuser:\n\tb helper\n\t.section .note.GNU-stack,\"\",%progbits\n";
const BACK_HELPER: &str = "\t.text\n\t.globl helper\nhelper:\n\tmov x0, #9\n\tret\n\
    \t.section .note.GNU-stack,\"\",%progbits\n";

impl Cx {
    fn new() -> Option<Self> {
        if std::env::consts::ARCH != "aarch64" {
            return None;
        }
        let (Some(gcc), Some(bfd), Some(readelf), Some(ar)) =
            (tool("gcc"), tool("ld.bfd"), tool("readelf"), tool("ar"))
        else {
            assert!(!tools_required(), "gcc, ld.bfd, readelf and ar are required");
            return None;
        };
        let scratch = Scratch::new("options-audit");
        let cx = Self { scratch, gcc, bfd, readelf };
        for (name, text, extra) in [
            ("start", START, &["-g", "-Wa,-L"][..]),
            ("undef", UNDEF, &[]),
            ("xstack", XSTACK, &[]),
            ("circ", CIRC_MAIN, &[]),
            ("ca", CIRC_A, &[]),
            ("cb", CIRC_B, &[]),
            ("bm", BACK_MAIN, &[]),
            ("bu", BACK_USER, &[]),
            ("bh", BACK_HELPER, &[]),
        ] {
            let src = cx.scratch.write(&format!("{name}.s"), text);
            let mut args: Vec<OsString> = extra.iter().map(OsString::from).collect();
            args.extend(["-c".into(), src.into(), "-o".into(), format!("{name}.o").into()]);
            let o = cx.run_in(&cx.gcc, &args);
            assert!(o.success(), "gcc {name}: {}", o.stderr_text());
        }
        std::fs::create_dir_all(cx.scratch.join("lib")).unwrap();
        for (lib, obj) in [
            ("lib/liba.a", "ca.o"),
            ("lib/libb.a", "cb.o"),
            ("lib/libh.a", "bh.o"),
            ("lib/libu.a", "bu.o"),
        ] {
            let o = cx.run_in(&ar, &["rcs", lib, obj].map(OsString::from));
            assert!(o.success(), "ar: {}", o.stderr_text());
        }
        Some(cx)
    }

    fn run_in(&self, exe: &Path, args: &[OsString]) -> Outcome {
        let mut c = Command::new(exe);
        c.args(args).current_dir(self.scratch.path()).env_remove("MORTAR_SCHED_SEED");
        run_with(&mut c, b"", Duration::from_mins(2))
    }

    /// mortar on `args`.
    fn mortar(&self, args: &[&str]) -> Outcome {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        self.run_in(Path::new(MORTAR), &args)
    }

    /// GNU ld on `args`.
    fn ld(&self, args: &[&str]) -> Outcome {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        self.run_in(&self.bfd, &args)
    }

    /// mortar's link of `start.o` with `extra` to `out`, which must
    /// succeed; the output's path.
    fn link(&self, extra: &[&str], out: &str) -> PathBuf {
        let mut args = vec!["start.o", "-o", out];
        args.extend_from_slice(extra);
        let o = self.mortar(&args);
        assert!(o.success(), "mortar {extra:?}: {}", o.stderr_text());
        self.scratch.join(out)
    }

    /// GNU ld's link of `start.o` with `extra` to `out`.
    fn link_ld(&self, extra: &[&str], out: &str) -> PathBuf {
        let mut args = vec!["start.o", "-o", out];
        args.extend_from_slice(extra);
        let o = self.ld(&args);
        assert!(o.success(), "ld.bfd {extra:?}: {}", o.stderr_text());
        self.scratch.join(out)
    }

    fn bytes(&self, extra: &[&str], out: &str) -> Vec<u8> {
        std::fs::read(self.link(extra, out)).unwrap()
    }

    /// `readelf <flags> <path>`'s standard output.
    fn readelf(&self, flags: &str, path: &Path) -> String {
        let o = self.run_in(&self.readelf, &[flags.into(), path.into()]);
        assert!(o.success(), "readelf: {}", o.stderr_text());
        o.stdout_text()
    }

    /// The program's exit status.
    fn exit(path: &Path) -> Option<i32> {
        run_with(&mut Command::new(path), b"", Duration::from_mins(1)).status.and_then(|s| s.code())
    }
}

/// The names of `path`'s symbols of binding `bind` (`readelf -sW`).
fn symbols(cx: &Cx, path: &Path, bind: &str) -> Vec<String> {
    cx.readelf("-sW", path)
        .lines()
        .filter_map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            (w.len() >= 8 && w[4] == bind).then(|| w[7].to_owned())
        })
        .collect()
}

/// The program headers of `path`: (type, flags, align).
fn segments(cx: &Cx, path: &Path) -> Vec<(String, String, String)> {
    cx.readelf("-lW", path)
        .lines()
        .filter_map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            let ty = *w.first()?;
            if !(ty == "LOAD" || ty.starts_with("GNU_") || ty == "NOTE" || ty == "TLS") {
                return None;
            }
            let align = (*w.last()?).to_owned();
            let flags = w[6..w.len() - 1].join("");
            Some((ty.to_owned(), flags, align))
        })
        .collect()
}

fn has_segment(cx: &Cx, path: &Path, ty: &str) -> bool {
    segments(cx, path).iter().any(|s| s.0 == ty)
}

fn sections(cx: &Cx, path: &Path) -> String {
    cx.readelf("-SW", path)
}

// ---- supported rows -----------------------------------------------------------

// spec: cli.md §3.2 (`-o`)
fn output_is_named(cx: &Cx) {
    let out = cx.link(&[], "named.out");
    assert_eq!(Cx::exit(&out), Some(42));
}

// spec: cli.md §2.8 (`-m` sets the target; objects of another machine are
// input.md's incompatible-file)
fn emulation_sets_the_target(cx: &Cx) {
    let out = cx.link(&["-m", "aarch64elf"], "em.out");
    assert_eq!(Cx::exit(&out), Some(42));
    let o = cx.mortar(&["-m", "elf_x86_64", "start.o", "-o", "em2.out"]);
    assert!(!o.success());
    assert!(o.stderr_text().contains("the target was set by -m elf_x86_64"), "{}", o.stderr_text());
}

// spec: layout.md L-103 (`-e`)
fn entry_is_the_symbol(cx: &Cx) {
    let out = cx.link(&["-e", "alt"], "entry.out");
    assert_eq!(Cx::exit(&out), Some(7));
}

// spec: synth.md S-77 (`-s`), as GNU ld
fn strip_all(cx: &Cx) {
    for out in [cx.link(&["-s"], "s.out"), cx.link_ld(&["-s"], "s.ld")] {
        let s = sections(cx, &out);
        assert!(!s.contains(".symtab") && !s.contains(".debug_"), "{s}");
        assert_eq!(Cx::exit(&out), Some(42));
    }
}

// spec: synth.md S-77 (`-S`), as GNU ld
fn strip_debug(cx: &Cx) {
    for out in [cx.link(&["-S"], "ss.out"), cx.link_ld(&["-S"], "ss.ld")] {
        let s = sections(cx, &out);
        assert!(s.contains(".symtab") && !s.contains(".debug_"), "{s}");
    }
    let plain = cx.link(&[], "ss-plain.out");
    assert!(sections(cx, &plain).contains(".debug_info"));
}

// spec: synth.md S-78 (`-x`: no local of an input file)
fn discard_all(cx: &Cx) {
    for out in [cx.link(&["-x"], "x.out"), cx.link_ld(&["-x"], "x.ld")] {
        let locals = symbols(cx, &out, "LOCAL");
        assert!(!locals.iter().any(|n| n == "sfn" || n == "msg"), "{locals:?}");
        assert!(symbols(cx, &out, "GLOBAL").contains(&"_start".to_owned()));
    }
}

// spec: synth.md S-78 (`-X`: temporary locals only)
fn discard_locals(cx: &Cx) {
    let plain = symbols(cx, &cx.link(&[], "xx-plain.out"), "LOCAL");
    assert!(plain.contains(&".Ltemp".to_owned()), "{plain:?}");
    for out in [cx.link(&["-X"], "xx.out"), cx.link_ld(&["-X"], "xx.ld")] {
        let locals = symbols(cx, &out, "LOCAL");
        assert!(!locals.contains(&".Ltemp".to_owned()), "{locals:?}");
        assert!(locals.contains(&"sfn".to_owned()), "{locals:?}");
    }
}

// spec: synth.md S-78 (`--discard-none`, the last one wins)
fn discard_none(cx: &Cx) {
    for out in [
        cx.link(&["-X", "--discard-none"], "dn.out"),
        cx.link_ld(&["-X", "--discard-none"], "dn.ld"),
    ] {
        let locals = symbols(cx, &out, "LOCAL");
        assert!(locals.contains(&".Ltemp".to_owned()) && locals.contains(&"sfn".to_owned()));
    }
}

// spec: synth.md S-91 (the descriptor size of each style mortar writes;
// md5 and uuid: tests/refusals.rs)
fn build_id_styles(cx: &Cx) {
    for (opt, want) in [
        ("--build-id", Some("0x00000014")),
        ("--build-id=sha1", Some("0x00000014")),
        ("--build-id=sha256", Some("0x00000020")),
        ("--build-id=fast", Some("0x00000008")),
        ("--build-id=0x0123456789abcdef", Some("0x00000008")),
        ("--build-id=none", None),
        ("--no-build-id", None),
    ] {
        // `--no-build-id` after a bare one: the last one wins.
        let extra: &[&str] = if opt == "--no-build-id" { &["--build-id", opt] } else { &[opt] };
        let notes = cx.readelf("-nW", &cx.link(extra, "bid.out"));
        let line = notes.lines().find(|l| l.contains("NT_GNU_BUILD_ID"));
        match want {
            Some(size) => {
                assert!(line.is_some_and(|l| l.contains(size)), "{opt}: {notes}");
            }
            None => assert!(line.is_none(), "{opt}: {notes}"),
        }
    }
    let hex = cx.readelf("-nW", &cx.link(&["--build-id=0x0123456789abcdef"], "bid.out"));
    assert!(hex.contains("Build ID: 0123456789abcdef"), "{hex}");
}

// spec: synth.md S-57 (`--eh-frame-hdr`, `--no-eh-frame-hdr`)
fn eh_frame_hdr(cx: &Cx) {
    for (extra, want) in [
        (&["--eh-frame-hdr"][..], true),
        (&["--eh-frame-hdr", "--no-eh-frame-hdr"][..], false),
        (&[][..], false),
    ] {
        for out in [cx.link(extra, "eh.out"), cx.link_ld(extra, "eh.ld")] {
            assert_eq!(has_segment(cx, &out, "GNU_EH_FRAME"), want, "{extra:?}");
            assert_eq!(sections(cx, &out).contains(".eh_frame_hdr"), want, "{extra:?}");
        }
    }
}

// spec: input.md §4 (`-L` searches, `-l` takes `libNAME.a` or `:FILE`)
fn libraries(cx: &Cx) {
    for libs in [&["-la", "-lb", "-la"][..], &["-l:liba.a", "-l:libb.a", "-l:liba.a"][..]] {
        let mut args = vec!["circ.o", "-Llib"];
        args.extend_from_slice(libs);
        args.extend_from_slice(&["-o", "lib.out"]);
        let o = cx.mortar(&args);
        assert!(o.success(), "{libs:?}: {}", o.stderr_text());
        assert_eq!(Cx::exit(&cx.scratch.join("lib.out")), Some(9));
    }
    let o = cx.mortar(&["circ.o", "-la", "-o", "lib.out"]);
    assert!(o.stderr_text().contains("-la"), "without -L: {}", o.stderr_text());
}

// spec: resolve.md §4.3 (mortar searches every archive seen, as lld and
// mold), §4.6 (`--warn-backrefs` reports what GNU ld's single pass would
// leave undefined; `--no-warn-backrefs` after it turns it off): `cc` in
// `liba.a` is needed by `cb` from the later `libb.a`
fn backrefs(cx: &Cx) {
    fn line<'a>(extra: &[&'a str]) -> Vec<&'a str> {
        let mut args = vec!["bm.o", "-Llib", "-lh", "-lu", "-o", "br.out"];
        args.extend_from_slice(extra);
        args
    }
    let warn = |extra: &[&str]| {
        let o = cx.mortar(&line(extra));
        assert!(o.success(), "{extra:?}: {}", o.stderr_text());
        assert_eq!(Cx::exit(&cx.scratch.join("br.out")), Some(9));
        o.stderr_text()
    };
    assert_eq!(warn(&[]), "");
    let e = warn(&["--warn-backrefs"]);
    assert_eq!(
        e,
        "mortar: warning: lib/libu.a(bu.o):(helper): backward reference to an archive member: \
         helper\n>>> defined in lib/libh.a(bh.o)\n"
    );
    assert_eq!(warn(&["--warn-backrefs", "--no-warn-backrefs"]), "");
    // GNU ld's single pass leaves `helper` undefined on that line.
    let o = cx.ld(&line(&[]));
    assert!(!o.success() && o.stderr_text().contains("helper"), "{}", o.stderr_text());
    // With `-lh` again GNU ld finds it, and mortar does not warn; nor
    // about the circle of `liba.a` and `libb.a`, whose `cc` comes with the
    // member already taken.
    for args in [
        &["bm.o", "-Llib", "-lh", "-lu", "-lh", "--warn-backrefs", "-o", "br.out"][..],
        &["circ.o", "-Llib", "-la", "-lb", "--warn-backrefs", "-o", "br.out"][..],
    ] {
        let o = cx.mortar(args);
        assert!(o.success() && o.stderr_text().is_empty(), "{args:?}: {}", o.stderr_text());
    }
}

// spec: resolve.md §4.3 (a group's archives are searched until nothing
// more is taken; GNU ld needs the group for the cycle)
fn groups(cx: &Cx) {
    let args = ["circ.o", "-(", "lib/liba.a", "lib/libb.a", "-)", "-o", "grp.out"];
    for o in [cx.mortar(&args), cx.ld(&args)] {
        assert!(o.success(), "{}", o.stderr_text());
        assert_eq!(Cx::exit(&cx.scratch.join("grp.out")), Some(9));
    }
}

// spec: input.md §4 (`-Bstatic`: `-l` takes archives only, so a library
// with only a shared object is not found)
fn bstatic(cx: &Cx) {
    let dir = cx.scratch.join("so");
    std::fs::create_dir_all(&dir).unwrap();
    let o = cx.run_in(
        &cx.gcc,
        &["-shared", "-nostdlib", "ca.o", "-o", "so/libonly.so"].map(OsString::from),
    );
    assert!(o.success(), "gcc -shared: {}", o.stderr_text());
    std::fs::copy(cx.scratch.join("lib/liba.a"), dir.join("libonly.a")).unwrap();
    std::fs::copy(cx.scratch.join("lib/libb.a"), dir.join("libb.a")).unwrap();
    let o = cx.mortar(&["circ.o", "-Lso", "-Bstatic", "-lonly", "-lb", "-lonly", "-o", "bs.out"]);
    assert!(o.success(), "{}", o.stderr_text());
    let o = cx.mortar(&["circ.o", "-Lso", "-lonly", "-lb", "-o", "bs.out"]);
    assert!(o.stderr_text().contains("libonly.so"), "without -Bstatic: {}", o.stderr_text());
}

// spec: diag.md §3.9 / cli.md §3.9 (`--error-limit`)
fn error_limit(cx: &Cx) {
    let o = cx.mortar(&["undef.o", "-o", "el.out", "--error-limit=2"]);
    let e = o.stderr_text();
    assert_eq!(e.matches("undefined symbol").count(), 2, "{e}");
    assert!(e.contains("too many errors emitted"), "{e}");
    let o = cx.mortar(&["undef.o", "-o", "el.out", "--error-limit=0"]);
    assert_eq!(o.stderr_text().matches("undefined symbol").count(), 3, "{}", o.stderr_text());
}

// spec: cli.md §3.9 (`--fatal-warnings`, `--no-fatal-warnings`, `-w`),
// with the executable-stack warning (layout.md L-84)
fn warning_switches(cx: &Cx) {
    let base = ["start.o", "xstack.o", "-o", "w.out"];
    let run = |extra: &[&str]| {
        let mut args = base.to_vec();
        args.extend_from_slice(extra);
        cx.mortar(&args)
    };
    let plain = run(&[]);
    assert!(plain.success() && plain.stderr_text().contains("warning:"), "{}", plain.stderr_text());
    let fatal = run(&["--fatal-warnings"]);
    assert!(!fatal.success() && fatal.stderr_text().contains("error:"), "{}", fatal.stderr_text());
    let back = run(&["--fatal-warnings", "--no-fatal-warnings"]);
    assert!(back.success(), "{}", back.stderr_text());
    let quiet = run(&["-w"]);
    assert!(quiet.success() && quiet.stderr_text().is_empty(), "{}", quiet.stderr_text());
}

// spec: diag.md (colored diagnostics)
fn color(cx: &Cx) {
    let o = cx.mortar(&["undef.o", "-o", "c.out", "--color-diagnostics"]);
    assert!(o.stderr_text().contains('\u{1b}'), "{}", o.stderr_text());
    let o = cx.mortar(&["undef.o", "-o", "c.out", "--color-diagnostics", "--no-color-diagnostics"]);
    assert!(!o.stderr_text().contains('\u{1b}'), "{}", o.stderr_text());
}

// spec: layout.md L-64, L-65 (`-z relro` the default, `-z norelro`), as
// GNU ld
fn relro(cx: &Cx) {
    for (extra, want) in [(&["-z", "relro"][..], true), (&["-z", "norelro"][..], false)] {
        // A relro section is needed for the segment: .data.rel.ro through
        // the C start files is not here, so the check uses a TLS-free
        // object with .init_array.
        let src = cx.scratch.write(
            "ro.s",
            "\t.section .init_array,\"aw\",%init_array\n\t.quad 0\n\
             \t.section .note.GNU-stack,\"\",%progbits\n",
        );
        let o = cx.run_in(&cx.gcc, &["-c".into(), src.into(), "-o".into(), "ro.o".into()]);
        assert!(o.success());
        let mut args = vec!["start.o", "ro.o", "-o", "ro.out"];
        args.extend_from_slice(extra);
        for o in [cx.mortar(&args), cx.ld(&args)] {
            assert!(o.success(), "{}", o.stderr_text());
            assert_eq!(has_segment(cx, &cx.scratch.join("ro.out"), "GNU_RELRO"), want, "{extra:?}");
        }
    }
}

// spec: layout.md L-84 (`-z execstack`, `-z noexecstack`), as GNU ld
fn execstack(cx: &Cx) {
    for (extra, want) in
        [(&["-z", "execstack"][..], "RWE"), (&["-z", "noexecstack"][..], "RW"), (&[][..], "RW")]
    {
        for out in [cx.link(extra, "es.out"), cx.link_ld(extra, "es.ld")] {
            let s = segments(cx, &out);
            let stack = s.iter().find(|s| s.0 == "GNU_STACK").expect("PT_GNU_STACK");
            assert_eq!(stack.1, want, "{extra:?}");
        }
    }
}

// spec: layout.md L-71 (`-z separate-code`: the RX segment starts and
// ends on page boundaries in the file, so no page holds code and other
// bytes)
fn separate_code(cx: &Cx) {
    for (extra, separate) in
        [(&["-z", "separate-code"][..], true), (&["-z", "noseparate-code"][..], false)]
    {
        let out = cx.link(extra, "sc.out");
        let text = cx.readelf("-lW", &out);
        let rx = text
            .lines()
            .find(|l| l.trim_start().starts_with("LOAD") && l.contains("R E"))
            .expect("an RX PT_LOAD");
        let w: Vec<&str> = rx.split_whitespace().collect();
        let offset = u64::from_str_radix(w[1].trim_start_matches("0x"), 16).unwrap();
        assert_eq!(offset.is_multiple_of(0x10000), separate, "{extra:?}: {rx}");
    }
}

// spec: layout.md L-74 (`-z max-page-size`), as GNU ld
fn max_page_size(cx: &Cx) {
    for out in [
        cx.link(&["-z", "max-page-size=0x1000"], "mp.out"),
        cx.link_ld(&["-z", "max-page-size=0x1000"], "mp.ld"),
    ] {
        let loads: Vec<_> = segments(cx, &out).into_iter().filter(|s| s.0 == "LOAD").collect();
        assert!(!loads.is_empty() && loads.iter().all(|s| s.2 == "0x1000"), "{loads:?}");
        assert_eq!(Cx::exit(&out), Some(42));
    }
    let o = cx.mortar(&["start.o", "-o", "mp.out", "-z", "max-page-size=12"]);
    assert!(o.stderr_text().contains("max-page-size"), "{}", o.stderr_text());
}

// spec: synth.md §8.2 (`-z memory-seal` adds GNU_PROPERTY_MEMORY_SEAL), as
// GNU ld
fn memory_seal(cx: &Cx) {
    for (extra, want) in [
        (&["-z", "memory-seal"][..], true),
        (&["-z", "memory-seal", "-z", "nomemory-seal"][..], false),
    ] {
        for out in [cx.link(extra, "seal.out"), cx.link_ld(extra, "seal.ld")] {
            assert_eq!(cx.readelf("-nW", &out).contains("memory seal"), want, "{extra:?}");
        }
    }
}

// ---- the reasons of noop rows -------------------------------------------------

// The reason of `--as-needed`: a shared-object input is refused by name.
fn shared_objects_are_refused(cx: &Cx) {
    let Some(libc) = ["/usr/lib/aarch64-linux-gnu/libc.so.6", "/lib/aarch64-linux-gnu/libc.so.6"]
        .into_iter()
        .find(|p| Path::new(p).exists())
    else {
        return;
    };
    let o = cx.mortar(&["start.o", "--as-needed", libc, "-o", "so.out"]);
    assert!(!o.success());
    assert!(o.stderr_text().contains("unsupported feature"), "{}", o.stderr_text());
}

// The reason of `--hash-style`: every dynamic output is refused by name.
fn dynamic_outputs_are_refused(cx: &Cx) {
    for opt in ["-pie", "-shared", "--export-dynamic"] {
        let o = cx.mortar(&["start.o", opt, "-o", "dyn.out"]);
        assert!(!o.success());
        assert!(o.stderr_text().contains(&format!("unsupported option: {opt}")), "{opt}");
    }
}

// The reason of `--rosegment` (layout.md L-65).
fn rodata_has_its_own_load(cx: &Cx) {
    let out = cx.link(&[], "ro2.out");
    let s = segments(cx, &out);
    assert!(s.iter().any(|s| s.0 == "LOAD" && s.1 == "R"), "{s:?}");
}

// The reason of `--warn-execstack` (layout.md L-84).
fn execstack_warns_by_default(cx: &Cx) {
    let o = cx.mortar(&["start.o", "xstack.o", "-o", "xw.out"]);
    assert!(o.stderr_text().contains("requires executable stack"), "{}", o.stderr_text());
}

// The reason of `-plugin`: an LTO object is refused by name when read.
fn lto_is_refused(cx: &Cx) {
    let src = cx.scratch.write("lto.c", "int f(void) { return 1; }\n");
    let o =
        cx.run_in(&cx.gcc, &["-flto".into(), "-c".into(), src.into(), "-o".into(), "lto.o".into()]);
    assert!(o.success(), "{}", o.stderr_text());
    let o = cx.mortar(&["start.o", "lto.o", "-o", "lto.out"]);
    assert!(!o.success());
    assert!(o.stderr_text().contains("unsupported"), "{}", o.stderr_text());
}

// The reason of `-z common-page-size` (layout.md L-75): checked, unread.
fn common_page_size_is_checked(cx: &Cx) {
    let o = cx.mortar(&["start.o", "-o", "cp.out", "-z", "common-page-size=xyz"]);
    assert!(o.stderr_text().contains("common-page-size"), "{}", o.stderr_text());
}

// The reason of `-EL`: big-endian input is refused by name.
fn big_endian_is_refused(cx: &Cx) {
    let Some(clang) = tool("clang") else { return };
    let src = cx.scratch.join("start.s");
    let o = cx.run_in(
        &clang,
        &[
            "--target=aarch64_be-linux-gnu".into(),
            "-c".into(),
            src.into(),
            "-o".into(),
            "be.o".into(),
        ],
    );
    if !o.success() {
        return;
    }
    let o = cx.mortar(&["-EL", "be.o", "-o", "be.out"]);
    assert!(o.stderr_text().contains("big-endian"), "{}", o.stderr_text());
}

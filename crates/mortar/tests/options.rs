//! The audit of the option table (`spec/cli.md` §4.2, WP-027d): every row
//! that is not `refused` has evidence. A `supported` or `partial` row is
//! linked with an observable effect checked here (against GNU ld's output
//! where GNU ld has the option), or names the test that checks it; a
//! `noop` row links to the same bytes as the line without it, and the
//! reason its row gives is checked where it can be. A row whose status
//! changes from `refused` must be added here: the list is compared with
//! the table.

mod support;

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
    ("-no-pie", Check(no_pie)),
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
    ("--compress-debug-sections", Check(compress_debug_sections)),
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
    ("--no-gdb-index", Same(&["--no-gdb-index"], None)),
    ("-l", Check(libraries)),
    ("-(", Check(groups)),
    ("-)", Check(groups)),
    ("--as-needed", Same(&["--as-needed"], Some(shared_objects_are_refused))),
    ("--no-as-needed", Same(&["--no-as-needed"], Some(shared_objects_are_refused))),
    ("-Bstatic", Check(bstatic)),
    (
        "--no-copy-dt-needed-entries",
        Same(&["--no-copy-dt-needed-entries"], Some(copy_dt_needed_is_refused)),
    ),
    ("--push-state", Check(push_state)),
    ("--pop-state", Check(push_state)),
    // Its reason: tests/link/resolve/deplibs.test (resolve.md §10).
    ("--no-dependent-libraries", Same(&["--no-dependent-libraries"], None)),
    ("--warn-backrefs", Check(backrefs)),
    ("--no-warn-backrefs", Check(backrefs)),
    ("-L", Check(libraries)),
    (
        "--sysroot",
        Test("crates/mortar/tests/sysroot.rs", "sysroot_directories_and_scripts_are_under_it"),
    ),
    ("-rpath-link", Same(&["-rpath-link", "/opt/lib"], Some(shared_objects_are_refused))),
    ("-E", Same(&["--export-dynamic"], Some(dynamic_outputs_are_refused))),
    ("--no-export-dynamic", Same(&["--no-export-dynamic"], Some(dynamic_outputs_are_refused))),
    ("-Bsymbolic", Same(&["-Bsymbolic"], Some(dynamic_outputs_are_refused))),
    ("-Bsymbolic-functions", Same(&["-Bsymbolic-functions"], Some(dynamic_outputs_are_refused))),
    ("--Bsymbolic-non-weak", Same(&["--Bsymbolic-non-weak"], Some(dynamic_outputs_are_refused))),
    (
        "--Bsymbolic-non-weak-functions",
        Same(&["--Bsymbolic-non-weak-functions"], Some(dynamic_outputs_are_refused)),
    ),
    ("-Bno-symbolic", Same(&["-Bno-symbolic"], Some(dynamic_outputs_are_refused))),
    ("-rpath", Same(&["-rpath", "/opt/lib"], Some(dynamic_outputs_are_refused))),
    ("--no-dynamic-linker", Same(&["--no-dynamic-linker"], Some(no_interpreter))),
    ("--enable-new-dtags", Same(&["--enable-new-dtags"], Some(dynamic_outputs_are_refused))),
    ("--disable-new-dtags", Same(&["--disable-new-dtags"], Some(dynamic_outputs_are_refused))),
    ("--hash-style", Check(hash_style)),
    ("--rosegment", Same(&["--rosegment"], Some(rodata_has_its_own_load))),
    ("--error-limit", Check(error_limit)),
    ("--fatal-warnings", Check(warning_switches)),
    ("--no-fatal-warnings", Check(warning_switches)),
    ("-w", Check(warning_switches)),
    ("--color-diagnostics", Check(color)),
    ("--no-color-diagnostics", Check(color)),
    ("--no-undefined", Same(&["--no-undefined"], Some(undefined_is_an_error))),
    ("--warn-execstack", Same(&["--warn-execstack"], Some(execstack_warns_by_default))),
    ("--odr", Test("crates/mortar/tests/odr.rs", "modes")),
    ("--no-odr-check", Test("crates/mortar/tests/odr.rs", "off_reads_nothing_and_changes_no_byte")),
    ("--odr-stats", Test("crates/mortar/tests/odr.rs", "stats_line")),
    ("--detect-odr-violations", Same(&["--detect-odr-violations"], None)),
    ("--version", Test("crates/mortar/tests/cli.rs", "version_goes_to_stdout")),
    ("-v", Test("crates/mortar/src/lib.rs", "requests_are_answered")),
    ("-V", Test("crates/mortar/src/lib.rs", "requests_are_answered")),
    ("--help", Test("crates/mortar/src/lib.rs", "requests_are_answered")),
    (
        "-Map",
        Test("crates/mortar/tests/pipeline.rs", "the_text_map_matches_the_output_and_gnus_map"),
    ),
    ("-M", Test("crates/mortar/tests/pipeline.rs", "print_map_writes_the_map_to_standard_output")),
    ("--dependency-file", Check(dependency_file)),
    (
        "--threads",
        Test("crates/mortar/tests/pipeline.rs", "the_output_is_a_function_of_the_inputs_alone"),
    ),
    (
        "--mortar-map",
        Test("crates/mortar/tests/pipeline.rs", "print_map_writes_the_map_to_standard_output"),
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
    ("-z now", Test("crates/mortar/tests/pipeline.rs", "z_now_puts_the_got_plt_in_relro")),
    ("-z lazy", Test("crates/mortar/tests/pipeline.rs", "z_now_puts_the_got_plt_in_relro")),
    ("-z relro", Check(relro)),
    ("-z norelro", Check(relro)),
    ("-z execstack", Check(execstack)),
    ("-z noexecstack", Check(execstack)),
    ("-z text", Same(&["-z", "text"], Some(dynamic_outputs_are_refused))),
    ("-z separate-code", Check(separate_code)),
    ("-z noseparate-code", Check(separate_code)),
    ("-z defs", Same(&["-z", "defs"], Some(undefined_is_an_error))),
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

    /// mortar on `args`. With `-o X`, mortar also writes its map to
    /// `X.mortar-map`, and an output it writes is judged by the verifier,
    /// which must find nothing (`docs/VALIDATION.md` L3, WP-035).
    fn mortar(&self, args: &[&str]) -> Outcome {
        let out = args.windows(2).filter(|w| w[0] == "-o").map(|w| w[1]).next_back();
        let mut words: Vec<OsString> = args.iter().map(OsString::from).collect();
        let map = out.map(|o| self.scratch.join(format!("{o}.mortar-map")));
        if let Some(map) = &map {
            words.push(format!("--mortar-map={}", map.display()).into());
        }
        let o = self.run_in(Path::new(MORTAR), &words);
        if let (Some(out), Some(map), true) = (out, &map, o.success()) {
            let out = self.scratch.join(out);
            if out.exists() {
                let findings = support::plumb::verify(&out, map, self.scratch.path());
                if args.iter().any(|a| a.starts_with("--build-id=0x")) {
                    // spec/plumb.md §6.7 "The style from the map": a given
                    // id (`0xHEX`) of a hash's length is judged as a hash
                    // (a false B1) until mortar's map states the style
                    // `given` (a request to the image package). When it
                    // does, this exception is stale and fails.
                    assert!(
                        !findings.is_empty()
                            && findings.iter().all(|f| f.rule == mortar_plumb::Rule::BuildId),
                        "{args:?}: the given build id's exception is stale or hides more: \
                         {findings:#?}"
                    );
                } else {
                    assert!(findings.is_empty(), "plumb on {}: {findings:#?}", out.display());
                }
            }
        }
        o
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
    so_dir(cx);
    let o = cx.mortar(&["circ.o", "-Lso", "-Bstatic", "-lonly", "-lb", "-lonly", "-o", "bs.out"]);
    assert!(o.success(), "{}", o.stderr_text());
    let o = cx.mortar(&["circ.o", "-Lso", "-lonly", "-lb", "-o", "bs.out"]);
    assert!(o.stderr_text().contains("libonly.so"), "without -Bstatic: {}", o.stderr_text());
}

/// `so/`: `libonly.so` and `libonly.a` (both `ca`), and `libb.a`.
fn so_dir(cx: &Cx) {
    let dir = cx.scratch.join("so");
    std::fs::create_dir_all(&dir).unwrap();
    let o = cx.run_in(
        &cx.gcc,
        &["-shared", "-nostdlib", "ca.o", "-o", "so/libonly.so"].map(OsString::from),
    );
    assert!(o.success(), "gcc -shared: {}", o.stderr_text());
    std::fs::copy(cx.scratch.join("lib/liba.a"), dir.join("libonly.a")).unwrap();
    std::fs::copy(cx.scratch.join("lib/libb.a"), dir.join("libb.a")).unwrap();
}

// spec: cli.md §3.3 (`--push-state` saves the switches, `--pop-state`
// restores them): `-Bstatic` inside the pair chooses `libonly.a`, and
// after it `-l` chooses `libonly.so` again (refused by name), as GNU ld
// records it in `DT_NEEDED`; a `--pop-state` without a push is an error.
fn push_state(cx: &Cx) {
    so_dir(cx);
    let inside =
        ["circ.o", "-Lso", "--push-state", "-Bstatic", "-lonly", "-lb", "-lonly", "--pop-state"];
    let after =
        ["circ.o", "-Lso", "--push-state", "-Bstatic", "--pop-state", "-lonly", "-lb", "-lonly"];
    let o = cx.mortar(&[&inside[..], &["-o", "ps.out"]].concat());
    assert!(o.success(), "{}", o.stderr_text());
    assert_eq!(Cx::exit(&cx.scratch.join("ps.out")), Some(9));
    let o = cx.mortar(&[&after[..], &["-o", "ps.out"]].concat());
    assert!(o.stderr_text().contains("libonly.so"), "after --pop-state: {}", o.stderr_text());
    for (line, needed) in [(&inside, false), (&after, true)] {
        let out = cx.scratch.join("ps.ld");
        let o = cx.ld(&[&line[..], &["-o", "ps.ld"]].concat());
        assert!(o.success(), "ld.bfd: {}", o.stderr_text());
        assert_eq!(cx.readelf("-dW", &out).contains("libonly.so"), needed, "GNU ld {line:?}");
    }
    let o = cx.mortar(&["start.o", "--pop-state", "-o", "ps.out"]);
    assert!(!o.success());
    assert!(o.stderr_text().contains("--push-state"), "{}", o.stderr_text());
}

// spec: cli.md §4.2 (`--compress-debug-sections` is partial): `=none` is
// the uncompressed output mortar always writes, as GNU ld's; the others
// are refused by value.
fn compress_debug_sections(cx: &Cx) {
    let plain = cx.bytes(&[], "cds-plain");
    assert_eq!(cx.bytes(&["--compress-debug-sections=none"], "cds-none"), plain);
    let gnu = cx.link_ld(&["--compress-debug-sections=none"], "cds.ld");
    for out in [cx.scratch.join("cds-none"), gnu] {
        let s = sections(cx, &out);
        let debug: Vec<&str> = s.lines().filter(|l| l.contains(".debug_")).collect();
        assert!(!debug.is_empty() && debug.iter().all(|l| !l.contains(" C ")), "{s}");
    }
    for v in ["zlib", "zlib-gnu", "zlib-gabi", "zstd"] {
        let opt = format!("--compress-debug-sections={v}");
        let o = cx.mortar(&["start.o", opt.as_str(), "-o", "cds.out"]);
        assert!(!o.success() && o.stderr_text().contains(&opt), "{opt}: {}", o.stderr_text());
    }
}

// spec: input.md §10.4 (`--dependency-file`: the output, then each found
// path once in load order, escaped for Make; GNU ld lists the same
// paths; nothing is written when the link fails)
fn dependency_file(cx: &Cx) {
    std::fs::copy(cx.scratch.join("circ.o"), cx.scratch.join("c i#r$c.o")).unwrap();
    let line = ["c i#r$c.o", "-Llib", "-la", "-lb", "-la"];
    let o = cx.mortar(&[&line[..], &["--dependency-file=m.d", "-o", "d p.out"]].concat());
    assert!(o.success(), "{}", o.stderr_text());
    let got = std::fs::read_to_string(cx.scratch.join("m.d")).unwrap();
    assert_eq!(
        got,
        "d\\ p.out: \\\n c\\ i\\#r$$c.o \\\n lib/liba.a \\\n lib/libb.a\n\n\
         c\\ i\\#r$$c.o:\n\nlib/liba.a:\n\nlib/libb.a:\n"
    );
    let o = cx.ld(&[&line[..], &["--dependency-file=g.d", "-o", "d p.ld"]].concat());
    assert!(o.success(), "ld.bfd: {}", o.stderr_text());
    let gnu = std::fs::read_to_string(cx.scratch.join("g.d")).unwrap();
    let rules = |t: &str| -> Vec<String> {
        t.lines().filter_map(|l| l.strip_suffix(':')).map(str::to_owned).collect()
    };
    let unescaped: Vec<String> = rules(&got)
        .iter()
        .map(|r| r.replace("\\ ", " ").replace("\\#", "#").replace("$$", "$"))
        .collect();
    // GNU ld lists a path once per mention; mortar once (lld's choice).
    let mut gnu_once = rules(&gnu);
    let mut seen = std::collections::BTreeSet::new();
    gnu_once.retain(|p| seen.insert(p.clone()));
    assert_eq!(unescaped, gnu_once, "GNU ld: {gnu}");
    let _ = std::fs::remove_file(cx.scratch.join("u.d"));
    let o = cx.mortar(&["undef.o", "--dependency-file=u.d", "-o", "u.out"]);
    assert!(!o.success());
    assert!(!cx.scratch.join("u.d").exists(), "a failed link writes no dependency file");
}

// The reason of `--no-undefined` and `-z defs`: an executable's undefined
// symbol is an error without them (resolve.md §5).
fn undefined_is_an_error(cx: &Cx) {
    let o = cx.mortar(&["undef.o", "-o", "ud.out"]);
    assert!(!o.success());
    assert!(o.stderr_text().contains("undefined symbol: u1"), "{}", o.stderr_text());
}

// The reason of `--no-dynamic-linker`: a static executable has no
// `PT_INTERP`.
fn no_interpreter(cx: &Cx) {
    let out = cx.link(&[], "ni.out");
    assert!(!has_segment(cx, &out, "INTERP"));
    dynamic_outputs_are_refused(cx);
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

// spec: cli.md §3.2 (`-no-pie`: the default kind, a position-dependent
// executable; WP-042)
fn no_pie(cx: &Cx) {
    let plain = cx.bytes(&[], "nopie-plain");
    let out = cx.link(&["-no-pie"], "nopie.out");
    assert!(std::fs::read(&out).unwrap() == plain, "-no-pie changed the output");
    assert!(cx.readelf("-hW", &out).contains("EXEC (Executable file)"));
    let ld = cx.link_ld(&["-no-pie"], "nopie.ld");
    assert!(cx.readelf("-hW", &ld).contains("EXEC (Executable file)"));
}

// spec: dyn.md D-20, R-C1 (`--hash-style`: `gnu` only, which a static
// executable honours by having no hash table, as GNU ld's; the other
// styles are refused by name; WP-042)
fn hash_style(cx: &Cx) {
    let plain = cx.bytes(&[], "hash-plain");
    let out = cx.link(&["--hash-style=gnu"], "hash.out");
    assert!(std::fs::read(&out).unwrap() == plain, "--hash-style=gnu changed the output");
    let ld = cx.link_ld(&["--hash-style=gnu"], "hash.ld");
    for path in [&out, &ld] {
        let sections = cx.readelf("-SW", path);
        assert!(!sections.contains(".gnu.hash") && !sections.contains(" .hash"), "{sections}");
    }
    for style in ["sysv", "both", "none"] {
        let o = cx.mortar(&["start.o", &format!("--hash-style={style}"), "-o", "hash-x.out"]);
        assert!(!o.success(), "{style}");
        let want = format!("unsupported option value: --hash-style={style}");
        assert!(o.stderr_text().contains(&want), "{style}: {}", o.stderr_text());
        assert!(o.stderr_text().contains("dyn.md D-20"), "{}", o.stderr_text());
    }
}

// ---- the reasons of noop rows -------------------------------------------------

// The reason of `--no-copy-dt-needed-entries` (dyn.md D-15): its positive
// form is refused by name.
fn copy_dt_needed_is_refused(cx: &Cx) {
    let o = cx.mortar(&["start.o", "--copy-dt-needed-entries", "-o", "cdn.out"]);
    assert!(!o.success());
    let text = o.stderr_text();
    assert!(text.contains("unsupported option: --copy-dt-needed-entries"), "{text}");
}

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
    for opt in ["-pie", "-shared"] {
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

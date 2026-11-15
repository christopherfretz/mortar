//! The census (`spec/cli.md` §6): the link lines real compiler drivers
//! pass to `ld`, captured with a recording `ld` and parsed. Every line
//! must parse without an error and give the expected kind, target and
//! inputs, and use only the rows §6.2 lists.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use mortar_cli::{Emulation, InputSpec, OutputKind, Parsed, Target, parse};
use mortar_diag::Style;
use mortar_testutil::target::Arch;
use mortar_testutil::{DEFAULT_TIMEOUT, Scratch, run_with, tool};

/// The rows real link lines use, by name (`spec/cli.md` §6.2).
const CENSUS_ROWS: &[&str] = &[
    "-(",
    "-)",
    "--as-needed",
    "--build-id",
    "--eh-frame-hdr",
    "--fix-cortex-a53-843419",
    "--gc-sections",
    "--hash-style",
    "--no-as-needed",
    "--no-dynamic-linker",
    "--no-undefined-version",
    "--pop-state",
    "--push-state",
    "--sysroot",
    "--version-script",
    "-Bdynamic",
    "-Bstatic",
    "-EL",
    "-I",
    "-L",
    "-X",
    "-l",
    "-m",
    "-o",
    "-pie",
    "-plugin",
    "-plugin-opt",
    "-r",
    "-shared",
    "-z noexecstack",
    "-z now",
    "-z relro",
    "-z text",
];

/// A recording `ld` in `dir`: writes its arguments, NUL-terminated, to
/// `$CENSUS_OUT`.
fn recorder(scratch: &Scratch) -> PathBuf {
    let bin = scratch.join("bin");
    std::fs::create_dir_all(&bin).expect("mkdir bin");
    mortar_testutil::write_script(
        &bin.join("ld"),
        r#": > "$CENSUS_OUT"; for a in "$@"; do printf '%s\0' "$a" >> "$CENSUS_OUT"; done"#,
    );
    bin
}

/// Runs a driver with the recorder as its `ld` and returns the line.
fn capture(scratch: &Scratch, mut command: Command, label: &str) -> Vec<OsString> {
    let out = scratch.join(format!("{label}.args"));
    command.env("CENSUS_OUT", &out).current_dir(scratch.path());
    let outcome = run_with(&mut command, &[], DEFAULT_TIMEOUT);
    assert!(outcome.success(), "{label}: {command:?}\n{}", outcome.stderr_text());
    let bytes = std::fs::read(&out).unwrap_or_else(|e| panic!("{label}: no line recorded: {e}"));
    bytes
        .split(|&b| b == 0)
        .filter(|a| !a.is_empty())
        .map(|a| OsString::from_vec(a.to_vec()))
        .collect()
}

/// What a line must give.
struct Expect<'a> {
    kind: OutputKind,
    target: Target,
    /// Libraries (`-l`) the line must name.
    libs: &'a [&'a str],
    /// A suffix one of the path inputs must have (the compiled object).
    object: &'a str,
}

fn check(label: &str, line: &[OsString], want: &Expect<'_>, rows_seen: &mut BTreeSet<String>) {
    let parsed: Parsed = parse(line);
    let errors: Vec<String> =
        parsed.diagnostics.iter().map(|(_, d)| d.render(&Style::plain("ld"))).collect();
    assert!(errors.is_empty(), "{label}: {errors:?}\n{line:?}");
    assert_eq!(parsed.plan.kind, want.kind, "{label}: {line:?}");
    assert_eq!(parsed.plan.emulation.map(Emulation::target), Some(want.target), "{label}");
    for lib in want.libs {
        assert!(
            parsed.plan.inputs.iter().any(|i| i.spec == InputSpec::Library((*lib).into())),
            "{label}: no -l{lib} in {line:?}"
        );
    }
    assert!(
        parsed.plan.inputs.iter().any(|i| matches!(&i.spec, InputSpec::Path(p)
            if p.to_string_lossy().ends_with(want.object))),
        "{label}: no input ending {} in {line:?}",
        want.object
    );
    for u in &parsed.uses {
        rows_seen.insert(u.row().name());
    }
}

fn assert_census_rows(seen: &BTreeSet<String>) {
    let extra: Vec<&String> = seen.iter().filter(|n| !CENSUS_ROWS.contains(&n.as_str())).collect();
    assert!(
        extra.is_empty(),
        "rows used by link lines but not in spec/cli.md §6.2 and CENSUS_ROWS: {extra:?}"
    );
}

/// The C-family variants (`spec/cli.md` §6.1).
const VARIANTS: &[(&str, &[&str], OutputKind)] = &[
    ("default", &[], OutputKind::PieExecutable),
    ("no-pie", &["-no-pie"], OutputKind::Executable),
    ("static", &["-static"], OutputKind::StaticExecutable),
    ("static-pie", &["-static-pie"], OutputKind::StaticPie),
    ("shared", &["-shared"], OutputKind::Shared),
    ("r", &["-r"], OutputKind::Relocatable),
    ("pthread", &["-pthread"], OutputKind::PieExecutable),
    ("gc-as-needed", &["-Wl,--gc-sections,--as-needed"], OutputKind::PieExecutable),
    ("flto", &["-flto"], OutputKind::PieExecutable),
    ("g", &["-g"], OutputKind::PieExecutable),
];

/// One driver: compiles `source` once (and once with `-flto`), then links
/// each variant through the recorder.
fn driver(
    driver: &str,
    extra: &[&str],
    source: (&str, &str),
    target: Target,
    libs: &[&str],
    rows: &mut BTreeSet<String>,
) -> bool {
    let Some(path) = tool(driver) else { return false };
    let scratch = Scratch::new(&format!("census-{driver}"));
    let bin = recorder(&scratch);
    let src = scratch.write(source.0, source.1);
    let compile = |obj: &str, lto: bool| {
        let mut c = Command::new(&path);
        c.args(extra).arg("-c").arg("-fPIC").arg(&src).arg("-o").arg(scratch.join(obj));
        if lto {
            c.arg("-flto");
        }
        let outcome = run_with(&mut c, &[], DEFAULT_TIMEOUT);
        assert!(outcome.success(), "{driver} -c: {}", outcome.stderr_text());
    };
    compile("h.o", false);
    compile("hl.o", true);
    let clang = driver.starts_with("clang");
    for (label, flags, kind) in VARIANTS {
        let mut c = Command::new(&path);
        c.args(extra);
        if clang {
            c.arg(format!("--ld-path={}", bin.join("ld").display()));
        } else {
            c.arg(format!("-B{}/", bin.display()));
        }
        c.args(*flags);
        c.arg(scratch.join(if *label == "flto" { "hl.o" } else { "h.o" }));
        c.arg("-o").arg(scratch.join("out"));
        let line = capture(&scratch, c, label);
        let libs = if *label == "r" { &[][..] } else { libs };
        let object = if *label == "flto" { "hl.o" } else { "h.o" };
        check(
            &format!("{driver} {label}"),
            &line,
            &Expect { kind: *kind, target, libs, object },
            rows,
        );
    }
    true
}

const C: (&str, &str) = ("h.c", "int main(void) { return 0; }\n");
const CXX: (&str, &str) =
    ("h.cc", "#include <string>\nint main() { std::string s(\"x\"); return int(s.size()) - 1; }\n");
const FORTRAN: (&str, &str) = ("h.f90", "program h\n  print *, 'h'\nend program h\n");

fn census(name: &str, extra: &[&str], source: (&str, &str), target: Target, libs: &[&str]) {
    let mut rows = BTreeSet::new();
    if driver(name, extra, source, target, libs, &mut rows) {
        assert_census_rows(&rows);
    }
}

// spec: cli.md §6 (gcc, g++, gfortran for AArch64: native on an AArch64
// host, `aarch64-linux-gnu-*` elsewhere)
#[test]
fn gcc() {
    census(&Arch::Aarch64.gnu("gcc"), &[], C, Target::Aarch64, &["c", "gcc"]);
}

#[test]
fn gxx() {
    census(&Arch::Aarch64.gnu("g++"), &[], CXX, Target::Aarch64, &["stdc++", "m", "c"]);
}

#[test]
fn gfortran() {
    let gfortran = Arch::Aarch64.gnu("gfortran");
    census(&gfortran, &[], FORTRAN, Target::Aarch64, &["gfortran", "m", "c"]);
}

/// A Clang census for AArch64: `--target=aarch64-linux-gnu` off its host.
fn clang_aarch64(name: &str, source: (&str, &str), libs: &[&str]) {
    let args = Arch::Aarch64.clang_args();
    let extra: Vec<&str> = args.iter().map(String::as_str).collect();
    census(name, &extra, source, Target::Aarch64, libs);
}

// spec: cli.md §6 (clang and clang++, 19 and 22, for AArch64)
#[test]
fn clang_19() {
    clang_aarch64("clang-19", C, &["c"]);
}

#[test]
fn clang_22() {
    clang_aarch64("clang-22", C, &["c"]);
}

#[test]
fn clangxx_19() {
    clang_aarch64("clang++-19", CXX, &["stdc++", "m", "c"]);
}

#[test]
fn clangxx_22() {
    clang_aarch64("clang++-22", CXX, &["stdc++", "m", "c"]);
}

// spec: cli.md §6 (the cross drivers: x86-64 and i386)
#[test]
fn gcc_x86_64() {
    census("x86_64-linux-gnu-gcc", &[], C, Target::X86_64, &["c"]);
}

#[test]
fn gxx_x86_64() {
    census("x86_64-linux-gnu-g++", &[], CXX, Target::X86_64, &["stdc++", "c"]);
}

#[test]
fn gfortran_x86_64() {
    census("x86_64-linux-gnu-gfortran", &[], FORTRAN, Target::X86_64, &["gfortran", "c"]);
}

#[test]
fn gcc_i686() {
    census("i686-linux-gnu-gcc", &[], C, Target::I386, &["c"]);
}

#[test]
fn gxx_i686() {
    census("i686-linux-gnu-g++", &[], CXX, Target::I386, &["stdc++", "c"]);
}

#[test]
fn clang_x86_64() {
    census("clang-22", &["--target=x86_64-linux-gnu"], C, Target::X86_64, &["c"]);
}

#[test]
fn clang_i686() {
    census("clang-22", &["--target=i686-linux-gnu"], C, Target::I386, &["c"]);
}

// spec: cli.md §6 (rustc, through the C driver it runs). The installed
// rustc has its host's standard library only, so its line is for the
// host's architecture: the subject is the host's rustc, on purpose.
#[test]
fn rustc() {
    let target = match Arch::host() {
        Some(Arch::Aarch64) => Target::Aarch64,
        Some(Arch::X86_64) => Target::X86_64,
        Some(Arch::I386) => Target::I386,
        None => {
            mortar_testutil::named_skip(
                "census::rustc",
                "the host is none of the three targets; rustc links for the host",
            );
            return;
        }
    };
    let Some(rustc) = tool("rustc") else { return };
    let scratch = Scratch::new("census-rustc");
    let bin = recorder(&scratch);
    let src = scratch.write("h.rs", "fn main() { println!(\"h\"); }\n");
    let lib = scratch.write("l.rs", "#[unsafe(no_mangle)] pub extern \"C\" fn f() -> i32 { 1 }\n");
    let mut rows = BTreeSet::new();
    // `+crt-static` is a static PIE where rustc's target supports one
    // (x86_64-unknown-linux-gnu: `-static-pie`, which gcc passes on as
    // `-static -pie`), a static executable elsewhere.
    let static_kind =
        if target == Target::X86_64 { OutputKind::StaticPie } else { OutputKind::StaticExecutable };
    let cases: &[(&str, &Path, &[&str], OutputKind, &str)] = &[
        ("default", &src, &[], OutputKind::PieExecutable, ".rcgu.o"),
        ("g", &src, &["-g"], OutputKind::PieExecutable, ".rcgu.o"),
        ("static", &src, &["-C", "target-feature=+crt-static"], static_kind, ".rcgu.o"),
        ("cdylib", &lib, &["--crate-type", "cdylib"], OutputKind::Shared, ".rcgu.o"),
        ("lto", &src, &["-C", "lto"], OutputKind::PieExecutable, ".rcgu.o"),
    ];
    for (label, source, flags, kind, object) in cases {
        let mut c = Command::new(&rustc);
        c.arg(source).args(*flags).arg("-C").arg(format!("link-arg=-B{}/", bin.display()));
        if target == Target::X86_64 {
            // rustc links x86-64 with its own rust-lld by default (Rust
            // 1.90): the system `ld`, the one mortar stands in for, only
            // when told so (stable on this target alone).
            c.args(["-C", "linker-features=-lld"]);
        }
        c.arg("-o").arg(scratch.join(format!("out-{label}")));
        let line = capture(&scratch, c, label);
        let want = Expect { kind: *kind, target, libs: &["c"], object };
        check(&format!("rustc {label}"), &line, &want, &mut rows);
    }
    assert_census_rows(&rows);
}

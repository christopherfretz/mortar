//! `cargo xtask census`: the driver-line census of `spec/dyn.md` §2 as a
//! lane (WP-056). Every compiler driver of `scripts/ci-tools.sh` (GCC's
//! `gcc`, `g++` and `gfortran`; Clang 22's and 19's `clang` and
//! `clang++`; kiln's `kiln++` where the workspace has a kiln and hod
//! build, skipped by name where not) links a hello program in each
//! variation a real build passes; GNU ld's line is captured behind `-B`
//! wrappers, linked by the reference and run; mortar links the same line,
//! its program runs and the verifier judges its output.
//!
//!   cargo xtask census [--quick] [--arch <a>[,<a>]] [--filter <s>]... [--mortar <path>] [-j <n>] [--keep]
//!
//! A line passes when mortar's program runs as the reference's (exit
//! status and standard output) and the verifier is clean, or when mortar
//! refuses it by a name of [`OUT_OF_M2`] (a rule puts it out of the
//! milestone), or when GNU ld cannot link it either. Where the
//! reference's own program is known broken ([`REFERENCE_BROKEN`]), the
//! oracle is another variation's reference run. Every other outcome
//! fails the lane: a refusal no rule names, a program that runs
//! differently, a verifier finding [`KNOWN_FINDINGS`] does not list.
//! `--quick` is the subset `cargo xtask ci --quick` runs: the host's
//! architecture, [`QUICK_DRIVERS`] x [`QUICK_VARIATIONS`].

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::linktest::target::{Arch, ArchTools, Runner};
use crate::process::{self, End, Limits};

pub const USAGE: &str = "usage: cargo xtask census [--quick] [--arch <a>[,<a>]] [--filter <s>]... \
                         [--mortar <path>] [-j <n>] [--keep]\n";

/// The limits of a compile or a link.
const TOOL_LIMITS: Limits = Limits { time: Duration::from_mins(3), memory_kib: 8 << 20 };
/// The limits of a program.
const RUN_LIMITS: Limits = Limits { time: Duration::from_mins(1), memory_kib: 4 << 20 };

/// The language of a driver's hello.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Lang {
    C,
    Cxx,
    Fortran,
}

impl Lang {
    fn name(self) -> &'static str {
        match self {
            Self::C => "c",
            Self::Cxx => "c++",
            Self::Fortran => "fortran",
        }
    }

    /// The source file and its text: `spec/dyn.md` §2's census programs
    /// (C++: an exception, iostream, a `thread_local`).
    fn source(self) -> (&'static str, &'static str) {
        match self {
            Self::C => (
                "hello.c",
                "#include <stdio.h>\nint main(void){printf(\"hello C\\n\");return 0;}\n",
            ),
            Self::Cxx => (
                "hello.cpp",
                "#include <iostream>\n#include <stdexcept>\nthread_local int tl = 41;\n\
                 int main(){ try { throw std::runtime_error(\"boom\"); } catch (const std::exception&e)\
                 { std::cout << \"hello C++ \" << e.what() << \" \" << ++tl << std::endl; } return 0; }\n",
            ),
            Self::Fortran => {
                ("hello.f90", "program hello\nprint *, 'hello Fortran'\nend program hello\n")
            }
        }
    }
}

/// The objects a variation links: plain, `-fsanitize=address`, `-flto`,
/// `-fPIC` (a shared object's, WP-120).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Flavour {
    Plain,
    Asan,
    Lto,
    Pic,
}

impl Flavour {
    fn name(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Asan => "asan",
            Self::Lto => "lto",
            Self::Pic => "pic",
        }
    }

    fn args(self) -> &'static [&'static str] {
        match self {
            Self::Plain => &[],
            Self::Asan => &["-fsanitize=address"],
            Self::Lto => &["-flto"],
            Self::Pic => &["-fPIC"],
        }
    }
}

/// A variation of the link line: the driver options a real build adds.
#[derive(Clone, Copy, Debug)]
pub struct Variation {
    pub name: &'static str,
    pub args: &'static [&'static str],
    pub flavour: Flavour,
}

const fn v(name: &'static str, args: &'static [&'static str], flavour: Flavour) -> Variation {
    Variation { name, args, flavour }
}

/// The census's twenty variations (`spec/dyn.md` §2), then those WP-056
/// added (`x-`: the options of a real build no default line passes).
pub const VARIATIONS: &[Variation] = &[
    v("default", &[], Flavour::Plain),
    v("no-pie", &["-no-pie"], Flavour::Plain),
    v("pie", &["-pie"], Flavour::Plain),
    v("shared-libgcc", &["-shared-libgcc"], Flavour::Plain),
    v("static-libgcc", &["-static-libgcc"], Flavour::Plain),
    v("static-libstdc++", &["-static-libstdc++"], Flavour::Plain),
    v("pthread", &["-pthread"], Flavour::Plain),
    v("rdynamic", &["-rdynamic"], Flavour::Plain),
    v("as-needed", &["-Wl,--as-needed"], Flavour::Plain),
    v("relro", &["-Wl,-z,relro"], Flavour::Plain),
    v("now", &["-Wl,-z,now"], Flavour::Plain),
    v("hash-gnu", &["-Wl,--hash-style=gnu"], Flavour::Plain),
    v("hash-sysv", &["-Wl,--hash-style=sysv"], Flavour::Plain),
    v("hash-both", &["-Wl,--hash-style=both"], Flavour::Plain),
    v("build-id", &["-Wl,--build-id"], Flavour::Plain),
    v("rpath", &["-Wl,-rpath,/x"], Flavour::Plain),
    v("lld", &["-fuse-ld=lld"], Flavour::Plain),
    v("mold", &["-fuse-ld=mold"], Flavour::Plain),
    v("asan", &["-fsanitize=address"], Flavour::Asan),
    v("lto", &["-flto"], Flavour::Lto),
    v("x-norelro", &["-Wl,-z,norelro"], Flavour::Plain),
    v("x-no-pie-now", &["-no-pie", "-Wl,-z,now"], Flavour::Plain),
    v("x-static-pie", &["-static-pie"], Flavour::Plain),
    v("x-static-pie-export", &["-static-pie", "-Wl,--export-dynamic"], Flavour::Plain),
    v("x-init-fini", &["-Wl,-init=main", "-Wl,-fini=main"], Flavour::Plain),
    v("x-export-symbol", &["-Wl,--export-dynamic-symbol=main"], Flavour::Plain),
    v("x-export-glob", &["-Wl,--export-dynamic-symbol=ma*"], Flavour::Plain),
    v("x-bdynamic", &["-Wl,-Bstatic", "-Wl,-Bdynamic"], Flavour::Plain),
    v("x-runpath-old", &["-Wl,-rpath,/x", "-Wl,--disable-new-dtags"], Flavour::Plain),
    v("x-ztext", &["-Wl,-z,text"], Flavour::Plain),
    // The `-shared` lines (WP-120, spec/harness.md LT-7): a shared object
    // of the hello, loaded by `dlopen` from a loader the driver links.
    v("shared", &["-shared"], Flavour::Pic),
    v("shared-soname", &["-shared", "-Wl,-soname,libcensus.so"], Flavour::Pic),
    v(
        "shared-ldshared",
        &["-shared", "-Wl,-O1", "-Wl,-Bsymbolic-functions", "-Wl,-z,relro"],
        Flavour::Pic,
    ),
    // The `-r` lines (spec/harness.md "The census at `-r`"; record
    // spec/landed/layout/WP-154.md §A.2): the driver lines a reference
    // links. `-r -shared`, `-r -static-pie`, `-r -Wl,--gc-sections` and
    // `-Wl,-r` with the default PIE are refused by every driver: not
    // variations.
    v("r-plain", &["-r"], Flavour::Plain),
    v("r-nostdlib", &["-r", "-nostdlib"], Flavour::Plain),
    v("r-g", &["-r", "-g"], Flavour::Plain),
    v("r-pthread", &["-r", "-pthread"], Flavour::Plain),
    v("r-pie", &["-r", "-pie"], Flavour::Plain),
    v("r-no-pie", &["-r", "-no-pie"], Flavour::Plain),
    v("r-static", &["-r", "-static"], Flavour::Plain),
    v("r-s", &["-r", "-s"], Flavour::Plain),
    v("r-flto", &["-r"], Flavour::Lto),
    v("r-no-pie-nostdlib", &["-no-pie", "-nostdlib", "-Wl,-r"], Flavour::Plain),
    v("r-gc-e", &["-r", "-Wl,--gc-sections,-e,a"], Flavour::Plain),
    v("r-fuse-lld", &["-r", "-fuse-ld=lld"], Flavour::Plain),
    v("r-fuse-mold", &["-r", "-fuse-ld=mold"], Flavour::Plain),
];

/// The source of the loader of a `-shared` line's library (LT-7): it
/// `dlopen`s its argument and prints whether that loaded. C that is also
/// C++, so that every driver links it.
const LOADER: (&str, &str) = (
    "census-load.c",
    "#include <dlfcn.h>\n#include <stdio.h>\nint main(int argc, char **argv) {\n\
     void *h = argc > 1 ? dlopen(argv[1], RTLD_NOW) : 0;\n\
     puts(h ? \"loaded\" : \"failed\");\nreturn h ? 0 : 1;\n}\n",
);

impl Variation {
    /// Whether the line links a shared object (LT-7).
    pub fn is_shared(&self) -> bool {
        self.args.contains(&"-shared")
    }

    /// Whether the line is a `-r` line (its output is `ET_REL`).
    pub fn is_relocatable(&self) -> bool {
        self.args.iter().any(|a| *a == "-r" || *a == "-Wl,-r")
    }
}

/// The drivers of `--quick` (by [`Driver::name`]'s family and language).
pub const QUICK_DRIVERS: &[&str] = &["gcc/c", "gcc/c++", "gcc/fortran", "clang22/c++"];
/// The variations of `--quick`.
pub const QUICK_VARIATIONS: &[&str] =
    &["default", "no-pie", "rdynamic", "x-no-pie-now", "x-static-pie", "r-plain"];

/// The refusals of a `-r` line that are out of M4-r for now: a key
/// mortar's diagnostic carries, and why (`spec/diag.md` §4.4). RL-7
/// (WP-187) lifted `-r` itself; what stays refused is a later package's
/// name (or C-R4's for good).
pub const OUT_OF_M4R: &[(&str, &str)] = &[
    ("unsupported feature: section groups with -r", "RL-8: COMDAT groups at -r (C++)"),
    ("unsupported feature: removed .eh_frame records at -r", "RL-8: spec/layout.md L-R6"),
    (
        "unsupported feature: relocations against a dropped COMDAT copy at -r",
        "RL-8: spec/image.md I-R7",
    ),
    (
        "unsupported feature: the Mason container at -r",
        "RL-12: spec/meta.md MT-30 at -r (kiln's objects)",
    ),
    ("unsupported feature: x86-64 output with -r", "RL-9: x86-64 at -r"),
    ("unsupported feature: i386 output with -r", "RL-9: i386 (REL) at -r"),
    ("unsupported option: -s with -r", "RL-11: spec/image.md I-R9"),
    ("unsupported option: --strip-all with -r", "RL-11: spec/image.md I-R9"),
    ("unsupported option: -S with -r", "RL-11: spec/image.md I-R9"),
    ("unsupported option: --strip-debug with -r", "RL-11: spec/image.md I-R9"),
    ("unsupported option: -T with -r", "RL-11: spec/cli.md C-R3"),
    (
        "unsupported option: --gc-sections with -r",
        "spec/cli.md C-R4: a partial link cannot know what the final link needs (for good)",
    ),
    (
        "unsupported feature: GCC LTO sections with -r",
        "spec/resolve.md RV-R9: a line that needs it",
    ),
];

/// The refusals a rule puts out of M2: a key mortar's diagnostic carries,
/// and the rule.
pub const OUT_OF_M2: &[(&str, &str)] = &[
    (
        "unsupported option value: --hash-style=sysv",
        "dyn.md D-20: an executable gets .gnu.hash and no .hash; =sysv, =both and =none are refused",
    ),
    (
        "unsupported option value: --hash-style=both",
        "dyn.md D-20: an executable gets .gnu.hash and no .hash; =sysv, =both and =none are refused",
    ),
    ("unsupported file format: GCC LTO object", "LTO is out of M2 (dyn.md §2.7, item 6)"),
    ("unsupported file format: LLVM bitcode", "LTO is out of M2 (dyn.md §2.7, item 6)"),
    ("unsupported feature: dynamically linked output on i386", "i386 dynamic output is M4"),
    ("unsupported feature: position-independent executable output", "i386 PIE output is M4"),
    // The `-shared` lines (LT-7) since WP-123: what "Where the refusal
    // stays" of record WP-110 §D keeps refused in a shared object.
    (
        "unsupported feature: ELFCLASS32 shared object",
        "i386 shared objects are WP-130's (spec/dyn.md D-130)",
    ),
];

/// Lines whose REFERENCE program is known broken: the variation, the
/// architecture, the variation whose reference run is the oracle
/// instead, and the reason. The line fails when the reference's program
/// runs after all (the entry is stale), and passes when mortar's program
/// runs as the oracle variation's reference does.
pub const REFERENCE_BROKEN: &[(&str, &str, &str, &str)] = &[(
    "x-static-pie-export",
    "x86_64",
    "x-static-pie",
    "plumb.md A34: GNU ld's x86-64 static-pie with --export-dynamic dies at start-up (TPOFF64 records applied before the thread pointer exists)",
)];

/// Verifier findings judged wrong in the verifier, not in mortar (a
/// finding of the record that names them): the variation, the rule, and
/// the reason. Only a line whose findings are all of that rule passes.
pub const KNOWN_FINDINGS: &[(&str, &str, &str)] = &[];

/// A compiler driver of the census.
#[derive(Clone, Debug)]
pub struct Driver {
    /// `<family>/<arch>/<lang>`, the family `gcc`, `clang22`, `clang19`
    /// or `kiln`.
    pub name: String,
    pub family: &'static str,
    pub arch: Arch,
    pub lang: Lang,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl Driver {
    fn short(&self) -> String {
        format!("{}/{}", self.family, self.lang.name())
    }
}

/// The command line.
#[derive(Clone, Debug, Default)]
pub struct Args {
    pub quick: bool,
    pub arches: Vec<Arch>,
    pub filters: Vec<String>,
    pub mortar: Option<PathBuf>,
    pub jobs: usize,
    pub keep: bool,
}

impl Args {
    /// Parses `cargo xtask census`'s arguments.
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut a = Args {
            jobs: std::thread::available_parallelism().map_or(4, std::num::NonZero::get),
            ..Args::default()
        };
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            let mut value = |what: &str| it.next().cloned().ok_or(format!("{arg} needs {what}"));
            match arg.as_str() {
                "--quick" => a.quick = true,
                "--keep" => a.keep = true,
                "--arch" => {
                    for name in value("an architecture")?.split(',') {
                        a.arches
                            .push(Arch::parse(name).ok_or(format!("unknown architecture {name}"))?);
                    }
                }
                "--filter" => a.filters.push(value("a string")?),
                "--mortar" => a.mortar = Some(PathBuf::from(value("a path")?)),
                "-j" => {
                    a.jobs =
                        value("a number")?.parse().map_err(|_| "-j needs a number".to_owned())?;
                }
                other => return Err(format!("unknown argument {other}")),
            }
        }
        a.jobs = a.jobs.max(1);
        Ok(a)
    }
}

/// What became of one line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Linked, ran as the reference's, verifier clean.
    Ok,
    /// As [`Outcome::Ok`], the reference's program failing the same way.
    OkSameFailure,
    /// The driver writes no link line (it fails before linking).
    NoLine(String),
    /// Refused by a name of [`OUT_OF_M2`]: the key, the rule.
    OutOfM2(&'static str, &'static str),
    /// A `-r` line refused by a name of [`OUT_OF_M4R`]: the key, why.
    OutOfM4r(&'static str, &'static str),
    /// Refused; GNU ld cannot link the line either.
    RefusedWithReference(String),
    /// A finding of [`KNOWN_FINDINGS`]: the rule, the reason.
    KnownFinding(&'static str, &'static str),
    /// Fails the lane.
    Fail(String),
}

impl Outcome {
    fn word(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::OkSameFailure => "ok (the reference's program fails alike)",
            Self::NoLine(_) => "no line (the driver fails)",
            Self::OutOfM2(..) => "refused by name, out of M2",
            Self::OutOfM4r(..) => "refused by name, out of M4-r",
            Self::RefusedWithReference(_) => "refused (GNU ld cannot link it either)",
            Self::KnownFinding(..) => "known verifier finding",
            Self::Fail(_) => "FAIL",
        }
    }

    fn key(&self) -> String {
        match self {
            Self::Ok | Self::OkSameFailure => String::new(),
            Self::NoLine(k) | Self::RefusedWithReference(k) | Self::Fail(k) => k.clone(),
            Self::OutOfM2(k, why) | Self::OutOfM4r(k, why) | Self::KnownFinding(k, why) => {
                format!("{k} ({why})")
            }
        }
    }

    /// Whether the line passes the lane.
    pub fn passes(&self) -> bool {
        !matches!(self, Self::Fail(_))
    }
}

/// One line's result.
#[derive(Clone, Debug)]
pub struct Line {
    pub driver: String,
    pub short: String,
    pub arch: Arch,
    pub variation: &'static str,
    pub outcome: Outcome,
}

impl Line {
    fn id(&self) -> String {
        format!("{}.{}", self.driver, self.variation)
    }
}

/// The drivers of `arches` found on this machine; what is missing is a
/// note.
fn drivers(arches: &[Arch], host: Option<Arch>, notes: &mut Vec<String>) -> Vec<Driver> {
    let find = mortar_testutil::find_listed_tool;
    let mut out = Vec::new();
    for &arch in arches {
        for (lang, tool) in [(Lang::C, "gcc"), (Lang::Cxx, "g++"), (Lang::Fortran, "gfortran")] {
            let name = arch.gnu_tool(host, tool);
            match find(&name) {
                Some(program) => out.push(Driver {
                    name: format!("gcc/{}/{}", arch.name(), lang.name()),
                    family: "gcc",
                    arch,
                    lang,
                    program,
                    args: Vec::new(),
                    env: Vec::new(),
                }),
                None => notes.push(format!("{name} not found: its lines skipped")),
            }
        }
        for (family, major) in [("clang22", "22"), ("clang19", "19")] {
            for (lang, tool) in [(Lang::C, "clang"), (Lang::Cxx, "clang++")] {
                let name = format!("{tool}-{major}");
                let target = if host == Some(arch) {
                    Vec::new()
                } else {
                    vec![format!("--target={}", arch.triple())]
                };
                match find(&name) {
                    Some(program) => out.push(Driver {
                        name: format!("{family}/{}/{}", arch.name(), lang.name()),
                        family,
                        arch,
                        lang,
                        program,
                        args: target,
                        env: Vec::new(),
                    }),
                    None => notes.push(format!("{name} not found: its lines skipped")),
                }
            }
        }
        // kiln++ with hod, on an AArch64 host: natively for AArch64, with
        // `--target=<triple>` for x86-64 (WP-058; its programs run under
        // qemu as the other drivers' do). i386 is not kiln's target.
        if arch != Arch::I386 {
            match crate::mason::Mason::find() {
                Ok(m) if host == Some(Arch::Aarch64) => {
                    let hod_dir = m.hod.parent().map(Path::to_path_buf).unwrap_or_default();
                    let path = std::env::var_os("PATH").unwrap_or_default();
                    let mut dirs = vec![hod_dir];
                    dirs.extend(std::env::split_paths(&path));
                    let path = std::env::join_paths(dirs).unwrap_or(path);
                    out.push(Driver {
                        name: format!("kiln/{}/c++", arch.name()),
                        family: "kiln",
                        arch,
                        lang: Lang::Cxx,
                        program: m.kiln.clone(),
                        args: if host == Some(arch) {
                            Vec::new()
                        } else {
                            vec![format!("--target={}", arch.triple())]
                        },
                        env: vec![
                            ("PATH".into(), path.to_string_lossy().into_owned()),
                            ("KILN_BACKEND".into(), "hod".into()),
                            ("KILN_HOD".into(), m.hod.display().to_string()),
                            ("KILN_HOD_STRICT".into(), "1".into()),
                        ],
                    });
                }
                Ok(_) => notes.push(format!(
                    "kiln++ ({}): skipped, the census runs kiln on an AArch64 host only",
                    arch.name()
                )),
                Err(e) => notes.push(format!("kiln++ ({}): skipped: {e}", arch.name())),
            }
        }
    }
    out
}

/// The `-B` wrapper: records the linker's name and arguments in
/// `$CENSUS_ARGS`, then runs the real linker of its name.
fn wrapper(bfd: &Path, lld: Option<&Path>, mold: Option<&Path>) -> String {
    let or_missing =
        |p: Option<&Path>| p.map_or_else(|| "/nonexistent".to_owned(), |p| p.display().to_string());
    format!(
        "#!/bin/bash\nname=$(basename \"$0\")\n\
         if [ -n \"$CENSUS_ARGS\" ]; then printf '%s\\n' \"$name\" \"$@\" > \"$CENSUS_ARGS\"; fi\n\
         case \"$name\" in\n  *ld.lld) real={} ;;\n  *ld.mold) real={} ;;\n  *) real={} ;;\nesac\n\
         exec \"$real\" \"$@\"\n",
        or_missing(lld),
        or_missing(mold),
        bfd.display()
    )
}

/// The tools of the lane.
struct Lane {
    work: PathBuf,
    mortar: PathBuf,
    arch_tools: BTreeMap<Arch, ArchTools>,
    /// What the verifier judged of erratum 843419 (WP-103).
    erratum: Mutex<crate::linktest::engine::ErratumJudged>,
}

impl Lane {
    fn wrap_dir(&self, arch: Arch) -> PathBuf {
        self.work.join("wrap").join(arch.name())
    }

    fn object(&self, d: &Driver, f: Flavour) -> PathBuf {
        self.work.join("obj").join(format!("{}.{}.o", d.name.replace('/', "_"), f.name()))
    }

    fn run_program(&self, arch: Arch, exe: &Path) -> Result<(End, String), String> {
        self.run_with(arch, exe, &[])
    }

    fn run_with(&self, arch: Arch, exe: &Path, extra: &[String]) -> Result<(End, String), String> {
        let dir = exe.parent().unwrap_or(Path::new("."));
        let (program, mut args) = match &self.arch_tools[&arch].runner {
            Runner::Native => (exe.to_path_buf(), Vec::new()),
            Runner::Emulated { qemu, sysroot } => (
                qemu.clone(),
                vec!["-L".to_owned(), sysroot.display().to_string(), exe.display().to_string()],
            ),
        };
        args.extend(extra.iter().cloned());
        let o = run(&program, &args, dir, &[], RUN_LIMITS)?;
        Ok((o.end, String::from_utf8_lossy(&o.stdout).into_owned()))
    }

    /// What a `-shared` line's library does (LT-7): the loader, linked by
    /// the line's driver with GNU ld into `dir` once, `dlopen`s it.
    fn load(&self, d: &Driver, dir: &Path, library: &Path) -> Result<(End, String), String> {
        let loader = dir.join("census-load");
        if !loader.is_file() {
            let source = dir.join(LOADER.0);
            std::fs::write(&source, LOADER.1).map_err(|e| format!("{}: {e}", source.display()))?;
            let mut argv = d.args.clone();
            argv.push(format!("-B{}/", self.wrap_dir(d.arch).display()));
            argv.extend([source.display().to_string(), "-o".to_owned()]);
            argv.push(loader.display().to_string());
            let env: Vec<(&str, &str)> =
                d.env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            let linked = run(&d.program, &argv, dir, &env, TOOL_LIMITS)?;
            if !linked.end.success() {
                return Err(format!("the loader: {}", linked.first_error_line()));
            }
        }
        self.run_with(d.arch, &loader, &[library.display().to_string()])
    }

    /// The line's output, run: the program, or a `-shared` line's library
    /// under the loader.
    fn try_output(&self, d: &Driver, var: &Variation, out: &Path) -> Result<(End, String), String> {
        if var.is_shared() {
            self.load(d, out.parent().unwrap_or(Path::new(".")), out)
        } else {
            self.run_program(d.arch, out)
        }
    }

    /// The run of `d`'s reference program in the variation `oracle`,
    /// linked into `dir` (a line of `REFERENCE_BROKEN`).
    fn oracle_run(&self, d: &Driver, oracle: &str, dir: &Path) -> Result<(End, String), String> {
        let var = VARIATIONS
            .iter()
            .find(|v| v.name == oracle)
            .ok_or_else(|| "no such variation".to_owned())?;
        let exe = dir.join("ref-oracle");
        let mut argv = d.args.clone();
        argv.push(format!("-B{}/", self.wrap_dir(d.arch).display()));
        argv.extend(var.args.iter().map(|s| (*s).to_owned()));
        argv.push(self.object(d, var.flavour).display().to_string());
        argv.extend(["-o".to_owned(), exe.display().to_string()]);
        let args_s = dir.join("ld-oracle.args").display().to_string();
        let mut env: Vec<(&str, &str)> =
            d.env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        env.push(("CENSUS_ARGS", &args_s));
        let linked = run(&d.program, &argv, dir, &env, TOOL_LIMITS)?;
        if !linked.end.success() || !exe.is_file() {
            return Err(format!("GNU ld cannot link it: {}", linked.first_error_line()));
        }
        self.run_program(d.arch, &exe)
    }

    #[allow(clippy::too_many_lines, reason = "one line, start to verdict")]
    fn line(&self, d: &Driver, var: &Variation) -> Outcome {
        let id = format!("{}.{}", d.name.replace('/', "_"), var.name);
        let dir = self.work.join("lines").join(&id);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            return Outcome::Fail(format!("{}: {e}", dir.display()));
        }
        let args_file = dir.join("ld.args");
        let reference = dir.join("ref");
        let wrap = self.wrap_dir(d.arch);
        let mut argv = d.args.clone();
        argv.push(format!("-B{}/", wrap.display()));
        argv.extend(var.args.iter().map(|s| (*s).to_owned()));
        argv.push(self.object(d, var.flavour).display().to_string());
        argv.extend(["-o".to_owned(), reference.display().to_string()]);
        let mut env: Vec<(&str, &str)> =
            d.env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let args_s = args_file.display().to_string();
        env.push(("CENSUS_ARGS", &args_s));
        let linked = match run(&d.program, &argv, &dir, &env, TOOL_LIMITS) {
            Ok(o) => o,
            Err(e) => return Outcome::Fail(format!("{}: {e}", d.program.display())),
        };
        let Ok(captured) = std::fs::read_to_string(&args_file) else {
            return Outcome::NoLine(linked.first_error_line());
        };
        let wrap_s = wrap.display().to_string();
        let mut words: Vec<String> = captured
            .lines()
            .skip(1)
            .filter(|w| !(w.starts_with("-L") && w[2..].trim_end_matches('/') == wrap_s))
            .map(str::to_owned)
            .collect();
        let ref_run = if var.is_relocatable() {
            // A `-r` line's output runs only through a final link (below).
            (linked.end.success() && reference.is_file()).then(|| (linked.end, String::new()))
        } else if linked.end.success() && reference.is_file() {
            match self.try_output(d, var, &reference) {
                Ok(r) => Some(r),
                Err(e) => return Outcome::Fail(format!("the reference's program: {e}")),
            }
        } else {
            None
        };
        // A reference known broken (`REFERENCE_BROKEN`): the oracle is
        // another variation's reference run.
        let broken = REFERENCE_BROKEN
            .iter()
            .find(|(name, arch, _, _)| *name == var.name && *arch == d.arch.name());
        let ref_run = match (broken, ref_run) {
            (Some((_, _, oracle, why)), Some(theirs)) => {
                if theirs.0.success() {
                    return Outcome::Fail(format!(
                        "the reference's program runs now; remove the REFERENCE_BROKEN entry ({why})"
                    ));
                }
                match self.oracle_run(d, oracle, &dir) {
                    Ok(r) => Some(r),
                    Err(e) => return Outcome::Fail(format!("the oracle {oracle}: {e}")),
                }
            }
            (_, r) => r,
        };
        // mortar on the same words, its output and map beside.
        let out = dir.join("mortar.out");
        let map = dir.join("mortar.map");
        if let Some(i) = words.iter().position(|w| w == "-o")
            && let Some(o) = words.get_mut(i + 1)
        {
            *o = out.display().to_string();
        }
        let mut margs = words.clone();
        margs.push(format!("--mortar-map={}", map.display()));
        let linked = match run(&self.mortar, &margs, &dir, &[], TOOL_LIMITS) {
            Ok(o) => o,
            Err(e) => return Outcome::Fail(format!("mortar: {e}")),
        };
        if !linked.end.success() {
            let text = String::from_utf8_lossy(&linked.stderr).into_owned();
            if let Some((key, why)) = OUT_OF_M2.iter().find(|(k, _)| text.contains(k)) {
                return Outcome::OutOfM2(key, why);
            }
            if var.is_relocatable()
                && let Some((key, why)) = OUT_OF_M4R.iter().find(|(k, _)| text.contains(k))
            {
                return Outcome::OutOfM4r(key, why);
            }
            let first = text.lines().find(|l| l.contains("error")).unwrap_or_else(|| text.trim());
            let first = first.to_owned();
            return if ref_run.is_none() {
                Outcome::RefusedWithReference(first)
            } else {
                Outcome::Fail(format!("refused, no rule puts it out of M2: {first}"))
            };
        }
        if var.is_relocatable() {
            return self.relocatable(d, var, &dir, &reference, &out, &map, ref_run.is_some());
        }
        let ours = match self.try_output(d, var, &out) {
            Ok(r) => r,
            Err(e) => return Outcome::Fail(format!("mortar's program: {e}")),
        };
        let Some(theirs) = ref_run else {
            return Outcome::Fail(format!(
                "mortar links a line GNU ld cannot ({:?}, {:?})",
                ours.0,
                ours.1.trim()
            ));
        };
        if ours != theirs {
            return Outcome::Fail(format!(
                "runs differently: GNU ld's {:?} {:?}, mortar's {:?} {:?}",
                theirs.0,
                theirs.1.trim(),
                ours.0,
                ours.1.trim()
            ));
        }
        // Mortar's map states its line (`arg`, spec/plumb.md §3.2a): the
        // verifier reads the option and stage 3's words from it alone.
        let verified = crate::plumb::verify_link_line(
            &out,
            &map,
            mortar_plumb::Format::Mortar,
            &dir,
            None,
            None,
            None,
        );
        let report = match verified {
            Ok((_, report, _)) => report,
            Err(e) => return Outcome::Fail(format!("the verifier: {e}")),
        };
        self.erratum
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .count(report.coverage.erratum_option, &report.coverage);
        if !report.findings.is_empty() {
            let text = report.render(&out.display().to_string());
            let rules: Vec<String> = report.findings.iter().map(|f| f.rule.id()).collect();
            if let Some((_, rule, why)) = KNOWN_FINDINGS.iter().find(|(var_name, rule, _)| {
                *var_name == var.name && rules.iter().all(|r| r == rule)
            }) {
                return Outcome::KnownFinding(rule, why);
            }
            let first = text.lines().next().unwrap_or("").to_owned();
            return Outcome::Fail(format!("{} verifier finding(s): {first}", rules.len()));
        }
        if theirs.0.success() { Outcome::Ok } else { Outcome::OkSameFailure }
    }
}

impl Lane {
    /// A `-r` line mortar linked (spec/harness.md "The census at `-r`"):
    /// stage R on its output, then each `-r` output, mortar's and the
    /// reference's, final-linked by the driver (GNU ld) and run: the two
    /// programs must end and print alike. LT-R1's final link by mortar
    /// and P-P1 are the link tests' (RL-7 adds them here).
    #[allow(clippy::too_many_arguments, reason = "one line's files")]
    fn relocatable(
        &self,
        d: &Driver,
        var: &Variation,
        dir: &Path,
        reference: &Path,
        out: &Path,
        map: &Path,
        reference_linked: bool,
    ) -> Outcome {
        if !reference_linked {
            return Outcome::Fail("mortar links a -r line GNU ld cannot".to_owned());
        }
        let report = match crate::plumb::verify_link_line(
            out,
            map,
            mortar_plumb::Format::Mortar,
            dir,
            None,
            None,
            None,
        ) {
            Ok((_, report, _)) => report,
            Err(e) => return Outcome::Fail(format!("the verifier: {e}")),
        };
        if let Some(f) = report.findings.first() {
            return Outcome::Fail(format!(
                "stage R: {} finding(s): {} {}",
                report.findings.len(),
                f.rule.id(),
                f.message
            ));
        }
        let mut env: Vec<(&str, &str)> =
            d.env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        // The final line's words, for mortar's own finals (below).
        let final_args = dir.join("final.args");
        let final_args_s = final_args.display().to_string();
        env.push(("CENSUS_ARGS", &final_args_s));
        let mut runs = Vec::new();
        for (name, rout) in [("ref", reference), ("mortar", out)] {
            let exe = dir.join(format!("{name}.final"));
            let mut argv = d.args.clone();
            argv.push(format!("-B{}/", self.wrap_dir(d.arch).display()));
            argv.extend([rout.display().to_string(), "-o".to_owned(), exe.display().to_string()]);
            match run(&d.program, &argv, dir, &env, TOOL_LIMITS) {
                Ok(o) if o.end.success() => {}
                Ok(o) => {
                    return Outcome::Fail(format!(
                        "the final link of {name}'s -r output: {}",
                        o.first_error_line()
                    ));
                }
                Err(e) => return Outcome::Fail(format!("the final link: {e}")),
            }
            match self.run_program(d.arch, &exe) {
                Ok(r) => runs.push(r),
                Err(e) => return Outcome::Fail(format!("{name}'s final program: {e}")),
            }
        }
        if runs[0] != runs[1] {
            return Outcome::Fail(format!(
                "runs differently through -r: GNU ld's {:?}, mortar's {:?}",
                runs[0], runs[1]
            ));
        }
        self.mortar_finals(d, var, dir, out, map, &final_args, &runs[0])
    }

    /// RL-7 (WP-187; spec/harness.md "The census at `-r`"): mortar's own
    /// final link, on the driver's final words (the last final above, of
    /// mortar's `-r` output), of the object directly (`D`) and of mortar's
    /// `-r` output (`V`); both programs run as GNU ld's through `-r` did,
    /// and P-P1 judges `D`, `V` and the `-r` output `R`.
    #[allow(clippy::too_many_arguments, reason = "one line's files")]
    fn mortar_finals(
        &self,
        d: &Driver,
        var: &Variation,
        dir: &Path,
        r_out: &Path,
        r_map: &Path,
        final_args: &Path,
        want: &(End, String),
    ) -> Outcome {
        let Ok(captured) = std::fs::read_to_string(final_args) else {
            return Outcome::Fail("no final line captured".to_owned());
        };
        let wrap_s = self.wrap_dir(d.arch).display().to_string();
        let words: Vec<String> = captured
            .lines()
            .skip(1)
            .filter(|w| !(w.starts_with("-L") && w[2..].trim_end_matches('/') == wrap_s))
            .map(str::to_owned)
            .collect();
        let r_s = r_out.display().to_string();
        if !words.contains(&r_s) {
            return Outcome::Fail(format!("the final line does not name {r_s}"));
        }
        let object = self.object(d, var.flavour).display().to_string();
        let mut links = Vec::new();
        for (name, input) in [("direct", object.as_str()), ("via", r_s.as_str())] {
            let exe = dir.join(format!("mortar.{name}"));
            let exe_map = dir.join(format!("mortar.{name}.map"));
            let mut argv: Vec<String> = words
                .iter()
                .map(|w| if *w == r_s { input.to_owned() } else { w.clone() })
                .collect();
            if let Some(i) = argv.iter().position(|w| w == "-o")
                && let Some(o) = argv.get_mut(i + 1)
            {
                *o = exe.display().to_string();
            }
            argv.push(format!("--mortar-map={}", exe_map.display()));
            match run(&self.mortar, &argv, dir, &[], TOOL_LIMITS) {
                Ok(o) if o.end.success() => {}
                Ok(o) => {
                    return Outcome::Fail(format!(
                        "mortar's final link ({name}): {}",
                        o.first_error_line()
                    ));
                }
                Err(e) => return Outcome::Fail(format!("mortar's final link ({name}): {e}")),
            }
            match self.run_program(d.arch, &exe) {
                Ok(r) if r == *want => {}
                Ok(r) => {
                    return Outcome::Fail(format!(
                        "mortar's final ({name}) runs differently: {r:?}, GNU ld's through -r {want:?}"
                    ));
                }
                Err(e) => return Outcome::Fail(format!("mortar's final ({name}): {e}")),
            }
            links.push((exe, exe_map));
        }
        let [(dx, dm), (vx, vm)]: [(PathBuf, PathBuf); 2] =
            links.try_into().unwrap_or_else(|_| unreachable!());
        let args = crate::plumb::ComposeArgs {
            links: [(dx, dm), (vx, vm), (r_out.to_path_buf(), r_map.to_path_buf())],
            format: mortar_plumb::Format::Mortar,
            r_format: mortar_plumb::Format::Mortar,
            lines: [None, None, None],
        };
        match crate::plumb::compose_report(&args) {
            Ok((true, _)) => Outcome::Ok,
            Ok((false, text)) => Outcome::Fail(format!(
                "P-P1: {}",
                text.lines().find(|l| l.contains("P-P1")).unwrap_or(text.trim())
            )),
            Err(e) => Outcome::Fail(format!("P-P1: {e}")),
        }
    }
}

/// Runs `program args` in `dir` with `env`.
fn run(
    program: &Path,
    args: &[String],
    dir: &Path,
    env: &[(&str, &str)],
    limits: Limits,
) -> Result<process::Output, String> {
    process::run(
        program,
        |c| {
            c.args(args).current_dir(dir).env("LC_ALL", "C");
            for (k, v) in env {
                c.env(k, v);
            }
        },
        b"",
        limits,
    )
}

/// Runs `work` over `items` on `jobs` threads, results in item order.
fn parallel<T: Sync, R: Send>(items: &[T], jobs: usize, work: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let next = Mutex::new(0usize);
    let done = Mutex::new(Vec::with_capacity(items.len()));
    std::thread::scope(|s| {
        for _ in 0..jobs.min(items.len()).max(1) {
            s.spawn(|| {
                loop {
                    let i = {
                        let mut n = next.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                        let i = *n;
                        *n += 1;
                        i
                    };
                    let Some(item) = items.get(i) else { break };
                    let r = work(item);
                    done.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((i, r));
                }
            });
        }
    });
    let mut done = done.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner);
    done.sort_by_key(|(i, _)| *i);
    done.into_iter().map(|(_, r)| r).collect()
}

/// The lines of a run: (driver, variation), filtered.
fn plan<'a>(drivers: &'a [Driver], args: &Args) -> Vec<(&'a Driver, &'static Variation)> {
    let mut out = Vec::new();
    for d in drivers {
        for var in VARIATIONS {
            if var.name == "static-libstdc++" && d.lang != Lang::Cxx {
                continue;
            }
            if d.family == "kiln" && var.flavour != Flavour::Plain {
                continue;
            }
            if args.quick
                && !(QUICK_DRIVERS.contains(&d.short().as_str())
                    && QUICK_VARIATIONS.contains(&var.name))
            {
                continue;
            }
            let id = format!("{}.{}", d.name, var.name);
            if !args.filters.is_empty() && !args.filters.iter().any(|f| id.contains(f.as_str())) {
                continue;
            }
            out.push((d, var));
        }
    }
    out
}

/// Runs the lane; `Ok(passed)`.
#[allow(
    clippy::similar_names,
    clippy::too_many_lines,
    reason = "args and argv; the lane start to end"
)]
pub fn run_lane(args: &Args) -> Result<bool, String> {
    let started = Instant::now();
    let host = Arch::host();
    let arches: Vec<Arch> = if !args.arches.is_empty() {
        args.arches.clone()
    } else if args.quick {
        host.into_iter().filter(|a| *a != Arch::I386).collect()
    } else {
        vec![Arch::Aarch64, Arch::X86_64]
    };
    let mut notes = Vec::new();
    let mut arch_tools = BTreeMap::new();
    for &arch in &arches {
        match ArchTools::find(arch, host, mortar_testutil::find_listed_tool) {
            Ok(t) if t.bfd.is_some() => {
                arch_tools.insert(arch, t);
            }
            Ok(_) => notes.push(format!("{}: no GNU ld: skipped", arch.name())),
            Err(m) => notes.push(format!("{}: {} not found: skipped", arch.name(), m.what)),
        }
    }
    if mortar_testutil::tools_required() && !notes.is_empty() {
        return Err(format!("{} (MORTAR_REQUIRE_TOOLS=1)", notes.join("; ")));
    }
    let arches: Vec<Arch> = arch_tools.keys().copied().collect();
    let all = drivers(&arches, host, &mut notes);
    let lines = plan(&all, args);
    let mortar = match &args.mortar {
        Some(p) => p.clone(),
        None => crate::linktest::build_mortar()?,
    };
    let work =
        crate::linktest::build_dir().join("census").join(format!("run-{}", std::process::id()));
    let lane = Lane { work, mortar, arch_tools, erratum: Mutex::default() };
    let mk = |p: &Path| std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()));
    mk(&lane.work.join("obj"))?;
    let lld = mortar_testutil::find_listed_tool("ld.lld");
    let mold = mortar_testutil::find_listed_tool("mold");
    for (arch, t) in &lane.arch_tools {
        let dir = lane.wrap_dir(*arch);
        mk(&dir)?;
        let bfd = t.bfd.as_deref().unwrap_or(Path::new("ld.bfd"));
        let text = wrapper(bfd, lld.as_deref(), mold.as_deref());
        let mut names = vec!["ld".to_owned(), "ld.bfd".into(), "ld.lld".into(), "ld.mold".into()];
        for a in Arch::ALL {
            names.extend(
                ["ld", "ld.bfd", "ld.lld", "ld.mold"].iter().map(|n| format!("{}-{n}", a.triple())),
            );
        }
        for n in names {
            let p = dir.join(n);
            std::fs::write(&p, &text).map_err(|e| format!("{}: {e}", p.display()))?;
            set_executable(&p)?;
        }
    }
    for lang in [Lang::C, Lang::Cxx, Lang::Fortran] {
        let (file, text) = lang.source();
        let p = lane.work.join("obj").join(file);
        std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    // The objects the planned lines link, once each.
    let mut objects: Vec<(&Driver, Flavour)> = Vec::new();
    for (d, var) in &lines {
        if !objects.iter().any(|(o, f)| o.name == d.name && *f == var.flavour) {
            objects.push((d, var.flavour));
        }
    }
    let compiled = parallel(&objects, args.jobs, |(d, f)| {
        let mut argv = d.args.clone();
        argv.extend(f.args().iter().map(|s| (*s).to_owned()));
        let (src, _) = d.lang.source();
        argv.extend(["-c".to_owned(), src.to_owned(), "-o".to_owned()]);
        argv.push(lane.object(d, *f).display().to_string());
        let env: Vec<(&str, &str)> = d.env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        match run(&d.program, &argv, &lane.work.join("obj"), &env, TOOL_LIMITS) {
            Ok(o) if o.end.success() => None,
            Ok(o) => {
                Some(format!("{} {}: does not compile: {}", d.name, f.name(), o.first_error_line()))
            }
            Err(e) => Some(format!("{} {}: {e}", d.name, f.name())),
        }
    });
    let compile_failures: Vec<String> = compiled.into_iter().flatten().collect();
    let results: Vec<Line> = parallel(&lines, args.jobs, |(d, var)| Line {
        driver: d.name.clone(),
        short: d.short(),
        arch: d.arch,
        variation: var.name,
        outcome: if lane.object(d, var.flavour).is_file() {
            lane.line(d, var)
        } else {
            Outcome::Fail(format!("{} {}: no object", d.name, var.flavour.name()))
        },
    });
    print!("{}", report(&results, &notes, &compile_failures));
    let failed = results.iter().filter(|l| !l.outcome.passes()).count() + compile_failures.len();
    eprintln!("{}", shared_count(&results));
    eprintln!("{}", summary(&results, all.len(), failed, started.elapsed().as_secs_f64()));
    let e = *lane.erratum.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    eprintln!(
        "census: erratum 843419: {} outputs judged with the option ({} sites fixed, {} not \
         judged), {} without ({} sites)",
        e.with_option, e.sites, e.not_judged, e.without, e.sites_without
    );
    if args.keep {
        eprintln!("census: kept {}", lane.work.display());
    } else {
        let _ = std::fs::remove_dir_all(&lane.work);
    }
    Ok(failed == 0)
}

#[cfg(unix)]
fn set_executable(p: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("{}: {e}", p.display()))
}

/// The `-shared` lines' count (LT-7): how many, how many mortar linked
/// (its library loaded as the reference's and the verifier passed it, or
/// its finding is known), how many it refused by name.
/// The census's last line but one: the counts, the `-r` lines' in
/// brackets (spec/harness.md "The census at `-r`"). `failed` counts the
/// compile failures too.
pub fn summary(lines: &[Line], drivers: usize, failed: usize, secs: f64) -> String {
    let is_r = |l: &&Line| VARIATIONS.iter().any(|v| v.name == l.variation && v.is_relocatable());
    let r = lines.iter().filter(is_r).count();
    let rf = lines.iter().filter(is_r).filter(|l| !l.outcome.passes()).count();
    let passed = lines.len() - lines.iter().filter(|l| !l.outcome.passes()).count();
    format!(
        "census: {} lines ({r} -r), {drivers} drivers, {passed} pass ({} -r), {failed} fail \
         ({rf} -r), in {secs:.1} s",
        lines.len(),
        r - rf
    )
}

pub fn shared_count(lines: &[Line]) -> String {
    let shared: Vec<&Line> = lines
        .iter()
        .filter(|l| VARIATIONS.iter().any(|v| v.name == l.variation && v.is_shared()))
        .collect();
    let linked = shared
        .iter()
        .filter(|l| {
            matches!(l.outcome, Outcome::Ok | Outcome::OkSameFailure | Outcome::KnownFinding(..))
        })
        .count();
    let refused = shared.iter().filter(|l| matches!(l.outcome, Outcome::OutOfM2(..))).count();
    format!("census: -shared: {} lines, {linked} linked, {refused} refused by name", shared.len())
}

/// The table (architecture, variation, outcome: count), then every line
/// that is not `ok`, with its key.
pub fn report(lines: &[Line], notes: &[String], compile_failures: &[String]) -> String {
    let mut s = String::new();
    for n in notes {
        let _ = writeln!(s, "census: note: {n}");
    }
    for c in compile_failures {
        let _ = writeln!(s, "census: FAIL: {c}");
    }
    let mut counts: BTreeMap<(&str, usize, &str), usize> = BTreeMap::new();
    for l in lines {
        let order = VARIATIONS.iter().position(|v| v.name == l.variation).unwrap_or(usize::MAX);
        *counts.entry((l.arch.name(), order, l.outcome.word())).or_default() += 1;
    }
    let _ = writeln!(s, "| arch | variation | outcome | lines |");
    let _ = writeln!(s, "|---|---|---|---|");
    for ((arch, order, word), n) in &counts {
        let name = VARIATIONS.get(*order).map_or("?", |v| v.name);
        let _ = writeln!(s, "| {arch} | {name} | {word} | {n} |");
    }
    for l in lines.iter().filter(|l| l.outcome != Outcome::Ok) {
        let tag = if l.outcome.passes() { "note" } else { "FAIL" };
        let _ = writeln!(
            s,
            "census: {tag}: {} [{}]: {}: {}",
            l.id(),
            l.short,
            l.outcome.word(),
            l.outcome.key()
        );
    }
    s
}

/// The entry point: exit status 0 when every line passes.
pub fn main(args: &[String]) -> u8 {
    match Args::parse(args) {
        Ok(a) => match run_lane(&a) {
            Ok(true) => 0,
            Ok(false) => 1,
            Err(e) => {
                eprintln!("xtask census: {e}");
                1
            }
        },
        Err(e) => {
            eprint!("xtask census: {e}\n{USAGE}");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments() {
        let a = Args::parse(&[
            "--quick".into(),
            "--arch".into(),
            "aarch64,x86_64".into(),
            "-j".into(),
            "3".into(),
        ])
        .unwrap();
        assert!(a.quick && a.jobs == 3);
        assert_eq!(a.arches, [Arch::Aarch64, Arch::X86_64]);
        assert!(Args::parse(&["--bogus".into()]).is_err());
    }

    // The quick subset names variations and drivers that exist.
    #[test]
    fn quick_names_exist() {
        for q in QUICK_VARIATIONS {
            assert!(VARIATIONS.iter().any(|v| v.name == *q), "{q}");
        }
        for k in KNOWN_FINDINGS {
            assert!(VARIATIONS.iter().any(|v| v.name == k.0), "{}", k.0);
        }
        for d in QUICK_DRIVERS {
            let (family, lang) = d.split_once('/').unwrap();
            assert!(["gcc", "clang22", "clang19", "kiln"].contains(&family));
            assert!(["c", "c++", "fortran"].contains(&lang));
        }
    }

    // LT-7: the `-shared` lines are counted, linked and refused by name.
    #[test]
    fn shared_lines_are_counted() {
        let line = |variation, outcome| Line {
            driver: "gcc/aarch64/c".into(),
            short: "gcc/c".into(),
            arch: Arch::Aarch64,
            variation,
            outcome,
        };
        let refused = OUT_OF_M2.iter().find(|(k, _)| k.contains("shared object")).unwrap();
        let lines = [
            line("default", Outcome::Ok),
            line("shared", Outcome::OutOfM2(refused.0, refused.1)),
            line("shared-soname", Outcome::Ok),
            line("shared-ldshared", Outcome::Fail("x".into())),
        ];
        assert_eq!(shared_count(&lines), "census: -shared: 3 lines, 1 linked, 1 refused by name");
        assert!(VARIATIONS.iter().filter(|v| v.is_shared()).all(|v| v.flavour == Flavour::Pic));
        assert!(!QUICK_VARIATIONS.iter().any(|q| q.starts_with("shared-") || *q == "shared"));
    }

    // "The census at `-r`" (RL-6): the `-r` lines are a class of the
    // summary line; since RL-7 a refusal passes by a later package's name.
    #[test]
    fn relocatable_lines_are_counted() {
        let line = |variation, outcome| Line {
            driver: "gcc/aarch64/c".into(),
            short: "gcc/c".into(),
            arch: Arch::Aarch64,
            variation,
            outcome,
        };
        let r = OUT_OF_M4R[0];
        let lines = [
            line("default", Outcome::Ok),
            line("r-plain", Outcome::OutOfM4r(r.0, r.1)),
            line("r-g", Outcome::Fail("x".into())),
            line("no-pie", Outcome::Fail("y".into())),
        ];
        assert_eq!(
            summary(&lines, 3, 2, 1.0),
            "census: 4 lines (2 -r), 3 drivers, 2 pass (1 -r), 2 fail (1 -r), in 1.0 s"
        );
        let names: Vec<&str> =
            VARIATIONS.iter().filter(|v| v.is_relocatable()).map(|v| v.name).collect();
        assert_eq!(names.len(), 13, "{names:?}");
        assert!(names.iter().all(|n| n.starts_with("r-")));
        assert!(
            VARIATIONS.iter().filter(|v| v.name.starts_with("r-")).all(Variation::is_relocatable)
        );
        assert!(Outcome::OutOfM4r(r.0, r.1).passes());
    }

    #[test]
    fn outcomes_pass_or_fail() {
        assert!(Outcome::Ok.passes());
        assert!(Outcome::OutOfM2(OUT_OF_M2[0].0, OUT_OF_M2[0].1).passes());
        assert!(!Outcome::Fail("x".into()).passes());
        let line = Line {
            driver: "gcc/aarch64/c".into(),
            short: "gcc/c".into(),
            arch: Arch::Aarch64,
            variation: "hash-sysv",
            outcome: Outcome::OutOfM2(OUT_OF_M2[0].0, OUT_OF_M2[0].1),
        };
        let text = report(&[line], &[], &[]);
        assert!(
            text.contains("| aarch64 | hash-sysv | refused by name, out of M2 | 1 |"),
            "{text}"
        );
        assert!(text.contains("census: note: gcc/aarch64/c.hash-sysv"), "{text}");
    }
}

//! `cargo xtask sweep`: the byte sweep of `spec/plumb.md` §12.3 on
//! mortar's own dynamic outputs, as a lane (WP-088). Each case compiles a
//! hello program with GCC, links it with mortar behind a `-B` directory
//! whose `ld` is mortar (its map beside the output), and sweeps the
//! output's synthetic sections in-process: every byte flipped by bit 0
//! and by bit 7, each mutant judged by the verifier.
//!
//!   cargo xtask sweep [--quick] [--arch <a>[,<a>]] [--case <c>[,<c>]] [--threads <n>]
//!
//! After each flip the sweep reseals mortar's build id (WP-101: unsealed,
//! every flip was B1's finding and the other rules went unmeasured); the
//! id's own bytes are swept unsealed, and one flip of `e_ident`'s padding is judged
//! unsealed to show B1 still judges the id. The lane fails when the
//! unmutated output has a finding, when a mutant makes the verifier panic
//! (a hostile-input bug, §9.4), when a byte survives (no flip of it makes
//! a new finding) and its field is not in §12.3's table of equivalent
//! fields ([`mortar_plumb::sweep::EQUIVALENT`]), or when the unsealed
//! flip is not B1's. Each case's line prints the flips, those caught (and
//! by which rule), those of equivalent fields and those surviving.
//! The architecture is the host's (AArch64 when the host is neither
//! 64-bit one) unless `--arch` names others: each hosted lane sweeps its
//! own, so the full gate's two lanes cover both targets. `--quick` is
//! what `cargo xtask ci --quick` runs: [`QUICK_CASES`]; the full lane
//! runs every case.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::linktest::target::Arch;
use crate::process::{self, Limits};

pub const USAGE: &str =
    "usage: cargo xtask sweep [--quick] [--arch <a>[,<a>]] [--case <c>[,<c>]] [--threads <n>]\n";

/// The limits of a compile or a link.
const TOOL_LIMITS: Limits = Limits { time: Duration::from_mins(3), memory_kib: 8 << 20 };

const HELLO_C: &str = "#include <stdio.h>\nint main(void){puts(\"hello\");return 0;}\n";
const HELLO_CXX: &str = "#include <iostream>\n#include <string>\n\
                         int main(){std::string s=\"hi\"; std::cout<<s<<std::endl; return 0;}\n";

/// One output of the lane.
struct Case {
    name: &'static str,
    cxx: bool,
    /// The compile's options (besides `-c -O1`).
    compile: &'static [&'static str],
    /// The link's options.
    link: &'static [&'static str],
    /// [`mortar_plumb::sweep::run`]'s window: 0 sweeps every byte; a
    /// static-pie's `.rela.dyn`, `.got` and `.eh_frame_hdr` are swept at
    /// their two ends only (every byte of them is one of a few fields
    /// repeated: 537 s on AArch64 for the whole, measured).
    window: u64,
    /// A test-producer description (`cargo xtask meta`) added to the
    /// object as its `.mason.meta`; the case then compiles `odr.c`
    /// (stage 4, `spec/plumb.md` §13; WP-078).
    meta: Option<&'static str>,
    /// The sections swept: [`mortar_plumb::sweep::SECTIONS`] when empty.
    only: &'static [&'static str],
    /// A library's source (`shared.c`, [`SHARED_C`]) and the files beside
    /// it, instead of the hello program; compiled `-fPIC` with TLS
    /// descriptors (AArch64's `-mtls-dialect=desc`, x86-64's `gnu2`).
    library: bool,
}

/// The source of the stage-4 cases: an exported and a hidden definition.
const ODR_C: &str = "#include <stdio.h>\n\
                     __attribute__((visibility(\"hidden\"))) int helper(void){return puts(\"hello\");}\n\
                     int main(void){return helper() < 0;}\n";

/// Their units: both definitions vague (O-15 bit 0), `helper` also
/// `SYSTEM` (bit 8), and two types.
const ODR_META: &str = "summary name=lang value=c++\n\
    definition anchor=main hash=0123456789abcdef0123456789abcdef class=1 flags=1\n\
    definition anchor=helper hash=fedcba9876543210fedcba9876543210 class=1 flags=0x101\n\
    type name=1S hash=00112233445566778899aabbccddeeff layout=0x55 size=8 align=4 flags=0\n\
    type name=1T hash=ffeeddccbbaa99887766554433221100 layout=0x66 size=16 align=8 flags=1\n";

/// The `Shared` cases' library (WP-161): exports of two version nodes, a
/// symbol the script makes local, an exported TLS definition and a local
/// one (TLS descriptors), a PLT call, and the `-init`/`-fini` functions.
const SHARED_C: &str = "#include <stdio.h>\n\
                        __thread int sw_tls = 1;\n\
                        static __thread int sw_ltls = 2;\n\
                        int sw_data = 3;\n\
                        int sw_hidden(void){return sw_data;}\n\
                        int sw_old(void){return sw_hidden() + 1;}\n\
                        int sw_new(void){return puts(\"hello\") + sw_tls + sw_ltls + sw_old();}\n\
                        void sw_init(void){sw_data++;}\n\
                        void sw_fini(void){sw_data--;}\n";

/// Its version script: two nodes, the second a child of the first.
const SHARED_VER: &str = "SW_1 { global: sw_old; sw_data; sw_tls; local: *; };\n\
                          SW_2 { global: sw_new; sw_init; sw_fini; } SW_1;\n";

/// The units of `shared.table` (WP-164): rows for the two exported
/// functions (one `SYSTEM`), the exported variable, the one the script
/// makes local (no row, O-50) and a type.
const SHARED_META: &str = "summary name=lang value=c++\n\
    definition anchor=sw_old hash=0123456789abcdef0123456789abcdef class=1 flags=1\n\
    definition anchor=sw_new hash=fedcba9876543210fedcba9876543210 class=1 flags=0x101\n\
    definition anchor=sw_data hash=00112233445566778899aabbccddeeff class=2 flags=8\n\
    definition anchor=sw_hidden hash=ffeeddccbbaa99887766554433221100 class=1 flags=1\n\
    type name=1S hash=0f1e2d3c4b5a69788796a5b4c3d2e1f0 layout=0x55 size=8 align=4 flags=0\n";

const CASES: &[Case] = &[
    Case {
        name: "pie.lazy",
        cxx: false,
        compile: &["-fPIE"],
        link: &["-pie"],
        window: 0,
        meta: None,
        only: &[],
        library: false,
    },
    Case {
        name: "pie.now",
        cxx: false,
        compile: &["-fPIE"],
        link: &["-pie", "-Wl,-z,now"],
        window: 0,
        meta: None,
        only: &[],
        library: false,
    },
    // Stage 4: a table (`=exported`, `main` exported by `-rdynamic`), and
    // `=full` with the kept container (`helper` a hidden definition row).
    // Over stage 4's sections only, without a build id (WP-078, written
    // before the sweep resealed one: WP-101).
    Case {
        name: "table",
        cxx: false,
        compile: &["-fPIE"],
        link: &["-pie", "-rdynamic", "-Wl,--odr-table=exported", "-Wl,--build-id=none"],
        window: 0,
        meta: Some(ODR_META),
        only: &[".note.mason"],
        library: false,
    },
    Case {
        name: "table.full",
        cxx: false,
        compile: &["-fPIE"],
        link: &[
            "-pie",
            "-rdynamic",
            "-Wl,--odr-table=full",
            "-Wl,--keep-mason-meta",
            "-Wl,--build-id=none",
        ],
        window: 0,
        meta: Some(ODR_META),
        only: &[".note.mason", ".mason.meta"],
        library: false,
    },
    Case {
        name: "nopie",
        cxx: false,
        compile: &["-fno-PIE"],
        link: &["-no-pie"],
        window: 0,
        meta: None,
        only: &[],
        library: false,
    },
    Case {
        name: "cxx",
        cxx: true,
        compile: &["-fPIE"],
        link: &["-pie"],
        window: 0,
        meta: None,
        only: &[],
        library: false,
    },
    Case {
        name: "spie",
        cxx: false,
        compile: &["-fPIE"],
        link: &["-static-pie"],
        window: 256,
        meta: None,
        only: &[],
        library: false,
    },
    // `.gdb_index` (WP-098), its bytes only; without a build id (WP-098,
    // before the sweep resealed one: WP-101).
    Case {
        name: "gdbindex",
        cxx: false,
        compile: &["-fPIE", "-g", "-ggnu-pubnames"],
        link: &["-pie", "-Wl,--gdb-index", "-Wl,--build-id=none"],
        window: 0,
        meta: None,
        only: &[".gdb_index"],
        library: false,
    },
    // `Shared` (WP-161): a versioned library with TLS descriptors, and the
    // same source unversioned (every default global exported) with the
    // option tail of `-shared` (dyn.md D-102, D-103, D-110).
    Case {
        name: "shared",
        cxx: false,
        compile: &["-fPIC"],
        link: &["-shared", "-Wl,--version-script=shared.ver", "-Wl,-soname,libsw.so"],
        window: 0,
        meta: None,
        only: &[],
        library: true,
    },
    Case {
        name: "shared.tail",
        cxx: false,
        compile: &["-fPIC"],
        link: &[
            "-shared",
            "-Wl,-z,nodelete",
            "-Wl,-z,nodlopen",
            "-Wl,-z,initfirst",
            "-Wl,-z,interpose",
            "-Wl,-z,origin",
            "-Wl,-Bsymbolic",
            "-Wl,-init,sw_init",
            "-Wl,-fini,sw_fini",
            "-Wl,--exclude-libs,ALL",
        ],
        window: 0,
        meta: None,
        only: &[],
        library: true,
    },
    // `Shared` with stage 4's sections (WP-164, O-60 and MT-32): the
    // versioned library's table under `-Bsymbolic-functions` (the two
    // functions' rows NONPREEMPTIBLE, the variable's not), `=full`'s note
    // with the local `sw_hidden`, and the kept container.
    Case {
        name: "shared.table",
        cxx: false,
        compile: &["-fPIC"],
        link: &[
            "-shared",
            "-Wl,--version-script=shared.ver",
            "-Wl,-Bsymbolic-functions",
            "-Wl,--odr-table=full",
            "-Wl,--keep-mason-meta",
            "-Wl,--build-id=none",
        ],
        window: 0,
        meta: Some(SHARED_META),
        only: &[".note.mason", ".mason.meta"],
        library: true,
    },
];

/// The cases of `--quick` (WP-161 added the two `Shared` ones: about 20 s
/// each on one architecture, measured at a load of 20).
pub const QUICK_CASES: &[&str] =
    &["pie.lazy", "nopie", "shared", "shared.tail", "shared.table"];

/// What `cargo xtask sweep` was asked.
#[derive(Debug, Default)]
pub struct Args {
    pub quick: bool,
    pub arches: Vec<Arch>,
    pub threads: Option<usize>,
    /// `--case <name>[,<name>]`: these cases only.
    pub cases: Vec<String>,
}

impl Args {
    /// # Errors
    /// An unknown argument, architecture or a bad number.
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut a = Self::default();
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--quick" => a.quick = true,
                "--arch" => {
                    let list = it.next().ok_or("--arch needs a value")?;
                    for name in list.split(',') {
                        a.arches.push(
                            Arch::parse(name).ok_or_else(|| format!("unknown arch `{name}`"))?,
                        );
                    }
                }
                "--threads" => {
                    let n = it.next().ok_or("--threads needs a value")?;
                    a.threads = Some(
                        n.parse()
                            .ok()
                            .filter(|n| *n > 0)
                            .ok_or_else(|| format!("bad --threads `{n}`"))?,
                    );
                }
                "--case" => {
                    let list = it.next().ok_or("--case needs a value")?;
                    for name in list.split(',') {
                        if !CASES.iter().any(|c| c.name == name) {
                            return Err(format!("unknown case `{name}`"));
                        }
                        a.cases.push(name.to_owned());
                    }
                }
                other => return Err(format!("unknown argument `{other}`")),
            }
        }
        Ok(a)
    }
}

/// `cargo xtask sweep <args>`: 0 when every case is clean, 1 when one is
/// not or the lane cannot run, 2 for bad arguments.
pub fn main(args: &[String]) -> u8 {
    match Args::parse(args) {
        Ok(a) => match run_lane(&a) {
            Ok(true) => 0,
            Ok(false) => 1,
            Err(e) => {
                eprintln!("xtask sweep: {e}");
                1
            }
        },
        Err(e) => {
            eprint!("xtask sweep: {e}\n{USAGE}");
            2
        }
    }
}

/// The architectures of a run: those asked, else the host's (AArch64
/// when the host is none of the two 64-bit ones).
fn arches(a: &Args, host: Option<Arch>) -> Vec<Arch> {
    if !a.arches.is_empty() {
        return a.arches.clone();
    }
    match host {
        Some(h @ (Arch::Aarch64 | Arch::X86_64)) => vec![h],
        _ => vec![Arch::Aarch64],
    }
}

fn run_lane(a: &Args) -> Result<bool, String> {
    let started = Instant::now();
    let host = Arch::host();
    let mortar = crate::linktest::build_mortar()?;
    let threads = a
        .threads
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(2, |n| n.get().min(4)));
    let mut clean = true;
    let mut swept = 0usize;
    for arch in arches(a, host) {
        let work = crate::linktest::build_dir().join("sweep").join(arch.name());
        let bin = work.join("bin");
        std::fs::create_dir_all(&bin).map_err(|e| format!("{}: {e}", bin.display()))?;
        let ld = bin.join("ld");
        let _ = std::fs::remove_file(&ld);
        std::os::unix::fs::symlink(&mortar, &ld).map_err(|e| format!("{}: {e}", ld.display()))?;
        std::fs::write(work.join("hello.c"), HELLO_C).map_err(|e| e.to_string())?;
        std::fs::write(work.join("hello.cc"), HELLO_CXX).map_err(|e| e.to_string())?;
        std::fs::write(work.join("odr.c"), ODR_C).map_err(|e| e.to_string())?;
        std::fs::write(work.join("shared.c"), SHARED_C).map_err(|e| e.to_string())?;
        std::fs::write(work.join("shared.ver"), SHARED_VER).map_err(|e| e.to_string())?;
        let wanted = |c: &&Case| {
            (!a.quick || QUICK_CASES.contains(&c.name))
                && (a.cases.is_empty() || a.cases.iter().any(|n| n == c.name))
        };
        for case in CASES.iter().filter(wanted) {
            let id = format!("{}/{}", arch.name(), case.name);
            let t = Instant::now();
            let verdict = sweep_case(arch, host, case, &work, &bin, threads);
            let secs = t.elapsed().as_secs_f64();
            match verdict {
                Ok((t, problems)) => {
                    swept += t.bytes;
                    eprintln!(
                        "sweep: {id}: {} bytes, {} flips: {} caught, {} equivalent, {} survive{} ({secs:.1} s)",
                        t.bytes,
                        t.flips,
                        t.caught,
                        t.equivalent,
                        t.flips - t.caught - t.equivalent,
                        if problems.is_empty() { "" } else { ": FAIL" }
                    );
                    eprintln!("sweep:   caught by: {}", t.by_rule);
                    if problems.is_empty() {
                        continue;
                    }
                    clean = false;
                    for p in problems {
                        eprintln!("sweep:   {p}");
                    }
                }
                Err(e) => {
                    clean = false;
                    eprintln!("sweep: {id}: FAIL: {e}");
                }
            }
        }
    }
    eprintln!(
        "sweep: {swept} bytes on {threads} thread(s) in {:.1} s: {}",
        started.elapsed().as_secs_f64(),
        if clean { "ok" } else { "FAILED" }
    );
    Ok(clean)
}

/// Builds, links and sweeps one case in `work`: the bytes swept and what
/// fails the lane.
fn sweep_case(
    arch: Arch,
    host: Option<Arch>,
    case: &Case,
    work: &Path,
    bin: &Path,
    threads: usize,
) -> Result<(Tally, Vec<String>), String> {
    let driver_name = arch.gnu_tool(host, if case.cxx { "g++" } else { "gcc" });
    let driver: PathBuf = mortar_testutil::find_listed_tool(&driver_name)
        .ok_or_else(|| format!("`{driver_name}` not found"))?;
    let source = match (case.cxx, case.meta) {
        _ if case.library => "shared.c",
        (_, Some(_)) => "odr.c",
        (true, None) => "hello.cc",
        (false, None) => "hello.c",
    };
    let object = format!("{}.o", case.name);
    let output = case.name.to_owned();
    let map = format!("{}.map", case.name);
    let mut compile: Vec<String> = vec!["-c".into(), "-O1".into()];
    compile.extend(case.compile.iter().map(|s| (*s).to_owned()));
    if case.library {
        let dialect = if arch == Arch::X86_64 { "gnu2" } else { "desc" };
        compile.push(format!("-mtls-dialect={dialect}"));
    }
    compile.extend([source.to_owned(), "-o".into(), object.clone()]);
    tool(&driver, &compile, work)?;
    if let Some(desc) = case.meta {
        let sections = crate::meta::parse(desc)?;
        let plain = work.join(&object);
        let with = work.join(format!("{}.meta.o", case.name));
        crate::meta::add_to(&sections[0], &plain, &with)?;
        std::fs::rename(&with, &plain).map_err(|e| format!("{}: {e}", plain.display()))?;
    }
    let mut link: Vec<String> = vec![format!("-B{}/", bin.display())];
    link.extend(case.link.iter().map(|s| (*s).to_owned()));
    link.extend([object, "-o".into(), output.clone(), format!("-Wl,--mortar-map={map}")]);
    tool(&driver, &link, work)?;
    // The sweep's threads each hold a jobserver token (spec/harness.md
    // "Jobserver"); the first is this process's own.
    let client = crate::jobs::global();
    let tokens: Vec<_> = (0..threads).map(|_| client.acquire()).collect();
    let sweep = crate::plumb::sweep_link(
        &work.join(&output),
        &work.join(&map),
        mortar_plumb::Format::Mortar,
        work,
        threads,
        case.window,
        if case.only.is_empty() { mortar_plumb::sweep::SECTIONS } else { case.only },
    )?;
    drop(tokens);
    // The whole table beside the output, for a reader of the lane's log.
    let table = work.join(format!("{}.sweep.txt", case.name));
    std::fs::write(&table, sweep.render()).map_err(|e| format!("{}: {e}", table.display()))?;
    let mut problems = Vec::new();
    if sweep.baseline > 0 {
        problems.push(format!("the output has {} finding(s) unmutated", sweep.baseline));
    }
    for b in &sweep.bytes {
        if b.panicked {
            problems.push(format!("panic {} +{:#x} {} {}", b.section, b.off, b.kind, b.instance));
        } else if b.caught.iter().any(|c| !c) && mortar_plumb::sweep::equivalent(b).is_none() {
            problems
                .push(format!("survivor {} +{:#x} {} {}", b.section, b.off, b.kind, b.instance));
        }
    }
    if let Some(u) = &sweep.unsealed
        && !u.iter().any(|r| r == "B1")
    {
        problems.push(format!("a flip left unsealed is not B1's (its rules: {u:?})"));
    }
    Ok((tally(&sweep), problems))
}

/// What a case's line reports: bytes, flips, flips caught by each rule,
/// flips of fields named equivalent.
#[derive(Debug, Default)]
struct Tally {
    bytes: usize,
    flips: usize,
    caught: usize,
    equivalent: usize,
    by_rule: String,
}

fn tally(sweep: &mortar_plumb::sweep::Sweep) -> Tally {
    let flips = sweep.bytes.len() * mortar_plumb::sweep::FLIPS.len();
    let caught = sweep.bytes.iter().flat_map(|b| b.caught).filter(|c| *c).count();
    let equivalent = sweep
        .bytes
        .iter()
        .filter(|b| mortar_plumb::sweep::equivalent(b).is_some())
        .map(|b| b.caught.iter().filter(|c| !**c).count())
        .sum();
    let by_rule = sweep
        .by_rule()
        .iter()
        .map(|(r, (any, _))| format!("{r} {any}"))
        .collect::<Vec<_>>()
        .join(", ");
    Tally { bytes: sweep.bytes.len(), flips, caught, equivalent, by_rule }
}

/// Runs `program args` in `dir`; an error unless it exits 0.
fn tool(program: &Path, args: &[String], dir: &Path) -> Result<(), String> {
    let o = process::run(
        program,
        |c| {
            c.args(args).current_dir(dir).env("LC_ALL", "C");
        },
        b"",
        TOOL_LIMITS,
    )?;
    if o.end.success() {
        Ok(())
    } else {
        Err(format!(
            "{} {}: {}: {}",
            program.display(),
            args.join(" "),
            o.end,
            o.first_error_line()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments() {
        let a = Args::parse(&["--quick".into(), "--arch".into(), "x86_64".into()]).unwrap();
        assert!(a.quick && a.arches == [Arch::X86_64] && a.threads.is_none());
        assert_eq!(Args::parse(&["--threads".into(), "3".into()]).unwrap().threads, Some(3));
        assert!(Args::parse(&["--threads".into(), "0".into()]).is_err());
        assert!(Args::parse(&["--bogus".into()]).is_err());
        assert_eq!(Args::parse(&["--case".into(), "gdbindex".into()]).unwrap().cases, ["gdbindex"]);
        assert!(Args::parse(&["--case".into(), "nope".into()]).is_err());
    }

    // The host's architecture unless `--arch` names others.
    #[test]
    fn architectures() {
        let quick = Args { quick: true, ..Args::default() };
        assert_eq!(arches(&quick, Some(Arch::X86_64)), [Arch::X86_64]);
        assert_eq!(arches(&quick, Some(Arch::I386)), [Arch::Aarch64]);
        assert_eq!(arches(&Args::default(), Some(Arch::Aarch64)), [Arch::Aarch64]);
        let both = Args { arches: vec![Arch::Aarch64, Arch::X86_64], ..Args::default() };
        assert_eq!(arches(&both, None), [Arch::Aarch64, Arch::X86_64]);
    }

    #[test]
    fn quick_cases_exist() {
        for q in QUICK_CASES {
            assert!(CASES.iter().any(|c| c.name == *q), "{q}");
        }
    }
}

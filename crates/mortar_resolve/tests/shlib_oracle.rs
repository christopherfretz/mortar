//! Resolution of real dynamic link lines against GNU ld's (`spec/dyn.md`
//! §2.1, D-11 to D-21; WP-043's oracle).
//!
//! Each line is run once through the compiler driver with GNU ld behind a
//! recording wrapper (`-B<dir>/ld`): the wrapper keeps the driver's linker
//! command line, GNU ld's exit status, its `DT_NEEDED` list and the
//! symbols its `.dynsym` defines. The same command line then goes through
//! mortar's parser, loader and resolution (`mortar_cli::parse`,
//! `mortar_input::load::load`, `mortar_resolve::resolve_inputs`), and the
//! verdict, the needed list in order and the export set are compared.

#![allow(clippy::too_many_lines, reason = "tables of link lines")]

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mortar_core::FileId;
use mortar_core::par::Pool;
use mortar_testutil::{Scratch, run_with, tool, tools_required};

/// A target: the driver prefix and GNU ld for it.
#[derive(Clone, Copy, Debug)]
pub struct Arch {
    pub name: &'static str,
    /// Prefix of the cross drivers (empty for the host's).
    pub prefix: &'static str,
    pub ld: &'static str,
}

pub const AARCH64: Arch = Arch { name: "aarch64", prefix: "aarch64-linux-gnu-", ld: "ld.bfd" };
pub const X86_64: Arch =
    Arch { name: "x86_64", prefix: "x86_64-linux-gnu-", ld: "x86_64-linux-gnu-ld.bfd" };

/// What GNU ld did with a line.
#[derive(Debug, Clone)]
pub struct Gnu {
    /// The linker's arguments (the driver's collect2 line).
    pub args: Vec<String>,
    pub ok: bool,
    pub stderr: String,
    /// `DT_NEEDED`, in order.
    pub needed: Vec<String>,
    /// The names `.dynsym` defines (exports), without versions.
    pub exported: BTreeSet<String>,
}

/// What mortar's resolution did with the same line.
#[derive(Debug, Clone)]
pub struct Mortar {
    /// Error diagnostics: (id, rendered).
    pub errors: Vec<(String, String)>,
    pub warnings: Vec<String>,
    /// The needed objects' sonames (`DT_NEEDED` names), in order.
    pub needed: Vec<String>,
    pub exported: BTreeSet<String>,
    /// Keys bound to a shared definition that a relocatable object
    /// mentions (D-21 item 1): what GNU ld copies shows as a definition.
    pub imported: BTreeSet<String>,
    /// Linker-defined keys (file 0, D-21 item 2).
    pub internal: BTreeSet<String>,
}

impl Mortar {
    fn refused(errors: Vec<(String, String)>, warnings: Vec<String>) -> Self {
        Self {
            errors,
            warnings,
            needed: Vec::new(),
            exported: BTreeSet::new(),
            imported: BTreeSet::new(),
            internal: BTreeSet::new(),
        }
    }

    /// GNU ld's `.dynsym` definitions less the deliberate differences of
    /// D-21: a copy of an imported key (item 3, the scan's, D-42) and a
    /// linker-defined key no shared object refers to (item 2).
    pub fn gnu_exports(&self, gnu: &BTreeSet<String>) -> BTreeSet<String> {
        gnu.iter()
            .filter(|n| !self.imported.contains(*n))
            .filter(|n| {
                let linker = self.internal.contains(*n)
                    || mortar_resolve::linker::TABLE.iter().any(|r| r.name == n.as_bytes());
                !linker || self.exported.contains(*n)
            })
            .cloned()
            .collect()
    }
}

fn readelf() -> Option<PathBuf> {
    tool("readelf")
}

fn output(cmd: &mut Command) -> String {
    let out = run_with(cmd, b"", Duration::from_mins(2));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// `DT_NEEDED` names of `path`, and its `DT_SONAME`.
fn dynamic_names(path: &Path) -> (Vec<String>, Option<String>) {
    let text = output(Command::new(readelf().unwrap()).arg("-dW").arg(path));
    let mut needed = Vec::new();
    let mut soname = None;
    for line in text.lines() {
        let name = line.split('[').nth(1).and_then(|s| s.split(']').next()).map(str::to_owned);
        if line.contains("(NEEDED)") {
            needed.extend(name);
        } else if line.contains("(SONAME)") {
            soname = name;
        }
    }
    (needed, soname)
}

/// The names `.dynsym` of `path` defines.
fn dynsym_defs(path: &Path) -> BTreeSet<String> {
    let text = output(Command::new(readelf().unwrap()).args(["--dyn-syms", "-W"]).arg(path));
    let mut out = BTreeSet::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 8
            || f[0].trim_end_matches(':').parse::<u32>().is_err()
            || f[6] == "UND"
            || f[4] == "LOCAL"
        {
            continue;
        }
        if f[3] == "SECTION" || f[3] == "FILE" {
            continue;
        }
        let name = f[7];
        let name = name.split('@').next().unwrap_or(name);
        out.insert(name.to_owned());
    }
    out
}

/// The recording wrapper for `arch` in `dir`/`<arch>-bin/ld`.
fn wrapper(dir: &Path, arch: Arch) -> PathBuf {
    let bin = dir.join(format!("{}-bin", arch.name));
    std::fs::create_dir_all(&bin).unwrap();
    let ld = tool(arch.ld).expect("GNU ld");
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$MORTAR_ORACLE_ARGS\"\nexec {} \"$@\"\n",
        ld.display()
    );
    let path = bin.join("ld");
    std::fs::write(&path, script).unwrap();
    let mut perm = std::fs::metadata(&path).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o755);
    std::fs::set_permissions(&path, perm).unwrap();
    bin
}

/// Runs `driver args` in `cwd` with GNU ld behind the wrapper; `out` is
/// the output's name (made fresh).
pub fn record(arch: Arch, driver: &str, args: &[&str], cwd: &Path, out: &str) -> Gnu {
    let bin = wrapper(cwd, arch);
    let argfile = cwd.join(format!("{out}.args"));
    let exe = cwd.join(out);
    let _ = std::fs::remove_file(&exe);
    let mut cmd = driver_cmd(arch, driver);
    cmd.current_dir(cwd).env("MORTAR_ORACLE_ARGS", &argfile).env("TMPDIR", cwd);
    cmd.arg(format!("-B{}/", bin.display())).args(args).arg("-o").arg(&exe);
    let o = run_with(&mut cmd, b"", Duration::from_mins(2));
    let args: Vec<String> =
        std::fs::read_to_string(&argfile).unwrap_or_default().lines().map(str::to_owned).collect();
    let ok = o.success();
    let (needed, exported) =
        if ok { (dynamic_names(&exe).0, dynsym_defs(&exe)) } else { (Vec::new(), BTreeSet::new()) };
    Gnu { args, ok, stderr: o.stderr_text(), needed, exported }
}

/// The driver command for `arch`: `gcc`, `g++`, `gfortran` through their
/// cross names, `clang`/`clang++` with `--target`.
pub fn driver_cmd(arch: Arch, driver: &str) -> Command {
    if driver.starts_with("clang") {
        let mut c = Command::new(tool(driver).expect("clang"));
        c.arg(format!("--target={}-linux-gnu", arch.name));
        c
    } else {
        Command::new(tool(&format!("{}{driver}", arch.prefix)).expect("driver"))
    }
}

/// Compiles `src` to `obj` in `cwd` with `driver`.
pub fn compile(arch: Arch, driver: &str, cwd: &Path, src: &str, obj: &str) {
    let mut cmd = driver_cmd(arch, driver);
    cmd.current_dir(cwd).env("TMPDIR", cwd).args(["-c", src, "-o", obj]);
    let o = run_with(&mut cmd, b"", Duration::from_mins(2));
    assert!(o.success(), "{driver} -c {src}: {}", o.stderr_text());
}

/// The linker arguments mortar is given: GNU ld's less the plugin.
fn mortar_args(args: &[String]) -> Vec<OsString> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "-plugin" {
            i += 2;
            continue;
        }
        if a.starts_with("-plugin-opt") {
            i += 1;
            continue;
        }
        out.push(OsString::from(a));
        i += 1;
    }
    out
}

/// Resolves GNU ld's command line with mortar, in `cwd`.
pub fn resolve_line(
    args: &[String],
    cwd: &Path,
    sonames: &mut BTreeMap<PathBuf, String>,
) -> Mortar {
    let argv = mortar_args(args);
    let parsed = mortar_cli::parse(&argv);
    let mut errors: Vec<(String, String)> = Vec::new();
    let mut warnings = Vec::new();
    for (_, d) in &parsed.diagnostics {
        if d.severity() == mortar_diag::Severity::Error {
            errors.push((d.id().to_owned(), format!("{d:?}")));
        }
    }
    if !errors.is_empty() {
        return Mortar::refused(errors, warnings);
    }
    let plan = parsed.plan;
    let store = mortar_input::store::Store::new(mortar_input::store::StorePolicy::Resident);
    let pool = Pool::new(4);
    let diag = mortar_diag::Collector::new(mortar_diag::Policy::default());
    let env = mortar_input::load::LoadEnv { cwd: cwd.to_path_buf(), pool };
    let Some(inputs) = mortar_input::load::load(&plan, &env, &store, &diag) else {
        for d in diag.finish().diagnostics() {
            if d.severity() == mortar_diag::Severity::Error {
                errors.push((d.id().to_owned(), format!("{d:?}")));
            }
        }
        if errors.is_empty() {
            errors.push(("load".to_owned(), "load gave no inputs".to_owned()));
        }
        return Mortar::refused(errors, warnings);
    };
    for d in diag.finish().diagnostics() {
        if d.severity() == mortar_diag::Severity::Error {
            errors.push((d.id().to_owned(), format!("{d:?}")));
        } else {
            warnings.push(format!("{d:?}"));
        }
    }
    let out = mortar_resolve::resolve_inputs(&inputs, &plan, &pool);
    for (_, d) in &out.diagnostics {
        if d.severity() == mortar_diag::Severity::Error {
            errors.push((d.id().to_owned(), format!("{d:?}")));
        } else {
            warnings.push(format!("{} {d:?}", d.id()));
        }
    }
    let path_of = |f: FileId| -> PathBuf {
        let p = String::from_utf8_lossy(inputs.files[f].path).into_owned();
        let p = PathBuf::from(p);
        if p.is_absolute() { p } else { cwd.join(p) }
    };
    let needed = out
        .needed
        .iter()
        .map(|&f| {
            let p = path_of(f);
            sonames
                .entry(p.clone())
                .or_insert_with(|| {
                    dynamic_names(&p)
                        .1
                        .unwrap_or_else(|| p.file_name().unwrap().to_string_lossy().into_owned())
                })
                .clone()
        })
        .collect();
    let name = |k| String::from_utf8_lossy(inputs.symbols[k].name).into_owned();
    let exported = out.resolution.exported.iter().map(name).collect();
    let imported = out.resolution.imported.iter().map(name).collect();
    let internal = out
        .resolution
        .def
        .iter()
        .filter(|(_, d)| matches!(d, mortar_core::resolution::Def::Internal(_)))
        .map(|(k, _)| name(k))
        .collect();
    Mortar { errors, warnings, needed, exported, imported, internal }
}

/// Whether the oracle can run here (drivers, GNU ld, readelf).
pub fn tools(arch: Arch, drivers: &[&str]) -> bool {
    let mut names: Vec<String> =
        drivers
            .iter()
            .map(|d| {
                if d.starts_with("clang") { (*d).to_owned() } else { format!("{}{d}", arch.prefix) }
            })
            .collect();
    names.push(arch.ld.to_owned());
    names.push("readelf".to_owned());
    let missing: Vec<&String> =
        names.iter().filter(|n| mortar_testutil::find_tool(n).is_none()).collect();
    if !missing.is_empty() {
        assert!(!tools_required(), "required tools missing: {missing:?}");
        return false;
    }
    true
}

const HELLO_C: &str = "#include <stdio.h>\nint main(void){puts(\"hi\");return 0;}\n";
const HELLO_CC: &str = "#include <iostream>\n#include <string>\nint main(){std::string s=\"hi\"; \
                        try { throw 1; } catch (int) {} std::cout<<s<<std::endl;}\n";
const HELLO_F: &str = "program h\n  print *, \"hi\"\nend program h\n";

/// One default line of `spec/dyn.md` §2.1.
fn hello(arch: Arch, driver: &str, extra: &[&str]) {
    let drivers: &[&str] = &[driver];
    if !tools(arch, drivers) {
        return;
    }
    let dir = Scratch::new("shlib-hello");
    let (src, file) = match driver {
        "g++" | "clang++" => (HELLO_CC, "h.cc"),
        "gfortran" => (HELLO_F, "h.f90"),
        _ => (HELLO_C, "h.c"),
    };
    dir.write(file, src);
    compile(arch, driver, dir.path(), file, "h.o");
    let mut args: Vec<&str> = vec!["h.o"];
    args.extend_from_slice(extra);
    let g = record(arch, driver, &args, dir.path(), "h");
    assert!(g.ok, "GNU ld: {}", g.stderr);
    let mut sonames = BTreeMap::new();
    let m = resolve_line(&g.args, dir.path(), &mut sonames);
    eprintln!("{} {driver} {extra:?}: needed {:?} exported {:?}", arch.name, m.needed, m.exported);
    assert!(m.errors.is_empty(), "{:#?}\nwarnings {:#?}", m.errors, m.warnings);
    assert_eq!(m.needed, g.needed, "DT_NEEDED");
    assert_eq!(m.exported, m.gnu_exports(&g.exported), "exports");
}

fn default_lines(arch: Arch) {
    for driver in ["gcc", "g++", "gfortran", "clang", "clang++"] {
        hello(arch, driver, &[]);
        hello(arch, driver, &["-no-pie"]);
    }
    hello(arch, "gcc", &["-Wl,-z,now"]);
    hello(arch, "gcc", &["-rdynamic"]);
    hello(arch, "g++", &["-static-libstdc++"]);
    hello(arch, "g++", &["-static-libgcc"]);
}

#[test]
fn d2_1_default_lines_aarch64() {
    default_lines(AARCH64);
}

#[test]
fn d2_1_default_lines_x86_64() {
    default_lines(X86_64);
}

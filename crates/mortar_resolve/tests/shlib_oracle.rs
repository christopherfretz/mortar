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
    /// The keys bound to a shared definition: name to `bind(k)`'s needed
    /// name (D-11).
    pub bind: BTreeMap<String, String>,
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
            bind: BTreeMap::new(),
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
    parse_dynsym(&output(Command::new(readelf().unwrap()).args(["--dyn-syms", "-W"]).arg(path)))
}

/// The defined names of `readelf --dyn-syms -W` output.
fn parse_dynsym(text: &str) -> BTreeSet<String> {
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
    cmd.current_dir(cwd)
        .env("MORTAR_ORACLE_ARGS", &argfile)
        .env("TMPDIR", cwd)
        .env_remove("LD_LIBRARY_PATH")
        .env_remove("LD_RUN_PATH");
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
    let line = mortar_args(args);
    let parsed = mortar_cli::parse(&line);
    let mut errors: Vec<(String, String)> = Vec::new();
    let mut warnings = Vec::new();
    // The CLI refuses the dynamic options until their packages land
    // (WP-042, WP-056); of its refusals only the permanent one of this
    // area counts: `--copy-dt-needed-entries` (D-15).
    let refusals: Vec<(_, mortar_diag::Diagnostic)> = parsed
        .refusals()
        .into_iter()
        .filter(|(_, d)| {
            let text = format!("{d:?}");
            text.contains("copy-dt-needed") || text.contains("add-needed")
        })
        .collect();
    for (_, d) in parsed.diagnostics.iter().chain(&refusals) {
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
    let mut needed_name = |f: FileId| -> String {
        let p = path_of(f);
        let soname =
            sonames.entry(p.clone()).or_insert_with(|| dynamic_names(&p).1.unwrap_or_default());
        if !soname.is_empty() {
            return soname.clone();
        }
        // input §8.3: without a soname, the path as written, or the
        // candidate's file name for `-l`.
        match inputs.files[f].origin {
            mortar_core::inputs::Origin::Input { input, entry: None, .. } => {
                match &plan.inputs[input as usize].spec {
                    mortar_core::plan::InputSpec::Path(w) => w.to_string_lossy().into_owned(),
                    _ => p.file_name().unwrap().to_string_lossy().into_owned(),
                }
            }
            _ => "<sonameless>".to_owned(),
        }
    };
    let needed: Vec<String> = out.needed.iter().map(|&f| needed_name(f)).collect();
    let name = |k| String::from_utf8_lossy(inputs.symbols[k].name).into_owned();
    // The scan reports `undefined-symbol` for the keys whose verdict is an
    // error (resolve §5.5): the link fails there.
    for (k, p) in out.resolution.props.iter() {
        if p.verdict == mortar_core::resolution::Verdict::Error {
            let closure = out.closure_only.iter().find(|c| c.0 == k).map(|c| c.1);
            errors.push((
                "undefined-symbol".to_owned(),
                format!(
                    "(scan) {}{}",
                    name(k),
                    closure.map_or(String::new(), |f| format!(", defined in closure file {f:?}"))
                ),
            ));
        }
    }
    let exported = out.resolution.exported.iter().map(name).collect();
    let imported = out.resolution.imported.iter().map(name).collect();
    let internal = out
        .resolution
        .def
        .iter()
        .filter(|(_, d)| matches!(d, mortar_core::resolution::Def::Internal(_)))
        .map(|(k, _)| name(k))
        .collect();
    let bind = out
        .resolution
        .def
        .iter()
        .filter_map(|(k, d)| match d {
            mortar_core::resolution::Def::Shared(e) => Some((name(k), needed_name(e.file))),
            _ => None,
        })
        .collect();
    Mortar { errors, warnings, needed, exported, imported, internal, bind }
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

/// How mortar's resolution of one line compares with GNU ld's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Same verdict, same `DT_NEEDED`, same exports (less D-21's pinned
    /// differences).
    Agree,
    /// GNU ld refuses, mortar resolves (with mortar's `DT_NEEDED`).
    GnuRefuses(Vec<String>),
    /// mortar refuses (ids), GNU ld links.
    MortarRefuses(Vec<String>),
    /// Both link, `DT_NEEDED` differs: (GNU ld, mortar).
    Needed(Vec<String>, Vec<String>),
    /// Both link, exports differ: (only GNU ld, only mortar).
    Exports(Vec<String>, Vec<String>),
}

/// Compares one recorded line.
pub fn judge(g: &Gnu, m: &Mortar) -> Verdict {
    let refused = !m.errors.is_empty();
    match (g.ok, refused) {
        (false, true) => Verdict::Agree,
        (false, false) => Verdict::GnuRefuses(m.needed.clone()),
        (true, true) => Verdict::MortarRefuses(m.errors.iter().map(|e| e.0.clone()).collect()),
        (true, false) => {
            if g.needed != m.needed {
                return Verdict::Needed(g.needed.clone(), m.needed.clone());
            }
            let want = m.gnu_exports(&g.exported);
            if want != m.exported {
                return Verdict::Exports(
                    want.difference(&m.exported).cloned().collect(),
                    m.exported.difference(&want).cloned().collect(),
                );
            }
            Verdict::Agree
        }
    }
}

/// Splits a mini line (`dynamic/needed_order.sh`): a token starting with
/// `-` other than `-l`/`-L` goes to the linker (`-Wl,`).
fn expand(name: &str, line: &str) -> Vec<String> {
    let mut out = Vec::new();
    if !name.starts_with("drv_") {
        out.push("-Wl,--no-as-needed".to_owned());
    }
    for t in line.split_whitespace() {
        if t.starts_with('-') && !t.starts_with("-l") && !t.starts_with("-L") {
            out.push(format!("-Wl,{t}"));
        } else {
            out.push(t.to_owned());
        }
    }
    out
}

/// Runs `script CC` in `dir` (the inputs of an experiment).
fn build_inputs(arch: Arch, dir: &Path, script: &str) {
    let cc = format!("{}gcc", arch.prefix);
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/oracle").join(script);
    let mut cmd = Command::new("bash");
    cmd.current_dir(dir).env("TMPDIR", dir).arg(&path).arg(tool(&cc).unwrap());
    let o = run_with(&mut cmd, b"", Duration::from_mins(5));
    assert!(o.success(), "{script}: {}", o.stderr_text());
}

/// `dynamic/needed_order.sh`'s cases (F5): name, mini line.
const NEEDED_ORDER: &[(&str, &str)] = &[
    (
        "drv_f5_review",
        "main_abd.o libna.so --as-needed libnc.so --no-as-needed libnb.so libnc.so libnd.so",
    ),
    (
        "f5_review_unref",
        "main_abd.o libna.so --as-needed libnc.so --no-as-needed libnb.so libnc.so libnd.so",
    ),
    ("drv_obj_after_all", "libnd.so libna.so libnc.so libnb.so libnf.so libne.so main.o"),
    (
        "drv_obj_mid",
        "libnd.so libna.so main.o libnc.so libnb.so libnf.so libne.so libna.so libnd.so",
    ),
    ("base_plain", "main.o libna.so libnb.so libnc.so libnd.so libne.so libnf.so"),
    (
        "f5_review",
        "main.o libna.so --as-needed libnc.so --no-as-needed libnb.so libnc.so libnd.so libne.so libnf.so",
    ),
    ("twice_plain", "main.o libna.so libnc.so libnb.so libnc.so libnd.so libne.so libnf.so"),
    (
        "plain_then_asn",
        "main.o libna.so libnc.so libnb.so --as-needed libnc.so --no-as-needed libnd.so libne.so libnf.so",
    ),
    (
        "asn_then_plain",
        "main.o libna.so --as-needed libnc.so --no-as-needed libnb.so libnc.so libnd.so libne.so libnf.so",
    ),
    (
        "asn_twice_ref",
        "main.o libna.so --as-needed libnc.so --no-as-needed libnb.so --as-needed libnc.so --no-as-needed libnd.so libne.so libnf.so",
    ),
    (
        "asn_unref_then_plain",
        "main.o libna.so --as-needed libnx.so --no-as-needed libnb.so libnx.so libnc.so libnd.so libne.so libnf.so",
    ),
    (
        "asn_unref_twice",
        "main.o libna.so --as-needed libnx.so --no-as-needed libnb.so --as-needed libnx.so --no-as-needed libnc.so libnd.so libne.so libnf.so",
    ),
    ("all_asn_ref", "main.o --as-needed libnf.so libne.so libnd.so libnc.so libnb.so libna.so"),
    (
        "three_a_p_a",
        "main.o libna.so --as-needed libnc.so --no-as-needed libnb.so libnc.so libnd.so --as-needed libnc.so --no-as-needed libne.so libnf.so",
    ),
    (
        "three_a_a_p",
        "main.o libna.so --as-needed libnc.so --no-as-needed libnb.so --as-needed libnc.so --no-as-needed libnd.so libnc.so libne.so libnf.so",
    ),
    (
        "three_p_a_p",
        "main.o libna.so libnc.so libnb.so --as-needed libnc.so --no-as-needed libnd.so libnc.so libne.so libnf.so",
    ),
    (
        "three_a_p_p",
        "main.o libna.so --as-needed libnc.so --no-as-needed libnb.so libnc.so libnd.so libnc.so libne.so libnf.so",
    ),
    (
        "two_libs_swap",
        "main.o --as-needed libnc.so libnd.so --no-as-needed libna.so libnd.so libnb.so libnc.so libne.so libnf.so",
    ),
    (
        "obj_after_asn",
        "--as-needed libnc.so --no-as-needed libna.so libnb.so libnd.so libne.so libnf.so main.o",
    ),
    (
        "obj_after_asn_plain",
        "--as-needed libnc.so --no-as-needed libna.so libnb.so main.o libnc.so libnd.so libne.so libnf.so",
    ),
    (
        "obj_between",
        "libna.so --as-needed libnc.so --no-as-needed main.o libnb.so libnc.so libnd.so libne.so libnf.so",
    ),
    ("obj_after_all", "libnd.so libna.so libnc.so libnb.so libnf.so libne.so main.o"),
    (
        "asn_unref_obj_asn",
        "--as-needed libnc.so --no-as-needed libna.so libnb.so --as-needed main.o libnc.so --no-as-needed libnd.so libne.so libnf.so",
    ),
    (
        "asn_unref_obj_asn2",
        "--as-needed libnc.so libnd.so --no-as-needed libna.so main.o libnb.so --as-needed libnd.so libnc.so --no-as-needed libne.so libnf.so",
    ),
    (
        "pushpop",
        "main.o libna.so --push-state --as-needed libnc.so --pop-state libnb.so libnc.so libnd.so libne.so libnf.so",
    ),
    (
        "pushpop_nested",
        "main.o --push-state --as-needed libna.so --push-state --no-as-needed libnc.so --pop-state libnb.so --pop-state libnd.so libne.so libnf.so",
    ),
    ("l_and_path", "-L. main.o libna.so -lnc libnb.so ./libnc.so libnd.so libne.so libnf.so"),
    (
        "l_asn_then_path",
        "-L. main.o libna.so --as-needed -lnc --no-as-needed libnb.so ./libnc.so libnd.so libne.so libnf.so",
    ),
    (
        "two_files_soname",
        "main.o libna.so d1/libnc.so libnb.so d2/libnc.so libnd.so libne.so libnf.so",
    ),
    (
        "two_files_asn_first",
        "main.o libna.so --as-needed d1/libnc.so --no-as-needed libnb.so d2/libnc.so libnd.so libne.so libnf.so",
    ),
    (
        "two_files_asn_second",
        "main.o libna.so d1/libnc.so libnb.so --as-needed d2/libnc.so --no-as-needed libnd.so libne.so libnf.so",
    ),
    (
        "bstatic_first",
        "-L. main.o libna.so -Bstatic -lnc -Bdynamic libnb.so -lnc libnd.so libne.so libnf.so",
    ),
    (
        "bstatic_second",
        "-L. main.o libna.so -lnc libnb.so -Bstatic -lnc -Bdynamic libnd.so libne.so libnf.so",
    ),
    (
        "bstatic_asn",
        "-L. main.o libna.so --as-needed -Bstatic -lnc -Bdynamic -lnc --no-as-needed libnb.so libnd.so libne.so libnf.so",
    ),
    ("scr_group", "main.o libnd.so s_grp_ab.so libnc.so libne.so libnf.so"),
    ("scr_input", "main.o libna.so s_inp_cd.so libnb.so libne.so libnf.so"),
    ("scr_before_plain", "main.o s_inp_cd.so libna.so libnb.so libnc.so libne.so libnf.so"),
    ("scr_after_plain", "main.o libna.so libnc.so libnb.so s_inp_cd.so libne.so libnf.so"),
    ("scr_asn_in_group", "main.o libna.so s_grp_d_asn_cx.so libnb.so libne.so libnf.so"),
    ("scr_asn_then_plain", "main.o libna.so s_grp_d_asn_cx.so libnb.so libnc.so libne.so libnf.so"),
    (
        "scr_asn_unref_then_plain",
        "main.o libna.so s_grp_d_asn_cx.so libnb.so libnx.so libnc.so libne.so libnf.so",
    ),
    (
        "scr_inp_asn_then_plain",
        "main.o libna.so s_inp_asn_c.so libnb.so libnc.so libnd.so libne.so libnf.so",
    ),
    (
        "scr_plain_then_inp_asn",
        "main.o libna.so libnc.so libnb.so s_inp_asn_c.so libnd.so libne.so libnf.so",
    ),
    (
        "scr_asn_flag_then_script",
        "main.o libna.so --as-needed libnc.so --no-as-needed libnb.so s_inp_cd.so libne.so libnf.so",
    ),
    (
        "scr_under_asn_flag",
        "main.o libna.so --as-needed s_inp_cd.so --no-as-needed libnb.so libnc.so libne.so libnf.so",
    ),
    (
        "scr_under_asn_flag_unref",
        "main_abd.o libna.so --as-needed s_inp_cd.so --no-as-needed libnb.so libnc.so",
    ),
    (
        "scr_under_asn_then_flag",
        "main_abd.o libna.so --as-needed s_nest.so --no-as-needed libnc.so",
    ),
    ("scr_nested", "main.o libna.so s_nest.so libnc.so libne.so libnf.so"),
    ("scr_nested2", "main.o libna.so libnb.so s_nest2.so libnc.so"),
    ("scr_inp_l", "-L. main.o libna.so s_inp_lc.so libnb.so libnc.so libnd.so libne.so libnf.so"),
    ("grp_rescan_script", "main_abd.o libna.so s_grp_rescan.so libnb.so libnd.so"),
    (
        "grp_rescan_flags",
        "main_abd.o libna.so --start-group --as-needed libnc.so --no-as-needed usec.o libnd.so --end-group libnb.so",
    ),
    (
        "grp_rescan_time_vs_pos",
        "main_abd.o libna.so libnb.so --start-group --as-needed libnc.so libnd.so --no-as-needed usec.o --end-group",
    ),
    (
        "grp_no_group",
        "main_abd.o libna.so --as-needed libnc.so --no-as-needed usec.o libnd.so libnb.so",
    ),
    (
        "grp_rescan_later_plain",
        "main_abd.o libna.so --start-group --as-needed libnc.so --no-as-needed usec.o libnd.so --end-group libnb.so libnc.so",
    ),
    ("plain_unref", "main.o libna.so libnx.so libnb.so libnc.so libnd.so libne.so libnf.so"),
    ("weak_ref_asn", "main_wk.o libna.so --as-needed libnc.so --no-as-needed libnb.so"),
    (
        "weak_ref_asn_plain",
        "main_wk.o libna.so --as-needed libnc.so --no-as-needed libnb.so libnc.so",
    ),
    (
        "archive_path_then_so",
        "main.o libna.so libnc.a libnb.so libnc.so libnd.so libne.so libnf.so",
    ),
    (
        "two_files_asn_unref_first",
        "main_abd.o libna.so --as-needed d1/libnc.so --no-as-needed libnb.so d2/libnc.so libnd.so",
    ),
    ("nosoname_spellings", "-L. main_z.o libnz.so libna.so -lnz ./libnz.so"),
    ("nosoname_asn_then", "-L. main_z.o libna.so --as-needed ./libnz.so --no-as-needed libnz.so"),
    (
        "under_asn_before",
        "mainu.o libna.so libnb.so libnc.so libnu.so --as-needed libny.so --no-as-needed",
    ),
    (
        "under_asn_first",
        "mainu.o libna.so --as-needed libny.so --no-as-needed libnb.so libnc.so libnu.so",
    ),
    (
        "under_asn_first2",
        "mainu.o libna.so --as-needed libny.so --no-as-needed libnb.so libnu.so libnc.so",
    ),
    (
        "under_asn_first_then_plain",
        "mainu.o libna.so --as-needed libny.so --no-as-needed libnb.so libnu.so libnc.so libny.so",
    ),
    (
        "under_asn_both",
        "mainu.o --as-needed libna.so libny.so libnb.so libnu.so libnc.so --no-as-needed",
    ),
    (
        "under_asn_both_rev",
        "mainu.o --as-needed libnu.so libna.so libny.so libnb.so libnc.so --no-as-needed",
    ),
    (
        "under_asn_mid",
        "mainu.o libna.so libnu.so --as-needed libny.so --no-as-needed libnb.so libnc.so",
    ),
    (
        "under_asn_first_allow",
        "mainu.o libna.so --as-needed libny.so --no-as-needed libnb.so libnc.so libnu.so --allow-shlib-undefined",
    ),
    (
        "under_asn_first_asn_again",
        "mainu.o libna.so --as-needed libny.so --no-as-needed libnb.so libnu.so libnc.so --as-needed libny.so --no-as-needed",
    ),
    ("under_unref_lib", "mainw.o libna.so --as-needed libnx.so --no-as-needed libnb.so libnw.so"),
    (
        "properly_linked_asn",
        "mainv.o libna.so --as-needed libny.so --no-as-needed libnb.so libnv.so",
    ),
    (
        "properly_linked_asn_after",
        "mainv.o libna.so libnb.so libnv.so --as-needed libny.so --no-as-needed",
    ),
    (
        "main_refs_y_after_v",
        "mainy.o libna.so libnb.so libnv.so --as-needed libny.so --no-as-needed",
    ),
];

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|n| format!("lib{n}.so")).chain(["libc.so.6".to_owned()]).collect()
}

/// The cases where mortar deliberately differs from GNU ld (D-19a): GNU
/// ld refuses a line whose as-needed library is referenced (by an object
/// or by a needed shared object) only after its last mention, or drops it
/// under `--allow-shlib-undefined`; D-11 does not depend on positions, so
/// mortar needs it, at its first mention.
fn needed_order_deliberate() -> Vec<(&'static str, Verdict)> {
    let g = |l: &[&str]| Verdict::GnuRefuses(names(l));
    vec![
        ("drv_obj_after_all", g(&["nd", "na", "nc", "nb", "nf", "ne"])),
        ("obj_after_asn", g(&["nc", "na", "nb", "nd", "ne", "nf"])),
        ("grp_no_group", g(&["na", "nc", "nd", "nb"])),
        ("under_asn_first", g(&["na", "ny", "nb", "nc", "nu"])),
        ("under_asn_first2", g(&["na", "ny", "nb", "nu", "nc"])),
        ("under_asn_both", g(&["na", "ny", "nb", "nu", "nc"])),
        ("under_unref_lib", g(&["na", "nx", "nb", "nw"])),
        (
            "under_asn_first_allow",
            Verdict::Needed(
                names(&["na", "nb", "nc", "nu"]),
                names(&["na", "ny", "nb", "nc", "nu"]),
            ),
        ),
    ]
}

fn needed_order(arch: Arch) {
    if !tools(arch, &["gcc"]) {
        return;
    }
    let dir = Scratch::new("shlib-needed-order");
    build_inputs(arch, dir.path(), "needed_order_inputs.sh");
    let rpath = format!("-Wl,-rpath,{}", dir.path().display());
    let mut sonames = BTreeMap::new();
    let mut table = Vec::new();
    for (i, &(name, line)) in NEEDED_ORDER.iter().enumerate() {
        let mut args = vec![rpath.clone()];
        args.extend(expand(name, line));
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let g = record(arch, "gcc", &args, dir.path(), &format!("out{i}"));
        let m = resolve_line(&g.args, dir.path(), &mut sonames);
        let v = judge(&g, &m);
        if v != Verdict::Agree {
            eprintln!(
                "{} {name}: {v:?}\n  gnu: {}\n  mortar: {:?}",
                arch.name,
                g.stderr.trim(),
                m.errors
            );
        }
        table.push((name, v));
    }
    report(arch, "needed_order", &table, &needed_order_deliberate());
}

/// Prints the counts and checks every difference is pinned.
fn report(arch: Arch, what: &str, table: &[(&str, Verdict)], pinned: &[(&str, Verdict)]) {
    let agree = table.iter().filter(|(_, v)| *v == Verdict::Agree).count();
    let mut deliberate = 0;
    let mut unexplained = Vec::new();
    for (name, v) in table {
        if *v == Verdict::Agree {
            continue;
        }
        if pinned.iter().any(|(n, p)| n == name && p == v) {
            deliberate += 1;
        } else {
            unexplained.push((*name, v.clone()));
        }
    }
    eprintln!(
        "{what} {}: {} lines, {agree} agree, {deliberate} deliberate, {} unexplained",
        arch.name,
        table.len(),
        unexplained.len()
    );
    assert!(unexplained.is_empty(), "{what} {}: unexplained {unexplained:#?}", arch.name);
    let stale: Vec<&str> = pinned
        .iter()
        .filter(|(n, p)| !table.iter().any(|(m, v)| m == n && v == p))
        .map(|(n, _)| *n)
        .collect();
    assert!(stale.is_empty(), "{what} {}: pinned but not seen {stale:?}", arch.name);
}

#[test]
fn d19a_needed_order_aarch64() {
    needed_order(AARCH64);
}

#[test]
fn d19a_needed_order_x86_64() {
    needed_order(X86_64);
}

/// Reads the records `usedset.sh`'s wrapper left in `model`: per link
/// `<id>.args` (cwd, then GNU ld's arguments), `.rc`, `.needed`,
/// `.dynsyms.all`, `.err`, `.env`.
fn records(model: &Path) -> Vec<(String, Gnu, PathBuf, String)> {
    let mut ids: Vec<String> = std::fs::read_dir(model)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().to_str()?.strip_suffix(".args").map(str::to_owned))
        .collect();
    ids.sort();
    let read = |id: &str, ext: &str| {
        std::fs::read_to_string(model.join(format!("{id}.{ext}"))).unwrap_or_default()
    };
    ids.into_iter()
        .map(|id| {
            let text = read(&id, "args");
            let mut lines = text.lines();
            let cwd = PathBuf::from(lines.next().unwrap());
            let args: Vec<String> = lines.map(str::to_owned).collect();
            let ok = read(&id, "rc").trim() == "0";
            let needed = read(&id, "needed")
                .trim()
                .split(',')
                .filter(|n| !n.is_empty())
                .map(str::to_owned)
                .collect();
            let exported =
                if ok { parse_dynsym(&read(&id, "dynsyms.all")) } else { BTreeSet::new() };
            let env = read(&id, "env").trim().to_owned();
            let g = Gnu { args, ok, stderr: read(&id, "err"), needed, exported };
            (id, g, cwd, env)
        })
        .collect()
}

/// The links of `shlib/usedset.sh` where mortar deliberately differs from
/// GNU ld at resolution, by rule:
///
/// - D-15: `--copy-dt-needed-entries` is refused (`2.10b`; `2.10c`
///   agrees: GNU ld refuses it too, its closure symbol undefined).
/// - input §8.3: a shared object without `DT_SONAME` named by a script is
///   refused by name (`1.6a`-`1.6c`, `6.3a`, `6.3b`: `libns.so`).
/// - D-19: `LD_RUN_PATH`/`LD_LIBRARY_PATH` are never read, so a closure
///   library found only through them is not found (`4.4c`, `4.4f`,
///   `4.4g`): D-13 judges the needing library's reference undefined.
/// - D-19a/D-11 (positions do not matter): an as-needed library named
///   before the object that references it (`2.8a`) is needed; GNU ld
///   refuses.
/// - input §4.2/§8.6: the closure search has no built-in directories but
///   finds a closure library in the `-L` directories and the needing
///   library's own directory as GNU ld does not (`4.4h`, `4.4i`).
/// - D-18 (phase 70, the scan's): a copy of, or canonical PLT for, a
///   protected definition is refused after resolution, so resolution
///   alone resolves `5.6*` lines GNU ld refuses.
/// - W1 (`5.7`): a hidden `.dynsym` definition GNU ld binds to is ignored
///   (D-10); the reference stays undefined and mortar refuses.
fn usedset_deliberate(arch: Arch) -> Vec<(&'static str, &'static str)> {
    let mut v = vec![
        ("1.6a-group-script-NA", "mortar-refuses"),
        ("1.6b-group-script-AN", "mortar-refuses"),
        ("1.6c-group-script-hook", "mortar-refuses"),
        ("6.3a-INPUT-script", "mortar-refuses"),
        ("6.3b-INPUT-script-l", "mortar-refuses"),
        ("2.10b-copy-dt-needed", "mortar-refuses"),
        ("2.8a-AN-lib-before-obj", "gnu-refuses"),
        ("4.4h-L-dir-only", "gnu-refuses"),
        ("4.4i-needing-libs-own-dir", "gnu-refuses"),
        ("5.6b-protected-data-nopie", "gnu-refuses"),
        ("5.6d-protected-func-nopie", "gnu-refuses"),
        ("5.7-hidden-in-dynsym", "mortar-refuses"),
    ];
    if arch.name == "aarch64" {
        // The native GNU ld reads the environment (D-19).
        v.extend([
            ("4.4c-LD_RUN_PATH-only", "mortar-refuses"),
            ("4.4f-LD_RUN_PATH-vs-LD_LIBRARY_PATH", "mortar-refuses"),
            ("4.4g-LD_LIBRARY_PATH-only", "mortar-refuses"),
        ]);
    } else {
        // x86-64's PIE takes a copy of protected data (D-18).
        v.push(("5.6a-protected-data-pie", "gnu-refuses"));
    }
    v
}

fn tag(v: &Verdict) -> &'static str {
    match v {
        Verdict::Agree => "agree",
        Verdict::GnuRefuses(_) => "gnu-refuses",
        Verdict::MortarRefuses(_) => "mortar-refuses",
        Verdict::Needed(..) => "needed",
        Verdict::Exports(..) => "exports",
    }
}

/// `shlib/usedset.sh` (E-usedset): every E-shlib experiment, X1-X3c and
/// the default lines, recorded with GNU ld on both architectures, then
/// resolved by mortar.
#[test]
fn e_usedset() {
    if !tools(AARCH64, &["gcc"]) || !tools(X86_64, &["gcc"]) || tool("python3").is_none() {
        return;
    }
    let dir = Scratch::new("shlib-usedset");
    let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/oracle/shlib");
    for (script, envs) in [
        ("run.sh", vec![("LINKERS", ""), ("NORUN", "1")]),
        ("usedset.sh", vec![("NORUN", "1"), ("NOMODEL", "1")]),
    ] {
        let mut cmd = Command::new("bash");
        cmd.current_dir(dir.path())
            .env("TMPDIR", dir.path())
            .env_remove("LD_LIBRARY_PATH")
            .env_remove("LD_RUN_PATH")
            .arg(here.join(script))
            .arg(dir.path());
        if script == "run.sh" {
            cmd.arg("all");
        }
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let o = run_with(&mut cmd, b"", Duration::from_mins(10));
        assert!(o.success(), "{script}: {}", o.stderr_text());
    }
    let mut failures = Vec::new();
    for arch in [AARCH64, X86_64] {
        let model = dir.path().join(arch.name).join("model");
        let mut sonames = BTreeMap::new();
        let mut table: Vec<(String, Verdict)> = Vec::new();
        for (id, g, cwd, env) in records(&model) {
            let m = resolve_line(&g.args, &cwd, &mut sonames);
            let v = judge(&g, &m);
            if v != Verdict::Agree {
                eprintln!(
                    "{} {id}: {v:?}{}\n  gnu: {}\n  mortar: {:?}",
                    arch.name,
                    if env.is_empty() { String::new() } else { format!(" (env {env})") },
                    g.stderr.lines().next().unwrap_or(""),
                    m.errors.iter().map(|e| &e.1).collect::<Vec<_>>()
                );
            }
            table.push((id, v));
        }
        let pinned = usedset_deliberate(arch);
        let agree = table.iter().filter(|(_, v)| *v == Verdict::Agree).count();
        let mut deliberate = 0;
        for (id, v) in &table {
            if *v == Verdict::Agree {
                continue;
            }
            if pinned.iter().any(|(n, t)| n == id && *t == tag(v)) {
                deliberate += 1;
            } else {
                failures.push(format!("{} {id}: {v:?}", arch.name));
            }
        }
        for (n, t) in &pinned {
            if !table.iter().any(|(id, v)| id == n && tag(v) == *t) {
                failures.push(format!("{} {n}: pinned {t}, not seen", arch.name));
            }
        }
        eprintln!(
            "usedset {}: {} links, {agree} agree, {deliberate} deliberate, {} unexplained",
            arch.name,
            table.len(),
            table.len() - agree - deliberate
        );
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// `shlib/dupdef_needed.sh` and the review's `dupdef` (N1): two needed
/// libraries define `dv` (`libnA` `int dv[4]`, `libnB` `int dv[8]`); the
/// `DT_NEEDED` list and the binding (GNU ld's copy size: 16 is `nA`'s, 32
/// `nB`'s) against D-11/D-19a's table.
fn dupdef(arch: Arch) {
    if !tools(arch, &["gcc"]) {
        return;
    }
    let dir = Scratch::new("shlib-dupdef");
    dir.write("a.c", "int dv[4] = {1,1,1,1}; int fa(void){return dv[0];}\n");
    dir.write(
        "b.c",
        "int dv[8] = {2,2,2,2,2,2,2,2}; int fb(void){int s=0; for(int i=0;i<8;i++) s+=dv[i]; return s;}\n",
    );
    dir.write("m.c", "#include <stdio.h>\nextern int dv[];\nint main(void){printf(\"%d %d\\n\",dv[0],dv[7]);return 0;}\n");
    let (cflags, lflags): (&[&str], &[&str]) =
        if arch.name == "aarch64" { (&["-fno-pic"], &["-no-pie"]) } else { (&[], &[]) };
    for (lib, src) in [("libnA.so", "a.c"), ("libnB.so", "b.c")] {
        let soname = format!("-Wl,-soname,{lib}");
        let mut cmd = driver_cmd(arch, "gcc");
        cmd.current_dir(dir.path()).env("TMPDIR", dir.path());
        cmd.args(["-fPIC", "-shared", &soname, "-o", lib, src]);
        let o = run_with(&mut cmd, b"", Duration::from_mins(2));
        assert!(o.success(), "{}", o.stderr_text());
    }
    let mut cmd = driver_cmd(arch, "gcc");
    cmd.current_dir(dir.path())
        .env("TMPDIR", dir.path())
        .args(cflags)
        .args(["-O1", "-c", "m.c", "-o", "m.o"]);
    assert!(run_with(&mut cmd, b"", Duration::from_mins(2)).success());
    let an = "-Wl,--as-needed";
    let na = "-Wl,--no-as-needed";
    // (line, mortar's DT_NEEDED, mortar's bind(dv)); GNU ld's where they
    // agree (dyn.md D-11's table).
    let lines: [(&[&str], &[&str], &str); 4] = [
        (&[an, "-lnA", na, "-lnB", "m.o", an, "-lnA"], &["nB", "nA"], "nB"),
        (&["m.o", an, "-lnA", na, "-lnB"], &["nA", "nB"], "nA"),
        (&[an, "-lnA", na, "-lnB", "m.o", "-lnA"], &["nB", "nA"], "nB"),
        (&["m.o", an, "-lnA", na, "-lnB", "-lnA"], &["nA", "nB"], "nA"),
    ];
    let mut sonames = BTreeMap::new();
    for (i, (line, want, bind)) in lines.iter().enumerate() {
        let mut args: Vec<&str> = lflags.to_vec();
        args.push("-L.");
        args.extend_from_slice(line);
        let g = record(arch, "gcc", &args, dir.path(), &format!("dupdef{i}"));
        assert!(g.ok, "{}", g.stderr);
        let m = resolve_line(&g.args, dir.path(), &mut sonames);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let want: Vec<String> = names(want);
        let bind = format!("lib{bind}.so");
        assert_eq!(m.needed, want, "line {}: DT_NEEDED", i + 1);
        assert_eq!(m.bind.get("dv"), Some(&bind), "line {}: bind(dv)", i + 1);
        // GNU ld's copy of dv.
        let dynsym = output(
            Command::new(readelf().unwrap())
                .args(["--dyn-syms", "-W"])
                .arg(dir.join(format!("dupdef{i}"))),
        );
        let size = dynsym.lines().find_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            (f.len() >= 8 && f[7].split('@').next() == Some("dv")).then(|| f[2].to_owned())
        });
        let gnu_bind = match size.as_deref() {
            Some("16") => "libnA.so",
            Some("32") => "libnB.so",
            other => panic!("line {}: GNU ld's dv {other:?}", i + 1),
        };
        // Line 1 is the deliberate superset: GNU ld drops nA (an
        // as-needed library its binding never used); the binding agrees.
        if i == 0 {
            assert_eq!(g.needed, names(&["nB"]), "line 1: GNU ld");
        } else {
            assert_eq!(g.needed, want, "line {}: GNU ld", i + 1);
        }
        assert_eq!(gnu_bind, bind, "line {}: GNU ld's copy", i + 1);
        eprintln!("dupdef {} line {}: DT_NEEDED {:?}, bind(dv) {bind}", arch.name, i + 1, m.needed);
    }
}

#[test]
fn d11_dupdef_needed_aarch64() {
    dupdef(AARCH64);
}

#[test]
fn d11_dupdef_needed_x86_64() {
    dupdef(X86_64);
}

/// Runs `bash -c script` in `dir` (inputs of an experiment).
fn shell(dir: &Path, script: &str) {
    let mut cmd = Command::new("bash");
    cmd.current_dir(dir).env("TMPDIR", dir).env_remove("LD_LIBRARY_PATH").args(["-c", script]);
    let o = run_with(&mut cmd, b"", Duration::from_mins(5));
    assert!(o.success(), "{script}: {}", o.stderr_text());
}

/// The review's `neededorder` (F5, `.build/review-m2-adv/neededorder`):
/// twelve lines of D-19a with scripts, groups and under-linking; mortar's
/// `DT_NEEDED` equals the review's model of the text on every line, and
/// GNU ld's on every line GNU ld links (it refuses `underlink-after`,
/// whose image mortar's order runs: D-19a).
fn review_neededorder(arch: Arch) {
    if !tools(arch, &["gcc"]) {
        return;
    }
    let dir = Scratch::new("shlib-review-neededorder");
    let cc = tool(&format!("{}gcc", arch.prefix)).unwrap();
    let cc = cc.display();
    shell(
        dir.path(),
        &format!(
            "set -e
for l in na nb nc nx ny nq; do echo \"int fn_$l(void){{return 1;}}\" > $l.c; {cc} -fPIC -shared -Wl,-soname,lib$l.so -o lib$l.so $l.c; done
echo 'int fn_nq(void); int fn_np(void){{return fn_nq();}}' > np.c; {cc} -fPIC -shared -Wl,-soname,libnp.so -o libnp.so np.c
mk() {{ local o=$1; shift; {{ for f in \"$@\"; do echo \"int fn_$f(void);\"; done; echo 'int main(void){{return 0'; for f in \"$@\"; do echo \"+fn_$f()\"; done; echo ';}}'; }} > $o.c; {cc} -c $o.c -o $o.o; }}
mk m0; mk mx nx; mk mp np; mk mxy nx ny
echo 'INPUT ( AS_NEEDED ( libnx.so ) libny.so libnx.so )' > libs1.so
echo 'INPUT ( libnx.so AS_NEEDED ( libnx.so ) libny.so )' > libs2.so
echo 'GROUP ( AS_NEEDED ( libnx.so ) libna.so )' > libs3.so"
        ),
    );
    let na = "-Wl,--no-as-needed";
    let an = "-Wl,--as-needed";
    let (sg, eg) = ("-Wl,--start-group", "-Wl,--end-group");
    // (name, line, the model's DT_NEEDED)
    let cases: Vec<(&str, Vec<&str>, &[&str])> = vec![
        (
            "thrice-ref",
            vec![na, "-lna", an, "-lnx", na, "-lnb", an, "mx.o", "-lnx", na, "-lnc", "-lnx"],
            &["na", "nb", "nx", "nc"],
        ),
        (
            "thrice-unref",
            vec![na, "-lna", an, "-lnx", na, "-lnb", an, "m0.o", "-lnx", na, "-lnc", "-lnx"],
            &["na", "nb", "nc", "nx"],
        ),
        ("group", vec![na, "-lnb", an, sg, "-lnx", "mx.o", eg, na, "-lnc"], &["nb", "nx", "nc"]),
        (
            "group-after",
            vec![na, "-lnb", an, sg, "-lnx", eg, "mx.o", "-lnx", na, "-lnc"],
            &["nb", "nx", "nc"],
        ),
        ("script1-unref", vec![na, "-lna", "m0.o", "libs1.so", "-lnc"], &["na", "ny", "nx", "nc"]),
        ("script1-ref", vec![na, "-lna", "mx.o", "libs1.so", "-lnc"], &["na", "nx", "ny", "nc"]),
        ("script2", vec![na, "-lna", "m0.o", "libs2.so", "-lnc"], &["na", "nx", "ny", "nc"]),
        ("script3-ref", vec![na, "-lnb", "mx.o", "libs3.so", "-lnc"], &["nb", "nx", "na", "nc"]),
        ("script3-unref", vec![na, "-lnb", "m0.o", "libs3.so", "-lnc"], &["nb", "na", "nc"]),
        ("underlink-before", vec![na, "mp.o", "-lnp", an, "-lnq", na, "-lnc"], &["np", "nq", "nc"]),
        ("underlink-after", vec![na, "mp.o", an, "-lnq", na, "-lnp", "-lnc"], &["nq", "np", "nc"]),
        (
            "underlink-after2",
            vec![na, "mp.o", an, "-lnq", na, "-lnp", an, "-lnq", na, "-lnc"],
            &["np", "nq", "nc"],
        ),
    ];
    let mut sonames = BTreeMap::new();
    for (i, (name, line, model)) in cases.iter().enumerate() {
        let mut args = vec!["-L.", "-Wl,-rpath-link,."];
        args.extend(line.iter().copied());
        let g = record(arch, "gcc", &args, dir.path(), &format!("no{i}"));
        let m = resolve_line(&g.args, dir.path(), &mut sonames);
        assert!(m.errors.is_empty(), "{name}: {:?}", m.errors);
        let model: Vec<String> = model.iter().map(|n| format!("lib{n}.so")).collect();
        // `-lnc` is the last library of every line: libc.so.6 follows.
        let mut want = model.clone();
        want.push("libc.so.6".to_owned());
        assert_eq!(m.needed, want, "{name}: mortar vs the model");
        if *name == "underlink-after" {
            assert!(!g.ok, "{name}: GNU ld refuses it");
        } else {
            assert!(g.ok, "{name}: {}", g.stderr);
            assert_eq!(g.needed, want, "{name}: GNU ld");
        }
    }
    eprintln!(
        "review neededorder {}: 12 lines, 11 agree with GNU ld, 1 deliberate (underlink-after), 12 = model",
        arch.name
    );
}

#[test]
fn d19a_review_neededorder_aarch64() {
    review_neededorder(AARCH64);
}

#[test]
fn d19a_review_neededorder_x86_64() {
    review_neededorder(X86_64);
}

/// The review's `usedset2` (F1, `.build/review-m2-adv/usedset2`): default
/// lines E-usedset lacks (clang, clang++, `-static-libstdc++`, duplicate
/// `-pthread -ldl -lm`, `--as-needed` before `libgcc_s`), an under-linked
/// library whose provider only a script's `AS_NEEDED` names, and a script
/// naming `ld-linux` plainly.
fn review_usedset2(arch: Arch) {
    if !tools(arch, &["gcc", "g++", "gfortran", "clang", "clang++"]) {
        return;
    }
    let dir = Scratch::new("shlib-review-usedset2");
    let pfx = arch.prefix;
    let ldso = if arch.name == "aarch64" {
        "/lib/ld-linux-aarch64.so.1"
    } else {
        "/usr/x86_64-linux-gnu/lib64/ld-linux-x86-64.so.2"
    };
    let ct = format!("--target={}-linux-gnu", arch.name);
    let here = dir.path().display();
    shell(
        dir.path(),
        &format!(
            r#"set -e
mkdir -p s
printf '#include <stdio.h>\n#include <math.h>\n#include <pthread.h>\n#include <dlfcn.h>\nint main(int c,char**v){{(void)v;puts("hi");printf("%%g %%p\\n",sin(c),(void*)dlsym);return 0;}}\n' > h.c
printf '#include <iostream>\n#include <thread>\nthread_local int t=1; int main(){{std::thread th([]{{t=2;}}); th.join(); try{{throw 1;}}catch(int){{}} std::cout<<t<<std::endl;}}\n' > h.cc
printf 'program h\n print *, "hi"\nend program h\n' > h.f90
echo 'int q_fn(void){{return 7;}}' > s/q.c
echo 'int q_fn(void); int p_fn(void){{return 100+q_fn();}}' > s/p.c
printf '#include <stdio.h>\nint p_fn(void); int main(void){{printf("%%d\\n",p_fn());return 0;}}\n' > s/m.c
{pfx}gcc -fPIC -shared -Wl,-soname,libq.so -o s/libq.so s/q.c
{pfx}gcc -fPIC -shared -Wl,-soname,libp.so -o s/libp.so s/p.c
echo "GROUP ( {here}/s/libp.so AS_NEEDED ( {here}/s/libq.so ) )" > s/libpq.so
echo "GROUP ( {ldso} )" > s/libldso.so
{pfx}gcc -c h.c -o hc.o; {pfx}g++ -c h.cc -o hcc.o; {pfx}gfortran -c h.f90 -o hf.o; {pfx}gcc -c s/m.c -o s/m.o
clang {ct} -c h.c -o hcl.o; clang++ {ct} -c h.cc -o hccl.o"#
        ),
    );
    let lines: Vec<(&str, &str, Vec<&str>)> = vec![
        ("gcc-default", "gcc", vec!["hc.o", "-lm"]),
        (
            "gcc-dups",
            "gcc",
            vec!["-pthread", "hc.o", "-ldl", "-lm", "-lpthread", "-ldl", "-lm", "-lpthread"],
        ),
        (
            "gcc-asn-gcc_s",
            "gcc",
            vec!["-Wl,--as-needed", "hc.o", "-lm", "-shared-libgcc", "-Wl,--no-as-needed"],
        ),
        ("gpp-default", "g++", vec!["hcc.o"]),
        ("gpp-static-stdcxx", "g++", vec!["-static-libstdc++", "hcc.o"]),
        ("gpp-static-libgcc", "g++", vec!["-static-libstdc++", "-static-libgcc", "hcc.o"]),
        ("gpp-asn-all", "g++", vec!["-Wl,--as-needed", "hcc.o", "-Wl,--no-as-needed", "-lgcc_s"]),
        (
            "gpp-pthread-dups",
            "g++",
            vec!["-pthread", "hcc.o", "-lpthread", "-ldl", "-lm", "-lpthread"],
        ),
        ("gfortran-default", "gfortran", vec!["hf.o"]),
        ("gfortran-nopie", "gfortran", vec!["-no-pie", "hf.o"]),
        ("clang-default", "clang", vec!["hcl.o", "-lm"]),
        ("clangpp-default", "clang++", vec!["hccl.o"]),
        ("clangpp-static", "clang++", vec!["-static-libstdc++", "hccl.o"]),
        ("script-underlink", "gcc", vec!["s/m.o", "-Ls", "-lpq"]),
        (
            "script-underlink-rev",
            "gcc",
            vec!["s/m.o", "-Wl,--as-needed", "s/libpq.so", "-Wl,--no-as-needed"],
        ),
        ("script-ldso-plain", "gcc", vec!["hc.o", "-lm", "s/libldso.so"]),
    ];
    let mut sonames = BTreeMap::new();
    let mut table = Vec::new();
    for (i, (name, driver, args)) in lines.iter().enumerate() {
        let g = record(arch, driver, args, dir.path(), &format!("u2-{i}"));
        let m = resolve_line(&g.args, dir.path(), &mut sonames);
        let v = judge(&g, &m);
        if v != Verdict::Agree {
            eprintln!(
                "{} {name}: {v:?}\n  gnu: {}\n  mortar: {:?}",
                arch.name,
                g.stderr.trim(),
                m.errors
            );
        }
        table.push((*name, v));
    }
    report(arch, "review usedset2", &table, &[]);
}

#[test]
fn d11_review_usedset2_aarch64() {
    review_usedset2(AARCH64);
}

#[test]
fn d11_review_usedset2_x86_64() {
    review_usedset2(X86_64);
}

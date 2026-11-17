//! `cargo xtask odr-runtime [--check | --update]`: the objects of the
//! load-time ODR runtime (`spec/odr.md` O-53b, `runtime/odr/`).
//!
//! The runtime's one source is `runtime/odr/mason_odr_rt.cpp`. Its six
//! objects (`aarch64`, `x86_64`, `i386` × `so`, `exe`) are checked in under
//! `runtime/odr/obj/`, and `runtime/odr/PINS` names the tools that built
//! them and the commands. This command rebuilds the six from the source:
//!
//! - kiln (`MASON_KILN`) with hod (`MASON_HOD`) as its back end, for
//!   aarch64 and x86-64;
//! - clang (`MASON_CLANG_I386`, default `clang++-22`) for the IR and hod
//!   (`MASON_HOD_I386`, default `MASON_HOD`) for i386, which the pinned
//!   kiln does not target.
//!
//! First the tools' identities (kiln's and hod's commits, clang's version
//! line) are compared with `PINS`: other tools are a named skip, or a
//! failure under `MORTAR_REQUIRE_TOOLS=1`. Then `--check` (the default)
//! fails unless every rebuilt byte equals the checked-in object, and
//! `--update` writes the objects and `PINS` anew (a pin move is that one
//! command, then the matrix and the cost tables of `spec/odr.md` O-58).
//!
//! A tool's commit is its source checkout's `HEAD` (`-dirty` when the
//! checkout has changes): the checkout named by `MASON_KILN_SRC`,
//! `MASON_HOD_SRC` or `MASON_HOD_I386_SRC`, else the build root's
//! convention, `<root>/<name>` for a tool at
//! `<root>/target-<name>/release/<tool>`. The binaries do not say their
//! commit, so a binary not rebuilt after its checkout moved is not caught.
//!
//! The source is copied into the scratch directory and compiled there by
//! its file name, so the bytes do not depend on the checkout's path.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use crate::process::{self, Limits};

macro_rules! say {
    ($($t:tt)*) => { $crate::mason::emit(format_args!("{}\n", format_args!($($t)*))) };
}

/// The usage line.
pub const USAGE: &str = "usage: cargo xtask odr-runtime [--check | --update]\n";

/// A compiler's limits: the runtime compiles in well under a second.
const LIMITS: Limits = Limits { time: Duration::from_mins(3), memory_kib: 8 << 20 };

/// The source, in `runtime/odr/`.
const SOURCE: &str = "mason_odr_rt.cpp";

/// The flags every route compiles with.
const FLAGS: [&str; 5] = ["-c", "-O0", "-fPIC", "-fno-exceptions", "-fno-rtti"];

/// A target's route to an object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// kiln++ with hod as its back end; the `--target` argument, if any.
    Kiln(Option<&'static str>),
    /// clang's IR, then hod for `i686-linux-gnu`.
    ClangHod,
}

/// The three targets, in the order of `PINS`.
pub const TARGETS: [(&str, Route); 3] = [
    ("aarch64", Route::Kiln(None)),
    ("x86_64", Route::Kiln(Some("--target=x86_64-linux-gnu"))),
    ("i386", Route::ClangHod),
];

/// The two kinds: the shared object's flavour and the executable's.
pub const KINDS: [(&str, Option<&str>); 2] = [("so", None), ("exe", Some("-DMASON_RT_EXE"))];

/// `mason_odr_rt-<arch>-<kind>.o`.
pub fn object_name(arch: &str, kind: &str) -> String {
    format!("mason_odr_rt-{arch}-{kind}.o")
}

/// The command that builds one object, in words, as `PINS` records it.
pub fn command_text(arch: &str, route: Route, define: Option<&str>) -> String {
    let flags = FLAGS.join(" ");
    let define = define.map_or(String::new(), |d| format!(" {d}"));
    match route {
        Route::Kiln(target) => {
            let target = target.map_or(String::new(), |t| format!(" {t}"));
            format!(
                "KILN_BACKEND=hod KILN_HOD=<hod> KILN_HOD_STRICT=1 <kiln>{target} {flags}{define} \
                 {SOURCE} -o <object>"
            )
        }
        Route::ClangHod => {
            debug_assert_eq!(arch, "i386", "the clang route is i386's");
            format!(
                "<clang-i386> --target=i686-linux-gnu -O0 -fPIC -fno-exceptions -fno-rtti{define} \
                 -S -emit-llvm -o <object>.ll {SOURCE} && <hod-i386> -O0 -filetype=obj \
                 -mtriple=i686-linux-gnu -relocation-model=pic --strict <object>.ll -o <object>"
            )
        }
    }
}

/// What `PINS` records.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pins {
    pub kiln: String,
    pub hod: String,
    pub hod_i386: String,
    pub clang_i386: String,
    /// (object name, SHA-256 in hex, size in bytes).
    pub objects: Vec<(String, String, u64)>,
}

impl Pins {
    /// Reads `PINS`' text: `key value` lines, `#` comments.
    pub fn parse(text: &str) -> Result<Pins, String> {
        let mut pins = Pins::default();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line.split_once(' ').unwrap_or((line, ""));
            let value = value.trim();
            match key {
                "kiln" => value.clone_into(&mut pins.kiln),
                "hod" => value.clone_into(&mut pins.hod),
                "hod-i386" => value.clone_into(&mut pins.hod_i386),
                "clang-i386" => value.clone_into(&mut pins.clang_i386),
                "command" => {}
                "object" => {
                    let fields: Vec<&str> = value.split_whitespace().collect();
                    let [name, digest, size] = fields[..] else {
                        return Err(format!(
                            "PINS line {}: `object <name> <sha256> <size>`",
                            n + 1
                        ));
                    };
                    let size =
                        size.parse().map_err(|_| format!("PINS line {}: size '{size}'", n + 1))?;
                    pins.objects.push((name.to_owned(), digest.to_owned(), size));
                }
                other => return Err(format!("PINS line {}: unknown key '{other}'", n + 1)),
            }
        }
        Ok(pins)
    }

    /// `PINS`' text.
    pub fn render(&self) -> String {
        use std::fmt::Write;
        let mut s = String::from(
            "# The tools that built runtime/odr/obj/ (spec/odr.md O-53b). Written by\n\
             # `cargo xtask odr-runtime --update`; `cargo xtask odr-runtime --check`\n\
             # rebuilds the objects with these tools and fails unless every byte is\n\
             # identical. kiln and hod: their source checkouts' commits; clang: its\n\
             # `--version` line (i386's IR, which the pinned kiln does not target).\n",
        );
        let _ = writeln!(s, "kiln {}", self.kiln);
        let _ = writeln!(s, "hod {}", self.hod);
        let _ = writeln!(s, "hod-i386 {}", self.hod_i386);
        let _ = writeln!(s, "clang-i386 {}", self.clang_i386);
        s.push_str(
            "# The commands, run in a directory holding a copy of runtime/odr/mason_odr_rt.cpp\n\
             # (<kiln> = MASON_KILN, <hod> = MASON_HOD, <hod-i386> = MASON_HOD_I386,\n\
             # <clang-i386> = MASON_CLANG_I386):\n",
        );
        for (arch, route) in TARGETS {
            for (kind, define) in KINDS {
                let _ = writeln!(s, "command {arch}-{kind} {}", command_text(arch, route, define));
            }
        }
        s.push_str("# The objects: name, SHA-256, size in bytes.\n");
        for (name, digest, size) in &self.objects {
            let _ = writeln!(s, "object {name} {digest} {size}");
        }
        s
    }
}

/// The tools of a rebuild.
#[derive(Clone, Debug)]
struct Tools {
    kiln: PathBuf,
    hod: PathBuf,
    hod_i386: PathBuf,
    clang_i386: PathBuf,
}

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

impl Tools {
    fn find() -> Result<Tools, String> {
        let root = crate::mason::build_root();
        let kiln =
            env_path("MASON_KILN").unwrap_or_else(|| root.join("target-kiln/release/kiln++"));
        let hod = env_path("MASON_HOD").unwrap_or_else(|| root.join("target-hod/release/hod"));
        let hod_i386 = env_path("MASON_HOD_I386").unwrap_or_else(|| hod.clone());
        let clang_i386 = match env_path("MASON_CLANG_I386") {
            Some(p) => crate::bench::prepare::which(&p.to_string_lossy())?,
            None => crate::bench::prepare::which("clang++-22")?,
        };
        let missing: Vec<String> = [&kiln, &hod, &hod_i386]
            .iter()
            .filter(|p| !p.is_file())
            .map(|p| p.display().to_string())
            .collect();
        if missing.is_empty() {
            Ok(Tools { kiln, hod, hod_i386, clang_i386 })
        } else {
            Err(format!(
                "no build at {} (MASON_KILN, MASON_HOD, MASON_HOD_I386)",
                missing.join(", ")
            ))
        }
    }
}

fn run_tool(
    program: &Path,
    args: &[&str],
    dir: &Path,
    env: &[(&str, &Path)],
) -> Result<process::Output, String> {
    process::run(
        program,
        |c| {
            c.args(args).current_dir(dir);
            for (k, v) in env {
                c.env(k, v);
            }
        },
        b"",
        LIMITS,
    )
}

/// The commit of the checkout a tool was built from (`-dirty` when it has
/// changes), or why it is not known.
fn commit_of(tool: &Path, var: &str) -> Result<String, String> {
    let checkout = if let Some(dir) = env_path(var) {
        dir
    } else {
        // <root>/target-<name>/release/<tool> -> <root>/<name>
        let target = tool.parent().and_then(Path::parent);
        let name = target
            .and_then(Path::file_name)
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_prefix("target-"));
        match (target.and_then(Path::parent), name) {
            (Some(root), Some(name)) => root.join(name),
            _ => {
                return Err(format!(
                    "{}: no checkout by the build root's convention (set {var})",
                    tool.display()
                ));
            }
        }
    };
    let git = crate::bench::prepare::which("git")?;
    let git = git.as_path();
    let dir = checkout.to_string_lossy().into_owned();
    let head = run_tool(git, &["-C", &dir, "rev-parse", "HEAD"], Path::new("/"), &[])?;
    if !head.end.success() {
        return Err(format!("{}: not a git checkout ({var})", checkout.display()));
    }
    let mut commit = String::from_utf8_lossy(&head.stdout).trim().to_owned();
    let status = run_tool(
        git,
        &["-C", &dir, "status", "--porcelain", "--untracked-files=no"],
        Path::new("/"),
        &[],
    )?;
    if !status.end.success() || !status.stdout.is_empty() {
        commit.push_str("-dirty");
    }
    Ok(commit)
}

/// clang's first `--version` line.
fn clang_version(clang: &Path) -> Result<String, String> {
    let out = run_tool(clang, &["--version"], Path::new("/"), &[])
        .map_err(|e| format!("{}: {e}", clang.display()))?;
    if !out.end.success() {
        return Err(format!("{} --version: {}", clang.display(), out.end));
    }
    Ok(String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or("").trim().to_owned())
}

fn identities(tools: &Tools) -> Result<Pins, String> {
    Ok(Pins {
        kiln: commit_of(&tools.kiln, "MASON_KILN_SRC")?,
        hod: commit_of(&tools.hod, "MASON_HOD_SRC")?,
        hod_i386: commit_of(&tools.hod_i386, "MASON_HOD_I386_SRC")?,
        clang_i386: clang_version(&tools.clang_i386)?,
        objects: Vec::new(),
    })
}

/// Builds one object into `dir` (which holds the source); its bytes.
fn build_one(
    tools: &Tools,
    dir: &Path,
    arch: &str,
    route: Route,
    kind: &str,
    define: Option<&str>,
) -> Result<Vec<u8>, String> {
    let object = object_name(arch, kind);
    let _ = std::fs::remove_file(dir.join(&object));
    let failed = |what: &str, out: &process::Output| {
        format!(
            "{object}: {what} failed ({}): {}",
            out.end,
            String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("")
        )
    };
    match route {
        Route::Kiln(target) => {
            let mut args: Vec<&str> = Vec::new();
            args.extend(target);
            args.extend(FLAGS);
            args.extend(define);
            args.extend([SOURCE, "-o", &object]);
            let env = [
                ("KILN_BACKEND", Path::new("hod")),
                ("KILN_HOD", tools.hod.as_path()),
                ("KILN_HOD_STRICT", Path::new("1")),
            ];
            let out = run_tool(&tools.kiln, &args, dir, &env)?;
            if !out.end.success() {
                return Err(failed("kiln++", &out));
            }
        }
        Route::ClangHod => {
            let ir = format!("{object}.ll");
            let mut args =
                vec!["--target=i686-linux-gnu", "-O0", "-fPIC", "-fno-exceptions", "-fno-rtti"];
            args.extend(define);
            args.extend(["-S", "-emit-llvm", "-o", &ir, SOURCE]);
            let out = run_tool(&tools.clang_i386, &args, dir, &[])?;
            if !out.end.success() {
                return Err(failed("clang", &out));
            }
            let args = [
                "-O0",
                "-filetype=obj",
                "-mtriple=i686-linux-gnu",
                "-relocation-model=pic",
                "--strict",
                &ir,
                "-o",
                &object,
            ];
            let out = run_tool(&tools.hod_i386, &args, dir, &[])?;
            if !out.end.success() {
                return Err(failed("hod", &out));
            }
        }
    }
    std::fs::read(dir.join(&object)).map_err(|e| format!("{object}: {e}"))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// The checkout's `runtime/odr`.
pub fn runtime_dir() -> PathBuf {
    crate::linktest::workspace_root().join("runtime").join("odr")
}

fn scratch_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map_or_else(|| crate::linktest::workspace_root().join("target"), PathBuf::from)
        .join("odr-runtime")
}

/// `cargo xtask odr-runtime`.
pub fn main(args: &[String]) -> ExitCode {
    let update = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [] | ["--check"] => false,
        ["--update"] => true,
        _ => {
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(update) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("xtask odr-runtime: {e}");
            ExitCode::FAILURE
        }
    }
}

fn skip_or_fail(why: &str) -> Result<ExitCode, String> {
    if mortar_testutil::tools_required() {
        Err(format!("{why} (MORTAR_REQUIRE_TOOLS=1)"))
    } else {
        say!("xtask odr-runtime: skipped: {why}");
        Ok(ExitCode::SUCCESS)
    }
}

fn run(update: bool) -> Result<ExitCode, String> {
    let start = Instant::now();
    let rt = runtime_dir();
    let pins_path = rt.join("PINS");
    let tools = match Tools::find() {
        Ok(t) => t,
        Err(e) => return skip_or_fail(&e),
    };
    let mut now = match identities(&tools) {
        Ok(p) => p,
        Err(e) => return skip_or_fail(&e),
    };
    if !update {
        let text = std::fs::read_to_string(&pins_path)
            .map_err(|e| format!("{}: {e}", pins_path.display()))?;
        let pinned = Pins::parse(&text)?;
        let differ: Vec<String> = [
            ("kiln", &pinned.kiln, &now.kiln),
            ("hod", &pinned.hod, &now.hod),
            ("hod-i386", &pinned.hod_i386, &now.hod_i386),
            ("clang-i386", &pinned.clang_i386, &now.clang_i386),
        ]
        .iter()
        .filter(|(_, a, b)| a != b)
        .map(|(k, a, b)| format!("{k} is '{b}', PINS names '{a}'"))
        .collect();
        if !differ.is_empty() {
            return skip_or_fail(&format!(
                "not the pinned tools: {} (cargo xtask odr-runtime --update re-records the objects)",
                differ.join("; ")
            ));
        }
        // PINS's digests are the checked-in objects'
        for (name, digest, size) in &pinned.objects {
            let bytes =
                std::fs::read(rt.join("obj").join(name)).map_err(|e| format!("obj/{name}: {e}"))?;
            if hex(&mortar_digest::sha256(&bytes)) != *digest || bytes.len() as u64 != *size {
                return Err(format!("obj/{name} is not the object PINS records"));
            }
        }
    }
    let dir = scratch_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    std::fs::copy(rt.join(SOURCE), dir.join(SOURCE)).map_err(|e| format!("{SOURCE}: {e}"))?;
    let mut changed = Vec::new();
    for (arch, route) in TARGETS {
        for (kind, define) in KINDS {
            let bytes = build_one(&tools, &dir, arch, route, kind, define)?;
            let name = object_name(arch, kind);
            let path = rt.join("obj").join(&name);
            let same = std::fs::read(&path).is_ok_and(|old| old == bytes);
            if !same {
                changed.push(name.clone());
            }
            if update && !same {
                std::fs::create_dir_all(rt.join("obj")).map_err(|e| format!("obj/: {e}"))?;
                std::fs::write(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
            }
            now.objects.push((name, hex(&mortar_digest::sha256(&bytes)), bytes.len() as u64));
        }
    }
    let secs = start.elapsed().as_secs_f64();
    if update {
        std::fs::write(&pins_path, now.render())
            .map_err(|e| format!("{}: {e}", pins_path.display()))?;
        say!(
            "xtask odr-runtime: updated: {} of 6 objects changed{}; PINS rewritten ({secs:.1} s)",
            changed.len(),
            if changed.is_empty() { String::new() } else { format!(" ({})", changed.join(", ")) }
        );
        return Ok(ExitCode::SUCCESS);
    }
    if changed.is_empty() {
        say!("xtask odr-runtime: ok: 6 objects rebuilt byte-identical ({secs:.1} s)");
        Ok(ExitCode::SUCCESS)
    } else {
        Err(format!(
            "the rebuilt bytes differ from runtime/odr/obj/ for {} ({secs:.1} s; \
             cargo xtask odr-runtime --update re-records them)",
            changed.join(", ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_round_trip() {
        let pins = Pins {
            kiln: "k1".into(),
            hod: "h1".into(),
            hod_i386: "h2-dirty".into(),
            clang_i386: "Debian clang version 22.1.8 (1~deb13u4)".into(),
            objects: vec![("mason_odr_rt-aarch64-so.o".into(), "ab".repeat(32), 123)],
        };
        assert_eq!(Pins::parse(&pins.render()).unwrap(), pins);
    }

    #[test]
    fn pins_refuses_unknown_keys() {
        assert!(Pins::parse("kilm abc\n").is_err());
        assert!(Pins::parse("object a b\n").is_err());
    }

    #[test]
    fn six_objects() {
        let names: Vec<String> = TARGETS
            .iter()
            .flat_map(|(arch, _)| KINDS.iter().map(move |(kind, _)| object_name(arch, kind)))
            .collect();
        assert_eq!(names.len(), 6);
        assert!(names.contains(&"mason_odr_rt-i386-exe.o".to_owned()));
    }
}

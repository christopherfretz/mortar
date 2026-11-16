//! The `ninja` workloads (`spec/harness.md` "bench", "Large workloads",
//! WP-081): a build tree too long to make in a bench run (hours of
//! compiling) is made once by a pinned script of this checkout; the bench
//! captures the link of one target from it, copies the files that link
//! reads out of the tree into the tree's `inputs/` (thin archives written
//! as regular ones), and replays the link there. Once every workload of a
//! tree is captured the tree itself can be deleted; the capture is never
//! redone or deleted by the bench.

use std::path::{Component, Path, PathBuf};

use super::manifest::{Ninja, Workload};
use super::prepare::{self, Env, Log, Prepared};
use super::response::{Capture, wrapper_script, write_response};

/// How one argument of the captured link names a file or directory of
/// the build tree: the text before the path, the path.
fn path_part(arg: &str) -> (&str, &str) {
    if let Some(path) = arg.strip_prefix("-L") {
        return ("-L", path);
    }
    if arg.starts_with("--")
        && let Some((option, path)) = arg.split_once('=')
    {
        return (&arg[..=option.len()], path);
    }
    ("", arg)
}

/// `path` (absolute, or relative to the tree's `build`) as a path relative
/// to `build` that stays inside it; `None` for a path outside the tree.
pub fn inside(path: &str, build: &Path) -> Option<PathBuf> {
    if path.is_empty() {
        return None;
    }
    let path = Path::new(path);
    let rel = if path.is_absolute() { path.strip_prefix(build).ok()? } else { path };
    let mut out = PathBuf::new();
    for part in rel.components() {
        match part {
            Component::Normal(p) => out.push(p),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// What the capture does with one argument.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Move {
    /// Not a path of the tree: kept as it is.
    Keep,
    /// A file of the tree: copied to `inputs/<path>`, named by that path.
    Copy(PathBuf),
    /// A directory of the tree (`-L`), or a file the link writes there
    /// (`--dependency-file=`): named relative to `inputs`, its directory
    /// made.
    Name(PathBuf),
}

/// The argument relocated from the tree to `inputs`, and what is to be
/// done for it. `stat` says whether a path of the tree is a file
/// (`Some(true)`), a directory (`Some(false)`) or nothing.
pub fn relocate(arg: &str, build: &Path, stat: &dyn Fn(&Path) -> Option<bool>) -> (String, Move) {
    let (prefix, path) = path_part(arg);
    let Some(rel) = inside(path, build) else { return (arg.to_owned(), Move::Keep) };
    let named = format!("{prefix}{}", rel.display());
    match stat(&build.join(&rel)) {
        Some(true) => (named, Move::Copy(rel)),
        Some(false) => (named, Move::Name(rel)),
        // A relative name that is nothing in the tree (`-lm`, the output
        // the link writes) stays as written; an output under the tree
        // given by an option is moved with it.
        None if !prefix.is_empty() && prefix != "-L" => {
            (named, Move::Name(rel.parent().map(Path::to_path_buf).unwrap_or_default()))
        }
        None if Path::new(path).is_absolute() => (named, Move::Keep),
        None => (arg.to_owned(), Move::Keep),
    }
}

/// Prepares a `ninja` workload: from its stamp, by a census alone, or by
/// capturing its link from the tree.
pub fn prepare(
    w: &Workload,
    ninja: &Ninja,
    env: &Env,
    key: &str,
    log: Log<'_>,
) -> Result<Prepared, String> {
    let dir = env.workloads().join(&w.name);
    let tree = env.workloads().join(&ninja.tree);
    let inputs = tree.join("inputs");
    let response = dir.join("response.txt");
    let stamp_path = dir.join("stamp");
    if response.is_file() {
        let stamp =
            std::fs::read_to_string(&stamp_path).ok().and_then(|t| prepare::Stamp::parse(&t));
        if let Some(stamp) = stamp.filter(|s| s.key == key && s.files_unchanged()) {
            return Ok(stamp.prepared(w, dir, response));
        }
        log(&format!("{}: counting the captured link's inputs", w.name));
        let stamp = prepare::census(w, &dir, &inputs, &tree, key, env)?;
        stamp.write(&stamp_path)?;
        return Ok(stamp.prepared(w, dir, response));
    }
    let build = tree.join("build");
    if !build.join("build.ninja").is_file() {
        return Err(format!(
            "{}: not captured, and no build tree at {}: make it with `{} {} {}` (hours; \
             spec/harness.md \"bench\", \"Large workloads\")",
            w.name,
            build.display(),
            ninja.script[0],
            env.work.display(),
            ninja.script[1..].join(" ")
        ));
    }
    log(&format!("{}: capturing the link of {} from {}", w.name, ninja.target, build.display()));
    for sub in ["records", "wrapper", "tmp"] {
        let path = dir.join(sub);
        std::fs::create_dir_all(&path)
            .map_err(|e| format!("cannot create {}: {e}", path.display()))?;
    }
    let records = dir.join("records");
    let wrapper = dir.join("wrapper");
    prepare::write_executable(&wrapper.join("ld"), &wrapper_script(&records, None))?;
    let commands = prepare::tool(
        &format!("{}: ninja -t commands -s {}", w.name, ninja.target),
        &prepare::which("ninja")?,
        |c| {
            c.arg("-C").arg(&build).args(["-t", "commands", "-s", &ninja.target]);
        },
        prepare::TOOL_LIMITS,
    )?;
    let text = String::from_utf8_lossy(&commands.stdout);
    let command = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").to_owned();
    if command.is_empty() {
        return Err(format!("{}: ninja has no command for {}", w.name, ninja.target));
    }
    prepare::tool(
        &format!("{}: the link of {} with the recording ld", w.name, ninja.target),
        Path::new("/bin/sh"),
        |c| {
            c.arg("-c").arg(&command).current_dir(&build);
            c.env("TMPDIR", dir.join("tmp")).env("LC_ALL", "C").env("COMPILER_PATH", &wrapper);
        },
        prepare::TOOL_LIMITS,
    )?;
    let capture = prepare::select_capture(w, &records)?;
    let canonical = std::fs::canonicalize(&build)
        .map_err(|e| format!("cannot read {}: {e}", build.display()))?;
    if std::fs::canonicalize(&capture.cwd).ok().as_deref() != Some(canonical.as_path()) {
        return Err(format!(
            "{}: the link ran in {}, not in the tree {}",
            w.name,
            capture.cwd.display(),
            build.display()
        ));
    }
    let args = copy_inputs(w, &capture, &build, &inputs, log)?;
    std::fs::write(&response, write_response(&args))
        .map_err(|e| format!("cannot write {}: {e}", response.display()))?;
    let stamp = prepare::census(w, &dir, &inputs, &tree, key, env)?;
    stamp.write(&stamp_path)?;
    Ok(stamp.prepared(w, dir, response))
}

/// Relocates every argument and copies what the link reads from the tree
/// into `inputs` (a file already there with the same size is taken as
/// copied: the workloads of one tree share their inputs).
fn copy_inputs(
    w: &Workload,
    capture: &Capture,
    build: &Path,
    inputs: &Path,
    log: Log<'_>,
) -> Result<Vec<String>, String> {
    let stat = |p: &Path| std::fs::metadata(p).ok().map(|m| m.is_file());
    let mut args = Vec::new();
    let mut copied = 0usize;
    for arg in &capture.args {
        let (arg, action) = relocate(arg, build, &stat);
        match action {
            Move::Keep => {}
            Move::Name(rel) => {
                let path = inputs.join(rel);
                std::fs::create_dir_all(&path)
                    .map_err(|e| format!("cannot create {}: {e}", path.display()))?;
            }
            Move::Copy(rel) => {
                copy_input(w, build, &rel, inputs)?;
                copied += 1;
            }
        }
        args.push(arg);
    }
    log(&format!("{}: {copied} files of the tree are the link's inputs", w.name));
    Ok(args)
}

/// Copies `build/<rel>` to `inputs/<rel>`: a thin archive as the regular
/// archive of the same members in the same order (`ar Dqc`, `ranlib -D`,
/// as LLVM's build makes its archives), anything else as it is.
fn copy_input(w: &Workload, build: &Path, rel: &Path, inputs: &Path) -> Result<(), String> {
    let from = build.join(rel);
    let to = inputs.join(rel);
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let mut head = [0u8; 8];
    {
        use std::io::Read;
        let mut file = std::fs::File::open(&from)
            .map_err(|e| format!("cannot read {}: {e}", from.display()))?;
        let _ = file.read(&mut head);
    }
    if &head != b"!<thin>\n" {
        let size = std::fs::metadata(&from).map(|m| m.len()).ok();
        if size.is_some() && std::fs::metadata(&to).map(|m| m.len()).ok() == size {
            return Ok(());
        }
        std::fs::copy(&from, &to).map_err(|e| format!("cannot copy {}: {e}", from.display()))?;
        return Ok(());
    }
    if to.is_file() {
        // Written by an earlier capture from this tree (a regular archive
        // is only ever written whole: below, through a temporary name).
        return Ok(());
    }
    let ar = prepare::which("ar")?;
    let listed = prepare::tool(
        &format!("{}: ar t {}", w.name, rel.display()),
        &ar,
        |c| {
            c.arg("t").arg(rel).current_dir(build);
        },
        prepare::TOOL_LIMITS,
    )?;
    let members: Vec<String> =
        String::from_utf8_lossy(&listed.stdout).lines().map(str::to_owned).collect();
    let part = to.with_extension("a.part");
    let _ = std::fs::remove_file(&part);
    prepare::tool(
        &format!("{}: ar Dqc {}", w.name, rel.display()),
        &ar,
        |c| {
            c.arg("Dqc").arg(&part).args(&members).current_dir(build);
        },
        prepare::TOOL_LIMITS,
    )?;
    prepare::tool(
        &format!("{}: ranlib -D {}", w.name, rel.display()),
        &prepare::which("ranlib")?,
        |c| {
            c.arg("-D").arg(&part);
        },
        prepare::TOOL_LIMITS,
    )?;
    std::fs::rename(&part, &to).map_err(|e| format!("cannot rename {}: {e}", part.display()))
}

// spec/harness.md "bench", "Large workloads".
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_relocated() {
        let build = Path::new("/w/t/build");
        let stat = |p: &Path| match p.to_str().unwrap() {
            "/w/t/build/lib/libA.a" | "/w/t/build/main.o" => Some(true),
            "/w/t/build/lib" => Some(false),
            _ => None,
        };
        let r = |a: &str| relocate(a, build, &stat);
        assert_eq!(r("lib/libA.a"), ("lib/libA.a".into(), Move::Copy("lib/libA.a".into())));
        assert_eq!(r("./main.o"), ("main.o".into(), Move::Copy("main.o".into())));
        assert_eq!(
            r("/w/t/build/lib/libA.a"),
            ("lib/libA.a".into(), Move::Copy("lib/libA.a".into()))
        );
        assert_eq!(r("-L/w/t/build/lib"), ("-Llib".into(), Move::Name("lib".into())));
        assert_eq!(r("-L/usr/lib"), ("-L/usr/lib".into(), Move::Keep));
        assert_eq!(r("-lm"), ("-lm".into(), Move::Keep));
        assert_eq!(r("/usr/lib/crt1.o"), ("/usr/lib/crt1.o".into(), Move::Keep));
        assert_eq!(r("bin/out"), ("bin/out".into(), Move::Keep));
        assert_eq!(
            r("--dependency-file=tools/x/link.d"),
            ("--dependency-file=tools/x/link.d".into(), Move::Name("tools/x".into()))
        );
        assert_eq!(r("../outside.o"), ("../outside.o".into(), Move::Keep));
        assert_eq!(r("-plugin-opt=x"), ("-plugin-opt=x".into(), Move::Keep));
        assert_eq!(inside("a/../b", build), Some(PathBuf::from("b")));
        assert_eq!(inside("/w/t/other", build), None);
    }
}

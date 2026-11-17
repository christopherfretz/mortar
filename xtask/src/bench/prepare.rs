//! Preparing a workload (`spec/harness.md` "bench", "Preparation",
//! "Capture", "Census", "The cache"): build its inputs from pinned
//! sources, capture the link line with a recording `ld`, store it as a
//! response file, and count and hash what the link reads.

use std::fmt::Write as _;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use super::manifest::{self, Cargo, Cmake, Kind, Workload};
use super::record::{Fnv, WorkloadInfo};
use super::response::{
    Capture, glob_match, parse_depfile, sh_quote, wrapper_script, write_response,
};
use super::synth::{GENERATOR, Request, Shape};
use crate::process::{self, Limits, Output};

/// The first line of a stamp.
const STAMP_MAGIC: &str = "# mortar-bench-stamp 1";

/// The real linker behind the recording wrapper of a build that needs
/// the link's output, and the linker of the census.
pub const GNU_LD: &str = "/usr/bin/ld.bfd";

/// A compiler or archiver of a build.
pub(super) const TOOL_LIMITS: Limits =
    Limits { time: Duration::from_mins(15), memory_kib: 8 << 20 };
/// make, for a whole build.
const MAKE_LIMITS: Limits = Limits { time: Duration::from_hours(3), memory_kib: 8 << 20 };
/// A download.
const FETCH_LIMITS: Limits = Limits { time: Duration::from_mins(20), memory_kib: 4 << 20 };
/// The census link.
const CENSUS_LIMITS: Limits = Limits { time: Duration::from_mins(10), memory_kib: 16 << 20 };

/// Source files compiled by one compiler process of a synthetic build.
const BATCH: usize = 64;
/// The most compiler processes a synthetic build runs at once (each also
/// holds a token of the pool).
const MAX_WORKERS: usize = 8;

/// Where a run of the bench works.
#[derive(Clone, Debug)]
pub struct Env {
    /// The checkout: the manifest, the sources, the repository of the
    /// `cargo` workloads.
    pub workspace: PathBuf,
    /// The cache (`<build directory>/bench-work`).
    pub work: PathBuf,
}

impl Env {
    pub fn workloads(&self) -> PathBuf {
        self.work.join("workloads")
    }
    pub fn scratch(&self) -> PathBuf {
        self.work.join("scratch")
    }
}

/// A workload ready to be linked.
#[derive(Clone, Debug)]
pub struct Prepared {
    pub workload: Workload,
    /// The workload's directory in the cache.
    pub dir: PathBuf,
    /// Where the link runs (the directory the driver ran the linker in).
    pub cwd: PathBuf,
    /// The response file of the captured arguments.
    pub response: PathBuf,
    pub info: WorkloadInfo,
}

/// What [`prepare`] says it is doing.
pub type Log<'a> = &'a (dyn Fn(&str) + Sync);

/// Prepares `w`: from its stamp when the stamp's key is current and the
/// files the link reads are unchanged; by a census alone when only those
/// files changed; else by building it again from nothing.
pub fn prepare(w: &Workload, env: &Env, log: Log<'_>) -> Result<Prepared, String> {
    let dir = env.workloads().join(&w.name);
    let key = stamp_key(w, env)?;
    if let Kind::Ninja(ninja) = &w.kind {
        // Captured from a tree made outside the bench, never rebuilt here.
        return super::tree::prepare(w, ninja, env, &key, log);
    }
    let stamp_path = dir.join("stamp");
    let response = dir.join("response.txt");
    let stamp = std::fs::read_to_string(&stamp_path).ok().and_then(|t| Stamp::parse(&t));
    if let Some(stamp) = stamp.filter(|s| s.key == key && response.is_file()) {
        if stamp.files_unchanged() {
            return Ok(stamp.prepared(w, dir, response));
        }
        log(&format!("{}: the files the link reads changed; counting them again", w.name));
        let stamp = census(w, &dir, &stamp.cwd, &dir, &key, env)?;
        stamp.write(&stamp_path)?;
        return Ok(stamp.prepared(w, dir, response));
    }
    log(&format!("{}: building ({})", w.name, w.kind.name()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .map_err(|e| format!("cannot remove {}: {e}", dir.display()))?;
    }
    for sub in ["build", "tmp", "records", "wrapper"] {
        mkdir(&dir.join(sub))?;
    }
    let capture = build(w, &dir, env, log)?;
    std::fs::write(&response, write_response(&capture.args))
        .map_err(|e| format!("cannot write {}: {e}", response.display()))?;
    let stamp = census(w, &dir, &capture.cwd, &dir, &key, env)?;
    stamp.write(&stamp_path)?;
    Ok(stamp.prepared(w, dir, response))
}

fn mkdir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))
}

/// Finds `name` on `PATH` (or takes an absolute path as it is).
pub fn which(name: &str) -> Result<PathBuf, String> {
    if Path::new(name).is_absolute() {
        return Ok(PathBuf::from(name));
    }
    mortar_testutil::find_tool(name).ok_or_else(|| format!("`{name}` is not on PATH"))
}

/// Runs a tool of a build; a failure is an error naming the step.
pub(super) fn tool(
    what: &str,
    program: &Path,
    configure: impl FnOnce(&mut Command),
    limits: Limits,
) -> Result<Output, String> {
    let output = process::run(program, configure, b"", limits)?;
    if output.end.success() {
        return Ok(output);
    }
    let text = String::from_utf8_lossy(&output.stderr);
    let tail: Vec<&str> = text.lines().rev().take(12).collect();
    let tail: Vec<&str> = tail.into_iter().rev().collect();
    Err(format!("{what}: {}\n{}", output.end, tail.join("\n")))
}

/// The first line a tool prints for `--version`.
pub fn version_line(program: &Path) -> Result<String, String> {
    let limits = Limits { time: Duration::from_mins(1), memory_kib: 4 << 20 };
    let output = tool(
        &format!("{} --version", program.display()),
        program,
        |c| {
            c.arg("--version");
        },
        limits,
    )?;
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text.lines().next().unwrap_or("").trim().to_owned())
}

/// The key a prepared workload is cached under: the manifest's section,
/// the tools that build it, and what is built from.
fn stamp_key(w: &Workload, env: &Env) -> Result<String, String> {
    let mut key = Fnv::default();
    key.field(STAMP_MAGIC.as_bytes());
    key.field(w.text.as_bytes());
    key.field(version_line(&which(&w.driver)?)?.as_bytes());
    key.field(version_line(Path::new(GNU_LD))?.as_bytes());
    match &w.kind {
        Kind::Sources { sources } => {
            for source in sources {
                let path = env.workspace.join(manifest::SOURCES).join(source);
                let bytes = std::fs::read(&path)
                    .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
                key.field(source.as_bytes());
                key.field(&bytes);
            }
        }
        Kind::Synthetic(_) => key.field(&GENERATOR.to_le_bytes()),
        Kind::Cmake(_) => {
            key.field(version_line(&which("cmake")?)?.as_bytes());
            key.field(version_line(&which("make")?)?.as_bytes());
        }
        Kind::Cargo(_) => {
            let rustc = which("rustc")?;
            key.field(version_line(&rustc)?.as_bytes());
        }
        Kind::Ninja(ninja) => {
            let path = env.workspace.join(&ninja.script[0]);
            let bytes =
                std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            key.field(&bytes);
            key.field(version_line(&which("ninja")?)?.as_bytes());
        }
        Kind::Planned { .. } => return Err(format!("{}: a planned workload is not built", w.name)),
    }
    Ok(key.text())
}

/// The environment of every build step: temporary files inside the
/// workload's directory (the names the drivers give them are random, and
/// the link reads some of them), the C locale.
fn build_env(c: &mut Command, dir: &Path) {
    c.env("TMPDIR", dir.join("tmp")).env("LC_ALL", "C");
}

/// Builds `w` in `dir` and returns its captured link.
fn build(w: &Workload, dir: &Path, env: &Env, log: Log<'_>) -> Result<Capture, String> {
    let build = dir.join("build");
    let records = dir.join("records");
    let wrapper = dir.join("wrapper");
    // The cmake and cargo builds need the link's output (make and cargo go
    // on with it); the others are linked by the harness alone.
    let real = matches!(w.kind, Kind::Cmake(_) | Kind::Cargo(_)).then_some(Path::new(GNU_LD));
    write_executable(&wrapper.join("ld"), &wrapper_script(&records, real))?;
    let driver = which(&w.driver)?;
    let capture_env = |c: &mut Command| {
        build_env(c, dir);
        c.env("COMPILER_PATH", &wrapper);
    };
    match &w.kind {
        Kind::Sources { sources } => {
            let mut objects = Vec::new();
            for source in sources {
                let from = env.workspace.join(manifest::SOURCES).join(source);
                std::fs::copy(&from, build.join(source))
                    .map_err(|e| format!("cannot copy {}: {e}", from.display()))?;
                let object = format!(
                    "{}.o",
                    source.rsplit_once('.').map_or(source.as_str(), |(stem, _)| stem)
                );
                tool(
                    &format!("{}: compiling {source}", w.name),
                    &driver,
                    |c| {
                        c.args(&w.cflags)
                            .arg("-c")
                            .arg(source)
                            .arg("-o")
                            .arg(&object)
                            .current_dir(&build);
                        build_env(c, dir);
                    },
                    TOOL_LIMITS,
                )?;
                objects.push(object);
            }
            link_with_driver(w, &driver, &objects, &build, &capture_env)?;
        }
        Kind::Synthetic(request) => {
            let inputs = synthesize(w, request, &driver, &build, dir, log)?;
            link_with_driver(w, &driver, &inputs, &build, &capture_env)?;
        }
        Kind::Cmake(cmake) => build_cmake(w, cmake, &driver, dir, env, log, &capture_env)?,
        Kind::Cargo(cargo) => build_cargo(w, cargo, dir, env, log, &capture_env)?,
        Kind::Ninja(_) => unreachable!("ninja workloads are captured by `tree::prepare`"),
        Kind::Planned { .. } => unreachable!("planned workloads have no stamp key"),
    }
    select_capture(w, &records)
}

/// Writes an executable script. Written before any child of this run
/// starts that could inherit the file open.
pub(super) fn write_executable(path: &Path, text: &str) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("cannot make {} executable: {e}", path.display()))
}

/// The driver's link of `inputs` to `out`, with the recording wrapper.
fn link_with_driver(
    w: &Workload,
    driver: &Path,
    inputs: &[String],
    build: &Path,
    capture_env: &dyn Fn(&mut Command),
) -> Result<(), String> {
    tool(
        &format!("{}: linking with the recording ld", w.name),
        driver,
        |c| {
            c.args(&w.ldflags);
            match inputs.split_first() {
                // The generated inputs after `main.o` in one group.
                Some((first, rest)) if w.group => {
                    c.arg(first).arg("-Wl,--start-group").args(rest).arg("-Wl,--end-group");
                }
                _ => {
                    c.args(inputs);
                }
            }
            c.args(["-o", "out"]).current_dir(build);
            capture_env(c);
        },
        TOOL_LIMITS,
    )
    .map(drop)
}

/// Writes the generated sources, compiles them in batches on a few
/// threads, archives them for the `archives` shape, and returns the link's
/// inputs in their order: `main.o` first.
fn synthesize(
    w: &Workload,
    request: &Request,
    driver: &Path,
    build: &Path,
    dir: &Path,
    log: Log<'_>,
) -> Result<Vec<String>, String> {
    let mut compiled = Vec::new();
    let mut members: Vec<Vec<String>> = Vec::new();
    for index in 0..request.count() {
        let source = request.source(index);
        let path = build.join(&source.name);
        std::fs::write(&path, &source.text)
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        if !source.compiled {
            continue;
        }
        let object =
            format!("{}.o", source.name.rsplit_once('.').map_or(source.name.as_str(), |(s, _)| s));
        if let Some(archive) = source.archive {
            members.resize(members.len().max(archive + 1), Vec::new());
            members[archive].push(object);
        }
        compiled.push(source.name);
    }
    let batches: Vec<&[String]> = compiled.chunks(BATCH).collect();
    log(&format!("{}: compiling {} generated sources", w.name, compiled.len()));
    let next = AtomicUsize::new(0);
    let failure = Mutex::new(None::<String>);
    let workers =
        std::thread::available_parallelism().map_or(1, std::num::NonZero::get).min(MAX_WORKERS);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let at = next.fetch_add(1, Ordering::Relaxed);
                    let Some(batch) = batches.get(at) else { break };
                    if failure.lock().is_ok_and(|f| f.is_some()) {
                        break;
                    }
                    let what =
                        format!("{}: compiling {} ({} files)", w.name, batch[0], batch.len());
                    let result = tool(
                        &what,
                        driver,
                        |c| {
                            c.args(&w.cflags).arg("-c").args(*batch).current_dir(build);
                            build_env(c, dir);
                        },
                        TOOL_LIMITS,
                    );
                    if let Err(e) = result {
                        if let Ok(mut f) = failure.lock() {
                            f.get_or_insert(e);
                        }
                        break;
                    }
                }
            });
        }
    });
    if let Some(e) = failure.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner) {
        return Err(e);
    }
    let main = "main.o".to_owned();
    if !matches!(request.shape, Shape::Archives | Shape::Library | Shape::Line) {
        let mut inputs = vec![main.clone()];
        inputs.extend(
            compiled
                .iter()
                .map(|s| format!("{}.o", s.rsplit_once('.').map_or(s.as_str(), |(s, _)| s)))
                .filter(|o| *o != main),
        );
        return Ok(inputs);
    }
    let ar = which("ar")?;
    let mut inputs = vec![main];
    for (archive, objects) in members.iter().enumerate() {
        let name = format!("lib{archive:05}.a");
        tool(
            &format!("{}: archiving {name}", w.name),
            &ar,
            |c| {
                c.arg("rcsD").arg(&name).args(objects).current_dir(build);
                build_env(c, dir);
            },
            TOOL_LIMITS,
        )?;
        inputs.push(name);
    }
    Ok(inputs)
}

/// Fetches `cmake.url` into the cache's downloads (once; checked by its
/// SHA-256) and unpacks it (once per hash) for the workloads that share it.
fn fetch_and_unpack(
    w: &Workload,
    cmake: &Cmake,
    env: &Env,
    log: Log<'_>,
) -> Result<PathBuf, String> {
    Ok(fetch_archive(&w.name, &cmake.url, &cmake.sha256, &env.work, log)?.join(&cmake.root))
}

/// Fetches `url` into `<work>/downloads` (named by the URL's last
/// component; only when the file is missing or its SHA-256 is not
/// `sha256`), checks it against `sha256`, and unpacks it once into
/// `<work>/src/<first 16 digits of sha256>` (`tar -xf`, which reads gzip,
/// xz and bzip2 alike), without the members [`members_not_unpacked`]
/// holds back (the skipped ones are logged, the links are made here). Returns the unpacked directory. `what` names the
/// workload or project in the messages. Shared by the bench and the
/// real-builds lane (`spec/harness.md` "The cache", "realbuild").
pub fn fetch_archive(
    what: &str,
    url: &str,
    sha256: &str,
    work: &Path,
    log: Log<'_>,
) -> Result<PathBuf, String> {
    let downloads = work.join("downloads");
    let unpacked = work.join("src").join(&sha256[..16.min(sha256.len())]);
    let done = unpacked.join(".unpacked");
    if done.is_file() {
        return Ok(unpacked);
    }
    mkdir(&downloads)?;
    let file = url.rsplit('/').next().filter(|f| !f.is_empty()).unwrap_or("download");
    let archive = downloads.join(file);
    let sha256sum = which("sha256sum")?;
    let digest = |path: &Path| -> Result<String, String> {
        let out = tool(
            "sha256sum",
            &sha256sum,
            |c| {
                c.arg(path);
            },
            TOOL_LIMITS,
        )?;
        Ok(String::from_utf8_lossy(&out.stdout).split_whitespace().next().unwrap_or("").to_owned())
    };
    if !archive.is_file() || digest(&archive)? != sha256 {
        log(&format!("{what}: fetching {url}"));
        let part = downloads.join(format!("{file}.part"));
        tool(
            &format!("{what}: fetching {url}"),
            &which("curl")?,
            |c| {
                c.args(["-fsSL", "--retry", "3", "-o"]).arg(&part).arg(url);
            },
            FETCH_LIMITS,
        )?;
        let got = digest(&part)?;
        if got != sha256 {
            let _ = std::fs::remove_file(&part);
            return Err(format!("{what}: {url} has SHA-256 {got}, the manifest pins {sha256}"));
        }
        std::fs::rename(&part, &archive)
            .map_err(|e| format!("cannot rename {}: {e}", part.display()))?;
    }
    if unpacked.exists() {
        std::fs::remove_dir_all(&unpacked)
            .map_err(|e| format!("cannot remove {}: {e}", unpacked.display()))?;
    }
    mkdir(&unpacked)?;
    let tar = which("tar")?;
    let listing = tool(
        &format!("{what}: listing {file}"),
        &tar,
        |c| {
            c.args(["--numeric-owner", "--quoting-style=literal", "-tvf"]).arg(&archive);
        },
        TOOL_LIMITS,
    )?;
    let Held { skipped, links } = members_not_unpacked(&String::from_utf8_lossy(&listing.stdout))
        .map_err(|e| format!("{what}: {file}: {e}"))?;
    if skipped.len() + links.len() > MAX_SKIPPED {
        return Err(format!(
            "{what}: {file} has {} members that tar is not given (limit {MAX_SKIPPED})",
            skipped.len() + links.len()
        ));
    }
    for member in &skipped {
        log(&format!("{what}: not unpacked (unreadable by its owner, or not a file): {member}"));
    }
    tool(
        &format!("{what}: unpacking {file}"),
        &tar,
        |c| {
            c.arg("-xf").arg(&archive).arg("-C").arg(&unpacked).arg("--no-same-owner");
            // The two options must precede the patterns they govern.
            c.args(["--anchored", "--no-wildcards"]);
            for member in skipped.iter().chain(links.iter().map(|(name, _)| name)) {
                c.arg(format!("--exclude={member}"));
            }
        },
        TOOL_LIMITS,
    )?;
    for (name, target) in &links {
        match link_place(&unpacked, name) {
            Some(place) => std::os::unix::fs::symlink(target, &place)
                .map_err(|e| format!("{what}: cannot make the link {}: {e}", place.display()))?,
            None => {
                log(&format!("{what}: not unpacked (a link outside what was unpacked): {name}"))
            }
        }
    }
    std::fs::write(&done, b"").map_err(|e| format!("cannot write {}: {e}", done.display()))?;
    Ok(unpacked)
}

/// The most members [`members_not_unpacked`] may name before the archive
/// is refused (each becomes one argument of `tar`).
const MAX_SKIPPED: usize = 4096;

/// The members of an archive that `tar` is not given
/// ([`members_not_unpacked`]).
#[derive(Debug, Default, PartialEq, Eq)]
struct Held {
    /// Not unpacked at all, by name without a trailing `/`.
    skipped: Vec<String>,
    /// Symbolic links made after `tar` has run: name and target.
    links: Vec<(String, String)>,
}

/// The members of an archive that `tar` must not create, from the text
/// of `tar --numeric-owner --quoting-style=literal -tvf`. On a shared
/// mount a file without owner read permission can be created but never
/// read, changed or removed again (as root too), and it breaks whatever
/// walks the tree. Two things make one:
/// - a symbolic link whose target is absolute or has a `..` component:
///   GNU tar first writes a mode-000 placeholder file in its place and
///   swaps the link in when the archive is done, and the swap fails
///   where the placeholder cannot be examined (LLVM's archive has five
///   such links; that is what was left behind). These are `links`: the
///   caller makes them itself, after `tar`;
/// - a member whose own mode says so: a regular file or a hard link
///   without owner read permission, a directory without owner read or
///   search permission (and with it everything below). These, a device
///   and a FIFO are `skipped`: not unpacked.
///
/// Other symbolic links are `tar`'s. A line that does not have the
/// listing's form is the error.
fn members_not_unpacked(listing: &str) -> Result<Held, String> {
    let mut out = Held::default();
    for line in listing.lines() {
        let bad = || format!("a line of tar's listing is not understood: {line:?}");
        let mode = line.get(..10).ok_or_else(bad)?.as_bytes();
        // owner/group, size, date, time; then one space and the name.
        let mut rest = line.get(10..).ok_or_else(bad)?;
        for _ in 0..4 {
            rest = rest.trim_start_matches(' ');
            let end = rest.find(' ').filter(|&e| e > 0).ok_or_else(bad)?;
            rest = &rest[end..];
        }
        let name = rest.strip_prefix(' ').ok_or_else(bad)?;
        let read = mode[1] == b'r';
        let search = matches!(mode[3], b'x' | b's');
        let (skip, name) = match mode[0] {
            b'l' => {
                let (name, target) = name.split_once(" -> ").ok_or_else(bad)?;
                if name.is_empty() || target.is_empty() {
                    return Err(bad());
                }
                if target.starts_with('/') || target.split('/').any(|part| part == "..") {
                    out.links.push((name.to_owned(), target.to_owned()));
                }
                continue;
            }
            b'-' | b'C' => (!read, name),
            b'h' => (!read, name.rsplit_once(" link to ").ok_or_else(bad)?.0),
            b'd' => (!read || !search, name),
            b'c' | b'b' | b'p' => (true, name),
            _ => return Err(bad()),
        };
        let name = name.trim_end_matches('/');
        if name.is_empty() {
            return Err(bad());
        }
        if skip {
            out.skipped.push(name.to_owned());
        }
    }
    Ok(out)
}

/// Where the link member `name` goes under `root`, when that is a place
/// inside what `tar` unpacked: the name is relative with plain
/// components, every directory above it exists and is a directory itself
/// (not a link to one), and nothing is at the place yet.
fn link_place(root: &Path, name: &str) -> Option<PathBuf> {
    let mut place = root.to_path_buf();
    let mut parts = name.split('/').peekable();
    while let Some(part) = parts.next() {
        if part.is_empty() || part == "." || part == ".." {
            return None;
        }
        if !std::fs::symlink_metadata(&place).ok()?.file_type().is_dir() {
            return None;
        }
        place.push(part);
        if parts.peek().is_none() {
            return std::fs::symlink_metadata(&place).is_err().then_some(place);
        }
    }
    None
}

fn build_cmake(
    w: &Workload,
    cmake: &Cmake,
    driver: &Path,
    dir: &Path,
    env: &Env,
    log: Log<'_>,
    capture_env: &dyn Fn(&mut Command),
) -> Result<(), String> {
    let source = fetch_and_unpack(w, cmake, env, log)?;
    let build = dir.join("build");
    let flags = w.cflags.join(" ");
    log(&format!("{}: configuring", w.name));
    tool(
        &format!("{}: cmake", w.name),
        &which("cmake")?,
        |c| {
            c.arg("-S").arg(&source).arg("-B").arg(&build).args(["-G", "Unix Makefiles"]);
            c.arg(format!("-DCMAKE_CXX_COMPILER={}", driver.display()));
            c.arg(format!("-DCMAKE_CXX_FLAGS={flags}")).arg(format!("-DCMAKE_C_FLAGS={flags}"));
            c.args(&cmake.options).current_dir(&build);
            build_env(c, dir);
        },
        TOOL_LIMITS,
    )?;
    // make is a client of the machine's pool (MAKEFLAGS is inherited) and
    // takes its parallelism from it; without a pool it runs serially.
    let make = which("make")?;
    if !cmake.prebuild.is_empty() {
        log(&format!("{}: make {} (through the jobserver pool)", w.name, cmake.prebuild.join(" ")));
        tool(
            &format!("{}: make {}", w.name, cmake.prebuild.join(" ")),
            &make,
            |c| {
                c.args(&cmake.prebuild).current_dir(&build);
                build_env(c, dir);
            },
            MAKE_LIMITS,
        )?;
    }
    log(&format!("{}: make {} with the recording ld", w.name, cmake.target));
    tool(
        &format!("{}: make {}", w.name, cmake.target),
        &make,
        |c| {
            c.arg(&cmake.target).current_dir(&build);
            capture_env(c);
        },
        MAKE_LIMITS,
    )
    .map(drop)
}

fn build_cargo(
    w: &Workload,
    cargo: &Cargo,
    dir: &Path,
    env: &Env,
    log: Log<'_>,
    capture_env: &dyn Fn(&mut Command),
) -> Result<(), String> {
    let source = dir.join("src");
    mkdir(&source)?;
    let tarball = dir.join("src.tar");
    tool(
        &format!("{}: git archive {}", w.name, cargo.rev),
        &which("git")?,
        |c| {
            c.arg("-C")
                .arg(&env.workspace)
                .args(["archive", "--format=tar", "-o"])
                .arg(&tarball)
                .arg(&cargo.rev);
        },
        TOOL_LIMITS,
    )?;
    tool(
        &format!("{}: unpacking {}", w.name, cargo.rev),
        &which("tar")?,
        |c| {
            c.arg("-xf").arg(&tarball).arg("-C").arg(&source);
        },
        TOOL_LIMITS,
    )?;
    let _ = std::fs::remove_file(&tarball);
    log(&format!(
        "{}: cargo rustc -p {} --bin {} at {}",
        w.name, cargo.package, cargo.bin, cargo.rev
    ));
    let mut command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command
        .args(["rustc", "--offline", "--locked", "--quiet", "--manifest-path"])
        .arg(source.join("Cargo.toml"))
        .args(["-p", &cargo.package, "--bin", &cargo.bin, "--"])
        .args(&cargo.rustc_args)
        .current_dir(&source)
        .env("CARGO_TARGET_DIR", dir.join("target"))
        // The dev profile as cargo has it: full debug information.
        .env("CARGO_PROFILE_DEV_DEBUG", "true")
        .env("CARGO_INCREMENTAL", "0");
    for flags in ["RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS", "RUSTC_WRAPPER"]
    {
        command.env_remove(flags);
    }
    capture_env(&mut command);
    // cargo is a client of the pool itself (spec/harness.md "Jobserver").
    let (status, stderr) = process::run_step(&mut command, false)
        .map_err(|e| format!("{}: cannot run cargo: {e}", w.name))?;
    if !status.success() {
        return Err(format!("{}: cargo rustc: {status}\n{stderr}", w.name));
    }
    Ok(())
}

/// The one recorded link whose output's basename matches the workload's
/// `capture` pattern.
pub(super) fn select_capture(w: &Workload, records: &Path) -> Result<Capture, String> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(records)
        .map_err(|e| format!("cannot list {}: {e}", records.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    let mut seen = Vec::new();
    let mut found = Vec::new();
    for path in entries {
        let bytes =
            std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let capture =
            Capture::parse(&bytes).map_err(|e| format!("{}: {}: {e}", w.name, path.display()))?;
        let output = capture.output().unwrap_or("a.out");
        let base = output.rsplit('/').next().unwrap_or(output).to_owned();
        if glob_match(&w.capture, &base) {
            found.push(capture);
        }
        seen.push(base);
    }
    match found.len() {
        1 => Ok(found.remove(0)),
        n => Err(format!(
            "{}: {n} recorded links match `{}` (recorded: {}); exactly one must",
            w.name,
            w.capture,
            if seen.is_empty() { "none".to_owned() } else { seen.join(", ") }
        )),
    }
}

/// An argument or a path with the workload's directory written `$W`, and
/// the names the drivers give temporary files (`ccXXXXXX.res`,
/// `rustcXXXXXX`) under it written `*`: so that the workload's hash is
/// the same on every preparation and in every checkout.
pub fn normalize(text: &str, dir: &Path) -> String {
    let dir = dir.to_string_lossy();
    if dir.is_empty() || !text.contains(dir.as_ref()) {
        return text.to_owned();
    }
    let replaced = text.replace(dir.as_ref(), "$W");
    replaced
        .split('/')
        .map(|part| if is_temporary_name(part) { "*" } else { part })
        .collect::<Vec<_>>()
        .join("/")
}

/// `cc` and six letters or digits, with an extension or none; `rustc` and
/// six letters or digits.
fn is_temporary_name(part: &str) -> bool {
    let random = |s: &str| s.len() == 6 && s.bytes().all(|b| b.is_ascii_alphanumeric());
    if let Some(rest) = part.strip_prefix("rustc") {
        return random(rest);
    }
    part.strip_prefix("cc")
        .is_some_and(|rest| random(rest.split_once('.').map_or(rest, |(stem, _)| stem)))
}

/// What a file the link reads is, by its first bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Object,
    Archive,
    Shared,
    Other,
}

/// `\x7fELF` with `e_type` 1 (relocatable) or 3 (shared), `!<arch>\n` or
/// `!<thin>\n`, else other (a linker script, a plugin's file).
pub fn classify(head: &[u8]) -> FileKind {
    if head.starts_with(b"!<arch>\n") || head.starts_with(b"!<thin>\n") {
        return FileKind::Archive;
    }
    if head.len() >= 18 && head.starts_with(b"\x7fELF") {
        let e_type = if head[5] == 2 {
            u16::from_be_bytes([head[16], head[17]])
        } else {
            u16::from_le_bytes([head[16], head[17]])
        };
        return match e_type {
            1 => FileKind::Object,
            3 => FileKind::Shared,
            _ => FileKind::Other,
        };
    }
    FileKind::Other
}

/// The number of entries of the `.symtab` of the 64-bit little-endian ELF
/// file at `path` (0 without one).
pub fn symtab_entries(path: &Path) -> Result<u64, String> {
    let cannot = |e: std::io::Error| format!("cannot read {}: {e}", path.display());
    let mut file = std::fs::File::open(path).map_err(cannot)?;
    let mut header = [0u8; 64];
    file.read_exact(&mut header).map_err(cannot)?;
    if !header.starts_with(b"\x7fELF") || header[4] != 2 || header[5] != 1 {
        return Err(format!("{} is not a 64-bit little-endian ELF file", path.display()));
    }
    let u16_at = |b: &[u8], at: usize| u64::from(u16::from_le_bytes([b[at], b[at + 1]]));
    let u32_at = |b: &[u8], at: usize| u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    let u64_at = |b: &[u8], at: usize| {
        let mut v = [0u8; 8];
        v.copy_from_slice(&b[at..at + 8]);
        u64::from_le_bytes(v)
    };
    let (shoff, shentsize, mut shnum) =
        (u64_at(&header, 0x28), u16_at(&header, 0x3a), u16_at(&header, 0x3c));
    if shoff == 0 || shentsize < 64 {
        return Ok(0);
    }
    let mut section = vec![0u8; shentsize as usize];
    let mut read_section = |index: u64, buf: &mut [u8]| -> Result<(), String> {
        file.seek(SeekFrom::Start(shoff + index * shentsize)).map_err(cannot)?;
        file.read_exact(buf).map_err(cannot)
    };
    if shnum == 0 {
        // More than 0xff00 sections: the number is in section 0's size.
        read_section(0, &mut section)?;
        shnum = u64_at(&section, 32);
    }
    let mut entries = 0;
    for index in 0..shnum {
        read_section(index, &mut section)?;
        let (kind, size, entsize) =
            (u32_at(&section, 4), u64_at(&section, 32), u64_at(&section, 56));
        if kind == 2 && entsize > 0 {
            entries += size / entsize;
        }
    }
    Ok(entries)
}

/// A workload's stamp: the key it was prepared under, where its link runs,
/// its census, and the files the census read with their size and time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub key: String,
    pub cwd: PathBuf,
    pub info: WorkloadInfo,
    /// Size, modification time in nanoseconds, path.
    pub files: Vec<(u64, u128, PathBuf)>,
}

impl Stamp {
    pub fn to_text(&self) -> String {
        let i = &self.info;
        let mut text = format!("{STAMP_MAGIC}\nkey\t{}\ncwd\t{}\n", self.key, self.cwd.display());
        let _ = writeln!(
            text,
            "census\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            i.objects,
            i.archives,
            i.shared,
            i.others,
            i.input_bytes,
            i.symbols,
            i.output_bytes,
            i.input_hash
        );
        for (size, time, path) in &self.files {
            let _ = writeln!(text, "file\t{size}\t{time}\t{}", path.display());
        }
        text
    }

    /// `None` for anything but a stamp this version wrote.
    pub fn parse(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        if lines.next()? != STAMP_MAGIC {
            return None;
        }
        let key = lines.next()?.strip_prefix("key\t")?.to_owned();
        let cwd = PathBuf::from(lines.next()?.strip_prefix("cwd\t")?);
        let census: Vec<&str> = lines.next()?.strip_prefix("census\t")?.split('\t').collect();
        let &[objects, archives, shared, others, input_bytes, symbols, output_bytes, hash] =
            census.as_slice()
        else {
            return None;
        };
        let info = WorkloadInfo {
            name: String::new(),
            class: String::new(),
            objects: objects.parse().ok()?,
            archives: archives.parse().ok()?,
            shared: shared.parse().ok()?,
            others: others.parse().ok()?,
            input_bytes: input_bytes.parse().ok()?,
            symbols: symbols.parse().ok()?,
            output_bytes: output_bytes.parse().ok()?,
            input_hash: hash.to_owned(),
        };
        let mut files = Vec::new();
        for line in lines {
            let mut fields = line.strip_prefix("file\t")?.splitn(3, '\t');
            let size = fields.next()?.parse().ok()?;
            let time = fields.next()?.parse().ok()?;
            files.push((size, time, PathBuf::from(fields.next()?)));
        }
        Some(Self { key, cwd, info, files })
    }

    pub(super) fn write(&self, path: &Path) -> Result<(), String> {
        std::fs::write(path, self.to_text())
            .map_err(|e| format!("cannot write {}: {e}", path.display()))
    }

    /// Every file the census read has its recorded size and time.
    pub(super) fn files_unchanged(&self) -> bool {
        self.files
            .iter()
            .all(|(size, time, path)| file_stat(path).is_some_and(|now| now == (*size, *time)))
    }

    pub(super) fn prepared(self, w: &Workload, dir: PathBuf, response: PathBuf) -> Prepared {
        let info = WorkloadInfo { name: w.name.clone(), class: w.class.clone(), ..self.info };
        Prepared { workload: w.clone(), dir, cwd: self.cwd, response, info }
    }
}

fn file_stat(path: &Path) -> Option<(u64, u128)> {
    let meta = std::fs::metadata(path).ok()?;
    let time = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    Some((meta.len(), time))
}

/// The census of a workload: GNU ld replays the captured link once with
/// `--dependency-file`; the files it names are counted by kind, their
/// bytes summed, and the normalized arguments and every file's contents
/// hashed. A workload GNU ld cannot replay is not admitted.
pub(super) fn census(
    w: &Workload,
    dir: &Path,
    cwd: &Path,
    root: &Path,
    key: &str,
    env: &Env,
) -> Result<Stamp, String> {
    let scratch = env.scratch();
    mkdir(&scratch)?;
    let output = scratch.join(format!("census-{}.out", w.name));
    let depfile = scratch.join(format!("census-{}.d", w.name));
    let response = dir.join("response.txt");
    let _ = std::fs::remove_file(&output);
    let replay = process::run(
        Path::new(GNU_LD),
        |c| {
            c.arg(format!("@{}", response.display())).arg("-o").arg(&output);
            c.arg(format!("--dependency-file={}", depfile.display())).current_dir(cwd);
        },
        b"",
        CENSUS_LIMITS,
    )?;
    let cleanup = || {
        let _ = std::fs::remove_file(&output);
        let _ = std::fs::remove_file(&depfile);
    };
    if !replay.end.success() {
        cleanup();
        return Err(format!(
            "{}: not admitted: GNU ld cannot replay the captured link ({}: {})",
            w.name,
            replay.end,
            replay.first_error_line()
        ));
    }
    let result = count_inputs(w, root, cwd, &response, &output, &depfile);
    cleanup();
    let (info, files) = result?;
    Ok(Stamp { key: key.to_owned(), cwd: cwd.to_owned(), info, files })
}

type Counted = (WorkloadInfo, Vec<(u64, u128, PathBuf)>);

fn count_inputs(
    w: &Workload,
    dir: &Path,
    cwd: &Path,
    response: &Path,
    output: &Path,
    depfile: &Path,
) -> Result<Counted, String> {
    let text = std::fs::read_to_string(depfile)
        .map_err(|e| format!("cannot read {}: {e}", depfile.display()))?;
    let mut paths: Vec<PathBuf> = Vec::new();
    for name in parse_depfile(&text)? {
        let path = cwd.join(&name);
        let path = std::fs::canonicalize(&path)
            .map_err(|e| format!("{}: {}: {e}", w.name, path.display()))?;
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    let args = std::fs::read_to_string(response)
        .map_err(|e| format!("cannot read {}: {e}", response.display()))?;
    let mut hash = Fnv::default();
    for arg in super::response::parse_response(&args)? {
        hash.field(normalize(&arg, dir).as_bytes());
    }
    let mut info = WorkloadInfo {
        name: w.name.clone(),
        class: w.class.clone(),
        objects: 0,
        archives: 0,
        shared: 0,
        others: 0,
        input_bytes: 0,
        symbols: symtab_entries(output)?,
        output_bytes: std::fs::metadata(output)
            .map_err(|e| format!("cannot read {}: {e}", output.display()))?
            .len(),
        input_hash: String::new(),
    };
    let mut files = Vec::new();
    let mut buffer = vec![0u8; 1 << 20];
    for path in paths {
        let cannot = |e: std::io::Error| format!("cannot read {}: {e}", path.display());
        let mut file = std::fs::File::open(&path).map_err(cannot)?;
        hash.field(normalize(&path.to_string_lossy(), dir).as_bytes());
        let mut first = true;
        let mut size = 0u64;
        loop {
            let n = file.read(&mut buffer).map_err(cannot)?;
            if n == 0 {
                break;
            }
            if first {
                match classify(&buffer[..n]) {
                    FileKind::Object => info.objects += 1,
                    FileKind::Archive => info.archives += 1,
                    FileKind::Shared => info.shared += 1,
                    FileKind::Other => info.others += 1,
                }
                first = false;
            }
            hash.update(&buffer[..n]);
            size += n as u64;
        }
        if first {
            info.others += 1;
        }
        hash.update(&[0]);
        info.input_bytes += size;
        let (len, time) =
            file_stat(&path).ok_or_else(|| format!("cannot read {}", path.display()))?;
        files.push((len, time, path));
    }
    info.input_hash = hash.text();
    Ok((info, files))
}

/// The cache's lock: one bench run at a time uses a cache (worktrees share
/// it). Holds `<work>/lock` with this process's id; a lock whose process
/// is gone is taken over.
#[derive(Debug)]
pub struct Lock(PathBuf);

impl Lock {
    pub fn take(work: &Path) -> Result<Self, String> {
        mkdir(work)?;
        let path = work.join("lock");
        for _ in 0..2 {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    use std::io::Write;
                    let _ = write!(file, "{}", std::process::id());
                    return Ok(Self(path));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let holder = std::fs::read_to_string(&path).unwrap_or_default();
                    let pid = holder.trim();
                    if !pid.is_empty() && Path::new("/proc").join(pid).exists() {
                        return Err(format!(
                            "another bench run (process {pid}) is using {}; wait for it",
                            work.display()
                        ));
                    }
                    let _ = std::fs::remove_file(&path);
                }
                Err(e) => return Err(format!("cannot create {}: {e}", path.display())),
            }
        }
        Err(format!("cannot take {}", path.display()))
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// `text` as one `/bin/sh` word (for messages that show a command).
pub fn shell_word(text: &str) -> String {
    if !text.is_empty()
        && text.bytes().all(|b| b.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&b))
    {
        text.to_owned()
    } else {
        sh_quote(text)
    }
}

// spec/harness.md "bench", "Capture", "Census" and "The cache".
#[cfg(test)]
mod tests {
    use super::*;
    use crate::bench::record::tests::info;

    // "Census": the workload's directory and the drivers' temporary names
    // do not reach the hash.
    #[test]
    fn members_tar_must_not_create() {
        // GNU tar 1.35, `--numeric-owner --quoting-style=literal -tvf`.
        let listing = "\
drwxr-xr-x 0/0               0 2026-11-17 16:45 top/
prw-r--r-- 0/0               0 2026-11-17 16:45 top/pipe
-rw-r--r-- 0/0               2 2026-11-17 16:45 top/ok.txt
lrwxrwxrwx 0/0               0 2026-11-17 16:45 top/link -> ok.txt
lrwxrwxrwx 0/0               0 2026-11-17 16:45 top/sub/up -> ../ok.txt
lrwxrwxrwx 0/0               0 2026-11-17 16:45 top/sub/mid -> a/../b
lrwxrwxrwx 0/0               0 2026-11-17 16:45 top/sub/abs -> /bin/sh
lrwxrwxrwx 0/0               0 2026-11-17 16:45 top/sub/dots -> ..x/y..
drwxr-xr-x 0/0               0 2026-11-17 16:45 top/sub/
--w------- 0/0               2 2026-11-17 16:45 top/sub/has space.txt
---------- 0/0               2 2026-11-17 16:45 top/sub/hard
h--------- 0/0               0 2026-11-17 16:45 top/sub/secret link to top/sub/hard
d--------- 0/0               0 2026-11-17 16:45 top/locked/
drw------- 0/0               0 2026-11-17 16:45 top/nosearch/
-rw-r--r-- 0/0               2 2026-11-17 16:45 top/locked/inner.txt
crw-rw-rw- 0/0             1,3 2026-11-17 16:45 top/null
-rw-r--r-- 1000/1000 123456789 2026-11-17 16:45 top/big
";
        let held = members_not_unpacked(listing).unwrap();
        assert_eq!(
            held.skipped,
            [
                "top/pipe",
                "top/sub/has space.txt",
                "top/sub/hard",
                "top/sub/secret",
                "top/locked",
                "top/nosearch",
                "top/null",
            ]
        );
        let links: Vec<(&str, &str)> =
            held.links.iter().map(|(n, t)| (n.as_str(), t.as_str())).collect();
        assert_eq!(
            links,
            [("top/sub/up", "../ok.txt"), ("top/sub/mid", "a/../b"), ("top/sub/abs", "/bin/sh")]
        );
        assert_eq!(members_not_unpacked("").unwrap(), Held::default());
        for line in [
            "short",
            "?rw-r--r-- 0/0 2 2026-11-17 16:45 x",
            "-rw-r--r-- 0/0 2 2026-11-17",
            "h--------- 0/0 0 2026-11-17 16:45 a",
            "lrwxrwxrwx 0/0 0 2026-11-17 16:45 a",
        ] {
            assert!(members_not_unpacked(line).is_err(), "{line}");
        }
    }

    #[test]
    fn a_held_link_is_made_only_inside_the_unpacked_tree() {
        let root = std::env::temp_dir().join(format!("mortar-link-place-{}", std::process::id()));
        std::fs::create_dir_all(root.join("top/sub")).unwrap();
        std::os::unix::fs::symlink("sub", root.join("top/alias")).unwrap();
        std::fs::write(root.join("top/taken"), b"").unwrap();
        assert_eq!(link_place(&root, "top/sub/up"), Some(root.join("top/sub/up")));
        for name in
            ["top/taken", "top/alias/x", "top/gone/x", "../x", "/x", "top//x", "top/./x", ""]
        {
            assert_eq!(link_place(&root, name), None, "{name}");
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn arguments_are_normalized() {
        let dir = Path::new("/c/workloads/hello-c");
        assert_eq!(
            normalize("-plugin-opt=-fresolution=/c/workloads/hello-c/tmp/ccOZNzXY.res", dir),
            "-plugin-opt=-fresolution=$W/tmp/*"
        );
        assert_eq!(
            normalize("/c/workloads/hello-c/target/debug/deps/rustcSFeQ1s/symbols.o", dir),
            "$W/target/debug/deps/*/symbols.o"
        );
        assert_eq!(
            normalize("-L/c/workloads/hello-c/target/debug/deps/rustcSFeQ1s/raw-dylibs", dir),
            "-L$W/target/debug/deps/*/raw-dylibs"
        );
        assert_eq!(normalize("/c/workloads/hello-c/build/hello.o", dir), "$W/build/hello.o");
        // Outside the directory nothing changes, not even a name that looks random.
        assert_eq!(normalize("/usr/lib/ccABCDEF.o", dir), "/usr/lib/ccABCDEF.o");
        assert_eq!(normalize("-lc", dir), "-lc");
        assert!(
            is_temporary_name("ccOZNzXY.res")
                && is_temporary_name("ccabcdef")
                && is_temporary_name("rustcSFeQ1s")
        );
        assert!(
            !is_temporary_name("ccache")
                && !is_temporary_name("crtbeginS.o")
                && !is_temporary_name("rustc")
        );
    }

    // "Census": files are counted by their first bytes.
    #[test]
    fn files_are_classified() {
        let mut elf = b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0\x01\0".to_vec();
        assert_eq!(classify(&elf), FileKind::Object);
        elf[16] = 3;
        assert_eq!(classify(&elf), FileKind::Shared);
        elf[16] = 2;
        assert_eq!(classify(&elf), FileKind::Other);
        assert_eq!(classify(b"!<arch>\nmember"), FileKind::Archive);
        assert_eq!(classify(b"!<thin>\n"), FileKind::Archive);
        assert_eq!(classify(b"/* GNU ld script */\nGROUP ( libc.so.6 )"), FileKind::Other);
        assert_eq!(classify(b"\x7fELF"), FileKind::Other);
    }

    // "Census": the symbols of an output are its `.symtab`'s entries,
    // also with extended section numbering.
    #[test]
    fn symbol_tables_are_counted() {
        let scratch = mortar_testutil::Scratch::new("bench-symtab");
        let elf = |shnum_in_header: u16, sections: &[(u32, u64, u64)]| {
            let mut bytes = vec![0u8; 64];
            bytes[..6].copy_from_slice(b"\x7fELF\x02\x01");
            bytes[0x28..0x30].copy_from_slice(&64u64.to_le_bytes());
            bytes[0x3a..0x3c].copy_from_slice(&64u16.to_le_bytes());
            bytes[0x3c..0x3e].copy_from_slice(&shnum_in_header.to_le_bytes());
            for &(kind, size, entsize) in sections {
                let mut section = [0u8; 64];
                section[4..8].copy_from_slice(&kind.to_le_bytes());
                section[32..40].copy_from_slice(&size.to_le_bytes());
                section[56..64].copy_from_slice(&entsize.to_le_bytes());
                bytes.extend_from_slice(&section);
            }
            bytes
        };
        let plain = scratch.write("plain", elf(3, &[(0, 0, 0), (3, 100, 0), (2, 24 * 7, 24)]));
        assert_eq!(symtab_entries(&plain).unwrap(), 7);
        // Section 0's size holds the number of sections.
        let extended =
            scratch.write("extended", elf(0, &[(0, 3, 0), (2, 24 * 5, 24), (2, 24, 24)]));
        assert_eq!(symtab_entries(&extended).unwrap(), 6);
        let stripped = scratch.write("stripped", elf(1, &[(0, 0, 0)]));
        assert_eq!(symtab_entries(&stripped).unwrap(), 0);
        let not_elf = scratch.write("script", vec![b'x'; 64]);
        assert!(symtab_entries(&not_elf).unwrap_err().contains("not a 64-bit"));
    }

    // "The cache": a stamp is read back as it was written.
    #[test]
    fn stamps_round_trip() {
        let mut census = info("w", "small");
        (census.name, census.class) = (String::new(), String::new());
        let stamp = Stamp {
            key: "fnv1a64:0011223344556677".into(),
            cwd: "/c/w/build".into(),
            info: census,
            files: vec![(10, 1_700_000_000_123_456_789, "/usr/lib/a b.o".into())],
        };
        let text = stamp.to_text();
        assert!(
            text.starts_with("# mortar-bench-stamp 1\nkey\tfnv1a64:0011223344556677\n"),
            "{text}"
        );
        assert_eq!(Stamp::parse(&text), Some(stamp.clone()));
        assert_eq!(Stamp::parse(&text.replace("stamp 1", "stamp 2")), None);
        assert_eq!(Stamp::parse(&text.replace("census\t4", "census\tx")), None);
        // A file that is not there is a change.
        assert!(!stamp.files_unchanged());
        let none = Stamp { files: Vec::new(), ..stamp };
        assert!(none.files_unchanged());
    }

    // "The cache": one run at a time; a lock whose process is gone is
    // taken over.
    #[test]
    fn the_cache_is_locked() {
        let scratch = mortar_testutil::Scratch::new("bench-lock");
        let lock = Lock::take(scratch.path()).unwrap();
        let busy = Lock::take(scratch.path()).unwrap_err();
        assert!(busy.contains(&format!("process {}", std::process::id())), "{busy}");
        drop(lock);
        assert!(!scratch.join("lock").exists());
        // No process has this number (above the kernel's pid limit).
        std::fs::write(scratch.join("lock"), "99999999").unwrap();
        drop(Lock::take(scratch.path()).unwrap());
    }
}

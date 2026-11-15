//! `cargo xtask mason setup`: what the libc++ lane needs, made in the lane
//! directory (`super::lane_dir`), each step skipped when its result is
//! there (`tests/mason/README.md` "Setup"):
//!
//! 1. `debs/`: Debian's `libc++-22-dev`, `libc++abi-22-dev` and
//!    `libunwind-22-dev` (`apt-get download`; nothing is installed).
//! 2. `libcxx-22-static/`: their headers, module sources and static
//!    archives, extracted without the packages' symlinks (the archives
//!    are moved from `usr/lib/llvm-22/lib`'s links' targets in
//!    `usr/lib/<triple>`). Only regular members are extracted: a relative
//!    symlink cannot be created on some bind mounts, and the `.so`
//!    linker scripts would make the driver's trailing `-lc++` ask for the
//!    `libc++.so.1` runtime that is not installed.
//! 3. `libcxx-22-stripped/`: the four archives with `.ARM.attributes` and
//!    `.deplibs` removed (`objcopy -R`).
//! 4. `llvm-project-<tag>/`: libc++'s tests, by kiln's
//!    `scripts/fetch-libcxx-tests.sh`.
//! 5. `kiln-src/`: kiln's tree at the submodule's commit (`git archive`,
//!    which reads the submodule and writes nothing in it), its
//!    `.toolchain/libcxx-22-root` an absolute link to step 2, and its
//!    `xtask` built into `kiln-target/` (kiln's runner finds the libc++
//!    tree through its own source root).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use crate::process::Limits;

use super::run_in;

/// The libc++ release kiln pins (`kiln/spec/libcxx.md`).
pub const TAG: &str = "llvmorg-22.1.8";
pub const PACKAGES: &[&str] = &["libc++-22-dev", "libc++abi-22-dev", "libunwind-22-dev"];
pub const ARCHIVES: &[&str] = &["libc++.a", "libc++abi.a", "libc++experimental.a", "libunwind.a"];

const LONG: Limits = Limits { time: Duration::from_mins(20), memory_kib: 8 << 20 };

/// The wrapper repository: the parent of the build root.
pub fn wrapper_root() -> PathBuf {
    let root = super::build_root();
    root.parent().map_or(root.clone(), Path::to_path_buf)
}

/// kiln's checkout (read only): `MASON_KILN_SRC`, else `<wrapper>/kiln`.
pub fn kiln_checkout() -> PathBuf {
    std::env::var_os("MASON_KILN_SRC")
        .filter(|v| !v.is_empty())
        .map_or_else(|| wrapper_root().join("kiln"), PathBuf::from)
}

/// kiln's runner, built by step 5.
pub fn kiln_xtask() -> PathBuf {
    super::lane_dir().join("kiln-target/release/xtask")
}

/// libc++'s tests, fetched by step 4.
pub fn libcxx_tests() -> PathBuf {
    super::lane_dir().join(format!("llvm-project-{}", TAG.trim_start_matches("llvmorg-")))
}

fn sh(script: &str, dir: &Path, what: &str) -> Result<(), String> {
    println!("xtask mason setup: {what}");
    let o = run_in(Path::new("/bin/sh"), &["-c".into(), script.into()], dir, &[], LONG)?;
    if o.end.success() {
        Ok(())
    } else {
        Err(format!("{what}: {}\n{}", o.end, String::from_utf8_lossy(&o.stderr)))
    }
}

fn q(p: &Path) -> String {
    format!("'{}'", p.display().to_string().replace('\'', "'\\''"))
}

pub fn main(args: &[String]) -> Result<ExitCode, String> {
    if !args.is_empty() {
        return Err("setup takes no arguments".into());
    }
    let lane = super::lane_dir();
    let mk = |p: &Path| std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()));
    mk(&lane)?;
    let debs = lane.join("debs");
    if !PACKAGES.iter().all(|p| has_deb(&debs, p)) {
        mk(&debs)?;
        sh(
            &format!("apt-get download {}", PACKAGES.join(" ")),
            &debs,
            "downloading the libc++ packages",
        )?;
    }
    let tree = lane.join("libcxx-22-static");
    let lib = tree.join("usr/lib/aarch64-linux-gnu");
    if !ARCHIVES.iter().all(|a| lib.join(a).is_file()) {
        let tmp = lane.join(format!("extract-{}", std::process::id()));
        mk(&tmp)?;
        let mut script = String::from("set -e\n");
        for p in PACKAGES {
            let deb = find_deb(&debs, p).ok_or_else(|| format!("no {p} in {}", debs.display()))?;
            // Regular files only: members under the three trees and `*.a`.
            let _ = writeln!(
                script,
                "dpkg-deb --fsys-tarfile {} | tar -x --no-same-owner -f - --wildcards \
                 './usr/lib/llvm-22/include/*' './usr/lib/llvm-22/share/*' './usr/lib/aarch64-linux-gnu/*.a' \
                 2>/dev/null || true",
                q(&deb)
            );
        }
        sh(&script, &tmp, "extracting headers and static archives")?;
        let _ = std::fs::remove_dir_all(&tree);
        std::fs::rename(&tmp, &tree)
            .map_err(|e| format!("{} -> {}: {e}", tmp.display(), tree.display()))?;
        for a in ARCHIVES {
            if !lib.join(a).is_file() {
                return Err(format!("the packages gave no {a}"));
            }
        }
    }
    let stripped = lane.join("libcxx-22-stripped");
    if !ARCHIVES.iter().all(|a| stripped.join(a).is_file()) {
        mk(&stripped)?;
        let mut script = String::from("set -e\n");
        for a in ARCHIVES {
            let _ = writeln!(
                script,
                "objcopy -R .ARM.attributes -R .deplibs {} {}",
                q(&lib.join(a)),
                q(&stripped.join(a))
            );
        }
        sh(&script, &lane, "stripping .ARM.attributes and .deplibs")?;
    }
    let kiln = kiln_checkout();
    let tests = libcxx_tests();
    if !tests.join("libcxx/test").is_dir() {
        let fetch = kiln.join("scripts/fetch-libcxx-tests.sh");
        sh(&format!("{} {TAG} {}", q(&fetch), q(&tests)), &lane, "fetching libc++'s tests")?;
    }
    let src = lane.join("kiln-src");
    let commit = run_in(
        Path::new("git"),
        &["-C".into(), kiln.display().to_string(), "rev-parse".into(), "HEAD".into()],
        &lane,
        &[],
        LONG,
    )?;
    let commit = String::from_utf8_lossy(&commit.stdout).trim().to_owned();
    let stamp = src.join(".mason-commit");
    if std::fs::read_to_string(&stamp).ok().as_deref() != Some(commit.as_str()) {
        let _ = std::fs::remove_dir_all(&src);
        mk(&src)?;
        sh(
            &format!("git -C {} archive HEAD | tar -x -C {}", q(&kiln), q(&src)),
            &lane,
            "copying kiln's tree at the submodule's commit",
        )?;
        std::fs::write(&stamp, &commit).map_err(|e| format!("{}: {e}", stamp.display()))?;
    }
    let link = src.join(".toolchain/libcxx-22-root");
    if std::fs::read_link(&link).ok().as_deref() != Some(tree.as_path()) {
        mk(&src.join(".toolchain"))?;
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&tree, &link).map_err(|e| format!("{}: {e}", link.display()))?;
    }
    println!("xtask mason setup: building kiln's xtask");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut c = std::process::Command::new(cargo);
    c.args(["build", "--release", "--quiet", "-p", "xtask"])
        .current_dir(&src)
        .env("CARGO_TARGET_DIR", lane.join("kiln-target"));
    match crate::process::run_step(&mut c, true) {
        Ok((s, _)) if s.success() => {}
        Ok((s, _)) => return Err(format!("kiln's xtask: cargo build: {s}")),
        Err(e) => return Err(format!("kiln's xtask: cannot run cargo: {e}")),
    }
    println!("xtask mason setup: ready in {}", lane.display());
    Ok(ExitCode::SUCCESS)
}

fn find_deb(dir: &Path, package: &str) -> Option<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                n.starts_with(&format!("{package}_"))
                    && Path::new(n).extension().is_some_and(|e| e.eq_ignore_ascii_case("deb"))
            })
        })
        .collect();
    v.sort();
    v.pop()
}

fn has_deb(dir: &Path, package: &str) -> bool {
    find_deb(dir, package).is_some()
}

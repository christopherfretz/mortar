//! Helpers of the `mortar_scan` tests: the phases before the scan, run
//! for real (`mortar_cli::parse` → `mortar_input::load` →
//! `mortar_resolve::resolve_inputs` → `mortar_live::liveness`), the
//! AArch64 instruction proof, and the tools.
//!
//! [`link`] is the glue a link driver (WP-027) needs between the phases;
//! nothing here is test-only except the tools.
#![allow(dead_code, reason = "each test file uses a part")]

pub mod dynarch;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mortar_a64::A64;
use mortar_a64::mapping::{MapSym, Mapping, mapping_kind, proof_runs};
use mortar_arch::{OutputKind, ProofRun};
use mortar_core::Liveness;
use mortar_core::assignment::Assignment;
use mortar_core::inputs::Inputs;
use mortar_core::par::Pool;
use mortar_core::plan::{self, Plan};
use mortar_core::split::Split;
use mortar_diag::{Collector, Diagnostic, Key, Policy, Style};
use mortar_input::load::{LoadEnv, load};
use mortar_input::store::{Store, StorePolicy};
use mortar_live::{LiveInputs, LiveOptions, liveness};
use mortar_resolve::Outcome;
use mortar_scan::{CodeProof, Mark, Mismatch, ScanInputs, ScanOptions, Scanned, scan};
use mortar_testutil::target::Arch;

/// The architecture these tests are about (`spec/harness.md` "A test's
/// subject decides its tools"): its GNU tools by [`Arch::gnu`]'s names,
/// whatever the host.
pub const ARCH: Arch = Arch::Aarch64;

/// AArch64's instruction proof: its mapping symbols (a64.md §3.4).
#[derive(Debug, Clone, Copy, Default)]
pub struct A64Proof;

impl CodeProof for A64Proof {
    fn mark(&self, name: &[u8], st_info: u8) -> Option<bool> {
        mapping_kind(name, st_info).map(|m| m == Mapping::Code)
    }

    fn runs(&self, marks: &mut [Mark], len: u64, exec: bool, out: &mut Vec<ProofRun>) {
        let mut syms: Vec<MapSym> = marks
            .iter()
            .map(|m| MapSym {
                index: m.index,
                value: m.value,
                kind: if m.code { Mapping::Code } else { Mapping::Data },
            })
            .collect();
        proof_runs(&mut syms, len, exec, 0, 0, out);
    }
}

/// The scan's output kind for a plan's (M1: static executables).
pub fn output_kind(kind: plan::OutputKind) -> OutputKind {
    match kind {
        plan::OutputKind::StaticExecutable => OutputKind::Static,
        plan::OutputKind::Executable => OutputKind::Exec,
        plan::OutputKind::PieExecutable | plan::OutputKind::StaticPie => OutputKind::Pie,
        plan::OutputKind::Shared => OutputKind::Shared,
        plan::OutputKind::Relocatable => OutputKind::Relocatable,
    }
}

/// Resolution's `ComdatMismatch` rows as the scan takes them.
pub fn mismatches(out: &Outcome) -> Vec<Mismatch> {
    out.comdat_mismatch
        .iter()
        .map(|m| Mismatch { key: m.key, group: m.group, keeper: m.keeper, dropped: m.dropped })
        .collect()
}

/// A link up to the scan.
pub struct Linked {
    pub plan: Plan,
    pub inputs: Inputs<'static>,
    pub outcome: Outcome,
    pub liveness: Liveness,
    pub mismatch: Vec<Mismatch>,
    /// The architecture [`Linked::rescan`] uses (`A64::M1` by default).
    pub arch: A64,
    pub scanned: Scanned,
    /// The diagnostics of the phases before the scan.
    pub before: Vec<(Key, Diagnostic)>,
}

impl Linked {
    /// The scan's diagnostics, rendered in key order as the collector
    /// prints them (default policy).
    pub fn rendered(&self) -> String {
        render(&self.scanned.diagnostics, Policy::default())
    }

    /// The ids of the scan's diagnostics, in key order.
    pub fn ids(&self) -> Vec<&'static str> {
        let mut v: Vec<(Key, &'static str)> =
            self.scanned.diagnostics.iter().map(|(k, d)| (*k, d.id())).collect();
        v.sort();
        v.into_iter().map(|x| x.1).collect()
    }

    /// The scan again, on `pool`.
    pub fn rescan(&self, pool: &Pool) -> Scanned {
        let t = ScanInputs {
            inputs: &self.inputs,
            resolution: &self.outcome.resolution,
            liveness: &self.liveness,
            assignment: None,
            comdat_mismatch: &self.mismatch,
            gnu_warnings: &self.outcome.gnu_warning_syms,
            closure_only: &self.outcome.closure_only,
        };
        let opts = ScanOptions {
            output: output_kind(self.plan.kind),
            dynamic: false,
            sections: None,
            proof: &A64Proof,
        };
        scan(&self.arch, &t, &opts, pool)
    }

    /// A schedule-independent picture of a scan: the demands, the record
    /// index and each diagnostic rendered, in key order.
    pub fn picture(s: &Scanned) -> (String, String, Vec<(Key, String)>) {
        let mut d: Vec<(Key, String)> = s
            .diagnostics
            .iter()
            .map(|(k, d)| {
                (*k, render(&[(*k, d.clone())], Policy { error_limit: 0, ..Policy::default() }))
            })
            .collect();
        d.sort();
        (format!("{:?}", s.demands), format!("{:?}", s.order), d)
    }
}

/// `diags` through a collector with `policy`, rendered (`ld:`).
pub fn render(diags: &[(Key, Diagnostic)], policy: Policy) -> String {
    let c = Collector::new(policy);
    for (k, d) in diags {
        c.emit(*k, d.clone());
    }
    c.finish().render(&Style::plain("ld"))
}

/// The phases before and including the scan, on a command line (no
/// program name) run in `cwd`, under both stores (`spec/input.md`
/// §11.1): the scan over the staged store's inputs must be the scan over
/// the resident store's (the same demands, record index, diagnostics and
/// statistics, and the same diagnostics before the scan); the staged
/// link, the default, is returned. `None` when loading fails (its
/// diagnostics are printed).
pub fn link(cwd: &Path, args: &[OsString], pool: &Pool) -> Option<Linked> {
    let resident = link_in(cwd, args, pool, StorePolicy::Resident);
    let staged = link_in(cwd, args, pool, StorePolicy::Staged);
    match (&resident, &staged) {
        (Some(r), Some(s)) => {
            assert_eq!(
                Linked::picture(&r.scanned),
                Linked::picture(&s.scanned),
                "the scan differs between the resident and the staged store"
            );
            assert_eq!(r.scanned.stats, s.scanned.stats, "scan statistics differ by store");
            let text = |l: &Linked| render(&l.before, Policy::default());
            assert_eq!(text(r), text(s), "the phases before the scan differ by store");
        }
        (None, None) => {}
        _ => panic!("loading succeeded under one store only"),
    }
    staged
}

/// [`link`] under one store.
pub fn link_in(cwd: &Path, args: &[OsString], pool: &Pool, policy: StorePolicy) -> Option<Linked> {
    let parsed = mortar_cli::parse(args);
    let plan = parsed.plan;
    let mut before = parsed.diagnostics;
    let store: &'static Store = Box::leak(Box::new(Store::new(policy)));
    let diag = Collector::new(Policy::default());
    let env = LoadEnv { cwd: cwd.to_path_buf(), pool: *pool };
    let Some(inputs) = load(&plan, &env, store, &diag) else {
        eprintln!("{}", diag.finish().render(&Style::plain("ld")));
        return None;
    };
    let outcome = mortar_resolve::resolve_inputs(&inputs, &plan, pool);
    before.extend(outcome.diagnostics.iter().cloned());
    let split = Split::default();
    let sections = outcome.resolution.sections.len();
    let assignment = Assignment::empty(sections);
    let live = LiveInputs {
        inputs: &inputs,
        resolution: &outcome.resolution,
        assignment: &assignment,
        split: &split,
        roots: &outcome.roots,
        arch: None,
    };
    let options = LiveOptions {
        gc_sections: plan.shaping.gc_sections,
        keep_exported: plan.shaping.gc_keep_exported,
    };
    let liveness = liveness(&live, options, pool).ok()?;
    let mismatch = mismatches(&outcome);
    let mut l = Linked {
        plan,
        inputs,
        outcome,
        liveness,
        mismatch,
        arch: A64::M1,
        scanned: Scanned::default(),
        before,
    };
    l.scanned = l.rescan(pool);
    Some(l)
}

/// [`link`] on string arguments.
pub fn link_args(cwd: &Path, args: &[&str], pool: &Pool) -> Option<Linked> {
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    link(cwd, &args, pool)
}

/// A tool, or `None` with a note (a failure under `MORTAR_REQUIRE_TOOLS`).
pub fn need(name: &str) -> Option<PathBuf> {
    let t = mortar_testutil::tool(name);
    if t.is_none() {
        assert!(!mortar_testutil::tools_required(), "{name} is required (MORTAR_REQUIRE_TOOLS)");
        eprintln!("note: {name} not found; skipped");
    }
    t
}

/// [`need`] of [`ARCH`]'s GNU tool `tool` (`gcc`, `as`, `ld.bfd`, ...).
pub fn need_gnu(tool: &str) -> Option<PathBuf> {
    need(&ARCH.gnu(tool))
}

/// The driver options that link with `lld` (the `ld.lld` [`need`]
/// found): `-B <dir> -fuse-ld=lld`, `dir` a directory under `scratch`
/// holding `ld.lld`. The driver alone looks for `ld.lld` on `PATH`, which
/// need not be the lld found, nor be there at all.
pub fn fuse_lld(scratch: &Path, lld: &Path) -> Vec<OsString> {
    let bin = scratch.join("lldbin");
    std::fs::create_dir_all(&bin).unwrap();
    let link = bin.join("ld.lld");
    if link.symlink_metadata().is_err() {
        std::os::unix::fs::symlink(lld, &link).unwrap();
    }
    vec!["-B".into(), bin.into_os_string(), "-fuse-ld=lld".into()]
}

/// Runs `program` in `cwd`.
pub fn run_in(program: &Path, args: &[OsString], cwd: &Path) -> mortar_testutil::Outcome {
    let mut c = Command::new(program);
    c.args(args).current_dir(cwd);
    mortar_testutil::run_with(&mut c, b"", Duration::from_mins(3))
}

/// Assembles `src` into `dir/name` ([`ARCH`]'s `as`), or `None` without
/// the tool.
pub fn assemble(dir: &Path, name: &str, src: &str) -> Option<PathBuf> {
    let gas = need_gnu("as")?;
    let s = dir.join(format!("{name}.s"));
    std::fs::write(&s, src).unwrap();
    let o = dir.join(name);
    let r = run_in(&gas, &["-o".into(), o.clone().into(), s.into()], dir);
    assert!(r.success(), "as {name}: {}", r.stderr_text());
    Some(o)
}

/// Compiles `src` with [`ARCH`]'s `driver` (`gcc`, `g++`, `gfortran`)
/// into `dir/name` with `-c` and `flags`.
pub fn compile(
    dir: &Path,
    driver: &str,
    name: &str,
    ext: &str,
    src: &str,
    flags: &[&str],
) -> Option<PathBuf> {
    let cc = need_gnu(driver)?;
    let s = dir.join(format!("{name}.{ext}"));
    std::fs::write(&s, src).unwrap();
    let o = dir.join(name);
    let mut a: Vec<OsString> = vec!["-c".into()];
    a.extend(flags.iter().map(OsString::from));
    a.extend(["-o".into(), o.clone().into(), s.into()]);
    let r = run_in(&cc, &a, dir);
    assert!(r.success(), "{driver} {name}: {}", r.stderr_text());
    Some(o)
}

/// The linker command line `driver` runs (`-###`): the arguments after
/// the linker's path, with the LTO plugin's options removed (mortar has
/// no plugin interface).
pub fn driver_link_args(driver: &Path, args: &[&str], cwd: &Path) -> Option<Vec<OsString>> {
    let mut a: Vec<OsString> = vec!["-###".into()];
    a.extend(args.iter().map(OsString::from));
    let out = run_in(driver, &a, cwd);
    let text = out.stderr_text();
    let line = text.lines().rev().find(|l| {
        let first = shell_words(l).into_iter().next().unwrap_or_default();
        let name = first.rsplit('/').next().unwrap_or_default().to_owned();
        first.starts_with('/') && (name == "collect2" || name == "ld" || name.ends_with("-ld"))
    })?;
    let mut words = shell_words(line);
    words.remove(0);
    let mut out = Vec::new();
    let mut it = words.into_iter();
    while let Some(w) = it.next() {
        if w == "-plugin" {
            it.next();
        } else if !w.starts_with("-plugin-opt=") {
            out.push(OsString::from(w));
        }
    }
    Some(out)
}

fn shell_words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut w = String::new();
    let mut in_word = false;
    let mut quoted = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                in_word = true;
            }
            '\\' => {
                if let Some(n) = chars.next() {
                    w.push(n);
                }
                in_word = true;
            }
            c if c.is_whitespace() && !quoted => {
                if in_word {
                    out.push(std::mem::take(&mut w));
                    in_word = false;
                }
            }
            c => {
                w.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        out.push(w);
    }
    out
}

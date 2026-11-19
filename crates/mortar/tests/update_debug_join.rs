//! WP-230 G-3: the update with `-g` (`spec/incr.md` IN-50..IN-57, IN-70's
//! debug condition), through `update::run_with` (the `mortar` binary),
//! static C++ by host-aware GNU drivers, AArch64 and x86-64, linked with
//! `--build-id`.
//!
//! - The identity case: a same-size body edit whose debug sections keep
//!   their sizes and strings. The update is taken; G-1's decisions are
//!   judged against the front (`G-3 debug: judged` in the findings file,
//!   no FINDING); the output is byte-equal to the full link of the new
//!   inputs (`--incremental=full`), build id included; gdb gives the same
//!   answers on both (a breakpoint on the edited function, `bt`, `info
//!   line`, a local printed: run natively on its host, under `qemu-<arch>
//!   -g` elsewhere); lldb too on its host. A second update from that
//!   state is taken as well.
//! - The same under the operator's configuration (`-ffunction-sections
//!   -fdata-sections --gc-sections`).
//! - The refusals that stay, by name: a name index (`-gpubnames`:
//!   `debug-name-index`), DWARF 4 type units (`-fdebug-types-section`:
//!   `debug-unit-kind`), an edit that is not the identity case (a renamed
//!   local: an appended string, `(layout)`).
//!
//! `UPDATE_DEBUG_JOIN_KEEP=<dir>` copies each update's output and map
//! there, with the scratch directory left in place, for `cargo xtask
//! plumb`.

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, run_with, tool, tools_required};
use support::{Source, build_by, with_output};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

const MAIN: &str = "#include <cstdio>\nint c_value(int);\nint u_helper(int);\n\
int main(int argc, char **){ std::printf(\"c=%d u=%d\\n\", c_value(argc + 1), u_helper(argc)); return 0; }\n";
const U: &str = "int u_helper(int x) {\n  int tripled = x * 3;\n  return tripled + 1;\n}\n\
int u_dead(int x) { return x - 1; }\n";
/// The changed file: the immediate is a macro, so an edit leaves every line,
/// string and size of its debug sections alone.
fn c_cc(k: u32, local: &str) -> String {
    format!(
        "#define C_K {k}\nint c_value(int x) {{\n  int {local} = x * C_K;\n  return {local} + 1;\n}}\n"
    )
}

/// The line of `c_value`'s `return` in [`c_cc`].
const RETURN_LINE: u32 = 4;

struct Prog {
    arch: Arch,
    cxx: String,
    flags: Vec<&'static str>,
    scratch: Option<Scratch>,
    dir: PathBuf,
    args: Vec<String>,
    findings: PathBuf,
}

impl Prog {
    fn new(arch: Arch, case: &str, flags: &[&'static str], gc: bool) -> Option<Self> {
        let cxx = arch.gnu("g++");
        let scratch = Scratch::new(&format!("g3join-{case}-{}", arch.name()));
        // `Source` takes static text: the first version lives for the test.
        let c: &'static str = Box::leak(c_cc(7, "scaled").into_boxed_str());
        let sources = [
            Source { name: "main.cc", text: MAIN },
            Source { name: "u.cc", text: U },
            Source { name: "c.cc", text: c },
        ];
        let mut all = vec!["-O0", "-g"];
        all.extend_from_slice(flags);
        let Some(b) = build_by(&scratch, &cxx, &all, &sources) else {
            assert!(!tools_required(), "{cxx} is required");
            return None;
        };
        let mut args = b.args;
        args.push("--build-id".into());
        if gc {
            args.push("--gc-sections".into());
        }
        let findings = scratch.join("findings.txt");
        Some(Self { arch, cxx, flags: all, scratch: Some(scratch), dir: b.dir, args, findings })
    }

    fn link(&self, out: &str, extra: &[&str]) -> (bool, String) {
        let mut c = Command::new(MORTAR);
        c.args(with_output(&self.args, &self.dir.join(out)))
            .arg("--mortar-map")
            .arg(self.dir.join(format!("{out}.map")))
            .args(extra)
            .current_dir(&self.dir)
            .env("MORTAR_UPDATE_FINDINGS", &self.findings);
        let o = run_with(&mut c, b"", Duration::from_mins(5));
        assert!(!o.timed_out(), "mortar: TIMEOUT");
        (o.success(), o.stderr_text())
    }

    /// `c.cc.o` rebuilt from `text` with `extra` flags, as a new inode,
    /// compiled from the same path in the same directory as the first
    /// build (one `DW_AT_comp_dir`, as a rebuild has).
    fn rebuild_c(&self, text: &str, extra: &[&str]) {
        let src = self.dir.join("c.cc");
        std::fs::write(&src, text).expect("c.cc");
        let tmp = self.dir.join("c.cc.o.new");
        let mut c = Command::new(&self.cxx);
        c.args(&self.flags).args(extra).arg("-c").arg(&src).arg("-o").arg(&tmp);
        let o = run_with(&mut c, b"", Duration::from_mins(2));
        assert!(o.success(), "{}: {}", self.cxx, o.stderr_text());
        std::fs::rename(&tmp, self.dir.join("c.cc.o")).expect("rename c.cc.o");
    }

    fn bytes(&self, name: &str) -> Vec<u8> {
        std::fs::read(self.dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    fn equals_full_link(&self, out: &str, label: &str) {
        let (ok, err) = self.link("full", &["--incremental=full"]);
        assert!(ok, "{label}: the full link: {err}");
        let (got, want) = (self.bytes(out), self.bytes("full"));
        if let Some(i) = got.iter().zip(&want).position(|(a, b)| a != b) {
            panic!("{label}: the output differs from the full link at {i:#x}");
        }
        assert_eq!(got.len(), want.len(), "{label}: sizes");
    }

    fn run(&self, out: &str) -> String {
        let mut c = self.arch.command(&self.dir.join(out), None);
        let o = run_with(&mut c, b"", Duration::from_mins(2));
        assert!(!o.timed_out(), "{out}: TIMEOUT");
        assert!(o.success(), "{out}: {}", o.stderr_text());
        o.stdout_text()
    }

    fn findings(&self) -> String {
        std::fs::read_to_string(&self.findings).unwrap_or_default()
    }

    fn keep(&mut self, out: &str, label: &str) {
        let Some(dir) = std::env::var_os("UPDATE_DEBUG_JOIN_KEEP") else { return };
        let dir = Path::new(&dir);
        std::fs::create_dir_all(dir).expect("UPDATE_DEBUG_JOIN_KEEP");
        let name = label.replace(' ', "-");
        for (from, to) in
            [(out.to_owned(), name.clone()), (format!("{out}.map"), format!("{name}.map"))]
        {
            std::fs::copy(self.dir.join(from), dir.join(to)).expect("a kept copy");
        }
        std::fs::write(dir.join(format!("{name}.dir")), self.dir.as_os_str().as_encoded_bytes())
            .expect("the kept directory's name");
        std::mem::forget(self.scratch.take());
    }

    /// gdb's answers on `out`: a breakpoint on the edited function and on
    /// its `return` line, the run to it, `bt`, `info line`, the local; under
    /// `timeout 120` (a timeout is TIMEOUT, never an answer). Off its host
    /// the program runs under `qemu-<arch> -g <port>` and gdb attaches.
    fn gdb(&self, out: &str, local: &str) -> String {
        let prog = self.dir.join(out);
        let mut ex: Vec<String> =
            ["set pagination off", "set width 0", "set confirm off"].map(String::from).to_vec();
        ex.push("break c_value".into());
        ex.push(format!("info line c.cc:{RETURN_LINE}"));
        let mut qemu = None;
        if self.arch.is_host() {
            ex.push("run > /dev/null 2>&1".into());
        } else {
            let port = 20000
                + (std::process::id() % 20000) as u16
                + u16::from(self.arch.name().len() as u8);
            let mut q = Command::new(tool(self.arch.qemu()).expect("qemu"));
            q.arg("-L")
                .arg(self.arch.sysroot())
                .arg("-g")
                .arg(port.to_string())
                .arg(&prog)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            qemu = Some(q.spawn().expect("qemu -g"));
            std::thread::sleep(Duration::from_millis(300));
            ex.push(format!("target remote 127.0.0.1:{port}"));
            ex.push("continue".into());
        }
        ex.extend(["bt".into(), "info line *$pc".into(), format!("print {local}"), "next".into()]);
        ex.push(format!("print {local}"));
        ex.push("kill".into());
        let mut c = Command::new("timeout");
        c.arg("120").arg(tool(self.arch.gdb()).expect("gdb")).arg("-nx").arg("-batch");
        for e in &ex {
            c.arg("-ex").arg(e);
        }
        c.arg(&prog).current_dir(&self.dir);
        let o = run_with(&mut c, b"", Duration::from_secs(130));
        if let Some(mut q) = qemu {
            let _ = q.kill();
            let _ = q.wait();
        }
        if o.timed_out() || o.status.and_then(|s| s.code()) == Some(124) {
            return "TIMEOUT".into();
        }
        normalize(&o.stdout_text())
    }

    /// lldb on its host: the same questions.
    fn lldb(&self, out: &str, local: &str) -> Option<String> {
        if !self.arch.is_host() {
            return None;
        }
        let lldb = tool("lldb")?;
        let mut c = Command::new("timeout");
        c.arg("120").arg(lldb).arg("-b");
        for e in [
            // The container refuses personality(ADDR_NO_RANDOMIZE); the
            // programs are static, not PIE: no address moves.
            "settings set target.disable-aslr false".to_owned(),
            "breakpoint set -n c_value".to_owned(),
            "process launch -o /dev/null -e /dev/null".into(),
            "bt".into(),
            "next".into(),
            format!("frame variable {local}"),
            "source info".into(),
            "kill".into(),
        ] {
            c.arg("-o").arg(e);
        }
        c.arg(self.dir.join(out)).current_dir(&self.dir);
        let o = run_with(&mut c, b"", Duration::from_secs(130));
        if o.timed_out() || o.status.and_then(|s| s.code()) == Some(124) {
            return Some("TIMEOUT".into());
        }
        Some(normalize(&o.stdout_text()))
    }
}

/// A transcript without what differs between two runs of two files of
/// different names (the program's name, process ids, addresses of the
/// stack).
fn normalize(text: &str) -> String {
    text.lines()
        .filter(|l| {
            !l.contains("process ")
                && !l.contains("Process ")
                && !l.starts_with("[Inferior")
                && !l.contains("Remote debugging")
                && !l.contains("Current executable set to")
                && !l.starts_with("(lldb) target create")
        })
        .map(|l| {
            l.split_whitespace()
                .map(|w| if w.contains("0x0000ff") || w.contains("0x7ff") { "<stack>" } else { w })
                // lldb names the module: `o` or `full`, the two outputs.
                .map(|w| w.split_once('`').map_or(w, |(_, sym)| sym))
                .collect::<Vec<_>>()
                .join(" ")
                .replace("name = 'o'", "name = <out>")
                .replace("name = 'full'", "name = <out>")
                .replace("module o", "module <out>")
                .replace("module full", "module <out>")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn targets() -> [Arch; 2] {
    [Arch::Aarch64, Arch::X86_64]
}

fn no_finding(p: &Prog, label: &str) -> String {
    let text = p.findings();
    let bad: Vec<&str> = text.lines().filter(|l| l.contains("finding")).collect();
    assert!(bad.is_empty(), "{label}: FINDINGS:\n{}", bad.join("\n"));
    text
}

/// The identity case on `p`: two updates, each judged (`G-3 debug:
/// judged`), byte-equal to the full link, run, and gdb (and lldb) giving
/// the full link's answers.
fn identity_case(p: &mut Prog, label: &str) {
    let (ok, err) = p.link("o", &["--incremental"]);
    assert!(ok, "{label}: the first link: {err}");
    assert_eq!(p.run("o"), "c=15 u=4\n", "{label}: v0");
    for (k, want) in [(9, "c=19 u=4\n"), (7, "c=15 u=4\n")] {
        let _ = std::fs::remove_file(&p.findings);
        p.rebuild_c(&c_cc(k, "scaled"), &[]);
        let (ok, err) = p.link("o", &["--incremental=update", "--incremental-report"]);
        assert!(ok, "{label} k={k}: not an update: {err}\nfindings:\n{}", p.findings());
        assert!(err.contains("incremental: update ("), "{label}: {err}");
        let text = no_finding(p, label);
        assert!(
            text.lines().any(|l| l.starts_with("G-3 debug: judged")),
            "{label} k={k}: G-1 not judged:\n{text}"
        );
        let o = p.bytes("o");
        p.equals_full_link("o", label);
        assert_eq!(p.run("o"), want, "{label} k={k}");
        let (gu, gf) = (p.gdb("o", "scaled"), p.gdb("full", "scaled"));
        assert_ne!(gu, "TIMEOUT", "{label} k={k}: gdb on the update");
        assert!(gu.contains("c_value"), "{label} k={k}: gdb:\n{gu}");
        assert!(
            gu.contains(&format!("c.cc:{RETURN_LINE}")) || gu.contains("Line 4"),
            "{label}: {gu}"
        );
        assert_eq!(gu, gf, "{label} k={k}: gdb answers differ (update, full link)");
        if let (Some(lu), Some(lf)) = (p.lldb("o", "scaled"), p.lldb("full", "scaled")) {
            assert_ne!(lu, "TIMEOUT", "{label} k={k}: lldb on the update");
            assert_eq!(lu, lf, "{label} k={k}: lldb answers differ");
        }
        assert_eq!(o, p.bytes("o"), "{label}: the update's output changed under the debuggers");
        eprintln!("{label} k={k}: gdb transcript ({} lines):\n{gu}", gu.lines().count());
    }
    p.keep("o", label);
}

// spec: incr.md IN-70 (G-3: the debug condition is IN-55/IN-55a only),
// IN-50 step 1, IN-54, IN-56 (WP-230): a same-size body edit with -g is an
// update; G-1 judged against the front; byte-equal to the full link
// (build id re-sealed); the debuggers answer as on the full link.
#[test]
fn identity_edit_with_debug_is_taken() {
    let mut ran = 0;
    for arch in targets() {
        let Some(mut p) = Prog::new(arch, "id", &[], false) else { continue };
        ran += 1;
        identity_case(&mut p, &format!("id {}", arch.name()));
    }
    assert!(ran == 2 || !tools_required(), "{ran} of the 2 targets");
}

// spec: incr.md IN-70, IN-71a (WP-230; the operator's configuration):
// static, -g, -ffunction-sections -fdata-sections, --gc-sections. The
// delta's inputs refuse `--gc-sections` (`layout_in::layout_inputs`,
// `(live)`, recorded), so G-1 never runs and the identity edit falls back
// `(debug)` by name ("G-1 did not run"), the update writing nothing.
// When the delta's inputs take `--gc-sections` this case is
// `identity_case` (as `identity_edit_with_debug_is_taken`).
#[test]
fn gc_identity_edit_with_debug_falls_back_until_g1_runs() {
    let mut ran = 0;
    for arch in targets() {
        let fs = ["-ffunction-sections", "-fdata-sections"];
        let Some(p) = Prog::new(arch, "gc", &fs, true) else { continue };
        ran += 1;
        let label = format!("gc {}", arch.name());
        let (ok, err) = p.link("o", &["--incremental"]);
        assert!(ok, "{label}: the first link: {err}");
        let before = p.bytes("o");
        let _ = std::fs::remove_file(&p.findings);
        p.rebuild_c(&c_cc(9, "scaled"), &[]);
        let (ok, err) = p.link("o", &["--incremental=update", "--incremental-report"]);
        assert!(!ok, "{label}: updated: {err}");
        assert!(err.contains("cannot update: update-not-implemented (debug)"), "{label}: {err}");
        let text = no_finding(&p, &label);
        assert!(text.contains("G-1 did not run"), "{label}:\n{text}");
        assert_eq!(before, p.bytes("o"), "{label}: a fall-back wrote the output");
    }
    assert!(ran == 2 || !tools_required(), "{ran} of the 2 targets");
}

/// The update after rebuilding `c.cc` with `extra` and `local` is not
/// taken: its reason line.
fn refused(
    arch: Arch,
    case: &str,
    base: &[&'static str],
    extra: &[&str],
    local: &str,
) -> Option<String> {
    let p = Prog::new(arch, case, base, false)?;
    let (ok, err) = p.link("o", &["--incremental"]);
    assert!(ok, "{case}: the first link: {err}");
    p.rebuild_c(&c_cc(9, local), extra);
    let (ok, err) = p.link("o", &["--incremental=update", "--incremental-report"]);
    assert!(!ok, "{case} {}: updated: {err}", arch.name());
    no_finding(&p, case);
    let line = err.lines().find(|l| l.contains("cannot update: ")).unwrap_or_default().to_owned();
    Some(line)
}

// spec: incr.md IN-55 (WP-230: at U2, by name): a changed file with
// `.debug_pubnames` (g++ -gpubnames) falls back `debug-name-index`.
#[test]
fn name_index_is_debug_name_index() {
    for arch in targets() {
        let Some(l) = refused(arch, "pub", &["-gpubnames"], &[], "scaled") else { continue };
        assert!(l.contains("cannot update: debug-name-index"), "{}: {l}", arch.name());
    }
}

// spec: incr.md IN-55a (WP-230): DWARF 4 type units (`.debug_types`) in a
// changed file fall back `debug-unit-kind`.
#[test]
fn type_units_are_debug_unit_kind() {
    const TYPES: [&str; 2] = ["-gdwarf-4", "-fdebug-types-section"];
    for arch in targets() {
        let Some(p) = Prog::new(arch, "types", &TYPES, false) else { continue };
        let (ok, err) = p.link("o", &["--incremental"]);
        assert!(ok, "the first link: {err}");
        // A type the unit describes, so the section is not empty.
        let text = "struct pt { int x, y; };\nint c_value(int x) {\n  pt scaled = {x, 9};\n  return scaled.x * scaled.y + 1;\n}\n";
        p.rebuild_c(text, &[]);
        let (ok, err) = p.link("o", &["--incremental=update", "--incremental-report"]);
        assert!(!ok, "{}: updated: {err}", arch.name());
        assert!(err.contains("cannot update: debug-unit-kind"), "{}: {err}", arch.name());
        no_finding(&p, "types");
    }
}

// spec: incr.md IN-70, IN-56 (WP-230): a renamed local appends a string:
// not the identity case, `update-not-implemented (layout)` by name (G-1's
// tail and dead ranges are written with the delta's placement, M2b-3's
// wiring), and no FINDING.
#[test]
fn renamed_local_is_not_the_identity_case() {
    for arch in targets() {
        let Some(l) = refused(arch, "rename", &[], &[], "renamed_local_wp230") else { continue };
        assert!(
            l.contains("cannot update: update-not-implemented (layout)"),
            "{}: {l}",
            arch.name()
        );
    }
}

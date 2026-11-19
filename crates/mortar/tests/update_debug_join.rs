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
        // `Source` takes static text: the first version lives for the test.
        let c: &'static str = Box::leak(c_cc(7, "scaled").into_boxed_str());
        let sources = [
            Source { name: "main.cc", text: MAIN },
            Source { name: "u.cc", text: U },
            Source { name: "c.cc", text: c },
        ];
        Self::with(arch, case, &["-g"], flags, gc, &sources)
    }

    /// The program of `sources` in their order (link order), compiled
    /// `-O0 <debug> <flags>`; `c.cc` is the file the cases rebuild.
    fn with(
        arch: Arch,
        case: &str,
        debug: &[&'static str],
        flags: &[&'static str],
        gc: bool,
        sources: &[Source],
    ) -> Option<Self> {
        let cxx = arch.gnu("g++");
        let scratch = Scratch::new(&format!("g3join-{case}-{}", arch.name()));
        let mut all = vec!["-O0"];
        all.extend_from_slice(debug);
        all.extend_from_slice(flags);
        // Headers are written beside the units, not compiled.
        let (headers, units): (Vec<&Source>, Vec<&Source>) =
            sources.iter().partition(|s| Path::new(s.name).extension().is_some_and(|e| e == "h"));
        for h in headers {
            scratch.write(h.name, h.text);
        }
        let units: Vec<Source> = units.into_iter().copied().collect();
        let Some(b) = build_by(&scratch, &cxx, &all, &units) else {
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
        self.link_env(out, extra, &[])
    }

    /// [`Prog::link`] with `env` set (the tests' hooks).
    fn link_env(&self, out: &str, extra: &[&str], env: &[(&str, &str)]) -> (bool, String) {
        let mut c = Command::new(MORTAR);
        c.envs(env.iter().copied());
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

    /// [`Prog::rebuild_c`] with the build's flags but `debug` in place of
    /// its debug flags (`-g`, `-g3`, ...).
    fn rebuild_c_as(&self, text: &str, debug: &[&str]) {
        let src = self.dir.join("c.cc");
        std::fs::write(&src, text).expect("c.cc");
        let tmp = self.dir.join("c.cc.o.new");
        let mut c = Command::new(&self.cxx);
        c.args(self.flags.iter().filter(|f| !f.starts_with("-g")))
            .args(debug)
            .arg("-c")
            .arg(&src)
            .arg("-o")
            .arg(&tmp);
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
            // A port the kernel hands out free (the tests of this binary
            // run in parallel: a port derived from the pid alone was shared,
            // and gdb attached to another test's qemu).
            let port = std::net::TcpListener::bind("127.0.0.1:0")
                .and_then(|l| l.local_addr())
                .expect("a free port")
                .port();
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
fn identity_case(p: &mut Prog, label: &str) -> Vec<String> {
    identity_case_of(p, label, &|k| c_cc(k, "scaled"))
}

/// [`identity_case`] with `c.cc`'s versions `text(k)` (`c_value`'s
/// immediate `k`, its `return` on line [`RETURN_LINE`]); returns each
/// update's `G-3 debug: judged` line.
fn identity_case_of(p: &mut Prog, label: &str, text: &dyn Fn(u32) -> String) -> Vec<String> {
    let mut judged = Vec::new();
    let (ok, err) = p.link("o", &["--incremental"]);
    assert!(ok, "{label}: the first link: {err}");
    assert_eq!(p.run("o"), "c=15 u=4\n", "{label}: v0");
    for (k, want) in [(9, "c=19 u=4\n"), (7, "c=15 u=4\n")] {
        let _ = std::fs::remove_file(&p.findings);
        p.rebuild_c(&text(k), &[]);
        let (ok, err) = p.link("o", &["--incremental=update", "--incremental-report"]);
        assert!(ok, "{label} k={k}: not an update: {err}\nfindings:\n{}", p.findings());
        assert!(err.contains("incremental: update ("), "{label}: {err}");
        let text = no_finding(p, label);
        let line = text.lines().find(|l| l.starts_with("G-3 debug: judged"));
        let Some(line) = line else { panic!("{label} k={k}: G-1 not judged:\n{text}") };
        judged.push(line.to_owned());
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
    judged
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

// spec: incr.md IN-70, IN-71a, IN-50 step 1, IN-54, IN-56 (WP-230 G-3b;
// the operator's configuration): static, -g, -ffunction-sections
// -fdata-sections, --gc-sections. Since M2b-2d the delta's inputs take
// `--gc-sections` (`layout_in::layout_inputs_live`), so G-1 runs on the
// edit that flips no liveness and is judged as without gc: the identity
// case, taken, byte-equal to the full link, the debuggers' answers equal.
#[test]
fn gc_identity_edit_with_debug_is_taken() {
    let mut ran = 0;
    for arch in targets() {
        let fs = ["-ffunction-sections", "-fdata-sections"];
        let Some(mut p) = Prog::new(arch, "gc", &fs, true) else { continue };
        ran += 1;
        identity_case(&mut p, &format!("gc {}", arch.name()));
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

// ---- G-3b (WP-230): the judged items' FINDINGs, R-5 a, G-1's refusals ----

/// `c.cc` without a macro of its own (`-g3` records each `#define` in
/// `.debug_str`): the immediate is a literal, the `return` on line
/// [`RETURN_LINE`] as in [`c_cc`].
fn c_lit(k: u32) -> String {
    format!(
        "// no macro\nint c_value(int x) {{\n  int scaled = x * {k};\n  return scaled + 1;\n}}\n"
    )
}

/// The update after `rebuild` is not taken, the output untouched: its
/// reason line and the findings file.
fn not_taken(
    p: &Prog,
    label: &str,
    env: &[(&str, &str)],
    rebuild: &dyn Fn(&Prog),
) -> (String, String) {
    let before = p.bytes("o");
    let _ = std::fs::remove_file(&p.findings);
    rebuild(p);
    let (ok, err) = p.link_env("o", &["--incremental=update", "--incremental-report"], env);
    assert!(!ok, "{label}: updated: {err}");
    assert_eq!(before, p.bytes("o"), "{label}: a fall-back wrote the output");
    let line = err.lines().find(|l| l.contains("cannot update: ")).unwrap_or_default().to_owned();
    (line, p.findings())
}

// spec: incr.md IN-50 step 1, IN-56, IN-54 (WP-230 G-3b): in the identity
// case G-1's plan is judged against the front's bytes; the test hook
// `MORTAR_G3_INJECT` makes one judged answer differ by one byte (IN-50: a
// one-byte hole after a contribution; IN-56: one string's output offset
// plus one; IN-54: one byte of the `.debug_aranges` rewrite), and each
// gives `debug_judged`'s FINDING and falls back
// `update-not-implemented (debug)`, the output untouched; the next update,
// without the hook, is taken (the state was not touched either).
#[cfg(feature = "testhooks")]
#[test]
fn injected_disagreement_is_a_finding() {
    let mut ran = 0;
    for arch in targets() {
        let Some(p) = Prog::new(arch, "inject", &[], false) else { continue };
        ran += 1;
        let (ok, err) = p.link("o", &["--incremental"]);
        assert!(ok, "the first link: {err}");
        for (n, item) in ["in50", "in56", "in54"].into_iter().enumerate() {
            let label = format!("inject {item} {}", arch.name());
            let k = if n % 2 == 0 { 9 } else { 7 };
            let (line, text) = not_taken(&p, &label, &[("MORTAR_G3_INJECT", item)], &|p| {
                p.rebuild_c(&c_cc(k, "scaled"), &[]);
            });
            assert!(
                line.contains("cannot update: update-not-implemented (debug)"),
                "{label}: {line}"
            );
            assert!(text.contains(&format!("G-3 debug: injected {item}")), "{label}:\n{text}");
            let finding = text.lines().find(|l| l.starts_with("finding: G-3 debug: "));
            let Some(finding) = finding else { panic!("{label}: no FINDING:\n{text}") };
            eprintln!("{label}: {finding}");
            // The same inputs, no hook: taken.
            let (ok, err) = p.link("o", &["--incremental=update", "--incremental-report"]);
            assert!(ok && err.contains("incremental: update ("), "{label}: then {err}");
            p.equals_full_link("o", &label);
        }
    }
    assert!(ran == 2 || !tools_required(), "{ran} of the 2 targets");
}

/// The R-5 a program: `-g3` (GCC puts each header's macros in a `wm4.*`
/// group of `.debug_macro`), `c.cc` FIRST in link order, so it keeps the
/// groups it shares with `main.cc` (`wm4.0.*`, `wm4.stdcpredef.h.*`).
fn r5a_prog(arch: Arch, case: &str) -> Option<Prog> {
    let c: &'static str = Box::leak(c_lit(7).into_boxed_str());
    let sources = [
        Source { name: "c.cc", text: c },
        Source { name: "main.cc", text: MAIN },
        Source { name: "u.cc", text: U },
    ];
    Prog::with(arch, case, &["-g3"], &[], false, &sources)
}

// spec: incr.md R-5 (a), IN-50 step 1 (WP-230 G-3b): a same-size body edit
// of the file that keeps `-g3`'s macro groups: G-1 leaves its kept copies in
// place (`DebugPlan::comdat_in_place`), and their bytes in the new output
// equal the previous output's at their old place (judged, `N group copies`
// with N > 0); the update is taken, byte-equal to the full link.
#[test]
fn kept_group_copy_left_in_place_is_judged() {
    let mut ran = 0;
    for arch in targets() {
        let Some(mut p) = r5a_prog(arch, "r5a") else { continue };
        ran += 1;
        let label = format!("r5a {}", arch.name());
        for line in identity_case_of(&mut p, &label, &c_lit) {
            let copies = line.rsplit(", ").next().unwrap_or_default();
            assert!(
                copies.ends_with("group copies") && !copies.starts_with("0 "),
                "{label}: {line}"
            );
            eprintln!("{label}: {line}");
        }
    }
    assert!(ran == 2 || !tools_required(), "{ran} of the 2 targets");
}

// spec: incr.md R-5 (a) (WP-230 G-3b): the copy's bytes are judged: one
// byte of the previous output's copy made to differ (`MORTAR_G3_INJECT=r5a`)
// is `debug-comdat-keeper` by name, the output untouched.
#[cfg(feature = "testhooks")]
#[test]
fn kept_group_copy_that_differs_is_debug_comdat_keeper() {
    let mut ran = 0;
    for arch in targets() {
        let Some(p) = r5a_prog(arch, "r5a-inject") else { continue };
        ran += 1;
        let label = format!("r5a inject {}", arch.name());
        let (ok, err) = p.link("o", &["--incremental"]);
        assert!(ok, "{label}: the first link: {err}");
        let (line, text) = not_taken(&p, &label, &[("MORTAR_G3_INJECT", "r5a")], &|p| {
            p.rebuild_c(&c_lit(9), &[]);
        });
        assert!(line.contains("cannot update: debug-comdat-keeper"), "{label}: {line}\n{text}");
        assert!(text.contains("G-3 debug: injected r5a"), "{label}:\n{text}");
    }
    assert!(ran == 2 || !tools_required(), "{ran} of the 2 targets");
}

/// `c.cc` with a table of `n` words in `.data` (past its chunk's tail
/// reserve, `max(4096, size/16)`).
fn c_grown(n: usize) -> String {
    format!(
        "static int c_tab[{n}] = {{1}};\nint c_value(int x) {{\n  int scaled = x * 9 + c_tab[x & 1];\n  return scaled + 1;\n}}\n"
    )
}

/// `c.cc` with `n` statements more in `c_value` (about 18 bytes of code
/// each at -O0: past `.text`'s tail reserve).
fn c_body(n: usize) -> String {
    format!(
        "#define C_K 9\nint c_value(int x) {{\n  int scaled = x * C_K;\n  {}return scaled + 1;\n}}\n",
        "scaled = scaled * 3 + 1; ".repeat(n)
    )
}

// spec: incr.md R-8 (WP-230 G-3b; G-1's `place`): a body that grows past
// `.text`'s reserve makes a growth step; in an output with debug sections
// that is `debug-growth` by name (G-1's refusal binds). x86-64. On AArch64
// `delta::layout` refuses the step itself, before G-1 runs, and the identity
// check falls back `(layout)`: `.text`'s tail is a patch island's
// (`reserve-exhausted`, mortar_layout delta.rs, "its tail is in a patch
// island chunk"), and a `.data` that grows as far meets the next file
// chunk (`segment reserve (file)`): both recorded.
#[test]
fn growth_step_with_debug_is_debug_growth() {
    for arch in targets() {
        let c: &'static str = Box::leak(c_cc(7, "scaled").into_boxed_str());
        // `c.cc` between two units: its growth moves `u.cc`'s sections.
        let sources = [
            Source { name: "main.cc", text: MAIN },
            Source { name: "c.cc", text: c },
            Source { name: "u.cc", text: U },
        ];
        let Some(p) = Prog::with(arch, "growth", &["-g"], &[], false, &sources) else { continue };
        let label = format!("growth {}", arch.name());
        let (ok, err) = p.link("o", &["--incremental"]);
        assert!(ok, "{label}: the first link: {err}");
        let (line, text) = not_taken(&p, &label, &[], &|p| p.rebuild_c(&c_body(2048), &[]));
        assert!(!text.contains("finding"), "{label}:\n{text}");
        if arch == Arch::Aarch64 {
            assert!(
                line.contains("cannot update: update-not-implemented (layout)"),
                "{label}: {line}"
            );
            assert!(
                text.contains("reserve-exhausted: .text: its tail is in a patch island chunk"),
                "{label}:\n{text}"
            );
            let (line, text) = not_taken(&p, &label, &[], &|p| p.rebuild_c(&c_grown(1536), &[]));
            assert!(
                line.contains("cannot update: update-not-implemented (layout)"),
                "{label}: {line}"
            );
            assert!(text.contains("segment reserve (file)"), "{label}:\n{text}");
            continue;
        }
        assert!(line.contains("cannot update: debug-growth"), "{label}: {line}\n{text}");
        assert!(text.contains("G-3 fallback: debug::place: debug-growth"), "{label}:\n{text}");
    }
}

/// The R-5 b program: `-g3`, `c.cc` first; both units include `t.h`, so
/// `c.cc` keeps its macro group `wm4.t.h.*`.
fn r5b_prog(arch: Arch) -> Option<Prog> {
    const T: &str = "#define T_ONE 1\n#define T_TWO 2\n";
    let c: &'static str =
        Box::leak(format!("#include \"t.h\"\n{}", &c_lit(7)[12..]).into_boxed_str());
    let main: &'static str = Box::leak(format!("#include \"t.h\"\n{MAIN}").into_boxed_str());
    let sources = [
        Source { name: "t.h", text: T },
        Source { name: "c.cc", text: c },
        Source { name: "main.cc", text: main },
        Source { name: "u.cc", text: U },
    ];
    Prog::with(arch, "r5b", &["-g3"], &[], false, &sources)
}

// spec: incr.md R-5 (b) (WP-230 G-3b): the changed file kept a macro group
// (`wm4.t.h.*`) and its new version no longer includes the header: the kept
// copy moves to `main.cc`'s, which G-1's `place` refuses
// `debug-comdat-keeper`. It is not reached: the placement's inputs refuse
// first (`layout_in::layout_inputs_live`, a file of the change set that is
// not `Change::Changed`: `update-not-implemented (layout)`, recorded
// `M2b-2b fallback`), and the identity check falls back `(layout)`.
#[test]
fn kept_group_that_moves_falls_back_before_g1() {
    for arch in targets() {
        let Some(p) = r5b_prog(arch) else { continue };
        let label = format!("r5b {}", arch.name());
        let (ok, err) = p.link("o", &["--incremental"]);
        assert!(ok, "{label}: the first link: {err}");
        let (line, text) = not_taken(&p, &label, &[], &|p| {
            p.rebuild_c(&format!("// no t.h\n{}", &c_lit(7)[12..]), &[]);
        });
        assert!(line.contains("cannot update: update-not-implemented (layout)"), "{label}: {line}");
        assert!(
            text.contains("M2b-2b fallback: update-not-implemented (layout)"),
            "{label}:\n{text}"
        );
        assert!(!text.contains("G-3 debug: placed"), "{label}: G-1 ran:\n{text}");
        assert!(!text.contains("finding"), "{label}:\n{text}");
    }
}

/// `c.cc` with a type of `n` members, each named in 3 bytes or fewer (GCC
/// writes such names inline, `DW_FORM_string`: no new debug string), used
/// by a local: `.debug_info` grows by about 10 bytes a member, the code
/// barely.
fn c_typed(n: usize) -> String {
    let members = (0..n).fold(String::new(), |mut m, i| {
        use std::fmt::Write as _;
        let _ = write!(m, "int m{i};");
        m
    });
    format!(
        "struct big {{ {members} }};\nint c_value(int x) {{\n  big *b = 0; int scaled = x * 9; (void)b;\n  return scaled + 1;\n}}\n"
    )
}

// spec: incr.md IN-57 (WP-230 G-3b; G-1's `place`): a `.debug_info`
// contribution that grows past its allotment, no dead range and the
// chunk's tail (`size/32`) able to take it: `debug-compaction` by name.
#[test]
fn debug_info_that_fits_nowhere_is_debug_compaction() {
    for arch in targets() {
        let Some(p) = Prog::new(arch, "compaction", &[], false) else { continue };
        let label = format!("compaction {}", arch.name());
        let (ok, err) = p.link("o", &["--incremental"]);
        assert!(ok, "{label}: the first link: {err}");
        let (line, text) = not_taken(&p, &label, &[], &|p| p.rebuild_c(&c_typed(99), &[]));
        assert!(line.contains("cannot update: debug-compaction"), "{label}: {line}\n{text}");
        assert!(text.contains("G-3 fallback: debug::place: debug-compaction"), "{label}:\n{text}");
        assert!(!text.contains("finding"), "{label}:\n{text}");
    }
}

// spec: incr.md IN-56, B1-5 (WP-230 G-3b; G-1's `strings`): an appended
// debug string whose offset does not fit the 32-bit word that names it is
// `debug-str-limit` by name. A 4 GiB `.debug_str` is not a test's: the hook
// `MORTAR_G3_STR_LIMIT` lowers the bound to 1, so the renamed local's
// appended string is past it.
#[cfg(feature = "testhooks")]
#[test]
fn appended_string_past_the_word_is_debug_str_limit() {
    let mut ran = 0;
    for arch in targets() {
        let Some(p) = Prog::new(arch, "strlimit", &[], false) else { continue };
        ran += 1;
        let label = format!("strlimit {}", arch.name());
        let (ok, err) = p.link("o", &["--incremental"]);
        assert!(ok, "{label}: the first link: {err}");
        let (line, text) = not_taken(&p, &label, &[("MORTAR_G3_STR_LIMIT", "1")], &|p| {
            p.rebuild_c(&c_cc(9, "renamed_local_wp230b"), &[]);
        });
        assert!(line.contains("cannot update: debug-str-limit"), "{label}: {line}\n{text}");
        assert!(text.contains("G-3 fallback: debug::strings: debug-str-limit"), "{label}:\n{text}");
    }
    assert!(ran == 2 || !tools_required(), "{ran} of the 2 targets");
}

/// The section header count of the ELF file `b` (`e_shnum`, ELF64).
fn shnum(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0x3c], b[0x3d]])
}

// spec: incr.md IN-56 `new-output-section` (WP-230 G-3b): every file is
// DWARF 4 (no `.debug_line_str`, no `.debug_rnglists`); the changed file's
// new version is DWARF 5 and brings those output sections. G-1's
// `strings` would refuse `new-output-section`, but path (A)'s refill of the
// front comes first (`reference_of`, before U5 and the placement): the
// front's output has more section headers than the previous output, and
// `mortar_image::update::refill` refuses `update-not-implemented (image)`
// (its section-count check). The counts are the bytes shown.
#[test]
fn new_debug_output_section_is_image_before_g1() {
    for arch in targets() {
        let Some(p) = Prog::new(arch, "newsec", &["-gdwarf-4"], false) else { continue };
        let label = format!("newsec {}", arch.name());
        let (ok, err) = p.link("o", &["--incremental"]);
        assert!(ok, "{label}: the first link: {err}");
        let (line, text) = not_taken(&p, &label, &[], &|p| {
            p.rebuild_c_as(&c_cc(9, "scaled"), &["-g", "-gdwarf-5"]);
        });
        assert!(line.contains("cannot update: update-not-implemented (image)"), "{label}: {line}");
        assert!(!text.contains("G-3 debug: placed"), "{label}: G-1 ran:\n{text}");
        let (ok, err) = p.link("full", &["--incremental=full"]);
        assert!(ok, "{label}: the full link: {err}");
        let (was, now) = (shnum(&p.bytes("o")), shnum(&p.bytes("full")));
        eprintln!("{label}: e_shnum {was} in the previous output, {now} in the full link");
        assert!(now > was, "{label}: e_shnum {was} -> {now}");
    }
}

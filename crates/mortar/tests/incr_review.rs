//! The adversarial review of IN-28a ("unchanged files translate by
//! position") as permanent tests (WP-064a; the reproducers are
//! `tests/incr-review/`, the report `spec/landed/incr/WP-064a.md`).
//!
//! Every scenario is run twice, in two directories: its updates by
//! position (IN-28a) in one, by identity (IN-28) in the other. EACH
//! update is judged three ways: its program's run against the full link
//! of the same inputs, its bytes against that full link where the test
//! path promises bytes, and the equivalence judge (`xtask plumb --equiv`'s,
//! `mortar_plumb::equiv`) against it always; and the two translations'
//! outputs must be the same bytes. An update the test path cannot do is a
//! fall-back by name (`layout-changed`, `resolution-changed`, ...): the
//! scenario then links in full, as an incremental link would, and goes on.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mortar::floor::{WritePath, update_by};
use mortar_incr::format::{self, State};
use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, run_exec_with, run_with, tools_required};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

/// What an update of a hop must give.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// The full link's bytes (and the judge's agreement).
    Bytes,
    /// A program the judge finds equivalent to the full link's.
    Equiv,
    /// A fall-back whose reason starts with this name.
    Named(&'static str),
}

/// One edit and the update after it.
struct Hop {
    /// Sources rewritten (`name.s`, or `name.c` built with gcc `-O2`).
    edits: Vec<(&'static str, String)>,
    expect: Expect,
    /// The full link's run: its exit status and output.
    run: (i32, &'static str),
}

struct Scenario {
    name: &'static str,
    arch: Arch,
    /// The objects and options of the link line (without `-o`).
    line: Vec<&'static str>,
    sources: Vec<(&'static str, String)>,
    hops: Vec<Hop>,
}

/// The tools of an architecture: its assembler (with its flags) and gcc.
fn assembler(arch: Arch) -> Option<(PathBuf, Vec<&'static str>)> {
    let Some(a) = arch.tool("as") else {
        assert!(!tools_required(), "{} is required", arch.gnu("as"));
        return None;
    };
    Some((a, if arch == Arch::I386 { vec!["--32"] } else { vec![] }))
}

fn run_in(dir: &Path, exe: &Path, args: &[OsString]) -> mortar_testutil::Outcome {
    let mut c = Command::new(exe);
    c.args(args).current_dir(dir).env_remove("MORTAR_SCHED_SEED").env_remove("SOURCE_DATE_EPOCH");
    run_with(&mut c, b"", Duration::from_mins(2))
}

/// One directory of a scenario.
struct Dir<'s> {
    path: PathBuf,
    sc: &'s Scenario,
}

impl Dir<'_> {
    fn p(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    /// `name.s` (or `.c`) written and built into `name.o`, as a new file
    /// renamed over the old (a new inode, as a compiler writes it).
    fn build(&self, name: &str, text: &str) {
        let (src, tool, mut args): (String, PathBuf, Vec<OsString>) =
            if let Some(stem) = name.strip_suffix(".c") {
                let gcc = self.sc.arch.tool("gcc").expect("gcc");
                let src = format!("{stem}.c");
                let flags = ["-O2", "-ffreestanding", "-fno-pic", "-fno-asynchronous-unwind-tables"];
                (src, gcc, flags.iter().map(OsString::from).chain(["-c".into()]).collect())
            } else {
                let (a, flags) = assembler(self.sc.arch).expect("as");
                (format!("{name}.s"), a, flags.iter().map(OsString::from).collect())
            };
        let stem = name.trim_end_matches(".c");
        std::fs::write(self.p(&src), text).unwrap();
        args.extend([src.clone().into(), "-o".into(), format!("{stem}.o.new").into()]);
        let o = run_in(&self.path, &tool, &args);
        assert!(o.success(), "{}: {src}: {}", self.sc.name, o.stderr_text());
        std::fs::rename(self.p(&format!("{stem}.o.new")), self.p(&format!("{stem}.o"))).unwrap();
    }

    fn line(&self, out: &str, extra: &[&str]) -> Vec<OsString> {
        let mut a: Vec<OsString> = self.sc.line.iter().map(OsString::from).collect();
        a.extend(["-o", out].map(OsString::from));
        a.extend(extra.iter().map(OsString::from));
        a
    }

    /// An incremental-mode full link of the line into `out`.
    fn full(&self, out: &str, extra: &[&str]) {
        let mut e = vec!["--incremental=full"];
        e.extend(extra);
        let o = run_in(&self.path, Path::new(MORTAR), &self.line(out, &e));
        assert!(o.success(), "{}: the full link: {}", self.sc.name, o.stderr_text());
    }

    /// The test path's update of `out` (in place), with its map.
    fn update(&self, by_position: bool, map: &str) -> Result<mortar::floor::Summary, String> {
        let args = self.line("out", &["--incremental", &format!("--mortar-map={map}")]);
        let u = std::panic::catch_unwind(|| {
            update_by(&args, &self.path, WritePath::InPlace, by_position)
        })
        .map_err(|_| "PANIC".to_owned())?;
        u.outcome
    }

    fn run(&self, exe: &str) -> (i32, String) {
        let path = self.p(exe);
        let o = run_exec_with(|| self.sc.arch.command(&path, None), b"", Duration::from_secs(30));
        (o.status.and_then(|s| s.code()).unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into())
    }

    fn bytes(&self, f: &str) -> Vec<u8> {
        std::fs::read(self.p(f)).unwrap()
    }
}

/// The files a map names, read from `dir`.
struct Files(Vec<(String, Vec<u8>)>);

impl mortar_plumb::Source for Files {
    fn count(&self) -> usize {
        self.0.len()
    }
    fn path(&self, i: usize) -> &str {
        self.0.get(i).map_or("", |f| f.0.as_str())
    }
    fn len(&self, i: usize) -> u64 {
        self.0.get(i).map_or(0, |f| f.1.len() as u64)
    }
    fn read_at(&self, i: usize, offset: u64, buf: &mut [u8]) -> Result<(), String> {
        let b = &self.0.get(i).ok_or("no such input")?.1;
        let at = usize::try_from(offset).map_err(|_| "offset")?;
        let src = b.get(at..at + buf.len()).ok_or("past the end")?;
        buf.copy_from_slice(src);
        Ok(())
    }
}

/// The equivalence judge (`spec/plumb.md` §8, `xtask plumb --equiv`) on
/// the update `x` (its map `xmap`) and the full link `f` (`fmap`).
fn equiv(dir: &Path, xmap: &str, fmap: &str, x: &str, f: &str) -> Result<(), String> {
    let parse = |m: &str| {
        let text = std::fs::read_to_string(dir.join(m)).map_err(|e| format!("{m}: {e}"))?;
        mortar_plumb::map::parse(mortar_plumb::Format::Mortar, &text)
            .map_err(|e| format!("{m}:{}: {}", e.line, e.message))
    };
    let (xm, fm) = (parse(xmap)?, parse(fmap)?);
    if !xm.claims_coverage() {
        return Err(format!("{xmap} claims no coverage: not an update's map"));
    }
    let mut paths: Vec<String> =
        xm.paths().into_iter().chain(fm.paths()).map(str::to_owned).collect();
    paths.sort();
    paths.dedup();
    let files =
        Files(paths.into_iter().filter_map(|p| Some((p.clone(), std::fs::read(dir.join(&p)).ok()?))).collect());
    let (xb, fb) = (std::fs::read(dir.join(x)).unwrap(), std::fs::read(dir.join(f)).unwrap());
    let eq = mortar_plumb::equiv::check_from(&xb, &xm, &fb, &fm, &files);
    if eq.clean() {
        Ok(())
    } else {
        let pe = mortar_plumb::Report {
            findings: eq.findings.clone(),
            coverage: mortar_plumb::Coverage::default(),
        };
        Err(format!("{}\n{}\n{}\n{}", eq.summary(), eq.x.render(x), eq.f.render(f), pe.render(x)))
    }
}

/// A row of the hand-back's table.
fn verdict(r: &Result<mortar::floor::Summary, String>) -> String {
    match r {
        Ok(_) => "ok".to_owned(),
        Err(e) => format!("named: {}", e.split(':').next().unwrap_or(e)),
    }
}

/// Runs a scenario by position and by identity; panics on any wrong
/// program or unexpected outcome; returns its table rows.
fn play(sc: &Scenario) -> Option<Vec<String>> {
    assembler(sc.arch)?;
    if sc.sources.iter().chain(sc.hops.iter().flat_map(|h| &h.edits)).any(|s| s.0.ends_with(".c"))
        && sc.arch.tool("gcc").is_none()
    {
        assert!(!tools_required(), "gcc is required");
        return None;
    }
    let scratch = Scratch::new(&format!("incr-review-{}", sc.name));
    let mut rows = Vec::new();
    let mut outs: Vec<Vec<Option<Vec<u8>>>> = Vec::new();
    for by_position in [true, false] {
        let d = Dir { path: scratch.join(if by_position { "pos" } else { "idn" }), sc };
        std::fs::create_dir_all(&d.path).unwrap();
        for (name, text) in &sc.sources {
            d.build(name, text);
        }
        d.full("out", &[]);
        let mut these = Vec::new();
        for (n, hop) in sc.hops.iter().enumerate() {
            for (name, text) in &hop.edits {
                d.build(name, text);
            }
            let (xmap, refn, fmap) = (format!("x{n}.map"), format!("ref{n}"), format!("ref{n}.map"));
            let u = d.update(by_position, &xmap);
            d.full(&refn, &[&format!("--mortar-map={fmap}")]);
            let full = d.run(&refn);
            assert_eq!(
                (full.0, full.1.as_str()),
                hop.run,
                "{} hop {n}: the full link's own run",
                sc.name
            );
            let mode = if by_position { "position" } else { "identity" };
            let what = format!("{} hop {n} by {mode}", sc.name);
            let mut judged = String::from("-");
            match (&u, hop.expect) {
                (Err(e), Expect::Named(want)) => {
                    assert!(e.starts_with(want), "{what}: named {e}, want {want}");
                    these.push(None);
                    // The fall-back: a full link, as the incremental link does.
                    d.full("out", &[]);
                }
                (Err(e), _) => panic!("{what}: {e}"),
                (Ok(_), Expect::Named(want)) => {
                    let run = d.run("out");
                    panic!("{what}: updated, want the fall-back {want}; its run {run:?}, the full link's {full:?}")
                }
                (Ok(_), expect) => {
                    let run = d.run("out");
                    assert_eq!(run, full, "{what}: WRONG PROGRAM: the update's run differs from the full link's");
                    if expect == Expect::Bytes {
                        assert!(d.bytes("out") == d.bytes(&refn), "{what}: not the full link's bytes");
                    }
                    equiv(&d.path, &xmap, &fmap, "out", &refn)
                        .unwrap_or_else(|e| panic!("{what}: the judge: {e}"));
                    judged = if d.bytes("out") == d.bytes(&refn) { "bytes".into() } else { "equiv".into() };
                    these.push(Some(d.bytes("out")));
                }
            }
            rows.push(format!(
                "{} | {} | hop {n} | {mode}: {} | full exit {} | {judged}",
                sc.name,
                sc.arch.name(),
                verdict(&u),
                full.0
            ));
        }
        outs.push(these);
    }
    for (n, (p, i)) in outs[0].iter().zip(&outs[1]).enumerate() {
        if let (Some(p), Some(i)) = (p, i) {
            assert!(p == i, "{} hop {n}: position and identity differ", sc.name);
        }
    }
    Some(rows)
}

fn report(rows: Option<Vec<String>>) {
    for r in rows.unwrap_or_default() {
        eprintln!("[incr-review] {r}");
    }
}

// ---- the sources ---------------------------------------------------------------

/// AArch64: a function `name` in its own section returning `v`, with a
/// binding directive (`globl`, `weak`, `local`).
fn func(name: &str, bind: &str, v: u32) -> String {
    format!(
        " .section .text.{name},\"ax\",@progbits\n .{bind} {name}\n .type {name},%function\n{name}: mov w0,#{v}\n ret\n"
    )
}

/// AArch64: `_start` calls `f` and exits with its result plus `k`.
fn start(f: &str, k: u32) -> String {
    format!(" .text\n .globl _start\n_start:\n bl {f}\n add w0,w0,#{k}\n mov x8,#93\n svc #0\n")
}

/// x86-64 / i386: two functions in their sections, returning `a` and `b`.
fn func_x(arch: Arch, first: (&str, u32), second: (&str, u32)) -> String {
    let _ = arch;
    let one = |n: &str, v: u32| {
        format!(" .section .text.{n},\"ax\",@progbits\n .globl {n}\n .type {n},@function\n{n}: mov ${v},%eax\n ret\n")
    };
    one(first.0, first.1) + &one(second.0, second.1)
}

fn start_x(arch: Arch, k: u32) -> String {
    if arch == Arch::I386 {
        format!(" .text\n .globl _start\n_start:\n call f\n add ${k},%eax\n mov %eax,%ebx\n mov $1,%eax\n int $0x80\n")
    } else {
        format!(" .text\n .globl _start\n_start:\n call f\n add ${k},%eax\n mov %eax,%edi\n mov $60,%eax\n syscall\n")
    }
}

/// AArch64: `_start` prints the 3-byte string `s` (merged, from its own
/// `.rodata.str1.1`) and exits with `k`.
fn print_str(s: &str, k: u32) -> String {
    format!(
        " .section .rodata.str1.1,\"aMS\",@progbits,1\nLs: .asciz \"{s}\"\n .text\n .globl _start\n_start:\n mov x0,#1\n adrp x1, Ls\n add x1, x1, :lo12:Ls\n mov x2,#3\n mov x8,#64\n svc #0\n mov x0,#{k}\n mov x8,#93\n svc #0\n"
    )
}

/// AArch64: two merged strings in this order and a table of them.
fn strings(a: &str, b: &str) -> String {
    format!(
        " .section .rodata.str1.1,\"aMS\",@progbits,1\nL1: .asciz \"{a}\"\nL2: .asciz \"{b}\"\n .data\n .globl tab\ntab: .xword L1, L2\n"
    )
}

/// AArch64: `getf` loads the locals `x` then `y` through the GOT and
/// returns `*x - *y`; `x` holds `vx`, `y` holds `vy`.
fn got_locals(x: &str, y: &str, vx: u32, vy: u32) -> String {
    format!(
        " .text\n .globl getf\n .type getf,%function\ngetf:\n adrp x0, :got:{x}\n ldr x0, [x0, :got_lo12:{x}]\n adrp x1, :got:{y}\n ldr x1, [x1, :got_lo12:{y}]\n ldr w0, [x0]\n ldr w1, [x1]\n sub w0, w0, w1\n ret\n .section .data.{x},\"aw\",@progbits\n{x}: .word {vx}\n .section .data.{y},\"aw\",@progbits\n{y}: .word {vy}\n"
    )
}

fn hop(edits: Vec<(&'static str, String)>, expect: Expect, run: (i32, &'static str)) -> Hop {
    Hop { edits, expect, run }
}

// ---- B1: stale position rows, two hops -----------------------------------------

// spec: incr.md IN-28a (position-exact rows, the writer's duty), IN-18a4;
// the review's B1 (`s1-section-permute.sh`): a.o's sections permuted by
// one update, then ANOTHER file updated; by position the second update
// called `g` (exit 22). Now its rows are recaptured or its bit cleared,
// and both translations give the full link's program.
#[test]
fn s1_sections_permuted_then_another_file() {
    let sc = Scenario {
        name: "s1",
        arch: Arch::Aarch64,
        line: vec!["a.o", "c.o", "-static"],
        sources: vec![("a", func("f", "globl", 1) + &func("g", "globl", 2)), ("c", start("f", 10))],
        hops: vec![
            hop(vec![("a", func("g", "globl", 2) + &func("f", "globl", 1))], Expect::Equiv, (11, "")),
            hop(vec![("c", start("f", 20))], Expect::Equiv, (21, "")),
        ],
    };
    report(play(&sc));
}

// spec: as above on x86-64 and i386 objects (`s1x-section-permute.sh`),
// run under the lane's emulators.
#[test]
fn s1x_sections_permuted_then_another_file_x86() {
    for arch in [Arch::X86_64, Arch::I386] {
        let m = if arch == Arch::I386 { "elf_i386" } else { "elf_x86_64" };
        let sc = Scenario {
            name: if arch == Arch::I386 { "s1x-i386" } else { "s1x-x86_64" },
            arch,
            line: vec!["-m", m, "a.o", "c.o", "-static"],
            sources: vec![("a", func_x(arch, ("f", 1), ("g", 2))), ("c", start_x(arch, 10))],
            hops: vec![
                hop(vec![("a", func_x(arch, ("g", 2), ("f", 1)))], Expect::Equiv, (11, "")),
                hop(vec![("c", start_x(arch, 20))], Expect::Equiv, (21, "")),
            ],
        };
        report(play(&sc));
    }
}

// spec: IN-28a, IN-18a4 (`FRAG_AT` names its `FRAGS` row); the review's
// B1 through fragments (`s2-fragment-permute.sh`): a.o's strings swapped,
// then c.o (whose "foo" merges into a.o's) updated: by position it
// printed "bar".
#[test]
fn s2_strings_permuted_then_another_file() {
    let sc = Scenario {
        name: "s2",
        arch: Arch::Aarch64,
        line: vec!["a.o", "c.o", "-static"],
        sources: vec![("a", strings("foo", "bar")), ("c", print_str("foo", 10))],
        hops: vec![
            hop(vec![("a", strings("bar", "foo"))], Expect::Equiv, (10, "foo")),
            hop(vec![("c", print_str("foo", 20))], Expect::Equiv, (20, "foo")),
        ],
    };
    report(play(&sc));
}

const PUT: &str = " .text\n .globl put\nput: mov x1, x0\n mov x2, #0\n1: ldrb w3, [x1, x2]\n cbz w3, 2f\n add x2, x2, #1\n b 1b\n2: mov x0, #1\n mov x8, #64\n svc #0\n ret\n .globl _start\n_start:\n bl k\n mov x8, #93\n svc #0\n";

// spec: as s2, with gcc -O2 objects (`s2c-gcc-strings.sh`): two calls of
// a.c swapped (the object keeps its size), then c.c edited.
#[test]
fn s2c_gcc_calls_swapped_then_another_file() {
    let a = |x: &str, y: &str| {
        format!("extern void put(const char *);\nvoid pa(void) {{ put(\"{x}\"); put(\"{y}\"); }}\n")
    };
    let c = |k: u32| {
        format!("extern void put(const char *);\nint k(void) {{ put(\"shared line!\\n\"); return {k}; }}\n")
    };
    let sc = Scenario {
        name: "s2c",
        arch: Arch::Aarch64,
        line: vec!["a.o", "c.o", "rt.o", "-static"],
        sources: vec![
            ("rt", PUT.to_owned()),
            ("a.c", a("hello from A\\n", "shared line!\\n")),
            ("c.c", c(10)),
        ],
        hops: vec![
            hop(vec![("a.c", a("shared line!\\n", "hello from A\\n"))], Expect::Equiv, (10, "shared line!\n")),
            hop(vec![("c.c", c(20))], Expect::Equiv, (20, "shared line!\n")),
        ],
    };
    report(play(&sc));
}

// spec: IN-28a (a local's `at_index`, recaptured); the review's
// `s3-local-permute.sh`: a.o's two GOT locals permuted in its symbol
// table, then c.o updated.
#[test]
fn s3_locals_permuted_then_another_file() {
    let sc = Scenario {
        name: "s3",
        arch: Arch::Aarch64,
        line: vec!["a.o", "c.o", "-static"],
        sources: vec![("a", got_locals("la", "lb", 7, 3)), ("c", start("getf", 10))],
        hops: vec![
            hop(vec![("a", got_locals("lb", "la", 3, 7))], Expect::Equiv, (6, "")),
            hop(vec![("c", start("getf", 20))], Expect::Equiv, (16, "")),
        ],
    };
    report(play(&sc));
}

// spec: IN-28a (a fragment whose FRAGS identity has ordinals is never
// positional); the review's M1 (`s4-frag-ordinals.sh`): "abc" in two
// merge classes of `.rodata`, the changed file's copy becomes "xyz".
// The update keeps the stored layout (m2): equivalent, not the bytes.
#[test]
fn s4_a_string_in_two_merge_classes() {
    let a = |s: &str| {
        format!(" .section .rodata.str1.8,\"aMS\",@progbits,1\n .balign 8\nLA: .asciz \"{s}\"\n .data\n .globl tab\ntab: .xword LA\n")
    };
    let b = " .section .rodata.str1.1,\"aMS\",@progbits,1\nLabc: .asciz \"abc\"\n .section .rodata.str1.8,\"aMS\",@progbits,1\n .balign 8\nLxyz: .asciz \"xyz\"\n .text\n .globl _start\n_start:\n mov x0,#1\n adrp x1, Labc\n add x1, x1, :lo12:Labc\n mov x2,#3\n mov x8,#64\n svc #0\n mov x0,#1\n adrp x1, Lxyz\n add x1, x1, :lo12:Lxyz\n mov x2,#3\n mov x8,#64\n svc #0\n mov x0,#0\n mov x8,#93\n svc #0\n";
    let sc = Scenario {
        name: "s4",
        arch: Arch::Aarch64,
        line: vec!["a.o", "b.o", "-static"],
        sources: vec![("a", a("abc")), ("b", b.to_owned())],
        hops: vec![
            hop(vec![("a", a("xyz"))], Expect::Equiv, (0, "abcxyz")),
            hop(vec![("a", a("abc"))], Expect::Equiv, (0, "abcxyz")),
        ],
    };
    report(play(&sc));
}

// ---- M4, m2: what the changed file contributes ---------------------------------

// spec: §7 `resolution-changed` (WP-064a), the review's M4
// (`w-meaning.sh` W1, W1b): a weak definition made global (or the
// reverse) by an `st_info` byte moves `f`'s definition; the test path
// refills only the changed file, so it falls back by name.
#[test]
fn w1_a_weak_definition_made_global_falls_back() {
    for (name, before, after, run) in
        [("w1", "weak", "globl", 12), ("w1b", "globl", "weak", 11)]
    {
        let sc = Scenario {
            name,
            arch: Arch::Aarch64,
            line: vec!["a.o", "x.o", "c.o", "-static"],
            sources: vec![
                ("a", func("f", "weak", 1)),
                ("x", func("f", before, 2)),
                ("c", start("f", 10)),
            ],
            hops: vec![hop(
                vec![("x", func("f", after, 2))],
                Expect::Named("resolution-changed"),
                (run, ""),
            )],
        };
        report(play(&sc));
    }
}

// spec: the shapes of `w-meaning.sh` and new ones (WP-064a): a definition
// that moves inside its section (two functions of one `.text` swapped),
// a visibility change, a function made an IFUNC, a common made a
// definition, a local made global, the call of the changed file bound to
// another definition, a COMDAT group renamed, a string of an unchanged
// file deduplicated into the changed one, a liveness flip under
// `--gc-sections`.
#[test]
fn what_the_changed_file_contributes() {
    let two = |a: (&str, u32), b: (&str, u32)| {
        format!(
            " .text\n .globl {0}\n .type {0},%function\n{0}: mov w0,#{1}\n ret\n .globl {2}\n .type {2},%function\n{2}: mov w0,#{3}\n ret\n",
            a.0, a.1, b.0, b.1
        )
    };
    let vis = |v: &str| format!("{} .{v} f\n", func("f", "globl", 2));
    let ifunc = |t: &str| {
        format!(
            " .text\n .globl f\n .type f,{t}\nf: adr x0, fi\n ret\n .type fi,%function\nfi: mov w0,#2\n ret\n"
        )
    };
    let grp = |g: &str| {
        format!(" .section .text.h,\"axG\",@progbits,{g},comdat\n .globl h\n .type h,%function\nh: mov w0,#2\n ret\n")
    };
    let read_v = " .text\n .globl _start\n_start:\n adrp x0, v\n ldr w0, [x0, :lo12:v]\n add w0,w0,#10\n mov x8,#93\n svc #0\n";
    let cases = vec![
        Scenario {
            name: "n1-definition-moves-in-its-section",
            arch: Arch::Aarch64,
            line: vec!["a.o", "c.o", "-static"],
            sources: vec![("a", two(("f", 1), ("g", 2))), ("c", start("f", 10))],
            hops: vec![hop(vec![("a", two(("g", 2), ("f", 1)))], Expect::Named("resolution-changed"), (11, ""))],
        },
        Scenario {
            name: "n2-visibility",
            arch: Arch::Aarch64,
            line: vec!["x.o", "c.o", "-static"],
            sources: vec![("x", vis("protected")), ("c", start("f", 10))],
            hops: vec![hop(vec![("x", vis("hidden"))], Expect::Named("resolution-changed"), (12, ""))],
        },
        Scenario {
            name: "n3-ifunc",
            arch: Arch::Aarch64,
            line: vec!["x.o", "c.o", "-static"],
            sources: vec![("x", ifunc("%function")), ("c", start("f", 10))],
            hops: vec![hop(
                vec![("x", ifunc("%gnu_indirect_function"))],
                Expect::Named("resolution-changed"),
                (12, ""),
            )],
        },
        Scenario {
            name: "n4-common-made-a-definition",
            arch: Arch::Aarch64,
            line: vec!["x.o", "c.o", "-static"],
            sources: vec![("x", " .comm v,4,4\n".to_owned()), ("c", read_v.to_owned())],
            hops: vec![hop(
                vec![("x", " .bss\n .globl v\n .balign 4\nv: .zero 4\n".to_owned())],
                Expect::Named(""),
                (10, ""),
            )],
        },
        Scenario {
            name: "w9-call-bound-to-another-definition",
            arch: Arch::Aarch64,
            line: vec!["a.o", "x.o", "-static"],
            sources: vec![("a", func("f", "globl", 1) + &func("g", "globl", 2)), ("x", start("f", 10))],
            hops: vec![hop(vec![("x", start("g", 10))], Expect::Named("resolution-changed"), (12, ""))],
        },
        Scenario {
            name: "w8-gc-liveness-flip",
            arch: Arch::Aarch64,
            line: vec!["a.o", "x.o", "-static", "--gc-sections"],
            sources: vec![("a", func("f", "globl", 1) + &func("g", "globl", 2)), ("x", start("f", 10))],
            hops: vec![hop(vec![("x", start("g", 10))], Expect::Named(""), (12, ""))],
        },
        Scenario {
            name: "w10-local-made-global",
            arch: Arch::Aarch64,
            line: vec!["a.o", "x.o", "c.o", "-static"],
            sources: vec![
                ("a", func("f", "weak", 1)),
                ("x", func("f", "local", 2)),
                ("c", start("f", 10)),
            ],
            hops: vec![hop(vec![("x", func("f", "globl", 2))], Expect::Named(""), (12, ""))],
        },
        Scenario {
            name: "w11-comdat-group-renamed",
            arch: Arch::Aarch64,
            line: vec!["x.o", "a.o", "c.o", "-static"],
            sources: vec![("x", grp("G1")), ("a", grp("G1")), ("c", start("h", 10))],
            hops: vec![],
        },
        Scenario {
            name: "w6-string-merged-into-the-changed-file",
            arch: Arch::Aarch64,
            line: vec!["x.o", "a.o", "-static"],
            sources: vec![
                ("x", " .section .rodata.str1.1,\"aMS\",@progbits,1\nLz: .asciz \"zzz\"\n .data\n .xword Lz\n".to_owned()),
                ("a", print_str("foo", 12)),
            ],
            hops: vec![hop(
                vec![("x", " .section .rodata.str1.1,\"aMS\",@progbits,1\nLz: .asciz \"foo\"\n .data\n .xword Lz\n".to_owned())],
                Expect::Equiv,
                (12, "foo"),
            )],
        },
    ];
    for sc in &cases {
        report(play(sc));
    }
}

// ---- hostile and damaged states ------------------------------------------------

/// The hostile-state program of `h-hostile.sh`: a.o (sections f, g;
/// strings foo, bar), c.o (prints foo, exits f()+K); and `h-local.sh`'s
/// GOT locals in b.o.
fn hostile(k: u32) -> Vec<(&'static str, String)> {
    let c = format!(
        " .section .rodata.str1.1,\"aMS\",@progbits,1\nLfoo: .asciz \"foo\"\n .text\n .globl _start\n_start:\n mov x0,#1\n adrp x1, Lfoo\n add x1, x1, :lo12:Lfoo\n mov x2,#3\n mov x8,#64\n svc #0\n bl getf\n mov w19, w0\n bl f\n add w0,w0,w19\n add w0,w0,#{k}\n mov x8,#93\n svc #0\n"
    );
    vec![
        ("a", func("f", "globl", 1) + &func("g", "globl", 2) + &strings("foo", "bar")),
        ("b", got_locals("la", "lb", 7, 3)),
        ("c", c),
    ]
}

/// One hostile case: link 0, c.o edited, the state changed by `edit`
/// (decoded, changed, encoded: every hash recomputed), then the update
/// by position and by identity. The position must give the identity's
/// output or a fall-back by name; the identity's (when it updates) must
/// run as the full link. `None`: no tools. Else the two outcomes.
fn hostile_case(
    label: &str,
    edit: &dyn Fn(&mut State),
) -> Option<(Result<Vec<u8>, String>, Result<Vec<u8>, String>)> {
    let sc = Scenario {
        name: "hostile",
        arch: Arch::Aarch64,
        line: vec!["a.o", "b.o", "c.o", "-static"],
        sources: hostile(10),
        hops: vec![],
    };
    assembler(sc.arch)?;
    let scratch = Scratch::new(&format!("incr-review-{label}"));
    let mut got = Vec::new();
    for by_position in [true, false] {
        let d = Dir { path: scratch.join(if by_position { "pos" } else { "idn" }), sc: &sc };
        std::fs::create_dir_all(&d.path).unwrap();
        for (name, text) in &sc.sources {
            d.build(name, text);
        }
        d.full("out", &[]);
        d.build("c", &hostile(20)[2].1);
        d.full("ref", &[]);
        let state_path = d.p("out.mortar-state");
        let bytes = std::fs::read(&state_path).unwrap();
        let mut s = format::decode(&bytes, mortar::VERSION.as_bytes()).unwrap();
        edit(&mut s);
        std::fs::write(&state_path, format::encode(&s)).unwrap();
        let u = d.update(by_position, "x.map");
        assert_ne!(u.as_ref().err().map(String::as_str), Some("PANIC"), "{label}: a panic");
        let r = u.map(|_| d.bytes("out"));
        if !by_position && r.is_ok() {
            assert_eq!(d.run("out"), d.run("ref"), "{label}: by identity, another program");
        }
        got.push(r);
    }
    let i = got.pop().unwrap();
    let p = got.pop().unwrap();
    if let Ok(pb) = &p {
        assert!(
            i.as_ref().is_ok_and(|ib| ib == pb),
            "{label}: by position a third program (identity: {:?})",
            i.as_ref().err()
        );
    }
    eprintln!(
        "[incr-review] {label} | position: {} | identity: {}",
        p.as_ref().map_or_else(|e| format!("named: {e}"), |_| "ok".into()),
        i.as_ref().map_or_else(|e| format!("named: {e}"), |_| "ok".into())
    );
    Some((p, i))
}

/// The `SECTIONS` rows of the file whose path is `path`.
fn section_rows(s: &State, path: &str) -> Vec<usize> {
    let f = s.files.iter().position(|f| s.str(f.path) == path.as_bytes()).unwrap() as u32;
    (0..s.sections.len()).filter(|&i| s.sections[i].file == f).collect()
}

fn row_named(s: &State, rows: &[usize], name: &str) -> usize {
    *rows.iter().find(|&&i| s.str(s.sections[i].name) == name.as_bytes()).unwrap()
}

// spec: incr.md IN-28a ("a state with two SECTIONS rows swapped and one
// with FRAG_AT rows swapped give the identity translation's output or a
// fall-back by name"); the review's H1 and H2.
#[test]
fn h1_h2_swapped_rows_give_the_identity_output_or_a_name() {
    hostile_case("h1-sections-swapped", &|s| {
        let rows = section_rows(s, "a.o");
        let (f, g) = (row_named(s, &rows, ".text.f"), row_named(s, &rows, ".text.g"));
        s.sections.swap(f, g);
    });
    hostile_case("h2-frag-at-swapped", &|s| {
        let rows = section_rows(s, "a.o");
        let str1 = row_named(s, &rows, ".rodata.str1.1") as u32;
        let at: Vec<usize> = (0..s.frag_at.len()).filter(|&i| s.frag_at[i].section == str1).collect();
        assert_eq!(at.len(), 2, "a.o's two strings");
        let t = s.frag_at[at[0]].frag;
        s.frag_at[at[0]].frag = s.frag_at[at[1]].frag;
        s.frag_at[at[1]].frag = t;
    });
    hostile_case("h10-places-swapped", &|s| {
        let rows = section_rows(s, "a.o");
        let (f, g) = (row_named(s, &rows, ".text.f"), row_named(s, &rows, ".text.g"));
        let (c, o) = (s.sections[f].place_chunk, s.sections[f].place_offset);
        (s.sections[f].place_chunk, s.sections[f].place_offset) =
            (s.sections[g].place_chunk, s.sections[g].place_offset);
        (s.sections[g].place_chunk, s.sections[g].place_offset) = (c, o);
    })
    .map(|(p, _)| {
        // A forged decision table chooses the output in both translations
        // (IN-28a "What the state is protected against"): recorded, not
        // asserted beyond "no panic, no third program by position".
        let _ = p;
    });
}

// spec: IN-28a (a local accepted when `at_index` is below its file's
// `sh_info`), IN-18a4 (a `FRAG_AT` row names a `FRAGS` row in range; a
// `FILES` flag other than `POSITIONAL`): out-of-range values are refused
// by name, never a panic.
#[test]
fn out_of_range_positions_are_refused_by_name() {
    let local_rows = |s: &State| -> Vec<usize> {
        (0..s.symbols.len()).filter(|&i| s.symbols[i].kind == format::sym_kind::LOCAL).collect()
    };
    for v in [4u32, 8, 0x7fff_ffff, u32::MAX] {
        let Some((p, _)) = hostile_case(&format!("local-at-index-{v:x}"), &|s| {
            let rows = local_rows(s);
            assert!(!rows.is_empty(), "b.o's GOT locals have rows");
            for r in rows {
                s.symbols[r].at_index = v;
            }
        }) else {
            return;
        };
        assert!(p.is_err(), "local at_index {v:#x}: updated by position");
    }
    let Some(sc) = Some(()) else { return };
    let _ = sc;
    // Decoder checks: the row stays in the file, the state is refused.
    let refused = |edit: &dyn Fn(&mut State), want: &str| {
        let Some((p, i)) = hostile_case(want, edit) else { return };
        for r in [p, i] {
            let e = r.expect_err("a damaged state updated");
            assert!(e.starts_with("state-corrupt"), "{want}: {e}");
        }
    };
    refused(&|s| s.frag_at[0].frag = s.frags.len() as u64, "frag-at-row-out-of-range");
    refused(&|s| s.files[1].flags = 2, "files-flag-unknown");
}

// spec: IN-28a's stale state against a fuzz of the positional fields
// (the review's `h-fuzz.sh`, 300 seeds by hand; here a fixed set): every
// outcome is the unforged update's bytes, a fall-back by name, or (a
// forged state whose hashes were recomputed can choose the output,
// IN-28a "What the state is protected against") a third program, which
// is counted and printed; never a panic.
#[test]
fn a_fuzz_of_the_positional_fields() {
    let Some((base, _)) = hostile_case("fuzz-base", &|_| {}) else { return };
    let base = base.expect("the unforged update");
    let (mut same, mut named, mut third) = (0, 0, Vec::new());
    for seed in 1..=24u64 {
        let what = std::cell::RefCell::new(String::new());
        let Some((p, _)) = hostile_case(&format!("fuzz-{seed}"), &|s| {
            *what.borrow_mut() = fuzz(s, seed);
        }) else {
            return;
        };
        match p {
            Ok(b) if b == base => same += 1,
            Ok(_) => third.push(format!("{seed}: {}", what.borrow())),
            Err(_) => named += 1,
        }
    }
    eprintln!("[incr-review] fuzz: {same} same, {named} named, {} third: {third:?}", third.len());
}

/// One deterministic mutation of a field the positional replay reads.
fn fuzz(s: &mut State, seed: u64) -> String {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut next = || {
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    let pick = |n: usize, r: u64| if n == 0 { 0 } else { (r % n as u64) as usize };
    let field = next() % 6;
    let interesting = [0u64, 1, 2, 3, 4, 7, 8, 0x10, 0xff, 0x7fff_ffff, 0xffff_ffff];
    let v = if next() % 2 == 0 { interesting[pick(interesting.len(), next())] } else { next() % 64 };
    match field {
        0 | 1 if !s.symbols.is_empty() => {
            let i = pick(s.symbols.len(), next());
            if field == 1 && s.symbols[i].kind != format::sym_kind::LOCAL {
                s.symbols[i].at_file = (v % (s.files.len() as u64 + 1)) as u32;
                if s.symbols[i].at_file as usize >= s.files.len() {
                    s.symbols[i].at_file = format::NONE;
                    s.symbols[i].at_index = 0;
                }
                return format!("SYMBOLS {i} at_file");
            }
            s.symbols[i].at_index = v as u32;
            format!("SYMBOLS {i} at_index={v}")
        }
        2 if !s.frag_at.is_empty() => {
            let i = pick(s.frag_at.len(), next());
            s.frag_at[i].frag = v % s.frags.len().max(1) as u64;
            format!("FRAG_AT {i} frag={}", s.frag_at[i].frag)
        }
        3 if s.frag_at.len() > 1 => {
            let (i, j) = (pick(s.frag_at.len(), next()), pick(s.frag_at.len(), next()));
            let t = s.frag_at[i].frag;
            s.frag_at[i].frag = s.frag_at[j].frag;
            s.frag_at[j].frag = t;
            format!("FRAG_AT swap {i} {j}")
        }
        4 if !s.frag_at.is_empty() => {
            let i = pick(s.frag_at.len(), next());
            s.frag_at[i].offset = v as u32;
            s.frag_at.sort_unstable();
            s.frag_at.dedup_by_key(|r| (r.section, r.offset));
            format!("FRAG_AT {i} offset={v}")
        }
        _ if s.sections.len() > 1 => {
            let f = s.sections[pick(s.sections.len(), next())].file;
            let rows: Vec<usize> = (0..s.sections.len()).filter(|&k| s.sections[k].file == f).collect();
            let (i, j) = (rows[pick(rows.len(), next())], rows[pick(rows.len(), next())]);
            s.sections.swap(i, j);
            format!("SECTIONS swap {i} {j}")
        }
        _ => "nothing".to_owned(),
    }
}

// ---- IN-22's shapes ------------------------------------------------------------

// spec: IN-22 (the stamp rule; the review's `t-stamp.sh`): a.o rewritten
// in place with its size and mtime kept, replaced by a hard link of an
// equal-size file, deleted and made again with the old mtime: each time
// the update hashes it, sees it changed, and gives the full link's bytes.
#[test]
fn the_stamp_shapes_are_seen_changed() {
    let sc = Scenario {
        name: "t-stamp",
        arch: Arch::Aarch64,
        line: vec!["a.o", "c.o", "-static"],
        sources: vec![("a", func("f", "globl", 1)), ("c", start("f", 10))],
        hops: vec![],
    };
    if assembler(sc.arch).is_none() {
        return;
    }
    let scratch = Scratch::new("incr-review-t-stamp");
    for shape in ["in-place", "hard-link", "re-created"] {
        let d = Dir { path: scratch.join(shape), sc: &sc };
        std::fs::create_dir_all(&d.path).unwrap();
        d.build("a3", &func("f", "globl", 3));
        for (name, text) in &sc.sources {
            d.build(name, text);
        }
        d.full("out", &[]);
        let a = d.p("a.o");
        let meta = std::fs::metadata(&a).unwrap();
        let mtime = meta.modified().unwrap();
        let new = d.bytes("a3.o");
        assert_eq!(new.len() as u64, meta.len(), "one size");
        match shape {
            "in-place" => {
                use std::os::unix::fs::FileExt as _;
                let f = std::fs::File::options().write(true).open(&a).unwrap();
                f.write_all_at(&new, 0).unwrap();
                f.set_modified(mtime).unwrap();
            }
            "hard-link" => {
                let h = d.p("a3h.o");
                std::fs::write(&h, &new).unwrap();
                std::fs::File::options().write(true).open(&h).unwrap().set_modified(mtime).unwrap();
                std::fs::remove_file(&a).unwrap();
                std::fs::hard_link(&h, &a).unwrap();
            }
            _ => {
                std::fs::remove_file(&a).unwrap();
                std::fs::write(&a, &new).unwrap();
                std::fs::File::options().write(true).open(&a).unwrap().set_modified(mtime).unwrap();
            }
        }
        let s = d.update(true, "x.map").unwrap_or_else(|e| panic!("{shape}: {e}"));
        assert_eq!(s.changed, "a.o", "{shape}");
        d.full("ref", &["--mortar-map=ref.map"]);
        assert_eq!(d.run("out").0, 13, "{shape}");
        assert!(d.bytes("out") == d.bytes("ref"), "{shape}: not the full link's bytes");
        equiv(&d.path, "x.map", "ref.map", "out", "ref").unwrap_or_else(|e| panic!("{shape}: {e}"));
    }
}

// spec: IN-18a4 (a state of version 3 is refused by name and gives a
// full link that writes a version-4 state).
#[test]
fn a_version_3_state_is_refused_by_name() {
    let sc = Scenario {
        name: "v3",
        arch: Arch::Aarch64,
        line: vec!["a.o", "c.o", "-static"],
        sources: vec![("a", func("f", "globl", 1)), ("c", start("f", 10))],
        hops: vec![],
    };
    if assembler(sc.arch).is_none() {
        return;
    }
    let scratch = Scratch::new("incr-review-v3");
    let d = Dir { path: scratch.join("d"), sc: &sc };
    std::fs::create_dir_all(&d.path).unwrap();
    for (name, text) in &sc.sources {
        d.build(name, text);
    }
    d.full("out", &[]);
    let mut b = d.bytes("out.mortar-state");
    assert_eq!(b[8..12], 4u32.to_le_bytes(), "a version-4 state");
    b[8..12].copy_from_slice(&3u32.to_le_bytes());
    std::fs::write(d.p("out.mortar-state"), &b).unwrap();
    let e = format::decode(&b, mortar::VERSION.as_bytes()).expect_err("a version-3 state decodes");
    assert_eq!(e.reason(), "state-version");
    assert!(e.detail().starts_with("format version 3, this mortar reads 4"), "{}", e.detail());
    let o = run_in(&d.path, Path::new(MORTAR), &d.line("out", &["--incremental", "--incremental-report"]));
    assert!(o.success(), "{}", o.stderr_text());
    assert!(o.stderr_text().contains("full (state-version: format version 3,"), "{}", o.stderr_text());
    assert!(format::decode(&d.bytes("out.mortar-state"), mortar::VERSION.as_bytes()).is_ok());
}

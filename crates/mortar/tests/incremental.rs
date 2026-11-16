//! The link state of incremental linking, stage A (WP-061): an
//! incremental-mode full link writes the state and the same bytes as a
//! plain link; a plain link touches no state; the reader refuses every
//! damaged state by a named reason and never panics; the fall-back
//! reasons of `spec/incr.md` §7; the replay hook (IN-61) reproduces the
//! output from the state alone, and each stored decision table matters;
//! the stamp rule (IN-21, IN-22, IN-22a) on the file systems at hand.

mod support;

use std::ffi::OsString;
use std::os::unix::fs::FileExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mortar_incr::format::{self, FLAG_UPDATING, State, StateError};
use mortar_incr::stamp::{self, Cleanliness, Tuple};
use mortar_testutil::target::Arch;
use mortar_testutil::{Outcome, Scratch, run_with, tools_required};
use support::{AARCH64, Built, Driver, Source, build, with_output};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

/// A static executable from assembly alone: text, data, read-only data,
/// a merged string section, `.eh_frame` from CFI, and a GOT load.
const ASM: &str = "\t.text\n\t.globl _start\n\t.type _start, %function\n_start:\n\t.cfi_startproc\n\
    \tadrp x1, :got:word\n\tldr x1, [x1, :got_lo12:word]\n\tadrp x2, :got:word2\n\
    \tldr x2, [x2, :got_lo12:word2]\n\tbl sfn\n\tmov x8, #93\n\tsvc #0\n\
    \t.cfi_endproc\n\t.type sfn, %function\nsfn:\n\t.cfi_startproc\n\tmov x0, #42\n\tret\n\
    \t.cfi_endproc\n\t.data\n\t.globl word\nword:\t.quad 1\n\t.globl word2\nword2:\t.quad 2\n\
    \t.section .rodata.str1.1,\"aMS\",%progbits,1\nmsg:\t.asciz \"hello\"\nm2:\t.asciz \"lo\"\n\
    m3:\t.asciz \"world\"\nm4:\t.asciz \"again\"\n\
    \t.section .rodata\n\t.quad msg\n\t.quad m3\n\t.quad m4\n\
    \t.section .note.GNU-stack,\"\",%progbits\n";

/// An ADRP at a page's `0xff8` whose load is two words on: erratum
/// 843419's site, fixed through a patch (from `tests/pipeline.rs`). The
/// site loads a GOT slot, and `_start` another, so that the patch's words
/// depend on the slots' order; three merged strings.
const ASM_E843419: &str = "    .text\n    .globl _start\n_start:\n\
    \n    adrp x1, :got:other\n    ldr x1, [x1, :got_lo12:other]\n    bl f\n    mov x8, #93\n    svc #0\n\
    \n    .section .text.f,\"ax\",@progbits\n    .balign 4096\n    .skip 0xff8\n\
    \n    .globl f\n    .type f, %function\nf:\n    adrp x0, :got:val\n    ldr x2, [sp]\n\
    \n    ldr x0, [x0, :got_lo12:val]\n    ldr x0, [x0]\n    ret\n\
    \n    .section .text.gap,\"ax\",@progbits\n    .skip 0x200000\n\
    \n    .data\n    .balign 8\n    .globl val\nval:\n    .xword 42\n    .globl other\nother:\n    .xword 7\n\
    \t.section .rodata.str1.1,\"aMS\",%progbits,1\nm1:\t.asciz \"hello\"\nm2:\t.asciz \"world\"\n\
    m3:\t.asciz \"again\"\n\t.section .rodata\n\t.quad m1\n\t.quad m2\n\t.quad m3\n\
    \t.section .note.GNU-stack,\"\",%progbits\n";

/// The same on x86-64: a GOT load, a call, CFI, data and merged strings.
const ASM_X86_64: &str = "\t.text\n\t.globl _start\n\t.type _start, @function\n_start:\n\t.cfi_startproc\n\
    \tmovq word@GOTPCREL(%rip), %rax\n\tleaq msg(%rip), %rsi\n\tcall sfn\n\tmovl $60, %eax\n\tsyscall\n\
    \t.cfi_endproc\n\t.type sfn, @function\nsfn:\n\t.cfi_startproc\n\tmovl $42, %edi\n\tret\n\
    \t.cfi_endproc\n\t.data\n\t.globl word\nword:\t.quad 1\n\
    \t.section .rodata.str1.1,\"aMS\",@progbits,1\nmsg:\t.asciz \"hello\"\nm2:\t.asciz \"lo\"\n\
    m3:\t.asciz \"world\"\nm4:\t.asciz \"again\"\n\
    \t.section .rodata\n\t.long 7\n\t.long m3 - .\n\t.long m4 - .\n\
    \t.section .note.GNU-stack,\"\",@progbits\n";

fn run_in(dir: &Path, exe: &Path, args: &[OsString], env: &[(&str, &str)]) -> Outcome {
    let mut c = Command::new(exe);
    c.args(args).current_dir(dir).env_remove("MORTAR_SCHED_SEED").env_remove("SOURCE_DATE_EPOCH");
    for (k, v) in env {
        c.env(k, v);
    }
    run_with(&mut c, b"", Duration::from_mins(2))
}

/// A program's link line, built in its scratch directory.
struct Prog {
    _scratch: Scratch,
    built: Built,
    /// The programs' architecture (natively on its host, else under qemu).
    arch: Arch,
}

impl Prog {
    fn c() -> Option<Self> {
        Self::of("incr-c", Driver::Gcc, support::C_PROGRAM)
    }

    fn cxx() -> Option<Self> {
        Self::of("incr-cxx", Driver::Gxx, support::CXX_PROGRAM)
    }

    fn of(label: &str, driver: Driver, sources: &[Source]) -> Option<Self> {
        let scratch = Scratch::new(label);
        let Some(built) = build(&scratch, driver, &["-O1"], sources) else {
            assert!(!tools_required(), "{} is required", driver.tool());
            return None;
        };
        Some(Self { _scratch: scratch, built, arch: AARCH64 })
    }

    /// The AArch64 assembly program: `gcc -c` and a bare link line.
    fn asm() -> Option<Self> {
        Self::asm_for(AARCH64, "incr-asm", ASM, &[])
    }

    /// An erratum-843419 site (`spec/a64.md` §11.2): a patch and the
    /// redirect of its site.
    fn erratum() -> Option<Self> {
        Self::asm_for(AARCH64, "incr-e843", ASM_E843419, &["--fix-cortex-a53-843419"])
    }

    /// The x86-64 assembly program.
    fn asm_x86_64() -> Option<Self> {
        Self::asm_for(Arch::X86_64, "incr-asm-x86-64", ASM_X86_64, &[])
    }

    fn asm_for(arch: Arch, label: &str, text: &str, extra: &[&str]) -> Option<Self> {
        let Some(gcc) = arch.tool("gcc") else {
            assert!(!tools_required(), "{} is required", arch.gnu("gcc"));
            return None;
        };
        let scratch = Scratch::new(label);
        let src = scratch.write("a.s", text);
        let obj = scratch.join("a.o");
        let o = run_in(
            scratch.path(),
            &gcc,
            &["-c".into(), src.into(), "-o".into(), obj.clone().into()],
            &[],
        );
        assert!(o.success(), "gcc: {}", o.stderr_text());
        let built = Built {
            dir: scratch.path().to_path_buf(),
            args: [obj.display().to_string(), "-static".into(), "--eh-frame-hdr".into()]
                .into_iter()
                .chain(extra.iter().map(|x| (*x).to_owned()))
                .chain(["-o".into(), "out".into()])
                .collect(),
        };
        Some(Self { _scratch: scratch, built, arch })
    }

    fn path(&self, name: &str) -> PathBuf {
        self.built.dir.join(name)
    }

    fn args(&self, out: &str, extra: &[&str]) -> Vec<OsString> {
        let mut a = with_output(&self.built.args, &self.path(out));
        a.extend(extra.iter().map(OsString::from));
        a
    }

    /// mortar's link to `out` with `extra`; its outcome.
    fn mortar_env(&self, out: &str, extra: &[&str], env: &[(&str, &str)]) -> Outcome {
        run_in(&self.built.dir, Path::new(MORTAR), &self.args(out, extra), env)
    }

    /// A link that must succeed; its standard error.
    fn link(&self, out: &str, extra: &[&str]) -> String {
        let o = self.mortar_env(out, extra, &[]);
        assert!(o.success(), "mortar {extra:?}: {}", o.stderr_text());
        o.stderr_text()
    }

    /// Runs the linked program `out` (no core file) under one name
    /// (`runme`: a program may print its `argv[0]`): its exit status and
    /// standard output.
    fn run(&self, out: &str) -> (Option<i32>, Vec<u8>) {
        std::fs::copy(self.path(out), self.path("runme")).unwrap();
        let (exe, args) = self.arch.runner(&self.path("runme"));
        let mut c = Command::new("sh");
        c.arg("-c").arg("ulimit -c 0; exec \"$0\" \"$@\"").arg(exe).args(args);
        c.current_dir(&self.built.dir);
        let o = run_with(&mut c, b"", Duration::from_mins(1));
        (o.status.and_then(|s| s.code()), o.stdout.clone())
    }

    fn bytes(&self, out: &str) -> Vec<u8> {
        std::fs::read(self.path(out)).unwrap()
    }

    fn state_of(&self, out: &str) -> PathBuf {
        self.path(&format!("{out}.mortar-state"))
    }

    fn read_state(&self, out: &str) -> (Vec<u8>, State) {
        let bytes = std::fs::read(self.state_of(out)).unwrap();
        let state = format::decode(&bytes, mortar::VERSION.as_bytes())
            .unwrap_or_else(|e| panic!("{}: {}", e.reason(), e.detail()));
        (bytes, state)
    }
}

// spec: incr.md IN-10, IN-11 (a plain link touches no state; an
// incremental-mode full link writes it beside the output and the same
// bytes as the plain link), IN-18a (the state round-trips)
#[test]
fn a_full_link_in_incremental_mode_is_the_plain_link() {
    for prog in [Prog::asm(), Prog::c(), Prog::cxx(), Prog::asm_x86_64()].into_iter().flatten() {
        let err = prog.link("plain", &[]);
        assert!(!err.contains("incremental"), "{err}");
        assert!(!prog.state_of("plain").exists(), "a plain link wrote a state");
        let err = prog.link("full", &["--incremental=full"]);
        assert!(!err.contains("incremental"), "{err}");
        assert!(prog.bytes("plain") == prog.bytes("full"), "the outputs differ");
        let err = prog.link("auto", &["--incremental"]);
        assert!(!err.contains("incremental"), "a first link prints no reason: {err}");
        assert!(prog.bytes("plain") == prog.bytes("auto"), "the outputs differ");
        let (bytes, state) = prog.read_state("full");
        assert_eq!(format::encode(&state), bytes, "the state does not round-trip");
        assert!(!state.chunks.is_empty() && !state.order.is_empty() && !state.files.is_empty());
        assert!(state.stat.iter().any(|r| r.tag == format::stat_tag::OUTPUT));
        assert!(state.stat.iter().any(|r| r.tag == format::stat_tag::DEVICE));
        assert!(!state.leaves.is_empty(), "the build id's leaves");
        // --incremental-state names the file; --no-incremental cancels.
        prog.link("named", &["--incremental", "--incremental-state=named.st"]);
        assert!(prog.path("named.st").exists() && !prog.state_of("named").exists());
        prog.link("off", &["--incremental", "--no-incremental"]);
        assert!(!prog.state_of("off").exists());
    }
}

fn line_with<'a>(err: &'a str, needle: &str) -> &'a str {
    err.lines()
        .find(|l| l.starts_with("incremental:"))
        .filter(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no `incremental: ...{needle}` line in:\n{err}"))
}

// spec: incr.md §7 (the fall-back reasons in their order), IN-21 (the
// output bound to the state), IN-63 (the report line)
#[test]
fn fall_back_reasons() {
    let Some(prog) = Prog::asm() else { return };
    let inc = ["--incremental", "--incremental-report"];
    line_with(&prog.link("o", &inc), "full (no-state) changed=");
    line_with(&prog.link("o", &inc), "full (update-not-implemented)");
    // Without the report, a reason other than the first link's is a line.
    line_with(&prog.link("o", &["--incremental"]), "full (update-not-implemented)");
    line_with(&prog.link("o", &["--incremental=full", "--incremental-report"]), "(full requested)");
    // The output touched: its tuple differs.
    let f = std::fs::File::options().write(true).open(prog.path("o")).unwrap();
    f.set_modified(std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000)).unwrap();
    drop(f);
    line_with(&prog.link("o", &inc), "full (output-changed: ");
    // The output rewritten in place with its size kept.
    let f = std::fs::File::options().write(true).open(prog.path("o")).unwrap();
    f.write_at(b"\x7f", 0).unwrap();
    drop(f);
    line_with(&prog.link("o", &inc), "full (output-changed: ");
    // Another option.
    line_with(
        &prog.link("o", &["--incremental", "--incremental-report", "--gc-sections"]),
        "(options-changed)",
    );
    line_with(&prog.link("o", &inc), "(options-changed)");
    // The updating flag.
    let state = prog.state_of("o");
    let mut bytes = std::fs::read(&state).unwrap();
    let at = usize::try_from(format::FLAGS_OFFSET).unwrap();
    bytes[at..at + 4].copy_from_slice(&FLAG_UPDATING.to_le_bytes());
    std::fs::write(&state, &bytes).unwrap();
    line_with(&prog.link("o", &inc), "(output-torn)");
    // Another mortar's state.
    let (_, mut s) = prog.read_state("o");
    let off = u32::try_from(s.strings.len()).unwrap();
    s.strings.extend_from_slice(b"0.0.0-another");
    s.meta.version = format::Str { off, len: 13 };
    std::fs::write(&state, format::encode(&s)).unwrap();
    line_with(&prog.link("o", &inc), "(state-version: ");
    // Damaged bytes.
    std::fs::write(&state, b"not a state").unwrap();
    line_with(&prog.link("o", &inc), "(state-corrupt: ");
    // The report to a file, appended.
    prog.link("o", &["--incremental", "--incremental-report=rep.txt"]);
    prog.link("o", &["--incremental", "--incremental-report=rep.txt"]);
    let rep = std::fs::read_to_string(prog.path("rep.txt")).unwrap();
    let lines: Vec<&str> = rep.lines().collect();
    assert_eq!(lines.len(), 2, "{rep}");
    assert!(lines[1].starts_with("incremental: full (update-not-implemented) changed="), "{rep}");
}

// spec: incr.md IN-10 (`--incremental=update` fails by name rather than
// link in full), IN-12 (what this stage does not support links plain,
// with a warning), §7 SOURCE_DATE_EPOCH
#[test]
fn update_unsupported_and_reproducible() {
    let Some(prog) = Prog::asm() else { return };
    let o = prog.mortar_env("u", &["--incremental=update"], &[]);
    assert!(!o.success());
    assert!(
        o.stderr_text().contains("--incremental=update: cannot update: no-state"),
        "{}",
        o.stderr_text()
    );
    assert!(!prog.path("u").exists() && !prog.state_of("u").exists());
    prog.link("u", &["--incremental"]);
    let o = prog.mortar_env("u", &["--incremental=update"], &[]);
    assert!(!o.success());
    assert!(
        o.stderr_text().contains("cannot update: update-not-implemented"),
        "{}",
        o.stderr_text()
    );
    // -pie (with -static: a static PIE, refused on this branch) is
    // WP-066's: the warning comes first, and no state is written.
    let o = prog.mortar_env("w", &["--incremental", "-pie"], &[]);
    let err = o.stderr_text();
    assert!(err.contains("--incremental is not supported with -"), "{err}");
    assert!(err.contains("pie; linking without a state"), "{err}");
    assert!(!prog.state_of("w").exists());
    let o = prog.mortar_env("w2", &["--incremental=update", "-pie"], &[]);
    assert!(
        !o.success() && o.stderr_text().contains("incremental-unsupported"),
        "{}",
        o.stderr_text()
    );
    let o = prog.mortar_env("e", &["--incremental"], &[("SOURCE_DATE_EPOCH", "1")]);
    assert!(o.success());
    assert!(o.stderr_text().contains("SOURCE_DATE_EPOCH is set"), "{}", o.stderr_text());
    let o = prog.mortar_env("e2", &[], &[("SOURCE_DATE_EPOCH", "1")]);
    assert!(!o.stderr_text().contains("SOURCE_DATE_EPOCH"), "a plain link: {}", o.stderr_text());
}

/// Checks the reader on one damaged state: a named reason or a state,
/// never a panic.
fn read_damaged(bytes: &[u8], what: &str) {
    let r = std::panic::catch_unwind(|| format::decode(bytes, mortar::VERSION.as_bytes()));
    match r {
        Err(payload) => {
            panic!("the reader panicked on {what}: {:?}", payload.downcast_ref::<String>())
        }
        Ok(Ok(_)) => {}
        Ok(Err(e)) => {
            assert!(matches!(e, StateError::Corrupt(_) | StateError::Version(_)));
            assert!(e.reason() == "state-corrupt" || e.reason() == "state-version", "{what}");
            assert!(!e.detail().is_empty(), "{what}: no detail");
        }
    }
}

/// A small deterministic generator (xorshift).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(n).unwrap()).unwrap()
    }
}

// spec: incr.md IN-18a (the validating reader: every damaged state is a
// named reason, never a panic), §7 state-corrupt
#[test]
fn every_damaged_state_is_a_named_reason() {
    let Some(prog) = Prog::asm() else { return };
    prog.link("o", &["--incremental"]);
    let (good, _) = prog.read_state("o");
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let mut corrupt = 0usize;
    // Every byte, three ways, and every truncation.
    for i in 0..good.len() {
        for x in [0xffu8, 0x01, 0x80] {
            let mut b = good.clone();
            b[i] ^= x;
            read_damaged(&b, &format!("byte {i} ^ {x:#x}"));
            if format::decode(&b, mortar::VERSION.as_bytes()).is_err() {
                corrupt += 1;
            } else {
                // Only the updating flag may change without a refusal.
                let flags = usize::try_from(format::FLAGS_OFFSET).unwrap();
                assert!(i == flags && u32::from(x) == FLAG_UPDATING, "byte {i} ^ {x:#x} accepted");
            }
        }
        read_damaged(&good[..i], &format!("truncated to {i}"));
        assert!(
            format::decode(&good[..i], mortar::VERSION.as_bytes()).is_err(),
            "truncated to {i}"
        );
    }
    // Random damage: runs of bytes overwritten, lengths and counts set to
    // extreme values.
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for n in 0..20_000 {
        let mut b = good.clone();
        for _ in 0..=rng.below(4) {
            let at = rng.below(b.len().saturating_sub(4));
            let v: u32 = match rng.below(4) {
                0 => u32::MAX,
                1 => 0,
                2 => u32::try_from(rng.below(1 << 20)).unwrap(),
                _ => u32::try_from(rng.next() >> 32).unwrap(),
            };
            b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        read_damaged(&b, &format!("random damage {n}"));
    }
    std::panic::set_hook(hook);
    eprintln!("{} bytes; {corrupt} of {} single-byte flips refused", good.len(), good.len() * 3);
    assert!(read_full(&good));
}

fn read_full(bytes: &[u8]) -> bool {
    format::decode(bytes, mortar::VERSION.as_bytes()).is_ok()
}

/// How a replay of a state (IN-61) ended.
enum Replayed {
    /// The output's bytes, and the verifier's findings on it (each as
    /// text; empty when it accepts the output).
    Linked(Vec<u8>, Vec<String>),
    /// The link failed: its report.
    Failed(String),
}

/// The replay of `state` to `out`, with a map, judged by the verifier.
fn replay_judged(prog: &Prog, out: &str, state: &State) -> Replayed {
    let map = format!("{out}.map");
    let map_arg = format!("--mortar-map={}", prog.path(&map).display());
    let inv = mortar::replay_link("mortar", &prog.args(out, &[&map_arg]), &prog.built.dir, state);
    match inv.outcome {
        mortar::Outcome::Linked { .. } => {
            let findings =
                support::plumb::verify(&prog.path(out), &prog.path(&map), &prog.built.dir);
            Replayed::Linked(prog.bytes(out), findings.iter().map(|f| format!("{f:?}")).collect())
        }
        mortar::Outcome::Failed(r) => {
            Replayed::Failed(r.render(&mortar_diag::Style::plain("mortar")))
        }
        mortar::Outcome::Done(t) => Replayed::Failed(t),
    }
}

/// Applies `f` to the first row, if any; whether there was one.
fn first<T>(v: &mut [T], f: impl FnOnce(&mut T)) -> bool {
    v.first_mut().map(f).is_some()
}

/// What a mutation of a stored table is: a layout the rules admit, which
/// the image must follow (changed bytes the verifier accepts), or one
/// they do not, which must fail by name or give bytes the verifier
/// rejects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Valid,
    Invalid,
}

/// One mutation of a stored decision table: its name, its kind, and the
/// change (whether it applied: the table has the rows it needs).
/// A change to a state and whether it applied.
type Mutation = (&'static str, fn(&mut State) -> bool);

/// A mutation of a decision table with its kind.
type TableMutation = (&'static str, Kind, fn(&mut State) -> bool);

/// The live `.eh_frame` records of one section: the positions in
/// `EH_RECORDS` of the first section with at least `n` listed records.
fn eh_run(s: &State, n: usize) -> Option<std::ops::Range<usize>> {
    let mut at = 0;
    while at < s.eh_records.len() {
        let sec = s.eh_records[at].section;
        let end = at + s.eh_records[at..].iter().take_while(|r| r.section == sec).count();
        if end - at >= n {
            return Some(at..end);
        }
        at = end;
    }
    None
}

/// Two fragments of one output section, adjacent in offset order, with
/// the same size (the distance to the next fragment; alignment 1): their
/// rows in `FRAGS`.
fn same_size_frags(s: &State) -> Option<(usize, usize)> {
    let mut rows: Vec<usize> = (0..s.frags.len()).collect();
    rows.sort_by_key(|&i| (s.frags[i].out, s.frags[i].offset));
    rows.windows(3).find_map(|w| {
        let [a, b, c] = [&s.frags[w[0]], &s.frags[w[1]], &s.frags[w[2]]];
        (a.out == b.out && b.out == c.out && b.offset - a.offset == c.offset - b.offset)
            .then_some((w[0], w[1]))
    })
}

#[allow(clippy::too_many_lines, reason = "one row per mutation")]
fn mutations() -> Vec<TableMutation> {
    use Kind::{Invalid, Valid};
    vec![
        ("CHUNKS", Invalid, |s| {
            let Some(c) = s.chunks.iter_mut().find(|c| c.size > 0 && c.bits & 1 != 0) else {
                return false;
            };
            c.addr += 16;
            c.offset += 16;
            true
        }),
        // Two non-allocated output sections trade section indices.
        ("CHUNKS (shndx)", Invalid, |s| {
            let na: Vec<usize> = (0..s.chunks.len())
                .filter(|&i| s.chunks[i].bits & 2 != 0 && s.chunks[i].sh_flags & 2 == 0)
                .collect();
            na.len() >= 2 && {
                let (a, b) = (na[0], na[na.len() - 1]);
                let t = s.chunks[a].shndx;
                s.chunks[a].shndx = s.chunks[b].shndx;
                s.chunks[b].shndx = t;
                true
            }
        }),
        ("ORDER", Invalid, |s| {
            let n = s.order.len();
            n >= 3 && {
                s.order.swap(1, n - 1);
                true
            }
        }),
        ("SEGMENTS", Invalid, |s| first(&mut s.segments, |g| g.addr += 0x1000)),
        ("SECTIONS", Invalid, |s| {
            s.sections
                .iter_mut()
                .find(|x| x.size > 0 && x.place_chunk != format::NONE)
                .map(|x| x.place_offset += 4)
                .is_some()
        }),
        ("FRAGS", Invalid, |s| {
            s.frags.iter_mut().find(|f| f.offset > 0).map(|f| f.offset = 0).is_some()
        }),
        // Two strings of one size trade places: a merged map the rules
        // admit.
        ("FRAGS (swap)", Valid, |s| {
            let Some((a, b)) = same_size_frags(s) else { return false };
            let (oa, ob) = (s.frags[a].offset, s.frags[b].offset);
            s.frags[a].offset = ob;
            s.frags[b].offset = oa;
            true
        }),
        ("SLOTS", Invalid, |s| {
            let n = s.slots.len();
            n >= 1 && {
                s.slots.truncate(n - 1);
                true
            }
        }),
        // Two GOT slots trade places: the rules admit any order.
        ("SLOTS (swap)", Valid, |s| {
            let got: Vec<usize> = (0..s.slots.len()).filter(|&i| s.slots[i].kind == 0).collect();
            got.len() >= 2 && {
                let (a, b) = (got[0], got[got.len() - 1]);
                let t = s.slots[a].symbol;
                s.slots[a].symbol = s.slots[b].symbol;
                s.slots[b].symbol = t;
                true
            }
        }),
        ("EH_RECORDS", Invalid, |s| {
            let n = s.eh_records.len();
            n >= 1 && {
                s.eh_records.truncate(n - 1);
                true
            }
        }),
        // S-46's order broken: two records of one section swapped.
        ("EH_RECORDS (swap)", Invalid, |s| {
            let Some(r) = eh_run(s, 3) else { return false };
            s.eh_records.swap(r.start + 1, r.start + 2);
            true
        }),
        // A live FDE dropped (S-41).
        ("EH_RECORDS (drop)", Invalid, |s| {
            let Some(r) = eh_run(s, 2) else { return false };
            s.eh_records.remove(r.start + 1);
            true
        }),
        // A record given twice.
        ("EH_RECORDS (dup)", Invalid, |s| {
            let Some(r) = eh_run(s, 2) else { return false };
            let row = s.eh_records[r.start + 1].clone();
            s.eh_records.insert(r.start + 1, row);
            true
        }),
        // A record the list left out that is not a section's first: an
        // FDE of dead code (S-41) or a CIE that S-44 removed, in place.
        ("EH_RECORDS (unlisted)", Invalid, |s| {
            let mut at = 0;
            while at < s.eh_records.len() {
                let sec = s.eh_records[at].section;
                let n = s.eh_records[at..].iter().take_while(|r| r.section == sec).count();
                for k in at..at + n - 1 {
                    let (p, q) = (s.eh_records[k].piece, s.eh_records[k + 1].piece);
                    if q > p + 1 {
                        s.eh_records.insert(k + 1, format::EhRow { section: sec, piece: p + 1 });
                        return true;
                    }
                }
                at += n;
            }
            false
        }),
        // A CIE that S-44 removed as a duplicate is kept in its section,
        // the rest of the state unchanged: a list S-49 admits, but not
        // with this placement (the chunk grows, and on AArch64 its `$d`
        // joins the symbol table). The admitted layout with its own
        // placement is `a_kept_equal_cie_is_followed`.
        ("EH_RECORDS (keep a CIE)", Invalid, |s| {
            let mut at = 0;
            while at < s.eh_records.len() {
                let sec = s.eh_records[at].section;
                let first = s.eh_records[at].piece;
                if first > 0 {
                    let row = format::EhRow { section: sec, piece: 0 };
                    s.eh_records.insert(at, row);
                    return true;
                }
                at += s.eh_records[at..].iter().take_while(|r| r.section == sec).count();
            }
            false
        }),
        ("SYMTAB_LOCALS", Invalid, |s| {
            s.symtab_locals.iter_mut().find(|x| x.len > 0).map(|x| x.len -= 1).is_some()
        }),
        // Every block after the first moved by one entry.
        ("SYMTAB_LOCALS (shift)", Invalid, |s| {
            let n = s.symtab_locals.len();
            n >= 2 && {
                for x in &mut s.symtab_locals[1..] {
                    x.start += 1;
                }
                true
            }
        }),
        ("SIZES", Invalid, |s| first(&mut s.sizes, |z| z.size += 16)),
        ("SYM_PLACE", Invalid, |s| first(&mut s.sym_place, |p| p.offset += 8)),
        ("DEFINED", Invalid, |s| first(&mut s.defined, |d| d.value += 4)),
        ("PATCHES", Invalid, |s| first(&mut s.patches, |p| p.offset += 4)),
        ("REDIRECTS", Invalid, |s| first(&mut s.redirects, |r| r.offset += 4)),
        // The WP-061 review's F8: an erratum site's redirect dropped.
        ("REDIRECTS (drop)", Invalid, |s| s.redirects.pop().is_some()),
        ("SIZED", Invalid, |s| first(&mut s.sized, |z| z.size += 8)),
        ("LAYOUT", Invalid, |s| {
            s.layout.entry += 4;
            true
        }),
    ]
}

/// The first line of a failed link's report that names the error.
fn reason(report: &str) -> &str {
    report.lines().find(|l| l.contains("error")).unwrap_or(report).trim()
}

// spec: incr.md IN-61 (the state alone reproduces the output, and every
// stored decision table is used: one changed row changes the output or
// fails the link)
#[test]
fn replay_reproduces_and_each_table_matters() {
    let progs = [Prog::asm(), Prog::c(), Prog::cxx(), Prog::asm_x86_64(), Prog::erratum()];
    for prog in progs.into_iter().flatten() {
        let dir = prog.built.dir.display().to_string();
        prog.link("plain", &[]);
        prog.link("full", &["--incremental=full"]);
        let (_, state) = prog.read_state("full");
        let plain = prog.bytes("plain");
        match replay_judged(&prog, "replayed", &state) {
            Replayed::Linked(same, f) => {
                assert!(same == plain, "{dir}: the replay differs from the plain link");
                assert!(f.is_empty(), "{dir}: the verifier rejects the plain link: {f:?}");
            }
            Replayed::Failed(r) => panic!("{dir}: the replay failed: {r}"),
        }
        let expected = prog.run("plain");
        let mut applied = Vec::new();
        for (n, (table, kind, mutate)) in mutations().into_iter().enumerate() {
            let mut s = state.clone();
            if !mutate(&mut s) {
                continue;
            }
            applied.push(table);
            let out = format!("mut{n}");
            // A valid layout: changed bytes the verifier accepts, and the
            // program runs as the plain link's. An invalid one: a named
            // failure, or bytes the verifier rejects. Never the same bytes.
            let line = match replay_judged(&prog, &out, &s) {
                Replayed::Linked(bytes, _) if bytes == plain => {
                    panic!("{dir}: {table}: the replay ignored the change")
                }
                Replayed::Linked(_, f) if f.is_empty() => {
                    assert_eq!(kind, Kind::Valid, "{dir}: {table}: an invalid layout accepted");
                    let ran = prog.run(&out);
                    assert_eq!(ran, expected, "{dir}: {table}: the program runs otherwise");
                    format!("changed, verifier accepts, runs as plain (exit {:?})", ran.0)
                }
                Replayed::Linked(_, f) => {
                    assert_eq!(kind, Kind::Invalid, "{dir}: {table}: the verifier rejects {f:?}");
                    format!("changed, verifier rejects ({}): {}", f.len(), f[0])
                }
                Replayed::Failed(r) => {
                    assert_eq!(kind, Kind::Invalid, "{dir}: {table}: {r}");
                    format!("failed: {}", reason(&r))
                }
            };
            eprintln!("OUTCOME {dir} | {table} | {kind:?} | {line}");
        }
        eprintln!("{dir}: mutated {applied:?}");
        for need in ["CHUNKS", "ORDER", "SEGMENTS", "SECTIONS", "SIZES", "LAYOUT"] {
            assert!(applied.contains(&need), "{need} was empty");
        }
    }
}

// spec: synth.md S-49 (the CIE rule: a listed CIE that S-44 would have
// removed is taken by its own FDEs), incr.md IN-61 (the image follows the
// state's list)
//
// The state of a link whose second object has its own CIE (return column
// 29) is replayed over the same objects with that CIE equal to the first
// one's (return column 30, the same size): a layout the rules admit that
// the computed link would not choose. Its bytes differ from the plain
// link's, the verifier accepts them and the program runs the same.
#[test]
fn a_kept_equal_cie_is_followed() {
    let Some(gcc) = AARCH64.tool("gcc") else {
        assert!(!tools_required(), "{} is required", AARCH64.gnu("gcc"));
        return;
    };
    let a = "\t.text\n\t.globl _start\n\t.type _start, %function\n_start:\n\t.cfi_startproc\n\
        \tbl f\n\tmov x8, #93\n\tsvc #0\n\t.cfi_endproc\n\
        \t.section .note.GNU-stack,\"\",%progbits\n";
    let b = |column: u32| {
        format!(
            "\t.text\n\t.globl f\n\t.type f, %function\nf:\n\t.cfi_startproc\n\
             \t.cfi_return_column {column}\n\tmov x0, #42\n\tret\n\t.cfi_endproc\n\
             \t.section .note.GNU-stack,\"\",%progbits\n"
        )
    };
    let scratch = Scratch::new("incr-kept-cie");
    let gcc_c = |scratch: &Scratch, src: &str, text: &str, obj: &str| {
        let src = scratch.write(src, text);
        let args: [OsString; 4] = ["-c".into(), src.into(), "-o".into(), scratch.join(obj).into()];
        let o = run_in(scratch.path(), &gcc, &args, &[]);
        assert!(o.success(), "gcc: {}", o.stderr_text());
        std::fs::read(scratch.join(obj)).unwrap()
    };
    gcc_c(&scratch, "a.s", a, "a.o");
    let own = gcc_c(&scratch, "b29.s", &b(29), "b.o");
    let built = Built {
        dir: scratch.path().to_path_buf(),
        args: vec![
            scratch.join("a.o").display().to_string(),
            scratch.join("b.o").display().to_string(),
            "-static".into(),
            "--eh-frame-hdr".into(),
            "-o".into(),
            "out".into(),
        ],
    };
    let prog = Prog { _scratch: scratch, built, arch: AARCH64 };
    prog.link("full", &["--incremental=full"]);
    let (_, state) = prog.read_state("full");
    let kept = |s: &State| s.eh_records.iter().filter(|r| r.piece == 0).count();
    assert_eq!(kept(&state), 2, "two CIEs that differ are both kept");
    // b.o again, its CIE now equal to a.o's: the same size.
    let equal = gcc_c(&prog._scratch, "b30.s", &b(30), "b.o");
    assert_eq!(own.len(), equal.len());
    prog.link("plain", &[]);
    let plain = prog.bytes("plain");
    match replay_judged(&prog, "replayed", &state) {
        Replayed::Linked(bytes, findings) => {
            assert!(bytes != plain, "the replay did not keep the CIE");
            assert!(findings.is_empty(), "the verifier rejects: {findings:?}");
            assert!(bytes.len() > plain.len());
            assert_eq!(prog.run("replayed"), prog.run("plain"));
            assert_eq!(prog.run("plain").0, Some(42));
        }
        Replayed::Failed(r) => panic!("the replay failed: {r}"),
    }
}

// spec: incr.md IN-18a (WP-086 keeps format version 1: a state written
// before the image read EH_RECORDS and SYMTAB_LOCALS replays to the same
// bytes); the fixtures' README says how they were made.
#[test]
fn a_state_written_before_wp086_replays_identically() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/incr-state-v1-51aa582");
    for (dir, objects) in [("a64", &["a.o", "b.o"][..]), ("x86_64", &["a.o"][..])] {
        let scratch = Scratch::new(&format!("incr-v1-{dir}"));
        for f in objects.iter().chain(&["out", "out.mortar-state"]) {
            std::fs::copy(root.join(dir).join(f), scratch.join(f)).unwrap();
        }
        let bytes = std::fs::read(scratch.join("out.mortar-state")).unwrap();
        let state = format::decode(&bytes, mortar::VERSION.as_bytes())
            .unwrap_or_else(|e| panic!("{dir}: {}: {}", e.reason(), e.detail()));
        let want = std::fs::read(scratch.join("out")).unwrap();
        let args = |out: &str| -> Vec<OsString> {
            objects
                .iter()
                .map(|o| (*o).into())
                .chain(["-static", "--eh-frame-hdr", "-o", out].map(OsString::from))
                .collect()
        };
        let inv = mortar::replay_link("mortar", &args("replayed"), scratch.path(), &state);
        assert!(matches!(inv.outcome, mortar::Outcome::Linked { .. }), "{dir}: replay failed");
        assert!(std::fs::read(scratch.join("replayed")).unwrap() == want, "{dir}: replay differs");
        let o = run_in(scratch.path(), Path::new(MORTAR), &args("plain"), &[]);
        assert!(o.success(), "{dir}: {}", o.stderr_text());
        assert!(std::fs::read(scratch.join("plain")).unwrap() == want, "{dir}: plain differs");
    }
}

// ---- stamps ----------------------------------------------------------------

/// The directories to try the stamp rule on: the build directory's
/// scratch (the workspace's file system), then `MORTAR_INCR_STAMP_DIR`
/// when set, the system's temporary directory and `/dev/shm`, each when
/// it is writable and on a file system not already in the list (here an
/// overlay `/tmp` and a tmpfs; on the hosted runners whatever they have).
fn stamp_dirs(scratch: &Scratch) -> Vec<PathBuf> {
    use std::os::unix::fs::MetadataExt as _;
    let mut v = vec![scratch.path().to_path_buf()];
    let mut devs = vec![std::fs::metadata(scratch.path()).unwrap().dev()];
    let more = std::env::var_os("MORTAR_INCR_STAMP_DIR")
        .map(PathBuf::from)
        .into_iter()
        .chain([std::env::temp_dir(), PathBuf::from("/dev/shm")]);
    for d in more {
        let Ok(m) = std::fs::metadata(&d) else { continue };
        let probe = d.join(format!("mortar-incr-writable-{}", std::process::id()));
        if devs.contains(&m.dev()) || std::fs::write(&probe, b"").is_err() {
            continue;
        }
        let _ = std::fs::remove_file(&probe);
        devs.push(m.dev());
        v.push(d);
    }
    v
}

fn fs_of(dir: &Path) -> String {
    use std::os::unix::fs::MetadataExt as _;
    let mountinfo = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    let dev = std::fs::metadata(dir).unwrap().dev();
    stamp::fs_type(&mountinfo, dev).unwrap_or_default()
}

// spec: incr.md IN-22, IN-22a (`stamp_rule.py` case A: an input rewritten
// with its size kept right after the link read it is never clean)
#[test]
fn stamp_rule_a_a_rewrite_after_the_read_is_never_clean() {
    let scratch = Scratch::new("incr-stamp-a");
    for dir in stamp_dirs(&scratch) {
        let input = dir.join(format!("race-input-{}.o", std::process::id()));
        std::fs::write(&input, vec![b'A'; 4096]).unwrap();
        let (mut stale, mut racy) = (0, 0);
        for i in 0..400 {
            let (_, s) = stamp::take(&dir).unwrap();
            let rec = Tuple::of(&std::fs::metadata(&input).unwrap());
            let f = std::fs::File::options().write(true).open(&input).unwrap();
            f.write_at(if i % 2 == 0 { b"B" } else { b"C" }, 0).unwrap();
            drop(f);
            let now = Tuple::of(&std::fs::metadata(&input).unwrap());
            match stamp::classify(&rec, &now, Some(s)) {
                Cleanliness::Clean => stale += 1,
                Cleanliness::Racy => racy += 1,
                Cleanliness::Changed | Cleanliness::Hashed => {}
            }
        }
        std::fs::remove_file(&input).unwrap();
        eprintln!(
            "A {} ({}): 400 trials, stale {stale}, racy (hashed) {racy}",
            dir.display(),
            fs_of(&dir)
        );
        assert_eq!(stale, 0);
    }
}

// spec: incr.md IN-21 (`stamp_rule.py` case B and `state_tick.py`: the
// link's own output, bound after its last write, is never seen as changed
// or racy by the next link)
#[test]
fn stamp_rule_b_own_output_is_clean() {
    let scratch = Scratch::new("incr-stamp-b");
    for dir in stamp_dirs(&scratch) {
        let pid = std::process::id();
        let out = dir.join(format!("race-out-{pid}.bin"));
        let state = dir.join(format!("race-out-{pid}.state"));
        let tmp = dir.join(format!("race-out-{pid}.state.tmp"));
        std::fs::File::create(&out).unwrap().set_len(8 << 20).unwrap();
        let blob = vec![0x5au8; 1 << 20];
        let (mut clean, mut unbound, mut old_rule) = (0, 0, 0);
        let n = 200;
        for i in 0..n {
            let f = std::fs::File::options().write(true).open(&out).unwrap();
            f.write_at(&blob, (i % 7) << 20).unwrap();
            f.sync_data().unwrap();
            drop(f);
            let Some((rec, s)) = stamp::bind_output(&out).unwrap() else {
                unbound += 1;
                continue;
            };
            let w = std::fs::File::create(&tmp).unwrap();
            w.write_at(&vec![b'S'; 65536], 0).unwrap();
            w.sync_data().unwrap();
            drop(w);
            std::fs::rename(&tmp, &state).unwrap();
            // The next link.
            let now = Tuple::of(&std::fs::metadata(&out).unwrap());
            if stamp::classify(&rec, &now, Some(s)) == Cleanliness::Clean {
                clean += 1;
            }
            // state_tick.py's old rule: the output's ctime not older than
            // the state's write time is "changed".
            let st = Tuple::of(&std::fs::metadata(&state).unwrap());
            if now.ctime_ns >= st.mtime_ns.min(st.ctime_ns) {
                old_rule += 1;
            }
        }
        std::fs::remove_file(&out).unwrap();
        std::fs::remove_file(&state).unwrap();
        eprintln!(
            "B {} ({}): {n} updates, own output clean {clean}, unbound {unbound}; \
             the old rule's fall-backs {old_rule}",
            dir.display(),
            fs_of(&dir)
        );
        assert_eq!((clean, unbound), (n, 0));
    }
}

// spec: incr.md IN-22a (the probe passes on the allow-listed file systems
// at hand; a stamp per device)
#[test]
fn probe_on_the_file_systems_at_hand() {
    let scratch = Scratch::new("incr-probe");
    for dir in stamp_dirs(&scratch) {
        let fs = fs_of(&dir);
        let ok = stamp::probe(&dir).unwrap();
        eprintln!("probe {} ({fs}): {ok}", dir.display());
        if stamp::allowed(&fs) {
            assert!(ok, "the probe failed on {fs}");
        }
        let mountinfo = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
        let s = stamp::stamps(&[dir.clone(), dir.join("missing"), dir.clone()], &mountinfo);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].trusted, stamp::allowed(&fs) && ok);
    }
}

// ---- the review's findings (F1-F4, F7) ---------------------------------------

/// The state with its output row's stamp set to the output's ctime as it
/// is now: the next check takes the racy path and compares the stored
/// leaves with the output's bytes.
fn forge_racy(state: &State, out: &Path) -> State {
    let mut s = state.clone();
    let now = Tuple::of(&std::fs::metadata(out).unwrap());
    for r in s.stat.iter_mut().filter(|r| r.tag == format::stat_tag::OUTPUT) {
        (r.dev, r.ino, r.size, r.mtime_ns, r.ctime_ns) =
            (now.dev, now.ino, now.size, now.mtime_ns, now.ctime_ns);
        r.stamp_ns = now.ctime_ns;
    }
    s
}

/// The reader's refusal of `s`, re-encoded: its detail.
fn refused(s: &State) -> String {
    match format::decode(&format::encode(s), mortar::VERSION.as_bytes()) {
        Ok(_) => String::new(),
        Err(e) => {
            assert_eq!(e.reason(), "state-corrupt");
            e.detail().to_owned()
        }
    }
}

// spec: incr.md IN-18a (the reader's cross-table checks: META's output
// size is the output row's; the build-id descriptor is the build id's
// length, at most 64 bytes; no duplicate rows in STAT, ORDER, SLOTS or
// DEFINED; every chunk in ORDER; FILES ranges within 2^64), review F2, F7
#[test]
fn the_reader_refuses_tables_that_disagree() {
    let progs: Vec<Prog> = [Prog::asm(), Prog::c()].into_iter().flatten().collect();
    let states: Vec<State> = progs
        .iter()
        .map(|p| {
            p.link("o", &["--incremental"]);
            p.read_state("o").1
        })
        .collect();
    let forged: Vec<Mutation> = vec![
        ("meta-size", |s| {
            s.meta.file_size += 1 << 20;
            s.leaves = vec![
                [0; 32];
                usize::try_from(mortar_digest::leaf_count(s.meta.file_size)).unwrap()
            ];
            true
        }),
        ("sealed-huge", |s| {
            s.meta.file_size = 1 << 40;
            s.leaves = vec![[0; 32]; usize::try_from(mortar_digest::leaf_count(1 << 40)).unwrap()];
            for r in s.stat.iter_mut().filter(|r| r.tag == format::stat_tag::OUTPUT) {
                r.size = 1 << 40;
            }
            s.meta.sealed_start = 0;
            s.meta.sealed_end = 1 << 40;
            true
        }),
        ("stat-dup", |s| {
            let Some(i) = s.stat.iter().position(|r| r.tag == format::stat_tag::INPUT) else {
                return false;
            };
            let r = s.stat[i].clone();
            s.stat.insert(i, r);
            true
        }),
        ("files-range", |s| {
            s.files.get_mut(1).map(|f| (f.base, f.len) = (u64::MAX - 1, u64::MAX)).is_some()
        }),
        ("order-dup", |s| {
            let Some(&c) = s.order.first() else { return false };
            s.order.push(c);
            true
        }),
        ("segment-align", |s| first(&mut s.segments, |g| g.align = 64)),
        ("order-drop", |s| s.order.pop().is_some()),
        ("slots-dup", |s| {
            let Some(r) = s.slots.first().cloned() else { return false };
            s.slots.insert(0, r);
            true
        }),
        ("defined-dup", |s| {
            let Some(r) = s.defined.first().cloned() else { return false };
            s.defined.insert(0, r);
            true
        }),
    ];
    for (name, mutate) in forged {
        let mut applied = false;
        for state in &states {
            assert!(refused(state).is_empty(), "the unforged state is refused");
            let mut s = state.clone();
            if !mutate(&mut s) {
                continue;
            }
            applied = true;
            let why = refused(&s);
            assert!(!why.is_empty(), "{name}: accepted");
            eprintln!("{name}: {why}");
        }
        assert!(applied || progs.len() < 2, "{name} applied to no state");
    }
}

// spec: incr.md §7 (a state of another target at the same path is
// options-changed), review F7
#[test]
fn a_target_change_at_the_same_path_is_options_changed() {
    let (Some(a64), Some(x64)) = (Prog::asm(), Prog::asm_x86_64()) else { return };
    let dir = Scratch::new("incr-target");
    let input = dir.join("in.o");
    let link = |from: &Prog| {
        std::fs::copy(from.path("a.o"), &input).unwrap();
        let args: Vec<OsString> =
            ["in.o", "-static", "-o", "o", "--incremental", "--incremental-report"]
                .iter()
                .map(OsString::from)
                .collect();
        let o = run_in(dir.path(), Path::new(MORTAR), &args, &[]);
        assert!(o.success(), "{}", o.stderr_text());
        o.stderr_text()
    };
    line_with(&link(&x64), "full (no-state)");
    line_with(&link(&x64), "full (update-not-implemented)");
    line_with(&link(&a64), "full (options-changed)");
    line_with(&link(&a64), "full (update-not-implemented)");
}

// spec: incr.md IN-12 (dynamic outputs link plain with a warning on
// x86-64 as on AArch64; no state is written; `-shared` is refused on
// this branch before incremental mode looks)
#[test]
fn x86_64_dynamic_outputs_are_unsupported() {
    let Some(prog) = Prog::asm_x86_64() else { return };
    let base: Vec<String> = prog.built.args.iter().filter(|a| *a != "-static").cloned().collect();
    for (out, extra, what) in [
        ("dyn", &["--dynamic-linker=/lib64/ld-linux-x86-64.so.2"][..], "a dynamic executable"),
        ("pie", &["-pie", "--dynamic-linker=/lib64/ld-linux-x86-64.so.2"][..], "-pie"),
    ] {
        let mut args = with_output(&base, &prog.path(out));
        args.extend(extra.iter().map(OsString::from));
        args.push("--incremental".into());
        let o = run_in(&prog.built.dir, Path::new(MORTAR), &args, &[]);
        let err = o.stderr_text();
        assert!(o.success(), "{out}: {err}");
        assert!(err.contains("--incremental is not supported with"), "{out}: {err}");
        assert!(err.contains(what) && err.contains("linking without a state"), "{out}: {err}");
        assert!(prog.path(out).exists() && !prog.state_of(out).exists(), "{out}");
        args.pop();
        args.push("--incremental=update".into());
        let o = run_in(&prog.built.dir, Path::new(MORTAR), &args, &[]);
        assert!(!o.success() && o.stderr_text().contains("incremental-unsupported"), "{out}");
    }
}

// spec: incr.md IN-17 (the state and the report never name a file of the
// link: refused by name before anything is written), review F4
#[test]
fn the_state_or_report_naming_a_file_of_the_link_is_refused() {
    let Some(prog) = Prog::asm() else { return };
    let input = prog.path("a.o");
    let before = std::fs::read(&input).unwrap();
    std::fs::hard_link(&input, prog.path("same.o")).unwrap();
    let ip = input.display().to_string();
    let cases: Vec<(String, Vec<String>, &str)> = vec![
        ("o".into(), vec![format!("--incremental-state={ip}")], "names the input"),
        ("o".into(), vec![format!("--incremental-report={ip}")], "names the input"),
        ("o".into(), vec!["--incremental-state=same.o".into()], "names the input"),
        ("o".into(), vec!["--incremental-state=o".into()], "names the output"),
        ("o".into(), vec!["--incremental-report=o".into()], "names the output"),
    ];
    for (out, extra, what) in &cases {
        let mut x: Vec<&str> = vec!["--incremental"];
        x.extend(extra.iter().map(String::as_str));
        let o = prog.mortar_env(out, &x, &[]);
        let err = o.stderr_text();
        assert!(!o.success(), "{extra:?}: {err}");
        assert!(err.contains(what) && err.contains("nothing is written to it"), "{extra:?}: {err}");
        assert!(!prog.path(out).exists(), "{extra:?}: an output was written");
        assert_eq!(std::fs::read(&input).unwrap(), before, "{extra:?}: the input changed");
    }
    // An input a search finds: refused after the front, before the output.
    let args: Vec<OsString> =
        ["-L", ".", "-l:a.o", "-static", "-o", "o", "--incremental", "--incremental-state=a.o"]
            .iter()
            .map(OsString::from)
            .collect();
    let o = run_in(&prog.built.dir, Path::new(MORTAR), &args, &[]);
    let err = o.stderr_text();
    assert!(!o.success() && err.contains("an input of the link"), "{err}");
    assert!(!prog.path("o").exists());
    assert_eq!(std::fs::read(&input).unwrap(), before, "the searched input changed");
}

// spec: incr.md IN-21 (the state is bound to the file the link wrote: two
// links racing to one output and one state never leave a state that is
// accepted for one link's command line and the other's output), review F3
#[test]
fn concurrent_links_never_cross_output_and_state() {
    let (Some(big), Some(small)) = (Prog::cxx(), Prog::asm()) else { return };
    let dir = Scratch::new("incr-cross");
    let out = dir.join("co");
    let common = ["--incremental-state=co.st", "--build-id=none"];
    let line = |p: &Prog, mode: &str| -> Vec<OsString> {
        let mut a = with_output(&p.built.args, &out);
        a.extend(common.iter().map(OsString::from));
        a.push(mode.into());
        a
    };
    let reason = |p: &Prog| -> String {
        let o = run_in(dir.path(), Path::new(MORTAR), &line(p, "--incremental=update"), &[]);
        let err = o.stderr_text();
        err.split("cannot update: ").nth(1).unwrap_or("").trim().to_owned()
    };
    // X's link time spreads Y's start across X's whole link.
    let started = std::time::Instant::now();
    let first = run_in(dir.path(), Path::new(MORTAR), &line(&big, "--incremental=full"), &[]);
    assert!(first.success(), "{}", first.stderr_text());
    let span = started.elapsed().as_micros().max(1000);
    let (mut accepted, mut crossed) = (0, 0);
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let runs = 40;
    for _ in 0..runs {
        let delay = Duration::from_micros(
            u64::try_from(rng.below(usize::try_from(span).unwrap())).unwrap(),
        );
        let xa = line(&big, "--incremental=full");
        std::thread::scope(|sc| {
            let handle = sc.spawn(|| run_in(dir.path(), Path::new(MORTAR), &xa, &[]));
            std::thread::sleep(delay);
            run_in(dir.path(), Path::new(MORTAR), &line(&small, "--incremental=full"), &[]);
            handle.join().unwrap();
        });
        for p in [&big, &small] {
            if reason(p) != "update-not-implemented" {
                continue;
            }
            accepted += 1;
            let st = dir.join("co.st");
            let keep = std::fs::read(&st).unwrap();
            let state = format::decode(&keep, mortar::VERSION.as_bytes()).unwrap();
            std::fs::write(&st, format::encode(&forge_racy(&state, &out))).unwrap();
            let again = reason(p);
            if again != "update-not-implemented" {
                crossed += 1;
                eprintln!("crossed: {again}");
            }
            std::fs::write(&st, keep).unwrap();
        }
    }
    eprintln!("{runs} races over {span} us: {accepted} states accepted, {crossed} crossed");
    assert_eq!(crossed, 0);
}

// spec: incr.md IN-18b (a row the option table accepts and ignores does
// not enter the plan hash: gcc's `-plugin-opt=-fresolution=` names a new
// file on every run), review F1. Each driver links twice through
// `-B<dir>` with `dir/ld` mortar; the second link finds the first's
// state.
#[test]
fn a_compiler_driver_link_twice_finds_its_state() {
    let bin = Scratch::new("incr-driver-bin");
    std::os::unix::fs::symlink(MORTAR, bin.join("ld")).unwrap();
    let b = format!("-B{}/", bin.path().display());
    let drivers = [
        (Driver::Gcc, support::C_PROGRAM),
        (Driver::Gxx, support::CXX_PROGRAM),
        (Driver::Gfortran, support::FORTRAN_PROGRAM),
        (Driver::Clang, support::C_PROGRAM),
    ];
    for (driver, sources) in drivers {
        let name = driver.tool();
        let Some(cc) = mortar_testutil::tool(&name) else {
            assert!(!tools_required(), "{name} is required");
            continue;
        };
        let scratch = Scratch::new("incr-driver");
        let mut objects: Vec<OsString> = Vec::new();
        for s in sources {
            let src = scratch.write(s.name, s.text);
            let obj = scratch.join(format!("{}.o", s.name));
            let mut a: Vec<OsString> =
                driver.target_args().into_iter().map(OsString::from).collect();
            a.extend(["-O1".into(), "-c".into(), src.into(), "-o".into(), obj.clone().into()]);
            let o = run_in(scratch.path(), &cc, &a, &[]);
            assert!(o.success(), "{name} -c: {}", o.stderr_text());
            objects.push(obj.into());
        }
        let mut a: Vec<OsString> = driver.target_args().into_iter().map(OsString::from).collect();
        // Clang finds `ld` in -B's directory only for its default linker.
        if matches!(driver, Driver::Clang) {
            a.push("-fuse-ld=ld".into());
        }
        a.extend([b.clone().into(), "-static".into()]);
        a.extend(objects);
        a.extend(["-o".into(), "out".into(), "-Wl,--incremental,--incremental-report".into()]);
        let first = run_in(scratch.path(), &cc, &a, &[]);
        assert!(first.success(), "{name}: {}", first.stderr_text());
        line_with(&first.stderr_text(), "full (no-state)");
        let second = run_in(scratch.path(), &cc, &a, &[]);
        assert!(second.success(), "{name}: {}", second.stderr_text());
        let err = second.stderr_text();
        let line = err.lines().find(|l| l.starts_with("incremental:")).unwrap_or("");
        eprintln!("{name}: second link: {line}");
        line_with(&err, "full (update-not-implemented)");
    }
}

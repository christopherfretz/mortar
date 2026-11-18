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
use mortar_testutil::{Outcome, Scratch, run_exec_with, run_with, tools_required};
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

/// Two calls beyond `bl`'s reach (AAELF64 "Call and Jump relocations";
/// as `tests/pipeline.rs`'s `a_far_call_goes_through_a_thunk`): 128 MiB
/// of code between `_start` and `far`/`far2`, so the calls go through two
/// thunks in one island (L-110, L-115), named in `.symtab` (S-74). Exits
/// 42 only if both calls arrive, in order. (The object and each output
/// are 128 MiB; the replay test removes each output once judged.)
const ASM_THUNKS: &str = "    .text\n    .globl _start\n    .type _start, %function\n_start:\n\
    \n    mov x0, #0\n    bl far\n    bl far2\n    mov x8, #93\n    svc #0\n\
    \n    .section .text.gap1,\"ax\",@progbits\n    .skip 0x4000000\n\
    \n    .section .text.gap2,\"ax\",@progbits\n    .skip 0x4000000\n\
    \n    .section .text.far,\"ax\",@progbits\n    .globl far\n    .type far, %function\n\
    far:\n    add x0, x0, #40\n    ret\n    .globl far2\n    .type far2, %function\n\
    far2:\n    lsl x0, x0, #1\n    sub x0, x0, #38\n    ret\n\
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

/// x86-64 large sections (`spec/x64.md` X-52, X-54): `.lrodata`, `.ldata`,
/// `.lbss` and a large common, reached by 64-bit absolute addresses; their
/// chunks, load classes and segments go through the state like any other.
const ASM_X86_64_LARGE: &str = "\t.text\n\t.globl _start\n\t.type _start, @function\n_start:\n\t.cfi_startproc\n\
    \tmovabsq $lr, %rax\n\tmovabsq $ld, %rcx\n\tmovabsq $lb, %rdx\n\tmovabsq $lc, %rsi\n\
    \tmovl (%rax), %edi\n\taddl (%rcx), %edi\n\tmovl %edi, (%rdx)\n\tmovl %edi, 65532(%rsi)\n\
    \tmovl $60, %eax\n\tsyscall\n\t.cfi_endproc\n\
    \t.data\n\t.globl word\nword:\t.quad 1\n\
    \t.section .lrodata,\"al\",@progbits\nlr:\t.long 40\n\
    \t.section .ldata,\"awl\",@progbits\nld:\t.long 2\n\t.quad lr\n\
    \t.section .lbss,\"awl\",@nobits\nlb:\t.zero 65536\n\
    \t.largecomm lc,65536,32\n\
    \t.section .note.GNU-stack,\"\",@progbits\n";

/// Commons and a RELRO run (`spec/plumb.md` §3.2, the allocation
/// records): four commons, `cb` asked 32-aligned here and 64-aligned by
/// a second definition, and a `.data.rel.ro` word, so the RELRO run
/// ends with layout's NOBITS `.pad.relro` (L-64). Exits 0.
const ASM_COMMONS: &str = "\t.text\n\t.globl _start\n\t.type _start, %function\n_start:\n\
    \tmov x0, #0\n\tmov x8, #93\n\tsvc #0\n\
    \t.section .data.rel.ro,\"aw\",%progbits\n\t.balign 8\n\t.quad _start\n\
    \t.data\n\t.globl dat\ndat:\t.quad 1\n\
    \t.comm ca,4,4\n\t.comm cb,800,32\n\t.comm cc,24,8\n\t.comm cb,800,64\n\t.comm cd,1,1\n\
    \t.section .note.GNU-stack,\"\",%progbits\n";

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
    scratch: Scratch,
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
        Some(Self { scratch, built, arch: AARCH64 })
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

    /// Range-extension thunks (`spec/a64.md` §9): two patches in an
    /// island and the redirects of their sites.
    fn thunks() -> Option<Self> {
        Self::asm_for(AARCH64, "incr-thunks", ASM_THUNKS, &[])
    }

    /// The x86-64 assembly program.
    fn asm_x86_64() -> Option<Self> {
        Self::asm_for(Arch::X86_64, "incr-asm-x86-64", ASM_X86_64, &[])
    }

    /// The x86-64 program with large sections and a large common.
    fn asm_x86_64_large() -> Option<Self> {
        Self::asm_for(Arch::X86_64, "incr-asm-x86-64-large", ASM_X86_64_LARGE, &[])
    }

    /// Commons and a RELRO run (the allocation records).
    fn commons() -> Option<Self> {
        Self::asm_for(AARCH64, "incr-commons", ASM_COMMONS, &[])
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
        Some(Self { scratch, built, arch })
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
        // The copy can be busy for a moment (`run_exec_with`).
        let o = run_exec_with(
            || {
                let mut c = Command::new("sh");
                c.arg("-c").arg("ulimit -c 0; exec \"$0\" \"$@\"").arg(&exe).args(&args);
                c.current_dir(&self.built.dir);
                c
            },
            b"",
            Duration::from_mins(1),
        );
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
// incremental-mode full link writes it beside the output), IN-30, IN-33
// (with slack: other bytes than the plain link's, the same with either
// spelling, equivalent: the program runs the same), IN-18a (the state
// round-trips)
#[test]
fn a_full_link_in_incremental_mode_lays_out_slack() {
    let progs = [Prog::asm(), Prog::c(), Prog::cxx(), Prog::asm_x86_64(), Prog::asm_x86_64_large()];
    for prog in progs.into_iter().flatten() {
        let err = prog.link("plain", &[]);
        assert!(!err.contains("incremental"), "{err}");
        assert!(!prog.state_of("plain").exists(), "a plain link wrote a state");
        let err = prog.link("full", &["--incremental=full"]);
        assert!(!err.contains("incremental"), "{err}");
        assert!(prog.bytes("plain") != prog.bytes("full"), "no slack laid out");
        assert_eq!(prog.run("full"), prog.run("plain"), "the program runs otherwise");
        let err = prog.link("auto", &["--incremental"]);
        assert!(!err.contains("incremental"), "a first link prints no reason: {err}");
        assert!(prog.bytes("full") == prog.bytes("auto"), "the outputs differ");
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
    line_with(&prog.link("o", &inc), "full (small-output)");
    // Without the report, a reason other than the first link's is a line.
    line_with(&prog.link("o", &["--incremental"]), "full (small-output)");
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
    let lines: Vec<&str> = rep.lines().filter(|l| l.starts_with("incremental:")).collect();
    assert_eq!(lines.len(), 2, "{rep}");
    assert!(lines[1].starts_with("incremental: full (small-output) changed="), "{rep}");
    // WP-063: the dry run's lines follow the second (docs/cli.md §3.13).
    assert_eq!(rep.lines().last(), Some("delta end"), "{rep}");
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

// spec: incr.md IN-12 (`--sort-common` and `--sort-section` link plain,
// with the warning naming the option; WP-111)
#[test]
fn sort_options_are_unsupported() {
    let Some(prog) = Prog::asm() else { return };
    for (out, opt) in [("sc", "--sort-common"), ("ss", "--sort-section=alignment")] {
        let o = prog.mortar_env(out, &["--incremental", opt], &[]);
        let err = o.stderr_text();
        assert!(o.success(), "{err}");
        let name = opt.split('=').next().unwrap_or(opt);
        assert!(err.contains(&format!("--incremental is not supported with {name}")), "{err}");
        assert!(!prog.state_of(out).exists(), "{opt}: no state");
        let o = prog.mortar_env(&format!("{out}2"), &["--incremental=update", opt], &[]);
        assert!(!o.success() && o.stderr_text().contains("incremental-unsupported"), "{opt}");
    }
}

// spec: incr.md IN-71b (WP-212 F-9; WP-215): `--start-lib` links plain
// with the warning naming it; `--incremental=update` is
// `incremental-unsupported`
#[test]
fn start_lib_is_unsupported() {
    let Some(prog) = Prog::asm() else { return };
    let obj = prog
        .built
        .args
        .iter()
        .find(|a| Path::new(a).extension().is_some_and(|e| e == "o"))
        .expect("an object among the link's inputs")
        .clone();
    let o = prog.mortar_env("sl", &["--incremental", "--start-lib", &obj, "--end-lib"], &[]);
    let err = o.stderr_text();
    assert!(o.success(), "{err}");
    assert!(err.contains("--incremental is not supported with --start-lib"), "{err}");
    assert!(!prog.state_of("sl").exists(), "no state");
    let o =
        prog.mortar_env("sl2", &["--incremental=update", "--start-lib", &obj, "--end-lib"], &[]);
    assert!(!o.success() && o.stderr_text().contains("incremental-unsupported"));
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

/// `PATCHES` kind of an erratum patch (`spec/incr.md` IN-18a); the
/// others are thunks.
const PATCH_ERRATUM: u8 = 2;

/// Two thunk rows of `PATCHES` in one island, adjacent, of one size (the
/// first's size the distance to the second, the second's to the end of
/// its chunk): their rows.
fn same_size_thunks(s: &State) -> Option<(usize, usize)> {
    (1..s.patches.len()).find_map(|b| {
        let a = b - 1;
        let (pa, pb) = (&s.patches[a], &s.patches[b]);
        let end = s.chunks.get(pb.chunk as usize)?.size;
        (pa.kind != PATCH_ERRATUM
            && pb.kind != PATCH_ERRATUM
            && pa.chunk == pb.chunk
            && s.patches.get(b + 1).is_none_or(|p| p.chunk != pb.chunk)
            && pb.offset - pa.offset == end - pb.offset)
            .then_some((a, b))
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
        // A thunk dropped with its redirects: its sites are out of reach.
        ("PATCHES (drop a thunk)", Invalid, |s| {
            let Some(at) = s.patches.iter().position(|p| p.kind != PATCH_ERRATUM) else {
                return false;
            };
            s.patches.remove(at);
            let at = at as u32;
            s.redirects.retain(|r| r.patch != at);
            for r in &mut s.redirects {
                if r.patch > at {
                    r.patch -= 1;
                }
            }
            true
        }),
        // Two thunks of one island, one after the other and of one size,
        // trade places, and their sites follow: a placement the rules
        // admit (L-110, L-115).
        ("PATCHES (swap thunks)", Valid, |s| {
            let Some((a, b)) = same_size_thunks(s) else { return false };
            let (pa, pb) = (s.patches[a].clone(), s.patches[b].clone());
            (s.patches[a].symbol, s.patches[a].section, s.patches[a].value) =
                (pb.symbol, pb.section, pb.value);
            s.patches[a].kind = pb.kind;
            (s.patches[b].symbol, s.patches[b].section, s.patches[b].value) =
                (pa.symbol, pa.section, pa.value);
            s.patches[b].kind = pa.kind;
            let (a, b) = (a as u32, b as u32);
            for r in &mut s.redirects {
                if r.patch == a {
                    r.patch = b;
                } else if r.patch == b {
                    r.patch = a;
                }
            }
            true
        }),
        ("REDIRECTS", Invalid, |s| first(&mut s.redirects, |r| r.offset += 4)),
        // The WP-061 review's F8: an erratum site's redirect dropped (or,
        // with thunks, a site that is out of reach left without one).
        ("REDIRECTS (drop)", Invalid, |s| s.redirects.pop().is_some()),
        // A site sent to the other thunk: it would branch to another
        // function.
        ("REDIRECTS (other thunk)", Invalid, |s| {
            let thunks: Vec<u32> = (0..s.patches.len() as u32)
                .filter(|&i| s.patches[i as usize].kind != PATCH_ERRATUM)
                .collect();
            thunks.len() >= 2
                && s.redirects
                    .iter_mut()
                    .find(|r| thunks.contains(&r.patch))
                    .map(|r| r.patch = *thunks.iter().find(|&&t| t != r.patch).unwrap())
                    .is_some()
        }),
        ("SIZED", Invalid, |s| first(&mut s.sized, |z| z.size += 8)),
        ("LAYOUT", Invalid, |s| {
            s.layout.entry += 4;
            true
        }),
        // A chunk's capacity reaching into the next chunk in the file
        // (IN-33, L-141: capacity is reserved, never shared).
        ("CHUNKS (capacity into the next)", Invalid, |s| {
            let progbits = |c: &format::ChunkRow| c.sh_type == 1 && c.size > 0;
            for w in s.order.windows(2) {
                let (a, b) = (&s.chunks[w[0] as usize], &s.chunks[w[1] as usize]);
                if progbits(a) && progbits(b) && b.offset > a.offset {
                    let to = b.offset - a.offset + 1;
                    s.chunks[w[0] as usize].capacity = to;
                    return true;
                }
            }
            false
        }),
        // A capacity below the chunk's size (L-141). A capacity given
        // back (down to the size) is not here: a first link's bytes do
        // not show it (code gaps are trap fill either way, I-41; data
        // gaps zero), only an update's room does (WP-064).
        ("CHUNKS (capacity below the size)", Invalid, |s| {
            s.chunks.iter_mut().find(|c| c.size > 0).map(|c| c.capacity = c.size - 1).is_some()
        }),
        // An `.eh_frame` range's padding CIE left out (IN-34a): the
        // records after it move, the chunk's size does not.
        ("HOLES (drop an .eh_frame padding)", Invalid, |s| {
            let Some(at) = s.holes.iter().position(|h| h.kind == format::hole_kind::EH_PAD) else {
                return false;
            };
            s.holes.remove(at);
            true
        }),
        // 4 bytes of a padding CIE absorbed by the record before it and
        // the rest a shorter padding CIE (IN-34a): the same records at the
        // same places.
        ("HOLES (absorb 4 of an .eh_frame padding)", Valid, |s| {
            let Some(at) = s
                .holes
                .iter()
                .position(|h| h.kind == format::hole_kind::EH_PAD && h.size >= 20 && h.offset > 0)
            else {
                return false;
            };
            let h = s.holes[at];
            s.holes[at].size = 4;
            s.holes.insert(at + 1, format::HoleRow { offset: h.offset + 4, size: h.size - 4, ..h });
            true
        }),
        // A hole over a placed section (IN-3: a hole is what no live unit
        // owns).
        ("HOLES (over a placement)", Invalid, |s| {
            let Some(x) = s.sections.iter().find(|x| x.place_chunk != format::NONE && x.size > 0)
            else {
                return false;
            };
            let h = format::HoleRow {
                chunk: x.place_chunk,
                kind: format::hole_kind::FILL,
                offset: x.place_offset,
                size: 1,
            };
            s.holes.push(h);
            s.holes.sort_by_key(|h| (h.chunk, h.offset));
            true
        }),
        // A hole past its chunk's size (L-141: the tail is capacity, not
        // a hole).
        ("HOLES (past the size)", Invalid, |s| {
            let Some(h) = s.holes.first().copied() else { return false };
            let size = s.chunks[h.chunk as usize].size;
            s.holes[0].size = size - h.offset + 1;
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
    let progs = [
        Prog::asm(),
        Prog::c(),
        Prog::cxx(),
        Prog::asm_x86_64(),
        Prog::asm_x86_64_large(),
        Prog::erratum(),
        Prog::thunks(),
    ];
    for prog in progs.into_iter().flatten() {
        let dir = prog.built.dir.display().to_string();
        prog.link("plain", &[]);
        prog.link("full", &["--incremental=full"]);
        let (_, state) = prog.read_state("full");
        // The replay reproduces the incremental-mode link (with its
        // slack and holes, IN-30), not the plain one.
        let full = prog.bytes("full");
        match replay_judged(&prog, "replayed", &state) {
            Replayed::Linked(same, f) => {
                assert!(same == full, "{dir}: the replay differs from the link");
                assert!(f.is_empty(), "{dir}: the verifier rejects the link: {f:?}");
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
                Replayed::Linked(bytes, _) if bytes == full => {
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
            // The thunk program's outputs are 128 MiB each.
            for f in [out.clone(), format!("{out}.map"), "runme".to_owned()] {
                let _ = std::fs::remove_file(prog.path(&f));
            }
        }
        eprintln!("{dir}: mutated {applied:?}");
        for need in ["CHUNKS", "ORDER", "SEGMENTS", "SECTIONS", "SIZES", "LAYOUT"] {
            assert!(applied.contains(&need), "{need} was empty");
        }
    }
}

/// The `hole` and `dead` records of a map: `(record, section, offset,
/// size, dead kind)`.
fn hole_records(map: &str) -> Vec<(String, String, u64, u64, String)> {
    map.lines()
        .filter(|l| l.starts_with("hole ") || l.starts_with("dead "))
        .map(|l| {
            let t: Vec<&str> = l.split(' ').collect();
            let hex = |x: &str| u64::from_str_radix(x, 16).unwrap();
            let kind = t.get(4).map_or(String::new(), |k| (*k).to_owned());
            (t[0].to_owned(), t[1].to_owned(), hex(t[2]), hex(t[3]), kind)
        })
        .collect()
}

/// The start of the first `.eh_frame` record of the second file with
/// records, from the chunk's start: a range boundary (IN-34a).
fn eh_boundary(map: &str, eh_addr: u64, eh_end: u64) -> Option<u64> {
    let mut pieces: Vec<(u64, u32)> = map
        .lines()
        .filter(|l| l.starts_with("piece "))
        .filter_map(|l| {
            let t: Vec<&str> = l.split(' ').collect();
            let addr = u64::from_str_radix(t[4], 16).ok()?;
            let file: u32 = t[1].parse().ok()?;
            (eh_addr..eh_end).contains(&addr).then_some((addr, file))
        })
        .collect();
    pieces.sort_unstable();
    let first = pieces.first()?.1;
    pieces.iter().find(|p| p.1 != first).map(|p| p.0 - eh_addr)
}

/// `state` with an `.eh_frame` hole of `size` bytes and `kind` at
/// `offset`: the chunk, its slot size and its segment grow by `size`
/// into the `PT_LOAD` reserve after it (IN-33; `.eh_frame` is the last
/// chunk of its segment here, L-141).
fn with_eh_hole(state: &State, eh: usize, offset: u64, size: u64, kind: u8) -> State {
    let mut s = state.clone();
    let c = &mut s.chunks[eh];
    let (addr, end) = (c.addr, c.addr + c.size);
    c.size += size;
    c.capacity = c.capacity.max(c.size);
    for z in s.sizes.iter_mut().filter(|z| z.chunk == eh as u32) {
        z.size += size;
    }
    for g in
        s.segments.iter_mut().filter(|g| g.p_type == 1 && g.addr <= addr && end <= g.addr + g.memsz)
    {
        g.filesz += size;
        g.memsz += size;
    }
    s.holes.push(format::HoleRow { chunk: eh as u32, kind, offset, size });
    s.holes.sort_by_key(|h| (h.chunk, h.offset));
    s
}

// spec: incr.md IN-34a (padding CIEs of 16 bytes or more; 4, 8 or 12 bytes
// absorbed by the record before as DW_CFA_nop; the terminator last),
// IN-3a and plumb.md §3.2 (the `dead` records: a first link writes none;
// a dead range's bytes are the hole's), IN-61 (through the replay, with
// hand-made HOLES rows: a first link has no `.eh_frame` hole, IN-33
// gives the packed `.eh_frame` no per-range slack)
#[test]
fn padding_cies_and_dead_ranges_through_the_replay() {
    let scratch = Scratch::new("incr-holes-g");
    let Some(built) = build(&scratch, Driver::Gxx, &["-O1", "-g"], support::CXX_PROGRAM) else {
        assert!(!tools_required(), "{} is required", Driver::Gxx.tool());
        return;
    };
    let prog = Prog { scratch, built, arch: AARCH64 };
    prog.link("plain", &["--mortar-map=plain.map"]);
    prog.link("full", &["--incremental=full", "--mortar-map=full.map"]);
    let expected = prog.run("plain");
    assert_eq!(prog.run("full"), expected);
    let map = std::fs::read_to_string(prog.path("full.map")).unwrap();
    let first = hole_records(&map);
    assert_eq!(map.lines().filter(|l| *l == "incremental").count(), 1, "no incremental record");
    let plain_map = std::fs::read_to_string(prog.path("plain.map")).unwrap();
    assert!(!plain_map.lines().any(|l| l == "incremental"), "a plain map claims coverage");
    // Allocated slack is `hole`; a debug section's neutral room is
    // `dead … neutral` (plumb.md §3.2, §6.8), the only dead kind of a
    // first link.
    for h in &first {
        let debug = h.1.starts_with(".debug_");
        assert!(if debug { h.0 == "dead" && h.4 == "neutral" } else { h.0 == "hole" }, "{h:?}");
    }
    for sec in [".text", ".data", ".debug_info", ".debug_abbrev", ".debug_line"] {
        assert!(first.iter().any(|h| h.1 == sec), "no hole or neutral range in {sec}");
    }
    assert!(!first.iter().any(|h| h.1 == ".eh_frame"), "a first link's .eh_frame has no hole");
    support::plumb::readers(&prog.path("full"));
    let (_, state) = prog.read_state("full");
    let name = |c: &format::ChunkRow| state.str(c.name).to_vec();
    let eh = state.chunks.iter().position(|c| name(c) == b".eh_frame").expect("an .eh_frame chunk");
    let (eh_addr, eh_size) = (state.chunks[eh].addr, state.chunks[eh].size);
    let at = eh_boundary(&map, eh_addr, eh_addr + eh_size).expect("two files' records");
    let full = prog.bytes("full");
    // Padding CIEs, and paddings the record before absorbs.
    for size in [16u64, 32, 4, 8, 12] {
        let s = with_eh_hole(&state, eh, at, size, format::hole_kind::EH_PAD);
        let out = format!("pad{size}");
        match replay_judged(&prog, &out, &s) {
            Replayed::Linked(bytes, f) => {
                assert!(bytes != full, "{size}: the replay ignored the hole");
                assert!(f.is_empty(), "{size}: the verifier rejects: {f:?}");
            }
            Replayed::Failed(r) => panic!("{size}: the replay failed: {r}"),
        }
        support::plumb::readers(&prog.path(&out));
        assert_eq!(prog.run(&out), expected, "{size}: the program throws otherwise");
        let m = std::fs::read_to_string(prog.path(&format!("{out}.map"))).unwrap();
        // A padding CIE is a hole; an absorbed pad is no line (the
        // record before it is its piece, P-I1).
        let want = ("hole".to_owned(), ".eh_frame".to_owned(), at, size, String::new());
        let listed = hole_records(&m).contains(&want);
        assert_eq!(listed, size >= 16, "{size}: the map's .eh_frame hole");
        let bytes = prog.bytes(&out);
        let off = (state.chunks[eh].offset + at) as usize;
        let rec = &bytes[off..][..size as usize];
        if size >= 16 {
            // IN-34a's padding CIE: length, id 0, version 1, "", 1, -4,
            // the AArch64 return-address register, then nops.
            let mut want = ((size - 4) as u32).to_le_bytes().to_vec();
            want.extend_from_slice(&[0, 0, 0, 0, 1, 0, 1, 0x7c, 30]);
            want.resize(size as usize, 0);
            assert_eq!(rec, &want[..], "{size}: the padding CIE");
        } else {
            assert!(rec.iter().all(|&b| b == 0), "{size}: the absorbed bytes are DW_CFA_nop");
        }
    }
    // The dead writer: dead ranges hold their hole's bytes and the map
    // names them by kind.
    let mut s = with_eh_hole(&state, eh, at, 16, format::hole_kind::DEAD_CIE);
    let debug =
        s.holes.iter().position(|h| h.kind == format::hole_kind::DEBUG).expect("a debug allotment");
    s.holes[debug].kind = format::hole_kind::DEAD_DEBUG;
    let dead_debug = s.holes[debug];
    match replay_judged(&prog, "dead", &s) {
        Replayed::Linked(bytes, f) => {
            assert!(f.is_empty(), "the verifier rejects: {f:?}");
            assert!(bytes == prog.bytes("pad16"), "a dead range's bytes are not its hole's");
        }
        Replayed::Failed(r) => panic!("the replay failed: {r}"),
    }
    let m = std::fs::read_to_string(prog.path("dead.map")).unwrap();
    let records = hole_records(&m);
    // The dead CIE replaces pad16's hole line; a dead debug range is
    // neutral, as the allotment it replaces was.
    let cie = ("dead".to_owned(), ".eh_frame".to_owned(), at, 16, "cie".to_owned());
    let mut want = hole_records(&std::fs::read_to_string(prog.path("pad16.map")).unwrap());
    let hole = want.iter().position(|h| h.0 == "hole" && h.1 == ".eh_frame").expect("pad16's");
    want[hole] = cie;
    assert_eq!(records, want, "the dead records");
    assert!(
        records.iter().any(|h| h.0 == "dead" && h.4 == "neutral" && h.3 == dead_debug.size),
        "no neutral record of the dead debug range"
    );
    support::plumb::readers(&prog.path("dead"));
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
    let prog = Prog { scratch, built, arch: AARCH64 };
    prog.link("full", &["--incremental=full"]);
    let (_, state) = prog.read_state("full");
    let kept = |s: &State| s.eh_records.iter().filter(|r| r.piece == 0).count();
    assert_eq!(kept(&state), 2, "two CIEs that differ are both kept");
    // b.o again, its CIE now equal to a.o's: the same size.
    let equal = gcc_c(&prog.scratch, "b30.s", &b(30), "b.o");
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

// spec: incr.md IN-18a (one format version 2: a state of version 1 is
// refused by name, `state-version`, and the link is a full one); the
// fixtures' README says how the version-1 states were made.
#[test]
fn a_version_1_state_is_refused_by_name() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/incr-state-v1-51aa582");
    for (dir, objects) in [("a64", &["a.o", "b.o"][..]), ("x86_64", &["a.o"][..])] {
        let scratch = Scratch::new(&format!("incr-v1-{dir}"));
        for f in objects.iter().chain(&["out", "out.mortar-state"]) {
            std::fs::copy(root.join(dir).join(f), scratch.join(f)).unwrap();
        }
        let bytes = std::fs::read(scratch.join("out.mortar-state")).unwrap();
        let e = format::decode(&bytes, mortar::VERSION.as_bytes())
            .expect_err("a version-1 state decodes");
        assert_eq!(e.reason(), "state-version", "{dir}: {}", e.detail());
        assert!(e.detail().starts_with("format version 1,"), "{dir}: {}", e.detail());
        let args: Vec<OsString> = objects
            .iter()
            .map(|o| (*o).into())
            .chain(
                ["-static", "--eh-frame-hdr", "--incremental", "--incremental-report", "-o", "out"]
                    .map(OsString::from),
            )
            .collect();
        let o = run_in(scratch.path(), Path::new(MORTAR), &args, &[]);
        assert!(o.success(), "{dir}: {}", o.stderr_text());
        line_with(&o.stderr_text(), "full (state-version: format version 1,");
        let bytes = std::fs::read(scratch.join("out.mortar-state")).unwrap();
        assert!(format::decode(&bytes, mortar::VERSION.as_bytes()).is_ok(), "{dir}: not rewritten");
    }
}

// ---- stamps ----------------------------------------------------------------

/// The directories to try the stamp rule on: the build directory's
/// scratch (the workspace's file system), then `MORTAR_INCR_STAMP_DIR`
/// when set, the system's temporary directory and `/dev/shm`, each when
/// it is writable and on a file system not already in the list (here an
/// overlay `/tmp` and a tmpfs; on the hosted runners whatever they have).
/// Held by every test that takes stamps in [`stamp_dirs`]: the stamp file
/// is `<dir>/.mortar-stamp-<pid>` (IN-22), one name per process and
/// directory, and the system temporary directory and `/dev/shm` are the
/// same directories for every test of this process. Two tests there at
/// once create and remove one file between them, and the second removal
/// is `NotFound` (seen once in a gate under load). Since the stamps of one
/// process are taken one at a time (`stamp::take`, `stamp::probe`: the
/// in-process links of this file's other tests stamp the temporary
/// directory too, which this lock never covered), the lock only keeps the
/// three timing tests from disturbing each other's measurements.
static STAMP_DIRS: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn stamp_lock() -> std::sync::MutexGuard<'static, ()> {
    STAMP_DIRS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

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
    let _stamps = stamp_lock();
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
    let _stamps = stamp_lock();
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
    let _stamps = stamp_lock();
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
    line_with(&link(&x64), "full (small-output)");
    // WP-106: no update could start from a state of another target, so
    // the report names the reason and computes no delta.
    let err = link(&a64);
    line_with(&err, "full (options-changed)");
    assert!(!err.lines().any(|l| l.starts_with("delta ")), "a dry run: {err}");
    let err = link(&a64);
    line_with(&err, "full (small-output)");
    assert!(err.lines().any(|l| l == "delta end"), "no dry run: {err}");
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

// spec: incr.md IN-12 (`-static-pie` is not a static executable: a
// warning naming it, a plain link with no state, the same bytes as the
// plain link; `--incremental=update` the error), WP-052, both
// architectures. AArch64's assembly keeps its address word in a writable
// section (in `.rodata` a static-pie refuses it, D-47).
#[test]
fn static_pie_is_unsupported() {
    let pic = ASM.replace(
        "\t.section .rodata\n\t.quad msg\n",
        "\t.section .data.rel.ro,\"aw\"\n\t.quad msg\n",
    );
    assert_ne!(pic, ASM);
    for prog in [
        Prog::asm_for(AARCH64, "incr-spie", &pic, &["-pie", "--no-dynamic-linker"]),
        Prog::asm_for(
            Arch::X86_64,
            "incr-spie-x86-64",
            ASM_X86_64,
            &["-pie", "--no-dynamic-linker"],
        ),
    ]
    .into_iter()
    .flatten()
    {
        prog.link("plain", &[]);
        assert!(!prog.state_of("plain").exists());
        for mode in ["--incremental", "--incremental=full"] {
            let err = prog.link("spie", &[mode]);
            assert!(
                err.contains("--incremental is not supported with -static-pie"),
                "{mode}: {err}"
            );
            assert!(err.contains("linking without a state"), "{mode}: {err}");
            assert!(!prog.state_of("spie").exists(), "{mode}: a state was written");
            assert_eq!(
                prog.bytes("spie"),
                prog.bytes("plain"),
                "{mode}: not the plain link's bytes"
            );
        }
        let o = prog.mortar_env("upd", &["--incremental=update"], &[]);
        let err = o.stderr_text();
        assert!(!o.success() && err.contains("incremental-unsupported (-static-pie)"), "{err}");
        assert!(!prog.state_of("upd").exists());
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
    assert!(!o.success() && err.contains("names the input"), "{err}");
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
        line_with(&err, "full (small-output)");
    }
}

// spec: incr.md §1.5 ("Go/no-go rule": the measured floor, WP-061 stage
// B), §5.2 (the update phase by phase), IN-27 (a same-size body edit
// moves no definition), IN-46 / IN-47 (the two write paths)
//
// The C program's `--incremental=full` link, then one function body of
// its object edited in place (`impl_a`'s `mov w0, #1` becomes `#2`: the
// same size). The test-only update path (`mortar::floor::update`) on
// each write path writes the bytes of an incremental-mode full link of the
// edited inputs (the layout with slack: WP-062), the verifier accepts them, the program prints the new value, and the
// rewritten state chains: a second edit updates again, and
// `--incremental=update` still fails by name.
#[test]
fn the_floor_update_is_the_full_link_of_the_edited_inputs() {
    use mortar::floor::{WritePath, update};
    for write in [WritePath::Copy, WritePath::InPlace] {
        let Some(prog) = Prog::c() else { return };
        let obj = prog.path("c1.c.o");
        let edit = |from: u32, to: u32| {
            let mut b = std::fs::read(&obj).unwrap();
            // `mov w0, #n; ret`: impl_a's body.
            let body: Vec<u8> = [from, 0xd65f_03c0].iter().flat_map(|w| w.to_le_bytes()).collect();
            let at: Vec<usize> = (0..b.len().saturating_sub(7))
                .step_by(4)
                .filter(|&i| b[i..i + 8] == body[..])
                .collect();
            assert_eq!(at.len(), 1, "one `mov w0, #{}; ret` in c1.c.o", (from >> 5) & 0xffff);
            b[at[0]..at[0] + 4].copy_from_slice(&to.to_le_bytes());
            // A new file (a new inode, as a compiler writes it).
            std::fs::write(prog.path("c1.c.o.new"), &b).unwrap();
            std::fs::rename(prog.path("c1.c.o.new"), &obj).unwrap();
        };
        prog.link("o", &["--incremental=full"]);
        let inode =
            || std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(prog.path("o")).unwrap());
        let before = inode();
        // The first output stays open to the end of the test: a file system
        // may give the number of a freed inode to the next file made, and
        // the second copy would then "keep" an inode that the first copy
        // had replaced (seen once on the hosted x86-64 lane).
        let _first = std::fs::File::open(prog.path("o")).unwrap();
        // mov w0, #1 -> #2 -> #3
        for (step, (from, to)) in
            [(0x5280_0020_u32, 0x5280_0040_u32), (0x5280_0040, 0x5280_0060)].into_iter().enumerate()
        {
            edit(from, to);
            let u = update(&prog.args("o", &["--incremental"]), &prog.built.dir, write);
            let s = u.outcome.unwrap_or_else(|why| {
                let r = u.report.map(|r| r.render(&mortar_diag::Style::plain("mortar")));
                panic!("{write:?} step {step}: {why}\n{}\n{}", r.unwrap_or_default(), u.phases)
            });
            assert_eq!(s.changed, obj.display().to_string(), "{write:?}: the changed file");
            assert!(s.windows >= 1 && s.jobs >= 1);
            let map = format!("ref{step}.map");
            let map_arg = format!("--mortar-map={}", prog.path(&map).display());
            // The reference is an incremental-mode full link of the edited
            // inputs: its layout has the slack and the per-file CIEs the
            // state's has (WP-062); a plain link's has neither.
            prog.link(&format!("ref{step}"), &["--incremental=full", &map_arg]);
            assert!(
                prog.bytes("o") == prog.bytes(&format!("ref{step}")),
                "{write:?} step {step}: the update differs from the full link"
            );
            let findings =
                support::plumb::verify(&prog.path("o"), &prog.path(&map), &prog.built.dir);
            assert!(findings.is_empty(), "{write:?}: the verifier rejects: {findings:?}");
            let (status, out) = prog.run("o");
            assert_eq!(status, Some(0));
            let value = (to >> 5) & 0xffff;
            assert!(
                String::from_utf8_lossy(&out).contains(&format!(" {value} ")),
                "{write:?}: {}",
                String::from_utf8_lossy(&out)
            );
            let (_, state) = prog.read_state("o");
            assert_eq!(state.flags & FLAG_UPDATING, 0, "the flag is cleared");
        }
        match write {
            WritePath::InPlace => assert_eq!(inode(), before, "in place keeps the inode"),
            WritePath::Copy => assert_ne!(inode(), before, "the copy renames a new file"),
        }
        let o = prog.mortar_env("o", &["--incremental=update"], &[]);
        assert!(!o.success());
        assert!(
            o.stderr_text().contains("cannot update: update-not-implemented"),
            "{}",
            o.stderr_text()
        );
    }
}

// spec: plumb.md §3.2 (the allocation records, WP-096) and image.md
// I-78: a plain map and an incremental-mode map both state the NOBITS
// `.pad.relro` as a chunk and each common with the alignment it was
// placed at, and the verifier, which checks each (M6, M7), accepts both.
#[test]
fn plain_and_incremental_maps_state_the_allocations() {
    let progs = [Prog::commons(), Prog::asm_x86_64_large()];
    for (k, prog) in progs.into_iter().enumerate() {
        let Some(prog) = prog else { continue };
        prog.link("plain", &["--mortar-map=plain.map"]);
        prog.link("full", &["--incremental=full", "--mortar-map=full.map"]);
        for out in ["plain", "full"] {
            let map = std::fs::read_to_string(prog.path(&format!("{out}.map"))).unwrap();
            assert!(map.starts_with("mortar-map 2\n"), "{out}");
            let commons: Vec<(String, u64)> = map
                .lines()
                .filter_map(|l| {
                    let t: Vec<&str> = l.split(' ').collect();
                    (t[0] == "common").then(|| {
                        assert_eq!(t.len(), 5, "{l}");
                        (t[1].to_owned(), u64::from_str_radix(t[4], 16).unwrap())
                    })
                })
                .collect();
            let want: &[(&str, u64)] = if k == 0 {
                &[("ca", 4), ("cb", 0x40), ("cc", 8), ("cd", 1)]
            } else {
                &[("lc", 0x20)]
            };
            let mut got = commons.clone();
            got.sort();
            let want: Vec<(String, u64)> =
                want.iter().map(|(n, a)| ((*n).to_owned(), *a)).collect();
            assert_eq!(got, want, "{out}: the common records");
            assert!(!map.lines().any(|l| l.starts_with("chunk common ")), "{out}: a chunk common");
            if k == 0 {
                let pad: Vec<&str> =
                    map.lines().filter(|l| l.starts_with("chunk synthetic .pad.relro ")).collect();
                assert_eq!(pad.len(), 1, "{out}: the .pad.relro chunk: {pad:?}");
            }
            let dir = prog.built.dir.clone();
            support::plumb::clean(&prog.path(out), &prog.path(&format!("{out}.map")), &dir);
        }
    }
}

/// The members of the archive tests (WP-106): `m1` (`f1`, edited as
/// `m1b`, the same size), `m2` (`f2`), both extracted by `main`; `m3`
/// and `m4` define names nothing references (never extracted); `m5`
/// defines `f2` too (it replaces `m2`).
const THIN_MAIN: &str =
    "\t.text\n\t.globl _start\n_start:\n\tbl f1\n\tbl f2\n\tmov x8, #93\n\tsvc #0\n";
const THIN_MEMBERS: [(&str, &str); 6] = [
    ("m1", "\t.text\n\t.globl f1\nf1:\n\tmov x0, #1\n\tret\n"),
    ("m1b", "\t.text\n\t.globl f1\nf1:\n\tmov x0, #3\n\tret\n"),
    ("m2", "\t.text\n\t.globl f2\nf2:\n\tret\n"),
    ("m3", "\t.text\n\t.globl u3\nu3:\n\tret\n"),
    ("m4", "\t.text\n\t.globl u4\nu4:\n\tret\n"),
    ("m5", "\t.text\n\t.globl f2\nf2:\n\tnop\n\tret\n"),
];

/// A directory with `main.o`, `lib/m*.o` (and `lib/m1b.o`), and the
/// archive tools.
struct Members {
    scratch: Scratch,
    ar: PathBuf,
}

impl Members {
    fn new(label: &str) -> Option<Self> {
        let (Some(gcc), Some(ar)) = (AARCH64.tool("gcc"), AARCH64.tool("ar")) else {
            assert!(
                !tools_required(),
                "{} and {} are required",
                AARCH64.gnu("gcc"),
                AARCH64.gnu("ar")
            );
            return None;
        };
        let scratch = Scratch::new(label);
        std::fs::create_dir_all(scratch.join("lib")).unwrap();
        std::fs::create_dir_all(scratch.join("run")).unwrap();
        let sources = std::iter::once(("main", THIN_MAIN)).chain(THIN_MEMBERS.iter().copied());
        for (name, text) in sources {
            let src = scratch.write(&format!("{name}.s"), text);
            let obj = if name == "main" {
                scratch.join("main.o")
            } else {
                scratch.join(format!("lib/{name}.o"))
            };
            let o = run_in(
                scratch.path(),
                &gcc,
                &["-c".into(), src.into(), "-o".into(), obj.into()],
                &[],
            );
            assert!(o.success(), "gcc: {}", o.stderr_text());
        }
        std::fs::copy(scratch.join("lib/m1.o"), scratch.join("lib/m1a.o")).unwrap();
        Some(Self { scratch, ar })
    }

    /// `lib/<name>` made again from `members` (thin with `T`).
    fn archive(&self, name: &str, flags: &str, members: &[&str]) {
        let path = self.scratch.join(format!("lib/{name}"));
        if path.exists() {
            std::fs::remove_file(&path).unwrap();
        }
        let mut a: Vec<OsString> = vec![flags.into(), name.into()];
        a.extend(members.iter().map(|m| OsString::from(format!("{m}.o"))));
        let o = run_in(&self.scratch.join("lib"), &self.ar, &a, &[]);
        assert!(o.success(), "ar: {}", o.stderr_text());
    }

    /// `lib/m1.o` rewritten in place with the other version's bytes
    /// (`m1b`'s, or `m1`'s again: the same size); with `keep_times`, its
    /// mtime set back to what it was.
    fn edit_m1(&self, keep_times: bool) {
        let path = self.scratch.join("lib/m1.o");
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        let now = std::fs::read(&path).unwrap();
        let [a, b] =
            ["lib/m1a.o", "lib/m1b.o"].map(|p| std::fs::read(self.scratch.join(p)).unwrap());
        let bytes = if now == b { a } else { b };
        assert_eq!(bytes.len(), now.len(), "the two versions have one size");
        let f = std::fs::File::options().write(true).open(&path).unwrap();
        f.write_all_at(&bytes, 0).unwrap();
        if keep_times {
            f.set_modified(before).unwrap();
        }
        drop(f);
    }

    /// mortar from `dir` (relative to the scratch) with `args`, linking
    /// `out` with `--incremental` and the report to `rep.txt`; the report.
    fn link(&self, dir: &str, args: &[&str]) -> String {
        let cwd = self.scratch.join(dir);
        let rep = cwd.join("rep.txt");
        if rep.exists() {
            std::fs::remove_file(&rep).unwrap();
        }
        let mut a: Vec<OsString> = args.iter().map(OsString::from).collect();
        a.extend(
            ["-static", "-o", "out", "--incremental", "--incremental-report=rep.txt"]
                .map(OsString::from),
        );
        let o = run_in(&cwd, Path::new(MORTAR), &a, &[]);
        assert!(o.success(), "mortar {args:?}: {}", o.stderr_text());
        std::fs::read_to_string(rep).unwrap()
    }
}

// spec: incr.md IN-24 (a thin archive's members are files, stat'ed as
// such; a changed member is a changed file, not a change of plan), §7
// (`options-changed` is not raised by a member's edit), WP-094's finding
// (the dry run named a changed thin member `options-changed`, "is not an
// object or archive"). The neighbours: a member removed, added, replaced
// with its size and mtime kept, the archive in a group, the link run from
// another directory with relative paths, and a regular archive (the
// control).
#[test]
fn a_changed_thin_member_is_a_changed_file() {
    let Some(m) = Members::new("incr-thin") else { return };
    let no_fallback = |rep: &str, what: &str| {
        assert!(rep.lines().last() == Some("delta end"), "{what}: no dry run:\n{rep}");
        let falls: Vec<&str> = rep.lines().filter(|l| l.starts_with("delta fallback ")).collect();
        assert!(falls.is_empty(), "{what}: {falls:?}\n{rep}");
    };
    let changed = |rep: &str, archive: &str, what: &str| {
        let want = format!("delta file changed {archive} m1.o");
        assert!(
            rep.lines().any(|l| l.ends_with(&want) || l == want),
            "{what}: no `{want}`:\n{rep}"
        );
    };
    for (flags, name) in [("rcsT", "libt.a"), ("rcs", "libr.a")] {
        let lib = format!("lib/{name}");
        let thin = flags.ends_with('T');
        // The member edited (the same size, a new mtime).
        m.archive(name, flags, &["m1", "m2", "m3"]);
        m.link(".", &["main.o", &lib]);
        m.edit_m1(false);
        if !thin {
            m.archive(name, flags, &["m1", "m2", "m3"]);
        }
        let rep = m.link(".", &["main.o", &lib]);
        no_fallback(&rep, &format!("{name}: m1 edited"));
        changed(&rep, &lib, &format!("{name}: m1 edited"));
        // An unextracted member removed, then added back, then another.
        m.archive(name, flags, &["m1", "m2"]);
        let rep = m.link(".", &["main.o", &lib]);
        no_fallback(&rep, &format!("{name}: m3 removed"));
        m.archive(name, flags, &["m1", "m2", "m3", "m4"]);
        let rep = m.link(".", &["main.o", &lib]);
        no_fallback(&rep, &format!("{name}: m3, m4 added"));
        // An extracted member removed, another that defines its name
        // added: the old member file left, the new one is read.
        m.archive(name, flags, &["m1", "m5", "m3", "m4"]);
        let rep = m.link(".", &["main.o", &lib]);
        no_fallback(&rep, &format!("{name}: m2 replaced by m5"));
        for want in
            [format!("delta file appeared {lib} m5.o"), format!("delta file gone {lib} m2.o")]
        {
            assert!(rep.lines().any(|l| l == want), "{name}: no `{want}`:\n{rep}");
        }
        m.archive(name, flags, &["m1", "m2", "m3", "m4"]);
        // In a group.
        let group = ["main.o", "--start-group", lib.as_str(), "--end-group"];
        m.link(".", &group);
        m.edit_m1(true);
        if !thin {
            m.archive(name, flags, &["m1", "m2", "m3", "m4"]);
        }
        let rep = m.link(".", &group);
        no_fallback(&rep, &format!("{name}: in a group, m1 edited, its mtime kept"));
        changed(&rep, &lib, &format!("{name}: in a group, m1 edited, its mtime kept"));
        // From another directory, by relative paths.
        let rel = format!("../{lib}");
        m.link("run", &["../main.o", &rel]);
        m.edit_m1(false);
        if !thin {
            m.archive(name, flags, &["m1", "m2", "m3", "m4"]);
        }
        let rep = m.link("run", &["../main.o", &rel]);
        no_fallback(&rep, &format!("{name}: from run/, m1 edited"));
        changed(&rep, &rel, &format!("{name}: from run/, m1 edited"));
    }
}

// spec: incr.md §7 (`small-output`: a state of an output below
// `mortar_incr::SMALL_OUTPUT` is linked in full, by that name;
// `--incremental=update` is not bound by it), IN-10a (the dry run still
// runs for it when the report asks), WP-106
#[test]
fn a_small_output_is_linked_in_full_by_name() {
    let Some(prog) = Prog::asm() else { return };
    prog.link("o", &["--incremental"]);
    let size = std::fs::metadata(prog.path("o")).unwrap().len();
    assert!(size < mortar_incr::SMALL_OUTPUT && mortar_incr::small_output(size), "{size}");
    let err = prog.link("o", &["--incremental"]);
    assert_eq!(err.lines().filter(|l| l.starts_with("incremental:")).count(), 1, "{err}");
    line_with(&err, "full (small-output)");
    let err = prog.link("o", &["--incremental", "--incremental-report"]);
    line_with(&err, "full (small-output) changed=");
    assert_eq!(err.lines().last(), Some("delta end"), "{err}");
    let o = prog.mortar_env("o", &["--incremental=update"], &[]);
    assert!(!o.success());
    assert!(
        o.stderr_text().contains("cannot update: update-not-implemented"),
        "{}",
        o.stderr_text()
    );
}

// spec: incr.md IN-11 (a plain link never opens, removes or writes a
// state or a report; `--incremental-report` and `--incremental-state`
// without `--incremental` do nothing), IN-10a, WP-106. A FIFO at the
// state paths would block a plain link that opened one; an existing
// state keeps its bytes, inode and times; the directory gains only the
// output. (A `stat` alone is not observable here: no tracer is in
// `scripts/ci-tools.sh`.)
#[test]
fn a_plain_link_touches_nothing_of_incremental_mode() {
    let Some(prog) = Prog::asm() else { return };
    let Some(mkfifo) = mortar_testutil::tool("mkfifo") else {
        assert!(!tools_required(), "mkfifo is required");
        return;
    };
    prog.link("o", &["--incremental"]);
    let state = prog.state_of("o");
    let meta = |p: &Path| {
        use std::os::unix::fs::MetadataExt as _;
        let m = std::fs::symlink_metadata(p).unwrap();
        (m.ino(), m.len(), m.mtime(), m.mtime_nsec(), m.ctime(), m.ctime_nsec())
    };
    let before = (std::fs::read(&state).unwrap(), meta(&state));
    for fifo in ["p.mortar-state", "p.st"] {
        let o = run_in(&prog.built.dir, &mkfifo, &[fifo.into()], &[]);
        assert!(o.success(), "{}", o.stderr_text());
    }
    let listing = || {
        let mut v: Vec<OsString> =
            std::fs::read_dir(&prog.built.dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        v.sort();
        v
    };
    let entries = listing();
    std::thread::sleep(Duration::from_millis(20));
    for (out, extra) in [
        ("o", &[][..]),
        ("p", &["--incremental-report=rep.txt"][..]),
        ("p", &["--incremental-state=p.st", "--incremental-report"][..]),
        ("p", &["--incremental", "--no-incremental", "--incremental-report=rep.txt"][..]),
    ] {
        let err = prog.link(out, extra);
        assert!(!err.contains("incremental") && !err.contains("delta"), "{extra:?}: {err}");
    }
    assert_eq!((std::fs::read(&state).unwrap(), meta(&state)), before, "the state was touched");
    for fifo in ["p.mortar-state", "p.st"] {
        use std::os::unix::fs::FileTypeExt as _;
        let t = std::fs::symlink_metadata(prog.path(fifo)).unwrap().file_type();
        assert!(t.is_fifo(), "{fifo} replaced");
    }
    let mut want = entries;
    want.push("p".into());
    want.sort();
    assert_eq!(listing(), want, "a plain link left another file");
    // IN-11: the state no longer matches the output a plain link replaced.
    line_with(&prog.link("o", &["--incremental"]), "full (output-changed: ");
}

/// `n` functions, each global in its own section; `pad` instructions
/// ahead of the first, so that with `pad = 1` every function moves.
fn many_sections(n: usize, pad: usize) -> String {
    use std::fmt::Write as _;
    let mut s = String::from("\t.text\n\t.globl _start\n_start:\n");
    s.push_str(&"\tnop\n".repeat(pad));
    s.push_str("\tmov x8, #93\n\tsvc #0\n");
    for i in 0..n {
        let _ = write!(s, "\t.section .text.f{i},\"ax\",%progbits\n\t.globl f{i}\nf{i}:\n\tret\n");
    }
    s.push_str("\t.section .note.GNU-stack,\"\",%progbits\n");
    s
}

// spec: incr.md IN-10 (the report is bounded: per kind the first
// `REPORT_BOUND` lines and a `delta elided` line with the total, a
// `delta listed` line with the sum; nothing left out unsaid), docs/cli.md
// §3.13, WP-106. One object of 100,000 sections, each with its global,
// all moved by one instruction: 100,000 refilled units, holes and moved
// symbols. Two links of the same history give the same report, but for
// its two time fields.
#[test]
fn the_report_is_bounded() {
    use mortar_incr::delta::REPORT_BOUND;
    const N: usize = 100_000;
    let Some(gcc) = AARCH64.tool("gcc") else {
        assert!(!tools_required(), "{} is required", AARCH64.gnu("gcc"));
        return;
    };
    let scratch = Scratch::new("incr-bound");
    for v in 0..2 {
        let src = scratch.write(&format!("v{v}.s"), many_sections(N, v));
        let obj = scratch.join(format!("v{v}.o"));
        let o =
            run_in(scratch.path(), &gcc, &["-c".into(), src.into(), "-o".into(), obj.into()], &[]);
        assert!(o.success(), "gcc: {}", o.stderr_text());
    }
    let mut reports = Vec::new();
    for run in ["a", "b"] {
        let dir = scratch.join(run);
        std::fs::create_dir_all(&dir).unwrap();
        let link = |v: usize, report: bool| {
            std::fs::copy(scratch.join(format!("v{v}.o")), dir.join("in.o")).unwrap();
            let mut a: Vec<OsString> =
                ["in.o", "-static", "-o", "out", "--incremental"].map(OsString::from).to_vec();
            if report {
                a.push("--incremental-report=rep.txt".into());
            }
            let o = run_in(&dir, Path::new(MORTAR), &a, &[]);
            assert!(o.success(), "{}", o.stderr_text());
        };
        link(0, false);
        link(1, true);
        let rep = std::fs::read_to_string(dir.join("rep.txt")).unwrap();
        // The files of this run, before the next (about 40 MB).
        for f in ["out", "out.mortar-state", "in.o"] {
            std::fs::remove_file(dir.join(f)).unwrap();
        }
        reports.push(rep);
    }
    let rep = &reports[0];
    let count =
        |kind: &str| rep.lines().filter(|l| l.starts_with(&format!("delta {kind} "))).count();
    for kind in ["unit", "hole", "symbol"] {
        assert_eq!(count(kind), REPORT_BOUND, "{kind}");
        let elided = rep
            .lines()
            .find_map(|l| {
                l.strip_prefix(&format!("delta elided {kind} listed={REPORT_BOUND} total="))
            })
            .unwrap_or_else(|| panic!("no `delta elided {kind}` line"));
        assert!(elided.parse::<usize>().unwrap() >= N, "{kind}: {elided}");
    }
    let refill = rep.lines().find(|l| l.starts_with("delta refill ")).unwrap();
    let units: usize =
        refill.split(' ').find_map(|w| w.strip_prefix("units=")).unwrap().parse().unwrap();
    assert!(units >= N, "{refill}");
    let listed = rep.lines().find(|l| l.starts_with("delta listed ")).unwrap();
    let elided: usize = listed.rsplit_once("elided=").unwrap().1.parse().unwrap();
    let total: usize = rep
        .lines()
        .filter_map(|l| l.strip_prefix("delta elided "))
        .map(|l| l.rsplit_once("total=").unwrap().1.parse::<usize>().unwrap() - REPORT_BOUND)
        .sum();
    assert_eq!(elided, total, "{listed}");
    assert_eq!(rep.lines().last(), Some("delta end"));
    assert!(rep.len() < 4 << 20, "the report is {} bytes", rep.len());
    // Deterministic: the same history, the same report, but for the times.
    let mask = |r: &str| -> String {
        r.lines()
            .map(|l| {
                if l.starts_with("delta time ") {
                    "delta time".to_owned()
                } else {
                    l.split(' ').filter(|w| !w.starts_with("time=")).collect::<Vec<_>>().join(" ")
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(mask(&reports[0]) == mask(&reports[1]), "two reports of one history differ");
    eprintln!("the report: {} bytes, {} lines; {listed}", rep.len(), rep.lines().count());
}

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
use mortar_testutil::{Outcome, Scratch, run_with, tools_required};
use support::{Built, Driver, Source, build, with_output};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

/// A static executable from assembly alone: text, data, read-only data,
/// a merged string section, `.eh_frame` from CFI, and a GOT load.
const ASM: &str = "\t.text\n\t.globl _start\n\t.type _start, %function\n_start:\n\t.cfi_startproc\n\
    \tadrp x1, :got:word\n\tldr x1, [x1, :got_lo12:word]\n\tbl sfn\n\tmov x8, #93\n\tsvc #0\n\
    \t.cfi_endproc\n\t.type sfn, %function\nsfn:\n\t.cfi_startproc\n\tmov x0, #42\n\tret\n\
    \t.cfi_endproc\n\t.data\n\t.globl word\nword:\t.quad 1\n\
    \t.section .rodata.str1.1,\"aMS\",%progbits,1\nmsg:\t.asciz \"hello\"\nm2:\t.asciz \"lo\"\n\
    \t.section .rodata\n\t.quad msg\n\
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
    _scratch: Scratch,
    built: Built,
}

impl Prog {
    fn c() -> Option<Self> {
        Self::of("incr-c", Driver::Gcc, support::C_PROGRAM)
    }

    fn cxx() -> Option<Self> {
        Self::of("incr-cxx", Driver::Gxx, support::CXX_PROGRAM)
    }

    fn of(label: &str, driver: Driver, sources: &[Source]) -> Option<Self> {
        if std::env::consts::ARCH != "aarch64" {
            return None;
        }
        let scratch = Scratch::new(label);
        let Some(built) = build(&scratch, driver, &["-O1"], sources) else {
            assert!(!tools_required(), "{} is required", driver.tool());
            return None;
        };
        Some(Self { _scratch: scratch, built })
    }

    /// The assembly program: `gcc -c` and a bare link line.
    fn asm() -> Option<Self> {
        if std::env::consts::ARCH != "aarch64" {
            return None;
        }
        let Some(gcc) = mortar_testutil::tool("gcc") else {
            assert!(!tools_required(), "gcc is required");
            return None;
        };
        let scratch = Scratch::new("incr-asm");
        let src = scratch.write("a.s", ASM);
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
            args: vec![
                obj.display().to_string(),
                "-static".into(),
                "--eh-frame-hdr".into(),
                "-o".into(),
                "out".into(),
            ],
        };
        Some(Self { _scratch: scratch, built })
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
    for prog in [Prog::asm(), Prog::c(), Prog::cxx()].into_iter().flatten() {
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

/// The replay of `state` (IN-61) to `out`: `None` when the link failed.
fn replay(prog: &Prog, out: &str, state: &State) -> Option<Vec<u8>> {
    let inv = mortar::replay_link("mortar", &prog.args(out, &[]), &prog.built.dir, state);
    match inv.outcome {
        mortar::Outcome::Linked { .. } => Some(prog.bytes(out)),
        _ => None,
    }
}

/// One mutation per stored decision table: its name and whether it
/// applied (the table is not empty).
type Mutation = (&'static str, fn(&mut State) -> bool);

fn mutations() -> Vec<Mutation> {
    fn first<T>(v: &mut [T], f: impl FnOnce(&mut T)) -> bool {
        v.first_mut().map(f).is_some()
    }
    vec![
        ("CHUNKS", |s| {
            let Some(c) = s.chunks.iter_mut().find(|c| c.size > 0 && c.bits & 1 != 0) else {
                return false;
            };
            c.addr += 16;
            c.offset += 16;
            true
        }),
        ("ORDER", |s| {
            let n = s.order.len();
            n >= 3 && {
                s.order.swap(1, n - 1);
                true
            }
        }),
        ("SEGMENTS", |s| first(&mut s.segments, |g| g.addr += 0x1000)),
        ("SECTIONS", |s| {
            s.sections
                .iter_mut()
                .find(|x| x.size > 0 && x.place_chunk != format::NONE)
                .map(|x| x.place_offset += 4)
                .is_some()
        }),
        ("FRAGS", |s| s.frags.iter_mut().find(|f| f.offset > 0).map(|f| f.offset = 0).is_some()),
        ("SLOTS", |s| {
            let n = s.slots.len();
            n >= 1 && {
                s.slots.truncate(n - 1);
                true
            }
        }),
        ("EH_RECORDS", |s| {
            let n = s.eh_records.len();
            n >= 1 && {
                s.eh_records.truncate(n - 1);
                true
            }
        }),
        ("SYMTAB_LOCALS", |s| {
            s.symtab_locals.iter_mut().find(|x| x.len > 0).map(|x| x.len -= 1).is_some()
        }),
        ("SIZES", |s| first(&mut s.sizes, |z| z.size += 16)),
        ("SYM_PLACE", |s| first(&mut s.sym_place, |p| p.offset += 8)),
        ("DEFINED", |s| first(&mut s.defined, |d| d.value += 4)),
        ("PATCHES", |s| first(&mut s.patches, |p| p.offset += 4)),
        ("REDIRECTS", |s| first(&mut s.redirects, |r| r.offset += 4)),
        ("SIZED", |s| first(&mut s.sized, |z| z.size += 8)),
        ("LAYOUT", |s| {
            s.layout.entry += 4;
            true
        }),
    ]
}

// spec: incr.md IN-61 (the state alone reproduces the output, and every
// stored decision table is used: one changed row changes the output or
// fails the link)
#[test]
fn replay_reproduces_and_each_table_matters() {
    for prog in [Prog::asm(), Prog::c(), Prog::cxx()].into_iter().flatten() {
        prog.link("plain", &[]);
        prog.link("full", &["--incremental=full"]);
        let (_, state) = prog.read_state("full");
        let plain = prog.bytes("plain");
        let same = replay(&prog, "replayed", &state).expect("the replay failed");
        assert!(
            same == plain,
            "{}: the replay differs from the plain link",
            prog.built.dir.display()
        );
        let mut applied = Vec::new();
        for (n, (table, mutate)) in mutations().into_iter().enumerate() {
            let mut s = state.clone();
            if !mutate(&mut s) {
                continue;
            }
            applied.push(table);
            let out = format!("mut{n}");
            if let Some(bytes) = replay(&prog, &out, &s) {
                assert!(
                    bytes != plain,
                    "{}: a changed {table} row changed nothing",
                    prog.built.dir.display()
                );
            }
        }
        eprintln!("{}: mutated {applied:?}", prog.built.dir.display());
        for need in ["CHUNKS", "ORDER", "SEGMENTS", "SECTIONS", "SIZES", "LAYOUT"] {
            assert!(applied.contains(&need), "{need} was empty");
        }
    }
}

// ---- stamps ----------------------------------------------------------------

/// The directories to try the stamp rule on: the build directory's
/// scratch (the workspace's file system), and `MORTAR_INCR_STAMP_DIR` when
/// set (another file system, such as an overlay `/tmp`).
fn stamp_dirs(scratch: &Scratch) -> Vec<PathBuf> {
    let mut v = vec![scratch.path().to_path_buf()];
    if let Some(d) = std::env::var_os("MORTAR_INCR_STAMP_DIR") {
        v.push(PathBuf::from(d));
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

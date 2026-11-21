//! WP-267 P-2 (M-7): the replay hook's context (`mortar::front::Replay`,
//! `spec/incr.md` IN-61). Front 2's hook runs `materialize` and the judges
//! of the update, so it must be given what front 1's `Front` gives them:
//! the link, the computed layout, the merge classes and each file's
//! `.eh_frame` run. These tests hold the context to the front's own
//! tables (a front with a hook sees what a front without one keeps), check
//! that a hook still fires and replaces the layout (IN-61's test hooks),
//! and check that runs the front cannot give reach the hook as an error,
//! by name, which the hook turns into a named refusal.
//!
//! The fixtures are AArch64 and x86-64 assembly: `a.o` (`_start`, CFI, a
//! call into `c.o`), `b.o` (data, merged strings), `c.o` (a function with
//! CFI).

use std::cell::RefCell;
use std::ffi::OsString;
use std::process::Command;
use std::time::Duration;

use mortar::front::{Env, Front, Replay, ReplayHook, front};
use mortar::timing::Timer;
use mortar_core::layout::Layout;
use mortar_core::par::Pool;
use mortar_diag::{Collector, Policy};
use mortar_input::store::{Store, StorePolicy};
use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, run_with, tools_required};

const ARCHES: [Arch; 2] = [Arch::Aarch64, Arch::X86_64];

fn sources(arch: Arch) -> [String; 3] {
    let (ty, pb, start, call) = match arch {
        Arch::X86_64 => (
            "@function",
            "@progbits",
            "\tcall f\n\tmovl %eax, %edi\n\tmovl $60, %eax\n\tsyscall\n",
            "\tmovl $42, %eax\n\tret\n",
        ),
        _ => (
            "%function",
            "%progbits",
            "\tbl f\n\tmov x8, #93\n\tsvc #0\n",
            "\tmov x0, #42\n\tret\n",
        ),
    };
    [
        format!(
            "\t.text\n\t.globl _start\n\t.type _start, {ty}\n_start:\n\t.cfi_startproc\n{start}\
             \t.cfi_endproc\n\t.section .note.GNU-stack,\"\",{pb}\n"
        ),
        format!(
            "\t.data\n\t.globl d\nd:\t.quad 7\n\
             \t.section .rodata.str1.1,\"aMS\",{pb},1\n\t.asciz \"hello\"\n\t.asciz \"world\"\n\
             \t.section .note.GNU-stack,\"\",{pb}\n"
        ),
        format!(
            "\t.text\n\t.globl f\n\t.type f, {ty}\nf:\n\t.cfi_startproc\n{call}\t.cfi_endproc\n\
             \t.section .note.GNU-stack,\"\",{pb}\n"
        ),
    ]
}

/// The objects of `arch` in a scratch directory, or `None` without its
/// assembler.
fn fixture(arch: Arch, label: &str) -> Option<Scratch> {
    let Some(gcc) = arch.tool("gcc") else {
        assert!(!tools_required(), "{} is required", arch.gnu("gcc"));
        return None;
    };
    let scratch = Scratch::new(label);
    for (name, text) in ["a", "b", "c"].iter().zip(sources(arch)) {
        let src = scratch.write(&format!("{name}.s"), text);
        let obj = scratch.join(format!("{name}.o"));
        let mut c = Command::new(gcc.clone());
        c.args(["-c".as_ref(), src.as_os_str(), "-o".as_ref(), obj.as_os_str()]);
        let o = run_with(&mut c, b"", Duration::from_mins(1));
        assert!(o.success(), "gcc: {}", o.stderr_text());
    }
    Some(scratch)
}

/// The front of the incremental-mode link of the fixture (`state5`: the
/// format-5 data, `.eh_frame` per-file runs), with `hook` as its replay;
/// `go` is given the front (`None`: it failed) and its diagnostics.
fn with_front<T>(
    scratch: &Scratch,
    state5: bool,
    hook: Option<&ReplayHook<'_>>,
    go: impl FnOnce(Option<&Front<'_>>, String) -> T,
) -> T {
    let args: Vec<OsString> =
        ["a.o", "b.o", "c.o", "-static", "-o", "out"].iter().map(OsString::from).collect();
    let parsed = mortar_cli::parse(&args);
    assert!(!parsed.failed());
    let store = Store::new(StorePolicy::from_env());
    let diag = Collector::new(Policy::default());
    let env = Env { cwd: scratch.path(), pool: Pool::new(2), replay: hook, slack: true, state5 };
    let f = front(parsed.plan, env, &store, &diag, &mut Timer::new(false));
    let report = format!("{:?}", diag.finish());
    go(f.as_ref(), report)
}

/// What a hook was given.
#[derive(Debug, Default)]
struct Seen {
    calls: usize,
    computed: Option<Layout>,
    meta: Vec<mortar_merge::merge::MergeMeta>,
    eh_runs: Option<Result<Vec<mortar_synth::eh::EhRun>, String>>,
    files: u32,
}

fn seen_of(seen: &RefCell<Seen>, cx: &Replay<'_, '_>) {
    let mut s = seen.borrow_mut();
    s.calls += 1;
    s.computed = Some(cx.computed.clone());
    s.meta = cx.meta.to_vec();
    s.eh_runs = Some(cx.eh_runs.map(<[_]>::to_vec).map_err(str::to_owned));
    s.files = cx.link.inputs.files.len();
}

// spec: incr.md IN-61 (the replay hook), WP-267 M-7 (its context).
#[test]
fn replay_context_is_the_fronts_own_tables() {
    for arch in ARCHES {
        let Some(scratch) = fixture(arch, "p2-context") else { continue };
        // The front without a hook: what front 1 hands `materialize`.
        let (layout, meta, runs, files) = with_front(&scratch, true, None, |f, report| {
            let f = f.unwrap_or_else(|| panic!("{arch:?}: front: {report}"));
            let runs = f.incr5.as_ref().map(|i| i.eh.clone()).expect("format-5 data");
            (f.layout.clone(), f.merge.meta.to_vec(), runs, f.inputs.files.len())
        });
        let runs = runs.unwrap_or_else(|e| panic!("{arch:?}: the front has no runs: {e}"));
        assert!(!runs.is_empty(), "{arch:?}: no .eh_frame run in a link with CFI");
        assert!(!meta.is_empty(), "{arch:?}: no merge class in a link with merged strings");
        // The same link with a hook that keeps the computed layout: it
        // fires once and is given those same tables.
        let seen = RefCell::new(Seen::default());
        let hook = |cx: &Replay<'_, '_>| {
            seen_of(&seen, cx);
            Ok(cx.computed.clone())
        };
        let replayed = with_front(&scratch, true, Some(&hook), |f, report| {
            f.unwrap_or_else(|| panic!("{arch:?}: front with the hook: {report}")).layout.clone()
        });
        let s = seen.into_inner();
        assert_eq!(s.calls, 1, "{arch:?}: the hook fires once");
        assert_eq!(s.computed.as_ref(), Some(&layout), "{arch:?}: computed");
        assert_eq!(replayed, layout, "{arch:?}: the hook's layout is the front's");
        assert_eq!(s.meta, meta, "{arch:?}: merge classes");
        assert_eq!(s.eh_runs, Some(Ok(runs)), "{arch:?}: .eh_frame runs (Incr5::eh)");
        assert_eq!(s.files, files, "{arch:?}: the link's files");
    }
}

// spec: incr.md IN-61: the hook's layout is the one the front writes from;
// a hook's error fails the link by name.
#[test]
fn replay_hook_replaces_and_refuses_by_name() {
    for arch in ARCHES {
        let Some(scratch) = fixture(arch, "p2-replace") else { continue };
        // A hook that returns another (invalid) layout: the front takes it
        // and `Layout::validate` refuses it (the hook's table is used).
        let hook = |_: &Replay<'_, '_>| Ok(Layout::default());
        with_front(&scratch, true, Some(&hook), |f, report| {
            assert!(f.is_none(), "{arch:?}: an empty layout replayed was accepted");
            assert!(report.contains("the layout breaks its rules"), "{arch:?}: {report}");
        });
        // A hook that refuses: `replay: <its words>`.
        let hook = |_: &Replay<'_, '_>| Err("P-2 refuses".to_owned());
        with_front(&scratch, true, Some(&hook), |f, report| {
            assert!(f.is_none(), "{arch:?}: a refusing hook was ignored");
            assert!(report.contains("replay: P-2 refuses"), "{arch:?}: {report}");
        });
    }
}

// WP-267 M-7 (review §5): `.eh_frame` runs the front cannot give reach
// the hook as an error, by name; `materialize`'s hook refuses with it.
#[test]
fn hook_with_an_err_eh_run_is_refused_by_name() {
    for arch in ARCHES {
        let Some(scratch) = fixture(arch, "p2-eh-err") else { continue };
        // Not a format-5 link: no per-file runs to give.
        let seen = RefCell::new(Seen::default());
        let hook = |cx: &Replay<'_, '_>| {
            seen_of(&seen, cx);
            // As `update::materialize_in_hook` (via `eh_files`) refuses.
            match cx.eh_runs {
                Ok(_) => Ok(cx.computed.clone()),
                Err(why) => Err(format!("no .eh_frame runs: {why}")),
            }
        };
        with_front(&scratch, false, Some(&hook), |f, report| {
            assert!(f.is_none(), "{arch:?}: the hook's refusal was ignored");
            assert!(
                report.contains("replay: no .eh_frame runs: not a format-5 link"),
                "{arch:?}: {report}"
            );
        });
        let s = seen.into_inner();
        assert_eq!(s.calls, 1, "{arch:?}");
        let why = s.eh_runs.expect("seen").expect_err("Err runs without state5");
        assert!(why.contains("not a format-5 link"), "{arch:?}: {why}");
    }
}

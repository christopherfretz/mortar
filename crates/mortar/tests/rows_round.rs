//! WP-261: a full link's capture writes `FILES.round` (`spec/incr.md`
//! IN-71b), and archive members are `LAZY` exactly when not extracted.
//!
//! `stored_round_equals_the_front_and_the_walk`: on a static C++ link
//! (the `support` program: libstdc++, libm, libc) with an archive whose
//! members chain `-u wp261_a` -> `a` -> `b` -> `c` (`r1.o` round 1, `r2.o`
//! 2, `r3.o` 3; `r4.o` never extracted), on AArch64 and x86-64, for EVERY
//! `FILES` row, three ways equal: the stored round, the front's
//! `Resolution::file_round`, and a breadth-first walk written here from
//! the stored `KEY_REFS` alone (§4.1's edges: a non-weak undefined
//! mention of `K` to the least file with a defined or common mention of
//! `K`, when that file is a member; a root option's key from round 0).
//! Every member is `LAZY` or of round at least 1 (the line has no
//! `--whole-archive`), and a member without a symbol table (x86-64's
//! `tinfo2.o`, libm's `w_*_compat.o`: F-7) is `LAZY`.

mod support;

use std::ffi::OsString;
use std::path::PathBuf;

use mortar::front::{Env, front};
use mortar::incremental::State5;
use mortar::timing::Timer;
use mortar_core::par::Pool;
use mortar_diag::{Collector, Policy};
use mortar_incr::format::def_kind;
use mortar_incr::update::rows::file_flags;
use mortar_incr::update::testing::MemoryState;
use mortar_testutil::{Scratch, run, tool, tools_required};
use mortar_core::incr::{IncrReason, OptionKeys};
use mortar_incr::update::extract;
use mortar_incr::update::view::ChangedFiles;
use support::{Driver, Source, build, build_by, with_output};

const STB_WEAK: u8 = 2;

const MEMBERS: [(&str, &str); 4] = [
    ("r1", "extern int wp261_b(int);\nint wp261_a(int x) { return wp261_b(x) + 1; }\n"),
    ("r2", "extern int wp261_c(int);\nint wp261_b(int x) { return wp261_c(x) * 2; }\n"),
    ("r3", "int wp261_c(int x) { return x - 3; }\n"),
    ("r4", "int wp261_unused(int x) { return x; }\n"),
];

struct Line {
    _scratch: Scratch,
    label: &'static str,
    dir: PathBuf,
    args: Vec<String>,
}

fn lines() -> Vec<Line> {
    let mut v = Vec::new();
    for (label, cc, a64) in [
        ("aarch64 g++", Driver::Gxx.tool(), true),
        ("x86-64 g++", "x86_64-linux-gnu-g++".into(), false),
    ] {
        let s = Scratch::new(if a64 { "wp261-a64" } else { "wp261-x64" });
        let built = if a64 {
            build(&s, Driver::Gxx, &["-O1"], support::CXX_PROGRAM)
        } else {
            build_by(&s, &cc, &["-O1"], support::CXX_PROGRAM)
        };
        let Some(b) = built else {
            assert!(!tools_required(), "{cc} is required");
            continue;
        };
        let compiler = tool(&cc).expect("the compiler");
        let mut objs: Vec<OsString> = Vec::new();
        for (name, text) in MEMBERS {
            let src = s.write(&format!("{name}.c"), text);
            let obj = s.join(format!("{name}.o"));
            let args: Vec<OsString> = vec![
                "-x".into(),
                "c".into(),
                "-O1".into(),
                "-c".into(),
                src.into_os_string(),
                "-o".into(),
                obj.clone().into_os_string(),
            ];
            let o = run(&compiler, &args);
            assert!(o.success(), "{cc}: {}", o.stderr_text());
            objs.push(obj.into_os_string());
        }
        let ar = tool("ar").expect("ar");
        let lib = b.dir.join("lib261.a");
        let mut args: Vec<OsString> = vec!["rcs".into(), lib.clone().into_os_string()];
        args.extend(objs);
        let o = run(&ar, &args);
        assert!(o.success(), "ar: {}", o.stderr_text());
        let mut args = b.args;
        args.push("-u".into());
        args.push("wp261_a".into());
        args.push(lib.to_string_lossy().into_owned());
        v.push(Line { _scratch: s, label, dir: b.dir, args });
    }
    v
}

/// The link's format-5 state, decoded whole (a test's read).
fn state(line: &Line) -> MemoryState {
    let path = line.dir.join("out.state5");
    let mut args = with_output(&line.args, &line.dir.join("out"));
    args.extend(["--incremental=full", "--incremental-state"].iter().map(OsString::from));
    args.push("out.mortar-state".into());
    let inv = mortar::invoke_state5(
        "mortar",
        &args,
        &line.dir,
        State5 { path: path.clone(), collide: false, rows: None },
    );
    let ok = matches!(inv.outcome, mortar::Outcome::Linked { .. });
    assert!(ok, "{}: the link failed: {:?}", line.label, inv.outcome);
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|_| panic!("{}: no state: {}", line.label, inv.incremental));
    mortar_incr::state5::read::decode(&bytes).expect("the state decodes")
}

/// `Resolution::file_round` of the whole front of `line`.
fn front_rounds(line: &Line) -> Vec<u32> {
    let parsed = mortar_cli::parse(&with_output(&line.args, &line.dir.join("front")));
    assert!(!parsed.failed(), "{:?}", parsed.diagnostics);
    let store = mortar_input::store::Store::new(mortar_input::store::StorePolicy::from_env());
    let diag = Collector::new(Policy::default());
    let pool = Pool::try_new(Pool::available()).expect("a pool");
    let env = Env { cwd: &line.dir, pool, replay: None, slack: true, state5: true };
    let f = front(parsed.plan, env, &store, &diag, &mut Timer::new(false));
    let report = diag.finish();
    assert!(!report.failed(), "{}: {report:?}", line.label);
    f.expect("the front").resolution.file_round.to_vec()
}

fn text(m: &MemoryState, s: mortar_incr::format::Str) -> String {
    let b = m.strings.get(s.off as usize..(s.off + s.len) as usize).unwrap_or_default();
    String::from_utf8_lossy(b).into_owned()
}

/// IN-71b's breadth-first rounds from the stored rows alone: live
/// non-members (every file not `LAZY` and not a member) round 0, a root
/// option's target round 1, then one more than the least round of a live
/// file with an edge to it. Per `FILES` row; 0 for a file not reached.
fn walk(m: &MemoryState) -> Vec<u32> {
    let n = m.files.len();
    let member = |f: usize| m.files[f].member != 0;
    let live = |f: usize| m.files[f].flags & (file_flags::LAZY | file_flags::GONE) == 0;
    let mut first = vec![u32::MAX; m.symbols.len()];
    for r in &m.key_refs {
        if r.def != def_kind::UNDEFINED && !r.tomb {
            let c = &mut first[r.key as usize];
            *c = (*c).min(r.file);
        }
    }
    let mut out: Vec<Option<u32>> = vec![None; n];
    let mut by_file: Vec<Vec<u32>> = vec![Vec::new(); n];
    for r in &m.key_refs {
        if r.def == def_kind::UNDEFINED && r.bind != STB_WEAK && !r.tomb && r.file != 0 {
            by_file[r.file as usize].push(r.key);
        }
    }
    let mut layer: Vec<usize> = Vec::new();
    for f in 1..n {
        if live(f) && !member(f) {
            out[f] = Some(0);
            layer.push(f);
        }
    }
    let mut next: Vec<usize> = Vec::new();
    for (k, s) in m.symbols.iter().enumerate() {
        let t = first[k];
        if s.option && t != u32::MAX && member(t as usize) && out[t as usize].is_none() {
            out[t as usize] = Some(1);
            next.push(t as usize);
        }
    }
    let mut r = 0u32;
    while !layer.is_empty() || !next.is_empty() {
        let mut found = Vec::new();
        for &f in &layer {
            for &k in &by_file[f] {
                let t = first[k as usize];
                if t != u32::MAX && member(t as usize) && out[t as usize].is_none() {
                    out[t as usize] = Some(r + 1);
                    found.push(t as usize);
                }
            }
        }
        // The next layer: this round's finds and the roots' targets (at
        // round 1, found before round 0 ran).
        found.append(&mut next);
        found.retain(|&t| out[t] == Some(r + 1));
        found.sort_unstable();
        found.dedup();
        layer = found;
        r += 1;
    }
    out.into_iter().map(|x| x.unwrap_or(0)).collect()
}

#[test]
fn stored_round_equals_the_front_and_the_walk() {
    let mut ran = 0;
    for line in lines() {
        let m = state(&line);
        let front = front_rounds(&line);
        let walked = walk(&m);
        let label = line.label;
        assert_eq!(m.files.len(), front.len(), "{label}: FILES rows against the front's files");
        let (mut extracted, mut lazy, mut no_syms, mut deepest) = (0, 0, 0, 0);
        let mut named = Vec::new();
        let mut mentioned = vec![false; m.files.len()];
        for r in &m.key_refs {
            if let Some(x) = mentioned.get_mut(r.file as usize) {
                *x = true;
            }
        }
        for (i, row) in m.files.iter().enumerate() {
            let is_lazy = row.flags & file_flags::LAZY != 0;
            let name = format!("{}({})", text(&m, row.path), text(&m, row.member_name));
            assert_eq!(row.round, front[i], "{label}: file {i} {name}: stored against the front");
            assert_eq!(row.round, walked[i], "{label}: file {i} {name}: stored against the walk");
            if row.member != 0 {
                assert!(
                    is_lazy != (row.round > 0),
                    "{label}: file {i} {name}: a member neither LAZY nor of round >= 1 \
                     (flags {:#x}, round {})",
                    row.flags,
                    row.round
                );
                if is_lazy {
                    lazy += 1;
                    assert_eq!(row.round, 0, "{label}: file {i} {name}: a LAZY row's round");
                } else {
                    extracted += 1;
                    deepest = deepest.max(row.round);
                }
                if !mentioned[i] {
                    no_syms += 1;
                    assert!(is_lazy, "{label}: file {i} {name}: no mention, yet extracted");
                }
            } else {
                assert_eq!(row.round, 0, "{label}: file {i} {name}: a non-member's round");
            }
            let member = text(&m, row.member_name);
            if let Some(r) = MEMBERS.iter().position(|(n, _)| member == format!("{n}.o")) {
                named.push((r, if is_lazy { None } else { Some(row.round) }));
            }
        }
        assert_eq!(m.meta.iterations, 1, "{label}: META.iterations of a link that ran once");
        named.sort_unstable();
        assert_eq!(
            named,
            vec![(0, Some(1)), (1, Some(2)), (2, Some(3)), (3, None)],
            "{label}: lib261.a's members (-u: r1 1, r2 2, r3 3; r4 LAZY)"
        );
        eprintln!(
            "[wp261] {label}: {} FILES rows, {extracted} extracted members (deepest round \
             {deepest}), {lazy} LAZY, {no_syms} members without a mention (all LAZY)",
            m.files.len()
        );
        ran += 1;
    }
    eprintln!("[wp261] {ran} links compared");
}

/// resolve.md §6.2's iterated shape (WP-200; `mortar_resolve`'s
/// `s6_2_wp200_strong_reference_extracts_past_a_dropped_weak_definition`)
/// on real objects: `wu.o` refers strongly to `wp261_y`, `wp261_g` and
/// `wp261_hx`; `wa.o` keeps COMDAT group `wp261G` (`wp261_g`); `wb.o`'s
/// copy of it also defines `wp261_hx` WEAK, and `wb.o` defines
/// `wp261_y`; `libhx.a`'s `hx.o` defines `wp261_hx`. Round 0 takes
/// `wb.o`'s weak definition as `first(wp261_hx)`, so `hx.o` is not
/// extracted; `wb.o`'s copy is dropped, its `wp261_hx` demoted, and the
/// closure runs again and extracts `hx.o`: 2 iterations.
const ITERATED: [Source; 3] = [
    Source {
        name: "wu.c",
        text: "extern int wp261_y(void);\nextern void wp261_g(void), wp261_hx(void);\n\
void *wp261_refs[] = { (void *)wp261_y, (void *)wp261_g, (void *)wp261_hx };\n\
int main(void) { return wp261_refs[2] == 0; }\n",
    },
    Source {
        name: "wa.c",
        text: "__asm__(\".section .text.wp261g,\\\"axG\\\",@progbits,wp261G,comdat\\n\"\n\
\".globl wp261_g\\n.type wp261_g, @function\\nwp261_g: .long 0\\n.text\\n\");\n\
int wp261_a_only(void) { return 4; }\n",
    },
    Source {
        name: "wb.c",
        text: "__asm__(\".section .text.wp261g,\\\"axG\\\",@progbits,wp261G,comdat\\n\"\n\
\".globl wp261_g\\n.type wp261_g, @function\\nwp261_g: .long 0\\n\"\n\
\".weak wp261_hx\\n.type wp261_hx, @function\\nwp261_hx: .long 0\\n.text\\n\");\n\
int wp261_y(void) { return 1; }\n",
    },
];

// spec: incr.md IN-71b "Iterations" (WP-261; rulings "After WP-261's
// first hand-back" item 4): the full link's capture writes
// `Resolution::iterations` into `META.iterations`; the update of an
// iterated link falls back `closure-iterated`, by name, from `extract`
// and end to end.
#[test]
fn iterated_link_stores_its_iterations_and_its_update_falls_back() {
    let mut ran = 0;
    for (label, cc) in [("aarch64", "gcc"), ("x86-64", "x86_64-linux-gnu-gcc")] {
        let s = Scratch::new(&format!("wp261-iter-{label}"));
        let Some(b) = build_by(&s, cc, &["-O1"], &ITERATED) else {
            assert!(!tools_required(), "{cc} is required");
            continue;
        };
        let compiler = tool(cc).expect("the compiler");
        let hx_src = s.write("hx.c", "void wp261_hx(void) {}\n");
        let hx = s.join("hx.o");
        let o = run(
            &compiler,
            &["-O1".into(), "-c".into(), hx_src.into_os_string(), "-o".into(), hx.clone().into()],
        );
        assert!(o.success(), "{cc}: {}", o.stderr_text());
        let lib = b.dir.join("libhx.a");
        let ar = tool("ar").expect("ar");
        let o = run(&ar, &["rcs".into(), lib.clone().into_os_string(), hx.into_os_string()]);
        assert!(o.success(), "ar: {}", o.stderr_text());
        let mut args = b.args;
        // Before libc's group: the line `wu.o wa.o wb.o libhx.a`.
        let at = args.iter().position(|a| a.ends_with("wb.c.o")).expect("wb.c.o") + 1;
        args.insert(at, lib.to_string_lossy().into_owned());
        let line = Line { _scratch: s, label, dir: b.dir, args };
        let m = state(&line);
        assert_eq!(m.meta.iterations, 2, "{label}: META.iterations of the iterated link");
        let hx_row = m.files.iter().find(|r| text(&m, r.member_name) == "hx.o").expect("hx.o");
        assert_eq!(hx_row.flags & file_flags::LAZY, 0, "{label}: hx.o extracted");
        let view = m.view();
        let r = extract::extract(&view, &ChangedFiles::default(), &OptionKeys::default());
        assert!(matches!(r, Err(IncrReason::ClosureIterated(_))), "{label}: {r:?}");
        // End to end: `wa.o` edited (a new body), then an update.
        let src = line.dir.join("wa.c");
        let edited = ITERATED[1].text.replace("return 4;", "return 5;");
        std::fs::write(&src, edited).expect("wa.c");
        let obj = line.dir.join("wa.c.o.new");
        let o = run(
            &compiler,
            &["-O1".into(), "-c".into(), src.into_os_string(), "-o".into(), obj.clone().into()],
        );
        assert!(o.success(), "{cc}: {}", o.stderr_text());
        std::fs::rename(&obj, line.dir.join("wa.c.o")).expect("rename");
        let mut args = with_output(&line.args, &line.dir.join("out"));
        args.extend(
            ["--incremental=update", "--incremental-state", "out.mortar-state"]
                .map(OsString::from),
        );
        let second = mortar::invoke_in("mortar", &args, &line.dir);
        let report = format!("{:?}\n{}", second.outcome, second.incremental);
        assert!(report.contains("closure-iterated"), "{label}: {report}");
        eprintln!("[wp261] {label}: iterated link, META.iterations 2, update: closure-iterated");
        ran += 1;
    }
    assert!(ran == 2 || !tools_required(), "{ran} of 2 links");
}

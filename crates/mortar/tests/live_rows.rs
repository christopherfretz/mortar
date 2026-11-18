//! WP-238 Part 2: the stored liveness rows (`EDGES`, `META.live_hash`)
//! that an incremental-mode `--gc-sections` link captures, checked by the
//! Rust oracle (`mortar::incremental::live_check`, P0-7): the stored-form
//! walk over the captured rows gives the full mark's sections and
//! fragments, or refuses by a named rule. The reviewers' scenarios
//! (`tests/incr-review/wp238/roots.sh`, `fdeglobal.sh`; wp238b's
//! `lorder.sh`, `values.sh`, `ident.sh`) on AArch64 and x86-64, and F1:
//! `OptionKeys::live_roots()` through `find_key` is resolve's
//! `Outcome::roots`, element for element.

mod support;

use std::ffi::OsString;
use std::path::PathBuf;

use mortar::front::{Env, Front, front};
use mortar::incremental::State5;
use mortar::timing::Timer;
use mortar_diag::{Collector, Policy};
use mortar_incr::update::rows::edge_kind;
use mortar_incr::update::testing::MemoryState;
use mortar_testutil::{Scratch, tools_required};
use support::{Source, build_by, store, with_output};

/// A static `--gc-sections` link line in its scratch directory.
struct Line {
    _scratch: Scratch,
    label: String,
    dir: PathBuf,
    args: Vec<String>,
}

impl Line {
    fn args(&self, out: &str) -> Vec<OsString> {
        let mut a = with_output(&self.args, &self.dir.join(out));
        a.extend(["--incremental=full", "--incremental-state"].iter().map(OsString::from));
        a.push(format!("{out}.mortar-state").into());
        a
    }
}

/// The two 64-bit targets' C drivers and their return-address column.
const TARGETS: [(&str, &str, u32); 2] =
    [("aarch64", "gcc", 30), ("x86-64", "x86_64-linux-gnu-gcc", 16)];

/// Compiles `sources` with `cc` and `flags`; the static link line with
/// `--gc-sections` and `extra` appended (`None`: the tool is missing).
fn line(label: &str, cc: &str, flags: &[&str], sources: &[Source], extra: &[&str]) -> Option<Line> {
    let s = Scratch::new(&format!("wp238-{}", label.replace([' ', '/'], "-")));
    let Some(b) = build_by(&s, cc, flags, sources) else {
        assert!(!tools_required(), "{cc} is required");
        return None;
    };
    let mut args = b.args;
    args.push("--gc-sections".into());
    for e in extra {
        args.push(e.replace("@DIR@", &b.dir.display().to_string()));
    }
    Some(Line { _scratch: s, label: label.to_owned(), dir: b.dir, args })
}

/// The captured format-5 rows of `l`'s link (incremental mode, the
/// switch on), and its `incremental:` lines.
fn rows5(l: &Line) -> (MemoryState, String) {
    let sink: mortar::incremental::Rows5Sink = std::sync::Arc::default();
    let inv = mortar::invoke_state5(
        "mortar",
        &l.args("out"),
        &l.dir,
        State5 { path: l.dir.join("out.state5"), collide: false, rows: Some(sink.clone()) },
    );
    assert!(
        matches!(inv.outcome, mortar::Outcome::Linked { .. }),
        "{}: the link failed: {:?}\n{}",
        l.label,
        inv.outcome,
        inv.incremental
    );
    let m = sink.lock().expect("the sink").take();
    let m = m.unwrap_or_else(|| panic!("{}: no rows: {}", l.label, inv.incremental));
    (m, inv.incremental)
}

/// The front of `l`'s link (incremental mode, the switch on).
fn front5<'s>(l: &Line, store: &'s mortar_input::store::Store) -> Front<'s> {
    let parsed = mortar_cli::parse(&l.args("front"));
    assert!(!parsed.failed(), "{:?}", parsed.diagnostics);
    let diag = Collector::new(Policy::default());
    let pool =
        mortar_core::par::Pool::try_new(mortar_core::par::Pool::available()).expect("a pool");
    let env = Env { cwd: &l.dir, pool, replay: None, slack: true, state5: true };
    let f = front(parsed.plan, env, store, &diag, &mut Timer::new(false));
    let report = diag.finish();
    assert!(!report.failed(), "{}: {report:?}", l.label);
    f.expect("the front")
}

/// The oracle's verdict on `l`: its line, and the number of `REFUSE` rows
/// captured. Panics on a disagreement (the first difference).
fn check(l: &Line) -> (String, usize) {
    let (m, lines) = rows5(l);
    assert!(lines.contains("EDGES "), "{}: no EDGES note: {lines}", l.label);
    let store = store();
    let f = front5(l, &store);
    let verdict = mortar::incremental::live_check(&f, &m)
        .unwrap_or_else(|why| panic!("{}: live-check DISAGREE {why}", l.label));
    let refuse = m.edges.iter().filter(|(_, e)| e.kind == edge_kind::REFUSE).count();
    (verdict, refuse)
}

fn agrees(l: &Line) {
    let (v, refuse) = check(l);
    assert!(v.contains("live-check agree"), "{}: {v}", l.label);
    assert_eq!(refuse, 0, "{}: {v}", l.label);
}

fn refused(l: &Line, rule: &str) {
    let (v, refuse) = check(l);
    assert!(v.contains(&format!("live-check refused {rule}:")), "{}: {v}", l.label);
    assert!(refuse > 0, "{}: no REFUSE row: {v}", l.label);
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// roots.sh s1: `-init=myinit` is the only root of myinit's section.
#[test]
fn s1_init_root_agrees() {
    for (arch, cc, _) in TARGETS {
        let src = [
            Source { name: "a.c", text: "int main(void){ return 0; }\n" },
            Source {
                name: "b.c",
                text: "int myinit(void){ return 40; }\nint other(void){ return 1; }\n",
            },
        ];
        let Some(l) = line(
            &format!("s1 {arch}"),
            cc,
            &["-O1", "-g", "-ffunction-sections"],
            &src,
            &["-init=myinit"],
        ) else {
            continue;
        };
        agrees(&l);
    }
}

/// roots.sh s2: a non-allocated `.myinfo` names `__start_ureg`; `ureg`
/// is live only through that row.
#[test]
fn s2_start_stop_from_nonalloc_agrees() {
    for (arch, cc, _) in TARGETS {
        let src = [
            Source { name: "a.c", text: "int main(void){ return 0; }\n" },
            Source {
                name: "u.c",
                text: "__attribute__((section(\"ureg\"), used)) const int ureg_a = 10;\n\
                       int helper(void){ return 0; }\n\
                       asm(\".pushsection .myinfo,\\\"\\\",%progbits\\n.quad __start_ureg\\n\
                       .quad __stop_ureg\\n.popsection\");\n",
            },
        ];
        let Some(l) =
            line(&format!("s2 {arch}"), cc, &["-O1", "-g", "-ffunction-sections"], &src, &[])
        else {
            continue;
        };
        agrees(&l);
    }
}

/// fdeglobal.sh: an FDE naming its code through a global (S5): a
/// `REFUSE` row, and the walk refuses `fde-global`.
#[test]
fn fde_global_is_refused() {
    for (arch, cc, ra) in TARGETS {
        let f = leak(format!(
            "  .section .text.foo,\"ax\",%progbits\n  .weak foo\n  .type foo,%function\nfoo:\n  ret\n\
             \x20 .size foo,.-foo\n  .section .rodata.lsda,\"a\",%progbits\nlsda:\n  .long 0x4c534441\n\
             \x20 .section .eh_frame,\"a\",%progbits\ncie:\n  .long cie_end - cie_start\ncie_start:\n\
             \x20 .long 0\n  .byte 1\n  .asciz \"zLR\"\n  .uleb128 1\n  .sleb128 -8\n  .uleb128 {ra}\n\
             \x20 .uleb128 2\n  .byte 0x1b\n  .byte 0x1b\n  .balign 4, 0\ncie_end:\nfde:\n\
             \x20 .long fde_end - fde_start\nfde_start:\n  .long fde_start - cie\n  .long foo - .\n\
             \x20 .long 4\n  .uleb128 4\n  .long lsda - .\n  .balign 4, 0\nfde_end:\n"
        ));
        let src = [
            Source { name: "m.c", text: "int foo(void);\nint main(void){ return foo(); }\n" },
            Source { name: "f.s", text: f },
            Source {
                name: "g.s",
                text: "  .section .text.foo,\"ax\",%progbits\n  .globl foo\n  .type foo,%function\n\
                       foo:\n  ret\n  .size foo,.-foo\n",
            },
        ];
        let Some(l) = line(&format!("fdeglobal {arch}"), cc, &["-O1"], &src, &[]) else { continue };
        refused(&l, "fde-global");
    }
}

const LO_A: &str = "  .section .text.g,\"axG\",%progbits,grp,comdat\n  .globl gfun\n  .type gfun,%function\n\
gfun:\n  ret\n  .section .meta,\"ao\",%progbits,gfun\n  .globl metasym\nmetasym:\n  .long 5\n";
const LO_B_GROUP: &str = "  .section .text.g,\"axG\",%progbits,grp,comdat\n  .globl gfun\n  .type gfun,%function\ngfun:\n  ret\n";
const LO_B_PLAIN: &str = "  .section .text.b,\"ax\",%progbits\n  .globl bfun\nbfun:\n  ret\n";
const LO_M: &str = "int gfun(void);\nint main(void){ return gfun(); }\n";

/// lorder.sh lo1 (and lo2's second version): `metasym` is defined in a
/// link-order `.meta` whose group member loses the election, so `.meta`
/// is dropped while `-u metasym` names it (F11, judged on the outcome): a
/// `REFUSE` row, the walk refuses `def-dropped`. lo2's first version (b.o
/// without the group: a.o's copy wins, `.meta` kept) agrees.
#[test]
fn link_order_def_dropped_is_refused() {
    for (arch, cc, _) in TARGETS {
        for (n, b, refuse) in [("lo1", LO_B_GROUP, true), ("lo2 v0", LO_B_PLAIN, false)] {
            let src = [
                Source { name: "m.c", text: LO_M },
                Source { name: "b.s", text: b },
                Source { name: "a.s", text: LO_A },
            ];
            let Some(l) = line(&format!("{n} {arch}"), cc, &["-O1"], &src, &["-u", "metasym"])
            else {
                continue;
            };
            if refuse {
                refused(&l, "def-dropped");
            } else {
                agrees(&l);
            }
        }
    }
}

/// values.sh kbig: `-u s_big`, `s_big` = a split section's start +
/// 0x100000005 (a value of 2^32 or more: F12's one conversion, every
/// piece).
#[test]
fn kbig_value_agrees() {
    for (arch, cc, _) in TARGETS {
        let main = if arch == "x86-64" {
            "  .text\n  .globl main\nmain:\n  movl $0, %eax\n  ret\n"
        } else {
            "  .text\n  .globl main\nmain:\n  mov w0, 0\n  ret\n"
        };
        let src = [
            Source { name: "m.s", text: main },
            Source {
                name: "c.s",
                text: "  .section .rodata.s,\"aMS\",%progbits,1\n.Lsa: .asciz \"alpha\"\n\
                       .Lsb: .asciz \"bravo\"\n.Lsc: .asciz \"charlie\"\n  .globl s_big\n\
                       \x20 .set s_big, .Lsa + 0x100000005\n  .section .rodata.t,\"aMS\",%progbits,1\n\
                       \x20 .asciz \"other\"\n",
            },
        ];
        let Some(l) =
            line(&format!("kbig {arch}"), cc, &["-nostdlib"], &src, &["-e", "main", "-u", "s_big"])
        else {
            continue;
        };
        agrees(&l);
    }
}

/// ident.sh dup: w.o twice on the line (two files of one identity; S4's
/// OCCURRENCE, F14, tells them apart): the hash over both copies agrees.
#[test]
fn duplicate_file_agrees() {
    for (arch, cc, _) in TARGETS {
        let src = [
            Source { name: "m.c", text: "int wf(void);\nint main(void){ return wf(); }\n" },
            Source {
                name: "w.c",
                text: "__attribute__((weak)) int wf(void){ return 1; }\n\
                       __attribute__((weak)) int wg(void){ return 2; }\n",
            },
        ];
        let Some(l) = line(
            &format!("dup {arch}"),
            cc,
            &["-O1", "-ffunction-sections"],
            &src,
            &["@DIR@/w.c.o"],
        ) else {
            continue;
        };
        let (m, _) = rows5(&l);
        let w = m
            .files
            .iter()
            .filter(|r| {
                let p = &m.strings[r.path.off as usize..(r.path.off + r.path.len) as usize];
                p.ends_with(b"w.c.o")
            })
            .count();
        assert_eq!(w, 2, "{}: w.o's FILES rows", l.label);
        agrees(&l);
    }
}

/// F1: `live_roots()` through `find_key` (`live_root_keys`, the walk's
/// lookup) is resolve's `Outcome::roots`, element for element, on a link
/// rooting through the entry, `-u`, `EXTERN`, `--require-defined`, a
/// `--defsym` base, `-init` and `-fini`.
#[test]
fn live_roots_are_resolves_roots() {
    for (arch, cc, _) in TARGETS {
        let src = [
            Source { name: "a.c", text: "int main(void){ return 0; }\n" },
            Source {
                name: "r.c",
                text: "int r_undef(void){ return 1; }\nint r_extern(void){ return 2; }\n\
                       int r_req(void){ return 3; }\nint r_base(void){ return 4; }\n\
                       void r_init(void){}\nvoid r_fini(void){}\n",
            },
        ];
        let Some(l) = line(
            &format!("f1 {arch}"),
            cc,
            &["-O1", "-ffunction-sections"],
            &src,
            &[
                "-u",
                "r_undef",
                "@DIR@/extern.ld",
                "--require-defined=r_req",
                "--defsym=r_alias=r_base",
                "-init=r_init",
                "-fini=r_fini",
            ],
        ) else {
            continue;
        };
        std::fs::write(l.dir.join("extern.ld"), "EXTERN(r_extern)\n").expect("the script");
        let (m, _) = rows5(&l);
        let store = store();
        let f = front5(&l, &store);
        let options = mortar_resolve::tables::option_keys(&f.plan, &f.inputs.link.directives);
        let names: Vec<Vec<u8>> = options.live_roots().iter().map(|(n, _)| n.to_vec()).collect();
        for want in [
            b"_start".as_slice(),
            b"r_undef",
            b"r_extern",
            b"r_req",
            b"r_base",
            b"r_init",
            b"r_fini",
        ] {
            assert!(names.iter().any(|n| n == want), "{}: {want:?} not a live root", l.label);
        }
        let view = m.view();
        let stored = mortar_incr::update::live::live_root_keys(&view, &options).expect("find_key");
        let s = |x: mortar_incr::format::Str| {
            m.strings[x.off as usize..(x.off + x.len) as usize].to_vec()
        };
        let stored: Vec<(Vec<u8>, Vec<u8>)> = stored
            .iter()
            .map(|k| {
                let k = k.unwrap_or_else(|| panic!("{}: a root without a SYMBOLS row", l.label));
                let r = &m.symbols[k.0 as usize];
                (s(r.name), s(r.version))
            })
            .collect();
        let outcome = mortar_resolve::tables::resolve_inputs(&f.inputs, &f.plan, &f.pool);
        let resolved: Vec<(Vec<u8>, Vec<u8>)> = outcome
            .roots
            .iter()
            .map(|&id| {
                let k = &f.inputs.symbols[id];
                (k.name.to_vec(), k.version.unwrap_or_default().to_vec())
            })
            .collect();
        assert_eq!(stored, resolved, "{}: live_roots through find_key vs Outcome::roots", l.label);
    }
}

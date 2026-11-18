//! WP-255c2a: the clip plan (`mortar_image::clip_plan::for_clips`,
//! `spec/landed/incr/WP-255.md` §9.1) on linked fixtures, AArch64 and
//! x86-64: U-6's three files (`a.o` with `_start`, `b.o` with 4 MiB of
//! data and merged strings, `c.o` with the edited function) and the pad
//! fixture (c.o's function grown by 0x3000 bytes, the review's code gap).
//! The clip plan is built from the link's rows (`FrontRows`) and data
//! lists computed from the `Link`; inside the clips its rows are the whole
//! plan's rows, and its fill is `refill_ranges`' fill.

use std::ffi::OsString;
use std::ops::Range;
use std::process::Command;
use std::time::Duration;

use mortar::front::{Env, front};
use mortar::timing::Timer;
use mortar_core::inputs::Reader;
use mortar_core::layout::ChunkKind;
use mortar_core::par::Pool;
use mortar_diag::{Collector, Policy};
use mortar_image::clip_plan::{ClipError, NotYet};
use mortar_image::link::{Link, refill_clip_plan, refill_ranges_for_test};
use mortar_image::plan::{InputUnit, Pattern, Plan, RelocSection, Unit};
use mortar_input::store::{Store, StorePolicy};
use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, run_with, tools_required};

const ARCHES: [Arch; 2] = [Arch::Aarch64, Arch::X86_64];
const W: u64 = mortar_image::IMAGE_BLOCK;

/// U-6's sources (`update_image.rs`): `c.s`'s function jumps over `pad`
/// bytes.
fn sources(arch: Arch, pad: usize) -> [String; 3] {
    let pad = ".skip ".to_owned() + &pad.to_string();
    match arch {
        Arch::X86_64 => [
            "\t.text\n\t.globl _start\n\t.type _start, @function\n_start:\n\t.cfi_startproc\n\
             \tmovq d@GOTPCREL(%rip), %rax\n\tmovq (%rax), %rdi\n\tcall f\n\tmovl %eax, %edi\n\
             \tmovl $60, %eax\n\tsyscall\n\t.cfi_endproc\n\
             \t.section .note.GNU-stack,\"\",@progbits\n"
                .to_owned(),
            "\t.data\n\t.globl d\nd:\t.quad 7\n\
             \t.section .rodata.str1.1,\"aMS\",@progbits,1\n\t.asciz \"hello\"\n\t.asciz \"world\"\n\
             \t.section .rodata.cst8,\"aM\",@progbits,8\n\t.p2align 3\n\t.quad 0x1122334455667788\n\
             \t.section .note.GNU-stack,\"\",@progbits\n\t.data\n\t.skip 0x400000, 1\n"
                .to_owned(),
            format!(
                "\t.text\n\t.p2align 4\n\t.globl f\n\t.type f, @function\nf:\n\t.cfi_startproc\n\
                 \tmovl $42, %eax\n\tjmp 1f\n\t{pad}\n1:\tret\n\t.cfi_endproc\n\
                 \t.section .note.GNU-stack,\"\",@progbits\n"
            ),
        ],
        _ => [
            "\t.text\n\t.globl _start\n\t.type _start, %function\n_start:\n\t.cfi_startproc\n\
             \tadrp x1, :got:d\n\tldr x1, [x1, :got_lo12:d]\n\tldr x0, [x1]\n\tbl f\n\
             \tmov x8, #93\n\tsvc #0\n\t.cfi_endproc\n\
             \t.section .note.GNU-stack,\"\",%progbits\n"
                .to_owned(),
            "\t.data\n\t.globl d\nd:\t.quad 7\n\
             \t.section .rodata.str1.1,\"aMS\",%progbits,1\n\t.asciz \"hello\"\n\t.asciz \"world\"\n\
             \t.section .rodata.cst8,\"aM\",%progbits,8\n\t.p2align 3\n\t.quad 0x1122334455667788\n\
             \t.section .note.GNU-stack,\"\",%progbits\n\t.data\n\t.skip 0x400000, 1\n"
                .to_owned(),
            format!(
                "\t.text\n\t.p2align 4\n\t.globl f\n\t.type f, %function\nf:\n\t.cfi_startproc\n\
                 \tmov x0, #42\n\tb 1f\n\t{pad}\n1:\tret\n\t.cfi_endproc\n\
                 \t.section .note.GNU-stack,\"\",%progbits\n"
            ),
        ],
    }
}

/// The fixture's objects in a scratch directory, or `None` without the
/// arch's assembler.
fn fixture(arch: Arch, label: &str, pad: usize) -> Option<Scratch> {
    let Some(gcc) = arch.tool("gcc") else {
        assert!(!tools_required(), "{} is required", arch.gnu("gcc"));
        return None;
    };
    let s = Scratch::new(label);
    for (name, text) in ["a", "b", "c"].iter().zip(sources(arch, pad)) {
        let src = s.write(&format!("{name}.s"), &text);
        let obj = s.join(format!("{name}.o"));
        let mut c = Command::new(&gcc);
        c.args(["-c".as_ref(), src.as_os_str(), "-o".as_ref(), obj.as_os_str()]);
        let o = run_with(&mut c, b"", Duration::from_mins(1));
        assert!(o.success(), "gcc: {}", o.stderr_text());
    }
    Some(s)
}

/// `go` with the `Link` of the fixture's incremental-mode link (U-6's
/// arguments; nothing is written).
fn with_link<T>(s: &Scratch, go: impl FnOnce(&Link<'_, '_>) -> T) -> T {
    let args: Vec<OsString> =
        ["a.o", "b.o", "c.o", "-static", "--eh-frame-hdr", "--build-id=sha1", "-o", "out"]
            .iter()
            .map(OsString::from)
            .collect();
    let parsed = mortar_cli::parse(&args);
    assert!(!parsed.failed());
    let store = Store::new(StorePolicy::from_env());
    let diag = Collector::new(Policy::default());
    let env = Env { cwd: s.path(), pool: Pool::new(2), replay: None, slack: true, state5: false };
    let f = front(parsed.plan, env, &store, &diag, &mut Timer::new(false))
        .unwrap_or_else(|| panic!("front: {:?}", diag.finish()));
    mortar::image::with_link(&f, &store, &|_| {}, go).unwrap()
}

/// The file range of input section `sec` of the object `file`.
#[allow(clippy::many_single_char_names)]
fn placed(l: &Link<'_, '_>, file: &str, sec: &[u8]) -> Range<u64> {
    let pl = &l.layout.placement;
    for (s, p) in pl.place.iter() {
        let Some((c, at)) = p.get() else { continue };
        let Some((f, index)) = l.resolution.sh_index(s) else { continue };
        let row = &l.inputs.files[f];
        let Reader::Object(o) = &row.reader else { continue };
        let x = o.section(index).unwrap();
        if row.path.ends_with(file.as_bytes()) && x.name().ok() == Some(sec) {
            let lo = pl.chunks[c].offset + at;
            return lo..lo + x.size();
        }
    }
    panic!("no {} in {file}", String::from_utf8_lossy(sec))
}

/// The file range of the chunk named `name` and its id.
fn chunk(l: &Link<'_, '_>, name: &[u8]) -> (mortar_core::ChunkId, Range<u64>, u64) {
    let (id, c) = l
        .layout
        .placement
        .chunks
        .iter()
        .find(|(_, c)| c.in_file() && c.name(l.assignment) == Some(name))
        .unwrap_or_else(|| panic!("no chunk {}", String::from_utf8_lossy(name)));
    (id, c.offset..c.offset + c.size, c.offset + c.size.max(c.capacity))
}

/// One row as the fill reads it, its indices resolved.
#[derive(Debug, PartialEq)]
enum View<'p> {
    Input(InputUnit, &'p [u8], Option<&'p RelocSection>),
    Bytes(&'p [u8]),
    Synth(u32),
    Merged(u32),
    Gap(Pattern),
}

/// The rows of `plan` over `piece`, each cut to it.
fn rows_over<'p>(plan: &'p Plan, piece: &Range<u64>) -> Vec<(Range<u64>, View<'p>)> {
    let mut out = Vec::new();
    for r in &plan.rows[plan.first_row(piece.start)..] {
        if r.offset >= piece.end {
            break;
        }
        let v = match r.unit {
            Unit::Input(i) => {
                let u = &plan.inputs[i as usize];
                let relocs = u.relocs.map(|k| &plan.relocs[k as usize]);
                View::Input(
                    InputUnit { name: 0, relocs: None, ..u.clone() },
                    plan.name(u.name),
                    relocs,
                )
            }
            Unit::Bytes(i) => View::Bytes(&plan.bytes[i as usize]),
            Unit::Synth(i) => View::Synth(i),
            Unit::Merged(i) => View::Merged(i),
            Unit::Gap(p) => View::Gap(p),
        };
        out.push((r.offset.max(piece.start)..r.end().min(piece.end), v));
    }
    out
}

/// The clips of one case: a name, the ranges, the units they must show.
type Case = (&'static str, Vec<Range<u64>>);

/// The cases over link `l`: one input section, a section's edge, two
/// windows, a code gap (every exec byte no section covers, the `.text`
/// tail included), the headers, and several at once.
#[allow(clippy::single_range_in_vec_init)]
fn cases(l: &Link<'_, '_>) -> Vec<Case> {
    let c_text = placed(l, "c.o", b".text");
    let a_text = placed(l, "a.o", b".text");
    let data = placed(l, "b.o", b".data");
    let boundary = (data.start / W + 1) * W;
    assert!(boundary + 16 < data.end, "b.o's .data holds a window boundary");
    let (_, text, text_cap) = chunk(l, b".text");
    let mut covered = vec![a_text.clone(), c_text.clone()];
    covered.sort_by_key(|r| r.start);
    let mut gaps = Vec::new();
    let mut at = text.start;
    for r in &covered {
        if r.start > at {
            gaps.push(at..r.start);
        }
        at = at.max(r.end);
    }
    if text_cap > at {
        gaps.push(at..text_cap);
    }
    assert!(!gaps.is_empty(), "the .text chunk has a code gap");
    let mut several = vec![0..0x40, c_text.start + 1..c_text.end - 1, boundary - 16..boundary + 16];
    several.sort_by_key(|r| r.start);
    vec![
        ("one section", vec![c_text.start + 1..c_text.end - 1]),
        ("a section's edge", vec![a_text.end - 2..a_text.end + 2]),
        ("two windows", vec![boundary - 16..boundary + 16]),
        ("a code gap", gaps),
        ("the headers", vec![0..0x40]),
        ("several", several),
    ]
}

// spec: WP-255.md §9.1 (the clip plan calls the whole plan's row makers;
// inside the clips its rows are the whole plan's rows), image.md I-27,
// I-41. For each case, every row of the whole plan over each piece of
// `clip_jobs(whole plan, clips)` (`refill_ranges`' pieces) is the clip
// plan's row over it, unit for unit (indices resolved), and the clip
// plan's pieces are those pieces.
#[test]
fn clip_plan_rows_equal_the_whole_plan_clipped() {
    for arch in ARCHES {
        for pad in [0, 0x3000] {
            let Some(s) = fixture(arch, &format!("clip-rows-{pad:x}"), pad) else { continue };
            with_link(&s, |l| {
                for (what, clips) in cases(l) {
                    let got = refill_clip_plan(l, &clips, |_| {})
                        .unwrap_or_else(|e| panic!("{arch:?} {what}: {e:?}"));
                    let whole = refill_ranges_for_test(l, &clips)
                        .unwrap_or_else(|e| panic!("{arch:?} {what}: {:?}", e.diagnostics));
                    let pieces: Vec<_> = whole.jobs.iter().map(|j| j.0.clone()).collect();
                    let mine: Vec<_> = got.jobs.iter().map(|j| j.0.clone()).collect();
                    assert_eq!(pieces, mine, "{arch:?} {what}: the pieces");
                    let mut n = 0;
                    for p in &pieces {
                        let want = rows_over(&got.whole, p);
                        assert_eq!(rows_over(&got.clip, p), want, "{arch:?} {what}: {p:x?}");
                        n += want.len();
                    }
                    eprintln!(
                        "[clip-rows] {arch:?} pad {pad:#x} {what}: {n} rows over {pieces:x?}"
                    );
                }
            });
        }
    }
}

// spec: WP-255.md §9.1 (the written-bitmap rule: a clip byte no row wrote
// is `internal-error (image: clip byte {offset:#x} written by no row)`,
// never a zero; it would have caught the review's F1), WP-255c2a (a clip
// that meets a chunk the clip plan does not build yet is refused by
// name). The bytes the clip plan fills are `refill_ranges`' bytes and its
// bitmap is full; a pad row (the code gap) or a header row turned into a
// zero gap by the test's hook is the internal error at its first byte;
// a clip over `.symtab` is `NotYet(Synth)`, over the merged strings
// `NotYet(Merged)`.
#[test]
#[allow(clippy::single_range_in_vec_init)]
fn clip_plan_fills_every_byte() {
    for arch in ARCHES {
        let Some(s) = fixture(arch, "clip-fill", 0x3000) else { continue };
        with_link(&s, |l| {
            for (what, clips) in cases(l) {
                let got = refill_clip_plan(l, &clips, |_| {})
                    .unwrap_or_else(|e| panic!("{arch:?} {what}: {e:?}"));
                let whole = refill_ranges_for_test(l, &clips)
                    .unwrap_or_else(|e| panic!("{arch:?} {what}: {:?}", e.diagnostics));
                assert_eq!(got.jobs, whole.jobs, "{arch:?} {what}: the bytes");
            }
            let all = cases(l);
            for (what, clips) in all.iter().filter(|c| c.0 == "a code gap" || c.0 == "the headers")
            {
                let mut hit = None;
                let r = refill_clip_plan(l, clips, |p| {
                    let k = p
                        .rows
                        .iter()
                        .position(|r| {
                            let wanted = match r.unit {
                                Unit::Gap(g) => g != Pattern::ZERO,
                                Unit::Bytes(_) => true,
                                _ => false,
                            };
                            wanted && clips.iter().any(|c| c.start < r.end() && r.offset < c.end)
                        })
                        .expect("a pad or header row in the clips");
                    let r = &mut p.rows[k];
                    hit = clips
                        .iter()
                        .find(|c| c.start < r.end() && r.offset < c.end)
                        .map(|c| c.start.max(r.offset));
                    r.unit = Unit::Gap(Pattern::ZERO);
                });
                if !cfg!(debug_assertions) {
                    continue;
                }
                let want = format!("image: clip byte {:#x} written by no row", hit.unwrap());
                match r {
                    Err(Err(f)) => assert!(
                        f.diagnostics.iter().any(|(_, d)| format!("{d:?}").contains(&want)),
                        "{arch:?} {what}: {:?}",
                        f.diagnostics
                    ),
                    other => panic!("{arch:?} {what}: no bitmap error: {other:?}"),
                }
                eprintln!("[clip-fill] {arch:?} {what}: the removed row is `{want}`");
            }
            let (symtab, ..) = chunk(l, b".symtab");
            let sym = &l.layout.placement.chunks[symtab];
            let r = refill_clip_plan(l, &[sym.offset..sym.offset + 8], |_| {});
            assert!(
                matches!(r, Err(Ok(ClipError::NotYet(NotYet::Synth, c))) if c == symtab),
                "{arch:?} .symtab: {r:?}"
            );
            let merged = l
                .layout
                .placement
                .chunks
                .iter()
                .find(|(_, c)| matches!(c.kind, ChunkKind::Merged(_)) && c.in_file() && c.size > 0)
                .expect("a merged chunk");
            let r = refill_clip_plan(l, &[merged.1.offset..merged.1.offset + 1], |_| {});
            assert!(
                matches!(r, Err(Ok(ClipError::NotYet(NotYet::Merged, c))) if c == merged.0),
                "{arch:?} merged: {r:?}"
            );
        });
    }
}

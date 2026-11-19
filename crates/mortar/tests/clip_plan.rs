//! WP-255c2a: the clip plan (`mortar_image::clip_plan::for_clips`,
//! `spec/landed/incr/WP-255.md` §9.1) on linked fixtures, AArch64 and
//! x86-64: U-6's three files (`a.o` with `_start`, `b.o` with 4 MiB of
//! data and merged strings, `c.o` with the edited function) and the pad
//! fixture (c.o's function grown by 0x3000 bytes, the review's code gap).
//! WP-255c2b adds to b.o a merge section of its own output section
//! (`.mycst`, two 8-byte constants).
//! The clip plan is built from the link's rows (`FrontRows`) and data
//! lists computed from the `Link`; inside the clips its rows are the whole
//! plan's rows, and its fill is `refill_ranges`' fill.
//!
//! WP-255c2b: the clip's section list from ONE producer
//! (`clip_plan::sections_in` over the rows and the member index) equals
//! the whole plan's units on every body chunk (the fixtures and a static
//! C++ link with `-ffunction-sections -fdata-sections`), and the gap rule
//! refuses a clip whose member index lost a section.

mod support;

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
use mortar_image::clip_plan::{ClipError, NotYet, Placed};
use mortar_image::link::{
    Link, clip_sections_for_test, refill_clip_plan, refill_clip_plan_with, refill_ranges_for_test,
};
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
             \t.section .mycst,\"aM\",@progbits,8\n\t.p2align 3\n\t.quad 0x99aabbccddeeff00\n\t.quad 0x0123456789abcdef\n\
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
             \t.section .mycst,\"aM\",%progbits,8\n\t.p2align 3\n\t.quad 0x99aabbccddeeff00\n\t.quad 0x0123456789abcdef\n\
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
    with_link_args(s.path(), &args, go)
}

/// `go` with the `Link` of the line `args` run in `cwd` (incremental mode;
/// nothing is written).
fn with_link_args<T>(
    cwd: &std::path::Path,
    args: &[OsString],
    go: impl FnOnce(&Link<'_, '_>) -> T,
) -> T {
    let parsed = mortar_cli::parse(args);
    assert!(!parsed.failed(), "{:?}", parsed.diagnostics);
    let store = Store::new(StorePolicy::from_env());
    let diag = Collector::new(Policy::default());
    let env = Env { cwd, pool: Pool::new(2), replay: None, slack: true, state5: false };
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
    /// The fragment order's chunk and the output section's name.
    Merged(u32, &'p [u8]),
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
            Unit::Merged(i) => {
                let m = &plan.merged[i as usize];
                View::Merged(m.chunk, plan.name(m.name))
            }
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
    let strs = merged_chunk(l, true);
    let cst = merged_chunk(l, false);
    let mut both = vec![strs.clone(), cst.clone()];
    both.sort_by_key(|r| r.start);
    vec![
        ("one section", vec![c_text.start + 1..c_text.end - 1]),
        ("a section's edge", vec![a_text.end - 2..a_text.end + 2]),
        ("two windows", vec![boundary - 16..boundary + 16]),
        ("a code gap", gaps),
        ("the headers", vec![0..0x40]),
        ("several", several),
        ("inside the merged strings", vec![strs.start + 2..strs.end - 2]),
        ("the merged strings' edge", vec![strs.start - 2..strs.start + 3]),
        ("the merged strings whole", vec![strs.clone()]),
        ("inside the merged constant", vec![cst.start + 1..cst.end - 1]),
        ("the merged constant's edge", vec![cst.end - 3..cst.end + 2]),
        ("both merged chunks whole", both),
    ]
}

/// The file range of the merged chunk holding the strings (`strings`,
/// in `.rodata` with b.o's constant) or of the merged constant chunk
/// (`.mycst`: two 8-byte constants).
fn merged_chunk(l: &Link<'_, '_>, strings: bool) -> Range<u64> {
    let want: &[u8] = if strings { b".rodata" } else { b".mycst" };
    let (_, c) = l
        .layout
        .placement
        .chunks
        .iter()
        .find(|(_, c)| {
            matches!(c.kind, ChunkKind::Merged(_))
                && c.in_file()
                && c.size > 0
                && c.name(l.assignment) == Some(want)
        })
        .expect("the merged chunk");
    c.offset..c.offset + c.size
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
// a clip over `.symtab` is `NotYet(Synth)`. WP-255c2b: the cases include
// clips inside, across the edge of and covering the merged string and
// constant chunks; one fragment dropped from the index (the hook) is
// refused by the gap rule at its first byte.
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
            merged_drop_refused(l, arch);
        });
    }
}

/// WP-255c2b: one fragment dropped from the index of the merged string
/// chunk and of the merged constant chunk is refused by the gap rule at
/// its first byte.
#[allow(clippy::many_single_char_names)]
fn merged_drop_refused(l: &Link<'_, '_>, arch: Arch) {
    for strings in [true, false] {
        let r = merged_chunk(l, strings);
        let (c, _) = l
            .layout
            .placement
            .chunks
            .iter()
            .find(|(_, c)| c.offset == r.start && matches!(c.kind, ChunkKind::Merged(_)))
            .unwrap();
        let frags = mortar_image::link::clip_frags_for_test(l, c, 0..r.end - r.start)
            .expect("the fragments");
        let (f, p) = frags[0];
        let at = r.start + p.offset;
        let res = refill_clip_plan_with(
            l,
            std::slice::from_ref(&r),
            |m| {
                if let Some(x) = m.merged.get_mut(&c) {
                    x.frags.retain(|g| g.frag != f);
                }
            },
            |_| {},
        );
        let want = format!("image: clip byte {at:#x} written by no row");
        match res {
            Err(Ok(ClipError::Internal(e))) => {
                assert!(format!("{e:?}").contains(&want), "{arch:?}: {e:?}");
            }
            other => panic!("{arch:?}: fragment {} dropped: {other:?}", f.0),
        }
        eprintln!(
            "[clip-fill] {arch:?} merged (strings {strings}, {} fragments): fragment {} \
             dropped: `{want}`",
            frags.len(),
            f.0
        );
    }
}

/// The two 64-bit targets' C++ drivers.
const CXX: [(Arch, &str); 2] = [(Arch::Aarch64, "g++"), (Arch::X86_64, "x86_64-linux-gnu-g++")];

/// A C++ program with many functions and data of several alignments in
/// their own sections.
const CXX_PROGRAM: &[support::Source] = &[support::Source {
    name: "m.cc",
    text: r#"#include <cstdio>
#include <map>
#include <string>
#include <vector>
struct alignas(32) V { double d[4]; };
V table[3] = {{{1, 2, 3, 4}}, {{5, 6, 7, 8}}, {{9, 10, 11, 12}}};
char small1 = 1;
short small2 = 2;
long counters[17];
const char *const names[] = {"alpha", "beta", "gamma"};
template <int N> __attribute__((noinline)) int f(int x) { return x * N + (int)table[N % 3].d[N % 4]; }
int main(int argc, char **argv) {
    std::map<std::string, std::vector<int>> m;
    m[argv[0]].push_back(argc);
    counters[argc % 17] = small1 + small2;
    std::printf("%d %zu %s\n", f<1>(argc) + f<2>(argc) + f<3>(argc) + f<5>(argc) + f<7>(argc),
                m.size(), names[argc % 3]);
    return 0;
}
"#,
}];

/// `go` over the static C++ link of [`CXX_PROGRAM`] for `arch`, or nothing
/// without its driver.
fn with_cxx_link(arch: Arch, cxx: &str, go: impl FnOnce(&Link<'_, '_>)) {
    let s = Scratch::new(&format!("clip-cxx-{arch:?}"));
    let flags = ["-O1", "-ffunction-sections", "-fdata-sections"];
    let Some(b) = support::build_by(&s, cxx, &flags, CXX_PROGRAM) else {
        assert!(!tools_required(), "{cxx} is required");
        return;
    };
    let args: Vec<OsString> = b.args.iter().map(OsString::from).collect();
    with_link_args(&b.dir, &args, go);
}

/// Every in-file `Body` chunk with bytes: its id and file range.
fn bodies(l: &Link<'_, '_>) -> Vec<(mortar_core::ChunkId, Range<u64>)> {
    let mut v: Vec<_> = l
        .layout
        .placement
        .chunks
        .iter()
        .filter(|(_, c)| matches!(c.kind, ChunkKind::Body(_)) && c.in_file() && c.size > 0)
        .map(|(id, c)| (id, c.offset..c.offset + c.size))
        .collect();
    v.sort_by_key(|b| b.1.start);
    v
}

/// The whole plan's `Input` units over the file range `r`: each section
/// once, with the range of its rows, in offset order.
fn units(whole: &Plan, r: &Range<u64>) -> Vec<(mortar_core::SectionId, Range<u64>)> {
    let mut out: Vec<(mortar_core::SectionId, Range<u64>)> = Vec::new();
    for row in &whole.rows[whole.first_row(r.start)..] {
        if row.offset >= r.end {
            break;
        }
        let Unit::Input(i) = row.unit else { continue };
        if row.end() <= r.start {
            continue;
        }
        let s = whole.inputs[i as usize].section;
        match out.last_mut() {
            Some(l) if l.0 == s => l.1.end = row.end(),
            _ => out.push((s, row.offset..row.end())),
        }
    }
    out
}

/// The clip's sections of chunk `c` (at file offset `base`) over `r`, as
/// [`units`] gives them.
fn listed(got: &[Placed], base: u64) -> Vec<(mortar_core::SectionId, Range<u64>)> {
    got.iter().map(|p| (p.section, base + p.offset..base + p.offset + p.size)).collect()
}

/// A small deterministic generator (the sub-ranges).
fn next(x: &mut u64) -> u64 {
    *x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
    *x >> 33
}

/// The `[clip-sections]` comparison over every body chunk of `l`: the
/// chunk whole and 24 sub-ranges; then one clip over every body chunk at
/// once (the gap rule on every member) fills `refill_ranges`' bytes.
fn sections_agree(l: &Link<'_, '_>, label: &str) {
    let headers = std::slice::from_ref(&(0..0x40));
    let whole = refill_clip_plan(l, headers, |_| {}).expect("the headers' clip").whole;
    let mut queries = Vec::new();
    let mut seed = 0x255c_2b00_u64;
    for (c, r) in bodies(l) {
        let size = r.end - r.start;
        queries.push((c, 0..size));
        for _ in 0..24 {
            let a = next(&mut seed) % size;
            let b = a + 1 + next(&mut seed) % (size - a).min(0x2000);
            queries.push((c, a..b));
        }
    }
    let got = clip_sections_for_test(l, &queries).expect("the member index");
    let (chunks, mut secs) = (bodies(l).len(), 0);
    for (k, ((c, q), g)) in queries.iter().zip(got).enumerate() {
        let base = l.layout.placement.chunks[*c].offset;
        let g = g.unwrap_or_else(|e| panic!("{label}: chunk {} {q:x?}: {e:?}", c.0));
        let want = units(&whole, &(base + q.start..base + q.end));
        assert_eq!(listed(&g, base), want, "{label}: chunk {} {q:x?}", c.0);
        if k % 25 == 0 {
            secs += want.len();
        }
    }
    let mut clips: Vec<Range<u64>> = bodies(l).into_iter().map(|b| b.1).collect();
    let merged: Vec<Range<u64>> = l
        .layout
        .placement
        .chunks
        .iter()
        .filter(|(_, c)| matches!(c.kind, ChunkKind::Merged(_)) && c.in_file() && c.size > 0)
        .map(|(_, c)| c.offset..c.offset + c.size)
        .collect();
    let n_merged = merged.len();
    clips.extend(merged);
    clips.sort_by_key(|r| r.start);
    let fill = loop {
        match refill_clip_plan(l, &clips, |_| {}) {
            Ok(f) => break f,
            Err(Ok(ClipError::NotYet(NotYet::EhFrame, c))) => {
                eprintln!("[clip-sections] {label}: chunk {} is NotYet(EhFrame)", c.0);
                let at = l.layout.placement.chunks[c].offset;
                clips.retain(|r| r.start != at);
            }
            Err(e) => panic!("{label}: every body chunk: {e:?}"),
        }
    };
    let want = refill_ranges_for_test(l, &clips)
        .unwrap_or_else(|e| panic!("{label}: {:?}", e.diagnostics));
    assert_eq!(fill.jobs, want.jobs, "{label}: every body chunk's bytes");
    eprintln!(
        "[clip-sections] {label}: {chunks} body chunks, {secs} sections, {} queries agree; \
         {} chunks ({n_merged} merged) filled through the gap rule",
        queries.len(),
        clips.len()
    );
}

// spec: ruling "After 255c2a" 2(i) (the section list of a clip comes from
// ONE function over the rows, with its own differential test against the
// whole plan on every chunk of real links). For every body chunk of U-6's
// fixtures and of a static C++ link with `-ffunction-sections
// -fdata-sections` (libc and libstdc++ members included), `sections_in`
// over the chunk's whole range and over 24 random sub-ranges is the whole
// plan's `Input` units there, section for section with their ranges; and a
// clip over every body chunk passes the gap rule and fills the whole
// plan's bytes.
#[test]
fn clip_sections_equal_the_whole_plan() {
    for arch in ARCHES {
        for pad in [0, 0x3000] {
            let Some(s) = fixture(arch, &format!("clip-sec-{pad:x}"), pad) else { continue };
            with_link(&s, |l| sections_agree(l, &format!("{arch:?} U-6 pad {pad:#x}")));
        }
    }
    for (arch, cxx) in CXX {
        with_cxx_link(arch, cxx, |l| sections_agree(l, &format!("{arch:?} C++")));
    }
}

/// `x` rounded up to `a`.
fn align_up(x: u64, a: u64) -> u64 {
    x.div_ceil(a.max(1)) * a.max(1)
}

/// Whether the gap rule sees member `i` of `list` (a chunk of `size`
/// bytes with `holes`) dropped: past the holes that start where the
/// previous member ends, the gap it leaves is not the next member's
/// alignment padding exactly (or, for the last, not empty).
fn detectable(list: &[Placed], holes: &[(u64, u64)], i: usize, size: u64) -> bool {
    let mut p = if i > 0 { list[i - 1].offset + list[i - 1].size } else { 0 };
    while let Some(h) = holes.iter().find(|h| h.0 == p && h.1 > p) {
        p = h.1;
    }
    match list.get(i + 1) {
        Some(n) => align_up(p, n.align) != n.offset,
        None => p != size,
    }
}

/// The clip over member `d` of chunk `c` with `d` dropped from the member
/// index.
fn dropped(
    l: &Link<'_, '_>,
    c: mortar_core::ChunkId,
    d: &Placed,
) -> Result<mortar_image::link::ClipFill, Result<ClipError, mortar_image::fill::Failed>> {
    let base = l.layout.placement.chunks[c].offset;
    let clip = base + d.offset..base + d.offset + d.size;
    refill_clip_plan_with(
        l,
        &[clip],
        |m| {
            if let Some(v) = m.by_chunk.get_mut(&c) {
                v.retain(|x| x.section != d.section);
            }
        },
        |_| {},
    )
}

/// The residue's sources: a 1-byte member, then a 4-byte one, then one
/// aligned to 64, in a code and in a data section.
const RESIDUE: &str = "\t.section .text.a,\"ax\",%progbits\n\t.globl _start\n_start:\n\tret\n\
     \t.section .text.b,\"ax\",%progbits\n\tret\n\
     \t.section .text.c,\"ax\",%progbits\n\t.p2align 6\n\tret\n\
     \t.section .data.a,\"aw\",%progbits\n\t.byte 1\n\
     \t.section .data.b,\"aw\",%progbits\n\t.byte 2, 3, 4, 5\n\
     \t.section .data.c,\"aw\",%progbits\n\t.p2align 6\n\t.quad 9\n\
     \t.section .note.GNU-stack,\"\",%progbits\n";

// spec: ruling "After 255c2a" 2(ii)-(iii) (a derived gap in a body chunk
// is legal only if the rows explain it: alignment padding, a region's
// slack or a hole; the mutation 255c2a could not catch). In the C++ link,
// one member dropped from the member index (the test's hook) in the exec
// `.text` chunk and in a non-exec chunk is refused by the gap rule at the
// dropped member's first byte (`image: clip byte {offset:#x} written by
// no row`), before any byte is filled. The residue: a member that lies
// wholly inside the alignment padding the next member needs anyway
// (smaller than that alignment) is not seen: in RESIDUE's `.text.b` and
// `.data.b` the clip plan fills the trap or zero pattern where the bytes
// were, and only the comparison with the whole plan sees it.
#[test]
fn clip_plan_refuses_a_missing_section() {
    for (arch, cxx) in CXX {
        with_cxx_link(arch, cxx, |l| {
            let label = format!("{arch:?} C++");
            let chunks = &l.layout.placement.chunks;
            let all: Vec<_> = bodies(l).into_iter().map(|(c, r)| (c, 0..r.end - r.start)).collect();
            let lists = clip_sections_for_test(l, &all).expect("the member index");
            let (mut seen, mut residue, mut largest) = (0usize, 0usize, 0u64);
            let mut tried = [false, false];
            for ((c, q), list) in all.iter().zip(lists) {
                let list = list.expect("the members");
                let exec = chunks[*c].flags & 4 != 0;
                let holes: Vec<(u64, u64)> = l
                    .layout
                    .placement
                    .holes
                    .iter()
                    .filter(|h| h.chunk == *c)
                    .map(|h| (h.offset, h.offset + h.size))
                    .collect();
                for i in 0..list.len() {
                    if !detectable(&list, &holes, i, q.end) {
                        residue += 1;
                        largest = largest.max(list[i].size);
                        continue;
                    }
                    seen += 1;
                    if tried[usize::from(exec)] || list.len() < 3 || list[i].size < 8 {
                        continue;
                    }
                    tried[usize::from(exec)] = true;
                    let d = list[i];
                    let at = chunks[*c].offset + d.offset;
                    let want = format!("image: clip byte {at:#x} written by no row");
                    match dropped(l, *c, &d) {
                        Err(Ok(ClipError::Internal(e))) => {
                            assert!(format!("{e:?}").contains(&want), "{label}: {e:?}");
                        }
                        other => panic!(
                            "{label}: section {} dropped from chunk {} ({:x?}): prev {:x?} next {:x?}: {}",
                            d.section.0,
                            c.0,
                            d,
                            list.get(i.wrapping_sub(1)),
                            list.get(i + 1),
                            other.is_ok()
                        ),
                    }
                    eprintln!(
                        "[clip-missing] {label}: exec {exec}: section {} ({:#x} bytes) dropped \
                         from chunk {}: `{want}`",
                        d.section.0, d.size, c.0
                    );
                }
            }
            assert_eq!(tried, [true, true], "{label}: an exec and a non-exec drop");
            eprintln!(
                "[clip-missing] {label}: of {} members, {seen} drops refused by the gap rule, \
                 {residue} inside the next one's alignment padding (largest {largest:#x} bytes)",
                seen + residue
            );
        });
    }
    for arch in ARCHES {
        residue_is_not_seen(arch);
    }
}

/// The residue of the gap rule on RESIDUE's `.text.b` and `.data.b`:
/// dropped, the clip plan is built and its bytes differ from the whole
/// plan's.
fn residue_is_not_seen(arch: Arch) {
    let Some(gcc) = arch.tool("gcc") else {
        assert!(!tools_required(), "{} is required", arch.gnu("gcc"));
        return;
    };
    let dir = Scratch::new("clip-residue");
    let src = dir.write("r.s", RESIDUE);
    let obj = dir.join("r.o");
    let mut cmd = Command::new(&gcc);
    cmd.args(["-c".as_ref(), src.as_os_str(), "-o".as_ref(), obj.as_os_str()]);
    let out = run_with(&mut cmd, b"", Duration::from_mins(1));
    assert!(out.success(), "gcc: {}", out.stderr_text());
    let args: Vec<OsString> = ["r.o", "-static", "-o", "out"].iter().map(OsString::from).collect();
    with_link_args(dir.path(), &args, |l| {
        for name in [&b".text.b"[..], b".data.b"] {
            let r = placed(l, "r.o", name);
            let (c, off) = l
                .layout
                .placement
                .chunks
                .iter()
                .find(|(_, c)| {
                    matches!(c.kind, ChunkKind::Body(_))
                        && c.in_file()
                        && c.offset <= r.start
                        && r.end <= c.offset + c.size
                })
                .map(|(id, c)| (id, r.start - c.offset))
                .expect("its chunk");
            let list = clip_sections_for_test(l, &[(c, off..off + 1)]).unwrap();
            let d = *list[0].as_ref().expect("the member").first().expect("one");
            let got = dropped(l, c, &d)
                .unwrap_or_else(|e| panic!("{arch:?}: the residue was refused: {e:?}"));
            let whole = refill_ranges_for_test(l, std::slice::from_ref(&r)).unwrap();
            assert_ne!(got.jobs, whole.jobs, "{arch:?}: a dropped member changes bytes");
            let n = got.jobs[0].1.iter().zip(&whole.jobs[0].1).filter(|(a, b)| a != b).count();
            eprintln!(
                "[clip-residue] {arch:?} {}: {:#x} bytes dropped inside the next member's \
                 alignment padding: the gap rule passes, {n} bytes differ from the whole plan",
                String::from_utf8_lossy(name),
                d.size
            );
        }
    });
}

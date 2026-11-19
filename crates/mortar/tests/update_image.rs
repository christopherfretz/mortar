//! U-6's refill on linked fixtures (`spec/incr.md` IN-75; WP-218): an
//! incremental-mode full link is the previous output; the update's output
//! `P_U` is the image of a second front over the edited inputs (the first
//! link's layout replayed for a same-size edit, the computed layout for a
//! growth); `mortar_image::update::refill` is given the delta and the
//! previous file, and the previous bytes patched by its ranges must be
//! `P_U`'s bytes exactly: every byte that differs lies in a range (the
//! invariant), and the ranges hold the right bytes.
//!
//! The fixtures are AArch64 and x86-64 assembly (the targets completion is
//! judged on), three files: `a.o` (`_start`, a GOT load of `b.o`'s data, a
//! call into `c.o`, CFI), `b.o` (data and merged strings, no relocation),
//! `c.o` (the edited file: a function with CFI).

use std::ffi::OsString;
use std::fs::File;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use mortar::front::{Env, Front, front};
use mortar::timing::Timer;
use mortar_core::incr::{
    DirtyRange, EhLayout, GrowthStep, IncrReason, LayoutDelta, MemoryLayout, RefillSet, RefillUnit,
    StoredLayout, SymtabEntry, SymtabPlan, SymtabTables,
};
use mortar_core::layout::Layout;
use mortar_core::par::Pool;
use mortar_core::{FileId, SectionId};
use mortar_diag::{Collector, Policy};
use mortar_image::update::{
    Counts, Refill, RefillError, Shift, move_eh_range, output_size, patch_eh, refill, walk_eh,
};
use mortar_input::store::{Store, StorePolicy};
use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, run_with, tools_required};

/// `(a.s, b.s, c.s with the function body's immediate)` of an arch.
fn sources(arch: Arch, imm: u32, pad: usize) -> [String; 3] {
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
             \t.section .note.GNU-stack,\"\",@progbits\n"
                .to_owned(),
            format!(
                "\t.text\n\t.globl f\n\t.type f, @function\nf:\n\t.cfi_startproc\n\tmovl ${imm}, %eax\n\
                 \tjmp 1f\n\t{pad}\n1:\tret\n\t.cfi_endproc\n\t.section .note.GNU-stack,\"\",@progbits\n"
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
             \t.section .note.GNU-stack,\"\",%progbits\n"
                .to_owned(),
            format!(
                "\t.text\n\t.globl f\n\t.type f, %function\nf:\n\t.cfi_startproc\n\tmov x0, #{imm}\n\
                 \tb 1f\n\t{pad}\n1:\tret\n\t.cfi_endproc\n\t.section .note.GNU-stack,\"\",%progbits\n"
            ),
        ],
    }
}

/// The fixture's scratch directory and objects, or `None` without the
/// arch's assembler.
struct Fixture {
    scratch: Scratch,
    arch: Arch,
    /// The leaves of each output written (`LEAVES`, IN-18e).
    leaves: std::cell::RefCell<std::collections::BTreeMap<String, Vec<[u8; 32]>>>,
}

impl Fixture {
    fn new(arch: Arch, label: &str) -> Option<Self> {
        if arch.tool("gcc").is_none() {
            assert!(!tools_required(), "{} is required", arch.gnu("gcc"));
            return None;
        }
        let f = Self { scratch: Scratch::new(label), arch, leaves: std::cell::RefCell::default() };
        let [a, b, c] = sources(arch, 42, 0);
        f.assemble("a", &a);
        f.assemble("b", &b);
        f.assemble("c", &c);
        Some(f)
    }

    /// As [`Fixture::new`], with 4 MiB of data in b.o: an output of five
    /// leaves (I-72's 1 MiB blocks), the text in the first.
    fn big(arch: Arch, label: &str) -> Option<Self> {
        let f = Self::new(arch, label)?;
        let [_, b, _] = sources(arch, 42, 0);
        f.assemble("b", &(b + "\t.data\n\t.skip 0x400000, 1\n"));
        Some(f)
    }

    fn leaves_of(&self, out: &str) -> Vec<[u8; 32]> {
        self.leaves.borrow()[out].clone()
    }

    fn assemble(&self, name: &str, text: &str) {
        let src = self.scratch.write(&format!("{name}.s"), text);
        let obj = self.scratch.join(format!("{name}.o"));
        let gcc = self.arch.tool("gcc").unwrap();
        let mut c = Command::new(gcc);
        c.args(["-c".as_ref(), src.as_os_str(), "-o".as_ref(), obj.as_os_str()]);
        let o = run_with(&mut c, b"", Duration::from_mins(1));
        assert!(o.success(), "gcc: {}", o.stderr_text());
    }

    fn args(out: &str) -> Vec<OsString> {
        ["a.o", "b.o", "c.o", "-static", "--eh-frame-hdr", "--build-id=sha1", "-o", out]
            .iter()
            .map(OsString::from)
            .collect()
    }

    fn path(&self, name: &str) -> std::path::PathBuf {
        self.scratch.join(name)
    }

    /// `go` with the front of the link to `out` (incremental mode), its
    /// layout replayed from `replay` when given.
    fn with_front<T>(
        &self,
        out: &str,
        replay: Option<&Layout>,
        go: impl FnOnce(&mut Front<'_>, &Store) -> T,
    ) -> T {
        let parsed = mortar_cli::parse(&Self::args(out));
        assert!(!parsed.failed());
        let store = Store::new(StorePolicy::from_env());
        let diag = Collector::new(Policy::default());
        let hook = |_: &mortar_incr::capture::Link<'_, '_>, _: &Layout| {
            Ok::<Layout, String>(replay.cloned().expect("a replay"))
        };
        let env = Env {
            cwd: self.scratch.path(),
            pool: Pool::new(2),
            replay: replay.map(|_| &hook as &mortar::front::Replay<'_>),
            slack: true,
            state5: false,
        };
        let mut f = front(parsed.plan, env, &store, &diag, &mut Timer::new(false))
            .unwrap_or_else(|| panic!("front: {:?}", diag.finish()));
        f.keep_leaves = true;
        go(&mut f, &store)
    }

    /// The full link to `out`; its layout.
    fn write(&self, out: &str, replay: Option<&Layout>) -> Layout {
        self.with_front(out, replay, |f, store| {
            let w = mortar::image::write(f, store, &self.path(out), &|_| {}, &mut |_, _| {})
                .unwrap_or_else(|e| panic!("write: {:?}", e.diagnostics));
            let (_, leaves) = w.leaves.expect("an incremental-mode link keeps its leaves");
            let leaves = leaves
                .iter()
                .map(|l| {
                    let mut d = [0u8; 32];
                    d[..l.as_bytes().len()].copy_from_slice(l.as_bytes());
                    d
                })
                .collect();
            self.leaves.borrow_mut().insert(out.to_owned(), leaves);
            f.layout.clone()
        })
    }
}

/// The file id of the object named `name` in `f`.
fn file_of(f: &Front<'_>, name: &str) -> FileId {
    f.inputs
        .link
        .opened
        .iter()
        .find_map(|o| match o.kind {
            mortar_core::inputs::OpenedKind::Object(id) if o.path.ends_with(name.as_bytes()) => {
                Some(id)
            }
            _ => None,
        })
        .unwrap()
}

/// Every section of the files `names` as a refill unit.
fn units(f: &Front<'_>, names: &[&str]) -> Vec<RefillUnit> {
    let mut u: Vec<RefillUnit> = names
        .iter()
        .flat_map(|n| f.resolution.section_base[file_of(f, n)].iter())
        .map(|section: SectionId| RefillUnit { section, reasons: 1 })
        .collect();
    u.sort();
    u
}

/// The previous output patched by the ranges.
fn patched(prev: &[u8], ranges: &[DirtyRange], bytes: &[u8], size: u64) -> Vec<u8> {
    let mut out = prev.to_vec();
    out.resize(size as usize, 0);
    let mut at = 0usize;
    for r in ranges {
        let (o, n) = (r.offset as usize, r.len as usize);
        out[o..o + n].copy_from_slice(&bytes[at..at + n]);
        at += n;
    }
    assert_eq!(at, bytes.len(), "the bytes are the ranges' concatenation");
    out
}

/// IN-75's invariant: sorted, disjoint, every differing byte covered; and
/// the patched output is `want` exactly.
fn judge(what: &str, prev: &[u8], want: &[u8], ranges: &[DirtyRange], bytes: &[u8]) {
    for w in ranges.windows(2) {
        assert!(w[0].offset + w[0].len <= w[1].offset, "{what}: ranges not sorted/disjoint");
    }
    if let Some(dir) = std::env::var_os("MORTAR_U6_DUMP") {
        let d = Path::new(&dir).join(what.replace(' ', "-"));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("prev"), prev).unwrap();
        std::fs::write(d.join("want"), want).unwrap();
        std::fs::write(d.join("got"), patched(prev, ranges, bytes, want.len() as u64)).unwrap();
        std::fs::write(d.join("ranges"), format!("{ranges:x?}\n")).unwrap();
    }
    let covered = |at: u64| ranges.iter().any(|r| r.offset <= at && at < r.offset + r.len);
    for (i, &b) in want.iter().enumerate() {
        if prev.get(i).copied().unwrap_or(0) != b {
            assert!(covered(i as u64), "{what}: byte {i:#x} differs and is in no range");
        }
    }
    let got = patched(prev, ranges, bytes, want.len() as u64);
    if let Some(i) = got.iter().zip(want).position(|(a, b)| a != b) {
        panic!("{what}: the patched output differs from P_U at {i:#x}");
    }
}

/// What a refill gave: ranges, bytes, counts, the output's size, leaves.
type Got = (Vec<DirtyRange>, Vec<u8>, Counts, u64, Vec<[u8; 32]>);

/// `refill` against the output `prev` of `fx`, with its stored leaves.
#[allow(clippy::too_many_arguments)]
fn run_refill(
    f: &Front<'_>,
    store: &Store,
    delta: &LayoutDelta,
    plan: &SymtabPlan,
    set: &RefillSet,
    fx: &Fixture,
    prev: &str,
) -> Result<Got, RefillError> {
    run_refill_with(f, store, delta, plan, set, fx, prev, &fx.leaves_of(prev))
}

#[allow(clippy::too_many_arguments)]
fn run_refill_with(
    f: &Front<'_>,
    store: &Store,
    delta: &LayoutDelta,
    plan: &SymtabPlan,
    set: &RefillSet,
    fx: &Fixture,
    prev: &str,
    leaves: &[[u8; 32]],
) -> Result<Got, RefillError> {
    let file = File::open(fx.path(prev)).unwrap();
    // No stored row is read unless a whole `.eh_frame` rewrite moves a
    // range (`eh_range_of`): an empty stored layout.
    let rows = MemoryLayout::default();
    let stored = StoredLayout::new(&rows);
    mortar::image::with_link(f, store, &|_| {}, |l| {
        refill(l, delta, plan, set, &stored, &file, leaves).map(|u| {
            clips_equal_windows(l, delta, &u);
            (u.ranges, u.bytes, u.counts, output_size(l), u.leaves)
        })
    })
    .unwrap()
}

std::thread_local! {
    /// The refills [`clips_equal_windows`] compared on this thread.
    static WINDOW_CHECKS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// WP-255a (`refill_clips_equal_windows`): the bytes of every range the
/// refill returns, which the image filled range by range (I-95), equal
/// those of the whole 1 MiB windows that hold them, as the image phase's
/// window path fills them (`mortar_image::link::refill`, the WP-061
/// floor's own, unchanged: not a second writer, the fill of the same
/// plan by the same `fill_range` over whole jobs). Aside: the build id's
/// descriptor (zeros in a fill, the update's id in the range) and a
/// retired slot's words (zeros the update writes over the fill, IN-72
/// item 7, asserted by its test).
fn clips_equal_windows(l: &mortar_image::link::Link<'_, '_>, delta: &LayoutDelta, u: &Refill) {
    let w = mortar_digest::IMAGE_BLOCK;
    let mut windows: Vec<u64> =
        u.ranges.iter().flat_map(|r| r.offset / w..(r.offset + r.len).div_ceil(w)).collect();
    windows.sort_unstable();
    windows.dedup();
    let whole = mortar_image::link::refill(l, Path::new(""), &windows)
        .unwrap_or_else(|e| panic!("the window fill: {:?}", e.diagnostics));
    let pl = &l.layout.placement;
    let mut aside: Vec<std::ops::Range<u64>> = whole.sealed.iter().cloned().collect();
    for h in delta.holes.iter().filter(|h| h.kind == mortar_core::layout::HoleKind::DeadSlot) {
        let c = pl.chunks[h.chunk];
        aside.push(c.offset + h.offset..c.offset + h.offset + h.size);
    }
    // The window fill laid out at its file offsets.
    let mut image = vec![None; whole.file_size as usize];
    for (j, b) in &whole.jobs {
        for (i, &x) in b.iter().enumerate() {
            image[j.start as usize + i] = Some(x);
        }
    }
    let mut at = 0usize;
    for r in &u.ranges {
        for i in 0..r.len {
            let o = r.offset + i;
            let got = u.bytes[at + i as usize];
            if aside.iter().any(|a| a.contains(&o)) || o >= whole.file_size {
                continue;
            }
            assert_eq!(
                Some(got),
                image[o as usize],
                "the range fill and the window fill differ at {o:#x} (range {r:x?})"
            );
        }
        at += r.len as usize;
    }
    // The fill is O(range), not O(window): never more than the windows
    // (an empty delta's header ranges are filled, and then dropped).
    let windows_filled: u64 = whole.jobs.iter().map(|j| j.1.len() as u64).sum();
    assert!(
        u.ranges.is_empty() || u.counts.filled <= windows_filled,
        "{:?} vs {windows_filled}",
        u.counts
    );
    eprintln!("[u6-clips] filled {} bytes, the windows {windows_filled}", u.counts.filled);
    WINDOW_CHECKS.with(|c| c.set(c.get() + 1));
}

const ARCHES: [Arch; 2] = [Arch::Aarch64, Arch::X86_64];

// spec: incr.md IN-75 (the dirty ranges cover every differing byte; an
// empty delta writes no byte; the build id is the tree over the whole new
// output), IN-73 (the changed file's units refilled): a same-size edit of
// c.o, the first link's layout replayed.
#[test]
fn dirty_ranges_cover_every_differing_byte() {
    for arch in ARCHES {
        let Some(fx) = Fixture::big(arch, "u6-cover") else { continue };
        let l0 = fx.write("prev", None);
        let prev = std::fs::read(fx.path("prev")).unwrap();
        // An empty delta: no byte.
        fx.with_front("prev", Some(&l0), |f, store| {
            let (r, b, c, size, leaves) = run_refill(
                f,
                store,
                &LayoutDelta::default(),
                &SymtabPlan::default(),
                &RefillSet::default(),
                &fx,
                "prev",
            )
            .unwrap();
            assert_eq!(size, prev.len() as u64, "{arch:?}: output_size");
            assert!(r.is_empty() && b.is_empty(), "{arch:?}: an empty delta wrote {r:?}");
            assert_eq!(c.copied, 0);
            assert_eq!(leaves, fx.leaves_of("prev"), "{arch:?}: the stored leaves kept");
            assert_eq!(c.leaves_hashed, 1, "{arch:?}: the note's block only");
            // A stored leaf vector of the wrong length: `state-corrupt`,
            // before any byte is used.
            let mut short = fx.leaves_of("prev");
            short.pop();
            let r = run_refill_with(
                f,
                store,
                &LayoutDelta::default(),
                &SymtabPlan::default(),
                &RefillSet::default(),
                &fx,
                "prev",
                &short,
            );
            assert!(
                matches!(r, Err(RefillError::Fallback(IncrReason::StateCorrupt(_)))),
                "{arch:?}: {:?}",
                r.map(|x| x.2)
            );
        });
        let [_, _, c] = sources(arch, 43, 0);
        fx.assemble("c", &c);
        let want = {
            fx.write("want", Some(&l0));
            std::fs::read(fx.path("want")).unwrap()
        };
        assert_ne!(prev, want);
        fx.with_front("want", Some(&l0), |f, store| {
            let c_o = file_of(f, "c.o");
            let delta = LayoutDelta { files: vec![c_o], ..LayoutDelta::default() };
            let set = RefillSet { units: units(f, &["c.o"]), ..RefillSet::default() };
            let (r, b, counts, size, leaves) =
                run_refill(f, store, &delta, &SymtabPlan::default(), &set, &fx, "prev").unwrap();
            assert_eq!(size, want.len() as u64);
            judge(&format!("{arch:?} same-size edit"), &prev, &want, &r, &b);
            // The new leaves are the full link's (I-76, IN-75).
            assert_eq!(leaves, fx.leaves_of("want"), "{arch:?}: the new leaves");
            assert!(leaves.len() >= 5, "{arch:?}: {} leaves", leaves.len());
            // O(change): the edited instruction's range and the build id;
            // nothing copied, no `.eh_frame` walk (nothing moved).
            eprintln!("[u6] {arch:?} same-size edit: {counts:?}");
            assert!(counts.ranges <= 3, "{arch:?}: {r:?}");
            assert!(counts.dirty <= 64, "{arch:?}: {} dirty bytes", counts.dirty);
            assert_eq!((counts.copied, counts.eh_records), (0, 0));
            // The tree reads the dirty blocks only: the note and the text
            // share the first.
            assert_eq!(counts.leaves_hashed, 1, "{arch:?}: {counts:?}");
            assert_eq!(counts.build_id_read, mortar_digest::IMAGE_BLOCK, "{arch:?}");
        });
    }
}

// spec: incr.md IN-75 (inputs are fstat-checked before their first byte is
// used: a file rewritten after the front read it is `input-changed`).
#[test]
fn input_rewritten_after_u4_is_input_changed() {
    for arch in ARCHES {
        // The staged store keeps a large input open (input.md §11.4) and
        // rechecks it; a file read whole is held whole (its bytes are the
        // ones the front read) and needs no recheck.
        if StorePolicy::from_env() == StorePolicy::Resident {
            return;
        }
        let Some(fx) = Fixture::new(arch, "u6-changed") else { continue };
        let [_, _, c] = sources(arch, 42, 0x20000);
        fx.assemble("c", &c);
        let l0 = fx.write("prev", None);
        fx.with_front("prev", Some(&l0), |f, store| {
            // After the front (U-4 ran on what it read): c.o rewritten in
            // place (the store holds the file it read; a file replaced by a
            // new inode is not the one the link reads, input.md §11.5).
            let mut w = std::fs::OpenOptions::new().append(true).open(fx.path("c.o")).unwrap();
            std::io::Write::write_all(&mut w, b"\0").unwrap();
            drop(w);
            let set = RefillSet { units: units(f, &["c.o"]), ..RefillSet::default() };
            let r = run_refill(
                f,
                store,
                &LayoutDelta::default(),
                &SymtabPlan::default(),
                &set,
                &fx,
                "prev",
            );
            match r {
                Err(RefillError::Fallback(IncrReason::InputChanged(d))) => {
                    assert!(d.contains("c.o"), "{arch:?}: {d}");
                }
                other => panic!("{arch:?}: want input-changed, got {:?}", other.map(|x| x.2)),
            }
        });
    }
}

// spec: U-5's Q2-5, incr.md IN-49a (a shrinking global area: the entries
// [new end, old end) are zeros; WP-218 resume ruling 2), and the guard of
// that zeroing: c.o loses an absolute global and the full link's table is
// one entry shorter. Where the previous table's tail is the new output's
// `.strtab` (no slack after the table), zeroing it is refused by name,
// never written over another section; where IN-49's slack follows the
// table (an incremental-mode output since U-5), the tail is inside the
// table's own reserve, the zeros are written and the patched output is the
// full link's. The zeros themselves:
// `symtab_move_inside_remap_is_remapped_and_shrink_zeroed` (hand-made).
#[test]
fn symtab_shrink_zeroes_its_own_tail_only() {
    for arch in ARCHES {
        let Some(fx) = Fixture::new(arch, "u6-shrink") else { continue };
        let [_, _, c_s] = sources(arch, 42, 0);
        fx.assemble("c", &(c_s.clone() + "\t.globl zz_gone\n\t.set zz_gone, 0x1234\n"));
        fx.write("prev", None);
        let prev = std::fs::read(fx.path("prev")).unwrap();
        fx.assemble("c", &c_s);
        fx.write("want", None);
        let want = std::fs::read(fx.path("want")).unwrap();
        let (o, n) = (section_range(&prev, b".symtab"), section_range(&want, b".symtab"));
        assert_eq!(o.start, n.start, "{arch:?}: the table stays in place");
        assert!(o.end > n.end, "{arch:?}: the table shrinks: {o:?} -> {n:?}");
        let strtab = section_range(&want, b".strtab");
        let reserved = strtab.start >= o.end;
        eprintln!("[u6] {arch:?} shrink: old tail {:?}, .strtab {strtab:?}", n.end..o.end);
        fx.with_front("want", None, |f, store| {
            let c_o = file_of(f, "c.o");
            let delta = LayoutDelta { files: vec![c_o], ..LayoutDelta::default() };
            let set = RefillSet { units: units(f, &["c.o"]), ..RefillSet::default() };
            let plan = whole_symtab_plan(&want);
            match run_refill(f, store, &delta, &plan, &set, &fx, "prev") {
                Ok((r, b, _, size, _)) if reserved => {
                    assert_eq!(size, want.len() as u64, "{arch:?}: output_size");
                    judge(&format!("{arch:?} symtab shrink"), &prev, &want, &r, &b);
                    assert!(
                        want[n.end..o.end].iter().all(|&x| x == 0),
                        "{arch:?}: the full link's reserve is zeros"
                    );
                    assert!(
                        prev[n.end..o.end].iter().any(|&x| x != 0),
                        "{arch:?}: the old tail held an entry"
                    );
                }
                Err(RefillError::Fallback(IncrReason::StateCorrupt(d))) if !reserved => {
                    assert!(d.contains("reaches another section"), "{arch:?}: {d}");
                }
                other => panic!(
                    "{arch:?}: reserve after the table {reserved}, got {:?}",
                    other.map(|x| x.2)
                ),
            }
        });
    }
}

/// The `.symtab` entries and `.strtab` of an ELF file, as a hand-made U-5
/// plan that rewrites both whole (until U-5's plan lands, WP-218).
fn whole_symtab_plan(elf: &[u8]) -> SymtabPlan {
    let le = |at: usize, n: usize| -> u64 {
        elf[at..at + n].iter().rev().fold(0, |a, &x| a << 8 | u64::from(x))
    };
    let class64 = elf[4] == 2;
    let (shoff, shentsize, shnum) = if class64 {
        (le(0x28, 8) as usize, le(0x3a, 2) as usize, le(0x3c, 2) as usize)
    } else {
        (le(0x20, 4) as usize, le(0x2e, 2) as usize, le(0x30, 2) as usize)
    };
    let sh = |i: usize, f: usize, n: usize| le(shoff + i * shentsize + f, n);
    let (ty, off, size, link, info) = if class64 {
        ((4, 4), (24, 8), (32, 8), (40, 4), (44, 4))
    } else {
        ((4, 4), (16, 4), (20, 4), (24, 4), (28, 4))
    };
    let st = (0..shnum).find(|&i| sh(i, ty.0, ty.1) == 2).unwrap();
    let ss = sh(st, link.0, link.1) as usize;
    let (o, n) = (sh(st, off.0, off.1) as usize, sh(st, size.0, size.1) as usize);
    let es = if class64 { 24 } else { 16 };
    let mut plan = SymtabPlan::default();
    for i in 0..n / es {
        let e = o + i * es;
        let (name, info_, other, shndx, value, size_) = if class64 {
            (le(e, 4), le(e + 4, 1), le(e + 5, 1), le(e + 6, 2), le(e + 8, 8), le(e + 16, 8))
        } else {
            (le(e, 4), le(e + 12, 1), le(e + 13, 1), le(e + 14, 2), le(e + 4, 4), le(e + 8, 4))
        };
        plan.entries.push(SymtabEntry {
            index: i as u32,
            name: name as u32,
            info: info_ as u8,
            other: other as u8,
            shndx: shndx as u32,
            value,
            size: size_,
        });
    }
    let (so, sn) = (sh(ss, off.0, off.1) as usize, sh(ss, size.0, size.1) as usize);
    plan.strtab.push((0, elf[so..so + sn].to_vec()));
    plan.tables = SymtabTables {
        sh_info: sh(st, info.0, info.1) as u32,
        symtab_size: n as u64,
        strtab_size: sn as u64,
        ..SymtabTables::default()
    };
    plan
}

// spec: incr.md IN-72 item 3, IN-75 (a growth step: every later output
// section of its PT_LOAD moves; the header tables are rewritten whole;
// every moved byte is dirty, copied from the previous output), IN-34a (the
// unchanged records' FDE fields follow the code). c.o's `.text` grows past
// its region and the `.text` reserve: the full link of the edit is P_U.
#[test]
fn growth_step_rewrites_headers() {
    for arch in ARCHES {
        let Some(fx) = Fixture::new(arch, "u6-growth") else { continue };
        // b.o's data sized so that the previous output ends 0x800 bytes
        // short of a block's end and the grown one past it: the leaf count
        // changes (I-72).
        // The data's slack grows with it (IN-33): the size is near-linear
        // in the data's, found by secant steps.
        let target = (4u64 << 20) - 0x800;
        let size_with = |skip: u64| {
            let [_, b, _] = sources(arch, 42, 0);
            fx.assemble("b", &(b + &format!("\t.data\n\t.skip {skip}, 1\n")));
            fx.write("prev", None);
            std::fs::metadata(fx.path("prev")).unwrap().len()
        };
        let (mut s0, mut z0) = (0u64, size_with(0));
        let (mut s1, mut z1) = (target - z0, size_with(target - z0));
        for _ in 0..6 {
            if z1 <= target && target - z1 < 0x400 {
                break;
            }
            #[allow(clippy::cast_precision_loss)]
            let slope = (z1 - z0) as f64 / (s1 - s0) as f64;
            #[allow(clippy::cast_precision_loss)]
            let next = s1 as f64 + ((target - 0x200) as f64 - z1 as f64) / slope;
            (s0, z0) = (s1, z1);
            s1 = next as u64;
            z1 = size_with(s1);
        }
        let prev = std::fs::read(fx.path("prev")).unwrap();
        assert!((target - 0x400..=target).contains(&(prev.len() as u64)), "{}", prev.len());
        let [_, _, c] = sources(arch, 42, 0x3000);
        fx.assemble("c", &c);
        fx.write("want", None);
        let want = std::fs::read(fx.path("want")).unwrap();
        fx.with_front("want", None, |f, store| {
            let c_o = file_of(f, "c.o");
            let eh_chunk = f
                .layout
                .placement
                .chunks
                .iter()
                .find(|(_, c)| c.name(&f.assignment) == Some(&b".eh_frame"[..]))
                .unwrap()
                .0;
            let text = f.assignment.outsecs.iter().find(|(_, o)| o.name == b".text").unwrap().0;
            let delta = LayoutDelta {
                files: vec![c_o],
                growth_steps: vec![GrowthStep { out: text, step: 0x3000 }],
                // U-4's: c.o's FDE (its function grew) is rewritten; the
                // whole section here (an appearing size, IN-72 item 5).
                eh: Some(EhLayout { chunk: eh_chunk, whole: true, ranges: Vec::new() }),
                ..LayoutDelta::default()
            };
            // U-3's: the edited file and the referrers of what moved (a.o
            // loads b.o's data through the GOT and calls c.o); b.o has no
            // relocation, so its bytes are copied.
            let set = RefillSet { units: units(f, &["a.o", "c.o"]), ..RefillSet::default() };
            let plan = whole_symtab_plan(&want);
            let (r, b, counts, size, leaves) =
                run_refill(f, store, &delta, &plan, &set, &fx, "prev").unwrap();
            assert_eq!(leaves, fx.leaves_of("want"), "{arch:?}: the new leaves");
            assert_eq!(size, want.len() as u64);
            eprintln!("[u6] {arch:?} growth: {counts:?}; {} bytes to {}", prev.len(), want.len());
            assert_eq!(
                (fx.leaves_of("prev").len(), leaves.len()),
                (4, 5),
                "{arch:?}: a growth past a block's end adds a leaf"
            );
            judge(&format!("{arch:?} growth"), &prev, &want, &r, &b);
            // The ELF header, and both header tables whole.
            let le = |b: &[u8], at: usize, n: usize| {
                b[at..at + n].iter().rev().fold(0u64, |a, &x| a << 8 | u64::from(x))
            };
            let (phoff, phsz) = (le(&want, 0x20, 8), le(&want, 0x36, 2) * le(&want, 0x38, 2));
            let (shoff, shsz) = (le(&want, 0x28, 8), le(&want, 0x3a, 2) * le(&want, 0x3c, 2));
            for (lo, n) in [(phoff, phsz), (shoff, shsz)] {
                assert!(
                    r.iter().any(|x| x.offset <= lo && lo + n <= x.offset + x.len),
                    "{arch:?}: a header table [{lo:#x}, +{n:#x}) not rewritten whole: {r:?}"
                );
            }
            assert!(counts.copied > 0, "{arch:?}: nothing moved");
        });
    }
}

// spec: incr.md IN-34a ("Pad and move": an unchanged range moved, its FDEs'
// `pc_begin` and LSDA adjusted, the CIE pointer unchanged; only 0x1b/0x9b
// moved), WP-239 R-3 (from the previous output's bytes alone): a linked
// `.eh_frame`, its section moved by 0x2000 and the code by 0x1000.
#[test]
#[allow(clippy::field_reassign_with_default, reason = "`Shift` has a private field")]
fn eh_frame_moved_from_its_own_bytes() {
    for arch in ARCHES {
        let Some(fx) = Fixture::new(arch, "u6-eh") else { continue };
        fx.write("prev", None);
        let elf = std::fs::read(fx.path("prev")).unwrap();
        let le = |at: usize, n: usize| {
            elf[at..at + n].iter().rev().fold(0u64, |a, &x| a << 8 | u64::from(x))
        };
        let (shoff, ent, num, strx) = (
            le(0x28, 8) as usize,
            le(0x3a, 2) as usize,
            le(0x3c, 2) as usize,
            le(0x3e, 2) as usize,
        );
        let sh = |i: usize, f: usize| le(shoff + i * ent + f, 8);
        let names = sh(strx, 24) as usize;
        let name_of = |i: usize| {
            let at = names + le(shoff + i * ent, 4) as usize;
            let end = at + elf[at..].iter().position(|&b| b == 0).unwrap();
            elf[at..end].to_vec()
        };
        let eh = (0..num).find(|&i| name_of(i) == b".eh_frame").unwrap();
        let text = (0..num).find(|&i| name_of(i) == b".text").unwrap();
        let (eh_addr, eh_off, eh_size) = (sh(eh, 16), sh(eh, 24) as usize, sh(eh, 32));
        let bytes = &elf[eh_off..eh_off + eh_size as usize];
        let mut counts = Counts::default();
        let records = walk_eh(bytes, &mut counts).unwrap();
        let fdes: Vec<_> = records.iter().filter(|r| r.cie.is_some()).collect();
        assert!(fdes.len() >= 2, "{arch:?}: {records:?}");
        // Nothing moved: nothing patched.
        let none =
            patch_eh(bytes, &records, eh_addr, eh_addr, &Shift::default(), &mut counts).unwrap();
        assert!(none.is_empty());
        let (t_addr, t_size) = (sh(text, 16), sh(text, 32));
        let mut shift = Shift::default();
        shift.addrs = vec![(t_addr..t_addr + t_size, 0x1000), (eh_addr..eh_addr + eh_size, 0x2000)];
        shift.addrs.sort_by_key(|a| a.0.start);
        let patches =
            patch_eh(bytes, &records, eh_addr, eh_addr + 0x2000, &shift, &mut counts).unwrap();
        assert_eq!(patches.len(), fdes.len(), "{arch:?}: one pc_begin per FDE");
        let mut moved = bytes.to_vec();
        for (o, b) in &patches {
            moved[*o as usize..*o as usize + 4].copy_from_slice(b);
        }
        for f in fdes {
            let at = f.offset as usize + 8;
            let v0 = i32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
            let v1 = i32::from_le_bytes(moved[at..at + 4].try_into().unwrap());
            let t0 = (eh_addr + at as u64).wrapping_add(i64::from(v0) as u64);
            let t1 = (eh_addr + 0x2000 + at as u64).wrapping_add(i64::from(v1) as u64);
            assert_eq!(t1, t0 + 0x1000, "{arch:?}: the FDE at {:#x}", f.offset);
            // The CIE pointer (a distance inside the section) unchanged.
            assert_eq!(moved[at - 4..at], bytes[at - 4..at]);
        }
        // WP-243's moving range (a whole rewrite's range with no record
        // offsets): the first file's CIE and its FDEs, moved 0x40 bytes
        // with the code where it was; each FDE still names its code.
        let ranges: Vec<u64> =
            records.iter().filter(|r| r.cie.is_none()).map(|r| r.offset).chain([eh_size]).collect();
        let (lo, hi) = (ranges[0] as usize, ranges[1] as usize);
        let range = &bytes[lo..hi];
        let n = records.iter().filter(|r| r.cie == Some(lo as u64)).count() as u32;
        assert!(n >= 1, "{arch:?}: the first CIE's FDEs");
        let old_at = eh_addr + lo as u64;
        let moved =
            move_eh_range(range, old_at, old_at + 0x40, n, &Shift::default(), &mut counts).unwrap();
        for r in records.iter().filter(|r| r.cie == Some(lo as u64)) {
            let at = r.offset as usize - lo + 8;
            let v0 = i32::from_le_bytes(range[at..at + 4].try_into().unwrap());
            let v1 = i32::from_le_bytes(moved[at..at + 4].try_into().unwrap());
            assert_eq!(
                (old_at + at as u64).wrapping_add(i64::from(v0) as u64),
                (old_at + 0x40 + at as u64).wrapping_add(i64::from(v1) as u64),
                "{arch:?}: the FDE at {:#x} names its code",
                r.offset
            );
        }
        // The stored FDE count disagrees: `state-corrupt`; a range cut
        // from its CIE: `update-not-implemented (eh)`.
        assert!(matches!(
            move_eh_range(range, old_at, old_at, n + 1, &Shift::default(), &mut counts),
            Err(RefillError::Fallback(IncrReason::StateCorrupt(_)))
        ));
        let fde = records.iter().find(|r| r.cie == Some(lo as u64)).unwrap().offset as usize;
        assert!(matches!(
            move_eh_range(&bytes[fde..hi], old_at, old_at, n, &Shift::default(), &mut counts),
            Err(RefillError::Fallback(IncrReason::UpdateNotImplemented(Some(
                mortar_core::incr::Unimplemented::Eh
            ))))
        ));
        // An encoding IN-34a does not move: `update-not-implemented (eh)`.
        let cie = records.iter().find(|r| r.cie.is_none() && r.z).unwrap();
        let mut bad = bytes.to_vec();
        let aug = cie.offset as usize + 9;
        let r_at = aug + bad[aug..].iter().position(|&b| b == 0).unwrap() + 1;
        // code align, data align, RA (version 1: a byte), aug length: R's byte.
        let enc = r_at + 4;
        assert_eq!(bad[enc], 0x1b, "{arch:?}: the fixture's R encoding");
        bad[enc] = 0x03;
        assert!(matches!(
            walk_eh(&bad, &mut counts),
            Err(RefillError::Fallback(IncrReason::UpdateNotImplemented(Some(
                mortar_core::incr::Unimplemented::Eh
            ))))
        ));
    }
}

// spec: image.md I-78a (`symblock`), IN-49a (`hole .symtab`/`.strtab` with
// the plan's slack, `dead .strtab <off> <size> strings`): the update's map
// states U-5's plan; a default plan leaves the full link's lines.
#[test]
fn update_map_states_the_symtab_plan() {
    use mortar_core::incr::SymtabBlock;
    use mortar_image::map::{render, update_map};
    for arch in ARCHES {
        let Some(fx) = Fixture::new(arch, "u6-map") else { continue };
        let l0 = fx.write("prev", None);
        fx.with_front("prev", Some(&l0), |f, store| {
            let c_o = file_of(f, "c.o");
            let delta = LayoutDelta { files: vec![c_o], ..LayoutDelta::default() };
            let plan = SymtabPlan {
                blocks: vec![(
                    c_o,
                    SymtabBlock {
                        first: 9,
                        count: 1,
                        capacity: 3,
                        strtab_offset: 0x40,
                        strtab_len: 2,
                        strtab_capacity: 0x20,
                    },
                )],
                tables: SymtabTables {
                    symtab_size: 0x180,
                    symtab_slack: 0x60,
                    strtab_size: 0x90,
                    strtab_slack: 0x30,
                    ..SymtabTables::default()
                },
                strtab_dead: vec![(1, 5)],
                ..SymtabPlan::default()
            };
            // The stored rows: a.o's block (unchanged, not in the plan) and
            // the global area (WP-218 ruling 2).
            let blk = |first, count, capacity, off| SymtabBlock {
                first,
                count,
                capacity,
                strtab_offset: off,
                strtab_len: 4,
                strtab_capacity: 0x10,
            };
            let rows = MemoryLayout {
                symtab_blocks: vec![(file_of(f, "a.o"), blk(1, 2, 4, 0x8))],
                global_area: blk(12, 3, 8, 0x60),
                ..MemoryLayout::default()
            };
            let stored = StoredLayout::new(&rows);
            let empty = MemoryLayout::default();
            let none = StoredLayout::new(&empty);
            let pool = Pool::new(1);
            let (with, without, full) = mortar::image::with_link(f, store, &|_| {}, |l| {
                (
                    render(&update_map(l, &delta, &stored, &plan).unwrap(), &pool),
                    render(&update_map(l, &delta, &none, &SymtabPlan::default()).unwrap(), &pool),
                    render(&mortar_image::link::map_of(l), &pool),
                )
            })
            .unwrap();
            let with = String::from_utf8(with).unwrap();
            let lines: Vec<&str> = with.lines().collect();
            for want in [
                "incremental",
                "hole .symtab 180 60",
                "hole .strtab 90 30",
                "dead .strtab 1 5 strings",
            ] {
                assert!(lines.contains(&want), "{arch:?}: no `{want}` in\n{with}");
            }
            // F-4 (ruling 3): a fragment's class in the inputs' terms:
            // `sh_flags` bits (SHF_MERGE 0x10, SHF_STRINGS 0x20),
            // `sh_entsize`, `sh_addralign` in BYTES.
            let frags: Vec<&str> =
                lines.iter().copied().filter(|l| l.starts_with("frag ")).collect();
            assert!(
                frags.iter().any(|l| l.split(' ').nth(2) == Some("30,1,1")),
                "{arch:?}: the strings' class: {frags:?}"
            );
            assert!(
                frags.iter().any(|l| l.split(' ').nth(2) == Some("10,8,8")),
                "{arch:?}: the cst8 class: {frags:?}"
            );
            let blocks: Vec<&str> =
                lines.iter().copied().filter(|l| l.starts_with("symblock ")).collect();
            assert_eq!(blocks.len(), 2, "{arch:?}: {blocks:?}");
            assert!(blocks[0].ends_with(" 1 2 4 8 10"), "{arch:?}: a.o's stored block: {blocks:?}");
            assert!(blocks[1].ends_with(" 9 1 3 40 20"), "{arch:?}: c.o's planned: {blocks:?}");
            assert!(!with.contains("symblock - 12 "), "{arch:?}: the global area is no block");
            assert!(lines.iter().any(|l| l.starts_with("region ")), "{arch:?}: no region");
            // The full link was incremental-mode: its own map has the
            // decision records; a default plan changes no line of it.
            assert_eq!(String::from_utf8(without).unwrap(), String::from_utf8(full).unwrap());
        });
    }
}

/// The file range of the section named `name` of an ELF64 file.
fn section_range(elf: &[u8], name: &[u8]) -> std::ops::Range<usize> {
    let le = |at: usize, n: usize| {
        elf[at..at + n].iter().rev().fold(0u64, |a, &x| a << 8 | u64::from(x))
    };
    let (shoff, ent, num, strx) =
        (le(0x28, 8) as usize, le(0x3a, 2) as usize, le(0x3c, 2) as usize, le(0x3e, 2) as usize);
    let names = le(shoff + strx * ent + 24, 8) as usize;
    (0..num)
        .find_map(|i| {
            let at = names + le(shoff + i * ent, 4) as usize;
            let end = at + elf[at..].iter().position(|&b| b == 0).unwrap();
            let (o, n) =
                (le(shoff + i * ent + 24, 8) as usize, le(shoff + i * ent + 32, 8) as usize);
            (&elf[at..end] == name).then_some(o..o + n)
        })
        .unwrap()
}

// spec: incr.md IN-75 (the build id is the tree over the whole new output,
// I-58's; an update's id equals a full link's of the same bytes).
#[test]
fn build_id_is_the_tree_over_the_whole_new_output() {
    for arch in ARCHES {
        let Some(fx) = Fixture::big(arch, "u6-buildid") else { continue };
        let l0 = fx.write("prev", None);
        let prev = std::fs::read(fx.path("prev")).unwrap();
        let [_, _, c] = sources(arch, 45, 0);
        fx.assemble("c", &c);
        fx.write("want", Some(&l0));
        let want = std::fs::read(fx.path("want")).unwrap();
        let note = section_range(&want, b".note.gnu.build-id");
        assert_eq!(note, section_range(&prev, b".note.gnu.build-id"));
        // The 20-byte sha1 desc closes the note.
        let id = note.end - 20..note.end;
        assert_ne!(prev[id.clone()], want[id.clone()], "{arch:?}: the edit changed the id");
        fx.with_front("want", Some(&l0), |f, store| {
            let delta = LayoutDelta { files: vec![file_of(f, "c.o")], ..LayoutDelta::default() };
            let set = RefillSet { units: units(f, &["c.o"]), ..RefillSet::default() };
            let (r, b, counts, _, leaves) =
                run_refill(f, store, &delta, &SymtabPlan::default(), &set, &fx, "prev").unwrap();
            assert_eq!(leaves, fx.leaves_of("want"), "{arch:?}: the new leaves");
            assert!(counts.build_id_read < want.len() as u64, "{arch:?}: {counts:?}");
            let got = patched(&prev, &r, &b, want.len() as u64);
            assert_eq!(got[id.clone()], want[id.clone()], "{arch:?}: the update's id");
        });
    }
}

// spec: model.md §4.1 (a folded section has its target's place), image.md
// I-78a (`region`: a run of one file's members), WP-218 F-5: a folded
// section is not a member of its target's region (it takes no bytes; the
// image plan skips it too). No link folds today (`--icf` is refused by
// name); the fold is made here on a linked fixture's layout.
#[test]
fn a_folded_section_is_in_no_region() {
    use mortar_core::column::Column;
    use mortar_core::id::OptId;
    use mortar_core::layout::Folds;
    use mortar_image::map::render;
    for arch in ARCHES {
        let Some(fx) = Fixture::new(arch, "u6-fold") else { continue };
        let l0 = fx.write("prev", None);
        fx.with_front("prev", Some(&l0), |f, store| {
            let regions = |f: &Front<'_>| -> Vec<String> {
                let pool = Pool::new(1);
                let m = mortar::image::with_link(f, store, &|_| {}, |l| {
                    render(&mortar_image::link::map_of(l), &pool)
                })
                .unwrap();
                String::from_utf8(m)
                    .unwrap()
                    .lines()
                    .filter(|l| l.starts_with("region "))
                    .map(str::to_owned)
                    .collect()
            };
            let text_of = |f: &Front<'_>, name: &str| -> SectionId {
                f.resolution.section_base[file_of(f, name)]
                    .iter()
                    .find(|&s| {
                        f.layout.placement.place[s].get().is_some_and(|(c, _)| {
                            f.layout.placement.chunks[c].name(&f.assignment) == Some(&b".text"[..])
                        })
                    })
                    .unwrap()
            };
            let before = regions(f);
            let (a, c) = (text_of(f, "a.o"), text_of(f, "c.o"));
            let n = f.resolution.sections.len();
            let mut folded = Column::filled(n, OptId::NONE);
            folded[c] = OptId::some(a);
            f.layout.folds = Folds { folded: Some(folded) };
            f.layout.placement.place[c] = f.layout.placement.place[a];
            let after = regions(f);
            assert!(after.iter().all(|r| before.contains(r)), "{arch:?}: {before:?} -> {after:?}");
            assert_eq!(after.len() + 1, before.len(), "{arch:?}: c.o's region gone: {after:?}");
        });
    }
}

/// a.s with its GOT load of `d` made a direct one of the same size: the
/// slot's demand is gone.
fn a_without_got(arch: Arch) -> String {
    let [a, _, _] = sources(arch, 42, 0);
    let (from, to) = match arch {
        Arch::X86_64 => ("movq d@GOTPCREL(%rip), %rax", "leaq d(%rip), %rax"),
        _ => ("adrp x1, :got:d\n\tldr x1, [x1, :got_lo12:d]", "adrp x1, d\n\tadd x1, x1, :lo12:d"),
    };
    assert!(a.contains(from));
    a.replace(from, to)
}

// spec: incr.md IN-72 item 7 (seam 6: `slots_dead`: a retired slot's
// words written 0, a `HoleKind::DeadSlot` hole; IN-3a), image.md I-78a
// (no `slot` record for it; the `hole` line). A hand-made delta retires
// d's GOT slot; P_U is the first link's layout replayed with the hole;
// its image with the slot's words zero is what the update must equal. With
// `MORTAR_U6_DUMP`, the output, its map and the inputs are kept for
// `cargo xtask plumb`.
#[test]
#[allow(clippy::many_single_char_names)]
fn a_retired_slot_is_zero_and_a_hole() {
    use mortar_core::layout::{Hole, HoleKind};
    use mortar_image::map::{render, update_map};
    for arch in ARCHES {
        let Some(fx) = Fixture::new(arch, "u6-deadslot") else { continue };
        let l0 = fx.write("prev", None);
        let prev = std::fs::read(fx.path("prev")).unwrap();
        if l0.slots.got.is_empty() {
            eprintln!("[u6] {arch:?}: the GOT load was relaxed, no slot to retire");
            continue;
        }
        assert_eq!(l0.slots.got.len(), 1, "{arch:?}: d's slot only");
        // The edit that retires the slot (a.o's load made direct) cannot
        // be linked with the slot kept: the layout's rule `Slots` refuses
        // "a got slot without a demand" (WP-218 finding, the join's). The
        // delta retires it by hand; a.o is unchanged.
        let _ = a_without_got;
        fx.write("want", Some(&l0));
        let mut want = std::fs::read(fx.path("want")).unwrap();
        fx.with_front("want", Some(&l0), |f, store| {
            let pl = &f.layout.placement;
            let (got, chunk) = pl
                .chunks
                .iter()
                .find(|(_, c)| c.name(&f.assignment) == Some(&b".got"[..]))
                .map(|(id, c)| (id, *c))
                .unwrap();
            // d's slot: the 8-byte word of `.got` holding d's address.
            let d = {
                let plan = whole_symtab_plan(&prev);
                let names = &plan.strtab[0].1;
                plan.entries
                    .iter()
                    .find(|e| {
                        let n = &names[e.name as usize..];
                        &n[..n.iter().position(|&b| b == 0).unwrap()] == b"d"
                    })
                    .unwrap()
                    .value
            };
            let at = (0..chunk.size / 8)
                .map(|k| k * 8)
                .find(|&k| {
                    let o = (chunk.offset + k) as usize;
                    prev[o..o + 8] == d.to_le_bytes()
                })
                .expect("d's GOT slot");
            let hole = Hole { chunk: got, offset: at, size: 8, kind: HoleKind::DeadSlot };
            let mut holes = f.layout.placement.holes.to_vec();
            holes.push(hole);
            holes.sort_by_key(|h| (h.chunk, h.offset));
            f.layout.placement.holes = holes.into_boxed_slice();
            let dead = mortar_core::incr::StoredSlot {
                kind: mortar_core::layout::SlotKind::Got,
                key: l0.slots.got[0],
                index: 0,
                refs: 0,
            };
            let delta = LayoutDelta {
                files: vec![file_of(f, "a.o")],
                hole_chunks: vec![got],
                holes: vec![hole],
                slots_dead: vec![dead],
                placement: f.layout.placement.clone(),
                ..LayoutDelta::default()
            };
            let set = RefillSet { units: units(f, &["a.o"]), ..RefillSet::default() };
            let (r, b, counts, _, _) =
                run_refill(f, store, &delta, &SymtabPlan::default(), &set, &fx, "prev").unwrap();
            eprintln!("[u6] {arch:?} dead slot: {counts:?}");
            // P_U with the slot's words zero; its build id follows.
            let o = (chunk.offset + at) as usize;
            want[o..o + 8].fill(0);
            let got_bytes = patched(&prev, &r, &b, want.len() as u64);
            assert_eq!(got_bytes[o..o + 8], [0; 8], "{arch:?}: the retired slot is zero");
            let note = section_range(&want, b".note.gnu.build-id");
            let mut a = got_bytes.clone();
            a[note.clone()].fill(0);
            want[note].fill(0);
            assert!(a == want, "{arch:?}: P_U with the slot zero, the note aside");
            let rows = MemoryLayout::default();
            let stored = StoredLayout::new(&rows);
            let pool = Pool::new(1);
            let map = mortar::image::with_link(f, store, &|_| {}, |l| {
                render(&update_map(l, &delta, &stored, &SymtabPlan::default()).unwrap(), &pool)
            })
            .unwrap();
            let map = String::from_utf8(map).unwrap();
            let hole_line = format!("hole .got {at:x} 8");
            assert!(map.lines().any(|l| l == hole_line), "{arch:?}: no `{hole_line}` in\n{map}");
            assert!(
                !map.lines().any(|l| l.starts_with("slot got d ")),
                "{arch:?}: a retired slot's record:\n{map}"
            );
            if let Some(dir) = std::env::var_os("MORTAR_U6_DUMP") {
                let d = Path::new(&dir).join(format!("{arch:?}-deadslot"));
                std::fs::create_dir_all(&d).unwrap();
                std::fs::write(d.join("want"), &got_bytes).unwrap();
                std::fs::write(d.join("want.map"), &map).unwrap();
                for i in ["a.o", "b.o", "c.o"] {
                    std::fs::copy(fx.path(i), d.join(i)).unwrap();
                }
            }
        });
    }
}

// spec: image.md I-95 (WP-255a: the update fills its dirty ranges; the
// 1 MiB block is the unit of the leaf hash, not of the fill). Every U-6
// fixture that refills, both targets: each refill's ranges and bytes are
// compared with the whole windows that hold them (`clips_equal_windows`,
// run by `run_refill_with` on every refill of this file).
#[test]
fn refill_clips_equal_windows() {
    let before = WINDOW_CHECKS.with(std::cell::Cell::get);
    dirty_ranges_cover_every_differing_byte();
    symtab_shrink_zeroes_its_own_tail_only();
    growth_step_rewrites_headers();
    build_id_is_the_tree_over_the_whole_new_output();
    a_retired_slot_is_zero_and_a_hole();
    let n = WINDOW_CHECKS.with(std::cell::Cell::get) - before;
    eprintln!("[u6-clips] {n} refills compared with their windows");
    if ARCHES.iter().all(|a| a.tool("gcc").is_some()) {
        // Per arch: 2 (same-size), 1 (shrink, when it has a reserve),
        // 1 (growth), 1 (build id), 1 (dead slot, when not relaxed).
        assert!(n >= 2 * 4, "{n} refills compared");
    }
}

/// The file ranges of `f`'s exec chunks no placed input section covers,
/// each with whether it is the chunk's tail after its last section (the
/// reserve, IN-33's slack) rather than a gap between sections (I-41).
fn exec_gaps(f: &Front<'_>) -> Vec<(std::ops::Range<u64>, bool)> {
    let pl = &f.layout.placement;
    let mut out = Vec::new();
    for (id, c) in pl.chunks.iter() {
        if c.flags & mortar_elf::consts::SHF_EXECINSTR == 0 || !c.in_file() {
            continue;
        }
        let mut covered: Vec<std::ops::Range<u64>> = Vec::new();
        for (s, p) in pl.place.iter() {
            let Some((pc, at)) = p.get() else { continue };
            if pc != id {
                continue;
            }
            let Some((file, index)) = f.resolution.sh_index(s) else { continue };
            let Some(mortar_core::inputs::Reader::Object(o)) =
                f.inputs.files.get(file).map(|x| &x.reader)
            else {
                continue;
            };
            let size = o.section(index).unwrap().size();
            covered.push(c.offset + at..c.offset + at + size);
        }
        covered.sort_by_key(|r| r.start);
        let mut at = c.offset;
        for r in covered {
            if r.start > at {
                out.push((at..r.start, false));
            }
            at = at.max(r.end);
        }
        let end = c.offset + c.size.max(c.capacity);
        if end > at {
            out.push((at..end, true));
        }
    }
    out
}

/// The bytes of `gaps` (`exec_gaps`) inside `ranges`: each the arch's
/// pad in the full link `want` (I-41: the trap pattern by address, every
/// byte 0xCC on x86-64; AArch64's is 4 zero bytes) and the update's
/// `got`; the counts of gap and tail bytes.
fn pads_in(
    arch: Arch,
    gaps: &[(std::ops::Range<u64>, bool)],
    ranges: &[DirtyRange],
    got: &[u8],
    want: &[u8],
) -> (u64, u64) {
    let pad = if arch == Arch::X86_64 { 0xcc } else { 0 };
    let in_range = |at: u64| ranges.iter().any(|x| x.offset <= at && at < x.offset + x.len);
    let (mut gap, mut tail) = (0u64, 0u64);
    for (g, is_tail) in gaps {
        for at in g.clone().filter(|&at| in_range(at)) {
            assert_eq!(want[at as usize], pad, "{arch:?}: the full link's pad at {at:#x}");
            assert_eq!(got[at as usize], want[at as usize], "{arch:?}: the update at {at:#x}");
            if *is_tail {
                tail += 1;
            } else {
                gap += 1;
            }
        }
    }
    (gap, tail)
}

// spec: image.md I-41 (a gap's bytes: the trap pattern inside exec chunks,
// 0xCC on x86-64, 4 zero bytes on AArch64), I-95 (WP-255a; review F1): the
// range fill writes every byte of a range from the whole plan's rows,
// pads included. Two U-6 shapes, both targets: the growth fixture, whose
// ranges cover the `.text` tail after c.o's grown function (the review's
// 1,537 bytes on x86-64), and a same-size edit with the `.text` chunk
// rewritten whole (a `RefillSet` chunk, IN-72 item 4's shape), whose range
// covers the gaps between a.o's and c.o's functions. There the update's
// bytes are the full link's, and they are the arch's pad.
#[test]
fn clip_fill_writes_pad_bytes() {
    for arch in ARCHES {
        let Some(fx) = Fixture::new(arch, "u6-pad") else { continue };
        let l0 = fx.write("prev", None);
        let prev = std::fs::read(fx.path("prev")).unwrap();
        // A: the growth step.
        let [_, _, c] = sources(arch, 42, 0x3000);
        fx.assemble("c", &c);
        fx.write("grown", None);
        let grown = std::fs::read(fx.path("grown")).unwrap();
        let (gap_a, tail_a) = fx.with_front("grown", None, |f, store| {
            let gaps = exec_gaps(f);
            let c_o = file_of(f, "c.o");
            let eh_chunk = f
                .layout
                .placement
                .chunks
                .iter()
                .find(|(_, c)| c.name(&f.assignment) == Some(&b".eh_frame"[..]))
                .unwrap()
                .0;
            let text = f.assignment.outsecs.iter().find(|(_, o)| o.name == b".text").unwrap().0;
            let delta = LayoutDelta {
                files: vec![c_o],
                growth_steps: vec![GrowthStep { out: text, step: 0x3000 }],
                eh: Some(EhLayout { chunk: eh_chunk, whole: true, ranges: Vec::new() }),
                ..LayoutDelta::default()
            };
            let set = RefillSet { units: units(f, &["a.o", "c.o"]), ..RefillSet::default() };
            let plan = whole_symtab_plan(&grown);
            let (r, b, _, _, _) = run_refill(f, store, &delta, &plan, &set, &fx, "prev").unwrap();
            judge(&format!("{arch:?} pad growth"), &prev, &grown, &r, &b);
            let got = patched(&prev, &r, &b, grown.len() as u64);
            pads_in(arch, &gaps, &r, &got, &grown)
        });
        // B: a same-size edit, `.text` rewritten whole.
        let [_, _, c] = sources(arch, 43, 0);
        fx.assemble("c", &c);
        fx.write("want", Some(&l0));
        let want = std::fs::read(fx.path("want")).unwrap();
        let (gap_b, tail_b) = fx.with_front("want", Some(&l0), |f, store| {
            let gaps = exec_gaps(f);
            let text = f
                .layout
                .placement
                .chunks
                .iter()
                .find(|(_, c)| c.name(&f.assignment) == Some(&b".text"[..]))
                .unwrap()
                .0;
            let delta = LayoutDelta { files: vec![file_of(f, "c.o")], ..LayoutDelta::default() };
            let set = RefillSet {
                units: units(f, &["c.o"]),
                chunks: vec![mortar_core::incr::RefillChunk { chunk: text, reasons: 1 }],
                ..RefillSet::default()
            };
            let (r, b, _, _, _) =
                run_refill(f, store, &delta, &SymtabPlan::default(), &set, &fx, "prev").unwrap();
            judge(&format!("{arch:?} pad chunk"), &prev, &want, &r, &b);
            let got = patched(&prev, &r, &b, want.len() as u64);
            pads_in(arch, &gaps, &r, &got, &want)
        });
        eprintln!(
            "[u6-pad] {arch:?}: growth {gap_a} gap and {tail_a} tail bytes in the ranges; \
             .text whole {gap_b} gap and {tail_b} tail bytes"
        );
        assert!(tail_a > 0, "{arch:?}: the growth's ranges hold no tail");
        assert!(gap_b > 0, "{arch:?}: the whole .text's range holds no gap between functions");
    }
}

// spec: incr.md IN-75 and ruling F8 (WP-255a; the review's m3): a byte of
// the previous output flipped inside a block the update dirties, outside
// every dirty range, is `output-changed` before any byte is written (the
// block's stored leaf does not hash it). m2's counterpart: a byte flipped
// in a block the update leaves clean is NOT seen here; that block is not
// read, and only IN-21's stat binding (and judgement (b)) stands for it:
// the refill succeeds and the patched output keeps the flipped byte.
#[test]
fn dirty_block_flipped_is_output_changed() {
    for arch in ARCHES {
        let Some(fx) = Fixture::big(arch, "u6-m3") else { continue };
        let l0 = fx.write("prev", None);
        let prev = std::fs::read(fx.path("prev")).unwrap();
        let [_, _, c] = sources(arch, 43, 0);
        fx.assemble("c", &c);
        fx.write("want", Some(&l0));
        let want = std::fs::read(fx.path("want")).unwrap();
        fx.with_front("want", Some(&l0), |f, store| {
            let delta = LayoutDelta { files: vec![file_of(f, "c.o")], ..LayoutDelta::default() };
            let set = RefillSet { units: units(f, &["c.o"]), ..RefillSet::default() };
            let none = SymtabPlan::default();
            let (r, _, counts, _, _) =
                run_refill(f, store, &delta, &none, &set, &fx, "prev").unwrap();
            // The dirty block: block 0 only (the text and the note).
            assert_eq!(counts.leaves_hashed, 1, "{arch:?}: {counts:?}");
            assert!(r.iter().all(|x| x.offset + x.len <= mortar_digest::IMAGE_BLOCK));
            let leaves = fx.leaves_of("prev");
            let flip = |at: usize, name: &str| {
                assert!(
                    !r.iter().any(|x| x.offset <= at as u64 && (at as u64) < x.offset + x.len),
                    "{arch:?}: {at:#x} is in a range"
                );
                let mut p = prev.clone();
                p[at] ^= 0x40;
                std::fs::write(fx.path(name), &p).unwrap();
                p
            };
            // m3: inside block 0, outside every range.
            let at = 0x80000;
            let before = flip(at, "prev-m3");
            let m3 = run_refill_with(f, store, &delta, &none, &set, &fx, "prev-m3", &leaves);
            match m3 {
                Err(RefillError::Fallback(IncrReason::OutputChanged(d))) => {
                    assert!(d.contains("block 0"), "{arch:?}: {d}");
                }
                other => panic!("{arch:?}: m3 want output-changed, got {:?}", other.map(|x| x.2)),
            }
            // Nothing written: the refill writes nothing, and the file is
            // as it was.
            assert_eq!(std::fs::read(fx.path("prev-m3")).unwrap(), before);
            // m2: inside block 3, which no range touches (IN-21's case).
            let at = 3 * (1 << 20) + 0x80000;
            let p2 = flip(at, "prev-m2");
            let (r2, b2, c2, _, _) =
                run_refill_with(f, store, &delta, &none, &set, &fx, "prev-m2", &leaves).unwrap();
            assert_eq!(c2.leaves_hashed, 1, "{arch:?}: block 3 is not read");
            let got = patched(&p2, &r2, &b2, want.len() as u64);
            let differ: Vec<usize> = (0..want.len()).filter(|&i| got[i] != want[i]).collect();
            assert_eq!(differ, vec![at], "{arch:?}: m2 keeps the flipped byte (IN-21)");
        });
    }
}

/// The file range of input section `s` in `f`'s output, if placed.
fn placed_range(f: &Front<'_>, s: SectionId) -> Option<std::ops::Range<u64>> {
    let pl = &f.layout.placement;
    let (c, at) = pl.place.get(s)?.get()?;
    let c = pl.chunks.get(c)?;
    let (file, index) = f.resolution.sh_index(s)?;
    let mortar_core::inputs::Reader::Object(o) = &f.inputs.files.get(file)?.reader else {
        return None;
    };
    let size = o.section(index).ok()?.size();
    Some(c.offset + at..c.offset + at + size)
}

// spec: ruling F8 (WP-255a): the leaf check and the new leaf share one
// read of each block; a block no range touches is not read. A same-size
// edit of c.o (block 0) and of a small data section of b.o in block 2
// (b.o's other sections, 4 MiB of data around it, not refilled): two
// blocks read, each once, the bytes read the two blocks' length, and the
// check passed.
#[test]
fn leaf_check_reads_each_dirty_block_once() {
    let b_with = |arch: Arch, v: u32| {
        let [_, b, _] = sources(arch, 42, 0);
        b + &format!(
            "\t.data\n\t.skip 0x200000, 1\n\t.section .data.y,\"aw\"\n\t.quad {v}\n\
             \t.section .data.z,\"aw\"\n\t.skip 0x1ffff8, 1\n"
        )
    };
    for arch in ARCHES {
        let Some(fx) = Fixture::new(arch, "u6-once") else { continue };
        fx.assemble("b", &b_with(arch, 5));
        let l0 = fx.write("prev", None);
        let prev = std::fs::read(fx.path("prev")).unwrap();
        let [_, _, c_s] = sources(arch, 43, 0);
        fx.assemble("c", &c_s);
        fx.assemble("b", &b_with(arch, 6));
        fx.write("want", Some(&l0));
        let want = std::fs::read(fx.path("want")).unwrap();
        let w = mortar_digest::IMAGE_BLOCK;
        let differ: Vec<u64> =
            (0..want.len()).filter(|&i| prev[i] != want[i]).map(|i| i as u64).collect();
        let blocks: std::collections::BTreeSet<u64> = differ.iter().map(|i| i / w).collect();
        assert_eq!(blocks.len(), 2, "{arch:?}: the edits' blocks: {blocks:?}");
        assert!(blocks.contains(&0), "{arch:?}: {blocks:?}");
        let data_at = *differ.iter().find(|&&i| i >= w).unwrap();
        fx.with_front("want", Some(&l0), |f, store| {
            let delta = LayoutDelta { files: vec![file_of(f, "c.o")], ..LayoutDelta::default() };
            // c.o's units and b.o's section that holds the changed word.
            let y = f.resolution.section_base[file_of(f, "b.o")]
                .iter()
                .find(|&s| placed_range(f, s).is_some_and(|r| r.contains(&data_at)))
                .unwrap();
            let mut units = units(f, &["c.o"]);
            units.push(RefillUnit { section: y, reasons: 1 });
            units.sort();
            let set = RefillSet { units, ..RefillSet::default() };
            let (r, b, counts, _, leaves) =
                run_refill(f, store, &delta, &SymtabPlan::default(), &set, &fx, "prev").unwrap();
            judge(&format!("{arch:?} two blocks"), &prev, &want, &r, &b);
            assert_eq!(leaves, fx.leaves_of("want"), "{arch:?}: the new leaves");
            eprintln!("[u6-once] {arch:?}: {counts:?}");
            assert_eq!(counts.leaves_hashed, 2, "{arch:?}: {counts:?}");
            let len: u64 = blocks
                .iter()
                .map(|&k| mortar_digest::leaf_range(want.len() as u64, k))
                .map(|x| x.end - x.start)
                .sum();
            assert_eq!(counts.build_id_read, len, "{arch:?}: each dirty block read once");
        });
    }
}

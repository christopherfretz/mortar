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
    DirtyRange, EhLayout, GrowthStep, IncrReason, LayoutDelta, RefillSet, RefillUnit, SymtabEntry,
    SymtabPlan, SymtabTables,
};
use mortar_core::layout::Layout;
use mortar_core::par::Pool;
use mortar_core::{FileId, SectionId};
use mortar_diag::{Collector, Policy};
use mortar_image::update::{
    Counts, RefillError, Shift, output_size, patch_eh, refill, walk_eh,
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
        let f = Self { scratch: Scratch::new(label), arch, leaves: Default::default() };
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
    mortar::image::with_link(f, store, &|_| {}, |l| {
        refill(l, delta, plan, set, &file, leaves)
            .map(|u| (u.ranges, u.bytes, u.counts, output_size(l), u.leaves))
    })
    .unwrap()
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
                run_refill(f, store, &delta, &SymtabPlan::default(), &set, &fx, "prev")
                    .unwrap();
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
            let slope = (z1 - z0) as f64 / (s1 - s0) as f64;
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
    use mortar_core::incr::{MemoryLayout, StoredLayout, SymtabBlock};
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
                    render(
                        &update_map(l, &delta, &none, &SymtabPlan::default()).unwrap(),
                        &pool,
                    ),
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
            let blocks: Vec<&str> =
                lines.iter().copied().filter(|l| l.starts_with("symblock ")).collect();
            assert_eq!(blocks.len(), 3, "{arch:?}: {blocks:?}");
            assert!(blocks[0].ends_with(" 1 2 4 8 10"), "{arch:?}: a.o's stored block: {blocks:?}");
            assert!(blocks[1].ends_with(" 9 1 3 40 20"), "{arch:?}: c.o's planned: {blocks:?}");
            assert_eq!(blocks[2], "symblock - 12 3 8 60 10", "{arch:?}: the global area");
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
                run_refill(f, store, &delta, &SymtabPlan::default(), &set, &fx, "prev")
                    .unwrap();
            assert_eq!(leaves, fx.leaves_of("want"), "{arch:?}: the new leaves");
            assert!(counts.build_id_read < want.len() as u64, "{arch:?}: {counts:?}");
            let got = patched(&prev, &r, &b, want.len() as u64);
            assert_eq!(got[id.clone()], want[id.clone()], "{arch:?}: the update's id");
        });
    }
}

//! Hand-run benchmark of the synthetic sections (`spec/synth.md` S-89,
//! `spec/landed/synth/WP-025.md`): `cargo run --release -p mortar_synth
//! --example synth_bench [--big]`. Plans and writes `.symtab` and
//! `.strtab` for 10^6 symbols (10^7 with `--big`), and `.eh_frame` for
//! 10^6 FDEs, at 1, 2, 4 and 8 threads; prints wall time, peak RSS and
//! the load average. The write is by 4 MiB windows (the image's division,
//! S-4), so no buffer of the table's size exists; the windows' bytes are
//! folded into a checksum that must not depend on the thread count.
//! Short (seconds), at most 8 threads: the host is shared.

#![allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]

use std::time::{Duration, Instant};

use mortar_core::par::Pool;
use mortar_core::{FileId, SectionId, SymKey, SymbolId};
use mortar_diag::{Collector, Policy};
use mortar_elf::Class;
use mortar_elf::consts::{STB_GLOBAL, STT_FUNC, STV_HIDDEN};
use mortar_merge::eh::{EhFrames, EhIn, EhReloc, RelocClass, Target};
use mortar_synth::eh::{EhInfo, EhPlan, EhSource};
use mortar_synth::out::Window;
use mortar_synth::symtab::{
    GlobalIn, InternalValue, LocalIn, PatchFacts, SecKind, Sections, SymAddrs, SymCtx, SymDef,
    SymSource, SymtabPlan,
};

const THREADS: [usize; 4] = [1, 2, 4, 8];
const REPEATS: usize = 5;
const WINDOW: usize = 4 << 20;
const FILES: u32 = 2000;

fn load_average() -> String {
    std::fs::read_to_string("/proc/loadavg").map_or_else(
        |_| "unknown".into(),
        |s| s.split_whitespace().take(3).collect::<Vec<_>>().join(" "),
    )
}

/// Resets the peak RSS (`/proc/self/clear_refs`, 5); it then includes the
/// inputs, which stay resident.
fn reset_peak() {
    let _ = std::fs::write("/proc/self/clear_refs", "5");
}

fn peak_mib() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse::<f64>().ok())
        })
        .map_or(f64::NAN, |kb| kb / 1024.0)
}

/// Symbols from a pool of 4096 names of 8 to 39 bytes (C++-sized), half
/// locals over `FILES` files, half globals of which every 10th is hidden.
struct Synthetic {
    locals_per_file: u32,
    globals: u32,
    names: Vec<Vec<u8>>,
}

impl Synthetic {
    fn new(n: u32) -> Self {
        let names = (0..4096u32)
            .map(|i| {
                let len = 8 + (i.wrapping_mul(2_654_435_761) >> 27) as usize;
                let mut v = format!("_ZN4name{i:04x}").into_bytes();
                v.resize(len.max(v.len()), b'x');
                v
            })
            .collect();
        Self { locals_per_file: n / 2 / FILES, globals: n / 2, names }
    }

    fn name(&self, i: u32) -> &[u8] {
        &self.names[(i & 4095) as usize]
    }
}

impl SymSource for Synthetic {
    fn files(&self) -> u32 {
        FILES + 1
    }
    fn locals(&self, file: FileId) -> u32 {
        if file.0 == 0 { 0 } else { self.locals_per_file }
    }
    fn local(&self, file: FileId, i: u32) -> LocalIn<'_> {
        let s = file.0 * 64 + i % 64;
        LocalIn {
            name: self.name(file.0.wrapping_mul(31) ^ i),
            info: STT_FUNC,
            other: 0,
            size: 16,
            def: SymDef::Section { section: SectionId(s), value: u64::from(i) * 16 },
            referenced: false,
        }
    }
    fn globals(&self) -> u32 {
        self.globals
    }
    fn global(&self, id: SymbolId) -> Option<GlobalIn<'_>> {
        Some(GlobalIn {
            name: self.name(id.0),
            typ: STT_FUNC,
            binding: STB_GLOBAL,
            other: if id.0.is_multiple_of(10) { STV_HIDDEN } else { 0 },
            size: 32,
            def: SymDef::Section { section: SectionId(64 + id.0 % 100_000), value: 0 },
            file: FileId(1 + id.0 % FILES),
            referenced: true,
        })
    }
    fn key_name(&self, _: SymKey) -> &[u8] {
        b"k"
    }
}

struct Live;
impl Sections for Live {
    fn live(&self, _: SectionId) -> bool {
        true
    }
    fn kind(&self, _: SectionId) -> SecKind {
        SecKind::Plain
    }
    fn merge_live(&self, _: SectionId, _: u64) -> bool {
        true
    }
}

struct Flat;
impl SymAddrs for Flat {
    fn section(&self, section: SectionId, value: u64) -> Option<(u32, u64)> {
        Some((1, 0x10_0000 + u64::from(section.0) * 0x1000 + value % 0x1000))
    }
    fn eh_frame(&self) -> Option<(u32, u64)> {
        None
    }
    fn common(&self, _: SymbolId) -> Option<(u32, u64)> {
        None
    }
    fn internal(&self, _: SymbolId) -> Option<InternalValue> {
        None
    }
    fn patch(&self, _: usize) -> Option<(u32, u64)> {
        None
    }
    fn tls_start(&self) -> Option<u64> {
        None
    }
    fn section_range(&self, _: u32) -> Option<(u64, u64)> {
        Some((0, u64::MAX / 2))
    }
}

fn fold(sum: u64, bytes: &[u8]) -> u64 {
    bytes.chunks(8).fold(sum, |s, c| {
        let mut w = [0u8; 8];
        w[..c.len()].copy_from_slice(c);
        s.rotate_left(5) ^ u64::from_le_bytes(w)
    })
}

/// Writes `size` bytes by windows in parallel; the windows' checksums.
fn windows(pool: &Pool, size: u64, write: &(dyn Fn(&mut Window<'_>) + Sync)) -> Vec<u64> {
    let n = size.div_ceil(WINDOW as u64) as usize;
    pool.map(n, |k| {
        let start = (k * WINDOW) as u64;
        let len = (size - start).min(WINDOW as u64) as usize;
        let mut buf = vec![0u8; len];
        write(&mut Window::new(start, &mut buf));
        fold(0, &buf)
    })
}

/// Median and minimum in ms.
fn stats(mut v: Vec<Duration>) -> String {
    v.sort();
    format!("{:.1}/{:.1}", v[v.len() / 2].as_secs_f64() * 1e3, v[0].as_secs_f64() * 1e3)
}

fn report(threads: usize, plan: Vec<Duration>, write: Vec<Duration>, extra: &str) {
    println!(
        "  {threads} threads: plan {} ms, write {} ms (median/min of {REPEATS}), peak RSS {:.0} MiB, load {}{extra}",
        stats(plan),
        stats(write),
        peak_mib(),
        load_average()
    );
}

fn symtab(n: u32) {
    let syms = Synthetic::new(n);
    let cx = SymCtx { syms: &syms, sections: &Live, eh: None, discard: None };
    println!("symtab/strtab, {n} symbols:");
    let mut reference = None;
    for threads in THREADS {
        let pool = Pool::new(threads);
        reset_peak();
        let (mut planned, mut written, mut extra) = (Vec::new(), Vec::new(), String::new());
        for _ in 0..REPEATS {
            let diag = Collector::new(Policy::default());
            let t = Instant::now();
            let facts = PatchFacts { thunk_size: 12, erratum_843419_size: 8, mapping: true };
            let plan = SymtabPlan::new(&cx, Class::Elf64, &[], facts, &pool, &diag).expect("plan");
            planned.push(t.elapsed());
            let t = Instant::now();
            let sym = windows(&pool, plan.symtab_size(), &|w| {
                plan.write_symtab(&cx, &Flat, w).expect("symtab");
            });
            let str = windows(&pool, plan.strtab_size(), &|w| {
                plan.write_strtab(&cx, w).expect("strtab");
            });
            written.push(t.elapsed());
            let both = (sym, str);
            assert!(reference.get_or_insert_with(|| both.clone()) == &both, "{threads} threads");
            extra = format!(
                " (.symtab {:.0} MB, .strtab {:.0} MB)",
                plan.symtab_size() as f64 / 1e6,
                plan.strtab_size() as f64 / 1e6
            );
        }
        report(threads, planned, written, &extra);
    }
}

/// `sections` sections of one CIE and `per` FDEs each; every 4th
/// section's CIE differs. Parsed by `mortar_merge::eh::parse` (not timed:
/// phase 60's cost is `merge_bench`'s).
struct EhInput {
    frames: EhFrames,
    bytes: Vec<Vec<u8>>,
    info: Vec<EhInfo>,
}

impl EhInput {
    fn new(pool: &Pool, sections: u32, per: u32) -> Self {
        let mut bytes = Vec::new();
        let mut relocs = Vec::new();
        let mut info = Vec::new();
        for s in 0..sections {
            let mut b = Vec::new();
            let mut r = Vec::new();
            b.extend_from_slice(&20u32.to_le_bytes());
            b.extend_from_slice(&0u32.to_le_bytes());
            b.extend_from_slice(&[1, b'z', b'R', 0, 4, 0x78, 30, 1, 0x1b, (s % 4) as u8]);
            b.extend_from_slice(&[0; 6]);
            for k in 0..per {
                let off = b.len() as u32;
                b.extend_from_slice(&20u32.to_le_bytes());
                b.extend_from_slice(&(off + 4).to_le_bytes());
                b.extend_from_slice(&[0; 16]);
                r.push(EhReloc {
                    offset: u64::from(off) + 8,
                    r_type: 261,
                    name: "R_AARCH64_PREL32",
                    class: RelocClass::Pc,
                    width: 4,
                    addend: 0,
                    target: Target::Section {
                        section: SectionId(1_000_000 + s * per + k),
                        value: 0,
                    },
                });
            }
            bytes.push(b);
            relocs.push(r);
            info.push(EhInfo { file: FileId(1 + s), align: 3, sh_type: 1 });
        }
        let ins: Vec<EhIn<'_>> = (0..sections as usize)
            .map(|s| EhIn {
                section: SectionId(1 + s as u32),
                file: FileId(1 + s as u32),
                index: 1,
                bytes: &bytes[s],
                relocs: &relocs[s],
            })
            .collect();
        let frames = mortar_merge::eh::parse(pool, 8, &ins).expect("parse");
        Self { frames, bytes, info }
    }
}

impl EhSource for EhInput {
    fn frames(&self) -> &EhFrames {
        &self.frames
    }
    fn info(&self, s: usize) -> EhInfo {
        self.info[s]
    }
    fn bytes(&self, s: usize) -> &[u8] {
        &self.bytes[s]
    }
}

fn eh_frame() {
    let src = EhInput::new(&Pool::new(8), 10_000, 100);
    println!(".eh_frame, 10^6 FDEs in 10^4 sections (10^4 CIEs, 4 distinct):");
    let mut reference = None;
    for threads in THREADS {
        let pool = Pool::new(threads);
        reset_peak();
        let (mut planned, mut written, mut extra) = (Vec::new(), Vec::new(), String::new());
        for _ in 0..REPEATS {
            let diag = Collector::new(Policy::default());
            let t = Instant::now();
            let plan = EhPlan::new(&src, |s| s.0 % 13 != 0, &pool, &diag).expect("plan");
            planned.push(t.elapsed());
            let t = Instant::now();
            let sums = windows(&pool, plan.size(), &|w| plan.write(&src, w).expect("write"));
            written.push(t.elapsed());
            assert!(reference.get_or_insert_with(|| sums.clone()) == &sums, "{threads} threads");
            extra = format!(" ({} FDEs, {:.0} MB)", plan.fde_count(), plan.size() as f64 / 1e6);
        }
        report(threads, planned, written, &extra);
    }
}

fn main() {
    let big = std::env::args().any(|a| a == "--big");
    println!("load average at start: {}", load_average());
    symtab(1_000_000);
    if big {
        symtab(10_000_000);
    }
    eh_frame();
    println!("load average at end: {}", load_average());
}

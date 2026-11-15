//! Fill + write of plans made from the `cxx-medium-g` inputs (spec/image.md
//! §10, I-80..I-84), for WP-026's measurements.
//!
//! ```text
//! image_bench <out-dir> <MB> <threads> <fresh|exist> <resident|staged> <reps> <input>...
//! ```
//!
//! The plan: every section with file bytes (not symbol, string, relocation
//! or group tables) of every ELF64 object and archive member, in input
//! order, aligned to `sh_addralign`, repeated until the output has `MB`
//! megabytes (copies read the same input bytes). Each section has an
//! `R_AARCH64_ABS64` record every 64 bytes (the review's `wr.rs` applies an
//! 8-byte xor every 64 bytes), applied by `mortar_a64`. Timed per rep:
//! `Output::create` → `fill::write` → `commit` (the old output set aside
//! when `exist`). Printed: the median and range of wall time, CPU time per
//! rep, the process's peak RSS, and the time to read the inputs (resident
//! store only).
#![allow(clippy::cast_precision_loss)]

use std::collections::BTreeMap;
use std::fs::File;
use std::ops::Range;
use std::os::unix::fs::FileExt;
use std::path::PathBuf;
use std::time::Instant;

use mortar_a64::A64;
use mortar_arch::{
    DemandFlags, FillTables, OutputKind, Reloc, SlotKind, SymNames, SymValue, TargetKind,
};
use mortar_core::inputs::{CopyError, Errno};
use mortar_core::par::Pool;
use mortar_core::{FileId, SectionId, SymKey, SymbolId};
use mortar_image::fill::{self, Image, Tables};
use mortar_image::output::{Output, OutputOptions};
use mortar_image::phases::Sample;
use mortar_image::plan::{Builder, InputUnit, RecOrder, RelocSection};
use mortar_image::source::Source;
use mortar_image::target::Target;

struct Store {
    resident: Vec<Vec<u8>>,
    files: Vec<File>,
}

impl Source for Store {
    fn resident(&self, file: FileId, range: Range<u64>) -> Option<&[u8]> {
        self.resident.get(file.0 as usize)?.get(range.start as usize..range.end as usize)
    }
    fn copy_to(&self, file: FileId, range: Range<u64>, out: &mut [u8]) -> Result<(), CopyError> {
        if let Some(b) = self.resident(file, range.clone()) {
            out.copy_from_slice(b);
            return Ok(());
        }
        let f = self.files.get(file.0 as usize).ok_or(CopyError::OutOfRange)?;
        f.read_exact_at(out, range.start)
            .map_err(|e| CopyError::Read(Errno(e.raw_os_error().unwrap_or(5))))
    }
}

struct T;
impl SymNames for T {
    fn name(&self, _key: SymKey) -> &[u8] {
        b"s"
    }
}
impl FillTables for T {
    fn sym(&self, _key: SymKey) -> SymValue {
        SymValue { s: 0x40_1000, kind: TargetKind::Local, size: 0, live: true }
    }
    fn slot(&self, _key: SymKey, _kind: SlotKind) -> Option<u64> {
        None
    }
    fn plt(&self, _key: SymKey) -> Option<u64> {
        None
    }
    fn flags(&self, _key: SymKey) -> DemandFlags {
        DemandFlags::default()
    }
    fn redirect(&self, _s: SectionId, _o: u64) -> Option<u64> {
        None
    }
}
impl Tables for T {
    fn target_alloc(&self, _key: SymKey) -> bool {
        true
    }
}

fn u16_at(b: &[u8], o: usize) -> u64 {
    u64::from(u16::from_le_bytes([b[o], b[o + 1]]))
}
fn u32_at(b: &[u8], o: usize) -> u64 {
    u64::from(u32::from_le_bytes(b[o..o + 4].try_into().unwrap()))
}
fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// `(file offset, size, align)` of every section with bytes of the ELF64
/// object at `o`.
fn sections(b: &[u8], o: usize, out: &mut Vec<(u64, u64, u64)>) {
    if b.get(o..o + 4) != Some(b"\x7fELF") || b[o + 4] != 2 {
        return;
    }
    let shoff = u64_at(b, o + 0x28) as usize;
    let shnum = u16_at(b, o + 0x3c) as usize;
    for i in 0..shnum {
        let h = o + shoff + 64 * i;
        let ty = u32_at(b, h + 4) as u32;
        let off = u64_at(b, h + 24);
        let size = u64_at(b, h + 32);
        let align = u64_at(b, h + 48).max(1);
        if [0, 2, 3, 4, 8, 9, 17, 18].contains(&ty) || size == 0 {
            continue;
        }
        out.push((o as u64 + off, size, align));
    }
}

fn rows_of(b: &[u8]) -> Vec<(u64, u64, u64)> {
    let mut out = Vec::new();
    if b.starts_with(b"!<arch>\n") {
        let mut p = 8usize;
        while p + 60 <= b.len() {
            let h = &b[p..p + 60];
            let size: usize = std::str::from_utf8(&h[48..58]).unwrap().trim().parse().unwrap();
            if !(h.starts_with(b"/ ") || h.starts_with(b"// ") || h.starts_with(b"/SYM64/")) {
                sections(b, p + 60, &mut out);
            }
            p += 60 + size;
            p += p & 1;
        }
    } else {
        sections(b, 0, &mut out);
    }
    out
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    assert!(
        a.len() >= 8,
        "usage: image_bench <out-dir> <MB> <threads> <fresh|exist> <resident|staged> <reps> <input>..."
    );
    let dir = PathBuf::from(&a[1]);
    let mb: u64 = a[2].parse().unwrap();
    let threads: usize = a[3].parse().unwrap();
    let exist = a[4] == "exist";
    let resident = a[5] == "resident";
    let reps: usize = a[6].parse().unwrap();
    let inputs = &a[7..];
    let target_size = mb * 1_000_000;

    let t_read = Instant::now();
    let pool = Pool::new(threads);
    let bytes: Vec<Vec<u8>> = pool.map(inputs.len(), |i| std::fs::read(&inputs[i]).unwrap());
    let read_s = t_read.elapsed().as_secs_f64();
    let per_file: Vec<Vec<(u64, u64, u64)>> = bytes.iter().map(|b| rows_of(b)).collect();
    let files: Vec<File> = inputs.iter().map(|p| File::open(p).unwrap()).collect();
    let store = Store { resident: if resident { bytes } else { Vec::new() }, files };

    // The plan: rows until the target size; relocation sections shared
    // between copies.
    let mut b = Builder::new(Target::from_facts(mortar_arch::Arch::facts(&A64::M1)), 0);
    let name = b.name(b".data");
    let mut rel_of: BTreeMap<(usize, usize), u32> = BTreeMap::new();
    let mut rows = Vec::new();
    let mut off = 0u64;
    let mut records_total = 0u64;
    'fill: loop {
        for (fi, secs) in per_file.iter().enumerate() {
            for (si, &(src, size, align)) in secs.iter().enumerate() {
                if off >= target_size {
                    break 'fill;
                }
                off = off.next_multiple_of(align);
                let r = *rel_of.entry((fi, si)).or_insert_with(|| {
                    let records: Vec<Reloc> = (0..size.saturating_sub(7).div_ceil(64))
                        .map(|k| Reloc {
                            r_offset: 64 * k,
                            key: SymKey::global(SymbolId(1)),
                            r_type: 257,
                            addend: k as i64,
                        })
                        .collect();
                    records_total += records.len() as u64;
                    b.relocs(RelocSection {
                        records: records.into(),
                        order: RecOrder::Sorted,
                        proof: Vec::new().into(),
                    })
                });
                rows.push((off, size, fi, src, r));
                off += size;
            }
        }
    }
    let file_size = off;
    let mut plan_b =
        Builder::new(Target::from_facts(mortar_arch::Arch::facts(&A64::M1)), file_size);
    let _ = plan_b.name(b".data");
    // Move the relocation sections over.
    let tmp = b.finish(mortar_image::IMAGE_BLOCK, &pool).unwrap();
    for rs in tmp.relocs.into_vec() {
        plan_b.relocs(rs);
    }
    let rows_n = rows.len();
    for (k, (off, size, fi, src, r)) in rows.into_iter().enumerate() {
        let unit = InputUnit {
            file: FileId(fi as u32),
            index: 1,
            section: SectionId(k as u32),
            src,
            start: 0,
            section_size: size,
            addr: 0x40_0000 + off,
            alloc: true,
            name,
            relocs: Some(r),
        };
        plan_b.input(off, size, unit);
    }
    let plan = plan_b.finish(mortar_image::IMAGE_BLOCK, &pool).unwrap();
    let arch = A64::M1;
    let img = Image {
        plan: &plan,
        source: &store,
        arch: &arch,
        tables: &T,
        output: OutputKind::Static,
        got_base: None,
        tls: None,
        chunks: &[],
        build_id: None,
    };
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("out");
    let opts = OutputOptions::executable(Vec::new());
    let mut walls = Vec::new();
    let mut cpus = Vec::new();
    for rep in 0..=reps {
        if exist {
            if !p.exists() {
                let f = File::create(&p).unwrap();
                let ones = vec![1u8; 1 << 20];
                let mut o = 0;
                while o < file_size {
                    let n = (file_size - o).min(ones.len() as u64) as usize;
                    f.write_all_at(&ones[..n], o).unwrap();
                    o += n as u64;
                }
            }
        } else {
            let _ = std::fs::remove_file(&p);
        }
        let s0 = Sample::now();
        let out = Output::create(&p, &opts).unwrap();
        fill::write(&img, &out, out.shown(), &pool).unwrap();
        out.commit().unwrap();
        let s1 = Sample::now();
        if rep == 0 {
            continue; // warm-up
        }
        walls.push(s1.at.duration_since(s0.at).as_secs_f64() * 1000.0);
        cpus.push((s1.cpu_ticks.unwrap_or(0) - s0.cpu_ticks.unwrap_or(0)) * 10);
    }
    let _ = std::fs::remove_file(&p);
    let mut sorted = walls.clone();
    sorted.sort_by(f64::total_cmp);
    cpus.sort_unstable();
    let peak = Sample::now().peak_kib.unwrap_or(0) / 1024;
    let load = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    println!(
        "{} MB={} out_bytes={} rows={} records={} T={} {} wall_ms median {:.1} [{:.1}-{:.1}] cpu_ms median {} peak_MiB {} read_inputs_ms {:.1} load {}",
        if resident { "resident" } else { "staged" },
        mb,
        file_size,
        rows_n,
        records_total,
        threads,
        if exist { "exist" } else { "fresh" },
        sorted[sorted.len() / 2],
        sorted[0],
        sorted[sorted.len() - 1],
        cpus[cpus.len() / 2],
        peak,
        read_s * 1000.0,
        load.split_whitespace().take(3).collect::<Vec<_>>().join(" "),
    );
}

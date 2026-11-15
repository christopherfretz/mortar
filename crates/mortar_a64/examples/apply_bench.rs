//! Relocations per second of `A64::apply` on one thread (WP-023).
//!
//! Synthesises a section of `N` words carrying the mix of types a static
//! C++ link sees most (CALL26, ADRP/ADD, ABS64, PREL32, the GOT pair,
//! LDST64), classifies them once, and times `apply` over every record,
//! several rounds. Prints relocations per second and the machine load.
//!
//! `cargo run --release -p mortar_a64 --example apply_bench [N] [rounds]`

#![allow(clippy::cast_precision_loss)]

use std::time::Instant;

use mortar_a64::A64;
use mortar_arch::{
    Arch, DemandFlags, FillCx, FillTables, OutputKind, Reloc, RelocAt, SectionOut, Site, SlotKind,
    SymNames, SymValue, TargetKind,
};
use mortar_core::{FileId, SectionId, SymKey, SymbolId};

struct Tables {
    syms: Vec<SymValue>,
    got: u64,
}

fn index(key: SymKey) -> usize {
    key.as_global().map_or(0, |id| id.0 as usize)
}

impl FillTables for Tables {
    fn sym(&self, key: SymKey) -> SymValue {
        self.syms[index(key)]
    }
    fn slot(&self, key: SymKey, _kind: SlotKind) -> Option<u64> {
        Some(self.got + 8 * index(key) as u64)
    }
    fn plt(&self, _key: SymKey) -> Option<u64> {
        None
    }
    fn flags(&self, _key: SymKey) -> DemandFlags {
        DemandFlags::empty()
    }
    fn redirect(&self, _section: SectionId, _offset: u64) -> Option<u64> {
        None
    }
}

impl SymNames for Tables {
    fn name(&self, _key: SymKey) -> &[u8] {
        b"f"
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let n: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(1 << 20);
    let rounds: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(10);
    let base = 0x40_0000u64;
    let nsyms = 4096;
    let tables = Tables {
        syms: (0..nsyms)
            .map(|i| SymValue {
                s: base + 0x10_0000 + 64 * i as u64,
                kind: TargetKind::Local,
                size: 8,
                live: true,
            })
            .collect(),
        got: base + 0x80_0000,
    };
    // (type, input word) in proportion to a static C++ link.
    let mix: [(u32, u32); 10] = [
        (283, 0x9400_0000),
        (283, 0x9400_0000),
        (275, 0x9000_0000),
        (277, 0x9100_0000),
        (257, 0),
        (261, 0),
        (311, 0x9000_0000),
        (312, 0xF940_0000),
        (286, 0xF940_0000),
        (282, 0x1400_0000),
    ];
    let mut bytes = vec![0u8; 8 * n];
    let mut relocs = Vec::with_capacity(n);
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    for i in 0..n {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let (r_type, w) = mix[(seed % mix.len() as u64) as usize];
        let off = 8 * i;
        bytes[off..off + 4].copy_from_slice(&w.to_le_bytes());
        let addend = if matches!(r_type, 311 | 312 | 286) { 0 } else { (seed >> 40) as i64 & 0xF8 };
        let sym = 1 + (seed >> 20) as u32 % (nsyms as u32 - 1);
        relocs.push(Reloc {
            r_offset: off as u64,
            key: SymKey::global(SymbolId(sym)),
            r_type,
            addend,
        });
    }
    let arch = A64::M1;
    let input = bytes.clone();
    let howtos: Vec<_> = relocs
        .iter()
        .map(|r| {
            arch.classify(r.r_type, Site { bytes: &input, offset: r.r_offset }).expect("M1 type")
        })
        .collect();
    let cx = FillCx { tables: &tables, output: OutputKind::Static, got_base: None, tls: None };
    let records: &[Reloc] = &relocs;
    let mut best = f64::MAX;
    for _ in 0..rounds {
        let start = Instant::now();
        let mut out = SectionOut { bytes: &mut bytes, addr: base, alloc: true, name: b".text" };
        for (i, (r, h)) in relocs.iter().zip(&howtos).enumerate() {
            let at = RelocAt {
                file: FileId(0),
                section: SectionId(0),
                index: i as u32,
                r_offset: r.r_offset,
                key: r.key,
                r_type: r.r_type,
                addend: r.addend,
                howto: h,
                site: Site { bytes: &input, offset: r.r_offset },
                records: &records,
                proof: &[],
            };
            arch.apply(&cx, &at, &mut out).expect("in range");
        }
        best = best.min(start.elapsed().as_secs_f64());
    }
    std::hint::black_box(&bytes);
    let load = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    let cpus = std::thread::available_parallelism().map_or(0, std::num::NonZero::get);
    println!(
        "apply: {n} relocations, best of {rounds}: {:.3} ms, {:.1} M relocations/s on one thread",
        best * 1e3,
        n as f64 / best / 1e6
    );
    println!(
        "load average: {} ({cpus} CPUs)",
        load.split_whitespace().take(3).collect::<Vec<_>>().join(" ")
    );
}

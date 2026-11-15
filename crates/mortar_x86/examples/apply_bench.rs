//! Relocations per second of `X86::scan` and `X86::apply` on one thread
//! (WP-030b), comparable with `mortar_x64`'s `apply_bench`.
//!
//! Synthesises a section of `N` 16-byte sites carrying the mix of types a
//! static i386 C++ link sees most (`PLT32` calls, `PC32`, `GOTOFF`
//! `lea`s, `GOTPC`, `32` words, `GOT32X` loads relaxed to `mov $S` (G1),
//! `GOT32X` calls relaxed to `addr32 call` (G4), `TLS_LE`, `TLS_GOTIE`
//! relaxed to `mov $T` (I4)); the REL addends are in the field bytes,
//! classifies them once, and times `scan` and `apply` over every record,
//! several rounds. Prints relocations per second and the machine load.
//!
//! `cargo run --release -p mortar_x86 --example apply_bench [N] [rounds]`

#![allow(clippy::cast_precision_loss, clippy::too_many_lines)]

use std::time::Instant;

use mortar_arch::{
    Arch, DemandFlags, DemandOut, DemandSink, DynRole, EventClass, FillCx, FillTables, GlobalFlags,
    OutputKind, PlaceFacts, Reloc, RelocAt, ScanCx, ScanError, ScanTables, SectionOut, Site,
    SlotKind, SymFacts, SymNames, SymValue, TargetKind, TlsBlock,
};
use mortar_core::{FileId, SectionId, SymKey, SymbolId};
use mortar_x86::X86;

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
        Some(self.got + 4 * index(key) as u64)
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

impl ScanTables for Tables {
    fn sym(&self, key: SymKey) -> SymFacts {
        let v = self.syms[index(key)];
        SymFacts {
            kind: v.kind,
            sym_type: if v.tls { 6 } else { 1 },
            st_other: 0,
            tls: v.tls,
            dead: false,
            comdat_only: false,
            gnu_warning: false,
        }
    }
}

impl SymNames for Tables {
    fn name(&self, _key: SymKey) -> &[u8] {
        b"f"
    }
}

#[derive(Default)]
struct Count(u64);

impl DemandOut for Count {
    fn raise(&mut self, _key: SymKey, _flags: DemandFlags) {
        self.0 += 1;
    }
    fn raise_global(&mut self, _flags: GlobalFlags) {}
    fn count(&mut self, _role: DynRole, _n: u32) {}
    fn error(&mut self, _record: u32, error: ScanError) {
        panic!("scan: {error:?}");
    }
    fn reference(&mut self, _record: u32, _class: EventClass, _key: SymKey) {}
}

fn main() {
    let mut args = std::env::args().skip(1);
    let n: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(1 << 20);
    let rounds: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(10);
    let base = 0x0804_8000u64;
    let nsyms = 4096;
    let tls_syms = 64;
    let tables = Tables {
        syms: (0..nsyms)
            .map(|i| SymValue {
                s: if i < tls_syms {
                    0x0900_0000 + 4 * i as u64
                } else {
                    base + 0x10_0000 + 64 * i as u64
                },
                kind: TargetKind::Local,
                size: 8,
                live: true,
                tls: i < tls_syms,
            })
            .collect(),
        got: base + 0x80_0000,
    };
    // (type, bytes before the field, A, thread-local) in proportion to a
    // static C++ link; the field is at offset 3 of each 16-byte site.
    let mix: [(u32, &[u8], i32, bool); 10] = [
        (4, &[0x90, 0x90, 0xe8], -4, false),
        (4, &[0x90, 0x90, 0xe8], -4, false),
        (4, &[0x90, 0x90, 0xe8], -4, false),
        (2, &[0x90, 0x90, 0x68], -4, false),
        (9, &[0x90, 0x8d, 0x83], 0, false),
        (1, &[0x90, 0x90, 0x90], 0, false),
        (43, &[0x90, 0x8b, 0x83], 0, false),
        (43, &[0x90, 0xff, 0x93], 0, false),
        (17, &[0x65, 0x8b, 0x0d], 0, true),
        (16, &[0x90, 0x8b, 0x83], 0, true),
    ];
    let mut bytes = vec![0u8; 16 * n];
    let mut relocs = Vec::with_capacity(n);
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    for i in 0..n {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let (r_type, pre, addend, tls) = mix[(seed % mix.len() as u64) as usize];
        let off = 16 * i;
        bytes[off..off + 3].copy_from_slice(pre);
        bytes[off + 3..off + 7].copy_from_slice(&addend.to_le_bytes());
        let sym = if tls {
            (seed >> 20) as u32 % tls_syms as u32
        } else {
            tls_syms as u32 + (seed >> 20) as u32 % (nsyms - tls_syms) as u32
        };
        relocs.push(Reloc {
            r_offset: off as u64 + 3,
            key: SymKey::global(SymbolId(sym)),
            r_type,
            addend: 0,
        });
    }
    let arch = X86::M1;
    let input = bytes.clone();
    let howtos: Vec<_> = relocs
        .iter()
        .map(|r| {
            arch.classify(r.r_type, Site { bytes: &input, offset: r.r_offset }).expect("M1 type")
        })
        .collect();
    let tls = TlsBlock { start: 0x0900_0000, size: 0x100, align: 64 };
    let cx = FillCx {
        tables: &tables,
        output: OutputKind::Static,
        got_base: Some(tables.got),
        tls: Some(tls),
    };
    let records: &[Reloc] = &relocs;
    let leads = relocs.iter().any(|r| arch.facts().seq_leads.contains(&r.r_type));
    let at = |i: usize| RelocAt {
        file: FileId(0),
        section: SectionId(0),
        index: i as u32,
        r_offset: relocs[i].r_offset,
        key: relocs[i].key,
        r_type: relocs[i].r_type,
        addend: None,
        howto: &howtos[i],
        site: Site { bytes: &input, offset: relocs[i].r_offset },
        records: &records,
        proof: &[],
        leads,
    };
    let scx = ScanCx {
        tables: &tables,
        output: OutputKind::Static,
        place: PlaceFacts { alloc: true, writable: false, exec: None },
        name: b".text",
    };
    let mut best_scan = f64::MAX;
    let mut count = Count::default();
    for _ in 0..rounds {
        let start = Instant::now();
        let mut sink = DemandSink::new(&mut count);
        for i in 0..n {
            arch.scan(&scx, &at(i), &mut sink);
        }
        best_scan = best_scan.min(start.elapsed().as_secs_f64());
    }
    let mut best = f64::MAX;
    for _ in 0..rounds {
        let start = Instant::now();
        let mut out = SectionOut::whole(&mut bytes, base, true, b".text").with_exec(true);
        for i in 0..n {
            arch.apply(&cx, &at(i), &mut out).expect("in range");
        }
        best = best.min(start.elapsed().as_secs_f64());
    }
    std::hint::black_box((&bytes, count.0));
    let load = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    let cpus = std::thread::available_parallelism().map_or(0, std::num::NonZero::get);
    println!(
        "scan: {n} relocations, best of {rounds}: {:.3} ms, {:.1} M relocations/s on one thread",
        best_scan * 1e3,
        n as f64 / best_scan / 1e6
    );
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

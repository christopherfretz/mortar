//! `spec/dyn.md` D-40, D-46 (WP-046): the records of a section and the
//! parallel fill of `.rela.dyn`'s `RELATIVE` part.

use std::sync::OnceLock;

use mortar_a64::A64;
use mortar_arch::{
    Arch, OutputKind, PlaceFacts, ProofRun, Records, RelForm, Reloc, RelocAt, ScanCx, ScanTables,
    Site, SymFacts, SymNames, TargetKind,
};
use mortar_core::par::Pool;
use mortar_core::{FileId, SectionId, SymKey, SymbolId};
use mortar_elf::Class;
use mortar_image::dynrel::{self, RelaDyn, SectionCx};
use mortar_synth::Window;
use mortar_synth::rela::{self, DynRecord};

const ABS64: u32 = 257;

/// Every key `Local`, except `SymbolId(1)` (`Shlib` data) and
/// `SymbolId(2)` (`WeakDyn`).
struct Facts;

impl SymNames for Facts {
    fn name(&self, _: SymKey) -> &[u8] {
        b"x"
    }
}

impl ScanTables for Facts {
    fn sym(&self, key: SymKey) -> SymFacts {
        let kind = match key.as_global() {
            Some(SymbolId(1)) => TargetKind::Shlib { func: false },
            Some(SymbolId(2)) => TargetKind::WeakDyn,
            _ => TargetKind::Local,
        };
        SymFacts {
            kind,
            sym_type: 1,
            st_other: 0,
            tls: false,
            dead: false,
            comdat_only: false,
            gnu_warning: false,
        }
    }
}

struct Recs(Vec<Reloc>);

impl Records for Recs {
    fn len(&self) -> u32 {
        self.0.len() as u32
    }
    fn get(&self, i: u32) -> Option<Reloc> {
        <[Reloc]>::get(&self.0, i as usize).copied()
    }
}

fn howto() -> &'static mortar_arch::Howto {
    static H: OnceLock<mortar_arch::Howto> = OnceLock::new();
    H.get_or_init(|| A64::M1.classify(ABS64, Site { bytes: &[0; 8], offset: 0 }).unwrap())
}

/// The records of a writable section of `n` `ABS64` words in a `Pie`,
/// word `i` against `keys(i)` with addend `i`.
fn section(n: u32, addr: u64, keys: impl Fn(u32) -> SymKey) -> dynrel::SectionRecords {
    let recs = Recs(
        (0..n)
            .map(|i| Reloc {
                r_offset: 8 * u64::from(i),
                key: keys(i),
                r_type: ABS64,
                addend: i64::from(i),
            })
            .collect(),
    );
    let bytes = vec![0u8; 8 * n as usize];
    let proof: [ProofRun; 0] = [];
    let scan = ScanCx {
        tables: &Facts,
        output: OutputKind::Pie,
        place: PlaceFacts { alloc: true, writable: true, exec: Some(false) },
        name: b".data",
    };
    let value = |k: SymKey, a: i64| -> Option<u64> {
        let s = match k.as_global() {
            Some(SymbolId(1 | 2)) => 0,
            _ => 0x10_0000,
        };
        Some((i64::from(s) + a) as u64)
    };
    let dynsym = |id: SymbolId| -> Option<u32> { (id.0 <= 2).then_some(10 - id.0) };
    let cx = SectionCx { arch: &A64::M1, scan, addr, value: &value, dynsym: &dynsym };
    let at = |i: u32| {
        let r = recs.get(i)?;
        Some(RelocAt {
            file: FileId(0),
            section: SectionId(0),
            index: i,
            r_offset: r.r_offset,
            key: r.key,
            r_type: r.r_type,
            addend: Some(r.addend),
            howto: howto(),
            site: Site { bytes: &bytes, offset: r.r_offset },
            records: &recs,
            proof: &proof,
            leads: true,
        })
    };
    dynrel::section_records(&cx, &recs, &at).unwrap()
}

fn local(_: u32) -> SymKey {
    SymKey::global(SymbolId(5))
}

// D-46: an absolute word in a writable place in a `Pie` is a `RELATIVE`
// with the addend `S + A` against a local; a symbolic record against a
// shared or weak-dynamic key, `r_sym` its `.dynsym` index, addend `A`.
#[test]
fn d46_section_records_by_target_kind() {
    let keys = |i: u32| {
        SymKey::global(SymbolId(match i % 3 {
            0 => 5,
            1 => 1,
            _ => 2,
        }))
    };
    let r = section(6, 0x4000, keys);
    assert_eq!(r.relative.len(), 2);
    assert_eq!(
        r.relative[0],
        DynRecord { r_offset: 0x4000, r_sym: 0, r_type: 1027, addend: 0x10_0000 }
    );
    assert_eq!(r.relative[1].addend, 0x10_0003);
    assert_eq!(
        r.other,
        [
            DynRecord { r_offset: 0x4008, r_sym: 9, r_type: ABS64, addend: 1 },
            DynRecord { r_offset: 0x4010, r_sym: 8, r_type: ABS64, addend: 2 },
            DynRecord { r_offset: 0x4020, r_sym: 9, r_type: ABS64, addend: 4 },
            DynRecord { r_offset: 0x4028, r_sym: 8, r_type: ABS64, addend: 5 },
        ]
    );
}

/// `units` sections of RELATIVE records, unit `u` holding `count(u)`.
fn counts(units: usize, total: usize) -> Vec<u32> {
    // Uneven sizes: 1..=2k, scaled to the total.
    let raw: Vec<u64> = (0..units as u64).map(|u| 1 + (u * 7919) % 2000).collect();
    let sum: u64 = raw.iter().sum();
    let mut c: Vec<u32> = raw.iter().map(|&r| (r * total as u64 / sum) as u32).collect();
    let short = total - c.iter().map(|&n| n as usize).sum::<usize>();
    c[0] += short as u32;
    c
}

fn fill(pool: &Pool, counts: &[u32]) -> Vec<u8> {
    let addrs: Vec<u64> = counts
        .iter()
        .scan(0x1_0000u64, |a, &n| {
            let here = *a;
            *a += 8 * u64::from(n) + 64;
            Some(here)
        })
        .collect();
    dynrel::fill_relative(pool, Class::Elf64, RelForm::Rela, counts, |u| {
        Ok(section(counts[u], addrs[u], local).relative)
    })
    .unwrap()
}

// D-40: the RELATIVE part is the same bytes at one thread and at many,
// whatever the batching (prefix sums; no unit reads another's output).
#[test]
fn d40_relative_part_is_independent_of_threads() {
    let c = counts(257, 20_000);
    let one = fill(&Pool::sequential(), &c);
    for threads in [2, 3, 8] {
        assert_eq!(fill(&Pool::new(threads), &c), one, "{threads} threads");
    }
    // Sorted by r_offset, as the units are in address order.
    let offs: Vec<u64> =
        one.chunks_exact(24).map(|r| u64::from_le_bytes(r[..8].try_into().unwrap())).collect();
    assert_eq!(offs.len(), 20_000);
    assert!(offs.windows(2).all(|w| w[0] < w[1]));
}

// A unit whose records differ from the plan's count is an internal error,
// never a short or overlapping table.
#[test]
fn d40_a_count_mismatch_is_an_error() {
    let e = dynrel::fill_relative(&Pool::sequential(), Class::Elf64, RelForm::Rela, &[2, 3], |u| {
        Ok(section(2, 0, local).relative.into_iter().take(2 + u).collect())
    });
    assert!(e.unwrap_err().contains("unit 1"));
}

// D-40 step 2: the other records sorted by (.dynsym index, r_offset),
// after the RELATIVE part; any window gives the same bytes.
#[test]
fn d40_rela_dyn_windows() {
    use mortar_core::demands::{Demands, DynCounts};
    let r = section(6, 0x4000, |i| SymKey::global(SymbolId([5, 1, 2][i as usize % 3])));
    let mut d = Demands::empty(8);
    d.dyn_relocs = vec![(SectionId(0), DynCounts([2, 4, 0, 0]))].into();
    let got = got_plan();
    let plan = mortar_synth::RelaPlan::new(Class::Elf64, RelForm::Rela, &d, &got, 0).unwrap();
    assert_eq!((plan.relative_count(), plan.other_count(), plan.size()), (2, 4, 144));
    let mut rel = vec![0u8; 48];
    rela::encode(Class::Elf64, RelForm::Rela, &r.relative, &mut rel);
    let dynr =
        RelaDyn::new(&plan, Class::Elf64, RelForm::Rela, rel.clone(), r.other.clone(), Vec::new())
            .unwrap();
    // An IRELATIVE the plan does not count (not a StaticPie) is an error.
    let stray = r.other[..1].to_vec();
    assert!(RelaDyn::new(&plan, Class::Elf64, RelForm::Rela, rel, r.other.clone(), stray).is_err());
    let mut whole = vec![0u8; 144];
    dynr.write(&mut Window::new(0, &mut whole));
    let syms: Vec<(u64, u64)> = whole[48..]
        .chunks_exact(24)
        .map(|c| {
            let info = u64::from_le_bytes(c[8..16].try_into().unwrap());
            (info >> 32, u64::from_le_bytes(c[..8].try_into().unwrap()))
        })
        .collect();
    assert_eq!(syms, [(8, 0x4010), (8, 0x4028), (9, 0x4008), (9, 0x4020)]);
    for step in [1usize, 7, 24, 50] {
        let mut pieces = vec![0u8; 144];
        for (k, chunk) in pieces.chunks_mut(step).enumerate() {
            dynr.write(&mut Window::new((k * step) as u64, chunk));
        }
        assert_eq!(pieces, whole, "windows of {step}");
    }
}

fn got_plan() -> mortar_synth::GotPlan {
    let diag = mortar_diag::Collector::new(mortar_diag::Policy::default());
    mortar_synth::GotPlan::new(
        &A64::M1,
        &mortar_core::Slots::default(),
        mortar_core::demands::GlobalFlags::empty(),
        mortar_arch::PltVariant::Plain,
        &diag,
    )
    .unwrap()
}

// The cost of the RELATIVE fill on 10^6 records (D-40's bench), at 1 and
// 8 threads: `cargo test --release -p mortar_image --test dynrel --
// --ignored --nocapture`.
#[test]
#[ignore = "a measurement, not a check"]
fn bench_relative_fill_1e6() {
    let c = counts(4096, 1_000_000);
    for threads in [1usize, 8] {
        let pool = if threads == 1 { Pool::sequential() } else { Pool::new(threads) };
        let mut best = f64::MAX;
        let mut bytes = Vec::new();
        for _ in 0..5 {
            let t = std::time::Instant::now();
            bytes = fill(&pool, &c);
            best = best.min(t.elapsed().as_secs_f64() * 1e3);
        }
        assert_eq!(bytes.len(), 24_000_000);
        println!("relative fill, 10^6 records, {threads} thread(s): {best:.1} ms (best of 5)");
    }
}

//! `spec/dyn.md` D-120, D-46 (WP-118): the records of a writable section
//! of a shared object on AArch64. An absolute word against a preemptible
//! definition is an `ABS64` against its `.dynsym` entry (its place zeroed
//! under RELA), as GNU ld writes for `int *const ptr = &counter;` in a
//! library (record §F: `R_AARCH64_ABS64 counter + 0`); against a local
//! definition a `RELATIVE`; against a shared or weak-dynamic key the
//! symbolic record of an executable.

use std::sync::OnceLock;

use mortar_a64::A64;
use mortar_arch::{
    Arch, OutputKind, PlaceFacts, ProofRun, Records, Reloc, RelocAt, ScanCx, ScanTables, Site,
    SymFacts, SymNames, TargetKind,
};
use mortar_core::{FileId, SectionId, SymKey, SymbolId};
use mortar_image::dynrel::{self, SectionCx};
use mortar_synth::rela::DynRecord;

const ABS64: u32 = 257;
const RELATIVE: u32 = 1027;

/// 1: preemptible (an export here); 2: `Shlib` data; 3: `WeakDyn`; any
/// other key `Local`.
struct Facts;

impl SymNames for Facts {
    fn name(&self, _: SymKey) -> &[u8] {
        b"x"
    }
}

impl ScanTables for Facts {
    fn sym(&self, key: SymKey) -> SymFacts {
        SymFacts {
            kind: kind(key),
            sym_type: 1,
            st_other: 0,
            tls: false,
            dead: false,
            comdat_only: false,
            gnu_warning: false,
        }
    }
}

fn kind(key: SymKey) -> TargetKind {
    match key.as_global() {
        Some(SymbolId(1)) => TargetKind::Preemptible,
        Some(SymbolId(2)) => TargetKind::Shlib { func: false },
        Some(SymbolId(3)) => TargetKind::WeakDyn,
        _ => TargetKind::Local,
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

// D-120, D-46: one `ABS64` word per key kind in `.data` of a `Shared`
// output: the preemptible key's is symbolic (r_sym its `.dynsym` index,
// addend A) with its place zeroed; the local key's a `RELATIVE` (S + A).
#[test]
fn d120_records_by_target_kind() {
    let keys = [1u32, 5, 2, 3].map(|k| SymKey::global(SymbolId(k)));
    let recs = Recs(
        keys.iter()
            .enumerate()
            .map(|(i, &key)| Reloc { r_offset: 8 * i as u64, key, r_type: ABS64, addend: i as i64 })
            .collect(),
    );
    let bytes = vec![0u8; 8 * keys.len()];
    let proof: [ProofRun; 0] = [];
    let scan = ScanCx {
        tables: &Facts,
        output: OutputKind::Shared,
        place: PlaceFacts { alloc: true, writable: true, exec: Some(false) },
        name: b".data",
    };
    let value = |k: SymKey, a: i64| -> Result<u64, dynrel::Unvalued> {
        let s = match k.as_global() {
            Some(SymbolId(2 | 3)) => 0,
            Some(SymbolId(1)) => 0x2_0010,
            _ => 0x10_0000,
        };
        Ok((s + a) as u64)
    };
    // `.dynsym`: 1, 2 and 3 at 7, 4 and 3.
    let dynsym = |id: SymbolId| -> Option<u32> {
        match id.0 {
            1 => Some(7),
            2 => Some(4),
            3 => Some(3),
            _ => None,
        }
    };
    let cx = SectionCx { arch: &A64::M1, scan, addr: 0x1_e000, value: &value, dynsym: &dynsym };
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
    let r = dynrel::section_records(&cx, &recs, &at).unwrap();
    assert_eq!(
        r.relative,
        [DynRecord { r_offset: 0x1_e008, r_sym: 0, r_type: RELATIVE, addend: 0x10_0001 }]
    );
    assert_eq!(
        r.other,
        [
            DynRecord { r_offset: 0x1_e000, r_sym: 7, r_type: ABS64, addend: 0 },
            DynRecord { r_offset: 0x1_e010, r_sym: 4, r_type: ABS64, addend: 2 },
            DynRecord { r_offset: 0x1_e018, r_sym: 3, r_type: ABS64, addend: 3 },
        ]
    );
    // D-46: the symbolic records' places hold 0; the RELATIVE's is the
    // fill's value (not zeroed).
    let zeroed: Vec<bool> = (0..4)
        .map(|i| dynrel::zeroed_place(&A64::M1, kind(keys[i]), &cx.scan, &at(i as u32).unwrap()))
        .collect();
    assert_eq!(zeroed, [true, false, true, true]);
}

//! `spec/dyn.md` D-32, D-41, D-42, D-46, D-48, D-49 (WP-046): the GOT,
//! `.got.plt`, PLT and `.rela.plt` of an `Exec` or `Pie` with `.dynamic`
//! on AArch64, and the copy plan.

mod common;

#[path = "../../mortar_scan/tests/support/dynarch.rs"]
#[allow(dead_code, reason = "a fixture shared with the scan's tests")]
mod dynarch;

use std::collections::BTreeMap;

use common::collector;
use dynarch::DynA64;
use mortar_arch::{DynRole, PltVariant, TargetKind, TlsBlock};
use mortar_core::demands::{DemandFlags, Demands, GlobalFlags};
use mortar_core::layout::CopyPlace;
use mortar_core::{FileId, Slots, SymKey, SymbolId};
use mortar_synth::got::{GotAddrs, SlotValues};
use mortar_synth::{CopyDef, CopyDefs, DynGotCx, DynsymPlan, GotPlan, KeyKinds, Window, copy_plan};

fn k(n: u32) -> SymKey {
    SymKey::global(SymbolId(n))
}

/// 1, 2: shared functions; 3: weak-dynamic; 4: local; 5: an IFUNC here;
/// 6: shared data.
struct Kinds;

impl KeyKinds for Kinds {
    fn kind(&self, key: SymKey) -> TargetKind {
        match key.as_global().map(|i| i.0) {
            Some(1 | 2) => TargetKind::Shlib { func: true },
            Some(3) => TargetKind::WeakDyn,
            Some(5) => TargetKind::Ifunc,
            Some(6) => TargetKind::Shlib { func: false },
            _ => TargetKind::Local,
        }
    }
}

struct Values;

impl SlotValues for Values {
    fn address(&self, key: SymKey) -> Option<u64> {
        BTreeMap::from([(k(4), 0x2_0000u64), (k(5), 0x1_1000)]).get(&key).copied()
    }
    fn tls(&self) -> Option<TlsBlock> {
        None
    }
    fn dynamic(&self) -> Option<u64> {
        Some(0x3_0000)
    }
}

/// `.dynsym` holds 1, 2, 3, 6 as entries 1..=4.
fn dynsym() -> DynsymPlan {
    DynsymPlan::new([1, 2, 3, 6].map(SymbolId).into(), 0, 1).unwrap()
}

fn plan(pie: bool) -> GotPlan {
    let slots = Slots {
        got: [k(1), k(3), k(4), k(6)].into(),
        plt: [k(1), k(2), k(3)].into(),
        iplt: [k(5)].into(),
        ..Slots::default()
    };
    let diag = collector();
    GotPlan::new_dynamic(
        &DynA64::default(),
        &slots,
        GlobalFlags::empty(),
        PltVariant::Plain,
        DynGotCx { pie, bind_now: false },
        &Kinds,
        &dynsym(),
        &diag,
    )
    .unwrap()
}

const AT: GotAddrs = GotAddrs { got: 0x4_0000, got_plt: 0x4_1000, plt: 0x1_0000 };

fn words(b: &[u8]) -> Vec<u64> {
    b.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect()
}

// D-46, D-48: a shared or weak-dynamic slot is a GLOB_DAT against the
// key's `.dynsym` index; a local slot a RELATIVE in a PIE, nothing in an
// `Exec`; AArch64's `.got` starts with the `_DYNAMIC` word (D-32).
#[test]
fn d46_got_records_by_kind() {
    let pie = plan(true);
    // The RELATIVE ones first (the GOT's unit of D-40's first part),
    // each part by offset; slots after the header word, in slot order.
    let recs: Vec<(u64, DynRole, u32, u32)> =
        pie.got_records().iter().map(|r| (r.offset, r.role, r.r_type, r.r_sym)).collect();
    assert_eq!(
        recs,
        [
            (24, DynRole::Relative, 1027, 0),
            (8, DynRole::GlobDat, 1025, 1),
            (16, DynRole::GlobDat, 1025, 3),
            (32, DynRole::GlobDat, 1025, 4),
        ]
    );
    assert_eq!(pie.got_relative(), 1);
    let exec = plan(false);
    assert_eq!(exec.got_records().len(), 3);
    assert_eq!(exec.got_relative(), 0);
    // The place of a GLOB_DAT holds 0 (RELA), the header `_DYNAMIC`.
    let mut got = vec![0xffu8; exec.got_size() as usize];
    exec.write_got(&AT, &Values, &mut Window::new(0, &mut got)).unwrap();
    assert_eq!(words(&got), [0x3_0000, 0, 0, 0x2_0000, 0]);
}

// D-41, D-49: `.rela.plt` holds one JUMP_SLOT per PLT entry, in PLT
// order, then the IRELATIVEs; each lazy `.got.plt` word holds PLT0 on
// AArch64; three reserved words.
#[test]
fn d41_d49_rela_plt_and_lazy_words() {
    let p = plan(true);
    assert!(p.is_dynamic());
    assert_eq!(p.got_plt_words(), 3 + 3 + 1);
    let mut gp = vec![0u8; p.got_plt_size() as usize];
    p.write_got_plt_at(Some(&AT), &Values, &mut Window::new(0, &mut gp)).unwrap();
    let w = words(&gp);
    assert_eq!(&w[..3], [0, 0, 0]);
    assert_eq!(&w[3..6], [AT.plt; 3]);
    let mut rp = vec![0u8; p.rela_plt_size() as usize];
    p.write_rela_plt(&AT, &Values, &mut Window::new(0, &mut rp)).unwrap();
    let rows: Vec<(u64, u64, u64)> = rp
        .chunks_exact(24)
        .map(|c| {
            let w = words(c);
            (w[0], w[1], w[2])
        })
        .collect();
    assert_eq!(
        rows,
        [
            (AT.got_plt + 24, (1 << 32) | 0x402, 0),
            (AT.got_plt + 32, (2 << 32) | 0x402, 0),
            (AT.got_plt + 40, (3 << 32) | 0x402, 0),
            (AT.got_plt + 48, 1032, 0x1_1000),
        ]
    );
    // Any window gives the same bytes.
    let mut pieces = vec![0u8; rp.len()];
    for (i, c) in pieces.chunks_mut(13).enumerate() {
        p.write_rela_plt(&AT, &Values, &mut Window::new(13 * i as u64, c)).unwrap();
    }
    assert_eq!(pieces, rp);
    // The PLT: PLT0, then the entries, then the IPLT entry; a key's
    // address is its entry's.
    assert_eq!(p.plt_address(&AT, k(1)), Some(AT.plt + p.plt_entry(0)));
    assert!(p.plt_entry(0) > 0);
    assert_eq!(p.plt_address(&AT, k(4)), None);
}

struct Defs(BTreeMap<SymbolId, CopyDef>);

impl CopyDefs for Defs {
    fn copy_def(&self, key: SymbolId) -> Option<CopyDef> {
        self.0.get(&key).copied()
    }
}

fn def(file: u32, value: u64, size: u64, read_only: bool) -> CopyDef {
    CopyDef {
        file: FileId(file),
        value,
        size,
        section_align: Some(16),
        read_only,
        sym_type: 1,
        visibility: 0,
    }
}

// D-42: one allocation per (file, value), its size the largest alias's,
// its alignment min(sh_addralign, 2^ctz(value)), its key the least
// alias; read-only definitions in `.bss.rel.ro`'s place; offsets by the
// least alias.
#[test]
fn d42_copy_plan() {
    let mut d = Demands::empty(12);
    for id in [2, 3, 7, 9] {
        d.global[SymbolId(id)] = DemandFlags::COPY;
    }
    let defs = Defs(BTreeMap::from([
        (SymbolId(2), def(1, 0x1008, 4, false)),
        (SymbolId(7), def(1, 0x1008, 8, false)),
        (SymbolId(9), def(1, 0x2000, 16, false)),
        (SymbolId(3), def(1, 0x3004, 12, true)),
    ]));
    let p = copy_plan(&d, &defs).unwrap();
    assert_eq!(p.copies.len(), 3);
    let by_alias: BTreeMap<u32, (u64, u64, CopyPlace)> = p
        .copies
        .iter()
        .enumerate()
        .map(|(i, c)| (c.alias.0, (p.size[i], c.align, c.place)))
        .collect();
    assert_eq!(by_alias[&2], (8, 8, CopyPlace::DynBss));
    assert_eq!(by_alias[&9], (16, 16, CopyPlace::DynBss));
    assert_eq!(by_alias[&3], (12, 4, CopyPlace::RelRo));
    assert_eq!(
        p.copy_of.iter().map(|&(k, i)| (k.0, p.copies[i as usize].alias.0)).collect::<Vec<_>>(),
        [(2, 2), (3, 3), (7, 2), (9, 9)]
    );
    // .dynbss: alias 2 at 0 (8 bytes), alias 9 at 16; .bss.rel.ro: 12.
    assert_eq!((p.dynbss, p.relro), ((32, 16), (12, 4)));
    let off: BTreeMap<u32, u64> =
        p.copies.iter().enumerate().map(|(i, c)| (c.alias.0, p.offset[i])).collect();
    assert_eq!(off, BTreeMap::from([(2, 0), (3, 0), (9, 16)]));
    // A demand the scan should have refused (size 0) is an internal error.
    let mut bad = defs.0.clone();
    bad.insert(SymbolId(9), def(1, 0x2000, 0, false));
    assert!(copy_plan(&d, &Defs(bad)).is_err());
}

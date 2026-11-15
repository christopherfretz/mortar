//! `spec/image.md` §4.5: the parallel merge of events (I-20, I-21)
//! equals the sequential meaning, kept here as an oracle, under every
//! schedule of `par::check_schedules`.

use std::collections::BTreeMap;

use mortar_core::par::{Pool, check_schedules};
use mortar_core::{FileId, SymKey, SymbolId};
use mortar_scan::events::{Event, EventClass, GroupKey, RefId, UnitEvents, merge, sym_group};

/// The sequential meaning (§4.5): every event in `RefId` order; per
/// group, the first three places and the count.
fn oracle(mut events: Vec<Event>) -> Vec<(GroupKey, Vec<RefId>, u64)> {
    events.sort_by_key(|e| e.at);
    let mut m: BTreeMap<GroupKey, (Vec<RefId>, u64)> = BTreeMap::new();
    for e in events {
        let g = m.entry(e.key()).or_default();
        if g.0.len() < 3 {
            g.0.push(e.at);
        }
        g.1 += 1;
    }
    m.into_iter().map(|(k, (f, c))| (k, f, c)).collect()
}

/// The units' events through `UnitEvents` and `merge`.
fn parallel(units: &[Vec<Event>], pool: &Pool) -> Vec<(GroupKey, Vec<RefId>, u64)> {
    let per: Vec<_> = pool.map(units.len(), |u| {
        let mut ev = UnitEvents::default();
        for &e in &units[u] {
            ev.push(e);
        }
        ev.finish()
    });
    merge(per, pool).into_iter().map(|(k, a)| (k, a.firsts().to_vec(), a.count)).collect()
}

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 =
            self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
}

/// Units as the scan makes them: files, sections in order, blocks of
/// records; each record has at most one event of each class.
fn units(seed: u64, files: u32, sections: u32, blocks: u32, per: u32) -> Vec<Vec<Event>> {
    let mut r = Lcg(seed);
    let hot = sym_group(SymKey::global(SymbolId(7)));
    let mut out = Vec::new();
    for f in 1..=files {
        for s in 1..=sections {
            for b in 0..blocks {
                let mut v = Vec::new();
                for i in 0..per {
                    let at = RefId { file: FileId(f), section: s, record: b * per + i };
                    // The key referenced from every block.
                    if i == 0 {
                        v.push(Event { class: EventClass::Undefined, group: hot, at });
                    }
                    match r.next() % 16 {
                        0 => v.push(Event {
                            class: EventClass::Undefined,
                            group: sym_group(SymKey::global(SymbolId((r.next() % 50) as u32))),
                            at,
                        }),
                        1 => v.push(Event {
                            class: EventClass::ComdatMismatch,
                            group: sym_group(SymKey::global(SymbolId(100 + (r.next() % 3) as u32))),
                            at,
                        }),
                        // W1: equal (section, symbol) in every file.
                        2 => v.push(Event {
                            class: EventClass::DiscardedSection,
                            group: u64::from(s) << 32 | 5,
                            at,
                        }),
                        _ => {}
                    }
                    if r.next().is_multiple_of(32) {
                        v.push(Event { class: EventClass::GnuWarning, group: hot, at });
                    }
                }
                out.push(v);
            }
        }
    }
    out
}

#[test]
fn i21_merge_equals_sequential_meaning_under_schedules() {
    for seed in [1, 2, 3] {
        let u = units(seed, 3, 4, 5, 40);
        let want = oracle(u.iter().flatten().copied().collect());
        let got = check_schedules(|pool| parallel(&u, pool));
        assert_eq!(got, want, "seed {seed}");
    }
}

#[test]
fn i21_a_key_referenced_from_thousands_of_blocks() {
    // 4 files × 2 sections × 500 blocks = 4000 units, each referencing
    // the hot key: one group, the first three places in file 1, the
    // count every reference.
    let u = units(9, 4, 2, 500, 3);
    let want = oracle(u.iter().flatten().copied().collect());
    let got = check_schedules(|pool| parallel(&u, pool));
    assert_eq!(got, want);
    let hot = GroupKey {
        class: EventClass::Undefined,
        file: FileId(0),
        group: sym_group(SymKey::global(SymbolId(7))),
    };
    let row = got.iter().find(|g| g.0 == hot).unwrap();
    assert!(row.2 >= 4000);
    assert_eq!(
        row.1,
        [0, 3, 6].map(|record| RefId { file: FileId(1), section: 1, record }).to_vec()
    );
}

#[test]
fn w1_discarded_groups_of_two_files_stay_apart() {
    let at = |f, record| RefId { file: FileId(f), section: 2, record };
    let e = |f, r| Event { class: EventClass::DiscardedSection, group: 2 << 32 | 9, at: at(f, r) };
    let u = vec![vec![e(1, 0), e(1, 4)], vec![e(2, 0)], vec![e(2, 8)]];
    let got = check_schedules(|pool| parallel(&u, pool));
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].0.file, FileId(1));
    assert_eq!((got[0].1.clone(), got[0].2), (vec![at(1, 0), at(1, 4)], 2));
    assert_eq!((got[1].1.clone(), got[1].2), (vec![at(2, 0), at(2, 8)], 2));
}

#[test]
fn i20_acc_keeps_the_least_three_whatever_the_arrival_order() {
    let at = |r| RefId { file: FileId(1), section: 1, record: r };
    let mut a = mortar_scan::events::Acc::of(at(9));
    for r in [5, 7, 1, 8, 2] {
        a.add(at(r));
    }
    assert_eq!(a.firsts(), &[at(1), at(2), at(5)]);
    assert_eq!(a.count, 6);
}

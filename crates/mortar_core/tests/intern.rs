//! L0: the interner and the numbering of `spec/model.md` §2.3
//! (`mortar_core::intern`): the result equals the sequential definition,
//! written below in twenty lines, for every thread count and scheduling
//! seed (§11), on generated inputs.

mod intern_support;

use std::collections::HashMap as StdHashMap;
use std::hash::Hash;
use std::panic::{AssertUnwindSafe, catch_unwind};

use intern_support::{Rng, Strtabs, workload};
use mortar_core::intern::{
    Full, InternKey, Interner, Mention, Slot, SymName, number, number_sized,
};
use mortar_core::par::{Pool, check_schedules};
use mortar_core::{ComdatId, Id, SymbolId};

/// The sequential definition of §2.3: files in order, each file's keys in
/// order, a key seen for the first time taking the next id. Returns the
/// map (per mention, its id) and the keys in id order.
fn reference<K: Copy + Eq + Hash>(files: &[Vec<K>]) -> (Vec<u32>, Vec<K>) {
    let mut ids = StdHashMap::new();
    let mut keys = Vec::new();
    let mut map = Vec::new();
    for file in files {
        for &key in file {
            let id = *ids.entry(key).or_insert_with(|| {
                keys.push(key);
                keys.len() as u32 - 1
            });
            map.push(id);
        }
    }
    (map, keys)
}

/// What a numbering is compared by: the map and the keys in id order.
fn run<K: InternKey + Hash, I: Id>(pool: &Pool, files: &[Vec<K>]) -> (Vec<u32>, Vec<K>) {
    let counts: Vec<u32> = files.iter().map(|f| f.len() as u32).collect();
    let numbered = number::<K, I, _, _>(pool, &counts, |u| files[u].iter().copied()).expect("fits");
    for (u, file) in files.iter().enumerate() {
        assert_eq!(numbered.unit(u).len(), file.len());
    }
    let keys: Vec<K> = numbered.names.iter().map(|(_, k)| k).collect();
    (numbered.map.iter().map(|id| id.index()).collect(), keys)
}

// ---- §2.3: the key ----------------------------------------------------------

/// §2.3: `foo` and `foo@@V2` are one key; `foo@V1` is another; the split
/// is at the first `@`, wherever it falls in the name.
#[test]
fn s2_3_key_is_the_name_before_at_plus_a_non_default_version() {
    assert_eq!(SymName::parse(b"foo"), SymName::plain(b"foo"));
    assert_eq!(SymName::parse(b"foo@@V2"), SymName::plain(b"foo"));
    assert_eq!(SymName::parse(b"foo@V1"), SymName::versioned(b"foo", b"V1"));
    assert_eq!(SymName::parse(b"foo@V1@x"), SymName::versioned(b"foo", b"V1@x"));
    assert_eq!(SymName::parse(b"@"), SymName::versioned(b"", b""));
    assert_eq!(SymName::parse(b""), SymName::plain(b""));
    assert_ne!(
        InternKey::hash(&SymName::plain(b"foo")),
        InternKey::hash(&SymName::versioned(b"foo", b""))
    );
    // The search runs eight bytes at a time: every position, every length.
    for len in 1..40usize {
        for at in 0..len {
            let mut raw = vec![b'x'; len];
            raw[at] = b'@';
            let key = SymName::parse(&raw);
            assert_eq!(key.name.len(), at, "len {len}, @ at {at}");
            assert_eq!(
                key.version.map(<[u8]>::len),
                if at + 1 < len && raw[at + 1] == b'@' { None } else { Some(len - at - 1) }
            );
        }
        // Bytes that differ from '@' by one bit, and 0x80 bytes, are not '@'.
        let near: Vec<u8> = (0..len).map(|i| [b'A', b'`', 0xc0, 0x80, 0x00, 0x41][i % 6]).collect();
        assert_eq!(SymName::parse(&near), SymName::plain(&near));
    }
}

// ---- §2.3 pass 1: the interner ---------------------------------------------

/// §2.3 pass 1: equal keys get one slot; the slot keeps the minimum
/// mention whatever the insertion order; lookup finds every key.
#[test]
fn s2_3_interner_keeps_the_first_mention() {
    let table = Interner::with_capacity(10);
    let mut ins = table.inserter();
    let m = |unit, ordinal| Mention { unit, ordinal };
    let a = ins.insert(b"a".as_slice(), m(3, 1)).unwrap();
    let b = ins.insert(b"b".as_slice(), m(0, 9)).unwrap();
    assert_eq!(ins.insert(b"a".as_slice(), m(2, 7)).unwrap(), a);
    assert_eq!(ins.insert(b"a".as_slice(), m(2, 8)).unwrap(), a);
    assert_eq!(ins.insert(b"a".as_slice(), m(5, 0)).unwrap(), a);
    assert_ne!(a, b);
    assert_eq!(table.get(&b"b".as_slice()), Some(b));
    assert_eq!(table.get(&b"c".as_slice()), None);
    let frozen = table.freeze();
    assert_eq!(frozen.first(a), m(2, 7));
    assert_eq!(frozen.first(b), m(0, 9));
    assert_eq!(frozen.key(a), b"a");
    assert_eq!(frozen.hash(a), InternKey::hash(&b"a".as_slice()));
    assert_eq!(frozen.get(&b"a".as_slice()), Some(a));
    assert!(frozen.slots() >= 2 && frozen.heap_bytes() > 0);
    assert_eq!(Mention::unpack(m(7, 9).pack()), m(7, 9));
    assert!(m(1, 0) > m(0, u32::MAX));
}

/// §2.3 pass 1: concurrent inserts from many threads give one slot per
/// key, and the minimum mention per key.
#[test]
fn s2_3_interner_concurrent_inserts_agree() {
    let keys: Vec<Vec<u8>> = (0..300).map(|i| format!("k{i}").into_bytes()).collect();
    for pool in
        [Pool::sequential().with_threads(7), Pool::sequential().with_threads(64).with_seed(Some(3))]
    {
        let table = Interner::with_capacity(keys.len());
        let slots: Vec<Vec<Slot>> = pool.map(40, |unit| {
            let mut ins = table.inserter();
            (0..keys.len())
                .map(|i| {
                    let k = (i * 7 + unit * 13) % keys.len();
                    ins.insert(keys[k].as_slice(), Mention { unit: unit as u32, ordinal: i as u32 })
                        .unwrap()
                })
                .collect()
        });
        let frozen = table.freeze();
        for (k, key) in keys.iter().enumerate() {
            let slot = frozen.get(&key.as_slice()).expect("interned");
            // The first mention of key k: the least unit, then its ordinal.
            let ordinal = (0..keys.len()).find(|&i| (i * 7) % keys.len() == k).unwrap();
            assert_eq!(frozen.first(slot), Mention { unit: 0, ordinal: ordinal as u32 });
            for (unit, row) in slots.iter().enumerate() {
                let i = (0..keys.len()).find(|&i| (i * 7 + unit * 13) % keys.len() == k).unwrap();
                assert_eq!(row[i], slot);
            }
        }
    }
}

/// §2.3 pass 1: a table holds what it was sized for and says it is full
/// beyond, rather than probing forever.
#[test]
fn s2_3_interner_reports_full() {
    let table = Interner::with_capacity(1);
    let mut ins = table.inserter();
    let keys: Vec<u64> = (0..1000).collect();
    let mut result = Ok(Slot(0));
    for (i, &k) in keys.iter().enumerate() {
        result = ins.insert(k, Mention { unit: 0, ordinal: i as u32 });
        if result.is_err() {
            break;
        }
    }
    assert_eq!(result, Err(Full));
}

// ---- §2.3: the numbering ------------------------------------------------------

/// The inputs the numbering is tested on: heavy sharing, a file mentioning
/// a key twice, empty files, a key whose first mention is in the last file.
fn files(seed: u64, n: usize) -> Vec<Vec<u64>> {
    let mut rng = Rng(seed);
    let mut files: Vec<Vec<u64>> = (0..n)
        .map(|f| {
            if f % 5 == 3 {
                return Vec::new();
            }
            let len = rng.below(30);
            (0..len)
                .map(|_| {
                    if rng.below(4) == 0 {
                        1000 + f as u64 * 100 + rng.below(100) as u64
                    } else {
                        rng.below(40) as u64
                    }
                })
                .collect()
        })
        .collect();
    files[1].extend([7, 7, 7]);
    files[n - 1].push(999_999);
    files
}

/// §2.3 (§11): the numbering equals the sequential definition for every
/// thread count and seed: integer keys, many small files.
#[test]
fn s2_3_numbering_equals_the_sequential_definition() {
    for (seed, n) in [(1, 2), (2, 7), (3, 64), (4, 300)] {
        let files = files(seed, n);
        let got = check_schedules(|pool| run::<u64, SymbolId>(pool, &files));
        assert_eq!(got, reference(&files), "seed {seed}");
    }
}

/// §2.3 (§11): the same on realistic symbol names, versions included
/// (`foo@@V2` joins `foo`; `foo@V1` is its own key).
#[test]
fn s2_3_numbering_of_symbol_names() {
    let w = workload(5, 120, 40, 300, 20);
    let tabs = Strtabs::new(&w);
    let mut raw: Vec<Vec<Vec<u8>>> =
        (0..w.files.len()).map(|f| tabs.names(f).map(<[u8]>::to_vec).collect()).collect();
    raw[3].push(b"memcpy@@GLIBC_2.14".to_vec());
    raw[4].push(b"memcpy@GLIBC_2.2.5".to_vec());
    raw[5].push(b"memcpy".to_vec());
    raw[119].push(b"only_in_the_last_file".to_vec());
    let files: Vec<Vec<SymName<'_>>> =
        raw.iter().map(|f| f.iter().map(|n| SymName::parse(n)).collect()).collect();
    let got = check_schedules(|pool| run::<SymName<'_>, SymbolId>(pool, &files));
    let (map, keys) = reference(&files);
    assert_eq!(got, (map, keys.clone()));
    let at = |name: SymName<'_>| keys.iter().position(|&k| k == name).unwrap();
    assert!(at(SymName::plain(b"memcpy")) < at(SymName::versioned(b"memcpy", b"GLIBC_2.2.5")));
    assert_eq!(*keys.last().unwrap(), SymName::plain(b"only_in_the_last_file"));
}

/// §2.3: `ComdatId` likewise, over group signatures (plain byte strings).
#[test]
fn s2_3_comdat_numbering_over_signatures() {
    let files: Vec<Vec<&[u8]>> = vec![
        vec![b"_ZN1AC2Ev", b"_ZN1AD2Ev"],
        vec![],
        vec![b"_ZN1AD2Ev", b"_ZTV1A", b"_ZN1AC2Ev"],
        vec![b"_ZTV1A", b"_ZTS1A"],
    ];
    let got = check_schedules(|pool| run::<&[u8], ComdatId>(pool, &files));
    assert_eq!(got, reference(&files));
    assert_eq!(got.0, [0, 1, 1, 2, 0, 2, 3]);
}

/// §2.3: the names table answers lookups by key after the parallel phase
/// (`--defsym`, `-u`, the linker's own symbols), with each id's key, hash
/// and first mention.
#[test]
fn s2_3_names_lookup_by_key() {
    let files = files(9, 50);
    let counts: Vec<u32> = files.iter().map(|f| f.len() as u32).collect();
    let (map, keys) = reference(&files);
    let numbered =
        number::<u64, SymbolId, _, _>(&Pool::sequential().with_threads(7), &counts, |u| {
            files[u].iter().copied()
        })
        .unwrap();
    let names = &numbered.names;
    assert_eq!(names.len(), keys.len());
    assert!(!names.is_empty() && names.heap_bytes() > 0);
    assert_eq!(numbered.offsets.len(), files.len() + 1);
    for (id, &key) in keys.iter().enumerate() {
        let id = SymbolId(id as u32);
        assert_eq!(names.get(&key), Some(id));
        assert_eq!(names.key(id), key);
        assert_eq!(names.hash(id), InternKey::hash(&key));
        let first = names.first(id);
        assert_eq!(files[first.unit as usize][first.ordinal as usize], key);
        let flat = numbered.offsets[first.unit as usize] + first.ordinal as usize;
        assert_eq!(map[..flat].iter().position(|&x| x == id.0), None, "nothing earlier has the id");
    }
    assert_eq!(names.get(&123_456_789), None);
}

/// §2.3: a table sized too small is sized again; the result does not
/// change.
#[test]
fn s2_3_numbering_survives_an_estimate_that_falls_short() {
    let files = files(13, 200);
    let counts: Vec<u32> = files.iter().map(|f| f.len() as u32).collect();
    let expect = reference(&files);
    for pool in [
        Pool::sequential(),
        Pool::sequential().with_threads(7),
        Pool::sequential().with_threads(64).with_seed(Some(1)),
    ] {
        let numbered =
            number_sized::<u64, SymbolId, _, _>(&pool, &counts, |u| files[u].iter().copied(), 1)
                .unwrap();
        let keys: Vec<u64> = numbered.names.iter().map(|(_, k)| k).collect();
        assert_eq!(
            (numbered.map.iter().map(|id| id.0).collect::<Vec<_>>(), keys),
            expect,
            "{pool:?}"
        );
    }
}

/// §2.3: no files, and files with no keys.
#[test]
fn s2_3_numbering_of_nothing() {
    for files in [vec![], vec![Vec::<u64>::new(); 9]] {
        let got = check_schedules(|pool| run::<u64, SymbolId>(pool, &files));
        assert_eq!(got, (vec![], vec![]));
    }
}

/// §2.3: a unit that yields other than its count of keys is a bug of the
/// caller's, and stops.
#[test]
fn s2_3_numbering_rejects_a_wrong_count() {
    for (count, yielded) in [(2u32, 3u64), (3, 2)] {
        let caught = catch_unwind(AssertUnwindSafe(|| {
            number::<u64, SymbolId, _, _>(&Pool::sequential(), &[count], |_| 0..yielded)
                .map(|n| n.map.len())
        }));
        assert!(caught.is_err(), "{count} counted, {yielded} yielded");
    }
}

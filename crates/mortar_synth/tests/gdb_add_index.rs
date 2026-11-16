//! mortar's hash, probe and table size (`spec/synth.md` S-G7) against the
//! bytes gdb's own writer produced: `tests/data/gdb-add-index-v9.bin` is
//! the `.gdb_index` section `gdb-add-index` (gdb 16.3, AArch64) wrote for
//! a static program of one C unit with 800 functions and 500 variables
//! (`int Fn_<i>(int x) { return x + <i>; }`, `int var_<i> = <i>;`, a
//! `main` calling `Fn_3(var_4)`; `gcc -g -O0`, `gcc -static`), 1303
//! names in 2048 slots. Version 9 differs from the version 7 mortar
//! writes only by the shortcut table between the symbol table and the
//! constant pool (header word 6); the symbol table, the hash and the
//! probe are the same.

use mortar_synth::gdbindex::{hash, place, table_size};

const BYTES: &[u8] = include_bytes!("data/gdb-add-index-v9.bin");

fn word(at: usize) -> usize {
    u32::from_le_bytes(BYTES[at..at + 4].try_into().unwrap()) as usize
}

/// One filled slot: (slot, name, CU-vector values).
type Slot = (usize, Vec<u8>, Vec<u32>);

/// The table size and the filled slots.
fn slots() -> (usize, Vec<Slot>) {
    assert_eq!(word(0), 9, "the fixture is version 9");
    let (sym, shortcut, pool) = (word(16), word(20), word(24));
    let n = (shortcut - sym) / 8;
    let mut out = Vec::new();
    for slot in 0..n {
        let (name, vec) = (word(sym + 8 * slot), word(sym + 8 * slot + 4));
        if name == 0 && vec == 0 {
            continue;
        }
        let start = pool + name;
        let end = start + BYTES[start..].iter().position(|&c| c == 0).unwrap();
        let count = word(pool + vec);
        let values = (0..count).map(|k| word(pool + vec + 4 + 4 * k) as u32).collect();
        out.push((slot, BYTES[start..end].to_vec(), values));
    }
    (n, out)
}

// spec: synth.md S-G7 (the size rule is gdb's)
#[test]
fn the_table_size_is_gdbs() {
    let (n, filled) = slots();
    assert_eq!(filled.len(), 1303);
    assert_eq!(table_size(filled.len() as u64), Some(n as u64));
    assert_eq!(table_size(0), Some(1024));
    assert_eq!(table_size(767), Some(1024));
    assert_eq!(table_size(768), Some(2048));
}

// spec: synth.md S-G7 (gdb finds every name where gdb's writer put it
// by mortar's hash and probe sequence)
#[test]
fn every_name_is_where_mortars_probe_looks() {
    let (n, filled) = slots();
    let mask = n - 1;
    let taken: std::collections::BTreeSet<usize> = filled.iter().map(|f| f.0).collect();
    let mut first_probe = 0;
    for (slot, name, _) in &filled {
        let h = hash(name) as usize;
        let step = (h.wrapping_mul(17) & mask) | 1;
        let mut at = h & mask;
        first_probe += usize::from(at == *slot);
        while at != *slot {
            assert!(
                taken.contains(&at),
                "{:?}: an empty slot before its own",
                String::from_utf8_lossy(name)
            );
            at = (at + step) & mask;
        }
    }
    // Most names sit at their first probe: the hash itself is gdb's.
    assert!(first_probe * 2 > filled.len(), "{first_probe} of {}", filled.len());
}

// spec: synth.md S-G7 (placing the names by mortar's probe gives each
// one a slot gdb's lookup reaches)
#[test]
fn placing_gdbs_names_reaches_each_by_its_probe() {
    let (n, filled) = slots();
    let hashes: Vec<u32> = filled.iter().map(|f| hash(&f.1)).collect();
    let table = place(&hashes, n as u64);
    let mask = (n - 1) as u32;
    for (i, &h) in hashes.iter().enumerate() {
        let step = (h.wrapping_mul(17) & mask) | 1;
        let mut at = h & mask;
        while table[at as usize] != i as u32 {
            assert_ne!(table[at as usize], u32::MAX, "name {i} unreachable");
            at = (at + step) & mask;
        }
    }
}

// spec: synth.md S-G4 (CU vectors sorted and unique, as gdb writes them)
#[test]
fn gdbs_vectors_are_sorted_and_unique() {
    for (_, name, values) in slots().1 {
        assert!(
            values.windows(2).all(|w| w[0] < w[1]),
            "{:?}: {values:x?}",
            String::from_utf8_lossy(&name)
        );
        assert!(!values.is_empty());
    }
}

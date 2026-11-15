//! The reader never panics and is deterministic on any input (WP-071
//! scope 1): valid units mutated at random (byte flips, overwritten
//! fields, truncation, splices), read twice. No third-party crate: a
//! xorshift generator with fixed seeds, so a failure reproduces.

use std::fmt::Write as _;

use mortar_meta::read::SectionAttrs;
use mortar_meta::write::{Declaration, Definition, Summary, Type, section};
use mortar_meta::{Finding, UnitBuilder, class, fnv1a64, producer, read_section};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
}

fn unit(rng: &mut Rng) -> Vec<u8> {
    let mut b = UnitBuilder::new(producer::TEST);
    for i in 0..rng.below(4) {
        b.summary.push(Summary {
            name: format!("k{i}").into_bytes(),
            value: b"v".to_vec(),
            flags: 1,
        });
    }
    for i in 0..rng.below(6) {
        let n = format!("_Z{i}fv").into_bytes();
        b.definitions.push(Definition {
            key: fnv1a64(&n),
            hash: [rng.next() as u8; 16],
            name: (i % 2 == 0).then_some(n),
            class: class::FUNCTION,
            flags: 0,
            module: Vec::new(),
            file: b"f.cc".to_vec(),
            line: i as u32,
        });
    }
    for i in 0..rng.below(3) {
        b.types.push(Type::named(format!("N1T{i}E").as_bytes()));
    }
    for i in 0..rng.below(4) {
        b.declarations.push(Declaration {
            key: rng.next(),
            ty: 0,
            size: 0,
            name: None,
            type_name: format!("t{i}").into_bytes(),
            flags: 0,
        });
    }
    if rng.below(3) == 0 {
        b.details = Some(vec![rng.next() as u8; rng.below(20)]);
    }
    b.finish().unwrap()
}

fn digest(units: &[mortar_meta::Unit<'_>], findings: &[Finding]) -> String {
    let mut out = format!("{findings:?}");
    for u in units {
        let _ = write!(out, "|{}:{}", u.section, u.offset);
        for r in u.definitions().iter() {
            let _ = write!(out, " {:x}/{:?}", r.key, u.string(r.name));
        }
        for r in u.summary().iter() {
            let _ = write!(out, " {:?}={:?}", u.string(r.name), u.string(r.value));
        }
        for r in u.types().iter() {
            let _ = write!(out, " {:?}", u.string(r.name));
        }
        for r in u.declarations().iter() {
            let _ = write!(out, " {:?}", u.string(r.type_name));
        }
        let _ = write!(out, " {:?}", u.details());
        for b in u.blocks() {
            let _ = write!(out, " {b:?}");
        }
    }
    out
}

#[test]
fn mutated_units_never_panic() {
    let attrs = SectionAttrs::conforming(1);
    for seed in 1..=4000u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let n = 1 + rng.below(3);
        let units: Vec<Vec<u8>> = (0..n).map(|_| unit(&mut rng)).collect();
        let mut bytes = section(&units, rng.below(2));
        for _ in 0..=rng.below(6) {
            match rng.below(6) {
                0 => {
                    let i = rng.below(bytes.len());
                    bytes[i] ^= 1 << rng.below(8);
                }
                1 => {
                    // Overwrite an aligned u32 with an edge value.
                    let i = rng.below(bytes.len() / 4) * 4;
                    let v: u32 = [0, 1, 8, 16, 32, 0xffff_ffff, 0x8000_0000, 7][rng.below(8)];
                    if i + 4 <= bytes.len() {
                        bytes[i..i + 4].copy_from_slice(&v.to_le_bytes());
                    }
                }
                2 => {
                    let at = rng.below(bytes.len() + 1);
                    bytes.truncate(at);
                }
                3 => {
                    let i = rng.below(bytes.len());
                    bytes[i] = rng.next() as u8;
                }
                4 => {
                    let at = rng.below(bytes.len() + 1);
                    let extra = vec![rng.next() as u8; rng.below(24)];
                    bytes.splice(at..at, extra);
                }
                _ => {
                    let i = rng.below(bytes.len() / 2) * 2;
                    if i + 2 <= bytes.len() {
                        bytes[i..i + 2].copy_from_slice(&(rng.next() as u16).to_le_bytes());
                    }
                }
            }
            if bytes.is_empty() {
                break;
            }
        }
        let (u1, f1) = read_section(attrs, &bytes);
        let (u2, f2) = read_section(attrs, &bytes);
        assert_eq!(digest(&u1, &f1), digest(&u2, &f2), "seed {seed}");
        // Every finding renders.
        for f in &f1 {
            let _ = f.reason();
        }
        // Lookups on whatever was read never panic either.
        for u in &u1 {
            let d = u.definitions();
            let _ = d.with_key(rng.next());
            let _ = u.summary_value(b"k1");
            let _ = u.string(rng.next() as u32);
            let _ = u.types().with_key(0);
            let _ = d.get(usize::MAX);
        }
    }
}

#[test]
fn random_bytes_never_panic() {
    let attrs = SectionAttrs::conforming(1);
    let mut rng = Rng(0x1234_5678_9abc_def1);
    for _ in 0..4000 {
        let len = rng.below(200);
        let mut bytes: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        if len >= 4 && rng.below(2) == 0 {
            bytes[..4].copy_from_slice(b"MASN");
        }
        let _ = read_section(attrs, &bytes);
    }
}

//! Debug references to empty sections not in the output (spec/plumb.md
//! §3.3, WP-156b), on checked-in links (`tests/gc_empty/gen.sh`):
//! collected by `--gc-sections` (`p`) or a COMDAT copy not kept (`c`, the
//! clang `-g` bench link's shape). Their words are judged as any such
//! section's, against I-47's tombstone (GNU ld: or the kept copy), and a
//! live empty section's must be its address. Before WP-156b both were R5
//! "refers to empty section ... at 0x0" on mortar's and lld's outputs.

use mortar_plumb::{Ctx, Files, Format, Input, Map, Report, map, verify_with};
use std::path::{Path, PathBuf};

const ARCHES: [&str; 2] = ["aarch64", "x86_64"];
/// The links: output, map, inputs, line.
const P: &str = "-static --gc-sections e.o";
const C: &str = "-static s.o c1.o c2.o";
const LINKS: [(&str, &str, &str); 6] = [
    ("p", "p.map", P),
    ("p.gnu", "p.gnu.map", P),
    ("p.lld", "p.lld.map", P),
    ("c", "c.map", C),
    ("c.gnu", "c.gnu.map", C),
    ("c.lld", "c.lld.map", C),
];

struct Dir(PathBuf);

impl Files for Dir {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(path)).ok()
    }
}

fn dir(arch: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gc_empty").join(arch)
}

fn read_map(arch: &str, name: &str) -> Map {
    let format = if name.ends_with(".gnu.map") {
        Format::Gnu
    } else if name.ends_with(".lld.map") {
        Format::Lld
    } else {
        Format::Mortar
    };
    let text = std::fs::read_to_string(dir(arch).join(name)).unwrap();
    map::parse(format, &text).unwrap()
}

fn verify(arch: &str, out: &[u8], m: &Map, line: &str) -> Report {
    let d = dir(arch);
    let bytes: Vec<(&str, Vec<u8>)> = line
        .split(' ')
        .filter(|w| Path::new(w).extension().is_some_and(|e| e == "o"))
        .map(|p| (p, std::fs::read(d.join(p)).unwrap()))
        .collect();
    let inputs: Vec<Input<'_>> = bytes.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
    let files = Dir(d);
    let line = (m.format != Format::Mortar).then(|| line.split(' ').map(str::to_string).collect());
    verify_with(out, &inputs, m, &Ctx { line, files: &files })
}

fn findings(r: &Report) -> Vec<String> {
    r.findings.iter().map(|f| format!("{} {}", f.rule.id(), f.message)).collect()
}

/// The file offset of the output's section `name` (ELF64, little-endian).
fn section(out: &[u8], name: &str) -> usize {
    let u = |at: usize, n: usize| -> u64 {
        out[at..at + n].iter().rev().fold(0, |a, &b| a << 8 | u64::from(b))
    };
    let shoff = u(0x28, 8) as usize;
    let shnum = u(0x3c, 2) as usize;
    let strndx = u(0x3e, 2) as usize;
    let sh = |i: usize| shoff + i * 64;
    let names = u(sh(strndx) + 0x18, 8) as usize;
    (0..shnum)
        .find(|&i| {
            let n = names + u(sh(i), 4) as usize;
            out[n..].starts_with(name.as_bytes()) && out[n + name.len()] == 0
        })
        .map(|i| u(sh(i) + 0x18, 8) as usize)
        .unwrap()
}

fn put(out: &mut [u8], at: usize, v: u64) {
    out[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

/// The six links are clean.
#[test]
fn s3_3_empty_sections_not_in_the_output_clean() {
    for arch in ARCHES {
        for (out, m, line) in LINKS {
            let bytes = std::fs::read(dir(arch).join(out)).unwrap();
            let r = verify(arch, &bytes, &read_map(arch, m), line);
            assert!(r.findings.is_empty(), "{arch} {out}: {:?}", findings(&r));
        }
    }
}

/// The mutations: a collected empty section's word given the live one's
/// address, or the wrong tombstone; a live empty section's word given a
/// tombstone. Each is R1 on every map: the collected one's against I-47's
/// tombstone, the live one's against its address.
#[test]
fn s3_3_collected_and_live_empty_sections_mutated() {
    for arch in ARCHES {
        for (out, m, line) in &LINKS[..3] {
            let map = read_map(arch, m);
            let bytes = std::fs::read(dir(arch).join(out)).unwrap();
            let addr = section(&bytes, ".debug_addr");
            let live = u64::from_le_bytes(bytes[addr + 8..addr + 16].try_into().unwrap());
            assert!(live > 0x10000, "{arch} {out}: {live:#x}");
            let ranges = section(&bytes, ".debug_ranges");
            let tomb = "R1 the reference to a section not in the output holds another value than the tombstone";
            let wrong = "R1 the relocated field holds the wrong value";
            for (what, at, v, want) in [
                ("the collected empty section's word, an address", addr, live, tomb),
                ("the collected empty section's word, all ones", addr, u64::MAX, tomb),
                ("the live empty section's word, the tombstone", addr + 8, 0, wrong),
                // Every linker's .debug_ranges tombstone is 1 (I-47's table).
                ("the collected empty section's .debug_ranges word, 0", ranges, 0, tomb),
            ] {
                let mut b = bytes.clone();
                put(&mut b, at, v);
                let r = verify(arch, &b, &map, line);
                let f = findings(&r);
                assert_eq!(f, [want], "{arch} {out} {what}");
            }
        }
    }
}

/// The COMDAT copy not kept: its word given an address that is not the
/// kept copy's, or the wrong tombstone, is R1; GNU ld's kept-copy
/// redirect is the one address it may hold.
#[test]
fn s3_3_empty_comdat_copy_mutated() {
    let tomb =
        "R1 the reference to a section not in the output holds another value than the tombstone";
    let gnu = "R1 a reference into a dropped COMDAT copy does not hold the linker's kept section + st_value + A (spec/plumb.md §5.8)";
    for arch in ARCHES {
        for (out, m, line) in &LINKS[3..] {
            let map = read_map(arch, m);
            let bytes = std::fs::read(dir(arch).join(out)).unwrap();
            let addr = section(&bytes, ".debug_addr");
            let kept = u64::from_le_bytes(bytes[addr..addr + 8].try_into().unwrap());
            for (what, v) in [("an address", kept + 0x100), ("all ones", u64::MAX), ("1", 1)] {
                let mut b = bytes.clone();
                put(&mut b, addr + 8, v);
                // GNU ld's redirect is judged as a kept copy (§5.8, WP-250).
                let want = if m.contains(".gnu.") {
                    format!(
                        "{gnu}{}",
                        if what == "an address" { "" } else { "; it holds a tombstone" }
                    )
                } else {
                    tomb.to_string()
                };
                assert_eq!(findings(&verify(arch, &b, &map, line)), [want], "{arch} {out} {what}");
            }
        }
    }
}

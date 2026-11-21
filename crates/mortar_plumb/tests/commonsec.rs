//! An input section named exactly `COMMON` (WP-210, WP-203's verifier
//! gaps; `spec/plumb.md` §3.3 "An input section named `COMMON`", §10.2
//! A129-A131): lld's, GNU ld's and mold's `-nostdlib` static links,
//! checked in under `tests/commonsec/` (`gen.sh`), on AArch64 and x86-64.

use std::path::{Path, PathBuf};

use mortar_plumb::{Ctx, Files, Format, Input, Report};

struct Dir<'a>(&'a Path);

impl Files for Dir<'_> {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(path)).ok()
    }
}

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/commonsec")
}

const LINKERS: &[(&str, Format)] =
    &[("lld", Format::Lld), ("bfd", Format::Gnu), ("mold", Format::Mold)];

/// The verifier on fixture `name` (`{cell}-{arch}.{linker}`), with `bytes`
/// as the output.
fn verify(name: &str, format: Format, bytes: &[u8]) -> Report {
    let d = dir();
    let text = std::fs::read_to_string(d.join(format!("{name}.map"))).unwrap();
    let map = mortar_plumb::map::parse(format, &text).unwrap();
    let inputs: Vec<(String, Vec<u8>)> = map
        .paths()
        .into_iter()
        .filter_map(|p| std::fs::read(d.join(p)).ok().map(|b| (p.to_string(), b)))
        .collect();
    let inputs: Vec<Input<'_>> = inputs.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
    let line = std::fs::read_to_string(d.join(format!("{name}.line")))
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    mortar_plumb::verify_with(bytes, &inputs, &map, &Ctx { line: Some(line), files: &Dir(&d) })
}

/// The findings that are no anomaly of the format, and the anomalies' ids.
fn judge(name: &str, format: Format, bytes: &[u8]) -> (Vec<String>, Vec<String>) {
    let r = verify(name, format, bytes);
    let mut plain = Vec::new();
    let mut named = Vec::new();
    for f in &r.findings {
        match mortar_plumb::anomaly::classify(format, bytes, f) {
            Some(a) => named.push(a.split(':').next().unwrap().to_string()),
            None => plain.push(format!("{} {} @{:x?}", f.rule.id(), f.message, f.addr)),
        }
    }
    named.sort();
    named.dedup();
    (plain, named)
}

fn bytes(name: &str) -> Vec<u8> {
    std::fs::read(dir().join(name)).unwrap()
}

const CELLS: &[&str] = &["nobits", "progbits", "dotx", "split", "gc"];

#[test]
#[ignore = "prints every fixture's findings"]
fn survey() {
    for a in ["a64", "x64"] {
        for c in CELLS {
            for (l, format) in LINKERS {
                let n = format!("{c}-{a}.{l}");
                let b = bytes(&n);
                eprintln!("== {n}");
                for f in &verify(&n, *format, &b).findings {
                    let x = mortar_plumb::anomaly::classify(*format, &b, f)
                        .map_or("-", |a| a.split(':').next().unwrap());
                    eprintln!("   {x} {} {} @{:x?}", f.rule.id(), f.message, f.addr);
                }
            }
        }
    }
}

// ---- ELF64 little-endian reads and edits ----

fn le16(b: &[u8], o: usize) -> usize {
    u16::from_le_bytes([b[o], b[o + 1]]) as usize
}
fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn le64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
fn cstr(b: &[u8], o: usize) -> String {
    let e = b[o..].iter().position(|&c| c == 0).unwrap();
    String::from_utf8_lossy(&b[o..o + e]).into_owned()
}

/// `(name, sh_type, sh_addr, sh_offset, sh_size, sh_link)` per section.
fn sections(b: &[u8]) -> Vec<(String, u32, u64, usize, u64, usize)> {
    let shoff = le64(b, 0x28) as usize;
    let (n, ndx) = (le16(b, 0x3c), le16(b, 0x3e));
    let sh = |i: usize| shoff + i * 64;
    let strs = le64(b, sh(ndx) + 0x18) as usize;
    (0..n)
        .map(|i| {
            let h = sh(i);
            (
                cstr(b, strs + le32(b, h) as usize),
                le32(b, h + 4),
                le64(b, h + 0x10),
                le64(b, h + 0x18) as usize,
                le64(b, h + 0x20),
                le32(b, h + 0x28) as usize,
            )
        })
        .collect()
}

/// Sets every `.symtab` entry named `name` to `value`.
fn set_symbol(b: &mut [u8], name: &str, value: u64) {
    let s = sections(b);
    let t = s.iter().find(|x| x.1 == 2).unwrap();
    let strtab = s[t.5].3;
    let mut n = 0;
    for i in 0..(t.4 as usize) / 24 {
        let at = t.3 + i * 24;
        if cstr(b, strtab + le32(b, at) as usize) == name {
            b[at + 8..at + 16].copy_from_slice(&value.to_le_bytes());
            n += 1;
        }
    }
    assert!(n > 0, "no symbol {name}");
}

/// The first allocated section named `name`: `(sh_addr, sh_size)`.
fn sec(b: &[u8], name: &str) -> (u64, u64) {
    let x = sections(b).into_iter().find(|x| x.0 == name && x.2 != 0).unwrap();
    (x.2, x.4)
}

/// The anomalies each fixture has (beyond mold's run-wide A3 and A9, which
/// every one of its static links has): every finding is one of them, and
/// each occurs.
fn expected(cell: &str, arch: &str, linker: &str) -> &'static [&'static str] {
    match (cell, arch, linker) {
        ("progbits" | "split", _, "bfd" | "lld") => &["A129"],
        ("split", _, "mold") => &["A131"],
        ("dotx", "a64", "bfd") => &["A130"],
        ("gc", _, "mold") => &["A11"],
        _ => &[],
    }
}

// §3.3 "An input section named `COMMON`" and §10.2 A129-A131 (WP-210,
// WP-203's verifier gaps): every finding on the three references' links
// of an input `COMMON` (NOBITS, PROGBITS, both, with an orphan
// `COMMON.x`, under --gc-sections) is a named anomaly, and the fixture's
// anomalies are exactly the expected ones. Before WP-210: R5 on every
// record into the section (all three), Y5 on GNU ld's `_edata` and
// `__bss_start`, its AArch64 `__bss_end__`, lld's `__bss_start` and
// mold's `__start_COMMON`.
fn judge_cells(a: &str) {
    for c in CELLS {
        for (l, format) in LINKERS {
            let n = format!("{c}-{a}.{l}");
            let b = bytes(&n);
            let (plain, mut named) = judge(&n, *format, &b);
            assert!(plain.is_empty(), "{n}: {plain:#?}");
            if *l == "mold" {
                named.retain(|x| x != "A3" && x != "A9");
            }
            assert_eq!(named, expected(c, a, l), "{n}");
        }
    }
}

#[test]
fn s3_3_input_common_section_aarch64() {
    judge_cells("a64");
}

#[test]
fn s3_3_input_common_section_x86_64() {
    judge_cells("x64");
}

// §10.2 A129-A131 are strict: a value outside each one's form is a
// finding no anomaly names. GNU ld's `_edata` inside the PROGBITS `.bss`;
// its `__bss_end__` 8 before `.bss`'s end; mold's `__start_COMMON` 8
// into the first `COMMON`; lld's `__bss_start` 8 into `.bss`.
#[test]
fn s10_2_a129_a131_are_strict() {
    type Case = (&'static str, Format, &'static str, fn(&[u8]) -> u64);
    let cases: [Case; 4] = [
        ("progbits-a64.bfd", Format::Gnu, "_edata", |b| sec(b, ".bss").0 + 8),
        ("dotx-a64.bfd", Format::Gnu, "__bss_end__", |b| {
            let (a, z) = sec(b, ".bss");
            a + z - 8
        }),
        ("split-x64.mold", Format::Mold, "__start_COMMON", |b| sec(b, "COMMON").0 + 8),
        ("progbits-x64.lld", Format::Lld, "__bss_start", |b| sec(b, ".bss").0 + 8),
    ];
    for (n, format, name, value) in cases {
        let mut b = bytes(n);
        let v = value(&b);
        set_symbol(&mut b, name, v);
        let (plain, _) = judge(n, format, &b);
        assert!(
            plain.iter().any(|f| f.starts_with("Y5 ") && f.contains(&format!("'{name}'"))),
            "{n}: {name} = {v:#x}: {plain:#?}"
        );
    }
}

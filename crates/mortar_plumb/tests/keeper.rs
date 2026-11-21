//! The COMDAT keeper among one archive's members (WP-210, item 9;
//! `spec/plumb.md` §5.7a): the members of one archive share its rank on
//! the line, so the map's order decides among them. Measured on GNU ld's,
//! lld's, mold's and mortar's links checked in under `tests/keeper/`
//! (`gen.sh`), on AArch64 and x86-64: GNU ld and lld keep the copy of the
//! member they extract first and list it first; mold and mortar keep the
//! copy of the first member in the archive (mortar: the least `FileId`,
//! `spec/resolve.md` §6.1) and list it first. The verifier holds each to
//! its own keeper and finds nothing (mold's run-wide A3 and A9 aside).

use std::path::{Path, PathBuf};

use mortar_plumb::{Ctx, Files, Format, Input, Report};

struct Dir<'a>(&'a Path);

impl Files for Dir<'_> {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(path)).ok()
    }
}

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/keeper")
}

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

// ---- ELF64 little-endian reads ----

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

/// The first byte of `.symtab`'s `g` that tells the copies apart: the
/// immediate of m1.o's `mov w0, #1` / `movl $1, %eax` or m2.o's `#2`.
#[allow(clippy::many_single_char_names)] // b, s, t, g, x, o: the ELF parts
fn g_copy(b: &[u8], arch: &str) -> u8 {
    let s = sections(b);
    let t = s.iter().find(|x| x.1 == 2).unwrap();
    let strtab = s[t.5].3;
    let g = (0..(t.4 as usize) / 24)
        .map(|i| t.3 + i * 24)
        .find(|&at| cstr(b, strtab + le32(b, at) as usize) == "g")
        .map(|at| le64(b, at + 8))
        .unwrap();
    let x = s.iter().find(|x| x.2 <= g && g < x.2 + x.4 && x.1 == 1).unwrap();
    let o = x.3 + (g - x.2) as usize;
    match arch {
        // MOVZ w0, #imm16: imm16 in bits 5..21.
        "a64" => ((le32(b, o) >> 5) & 0xffff) as u8,
        // B8 imm32.
        _ => b[o + 1],
    }
}

/// Whether `.dynstr` names `libh-<arch>.so` (it is `DT_NEEDED`: the
/// output's only library).
fn needs_libh(b: &[u8], arch: &str) -> bool {
    let want = format!("libh-{arch}.so");
    sections(b)
        .iter()
        .filter(|x| x.0 == ".dynstr")
        .any(|x| b[x.3..x.3 + x.4 as usize].split(|&c| c == 0).any(|w| w == want.as_bytes()))
}

/// `(output kind, linker, format, the copy of g it keeps, libh.so needed)`.
/// lld refuses the pie link ("relocation refers to a symbol in a
/// discarded section: h": it keeps m2.o's copy, and m1.o's h is in the
/// copy it drops).
const CELLS: &[(&str, &str, Format, u8, bool)] = &[
    ("st", "bfd", Format::Gnu, 2, false),
    ("st", "lld", Format::Lld, 2, false),
    ("st", "mold", Format::Mold, 1, false),
    ("st", "mortar", Format::Mortar, 1, false),
    ("pie", "bfd", Format::Gnu, 2, true),
    ("pie", "mold", Format::Mold, 1, false),
    ("pie", "mortar", Format::Mortar, 1, false),
];

// §5.7a (WP-210): among one archive's members the map's order decides the
// keeper. GNU ld and lld keep the first member they extract (m2.o) and
// GNU ld imports h from libh.so, which it makes needed; mold and mortar
// keep the archive's first member (m1.o) and define h. The verifier finds
// nothing on any of them. Reversing the order among members that share a
// rank (keeping the LAST in the map's order) gives findings on every one.
fn keeper(arch: &str) {
    for (k, l, format, copy, needed) in CELLS {
        let n = format!("{k}-{arch}.{l}");
        let b = std::fs::read(dir().join(&n)).unwrap();
        assert_eq!(g_copy(&b, arch), *copy, "{n}: the copy of g it keeps");
        if *k == "pie" {
            assert_eq!(needs_libh(&b, arch), *needed, "{n}: libh.so needed");
        }
        let plain: Vec<String> = verify(&n, *format, &b)
            .findings
            .iter()
            .filter(|f| {
                let a = mortar_plumb::anomaly::classify(*format, &b, f);
                !matches!(a.map(|a| a.split(':').next().unwrap()), Some("A3" | "A9"))
            })
            .map(|f| format!("{} {} @{:x?}", f.rule.id(), f.message, f.addr))
            .collect();
        assert!(plain.is_empty(), "{n}: {plain:#?}");
    }
}

#[test]
fn s5_7a_keeper_among_archive_members_aarch64() {
    keeper("a64");
}

#[test]
fn s5_7a_keeper_among_archive_members_x86_64() {
    keeper("x64");
}

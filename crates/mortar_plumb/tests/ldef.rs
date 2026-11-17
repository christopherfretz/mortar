//! dyn.md D-110 item 2 and D-21 item 2 as WP-150 amended them, under the
//! verifier (WP-166): mortar's, lld's, GNU ld's and mold's `-nostdlib`
//! links of D-21 item 2's seven linker-defined names, checked in under
//! `tests/ldef/` (`gen.sh`), on AArch64 and x86-64.
#![allow(clippy::many_single_char_names)]

use std::path::{Path, PathBuf};

use mortar_plumb::{Ctx, Files, Format, Input, Report};

struct Dir<'a>(&'a Path);

impl Files for Dir<'_> {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(path)).ok()
    }
}

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ldef")
}

/// One fixture: its output name, map, the line (a reference's) and format.
struct Fx {
    name: String,
    format: Format,
}

impl Fx {
    fn mortar(name: &str) -> Fx {
        Fx { name: name.into(), format: Format::Mortar }
    }
    fn reference(name: &str, linker: &str) -> Fx {
        let format = match linker {
            "lld" => Format::Lld,
            "mold" => Format::Mold,
            _ => Format::Gnu,
        };
        Fx { name: format!("{name}.{linker}"), format }
    }
    fn bytes(&self) -> Vec<u8> {
        std::fs::read(dir().join(&self.name)).unwrap()
    }
    fn verify(&self, bytes: &[u8]) -> Report {
        let d = dir();
        let text = std::fs::read_to_string(d.join(format!("{}.map", self.name))).unwrap();
        let map = mortar_plumb::map::parse(self.format, &text).unwrap();
        let inputs: Vec<(String, Vec<u8>)> = map
            .paths()
            .into_iter()
            .filter_map(|p| std::fs::read(d.join(p)).ok().map(|b| (p.to_string(), b)))
            .collect();
        let inputs: Vec<Input<'_>> =
            inputs.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
        let line = (self.format != Format::Mortar).then(|| {
            std::fs::read_to_string(d.join(format!("{}.line", self.name)))
                .unwrap()
                .lines()
                .map(str::to_string)
                .collect()
        });
        mortar_plumb::verify_with(bytes, &inputs, &map, &Ctx { line, files: &Dir(&d) })
    }
    /// The findings that are no anomaly of the format, and the anomalies'
    /// ids.
    fn judge(&self, bytes: &[u8]) -> (Vec<String>, Vec<String>) {
        let r = self.verify(bytes);
        let mut plain = Vec::new();
        let mut named = Vec::new();
        for f in &r.findings {
            match mortar_plumb::anomaly::classify(self.format, bytes, f) {
                Some(a) => named.push(a.split(':').next().unwrap().to_string()),
                None => plain.push(format!("{} {} @{:x?}", f.rule.id(), f.message, f.addr)),
            }
        }
        named.sort();
        named.dedup();
        (plain, named)
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
fn sections(b: &[u8]) -> Vec<(String, u32, u64, usize, usize, usize)> {
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
                le64(b, h + 0x20) as usize,
                le32(b, h + 0x28) as usize,
            )
        })
        .collect()
}

fn section<'s>(
    s: &'s [(String, u32, u64, usize, usize, usize)],
    name: &str,
) -> &'s (String, u32, u64, usize, usize, usize) {
    s.iter().find(|x| x.0 == name).unwrap_or_else(|| panic!("no {name}"))
}

/// The file offset of address `a` (a section that holds it).
fn file_of(b: &[u8], a: u64) -> usize {
    let s = sections(b);
    let x = s
        .iter()
        .find(|x| x.1 != 8 && x.2 != 0 && x.2 <= a && a < x.2 + x.4 as u64)
        .unwrap_or_else(|| panic!("no section holds {a:#x}"));
    x.3 + (a - x.2) as usize
}


const CELLS: &[&str] = &[
    "sh", "shsym", "shfn", "shloc", "shx", "exref-pie", "exref-nopie", "exdef-pie", "exdef-nopie", "exdefr-pie", "exdefr-nopie",
    "pie-e",
];

#[test]
#[ignore = "prints every fixture's findings"]
fn survey() {
    for a in ["a64", "x64"] {
        for c in CELLS {
            let n = format!("{c}-{a}");
            let mut fxs = vec![Fx::mortar(&n)];
            for l in ["lld", "bfd", "mold"] {
                fxs.push(Fx::reference(&n, l));
            }
            for fx in fxs {
                let b = fx.bytes();
                eprintln!("== {}", fx.name);
                for f in &fx.verify(&b).findings {
                    let a = mortar_plumb::anomaly::classify(fx.format, &b, f)
                        .map_or("-", |a| a.split(':').next().unwrap());
                    eprintln!("   {a} {} {} @{:x?}", f.rule.id(), f.message, f.addr);
                }
            }
        }
    }
}

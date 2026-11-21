//! §6.5 Y5's closed set (WP-210, WP-246's follow-ups): a name a `-T`
//! script of the line assigns, and a name `.dynsym` has. GNU ld's, lld's
//! and mold's links checked in under `tests/y5names/` (`gen.sh`), on
//! AArch64 and x86-64.

use std::path::{Path, PathBuf};

use mortar_plumb::{Ctx, Files, Format, Input, Report};

struct Dir<'a>(&'a Path);

impl Files for Dir<'_> {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(path)).ok()
    }
}

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/y5names")
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

/// The Y5 findings, `"<message>"`.
fn y5(r: &Report) -> Vec<String> {
    r.findings.iter().filter(|f| f.rule.id() == "Y5").map(|f| f.message.clone()).collect()
}

fn bytes(name: &str) -> Vec<u8> {
    std::fs::read(dir().join(name)).unwrap()
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

/// `(sh_type, sh_offset, sh_size, sh_link)` per section.
fn sections(b: &[u8]) -> Vec<(u32, usize, u64, usize)> {
    let shoff = le64(b, 0x28) as usize;
    let n = le16(b, 0x3c);
    (0..n)
        .map(|i| {
            let h = shoff + i * 64;
            (
                le32(b, h + 4),
                le64(b, h + 0x18) as usize,
                le64(b, h + 0x20),
                le32(b, h + 0x28) as usize,
            )
        })
        .collect()
}

/// The `.symtab` entries named `name`: their offsets in `b`.
fn symtab_entries(b: &[u8], name: &str) -> Vec<usize> {
    let s = sections(b);
    let t = s.iter().find(|x| x.0 == 2).unwrap();
    let strtab = s[t.3].1;
    (0..(t.2 as usize) / 24)
        .map(|i| t.1 + i * 24)
        .filter(|&at| cstr(b, strtab + le32(b, at) as usize) == name)
        .collect()
}

/// Renames `.symtab`'s `from` to `to` (one length) in `.strtab`.
fn rename(b: &mut [u8], from: &str, to: &str) {
    assert_eq!(from.len(), to.len());
    let s = sections(b);
    let t = s.iter().find(|x| x.0 == 2).unwrap();
    let strtab = s[t.3].1;
    let at = symtab_entries(b, from)[0];
    let o = strtab + le32(b, at) as usize;
    b[o..o + to.len()].copy_from_slice(to.as_bytes());
}

const ARCHES: [&str; 2] = ["a64", "x64"];

// Y5 for a line with a linker script (WP-210): the names the line's
// `-T` scripts assign (`my_abs`, top level; `my_end`, in SECTIONS;
// `my_free`, which no input refers to) are listed "the line's script
// assigns it", not judged; any other name no input has is a finding.
// Before WP-210 the clause "any name when the line has a script" read
// mortar's accepted options only, and mortar refuses `-T`: it never
// applied, and a reference's `-T` line judged every one of them (3 Y5
// findings on GNU ld's; lld's are at no listed input section, which
// lists them). A defined name the script does not assign (GNU ld's
// `my_free` renamed `my_frex`) is a finding; under lld's map the
// address rule (a name at no listed input section) still lists it.
#[test]
fn s6_5_y5_names_a_script_assigns() {
    for a in ARCHES {
        for (l, format) in [("bfd", Format::Gnu), ("lld", Format::Lld)] {
            let n = format!("st-{a}.{l}");
            let b = bytes(&n);
            let r = verify(&n, format, &b);
            assert!(r.findings.is_empty(), "{n}: {:#?}", r.findings);
            for x in ["my_abs", "my_end", "my_free"] {
                let want = format!("{x} (the line's script assigns it)");
                assert!(
                    r.coverage.linker_defined_not_judged.contains(&want),
                    "{n}: {:?}",
                    r.coverage.linker_defined_not_judged
                );
            }
        }
        let n = format!("st-{a}.bfd");
        let mut b = bytes(&n);
        rename(&mut b, "my_free", "my_frex");
        let f = y5(&verify(&n, Format::Gnu, &b));
        assert!(
            f.iter()
                .any(|m| m.contains("('my_frex') is defined, but no input symbol has the name")),
            "{n}: {f:#?}"
        );
    }
}

// Y5 for a name also in `.dynsym` (WP-210): the copied `dv` (libd.so
// defines it, no input does) is in `.symtab` at its `.dynsym` entry's
// value in every reference's link; its `.symtab` value moved by 8 is a
// finding. Before WP-210 a name `.dynsym` has was passed over whatever
// `.symtab` said.
#[test]
fn s6_5_y5_a_dynsym_name_is_its_symbol() {
    for a in ARCHES {
        for (l, format) in [("bfd", Format::Gnu), ("lld", Format::Lld), ("mold", Format::Mold)] {
            let n = format!("cp-{a}.{l}");
            let mut b = bytes(&n);
            let f = y5(&verify(&n, format, &b));
            assert!(f.is_empty(), "{n}: {f:#?}");
            let at = symtab_entries(&b, "dv");
            assert!(!at.is_empty(), "{n}: dv in .symtab");
            for o in at {
                let v = le64(&b, o + 8) + 8;
                b[o + 8..o + 16].copy_from_slice(&v.to_le_bytes());
            }
            let f = y5(&verify(&n, format, &b));
            assert!(
                f.iter().any(|m| m.contains("('dv') is at") && m.contains(".dynsym's 'dv'")),
                "{n}: {f:#?}"
            );
        }
    }
}

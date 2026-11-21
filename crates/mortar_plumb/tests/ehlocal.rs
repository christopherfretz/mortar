//! §6.5 Y6 for the locals of an input `.eh_frame` (WP-210, WP-246's
//! follow-up; `spec/synth.md` S-50, S-76): mortar's links of a hand-made
//! `.eh_frame` (`tests/ehlocal/`, `gen.sh`), `-static --gc-sections`, on
//! AArch64 and x86-64. One CIE, the live `_start`'s FDE, the dead
//! function's FDE (removed), and a label at each of S-50's cases.

use std::path::{Path, PathBuf};

use mortar_plumb::{Format, Input, Report};

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ehlocal")
}

fn verify(a: &str, bytes: &[u8]) -> Report {
    let d = dir();
    let text = std::fs::read_to_string(d.join(format!("eh-{a}.map"))).unwrap();
    let map = mortar_plumb::map::parse(Format::Mortar, &text).unwrap();
    let obj = std::fs::read(d.join(format!("eh-{a}.o"))).unwrap();
    let path = format!("eh-{a}.o");
    mortar_plumb::verify(bytes, &[Input { path: &path, bytes: &obj }], &map)
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

/// `.symtab`: `(name, entry offset in b)`.
fn symtab(b: &[u8]) -> Vec<(String, usize)> {
    let s = sections(b);
    let t = s.iter().find(|x| x.1 == 2).unwrap();
    let strtab = s[t.5].3;
    (0..(t.4 as usize) / 24)
        .map(|i| t.3 + i * 24)
        .map(|at| (cstr(b, strtab + le32(b, at) as usize), at))
        .collect()
}

fn value(b: &[u8], name: &str) -> Option<u64> {
    symtab(b).into_iter().find(|(n, _)| n == name).map(|(_, at)| le64(b, at + 8))
}

fn set_value(b: &mut [u8], name: &str, v: u64) {
    let at = symtab(b).into_iter().find(|(n, _)| n == name).unwrap().1;
    b[at + 8..at + 16].copy_from_slice(&v.to_le_bytes());
}

fn y6(r: &Report) -> Vec<String> {
    r.findings.iter().filter(|f| f.rule.id() == "Y6").map(|f| f.message.clone()).collect()
}

// S-50 in `.symtab` (WP-210): `eh_begin` (offset 0) at the output's
// start (rule 1); `fl_mid` inside the kept FDE at its place (rule 2);
// `fde_dead` (the removed FDE's start) and `eh_end` (the section's end)
// at the final terminator, no kept record following (rule 3); `fd_mid`
// inside the removed FDE, no relocation naming it, dropped (rule 4);
// AArch64's `$d` at the kept CIE's place. mortar's outputs are clean.
// REJECTED, each a Y6 finding: `fl_mid` moved 4; `fde_dead` at the kept
// FDE; `fl_cie` renamed `fd_mid` (a dropped local present). Before
// WP-210 every one of them was "not judged".
#[allow(clippy::many_single_char_names)] // the image, the map, the values
fn ehlocal(a: &str) {
    let b = std::fs::read(dir().join(format!("eh-{a}"))).unwrap();
    let r = verify(a, &b);
    assert!(r.findings.is_empty(), "{a}: {:#?}", r.findings);
    let eh = sections(&b).into_iter().find(|s| s.0 == ".eh_frame").unwrap();
    let (start, end) = (eh.2, eh.2 + eh.4);
    assert_eq!(value(&b, "eh_begin"), Some(start), "{a}: rule 1");
    assert_eq!(value(&b, "fl_mid"), Some(start + 0x1c), "{a}: rule 2");
    assert_eq!(value(&b, "fde_dead"), Some(end - 4), "{a}: rule 3");
    assert_eq!(value(&b, "eh_end"), Some(end - 4), "{a}: rule 3 at sh_size");
    assert_eq!(value(&b, "fd_mid"), None, "{a}: rule 4");
    if a == "a64" {
        assert_eq!(value(&b, "$d"), Some(start), "{a}: the CIE's mapping symbol");
    }
    let mut m = b.clone();
    set_value(&mut m, "fl_mid", start + 0x20);
    assert!(!y6(&verify(a, &m)).is_empty(), "{a}: fl_mid moved");
    let mut m = b.clone();
    set_value(&mut m, "fde_dead", start + 0x14);
    assert!(!y6(&verify(a, &m)).is_empty(), "{a}: fde_dead at the kept FDE");
    let mut m = b.clone();
    let s = sections(&m);
    let t = s.iter().find(|x| x.1 == 2).unwrap();
    let strtab = s[t.5].3;
    let at = symtab(&m).into_iter().find(|(n, _)| n == "fl_cie").unwrap().1;
    let o = strtab + le32(&m, at) as usize;
    m[o..o + 6].copy_from_slice(b"fd_mid");
    let f = y6(&verify(a, &m));
    assert!(f.iter().any(|x| x.contains("'fd_mid'")), "{a}: fd_mid present: {f:#?}");
}

#[test]
fn s6_5_y6_eh_frame_locals_aarch64() {
    ehlocal("a64");
}

#[test]
fn s6_5_y6_eh_frame_locals_x86_64() {
    ehlocal("x64");
}

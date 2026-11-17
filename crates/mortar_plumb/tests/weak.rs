//! dyn.md D-25's branch cells under the verifier (WP-165, after WP-155's
//! ruling (A)): mortar's, lld's and GNU ld's `-nostdlib` links of an
//! undefined weak `vfn` reached by a call only (`bo-*`, `WeakDyn`), by a
//! call and an unfillable use in `-no-pie` (`split-*`, the split key) and
//! by a GOT use and an unfillable use (`w12-*`, `Weak0`; mortar's only),
//! on AArch64 and x86-64, checked in under `tests/weak/` (`gen.sh`). Each
//! output is clean but for its named anomaly (GNU ld x86-64's split: A56),
//! and each of §9.2's mutations of the ruled form is caught: the import
//! removed, the `JUMP_SLOT` removed, the branch patched to 0, the split
//! key's unfillable use given a value or a record, and `Weak0` written
//! for a branch-only key (mortar's form before WP-155).
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
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/weak")
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
        let format = if linker == "lld" { Format::Lld } else { Format::Gnu };
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

/// `.symtab`'s value of `name`.
fn sym(b: &[u8], name: &str) -> u64 {
    let s = sections(b);
    let t = section(&s, ".symtab");
    let strs = s[t.5].3;
    (t.3..t.3 + t.4)
        .step_by(24)
        .find(|&o| cstr(b, strs + le32(b, o) as usize) == name)
        .map_or_else(|| panic!("no .symtab {name}"), |o| le64(b, o + 8))
}

/// The `.dynsym` index of `name`, if any.
fn dynsym_index(b: &[u8], name: &str) -> Option<u32> {
    let s = sections(b);
    let t = section(&s, ".dynsym");
    let strs = s[t.5].3;
    (t.3..t.3 + t.4)
        .step_by(24)
        .position(|o| cstr(b, strs + le32(b, o) as usize) == name)
        .map(|i| i as u32)
}

/// The file offsets of the RELA records of `.rela.plt` against `.dynsym`
/// entry `j`.
fn plt_record(b: &[u8], j: u32) -> usize {
    let s = sections(b);
    let t = section(&s, ".rela.plt");
    (t.3..t.3 + t.4).step_by(24).find(|&o| (le64(b, o + 8) >> 32) as u32 == j).unwrap()
}

fn a64(b: &[u8]) -> bool {
    le16(b, 0x12) == 183
}

/// The place of the split key's unfillable use: `taddr`'s literal
/// (AArch64 `adrp; ldr`, the large model's `ABS64` in `.rodata`; x86-64
/// `movabs`'s immediate).
fn unfillable_place(b: &[u8]) -> u64 {
    let t = sym(b, "taddr");
    let o = file_of(b, t);
    if a64(b) {
        let adrp = le32(b, o);
        let ldr = le32(b, o + 4);
        assert_eq!(adrp & 0x9f00_0000, 0x9000_0000, "adrp at taddr");
        assert_eq!(ldr & 0xffc0_0000, 0xf940_0000, "ldr at taddr+4");
        let imm = ((adrp >> 29) & 3) | (((adrp >> 5) & 0x7ffff) << 2);
        let imm = i64::from((imm << 11) as i32 >> 11) << 12;
        let page = (t & !0xfff).wrapping_add(imm as u64);
        page + u64::from((ldr >> 10) & 0xfff) * 8
    } else {
        assert_eq!(&b[o..o + 2], &[0x48, 0xb8], "movabs at taddr");
        t + 2
    }
}

/// `ccall`'s tail branch to `vfn` (AArch64 `b`, x86-64 `jmp rel32`):
/// its address and file offset.
fn branch(b: &[u8]) -> (u64, usize) {
    let p = sym(b, "ccall");
    let o = file_of(b, p);
    if a64(b) {
        assert_eq!(le32(b, o) & 0xfc00_0000, 0x1400_0000, "b at ccall");
    } else {
        assert_eq!(b[o], 0xe9, "jmp at ccall");
    }
    (p, o)
}

/// The branch to address `to`.
fn branch_to(b: &mut [u8], to: u64) {
    let (p, o) = branch(b);
    if a64(b) {
        let d = to.wrapping_sub(p) as i64 / 4;
        assert!((-(1 << 25)..(1 << 25)).contains(&d), "b range");
        let w = 0x1400_0000 | (d as u32 & 0x03ff_ffff);
        b[o..o + 4].copy_from_slice(&w.to_le_bytes());
    } else {
        let d = to.wrapping_sub(p + 5) as u32;
        b[o + 1..o + 5].copy_from_slice(&d.to_le_bytes());
    }
}

/// The import of `vfn` removed: its `.dynstr` name changed (`vfo`).
fn rename(b: &mut [u8]) {
    let s = sections(b);
    let t = section(&s, ".dynsym");
    let strs = s[t.5].3;
    let j = dynsym_index(b, "vfn").expect("vfn in .dynsym") as usize;
    let n = strs + le32(b, t.3 + j * 24) as usize;
    b[n + 2] = b'o';
}

type Mutation = (&'static str, fn(&mut [u8]), &'static str, &'static str);

/// §9.2's mutations of WP-165, each with the rule and the message part
/// that must catch it.
const BO: &[Mutation] = &[
    ("import removed", rename, "P-D7", "(D-25: WeakDyn)"),
    ("JUMP_SLOT removed", slot_removed, "P-D9", "has no JUMP_SLOT record"),
    ("branch patched to 0", |b| branch_to(b, 0), "P-D9", "view 3"),
    ("Weak0 for the branch-only key (before WP-155)", weak0, "P-D7", "(D-25: WeakDyn)"),
];

const SPLIT: &[Mutation] = &[
    ("import removed", rename, "P-D7", "ruling (A): WeakDyn for its branches"),
    ("JUMP_SLOT removed", slot_removed, "P-D9", "has no JUMP_SLOT record"),
    ("branch patched to 0", |b| branch_to(b, 0), "P-D9", "view 3"),
    ("unfillable use given a value", nonzero, "", ""),
    (
        "record added on the unfillable use",
        record_added,
        "P-D9",
        "keeps Weak0's value and has no record",
    ),
];

fn slot_removed(b: &mut [u8]) {
    let j = dynsym_index(b, "vfn").unwrap();
    let o = plt_record(b, j);
    // r_info: the symbol kept, the type NONE.
    b[o + 8..o + 12].copy_from_slice(&0u32.to_le_bytes());
}

/// mortar's pre-WP-155 form: no entry, the branch `Weak0`'s (AArch64 to
/// the next instruction, x86-64 to 0).
fn weak0(b: &mut [u8]) {
    rename(b);
    let (p, _) = branch(b);
    let to = if a64(b) { p + 4 } else { 0 };
    branch_to(b, to);
}

fn nonzero(b: &mut [u8]) {
    let p = unfillable_place(b);
    let o = file_of(b, p);
    b[o..o + 8].copy_from_slice(&0x1234u64.to_le_bytes());
}

/// `m2fn`'s `JUMP_SLOT` rewritten as an absolute record against `vfn` on
/// the split key's unfillable use.
fn record_added(b: &mut [u8]) {
    let p = unfillable_place(b);
    let j = dynsym_index(b, "vfn").unwrap();
    let o = plt_record(b, dynsym_index(b, "m2fn").unwrap());
    let ty: u64 = if a64(b) { 257 } else { 1 };
    b[o..o + 8].copy_from_slice(&p.to_le_bytes());
    b[o + 8..o + 16].copy_from_slice(&((u64::from(j) << 32) | ty).to_le_bytes());
}

fn caught(fx: &Fx, what: &str, m: &[u8], rule: &str, part: &str, at: Option<u64>) {
    let (plain, _) = fx.judge(m);
    let hit = plain.iter().find(|f| {
        if rule.is_empty() {
            at.is_some_and(|a| f.ends_with(&format!("@Some({a:x})")))
        } else {
            f.starts_with(&format!("{rule} ")) && f.contains(part)
        }
    });
    let Some(hit) = hit else {
        panic!("{}: '{what}' not caught ({rule} '{part}'): {plain:#?}", fx.name);
    };
    eprintln!("{}: {what}: {hit}", fx.name);
}

// spec: plumb.md P-D7, P-D9 (WP-165) — mortar's outputs of D-25's branch
// cells and of W12's Weak0 are clean.
#[test]
fn d25_mortar_outputs_are_clean() {
    for a in ["a64", "x64"] {
        for n in ["bo-exec", "bo-pie", "split", "w12"] {
            let fx = Fx::mortar(&format!("{n}-{a}"));
            let (plain, named) = fx.judge(&fx.bytes());
            assert!(plain.is_empty() && named.is_empty(), "{}: {plain:#?} {named:?}", fx.name);
        }
    }
}

// spec: plumb.md P-D7, §10.2 A56 — lld's and GNU ld's outputs are held to
// mortar's form; GNU ld x86-64's split key without an entry is A56.
#[test]
fn d25_reference_outputs() {
    for a in ["a64", "x64"] {
        for n in ["bo-exec", "split"] {
            for l in ["lld", "bfd"] {
                let fx = Fx::reference(&format!("{n}-{a}"), l);
                let (plain, named) = fx.judge(&fx.bytes());
                let want: &[&str] =
                    if (n, l, a) == ("split", "bfd", "x64") { &["A56"] } else { &[] };
                assert!(plain.is_empty(), "{}: {plain:#?}", fx.name);
                assert_eq!(named, want, "{}", fx.name);
            }
        }
    }
}

// spec: plumb.md §9.2 (WP-165) — each mutation of the ruled form, on
// mortar's and on the references' outputs, both machines.
#[test]
fn d25_mutations_are_caught() {
    let mut n = 0;
    for a in ["a64", "x64"] {
        let mut fxs = vec![
            (Fx::mortar(&format!("bo-exec-{a}")), BO),
            (Fx::mortar(&format!("bo-pie-{a}")), BO),
            (Fx::mortar(&format!("split-{a}")), SPLIT),
        ];
        for l in ["lld", "bfd"] {
            fxs.push((Fx::reference(&format!("bo-exec-{a}"), l), BO));
            if (l, a) != ("bfd", "x64") {
                fxs.push((Fx::reference(&format!("split-{a}"), l), SPLIT));
            }
        }
        for (fx, ms) in &fxs {
            let b = fx.bytes();
            for &(what, f, rule, part) in *ms {
                let mut m = b.clone();
                f(&mut m);
                assert_ne!(m, b, "{}: '{what}' changed nothing", fx.name);
                let at = rule.is_empty().then(|| unfillable_place(&b));
                caught(fx, what, &m, rule, part, at);
                n += 1;
            }
        }
    }
    eprintln!("WP-165: {n} mutations, all caught");
    assert_eq!(n, 57);
}

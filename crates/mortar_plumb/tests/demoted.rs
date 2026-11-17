//! `spec/plumb.md` §5.7a and A103 (WP-178): a reference through a weak
//! mention defined only in a dropped COMDAT copy, whose key nothing live
//! defines (`spec/resolve.md` §6.3 case (b)). GNU ld's and lld's
//! `-nostdlib` outputs, checked in under `tests/demoted/` (`gen.sh`): the
//! static call (AArch64 and x86-64) and GNU ld's PIE GOT load. Each is R5,
//! and its text names the undefined-weak form (A103) exactly when the
//! field holds it: a field mutated to another value, or the same output
//! judged with an input whose mention is not demoted (its group not
//! COMDAT) or not weak, is plain R5 and no anomaly. mortar refuses such a
//! link; an image in which mortar wrote the undefined-weak value (its own
//! output of the same objects with the mention a genuine undefined weak
//! one, judged with the demoting object in its place) is R5, a FAIL.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mortar_plumb::{Ctx, Files, Format, Input, Report};

struct Dir<'a>(&'a Path);

impl Files for Dir<'_> {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(path)).ok()
    }
}

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/demoted")
}

/// One fixture: its output name and format, the inputs it is judged
/// with in place of the map's (by the map's path), and map text replaced.
struct Fx {
    name: String,
    format: Format,
    inputs: HashMap<String, Vec<u8>>,
    map_edit: Option<(String, String)>,
}

impl Fx {
    fn reference(name: &str, linker: &str) -> Fx {
        let format = if linker == "lld" { Format::Lld } else { Format::Gnu };
        Fx { name: format!("{name}.{linker}"), format, inputs: HashMap::new(), map_edit: None }
    }
    fn bytes(&self) -> Vec<u8> {
        std::fs::read(dir().join(&self.name)).unwrap()
    }
    fn verify(&self, bytes: &[u8]) -> Report {
        let d = dir();
        let mut text = std::fs::read_to_string(d.join(format!("{}.map", self.name))).unwrap();
        if let Some((a, b)) = &self.map_edit {
            text = text.replace(a.as_str(), b);
        }
        let map = mortar_plumb::map::parse(self.format, &text).unwrap();
        let inputs: Vec<(String, Vec<u8>)> = map
            .paths()
            .into_iter()
            .filter_map(|p| {
                self.inputs
                    .get(p)
                    .cloned()
                    .or_else(|| std::fs::read(d.join(p)).ok())
                    .map(|b| (p.to_string(), b))
            })
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
    /// The findings no anomaly of the format names, and the anomalies'
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

/// The file offset of address `a` in an output.
fn file_of(b: &[u8], a: u64) -> usize {
    let x = sections(b)
        .into_iter()
        .find(|x| x.1 != 8 && x.2 != 0 && x.2 <= a && a < x.2 + x.4 as u64)
        .unwrap_or_else(|| panic!("no section holds {a:#x}"));
    x.3 + (a - x.2) as usize
}

/// `.symtab`'s value of `name`.
fn sym(b: &[u8], name: &str) -> u64 {
    let s = sections(b);
    let t = s.iter().find(|x| x.0 == ".symtab").unwrap();
    let strs = s[t.5].3;
    (t.3..t.3 + t.4)
        .step_by(24)
        .find(|&o| cstr(b, strs + le32(b, o) as usize) == name)
        .map_or_else(|| panic!("no .symtab {name}"), |o| le64(b, o + 8))
}

/// The file offset of the call to `hx` in `Y` (AArch64: the word at
/// Y+4, which GNU ld writes as a NOP (W4) and lld as a `bl` to the next
/// instruction; x86-64 `call rel32`'s field at Y+5, after `sub $8,%rsp`).
fn call_field(b: &[u8]) -> usize {
    let y = sym(b, "Y");
    if le16(b, 0x12) == 183 {
        let o = file_of(b, y + 4);
        let w = le32(b, o);
        assert!(w == 0xd503_201f || w == 0x9400_0001, "nop or bl .+4 at Y+4: {w:#x}");
        o
    } else {
        let o = file_of(b, y + 4);
        assert_eq!(b[o], 0xe8, "call at Y+4");
        o + 1
    }
}

/// Input object `o` with the flag word of its first `SHT_GROUP` set to
/// `flags`, or the binding of its symbol `name` set to `bind`.
fn object_edit(o: &[u8], flags: Option<u32>, bind: Option<(&str, u8)>) -> Vec<u8> {
    let mut b = o.to_vec();
    let s = sections(o);
    if let Some(f) = flags {
        let g = s.iter().find(|x| x.1 == 17).expect("an SHT_GROUP");
        b[g.3..g.3 + 4].copy_from_slice(&f.to_le_bytes());
    }
    if let Some((name, bind)) = bind {
        let t = s.iter().find(|x| x.0 == ".symtab").unwrap();
        let strs = s[t.5].3;
        let at = (t.3..t.3 + t.4)
            .step_by(24)
            .find(|&e| cstr(o, strs + le32(o, e) as usize) == name)
            .unwrap();
        b[at + 4] = (bind << 4) | (b[at + 4] & 0xf);
    }
    b
}

const TAG: &str = "written as an undefined weak reference: resolve.md §6.3 case (b))";

/// The R5 findings' texts of a judged output.
fn r5(fx: &Fx, b: &[u8]) -> Vec<String> {
    fx.verify(b)
        .findings
        .iter()
        .filter(|f| f.rule.id() == "R5")
        .map(|f| f.message.clone())
        .collect()
}

#[test]
fn s5_7a_reference_outputs_are_a103_only() {
    for (name, linker) in [
        ("st-a64", "bfd"),
        ("st-a64", "lld"),
        ("st-x64", "bfd"),
        ("st-x64", "lld"),
        ("pie-a64", "bfd"),
    ] {
        let fx = Fx::reference(name, linker);
        let (plain, named) = fx.judge(&fx.bytes());
        assert!(plain.is_empty(), "{name}.{linker}: {plain:#?}");
        assert_eq!(named, ["A103"], "{name}.{linker}");
        assert!(r5(&fx, &fx.bytes()).iter().all(|m| m.ends_with(TAG)), "{name}.{linker}");
    }
}

#[test]
fn s5_7a_another_value_is_plain_r5() {
    for (name, linker) in [("st-a64", "bfd"), ("st-a64", "lld"), ("st-x64", "bfd"), ("st-x64", "lld")]
    {
        let fx = Fx::reference(name, linker);
        let mut b = fx.bytes();
        let o = call_field(&b);
        // Another value: AArch64 a `bl` to Y+12 (neither the NOP nor the
        // branch to the next instruction); x86-64 a call 8 bytes further.
        let w = le32(&b, o);
        let m = if le16(&b, 0x12) == 183 {
            0x9400_0002
        } else {
            w.wrapping_add(8)
        };
        b[o..o + 4].copy_from_slice(&m.to_le_bytes());
        let (plain, named) = fx.judge(&b);
        assert!(named.is_empty(), "{name}.{linker}: {named:?}");
        assert!(
            plain.iter().any(|p| p.starts_with("R5 refers to section '.text.g', which is not in the output @")),
            "{name}.{linker}: {plain:#?}"
        );
        assert!(r5(&fx, &b).iter().all(|m| !m.ends_with(TAG)), "{name}.{linker}");
    }
    // GNU ld's PIE: the GOT load's low part names the word after the slot.
    let fx = Fx::reference("pie-a64", "bfd");
    let mut b = fx.bytes();
    let y = sym(&b, "Y");
    let o = (0..16)
        .map(|k| file_of(&b, y + 4 * k))
        .find(|&o| le32(&b, o) & 0xffc0_0000 == 0xf940_0000)
        .expect("ldr x, [x, #lo12] in Y");
    let w = le32(&b, o) + (1 << 10);
    b[o..o + 4].copy_from_slice(&w.to_le_bytes());
    let (plain, named) = fx.judge(&b);
    assert!(named.is_empty(), "pie: {named:?}");
    assert!(plain.iter().any(|p| p.starts_with("R5 refers to section '.data.g'")), "pie: {plain:#?}");
}

#[test]
fn s5_7a_the_same_value_at_a_mention_not_demoted_is_plain_r5() {
    for (name, linker, a) in [
        ("st-a64", "bfd", "a64"),
        ("st-a64", "lld", "a64"),
        ("st-x64", "bfd", "x64"),
        ("st-x64", "lld", "x64"),
    ] {
        let path = format!("wb-{a}.o");
        let o = std::fs::read(dir().join(&path)).unwrap();
        // The group is no COMDAT group (flags 0): the copy is not dropped
        // by the keeper (only the map discards it). Then: hx STB_GLOBAL.
        for (what, edited) in [
            ("not COMDAT", object_edit(&o, Some(0), None)),
            ("strong", object_edit(&o, None, Some(("hx", 1)))),
        ] {
            let mut fx = Fx::reference(name, linker);
            fx.inputs.insert(path.clone(), edited);
            let b = fx.bytes();
            let (_, named) = fx.judge(&b);
            assert!(!named.contains(&"A103".to_string()), "{name}.{linker} {what}: {named:?}");
            let r = r5(&fx, &b);
            assert!(!r.is_empty() && r.iter().all(|m| !m.ends_with(TAG)), "{name}.{linker} {what}: {r:?}");
        }
    }
}

#[test]
fn s5_7a_mortar_writing_the_undefined_weak_value_fails() {
    // mortar's own link of wbu-a64.o (hx a genuine undefined weak
    // reference, the same sections and symbol indexes as wb-a64.o) is
    // clean; judged with wb-a64.o in its place it is the image mortar
    // would write if it took the demoted mention as GNU ld does: R5, and
    // no anomaly is ever a mortar finding's excuse.
    let mut fx = Fx {
        name: "mo-a64".into(),
        format: Format::Mortar,
        inputs: HashMap::new(),
        map_edit: None,
    };
    let b = fx.bytes();
    let (plain, _) = fx.judge(&b);
    assert!(plain.is_empty(), "mortar's own link: {plain:#?}");
    fx.map_edit = Some(("wbu-a64.o".into(), "wb-a64.o".into()));
    let r = fx.verify(&b);
    assert!(r.findings.iter().all(|f| mortar_plumb::anomaly::classify(Format::Mortar, &b, f).is_none()));
    let r5: Vec<_> = r.findings.iter().filter(|f| f.rule.id() == "R5").collect();
    assert_eq!(r5.len(), 1, "{:#?}", r.findings);
    assert!(r5[0].message.starts_with("refers to section '.text.g', which is not in the output"));
    assert!(r5[0].message.ends_with(TAG), "{}", r5[0].message);
}

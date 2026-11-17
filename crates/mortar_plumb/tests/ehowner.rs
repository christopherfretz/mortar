//! `spec/plumb.md` §5.7b (WP-201; `spec/resolve.md` §6.3, WP-177): a
//! field of `.gcc_except_table` or `.eh_frame` that refers to a section
//! not in the output is judged by the record that owns it. AArch64
//! `-nostdlib` static outputs of GNU ld, mold and mortar, checked in under
//! `tests/ehowner/` (`gen.sh`; lld and, but for `dead`, mortar refuse
//! every case). The field refers to the local `DW.ref.pp` of a dropped
//! copy of the COMDAT group `DW.ref.pp` (§6.3 case (a)).
//!
//! - A dead owner (`dead`: the only FDE whose LSDA holds the field is in
//!   a dropped copy): no finding, whatever the field holds, on another
//!   linker's output; mortar's must be 0 (a Field finding otherwise).
//! - A live owner (`live`, `shared`: a dead and a live FDE at one start;
//!   `xfile`: the owner is an FDE of another file, after a dead FDE's
//!   start in the same table; `x8`: that in the 64-bit DWARF form; `cie`:
//!   the personality of a CIE a live FDE uses): R5, in GNU ld's form 0
//!   (A105) and mold's "target at address 0" form (A106); another value
//!   is plain R5, no anomaly.
//! - An owner the verifier cannot name (`none`: the field is before the
//!   table's first LSDA start; `drop`: the only FDE whose LSDA is in the
//!   table is in an `.eh_frame` the link drops with its group; `abs`: an
//!   LSDA pointer against an absolute symbol, which may name any table;
//!   `upc`: the only FDE at the start has an initial location §6.4 cannot
//!   resolve): R5, no anomaly — never taken as dead.
//!
//! Each live and unknown case is the mutant of the unsafe direction: an
//! owner taken for dead would let the field go unreported.

use std::path::{Path, PathBuf};

use mortar_plumb::{Ctx, Files, Format, Input, Report, Rule};

struct Dir<'a>(&'a Path);

impl Files for Dir<'_> {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(path)).ok()
    }
}

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ehowner")
}

struct Fx {
    name: String,
    format: Format,
}

impl Fx {
    fn new(case: &str, linker: &str) -> Fx {
        let format = match linker {
            "bfd" => Format::Gnu,
            "mold" => Format::Mold,
            _ => Format::Mortar,
        };
        Fx { name: format!("{case}.{linker}"), format }
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
        let line = Some(
            std::fs::read_to_string(d.join(format!("{}.line", self.name)))
                .unwrap()
                .lines()
                .map(str::to_string)
                .collect(),
        );
        mortar_plumb::verify_with(bytes, &inputs, &map, &Ctx { line, files: &Dir(&d) })
    }
    /// The findings, each with the anomaly id that names it (or none).
    fn judge(&self, bytes: &[u8]) -> (Report, Vec<(String, Option<String>)>) {
        let r = self.verify(bytes);
        let out = r
            .findings
            .iter()
            .map(|f| {
                let a = mortar_plumb::anomaly::classify(self.format, bytes, f)
                    .map(|a| a.split(':').next().unwrap().to_string());
                (format!("{} {} @{:x?}", f.rule.id(), f.message, f.addr), a)
            })
            .collect();
        (r, out)
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

/// `(sh_addr, sh_offset, sh_size)` of the section named `name`.
fn section(b: &[u8], name: &str) -> (u64, usize, usize) {
    let shoff = le64(b, 0x28) as usize;
    let (n, ndx) = (le16(b, 0x3c), le16(b, 0x3e));
    let sh = |i: usize| shoff + i * 64;
    let strs = le64(b, sh(ndx) + 0x18) as usize;
    (0..n).map(sh).find(|&h| cstr(b, strs + le32(b, h) as usize) == name).map_or_else(
        || panic!("no section {name}"),
        |h| (le64(b, h + 0x10), le64(b, h + 0x18) as usize, le64(b, h + 0x20) as usize),
    )
}

/// The address and file offset of the field: the word between the
/// markers 0x11223344 and 0x55667788 of the output's `.gcc_except_table`.
fn field(b: &[u8]) -> (u64, usize) {
    let (addr, off, size) = section(b, ".gcc_except_table");
    let t = &b[off..off + size];
    let i = (0..size.saturating_sub(11))
        .find(|&i| le32(t, i) == 0x1122_3344 && le32(t, i + 8) == 0x5566_7788)
        .expect("the marked field");
    (addr + i as u64 + 4, off + i + 4)
}

fn with_word(b: &[u8], off: usize, v: u32) -> Vec<u8> {
    let mut m = b.to_vec();
    m[off..off + 4].copy_from_slice(&v.to_le_bytes());
    m
}

const OTHER: u32 = 0x0012_3450;

/// mold's anomalies on every output of these objects (A3: a TLS symbol
/// without `PT_TLS`; A9: a global's binding), not this test's.
fn own(found: Vec<(String, Option<String>)>) -> Vec<(String, Option<String>)> {
    found.into_iter().filter(|(_, a)| !matches!(a.as_deref(), Some("A3" | "A9"))).collect()
}

/// The one R5 at `at`, with its anomaly id; panics on any other finding.
fn only_r5(fx: &Fx, b: &[u8], at: Option<u64>) -> (String, Option<String>) {
    let found = own(fx.judge(b).1);
    let r5: Vec<_> = found.iter().filter(|(m, _)| m.starts_with("R5 ")).cloned().collect();
    let other: Vec<_> = found.iter().filter(|(m, _)| !m.starts_with("R5 ")).collect();
    assert!(other.is_empty(), "{}: other findings {other:#?}", fx.name);
    assert_eq!(r5.len(), 1, "{}: {found:#?}", fx.name);
    if let Some(a) = at {
        assert!(r5[0].0.ends_with(&format!("@Some({a:x})")), "{}: {r5:#?} not at {a:#x}", fx.name);
    }
    r5[0].clone()
}

fn want_id(linker: &str) -> &'static str {
    if linker == "bfd" { "A105" } else { "A106" }
}

#[test]
fn dead_owner_is_free_on_other_linkers() {
    for l in ["bfd", "mold"] {
        let fx = Fx::new("dead", l);
        let b = fx.bytes();
        let (_, off) = field(&b);
        for m in [b.clone(), with_word(&b, off, OTHER)] {
            let (r, found) = fx.judge(&m);
            let found = own(found);
            assert!(found.is_empty(), "{}: {found:#?}", fx.name);
            assert!(r.coverage.absent_records >= 1, "{}: the field is counted absent", fx.name);
        }
    }
}

#[test]
fn dead_owner_on_mortar_must_be_zero() {
    let fx = Fx::new("dead", "mortar");
    let b = fx.bytes();
    let (at, off) = field(&b);
    assert_eq!(le32(&b, off), 0, "mortar writes 0");
    let (r, found) = fx.judge(&b);
    assert!(found.is_empty(), "{found:#?}");
    assert!(r.coverage.verified.iter().any(|(v, &n)| format!("{v:?}") == "Tombstone" && n >= 1));
    for v in [1, OTHER] {
        let r = fx.verify(&with_word(&b, off, v));
        let f: Vec<_> = r.findings.iter().filter(|f| f.rule == Rule::Field).collect();
        assert_eq!(f.len(), 1, "{:#?}", r.findings);
        assert_eq!(f[0].addr, Some(at));
        assert_eq!((f[0].expected, f[0].found), (Some(0), Some(u64::from(v))));
        assert!(mortar_plumb::anomaly::classify(Format::Mortar, &b, f[0]).is_none());
    }
}

/// A live owner: R5 in the linker's form, plain R5 for another value.
fn live(case: &str, linkers: &[&str]) {
    for &l in linkers {
        let fx = Fx::new(case, l);
        let b = fx.bytes();
        let (at, off) = field(&b);
        let (m, id) = only_r5(&fx, &b, Some(at));
        assert_eq!(id.as_deref(), Some(want_id(l)), "{}: {m}", fx.name);
        let (m, id) = only_r5(&fx, &with_word(&b, off, OTHER), Some(at));
        assert_eq!(id, None, "{}: {m}", fx.name);
        assert!(m.contains("which is not in the output @"), "{}: {m}", fx.name);
    }
}

#[test]
fn live_owner_same_file() {
    live("live", &["bfd", "mold"]);
}

#[test]
fn live_owner_shared_start() {
    live("shared", &["bfd", "mold"]);
}

#[test]
fn live_owner_in_another_file_after_a_dead_start() {
    live("xfile", &["bfd", "mold"]);
}

#[test]
fn live_owner_64bit_records() {
    live("x8", &["bfd"]);
}

#[test]
fn live_cie_personality() {
    for l in ["bfd", "mold"] {
        let fx = Fx::new("cie", l);
        let b = fx.bytes();
        let (m, id) = only_r5(&fx, &b, None);
        assert_eq!(id.as_deref(), Some(want_id(l)), "{}: {m}", fx.name);
        let r = fx.verify(&b);
        let at = r.findings[0].addr.unwrap();
        let (eh, off, _) = section(&b, ".eh_frame");
        let off = off + (at - eh) as usize;
        let (m, id) = only_r5(&fx, &with_word(&b, off, OTHER), Some(at));
        assert_eq!(id, None, "{}: {m}", fx.name);
    }
}

/// No owner the verifier can name: R5 whatever the field holds.
fn unknown(case: &str, linkers: &[&str]) {
    for &l in linkers {
        let fx = Fx::new(case, l);
        let b = fx.bytes();
        let (at, off) = field(&b);
        for m in [b.clone(), with_word(&b, off, 0), with_word(&b, off, OTHER)] {
            let (msg, id) = only_r5(&fx, &m, Some(at));
            assert_eq!(id, None, "{}: {msg}", fx.name);
            assert!(msg.contains("the field's owner is not known"), "{}: {msg}", fx.name);
        }
    }
}

#[test]
fn no_start_before_the_field() {
    unknown("none", &["bfd", "mold"]);
}

#[test]
fn owner_in_a_dropped_eh_frame() {
    unknown("drop", &["bfd", "mold"]);
}

/// An LSDA pointer whose target the verifier cannot place (an absolute
/// symbol) may name any table: no owner is named, the dead word of
/// `dead` included, on every output (mortar's 0 too).
#[test]
fn an_lsda_pointer_not_placed_names_no_owner() {
    unknown("abs", &["bfd", "mold", "mortar"]);
}

/// The only FDE at the start has an initial location §6.4 cannot resolve
/// (no relocation, PC-relative): its life is unknown, and so is the
/// owner (GNU ld links it; mold and mortar refuse the FDE).
#[test]
fn an_fde_of_unknown_life_names_no_owner() {
    unknown("upc", &["bfd"]);
}

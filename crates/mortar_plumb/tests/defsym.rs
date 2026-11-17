//! `--defsym=SYM=EXPR` under the verifier (`spec/plumb.md` §4.4b,
//! `spec/resolve.md` §7.2; WP-170 part 2): mortar's, lld's, GNU ld's and
//! mold's `-nostdlib` links checked in under `tests/defsym/` (`gen.sh`),
//! static on AArch64, x86-64 and i386, `-pie` and `-shared` on AArch64
//! and x86-64, each clean, and the mutations of §9.2's form on every
//! output they apply to.
#![allow(clippy::many_single_char_names)]

use std::path::{Path, PathBuf};

use mortar_plumb::{Ctx, Files, Format, Input, Map, MapFile, Report};

struct Dir<'a>(&'a Path);

impl Files for Dir<'_> {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(path)).ok()
    }
}

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/defsym")
}

const ARCHES: [&str; 3] = ["a64", "x64", "x86"];
const CELLS: [&str; 6] = ["num", "st", "ov", "mem", "pie", "sh"];
const LINKERS: [&str; 4] = ["mortar", "lld", "bfd", "mold"];

/// One fixture: its output's file name and the map's format.
struct Fx {
    name: String,
    format: Format,
}

impl Fx {
    fn new(cell: &str, arch: &str, linker: &str) -> Fx {
        let n = format!("{cell}-{arch}");
        match linker {
            "mortar" => Fx { name: n, format: Format::Mortar },
            "lld" => Fx { name: format!("{n}.lld"), format: Format::Lld },
            "mold" => Fx { name: format!("{n}.mold"), format: Format::Mold },
            _ => Fx { name: format!("{n}.bfd"), format: Format::Gnu },
        }
    }
    fn exists(&self) -> bool {
        dir().join(&self.name).exists()
    }
    fn bytes(&self) -> Vec<u8> {
        std::fs::read(dir().join(&self.name)).unwrap()
    }
    fn map(&self) -> Map {
        let text = std::fs::read_to_string(dir().join(format!("{}.map", self.name))).unwrap();
        mortar_plumb::map::parse(self.format, &text).unwrap()
    }
    fn line(&self) -> Option<Vec<String>> {
        (self.format != Format::Mortar).then(|| {
            std::fs::read_to_string(dir().join(format!("{}.line", self.name)))
                .unwrap()
                .lines()
                .map(str::to_string)
                .collect()
        })
    }
    fn verify(bytes: &[u8], map: &Map, line: Option<Vec<String>>) -> Report {
        let d = dir();
        let inputs: Vec<(String, Vec<u8>)> = map
            .paths()
            .into_iter()
            .filter_map(|p| std::fs::read(d.join(p)).ok().map(|b| (p.to_string(), b)))
            .collect();
        let inputs: Vec<Input<'_>> =
            inputs.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
        mortar_plumb::verify_with(bytes, &inputs, map, &Ctx { line, files: &Dir(&d) })
    }
    /// The findings that are no anomaly of the format (rule, message,
    /// symbol, site file), and the anomalies' ids.
    fn judge_with(&self, bytes: &[u8], map: &Map, line: Option<Vec<String>>) -> Judged {
        let r = Fx::verify(bytes, map, line);
        let mut plain = Vec::new();
        let mut named = Vec::new();
        for f in &r.findings {
            match mortar_plumb::anomaly::classify(self.format, bytes, f) {
                Some(a) => named.push(a.split(':').next().unwrap().to_string()),
                None => plain.push(format!(
                    "{} {} sym={} site={} @{:x?}",
                    f.rule.id(),
                    f.message,
                    f.symbol.as_deref().unwrap_or("-"),
                    f.site.as_ref().map_or("-", |s| s.file.as_str()),
                    f.addr
                )),
            }
        }
        named.sort();
        named.dedup();
        Judged { plain, named, coverage: r.coverage }
    }
    fn judge(&self, bytes: &[u8]) -> Judged {
        self.judge_with(bytes, &self.map(), self.line())
    }
}

struct Judged {
    plain: Vec<String>,
    named: Vec<String>,
    coverage: mortar_plumb::Coverage,
}

/// The fixtures of a cell that exist (`pie` and `sh` have no i386 link;
/// mold links only `num`).
fn all(cell: &str) -> Vec<(&'static str, &'static str, Fx)> {
    let mut v = Vec::new();
    for a in ARCHES {
        for l in LINKERS {
            let fx = Fx::new(cell, a, l);
            if fx.exists() {
                v.push((a, l, fx));
            }
        }
    }
    v
}

// ---- ELF reads and edits (ELFCLASS32 and ELFCLASS64, little-endian) ----

fn le16(b: &[u8], o: usize) -> u64 {
    u64::from(u16::from_le_bytes([b[o], b[o + 1]]))
}
fn le32(b: &[u8], o: usize) -> u64 {
    u64::from(u32::from_le_bytes(b[o..o + 4].try_into().unwrap()))
}
fn le64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn cstr(b: &[u8], o: usize) -> String {
    let e = b[o..].iter().position(|&c| c == 0).unwrap();
    String::from_utf8_lossy(&b[o..o + e]).into_owned()
}
fn wide(b: &[u8]) -> bool {
    b[4] == 2
}
/// A word of the class: 4 or 8 bytes.
fn word(b: &[u8], o: usize) -> u64 {
    if wide(b) { le64(b, o) } else { le32(b, o) }
}
fn put_word(b: &mut [u8], o: usize, v: u64) {
    if wide(b) {
        b[o..o + 8].copy_from_slice(&v.to_le_bytes());
    } else {
        put32(b, o, v as u32);
    }
}
fn machine(b: &[u8]) -> u64 {
    le16(b, 18)
}

struct Sec {
    name: String,
    ty: u64,
    addr: u64,
    off: usize,
    size: usize,
    link: usize,
}

fn sections(b: &[u8]) -> Vec<Sec> {
    let (shoff, shentsize, n, ndx) = if wide(b) {
        (le64(b, 0x28) as usize, 64, le16(b, 0x3c) as usize, le16(b, 0x3e) as usize)
    } else {
        (le32(b, 0x20) as usize, 40, le16(b, 0x30) as usize, le16(b, 0x32) as usize)
    };
    let h = |i: usize| shoff + i * shentsize;
    let field = |i: usize, o64: usize, o32: usize| {
        if wide(b) { le64(b, h(i) + o64) } else { le32(b, h(i) + o32) }
    };
    let strs = field(ndx, 0x18, 0x10) as usize;
    (0..n)
        .map(|i| Sec {
            name: cstr(b, strs + le32(b, h(i)) as usize),
            ty: le32(b, h(i) + 4),
            addr: field(i, 0x10, 0x0c),
            off: field(i, 0x18, 0x10) as usize,
            size: field(i, 0x20, 0x14) as usize,
            link: le32(b, h(i) + if wide(b) { 0x28 } else { 0x18 }) as usize,
        })
        .collect()
}

/// The file offset of address `a` (in a section with file bytes).
fn offset_of(b: &[u8], a: u64) -> Option<usize> {
    sections(b)
        .into_iter()
        .find(|s| s.ty != 8 && s.addr != 0 && s.addr <= a && a < s.addr + s.size as u64)
        .map(|s| s.off + (a - s.addr) as usize)
}

struct Sym {
    name: String,
    /// The file offset of the entry.
    at: usize,
    value: u64,
    size: u64,
}

/// The symbols of the first section of type `ty` (2 `.symtab`, 11
/// `.dynsym`).
fn syms(b: &[u8], ty: u64) -> Vec<Sym> {
    let s = sections(b);
    let Some(t) = s.iter().find(|x| x.ty == ty) else { return Vec::new() };
    let strtab = s[t.link].off;
    let ent = if wide(b) { 24 } else { 16 };
    (0..t.size / ent)
        .map(|i| {
            let at = t.off + i * ent;
            let st_name = le32(b, at) as usize;
            let (value, size) = if wide(b) {
                (le64(b, at + 8), le64(b, at + 16))
            } else {
                (le32(b, at + 4), le32(b, at + 8))
            };
            Sym { name: cstr(b, strtab + st_name), at, value, size }
        })
        .collect()
}

fn sym<'s>(v: &'s [Sym], name: &str) -> Option<&'s Sym> {
    v.iter().find(|s| s.name == name && s.value != 0)
}

/// The direct calls (and AArch64 `B`) in function `func`: (file offset,
/// place, target).
fn calls(b: &[u8], func: &str) -> Vec<(usize, u64, u64)> {
    let t = syms(b, 2);
    let Some(f) = sym(&t, func) else { return Vec::new() };
    let Some(start) = offset_of(b, f.value) else { return Vec::new() };
    let mut v = Vec::new();
    if machine(b) == 183 {
        for i in (0..f.size as usize).step_by(4) {
            let w = le32(b, start + i) as u32;
            if matches!(w & 0xfc00_0000, 0x9400_0000 | 0x1400_0000) {
                let imm = (i64::from(((w & 0x03ff_ffff) << 6) as i32) >> 6) << 2;
                let p = f.value + i as u64;
                v.push((start + i, p, p.wrapping_add(imm as u64)));
            }
        }
    } else {
        for i in 0..(f.size as usize).saturating_sub(4) {
            if b[start + i] == 0xe8 {
                let rel = le32(b, start + i + 1) as u32 as i32;
                let p = f.value + i as u64;
                v.push((start + i, p, p.wrapping_add(5).wrapping_add(i64::from(rel) as u64)));
            }
        }
    }
    v
}

/// The call at `off` (place `p`) made to reach `to`.
fn retarget(b: &mut [u8], off: usize, p: u64, to: u64) {
    if machine(b) == 183 {
        let w = le32(b, off) as u32;
        let imm = ((to.wrapping_sub(p) as i64 >> 2) as u32) & 0x03ff_ffff;
        put32(b, off, (w & 0xfc00_0000) | imm);
    } else {
        put32(b, off + 1, to.wrapping_sub(p).wrapping_sub(5) as u32);
    }
}

/// The `.symtab` entry of `name` given the value `v`.
fn symbol_set(b: &[u8], name: &str, v: u64) -> Option<Vec<u8>> {
    let t = syms(b, 2);
    let s = t.iter().find(|s| s.name == name)?;
    let mut m = b.to_vec();
    let o = if wide(b) { s.at + 8 } else { s.at + 4 };
    put_word(&mut m, o, v);
    Some(m)
}

/// The `R_*_RELATIVE` records of `.rela.dyn`/`.rel.dyn`: (file offset of
/// the record, `r_offset`, addend).
fn relatives(b: &[u8]) -> Vec<(usize, u64, u64)> {
    let relative = if machine(b) == 183 { 1027 } else { 8 };
    let Some(rela) = sections(b).into_iter().find(|s| s.name == ".rela.dyn") else {
        return Vec::new();
    };
    (0..rela.size / 24)
        .map(|i| rela.off + i * 24)
        .filter(|&at| le64(b, at + 8) & 0xffff_ffff == relative)
        .map(|at| (at, le64(b, at), le64(b, at + 16)))
        .collect()
}

/// The word at `name`'s address made `f` of what it was: in a PIE the
/// addend of its `RELATIVE` record (and the word, when it holds the
/// addend), else the word.
fn word_set(b: &[u8], name: &str, f: impl Fn(u64) -> u64) -> Option<Vec<u8>> {
    let a = sym(&syms(b, 2), name)?.value;
    let o = offset_of(b, a)?;
    let mut m = b.to_vec();
    if let Some((at, _, add)) = relatives(b).into_iter().find(|r| r.1 == a) {
        m[at + 16..at + 24].copy_from_slice(&f(add).to_le_bytes());
        if word(b, o) == add {
            put_word(&mut m, o, f(add));
        }
    } else {
        put_word(&mut m, o, f(word(b, o)));
    }
    Some(m)
}

// ---- the mutations ----

type Mutation = fn(&Fx) -> Option<Mutant>;

/// A mutated output, with the map and line it is judged by.
struct Mutant {
    bytes: Vec<u8>,
    map: Map,
    line: Option<Vec<String>>,
}

fn bytes_only(fx: &Fx, bytes: Option<Vec<u8>>) -> Option<Mutant> {
    Some(Mutant { bytes: bytes?, map: fx.map(), line: fx.line() })
}

/// The `.symtab` value of `k` (`--defsym=k=NUMBER`) one more.
fn number_symbol_moved(fx: &Fx) -> Option<Mutant> {
    let b = fx.bytes();
    let k = sym(&syms(&b, 2), "k")?.value;
    bytes_only(fx, symbol_set(&b, "k", k + 1))
}

/// The word `pk = (long)k` one more than `k`'s number.
fn number_word_moved(fx: &Fx) -> Option<Mutant> {
    bytes_only(fx, word_set(&fx.bytes(), "pk", |v| v + 1))
}

/// `_start`'s call of `a` (`--defsym=a=b`) made a call of `g`.
fn alias_call_elsewhere(fx: &Fx) -> Option<Mutant> {
    let b = fx.bytes();
    let t = syms(&b, 2);
    let (a, g) = (sym(&t, "a")?.value, sym(&t, "g")?.value);
    let (off, p, _) = calls(&b, "_start").into_iter().find(|c| c.2 == a)?;
    let mut m = b.clone();
    retarget(&mut m, off, p, g);
    bytes_only(fx, Some(m))
}

/// The word `pa = (long)a` given `g`'s address.
fn alias_word_elsewhere(fx: &Fx) -> Option<Mutant> {
    let b = fx.bytes();
    let g = sym(&syms(&b, 2), "g")?.value;
    bytes_only(fx, word_set(&b, "pa", |_| g))
}

/// The word `pc = (long)c` (`--defsym=c=b+4`) one more: `b+5`.
fn offset_word_off_by_one(fx: &Fx) -> Option<Mutant> {
    bytes_only(fx, word_set(&fx.bytes(), "pc", |v| v + 1))
}

/// The `.symtab` value of `c` (`b+4`) one less: `b+3`.
fn offset_symbol_off_by_one(fx: &Fx) -> Option<Mutant> {
    let b = fx.bytes();
    let c = sym(&syms(&b, 2), "c")?.value;
    bytes_only(fx, symbol_set(&b, "c", c - 1))
}

/// In the PIE, `pa`'s `RELATIVE` record moved to `pk`'s word, with `k`'s
/// number as addend: the number relocated (`pa` left unrelocated).
fn number_relocated(fx: &Fx) -> Option<Mutant> {
    let b = fx.bytes();
    let t = syms(&b, 2);
    let (pa, pk, k) = (sym(&t, "pa")?.value, sym(&t, "pk")?.value, sym(&t, "k")?.value);
    let (at, _, _) = relatives(&b).into_iter().find(|r| r.1 == pa)?;
    let mut m = b.clone();
    m[at..at + 8].copy_from_slice(&pk.to_le_bytes());
    m[at + 16..at + 24].copy_from_slice(&k.to_le_bytes());
    bytes_only(fx, Some(m))
}

/// The address of `ov.o`'s own `a` (the definition `--defsym=a=b`
/// replaces): `self`'s address less `self`'s offset in `ov.o`.
fn replaced_a(fx: &Fx, b: &[u8]) -> Option<u64> {
    let arch = fx.name.split('.').next()?.rsplit('-').next()?;
    let o = std::fs::read(dir().join(format!("ov-{arch}.o"))).ok()?;
    let ot = syms(&o, 2);
    let (a, s) = (ot.iter().find(|s| s.name == "a")?, ot.iter().find(|s| s.name == "self")?);
    Some(sym(&syms(b, 2), "self")?.value - (s.value - a.value))
}

/// The `.symtab` value of `a` made the replaced definition's.
fn replaced_symbol_written(fx: &Fx) -> Option<Mutant> {
    let b = fx.bytes();
    let a = replaced_a(fx, &b)?;
    bytes_only(fx, symbol_set(&b, "a", a))
}

/// `self`'s call of `a` (in the object whose `a` is replaced) made a call
/// of the replaced definition.
fn replaced_call(fx: &Fx) -> Option<Mutant> {
    let b = fx.bytes();
    let a = replaced_a(fx, &b)?;
    let (off, p, _) = calls(&b, "self").into_iter().next()?;
    let mut m = b.clone();
    retarget(&mut m, off, p, a);
    bytes_only(fx, Some(m))
}

/// `libam.a(am.o)`, the member defining `a`, added to mortar's map
/// (M12; a reference's map is judged by the union of edges, rule 2,
/// which accepts it: lld's extracts it).
fn member_extracted(fx: &Fx) -> Option<Mutant> {
    if fx.format != Format::Mortar {
        return None;
    }
    let mut map = fx.map();
    if map.files.iter().any(|f| f.member.as_deref().is_some_and(|m| m.starts_with("am-"))) {
        return None;
    }
    let arch = fx.name.rsplit('-').next()?;
    map.files.push(MapFile::member(format!("libam-{arch}.a"), format!("am-{arch}.o")));
    Some(Mutant { bytes: fx.bytes(), map, line: fx.line() })
}

/// In `-shared`, `via`'s call of `calla` (preemptible by its own name)
/// made a call of `b`'s address: bound locally, without the PLT.
fn preemptible_call_local(fx: &Fx) -> Option<Mutant> {
    let b = fx.bytes();
    let calla = sym(&syms(&b, 2), "calla")?.value;
    let (off, p, _) = calls(&b, "via").into_iter().next()?;
    let mut m = b.clone();
    retarget(&mut m, off, p, calla);
    bytes_only(fx, Some(m))
}

/// A mutation, the cells it applies to, and the finding that must catch
/// it: one of the rules, its text holding `key`.
struct Mut {
    what: &'static str,
    m: Mutation,
    cells: &'static [&'static str],
    rules: &'static [&'static str],
    key: &'static str,
}

const MUTATIONS: &[Mut] = &[
    Mut {
        what: "k's .symtab value not the --defsym number",
        m: number_symbol_moved,
        cells: &["num", "st", "pie", "sh"],
        rules: &["Y1"],
        key: "'k'",
    },
    Mut {
        what: "the word (long)k not the --defsym number",
        m: number_word_moved,
        cells: &["num", "st", "pie"],
        rules: &["R1"],
        key: "sym=k",
    },
    Mut {
        what: "a call of the alias a bound to g",
        m: alias_call_elsewhere,
        cells: &["st", "mem", "pie"],
        rules: &["R1"],
        key: "sym=a",
    },
    Mut {
        what: "the word (long)a given g's address",
        m: alias_word_elsewhere,
        cells: &["st", "mem", "pie"],
        rules: &["R1"],
        key: "sym=a site=m",
    },
    Mut {
        what: "the word (long)c one off b+4",
        m: offset_word_off_by_one,
        cells: &["st", "mem", "pie"],
        rules: &["R1"],
        key: "sym=c site=m",
    },
    Mut {
        what: "c's .symtab value one off b+4",
        m: offset_symbol_off_by_one,
        cells: &["st", "mem", "pie"],
        rules: &["Y1"],
        key: "'c'",
    },
    Mut {
        what: "the --defsym number k given a RELATIVE record in the PIE",
        m: number_relocated,
        cells: &["pie"],
        rules: &["P-D9"],
        key: "a RELATIVE record on a word",
    },
    Mut {
        what: "a's .symtab value the replaced definition's",
        m: replaced_symbol_written,
        cells: &["ov"],
        rules: &["Y1"],
        key: "'a'",
    },
    Mut {
        what: "a call of a bound to the replaced definition",
        m: replaced_call,
        cells: &["ov"],
        rules: &["R1"],
        key: "sym=a site=ov-",
    },
    Mut {
        what: "a's defining member extracted (mortar's map)",
        m: member_extracted,
        cells: &["mem"],
        rules: &["M12"],
        key: "(am-",
    },
    Mut {
        what: "a call of the preemptible alias calla bound locally in -shared",
        m: preemptible_call_local,
        cells: &["sh"],
        rules: &["P-D17", "R1"],
        key: "sym=calla",
    },
];

/// The anomalies each linker's clean fixtures may draw: GNU ld's forms of
/// §7.2 (A85 B+N, A86 the replaced size, A87 a number relocated, A88 an
/// earlier type) and its static-pie GOT (A30), mold's `.symtab` (A9).
fn allowed(linker: &str) -> &'static [&'static str] {
    match linker {
        "bfd" => &["A85", "A86", "A87", "A88", "A30"],
        "mold" => &["A9", "A3", "A13"],
        _ => &[],
    }
}

#[test]
#[ignore = "prints every fixture's findings"]
fn survey() {
    for c in CELLS {
        for (_, _, fx) in all(c) {
            let j = fx.judge(&fx.bytes());
            eprintln!("== {} named {:?}", fx.name, j.named);
            for p in &j.plain {
                eprintln!("   {p}");
            }
        }
    }
    for x in MUTATIONS {
        for c in x.cells {
            for (_, _, fx) in all(c) {
                let Some(m) = (x.m)(&fx) else { continue };
                let j = fx.judge_with(&m.bytes, &m.map, m.line);
                eprintln!("== {}: {} named {:?}", x.what, fx.name, j.named);
                for p in &j.plain {
                    eprintln!("   {p}");
                }
            }
        }
    }
}

/// Every fixture is clean: mortar's draws nothing; a reference's
/// findings are its linker's named anomalies.
#[test]
fn clean() {
    let mut bad = Vec::new();
    let mut n = 0;
    for c in CELLS {
        for (_, l, fx) in all(c) {
            n += 1;
            let j = fx.judge(&fx.bytes());
            for p in j.plain {
                bad.push(format!("{}: {p}", fx.name));
            }
            for a in j.named.iter().filter(|a| !allowed(l).contains(&a.as_str())) {
                bad.push(format!("{}: unexpected {a}", fx.name));
            }
            if l == "mortar" && !j.named.is_empty() {
                bad.push(format!("{}: mortar's output named {:?}", fx.name, j.named));
            }
        }
    }
    // mortar, lld and GNU ld on 4 static cells x 3 machines and 2 cells x
    // 2 machines; mold on `num` x 3.
    assert_eq!(n, 3 * (4 * 3 + 2 * 2) + 3, "every fixture is there");
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// GNU ld's named forms occur where §7.2 says (A85 on `st`, A86 on `ov`,
/// A87 on the x86-64 `pie`), so the anomalies are not dead text.
#[test]
fn bfd_forms_are_named() {
    for (cell, arch, id) in [("st", "x64", "A85"), ("ov", "x64", "A86"), ("pie", "x64", "A87")] {
        let fx = Fx::new(cell, arch, "bfd");
        let j = fx.judge(&fx.bytes());
        assert!(j.named.iter().any(|a| a == id), "{}: {id} not drawn: {:?}", fx.name, j.named);
    }
}

/// The coverage of a line without `--defsym` names nothing of it, and a
/// line with derived forms only names nothing either.
#[test]
fn no_defsym_no_coverage() {
    for cell in ["num", "st"] {
        let fx = Fx::new(cell, "x64", "bfd");
        let j = fx.judge(&fx.bytes());
        assert!(j.coverage.defsym_not_judged.is_empty(), "{}", fx.name);
    }
}

/// §9.2: each mutation is caught by a finding no anomaly names, on
/// mortar's output and on every reference's it applies to.
#[test]
fn mutations() {
    let mut missed = Vec::new();
    for x in MUTATIONS {
        let mut mortar = 0;
        let mut want = 0;
        for c in x.cells {
            want += if matches!(*c, "pie" | "sh") { 2 } else { 3 };
            for (_, l, fx) in all(c) {
                let Some(m) = (x.m)(&fx) else { continue };
                if l == "mortar" {
                    mortar += 1;
                }
                let j = fx.judge_with(&m.bytes, &m.map, m.line);
                let hit = j.plain.iter().find(|p| {
                    x.rules.iter().any(|r| p.starts_with(&format!("{r} "))) && p.contains(x.key)
                });
                match hit {
                    None => missed.push(format!(
                        "{}: {} not caught: {}",
                        x.what,
                        fx.name,
                        j.plain.join(" | ")
                    )),
                    Some(h) if std::env::var_os("DEFSYM_SHOW").is_some() => {
                        eprintln!("{}: {}: {h}", x.what, fx.name);
                    }
                    Some(_) => {}
                }
            }
        }
        assert_eq!(mortar, want, "{}: every mortar output of its cells mutated", x.what);
    }
    assert!(missed.is_empty(), "{}", missed.join("\n"));
}

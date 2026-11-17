//! `--wrap=SYM` under the verifier (`spec/plumb.md` §4.4a,
//! `spec/resolve.md` §7.1; WP-170): mortar's, lld's, GNU ld's and mold's
//! `-nostdlib` links checked in under `tests/wrap/` (`gen.sh`), static on
//! AArch64, x86-64 and i386, `-pie` on AArch64 and x86-64, each clean,
//! and the mutations of §9.2's form on every output they apply to.
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
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/wrap")
}

const ARCHES: [&str; 3] = ["a64", "x64", "x86"];
const CELLS: [&str; 5] = ["st", "mem", "real", "gc", "dm"];
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

/// The fixtures of a cell that exist (`dm` has no i386 link).
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
    name_off: usize,
    value: u64,
    size: u64,
    index: usize,
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
            Sym {
                name: cstr(b, strtab + st_name),
                name_off: strtab + st_name,
                value,
                size,
                index: i,
            }
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

/// In `func`, the call that reaches `from` made to reach `to`.
fn call_moved(b: &[u8], func: &str, from: &str, to: u64) -> Option<Vec<u8>> {
    let t = syms(b, 2);
    let from = sym(&t, from)?.value;
    let (off, p, _) = calls(b, func).into_iter().find(|c| c.2 == from)?;
    let mut m = b.to_vec();
    retarget(&mut m, off, p, to);
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

/// `_start`'s call of `f` (bound to `__wrap_f`) made a call of `f`.
fn wrapped_call_to_f(fx: &Fx) -> Option<Mutant> {
    let b = fx.bytes();
    let f = sym(&syms(&b, 2), "f")?.value;
    bytes_only(fx, call_moved(&b, "_start", "__wrap_f", f))
}

/// `_start`'s call of `__real_f` (bound to `f`) made a call of
/// `__wrap_f`.
fn real_call_to_wrap(fx: &Fx) -> Option<Mutant> {
    let b = fx.bytes();
    let w = sym(&syms(&b, 2), "__wrap_f")?.value;
    bytes_only(fx, call_moved(&b, "_start", "f", w))
}

/// `g`'s call of `f` in `f`'s own object made a call of `__wrap_f` (lld's
/// E13 form).
fn own_call_wrapped(fx: &Fx) -> Option<Mutant> {
    let b = fx.bytes();
    let w = sym(&syms(&b, 2), "__wrap_f")?.value;
    bytes_only(fx, call_moved(&b, "g", "f", w))
}

/// The `.symtab` name `__wrap_f` overwritten with `__real_f` (a
/// `__real_f` symbol written).
fn real_symbol_written(fx: &Fx) -> Option<Mutant> {
    let b = fx.bytes();
    let t = syms(&b, 2);
    let s = t.iter().find(|s| s.name == "__wrap_f")?;
    let mut m = b.clone();
    m[s.name_off..s.name_off + 8].copy_from_slice(b"__real_f");
    bytes_only(fx, Some(m))
}

/// `libfo.a(fo.o)` added to the map, though only `f`, wrapped, names it
/// (M12).
fn member_extracted(fx: &Fx) -> Option<Mutant> {
    let mut map = fx.map();
    if map.files.iter().any(|f| f.member.as_deref().is_some_and(|m| m.starts_with("fo-"))) {
        return None;
    }
    let arch = fx.name.split('.').next()?.rsplit('-').next()?;
    let (lib, member) = (format!("libfo-{arch}.a"), format!("fo-{arch}.o"));
    map.files.push(MapFile::member(&lib, &member));
    Some(Mutant { bytes: fx.bytes(), map, line: fx.line() })
}

/// `libfo.a(fo.o)` taken out of the map, though `__real_f` reaches it
/// (M11).
fn member_missing(fx: &Fx) -> Option<Mutant> {
    let mut map = fx.map();
    let i =
        map.files.iter().position(|f| f.member.as_deref().is_some_and(|m| m.starts_with("fo-")))?;
    map.files.remove(i);
    let i = i as u32;
    let moved = |f: &mut u32| {
        if *f > i {
            *f -= 1;
        }
    };
    map.placed.retain(|p| p.file != i);
    map.placed.iter_mut().for_each(|p| moved(&mut p.file));
    map.discarded.retain(|p| p.file != i);
    map.discarded.iter_mut().for_each(|p| moved(&mut p.file));
    map.nonalloc.retain(|p| p.file != i);
    map.nonalloc.iter_mut().for_each(|p| moved(&mut p.file));
    map.included.retain(|x| !x.file.member.as_deref().is_some_and(|m| m.starts_with("fo-")));
    Some(Mutant { bytes: fx.bytes(), map, line: fx.line() })
}

/// `_start`'s call of `malloc` (bound to `__wrap_malloc`) made a call of
/// the PLT entry `__wrap_malloc`'s `__real_malloc` uses (`JUMP_SLOT
/// malloc`).
fn malloc_call_to_plt(fx: &Fx) -> Option<Mutant> {
    let b = fx.bytes();
    let (_, _, plt) = calls(&b, "__wrap_malloc").into_iter().next()?;
    bytes_only(fx, call_moved(&b, "_start", "__wrap_malloc", plt))
}

/// Every `R_*_RELATIVE` record whose addend is `__wrap_malloc` (the word
/// `p` and the GOT slot of `q = malloc`) made `R_*_GLOB_DAT malloc`.
fn malloc_slot_named(fx: &Fx) -> Option<Mutant> {
    let b = fx.bytes();
    let w = sym(&syms(&b, 2), "__wrap_malloc")?.value;
    let d = syms(&b, 11);
    let malloc = d.iter().find(|s| s.name == "malloc")?.index as u64;
    let (relative, glob_dat) = if machine(&b) == 183 { (1027, 1025) } else { (8, 6) };
    let rela = sections(&b).into_iter().find(|s| s.name == ".rela.dyn")?;
    let mut m = b.clone();
    let mut n = 0;
    for i in 0..rela.size / 24 {
        let at = rela.off + i * 24;
        if le64(&b, at + 8) & 0xffff_ffff == relative && le64(&b, at + 16) == w {
            m[at + 8..at + 16].copy_from_slice(&((malloc << 32) | glob_dat).to_le_bytes());
            m[at + 16..at + 24].copy_from_slice(&0u64.to_le_bytes());
            n += 1;
        }
    }
    (n > 0).then(|| Mutant { bytes: m, map: fx.map(), line: fx.line() })
}

/// The entry point made `__wrap__start` (lld's form of `-e` under
/// `--wrap=_start`).
fn entry_wrapped(fx: &Fx) -> Option<Mutant> {
    let b = fx.bytes();
    let w = sym(&syms(&b, 2), "__wrap__start")?.value;
    if word(&b, 0x18) == w {
        return None;
    }
    let mut m = b.clone();
    put_word(&mut m, 0x18, w);
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
        what: "a call to f bound to f, not __wrap_f",
        m: wrapped_call_to_f,
        cells: &["st"],
        rules: &["R1"],
        key: "sym=f site=m-",
    },
    Mut {
        what: "a call to __real_f bound to __wrap_f",
        m: real_call_to_wrap,
        cells: &["st"],
        rules: &["R1"],
        key: "sym=__real_f site=m-",
    },
    Mut {
        what: "a call in f's defining object wrapped (lld's E13 form)",
        m: own_call_wrapped,
        cells: &["st"],
        rules: &["R1"],
        key: "sym=f site=f-",
    },
    Mut {
        what: "a __real_f symbol written",
        m: real_symbol_written,
        cells: &["st"],
        rules: &["Y3"],
        key: "'__real_f'",
    },
    Mut {
        what: "f's member extracted though only a wrapped f names it",
        m: member_extracted,
        cells: &["mem"],
        rules: &["M12"],
        key: "(fo-",
    },
    Mut {
        what: "f's member missing though __real_f reaches it",
        m: member_missing,
        cells: &["real"],
        rules: &["M11"],
        key: "(fo-",
    },
    Mut {
        what: "a wrapped malloc called through malloc's PLT entry (JUMP_SLOT malloc)",
        m: malloc_call_to_plt,
        cells: &["dm"],
        rules: &["R1"],
        key: "sym=malloc site=dm-",
    },
    Mut {
        what: "the word and the GOT slot of a wrapped malloc name malloc (GLOB_DAT)",
        m: malloc_slot_named,
        cells: &["dm"],
        rules: &["P-D3", "P-D9", "P-D7", "R1"],
        key: "malloc",
    },
    Mut {
        what: "the entry point __wrap__start under --wrap=_start",
        m: entry_wrapped,
        cells: &["dm"],
        rules: &["P-D4"],
        key: "'_start'",
    },
];

/// The anomalies each linker's clean fixtures may draw: lld's forms of
/// §7.1 (A80 E13, A82 `-e`, A83 extraction), mold's `.symtab` (A9).
fn allowed(linker: &str) -> &'static [&'static str] {
    match linker {
        "lld" => &["A80", "A82", "A83"],
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
        }
    }
    assert_eq!(n, 4 * 3 * 4 + 4 * 2, "every fixture is there");
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// lld's named forms occur where §7.1 says (A80 E13 on `st`, A83 on
/// `mem`, A82 on `dm`), so the anomalies are not dead text.
#[test]
fn lld_forms_are_named() {
    for (cell, id) in [("st", "A80"), ("mem", "A83"), ("dm", "A82")] {
        for (_, _, fx) in all(cell).into_iter().filter(|(_, l, _)| *l == "lld") {
            let j = fx.judge(&fx.bytes());
            assert!(j.named.iter().any(|a| a == id), "{}: {id} not drawn: {:?}", fx.name, j.named);
        }
    }
}

/// The `gck` output keeps `f`'s section (linked without
/// `--gc-sections`); judged by `gc`'s line it is NOT caught: the verifier
/// judges why a section is dropped (M10), never that a kept one is live
/// (`spec/plumb.md` §3.3 has no rule for it). Recorded so a rule that
/// starts to catch it updates this test.
#[test]
fn gc_over_retention_is_not_judged() {
    for a in ARCHES {
        for l in LINKERS {
            let kept = Fx::new("gck", a, l);
            let gc = Fx::new("gc", a, l);
            let mut map = kept.map();
            let line = gc.line();
            if l == "mortar" {
                let i = map.args.iter().position(|w| w.starts_with(b"gcm-")).unwrap();
                map.args.insert(i, b"--gc-sections".to_vec());
            }
            let j = kept.judge_with(&kept.bytes(), &map, line);
            assert!(j.plain.is_empty(), "{}: now judged: {:?}", kept.name, j.plain);
        }
    }
}

/// The coverage of a line without `--wrap` names nothing of it.
#[test]
fn no_wrap_no_coverage() {
    let fx = Fx::new("st", "x64", "bfd");
    let j = fx.judge(&fx.bytes());
    assert!(j.coverage.wrap_not_judged.is_empty());
}

/// §9.2: each mutation is caught by a finding no anomaly names, on
/// mortar's output and on every reference's it applies to.
#[test]
fn mutations() {
    let mut missed = Vec::new();
    for x in MUTATIONS {
        let mut mortar = 0;
        let mut arches = std::collections::BTreeSet::new();
        for c in x.cells {
            for (a, l, fx) in all(c) {
                let Some(m) = (x.m)(&fx) else { continue };
                if l == "mortar" {
                    mortar += 1;
                    arches.insert(a);
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
                    Some(h) if std::env::var_os("WRAP_SHOW").is_some() => {
                        eprintln!("{}: {}: {h}", x.what, fx.name);
                    }
                    Some(_) => {}
                }
            }
        }
        let want = if x.cells.contains(&"dm") { 2 } else { 3 };
        assert_eq!(mortar, want, "{}: mortar outputs mutated on {arches:?}", x.what);
    }
    assert!(missed.is_empty(), "{}", missed.join("\n"));
}

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

// ---- symbol tables and relocations ----

/// One symbol: its entry's offset, name, `st_name`, value, and the
/// string table's file offset.
struct Sym {
    at: usize,
    name: String,
    st_name: usize,
    value: u64,
    strtab: usize,
}

/// The symbols of the first section of `sh_type` `ty` (11 `.dynsym`, 2
/// `.symtab`).
fn syms(b: &[u8], ty: u32) -> Vec<Sym> {
    let s = sections(b);
    let Some(t) = s.iter().find(|x| x.1 == ty) else { return Vec::new() };
    let strtab = s[t.5].3;
    (0..t.4 / 24)
        .map(|i| {
            let at = t.3 + i * 24;
            let st_name = le32(b, at) as usize;
            Sym { at, name: cstr(b, strtab + st_name), st_name, value: le64(b, at + 8), strtab }
        })
        .collect()
}

fn sym<'s>(v: &'s [Sym], name: &str) -> Option<(usize, &'s Sym)> {
    v.iter().enumerate().find(|(_, s)| s.name == name)
}

/// `(entry offset, r_info, r_addend)` of every `SHT_RELA` entry.
fn relas(b: &[u8]) -> Vec<(usize, u64, u64)> {
    sections(b)
        .iter()
        .filter(|x| x.1 == 4)
        .flat_map(|x| (0..x.4 / 24).map(move |i| x.3 + i * 24))
        .map(|at| (at, le64(b, at + 8), le64(b, at + 16)))
        .collect()
}

fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn put64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

fn is_a64(b: &[u8]) -> bool {
    le16(b, 0x12) == 183
}
fn glob_dat(b: &[u8]) -> u64 {
    if is_a64(b) { 1025 } else { 6 }
}
fn relative(b: &[u8]) -> u64 {
    if is_a64(b) { 1027 } else { 8 }
}

/// `_end`'s value (the static symbol table's, which a local `_end` has
/// too).
fn end_value(b: &[u8]) -> Option<u64> {
    sym(&syms(b, 2), "_end").map(|(_, s)| s.value)
}

/// Points the `.dynsym` entry `from` at the tail of its own name that
/// starts `skip` bytes in, and gives it the static symbol `like`'s value
/// and section (when the output has one), `NOTYPE`, size 0.
fn rename(b: &mut [u8], from: &str, skip: usize, like: &str) -> Option<()> {
    let d = syms(b, 11);
    let (_, s) = sym(&d, from)?;
    let (at, st) = (s.at, s.st_name + skip);
    put32(b, at, st as u32);
    if let Some((_, l)) = sym(&syms(b, 2), like) {
        // `like`'s value and section, `NOTYPE`, size 0.
        let (v, ndx) = (l.value, le16(b, l.at + 6) as u16);
        put64(b, at + 8, v);
        b[at + 6..at + 8].copy_from_slice(&ndx.to_le_bytes());
        b[at + 4] &= 0xf0;
        put64(b, at + 16, 0);
    }
    Some(())
}

/// The mutations of WP-166 item 4: each returns the mutated bytes, or
/// `None` when the output has nothing to mutate.
type Mutation = fn(&[u8]) -> Option<Vec<u8>>;

/// `_end` taken out of `.dynsym`: its name's first byte changed (`Xend`;
/// `end` may share the tail of the string).
fn drop_end(b: &[u8]) -> Option<Vec<u8>> {
    let d = syms(b, 11);
    let (_, s) = sym(&d, "_end")?;
    let mut m = b.to_vec();
    m[s.strtab + s.st_name] = b'X';
    Some(m)
}

/// `_end`'s `GLOB_DAT` replaced by a `RELATIVE` of its value.
fn glob_to_relative(b: &[u8]) -> Option<Vec<u8>> {
    let (i, s) = sym(&syms(b, 11), "_end").map(|(i, s)| (i, s.value))?;
    let (at, ..) = *relas(b).iter().find(|r| r.1 == ((i as u64) << 32 | glob_dat(b)))?;
    let mut m = b.to_vec();
    put64(&mut m, at + 8, relative(b));
    put64(&mut m, at + 16, s);
    Some(m)
}

/// The `RELATIVE` of `_end`'s value replaced by a `GLOB_DAT` of `_end`,
/// moved to the table's end, `DT_RELACOUNT` one lower (so the edit is the
/// record's binding alone).
fn relative_to_glob(b: &[u8]) -> Option<Vec<u8>> {
    let (i, _) = sym(&syms(b, 11), "_end")?;
    let v = end_value(b)?;
    let r = relas(b);
    let k = r.iter().position(|r| r.1 == relative(b) && r.2 == v)?;
    let mut m = b.to_vec();
    let mut rows: Vec<[u8; 24]> =
        r.iter().map(|x| b[x.0..x.0 + 24].try_into().unwrap()).collect();
    let mut row = rows.remove(k);
    row[8..16].copy_from_slice(&((i as u64) << 32 | glob_dat(b)).to_le_bytes());
    row[16..24].copy_from_slice(&0u64.to_le_bytes());
    rows.push(row);
    for (x, row) in r.iter().zip(&rows) {
        m[x.0..x.0 + 24].copy_from_slice(row);
    }
    let s = sections(b);
    let d = s.iter().find(|x| x.1 == 6)?;
    for at in (d.3..d.3 + d.4).step_by(16) {
        if le64(b, at) == 0x6fff_fff9 {
            put64(&mut m, at + 8, le64(b, at + 8) - 1);
        }
    }
    Some(m)
}

/// `end`, referenced by no object, exported with `_end`'s value: the
/// entry of `api_end` (a library's) or of `_edata` (a program's) pointed
/// at the `end` that ends `api_end` or `_end`.
fn export_end(b: &[u8]) -> Option<Vec<u8>> {
    let mut m = b.to_vec();
    let d = syms(b, 11);
    if sym(&d, "api_end").is_some_and(|(_, s)| le16(b, s.at + 6) != 0) {
        rename(&mut m, "api_end", 4, "_end")?;
    } else {
        let (_, e) = sym(&d, "_end")?;
        let (_, s) = sym(&d, "_edata")?;
        put32(&mut m, s.at, (e.st_name + 1) as u32);
        put64(&mut m, s.at + 8, e.value);
    }
    Some(m)
}

/// `_end` exported though `local: *` hides it: `api_end`'s entry pointed
/// at the `_end` ending its name, with `_end`'s value.
fn export_local(b: &[u8]) -> Option<Vec<u8>> {
    let mut m = b.to_vec();
    rename(&mut m, "api_end", 3, "_end")?;
    Some(m)
}

/// GNU ld's `__etext`, a linker-defined key outside the seven, exported:
/// `api__etext` pointed at the `__etext` ending it, with `__etext`'s value.
fn export_etext(b: &[u8]) -> Option<Vec<u8>> {
    let mut m = b.to_vec();
    rename(&mut m, "api__etext", 3, "__etext")?;
    Some(m)
}

/// `__executable_start` exported (`api__executable_start`'s entry).
fn export_executable_start(b: &[u8]) -> Option<Vec<u8>> {
    let mut m = b.to_vec();
    rename(&mut m, "api__executable_start", 3, "__executable_start")?;
    Some(m)
}

/// The `RELATIVE` of `_end`'s value (one past a `PT_LOAD`'s end) moved 2
/// bytes further, its GOT slot too.
fn past_end(b: &[u8]) -> Option<Vec<u8>> {
    let v = end_value(b)?;
    let r = relas(b);
    let (at, ..) = *r.iter().find(|r| r.1 == relative(b) && r.2 == v)?;
    let mut m = b.to_vec();
    put64(&mut m, at + 16, v + 2);
    let slot = file_of(b, le64(b, at));
    if le64(b, slot) == v {
        put64(&mut m, slot, v + 2);
    }
    Some(m)
}

/// A mutation, the cells it applies to, and the finding that must catch
/// it: one of the rules, its message naming the key (when not empty).
struct Mut {
    what: &'static str,
    m: Mutation,
    cells: &'static [&'static str],
    rules: &'static [&'static str],
    key: &'static str,
    /// The linkers whose outputs are not judged: the mutated output is
    /// the linker's own named behaviour.
    skip: &'static [&'static str],
}

const fn mu(
    what: &'static str,
    m: Mutation,
    cells: &'static [&'static str],
    rules: &'static [&'static str],
    key: &'static str,
) -> Mut {
    Mut { what, m, cells, rules, key, skip: &[] }
}

const MUTATIONS: &[Mut] = &[
    mu("_end out of .dynsym", drop_end, &["sh", "exref-pie", "exref-nopie"], &["P-D19", "P-D7", "P-D17"], "'_end'"),
    mu("GLOB_DAT -> RELATIVE", glob_to_relative, &["sh", "shfn"], &["P-D17", "P-D25"], "'_end'"),
    // A reference's record naming a key its `DF_SYMBOLIC` line binds is
    // glibc's own binding (P-D25, GNU ld's form), not a finding.
    Mut {
        skip: &["lld", "bfd", "mold"],
        ..mu("RELATIVE -> GLOB_DAT under -Bsymbolic", relative_to_glob, &["shsym"], &["P-D25"], "'_end'")
    },
    mu("an unreferenced end exported", export_end, &["sh"], &["P-D19"], "'end' "),
    // Without -E: GNU ld's A60 is the -E form alone.
    mu("an unreferenced end exported", export_end, &["exref-pie", "exref-nopie"], &["P-D7"], "'end' "),
    mu("_end exported though local:", export_local, &["shloc"], &["P-D19"], "'_end'"),
    mu("__etext exported", export_etext, &["shx"], &["P-D19", "P-D7"], "'__etext'"),
    mu("__executable_start exported", export_executable_start, &["shx"], &["P-D19"], "'__executable_start'"),
    mu("the one-past-the-end RELATIVE moved by 2", past_end, &["shsym", "shloc", "pie-e"], &["P-D9", "R4"], ""),
];

/// The anomalies each linker's clean fixtures may draw.
fn allowed(linker: &str) -> &'static [&'static str] {
    match linker {
        "bfd" => &["A58", "A59", "A60"],
        "mold" => &["A3", "A9", "A57"],
        _ => &[],
    }
}

fn all(c: &str, a: &str) -> Vec<(&'static str, Fx)> {
    let n = format!("{c}-{a}");
    vec![
        ("mortar", Fx::mortar(&n)),
        ("lld", Fx::reference(&n, "lld")),
        ("bfd", Fx::reference(&n, "bfd")),
        ("mold", Fx::reference(&n, "mold")),
    ]
}

/// Every fixture is clean: mortar's draws nothing; a reference's
/// findings are its linker's named anomalies.
#[test]
fn clean() {
    let mut bad = Vec::new();
    for a in ["a64", "x64"] {
        for c in CELLS {
            for (l, fx) in all(c, a) {
                let (plain, named) = fx.judge(&fx.bytes());
                for p in plain {
                    bad.push(format!("{}: {p}", fx.name));
                }
                for n in named.iter().filter(|n| !allowed(l).contains(&n.as_str())) {
                    bad.push(format!("{}: unexpected {n}", fx.name));
                }
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// WP-166 item 4: each mutation is caught by a finding no anomaly names,
/// on mortar's output and on every reference's it applies to.
#[test]
fn mutations() {
    let mut missed = Vec::new();
    for x in MUTATIONS {
        let mut mortar = 0;
        for a in ["a64", "x64"] {
            for c in x.cells {
                for (l, fx) in all(c, a) {
                    if x.skip.contains(&l) {
                        continue;
                    }
                    let Some(b) = (x.m)(&fx.bytes()) else { continue };
                    if l == "mortar" {
                        mortar += 1;
                    }
                    let (plain, _) = fx.judge(&b);
                    let hit = plain.iter().find(|p| {
                        x.rules.iter().any(|r| p.starts_with(&format!("{r} ")))
                            && p.contains(x.key)
                    });
                    match hit {
                        None => missed.push(format!("{}: {} not caught: {}", x.what, fx.name, plain.join(" | "))),
                        Some(h) if std::env::var_os("LDEF_SHOW").is_some() => {
                            eprintln!("{}: {}: {h}", x.what, fx.name);
                        }
                        Some(_) => {}
                    }
                }
            }
        }
        assert!(mortar > 0, "{}: no mortar output to mutate", x.what);
    }
    assert!(missed.is_empty(), "{}", missed.join("\n"));
}

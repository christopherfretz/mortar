//! synth S-75, S-77, S-121 and dyn.md D-114 as WP-150 decided them,
//! under the verifier (WP-169): mortar's, lld's, GNU ld's and mold's
//! `-nostdlib` links of version-local keys, a local common, `.symver`
//! definitions, `__start_`/`__stop_` under `local: *`, `__bss_start`
//! without `.data` and `--exclude-libs` executables, checked in under
//! `tests/symloc/` (`gen.sh`), on AArch64 and x86-64.
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
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/symloc")
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

fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn put64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
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

const CELLS: &[&str] = &["vl", "gc", "sv", "exl-pie", "exl-nopie"];

fn all(c: &str, a: &str) -> Vec<(&'static str, Fx)> {
    let n = format!("{c}-{a}");
    vec![
        ("mortar", Fx::mortar(&n)),
        ("lld", Fx::reference(&n, "lld")),
        ("bfd", Fx::reference(&n, "bfd")),
        ("mold", Fx::reference(&n, "mold")),
    ]
}

#[test]
#[ignore = "prints every fixture's findings"]
fn survey() {
    for a in ["a64", "x64"] {
        for c in CELLS {
            for (_, fx) in all(c, a) {
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

// ---- edits of `.symtab` ----

/// `.symtab`'s section header offset, entries' offset and count, and
/// `sh_info` (the first global's index).
fn symtab(b: &[u8]) -> Option<(usize, usize, usize, usize)> {
    let shoff = le64(b, 0x28) as usize;
    let n = le16(b, 0x3c);
    (0..n).map(|i| shoff + i * 64).find(|&h| le32(b, h + 4) == 2).map(|h| {
        (h, le64(b, h + 0x18) as usize, le64(b, h + 0x20) as usize / 24, le32(b, h + 0x2c) as usize)
    })
}

/// Swaps `.symtab` entries `i` and `j`.
fn swap(m: &mut [u8], off: usize, i: usize, j: usize) {
    let (a, c) = (off + i * 24, off + j * 24);
    let x: [u8; 24] = m[a..a + 24].try_into().unwrap();
    m.copy_within(c..c + 24, a);
    m[c..c + 24].copy_from_slice(&x);
}

/// The local `name` made `GLOBAL` (moved to the first global's place,
/// `sh_info` one lower). `None` when it is not local.
fn make_global(b: &[u8], name: &str) -> Option<Vec<u8>> {
    let (h, off, _, info) = symtab(b)?;
    let t = syms(b, 2);
    let (i, _) = sym(&t, name).filter(|(i, _)| *i < info)?;
    let mut m = b.to_vec();
    swap(&mut m, off, i, info - 1);
    let at = off + (info - 1) * 24 + 4;
    m[at] = (m[at] & 0x0f) | 0x10;
    put32(&mut m, h + 0x2c, (info - 1) as u32);
    Some(m)
}

/// The global `name` made `LOCAL` (moved to the last local's place,
/// `sh_info` one higher). `None` when it is not global.
fn make_local(b: &[u8], name: &str) -> Option<Vec<u8>> {
    let (h, off, _, info) = symtab(b)?;
    let t = syms(b, 2);
    let (i, _) = sym(&t, name).filter(|(i, _)| *i >= info)?;
    let mut m = b.to_vec();
    swap(&mut m, off, i, info);
    let at = off + info * 24 + 4;
    m[at] &= 0x0f;
    put32(&mut m, h + 0x2c, (info + 1) as u32);
    Some(m)
}

/// `name`'s `.symtab` visibility set to `vis`; `None` when it has it.
fn set_vis(b: &[u8], name: &str, vis: u8) -> Option<Vec<u8>> {
    let t = syms(b, 2);
    let (_, s) = sym(&t, name).filter(|(_, s)| b[s.at + 5] & 3 != vis)?;
    let mut m = b.to_vec();
    m[s.at + 5] = (m[s.at + 5] & !3) | vis;
    Some(m)
}

/// The mutations of WP-169 item 4.
type Mutation = fn(&[u8]) -> Option<Vec<u8>>;

/// A version-local key (`foo`; the excluded `xfoo`) left `GLOBAL`.
fn local_left_global(b: &[u8]) -> Option<Vec<u8>> {
    make_global(b, "foo").or_else(|| make_global(b, "xfoo"))
}

/// Its visibility changed to `PROTECTED` (its own is `DEFAULT`).
fn local_vis(b: &[u8]) -> Option<Vec<u8>> {
    set_vis(b, "foo", 3).or_else(|| set_vis(b, "xfoo", 3))
}

/// mortar's own `PROTECTED` version-local `bar` made `DEFAULT` (GNU ld's
/// form: mortar's alone is judged).
fn own_vis_dropped(b: &[u8]) -> Option<Vec<u8>> {
    set_vis(b, "bar", 0)
}

/// `foo@@V2`'s `.symtab` name stripped to `foo` (a NUL at its `@`).
fn symver_stripped(b: &[u8]) -> Option<Vec<u8>> {
    let t = syms(b, 2);
    let (_, s) = sym(&t, "foo@@V2")?;
    let mut m = b.to_vec();
    m[s.strtab + s.st_name + 3] = 0;
    Some(m)
}

/// `foo@V1`'s and `foo@@V2`'s `.symtab` names swapped.
fn versions_swapped(b: &[u8]) -> Option<Vec<u8>> {
    let t = syms(b, 2);
    let (_, x) = sym(&t, "foo@V1")?;
    let (_, y) = sym(&t, "foo@@V2")?;
    let mut m = b.to_vec();
    put32(&mut m, x.at, y.st_name as u32);
    put32(&mut m, y.at, x.st_name as u32);
    Some(m)
}

/// `__start_mysec` under `local: *` left `GLOBAL`.
fn start_global(b: &[u8]) -> Option<Vec<u8>> {
    make_global(b, "__start_mysec")
}

/// `__start_mysec` exported: `api__start_mysec`'s `.dynsym` entry
/// pointed at the `__start_mysec` ending its name, with its value.
fn start_exported(b: &[u8]) -> Option<Vec<u8>> {
    let mut m = b.to_vec();
    rename(&mut m, "api__start_mysec", 3, "__start_mysec")?;
    Some(m)
}

/// A global common (`cm`, no version script) made `LOCAL` with no reason.
fn common_local(b: &[u8]) -> Option<Vec<u8>> {
    make_local(b, "cm")
}

/// `__bss_start` (`.symtab`) moved 8 bytes into `.bss`.
fn bss_moved(b: &[u8]) -> Option<Vec<u8>> {
    let t = syms(b, 2);
    let (_, s) = sym(&t, "__bss_start")?;
    let mut m = b.to_vec();
    put64(&mut m, s.at + 8, s.value + 8);
    Some(m)
}

/// A mutation, the cells it applies to, and the finding that must catch
/// it: one of the rules, its message naming the key.
struct Mut {
    what: &'static str,
    m: Mutation,
    cells: &'static [&'static str],
    rules: &'static [&'static str],
    key: &'static str,
    /// The linkers whose outputs are not judged: the mutated output is
    /// the linker's own named form.
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
    mu("a version-local key left GLOBAL", local_left_global, &["vl"], &["Y3"], "'foo'"),
    mu(
        "an excluded key left GLOBAL",
        local_left_global,
        &["exl-pie", "exl-nopie"],
        &["Y3"],
        "'xfoo'",
    ),
    mu("a version-local key's visibility changed", local_vis, &["vl"], &["Y3"], "'foo'"),
    mu(
        "an excluded key's visibility changed",
        local_vis,
        &["exl-pie", "exl-nopie"],
        &["Y3"],
        "'xfoo'",
    ),
    Mut {
        skip: &["lld", "bfd", "mold"],
        ..mu(
            "a version-local key's own visibility dropped",
            own_vis_dropped,
            &["vl"],
            &["Y3"],
            "'bar'",
        )
    },
    // lld and mold write `foo` twice (named, WP-150 item 2).
    Mut {
        skip: &["lld", "mold"],
        ..mu("foo@@V2 stripped to foo", symver_stripped, &["sv"], &["P-D7"], "foo@@V2")
    },
    Mut {
        skip: &["lld", "mold"],
        ..mu("foo@V1 and foo@@V2 swapped", versions_swapped, &["sv"], &["P-D7"], "foo@")
    },
    // lld keeps them exported GLOBAL PROTECTED (A61).
    Mut {
        skip: &["lld"],
        ..mu("__start_mysec left GLOBAL", start_global, &["vl"], &["Y5"], "'__start_mysec'")
    },
    Mut {
        skip: &["lld"],
        ..mu("__start_mysec exported", start_exported, &["vl"], &["P-D19"], "'__start_mysec'")
    },
    mu("a global common made LOCAL", common_local, &["gc"], &["M7", "Y3", "P-D7"], "'cm'"),
    mu("__bss_start moved into .bss", bss_moved, &["vl", "gc"], &["Y5"], "'__bss_start'"),
];

/// The anomalies each linker's clean fixtures may draw.
fn allowed(linker: &str) -> &'static [&'static str] {
    match linker {
        "lld" => &["A61"],
        "bfd" => &["A60"],
        "mold" => &["A3", "A57"],
        _ => &[],
    }
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

/// WP-169 item 4: each mutation is caught by a finding no anomaly names,
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
                        x.rules.iter().any(|r| p.starts_with(&format!("{r} "))) && p.contains(x.key)
                    });
                    match hit {
                        None => missed.push(format!(
                            "{}: {} not caught: {}",
                            x.what,
                            fx.name,
                            plain.join(" | ")
                        )),
                        Some(h) if std::env::var_os("SYMLOC_SHOW").is_some() => {
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

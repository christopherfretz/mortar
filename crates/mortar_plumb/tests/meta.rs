//! Stage 4 (`spec/plumb.md` §13; WP-078): the rules P-M1-P-M3 on mortar's
//! PIEs with `--odr-table=full` and `--keep-mason-meta`, AArch64 and
//! x86-64 (`tests/meta/gen.sh`), accepted as linked, and one mutant per
//! field a rule reads, each caught by its rule and sub-rule name.

use std::path::PathBuf;

use mortar_plumb::{Ctx, Files, Format, Input, Report};

const ARCHES: [&str; 2] = ["aarch64", "x86_64"];

/// A change to an output's bytes.
type Mutation = Box<dyn Fn(&mut Vec<u8>)>;

struct Dir(PathBuf);

impl Files for Dir {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(path)).ok()
    }
}

fn dir(arch: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/meta").join(arch)
}

struct Fx {
    dir: PathBuf,
    prog: Vec<u8>,
    map: String,
}

fn fixture(arch: &str) -> Fx {
    let d = dir(arch);
    Fx {
        prog: std::fs::read(d.join("prog")).unwrap(),
        map: std::fs::read_to_string(d.join("prog.map")).unwrap(),
        dir: d,
    }
}

fn judge(fx: &Fx, prog: &[u8], map_text: &str) -> Report {
    let map = mortar_plumb::map::parse(Format::Mortar, map_text).unwrap();
    let files: Vec<(String, Vec<u8>)> = map
        .paths()
        .into_iter()
        .filter_map(|p| std::fs::read(fx.dir.join(p)).ok().map(|b| (p.to_string(), b)))
        .collect();
    let inputs: Vec<Input<'_>> = files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
    let files = Dir(fx.dir.clone());
    mortar_plumb::verify_with(prog, &inputs, &map, &Ctx { line: None, files: &files })
}

fn keys(r: &Report) -> Vec<String> {
    r.findings.iter().map(|f| format!("{} {}", f.rule.id(), f.message)).collect()
}

/// The mutant is caught by `rule` with a message starting `sub`.
fn caught(fx: &Fx, prog: &[u8], map: &str, rule: &str, sub: &str, what: &str) {
    let r = judge(fx, prog, map);
    assert!(
        r.findings.iter().any(|f| f.rule.id() == rule && f.message.starts_with(sub)),
        "{}: {what}: not caught as {rule} '{sub}': {:#?}",
        fx.dir.display(),
        keys(&r)
    );
}

// ---- ELF64 little-endian helpers (the fixtures are ELF64) ----

fn rd(b: &[u8], o: usize, n: usize) -> u64 {
    let mut v = 0u64;
    for i in (0..n).rev() {
        v = (v << 8) | u64::from(b[o + i]);
    }
    v
}

fn wr(b: &mut [u8], o: usize, n: usize, v: u64) {
    for i in 0..n {
        b[o + i] = (v >> (8 * i)) as u8;
    }
}

/// (section header offset, name, file offset, size).
fn sections(b: &[u8]) -> Vec<(usize, String, usize, usize)> {
    let shoff = rd(b, 0x28, 8) as usize;
    let n = rd(b, 0x3c, 2) as usize;
    let strndx = rd(b, 0x3e, 2) as usize;
    let str_off = rd(b, shoff + strndx * 64 + 0x18, 8) as usize;
    (0..n)
        .map(|i| {
            let h = shoff + i * 64;
            let name_at = str_off + rd(b, h, 4) as usize;
            let end = b[name_at..].iter().position(|&c| c == 0).unwrap();
            let name = String::from_utf8_lossy(&b[name_at..name_at + end]).into_owned();
            (h, name, rd(b, h + 0x18, 8) as usize, rd(b, h + 0x20, 8) as usize)
        })
        .collect()
}

fn section(b: &[u8], name: &str) -> (usize, usize, usize) {
    let s = sections(b).into_iter().find(|s| s.1 == name).unwrap();
    (s.0, s.2, s.3)
}

/// The descriptor offsets of `.note.mason`'s two notes.
fn notes(b: &[u8]) -> (usize, usize) {
    let (_, off, _) = section(b, ".note.mason");
    let d1 = off + 24;
    let first = d1 + rd(b, off + 4, 4) as usize;
    (d1, first + 24)
}

fn map_without(map: &str, line: &str) -> String {
    assert!(map.lines().any(|l| l == line), "{line}");
    let mut out = String::new();
    for l in map.lines().filter(|l| *l != line) {
        out.push_str(l);
        out.push('\n');
    }
    out
}

fn map_with(map: &str, before: &str, line: &str) -> String {
    map.replacen(before, &format!("{line}\n{before}"), 1)
}

#[test]
fn the_fixtures_are_accepted() {
    for arch in ARCHES {
        let fx = fixture(arch);
        let r = judge(&fx, &fx.prog, &fx.map);
        assert!(r.clean(), "{arch}: {:#?}", keys(&r));
        let m = &r.coverage.meta;
        assert_eq!(
            (m.outputs, m.units, m.tables, m.rows, m.full_rows),
            (1, 3, 1, 3, 3),
            "{arch}: {m:?}"
        );
        assert!(m.options_known && m.kept_bytes == 888 && m.not_judged == 0, "{arch}: {m:?}");
        assert!(r.summary().contains("ODR metadata: live units 3"), "{arch}");
    }
}

/// P-M1: the kept section, field by field, and the options and map
/// records it is judged against.
#[test]
fn p_m1_mutants() {
    for arch in ARCHES {
        let fx = fixture(arch);
        let (h, off, size) = section(&fx.prog, ".mason.meta");
        let mut p = fx.prog.clone();
        p[off + size / 2] ^= 1;
        caught(&fx, &p, &fx.map, "P-M1", "kept section", "a byte of the kept section");
        let mut p = fx.prog.clone();
        wr(&mut p, h + 0x20, 8, (size - 8) as u64);
        caught(&fx, &p, &fx.map, "P-M1", "kept section", "the kept section's size");
        let mut p = fx.prog.clone();
        wr(&mut p, h + 0x08, 8, 0x8000_0000);
        caught(&fx, &p, &fx.map, "P-M1", "kept section", "the kept section's flags");
        let mut p = fx.prog.clone();
        wr(&mut p, h + 0x30, 8, 1);
        caught(&fx, &p, &fx.map, "P-M1", "kept section", "the kept section's alignment");
        let mut p = fx.prog.clone();
        wr(&mut p, h + 0x04, 4, 7);
        caught(&fx, &p, &fx.map, "P-M1", "kept section", "the kept section's type");
        let m = map_without(&fx.map, "arg --keep-mason-meta");
        caught(&fx, &fx.prog, &m, "P-M1", "kept section", "the option dropped");
        let chunk = fx.map.lines().find(|l| l.starts_with("chunk synthetic .mason.meta")).unwrap();
        let m = map_without(&fx.map, chunk);
        caught(&fx, &fx.prog, &m, "P-M1", "kept section", "the map's chunk dropped");
        // An input container placed: a non-alloc record for b.o's.
        let b = std::fs::read(fx.dir.join("b.o")).unwrap();
        let idx = sections(&b).iter().position(|s| s.1 == ".mason.meta").unwrap();
        let m =
            map_with(&fx.map, "chunk synthetic .interp", &format!("nplace 1 {idx} .comment 0 8"));
        caught(&fx, &fx.prog, &m, "P-M1", "container", "an input container copied");
    }
}

/// P-M2 over the first note: its place, header, order and each row field.
#[test]
fn p_m2_table_mutants() {
    for arch in ARCHES {
        let fx = fixture(arch);
        let (h, off, _) = section(&fx.prog, ".note.mason");
        let (d1, _) = notes(&fx.prog);
        let row = move |i: usize| d1 + 16 + 24 * i;
        let cases: Vec<(&str, &str, Mutation)> = vec![
            ("place", "section alignment", Box::new(move |p| wr(p, h + 0x30, 8, 4))),
            ("place", "section type", Box::new(move |p| wr(p, h + 0x04, 4, 1))),
            ("place", "section flags", Box::new(move |p| wr(p, h + 0x08, 8, 0))),
            ("place", "the owner", Box::new(move |p| p[off + 12] ^= 0x20)),
            ("place", "n_namesz", Box::new(move |p| wr(p, off, 4, 9))),
            ("place", "n_type", Box::new(move |p| wr(p, off + 8, 4, 0x4d4f_4403))),
            ("place", "n_descsz", Box::new(move |p| wr(p, off + 4, 4, 0x1000))),
            ("place", "the name's padding", Box::new(move |p| p[off + 23] = 1)),
            ("header", "version", Box::new(move |p| wr(p, d1, 4, 2))),
            ("header", "count", Box::new(move |p| wr(p, d1 + 4, 4, 2))),
            ("header", "row_size", Box::new(move |p| wr(p, d1 + 8, 4, 32))),
            ("header", "reserved", Box::new(move |p| wr(p, d1 + 12, 4, 1))),
            ("header", "a row's reserved", Box::new(move |p| p[row(1) + 22] = 1)),
            (
                "order",
                "two rows swapped",
                Box::new(move |p| {
                    let a = p[row(0)..row(1)].to_vec();
                    let b = p[row(1)..row(2)].to_vec();
                    p[row(0)..row(1)].copy_from_slice(&b);
                    p[row(1)..row(2)].copy_from_slice(&a);
                }),
            ),
            ("key", "a key", Box::new(move |p| p[row(2) + 7] ^= 0x01)),
            ("hash", "a hash", Box::new(move |p| p[row(1) + 12] ^= 0x40)),
            ("flags", "NONPREEMPTIBLE cleared", Box::new(move |p| p[row(0) + 21] ^= 0x80)),
            ("flags", "VAGUE flipped", Box::new(move |p| p[row(1) + 20] ^= 0x01)),
            ("flags", "SYSTEM flipped", Box::new(move |p| p[row(2) + 21] ^= 0x01)),
            (
                "row set",
                "a .dynsym index past the end",
                Box::new(move |p| wr(p, row(2) + 16, 4, 999)),
            ),
        ];
        for (sub, what, f) in cases {
            let mut p = fx.prog.clone();
            f(&mut p);
            caught(&fx, &p, &fx.map, "P-M2", sub, what);
        }
        // The row set: a row pointed at an exported entry that has none
        // (the key made to match, so only the row set can see it).
        let r = judge(&fx, &fx.prog, &fx.map);
        assert!(r.clean());
        let dynsym = section(&fx.prog, ".dynsym");
        let dynstr = section(&fx.prog, ".dynstr");
        let name_of = |p: &[u8], i: usize| {
            let at = dynstr.1 + rd(p, dynsym.1 + 24 * i, 4) as usize;
            let end = p[at..].iter().position(|&c| c == 0).unwrap();
            p[at..at + end].to_vec()
        };
        let strong = (1..dynsym.2 / 24).find(|&i| name_of(&fx.prog, i) == b"_Z6strongv").unwrap();
        let mut p = fx.prog.clone();
        wr(&mut p, row(2) + 16, 4, strong as u64);
        wr(&mut p, row(2), 8, fnv(b"_Z6strongv"));
        caught(&fx, &p, &fx.map, "P-M2", "row set", "a row for a definition O-50 gives none");
        // The same mutant leaves the row of '_Z2wdv' missing: the other
        // half of the row set.
        let r = judge(&fx, &p, &fx.map);
        assert!(
            r.findings.iter().any(|f| f.rule.id() == "P-M2"
                && f.message.starts_with("row set: '_Z2wdv'")
                && f.message.ends_with("no row names it (O-50)")),
            "{arch}: {:#?}",
            keys(&r)
        );
        // The options: the table not asked for, and =exported for =full.
        let m = map_without(&fx.map, "arg --odr-table=full");
        caught(&fx, &fx.prog, &m, "P-M2", "presence", "--odr-table dropped");
        let m = fx.map.replace("arg --odr-table=full", "arg --odr-table=exported");
        caught(&fx, &fx.prog, &m, "P-M2", "presence", "=exported for =full");
        let m = map_with(&fx.map, "arg --mortar-map", "arg --odr=off");
        caught(&fx, &fx.prog, &m, "P-M2", "presence", "--odr=off");
        // The segment.
        let phoff = rd(&fx.prog, 0x20, 8) as usize;
        let phnum = rd(&fx.prog, 0x38, 2) as usize;
        let ph = (0..phnum)
            .map(|i| phoff + 56 * i)
            .find(|&ph| rd(&fx.prog, ph, 4) == 4 && rd(&fx.prog, ph + 0x30, 8) == 8)
            .unwrap();
        let mut p = fx.prog.clone();
        wr(&mut p, ph + 0x30, 8, 4);
        caught(&fx, &p, &fx.map, "P-M2", "place", "PT_NOTE's p_align");
        let mut p = fx.prog.clone();
        wr(&mut p, ph + 0x20, 8, 8);
        caught(&fx, &p, &fx.map, "P-M2", "place", "PT_NOTE's p_filesz");
    }
}

fn fnv(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for c in b {
        h ^= u64::from(*c);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// P-M2 over `=full`'s note: header, order, and each field of the
/// definition and type rows.
#[test]
fn p_m2_full_mutants() {
    for arch in ARCHES {
        let fx = fixture(arch);
        let (_, d2) = notes(&fx.prog);
        let row = move |i: usize| d2 + 16 + 32 * i;
        // Row 0: the hidden definition; rows 1, 2: the types.
        let cases: Vec<(&str, Mutation)> = vec![
            ("header version", Box::new(move |p| wr(p, d2, 4, 0))),
            ("header definitions", Box::new(move |p| wr(p, d2 + 4, 4, 2))),
            ("header row_size", Box::new(move |p| wr(p, d2 + 8, 4, 24))),
            ("header types", Box::new(move |p| wr(p, d2 + 12, 4, 1))),
            ("definition key", Box::new(move |p| p[row(0) + 3] ^= 0x10)),
            ("definition hash", Box::new(move |p| p[row(0) + 9] ^= 0x10)),
            ("definition layout", Box::new(move |p| p[row(0) + 16] = 1)),
            ("definition size", Box::new(move |p| p[row(0) + 24] = 1)),
            ("definition alignment", Box::new(move |p| p[row(0) + 28] = 1)),
            ("definition flags", Box::new(move |p| p[row(0) + 30] ^= 0x01)),
            ("definition NONPREEMPTIBLE", Box::new(move |p| p[row(0) + 31] ^= 0x80)),
            ("type key", Box::new(move |p| p[row(1) + 7] ^= 0x01)),
            ("type hash", Box::new(move |p| p[row(1) + 8] ^= 0x01)),
            ("type layout", Box::new(move |p| p[row(1) + 16] ^= 0x01)),
            ("type size", Box::new(move |p| p[row(2) + 24] ^= 0x01)),
            ("type alignment", Box::new(move |p| p[row(2) + 28] ^= 0x01)),
            ("type flags", Box::new(move |p| p[row(2) + 30] ^= 0x01)),
            (
                "types swapped",
                Box::new(move |p| {
                    let a = p[row(1)..row(2)].to_vec();
                    let b = p[row(2)..row(3)].to_vec();
                    p[row(1)..row(2)].copy_from_slice(&b);
                    p[row(2)..row(3)].copy_from_slice(&a);
                }),
            ),
        ];
        for (what, f) in cases {
            let mut p = fx.prog.clone();
            f(&mut p);
            caught(&fx, &p, &fx.map, "P-M2", "full", what);
        }
    }
}

/// P-M3: the initialiser array and the inputs.
#[test]
fn p_m3_mutants() {
    for arch in ARCHES {
        let fx = fixture(arch);
        let (h, _, _) = section(&fx.prog, ".init_array");
        let addr = rd(&fx.prog, h + 0x10, 8);
        // One more entry in .init_array than the inputs give.
        let mut p = fx.prog.clone();
        wr(&mut p, h + 0x20, 8, 16);
        let r = judge(&fx, &p, &fx.map);
        assert!(
            r.findings.iter().any(|f| f.rule.id() == "P-M3" && f.message.starts_with("init array")),
            "{arch}: {:#?}",
            keys(&r)
        );
        // A chunk of the linker's own in it.
        let m = map_with(
            &fx.map,
            "chunk synthetic .interp",
            &format!("chunk synthetic .init_array {addr:x} 8"),
        );
        caught(&fx, &fx.prog, &m, "P-M3", "init array", "a chunk in .init_array");
        // An input that is not on the command line.
        let m = map_without(&fx.map, "arg b.o");
        caught(&fx, &fx.prog, &m, "P-M3", "input", "an input not on the line");
    }
}

/// Stage 4 never panics: every single-byte change of the two notes and
/// the kept section of one fixture is judged.
#[test]
fn hostile_tables_never_panic() {
    let fx = fixture("aarch64");
    let (_, off, size) = section(&fx.prog, ".note.mason");
    let (_, koff, ksize) = section(&fx.prog, ".mason.meta");
    for i in (off..off + size).chain(koff..koff + ksize.min(64)) {
        for v in [0u8, 0xff] {
            let mut p = fx.prog.clone();
            p[i] = v;
            let _ = judge(&fx, &p, &fx.map);
        }
    }
}

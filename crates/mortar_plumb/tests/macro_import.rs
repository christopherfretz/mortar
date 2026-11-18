//! `spec/image.md` I-47a as the verifier judges it (`spec/plumb.md` §5.8,
//! WP-250): a debug reference into a dropped COMDAT copy of a non-alloc
//! group member (GCC `-g3`'s `DW_MACRO_import`) holds the kept copy's
//! place + `st_value` + A when the keeper's copy has exactly one live,
//! non-alloc member of the name and size, else I-47's tombstone; and the
//! reference linkers' columns of the same records.
//!
//! The mortar fixtures (`tests/macro_import/gen.sh`) were linked before
//! the linker wrote I-47a, so they hold the tombstone in every such field;
//! each test writes the field it judges itself:
//! - `gcc/<arch>/dw<v>`: GCC 14 `-O0 -g3` C, three units sharing headers;
//!   `imports.txt` lists each import of a dropped copy with the kept
//!   copy's offset, read by WP-249's own reader (`macros.py`);
//! - `hand/<arch>`: `hand/k.s` (the keeper) and `hand/d.s`, one group per
//!   branch of I-47a's condition.
//!
//! The reference linkers (GNU ld, lld 22, mold) are run here, on the same
//! sources (C and C++, DWARF 4 and 5) and on WP-249's probe of GNU ld's
//! condition (`gnu/k.s`, `gnu/d.s`), AArch64 and x86-64.

use std::path::{Path, PathBuf};

use mortar_elf::consts::SHF_ALLOC;
use mortar_elf::{FileHeader, SectionTable};
use mortar_plumb::{Ctx, Files, Finding, Format, Input, Map, Report, Rule, Via, map, verify_with};
use mortar_testutil::{Scratch, run, tool};

const ARCHES: [&str; 2] = ["aarch64", "x86_64"];

struct Dir(PathBuf);

impl Files for Dir {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(path)).ok()
    }
}

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/macro_import").join(rel)
}

/// Verifies `out` against the map `map_name` of directory `dir` (its
/// format `format`) and the files it names, read from `dir`.
fn verify_in(dir: &Path, out: &[u8], map_name: &str, format: Format) -> Report {
    let text = std::fs::read_to_string(dir.join(map_name)).unwrap();
    let m: Map = map::parse(format, &text)
        .unwrap_or_else(|e| panic!("{}: line {}: {}", dir.display(), e.line, e.message));
    let bytes: Vec<(String, Vec<u8>)> = m
        .paths()
        .into_iter()
        .map(|p| (p.to_string(), std::fs::read(dir.join(p)).unwrap()))
        .collect();
    let inputs: Vec<Input<'_>> = bytes.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
    let files = Dir(dir.to_path_buf());
    verify_with(out, &inputs, &m, &Ctx { line: None, files: &files })
}

/// The findings that are not a recorded anomaly of the map's linker
/// (§10.2).
fn unexpected<'r>(format: Format, out: &[u8], r: &'r Report) -> Vec<&'r Finding> {
    r.findings.iter().filter(|f| mortar_plumb::anomaly::classify(format, out, f).is_none()).collect()
}

fn put(out: &mut [u8], at: usize, w: usize, v: u64) {
    out[at..at + w].copy_from_slice(&v.to_le_bytes()[..w]);
}

fn get(out: &[u8], at: usize, w: usize) -> u64 {
    let mut b = [0u8; 8];
    b[..w].copy_from_slice(&out[at..at + w]);
    u64::from_le_bytes(b)
}

/// A section of an ELF file: (file offset, address, size), by name and
/// whether it is allocated.
fn section(elf: &[u8], name: &str, alloc: bool) -> Option<(usize, u64, usize)> {
    let h = FileHeader::parse(elf).unwrap();
    SectionTable::parse(elf, &h).unwrap().iter().find_map(|s| {
        let sh = s.header();
        (s.name().is_ok_and(|n| n == name.as_bytes()) && (sh.sh_flags & SHF_ALLOC != 0) == alloc)
            .then_some((sh.sh_offset as usize, sh.sh_addr, sh.sh_size as usize))
    })
}

/// The offset in section `name` (non-alloc) of `needle`, which must be
/// there once.
fn find_in(elf: &[u8], name: &str, needle: &[u8]) -> usize {
    let (off, _, size) = section(elf, name, false).unwrap_or_else(|| panic!("no {name}"));
    let b = &elf[off..off + size];
    let at: Vec<usize> =
        b.windows(needle.len()).enumerate().filter(|(_, w)| *w == needle).map(|(i, _)| i).collect();
    assert_eq!(at.len(), 1, "{name}: {:?} {at:?}", String::from_utf8_lossy(needle));
    at[0]
}

// ---- GCC -g3: mortar's output with the fields written each way ----

struct Gcc {
    label: String,
    dir: PathBuf,
    out: Vec<u8>,
    /// Per import of a dropped copy: file offset, width, the kept copy's
    /// offset (+ addend).
    imports: Vec<(usize, usize, u64)>,
}

fn gcc_fixtures() -> Vec<Gcc> {
    let mut v = Vec::new();
    for arch in ARCHES {
        for dw in [4, 5] {
            let dir = fixture(&format!("gcc/{arch}/dw{dw}"));
            let text = std::fs::read_to_string(dir.join("imports.txt")).unwrap();
            let imports = text
                .lines()
                .map(|l| {
                    let p: Vec<&str> = l.split_whitespace().collect();
                    let hex = |s: &str| u64::from_str_radix(s.trim_start_matches("0x"), 16).unwrap();
                    (hex(p[0]) as usize, p[1].parse().unwrap(), hex(p[2]))
                })
                .collect::<Vec<_>>();
            assert!(imports.len() >= 40, "{arch} dw{dw}: {} imports", imports.len());
            let out = std::fs::read(dir.join("out")).unwrap();
            v.push(Gcc { label: format!("{arch} dw{dw}"), dir, out, imports });
        }
    }
    v
}

impl Gcc {
    fn verify(&self, out: &[u8]) -> Report {
        verify_in(&self.dir, out, "out.map", Format::Mortar)
    }

    fn redirected(&self) -> Vec<u8> {
        let mut o = self.out.clone();
        for &(at, w, v) in &self.imports {
            put(&mut o, at, w, v);
        }
        o
    }
}

// I-47a's condition holds for every import GCC writes (WP-249 §1): the
// fields filled against the kept copy are clean, each one verified as
// `kept-copy`.
#[test]
fn i47a_gcc_g3_redirect_is_clean() {
    for g in gcc_fixtures() {
        let r = g.verify(&g.redirected());
        assert!(r.findings.is_empty(), "{}: {:?}\n{}", g.label, r.findings.first(), r.summary());
        assert_eq!(
            r.coverage.verified_of(Via::KeptCopy),
            g.imports.len() as u64,
            "{}\n{}",
            g.label,
            r.summary()
        );
    }
}

// The output as mortar wrote it before I-47a (all ones), and the same
// fields 0 (lld's convention): one R1 per import, expecting the kept
// copy's offset.
#[test]
fn i47a_gcc_g3_tombstone_and_zero_are_one_finding_per_import() {
    for g in gcc_fixtures() {
        let mut zero = g.out.clone();
        for &(at, w, _) in &g.imports {
            put(&mut zero, at, w, 0);
        }
        for (what, out, found) in [("ones", g.out.clone(), 0xffff_ffff), ("zero", zero, 0)] {
            let r = g.verify(&out);
            assert_eq!(r.findings.len(), g.imports.len(), "{} {what}\n{}", g.label, r.summary());
            let mut want: Vec<u64> = g.imports.iter().map(|i| i.2).collect();
            let mut got: Vec<u64> = r
                .findings
                .iter()
                .map(|f| {
                    assert_eq!(f.rule, Rule::Field, "{} {what}: {f:?}", g.label);
                    assert!(f.message.contains("I-47a"), "{} {what}: {f:?}", g.label);
                    assert_eq!(f.found, Some(found), "{} {what}: {f:?}", g.label);
                    f.expected.unwrap()
                })
                .collect();
            want.sort_unstable();
            got.sort_unstable();
            assert_eq!(got, want, "{} {what}", g.label);
        }
    }
}

// One redirected import off by 4 (another unit's offset would do the
// same): exactly that one is R1.
#[test]
fn i47a_gcc_g3_a_redirect_off_by_4_is_found() {
    for g in gcc_fixtures() {
        for k in [0, g.imports.len() / 2, g.imports.len() - 1] {
            let mut o = g.redirected();
            let (at, w, v) = g.imports[k];
            put(&mut o, at, w, v + 4);
            let r = g.verify(&o);
            assert_eq!(r.findings.len(), 1, "{} import {k}\n{}", g.label, r.summary());
            let f = &r.findings[0];
            assert_eq!((f.rule, f.expected, f.found), (Rule::Field, Some(v), Some(v + 4)), "{f:?}");
        }
    }
}

// ---- the branches of the condition (hand/k.s, hand/d.s) ----

/// The words of `hand/d.s` (offset after the marker in the output's
/// `.debug_macro`, width), each with what it is.
const HAND_DUE: [(usize, usize, u64); 4] = [
    (8, 4, 0),   // R
    (12, 4, 4),  // R + 4
    (16, 4, 16), // r_local, 16 bytes into R's member
    (20, 8, 0),  // R, a 64-bit word
];

/// The fall-back words: offset, the branch, the tag the patched redirect
/// names (in the section, alloc or not, whose name follows), the
/// fall-back's reason as the finding names it.
const HAND_FALLBACK: [(usize, &str, &[u8], &str, bool, &str); 5] = [
    (28, "N", b"Ntag", ".debug_types", false, "has no member of its name"),
    (32, "Z", b"Ztag", ".debug_macro", false, "has another size"),
    (36, "T", b"T1tag", ".debug_macro", false, "more than one member of its name and size"),
    (40, "A", b"Atag", ".wp250a", true, "is SHF_ALLOC"),
    (44, "L", b"", "", false, "is not in the output"),
];

struct Hand {
    dir: PathBuf,
    out: Vec<u8>,
    /// File offsets of the words of `.debug_macro` and `.debug_info`.
    macro_ref: usize,
    info_ref: usize,
    /// R's kept member: its offset in `.debug_macro`.
    r: u64,
}

fn hand(arch: &'static str) -> Hand {
    let dir = fixture(&format!("hand/{arch}"));
    let out = std::fs::read(dir.join("out")).unwrap();
    let file_of = |name: &str| section(&out, name, false).unwrap().0;
    let macro_ref = file_of(".debug_macro") + find_in(&out, ".debug_macro", b"WP250REF");
    let info_ref = file_of(".debug_info") + find_in(&out, ".debug_info", b"WP250REF");
    let r = find_in(&out, ".debug_macro", b"Rtag") as u64;
    Hand { dir, out, macro_ref, info_ref, r }
}

impl Hand {
    fn verify(&self, out: &[u8]) -> Report {
        verify_in(&self.dir, out, "out.map", Format::Mortar)
    }

    /// The output with every word I-47a redirects written so.
    fn redirected(&self) -> Vec<u8> {
        let mut o = self.out.clone();
        for (off, w, add) in HAND_DUE {
            put(&mut o, self.macro_ref + off, w, self.r + add);
        }
        put(&mut o, self.info_ref + 8, 4, self.r);
        o
    }
}

// As linked: the four `.debug_macro` words and the `.debug_info` word of
// R hold the tombstone where I-47a redirects: five R1; the five fall-back
// words hold it rightly.
#[test]
fn i47a_branches_as_linked() {
    for arch in ARCHES {
        let h = hand(arch);
        let r = h.verify(&h.out);
        let mut want: Vec<u64> = HAND_DUE.iter().map(|d| h.r + d.2).collect();
        want.push(h.r);
        want.sort_unstable();
        let mut got: Vec<u64> = r
            .findings
            .iter()
            .map(|f| {
                assert_eq!(f.rule, Rule::Field, "{arch}: {f:?}");
                assert!(f.message.ends_with("; it holds a tombstone"), "{arch}: {f:?}");
                f.expected.unwrap()
            })
            .collect();
        got.sort_unstable();
        assert_eq!(got, want, "{arch}\n{}", r.summary());
        assert_eq!(r.coverage.verified_of(Via::Tombstone), 5, "{arch}\n{}", r.summary());
    }
}

// Redirected: clean, five `kept-copy` and five tombstones. Then each
// word moved by 1 is one R1 (every word is judged exactly).
#[test]
fn i47a_branches_redirected_and_each_word_judged() {
    for arch in ARCHES {
        let h = hand(arch);
        let o = h.redirected();
        let r = h.verify(&o);
        assert!(r.findings.is_empty(), "{arch}: {:?}\n{}", r.findings.first(), r.summary());
        assert_eq!(r.coverage.verified_of(Via::KeptCopy), 5, "{arch}\n{}", r.summary());
        assert_eq!(r.coverage.verified_of(Via::Tombstone), 5, "{arch}\n{}", r.summary());
        let words = HAND_DUE
            .iter()
            .map(|d| (h.macro_ref + d.0, d.1))
            .chain(HAND_FALLBACK.iter().map(|f| (h.macro_ref + f.0, 4)))
            .chain([(h.info_ref + 8, 4)]);
        for (at, w) in words {
            let mut m = o.clone();
            put(&mut m, at, w, get(&o, at, w).wrapping_add(1));
            let r = h.verify(&m);
            assert_eq!(r.findings.len(), 1, "{arch}: word at {at:#x}\n{}", r.summary());
            assert_eq!(r.findings[0].rule, Rule::Field, "{arch}: {:?}", r.findings[0]);
        }
    }
}

// A redirect where the condition fails is R1, one per branch: no member
// of the name (N), another size (Z), two members of the name and size
// (T), the member SHF_ALLOC (A), the member not in the output (L, its
// group collected by --gc-sections). The finding names the branch.
#[test]
fn i47a_a_redirect_in_each_fallback_branch_is_found() {
    for arch in ARCHES {
        let h = hand(arch);
        let base = h.redirected();
        for (off, branch, tag, sec, alloc, why) in HAND_FALLBACK {
            // The value a redirect would give: the would-be member's place
            // (its tag), or any offset when it has none (L).
            let v = if tag.is_empty() {
                0x10
            } else if alloc {
                let (o, addr, size) = section(&h.out, sec, true).unwrap();
                let b = &h.out[o..o + size];
                addr + b.windows(tag.len()).position(|w| w == tag).unwrap() as u64
            } else {
                find_in(&h.out, sec, tag) as u64
            };
            let mut m = base.clone();
            put(&mut m, h.macro_ref + off, 4, v);
            let r = h.verify(&m);
            assert_eq!(r.findings.len(), 1, "{arch} {branch}\n{}", r.summary());
            let f = &r.findings[0];
            assert_eq!(f.rule, Rule::Field, "{arch} {branch}: {f:?}");
            assert_eq!((f.expected, f.found), (Some(0xffff_ffff), Some(v)), "{arch} {branch}");
            assert!(
                f.message.contains("I-47a keeps the tombstone here") && f.message.contains(why),
                "{arch} {branch}: {}",
                f.message
            );
        }
    }
}

// ---- the reference linkers ----

struct Tools {
    cc: PathBuf,
    cxx: PathBuf,
    asm: PathBuf,
    ld: [PathBuf; 3],
    em: &'static str,
}

fn tools(arch: &str) -> Option<Tools> {
    let t = |n: &str| tool(&format!("{arch}-linux-gnu-{n}"));
    Some(Tools {
        cc: t("gcc")?,
        cxx: t("g++")?,
        asm: t("as")?,
        ld: [t("ld.bfd")?, tool("ld.lld-22")?, tool("mold")?],
        em: if arch == "aarch64" { "aarch64linux" } else { "elf_x86_64" },
    })
}

fn tools_or_skip(arch: &str) -> Option<Tools> {
    let t = tools(arch);
    assert!(
        t.is_some() || !mortar_testutil::tools_required(),
        "{arch}: the GNU tools, ld.lld-22 and mold are required"
    );
    t
}

const LINKERS: [(&str, Format); 3] = [("gnu", Format::Gnu), ("lld", Format::Lld), ("mold", Format::Mold)];

/// Links `objs` in `dir` with each reference linker (`-static -e entry`,
/// a map), each output named after its linker.
fn link_all(t: &Tools, dir: &Path, objs: &[&str], entry: &str) {
    for (k, (name, _)) in LINKERS.iter().enumerate() {
        let mut args: Vec<String> =
            vec!["-m".into(), t.em.into(), "-static".into(), "-e".into(), entry.into()];
        args.extend(["-Map".into(), dir.join(format!("{name}.map")).display().to_string()]);
        args.extend(objs.iter().map(|o| dir.join(o).display().to_string()));
        args.extend(["-o".into(), dir.join(name).display().to_string()]);
        let o = run(&t.ld[k], &args);
        assert!(o.success(), "{name}: {}", o.stderr_text());
    }
}

/// The `DW_MACRO_import` fields of each compile unit of a `.debug_macro`
/// (the units with a `DW_MACRO_start_file`), in order: (offset, width).
fn cu_imports(b: &[u8]) -> Vec<Vec<(usize, usize)>> {
    fn uleb(b: &[u8], p: &mut usize) {
        while b[*p] & 0x80 != 0 {
            *p += 1;
        }
        *p += 1;
    }
    let mut out = Vec::new();
    let mut p = 0;
    while p < b.len() {
        let flags = b[p + 2];
        p += 3;
        let w = if flags & 1 != 0 { 8 } else { 4 };
        if flags & 2 != 0 {
            p += w;
        }
        assert_eq!(flags & 4, 0, "an opcode table");
        let (mut cu, mut imports) = (false, Vec::new());
        loop {
            let op = b[p];
            p += 1;
            match op {
                0 => break,
                1 | 2 => {
                    uleb(b, &mut p);
                    p += b[p..].iter().position(|&c| c == 0).unwrap() + 1;
                }
                3 => {
                    cu = true;
                    uleb(b, &mut p);
                    uleb(b, &mut p);
                }
                4 => {}
                5 | 6 | 8 | 9 => {
                    uleb(b, &mut p);
                    p += w;
                }
                7 | 10 => {
                    imports.push((p, w));
                    p += w;
                }
                11 | 12 => {
                    uleb(b, &mut p);
                    uleb(b, &mut p);
                }
                _ => panic!("macro op {op:#x} at {:#x}", p - 1),
            }
        }
        if cu {
            out.push(imports);
        }
    }
    out
}

// GCC 14 `-g3` C and C++ (a template and an inline function: code COMDAT
// groups too), DWARF 4 and 5, linked by GNU ld, lld 22 and mold: each
// judged by its own column with no finding but a recorded anomaly (GNU
// ld: its kept section; lld: 0; mold: every `wm4.*` copy kept, each
// import naming its unit's own copy). Then the first and last import of
// b's and c's units, moved by 1, are each found.
#[test]
fn reference_columns_on_gcc_g3() {
    for arch in ARCHES {
        let Some(t) = tools_or_skip(arch) else { return };
        for (lang, ext) in [("c", "c"), ("cxx", "cc")] {
            for dw in [4, 5] {
                let s = Scratch::new(&format!("plumb-macro-{arch}-{lang}-{dw}"));
                let dir = s.path();
                for f in ["a", "b", "c"] {
                    let src = fixture(&format!("src/{f}.{ext}"));
                    let cc = if lang == "c" { &t.cc } else { &t.cxx };
                    let mut args = vec!["-O0", "-g3", if dw == 4 { "-gdwarf-4" } else { "-gdwarf-5" }];
                    if lang == "cxx" {
                        args.extend(["-fno-exceptions", "-fno-rtti"]);
                    }
                    let mut a: Vec<String> = args.iter().map(ToString::to_string).collect();
                    a.extend([
                        "-c".into(),
                        src.display().to_string(),
                        "-o".into(),
                        dir.join(format!("{f}.o")).display().to_string(),
                    ]);
                    let o = run(cc, &a);
                    assert!(o.success(), "{arch} {lang} {f}: {}", o.stderr_text());
                }
                link_all(&t, dir, &["a.o", "b.o", "c.o"], "main");
                for (name, format) in LINKERS {
                    let label = format!("{arch} {lang} dw{dw} {name}");
                    let out = std::fs::read(dir.join(name)).unwrap();
                    let r = verify_in(dir, &out, &format!("{name}.map"), format);
                    let bad = unexpected(format, &out, &r);
                    assert!(bad.is_empty(), "{label}: {:?}\n{}", bad.first(), r.summary());
                    if format == Format::Gnu {
                        assert!(r.coverage.verified_of(Via::KeptCopy) >= 40, "{label}\n{}", r.summary());
                    }
                    let (off, _, size) = section(&out, ".debug_macro", false).unwrap();
                    let units = cu_imports(&out[off..off + size]);
                    assert_eq!(units.len(), 3, "{label}");
                    let picks = [units[1][0], *units[1].last().unwrap(), units[2][0]];
                    for (at, w) in picks {
                        let mut m = out.clone();
                        let at = off + at;
                        put(&mut m, at, w, get(&out, at, w).wrapping_add(1));
                        let r = verify_in(dir, &m, &format!("{name}.map"), format);
                        let bad = unexpected(format, &m, &r);
                        assert_eq!(bad.len(), 1, "{label}: import at {at:#x}\n{}", r.summary());
                        assert_eq!(bad[0].rule, Rule::Field, "{label}: {:?}", bad[0]);
                    }
                }
            }
        }
    }
}

/// The words of `gnu/d.s` after its marker: (offset, width), C1 to C14.
const GNU_WORDS: [(usize, usize); 13] = [
    (8, 4),
    (12, 4),
    (16, 4),
    (20, 4),
    (24, 4),
    (28, 4),
    (32, 4),
    (36, 4),
    (40, 8),
    (48, 4),
    (52, 4),
    (56, 4),
    (60, 4),
];

// WP-249's probe of GNU ld's condition (`gnu/k.s`, `gnu/d.s`: a kept
// member with matching symbols and the size, of any name; with and
// without k.o's references to its own copies): every reference linker's
// link is clean by its column, GNU ld's redirect to a `.debug_types`
// member (C4) included, and each word moved by 1 is one R1.
#[test]
fn reference_columns_on_gnu_probe() {
    for arch in ARCHES {
        let Some(t) = tools_or_skip(arch) else { return };
        for kref in [false, true] {
            let s = Scratch::new(&format!("plumb-macro-gnu-{arch}-{kref}"));
            let dir = s.path();
            for (f, defs) in [("k", kref), ("d", false)] {
                let mut a: Vec<String> = Vec::new();
                if defs {
                    a.extend(["--defsym".into(), "KREF=1".into()]);
                }
                a.extend([
                    fixture(&format!("gnu/{f}.s")).display().to_string(),
                    "-o".into(),
                    dir.join(format!("{f}.o")).display().to_string(),
                ]);
                let o = run(&t.asm, &a);
                assert!(o.success(), "{arch} {f}.s: {}", o.stderr_text());
            }
            link_all(&t, dir, &["k.o", "d.o"], "_start");
            for (name, format) in LINKERS {
                let label = format!("{arch} kref={kref} {name}");
                let out = std::fs::read(dir.join(name)).unwrap();
                let r = verify_in(dir, &out, &format!("{name}.map"), format);
                let bad = unexpected(format, &out, &r);
                assert!(bad.is_empty(), "{label}: {:?}\n{}", bad.first(), r.summary());
                let base = section(&out, ".debug_macro", false).unwrap().0
                    + find_in(&out, ".debug_macro", b"WP249REF");
                for (off, w) in GNU_WORDS {
                    let at = base + off;
                    let mut m = out.clone();
                    put(&mut m, at, w, get(&out, at, w).wrapping_add(1));
                    let r = verify_in(dir, &m, &format!("{name}.map"), format);
                    let bad = unexpected(format, &m, &r);
                    assert_eq!(bad.len(), 1, "{label}: word +{off}\n{}", r.summary());
                    assert_eq!(bad[0].rule, Rule::Field, "{label}: {:?}", bad[0]);
                }
            }
        }
    }
}

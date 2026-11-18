//! The symbol tables of an incremental-mode output and the decision
//! records of its map (WP-222, V-1; `spec/plumb.md` §3.2, §6.8, §6.9;
//! `spec/incr.md` IN-49a; `spec/image.md` I-78a).
//!
//! The fixtures (`tests/symfill/`, `gen.sh`) are mortar's static links of
//! one program on AArch64 and x86-64, each made twice from the same
//! objects: `x-<arch>` in incremental mode (`--incremental=full`), `f-<arch>`
//! plain. Today's mortar writes neither fillers, nor the symbol tables'
//! file slack, nor decision records, so `tests/symfill/edit.py` (bytes and
//! map lines only, no linker) gives `x` IN-49a's shape and §3.2's
//! decision records, and makes one mutation per sentence of the rules.
//! Each shape is ACCEPTED with its counts; each mutation is REJECTED by
//! the rule its sentence names.
#![allow(clippy::many_single_char_names)]

use std::path::{Path, PathBuf};

use mortar_plumb::equiv::Equiv;
use mortar_plumb::{Ctx, Files, Format, Input, Map, Report};
use mortar_testutil::{Scratch, run_with};

struct Dir<'a>(&'a Path);

impl Files for Dir<'_> {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(path)).ok()
    }
}

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/symfill")
}

const ARCHS: [&str; 2] = ["a64", "x64"];

/// The edited link: its output and map.
struct Link {
    out: Vec<u8>,
    map: Map,
}

/// `edit.py` on `<base>-<arch>` with `ops`, into a scratch directory;
/// `None` (the test skips) when python3 is missing and tools are optional.
fn edit<S: AsRef<std::ffi::OsStr> + std::fmt::Debug>(
    arch: &str,
    base: &str,
    ops: &[S],
) -> Option<Link> {
    let py = mortar_testutil::tool("python3")?;
    let s = Scratch::new("symfill");
    let d = dir();
    let (o, m) = (s.join("out"), s.join("out.map"));
    let mut c = std::process::Command::new(py);
    c.arg(d.join("edit.py"))
        .arg(d.join(format!("{base}-{arch}")))
        .arg(d.join(format!("{base}-{arch}.map")))
        .arg(&o)
        .arg(&m)
        .args(ops);
    let r = run_with(&mut c, b"", mortar_testutil::DEFAULT_TIMEOUT);
    assert!(r.success(), "edit.py {base}-{arch} {ops:?}: {}", r.stderr_text());
    let out = std::fs::read(&o).unwrap();
    let text = std::fs::read_to_string(&m).unwrap();
    let map = mortar_plumb::map::parse(Format::Mortar, &text)
        .unwrap_or_else(|e| panic!("{base}-{arch} {ops:?}: map line {}: {}", e.line, e.message));
    Some(Link { out, map })
}

fn inputs(maps: &[&Map]) -> Vec<(String, Vec<u8>)> {
    let d = dir();
    let mut v: Vec<(String, Vec<u8>)> = Vec::new();
    for m in maps {
        for p in m.paths() {
            if !v.iter().any(|(q, _)| q == p)
                && let Ok(b) = std::fs::read(d.join(p))
            {
                v.push((p.to_string(), b));
            }
        }
    }
    v
}

/// One link verified alone.
fn single(l: &Link) -> Report {
    let files = inputs(&[&l.map]);
    let inputs: Vec<Input<'_>> = files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
    let d = dir();
    mortar_plumb::verify_with(&l.out, &inputs, &l.map, &Ctx { line: None, files: &Dir(&d) })
}

fn equiv(x: &Link, f: &Link) -> Equiv {
    let files = inputs(&[&x.map, &f.map]);
    let inputs: Vec<Input<'_>> = files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
    mortar_plumb::equiv::check(&x.out, &x.map, &f.out, &f.map, &inputs)
}

/// `rule message` of each finding.
fn lines(findings: &[mortar_plumb::Finding]) -> Vec<String> {
    findings.iter().map(|f| format!("{} {}", f.rule.id(), f.message)).collect()
}

/// IN-49a's shape plus the decision records.
const SHAPE: &[&str] = &["fill", "slack", "decisions"];

fn with(extra: &[&str]) -> Vec<String> {
    SHAPE.iter().chain(extra).map(|s| (*s).to_string()).collect()
}

/// The single run's findings are exactly one, of `rule`, containing
/// `needle`.
fn rejected_alone<S: AsRef<std::ffi::OsStr> + std::fmt::Debug>(
    arch: &str,
    ops: &[S],
    rule: &str,
    needle: &str,
) {
    let Some(x) = edit(arch, "x", ops) else { return };
    let got = lines(&single(&x).findings);
    assert!(
        !got.is_empty()
            && got.iter().all(|l| l.starts_with(&format!("{rule} ")) && l.contains(needle)),
        "{arch} {ops:?}: want only {rule} '{needle}', got {got:#?}"
    );
}

/// `equiv(x, f)`'s P-E findings are exactly one, of `rule`, containing
/// `needle`; the sides' own findings are returned.
fn rejected_by_equiv(x: &Link, f: &Link, what: &str, rule: &str, needle: &str) -> Equiv {
    let e = equiv(x, f);
    let got = lines(&e.findings);
    assert!(
        got.len() == 1 && got[0].starts_with(&format!("{rule} ")) && got[0].contains(needle),
        "{what}: want one {rule} '{needle}', got {got:#?}"
    );
    e
}

// §6.9, §6.8, §3.2 (WP-222): IN-49a's shape with decision records is
// accepted alone and against the plain link, with its counts; with a
// `.symtab_shndx` of zeros on both sides too.
#[test]
fn in49a_shape_is_accepted() {
    for arch in ARCHS {
        let Some(x) = edit(arch, "x", SHAPE) else { return };
        let r = single(&x);
        assert!(r.findings.is_empty(), "{arch}: {:#?}", lines(&r.findings));
        assert_eq!(
            r.coverage.decisions.as_deref(),
            Some(
                "decision records: 10 read; region, symblock judged (place only); capacity, order, slot, redirect, frag not judged"
            ),
            "{arch}"
        );
        assert!(r.summary().contains("decision records: 10 read"), "{arch}");
        let f = edit::<&str>(arch, "f", &[]).unwrap();
        let e = equiv(&x, &f);
        assert!(
            e.clean(),
            "{arch}: {:#?} {:#?} {:#?}",
            lines(&e.findings),
            lines(&e.x.findings),
            lines(&e.f.findings)
        );
        assert_eq!(e.x.coverage.fillers, Some(70), "{arch}: 2 per block and a tail of 64");
        assert_eq!(e.f.coverage.fillers, None, "{arch}: a plain side counts none");
        // The plain map states no common's <file>: read, not judged, named.
        assert!(
            e.summary().contains("equiv not judged: common <file> of 1 common(s)"),
            "{arch}: {}",
            e.summary()
        );
        // `.symtab_shndx` of zeros on both sides.
        let xs = edit(arch, "x", &with(&["shndx"])).unwrap();
        let fs = edit(arch, "f", &["shndx"]).unwrap();
        let e = equiv(&xs, &fs);
        assert!(e.clean(), "{arch} shndx: {:#?}", lines(&e.findings));
        assert_eq!(e.x.coverage.fillers, Some(70));
        // A plain map's coverage lines gain nothing.
        let r = single(&f);
        assert!(r.findings.is_empty(), "{arch}");
        assert!(r.coverage.decisions.is_none() && r.coverage.fillers.is_none());
        assert!(!r.summary().contains("decision records") && !r.summary().contains("fillers"));
    }
}

// §6.9 (WP-222, L10's L-1): the unedited pair, whose debug sections differ
// in offsets (IN-51's slack in the incremental-mode link), is accepted:
// a non-allocated local corresponds by its `nplace` unit; a `$d` moved
// into another unit's range is P-E3 naming its place.
#[test]
fn p_e3_nonalloc_locals_by_identity() {
    for arch in ARCHS {
        let Some(x) = edit::<&str>(arch, "x", &[]) else { return };
        let f = edit::<&str>(arch, "f", &[]).unwrap();
        let e = equiv(&x, &f);
        assert!(e.clean(), "{arch}: {:#?}", lines(&e.findings));
        assert_eq!(e.x.coverage.fillers, Some(0), "{arch}: an incremental side counts 0");
    }
    // AArch64's mapping symbols mark the debug sections.
    let Some(x) = edit("a64", "x", &["l1"]) else { return };
    let f = edit::<&str>("a64", "f", &[]).unwrap();
    let e = rejected_by_equiv(&x, &f, "l1", "P-E3", "the local '$d' at .debug_info+0x0");
    // X alone: Y6 (WP-246) finds the moved `$d` too, as an input local
    // not in .symtab and an output local that is no kept input's.
    assert!(e.f.findings.is_empty(), "{:#?}", lines(&e.f.findings));
    let xl = lines(&e.x.findings);
    assert!(
        !xl.is_empty() && xl.iter().all(|l| l.starts_with("Y6 ") && l.contains("'$d'")),
        "{xl:#?}"
    );
}

// §6.9 (WP-222): a near-filler, one field away from the form before
// sh_info, is P-E3 naming the field, and nothing else (V-1's
// `near_filler_reported`, spec/incr.md IN-49a).
#[test]
fn near_filler_reported() {
    for arch in ARCHS {
        let Some(f) = edit::<&str>(arch, "f", &[]) else { return };
        for field in ["st_name", "st_info", "st_other", "st_shndx", "st_value", "st_size"] {
            let x = edit(arch, "x", &with(&[&format!("near={field}")])).unwrap();
            let e =
                rejected_by_equiv(&x, &f, field, "P-E3", &format!("a filler but for its {field}"));
            // A near-filler of st_info 0 is an unnamed NOTYPE local: X
            // alone finds it by Y5 and Y6 (WP-246), naming the entry.
            let xl = lines(&e.x.findings);
            if field == "st_info" {
                assert!(
                    xl.len() == 2
                        && xl.iter().any(|l| l.starts_with("Y5 ") && l.contains("has no name"))
                        && xl
                            .iter()
                            .any(|l| l.starts_with("Y6 ") && l.contains("('', st_info 0x0")),
                    "{arch} {field}: {xl:#?}"
                );
            } else {
                assert!(xl.is_empty(), "{arch} {field}: {xl:#?}");
            }
            assert_eq!(e.x.coverage.fillers, Some(69), "{arch} {field}");
        }
        let fs = edit(arch, "f", &["shndx"]).unwrap();
        let x = edit(arch, "x", &with(&["shndx", "near=.symtab_shndx"])).unwrap();
        let e = rejected_by_equiv(&x, &fs, "shndx", "P-E3", "a filler but for its .symtab_shndx");
        assert!(e.x.findings.is_empty());
    }
}

// §6.9 (WP-222): a filler after sh_info, and a filler in a side whose map
// does not say `incremental`, are P-E3.
#[test]
fn p_e3_misplaced_fillers() {
    for arch in ARCHS {
        let Some(f) = edit::<&str>(arch, "f", &[]) else { return };
        let x = edit(arch, "x", &with(&["filler-after-info"])).unwrap();
        let e = rejected_by_equiv(&x, &f, "after", "P-E3", "a filler at or after sh_info");
        // The side's own Y2 sees a local after the first global (§6.5).
        assert_eq!(lines(&e.x.findings).len(), 1, "{arch}: {:#?}", lines(&e.x.findings));
        assert!(lines(&e.x.findings)[0].starts_with("Y2 "));
        let x = edit(arch, "x", SHAPE).unwrap();
        let ff = edit(arch, "f", &["fill"]).unwrap();
        let e = equiv(&x, &ff);
        let got = lines(&e.findings);
        assert!(
            got.len() == 70
                && got.iter().all(|l| l.starts_with("P-E3 F: ")
                    && l.contains("a filler, in a link whose map does not say incremental")),
            "{arch}: {got:#?}"
        );
        assert!(e.x.findings.is_empty() && e.f.findings.is_empty());
    }
}

// §6.8 (WP-222): the file slack, judged alone (P-I2, P-I5).
#[test]
fn p_i2_p_i5_file_slack() {
    for arch in ARCHS {
        rejected_alone(arch, &with(&["slack-long"]), "P-I2", "runs past the next section");
        rejected_alone(arch, &with(&["slack-nonzero"]), "P-I5", "is not zero");
        rejected_alone(arch, &with(&["slack-off"]), "P-I2", "outside '.symtab'");
        rejected_alone(
            arch,
            &with(&["hole-nonalloc"]),
            "P-I2",
            "a hole is in an allocated section only",
        );
    }
}

// §6.8 (WP-222): `.strtab`'s dead names (P-I3, P-I6; P-I2 without
// `incremental`).
#[test]
fn p_i3_p_i6_dead_strtab_names() {
    for arch in ARCHS {
        rejected_alone(arch, &with(&["strtab-named"]), "P-I3", "('afun') names .strtab+");
        rejected_alone(arch, &with(&["strtab-nonul"]), "P-I6", "does not end with a string's NUL");
        rejected_alone(arch, &["slack", "plain"], "P-I2", "in a section with no merged strings");
    }
}

// §3.2, §6.8 (WP-222): the decision records judged (P-I2).
#[test]
fn p_i2_decision_records() {
    for arch in ARCHS {
        rejected_alone(arch, &with(&["region-out"]), "P-I2", "is not inside '.text'");
        rejected_alone(arch, &with(&["region-none"]), "P-I2", "names 0 output sections");
        rejected_alone(arch, &with(&["region-overlap"]), "P-I2", "meets the region of line");
        rejected_alone(arch, &with(&["symblock-past"]), "P-I2", "runs past .symtab's local area");
    }
}

// §6.9 P-E2 (WP-222): a common's `<file>`, stated by both maps, names
// one file by identity; another file is P-E2.
#[test]
fn p_e2_common_file() {
    for arch in ARCHS {
        let Some(x) = edit(arch, "x", SHAPE) else { return };
        let e = equiv(&x, &x);
        assert!(
            e.clean() && e.not_judged.is_empty(),
            "{arch}: {:#?} {:?}",
            lines(&e.findings),
            e.not_judged
        );
        let other = edit(arch, "x", &with(&["common-file=0"])).unwrap();
        rejected_by_equiv(
            &other,
            &x,
            "common-file",
            "P-E2",
            "the common 'shared_c' is allocated from",
        );
    }
}

/// ELF64 little endian: the section header of `name` (its file offset).
fn shdr_at(b: &[u8], name: &str) -> usize {
    let u16_at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]) as usize;
    let u64_at = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap()) as usize;
    let u32_at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize;
    let (shoff, shentsize, shnum, shstrndx) =
        (u64_at(0x28), u16_at(0x3a), u16_at(0x3c), u16_at(0x3e));
    let strs = u64_at(shoff + shstrndx * shentsize + 0x18);
    (0..shnum)
        .map(|i| shoff + i * shentsize)
        .find(|&h| {
            let n = strs + u32_at(h);
            b[n..].starts_with(name.as_bytes()) && b[n + name.len()] == 0
        })
        .unwrap_or_else(|| panic!("no section {name}"))
}

/// L-64's `.pad.relro` of `k2a` (`tests/inset_stages/`, mortar's dynamic
/// link): `x` its map with `incremental`, `f` the same link with the pad
/// emptied (size 0, `PT_GNU_RELRO` shortened to match, no `chunk` record:
/// the plain link of L10's L-2, whose RELRO end needed no pad).
fn pad_pair(arch: &str) -> (Link, Link, PathBuf) {
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inset_stages").join(arch);
    let out = std::fs::read(d.join("k2a")).unwrap();
    let text = std::fs::read_to_string(d.join("k2a.map")).unwrap();
    let parse = |t: &str| mortar_plumb::map::parse(Format::Mortar, t).unwrap();
    // Stage 3 reads the line's files from the working directory: the
    // map's paths (`file` and `arg` records) made absolute.
    let abs = |w: &str| {
        let p = d.join(w);
        if p.is_file() { p.to_str().unwrap().to_string() } else { w.to_string() }
    };
    let text: String = text
        .lines()
        .map(|l| {
            let mut w: Vec<String> = l.split(' ').map(str::to_string).collect();
            match w[0].as_str() {
                "file" => w[2] = abs(&w[2]),
                "arg" => w[1] = abs(&w[1]),
                _ => {}
            }
            w.join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let xt = text.replacen("\noutput ", "\nincremental\noutput ", 1);
    let x = Link { out: out.clone(), map: parse(&xt) };
    let pad = shdr_at(&out, ".pad.relro");
    let size = u64::from_le_bytes(out[pad + 0x20..pad + 0x28].try_into().unwrap());
    assert!(size > 0, "{arch}: k2a has a pad");
    let mut fo = out;
    fo[pad + 0x20..pad + 0x28].copy_from_slice(&0u64.to_le_bytes());
    let phoff = u64::from_le_bytes(fo[0x20..0x28].try_into().unwrap()) as usize;
    let phnum = u16::from_le_bytes([fo[0x38], fo[0x39]]) as usize;
    let addr = u64::from_le_bytes(fo[pad + 0x10..pad + 0x18].try_into().unwrap());
    // Every segment that ends at the pad's end (`PT_GNU_RELRO` and its
    // load segment) ends at its start.
    for i in 0..phnum {
        let p = phoff + i * 56;
        let v = u64::from_le_bytes(fo[p + 0x10..p + 0x18].try_into().unwrap());
        let m = u64::from_le_bytes(fo[p + 0x28..p + 0x30].try_into().unwrap());
        if v + m == addr + size {
            fo[p + 0x28..p + 0x30].copy_from_slice(&(m - size).to_le_bytes());
        }
    }
    let ft: String = text
        .lines()
        .filter(|l| !l.starts_with("chunk synthetic .pad.relro "))
        .collect::<Vec<_>>()
        .join("\n");
    (x, Link { out: fo, map: parse(ft.trim_end()) }, d)
}

fn equiv_in(x: &Link, f: &Link, d: &Path) -> Equiv {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for e in std::fs::read_dir(d).unwrap() {
        let p = e.unwrap().path();
        if p.extension().and_then(|x| x.to_str()).is_some_and(|x| matches!(x, "o" | "a" | "so")) {
            files.push((p.to_str().unwrap().to_string(), std::fs::read(&p).unwrap()));
        }
    }
    let inputs: Vec<Input<'_>> = files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
    mortar_plumb::equiv::check(&x.out, &x.map, &f.out, &f.map, &inputs)
}

// §6.9 (WP-222, L10's L-2): `.pad.relro` of size 0 with no chunk record in
// one side and of some size in the other is accepted by P-E1 and P-E2 (the
// pad's size is a layout fact, L-64); the pad's flags still compare
// (P-E1). The base verifier gave P-E2 "the synthetic chunk '.pad.relro'
// (0) is only in the update" and P-E1 on the two program headers.
#[test]
fn p_e1_p_e2_pad_relro_may_be_empty_in_one_side() {
    for arch in ["aarch64", "x86_64"] {
        let (x, f, d) = pad_pair(arch);
        // The edited F's RELRO end is no longer on a page (a plain link
        // whose pad is empty has its end there already): its own P-D4
        // findings are the fixture's, and only those.
        let e = equiv_in(&x, &f, &d);
        assert!(
            e.findings.is_empty()
                && e.x.findings.is_empty()
                && e.f.findings.iter().all(|g| g.rule.id() == "P-D4"),
            "{arch}: {:#?} {:#?} {:#?}",
            lines(&e.findings),
            lines(&e.x.findings),
            lines(&e.f.findings)
        );
        let e = equiv_in(&f, &x, &d);
        assert!(lines(&e.findings).is_empty(), "{arch} reversed: {:#?}", lines(&e.findings));
        // The pad not writable in the update: P-E1 on its flags.
        let mut xw = Link { out: x.out.clone(), map: x.map.clone() };
        let pad = shdr_at(&xw.out, ".pad.relro");
        xw.out[pad + 8] &= !1;
        let e = equiv_in(&xw, &f, &d);
        let got = lines(&e.findings);
        assert!(
            got.len() == 1 && got[0].starts_with("P-E1 ") && got[0].contains(".pad.relro"),
            "{arch} flags: {got:#?}"
        );
    }
}

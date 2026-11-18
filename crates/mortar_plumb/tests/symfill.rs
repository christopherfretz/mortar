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
        !got.is_empty() && got.iter().all(|l| l.starts_with(&format!("{rule} ")) && l.contains(needle)),
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
        assert!(e.clean(), "{arch}: {:#?} {:#?} {:#?}", lines(&e.findings), lines(&e.x.findings), lines(&e.f.findings));
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
    assert!(e.x.findings.is_empty() && e.f.findings.is_empty());
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
            let e = rejected_by_equiv(&x, &f, field, "P-E3", &format!("a filler but for its {field}"));
            assert!(e.x.findings.is_empty(), "{arch} {field}: {:#?}", lines(&e.x.findings));
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
        rejected_alone(arch, &with(&["hole-nonalloc"]), "P-I2", "a hole is in an allocated section only");
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
        assert!(e.clean() && e.not_judged.is_empty(), "{arch}: {:#?} {:?}", lines(&e.findings), e.not_judged);
        let other = edit(arch, "x", &with(&["common-file=0"])).unwrap();
        rejected_by_equiv(&other, &x, "common-file", "P-E2", "the common 'shared_c' is allocated from");
    }
}

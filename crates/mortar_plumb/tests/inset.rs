//! The input set (spec/plumb.md §3.8, WP-156) on checked-in links
//! (`tests/inset/gen.sh`): the EXTERN defect of 2026-11-17 found by name
//! in mortar's output and not in GNU ld's link of the same line, and the
//! mutations of §9.2 on mortar's and GNU ld's maps.

use mortar_plumb::{Ctx, Files, Format, Input, Map, MapFile, Report, Rule, map, verify_with};
use std::path::{Path, PathBuf};

const ARCHES: [&str; 2] = ["aarch64", "x86_64"];
const INPUTS: [&str; 7] = ["m.o", "k.o", "j.o", "r.o", "w.o", "libk.a", "ext.ld"];

struct Dir(PathBuf);

impl Files for Dir {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(path)).ok()
    }
}

fn dir(arch: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inset").join(arch)
}

fn read_map(arch: &str, name: &str) -> Map {
    let format = if name.contains(".gnu") { Format::Gnu } else { Format::Mortar };
    let text = std::fs::read_to_string(dir(arch).join(name)).unwrap();
    map::parse(format, &text).unwrap()
}

/// Verifies `output` against `m`, with `line` for a reference's map.
fn verify(arch: &str, output: &str, m: &Map, line: Option<&str>) -> Report {
    let d = dir(arch);
    let bytes: Vec<(String, Vec<u8>)> =
        INPUTS.iter().map(|p| ((*p).to_string(), std::fs::read(d.join(p)).unwrap())).collect();
    let inputs: Vec<Input<'_>> = bytes.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
    let out = std::fs::read(d.join(output)).unwrap();
    let files = Dir(d);
    let ctx = Ctx {
        line: line.map(|l| l.split_whitespace().map(str::to_string).collect()),
        files: &files,
    };
    verify_with(&out, &inputs, m, &ctx)
}

/// The input set's findings, `id message`.
fn set(r: &Report) -> Vec<String> {
    r.findings
        .iter()
        .filter(|f| matches!(f.rule, Rule::Map(11 | 12)))
        .map(|f| format!("{} {}", f.rule.id(), f.message))
        .collect()
}

fn coverage(r: &Report) -> String {
    r.coverage.input_set.clone().unwrap_or_default()
}

/// `m` without the file that is `member` of `libk.a`, and everything
/// of it (the later files renumbered).
fn without_last(mut m: Map, member: &str) -> Map {
    let i = m.files.iter().position(|f| f.member.as_deref() == Some(member)).unwrap();
    m.files.remove(i);
    let i = i as u32;
    let moved = |f: &mut u32| {
        if *f > i {
            *f -= 1;
        }
    };
    m.placed.retain(|p| p.file != i);
    m.placed.iter_mut().for_each(|p| moved(&mut p.file));
    m.discarded.retain(|p| p.file != i);
    m.discarded.iter_mut().for_each(|p| moved(&mut p.file));
    m.nonalloc.retain(|p| p.file != i);
    m.nonalloc.iter_mut().for_each(|p| moved(&mut p.file));
    assert!(m.folds.is_empty() && m.commons.is_empty(), "renumbered here too");
    m.included.retain(|x| x.file.member.as_deref() != Some(member));
    m
}

fn with_member(mut m: Map, member: &str) -> Map {
    m.files.push(MapFile::member("libk.a", member));
    m
}

const P_GNU: &str = "-static m.o ext.ld libk.a -o p.gnu -Map=p.gnu.map";
const Q_GNU: &str = "-static r.o libk.a -o q.gnu -Map=q.gnu.map";
const QA_GNU: &str =
    "-static r.o --whole-archive libk.a --no-whole-archive -o qa.gnu -Map=qa.gnu.map";
const QW_GNU: &str = "-static w.o libk.a -o qw.gnu -Map=qw.gnu.map";

/// The defect: mortar at e1e60cf read `EXTERN(k_fn)` and did not root
/// it; `libk.a(k.o)` (and its constructor) is missing. GNU ld's link of
/// the same line is clean.
#[test]
fn s3_8_the_extern_defect_is_found_by_name() {
    for arch in ARCHES {
        let r = verify(arch, "p", &read_map(arch, "p.map"), None);
        assert_eq!(
            set(&r),
            [
                "M11 libk.a(k.o) is not in the link: EXTERN(k_fn) roots `k_fn` (resolve §4.2) and it is first(`k_fn`)"
            ],
            "{arch}"
        );
        assert_eq!(
            r.findings.iter().find(|f| f.rule == Rule::Map(11)).unwrap().symbol.as_deref(),
            Some("k_fn")
        );
        assert_eq!(
            coverage(&r),
            "input set: files judged 3 of 3 (in the link 1, required 2)",
            "{arch}"
        );
        let g = verify(arch, "p.gnu", &read_map(arch, "p.gnu.map"), Some(P_GNU));
        assert!(set(&g).is_empty(), "{arch}: {:?}", set(&g));
        assert_eq!(
            coverage(&g),
            "input set: files judged 3 of 3 (in the link 2); mortar's closure: the same",
            "{arch}"
        );
    }
}

/// The links the mutations start from are clean, and the two orders of
/// an archive and the object that refers to it give the same set.
#[test]
fn s3_8_the_unmutated_links_are_clean() {
    for arch in ARCHES {
        for (out, m, line) in [
            ("q", "q.map", None),
            ("qr", "qr.map", None),
            ("qw", "qw.map", None),
            ("q.gnu", "q.gnu.map", Some(Q_GNU)),
            ("qa.gnu", "qa.gnu.map", Some(QA_GNU)),
            ("qw.gnu", "qw.gnu.map", Some(QW_GNU)),
        ] {
            let r = verify(arch, out, &read_map(arch, m), line);
            assert!(set(&r).is_empty(), "{arch} {out}: {:?}", set(&r));
            assert!(
                coverage(&r).starts_with("input set: files judged 3 of 3"),
                "{arch} {out}: {}",
                coverage(&r)
            );
        }
        let files = |m: &Map| {
            let mut v: Vec<String> = m.files.iter().map(ToString::to_string).collect();
            v.sort();
            v
        };
        assert_eq!(files(&read_map(arch, "q.map")), files(&read_map(arch, "qr.map")), "{arch}");
        assert_eq!(files(&read_map(arch, "q.map")), ["libk.a(k.o)", "r.o"], "{arch}");
    }
}

/// §9.2's mutations of the input set, on mortar's and GNU ld's maps.
#[test]
#[allow(clippy::too_many_lines)] // one block per mutation of §9.2 (s)
fn s3_8_mutations() {
    for arch in ARCHES {
        // A required member removed.
        let r = verify(arch, "q", &without_last(read_map(arch, "q.map"), "k.o"), None);
        assert_eq!(
            set(&r),
            [
                "M11 libk.a(k.o) is not in the link: r.o refers to `k_fn` and it is the target (resolve §4.1)"
            ],
            "{arch}"
        );
        // The other order (the archive first; mortar's rule does not
        // depend on it, resolve §4.3).
        let mut m = without_last(read_map(arch, "q.map"), "k.o");
        m.args = "-static libk.a r.o -o q --mortar-map=q.map"
            .split(' ')
            .map(|w| w.as_bytes().to_vec())
            .collect();
        let r = verify(arch, "q", &m, None);
        assert_eq!(
            set(&r),
            [
                "M11 libk.a(k.o) is not in the link: r.o refers to `k_fn` and it is the target (resolve §4.1)"
            ],
            "{arch}"
        );
        let r =
            verify(arch, "q.gnu", &without_last(read_map(arch, "q.gnu.map"), "k.o"), Some(Q_GNU));
        assert_eq!(
            set(&r),
            [
                "M11 `k_fn` (r.o) is defined by no file of the link, and libk.a(k.o) defines it (§3.8)"
            ],
            "{arch}"
        );
        // The same on GNU ld's line with the archive first: a
        // back-reference, which GNU ld does not extract: not judged.
        let r = verify(
            arch,
            "q.gnu",
            &without_last(read_map(arch, "q.gnu.map"), "k.o"),
            Some("-static libk.a r.o -o q.gnu"),
        );
        assert!(set(&r).is_empty(), "{arch}: {:?}", set(&r));
        assert!(
            coverage(&r).contains("demands not judged 1 (GNU ld's back-references)"),
            "{arch}: {}",
            coverage(&r)
        );

        // An unreferenced member added.
        let r = verify(arch, "q", &with_member(read_map(arch, "q.map"), "j.o"), None);
        assert_eq!(
            set(&r),
            [
                "M12 libk.a(j.o) is in the link and no root or live reference reaches it (resolve §4.1)"
            ],
            "{arch}"
        );
        let r =
            verify(arch, "q.gnu", &with_member(read_map(arch, "q.gnu.map"), "j.o"), Some(Q_GNU));
        assert_eq!(
            set(&r),
            [
                "M12 GNU ld's map places libk.a(j.o) and its preamble does not say it was included",
                "M12 libk.a(j.o) is in the link and no root or reference of the link reaches it (§3.8)",
            ],
            "{arch}"
        );

        // A --whole-archive member dropped: mortar's map of `r.o libk.a`
        // read as the line with --whole-archive (mortar at e1e60cf
        // refuses the option), and GNU ld's map without j.o.
        let mut m = read_map(arch, "q.map");
        m.args = "-static r.o --whole-archive libk.a --no-whole-archive -o q --mortar-map=q.map"
            .split(' ')
            .map(|w| w.as_bytes().to_vec())
            .collect();
        let r = verify(arch, "q", &m, None);
        assert_eq!(
            set(&r),
            ["M11 libk.a(j.o) is not in the link: it is live from the start (resolve §4.2)"],
            "{arch}"
        );
        let r = verify(
            arch,
            "qa.gnu",
            &without_last(read_map(arch, "qa.gnu.map"), "j.o"),
            Some(QA_GNU),
        );
        assert_eq!(
            set(&r),
            ["M11 libk.a(j.o) is not in the link: it is live from the start (resolve §4.2)"],
            "{arch}"
        );

        // A member extracted only for a weak reference.
        let r = verify(arch, "qw", &with_member(read_map(arch, "qw.map"), "k.o"), None);
        assert_eq!(
            set(&r),
            [
                "M12 libk.a(k.o) is in the link and no root or live reference reaches it (resolve §4.1)"
            ],
            "{arch}"
        );
        let r =
            verify(arch, "qw.gnu", &with_member(read_map(arch, "qw.gnu.map"), "k.o"), Some(QW_GNU));
        assert_eq!(
            set(&r),
            [
                "M12 GNU ld's map places libk.a(k.o) and its preamble does not say it was included",
                "M12 libk.a(k.o) is in the link and no root or reference of the link reaches it (§3.8)",
            ],
            "{arch}"
        );

        // GNU ld's printed reason, false.
        let mut m = read_map(arch, "q.gnu.map");
        assert_eq!(m.included.len(), 1);
        if let map::IncludedBy::Reference { symbol, .. } = &mut m.included[0].by {
            *symbol = "j_fn".into();
        }
        let r = verify(arch, "q.gnu", &m, Some(Q_GNU));
        assert_eq!(
            set(&r),
            [
                "M12 GNU ld's map (line 3) says libk.a(k.o) was included for r.o (j_fn): r.o has no undefined or common `j_fn`"
            ],
            "{arch}"
        );
    }
}

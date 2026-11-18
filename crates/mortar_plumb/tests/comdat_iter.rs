//! The input set over resolve §4.1's iterations (spec/plumb.md §3.8,
//! WP-231) on mortar's checked-in links of WP-200's cells
//! (`tests/comdat_iter/gen.sh`): a dropped COMDAT copy's definition stops
//! counting for `first` once it is dropped, so a strong reference
//! extracts the member that defines the key; a demoted mention pulls
//! nothing. The mutations of §9.2 (s) (6)-(8).

use mortar_plumb::{Ctx, Files, Format, Input, Map, MapFile, Report, Rule, map, verify_with};
use std::path::{Path, PathBuf};

const ARCHES: [&str; 2] = ["aarch64", "x86_64"];
const INPUTS: [&str; 13] = [
    "wu.o", "wu2.o", "wa.o", "wb.o", "wbs.o", "hx.o", "hxg.o", "wa2.o", "wb2.o", "libhx.a",
    "libwb.a", "libhxg.a", "libwa2.a",
];

struct Dir(PathBuf);

impl Files for Dir {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(path)).ok()
    }
}

fn dir(arch: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/comdat_iter").join(arch)
}

fn read_map(arch: &str, name: &str) -> Map {
    let text = std::fs::read_to_string(dir(arch).join(format!("{name}.map"))).unwrap();
    map::parse(Format::Mortar, &text).unwrap()
}

fn verify(arch: &str, output: &str, m: &Map) -> Report {
    let d = dir(arch);
    let bytes: Vec<(String, Vec<u8>)> =
        INPUTS.iter().map(|p| ((*p).to_string(), std::fs::read(d.join(p)).unwrap())).collect();
    let inputs: Vec<Input<'_>> = bytes.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
    let out = std::fs::read(d.join(output)).unwrap();
    let files = Dir(d);
    verify_with(&out, &inputs, m, &Ctx { line: None, files: &files })
}

/// The input set's findings, `id message`.
fn set(r: &Report) -> Vec<String> {
    r.findings
        .iter()
        .filter(|f| matches!(f.rule, Rule::Map(11 | 12)))
        .map(|f| format!("{} {}", f.rule.id(), f.message))
        .collect()
}

fn all(r: &Report) -> Vec<String> {
    r.findings.iter().map(|f| format!("{} {}", f.rule.id(), f.message)).collect()
}

/// `m` without the file `path(member)`, and everything of it (the later
/// files renumbered).
fn without(mut m: Map, path: &str, member: &str) -> Map {
    let i =
        m.files.iter().position(|f| f.path == path && f.member.as_deref() == Some(member)).unwrap();
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
    m
}

/// The cells where the member is extracted in the closure's second
/// iteration: (output, the member's archive, the member, who refers).
const CELLS: [(&str, &str, &str); 5] = [
    ("ext", "libhx.a", "hx.o"),
    ("after", "libhx.a", "hx.o"),
    ("member", "libhx.a", "hx.o"),
    ("copy", "libhxg.a", "hxg.o"),
    ("grp", "libhx.a", "hx.o"),
];

/// mortar's outputs of WP-200's cells pass, the member required by the
/// second iteration of resolve §4.1 (the old model, which counted the
/// dropped copy's `hx` as first(hx), called it M12).
#[test]
fn s3_8_the_iterated_closure_requires_the_member() {
    for arch in ARCHES {
        for (out, _, _) in CELLS {
            let r = verify(arch, out, &read_map(arch, out));
            assert!(r.findings.is_empty(), "{arch} {out}: {:?}", all(&r));
            let c = r.coverage.input_set.clone().unwrap_or_default();
            assert_eq!(
                c,
                "input set: files judged 4 of 4 (in the link 4, required 4); resolve §4.1's closure iterated 2 times",
                "{arch} {out}"
            );
        }
    }
}

/// Mutation (6): the member the iteration reaches removed from the map:
/// M11 naming it and `hx`, by wu.o's reference (the answer of the old
/// model, which passed it, fails).
#[test]
fn s3_8_the_member_the_iteration_reaches_removed_is_m11() {
    for arch in ARCHES {
        for (out, ar, member) in CELLS {
            let r = verify(arch, out, &without(read_map(arch, out), ar, member));
            assert_eq!(
                set(&r),
                [format!(
                    "M11 {ar}({member}) is not in the link: wu.o refers to `hx` and it is the target (resolve §4.1)"
                )],
                "{arch} {out}"
            );
            let m11 = r.findings.iter().find(|f| f.rule == Rule::Map(11)).unwrap();
            assert_eq!(m11.symbol.as_deref(), Some("hx"));
        }
    }
}

/// Mutation (7): a member added that only a demoted `STB_GLOBAL` mention
/// refers to (`dem`: wbs.o's `hx` in its dropped copy of `g`, nothing
/// else refers to `hx`): M12 naming the mention, its file and the group
/// (resolve §6.2: a demoted mention pulls nothing). The link itself is
/// clean; wbs.o's `hx` is first(hx) when its copy is dropped, so §4.1's
/// trigger runs a second iteration, which extracts nothing.
#[test]
fn s3_8_a_member_only_a_demoted_mention_names_is_m12() {
    for arch in ARCHES {
        let r = verify(arch, "dem", &read_map(arch, "dem"));
        assert!(r.findings.is_empty(), "{arch}: {:?}", all(&r));
        assert_eq!(
            r.coverage.input_set.as_deref(),
            Some(
                "input set: files judged 4 of 4 (in the link 3, required 3); resolve §4.1's closure iterated 2 times"
            ),
            "{arch}"
        );
        let mut m = read_map(arch, "dem");
        m.files.push(MapFile::member("libhx.a", "hx.o"));
        let r = verify(arch, "dem", &m);
        assert_eq!(
            set(&r),
            [
                "M12 libhx.a(hx.o) is in the link and no root or live reference reaches it, only the demoted mention `hx` of wbs.o (its dropped copy of COMDAT group `g`) (resolve §6.2: a demoted mention pulls nothing)"
            ],
            "{arch}"
        );
    }
}

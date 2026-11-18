//! WP-251: `spec/plumb.md` §3.3 M10's row for `spec/incr.md` IN-55
//! option (A): an incremental-mode output leaves the GNU name index
//! (`.debug_gnu_pubnames`, `.debug_gnu_pubtypes`) out, on AArch64 and
//! x86-64.
//!
//! The subjects are mortar's links checked in under `tests/gnuindex/`
//! (`gen.sh` regenerates them; WP-229's linker): `inc` (two
//! `-gsplit-dwarf` objects and a `-gpubnames` one, `--incremental=full`),
//! `plain` (the same, no `--incremental`), `sorted` (`--incremental
//! --sort-common`: plain by IN-12), `group` (a set in a COMDAT group with
//! an allocated member) and `alloc` (an allocated section of the name).
//! Each is clean; each mutation of the map or the line (a set kept in an
//! incremental-mode output, a set left out of a plain one, a third name
//! left out, the allocated section left out, the map's word against the
//! line) is found by M10.

mod common;

use std::path::{Path, PathBuf};

use common::verify_with;
use mortar_plumb::map::{DiscardReason, Discarded, Format, Map, SectionRef};
use mortar_plumb::{Report, map};

const ARCHES: [&str; 2] = ["aarch64", "x86_64"];

fn dir(arch: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gnuindex").join(arch)
}

/// An output, its inputs and its map.
struct Link {
    output: Vec<u8>,
    files: Vec<(String, Vec<u8>)>,
    map: Map,
}

fn load(arch: &str, name: &str) -> Link {
    let d = dir(arch);
    let output = std::fs::read(d.join(name)).unwrap();
    let text = std::fs::read_to_string(d.join(format!("{name}.map"))).unwrap();
    let map = map::parse(Format::Mortar, &text).unwrap_or_else(|e| panic!("{name}: {}", e.message));
    let files = map
        .paths()
        .into_iter()
        .map(|p| (p.to_string(), std::fs::read(d.join(p)).unwrap()))
        .collect();
    Link { output, files, map }
}

impl Link {
    fn verify(&self) -> Report {
        verify_with(&self.output, &self.files, &self.map)
    }

    /// The map's line with `drop` left out and `add` appended.
    fn line(&mut self, drop: &[&str], add: &[&str]) {
        self.map.args.retain(|w| !drop.iter().any(|d| w == d.as_bytes()));
        self.map.args.extend(add.iter().map(|a| a.as_bytes().to_vec()));
    }

    /// The input section `name` of file `file`, by index.
    fn index(&self, file: &str, name: &str) -> u32 {
        let bytes = &self.files.iter().find(|(p, _)| p == file).unwrap().1;
        let o = mortar_elf::Object::parse(bytes).unwrap();
        o.sections()
            .find(|s| s.name().is_ok_and(|n| n == name.as_bytes()))
            .unwrap_or_else(|| panic!("{file} has no {name}"))
            .index()
            .0
    }

    fn file_id(&self, file: &str) -> u32 {
        self.map.files.iter().position(|f| f.path == file).unwrap() as u32
    }

    /// Every record of `file`'s `section` replaced by a `discard ...
    /// unspecified` (`spec/image.md` I-78's line for a set left out).
    fn discard(&mut self, file: &str, section: &str) {
        let (f, s) = (self.file_id(file), SectionRef::Index(self.index(file, section)));
        self.map.nonalloc.retain(|r| !(r.file == f && r.section == s));
        self.map.placed.retain(|r| !(r.file == f && r.section == s));
        self.map.discarded.push(Discarded {
            file: f,
            section: s,
            reason: DiscardReason::Unspecified,
            line: 0,
        });
    }
}

/// The M10 findings.
fn m10(r: &Report) -> Vec<String> {
    r.with_rule("M10").map(|f| f.message.clone()).collect()
}

/// Every finding, for a clean subject.
fn all(r: &Report) -> Vec<String> {
    r.findings.iter().map(|f| format!("{} {}", f.rule.id(), f.message)).collect()
}

const LEFT_OUT: &str = "discarded as 'unspecified' but no row of resolve.md §10 drops it";
const PLACED: &str = "is placed in an incremental-mode output: incr.md IN-55 option (A)";

// ---- the subjects as mortar wrote them ----

/// (a) The two GNU sets of each split object left out of an
/// incremental-mode output, each with its `discard` record: clean; the
/// `-gpubnames` sets (not in the list) stay placed. The plain links keep
/// the sets: clean.
#[test]
fn wp251_subjects_are_clean() {
    for arch in ARCHES {
        for name in ["inc", "plain", "sorted", "group", "alloc"] {
            let l = load(arch, name);
            let r = l.verify();
            assert!(r.findings.is_empty(), "{arch} {name}: {:#?}", all(&r));
        }
        let inc = load(arch, "inc");
        assert!(inc.map.incremental);
        let gnu = inc
            .map
            .discarded
            .iter()
            .filter(|d| d.reason == DiscardReason::Unspecified && d.file < 2)
            .count();
        assert_eq!(gnu, 4, "{arch}: the four sets of a.o and b.o are left out");
        assert!(!load(arch, "sorted").map.incremental, "{arch}: IN-12 made it plain");
    }
}

// ---- (b) a set placed in an incremental-mode output ----

/// The plain link's output and map, with the map saying `incremental` and
/// the line `--incremental=full`: each of the four placed sets is M10.
#[test]
fn wp251_set_kept_in_incremental_output_is_m10() {
    for arch in ARCHES {
        let mut l = load(arch, "plain");
        l.map.incremental = true;
        l.line(&[], &["--incremental=full"]);
        let f = m10(&l.verify());
        let placed: Vec<_> = f.iter().filter(|m| m.contains(PLACED)).collect();
        assert_eq!(placed.len(), 4, "{arch}: {f:#?}");
        assert_eq!(f.len(), 4, "{arch}: {f:#?}");
        // The same output under its own (plain) map: clean.
        assert!(m10(&load(arch, "plain").verify()).is_empty());
    }
}

/// A map that says it left the sets out of an output that has them: the
/// output's bytes are M10, once per output section.
#[test]
fn wp251_set_in_the_output_with_a_discard_line_is_m10() {
    for arch in ARCHES {
        let mut l = load(arch, "plain");
        l.map.incremental = true;
        l.line(&[], &["--incremental=full"]);
        for f in ["a.o", "b.o"] {
            l.discard(f, ".debug_gnu_pubnames");
            l.discard(f, ".debug_gnu_pubtypes");
        }
        let f = m10(&l.verify());
        let have = |n: &str| {
            f.iter().any(|m| {
                m.contains(&format!("the incremental-mode output has section '{n}'"))
                    && m.contains("IN-55 option (A)")
            })
        };
        assert!(have(".debug_gnu_pubnames") && have(".debug_gnu_pubtypes"), "{arch}: {f:#?}");
        assert_eq!(f.len(), 2, "{arch}: {f:#?}");
    }
}

// ---- (c) a set left out of a plain output ----

/// The incremental link's output and map with the map's `incremental`
/// and the line's `--incremental=full` taken out: each left-out set is
/// M10 as before WP-251.
#[test]
fn wp251_set_left_out_of_plain_output_is_m10() {
    for arch in ARCHES {
        let mut l = load(arch, "inc");
        l.map.incremental = false;
        l.map.dead.clear();
        l.line(&["--incremental=full"], &[]);
        let f = m10(&l.verify());
        assert_eq!(f.iter().filter(|m| m.contains(LEFT_OUT)).count(), 4, "{arch}: {f:#?}");
        assert_eq!(f.len(), 4, "{arch}: {f:#?}");
    }
}

/// The map says `incremental` but the line rules the mode out (no
/// `--incremental`, `--no-incremental` last, or an IN-12 option): the
/// map's word is not taken alone, the output is judged as a plain one
/// and the four sets left out are M10 (none under `--gdb-index`, whose
/// own row, S-G6, drops them). The negations keep the mode: clean.
#[test]
fn wp251_map_word_against_the_line_is_plain() {
    let cases: [(&[&str], &[&str]); 6] = [
        (&["--incremental=full"], &[]),
        (&[], &["--no-incremental"]),
        (&[], &["--sort-common"]),
        (&[], &["--gdb-index"]),
        (&[], &["--compress-debug-sections=zlib"]),
        (&[], &["-r"]),
    ];
    for arch in ARCHES {
        for (drop, add) in cases {
            let mut l = load(arch, "inc");
            l.line(drop, add);
            let f = m10(&l.verify());
            let left = if add == ["--gdb-index"] { 0 } else { 4 };
            assert_eq!(f.iter().filter(|m| m.contains(LEFT_OUT)).count(), left, "{arch} {add:?}");
            assert_eq!(f.len(), left, "{arch} {add:?}: {f:#?}");
        }
        for add in [
            &["--gdb-index", "--no-gdb-index"][..],
            &["--compress-debug-sections=none"],
            &["--icf=none"],
            &["--no-incremental", "--incremental"],
        ] {
            let mut l = load(arch, "inc");
            l.line(&[], add);
            assert!(m10(&l.verify()).is_empty(), "{arch} {add:?}");
        }
    }
}

// ---- other names, other kinds ----

/// `.debug_pubnames` (g++/gcc `-gpubnames`) is not in IN-55 option (A)'s
/// list: left out of an incremental-mode output, it is M10.
#[test]
fn wp251_third_name_left_out_is_m10() {
    for arch in ARCHES {
        let mut l = load(arch, "inc");
        l.discard("p.o", ".debug_pubnames");
        let f = m10(&l.verify());
        assert!(
            f.len() == 1 && f[0].contains("'.debug_pubnames'") && f[0].contains(LEFT_OUT),
            "{arch}: {f:#?}"
        );
    }
}

/// An allocated section named `.debug_gnu_pubnames` is not debug
/// information: placed in an incremental-mode output is right (the
/// subject is clean); left out, it is M10.
#[test]
fn wp251_allocated_section_of_the_name_is_not_left_out() {
    for arch in ARCHES {
        let mut l = load(arch, "alloc");
        l.discard("x-alloc.o", ".debug_gnu_pubnames");
        let f = m10(&l.verify());
        assert!(
            f.len() == 1 && f[0].contains("'.debug_gnu_pubnames'") && f[0].contains(LEFT_OUT),
            "{arch}: {f:#?}"
        );
    }
}

/// A set with no record at all (neither placed nor discarded) is M10 in
/// incremental mode too: the row needs the `discard` line.
#[test]
fn wp251_set_with_no_record_is_m10() {
    for arch in ARCHES {
        let mut l = load(arch, "inc");
        let s = SectionRef::Index(l.index("a.o", ".debug_gnu_pubnames"));
        let f0 = l.file_id("a.o");
        l.map.discarded.retain(|d| !(d.file == f0 && d.section == s));
        let f = m10(&l.verify());
        assert!(
            f.len() == 1
                && f[0].contains("'.debug_gnu_pubnames'")
                && f[0].contains("neither places nor discards it"),
            "{arch}: {f:#?}"
        );
    }
}

/// The set of a COMDAT group with an allocated member (WP-229's GROUP):
/// the group's keeper is the only copy, so its set is a kept set, left
/// out with `discard ... unspecified` (the subject is clean, above); the
/// same record given as `comdat` (its group is the keeper) is M10.
#[test]
fn wp251_group_member_set() {
    for arch in ARCHES {
        let l = load(arch, "group");
        let i = l.index("x-group.o", ".debug_gnu_pubnames");
        let f2 = l.file_id("x-group.o");
        let mut l = l;
        let d = l
            .map
            .discarded
            .iter_mut()
            .find(|d| d.file == f2 && d.section == SectionRef::Index(i))
            .unwrap();
        assert_eq!(d.reason, DiscardReason::Unspecified, "{arch}");
        d.reason = DiscardReason::Comdat;
        let f = m10(&l.verify());
        assert!(f.len() == 1 && f[0].contains("its group is the keeper"), "{arch}: {f:#?}");
    }
}

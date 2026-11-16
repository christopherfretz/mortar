//! hod's `.mason.meta` (WP-099, `spec/landed/meta/WP-099.md`): objects
//! hod made from the `.ll` files of `tests/fixtures/hod-meta-a598555f/`
//! (its README has the command), linked by mortar on AArch64 and x86-64,
//! each case with the one rule of `spec/meta.md` that decides it: records
//! read through hod's container (a one-definition-rule error under
//! `--odr=strict`, two objects or two units of one), what `--keep-mason-meta`
//! keeps, and every section a code generator can write around the
//! container (`SHF_GNU_RETAIN`, a group, a relocation section, `SHF_ALLOC`,
//! a large-data name, alignment 4, empty) either handled by a rule or
//! warned by name, none read silently. The objects are committed so the
//! hosted lanes (no hod build) run this; [`the_fixtures_are_hods`]
//! re-makes them where hod is built and compares the bytes.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mortar_testutil::{Outcome, Scratch, run_with};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");
const ARCHES: [&str; 2] = ["aarch64", "x86_64"];
/// The `.ll` files, each made into `<case>.<arch>.o`.
const CASES: [&str; 12] = [
    "base",
    "used",
    "comdat",
    "two",
    "reloc",
    "alloc",
    "large",
    "align4",
    "empty",
    "odr-a",
    "odr-b",
    "two-units",
];
/// The 80-byte unit of the item-3 cases (one `summary lang=c++`).
const UNIT_LEN: usize = 80;

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hod-meta-a598555f")
}

fn object(case: &str, arch: &str) -> PathBuf {
    dir().join(format!("{case}.{arch}.o"))
}

/// The sections of a 64-bit little-endian ELF file: (name, flags,
/// contents).
fn sections(bytes: &[u8]) -> Vec<(String, u64, Vec<u8>)> {
    let word = |at: usize, n: usize| -> u64 {
        bytes[at..at + n].iter().rev().fold(0u64, |v, &x| (v << 8) | u64::from(x))
    };
    assert_eq!(&bytes[..6], b"\x7fELF\x02\x01", "a 64-bit little-endian ELF file");
    let as_usize = |v: u64| usize::try_from(v).unwrap();
    let shoff = as_usize(word(0x28, 8));
    let shentsize = as_usize(word(0x3a, 2));
    let shnum = as_usize(word(0x3c, 2));
    let shstrndx = as_usize(word(0x3e, 2));
    let header = |i: usize| shoff + i * shentsize;
    let strtab = as_usize(word(header(shstrndx) + 0x18, 8));
    (0..shnum)
        .map(|i| {
            let h = header(i);
            let name_at = strtab + as_usize(word(h, 4));
            let end = bytes[name_at..].iter().position(|&b| b == 0).unwrap();
            let name = String::from_utf8_lossy(&bytes[name_at..name_at + end]).into_owned();
            let kind = word(h + 4, 4);
            let flags = word(h + 8, 8);
            let offset = as_usize(word(h + 0x18, 8));
            let size = as_usize(word(h + 0x20, 8));
            // SHT_NOBITS has no bytes in the file.
            let data = if kind == 8 { Vec::new() } else { bytes[offset..offset + size].to_vec() };
            (name, flags, data)
        })
        .collect()
}

fn meta_of(path: &Path) -> Option<(u64, Vec<u8>)> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    sections(&bytes).into_iter().find(|(n, _, _)| n == ".mason.meta").map(|(_, f, d)| (f, d))
}

/// What a link did: its outcome and the output's `.mason.meta`, if any.
struct Linked {
    outcome: Outcome,
    meta: Option<(u64, Vec<u8>)>,
}

impl Linked {
    fn stderr(&self) -> String {
        self.outcome.stderr_text()
    }

    /// Succeeded with nothing on standard error.
    fn silent(&self) -> bool {
        self.outcome.success() && self.outcome.stderr.is_empty()
    }

    fn warned_not_container(&self, why: &str) -> bool {
        let e = self.stderr();
        self.outcome.success()
            && e.lines().count() == 1
            && e.contains("warning:")
            && e.contains("malformed Mason metadata: the section is not a container")
            && e.contains(why)
    }
}

fn link(scratch: &Scratch, arch: &str, cases: &[&str], options: &[&str]) -> Linked {
    let out = scratch.join(format!("{}-{arch}-{}", cases.join("+"), options.join("")));
    let _ = std::fs::remove_file(&out);
    let mut command = Command::new(MORTAR);
    command.args(["-static", "-e", "main"]);
    command.args(cases.iter().map(|c| object(c, arch)));
    command.args(options).arg("-o").arg(&out);
    let outcome = run_with(&mut command, b"", Duration::from_mins(1));
    assert!(!outcome.timed_out(), "{cases:?} {arch} {options:?}: timed out");
    let code = outcome.status.and_then(|s| s.code());
    assert!(
        matches!(code, Some(0 | 1)),
        "{cases:?} {arch} {options:?}: {code:?}: {}",
        outcome.stderr_text()
    );
    let meta = if outcome.success() { meta_of(&out) } else { None };
    Linked { outcome, meta }
}

/// The input container's bytes of a case.
fn input_unit(case: &str, arch: &str) -> Vec<u8> {
    meta_of(&object(case, arch)).expect("the object has a .mason.meta").1
}

// MT-1, MT-6, MT-42, MT-32: hod's container is a container; nothing is
// read by default; `--keep-mason-meta` keeps the unit byte for byte; the
// output never has one without the option (SHF_EXCLUDE).
#[test]
fn hods_container() {
    let s = Scratch::new("hod-meta-base");
    for arch in ARCHES {
        let (flags, unit) = meta_of(&object("base", arch)).unwrap();
        assert_eq!(flags, 0x8000_0000, "{arch}: SHF_EXCLUDE only");
        assert_eq!(unit.len(), UNIT_LEN);
        for options in [&[][..], &["--odr=strict"]] {
            let l = link(&s, arch, &["base"], options);
            assert!(l.silent(), "{arch} {options:?}: {}", l.stderr());
            assert!(l.meta.is_none(), "{arch} {options:?}: no .mason.meta in the output");
        }
        let l = link(&s, arch, &["base"], &["--keep-mason-meta"]);
        assert!(l.silent(), "{arch}: {}", l.stderr());
        assert_eq!(l.meta, Some((0, unit)), "{arch}: MT-32's section, the unit unchanged");
    }
}

// odr.md §7-§9 through hod: two objects whose records disagree on one
// definition are an error under --odr=strict and silent by default
// (kind 1 is strict-only, O-46); MT-4/MT-10: two units of one file (what
// `-r` of two translation units makes) are compared as two units.
#[test]
fn records_through_hod() {
    let s = Scratch::new("hod-meta-odr");
    for arch in ARCHES {
        for cases in [&["odr-a", "odr-b"][..], &["two-units"]] {
            let l = link(&s, arch, cases, &[]);
            assert!(l.silent(), "{arch} {cases:?}: {}", l.stderr());
            let l = link(&s, arch, cases, &["--odr=strict"]);
            assert!(!l.outcome.success(), "{arch} {cases:?}: an error under strict");
            let e = l.stderr();
            assert!(
                e.contains("error: _Z1fv violates the one-definition rule"),
                "{arch} {cases:?}: {e}"
            );
            let first = format!("{}.{arch}.o:(_Z1fv) (kept)", cases[0]);
            assert!(e.contains(&first), "{arch} {cases:?}: {e}");
        }
        let l = link(&s, arch, &["odr-a", "odr-b"], &["--keep-mason-meta"]);
        let mut kept = input_unit("odr-a", arch);
        kept.extend(input_unit("odr-b", arch));
        assert_eq!(l.meta.map(|m| m.1), Some(kept), "{arch}: both units, in file order");
    }
}

// MT-4, MT-10: two IR globals in one section are one section of two
// units, read and kept as two.
#[test]
fn two_globals_one_section() {
    let s = Scratch::new("hod-meta-two");
    for arch in ARCHES {
        let unit = input_unit("two", arch);
        assert_eq!(unit.len(), 2 * UNIT_LEN, "{arch}: one section, the initializers in order");
        assert!(link(&s, arch, &["two"], &["--odr=strict"]).silent());
        let l = link(&s, arch, &["two"], &["--keep-mason-meta"]);
        assert_eq!(l.meta.map(|m| m.1), Some(unit), "{arch}");
    }
}

// MT-6: SHF_GNU_RETAIN (`@llvm.used`) and SHF_GROUP (a comdat) make the
// section not a container: one warning per file in every mode, the
// section dropped by its SHF_EXCLUDE, nothing read, nothing kept.
// MT-2a (WP-099): a relocated container is not one either.
#[test]
fn not_containers_are_warned() {
    let s = Scratch::new("hod-meta-flags");
    for arch in ARCHES {
        for (case, flags, why) in [
            ("used", 0x8020_0000u64, "flags"),
            ("comdat", 0x8000_0200, "flags"),
            ("reloc", 0x8000_0000, "a relocation section applies to it"),
        ] {
            assert_eq!(
                meta_of(&object(case, arch)).unwrap().0,
                flags,
                "{case} {arch}: hod's flags"
            );
            for options in [&[][..], &["--odr=strict"], &["--keep-mason-meta"]] {
                let l = link(&s, arch, &[case], options);
                assert!(l.warned_not_container(why), "{case} {arch} {options:?}: {}", l.stderr());
                assert!(l.meta.is_none(), "{case} {arch} {options:?}: not copied, not kept");
            }
        }
    }
}

// MT-6: without `!exclude` the section is allocated, so it is an ordinary
// section, copied as any reference linker copies it, and warned; MT-32:
// with a kept section to merge into it is refused by name.
#[test]
fn allocated_section_is_copied_and_warned() {
    let s = Scratch::new("hod-meta-alloc");
    for arch in ARCHES {
        let (flags, bytes) = meta_of(&object("alloc", arch)).unwrap();
        assert_eq!(flags, 0x2, "{arch}: SHF_ALLOC only");
        let l = link(&s, arch, &["alloc"], &[]);
        assert!(l.warned_not_container("flags"), "{arch}: {}", l.stderr());
        assert_eq!(l.meta, Some((0x2, bytes)), "{arch}: copied as an ordinary section");
        let l = link(&s, arch, &["alloc", "odr-b"], &["--keep-mason-meta"]);
        assert!(!l.outcome.success(), "{arch}: refused");
        assert!(
            l.stderr().contains(
                "--keep-mason-meta with an input section named .mason.meta that is not a container"
            ),
            "{arch}: {}",
            l.stderr()
        );
    }
}

// MT-1: the name decides; hod's large-data name on x86-64
// (`.ldata.mason.meta`, SHF_X86_64_LARGE) is no container and its
// SHF_EXCLUDE drops it.
#[test]
fn large_data_name_is_not_the_container() {
    let s = Scratch::new("hod-meta-large");
    for arch in ARCHES {
        let bytes = std::fs::read(object("large", arch)).unwrap();
        let all = sections(&bytes);
        let (_, flags, _) = all.iter().find(|(n, _, _)| n == ".ldata.mason.meta").unwrap();
        let large = if arch == "x86_64" { 0x1000_0000 } else { 0 };
        assert_eq!(*flags, 0x8000_0000 | large, "{arch}");
        assert!(!all.iter().any(|(n, _, _)| n == ".mason.meta"), "{arch}");
        for options in [&[][..], &["--odr=strict"], &["--keep-mason-meta"]] {
            let l = link(&s, arch, &["large"], options);
            assert!(l.silent() && l.meta.is_none(), "{arch} {options:?}: {}", l.stderr());
        }
    }
}

// MT-14: alignment 4 is malformed, an error under --odr=strict; MT-42:
// nothing read by default; MT-32: dropped from the kept section.
// MT-10: an empty container is zero units.
#[test]
fn alignment_and_empty() {
    let s = Scratch::new("hod-meta-align");
    for arch in ARCHES {
        assert!(link(&s, arch, &["align4"], &[]).silent());
        let l = link(&s, arch, &["align4"], &["--odr=strict"]);
        assert!(!l.outcome.success(), "{arch}");
        assert!(
            l.stderr().contains("section sh_addralign is 4, not 8 (meta.md MT-1)"),
            "{arch}: {}",
            l.stderr()
        );
        let l = link(&s, arch, &["align4"], &["--keep-mason-meta"]);
        assert!(l.silent() && l.meta.is_none(), "{arch}: {}", l.stderr());
        assert!(input_unit("empty", arch).is_empty());
        for options in [&[][..], &["--odr=strict"], &["--keep-mason-meta"]] {
            let l = link(&s, arch, &["empty"], options);
            assert!(l.silent() && l.meta.is_none(), "{arch} {options:?}: {}", l.stderr());
        }
    }
}

/// hod, where it is built (`MASON_HOD`, else the build root's
/// `target-hod/release/hod`, the root being the parent of this test's
/// target directory, as `cargo xtask mason` finds it).
fn hod() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("MASON_HOD").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    // <root>/<target>/<profile>/deps/<test binary>
    let exe = std::env::current_exe().ok()?;
    let p = exe.ancestors().nth(4)?.join("target-hod/release/hod");
    p.is_file().then_some(p)
}

// The fixtures cannot go stale silently: where hod is built, each object
// is made again with the README's command and must be the same bytes
// (hod is deterministic). A hod other than a598555f that writes other
// bytes fails here: make the fixtures again in a new directory named for
// its sha.
#[test]
fn the_fixtures_are_hods() {
    let Some(hod) = hod() else {
        assert!(
            std::env::var_os("MASON_REQUIRE_TOOLS").is_none_or(|v| v != "1"),
            "hod is required (MASON_REQUIRE_TOOLS=1)"
        );
        return;
    };
    let s = Scratch::new("hod-meta-remake");
    for case in CASES {
        for arch in ARCHES {
            let out = s.join(format!("{case}.{arch}.o"));
            let mut command = Command::new(&hod);
            command
                .args(["--strict", "-O0", "-relocation-model=static", "-filetype=obj"])
                .arg(format!("-mtriple={arch}-unknown-linux-gnu"))
                .arg(dir().join(format!("{case}.ll")))
                .arg("-o")
                .arg(&out);
            let o = run_with(&mut command, b"", Duration::from_mins(1));
            assert!(o.success(), "hod {case} {arch}: {}", o.stderr_text());
            let made = std::fs::read(&out).unwrap();
            let fixture = std::fs::read(object(case, arch)).unwrap();
            assert!(made == fixture, "{case}.{arch}.o: hod {} makes other bytes", hod.display());
        }
    }
}

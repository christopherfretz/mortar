//! The input set (spec/plumb.md §3.8) and a shared object's versioned
//! undefined reference (resolve §9.2) on checked-in links
//! (`tests/inset_ver/gen.sh`): `libu.so` refers to `g@V1`, which only
//! `libv.so` (a closure file) defines as `g@@V1`; `libg.a(g.o)` defines
//! the plain `g`, which is not `first(g@V1)`: nothing extracts it (the
//! false M11 of batch 9: libstdc++.so's `__udivti3@GCC_3.0` and
//! libgcc.a(_udivdi3.o)). `libw.so` refers to `g` unversioned: an edge,
//! `g.o` is extracted, and a map without it is M11.

use mortar_plumb::anomaly::classify;
use mortar_plumb::{Ctx, Files, Format, Input, Map, MapFile, Report, Rule, map, verify_with};
use std::path::{Path, PathBuf};

const ARCHES: [&str; 2] = ["aarch64", "x86_64"];
const INPUTS: [&str; 3] = ["m.o", "g.o", "libg.a"];

struct Dir(PathBuf);

impl Files for Dir {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(path)).ok()
    }
}

fn dir(arch: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inset_ver").join(arch)
}

fn read_map(arch: &str, name: &str) -> Map {
    let format = if name.contains(".gnu") {
        Format::Gnu
    } else if name.contains(".lld") {
        Format::Lld
    } else if name.contains(".mold") {
        Format::Mold
    } else {
        Format::Mortar
    };
    let text = std::fs::read_to_string(dir(arch).join(name)).unwrap();
    map::parse(format, &text).unwrap()
}

/// The link line of `gen.sh` for output `p` with shared object `lib`.
fn line(arch: &str, p: &str, lib: &str, linker: &str) -> Vec<String> {
    let dl =
        if arch == "x86_64" { "/lib64/ld-linux-x86-64.so.2" } else { "/lib/ld-linux-aarch64.so.1" };
    let map = format!("-Map={p}.{linker}.map");
    let out = format!("{p}.{linker}");
    let emu = if arch == "x86_64" { "elf_x86_64" } else { "aarch64linux" };
    let m: &[&str] = if linker == "mold" { &["-m", emu] } else { &[] };
    m.iter()
        .chain(&["-dynamic-linker", dl, "m.o", lib, "libg.a", "-rpath-link", ".", "-o", &out, &map])
        .map(|s| (*s).to_string())
        .collect()
}

/// Verifies `output` against `m`; `line` for a reference's map.
fn verify(arch: &str, output: &str, m: &Map, line: Option<Vec<String>>) -> Report {
    let d = dir(arch);
    let bytes: Vec<(String, Vec<u8>)> =
        INPUTS.iter().map(|p| ((*p).to_string(), std::fs::read(d.join(p)).unwrap())).collect();
    let inputs: Vec<Input<'_>> = bytes.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
    let out = std::fs::read(d.join(output)).unwrap();
    let files = Dir(d);
    let ctx = Ctx { line, files: &files };
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

/// `m` without the file that is `libg.a(g.o)`, everything of it gone and
/// the later files renumbered.
fn without_g(mut m: Map) -> Map {
    let i = m.files.iter().position(|f| f.member.as_deref() == Some("g.o")).unwrap();
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
    m.included.retain(|x| x.file.member.as_deref() != Some("g.o"));
    m
}

/// The links as linked: clean, and every file judged (mold's versioned
/// link is A49's, below).
#[test]
fn s3_8_versioned_and_unversioned_shared_references_clean() {
    for arch in ARCHES {
        for (p, lib) in [("pv", "libu.so"), ("pw", "libw.so")] {
            for linker in ["mortar", "gnu", "lld", "mold"] {
                if (p, linker) == ("pv", "mold") {
                    continue;
                }
                let (out, m, l) = if linker == "mortar" {
                    (p.to_string(), format!("{p}.map"), None)
                } else {
                    (
                        format!("{p}.{linker}"),
                        format!("{p}.{linker}.map"),
                        Some(line(arch, p, lib, linker)),
                    )
                };
                let r = verify(arch, &out, &read_map(arch, &m), l);
                assert!(set(&r).is_empty(), "{arch} {out}: {:?}", set(&r));
                let c = coverage(&r);
                assert!(
                    c.starts_with("input set: files judged") && !c.contains("not judged"),
                    "{arch} {out}: {c}"
                );
            }
        }
    }
}

const A49: &str = "only a shared object's versioned reference reaches it, by its plain name (resolve §9.2: not an edge)";

/// The anomaly §10.2 names `f` on `out` linked by `format`'s linker.
fn named(arch: &str, out: &str, format: Format, r: &Report) -> Vec<Option<&'static str>> {
    let bytes = std::fs::read(dir(arch).join(out)).unwrap();
    r.findings
        .iter()
        .filter(|f| matches!(f.rule, Rule::Map(11 | 12)))
        .map(|f| classify(format, &bytes, f).map(|a| &a[..3]))
        .collect()
}

/// Both bounds. `g.o` added to the versioned link's map is M12: nothing
/// reaches it, `g@V1` is not `g`; only the plain name would, so the text
/// says so, and only on mold's output is that A49. mold's own versioned
/// link draws it; its maps without `g.o` are judged as GNU ld's (the
/// versioned one clean, the unversioned one M11, not excused). `g.o`
/// taken from the unversioned link's maps is M11 (`libw.so`'s `g` is an
/// edge to it).
#[test]
fn s3_8_versioned_and_unversioned_shared_references_mutated() {
    for arch in ARCHES {
        let mut m = read_map(arch, "pv.map");
        m.files.push(MapFile::member("libg.a", "g.o"));
        let r = verify(arch, "pv", &m, None);
        assert_eq!(set(&r), [format!("M12 libg.a(g.o) is in the link and {A49}")], "{arch}");
        assert_eq!(named(arch, "pv", Format::Mortar, &r), [None], "{arch}");
        for (format, linker) in [(Format::Gnu, "gnu"), (Format::Lld, "lld")] {
            let out = format!("pv.{linker}");
            let mut m = read_map(arch, &format!("pv.{linker}.map"));
            m.files.push(MapFile::member("libg.a", "g.o"));
            let r = verify(arch, &out, &m, Some(line(arch, "pv", "libu.so", linker)));
            // GNU ld's map also says the preamble does not include it.
            let want = format!("M12 libg.a(g.o) is in the link and {A49}");
            assert!(set(&r).contains(&want), "{arch} {out}: {:?}", set(&r));
            assert!(named(arch, &out, format, &r).iter().all(Option::is_none), "{arch} {out}");
        }
        let l = || Some(line(arch, "pv", "libu.so", "mold"));
        let r = verify(arch, "pv.mold", &read_map(arch, "pv.mold.map"), l());
        assert_eq!(set(&r), [format!("M12 libg.a(g.o) is in the link and {A49}")], "{arch}");
        assert_eq!(named(arch, "pv.mold", Format::Mold, &r), [Some("A49")], "{arch}");
        let r = verify(arch, "pv.mold", &without_g(read_map(arch, "pv.mold.map")), l());
        assert!(set(&r).is_empty(), "{arch}: {:?}", set(&r));
        // mortar's map is held to the closure exactly; a reference's map
        // to a reference that nothing in the link defines (§3.8 rule 2).
        let strict = "M11 libg.a(g.o) is not in the link: libw.so refers to `g` and it is the target (resolve §4.1)";
        let loose = "M11 `g` (libw.so) is defined by no file of the link, and libg.a(g.o) defines it (§3.8)";
        for (out, linker, format, want) in [
            ("pw", "mortar", Format::Mortar, strict),
            ("pw.gnu", "gnu", Format::Gnu, loose),
            ("pw.lld", "lld", Format::Lld, loose),
            ("pw.mold", "mold", Format::Mold, loose),
        ] {
            let map = if linker == "mortar" { "pw.map".to_string() } else { format!("{out}.map") };
            let l = (linker != "mortar").then(|| line(arch, "pw", "libw.so", linker));
            let r = verify(arch, out, &without_g(read_map(arch, &map)), l);
            assert_eq!(set(&r), [want], "{arch} {out}");
            assert_eq!(named(arch, out, format, &r), [None], "{arch} {out}");
        }
    }
}

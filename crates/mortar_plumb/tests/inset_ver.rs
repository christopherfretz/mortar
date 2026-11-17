//! The input set (spec/plumb.md §3.8) and a shared object's versioned
//! undefined reference (resolve §9.2) on checked-in links
//! (`tests/inset_ver/gen.sh`): `libu.so` refers to `g@V1`, which only
//! `libv.so` (a closure file) defines as `g@@V1`; `libg.a(g.o)` defines
//! the plain `g`, which is not `first(g@V1)`: nothing extracts it (the
//! false M11 of batch 9: libstdc++.so's `__udivti3@GCC_3.0` and
//! libgcc.a(_udivdi3.o)). `libw.so` refers to `g` unversioned: an edge,
//! `g.o` is extracted, and a map without it is M11.

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
    let map =
        if linker == "gnu" { format!("-Map={p}.gnu.map") } else { format!("-Map={p}.lld.map") };
    let out = format!("{p}.{linker}");
    ["-dynamic-linker", dl, "m.o", lib, "libg.a", "-rpath-link", ".", "-o", &out, &map]
        .iter()
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

/// The four links as linked: clean, and every file judged.
#[test]
fn s3_8_versioned_and_unversioned_shared_references_clean() {
    for arch in ARCHES {
        for (p, lib) in [("pv", "libu.so"), ("pw", "libw.so")] {
            for (out, m, l) in [
                (p.to_string(), format!("{p}.map"), None),
                (format!("{p}.gnu"), format!("{p}.gnu.map"), Some(line(arch, p, lib, "gnu"))),
                (format!("{p}.lld"), format!("{p}.lld.map"), Some(line(arch, p, lib, "lld"))),
            ] {
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

/// Both bounds: `g.o` added to the versioned link's map is M12 (nothing
/// reaches it: `g@V1` is not `g`); `g.o` taken from the unversioned
/// link's maps is M11 (`libw.so`'s `g` is an edge to it).
#[test]
fn s3_8_versioned_and_unversioned_shared_references_mutated() {
    for arch in ARCHES {
        let mut m = read_map(arch, "pv.map");
        m.files.push(MapFile::member("libg.a", "g.o"));
        let r = verify(arch, "pv", &m, None);
        assert_eq!(
            set(&r),
            [
                "M12 libg.a(g.o) is in the link and no root or live reference reaches it (resolve §4.1)"
            ],
            "{arch}"
        );
        // mortar's map is held to the closure exactly; a reference's map
        // to a reference that nothing in the link defines (§3.8 rule 2).
        let strict = "M11 libg.a(g.o) is not in the link: libw.so refers to `g` and it is the target (resolve §4.1)";
        let loose = "M11 `g` (libw.so) is defined by no file of the link, and libg.a(g.o) defines it (§3.8)";
        for (out, m, l, want) in [
            ("pw", "pw.map", None, strict),
            ("pw.gnu", "pw.gnu.map", Some(line(arch, "pw", "libw.so", "gnu")), loose),
            ("pw.lld", "pw.lld.map", Some(line(arch, "pw", "libw.so", "lld")), loose),
        ] {
            let r = verify(arch, out, &without_g(read_map(arch, m)), l);
            assert_eq!(set(&r), [want], "{arch} {out}");
        }
    }
}

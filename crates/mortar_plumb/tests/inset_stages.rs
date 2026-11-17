//! dyn.md D-12's stages in the input set (spec/plumb.md §3.8) and in N
//! (P-D6), on checked-in links (`tests/inset_stages/gen.sh`, WP-171): an
//! `--as-needed` command-line shared object's strong references are
//! edges exactly when it is a source of the last stage (in N by the
//! replay of D-11, and named by the output's `DT_NEEDED`); N only grows
//! (2a). Each line linked by mortar, GNU ld, lld and mold, AArch64 and
//! x86-64; the mutations of brief item 6 on each.

use mortar_plumb::anomaly::classify;
use mortar_plumb::{Ctx, Files, Format, Input, Map, MapFile, Report, Rule, map, verify_with};
use std::path::{Path, PathBuf};

const ARCHES: [&str; 2] = ["aarch64", "x86_64"];
const LINKERS: [&str; 4] = ["mortar", "gnu", "lld", "mold"];
const INPUTS: [&str; 9] =
    ["m0.o", "m1.o", "m1y.o", "g.o", "gy.o", "gv.o", "libg.a", "libgy.a", "libgv.a"];

struct Dir(PathBuf);

impl Files for Dir {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(path)).ok()
    }
}

fn dir(arch: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/inset_stages").join(arch)
}

fn format(linker: &str) -> Format {
    match linker {
        "gnu" => Format::Gnu,
        "lld" => Format::Lld,
        "mold" => Format::Mold,
        _ => Format::Mortar,
    }
}

/// The output and map of line `p` by `linker`.
fn names(p: &str, linker: &str) -> (String, String) {
    if linker == "mortar" {
        (p.to_string(), format!("{p}.map"))
    } else {
        (format!("{p}.{linker}"), format!("{p}.{linker}.map"))
    }
}

fn read_map(arch: &str, p: &str, linker: &str) -> Map {
    let text = std::fs::read_to_string(dir(arch).join(names(p, linker).1)).unwrap();
    map::parse(format(linker), &text).unwrap()
}

/// The words after `-dynamic-linker <dl>` of `gen.sh`'s line `p`.
fn rest(p: &str) -> &'static str {
    match p {
        "r1" => "m1.o --as-needed libu.so libg.a",
        "r1c" => "m0.o --as-needed libu.so libg.a",
        "k2a" => "m1y.o --as-needed libuy.so libgy.a",
        "j2b" => "m1.o --as-needed libu.so libgv.a libv.so",
        "vr" => "m1.o --as-needed libuv.so libg.a libgver.so",
        "nr" => "m0.o --no-as-needed libu.so libg.a",
        _ => unreachable!(),
    }
}

/// A reference's line of `gen.sh` for output `p`, the link words `rest`.
fn line(arch: &str, p: &str, rest: &str, linker: &str) -> Vec<String> {
    let dl =
        if arch == "x86_64" { "/lib64/ld-linux-x86-64.so.2" } else { "/lib/ld-linux-aarch64.so.1" };
    let emu = if arch == "x86_64" { "elf_x86_64" } else { "aarch64linux" };
    let (out, m) = names(p, linker);
    let mut w: Vec<String> = Vec::new();
    if linker == "mold" {
        w.extend(["-m".to_string(), emu.to_string()]);
    }
    w.extend(["-dynamic-linker".to_string(), dl.to_string()]);
    w.extend(rest.split(' ').map(str::to_string));
    w.extend(["-o".to_string(), out, format!("-Map={m}")]);
    w
}

/// Verifies `out` (bytes) against `m`; `line` for a reference's map.
fn verify_bytes(arch: &str, out: &[u8], m: &Map, line: Option<Vec<String>>) -> Report {
    let d = dir(arch);
    let bytes: Vec<(String, Vec<u8>)> =
        INPUTS.iter().map(|p| ((*p).to_string(), std::fs::read(d.join(p)).unwrap())).collect();
    let inputs: Vec<Input<'_>> = bytes.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
    let files = Dir(d);
    let ctx = Ctx { line, files: &files };
    verify_with(out, &inputs, m, &ctx)
}

fn output(arch: &str, p: &str, linker: &str) -> Vec<u8> {
    std::fs::read(dir(arch).join(names(p, linker).0)).unwrap()
}

/// Line `p` by `linker` as linked, or with `map`/`image` changed, judged
/// against the line `rest` (mortar: its map's `arg` records, with
/// `--no-as-needed` read as `--as-needed` when `rest` says so).
fn judge(
    arch: &str,
    p: &str,
    linker: &str,
    rest: &str,
    map: impl FnOnce(&mut Map),
    image: impl FnOnce(&mut Vec<u8>),
) -> (Report, Vec<u8>) {
    let mut m = read_map(arch, p, linker);
    map(&mut m);
    let mut out = output(arch, p, linker);
    image(&mut out);
    let l = if linker == "mortar" {
        if !rest.contains("--no-as-needed") {
            for w in &mut m.args {
                if w.as_slice() == b"--no-as-needed" {
                    *w = b"--as-needed".to_vec();
                }
            }
        }
        None
    } else {
        Some(line(arch, p, rest, linker))
    };
    (verify_bytes(arch, &out, &m, l), out)
}

/// The findings of the input set (M11, M12) and of P-D6 and P-D7,
/// `id message`.
fn found(r: &Report) -> Vec<String> {
    r.findings
        .iter()
        .filter(|f| matches!(f.rule, Rule::Map(11 | 12) | Rule::Dyn(6 | 7)))
        .map(|f| format!("{} {}", f.rule.id(), f.message))
        .collect()
}

/// The anomaly id of each finding of [`found`] on `out` by `linker`, or `-`.
fn named(linker: &str, out: &[u8], r: &Report) -> Vec<String> {
    r.findings
        .iter()
        .filter(|f| matches!(f.rule, Rule::Map(11 | 12) | Rule::Dyn(6 | 7)))
        .map(|f| classify(format(linker), out, f).map_or("-".to_string(), |a| a[..3].to_string()))
        .collect()
}

/// `m` without the member `member`, everything of it gone and the later
/// files renumbered.
fn without(m: &mut Map, member: &str) {
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
}

fn rd(b: &[u8], at: usize, n: usize) -> u64 {
    let mut v = [0u8; 8];
    v[..n].copy_from_slice(&b[at..at + n]);
    u64::from_le_bytes(v)
}

/// The `.dynamic` entries' offsets (ELF64, little-endian).
fn dynamic(b: &[u8]) -> Vec<usize> {
    let shoff = rd(b, 0x28, 8) as usize;
    let (size, num) = (rd(b, 0x3a, 2) as usize, rd(b, 0x3c, 2) as usize);
    for i in 0..num {
        let h = shoff + i * size;
        if rd(b, h + 4, 4) == 6 {
            let (off, len) = (rd(b, h + 0x18, 8) as usize, rd(b, h + 0x20, 8) as usize);
            return (off..off + len).step_by(16).collect();
        }
    }
    panic!("no .dynamic");
}

/// The image with its `DT_NEEDED` entries' order reversed.
fn needed_reversed(b: &mut Vec<u8>) {
    let n: Vec<usize> = dynamic(b).into_iter().filter(|&e| rd(b, e, 8) == 1).collect();
    let vals: Vec<u64> = n.iter().map(|&e| rd(b, e + 8, 8)).collect();
    for (&e, v) in n.iter().zip(vals.iter().rev()) {
        b[e + 8..e + 16].copy_from_slice(&v.to_le_bytes());
    }
}

/// The image without its first `DT_NEEDED` entry (the later entries move
/// up, a `DT_NULL` fills the end).
fn needed_dropped(b: &mut Vec<u8>) {
    let d = dynamic(b);
    let at = d.iter().position(|&e| rd(b, e, 8) == 1).unwrap();
    let last = *d.last().unwrap();
    for w in d[at..].windows(2) {
        let next: Vec<u8> = b[w[1]..w[1] + 16].to_vec();
        b[w[0]..w[0] + 16].copy_from_slice(&next);
    }
    b[last..last + 16].fill(0);
}

const UNNEEDED: &str = "only the references of an --as-needed shared object the output does not need reach it (dyn.md D-12: not an edge)";
const VPLAIN: &str = "only a shared object's versioned reference reaches it, by its plain name (resolve §9.2: not an edge)";
const KEPT: &str = "DT_NEEDED lacks 'libuy.so', which D-11 makes needed and D-12 keeps needed (row 2a: N only grows; a later stage's member defines what it gave)";

/// The links as linked: nothing of D-12's on mortar's, GNU ld's and
/// mold's outputs, and the input set judged; lld's 1c and 2a and mold's
/// 4b are the named anomalies, and nothing else.
#[test]
fn d12_stages_clean_and_named() {
    for arch in ARCHES {
        for p in ["r1", "r1c", "k2a", "j2b", "vr"] {
            for linker in LINKERS {
                let (r, out) = judge(arch, p, linker, rest(p), |_| {}, |_| {});
                let f = found(&r);
                let ids = named(linker, &out, &r);
                let who = format!("{arch} {p} {linker}");
                match (p, linker) {
                    ("r1c", "lld") => {
                        assert_eq!(
                            f,
                            [format!("M12 libg.a(g.o) is in the link and {UNNEEDED}")],
                            "{who}"
                        );
                        assert_eq!(ids, ["A70"], "{who}");
                    }
                    ("k2a", "lld") => {
                        assert_eq!(f, [format!("P-D6 {KEPT}")], "{who}");
                        assert_eq!(ids, ["A71"], "{who}");
                    }
                    ("vr", "mold") => {
                        assert_eq!(
                            f,
                            [format!("M12 libg.a(g.o) is in the link and {VPLAIN}")],
                            "{who}"
                        );
                        assert_eq!(ids, ["A49"], "{who}");
                    }
                    _ => assert!(f.is_empty(), "{who}: {f:?}"),
                }
                let c = r.coverage.input_set.clone().unwrap_or_default();
                assert!(
                    c.starts_with("input set: files judged") && !c.contains("not replayed"),
                    "{who}: {c}"
                );
            }
        }
    }
}

/// Mutation 1: the member a later stage extracts removed (row 1): M11 on
/// every linker's output, never named.
#[test]
fn d12_a_staged_member_removed_is_m11() {
    let strict = "M11 libg.a(g.o) is not in the link: libu.so refers to `g` and it is the target (resolve §4.1; a source of D-12's last stage)";
    let loose =
        "M11 `g` (libu.so) is defined by no file of the link, and libg.a(g.o) defines it (§3.8)";
    for arch in ARCHES {
        for linker in LINKERS {
            let (r, out) = judge(arch, "r1", linker, rest("r1"), |m| without(m, "g.o"), |_| {});
            let want = if linker == "mortar" { strict } else { loose };
            let who = format!("{arch} {linker}");
            assert!(found(&r).iter().any(|f| f == want), "{who}: {:?}", found(&r));
            for f in r.findings.iter().filter(|f| f.rule == Rule::Map(11)) {
                assert_eq!(classify(format(linker), &out, f), None, "{who}");
            }
        }
    }
}

/// Mutation 2: a member no edge reaches added, an unneeded as-needed
/// library referring to it (1c): M12 with the A70 text, named on lld's
/// output alone.
#[test]
fn d12_an_unneeded_library_s_member_added_is_m12() {
    for arch in ARCHES {
        for linker in ["mortar", "gnu", "mold"] {
            let add = |m: &mut Map| m.files.push(MapFile::member("libg.a", "g.o"));
            let (r, out) = judge(arch, "r1c", linker, rest("r1c"), add, |_| {});
            let want = format!("M12 libg.a(g.o) is in the link and {UNNEEDED}");
            let who = format!("{arch} {linker}");
            assert!(found(&r).contains(&want), "{who}: {:?} {:?}", found(&r), r.coverage.input_set);
            for f in r.findings.iter().filter(|f| f.rule == Rule::Map(12)) {
                assert_eq!(classify(format(linker), &out, f), None, "{who}");
            }
        }
    }
}

/// Mutation 3: the kept library's `DT_NEEDED` dropped (2a): P-D6 with the
/// kept text on every output, A71 only on lld's; on mortar's map gy.o is
/// then unreached too (libuy.so is no source), M12.
#[test]
fn d12_the_kept_library_dropped_is_p_d6() {
    for arch in ARCHES {
        for linker in ["mortar", "gnu", "mold"] {
            let (r, out) = judge(arch, "k2a", linker, rest("k2a"), |_| {}, needed_dropped);
            let who = format!("{arch} {linker}");
            let f = found(&r);
            assert!(f.contains(&format!("P-D6 {KEPT}")), "{who}: {f:?}");
            if linker == "mortar" {
                let m12 = format!("M12 libgy.a(gy.o) is in the link and {UNNEEDED}");
                assert!(f.contains(&m12), "{who}: {f:?}");
            }
            assert!(named(linker, &out, &r).iter().all(|a| a == "-"), "{who}");
        }
    }
}

/// Mutation 4: a `DT_NEEDED` nothing justifies. `nr` (libu.so linked
/// `--no-as-needed`, g.o extracted) judged as the line with
/// `--as-needed`: libu.so is not needed (m0.o refers to nothing of it),
/// so the output's entry is P-D6, and it licenses no stage: g.o is M12
/// (the output's `DT_NEEDED` is a precondition of a source, never a
/// reason).
#[test]
fn d12_an_unjustified_needed_entry_licenses_nothing() {
    for arch in ARCHES {
        for linker in LINKERS {
            let (r, out) = judge(arch, "nr", linker, rest("r1c"), |_| {}, |_| {});
            let who = format!("{arch} {linker}");
            let f = found(&r);
            let pd6 = "P-D6 DT_NEEDED names 'libu.so', which D-11 does not make needed";
            assert!(f.iter().any(|x| x.starts_with(pd6)), "{who}: {f:?}");
            let m12 = format!("M12 libg.a(g.o) is in the link and {UNNEEDED}");
            assert!(f.contains(&m12), "{who}: {f:?}");
            // g.o's own reference does not justify libu.so either.
            assert!(!f.iter().any(|x| x.contains("D-11 makes needed")), "{who}: {f:?}");
            let ids = named(linker, &out, &r);
            if linker != "lld" {
                assert!(ids.iter().all(|a| a == "-"), "{who}: {ids:?}");
            }
        }
    }
}

/// Mutation 5: a versioned reference's member added (4b): M12 with A49's
/// text, named on mold's output alone.
#[test]
fn d12_a_versioned_reference_s_member_added_is_m12() {
    for arch in ARCHES {
        for linker in ["mortar", "gnu", "lld"] {
            let add = |m: &mut Map| m.files.push(MapFile::member("libg.a", "g.o"));
            let (r, out) = judge(arch, "vr", linker, rest("vr"), add, |_| {});
            let who = format!("{arch} {linker}");
            let want = format!("M12 libg.a(g.o) is in the link and {VPLAIN}");
            assert!(found(&r).contains(&want), "{who}: {:?}", found(&r));
            for f in r.findings.iter().filter(|f| f.rule == Rule::Map(12)) {
                assert_eq!(classify(format(linker), &out, f), None, "{who}");
            }
        }
    }
}

/// Mutation 6: the late library misplaced (2b: libv.so joins N at stage
/// 2, after libu.so in D-19a's and in the first-mention order).
#[test]
fn d12_the_late_library_misplaced_is_p_d6() {
    for arch in ARCHES {
        for linker in LINKERS {
            let (r, out) = judge(arch, "j2b", linker, rest("j2b"), |_| {}, needed_reversed);
            let who = format!("{arch} {linker}");
            let rule = if matches!(linker, "mortar" | "gnu") {
                "D-19a"
            } else {
                "the first-mention order"
            };
            let want = format!(
                "P-D6 DT_NEEDED order is [\"libv.so\", \"libu.so\"]; {rule} gives [\"libu.so\", \"libv.so\"]"
            );
            assert!(found(&r).contains(&want), "{who}: {:?}", found(&r));
            assert!(named(linker, &out, &r).iter().all(|a| a == "-"), "{who}");
        }
    }
}

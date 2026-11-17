//! WP-147: the value of a symbol of an EMPTY `SHF_MERGE` section, which
//! mortar places whole (`spec/synth.md` S-6, `spec/resolve.md` §10.1
//! P-1) and, after a merged input, at the end of the merged chunk
//! (`spec/layout.md` L-162), on AArch64, x86-64 and i386.
//!
//! Under mortar's map the section has a `place` record of size 0, so it
//! is *whole* (`spec/plumb.md` §3.3): its labels and every reference to
//! them are judged by that one address (Y1/Y3, R1), not merely accepted
//! anywhere. mortar's outputs are checked in under `tests/emerge/<arch>/`
//! (`gen.sh`); each is clean, and `gms`, `wcs`, `gmid` moved by one byte
//! or to 0 in `.symtab`, or an address word of them moved by one, is a
//! finding. GNU ld and lld (linked at test time) write a row for the
//! empty section at the address its labels have: the same judgement
//! under their maps. (mold places it in an output section of its own,
//! `tests/link/resolve/place-empty-merge.test`'s skip.)
// ELF fields read best by their short names.
#![allow(clippy::many_single_char_names)]

use std::path::{Path, PathBuf};

use mortar_plumb::map::{self, Format, Map};
use mortar_plumb::{Input, Report};
use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, tool};

const ARCHES: [Arch; 3] = [Arch::Aarch64, Arch::X86_64, Arch::I386];

fn dir(arch: Arch) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/emerge").join(arch.name())
}

fn parse(d: &Path, map_file: &str, format: Format) -> Map {
    let text = std::fs::read_to_string(d.join(map_file)).unwrap();
    map::parse(format, &text).unwrap_or_else(|e| panic!("{map_file}: {}", e.message))
}

fn verify(d: &Path, out: &[u8], m: &Map) -> Report {
    let files: Vec<(String, Vec<u8>)> = m
        .paths()
        .into_iter()
        .filter_map(|p| std::fs::read(d.join(p)).ok().map(|b| (p.to_string(), b)))
        .collect();
    let inputs: Vec<Input<'_>> = files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
    mortar_plumb::verify(out, &inputs, m)
}

fn section(b: &[u8], name: &str) -> (usize, usize, u64) {
    let h = mortar_elf::FileHeader::parse(b).unwrap();
    let t = mortar_elf::SectionTable::parse(b, &h).unwrap();
    let s = t.iter().find(|s| s.name().is_ok_and(|n| n == name.as_bytes())).unwrap();
    let x = s.header();
    (x.sh_offset as usize, x.sh_size as usize, x.sh_addr)
}

/// The file offset and width of `name`'s value in `.symtab`.
fn sym_value_at(b: &[u8], arch: Arch, name: &str) -> (usize, usize) {
    let (so, ss, _) = section(b, ".strtab");
    let (o, s, _) = section(b, ".symtab");
    let (ent, at, w) = if arch == Arch::I386 { (16, 4, 4) } else { (24, 8, 8) };
    let want = format!("{name}\0");
    let e = (o..o + s)
        .step_by(ent)
        .find(|&e| {
            let n = u32::from_le_bytes(b[e..e + 4].try_into().unwrap()) as usize;
            b[so..so + ss].get(n..n + want.len()) == Some(want.as_bytes())
        })
        .unwrap_or_else(|| panic!("no {name}"));
    (e + at, w)
}

fn read(b: &[u8], at: usize, w: usize) -> u64 {
    let mut x = [0u8; 8];
    x[..w].copy_from_slice(&b[at..at + w]);
    u64::from_le_bytes(x)
}

fn write(b: &[u8], at: usize, w: usize, v: u64) -> Vec<u8> {
    let mut o = b.to_vec();
    o[at..at + w].copy_from_slice(&v.to_le_bytes()[..w]);
    o
}

fn keys(r: &Report) -> std::collections::BTreeSet<String> {
    r.findings.iter().map(|f| format!("{} {:?} {}", f.rule.id(), f.symbol, f.message)).collect()
}

/// A finding that names `name` and that the clean output does not have.
fn found(r: &Report, name: &str, clean: &std::collections::BTreeSet<String>) -> bool {
    r.findings
        .iter()
        .filter(|f| !clean.contains(&format!("{} {:?} {}", f.rule.id(), f.symbol, f.message)))
        .any(|f| f.symbol.as_deref() == Some(name) || f.message.contains(name))
}

/// The labels and address words of `out`, each moved by +1, -1 and to 0:
/// every one is a finding. `words`: (data symbol whose word refers to the
/// label, label).
fn mutations(d: &Path, out: &str, m: &Map, arch: Arch, labels: &[&str], words: &[(&str, &str)]) {
    let b = std::fs::read(d.join(out)).unwrap();
    let label = format!("{} {out}", arch.name());
    let clean = keys(&verify(d, &b, m));
    for name in labels {
        let (at, w) = sym_value_at(&b, arch, name);
        let v = read(&b, at, w);
        for nv in [v + 1, v - 1, 0] {
            let r = verify(d, &write(&b, at, w, nv), m);
            assert!(
                found(&r, name, &clean),
                "{label}: {name} at {nv:#x} not found:\n{}",
                r.render(out)
            );
        }
    }
    // An address word: the data symbol's value is its address in .data.
    let (doff, _, daddr) = section(&b, ".data");
    let w = if arch == Arch::I386 { 4 } else { 8 };
    for (holder, name) in words {
        let (at, sw) = sym_value_at(&b, arch, holder);
        let off = doff + (read(&b, at, sw) - daddr) as usize;
        let v = read(&b, off, w);
        for nv in [v + 1, v - 1, 0] {
            let r = verify(d, &write(&b, off, w, nv), m);
            assert!(
                r.findings.iter().any(|f| matches!(f.rule.id().as_str(), "R1" | "R5")
                    && !clean.contains(&format!("{} {:?} {}", f.rule.id(), f.symbol, f.message))),
                "{label}: the word of {holder} -> {name} at {nv:#x} not found:\n{}",
                r.render(out)
            );
        }
    }
}

const EM_LABELS: [&str; 2] = ["gms", "wcs"];
const EM_WORDS: [(&str, &str); 4] =
    [("gp", "gms"), ("wp", "wcs"), ("lp", "lms"), ("sp", "section")];

// S-6 and L-162 under mortar's map: clean, and each label or word moved by
// one byte or to 0 is a finding.
#[test]
fn wp147_mortars_empty_merge_labels_are_judged_by_value() {
    for arch in ARCHES {
        let d = dir(arch);
        for (out, labels, words) in
            [("em", &EM_LABELS[..], &EM_WORDS[..]), ("mid", &["gmid"][..], &[("mp", "gmid")][..])]
        {
            let m = parse(&d, &format!("{out}.map"), Format::Mortar);
            let b = std::fs::read(d.join(out)).unwrap();
            let r = verify(&d, &b, &m);
            assert!(r.findings.is_empty(), "{} {out}:\n{}", arch.name(), r.render(out));
            mutations(&d, out, &m, arch, labels, words);
        }
        // L-162: gmid at the end of gpre's string (7 bytes).
        let b = std::fs::read(d.join("mid")).unwrap();
        let (a, w) = sym_value_at(&b, arch, "gmid");
        let (p, _) = sym_value_at(&b, arch, "gpre");
        assert_eq!(read(&b, a, w), read(&b, p, w) + 7, "{}: L-162", arch.name());
    }
}

// The same links by GNU ld and lld: each writes a row for the empty
// section at its labels' address (whole, §3.3): clean, and the same
// mutations are findings.
#[test]
fn wp147_references_empty_merge_labels_are_judged_by_value() {
    for arch in ARCHES {
        let (Some(cc), Some(bfd), Some(lld)) =
            (arch.tool("gcc"), arch.tool("ld.bfd"), tool("ld.lld-22"))
        else {
            continue;
        };
        let scratch = Scratch::new("plumb-emerge");
        let d = scratch.path();
        for s in ["get", "em", "ew", "pre", "emid"] {
            std::fs::copy(dir(arch).join(format!("{s}.s")), d.join(format!("{s}.s"))).unwrap();
            let mut c = std::process::Command::new(&cc);
            c.current_dir(d).args(["-c", &format!("{s}.s"), "-o", &format!("{s}.o")]);
            assert!(
                mortar_testutil::run_with(&mut c, b"", mortar_testutil::DEFAULT_TIMEOUT).success()
            );
        }
        let em = match arch {
            Arch::Aarch64 => "aarch64linux",
            Arch::X86_64 => "elf_x86_64",
            Arch::I386 => "elf_i386",
        };
        for (ld, tag, format, extra) in
            [(&bfd, "bfd", Format::Gnu, vec![]), (&lld, "lld", Format::Lld, vec!["-m", em])]
        {
            for (out, objs, labels, words) in [
                ("em", &["get.o", "em.o", "ew.o"][..], &EM_LABELS[..], &EM_WORDS[..]),
                ("mid", &["get.o", "pre.o", "emid.o"][..], &["gmid"][..], &[("mp", "gmid")][..]),
            ] {
                let o = format!("{out}.{tag}");
                let mut c = std::process::Command::new(ld);
                c.current_dir(d).args(&extra).args(["-static", "-e", "get"]).args(objs);
                c.args(["-o", &o, &format!("-Map={o}.map")]);
                assert!(
                    mortar_testutil::run_with(&mut c, b"", mortar_testutil::DEFAULT_TIMEOUT)
                        .success()
                );
                let m = parse(d, &format!("{o}.map"), format);
                let b = std::fs::read(d.join(&o)).unwrap();
                let r = verify(d, &b, &m);
                // Recorded anomalies aside (A10: GNU ld names `.text` for
                // a label at `.data`'s start when it removes the empty
                // `.rodata`, x86-64 and i386).
                let other: Vec<_> = r
                    .findings
                    .iter()
                    .filter(|f| mortar_plumb::anomaly::classify(format, &[], f).is_none())
                    .collect();
                assert!(other.is_empty(), "{} {o}:\n{}", arch.name(), r.render(&o));
                mutations(d, &o, &m, arch, labels, words);
            }
        }
    }
}

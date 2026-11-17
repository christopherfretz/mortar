//! WP-147: definitions in sections with no place (`spec/resolve.md`
//! §10.1 P-1) in `.dynsym` and `.symtab`, on AArch64 and x86-64
//! (`spec/plumb.md` §12.5 P-D19, §6.5 Y4).
//!
//! `gprop` (`.note.gnu.property`, consumed), `gstk` (`.note.GNU-stack`),
//! `gex` (`SHF_EXCLUDE`) and `gbid` (an input build-id note) have no
//! place; `gplain` (`.data`) has one; none is referenced. P-1 omits an
//! unreferenced placeless symbol from `.dynsym` and `.symtab`.
//!
//! - mortar's outputs (checked in under `tests/pexport/<arch>/`, `gen.sh`)
//!   are clean; a placeless export present in `.dynsym` with a number
//!   (in any form, GNU ld's absolute one included) is a finding, and so is
//!   the placed `gplain` missing; with `-s` the same; a `.symtab` local of
//!   the consumed note (`lprop`, as mortar wrote it before WP-136: its
//!   offset in the note) is a finding;
//! - GNU ld, lld 22 and mold, linked at test time (`-shared` and `-pie
//!   --export-dynamic`), are clean under their own maps with what each
//!   writes (measured: GNU ld an absolute symbol whose value is the
//!   offset in its section, lld an undefined symbol under `-shared` and
//!   nothing under `-pie`, mold nothing); GNU ld's absolute form at
//!   another value is a finding in a library (an executable's export set
//!   is not P-D19's: `-pie --export-dynamic` is not judged).
//! - WP-147b: a key with a weak COMDAT copy in two files (`cka.s` kept,
//!   `ckb.s` named first by every map) is judged by its kept copy: GNU
//!   ld's, lld's and mold's libraries are clean (lld and mold write no
//!   line for the discarded copy, which is "absent"); with the kept copy's
//!   line removed from the map, every copy has no place and the export is
//!   a finding.
// ELF fields read best by their short names.
#![allow(clippy::many_single_char_names)]

use std::path::{Path, PathBuf};

use mortar_plumb::map::{self, Format, Map};
use mortar_plumb::{Ctx, Files, Input, Report};
use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, tool};

const ARCHES: [Arch; 2] = [Arch::Aarch64, Arch::X86_64];
const PLACELESS: [&str; 4] = ["gprop", "gstk", "gex", "gbid"];

fn dir(arch: Arch) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pexport").join(arch.name())
}

struct Dir<'a>(&'a Path);

impl Files for Dir<'_> {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(path)).ok()
    }
}

fn parse(d: &Path, map_file: &str, format: Format) -> Map {
    let text = std::fs::read_to_string(d.join(map_file)).unwrap();
    map::parse(format, &text).unwrap_or_else(|e| panic!("{map_file}: {}", e.message))
}

/// `out` judged under the map, with the line `words` (else the map's).
fn verify(d: &Path, out: &[u8], m: &Map, words: Option<Vec<String>>) -> Report {
    let files: Vec<(String, Vec<u8>)> = m
        .paths()
        .into_iter()
        .filter_map(|p| std::fs::read(d.join(p)).ok().map(|b| (p.to_string(), b)))
        .collect();
    let inputs: Vec<Input<'_>> = files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
    mortar_plumb::verify_with(out, &inputs, m, &Ctx { line: words, files: &Dir(d) })
}

fn has(r: &Report, id: &str, text: &str) -> bool {
    r.findings.iter().any(|f| f.rule.id() == id && f.message.contains(text))
}

/// Findings that name a placeless symbol.
fn about_placeless(r: &Report) -> Vec<String> {
    r.findings
        .iter()
        .filter(|f| {
            PLACELESS
                .iter()
                .any(|n| f.symbol.as_deref() == Some(*n) || f.message.contains(&format!("'{n}'")))
        })
        .map(|f| format!("{} {:?} {}", f.rule.id(), f.symbol, f.message))
        .collect()
}

/// The file range of section `name`.
fn section(b: &[u8], name: &str) -> Option<(usize, usize)> {
    let h = mortar_elf::FileHeader::parse(b).unwrap();
    let t = mortar_elf::SectionTable::parse(b, &h).unwrap();
    let s = t.iter().find(|s| s.name().is_ok_and(|n| n == name.as_bytes()))?;
    Some((s.header().sh_offset as usize, s.header().sh_size as usize))
}

/// The offset of the 64-bit symbol named `name` in table `tab` (names in
/// `strs`).
fn sym64(b: &[u8], tab: &str, strs: &str, name: &str) -> Option<usize> {
    let (so, ss) = section(b, strs)?;
    let st = &b[so..so + ss];
    let (o, s) = section(b, tab)?;
    let want = format!("{name}\0");
    (o..o + s).step_by(24).find(|&e| {
        let n = u32::from_le_bytes(b[e..e + 4].try_into().unwrap()) as usize;
        st.get(n..n + want.len()) == Some(want.as_bytes())
    })
}

/// `b` with `from` renamed `to` (not longer) in string table `strs`.
fn rename(b: &[u8], strs: &str, from: &str, to: &str) -> Vec<u8> {
    let mut o = b.to_vec();
    let (so, ss) = section(b, strs).unwrap();
    let pat = format!("\0{from}\0");
    let i = b[so..so + ss].windows(pat.len()).position(|w| w == pat.as_bytes()).unwrap();
    let at = so + i + 1;
    o[at..at + from.len()].fill(0);
    o[at..at + to.len()].copy_from_slice(to.as_bytes());
    o
}

/// `b` with the 64-bit symbol at `e` given section index `shndx` and
/// value `value`.
fn set_sym(b: &mut [u8], e: usize, shndx: u16, value: u64) {
    b[e + 6..e + 8].copy_from_slice(&shndx.to_le_bytes());
    b[e + 8..e + 16].copy_from_slice(&value.to_le_bytes());
}

// ---- mortar's outputs (checked in) ----

// P-1 and P-D19 (WP-147): mortar's libraries omit the four placeless
// exports and are clean (before WP-147, P-D19 expected them: "in the
// export set and not defined in .dynsym").
#[test]
fn wp147_mortars_library_without_placeless_exports_is_clean() {
    for arch in ARCHES {
        for lib in ["lib", "libs"] {
            let d = dir(arch);
            let m = parse(&d, &format!("{lib}.map"), Format::Mortar);
            let b = std::fs::read(d.join(format!("{lib}.so"))).unwrap();
            for n in PLACELESS {
                assert!(sym64(&b, ".dynsym", ".dynstr", n).is_none(), "{lib}: {n} exported");
            }
            let r = verify(&d, &b, &m, None);
            assert!(r.findings.is_empty(), "{} {lib}.so:\n{}", arch.name(), r.render(lib));
        }
    }
}

// Both directions under mortar's map, with and without `.symtab` (`-s`):
// a placeless export present in `.dynsym` (in mortar's form, and in GNU
// ld's absolute form at its offset) is a finding; the placed `gplain`
// missing is a finding.
#[test]
fn wp147_placeless_export_present_and_placed_export_missing_are_found() {
    for arch in ARCHES {
        for lib in ["lib", "libs"] {
            let label = format!("{} {lib}.so", arch.name());
            let d = dir(arch);
            let m = parse(&d, &format!("{lib}.map"), Format::Mortar);
            let b = std::fs::read(d.join(format!("{lib}.so"))).unwrap();
            // `gplain` renamed `gprop`: a placeless export with a number,
            // and the placed one missing.
            let mut bad = rename(&b, ".dynstr", "gplain", "gprop");
            let r = verify(&d, &bad, &m, None);
            assert!(
                has(&r, "P-D19", "'gprop' is defined in .dynsym (at 0x")
                    && has(&r, "P-D19", "but its definition has no place"),
                "{label}: placeless present:\n{}",
                r.render(lib)
            );
            assert!(
                has(&r, "P-D19", "'gplain' (Base) is in the export set and not defined in .dynsym"),
                "{label}: placed missing:\n{}",
                r.render(lib)
            );
            // GNU ld's form (absolute, the offset 0): still a finding under
            // mortar's map.
            let e = sym64(&bad, ".dynsym", ".dynstr", "gprop").unwrap();
            set_sym(&mut bad, e, mortar_elf::consts::SHN_ABS, 0);
            let r = verify(&d, &bad, &m, None);
            assert!(
                has(
                    &r,
                    "P-D19",
                    "'gprop' is defined in .dynsym (at 0x0), but its definition has no place"
                ),
                "{label}: GNU ld's form:\n{}",
                r.render(lib)
            );
        }
    }
}

// Y4 (WP-147): `lprop`, a local of the consumed note, is not in mortar's
// `.symtab`; written as its offset in the note (mortar before WP-136, and
// mold's form), in a section or absolute, it is a finding.
#[test]
fn wp147_local_of_a_placeless_section_in_symtab_is_found() {
    for arch in ARCHES {
        let label = arch.name();
        let d = dir(arch);
        let m = parse(&d, "loc.map", Format::Mortar);
        let b = std::fs::read(d.join("loc")).unwrap();
        assert!(sym64(&b, ".symtab", ".strtab", "lprop").is_none(), "{label}: lprop kept");
        let r = verify(&d, &b, &m, None);
        assert!(r.findings.is_empty(), "{label}:\n{}", r.render("loc"));
        let mut bad = rename(&b, ".strtab", "lpros", "lprop");
        let e = sym64(&bad, ".symtab", ".strtab", "lprop").unwrap();
        let shndx = u16::from_le_bytes(bad[e + 6..e + 8].try_into().unwrap());
        for (k, v) in [(shndx, 0xc), (mortar_elf::consts::SHN_ABS, 0xc)] {
            set_sym(&mut bad, e, k, v);
            let r = verify(&d, &bad, &m, None);
            assert!(
                has(
                    &r,
                    "Y4",
                    "local symbol 'lprop' is in the output, but every input local of its name is in a section with no place"
                ),
                "{label} shndx {k:#x}:\n{}",
                r.render("loc")
            );
        }
    }
}

// ---- the references ----

struct Tools {
    cc: PathBuf,
    bfd: PathBuf,
    lld: PathBuf,
    mold: PathBuf,
}

fn tools(arch: Arch) -> Option<Tools> {
    Some(Tools {
        cc: arch.tool("gcc")?,
        bfd: arch.tool("ld.bfd")?,
        lld: tool("ld.lld-22")?,
        mold: tool("mold")?,
    })
}

fn run(d: &Path, program: &Path, args: &[String]) {
    let mut c = std::process::Command::new(program);
    c.current_dir(d).args(args);
    let out = mortar_testutil::run_with(&mut c, b"", mortar_testutil::DEFAULT_TIMEOUT);
    assert!(out.success(), "{} {args:?} failed", program.display());
}

// What each reference writes for an unreferenced placeless export
// (measured, WP-147), judged clean under its own map: GNU ld an absolute
// symbol at the offset in its section, lld an undefined symbol under
// `-shared` and nothing under `-pie`, mold nothing. GNU ld's absolute
// form at another value is a finding.
#[test]
fn wp147_references_placeless_exports_are_judged_as_measured() {
    for arch in ARCHES {
        let Some(t) = tools(arch) else { continue };
        let scratch = Scratch::new("plumb-pexport");
        let d = scratch.path();
        for s in ["plain", "prop", "stk", "defs"] {
            std::fs::copy(dir(arch).join(format!("{s}.s")), d.join(format!("{s}.s"))).unwrap();
            run(d, &t.cc, &["-c".into(), format!("{s}.s"), "-o".into(), format!("{s}.o")]);
        }
        let em = if arch == Arch::Aarch64 { "aarch64linux" } else { "elf_x86_64" };
        for (mode, flags) in [
            ("shared", vec!["-shared"]),
            ("pie", vec!["-pie", "--export-dynamic", "-e", "f", "-dynamic-linker", "/lib/ld.so.1"]),
        ] {
            for (ld, tag, format) in [
                (&t.bfd, "bfd", Format::Gnu),
                (&t.lld, "lld", Format::Lld),
                (&t.mold, "mold", Format::Mold),
            ] {
                let out = format!("{mode}.{tag}");
                let mut words: Vec<String> = Vec::new();
                if tag != "bfd" {
                    words.extend(["-m".into(), em.into()]);
                }
                words.extend(flags.iter().map(|s| (*s).to_string()));
                words.extend(["plain.o", "prop.o", "stk.o", "defs.o", "-o"].map(String::from));
                words.push(out.clone());
                let mut args = words.clone();
                args.push(format!("-Map={out}.map"));
                run(d, ld, &args);
                let label = format!("{} {out}", arch.name());
                let b = std::fs::read(d.join(&out)).unwrap();
                let m = parse(d, &format!("{out}.map"), format);
                let r = verify(d, &b, &m, Some(words.clone()));
                assert!(
                    about_placeless(&r).is_empty(),
                    "{label}:\n{}",
                    about_placeless(&r).join("\n")
                );
                let e = sym64(&b, ".dynsym", ".dynstr", "gprop");
                match (tag, mode) {
                    ("bfd", _) => {
                        let e = e.expect("GNU ld exports gprop");
                        assert_eq!(&b[e + 6..e + 8], &mortar_elf::consts::SHN_ABS.to_le_bytes());
                        // An executable's export set is not P-D19's (§12.5
                        // is a `Shared` output's): not judged.
                        if mode == "pie" {
                            continue;
                        }
                        let mut bad = b.clone();
                        set_sym(&mut bad, e, mortar_elf::consts::SHN_ABS, 4);
                        let r = verify(d, &bad, &m, Some(words));
                        assert!(
                            has(&r, "P-D19", "'gprop' is defined in .dynsym (at 0x4)"),
                            "{label}: GNU ld's form at 4:\n{}",
                            r.render(&out)
                        );
                    }
                    ("lld", "shared") => {
                        let e = e.expect("lld imports gprop");
                        assert_eq!(&b[e + 6..e + 8], &[0, 0], "{label}: not undefined");
                    }
                    _ => assert!(e.is_none(), "{label}: gprop in .dynsym"),
                }
            }
        }
    }
}

// WP-147b: the definition of a key with several COMDAT copies is the kept
// one. `cka.o ckb.o`: every linker keeps cka's copies of `_Z4pickv` and
// of `_ZN3BoxC1Ev`/`C2Ev` (one group); ckb.o alone has a `.rodata`, so
// each map names it first. Before WP-147b the verifier took ckb's copy,
// which lld's and mold's maps do not name (absent: no place), as the
// definition and reported the exports. The kept copy's line removed from
// the map: every copy has no place, and each export is a finding.
#[test]
fn wp147b_comdat_export_is_judged_by_its_kept_copy() {
    const KEYS: [&str; 3] = ["_Z4pickv", "_ZN3BoxC1Ev", "_ZN3BoxC2Ev"];
    for arch in ARCHES {
        let Some(t) = tools(arch) else { continue };
        let scratch = Scratch::new("plumb-pexport-comdat");
        let d = scratch.path();
        for s in ["cka", "ckb"] {
            std::fs::copy(dir(arch).join(format!("{s}.s")), d.join(format!("{s}.s"))).unwrap();
            run(d, &t.cc, &["-c".into(), format!("{s}.s"), "-o".into(), format!("{s}.o")]);
        }
        let em = if arch == Arch::Aarch64 { "aarch64linux" } else { "elf_x86_64" };
        for (ld, tag, format) in [
            (&t.bfd, "bfd", Format::Gnu),
            (&t.lld, "lld", Format::Lld),
            (&t.mold, "mold", Format::Mold),
        ] {
            let out = format!("libck.{tag}.so");
            let mut words: Vec<String> = Vec::new();
            if tag != "bfd" {
                words.extend(["-m".into(), em.into()]);
            }
            words.extend(["-shared", "cka.o", "ckb.o", "-o"].map(String::from));
            words.push(out.clone());
            let mut args = words.clone();
            args.push(format!("-Map={out}.map"));
            run(d, ld, &args);
            let label = format!("{} {out}", arch.name());
            let b = std::fs::read(d.join(&out)).unwrap();
            for k in KEYS {
                assert!(sym64(&b, ".dynsym", ".dynstr", k).is_some(), "{label}: {k} not exported");
            }
            let text = std::fs::read_to_string(d.join(format!("{out}.map"))).unwrap();
            let m = map::parse(format, &text).unwrap_or_else(|e| panic!("{label}: {}", e.message));
            let r = verify(d, &b, &m, Some(words.clone()));
            let named: Vec<String> = r
                .findings
                .iter()
                .filter(|f| KEYS.iter().any(|k| f.symbol.as_deref() == Some(*k)))
                .map(|f| format!("{} {}", f.rule.id(), f.message))
                .collect();
            assert!(named.is_empty(), "{label}:\n{}", named.join("\n"));
            // The kept copy removed: lld and mold name cka.o's sections
            // on one line each (GNU ld's lines wrap; its discarded copies
            // are listed, so the shape does not arise under its map).
            if tag == "bfd" {
                continue;
            }
            let cut: String = text
                .lines()
                .filter(|l| {
                    !(l.contains("cka.o")
                        && (l.contains("(.text._Z4pickv)") || l.contains("(.text._ZN3BoxC2Ev)")))
                })
                .flat_map(|l| [l, "\n"])
                .collect();
            assert!(cut.len() < text.len(), "{label}: the kept copies' lines not found");
            let m = map::parse(format, &cut).unwrap_or_else(|e| panic!("{label}: {}", e.message));
            let r = verify(d, &b, &m, Some(words));
            for k in KEYS {
                assert!(
                    has(&r, "P-D19", &format!("'{k}' is defined in .dynsym (at 0x"))
                        && r.findings.iter().any(|f| f.symbol.as_deref() == Some(k)
                            && f.message.contains("but its definition has no place")),
                    "{label}: {k} with its kept copy removed:\n{}",
                    r.render(&out)
                );
            }
        }
    }
}

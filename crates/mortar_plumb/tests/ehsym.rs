//! WP-143b: a global label of an input `.eh_frame` (`spec/plumb.md` §6.5
//! Y3, `synth.md` S-48a and S-50), on AArch64, x86-64 and i386.
//!
//! `end.o`'s `.eh_frame` is a terminator with the global
//! `frame_end_after` at its end (offset 4 = `sh_size`). Three links:
//! - `end`:  `get.o end.o`, no live record: the output `.eh_frame` is the
//!   terminator alone (S-48a), the label its start (S-50 rule 3);
//! - `rec`:  `get.o getc.o end.o`: getc.o's CIE and FDE before it: the
//!   label is the final terminator;
//! - `rec2`: `get.o end.o getc.o`: the records after it: the label is
//!   getc.o's CIE, the next kept record in output order.
//!
//! mortar's outputs are checked in under `tests/ehsym/<arch>/` (`gen.sh`
//! regenerates them): they must stay clean, and each mutation (the label
//! moved out of `.eh_frame`, or to GNU ld's place, or the output without
//! an `.eh_frame`) is found. The same links by GNU ld, lld 22 and mold
//! are clean under their own maps, and a label moved out is found.

mod common;

use std::path::{Path, PathBuf};

use common::verify_with;
use mortar_plumb::map::{Format, Map};
use mortar_plumb::{Report, map};
use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, tool};

const ARCHES: [Arch; 3] = [Arch::Aarch64, Arch::X86_64, Arch::I386];
const SYM: &str = "frame_end_after";

fn dir(arch: Arch) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ehsym").join(arch.name())
}

/// The output, its inputs and its map read from `d`.
type Loaded = (Vec<u8>, Vec<(String, Vec<u8>)>, Map);

fn load(d: &Path, out: &str, map_file: &str, format: Format) -> Loaded {
    let output = std::fs::read(d.join(out)).unwrap();
    let text = std::fs::read_to_string(d.join(map_file)).unwrap();
    let m = map::parse(format, &text).unwrap_or_else(|e| panic!("{map_file}: {}", e.message));
    let files =
        m.paths().into_iter().map(|p| (p.to_string(), std::fs::read(d.join(p)).unwrap())).collect();
    (output, files, m)
}

/// Y3's findings about the label, R5 and M10.
fn ours(r: &Report) -> Vec<String> {
    let mut v: Vec<String> = ["R5", "M10"]
        .iter()
        .flat_map(|id| r.with_rule(id))
        .map(|f| format!("{} {:?} {}", f.rule.id(), f.symbol, f.message))
        .collect();
    v.extend(
        r.with_rule("Y3")
            .filter(|f| f.symbol.as_deref() == Some(SYM) || f.message.contains(SYM))
            .map(|f| format!("Y3 {:?} {}", f.symbol, f.message)),
    );
    v
}

/// A finding of rule `id` about `SYM`.
fn about(r: &Report, id: &str) -> bool {
    r.with_rule(id).any(|f| f.symbol.as_deref() == Some(SYM) || f.message.contains(SYM))
}

fn has(r: &Report, id: &str, text: &str) -> bool {
    r.with_rule(id).any(|f| f.message.contains(text))
}

/// The file range and address of output section `name`.
fn section(output: &[u8], name: &str) -> Option<(usize, u64, u64)> {
    let h = mortar_elf::FileHeader::parse(output).unwrap();
    let table = mortar_elf::SectionTable::parse(output, &h).unwrap();
    let s = table.iter().find(|s| s.name().is_ok_and(|n| n == name.as_bytes()))?;
    let hd = s.header();
    Some((hd.sh_offset as usize, hd.sh_size, hd.sh_addr))
}

/// The `.symtab` entry of `SYM`: its file offset and value's width.
fn sym_entry(output: &[u8], arch: Arch) -> Option<(usize, usize)> {
    let (str_at, str_len, _) = section(output, ".strtab")?;
    let strtab = &output[str_at..str_at + str_len as usize];
    let (off, size, _) = section(output, ".symtab")?;
    let (entsize, value_at, width) = if arch == Arch::I386 { (16, 4, 4) } else { (24, 8, 8) };
    let want = format!("{SYM}\0");
    (off..off + size as usize).step_by(entsize).find_map(|e| {
        let name = u32::from_le_bytes(output[e..e + 4].try_into().unwrap()) as usize;
        (strtab.get(name..name + want.len()) == Some(want.as_bytes()))
            .then_some((e + value_at, width))
    })
}

fn value(output: &[u8], arch: Arch) -> Option<u64> {
    let (at, w) = sym_entry(output, arch)?;
    let mut b = [0u8; 8];
    b[..w].copy_from_slice(&output[at..at + w]);
    Some(u64::from_le_bytes(b))
}

/// The output with `SYM`'s `st_value` set to `v`.
fn with_value(output: &[u8], arch: Arch, v: u64) -> Vec<u8> {
    let mut o = output.to_vec();
    let (at, w) = sym_entry(output, arch).expect("no label in .symtab");
    o[at..at + w].copy_from_slice(&v.to_le_bytes()[..w]);
    o
}

/// The output with its `.eh_frame` renamed `.eh_framx` in `.shstrtab`:
/// an output without an `.eh_frame`.
fn without_eh_frame(output: &[u8]) -> Vec<u8> {
    let mut o = output.to_vec();
    let (off, size, _) = section(output, ".shstrtab").unwrap();
    let t = &output[off..off + size as usize];
    let i = t.windows(11).position(|w| w == b"\0.eh_frame\0").unwrap();
    o[off + i + 9] = b'x';
    o
}

// ---- mortar's outputs (checked in) ----

// S-48a and S-50 rule 3: mortar's label is the next kept record, else the
// final terminator, and is clean (before WP-143b: WP-143's Y3 called the
// terminator-only input "not in the output" for `end`).
#[test]
fn wp143b_mortars_eh_frame_label_is_s50s_value_and_clean() {
    for arch in ARCHES {
        for out in ["end", "rec", "rec2"] {
            let label = format!("{}/{out}", arch.name());
            let (output, files, m) = load(&dir(arch), out, &format!("{out}.map"), Format::Mortar);
            let (_, size, addr) = section(&output, ".eh_frame").unwrap();
            let v = value(&output, arch).unwrap();
            match out {
                "end" => assert_eq!((size, v), (4, addr), "{label}"),
                "rec" => assert_eq!(v, addr + size - 4, "{label}"),
                _ => assert!(addr <= v && v < addr + size - 4, "{label}: {v:#x}"),
            }
            let r = verify_with(&output, &files, &m);
            assert!(ours(&r).is_empty(), "{label}:\n{}", ours(&r).join("\n"));
            assert!(r.coverage.failed == 0, "{label}:\n{}", r.render(out));
        }
    }
}

// The mutations under mortar's map: the label out of `.eh_frame`, at GNU
// ld's place (the end, after the terminator) where S-50 says otherwise,
// and an output with no `.eh_frame` (S-48a's "neither": the label is in a
// section not in the output).
#[test]
fn wp143b_mortars_eh_frame_label_mutations_are_found() {
    for arch in ARCHES {
        for out in ["end", "rec", "rec2"] {
            let label = format!("{}/{out}", arch.name());
            let (output, files, m) = load(&dir(arch), out, &format!("{out}.map"), Format::Mortar);
            let (_, size, addr) = section(&output, ".eh_frame").unwrap();
            for v in [addr + size + 8, addr + size, 0] {
                let r = verify_with(&with_value(&output, arch, v), &files, &m);
                assert!(
                    has(&r, "Y3", "not S-50's"),
                    "{label} at {v:#x}: not found:\n{}",
                    r.render(out)
                );
            }
            let r = verify_with(&without_eh_frame(&output), &files, &m);
            assert!(
                has(&r, "Y3", "every input definition is in a section not in it ('.eh_frame')"),
                "{label}: no .eh_frame: not found:\n{}",
                r.render(out)
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

fn emulation(arch: Arch) -> &'static str {
    match arch {
        Arch::Aarch64 => "aarch64linux",
        Arch::X86_64 => "elf_x86_64",
        Arch::I386 => "elf_i386",
    }
}

fn run(d: &Path, program: &Path, args: &[&str]) {
    let mut c = std::process::Command::new(program);
    c.current_dir(d).args(args);
    let out = mortar_testutil::run_with(&mut c, b"", mortar_testutil::DEFAULT_TIMEOUT);
    assert!(out.success(), "{} {args:?} failed", program.display());
}

// GNU ld and lld write the label at the end of the output `.eh_frame`
// (after its terminator), mold leaves it out; each is clean under its own
// map, and the label moved out of `.eh_frame` is found.
#[test]
#[allow(clippy::many_single_char_names)] // t, d, m, a, r, v
fn wp143b_references_are_clean_and_a_label_moved_out_is_found() {
    for arch in ARCHES {
        let Some(t) = tools(arch) else { continue };
        let scratch = Scratch::new("plumb-ehsym");
        let d = scratch.path();
        for s in ["get", "getc", "end"] {
            let src = std::fs::read_to_string(dir(arch).join(format!("{s}.s"))).unwrap();
            std::fs::write(d.join(format!("{s}.s")), src).unwrap();
            run(d, &t.cc, &["-c", &format!("{s}.s"), "-o", &format!("{s}.o")]);
        }
        let m = emulation(arch);
        for (name, objs) in [
            ("end", &["get.o", "end.o"][..]),
            ("rec", &["get.o", "getc.o", "end.o"][..]),
            ("rec2", &["get.o", "end.o", "getc.o"][..]),
        ] {
            for (ld, tag, format, extra) in [
                (&t.bfd, "bfd", Format::Gnu, vec![]),
                (&t.lld, "lld", Format::Lld, vec!["-m", m]),
                (&t.mold, "mold", Format::Mold, vec!["-m", m]),
            ] {
                let out = format!("{name}.{tag}");
                let mapf = format!("{out}.map");
                let mut args: Vec<String> = extra.iter().map(|s| (*s).to_string()).collect();
                args.extend(["-static".into(), "-e".into(), "get".into()]);
                args.extend(objs.iter().map(|s| (*s).to_string()));
                args.extend(["-o".into(), out.clone(), format!("-Map={mapf}")]);
                let a: Vec<&str> = args.iter().map(String::as_str).collect();
                run(d, ld, &a);
                let label = format!("{}/{out}", arch.name());
                let (output, files, mp) = load(d, &out, &mapf, format);
                let (_, size, addr) = section(&output, ".eh_frame").unwrap();
                let v = value(&output, arch);
                // Measured: GNU ld writes the label at end.o's input
                // section's place (edited to size 0: the end for `end`
                // and `rec`, offset 4 for `rec2`, whose terminator is
                // gone), lld the section's start + 4 (the input offset
                // from the output's start), mold leaves it out. All are
                // in or at the end of the output `.eh_frame`.
                match (tag, v) {
                    ("mold", v) => assert_eq!(v, None, "{label}: mold now writes the label"),
                    (_, Some(v)) => {
                        assert!(addr <= v && v <= addr + size, "{label}: {v:#x} outside");
                        if name == "end" {
                            assert_eq!(v, addr + size, "{label}: not at the end");
                        }
                    }
                    (_, None) => panic!("{label}: no label"),
                }
                let r = verify_with(&output, &files, &mp);
                assert!(ours(&r).is_empty(), "{label}:\n{}", ours(&r).join("\n"));
                assert!(!about(&r, "Y1"), "{label}:\n{}", r.render(&out));
                if v.is_some() {
                    let r = verify_with(&with_value(&output, arch, addr + size + 8), &files, &mp);
                    // Where GNU ld's map places end.o's `.eh_frame`
                    // (edited to size 0), Y1 judges the label by that
                    // row; elsewhere (lld's map, GNU ld's `rec2`), the
                    // Y3 row of WP-143b.
                    assert!(
                        about(&r, "Y1")
                            || has(&r, "Y3", "neither in nor at the end of the output's .eh_frame"),
                        "{label}: moved out: not found:\n{}",
                        r.render(&out)
                    );
                }
            }
        }
    }
}

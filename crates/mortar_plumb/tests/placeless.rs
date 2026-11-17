//! WP-143: a relocation against a defined symbol whose section has no
//! place (`spec/plumb.md` §4.4 step 0, §5.7 R5, §3.3 M4 and M10, §6.5
//! Y3), on AArch64, x86-64 and i386.
//!
//! Two kinds of subject, neither depending on which mortar is built:
//! - the two mislinks WP-136 found, as mortar wrote them before its fix,
//!   checked in under `tests/placeless/<arch>/` (`gen.sh` regenerates
//!   them): they must stay found whatever a later mortar does;
//! - case 1's inputs linked here by GNU ld, lld 22 and mold, which write
//!   the right address: the outputs stay clean of this package's rules,
//!   under their own maps and under lld's map rewritten as mortar's; and
//!   each mutation (a word zeroed, the symbol renamed or moved, a record
//!   removed or given a reason the line does not have) is found.

mod common;

use std::path::{Path, PathBuf};

use common::verify_with;
use mortar_plumb::map::{
    DiscardReason, Discarded, DynRecords, Format, Map, OutputKind, Placed, SectionRef,
};
use mortar_plumb::{Report, map};
use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, tool};

const ARCHES: [Arch; 3] = [Arch::Aarch64, Arch::X86_64, Arch::I386];

fn dir(arch: Arch) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/placeless").join(arch.name())
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

/// This package's findings: the relocated fields (R1 to R7), the place
/// rules M4 and M10, and Y3's findings about a symbol whose section has
/// no place, is empty, or is missing.
fn ours(r: &Report) -> Vec<String> {
    let mut v: Vec<String> = ["R1", "R4", "R5", "R6", "R7", "M4", "M10"]
        .iter()
        .flat_map(|id| r.with_rule(id))
        .map(|f| format!("{} {:?} {}", f.rule.id(), f.symbol, f.message))
        .collect();
    v.extend(
        r.with_rule("Y3")
            .filter(|f| {
                f.message.contains("is not in the output's .symtab")
                    || f.message.contains("neither in nor at the end")
                    || f.message.contains("every input definition is in a section not in it")
            })
            .map(|f| format!("Y3 {:?} {}", f.symbol, f.message)),
    );
    v
}

fn has(r: &Report, id: &str, text: &str) -> bool {
    r.with_rule(id).any(|f| f.message.contains(text))
}

// ---- the two mislinks, as mortar wrote them (regressions) ----

// WP-136 case 1: `gms` at offset 0 of an empty SHF_MERGE|SHF_STRINGS
// section; mortar wrote 0 in `.quad gms` and left `gms` out of `.symtab`.
// Before WP-143 the verifier found nothing (§4.4 ended in
// `symbol-unresolved`, a count).
#[test]
fn wp143_empty_merged_global_written_as_zero_is_found() {
    for arch in ARCHES {
        let (output, files, m) = load(&dir(arch), "gms", "gms.map", Format::Mortar);
        let r = verify_with(&output, &files, &m);
        let label = arch.name();
        assert!(
            has(&r, "R5", "empty section '.rodata.str1.1' at 0x0"),
            "{label}: the zero word is not found:\n{}",
            r.render("gms")
        );
        assert!(
            has(&r, "Y3", "is not in the output's .symtab"),
            "{label}: the missing symbol is not found:\n{}",
            r.render("gms")
        );
    }
}

// WP-136 case 2: `gprop` in an input `.note.gnu.property`, which the link
// consumes into its own note; mortar wrote 0 in `.quad gprop`. GNU ld,
// lld and mold refuse the reference.
#[test]
fn wp143_global_in_consumed_property_note_is_found() {
    for arch in ARCHES {
        let (output, files, m) = load(&dir(arch), "gprop", "gprop.map", Format::Mortar);
        let r = verify_with(&output, &files, &m);
        assert!(
            has(&r, "R5", "refers to section '.note.gnu.property'"),
            "{}: the reference into the consumed note is not found:\n{}",
            arch.name(),
            r.render("gprop")
        );
        // The note has a reason to be without a place (resolve.md §10).
        assert!(r.with_rule("M10").next().is_none(), "{}:\n{}", arch.name(), r.render("gprop"));
    }
}

// ---- reference links, and their mutations ----

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

/// Assembles the sources of `arch` into `scratch` and links `objs` with
/// each reference linker, writing `<name>.<linker>` and its map.
fn link_all(t: &Tools, arch: Arch, d: &Path, name: &str, objs: &[&str]) {
    for s in ["get", "emptymerge", "r-gms", "gsec", "r-gsec"] {
        let src = std::fs::read_to_string(dir(arch).join(format!("{s}.s"))).unwrap();
        std::fs::write(d.join(format!("{s}.s")), src).unwrap();
        if !d.join(format!("{s}.o")).exists() {
            run(d, &t.cc, &["-c", &format!("{s}.s"), "-o", &format!("{s}.o")]);
        }
    }
    let m = emulation(arch);
    let mut common: Vec<String> = vec!["-static".into(), "-e".into(), "get".into()];
    common.extend(objs.iter().map(|s| (*s).to_string()));
    for (ld, tag, extra) in
        [(&t.bfd, "bfd", vec![]), (&t.lld, "lld", vec!["-m", m]), (&t.mold, "mold", vec!["-m", m])]
    {
        let mut args: Vec<String> = extra.iter().map(|s| (*s).to_string()).collect();
        args.extend(common.iter().cloned());
        args.extend(["-o".into(), format!("{name}.{tag}"), format!("-Map={name}.{tag}.map")]);
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        run(d, ld, &a);
    }
}

/// lld's map of `out` rewritten as mortar's: no row for an empty merged
/// section (mortar's map has none: no piece, I-78), and a line.
fn as_mortar(d: &Path, out: &str, args: &[&str]) -> Map {
    let text = std::fs::read_to_string(d.join(format!("{out}.map"))).unwrap();
    let mut m = map::parse(Format::Lld, &text).unwrap();
    m.format = Format::Mortar;
    let em = m.files.iter().position(|f| f.path == "emptymerge.o").map(|i| i as u32);
    m.placed.retain(|p| {
        !(Some(p.file) == em && p.section == SectionRef::Name(".rodata.str1.1".into()))
    });
    m.args = args.iter().map(|a| a.as_bytes().to_vec()).collect();
    let text = map::mortar::write(&m);
    map::parse(Format::Mortar, &text).unwrap()
}

fn file_of(m: &Map, path: &str) -> u32 {
    m.files.iter().position(|f| f.path == path).unwrap() as u32
}

/// The file offset of the first byte of output section `name`.
fn section_offset(output: &[u8], name: &str) -> (usize, u64) {
    let h = mortar_elf::FileHeader::parse(output).unwrap();
    let table = mortar_elf::SectionTable::parse(output, &h).unwrap();
    let s = table.iter().find(|s| s.name().is_ok_and(|n| n == name.as_bytes())).unwrap();
    (s.header().sh_offset as usize, s.header().sh_size)
}

/// The output with the address word at the start of `.data` (`dref`,
/// r-gms.o's word) zeroed.
fn zero_word(output: &[u8], arch: Arch) -> Vec<u8> {
    let mut o = output.to_vec();
    let (off, _) = section_offset(output, ".data");
    let w = if arch == Arch::I386 { 4 } else { 8 };
    o[off..off + w].fill(0);
    o
}

/// The output with `gms` renamed `hms` in `.strtab` (the symbol gone).
fn rename_gms(output: &[u8]) -> Vec<u8> {
    let mut o = output.to_vec();
    let (off, size) = section_offset(output, ".strtab");
    let t = &output[off..off + size as usize];
    let i = t.windows(5).position(|w| w == b"\0gms\0").unwrap();
    o[off + i + 1] = b'h';
    o
}

/// The output with `gms`'s `st_value` set to 0 in `.symtab`.
fn zero_gms(output: &[u8], arch: Arch) -> Vec<u8> {
    let mut o = output.to_vec();
    let (str_at, str_len) = section_offset(output, ".strtab");
    let strtab = &output[str_at..str_at + str_len as usize];
    let (off, size) = section_offset(output, ".symtab");
    let (entsize, value_at, width) = if arch == Arch::I386 { (16, 4, 4) } else { (24, 8, 8) };
    for e in (off..off + size as usize).step_by(entsize) {
        let name = u32::from_le_bytes(output[e..e + 4].try_into().unwrap()) as usize;
        if strtab.get(name..name + 4) == Some(b"gms\0") {
            o[e + value_at..e + value_at + width].fill(0);
            return o;
        }
    }
    panic!("no gms in .symtab");
}

// spec/plumb.md §4.4 step 0, §3.3 M4, §6.5 Y3 (WP-143): the reference
// outputs of case 1 hold `gms`'s address (an empty section's label is
// where its bytes would be: GNU ld at the end of `.text`, lld and mold in
// an empty output section) and stay clean; zeroing the word is found.
#[test]
fn wp143_references_of_case_1_are_clean_and_a_zeroed_word_is_found() {
    for arch in ARCHES {
        let Some(t) = tools(arch) else { continue };
        let scratch = Scratch::new("plumb-placeless");
        let d = scratch.path();
        link_all(&t, arch, d, "gms", &["get.o", "emptymerge.o", "r-gms.o"]);
        for (tag, format) in [("bfd", Format::Gnu), ("lld", Format::Lld), ("mold", Format::Mold)] {
            let out = format!("gms.{tag}");
            let label = format!("{}/{out}", arch.name());
            let (output, files, m) = load(d, &out, &format!("{out}.map"), format);
            let r = verify_with(&output, &files, &m);
            assert!(ours(&r).is_empty(), "{label}:\n{}", ours(&r).join("\n"));
            let r = verify_with(&zero_word(&output, arch), &files, &m);
            assert!(
                r.coverage.failed > 0,
                "{label}: the zeroed word is not found:\n{}",
                r.render("o")
            );
        }
    }
}

// The same under mortar's map (lld's rewritten): clean; then mortar's
// mislink made of it (the word zeroed, `gms` out of `.symtab`), `gms` at
// 0, and an empty section's record at 0.
#[test]
fn wp143_case_1_under_mortars_map_mutations_are_found() {
    for arch in ARCHES {
        let Some(t) = tools(arch) else { continue };
        let scratch = Scratch::new("plumb-placeless");
        let d = scratch.path();
        link_all(&t, arch, d, "gms", &["get.o", "emptymerge.o", "r-gms.o"]);
        let label = arch.name();
        let line = ["-static", "get.o", "emptymerge.o", "r-gms.o", "-e", "get", "-o", "gms"];
        let m = as_mortar(d, "gms.lld", &line);
        let (output, files, _) = load(d, "gms.lld", "gms.lld.map", Format::Lld);
        let r = verify_with(&output, &files, &m);
        assert!(ours(&r).is_empty(), "{label}: clean:\n{}", ours(&r).join("\n"));

        // mortar's mislink: the word 0, the symbol gone.
        let bad = zero_word(&rename_gms(&output), arch);
        let r = verify_with(&bad, &files, &m);
        assert!(
            has(&r, "R5", "empty section '.rodata.str1.1' at 0x0"),
            "{label}:\n{}",
            r.render("o")
        );
        assert!(has(&r, "Y3", "is not in the output's .symtab"), "{label}:\n{}", r.render("o"));

        // `gms` at 0 in `.symtab`: a label of an empty section outside
        // every allocated section.
        let r = verify_with(&zero_gms(&output, arch), &files, &m);
        assert!(has(&r, "Y3", "neither in nor at the end"), "{label}:\n{}", r.render("o"));

        // A record placing the empty section at 0 (M4).
        let mut m0 = m.clone();
        m0.placed.push(Placed {
            file: file_of(&m, "emptymerge.o"),
            section: SectionRef::Name(".rodata.str1.1".into()),
            input_offset: None,
            addr: 0,
            size: 0,
            line: 0,
        });
        let m0 = map::parse(Format::Mortar, &map::mortar::write(&m0)).unwrap();
        let r = verify_with(&output, &files, &m0);
        assert!(has(&r, "M4", "an empty section placed at 0x0"), "{label}:\n{}", r.render("o"));
    }
}

// §3.3 M10 and §4.4 step 0 with a section that has bytes: `gx` in
// `.rodata.gx`, referenced by `.quad gx`. Under mortar's map (lld's
// rewritten) the link is clean; with the section discarded, the
// reference is R5 whatever the reason, and a reason the inputs and the
// line do not give is M10.
#[test]
#[allow(clippy::many_single_char_names)] // t, d, m, f, r
fn wp143_discarded_section_with_a_global_and_its_reasons() {
    for arch in ARCHES {
        let Some(t) = tools(arch) else { continue };
        let scratch = Scratch::new("plumb-placeless");
        let d = scratch.path();
        link_all(&t, arch, d, "gsec", &["get.o", "gsec.o", "r-gsec.o"]);
        let label = arch.name();
        let line = ["-static", "get.o", "gsec.o", "r-gsec.o", "-e", "get", "-o", "gsec"];
        let m = as_mortar(d, "gsec.lld", &line);
        let (output, files, _) = load(d, "gsec.lld", "gsec.lld.map", Format::Lld);
        let r = verify_with(&output, &files, &m);
        assert!(ours(&r).is_empty(), "{label}: clean:\n{}", ours(&r).join("\n"));

        let gx = SectionRef::Name(".rodata.gx".into());
        let f = file_of(&m, "gsec.o");
        let discard = |reason: DiscardReason, gc: bool| {
            let mut m2 = m.clone();
            m2.placed.retain(|p| !(p.file == f && p.section == gx));
            m2.discarded.push(Discarded { file: f, section: gx.clone(), reason, line: 0 });
            if gc {
                m2.args.push(b"--gc-sections".to_vec());
            }
            map::parse(Format::Mortar, &map::mortar::write(&m2)).unwrap()
        };
        // gc with --gc-sections: a reason; the reference is R5 (before
        // WP-143, `symbol-unresolved`: nothing).
        let r = verify_with(&output, &files, &discard(DiscardReason::Gc, true));
        assert!(has(&r, "R5", "refers to section '.rodata.gx'"), "{label}:\n{}", r.render("o"));
        assert!(r.with_rule("M10").next().is_none(), "{label}:\n{}", r.render("o"));
        // Reasons the line or the inputs do not give.
        for (reason, gc, text) in [
            (DiscardReason::Gc, false, "no --gc-sections"),
            (DiscardReason::Comdat, false, "in no COMDAT group"),
            (DiscardReason::Unspecified, false, "no row of resolve.md §10"),
            (DiscardReason::Script, false, "no -T"),
        ] {
            let r = verify_with(&output, &files, &discard(reason, gc));
            assert!(has(&r, "R5", "'.rodata.gx'"), "{label} {reason:?}:\n{}", r.render("o"));
            assert!(has(&r, "M10", text), "{label} {reason:?}:\n{}", r.render("o"));
        }
        // No record at all for a section with bytes: M10.
        let mut m3 = m.clone();
        m3.placed.retain(|p| !(p.file == f && p.section == gx));
        let m3 = map::parse(Format::Mortar, &map::mortar::write(&m3)).unwrap();
        let r = verify_with(&output, &files, &m3);
        assert!(has(&r, "M10", "neither places nor discards"), "{label}:\n{}", r.render("o"));
        // The output defining `gx` though its section is not in it (Y3).
        assert!(
            has(&r, "Y3", "every input definition is in a section not in it"),
            "{label}:\n{}",
            r.render("o")
        );
    }
}

// §3.3 M10 (WP-147): a `discard gc` of a section that defines a key the
// line roots (`spec/resolve.md` §4.2, §4.7): the entry (`-e get`), a name
// of `-init`, or, under `-shared` or `-E`, an export. Without a root
// name the same record is accepted (the test above).
#[test]
#[allow(clippy::many_single_char_names)] // t, d, m, f, r
fn wp147_gc_of_a_line_root_is_m10() {
    for arch in ARCHES {
        let Some(t) = tools(arch) else { continue };
        let scratch = Scratch::new("plumb-placeless");
        let d = scratch.path();
        link_all(&t, arch, d, "gsec", &["get.o", "gsec.o", "r-gsec.o"]);
        let label = arch.name();
        let line = ["-static", "get.o", "gsec.o", "r-gsec.o", "-e", "get", "-o", "gsec"];
        let m = as_mortar(d, "gsec.lld", &line);
        let (output, files, _) = load(d, "gsec.lld", "gsec.lld.map", Format::Lld);
        let gc = |file: &str, sec: &str, extra: &[&str]| {
            let f = file_of(&m, file);
            let s = SectionRef::Name(sec.into());
            let mut m2 = m.clone();
            m2.placed.retain(|p| !(p.file == f && p.section == s));
            m2.discarded.push(Discarded {
                file: f,
                section: s,
                reason: DiscardReason::Gc,
                line: 0,
            });
            m2.args.push(b"--gc-sections".to_vec());
            m2.args.extend(extra.iter().map(|w| w.as_bytes().to_vec()));
            map::parse(Format::Mortar, &map::mortar::write(&m2)).unwrap()
        };
        // The entry's section.
        let r = verify_with(&output, &files, &gc("get.o", ".text", &[]));
        assert!(
            has(&r, "M10", "defines 'get', which the line names"),
            "{label} entry:\n{}",
            r.render("o")
        );
        // `gx` named by -init.
        let r = verify_with(&output, &files, &gc("gsec.o", ".rodata.gx", &["-init", "gx"]));
        assert!(
            has(&r, "M10", "defines 'gx', which the line names"),
            "{label} -init:\n{}",
            r.render("o")
        );
        // `gx` exported by a library (the map's `output shared`); under
        // `-E` in a static link nothing is exported, so not a root.
        let mut ms = gc("gsec.o", ".rodata.gx", &["-shared"]);
        ms.dynamic = Some(DynRecords { kind: Some(OutputKind::Shared), ..DynRecords::default() });
        let r = verify_with(&output, &files, &ms);
        assert!(has(&r, "M10", "defines 'gx', an export"), "{label} -shared:\n{}", r.render("o"));
        let r = verify_with(&output, &files, &gc("gsec.o", ".rodata.gx", &["-E"]));
        assert!(r.with_rule("M10").next().is_none(), "{label} static -E:\n{}", r.render("o"));
        // No root: accepted (R5 for the reference, as above).
        let r = verify_with(&output, &files, &gc("gsec.o", ".rodata.gx", &[]));
        assert!(r.with_rule("M10").next().is_none(), "{label} no root:\n{}", r.render("o"));
    }
}

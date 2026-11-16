//! The allocation records of mortar's map (`spec/plumb.md` §3.2, §3.3
//! M6-M8; WP-096): on a link whose map states them as the output holds
//! them, no M6, M7 or M8 finding (ACCEPTED); each field of each record
//! changed, REJECTED by its rule.
//!
//! The link is lld's (static, no C library): it makes a `NOBITS`
//! `.relro_padding` (the shape of mortar's `.pad.relro`) and lists it as
//! a synthetic chunk, and it allocates four common symbols, one of them
//! asked 32-aligned by `a.o` and 64-aligned by `b.o`. Its map, converted
//! to mortar's format, gains the `common` records a mortar map states.
//! GNU ld's and mold's links of the same objects show that the foreign
//! readers do not change: their maps have no allocation record and are
//! judged by no M6-M8 rule (§9.3).

use std::path::Path;

use mortar_plumb::map::{ChunkKind, CommonAlloc, CopyHint, DynRecords, Map};
use mortar_plumb::{Format, Input};
use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, run_with};

/// The common symbols of the program and the alignment their
/// definitions ask (the largest of `a.o`'s and `b.o`'s).
const COMMONS: [(&str, u64); 4] = [("ca", 4), ("cb", 64), ("cc", 8), ("cd", 1)];

fn sources(arch: Arch) -> [(&'static str, String); 2] {
    let (halt, word, sec) = match arch {
        Arch::Aarch64 => ("b .", ".quad", "%progbits"),
        Arch::X86_64 => ("jmp .", ".quad", "@progbits"),
        Arch::I386 => ("jmp .", ".long", "@progbits"),
    };
    let a = format!(
        "\t.text\n\t.globl _start\n_start:\n\t{halt}\n\
         \t.section .data.rel.ro,\"aw\",{sec}\n\t.balign 8\n\t{word} _start\n\
         \t.data\n\t.globl dat\ndat:\t{word} 1\n\
         \t.comm ca,4,4\n\t.comm cb,800,32\n\t.comm cc,24,8\n"
    );
    let b = "\t.comm cb,800,64\n\t.comm cd,1,1\n".to_string();
    [("a", a), ("b", b)]
}

fn emulation(arch: Arch) -> &'static str {
    match arch {
        Arch::Aarch64 => "aarch64linux",
        Arch::X86_64 => "elf_x86_64",
        Arch::I386 => "elf_i386",
    }
}

fn run(dir: &Path, program: &Path, args: &[&str]) {
    let mut c = std::process::Command::new(program);
    c.current_dir(dir).args(args);
    let out = run_with(&mut c, b"", mortar_testutil::DEFAULT_TIMEOUT);
    assert!(
        out.success(),
        "{} {} failed in {}: {}",
        program.display(),
        args.join(" "),
        dir.display(),
        out.stderr_text()
    );
}

/// `(st_value, st_size)` of the `.symtab` symbol `name`.
fn symbol(bytes: &[u8], name: &str) -> (u64, u64) {
    let h = mortar_elf::header::FileHeader::parse(bytes).unwrap();
    let t = mortar_elf::section::SectionTable::parse(bytes, &h).unwrap();
    let s = t.find_symbol_table(mortar_elf::consts::SHT_SYMTAB).unwrap().unwrap();
    s.iter()
        .find(|(_, y)| s.name(y).is_ok_and(|n| n == name.as_bytes()))
        .map_or_else(|| panic!("no symbol {name}"), |(_, y)| (y.st_value, y.st_size))
}

/// `(sh_addr, sh_size)` of the output section `name`.
fn section(bytes: &[u8], name: &str) -> (u64, u64) {
    let h = mortar_elf::header::FileHeader::parse(bytes).unwrap();
    let t = mortar_elf::section::SectionTable::parse(bytes, &h).unwrap();
    t.iter()
        .find(|s| s.name().is_ok_and(|n| n == name.as_bytes()))
        .map_or_else(|| panic!("no section {name}"), |s| (s.header().sh_addr, s.header().sh_size))
}

/// One link: the output, its inputs and its map (in the linker's format).
#[derive(Clone)]
struct Linked {
    output: Vec<u8>,
    files: Vec<(String, Vec<u8>)>,
    map: Map,
}

impl Linked {
    /// The M6, M7 and M8 findings, `"<id> <message>"`, of `map` (through
    /// mortar's text form when it is mortar's).
    fn judge(&self, map: &Map) -> Vec<String> {
        let map = if map.format == Format::Mortar {
            let text = mortar_plumb::map::mortar::write(map);
            mortar_plumb::map::parse(Format::Mortar, &text).unwrap()
        } else {
            map.clone()
        };
        let inputs: Vec<Input<'_>> =
            self.files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
        mortar_plumb::verify(&self.output, &inputs, &map)
            .findings
            .iter()
            .filter(|f| matches!(f.rule.id().as_str(), "M6" | "M7" | "M8"))
            .map(|f| format!("{} {}", f.rule.id(), f.message))
            .collect()
    }
}

struct Program {
    _dir: Scratch,
    arch: Arch,
    lld: Linked,
    stripped: Linked,
    bfd: Linked,
    mold: Linked,
}

fn link(dir: &Path, program: &Path, args: &[&str], out: &str, format: Format) -> Linked {
    let map_arg = format!("-Map={out}.map");
    let mut a: Vec<&str> = args.to_vec();
    a.extend(["-static", "-e", "_start", "-o", out, &map_arg, "a.o", "b.o"]);
    run(dir, program, &a);
    let text = std::fs::read_to_string(dir.join(format!("{out}.map"))).unwrap();
    let map = mortar_plumb::map::parse(format, &text).unwrap();
    let files = ["a.o", "b.o"]
        .iter()
        .map(|p| ((*p).to_string(), std::fs::read(dir.join(p)).unwrap()))
        .collect();
    Linked { output: std::fs::read(dir.join(out)).unwrap(), files, map }
}

fn build(arch: Arch) -> Option<Program> {
    let asm = arch.tool("as")?;
    let bfd = arch.tool("ld.bfd")?;
    let lld = mortar_testutil::tool("ld.lld")?;
    let mold = mortar_testutil::tool("mold")?;
    let dir = Scratch::new(&format!("maprec-{}", arch.triple()));
    for (name, text) in sources(arch) {
        std::fs::write(dir.join(format!("{name}.s")), text).unwrap();
        run(dir.path(), &asm, &[&format!("{name}.s"), "-o", &format!("{name}.o")]);
    }
    let m = emulation(arch);
    let lld_link = link(dir.path(), &lld, &["-m", m], "out.lld", Format::Lld);
    let stripped = link(dir.path(), &lld, &["-m", m, "-s"], "out.strip", Format::Lld);
    let bfd_link =
        link(dir.path(), &bfd, &["-z", "relro", "-z", "noexecstack"], "out.bfd", Format::Gnu);
    let mold_link = link(dir.path(), &mold, &["-m", m], "out.mold", Format::Mold);
    Some(Program { _dir: dir, arch, lld: lld_link, stripped, bfd: bfd_link, mold: mold_link })
}

/// lld's map as mortar's, with the records a mortar map states: the
/// `.relro_padding` chunk (lld's own `<internal>` row) and one `common`
/// record per common symbol, at the output's value and size, aligned as
/// its definitions ask. lld's row for the padding has size 0 (it sizes
/// the section after it writes the map's rows); the record states the
/// section's address and size, as mortar's does.
fn stated(l: &Linked) -> Map {
    let mut map = l.map.clone();
    map.format = Format::Mortar;
    let (addr, size) = section(&l.output, ".relro_padding");
    assert!(size > 0, "the padding is not empty");
    let c = pad(&mut map);
    assert_eq!(c.kind, ChunkKind::Synthetic, "lld lists .relro_padding as a chunk");
    c.addr = Some(addr);
    c.size = size;
    for (name, align) in COMMONS {
        let (addr, size) = symbol(&l.output, name);
        map.commons.push(CommonAlloc { name: name.into(), addr, size, align, line: 0 });
    }
    map
}

fn archs() -> [Arch; 3] {
    [Arch::Aarch64, Arch::X86_64, Arch::I386]
}

/// `map` changed by `f` must add a finding of rule `id` whose message has
/// `needle`.
fn rejected(p: &Program, base: &Map, case: &str, id: &str, needle: &str, f: impl Fn(&mut Map)) {
    let mut m = base.clone();
    f(&mut m);
    let got = p.lld.judge(&m);
    assert!(
        got.iter().any(|g| g.starts_with(&format!("{id} ")) && g.contains(needle)),
        "{:?} {case}: no {id} finding with '{needle}': {got:#?}",
        p.arch
    );
}

fn pad(m: &mut Map) -> &mut mortar_plumb::map::Chunk {
    m.chunks.iter_mut().find(|c| c.name == ".relro_padding").unwrap()
}

fn common<'m>(m: &'m mut Map, name: &str) -> &'m mut CommonAlloc {
    m.commons.iter_mut().find(|c| c.name == name).unwrap()
}

// §3.3 M6, M7, M8: the records as the output holds them are ACCEPTED;
// each field changed is REJECTED by its rule.
#[test]
fn s3_3_allocation_records_each_field_is_judged() {
    for arch in archs() {
        let Some(p) = build(arch) else { continue };
        let base = stated(&p.lld);
        assert!(p.lld.judge(&base).is_empty(), "{arch:?}: {:#?}", p.lld.judge(&base));
        // M6: the NOBITS chunk is the whole section.
        let not = "and is not that section";
        rejected(&p, &base, "chunk name", "M6", not, |m| pad(m).name = ".relro_paddinG".into());
        rejected(&p, &base, "chunk addr", "M6", not, |m| *pad(m).addr.as_mut().unwrap() += 1);
        rejected(&p, &base, "chunk size+", "M6", not, |m| pad(m).size += 1);
        rejected(&p, &base, "chunk size-", "M6", not, |m| pad(m).size -= 1);
        let bss = symbol(&p.lld.output, "ca").0;
        rejected(&p, &base, "chunk in .bss", "M6", not, |m| pad(m).addr = Some(bss));
        // M7: each field of each common record.
        for (name, align) in COMMONS {
            let c = |s: &str| format!("{name}: {s}");
            rejected(&p, &base, &c("not common"), "M7", "only common definitions", |m| {
                common(m, name).name = "dat".into();
            });
            rejected(&p, &base, &c("no such name"), "M7", "only common definitions", |m| {
                common(m, name).name = "nosuch".into();
            });
            let other = if name == "ca" { "cb" } else { "ca" };
            rejected(&p, &base, &c("another's name"), "M7", "a second record", |m| {
                common(m, name).name = other.into();
            });
            rejected(&p, &base, &c("twice"), "M7", "a second record", |m| {
                let r = common(m, name).clone();
                m.commons.push(r);
            });
            rejected(&p, &base, &c("addr"), "M7", "is stated at address", |m| {
                common(m, name).addr += align;
            });
            if align > 1 {
                rejected(&p, &base, &c("addr unaligned"), "M7", "not a multiple", |m| {
                    common(m, name).addr += 1;
                });
            }
            rejected(&p, &base, &c("size"), "M7", "is stated at size", |m| {
                common(m, name).size += 1;
            });
            rejected(&p, &base, &c("align up"), "M7", "is stated aligned to", |m| {
                common(m, name).align *= 2;
            });
            if align > 1 {
                rejected(&p, &base, &c("align down"), "M7", "is stated aligned to", |m| {
                    common(m, name).align /= 2;
                });
            }
        }
        // M8: a copy record in an output with no dynamic section.
        rejected(&p, &base, "copy in a static output", "M8", "names no COPY record", |m| {
            m.dynamic = Some(DynRecords {
                copies: vec![CopyHint { name: "dat".into(), addr: bss, size: 4, align: Some(4) }],
                ..DynRecords::default()
            });
        });
    }
}

// §3.3 M7: a stripped output (no `.symtab`) has its records' names and
// alignments judged, their addresses and sizes not.
#[test]
fn s3_3_m7_stripped_output_judges_names_and_alignments() {
    for arch in archs() {
        let Some(p) = build(arch) else { continue };
        let s = &p.stripped;
        let mut base = stated(&p.lld);
        let (addr, size) = section(&s.output, ".relro_padding");
        let c = pad(&mut base);
        c.addr = Some(addr);
        c.size = size;
        assert!(s.judge(&base).is_empty(), "{arch:?}: {:#?}", s.judge(&base));
        let mut m = base.clone();
        m.commons[1].size += 1;
        m.commons[1].addr += m.commons[1].align;
        assert!(s.judge(&m).is_empty(), "{arch:?}: {:#?}", s.judge(&m));
        let mut m = base.clone();
        m.commons[1].align *= 2;
        assert!(s.judge(&m).iter().any(|f| f.starts_with("M7 ")), "{arch:?}");
        let mut m = base.clone();
        m.commons[1].name = "nosuch".into();
        assert!(s.judge(&m).iter().any(|f| f.starts_with("M7 ")), "{arch:?}");
    }
}

// §3.4 to §3.6 do not change (§9.3): GNU ld's, lld's and mold's maps of
// a link with commons and a RELRO padding have no allocation record, and
// no M6-M8 rule judges them; their commons stay `common` chunks.
#[test]
fn s3_3_foreign_maps_have_no_allocation_records() {
    for arch in archs() {
        let Some(p) = build(arch) else { continue };
        for (name, l) in [("GNU ld", &p.bfd), ("lld", &p.lld), ("mold", &p.mold)] {
            assert!(l.map.commons.is_empty(), "{arch:?} {name}");
            assert!(l.map.dynamic.is_none(), "{arch:?} {name}");
            assert!(
                l.map.chunks.iter().any(|c| c.kind == ChunkKind::Common),
                "{arch:?} {name}: its COMMON is a common chunk"
            );
            assert!(l.judge(&l.map).is_empty(), "{arch:?} {name}: {:#?}", l.judge(&l.map));
        }
    }
}

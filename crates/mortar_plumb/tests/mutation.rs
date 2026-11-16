//! Mutation tests (`spec/plumb.md` §9.2): a correct reference link, broken
//! one way at a time, must yield a finding. Detection rates are printed.

mod common;

use common::{Corpus, Link, Linker, Size, corpus, unexpected, verify_with};
use mortar_elf::consts::{
    PT_GNU_STACK, PT_LOAD, PT_NOTE, PT_TLS, SHF_ALLOC, SHT_NOBITS, SHT_RELA, SHT_SYMTAB,
};
use mortar_elf::{FileHeader, Object, SectionTable, SymbolIndex};
use mortar_plumb::map::{Placed, SectionRef};
use mortar_plumb::{Format, Map, Report};

fn le64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn put64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}
fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

/// The program headers of an ELF64 file: (offset of the entry, `p_type`,
/// `p_flags`, `p_offset`, `p_vaddr`, `p_filesz`).
fn phdrs(b: &[u8]) -> Vec<(usize, u32, u32, u64, u64, u64)> {
    let phoff = le64(b, 32) as usize;
    let n = usize::from(u16::from_le_bytes([b[56], b[57]]));
    (0..n)
        .map(|i| {
            let o = phoff + i * 56;
            (o, le32(b, o), le32(b, o + 4), le64(b, o + 8), le64(b, o + 16), le64(b, o + 32))
        })
        .collect()
}

/// The file offset of an address in a `PT_LOAD`'s file image.
fn file_offset(b: &[u8], addr: u64) -> Option<usize> {
    phdrs(b)
        .into_iter()
        .find(|p| p.1 == PT_LOAD && p.4 <= addr && addr < p.4 + p.5)
        .map(|p| (p.3 + (addr - p.4)) as usize)
}

/// The section headers: (offset of the entry, name, `sh_type`, `sh_flags`,
/// `sh_addr`, `sh_offset`, `sh_size`).
fn shdrs(b: &[u8]) -> Vec<(usize, String, u32, u64, u64, u64, u64)> {
    let h = FileHeader::parse(b).unwrap();
    let t = SectionTable::parse(b, &h).unwrap();
    let shoff = le64(b, 40) as usize;
    t.iter()
        .enumerate()
        .map(|(i, s)| {
            let sh = s.header();
            (
                shoff + i * 64,
                String::from_utf8_lossy(s.name().unwrap_or(b"")).into_owned(),
                sh.sh_type,
                sh.sh_flags,
                sh.sh_addr,
                sh.sh_offset,
                sh.sh_size,
            )
        })
        .collect()
}

struct Ctx {
    link: &'static Link,
    output: Vec<u8>,
    files: Vec<(String, Vec<u8>)>,
    map: Map,
}

impl Ctx {
    fn of(link: &'static Link) -> Ctx {
        let map = link.parse_map();
        let files = link.inputs(&map);
        Ctx { link, output: link.output_bytes(), files, map }
    }
    fn verify(&self, output: &[u8]) -> Report {
        verify_with(output, &self.files, &self.map)
    }
    /// Whether the report has a finding that is not a recorded anomaly.
    fn found(&self, output: &[u8]) -> bool {
        !unexpected(self.link, &self.verify(output)).is_empty()
    }
}

/// A relocated place of the output: its address, the field mask and width.
struct Place {
    p: u64,
    mask: u64,
    width: usize,
    name: &'static str,
}

/// The relocated places of a freestanding link, from the inputs and the
/// map (every section name is unique per file there).
fn places(ctx: &Ctx) -> Vec<Place> {
    let mut out = Vec::new();
    for (path, bytes) in &ctx.files {
        let Some(fi) = ctx.map.files.iter().position(|f| &f.path == path && f.member.is_none())
        else {
            continue;
        };
        let obj = Object::parse(bytes).unwrap();
        for s in obj.sections() {
            if s.sh_type() != SHT_RELA {
                continue;
            }
            let target = obj.section(s.info_section().unwrap()).unwrap();
            if target.flags() & SHF_ALLOC == 0 {
                continue;
            }
            let name = String::from_utf8_lossy(target.name().unwrap()).into_owned();
            let Some(pl) = ctx
                .map
                .placed
                .iter()
                .find(|p| p.file as usize == fi && p.section == SectionRef::Name(name.clone()))
            else {
                continue;
            };
            for r in s.rela().unwrap() {
                let Some(def) = mortar_plumb::a64::def(r.r_type) else { continue };
                if def.r_type == 0 {
                    continue;
                }
                out.push(Place {
                    p: pl.addr + r.r_offset,
                    mask: def.field.mask(),
                    width: usize::from(def.field.width()),
                    name: def.name,
                });
            }
        }
    }
    out
}

fn small() -> Option<&'static Corpus> {
    corpus(Size::Small)
}

// spec/plumb.md §9.2 (a), §5.2: a flipped bit in a relocated field, for each
// type of the hand-written corpus and every linker.
#[test]
fn s9_2a_flipped_field_bits_are_found() {
    let Some(c) = small() else { return };
    let (mut total, mut found, mut skipped) = (0, 0, 0);
    let mut missed = Vec::new();
    for case in ["types", "got", "veneer", "start", "erratum"] {
        for linker in Linker::ALL {
            let link = c.get(case, linker);
            let ctx = Ctx::of(link);
            assert!(!ctx.found(&ctx.output), "{} is not clean", link.label());
            // A place a recorded anomaly (§10.2 A6) already fails is not a
            // clean starting point.
            let base: Vec<u64> =
                ctx.verify(&ctx.output).findings.iter().filter_map(|f| f.addr).collect();
            for pl in places(&ctx) {
                if base.contains(&pl.p) {
                    skipped += 1;
                    continue;
                }
                let off = file_offset(&ctx.output, pl.p).unwrap();
                for bit in [pl.mask.trailing_zeros(), pl.mask.ilog2()] {
                    let mut m = ctx.output.clone();
                    let byte = off + (bit / 8) as usize;
                    assert!(byte < off + pl.width);
                    m[byte] ^= 1 << (bit % 8);
                    total += 1;
                    if ctx.found(&m) {
                        found += 1;
                    } else {
                        missed.push(format!(
                            "{} {} at {:#x} bit {bit}",
                            link.label(),
                            pl.name,
                            pl.p
                        ));
                    }
                }
            }
        }
    }
    eprintln!(
        "§9.2 (a) relocated-field bit flips: {found}/{total} found ({skipped} places of anomalies skipped)"
    );
    assert!(missed.is_empty(), "not found:\n{}", missed.join("\n"));
}

/// The offsets in the file of the `.symtab` entries with these names.
fn symtab_entries(b: &[u8], names: &[&str]) -> Vec<(String, usize)> {
    let h = FileHeader::parse(b).unwrap();
    let t = SectionTable::parse(b, &h).unwrap();
    let symtab = t.find_symbol_table(SHT_SYMTAB).unwrap().unwrap();
    let off = shdrs(b).into_iter().find(|s| s.2 == SHT_SYMTAB).unwrap().5 as usize;
    symtab
        .iter()
        .filter_map(|(i, s): (SymbolIndex, _)| {
            let n = String::from_utf8_lossy(symtab.name(&s).ok()?).into_owned();
            (names.contains(&n.as_str()) && s.st_shndx != 0).then(|| (n, off + i.0 as usize * 24))
        })
        .collect()
}

// spec/plumb.md §9.2 (b), §4.6: an output symbol moved in .symtab.
#[test]
fn s9_2b_moved_symbols_are_found() {
    let Some(c) = small() else { return };
    let (mut total, mut found) = (0, 0);
    for (case, names) in [
        ("start", &["_start", "msg", "len"][..]),
        ("types", &["func", "near", "gvar", "small", "mid", "big", "neg"][..]),
        ("got", &["gfunc", "gvar"][..]),
        ("veneer", &["farfunc", "_start"][..]),
    ] {
        for linker in Linker::ALL {
            let link = c.get(case, linker);
            let ctx = Ctx::of(link);
            let entries = symtab_entries(&ctx.output, names);
            assert_eq!(entries.len(), names.len(), "{}: {entries:?}", link.label());
            for (name, at) in entries {
                let mut m = ctx.output.clone();
                let v = le64(&m, at + 8).wrapping_add(4);
                put64(&mut m, at + 8, v);
                total += 1;
                assert!(ctx.found(&m), "{}: moving '{name}' is not found", link.label());
                found += 1;
            }
        }
    }
    eprintln!("§9.2 (b) moved symbols: {found}/{total} found");
}

// spec/plumb.md §9.2 (c), §5.5: a corrupted GOT slot. A slot only the
// linker's relaxation stopped using is not a defect, so the rate is
// printed and the referenced slots are required.
#[test]
fn s9_2c_corrupted_got_slots_are_found() {
    let Some(c) = small() else { return };
    for linker in Linker::ALL {
        let link = c.get("got", linker);
        let ctx = Ctx::of(link);
        let Some(got) = shdrs(&ctx.output).into_iter().find(|s| s.1 == ".got") else {
            eprintln!("§9.2 (c) {}: no .got", link.label());
            continue;
        };
        let (mut total, mut found) = (0, 0);
        for k in 0..got.6 / 8 {
            let at = got.5 as usize + k as usize * 8;
            let mut m = ctx.output.clone();
            m[at] ^= 0x08;
            total += 1;
            let r = ctx.verify(&m);
            if r.with_rule("R4").next().is_some() {
                found += 1;
            }
        }
        let used = ctx.verify(&ctx.output).coverage.verified_of(mortar_plumb::Via::GotSlot);
        eprintln!(
            "§9.2 (c) {}: {found}/{total} GOT words found (records through a slot: {used})",
            link.label()
        );
        if used > 0 {
            assert!(found > 0, "{}", link.label());
        }
    }
}

// spec/plumb.md §9.2 (d), §5.4: a retargeted veneer.
#[test]
fn s9_2d_retargeted_veneers_are_found() {
    let Some(c) = small() else { return };
    for linker in Linker::ALL {
        let link = c.get("veneer", linker);
        let ctx = Ctx::of(link);
        let start = symtab_entries(&ctx.output, &["_start"])[0].1;
        let pc = le64(&ctx.output, start + 8);
        let off = file_offset(&ctx.output, pc).unwrap();
        let insn = le32(&ctx.output, off);
        let (_, veneer) = mortar_plumb::a64::branch(insn, pc).unwrap();
        let voff = file_offset(&ctx.output, veneer).unwrap();
        let mut found = 0;
        for k in 0..3 {
            let mut m = ctx.output.clone();
            m[voff + 4 * k] ^= 1 << 5;
            if ctx.found(&m) {
                found += 1;
            }
        }
        eprintln!("§9.2 (d) {}: {found}/3 veneer words found", link.label());
        assert_eq!(found, 3, "{}", link.label());
    }
}

fn expect_rule(r: &Report, id: &str, what: &str) {
    assert!(r.with_rule(id).next().is_some(), "{what}: no {id} finding\n{}", r.render("out"));
}

// spec/plumb.md §9.2 (e), §6.2: each structure rule, broken.
#[test]
#[allow(clippy::many_single_char_names, clippy::too_many_lines)]
fn s9_2e_each_structure_rule_is_enforced() {
    let Some(c) = small() else { return };
    for linker in Linker::ALL {
        let link = c.get("start", linker);
        let ctx = Ctx::of(link);
        let o = &ctx.output;
        let what = link.label();
        let ph = phdrs(o);
        let sh = shdrs(o);
        let mutate = |f: &dyn Fn(&mut Vec<u8>)| {
            let mut m = o.clone();
            f(&mut m);
            ctx.verify(&m)
        };
        // S1: not an executable.
        expect_rule(&mutate(&|m| m[16] = 3), "S1", &what);
        // S2: a PT_LOAD whose offset and address are not congruent.
        let load = ph.iter().find(|p| p.1 == PT_LOAD && p.5 > 0).unwrap();
        expect_rule(
            &mutate(&|m| {
                let v = le64(m, load.0 + 8) + 1;
                put64(m, load.0 + 8, v);
            }),
            "S2",
            &what,
        );
        // S3: two PT_LOADs out of order.
        let loads: Vec<_> = ph.iter().filter(|p| p.1 == PT_LOAD).collect();
        assert!(loads.len() >= 2, "{what}");
        let (a, b) = (loads[0].0, loads[1].0);
        expect_rule(
            &mutate(&|m| {
                let ea: Vec<u8> = m[a..a + 56].to_vec();
                let eb: Vec<u8> = m[b..b + 56].to_vec();
                m[a..a + 56].copy_from_slice(&eb);
                m[b..b + 56].copy_from_slice(&ea);
            }),
            "S3",
            &what,
        );
        let text = sh.iter().find(|s| s.1 == ".text").unwrap();
        let data = sh.iter().find(|s| s.1 == ".data").unwrap();
        // S4: a section at another file offset than its segment maps.
        expect_rule(&mutate(&|m| put64(m, text.0 + 24, text.5 + 4)), "S4", &what);
        // S5: code in a segment that is not executable.
        let exec = ph.iter().find(|p| p.1 == PT_LOAD && p.2 & 1 != 0).unwrap();
        expect_rule(&mutate(&|m| put32(m, exec.0 + 4, exec.2 & !1)), "S5", &what);
        // S6: two sections in the same memory.
        expect_rule(&mutate(&|m| put64(m, data.0 + 16, text.4)), "S6", &what);
        // S7: the entry point outside the code (the data). An entry of 0
        // means "no entry point" (gABI) and is not a finding (WP-028b).
        expect_rule(&mutate(&|m| put64(m, 24, data.4)), "S7", &what);
        expect_rule(&mutate(&|m| put64(m, 24, 1)), "S7", &what);
        let r = mutate(&|m| put64(m, 24, 0));
        assert!(r.with_rule("S7").next().is_none(), "{what}: e_entry 0 is S7\n{}", r.render("o"));
        // S8: a sized symbol outside its section.
        let msg = symtab_entries(o, &["msg"])[0].1;
        expect_rule(
            &mutate(&|m| {
                let v = le64(m, msg + 8) + 0x10_0000;
                put64(m, msg + 8, v);
            }),
            "S8",
            &what,
        );
        // S9: a second PT_TLS.
        let other = ph.iter().find(|p| p.1 == PT_GNU_STACK).unwrap();
        expect_rule(&mutate(&|m| put32(m, other.0, PT_TLS)), "S9", &what);
        assert!(ph.iter().any(|p| p.1 == PT_TLS), "{what}: no PT_TLS");
        // S10: a second PT_GNU_STACK.
        let tls = ph.iter().find(|p| p.1 == PT_TLS).unwrap();
        expect_rule(&mutate(&|m| put32(m, tls.0, PT_GNU_STACK)), "S10", &what);
        // S11: a note whose name runs past its segment.
        if let Some(note) = ph.iter().find(|p| p.1 == PT_NOTE) {
            expect_rule(&mutate(&|m| put32(m, note.3 as usize, 0xffff)), "S11", &what);
        } else {
            panic!("{what}: no PT_NOTE");
        }
        // S12: a byte no relocation covers.
        let start = symtab_entries(o, &["_start"])[0].1;
        let at = file_offset(o, le64(o, start + 8)).unwrap();
        expect_rule(&mutate(&|m| m[at] ^= 1), "S12", &what);
        // S12 for NOBITS memory is covered by the .bss of the types case.
        // S13: PT_INTERP after a PT_LOAD.
        expect_rule(&mutate(&|m| put32(m, other.0, 3)), "S13", &what);
        // S14: `.data` made NOBITS, inside its PT_LOAD's file image.
        expect_rule(&mutate(&|m| put32(m, data.0 + 4, SHT_NOBITS)), "S14", &what);
        // S15: `.data` aligned to 1 MiB.
        expect_rule(&mutate(&|m| put64(m, data.0 + 48, 1 << 20)), "S15", &what);
        // S16: a PT_GNU_RELRO starting 8 bytes into `.data`, which is
        // writable and not RELRO: the page below is made read-only.
        expect_rule(
            &mutate(&|m| {
                put32(m, other.0, 0x6474_e552);
                put64(m, other.0 + 16, data.4 + 8);
                put64(m, other.0 + 40, 0x2000);
            }),
            "S16",
            &what,
        );
        // S17: the last PT_LOAD's file image stretched over the
        // non-allocated sections.
        let last = loads.last().unwrap();
        let len = o.len() as u64;
        expect_rule(
            &mutate(&|m| {
                put64(m, last.0 + 32, len - last.3);
                put64(m, last.0 + 40, len - last.3 + 0x10_0000);
            }),
            "S17",
            &what,
        );
    }
    // S2 (mortar's p_align) and S18 (mortar: a PT_LOAD ends at a
    // section's end), on lld's link read as mortar's.
    let (ctx, map) = mortar_map(c);
    let check = |m: &[u8]| verify_with(m, &ctx.files, &map);
    let r = check(&ctx.output);
    assert!(r.with_rule("S2").next().is_none() && r.with_rule("S18").next().is_none());
    let ph = phdrs(&ctx.output);
    let load = ph.iter().rfind(|p| p.1 == PT_LOAD).unwrap();
    let mut m = ctx.output.clone();
    put64(&mut m, load.0 + 48, 0x100);
    expect_rule(&check(&m), "S2", "mortar: p_align 0x100");
    let mut m = ctx.output.clone();
    let v = le64(&m, load.0 + 40);
    put64(&mut m, load.0 + 40, v + 0x100);
    expect_rule(&check(&m), "S18", "mortar: a PT_LOAD past its last section");
}

/// The lld map of the start link, in mortar's format: the base of the map
/// rule tests (its records are exact: no linker-made section is named after
/// an input).
/// lld's SHA-1 build id is rewritten as mortar's (B1, §6.7).
fn mortar_map(c: &'static Corpus) -> (Ctx, Map) {
    let link = c.get("start", Linker::Lld);
    let mut ctx = Ctx::of(link);
    mortar_plumb::buildid::seal(&mut ctx.output, mortar_plumb::buildid::Tree::L128);
    // lld's row for `.relro_padding` has size 0 (it sizes the section
    // after it writes its map); mortar's states the section (§3.2, M6).
    let h = FileHeader::parse(&ctx.output).unwrap();
    let t = SectionTable::parse(&ctx.output, &h).unwrap();
    if let Some(s) = t.iter().find(|s| s.name().is_ok_and(|n| n == b".relro_padding"))
        && let Some(c) = ctx.map.chunks.iter_mut().find(|c| c.name == ".relro_padding")
    {
        c.addr = Some(s.header().sh_addr);
        c.size = s.header().sh_size;
    }
    let text = mortar_plumb::map::mortar::write(&ctx.map);
    let map = mortar_plumb::map::parse(Format::Mortar, &text).unwrap();
    (ctx, map)
}

fn placed<'m>(map: &'m Map, section: &str) -> &'m Placed {
    map.placed.iter().find(|p| p.section == SectionRef::Name(section.into())).unwrap()
}

// spec/plumb.md §9.2 (e), §3.3: each map rule, broken.
#[test]
fn s9_2e_each_map_rule_is_enforced() {
    let Some(c) = small() else { return };
    let (ctx, base) = mortar_map(c);
    let check = |map: &Map| verify_with(&ctx.output, &ctx.files, map);
    let r = check(&base);
    assert!(
        unexpected(ctx.link, &r).is_empty(),
        "the base is not clean:\n{}",
        r.render("start.lld")
    );
    let text = placed(&base, ".text").clone();
    let data = placed(&base, ".data").clone();
    // M1: a section the file does not have.
    let mut m = base.clone();
    m.placed.push(Placed { section: SectionRef::Name(".nonexistent".into()), ..text.clone() });
    expect_rule(&check(&m), "M1", "M1");
    // M2: a section placed twice.
    let mut m = base.clone();
    m.placed.push(Placed { addr: text.addr + 0x1000, ..text.clone() });
    expect_rule(&check(&m), "M2", "M2");
    // M3: the wrong size.
    let mut m = base.clone();
    m.placed.iter_mut().find(|p| p.section == text.section).unwrap().size += 4;
    expect_rule(&check(&m), "M3", "M3");
    // M4: misaligned.
    let mut m = base.clone();
    m.placed.iter_mut().find(|p| p.section == text.section).unwrap().addr += 1;
    expect_rule(&check(&m), "M4", "M4");
    // M5: two input sections in the same place.
    let mut m = base.clone();
    let d = m.placed.iter_mut().find(|p| p.section == data.section).unwrap();
    d.addr = text.addr;
    expect_rule(&check(&m), "M5", "M5");
}

// spec/plumb.md §6.1: the input and output rules.
#[test]
fn s6_1_input_and_output_rules() {
    let Some(c) = small() else { return };
    let link = c.get("start", Linker::Bfd);
    let ctx = Ctx::of(link);
    // O1.
    expect_rule(&verify_with(b"not an elf file", &ctx.files, &ctx.map), "O1", "O1");
    // I1: an input the map names is missing.
    expect_rule(&verify_with(&ctx.output, &[], &ctx.map), "I1", "I1");
    // I2: an input that is not an object.
    let bad: Vec<(String, Vec<u8>)> =
        ctx.files.iter().map(|(p, _)| (p.clone(), b"!<arch>\nbroken".to_vec())).collect();
    expect_rule(&verify_with(&ctx.output, &bad, &ctx.map), "I2", "I2");
}

/// The relocated places of thread-local types in a freestanding link.
fn tls_places(ctx: &Ctx) -> Vec<Place> {
    places(ctx)
        .into_iter()
        .filter(|p| mortar_plumb::a64::TABLE.iter().any(|d| d.name == p.name && d.op.is_tls()))
        .collect()
}

// spec/plumb.md §9.2 (f), §4.7, W9 to W13: thread-local references broken
// three ways. (1) a flipped bit anywhere in a TLS-relocated instruction,
// relaxed or not; (2) the TLS block moved or realigned in PT_TLS, which
// moves every thread-pointer offset; (3) a corrupted TLS GOT entry.
#[test]
fn s9_2f_tls_mutations_are_found() {
    let Some(c) = corpus(Size::Tls) else { return };
    let mut missed = Vec::new();
    let (mut total, mut found) = (0, 0);
    for linker in Linker::ALL {
        let link = c.get("tls-types", linker);
        let ctx = Ctx::of(link);
        assert!(!ctx.found(&ctx.output), "{} is not clean", link.label());
        for pl in tls_places(&ctx) {
            let off = file_offset(&ctx.output, pl.p).unwrap();
            for bit in [0u32, 5, 10, 16, 22, 31] {
                let mut m = ctx.output.clone();
                m[off + (bit / 8) as usize] ^= 1 << (bit % 8);
                total += 1;
                if ctx.found(&m) {
                    found += 1;
                } else {
                    missed.push(format!("{} {} at {:#x} bit {bit}", link.label(), pl.name, pl.p));
                }
            }
        }
    }
    eprintln!("§9.2 (f1) TLS instruction bit flips: {found}/{total} found");
    assert!(missed.is_empty(), "not found:\n{}", missed.join("\n"));

    let (mut total, mut found) = (0, 0);
    for case in ["tls-types"] {
        for linker in Linker::ALL {
            let link = c.get(case, linker);
            let ctx = Ctx::of(link);
            let tls = phdrs(&ctx.output).into_iter().find(|p| p.1 == PT_TLS).unwrap();
            let align = le64(&ctx.output, tls.0 + 48).max(1);
            for (field, value) in [(16, tls.4 + align), (48, align * 4)] {
                let mut m = ctx.output.clone();
                put64(&mut m, tls.0 + field, value);
                if field == 16 {
                    put64(&mut m, tls.0 + 24, value);
                }
                total += 1;
                assert!(
                    ctx.found(&m),
                    "{}: PT_TLS field {field} := {value:#x} is not found",
                    link.label()
                );
                found += 1;
            }
        }
    }
    eprintln!("§9.2 (f2) PT_TLS moved or realigned: {found}/{total} found");

    for (case, linker) in [("tls-types", Linker::Bfd), ("tls-types", Linker::Mold)] {
        let link = c.get(case, linker);
        let ctx = Ctx::of(link);
        let got = shdrs(&ctx.output).into_iter().find(|s| s.1 == ".got").unwrap();
        let base = ctx.verify(&ctx.output).coverage.verified_of(mortar_plumb::Via::GotSlot);
        let (mut total, mut found) = (0, 0);
        for k in 0..got.6 / 8 {
            let mut m = ctx.output.clone();
            m[got.5 as usize + k as usize * 8] ^= 0x08;
            total += 1;
            if ctx.verify(&m).with_rule("R4").next().is_some() {
                found += 1;
            }
        }
        eprintln!(
            "§9.2 (f3) {}: {found}/{total} GOT words found (records through a slot: {base})",
            link.label()
        );
        assert!(base > 0 && found > 0, "{}", link.label());
    }
}

/// The records of an output `.eh_frame` (file offsets): (start, end of the
/// record, whether it is a CIE).
fn eh_records(b: &[u8], off: usize, size: usize) -> Vec<(usize, usize, bool)> {
    let mut out = Vec::new();
    let mut p = off;
    while p + 4 <= off + size {
        let len = le32(b, p) as usize;
        if len == 0 {
            break;
        }
        out.push((p, p + 4 + len, le32(b, p + 4) == 0));
        p += 4 + len;
    }
    out
}

// spec/plumb.md §9.2 (g), §6.4: each byte of every CIE and FDE of the
// output, the records' lengths, and every byte of `.eh_frame_hdr`.
#[test]
fn s9_2g_eh_frame_mutations_are_found() {
    let Some(c) = corpus(Size::Tls) else { return };
    // (class, mutations, found, found by an E rule)
    let mut tally: Vec<(&str, u32, u32, u32)> =
        ["CIE bytes", "FDE bytes", "record lengths", ".eh_frame_hdr bytes"]
            .iter()
            .map(|k| (*k, 0, 0, 0))
            .collect();
    let mut missed = Vec::new();
    let mut by_rule: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
    for linker in Linker::ALL {
        let link = c.get("eh-frames", linker);
        let ctx = Ctx::of(link);
        assert!(!ctx.found(&ctx.output), "{} is not clean", link.label());
        let secs = shdrs(&ctx.output);
        let eh = secs.iter().find(|s| s.1 == ".eh_frame").unwrap();
        let hdr = secs.iter().find(|s| s.1 == ".eh_frame_hdr").unwrap();
        let mut muts: Vec<(usize, String, Vec<u8>)> = Vec::new();
        for (start, end, cie) in eh_records(&ctx.output, eh.5 as usize, eh.6 as usize) {
            for at in start + 4..end {
                let mut m = ctx.output.clone();
                m[at] ^= 0x01;
                muts.push((usize::from(!cie), format!("{:#x}", at - start), m));
            }
            for at in [start + 8, start + 12].into_iter().filter(|_| !cie) {
                let mut m = ctx.output.clone();
                m[at + 1] ^= 0x10;
                muts.push((1, format!("{:#x} bit 12", at - start), m));
            }
            let mut m = ctx.output.clone();
            put32(&mut m, start, le32(&ctx.output, start) + 4);
            muts.push((2, format!("length at {:#x}", start - eh.5 as usize), m));
        }
        for at in hdr.5 as usize..(hdr.5 + hdr.6) as usize {
            let mut m = ctx.output.clone();
            m[at] ^= 0x01;
            muts.push((3, format!("{:#x}", at - hdr.5 as usize), m));
        }
        for (class, what, m) in muts {
            let r = ctx.verify(&m);
            let t = &mut tally[class];
            t.1 += 1;
            if unexpected(link, &r).is_empty() {
                missed.push(format!("{} {}: {what}", link.label(), t.0));
            } else {
                t.2 += 1;
                if r.findings.iter().any(|f| f.rule.id().starts_with('E')) {
                    t.3 += 1;
                }
                let ids: std::collections::BTreeSet<String> = r
                    .findings
                    .iter()
                    .map(|f| f.rule.id())
                    .filter(|id| id.starts_with('E'))
                    .collect();
                for id in ids {
                    *by_rule.entry(id).or_default() += 1;
                }
            }
        }
    }
    for (k, n, f, e) in &tally {
        eprintln!("§9.2 (g) {k}: {f}/{n} found, {e} by an E rule");
    }
    eprintln!("§9.2 (g) mutants each rule finds: {by_rule:?}");
    for id in ["E1", "E2", "E3", "E4", "E5"] {
        assert!(by_rule.contains_key(id), "{id} finds no mutant");
    }
    // Equivalent mutants: the frames mean what they meant. The zPLR CIE's
    // LSDA encoding sdata4 made sdata2 (byte 0x17) decodes the same LSDA,
    // whose offset fits in 16 bits; the last FDE lengthened over the
    // terminator lld and mold add (no input gives one) gains DW_CFA_nops.
    let equivalent = [
        "eh-frames/bfd CIE bytes: 0x17",
        "eh-frames/lld CIE bytes: 0x17",
        "eh-frames/mold CIE bytes: 0x17",
        "eh-frames/lld record lengths: length at 0xc0",
        "eh-frames/mold record lengths: length at 0xc0",
    ];
    missed.retain(|m| !equivalent.contains(&m.as_str()));
    assert!(missed.is_empty(), "not found:\n{}", missed.join("\n"));

    // E1: crtend.o's terminator, swallowed by the FDE before it.
    let (mut total, mut found) = (0, 0);
    for linker in Linker::ALL {
        let link = c.get("tls-c", linker);
        let ctx = Ctx::of(link);
        let eh = shdrs(&ctx.output).into_iter().find(|s| s.1 == ".eh_frame").unwrap();
        let recs = eh_records(&ctx.output, eh.5 as usize, eh.6 as usize);
        let &(last, end, _) = recs.last().unwrap();
        assert_eq!(le32(&ctx.output, end), 0, "{}: no terminator", link.label());
        let mut m = ctx.output.clone();
        put32(&mut m, last, le32(&ctx.output, last) + 4);
        total += 1;
        if ctx.verify(&m).with_rule("E1").next().is_some() {
            found += 1;
        }
    }
    eprintln!("§9.2 (g) the terminator swallowed: {found}/{total} found");
    assert_eq!(found, total);
}

/// The output's `.symtab` entries: (file offset, name, `st_info`,
/// `st_shndx`, `st_value`).
fn symtab_all(b: &[u8]) -> Vec<(usize, String, u8, u16, u64)> {
    let h = FileHeader::parse(b).unwrap();
    let t = SectionTable::parse(b, &h).unwrap();
    let st = t.find_symbol_table(SHT_SYMTAB).unwrap().unwrap();
    let off = shdrs(b).into_iter().find(|s| s.2 == SHT_SYMTAB).unwrap().5 as usize;
    st.iter()
        .map(|(i, s)| {
            (
                off + i.0 as usize * 24,
                String::from_utf8_lossy(st.name(&s).unwrap()).into_owned(),
                s.st_info,
                s.st_shndx,
                s.st_value,
            )
        })
        .collect()
}

// spec/plumb.md §9.2 (h), §6.5: the symbol table. Y2: the table's own
// fields; Y3: each attribute of every global the inputs define; Y4: each
// attribute of every local Y4 judges; Y5: every linker-defined symbol
// moved.
#[test]
#[allow(clippy::too_many_lines, clippy::many_single_char_names)] // one class after another
fn s9_2h_symbol_table_mutations_are_found() {
    let Some(c) = corpus(Size::Tls) else { return };
    let mut tally: std::collections::BTreeMap<&str, (u32, u32)> = std::collections::BTreeMap::new();
    let mut missed = Vec::new();
    for case in ["tls-types", "eh-frames", "got-ldr-w"] {
        for linker in Linker::ALL {
            let link = c.get(case, linker);
            let ctx = Ctx::of(link);
            let base = ctx.verify(&ctx.output);
            let key = |f: &&mortar_plumb::Finding| (f.rule, f.addr, f.message.clone());
            let before: std::collections::BTreeSet<_> =
                unexpected(link, &base).iter().map(key).collect();
            let new =
                |r: &Report| unexpected(link, r).iter().map(key).any(|k| !before.contains(&k));
            let syms = symtab_all(&ctx.output);
            let secs = shdrs(&ctx.output);
            let symtab = secs.iter().find(|s| s.2 == SHT_SYMTAB).unwrap();
            let mut muts: Vec<(&str, String, Vec<u8>)> = Vec::new();
            let mut m = ctx.output.clone();
            put32(&mut m, symtab.0 + 44, le32(&ctx.output, symtab.0 + 44) + 1);
            muts.push(("Y2", "sh_info + 1".into(), m));
            let mut m = ctx.output.clone();
            put32(&mut m, symtab.0 + 44, le32(&ctx.output, symtab.0 + 44) - 1);
            muts.push(("Y2", "sh_info - 1".into(), m));
            let mut m = ctx.output.clone();
            put64(&mut m, symtab.0 + 56, 16);
            muts.push(("Y2", "sh_entsize 16".into(), m));
            let mut m = ctx.output.clone();
            put64(&mut m, syms[0].0 + 8, 1);
            muts.push(("Y2", "symbol 0's value".into(), m));
            let first_global = syms.iter().position(|s| s.2 >> 4 != 0).unwrap_or(syms.len());
            if first_global < syms.len() {
                let (a, b) = (syms[first_global - 1].0, syms[first_global].0);
                let mut m = ctx.output.clone();
                let (x, y) = (ctx.output[a..a + 24].to_vec(), ctx.output[b..b + 24].to_vec());
                m[a..a + 24].copy_from_slice(&y);
                m[b..b + 24].copy_from_slice(&x);
                muts.push(("Y2", "last local and first global swapped".into(), m));
            }
            let strtab = &secs[le32(&ctx.output, symtab.0 + 40) as usize];
            let mut m = ctx.output.clone();
            m[strtab.5 as usize] = b'x';
            muts.push(("Y2", ".strtab's first byte".into(), m));
            let shstrndx = usize::from(u16::from_le_bytes([ctx.output[62], ctx.output[63]]));
            let mut m = ctx.output.clone();
            m[secs[shstrndx].5 as usize] = b'x';
            muts.push(("Y2", ".shstrtab's first byte".into(), m));
            // Y3 and Y4: the attributes of the symbols the inputs define.
            let names = |global: bool| -> std::collections::BTreeSet<String> {
                ctx.files
                    .iter()
                    .filter_map(|(_, b)| Object::parse(b).ok())
                    .flat_map(|o| {
                        let t = o.symbols().ok().flatten();
                        t.into_iter().flat_map(|t| {
                            t.iter()
                                .filter(|(_, s)| s.st_shndx != 0 && (!global || s.binding() != 0))
                                .filter_map(|(_, s)| {
                                    t.name(&s).ok().map(|n| String::from_utf8_lossy(n).into_owned())
                                })
                                .collect::<Vec<_>>()
                        })
                    })
                    .collect()
            };
            let defined = names(true);
            let defined_any = names(false);
            let other_sec = |shndx: u16, v: u64| {
                secs.iter()
                    .position(|s| s.3 & SHF_ALLOC != 0 && (v < s.4 || v > s.4 + s.6))
                    .map(|i| i as u16)
                    .filter(|&i| i != shndx)
            };
            for (off, name, info, shndx, value) in &syms {
                if name.is_empty() || *shndx == 0 || matches!(info & 0xf, 3 | 4) {
                    continue;
                }
                let class = if defined.contains(name) { "Y3" } else { "Y4" };
                // A Y4 local counts when moving it by 4 is found: those Y4
                // judges.
                if class == "Y4" {
                    let mut m = ctx.output.clone();
                    put64(&mut m, off + 8, value + 4);
                    if unexpected(link, &ctx.verify(&m)).iter().all(|f| f.rule.id() != "Y4") {
                        continue;
                    }
                }
                let mut m = ctx.output.clone();
                put64(&mut m, off + 16, le64(&ctx.output, off + 16) + 1);
                muts.push((class, format!("{name} size + 1"), m));
                if class == "Y4" {
                    let mut m = ctx.output.clone();
                    put64(&mut m, off + 8, value + 4);
                    muts.push((class, format!("{name} value + 4"), m));
                    let mut m = ctx.output.clone();
                    put64(&mut m, off + 8, value + 0x10_0000);
                    muts.push((class, format!("{name} moved far"), m));
                }
                let mut m = ctx.output.clone();
                m[off + 4] = (info & 0xf0) | if info & 0xf == 1 { 2 } else { 1 };
                if info & 0xf != 6 {
                    muts.push((class, format!("{name} type"), m));
                }
                if let Some(k) = other_sec(*shndx, *value) {
                    let mut m = ctx.output.clone();
                    m[off + 6..off + 8].copy_from_slice(&k.to_le_bytes());
                    muts.push((class, format!("{name} shndx {k}"), m));
                }
                if class == "Y3" {
                    let mut m = ctx.output.clone();
                    m[off + 4] = (info & 0xf) | if info >> 4 == 2 { 0x10 } else { 0x20 };
                    muts.push((class, format!("{name} binding"), m));
                    let mut m = ctx.output.clone();
                    let vis = ctx.output[off + 5] & 3;
                    m[off + 5] = if vis == 0 { 3 } else { 0 };
                    muts.push((class, format!("{name} visibility"), m));
                }
            }
            // Y5: the linker-defined symbols, moved far.
            for (off, name, info, shndx, value) in &syms {
                if *shndx == 0
                    || name.is_empty()
                    || defined_any.contains(name)
                    || matches!(info & 0xf, 3 | 4)
                    || base.coverage.linker_defined_not_judged.contains(name)
                    // A recorded anomaly already (A3).
                    || base.findings.iter().any(|f| f.symbol.as_deref() == Some(name))
                {
                    continue;
                }
                let mut m = ctx.output.clone();
                put64(&mut m, off + 8, value + 0x10_0000);
                muts.push(("Y5", format!("{name} moved"), m));
            }
            for (class, what, m) in muts {
                let t = tally.entry(class).or_default();
                t.0 += 1;
                if new(&ctx.verify(&m)) {
                    t.1 += 1;
                } else {
                    missed.push(format!("{} {class}: {what}", link.label()));
                }
            }
        }
    }
    for (k, (n, f)) in &tally {
        eprintln!("§9.2 (h) {k}: {f}/{n} found");
    }
    assert!(missed.is_empty(), "not found:\n{}", missed.join("\n"));
}

// spec/plumb.md §9.2 (i), §4.8: IFUNC references and tables broken. (1)
// the lowest and highest bit of every relocated field of `ifunc`; (2)
// every IRELATIVE record's type, symbol, addend and slot; (3) the words of
// every IPLT entry that decide what it loads and where it branches; (4) the
// data word of `gf` made `lf`'s entry (F3: two addresses for `gf`); (5)
// `__rela_iplt_start` or `__rela_iplt_end` one record short (F1).
#[test]
#[allow(clippy::too_many_lines)]
fn s9_2i_ifunc_mutations_are_found() {
    let Some(c) = corpus(Size::Tls) else { return };
    let mut missed = Vec::new();
    let mut counts = [(0u32, 0u32); 3];
    let mut try_one = |k: usize, ctx: &Ctx, m: &[u8], what: String| {
        counts[k].0 += 1;
        if ctx.found(m) {
            counts[k].1 += 1;
        } else {
            missed.push(what);
        }
    };
    for linker in Linker::ALL {
        let link = c.get("ifunc", linker);
        let ctx = Ctx::of(link);
        assert!(!ctx.found(&ctx.output), "{} is not clean", link.label());
        for pl in places(&ctx) {
            let off = file_offset(&ctx.output, pl.p).unwrap();
            for bit in [pl.mask.trailing_zeros(), pl.mask.ilog2()] {
                let mut m = ctx.output.clone();
                m[off + (bit / 8) as usize] ^= 1 << (bit % 8);
                try_one(
                    0,
                    &ctx,
                    &m,
                    format!("{} {} at {:#x} bit {bit}", link.label(), pl.name, pl.p),
                );
            }
        }
        let secs = shdrs(&ctx.output);
        let rela = secs.iter().find(|s| s.2 == SHT_RELA && s.3 & SHF_ALLOC != 0).unwrap();
        let mut slots = Vec::new();
        for k in 0..(rela.6 / 24) as usize {
            let o = rela.5 as usize + k * 24;
            slots.push(le64(&ctx.output, o));
            for (field, delta, what) in
                [(8, 1u64, "type"), (8, 1 << 32, "symbol"), (16, 4, "addend"), (0, 8, "slot")]
            {
                let mut m = ctx.output.clone();
                let v = le64(&m, o + field);
                put64(&mut m, o + field, v.wrapping_add(delta));
                try_one(1, &ctx, &m, format!("{} IRELATIVE {k} {what}", link.label()));
            }
        }
        // The IPLT entries: the code that loads an IRELATIVE slot.
        let plt = [".plt", ".iplt", ".plt.got"];
        for s in secs.iter().filter(|s| plt.contains(&s.1.as_str())) {
            for e in (0..s.6).step_by(16) {
                let words: &[u64] = if linker == Linker::Mold { &[0, 4, 8] } else { &[0, 4, 12] };
                for &w in words {
                    let mut m = ctx.output.clone();
                    m[(s.5 + e + w) as usize] ^= 1 << 5;
                    try_one(2, &ctx, &m, format!("{} {} entry {e:#x} word {w}", link.label(), s.1));
                }
            }
        }
        // (4): `.data` holds gf's then lf's canonical entry.
        let data = secs.iter().find(|s| s.1 == ".data").unwrap();
        let mut m = ctx.output.clone();
        let lf = le64(&m, data.5 as usize + 8);
        put64(&mut m, data.5 as usize, lf);
        expect_rule(&ctx.verify(&m), "F3", &format!("{}: gf's word made lf's entry", link.label()));
        assert!(slots.len() == 2, "{}", link.label());
    }
    // (5): the bounds one record short at either end: an IRELATIVE
    // record outside `__rela_iplt_*` (synth.md R-P3).
    for linker in Linker::ALL {
        let ctx = Ctx::of(c.get("ifunc", linker));
        for (name, delta) in [("__rela_iplt_start", 24i64), ("__rela_iplt_end", -24)] {
            let syms = symtab_entries(&ctx.output, &[name]);
            let (_, o) = syms.first().unwrap_or_else(|| panic!("{} defines {name}", linker.name()));
            let mut m = ctx.output.clone();
            let v = le64(&m, o + 8);
            put64(&mut m, o + 8, v.wrapping_add_signed(delta));
            expect_rule(&ctx.verify(&m), "F1", &format!("{}: {name} moved", linker.name()));
        }
    }
    let names = ["relocated fields", "IRELATIVE records", "IPLT entry words"];
    for (n, (total, found)) in names.iter().zip(counts) {
        eprintln!("§9.2 (i) {n}: {found}/{total} found");
    }
    assert!(missed.is_empty(), "not found:\n{}", missed.join("\n"));
}

/// The relocated data fields of non-alloc sections, as file offsets of the
/// output (GNU ld's and lld's maps give the offset in the output section).
fn nonalloc_places(ctx: &Ctx) -> Vec<(usize, u64, &'static str)> {
    let secs = shdrs(&ctx.output);
    let mut out = Vec::new();
    for (path, bytes) in &ctx.files {
        let Some(fi) = ctx.map.files.iter().position(|f| &f.path == path && f.member.is_none())
        else {
            continue;
        };
        let obj = Object::parse(bytes).unwrap();
        for s in obj.sections() {
            if s.sh_type() != SHT_RELA {
                continue;
            }
            let target = obj.section(s.info_section().unwrap()).unwrap();
            if target.flags() & SHF_ALLOC != 0 {
                continue;
            }
            let name = String::from_utf8_lossy(target.name().unwrap()).into_owned();
            let Some(o) = secs.iter().find(|x| x.1 == name && x.3 & SHF_ALLOC == 0) else {
                continue;
            };
            let Some(pl) = ctx
                .map
                .placed
                .iter()
                .find(|p| p.file as usize == fi && p.section == SectionRef::Name(name.clone()))
            else {
                continue;
            };
            for r in s.rela().unwrap() {
                let Some(def) = mortar_plumb::a64::def(r.r_type) else { continue };
                if def.r_type == 0 {
                    continue;
                }
                out.push(((o.5 + pl.addr - o.4 + r.r_offset) as usize, def.field.mask(), def.name));
            }
        }
    }
    out
}

// spec/plumb.md §9.2 (j), §5.8: debug information broken. (1) the lowest
// and highest bit of every relocated non-alloc field of `debug-comdat` and
// `debug-gc`, tombstones included, by GNU ld and lld; (2) lld's link read
// as mortar's: every tombstone differs from image.md I-47 (R1); written
// as I-47 and I-112 say it is clean; one aranges length restored is R1.
#[test]
#[allow(clippy::many_single_char_names, clippy::too_many_lines)]
fn s9_2j_debug_mutations_are_found() {
    let Some(c) = corpus(Size::Tls) else { return };
    let mut missed = Vec::new();
    let (mut total, mut found) = (0, 0);
    for case in ["debug-comdat", "debug-gc"] {
        for linker in [Linker::Bfd, Linker::Lld] {
            let link = c.get(case, linker);
            let ctx = Ctx::of(link);
            assert!(!ctx.found(&ctx.output), "{} is not clean", link.label());
            let places = nonalloc_places(&ctx);
            assert!(places.len() > 20, "{}: {} places", link.label(), places.len());
            // Every place's lowest and highest bit with MORTAR_PLUMB_FULL=1;
            // otherwise 24 places spread over the link, one bit each,
            // alternating (each flip is a whole verify of a debug link).
            let full = std::env::var_os("MORTAR_PLUMB_FULL").is_some();
            let stride = if full { 1 } else { places.len().div_ceil(24) };
            for (k, (off, mask, name)) in places.into_iter().enumerate().step_by(stride) {
                let bits = [mask.trailing_zeros(), mask.ilog2()];
                let bits = if full { &bits[..] } else { &bits[k / stride % 2..][..1] };
                for &bit in bits {
                    let mut m = ctx.output.clone();
                    m[off + (bit / 8) as usize] ^= 1 << (bit % 8);
                    total += 1;
                    if ctx.found(&m) {
                        found += 1;
                    } else {
                        missed.push(format!("{} {name} at file {off:#x} bit {bit}", link.label()));
                    }
                }
            }
        }
    }
    eprintln!("§9.2 (j1) non-alloc field bit flips: {found}/{total} found");
    assert!(missed.is_empty(), "not found:\n{}", missed.join("\n"));

    // (2) mortar's convention, with lld's non-alloc places written as
    // `nplace` records (R-9).
    let link = c.get("debug-comdat", Linker::Lld);
    let ctx = Ctx::of(link);
    let secs = shdrs(&ctx.output);
    let mut base = ctx.map.clone();
    let (nonalloc, placed): (Vec<_>, Vec<_>) = base.placed.drain(..).partition(|p| {
        matches!(&p.section, SectionRef::Name(n) if secs.iter().any(|x| &x.1 == n && x.3 & SHF_ALLOC == 0))
    });
    base.placed = placed;
    for p in nonalloc {
        let SectionRef::Name(n) = &p.section else { unreachable!() };
        let o = secs.iter().find(|x| &x.1 == n && x.3 & SHF_ALLOC == 0).unwrap();
        base.nonalloc.push(mortar_plumb::map::NonAlloc {
            file: p.file,
            section: p.section.clone(),
            out: n.clone(),
            offset: Some(p.addr - o.4),
            size: p.size,
            line: 0,
        });
    }
    assert!(base.nonalloc.len() > 6, "{} nplace records", base.nonalloc.len());
    let text = mortar_plumb::map::mortar::write(&base);
    let map = mortar_plumb::map::parse(Format::Mortar, &text).unwrap();
    // R-9's map rules within one output section: M2 and M5.
    let mut twice = map.clone();
    twice.nonalloc.push(map.nonalloc[0].clone());
    expect_rule(&verify_with(&ctx.output, &ctx.files, &twice), "M2", "an nplace twice");
    let mut over = map.clone();
    let (a, b) = {
        let mut k: Vec<usize> = (0..map.nonalloc.len()).collect();
        k.sort_by_key(|&i| (map.nonalloc[i].out.clone(), map.nonalloc[i].offset));
        let pair = k.windows(2).find(|w| {
            let (x, y) = (&map.nonalloc[w[0]], &map.nonalloc[w[1]]);
            x.out == y.out && x.size > 0 && y.size > 0
        });
        let w = pair.expect("two nplace records in one output section");
        (w[0], w[1])
    };
    over.nonalloc[b].offset = over.nonalloc[a].offset;
    expect_rule(&verify_with(&ctx.output, &ctx.files, &over), "M5", "two nplace records overlap");
    let judged = |out: &[u8]| -> Vec<mortar_plumb::Finding> {
        verify_with(out, &ctx.files, &map)
            .findings
            .into_iter()
            .filter(|f| matches!(f.rule.id().as_str(), "R1" | "R5" | "S12"))
            .collect()
    };
    let bad = judged(&ctx.output);
    assert!(bad.len() >= 4, "lld's tombstones are not mortar's:\n{bad:?}");
    let mut m = ctx.output.clone();
    let mut lengths = Vec::new();
    for f in &bad {
        assert_eq!(f.rule.id(), "R1", "{f:?}");
        let site = f.site.as_ref().unwrap();
        let o = secs.iter().find(|x| x.1 == site.section && x.3 & SHF_ALLOC == 0).unwrap();
        let at = (o.5 + f.addr.unwrap() - o.4) as usize;
        let def = mortar_plumb::a64::TABLE.iter().find(|d| Some(d.name) == f.reloc.as_deref());
        let w = usize::from(def.unwrap().field.width());
        let v = f.expected.unwrap().to_le_bytes();
        m[at..at + w].copy_from_slice(&v[..w]);
        if site.section == ".debug_aranges" {
            m[at + 8..at + 16].fill(0);
            lengths.push(at + 8);
        }
    }
    let after = judged(&m);
    assert!(after.is_empty(), "I-47 and I-112 written are not clean:\n{after:?}");
    assert!(!lengths.is_empty(), "no aranges tombstone in debug-comdat");
    let mut n = m.clone();
    n[lengths[0]] = 0x10;
    let r = verify_with(&n, &ctx.files, &map);
    assert!(
        r.findings.iter().any(|f| f.rule.id() == "R1" && f.message.contains("I-112")),
        "an aranges length kept is not found\n{}",
        r.render("o")
    );
}

/// The file offset of the first property's value in the output's
/// `.note.gnu.property`, and the entry of `PT_GNU_PROPERTY`.
fn property_places(b: &[u8]) -> (usize, usize) {
    let note = shdrs(b).into_iter().find(|s| s.1 == ".note.gnu.property").unwrap();
    let ph = phdrs(b).into_iter().find(|p| p.1 == 0x6474_e553).unwrap();
    (note.5 as usize + 16, ph.0)
}

// spec/plumb.md §9.2 (k), §6.6 (synth.md R-P3): program properties
// broken. (1) a feature bit no input has (GCS) set; (2) PAC set where one
// input has only BTI; (3) `PT_GNU_PROPERTY` moved by 8, and made
// `PT_NULL`; (4) the pr_type made an unknown type, read as mortar's link;
// (5) `prop-two` read as mortar's link: GNU ld's and lld's OR of one
// object's notes is not S-126's AND (P1), and the output written by S-126
// is clean.
#[test]
fn s9_2k_property_mutations_are_found() {
    let Some(c) = corpus(Size::Tls) else { return };
    let mut n = 0;
    for linker in [Linker::Bfd, Linker::Lld] {
        let label = linker.name();
        let ctx = Ctx::of(c.get("prop-and", linker));
        assert!(!ctx.found(&ctx.output), "prop-and/{label} is not clean");
        let (value, ph) = property_places(&ctx.output);
        let mut m = ctx.output.clone();
        m[value + 8] |= 4;
        expect_rule(&ctx.verify(&m), "P1", &format!("{label}: GCS claimed"));
        let mut m = ctx.output.clone();
        let v = le64(&m, ph + 16);
        put64(&mut m, ph + 16, v + 8);
        expect_rule(&ctx.verify(&m), "P2", &format!("{label}: PT_GNU_PROPERTY moved"));
        let mut m = ctx.output.clone();
        put32(&mut m, ph, 0);
        expect_rule(&ctx.verify(&m), "P2", &format!("{label}: PT_GNU_PROPERTY made PT_NULL"));
        let ctx = Ctx::of(c.get("prop-bti", linker));
        let (value, _) = property_places(&ctx.output);
        let mut m = ctx.output.clone();
        m[value + 8] |= 2;
        expect_rule(&ctx.verify(&m), "P1", &format!("{label}: PAC claimed"));
        n += 4;
        // Mortar's rules, on the same bytes.
        for case in ["prop-and", "prop-two"] {
            let ctx = Ctx::of(c.get(case, linker));
            let text = mortar_plumb::map::mortar::write(&ctx.map);
            let map = mortar_plumb::map::parse(Format::Mortar, &text).unwrap();
            let props = |out: &[u8]| -> Vec<String> {
                verify_with(out, &ctx.files, &map)
                    .findings
                    .into_iter()
                    .filter(|f| f.rule.id().starts_with('P'))
                    .map(|f| f.message)
                    .collect()
            };
            let (value, _) = property_places(&ctx.output);
            if case == "prop-and" {
                assert!(props(&ctx.output).is_empty(), "{case}/{label} as mortar's");
                let mut m = ctx.output.clone();
                put32(&mut m, value, 0xc000_0003);
                assert!(
                    props(&m).iter().any(|s| s.contains("0xc0000003")),
                    "{label}: an unknown type kept is not found"
                );
            } else {
                let bad = props(&ctx.output);
                assert!(bad.iter().any(|s| s.contains("0xc0000000")), "{label}: S-126 {bad:?}");
                let mut m = ctx.output.clone();
                m[value + 8] = 1;
                assert!(props(&m).is_empty(), "{label}: S-126 written is not clean");
            }
            n += 1;
        }
    }
    eprintln!("§9.2 (k) property mutations: {n}/{n} found");
}

// spec/plumb.md §9.2 (l), §6.7 (image.md R-8): mortar's build id broken.
// (1) lld's own SHA-1 id, read as mortar's link; (2) the id sealed by
// I-72 (the bare and the `sha1` tree) is clean, and not judged under
// lld's map; (3) one byte of the id changed; (4) one byte changed at 16
// places outside the id: the root no longer matches.
#[test]
fn s9_2l_build_id_mutations_are_found() {
    use mortar_plumb::buildid::{Tree, seal};
    let Some(c) = small() else { return };
    let link = c.get("start", Linker::Lld);
    let ctx = Ctx::of(link);
    let note = shdrs(&ctx.output).into_iter().find(|s| s.1 == ".note.gnu.build-id");
    let note = note.expect("the start link has no build id: the driver passes --build-id");
    let id = note.5 as usize + 16;
    let text = mortar_plumb::map::mortar::write(&ctx.map);
    let map = mortar_plumb::map::parse(Format::Mortar, &text).unwrap();
    let b1 = |out: &[u8]| verify_with(out, &ctx.files, &map);
    let mut n = 0;
    expect_rule(&b1(&ctx.output), "B1", "lld's SHA-1 id as mortar's");
    n += 1;
    for t in [Tree::L128, Tree::Sha1] {
        let mut m = ctx.output.clone();
        assert_eq!(seal(&mut m, t), 1);
        let r = b1(&m);
        assert!(r.with_rule("B1").next().is_none(), "{t:?} sealed:\n{}", r.render("o"));
        let what = r.coverage.build_id.clone().unwrap_or_default();
        assert!(what.contains(&format!("{t:?}")), "{t:?}: coverage says {what:?}");
        assert!(ctx.verify(&m).with_rule("B1").next().is_none(), "B1 judged under lld's map");
    }
    let mut sealed = ctx.output.clone();
    seal(&mut sealed, Tree::L128);
    let mut m = sealed.clone();
    m[id + 19] ^= 1;
    expect_rule(&b1(&m), "B1", "the id's last byte");
    n += 1;
    let len = sealed.len();
    let mut by_b1 = 0;
    for k in 0..16 {
        let mut at = 9 + k * (len - 10) / 16;
        if (id..id + 20).contains(&at) {
            at = id + 20;
        }
        let mut m = sealed.clone();
        m[at] ^= 0x80;
        let r = b1(&m);
        // A byte that breaks the output's form is another rule's (O1, or
        // S11 for a note header), which may keep B1 from finding the id.
        assert!(!r.clean(), "a byte at {at:#x} changed: no finding");
        if r.with_rule("B1").next().is_some() {
            by_b1 += 1;
        }
        n += 1;
    }
    eprintln!("§9.2 (l) build-id mutations: {n}/{n} found, {by_b1} of the 16 bytes by B1");
    assert!(by_b1 >= 12, "only {by_b1} of 16 bytes found by B1");
}

// spec/plumb.md §5.4 R9, §4.8 (WP-028d): the `bti` case is clean for every
// linker (the IFUNC's `bti c` IPLT entry, GNU ld's chained pad veneer,
// lld's falling-through BTI thunk); then, in the GNU ld and lld links,
// `padded`'s landing pad is removed from the input object and the output
// alike, so the content still matches and only R9 can see that the
// veneer's `BR x16` now lands on a `NOP`.
#[test]
fn s9_2n_a_veneer_to_no_landing_pad_is_found() {
    const NOP: u32 = 0xd503_201f;
    let Some(c) = small() else { return };
    for linker in Linker::ALL {
        let ctx = Ctx::of(c.get("bti", linker));
        let r = ctx.verify(&ctx.output);
        common::assert_clean(ctx.link, &r);
        assert!(r.coverage.iplt_entries == 1, "{}: {}", ctx.link.label(), r.summary());
        if linker == Linker::Mold {
            continue; // no property note on AArch64 (§6.6): nothing is guarded
        }
        let (_, off) = symtab_entries(&ctx.output, &["padded"]).remove(0);
        let value = le64(&ctx.output, off + 8);
        let at = file_offset(&ctx.output, value).unwrap();
        assert_eq!(le32(&ctx.output, at), 0xd503_245f, "padded starts with bti c");
        let mut out = ctx.output.clone();
        put32(&mut out, at, NOP);
        let mut files = ctx.files.clone();
        let (_, obj) = files.iter_mut().find(|(p, _)| p.ends_with("bti.o")).unwrap();
        let text_off = {
            let o = Object::parse(obj).unwrap();
            let s = o.sections().find(|s| s.name().ok() == Some(&b".fartext"[..])).unwrap();
            s.header().sh_offset as usize
        };
        put32(obj, text_off, NOP);
        let m = verify_with(&out, &files, &ctx.map);
        let rules: Vec<String> = unexpected(ctx.link, &m).iter().map(|f| f.rule.id()).collect();
        assert_eq!(rules, ["R9"], "{}: {}", ctx.link.label(), m.render("bti"));
    }
}

// spec/plumb.md §6.7 (WP-028d): B1 follows the style mortar's map states
// on its build-id chunk. `start` by lld read as mortar's link, its id
// replaced: (1) 20 bytes given on the command line (`--build-id=0x…`),
// with no style in the map, are judged as a hash (B1: what maps without
// the style still get); (2) the same map stating `given`: not judged;
// (3) the bare tree's id under `sha1`: B1, under `default`: clean; (4) a
// 20-byte id under `fast` (8 bytes): B1.
#[test]
fn s9_2l_build_id_style_from_the_map() {
    use mortar_plumb::buildid::{Tree, seal};
    use mortar_plumb::map::{Chunk, ChunkKind};
    let Some(c) = small() else { return };
    let ctx = Ctx::of(c.get("start", Linker::Lld));
    let note = shdrs(&ctx.output).into_iter().find(|s| s.1 == ".note.gnu.build-id").unwrap();
    let id = note.5 as usize + 16;
    let text = mortar_plumb::map::mortar::write(&ctx.map);
    let base = mortar_plumb::map::parse(Format::Mortar, &text).unwrap();
    let with_style = |style: Option<&str>| {
        let mut m = base.clone();
        m.chunks.retain(|c| c.name != ".note.gnu.build-id");
        m.chunks.push(Chunk {
            kind: ChunkKind::Synthetic,
            name: ".note.gnu.build-id".into(),
            addr: Some(note.4),
            size: note.6,
            target: style.map(|s| (s.to_owned(), 0)),
            line: 0,
        });
        // Through the text form: the style survives the writer and reader.
        let text = mortar_plumb::map::mortar::write(&m);
        mortar_plumb::map::parse(Format::Mortar, &text).unwrap()
    };
    let b1 = |out: &[u8], style: Option<&str>| {
        let r = verify_with(out, &ctx.files, &with_style(style));
        (r.with_rule("B1").count(), r.coverage.build_id.clone().unwrap_or_default())
    };
    let mut given = ctx.output.clone();
    for (k, b) in given[id..id + 20].iter_mut().enumerate() {
        *b = 0x10 + k as u8;
    }
    assert_eq!(b1(&given, None).0, 1, "(1) a given id with no style is judged as a hash");
    let (n, what) = b1(&given, Some("given"));
    assert_eq!(n, 0, "(2) {what}");
    assert!(what.contains("given"), "(2) {what}");
    let mut tree = ctx.output.clone();
    assert_eq!(seal(&mut tree, Tree::L128), 1);
    assert_eq!(b1(&tree, Some("sha1")).0, 1, "(3) the bare tree under sha1");
    assert_eq!(b1(&tree, Some("default")).0, 0, "(3) the bare tree under default");
    assert_eq!(b1(&tree, Some("fast")).0, 1, "(4) 20 bytes under fast");
}

// spec/plumb.md §9.5 (WP-028d): mutation coverage as a number on the small
// corpus. Every class the harness has is run, with a fixed seed, on the
// reference links of the cases below; the table and every missed mutant
// are printed, and the classes §9.5 states as fully caught must be.
#[test]
fn s9_5_mutation_coverage_on_the_small_corpus() {
    use mortar_plumb::mutate::{Config, Outcome, run};
    let Some(c) = small() else { return };
    let cfg = Config { per_class: 3, threads: 4, ..Config::default() };
    let mut total = Outcome::default();
    for case in ["start", "veneer", "bti", "got", "erratum", "types", "pieces"] {
        for linker in Linker::ALL {
            let ctx = Ctx::of(c.get(case, linker));
            let inputs: Vec<mortar_plumb::Input<'_>> =
                ctx.files.iter().map(|(p, b)| mortar_plumb::Input { path: p, bytes: b }).collect();
            let o = run(&ctx.output, &inputs, &ctx.map, &cfg).unwrap();
            total.add(&o, &ctx.link.label());
        }
    }
    let table = total.render();
    println!("{table}");
    let applied: usize = total.classes.iter().map(|r| r.applied).sum();
    assert!(applied > 100, "{table}");
    // §9.5's fully caught classes on this corpus; field-off-by-one's one
    // miss is mold's GOT load of an IFUNC moved to the IRELATIVE slot next
    // to it, which holds the same function once applied (equivalent).
    for class in [
        "field-off-by-one",
        "field-wrong-symbol",
        "field-wrong-addend",
        "stale-relaxation",
        "irelative-swap",
        "tls-segment",
        "property-bit",
        "section-overlap",
        "iplt-entry",
    ] {
        let r = total.classes.iter().find(|r| r.class == class).unwrap();
        let real: Vec<&String> = r
            .missed
            .iter()
            .filter(|m| {
                !(class == "field-off-by-one" && m.starts_with("bti/mold:") && m.contains("(f)"))
            })
            .collect();
        assert!(real.is_empty(), "{class}: {real:?}\n{table}");
    }
}

/// The output and input offsets of `.text` in the `erratum` case's
/// output and `e843.o`: its `W1` is at `.text + 0xff8` (`E843_S`).
fn e843_text(ctx: &Ctx) -> (u64, usize, usize) {
    let text = placed(&ctx.map, ".text");
    let at = file_offset(&ctx.output, text.addr).unwrap();
    let (_, obj) = ctx.files.iter().find(|(p, _)| p.ends_with("e843.o")).unwrap();
    let o = Object::parse(obj).unwrap();
    let s = o.sections().find(|s| s.name().ok() == Some(&b".text"[..])).unwrap();
    (text.addr, at, s.header().sh_offset as usize)
}

/// The C rules' findings of a report (§5.10), by id.
fn c_rules(r: &Report) -> Vec<String> {
    r.findings.iter().map(|f| f.rule.id()).filter(|r| r.starts_with('C')).collect()
}

fn rules_of(link: &Link, r: &Report) -> Vec<String> {
    let mut v: Vec<String> = unexpected(link, r).iter().map(|f| f.rule.id()).collect();
    v.sort();
    v.dedup();
    v
}

// spec/plumb.md §5.10 (WP-028d item 0; a64.md §11.2-§11.3): each erratum
// 843419 fix is judged by its meaning and by site. GNU ld writes the ADR
// form here, lld the patch. (1) An ADR to the next page, and a patch
// whose load lost its relocated offset (WP-032e's two mislinks): R1.
// (2) Either fix undone, with the option known: C1; with the option
// unknown, nothing shows it was given: not judged. (3) mold leaves the
// site (A4): C1, classified A4, when the option is known.
#[test]
#[allow(clippy::many_single_char_names)] // a64.md §11's Q, F, W1
fn s9_2o_erratum_fixes_are_judged_by_meaning_and_site() {
    let Some(c) = small() else { return };
    for linker in Linker::ALL {
        let ctx = Ctx::of(c.get("erratum", linker));
        let r = ctx.verify(&ctx.output);
        common::assert_clean(ctx.link, &r);
        let (addr, at, _) = e843_text(&ctx);
        let (q, f) = (at + 0xff8, at + 0x1000);
        let mut given = ctx.map.clone();
        given.fix_843419 = Some(true);
        let label = ctx.link.label();
        if linker == Linker::Mold {
            assert_eq!(r.coverage.erratum_not_judged, 1, "{label}: {}", r.summary());
            let m = verify_with(&ctx.output, &ctx.files, &given);
            assert!(m.with_rule("C1").next().is_some(), "{label}: {}", m.render("e"));
            assert!(unexpected(ctx.link, &m).is_empty(), "{label}: C1 is A4: {}", m.render("e"));
            continue;
        }
        assert_eq!(r.coverage.erratum_sites, 1, "{label}: {}", r.summary());
        let w1 = le32(&ctx.output, q);
        let wf = le32(&ctx.output, f);
        let mut out = ctx.output.clone();
        let mut undone = ctx.output.clone();
        if let Some((d, t)) = mortar_plumb::a64::adr(w1, addr + 0xff8) {
            assert_eq!(linker, Linker::Bfd);
            // The next page: adr's immediate + 0x1000 (immhi bit 10).
            put32(&mut out, q, w1 + (0x400 << 5));
            // The ADRP of the same page, at Q.
            let delta = ((t - ((addr + 0xff8) & !0xfff)) >> 12) as u32;
            put32(&mut undone, q, 0x9000_0000 | (delta & 3) << 29 | (delta >> 2) << 5 | d);
        } else {
            assert_eq!(linker, Linker::Lld, "{label}: neither fix");
            let (false, t) = mortar_plumb::a64::branch(wf, addr + 0x1000).unwrap() else {
                panic!("{label}: not a b")
            };
            let pt = file_offset(&ctx.output, t).unwrap();
            let moved = le32(&ctx.output, pt);
            assert_ne!(moved & (0xfff << 10), 0, "{label}: the load has its lo12");
            put32(&mut out, pt, moved & !(0xfff << 10));
            put32(&mut undone, f, moved);
        }
        let m = ctx.verify(&out);
        assert_eq!(rules_of(ctx.link, &m), ["R1"], "{label}: {}", m.render("e"));
        let m = verify_with(&undone, &ctx.files, &given);
        assert_eq!(rules_of(ctx.link, &m), ["C1"], "{label}: {}", m.render("e"));
        let m = ctx.verify(&undone);
        assert!(unexpected(ctx.link, &m).is_empty(), "{label}: {}", m.render("e"));
        assert_eq!(m.coverage.erratum_not_judged, 1, "{label}: {}", m.summary());
        eprintln!("§9.2 (o) {label}: next page / stale load R1, undone C1");
    }
}

// spec/plumb.md §5.10 (a64.md §11.3 items 4, 5, 7, 8): the rules on
// mortar's output only, the erratum links read as mortar's. (1) `W2` made
// a NOP (in the input too): the site is gone, so lld's patch is a
// redirect at no site (C3) and GNU ld's ADR a fix outside a site (C2).
// (2) The option known off: C6 on each fix. (3) A code chunk at page
// offset 0: C5.
#[test]
fn s9_2o_erratum_rules_on_mortar_output() {
    use mortar_plumb::map::{Chunk, ChunkKind};
    let Some(c) = small() else { return };
    for (linker, rule) in [(Linker::Bfd, "C2"), (Linker::Lld, "C3")] {
        let ctx = Ctx::of(c.get("erratum", linker));
        let text = mortar_plumb::map::mortar::write(&ctx.map);
        let map = mortar_plumb::map::parse(Format::Mortar, &text).unwrap();
        let label = ctx.link.label();
        let base = verify_with(&ctx.output, &ctx.files, &map);
        assert!(c_rules(&base).is_empty(), "{label} as mortar: {}", base.render("e"));
        let (addr, at, input) = e843_text(&ctx);
        let mut out = ctx.output.clone();
        let mut files = ctx.files.clone();
        put32(&mut out, at + 0xffc, 0xd503_201f);
        let (_, obj) = files.iter_mut().find(|(p, _)| p.ends_with("e843.o")).unwrap();
        put32(obj, input + 0xffc, 0xd503_201f);
        let m = verify_with(&out, &files, &map);
        assert_eq!(c_rules(&m), [rule], "{label}: {}", m.render("e"));
        let mut off = map.clone();
        off.fix_843419 = Some(false);
        let m = verify_with(&ctx.output, &ctx.files, &off);
        assert_eq!(c_rules(&m), ["C6"], "{label}: {}", m.render("e"));
        let mut chunk = map.clone();
        chunk.chunks.push(Chunk {
            kind: ChunkKind::Thunk,
            name: "__test_veneer".into(),
            addr: Some(addr + 0x1000),
            size: 8,
            target: None,
            line: 0,
        });
        let m = verify_with(&ctx.output, &ctx.files, &chunk);
        assert!(m.with_rule("C5").next().is_some(), "{label}: {}", m.render("e"));
    }
}

// spec/plumb.md §6.2 S2 and S17 (WP-059's F1): a writable PT_LOAD whose
// only section is `.bss` has an empty file image `[p_offset, p_offset)`.
// It lies inside any file whatever `p_offset` (S2: GNU ld's AArch64
// `-nostdlib` output puts it at 0xffe8 of a 1,736-byte file) and overlaps
// no section (S17: mortar's x86-64 one puts it inside `.comment`). Each
// shape, built from GNU ld's link of a `.bss`-only program, is ACCEPTED;
// the same segment with 8 file bytes is REJECTED by the rule.
#[test]
fn s6_2_s2_s17_an_empty_file_image_lies_anywhere() {
    let (Some(asm), Some(ld)) = (common::ARCH.tool("as"), common::ARCH.tool("ld.bfd")) else {
        return;
    };
    let dir = mortar_testutil::Scratch::new("plumb-empty-image");
    dir.write(
        "b.s",
        "\t.text\n\t.globl _start\n_start:\n\tadrp x0, w\n\tadd x0, x0, :lo12:w\n\tb .\n\
         \t.bss\n\t.balign 8\nw:\t.skip 8\n",
    );
    common::run_in(dir.path(), &asm, &["b.s", "-o", "b.o"]);
    common::run_in(dir.path(), &ld, &["-e", "_start", "-o", "out", "-Map=out.map", "b.o"]);
    let o = std::fs::read(dir.join("out")).unwrap();
    let text = std::fs::read_to_string(dir.join("out.map")).unwrap();
    let map = mortar_plumb::map::parse(Format::Gnu, &text).unwrap();
    let files = vec![("b.o".to_string(), std::fs::read(dir.join("b.o")).unwrap())];
    let check = |m: &[u8]| verify_with(m, &files, &map);
    let r = check(&o);
    assert!(r.findings.is_empty(), "{}", r.render("out"));
    let ph = phdrs(&o);
    let bss = ph.iter().find(|p| p.1 == PT_LOAD && p.2 & 2 != 0).unwrap();
    assert_eq!(bss.5, 0, "GNU ld's .bss segment has no file bytes");
    let align = le64(&o, bss.0 + 48);
    let with = |offset: u64, align: u64, filesz: u64| {
        let mut m = o.clone();
        put64(&mut m, bss.0 + 8, offset);
        put64(&mut m, bss.0 + 32, filesz);
        put64(&mut m, bss.0 + 48, align);
        check(&m)
    };
    // S2: past the end of the file, congruent.
    let past = bss.3 + align * (o.len() as u64 / align + 1);
    assert!(past > o.len() as u64);
    let r = with(past, align, 0);
    assert!(r.findings.is_empty(), "empty image past the end: {}", r.render("out"));
    expect_rule(&with(past, align, 8), "S2", "8 bytes past the end");
    // S17: inside `.symtab` (with a 16-byte alignment, congruent).
    let symtab = shdrs(&o).into_iter().find(|s| s.1 == ".symtab").unwrap();
    let inside = symtab.5 + 16 + (bss.4 - symtab.5 - 16) % 16;
    assert!(symtab.5 < inside && inside < symtab.5 + symtab.6);
    let r = with(inside, 16, 0);
    assert!(r.findings.is_empty(), "empty image inside .symtab: {}", r.render("out"));
    expect_rule(&with(inside, 16, 8), "S17", "8 bytes inside .symtab");
}

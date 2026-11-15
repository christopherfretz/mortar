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
#[allow(clippy::many_single_char_names)]
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
        // S7: the entry point outside the code.
        expect_rule(&mutate(&|m| put64(m, 24, 0)), "S7", &what);
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
        let _ = SHT_NOBITS;
    }
}

/// The lld map of the start link, in mortar's format: the base of the map
/// rule tests (its records are exact: no linker-made section is named after
/// an input).
fn mortar_map(c: &'static Corpus) -> (Ctx, Map) {
    let link = c.get("start", Linker::Lld);
    let ctx = Ctx::of(link);
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

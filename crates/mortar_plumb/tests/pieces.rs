//! `synth.md` S-17 and S-19 under mortar's map (`spec/plumb.md` §4.4,
//! §5.6): a reference to a section symbol of a section placed as pieces
//! names the piece holding byte `st_value + A`; a reference to a named
//! symbol names the piece holding `st_value`, and `A` is added after.
//!
//! The reference maps do not list merge pieces, so the `pieces` case of
//! the corpus is rewritten as mortar's map: each string of each input
//! `.rodata.str1.1` becomes a `piece` record at the address where the
//! linker put that string (found by content; no string of the case is a
//! suffix of another, so each has one place).

mod common;

use common::{Linker, Size, corpus, verify_with};
use mortar_plumb::map::{Format, Map, Placed, SectionRef};

/// The output address of each occurrence of `needle` in an allocated
/// section with file contents.
fn addrs_of(output: &[u8], needle: &[u8]) -> Vec<u64> {
    let h = mortar_elf::FileHeader::parse(output).unwrap();
    let table = mortar_elf::SectionTable::parse(output, &h).unwrap();
    let mut found = Vec::new();
    for s in &table {
        let sh = s.header();
        if sh.sh_addr == 0 || sh.sh_type == 8 {
            continue;
        }
        let (off, size) = (sh.sh_offset as usize, sh.sh_size as usize);
        let Some(bytes) = output.get(off..off + size) else { continue };
        for (i, w) in bytes.windows(needle.len()).enumerate() {
            if w == needle {
                found.push(sh.sh_addr + i as u64);
            }
        }
    }
    found
}

/// `link`'s map as mortar's, with the strings of `.rodata.str1.1` of each
/// `pieces_*.o` placed as pieces; `shift` moves the piece at `(file,
/// input offset)` by one byte.
fn piece_map(link: &common::Link, shift: Option<(&str, u64)>) -> (Map, Vec<(String, Vec<u8>)>) {
    let mut map = link.parse_map();
    let files = link.inputs(&map);
    let output = link.output_bytes();
    map.format = Format::Mortar;
    let sec = SectionRef::Name(".rodata.str1.1".into());
    map.placed.retain(|p| p.section != sec);
    for (fi, f) in map.files.clone().iter().enumerate() {
        if !f.path.starts_with("pieces_") {
            continue;
        }
        let bytes = &files.iter().find(|(p, _)| *p == f.path).unwrap().1;
        let h = mortar_elf::FileHeader::parse(bytes).unwrap();
        let table = mortar_elf::SectionTable::parse(bytes, &h).unwrap();
        let s = table.iter().find(|s| s.name().is_ok_and(|n| n == b".rodata.str1.1")).unwrap();
        let sh = s.header();
        let data = &bytes[sh.sh_offset as usize..(sh.sh_offset + sh.sh_size) as usize];
        let mut at = 0usize;
        for piece in data.split_inclusive(|&b| b == 0) {
            let a = addrs_of(&output, piece);
            assert_eq!(a.len(), 1, "{}: {:?} placed {} times", link.label(), piece, a.len());
            let moved = shift == Some((f.path.as_str(), at as u64));
            map.placed.push(Placed {
                file: fi as u32,
                section: sec.clone(),
                input_offset: Some(at as u64),
                addr: a[0] + u64::from(moved),
                size: piece.len() as u64,
                line: 0,
            });
            at += piece.len();
        }
    }
    // Through the text form, as mortar writes it.
    let text = mortar_plumb::map::mortar::write(&map);
    (mortar_plumb::map::parse(Format::Mortar, &text).unwrap(), files)
}

/// The findings about relocated fields and pieces (the mortar-only layout
/// rules S2, S18 and B1 do not hold of a reference output).
fn field_findings(r: &mortar_plumb::Report) -> Vec<String> {
    ["R1", "R4", "R5", "R6", "R7"]
        .iter()
        .flat_map(|id| r.with_rule(id))
        .map(|f| format!("{} {:?} {}", f.rule.id(), f.symbol, f.message))
        .collect()
}

// synth.md S-17, S-19; spec/plumb.md §4.4 ("pieces").
#[test]
fn s17_section_symbol_addend_selects_the_piece() {
    let Some(c) = corpus(Size::Small) else { return };
    for linker in Linker::ALL {
        let link = c.get("pieces", linker);
        let output = link.output_bytes();
        let (map, files) = piece_map(link, None);
        let r = verify_with(&output, &files, &map);
        let bad = field_findings(&r);
        assert!(bad.is_empty(), "{}:\n{}\n{}", link.label(), bad.join("\n"), r.render("o"));
        assert!(r.coverage.sections_pieces >= 2, "{}: no pieces", link.label());
        assert!(r.coverage.failed == 0, "{}:\n{}", link.label(), r.render("o"));
        // A piece of the second unit moved by a byte: every reference to it
        // by section symbol plus addend is wrong now (B's `charlie` at 5 is
        // reached by `+8` and `+5`; `yankee` at 13 by `+14`).
        for (file, at) in [("pieces_b.o", 5), ("pieces_b.o", 13), ("pieces_a.o", 6)] {
            let (map, files) = piece_map(link, Some((file, at)));
            let r = verify_with(&output, &files, &map);
            assert!(
                r.with_rule("R1").next().is_some(),
                "{}: {file}+{at} moved is not found:\n{}",
                link.label(),
                r.render("o")
            );
        }
    }
}

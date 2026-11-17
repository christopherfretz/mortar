//! L1 tests of `.eh_frame` and `.eh_frame_hdr` (`spec/synth.md` §4.2 to
//! §4.5), the review findings that concern them, and their determinism.

mod common;

use common::{Eh, R, collector, fde, text, walk};
use mortar_core::par::{Pool, check_schedules};
use mortar_core::{FileId, SectionId};
use mortar_synth::eh::{CodeRef, EhPlan, EhValue, EhWho};
use mortar_synth::hdr::{HdrAddrs, HdrPlan, OVERLAPPING};
use mortar_synth::out::Window;

/// A CIE body (version, augmentation "zR", alignment factors, RA
/// register, augmentation data) of `len` bytes, distinguished by `tag`.
fn cie(tag: u8, len: usize) -> R {
    let mut b = vec![1, b'z', b'R', 0, 4, 0x78, 30, 1, 0x1b];
    b.push(tag);
    b.resize(len, 0);
    R::Cie(b)
}

fn plan(src: &Eh, dead: &[u32]) -> EhPlan {
    let diag = collector();
    let dead: Vec<SectionId> = dead.iter().map(|&s| SectionId(s)).collect();
    let p = EhPlan::new(src, |s| !dead.contains(&s), &Pool::sequential(), &diag);
    let report = diag.finish();
    assert!(!report.failed(), "{}", text(&report));
    p.unwrap()
}

fn bytes(p: &EhPlan, src: &Eh) -> Vec<u8> {
    let mut out = vec![0xaa; p.size() as usize];
    p.write(src, &mut Window::new(0, &mut out)).unwrap();
    out
}

/// Two objects each with a CIE and FDEs for code sections 10.. and 20...
fn two_objects() -> Eh {
    let mut eh = Eh::default();
    eh.add(1, 1, &[cie(0, 16), fde(0, 10, 0), fde(0, 11, 0)]);
    eh.add(2, 2, &[cie(0, 16), fde(0, 20, 0), fde(0, 21, 0)]);
    eh
}

#[test]
fn s41_fde_of_dead_code_is_dropped() {
    let eh = two_objects();
    let p = plan(&eh, &[11]);
    assert_eq!(p.fde_count(), 3);
    let (recs, term) = walk(&bytes(&p, &eh));
    assert!(term);
    assert_eq!(recs.len(), 4, "one CIE (merged) and three FDEs");
}

#[test]
fn s42_finding14_cie_without_a_live_fde_is_dropped() {
    let mut eh = Eh::default();
    eh.add(1, 1, &[cie(0, 16), fde(0, 10, 0)]);
    eh.add(2, 2, &[cie(9, 16), fde(0, 20, 0)]);
    let p = plan(&eh, &[20]);
    let (recs, _) = walk(&bytes(&p, &eh));
    assert_eq!(recs.len(), 2);
    assert!(p.records().iter().all(|r| r.section == SectionId(1)));
}

#[test]
fn s43_equal_cies_are_merged_and_fdes_point_at_the_first() {
    let eh = two_objects();
    let p = plan(&eh, &[]);
    let out = bytes(&p, &eh);
    let (recs, _) = walk(&out);
    assert_eq!(recs.len(), 5);
    assert_eq!(recs[0].2, 0, "a CIE");
    // S-52: every FDE's pointer is the distance back to offset 0.
    for &(at, _, ptr) in &recs[1..] {
        assert_eq!(ptr as usize, at + 4);
    }
}

#[test]
fn s43_cies_with_different_bytes_or_relocations_are_kept() {
    let mut eh = Eh::default();
    eh.add(1, 1, &[cie(0, 16), fde(0, 10, 0)]);
    eh.add(2, 2, &[cie(1, 16), fde(0, 20, 0)]);
    eh.add(3, 3, &[cie(0, 16), fde(0, 30, 0)]);
    // S-43: equal bytes, relocations against different targets.
    eh.reloc(0, 0, 12, 50).reloc(2, 0, 12, 51);
    let p = plan(&eh, &[]);
    let (recs, _) = walk(&bytes(&p, &eh));
    assert_eq!(recs.iter().filter(|r| r.2 == 0).count(), 3);
}

#[test]
fn s44_s55_finding13_internal_records_are_last_and_not_merged() {
    let mut eh = Eh::default();
    eh.add(1, 0, &[cie(0, 16), fde(0, 5, 0)]);
    eh.add(2, 1, &[cie(0, 16), fde(0, 10, 0)]);
    let p = plan(&eh, &[]);
    let secs: Vec<u32> = p.records().iter().map(|r| r.section.0).collect();
    assert_eq!(secs, [2, 2, 1, 1], "the internal file's records follow, with their own CIE");
    let (recs, _) = walk(&bytes(&p, &eh));
    assert_eq!(recs[3].2 as usize, recs[3].0 + 4 - recs[2].0, "CIE_delta stays positive");
}

#[test]
fn s47_finding14_record_lengths_are_raised_to_a_multiple_of_4() {
    let mut eh = Eh::default();
    eh.add(1, 1, &[cie(0, 13), R::Fde { cie: 0, code: Some((10, 0)), extra: 1, empty: false }]);
    let p = plan(&eh, &[]);
    let out = bytes(&p, &eh);
    let (recs, term) = walk(&out);
    assert!(term);
    assert_eq!(recs[0].1, 20, "4 + 13 raised to 20");
    assert_eq!(recs[1].1, 20, "16 + 1 raised to 20");
    assert_eq!(&out[4 + 17..4 + 20], &[0, 0, 0], "zero padding");
    assert_eq!(p.size(), 24 + 24 + 4);
}

#[test]
fn s48_one_terminator_and_input_terminators_dropped() {
    let mut eh = Eh::default();
    eh.add(1, 1, &[cie(0, 16), fde(0, 10, 0)]);
    let p = plan(&eh, &[]);
    let out = bytes(&p, &eh);
    assert_eq!(&out[out.len() - 4..], &[0; 4]);
    assert_eq!(walk(&out).0.len(), 2);
}

#[test]
fn s48a_wp142_a_terminator_alone_when_no_record_survives() {
    // crtend.o's terminator, the only input: the chunk exists, 4 bytes,
    // and `__FRAME_END__` (offset 0) and a symbol at its end (offset 4)
    // lie at its start (S-50 rules 1 and 3).
    let mut eh = Eh::default();
    eh.add(1, 1, &[R::Term]);
    let p = plan(&eh, &[]);
    assert!(p.exists());
    assert_eq!((p.size(), p.fde_count()), (4, 0));
    assert_eq!(bytes(&p, &eh), [0; 4]);
    let sym = EhWho::Symbol { droppable: false };
    assert_eq!(p.value(&eh, SectionId(1), 0, sym), Ok(EhValue::Offset(0)));
    assert_eq!(p.value(&eh, SectionId(1), 4, sym), Ok(EhValue::Offset(0)));
    // Two terminators make one; an empty input (x86 crtbeginT.o's) is a
    // frame list too, alone or not.
    eh.add(2, 2, &[R::Term]);
    let p = plan(&eh, &[]);
    assert_eq!(bytes(&p, &eh), [0; 4]);
    let mut empty = Eh::default();
    empty.add(1, 1, &[]);
    let p = plan(&empty, &[]);
    assert_eq!((p.exists(), p.size()), (true, 4));
    assert_eq!(p.value(&empty, SectionId(1), 0, EhWho::Reloc), Ok(EhValue::Offset(0)));
    // Records that all die, with no terminator and no empty input: no
    // chunk (S-3); with crtend.o's terminator after them: the terminator.
    let mut dead = Eh::default();
    dead.add(1, 1, &[cie(0, 16), fde(0, 10, 0)]);
    assert!(!plan(&dead, &[10]).exists());
    dead.add(2, 2, &[R::Term]);
    let p = plan(&dead, &[10]);
    assert_eq!((p.exists(), p.size(), p.records().len()), (true, 4, 0));
    assert_eq!(p.value(&dead, SectionId(1), 0, EhWho::Reloc), Ok(EhValue::Offset(0)));
    // No input at all: no chunk.
    assert!(!plan(&Eh::default(), &[]).exists());
}

#[test]
fn s38_finding5_fde_without_initial_location_is_dropped() {
    let mut eh = Eh::default();
    eh.add(
        1,
        1,
        &[cie(0, 16), R::Fde { cie: 0, code: None, extra: 0, empty: false }, fde(0, 10, 0)],
    );
    let p = plan(&eh, &[]);
    assert_eq!(p.fde_count(), 1);
}

#[test]
fn s49_slots_eh_records_name_input_records() {
    let eh = two_objects();
    let p = plan(&eh, &[10]);
    let rows: Vec<(u32, u32)> = p.records().iter().map(|r| (r.section.0, r.piece)).collect();
    assert_eq!(rows, [(1, 0), (1, 2), (2, 1), (2, 2)]);
}

#[test]
fn s50_rule1_reference_at_zero_is_the_chunk_start() {
    let eh = two_objects();
    let p = plan(&eh, &[]);
    assert_eq!(p.value(&eh, SectionId(2), 0, EhWho::Reloc), Ok(EhValue::Offset(0)));
}

#[test]
fn s50_rule2_reference_inside_a_kept_record() {
    let eh = two_objects();
    let p = plan(&eh, &[]);
    // Section 2's first FDE is at input 24; its CIE was merged away, so
    // the FDE is output record 3 at 24 + 20 + 20 = 64.
    assert_eq!(p.value(&eh, SectionId(2), 24 + 8, EhWho::Reloc), Ok(EhValue::Offset(64 + 8)));
}

#[test]
fn s50_finding3_reference_at_the_end_of_an_empty_section() {
    let mut eh = Eh::default();
    eh.add(1, 1, &[cie(0, 16), fde(0, 10, 0)]);
    eh.add(2, 2, &[]);
    eh.add(3, 3, &[cie(1, 16), fde(0, 30, 0)]);
    let p = plan(&eh, &[]);
    // crtbeginT's `__EH_FRAME_BEGIN__` at offset 0 of an empty section.
    assert_eq!(p.value(&eh, SectionId(2), 0, EhWho::Reloc), Ok(EhValue::Offset(0)));
    // r == sh_size of a non-empty section: the next kept record.
    assert_eq!(p.value(&eh, SectionId(1), 44, EhWho::Reloc), Ok(EhValue::Offset(44)));
    assert_eq!(
        p.value(&eh, SectionId(3), 44, EhWho::Reloc),
        Ok(EhValue::Offset(88)),
        "the terminator"
    );
}

#[test]
fn s50_finding4_mapping_symbol_in_a_removed_cie_is_dropped() {
    let eh = two_objects();
    let p = plan(&eh, &[]);
    assert_eq!(p.value(&eh, SectionId(2), 0x16, EhWho::Mapping), Ok(EhValue::Dropped));
    assert_eq!(p.value(&eh, SectionId(1), 0x16, EhWho::Mapping), Ok(EhValue::Offset(0x16)));
}

#[test]
fn s50_finding12_symbol_at_a_removed_record_takes_the_next_kept_record() {
    let eh = two_objects();
    let p = plan(&eh, &[20]);
    // Section 2's CIE (merged) and first FDE (dead) are gone; a symbol at
    // the CIE's start takes the next kept record: the second FDE.
    let next = p.value(&eh, SectionId(2), 24, EhWho::Symbol { droppable: false });
    assert_eq!(next, Ok(EhValue::Offset(64)));
    let inside = p.value(&eh, SectionId(2), 4, EhWho::Symbol { droppable: false });
    assert_eq!(inside, Ok(EhValue::Offset(64)));
    let local = p.value(&eh, SectionId(2), 4, EhWho::Symbol { droppable: true });
    assert_eq!(local, Ok(EhValue::Dropped));
}

#[test]
fn s50_rule4_relocation_into_a_removed_record_is_refused() {
    let eh = two_objects();
    let p = plan(&eh, &[20]);
    let e = p.value(&eh, SectionId(2), 30, EhWho::Reloc).unwrap_err();
    let d = e.into_diagnostic();
    assert_eq!(d.id(), "malformed-input");
    let rendered = d.render(&mortar_diag::Style::plain("ld"));
    assert_eq!(rendered, "ld: error: reference into a removed .eh_frame record\n");
}

#[test]
fn s50_rule5_offset_outside_the_section_is_refused() {
    let eh = two_objects();
    let p = plan(&eh, &[]);
    let rel = p.value(&eh, SectionId(1), 65, EhWho::Reloc).unwrap_err().into_diagnostic();
    assert_eq!(
        rel.render(&mortar_diag::Style::plain("ld")),
        "ld: error: offset is outside the section; refer to it through a named symbol\n"
    );
    let sym = p
        .value(&eh, SectionId(1), -1, EhWho::Symbol { droppable: false })
        .unwrap_err()
        .into_diagnostic();
    assert_eq!(
        sym.render(&mortar_diag::Style::plain("ld")),
        "ld: error: offset is outside the section\n"
    );
}

#[test]
fn s52_place_of_an_input_offset() {
    let eh = two_objects();
    let p = plan(&eh, &[]);
    assert_eq!(p.place(&eh, SectionId(2), 24 + 8), Some(64 + 8), "FDE pc field");
    assert_eq!(p.place(&eh, SectionId(2), 4), None, "the merged CIE");
}

#[test]
fn s54_output_type_is_the_shared_type() {
    let mut eh = two_objects();
    eh.info[0].sh_type = 0x7000_0001;
    let p = plan(&eh, &[]);
    assert_eq!(p.sh_type(), mortar_elf::consts::SHT_PROGBITS);
    eh.info[1].sh_type = 0x7000_0001;
    assert_eq!(plan(&eh, &[]).sh_type(), 0x7000_0001);
}

#[test]
fn s4_i103_every_split_gives_the_same_bytes() {
    let mut eh = Eh::default();
    for s in 0..8u32 {
        eh.add(
            1 + s,
            1 + s,
            &[cie((s % 3) as u8, 13 + s as usize), fde(0, 100 + s, 0), fde(0, 200 + s, 0)],
        );
    }
    let p = plan(&eh, &[201, 203]);
    let whole = bytes(&p, &eh);
    for step in [1usize, 3, 4, 7, 64] {
        let mut out = vec![0xaa; whole.len()];
        for (k, chunk) in out.chunks_mut(step).enumerate() {
            p.write(&eh, &mut Window::new((k * step) as u64, chunk)).unwrap();
        }
        assert_eq!(out, whole, "split by {step}");
    }
}

#[test]
fn eh_plan_and_bytes_do_not_depend_on_the_schedule() {
    let mut eh = Eh::default();
    for s in 0..64u32 {
        eh.add(
            1 + s,
            1 + s % 5,
            &[cie((s % 4) as u8, 16), fde(0, 1000 + s, 0), fde(0, 2000 + s, 0)],
        );
    }
    check_schedules(|pool| {
        let diag = collector();
        let p = EhPlan::new(&eh, |s| s.0 % 7 != 0, pool, &diag).unwrap();
        let mut out = vec![0; p.size() as usize];
        p.write(&eh, &mut Window::new(0, &mut out)).unwrap();
        (p, out)
    });
}

#[test]
fn eh_plan_refuses_unsorted_sections() {
    let mut eh = Eh::default();
    eh.add(1, 1, &[cie(0, 16)]);
    eh.add(2, 1, &[cie(0, 16)]);
    eh.frames.sections.swap(0, 1);
    let diag = collector();
    assert!(EhPlan::new(&eh, |_| true, &Pool::sequential(), &diag).is_err());
    assert_eq!(diag.finish().diagnostics()[0].id(), "internal-error");
}

// ---- .eh_frame_hdr ----------------------------------------------------------

fn hdr(eh: &Eh, p: &EhPlan) -> Result<Option<HdrPlan>, String> {
    let diag = collector();
    let h = HdrPlan::new(eh, p, true, &Pool::sequential(), &diag);
    let r = diag.finish();
    h.map_err(|_| text(&r))
}

/// Code section `s` is at `0x1000 × s`.
fn code_addr(c: CodeRef) -> Option<u64> {
    0x1000u64.checked_mul(u64::from(c.section.0)).map(|a| a.wrapping_add_signed(c.offset))
}

#[test]
fn s57_no_option_no_table() {
    let eh = two_objects();
    let p = plan(&eh, &[]);
    let diag = collector();
    assert_eq!(HdrPlan::new(&eh, &p, false, &Pool::sequential(), &diag), Ok(None));
}

#[test]
fn s58_s59_header_and_sorted_table_without_empty_ranges() {
    let mut eh = Eh::default();
    eh.add(1, 1, &[cie(0, 16), fde(0, 30, 0), fde(0, 10, 0)]);
    eh.add(2, 2, &[cie(0, 16), R::Fde { cie: 0, code: Some((20, 0)), extra: 0, empty: true }]);
    let p = plan(&eh, &[]);
    let h = hdr(&eh, &p).unwrap().unwrap();
    assert_eq!(h.entries(), 2);
    assert_eq!(h.size(), 12 + 16);
    let mut out = vec![0; h.size() as usize];
    let at = HdrAddrs { hdr: 0x8000, eh: 0x9000 };
    h.write(&eh, &p, at, code_addr, &Pool::sequential(), &mut out).unwrap();
    assert_eq!(&out[..4], &[1, 0x1b, 0x03, 0x3b]);
    let word = |i: usize| i32::from_le_bytes(out[i..i + 4].try_into().unwrap());
    assert_eq!(word(4), 0x9000 - 0x8004, "eh_frame_ptr, pcrel from its field");
    assert_eq!(word(8), 2, "fde_count");
    assert_eq!(
        (word(12), word(16)),
        (0xa000 - 0x8000, 0x9000 + 44 - 0x8000),
        "sorted: section 10 first"
    );
    assert_eq!((word(20), word(24)), (0x1e000 - 0x8000, 0x9000 + 24 - 0x8000));
}

#[test]
fn s59_two_fdes_at_one_address_are_refused() {
    let mut eh = Eh::default();
    eh.add(1, 1, &[cie(0, 16), fde(0, 10, 4), fde(0, 10, 4)]);
    let p = plan(&eh, &[]);
    let e = hdr(&eh, &p).unwrap_err();
    // S-59: `malformed-input` with GNU's message, at plan time.
    assert_eq!(e, format!("ld: error: {OVERLAPPING}\n"));
}

#[test]
fn s62_an_entry_out_of_sdata4_range_is_an_error() {
    let mut eh = Eh::default();
    eh.add(1, 1, &[cie(0, 16), fde(0, 10, 0)]);
    let p = plan(&eh, &[]);
    let h = hdr(&eh, &p).unwrap().unwrap();
    let mut out = vec![0; h.size() as usize];
    let far = |_: CodeRef| Some(0x1_0000_0000u64 + 0x8000);
    let at = HdrAddrs { hdr: 0x8000, eh: 0x9000 };
    let e = h.write(&eh, &p, at, far, &Pool::sequential(), &mut out).unwrap_err().into_diagnostic();
    assert_eq!(e.id(), "relocation-out-of-range");
}

#[test]
fn hdr_does_not_depend_on_the_schedule() {
    let mut eh = Eh::default();
    for s in 0..40u32 {
        eh.add(1 + s, 1 + s, &[cie(0, 16), fde(0, 500 - s, 0), fde(0, 600 + s, 8)]);
    }
    check_schedules(|pool| {
        let diag = collector();
        let p = EhPlan::new(&eh, |_| true, pool, &diag).unwrap();
        let h = HdrPlan::new(&eh, &p, true, pool, &diag).unwrap().unwrap();
        let mut out = vec![0; h.size() as usize];
        h.write(&eh, &p, HdrAddrs { hdr: 0x10, eh: 0x100 }, code_addr, pool, &mut out).unwrap();
        out
    });
}

#[test]
fn internal_file_id_is_zero() {
    assert_eq!(FileId::INTERNAL, FileId(0));
}

fn list(rows: &[(u32, u32)]) -> Vec<mortar_core::layout::EhRecord> {
    rows.iter()
        .map(|&(s, p)| mortar_core::layout::EhRecord { section: SectionId(s), piece: p })
        .collect()
}

// spec: synth.md S-49 (the plan follows the list: `select` then
// `from_records` is `new`; an equal CIE that the list keeps is taken by
// its section's FDEs, the CIE rule)
#[test]
fn s49_the_plan_follows_the_list() {
    let eh = two_objects();
    let diag = collector();
    let pool = Pool::sequential();
    let chosen = EhPlan::select(&eh, |_| true, &pool, &diag).unwrap();
    assert_eq!(chosen, list(&[(1, 0), (1, 1), (1, 2), (2, 1), (2, 2)]));
    assert_eq!(EhPlan::from_records(&eh, &chosen, &pool, &diag).unwrap(), plan(&eh, &[]));
    // Section 2's CIE kept: its FDEs take it, section 1's take theirs.
    let kept = list(&[(1, 0), (1, 1), (1, 2), (2, 0), (2, 1), (2, 2)]);
    let p = EhPlan::from_records(&eh, &kept, &pool, &diag).unwrap();
    assert!(!diag.finish().failed());
    assert_eq!(p.records(), &kept[..]);
    assert_eq!(
        (p.cie_of(1), p.cie_of(2), p.cie_of(4), p.cie_of(5)),
        (Some(0), Some(0), Some(3), Some(3))
    );
    assert_eq!(p.size(), plan(&eh, &[]).size() + 24, "one more CIE of 4 + 20 bytes");
    let (recs, term) = walk(&bytes(&p, &eh));
    assert!(term);
    assert_eq!(recs.iter().filter(|r| r.2 == 0).count(), 2);
    for &(at, _, ptr) in &recs[4..] {
        assert_eq!(ptr as usize, at + 4 - recs[3].0, "section 2's FDEs point at its CIE");
    }
}

// spec: synth.md S-49 (a list the rules do not admit fails by name)
#[test]
fn s49_lists_the_rules_do_not_admit_fail_by_name() {
    let eh = two_objects();
    let mut internal = Eh::default();
    internal.add(1, 0, &[cie(0, 16), fde(0, 5, 0)]);
    internal.add(2, 1, &[cie(0, 16), fde(0, 10, 0)]);
    let cases: [(&Eh, Vec<_>, &str); 7] = [
        (&eh, list(&[(1, 0), (1, 1), (3, 0)]), "section 3 is no live .eh_frame"),
        (&eh, list(&[(1, 0), (1, 1), (1, 7)]), "section 1 has no record 7"),
        (&eh, list(&[(2, 0), (2, 1), (1, 0), (1, 1)]), "S-46's order (sections)"),
        (&eh, list(&[(1, 0), (1, 2), (1, 1)]), "S-46's order (records)"),
        (&eh, list(&[(1, 0), (1, 1), (1, 1)]), "given twice"),
        (&eh, list(&[(1, 1), (2, 0), (2, 1)]), "no CIE to take"),
        (&eh, list(&[(1, 0), (1, 1), (2, 0)]), "a CIE no listed FDE takes"),
    ];
    for (src, rows, what) in
        cases.iter().chain(&[(&internal, list(&[(2, 0), (2, 1), (1, 1)]), "no CIE to take")])
    {
        let diag = collector();
        let p = EhPlan::from_records(*src, rows, &Pool::sequential(), &diag);
        let report = diag.finish();
        assert!(p.is_err(), "{rows:?} accepted");
        let t = text(&report);
        assert!(t.contains("EH_RECORDS row") && t.contains(what), "{rows:?}: {t}");
    }
}

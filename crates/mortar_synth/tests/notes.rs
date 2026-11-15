//! L1 tests of notes (`spec/synth.md` §8, S-126), `.comment` (S-100),
//! `.shstrtab` (S-90) and the chunk list (S-3, S-108).

mod common;

use common::{collector, prop_note, text};
use mortar_arch::{Arch, PropMerge};
use mortar_core::plan::BuildId;
use mortar_core::{ChunkId, SectionId};
use mortar_elf::Class;
use mortar_synth::chunks::{SynthKind, chunk_ids};
use mortar_synth::image::{Division, SynthChunk};
use mortar_synth::notes::{
    BuildIdPlan, COMMENT, HEX_NOTE, MEMORY_SEAL, NO_COPY_ON_PROTECTED, ObjectNotes, PAUTH_DIFFERS,
    PropertyPlan, STACK_SIZE, check_note_alignment,
};
use mortar_synth::out::Window;
use mortar_synth::shstrtab::ShstrtabPlan;

const FEATURE_1_AND: u32 = 0xc000_0000;
const PAUTH: u32 = 0xc000_0001;

fn build_id(style: Option<&BuildId>) -> Result<Option<BuildIdPlan>, String> {
    let diag = collector();
    let p = BuildIdPlan::new(style, &diag);
    let r = diag.finish();
    p.map_err(|_| text(&r))
}

#[test]
fn s91_descriptor_size_per_style() {
    let n = |s| build_id(Some(&s)).unwrap().unwrap().desc_size();
    assert_eq!(n(BuildId::Default), 20);
    assert_eq!(n(BuildId::Sha1), 20);
    assert_eq!(n(BuildId::Fast), 8);
    assert_eq!(n(BuildId::Sha256), 32);
    assert_eq!(n(BuildId::Hex("0xABcd".into())), 2);
    assert_eq!(build_id(None), Ok(None), "none, --no-build-id, absent");
}

#[test]
fn s91_i71_size_is_padded_to_4() {
    let p = build_id(Some(&BuildId::Hex("0xabcdef".into()))).unwrap().unwrap();
    assert_eq!(p.size(), 16 + 4, "a 3-byte id: 16 + align_up(3, 4)");
    let mut out = vec![0xaa; p.size() as usize];
    p.write(&mut Window::new(0, &mut out));
    assert_eq!(&out[..16], b"\x04\0\0\0\x03\0\0\0\x03\0\0\0GNU\0");
    assert_eq!(&out[16..], &[0xab, 0xcd, 0xef, 0]);
    assert_eq!(SynthChunk::sealed(&p), None, "0xHEX is not sealed");
    let sha = build_id(Some(&BuildId::Sha1)).unwrap().unwrap();
    assert_eq!(SynthChunk::sealed(&sha), Some(16..36), "I-107: the descriptor");
    assert_eq!(sha.division(), Division::Whole { max_size: 36 });
}

#[test]
fn s91_md5_is_refused_with_a_note() {
    assert_eq!(
        build_id(Some(&BuildId::Md5)),
        Err("ld: error: unsupported option value: --build-id=md5\n>>> use sha1\n".into())
    );
}

#[test]
fn s91_uuid_is_m2_and_refused() {
    assert_eq!(
        build_id(Some(&BuildId::Uuid)),
        Err("ld: error: unsupported option value: --build-id=uuid\n".into())
    );
}

#[test]
fn s91_i70_bad_hex_is_invalid_option_value() {
    for bad in ["0xabc", "0x", "0xzz", "0Xab", "ab"] {
        let e = build_id(Some(&BuildId::Hex(bad.into()))).unwrap_err();
        assert_eq!(e, format!("ld: error: --build-id: invalid value '{bad}'\n>>> {HEX_NOTE}\n"));
    }
}

fn props(objects: &[Vec<Vec<u8>>]) -> Result<PropertyPlan, String> {
    let arch = mortar_a64::A64::M1;
    let secs: Vec<Vec<(SectionId, &[u8], u64)>> = objects
        .iter()
        .enumerate()
        .map(|(o, notes)| {
            notes
                .iter()
                .enumerate()
                .map(|(k, n)| (SectionId((o * 10 + k) as u32 + 1), &n[..], 8))
                .collect()
        })
        .collect();
    let paths: Vec<String> = (0..objects.len()).map(|o| format!("o{o}.o")).collect();
    let objs: Vec<ObjectNotes<'_>> =
        secs.iter().zip(&paths).map(|(s, p)| ObjectNotes { path: p, sections: s }).collect();
    let diag = collector();
    let p = PropertyPlan::new(&objs, Class::Elf64, &|t| arch.property_merge(t), false, &diag);
    let r = diag.finish();
    p.map_err(|_| text(&r))
}

fn u32b(v: u32) -> [u8; 4] {
    v.to_le_bytes()
}

#[test]
fn s95_and_survives_only_when_every_input_has_it() {
    let bti = prop_note(&[(FEATURE_1_AND, &u32b(3))]);
    let bti1 = prop_note(&[(FEATURE_1_AND, &u32b(1))]);
    let p = props(&[vec![bti.clone()], vec![bti1]]).unwrap();
    assert_eq!(p.value_u32(FEATURE_1_AND), 1);
    let p = props(&[vec![bti], vec![]]).unwrap();
    assert!(!p.exists(), "an object without a note has every property absent");
    assert_eq!(p.size(), 0);
}

#[test]
fn s95_finding2_no_copy_on_protected_is_or_over_those_that_have_it() {
    let nc = prop_note(&[(NO_COPY_ON_PROTECTED, &[])]);
    let p = props(&[vec![nc], vec![]]).unwrap();
    assert_eq!(p.properties(), &[(NO_COPY_ON_PROTECTED, Box::<[u8]>::from(&[][..]))]);
}

#[test]
fn s95_finding2_stack_size_is_the_largest_present() {
    let a = prop_note(&[(STACK_SIZE, &0x1000u64.to_le_bytes())]);
    let b = prop_note(&[(STACK_SIZE, &0x8000u64.to_le_bytes())]);
    let p = props(&[vec![a], vec![b], vec![]]).unwrap();
    assert_eq!(&*p.properties()[0].1, &0x8000u64.to_le_bytes());
}

#[test]
fn s95_finding6_generic_ranges_and_or() {
    let needed = prop_note(&[(0xb000_8000, &u32b(1)), (0xb000_0000, &u32b(5))]);
    let other = prop_note(&[(0xb000_0000, &u32b(4))]);
    let p = props(&[vec![needed], vec![other]]).unwrap();
    assert_eq!(
        p.properties().iter().map(|p| p.0).collect::<Vec<_>>(),
        [0xb000_0000, 0xb000_8000],
        "ascending; And kept as 5 & 4, Or kept though one input lacks it"
    );
    assert_eq!(p.value_u32(0xb000_0000), 4);
    assert_eq!(p.value_u32(0xb000_8000), 1);
}

#[test]
fn s95_unknown_type_is_dropped_with_a_warning() {
    let n = prop_note(&[(0xc000_0002, &u32b(3)), (FEATURE_1_AND, &u32b(1))]);
    let arch = mortar_a64::A64::M1;
    let secs = [(SectionId(1), &n[..], 8u64)];
    let objs = [ObjectNotes { path: "a.o", sections: &secs }];
    let diag = collector();
    let p =
        PropertyPlan::new(&objs, Class::Elf64, &|t| arch.property_merge(t), false, &diag).unwrap();
    assert_eq!(p.properties().len(), 1);
    assert_eq!(
        text(&diag.finish()),
        "ld: warning: a.o: unsupported GNU_PROPERTY_TYPE 0xc0000002; property dropped\n"
    );
}

#[test]
fn s95_r_a1_differing_pauth_is_refused() {
    let a = prop_note(&[(PAUTH, &[1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0])]);
    let b = prop_note(&[(PAUTH, &[1, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0])]);
    assert_eq!(props(&[vec![a.clone()], vec![a.clone()]]).unwrap().properties().len(), 1);
    assert_eq!(
        props(&[vec![a], vec![b]]),
        Err(format!("ld: error: unsupported feature: {PAUTH_DIFFERS}\n"))
    );
}

#[test]
fn s95_short_property_is_malformed() {
    let bad = prop_note(&[(FEATURE_1_AND, &[1, 0])]);
    let e = props(&[vec![bad]]).unwrap_err();
    assert!(e.contains("program property is too short"), "{e}");
}

#[test]
fn s126_finding7_two_notes_in_one_section_combine_by_rule() {
    let mut two = prop_note(&[(FEATURE_1_AND, &u32b(3))]);
    two.extend(prop_note(&[(FEATURE_1_AND, &u32b(1)), (0xb000_8000, &u32b(2))]));
    let p = props(&[vec![two]]).unwrap();
    assert_eq!(p.value_u32(FEATURE_1_AND), 1, "And within the object");
    assert_eq!(p.value_u32(0xb000_8000), 2, "Or of the notes that have it");
}

#[test]
fn s126_notes_in_two_sections_of_one_object() {
    let a = prop_note(&[(FEATURE_1_AND, &u32b(3))]);
    let b = prop_note(&[(0xb000_8000, &u32b(2))]);
    let p = props(&[vec![a, b]]).unwrap();
    assert!(p.properties().iter().all(|p| p.0 != FEATURE_1_AND), "And: one note lacks it");
    assert_eq!(p.value_u32(0xb000_8000), 2);
}

#[test]
fn s96_output_note_bytes() {
    let n = prop_note(&[(FEATURE_1_AND, &u32b(3))]);
    let p = props(&[vec![n.clone()]]).unwrap();
    assert_eq!(p.size(), n.len() as u64);
    let mut out = vec![0xaa; p.size() as usize];
    p.write(&mut Window::new(0, &mut out));
    assert_eq!(out, n);
}

#[test]
fn s95_memory_seal_is_the_option() {
    let arch = mortar_a64::A64::M1;
    let diag = collector();
    let p = PropertyPlan::new(&[], Class::Elf64, &|t| arch.property_merge(t), true, &diag).unwrap();
    assert_eq!(p.properties().iter().map(|p| p.0).collect::<Vec<_>>(), [MEMORY_SEAL]);
}

#[test]
fn s99_notes_of_different_alignment_are_refused() {
    let diag = collector();
    assert!(
        check_note_alignment(".note.x", &[(SectionId(1), 4), (SectionId(2), 4)], &diag).is_ok()
    );
    assert!(
        check_note_alignment(".note.x", &[(SectionId(1), 4), (SectionId(2), 8)], &diag).is_err()
    );
    assert_eq!(
        text(&diag.finish()),
        "ld: error: unsupported feature: notes of different alignment in one output section\n>>> in .note.x\n"
    );
}

#[test]
fn s100_comment_names_mortar_and_its_version() {
    assert!(COMMENT.starts_with("mortar ") && COMMENT.ends_with('\0'));
    assert_eq!(COMMENT.matches('.').count(), 2);
}

#[test]
fn s90_shstrtab_names_once_in_index_order() {
    let p = ShstrtabPlan::new(&[b".text", b".note.x", b".text", b".shstrtab"]);
    assert_eq!(p.bytes(), b"\0.text\0.note.x\0.shstrtab\0");
    assert_eq!(
        (p.sh_name(1), p.sh_name(2), p.sh_name(3), p.sh_name(4)),
        (Some(1), Some(7), Some(1), Some(15))
    );
    assert_eq!(p.sh_name(0), None);
    assert_eq!(p.division(), Division::Whole { max_size: 25 });
}

#[test]
fn s3_chunk_ids_follow_the_list_order() {
    let ids = chunk_ids(
        ChunkId(5),
        &[(SynthKind::Strtab, 9), (SynthKind::EhFrame, 4), (SynthKind::Symtab, 48)],
    )
    .unwrap();
    assert_eq!(&*ids, &[(ChunkId(5), 4), (ChunkId(6), 48), (ChunkId(7), 9)]);
    assert_eq!(chunk_ids(ChunkId(5), &[(SynthKind::Got, 8), (SynthKind::Got, 8)]), None);
}

#[test]
fn s3_names_and_headers() {
    assert_eq!(SynthKind::RelaPlt.name(true), b".rel.plt");
    assert_eq!(SynthKind::RelaPlt.name(false), b".rela.plt");
    let h = SynthKind::Symtab.header(Class::Elf32, true).unwrap();
    assert_eq!((h.entsize, h.align), (16, 4));
    assert!(SynthKind::Dynamic.header(Class::Elf64, false).is_none(), "M2");
    assert!(!SynthKind::Got.synth(false).address_dependent);
    assert!(SynthKind::RelrDyn.synth(false).address_dependent);
}

#[test]
fn pauth_rule_is_equal_or_absent() {
    assert_eq!(mortar_a64::A64::M1.property_merge(PAUTH), Some(PropMerge::EqualOrAbsent));
}

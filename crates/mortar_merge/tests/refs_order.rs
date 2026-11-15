//! L1 tests of S-17 to S-22 (references into merge sections), S-24 to
//! S-29 (order and offsets) and S-50 (references into `.eh_frame`).

use mortar_core::par::{Pool, check_schedules};
use mortar_core::{Column, FileId, FragId, MergeMap, OutSecId, SectionId, Split};
use mortar_merge::eh::{EhIn, EhReloc, RelocClass, Target, parse};
use mortar_merge::order::offsets;
use mortar_merge::refs::{
    EhValue, EhWho, PieceRef, check_got, eh_ref, named_symbol, section_symbol,
};
use mortar_merge::{MergeIn, MergeMeta, Merged, What, merge};

struct Sec {
    out: u32,
    strings: bool,
    entsize: u32,
    align: u8,
    bytes: Vec<u8>,
}

fn s(out: u32, strings: bool, entsize: u32, align: u8, bytes: &[u8]) -> Sec {
    Sec { out, strings, entsize, align, bytes: bytes.to_vec() }
}

fn merged(secs: &[Sec]) -> Merged {
    let ins: Vec<MergeIn<'_>> = secs
        .iter()
        .enumerate()
        .map(|(i, x)| MergeIn {
            section: SectionId(i as u32),
            file: FileId(i as u32),
            index: 1,
            out: OutSecId(x.out),
            strings: x.strings,
            entsize: x.entsize,
            align: x.align,
            bytes: &x.bytes,
        })
        .collect();
    merge(&Pool::sequential(), &ins).unwrap()
}

fn split(m: Merged) -> (Split, Box<[MergeMeta]>) {
    let meta = m.meta.clone();
    (m.into_split((Box::new([]), Box::new([]), Box::new([]))), meta)
}

#[test]
fn s17_section_symbol_plus_addend() {
    let (sp, _) = split(merged(&[s(0, true, 1, 0, b"abc\0de\0")]));
    let sec = SectionId(0);
    assert_eq!(
        section_symbol(&sp, sec, 7, 0),
        Ok(PieceRef { frag: FragId(0), within: 0, ordinal: 0 })
    );
    assert_eq!(
        section_symbol(&sp, sec, 7, 3),
        Ok(PieceRef { frag: FragId(0), within: 3, ordinal: 0 })
    );
    assert_eq!(
        section_symbol(&sp, sec, 7, 5),
        Ok(PieceRef { frag: FragId(1), within: 1, ordinal: 1 })
    );
    // S-18: r < 0 or r ≥ sh_size.
    assert_eq!(section_symbol(&sp, sec, 7, 7), Err(What::OutsideSection));
    assert_eq!(section_symbol(&sp, sec, 7, -1), Err(What::OutsideSection));
    assert_eq!(
        What::OutsideSection.message(),
        "offset is outside the section; refer to it through a named symbol"
    );
}

#[test]
fn s19_s22_named_symbol() {
    let (sp, _) = split(merged(&[s(0, true, 1, 0, b"abc\0de\0")]));
    let sec = SectionId(0);
    assert_eq!(
        named_symbol(&sp, sec, 7, 4),
        Ok(PieceRef { frag: FragId(1), within: 0, ordinal: 1 })
    );
    // v == sh_size: the piece ending there.
    assert_eq!(
        named_symbol(&sp, sec, 7, 7),
        Ok(PieceRef { frag: FragId(1), within: 3, ordinal: 1 })
    );
    assert_eq!(named_symbol(&sp, sec, 7, 8), Err(What::OutsideSection));
}

#[test]
fn s21_got_with_section_addend_is_refused() {
    assert_eq!(check_got(true, true, 4), Err(What::GotSectionAddend));
    assert_eq!(check_got(true, true, 0), Ok(()));
    assert_eq!(check_got(true, false, 4), Ok(()));
    assert_eq!(check_got(false, true, 4), Ok(()));
}

#[test]
fn s24_s25_s26_order_and_offsets() {
    let m = merged(&[
        s(0, false, 4, 2, &[1, 0, 0, 0]), // frag 0: constants, out 0
        s(0, true, 1, 0, b"ab\0"),        // frag 1: strings 1, align 1
        s(0, true, 1, 3, b"cd\0"),        // frag 2: strings 1, align 8
        s(0, true, 4, 2, &[7, 0, 0, 0, 0, 0, 0, 0]), // frag 3: strings 4
        s(1, true, 1, 0, b"ef\0"),        // frag 4: out 1
        s(0, true, 1, 0, b"gh\0ab\0"),    // frag 5, frag 1 again
    ]);
    let (sp, meta) = split(m);
    let live = Column::filled(sp.frags.len(), true);
    let o = check_schedules(|pool| offsets(pool, &sp, &meta, &live));
    let off = |f: u32| o.map.frag_off[FragId(f)];
    // Out 0: strings entsize 1 align 1 (1, 5), align 8 (2), strings
    // entsize 4 (3), constants (0).
    assert_eq!(&*o.order, [FragId(1), FragId(5), FragId(2), FragId(3), FragId(0), FragId(4)]);
    assert_eq!([off(1), off(5), off(2), off(3), off(0)], [0, 3, 8, 12, 20]);
    assert_eq!(off(4), 0);
    assert_eq!(o.chunks.len(), 2);
    assert_eq!((o.chunks[0].out, o.chunks[0].size, o.chunks[0].align), (OutSecId(0), 24, 3));
    assert_eq!((o.chunks[1].out, o.chunks[1].size, o.chunks[1].align), (OutSecId(1), 3, 0));
    // Dead fragments are not placed; a chunk with none does not exist.
    let mut live = live;
    live[FragId(4)] = false;
    live[FragId(5)] = false;
    let o = offsets(&Pool::sequential(), &sp, &meta, &live);
    assert_eq!(o.map.frag_off[FragId(4)], MergeMap::NONE);
    assert_eq!(o.map.frag_off[FragId(2)], 8);
    assert_eq!(o.chunks.len(), 1);
}

#[test]
fn frags_in_any_range() {
    let bytes: Vec<u8> =
        (0..200u32).flat_map(|i| [b'a' + (i % 26) as u8, b'0' + (i / 26) as u8, 0]).collect();
    let (sp, meta) = split(merged(&[s(0, true, 1, 1, &bytes)]));
    let o = offsets(&Pool::sequential(), &sp, &meta, &Column::filled(sp.frags.len(), true));
    // Every fragment is 3 bytes at a 2-aligned place: 0, 4, 8, ...
    for a in 0..40u64 {
        for b in a..44 {
            let want: Vec<FragId> = o
                .order
                .iter()
                .copied()
                .filter(|&f| {
                    let lo = o.map.frag_off[f];
                    lo < b && a < lo + u64::from(sp.frags[f].size)
                })
                .collect();
            assert_eq!(o.frags_in(&sp, 0, a..b), &want[..], "{a}..{b}");
        }
    }
}

/// `.eh_frame` with a CIE (0..20), FDE (20..40), FDE (40..60) and a
/// terminator (60..64).
fn eh() -> (Vec<u8>, Vec<EhReloc>) {
    let mut b =
        vec![16, 0, 0, 0, 0, 0, 0, 0, 1, b'z', b'R', 0, 1, 0x78, 30, 1, 0x1b, 0x0c, 0x1f, 0];
    for at in [20u32, 40] {
        let mut f = vec![16, 0, 0, 0];
        f.extend_from_slice(&(at + 4).to_le_bytes());
        f.extend_from_slice(&[0; 12]);
        b.extend(f);
    }
    b.extend_from_slice(&[0; 4]);
    let r = [28u64, 48]
        .map(|o| EhReloc {
            offset: o,
            r_type: 261,
            name: "R_AARCH64_PREL32",
            class: RelocClass::Pc,
            width: 4,
            addend: 0,
            target: Target::Section { section: SectionId(9), value: 0 },
        })
        .to_vec();
    (b, r)
}

#[test]
fn s50_references_into_eh_frame() {
    let (b, r) = eh();
    let ins = [EhIn { section: SectionId(1), file: FileId(0), index: 3, bytes: &b, relocs: &r }];
    let frames = parse(&Pool::sequential(), 8, &ins).unwrap();
    assert_eq!(frames.records.len(), 3);
    // The CIE kept at output 100, the first FDE removed, the second kept
    // at 120; the next kept record after the section is at 500.
    let kept = |i: u32| match i {
        0 => Some(100),
        2 => Some(120),
        _ => None,
    };
    let v = |r: i64, who: EhWho| eh_ref(&frames, 0, r, who, kept, 500);
    let reloc = EhWho::Reloc;
    let sym = EhWho::Symbol { droppable: false };
    let local = EhWho::Symbol { droppable: true };
    // Rule 1.
    assert_eq!(v(0, reloc), Ok(EhValue::Offset(0)));
    // Rule 2.
    assert_eq!(v(4, reloc), Ok(EhValue::Offset(104)));
    assert_eq!(v(44, reloc), Ok(EhValue::Offset(124)));
    // Rule 3: the start of the removed FDE, the terminator, the end.
    assert_eq!(v(20, reloc), Ok(EhValue::Offset(120)));
    assert_eq!(v(60, reloc), Ok(EhValue::Offset(500)));
    assert_eq!(v(64, reloc), Ok(EhValue::Offset(500)));
    // Rule 4.
    assert_eq!(v(24, reloc), Err(What::IntoRemovedRecord));
    assert_eq!(v(24, local), Ok(EhValue::Dropped));
    assert_eq!(v(24, sym), Ok(EhValue::Offset(120)));
    assert_eq!(v(62, reloc), Err(What::IntoRemovedRecord));
    // Rule 5.
    assert_eq!(v(-1, reloc), Err(What::OutsideSection));
    assert_eq!(v(65, sym), Err(What::OutsideSection));
    // Mapping symbols follow their record (rule 1 does not apply).
    assert_eq!(v(0, EhWho::Mapping), Ok(EhValue::Offset(100)));
    assert_eq!(v(20, EhWho::Mapping), Ok(EhValue::Dropped));
    assert_eq!(v(40, EhWho::Mapping), Ok(EhValue::Offset(120)));
    assert_eq!(v(60, EhWho::Mapping), Ok(EhValue::Dropped));
    // A removed first record: rule 1 still gives the output start; its
    // mapping symbol is dropped.
    let none_first = |i: u32| if i == 2 { Some(120) } else { None };
    assert_eq!(eh_ref(&frames, 0, 0, reloc, none_first, 500), Ok(EhValue::Offset(0)));
    assert_eq!(eh_ref(&frames, 0, 0, EhWho::Mapping, none_first, 500), Ok(EhValue::Dropped));
    // All removed: rule 3 falls through to `after`.
    assert_eq!(eh_ref(&frames, 0, 20, reloc, |_| None, 500), Ok(EhValue::Offset(500)));
    assert_eq!(What::IntoRemovedRecord.message(), "reference into a removed .eh_frame record");
}

#[test]
fn s50_empty_section_offset_zero() {
    // crtbeginT.o on x86: `.eh_frame+0` of a 0-byte section.
    let ins = [EhIn { section: SectionId(1), file: FileId(0), index: 3, bytes: &[], relocs: &[] }];
    let frames = parse(&Pool::sequential(), 8, &ins).unwrap();
    assert_eq!(eh_ref(&frames, 0, 0, EhWho::Reloc, |_| None, 7), Ok(EhValue::Offset(0)));
    assert_eq!(eh_ref(&frames, 0, 1, EhWho::Reloc, |_| None, 7), Err(What::OutsideSection));
}

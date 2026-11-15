//! L1 tests of S-34 to S-45: `.eh_frame` records from hand-made bytes.
#![allow(clippy::many_single_char_names)] // b(ytes), r(elocs), t, p, s

use mortar_core::par::{Pool, check_schedules};
use mortar_core::split::EhPiece;
use mortar_core::{FileId, OptId, SectionId, SymbolId};
use mortar_diag::{FileRef, Key, Style};
use mortar_merge::eh::{Code, EhFrames, EhIn, EhReloc, Kind, RelocClass, Target, parse};
use mortar_merge::{Problem, What};

const TEXT: SectionId = SectionId(100);

/// A record: the length word and `body`.
fn rec(body: &[u8]) -> Vec<u8> {
    let mut v = (body.len() as u32).to_le_bytes().to_vec();
    v.extend_from_slice(body);
    v
}

/// A CIE body: id 0, `version`, `aug`, code 1, data -8, return register
/// 30, then `aug_data` (with its length when `aug` starts with `z`), then
/// `def_cfa sp 0`, padded to a multiple of 4 with `nop`s.
fn cie_v(version: u8, aug: &str, aug_data: &[u8]) -> Vec<u8> {
    let mut b = vec![0, 0, 0, 0, version];
    b.extend_from_slice(aug.as_bytes());
    b.extend_from_slice(&[0, 1, 0x78]);
    if version == 1 {
        b.push(30);
    } else {
        b.push(30); // uleb 30
    }
    if aug.starts_with('z') {
        b.push(aug_data.len() as u8);
        b.extend_from_slice(aug_data);
    }
    b.extend_from_slice(&[0x0c, 0x1f, 0x00]);
    while !(b.len() + 4).is_multiple_of(4) {
        b.push(0);
    }
    rec(&b)
}

fn cie(aug: &str, aug_data: &[u8]) -> Vec<u8> {
    cie_v(1, aug, aug_data)
}

/// An FDE at `at` for the CIE at `cie_at`, with a 4-byte initial location
/// `pc` and range, an empty augmentation data (when `z`) and padding.
fn fde(at: usize, cie_at: usize, pc: u32, z: bool, lsda: Option<u32>) -> Vec<u8> {
    let mut b = ((at + 4 - cie_at) as u32).to_le_bytes().to_vec();
    b.extend_from_slice(&pc.to_le_bytes());
    b.extend_from_slice(&0x40u32.to_le_bytes());
    if z {
        match lsda {
            Some(l) => {
                b.push(4);
                b.extend_from_slice(&l.to_le_bytes());
            }
            None => b.push(0),
        }
    }
    while !(b.len() + 4).is_multiple_of(4) {
        b.push(0);
    }
    rec(&b)
}

fn pc32(offset: u64, target: Target) -> EhReloc {
    EhReloc {
        offset,
        r_type: 261,
        name: "R_AARCH64_PREL32",
        class: RelocClass::Pc,
        width: 4,
        addend: 0,
        target,
    }
}

fn code(offset: u64) -> EhReloc {
    pc32(offset, Target::Section { section: TEXT, value: 0x10 })
}

fn sec(bytes: &[u8], relocs: &[EhReloc]) -> (Vec<u8>, Vec<EhReloc>) {
    (bytes.to_vec(), relocs.to_vec())
}

fn run(secs: &[(Vec<u8>, Vec<EhReloc>)]) -> Result<EhFrames, Vec<Problem>> {
    let ins: Vec<EhIn<'_>> = secs
        .iter()
        .enumerate()
        .map(|(i, (b, r))| EhIn {
            section: SectionId(i as u32 * 3 + 1),
            file: FileId(i as u32),
            index: 5,
            bytes: b,
            relocs: r,
        })
        .collect();
    check_schedules(|pool| parse(pool, 8, &ins))
}

fn one(bytes: &[u8], relocs: &[EhReloc]) -> Result<EhFrames, Vec<Problem>> {
    run(&[sec(bytes, relocs)])
}

fn what(bytes: &[u8], relocs: &[EhReloc]) -> (What, u64) {
    let e = one(bytes, relocs).expect_err("refused");
    assert_eq!(e.len(), 1);
    (e[0].what.clone(), e[0].offset)
}

/// A zR CIE (20 bytes with its length) and one FDE after it, at 20.
fn basic() -> (Vec<u8>, Vec<EhReloc>) {
    let mut b = cie("zR", &[0x1b]);
    let at = b.len();
    b.extend(fde(at, 0, 0, true, None));
    (b, vec![code(at as u64 + 8)])
}

#[test]
fn s34_records_and_terminator() {
    let (mut b, r) = basic();
    let t = b.len();
    b.extend_from_slice(&[0; 4]);
    let eh = one(&b, &r).unwrap();
    assert_eq!(eh.records.len(), 2);
    assert_eq!(eh.sections[0].terminator, Some(t as u32));
    assert_eq!(eh.sections[0].size, b.len() as u32);
    assert!(matches!(eh.records[0].kind, Kind::Cie(_)));
    let Kind::Fde(f) = eh.records[1].kind else { panic!() };
    assert_eq!(f.cie, 0);
    assert_eq!(f.code, Code::Section { section: TEXT, offset: 0x10 });
    assert_eq!(f.range, 0x40);
    // An empty section and a terminator-only section have no records.
    let eh = run(&[sec(&[], &[]), sec(&[0; 4], &[])]).unwrap();
    assert_eq!(eh.records.len(), 0);
    assert_eq!(eh.sections[1].terminator, Some(0));
    assert_eq!(eh.split_rows().0.len(), 0);
}

#[test]
fn s34_dwarf64_is_refused() {
    let b = [0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0];
    assert_eq!(what(&b, &[]), (What::Dwarf64, 0));
}

#[test]
fn s35_accepted_cies() {
    // Empty augmentation: absptr (8 bytes here) with an Abs relocation.
    let mut b = cie("", &[]);
    let at = b.len();
    let mut f = (at as u32 + 4).to_le_bytes().to_vec();
    f.extend_from_slice(&[0; 16]);
    b.extend(rec(&f));
    let abs = EhReloc {
        offset: at as u64 + 8,
        r_type: 257,
        name: "R_AARCH64_ABS64",
        class: RelocClass::Abs,
        width: 8,
        addend: 4,
        target: Target::Section { section: TEXT, value: 0 },
    };
    let eh = one(&b, &[abs]).unwrap();
    let Kind::Fde(f) = eh.records[1].kind else { panic!() };
    assert_eq!(f.code, Code::Section { section: TEXT, offset: 4 });
    let c = eh.records[0].cie().unwrap();
    assert_eq!((c.version, c.fde_enc, c.z), (1, 0, false));
    // udata4 and sdata4 with a 4-byte absolute relocation (non-PIC
    // x86-64: R_X86_64_32).
    for enc in [0x03, 0x0b] {
        let mut b = cie("zR", &[enc]);
        let at = b.len();
        b.extend(fde(at, 0, 0, true, None));
        let r32 = EhReloc {
            r_type: 10,
            name: "R_X86_64_32",
            class: RelocClass::Abs,
            ..code(at as u64 + 8)
        };
        let eh = one(&b, &[r32]).unwrap_or_else(|e| panic!("{enc:#x}: {e:?}"));
        assert_eq!(eh.records[0].cie().unwrap().fde_enc, enc);
        // An 8-byte one does not fit the 4-byte field.
        let r64 = EhReloc {
            width: 8,
            name: "R_X86_64_64",
            class: RelocClass::Abs,
            ..code(at as u64 + 8)
        };
        assert_eq!(one(&b, &[r64]).unwrap_err()[0].what, What::PointerReloc("R_X86_64_64".into()));
    }
    // Version 3, zRS, zPLR with a personality and an LSDA, B and G.
    for (v, aug) in [(3, "zR"), (1, "zRS"), (1, "zRB"), (1, "zRG"), (1, "zRSBG")] {
        let mut b = cie_v(v, aug, &[0x1b]);
        let at = b.len();
        b.extend(fde(at, 0, 0, true, None));
        let eh = one(&b, &[code(at as u64 + 8)]).unwrap_or_else(|e| panic!("{aug}: {e:?}"));
        assert_eq!(eh.records[0].cie().unwrap().signal, aug.contains('S'));
    }
    let mut data = vec![0x9b, 0, 0, 0, 0, 0x1b, 0x1b];
    let mut b = cie("zPLR", &data);
    // The personality pointer: after length, id, version, "zPLR\0", the
    // three alignment fields, the data length and P's encoding.
    let pers = pc32(19, Target::Undefined { symbol: SymbolId(7), weak: false });
    let at = b.len();
    b.extend(fde(at, 0, 0, true, Some(0)));
    let lsda = pc32(at as u64 + 17, Target::Section { section: SectionId(200), value: 0 });
    let eh = one(&b, &[pers.clone(), code(at as u64 + 8), lsda]).unwrap();
    let c = eh.records[0].cie().unwrap();
    assert_eq!(c.personality.map(|p| (p.enc, p.at)), Some((0x9b, 19)));
    assert_eq!(c.lsda_enc, Some(0x1b));
    let f = eh.records[1].fde().unwrap();
    assert_eq!(f.lsda.map(|p| p.at), Some(17));
    // P and L may be omit.
    data = vec![0xff, 0xff, 0x1b];
    let mut b = cie("zPLR", &data);
    let at = b.len();
    b.extend(fde(at, 0, 0, true, None));
    let eh = one(&b, &[code(at as u64 + 8)]).unwrap();
    assert_eq!(eh.records[0].cie().unwrap().personality, None);
}

#[test]
fn s36_refused_cies_with_lld_wording() {
    assert_eq!(what(&cie_v(2, "zR", &[0x1b]), &[]).0, What::CieVersion(2));
    for aug in ["zX", "zRR", "eh", "R"] {
        assert_eq!(what(&cie(aug, &[0x1b]), &[]).0, What::Augmentation(aug.into()), "{aug}");
    }
    // Augmentation data running past the record.
    let mut body = vec![0, 0, 0, 0, 1, b'z', b'R', 0, 1, 0x78, 30, 0x20, 0x1b];
    body.extend_from_slice(&[0; 3]);
    assert_eq!(what(&rec(&body), &[]).0, What::CorruptedRecord);
    let p = Problem { file: FileId(0), section: 5, piece: 0, offset: 0, what: What::CieVersion(2) };
    let text = p.diagnostic(FileRef::new("a.o"), ".eh_frame").render(&Style::plain("ld"));
    assert!(text.contains("FDE version 1 or 3 expected, but got 2"), "{text}");
    let p = Problem { what: What::Augmentation("zX".into()), ..p };
    let text = p.diagnostic(FileRef::new("a.o"), ".eh_frame").render(&Style::plain("ld"));
    assert!(text.contains("unknown .eh_frame augmentation string: zX"), "{text}");
}

#[test]
fn s37_pointer_encodings() {
    for enc in [0x00, 0x02, 0x03, 0x04, 0x0a, 0x0b, 0x0c, 0x10, 0x1b, 0x1c] {
        assert!(one(&cie("zR", &[enc]), &[]).is_ok(), "{enc:#x}");
    }
    // uleb/sleb, datarel, textrel, funcrel, aligned, and indirect on R.
    for enc in [0x01, 0x09, 0x30, 0x20, 0x40, 0x50, 0x9b] {
        assert_eq!(what(&cie("zR", &[enc]), &[]).0, What::Encoding(enc), "{enc:#x}");
    }
    let text =
        Problem { file: FileId(0), section: 0, piece: 0, offset: 0, what: What::Encoding(0x30) }
            .diagnostic(FileRef::new("a.o"), ".eh_frame")
            .render(&Style::plain("ld"));
    assert!(text.contains("unsupported feature: .eh_frame pointer encoding 0x30"), "{text}");
}

#[test]
fn s38_initial_location() {
    let (b, _) = basic();
    let at = 20u64;
    // No relocation, zero field: dropped.
    let eh = one(&b, &[]).unwrap();
    assert_eq!(eh.records[1].fde().unwrap().code, Code::Dropped);
    assert_eq!(eh.split_rows().2[1], EhPiece { cie: 0, code: OptId::NONE });
    // Only an R_*_NONE: no relocation.
    let none =
        EhReloc { r_type: 0, name: "R_AARCH64_NONE", class: RelocClass::None, ..code(at + 8) };
    let eh = one(&b, std::slice::from_ref(&none)).unwrap();
    assert_eq!(eh.records[1].fde().unwrap().code, Code::Dropped);
    // NONE next to the real one.
    let eh = one(&b, &[none, code(at + 8)]).unwrap();
    assert!(matches!(eh.records[1].fde().unwrap().code, Code::Section { .. }));
    // Non-zero field without a relocation: refused.
    let mut nz = b.clone();
    nz[at as usize + 8] = 1;
    assert_eq!(what(&nz, &[]), (What::FdeWithoutReloc, at));
    // Class or width not the encoding's.
    let abs = EhReloc { class: RelocClass::Abs, name: "R_AARCH64_ABS32", ..code(at + 8) };
    assert_eq!(what(&b, &[abs]).0, What::PointerReloc("R_AARCH64_ABS32".into()));
    let wide = EhReloc { width: 8, name: "R_AARCH64_PREL64", ..code(at + 8) };
    assert_eq!(what(&b, &[wide]).0, What::PointerReloc("R_AARCH64_PREL64".into()));
    let other = EhReloc { class: RelocClass::Other, name: "R_X86_64_GOTPCREL", ..code(at + 8) };
    assert_eq!(what(&b, &[other]).0, What::PointerReloc("R_X86_64_GOTPCREL".into()));
    let inside = code(at + 9);
    assert!(matches!(what(&b, &[inside]).0, What::PointerReloc(_)));
    assert_eq!(what(&b, &[code(at + 8), code(at + 8)]).0, What::TwoRelocs);
    // Targets that make the FDE dead.
    for (t, c) in [
        (Target::Absolute(5), Code::Absolute),
        (Target::Undefined { symbol: SymbolId(1), weak: true }, Code::Undefined),
    ] {
        let eh = one(&b, &[pc32(at + 8, t)]).unwrap();
        assert_eq!(eh.records[1].fde().unwrap().code, c);
    }
    assert_eq!(what(&b, &[pc32(at + 8, Target::Elsewhere(SymbolId(1)))]).0, What::FdeCodeElsewhere);
    // The REL addend is part of the offset.
    let eh = one(&b, &[EhReloc { addend: 0x20, ..code(at + 8) }]).unwrap();
    assert_eq!(eh.records[1].fde().unwrap().code, Code::Section { section: TEXT, offset: 0x30 });
}

#[test]
fn s39_malformed_records() {
    let (b, r) = basic();
    // Truncated.
    assert_eq!(what(&b[..b.len() - 4], &r), (What::PastEnd, 20));
    assert_eq!(what(&b[..30], &[]), (What::PastEnd, 20));
    // CIE pointer to no CIE start, and to an FDE.
    let mut bad = b.clone();
    bad[24] = 8; // 20 + 4 - 8 = 16: inside the CIE
    assert_eq!(what(&bad, &r), (What::BadCieReference, 24));
    let mut two = b.clone();
    two.extend(fde(two.len(), 20, 0, true, None)); // points to the FDE
    assert_eq!(what(&two, &r).0, What::BadCieReference);
    let mut far = b.clone();
    far[24..28].copy_from_slice(&1000u32.to_le_bytes()); // before the section
    assert_eq!(what(&far, &r).0, What::BadCieReference);
    // After a terminator.
    let mut t = b.clone();
    t.extend_from_slice(&[0; 4]);
    t.extend(cie("zR", &[0x1b]));
    assert_eq!(what(&t, &r), (What::AfterTerminator, b.len() as u64 + 4));
    let mut rs = r.clone();
    rs.push(code(b.len() as u64 + 8));
    assert_eq!(what(&t, &rs), (What::RelocNotInPiece, b.len() as u64 + 8));
    // A relocation at the terminator, or past the records.
    let mut tt = b.clone();
    tt.extend_from_slice(&[0; 4]);
    let mut rs = r.clone();
    rs.push(code(b.len() as u64));
    assert_eq!(what(&tt, &rs), (What::RelocNotInPiece, b.len() as u64));
    let mut rs = r.clone();
    rs.push(code(b.len() as u64 + 100));
    assert_eq!(what(&b, &rs), (What::RelocNotInPiece, b.len() as u64 + 100));
    // A relocation of a length or CIE pointer.
    let mut rs = r.clone();
    rs.insert(0, code(24));
    assert_eq!(what(&b, &rs), (What::RelocInHeader, 24));
    for (w, text) in [
        (What::PastEnd, "CIE/FDE ends past the end of the section"),
        (What::BadCieReference, "invalid CIE reference"),
        (What::AfterTerminator, "data after the .eh_frame terminator"),
        (What::RelocNotInPiece, "relocation is not in any piece"),
    ] {
        let p = Problem { file: FileId(2), section: 5, piece: 1, offset: 28, what: w };
        assert_eq!(p.key(), Key::new(60, 2, (5 << 32) | 1));
        let s = p.diagnostic(FileRef::new("a.o"), ".eh_frame").render(&Style::plain("ld"));
        assert!(s.contains(text) && s.contains("a.o:(.eh_frame+0x1c)"), "{s}");
    }
}

#[test]
fn s39_problems_one_per_section_sorted() {
    let (b, r) = basic();
    let e = run(&[sec(&b, &r), sec(&b[..30], &[]), sec(&[0xff; 4], &[])]).unwrap_err();
    assert_eq!(
        e.iter().map(|p| (p.file.0, p.what.clone())).collect::<Vec<_>>(),
        [(1, What::PastEnd), (2, What::Dwarf64)]
    );
}

#[test]
fn s40_accepted_oddities() {
    // A length that is not a multiple of 4: S-47 pads the output.
    let mut body = cie("zR", &[0x1b])[4..].to_vec();
    body.push(0);
    let b = rec(&body);
    let eh = one(&b, &[]).unwrap();
    assert_eq!(eh.records[0].len, 17);
    assert_eq!(eh.records[0].out_size(), 4 + 20);
    // Several CIEs, one unused; FDEs use either.
    let mut b = cie("zR", &[0x1b]);
    b.extend(cie("zR", &[0x1b]));
    b.extend(cie("zR", &[0x03]));
    let at = b.len();
    b.extend(fde(at, 20, 0, true, None));
    let at2 = b.len();
    b.extend(fde(at2, 40, 0, true, None));
    let r = [code(at as u64 + 8), EhReloc { class: RelocClass::Abs, ..code(at2 as u64 + 8) }];
    let eh = one(&b, &r).unwrap();
    let cies: Vec<u32> = eh.records.iter().map(|r| r.fde().map_or(u32::MAX, |f| f.cie)).collect();
    assert_eq!(cies, [u32::MAX, u32::MAX, u32::MAX, 1, 2]);
}

#[test]
fn s43_cie_identity_is_bytes_and_relocations() {
    let data = [0x9b, 0, 0, 0, 0, 0x1b, 0x1b];
    let section = |personality: Target, pad: bool| {
        let mut b = Vec::new();
        if pad {
            b.extend(cie("zR", &[0x1b]));
        }
        let c = b.len();
        b.extend(cie("zPLR", &data));
        let at = b.len();
        b.extend(fde(at, c, 0, true, Some(0)));
        let relocs = vec![pc32(c as u64 + 19, personality), code(at as u64 + 8)];
        (b, relocs)
    };
    let gxx = Target::Undefined { symbol: SymbolId(3), weak: false };
    let other = Target::Undefined { symbol: SymbolId(4), weak: false };
    let eh =
        run(&[section(gxx, false), section(gxx, true), section(other, false), section(gxx, false)])
            .unwrap();
    let classes: Vec<u32> = eh.records.iter().filter_map(|r| r.cie().map(|c| c.class)).collect();
    // zPLR/gxx = 0, zR = 1, zPLR/gxx again, zPLR/other = 2, zPLR/gxx.
    assert_eq!(classes, [0, 1, 0, 2, 0]);
    assert_eq!(eh.classes, 3);
    // Different addends are different CIEs too.
    let (mut b, mut r) = section(gxx, false);
    r[0].addend = 4;
    let (b2, r2) = section(gxx, false);
    b.truncate(b.len());
    let eh = run(&[(b, r), (b2, r2)]).unwrap();
    assert_eq!(eh.classes, 2);
}

#[test]
fn s44_keepers_are_the_least_live_member() {
    let (b, r) = basic();
    let eh = run(&[sec(&b, &r), sec(&b, &r), sec(&b, &r)]).unwrap();
    assert_eq!(eh.classes, 1);
    // Records: 0 CIE, 1 FDE, 2 CIE, 3 FDE, 4 CIE, 5 FDE.
    assert_eq!(eh.keepers(|i| i != 0), [2]);
    assert_eq!(eh.keepers(|_| true), [0]);
    assert_eq!(eh.keepers(|_| false), [u32::MAX]);
}

#[test]
fn s45_split_rows() {
    let (b, r) = basic();
    let eh = run(&[sec(&b, &r), sec(&[], &[]), sec(&b, &[])]).unwrap();
    let (rows, off, piece) = eh.split_rows();
    assert_eq!(
        rows.iter().map(|r| (r.section.0, r.pieces.start, r.pieces.len)).collect::<Vec<_>>(),
        [(1, 0, 2), (7, 2, 2)]
    );
    assert_eq!(&*off, [0, 20, 0, 20]);
    assert_eq!(
        &*piece,
        [
            EhPiece::CIE,
            EhPiece { cie: 0, code: OptId::some(TEXT) },
            EhPiece::CIE,
            EhPiece { cie: 0, code: OptId::NONE },
        ]
    );
    // The records' relocation ranges.
    assert_eq!((eh.records[1].relocs.start, eh.records[1].relocs.len), (0, 1));
    assert_eq!(eh.records[0].relocs.len, 0);
}

#[test]
fn s56_thread_count_is_not_an_input() {
    let (b, r) = basic();
    let secs: Vec<_> = (0..50).map(|_| sec(&b, &r)).collect();
    let ins: Vec<EhIn<'_>> = secs
        .iter()
        .enumerate()
        .map(|(i, (b, r))| EhIn {
            section: SectionId(i as u32),
            file: FileId(i as u32),
            index: 1,
            bytes: b,
            relocs: r,
        })
        .collect();
    let one = parse(&Pool::sequential(), 8, &ins).unwrap();
    for t in [2, 9, 64] {
        assert_eq!(parse(&Pool::sequential().with_threads(t), 8, &ins).unwrap(), one);
    }
}

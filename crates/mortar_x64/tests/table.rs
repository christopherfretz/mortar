//! The table of x64.md §6 as data: every type number classified, the
//! names against `mortar_elf`, the status counts of §1, the facts of
//! X-36, X-49, X-50 and the shapes of §10.

use mortar_arch::{
    Arch, Check, DemandClass, DynRole, Field, OutputCx, PltPart, PltVariant, RefName, Site,
    SlotKind, Status, SynthCx, Window, x86_property_merge,
};
use mortar_diag::RefusalClass;
use mortar_x64::{FACTS, MAX_REACH, X64, table};

const M1_TYPES: [u32; 30] = [
    0, 1, 2, 4, 9, 10, 11, 12, 13, 14, 15, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 29, 31, 32,
    33, 34, 35, 41, 42,
];
const LATER: [u32; 12] = [3, 28, 30, 43, 44, 45, 46, 47, 48, 49, 50, 51];

fn site() -> Site<'static> {
    Site { bytes: &[0; 64], offset: 32 }
}

#[test]
fn every_type_number_is_classified_or_refused_by_name() {
    let x = X64::M1;
    let mut accepted = Vec::new();
    for t in 0..=0x1_0000u32 {
        match x.classify(t, site()) {
            Ok(h) => {
                assert_eq!(h.status, Status::M1, "{t}");
                accepted.push(t);
            }
            Err(r) => {
                assert_eq!(r.class, RefusalClass::RelocationType, "{t}");
                match mortar_elf::reloc::x86_64::name(t) {
                    Some(n) => assert_eq!(r.name, RefName::Named(n), "{t}"),
                    None => assert_eq!(r.name, RefName::Hex(u64::from(t)), "{t}"),
                }
            }
        }
    }
    assert_eq!(accepted, M1_TYPES, "§1: 30 M1 types");
}

#[test]
fn status_counts_and_names() {
    let mut later = Vec::new();
    for t in 0..=table::MAX_TYPE + 8 {
        let h = table::howto(t);
        match table::status(t) {
            Status::M1 => assert!(M1_TYPES.contains(&t)),
            Status::Later => later.push(t),
            Status::Refused => assert!(h.is_none(), "{t}"),
            s @ Status::M2 => panic!("{t}: unexpected status {s:?}"),
        }
        if let Some(h) = h {
            assert_eq!(Some(h.name), mortar_elf::reloc::x86_64::name(t), "{t}");
        }
    }
    assert_eq!(later, LATER, "X-13");
}

#[test]
fn dynamic_and_withdrawn_types_are_refused() {
    // X-14 and §6.3.
    for t in [5, 6, 7, 8, 16, 36, 37, 38, 39, 40, 52, 0xff, 0xffff_ffff] {
        let r = X64::M1.classify(t, site()).unwrap_err();
        assert_eq!(r.class, RefusalClass::RelocationType, "{t}");
    }
}

#[test]
fn fields_and_checks_follow_section_3() {
    // §3.2 and §3.3.
    let width = |t: u32| table::howto(t).unwrap().field.width();
    for t in [1, 17, 18, 24, 25, 27, 29, 31, 33] {
        assert_eq!(width(t), 8, "{t}");
        assert_eq!(table::howto(t).unwrap().check, Check::None, "{t}");
    }
    for t in [2, 4, 9, 10, 11, 19, 20, 21, 22, 23, 26, 32, 34, 41, 42] {
        assert_eq!(width(t), 4, "{t}");
    }
    for t in [2, 4, 9, 11, 19, 20, 21, 22, 23, 26, 34, 41, 42] {
        assert_eq!(table::howto(t).unwrap().check, Check::Signed(32), "{t}");
    }
    for t in [10, 32] {
        assert_eq!(table::howto(t).unwrap().check, Check::Unsigned(32), "{t}");
    }
    assert_eq!(table::howto(12).unwrap().check, Check::Either(16));
    assert_eq!(table::howto(13).unwrap().check, Check::Signed(16));
    assert_eq!(table::howto(14).unwrap().check, Check::Either(8));
    assert_eq!(table::howto(15).unwrap().check, Check::Signed(8));
    assert_eq!((width(12), width(13), width(14), width(15)), (2, 2, 1, 1));
    for t in [0, 35] {
        assert_eq!(table::howto(t).unwrap().field, Field::None, "{t}");
    }
    // X-5: no instruction field.
    for t in M1_TYPES {
        assert!(matches!(table::howto(t).unwrap().field, Field::Data { .. } | Field::None), "{t}");
    }
}

#[test]
fn classes_follow_section_5() {
    use DemandClass as C;
    let want: [(u32, C); 30] = [
        (0, C::None),
        (1, C::AbsWord),
        (2, C::Pc),
        (4, C::Branch),
        (9, C::Got),
        (10, C::AbsNarrow),
        (11, C::AbsNarrow),
        (12, C::AbsNarrow),
        (13, C::Pc),
        (14, C::AbsNarrow),
        (15, C::Pc),
        (17, C::TlsDtpRel),
        (18, C::TlsLe),
        (19, C::TlsGd),
        (20, C::TlsLd),
        (21, C::TlsDtpRel),
        (22, C::TlsIe),
        (23, C::TlsLe),
        (24, C::Pc),
        (25, C::GotRel),
        (26, C::GotBaseRef),
        (27, C::Got),
        (29, C::GotBaseRef),
        (31, C::Branch),
        (32, C::Size),
        (33, C::Size),
        (34, C::TlsDesc),
        (35, C::TlsDesc),
        (41, C::Got),
        (42, C::Got),
    ];
    for (t, c) in want {
        assert_eq!(table::howto(t).unwrap().class, c, "{t}");
    }
}

#[test]
fn facts() {
    // X-36, X-49, X-50, X-29.
    let f = X64::M1.facts();
    assert_eq!(f.e_machine, 62);
    assert_eq!((f.word, f.osabi, f.page_mask), (8, 0, 0));
    assert_eq!((f.max_page, f.common_page, f.image_base), (0x1000, 0x1000, 0x40_0000));
    assert_eq!((f.got_header, f.got_plt_reserved), (0, 3));
    assert_eq!(f.code_fill, &[0xCC]);
    assert_eq!(f.code_nop, &[0x90]);
    assert_eq!(f.seq_leads, &[19, 20]);
    assert_eq!(MAX_REACH, Window { before: 4, after: 15 });
    assert_eq!(FACTS.max_reach, MAX_REACH);
    assert_eq!(X64::M1.e_flags(&mut [1u32, 2, 0xffff_ffff].into_iter()), Ok(0));
}

#[test]
fn slots_and_dynamic_types() {
    // §6.2, §10.1.
    let g = X64::M1.got();
    assert_eq!((g.slot_words)(SlotKind::Addr), 1);
    assert_eq!((g.slot_words)(SlotKind::TpOff { negated: false }), 1);
    assert_eq!((g.slot_words)(SlotKind::TlsGd), 2);
    assert_eq!((g.slot_words)(SlotKind::TlsDesc), 2);
    let x = X64::M1;
    assert_eq!(x.dyn_type(DynRole::Relative), Some(8));
    assert_eq!(x.dyn_type(DynRole::IRelative), Some(37));
    assert_eq!(x.dyn_type(DynRole::GlobDat), Some(6));
    assert_eq!(x.dyn_type(DynRole::JumpSlot), Some(7));
    assert_eq!(x.dyn_type(DynRole::TpOff { negated: false }), Some(18));
    assert_eq!(x.dyn_type(DynRole::TpOff { negated: true }), None);
    assert_eq!(x.dyn_type(DynRole::TlsDesc), Some(36));
}

#[test]
fn branch_reach_and_no_thunks() {
    // X-33.
    let x = X64::M1;
    let plt32 = table::howto(4).unwrap();
    let b = x.branch(&plt32).unwrap();
    assert_eq!((b.reach.min, b.reach.max, b.veneer), (-(1 << 31), (1 << 31) - 1, false));
    assert!(x.branch(&table::howto(31).unwrap()).is_none(), "PLTOFF64 has no reach");
    assert!(x.branch(&table::howto(2).unwrap()).is_none());
    assert!(x.thunk().is_none());
    assert!(x.errata().is_empty());
}

#[test]
fn plt_variant_and_iplt_bytes() {
    // X-37, X-38, X-43.
    let x = X64::M1;
    let cx = |feature_1_and: u32, force_bti: bool| OutputCx {
        output: mortar_arch::OutputKind::Static,
        feature_1_and,
        pac_plt: false,
        force_bti,
    };
    assert_eq!(x.plt_variant(&cx(0, false)), Ok(PltVariant::Lazy));
    assert_eq!(x.plt_variant(&cx(2, false)), Ok(PltVariant::Lazy), "SHSTK alone");
    assert_eq!(x.plt_variant(&cx(3, false)), Ok(PltVariant::Ibt));
    assert_eq!(x.plt_variant(&cx(0, true)), Ok(PltVariant::Ibt));
    assert_eq!(x.plt(PltVariant::Lazy).iplt_entry, 8);
    assert_eq!(x.plt(PltVariant::Ibt).iplt_entry, 16);

    let synth =
        SynthCx { addr: 0x40_1000, header: 0, got_plt: 0x40_3018, got_base: 0x40_3000, index: 0 };
    let mut b = [0u8; 8];
    x.write_plt(PltVariant::Lazy, PltPart::Iplt(0), &synth, &mut b).unwrap();
    // rel32 = 0x403018 − 0x401006 = 0x2012.
    assert_eq!(b, [0xff, 0x25, 0x12, 0x20, 0, 0, 0x66, 0x90]);
    let mut b = [0u8; 16];
    x.write_plt(PltVariant::Ibt, PltPart::Iplt(0), &synth, &mut b).unwrap();
    assert_eq!(
        b,
        [0xf3, 0x0f, 0x1e, 0xfa, 0xff, 0x25, 0x0e, 0x20, 0, 0, 0x66, 0x0f, 0x1f, 0x44, 0, 0]
    );
    assert!(x.write_plt(PltVariant::Lazy, PltPart::Header(0), &synth, &mut [0; 16]).is_err());
}

#[test]
fn property_merge_is_the_x86_rule() {
    // X-41.
    for t in [0xc000_0002, 0xc000_8002, 0xc001_0001, 0xc001_0002, 0xc000_0001, 5] {
        assert_eq!(X64::M1.property_merge(t), x86_property_merge(t), "{t:#x}");
    }
}

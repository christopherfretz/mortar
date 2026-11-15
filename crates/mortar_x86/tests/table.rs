//! The table of x86.md §6 as data: every type number classified, every
//! row against §6.1, the names against `mortar_elf`, the facts of X86-47,
//! X86-64 and X86-65, the windows of X86-40 and the shapes of §11.

use mortar_arch::{
    Arch, Base, Check, DemandClass, DynRole, Field, Howto, Implicit, Minus, OutputCx, OutputKind,
    PltPart, PltVariant, RefName, RelForm, Site, SlotKind, Status, SynthCx, Window,
};
use mortar_diag::RefusalClass;
use mortar_elf::{Class, Machine};
use mortar_x86::{FACTS, MAX_REACH, TLS, X86, relax, synth, table};

/// §6.1's M1 rows.
const M1_TYPES: [u32; 20] = [0, 1, 2, 3, 4, 9, 10, 15, 16, 17, 18, 19, 20, 21, 22, 23, 32, 38, 39, 40];
const M1_ALL: [u32; 21] = [
    0, 1, 2, 3, 4, 9, 10, 15, 16, 17, 18, 19, 20, 21, 22, 23, 32, 38, 39, 40, 43,
];

fn site() -> Site<'static> {
    Site { bytes: &[0; 64], offset: 32 }
}

#[test]
fn every_type_number_is_classified_or_refused_by_name() {
    let mut accepted = Vec::new();
    for t in 0..=0x1_0000u32 {
        match X86::M1.classify(t, site()) {
            Ok(h) => {
                assert_eq!(h.status, Status::M1, "{t}");
                accepted.push(t);
            }
            Err(r) => {
                assert_eq!(r.class, RefusalClass::RelocationType, "{t}");
                match mortar_elf::reloc::i386::name(t) {
                    Some(n) => assert_eq!(r.name, RefName::Named(n), "{t}"),
                    None => assert_eq!(r.name, RefName::Hex(u64::from(t)), "{t}"),
                }
            }
        }
    }
    assert_eq!(accepted, M1_ALL, "§6.1: 21 M1 types");
    assert_eq!(&accepted[..20], &M1_TYPES);
}

#[test]
fn refused_types_of_6_2_and_6_3() {
    // X86-20: the dynamic types; X86-14: 33, 34; §6.3: 11, the Sun TLS
    // types, 12, 13, ≥ 44, 200.
    let named = [5, 6, 7, 8, 11, 14, 24, 25, 26, 27, 28, 29, 30, 31, 33, 34, 35, 36, 37, 41, 42];
    for t in named {
        let r = X86::M1.classify(t, site()).unwrap_err();
        assert_eq!(r.class, RefusalClass::RelocationType, "{t}");
        assert!(matches!(r.name, RefName::Named(n) if n.starts_with("R_386_")), "{t}");
    }
    for t in [12, 13, 44, 45, 200, 0xff, 0xffff_ffff] {
        let r = X86::M1.classify(t, site()).unwrap_err();
        assert_eq!(r.name, RefName::Hex(u64::from(t)), "{t}");
    }
    assert_eq!(table::status(33), Status::Later);
    assert_eq!(table::status(34), Status::Later);
    assert_eq!(table::status(11), Status::Refused);
}

/// One row of §6.1: name, class, field width, check.
type Row = (u32, &'static str, DemandClass, u8, Check);

const ROWS: [Row; 21] = {
    use Check::{Either, None as N, Signed, Unsigned};
    use DemandClass as C;
    [
        (0, "R_386_NONE", C::None, 0, N),
        (1, "R_386_32", C::AbsWord, 4, N),
        (2, "R_386_PC32", C::Pc, 4, N),
        (3, "R_386_GOT32", C::Got, 4, N),
        (4, "R_386_PLT32", C::Branch, 4, N),
        (9, "R_386_GOTOFF", C::GotRel, 4, N),
        (10, "R_386_GOTPC", C::GotBaseRef, 4, N),
        (15, "R_386_TLS_IE", C::TlsIe, 4, N),
        (16, "R_386_TLS_GOTIE", C::TlsIe, 4, N),
        (17, "R_386_TLS_LE", C::TlsLe, 4, Signed(32)),
        (18, "R_386_TLS_GD", C::TlsGd, 4, N),
        (19, "R_386_TLS_LDM", C::TlsLd, 4, N),
        (20, "R_386_16", C::AbsNarrow, 2, Either(16)),
        (21, "R_386_PC16", C::Pc, 2, Signed(16)),
        (22, "R_386_8", C::AbsNarrow, 1, Either(8)),
        (23, "R_386_PC8", C::Pc, 1, Signed(8)),
        (32, "R_386_TLS_LDO_32", C::TlsDtpRel, 4, N),
        (38, "R_386_SIZE32", C::Size, 4, Unsigned(32)),
        (39, "R_386_TLS_GOTDESC", C::TlsDesc, 4, N),
        (40, "R_386_TLS_DESC_CALL", C::TlsDesc, 0, N),
        (43, "R_386_GOT32X", C::Got, 4, N),
    ]
};

#[test]
fn rows_of_6_1() {
    for (t, name, class, width, check) in ROWS {
        let h: Howto = table::howto(t).unwrap();
        assert_eq!(h.name, name, "{t}");
        assert_eq!(Some(name), mortar_elf::reloc::i386::name(t), "{t}");
        assert_eq!(h.class, class, "{t}");
        assert_eq!(h.field.width(), width, "{t}");
        assert_eq!(h.check, check, "{t}");
        // X86-4: the implicit addend is the field, sign-extended.
        let imp = (width > 0).then_some(Implicit { bytes: width, signed: true, shift: 0 });
        assert_eq!(h.implicit, imp, "{t}");
        assert_eq!(h.window, Window::default(), "{t}");
        if width > 0 {
            assert_eq!(h.field, Field::Data { bytes: width }, "{t}");
        }
    }
}

#[test]
fn values_of_6_1() {
    let v = |t: u32| table::howto(t).unwrap().value;
    assert_eq!((v(1).base, v(1).minus), (Base::S, Minus::None));
    assert_eq!((v(2).base, v(2).minus), (Base::S, Minus::P));
    assert_eq!((v(3).base, v(3).minus), (Base::G(SlotKind::Addr), Minus::GotBase));
    assert_eq!((v(43).base, v(43).minus), (Base::G(SlotKind::Addr), Minus::GotBase));
    assert_eq!((v(4).base, v(4).minus), (Base::L, Minus::P));
    assert_eq!((v(9).base, v(9).minus), (Base::S, Minus::GotBase));
    assert_eq!((v(10).base, v(10).minus), (Base::GotBase, Minus::P));
    let tp = SlotKind::TpOff { negated: false };
    assert_eq!((v(15).base, v(15).minus), (Base::G(tp), Minus::None));
    assert_eq!((v(16).base, v(16).minus), (Base::G(tp), Minus::GotBase));
    assert_eq!(v(17).base, Base::TpRel);
    assert_eq!(v(32).base, Base::DtpRel);
    assert_eq!(v(38).base, Base::Size);
    for t in [1, 2, 3, 4, 9, 10, 15, 16, 20, 21, 22, 23, 38, 43] {
        assert!(v(t).add_a, "{t}");
    }
}

#[test]
fn read_addend_is_the_field_sign_extended() {
    // X86-4, X86-6.
    let h = |t| table::howto(t).unwrap();
    let x = X86::M1;
    assert_eq!(x.read_addend(&h(1), &[0xfc, 0xff, 0xff, 0xff]), -4);
    assert_eq!(x.read_addend(&h(1), &[0x10, 0, 0, 0x80]), -0x7fff_fff0);
    assert_eq!(x.read_addend(&h(20), &[0xfe, 0xff]), -2);
    assert_eq!(x.read_addend(&h(21), &[0xff, 0x7f]), 0x7fff);
    assert_eq!(x.read_addend(&h(22), &[0x80]), -128);
    assert_eq!(x.read_addend(&h(23), &[0x7f]), 127);
    assert_eq!(x.read_addend(&h(40), &[]), 0);
    // X86-7 (`-r`, later): write-back within the width.
    let mut f = [0u8; 2];
    x.write_addend(&h(20), &mut f, -2).unwrap();
    assert_eq!(f, [0xfe, 0xff]);
    assert!(x.write_addend(&h(20), &mut f, 0x1_0000).is_err());
}

#[test]
fn facts_of_x86_47_64_65() {
    assert_eq!(FACTS.machine, Machine::I386);
    assert_eq!(FACTS.class, Class::Elf32);
    assert_eq!(FACTS.e_machine, 3);
    assert_eq!(FACTS.rel_form, RelForm::Rel);
    assert_eq!(FACTS.word, 4);
    assert_eq!(FACTS.osabi, 0);
    assert_eq!((FACTS.max_page, FACTS.common_page), (0x1000, 0x1000));
    assert_eq!(FACTS.image_base, 0x0804_8000);
    assert_eq!(FACTS.got_plt_reserved, 3);
    assert_eq!(FACTS.got_header, 0);
    assert_eq!(FACTS.code_fill, &[0xCC]);
    assert_eq!(FACTS.code_nop, &[0x90]);
    // X86-40, X86-41.
    assert_eq!(MAX_REACH, Window { before: 3, after: 6 });
    assert_eq!(FACTS.max_reach, MAX_REACH);
    assert_eq!(FACTS.seq_leads, &[18, 19]);
    assert_eq!(relax::SEQ_LEADS, [18, 19]);
    assert_eq!(TLS.get_addr, b"___tls_get_addr");
    assert!(X86::M1.thunk().is_none());
    // X86-1.
    assert_eq!(X86::M1.e_flags(&mut [0u32, 0].into_iter()), Ok(0));
    assert!(X86::M1.e_flags(&mut [0u32, 1].into_iter()).is_err());
}

#[test]
fn dynamic_types_of_6_2() {
    let d = |r| X86::M1.dyn_type(r);
    assert_eq!(d(DynRole::Symbolic), Some(1));
    assert_eq!(d(DynRole::Copy), Some(5));
    assert_eq!(d(DynRole::GlobDat), Some(6));
    assert_eq!(d(DynRole::JumpSlot), Some(7));
    assert_eq!(d(DynRole::Relative), Some(8));
    assert_eq!(d(DynRole::TpOff { negated: false }), Some(14));
    assert_eq!(d(DynRole::DtpMod), Some(35));
    assert_eq!(d(DynRole::DtpOff), Some(36));
    assert_eq!(d(DynRole::TpOff { negated: true }), Some(37));
    assert_eq!(d(DynRole::TlsDesc), Some(41));
    assert_eq!(d(DynRole::IRelative), Some(42));
}

fn ocx(output: OutputKind, feature_1_and: u32) -> OutputCx {
    OutputCx { output, feature_1_and, pac_plt: false, force_bti: false }
}

#[test]
fn iplt_entries_of_x86_48_49() {
    let x = X86::M1;
    assert_eq!(x.plt_variant(&ocx(OutputKind::Static, 0)), Ok(PltVariant::Abs));
    assert_eq!(x.plt_variant(&ocx(OutputKind::Pie, 0)), Ok(PltVariant::Pic));
    assert_eq!(x.plt_variant(&ocx(OutputKind::Static, 3)), Ok(PltVariant::IbtAbs));
    assert_eq!(x.plt_variant(&ocx(OutputKind::Pie, 1)), Ok(PltVariant::IbtPic));
    assert_eq!(x.plt(PltVariant::Abs).iplt_entry, 8);
    assert_eq!(x.plt(PltVariant::IbtPic).iplt_entry, 16);
    let cx = SynthCx { addr: 0x0804_9020, header: 0, got_plt: 0x0804_c00c, got_base: 0x0804_c000, index: 0 };
    let mut out = [0u8; 16];
    x.write_plt(PltVariant::Abs, PltPart::Iplt(0), &cx, &mut out).unwrap();
    assert_eq!(out[..8], [0xff, 0x25, 0x0c, 0xc0, 0x04, 0x08, 0x66, 0x90]);
    x.write_plt(PltVariant::Pic, PltPart::Iplt(0), &cx, &mut out).unwrap();
    assert_eq!(out[..8], [0xff, 0xa3, 0x0c, 0, 0, 0, 0x66, 0x90]);
    x.write_plt(PltVariant::IbtAbs, PltPart::Iplt(0), &cx, &mut out).unwrap();
    assert_eq!(
        out,
        [0xf3, 0x0f, 0x1e, 0xfb, 0xff, 0x25, 0x0c, 0xc0, 0x04, 0x08, 0x66, 0x0f, 0x1f, 0x44, 0, 0]
    );
    x.write_plt(PltVariant::IbtPic, PltPart::Iplt(0), &cx, &mut out).unwrap();
    assert_eq!(out[4..10], [0xff, 0xa3, 0x0c, 0, 0, 0]);
    // §11.4: the lazy PLT is M2.
    assert!(x.write_plt(PltVariant::Abs, PltPart::Header(0), &cx, &mut out).is_err());
}

#[test]
fn slots_and_got_plt_of_x86_46_47() {
    let tls = mortar_arch::TlsBlock { start: 0x0804_b000, size: 0x10, align: 4 };
    let tp = SlotKind::TpOff { negated: false };
    assert_eq!(synth::static_slot(SlotKind::Addr, Some(0x0804_9000), None, &TLS), Ok(0x0804_9000));
    assert_eq!(synth::static_slot(SlotKind::Addr, None, None, &TLS), Ok(0));
    assert_eq!(synth::static_slot(tp, Some(0x0804_b004), Some(tls), &TLS), Ok(-12i32 as u32));
    assert_eq!(synth::static_slot(tp, None, None, &TLS), Ok(0));
    assert!(synth::static_slot(SlotKind::TlsGd, None, None, &TLS).is_err());
    assert_eq!(synth::got_plt_reserved(None), [0, 0, 0]);
    assert_eq!(synth::got_plt_reserved(Some(0xa4ef4)), [0xa4ef4, 0, 0]);
    assert_eq!((X86::M1.got().slot_words)(SlotKind::Addr), 1);
}

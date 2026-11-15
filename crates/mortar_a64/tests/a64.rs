//! Behavioural tests of `mortar_a64`, by section of `spec/a64.md`.
#![allow(clippy::many_single_char_names)]

mod support;

use mortar_a64::forms::{self, insert, requires};
use mortar_a64::{A64, synth, table};
use mortar_arch::{
    Arch, DemandFlags, DynRole, EventClass, FieldError, GlobalFlags, Howto, Incompatible,
    OutputKind, PlaceFacts, PltPart, PltVariant, PropMerge, Range, RefName, Reloc, ScanError, Site,
    SlotKind, Status, SynthCx, TargetKind, TlsBlock, Weak0Rule,
};
use mortar_diag::RefusalClass;
use support::{Case, M2, Tables, key, raised, scan_one, words};

fn rng(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}

// ---------------------------------------------------------------- §3.2

/// The bit moves of each form, transcribed from the `inserts` column of
/// §3.2: `(lowest bit of X, length, lowest bit of the word)`.
fn moves(form: u16) -> Vec<(u32, u32, u32)> {
    match form {
        forms::ADR => vec![(0, 2, 29), (2, 19, 5)],
        forms::ADRP => vec![(12, 2, 29), (14, 19, 5)],
        forms::ADD12 => vec![(0, 12, 10)],
        forms::HI12 => vec![(12, 12, 10)],
        // The value fills the whole imm12 field, zero-extended.
        f @ 10..=14 => {
            let n = u32::from(f - forms::LS12);
            vec![(n, 12, 10)]
        }
        forms::LDX12 | forms::LS15 => vec![(3, 12, 10)],
        forms::LIT19 | forms::LDX19 | forms::BC19 => vec![(2, 19, 5)],
        forms::TB14 => vec![(2, 14, 5)],
        forms::B26 => vec![(2, 26, 0)],
        f @ 30..=33 => vec![(16 * u32::from(f - forms::MOV), 16, 5)],
        _ => unreachable!(),
    }
}

fn expect_insert(form: u16, w: u32, x: i64) -> u32 {
    let mut out = w;
    for (src, len, dst) in moves(form) {
        let field = ((x as u64 >> src) & ((1 << len) - 1)) as u32;
        // ls12/n and ldx12 take X[11:n] only (§3.2).
        let field = match form {
            10..=14 | forms::LDX12 => field & ((1 << (12 - src)) - 1),
            _ => field,
        };
        let mask = (((1u64 << len) - 1) as u32) << dst;
        out = (out & !mask) | (field << dst);
    }
    out
}

/// A random word that `form` requires.
fn word_for(form: u16, seed: &mut u64) -> u32 {
    loop {
        let w = rng(seed) as u32;
        if requires(form, w) {
            return w;
        }
    }
}

#[test]
fn s3_2_every_form_inserts_exactly_its_bits() {
    let mut seed = 0x2545_F491_4F6C_DD1D;
    for &(form, name) in &forms::ALL {
        for _ in 0..500 {
            let w = word_for(form, &mut seed);
            let x = rng(&mut seed) as i64;
            if form >= forms::MOVS {
                let k = 16 * u32::from(form - forms::MOVS);
                let got = insert(form, w, x);
                let (imm, opc) = if x >= 0 {
                    ((x as u64 >> k) & 0xFFFF, 0b10)
                } else {
                    ((!x as u64 >> k) & 0xFFFF, 0b00)
                };
                let want = (w & !0x601F_FFE0) | (imm as u32) << 5 | opc << 29;
                assert_eq!(got, want, "{name} {w:#x} {x:#x}");
                assert!(requires(form, got), "{name}: still a MOVZ/MOVN");
            } else {
                assert_eq!(insert(form, w, x), expect_insert(form, w, x), "{name} {w:#x} {x:#x}");
                assert!(requires(form, insert(form, w, x)), "{name}: insertion keeps the opcode");
            }
        }
    }
}

#[test]
fn s3_2_extract_inverts_insert() {
    let mut seed = 7;
    for &(form, name) in &forms::ALL {
        for _ in 0..200 {
            let w = word_for(form, &mut seed);
            let x = rng(&mut seed) as i64;
            let back = forms::extract(form, insert(form, w, x));
            // Re-inserting what was read gives the same word.
            assert_eq!(insert(form, w, back), insert(form, w, x), "{name}");
        }
    }
}

/// Representative words of each instruction class.
const ADRP_X0: u32 = 0x9000_0000;
const ADR_X0: u32 = 0x1000_0000;
const ADD_X0: u32 = 0x9100_0000;
const ADD_X0_LSL12: u32 = 0x9140_0000;
const LDRB: u32 = 0x3940_0000;
const LDRH: u32 = 0x7940_0000;
const LDR_W: u32 = 0xB940_0000;
const LDR_X: u32 = 0xF940_0000;
const LDR_Q: u32 = 0x3DC0_0000;
const LDR_LIT_W: u32 = 0x1800_0000;
const LDR_LIT_X: u32 = 0x5800_0000;
const B_EQ: u32 = 0x5400_0000;
const BC_EQ: u32 = 0x5400_0010;
const CBZ_X: u32 = 0xB400_0000;
const TBZ: u32 = 0x3600_0000;
const BL: u32 = 0x9400_0000;
const MOVZ_X0: u32 = 0xD280_0000;
const MOVK_X0: u32 = 0xF280_0000;
const MOVN_X0: u32 = 0x9280_0000;

#[test]
fn s3_2_every_form_against_every_instruction() {
    let samples: &[(&str, u32)] = &[
        ("adrp", ADRP_X0),
        ("adr", ADR_X0),
        ("add", ADD_X0),
        ("add lsl12", ADD_X0_LSL12),
        ("ldrb", LDRB),
        ("ldrh", LDRH),
        ("ldr w", LDR_W),
        ("ldr x", LDR_X),
        ("ldr q", LDR_Q),
        ("ldr lit w", LDR_LIT_W),
        ("ldr lit x", LDR_LIT_X),
        ("b.eq", B_EQ),
        ("bc.eq", BC_EQ),
        ("cbz", CBZ_X),
        ("tbz", TBZ),
        ("bl", BL),
        ("movz", MOVZ_X0),
        ("movk", MOVK_X0),
        ("movn", MOVN_X0),
    ];
    // The forms that accept each sample, from §3.2's `requires` column.
    let accepts = |form: u16, sample: &str| -> bool {
        match form {
            forms::ADRP => sample == "adrp",
            forms::ADR => sample == "adr",
            forms::ADD12 => sample == "add",
            forms::HI12 => sample == "add lsl12",
            10 => sample == "ldrb",
            11 => sample == "ldrh",
            12 => sample == "ldr w",
            13 | forms::LDX12 | forms::LS15 => sample == "ldr x",
            14 => sample == "ldr q",
            forms::LIT19 => sample.starts_with("ldr lit"),
            forms::LDX19 => sample == "ldr lit x",
            forms::BC19 => matches!(sample, "b.eq" | "bc.eq" | "cbz"),
            forms::TB14 => sample == "tbz",
            forms::B26 => sample == "bl",
            30 => matches!(sample, "movz" | "movk" | "movn"),
            40 => matches!(sample, "movz" | "movn"),
            // hw = 0 in every sample.
            _ => false,
        }
    };
    for &(form, name) in &forms::ALL {
        for &(sample, w) in samples {
            assert_eq!(requires(form, w), accepts(form, sample), "{name} on {sample}");
        }
    }
    // mov/k: hw must be k, and sf = 1 from k = 2.
    assert!(requires(forms::MOV + 1, 0xF2A0_0000)); // movk x0, lsl 16
    assert!(requires(forms::MOV + 2, 0xF2C0_0000));
    assert!(!requires(forms::MOV + 2, 0x72C0_0000)); // sf = 0, hw = 2: unallocated
    assert!(requires(forms::MOV + 3, 0xD2E0_0000));
    assert!(!requires(forms::MOVS + 1, 0xF2A0_0000)); // movk is not movs
    // PRFM counts as 8 bytes.
    assert!(requires(forms::LS12 + 3, 0xF980_0000));
    // lit19 takes PRFM (literal).
    assert!(requires(forms::LIT19, 0xD800_0000));
}

#[test]
fn s3_2_movs_writes_movz_or_movn() {
    // MOVW_SABS_G0 with X = −2: MOVN of 1.
    assert_eq!(insert(forms::MOVS, MOVZ_X0, -2), 0x9280_0020);
    assert_eq!(insert(forms::MOVS, MOVN_X0, 5), 0xD280_00A0);
    // G1 of −0x10000: !X = 0xFFFF, chunk 1 = 0.
    assert_eq!(insert(forms::MOVS + 1, 0xD2A0_0000, -0x1_0000), 0x92A0_0000);
}

#[test]
fn s3_2_feat_hbc_bc_cond() {
    // BC.cond keeps bit 4 (the reviewer case).
    let w = insert(forms::BC19, BC_EQ, 0x40);
    assert_eq!(w, 0x5400_0210);
    assert!(requires(forms::BC19, w));
}

// ---------------------------------------------------------------- §6.1, §1

#[test]
fn s6_1_classify_every_type_number() {
    let site = Site { bytes: &[], offset: 0 };
    let m1 = A64::M1;
    let mut by_status = [0u32; 4];
    for r_type in 0..=0xFFFF_u32 {
        let row = table::howto(r_type);
        // The compile-time table is the match, row for row (WP-032b).
        assert_eq!(table::lookup(r_type).copied(), row, "{r_type}");
        if let Some(h) = row {
            by_status[h.status as usize] += 1;
            assert_eq!(Some(h.name), mortar_elf::reloc::aarch64::name(r_type), "{r_type}");
            assert!(!table::refused(r_type));
        }
        let dynamic = matches!(r_type, 1024..=1027 | 1030..=1032 | 1041..=1044);
        if table::refused(r_type) && !dynamic {
            by_status[Status::Refused as usize] += 1;
        }
        for arch in [m1, M2] {
            match arch.classify(r_type, site) {
                Ok(h) => {
                    assert!(arch.enables(h.status));
                    assert_eq!(row, Some(h));
                }
                Err(r) => {
                    assert_eq!(r.class, RefusalClass::RelocationType);
                    match mortar_elf::reloc::aarch64::name(r_type) {
                        Some(n) => assert_eq!(r.name, RefName::Named(n)),
                        None => assert_eq!(r.name, RefName::Hex(u64::from(r_type))),
                    }
                }
            }
        }
    }
    // a64.md §1: 61 M1, 41 M2, 16 later, 22 refused.
    assert_eq!(by_status, [61, 41, 16, 22]);
    // Row 313, M1 since WP-027d (GCC's `-fpie -O0` GOT loads; not in the census).
    assert!(m1.classify(313, site).is_ok());
    // The census's 34 types: 32 M1, the traditional TLSGD pair M2.
    let census = [
        257, 258, 260, 261, 264, 266, 268, 269, 273, 274, 275, 277, 278, 280, 282, 283, 284, 285,
        286, 299, 309, 311, 312, 513, 514, 541, 542, 543, 549, 551, 562, 563, 564, 569,
    ];
    let m1_count = census.iter().filter(|&&t| m1.classify(t, site).is_ok()).count();
    assert_eq!(m1_count, 32);
    assert!(M2.classify(513, site).is_ok() && m1.classify(513, site).is_err());
    // Refused and unknown.
    let hex = |t: u32| m1.classify(t, site).unwrap_err().to_string_name();
    assert_eq!(hex(0xE000), "0xe000");
    assert_eq!(hex(281), "0x119");
    assert_eq!(hex(598), "0x256");
    assert_eq!(hex(1028), "R_AARCH64_TLS_DTPMOD64");
    assert_eq!(hex(1027), "R_AARCH64_RELATIVE");
    assert_eq!(hex(580), "R_AARCH64_AUTH_ABS64");
    assert_eq!(hex(300), "R_AARCH64_MOVW_GOTOFF_G0");
    assert_eq!(
        M2.classify(300, site).unwrap_err().name,
        RefName::Named("R_AARCH64_MOVW_GOTOFF_G0")
    );
}

trait RefusalName {
    fn to_string_name(&self) -> String;
}
impl RefusalName for mortar_arch::Refusal {
    fn to_string_name(&self) -> String {
        self.name.to_string()
    }
}

// ---------------------------------------------------------------- reviewer cases

const ALLOC: PlaceFacts = PlaceFacts { alloc: true, writable: false, exec: None };

fn scan_err(arch: A64, tables: &Tables, input: &[u8], r: Reloc) -> Vec<ScanError> {
    scan_one(arch, tables, OutputKind::Static, ALLOC, input, r)
        .unwrap()
        .errors
        .into_iter()
        .map(|e| e.1)
        .collect()
}

#[test]
fn s3_2_ldr_w_with_ld64_got_lo12_is_refused() {
    let mut t = Tables::default();
    t.sym(key(1), 0x41_0000, TargetKind::Local);
    let input = words(&[LDR_W]);
    let r = Reloc { r_offset: 0, key: key(1), r_type: 312, addend: 0 };
    assert_eq!(
        scan_err(A64::M1, &t, &input, r),
        [ScanError::Incompatible(Incompatible::InstructionMismatch)]
    );
    t.slots.insert((key(1), SlotKind::Addr), 0x42_0008);
    let c = Case::new(&t, &input, 0x40_0000);
    assert_eq!(c.word(312, 0, key(1), 0), Err(FieldError::Instruction { word: LDR_W }));
}

#[test]
fn s3_2_hi12_with_sh0_is_refused() {
    let mut t = Tables::default();
    t.tls_sym(key(1), 0x42_5000, TargetKind::Local);
    let input = words(&[ADD_X0]);
    for (arch, r_type) in [(M2, 528), (A64::M1, 549)] {
        let r = Reloc { r_offset: 0, key: key(1), r_type, addend: 0 };
        assert_eq!(
            scan_err(arch, &t, &input, r),
            [ScanError::Incompatible(Incompatible::InstructionMismatch)]
        );
    }
    // With sh = 1 it is accepted and inserts X[23:12].
    let input = words(&[ADD_X0_LSL12]);
    let r = Reloc { r_offset: 0, key: key(1), r_type: 549, addend: 0 };
    assert!(scan_err(A64::M1, &t, &input, r).is_empty());
    let mut c = Case::new(&t, &input, 0x40_0000);
    c.tls = Some(TlsBlock { start: 0x42_0000, size: 0x6000, align: 16 });
    // TPREL = 0x5000 + 16.
    assert_eq!(c.word(549, 0, key(1), 0).unwrap(), ADD_X0_LSL12 | 5 << 10);
}

#[test]
fn s3_1_nonzero_addend_on_got_types() {
    let mut t = Tables::default();
    t.sym(key(1), 0x41_0000, TargetKind::Local);
    for (r_type, w) in
        [(311, ADRP_X0), (312, LDR_X), (309, LDR_LIT_X), (541, ADRP_X0), (562, ADRP_X0)]
    {
        let input = words(&[w]);
        let r = Reloc { r_offset: 0, key: key(1), r_type, addend: 8 };
        let errs = scan_err(A64::M1, &t, &input, r);
        assert_eq!(errs, [ScanError::Incompatible(Incompatible::AddendMustBeZero)], "{r_type}");
    }
    // GOTPCREL32 adds A outside G: allowed.
    let input = [0u8; 4];
    let r = Reloc { r_offset: 0, key: key(1), r_type: 315, addend: 8 };
    assert!(scan_err(M2, &t, &input, r).is_empty());
}

#[test]
fn s8_3_mismatched_ie_registers_keep_ie() {
    // adrp x2, :gottprel:v; ldr x3, [x2, :gottprel_lo12:v]. At M1 IE is
    // applied unrelaxed (§8.3, §12.5): both words keep their opcode and
    // registers and address the GTPREL slot.
    let mut t = Tables::default();
    t.tls_sym(key(1), 0x42_0010, TargetKind::Local);
    t.slots.insert((key(1), SlotKind::TpOff { negated: false }), 0x43_1238);
    let input = words(&[0x9000_0002, 0xF940_0043]);
    let mut c = Case::new(&t, &input, 0x40_0ff8);
    c.tls = Some(TlsBlock { start: 0x42_0000, size: 0x20, align: 16 });
    let relocs = [
        Reloc { r_offset: 0, key: key(1), r_type: 541, addend: 0 },
        Reloc { r_offset: 4, key: key(1), r_type: 542, addend: 0 },
    ];
    let out = c.apply(&relocs).unwrap();
    let w0 = u32::from_le_bytes(out[0..4].try_into().unwrap());
    let w1 = u32::from_le_bytes(out[4..8].try_into().unwrap());
    assert_eq!(w0, insert(forms::ADRP, 0x9000_0002, 0x43_1000 - 0x40_0000));
    assert_eq!(w1, 0xF940_0043 | (0x238 >> 3) << 10);
}

// ---------------------------------------------------------------- §6.1 apply

fn local(s: u64) -> Tables {
    let mut t = Tables::default();
    t.sym(key(1), s, TargetKind::Local);
    t
}

#[test]
fn s6_1_data() {
    let t = local(0x41_2340);
    let input = [0xAAu8; 16];
    let c = Case::new(&t, &input, 0x40_0000);
    let one = |r_type, off, a| c.apply(&[Reloc { r_offset: off, key: key(1), r_type, addend: a }]);
    let out = one(257, 0, 8).unwrap();
    assert_eq!(&out[..8], &0x41_2348u64.to_le_bytes());
    assert_eq!(&out[8..], &[0xAA; 8]);
    let out = one(258, 4, -0x41_2341).unwrap();
    assert_eq!(&out[4..8], &(-1i32).to_le_bytes());
    assert_eq!(&out[..4], &[0xAA; 4]);
    // su(32): −2^31 is the bound.
    assert!(matches!(one(258, 0, -0x8041_2340 - 1), Err(FieldError::Range(_))));
    assert!(one(258, 0, -0x8041_2340).is_ok());
    let out = one(261, 8, 0).unwrap(); // PREL32 at P = 0x400008
    assert_eq!(&out[8..12], &(0x1_2338i32).to_le_bytes());
    let out = one(262, 2, -0x1_2000).unwrap();
    assert_eq!(&out[2..4], &(0x41_2340u64 - 0x1_2000 - 0x40_0002).to_le_bytes()[..2]);
    assert_eq!(
        one(262, 2, 0),
        Err(FieldError::Range(Range { value: 0x1_233E, min: -32768, max: 32767 }))
    );
    let out = one(260, 8, 0).unwrap();
    assert_eq!(&out[8..16], &(0x1_2338u64).to_le_bytes());
    assert_eq!(
        one(259, 0, 0x10000 - 0x41_2340),
        Err(FieldError::Range(Range { value: 0x10000, min: -32768, max: 65535 }))
    );
    // A field outside the bytes.
    assert!(matches!(one(257, 12, 0), Err(FieldError::Outside { .. })));
}

#[test]
fn s6_1_pc_relative_and_lo12() {
    let t = local(0x41_2344);
    let input = words(&[ADRP_X0, ADD_X0, LDR_X, LDR_LIT_X, ADR_X0, LDRB, LDRH, LDR_W, LDR_Q]);
    let c = Case::new(&t, &input, 0x40_0ffc);
    let w = |r_type, off, a| c.word(r_type, off, key(1), a);
    // ADR_PREL_PG_HI21 at P = 0x400ffc: Page(S) − Page(P) = 0x12000.
    assert_eq!(w(275, 0, 0).unwrap(), insert(forms::ADRP, ADRP_X0, 0x1_2000));
    assert_eq!(w(276, 0, 0).unwrap(), insert(forms::ADRP, ADRP_X0, 0x1_2000));
    assert_eq!(w(277, 4, 0).unwrap(), ADD_X0 | 0x344 << 10);
    // LDST64 needs a(8): S + A = 0x412344 is not.
    assert_eq!(w(286, 8, 0), Err(FieldError::Misaligned { value: 0x41_2344, align: 8 }));
    assert_eq!(w(286, 8, 4).unwrap(), LDR_X | (0x348 >> 3) << 10);
    assert_eq!(w(278, 20, 1).unwrap(), LDRB | 0x345 << 10);
    assert_eq!(w(284, 24, 0).unwrap(), LDRH | (0x344 >> 1) << 10);
    assert_eq!(w(285, 28, 0).unwrap(), LDR_W | (0x344 >> 2) << 10);
    assert_eq!(w(299, 32, 0xC).unwrap(), LDR_Q | (0x350 >> 4) << 10);
    // LD_PREL_LO19 at P = 0x401008: X = 0x1133C.
    assert_eq!(w(273, 12, 0).unwrap(), insert(forms::LIT19, LDR_LIT_X, 0x1_133C));
    assert_eq!(w(273, 12, 2), Err(FieldError::Misaligned { value: 0x1_133E, align: 4 }));
    // ADR_PREL_LO21 at P = 0x40100c, s(21).
    assert_eq!(w(274, 16, 0).unwrap(), insert(forms::ADR, ADR_X0, 0x1_1338));
    assert!(matches!(w(274, 16, 0x10_0000), Err(FieldError::Range(_))));
}

#[test]
fn s6_1_movw() {
    let mut t = local(0x1234_5678_9ABC_DEF0);
    t.sym(key(2), (-0x1_2345_i64) as u64, TargetKind::Abs);
    let input =
        words(&[MOVZ_X0, 0xF2A0_0000, 0xF2C0_0000, 0xF2E0_0000, MOVZ_X0, 0xD2A0_0000, 0xD2C0_0000]);
    let c = Case::new(&t, &input, 0x40_0000);
    let w = |r_type, off, k| c.word(r_type, off, k, 0);
    assert_eq!(w(264, 0, key(1)).unwrap(), MOVZ_X0 | 0xDEF0 << 5);
    assert_eq!(w(266, 4, key(1)).unwrap(), 0xF2A0_0000 | 0x9ABC << 5);
    assert_eq!(w(268, 8, key(1)).unwrap(), 0xF2C0_0000 | 0x5678 << 5);
    assert_eq!(w(269, 12, key(1)).unwrap(), 0xF2E0_0000 | 0x1234 << 5);
    assert!(matches!(w(263, 0, key(1)), Err(FieldError::Range(_))));
    assert!(matches!(w(265, 4, key(1)), Err(FieldError::Range(_))));
    // A negative absolute: the MOVN path of movs.
    assert_eq!(
        w(270, 16, key(2)),
        Err(FieldError::Range(Range { value: -0x1_2345, min: -65536, max: 65535 }))
    );
    assert_eq!(w(271, 20, key(2)).unwrap(), 0x92A0_0000 | 0x1 << 5);
    assert_eq!(w(272, 24, key(2)).unwrap(), 0x92C0_0000);
}

#[test]
fn s6_1_branches() {
    let t = local(0x40_0000 + (1 << 27) - 4);
    let input = words(&[BL, BL, B_EQ, TBZ]);
    let c = Case::new(&t, &input, 0x40_0000);
    let w = |r_type, off, a| c.word(r_type, off, key(1), a);
    assert_eq!(w(283, 0, 0).unwrap(), BL | ((1 << 25) - 1));
    assert_eq!(
        w(283, 0, 4),
        Err(FieldError::Range(Range { value: 1 << 27, min: -(1 << 27), max: (1 << 27) - 1 }))
    );
    assert_eq!(w(282, 4, 2), Err(FieldError::Misaligned { value: (1 << 27) - 6, align: 4 }));
    assert!(matches!(w(280, 8, 0), Err(FieldError::Range(_))));
    assert!(matches!(w(279, 12, 0), Err(FieldError::Range(_))));
    let near = local(0x40_0100);
    let c = Case::new(&near, &input, 0x40_0000);
    assert_eq!(c.word(280, 8, key(1), 0).unwrap(), B_EQ | (0xF8 >> 2) << 5);
    assert_eq!(c.word(279, 12, key(1), 0).unwrap(), TBZ | (0xF4 >> 2) << 5);
    assert_eq!(c.word(280, 8, key(1), -(1 << 20) + 8 - 0x100).unwrap() & 0x00FF_FFE0, 0x0080_0000);
}

#[test]
fn s9_branch_to_its_thunk() {
    let mut t = local(0x900_0000);
    t.redirects.insert((mortar_core::SectionId(3), 4), 0x40_1000);
    let input = words(&[BL, BL]);
    let c = Case::new(&t, &input, 0x40_0000);
    // The record at offset 4 is redirected: X = thunk − P.
    assert_eq!(c.word(283, 4, key(1), 0).unwrap(), BL | (0xFFC >> 2));
    // The one at 0 is not: out of range.
    assert!(matches!(c.word(283, 0, key(1), 0), Err(FieldError::Range(_))));
}

#[test]
fn s6_1_plt_entry_is_the_branch_target() {
    let mut t = Tables::default();
    t.sym(key(1), 0, TargetKind::Shlib { func: true });
    t.plts.insert(key(1), 0x40_0200);
    t.flags.insert(key(1), DemandFlags::PLT);
    let input = words(&[BL]);
    let mut c = Case::new(&t, &input, 0x40_0000);
    c.output = OutputKind::Exec;
    assert_eq!(c.word(283, 0, key(1), 0).unwrap(), BL | 0x80);
}

#[test]
fn s6_1_got_forms() {
    let mut t = local(0x41_0000);
    t.slots.insert((key(1), SlotKind::Addr), 0x42_0ff8);
    let input = words(&[ADRP_X0, LDR_X, LDR_LIT_X]);
    let c = Case::new(&t, &input, 0x40_0000);
    assert_eq!(c.word(311, 0, key(1), 0).unwrap(), insert(forms::ADRP, ADRP_X0, 0x2_0000));
    assert_eq!(c.word(312, 4, key(1), 0).unwrap(), LDR_X | (0xFF8 >> 3) << 10);
    // GOT_LD_PREL19 at P = 0x400008: s(21).
    assert_eq!(c.word(309, 8, key(1), 0).unwrap(), insert(forms::LDX19, LDR_LIT_X, 0x2_0FF0));
    let mut far = local(0x41_0000);
    far.slots.insert((key(1), SlotKind::Addr), 0x50_0008);
    let c = Case::new(&far, &input, 0x40_0000);
    assert!(matches!(c.word(309, 8, key(1), 0), Err(FieldError::Range(_))));
    // A missing slot is an internal error.
    let bare = local(0x41_0000);
    let c = Case::new(&bare, &input, 0x40_0000);
    assert_eq!(c.word(311, 0, key(1), 0), Err(FieldError::MissingSlot));
}

#[test]
fn s6_1_tls_le() {
    let mut t = Tables::default();
    t.tls_sym(key(1), 0x42_0010, TargetKind::Local);
    let input = words(&[0xD2C0_0000, 0xF2A0_0000, MOVK_X0, ADD_X0, LDR_X, LDRB]);
    let mut c = Case::new(&t, &input, 0x40_0000);
    c.tls = Some(TlsBlock { start: 0x42_0000, size: 0x100, align: 64 });
    // TPREL = 0x10 + align_up(16, 64) = 0x50.
    let w = |r_type, off, a| c.word(r_type, off, key(1), a);
    assert_eq!(w(544, 0, 0).unwrap(), 0xD280_0000 | 2 << 21);
    assert_eq!(w(546, 4, 0).unwrap(), 0xF2A0_0000);
    assert_eq!(w(548, 8, 0).unwrap(), MOVK_X0 | 0x50 << 5);
    assert_eq!(w(551, 12, 4).unwrap(), ADD_X0 | 0x54 << 10);
    assert_eq!(
        w(550, 12, 0x1000),
        Err(FieldError::Range(Range { value: 0x1050, min: 0, max: 4095 }))
    );
    assert_eq!(w(559, 16, 0).unwrap(), LDR_X | (0x50 >> 3) << 10);
    assert_eq!(w(559, 16, 4), Err(FieldError::Misaligned { value: 0x54, align: 8 }));
    assert_eq!(
        w(552, 20, 0xFFF),
        Err(FieldError::Range(Range { value: 0x104F, min: 0, max: 4095 }))
    );
    // No template: an internal error.
    let c = Case::new(&t, &input, 0x40_0000);
    assert_eq!(c.word(551, 12, key(1), 0), Err(FieldError::MissingTerm("PT_TLS")));
}

// ---------------------------------------------------------------- §8.1 R1

#[test]
fn s8_1_r1_tlsdesc_to_le() {
    let mut t = Tables::default();
    t.tls_sym(key(1), 0x42_0010, TargetKind::Local);
    let blr_x1 = 0xD63F_0020;
    let input = words(&[0x9000_0000, 0xF940_0001, 0x9100_0000, blr_x1]);
    let relocs = [
        Reloc { r_offset: 0, key: key(1), r_type: 562, addend: 0 },
        Reloc { r_offset: 4, key: key(1), r_type: 563, addend: 0 },
        Reloc { r_offset: 8, key: key(1), r_type: 564, addend: 0 },
        Reloc { r_offset: 12, key: key(1), r_type: 569, addend: 0 },
    ];
    let mut c = Case::new(&t, &input, 0x40_0000);
    c.tls = Some(TlsBlock { start: 0x42_0000, size: 0x1_0000_0000, align: 16 });
    let out = c.apply(&relocs).unwrap();
    // T = 0x20.
    assert_eq!(out, words(&[0xD2A0_0000, 0xF280_0000 | 0x20 << 5, 0xD503_201F, 0xD503_201F]));
    // Any order gives the same bytes (arch.md §3.6).
    let mut rev = relocs;
    rev.reverse();
    assert_eq!(c.apply(&rev).unwrap(), out);
    // T beyond u(32) is reported on the ADR_PAGE21 record.
    t.syms.get_mut(&key(1)).unwrap().s = 0x1_4200_0000;
    let c = Case {
        tls: Some(TlsBlock { start: 0x42_0000, size: 0x2_0000_0000, align: 16 }),
        ..Case::new(&t, &input, 0x40_0000)
    };
    assert!(matches!(c.apply(&relocs[..1]), Err(FieldError::Range(_))));
    assert!(c.apply(&relocs[1..]).is_ok());
    // A non-ADRP word is never rewritten.
    let bad = words(&[ADD_X0]);
    let c = Case {
        tls: Some(TlsBlock { start: 0x42_0000, size: 0x20, align: 16 }),
        ..Case::new(&t, &bad, 0x40_0000)
    };
    assert_eq!(c.word(562, 0, key(1), 0), Err(FieldError::Instruction { word: ADD_X0 }));
}

#[test]
fn s8_1_tlsdesc_scan_demands() {
    let mut t = Tables::default();
    t.tls_sym(key(1), 0x42_0010, TargetKind::Local);
    t.tls_sym(key(2), 0, TargetKind::Shlib { func: false });
    let input = words(&[ADRP_X0]);
    let demand = |k, output| {
        let r = Reloc { r_offset: 0, key: k, r_type: 562, addend: 0 };
        let s = scan_one(M2, &t, output, ALLOC, &input, r).unwrap();
        assert!(s.errors.is_empty(), "{:?}", s.errors);
        s.raised.iter().fold(DemandFlags::empty(), |a, (_, f)| a | *f)
    };
    assert_eq!(demand(key(1), OutputKind::Static), DemandFlags::empty());
    assert_eq!(demand(key(2), OutputKind::Exec), DemandFlags::GOT_TPREL);
    assert_eq!(demand(key(1), OutputKind::Shared), DemandFlags::TLS_DESC);
}

// ---------------------------------------------------------------- §7

#[test]
fn s7_undefined_weak() {
    let mut t = Tables::default();
    t.sym(key(1), 0x1234, TargetKind::Weak0); // `s` is ignored
    t.slots.insert((key(1), SlotKind::Addr), 0x42_0000);
    let input = words(&[0, 0, ADRP_X0, ADD_X0, BL, BL, B_EQ, TBZ, ADR_X0, LDR_LIT_X, LDR_X]);
    let c = Case::new(&t, &input, 0x40_0ff8);
    let one = |r_type, off, a| {
        c.apply(&[Reloc { r_offset: off, key: key(1), r_type, addend: a }]).unwrap()
    };
    let word = |out: Vec<u8>, off: usize| u32::from_le_bytes(out[off..off + 4].try_into().unwrap());
    // AW, AN: S = 0.
    assert_eq!(&one(257, 0, 5)[..8], &5u64.to_le_bytes());
    // PC data: S = P, X = A.
    assert_eq!(&one(261, 0, 5)[..4], &5u32.to_le_bytes());
    assert_eq!(&one(260, 0, 5)[..8], &5u64.to_le_bytes());
    // ADRP: Page(P + A) − Page(P), P = 0x401000.
    assert_eq!(word(one(275, 8, 0x1000), 8), insert(forms::ADRP, ADRP_X0, 0x1000));
    assert_eq!(word(one(275, 8, 0), 8), ADRP_X0);
    // LO: S = 0, X = A.
    assert_eq!(word(one(277, 12, 0x10), 12), ADD_X0 | 0x10 << 10);
    // BR, BC: X = 4.
    for (r_type, off, w) in [(283, 16, BL), (282, 20, BL), (280, 24, B_EQ), (279, 28, TBZ)] {
        assert_eq!(
            word(one(r_type, off, 0x100), off as usize),
            w | if w == BL { 1 } else { 1 << 5 },
            "{r_type}"
        );
    }
    // ADR, literal: X = A.
    assert_eq!(word(one(274, 32, 8), 32), insert(forms::ADR, ADR_X0, 8));
    assert_eq!(word(one(273, 36, 8), 36), insert(forms::LIT19, LDR_LIT_X, 8));
    // GT: the ordinary formula; the slot holds 0 (synth's).
    assert_eq!(word(one(312, 40, 0), 40), LDR_X);
    // TLS against weak0: TPREL = A (the forms: `relax.rs`).
    let mut tt = Tables::default();
    tt.tls_sym(key(1), 0, TargetKind::Weak0);
    let ci = Case::new(&tt, &input, 0x40_0ff8);
    assert_eq!(ci.word(551, 12, key(1), 0x18), Ok(ADD_X0 | 0x18 << 10));
    let r = Reloc { r_offset: 12, key: key(1), r_type: 551, addend: 0 };
    assert_eq!(scan_err(A64::M1, &tt, &input, r), []);
    // The rules by howto.
    let h = |r| table::howto(r).unwrap();
    let a = A64::M1;
    assert_eq!(a.weak0(&h(257)), Weak0Rule::SZero);
    assert_eq!(a.weak0(&h(258)), Weak0Rule::SZero);
    assert_eq!(a.weak0(&h(277)), Weak0Rule::SZero);
    assert_eq!(a.weak0(&h(261)), Weak0Rule::SIsP);
    assert_eq!(a.weak0(&h(275)), Weak0Rule::SIsP);
    assert_eq!(a.weak0(&h(314)), Weak0Rule::SIsP);
    assert_eq!(a.weak0(&h(283)), Weak0Rule::XConst(4));
    assert_eq!(a.weak0(&h(279)), Weak0Rule::XConst(4));
}

// ---------------------------------------------------------------- §13.2

#[test]
fn s13_2_non_alloc_and_tombstones() {
    let mut t = local(0x41_0000);
    t.syms.get_mut(&key(1)).unwrap().live = false;
    let input = [0xAAu8; 8];
    let mut c = Case::new(&t, &input, 0);
    c.alloc = false;
    // arch.md §3.11, `spec/image.md` I-47: ELFCLASS64's column.
    let all = u64::MAX;
    for (name, v) in [
        (&b".debug_ranges"[..], 1u64),
        (b".debug_loc", 1),
        (b".debug_info", all),
        (b".debug_aranges", all),
        (b".debug_frame", all),
        (b".debug_addr", 0),
        (b".debug_rnglists", 0),
        (b".debug_loclists", 0),
        (b".debug_line", all),
    ] {
        c.name = name;
        let out =
            c.apply(&[Reloc { r_offset: 0, key: key(1), r_type: 257, addend: 0x40 }]).unwrap();
        assert_eq!(out, v.to_le_bytes());
        let out =
            c.apply(&[Reloc { r_offset: 0, key: key(1), r_type: 258, addend: 0x40 }]).unwrap();
        assert_eq!(&out[..4], &(v as u32).to_le_bytes(), "{}", String::from_utf8_lossy(name));
        assert_eq!(&out[4..], &[0xAA; 4]);
    }
    // `.gcc_except_table`, alloc or not: 0 (`spec/resolve.md` §6.3).
    for alloc in [false, true] {
        c.alloc = alloc;
        c.name = b".gcc_except_table";
        let out =
            c.apply(&[Reloc { r_offset: 0, key: key(1), r_type: 261, addend: 0x40 }]).unwrap();
        assert_eq!(out[..4], [0; 4]);
    }
    c.alloc = false;
    // Scan: a dead target demands nothing; from a debug name or
    // `.gcc_except_table` it is accepted, from any other section it is the
    // event `DiscardedSection` (arch.md §3.4).
    t.facts.get_mut(&key(1)).unwrap().dead = true;
    let r = Reloc { r_offset: 0, key: key(1), r_type: 257, addend: 0 };
    let na = PlaceFacts { alloc: false, writable: false, exec: None };
    let w = PlaceFacts { alloc: true, writable: true, exec: None };
    for (place, name, discarded) in [
        (na, &b".debug_info"[..], false),
        (na, b".gcc_except_table", false),
        (w, b".gcc_except_table", false),
        (na, b".comment", true),
        (w, b".data", true),
        (w, b".debug_info", true),
    ] {
        let mut sink = support::Sink::default();
        support::scan_named(M2, &t, OutputKind::Pie, place, name, &[0; 8], &[r], &mut sink)
            .unwrap();
        assert!(sink.raised.is_empty() && sink.counts.is_empty() && sink.errors.is_empty());
        let want = if discarded { vec![(0, EventClass::DiscardedSection, key(1))] } else { vec![] };
        assert_eq!(sink.events, want, "{}", String::from_utf8_lossy(name));
    }
    t.facts.get_mut(&key(1)).unwrap().dead = false;
    // A GOT class in .debug_info is refused (§4.3's non-alloc rule).
    let input = words(&[ADRP_X0]);
    let r = Reloc { r_offset: 0, key: key(1), r_type: 311, addend: 0 };
    let na = PlaceFacts { alloc: false, writable: false, exec: None };
    let s = scan_one(A64::M1, &t, OutputKind::Pie, na, &input, r).unwrap();
    assert_eq!(s.errors, [(0, ScanError::Incompatible(Incompatible::NotInNonAlloc))]);
    // ABS64 in .debug_info of a PIE: no dynamic relocation.
    let r = Reloc { r_offset: 0, key: key(1), r_type: 257, addend: 0 };
    let s = scan_one(A64::M1, &t, OutputKind::Pie, na, &[0; 8], r).unwrap();
    assert!(s.errors.is_empty() && s.counts.is_empty());
}

/// a64.md §13.2 with arch.md §4.3 (`spec/x64.md` R-13): at a non-alloc
/// place `ABS*` against a TLS symbol is the `DtpRel` exception, refused by
/// name until fill sees the symbol's type (the staging note); `PREL32`
/// against a TLS symbol is `it is thread-local`; `TLS_DTPREL64` against a
/// non-TLS symbol `it is not thread-local`; the ordinary cases unchanged.
#[test]
fn s13_2_non_alloc_tls_targets() {
    let mut t = local(0x41_0000);
    t.tls_sym(key(2), 0x42_0010, TargetKind::Local);
    let na = PlaceFacts { alloc: false, writable: false, exec: None };
    let one = |r_type, k| {
        let r = Reloc { r_offset: 0, key: k, r_type, addend: 0 };
        scan_one(M2, &t, OutputKind::Static, na, &[0; 8], r).unwrap()
    };
    let refused = ScanError::Refused(mortar_arch::Refusal::named(
        RefusalClass::Feature,
        mortar_a64::NON_ALLOC_TLS_ABS,
    ));
    for ty in [257, 258, 259] {
        let s = one(ty, key(2));
        assert_eq!(s.errors, [(0, refused)], "ABS {ty} against TLS");
        assert!(s.raised.is_empty() && s.counts.is_empty());
    }
    assert_eq!(one(261, key(2)).errors, [(0, ScanError::Incompatible(Incompatible::ThreadLocal))]);
    assert_eq!(
        one(1029, key(1)).errors,
        [(0, ScanError::Incompatible(Incompatible::NotThreadLocal))]
    );
    // Unchanged: ABS64 against a non-TLS symbol, TLS_DTPREL64 against TLS.
    assert!(one(257, key(1)).errors.is_empty());
    assert!(one(1029, key(2)).errors.is_empty());
    // At an alloc place ABS64 against TLS stays `it is thread-local`.
    let r = Reloc { r_offset: 0, key: key(2), r_type: 257, addend: 0 };
    let s = scan_one(M2, &t, OutputKind::Static, ALLOC, &[0; 8], r).unwrap();
    assert_eq!(s.errors, [(0, ScanError::Incompatible(Incompatible::ThreadLocal))]);
}

/// arch.md §4.3 (`spec/x64.md` R-6): `PC` against `weak0` in a PIE is the
/// architecture's §7, on AArch64 `S = P` with no demand and no error.
#[test]
fn s4_3_pc_weak0_in_pie_is_s_equals_p() {
    let mut t = Tables::default();
    t.sym(key(1), 0, TargetKind::Weak0);
    let r = Reloc { r_offset: 0, key: key(1), r_type: 261, addend: 5 };
    let w = PlaceFacts { alloc: true, writable: true, exec: None };
    let s = scan_one(M2, &t, OutputKind::Pie, w, &[0; 4], r).unwrap();
    assert!(s.errors.is_empty() && s.raised.is_empty() && s.counts.is_empty());
}

/// arch.md §3.4 (`spec/image.md` §4.4, I-18): one event per class per
/// record; `ComdatMismatch` instead of `Undefined`; `GnuWarning` beside
/// either, and beside an ordinary reference.
#[test]
fn s3_4_reference_events() {
    let mut t = local(0x41_0000);
    t.sym(key(2), 0, TargetKind::Undefined);
    t.sym(key(3), 0, TargetKind::Undefined);
    t.facts.get_mut(&key(3)).unwrap().comdat_only = true;
    t.facts.get_mut(&key(1)).unwrap().gnu_warning = true;
    t.sym(key(4), 0, TargetKind::Undefined);
    t.facts.get_mut(&key(4)).unwrap().gnu_warning = true;
    let input = words(&[ADRP_X0, ADRP_X0, ADRP_X0, ADRP_X0]);
    let r = |o: u64, k| Reloc { r_offset: o, key: k, r_type: 275, addend: 0 };
    let mut sink = support::Sink::default();
    support::scan_all(
        A64::M1,
        &t,
        OutputKind::Static,
        ALLOC,
        &input,
        &[r(0, key(2)), r(4, key(3)), r(8, key(1)), r(12, key(4))],
        &mut sink,
    )
    .unwrap();
    assert!(sink.errors.is_empty());
    assert_eq!(
        sink.events,
        [
            (0, EventClass::Undefined, key(2)),
            (1, EventClass::ComdatMismatch, key(3)),
            (2, EventClass::GnuWarning, key(1)),
            (3, EventClass::GnuWarning, key(4)),
            (3, EventClass::Undefined, key(4)),
        ]
    );
    assert_eq!(raised(&sink, key(1)), DemandFlags::empty());
}

#[test]
fn s5_scan_class_and_undefined() {
    let mut t = local(0x41_0000);
    t.sym(key(2), 0, TargetKind::Undefined);
    t.sym(key(3), 0x41_0000, TargetKind::Ifunc);
    let input = words(&[ADRP_X0]);
    let r = |k, r_type| Reloc { r_offset: 0, key: k, r_type, addend: 0 };
    let s = scan_one(A64::M1, &t, OutputKind::Static, ALLOC, &input, r(key(2), 275)).unwrap();
    // An undefined target is a reference event, merged by the driver.
    assert!(s.errors.is_empty());
    assert_eq!(s.undefined, [(key(2), 0)]);
    let s = scan_one(A64::M1, &t, OutputKind::Static, ALLOC, &input, r(key(3), 275)).unwrap();
    assert_eq!(s.raised, [(key(3), DemandFlags::IPLT)]);
    // An unpaired 311 also raises GOT_NORELAX (§8.5 condition 3).
    let s = scan_one(A64::M1, &t, OutputKind::Static, ALLOC, &input, r(key(1), 311)).unwrap();
    assert_eq!(raised(&s, key(1)), DemandFlags::GOT | DemandFlags::GOT_NORELAX);
    // PIE: ABS64 against local in a writable place counts one RELATIVE.
    let w = PlaceFacts { alloc: true, writable: true, exec: None };
    let s = scan_one(M2, &t, OutputKind::Pie, w, &[0; 8], r(key(1), 257)).unwrap();
    assert_eq!(s.counts, [(DynRole::Relative, 1)]);
    // A window outside the section.
    let s = scan_one(
        A64::M1,
        &t,
        OutputKind::Static,
        ALLOC,
        &[0; 6],
        Reloc { r_offset: 4, ..r(key(1), 261) },
    )
    .unwrap();
    assert_eq!(s.errors, [(0, ScanError::Outside)]);
    // Global demand of a TLS IE in a shared output.
    let mut tt = Tables::default();
    tt.tls_sym(key(1), 0x10, TargetKind::Local);
    let s = scan_one(A64::M1, &tt, OutputKind::Shared, ALLOC, &input, r(key(1), 541)).unwrap();
    assert_eq!(s.global, GlobalFlags::STATIC_TLS);
}

// ---------------------------------------------------------------- §9, §10

#[test]
fn s9_thunk_bytes_and_bti() {
    let a = A64::M1;
    let cx = mortar_arch::PatchCx {
        addr: 0x40_0ff4,
        target: 0x1234_5678,
        site: 0,
        word: 0,
        bti: false,
        target_word: None,
        target_abs: false,
    };
    let mut out = [0u8; 12];
    a.write_thunk(&cx, &mut out).unwrap();
    assert_eq!(
        out.to_vec(),
        words(&[
            insert(forms::ADRP, 0x9000_0010, 0x1234_5000 - 0x40_0000),
            0x9100_0210 | 0x678 << 10,
            0xD61F_0200
        ])
    );
    assert_eq!(a.thunk().unwrap().size, 12);
    assert!(matches!(
        a.write_thunk(&mortar_arch::PatchCx { target: 0x2_0000_0000, ..cx }, &mut out),
        Err(FieldError::Range(_))
    ));
    // BTI.
    let bti = mortar_arch::PatchCx { bti: true, ..cx };
    assert!(a.thunk_target_ok(&cx).is_ok());
    for pad in [0xD503_245F, 0xD503_249F, 0xD503_24DF, 0xD503_233F, 0xD503_237F] {
        assert!(a.thunk_target_ok(&mortar_arch::PatchCx { target_word: Some(pad), ..bti }).is_ok());
    }
    let refused =
        a.thunk_target_ok(&mortar_arch::PatchCx { target_word: Some(0xA9BF_7BFD), ..bti });
    assert_eq!(refused.unwrap_err().name, RefName::Named("BTI landing pad"));
    assert!(a.thunk_target_ok(&bti).is_err()); // not a proved instruction
    assert!(a.thunk_target_ok(&mortar_arch::PatchCx { target_abs: true, ..bti }).is_ok());
    // Branch facts.
    let b = a.branch(&table::howto(283).unwrap()).unwrap();
    assert!(b.veneer && b.reach.min == -(1 << 27) && b.reach.max == (1 << 27) - 1);
    let b = a.branch(&table::howto(280).unwrap()).unwrap();
    assert!(!b.veneer && b.reach.max == (1 << 20) - 1);
    assert!(!a.branch(&table::howto(279).unwrap()).unwrap().veneer);
    assert!(a.branch(&table::howto(314).unwrap()).is_none());
}

#[test]
fn s10_3_plt_bytes() {
    let a = A64::M1;
    let cx = SynthCx {
        addr: 0x40_1000,
        header: 0x40_1000,
        got_plt: 0x42_0010,
        got_base: 0x42_0000,
        index: 0,
    };
    let mut out = [0u8; 32];
    a.write_plt(PltVariant::Plain, PltPart::Header(0), &cx, &mut out).unwrap();
    let adrp = insert(forms::ADRP, 0x9000_0010, 0x2_0000 - 0x1000 + 0x1000 - 0x1000);
    assert_eq!(
        out.to_vec(),
        words(&[
            0xA9BF_7BF0,
            insert(forms::ADRP, 0x9000_0010, 0x2_0000 - 0x1000 + 0x1000 - 0x1000),
            0xF940_0211 | (0x10 >> 3) << 10,
            0x9100_0210 | 0x10 << 10,
            0xD61F_0220,
            0xD503_201F,
            0xD503_201F,
            0xD503_201F
        ])
    );
    assert_eq!(adrp, insert(forms::ADRP, 0x9000_0010, 0x42_0000 - 0x40_1000));
    a.write_plt(PltVariant::Bti, PltPart::Header(0), &cx, &mut out).unwrap();
    let w = |i: usize| u32::from_le_bytes(out[i * 4..i * 4 + 4].try_into().unwrap());
    assert_eq!(
        (w(0), w(1), w(5), w(6), w(7)),
        (0xD503_245F, 0xA9BF_7BF0, 0xD61F_0220, 0xD503_201F, 0xD503_201F)
    );
    let mut e = [0u8; 16];
    let ecx = SynthCx { addr: 0x40_1020, got_plt: 0x42_0018, ..cx };
    a.write_plt(PltVariant::Plain, PltPart::Entry { chunk: 0, n: 0 }, &ecx, &mut e).unwrap();
    assert_eq!(
        e.to_vec(),
        words(&[
            insert(forms::ADRP, 0x9000_0010, 0x2_0000 - 0x1000),
            0xF940_0211 | 3 << 10,
            0x9100_0210 | 0x18 << 10,
            0xD61F_0220
        ])
    );
    let mut e = [0u8; 24];
    a.write_plt(PltVariant::Bti, PltPart::Iplt(0), &ecx, &mut e).unwrap();
    assert_eq!(&e[..4], &0xD503_245Fu32.to_le_bytes());
    assert_eq!(&e[20..], &0xD503_201Fu32.to_le_bytes());
    // A .got.plt word that is not 8-aligned cannot be loaded.
    let bad = SynthCx { got_plt: 0x42_0014, ..ecx };
    assert!(matches!(
        a.write_plt(PltVariant::Plain, PltPart::Iplt(0), &bad, &mut e),
        Err(FieldError::Misaligned { .. })
    ));
    // Shapes and variants.
    assert_eq!(a.plt(PltVariant::Plain).chunks[0].entry, 16);
    assert_eq!(a.plt(PltVariant::Bti).iplt_entry, 24);
    let ocx = mortar_arch::OutputCx {
        output: OutputKind::Static,
        feature_1_and: 3,
        pac_plt: false,
        force_bti: false,
        bind_now: false,
    };
    assert_eq!(a.plt_variant(&ocx), Ok(PltVariant::Bti));
    assert_eq!(
        a.plt_variant(&mortar_arch::OutputCx { feature_1_and: 2, ..ocx }),
        Ok(PltVariant::Plain)
    );
    assert_eq!(
        a.plt_variant(&mortar_arch::OutputCx { pac_plt: true, ..ocx }).unwrap_err().class,
        RefusalClass::Option
    );
    assert!(a.write_plt(PltVariant::Pac, PltPart::Iplt(0), &ecx, &mut e).is_err());
}

#[test]
fn s10_1_static_slots_and_s12_tp_offsets() {
    let abi = A64::M1.tls();
    for (align, tcb) in [(8, 16), (16, 16), (64, 64)] {
        let tls = TlsBlock { start: 0x42_0000, size: 0x40, align };
        assert_eq!(abi.tp_rel(0x42_0010, tls), 0x10 + tcb);
        let ie =
            synth::static_slot(SlotKind::TpOff { negated: false }, Some(0x42_0010), Some(tls), abi)
                .unwrap();
        assert_eq!(ie, ([(0x10 + tcb) as u64, 0], 1));
        let gd = synth::static_slot(SlotKind::TlsGd, Some(0x42_0010), Some(tls), abi).unwrap();
        assert_eq!(gd, ([1, 0x10], 2));
    }
    assert_eq!(synth::static_slot(SlotKind::TlsLd, Some(0), None, abi).unwrap(), ([1, 0], 2));
    assert_eq!(
        synth::static_slot(SlotKind::Addr, Some(0x41_0000), None, abi).unwrap(),
        ([0x41_0000, 0], 1)
    );
    assert_eq!(
        synth::static_slot(SlotKind::TpOff { negated: false }, Some(0), None, abi),
        Err(FieldError::MissingTerm("PT_TLS"))
    );
    let got = A64::M1.got();
    assert_eq!((got.slot_words)(SlotKind::TlsDesc), 2);
    assert_eq!((got.slot_words)(SlotKind::Addr), 1);
}

#[test]
fn s13_3_facts_dyn_types_properties() {
    let a = A64::M1;
    let f = a.facts();
    assert_eq!(
        (f.e_machine, f.word, f.osabi, f.max_page, f.common_page, f.image_base),
        (183, 8, 0, 0x1_0000, 0x1000, 0x40_0000)
    );
    assert_eq!(f.page_mask, 0xFFF);
    assert_eq!(a.e_flags(&mut [5u32, 7].into_iter()), Ok(0));
    let types: Vec<Option<u32>> = DynRole::ALL.iter().map(|&r| a.dyn_type(r)).collect();
    assert_eq!(
        types,
        [
            Some(1027),
            Some(257),
            Some(1025),
            Some(1026),
            Some(1024),
            Some(1032),
            Some(1028),
            Some(1029),
            Some(1030),
            None,
            Some(1031)
        ]
    );
    // arch.md §3.13: FEATURE_1_AND, FEATURE_PAUTH; unknown types `None`.
    assert_eq!(a.property_merge(0xc000_0000), Some(PropMerge::And));
    assert_eq!(a.property_merge(0xc000_0001), Some(PropMerge::EqualOrAbsent));
    for t in [0xc000_0002, 0xc000_8002, 0xc001_0001, 0xdfff_ffff, 1, 0xb000_0000] {
        assert_eq!(a.property_merge(t), None, "{t:#x}");
    }
    assert_eq!(a.tombstone(b".debug_loc"), 1);
    assert_eq!(a.tombstone(b".debug_info"), u64::MAX);
    // layout.md L-82 (§20.5): trap fill and the no-op.
    assert_eq!(a.facts().code_fill, &[0; 4]);
    assert_eq!(a.facts().code_nop, &0xd503_201f_u32.to_le_bytes());
    // The erratum patch shape (§11.2).
    assert_eq!(
        a.patch(mortar_arch::Erratum::E843419),
        Some(&mortar_arch::ThunkShape { size: 8, align: 4 })
    );
    assert_eq!(a.eh().hdr_table, 0x3b);
    assert!(a.errata().is_empty());
    assert_eq!(M2.errata(), &[mortar_arch::Erratum::E843419]);
    // REL is refused (§1).
    let h: Howto = table::howto(257).unwrap();
    assert!(a.write_addend(&h, &mut [0; 8], 0).is_err());
}

// ---------------------------------------------------------------- §10.4

#[test]
fn s10_4_iplt_and_irelative_in_a_static_link() {
    // A non-preemptible IFUNC: every reference raises IPLT, and its address
    // everywhere is the IPLT entry's (`S`).
    let entry = 0x40_2000u64;
    let word = 0x42_0010u64;
    let resolver = 0x40_1234u64;
    let mut t = Tables::default();
    t.sym(key(1), entry, TargetKind::Ifunc);
    t.slots.insert((key(1), SlotKind::Addr), 0x42_0100);
    let bl = words(&[BL]);
    for (r_type, input, flags) in [
        (283, &bl, DemandFlags::IPLT),
        (275, &words(&[ADRP_X0]), DemandFlags::IPLT),
        (311, &words(&[ADRP_X0]), DemandFlags::GOT | DemandFlags::IPLT),
        (257, &vec![0; 8], DemandFlags::IPLT),
    ] {
        let r = Reloc { r_offset: 0, key: key(1), r_type, addend: 0 };
        let s = scan_one(A64::M1, &t, OutputKind::Static, ALLOC, input, r).unwrap();
        assert!(raised(&s, key(1)).contains(flags), "{r_type}: {:?}", s.raised);
        assert!(s.counts.is_empty(), "{r_type}: no dynamic relocation at the place");
    }
    // The call goes to the IPLT entry, the data word holds it.
    let c = Case::new(&t, &bl, 0x40_1000);
    assert_eq!(c.word(283, 0, key(1), 0).unwrap(), BL | (0x1000 >> 2));
    let c = Case::new(&t, &[0; 8], 0x41_0000);
    let out = c.apply(&[Reloc { r_offset: 0, key: key(1), r_type: 257, addend: 0 }]).unwrap();
    assert_eq!(out, entry.to_le_bytes());
    // The GOT slot of an ifunc holds S (§10.1), the entry loads its word.
    let (slot, n) =
        synth::static_slot(SlotKind::Addr, Some(entry), None, &mortar_a64::TLS).unwrap();
    assert_eq!((slot[0], n), (entry, 1));
    let cx = SynthCx { addr: entry, header: 0, got_plt: word, got_base: 0, index: 0 };
    let mut bytes = [0u8; 16];
    A64::M1.write_plt(PltVariant::Plain, PltPart::Iplt(0), &cx, &mut bytes).unwrap();
    let (expect, _) = synth::plt_entry(entry, word, false).unwrap();
    assert_eq!(bytes.to_vec(), words(&expect[..4]));
    // Its IRELATIVE record: r_offset the word, type 1032, addend the resolver.
    let rela = synth::irelative(word, resolver);
    assert_eq!(u64::from_le_bytes(rela[..8].try_into().unwrap()), word);
    assert_eq!(u64::from_le_bytes(rela[8..16].try_into().unwrap()), 1032);
    assert_eq!(u64::from_le_bytes(rela[16..].try_into().unwrap()), resolver);
    assert_eq!(A64::M1.dyn_type(DynRole::IRelative), Some(synth::IRELATIVE));
}

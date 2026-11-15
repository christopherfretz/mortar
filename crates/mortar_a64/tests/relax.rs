//! The relaxations of `spec/a64.md` §8 (R2 to R5) and the scan rules that
//! go with them (WP-023 stage B).
#![allow(clippy::many_single_char_names)]

mod support;

use mortar_a64::forms::{self, insert};
use mortar_a64::{A64, ADD_X0_X1_X0, MRS_X1_TPIDR};
use mortar_arch::{
    Arch, DemandFlags, FieldError, OutputKind, PlaceFacts, Proof, ProofRun, Reloc, RelocAt, Site,
    SlotKind, TargetKind, TlsBlock, UnitWindow,
};
use mortar_core::{FileId, SectionId};
use support::{Case, M2, Sink, Tables, key, raised, scan_all, words};

const ALLOC: PlaceFacts = PlaceFacts { alloc: true, writable: false };
const NOP: u32 = 0xD503_201F;
const TLS: TlsBlock = TlsBlock { start: 0x42_0000, size: 0x2_0000, align: 16 };
/// `S` with `T = TPREL(S) = 0x1_2360` (variant I, 16-byte TCB).
const S_TLS: u64 = 0x43_2350;
const T: u32 = 0x1_2360;
const GTPREL: u64 = 0x43_1238;

fn r(r_offset: u64, k: u32, r_type: u32) -> Reloc {
    Reloc { r_offset, key: key(k), r_type, addend: 0 }
}

fn word(out: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(out[off..off + 4].try_into().unwrap())
}

fn tls_tables(kind: TargetKind) -> Tables {
    let mut t = Tables::default();
    t.tls_sym(key(1), S_TLS, kind);
    t.slots.insert((key(1), SlotKind::TpOff { negated: false }), GTPREL);
    t
}

fn scan(arch: A64, t: &Tables, output: OutputKind, input: &[u8], relocs: &[Reloc]) -> Sink {
    let mut s = Sink::default();
    scan_all(arch, t, output, ALLOC, input, relocs, &mut s).unwrap();
    s
}

/// Applies every record in order and in reverse; both must agree
/// (arch.md §3.6 order independence).
fn apply_both(c: &Case<'_>, relocs: &[Reloc]) -> Result<Vec<u8>, FieldError> {
    let fwd = c.apply(relocs)?;
    // Reverse order with the same record indices: apply one at a time.
    let mut out = c.input.to_vec();
    for i in (0..relocs.len()).rev() {
        let one = c.apply_at(relocs, i)?;
        let r = relocs[i];
        let o = usize::try_from(r.r_offset).unwrap();
        let n = if r.r_type == 283 { 8 } else { 4 };
        out[o..o + n].copy_from_slice(&one[o..o + n]);
    }
    assert_eq!(fwd, out, "order changed the bytes");
    Ok(fwd)
}

// ---------------------------------------------------------------- §8.3 R3

/// `adrp x5, :gottprel:v; ldr x5, [x5, :gottprel_lo12:v]`.
const IE_X5: [u32; 2] = [0x9000_0005, 0xF940_00A5];

#[test]
fn s8_3_r3_ie_to_le() {
    let t = tls_tables(TargetKind::Local);
    let input = words(&IE_X5);
    let relocs = [r(0, 1, 541), r(4, 1, 542)];
    let mut c = Case::new(&t, &input, 0x40_0ff8);
    c.arch = M2;
    c.tls = Some(TLS);
    let out = apply_both(&c, &relocs).unwrap();
    // movz x5, #1, lsl #16; movk x5, #0x2360.
    assert_eq!(word(&out, 0), 0xD2A0_0000 | (T >> 16) << 5 | 5);
    assert_eq!(word(&out, 4), 0xF280_0000 | (T & 0xFFFF) << 5 | 5);
    // Scan: GOT_TPREL (kind 2: the slot stays demanded), no inhibitor.
    let s = scan(M2, &t, OutputKind::Static, &input, &relocs);
    assert_eq!(raised(&s, key(1)), DemandFlags::GOT_TPREL);
}

#[test]
fn s8_3_r3_not_taken() {
    let input = words(&IE_X5);
    let relocs = [r(0, 1, 541), r(4, 1, 542)];
    let ie = |out: &[u8]| {
        assert_eq!(word(out, 0), insert(forms::ADRP, IE_X5[0], 0x43_1000 - 0x40_0000));
        assert_eq!(word(out, 4), IE_X5[1] | (0x238 >> 3) << 10);
    };
    // The flag keeps IE for every use of the symbol.
    let mut t = tls_tables(TargetKind::Local);
    t.flags.insert(key(1), DemandFlags::GOT_TPREL | DemandFlags::TLSIE_NORELAX);
    let mut c = Case::new(&t, &input, 0x40_0ff8);
    c.arch = M2;
    c.tls = Some(TLS);
    ie(&apply_both(&c, &relocs).unwrap());
    // Before M2, IE is applied unrelaxed (§8.3 "Demand", §12.5).
    let t = tls_tables(TargetKind::Local);
    let mut c = Case::new(&t, &input, 0x40_0ff8);
    c.tls = Some(TLS);
    ie(&c.apply(&relocs).unwrap());
    // TLS-dynamic in an exec output, and any `shared` output: IE.
    for (kind, output) in [
        (TargetKind::Shlib { func: false }, OutputKind::Exec),
        (TargetKind::Local, OutputKind::Shared),
    ] {
        let t = tls_tables(kind);
        let mut c = Case::new(&t, &input, 0x40_0ff8);
        c.arch = M2;
        c.output = output;
        c.tls = Some(TLS);
        ie(&c.apply(&relocs).unwrap());
    }
}

#[test]
fn s8_3_tlsie_norelax_raised_by_rt_ne_rn() {
    let t = tls_tables(TargetKind::Local);
    // ldr x3, [x2]: Rt ≠ Rn raises the flag; on the 542 only.
    let input = words(&[0x9000_0002, 0xF940_0043]);
    let s = scan(M2, &t, OutputKind::Static, &input, &[r(0, 1, 541), r(4, 1, 542)]);
    assert_eq!(raised(&s, key(1)), DemandFlags::GOT_TPREL | DemandFlags::TLSIE_NORELAX);
    // It is input-only: raised in a shared output too.
    let s = scan(M2, &t, OutputKind::Shared, &input, &[r(4, 1, 542)]);
    assert!(raised(&s, key(1)).contains(DemandFlags::TLSIE_NORELAX));
}

#[test]
fn s8_3_r3_range_on_each_record() {
    let mut t = tls_tables(TargetKind::Local);
    t.syms.get_mut(&key(1)).unwrap().s = 0x1_4242_0000;
    let input = words(&IE_X5);
    let mut c = Case::new(&t, &input, 0x40_0ff8);
    c.arch = M2;
    c.tls = Some(TlsBlock { start: 0x42_0000, size: 0x2_0000_0000, align: 16 });
    assert!(matches!(c.word(541, 0, key(1), 0), Err(FieldError::Range(_))));
    assert!(matches!(c.word(542, 4, key(1), 0), Err(FieldError::Range(_))));
}

// ---------------------------------------------------------------- §8.5 R5

/// `adrp x3, :got:s; ldr x3, [x3, :got_lo12:s]`.
const GOT_X3: [u32; 2] = [0x9000_0003, 0xF940_0063];
const S_LOCAL: u64 = 0x41_2345;
const GOT_SLOT: u64 = 0x43_0010;

fn got_tables(kind: TargetKind) -> Tables {
    let mut t = Tables::default();
    t.sym(key(1), S_LOCAL, kind);
    t.slots.insert((key(1), SlotKind::Addr), GOT_SLOT);
    t
}

fn got_load(out: &[u8], p: u64) {
    assert_eq!(word(out, 0), insert(forms::ADRP, GOT_X3[0], (0x43_0000 - (p & !0xFFF)) as i64));
    assert_eq!(word(out, 4), GOT_X3[1] | (0x10 >> 3) << 10);
}

#[test]
fn s8_5_r5_got_load_to_address() {
    let t = got_tables(TargetKind::Local);
    let input = words(&GOT_X3);
    let relocs = [r(0, 1, 311), r(4, 1, 312)];
    let mut c = Case::new(&t, &input, 0x40_0ff8);
    c.arch = M2;
    let out = apply_both(&c, &relocs).unwrap();
    // adrp x3, s; add x3, x3, #0x345.
    assert_eq!(word(&out, 0), insert(forms::ADRP, GOT_X3[0], 0x41_2000 - 0x40_0000));
    assert_eq!(word(&out, 4), 0x9100_0000 | 0x345 << 10 | 3 << 5 | 3);
    // Scan: the pair raises GOT only (kind 2: the slot stays demanded).
    let s = scan(M2, &t, OutputKind::Static, &input, &relocs);
    assert_eq!(raised(&s, key(1)), DemandFlags::GOT);
}

#[test]
fn s8_5_r5_not_taken() {
    let input = words(&GOT_X3);
    let relocs = [r(0, 1, 311), r(4, 1, 312)];
    let p = 0x40_0ff8;
    // Condition 2: not local (abs, ifunc, weak0, shlib).
    for kind in
        [TargetKind::Abs, TargetKind::Ifunc, TargetKind::Weak0, TargetKind::Shlib { func: false }]
    {
        let t = got_tables(kind);
        let mut c = Case::new(&t, &input, p);
        c.arch = M2;
        c.output = OutputKind::Exec;
        got_load(&c.apply(&relocs).unwrap(), p);
    }
    // Condition 3: GOT_NORELAX.
    let mut t = got_tables(TargetKind::Local);
    t.flags.insert(key(1), DemandFlags::GOT | DemandFlags::GOT_NORELAX);
    let mut c = Case::new(&t, &input, p);
    c.arch = M2;
    got_load(&apply_both(&c, &relocs).unwrap(), p);
    // Condition 4: Page(S) − Page(P) outside s(33): the GOT load, no error.
    let mut t = got_tables(TargetKind::Local);
    t.syms.get_mut(&key(1)).unwrap().s = 0x2_0000_0000;
    let mut c = Case::new(&t, &input, p);
    c.arch = M2;
    got_load(&apply_both(&c, &relocs).unwrap(), p);
    // Before M2 the GOT load is written.
    let t = got_tables(TargetKind::Local);
    got_load(&Case::new(&t, &input, p).apply(&relocs).unwrap(), p);
}

#[test]
fn s8_5_got_norelax_when_a_use_is_unpaired() {
    let t = got_tables(TargetKind::Local);
    let flags = |input: &[u32], relocs: &[Reloc]| {
        raised(&scan(M2, &t, OutputKind::Static, &words(input), relocs), key(1))
    };
    let both = DemandFlags::GOT | DemandFlags::GOT_NORELAX;
    // Registers differ: ldr x4, [x3].
    assert_eq!(flags(&[GOT_X3[0], 0xF940_0064], &[r(0, 1, 311), r(4, 1, 312)]), both);
    // A hoisted ADRP: one 311, two 312 (the second is unpaired).
    let hoisted = [GOT_X3[0], GOT_X3[1], NOP, 0xF940_0063];
    assert_eq!(flags(&hoisted, &[r(0, 1, 311), r(4, 1, 312), r(12, 1, 312)]), both);
    // Not adjacent, or against another symbol.
    assert_eq!(flags(&[GOT_X3[0], NOP, GOT_X3[1]], &[r(0, 1, 311), r(8, 1, 312)]), both);
    let mut t2 = got_tables(TargetKind::Local);
    t2.sym(key(2), S_LOCAL, TargetKind::Local);
    let s = scan(M2, &t2, OutputKind::Static, &words(&GOT_X3), &[r(0, 1, 311), r(4, 2, 312)]);
    assert_eq!(raised(&s, key(1)), both);
    assert_eq!(raised(&s, key(2)), both);
    // The pair: no flag. It is input-only, so the same in any output.
    for output in [OutputKind::Static, OutputKind::Pie, OutputKind::Shared] {
        let s = scan(M2, &t, output, &words(&GOT_X3), &[r(0, 1, 311), r(4, 1, 312)]);
        assert!(!raised(&s, key(1)).contains(DemandFlags::GOT_NORELAX), "{output:?}");
    }
}

// ---------------------------------------------------------------- §8.4 R4

const GD: [u32; 4] = [0x9000_0000, 0x9100_0000, 0x9400_0000, NOP];

fn gd_relocs() -> [Reloc; 3] {
    [r(0, 1, 513), r(4, 1, 514), r(8, 9, 283)]
}

fn gd_tables(kind: TargetKind) -> Tables {
    let mut t = tls_tables(kind);
    t.sym(key(9), 0, TargetKind::Undefined);
    t.names.insert(key(9), b"__tls_get_addr".to_vec());
    t
}

#[test]
fn s8_4_r4_gd_to_le() {
    let t = gd_tables(TargetKind::Local);
    let input = words(&GD);
    let mut c = Case::new(&t, &input, 0x40_0ff8);
    c.arch = M2;
    c.tls = Some(TLS);
    let out = apply_both(&c, &gd_relocs()).unwrap();
    assert_eq!(
        out,
        words(&[
            0xD2A0_0000 | (T >> 16) << 5,
            0xF280_0000 | (T & 0xFFFF) << 5,
            MRS_X1_TPIDR,
            ADD_X0_X1_X0
        ])
    );
    // Scan: nothing demanded, the CALL26 makes no reference to the
    // undefined __tls_get_addr (arch.md §3.4).
    let s = scan(M2, &t, OutputKind::Static, &input, &gd_relocs());
    assert!(s.raised.iter().all(|(_, f)| f.is_empty()), "{:?}", s.raised);
    assert!(s.undefined.is_empty() && s.errors.is_empty());
}

#[test]
fn s8_4_r4_gd_to_ie() {
    let t = gd_tables(TargetKind::Shlib { func: false });
    let input = words(&GD);
    let p = 0x40_0ff8;
    let mut c = Case::new(&t, &input, p);
    c.arch = M2;
    c.output = OutputKind::Exec;
    c.tls = Some(TLS);
    let out = apply_both(&c, &gd_relocs()).unwrap();
    assert_eq!(
        out,
        words(&[
            insert(forms::ADRP, 0x9000_0000, 0x43_1000 - 0x40_0000),
            0xF940_0000 | (0x238 >> 3) << 10,
            MRS_X1_TPIDR,
            ADD_X0_X1_X0,
        ])
    );
    let s = scan(M2, &t, OutputKind::Exec, &input, &gd_relocs());
    assert_eq!(raised(&s, key(1)), DemandFlags::GOT_TPREL);
    assert!(raised(&s, key(9)).is_empty() && s.undefined.is_empty());
}

#[test]
fn s8_4_pattern_fails() {
    let input = words(&GD);
    let unrelaxed = |c: &Case<'_>, relocs: &[Reloc]| {
        let s = scan(M2, c.tables, c.output, c.input, relocs);
        assert!(raised(&s, key(1)).contains(DemandFlags::TLS_GD), "{:?}", s.raised);
        s
    };
    let mut t = gd_tables(TargetKind::Local);
    t.slots.insert((key(1), SlotKind::TlsGd), 0x43_2000);
    // The CALL26 calls another function.
    let mut other = gd_tables(TargetKind::Local);
    other.names.insert(key(9), b"f".to_vec());
    let mut c = Case::new(&other, &input, 0x40_0000);
    c.arch = M2;
    let s = unrelaxed(&c, &gd_relocs());
    assert_eq!(s.undefined, [(key(9), 2)], "the call is a reference");
    // o+12 is not a nop, or past the end.
    for bytes in [words(&[GD[0], GD[1], GD[2], 0xD503_203F]), words(&GD[..3])] {
        let c = Case { arch: M2, ..Case::new(&t, &bytes, 0x40_0000) };
        unrelaxed(&c, &gd_relocs());
    }
    // o+12 not proved an instruction (arch.md §3.6 rule 1).
    let relocs = gd_relocs();
    let h = M2.classify(513, Site { bytes: &input, offset: 0 }).unwrap();
    let proof = [ProofRun { start: 0, end: 12, proof: Proof::Insn, unit: 0, unit_offset: 0 }];
    let mut at = RelocAt {
        file: FileId(1),
        section: SectionId(3),
        index: 0,
        r_offset: 0,
        key: key(1),
        r_type: 513,
        addend: 0,
        howto: &h,
        site: Site { bytes: &input, offset: 0 },
        records: &&relocs[..],
        proof: &proof,
    };
    assert!(mortar_a64::relax::gd_lead(&at, &t).is_none(), "o+12 unproved");
    let all = support::all_code(16);
    at.proof = &all;
    assert_eq!(mortar_a64::relax::gd_lead(&at, &t), Some((0, 0)));
    // Another record in [o, o+16): the pattern still holds, and the record
    // overlaps the unit (refused by arch.md §3.4, never relaxed around).
    // A marker's window is its word (§6.1), so `NONE` at o+12 overlaps
    // the `CALL26`'s [o+8, o+16); one past the window does not.
    for (extra, overlap) in
        [(r(12, 1, 0), true), (r(12, 1, 258), true), (r(4, 1, 258), true), (r(16, 1, 0), false)]
    {
        let mut relocs = gd_relocs().to_vec();
        relocs.push(extra);
        let s = scan(M2, &t, OutputKind::Static, &input, &relocs);
        assert!(!raised(&s, key(1)).contains(DemandFlags::TLS_GD), "{extra:?}");
        let mut windows: Vec<UnitWindow> = (0..relocs.len())
            .map(|i| {
                let rr = relocs[i];
                let h =
                    M2.classify(rr.r_type, Site { bytes: &input, offset: rr.r_offset }).unwrap();
                let at = RelocAt {
                    file: FileId(1),
                    section: SectionId(3),
                    index: u32::try_from(i).unwrap(),
                    r_offset: rr.r_offset,
                    key: rr.key,
                    r_type: rr.r_type,
                    addend: 0,
                    howto: &h,
                    site: Site { bytes: &input, offset: rr.r_offset },
                    records: &&relocs[..],
                    proof: &all,
                };
                M2.unit_window(OutputKind::Static, &at, &t).unwrap()
            })
            .collect();
        let mut out = Vec::new();
        mortar_arch::seq::overlaps(&mut windows, &mut out);
        assert_eq!(out.iter().any(|&(rec, _)| rec == 3), overlap, "{extra:?}: {out:?}");
    }
    // A shared output: no TLS relaxation.
    let s = scan(M2, &t, OutputKind::Shared, &input, &gd_relocs());
    assert!(raised(&s, key(1)).contains(DemandFlags::TLS_GD));
    // Unrelaxed in a static link: the GTLSIDX pair is addressed.
    let mut c = Case::new(&t, &input, 0x40_0ff8);
    c.arch = M2;
    c.tls = Some(TLS);
    let out = c.apply(&[r(0, 1, 513), r(4, 1, 514)]).unwrap();
    assert_eq!(word(&out, 0), insert(forms::ADRP, GD[0], 0x43_2000 - 0x40_0000));
}

#[test]
fn s8_4_the_call26_member_is_consumed() {
    let t = gd_tables(TargetKind::Local);
    let input = words(&GD);
    let relocs = gd_relocs();
    let proof = support::all_code(input.len());
    let at = |i: usize, h: &mortar_arch::Howto| -> (UnitWindow, bool) {
        let rr = relocs[i];
        let at = RelocAt {
            file: FileId(1),
            section: SectionId(3),
            index: u32::try_from(i).unwrap(),
            r_offset: rr.r_offset,
            key: rr.key,
            r_type: rr.r_type,
            addend: 0,
            howto: h,
            site: Site { bytes: &input, offset: rr.r_offset },
            records: &&relocs[..],
            proof: &proof,
        };
        (
            M2.unit_window(OutputKind::Static, &at, &t).unwrap(),
            M2.consumed(OutputKind::Static, &at, &t),
        )
    };
    let h = |i: usize| M2.classify(relocs[i].r_type, Site { bytes: &input, offset: 0 }).unwrap();
    // One unit, led by record 0; the CALL26 owns [8, 16).
    assert_eq!(at(0, &h(0)), (UnitWindow { start: 0, end: 4, unit: 0, record: 0 }, false));
    assert_eq!(at(1, &h(1)), (UnitWindow { start: 4, end: 8, unit: 0, record: 1 }, false));
    assert_eq!(at(2, &h(2)), (UnitWindow { start: 8, end: 16, unit: 0, record: 2 }, true));
    // Not a branch site of the fixed point (arch.md §6 step 1).
    let h2 = h(2);
    let rr = relocs[2];
    let site = RelocAt {
        file: FileId(1),
        section: SectionId(3),
        index: 2,
        r_offset: rr.r_offset,
        key: rr.key,
        r_type: rr.r_type,
        addend: 0,
        howto: &h2,
        site: Site { bytes: &input, offset: 8 },
        records: &&relocs[..],
        proof: &proof,
    };
    let none = DemandFlags::empty();
    assert!(
        mortar_arch::need::branch_site(&M2, OutputKind::Static, &site, &t, TargetKind::Local, none)
            .is_none()
    );
    // In a shared output it is an ordinary call.
    assert!(!M2.consumed(OutputKind::Shared, &site, &t));
}

// ---------------------------------------------------------------- §8.2 R2

#[test]
fn s8_2_r2_tlsdesc_to_ie() {
    let t = tls_tables(TargetKind::Shlib { func: false });
    let blr_x1 = 0xD63F_0020;
    let input = words(&[0x9000_0000, 0xF940_0001, 0x9100_0000, blr_x1]);
    let relocs = [r(0, 1, 562), r(4, 1, 563), r(8, 1, 564), r(12, 1, 569)];
    let mut c = Case::new(&t, &input, 0x40_0ff8);
    c.arch = M2;
    c.output = OutputKind::Exec;
    let out = apply_both(&c, &relocs).unwrap();
    assert_eq!(
        out,
        words(&[
            insert(forms::ADRP, 0x9000_0000, 0x43_1000 - 0x40_0000),
            0xF940_0000 | (0x238 >> 3) << 10,
            NOP,
            NOP,
        ])
    );
    // Tiny: ldr x0, G − P; adr becomes nop.
    let tiny = words(&[0x5800_0001, 0x1000_0000, blr_x1]);
    let relocs = [r(0, 1, 560), r(4, 1, 561), r(8, 1, 569)];
    let mut c = Case::new(&t, &tiny, 0x43_0000);
    c.arch = M2;
    c.output = OutputKind::Pie;
    let out = apply_both(&c, &relocs).unwrap();
    assert_eq!(out, words(&[insert(forms::LDX19, 0x5800_0000, 0x1238), NOP, NOP]));
    // Scan: GOT_TPREL, never TLS_DESC; the large form stays a descriptor.
    let s = scan(M2, &t, OutputKind::Exec, &input, &[r(0, 1, 562)]);
    assert_eq!(raised(&s, key(1)), DemandFlags::GOT_TPREL);
    let movz = words(&[0xD2A0_0000]);
    let s = scan(M2, &t, OutputKind::Exec, &movz, &[r(0, 1, 565)]);
    assert_eq!(raised(&s, key(1)), DemandFlags::TLS_DESC);
}

// ---------------------------------------------------------------- §7 TLS

/// A thread-local `weak0` (a64.md §7): accepted, TLS-local, with
/// `TPREL(S + A) = DTPREL(S + A) = A` (the value lld writes; WP-023's
/// experiment). Each form, in a `static` output.
#[test]
fn s7_thread_local_undefined_weak() {
    let mut t = gd_tables(TargetKind::Weak0);
    // The template is irrelevant: a weak0 offset needs none.
    let tls = Some(TLS);
    let le =
        |t_: u32, r: u32| [0xD2A0_0000 | (t_ >> 16) << 5 | r, 0xF280_0000 | (t_ & 0xFFFF) << 5 | r];
    for tls in [tls, None] {
        // R1: TLSDESC → LE with T = A (lld: `movz x0, #0, lsl #16; movk x0, #8`).
        let desc = words(&[0x9000_0000, 0xF940_0001, 0x9100_0000, 0xD63F_0020]);
        let relocs = [r(0, 1, 562), r(4, 1, 563), r(8, 1, 564), r(12, 1, 569)]
            .map(|x| Reloc { addend: 8, ..x });
        let mut c = Case::new(&t, &desc, 0x40_0ff8);
        c.tls = tls;
        let [a, b] = le(8, 0);
        assert_eq!(apply_both(&c, &relocs).unwrap(), words(&[a, b, NOP, NOP]));
        // LE: X = A.
        let input = words(&[0x9100_0042, 0x9140_0042, 0xD2A0_0001, 0xF280_0001]);
        let relocs = [r(0, 1, 551), r(4, 1, 549), r(8, 1, 545), r(12, 1, 548)]
            .map(|x| Reloc { addend: 0x10, ..x });
        let mut c = Case::new(&t, &input, 0x40_0ff8);
        c.tls = tls;
        let out = c.apply(&relocs).unwrap();
        assert_eq!(word(&out, 0), 0x9100_0042 | 0x10 << 10);
        assert_eq!(word(&out, 4), 0x9140_0042);
        assert_eq!(word(&out, 12), 0xF280_0001 | 0x10 << 5);
        // R4: GD → LE with T = 0.
        let input = words(&GD);
        let mut c = Case::new(&t, &input, 0x40_0ff8);
        c.arch = M2;
        c.tls = tls;
        let [a, b] = le(0, 0);
        assert_eq!(
            apply_both(&c, &gd_relocs()).unwrap(),
            words(&[a, b, MRS_X1_TPIDR, ADD_X0_X1_X0])
        );
        // R3: IE → LE with T = 0.
        let input = words(&IE_X5);
        let ie = [r(0, 1, 541), r(4, 1, 542)];
        let mut c = Case::new(&t, &input, 0x40_0ff8);
        c.arch = M2;
        c.tls = tls;
        let [a, b] = le(0, 5);
        assert_eq!(apply_both(&c, &ie).unwrap(), words(&[a, b]));
    }
    // IE kept (M1, or `TLSIE_NORELAX`): the slot, whose content is 0.
    let input = words(&IE_X5);
    let ie = [r(0, 1, 541), r(4, 1, 542)];
    let mut c = Case::new(&t, &input, 0x40_0ff8);
    c.tls = tls;
    let out = c.apply(&ie).unwrap();
    assert_eq!(word(&out, 0), insert(forms::ADRP, IE_X5[0], 0x43_1000 - 0x40_0000));
    let abi = A64::M1.tls();
    let slot = |k| mortar_a64::synth::static_slot(k, None, None, abi).unwrap();
    assert_eq!(slot(SlotKind::TpOff { negated: false }), ([0, 0], 1));
    assert_eq!(slot(SlotKind::TlsGd), ([1, 0], 2));
    t.flags.insert(key(1), DemandFlags::GOT_TPREL | DemandFlags::TLSIE_NORELAX);
    let mut c = Case::new(&t, &input, 0x40_0ff8);
    c.arch = M2;
    assert_eq!(c.apply(&ie).unwrap(), out);
    // Scan: never refused; demands as for a TLS-local target, in every
    // output kind.
    for output in [OutputKind::Static, OutputKind::Exec, OutputKind::Pie] {
        let s = scan(M2, &t, output, &input, &ie);
        assert!(s.errors.is_empty(), "{output:?}: {:?}", s.errors);
        assert_eq!(raised(&s, key(1)), DemandFlags::GOT_TPREL, "{output:?}");
        let desc = words(&[0x9000_0000, 0xF940_0001, 0x9100_0000, 0xD63F_0020]);
        let relocs = [r(0, 1, 562), r(4, 1, 563), r(8, 1, 564), r(12, 1, 569)];
        let s = scan(M2, &t, output, &desc, &relocs);
        assert!(s.errors.is_empty() && raised(&s, key(1)).is_empty(), "{output:?}");
        let le = words(&[0x9100_0042]);
        let s = scan(M2, &t, output, &le, &[r(0, 1, 551)]);
        assert!(s.errors.is_empty() && raised(&s, key(1)).is_empty(), "{output:?}");
    }
    // LE in a shared output is refused as for any target (§5 `TE`).
    let le = words(&[0x9100_0042]);
    let s = scan(M2, &t, OutputKind::Shared, &le, &[r(0, 1, 551)]);
    assert_eq!(s.errors.len(), 1);
}

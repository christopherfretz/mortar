//! Mapping symbols (a64.md §3.4) and erratum 843419 (§11).

use mortar_a64::erratum::{self, adr_fix, candidates};
use mortar_a64::mapping::{MapSym, Mapping, mapping_kind, proof_runs};
use mortar_a64::{A64, forms};
use mortar_arch::{Arch, Code, Erratum, ErratumSite, PatchCx, Proof, ProofRun, Unproved};

fn words(ws: &[u32]) -> Vec<u8> {
    ws.iter().flat_map(|w| w.to_le_bytes()).collect()
}

const fn insn(start: u64, end: u64) -> ProofRun {
    ProofRun { start, end, proof: Proof::Insn, unit: 0, unit_offset: start }
}

fn sites(code: &Code<'_>) -> Result<Vec<ErratumSite>, Unproved> {
    let mut out = Vec::new();
    A64::new(mortar_arch::Status::M1, true).erratum_sites(Erratum::A53_843419, code, &mut out)?;
    Ok(out)
}

// ---------------------------------------------------------------- §3.4

#[test]
fn s3_4_mapping_symbol_names() {
    assert_eq!(mapping_kind(b"$x", 0), Some(Mapping::Code));
    assert_eq!(mapping_kind(b"$x.42", 0), Some(Mapping::Code));
    assert_eq!(mapping_kind(b"$d", 0), Some(Mapping::Data));
    assert_eq!(mapping_kind(b"$d.foo", 0), Some(Mapping::Data));
    assert_eq!(mapping_kind(b"$xy", 0), None);
    assert_eq!(mapping_kind(b"$a", 0), None);
    assert_eq!(mapping_kind(b"$x", 0x10), None); // global
    assert_eq!(mapping_kind(b"$x", 0x02), None); // STT_FUNC
    assert_eq!(mapping_kind(b"", 0), None);
}

#[test]
fn s3_4_ranges() {
    let m = |index, value, kind| MapSym { index, value, kind };
    let mut out = Vec::new();
    // Out of order; two at 8 (the later index, $d, wins); one past the end.
    let mut syms = [
        m(5, 8, Mapping::Data),
        m(2, 0, Mapping::Code),
        m(3, 8, Mapping::Code),
        m(7, 16, Mapping::Code),
        m(9, 64, Mapping::Data),
    ];
    proof_runs(&mut syms, 24, true, 0x100, 4, &mut out);
    let run = |start, end, proof, unit_offset| ProofRun { start, end, proof, unit: 4, unit_offset };
    assert_eq!(
        out,
        [
            run(0x100, 0x108, Proof::Insn, 0),
            run(0x108, 0x110, Proof::Data, 8),
            run(0x110, 0x118, Proof::Insn, 16)
        ]
    );
    // Bytes before the first symbol.
    out.clear();
    proof_runs(&mut [m(1, 8, Mapping::Code)], 16, true, 0, 1, &mut out);
    assert_eq!(out[0].proof, Proof::Unproved);
    assert_eq!((out[0].end, out[1].proof), (8, Proof::Insn));
    out.clear();
    proof_runs(&mut [], 16, false, 0, 1, &mut out);
    assert_eq!(out, [ProofRun { start: 0, end: 16, proof: Proof::Data, unit: 1, unit_offset: 0 }]);
    out.clear();
    proof_runs(&mut [], 0, true, 0, 1, &mut out);
    assert!(out.is_empty());
}

// ---------------------------------------------------------------- §11.1

/// `adrp x0`, `ldr x1, [x2]`, ..., `ldr x3, [x0, #8]`.
const W1: u32 = 0x9000_0000;
const W2: u32 = 0xF940_0041;
const W_OTHER: u32 = 0x8B02_0020; // add x0, x1, x2
const W_FINAL: u32 = 0xF940_0403;

/// A code view whose word at `0xFF8` (address `0x400ff8`) starts `ws`.
fn at_ff8(ws: &[u32], q: u64) -> Vec<u8> {
    let mut b = vec![0u8; 0x2000];
    let q = usize::try_from(q).unwrap();
    b[q..q + 4 * ws.len()].copy_from_slice(&words(ws));
    b
}

#[test]
fn s11_1_three_and_four_word_forms() {
    for q in [0xFF8, 0xFFC] {
        let bytes = at_ff8(&[W1, W2, W_FINAL], q);
        let proof = [insn(0, 0x2000)];
        let code = Code { bytes: &bytes, addr: 0x40_0000, proof: &proof };
        assert_eq!(sites(&code).unwrap(), [ErratumSite { q, f: q + 8 }], "{q:#x}");
        let bytes = at_ff8(&[W1, W2, W_OTHER, W_FINAL], q);
        let code = Code { bytes: &bytes, ..code };
        assert_eq!(sites(&code).unwrap(), [ErratumSite { q, f: q + 12 }]);
    }
    // At any other page offset: no site.
    for q in [0xFF0, 0xFF4, 0x1000, 0x7F8] {
        let bytes = at_ff8(&[W1, W2, W_FINAL], q);
        let proof = [insn(0, 0x2000)];
        assert!(sites(&Code { bytes: &bytes, addr: 0x40_0000, proof: &proof }).unwrap().is_empty());
    }
    // A view whose address is not page aligned.
    let bytes = at_ff8(&[W1, W2, W_FINAL], 0xFF8 - 0x10);
    let proof = [insn(0, 0x2000)];
    assert_eq!(sites(&Code { bytes: &bytes, addr: 0x40_0010, proof: &proof }).unwrap()[0].q, 0xFE8);
}

#[test]
fn s11_1_each_condition() {
    // Condition 1: not ADRP (ADR).
    assert_eq!(candidates(0x1000_0000, W2, Some(W_FINAL), None), [false, false]);
    // Condition 2: not a load/store.
    assert_eq!(candidates(W1, W_OTHER, Some(W_FINAL), None), [false, false]);
    // Condition 4: the base is not Xn, or not unsigned-immediate.
    assert_eq!(candidates(W1, W2, Some(0xF940_0423), None), [false, false]);
    assert_eq!(candidates(W1, W2, Some(0xF840_8403), None), [false, false]);
    // Both forms at once.
    assert_eq!(candidates(W1, W2, Some(W_FINAL), Some(W_FINAL)), [true, true]);
    // Stores and SIMD count.
    assert!(candidates(W1, W2, Some(0xF900_0403), None)[0]);
    assert!(candidates(W1, W2, Some(0x3D40_0400), None)[0]);
    assert!(erratum::is_load_store(0x3D40_0400) && erratum::is_adrp(0x9000_001F));
}

#[test]
fn s11_1_condition_5_proof() {
    let bytes = at_ff8(&[W1, W2, W_FINAL], 0xFF8);
    // A data word in the window: no site, even beside unproved bytes.
    let proof = [
        ProofRun { start: 0, end: 0xFFC, proof: Proof::Unproved, unit: 1, unit_offset: 0 },
        ProofRun { start: 0xFFC, end: 0x1000, proof: Proof::Data, unit: 2, unit_offset: 0 },
        insn(0x1000, 0x2000),
    ];
    assert!(sites(&Code { bytes: &bytes, addr: 0x40_0000, proof: &proof }).unwrap().is_empty());
    // Padding (no run) is data.
    let proof = [insn(0, 0xFFC), insn(0x1000, 0x2000)];
    assert!(sites(&Code { bytes: &bytes, addr: 0x40_0000, proof: &proof }).unwrap().is_empty());
    // Unproved and no data: refused, naming the unit and Q's offset in it.
    let proof = [
        ProofRun { start: 0x800, end: 0x1000, proof: Proof::Unproved, unit: 7, unit_offset: 0 },
        insn(0x1000, 0x2000),
    ];
    let err = sites(&Code { bytes: &bytes, addr: 0x40_0000, proof: &proof }).unwrap_err();
    assert_eq!(err, Unproved { unit: 7, q: 0xFF8, q_unit: 7, q_unit_offset: 0x7F8 });
    // Unproved bytes that are not a candidate are not refused.
    let bytes = at_ff8(&[W1, W_OTHER, W_FINAL], 0xFF8);
    assert!(sites(&Code { bytes: &bytes, addr: 0x40_0000, proof: &proof }).unwrap().is_empty());
}

/// Data words are never sites: candidate-shaped words at every word
/// offset of a `$d` range make none.
#[test]
fn s11_1_data_is_never_a_site() {
    for start in (0..0x40).step_by(4) {
        let mut bytes = vec![0u8; 0x3000];
        for q in (0..0x2FF0).step_by(4) {
            let w = [W1, W2, W_FINAL, W_FINAL][(q / 4) % 4];
            bytes[q..q + 4].copy_from_slice(&w.to_le_bytes());
        }
        let proof =
            [ProofRun { start: 0, end: 0x3000, proof: Proof::Data, unit: 0, unit_offset: 0 }];
        let code = Code { bytes: &bytes, addr: 0x40_0000 + start, proof: &proof };
        assert!(sites(&code).unwrap().is_empty());
        let proof = [insn(0, 0x3000)];
        let code = Code { bytes: &bytes, addr: 0x40_0000 + start, proof: &proof };
        // Proved: every FF8/FFC with the shape is one.
        for s in sites(&code).unwrap() {
            assert!(matches!((0x40_0000 + start + s.q) & 0xFFF, 0xFF8 | 0xFFC));
        }
    }
}

/// A site is found exactly when conditions 1 to 4 hold, against a
/// brute-force restatement over random words.
#[test]
fn s11_1_against_brute_force() {
    let mut seed = 1u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let pool =
        [W1, 0x9000_0001, W2, 0xB940_0002, W_OTHER, W_FINAL, 0xF940_0423, 0x3900_0003, 0x1000_0000];
    for _ in 0..2000 {
        let ws: Vec<u32> = (0..6).map(|_| pool[(next() % pool.len() as u64) as usize]).collect();
        let bytes = at_ff8(&ws, 0xFF0);
        let proof = [insn(0, 0x2000)];
        let got = sites(&Code { bytes: &bytes, addr: 0x40_0000, proof: &proof }).unwrap();
        let mut want = Vec::new();
        for (i, q) in [(2usize, 0xFF8u64), (3, 0xFFC)] {
            let (w1, w2) = (ws[i], ws[i + 1]);
            let ok = |w: u32| w & 0x3B00_0000 == 0x3900_0000 && (w >> 5) & 31 == w1 & 31;
            if w1 & 0x9F00_0000 == 0x9000_0000 && w2 & 0x0A00_0000 == 0x0800_0000 {
                let w3 = ws.get(i + 2).copied();
                let w4 = ws.get(i + 3).copied().or_else(|| (i + 3 == 6).then_some(0));
                if w3.is_some_and(ok) {
                    want.push(ErratumSite { q, f: q + 8 });
                }
                if w4.is_some_and(ok) {
                    want.push(ErratumSite { q, f: q + 12 });
                }
            }
        }
        assert_eq!(got, want, "{ws:x?}");
    }
}

// ---------------------------------------------------------------- §11.2

#[test]
fn s11_2_adr_fix_and_patch() {
    // adrp x5 at Q = 0x400ff8 to page 0x480000: within s(21) of Q.
    let w1 = forms::insert(forms::ADRP, 0x9000_0005, 0x8_0000);
    let q = 0x40_0ff8;
    let adr = adr_fix(q, w1).unwrap();
    assert_eq!(adr, forms::insert(forms::ADR, 0x1000_0005, 0x48_0000 - q as i64));
    assert_eq!(forms::extract(forms::ADR, adr) as u64 + q, 0x48_0000);
    assert_eq!(mortar_a64::erratum_adr(q, w1), Some(adr));
    // Out of reach: the patch.
    let far = forms::insert(forms::ADRP, 0x9000_0005, 0x10_1000);
    assert_eq!(adr_fix(q, far), None);
    let a = A64::new(mortar_arch::Status::M1, true);
    let cx = PatchCx {
        addr: 0x50_0000,
        target: 0x40_1004,
        site: q,
        word: W_FINAL,
        bti: false,
        target_word: None,
        target_abs: false,
    };
    assert!(!a.erratum_in_place(Erratum::A53_843419, &PatchCx { word: far, ..cx }));
    assert!(a.erratum_in_place(Erratum::A53_843419, &PatchCx { word: w1, ..cx }));
    let mut out = [0u8; 8];
    a.write_patch(Erratum::A53_843419, &cx, &mut out).unwrap();
    // W4', then b F + 4 = 0x401008 from 0x500004.
    let back = 0x40_1008 - 0x50_0004_i64;
    assert_eq!(out.to_vec(), words(&[W_FINAL, 0x1400_0000 | ((back >> 2) as u32 & 0x3FF_FFFF)]));
    assert_eq!(
        mortar_a64::erratum_branch(0x40_1004, 0x50_0000).unwrap(),
        0x1400_0000 | ((0xF_EFFC >> 2) as u32)
    );
    assert!(mortar_a64::erratum_branch(0, 0x800_0000).is_err());
    assert!(mortar_a64::erratum_branch(0, 2).is_err());
    assert!(a.write_patch(Erratum::A53_843419, &cx, &mut [0u8; 4]).is_err());
}

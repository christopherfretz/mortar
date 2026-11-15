//! Erratum 843419 (a64.md §11): the predicate over one output code
//! section, the ADR fix and the patch.

use mortar_arch::{Code, ErratumSite, FieldError, Proof, Unproved};

use crate::forms::{self, ADR, ADRP, B26};
use crate::synth::checked;

/// Condition 1, the word: ADRP (`W1 & 0x9F000000 = 0x90000000`).
#[must_use]
pub const fn is_adrp(w: u32) -> bool {
    w & 0x9F00_0000 == 0x9000_0000
}

/// Condition 2: `W2` is in the "Loads and Stores" encoding group.
#[must_use]
pub const fn is_load_store(w: u32) -> bool {
    w & 0x0A00_0000 == 0x0800_0000
}

/// Condition 4: a load/store register (unsigned immediate) with base
/// `Xn`.
#[must_use]
pub const fn is_final(w: u32, n: u32) -> bool {
    w & 0x3B00_0000 == 0x3900_0000 && forms::rn(w) == n
}

/// Conditions 1 to 4 at `Q`, on the words alone: the offsets of `F` that
/// make a candidate (3-word form first, then 4-word).
#[must_use]
pub fn candidates(w1: u32, w2: u32, w3: Option<u32>, w4: Option<u32>) -> [bool; 2] {
    if !is_adrp(w1) || !is_load_store(w2) {
        return [false, false];
    }
    let n = forms::rd(w1);
    [w3.is_some_and(|w| is_final(w, n)), w4.is_some_and(|w| is_final(w, n))]
}

/// The sites of one output code section, in address order (§11.1), or
/// the first candidate that has a word with no proof either way.
///
/// Only the two word offsets per 4 KiB page that condition 1 allows are
/// visited. Condition 5: every word `Insn` makes a site; any word `Data`
/// (a `$d` range or padding) makes none, even if another is unproved;
/// otherwise an `Unproved` word refuses.
pub fn sites(code: &Code<'_>, out: &mut Vec<ErratumSite>) -> Result<(), Unproved> {
    let len = code.bytes.len() as u64;
    // The first offset whose address is ≡ 0xFF8 (mod 4096).
    let mut page = 0xFF8u64.wrapping_sub(code.addr) & 0xFFF;
    while page < len {
        for q in [page, page + 4] {
            check_at(code, q, out)?;
        }
        page += 0x1000;
    }
    Ok(())
}

fn check_at(code: &Code<'_>, q: u64, out: &mut Vec<ErratumSite>) -> Result<(), Unproved> {
    let Some(w1) = code.word(q) else { return Ok(()) };
    let Some(w2) = code.word(q + 4) else { return Ok(()) };
    let found = candidates(w1, w2, code.word(q + 8), code.word(q + 12));
    for (k, hit) in found.into_iter().enumerate() {
        if !hit {
            continue;
        }
        let f = q + 8 + 4 * k as u64;
        let mut unproved = None;
        let mut data = false;
        let mut at = q;
        while at <= f {
            match code.word_proof(at) {
                (Proof::Insn, _) => {}
                (Proof::Data, _) => data = true,
                (Proof::Unproved, run) => unproved = unproved.or(run),
            }
            at += 4;
        }
        if data {
            continue;
        }
        if let Some(run) = unproved {
            let (q_unit, q_unit_offset) =
                code.run_at(q).map_or((run.unit, 0), |r| (r.unit, r.unit_offset + (q - r.start)));
            return Err(Unproved { unit: run.unit, q, q_unit, q_unit_offset });
        }
        out.push(ErratumSite { q, f });
    }
    Ok(())
}

/// `Page(V)` of a pre-fix ADRP word at `q`: `Page(q) + (immhi:immlo <<
/// 12)` (§11.2).
#[must_use]
pub const fn adrp_page(q: u64, w1: u32) -> u64 {
    (q & !0xFFF).wrapping_add(forms::extract(ADRP, w1) as u64)
}

/// The ADR fix of a site (§11.2): `adr xn, Page(V)` if `Page(V) − Q`
/// satisfies `s(21)`, else `None` (the site needs a patch).
#[must_use]
pub const fn adr_fix(q: u64, w1: u32) -> Option<u32> {
    let d = adrp_page(q, w1).wrapping_sub(q) as i64;
    if d < -(1 << 20) || d >= 1 << 20 {
        return None;
    }
    Some(forms::insert(ADR, 0x1000_0000 | forms::rd(w1), d))
}

/// `b T` at `P`, checked `s(28)+a(4)`.
pub fn branch(p: u64, t: u64) -> Result<u32, FieldError> {
    let x = t.wrapping_sub(p) as i64;
    checked(mortar_arch::Check::Signed(28), 4, x)?;
    Ok(forms::insert(B26, 0x1400_0000, x))
}

/// The patch (§11.2): `W4'`, then `b F + 4`, at `patch`.
pub fn patch(patch: u64, f: u64, w4: u32) -> Result<[u32; 2], FieldError> {
    Ok([w4, branch(patch.wrapping_add(4), f.wrapping_add(4))?])
}

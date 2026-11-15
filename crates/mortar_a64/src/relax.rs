//! The input-only predicates of the relaxations (a64.md §8): the two
//! sequence patterns (R4, R5) and the register condition of R3.
//!
//! Each is a pure function of the record, its neighbours and the section
//! bytes, so every member of a sequence evaluates it alike, in scan and in
//! fill, in any order (arch.md §3.5).

use mortar_arch::seq::look_back;
use mortar_arch::{Records, RelocAt, SymNames, Window};

use crate::synth::NOP;

pub const ADR_GOT_PAGE: u32 = 311;
pub const LD64_GOT_LO12_NC: u32 = 312;
pub const TLSGD_ADR_PAGE21: u32 = 513;
pub const TLSGD_ADD_LO12_NC: u32 = 514;
pub const CALL26: u32 = 283;
pub const ADR_GOTTPREL_PAGE21: u32 = 541;
pub const LD64_GOTTPREL_LO12_NC: u32 = 542;

/// R4's `CALL26` member (§8.4) owns `[o+8, o+16)`: its field at `o+8` and
/// the `nop` after it, 4 bytes past the field.
pub const R4_CALL_AFTER: u64 = 4;

/// The member windows of the recognized forms that differ from their
/// type's `Howto` window (arch.md §3.5 `unit_window`), around each
/// member's field: R4's `CALL26`. Every other member of R1 to R5 writes
/// only its own field word. `Facts.max_reach` is derived from this table
/// and the `Howto` table (arch.md §3.1).
pub const MEMBER_WINDOWS: [Window; 1] = [Window { before: 0, after: R4_CALL_AFTER as u8 }];

/// The lead types that `RelocAt.leads` gates (arch.md §3.5): R4's.
pub const SEQ_LEADS: [u32; 1] = [TLSGD_ADR_PAGE21];

/// arch.md §3.5 `sequence_extent`: 3 at an R4 lead whose next two records
/// are the 514 and the `CALL26`, 2 at an R5 lead followed by its 312, 1
/// otherwise. It reads only the record types: a pattern that may match
/// at `i` never consumes more.
#[must_use]
pub fn sequence_extent(records: &dyn Records, i: u32) -> u32 {
    let ty = |k: u32| i.checked_add(k).and_then(|j| records.get(j)).map(|r| r.r_type);
    match ty(0) {
        Some(TLSGD_ADR_PAGE21) if ty(1) == Some(TLSGD_ADD_LO12_NC) && ty(2) == Some(CALL26) => 3,
        Some(ADR_GOT_PAGE) if ty(1) == Some(LD64_GOT_LO12_NC) => 2,
        _ => 1,
    }
}

const fn rd(w: u32) -> u32 {
    w & 0x1F
}

const fn rn(w: u32) -> u32 {
    (w >> 5) & 0x1F
}

/// ADRP (any register).
const fn is_adrp(w: u32) -> bool {
    w & 0x9F00_0000 == 0x9000_0000
}

/// `LDR Xt, [Xn, #imm]` (64-bit, unsigned offset).
const fn is_ldr_x_imm(w: u32) -> bool {
    w & 0xFFC0_0000 == 0xF940_0000
}

/// The word at section offset `off` of the input.
fn word(at: &RelocAt<'_>, off: u64) -> Option<u32> {
    let i = usize::try_from(off).ok()?;
    let b = at.site.bytes.get(i..i.checked_add(4)?)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// §8.3: a 542 whose `Rt ≠ Rn` raises `TLSIE_NORELAX`. `None` when the
/// word is not there (scan's checks refuse it first).
#[must_use]
pub fn ie_registers_differ(at: &RelocAt<'_>) -> Option<bool> {
    let w = word(at, at.r_offset)?;
    Some(rd(w) != rn(w))
}

/// R5 condition 1 (§8.5): the pair `at` belongs to, as `(o, d)` (the
/// ADRP's offset and register), when `at` is a 311 whose next record
/// completes it or a 312 whose previous record leads it.
#[must_use]
pub fn got_pair(at: &RelocAt<'_>) -> Option<(u64, u32)> {
    let (lead, follower) = match at.r_type {
        ADR_GOT_PAGE => {
            let next = at.records.get(at.index.checked_add(1)?)?;
            (at.records.get(at.index)?, next)
        }
        LD64_GOT_LO12_NC => (look_back(at.records, at.index, 1)?.1, at.records.get(at.index)?),
        _ => return None,
    };
    let o = lead.r_offset;
    let ok = lead.r_type == ADR_GOT_PAGE
        && follower.r_type == LD64_GOT_LO12_NC
        && Some(follower.r_offset) == o.checked_add(4)
        && follower.key == lead.key
        && lead.addend == 0
        && follower.addend == 0;
    if !ok {
        return None;
    }
    let (w0, w1) = (word(at, o)?, word(at, o + 4)?);
    let d = rd(w0);
    (is_adrp(w0) && is_ldr_x_imm(w1) && rn(w1) == d && rd(w1) == d).then_some((o, d))
}

/// R4's pattern (§8.4): the lead's index and offset `o` of the GD sequence
/// `at` belongs to, if it holds. The 514 looks one record back, the
/// `CALL26` two.
///
/// The pattern does not look at other records: any other record whose
/// window meets `[o, o + 16)` overlaps the matched unit (its windows are
/// `unit_window`'s), and the overlap check of arch.md §3.4 refuses it.
#[must_use]
pub fn gd_lead(at: &RelocAt<'_>, names: &dyn SymNames) -> Option<(u32, u64)> {
    // No 513 in the section: no R4 (`RelocAt.leads`, arch.md §3.5).
    if !at.leads {
        return None;
    }
    let back = match at.r_type {
        TLSGD_ADR_PAGE21 => 0,
        TLSGD_ADD_LO12_NC => 1,
        CALL26 => 2,
        _ => return None,
    };
    // The words first: they are the cheap test, and rule out almost every
    // ordinary `CALL26`.
    let o = at.r_offset.checked_sub(4 * u64::from(back))?;
    o.checked_add(16)?;
    // The `nop` after the call first: ordinary calls fail there.
    if word(at, o + 12)? != NOP {
        return None;
    }
    let words = [word(at, o)?, word(at, o + 4)?, word(at, o + 8)?];
    let ok = words[0] & 0x9F00_001F == 0x9000_0000
        && words[1] & 0xFFC0_03FF == 0x9100_0000
        && words[2] & 0xFC00_0000 == 0x9400_0000;
    if !ok {
        return None;
    }
    let i = at.index.checked_sub(back)?;
    let r = at.records;
    let (lead, add, call) = (r.get(i)?, r.get(i + 1)?, r.get(i + 2)?);
    let ok = lead.r_type == TLSGD_ADR_PAGE21
        && lead.r_offset == o
        && add.r_type == TLSGD_ADD_LO12_NC
        && add.r_offset == o + 4
        && add.key == lead.key
        && call.r_type == CALL26
        && call.r_offset == o + 8
        && at.is_insn(o + 12)
        && names.name(call.key) == crate::TLS.get_addr;
    ok.then_some((i, o))
}

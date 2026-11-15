//! The two-record sequences of x64.md §8.3 and §8.4 (arch.md §3.5): a
//! `TLSGD` or `TLSLD` lead and its follower, the call's relocation at
//! record `+1`.
//!
//! Each predicate is a pure function of the records, the section bytes
//! and the follower's symbol name, so the lead and the follower decide
//! alike, in scan and in fill, in any order.

use mortar_arch::{Records, Reloc, RelocAt, Site, SymNames, Window, max_window};

use crate::forms::{self, Form, GOTPCREL, GOTPCRELX, PC32, PLT32, PLTOFF64, TLSGD, TLSLD};

/// The lead types that `RelocAt.leads` gates (arch.md §3.5; §8.3, §8.4).
pub const SEQ_LEADS: [u32; 2] = [TLSGD, TLSLD];

/// The largest window of every recognized form (§8.6), folded into
/// `Facts.max_reach` with the `Howto` table's.
pub const MEMBER_WINDOWS: Window = {
    let mut w = Window { before: 0, after: 0 };
    let mut k = 0;
    while k < forms::ALL_WINDOWS.len() {
        w = max_window(w, forms::ALL_WINDOWS[k].window());
        k += 1;
    }
    w
};

/// X-30: 2 at a lead whose next record is a follower the forms allow
/// (its type, at the offset that type has after this lead), 1 otherwise
/// (and past the end). It reads only the records: a sequence that may
/// match at `i` never consumes more.
#[must_use]
pub fn sequence_extent(records: &dyn Records, i: u32) -> u32 {
    let Some(lead) = records.get(i) else { return 1 };
    let Some(next) = i.checked_add(1).and_then(|j| records.get(j)) else { return 1 };
    let delta = next.r_offset.wrapping_sub(lead.r_offset);
    let ok = match (lead.r_type, next.r_type) {
        (TLSGD, PLT32 | PC32 | GOTPCRELX | GOTPCREL) => delta == 8,
        (TLSGD | TLSLD, PLTOFF64) | (TLSLD, GOTPCRELX | GOTPCREL) => delta == 6,
        (TLSLD, PLT32 | PC32) => delta == 5,
        _ => false,
    };
    if ok { 2 } else { 1 }
}

/// The form of the sequence `lead`, `follower` over the section `bytes`,
/// if it holds (X-25, X-27): the lead's bytes match a form, its addend is
/// −4, and the follower has a type the form allows, at the form's offset,
/// against `__tls_get_addr`.
fn matched(bytes: &[u8], lead: Reloc, follower: Reloc, names: &dyn SymNames) -> Option<Form> {
    if !matches!(lead.r_type, TLSGD | TLSLD) || lead.addend != -4 {
        return None;
    }
    let form = forms::form(lead.r_type, Site { bytes, offset: lead.r_offset })?;
    let (types, delta) = form.follower()?;
    let ok = types.contains(&follower.r_type)
        && Some(follower.r_offset) == lead.r_offset.checked_add(delta)
        && names.name(follower.key) == crate::TLS.get_addr;
    ok.then_some(form)
}

/// The form of the sequence `at` leads, when `at` is a `TLSGD` or `TLSLD`
/// whose sequence holds.
#[must_use]
pub fn lead_form(at: &RelocAt<'_>, names: &dyn SymNames) -> Option<Form> {
    if !at.leads || !matches!(at.r_type, TLSGD | TLSLD) {
        return None;
    }
    let lead =
        Reloc { r_offset: at.r_offset, key: at.key, r_type: at.r_type, addend: at.rela_addend() };
    let follower = at.records.get(at.index.checked_add(1)?)?;
    matched(at.site.bytes, lead, follower, names)
}

/// The lead's index and form, when `at` is the follower of a sequence
/// that holds: the record it consumes (X-25).
#[must_use]
pub fn follower_of(at: &RelocAt<'_>, names: &dyn SymNames) -> Option<(u32, Form)> {
    if !at.leads || !matches!(at.r_type, PLT32 | PC32 | GOTPCRELX | GOTPCREL | PLTOFF64) {
        return None;
    }
    let i = at.index.checked_sub(1)?;
    let lead = at.records.get(i)?;
    if !matches!(lead.r_type, TLSGD | TLSLD) {
        return None;
    }
    let me =
        Reloc { r_offset: at.r_offset, key: at.key, r_type: at.r_type, addend: at.rela_addend() };
    matched(at.site.bytes, lead, me, names).map(|f| (i, f))
}

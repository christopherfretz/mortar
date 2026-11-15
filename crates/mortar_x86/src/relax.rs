//! The two-record sequences of x86.md §9 (arch.md §3.5): a `TLS_GD` or
//! `TLS_LDM` lead and its partner, the call's relocation at record `+1`
//! (X86-31, X86-41).
//!
//! Each predicate is a pure function of the records, the input bytes and
//! the partner's symbol name, so the lead and the partner decide alike,
//! in scan and in fill, in any order. Both addends are read from the
//! input bytes (X86-5): the partner's lies inside the lead's window,
//! which the lead overwrites.

use mortar_arch::{Records, RelocAt, Site, SymNames, Window, max_window};

use crate::forms::{self, Form, GOT32, GOT32X, PC32, PLT32, TLS_GD, TLS_LDM};

/// The lead types that `RelocAt.leads` gates (arch.md §3.5; X86-41).
pub const SEQ_LEADS: [u32; 2] = [TLS_GD, TLS_LDM];

/// The largest window of every recognized form and every read window
/// (X86-40), folded into `Facts.max_reach` with the `Howto` table's.
pub const MEMBER_WINDOWS: Window = {
    let mut w = forms::READ_WINDOWS;
    let mut k = 0;
    while k < forms::ALL_WINDOWS.len() {
        w = max_window(w, forms::ALL_WINDOWS[k].window());
        k += 1;
    }
    w
};

/// X86-41: 2 when record `i` is a `TLS_GD` or `TLS_LDM` and record `i +
/// 1` exists, otherwise 1. It reads only the records.
#[must_use]
pub fn sequence_extent(records: &dyn Records, i: u32) -> u32 {
    let lead = records.get(i).is_some_and(|r| matches!(r.r_type, TLS_GD | TLS_LDM));
    let next = i.checked_add(1).and_then(|j| records.get(j)).is_some();
    if lead && next { 2 } else { 1 }
}

/// The form of the sequence whose lead is at `lead_off` (type
/// `lead_type`) and whose partner is `(type, r_offset, key)`, over the
/// input `bytes`, if it holds (X86-31, X86-34, X86-35): the lead's bytes
/// match a form, its `A` is 0, and the partner has a type the form
/// allows, at the form's offset, with the form's `A`, against
/// `___tls_get_addr`.
fn matched(
    bytes: &[u8],
    lead_type: u32,
    lead_off: u64,
    partner: (u32, u64, mortar_core::SymKey),
    names: &dyn SymNames,
) -> Option<Form> {
    let form = forms::form(lead_type, Site { bytes, offset: lead_off })?;
    let (types, delta, a) = form.follower()?;
    let (p_type, p_off, p_key) = partner;
    let ok = forms::addend32(bytes, lead_off) == Some(0)
        && types.contains(&p_type)
        && Some(p_off) == lead_off.checked_add(delta)
        && forms::addend32(bytes, p_off) == Some(a)
        && names.name(p_key) == crate::TLS.get_addr;
    ok.then_some(form)
}

/// The form of the sequence `at` leads, when `at` is a `TLS_GD` or
/// `TLS_LDM` whose sequence holds.
#[must_use]
pub fn lead_form(at: &RelocAt<'_>, names: &dyn SymNames) -> Option<Form> {
    if !at.leads || !matches!(at.r_type, TLS_GD | TLS_LDM) {
        return None;
    }
    let next = at.records.get(at.index.checked_add(1)?)?;
    matched(at.site.bytes, at.r_type, at.r_offset, (next.r_type, next.r_offset, next.key), names)
}

/// The lead's index and form, when `at` is the partner of a sequence
/// that holds: the record it consumes (X86-31, `lead_of`).
#[must_use]
pub fn follower_of(at: &RelocAt<'_>, names: &dyn SymNames) -> Option<(u32, Form)> {
    if !at.leads || !matches!(at.r_type, PLT32 | PC32 | GOT32 | GOT32X) {
        return None;
    }
    let i = at.index.checked_sub(1)?;
    let lead = at.records.get(i)?;
    if !matches!(lead.r_type, TLS_GD | TLS_LDM) {
        return None;
    }
    // `Records` are in `at.site`'s coordinates (arch.md §3.5).
    matched(at.site.bytes, lead.r_type, lead.r_offset, (at.r_type, at.r_offset, at.key), names)
        .map(|f| (i, f))
}

//! Instruction ranges from mapping symbols (a64.md §3.4).

use mortar_arch::{Proof, ProofRun};

/// What a mapping symbol says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mapping {
    Code,
    Data,
}

/// The kind of a symbol if it is a mapping symbol: `STT_NOTYPE`,
/// `STB_LOCAL`, named `$x`, `$x.<anything>`, `$d` or `$d.<anything>`.
#[must_use]
pub fn mapping_kind(name: &[u8], st_info: u8) -> Option<Mapping> {
    if st_info != 0 {
        // st_info = (STB_LOCAL << 4) | STT_NOTYPE = 0.
        return None;
    }
    let kind = match name {
        [b'$', b'x', rest @ ..] => (Mapping::Code, rest),
        [b'$', b'd', rest @ ..] => (Mapping::Data, rest),
        _ => return None,
    };
    match kind.1 {
        [] | [b'.', ..] => Some(kind.0),
        _ => None,
    }
}

/// One mapping symbol of a section: its symbol-table index, `st_value`
/// and kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MapSym {
    pub index: u32,
    pub value: u64,
    pub kind: Mapping,
}

/// The proof runs of one input section of `len` bytes placed at `start`
/// in a code view (§3.4), appended to `out` in order, tagged with `unit`.
///
/// - The symbols are sorted by `(st_value, index)`; each opens a range to
///   the next greater `st_value` or the section's end, and among equal
///   values the last by index sets the kind.
/// - Bytes before the first mapping symbol, and every byte of a section
///   without one, are not proved: `Unproved` in an executable section
///   with content (a64.md §11.1 condition 5), `Data` otherwise.
/// - Symbols at or past `len` open nothing.
///
/// `syms` is sorted in place.
pub fn proof_runs(
    syms: &mut [MapSym],
    len: u64,
    exec: bool,
    start: u64,
    unit: u32,
    out: &mut Vec<ProofRun>,
) {
    syms.sort_unstable_by_key(|s| (s.value, s.index));
    let unproved = if exec { Proof::Unproved } else { Proof::Data };
    let mut push = |from: u64, to: u64, proof: Proof| {
        if from < to {
            out.push(ProofRun {
                start: start.saturating_add(from),
                end: start.saturating_add(to),
                proof,
                unit,
                unit_offset: from,
            });
        }
    };
    let first = syms.first().map_or(len, |s| s.value.min(len));
    push(0, first, unproved);
    let mut i = 0;
    while i < syms.len() {
        let value = syms[i].value;
        if value >= len {
            break;
        }
        // The last symbol with this value sets the kind.
        let mut j = i;
        while j + 1 < syms.len() && syms[j + 1].value == value {
            j += 1;
        }
        let end = syms.get(j + 1).map_or(len, |s| s.value.min(len));
        let proof = match syms[j].kind {
            Mapping::Code => Proof::Insn,
            Mapping::Data => Proof::Data,
        };
        push(value, end, proof);
        i = j + 1;
    }
}

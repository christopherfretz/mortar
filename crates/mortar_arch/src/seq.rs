//! Windows and sequences (§3.4 "Overlap", §3.5), and the instruction
//! proof the erratum scan reads (§3.6 rule 1, §3.7).
//!
//! Nothing here is architecture-specific: an architecture says which
//! records form a matched sequence and what each unit's window is; this
//! module decides overlaps.

use crate::cx::Records;
use crate::vocab::Howto;

/// The bytes one unit may write, `[start, end)` in section offsets, with
/// the unit it belongs to: the lead record's index for a member of a
/// matched sequence, the record's own index otherwise (§3.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnitWindow {
    pub start: u64,
    pub end: u64,
    pub unit: u32,
    pub record: u32,
}

/// The ordinary window of a relocation: `[r_offset − before, r_offset +
/// width + after)` (§3.6). `None` when it wraps around the address space.
/// A marker (`Field::None`) occupies no bytes: its window is empty unless
/// a rule rewrites its word (the architecture then gives that window).
#[must_use]
pub fn field_window(r_offset: u64, h: &Howto) -> Option<(u64, u64)> {
    let start = r_offset.checked_sub(u64::from(h.window.before))?;
    let width = u64::from(h.field.width());
    let end = if width == 0 {
        r_offset
    } else {
        r_offset.checked_add(width)?.checked_add(u64::from(h.window.after))?
    };
    Some((start, end))
}

/// Whether `[start, end)` lies in a section of `len` bytes.
#[must_use]
pub const fn in_section(start: u64, end: u64, len: u64) -> bool {
    start <= end && end <= len
}

/// The records whose window overlaps the window of an earlier unit
/// (§3.4): units are compared by window; members of one unit are exempt
/// from each other; empty windows occupy nothing. Each offending record is
/// reported once, with the record it overlaps, in `(start, record)`
/// order. `windows` is sorted in place.
pub fn overlaps(windows: &mut [UnitWindow], out: &mut Vec<(u32, u32)>) {
    windows.sort_unstable_by_key(|w| (w.start, w.record));
    // Among the windows seen so far, the furthest-reaching one of each of
    // the two units that reach furthest: a later window overlaps an
    // earlier unit other than its own exactly when one of these two,
    // not of its unit, ends after it starts.
    let mut top: [Option<UnitWindow>; 2] = [None, None];
    for &w in windows.iter() {
        if w.start == w.end {
            continue;
        }
        let other = if top[0].is_some_and(|t| t.unit == w.unit) { top[1] } else { top[0] };
        if let Some(seen) = other.filter(|seen| seen.end > w.start) {
            out.push((w.record, seen.record));
        }
        match top {
            [Some(t), _] if t.unit == w.unit => {
                top[0] = Some(if w.end > t.end { w } else { t });
            }
            [_, Some(t)] if t.unit == w.unit => {
                top[1] = Some(if w.end > t.end { w } else { t });
            }
            [Some(t0), _] if w.end > t0.end => top = [Some(w), Some(t0)],
            [Some(_), t1] if t1.is_none_or(|t1| w.end > t1.end) => top[1] = Some(w),
            [None, _] => top[0] = Some(w),
            _ => {}
        }
        if let [Some(t0), Some(t1)] = top
            && t1.end > t0.end
        {
            top = [Some(t1), Some(t0)];
        }
    }
}

/// The record `back` places before `i`, if there is one (§3.5: a member
/// finds its lead by looking back a fixed distance).
#[must_use]
pub fn look_back(records: &dyn Records, i: u32, back: u32) -> Option<(u32, crate::cx::Reloc)> {
    let lead = i.checked_sub(back)?;
    records.get(lead).map(|r| (lead, r))
}

/// What is known about a run of bytes of an output code section (§3.6
/// rule 1; a64.md §3.4, §11.1 condition 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Proof {
    /// A `$x` range, or a synthetic code chunk: instructions.
    Insn,
    /// A `$d` range, or padding between chunks: never an instruction.
    Data,
    /// An executable input section with content and no mapping symbol at
    /// offset 0, outside every mapping range: neither proved code nor
    /// safely skipped.
    Unproved,
}

/// A run of one [`Proof`] over `[start, end)` of a code view, belonging to
/// `unit` (an opaque tag the caller gives: the input section or chunk),
/// where `start` is `unit_offset` bytes into that unit. Runs are sorted and
/// do not overlap; bytes no run covers are padding ([`Proof::Data`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProofRun {
    pub start: u64,
    pub end: u64,
    pub proof: Proof,
    pub unit: u32,
    pub unit_offset: u64,
}

/// The words of one whole output code section (§3.7 `erratum_sites`):
/// the bytes `fill` wrote before any erratum fix, the address of byte 0,
/// and the proof of each byte.
#[derive(Debug, Clone, Copy)]
pub struct Code<'a> {
    pub bytes: &'a [u8],
    pub addr: u64,
    pub proof: &'a [ProofRun],
}

impl Code<'_> {
    /// The word at byte offset `off`, if it lies in the bytes.
    #[must_use]
    pub fn word(&self, off: u64) -> Option<u32> {
        let at = usize::try_from(off).ok()?;
        let b = self.bytes.get(at..at.checked_add(4)?)?;
        Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// The proof of the word at `off`: `Insn` when its four bytes lie in
    /// one `Insn` run; otherwise `Data` when any byte is data or padding;
    /// otherwise `Unproved`, with the run that leaves it unproved.
    #[must_use]
    pub fn word_proof(&self, off: u64) -> (Proof, Option<ProofRun>) {
        let end = off.saturating_add(4);
        // The first run that ends after `off`.
        let at = self.proof.partition_point(|r| r.end <= off);
        let mut covered = off;
        let mut unproved = None;
        for run in &self.proof[at..] {
            if run.start >= end {
                break;
            }
            if run.start > covered {
                return (Proof::Data, None); // padding
            }
            match run.proof {
                Proof::Insn if run.start <= off && run.end >= end => return (Proof::Insn, None),
                Proof::Data => return (Proof::Data, None),
                Proof::Insn => {}
                Proof::Unproved => unproved = unproved.or(Some(*run)),
            }
            covered = run.end;
        }
        if covered < end {
            return (Proof::Data, None);
        }
        // Four bytes covered by more than one run: an instruction lies in
        // one `$x` range or it is not proved.
        match unproved {
            Some(run) => (Proof::Unproved, Some(run)),
            None => (Proof::Data, None),
        }
    }
}

/// An erratum site (§3.7): the code offsets of its first word `W1` (`Q`)
/// and of its final word `F`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ErratumSite {
    pub q: u64,
    pub f: u64,
}

/// A candidate that can be neither proved code nor safely skipped
/// (a64.md §11.1 condition 5): the unit of the unproved word and the
/// offset of `W1` in the unit of `W1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Unproved {
    pub unit: u32,
    pub q: u64,
    pub q_unit: u32,
    pub q_unit_offset: u64,
}

//! Liveness (`spec/image.md` §3, phase 50): which sections and pieces are
//! live, as the table `Liveness` (`spec/model.md` §3.6).
//!
//! Without `--gc-sections` (the default): a section is live exactly when
//! resolution keeps it (I-1), every fragment of a live mergeable section
//! is live (I-2), and an FDE is live exactly when the section its code
//! lies in is (I-3, [`Liveness::fde_live`]; a CIE by [`eh_piece_live`]).
//! `spec/resolve.md` §10 has already dropped consumed sections,
//! `SHF_EXCLUDE` sections, the losing copies of COMDAT groups and
//! stripped sections, so this liveness is a copy of `state == Kept`.
//!
//! With `--gc-sections` (I-6 to I-10, I-109): the least set of kept
//! sections that holds the roots and is closed under the edges of the
//! relocation records, marked in parallel rounds of atomic booleans (I-8);
//! fragments of allocated mergeable sections are live when referenced
//! (S-16). The collector is `gc`; [`removed`] lists what it removed.
//!
//! The only diagnostics of liveness are those of relocation sections it
//! cannot read; their key is `(50, FileId, section index)` ([`PHASE`]).
#![forbid(unsafe_code)]

mod gc;
mod read;

use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};

use mortar_core::assignment::Assignment;
use mortar_core::column::Column;
use mortar_core::inputs::{Inputs, Reader};
use mortar_core::par::Pool;
use mortar_core::resolution::Resolution;
use mortar_core::resolution::SectionKind;
use mortar_core::split::Split;
use mortar_core::{FragId, Liveness, SectionId, SymbolId};
use mortar_diag::{Diagnostic, Key};

/// The phase ordinal of liveness (`spec/model.md` §8).
pub const PHASE: u16 = 50;

/// What liveness reads (`spec/image.md` §3.1): `Inputs`, `Resolution`,
/// `Assignment` and `Split`. M1 reads `resolution` and `split` only;
/// `inputs` (the relocation edges) and `assignment` (`keep`, the start-up
/// tables) are the roots and edges of M2's mark (I-6, I-7).
#[derive(Clone, Copy)]
pub struct LiveInputs<'a, 's> {
    pub inputs: &'a Inputs<'s>,
    pub resolution: &'a Resolution,
    pub assignment: &'a Assignment<'a>,
    pub split: &'a Split,
    /// The keys resolution roots (`spec/resolve.md` §4.2, §4.7: the entry,
    /// `-u`, `--require-defined`): their definitions' sections are roots
    /// under `--gc-sections` (I-6). Unused without it.
    pub roots: &'a [SymbolId],
    /// The architecture, whose `read_addend` gives a `SHT_REL` record's
    /// implicit addend where a piece selection needs it (I-7; i386,
    /// WP-030c). `None`: such a reference marks every piece of its
    /// section (more than needed, never less).
    pub arch: Option<&'a dyn mortar_arch::Arch>,
}

impl std::fmt::Debug for LiveInputs<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveInputs")
            .field("inputs", &self.inputs)
            .field("resolution", &self.resolution)
            .field("assignment", &self.assignment)
            .field("split", &self.split)
            .field("roots", &self.roots)
            .field("arch", &self.arch.map(|a| a.facts().machine))
            .finish()
    }
}

/// The options liveness depends on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LiveOptions {
    /// `--gc-sections` (I-6 to I-10).
    pub gc_sections: bool,
    /// `--gc-keep-exported`: every section defining a global of default or
    /// protected visibility is a root (I-6).
    pub keep_exported: bool,
}

/// The `Liveness` table: I-1 and I-2 without `--gc-sections`, the mark of
/// I-6 to I-8 with it. `Err`: a relocation section the mark could not read
/// (the least-keyed one).
///
/// Cost without `--gc-sections`: one pass over the section rows and, when
/// some mergeable section is not live, one over the merge pieces; both in
/// parallel. With it: one pass over the section headers, one over the
/// keys, one over the `.eh_frame` relocations, and one walk of the
/// relocation records of the live allocated sections (I-9).
#[allow(clippy::result_large_err, reason = "the phases' diagnostic pair, returned once per link")]
pub fn liveness(
    t: &LiveInputs<'_, '_>,
    options: LiveOptions,
    pool: &Pool,
) -> Result<Liveness, (Key, Diagnostic)> {
    if options.gc_sections {
        return gc::collect(t, options, pool);
    }
    let sections = t.resolution.sections.as_slice();
    // I-1: live exactly when kept.
    let mut live = vec![false; sections.len()];
    pool.for_each_mut(&mut live, |s, cell| *cell = sections[s].is_kept());
    let frag_live = frag_live(t.split, &live, pool);
    let live = Column::from_vec(live).unwrap_or_default();
    Ok(Liveness { live, frag_live })
}

/// I-2: every fragment of every piece of a live `Merge` section. A
/// fragment is shared by the pieces of several sections, so the column is
/// marked with idempotent stores (`spec/model.md` §6 rule 2), and is a
/// function of the set of live sections only.
fn frag_live(split: &Split, live: &[bool], pool: &Pool) -> Column<FragId, bool> {
    let frags = split.frags.len();
    let rows = &split.merge_of;
    let is_live = |s: SectionId| live.get(s.0 as usize).copied().unwrap_or(false);
    // Without --gc-sections every kept section is live; when every merge
    // section is, every fragment is (each has an occurrence).
    if rows.iter().all(|row| is_live(row.section)) {
        return Column::filled(frags, true);
    }
    let marks: Vec<AtomicBool> = (0..frags).map(|_| AtomicBool::new(false)).collect();
    pool.for_each(rows.len(), |r| {
        let row = rows[r];
        if !is_live(row.section) {
            return;
        }
        for &frag in split.piece_frag.get(row.pieces.slots()).unwrap_or_default() {
            if let Some(m) = marks.get(frag.0 as usize) {
                m.store(true, Ordering::Relaxed);
            }
        }
    });
    Column::from_vec(marks.into_iter().map(AtomicBool::into_inner).collect()).unwrap_or_default()
}

/// I-3: whether eh piece `piece` (a position in `Split`'s eh columns) of
/// the `EhFrame` section `section` is live. An FDE is live exactly when the
/// section its code range lies in is live ([`Liveness::fde_live`]; folding,
/// M5, is not in M1: nothing is folded); a CIE is live when a live FDE of
/// the same section uses it. `None` when `piece` is not a piece of
/// `section`.
///
/// Cost: an FDE, one lookup; a CIE, a walk of its section's pieces.
#[must_use]
pub fn eh_piece_live(
    liveness: &Liveness,
    split: &Split,
    section: SectionId,
    piece: u32,
) -> Option<bool> {
    let span = split.eh_pieces(section)?;
    if u64::from(piece) < u64::from(span.start) || u64::from(piece) >= span.end() {
        return None;
    }
    let pieces = split.eh_piece.get(span.slots())?;
    let ordinal = (piece - span.start) as usize;
    let p = *pieces.get(ordinal)?;
    let fde = |q: mortar_core::split::EhPiece| liveness.fde_live(q, |_| false);
    if !p.is_cie() {
        return Some(fde(p));
    }
    Some(pieces.iter().any(|&q| !q.is_cie() && q.cie as usize == ordinal && fde(q)))
}

/// The sections `--gc-sections` removed (I-10): kept by resolution, with
/// output of their own, assigned an output section, and not live; in
/// `(FileId, section index)` order, which is `SectionId` order. This is
/// the order of `--print-gc-sections`' lines and of the map's `gc`
/// records.
#[must_use]
pub fn removed(
    resolution: &Resolution,
    assignment: &Assignment<'_>,
    liveness: &Liveness,
) -> Vec<SectionId> {
    resolution
        .sections
        .iter()
        .filter(|(s, row)| {
            row.is_kept()
                && row.kind.carries_output()
                && row.kind != SectionKind::EhFrame
                && assignment.out(*s).is_some()
                && !liveness.is_live(*s)
        })
        .map(|(s, _)| s)
        .collect()
}

/// `--print-gc-sections` (I-10): one line per removed section with bytes,
/// in [`removed`]'s order, as GNU ld prints them to standard error:
/// `<prog>: removing unused section '<name>' in file '<file>'`, where
/// `<file>` is `libx.a(y.o)` for an archive member and `<name>` is
/// `name[signature]` for a member of a COMDAT group. GNU ld skips sections
/// of size 0 (E14 Q9); so does this. Each line ends in a newline.
#[must_use]
pub fn print_gc_sections(
    prog: &str,
    inputs: &Inputs<'_>,
    resolution: &Resolution,
    assignment: &Assignment<'_>,
    liveness: &Liveness,
) -> String {
    let mut out = String::new();
    for s in removed(resolution, assignment, liveness) {
        let Some((file, index)) = resolution.sh_index(s) else { continue };
        let Reader::Object(obj) = inputs.files[file].reader else { continue };
        let Ok(sec) = obj.section(index) else { continue };
        if sec.header().sh_size == 0 {
            continue;
        }
        let mut name = String::from_utf8_lossy(sec.name().unwrap_or_default()).into_owned();
        // GNU ld names a member of a COMDAT group `name[signature]`.
        if let Some(g) = resolution.sections[s].comdat.get()
            && let Some(c) = inputs.comdats.get(g)
        {
            let _ = write!(name, "[{}]", String::from_utf8_lossy(c.name));
        }
        let f = inputs.files[file].display();
        let file = match f.member() {
            Some(m) => format!("{}({m})", f.path()),
            None => f.path().to_owned(),
        };
        let _ = writeln!(out, "{prog}: removing unused section '{name}' in file '{file}'");
    }
    out
}

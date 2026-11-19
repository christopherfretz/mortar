//! WP-225 M2b-2a: the delta layout's inputs of the changed files
//! (`LayoutInput`'s `changes`, `eh` and `pieces`; IN-72 items 1-6) from the
//! lazy `Link`'s per-file producers, ONE adapter ([`layout_inputs`]).
//!
//! Per field, its producer (the record `spec/landed/incr/WP-225.md`,
//! "M2b-2a", design note):
//! - a section's id, file, index, identity, header columns: the file's
//!   front (`front_of_changed` with `Ids::Update`, the ids `ChangedFile::first_section`
//!   allocates, the bytes `ChangedFile::row` names);
//! - `old`: the stored `SECTIONS` row of the file whose identity equals,
//!   by bytes, the capture's row of the section (`ChangedFile::sections`);
//! - `kept`: `changed_facts` (resolve's per-object body, with the update's
//!   keepers `ResolveDelta::keepers`); `live`: without `--gc-sections`
//!   `true`, with it the update's `LiveDelta` (`now_live`, by the fresh ids;
//!   F-8: never a stored bit), as `file_scan`'s own reading (M2b-2d);
//! - the pieces and the `.eh_frame` records: `split_of` (with the update's
//!   resolution, ruling F-9), the records selected by
//!   `EhPlan::select_in` per file (IN-34a) over `eh_source`;
//! - an `.eh_frame` file's rank: `ChangedFile::row.rank`.
//!
//! A plain link never calls this.

use std::sync::atomic::{AtomicBool, Ordering};

use mortar_core::id::SectionId;
use mortar_core::identity::{Anchor, SectionKey};
use mortar_core::incr::{
    EhFile, EhRecordIn, FilePieces, IncrReason, SectionChange, StoredLayout, Unimplemented,
};
use mortar_core::par::Pool;
use mortar_core::plan::Plan;
use mortar_diag::{Collector, Policy};
use mortar_elf::SectionIndex;
use mortar_elf::consts::SHF_COMPRESSED;
use mortar_incr::update::live::LiveDelta;
use mortar_incr::update::resolve::ResolveDelta;
use mortar_incr::update::view::{Change, ChangedFiles, StateView};
use mortar_merge::eh::Kind;
use mortar_merge::phase::Relocs;
use mortar_synth::eh::EhPlan;

use super::file_front::{FrontCache, Ids, front_of_changed};
use super::file_split::{SectionFact, changed_facts, eh_source, split_of};

/// The changed files' part of a `LayoutInput` (every list sorted by its
/// key: `changes` by id, `eh` and `pieces` by file).
#[derive(Debug, Clone, Default)]
pub struct LayoutInputs<'s> {
    pub changes: Vec<SectionChange<'s>>,
    pub eh: Vec<EhFile>,
    pub pieces: Vec<FilePieces>,
}

fn layout() -> IncrReason {
    IncrReason::UpdateNotImplemented(Some(Unimplemented::Layout))
}

fn split() -> IncrReason {
    IncrReason::UpdateNotImplemented(Some(Unimplemented::Split))
}

/// The anchor of `a` as the capture's row encodes it (`StoredSection`'s
/// `anchor_kind`: 0 none, 1 COMDAT, 2 global, 3 local) and its bytes; only
/// COMPARED with a capture row (the agreement of the front's identity with
/// the capture's), never written.
fn anchor_parts(a: Anchor<'_>) -> (u8, &[u8]) {
    match a {
        Anchor::None => (0, &[]),
        Anchor::Comdat(n) => (1, n),
        Anchor::Global(n) => (2, n),
        Anchor::Local(n) => (3, n),
    }
}

/// The layout's inputs of every file of `cf` (U-4's `changes`, `eh`,
/// `pieces`), each file through `front_of_changed` (`cache`, `Ids::Update` with
/// the file's `ChangedFile` ids and row), `changed_facts` (keepers from
/// `delta`), `split_of` and `eh_source` (both with `delta`, F-9). `stored`
/// is the state's layout (`old`'s rows), `plan` the update's line,
/// `relocs` the target's relocation classification. [`layout_inputs_live`]
/// with no liveness: under `--gc-sections` it refuses `(live)`.
///
/// # Errors
/// As [`layout_inputs_live`].
#[allow(
    clippy::too_many_arguments,
    reason = "the state, the update's line and results, the target"
)]
pub fn layout_inputs<'s>(
    cache: &mut FrontCache<'s>,
    view: &StateView<'_>,
    stored: &StoredLayout<'_>,
    plan: &Plan,
    cf: &ChangedFiles,
    delta: &ResolveDelta,
    relocs: &dyn Relocs,
    pool: &Pool,
) -> Result<LayoutInputs<'s>, IncrReason> {
    layout_inputs_live(cache, view, stored, plan, cf, delta, None, relocs, pool)
}

/// [`layout_inputs`] with the update's liveness `live` (`live::live`'s
/// delta, U6; WP-225 M2b-2d). Under `--gc-sections` a section of a changed
/// file is live when its fresh id is in `live.now_live` (a changed file's
/// sections take fresh ids, so every live one is there; `file_scan`'s
/// reading), an FDE's code when it is also kept and assigned, and the
/// file's pieces are those of its live merge sections. Without
/// `--gc-sections` `live` is not read.
///
/// # Errors
/// - `update-not-implemented (live)` under `--gc-sections` with no `live`;
/// - `(layout)` under `--gc-sections` for a live merge section of which
///   the walk marked some pieces only (`live.pieces`): a fragment's
///   liveness is the whole link's (any file's mark, `mortar_live` gc's
///   `frag`), which no file's pieces give, and `FRAG_USE` holds the live
///   fragments only;
/// - `(layout)` for a file that is neither `Change::Changed` nor
///   `Change::DrawnIn` (a drawn-in file's sections take fresh ids, as a
///   changed file's: D3-1, ruling "After M2b-2b's hand-back" 3;
///   `changed_files` makes no other kind yet), an
///   `SHF_COMPRESSED` section (the stored size and `header_of`'s
///   differ), a section whose alignment is not read, and a section whose
///   identity the front and the capture's row do not agree on (a FINDING
///   of two readings of one header);
/// - `(split)` for what the split refuses, and the producers' own.
#[allow(
    clippy::too_many_arguments,
    reason = "the state, the update's line and results, the target"
)]
#[allow(
    clippy::too_many_lines,
    reason = "per file: the changes, the pieces, the records, in order"
)]
pub fn layout_inputs_live<'s>(
    cache: &mut FrontCache<'s>,
    view: &StateView<'_>,
    stored: &StoredLayout<'_>,
    plan: &Plan,
    cf: &ChangedFiles,
    delta: &ResolveDelta,
    live: Option<&LiveDelta>,
    relocs: &dyn Relocs,
    pool: &Pool,
) -> Result<LayoutInputs<'s>, IncrReason> {
    let gc = match (plan.shaping.gc_sections, live) {
        (false, _) => None,
        (true, Some(l)) => Some(l),
        (true, None) => return Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Live))),
    };
    // Live: kept without `--gc-sections` (the caller's `kept` decides),
    // the walk's mark with it.
    let is_live = |id: SectionId| gc.is_none_or(|l| l.now_live.binary_search(&id).is_ok());
    let mut out = LayoutInputs::default();
    for c in &cf.files {
        // A drawn-in file (IN-72 item 2a; M2b-4 item 5) takes fresh ids
        // as a changed one does (D3-1), its bytes the stored ones: one
        // path for both.
        if !matches!(c.change, Change::Changed | Change::DrawnIn) {
            return Err(layout());
        }
        // D2-1's row is `c.row`, its strings in the update's space, read
        // through `ChangedFiles::string` (ruling M2b2a-1).
        let ids = Ids::Update { first_section: c.first_section, row: c.row.clone() };
        let ff = front_of_changed(cache, view, cf, c.file, ids)?;
        let facts = changed_facts(ff, view, plan, &delta.keepers)?;
        if facts.len() != ff.sections.len() || c.sections.len() != ff.sections.len() {
            return Err(layout());
        }
        // `old`: the stored rows of the file by identity, names by bytes.
        let mut by_key = std::collections::HashMap::new();
        for r in stored.sections_of(c.file)? {
            by_key.insert((r.anchor_kind, r.anchor, r.name, r.ordinal), r.id);
        }
        for (k, fs) in ff.sections.iter().enumerate() {
            let row = &c.sections[k];
            let (ak, anchor) = anchor_parts(fs.anchor);
            let name = cf.string(view, row.name)?;
            let row_anchor = cf.string(view, row.anchor)?;
            if (row.anchor_kind, row_anchor.as_slice(), name.as_slice(), row.ordinal)
                != (ak, anchor, fs.name, fs.ordinal)
            {
                return Err(layout());
            }
            if fs.flags & SHF_COMPRESSED != 0 || fs.align == 0 {
                return Err(layout());
            }
            let old = by_key.get(&(ak, anchor.to_vec(), fs.name.to_vec(), fs.ordinal)).copied();
            out.changes.push(SectionChange {
                id: fs.id,
                file: c.file,
                index: fs.index.0,
                key: SectionKey { anchor: fs.anchor, name: fs.name },
                ordinal: fs.ordinal,
                old,
                sh_type: fs.sh_type,
                sh_flags: fs.flags,
                size: fs.size,
                align: fs.align,
                live: is_live(fs.id),
                kept: facts[k].kept,
            });
        }
        let null = SectionFact {
            kind: mortar_core::resolution::SectionKind::Null,
            kept: false,
            out: None,
            reloc: SectionIndex(0),
        };
        let by_index = |i: SectionIndex| facts.get(i.0 as usize).copied().unwrap_or(null);
        let mut s = split_of(ff, view, &by_index, relocs, pool, Some(delta))?;
        // Under `--gc-sections`: the pieces of the live merge sections only
        // (`FilePieces`' contract), each of them marked whole.
        if let Some(l) = gc {
            let marks = |sec: SectionId| {
                l.pieces.binary_search_by_key(&sec, |x| x.0).map_or(&[][..], |i| &l.pieces[i].1[..])
            };
            for p in s.pieces.iter().filter(|p| is_live(p.section)) {
                let m = marks(p.section);
                let at = |o: u32| m.binary_search(&o).is_ok();
                if !(at(u32::MAX) || u32::try_from(p.offset).is_ok_and(at)) {
                    super::changed::record(&format!(
                        "M2b-2d: layout_in: a live merge section {} marked in part (piece at \
                         {:#x} unmarked): (layout)",
                        p.section.0, p.offset
                    ));
                    return Err(layout());
                }
            }
            s.pieces.retain(|p| is_live(p.section));
        }
        // Item 5: the file's kept records (S-46..S-49 per file, IN-34a);
        // an FDE's code section live when kept and assigned (the full
        // link's `liveness.is_live` without `--gc-sections`). An FDE whose
        // code is another file's section: not this adapter's to judge.
        let first = c.first_section.0;
        let foreign = AtomicBool::new(false);
        let code_live = |sec: SectionId| match sec.0.checked_sub(first) {
            Some(k) if (k as usize) < facts.len() => {
                let f = facts[k as usize];
                f.kept && f.out.is_some() && is_live(sec)
            }
            _ => {
                foreign.store(true, Ordering::Relaxed);
                false
            }
        };
        let src = eh_source(ff, view, &by_index, Some(delta), &s.eh)?;
        let diag = Collector::new(Policy::default());
        let list = EhPlan::select_in(&src, code_live, true, pool, &diag).map_err(|_| split())?;
        if foreign.load(Ordering::Relaxed) {
            return Err(layout());
        }
        let mut records = Vec::with_capacity(list.len());
        for r in &list {
            let sec = s.eh.sections.iter().find(|x| x.section == r.section).ok_or_else(split)?;
            let rec = s.eh.records[sec.records.slots()].get(r.piece as usize).ok_or_else(split)?;
            records.push(EhRecordIn {
                section: r.section,
                piece: r.piece,
                size: rec.out_size(),
                cie: matches!(rec.kind, Kind::Cie(_)),
            });
        }
        out.eh.push(EhFile { file: c.file, rank: c.row.rank, records });
        out.pieces.push(FilePieces { file: c.file, pieces: s.pieces });
    }
    out.changes.sort_by_key(|c| c.id);
    out.eh.sort_by_key(|e| e.file);
    out.pieces.sort_by_key(|p| p.file);
    Ok(out)
}

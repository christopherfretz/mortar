//! WP-225 M2b-2a: the delta layout's inputs of the changed files
//! (`LayoutInput`'s `changes`, `eh` and `pieces`; IN-72 items 1-6) from the
//! lazy `Link`'s per-file producers, ONE adapter ([`layout_inputs`]).
//!
//! Per field, its producer (the record `spec/landed/incr/WP-225.md`,
//! "M2b-2a", design note):
//! - a section's id, file, index, identity, header columns: the file's
//!   front (`front_of` with `Ids::Update`, the ids `ChangedFile::first_section`
//!   allocates, the bytes `ChangedFile::row` names);
//! - `old`: the stored `SECTIONS` row of the file whose identity equals,
//!   by bytes, the capture's row of the section (`ChangedFile::sections`);
//! - `kept`: `changed_facts` (resolve's per-object body, with the update's
//!   keepers `ResolveDelta::keepers`); `live`: `true` (`--gc-sections` is
//!   refused, `(live)`);
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
use mortar_incr::update::resolve::ResolveDelta;
use mortar_incr::update::view::{Change, ChangedFiles, StateView};
use mortar_merge::eh::Kind;
use mortar_merge::phase::Relocs;
use mortar_synth::eh::EhPlan;

use super::file_front::{FrontCache, Ids, front_of};
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
/// `pieces`), each file through `front_of` (`cache`, `Ids::Update` with
/// the file's `ChangedFile` ids and row), `changed_facts` (keepers from
/// `delta`), `split_of` and `eh_source` (both with `delta`, F-9). `stored`
/// is the state's layout (`old`'s rows), `plan` the update's line,
/// `relocs` the target's relocation classification.
///
/// # Errors
/// - `update-not-implemented (live)` under `--gc-sections` (no liveness
///   producer is called here);
/// - `(layout)` for a file that is not `Change::Changed` (a drawn-in
///   file's ids are fresh in `cf` but stored in `layout`'s contract: the
///   record's question; `changed_files` makes no other kind yet), an
///   `SHF_COMPRESSED` section (the stored size and `header_of`'s
///   differ), a section whose alignment is not read, and a section whose
///   identity the front and the capture's row do not agree on (a FINDING
///   of two readings of one header);
/// - `(split)` for what the split refuses, and the producers' own.
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
    if plan.shaping.gc_sections {
        return Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Live)));
    }
    let mut out = LayoutInputs::default();
    for c in &cf.files {
        if !matches!(c.change, Change::Changed) {
            return Err(layout());
        }
        // D2-1's row is `c.row`; its `path` and `member_name` are strings
        // of the UPDATE's buffer (past `STRINGS`), which `front_of` reads
        // from the state's. `changed_files` checked both equal, by bytes,
        // to the stored row's (`changed.rs`), so the stored strings name
        // them (the record's finding M2b2a-1).
        let st = view.file(c.file)?;
        let row = mortar_incr::update::rows::FileRow {
            path: st.path,
            member_name: st.member_name,
            ..c.row.clone()
        };
        let ids = Ids::Update { first_section: c.first_section, row };
        let ff = front_of(cache, view, c.file, ids)?;
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
                live: true,
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
        let s = split_of(ff, view, &by_index, relocs, pool, Some(delta))?;
        // Item 5: the file's kept records (S-46..S-49 per file, IN-34a);
        // an FDE's code section live when kept and assigned (the full
        // link's `liveness.is_live` without `--gc-sections`). An FDE whose
        // code is another file's section: not this adapter's to judge.
        let first = c.first_section.0;
        let foreign = AtomicBool::new(false);
        let code_live = |sec: SectionId| match sec.0.checked_sub(first) {
            Some(k) if (k as usize) < facts.len() => {
                let f = facts[k as usize];
                f.kept && f.out.is_some()
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

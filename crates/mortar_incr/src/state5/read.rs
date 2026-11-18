//! The lazy per-block reader of a format-5 state (`spec/incr.md` IN-18c,
//! IN-18d; IN-70 step U1). WP-213 (U-1): the entry point [`open`] and the
//! adapter [`LayoutOf`]; **filled by U-2**.

use std::fs::File;

use mortar_core::incr::{
    IncrReason, LayoutRows, StoredFrag, StoredRegion, StoredSection, StoredSlot, SymtabBlock,
    SymtabTables, Unimplemented,
};
use mortar_core::layout::{Chunk, Defined, Hole, Patch, Redirect, Segment, SymPlace, Tls};
use mortar_core::{ChunkId, FileId, OutSecId};

use crate::update::view::StateView;

/// Open the state on U1's descriptor (WP-211 §C; IN-18c): check the header
/// (magic, version 5, plan hash against `plan_hash`) and the directory in
/// full, and return a view that reads every other block on demand, each
/// hashed before use. The descriptor is read with positioned reads only
/// and is borrowed by the view. Called by the join (IN-70 U1) after the
/// lock and the flag (IN-77b).
///
/// # Errors
/// `state-corrupt`, `state-version` (a format-4 state), `options-changed`
/// (the plan hash). Until U-2 lands: `update-not-implemented (state5)`.
pub fn open(file: &File, plan_hash: [u8; 16]) -> Result<StateView<'_>, IncrReason> {
    let _ = (file, plan_hash);
    Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::State5)))
}

/// The stored layout of a state (`mortar_core::incr::LayoutRows` over a
/// [`StateView`]): the `Placement` rows in the core's types, which U-4 and
/// U-5 read through `StoredLayout::new(&LayoutOf { view })`. The rows to
/// core types translation is the inverse of the capture (U-2's).
///
/// Until U-2 lands every accessor is `update-not-implemented (state5)`.
#[derive(Debug, Clone, Copy)]
pub struct LayoutOf<'v, 's> {
    pub view: &'v StateView<'s>,
}

fn not_yet<T>() -> Result<T, IncrReason> {
    Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::State5)))
}

impl LayoutRows for LayoutOf<'_, '_> {
    fn chunks(&self) -> Result<Vec<(ChunkId, Chunk)>, IncrReason> {
        not_yet()
    }
    fn order(&self) -> Result<Vec<ChunkId>, IncrReason> {
        not_yet()
    }
    fn segments(&self) -> Result<Vec<Segment>, IncrReason> {
        not_yet()
    }
    fn outsecs(&self) -> Result<Vec<(OutSecId, Vec<u8>)>, IncrReason> {
        not_yet()
    }
    fn sections_of(&self, _file: FileId) -> Result<Vec<StoredSection>, IncrReason> {
        not_yet()
    }
    fn regions_of(&self, _file: FileId) -> Result<Vec<StoredRegion>, IncrReason> {
        not_yet()
    }
    fn holes_in(&self, _chunk: ChunkId) -> Result<Vec<Hole>, IncrReason> {
        not_yet()
    }
    fn frags_in(&self, _out: OutSecId) -> Result<Vec<StoredFrag>, IncrReason> {
        not_yet()
    }
    fn slots(&self) -> Result<Vec<StoredSlot>, IncrReason> {
        not_yet()
    }
    fn patches(&self) -> Result<Vec<Patch>, IncrReason> {
        not_yet()
    }
    fn redirects(&self) -> Result<Vec<Redirect>, IncrReason> {
        not_yet()
    }
    fn sym_place(&self) -> Result<Vec<SymPlace>, IncrReason> {
        not_yet()
    }
    fn defined(&self) -> Result<Vec<Defined>, IncrReason> {
        not_yet()
    }
    fn tls_and_entry(&self) -> Result<(Option<Tls>, Option<u64>), IncrReason> {
        not_yet()
    }
    fn symtab_block(&self, _file: FileId) -> Result<Option<SymtabBlock>, IncrReason> {
        not_yet()
    }
    fn symtab_tables(&self) -> Result<(SymtabBlock, SymtabTables), IncrReason> {
        not_yet()
    }
}

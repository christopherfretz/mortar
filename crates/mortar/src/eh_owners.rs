//! The owners of `.eh_frame` and `.gcc_except_table` fields for the scan
//! (`mortar_scan::EhOwners`, WP-177, `spec/resolve.md` §6.3), over phase
//! 60's parsed records and phase 50's liveness.
//!
//! The scan asks only for a reference to a dead target or through a
//! demoted mention nothing live defines, so a link without one pays
//! nothing: the LSDA starts are found on the first question about a
//! `.gcc_except_table` field, once.

use std::sync::OnceLock;

use mortar_core::split::EhOwner;
use mortar_core::{Liveness, SectionId};
use mortar_merge::EhFrames;
use mortar_merge::eh::{EhReloc, LsdaStarts};

/// [`mortar_scan::EhOwners`] of one link.
pub struct Owners<'a> {
    pub frames: &'a EhFrames,
    /// Phase 60's `eh_relocs`: per `frames.sections` row.
    pub relocs: &'a [Box<[EhReloc]>],
    pub liveness: &'a Liveness,
    lsda: OnceLock<LsdaStarts>,
}

impl std::fmt::Debug for Owners<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Owners").finish_non_exhaustive()
    }
}

impl<'a> Owners<'a> {
    #[must_use]
    pub fn new(frames: &'a EhFrames, relocs: &'a [Box<[EhReloc]>], liveness: &'a Liveness) -> Self {
        Self { frames, relocs, liveness, lsda: OnceLock::new() }
    }
}

impl mortar_scan::EhOwners for Owners<'_> {
    fn frame(&self, section: SectionId, offset: u64) -> EhOwner {
        self.frames.frame_owner(section, offset, |s| self.liveness.is_live(s))
    }

    fn lsda(&self, table: SectionId, offset: u64) -> EhOwner {
        let starts = self.lsda.get_or_init(|| {
            LsdaStarts::new(self.frames, self.relocs, |s| self.liveness.is_live(s))
        });
        starts.owner(table, offset)
    }
}

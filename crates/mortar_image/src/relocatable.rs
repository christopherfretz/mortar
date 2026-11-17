//! The image of a `-r` link, RELA targets (`spec/image.md` §17).
//! Owner: **RL-4** (image at `-r`): I-R1 to I-R6, I-R8 (the build-id
//! note over the file with `--build-id`, none without), I-R11, I-R12;
//! `spec/arch.md` A-R1, A-R3, A-R4, A-R5; MD-R2's `.symtab`; I-R5's REL
//! call site, which calls [`crate::relrel::write`] (RL-9's) and so
//! refuses by name until RL-9 fills it. RL-4 also owns the `-r` blocks of
//! `mortar_synth/src/symtab.rs` and the `Relocatable` short cut of
//! `mortar_arch/src/{class.rs,need.rs}`. After RL-7: RL-8 (groups and
//! `.eh_frame`), RL-11 (`-s`/`-S`/`-x`), one after the other.
//!
//! WP-181 (RL-1) wrote this file with the signatures of [`write`] and
//! [`map_of`] and their refusal, the input [`RelocatableLink`], and the
//! map's `-r` records (`crate::map::RelocatableRecords`, with their
//! writer); RL-4 fills [`write`] and [`map_of`] here and may add private
//! items to this file.

use std::path::Path;

use mortar_arch::Arch;
use mortar_core::inputs::CopyError;
use mortar_core::par::Pool;
use mortar_core::plan::Plan;
use mortar_core::{FileId, Inputs, Liveness, RelocatableLayout, Resolution, Split};
use mortar_diag::Key;
use mortar_digest::TreeStyle;

use crate::PHASE_IMAGE;
use crate::fill::Failed;
use crate::link::Written;
use crate::map::Map;
use crate::output::OutputOptions;

/// The tables a `-r` image is a function of (`spec/model.md` §4.6 MD-R1,
/// §5), with the input bytes. RL-7's driver builds it from RL-2's
/// resolution and RL-3's layout.
pub struct RelocatableLink<'a, 's> {
    pub arch: &'a dyn Arch,
    pub pool: &'a Pool,
    pub plan: &'a Plan,
    pub inputs: &'a Inputs<'s>,
    /// RL-2's (`mortar_resolve::relocatable::resolve`).
    pub resolution: &'a Resolution,
    /// Every section of a live file that is not dropped (MD-R1).
    pub liveness: &'a Liveness,
    /// `.eh_frame` only (L-R5 splits nothing else).
    pub split: &'a Split,
    /// RL-3's (`mortar_layout::relocatable::layout`).
    pub layout: &'a RelocatableLayout<'s>,
    /// `Store::recheck` of the blob holding a file, as in a final link's
    /// `Link::recheck` (input.md §11.5).
    pub recheck: &'a (dyn Fn(FileId) -> Result<(), CopyError> + Sync),
    /// The build id's tree (`crate::link::tree_style` of
    /// `plan.shaping.build_id`); `None` without `--build-id` (I-R8).
    pub build_id: Option<TreeStyle>,
    /// The command line's words, for the map's `arg` records.
    pub args: &'a [Vec<u8>],
}

impl std::fmt::Debug for RelocatableLink<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelocatableLink").finish_non_exhaustive()
    }
}

fn refused() -> Failed {
    Failed {
        diagnostics: vec![(
            Key::new(PHASE_IMAGE, 0, 0),
            mortar_diag::relocatable::refused_relocatable_output()
                .note("the image at -r is RL-4's (spec/image.md §17)"),
        )],
    }
}

/// Writes the `ET_REL` output of `link` to `path` (I-R1 to I-R12), as a
/// final link's `crate::link::write` does (the temporary name, the mode
/// `opts.mode`, `0o666` at `-r` (I-59)).
///
/// # Errors
///
/// Every diagnostic of the phase, keyed in phase 100. Until RL-4 lands:
/// `unsupported feature: relocatable output (-r)`.
pub fn write(
    link: &RelocatableLink<'_, '_>,
    path: &Path,
    opts: &OutputOptions,
) -> Result<Written, Failed> {
    let _ = (link, path, opts);
    Err(refused())
}

/// The map of `link` (`spec/plumb.md` §14.2): `file`, the `-r` records
/// ([`Map::relocatable`]), `discard`, `output relocatable` and `arg`.
///
/// # Errors
///
/// Until RL-4 lands: `unsupported feature: relocatable output (-r)`.
pub fn map_of(link: &RelocatableLink<'_, '_>) -> Result<Map, Failed> {
    let _ = link;
    Err(refused())
}

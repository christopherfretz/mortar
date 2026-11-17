//! The place of a definition (`spec/resolve.md` §10.1, rule P-1;
//! `spec/model.md` §4.5, "no place is never a number"; WP-136).
//!
//! A symbol defined in an input section has a place exactly when its
//! section is `Kept`, live and assigned to an output section; otherwise it
//! has a named reason for having none. Every phase that asks whether a
//! definition has an address (the scan's `dead`, the image's values)
//! asks [`section_place`], so they cannot disagree.

use crate::assignment::{Assignment, OutOf};
use crate::id::SectionId;
use crate::liveness::Liveness;
use crate::resolution::{DropReason, Resolution, SectionState};

/// Why a section, and so every symbol defined in it, has no place
/// (P-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NoPlace {
    /// `Dropped` by a row of resolve.md §10's table, or a script.
    Dropped(DropReason),
    /// `Kept`, but consumed by the linker (`OutOf::Consumed`:
    /// `.note.gnu.property`, layout L-34).
    Consumed,
    /// `Kept`, but not live (`--gc-sections`, image I-7).
    Collected,
    /// `Kept` and live, but assigned no output section (a kind without
    /// output; the assignment's check refuses one that carries output).
    Unassigned,
    /// Not a section of a live object.
    Unknown,
}

impl NoPlace {
    /// A short phrase for diagnostics and the landed record.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Dropped(DropReason::Comdat) => "a discarded COMDAT copy",
            Self::Dropped(DropReason::Excluded) => "an excluded section",
            Self::Dropped(DropReason::Consumed) | Self::Consumed => "a section the linker consumes",
            Self::Dropped(DropReason::Stripped) => "a stripped section",
            Self::Dropped(DropReason::Script) => "a section the script discards",
            Self::Collected => "a section --gc-sections collected",
            Self::Unassigned => "a section without output",
            Self::Unknown => "no section of a live object",
        }
    }
}

/// Whether section `s` has a place (P-1): `Ok(())` when it is `Kept`,
/// live, and (when the assignment is known) assigned to an output
/// section; else the reason it has none. Without an assignment (a phase
/// before layout's) only the state and liveness are asked.
///
/// # Errors
///
/// [`NoPlace`], the rule by which the section has no place.
pub fn section_place(
    resolution: &Resolution,
    assignment: Option<&Assignment<'_>>,
    liveness: &Liveness,
    s: SectionId,
) -> Result<(), NoPlace> {
    let Some(row) = resolution.sections.get(s) else { return Err(NoPlace::Unknown) };
    if let SectionState::Dropped(why) = row.state {
        return Err(NoPlace::Dropped(why));
    }
    if let Some(a) = assignment {
        match a.out_of.get(s).copied().unwrap_or(OutOf::None) {
            OutOf::Out(_) => {}
            OutOf::Consumed => return Err(NoPlace::Consumed),
            OutOf::Discard => return Err(NoPlace::Dropped(DropReason::Script)),
            OutOf::None => return Err(NoPlace::Unassigned),
        }
    }
    if !liveness.is_live(s) {
        return Err(NoPlace::Collected);
    }
    Ok(())
}

//! `Liveness` (`spec/model.md` §3.6). Written by `mortar_live`.

use crate::column::Column;
use crate::id::{FragId, SectionId};
use crate::resolution::Resolution;
use crate::split::{EhPiece, Split};
use crate::table::{Broken, ensure};

/// The table (§3.6).
#[derive(Debug, Clone, Default)]
pub struct Liveness {
    /// After `--gc-sections` from the roots; every `Kept` section when it
    /// is off.
    pub live: Column<SectionId, bool>,
    /// Referenced from a live section.
    pub frag_live: Column<FragId, bool>,
}

impl Liveness {
    /// Every section and fragment dead.
    #[must_use]
    pub fn empty(sections: u32, frags: u32) -> Self {
        Self { live: Column::filled(sections, false), frag_live: Column::filled(frags, false) }
    }

    /// Whether a section is live.
    #[must_use]
    pub fn is_live(&self, section: SectionId) -> bool {
        self.live.get(section).copied().unwrap_or(false)
    }

    /// The rule of §3.6 for an FDE: live exactly when the section its code
    /// lies in is live and not folded away (`folded` from `Folds`, §4.1).
    /// A CIE is live when an FDE that uses it is; that is `Slots`' business.
    #[must_use]
    pub fn fde_live(&self, piece: EhPiece, folded: impl Fn(SectionId) -> bool) -> bool {
        piece.code.get().is_some_and(|code| self.is_live(code) && !folded(code))
    }

    /// The invariants of §3.6 that the tables show: a column per section
    /// and per fragment, and only `Kept` sections live.
    pub fn check(&self, resolution: &Resolution, split: &Split) -> Result<(), Broken> {
        let t = "Liveness";
        ensure(self.live.len() == resolution.sections.len(), t, || {
            "live is not per section".to_owned()
        })?;
        ensure(self.frag_live.len() == split.frags.len(), t, || {
            "frag_live is not per fragment".to_owned()
        })?;
        for (section, row) in resolution.sections.iter() {
            ensure(!self.live[section] || row.is_kept(), t, || {
                format!("section {}: live but dropped", section.0)
            })?;
        }
        Ok(())
    }
}

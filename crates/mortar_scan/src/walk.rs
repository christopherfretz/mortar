//! The scan's reading of relocations, for the phases after it that walk
//! relocations again at layout's addresses (WP-027a): the branch sites
//! and the erratum code words of layout's fixed point (`spec/arch.md` §6
//! step 1), which need exactly the keys, howtos and facts the scan saw.
//!
//! [`Walker`] is the scan's own view (`sym_map` and `redirect` for keys,
//! the target kinds of §4.4, the instruction proof of [`CodeProof`]); no
//! rule is restated here.

use mortar_arch::{Arch, ProofRun, RelocAt, ScanTables, Site, SymFacts, SymNames};
use mortar_core::{FileId, SectionId, SymEntry, SymKey};
use mortar_diag::{Diagnostic, Key};
use mortar_elf::SymbolIndex;

use crate::driver::Sec;
use crate::view::{RecTables, View};
use crate::{ScanInputs, ScanOptions};

/// The scan's view of the tables, reusable after the scan.
pub struct Walker<'a, 's> {
    view: View<'a, 's>,
}

impl std::fmt::Debug for Walker<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Walker").finish_non_exhaustive()
    }
}

/// One record as the scan reads it, handed to [`Walker::walk`]'s visitor.
pub struct Visit<'r> {
    pub at: RelocAt<'r>,
    /// The target's facts as `arch.scan` saw them (an undefined key takes
    /// the type of this record's own mention).
    pub tables: &'r dyn ScanTables,
}

impl std::fmt::Debug for Visit<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Visit").field("r_offset", &self.at.r_offset).finish_non_exhaustive()
    }
}

impl<'a, 's> Walker<'a, 's> {
    /// The view of `t` with the scan's options.
    #[must_use]
    pub fn new(arch: &'a dyn Arch, t: &ScanInputs<'a, 's>, opts: ScanOptions<'a>) -> Self {
        Self { view: View::new(arch, t, opts) }
    }

    /// The facts of `key` with no mention (a definition's).
    #[must_use]
    pub fn facts(&self, key: SymKey) -> SymFacts {
        self.view.facts(key, None)
    }

    /// The names of the keys, as their symbol tables have them.
    #[must_use]
    pub fn names(&self) -> &dyn SymNames {
        self
    }

    /// The instruction proof of section `index` of `file`, `len` bytes, in
    /// section offsets (as the scan gives `RelocAt.proof`).
    #[must_use]
    pub fn proof(&self, file: FileId, index: u32, len: u64, exec: bool) -> Vec<ProofRun> {
        let mut out = Vec::new();
        let mut marks = self.view.marks(file, index);
        self.view.opts.proof.runs(&mut marks, len, exec, &mut out);
        out
    }

    /// Visits, in record order, every record of relocation section `rel`
    /// that applies to section `index` (`sid`) of `file` and whose
    /// `r_offset` is in `offsets`, decoded and classified as the scan does.
    /// `leads` is `RelocAt.leads` (`RecIndex::leads`). A record the scan
    /// faulted on (unreadable, bad symbol, refused type) is skipped: the
    /// scan reported it and the link has failed.
    ///
    /// # Errors
    ///
    /// The scan's diagnostic when the section cannot be opened.
    pub fn walk(
        &self,
        file: FileId,
        sid: SectionId,
        index: u32,
        rel: u32,
        offsets: std::ops::Range<u64>,
        leads: bool,
        visit: &mut dyn FnMut(&Visit<'_>),
    ) -> Result<(), (Key, Diagnostic)> {
        let sec = Sec::open(&self.view, file, sid, index, rel)?;
        let n = u32::try_from(sec.recs.records.len()).unwrap_or(u32::MAX);
        for i in 0..n {
            let Some(raw) = sec.recs.records.raw(i) else { continue };
            if !offsets.contains(&raw.r_offset) {
                continue;
            }
            let Some(key) = self.view.key_of(file, sec.recs.table, raw.r_sym) else { continue };
            let site = Site { bytes: sec.recs.bytes, offset: raw.r_offset };
            let Ok(h) = self.view.arch.classify(raw.r_type, site) else { continue };
            let mut at = sec.at(i, &raw, key, &h);
            at.leads = leads;
            let tables = RecTables {
                view: &self.view,
                key,
                mention: SymEntry { file, index: SymbolIndex(raw.r_sym) },
            };
            visit(&Visit { at, tables: &tables });
        }
        Ok(())
    }
}

impl SymNames for Walker<'_, '_> {
    fn name(&self, key: SymKey) -> &[u8] {
        self.view.name(key)
    }
}

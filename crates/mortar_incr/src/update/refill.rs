//! The refill set (`spec/incr.md` IN-27, IN-74; IN-70 step U8). WP-213
//! (U-1): the entry point [`refill_set`]; **filled by U-3** (the function
//! only). The type `RefillSet` is `mortar_core::incr::RefillSet` (moved
//! there by U-1: `mortar_image` reads it and cannot see this crate).

use std::collections::{BTreeMap, BTreeSet};

use mortar_core::incr::{IncrReason, LayoutDelta};
pub use mortar_core::incr::{RefillChunk, RefillSet, RefillUnit};
use mortar_core::layout::Place;
use mortar_core::{FileId, SectionId, SymbolId};
use mortar_elf::consts::SHF_ALLOC;

use super::defined::DefinedChange;
use super::resolve::ResolveDelta;
use super::rows::{FileTable, IndexRow};
use super::view::{ChangedFiles, StateView};
use crate::delta::{TOO_MUCH, reason};
use crate::format::{def_kind, index_kind, kind};

/// The units the update refills (WP-211 §C; IN-74). Called by the join
/// after `recompute` (IN-70 U8).
///
/// `changed` (WP-215 seam 3): the new sections' rows (their sizes count
/// toward IN-16's bound) and the files of IN-27's row 1.
///
/// IN-27's rows, without reading an unchanged input:
/// - `file`: every section past the state's `SECTIONS` count (a changed,
///   appeared, extracted or drawn-in file's new rows) that is placed;
/// - `global`: the referrers (`INDEX` of each file `KEY_REFS` lists) of
///   every key whose `RESOLVED` row `resolved` changes, of every key
///   `defined` changes, and of every global defined in a moved, killed or
///   revived section;
/// - `moved`, `liveness`: the section itself when placed, its own file's
///   references to it (`INDEX` kinds `SECTION`, `LOCAL_SLOT`,
///   `EH_FRAME`, `LINK_ORDER`), and its file's non-allocated units;
/// - `comdat-file`: a re-elected group's old and new keeper's
///   non-allocated units;
/// - `slots`: the units using a global's slot that was appended or died.
///
/// A unit is listed only when the update places it. Chunks (synthetic
/// tables, headers, gaps) are U-6's: `chunks` stays empty.
///
/// # Errors
/// `too-much-changed` (IN-16: the allocated bytes of the units in the
/// set, stored and new, over [`TOO_MUCH`] of the output's),
/// `state-corrupt`.
pub fn refill_set(
    view: &StateView<'_>,
    changed: &ChangedFiles,
    resolved: &ResolveDelta,
    layout: &LayoutDelta,
    defined: &[DefinedChange],
) -> Result<RefillSet, IncrReason> {
    let mut r = Refill::new(view, layout)?;
    r.changed = changed
        .files
        .iter()
        .flat_map(|c| {
            c.sections.iter().enumerate().map(move |(i, x)| {
                (c.first_section.0 + i as u32, (c.file.0, x.sh_flags & SHF_ALLOC != 0, x.size))
            })
        })
        .collect();
    r.changed_files = changed.files.iter().map(|c| c.file.0).collect();
    // IN-27 row 1: the new rows.
    for id in r.sections..layout.placement.place.len() {
        r.add(SectionId(id), reason::FILE);
    }
    // IN-27 row 3: keys whose resolution changed.
    let mut keys: BTreeSet<u32> = BTreeSet::new();
    for (k, row) in &resolved.resolved {
        if k.0 < r.symbols && view.resolved(*k)? != *row {
            keys.insert(k.0);
        }
    }
    for k in &resolved.gone {
        keys.insert(k.0);
    }
    for d in defined {
        keys.insert(d.key.0);
    }
    for k in keys {
        r.key_referrers(k, reason::GLOBAL)?;
    }
    // IN-27 rows 6 and 9.
    let mut defs_moved = 0u64;
    for &s in &layout.moved {
        defs_moved += r.section_moved(s, reason::MOVED, reason::MOVED_REF, reason::MOVED_FILE)?;
    }
    for &s in layout.killed.iter().chain(&layout.revived) {
        r.section_moved(s, reason::LIVE, reason::LIVE, reason::LIVE_FILE)?;
    }
    // IN-27 row 4: the old and the new keeper's other units.
    for (_, old, new) in &resolved.reelected {
        for f in [old, new] {
            r.file_nonalloc(*f, reason::COMDAT_FILE)?;
        }
    }
    // IN-27 rows 5 and 7: slots.
    for s in layout.slots_appended.iter().chain(&layout.slots_dead) {
        if let Some(k) = s.key.as_global()
            && k.0 < r.symbols
        {
            r.key_referrers(k.0, reason::SLOTS)?;
        }
    }
    r.finish(defs_moved)
}

struct Refill<'a, 's> {
    view: &'a StateView<'s>,
    layout: &'a LayoutDelta,
    sections: u32,
    symbols: u32,
    units: BTreeMap<u32, u32>,
    index: BTreeMap<u32, Vec<IndexRow>>,
    /// The new sections: (file, allocated, size).
    changed: BTreeMap<u32, (u32, bool, u64)>,
    changed_files: BTreeSet<u32>,
}

impl<'a, 's> Refill<'a, 's> {
    fn new(view: &'a StateView<'s>, layout: &'a LayoutDelta) -> Result<Self, IncrReason> {
        let count = |k| {
            u32::try_from(view.count(k))
                .map_err(|_| IncrReason::StateCorrupt("a table's count past u32".into()))
        };
        Ok(Self {
            view,
            layout,
            sections: count(kind::SECTIONS)?,
            symbols: count(kind::SYMBOLS)?,
            units: BTreeMap::new(),
            index: BTreeMap::new(),
            changed: BTreeMap::new(),
            changed_files: BTreeSet::new(),
        })
    }

    fn placed(&self, s: SectionId) -> bool {
        self.layout.placement.place.get(s).is_some_and(|p| *p != Place::NONE)
    }

    fn add(&mut self, s: SectionId, why: u32) {
        if self.placed(s) {
            *self.units.entry(s.0).or_default() |= why;
        }
    }

    fn index_of(&mut self, file: u32) -> Result<&[IndexRow], IncrReason> {
        if !self.index.contains_key(&file) {
            let rows = self.view.index_of(FileId(file))?;
            self.index.insert(file, rows);
        }
        Ok(&self.index[&file])
    }

    /// The units naming global `key` or its slot, through the `INDEX` of
    /// every file `KEY_REFS` lists for it.
    fn key_referrers(&mut self, key: u32, why: u32) -> Result<(), IncrReason> {
        let files: BTreeSet<u32> =
            self.view.key_refs(SymbolId(key))?.iter().filter(|r| !r.tomb).map(|r| r.file).collect();
        for f in files {
            let units: Vec<u32> = self
                .index_of(f)?
                .iter()
                .filter(|x| {
                    x.target == key && matches!(x.kind, index_kind::GLOBAL | index_kind::SLOT)
                })
                .map(|x| x.unit)
                .collect();
            for u in units {
                self.add(SectionId(u), why);
            }
        }
        Ok(())
    }

    /// A stored section that moved, died or came back: itself, its own
    /// file's references to it, the referrers of the globals it defines,
    /// its file's non-allocated units. The count of those globals.
    fn section_moved(
        &mut self,
        s: SectionId,
        own: u32,
        refs: u32,
        file: u32,
    ) -> Result<u64, IncrReason> {
        self.add(s, own);
        if s.0 >= self.sections {
            return Ok(0);
        }
        let f = self.view.section(s)?.file;
        let units: Vec<u32> = self
            .index_of(f)?
            .iter()
            .filter(|x| {
                x.target == s.0
                    && matches!(
                        x.kind,
                        index_kind::SECTION
                            | index_kind::LOCAL_SLOT
                            | index_kind::EH_FRAME
                            | index_kind::LINK_ORDER
                    )
            })
            .map(|x| x.unit)
            .collect();
        for u in units {
            self.add(SectionId(u), refs);
        }
        let mut n = 0;
        for m in self.view.mentions_of(FileId(f))? {
            if m.kind == def_kind::SECTION && m.section == s.0 {
                n += 1;
                self.key_referrers(m.symbol, refs)?;
            }
        }
        self.file_nonalloc(FileId(f), file)?;
        Ok(n)
    }

    /// A stored file's non-allocated units.
    fn file_nonalloc(&mut self, f: FileId, why: u32) -> Result<(), IncrReason> {
        // A changed or drawn-in file's units are its new rows (`file`).
        if self.changed_files.contains(&f.0) || self.view.file(f)?.kind == 0 {
            return Ok(());
        }
        let start = self.view.file_index(f)?.ranges[FileTable::Sections as usize].first;
        let rows = self.view.sections_of(f)?;
        for (i, x) in rows.iter().enumerate() {
            if x.sh_flags & SHF_ALLOC == 0 {
                self.add(SectionId(start + i as u32), why);
            }
        }
        Ok(())
    }

    fn finish(self, moved_definitions: u64) -> Result<RefillSet, IncrReason> {
        let mut set = RefillSet { moved_definitions, ..RefillSet::default() };
        let mut files = BTreeSet::new();
        for (&s, &why) in &self.units {
            set.units.push(RefillUnit { section: SectionId(s), reasons: why });
            if s < self.sections {
                let x = self.view.section(SectionId(s))?;
                files.insert(x.file);
                if x.sh_flags & SHF_ALLOC != 0 {
                    set.unit_bytes += x.size;
                }
            } else if let Some(&(file, alloc, size)) = self.changed.get(&s) {
                files.insert(file);
                if alloc {
                    set.unit_bytes += size;
                }
            }
        }
        set.referrer_files = files.len() as u64;
        set.alloc_bytes = self
            .layout
            .placement
            .chunks
            .as_slice()
            .iter()
            .filter(|c| c.is_alloc())
            .map(|c| c.size)
            .sum();
        let (num, den) = TOO_MUCH;
        if set.unit_bytes * den > set.alloc_bytes * num {
            return Err(IncrReason::TooMuchChanged(format!(
                "refilled-bytes={} alloc-bytes={}",
                set.unit_bytes, set.alloc_bytes
            )));
        }
        Ok(set)
    }
}

//! WP-255d2b: a CHANGED file's `.symtab` input
//! ([`mortar_synth::symtab_incr::FileSymbols`]) from its ONE producers.
//!
//! - `locals`: the full link's keep rule of ONE file
//!   (`mortar_synth::symtab::file_locals`, S-74's order, S-76, S-78) over the
//!   file's own symbol table, each section in the update's ids (the
//!   front's), with the kind of each section from its [`SectionFact`]
//!   (`changed_facts`: resolve's per-object body), `-x`/`-X` from the plan.
//!   Not decided here: a local's place. A local of a dead or not kept
//!   section, of a dead merge piece, or of an `.eh_frame` is given as read:
//!   `symtab_incr::plan` drops the `.eh_frame` ones S-50 drops and refuses
//!   the others (`update-not-implemented (symtab)`), by its doc.
//! - `pieces`: `split_of`'s `FileSplit::pieces`;
//! - `eh`: `split_of`'s `FileSplit::eh` and the join's kept records of the
//!   file (`EhFile::records`, S-46 order).
//!
//! The file's DEMOTED globals (S-74: a file's block is its locals, then its
//! demoted globals) are the resolution's (merged visibility, S-77, S-121):
//! [`global_changes`] gives them, [`demoted_run`] the file's run (ruling
//! R-1), appended by the join after `locals`. A plain link never calls
//! this.

use mortar_core::incr::{EhRecordIn, IncrReason, Unimplemented};
use mortar_core::plan::Discard;
use mortar_core::resolution::SectionKind;
use mortar_core::{FileId, SectionId, SymKey, SymbolId};
use mortar_elf::consts::{STT_NOTYPE, STV_HIDDEN, STV_INTERNAL};
use mortar_elf::{SymbolIndex, SymbolSection, SymbolTable};
use mortar_incr::format::{def_kind, kind};
use mortar_incr::update::resolve::ResolveDelta;
use mortar_incr::update::rows::ResolvedRow;
use mortar_incr::update::view::StateView;
use mortar_synth::SymSource;
use mortar_synth::symtab::{GlobalIn, LocalIn, SecKind, SymDef, file_locals, global_info};
use mortar_synth::symtab_incr::{
    FileEh, FileSymbols, GlobalChange, GlobalKind, GlobalPlace, LocalSymbol, NO_ENTRY,
};

use super::file_front::FileFront;
use super::file_split::{FileSplit, SectionFact};

fn not_built() -> IncrReason {
    IncrReason::UpdateNotImplemented(Some(Unimplemented::Symtab))
}

/// The symbols of ONE file for the keep rule: its locals as the full
/// link's `CoreSyms::local` reads them (name, `st_info`, `st_other`,
/// `st_size`, the definition by `section_of`), the section ids the front's.
struct OneFile<'f, 's> {
    file: FileId,
    table: SymbolTable<'s>,
    front: &'f FileFront<'s>,
}

impl OneFile<'_, '_> {
    fn def(&self, index: SymbolIndex, s: &mortar_elf::Sym) -> SymDef {
        match self.table.section_of(index, s) {
            Ok(SymbolSection::Section(k)) => {
                self.front.sections.get(k.0 as usize).map_or(SymDef::Undefined, |x| {
                    SymDef::Section { section: x.id, value: s.st_value }
                })
            }
            Ok(SymbolSection::Absolute) => SymDef::Absolute(s.st_value),
            Ok(SymbolSection::Common) => SymDef::Common,
            _ => SymDef::Undefined,
        }
    }
}

impl SymSource for OneFile<'_, '_> {
    fn files(&self) -> u32 {
        self.file.0 + 1
    }

    fn locals(&self, file: FileId) -> u32 {
        if file == self.file { self.table.first_global().0.saturating_sub(1) } else { 0 }
    }

    fn local(&self, _: FileId, i: u32) -> LocalIn<'_> {
        let index = SymbolIndex(i + 1);
        let Ok(s) = self.table.get(index) else {
            return LocalIn {
                name: b"",
                info: 0,
                other: 0,
                size: 0,
                def: SymDef::Undefined,
                referenced: false,
            };
        };
        LocalIn {
            name: self.table.name(&s).unwrap_or_default(),
            info: s.st_info,
            other: s.st_other,
            size: s.st_size,
            def: self.def(index, &s),
            // As the full link's (`CoreSyms::local`, WP-027b's follow-up).
            referenced: false,
        }
    }

    fn globals(&self) -> u32 {
        0
    }

    fn global(&self, _: SymbolId) -> Option<GlobalIn<'_>> {
        None
    }

    fn key_name(&self, _: SymKey) -> &[u8] {
        b""
    }
}

/// The `.symtab` input of the CHANGED file of `front` (opened with
/// `Ids::Update`): its kept locals in S-74's order, its `.eh_frame`, its
/// merged pieces (WP-255d2b; `symtab_incr::FileSymbols`' contract).
///
/// - `facts`: `changed_facts`' answer for the file (one per section
///   index): the kind of each section (S-78's "split merge section");
/// - `discard`: the update's `plan.shaping.discard` (`-x`, `-X`);
/// - `split`: `split_of`'s answer for the file (`pieces`, `eh`);
/// - `kept`: the file's kept `.eh_frame` records as the update's layout
///   input holds them (`EhFile::records`, S-46 order).
///
/// # Errors
/// `update-not-implemented (symtab)`: the symbol table does not read, or
/// `facts` or the front's sections are not one per section index. A local
/// whose section index the file does not have names nothing, as in the
/// full link (`CoreSyms::local`): dropped by the keep rule.
pub fn file_symbols<'a>(
    front: &'a FileFront<'_>,
    facts: &[SectionFact],
    discard: Option<Discard>,
    split: &'a FileSplit,
    kept: &'a [EhRecordIn],
) -> Result<FileSymbols<'a>, IncrReason> {
    let count = front.object.section_count() as usize;
    if facts.len() != count || front.sections.len() != count {
        return Err(not_built());
    }
    let Some(table) = front.object.symbols().map_err(|_| not_built())? else {
        return Ok(FileSymbols { file: front.file, locals: Vec::new(), eh: None, pieces: &[] });
    };
    let first = front.sections[0].id;
    let fact_of = |s: SectionId| {
        let k = s.0.checked_sub(first.0)? as usize;
        (front.sections.get(k)?.id == s).then(|| facts[k])
    };
    let kind = |s: SectionId| match fact_of(s).map(|f| f.kind) {
        Some(SectionKind::Merge) => SecKind::Merge,
        Some(SectionKind::EhFrame) => SecKind::EhFrame,
        _ => SecKind::Plain,
    };
    let one = OneFile { file: front.file, table, front };
    let mut locals = Vec::new();
    let mut in_eh = false;
    for (i, l) in file_locals(&one, kind, discard, front.file) {
        // The name with the file's lifetime (the table's bytes).
        let s = one.table.get(SymbolIndex(i + 1)).map_err(|_| not_built())?;
        let name = one.table.name(&s).map_err(|_| not_built())?;
        let (section, value) = match l.def {
            SymDef::Section { section, value } => {
                let Some(f) = fact_of(section) else { return Err(not_built()) };
                // S-76: a dead section's locals are dropped. A section the
                // link does not keep (a dropped COMDAT copy: an AArch64
                // copy's `$x`/`$d`) is dead; `kept` is the ONE producer's
                // (`changed_facts`, ruling item 5 of 255d2b's hand-back).
                if !f.kept {
                    continue;
                }
                in_eh |= f.kind == SectionKind::EhFrame;
                (Some(section), value)
            }
            SymDef::Absolute(v) => (None, v),
            // `file_locals` keeps no other definition.
            SymDef::Common | SymDef::Internal | SymDef::Undefined => return Err(not_built()),
        };
        locals.push(LocalSymbol {
            name,
            info: l.info,
            other: l.other,
            section,
            value,
            size: l.size,
        });
    }
    let eh = in_eh.then_some(FileEh { frames: &split.eh, kept });
    Ok(FileSymbols { file: front.file, locals, eh, pieces: &split.pieces })
}

/// The class of a global key's entry after the update (the full link's
/// `SymCtx::classify`, by [`global_info`]): `None` when it has no entry
/// (a hidden or unreferenced undefined), else `(demoted, the entry)`.
type NewEntry<'a> = Option<(bool, GlobalChange<'a>)>;

fn corrupt_key(id: SymbolId, what: &str) -> IncrReason {
    IncrReason::StateCorrupt(format!("SYMTAB_KEYS: key {}: {what}", id.0))
}

/// A defined key's chosen entry in a CHANGED file: the front of the file
/// whose section `section` is (the update's ids), and its symbol-table
/// entry that defines key `id` there (`ResolveDelta::key_ids`, position
/// for position with the front's keys: entry `first_global + k`).
fn changed_definer<'a>(
    fronts: &[&'a FileFront<'a>],
    delta: &ResolveDelta,
    id: SymbolId,
    section: SectionId,
) -> Option<(SymbolTable<'a>, mortar_elf::Sym)> {
    let front = fronts.iter().find(|f| f.sections.iter().any(|s| s.id == section))?;
    let ids = &delta.key_ids.iter().find(|(f, _)| *f == front.file)?.1;
    let table = front.object.symbols().ok()??;
    ids.iter().enumerate().filter(|(_, k)| **k == id).find_map(|(k, _)| {
        let index = SymbolIndex(front.first_global + u32::try_from(k).ok()?);
        let s = table.get(index).ok()?;
        match table.section_of(index, &s) {
            Ok(SymbolSection::Section(i)) => {
                (front.sections.get(i.0 as usize)?.id == section).then_some((table, s))
            }
            _ => None,
        }
    })
}

/// IN-49a: the globals whose `.symtab` entry the update changes, from
/// their ONE producers (ruling R-2 of 255d2b's hand-back):
/// - `kind`: the stored `SYMTAB_KEYS` row (a global entry or not) and the
///   full link's rule ([`global_info`]) over the NEW `RESOLVED` row;
/// - `name`, `info`, `other`: the full link's per-key rule
///   (`crate::syms::defined_global`, then [`global_info`]) over the chosen
///   entry of the changed file that defines it;
/// - `place`, `value`, `size`: the new `RESOLVED` row
///   (`ResolveDelta::resolved`, the section in the update's ids);
/// - `stored`: `view.symtab_key` (`None` for `Added` and `Promoted`).
///
/// `fronts` are the changed files' fronts (`Ids::Update`). `merged` is the
/// resolution's answer per affected key: its merged visibility (S-121) and
/// whether a live relocation refers to it (resolve.md §3.7); see the
/// record's R-4 (`ResolveDelta` does not carry them).
///
/// Every affected key gets an answer for the new link; a key that stays
/// a global is `Changed` (rewritten with what the full link writes). A
/// `Demoted` key is in its definer's block (`FileSymbols::locals`' demoted
/// run, [`demoted_run`]).
///
/// # Errors
/// `state-corrupt` naming the key: a `Removed`, `Demoted` or `Changed` key
/// without its stored `SYMTAB_KEYS` row. `update-not-implemented (symtab)`:
/// a linker-defined key (its rule, `CoreSyms::global`'s `Internal` arm, is
/// not shared yet), a common, a defined key whose definer is not a changed
/// file and whose entry needs a name (demoted or appended: its name may be
/// versioned), a key `merged` does not answer.
pub fn global_changes<'a>(
    view: &StateView<'_>,
    delta: &'a ResolveDelta,
    fronts: &[&'a FileFront<'a>],
    merged: &dyn Fn(SymbolId) -> Option<(u8, bool)>,
) -> Result<Vec<GlobalChange<'a>>, IncrReason> {
    let symbols = view.count(kind::SYMBOLS);
    let mut out = Vec::new();
    for &(id, r) in &delta.resolved {
        let in_state = u64::from(id.0) < symbols;
        let stored = if in_state {
            let k = view.symtab_key(id).map_err(|_| corrupt_key(id, "no row"))?;
            Some((k.index, k.strtab))
        } else {
            None
        };
        let old_global = stored.is_some_and(|(i, _)| i != NO_ENTRY);
        let Some((vis, referenced)) = merged(id) else { return Err(not_built()) };
        let new = new_entry(id, &r, vis, referenced, delta, fronts, old_global)?;
        let kind = match (old_global, &new) {
            (true, None) => GlobalKind::Removed,
            (_, Some((true, _))) => GlobalKind::Demoted,
            (true, Some((false, _))) => GlobalKind::Changed,
            (false, Some((false, _))) if in_state => GlobalKind::Promoted,
            (false, Some((false, _))) => GlobalKind::Added,
            (false, None) => continue,
        };
        let stored = if matches!(kind, GlobalKind::Added | GlobalKind::Promoted) {
            None
        } else if !in_state {
            // A NEW key demoted (a hidden definition): no area entry
            // before, plan's no-op `(Demoted, NO_ENTRY)`; it is in its
            // definer's demoted run only.
            Some((NO_ENTRY, 0))
        } else {
            Some(stored.ok_or_else(|| corrupt_key(id, "a stored key without its row"))?)
        };
        let g = match new {
            Some((_, g)) => g,
            // Removed: the area's last entry takes its index; the fields
            // are not written.
            None => GlobalChange {
                key: id,
                kind,
                name: &[],
                info: 0,
                other: 0,
                place: GlobalPlace::Undefined,
                value: 0,
                size: 0,
                stored,
            },
        };
        out.push(GlobalChange { kind, stored, ..g });
    }
    // Keys no file mentions any more (`ResolveDelta::gone`): a stored
    // global entry is removed.
    for &id in &delta.gone {
        let k = view.symtab_key(id).map_err(|_| corrupt_key(id, "no row"))?;
        if k.index != NO_ENTRY {
            out.push(GlobalChange {
                key: id,
                kind: GlobalKind::Removed,
                name: &[],
                info: 0,
                other: 0,
                place: GlobalPlace::Undefined,
                value: 0,
                size: 0,
                stored: Some((k.index, k.strtab)),
            });
        }
    }
    out.sort_by_key(|g| g.key);
    Ok(out)
}

/// The full link's entry of affected key `id` after the update, from its
/// new `RESOLVED` row `r` (see [`global_changes`]).
fn new_entry<'a>(
    id: SymbolId,
    r: &ResolvedRow,
    vis: u8,
    referenced: bool,
    delta: &ResolveDelta,
    fronts: &[&'a FileFront<'a>],
    old_global: bool,
) -> Result<NewEntry<'a>, IncrReason> {
    let key = |place, name, g: &GlobalIn<'_>, info| GlobalChange {
        key: id,
        kind: GlobalKind::Changed,
        name,
        info,
        other: g.other,
        place,
        value: r.value,
        size: g.size,
        stored: None,
    };
    let base = GlobalIn {
        name: &[],
        typ: r.typ,
        binding: r.bind,
        other: vis & 3,
        size: r.size,
        def: SymDef::Undefined,
        file: FileId::INTERNAL,
        referenced,
    };
    match r.kind {
        def_kind::UNDEFINED => {
            // `CoreSyms::global`: a hidden or internal undefined key, or one
            // no live object refers to, has no entry; else `STT_NOTYPE`,
            // size 0, its binding.
            if matches!(vis & 3, STV_HIDDEN | STV_INTERNAL) || !referenced {
                return Ok(None);
            }
            let g = GlobalIn { typ: STT_NOTYPE, size: 0, other: r.other, ..base };
            let (local, info) = global_info(&g);
            Ok(Some((local, key(GlobalPlace::Undefined, &[], &g, info))))
        }
        def_kind::SECTION | def_kind::ABSOLUTE => {
            let section = SectionId(r.section);
            let place = if r.kind == def_kind::SECTION {
                GlobalPlace::Section(section)
            } else {
                GlobalPlace::Absolute
            };
            let def = match place {
                GlobalPlace::Section(s) => SymDef::Section { section: s, value: r.value },
                _ => SymDef::Absolute(r.value),
            };
            let at = (r.kind == def_kind::SECTION)
                .then(|| changed_definer(fronts, delta, id, section))
                .flatten();
            if let Some((table, s)) = at {
                let name = table.name(&s).map_err(|_| not_built())?;
                let key_name = name.split(|&b| b == b'@').next().unwrap_or(name);
                let g = crate::syms::defined_global(
                    GlobalIn { name: key_name, def, ..base },
                    &s,
                    &table,
                );
                let (local, info) = global_info(&g);
                return Ok(Some((local, key(place, g.name, &g, info))));
            }
            // Not a changed file's definition: the row's own entry fields
            // (`RESOLVED` is the defining entry's `st_info`/`st_other`).
            let g = GlobalIn { def, other: vis & 3 | r.other & !3, ..base };
            let (local, info) = global_info(&g);
            if local || !old_global {
                // Its entry needs its name (a demoted run, or appended):
                // the definer's bytes are not read.
                return Err(not_built());
            }
            Ok(Some((false, key(place, &[], &g, info))))
        }
        _ => Err(not_built()),
    }
}

/// R-1: a changed file's DEMOTED run (S-74's `Seg::Demoted(f)`, after its
/// locals): the `Demoted` changes whose definition is in a section of
/// `front`, in `SymbolId` order (`ResolveDelta::resolved`'s order).
pub fn demoted_run<'a>(
    front: &FileFront<'_>,
    changes: &[GlobalChange<'a>],
) -> Vec<LocalSymbol<'a>> {
    changes
        .iter()
        .filter(|g| g.kind == GlobalKind::Demoted)
        .filter_map(|g| {
            let section = match g.place {
                GlobalPlace::Section(s) => Some(s),
                _ => None,
            };
            let mine = match section {
                Some(s) => front.sections.iter().any(|x| x.id == s),
                // An absolute demoted key: its definer is not in the row
                // (the update gives none; `new_entry` refuses an unread
                // definer's name).
                None => false,
            };
            mine.then_some(LocalSymbol {
                name: g.name,
                info: g.info,
                other: g.other,
                section,
                value: g.value,
                size: g.size,
            })
        })
        .collect()
}

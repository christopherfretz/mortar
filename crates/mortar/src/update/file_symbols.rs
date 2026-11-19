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
//! Not here: the file's DEMOTED globals (S-74: a file's block is its locals,
//! then its demoted globals; `symtab_incr::GlobalKind::Demoted`'s "the join
//! gives it among its definer's locals"). Which keys are demoted is the
//! resolution's (merged visibility, S-77, S-121), not the file's: see the
//! record `spec/landed/incr/WP-255d2b.md`, R-1. A plain link never calls
//! this.

use mortar_core::incr::{EhRecordIn, IncrReason, Unimplemented};
use mortar_core::plan::Discard;
use mortar_core::resolution::SectionKind;
use mortar_core::{FileId, SectionId, SymKey, SymbolId};
use mortar_elf::{SymbolIndex, SymbolSection, SymbolTable};
use mortar_synth::SymSource;
use mortar_synth::symtab::{GlobalIn, LocalIn, SecKind, SymDef, file_locals};
use mortar_synth::symtab_incr::{FileEh, FileSymbols, LocalSymbol};

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

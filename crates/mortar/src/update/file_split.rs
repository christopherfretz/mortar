//! WP-255d3: the per-file split (`spec/landed/incr/WP-255d3.md`; the
//! phase table's "split" row of `spec/landed/incr/WP-255d.md`).
//!
//! [`split_of`] gives ONE file's merge-section pieces and `.eh_frame`
//! records through the full link's own bodies: `mortar_merge::phase::units`
//! over a state-backed [`phase::Link`] (this module's `FileLink`), then
//! `mortar_merge::cut::cut` per split section and `mortar_merge::eh::parse`
//! over the file's `.eh_frame` sections.
//!
//! What `Link` answers, and from where:
//! - `object`, `section_id`, `bytes`: the file's [`FileFront`];
//! - `section`: the id from the front, the relocation section from the
//!   object (the first `SHT_RELA`/`SHT_REL` naming it, as `front_of`
//!   reads it), and the section's kind, kept-ness and output section from
//!   the caller's [`SectionFact`] source. The state does not store them
//!   for a section without a place (the record's M1b; supervisor's ruling
//!   "After 255d3's M1": stored for an unchanged file by WP-257a, the one
//!   per-file producer for a changed file by 255d2). This module derives
//!   none of them.
//! - `global`: the key from `FileFront::keys` (`find_key`), its
//!   resolution from `RESOLVED`;
//! - `class`, `form`: `LinkTarget::of` the object's machine (the full
//!   link's own rule).
//!
//! The split runs before liveness in the full link (phase 60, then 50):
//! a kept merge section that `--gc-sections` drops is split here too;
//! [`FileSplit::pieces`] therefore holds dead pieces as well (the
//! `FRAG_USE` rows are the live ones). A plain link never calls this.

use mortar_core::OutSecId;
use mortar_core::id::{FileId, SectionId, SymbolId};
use mortar_core::incr::{IncrReason, MergeClass, MergedPiece};
use mortar_core::inputs::{LinkTarget, RelocForm};
use mortar_core::par::Pool;
use mortar_core::resolution::SectionKind;
use mortar_elf::consts::{SHT_REL, SHT_RELA, STB_WEAK};
use mortar_elf::{Class, Object, Section, SectionIndex, SymbolIndex};
use mortar_incr::format::{NONE, def_kind};
use mortar_incr::update::view::StateView;
use mortar_merge::cut::{self, Whole};
use mortar_merge::eh::{self, EhFrames, EhIn, EhReloc, Target};
use mortar_merge::phase::{self, LinkSection, Relocs, Unit};
use mortar_merge::{Problem, What};
use std::borrow::Cow;

use super::file_front::FileFront;

/// What `phase::Link::section` answers of a section besides its id and
/// relocation section: its kind, whether it is kept, its output section
/// (the full link's `Resolution` row and `Assignment::out`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionFact {
    pub kind: SectionKind,
    /// `SectionState::Kept` (a COMDAT loser or a discarded section is not).
    pub kept: bool,
    /// `None`: no output section (L-30..L-32, consumed, discarded).
    pub out: Option<OutSecId>,
}

/// One split `.eh_frame` section's relocations, as `phase::Phase::eh_relocs`
/// holds them (sorted by offset).
pub type EhRelocs = Vec<EhReloc>;

/// What [`split_of`] gives of one file.
#[derive(Debug, Clone)]
pub struct FileSplit {
    pub file: FileId,
    /// Every piece of every split (`Unit::Merge`) section of the file, live
    /// or not, in `(section, offset)` order (IN-72 item 6's form; the hash
    /// is L128 of the piece's bytes, as the capture's `frag_hash`).
    pub pieces: Vec<MergedPiece>,
    /// The merge sections placed whole (S-6), and why.
    pub whole: Vec<(SectionId, Whole)>,
    /// The file's kept, assigned `.eh_frame` sections, parsed by
    /// `eh::parse` over this file alone (a CIE's `class` is numbered
    /// within the file).
    pub eh: EhFrames,
    /// Per `eh.sections` row: its relocations.
    pub eh_relocs: Vec<EhRelocs>,
}

/// The state-backed `phase::Link` of one file.
struct FileLink<'a, 's, F> {
    front: &'a FileFront<'s>,
    view: &'a StateView<'a>,
    facts: &'a F,
    target: LinkTarget,
}

impl<'s, F> FileLink<'_, 's, F> {
    fn index(&self, file: FileId, index: SectionIndex) -> Option<usize> {
        (file == self.front.file).then_some(index.0 as usize)
    }

    /// The relocation section applying to section `index` (`front_of`'s
    /// reading): the first `SHT_RELA`/`SHT_REL` whose `sh_info` names it.
    fn reloc(&self, index: SectionIndex) -> SectionIndex {
        let o = &self.front.object;
        for i in 1..o.section_count() {
            let Ok(s) = o.section(SectionIndex(i)) else { continue };
            if matches!(s.sh_type(), SHT_RELA | SHT_REL)
                && s.info_section().is_ok_and(|t| t == index)
            {
                return SectionIndex(i);
            }
        }
        SectionIndex(0)
    }
}

impl<'s, F: Fn(SectionIndex) -> SectionFact + Sync> phase::Link<'s> for FileLink<'_, 's, F> {
    fn files(&self) -> u32 {
        self.front.file.0 + 1
    }

    fn object(&self, file: FileId) -> Option<Object<'s>> {
        (file == self.front.file).then_some(self.front.object)
    }

    fn section_id(&self, file: FileId, index: SectionIndex) -> Option<SectionId> {
        Some(self.front.sections.get(self.index(file, index)?)?.id)
    }

    fn section(&self, file: FileId, index: SectionIndex) -> Option<LinkSection> {
        let id = self.section_id(file, index)?;
        let fact = (self.facts)(index);
        let out = fact.out.filter(|_| fact.kept)?;
        Some(LinkSection { id, kind: fact.kind, out, reloc: self.reloc(index) })
    }

    fn global(&self, file: FileId, index: SymbolIndex, first_global: u32) -> Result<Target, What> {
        if file != self.front.file {
            return Err(What::Malformed(format!("file {} is not the split's", file.0)));
        }
        let k = index.0.checked_sub(first_global).map(|k| k as usize);
        let Some(key) = k.and_then(|k| self.front.keys.get(k)) else {
            return Err(What::Malformed(format!("symbol {} has no key", index.0)));
        };
        // A changed file's new key (U-3 allocates it): no resolution here.
        let Some(id) = key.key else {
            return Err(What::Malformed(format!("symbol {} names a new key", index.0)));
        };
        let r = self.view.resolved(id).map_err(|e| What::Malformed(format!("{e:?}")))?;
        Ok(target_of(id, r.kind, r.bind, r.section, r.value))
    }

    fn bytes(&self, file: FileId, s: &Section<'s>) -> Result<Cow<'s, [u8]>, What> {
        if file != self.front.file {
            return Err(What::Malformed(format!("file {} is not the split's", file.0)));
        }
        let h = s.header();
        if h.sh_type == mortar_elf::consts::SHT_NOBITS || h.sh_size == 0 {
            return Ok(Cow::Borrowed(&[]));
        }
        s.data().map(Cow::Borrowed).map_err(|e| What::Malformed(e.to_string()))
    }

    fn class(&self) -> Class {
        self.target.class
    }

    fn form(&self) -> RelocForm {
        self.target.reloc
    }
}

/// `phase::Tables::global`'s answer from a `RESOLVED` row of key `id`
/// (the row is the key's resolution after the full link's redirect).
fn target_of(id: SymbolId, kind: u8, bind: u8, section: u32, value: u64) -> Target {
    match kind {
        def_kind::SECTION if section != NONE => {
            Target::Section { section: SectionId(section), value }
        }
        def_kind::ABSOLUTE => Target::Absolute(value),
        def_kind::UNDEFINED => Target::Undefined { symbol: id, weak: bind == STB_WEAK },
        _ => Target::Elsewhere(id),
    }
}

/// The reason of a problem the split of an opened file meets: the full
/// link diagnoses it, the update falls back.
fn reason(front: &FileFront<'_>, p: &Problem) -> IncrReason {
    IncrReason::InputChanged(format!(
        "file {}: the split of section {} meets {:?} (the full link reports it)",
        front.file.0, p.section, p.what
    ))
}

/// The split of the file of `front`, with `facts` answering each section
/// index's kind, kept-ness and output section, and `relocs` the target's
/// relocation classification (`phase::ArchRelocs` of the link's `Arch`).
///
/// # Errors
/// `input-changed` for a section the split refuses or finds malformed
/// (the full link reports it); `state-corrupt` never from here (the
/// rows `global` reads give theirs as a problem).
pub fn split_of<F>(
    front: &FileFront<'_>,
    view: &StateView<'_>,
    facts: &F,
    relocs: &dyn Relocs,
    pool: &Pool,
) -> Result<FileSplit, IncrReason>
where
    F: Fn(SectionIndex) -> SectionFact + Sync,
{
    let target = LinkTarget::of(front.object.machine());
    let link = FileLink { front, view, facts, target };
    let units = phase::units(&link, front.file, relocs).map_err(|p| reason(front, &p))?;
    let mut pieces = Vec::new();
    let mut whole = Vec::new();
    let mut eh_units = Vec::new();
    let mut offs = Vec::new();
    for u in units {
        match u {
            Unit::Merge(m, bytes) => {
                offs.clear();
                cut::cut(&bytes, m.strings, m.entsize as usize, &mut offs).map_err(
                    |(what, offset)| {
                        let p = Problem { file: m.file, section: m.index, piece: 0, offset, what };
                        reason(front, &p)
                    },
                )?;
                let class = MergeClass { strings: m.strings, entsize: m.entsize, align: m.align };
                for (j, &o) in offs.iter().enumerate() {
                    let end = offs.get(j + 1).map_or(bytes.len(), |&e| e as usize);
                    let b = &bytes[o as usize..end];
                    pieces.push(MergedPiece {
                        section: m.section,
                        offset: u64::from(o),
                        size: b.len() as u64,
                        out: m.out,
                        class,
                        hash: mortar_digest::l128(b),
                    });
                }
            }
            Unit::Whole(s, w) => whole.push((s, w)),
            Unit::Eh { section, index, bytes, relocs } => {
                eh_units.push((section, index, bytes, relocs));
            }
        }
    }
    let word = match target.class {
        Class::Elf32 => 4,
        Class::Elf64 => 8,
    };
    let eh_in: Vec<EhIn<'_>> = eh_units
        .iter()
        .map(|(section, index, bytes, relocs)| EhIn {
            section: *section,
            file: front.file,
            index: *index,
            bytes,
            relocs,
        })
        .collect();
    let eh = eh::parse(pool, word, &eh_in).map_err(|ps| {
        let first = ps.into_iter().next();
        first.map_or_else(
            || IncrReason::InputChanged(format!("file {}: .eh_frame", front.file.0)),
            |p| reason(front, &p),
        )
    })?;
    drop(eh_in);
    let eh_relocs = eh_units.into_iter().map(|u| u.3).collect();
    Ok(FileSplit { file: front.file, pieces, whole, eh, eh_relocs })
}

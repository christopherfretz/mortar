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
//!   resolution from `RESOLVED` for an unchanged file, from the update's
//!   `ResolveDelta::resolved` for a changed one (ruling F-9: the stored row
//!   is the OLD link's: a definer that moved, or the file's own definition
//!   at its old section id);
//! - `class`, `form`: `LinkTarget::of` the object's machine (the full
//!   link's own rule).
//!
//! The split runs before liveness in the full link (phase 60, then 50):
//! a kept merge section that `--gc-sections` drops is split here too;
//! [`FileSplit::pieces`] therefore holds dead pieces as well (the
//! `FRAG_USE` rows are the live ones). A plain link never calls this.

use mortar_core::OutSecId;
use mortar_core::id::{ComdatId, FileId, IdRange, OptId, SectionId, SymbolId};
use mortar_core::incr::{IncrReason, MergeClass, MergedPiece, Unimplemented};
use mortar_core::inputs::{LinkTarget, RelocForm};
use mortar_core::par::Pool;
use mortar_core::plan::Plan;
use mortar_core::resolution::SectionKind;
use mortar_elf::consts::{GRP_COMDAT, SHT_GROUP, SHT_REL, SHT_RELA, STB_WEAK};
use mortar_elf::{Class, Object, Section, SectionIndex, SymbolIndex};
use mortar_incr::format::{NONE, def_kind};
use mortar_incr::update::resolve::ResolveDelta;
use mortar_incr::update::view::StateView;
use mortar_input::facts::effective_signature;
use mortar_layout::Header;
use mortar_layout::assign::{Fate, fate};
use mortar_layout::headers::header_of;
use mortar_merge::What;
use mortar_merge::cut::{self, Whole};
use mortar_merge::eh::{self, EhFrames, EhIn, EhReloc, Target};
use mortar_merge::phase::{self, LinkSection, Relocs, Unit};
use mortar_resolve::resolve::object_section_rows;
use mortar_resolve::tables::section_info;
use mortar_resolve::view::Groups;
use std::borrow::Cow;
use std::collections::BTreeMap;

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
    /// F-9: the update's resolution, for a CHANGED file (`Ids::Update`);
    /// `None` for an unchanged one (its keys' `RESOLVED` rows stand).
    delta: Option<&'a ResolveDelta>,
}

impl<F> FileLink<'_, '_, F> {
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
        let Some(delta) = self.delta else {
            // An unchanged file: its key's stored resolution.
            let Some(id) = key.key else {
                return Err(What::Malformed(format!("symbol {} names a new key", index.0)));
            };
            let r = self.view.resolved(id).map_err(|e| What::Malformed(format!("{e:?}")))?;
            return Ok(target_of(id, r.kind, r.bind, r.section, r.value));
        };
        // F-9: a changed file's key is affected (IN-71: every key a changed
        // file mentions), so the update's row is THE answer; its section
        // is in the update's ids, as `section_id`'s. A key `find_key` did
        // not find is U-3's new key of that name (`new_keys`, numbered
        // past `SYMBOLS`).
        let id = if let Some(id) = key.key {
            id
        } else {
            let version = key.version.unwrap_or_default();
            let at = delta
                .new_keys
                .iter()
                .position(|n| n.name == key.name && n.version == version)
                .ok_or_else(|| {
                    What::Malformed(format!("symbol {}: no new key of its name", index.0))
                })?;
            let symbols = self.view.count(mortar_incr::format::kind::SYMBOLS);
            u32::try_from(symbols + at as u64)
                .map(SymbolId)
                .map_err(|_| What::Malformed("SYMBOLS past u32".into()))?
        };
        let at = delta.resolved.binary_search_by_key(&id, |r| r.0).map_err(|_| {
            What::Malformed(format!("symbol {}: key {} is not affected", index.0, id.0))
        })?;
        let r = &delta.resolved[at].1;
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
/// link diagnoses it, the update falls back (`update-not-implemented
/// (split)`, ruling D2-2: the file was opened and its hash checked, so its
/// input did not change).
fn reason() -> IncrReason {
    IncrReason::UpdateNotImplemented(Some(Unimplemented::Split))
}

/// The split of the file of `front`, with `facts` answering each section
/// index's kind, kept-ness and output section, and `relocs` the target's
/// relocation classification (`phase::ArchRelocs` of the link's `Arch`).
/// `delta`: the update's resolution when the file is CHANGED (opened with
/// `Ids::Update`; ruling F-9: its globals answer from
/// `ResolveDelta::resolved`), `None` for an unchanged file; a file whose
/// row's hash is not the stored one's given with `None` is refused.
///
/// # Errors
/// `update-not-implemented (split)` for a section the split refuses or
/// finds malformed (the full link reports it); `state-corrupt` never from here (the
/// rows `global` reads give theirs as a problem).
pub fn split_of<F>(
    front: &FileFront<'_>,
    view: &StateView<'_>,
    facts: &F,
    relocs: &dyn Relocs,
    pool: &Pool,
    delta: Option<&ResolveDelta>,
) -> Result<FileSplit, IncrReason>
where
    F: Fn(SectionIndex) -> SectionFact + Sync,
{
    // F-9: a changed file's bytes never answer from the old link's rows.
    if delta.is_none() && front.row.hash != view.file(front.file)?.hash {
        return Err(reason());
    }
    let target = LinkTarget::of(front.object.machine());
    let link = FileLink { front, view, facts, target, delta };
    let units = phase::units(&link, front.file, relocs).map_err(|_| reason())?;
    let mut pieces = Vec::new();
    let mut whole = Vec::new();
    let mut eh_units = Vec::new();
    let mut offs = Vec::new();
    for u in units {
        match u {
            Unit::Merge(m, bytes) => {
                offs.clear();
                cut::cut(&bytes, m.strings, m.entsize as usize, &mut offs).map_err(|_| reason())?;
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
    let eh = eh::parse(pool, word, &eh_in).map_err(|_| reason())?;
    drop(eh_in);
    let eh_relocs = eh_units.into_iter().map(|u| u.3).collect();
    Ok(FileSplit { file: front.file, pieces, whole, eh, eh_relocs })
}

/// The `.eh_frame` source of `frames` (a [`FileSplit::eh`] of the same
/// `front`, `facts` and `delta`): `mortar_synth::eh::MergeEh::new` over
/// the file's `FileLink`, the full link's one producer of each section's
/// `EhInfo` and bytes (WP-225 M2b-2a: `EhPlan::select_in` reads it for
/// IN-72 item 5's records).
///
/// # Errors
/// `update-not-implemented (split)` when the rows are not the split's.
pub fn eh_source<'a, 's: 'a, F>(
    front: &FileFront<'s>,
    view: &StateView<'_>,
    facts: &F,
    delta: Option<&ResolveDelta>,
    frames: &'a EhFrames,
) -> Result<mortar_synth::eh::MergeEh<'a>, IncrReason>
where
    F: Fn(SectionIndex) -> SectionFact + Sync,
{
    let target = LinkTarget::of(front.object.machine());
    let link = FileLink { front, view, facts, target, delta };
    mortar_synth::eh::MergeEh::new(frames, &link).map_err(|_| reason())
}

/// The section facts of a CHANGED file (`front`, opened with
/// `Ids::Update`), one per section index (index 0 included), from their
/// ONE producers (WP-255d2c; rulings D2-5..D2-8):
/// - `kept` and `kind`: resolve's per-object body
///   (`mortar_resolve::resolve::object_section_rows`, the whole link's
///   `section_rows`) over the file's groups, with each COMDAT signature's
///   keeper from the update's resolution (`keepers`:
///   `ResolveDelta::keepers`, the update's election);
/// - `out`: layout's `assign::fate` (the whole link's) and the state's
///   `OUTSECS` row of the name (the layout is the stored one).
///
/// `plan`: the update's line (its `-S`/`--gdb-index`). A plain link never
/// calls this.
///
/// # Errors
/// `update-not-implemented (resolve)` for a group signature `keepers` does
/// not name, a group or section the whole link would report or refuse
/// (resolve's problems; the update carries no diagnostic); `(layout)` for
/// a section `fate` refuses or whose output section the state has no
/// `OUTSECS` row of (an orphan placement: U-4's) or more than one
/// (ruling D2-9: two output sections share the name, IN-18e's ordinal;
/// which one the section joins is the layout's, never a guess);
/// `state-corrupt` from the rows.
pub fn changed_facts(
    front: &FileFront<'_>,
    view: &StateView<'_>,
    plan: &Plan,
    keepers: &[(Vec<u8>, Option<FileId>)],
) -> Result<Vec<SectionFact>, IncrReason> {
    let resolve = || IncrReason::UpdateNotImplemented(Some(Unimplemented::Resolve));
    let layout = || IncrReason::UpdateNotImplemented(Some(Unimplemented::Layout));
    let object = &front.object;
    let machine = object.machine();
    let count = object.section_count();
    // The file's groups as a `View` gives them, the signatures numbered
    // in the file (`ComdatId(k)`: the k-th distinct name), their keepers
    // in that numbering.
    let table = object.symbols().map_err(|_| resolve())?;
    let mut groups = Groups::default();
    let mut sig_ids: BTreeMap<&[u8], u32> = BTreeMap::new();
    let mut keeper: Vec<OptId<FileId>> = Vec::new();
    let mut members = Vec::new();
    for i in 1..count {
        let s = object.section(SectionIndex(i)).map_err(|_| resolve())?;
        if s.sh_type() != SHT_GROUP {
            continue;
        }
        let g = s.group().map_err(|_| resolve())?;
        let name = effective_signature(object, table.as_ref(), &s).map_err(|_| resolve())?;
        members.clear();
        for m in g.members() {
            members.push(m.map_err(|_| resolve())?.0);
        }
        let next = sig_ids.len() as u32;
        let sig = *sig_ids.entry(name).or_insert(next);
        if sig == next {
            keeper.push(OptId::NONE);
        }
        if g.flags() == GRP_COMDAT && !members.is_empty() {
            let at =
                keepers.binary_search_by(|k| k.0.as_slice().cmp(name)).map_err(|_| resolve())?;
            keeper[sig as usize] = keepers[at].1.map_or(OptId::NONE, OptId::some);
        }
        groups.push(SectionIndex(i), g.flags(), ComdatId(sig), &members);
    }
    let rows = object_section_rows(machine, plan, front.file, &groups, &keeper, count, |i| {
        section_info(object, i)
    })
    .ok_or_else(resolve)?;
    // `gather`'s row of each section, with the ids the front gives the
    // file (`first_section + k` for a changed file).
    let range = front.sections.first().map_or(IdRange::new(SectionId(0), SectionId(0)), |s| {
        IdRange::new(s.id, SectionId(s.id.0.saturating_add(count)))
    });
    let mut outsecs: Option<BTreeMap<Vec<u8>, Option<OutSecId>>> = None;
    let mut facts = Vec::with_capacity(rows.len());
    for (i, row) in rows.iter().enumerate() {
        let header = if i == 0 {
            Header::EMPTY
        } else {
            header_of(object, front.file, SectionIndex(i as u32), range).map_err(|_| resolve())?
        };
        let out = match fate(&header, row, machine).map_err(|_| layout())? {
            Fate::Out(name) => {
                if outsecs.is_none() {
                    let mut rows = Vec::new();
                    for r in &view.outsecs()? {
                        rows.push((view.string(r.name)?, r.ordinal));
                    }
                    outsecs = Some(out_ids(rows));
                }
                let id = outsecs.as_ref().and_then(|m| m.get(name).copied().flatten());
                Some(id.ok_or_else(layout)?)
            }
            Fate::None | Fate::Discard | Fate::Consumed => None,
        };
        facts.push(SectionFact { kind: row.kind, kept: row.is_kept(), out });
    }
    Ok(facts)
}

/// The state's output section of each name, from the `OUTSECS` rows
/// `(name, ordinal)` in row order (row `k` is `OutSecId(k)`): `Some` for a
/// name with ONE row, of ordinal 0; `None` for a name with a row of
/// ordinal > 0 or two rows (ruling D2-9: `update-not-implemented
/// (layout)` for a section sent there).
fn out_ids(rows: Vec<(Vec<u8>, u32)>) -> BTreeMap<Vec<u8>, Option<OutSecId>> {
    let mut by_name = BTreeMap::new();
    for (k, (name, ordinal)) in rows.into_iter().enumerate() {
        let id = (ordinal == 0).then_some(OutSecId(k as u32));
        by_name.entry(name).and_modify(|e| *e = None).or_insert(id);
    }
    by_name
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(r: &[(&str, u32)]) -> BTreeMap<Vec<u8>, Option<OutSecId>> {
        out_ids(r.iter().map(|&(n, o)| (n.as_bytes().to_vec(), o)).collect())
    }

    /// D2-9: a name with one row maps to it; a name two output sections
    /// share (ordinals 0 and 1, in either row order) maps to none, and so
    /// does a lone row of ordinal > 0 or a repeated ordinal 0.
    #[test]
    fn a_shared_output_name_has_no_id() {
        let m = rows(&[
            (".text", 0),
            (".data", 0),
            (".rodata", 1),
            (".bss", 0),
            (".rodata", 0),
            (".tdata", 1),
            (".init", 0),
            (".init", 0),
        ]);
        assert_eq!(m.get(&b".text"[..]), Some(&Some(OutSecId(0))));
        assert_eq!(m.get(&b".data"[..]), Some(&Some(OutSecId(1))));
        assert_eq!(m.get(&b".bss"[..]), Some(&Some(OutSecId(3))));
        assert_eq!(m.get(&b".rodata"[..]), Some(&None));
        assert_eq!(m.get(&b".tdata"[..]), Some(&None));
        assert_eq!(m.get(&b".init"[..]), Some(&None));
        assert_eq!(m.get(&b".got"[..]), None);
        let m = rows(&[(".rodata", 0), (".rodata", 1)]);
        assert_eq!(m.get(&b".rodata"[..]), Some(&None));
    }
}

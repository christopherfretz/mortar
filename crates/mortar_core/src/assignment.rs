//! `Assignment` (`spec/model.md` §3.4): the output section of each kept
//! section. Written by `mortar_layout`'s first stage.

use crate::column::Column;
use crate::id::{OutSecId, SectionId, SymbolId};
use crate::resolution::{Def, DropReason, InternalDef, RefFlags, Resolution, SectionState};
use crate::table::{Broken, ensure, unsorted_at};

/// Where a section goes (§3.4 `out_of`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutOf {
    /// Not assigned: a section without output, or not kept.
    None,
    /// A script's `/DISCARD/`: the section's state is `Dropped(Script)`.
    Discard,
    /// Kept, but consumed by the linker rather than placed
    /// (`spec/layout.md` §8.1): `.note.gnu.property`, whose content the
    /// synthetic property note merges (L-34). No output section, no place.
    Consumed,
    /// Into this output section.
    Out(OutSecId),
}

impl OutOf {
    /// The output section, if the section is assigned to one.
    #[must_use]
    pub fn out(self) -> Option<OutSecId> {
        match self {
            Self::Out(id) => Some(id),
            Self::None | Self::Discard | Self::Consumed => None,
        }
    }
}

/// How the pieces of an output section's mergeable inputs are compared
/// (§3.4: the entry size class for merging).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MergeClass {
    /// NUL-terminated strings (`SHF_STRINGS`), else fixed-size constants.
    pub strings: bool,
    pub entsize: u64,
}

/// A script's `ONLY_IF_RO` / `ONLY_IF_RW`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Constraint {
    OnlyIfRo,
    OnlyIfRw,
}

/// One output section (§3.4 `outsecs[OutSecId]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutSec<'n> {
    /// From the default rules, a script, or an input section's name.
    pub name: &'n [u8],
    pub sh_type: u32,
    pub flags: u64,
    /// For an output section with mergeable inputs.
    pub merge: Option<MergeClass>,
    /// From a script: `KEEP` on the matching pattern.
    pub keep: bool,
    /// From a script: the fill pattern.
    pub fill: Option<u32>,
    pub constraint: Option<Constraint>,
    /// From a script: the position of the output-section statement, which
    /// orders it.
    pub script_position: Option<u32>,
}

/// The table (§3.4).
#[derive(Debug, Clone, Default)]
pub struct Assignment<'n> {
    pub out_of: Column<SectionId, OutOf>,
    pub outsecs: Column<OutSecId, OutSec<'n>>,
    /// Whether a section is a root of liveness by rule or by `KEEP`.
    pub keep: Column<SectionId, bool>,
    /// The conditional linker-defined symbols (`Def::Internal(Provide {
    /// conditional: true })`) withdrawn because what they depend on is not
    /// in the output (`spec/resolve.md` §8.1), sorted. A withdrawn symbol
    /// is undefined from here on; the rest are confirmed.
    pub withdrawn: Box<[SymbolId]>,
}

impl Assignment<'_> {
    /// A table for `sections` sections, none assigned, no output sections.
    #[must_use]
    pub fn empty(sections: u32) -> Self {
        Self {
            out_of: Column::filled(sections, OutOf::None),
            outsecs: Column::new(),
            keep: Column::filled(sections, false),
            withdrawn: Box::new([]),
        }
    }

    /// Whether a conditional linker-defined symbol was withdrawn.
    #[must_use]
    pub fn is_withdrawn(&self, symbol: SymbolId) -> bool {
        self.withdrawn.binary_search(&symbol).is_ok()
    }

    /// The definition of a key after assignment: `resolution`'s, except
    /// that a withdrawn symbol is undefined, weak unless a reference to it
    /// is strong.
    #[must_use]
    pub fn def(&self, resolution: &Resolution, symbol: SymbolId) -> Option<Def> {
        let def = *resolution.def.get(symbol)?;
        if self.is_withdrawn(symbol) {
            let strong = resolution.props[symbol].refs.contains(RefFlags::STRONG);
            return Some(Def::Undefined { weak: !strong });
        }
        Some(def)
    }

    /// The output section of a section, if it has one.
    #[must_use]
    pub fn out(&self, section: SectionId) -> Option<OutSecId> {
        self.out_of.get(section).and_then(|o| o.out())
    }

    /// The invariants of §3.4 that the table and `Resolution` show:
    /// columns per section; output sections in range; a section is assigned
    /// to an output section only when it carries output and is kept or
    /// dropped with its COMDAT group (`spec/layout.md` L-42, L-43: the
    /// assignment does not depend on which copy of a group is kept); a kept
    /// section with output is assigned or consumed; `Discard` exactly when
    /// the script dropped it; `keep` only on kept sections; `withdrawn`
    /// sorted, each a conditional linker-defined symbol in range.
    pub fn check(&self, resolution: &Resolution) -> Result<(), Broken> {
        let t = "Assignment";
        let n = resolution.sections.len();
        ensure(self.out_of.len() == n && self.keep.len() == n, t, || {
            "columns are not per section".to_owned()
        })?;
        for (section, row) in resolution.sections.iter() {
            let s = section.0;
            let script = row.state == SectionState::Dropped(DropReason::Script);
            let assignable = row.kind.carries_output()
                && (row.is_kept() || row.state == SectionState::Dropped(DropReason::Comdat));
            match self.out_of[section] {
                OutOf::Out(out) => {
                    ensure(self.outsecs.has(out), t, || {
                        format!("section {s}: output section out of range")
                    })?;
                    ensure(assignable, t, || {
                        format!("section {s}: assigned but not kept or without output")
                    })?;
                }
                OutOf::Consumed => {
                    ensure(row.is_kept() && row.kind.carries_output(), t, || {
                        format!("section {s}: consumed but not kept or without output")
                    })?;
                }
                OutOf::Discard => {
                    ensure(script, t, || {
                        format!("section {s}: discarded but not dropped by the script")
                    })?;
                }
                OutOf::None => {
                    ensure(!(row.is_kept() && row.kind.carries_output()), t, || {
                        format!("section {s}: kept with output but not assigned")
                    })?;
                    ensure(!script, t, || {
                        format!("section {s}: dropped by the script but not discarded")
                    })?;
                }
            }
            ensure(!self.keep[section] || row.is_kept(), t, || {
                format!("section {s}: keep on a dropped section")
            })?;
        }
        if let Some(at) = unsorted_at(&self.withdrawn, |s| *s) {
            return Err(Broken { table: t, what: format!("withdrawn[{at}] out of order") });
        }
        for &symbol in &self.withdrawn {
            ensure(
                matches!(
                    resolution.def.get(symbol),
                    Some(Def::Internal(InternalDef::Provide { conditional: true }))
                ),
                t,
                || {
                    format!(
                        "symbol {}: withdrawn but not a conditional linker-defined symbol",
                        symbol.0
                    )
                },
            )?;
        }
        Ok(())
    }
}

const _: () = assert!(size_of::<OutOf>() == 8);

//! The relocation model a unit records (`spec/meta.md` MT-26's row
//! `reloc-model`, hod's H-3; `spec/dyn.md` D-117; WP-129): in a `Shared`
//! output, an object whose units say `static` or `pie` and that defines an
//! exported preemptible datum is refused, `non-pic-in-shared-object`.
//!
//! Such code was compiled on the premise that its own definitions bind
//! inside the module (it reaches them without the GOT), so a library made
//! of it would export the datum preemptible while its code uses its own
//! instance: D-117's bound use, here exact, with no relocation or
//! instruction to find (the residual hole of D-117's analysis included).
//! `pic` never exempts an object from D-117's rules and analysis, and an
//! object without the row (every GCC and Clang object, kiln's llc route)
//! is judged by them alone: the row adds a refusal and removes none.
//!
//! Not an ODR check: it runs in every `--odr` mode of a `Shared` link
//! (`spec/meta.md` MT-42's exception for this row). Its cost: nothing in a
//! link without containers (`Resolution.meta` is empty); otherwise a walk
//! of the globals of each live object with a container, and the container
//! read (positioned reads, `crate::records::container`) only for an object
//! that defines an exported preemptible datum.

use mortar_core::{FileId, Inputs, Resolution, SymbolId};
use mortar_diag::{Diagnostic, Key, Kind, Place};
use mortar_elf::consts::SHF_EXECINSTR;
use mortar_elf::{SectionIndex, SymbolIndex, SymbolSection};
use mortar_meta::read::FileReader;

use crate::foreign::PHASE;
use crate::records::{container, seq, tag};

/// The row's name (MT-26).
pub const ROW: &[u8] = b"reloc-model";

/// The values of the row (MT-26): the relocation model code generation
/// used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Model {
    /// `-fno-pic` (hod: `static` and `dynamic-no-pic`).
    Static,
    /// `-fPIE`: the module's own definitions bind locally.
    Pie,
    /// `-fPIC`: the module's own default-visibility definitions are
    /// reached through the GOT.
    Pic,
}

impl Model {
    /// The model a row's value names; `None` for any other value (a later
    /// producer's: read as no row, D-117's rules alone).
    #[must_use]
    pub fn parse(value: &[u8]) -> Option<Self> {
        match value {
            b"static" => Some(Self::Static),
            b"pie" => Some(Self::Pie),
            b"pic" => Some(Self::Pic),
            _ => None,
        }
    }

    /// The row's value.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Static => "static",
            Self::Pie => "pie",
            Self::Pic => "pic",
        }
    }

    /// Whether the model reaches the module's own definitions without the
    /// GOT (`static`, `pie`): the premise a shared object breaks.
    #[must_use]
    pub const fn binds_locally(self) -> bool {
        matches!(self, Self::Static | Self::Pie)
    }
}

/// The models the units of `file`'s containers record, in unit order
/// (`FileId`'s sections in index order, then offset: MT-4); a unit that
/// does not parse is skipped (MT-14: additive), a value MT-26 does not
/// name is left out.
#[must_use]
pub fn models(inputs: &Inputs<'_>, file: FileId, sections: &[SectionIndex]) -> Vec<Model> {
    let read: Vec<_> = sections.iter().filter_map(|&s| container(inputs, file, s)).collect();
    let mut reader = FileReader::new();
    for (attrs, bytes) in &read {
        reader.section(*attrs, bytes);
    }
    let mut units: Vec<_> = reader.units.iter().collect();
    units.sort_by_key(|u| (u.section, u.offset));
    units.iter().filter_map(|u| u.summary_value(ROW).and_then(|(v, _)| Model::parse(v))).collect()
}

/// The exported preemptible data definitions of live object `file`
/// (`spec/dyn.md` D-117's "preemptible data definition", as
/// `mortar_scan`'s address-level rule reads it): its non-local symbols
/// defined in a section without `SHF_EXECINSTR` (`STT_TLS` included) or
/// common, whose key `Resolution.preemptible` holds; in symbol order.
#[must_use]
pub fn preemptible_data(
    inputs: &Inputs<'_>,
    resolution: &Resolution,
    file: FileId,
) -> Vec<(SymbolIndex, SymbolId)> {
    let mut out = Vec::new();
    if resolution.preemptible.is_empty() {
        return out;
    }
    let Some(mortar_core::inputs::Reader::Object(obj)) = inputs.files.get(file).map(|f| &f.reader)
    else {
        return out;
    };
    let Ok(Some(table)) = obj.symbols() else { return out };
    let keys = inputs.sym_map_of(file);
    let first = table.first_global().0;
    for (index, sym) in table.globals() {
        let Some(&key) = index.0.checked_sub(first).and_then(|i| keys.get(i as usize)) else {
            continue;
        };
        if !resolution.preemptible.contains(key) {
            continue;
        }
        let data = match table.section_of(index, &sym) {
            Ok(SymbolSection::Section(t)) => {
                obj.section(t).is_ok_and(|s| s.flags() & SHF_EXECINSTR == 0)
            }
            Ok(SymbolSection::Common) => true,
            _ => false,
        };
        if data {
            out.push((index, key));
        }
    }
    out
}

/// The rule (D-117, MT-26's row): one `non-pic-in-shared-object` error per
/// live object of a `Shared` output whose units record `static` or `pie`
/// and that defines an exported preemptible datum, naming the object, the
/// first such datum (in symbol order; a note counts the others) and the
/// row's value (the first unit's that binds locally). Sorted by key.
#[must_use]
pub fn check(inputs: &Inputs<'_>, resolution: &Resolution) -> Vec<(Key, Diagnostic)> {
    let meta: &[(FileId, SectionIndex)] = &resolution.meta;
    let mut out = Vec::new();
    if meta.is_empty() || resolution.preemptible.is_empty() {
        return out;
    }
    let mut start = 0;
    while start < meta.len() {
        let file = meta[start].0;
        let end = meta[start..].iter().position(|m| m.0 != file).map_or(meta.len(), |p| start + p);
        let sections: Vec<SectionIndex> = meta[start..end].iter().map(|m| m.1).collect();
        start = end;
        let defs = preemptible_data(inputs, resolution, file);
        let Some(&(_, first)) = defs.first() else { continue };
        let Some(model) = models(inputs, file, &sections).into_iter().find(|m| m.binds_locally())
        else {
            continue;
        };
        out.push((key(file), diagnostic(inputs, file, first, defs.len() - 1, model)));
    }
    out
}

/// The error's key: phase 40 (after resolution), the object, the tag of
/// this check.
fn key(file: FileId) -> Key {
    Key::new(PHASE, file.0, seq(tag::RELOC_MODEL, 0))
}

fn diagnostic(
    inputs: &Inputs<'_>,
    file: FileId,
    def: SymbolId,
    others: usize,
    model: Model,
) -> Diagnostic {
    let fref = inputs.files[file].display();
    let name = inputs
        .symbols
        .get(def)
        .map_or_else(String::new, |k| String::from_utf8_lossy(k.name).into_owned());
    let shown = match fref.member() {
        Some(m) => format!("{}({m})", fref.path()),
        None => fref.path().to_owned(),
    };
    let mut d = Diagnostic::error(Kind::NonPicInSharedObject {
        r_type: String::new(),
        name: name.clone(),
        file: shown.clone(),
    })
    .at(Place::symbol(fref, name.clone()))
    .note(format!(
        "the Mason metadata of {shown} records reloc-model={} (spec/meta.md MT-26): its code \
         reaches its own definitions without the GOT, so the library would bind `{name}' inside \
         itself while exporting it preemptible (spec/dyn.md D-117)",
        model.word()
    ));
    if others > 0 {
        let s = if others == 1 { "" } else { "s" };
        d = d.note(format!(
            "{shown} defines {others} other exported preemptible data definition{s}"
        ));
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_values_and_no_other() {
        for (word, model, local) in
            [("static", Model::Static, true), ("pie", Model::Pie, true), ("pic", Model::Pic, false)]
        {
            assert_eq!(Model::parse(word.as_bytes()), Some(model));
            assert_eq!(model.word(), word);
            assert_eq!(model.binds_locally(), local);
        }
        // A value MT-26 does not name is no row: D-117's rules alone.
        for other in [&b"PIC"[..], b"dynamic-no-pic", b"ropi", b"", b"pic\0"] {
            assert_eq!(Model::parse(other), None);
        }
    }

    #[test]
    fn its_key_is_odr_phase_and_its_own_tag() {
        let a = key(FileId(3));
        assert_eq!(a, Key::new(PHASE, 3, seq(tag::RELOC_MODEL, 0)));
        assert!(key(FileId(2)) < a);
    }
}

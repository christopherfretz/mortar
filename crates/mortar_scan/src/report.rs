//! The diagnostic of each group (`spec/image.md` I-22): its key, head,
//! `>>> referenced by` lines and the extra context of its kind.

use mortar_core::resolution::Verdict;
use mortar_core::{FileId, SymKey, SymKind};
use mortar_diag::{Diagnostic, FileRef, Key, Kind, Place, Relation};
use mortar_elf::consts::{STT_SECTION, STV_HIDDEN, STV_INTERNAL};
use mortar_elf::{SectionIndex, SymbolIndex, SymbolSection};

use crate::events::{Acc, EventClass, GroupKey, RefId, group_sym};
use crate::view::{View, contents, rec_offset};

/// `{path}` or `{archive}({member})`, as a note names a file.
fn file_text(f: &FileRef) -> String {
    match f.member() {
        Some(m) => format!("{}({m})", f.path()),
        None => f.path().to_owned(),
    }
}

/// The place of a reference, `file:(section+0xoff)`, `r_offset` read from
/// the record (I-22): resident, or one `copy_to` of the record. The
/// record was read by the walk that made the reference, so a read that
/// fails now is a file that changed; the place then says offset 0 and the
/// image phase's `recheck` reports the change (`spec/input.md` §11.5).
fn ref_place(view: &View<'_, '_>, r: RefId) -> Place {
    let path = view.inputs.files[r.file].display();
    let section = view.section_name(r.file, r.section);
    let off = (|| {
        let sid = view.section_of(r.file, r.section)?;
        let rel = view.resolution.sections.get(sid)?.reloc;
        let obj = view.prep(r.file)?.obj?;
        rec_offset(view, r.file, &obj, rel, r.record).ok()?
    })()
    .unwrap_or(0);
    Place::section(path, section, off)
}

fn with_refs(view: &View<'_, '_>, mut d: Diagnostic, acc: &Acc) -> Diagnostic {
    for &r in acc.firsts() {
        d = d.with(Relation::ReferencedBy, ref_place(view, r));
    }
    if acc.count > 3 {
        d = d.note(format!("referenced {} more times", acc.count - 3));
    }
    d
}

/// The diagnostic of one group, keyed by its first reference.
pub(crate) fn group(view: &View<'_, '_>, g: &GroupKey, acc: &Acc) -> (Key, Diagnostic) {
    let first = acc.first[0];
    let key = mortar_arch::diag::undefined_key(first.file, first.section, first.record);
    let d = match g.class {
        EventClass::Undefined => undefined(view, group_sym(g.group), acc),
        EventClass::ComdatMismatch => mismatch(view, group_sym(g.group), acc),
        EventClass::DiscardedSection => discarded(view, g.file, g.group, acc),
        EventClass::GnuWarning => gnu_warning(view, group_sym(g.group), acc),
    };
    (key, d)
}

/// `undefined-symbol` (`spec/resolve.md` §5.3): an error, or a warning
/// under `--warn-unresolved-symbols`; a hidden key's is always an error,
/// with the note.
fn undefined(view: &View<'_, '_>, key: SymKey, acc: &Acc) -> Diagnostic {
    let kind = Kind::UndefinedSymbol { name: view.shown_name(key) };
    let (verdict, hidden) = match key.kind() {
        SymKind::Global(id) => {
            let p = view.resolution.props.get(id).copied().unwrap_or_default();
            (p.verdict, matches!(p.visibility, STV_HIDDEN | STV_INTERNAL))
        }
        SymKind::Local(_) => (Verdict::Error, false),
    };
    let d = if verdict == Verdict::Warning && !hidden {
        Diagnostic::warning(kind)
    } else {
        Diagnostic::error(kind)
    };
    let mut d = with_refs(view, d, acc);
    // D-12: a definition only a closure file has is not a candidate.
    if let Some(id) = key.as_global() {
        let c = view.closure_only;
        if let Some(&(_, file)) = c.get(c.partition_point(|row| row.0 < id)).filter(|r| r.0 == id) {
            let path = file_text(&view.inputs.files[file].display());
            d = d.note(format!("defined in {path}, which is not on the command line"));
        }
    }
    if hidden { d.note("the symbol has hidden visibility") } else { d }
}

/// `comdat-mismatch` (`spec/resolve.md` §6.3 (b)).
fn mismatch(view: &View<'_, '_>, key: SymKey, acc: &Acc) -> Diagnostic {
    let name = view.shown_name(key);
    let m = key.as_global().and_then(|id| {
        let at = view.mismatches.partition_point(|m| m.key < id);
        view.mismatches.get(at).filter(|m| m.key == id)
    });
    let Some(m) = m else {
        // A key the scan reports as a mismatch is one of these.
        return with_refs(view, Diagnostic::error(Kind::UndefinedSymbol { name }), acc);
    };
    let signature = view
        .inputs
        .comdats
        .get(m.group)
        .map_or_else(String::new, |c| String::from_utf8_lossy(c.name).into_owned());
    let kept = file_text(&view.inputs.files[m.keeper].display());
    let dropped = file_text(&view.inputs.files[m.dropped.file].display());
    let d = Diagnostic::error(Kind::ComdatMismatch { name, signature: signature.clone() })
        .note(format!("kept copy: {kept}"))
        .note(format!("discarded copy defining it: {dropped}"));
    with_refs(view, d, acc).note(format!(
        "the copies of {signature} differ: a one-definition-rule violation, or objects compiled with different options"
    ))
}

/// `discarded-section-reference` (`spec/resolve.md` §6.3 (a)): one per
/// (symbol, referring section).
fn discarded(view: &View<'_, '_>, file: FileId, group: u64, acc: &Acc) -> Diagnostic {
    let r_sym = group as u32;
    let path = view.inputs.files[file].display();
    let sym = view.sym(file, SymbolIndex(r_sym));
    let table = view.prep(file).and_then(|p| p.table().copied());
    let section = sym.and_then(|s| match table?.section_of(SymbolIndex(r_sym), &s).ok()? {
        SymbolSection::Section(i) => Some(i),
        _ => None,
    });
    let section_name =
        section.map_or_else(String::new, |i: SectionIndex| view.section_name(file, i.0));
    let name = match sym {
        Some(s) if s.sym_type() == STT_SECTION => section_name.clone(),
        _ => view
            .shown_name(SymKey::local(mortar_core::SymEntry { file, index: SymbolIndex(r_sym) })),
    };
    let d = Diagnostic::error(Kind::DiscardedSectionReference { name })
        .with(Relation::DefinedIn, Place::symbol(path, section_name));
    with_refs(view, d, acc)
}

/// `gnu-warning` per key (`spec/resolve.md` §10): the text of the first
/// `.gnu.warning.<key>` section of a live file, up to its first NUL.
fn gnu_warning(view: &View<'_, '_>, key: SymKey, acc: &Acc) -> Diagnostic {
    let id = key.as_global();
    let at = view.gnu_warnings.partition_point(|w| Some(w.0) < id);
    let mut buf = Vec::new();
    let mut text = String::new();
    if let Some(w) = view.gnu_warnings.get(at).filter(|w| Some(w.0) == id)
        && let Some(obj) = view.prep(w.1).and_then(|p| p.obj)
    {
        // `.gnu.warning*` is resident (`spec/input.md` §11.1); a read
        // that fails is its own diagnostic, never an empty warning.
        let data = obj
            .section(w.2)
            .map_err(|e| crate::view::ReadFail::Malformed(e.to_string()))
            .and_then(|s| contents(view, w.1, &s, &mut buf));
        match data {
            Ok(data) => {
                let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
                text = String::from_utf8_lossy(&data[..end]).into_owned();
            }
            Err(f) => {
                let path = view.inputs.files[w.1].display();
                let place = Place::section(path.clone(), view.section_name(w.1, w.2.0), 0);
                return f.diagnostic(&path).at(place);
            }
        }
    }
    with_refs(view, Diagnostic::warning(Kind::GnuWarning { text }), acc)
}

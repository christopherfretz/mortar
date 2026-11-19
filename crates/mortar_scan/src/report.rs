//! The diagnostic of each group (`spec/image.md` I-22): its key, head,
//! `>>> referenced by` lines and the extra context of its kind.

use mortar_core::resolution::{Def, DropReason, SectionState, Verdict};
use mortar_core::{FileId, SectionId, SymKey, SymKind};
use mortar_diag::{Diagnostic, FileRef, Key, Kind, Place, Relation};
use mortar_elf::consts::{STT_FILE, STT_FUNC, STT_SECTION, STV_HIDDEN, STV_INTERNAL};
use mortar_elf::{SectionIndex, SymbolIndex, SymbolSection};

use crate::EhOwner;
use crate::events::{Acc, EventClass, GroupKey, RefId, group_sym};
use crate::view::{EhPlace, View, contents, rec_offset};

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
    let path = view.whole.inputs.files[r.file].display();
    let section = view.section_name(r.file, r.section);
    Place::section(path, section, ref_offset(view, r))
}

/// `r_offset` of the record of reference `r` ([`ref_place`]'s).
fn ref_offset(view: &View<'_, '_>, r: RefId) -> u64 {
    (|| {
        let sid = view.section_of(r.file, r.section)?;
        let rel = view.whole.resolution.sections.get(sid)?.reloc;
        let obj = view.prep(r.file)?.obj?;
        rec_offset(view, r.file, &obj, rel, r.record).ok()?
    })()
    .unwrap_or(0)
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
            let p = view.whole.resolution.props.get(id).copied().unwrap_or_default();
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
            let path = file_text(&view.whole.inputs.files[file].display());
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
        .whole
        .inputs
        .comdats
        .get(m.group)
        .map_or_else(String::new, |c| String::from_utf8_lossy(c.name).into_owned());
    let kept = file_text(&view.whole.inputs.files[m.keeper].display());
    let dropped = file_text(&view.whole.inputs.files[m.dropped.file].display());
    let raw_name = view.name(key);
    let d = Diagnostic::error(Kind::ComdatMismatch { name, signature: signature.clone() })
        .note(format!("kept copy: {kept}"))
        .note(format!("discarded copy defining it: {dropped}"));
    let mut d = eh_note(view, with_refs(view, d, acc), acc.first[0]);
    // WP-177: the kept copy defines the name, but as a local symbol.
    if defines(view, m.keeper, raw_name, true) {
        d = d.note(format!("{kept} defines {} as a local symbol: {PRELINK}", view.shown_name(key)));
        return d;
    }
    d.note(format!(
        "the copies of {signature} differ: a one-definition-rule violation, or objects compiled with different options"
    ))
}

/// WP-177's note on a symbol of a COMDAT group made local after `ld -r`.
const PRELINK: &str = "it was probably made local after a partial link (objcopy \
    --localize-hidden, --keep-global-symbol); dissolve the groups first (ld -r \
    --force-group-allocation), then localize";

/// Whether `file` defines a symbol named `name` in a section: a local one
/// (`local`), or a non-local one.
fn defines(view: &View<'_, '_>, file: FileId, name: &[u8], local: bool) -> bool {
    let Some(t) = view.prep(file).and_then(|p| p.table().copied()) else { return false };
    let first = t.first_global().0;
    let (lo, hi) = if local { (1, first) } else { (first, t.len()) };
    (lo..hi).any(|i| {
        t.get(SymbolIndex(i)).is_ok_and(|s| {
            s.sym_type() != STT_SECTION
                && matches!(t.section_of(SymbolIndex(i), &s), Ok(SymbolSection::Section(_)))
                && t.name(&s).is_ok_and(|n| n == name)
        })
    })
}

/// WP-177 (`spec/resolve.md` §6.3): for a reference from `.eh_frame` or
/// `.gcc_except_table`, the record that owns its field, as a note: the
/// function whose unwind data it is, when known.
fn eh_note(view: &View<'_, '_>, d: Diagnostic, r: RefId) -> Diagnostic {
    let name = view.section_name(r.file, r.section);
    let Some(place) = EhPlace::of(name.as_bytes()) else { return d };
    let Some(sid) = view.section_of(r.file, r.section) else { return d };
    let off = ref_offset(view, r);
    let what = match place {
        EhPlace::Frame => "unwind information",
        EhPlace::ExceptTable => "exception table (LSDA)",
    };
    match view.eh_owner(place, sid, off) {
        Some(EhOwner::Live { code }) => {
            let f = code.map_or_else(|| "a live function".to_owned(), |c| function_at(view, c));
            d.note(format!("the field is in the {what} of {f}, which is written"))
        }
        Some(EhOwner::None) => d.note(match place {
            EhPlace::ExceptTable => {
                "no FDE has an LSDA in this section at or before this \
                                     place: the field's owner is not known"
            }
            EhPlace::Frame => "the field is in no record of this section: its owner is not known",
        }),
        Some(EhOwner::Dead) | None => d,
    }
}

/// The function at `offset` of section `sid`: a symbol of its file
/// defined there (a function first), else `section+0xoff`.
fn function_at(view: &View<'_, '_>, (sid, offset): (SectionId, u64)) -> String {
    let Some((file, index)) = view.whole.resolution.sh_index(sid) else { return String::new() };
    let at = || format!("{}+{offset:#x}", view.section_name(file, index.0));
    let Some(t) = view.prep(file).and_then(|p| p.table().copied()) else { return at() };
    let mut best: Option<(bool, SymbolIndex)> = None;
    for i in 1..t.len() {
        let Ok(s) = t.get(SymbolIndex(i)) else { continue };
        if s.st_value != offset
            || matches!(s.sym_type(), STT_SECTION | STT_FILE)
            || t.section_of(SymbolIndex(i), &s).ok() != Some(SymbolSection::Section(index))
        {
            continue;
        }
        let func = s.sym_type() == STT_FUNC;
        if best.is_none_or(|b| func && !b.0) {
            best = Some((func, SymbolIndex(i)));
        }
    }
    match best {
        Some((_, i)) => {
            let key = if i.0 < t.first_global().0 {
                SymKey::local(mortar_core::SymEntry { file, index: i })
            } else {
                return t
                    .get(i)
                    .ok()
                    .and_then(|s| t.name(&s).ok())
                    .map_or_else(at, |n| String::from_utf8_lossy(n).into_owned());
            };
            view.shown_name(key)
        }
        None => at(),
    }
}

/// `discarded-section-reference` (`spec/resolve.md` §6.3 (a)): one per
/// (symbol, referring section).
fn discarded(view: &View<'_, '_>, file: FileId, group: u64, acc: &Acc) -> Diagnostic {
    let r_sym = group as u32;
    let table = view.prep(file).and_then(|p| p.table().copied());
    // A global whose definition has no place (resolve.md §10.1 P-1,
    // WP-136): named as the key, defined in its definition's file and
    // section, which need not be the referring file.
    if let Some(key) = view.key_of(file, table.as_ref(), r_sym)
        && let SymKind::Global(id) = key.kind()
        && let Def::Defined(e) = view.def(id)
    {
        let path = view.whole.inputs.files[e.file].display();
        let dtable = view.prep(e.file).and_then(|p| p.table().copied());
        let section =
            view.sym(e.file, e.index).and_then(|s| match dtable?.section_of(e.index, &s).ok()? {
                SymbolSection::Section(i) => Some(view.section_name(e.file, i.0)),
                _ => None,
            });
        let name = view.shown_name(key);
        let d = Diagnostic::error(Kind::DiscardedSectionReference { name })
            .with(Relation::DefinedIn, Place::symbol(path, section.unwrap_or_default()));
        return with_refs(view, d, acc);
    }
    let path = view.whole.inputs.files[file].display();
    let sym = view.sym(file, SymbolIndex(r_sym));
    let section = sym.and_then(|s| match table?.section_of(SymbolIndex(r_sym), &s).ok()? {
        SymbolSection::Section(i) => Some(i),
        _ => None,
    });
    let section_name =
        section.map_or_else(String::new, |i: SectionIndex| view.section_name(file, i.0));
    let is_section = matches!(sym, Some(s) if s.sym_type() == STT_SECTION);
    let name = if is_section {
        section_name.clone()
    } else {
        view.shown_name(SymKey::local(mortar_core::SymEntry { file, index: SymbolIndex(r_sym) }))
    };
    let mut d = Diagnostic::error(Kind::DiscardedSectionReference { name: name.clone() })
        .with(Relation::DefinedIn, Place::symbol(path, section_name));
    // WP-177: the group and its kept copy, when COMDAT dropped the
    // section; the record that owns the field in `.eh_frame` and
    // `.gcc_except_table`; the note on a symbol made local after `ld -r`.
    let group = section
        .and_then(|i| view.section_of(file, i.0))
        .and_then(|sid| view.whole.resolution.sections.get(sid))
        .filter(|row| row.state == SectionState::Dropped(DropReason::Comdat))
        .and_then(|row| row.comdat.get());
    let mut prelink = false;
    if let Some(g) = group {
        let signature = view.whole.inputs.comdats.get(g).map_or(&[][..], |c| c.name);
        let keeper = view.whole.resolution.comdat.get(g).and_then(|k| k.get());
        d = d.note(format!(
            "the section is in a discarded copy of COMDAT group {}",
            String::from_utf8_lossy(signature)
        ));
        if let Some(k) = keeper {
            d = d.note(format!("kept copy: {}", file_text(&view.whole.inputs.files[k].display())));
            let raw = name.as_bytes();
            prelink = !is_section && (raw == signature || defines(view, k, raw, false));
        }
    }
    let d = eh_note(view, with_refs(view, d, acc), acc.first[0]);
    if prelink { d.note(format!("{name} is local in its file: {PRELINK}")) } else { d }
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
                let path = view.whole.inputs.files[w.1].display();
                let place = Place::section(path.clone(), view.section_name(w.1, w.2.0), 0);
                return f.diagnostic(&path).at(place);
            }
        }
    }
    with_refs(view, Diagnostic::warning(Kind::GnuWarning { text }), acc)
}

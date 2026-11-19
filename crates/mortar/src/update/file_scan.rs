//! WP-255d5: the scan of ONE changed file for the lazy `Link` (the record
//! `spec/landed/incr/WP-255d5.md`; rulings D4-1..D4-6 and "After 255d5's
//! second hand-back" in `briefs/wp-255-rulings.md`).
//!
//! [`scan_of`] runs the full link's own walk (`mortar_scan::scan_file`)
//! over a narrow [`Whole`] of the file: its own bytes, its section facts
//! (`file_split::changed_facts`), the update's liveness (`LiveDelta`) and
//! resolution (`ResolveDelta`) laid over the stored rows. Every answer of
//! the narrow view has ONE source:
//!
//! - the file's reader, symbol table, bytes, display: the [`FileFront`];
//! - per section index: the id from the front; kind, kept, output section
//!   and relocation section from its [`SectionFact`]; live from
//!   `LiveDelta::now_live` (by its new id) under `--gc-sections`, else
//!   kept (I-1); a place when kept, live and assigned (P-1);
//! - the file's keys: `ResolveDelta::key_ids` through the file's
//!   `ChangedFile::mentions` (entry `first_global + k`);
//! - a key's facts: for a key whose new `RESOLVED` row is a definition in
//!   a section of the changed file (`defined_row`'s `ment.new` branch),
//!   `mortar_scan::global_facts` over this view (ruling item 4: `def` the
//!   file's entry, `preemptible` false outside `Shared`, item 3); for a
//!   key whose delta row is the stored row, the stored `RESOLVED.facts`
//!   with `dead` recomputed from its stored section and the liveness
//!   delta. Anything else falls back `(scan) facts` by name.
//!
//! A read the view cannot answer exactly is never answered with a default:
//! it records why, and [`scan_of`] falls back with that reason. A plain
//! link never calls this module.

use std::sync::{Mutex, OnceLock};

use mortar_arch::{Arch, OutputKind, SymFacts, TargetKind};
use mortar_core::incr::{FileDemands, IncrReason, OptionKeys, Unimplemented};
use mortar_core::inputs::{CopyError, Reader};
use mortar_core::resolution::{Assigned, Def, SectionKind, Verdict};
use mortar_core::split::EhOwner;
use mortar_core::{FileId, SectionId, SymEntry, SymbolId};
use mortar_diag::FileRef;
use mortar_elf::{Machine, SectionIndex, SymbolIndex, SymbolSection};
use mortar_incr::format::{def_kind, kind, section_bits};
use mortar_incr::update::live::LiveDelta;
use mortar_incr::update::resolve::ResolveDelta;
use mortar_incr::update::rows::{ResolvedRow, sym_kind};
use mortar_incr::update::view::{ChangedFiles, StateView};
use mortar_merge::eh::{EhFrames, EhReloc, LsdaStarts, RelocClass, Target};
use mortar_scan::{Prep, Program, ScanOptions, Whole};

use super::file_front::FileFront;
use super::file_split::{FileSplit, SectionFact};

/// A fall-back and its detail (`live::live_why`'s form): the reason, and
/// the case word with what made it.
pub type Fail = (IncrReason, String);

fn fail(word: &str, detail: impl std::fmt::Display) -> Fail {
    (IncrReason::UpdateNotImplemented(Some(Unimplemented::Scan)), format!("{word}: {detail}"))
}

/// A row read's failure, its reason kept.
fn rows(e: IncrReason) -> Fail {
    let s = e.report();
    (e, s)
}

fn corrupt(what: impl std::fmt::Display) -> Fail {
    let s = format!("scan_of: {what}");
    (IncrReason::StateCorrupt(s.clone()), s)
}

/// [`scan_of_why`] with the detail dropped.
///
/// # Errors
/// As [`scan_of_why`].
#[allow(clippy::too_many_arguments, reason = "the update's inputs, each read once")]
pub fn scan_of(
    arch: &dyn Arch,
    opts: ScanOptions<'_>,
    front: &FileFront<'_>,
    facts: Option<&[SectionFact]>,
    split: &FileSplit,
    view: &StateView<'_>,
    changed: &ChangedFiles,
    options: &OptionKeys,
    resolved: &ResolveDelta,
    live: &LiveDelta,
) -> Result<FileDemands, IncrReason> {
    scan_of_why(arch, opts, front, facts, split, view, changed, options, resolved, live)
        .map_err(|e| e.0)
}

/// The slot demand of CHANGED file `front` (`FileDemands`, IN-72 item 7),
/// by the full link's walk over the narrow view (the module doc), its keys
/// in the update's id space and its locals `(front.file, index)`.
///
/// `facts`: the file's `changed_facts` (one per section index); `None` for
/// an `Ids::Stored` (drawn-in) file, which falls back `stored-reloc` (the
/// state holds no relocation section per section; ruling item 5). `split`:
/// the file's `split_of` (its `.eh_frame` records, for WP-177's owners).
/// `options`: what the update reads of the plan; `opts` the link's scan
/// options.
///
/// # Errors
/// `update-not-implemented (scan)`, the case word first in the detail:
/// - `wrap`: a `--wrap` link (D4-5);
/// - `stored-reloc`: `facts` is `None` (ruling item 5);
/// - `weak-dynamic`: a dynamic executable or PIE in which a key the
///   file's old or new version names, or an affected key, is undefined in
///   its stored or new row (D4-1: D-25's per-key classes are whole-program);
/// - `lsda-cross-file`: an FDE of the file whose LSDA is not a section of
///   the file, or a global defined in a `.gcc_except_table` section of it
///   (D4-4);
/// - `facts`: a key fact the view cannot give exactly (the reason in the
///   detail: a changed row it does not produce, an undefined or
///   linker-defined key, a `.gnu.warning` it cannot rule out, a read of
///   another file), a `Shared` output, a versioned name;
/// - `diagnostic`: the walk gave a diagnostic, an event or an overlap
///   record (F-2: an update carries none).
///
/// `state-corrupt` for rows that do not read back or disagree with the
/// file.
#[allow(clippy::too_many_lines, reason = "the refusals, then the view, in the doc's order")]
#[allow(clippy::too_many_arguments, reason = "the update's inputs, each read once")]
pub fn scan_of_why(
    arch: &dyn Arch,
    opts: ScanOptions<'_>,
    front: &FileFront<'_>,
    facts: Option<&[SectionFact]>,
    split: &FileSplit,
    view: &StateView<'_>,
    changed: &ChangedFiles,
    options: &OptionKeys,
    resolved: &ResolveDelta,
    live: &LiveDelta,
) -> Result<FileDemands, Fail> {
    if !options.wrap.is_empty() {
        return Err(fail("wrap", "the link has --wrap"));
    }
    let Some(facts) = facts else {
        return Err(fail("stored-reloc", "an Ids::Stored file: no stored relocation section"));
    };
    if opts.output == OutputKind::Shared {
        return Err(fail("facts", "a shared output (preemptible keys)"));
    }
    if front.versioned != 0 {
        return Err(fail("facts", "a versioned name"));
    }
    let object = &front.object;
    let count = object.section_count();
    if facts.len() != count as usize || front.sections.len() != count as usize {
        return Err(corrupt("the facts or the front's sections are not one per section index"));
    }
    let Some(first) = front.sections.first().map(|s| s.id.0) else {
        return Err(corrupt("a changed file without sections"));
    };
    let mine = |s: u32| s >= first && s - first < count;
    // The file's keys: `ChangedFile::mentions[k]` is entry
    // `first_global + k`, its `symbol` a position in `key_ids`.
    let c = changed
        .files
        .iter()
        .find(|c| c.file == front.file)
        .ok_or_else(|| fail("facts", "the file is not a changed file of the update"))?;
    let ids = resolved
        .key_ids
        .iter()
        .find(|k| k.0 == front.file)
        .map(|k| &k.1)
        .ok_or_else(|| corrupt("no key ids for the changed file"))?;
    if c.first_global != front.first_global || c.mentions.len() != front.keys.len() {
        return Err(corrupt("the changed file's mentions are not the front's entries"));
    }
    let mut globals = Vec::with_capacity(front.keys.len());
    for (k, m) in c.mentions.iter().enumerate() {
        let id = *ids.get(m.symbol as usize).ok_or_else(|| corrupt("a mention's key"))?;
        if front.keys[k].key.is_some_and(|s| s != id) {
            return Err(corrupt(format!("entry {k}: the front's key is not the delta's")));
        }
        globals.push(id);
    }
    // D4-1.
    if matches!(opts.output, OutputKind::Exec | OutputKind::Pie) {
        weak_dynamic(view, front.file, &globals, resolved)?;
    }
    // A `.gnu.warning` section of a changed file's new version (an
    // appeared or newly extracted file included) can make a key warned
    // that was not: the stored `gnu_warning` bits are then not the new
    // ones.
    for f in &changed.files {
        for s in &f.sections {
            let name = changed.string(view, s.name).map_err(rows)?;
            if name.starts_with(b".gnu.warning") {
                return Err(fail("facts", "a .gnu.warning section in a changed file"));
            }
        }
    }
    let secs: Vec<Sec> = facts
        .iter()
        .zip(&front.sections)
        .map(|(f, s)| {
            let live = if options.gc_sections {
                live.now_live.binary_search(&s.id).is_ok()
            } else {
                f.kept
            };
            Sec {
                id: s.id,
                reloc: f.reloc.0,
                kind: f.kind,
                live,
                place: f.kept && live && f.out.is_some(),
            }
        })
        .collect();
    let path = view.string(front.row.path).map_err(rows)?;
    let member = view.string(front.row.member_name).map_err(rows)?;
    let path = String::from_utf8_lossy(&path).into_owned();
    let display = if member.is_empty() {
        FileRef::new(path)
    } else {
        FileRef::in_archive(path, String::from_utf8_lossy(&member).into_owned())
    };
    let mut narrow = Narrow {
        front,
        display,
        machine: object.machine(),
        output: opts.output,
        prep: OnceLock::new(),
        secs,
        first,
        globals,
        defs: Vec::new(),
        keys: Vec::new(),
        asked: Mutex::new(None),
    };
    lsda_own(&narrow, split, &mine)?;
    // Per key of the file, its answer (or why there is none: asked only
    // when a record names the key).
    let symbols = view.count(kind::SYMBOLS);
    let mut keys: Vec<SymbolId> = narrow.globals.clone();
    keys.sort_unstable();
    keys.dedup();
    let mut pending = Vec::new();
    let mut answers: Vec<(SymbolId, Result<KeyAns, Fail>)> = Vec::with_capacity(keys.len());
    for &id in &keys {
        let delta = resolved
            .resolved
            .binary_search_by_key(&id, |r| r.0)
            .ok()
            .map(|i| resolved.resolved[i].1);
        let stored = if u64::from(id.0) < symbols {
            let s = view.symbol(id).map_err(rows)?;
            let r = view.resolved(id).map_err(rows)?;
            Some((s.kind == sym_kind::GONE, r))
        } else {
            None
        };
        let row = match (delta, stored) {
            (Some(d), _) => d,
            (None, Some((_, s))) => s,
            (None, None) => return Err(corrupt(format!("key {}: no row", id.0))),
        };
        if row.kind == def_kind::SECTION && mine(row.section) {
            // Ruling item 4: defined in the changed file.
            let index = row.section - first;
            let entry = narrow.defining_entry(id, index).ok_or_else(|| {
                corrupt(format!("key {}: no entry of the file defines it in section {index}", id.0))
            })?;
            narrow.defs.push((id, entry));
            pending.push((id, stored));
            continue;
        }
        let a = match stored {
            Some((false, s)) if delta.is_none_or(|d| d == s) => {
                stored_answer(view, options, live, id, s)
            }
            _ => Err(fail("facts", format!("key {}: its row changed (F-1)", id.0))),
        };
        answers.push((id, a));
    }
    narrow.defs.sort_unstable_by_key(|d| d.0);
    for (id, stored) in pending {
        let a = defined_answer(&narrow, id, stored);
        answers.push((id, a));
    }
    answers.sort_unstable_by_key(|a| a.0);
    narrow.keys = answers;
    let eh = NarrowEh::new(&narrow, split);
    let sections: Vec<SectionId> = front.sections.iter().map(|s| s.id).collect();
    let (demands, diags, problems) =
        mortar_scan::scan_file(arch, &narrow, opts, Some(&eh), front.file, sections.into_iter());
    if let Some(why) = narrow.why().or_else(|| eh.why()) {
        return Err(why);
    }
    if let Some((_, d)) = diags.first() {
        return Err(fail(
            "diagnostic",
            format!("{} diagnostics, the first {}", diags.len(), d.id()),
        ));
    }
    if problems > 0 {
        return Err(fail("diagnostic", format!("{problems} events or overlap records")));
    }
    Ok(demands)
}

/// D4-1: in a dynamic executable or PIE, a key undefined in its stored or
/// new row among the file's new keys, its stored mentions and the
/// affected keys.
fn weak_dynamic(
    view: &StateView<'_>,
    file: FileId,
    globals: &[SymbolId],
    resolved: &ResolveDelta,
) -> Result<(), Fail> {
    let symbols = view.count(kind::SYMBOLS);
    let old = if u64::from(file.0) < view.count(kind::FILES) {
        view.mentions_of(file).map_err(rows)?
    } else {
        Vec::new()
    };
    let keys = globals
        .iter()
        .copied()
        .chain(old.iter().map(|m| SymbolId(m.symbol)))
        .chain(resolved.affected.iter().copied());
    for id in keys {
        let delta = resolved.resolved.binary_search_by_key(&id, |r| r.0).ok();
        let undefined_now =
            delta.is_some_and(|i| resolved.resolved[i].1.kind == def_kind::UNDEFINED);
        let undefined_before = u64::from(id.0) < symbols
            && view.resolved(id).map_err(rows)?.kind == def_kind::UNDEFINED;
        if undefined_now || undefined_before {
            return Err(fail(
                "weak-dynamic",
                format!("key {} is undefined in a dynamic output", id.0),
            ));
        }
    }
    Ok(())
}

/// D4-4: every LSDA of the file's FDEs is a section of the file, and no
/// global of the file is defined in a `.gcc_except_table` section (another
/// file's FDE could then name it, and its owner is a whole-program fact).
fn lsda_own(n: &Narrow<'_, '_>, split: &FileSplit, mine: &dyn Fn(u32) -> bool) -> Result<(), Fail> {
    let frames = &split.eh;
    for row in 0..frames.sections.len() {
        let rel = split.eh_relocs.get(row).map_or(&[][..], Vec::as_slice);
        for rec in frames.records_of(row) {
            let Some(p) = rec.fde().and_then(|f| f.lsda) else { continue };
            let at = u64::from(rec.offset) + u64::from(p.at);
            let Some(r) = rel.iter().find(|r| r.offset == at && r.class != RelocClass::None) else {
                continue;
            };
            match r.target {
                Target::Section { section, .. } if mine(section.0) => {}
                _ => return Err(fail("lsda-cross-file", format!("an FDE's LSDA at {at}"))),
            }
        }
    }
    let o = &n.front.object;
    let Some(table) = n.prep().table() else { return Ok(()) };
    for k in 0..n.front.keys.len() as u32 {
        let index = SymbolIndex(n.front.first_global + k);
        let Ok(s) = table.get(index) else { continue };
        let Ok(SymbolSection::Section(i)) = table.section_of(index, &s) else { continue };
        let name = o.section(i).ok().and_then(|x| x.name().ok()).unwrap_or_default();
        if name.starts_with(b".gcc_except_table") {
            return Err(fail("lsda-cross-file", "a global defined in a .gcc_except_table section"));
        }
    }
    Ok(())
}

/// The answer for a key defined in a section of the changed file (ruling
/// item 4): `global_facts` over the narrow view; `comdat_only` false (a
/// live definition); `gnu_warning` the stored bit when false (a `.gnu.warning`
/// section of a changed file was refused before), else, and for a key the
/// old link did not have (new or `GONE`: an unchanged file's
/// `.gnu.warning.<key>` was then ignored, resolve's `find_key`), a
/// fall-back.
fn defined_answer(
    n: &Narrow<'_, '_>,
    id: SymbolId,
    stored: Option<(bool, ResolvedRow)>,
) -> Result<KeyAns, Fail> {
    let Some((false, s)) = stored else {
        return Err(fail("facts", format!("key {}: a new key's .gnu.warning is unknown", id.0)));
    };
    let old = SymFacts::unpack(s.facts).ok_or_else(|| corrupt("a RESOLVED.facts word"))?;
    if old.gnu_warning {
        return Err(fail("facts", format!("key {}: warned in the old link", id.0)));
    }
    let facts = mortar_scan::global_facts(&n, id, 0);
    Ok(KeyAns { facts, undefined: false })
}

/// The answer for a key whose new row is its stored row and not a
/// definition in the changed file: the stored facts, `dead` recomputed for
/// a section definition (its stored section's place after the liveness
/// delta). An undefined key (its kind follows the references' binding and
/// the COMDAT copies, `comdat_only`) and a linker-defined one (withdrawn
/// or not) fall back.
fn stored_answer(
    view: &StateView<'_>,
    options: &OptionKeys,
    live: &LiveDelta,
    id: SymbolId,
    row: ResolvedRow,
) -> Result<KeyAns, Fail> {
    let mut f = SymFacts::unpack(row.facts).ok_or_else(|| corrupt("a RESOLVED.facts word"))?;
    match row.kind {
        def_kind::UNDEFINED => return Err(fail("facts", format!("key {}: undefined", id.0))),
        def_kind::INTERNAL => return Err(fail("facts", format!("key {}: linker-defined", id.0))),
        def_kind::SECTION => {
            let sid = SectionId(row.section);
            let s = view.section(sid).map_err(rows)?;
            let kept = s.bits & section_bits::KEPT != 0;
            let is_live = if options.gc_sections {
                (kept
                    && s.bits & section_bits::COLLECTED == 0
                    && live.now_dead.binary_search(&sid).is_err())
                    || live.now_live.binary_search(&sid).is_ok()
            } else {
                kept
            };
            f.dead = !(kept && is_live && s.out != 0);
        }
        def_kind::ABSOLUTE | def_kind::COMMON | def_kind::SHARED => {}
        _ => return Err(corrupt(format!("key {}: RESOLVED kind {}", id.0, row.kind))),
    }
    if f.gnu_warning || f.comdat_only || f.kind == TargetKind::Undefined {
        return Err(fail("facts", format!("key {}: an event key", id.0)));
    }
    Ok(KeyAns { facts: f, undefined: false })
}

/// One section index of the file, as the walk reads it.
#[derive(Debug, Clone, Copy)]
struct Sec {
    id: SectionId,
    reloc: u32,
    kind: SectionKind,
    live: bool,
    place: bool,
}

/// A key's facts (`comdat_only` and `gnu_warning` included) and whether its
/// definition is undefined.
#[derive(Debug, Clone, Copy)]
struct KeyAns {
    facts: SymFacts,
    undefined: bool,
}

/// The narrow [`Whole`] of one changed file (the module doc).
struct Narrow<'f, 's> {
    front: &'f FileFront<'s>,
    display: FileRef,
    machine: Machine,
    output: OutputKind,
    prep: OnceLock<Prep<'s>>,
    secs: Vec<Sec>,
    first: u32,
    globals: Vec<SymbolId>,
    /// The keys defined in the file (ruling item 4) and their entries.
    defs: Vec<(SymbolId, SymEntry)>,
    /// Per key of the file, sorted.
    keys: Vec<(SymbolId, Result<KeyAns, Fail>)>,
    /// The first read the view could not answer.
    asked: Mutex<Option<Fail>>,
}

impl<'s> Narrow<'_, 's> {
    fn own(&self, file: FileId) -> bool {
        file == self.front.file
    }

    fn ask(&self, f: impl FnOnce() -> Fail) {
        if let Ok(mut a) = self.asked.lock()
            && a.is_none()
        {
            *a = Some(f());
        }
    }

    fn other(&self, what: &str) {
        self.ask(|| fail("facts", format!("a read of another file or key: {what}")));
    }

    fn why(&self) -> Option<Fail> {
        self.asked.lock().ok().and_then(|a| a.clone())
    }

    fn prep(&self) -> &Prep<'s> {
        self.prep.get_or_init(|| Prep::of(Some(Reader::Object(self.front.object))))
    }

    fn sec(&self, sid: SectionId) -> Option<&Sec> {
        let k = sid.0.checked_sub(self.first)?;
        self.secs.get(k as usize).filter(|s| s.id == sid)
    }

    fn key(&self, id: SymbolId) -> Option<KeyAns> {
        match self.keys.binary_search_by_key(&id, |k| k.0).map(|i| &self.keys[i].1) {
            Ok(Ok(a)) => Some(*a),
            Ok(Err(e)) => {
                self.ask(|| e.clone());
                None
            }
            Err(_) => {
                self.other("a key the file does not name");
                None
            }
        }
    }

    /// The first entry of the file whose key is `id` and which is defined
    /// in section `index`.
    fn defining_entry(&self, id: SymbolId, index: u32) -> Option<SymEntry> {
        let table = self.prep().table()?;
        for (k, &g) in self.globals.iter().enumerate() {
            if g != id {
                continue;
            }
            let at = SymbolIndex(self.front.first_global + k as u32);
            let s = table.get(at).ok()?;
            if table.section_of(at, &s).ok() == Some(SymbolSection::Section(SectionIndex(index))) {
                return Some(SymEntry { file: self.front.file, index: at });
            }
        }
        None
    }
}

impl<'s> Whole<'s> for &Narrow<'_, 's> {
    fn program(&self) -> Option<&Program<'_, 's>> {
        None
    }
    fn files(&self) -> usize {
        self.front.file.0 as usize + 1
    }
    fn reader(&self, file: FileId) -> Option<Reader<'s>> {
        if self.own(file) {
            return Some(Reader::Object(self.front.object));
        }
        self.other("reader");
        None
    }
    fn prep(&self, file: FileId) -> Option<&Prep<'s>> {
        if self.own(file) {
            return Some(Narrow::prep(self));
        }
        self.other("prep");
        None
    }
    fn display(&self, file: FileId) -> FileRef {
        if !self.own(file) {
            self.other("display");
        }
        self.display.clone()
    }
    fn machine(&self) -> Machine {
        self.machine
    }
    fn has_bytes(&self, file: FileId) -> bool {
        if !self.own(file) {
            self.other("has_bytes");
        }
        self.own(file)
    }
    fn copy_to(
        &self,
        file: FileId,
        range: std::ops::Range<u64>,
        out: &mut [u8],
    ) -> Result<(), CopyError> {
        if !self.own(file) {
            self.other("copy_to");
            return Err(CopyError::OutOfRange);
        }
        let (Ok(a), Ok(b)) = (usize::try_from(range.start), usize::try_from(range.end)) else {
            return Err(CopyError::OutOfRange);
        };
        match self.front.bytes.get(a..b) {
            Some(src) if src.len() == out.len() => {
                out.copy_from_slice(src);
                Ok(())
            }
            _ => Err(CopyError::OutOfRange),
        }
    }
    fn section_id(&self, file: FileId, index: SectionIndex) -> Option<SectionId> {
        if !self.own(file) {
            self.other("section_id");
            return None;
        }
        self.secs.get(index.0 as usize).map(|s| s.id)
    }
    fn reloc_of(&self, sid: SectionId) -> (u32, SectionKind) {
        if let Some(s) = self.sec(sid) {
            return (s.reloc, s.kind);
        }
        self.other("reloc_of");
        (0, SectionKind::Null)
    }
    fn is_live(&self, sid: SectionId) -> bool {
        if let Some(s) = self.sec(sid) {
            return s.live;
        }
        self.other("is_live");
        false
    }
    fn has_place(&self, file: FileId, index: SectionIndex) -> bool {
        if !self.own(file) {
            self.other("has_place");
            return false;
        }
        self.secs.get(index.0 as usize).is_some_and(|s| s.place)
    }
    fn globals(&self, file: FileId) -> &[SymbolId] {
        if !self.own(file) {
            self.other("globals");
            return &[];
        }
        &self.globals
    }
    fn redirect(&self, id: SymbolId) -> SymbolId {
        // D4-5: a `--wrap` link fell back before the walk.
        id
    }
    fn def_undefined(&self, id: SymbolId) -> bool {
        self.key(id).is_some_and(|a| a.undefined)
    }
    fn dropped_by_comdat(&self, _: FileId, _: SectionIndex) -> bool {
        // Asked after `is_mismatch` only, which no answered key is.
        self.ask(|| fail("facts", "dropped_by_comdat"));
        false
    }
    fn key_name(&self, id: SymbolId) -> Option<(&'s [u8], Option<&'s [u8]>)> {
        let k = self.globals.iter().position(|&g| g == id)?;
        let e = &self.front.keys[k];
        Some((e.name, e.version))
    }
    fn key_facts(&self, id: SymbolId) -> (SymFacts, bool) {
        match self.key(id) {
            Some(a) => (a.facts, a.undefined),
            None => (UNANSWERED, false),
        }
    }
    fn is_weak0(&self, id: SymbolId) -> bool {
        self.key(id).is_some_and(|a| a.facts.kind == TargetKind::Weak0)
    }
    fn event_keys(&self) -> Vec<SymbolId> {
        // Exact for its one reader (`Defer::mentions`, the file's own
        // globals); a key without an answer counts as one (its debug
        // records are then walked, and a record naming it falls back).
        let mut v: Vec<SymbolId> = self
            .keys
            .iter()
            .filter(|(_, a)| match a {
                Ok(a) => {
                    a.facts.kind == TargetKind::Undefined
                        || a.facts.comdat_only
                        || a.facts.gnu_warning
                }
                Err(_) => true,
            })
            .map(|k| k.0)
            .collect();
        v.sort_unstable();
        v
    }
    fn is_mismatch(&self, id: SymbolId) -> bool {
        self.key(id).is_some_and(|a| a.facts.comdat_only)
    }
    fn def(&self, id: SymbolId) -> Def {
        if let Ok(i) = self.defs.binary_search_by_key(&id, |d| d.0) {
            return Def::Defined(self.defs[i].1);
        }
        self.other("def");
        Def::Undefined { weak: false }
    }
    fn preemptible(&self, _: SymbolId) -> bool {
        // Ruling item 3: false for a key defined in the file outside a
        // `Shared` output (refused before the walk); `def` answers only
        // those keys.
        false
    }
    fn verdict(&self, _: SymbolId) -> Verdict {
        self.ask(|| fail("facts", "verdict"));
        Verdict::default()
    }
    fn assigned(&self, _: u32) -> Option<Assigned> {
        self.ask(|| fail("facts", "--defsym"));
        None
    }
    fn output(&self) -> OutputKind {
        self.output
    }
    fn is_weak_dyn(&self, _: SymbolId) -> bool {
        self.ask(|| fail("facts", "weak_dyn"));
        false
    }
}

/// The answer given with a recorded fall-back (never used: the fall-back
/// wins).
const UNANSWERED: SymFacts = SymFacts {
    kind: TargetKind::Undefined,
    sym_type: 0,
    st_other: 0,
    tls: false,
    dead: false,
    comdat_only: false,
    gnu_warning: false,
};

/// WP-177's owners over the file's own `.eh_frame` records (the full
/// link's `eh_owners::Owners` bodies, `EhFrames::frame_owner` and
/// `LsdaStarts`), liveness the view's; D4-4 made every LSDA the file's.
struct NarrowEh<'a> {
    frames: &'a EhFrames,
    relocs: Vec<Box<[EhReloc]>>,
    first: u32,
    live: Vec<bool>,
    lsda: OnceLock<LsdaStarts>,
    asked: Mutex<Option<Fail>>,
}

impl std::fmt::Debug for NarrowEh<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NarrowEh").finish_non_exhaustive()
    }
}

impl<'a> NarrowEh<'a> {
    fn new(n: &Narrow<'_, '_>, split: &'a FileSplit) -> Self {
        Self {
            frames: &split.eh,
            relocs: split.eh_relocs.iter().map(|r| r.clone().into_boxed_slice()).collect(),
            first: n.first,
            live: n.secs.iter().map(|s| s.live).collect(),
            lsda: OnceLock::new(),
            asked: Mutex::new(None),
        }
    }

    fn is_live(&self, s: SectionId) -> bool {
        let v = s.0.checked_sub(self.first).and_then(|k| self.live.get(k as usize).copied());
        if v.is_none()
            && let Ok(mut a) = self.asked.lock()
            && a.is_none()
        {
            *a = Some(fail("facts", "an .eh_frame record's section of another file"));
        }
        v.unwrap_or(false)
    }

    fn why(&self) -> Option<Fail> {
        self.asked.lock().ok().and_then(|a| a.clone())
    }
}

impl mortar_scan::EhOwners for NarrowEh<'_> {
    fn frame(&self, section: SectionId, offset: u64) -> EhOwner {
        self.frames.frame_owner(section, offset, |s| self.is_live(s))
    }

    fn lsda(&self, table: SectionId, offset: u64) -> EhOwner {
        let starts = self
            .lsda
            .get_or_init(|| LsdaStarts::new(self.frames, &self.relocs, |s| self.is_live(s)));
        starts.owner(table, offset)
    }
}

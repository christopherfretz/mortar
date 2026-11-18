//! Delta resolution (`spec/incr.md` IN-71, IN-71c; IN-70 step U5): the
//! affected keys re-resolved from their `KEY_REFS` rows and the changed
//! files' fresh mentions, COMDAT groups re-elected, commons re-chosen.
//! WP-213 (U-1): the type [`ResolveDelta`] and the entry point
//! [`resolve`]; **filled by U-3** (the function only).

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use mortar_core::incr::{IncrReason, OptionKeys, Unimplemented};
use mortar_core::{FileId, SectionId, SymbolId};
use mortar_elf::consts::{
    STB_WEAK, STT_GNU_IFUNC, STT_TLS, STV_DEFAULT, STV_HIDDEN, STV_INTERNAL, STV_PROTECTED,
};

use super::extract::{Ctx, Extraction, Ment};
use super::rows::{
    CommonRow, GroupRow, KeyRefRow, MentionRow, ResolvedRow, SymbolRow, keeper, key_ref_flags,
    sym_kind, winning_common,
};
use super::view::{Change, ChangedFile, ChangedFiles, KeyEdits, StateView};
use crate::format::{NONE, Str, def_kind};

/// A key new to the state (no `SYMBOLS` row, so `StateRows::find_key`
/// found none): its id is the state's `SYMBOLS` count plus its position in
/// [`ResolveDelta::new_keys`], and the patch puts its `NAMES` row. A `GONE`
/// key that comes back is not new: `find_key` returns its old row, which
/// is affected and set again (IN-18e).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewKey {
    /// Its `SYMBOLS` row as appended (name and version as strings the
    /// patch appends: U-2 interns them).
    pub row: SymbolRow,
    pub name: Vec<u8>,
    pub version: Vec<u8>,
}

/// U-3's resolution delta (WP-211 §C `ResolveDelta`; IN-71, IN-71c).
/// Written by U-3; read by U-3 (`live`, `refill_set`), U-4 (through the
/// join's `SectionChange`s: `kept`, commons), U-5 (the globals whose entry
/// changes), U-8, U-9, U-2 (its rows in the patch).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolveDelta {
    /// The affected keys (IN-71: every key a changed, appeared, gone,
    /// drawn-in, extracted or no longer extracted file mentions, old or new
    /// version, and the linker-defined keys of IN-71c), sorted.
    pub affected: Vec<SymbolId>,
    /// New keys, in the order their rows are appended. A file's new
    /// local keys are ONE contiguous run, by symbol-table index (the
    /// file's `FILE_INDEX.locals`: `FileRanges::locals`; seam 6). No row
    /// has the `option` bit.
    pub new_keys: Vec<NewKey>,
    /// Per changed file (`ChangedFiles::files` order), its
    /// `ChangedFile::keys` mapped to `SymbolId`s (position for position).
    pub key_ids: Vec<(FileId, Vec<SymbolId>)>,
    /// Each affected key's new `RESOLVED` row (sorted by key).
    pub resolved: Vec<(SymbolId, ResolvedRow)>,
    /// Keys no file mentions any more (`SYMBOLS.kind` `GONE`): globals
    /// only, never one with the `option` bit (a replaced or gone file's
    /// old locals are marked by U-2 through `FileRanges::locals`; seam 6).
    pub gone: Vec<SymbolId>,
    /// `at_file`, `at_index` of keys whose first mention changed (IN-71).
    pub at: Vec<(SymbolId, u32, u32)>,
    /// `KEY_REFS`, `GROUPS` (per (signature, file)), `COMMONS` (per (key,
    /// file)) edits.
    pub key_refs: KeyEdits<KeyRefRow>,
    pub groups: KeyEdits<GroupRow>,
    pub commons: KeyEdits<CommonRow>,
    /// Groups whose keeper changed: `(signature, old keeper, new keeper)`
    /// (`FILES` rows; IN-72 item 2a's drawn-in files, O-21).
    pub reelected: Vec<(Vec<u8>, FileId, FileId)>,
}

/// `resolve`'s result (WP-215 seam 3, G1): the complete delta, or the
/// unchanged files it needs read first. A partial delta is never
/// returned: the join cannot use one by mistake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Every affected key resolved: the delta (boxed: the variants'
    /// sizes differ by some 300 bytes).
    Delta(Box<ResolveDelta>),
    /// Unchanged files that become a key's definer, a common's winning
    /// file or a group's keeper, whose `st_value`/`st_size` (or section
    /// rows) the update does not have: sorted, none of them in `changed`.
    /// The join reads each as `Change::DrawnIn` (its rows and
    /// `ChangedFile::values` from its bytes, after IN-75's `STAT` check),
    /// adds it to `changed`, and calls `resolve` again with the same
    /// extraction. They count toward IN-16's bound (the join's).
    DrawIn(Vec<FileId>),
}

/// At most this many `resolve` calls per update (WP-215 seam 3): a
/// `DrawnIn` file's mentions equal its stored rows (else IN-75's check
/// fails first), so it adds no affected key and changes no candidate or
/// winner; the second call has every winner's values and asks for
/// nothing. A `DrawIn` asked for while `changed` already holds a
/// `DrawnIn` file is therefore impossible unless a drawn-in file's bytes
/// disagree with its stored rows: `resolve` gives `input-changed` then.
pub const RESOLVE_CALLS: usize = 2;

/// Resolve the affected keys (WP-211 §C; IN-71). Called by the join after
/// `extract` (IN-70 U5). `options`: the link's roots, `--wrap` (applied
/// when a stored row is read) and `--defsym` keys, passed in (WP-215
/// D3-D5).
///
/// A `DrawnIn` file's mentions are its stored ones: its keys are affected
/// (its sections take new ids) but none changes winner; its values stand
/// in for the stored link's unknown ones.
///
/// # Errors
/// A §7 reason, before anything is written: `closure-iterated` (the full
/// link of the new inputs would iterate §4.1's closure: an affected key's
/// `first` or `cfirst` is a live object whose definition of it is in a
/// COMDAT copy the new keepers drop, WP-215 §Q.2; or the stored link
/// iterated), `symbol-kind-changed` (IN-71's attribute list, for a key an
/// unchanged live file mentions), `input-changed` (a second draw-in,
/// [`RESOLVE_CALLS`]), `update-not-implemented (resolve)` (WP-215's
/// named cases: a shared object mentions an affected key, two live strong
/// definitions, a `STT_TLS` and a non-TLS mention of one key, a key that
/// becomes undefined and referenced while the stored link did not leave
/// it so; and a join that calls before every extracted member is read),
/// `update-not-implemented (versioned)`, `state-corrupt`.
pub fn resolve(
    view: &StateView<'_>,
    changed: &ChangedFiles,
    extraction: &Extraction,
    options: &OptionKeys,
) -> Result<Resolution, IncrReason> {
    let cx = Ctx::new(view, changed, options)?;
    let mut r = Res::new(&cx, extraction)?;
    r.run()
}

/// The named fall-back of this package's unsupported cases.
fn not_implemented() -> IncrReason {
    IncrReason::UpdateNotImplemented(Some(Unimplemented::Resolve))
}

fn corrupt(what: impl std::fmt::Display) -> IncrReason {
    IncrReason::StateCorrupt(what.to_string())
}

/// `SECTIONS.anchor_kind` of a section in a COMDAT group (the anchor is
/// the signature).
const COMDAT_ANCHOR: u8 = 1;

/// resolve.md §3.2's classes of a live object's candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Strong,
    Common,
    Weak,
}

fn class_of_bind(bind: u8) -> Class {
    if bind == STB_WEAK { Class::Weak } else { Class::Strong }
}

/// The constraint rank of a visibility (resolve.md §3.5: hidden and
/// internal 0, protected 1, default 2) and back.
const fn vis_rank(v: u8) -> u8 {
    match v & key_ref_flags::VISIBILITY {
        STV_INTERNAL | STV_HIDDEN => 0,
        STV_PROTECTED => 1,
        _ => 2,
    }
}

const fn rank_vis(r: u8) -> u8 {
    match r {
        0 => STV_HIDDEN,
        1 => STV_PROTECTED,
        _ => STV_DEFAULT,
    }
}

/// A key's definition in the update's link (resolve.md §3.2).
#[derive(Debug, Clone, Copy)]
enum NewDef {
    /// No candidate (whether a live reference is strong is the caller's).
    Undefined,
    /// File 0's: a `--defsym` key (rank 0) or a linker-provided one
    /// (rank 5, §8).
    Internal,
    /// A strong or weak definition of a live object.
    Defined { class: Class, ment: Ment },
    /// The least live common mention.
    Common { ment: Ment },
}

/// The update's resolution over [`Ctx`].
struct Res<'c, 'a, 's> {
    cx: &'c Ctx<'a, 's>,
    extracted: BTreeSet<u32>,
    unextracted: BTreeSet<u32>,
    /// `changed` holds a `DrawnIn` file: this is the second call.
    second: bool,
    stored_ments: RefCell<BTreeMap<u32, Rc<Vec<MentionRow>>>>,
    /// The re-elected signatures: (old keeper, new keeper).
    elected: BTreeMap<Vec<u8>, (Option<u32>, Option<u32>)>,
    /// Keepers of signatures not re-elected (the stored ones).
    kept: RefCell<BTreeMap<Vec<u8>, Option<u32>>>,
    /// The unchanged files to read (IN-72 item 2a).
    draw: BTreeSet<u32>,
    out: ResolveDelta,
}

impl<'c, 'a, 's> Res<'c, 'a, 's> {
    fn new(cx: &'c Ctx<'a, 's>, x: &Extraction) -> Result<Self, IncrReason> {
        let r = Self {
            cx,
            extracted: x.extracted.iter().map(|(f, _)| f.0).collect(),
            unextracted: x.unextracted.iter().map(|f| f.0).collect(),
            second: cx.changed.files.iter().any(|c| c.change == Change::DrawnIn),
            stored_ments: RefCell::new(BTreeMap::new()),
            elected: BTreeMap::new(),
            kept: RefCell::new(BTreeMap::new()),
            draw: BTreeSet::new(),
            out: ResolveDelta::default(),
        };
        // IN-71 "U4 and U5 iterate": every newly extracted member is read
        // before `resolve` (its undefined mentions are not stored).
        for &m in &r.extracted {
            if r.fresh(m).is_none() {
                debug_assert!(false, "resolve called before member {m} was read");
                return Err(not_implemented());
            }
        }
        Ok(r)
    }

    fn view(&self) -> &'c StateView<'s> {
        self.cx.view
    }

    /// The changed file `f` with new rows (not gone, not no longer
    /// extracted), drawn-in files included.
    fn fresh(&self, f: u32) -> Option<&'a ChangedFile> {
        self.cx
            .changed_file(f)
            .filter(|c| !matches!(c.change, Change::Gone | Change::NoLongerExtracted))
    }

    /// `f` is in `changed` other than drawn in: its rows are replaced (a
    /// drawn-in file's rows equal its stored ones).
    fn edited(&self, f: u32) -> bool {
        self.cx.changed_file(f).is_some_and(|c| c.change != Change::DrawnIn)
    }

    /// Live after the extraction.
    fn live_new(&self, f: u32) -> Result<bool, IncrReason> {
        if f == 0 {
            return Ok(true);
        }
        let x = self.cx.facts(f)?;
        Ok(match x.change {
            Some(Change::Gone | Change::NoLongerExtracted) => false,
            Some(Change::Extracted) => true,
            _ if self.extracted.contains(&f) => true,
            _ if self.unextracted.contains(&f) => false,
            _ => self.cx.live_base(f)?.is_some(),
        })
    }

    fn live_old(&self, f: u32) -> Result<bool, IncrReason> {
        Ok(f == 0 || self.cx.facts(f)?.live_old)
    }

    fn object(&self, f: u32) -> Result<bool, IncrReason> {
        Ok(f != 0 && self.cx.facts(f)?.kind == 1)
    }

    /// A stored file's `MENTIONS` range (read once per file).
    fn stored_ments(&self, f: u32) -> Result<Rc<Vec<MentionRow>>, IncrReason> {
        if let Some(r) = self.stored_ments.borrow().get(&f) {
            return Ok(r.clone());
        }
        let rows = Rc::new(self.view().mentions_of(FileId(f))?);
        self.stored_ments.borrow_mut().insert(f, rows.clone());
        Ok(rows)
    }

    /// A stored file's first `MENTIONS` row of `key` and its position.
    fn stored_mention(&self, f: u32, key: u32) -> Result<Option<(u32, MentionRow)>, IncrReason> {
        let rows = self.stored_ments(f)?;
        Ok(rows.iter().position(|m| m.symbol == key).map(|p| (p as u32, rows[p])))
    }

    /// The signature of the COMDAT copy a stored section is in.
    fn stored_sig(&self, section: u32) -> Result<Option<Vec<u8>>, IncrReason> {
        let s = self.view().section(SectionId(section))?;
        if s.anchor_kind == COMDAT_ANCHOR {
            Ok(Some(self.view().string(s.anchor)?))
        } else {
            Ok(None)
        }
    }

    /// The signature of the COMDAT copy `m` (a mention of `key` by its
    /// file) is defined in, if any.
    fn copy_sig(&self, key: u32, m: &Ment) -> Result<Option<Vec<u8>>, IncrReason> {
        if m.def != def_kind::SECTION {
            return Ok(None);
        }
        if let Some((i, p)) = m.new {
            let c = &self.cx.changed.files[i];
            let s = c.mentions[p].section;
            let row = s
                .checked_sub(c.first_section.0)
                .and_then(|x| c.sections.get(x as usize))
                .ok_or_else(|| corrupt("a changed file's mention names no section of it"))?;
            if row.anchor_kind != COMDAT_ANCHOR {
                return Ok(None);
            }
            return Ok(Some(self.cx.changed.string(self.view(), row.anchor)?));
        }
        // Cheap path (§Q.2): the stored definer's own section.
        if key < self.cx.symbols {
            let r = self.view().resolved(SymbolId(key))?;
            if r.kind == def_kind::SECTION
                && r.section != NONE
                && self.view().section(SectionId(r.section))?.file == m.file
            {
                return self.stored_sig(r.section);
            }
        }
        match self.stored_mention(m.file, key)? {
            Some((_, row)) if row.kind == def_kind::SECTION && row.section != NONE => {
                self.stored_sig(row.section)
            }
            _ => Err(corrupt(format!(
                "KEY_REFS defines key {key} in a section of file {}, MENTIONS does not",
                m.file
            ))),
        }
    }

    /// The new keeper of `sig`.
    fn keeper_new(&self, sig: &[u8]) -> Result<Option<u32>, IncrReason> {
        if let Some(&(_, new)) = self.elected.get(sig) {
            return Ok(new);
        }
        if let Some(&k) = self.kept.borrow().get(sig) {
            return Ok(k);
        }
        let rows = self.view().group(sig)?;
        let k = keeper(&rows, |f| Ok(self.cx.facts(f)?.rank))?.map(|r| r.file);
        self.kept.borrow_mut().insert(sig.to_vec(), k);
        Ok(k)
    }

    /// A live object's definition of `key` dropped with its COMDAT copy
    /// (resolve.md §6.2: a reference from then on).
    fn demoted(&self, key: u32, m: &Ment) -> Result<bool, IncrReason> {
        match self.copy_sig(key, m)? {
            Some(sig) => Ok(self.keeper_new(&sig)? != Some(m.file)),
            None => Ok(false),
        }
    }

    fn run(&mut self) -> Result<Resolution, IncrReason> {
        self.elect()?;
        let affected = self.affected()?;
        // §Q.2 first, over every affected key: the full link would
        // iterate whatever else the update finds.
        for &k in &affected {
            self.closure_check(k)?;
        }
        let named = self.named()?;
        for &k in &affected {
            self.resolve_key(k, &named)?;
        }
        if !self.draw.is_empty() {
            if self.second {
                // WP-215 seam 3: a drawn-in file adds no affected key and
                // changes no candidate, so the second call asks for
                // nothing; a test run fails here if that argument is wrong.
                debug_assert!(false, "a second draw-in: {:?}", self.draw);
                return Err(IncrReason::InputChanged(format!(
                    "resolve asked again for {} file(s) after a draw-in",
                    self.draw.len()
                )));
            }
            return Ok(Resolution::DrawIn(self.draw.iter().map(|&f| FileId(f)).collect()));
        }
        self.out.affected = affected.iter().map(|&k| SymbolId(k)).collect();
        self.key_refs()?;
        self.new_keys();
        self.out.key_ids = self
            .cx
            .changed
            .files
            .iter()
            .zip(&self.cx.ids)
            .map(|(c, ids)| (c.file, ids.clone()))
            .collect();
        Ok(Resolution::Delta(Box::new(std::mem::take(&mut self.out))))
    }

    /// IN-71's re-election of every group with a copy in an edited file
    /// (old or new version) or in a member whose extraction flips; the
    /// `GROUPS` edits; the old and new keepers drawn in (IN-72 item 2a).
    fn elect(&mut self) -> Result<(), IncrReason> {
        let view = self.view();
        let changed = self.cx.changed;
        let mut sigs: BTreeSet<Vec<u8>> = BTreeSet::new();
        let mut stored_files: BTreeSet<u32> = self.unextracted.clone();
        for c in &changed.files {
            if c.change == Change::DrawnIn {
                continue;
            }
            if c.change != Change::Appeared && self.cx.facts(c.file.0)?.stored {
                stored_files.insert(c.file.0);
            }
            if self.fresh(c.file.0).is_some() {
                for s in &c.sections {
                    if s.anchor_kind == COMDAT_ANCHOR {
                        sigs.insert(changed.string(view, s.anchor)?);
                    }
                }
            }
        }
        for &f in &stored_files {
            for s in view.sections_of(FileId(f))? {
                if s.anchor_kind == COMDAT_ANCHOR {
                    sigs.insert(view.string(s.anchor)?);
                }
            }
        }
        for sig in sigs {
            let rows = view.group(&sig)?;
            let old = keeper(&rows, |f| Ok(self.cx.facts(f)?.rank))?.map(|r| r.file);
            let mut holders: BTreeMap<u32, GroupRow> = BTreeMap::new();
            for r in &rows {
                if !self.edited(r.file) && self.live_new(r.file)? {
                    holders.insert(r.file, *r);
                }
            }
            for c in &changed.files {
                let f = c.file.0;
                if !self.edited(f) || self.fresh(f).is_none() || !self.live_new(f)? {
                    continue;
                }
                for s in &c.sections {
                    if s.anchor_kind == COMDAT_ANCHOR
                        && !holders.contains_key(&f)
                        && changed.string(view, s.anchor)? == sig
                    {
                        holders.insert(f, GroupRow { signature: s.anchor, file: f, tomb: false });
                    }
                }
            }
            let mut new: Option<(u64, u32)> = None;
            for &f in holders.keys() {
                let at = self.cx.order(f)?;
                if new.is_none_or(|n| at < n) {
                    new = Some(at);
                }
            }
            let new = new.map(|n| n.1);
            for r in &rows {
                if !holders.contains_key(&r.file) {
                    self.out.groups.delete.push(*r);
                }
            }
            for (f, g) in &holders {
                if !rows.iter().any(|r| r.file == *f) {
                    self.out.groups.put.push(*g);
                }
            }
            if old != new {
                if let (Some(o), Some(n)) = (old, new) {
                    self.out.reelected.push((sig.clone(), FileId(o), FileId(n)));
                }
                for f in [old, new].into_iter().flatten() {
                    if self.cx.changed_file(f).is_none() && self.live_new(f)? {
                        self.draw.insert(f);
                    }
                }
            }
            self.elected.insert(sig, (old, new));
        }
        Ok(())
    }

    /// IN-71's affected keys (sorted): every key a changed file mentions,
    /// old version or new; every key a member whose extraction flips
    /// mentions; the keys defined in the copies of a re-elected group's old
    /// and new keeper; each with its `--wrap` target. A drawn-in file's
    /// keys too (IN-71's list: its sections take new ids, so the rows
    /// naming them are rewritten); none of them changes a candidate or a
    /// winner (its mentions are its stored rows), so the second call asks
    /// for nothing (WP-215 seam 3).
    fn affected(&self) -> Result<Vec<u32>, IncrReason> {
        let mut keys: BTreeSet<u32> = BTreeSet::new();
        let mut stored_files: BTreeSet<u32> = self.unextracted.clone();
        for (c, ids) in self.cx.changed.files.iter().zip(&self.cx.ids) {
            if c.change != Change::Appeared
                && c.change != Change::DrawnIn
                && self.cx.facts(c.file.0)?.stored
            {
                stored_files.insert(c.file.0);
            }
            if self.fresh(c.file.0).is_some() {
                for m in &c.mentions {
                    let id = ids
                        .get(m.symbol as usize)
                        .ok_or_else(|| corrupt("a changed file's mention names no key"))?;
                    keys.insert(id.0);
                }
            }
        }
        for &f in &stored_files {
            for m in self.stored_ments(f)?.iter() {
                keys.insert(m.symbol);
            }
        }
        for (sig, &(old, new)) in &self.elected {
            if old == new {
                continue;
            }
            for f in [old, new].into_iter().flatten() {
                if self.edited(f) {
                    continue;
                }
                for m in self.stored_ments(f)?.iter() {
                    if m.kind == def_kind::SECTION
                        && m.section != NONE
                        && self.stored_sig(m.section)?.as_deref() == Some(sig.as_slice())
                    {
                        keys.insert(m.symbol);
                    }
                }
            }
        }
        for k in keys.clone() {
            keys.insert(self.cx.redirect(k)?);
        }
        Ok(keys.into_iter().collect())
    }

    /// WP-215 §Q.2: `first(K)` or `cfirst(K)` a live object whose
    /// definition of `K` the new keepers drop makes the full link iterate.
    fn closure_check(&self, k: u32) -> Result<(), IncrReason> {
        let t = self.cx.targets_new(k)?;
        let mut seen = None;
        for f in [t.first, t.cfirst].into_iter().flatten() {
            if seen == Some(f) || !self.object(f)? || !self.live_new(f)? {
                continue;
            }
            seen = Some(f);
            for m in self.cx.ments_new(k)? {
                if m.file == f && m.is_defined() && self.demoted(k, &m)? {
                    let (name, _) = self.cx.name_of(k)?;
                    return Err(IncrReason::ClosureIterated(format!(
                        "the definition of `{}` by file {f}, its first or cfirst, is in a \
                         dropped COMDAT copy",
                        String::from_utf8_lossy(&name)
                    )));
                }
            }
        }
        Ok(())
    }

    /// The keys the options name (never gone).
    fn named(&self) -> Result<BTreeSet<u32>, IncrReason> {
        let mut named: BTreeSet<u32> = self.cx.roots.iter().map(|r| r.0).collect();
        named.extend(self.cx.defsym.iter().copied());
        for w in &self.cx.options.wrap {
            for n in [
                w.to_vec(),
                [b"__wrap_".as_slice(), w].concat(),
                [b"__real_".as_slice(), w].concat(),
            ] {
                if let Some(k) = self.cx.key_named(&n)? {
                    named.insert(k);
                }
            }
        }
        Ok(named)
    }

    /// The merged visibility rank of `key` (resolve.md §3.5) over the
    /// live objects' mentions: its own definitions and commons, and the
    /// undefined mentions that bind to it.
    fn vis(&self, key: u32, old: bool) -> Result<u8, IncrReason> {
        let mut r = 2;
        let live = |f: u32| if old { self.live_old(f) } else { self.live_new(f) };
        for s in self.cx.sources(key)? {
            let ments = if old { self.cx.ments_old(s)? } else { self.cx.ments_new(s)? };
            for m in ments {
                if m.def == def_kind::UNDEFINED && self.object(m.file)? && live(m.file)? {
                    r = r.min(vis_rank(m.flags));
                }
            }
        }
        let ments = if old { self.cx.ments_old(key)? } else { self.cx.ments_new(key)? };
        for m in ments {
            if m.def != def_kind::UNDEFINED && self.object(m.file)? && live(m.file)? {
                r = r.min(vis_rank(m.flags));
            }
        }
        Ok(r)
    }

    /// Whether a live object refers to `key` (an undefined mention binding
    /// to it) or a root names it; `strong` asks for a non-weak one.
    fn referenced(&self, key: u32, old: bool, strong: bool) -> Result<bool, IncrReason> {
        if self.cx.roots.iter().any(|r| r.0 == key) {
            return Ok(true);
        }
        for s in self.cx.sources(key)? {
            let ments = if old { self.cx.ments_old(s)? } else { self.cx.ments_new(s)? };
            for m in ments {
                let live = if old { self.live_old(m.file)? } else { self.live_new(m.file)? };
                if m.def == def_kind::UNDEFINED
                    && (!strong || m.bind != STB_WEAK)
                    && self.object(m.file)?
                    && live
                {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// The stored definer of `key`'s stored row and its class.
    fn old_definer(&self, key: u32, row: &ResolvedRow) -> Result<Option<(u32, Class)>, IncrReason> {
        Ok(match row.kind {
            def_kind::SECTION => {
                Some((self.view().section(SectionId(row.section))?.file, class_of_bind(row.bind)))
            }
            def_kind::ABSOLUTE => {
                // No section names it: the least live absolute candidate of
                // its class (an earlier candidate of the class in a section
                // would have won).
                let class = class_of_bind(row.bind);
                let mut best: Option<(u64, u32)> = None;
                for m in self.cx.ments_old(key)? {
                    if m.def == def_kind::ABSOLUTE
                        && class_of_bind(m.bind) == class
                        && self.object(m.file)?
                        && self.live_old(m.file)?
                    {
                        let at = self.cx.order(m.file)?;
                        if best.is_none_or(|b| at < b) {
                            best = Some(at);
                        }
                    }
                }
                best.map(|b| (b.1, class))
            }
            def_kind::COMMON => winning_common(&self.view().commons(SymbolId(key))?, |f| {
                Ok(self.cx.facts(f)?.rank)
            })?
            .map(|c| (c.file, Class::Common)),
            _ => None,
        })
    }

    /// The new definition of `key` (resolve.md §3.2 over the live objects'
    /// mentions, demotion applied) and whether a strong reference demoted.
    fn new_def(&self, key: u32, ments: &[Ment]) -> Result<(NewDef, bool), IncrReason> {
        let mut live: Vec<((u64, u32), Ment)> = Vec::new();
        for m in ments {
            if self.object(m.file)? && self.live_new(m.file)? {
                live.push((self.cx.order(m.file)?, *m));
            }
        }
        live.sort_by_key(|x| x.0);
        let mut strong: Vec<Ment> = Vec::new();
        let mut demoted_strong = false;
        for (_, m) in &live {
            if m.is_defined() && m.bind != STB_WEAK {
                if self.demoted(key, m)? {
                    demoted_strong = true;
                } else {
                    strong.push(*m);
                }
            }
        }
        if strong.len() > 1 {
            // §3.8: the full link reports the clash (or takes the first
            // under `--allow-multiple-definition`, which the update does
            // not see).
            return Err(not_implemented());
        }
        let tls = live.iter().any(|(_, m)| m.typ == STT_TLS && m.def != def_kind::COMMON);
        let non_tls = live.iter().any(|(_, m)| !(m.typ == STT_TLS && m.def != def_kind::COMMON));
        if tls && non_tls {
            return Err(not_implemented());
        }
        if self.cx.defsym.contains(&key) {
            return Ok((NewDef::Internal, demoted_strong));
        }
        if let Some(m) = strong.first() {
            return Ok((NewDef::Defined { class: Class::Strong, ment: *m }, demoted_strong));
        }
        if let Some((_, m)) = live.iter().find(|(_, m)| m.def == def_kind::COMMON) {
            return Ok((NewDef::Common { ment: *m }, demoted_strong));
        }
        for (_, m) in &live {
            if m.is_defined() && m.bind == STB_WEAK && !self.demoted(key, m)? {
                return Ok((NewDef::Defined { class: Class::Weak, ment: *m }, demoted_strong));
            }
        }
        Ok((NewDef::Undefined, demoted_strong))
    }

    /// One affected key: gone, or its new `RESOLVED` row, `COMMONS` rows,
    /// `at`; the files to draw in; the named fall-backs.
    fn resolve_key(&mut self, k: u32, named: &BTreeSet<u32>) -> Result<(), IncrReason> {
        let view = self.view();
        let ments = self.cx.ments_new(k)?;
        let old_ments = self.cx.ments_old(k)?;
        // Shared objects (`spec/dyn.md`'s stage): not in an update.
        for m in ments.iter().chain(&old_ments) {
            if m.file != 0 && self.cx.facts(m.file)?.kind == 2 {
                return Err(not_implemented());
            }
        }
        let stored = k < self.cx.symbols;
        let old_row = if stored { Some(view.resolved(SymbolId(k))?) } else { None };
        if old_row.is_some_and(|r| r.kind == def_kind::SHARED) {
            return Err(not_implemented());
        }
        if !ments.iter().any(|m| m.file != 0) && !named.contains(&k) {
            if stored {
                self.out.gone.push(SymbolId(k));
            }
            self.commons(k, &ments, &old_ments)?;
            return Ok(());
        }
        let (def, demoted_strong) = self.new_def(k, &ments)?;
        let vis = self.vis(k, false)?;
        let mut row = ResolvedRow { section: NONE, ..ResolvedRow::default() };
        match def {
            NewDef::Undefined => {
                let referenced = demoted_strong || self.referenced(k, false, false)?;
                let strong_ref = demoted_strong || self.referenced(k, false, true)?;
                let provided = old_row.is_some_and(|r| r.kind == def_kind::INTERNAL)
                    || self.cx.stored_refs(k)?.iter().any(|r| r.file == 0);
                if referenced && provided {
                    // §8: provided while referenced and undefined (the
                    // stored link provided it: the name is a candidate).
                    row.kind = def_kind::INTERNAL;
                    row.other = rank_vis(vis);
                } else {
                    if referenced {
                        // Whether §8 provides it is the name's (a table the
                        // update does not have): known only when the stored
                        // link left it undefined and referenced; a strong
                        // reference newly unresolved is the full link's
                        // error to report.
                        // The stored row's strong binding is a strong
                        // reference (a demoted strong definition's
                        // included); a weak row needs a live undefined
                        // mention (a weakly demoted one is not looked for:
                        // the fall-back is conservative).
                        let ok = match old_row {
                            Some(r) if r.kind == def_kind::UNDEFINED => {
                                r.bind != STB_WEAK
                                    || (!strong_ref && self.referenced(k, true, false)?)
                            }
                            _ => false,
                        };
                        if !ok {
                            return Err(not_implemented());
                        }
                    }
                    row.kind = def_kind::UNDEFINED;
                    row.bind = if strong_ref { 1 } else { 2 };
                    row.other = rank_vis(vis);
                }
            }
            NewDef::Internal => {
                row.kind = def_kind::INTERNAL;
                row.other = rank_vis(vis);
            }
            NewDef::Defined { class, ment } => {
                row = self.defined_row(k, class, &ment, old_row.as_ref())?;
            }
            NewDef::Common { ment } => {
                row = self.common_row(k, &ment, &ments, old_row.as_ref())?;
            }
        }
        if let Some(old) = &old_row {
            self.kind_check(k, old, &row, &ments)?;
        }
        self.commons(k, &ments, &old_ments)?;
        self.at(k, &ments)?;
        self.out.resolved.push((SymbolId(k), row));
        Ok(())
    }

    /// The row of a strong or weak definition: from the changed file's
    /// rows and values, or the stored row when its definer and class are
    /// the stored ones; else the definer is drawn in (its value and size
    /// are not stored), the row's other columns from its `MENTIONS` row.
    fn defined_row(
        &mut self,
        key: u32,
        class: Class,
        ment: &Ment,
        old: Option<&ResolvedRow>,
    ) -> Result<ResolvedRow, IncrReason> {
        if let Some((i, p)) = ment.new {
            let c = &self.cx.changed.files[i];
            let x = c.mentions[p];
            let v = c.values.get(p).ok_or_else(|| {
                corrupt("a changed file's values are not parallel to its mentions")
            })?;
            return Ok(ResolvedRow {
                kind: x.kind,
                typ: x.typ,
                bind: x.bind,
                other: x.other,
                section: if x.kind == def_kind::SECTION { x.section } else { NONE },
                value: v.value,
                size: v.size,
                // Not known here: the scan runs after the resolution. The
                // join writes the key's packed `SymFacts` into the row
                // before the patch is applied (R-4a, WP-239 section 13).
                facts: 0,
            });
        }
        // An unchanged live object (a member already extracted included).
        if let Some(old) = old
            && matches!(old.kind, def_kind::SECTION | def_kind::ABSOLUTE)
            && self.old_definer(key, old)? == Some((ment.file, class))
        {
            // S-74: the merged visibility can demote the global to a local
            // of its definer's block, or promote it back (IN-72 item 2a).
            if (self.vis(key, true)? == 0) != (self.vis(key, false)? == 0) {
                self.draw.insert(ment.file);
            }
            return Ok(*old);
        }
        self.draw.insert(ment.file);
        let (_, x) = self.stored_mention(ment.file, key)?.ok_or_else(|| {
            corrupt(format!("KEY_REFS names key {key} in file {}, MENTIONS does not", ment.file))
        })?;
        Ok(ResolvedRow {
            kind: x.kind,
            typ: x.typ,
            bind: x.bind,
            other: x.other,
            section: if x.kind == def_kind::SECTION { x.section } else { NONE },
            value: 0,
            size: 0,
            // As above: the join's, after the scan (R-4a).
            facts: 0,
        })
    }

    /// A common's row (resolve.md §3.3): the least live common mention's
    /// type, binding, `st_other` and `st_value` (its alignment), the
    /// largest size. Never a draw-in: the sizes and alignments are
    /// `COMMONS` rows (per file) and the changed files' values, the
    /// winner's other columns its `MENTIONS` row.
    fn common_row(
        &self,
        key: u32,
        winner: &Ment,
        ments: &[Ment],
        old: Option<&ResolvedRow>,
    ) -> Result<ResolvedRow, IncrReason> {
        let mut size = 0u64;
        for c in self.common_rows(key, ments)? {
            size = size.max(c.size);
        }
        let mut row =
            ResolvedRow { kind: def_kind::COMMON, section: NONE, size, ..ResolvedRow::default() };
        if let Some((i, p)) = winner.new {
            let c = &self.cx.changed.files[i];
            let x = c.mentions[p];
            let v = c.values.get(p).ok_or_else(|| {
                corrupt("a changed file's values are not parallel to its mentions")
            })?;
            // `max(1, st_value)`: the full link's (`tables::resolved_row`).
            (row.typ, row.bind, row.other, row.value) = (x.typ, x.bind, x.other, v.value.max(1));
            return Ok(row);
        }
        if let Some(old) = old
            && old.kind == def_kind::COMMON
            && self.old_definer(key, old)? == Some((winner.file, Class::Common))
        {
            (row.typ, row.bind, row.other, row.value) = (old.typ, old.bind, old.other, old.value);
            return Ok(row);
        }
        let (_, x) = self.stored_mention(winner.file, key)?.ok_or_else(|| {
            corrupt(format!("KEY_REFS names key {key} in file {}, MENTIONS does not", winner.file))
        })?;
        let align = self
            .view()
            .commons(SymbolId(key))?
            .iter()
            .find(|c| c.file == winner.file)
            .map(|c| c.align)
            .ok_or_else(|| {
                corrupt(format!("no COMMONS row of key {key} for file {}", winner.file))
            })?;
        (row.typ, row.bind, row.other, row.value) = (x.typ, x.bind, x.other, align);
        Ok(row)
    }

    /// `key`'s `COMMONS` rows in the update's link: the stored rows of
    /// unedited live files, the edited live files' from their values.
    fn common_rows(&self, k: u32, ments: &[Ment]) -> Result<Vec<CommonRow>, IncrReason> {
        let mut rows = Vec::new();
        if k < self.cx.symbols {
            for c in self.view().commons(SymbolId(k))? {
                if !self.edited(c.file) && self.live_new(c.file)? {
                    rows.push(c);
                }
            }
        }
        for m in ments {
            if m.def != def_kind::COMMON || !self.edited(m.file) || !self.live_new(m.file)? {
                continue;
            }
            let Some((i, p)) = m.new else { continue };
            let v = self.cx.changed.files[i].values.get(p).ok_or_else(|| {
                corrupt("a changed file's values are not parallel to its mentions")
            })?;
            rows.push(CommonRow {
                key: k,
                file: m.file,
                size: v.size,
                align: v.value.max(1),
                tomb: false,
            });
        }
        Ok(rows)
    }

    /// `COMMONS` edits of `key`.
    fn commons(&mut self, k: u32, ments: &[Ment], old: &[Ment]) -> Result<(), IncrReason> {
        if !ments.iter().chain(old).any(|m| m.def == def_kind::COMMON) {
            return Ok(());
        }
        let new = self.common_rows(k, ments)?;
        let stored =
            if k < self.cx.symbols { self.view().commons(SymbolId(k))? } else { Vec::new() };
        for r in &stored {
            if !new.iter().any(|n| n.file == r.file) {
                self.out.commons.delete.push(*r);
            }
        }
        for n in new {
            if self.edited(n.file) {
                self.out.commons.put.push(n);
            }
        }
        Ok(())
    }

    /// IN-26's attribute list (IN-71): `symbol-kind-changed` for a key an
    /// unchanged live file mentions.
    fn kind_check(
        &self,
        k: u32,
        old: &ResolvedRow,
        new: &ResolvedRow,
        ments: &[Ment],
    ) -> Result<(), IncrReason> {
        let mut referrer = false;
        for m in ments {
            if m.file != 0 && !self.edited(m.file) && self.live_new(m.file)? {
                referrer = true;
                break;
            }
        }
        if !referrer {
            return Ok(());
        }
        let defined = |r: &ResolvedRow| {
            matches!(r.kind, def_kind::SECTION | def_kind::ABSOLUTE | def_kind::COMMON)
        };
        let why = if (old.typ == STT_GNU_IFUNC) != (new.typ == STT_GNU_IFUNC) {
            Some("to or from STT_GNU_IFUNC")
        } else if (old.typ == STT_TLS) != (new.typ == STT_TLS) {
            Some("to or from STT_TLS")
        } else if matches!(
            (old.kind, new.kind),
            (def_kind::ABSOLUTE, def_kind::SECTION) | (def_kind::SECTION, def_kind::ABSOLUTE)
        ) {
            Some("absolute to or from a section")
        } else if defined(old) && defined(new) && (old.other & !3) != (new.other & !3) {
            Some("st_other bits other than the visibility")
        } else {
            None
        };
        match why {
            Some(w) => {
                let (name, _) = self.cx.name_of(k)?;
                Err(IncrReason::SymbolKindChanged(format!(
                    "`{}`: {w}",
                    String::from_utf8_lossy(&name)
                )))
            }
            None => Ok(()),
        }
    }

    /// IN-71's `at_file`/`at_index`: the first live mentioner by current
    /// rank (file 0 aside) and its entry among its non-local entries.
    fn at(&mut self, k: u32, ments: &[Ment]) -> Result<(), IncrReason> {
        let mut best: Option<((u64, u32), Ment)> = None;
        for m in ments {
            if m.file != 0 && self.live_new(m.file)? {
                let at = self.cx.order(m.file)?;
                if best.is_none_or(|b| at < b.0) {
                    best = Some((at, *m));
                }
            }
        }
        let stored =
            if k < self.cx.symbols { Some(self.view().symbol(SymbolId(k))?) } else { None };
        let at = match best {
            None => (NONE, 0),
            Some((_, m)) => match m.new {
                Some((_, p)) => (m.file, p as u32),
                None => match stored {
                    Some(s) if s.at_file == m.file => (s.at_file, s.at_index),
                    _ => {
                        let (p, _) = self.stored_mention(m.file, k)?.ok_or_else(|| {
                            corrupt(format!(
                                "KEY_REFS names key {k} in file {}, MENTIONS does not",
                                m.file
                            ))
                        })?;
                        (m.file, p)
                    }
                },
            },
        };
        match stored {
            Some(s) => {
                if (s.at_file, s.at_index) != at {
                    self.out.at.push((SymbolId(k), at.0, at.1));
                }
            }
            None => self.out.at.push((SymbolId(k), at.0, at.1)),
        }
        Ok(())
    }

    /// `KEY_REFS` edits: an edited file's rows replaced by its new ones; a
    /// gone file's deleted; a member whose extraction flips keeps only its
    /// defined and common rows while `LAZY` (flag set), all of them while
    /// live.
    fn key_refs(&mut self) -> Result<(), IncrReason> {
        let mut files: BTreeMap<u32, Option<usize>> =
            self.unextracted.iter().map(|&f| (f, None)).collect();
        for (i, c) in self.cx.changed.files.iter().enumerate() {
            if c.change != Change::DrawnIn {
                files.insert(c.file.0, Some(i));
            }
        }
        for (f, i) in files {
            let live = self.live_new(f)?;
            let member = self.cx.facts(f)?.member;
            let lazy = member && !live;
            let mut put: BTreeMap<u32, KeyRefRow> = BTreeMap::new();
            if let Some(i) = i
                && let Some(c) = self.fresh(f)
            {
                let ids = &self.cx.ids[i];
                for m in &c.mentions {
                    let k = ids
                        .get(m.symbol as usize)
                        .ok_or_else(|| corrupt("a changed file's mention names no key"))?
                        .0;
                    put.entry(k).or_insert_with(|| key_ref_row(k, f, m, lazy));
                }
            } else if self.unextracted.contains(&f) && self.cx.facts(f)?.stored {
                for m in self.stored_ments(f)?.iter() {
                    put.entry(m.symbol).or_insert_with(|| key_ref_row(m.symbol, f, m, lazy));
                }
            }
            if lazy {
                put.retain(|_, r| r.def != def_kind::UNDEFINED);
            }
            let x = self.cx.facts(f)?;
            if x.stored && x.change != Some(Change::Appeared) {
                for m in self.stored_ments(f)?.iter() {
                    if !put.contains_key(&m.symbol) {
                        let row = KeyRefRow { key: m.symbol, file: f, ..KeyRefRow::default() };
                        if !self.out.key_refs.delete.contains(&row) {
                            self.out.key_refs.delete.push(row);
                        }
                    }
                }
            }
            self.out.key_refs.put.extend(put.into_values());
        }
        Ok(())
    }

    /// `new_keys`' rows: a new global's `at` as [`Self::at`] found it; a
    /// local's own file and index.
    fn new_keys(&mut self) {
        let at: BTreeMap<u32, (u32, u32)> =
            self.out.at.iter().map(|&(k, f, i)| (k.0, (f, i))).collect();
        for (i, n) in self.cx.new_keys.iter().enumerate() {
            let id = self.cx.symbols + i as u32;
            let row = if n.kind == sym_kind::LOCAL {
                SymbolRow {
                    kind: sym_kind::LOCAL,
                    file: n.file,
                    ordinal: 0,
                    name: Str::default(),
                    version: Str::default(),
                    at_file: n.file,
                    at_index: n.index,
                }
            } else {
                let (at_file, at_index) = at.get(&id).copied().unwrap_or((NONE, 0));
                SymbolRow {
                    kind: n.kind,
                    file: NONE,
                    ordinal: 0,
                    name: Str::default(),
                    version: Str::default(),
                    at_file,
                    at_index,
                }
            };
            self.out.new_keys.push(NewKey {
                row,
                name: n.name.clone(),
                version: n.version.clone(),
            });
        }
        self.out.at.retain(|&(k, _, _)| k.0 < self.cx.symbols);
    }
}

/// A `KEY_REFS` row of a fresh or stored mention.
fn key_ref_row(key: u32, file: u32, m: &MentionRow, lazy: bool) -> KeyRefRow {
    let mut flags = m.other & key_ref_flags::VISIBILITY;
    if m.kind == def_kind::SECTION {
        flags |= key_ref_flags::IN_SECTION;
    }
    if lazy {
        flags |= key_ref_flags::LAZY;
    }
    // `demand` is not known here: the join ORs a changed file's
    // `FileDemands` into its rows after the scan (WP-239 S4-4, S4-5); a
    // lazy member has no live unit and keeps 0.
    KeyRefRow { key, file, def: m.kind, bind: m.bind, typ: m.typ, flags, demand: 0, tomb: false }
}

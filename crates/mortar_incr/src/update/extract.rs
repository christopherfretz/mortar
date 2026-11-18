//! Archive extraction in an update (`spec/incr.md` IN-71b; IN-70 step
//! U5): members newly extracted and no longer extracted, with their
//! rounds. WP-213 (U-1): the type [`Extraction`] and the entry point
//! [`extract`]; **filled by U-3** (the function only).
//!
//! The update's link ([`Ctx`]) is shared with `resolve` and `live`: the
//! stored rows with the changed files' rows in place of their old ones,
//! every "first", "least" and "lowest" by the files' **current**
//! `FILES.rank` (WP-215's correction; never by row number).

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};

use mortar_core::incr::{IncrReason, OptionKeys, Unimplemented};
use mortar_core::{FileId, SymbolId};
use mortar_elf::consts::{STB_WEAK, STT_FUNC, STT_GNU_IFUNC};

use super::rows::{KeyRefRow, file_flags, key_ref_flags, sym_kind};
use super::view::{Change, ChangedFile, ChangedFiles, StateView};
use crate::format::{def_kind, kind};

/// The members whose extraction changed (WP-211 §C `Extraction`; IN-71b).
/// Written by U-3; read by U-3 (`resolve`), the join (an extracted member
/// is read by the front and joins `ChangedFiles` as `Extracted`), U-2
/// (`FILES` flags and rounds).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Extraction {
    /// `LAZY` members now extracted and their round (`FILES` rows),
    /// sorted.
    pub extracted: Vec<(FileId, u32)>,
    /// Members no longer extracted (they become `LAZY`), sorted.
    pub unextracted: Vec<FileId>,
    /// Members still extracted whose stored round changed (any supporting
    /// value, IN-71b), sorted.
    pub rounds: Vec<(FileId, u32)>,
}

/// IN-71b's bound (a policy constant, *argued, not run*): more members
/// flipping than this is `extraction-changed`. Drawn-in files count toward
/// IN-16's bound besides (the join's).
pub const FLIP_BOUND: usize = 1024;

/// IN-71b's bound on a removal cone's re-seeding: more `KEY_REFS` rows read
/// than this is `extraction-changed`.
pub const RESEED_ROWS_BOUND: u64 = 65_536;

/// The extraction of the update (WP-211 §C; IN-71b). Called by the join
/// before `resolve` (IN-70 U5). `options`: the link's root keys (§4.2's
/// root edges, IN-71b), passed in (WP-215 D3-D5).
///
/// The result is relative to the stored link and complete for what
/// `changed` holds: a member it extracts whose rows `changed` does not
/// hold yet (a `LAZY` member, whose undefined mentions the state does not
/// store) has no out-edges here; the join reads it (U4, `Extracted`) and
/// calls again, until every member of `extracted` is in `changed` (IN-71
/// "U4 and U5 iterate": each call only adds files).
///
/// # Errors
/// `extraction-changed` (IN-71b's bound), `closure-iterated` (the stored
/// link iterated, `META.iterations`), `update-not-implemented
/// (versioned)` (D8), `state-corrupt`.
pub fn extract(
    view: &StateView<'_>,
    changed: &ChangedFiles,
    options: &OptionKeys,
) -> Result<Extraction, IncrReason> {
    let cx = Ctx::new(view, changed, options)?;
    cx.extraction()
}

fn corrupt(what: impl std::fmt::Display) -> IncrReason {
    IncrReason::StateCorrupt(what.to_string())
}

/// An option's operand as its key's name and version
/// (`mortar_core::incr::OptionKey::name`, as `key_of` splits it): `foo@V`
/// is `(foo, V)`, `foo@@V` is `foo` with no version.
pub(crate) fn split_version(name: &[u8]) -> (&[u8], &[u8]) {
    match name.iter().position(|&b| b == b'@') {
        None => (name, b""),
        Some(i) if name.get(i + 1) == Some(&b'@') => (&name[..i], b""),
        Some(i) => (&name[..i], &name[i + 1..]),
    }
}

fn to_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// What the update knows of a file (stored `FILES` row, or a changed
/// file's).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Facts {
    /// Its current rank (an appeared file's is the join's,
    /// `ChangedFile::row.rank`; every other file keeps its stored rank).
    pub rank: u64,
    /// `FILES.kind` (0 internal, 1 relocatable object, 2 shared object).
    pub kind: u8,
    /// resolve.md §4.1's lazy file (IN-71b's *member*): from an archive
    /// and not under `--whole-archive`, extracted or not. A stored member
    /// is one when `LAZY` or its round is not 0 (an extracted member's
    /// round is at least 1 by the support property; a `--whole-archive`
    /// member's is 0, as a named file's).
    pub member: bool,
    /// Live in the stored link (not `LAZY`, not `GONE`).
    pub live_old: bool,
    /// Its stored round (0 unless an extracted member).
    pub round: u32,
    /// Its change, when `changed` names it.
    pub change: Option<Change>,
    /// It has a stored `FILES` row (not appeared) that is not `GONE`.
    pub stored: bool,
}

/// A mention of a key in the update's link: a stored `KEY_REFS` row or a
/// changed file's fresh `MENTIONS` row (`new`: its changed file and
/// position).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ment {
    pub file: u32,
    pub def: u8,
    pub bind: u8,
    pub typ: u8,
    /// `KEY_REFS.flags`' visibility and `IN_SECTION` bits.
    pub flags: u8,
    pub new: Option<(usize, usize)>,
}

impl Ment {
    pub fn is_defined(&self) -> bool {
        matches!(self.def, def_kind::SECTION | def_kind::ABSOLUTE)
    }
    fn strong_undefined(&self) -> bool {
        self.def == def_kind::UNDEFINED && self.bind != STB_WEAK
    }
}

/// `first(K)` and `cfirst(K)` (resolve.md §4.1): `FILES` rows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Targets {
    pub first: Option<u32>,
    pub cfirst: Option<u32>,
}

/// A key the update adds (`ResolveDelta::new_keys`' names).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewName {
    pub name: Vec<u8>,
    pub version: Vec<u8>,
    pub kind: u8,
    /// A local's file and index.
    pub file: u32,
    pub index: u32,
}

/// An edge of §4.1: (source file, target member).
type Edge = (u32, u32);

/// The update's link: the stored rows, the changed files' rows in place of
/// their old ones, and the options' keys.
pub(crate) struct Ctx<'a, 's> {
    pub view: &'a StateView<'s>,
    pub changed: &'a ChangedFiles,
    pub options: &'a OptionKeys,
    /// The state's `SYMBOLS` count: a new key's id is this plus its
    /// position in `new_keys`.
    pub symbols: u32,
    cf: BTreeMap<u32, usize>,
    /// Per changed file, its `keys` as `SymbolId`s.
    pub ids: Vec<Vec<SymbolId>>,
    pub new_keys: Vec<NewName>,
    new_by_name: BTreeMap<(Vec<u8>, Vec<u8>), u32>,
    /// The changed files' fresh mentions by key: (changed file, position).
    by_key: BTreeMap<u32, Vec<(usize, usize)>>,
    facts: RefCell<BTreeMap<u32, Facts>>,
    refs: RefCell<BTreeMap<u32, std::rc::Rc<Vec<KeyRefRow>>>>,
    targets_new: RefCell<BTreeMap<u32, Targets>>,
    /// `KEY_REFS` rows read (IN-71b's bound counts those of a re-seeding).
    pub rows_read: Cell<u64>,
    pub defsym: BTreeSet<u32>,
    /// The root keys and their `arg` (§4.2).
    pub roots: Vec<(u32, u32)>,
}

impl<'a, 's> Ctx<'a, 's> {
    /// The update's link over `view` and `changed`; the stored link's and
    /// the versions' checks (IN-71b Iterations, D8) first.
    pub fn new(
        view: &'a StateView<'s>,
        changed: &'a ChangedFiles,
        options: &'a OptionKeys,
    ) -> Result<Self, IncrReason> {
        let meta = view.meta()?;
        if meta.iterations > 1 {
            return Err(IncrReason::ClosureIterated(format!(
                "the stored link's closure ran {} iterations",
                meta.iterations
            )));
        }
        if meta.versioned != 0 || changed.files.iter().any(|f| f.versioned != 0) {
            return Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Versioned)));
        }
        let symbols = u32::try_from(view.count(kind::SYMBOLS))
            .map_err(|_| corrupt("SYMBOLS: count past u32"))?;
        let mut cx = Self {
            view,
            changed,
            options,
            symbols,
            cf: BTreeMap::new(),
            ids: Vec::new(),
            new_keys: Vec::new(),
            new_by_name: BTreeMap::new(),
            by_key: BTreeMap::new(),
            facts: RefCell::new(BTreeMap::new()),
            refs: RefCell::new(BTreeMap::new()),
            targets_new: RefCell::new(BTreeMap::new()),
            rows_read: Cell::new(0),
            defsym: BTreeSet::new(),
            roots: Vec::new(),
        };
        for (i, f) in changed.files.iter().enumerate() {
            if cx.cf.insert(f.file.0, i).is_some() {
                return Err(corrupt(format!("file {} changed twice", f.file.0)));
            }
            let mut ids = Vec::with_capacity(f.keys.len());
            for k in &f.keys {
                let id = if k.kind == sym_kind::LOCAL {
                    cx.add_key(NewName {
                        name: k.name.clone(),
                        version: k.version.clone(),
                        kind: sym_kind::LOCAL,
                        file: f.file.0,
                        index: k.index,
                    })
                } else {
                    cx.global(&k.name, &k.version, true)?.unwrap_or(u32::MAX)
                };
                ids.push(SymbolId(id));
            }
            if !matches!(f.change, Change::Gone | Change::NoLongerExtracted) {
                for (p, m) in f.mentions.iter().enumerate() {
                    let id = ids
                        .get(m.symbol as usize)
                        .ok_or_else(|| corrupt("a changed file's mention names no key"))?;
                    cx.by_key.entry(id.0).or_default().push((i, p));
                }
            }
            cx.ids.push(ids);
        }
        for d in options.defsym_names() {
            if let Some(k) = cx.global(d, b"", false)? {
                cx.defsym.insert(k);
            }
        }
        for (name, arg) in options.roots() {
            let (name, version) = split_version(name);
            if let Some(k) = cx.global(name, version, false)? {
                cx.roots.push((k, arg));
            }
        }
        Ok(cx)
    }

    fn add_key(&mut self, n: NewName) -> u32 {
        let id = self.symbols + to_u32(self.new_keys.len());
        if n.kind != sym_kind::LOCAL {
            self.new_by_name.insert((n.name.clone(), n.version.clone()), id);
        }
        self.new_keys.push(n);
        id
    }

    /// The global key `(name, version)`: a stored row, a key a changed file
    /// added, or (`add`) a new one.
    fn global(
        &mut self,
        name: &[u8],
        version: &[u8],
        add: bool,
    ) -> Result<Option<u32>, IncrReason> {
        if let Some(k) = self.view.find_key(name, version)? {
            return Ok(Some(k.0));
        }
        if let Some(&k) = self.new_by_name.get(&(name.to_vec(), version.to_vec())) {
            return Ok(Some(k));
        }
        if !add {
            return Ok(None);
        }
        let kind = if version.is_empty() { sym_kind::GLOBAL } else { sym_kind::VERSIONED };
        Ok(Some(self.add_key(NewName {
            name: name.to_vec(),
            version: version.to_vec(),
            kind,
            file: crate::format::NONE,
            index: 0,
        })))
    }

    /// The unversioned global named `name`, if the update's link has it.
    pub fn key_named(&self, name: &[u8]) -> Result<Option<u32>, IncrReason> {
        if let Some(k) = self.view.find_key(name, b"")? {
            return Ok(Some(k.0));
        }
        Ok(self.new_by_name.get(&(name.to_vec(), Vec::new())).copied())
    }

    /// A key's name and whether it is versioned (or local).
    pub fn name_of(&self, key: u32) -> Result<(Vec<u8>, bool), IncrReason> {
        if key < self.symbols {
            let s = self.view.symbol(SymbolId(key))?;
            let plain = matches!(s.kind, sym_kind::GLOBAL | sym_kind::GONE)
                && self.view.string(s.version)?.is_empty();
            return Ok((self.view.string(s.name)?, !plain));
        }
        let n = self
            .new_keys
            .get((key - self.symbols) as usize)
            .ok_or_else(|| corrupt("a key out of range"))?;
        Ok((n.name.clone(), n.kind != sym_kind::GLOBAL))
    }

    /// The changed file `file`, if `changed` names it.
    pub fn changed_file(&self, file: u32) -> Option<&'a ChangedFile> {
        self.cf.get(&file).map(|&i| &self.changed.files[i])
    }

    /// What the update knows of `file`.
    pub fn facts(&self, file: u32) -> Result<Facts, IncrReason> {
        if let Some(f) = self.facts.borrow().get(&file) {
            return Ok(*f);
        }
        let cf = self.changed_file(file);
        let facts = match cf {
            Some(c) if c.change == Change::Appeared => {
                let member =
                    c.row.member != 0 && (c.row.flags & file_flags::LAZY != 0 || c.row.round > 0);
                Facts {
                    rank: c.row.rank,
                    kind: c.row.kind,
                    member,
                    live_old: false,
                    round: 0,
                    change: Some(c.change),
                    stored: false,
                }
            }
            _ => {
                let row = self.view.file(FileId(file))?;
                let lazy = row.flags & file_flags::LAZY != 0;
                let gone = row.flags & file_flags::GONE != 0;
                let member = row.member != 0 && (lazy || row.round > 0);
                Facts {
                    rank: row.rank,
                    kind: row.kind,
                    member,
                    live_old: !lazy && !gone,
                    round: if member && !lazy { row.round } else { 0 },
                    change: cf.map(|c| c.change),
                    stored: !gone,
                }
            }
        };
        self.facts.borrow_mut().insert(file, facts);
        Ok(facts)
    }

    /// The order of "first", "least", "lowest": `(rank, row)`.
    pub fn order(&self, file: u32) -> Result<(u64, u32), IncrReason> {
        Ok((self.facts(file)?.rank, file))
    }

    /// Live before any extraction decision of this update: the stored
    /// liveness, without gone files, with appeared files that are not
    /// members; `Some(round)`.
    pub fn live_base(&self, file: u32) -> Result<Option<u32>, IncrReason> {
        let f = self.facts(file)?;
        Ok(match f.change {
            Some(Change::Gone) => None,
            Some(Change::Appeared) => (!f.member).then_some(0),
            _ => f.live_old.then_some(f.round),
        })
    }

    /// A key's stored `KEY_REFS` rows (counted).
    pub fn stored_refs(&self, key: u32) -> Result<std::rc::Rc<Vec<KeyRefRow>>, IncrReason> {
        if let Some(r) = self.refs.borrow().get(&key) {
            return Ok(r.clone());
        }
        let mut rows =
            if key < self.symbols { self.view.key_refs(SymbolId(key))? } else { Vec::new() };
        rows.retain(|r| !r.tomb);
        self.rows_read.set(self.rows_read.get() + rows.len() as u64);
        let rows = std::rc::Rc::new(rows);
        self.refs.borrow_mut().insert(key, rows.clone());
        Ok(rows)
    }

    fn ment_of_ref(r: &KeyRefRow) -> Ment {
        Ment {
            file: r.file,
            def: r.def,
            bind: r.bind,
            typ: r.typ,
            flags: r.flags & (key_ref_flags::VISIBILITY | key_ref_flags::IN_SECTION),
            new: None,
        }
    }

    /// A key's mentions in the stored link (stored rows of files not
    /// `GONE`).
    pub fn ments_old(&self, key: u32) -> Result<Vec<Ment>, IncrReason> {
        let mut out = Vec::new();
        for r in self.stored_refs(key)?.iter() {
            if self.facts(r.file)?.stored || r.file == 0 {
                out.push(Self::ment_of_ref(r));
            }
        }
        Ok(out)
    }

    /// A key's mentions in the update's link, one per file: the stored
    /// rows of unchanged files (a no-longer-extracted member keeps its
    /// defined and common rows), and the changed files' fresh rows.
    pub fn ments_new(&self, key: u32) -> Result<Vec<Ment>, IncrReason> {
        let mut out = Vec::new();
        for r in self.stored_refs(key)?.iter() {
            if r.file == 0 {
                out.push(Self::ment_of_ref(r));
                continue;
            }
            let f = self.facts(r.file)?;
            if !f.stored {
                continue;
            }
            match f.change {
                None => out.push(Self::ment_of_ref(r)),
                Some(Change::NoLongerExtracted) if r.def != def_kind::UNDEFINED => {
                    out.push(Self::ment_of_ref(r));
                }
                Some(_) => {}
            }
        }
        if let Some(list) = self.by_key.get(&key) {
            let mut seen = BTreeSet::new();
            for &(i, p) in list {
                let c = &self.changed.files[i];
                if !seen.insert(c.file.0) {
                    continue;
                }
                let m = c.mentions[p];
                let mut flags = m.other & key_ref_flags::VISIBILITY;
                if m.kind == def_kind::SECTION {
                    flags |= key_ref_flags::IN_SECTION;
                }
                out.push(Ment {
                    file: c.file.0,
                    def: m.kind,
                    bind: m.bind,
                    typ: m.typ,
                    flags,
                    new: Some((i, p)),
                });
            }
        }
        Ok(out)
    }

    /// `first(K)`, `cfirst(K)` over `ments` (resolve.md §4.1, as
    /// `mortar_resolve::live::targets`: file 0 does not count, a
    /// `--defsym` key has none), by current rank.
    pub fn targets_of(&self, key: u32, ments: &[Ment]) -> Result<Targets, IncrReason> {
        if self.defsym.contains(&key) {
            return Ok(Targets::default());
        }
        let mut first: Option<(u64, u32)> = None;
        let mut cfirst: Option<(u64, u32)> = None;
        for m in ments {
            if m.file == 0 || !(m.is_defined() || m.def == def_kind::COMMON) {
                continue;
            }
            let at = self.order(m.file)?;
            if first.is_none_or(|f| at < f) {
                first = Some(at);
            }
            let strong = m.is_defined() && m.bind != STB_WEAK;
            let data = !matches!(m.typ, STT_FUNC | STT_GNU_IFUNC);
            if strong && (!self.facts(m.file)?.member || data) && cfirst.is_none_or(|c| at < c) {
                cfirst = Some(at);
            }
        }
        Ok(Targets { first: first.map(|f| f.1), cfirst: cfirst.map(|f| f.1) })
    }

    /// The update's `first(K)`, `cfirst(K)` (memoized).
    pub fn targets_new(&self, key: u32) -> Result<Targets, IncrReason> {
        if let Some(t) = self.targets_new.borrow().get(&key) {
            return Ok(*t);
        }
        let t = self.targets_of(key, &self.ments_new(key)?)?;
        self.targets_new.borrow_mut().insert(key, t);
        Ok(t)
    }

    /// The key an undefined mention of `key` binds to (`--wrap`, §7.1).
    pub fn redirect(&self, key: u32) -> Result<u32, IncrReason> {
        if self.options.wrap.is_empty() {
            return Ok(key);
        }
        let (name, versioned) = self.name_of(key)?;
        if versioned {
            return Ok(key);
        }
        match self.options.redirect(&name) {
            Some(to) => Ok(self.key_named(&to)?.unwrap_or(key)),
            None => Ok(key),
        }
    }

    /// The keys whose undefined mentions bind to `key` (§7.1 inverted).
    pub fn sources(&self, key: u32) -> Result<Vec<u32>, IncrReason> {
        if self.options.wrap.is_empty() {
            return Ok(vec![key]);
        }
        let (name, versioned) = self.name_of(key)?;
        if versioned {
            return Ok(vec![key]);
        }
        let mut out = Vec::new();
        let mut cands = vec![name.clone(), [b"__real_".as_slice(), &name].concat()];
        if let Some(s) = name.strip_prefix(b"__wrap_".as_slice()) {
            cands.push(s.to_vec());
        }
        for c in cands {
            let binds = self.options.redirect(&c).unwrap_or_else(|| c.clone());
            if binds == name
                && let Some(k) = self.key_named(&c)?
                && !out.contains(&k)
            {
                out.push(k);
            }
        }
        Ok(out)
    }

    /// Whether `file` is a member (resolve.md §4.1's lazy file).
    fn member(&self, file: u32) -> Result<bool, IncrReason> {
        Ok(self.facts(file)?.member)
    }

    /// The edges into `first(K)`/`cfirst(K)` (§4.1 edges 1-3) of the
    /// update's link, sources live by `live`.
    fn edges_new(
        &self,
        key: u32,
        live: &dyn Fn(u32) -> Result<Option<u32>, IncrReason>,
        out: &mut BTreeSet<Edge>,
    ) -> Result<(), IncrReason> {
        let t = self.targets_new(key)?;
        if let Some(f) = t.first.filter(|&f| self.member(f).unwrap_or(false)) {
            for s in self.sources(key)? {
                for m in self.ments_new(s)? {
                    if m.strong_undefined() && live(m.file)?.is_some() {
                        out.insert((m.file, f));
                    }
                }
            }
            if self.roots.iter().any(|r| r.0 == key) {
                out.insert((0, f));
            }
        }
        if let Some(c) = t.cfirst.filter(|&c| self.member(c).unwrap_or(false)) {
            for m in self.ments_new(key)? {
                if m.def == def_kind::COMMON && live(m.file)?.is_some() {
                    out.insert((m.file, c));
                }
            }
        }
        Ok(())
    }

    /// The edges of the stored link into the stored `first(K)`/`cfirst(K)`.
    fn edges_old(&self, key: u32, out: &mut BTreeSet<Edge>) -> Result<(), IncrReason> {
        let t = self.targets_of(key, &self.ments_old(key)?)?;
        let member = |f: u32| -> Result<bool, IncrReason> {
            let x = self.facts(f)?;
            Ok(x.stored && x.member)
        };
        if let Some(f) = t.first
            && member(f)?
        {
            for s in self.sources(key)? {
                for m in self.ments_old(s)? {
                    if m.strong_undefined() && self.facts(m.file)?.live_old {
                        out.insert((m.file, f));
                    }
                }
            }
            if self.roots.iter().any(|r| r.0 == key) {
                out.insert((0, f));
            }
        }
        if let Some(c) = t.cfirst
            && member(c)?
        {
            for m in self.ments_old(key)? {
                if m.def == def_kind::COMMON && self.facts(m.file)?.live_old {
                    out.insert((m.file, c));
                }
            }
        }
        Ok(())
    }

    /// A file's mentions in the update's link as `(key, MentionRow)`:
    /// a changed file's fresh rows, else its stored `MENTIONS` range;
    /// `None` when they are not known (a `LAZY` member's undefined
    /// mentions are not stored, IN-71b: the join reads it).
    pub fn file_ments(
        &self,
        file: u32,
    ) -> Result<Option<Vec<(u32, super::rows::MentionRow)>>, IncrReason> {
        if let Some(c) = self.changed_file(file) {
            if matches!(c.change, Change::Gone | Change::NoLongerExtracted) {
                return Ok(Some(Vec::new()));
            }
            let ids = &self.ids[self.cf[&file]];
            return Ok(Some(c.mentions.iter().map(|m| (ids[m.symbol as usize].0, *m)).collect()));
        }
        let f = self.facts(file)?;
        if !f.stored || !f.live_old {
            return Ok(None);
        }
        Ok(Some(self.view.mentions_of(FileId(file))?.into_iter().map(|m| (m.symbol, m)).collect()))
    }

    /// The members `file`'s mentions have an edge to (§4.1 edges 1-2), or
    /// `None` when its mentions are not known.
    fn out_targets(&self, file: u32) -> Result<Option<BTreeSet<u32>>, IncrReason> {
        let Some(ments) = self.file_ments(file)? else { return Ok(None) };
        let mut out = BTreeSet::new();
        for (key, m) in ments {
            let t = if m.kind == def_kind::UNDEFINED && m.bind != STB_WEAK {
                self.targets_new(self.redirect(key)?)?.first
            } else if m.kind == def_kind::COMMON {
                self.targets_new(key)?.cfirst
            } else {
                None
            };
            if let Some(t) = t
                && t != file
                && self.member(t)?
            {
                out.insert(t);
            }
        }
        Ok(Some(out))
    }

    /// The live sources (by `live`) of the edges into member `m`, with
    /// their rounds.
    fn in_edges(
        &self,
        m: u32,
        live: &dyn Fn(u32) -> Result<Option<u32>, IncrReason>,
    ) -> Result<Vec<(u32, u32)>, IncrReason> {
        let Some(ments) = self.file_ments_defs(m)? else { return Ok(Vec::new()) };
        let mut out = Vec::new();
        for key in ments {
            let t = self.targets_new(key)?;
            if t.first == Some(m) {
                for s in self.sources(key)? {
                    for x in self.ments_new(s)? {
                        if x.strong_undefined()
                            && let Some(r) = live(x.file)?
                        {
                            out.push((x.file, r));
                        }
                    }
                }
                if self.roots.iter().any(|r| r.0 == key) {
                    out.push((0, 0));
                }
            }
            if t.cfirst == Some(m) {
                for x in self.ments_new(key)? {
                    if x.def == def_kind::COMMON
                        && let Some(r) = live(x.file)?
                    {
                        out.push((x.file, r));
                    }
                }
            }
        }
        Ok(out)
    }

    /// The keys a member defines or has a common of (its stored rows: a
    /// member's defined and common mentions are stored, `LAZY` or not).
    fn file_ments_defs(&self, file: u32) -> Result<Option<BTreeSet<u32>>, IncrReason> {
        let rows: Vec<(u32, u8)> = if let Some(c) = self.changed_file(file)
            && !matches!(c.change, Change::NoLongerExtracted)
        {
            let ids = &self.ids[self.cf[&file]];
            c.mentions.iter().map(|m| (ids[m.symbol as usize].0, m.kind)).collect()
        } else {
            if !self.facts(file)?.stored {
                return Ok(None);
            }
            self.view.mentions_of(FileId(file))?.into_iter().map(|m| (m.symbol, m.kind)).collect()
        };
        Ok(Some(
            rows.into_iter()
                .filter(|(_, k)| {
                    matches!(*k, def_kind::SECTION | def_kind::ABSOLUTE | def_kind::COMMON)
                })
                .map(|(k, _)| k)
                .collect(),
        ))
    }

    /// Every key a changed file mentions, old version (`MENTIONS`) or new.
    pub fn changed_keys(&self) -> Result<BTreeSet<u32>, IncrReason> {
        let mut keys: BTreeSet<u32> = self.by_key.keys().copied().collect();
        for c in &self.changed.files {
            if c.change != Change::Appeared && self.facts(c.file.0)?.stored {
                for m in self.view.mentions_of(c.file)? {
                    keys.insert(m.symbol);
                }
            }
        }
        Ok(keys)
    }

    fn check_bound(&self, start: u64) -> Result<(), IncrReason> {
        let read = self.rows_read.get() - start;
        if read > RESEED_ROWS_BOUND {
            return Err(IncrReason::ExtractionChanged(format!(
                "the removal cone's re-seeding read more than {RESEED_ROWS_BOUND} KEY_REFS rows"
            )));
        }
        Ok(())
    }

    /// IN-71b: lost and gained edges of the affected keys; the removal
    /// cone, re-seeded; the forward addition.
    #[allow(clippy::too_many_lines, reason = "IN-71b's steps in order")]
    fn extraction(&self) -> Result<Extraction, IncrReason> {
        let mut affected = self.changed_keys()?;
        for k in affected.clone() {
            affected.insert(self.redirect(k)?);
        }
        let base = |f: u32| self.live_base(f);
        let (mut lost, mut gained) = (BTreeSet::new(), BTreeSet::new());
        for &k in &affected {
            let (mut old, mut new) = (BTreeSet::new(), BTreeSet::new());
            self.edges_old(k, &mut old)?;
            self.edges_new(k, &base, &mut new)?;
            lost.extend(old.difference(&new).copied());
            gained.extend(new.difference(&old).copied());
        }
        // Removal: suspects in round order; a member without an edge from
        // a live file of smaller round outside the cone joins it.
        let start = self.rows_read.get();
        let mut suspects: BTreeSet<(u32, u32)> = BTreeSet::new();
        let suspect = |t: u32, set: &mut BTreeSet<(u32, u32)>| -> Result<(), IncrReason> {
            let f = self.facts(t)?;
            if f.member && f.live_old && f.change != Some(Change::Gone) {
                set.insert((f.round, t));
            }
            Ok(())
        };
        for &(_, t) in &lost {
            suspect(t, &mut suspects)?;
        }
        let mut cone: BTreeSet<u32> = BTreeSet::new();
        let mut ins: BTreeMap<u32, Vec<(u32, u32)>> = BTreeMap::new();
        while let Some((r, m)) = suspects.pop_first() {
            if cone.contains(&m) {
                continue;
            }
            let edges = self.in_edges(m, &base)?;
            let supported = edges.iter().any(|&(s, sr)| sr < r && !cone.contains(&s));
            ins.insert(m, edges);
            if !supported {
                cone.insert(m);
                for t in self.out_targets(m)?.unwrap_or_default() {
                    let f = self.facts(t)?;
                    if f.round > r {
                        suspect(t, &mut suspects)?;
                    }
                }
            }
            self.check_bound(start)?;
        }
        // Re-seeding and addition: from the live files outside the cone,
        // smallest round first; a member reached takes its supporter's
        // round + 1.
        let mut now: BTreeMap<u32, u32> = BTreeMap::new();
        let mut best: BTreeMap<u32, u32> = BTreeMap::new();
        let mut queue: BTreeSet<(u32, u32)> = BTreeSet::new();
        let live1 = |f: u32, now: &BTreeMap<u32, u32>| -> Result<Option<u32>, IncrReason> {
            if let Some(&r) = now.get(&f) {
                return Ok(Some(r));
            }
            if cone.contains(&f) {
                return Ok(None);
            }
            self.live_base(f)
        };
        let push =
            |t: u32, r: u32, best: &mut BTreeMap<u32, u32>, queue: &mut BTreeSet<(u32, u32)>| {
                if best.get(&t).is_none_or(|&b| r < b) {
                    if let Some(b) = best.insert(t, r) {
                        queue.remove(&(b, t));
                    }
                    queue.insert((r, t));
                }
            };
        for &(s, t) in &gained {
            if let Some(sr) = live1(s, &now)?
                && live1(t, &now)?.is_none()
            {
                push(t, sr.saturating_add(1), &mut best, &mut queue);
            }
        }
        for (&c, edges) in &ins {
            if !cone.contains(&c) {
                continue;
            }
            for &(s, sr) in edges {
                if !cone.contains(&s) {
                    push(c, sr.saturating_add(1), &mut best, &mut queue);
                }
            }
        }
        while let Some((r, m)) = queue.pop_first() {
            if live1(m, &now)?.is_some() {
                continue;
            }
            now.insert(m, r);
            if let Some(targets) = self.out_targets(m)? {
                for t in targets {
                    if live1(t, &now)?.is_none() {
                        push(t, r.saturating_add(1), &mut best, &mut queue);
                    }
                }
            }
            self.check_bound(start)?;
        }
        let mut x = Extraction::default();
        for (&m, &r) in &now {
            let f = self.facts(m)?;
            if cone.contains(&m) {
                if r != f.round {
                    x.rounds.push((FileId(m), r));
                }
            } else {
                x.extracted.push((FileId(m), r));
            }
        }
        x.unextracted = cone.iter().filter(|c| !now.contains_key(c)).map(|&c| FileId(c)).collect();
        let flips = x.extracted.len() + x.unextracted.len();
        if flips > FLIP_BOUND {
            return Err(IncrReason::ExtractionChanged(format!(
                "{flips} members flip, more than {FLIP_BOUND}"
            )));
        }
        Ok(x)
    }
}

// The resolution model shared by `crates/mortar_incr/tests/update_resolve.rs`
// and `crates/mortar/tests/update_feedback.rs` (`include!`d by both, WP-215):
// small links by mention, their full link written from resolve.md §4.1,
// §3.2, §3.3, §3.5, §6.1, §6.2 (not from IN-71), the state such a link
// stores, the join's `ChangedFiles` and its `extract`/`resolve` loop.

use std::collections::{BTreeMap, BTreeSet};

use mortar_core::incr::{IncrReason, OptionKey, OptionKeys, Unimplemented};
use mortar_core::{FileId, SectionId};
use mortar_incr::format::{NONE, SectionRow, Str, def_kind};
use mortar_incr::update::rows::{
    CommonRow, FileRow, GroupRow, KeyRefRow, MentionRow, ResolvedRow, SymbolRow, file_flags,
    key_ref_flags, sym_kind,
};
use mortar_incr::update::testing::MemoryState;
use mortar_incr::update::view::{
    Change, ChangedFile, ChangedFiles, KeyName, MentionValue, OdrRows,
};
use mortar_incr::update::{extract, resolve};

const WEAK: u8 = 2;
const GLOBAL: u8 = 1;
const OBJECT: u8 = 1;
const FUNC: u8 = 2;
const HIDDEN: u8 = 2;

/// A mention in the model: key name, kind (`def_kind`), binding, type,
/// visibility, the COMDAT group its section is in (`""`: none), `st_value`
/// (a common's: its alignment) and `st_size`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct M {
    key: String,
    kind: u8,
    bind: u8,
    typ: u8,
    vis: u8,
    sig: String,
    value: u64,
    size: u64,
}

fn ment(k: &str, kind: u8, bind: u8) -> M {
    M { key: k.into(), kind, bind, typ: OBJECT, vis: 0, sig: String::new(), value: 0, size: 0 }
}
fn def(k: &str) -> M {
    ment(k, def_kind::SECTION, GLOBAL)
}
fn weak_def(k: &str) -> M {
    ment(k, def_kind::SECTION, WEAK)
}
fn undef(k: &str) -> M {
    ment(k, def_kind::UNDEFINED, GLOBAL)
}
fn weak_undef(k: &str) -> M {
    ment(k, def_kind::UNDEFINED, WEAK)
}
fn common(k: &str) -> M {
    M { value: 4, size: 8, ..ment(k, def_kind::COMMON, GLOBAL) }
}
fn abs(k: &str, value: u64) -> M {
    M { value, ..ment(k, def_kind::ABSOLUTE, GLOBAL) }
}

impl M {
    /// Defined in a section of COMDAT group `sig`.
    fn group(mut self, sig: &str) -> Self {
        self.sig = sig.into();
        self
    }
    fn val(mut self, value: u64, size: u64) -> Self {
        (self.value, self.size) = (value, size);
        self
    }
    fn func(mut self) -> Self {
        self.typ = FUNC;
        self
    }
    fn hidden(mut self) -> Self {
        self.vis = HIDDEN;
        self
    }
    fn defined(&self) -> bool {
        self.kind == def_kind::SECTION || self.kind == def_kind::ABSOLUTE
    }
}

/// A file of the model link.
#[derive(Debug, Clone)]
struct F {
    rank: u64,
    /// An archive member (not `--whole-archive`).
    member: bool,
    ments: Vec<M>,
}

impl F {
    /// Its sections: a plain one, then one per COMDAT group, in the order
    /// its definitions name them (the state's and the join's order).
    fn sections(&self) -> Vec<String> {
        let mut v = vec![String::new()];
        for m in &self.ments {
            if m.kind == def_kind::SECTION && !v.contains(&m.sig) {
                v.push(m.sig.clone());
            }
        }
        v
    }
    fn local(&self, m: &M) -> u32 {
        self.sections().iter().position(|s| *s == m.sig).unwrap() as u32
    }
    /// The position of its first mention of `key` (`at_index`).
    fn index_of(&self, key: &str) -> Option<u32> {
        self.ments.iter().position(|m| m.key == key).map(|p| p as u32)
    }
}

/// A `RESOLVED` row with its section as `(file, section of the file)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Row {
    kind: u8,
    typ: u8,
    bind: u8,
    other: u8,
    sec: Option<(u32, u32)>,
    value: u64,
    size: u64,
}

/// A full link of the model (resolve.md §4.1, §3.2, §3.3, §3.5, §6.1,
/// §6.2), written from resolve.md, not from IN-71.
#[derive(Debug, Clone, Default)]
struct Full {
    live: BTreeMap<u32, u32>,
    /// A live file's definition that is `first`/`cfirst` of its key is in
    /// a COMDAT copy the keepers drop (§6.2: the closure iterates).
    iterated: bool,
    keeper: BTreeMap<String, u32>,
    rows: BTreeMap<String, Row>,
    /// Keys with two live strong definitions (§3.8).
    clash: BTreeSet<String>,
    /// Referenced by an undefined mention of a live file or a root.
    referenced: BTreeSet<String>,
    /// The first live mentioner by rank and its entry (IN-71's reading).
    at: BTreeMap<String, (u32, u32)>,
    /// The defining file of each defined or common key.
    by: BTreeMap<String, u32>,
}

/// A link of the model: files by `FILES` row (row 0 is file 0, the
/// internal file), and the root names.
#[derive(Debug, Clone, Default)]
struct Link {
    files: Vec<Option<F>>,
    roots: Vec<String>,
    /// `--wrap=K` (§7.1).
    wrap: Vec<String>,
    /// `--defsym=K=<number>` (§7.2).
    defsym: Vec<String>,
}

const fn vis_rank(v: u8) -> u8 {
    match v {
        1 | 2 => 0,
        3 => 1,
        _ => 2,
    }
}

const fn rank_vis(r: u8) -> u8 {
    match r {
        0 => 2,
        1 => 3,
        _ => 0,
    }
}

impl Link {
    fn new() -> Self {
        Self {
            files: vec![Some(F { rank: 0, member: false, ments: Vec::new() })],
            roots: Vec::new(),
            wrap: Vec::new(),
            defsym: Vec::new(),
        }
    }
    fn add(&mut self, rank: u64, member: bool, ments: Vec<M>) -> u32 {
        self.files.push(Some(F { rank, member, ments }));
        (self.files.len() - 1) as u32
    }
    fn file(&self, f: u32) -> Option<&F> {
        self.files.get(f as usize).and_then(Option::as_ref)
    }
    /// §7.1: the key an undefined mention of `k` binds to.
    fn redirect(&self, k: &str) -> String {
        if self.wrap.iter().any(|w| w == k) {
            return format!("__wrap_{k}");
        }
        match k.strip_prefix("__real_") {
            Some(w) if self.wrap.iter().any(|x| x == w) => w.to_owned(),
            _ => k.to_owned(),
        }
    }

    fn order(&self, f: u32) -> (u64, u32) {
        (self.file(f).unwrap().rank, f)
    }

    /// `first(K)`, `cfirst(K)` (resolve.md §4.1) by rank.
    #[allow(clippy::type_complexity)]
    fn targets(&self) -> (BTreeMap<&str, (u64, u32)>, BTreeMap<&str, (u64, u32)>) {
        let mut first: BTreeMap<&str, (u64, u32)> = BTreeMap::new();
        let mut cfirst: BTreeMap<&str, (u64, u32)> = BTreeMap::new();
        for (i, f) in self.files.iter().enumerate().skip(1) {
            let Some(f) = f else { continue };
            let at = (f.rank, i as u32);
            for m in f.ments.iter().filter(|m| !self.defsym.contains(&m.key)) {
                if m.defined() || m.kind == def_kind::COMMON {
                    let e = first.entry(&m.key).or_insert(at);
                    *e = (*e).min(at);
                }
                if m.defined() && m.bind != WEAK && (!f.member || m.typ != FUNC) {
                    let e = cfirst.entry(&m.key).or_insert(at);
                    *e = (*e).min(at);
                }
            }
        }
        (first, cfirst)
    }

    /// resolve.md §4.1 from scratch: the live files and a round (the
    /// shortest path from a named file or a root).
    fn closure(&self) -> BTreeMap<u32, u32> {
        let (first, cfirst) = self.targets();
        let mut live: BTreeMap<u32, u32> = BTreeMap::new();
        let mut queue: BTreeSet<(u32, u32)> = BTreeSet::new();
        live.insert(0, 0);
        for (i, f) in self.files.iter().enumerate().skip(1) {
            if let Some(f) = f
                && !f.member
            {
                queue.insert((0, i as u32));
            }
        }
        for r in &self.roots {
            if let Some(&(_, t)) = first.get(r.as_str()) {
                queue.insert((1, t));
            }
        }
        while let Some((r, f)) = queue.pop_first() {
            if live.contains_key(&f) {
                continue;
            }
            live.insert(f, r);
            for m in &self.file(f).unwrap().ments {
                let t = if m.kind == def_kind::UNDEFINED && m.bind != WEAK {
                    first.get(self.redirect(&m.key).as_str())
                } else if m.kind == def_kind::COMMON {
                    cfirst.get(m.key.as_str())
                } else {
                    None
                };
                if let Some(&(_, t)) = t
                    && self.file(t).unwrap().member
                    && !live.contains_key(&t)
                {
                    queue.insert((r + 1, t));
                }
            }
        }
        live
    }

    /// Every key name the link has (a mention, a root), sorted.
    /// The keys an option names: roots, `--defsym` keys, and `--wrap`'s
    /// three names rows (the input phase gives them, model.md §2.3).
    fn option_names(&self) -> BTreeSet<String> {
        let mut v: BTreeSet<String> = self.roots.iter().chain(&self.defsym).cloned().collect();
        for w in &self.wrap {
            v.extend([w.clone(), format!("__wrap_{w}"), format!("__real_{w}")]);
        }
        v
    }

    /// Every key of the link: the options' and the mentioned.
    fn names(&self) -> BTreeSet<String> {
        let mut v = self.option_names();
        for f in self.files.iter().flatten() {
            v.extend(f.ments.iter().map(|m| m.key.clone()));
        }
        v
    }

    /// The full link.
    fn full(&self) -> Full {
        let live = self.closure();
        let (first, cfirst) = self.targets();
        let mut full = Full { live: live.clone(), ..Full::default() };
        let mut keeper: BTreeMap<String, (u64, u32)> = BTreeMap::new();
        for &f in live.keys().filter(|&&f| f != 0) {
            for s in self.file(f).unwrap().sections().into_iter().skip(1) {
                let at = self.order(f);
                let e = keeper.entry(s).or_insert(at);
                *e = (*e).min(at);
            }
        }
        full.keeper = keeper.iter().map(|(s, k)| (s.clone(), k.1)).collect();
        let demoted = |f: u32, m: &M| {
            m.kind == def_kind::SECTION && !m.sig.is_empty() && keeper[&m.sig].1 != f
        };
        for &f in live.keys().filter(|&&f| f != 0) {
            for m in &self.file(f).unwrap().ments {
                let k = m.key.as_str();
                if m.defined()
                    && demoted(f, m)
                    && (first.get(k).map(|x| x.1) == Some(f) || cfirst.get(k).map(|x| x.1) == Some(f))
                {
                    full.iterated = true;
                }
            }
        }
        for k in self.names() {
            // Its own definitions, and the undefined mentions binding to
            // it (§7.1).
            let mut ms: Vec<((u64, u32), &M)> = Vec::new();
            let mut own: Option<(u64, u32)> = None;
            for &f in live.keys().filter(|&&f| f != 0) {
                for m in &self.file(f).unwrap().ments {
                    let to = if m.kind == def_kind::UNDEFINED { self.redirect(&m.key) } else { m.key.clone() };
                    if to == k {
                        ms.push((self.order(f), m));
                    }
                    if m.key == k && own.is_none_or(|o| self.order(f) < o) {
                        own = Some(self.order(f));
                    }
                }
            }
            ms.sort_by_key(|x| x.0);
            let rank = ms.iter().map(|(_, m)| vis_rank(m.vis)).min().unwrap_or(2);
            let row_of = |f: u32, m: &M| Row {
                kind: m.kind,
                typ: m.typ,
                bind: m.bind,
                other: m.vis,
                sec: (m.kind == def_kind::SECTION).then(|| (f, self.file(f).unwrap().local(m))),
                value: m.value,
                size: m.size,
            };
            let strong: Vec<&((u64, u32), &M)> = ms
                .iter()
                .filter(|(at, m)| m.defined() && m.bind != WEAK && !demoted(at.1, m))
                .collect();
            let demoted_strong =
                ms.iter().any(|(at, m)| m.defined() && m.bind != WEAK && demoted(at.1, m));
            if strong.len() > 1 {
                full.clash.insert(k.clone());
            }
            // A demoted definition is a reference (§6.2).
            let refd = self.roots.contains(&k)
                || ms.iter().any(|(at, m)| m.kind == def_kind::UNDEFINED || (m.defined() && demoted(at.1, m)));
            if refd {
                full.referenced.insert(k.clone());
            }
            let row = if self.defsym.contains(&k) {
                Row { kind: def_kind::INTERNAL, typ: 0, bind: 0, other: rank_vis(rank), sec: None, value: 0, size: 0 }
            } else if let Some((at, m)) = strong.first() {
                full.by.insert(k.clone(), at.1);
                row_of(at.1, m)
            } else if let Some((at, m)) = ms.iter().find(|(_, m)| m.kind == def_kind::COMMON) {
                let size = ms
                    .iter()
                    .filter(|(_, m)| m.kind == def_kind::COMMON)
                    .map(|(_, m)| m.size)
                    .max()
                    .unwrap();
                full.by.insert(k.clone(), at.1);
                // The value: `max(1, st_value)` (`tables::resolved_row`).
                Row { size, value: m.value.max(1), ..row_of(at.1, m) }
            } else if let Some((at, m)) = ms
                .iter()
                .find(|(at, m)| m.defined() && m.bind == WEAK && !demoted(at.1, m))
            {
                full.by.insert(k.clone(), at.1);
                row_of(at.1, m)
            } else {
                let strong_ref = demoted_strong
                    || self.roots.contains(&k)
                    || ms.iter().any(|(_, m)| m.kind == def_kind::UNDEFINED && m.bind != WEAK);
                Row {
                    kind: def_kind::UNDEFINED,
                    typ: 0,
                    bind: if strong_ref { 1 } else { 2 },
                    other: rank_vis(rank),
                    sec: None,
                    value: 0,
                    size: 0,
                }
            };
            if let Some((_, f)) = own {
                full.at.insert(k.clone(), (f, self.file(f).unwrap().index_of(&k).unwrap()));
            }
            full.rows.insert(k, row);
        }
        full
    }

    fn options(&self) -> OptionKeys {
        OptionKeys {
            undefined: self
                .roots
                .iter()
                .map(|r| OptionKey { name: r.as_bytes().into(), arg: 1 })
                .collect(),
            wrap: self.wrap.iter().map(|w| w.as_bytes().into()).collect(),
            defsym: self
                .defsym
                .iter()
                .enumerate()
                .map(|(i, d)| mortar_core::incr::OptionDefsym {
                    name: d.as_bytes().into(),
                    value: mortar_core::incr::DefsymOperand::Number(0x1000),
                    arg: 10 + i as u32,
                })
                .collect(),
            ..OptionKeys::default()
        }
    }

    /// The state a full link of this link stores (the rows U-3 reads); the
    /// key names in `keys` (appended to) give the `SYMBOLS` rows.
    fn state(&self, keys: &mut Vec<String>) -> MemoryState {
        let full = self.full();
        let live = &full.live;
        let mut m = MemoryState::default();
        m.meta.iterations = if full.iterated { 2 } else { 1 };
        for k in self.names() {
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
        let empty = m.intern(b"");
        for k in keys.iter() {
            // `foo@V` is the key `(foo, V)` (only a root names one here).
            let (n, v) = k.split_once('@').unwrap_or((k, ""));
            let name = m.intern(n.as_bytes());
            let version = if v.is_empty() { empty } else { m.intern(v.as_bytes()) };
            let (at_file, at_index) = full.at.get(k).copied().unwrap_or((NONE, 0));
            m.symbols.push(SymbolRow {
                kind: if v.is_empty() { sym_kind::GLOBAL } else { sym_kind::VERSIONED },
                file: NONE,
                ordinal: 0,
                name,
                version,
                at_file,
                at_index,
            });
        }
        m.resolved = vec![ResolvedRow::default(); keys.len()];
        let id = |k: &str| keys.iter().position(|x| x == k).unwrap() as u32;
        let mut first_section: BTreeMap<u32, u32> = BTreeMap::new();
        for (i, f) in self.files.iter().enumerate() {
            let file = i as u32;
            let Some(f) = f else {
                m.files.push(FileRow { flags: file_flags::GONE, ..FileRow::default() });
                continue;
            };
            let lazy = !live.contains_key(&file);
            m.files.push(FileRow {
                kind: u8::from(i != 0),
                member: u8::from(f.member),
                flags: if lazy { file_flags::LAZY } else { 0 },
                rank: f.rank,
                round: if lazy { 0 } else { live[&file] },
                ..FileRow::default()
            });
            if i == 0 {
                continue;
            }
            let first = m.sections.len() as u32;
            first_section.insert(file, first);
            for s in f.sections() {
                let (anchor_kind, anchor) =
                    if s.is_empty() { (0, Str::default()) } else { (1, m.intern(s.as_bytes())) };
                let name = m.intern(b".text");
                m.sections.push(SectionRow { file, anchor_kind, anchor, name, ..SectionRow::default() });
            }
            for x in &f.ments {
                let k = id(&x.key);
                let section = if x.kind == def_kind::SECTION { first + f.local(x) } else { NONE };
                if !lazy || x.kind != def_kind::UNDEFINED {
                    m.mentions.push((FileId(file), mention(k, x, section)));
                    m.key_refs.push(key_ref(k, file, x, lazy));
                }
                if !lazy && x.kind == def_kind::COMMON {
                    m.commons.push(CommonRow {
                        key: k,
                        file,
                        size: x.size,
                        align: x.value.max(1),
                        tomb: false,
                    });
                }
            }
            if !lazy {
                for s in f.sections().into_iter().skip(1) {
                    let signature = m.intern(s.as_bytes());
                    m.groups.push(GroupRow { signature, file, tomb: false });
                }
            }
        }
        for (k, row) in &full.rows {
            m.resolved[id(k) as usize] = ResolvedRow {
                kind: row.kind,
                typ: row.typ,
                bind: row.bind,
                other: row.other,
                section: row.sec.map_or(NONE, |(f, l)| first_section[&f] + l),
                value: row.value,
                size: row.size,
            };
        }
        // Roots are the options' (`OptionKeys`), never `KEY_REFS` rows.
        m.name_rows();
        m.file_index = Vec::new();
        m
    }
}

fn mention(k: u32, x: &M, section: u32) -> MentionRow {
    MentionRow { symbol: k, kind: x.kind, bind: x.bind, typ: x.typ, other: x.vis, section }
}

fn key_ref(k: u32, file: u32, x: &M, lazy: bool) -> KeyRefRow {
    let mut flags = x.vis & key_ref_flags::VISIBILITY;
    if lazy {
        flags |= key_ref_flags::LAZY;
    }
    if x.kind == def_kind::SECTION {
        flags |= key_ref_flags::IN_SECTION;
    }
    KeyRefRow { key: k, file, def: x.kind, bind: x.bind, typ: x.typ, flags, tomb: false }
}

/// A changed file's rows as the join gives them: its sections from
/// `first_section` on, their names appended to `strings` (the update's
/// buffer, at `base` past the state's `STRINGS`).
fn changed_file(
    file: u32,
    change: Change,
    f: Option<&F>,
    first_section: u32,
    strings: &mut Vec<u8>,
    base: u32,
) -> ChangedFile {
    let mut keys: Vec<KeyName> = Vec::new();
    let mut mentions = Vec::new();
    let mut values = Vec::new();
    let mut sections = Vec::new();
    let mut row = FileRow::default();
    let add = |s: &[u8], strings: &mut Vec<u8>| {
        let off = base + strings.len() as u32;
        strings.extend_from_slice(s);
        Str { off, len: s.len() as u32 }
    };
    if let Some(f) = f
        && !matches!(change, Change::Gone | Change::NoLongerExtracted)
    {
        for s in f.sections() {
            let (anchor_kind, anchor) =
                if s.is_empty() { (0, Str::default()) } else { (1, add(s.as_bytes(), strings)) };
            let name = add(b".text", strings);
            sections.push(SectionRow { file, anchor_kind, anchor, name, ..SectionRow::default() });
        }
        for x in &f.ments {
            let name = x.key.as_bytes().to_vec();
            let k = if let Some(p) = keys.iter().position(|n| n.name == name) {
                p
            } else {
                keys.push(KeyName {
                    name,
                    version: Vec::new(),
                    kind: sym_kind::GLOBAL,
                    index: 0,
                });
                keys.len() - 1
            };
            let section =
                if x.kind == def_kind::SECTION { first_section + f.local(x) } else { NONE };
            mentions.push(mention(k as u32, x, section));
            values.push(MentionValue { value: x.value, size: x.size });
        }
        row = FileRow {
            kind: 1,
            member: u8::from(f.member),
            flags: if f.member { file_flags::LAZY } else { 0 },
            rank: f.rank,
            ..FileRow::default()
        };
    }
    ChangedFile {
        file: FileId(file),
        change,
        row,
        first_section: SectionId(first_section),
        sections,
        keys,
        first_global: 0,
        mentions,
        values,
        index: Vec::new(),
        edges: Vec::new(),
        eh_records: Vec::new(),
        meta_units: 0,
        debug: false,
        versioned: 0,
        odr: OdrRows::default(),
    }
}

/// The join's `ChangedFiles` for `changes` over `state` (sections numbered
/// from the state's count in file order).
fn assemble(
    state: &MemoryState,
    new: &Link,
    changes: &BTreeMap<u32, Change>,
    appeared: &[FileId],
) -> ChangedFiles {
    let base = state.strings.len() as u32;
    let mut strings = Vec::new();
    let mut next = state.sections.len() as u32;
    let mut files = Vec::new();
    for (&f, &change) in changes {
        let c = changed_file(f, change, new.file(f), next, &mut strings, base);
        next += c.sections.len() as u32;
        files.push(c);
    }
    ChangedFiles { files, appeared: appeared.to_vec(), positional: Vec::new(), strings }
}

/// The join's changes between `old` and `new` (same rows; a row past
/// `old`'s is appeared).
fn changes(old: &Link, new: &Link) -> (BTreeMap<u32, Change>, Vec<FileId>) {
    let mut files = BTreeMap::new();
    let mut appeared = Vec::new();
    for i in 1..new.files.len().max(old.files.len()) {
        let f = i as u32;
        let change = match (old.file(f), new.file(f)) {
            (Some(_), None) => Change::Gone,
            (None, Some(_)) => {
                appeared.push(FileId(f));
                Change::Appeared
            }
            (Some(o), Some(n)) if o.ments != n.ments => Change::Changed,
            _ => continue,
        };
        files.insert(f, change);
    }
    (files, appeared)
}

/// What the join does: the changed files, then `extract` called again
/// with every newly extracted member read (`Extracted`) until it adds
/// none. The update's live set.
fn update(old: &Link, new: &Link) -> Result<(BTreeMap<u32, u32>, extract::Extraction), IncrReason> {
    let mut keys = Vec::new();
    let state = old.state(&mut keys);
    let view = state.view();
    let (mut files, appeared) = changes(old, new);
    let x = extraction(&view, &state, new, &mut files, &appeared)?;
    // The update's live set.
    let mut live: BTreeMap<u32, u32> = BTreeMap::new();
    for (i, f) in state.files.iter().enumerate() {
        let gone = new.file(i as u32).is_none();
        if f.flags & (file_flags::LAZY | file_flags::GONE) == 0 && !gone {
            live.insert(i as u32, f.round);
        }
    }
    for &f in &appeared {
        if !new.file(f.0).unwrap().member {
            live.insert(f.0, 0);
        }
    }
    for f in &x.unextracted {
        live.remove(&f.0);
    }
    for &(f, r) in x.extracted.iter().chain(&x.rounds) {
        live.insert(f.0, r);
    }
    Ok((live, x))
}

/// U4/U5's loop: `extract` until every newly extracted member is read.
fn extraction(
    view: &mortar_incr::update::view::StateView<'_>,
    state: &MemoryState,
    new: &Link,
    files: &mut BTreeMap<u32, Change>,
    appeared: &[FileId],
) -> Result<extract::Extraction, IncrReason> {
    let options = new.options();
    loop {
        let changed = assemble(state, new, files, appeared);
        let x = extract::extract(view, &changed, &options)?;
        let mut more = false;
        for &(m, _) in &x.extracted {
            if let std::collections::btree_map::Entry::Vacant(e) = files.entry(m.0) {
                e.insert(Change::Extracted);
                more = true;
            }
        }
        if !more {
            return Ok(x);
        }
    }
}

/// A whole update through `resolve`: the stored state, the final
/// `ChangedFiles`, the result and the number of `resolve` calls.
struct Joined {
    state: MemoryState,
    keys: Vec<String>,
    changed: ChangedFiles,
    result: Result<resolve::ResolveDelta, IncrReason>,
    calls: usize,
    drawn: Vec<u32>,
}

fn join(old: &Link, new: &Link) -> Joined {
    join_patched(old, new, |_, _| {})
}

/// [`join`] over the stored state as `patch` leaves it (rows the model
/// does not make: a shared object's).
fn join_patched(old: &Link, new: &Link, patch: impl Fn(&mut MemoryState, &[String])) -> Joined {
    let mut keys = Vec::new();
    let mut state = old.state(&mut keys);
    patch(&mut state, &keys);
    let (mut files, appeared) = changes(old, new);
    let options = new.options();
    let mut calls = 0;
    let mut drawn = Vec::new();
    let (changed, result) = {
        let view = state.view();
        match extraction(&view, &state, new, &mut files, &appeared) {
            Err(e) => (assemble(&state, new, &files, &appeared), Err(e)),
            Ok(x) => loop {
                let changed = assemble(&state, new, &files, &appeared);
                calls += 1;
                match resolve::resolve(&view, &changed, &x, &options) {
                    Err(e) => break (changed, Err(e)),
                    Ok(resolve::Resolution::Delta(d)) => break (changed, Ok(*d)),
                    Ok(resolve::Resolution::DrawIn(list)) => {
                        assert!(calls < resolve::RESOLVE_CALLS, "a second draw-in: {list:?}");
                        for f in list {
                            assert!(files.insert(f.0, Change::DrawnIn).is_none(), "{f:?} drawn twice");
                            drawn.push(f.0);
                        }
                    }
                }
            },
        }
    };
    Joined { state, keys, changed, result, calls, drawn }
}

impl Joined {
    fn name(&self, d: &resolve::ResolveDelta, k: u32) -> String {
        let n = self.keys.len() as u32;
        if k < n {
            self.keys[k as usize].clone()
        } else {
            String::from_utf8(d.new_keys[(k - n) as usize].name.clone()).unwrap()
        }
    }
    /// A section id of the update's id space as `(file, section of it)`.
    fn sec(&self, s: u32) -> Option<(u32, u32)> {
        if s == NONE {
            return None;
        }
        if (s as usize) < self.state.sections.len() {
            let f = self.state.sections[s as usize].file;
            let first = self.state.sections.iter().position(|x| x.file == f).unwrap() as u32;
            return Some((f, s - first));
        }
        let c = self
            .changed
            .files
            .iter()
            .find(|c| c.first_section.0 <= s && s < c.first_section.0 + c.sections.len() as u32)
            .expect("a new section of a changed file");
        Some((c.file.0, s - c.first_section.0))
    }
    fn bytes(&self, s: Str) -> Vec<u8> {
        self.changed.string(&self.state.view(), s).unwrap()
    }
}

/// IN-71b "Live keys" (WP-215 R.6 3): a global key is live when a stored
/// row gives it a mention by a live file or a `LAZY` definition (a
/// `KEY_REFS` row: a `LAZY` member's undefined mentions are not stored)
/// or an option names it. The test's stand-in for the rule as IN-78's
/// `canon` applies it: U-2's `canon` replaces [`canon_live_keys`] when it
/// lands (not on this branch).
fn is_live_key(k: &str, key_refs: &BTreeSet<String>, named: &BTreeSet<String>) -> bool {
    key_refs.contains(k) || named.contains(k)
}

/// The rows of the live keys only: what `canon` keeps (IN-78, dead keys
/// dropped).
fn canon_live_keys(
    rows: BTreeMap<String, Row>,
    key_refs: &BTreeSet<String>,
    named: &BTreeSet<String>,
) -> BTreeMap<String, Row> {
    rows.into_iter().filter(|(k, _)| is_live_key(k, key_refs, named)).collect()
}

/// The update `old` -> `new` equals the full link of `new`: every key's
/// `RESOLVED` row (affected ones from the delta, the others stored), gone
/// and new keys, `at`, `KEY_REFS`, `COMMONS`, `GROUPS`.
fn same_as_full(j: &Joined, d: &resolve::ResolveDelta, new: &Link, what: &str) {
    let full = new.full();
    let symbols = j.keys.len() as u32;
    let mut rows: BTreeMap<String, Row> = BTreeMap::new();
    let canon = |r: &ResolvedRow| Row {
        kind: r.kind,
        typ: r.typ,
        bind: r.bind,
        other: r.other,
        sec: j.sec(r.section),
        value: r.value,
        size: r.size,
    };
    let affected: BTreeMap<u32, ResolvedRow> = d.resolved.iter().map(|(k, r)| (k.0, *r)).collect();
    let gone: BTreeSet<String> = d.gone.iter().map(|k| j.name(d, k.0)).collect();
    for (i, k) in j.keys.iter().enumerate() {
        if gone.contains(k) {
            continue;
        }
        let r = affected.get(&(i as u32)).copied().unwrap_or(j.state.resolved[i]);
        rows.insert(k.clone(), canon(&r));
    }
    for (i, n) in d.new_keys.iter().enumerate() {
        let id = symbols + i as u32;
        let r = affected.get(&id).unwrap_or_else(|| panic!("{what}: new key {id} has no row"));
        rows.insert(String::from_utf8(n.name.clone()).unwrap(), canon(r));
    }
    // KEY_REFS first: the live keys (IN-71b "Live keys") are read from it.
    let mut nk = Vec::new();
    let want = new.state(&mut nk);
    let mut key_refs: BTreeMap<(String, u32), KeyRefRow> = BTreeMap::new();
    for r in &j.state.key_refs {
        key_refs.insert((j.keys[r.key as usize].clone(), r.file), KeyRefRow { key: 0, ..*r });
    }
    for r in &d.key_refs.delete {
        key_refs.remove(&(j.name(d, r.key), r.file));
    }
    for r in &d.key_refs.put {
        key_refs.insert((j.name(d, r.key), r.file), KeyRefRow { key: 0, ..*r });
    }
    let want_refs: BTreeMap<(String, u32), KeyRefRow> = want
        .key_refs
        .iter()
        .map(|r| ((nk[r.key as usize].clone(), r.file), KeyRefRow { key: 0, ..*r }))
        .collect();
    assert_eq!(key_refs, want_refs, "{what}: KEY_REFS");
    // Both sides through the rule (WP-215 R.6 3, ruled): a dead key (no
    // `KEY_REFS` row, no option naming it) is dropped from the update's
    // rows and from the full link's; no list of tolerated keys.
    let named = new.option_names();
    let live_after: BTreeSet<String> = key_refs.keys().map(|(k, _)| k.clone()).collect();
    let want_live: BTreeSet<String> = want_refs.keys().map(|(k, _)| k.clone()).collect();
    let rows = canon_live_keys(rows, &live_after, &named);
    let want_rows = canon_live_keys(full.rows.clone(), &want_live, &named);
    assert_eq!(rows, want_rows, "{what}: RESOLVED");
    for k in &gone {
        assert!(!is_live_key(k, &live_after, &named), "{what}: gone key {k} is live");
    }
    // A dead key the update leaves (not gone) is undefined: no reader
    // gives it a definition.
    for (i, k) in j.keys.iter().enumerate() {
        if !gone.contains(k) && !is_live_key(k, &live_after, &named) {
            let r = affected.get(&(i as u32)).copied().unwrap_or(j.state.resolved[i]);
            assert_eq!(r.kind, def_kind::UNDEFINED, "{what}: dead key {k} left defined");
        }
    }
    // at
    let mut at: BTreeMap<String, (u32, u32)> = BTreeMap::new();
    for (i, k) in j.keys.iter().enumerate() {
        let s = j.state.symbols[i];
        at.insert(k.clone(), (s.at_file, s.at_index));
    }
    for &(k, f, x) in &d.at {
        at.insert(j.name(d, k.0), (f, x));
    }
    for n in &d.new_keys {
        at.insert(String::from_utf8(n.name.clone()).unwrap(), (n.row.at_file, n.row.at_index));
    }
    for k in want_rows.keys() {
        let w = full.at.get(k).copied().unwrap_or((NONE, 0));
        assert_eq!(at.get(k).copied(), Some(w), "{what}: at of {k}");
    }
    let mut commons: BTreeMap<(String, u32), (u64, u64)> = BTreeMap::new();
    for c in &j.state.commons {
        commons.insert((j.keys[c.key as usize].clone(), c.file), (c.size, c.align));
    }
    for c in &d.commons.delete {
        commons.remove(&(j.name(d, c.key), c.file));
    }
    for c in &d.commons.put {
        commons.insert((j.name(d, c.key), c.file), (c.size, c.align));
    }
    let want_commons: BTreeMap<(String, u32), (u64, u64)> = want
        .commons
        .iter()
        .map(|c| ((nk[c.key as usize].clone(), c.file), (c.size, c.align)))
        .collect();
    assert_eq!(commons, want_commons, "{what}: COMMONS");
    let mut groups: BTreeSet<(Vec<u8>, u32)> = BTreeSet::new();
    for g in &j.state.groups {
        groups.insert((j.bytes(g.signature), g.file));
    }
    for g in &d.groups.delete {
        assert!(groups.remove(&(j.bytes(g.signature), g.file)), "{what}: a deleted group row");
    }
    for g in &d.groups.put {
        groups.insert((j.bytes(g.signature), g.file));
    }
    let want_groups: BTreeSet<(Vec<u8>, u32)> = want
        .groups
        .iter()
        .map(|g| (want.view().string(g.signature).unwrap(), g.file))
        .collect();
    assert_eq!(groups, want_groups, "{what}: GROUPS");
    let new_names: BTreeSet<String> = d
        .new_keys
        .iter()
        .map(|n| String::from_utf8(n.name.clone()).unwrap())
        .filter(|k| is_live_key(k, &live_after, &named))
        .collect();
    let want_new: BTreeSet<String> =
        want_rows.keys().filter(|k| !j.keys.contains(k)).cloned().collect();
    assert_eq!(new_names, want_new, "{what}: new (live) keys");
}

/// The update's live set equals §4.1's on the new inputs, and its rounds
/// are supporting values (IN-71b).
fn check(old: &Link, new: &Link) -> extract::Extraction {
    let (live, x) = update(old, new).unwrap_or_else(|e| panic!("update failed: {e:?}"));
    let want = new.closure();
    let a: BTreeSet<u32> = live.keys().copied().collect();
    let b: BTreeSet<u32> = want.keys().copied().collect();
    assert_eq!(a, b, "live files: update vs full link\nold {old:#?}\nnew {new:#?}\n{x:?}");
    supported(new, &live);
    x
}

/// Every live member has an edge from a live file (or a root) of smaller
/// round.
fn supported(link: &Link, live: &BTreeMap<u32, u32>) {
    let (first, cfirst) = link.targets();
    for (&f, &r) in live {
        if f == 0 || !link.file(f).unwrap().member {
            continue;
        }
        let mut ok = link.roots.iter().any(|k| first.get(k.as_str()).map(|x| x.1) == Some(f));
        for (&s, &sr) in live {
            if sr >= r || s == 0 {
                continue;
            }
            for m in &link.file(s).unwrap().ments {
                let t = if m.kind == def_kind::UNDEFINED && m.bind != WEAK {
                    first.get(link.redirect(&m.key).as_str())
                } else if m.kind == def_kind::COMMON {
                    cfirst.get(m.key.as_str())
                } else {
                    None
                };
                ok |= t.map(|x| x.1) == Some(f);
            }
        }
        assert!(ok, "member {f} at round {r} has no supporting edge: {live:?}");
    }
}

const R: u64 = 1 << 20;

/// xorshift64*.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Mentions for the resolution series: COMDAT copies of mismatched symbol
/// sets (a group's symbols differ between files), weak behind strong,
/// commons of differing size and alignment, values and sizes.
fn random_resolve_ments(rng: &mut Rng, keys: u64) -> Vec<M> {
    let mut v: Vec<M> = Vec::new();
    for _ in 0..rng.below(5) {
        let k = format!("k{}", rng.below(keys));
        if v.iter().any(|m| m.key == k) {
            continue;
        }
        let value = rng.below(4) * 8;
        let size = rng.below(3) * 4;
        let mut m = match rng.below(12) {
            0..=2 => undef(&k),
            3 => weak_undef(&k),
            4 => weak_def(&k).val(value, size),
            5 => M { value: 1 << rng.below(4), size: 4 + 4 * rng.below(3), ..common(&k) },
            6 | 7 => weak_def(&k).func().group(&format!("g{}", rng.below(2))).val(value, size),
            8 => def(&k).group(&format!("g{}", rng.below(2))).val(value, size),
            _ => def(&k).val(value, size),
        };
        if rng.below(8) == 0 {
            m = m.hidden();
        }
        v.push(m);
    }
    v
}

/// One random edit of `new`'s file `f`: gone, an appended middle-rank file,
/// a value-only edit, new mentions.
fn random_edit(rng: &mut Rng, new: &mut Link, n: u64, keys: u64) {
    let f = 1 + rng.below(n) as usize;
    match rng.below(6) {
        0 => new.files[f] = None,
        1 | 2 => {
            let rank = (1 + rng.below(n + 1)) * R - 1 - rng.below(1000);
            let member = rng.below(2) == 0;
            let ments = random_resolve_ments(rng, keys);
            new.add(rank, member, ments);
        }
        3 => {
            if let Some(x) = new.files[f].as_mut()
                && x.ments.iter().any(|m| m.kind != def_kind::UNDEFINED)
            {
                let defs: Vec<usize> = (0..x.ments.len()).filter(|&i| x.ments[i].kind != def_kind::UNDEFINED).collect();
                let i = defs[rng.below(defs.len() as u64) as usize];
                let m = &mut x.ments[i];
                if m.kind == def_kind::COMMON {
                    m.size += 4;
                } else {
                    m.value += 4;
                    m.size += 1;
                }
            }
        }
        _ => {
            if let Some(x) = new.files[f].as_mut() {
                x.ments = random_resolve_ments(rng, keys);
            }
        }
    }
}

/// Why an update may fall back, from the full links alone; `None`: it
/// must not.
fn may_fall_back(old: &Link, new: &Link, e: &IncrReason) -> bool {
    let (a, b) = (old.full(), new.full());
    match e {
        IncrReason::ClosureIterated(_) => a.iterated || b.iterated,
        // resolve: a clash, a key that becomes referenced while undefined
        // (the referenced-undefined rows are not stored for unchanged
        // files: IN-71 "Undefined").
        IncrReason::UpdateNotImplemented(Some(Unimplemented::Resolve)) => {
            !b.clash.is_empty()
                || b.rows.iter().any(|(k, r)| {
                    r.kind == def_kind::UNDEFINED
                        && b.referenced.contains(k)
                        && a.rows.get(k).is_none_or(|o| {
                            o.kind != def_kind::UNDEFINED
                                || !a.referenced.contains(k)
                                || (o.bind == WEAK && r.bind != WEAK)
                        })
                })
        }
        IncrReason::SymbolKindChanged(_) => b.rows.iter().any(|(k, r)| {
            a.rows.get(k).is_some_and(|o| {
                let d = |x: u8| x == def_kind::SECTION || x == def_kind::ABSOLUTE;
                d(o.kind) && d(r.kind) && o.kind != r.kind
            })
        }),
        _ => false,
    }
}


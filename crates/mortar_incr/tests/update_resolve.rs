//! U-3's tests (`spec/incr.md` IN-71, IN-71a..c, IN-74; WP-215). Each
//! entry point is tested alone against a `StateView` over hand-made rows
//! (`MemoryState`), the stored link built by a test-local copy of
//! resolve.md §4.1 and the update checked against the same rule on the
//! new inputs.

use std::collections::{BTreeMap, BTreeSet};

use mortar_core::incr::{IncrReason, LayoutDelta, OptionKey, OptionKeys};
use mortar_core::{FileId, SectionId};
use mortar_incr::format::{NONE, def_kind};
use mortar_incr::update::rows::{
    FileRow, KeyRefRow, MentionRow, SymbolRow, file_flags, key_ref_flags, sym_kind,
};
use mortar_incr::update::testing::MemoryState;
use mortar_incr::update::view::{Change, ChangedFile, ChangedFiles, KeyName, OdrRows};
use mortar_incr::update::{defined, extract, refill, resolve};

const WEAK: u8 = 2;
const GLOBAL: u8 = 1;
const OBJECT: u8 = 1;

/// A mention in the model: key name, kind (`def_kind`), binding.
#[derive(Debug, Clone, PartialEq, Eq)]
struct M {
    key: String,
    kind: u8,
    bind: u8,
}

fn def(k: &str) -> M {
    M { key: k.into(), kind: def_kind::SECTION, bind: GLOBAL }
}
fn weak_def(k: &str) -> M {
    M { key: k.into(), kind: def_kind::SECTION, bind: WEAK }
}
fn undef(k: &str) -> M {
    M { key: k.into(), kind: def_kind::UNDEFINED, bind: GLOBAL }
}
fn common(k: &str) -> M {
    M { key: k.into(), kind: def_kind::COMMON, bind: GLOBAL }
}

/// A file of the model link.
#[derive(Debug, Clone)]
struct F {
    rank: u64,
    /// An archive member (not `--whole-archive`).
    member: bool,
    ments: Vec<M>,
}

/// A link of the model: files by `FILES` row (row 0 is file 0, the
/// internal file), and the root names.
#[derive(Debug, Clone, Default)]
struct Link {
    files: Vec<Option<F>>,
    roots: Vec<String>,
}

impl Link {
    fn new() -> Self {
        Self {
            files: vec![Some(F { rank: 0, member: false, ments: Vec::new() })],
            roots: Vec::new(),
        }
    }
    fn add(&mut self, rank: u64, member: bool, ments: Vec<M>) -> u32 {
        self.files.push(Some(F { rank, member, ments }));
        (self.files.len() - 1) as u32
    }
    fn file(&self, f: u32) -> Option<&F> {
        self.files.get(f as usize).and_then(Option::as_ref)
    }

    /// resolve.md §4.1 from scratch: the live files and a round (the
    /// shortest path from a named file or a root).
    fn closure(&self) -> BTreeMap<u32, u32> {
        let mut first: BTreeMap<&str, (u64, u32)> = BTreeMap::new();
        let mut cfirst: BTreeMap<&str, (u64, u32)> = BTreeMap::new();
        for (i, f) in self.files.iter().enumerate().skip(1) {
            let Some(f) = f else { continue };
            let at = (f.rank, i as u32);
            for m in &f.ments {
                let defined = m.kind == def_kind::SECTION || m.kind == def_kind::ABSOLUTE;
                if defined || m.kind == def_kind::COMMON {
                    let e = first.entry(&m.key).or_insert(at);
                    *e = (*e).min(at);
                }
                if defined && m.bind != WEAK {
                    let e = cfirst.entry(&m.key).or_insert(at);
                    *e = (*e).min(at);
                }
            }
        }
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
                    first.get(m.key.as_str())
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

    fn options(&self) -> OptionKeys {
        OptionKeys {
            undefined: self
                .roots
                .iter()
                .map(|r| OptionKey { name: r.as_bytes().into(), arg: 1 })
                .collect(),
            ..OptionKeys::default()
        }
    }

    /// The state a full link of this link stores (the rows U-3 reads).
    fn state(&self, keys: &mut Vec<String>) -> MemoryState {
        let live = self.closure();
        let mut m = MemoryState::default();
        m.meta.iterations = 1;
        for k in self.files.iter().flatten().flat_map(|f| f.ments.iter().map(|m| m.key.clone())) {
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
        for r in &self.roots {
            if !keys.contains(r) {
                keys.push(r.clone());
            }
        }
        let empty = m.intern(b"");
        for k in keys.iter() {
            let name = m.intern(k.as_bytes());
            m.symbols.push(SymbolRow {
                kind: sym_kind::GLOBAL,
                file: NONE,
                ordinal: 0,
                name,
                version: empty,
                at_file: NONE,
                at_index: 0,
            });
        }
        m.resolved = vec![Default::default(); keys.len()];
        let id = |k: &str| keys.iter().position(|x| x == k).unwrap() as u32;
        for (i, f) in self.files.iter().enumerate() {
            let file = i as u32;
            let Some(f) = f else {
                m.files.push(FileRow { flags: file_flags::GONE, ..FileRow::default() });
                continue;
            };
            let lazy = !live.contains_key(&file);
            let mut flags = 0;
            if lazy {
                flags |= file_flags::LAZY;
            }
            m.files.push(FileRow {
                kind: u8::from(i != 0),
                member: u8::from(f.member),
                flags,
                rank: f.rank,
                round: if lazy { 0 } else { live[&file] },
                ..FileRow::default()
            });
            for x in &f.ments {
                let k = id(&x.key);
                if !lazy {
                    m.mentions.push((FileId(file), mention(k, x)));
                }
                if !lazy || x.kind != def_kind::UNDEFINED {
                    m.key_refs.push(key_ref(k, file, x, lazy));
                }
            }
        }
        for r in &self.roots {
            let k = id(r);
            if !m.key_refs.iter().any(|x| x.key == k && x.file == 0) {
                m.key_refs.push(KeyRefRow {
                    key: k,
                    file: 0,
                    def: def_kind::UNDEFINED,
                    bind: GLOBAL,
                    ..KeyRefRow::default()
                });
            }
        }
        m.name_rows();
        m
    }
}

fn mention(k: u32, x: &M) -> MentionRow {
    MentionRow { symbol: k, kind: x.kind, bind: x.bind, typ: OBJECT, other: 0, section: NONE }
}

fn key_ref(k: u32, file: u32, x: &M, lazy: bool) -> KeyRefRow {
    let mut flags = 0;
    if lazy {
        flags |= key_ref_flags::LAZY;
    }
    if x.kind == def_kind::SECTION {
        flags |= key_ref_flags::IN_SECTION;
    }
    KeyRefRow { key: k, file, def: x.kind, bind: x.bind, typ: OBJECT, flags, tomb: false }
}

/// A changed file's rows as the join gives them.
fn changed_file(file: u32, change: Change, f: Option<&F>) -> ChangedFile {
    let mut keys: Vec<KeyName> = Vec::new();
    let mut mentions = Vec::new();
    let mut row = FileRow::default();
    if let Some(f) = f
        && !matches!(change, Change::Gone | Change::NoLongerExtracted)
    {
        for x in &f.ments {
            let name = x.key.as_bytes().to_vec();
            let k = match keys.iter().position(|n| n.name == name) {
                Some(p) => p,
                None => {
                    keys.push(KeyName {
                        name,
                        version: Vec::new(),
                        kind: sym_kind::GLOBAL,
                        index: 0,
                    });
                    keys.len() - 1
                }
            };
            mentions.push(mention(k as u32, x));
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
        first_section: SectionId(0),
        sections: Vec::new(),
        keys,
        mentions,
        index: Vec::new(),
        edges: Vec::new(),
        eh_records: Vec::new(),
        meta_units: 0,
        debug: false,
        versioned: 0,
        odr: OdrRows::default(),
    }
}

/// What the join does: the changed files between `old` and `new` (same
/// rows; a row past `old`'s is appeared), then `extract` called again with
/// every newly extracted member read (`Extracted`) until it adds none.
/// The update's live set.
fn update(old: &Link, new: &Link) -> Result<(BTreeMap<u32, u32>, extract::Extraction), IncrReason> {
    let mut keys = Vec::new();
    let state = old.state(&mut keys);
    let view = state.view();
    let mut files: BTreeMap<u32, ChangedFile> = BTreeMap::new();
    let mut appeared = Vec::new();
    for i in 1..new.files.len().max(old.files.len()) {
        let f = i as u32;
        let (o, n) = (old.file(f), new.file(f));
        let c = match (o, n) {
            (None, None) => continue,
            (Some(_), None) => changed_file(f, Change::Gone, None),
            (None, Some(n)) => {
                appeared.push(FileId(f));
                changed_file(f, Change::Appeared, Some(n))
            }
            (Some(o), Some(n)) if o.ments != n.ments => changed_file(f, Change::Changed, Some(n)),
            _ => continue,
        };
        files.insert(f, c);
    }
    let options = new.options();
    loop {
        let changed = ChangedFiles {
            files: files.values().cloned().collect(),
            appeared: appeared.clone(),
            positional: Vec::new(),
        };
        let x = extract::extract(&view, &changed, &options)?;
        let mut more = false;
        for &(m, _) in &x.extracted {
            if let std::collections::btree_map::Entry::Vacant(e) = files.entry(m.0) {
                e.insert(changed_file(m.0, Change::Extracted, new.file(m.0)));
                more = true;
            }
        }
        if more {
            continue;
        }
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
        return Ok((live, x));
    }
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
    let mut first: BTreeMap<&str, (u64, u32)> = BTreeMap::new();
    let mut cfirst: BTreeMap<&str, (u64, u32)> = BTreeMap::new();
    for (i, f) in link.files.iter().enumerate().skip(1) {
        let Some(f) = f else { continue };
        for m in &f.ments {
            let d = m.kind == def_kind::SECTION;
            let at = (f.rank, i as u32);
            if d || m.kind == def_kind::COMMON {
                let e = first.entry(&m.key).or_insert(at);
                *e = (*e).min(at);
            }
            if d && m.bind != WEAK {
                let e = cfirst.entry(&m.key).or_insert(at);
                *e = (*e).min(at);
            }
        }
    }
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
                    first.get(m.key.as_str())
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

// ---- extraction (IN-71b) ------------------------------------------------------

// spec: incr.md IN-71b
#[test]
fn nothing_changed_extracts_nothing() {
    let mut l = Link::new();
    l.add(R, false, vec![undef("a")]);
    l.add(2 * R, true, vec![def("a")]);
    let x = check(&l, &l);
    assert_eq!(x, extract::Extraction::default());
}

// spec: incr.md IN-71b
#[test]
fn newly_extracted_member() {
    let mut old = Link::new();
    let a = old.add(R, false, vec![]);
    let m = old.add(2 * R, true, vec![def("f")]);
    let mut new = old.clone();
    new.files[a as usize].as_mut().unwrap().ments = vec![undef("f")];
    let x = check(&old, &new);
    assert_eq!(x.extracted, vec![(FileId(m), 1)]);
}

// spec: incr.md IN-71b
#[test]
fn newly_extracted_member_pulls_another() {
    let mut old = Link::new();
    let a = old.add(R, false, vec![]);
    let m1 = old.add(2 * R, true, vec![def("f"), undef("g")]);
    let m2 = old.add(3 * R, true, vec![def("g")]);
    let mut new = old.clone();
    new.files[a as usize].as_mut().unwrap().ments = vec![undef("f")];
    let x = check(&old, &new);
    assert_eq!(x.extracted, vec![(FileId(m1), 1), (FileId(m2), 2)]);
}

// spec: incr.md IN-71b
#[test]
fn no_longer_extracted_member() {
    let mut old = Link::new();
    let a = old.add(R, false, vec![undef("f")]);
    let m = old.add(2 * R, true, vec![def("f")]);
    let mut new = old.clone();
    new.files[a as usize].as_mut().unwrap().ments = vec![];
    let x = check(&old, &new);
    assert_eq!(x.unextracted, vec![FileId(m)]);
}

// spec: incr.md IN-71b (WP-211 §D)
#[test]
fn member_cycle_dropped_together() {
    let mut old = Link::new();
    let a = old.add(R, false, vec![undef("f")]);
    let m1 = old.add(2 * R, true, vec![def("f"), undef("g")]);
    let m2 = old.add(3 * R, true, vec![def("g"), undef("f")]);
    let mut new = old.clone();
    new.files[a as usize].as_mut().unwrap().ments = vec![];
    let x = check(&old, &new);
    assert_eq!(x.unextracted, vec![FileId(m1), FileId(m2)]);
}

// spec: incr.md IN-71b (WP-211 §D: `start_group_member_cycle`)
#[test]
fn start_group_member_cycle() {
    // Two members referencing each other, one also referenced by a named
    // file: dropping the cycle's other in-edge keeps both.
    let mut old = Link::new();
    let a = old.add(R, false, vec![undef("f")]);
    let b = old.add(R + 1, false, vec![undef("g")]);
    let m1 = old.add(2 * R, true, vec![def("f"), undef("g")]);
    let m2 = old.add(3 * R, true, vec![def("g"), undef("f")]);
    let mut new = old.clone();
    new.files[b as usize].as_mut().unwrap().ments = vec![];
    let x = check(&old, &new);
    assert!(x.unextracted.is_empty(), "{x:?}");
    // m2 was round 1 (from b), now only m1 (round 1) supports it.
    assert_eq!(x.rounds, vec![(FileId(m2), 2)]);
    let _ = (a, m1);
}

// spec: incr.md IN-71b
#[test]
fn extracted_member_keeps_in_edge() {
    let mut old = Link::new();
    let a = old.add(R, false, vec![undef("f")]);
    let _b = old.add(R + 2, false, vec![undef("f")]);
    let m = old.add(2 * R, true, vec![def("f")]);
    let mut new = old.clone();
    new.files[a as usize].as_mut().unwrap().ments = vec![];
    let x = check(&old, &new);
    assert_eq!(x, extract::Extraction::default());
    let _ = m;
}

// spec: incr.md IN-71b
#[test]
fn member_round_shortcut() {
    // m3 reached at round 3 through m1 -> m2 -> m3; the edit adds a direct
    // reference from a named file: m3 stays live (any supporting round).
    let mut old = Link::new();
    let a = old.add(R, false, vec![undef("f")]);
    old.add(2 * R, true, vec![def("f"), undef("g")]);
    old.add(3 * R, true, vec![def("g"), undef("h")]);
    old.add(4 * R, true, vec![def("h")]);
    let mut new = old.clone();
    new.files[a as usize].as_mut().unwrap().ments = vec![undef("f"), undef("h")];
    check(&old, &new);
}

// spec: incr.md IN-71b (WP-211 §D)
#[test]
fn member_flips_back_on_third_hop() {
    // a drops its reference to f; m1 (f) goes, but m3 (reached from m1
    // via m2) is referenced by an appeared file: m2 goes, m3 stays.
    let mut old = Link::new();
    let a = old.add(R, false, vec![undef("f")]);
    let m1 = old.add(2 * R, true, vec![def("f"), undef("g")]);
    let m2 = old.add(3 * R, true, vec![def("g"), undef("h")]);
    let m3 = old.add(4 * R, true, vec![def("h")]);
    let mut new = old.clone();
    new.files[a as usize].as_mut().unwrap().ments = vec![];
    new.add(R + 5, false, vec![undef("h")]);
    let x = check(&old, &new);
    assert_eq!(x.unextracted, vec![FileId(m1), FileId(m2)]);
    // m3 keeps its stored round: the appeared file (round 0) supports it.
    assert!(x.rounds.is_empty(), "{m3}: {x:?}");
}

// spec: incr.md IN-71b, resolve.md §4.1 (weak definition before a member)
#[test]
fn weak_definition_vs_later_member() {
    // A weak definition in a named file BEFORE the member keeps it out;
    // one AFTER it does not (first(K) counts any definition).
    let mut old = Link::new();
    let a = old.add(R, false, vec![undef("f")]);
    let w = old.add(R + 1, false, vec![]);
    let m = old.add(2 * R, true, vec![def("f")]);
    let mut new = old.clone();
    new.files[w as usize].as_mut().unwrap().ments = vec![weak_def("f")];
    let x = check(&old, &new);
    assert_eq!(x.unextracted, vec![FileId(m)]);
    // Weak definition after the member: the member stays.
    let mut old2 = Link::new();
    old2.add(R, false, vec![undef("f")]);
    let m2 = old2.add(2 * R, true, vec![def("f")]);
    let w2 = old2.add(3 * R, false, vec![]);
    let mut new2 = old2.clone();
    new2.files[w2 as usize].as_mut().unwrap().ments = vec![weak_def("f")];
    let x2 = check(&old2, &new2);
    assert!(x2.unextracted.is_empty(), "{m2}: {x2:?}");
    let _ = a;
}

// spec: incr.md IN-71b, WP-215 rank correction: an appended file with a
// middle rank decides first(K).
#[test]
fn appended_middle_rank_member_wins_extraction() {
    // Two members define f, ranks 2R and 4R; a member appended at 3R
    // (row past both) is not first; one appended at R + 1 (row last) is.
    let mut old = Link::new();
    old.add(R, false, vec![]);
    let m1 = old.add(2 * R, true, vec![def("f")]);
    let m2 = old.add(4 * R, true, vec![def("f")]);
    let a = 1;
    let mut new = old.clone();
    new.files[a].as_mut().unwrap().ments = vec![undef("f")];
    let mid = new.add(3 * R, true, vec![def("f")]);
    let x = check(&old, &new);
    assert_eq!(x.extracted, vec![(FileId(m1), 1)]);
    let mut new2 = old.clone();
    new2.files[a].as_mut().unwrap().ments = vec![undef("f")];
    let early = new2.add(R + 1, true, vec![def("f")]);
    let x2 = check(&old, &new2);
    assert_eq!(x2.extracted, vec![(FileId(early), 1)]);
    let _ = (m2, mid);
}

// spec: incr.md IN-71b (common pulls cfirst)
#[test]
fn common_pulls_strong_member() {
    let mut old = Link::new();
    let a = old.add(R, false, vec![]);
    let m = old.add(2 * R, true, vec![def("c")]);
    let mut new = old.clone();
    new.files[a as usize].as_mut().unwrap().ments = vec![common("c")];
    let x = check(&old, &new);
    assert_eq!(x.extracted, vec![(FileId(m), 1)]);
}

// spec: incr.md IN-71b (roots, §4.2)
#[test]
fn root_option_targets_member() {
    let mut old = Link::new();
    old.add(R, false, vec![]);
    let m = old.add(2 * R, true, vec![def("r")]);
    old.roots.push("r".into());
    // A changed named file now defines r first: the member is dropped.
    let mut new = old.clone();
    new.files[1].as_mut().unwrap().ments = vec![def("r")];
    let x = check(&old, &new);
    assert_eq!(x.unextracted, vec![FileId(m)]);
}

// spec: incr.md IN-71b ("Iterations": the stored link iterated)
#[test]
fn stored_link_iterated_is_fallback() {
    let l = Link::new();
    let mut keys = Vec::new();
    let mut state = l.state(&mut keys);
    state.meta.iterations = 2;
    let view = state.view();
    let r = extract::extract(&view, &ChangedFiles::default(), &OptionKeys::default());
    assert!(matches!(r, Err(IncrReason::ClosureIterated(_))), "{r:?}");
}

// spec: incr.md IN-71 "Versions" (WP-215 D8)
#[test]
fn versioned_old_or_new_is_fallback() {
    let l = Link::new();
    let mut keys = Vec::new();
    let mut state = l.state(&mut keys);
    state.meta.versioned = 1;
    {
        let view = state.view();
        let r = extract::extract(&view, &ChangedFiles::default(), &OptionKeys::default());
        assert_eq!(
            r.err(),
            Some(IncrReason::UpdateNotImplemented(Some(
                mortar_core::incr::Unimplemented::Versioned
            )))
        );
    }
    state.meta.versioned = 0;
    let view = state.view();
    let mut c =
        changed_file(1, Change::Appeared, Some(&F { rank: R, member: false, ments: vec![] }));
    c.versioned = 1;
    let changed = ChangedFiles { files: vec![c], appeared: vec![FileId(1)], positional: vec![] };
    let r = extract::extract(&view, &changed, &OptionKeys::default());
    assert_eq!(
        r.err(),
        Some(IncrReason::UpdateNotImplemented(Some(mortar_core::incr::Unimplemented::Versioned)))
    );
}

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

fn random_ments(rng: &mut Rng, keys: u64) -> Vec<M> {
    let mut v: Vec<M> = Vec::new();
    for _ in 0..rng.below(4) {
        let k = format!("k{}", rng.below(keys));
        if v.iter().any(|m| m.key == k) {
            continue;
        }
        v.push(match rng.below(8) {
            0..=2 => undef(&k),
            3 => M { key: k, kind: def_kind::UNDEFINED, bind: WEAK },
            4 => weak_def(&k),
            5 => common(&k),
            _ => def(&k),
        });
    }
    v
}

// spec: incr.md IN-71b: the model's random series (each update's live set
// equals §4.1 on the new inputs; rounds support).
#[test]
fn extraction_random_series() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut runs = 0;
    for _ in 0..3000 {
        let keys = 2 + rng.below(6);
        let mut old = Link::new();
        let n = 2 + rng.below(7);
        for i in 0..n {
            let member = rng.below(3) != 0;
            let ments = random_ments(&mut rng, keys);
            old.add((i + 1) * R, member, ments);
        }
        if rng.below(4) == 0 {
            old.roots.push(format!("k{}", rng.below(keys)));
        }
        let mut new = old.clone();
        for _ in 0..1 + rng.below(3) {
            let f = 1 + rng.below(n) as usize;
            match rng.below(5) {
                0 => new.files[f] = None,
                1 => {
                    let rank = (1 + rng.below(n + 1)) * R - 1 - rng.below(1000);
                    let member = rng.below(2) == 0;
                    let ments = random_ments(&mut rng, keys);
                    new.add(rank, member, ments);
                }
                _ => {
                    if let Some(x) = new.files[f].as_mut() {
                        x.ments = random_ments(&mut rng, keys);
                    }
                }
            }
        }
        check(&old, &new);
        runs += 1;
    }
    assert_eq!(runs, 3000);
}

// ---- the other entry points (until their tests) ---------------------------------

// spec: incr.md IN-71c, IN-74
#[test]
fn empty_update_recompute_and_refill_set() {
    let m = MemoryState::default();
    let view = m.view();
    let layout = LayoutDelta::default();
    assert!(defined::recompute(&view, &layout).is_empty());
    let resolved = resolve::ResolveDelta::default();
    let set = refill::refill_set(&view, &resolved, &layout, &[]).unwrap();
    assert!(set.units.is_empty());
}

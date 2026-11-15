//! `--warn-backrefs`: a simulation of GNU ld's archive walk
//! (`spec/resolve.md` §4.6).
//!
//! The walk is sequential by definition (a later member of a pass sees
//! what an earlier one loaded). Each lazy file's candidate mentions are
//! summarised once, in parallel, so that a pass over an archive reads a
//! flat list rather than the member's symbol table again.

use std::collections::BTreeSet;

use mortar_core::par::Pool;
use mortar_core::resolution::FileLive;
use mortar_core::{FileId, SymEntry, SymbolId};
use mortar_elf::SymbolSection;

use crate::comdat::{Keepers, SectionMap};
use crate::mention::{Class, classify, is_data};
use crate::options::Root;
use crate::problem::{Problem, What};
use crate::resolve::Def;
use crate::view::{FileKind, Mention, View, entry_word, word_entry};

/// A key's state in the walk, in the order of §4.6.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum State {
    None = 0,
    WeakUndefined = 1,
    Undefined = 2,
    DefinedWeak = 3,
    Common = 4,
    Defined = 5,
}

/// The simulation's outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Simulation {
    /// The lazy files GNU ld would load, in `FileId` order.
    pub loaded: Vec<FileId>,
    /// Extracted by mortar, not loaded by the simulation.
    pub mortar_only: Vec<FileId>,
    /// Loaded by the simulation, not extracted by mortar.
    pub gnu_only: Vec<FileId>,
    /// Every key's final state.
    pub state: Vec<State>,
    /// `archive-backref` and `archive-choice`, by key.
    pub problems: Vec<Problem>,
}

/// One candidate mention of a lazy file: what makes it load (§4.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Pull {
    key: u32,
    /// A non-weak data definition (loads for a `common` key too).
    strong_data: bool,
}

struct Walk<'a, V> {
    view: &'a V,
    redirect: &'a [SymbolId],
    state: Vec<State>,
    /// The entry word of the mention that set the state.
    definer: Vec<u64>,
    /// The first non-weak reference.
    referrer: Vec<u64>,
    loaded: Vec<bool>,
    buf: Vec<Mention>,
    /// The lazy files that may load: a key they pull became `undefined`
    /// (or `common`, for a data definition) since they were last looked at.
    pending: BTreeSet<u32>,
    by_key: ByKey,
    /// Test support: scan every member in every pass (the definition).
    naive: bool,
}

/// For each key, the lazy files that pull it, in `FileId` order, with
/// whether the mention is a non-weak data definition.
struct ByKey {
    offsets: Vec<u32>,
    files: Vec<(u32, bool)>,
}

impl ByKey {
    fn new(keys: usize, pulls: &Pulls) -> Self {
        let mut offsets = vec![0u32; keys + 1];
        for p in &pulls.items {
            offsets[p.key as usize + 1] += 1;
        }
        for k in 0..keys {
            offsets[k + 1] += offsets[k];
        }
        let mut next = offsets.clone();
        let mut files = vec![(0, false); pulls.items.len()];
        for f in 0..pulls.offsets.len() - 1 {
            for p in pulls.of(f) {
                let at = &mut next[p.key as usize];
                files[*at as usize] = (f as u32, p.strong_data);
                *at += 1;
            }
        }
        Self { offsets, files }
    }

    fn of(&self, key: usize) -> &[(u32, bool)] {
        &self.files[self.offsets[key] as usize..self.offsets[key + 1] as usize]
    }
}

impl<V: View> Walk<'_, V> {
    /// Sets a key's state, and makes the files it may now load pending.
    fn set(&mut self, k: usize, to: State) {
        self.state[k] = to;
        if matches!(to, State::Undefined | State::Common) {
            for &(f, strong_data) in self.by_key.of(k) {
                if (to == State::Undefined || strong_data) && !self.loaded[f as usize] {
                    self.pending.insert(f);
                }
            }
        }
    }

    fn raise(&mut self, key: SymbolId, to: State, word: u64) {
        let k = key.0 as usize;
        self.set(k, to);
        self.definer[k] = word;
    }

    /// Adds a file: applies its mentions in symbol-table order.
    fn add(&mut self, file: FileId, shared: bool) {
        self.buf.clear();
        self.view.mentions(file, &mut self.buf);
        let mentions = std::mem::take(&mut self.buf);
        for m in &mentions {
            let word = m.word(file);
            let (class, _) = classify(m, SymEntry { file, index: m.index });
            let keys = [Some(m.key), m.also.get()];
            match class {
                Class::Undefined { weak } => {
                    let key = self.redirect[m.key.0 as usize];
                    let k = key.0 as usize;
                    let s = self.state[k];
                    if !weak && matches!(s, State::None | State::WeakUndefined) {
                        self.set(k, State::Undefined);
                    } else if weak && s == State::None {
                        self.set(k, State::WeakUndefined);
                    }
                    if !weak && self.referrer[k] == u64::MAX {
                        self.referrer[k] = word;
                    }
                }
                Class::Strong => {
                    for key in keys.into_iter().flatten() {
                        if self.state[key.0 as usize] != State::Defined {
                            self.raise(key, State::Defined, word);
                        }
                    }
                }
                Class::Weak => {
                    for key in keys.into_iter().flatten() {
                        let s = self.state[key.0 as usize];
                        if shared && s != State::Defined {
                            self.raise(key, State::Defined, word);
                        } else if s <= State::Undefined {
                            self.raise(key, State::DefinedWeak, word);
                        }
                    }
                }
                Class::Common => {
                    for key in keys.into_iter().flatten() {
                        if self.state[key.0 as usize] < State::Common {
                            self.raise(key, State::Common, word);
                        }
                    }
                }
            }
        }
        self.buf = mentions;
        self.loaded[file.0 as usize] = true;
    }

    fn wants(&self, pulls: &[Pull]) -> bool {
        pulls.iter().any(|p| match self.state[p.key as usize] {
            State::Undefined => true,
            State::Common => p.strong_data,
            _ => false,
        })
    }

    /// Scans one archive (its members `members`) in passes until one
    /// loads nothing; whether any pass loaded a member.
    ///
    /// A pass visits the members in order and loads those that are
    /// wanted when visited. Only a pending member can be wanted, so a pass
    /// visits the pending members of the range in order: one loaded at
    /// `f` makes the members after `f` it pulls in visited in this pass,
    /// those before it in the next one, as the full visit would.
    fn scan(&mut self, members: std::ops::Range<usize>, pulls: &Pulls) -> bool {
        if self.naive {
            return self.scan_naive(members, pulls);
        }
        let end = members.end as u32;
        let mut any = false;
        loop {
            let mut pass = false;
            let mut at = members.start as u32;
            while let Some(&f) = self.pending.range(at..end).next() {
                self.pending.remove(&f);
                at = f + 1;
                if !self.loaded[f as usize] && self.wants(pulls.of(f as usize)) {
                    self.add(FileId(f), false);
                    pass = true;
                }
            }
            if !pass {
                return any;
            }
            any = true;
        }
    }

    fn scan_naive(&mut self, members: std::ops::Range<usize>, pulls: &Pulls) -> bool {
        let mut any = false;
        loop {
            let mut pass = false;
            for f in members.clone() {
                if !self.loaded[f] && self.wants(pulls.of(f)) {
                    self.add(FileId(f as u32), false);
                    pass = true;
                }
            }
            if !pass {
                return any;
            }
            any = true;
        }
    }
}

/// The candidate mentions of every lazy file, flat.
struct Pulls {
    items: Vec<Pull>,
    offsets: Vec<usize>,
}

impl Pulls {
    fn of(&self, f: usize) -> &[Pull] {
        &self.items[self.offsets[f]..self.offsets[f + 1]]
    }
}

fn pulls<V: View>(view: &V, kinds: &[FileKind], pool: &Pool) -> Pulls {
    let got = pool.map_collect(kinds.len(), |f, out: &mut Vec<Pull>| {
        if !kinds[f].is_lazy() {
            return;
        }
        let file = FileId(f as u32);
        let mut buf = Vec::new();
        view.mentions(file, &mut buf);
        for m in &buf {
            let (class, _) = classify(m, SymEntry { file, index: m.index });
            if !class.is_candidate() {
                continue;
            }
            let strong_data = class == Class::Strong && is_data(m);
            for key in [Some(m.key), m.also.get()].into_iter().flatten() {
                out.push(Pull { key: key.0, strong_data });
            }
        }
    });
    // `Collected` exposes the units' rows; rebuild one flat list.
    let mut items = Vec::new();
    let mut offs = Vec::with_capacity(got.units() + 1);
    for u in 0..got.units() {
        offs.push(items.len());
        items.extend_from_slice(got.unit(u));
    }
    offs.push(items.len());
    Pulls { items, offsets: offs }
}

/// An archive's run of members from `start`: the files that follow with
/// the same archive and a lazy kind.
fn archive_end<V: View>(view: &V, kinds: &[FileKind], start: usize) -> usize {
    let archive = view.file(FileId(start as u32)).archive;
    let mut end = start + 1;
    while end < kinds.len()
        && kinds[end].is_lazy()
        && archive.is_some()
        && view.file(FileId(end as u32)).archive == archive
    {
        end += 1;
    }
    end
}

/// Simulates GNU ld's walk over the inputs in command-line (`FileId`)
/// order and compares it with mortar's closure (§4.6).
#[allow(clippy::too_many_arguments)]
pub(crate) fn simulate<V: View>(
    view: &V,
    kinds: &[FileKind],
    redirect: &[SymbolId],
    roots: &[Root],
    live: &[FileLive],
    def: &[Def],
    keepers: &Keepers,
    pool: &Pool,
) -> Simulation {
    simulate_with(view, kinds, redirect, roots, live, def, keepers, pool, false)
}

/// [`simulate`]; `naive` visits every member in every pass (test
/// support: the definition the pending sets are checked against).
#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
pub(crate) fn simulate_with<V: View>(
    view: &V,
    kinds: &[FileKind],
    redirect: &[SymbolId],
    roots: &[Root],
    live: &[FileLive],
    def: &[Def],
    keepers: &Keepers,
    pool: &Pool,
    naive: bool,
) -> Simulation {
    let _ = keepers;
    let keys = view.keys() as usize;
    let n = kinds.len();
    let pulls = pulls(view, kinds, pool);
    let by_key = ByKey::new(keys, &pulls);
    let mut walk = Walk {
        view,
        redirect,
        state: vec![State::None; keys],
        definer: vec![u64::MAX; keys],
        referrer: vec![u64::MAX; keys],
        loaded: vec![false; n],
        buf: Vec::new(),
        pending: BTreeSet::new(),
        by_key,
        naive,
    };
    // `-u`, `--require-defined` and the entry are undefined references
    // before the first file.
    for root in roots {
        let k = root.key.0 as usize;
        if walk.state[k] < State::Undefined {
            walk.set(k, State::Undefined);
        }
        if walk.referrer[k] == u64::MAX {
            walk.referrer[k] = entry_word(FileId::INTERNAL, mortar_elf::SymbolIndex(root.arg));
        }
    }
    let mut f = 0;
    while f < n {
        let group = view.file(FileId(f as u32)).group;
        if group.is_none() {
            f = step(&mut walk, kinds, &pulls, f);
            continue;
        }
        // A `--start-group` region: the files that follow in the group.
        let mut end = f + 1;
        while end < n && view.file(FileId(end as u32)).group == group {
            end += 1;
        }
        let mut archives = Vec::new();
        let mut i = f;
        while i < end {
            if kinds[i].is_lazy() {
                let stop = archive_end(view, kinds, i).min(end);
                archives.push(i..stop);
                walk.scan(i..stop, &pulls);
                i = stop;
            } else {
                i = step(&mut walk, kinds, &pulls, i);
            }
        }
        loop {
            let mut any = false;
            for a in &archives {
                any |= walk.scan(a.clone(), &pulls);
            }
            if !any {
                break;
            }
        }
        f = end;
    }
    report(view, kinds, live, def, walk)
}

/// One input outside a group; returns the next file.
fn step<V: View>(walk: &mut Walk<'_, V>, kinds: &[FileKind], pulls: &Pulls, f: usize) -> usize {
    match kinds[f] {
        FileKind::Internal => f + 1,
        FileKind::Object | FileKind::WholeMember => {
            walk.add(FileId(f as u32), false);
            f + 1
        }
        FileKind::Shared => {
            walk.add(FileId(f as u32), true);
            f + 1
        }
        FileKind::LazyMember | FileKind::LazyObject => {
            let end = archive_end(walk.view, kinds, f);
            walk.scan(f..end, pulls);
            end
        }
    }
}

fn report<V: View>(
    view: &V,
    kinds: &[FileKind],
    live: &[FileLive],
    def: &[Def],
    walk: Walk<'_, V>,
) -> Simulation {
    let mut loaded = Vec::new();
    let mut mortar_only = Vec::new();
    let mut gnu_only = Vec::new();
    for f in 0..kinds.len() {
        if !kinds[f].is_lazy() {
            continue;
        }
        let file = FileId(f as u32);
        let ours = matches!(live[f], FileLive::Extracted(_));
        let theirs = walk.loaded[f];
        if theirs {
            loaded.push(file);
        }
        match (ours, theirs) {
            (true, false) => mortar_only.push(file),
            (false, true) => gnu_only.push(file),
            _ => {}
        }
    }
    // The keys defined from an extracted member, by file, then whether
    // the definition is in a COMDAT group.
    let mut candidates: Vec<(SymEntry, SymbolId)> = def
        .iter()
        .enumerate()
        .filter_map(|(k, d)| {
            let e = d.entry()?;
            matches!(live[e.file.0 as usize], FileLive::Extracted(_))
                .then_some((e, SymbolId(k as u32)))
        })
        .collect();
    candidates.sort_unstable_by_key(|&(e, k)| (e.file, e.index, k));
    let mut problems = Vec::new();
    let mut map = SectionMap::default();
    let mut ignore = Vec::new();
    let mut current = None;
    for (e, key) in candidates {
        if current != Some(e.file) {
            current = Some(e.file);
            map.fill(view, e.file, view.file(e.file).sections, &[], false, &mut ignore);
        }
        let in_group = view.mention(e.file, e.index).is_some_and(|m| match m.place {
            SymbolSection::Section(s) => map.group_of(s.0).is_some(),
            _ => false,
        });
        if in_group {
            continue;
        }
        let k = key.0 as usize;
        let referrer = (walk.referrer[k] != u64::MAX).then(|| word_entry(walk.referrer[k]));
        match walk.state[k] {
            State::Undefined => {
                if let Some(referrer) = referrer {
                    problems.push(Problem::at(
                        e,
                        What::ArchiveBackref { key, member: e.file, referrer },
                    ));
                }
            }
            State::DefinedWeak | State::Common | State::Defined => {
                let gnu = word_entry(walk.definer[k]);
                // The same member of one archive mentioned twice is the
                // same definition: no choice to report.
                let same = view.file(gnu.file).source == view.file(e.file).source;
                if gnu.file != e.file && !same {
                    problems.push(Problem::at(
                        e,
                        What::ArchiveChoice {
                            key,
                            mortar: e,
                            gnu,
                            referrer: referrer.unwrap_or(gnu),
                        },
                    ));
                }
            }
            State::None | State::WeakUndefined => {}
        }
    }
    Simulation { loaded, mortar_only, gnu_only, state: walk.state, problems }
}

/// Test support: the simulation of `resolved` (a result of
/// [`crate::resolve`] over `view` with `opts`) recomputed by visiting every
/// member in every pass, the definition the pending sets implement.
pub fn naive<V: View>(view: &V, opts: &crate::Options, resolved: &crate::Resolved) -> Simulation {
    let kinds = crate::live::kinds(view);
    simulate_with(
        view,
        &kinds,
        &resolved.redirect,
        &opts.roots(),
        &resolved.file_live,
        &resolved.def,
        &resolved.keepers,
        &Pool::sequential(),
        true,
    )
}

//! Liveness of files as reachability (`spec/resolve.md` §4.1, §4.2,
//! §12.1-§12.2).
//!
//! 1. [`targets`]: `first(K)` and `cfirst(K)`, atomic minima of `FileId`
//!    over the mentions of every file, live or lazy, before the closure.
//! 2. [`closure`]: the files reachable from the roots, breadth-first, one
//!    parallel map per round over the round's files; the pulling reference
//!    of an extracted member is an atomic minimum of the entry words of the
//!    live mentions with an edge to it.
//!
//! [`oracle`] is the sequential definition (a worklist in `FileId` order),
//! which the tests compare with the rounds.

use std::sync::Mutex;
use std::sync::atomic::Ordering::Relaxed;

use mortar_core::par::Pool;
use mortar_core::resolution::FileLive;
use mortar_core::{FileId, OptId, SymbolId};
use mortar_elf::SymbolIndex;

use crate::comdat::SectionMap;
use crate::mention::{Bits, Class, atomics_u32, atomics_u64, classify, is_data, min_u32, min_u64};
use crate::options::Root;
use crate::view::{FileKind, Mention, View, entry_word, word_entry};

/// No file: the cell of a key with no qualifying mention.
pub const NO_FILE: u32 = u32::MAX;

/// The two targets of every key (§4.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Targets {
    /// `first(K)`: the lowest `FileId` with a defined or common mention of
    /// `K`, of any binding ([`NO_FILE`] for none).
    pub first: Vec<u32>,
    /// `cfirst(K)`: the target of a common.
    pub cfirst: Vec<u32>,
}

/// The kinds of the files, read once.
pub(crate) fn kinds<V: View>(view: &V) -> Vec<FileKind> {
    (0..view.files()).map(|f| view.file(FileId(f)).kind).collect()
}

/// Whether a file's definitions count for `first`/`cfirst` (§4.1): every
/// file but a closure shared object (`spec/dyn.md` D-12: closure files
/// never give `def`).
pub(crate) fn defines<V: View>(view: &V, kind: FileKind, file: FileId) -> bool {
    kind != FileKind::Shared || view.shared(file).is_some_and(|s| !s.closure)
}

/// Whether a file that is not lazy is a source of edges from the start
/// (§4.1, §4.2): every one but a shared object whose references do not
/// extract (`spec/dyn.md` D-12, Q1: an as-needed one or a closure file).
pub(crate) fn extracts<V: View>(view: &V, kind: FileKind, file: FileId) -> bool {
    kind != FileKind::Shared || view.shared(file).is_some_and(crate::view::SharedInfo::extracts)
}

/// `first(K)` and `cfirst(K)` (§4.1, §12.1 first row).
pub fn targets<V: View>(view: &V, pool: &Pool) -> Targets {
    let kinds = kinds(view);
    targets_with(view, &kinds, pool)
}

pub(crate) fn targets_with<V: View>(view: &V, kinds: &[FileKind], pool: &Pool) -> Targets {
    targets_demoted(view, kinds, None, pool)
}

/// `first(K)` and `cfirst(K)` without the definitions `prior` demotes
/// (§6.2, WP-200): a mention of a file live in `prior` defined in a
/// section of a copy its keepers drop is not a definition, it is an
/// undefined mention of its key from the moment its copy is dropped.
fn targets_demoted<V: View>(
    view: &V,
    kinds: &[FileKind],
    prior: Option<Prior<'_>>,
    pool: &Pool,
) -> Targets {
    let keys = view.keys() as usize;
    let first = atomics_u32(keys, NO_FILE);
    let cfirst = atomics_u32(keys, NO_FILE);
    let init = || (Vec::new(), SectionMap::default(), Vec::new());
    pool.unit_cost(280).for_each_init(kinds.len(), init, |(buf, map, scratch), f| {
        let kind = kinds[f];
        let file = FileId(f as u32);
        if !defines(view, kind, file) || kind == FileKind::Internal {
            return;
        }
        buf.clear();
        view.mentions(file, buf);
        let demote = prior.is_some_and(|p| p.fill(view, kind, file, map, scratch));
        let lazy = kind.is_lazy();
        for m in buf.iter() {
            let (class, _) = classify(m, mortar_core::SymEntry { file, index: m.index });
            if !class.is_candidate() || (demote && is_demoted(m, map)) {
                continue;
            }
            let c =
                if lazy { class == Class::Strong && is_data(m) } else { class == Class::Strong };
            for key in [Some(m.key), m.also.get()].into_iter().flatten() {
                min_u32(&first[key.0 as usize], file.0);
                if c {
                    min_u32(&cfirst[key.0 as usize], file.0);
                }
            }
        }
    });
    Targets {
        first: first.into_iter().map(std::sync::atomic::AtomicU32::into_inner).collect(),
        cfirst: cfirst.into_iter().map(std::sync::atomic::AtomicU32::into_inner).collect(),
    }
}

/// A [`Prior`]'s columns, owned between iterations.
pub(crate) type PriorOwned = (Vec<FileLive>, Vec<OptId<FileId>>, Vec<bool>);

/// The previous iteration of the closure (§4.1, §6.2; WP-200): its live
/// files, the keepers over them, and per key whether no live object
/// defines it outside a dropped copy (`open`). A mention of one of those
/// files in a section its keepers drop is demoted: an undefined mention of
/// its key with its binding, for the edges and the targets of the next
/// iteration.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Prior<'a> {
    pub live: &'a [FileLive],
    pub keeper: &'a [OptId<FileId>],
    pub open: &'a [bool],
}

impl Prior<'_> {
    /// Fills `map` for `file` when it was live and has groups; whether
    /// its mentions can be demoted.
    fn fill<V: View>(
        self,
        view: &V,
        kind: FileKind,
        file: FileId,
        map: &mut SectionMap,
        scratch: &mut Vec<crate::problem::Problem>,
    ) -> bool {
        if !kind.is_object() || !self.live.get(file.0 as usize).is_some_and(|l| l.is_live()) {
            return false;
        }
        scratch.clear();
        map.fill(view, file, view.file(file).sections, self.keeper, false, scratch);
        map.any
    }
}

/// Whether mention `m` is defined in a section `map` drops (§6.2).
#[inline]
pub(crate) fn is_demoted(m: &Mention, map: &SectionMap) -> bool {
    match m.place {
        mortar_elf::SymbolSection::Section(s) => map.is_dropped(s.0),
        _ => false,
    }
}

/// The lazy target of the edge out of a demoted mention (§6.2, WP-200),
/// given that no live object defines its key outside a dropped copy: an
/// undefined mention of its own key with its binding, so a weak one has
/// none. A key a live object defines (the keeper's copy) takes no member:
/// the mention binds to that definition (GNU ld: the i386 thunks
/// `__x86.get_pc_thunk.*`, a global in a group of every object, extract
/// no lower member that has a copy too).
#[inline]
fn demoted_edge(m: &Mention, targets: &Targets, kinds: &[FileKind]) -> Option<u32> {
    if m.binding == mortar_elf::consts::STB_WEAK {
        return None;
    }
    let target = targets.first[m.key.0 as usize];
    (target != NO_FILE && kinds[target as usize].is_lazy()).then_some(target)
}

/// Whether the definition the demoted mention `m` of the live file `file`
/// had is `first` or `cfirst` of its key (§6.2, WP-200), which then moves
/// on. A link without a mismatched COMDAT copy has none: a kept copy
/// defining the key has a lower `FileId`.
#[inline]
pub(crate) fn feeds_back(m: &Mention, file: FileId, targets: &Targets) -> bool {
    [Some(m.key), m.also.get()].into_iter().flatten().any(|key| {
        let k = key.0 as usize;
        targets.first[k] == file.0 || targets.cfirst[k] == file.0
    })
}

/// Whether the demoted mention `m` would extract a member `live` left
/// out, if no live object defines its key (§6.2, WP-200).
#[inline]
pub(crate) fn would_pull(
    m: &Mention,
    targets: &Targets,
    kinds: &[FileKind],
    live: &[FileLive],
) -> bool {
    demoted_edge(m, targets, kinds).is_some_and(|t| !live[t as usize].is_live())
}

/// The targets of the iteration after `prior` (§6.2, WP-200): `targets`
/// without the definitions `prior` demotes. Never lower than `targets`
/// (a `--defsym` key keeps [`NO_FILE`]).
pub(crate) fn next_targets<V: View>(
    view: &V,
    kinds: &[FileKind],
    targets: &Targets,
    prior: Prior<'_>,
    pool: &Pool,
) -> Targets {
    let mut next = targets_demoted(view, kinds, Some(prior), pool);
    for (n, t) in next.first.iter_mut().zip(&targets.first) {
        *n = (*n).max(*t);
    }
    for (n, t) in next.cfirst.iter_mut().zip(&targets.cfirst) {
        *n = (*n).max(*t);
    }
    next
}

/// The lazy target of the edge out of mention `m` (§4.1 edges 1 and 2),
/// if it has one.
#[inline]
pub(crate) fn edge(
    m: &Mention,
    targets: &Targets,
    redirect: &[SymbolId],
    kinds: &[FileKind],
) -> Option<u32> {
    let target = match m.place {
        mortar_elf::SymbolSection::Undefined => {
            if m.binding == mortar_elf::consts::STB_WEAK {
                return None;
            }
            targets.first[redirect[m.key.0 as usize].0 as usize]
        }
        mortar_elf::SymbolSection::Common => targets.cfirst[m.key.0 as usize],
        _ => return None,
    };
    (target != NO_FILE && kinds[target as usize].is_lazy()).then_some(target)
}

/// The result of the closure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Closure {
    /// `file_live` (§4.1): `Always` for the roots, `Extracted(pulling
    /// reference)` for the lazy files reached, `Dead` otherwise.
    pub live: Vec<FileLive>,
    /// The number of rounds after round 0 (the depth of the graph).
    pub rounds: u32,
}

impl Closure {
    #[must_use]
    pub fn is_live(&self, file: FileId) -> bool {
        self.live[file.0 as usize].is_live()
    }
}

/// A frontier smaller than this runs on the calling thread: a deep chain
/// of single files costs one visit per file, not one parallel call each.
/// The result does not depend on it (§12.2).
const INLINE_FRONTIER: usize = 16;

/// The closure of §4.1 from the roots of §4.2, in parallel rounds.
/// `sources` (per `FileId`, may be short) are the shared objects whose
/// references are edges in this stage although [`extracts`] says no: the
/// members of N under `--as-needed` of an earlier stage (`spec/dyn.md`
/// D-12, WP-167).
#[allow(clippy::too_many_arguments, reason = "the closure's inputs, one per column")]
pub(crate) fn closure<V: View>(
    view: &V,
    kinds: &[FileKind],
    targets: &Targets,
    redirect: &[SymbolId],
    roots: &[Root],
    sources: &[bool],
    prior: Option<Prior<'_>>,
    pool: &Pool,
) -> Closure {
    let n = kinds.len();
    let live = Bits::new(n);
    let pull = atomics_u64(n, u64::MAX);
    let mut frontier: Vec<u32> = Vec::new();
    for (f, kind) in kinds.iter().enumerate() {
        let source = sources.get(f).copied().unwrap_or(false);
        let before = prior.and_then(|p| p.live.get(f)).copied();
        if let Some(FileLive::Extracted(e)) = before {
            min_u64(&pull[f], entry_word(e.file, e.index));
        }
        if ((source || extracts(view, *kind, FileId(f as u32))) && !kind.is_lazy())
            || before.is_some_and(FileLive::is_live)
        {
            live.set(f);
            if *kind != FileKind::Internal {
                frontier.push(f as u32);
            }
        }
    }
    for root in roots {
        let target = targets.first[root.key.0 as usize];
        if target != NO_FILE && kinds[target as usize].is_lazy() {
            min_u64(&pull[target as usize], entry_word(FileId::INTERNAL, SymbolIndex(root.arg)));
            if live.set(target as usize) {
                frontier.push(target);
            }
        }
    }
    frontier.sort_unstable();
    let mut rounds = 0;
    let inline = Pool::sequential();
    let next = Mutex::new(Vec::new());
    let mut first = true;
    while !frontier.is_empty() {
        if !first {
            rounds += 1;
        }
        first = false;
        let run = if frontier.len() < INLINE_FRONTIER { &inline } else { pool };
        run.unit_cost(1500).for_each_init(
            frontier.len(),
            || (Vec::new(), Vec::new(), SectionMap::default(), Vec::new()),
            |(buf, found, map, scratch), u| {
                let file = FileId(frontier[u]);
                buf.clear();
                view.mentions(file, buf);
                let demote =
                    prior.is_some_and(|p| p.fill(view, kinds[file.0 as usize], file, map, scratch));
                for m in buf.iter() {
                    let target = if demote && is_demoted(m, map) {
                        prior
                            .filter(|p| p.open[m.key.0 as usize])
                            .and_then(|_| demoted_edge(m, targets, kinds))
                    } else {
                        edge(m, targets, redirect, kinds)
                    };
                    if let Some(target) = target {
                        min_u64(&pull[target as usize], m.word(file));
                        if live.set(target as usize) {
                            found.push(target);
                        }
                    }
                }
                if !found.is_empty() {
                    next.lock().unwrap_or_else(std::sync::PoisonError::into_inner).append(found);
                }
            },
        );
        frontier =
            std::mem::take(&mut *next.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        frontier.sort_unstable();
    }
    let live = (0..n).map(|f| finish(kinds[f], live.get(f), pull[f].load(Relaxed))).collect();
    Closure { live, rounds }
}

fn finish(kind: FileKind, live: bool, pull: u64) -> FileLive {
    if !live {
        FileLive::Dead
    } else if kind.is_lazy() {
        FileLive::Extracted(word_entry(pull))
    } else {
        FileLive::Always
    }
}

/// The sequential definition of §4.1, for tests: a worklist visited in
/// `FileId` order (lowest first), with plain arrays. `targets` and
/// `redirect` as [`crate::resolve`] computes them.
pub fn oracle<V: View>(
    view: &V,
    targets: &Targets,
    redirect: &[SymbolId],
    roots: &[(SymbolId, u32)],
) -> Vec<FileLive> {
    oracle_with(view, targets, redirect, roots, &[])
}

/// [`oracle`] with the shared objects `sources` (per `FileId`) as sources
/// of edges, as [`crate::resolve`]'s later stages have them (`spec/dyn.md`
/// D-12, WP-167).
///
/// The iterations of §6.2 (WP-200): after each closure, the keepers over
/// its live objects (§6.1); when a demoted mention [`feeds_back`], the
/// closure again from its live files, the demoted mentions undefined with
/// their binding and the targets without them; until nothing changes.
pub fn oracle_with<V: View>(
    view: &V,
    targets: &Targets,
    redirect: &[SymbolId],
    roots: &[(SymbolId, u32)],
    sources: &[bool],
) -> Vec<FileLive> {
    let kinds = kinds(view);
    let seq = Pool::sequential();
    let mut targets = targets.clone();
    let mut prior: Option<PriorOwned> = None;
    loop {
        let p = prior.as_ref().map(|(live, keeper, open)| Prior { live, keeper, open });
        let live = oracle_once(view, &kinds, &targets, redirect, roots, sources, p);
        let objects: Vec<FileId> = (0..kinds.len())
            .filter(|&f| live[f].is_live() && kinds[f].is_object())
            .map(|f| FileId(f as u32))
            .collect();
        let keeper = crate::comdat::keepers(view, &objects, &seq).keeper;
        let mut map = SectionMap::default();
        let mut scratch = Vec::new();
        let mut buf = Vec::new();
        let mut open = vec![true; view.keys() as usize];
        let mut back = false;
        let mut pulls = Vec::new();
        for &file in &objects {
            map.fill(view, file, view.file(file).sections, &keeper, false, &mut scratch);
            buf.clear();
            view.mentions(file, &mut buf);
            for m in &buf {
                let (class, _) = classify(m, mortar_core::SymEntry { file, index: m.index });
                if !class.is_candidate() {
                    continue;
                }
                if map.any && is_demoted(m, &map) {
                    back |= feeds_back(m, file, &targets);
                    if would_pull(m, &targets, &kinds, &live) {
                        pulls.push(m.key);
                    }
                } else {
                    for key in [Some(m.key), m.also.get()].into_iter().flatten() {
                        open[key.0 as usize] = false;
                    }
                }
            }
        }
        let again = back || pulls.iter().any(|k| open[k.0 as usize]);
        if !again {
            return live;
        }
        let p = Prior { live: &live, keeper: &keeper, open: &open };
        let next = next_targets(view, &kinds, &targets, p, &seq);
        if next == targets
            && prior.as_ref().is_some_and(|(l, k, o)| *l == live && *k == keeper && *o == open)
        {
            return live;
        }
        targets = next;
        prior = Some((live, keeper, open));
    }
}

/// One closure of [`oracle_with`], sequentially.
fn oracle_once<V: View>(
    view: &V,
    kinds: &[FileKind],
    targets: &Targets,
    redirect: &[SymbolId],
    roots: &[(SymbolId, u32)],
    sources: &[bool],
    prior: Option<Prior<'_>>,
) -> Vec<FileLive> {
    let n = kinds.len();
    let mut live = vec![false; n];
    let mut pull = vec![u64::MAX; n];
    let mut work = std::collections::BTreeSet::new();
    for (f, kind) in kinds.iter().enumerate() {
        let source = sources.get(f).copied().unwrap_or(false);
        let before = prior.and_then(|p| p.live.get(f)).copied();
        if let Some(FileLive::Extracted(e)) = before {
            pull[f] = pull[f].min(entry_word(e.file, e.index));
        }
        if ((source || extracts(view, *kind, FileId(f as u32))) && !kind.is_lazy())
            || before.is_some_and(FileLive::is_live)
        {
            live[f] = true;
            work.insert(f as u32);
        }
    }
    for &(key, arg) in roots {
        let target = targets.first[key.0 as usize];
        if target != NO_FILE && kinds[target as usize].is_lazy() {
            let w = entry_word(FileId::INTERNAL, SymbolIndex(arg));
            pull[target as usize] = pull[target as usize].min(w);
            if !live[target as usize] {
                live[target as usize] = true;
                work.insert(target);
            }
        }
    }
    let mut buf = Vec::new();
    let mut map = SectionMap::default();
    let mut scratch = Vec::new();
    while let Some(f) = work.pop_first() {
        if kinds[f as usize] == FileKind::Internal {
            continue;
        }
        buf.clear();
        view.mentions(FileId(f), &mut buf);
        let demote = prior
            .is_some_and(|p| p.fill(view, kinds[f as usize], FileId(f), &mut map, &mut scratch));
        for m in &buf {
            let target = if demote && is_demoted(m, &map) {
                prior
                    .filter(|p| p.open[m.key.0 as usize])
                    .and_then(|_| demoted_edge(m, targets, kinds))
            } else {
                edge(m, targets, redirect, kinds)
            };
            if let Some(target) = target {
                let t = target as usize;
                pull[t] = pull[t].min(m.word(FileId(f)));
                if !live[t] {
                    live[t] = true;
                    work.insert(target);
                }
            }
        }
    }
    (0..n).map(|f| finish(kinds[f], live[f], pull[f])).collect()
}

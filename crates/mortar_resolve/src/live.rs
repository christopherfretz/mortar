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
use mortar_core::{FileId, SymbolId};
use mortar_elf::SymbolIndex;

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
    let keys = view.keys() as usize;
    let first = atomics_u32(keys, NO_FILE);
    let cfirst = atomics_u32(keys, NO_FILE);
    pool.unit_cost(280).for_each_init(kinds.len(), Vec::new, |buf: &mut Vec<Mention>, f| {
        let kind = kinds[f];
        let file = FileId(f as u32);
        if !defines(view, kind, file) || kind == FileKind::Internal {
            return;
        }
        buf.clear();
        view.mentions(file, buf);
        let lazy = kind.is_lazy();
        for m in buf.iter() {
            let (class, _) = classify(m, mortar_core::SymEntry { file, index: m.index });
            if !class.is_candidate() {
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
pub(crate) fn closure<V: View>(
    view: &V,
    kinds: &[FileKind],
    targets: &Targets,
    redirect: &[SymbolId],
    roots: &[Root],
    sources: &[bool],
    pool: &Pool,
) -> Closure {
    let n = kinds.len();
    let live = Bits::new(n);
    let pull = atomics_u64(n, u64::MAX);
    let mut frontier: Vec<u32> = Vec::new();
    for (f, kind) in kinds.iter().enumerate() {
        let source = sources.get(f).copied().unwrap_or(false);
        if (source || extracts(view, *kind, FileId(f as u32))) && !kind.is_lazy() {
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
            || (Vec::new(), Vec::new()),
            |(buf, found): &mut (Vec<Mention>, Vec<u32>), u| {
                let file = FileId(frontier[u]);
                buf.clear();
                view.mentions(file, buf);
                for m in buf.iter() {
                    if let Some(target) = edge(m, targets, redirect, kinds) {
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
pub fn oracle_with<V: View>(
    view: &V,
    targets: &Targets,
    redirect: &[SymbolId],
    roots: &[(SymbolId, u32)],
    sources: &[bool],
) -> Vec<FileLive> {
    let kinds = kinds(view);
    let n = kinds.len();
    let mut live = vec![false; n];
    let mut pull = vec![u64::MAX; n];
    let mut work = std::collections::BTreeSet::new();
    for (f, kind) in kinds.iter().enumerate() {
        let source = sources.get(f).copied().unwrap_or(false);
        if (source || extracts(view, *kind, FileId(f as u32))) && !kind.is_lazy() {
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
    while let Some(f) = work.pop_first() {
        if kinds[f as usize] == FileKind::Internal {
            continue;
        }
        buf.clear();
        view.mentions(FileId(f), &mut buf);
        for m in &buf {
            if let Some(target) = edge(m, targets, redirect, &kinds) {
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

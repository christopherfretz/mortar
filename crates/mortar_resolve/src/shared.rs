//! Shared objects in a link (`spec/dyn.md` D-11 to D-21, D-27;
//! `spec/resolve.md` §3.2 rules 2-3, §3.5, §5.2, §9).
//!
//! Run once after `def` of the program is known (the closure, D1, the
//! linker's symbols): nothing here feeds back into extraction (Q1).
//!
//! 1. The definitions of every loaded shared object, by key: `giver(k)`
//!    (the first member of C by `FileId`) decides neededness only.
//! 2. N, the least fixed point of D-11, by a worklist (item 3 depends on N
//!    only through "`x` ∈ N"); U = reach(N) \ N (D-12).
//! 3. The keeping mention of every member of N and the `DT_NEEDED` order
//!    (D-19a), then the loader order and `bind(k)` (D-10 setting).
//! 4. `def` of the keys the program does not define becomes `Shared` of
//!    `bind(k)`; D-13, §3.5's `non-exported-symbol`, D-16, Q1's refusal;
//!    the columns `ref_dynamic`, `defined_dynamic`, `exported`,
//!    `imported` (D-12, D-21).

use std::collections::{BTreeSet, HashMap, VecDeque};

use mortar_core::par::Pool;
use mortar_core::resolution::FileLive;
use mortar_core::{FileId, SymEntry, SymbolId};
use mortar_elf::consts::{STB_WEAK, STT_TLS, STV_HIDDEN};
use mortar_elf::{SymbolIndex, SymbolSection};

use crate::export;
use crate::linker;
use crate::live::{NO_FILE, Targets};
use crate::mention::{Class, classify, is_data};
use crate::options::{MentionGroup, Options, ShlibUndefined};
use crate::problem::{Problem, What};
use crate::resolve::{Def, Props, flags};
use crate::view::{FileKind, Mention, SharedInfo, View};

/// What the shared stage adds to a resolution.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Dynamic {
    /// The needed objects in `DT_NEEDED` order (D-19a): `Slots.needed`.
    pub needed: Vec<FileId>,
    /// The members of U in breadth-first loader order (D-10 setting).
    pub used: Vec<FileId>,
    /// Per key (D-12, D-21, resolve §9.6).
    pub exported: Vec<bool>,
    pub imported: Vec<bool>,
    pub ref_dynamic: Vec<bool>,
    pub defined_dynamic: Vec<bool>,
    /// Per key: preemptible in a shared object (`spec/dyn.md` D-112, a
    /// subset of `exported`); empty (no room) outside `Shared` (WP-117).
    pub preemptible: Vec<bool>,
    /// Keys a relocatable object or file 0 refers to strongly that stay
    /// undefined while a closure file (W) defines them: the note `defined
    /// in <c>, which is not on the command line` (D-12), with the first
    /// such file. Sorted by key.
    pub closure_only: Vec<(SymbolId, FileId)>,
    /// Per key: the output version index of an export of a shared object
    /// (`crate::version::apply`; 0 unversioned); empty outside `Shared`.
    pub versions: Vec<u16>,
    /// The version stage's diagnostics (WP-125).
    pub version_problems: Vec<crate::version::VersionProblem>,
}

/// One visible definition in a loaded shared object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ShDef {
    key: SymbolId,
    file: FileId,
    index: SymbolIndex,
    data: bool,
    tls: bool,
}

/// The shared objects of the link and their mentions, read once.
struct Loaded {
    /// The shared files, in `FileId` order.
    files: Vec<FileId>,
    /// Per shared file (same order): its mentions, its `DT_NEEDED` edges.
    mentions: Vec<Vec<Mention>>,
    edges: Vec<Vec<Option<FileId>>>,
    /// Per `FileId`: its index in `files`, or `usize::MAX`.
    slot: Vec<usize>,
    /// Per `FileId`: a member of C (a shared object that is not a closure
    /// file).
    in_c: Vec<bool>,
    /// Every visible definition, sorted by `(key, file, index)`; a default
    /// definition `foo@@V` is listed under `foo` and `foo@V`.
    defs: Vec<ShDef>,
}

impl Loaded {
    fn read<V: View>(view: &V, kinds: &[FileKind], pool: &Pool) -> Self {
        let files: Vec<FileId> = (0..kinds.len())
            .filter(|&f| kinds[f] == FileKind::Shared && view.shared(FileId(f as u32)).is_some())
            .map(|f| FileId(f as u32))
            .collect();
        let read: Vec<(Vec<Mention>, Vec<Option<FileId>>)> =
            pool.unit_cost(20_000).map(files.len(), |i| {
                let mut m = Vec::new();
                view.mentions(files[i], &mut m);
                let mut e = Vec::new();
                view.needed(files[i], &mut e);
                (m, e)
            });
        let (mentions, edges): (Vec<_>, Vec<_>) = read.into_iter().unzip();
        let mut slot = vec![usize::MAX; kinds.len()];
        let mut in_c = vec![false; kinds.len()];
        for (i, &f) in files.iter().enumerate() {
            slot[f.0 as usize] = i;
            in_c[f.0 as usize] = view.shared(f).is_some_and(|s| !s.closure);
        }
        let mut defs = Vec::new();
        for (i, &file) in files.iter().enumerate() {
            for m in &mentions[i] {
                let (class, _) = classify(m, SymEntry { file, index: m.index });
                if !class.is_candidate() {
                    continue;
                }
                let tls = m.sym_type == STT_TLS;
                for key in [Some(m.key), m.also.get()].into_iter().flatten() {
                    defs.push(ShDef { key, file, index: m.index, data: is_data(m), tls });
                }
            }
        }
        defs.sort_unstable();
        Self { files, mentions, edges, slot, in_c, defs }
    }

    /// The visible definitions of `key`, in `FileId` order.
    fn defs_of(&self, key: SymbolId) -> &[ShDef] {
        let from = self.defs.partition_point(|d| d.key < key);
        let to = from + self.defs[from..].partition_point(|d| d.key == key);
        &self.defs[from..to]
    }

    fn edges_of(&self, file: FileId) -> &[Option<FileId>] {
        &self.edges[self.slot[file.0 as usize]]
    }

    fn mentions_of(&self, file: FileId) -> &[Mention] {
        &self.mentions[self.slot[file.0 as usize]]
    }

    /// `reach(S)` (D-10 setting): `S` and every loaded object reachable
    /// along `DT_NEEDED` edges.
    fn reach(&self, from: impl IntoIterator<Item = FileId>) -> Vec<bool> {
        let mut seen = vec![false; self.slot.len()];
        let mut work: Vec<FileId> = Vec::new();
        for f in from {
            if !seen[f.0 as usize] {
                seen[f.0 as usize] = true;
                work.push(f);
            }
        }
        while let Some(f) = work.pop() {
            for &e in self.edges_of(f).iter().flatten() {
                if !seen[e.0 as usize] {
                    seen[e.0 as usize] = true;
                    work.push(e);
                }
            }
        }
        seen
    }
}

/// Whether the program defines `k` for D-11 (`giver`, `bind`): a strong,
/// weak or linker definition, or a common no member of C gives way to
/// (resolve §3.2 rule 2: a common yields to a shared data definition).
fn program_defines(def: Def, rule2: bool) -> bool {
    match def {
        Def::Strong(_) | Def::Weak(_) | Def::Provide { .. } => true,
        Def::Common(_) => !rule2,
        Def::Undefined { .. } | Def::Shared(_) => false,
    }
}

/// The shared stage. `file_live` gets the `Shared` rows (N, U, `Dead`),
/// `def` the `Shared` definitions; problems are appended.
#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "the steps of the module docs"
)]
pub(crate) fn link<V: View>(
    view: &V,
    opts: &Options,
    kinds: &[FileKind],
    targets: &Targets,
    redirect: &[SymbolId],
    file_live: &mut [FileLive],
    def: &mut [Def],
    props: &mut [Props],
    problems: &mut Vec<Problem>,
    pool: &Pool,
) -> Dynamic {
    let keys = def.len();
    let l = Loaded::read(view, kinds, pool);
    let n_files = kinds.len();

    // Rule 2 (resolve §3.2): a common with a data definition in C.
    let rule2 = |k: usize| -> bool {
        matches!(def[k], Def::Common(_))
            && l.defs_of(SymbolId(k as u32)).iter().any(|d| d.data && l.in_c[d.file.0 as usize])
    };
    // giver(k): the first member of C by FileId with a visible definition
    // (a data definition when rule 2 applies).
    let giver = |k: usize, def: &[Def]| -> Option<FileId> {
        let r2 = rule2(k);
        if program_defines(def[k], r2) {
            return None;
        }
        l.defs_of(SymbolId(k as u32))
            .iter()
            .find(|d| l.in_c[d.file.0 as usize] && (!r2 || d.data))
            .map(|d| d.file)
    };

    // ---- N (D-11) ---------------------------------------------------------
    let mut in_n = vec![false; n_files];
    for &f in &l.files {
        if l.in_c[f.0 as usize] && view.shared(f).is_some_and(|s| !s.as_needed) {
            in_n[f.0 as usize] = true;
        }
    }
    // Item 2: keys with a strong reference from a live relocatable object
    // or file 0 (D1's `ref_strong`), or a common that yields (rule 2).
    let item2: Vec<Option<FileId>> = pool.unit_cost(4).map(keys, |k| {
        let strong = props[k].flags & flags::REF_STRONG != 0 || rule2(k);
        if strong { giver(k, def) } else { None }
    });
    for g in item2.iter().flatten() {
        in_n[g.0 as usize] = true;
    }
    let n0: Vec<FileId> = l.files.iter().copied().filter(|f| in_n[f.0 as usize]).collect();
    let reach_n0 = l.reach(n0.iter().copied());
    // Item 3, as a worklist; `reasons3` keeps every (x, s) pair of item 3
    // for D-19a.
    let mut work: VecDeque<FileId> = n0.iter().copied().collect();
    let mut reasons3: Vec<(FileId, FileId)> = Vec::new();
    while let Some(x) = work.pop_front() {
        let mut reach_x: Option<Vec<bool>> = None;
        for m in l.mentions_of(x) {
            if m.place != SymbolSection::Undefined || m.binding == STB_WEAK {
                continue;
            }
            let Some(s) = giver(m.key.0 as usize, def) else { continue };
            if reach_n0[s.0 as usize] {
                continue;
            }
            let rx = reach_x.get_or_insert_with(|| l.reach([x]));
            if rx[s.0 as usize] {
                continue;
            }
            reasons3.push((x, s));
            if !in_n[s.0 as usize] {
                in_n[s.0 as usize] = true;
                work.push_back(s);
            }
        }
    }
    reasons3.sort_unstable();
    reasons3.dedup();

    // ---- U (D-12) ---------------------------------------------------------
    let reach_n = l.reach(l.files.iter().copied().filter(|f| in_n[f.0 as usize]));
    let in_u: Vec<bool> = (0..n_files).map(|f| reach_n[f] && !in_n[f]).collect();
    for &f in &l.files {
        let i = f.0 as usize;
        file_live[i] = if in_n[i] {
            FileLive::Shared { needed: true, used: false }
        } else if in_u[i] {
            FileLive::Shared { needed: false, used: true }
        } else {
            FileLive::Dead
        };
    }

    // ---- D-19a: keeping mentions and the DT_NEEDED order ------------------
    let needed = needed_order(view, opts, &l, file_live, &in_n, &item2, &reasons3, problems, pool);

    // ---- loader order and bind(k) -----------------------------------------
    let mut rank = vec![u32::MAX; n_files];
    let mut order: Vec<FileId> = needed.clone();
    let mut seen = in_n.clone();
    let mut queue: VecDeque<FileId> = needed.iter().copied().collect();
    let mut used = Vec::new();
    while let Some(x) = queue.pop_front() {
        for &e in l.edges_of(x).iter().flatten() {
            if !seen[e.0 as usize] {
                seen[e.0 as usize] = true;
                if in_u[e.0 as usize] {
                    used.push(e);
                    order.push(e);
                }
                queue.push_back(e);
            }
        }
    }
    for (r, f) in order.iter().enumerate() {
        rank[f.0 as usize] = r as u32;
    }
    let machine = view.machine();
    // D-10 setting, `bind(k)` in `Shared` (WP-137): there is no
    // `DT_NEEDED` search (input §8.6), so a member of C that is not needed
    // is dropped and binds nothing, even when a needed object's edge
    // reaches it (U). Only a weak-only key can have every definition
    // outside N (a strong one makes `giver(k)` needed, D-11 item 2): it
    // stays `Undefined{weak: true}` (D-111 item 3), with no version and
    // no `Verneed` (D-27).
    let binds = |f: FileId| -> bool {
        rank[f.0 as usize] != u32::MAX && (opts.shared_output.is_none() || in_n[f.0 as usize])
    };
    let new_def: Vec<Option<Def>> = pool.unit_cost(4).map(keys, |k| {
        let defs = l.defs_of(SymbolId(k as u32));
        if defs.is_empty() || props[k].visibility == STV_HIDDEN {
            return None;
        }
        let r2 = rule2(k);
        if program_defines(def[k], r2) {
            return None;
        }
        // Rule 3: a linker-defined name never binds a shared definition.
        let (name, version) = view.key_name(SymbolId(k as u32));
        if version.is_none() && linker::candidate(name, machine, opts.output).is_some() {
            return None;
        }
        defs.iter()
            .filter(|d| l.in_c[d.file.0 as usize] && binds(d.file))
            .filter(|d| !r2 || d.data)
            .min_by_key(|d| (rank[d.file.0 as usize], d.index.0))
            .map(|d| Def::Shared(SymEntry { file: d.file, index: d.index }))
    });
    for (k, d) in new_def.into_iter().enumerate() {
        if let Some(d) = d {
            // A common that yields (rule 2) is demoted: its mentions are
            // references of the shared definition (§3.1, §3.6).
            if matches!(def[k], Def::Common(_)) {
                props[k].flags |= flags::REF_REGULAR | flags::REF_STRONG;
            }
            def[k] = d;
        }
    }

    // ---- the columns (D-12, D-21) -----------------------------------------
    let live_shared: Vec<FileId> =
        l.files.iter().copied().filter(|f| in_n[f.0 as usize] || in_u[f.0 as usize]).collect();
    let mut ref_dynamic = vec![false; keys];
    let mut defined_dynamic = vec![false; keys];
    // A versioned reference `foo@V` names the program's `foo` too: the
    // program's definition interposes whatever version is asked.
    let base_of = |k: SymbolId| -> Option<SymbolId> {
        let (name, version) = view.key_name(k);
        version.and_then(|_| view.find_key(name))
    };
    for &x in &live_shared {
        for m in l.mentions_of(x) {
            if m.place == SymbolSection::Undefined {
                ref_dynamic[m.key.0 as usize] = true;
                if let Some(b) = base_of(m.key) {
                    ref_dynamic[b.0 as usize] = true;
                }
            } else {
                for key in [Some(m.key), m.also.get()].into_iter().flatten() {
                    defined_dynamic[key.0 as usize] = true;
                }
            }
        }
    }
    let ours = |d: Def| matches!(d, Def::Strong(_) | Def::Weak(_) | Def::Common(_));
    // D-110 item 1 (WP-128): a definition in a member of an archive
    // `--exclude-libs` names is never exported, in every kind (all three
    // drop the exports `-E`, `--export-dynamic-symbol`, a version script's
    // `global:` and a shared object's definition of the name would give).
    let excluded = |d: Def| match d {
        Def::Strong(e) | Def::Weak(e) | Def::Common(e) => {
            opts.exclude_libs.get(e.file.0 as usize).copied().unwrap_or(false)
        }
        _ => false,
    };
    // WP-117: a shared object's export set is D-110's, and preemption
    // D-112's (`crate::export`).
    let mut versions: Vec<u16> = Vec::new();
    let mut version_problems = Vec::new();
    // D-112, §9.6 (WP-128): the keys a `--dynamic-list` lists, by name
    // without a version.
    let listed = |k: usize| {
        opts.dynamic_list
            .as_ref()
            .is_some_and(|list| list.matches(view.key_name(SymbolId(k as u32)).0))
    };
    let exported: Vec<bool> = if opts.shared_output.is_some() {
        let mut e: Vec<bool> = (0..keys)
            .map(|k| {
                !excluded(def[k]) && export::exported(def[k], props[k].visibility, ref_dynamic[k])
            })
            .collect();
        // D-110 item 1, D-114 (WP-125): the version script.
        versions = vec![0; keys];
        version_problems = crate::version::apply(view, &opts.versions, def, &mut e, &mut versions);
        e
    } else {
        (0..keys)
            .map(|k| {
                let d = def[k];
                if props[k].visibility == STV_HIDDEN || excluded(d) {
                    return false;
                }
                if matches!(d, Def::Provide { .. }) {
                    return ref_dynamic[k];
                }
                ours(d)
                    && (ref_dynamic[k]
                        || defined_dynamic[k]
                        || opts.shared.export_dynamic
                        || opts.shared.export_symbols.binary_search(&SymbolId(k as u32)).is_ok()
                        || listed(k))
            })
            .collect()
    };
    let preemptible: Vec<bool> = match opts.shared_output {
        Some(out) => (0..keys)
            .map(|k| {
                let d = def[k];
                let sym_type = match d {
                    Def::Strong(e) | Def::Weak(e) | Def::Common(e) => {
                        view.mention(e.file, e.index).map_or(0, |m| m.sym_type)
                    }
                    _ => 0,
                };
                export::preemptible(
                    d,
                    props[k].visibility,
                    sym_type,
                    exported[k],
                    out.symbolic,
                    listed(k),
                )
            })
            .collect(),
        None => Vec::new(),
    };
    let imported: Vec<bool> = (0..keys)
        .map(|k| matches!(def[k], Def::Shared(_)) && props[k].flags & flags::REF_REGULAR != 0)
        .collect();

    // ---- D-13, §3.5 -------------------------------------------------------
    let live_def = |k: SymbolId| -> bool {
        l.defs_of(k).iter().any(|d| in_n[d.file.0 as usize] || in_u[d.file.0 as usize])
    };
    // An executable's definition excluded by `--exclude-libs` is as a
    // hidden one here (GNU ld and lld refuse a shared object's reference
    // to it; mold links a program that fails at load); in a `Shared`
    // output all three leave the reference to the loader.
    let as_hidden = |d: Def| opts.shared_output.is_none() && excluded(d);
    let program = |k: SymbolId| -> Option<(Def, u8)> {
        let d = def[k.0 as usize];
        let vis = if as_hidden(d) { STV_HIDDEN } else { props[k.0 as usize].visibility };
        (ours(d) || matches!(d, Def::Provide { .. })).then_some((d, vis))
    };
    for &x in &live_shared {
        for m in l.mentions_of(x) {
            if m.place != SymbolSection::Undefined {
                continue;
            }
            let entry = SymEntry { file: x, index: m.index };
            let candidates = [Some(m.key), base_of(m.key)];
            let mut hidden_def = None;
            let mut satisfied = false;
            for k in candidates.into_iter().flatten() {
                if let Some((d, vis)) = program(k) {
                    if vis == STV_HIDDEN {
                        hidden_def = hidden_def.or_else(|| d.entry().map(|e| (k, e)));
                    } else {
                        satisfied = true;
                    }
                }
            }
            if live_def(m.key) {
                satisfied = true;
            }
            if satisfied {
                continue;
            }
            if let Some((key, d)) = hidden_def {
                problems.push(Problem::at(entry, What::NonExported { key, def: d, entry }));
                continue;
            }
            if m.binding == STB_WEAK {
                continue;
            }
            let warn = match opts.shared.shlib_undefined {
                ShlibUndefined::Allow => continue,
                ShlibUndefined::Error => false,
                ShlibUndefined::Warn => true,
            };
            problems.push(Problem::at(
                entry,
                What::ShlibUndefined { file: x, index: m.index, key: m.key, warn },
            ));
        }
    }

    // ---- Q1's refusal (D-12) ----------------------------------------------
    for &x in &l.files {
        if !in_n[x.0 as usize] || view.shared(x).is_none_or(SharedInfo::extracts) {
            continue;
        }
        for m in l.mentions_of(x) {
            if m.place != SymbolSection::Undefined || m.binding == STB_WEAK {
                continue;
            }
            let t = targets.first[redirect[m.key.0 as usize].0 as usize];
            if t != NO_FILE && kinds[t as usize].is_lazy() && !file_live[t as usize].is_live() {
                let entry = SymEntry { file: x, index: m.index };
                problems.push(Problem::at(
                    entry,
                    What::AsNeededExtraction { key: m.key, entry, member: FileId(t) },
                ));
            }
        }
    }

    // ---- D-16: TLS against a shared definition ----------------------------
    let mut tls_keys: Vec<(SymbolId, SymEntry, bool)> = Vec::new();
    for k in 0..keys {
        let Def::Shared(e) = def[k] else { continue };
        let Some(d) =
            l.defs_of(SymbolId(k as u32)).iter().find(|d| d.file == e.file && d.index == e.index)
        else {
            continue;
        };
        let f = props[k].flags;
        let other = if d.tls { flags::NON_TLS } else { flags::TLS };
        if f & other != 0 {
            tls_keys.push((SymbolId(k as u32), e, d.tls));
        }
    }
    if !tls_keys.is_empty() {
        tls_against_shared(view, kinds, file_live, redirect, &tls_keys, problems);
    }

    // ---- D-12's note ------------------------------------------------------
    let mut closure_only = Vec::new();
    for k in 0..keys {
        if !matches!(def[k], Def::Undefined { weak: false })
            || props[k].flags & flags::REF_STRONG == 0
        {
            continue;
        }
        if let Some(d) = l.defs_of(SymbolId(k as u32)).iter().find(|d| !l.in_c[d.file.0 as usize]) {
            closure_only.push((SymbolId(k as u32), d.file));
        }
    }

    Dynamic {
        needed,
        used,
        exported,
        imported,
        ref_dynamic,
        defined_dynamic,
        preemptible,
        closure_only,
        versions,
        version_problems,
    }
}

/// `tls-mismatch` against a shared definition (D-16, resolve §3.4): the
/// first live object mention of the other kind, by `≺`.
fn tls_against_shared<V: View>(
    view: &V,
    kinds: &[FileKind],
    file_live: &[FileLive],
    redirect: &[SymbolId],
    keys: &[(SymbolId, SymEntry, bool)],
    problems: &mut Vec<Problem>,
) {
    let want: HashMap<SymbolId, (SymEntry, bool)> =
        keys.iter().map(|&(k, e, tls)| (k, (e, tls))).collect();
    let mut found: HashMap<SymbolId, SymEntry> = HashMap::new();
    let mut buf = Vec::new();
    for f in 0..kinds.len() {
        if !kinds[f].is_object() || !file_live[f].is_live() {
            continue;
        }
        let file = FileId(f as u32);
        buf.clear();
        view.mentions(file, &mut buf);
        for m in &buf {
            let key = match classify(m, SymEntry { file, index: m.index }).0 {
                Class::Undefined { .. } => redirect[m.key.0 as usize],
                _ => m.key,
            };
            let Some(&(_, def_tls)) = want.get(&key) else { continue };
            let tls = m.sym_type == STT_TLS && m.place != SymbolSection::Common;
            if tls != def_tls {
                found.entry(key).or_insert(SymEntry { file, index: m.index });
            }
        }
    }
    for (key, other) in found {
        let (first, first_tls) = want[&key];
        problems.push(Problem::at(other, What::TlsMismatch { key, first, first_tls, other }));
    }
}

/// The object reasons (D-11 item 2) of one needed object, for D-19a.
#[derive(Default, Clone)]
struct Reasons {
    /// The least position of a live object (or root, 0) giving a reason.
    least: u32,
    /// The groups such objects are in.
    groups: BTreeSet<u32>,
    /// Object reasons after their mention whose group is unknown to us.
    any: bool,
}

/// The `DT_NEEDED` order (D-19a): each needed object at its keeping
/// mention, the needed objects by the positions of those.
#[allow(clippy::too_many_arguments, reason = "the inputs of D-19a")]
#[allow(clippy::too_many_lines, reason = "one walk over the mentions of D-19a")]
fn needed_order<V: View>(
    view: &V,
    opts: &Options,
    l: &Loaded,
    file_live: &[FileLive],
    in_n: &[bool],
    item2: &[Option<FileId>],
    reasons3: &[(FileId, FileId)],
    problems: &mut Vec<Problem>,
    pool: &Pool,
) -> Vec<FileId> {
    let link = &opts.shared;
    // The object reasons (D-11 item 2) of each needed object: the least
    // position of a live relocatable object with a strong reference whose
    // giver it is, and the groups such objects are in; file 0's roots come
    // before every mention.
    let mut reasons: HashMap<FileId, Reasons> = HashMap::new();
    let roots = opts.roots();
    for root in &roots {
        if let Some(g) = item2[root.key.0 as usize] {
            let r = reasons.entry(g).or_insert(Reasons { least: u32::MAX, ..Reasons::default() });
            r.least = 0;
            r.any = true;
        }
    }
    let files = view.files() as usize;
    let objects: Vec<FileId> = (0..files)
        .filter(|&f| {
            link.position.get(f).is_some_and(|&p| p != u32::MAX)
                && file_live[f].is_live()
                && view.file(FileId(f as u32)).kind.is_object()
        })
        .map(|f| FileId(f as u32))
        .collect();
    let per: Vec<Vec<(FileId, u32, Option<u32>)>> = pool.unit_cost(1500).map(objects.len(), |i| {
        let file = objects[i];
        let mut out = Vec::new();
        let info = view.file(file);
        let mut buf = Vec::new();
        view.mentions(file, &mut buf);
        let pos = link.position[file.0 as usize];
        for m in &buf {
            if m.place != SymbolSection::Undefined || m.binding == STB_WEAK {
                continue;
            }
            if let Some(g) = item2[m.key.0 as usize] {
                out.push((g, pos, info.group));
            }
        }
        out
    });
    for rows in per {
        for (g, pos, group) in rows {
            let r = reasons.entry(g).or_insert(Reasons { least: u32::MAX, ..Reasons::default() });
            r.least = r.least.min(pos);
            r.any = true;
            if let Some(group) = group {
                r.groups.insert(group);
            }
        }
    }
    let mut keeping: HashMap<FileId, u32> = HashMap::new();
    let mut first: HashMap<FileId, u32> = HashMap::new();
    let mut unknown: BTreeSet<FileId> = BTreeSet::new();
    for m in &link.mentions {
        let d = m.file;
        if !in_n[d.0 as usize] {
            continue;
        }
        first.entry(d).or_insert(m.position);
        if keeping.contains_key(&d) {
            continue;
        }
        let r = reasons.get(&d);
        let by_object = r.is_some_and(|r| {
            r.least < m.position || matches!(m.group, MentionGroup::In(g) if r.groups.contains(&g))
        });
        let by_shared = reasons3
            .iter()
            .filter(|(_, s)| *s == d)
            .any(|(x, _)| keeping.get(x).is_some_and(|&p| p < m.position));
        let group_unknown = m.group == MentionGroup::Unknown && r.is_some_and(|r| r.any);
        match m.as_needed {
            Some(false) => {
                keeping.insert(d, m.position);
            }
            _ if by_object || by_shared => {
                keeping.insert(d, m.position);
            }
            None => {
                unknown.insert(d);
                keeping.insert(d, m.position);
            }
            Some(true) if group_unknown => {
                unknown.insert(d);
            }
            Some(true) => {}
        }
    }
    for d in unknown {
        problems.push(Problem {
            key: crate::problem::DiagKey::of(d, 0),
            what: What::NeededOrderUnknown { file: d },
        });
    }
    let mut needed: Vec<(u32, FileId)> = l
        .files
        .iter()
        .copied()
        .filter(|f| in_n[f.0 as usize])
        .map(|f| (keeping.get(&f).or(first.get(&f)).copied().unwrap_or(u32::MAX), f))
        .collect();
    needed.sort_unstable();
    needed.into_iter().map(|(_, f)| f).collect()
}

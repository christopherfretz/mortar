//! The definition of every key (`spec/resolve.md` §3, §5, §6.3, §8) and
//! the assembly of stage A's result.
//!
//! After the closure (§4) and the keepers (§6.1), one parallel pass over
//! the live objects (D1) writes each file's section rows (§10) into its
//! own slice, and merges its mentions into per-key atomics: the minimum
//! entry of each class (`Strong`, `Common`, `Weak`), the common size and
//! alignment maxima, the most constraining visibility and a set of flags
//! (§12.1). `def` is then a function of those cells, per key. The rules
//! that need several mentions of one key at once (duplicates, TLS
//! agreement, the common warnings, `comdat-mismatch`) concern few keys:
//! a second pass (D2) collects only the mentions of the keys flagged in
//! D1, sorted by `(key, ≺)`.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, AtomicU16, AtomicU64, Ordering::Relaxed};

use mortar_core::par::{Pool, split_offsets};
use mortar_core::resolution::{
    CommonShape, DropReason, FileLive, SectionKind, SectionRow, SectionState,
};
use mortar_core::{ComdatId, FileId, IdRange, OptId, SectionId, SymEntry, SymbolId};
use mortar_elf::consts::{STB_WEAK, STT_TLS};
use mortar_elf::{SectionIndex, SymbolSection};

use crate::comdat::{Keepers, SectionMap, keepers};
use crate::linker::{self, Provided};
use crate::live::{Closure, Targets, closure, kinds, targets_with};
use crate::mention::{
    Class, align_log2, atomics_u8, atomics_u16, atomics_u64, classify, max_u8, max_u64, min_u8,
    min_u64, or_u16, rank_vis, vis_rank,
};
use crate::options::{Entry, Options};
use crate::problem::{ComdatMismatch, DefPlace, DiagKey, Problem, What};
use crate::sections::{self, Note, SectionRefusal};
use crate::steps::Steps;
use crate::view::{FileKind, Mention, View, entry_word, word_entry};

/// The definition of a key (§3.2), stage A's form of `def`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Def {
    /// No live candidate; `weak` when every live reference is weak.
    Undefined { weak: bool },
    /// A defined `STB_GLOBAL` or `STB_GNU_UNIQUE` mention (rank 1).
    Strong(SymEntry),
    /// The least common mention by `≺` (rank 2); the shape is `common`'s.
    Common(SymEntry),
    /// A defined `STB_WEAK` mention (rank 3).
    Weak(SymEntry),
    /// A linker-defined symbol (rank 5, §8); `conditional` for the rows
    /// the first layout stage confirms or withdraws (§5.1).
    Provide { sym: Provided, conditional: bool },
    /// A definition in a shared object (rank 4, M2): the entry of
    /// `bind(k)` (`spec/dyn.md` D-11).
    Shared(SymEntry),
}

impl Def {
    /// The defining entry, if a relocatable object defines the key.
    #[must_use]
    pub fn entry(self) -> Option<SymEntry> {
        match self {
            Self::Strong(e) | Self::Common(e) | Self::Weak(e) => Some(e),
            Self::Undefined { .. } | Self::Provide { .. } | Self::Shared(_) => None,
        }
    }

    #[must_use]
    pub fn is_undefined(self) -> bool {
        matches!(self, Self::Undefined { .. })
    }
}

/// The merged properties of a key (§3.5, §3.6), and flags of D1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Props {
    /// The most constraining visibility (`STV_HIDDEN` for internal).
    pub visibility: u8,
    /// [`flags`] bits.
    pub flags: u16,
}

/// The bits of [`Props::flags`].
pub mod flags {
    /// `ref_regular` (§3.6): an undefined or demoted mention in a live
    /// object, or a root.
    pub const REF_REGULAR: u16 = 1 << 0;
    /// `ref_strong`: one of those is not weak.
    pub const REF_STRONG: u16 = 1 << 1;
    /// A live mention is `STT_TLS`.
    pub const TLS: u16 = 1 << 2;
    /// A live mention is not `STT_TLS`.
    pub const NON_TLS: u16 = 1 << 3;
    pub(crate) const HAS_STRONG: u16 = 1 << 4;
    pub(crate) const MULTI_STRONG: u16 = 1 << 5;
    /// A live common mention.
    pub const HAS_COMMON: u16 = 1 << 6;
    pub(crate) const MULTI_COMMON: u16 = 1 << 7;
    /// A live mention is demoted (§6.2).
    pub const DEMOTED: u16 = 1 << 8;
    /// A live common mention is `SHN_COMMON` (not x86-64's
    /// `SHN_X86_64_LCOMMON`): the common is small (§3.3).
    pub const SMALL_COMMON: u16 = 1 << 9;
    /// A live reference (an undefined or demoted mention) has bit 7 of
    /// `st_other` set, AArch64's `STO_AARCH64_VARIANT_PCS` (§3.7: an
    /// undefined key's `.symtab` entry carries it).
    pub const VARIANT_PCS: u16 = 1 << 10;
}

/// The entry address (§5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryPoint {
    /// The key's value.
    Symbol(SymbolId),
    /// `-e` named a number no file mentions.
    Address(u64),
    /// No entry symbol, or it is undefined (`entry-not-found`): layout's
    /// default.
    Default,
}

/// Stage A's result: `Resolution`'s columns and what the scan and
/// `--gc-sections` need from resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// `file_live` (§4.1).
    pub file_live: Vec<FileLive>,
    /// The closure's rounds after round 0.
    pub rounds: u32,
    /// `first`/`cfirst` (§4.1).
    pub targets: Targets,
    /// `def` (§3.2).
    pub def: Vec<Def>,
    pub props: Vec<Props>,
    /// `common` (§3.3): the keys whose `def` is `Common`, by id.
    pub common: Vec<(SymbolId, CommonShape)>,
    /// `clash` (§3.8).
    pub clash: Vec<bool>,
    /// `redirect` (§7.1): for undefined mentions.
    pub redirect: Vec<SymbolId>,
    /// `comdat` (§6.1) and the live copies (§6.7).
    pub keepers: Keepers,
    /// The keys only dropped copies define (§6.3 (b)), by key.
    pub comdat_mismatch: Vec<ComdatMismatch>,
    /// `section_base` (§10): per file; empty for files that are not live
    /// objects.
    pub section_base: Vec<IdRange<SectionId>>,
    /// `sections` (§10).
    pub sections: Vec<SectionRow>,
    /// `SHF_GNU_RETAIN` sections: roots of `--gc-sections` (§4.7).
    pub retained: Vec<(FileId, SectionIndex)>,
    /// The Mason containers (`spec/meta.md` MT-6), sorted.
    pub meta: Vec<(FileId, SectionIndex)>,
    /// `.gnu.warning.<sym>` sections of live files, by the key `<sym>`
    /// (§10: the scan warns per reference).
    pub gnu_warning_syms: Vec<(SymbolId, FileId, SectionIndex)>,
    pub entry: EntryPoint,
    /// Every diagnostic, sorted.
    pub problems: Vec<Problem>,
    /// The simulation of GNU ld (§4.6), under `--warn-backrefs` only.
    pub gnu: Option<crate::gnu::Simulation>,
    /// The shared stage (`spec/dyn.md` D-11-D-21); `None` when no shared
    /// object is loaded (every static link).
    pub dynamic: Option<crate::shared::Dynamic>,
}

/// The per-key cells of D1.
struct Acc {
    strong: Vec<AtomicU64>,
    common: Vec<AtomicU64>,
    weak: Vec<AtomicU64>,
    csize: Vec<AtomicU64>,
    calign: Vec<AtomicU8>,
    vis: Vec<AtomicU8>,
    flags: Vec<AtomicU16>,
}

impl Acc {
    fn new(keys: usize) -> Self {
        Self {
            strong: atomics_u64(keys, u64::MAX),
            common: atomics_u64(keys, u64::MAX),
            weak: atomics_u64(keys, u64::MAX),
            csize: atomics_u64(keys, 0),
            calign: atomics_u8(keys, 0),
            vis: atomics_u8(keys, 2),
            flags: atomics_u16(keys),
        }
    }
}

/// What D1 and D2 read, shared by every worker.
struct Ctx<'a, V> {
    view: &'a V,
    keeper: &'a [OptId<FileId>],
    redirect: &'a [SymbolId],
    lto_slim: Option<SymbolId>,
    opts: &'a Options,
}

/// The effect of one mention of a live object, after demotion (§6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Effect {
    /// A reference to the key (redirected for an undefined mention).
    Ref {
        key: SymbolId,
        weak: bool,
    },
    Strong,
    Common,
    Weak,
}

fn effect(m: &Mention, class: Class, dropped: bool, redirect: &[SymbolId]) -> Effect {
    match class {
        Class::Undefined { weak } => Effect::Ref { key: redirect[m.key.0 as usize], weak },
        Class::Strong | Class::Weak if dropped => {
            Effect::Ref { key: m.key, weak: m.binding == STB_WEAK }
        }
        Class::Strong => Effect::Strong,
        Class::Weak => Effect::Weak,
        Class::Common => Effect::Common,
    }
}

/// Stage A: resolution over `view` with `opts` (§1.1).
pub fn resolve<V: View>(view: &V, opts: &Options, pool: &Pool) -> Resolved {
    resolve_steps(view, opts, pool, &mut Steps::default())
}

/// [`resolve`], recording where the time went.
#[allow(clippy::too_many_lines, reason = "the steps of the crate docs, in order")]
pub fn resolve_steps<V: View>(
    view: &V,
    opts: &Options,
    pool: &Pool,
    steps: &mut Steps,
) -> Resolved {
    steps.restart();
    let keys = view.keys() as usize;
    let kinds = kinds(view);
    let redirect = opts.redirect(view.keys());
    let roots = opts.roots();
    let mut problems = Vec::new();
    steps.step("resolve.kinds");
    let targets = targets_with(view, &kinds, pool);
    steps.step("resolve.targets");
    let Closure { live: mut file_live, rounds } =
        closure(view, &kinds, &targets, &redirect, &roots, pool);
    steps.step("resolve.closure");
    let live_objects: Vec<FileId> = (0..kinds.len())
        .filter(|&f| file_live[f].is_live() && kinds[f].is_object())
        .map(|f| FileId(f as u32))
        .collect();
    let keepers = keepers(view, &live_objects, pool);
    steps.step("resolve.keepers");

    // section_base (§10): an exclusive prefix sum over the live objects.
    let mut section_base = Vec::with_capacity(kinds.len());
    let mut offsets = Vec::with_capacity(live_objects.len());
    let mut next = 0u32;
    for f in 0..kinds.len() {
        let start = SectionId(next);
        if file_live[f].is_live() && kinds[f].is_object() {
            offsets.push(next as usize);
            next = next.saturating_add(view.file(FileId(f as u32)).sections);
        }
        section_base.push(IdRange::new(start, SectionId(next)));
    }
    offsets.push(next as usize);
    let blank = SectionRow {
        kind: SectionKind::Null,
        state: SectionState::Dropped(DropReason::Consumed),
        reloc: SectionIndex(0),
        comdat: OptId::NONE,
    };
    let mut sections = vec![blank; next as usize];
    steps.step("resolve.section_base");

    let ctx = Ctx {
        view,
        keeper: &keepers.keeper,
        redirect: &redirect,
        lto_slim: view.find_key(b"__gnu_lto_slim"),
        opts,
    };
    let acc = Acc::new(keys);
    for root in &roots {
        acc.flags[root.key.0 as usize].fetch_or(flags::REF_REGULAR | flags::REF_STRONG, Relaxed);
    }
    let found = Mutex::new(Found::default());
    {
        let parts = split_offsets(&mut sections, &offsets);
        pool.unit_cost(6300).for_each_part_init(
            live_objects.iter().copied().zip(parts),
            Scratch::default,
            |scratch, _, (file, rows)| {
                let mut out = Found::default();
                d1_file(&ctx, &acc, file, rows, scratch, &mut out);
                if !out.is_empty() {
                    found.lock().unwrap_or_else(std::sync::PoisonError::into_inner).append(out);
                }
            },
        );
    }
    let mut found = found.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner);
    problems.append(&mut found.problems);
    found.retained.sort_unstable();
    found.meta.sort_unstable();
    found.gnu_warning_syms.sort_unstable();
    steps.step("resolve.d1");

    // def (§3.2) and the merged properties (§3.5, §3.6).
    let mut def = vec![Def::Undefined { weak: false }; keys];
    pool.unit_cost(2).for_each_mut(&mut def, |k, d| *d = def_of(&acc, k));
    let mut props: Vec<Props> = pool.unit_cost(1).map(keys, |k| Props {
        visibility: rank_vis(acc.vis[k].load(Relaxed)),
        flags: acc.flags[k].load(Relaxed),
    });
    let mut common: Vec<(SymbolId, CommonShape)> = def
        .iter()
        .enumerate()
        .filter(|(_, d)| matches!(d, Def::Common(_)))
        .map(|(k, _)| {
            let shape = CommonShape {
                size: acc.csize[k].load(Relaxed),
                align: acc.calign[k].load(Relaxed),
                // §3.3: large only when every common mention is LCOMMON.
                large: acc.flags[k].load(Relaxed) & flags::SMALL_COMMON == 0,
            };
            (SymbolId(k as u32), shape)
        })
        .collect();
    steps.step("resolve.def");

    // The linker's own symbols (§8) for referenced keys nothing defines,
    // and the forced names an object defines (§8.2).
    let undefined_refs: Vec<u32> = (0..keys)
        .filter(|&k| def[k].is_undefined() && props[k].flags & flags::REF_REGULAR != 0)
        .map(|k| k as u32)
        .collect();
    for k in undefined_refs {
        let (name, version) = view.key_name(SymbolId(k));
        if version.is_some() {
            continue;
        }
        if let Some((sym, conditional)) = linker::candidate(name, view.machine(), opts.output) {
            def[k as usize] = Def::Provide { sym, conditional };
        }
    }
    for name in linker::forced(view.machine()) {
        if let Some(key) = view.find_key(name)
            && let Some(entry) = def[key.0 as usize].entry()
        {
            problems.push(Problem::at(entry, What::LinkerSymbolRedefined { key, entry }));
        }
    }

    steps.step("resolve.linker");
    // The rules over several mentions of a key (D2).
    let mut clash = vec![false; keys];
    let mut comdat_mismatch = Vec::new();
    let interest: Vec<u8> =
        pool.unit_cost(2).map(keys, |k| interest_of(props[k].flags, def[k], opts));
    if interest.iter().any(|&i| i != 0) {
        let rare = d2(&ctx, &live_objects, &interest, pool);
        several(
            &ctx,
            &rare,
            &def,
            &interest,
            &keepers,
            &mut clash,
            &mut comdat_mismatch,
            &mut problems,
        );
    }

    steps.step("resolve.d2");
    let dynamic = kinds.contains(&FileKind::Shared).then(|| {
        crate::shared::link(
            view,
            opts,
            &kinds,
            &targets,
            &redirect,
            &mut file_live,
            &mut def,
            &mut props,
            &mut problems,
            pool,
        )
    });
    // A common that yields to a shared data definition (§3.2 rule 2) is
    // allocated nowhere.
    common.retain(|(k, _)| matches!(def[k.0 as usize], Def::Common(_)));
    steps.step("resolve.shared");
    // Roots that must be defined (§4.2, §5.5).
    for named in &opts.require_defined {
        if named.key.get().is_none_or(|k| def[k.0 as usize].is_undefined()) {
            problems.push(Problem {
                key: DiagKey::of(FileId::INTERNAL, u64::from(named.arg)),
                what: What::RequiredSymbol {
                    key: named.key,
                    name: named.name.clone(),
                    arg: named.arg,
                },
            });
        }
    }
    let entry = entry_point(&opts.entry, &def, view, &mut problems);

    let gnu = opts.warn_backrefs.then(|| {
        crate::gnu::simulate(view, &kinds, &redirect, &roots, &file_live, &def, &keepers, pool)
    });
    if let Some(sim) = &gnu {
        problems.extend(sim.problems.iter().cloned());
    }
    problems.sort();
    problems.dedup();
    steps.step("resolve.finish");
    Resolved {
        file_live,
        rounds,
        targets,
        def,
        props,
        common,
        clash,
        redirect,
        keepers,
        comdat_mismatch,
        section_base,
        sections,
        retained: found.retained,
        meta: found.meta,
        gnu_warning_syms: found.gnu_warning_syms,
        entry,
        problems,
        gnu,
        dynamic,
    }
}

fn def_of(acc: &Acc, k: usize) -> Def {
    let s = acc.strong[k].load(Relaxed);
    if s != u64::MAX {
        return Def::Strong(word_entry(s));
    }
    let c = acc.common[k].load(Relaxed);
    if c != u64::MAX {
        return Def::Common(word_entry(c));
    }
    let w = acc.weak[k].load(Relaxed);
    if w != u64::MAX {
        return Def::Weak(word_entry(w));
    }
    Def::Undefined { weak: acc.flags[k].load(Relaxed) & flags::REF_STRONG == 0 }
}

/// spec: meta.md MT-6: a section named `.mason.meta` that is not a
/// container keeps the rows it matched and is reported once per file.
/// Whether it reported.
fn misnamed_container(
    file: FileId,
    i: u32,
    name: &[u8],
    class: &Result<(SectionKind, Note), SectionRefusal>,
    out: &mut Found,
) -> bool {
    if name != mortar_meta::SECTION_NAME || matches!(class, Ok((SectionKind::Meta, _))) {
        return false;
    }
    out.problems.push(Problem {
        key: DiagKey::of(file, u64::from(i)),
        what: What::MetaNotContainer { file, section: SectionIndex(i), relocated: false },
    });
    true
}

/// What D1 gathers besides the per-key cells.
#[derive(Debug, Default)]
struct Found {
    problems: Vec<Problem>,
    retained: Vec<(FileId, SectionIndex)>,
    meta: Vec<(FileId, SectionIndex)>,
    gnu_warning_syms: Vec<(SymbolId, FileId, SectionIndex)>,
}

impl Found {
    fn is_empty(&self) -> bool {
        self.problems.is_empty()
            && self.retained.is_empty()
            && self.meta.is_empty()
            && self.gnu_warning_syms.is_empty()
    }

    fn append(&mut self, mut other: Self) {
        self.problems.append(&mut other.problems);
        self.retained.append(&mut other.retained);
        self.meta.append(&mut other.meta);
        self.gnu_warning_syms.append(&mut other.gnu_warning_syms);
    }
}

/// A worker's buffers.
#[derive(Default)]
struct Scratch {
    mentions: Vec<Mention>,
    map: SectionMap,
}

/// D1 over one live object: its section rows, then its mentions.
fn d1_file<V: View>(
    ctx: &Ctx<'_, V>,
    acc: &Acc,
    file: FileId,
    rows: &mut [SectionRow],
    scratch: &mut Scratch,
    out: &mut Found,
) {
    let view = ctx.view;
    let info = view.file(file);
    scratch.map.fill(view, file, info.sections, ctx.keeper, true, &mut out.problems);
    section_rows(ctx, file, rows, &scratch.map, out);
    scratch.mentions.clear();
    view.mentions(file, &mut scratch.mentions);
    let mut lto = false;
    for m in &scratch.mentions {
        let entry = SymEntry { file, index: m.index };
        let (class, fault) = classify(m, entry);
        if let Some(what) = fault {
            out.problems.push(Problem::at(entry, what));
        }
        if Some(m.key) == ctx.lto_slim {
            lto = true;
        }
        let dropped = match m.place {
            SymbolSection::Section(s) => scratch.map.is_dropped(s.0),
            _ => false,
        };
        let word = entry_word(file, m.index);
        let tls = if m.sym_type == STT_TLS && m.place != SymbolSection::Common {
            flags::TLS
        } else {
            flags::NON_TLS
        };
        let rank = vis_rank(m.visibility);
        match effect(m, class, dropped, ctx.redirect) {
            Effect::Ref { key, weak } => {
                let k = key.0 as usize;
                let mut bits = tls | flags::REF_REGULAR;
                if !weak {
                    bits |= flags::REF_STRONG;
                }
                if dropped {
                    bits |= flags::DEMOTED;
                }
                if m.other & 0x80 != 0 {
                    bits |= flags::VARIANT_PCS;
                }
                or_u16(&acc.flags[k], bits);
                min_u8(&acc.vis[k], rank);
            }
            effect => {
                for key in [Some(m.key), m.also.get()].into_iter().flatten() {
                    let k = key.0 as usize;
                    min_u8(&acc.vis[k], rank);
                    match effect {
                        Effect::Strong => {
                            min_u64(&acc.strong[k], word);
                            let old = or_u16(&acc.flags[k], tls | flags::HAS_STRONG);
                            if old & flags::HAS_STRONG != 0 {
                                or_u16(&acc.flags[k], flags::MULTI_STRONG);
                            }
                        }
                        Effect::Weak => {
                            min_u64(&acc.weak[k], word);
                            or_u16(&acc.flags[k], tls);
                        }
                        Effect::Common => {
                            min_u64(&acc.common[k], word);
                            max_u64(&acc.csize[k], m.size);
                            max_u8(&acc.calign[k], align_log2(m.value));
                            let small = if m.large { 0 } else { flags::SMALL_COMMON };
                            let old = or_u16(&acc.flags[k], tls | small | flags::HAS_COMMON);
                            if old & flags::HAS_COMMON != 0 {
                                or_u16(&acc.flags[k], flags::MULTI_COMMON);
                            }
                        }
                        Effect::Ref { .. } => {}
                    }
                }
            }
        }
    }
    if lto {
        out.problems.push(Problem { key: DiagKey::of(file, 0), what: What::LtoSlim { file } });
    }
}

/// spec: meta.md MT-2a, MT-6 (WP-099): a container a relocation section
/// applies to is not one. Its records are not read (mortar applies no
/// relocation to a container, and a producer that meant one meant
/// something MT-2 forbids); it takes the `SHF_EXCLUDE` row, its
/// relocations are dropped with it, and the file is warned once
/// (`misnamed`: it was already).
fn relocated_containers<V: View>(
    ctx: &Ctx<'_, V>,
    file: FileId,
    n: u32,
    rows: &mut [SectionRow],
    map: &SectionMap,
    mut misnamed: bool,
    out: &mut Found,
) {
    for i in 1..n {
        let row = &mut rows[i as usize];
        if row.kind != SectionKind::Meta || row.reloc.0 == 0 {
            continue;
        }
        let Some(s) = ctx.view.section(file, SectionIndex(i)) else { continue };
        row.kind = SectionKind::Excluded;
        let excluded = sections::excluded_reason(&s, Note::None);
        let strip = ctx.opts.strip_debug && sections::is_debug(&s);
        row.state = sections::state(SectionKind::Excluded, excluded, map.is_dropped(i), strip);
        out.meta.retain(|&m| m != (file, SectionIndex(i)));
        if !misnamed {
            misnamed = true;
            out.problems.push(Problem {
                key: DiagKey::of(file, u64::from(i)),
                what: What::MetaNotContainer { file, section: SectionIndex(i), relocated: true },
            });
        }
    }
}

/// §10 for the sections of one live object.
fn section_rows<V: View>(
    ctx: &Ctx<'_, V>,
    file: FileId,
    rows: &mut [SectionRow],
    map: &SectionMap,
    out: &mut Found,
) {
    let machine = ctx.view.machine();
    let n = rows.len() as u32;
    let mut misnamed = false;
    for i in 0..n {
        let Some(s) = ctx.view.section(file, SectionIndex(i)) else { continue };
        let group = map.group_of(i);
        let dropped = map.is_dropped(i);
        let row = &mut rows[i as usize];
        row.comdat = group;
        let class = sections::classify(machine, i, &s);
        misnamed = misnamed || misnamed_container(file, i, s.name, &class, out);
        match class {
            Ok((kind, note)) => {
                row.kind = kind;
                if kind == SectionKind::Meta {
                    out.meta.push((file, SectionIndex(i)));
                }
                let excluded = sections::excluded_reason(&s, note);
                let strip = ctx.opts.strip_debug && sections::is_debug(&s);
                row.state = sections::state(kind, excluded, dropped, strip);
                match note {
                    Note::Retain if row.is_kept() => out.retained.push((file, SectionIndex(i))),
                    Note::GnuWarning if !dropped => out.problems.push(Problem {
                        key: DiagKey::of(file, u64::from(i)),
                        what: What::GnuWarning { file, section: SectionIndex(i) },
                    }),
                    Note::GnuWarningSym if !dropped => {
                        if let Some(key) = ctx.view.find_key(&s.name[b".gnu.warning.".len()..]) {
                            out.gnu_warning_syms.push((key, file, SectionIndex(i)));
                        }
                    }
                    _ => {}
                }
            }
            Err(why) if dropped => {
                // A refusal of a section's content or fate applies to kept
                // sections only: a dropped copy contributes nothing (§10).
                let _ = why;
                row.kind = SectionKind::Regular;
                row.state = SectionState::Dropped(DropReason::Comdat);
            }
            Err(SectionRefusal::Zdebug) if ctx.opts.strip_debug => {
                row.kind = SectionKind::Debug;
                row.state = SectionState::Dropped(DropReason::Stripped);
            }
            Err(SectionRefusal::Compressed) if ctx.opts.strip_debug && sections::is_debug(&s) => {
                row.kind = SectionKind::Debug;
                row.state = SectionState::Dropped(DropReason::Stripped);
            }
            Err(why) => {
                // Refused; the row stays `Null`/consumed, the link fails.
                out.problems.push(Problem {
                    key: DiagKey::of(file, u64::from(i)),
                    what: What::Section { file, section: SectionIndex(i), why },
                });
            }
        }
    }
    // A relocation section applies to its target (§6.5: one applying to a
    // dropped section is dropped with it, as every `Reloc` row is).
    for i in 1..n {
        if rows[i as usize].kind != SectionKind::Reloc {
            continue;
        }
        let Some(s) = ctx.view.section(file, SectionIndex(i)) else { continue };
        let Some(target) = rows.get_mut(s.info as usize).filter(|_| s.info != 0) else {
            continue;
        };
        if target.reloc.0 == 0 {
            target.reloc = SectionIndex(i);
        } else if target.is_kept() {
            out.problems.push(Problem {
                key: DiagKey::of(file, u64::from(i)),
                what: What::Section {
                    file,
                    section: SectionIndex(i),
                    why: SectionRefusal::SecondReloc(s.info),
                },
            });
        }
    }
    relocated_containers(ctx, file, n, rows, map, misnamed, out);
    // synth.md S-6: a `Merge` section that is not certain to merge safely
    // is placed whole, as `Regular` (§10), so every later phase (layout's
    // `place`, `addr_in`, the symbol stage) treats it as one piece.
    for i in 1..n {
        if rows[i as usize].kind != SectionKind::Merge {
            continue;
        }
        let Some(s) = ctx.view.section(file, SectionIndex(i)) else { continue };
        if sections::placed_whole(&s, rows[i as usize].reloc.0 != 0) {
            rows[i as usize].kind = SectionKind::Regular;
        }
    }
    // A `SHF_LINK_ORDER` section outside the groups whose linked section
    // is dropped is dropped too (§6.5).
    if map.any {
        for i in 1..n {
            let Some(s) = ctx.view.section(file, SectionIndex(i)) else { continue };
            if s.flags & mortar_elf::consts::SHF_LINK_ORDER != 0
                && map.group_of(i).is_none()
                && map.is_dropped(s.link)
                && rows[i as usize].kind.carries_output()
            {
                rows[i as usize].state = SectionState::Dropped(DropReason::Comdat);
                rows[i as usize].comdat = map.group_of(s.link);
            }
        }
    }
}

const I_DUP: u8 = 1;
const I_TLS: u8 = 2;
const I_COMMON: u8 = 4;
const I_WARN_COMMON: u8 = 8;
const I_DEMOTED: u8 = 16;

fn interest_of(f: u16, def: Def, opts: &Options) -> u8 {
    let mut i = 0;
    if f & flags::MULTI_STRONG != 0 {
        i |= I_DUP;
    }
    if f & flags::TLS != 0 && f & flags::NON_TLS != 0 {
        i |= I_TLS;
    }
    let common = f & flags::HAS_COMMON != 0;
    let common_def = matches!(def, Def::Common(_));
    if common && matches!(def, Def::Strong(_) | Def::Weak(_)) {
        i |= I_COMMON;
    }
    if opts.warn_common && common && (!common_def || f & flags::MULTI_COMMON != 0) {
        i |= I_WARN_COMMON;
    }
    if f & flags::DEMOTED != 0 && def.is_undefined() {
        i |= I_DEMOTED;
    }
    i
}

/// A mention of an interesting key, as D2 collects it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Rare {
    key: u32,
    word: u64,
    effect: u8,
    demoted: bool,
    group: OptId<ComdatId>,
    section: Option<u32>,
    value: u64,
    size: u64,
    tls: bool,
}

const E_REF: u8 = 0;
const E_STRONG: u8 = 1;
const E_COMMON: u8 = 2;
const E_WEAK: u8 = 3;

fn d2<V: View>(ctx: &Ctx<'_, V>, live: &[FileId], interest: &[u8], pool: &Pool) -> Vec<Rare> {
    let got = pool.map_collect(live.len(), |u, out: &mut Vec<Rare>| {
        let file = live[u];
        let view = ctx.view;
        let mut mentions = Vec::new();
        view.mentions(file, &mut mentions);
        if !mentions.iter().any(|m| {
            interest[m.key.0 as usize] != 0
                || m.also.get().is_some_and(|k| interest[k.0 as usize] != 0)
                || interest[ctx.redirect[m.key.0 as usize].0 as usize] != 0
        }) {
            return;
        }
        let mut map = SectionMap::default();
        let mut ignore = Vec::new();
        map.fill(view, file, view.file(file).sections, ctx.keeper, false, &mut ignore);
        for m in &mentions {
            let entry = SymEntry { file, index: m.index };
            let (class, _) = classify(m, entry);
            let (section, group, dropped) = match m.place {
                SymbolSection::Section(s) => (Some(s.0), map.group_of(s.0), map.is_dropped(s.0)),
                _ => (None, OptId::NONE, false),
            };
            let tls = m.sym_type == STT_TLS && m.place != SymbolSection::Common;
            let (keys, e) = match effect(m, class, dropped, ctx.redirect) {
                Effect::Ref { key, .. } => ([Some(key), None], E_REF),
                Effect::Strong => ([Some(m.key), m.also.get()], E_STRONG),
                Effect::Common => ([Some(m.key), m.also.get()], E_COMMON),
                Effect::Weak => ([Some(m.key), m.also.get()], E_WEAK),
            };
            for key in keys.into_iter().flatten() {
                if interest[key.0 as usize] != 0 {
                    out.push(Rare {
                        key: key.0,
                        word: entry_word(file, m.index),
                        effect: e,
                        demoted: dropped,
                        group,
                        section,
                        value: m.value,
                        size: m.size,
                        tls,
                    });
                }
            }
        }
    });
    let mut rare: Vec<Rare> = (0..got.units()).flat_map(|u| got.unit(u).iter().copied()).collect();
    pool.sort_by(&mut rare, |a, b| (a.key, a.word).cmp(&(b.key, b.word)));
    rare
}

/// The rules over several mentions of one key, from D2's rows.
#[allow(clippy::too_many_arguments)]
fn several<V: View>(
    ctx: &Ctx<'_, V>,
    rare: &[Rare],
    def: &[Def],
    interest: &[u8],
    keepers: &Keepers,
    clash: &mut [bool],
    mismatch: &mut Vec<ComdatMismatch>,
    problems: &mut Vec<Problem>,
) {
    let opts = ctx.opts;
    for rows in rare.chunk_by(|a, b| a.key == b.key) {
        let k = rows[0].key as usize;
        let key = SymbolId(rows[0].key);
        let i = interest[k];
        let d = def[k];
        if i & I_DUP != 0 {
            let strong: Vec<&Rare> = rows.iter().filter(|r| r.effect == E_STRONG).collect();
            let all_abs_equal = strong.iter().all(|r| r.section.is_none())
                && strong.windows(2).all(|w| w[0].value == w[1].value);
            if strong.len() >= 2 && !all_abs_equal {
                clash[k] = true;
                if !opts.allow_multiple_definition {
                    let defs: Box<[DefPlace]> = strong
                        .iter()
                        .map(|r| DefPlace {
                            entry: word_entry(r.word),
                            section: r.section.map(SectionIndex),
                            value: r.value,
                        })
                        .collect();
                    problems.push(Problem::at(defs[1].entry, What::Duplicate { key, defs }));
                }
            }
        }
        let tls_mismatch = i & I_TLS != 0;
        if tls_mismatch {
            let first = d
                .entry()
                .and_then(|e| rows.iter().find(|r| r.word == entry_word(e.file, e.index)))
                .unwrap_or(&rows[0]);
            if let Some(other) = rows.iter().find(|r| r.tls != first.tls) {
                let other = word_entry(other.word);
                problems.push(Problem::at(
                    other,
                    What::TlsMismatch {
                        key,
                        first: word_entry(first.word),
                        first_tls: first.tls,
                        other,
                    },
                ));
            }
        }
        let commons: Vec<&Rare> = rows.iter().filter(|r| r.effect == E_COMMON).collect();
        if i & I_COMMON != 0 && !tls_mismatch {
            common_warnings(ctx, key, d, rows, &commons, problems);
        }
        if i & I_WARN_COMMON != 0 && !tls_mismatch && !commons.is_empty() {
            if let Some(e) = d.entry().filter(|_| !matches!(d, Def::Common(_))) {
                let mut all: Vec<u64> = commons.iter().map(|r| r.word).collect();
                all.push(entry_word(e.file, e.index));
                all.sort_unstable();
                if let Some(&second) = all.get(1) {
                    problems.push(Problem::at(
                        word_entry(second),
                        What::CommonOverridden { key, common: word_entry(commons[0].word), def: e },
                    ));
                }
            } else if let Some(second) =
                commons.iter().find(|r| (r.word >> 32) != (commons[0].word >> 32))
            {
                let second = word_entry(second.word);
                problems.push(Problem::at(
                    second,
                    What::MultipleCommon { key, first: word_entry(commons[0].word), second },
                ));
            }
        }
        if i & I_DEMOTED != 0
            && let Some(r) = rows.iter().find(|r| r.demoted)
            && let Some(group) = r.group.get()
            && let Some(keeper) = keepers.keeper.get(group.0 as usize).and_then(|k| k.get())
        {
            mismatch.push(ComdatMismatch { key, group, keeper, dropped: word_entry(r.word) });
        }
    }
}

/// `common-larger-than-definition` and `common-alignment` (§3.3).
fn common_warnings<V: View>(
    ctx: &Ctx<'_, V>,
    key: SymbolId,
    d: Def,
    rows: &[Rare],
    commons: &[&Rare],
    problems: &mut Vec<Problem>,
) {
    let Some(e) = d.entry() else { return };
    let Some(def_row) = rows.iter().find(|r| r.word == entry_word(e.file, e.index)) else {
        return;
    };
    let Some(largest) =
        commons.iter().copied().max_by(|a, b| a.size.cmp(&b.size).then(b.word.cmp(&a.word)))
    else {
        return;
    };
    if largest.size > def_row.size {
        problems.push(Problem::at(
            e,
            What::CommonLarger {
                key,
                def: e,
                def_size: def_row.size,
                common: word_entry(largest.word),
                common_size: largest.size,
            },
        ));
    }
    let Some(section) = def_row.section else { return };
    let Some(s) = ctx.view.section(e.file, SectionIndex(section)) else { return };
    let mut align = s.addralign.max(1);
    if def_row.value != 0 {
        align = align.min(1u64 << def_row.value.trailing_zeros().min(63));
    }
    let Some(aligned) = commons
        .iter()
        .copied()
        .filter(|r| r.value.is_power_of_two())
        .max_by(|a, b| a.value.cmp(&b.value).then(b.word.cmp(&a.word)))
    else {
        return;
    };
    if align < aligned.value {
        problems.push(Problem::at(
            e,
            What::CommonAlignment {
                key,
                def: e,
                def_align: align,
                common: word_entry(aligned.word),
                common_align: aligned.value,
            },
        ));
    }
}

/// The entry address and `entry-not-found` (§5.5).
fn entry_point<V: View>(
    entry: &Entry,
    def: &[Def],
    view: &V,
    problems: &mut Vec<Problem>,
) -> EntryPoint {
    let (key, name, arg): (OptId<SymbolId>, &[u8], u32) = match entry {
        Entry::None => return EntryPoint::Default,
        Entry::Default(key) => (*key, b"_start", 0),
        Entry::Named(named) => (named.key, &named.name, named.arg),
    };
    if let Some(k) = key.get()
        && !def[k.0 as usize].is_undefined()
    {
        return EntryPoint::Symbol(k);
    }
    if key.is_none()
        && matches!(entry, Entry::Named(_))
        && let Some(address) = parse_number(name)
    {
        return EntryPoint::Address(address);
    }
    let _ = view;
    problems.push(Problem {
        key: DiagKey::of(FileId::INTERNAL, u64::from(arg)),
        what: What::EntryNotFound { key, name: name.into(), arg },
    });
    EntryPoint::Default
}

/// A number as GNU ld reads an `-e` argument: `0x` hexadecimal, a leading
/// `0` octal, decimal otherwise.
fn parse_number(text: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(text).ok()?;
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok()
    } else if text.len() > 1 && text.starts_with('0') {
        u64::from_str_radix(&text[1..], 8).ok()
    } else {
        text.parse().ok()
    }
}

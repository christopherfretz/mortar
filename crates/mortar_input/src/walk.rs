//! The walk of `spec/input.md` §10.1 over the files of §4-§9, and the
//! waves that feed it (§12.1).
//!
//! The walk is sequential and pure: it reads only the caches of stats,
//! reads and per-file analyses. Where it needs a fact the caches do not
//! hold, it records the request and goes on with the rest of the line (a
//! speculative pass); a wave then makes every request of the pass in
//! parallel (stats, reads in 4 MiB chunks, analyses per file and per
//! archive member) and the walk runs again. The pass that needs nothing is
//! the result, so numbering, probes and diagnostics are those of the
//! sequential definition whatever the schedule, and every file the line
//! reaches is read in parallel with the others of its wave.
#![allow(
    clippy::many_single_char_names,
    reason = "m: the mention, f: facts, p: a problem, k: a file ordinal"
)]

use std::collections::{BTreeSet, HashMap, HashSet};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use mortar_core::hash::HashMap as FixedMap;
use mortar_core::id::FileId;
use mortar_core::inputs::{BindsLocally, ContentHash, LinkTarget, LoadSpan, LocalBinding, Reader};
use mortar_core::par::Pool;
use mortar_core::plan::{InputFormat, InputSpec, OutputKind, Plan, SymbolRole};
use mortar_diag::{FileRef, Kind as Dk, Place, RefusalClass, Severity};
use mortar_elf::Machine;
use mortar_elf::consts::{EM_386, EM_AARCH64, EM_X86_64, ET_REL};
use mortar_script::{Command, CommandKind, FileForm, InputItem, Script};

use crate::facts::{
    self, ArchiveFacts, Bad, ElfIdent, Facts, Kind, MemberContent, MemberFacts, ObjectFacts,
    ObjectKeys, SharedFacts, VersionSlots,
};
use crate::problem::{Problem, seq};
use crate::search::{
    self, InSysroot, SearchForm, Spelled, arch_name, bfd_name, dir_of, file_name, fs_path,
    in_sysroot, join, library_names, search_label, split_colon, sysroot_prefix, text,
};
use crate::store::{Blob, ReadError, ReadJob, Stat, Store};

/// More than this many implicit scripts on one chain is refused (§5.3).
pub const MAX_SCRIPT_DEPTH: usize = 16;
/// `INCLUDE` nesting (script.md §2.8).
pub const MAX_INCLUDE_DEPTH: usize = 10;

/// What the phase reads besides the `Plan` (§4.7).
#[derive(Clone, Debug)]
pub struct WalkEnv {
    pub cwd: PathBuf,
}

/// Switches of the stage-A walk.
#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    /// Load the `DT_NEEDED` closure (§8.6) where §8.6 says it is loaded.
    /// M2: implemented, off by default (the driver does not ask for it).
    pub closure: bool,
}

/// How the target was decided (§3.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetChoice {
    pub target: LinkTarget,
    /// 1: `-m`; 2: the first ELF `Path` input; 3: the first ELF file in
    /// load order.
    pub rule: u8,
    /// What set it, for the note of `incompatible-file`: `-m elf_x86_64`,
    /// or a found path.
    pub by: String,
}

/// What a row is (§10.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Object,
    SharedObject,
}

/// A row's archive (§10.3 `archive`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberRef {
    pub archive: u32,
    pub header_offset: u64,
    /// The member name (for a thin archive, the stored path).
    pub name: Spelled,
    /// Ordinal among the archive's members of that name (§9.4).
    pub ordinal: u32,
    /// A thin member's own found path.
    pub thin_path: Option<Spelled>,
}

/// Where a row came from (§10.3 `origin`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Origin {
    /// The `Plan` input (index into `plan.inputs`).
    pub input: u32,
    /// Its argument index: the key's unit.
    pub arg: u32,
    /// The script entry (index into `script_entries`).
    pub entry: Option<u32>,
    /// For a closure file, the edge (index into `closure`).
    pub edge: Option<u32>,
}

/// The mention's own state (§10.3 `state`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools, reason = "one field per switch of the mention")]
pub struct RowState {
    pub as_needed: bool,
    pub whole_archive: bool,
    pub lazy: bool,
    pub link_static: bool,
    pub copy_dt_needed: bool,
    pub exclude_libs: bool,
}

/// §9.4: found path and mention ordinal; for a member, also its name and
/// ordinal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub path: Spelled,
    pub mention: u32,
    pub member: Option<(Spelled, u32)>,
}

/// A row of `files` (§10.3), `FileId(i + 1)` for `files[i]`.
#[derive(Clone, Debug)]
pub struct FileRow<'s> {
    pub kind: RowKind,
    pub name: FileRef,
    pub path: Spelled,
    pub member: Option<MemberRef>,
    pub origin: Origin,
    pub state: RowState,
    pub blob: Blob,
    pub base: u64,
    pub len: u64,
    pub sections: u32,
    pub globals: u32,
    pub groups: u32,
    pub identity: Identity,
    /// The file's hash (§11.6); `ContentHash(0)`, "not computed", when
    /// `Store::hashes()` is false (the staged store's default). Never
    /// trust a 0: see `mortar_core::inputs::File::hash`.
    pub hash: ContentHash,
    /// A shared object's soname (§8.2) and whether it is `DT_SONAME`.
    pub soname: Option<(Spelled, bool)>,
    pub reader: Reader<'s>,
    /// A shared object's version slots (for its keys, §6).
    pub versions: VersionSlots<'s>,
    /// An object's names and signatures, as its facts read them (§6).
    pub keys: ObjectKeys<'s>,
    /// The `opened` row of the file: its own, or for an archive member its
    /// archive's (a thin member: its own file's).
    pub opened: u32,
}

/// A row of `archives` (§10.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveRow {
    pub path: Spelled,
    pub thin: bool,
    pub input: u32,
    /// Its `opened` row.
    pub opened: u32,
    /// The mention's origin.
    pub origin: Origin,
}

/// What an opened file was (§10.4 `opened`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenedKind {
    Object,
    Archive,
    /// `loaded`: it has a row (not a later file of a loaded soname).
    Shared {
        loaded: bool,
    },
    Script,
    Include,
    Empty,
    ThinMember,
    /// A nested thin archive, read for one member (§7.2).
    NestedArchive,
    /// Refused, malformed or unknown.
    Other,
}

/// A row of `opened`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened {
    pub path: Spelled,
    pub kind: OpenedKind,
    pub origin: Origin,
}

/// What a probe was made for (§4.6 `search`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchRef {
    /// The `Plan` input of that index (`-l`, `-l:`).
    Input(u32),
    /// The script entry of that index (an `INCLUDE` is one too).
    ScriptEntry(u32),
    Closure(u32),
}

/// A probe's outcome (§4.6). `Found` names the `opened` row the file
/// became, not a `FileId`: a found script or archive has none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeOutcome {
    Absent,
    Skipped(SkipWhy),
    Found(u32),
    Error(Option<i32>),
}

/// The target of a skipped candidate (§4.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkipWhy {
    /// An ELF file, or an archive's first relocatable member.
    Elf(ElfIdent),
    /// A script whose `OUTPUT_FORMAT` names another target.
    Format(Spelled),
}

impl std::fmt::Display for SkipWhy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Elf(ident) => f.write_str(&ident.target_name()),
            Self::Format(name) => f.write_str(&text(name)),
        }
    }
}

/// A row of `probes`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    pub path: Spelled,
    pub search: SearchRef,
    pub outcome: ProbeOutcome,
}

/// A row of `script_entries` (§10.3 `origin`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScriptEntry {
    /// The `opened` row of the file the entry is written in.
    pub script: u32,
    pub offset: u64,
    pub parent: Option<u32>,
}

/// `OUTPUT`, `ENTRY`, `EXTERN` (§5.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Directive {
    pub command: &'static str,
    pub value: Spelled,
    pub script: u32,
    pub offset: u64,
}

/// One loaded soname (§10.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SonameUnit {
    /// `FileId` index (`files[file - 1]`).
    pub file: u32,
    /// The `opened` rows of every mention, in load order.
    pub mentions: Vec<u32>,
    pub as_needed: bool,
    pub copy_dt_needed: bool,
}

/// A `DT_NEEDED` edge (§8.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClosureEdge {
    pub needing: u32,
    pub index: u32,
    pub name: Spelled,
    /// The `FileId` that satisfies it, or `None`: not found.
    pub satisfied: Option<u32>,
}

/// The per-link table (§10.4).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LinkTable {
    /// Group ordinal per row (§10.2).
    pub group: Vec<Option<u32>>,
    pub sonames: Vec<SonameUnit>,
    pub closure: Vec<ClosureEdge>,
    pub opened: Vec<Opened>,
    pub probes: Vec<Probe>,
    pub script_entries: Vec<ScriptEntry>,
    pub directives: Vec<Directive>,
    pub rpath_from_r: Vec<(u32, Spelled)>,
    /// `spec/dyn.md` D-43a's side table: a row per loaded shared object
    /// that binds a definition locally, in `FileId` order.
    pub local_binding: Vec<LocalBinding>,
}

/// The result of the walk.
#[derive(Clone, Debug)]
pub struct Loaded<'s> {
    pub target: Option<TargetChoice>,
    pub files: Vec<FileRow<'s>>,
    pub archives: Vec<ArchiveRow>,
    pub link: LinkTable,
    /// In key order (§12.3).
    pub problems: Vec<Problem>,
    /// The number of waves the walk took.
    pub waves: u32,
    /// Where the time went (for the bench; not part of the result).
    pub timing: Timing,
}

/// Wall time per step of [`walk`], summed over the waves.
#[derive(Clone, Copy, Debug, Default)]
pub struct Timing {
    /// The sequential passes of the walk.
    pub walk: Duration,
    /// `stat` of every path asked for.
    pub stat: Duration,
    /// `canonicalize` (§5.3 cycle detection, §9).
    pub canon: Duration,
    /// [`Store::read_many`].
    pub read: Duration,
    /// Sniffing and parsing files and archive members.
    pub facts: Duration,
    /// The content hashes of archive members.
    pub hash: Duration,
}

impl Loaded<'_> {
    /// Whether an error was found: then no ids are given out (§10.1).
    #[must_use]
    pub fn failed(&self) -> bool {
        self.problems.iter().any(Problem::is_error)
    }
}

// ---------------------------------------------------------------- caches

#[derive(Default)]
struct Caches<'s> {
    stats: HashMap<PathBuf, Stat>,
    reads: HashMap<PathBuf, Result<Blob, ReadError>>,
    facts: HashMap<(Blob, bool), Facts<'s>>,
    canon: HashMap<PathBuf, Option<PathBuf>>,
}

#[derive(Default)]
struct Needs {
    stat: BTreeSet<PathBuf>,
    read: BTreeSet<(PathBuf, bool)>,
    facts: BTreeSet<(Blob, bool)>,
    canon: BTreeSet<PathBuf>,
}

impl Needs {
    fn is_empty(&self) -> bool {
        self.stat.is_empty()
            && self.read.is_empty()
            && self.facts.is_empty()
            && self.canon.is_empty()
    }
}

/// A lookup in the caches: `None` is "not known yet" (requested).
enum Got<'c, 's> {
    Pending,
    Failed(ReadError),
    File(Blob, &'c Facts<'s>),
}

#[derive(Clone, Copy)]
struct Look<'c, 's> {
    caches: &'c Caches<'s>,
    cwd: &'c Path,
}

impl<'c, 's> Look<'c, 's> {
    fn stat(self, needs: &mut Needs, spelled: &[u8]) -> Option<&'c Stat> {
        let p = fs_path(self.cwd, spelled);
        let s = self.caches.stats.get(&p);
        if s.is_none() {
            needs.stat.insert(p);
        }
        s
    }

    fn open(self, needs: &mut Needs, spelled: &[u8], member: bool) -> Got<'c, 's> {
        let p = fs_path(self.cwd, spelled);
        let Some(stat) = self.caches.stats.get(&p) else {
            needs.stat.insert(p.clone());
            needs.read.insert((p, member));
            return Got::Pending;
        };
        if matches!(stat, Stat::Absent) {
            return Got::Failed(ReadError::Missing);
        }
        match self.caches.reads.get(&p) {
            None => {
                needs.read.insert((p, member));
                Got::Pending
            }
            Some(Err(e)) => Got::Failed(e.clone()),
            Some(Ok(b)) => {
                if let Some(f) = self.caches.facts.get(&(*b, member)) {
                    Got::File(*b, f)
                } else {
                    needs.facts.insert((*b, member));
                    Got::Pending
                }
            }
        }
    }

    #[allow(clippy::option_option, reason = "outer None: not known yet")]
    fn canon(self, needs: &mut Needs, spelled: &[u8]) -> Option<Option<&'c Path>> {
        let p = fs_path(self.cwd, spelled);
        if let Some(c) = self.caches.canon.get(&p) {
            Some(c.as_deref())
        } else {
            needs.canon.insert(p);
            None
        }
    }
}

impl<'s> Caches<'s> {
    fn fill(&mut self, store: &'s Store, pool: &Pool, needs: Needs, timing: &mut Timing) {
        let t = Instant::now();
        let mut stat_paths: Vec<PathBuf> =
            needs.stat.into_iter().filter(|p| !self.stats.contains_key(p)).collect();
        stat_paths.sort();
        stat_paths.dedup();
        let stats = pool.unit_cost(6200).map(stat_paths.len(), |i| store.stat(&stat_paths[i]));
        self.stats.extend(stat_paths.into_iter().zip(stats));
        timing.stat += t.elapsed();
        let t = Instant::now();

        let canon_paths: Vec<PathBuf> =
            needs.canon.into_iter().filter(|p| !self.canon.contains_key(p)).collect();
        let canon = pool.map(canon_paths.len(), |i| std::fs::canonicalize(&canon_paths[i]).ok());
        self.canon.extend(canon_paths.into_iter().zip(canon));
        timing.canon += t.elapsed();
        let t = Instant::now();

        // Reads, one job per distinct inode (§9.3).
        let mut wanted_facts: BTreeSet<(Blob, bool)> = needs.facts;
        let mut jobs: Vec<ReadJob> = Vec::new();
        let mut job_of: Vec<(PathBuf, usize)> = Vec::new();
        let mut queued: HashSet<PathBuf> = HashSet::new();
        let mut by_inode: HashMap<(u64, u64), usize> = HashMap::new();
        let mut modes: Vec<(PathBuf, bool)> = Vec::new();
        for (p, member) in needs.read {
            modes.push((p.clone(), member));
            if self.reads.contains_key(&p) || !queued.insert(p.clone()) {
                continue;
            }
            let stat = self.stats.get(&p).cloned().unwrap_or(Stat::Absent);
            let inode = match &stat {
                Stat::Regular(m) | Stat::CharDevice(m) => Some((m.dev, m.ino)),
                _ => None,
            };
            let j = if let Some(&j) = inode.and_then(|i| by_inode.get(&i)) {
                j
            } else {
                jobs.push(ReadJob { path: p.clone(), stat });
                if let Some(i) = inode {
                    by_inode.insert(i, jobs.len() - 1);
                }
                jobs.len() - 1
            };
            job_of.push((p, j));
        }
        let results = store.read_many(pool, &jobs);
        timing.read += t.elapsed();
        let t = Instant::now();
        for (p, j) in job_of {
            self.reads.insert(p, results[j].clone());
        }
        for (p, member) in modes {
            if let Some(Ok(b)) = self.reads.get(&p) {
                wanted_facts.insert((*b, member));
            }
        }
        let wanted: Vec<(Blob, bool)> =
            wanted_facts.into_iter().filter(|k| !self.facts.contains_key(k)).collect();
        let mut new: Vec<Facts<'s>> =
            pool.map(wanted.len(), |i| facts::analyse_view(store.view(wanted[i].0), wanted[i].1));
        // The members of the archives just analysed, in parallel (§12.1).
        let mut slots: Vec<(usize, usize, Blob, u64, u64)> = Vec::new();
        for (i, f) in new.iter().enumerate() {
            if let Kind::Archive(a) = &f.kind {
                for (m, member) in a.members.iter().enumerate() {
                    if let MemberContent::Inline { offset, len, .. } = member.content {
                        slots.push((i, m, wanted[i].0, offset, len));
                    }
                }
            }
        }
        let member_facts = pool.unit_cost(2400).map(slots.len(), |s| {
            let (_, _, b, offset, len) = slots[s];
            // The member's view: its parts, which hold what the analysis
            // reads (§11.3).
            match store.view(b).sub(offset, len) {
                Some(view) => facts::analyse_view(view, true),
                None => Facts {
                    ident: None,
                    kind: Kind::Bad(Bad::Malformed("member out of range".to_owned())),
                },
            }
        });
        for ((i, m, ..), mf) in slots.into_iter().zip(member_facts) {
            if let Kind::Archive(a) = &mut new[i].kind
                && let MemberContent::Inline { facts, .. } = &mut a.members[m].content
            {
                *facts = Some(Box::new(mf));
            }
        }
        self.facts.extend(wanted.into_iter().zip(new));
        timing.facts += t.elapsed();
    }
}

/// Runs the walk until it needs nothing, filling the caches between
/// passes (§12.1).
#[must_use]
pub fn walk<'s>(
    plan: &Plan,
    env: &WalkEnv,
    store: &'s Store,
    pool: &Pool,
    options: Options,
) -> Loaded<'s> {
    let mut caches = Caches::default();
    let mut waves = 0u32;
    let mut timing = Timing::default();
    loop {
        let t = Instant::now();
        let mut w = Walk::new(plan, env, store, &caches, options);
        w.run();
        if w.needs.is_empty() {
            let mut out = w.finish();
            timing.walk += t.elapsed();
            out.waves = waves;
            let t = Instant::now();
            if store.hashes() {
                hash_members(store, pool, &mut out);
            }
            timing.hash += t.elapsed();
            out.timing = timing;
            return out;
        }
        let needs = std::mem::take(&mut w.needs);
        drop(w);
        timing.walk += t.elapsed();
        caches.fill(store, pool, needs, &mut timing);
        waves += 1;
        assert!(waves < 100_000, "the input walk does not converge (an internal error)");
    }
}

/// §11.6: the hash of every row's bytes (a file's, or an archive
/// member's). The staged store reads the bytes again to hash them, and a
/// file that no longer holds what its parts hold is `input-changed`, at
/// the row's file.
fn hash_members(store: &Store, pool: &Pool, out: &mut Loaded<'_>) {
    let files = &out.files;
    let hashes = pool.map(files.len(), |i| {
        let f = &files[i];
        if f.base == 0 && f.len == store.len(f.blob) {
            return store.hash(f.blob);
        }
        store.hash_range(f.blob, f.base..f.base + f.len)
    });
    let mut changed = false;
    for (f, h) in out.files.iter_mut().zip(hashes) {
        match h {
            Ok(h) => f.hash = h,
            Err(e) => {
                let p = read_problem(f.origin.arg, 0, &f.path, &e);
                out.problems.push(p.at(Place::File(f.name.clone())));
                changed = true;
            }
        }
    }
    if changed {
        crate::problem::sort(&mut out.problems);
    }
}

// ------------------------------------------------------------------ walk

/// A group (§10.2): a command-line group, or a script `GROUP` outside one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum GroupKey {
    Cli(u32),
    Script(u32),
}

/// How a mention named its file, for the needed name (§8.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Via {
    Path,
    Library,
    Script,
    Closure,
}

#[derive(Clone, Copy, Debug)]
#[allow(clippy::struct_excessive_bools, reason = "the positional state")]
struct MState {
    as_needed: bool,
    whole_archive: bool,
    link_static: bool,
    copy_dt_needed: bool,
    group: Option<GroupKey>,
    lib: bool,
}

/// One mention (§9.1): an input or a script entry, with its state.
#[derive(Clone, Debug)]
struct Mention {
    input: u32,
    arg: u32,
    state: MState,
    /// The scripts being expanded on this chain (§5.3).
    chain: Vec<Blob>,
    /// `>>> named by` places, innermost first (§13).
    named_by: Vec<Place>,
    entry: Option<u32>,
    edge: Option<u32>,
    via: Via,
    /// The key position of the mention itself (§12.2).
    seq: u64,
    /// Where a problem of the mention itself is (an argument, an entry).
    place: Place,
    /// `-b NAME` of the target: the file must be an ELF file (§2.6).
    require_elf: Option<String>,
}

impl Mention {
    /// The `named by` places of a file this mention's script names: its
    /// own chain, or, for a command-line input, its argument.
    fn chain_places(&self) -> Vec<Place> {
        if self.named_by.is_empty() { vec![self.place.clone()] } else { self.named_by.clone() }
    }

    fn origin(&self) -> Origin {
        Origin { input: self.input, arg: self.arg, entry: self.entry, edge: self.edge }
    }
}

/// A file of a script being expanded (the script or an `INCLUDE`d file).
struct ScriptFile<'s> {
    path: Spelled,
    bytes: &'s [u8],
    k: u32,
    opened: u32,
}

/// The result of a search.
enum Searched<'c, 's> {
    Pending,
    Found {
        path: Spelled,
        blob: Blob,
        facts: &'c Facts<'s>,
    },
    NotFound {
        skipped: Vec<(Spelled, SkipWhy)>,
    },
    /// An error was emitted.
    Failed,
}

struct Walk<'c, 's> {
    plan: &'c Plan,
    look: Look<'c, 's>,
    store: &'s Store,
    options: Options,
    needs: Needs,
    target: Option<TargetChoice>,
    files: Vec<FileRow<'s>>,
    archives: Vec<ArchiveRow>,
    link: LinkTable,
    problems: Vec<Problem>,
    group_ordinals: HashMap<GroupKey, u32>,
    script_groups: u32,
    soname_units: FixedMap<Spelled, usize>,
    mentions_of_path: FixedMap<Spelled, u32>,
    read_reported: HashSet<PathBuf>,
    /// The `-L` directories, resolved (§4.2 item 1, §4.3).
    cli_dirs: Vec<Spelled>,
    /// The `SEARCH_DIR`s of the scripts expanded so far (§4.2 item 2).
    script_dirs: Vec<Spelled>,
    /// The next file ordinal of each unit (§12.2).
    next_k: HashMap<u32, u32>,
    static_refused: bool,
    /// `OUTPUT_FORMAT`, `OUTPUT_ARCH`, `TARGET`: checked once the target
    /// is known (§3.2).
    late: Vec<(Problem, LateCheck)>,
    little: bool,
}

#[derive(Clone, Debug)]
enum LateCheck {
    Format(Spelled),
    Arch(Spelled),
    Target(Spelled),
}

impl<'c, 's> Walk<'c, 's> {
    fn new(
        plan: &'c Plan,
        env: &'c WalkEnv,
        store: &'s Store,
        caches: &'c Caches<'s>,
        options: Options,
    ) -> Self {
        Self {
            plan,
            look: Look { caches, cwd: &env.cwd },
            store,
            options,
            needs: Needs::default(),
            target: None,
            files: Vec::new(),
            archives: Vec::new(),
            link: LinkTable::default(),
            problems: Vec::new(),
            group_ordinals: HashMap::new(),
            script_groups: 0,
            soname_units: FixedMap::default(),
            mentions_of_path: FixedMap::default(),
            read_reported: HashSet::new(),
            cli_dirs: Vec::new(),
            script_dirs: Vec::new(),
            next_k: HashMap::new(),
            static_refused: false,
            late: Vec::new(),
            little: plan.other.iter().any(|o| o.name == "-EL"),
        }
    }

    fn sysroot(&self) -> Option<&'c [u8]> {
        self.plan.sysroot.as_deref().map(OsStrExt::as_bytes)
    }

    fn take_k(&mut self, unit: u32) -> u32 {
        let k = self.next_k.entry(unit).or_insert(0);
        let out = *k;
        *k += 1;
        out
    }

    #[allow(clippy::too_many_lines, reason = "the passes of §10.1 over the plan inputs, in order")]
    fn run(&mut self) {
        self.resolve_cli_dirs();
        self.decide_target_early();
        let plan = self.plan;
        for (i, input) in plan.inputs.iter().enumerate() {
            let s = &input.state;
            let state = MState {
                as_needed: s.as_needed,
                whole_archive: s.whole_archive,
                link_static: s.link_static,
                copy_dt_needed: s.copy_dt_needed,
                group: s.group.map(GroupKey::Cli),
                lib: s.lib.is_some(),
            };
            let label = argument_text(&input.spec);
            let mut m = Mention {
                input: u32::try_from(i).unwrap_or(u32::MAX),
                arg: input.arg,
                state,
                chain: Vec::new(),
                named_by: Vec::new(),
                entry: None,
                edge: None,
                via: Via::Path,
                seq: 0,
                place: Place::argument(label),
                require_elf: None,
            };
            // The input's own file is ordinal 0 of its unit (§12.2).
            self.next_k.insert(input.arg, 0);
            match &s.format {
                InputFormat::Detect => {}
                InputFormat::Binary => {
                    self.problems.push(
                        Problem::refused(m.arg, 0, RefusalClass::FileFormat, "binary", false)
                            .at(m.place.clone()),
                    );
                    continue;
                }
                InputFormat::Named(n) => {
                    let n = n.as_bytes();
                    let ok = match &self.target {
                        Some(t) => n == bfd_name(t.target.machine).as_bytes(),
                        None => [Machine::AArch64, Machine::X86_64, Machine::I386]
                            .iter()
                            .any(|&mm| n == bfd_name(mm).as_bytes()),
                    };
                    if !ok {
                        self.problems.push(
                            Problem::refused(m.arg, 0, RefusalClass::FileFormat, text(n), false)
                                .at(m.place.clone()),
                        );
                        continue;
                    }
                    m.require_elf = Some(text(n));
                }
            }
            match &input.spec {
                InputSpec::Path(p) => {
                    let p = p.as_bytes();
                    if p.starts_with(b"=") {
                        self.problems.push(
                            Problem::refused(m.arg, 0, RefusalClass::Feature, "'=' prefix on an input path", false)
                                .at(m.place.clone())
                                .note("GNU ld reads it as a path inside the sysroot, lld and mold as a file whose name starts with '='"),
                        );
                        continue;
                    }
                    self.visit_explicit(&m, p.into());
                }
                InputSpec::Library(name) => {
                    m.via = Via::Library;
                    let dirs = self.search_dirs(&[]);
                    let names = library_names(name.as_bytes(), m.state.link_static);
                    self.visit_search(
                        &m,
                        SearchForm::Library,
                        name.as_bytes(),
                        &dirs,
                        &names,
                        SearchRef::Input(m.input),
                    );
                }
                InputSpec::LibraryFile(name) => {
                    m.via = Via::Library;
                    let dirs = self.search_dirs(&[]);
                    let names = vec![name.as_bytes().into()];
                    self.visit_search(
                        &m,
                        SearchForm::LibraryFile,
                        name.as_bytes(),
                        &dirs,
                        &names,
                        SearchRef::Input(m.input),
                    );
                }
                InputSpec::JustSymbols(x) => self.just_symbols(&m, x.as_bytes()),
            }
        }
        if self.options.closure {
            self.closure();
        }
        self.finish_checks();
    }

    // ------------------------------------------------------- the target

    /// §3.1 rules 1 and 2.
    fn decide_target_early(&mut self) {
        if let Some(e) = self.plan.emulation {
            let machine = search::machine_of(e.target());
            self.target = Some(TargetChoice {
                target: LinkTarget::of(machine),
                rule: 1,
                by: format!("-m {}", e.name()),
            });
            return;
        }
        let look = self.look;
        for input in &self.plan.inputs {
            let InputSpec::Path(p) = &input.spec else { continue };
            let p = p.as_bytes();
            if p.starts_with(b"=")
                || !matches!(input.state.format, InputFormat::Detect | InputFormat::Named(_))
            {
                continue;
            }
            match look.open(&mut self.needs, p, false) {
                Got::Pending => return,
                Got::Failed(_) => {}
                Got::File(_, f) => match self.elf_machine(p, f) {
                    Some(Some(machine)) => {
                        self.target = Some(TargetChoice {
                            target: LinkTarget::of(machine),
                            rule: 2,
                            by: text(p),
                        });
                        return;
                    }
                    Some(None) => {}
                    None => return,
                },
            }
        }
    }

    /// The machine a file sets the target to, if it is an "ELF file" of
    /// §3.1; `None`: not known yet.
    #[allow(clippy::option_option, reason = "outer None: not known yet")]
    fn elf_machine(&mut self, path: &[u8], f: &'c Facts<'s>) -> Option<Option<Machine>> {
        match &f.kind {
            Kind::Object(_) | Kind::Shared(_) => Some(f.ident.and_then(machine_of_ident)),
            Kind::Archive(a) => {
                for m in &a.members {
                    match self.member_facts(path, m) {
                        MemberGot::Pending => return None,
                        MemberGot::Facts { facts, .. } => {
                            if matches!(facts.kind, Kind::Object(_)) {
                                return Some(facts.ident.and_then(machine_of_ident));
                            }
                        }
                        MemberGot::Failed(_) => {}
                    }
                }
                Some(None)
            }
            _ => Some(None),
        }
    }

    /// Sets the target by rule 3 when it is not known yet.
    fn rule_3(&mut self, path: &[u8], f: &'c Facts<'s>) -> bool {
        if self.target.is_some() {
            return true;
        }
        match self.elf_machine(path, f) {
            None => false,
            Some(Some(machine)) => {
                self.target =
                    Some(TargetChoice { target: LinkTarget::of(machine), rule: 3, by: text(path) });
                true
            }
            Some(None) => true,
        }
    }

    /// `incompatible-file` for an ELF ident not of the target (§3.3).
    fn incompatible(&self, ident: Option<ElfIdent>) -> Option<(Dk, String)> {
        let t = self.target.as_ref()?;
        let ident = ident?;
        if ident.is_of(t.target.machine) {
            return None;
        }
        Some((
            Dk::IncompatibleFile {
                expected: bfd_name(t.target.machine).to_owned(),
                found: ident.target_name(),
            },
            format!("the target was set by {}", t.by),
        ))
    }

    // ------------------------------------------------- search directories

    fn resolve_cli_dirs(&mut self) {
        let plan = self.plan;
        for l in &plan.library_paths {
            let d = l.path.as_bytes();
            let place = Place::argument(format!(
                "-L{}",
                if l.in_sysroot { "=" } else { "" }.to_owned() + &text(d)
            ));
            if d.is_empty() {
                self.problems.push(
                    Problem::error(
                        l.arg,
                        0,
                        Dk::InvalidOptionValue { option: "-L".to_owned(), value: String::new() },
                    )
                    .at(place),
                );
                continue;
            }
            if l.in_sysroot {
                match in_sysroot(self.sysroot(), d) {
                    InSysroot::Dir(dir) => self.cli_dirs.push(dir),
                    InSysroot::Ambiguous { concatenated, joined } => {
                        self.problems.push(
                            ambiguous(l.arg, 0, &format!("={}", text(d)), &concatenated, &joined)
                                .at(place),
                        );
                    }
                }
            } else {
                self.cli_dirs.push(d.into());
            }
        }
    }

    /// The library search path at this point: `-L`, the `SEARCH_DIR`s of
    /// earlier scripts, then `own` (§4.2).
    fn search_dirs(&self, own: &[Spelled]) -> Vec<Spelled> {
        let mut v = self.cli_dirs.clone();
        if !self.plan.nostdlib {
            v.extend(self.script_dirs.iter().cloned());
            v.extend(own.iter().cloned());
        }
        v
    }

    // ------------------------------------------------------- searching

    /// Makes the probes of one search (§4.4, §4.6) over `dirs` × `names`;
    /// `skip` applies the skip rule when the target is known.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_lines, reason = "the §4.4 candidate loop with its probe records")]
    fn search(
        &mut self,
        m: &Mention,
        form: SearchForm,
        name: &[u8],
        candidates: &[Spelled],
        sref: SearchRef,
        skip: bool,
    ) -> Searched<'c, 's> {
        let look = self.look;
        // Every candidate is stat'ed in one wave.
        let mut missing = false;
        for c in candidates {
            if look.stat(&mut self.needs, c).is_none() {
                missing = true;
            }
        }
        if missing {
            return Searched::Pending;
        }
        let label = search_label(form, name);
        let mut skipped = Vec::new();
        for c in candidates {
            let stat = look.stat(&mut self.needs, c).cloned().unwrap_or(Stat::Absent);
            match stat {
                Stat::Absent => {
                    self.link.probes.push(Probe {
                        path: c.clone(),
                        search: sref,
                        outcome: ProbeOutcome::Absent,
                    });
                }
                Stat::Directory => {
                    self.link.probes.push(Probe {
                        path: c.clone(),
                        search: sref,
                        outcome: ProbeOutcome::Error(Some(21)),
                    });
                    if self.first_report(c) {
                        self.problems.push(Self::mention_problem(
                            m,
                            Problem::error(
                                m.arg,
                                m.seq,
                                Dk::CannotOpenFile {
                                    path: text(c),
                                    reason: "Is a directory".to_owned(),
                                },
                            ),
                        ));
                    }
                    return Searched::Failed;
                }
                Stat::Error { errno, reason } => {
                    self.link.probes.push(Probe {
                        path: c.clone(),
                        search: sref,
                        outcome: ProbeOutcome::Error(Some(errno)),
                    });
                    if self.first_report(c) {
                        self.problems.push(Self::mention_problem(
                            m,
                            Problem::error(
                                m.arg,
                                m.seq,
                                Dk::CannotOpenFile { path: text(c), reason: reason.clone() },
                            ),
                        ));
                    }
                    return Searched::Failed;
                }
                _ => match look.open(&mut self.needs, c, false) {
                    Got::Pending => return Searched::Pending,
                    Got::Failed(e) => {
                        let errno = match &e {
                            ReadError::Os { errno, .. } => *errno,
                            _ => None,
                        };
                        self.link.probes.push(Probe {
                            path: c.clone(),
                            search: sref,
                            outcome: ProbeOutcome::Error(errno),
                        });
                        self.read_error(m, c, &e);
                        return Searched::Failed;
                    }
                    Got::File(blob, f) => {
                        if skip && self.target.is_some() {
                            match self.skip_reason(c, f) {
                                None => return Searched::Pending,
                                Some(Some(why)) => {
                                    self.link.probes.push(Probe {
                                        path: c.clone(),
                                        search: sref,
                                        outcome: ProbeOutcome::Skipped(why.clone()),
                                    });
                                    self.problems.push(Self::mention_problem(
                                        m,
                                        Problem::warning(
                                            m.arg,
                                            m.seq,
                                            Dk::SkippedIncompatible {
                                                path: text(c),
                                                search: label.clone(),
                                            },
                                        ),
                                    ));
                                    skipped.push((c.clone(), why));
                                    continue;
                                }
                                Some(None) => {}
                            }
                        }
                        let at = u32::try_from(self.link.opened.len()).unwrap_or(u32::MAX);
                        self.link.probes.push(Probe {
                            path: c.clone(),
                            search: sref,
                            outcome: ProbeOutcome::Found(at),
                        });
                        return Searched::Found { path: c.clone(), blob, facts: f };
                    }
                },
            }
        }
        Searched::NotFound { skipped }
    }

    /// Why a found candidate is skipped (§4.4): `Some(None)` it is not,
    /// `None` not known yet.
    #[allow(clippy::option_option, reason = "outer None: not known yet")]
    fn skip_reason(&mut self, path: &[u8], f: &'c Facts<'s>) -> Option<Option<SkipWhy>> {
        let target = self.target.as_ref()?.target.machine;
        if let Some(ident) = f.ident {
            return Some((!ident.is_of(target)).then_some(SkipWhy::Elf(ident)));
        }
        match &f.kind {
            Kind::Archive(a) => {
                for m in &a.members {
                    match self.member_facts(path, m) {
                        MemberGot::Pending => return None,
                        MemberGot::Facts { facts, .. } => {
                            if let Some(ident) = facts.ident.filter(|i| i.e_type == ET_REL) {
                                return Some((!ident.is_of(target)).then_some(SkipWhy::Elf(ident)));
                            }
                        }
                        MemberGot::Failed(_) => {}
                    }
                }
                Some(None)
            }
            Kind::Script(s) => {
                let Ok(script) = &s.parsed else { return Some(None) };
                for c in &script.commands {
                    if let CommandKind::OutputFormat { default, little, .. } = &c.kind {
                        let name = self.format_in_effect(default, little.as_ref());
                        if search::is_known_bfd(name) && name != bfd_name(target).as_bytes() {
                            return Some(Some(SkipWhy::Format(name.into())));
                        }
                    }
                }
                Some(None)
            }
            _ => Some(None),
        }
    }

    /// The `OUTPUT_FORMAT` name in effect (script.md §2.5: the third with
    /// `-EL`, the first otherwise; `-EB` is refused by the command line).
    fn format_in_effect<'w>(
        &self,
        default: &'w mortar_script::Word,
        little: Option<&'w mortar_script::Word>,
    ) -> &'w [u8] {
        match little {
            Some(l) if self.little => &l.text,
            _ => &default.text,
        }
    }

    /// A `-l`, `-l:` or script library entry (§4.4).
    fn visit_search(
        &mut self,
        m: &Mention,
        form: SearchForm,
        name: &[u8],
        dirs: &[Spelled],
        names: &[Spelled],
        sref: SearchRef,
    ) {
        let candidates: Vec<Spelled> =
            dirs.iter().flat_map(|d| names.iter().map(move |n| join(d, n))).collect();
        match self.search(m, form, name, &candidates, sref, true) {
            Searched::Pending | Searched::Failed => {}
            Searched::Found { path, blob, facts } => self.visit_open(m, path, blob, facts),
            Searched::NotFound { skipped } => {
                let label = search_label(form, name);
                let mut p = Problem::error(
                    m.arg,
                    m.seq,
                    Dk::LibraryNotFound {
                        name: label.strip_prefix("-l").unwrap_or(&label).to_owned(),
                    },
                );
                if dirs.is_empty() {
                    p = p.note("the library search path is empty");
                }
                for d in dirs {
                    p = p.note(format!("searched {}", text(d)));
                }
                for (path, why) in &skipped {
                    p = p.note(format!("skipped {}: {why}", text(path)));
                }
                if form == SearchForm::Library && m.state.link_static {
                    let mut so = b"lib".to_vec();
                    so.extend_from_slice(name);
                    so.extend_from_slice(b".so");
                    let look = self.look;
                    for d in dirs {
                        let c = join(d, &so);
                        match look.stat(&mut self.needs, &c) {
                            Some(s) if s.exists() => {
                                p = p
                                    .note(format!("{} exists but -Bstatic is in effect", text(&c)));
                            }
                            _ => {}
                        }
                    }
                }
                let p = Self::mention_problem(m, p);
                self.problems.push(p);
            }
        }
    }

    /// An explicit path: opened as written, never searched (§4.1).
    fn visit_explicit(&mut self, m: &Mention, path: Spelled) {
        let look = self.look;
        match look.open(&mut self.needs, &path, false) {
            Got::Pending => {}
            Got::Failed(e) => self.read_error(m, &path, &e),
            Got::File(blob, f) => self.visit_open(m, path, blob, f),
        }
    }

    /// A problem of the mention itself: at its place, with its chain.
    fn mention_problem(m: &Mention, p: Problem) -> Problem {
        let mut p = p.at(m.place.clone());
        if m.named_by.len() > 1 {
            p = p.named_by(&m.named_by[1..]);
        }
        p
    }

    /// A problem at a file the mention opened.
    fn file_problem(m: &Mention, p: Problem, file: FileRef) -> Problem {
        p.at(Place::File(file)).named_by(&m.named_by)
    }

    /// Whether a failure to open or read `path` is the first for its file
    /// (§9.3: one diagnostic, at the lowest mention): the file is its
    /// device and inode when `stat` gave them, else its path.
    fn first_report(&mut self, path: &[u8]) -> bool {
        let fs = fs_path(self.look.cwd, path);
        let key = match self.look.caches.stats.get(&fs) {
            Some(Stat::Regular(meta) | Stat::CharDevice(meta)) => {
                PathBuf::from(format!("\0inode:{}:{}", meta.dev, meta.ino))
            }
            _ => fs,
        };
        self.read_reported.insert(key)
    }

    /// A read error, once per file (§9.3: at the lowest mention).
    fn read_error(&mut self, m: &Mention, path: &[u8], e: &ReadError) {
        if !self.first_report(path) {
            return;
        }
        let file = FileRef::new(text(path));
        let p = read_problem(m.arg, m.seq, path, e);
        self.problems.push(Self::file_problem(m, p, file));
    }

    fn push_opened(&mut self, m: &Mention, path: &[u8], kind: OpenedKind) -> u32 {
        self.link.opened.push(Opened { path: path.into(), kind, origin: m.origin() });
        u32::try_from(self.link.opened.len() - 1).unwrap_or(u32::MAX)
    }

    fn mention_ordinal(&mut self, path: &[u8]) -> u32 {
        let n = self.mentions_of_path.entry(path.into()).or_insert(0);
        let out = *n;
        *n += 1;
        out
    }

    fn group_of(&mut self, state: &MState) -> Option<u32> {
        let key = state.group?;
        let next = u32::try_from(self.group_ordinals.len() + 1).unwrap_or(u32::MAX);
        Some(*self.group_ordinals.entry(key).or_insert(next))
    }

    // -------------------------------------------------- classification

    /// A file the mention found and read (§10.1 `match the kind of F`).
    fn visit_open(&mut self, m: &Mention, path: Spelled, blob: Blob, f: &'c Facts<'s>) {
        let unit = m.arg;
        let file = FileRef::new(text(&path));
        if let Some(name) = &m.require_elf
            && f.ident.is_none()
            && !matches!(f.kind, Kind::Bad(_))
        {
            let k = self.take_k(unit);
            self.push_opened(m, &path, OpenedKind::Other);
            let p =
                Problem::refused(unit, seq(k, 0), RefusalClass::FileFormat, name.clone(), false);
            self.problems.push(Self::file_problem(m, p, file));
            return;
        }
        match &f.kind {
            Kind::Empty => {
                self.take_k(unit);
                self.push_opened(m, &path, OpenedKind::Empty);
            }
            Kind::Object(o) => {
                if !self.rule_3(&path, f) {
                    return;
                }
                let k = self.take_k(unit);
                let opened = self.push_opened(m, &path, OpenedKind::Object);
                if let Some((msg, note)) = self.incompatible(f.ident) {
                    let p = Problem::error(unit, seq(k, 0), msg).note(note);
                    self.problems.push(Self::file_problem(m, p, file));
                    return;
                }
                let _ = opened;
                let len = self.store.len(blob);
                let identity = Identity {
                    path: path.clone(),
                    mention: self.mention_ordinal(&path),
                    member: None,
                };
                self.object_row(m, file, path, None, o, blob, 0, len, identity, m.state.lib);
            }
            Kind::Shared(s) => {
                if !self.rule_3(&path, f) {
                    return;
                }
                self.visit_shared(m, path, blob, f, s);
            }
            Kind::Archive(a) => {
                if !self.rule_3(&path, f) {
                    return;
                }
                self.visit_archive(m, &path, blob, a);
            }
            Kind::Script(s) => self.visit_script(m, &path, blob, s),
            Kind::Bad(b) => {
                let k = self.take_k(unit);
                self.push_opened(m, &path, OpenedKind::Other);
                let p = bad_problem(unit, seq(k, 0), b);
                self.problems.push(Self::file_problem(m, p, file));
            }
            Kind::Unknown | Kind::NotObject(_) => {
                let k = self.take_k(unit);
                self.push_opened(m, &path, OpenedKind::Other);
                let p = Problem::error(unit, seq(k, 0), Dk::UnknownFileType);
                self.problems.push(Self::file_problem(m, p, file));
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn object_row(
        &mut self,
        m: &Mention,
        name: FileRef,
        path: Spelled,
        member: Option<MemberRef>,
        o: &ObjectFacts<'s>,
        blob: Blob,
        base: u64,
        len: u64,
        identity: Identity,
        lazy: bool,
    ) {
        let group = self.group_of(&m.state);
        self.link.group.push(group);
        let opened = u32::try_from(self.link.opened.len().saturating_sub(1)).unwrap_or(u32::MAX);
        self.files.push(FileRow {
            kind: RowKind::Object,
            name,
            path,
            member,
            origin: m.origin(),
            state: RowState {
                as_needed: m.state.as_needed,
                whole_archive: m.state.whole_archive,
                lazy,
                link_static: m.state.link_static,
                copy_dt_needed: m.state.copy_dt_needed,
                exclude_libs: false,
            },
            blob,
            base,
            len,
            sections: o.sections,
            globals: o.globals,
            groups: o.groups,
            identity,
            hash: ContentHash(0),
            soname: None,
            reader: Reader::Object(o.object),
            versions: VersionSlots::default(),
            keys: o.keys.clone(),
            opened,
        });
    }

    /// §8.1-§8.5.
    #[allow(clippy::too_many_lines, reason = "the rules of §8.1-§8.5 in order")]
    fn visit_shared(
        &mut self,
        m: &Mention,
        path: Spelled,
        blob: Blob,
        f: &Facts<'s>,
        s: &SharedFacts<'s>,
    ) {
        let unit = m.arg;
        let file = FileRef::new(text(&path));
        let k = self.take_k(unit);
        let at = seq(k, 0);
        if m.state.link_static {
            self.push_opened(m, &path, OpenedKind::Shared { loaded: false });
            let p = Problem::error(unit, at, Dk::StaticLinkOfSharedObject { path: text(&path) });
            self.problems.push(Self::file_problem(m, p, file));
            return;
        }
        if let Some((msg, note)) = self.incompatible(f.ident) {
            self.push_opened(m, &path, OpenedKind::Shared { loaded: false });
            let p = Problem::error(unit, at, msg).note(note);
            self.problems.push(Self::file_problem(m, p, file));
            return;
        }
        let soname: (Spelled, bool) = match s.soname {
            Some(n) => (n.into(), true),
            None => match m.via {
                Via::Path => (path.clone(), false),
                Via::Library | Via::Closure => (file_name(&path).into(), false),
                Via::Script => {
                    self.push_opened(m, &path, OpenedKind::Shared { loaded: false });
                    let p = Problem::refused(
                        unit,
                        at,
                        RefusalClass::Feature,
                        "shared object without DT_SONAME named by a linker script",
                        false,
                    );
                    self.problems.push(Self::file_problem(m, p, file));
                    return;
                }
            },
        };
        if let Some(&u) = self.soname_units.get(&soname.0) {
            let opened = self.push_opened(m, &path, OpenedKind::Shared { loaded: false });
            let first = &self.files[self.link.sonames[u].file as usize - 1];
            // Both are read whole by either store (§11.1): no read can fail.
            let same = self.store.same_bytes(first.blob, blob).unwrap_or(false);
            if !same {
                let p = Problem::warning(unit, at, Dk::DuplicateSoname { soname: text(&soname.0) })
                    .note(format!("loaded from {}", text(&first.path)));
                self.problems.push(Self::file_problem(m, p, file));
            }
            let unit_row = &mut self.link.sonames[u];
            unit_row.mentions.push(opened);
            unit_row.as_needed &= m.state.as_needed;
            unit_row.copy_dt_needed |= m.state.copy_dt_needed;
            return;
        }
        let opened = self.push_opened(m, &path, OpenedKind::Shared { loaded: true });
        if matches!(self.plan.kind, OutputKind::StaticExecutable | OutputKind::StaticPie)
            && !self.static_refused
        {
            self.static_refused = true;
            let p = Problem::refused(
                unit,
                at,
                RefusalClass::Feature,
                "dynamic executable without -dynamic-linker",
                false,
            )
            .note(format!("the shared object is {}", text(&path)));
            self.problems.push(Self::file_problem(m, p, file.clone()));
        }
        let group = self.group_of(&m.state);
        self.link.group.push(group);
        let len = self.store.len(blob);
        let identity =
            Identity { path: path.clone(), mention: self.mention_ordinal(&path), member: None };
        let id = u32::try_from(self.files.len() + 1).unwrap_or(u32::MAX);
        self.soname_units.insert(soname.0.clone(), self.link.sonames.len());
        self.link.local_binding.extend(local_binding(FileId(id), s));
        self.link.sonames.push(SonameUnit {
            file: id,
            mentions: vec![opened],
            as_needed: m.state.as_needed,
            copy_dt_needed: m.state.copy_dt_needed,
        });
        self.files.push(FileRow {
            kind: RowKind::SharedObject,
            name: file,
            path,
            member: None,
            origin: m.origin(),
            state: RowState {
                as_needed: m.state.as_needed,
                whole_archive: m.state.whole_archive,
                lazy: false,
                link_static: m.state.link_static,
                copy_dt_needed: m.state.copy_dt_needed,
                exclude_libs: false,
            },
            blob,
            base: 0,
            len,
            sections: s.dso.section_table().len(),
            globals: s.globals,
            groups: 0,
            identity,
            hash: ContentHash(0),
            soname: Some(soname),
            reader: Reader::Shared(s.dso),
            versions: s.versions.clone(),
            keys: ObjectKeys::default(),
            opened,
        });
    }

    // ------------------------------------------------------- archives

    /// A member's facts and bytes (§7.2).
    fn member_facts(&mut self, archive: &[u8], m: &'c MemberFacts<'s>) -> MemberGot<'c, 's> {
        let look = self.look;
        match &m.content {
            MemberContent::Inline { facts: Some(f), offset, len } => {
                MemberGot::Facts { facts: f, blob: None, base: *offset, len: *len, thin: None }
            }
            MemberContent::Inline { facts: None, .. } => {
                MemberGot::Failed(MemberError::Malformed("member not analysed".to_owned()))
            }
            MemberContent::External { path } => {
                let p = thin_path(archive, path);
                match look.open(&mut self.needs, &p, true) {
                    Got::Pending => MemberGot::Pending,
                    Got::Failed(e) => MemberGot::Failed(MemberError::Read(p, e)),
                    Got::File(b, f) => {
                        let len = self.store.len(b);
                        MemberGot::Facts { facts: f, blob: Some(b), base: 0, len, thin: Some(p) }
                    }
                }
            }
            MemberContent::ExternalMember { archive_path, header_offset } => {
                let p = thin_path(archive, archive_path);
                match look.open(&mut self.needs, &p, false) {
                    Got::Pending => MemberGot::Pending,
                    Got::Failed(e) => MemberGot::Failed(MemberError::Read(p, e)),
                    Got::File(b, f) => {
                        let Kind::Archive(nested) = &f.kind else {
                            return MemberGot::Failed(MemberError::Malformed(format!(
                                "the thin member's archive {} is not an archive",
                                text(&p)
                            )));
                        };
                        let found =
                            nested.members.iter().find(|n| n.header_offset == *header_offset);
                        match found.map(|n| &n.content) {
                            Some(MemberContent::Inline { facts: Some(mf), offset, len }) => {
                                MemberGot::Facts {
                                    facts: mf,
                                    blob: Some(b),
                                    base: *offset,
                                    len: *len,
                                    thin: Some(p),
                                }
                            }
                            _ => MemberGot::Failed(MemberError::Malformed(format!(
                                "no member stored at offset {header_offset:#x} of {}",
                                text(&p)
                            ))),
                        }
                    }
                }
            }
        }
    }

    /// §7.1-§7.5.
    #[allow(clippy::too_many_lines, reason = "the §7.3 member table, case by case")]
    fn visit_archive(&mut self, m: &Mention, path: &Spelled, blob: Blob, a: &'c ArchiveFacts<'s>) {
        // Every member's file first, so that one wave reads them all.
        let mut pending = false;
        for mem in &a.members {
            if matches!(self.member_facts(path, mem), MemberGot::Pending) {
                pending = true;
            }
        }
        if pending {
            return;
        }
        let unit = m.arg;
        let k = self.take_k(unit);
        let opened = self.push_opened(m, path, OpenedKind::Archive);
        let archive_file = text(path);
        if let Some((e, _)) = &a.error {
            let p = Problem::error(unit, seq(k, 0), Dk::Malformed { what: e.clone() });
            self.problems.push(Self::file_problem(m, p, FileRef::new(archive_file.clone())));
        }
        if let Some(e) = &a.index_error {
            let what = format!("the symbol index is malformed and is not used: {e}");
            let p = Problem::warning(unit, seq(k, 0), Dk::Malformed { what });
            self.problems.push(Self::file_problem(m, p, FileRef::new(archive_file.clone())));
        }
        let id = u32::try_from(self.archives.len()).unwrap_or(u32::MAX);
        self.archives.push(ArchiveRow {
            path: path.clone(),
            thin: a.thin,
            input: m.input,
            opened,
            origin: m.origin(),
        });
        let archive_mention = self.mention_ordinal(path);
        let mut ordinals: FixedMap<&[u8], u32> = FixedMap::default();
        for mem in &a.members {
            let ordinal = {
                let n = ordinals.entry(mem.name).or_insert(0);
                *n += 1;
                *n - 1
            };
            let got = self.member_facts(path, mem);
            let (facts, member_blob, base, len, thin) = match got {
                MemberGot::Pending => return,
                MemberGot::Facts { facts, blob: b, base, len, thin } => (facts, b, base, len, thin),
                MemberGot::Failed(MemberError::Read(p, e)) => {
                    let place = FileRef::in_archive(archive_file.clone(), text(&p));
                    let pr = read_problem(unit, seq(k, mem.header_offset), &p, &e);
                    self.problems.push(Self::file_problem(m, pr, place));
                    continue;
                }
                MemberGot::Failed(MemberError::Malformed(text_)) => {
                    let place = FileRef::in_archive(archive_file.clone(), text(mem.name));
                    let pr = Problem::error(
                        unit,
                        seq(k, mem.header_offset),
                        Dk::Malformed { what: text_ },
                    );
                    self.problems.push(Self::file_problem(m, pr, place));
                    continue;
                }
            };
            let member_k = if let Some(t) = &thin {
                let mk = self.take_k(unit);
                self.push_opened(m, t, OpenedKind::ThinMember);
                Some(mk)
            } else {
                None
            };
            let display = FileRef::in_archive(
                archive_file.clone(),
                thin.as_deref().map_or_else(|| text(mem.name), text),
            );
            let at = member_k.map_or(seq(k, mem.header_offset), |mk| seq(mk, 0));
            match &facts.kind {
                Kind::Object(o) => {
                    if let Some((msg, note)) = self.incompatible(facts.ident) {
                        let p = Problem::error(unit, at, msg).note(note);
                        self.problems.push(Self::file_problem(m, p, display));
                        continue;
                    }
                    let member = MemberRef {
                        archive: id,
                        header_offset: mem.header_offset,
                        name: mem.name.into(),
                        ordinal,
                        thin_path: thin.clone(),
                    };
                    let identity = Identity {
                        path: path.clone(),
                        mention: archive_mention,
                        member: Some((mem.name.into(), ordinal)),
                    };
                    self.object_row(
                        m,
                        display,
                        path.clone(),
                        Some(member),
                        o,
                        member_blob.unwrap_or(blob),
                        base,
                        len,
                        identity,
                        false,
                    );
                }
                Kind::NotObject(what) => {
                    let message = Dk::IgnoredMember { what: what.to_string() };
                    let p = if m.state.whole_archive {
                        Problem::error(unit, at, message)
                    } else {
                        Problem::warning(unit, at, message)
                    };
                    self.problems.push(Self::file_problem(m, p, display));
                }
                Kind::Bad(b) => {
                    let p = bad_problem(unit, at, b);
                    self.problems.push(Self::file_problem(m, p, display));
                }
                Kind::Empty
                | Kind::Unknown
                | Kind::Script(_)
                | Kind::Shared(_)
                | Kind::Archive(_) => {
                    unreachable!("member facts are analysed as members (facts::analyse_member)")
                }
            }
        }
    }

    // -------------------------------------------------------- scripts

    /// §5: an implicit script.
    #[allow(clippy::too_many_lines, reason = "the §5 script cases in order")]
    fn visit_script(&mut self, m: &Mention, path: &Spelled, blob: Blob, s: &'c facts::ScriptFacts) {
        let unit = m.arg;
        if m.chain.contains(&blob) {
            let p = Self::mention_problem(m, Problem::error(unit, m.seq, Dk::ScriptCycle));
            self.problems.push(p);
            return;
        }
        if m.chain.len() >= MAX_SCRIPT_DEPTH {
            let p = Problem::refused(
                unit,
                m.seq,
                RefusalClass::Feature,
                "linker scripts nested more than 16 deep",
                false,
            );
            let p = Self::mention_problem(m, p);
            self.problems.push(p);
            return;
        }
        let k = self.take_k(unit);
        let opened = self.push_opened(m, path, OpenedKind::Script);
        let bytes = self
            .store
            .whole(blob)
            .unwrap_or_else(|| unreachable!("a script is read whole by either store (§11.1)"));
        let Ok(script) = &s.parsed else {
            let Err(d) = mortar_script::parse(bytes, &text(path)) else {
                unreachable!("the parser is a function of the bytes")
            };
            let p = Problem::parse(unit, seq(k, 0), d).named_by(&m.named_by);
            self.problems.push(p);
            return;
        };
        let file = Rc::new(ScriptFile { path: path.clone(), bytes, k, opened });
        let mut commands = Vec::new();
        let own_before = Vec::new();
        if !self.splice(m, script, &file, 0, own_before, &mut commands) {
            return;
        }
        // All of a script's SEARCH_DIRs apply to all of its entries
        // (§4.2, script.md §2.6).
        let mut own = Vec::new();
        for (c, f) in &commands {
            if let CommandKind::SearchDir(w) = &c.kind
                && let Some(d) = self.search_dir(m, f, c, w)
            {
                own.push(d);
            }
        }
        if !self.plan.nostdlib {
            self.script_dirs.extend(own.iter().cloned());
        }
        let mut chain = m.chain.clone();
        chain.push(blob);
        for (c, f) in &commands {
            let place = script_place(f, c.span.start);
            let at = seq(f.k, c.span.start as u64);
            match &c.kind {
                CommandKind::Input(items) => self.entries(m, &chain, f, items, m.state),
                CommandKind::Group(items) => {
                    let mut state = m.state;
                    if state.group.is_none() {
                        self.script_groups += 1;
                        state.group = Some(GroupKey::Script(self.script_groups));
                    }
                    self.entries(m, &chain, f, items, state);
                }
                CommandKind::OutputFormat { default, little, .. } => {
                    let name = self.format_in_effect(default, little.as_ref()).into();
                    let p = Problem::error(unit, at, Dk::NoTarget).at(place).named_by(&m.named_by);
                    self.late.push((p, LateCheck::Format(name)));
                }
                CommandKind::OutputArch(w) => {
                    let p = Problem::error(unit, at, Dk::NoTarget).at(place).named_by(&m.named_by);
                    self.late.push((p, LateCheck::Arch(w.text.clone().into())));
                }
                CommandKind::Target(w) => {
                    let p = Problem::error(unit, at, Dk::NoTarget).at(place).named_by(&m.named_by);
                    self.late.push((p, LateCheck::Target(w.text.clone().into())));
                }
                CommandKind::SearchDir(_) | CommandKind::Include(_) => {}
                CommandKind::Output(w) | CommandKind::Entry(w) => {
                    self.link.directives.push(Directive {
                        command: c.kind.keyword(),
                        value: w.text.clone().into(),
                        script: f.opened,
                        offset: c.span.start as u64,
                    });
                }
                CommandKind::Extern(ws) => {
                    for w in ws {
                        self.link.directives.push(Directive {
                            command: "EXTERN",
                            value: w.text.clone().into(),
                            script: f.opened,
                            offset: w.span.start as u64,
                        });
                    }
                }
                CommandKind::Startup(_) => {
                    let p =
                        Problem::refused(unit, at, RefusalClass::ScriptCommand, "STARTUP", false)
                            .at(place)
                            .named_by(&m.named_by);
                    self.problems.push(p);
                }
            }
        }
    }

    /// A `SEARCH_DIR` as a directory (§4.2, §4.3).
    fn search_dir(
        &mut self,
        m: &Mention,
        f: &ScriptFile<'s>,
        c: &Command,
        w: &mortar_script::Word,
    ) -> Option<Spelled> {
        let word = &w.text[..];
        match sysroot_prefix(word) {
            None => Some(word.into()),
            Some(rest) => match in_sysroot(self.sysroot(), rest) {
                InSysroot::Dir(d) => Some(d),
                InSysroot::Ambiguous { concatenated, joined } => {
                    let p = ambiguous(
                        m.arg,
                        seq(f.k, c.span.start as u64),
                        &text(word),
                        &concatenated,
                        &joined,
                    )
                    .at(script_place(f, c.span.start))
                    .named_by(&m.named_by);
                    self.problems.push(p);
                    None
                }
            },
        }
    }

    /// Splices `INCLUDE`d files (script.md §2.8). `false`: not all known
    /// yet, or an `INCLUDE` failed (an error was emitted).
    #[allow(clippy::too_many_lines, reason = "the §5.2 entry kinds in order")]
    fn splice(
        &mut self,
        m: &Mention,
        script: &'c Script,
        file: &Rc<ScriptFile<'s>>,
        depth: usize,
        mut dirs_before: Vec<Spelled>,
        out: &mut Vec<(Command, Rc<ScriptFile<'s>>)>,
    ) -> bool {
        let mut complete = true;
        for c in &script.commands {
            match &c.kind {
                CommandKind::SearchDir(w) => {
                    if let Some(d) =
                        sysroot_prefix(&w.text).map_or(Some(w.text.clone().into()), |rest| {
                            match in_sysroot(self.sysroot(), rest) {
                                InSysroot::Dir(d) => Some(d),
                                InSysroot::Ambiguous { .. } => None,
                            }
                        })
                    {
                        dirs_before.push(d);
                    }
                    out.push((c.clone(), Rc::clone(file)));
                }
                CommandKind::Include(w) => {
                    let at = seq(file.k, c.span.start as u64);
                    let place = script_place(file, c.span.start);
                    if depth >= MAX_INCLUDE_DEPTH {
                        let p = Problem::refused(
                            m.arg,
                            at,
                            RefusalClass::Feature,
                            "INCLUDE nested more than 10 deep",
                            false,
                        )
                        .at(place)
                        .named_by(&m.named_by);
                        self.problems.push(p);
                        complete = false;
                        continue;
                    }
                    let name = &w.text[..];
                    let mut candidates: Vec<Spelled> = vec![name.into()];
                    if !search::is_absolute(name) {
                        let dirs = self.search_dirs(&dirs_before);
                        candidates.extend(dirs.iter().map(|d| join(d, name)));
                    }
                    let entry = u32::try_from(self.link.script_entries.len()).unwrap_or(u32::MAX);
                    self.link.script_entries.push(ScriptEntry {
                        script: file.opened,
                        offset: c.span.start as u64,
                        parent: m.entry,
                    });
                    let sref = SearchRef::ScriptEntry(entry);
                    let mut sub = m.clone();
                    sub.entry = Some(entry);
                    sub.place = place.clone();
                    sub.seq = at;
                    sub.named_by = std::iter::once(place.clone()).chain(m.chain_places()).collect();
                    match self.search(&sub, SearchForm::Name, name, &candidates, sref, false) {
                        Searched::Pending | Searched::Failed => complete = false,
                        Searched::NotFound { .. } => {
                            let p = Problem::error(
                                m.arg,
                                at,
                                Dk::CannotOpenFile {
                                    path: text(name),
                                    reason: "No such file or directory".to_owned(),
                                },
                            );
                            let p = Self::mention_problem(&sub, p);
                            self.problems.push(p);
                            complete = false;
                        }
                        Searched::Found { path, blob, facts } => {
                            let k = self.take_k(m.arg);
                            let opened = self.push_opened(&sub, &path, OpenedKind::Include);
                            let bytes = self.store.whole(blob).unwrap_or_else(|| {
                                unreachable!("a script is read whole by either store (§11.1)")
                            });
                            let sf = Rc::new(ScriptFile { path: path.clone(), bytes, k, opened });
                            match &facts.kind {
                                Kind::Script(s) => {
                                    if let Ok(inner) = &s.parsed {
                                        if !self.splice(
                                            &sub,
                                            inner,
                                            &sf,
                                            depth + 1,
                                            dirs_before.clone(),
                                            out,
                                        ) {
                                            complete = false;
                                        }
                                    } else {
                                        {
                                            if let Err(d) =
                                                mortar_script::parse(bytes, &text(&path))
                                            {
                                                self.problems.push(
                                                    Problem::parse(m.arg, seq(k, 0), d)
                                                        .named_by(&sub.named_by),
                                                );
                                            }
                                            complete = false;
                                        }
                                    }
                                }
                                Kind::Empty => {}
                                _ => {
                                    let p = Problem::error(m.arg, seq(k, 0), Dk::UnknownFileType);
                                    self.problems.push(Self::file_problem(
                                        &sub,
                                        p,
                                        FileRef::new(text(&path)),
                                    ));
                                    complete = false;
                                }
                            }
                        }
                    }
                }
                _ => out.push((c.clone(), Rc::clone(file))),
            }
        }
        complete
    }

    /// The entries of an `INPUT` or `GROUP` list (§5.2).
    fn entries(
        &mut self,
        m: &Mention,
        chain: &[Blob],
        f: &ScriptFile<'s>,
        items: &[InputItem],
        state: MState,
    ) {
        for item in items {
            match item {
                InputItem::AsNeeded { items, .. } => {
                    let mut inner = state;
                    inner.as_needed = true;
                    self.entries(m, chain, f, items, inner);
                }
                InputItem::File(file) => {
                    let offset = file.spelling.span.start;
                    let entry = u32::try_from(self.link.script_entries.len()).unwrap_or(u32::MAX);
                    self.link.script_entries.push(ScriptEntry {
                        script: f.opened,
                        offset: offset as u64,
                        parent: m.entry,
                    });
                    let place = script_place(f, offset);
                    let sub = Mention {
                        input: m.input,
                        arg: m.arg,
                        state,
                        chain: chain.to_vec(),
                        named_by: std::iter::once(place.clone()).chain(m.chain_places()).collect(),
                        entry: Some(entry),
                        edge: None,
                        via: Via::Script,
                        seq: seq(f.k, offset as u64),
                        place,
                        require_elf: None,
                    };
                    self.entry(&sub, f, file);
                }
            }
        }
    }

    /// One script entry (script.md §2.4).
    fn entry(&mut self, m: &Mention, f: &ScriptFile<'s>, e: &mortar_script::InputFile) {
        let name = &e.name[..];
        let sref = SearchRef::ScriptEntry(m.entry.unwrap_or(0));
        match e.form {
            FileForm::Library => {
                let dirs = self.search_dirs(&[]);
                let names = library_names(name, m.state.link_static);
                self.visit_search(m, SearchForm::Library, name, &dirs, &names, sref);
            }
            FileForm::LibraryVerbatim => {
                let dirs = self.search_dirs(&[]);
                self.visit_search(m, SearchForm::LibraryFile, name, &dirs, &[name.into()], sref);
            }
            FileForm::Sysroot => match in_sysroot(self.sysroot(), name) {
                InSysroot::Dir(p) => self.visit_explicit(m, p),
                InSysroot::Ambiguous { concatenated, joined } => {
                    let p =
                        ambiguous(m.arg, m.seq, &text(&e.spelling.text), &concatenated, &joined);
                    let p = Self::mention_problem(m, p);
                    self.problems.push(p);
                }
            },
            FileForm::Path if search::is_absolute(name) => {
                // Rule 1: inside the sysroot when the script is.
                let path = match self.sysroot() {
                    None => name.into(),
                    Some(sysroot) => {
                        let look = self.look;
                        let script = look.canon(&mut self.needs, &f.path);
                        let root = look.canon(&mut self.needs, sysroot);
                        match (script, root) {
                            (Some(script), Some(root)) => {
                                let inside = matches!((script, root), (Some(s), Some(r)) if s.starts_with(r));
                                if inside {
                                    search::under_sysroot(sysroot, name)
                                } else {
                                    name.into()
                                }
                            }
                            _ => return,
                        }
                    }
                };
                self.visit_explicit(m, path);
            }
            FileForm::Path => {
                // Rule 3: the script's directory, the working directory,
                // the library search path.
                // The script's directory, unless it is the working
                // directory (then the two probes are one).
                let script_dir = dir_of(&f.path);
                let mut candidates: Vec<Spelled> = Vec::new();
                if !script_dir.is_empty() {
                    candidates.push(join(script_dir, name));
                }
                candidates.push(name.into());
                candidates.extend(self.search_dirs(&[]).iter().map(|d| join(d, name)));
                let dirs = self.search_dirs(&[]);
                match self.search(m, SearchForm::Name, name, &candidates, sref, true) {
                    Searched::Pending | Searched::Failed => {}
                    Searched::Found { path, blob, facts } => self.visit_open(m, path, blob, facts),
                    Searched::NotFound { skipped } => {
                        let mut p = Problem::error(
                            m.arg,
                            m.seq,
                            Dk::CannotOpenFile {
                                path: text(name),
                                reason: "No such file or directory".to_owned(),
                            },
                        );
                        if !script_dir.is_empty() {
                            p = p.note(format!("searched {}", text(script_dir)));
                        }
                        p = p.note("searched the working directory");
                        for d in &dirs {
                            p = p.note(format!("searched {}", text(d)));
                        }
                        for (path, why) in &skipped {
                            p = p.note(format!("skipped {}: {why}", text(path)));
                        }
                        let p = Self::mention_problem(m, p);
                        self.problems.push(p);
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------ -R

    /// §4.8.
    fn just_symbols(&mut self, m: &Mention, x: &[u8]) {
        let look = self.look;
        match look.stat(&mut self.needs, x) {
            None => {}
            Some(Stat::Directory) => self.link.rpath_from_r.push((m.arg, x.into())),
            Some(_) => {
                self.problems.push(
                    Problem::refused(m.arg, 0, RefusalClass::Feature, "--just-symbols", false)
                        .at(m.place.clone()),
                );
            }
        }
    }

    // -------------------------------------------------------- closure

    /// Whether §8.6 loads the closure, and of which shared objects.
    fn closure_roots(&self) -> Vec<u32> {
        let all = match self.plan.kind {
            OutputKind::Executable | OutputKind::PieExecutable => true,
            OutputKind::Shared => self.plan.dynamic.allow_shlib_undefined == Some(false),
            _ => false,
        };
        if matches!(self.plan.kind, OutputKind::Relocatable) {
            return Vec::new();
        }
        self.link.sonames.iter().filter(|u| all || u.copy_dt_needed).map(|u| u.file).collect()
    }

    /// §8.6: breadth first over `(needing FileId, DT_NEEDED index)`.
    #[allow(clippy::too_many_lines, reason = "the steps of §8.6 in order")]
    fn closure(&mut self) {
        let loads = self.closure_roots();
        // A shared object of a `Shared` output whose closure is not loaded
        // (no `--no-allow-shlib-undefined`) still has its edges: each
        // satisfied by a shared object the walk loaded with that soname,
        // else unsatisfied, with no search (`spec/dyn.md` D-11 item 3's
        // `reach`; WP-123). Every other kind is unchanged.
        let mut roots: Vec<(u32, bool)> = if self.plan.kind == OutputKind::Shared {
            self.link.sonames.iter().map(|u| (u.file, loads.contains(&u.file))).collect()
        } else {
            loads.into_iter().map(|f| (f, true)).collect()
        };
        roots.sort_unstable();
        roots.dedup_by_key(|r| r.0);
        let mut queue: Vec<(u32, u32, Spelled)> = Vec::new();
        // Per entry of `queue`: whether its edge is searched.
        let mut searched: Vec<bool> = Vec::new();
        for (r, search) in roots {
            self.push_needed(r, &mut queue);
            searched.resize(queue.len(), search);
        }
        let mut reported: HashSet<(Spelled, u32)> = HashSet::new();
        let mut refused_runpath: HashSet<u32> = HashSet::new();
        let mut i = 0;
        while i < queue.len() {
            let (needing, index, name) = queue[i].clone();
            let search = searched.get(i).copied().unwrap_or(true);
            i += 1;
            let edge = u32::try_from(self.link.closure.len()).unwrap_or(u32::MAX);
            self.link.closure.push(ClosureEdge {
                needing,
                index,
                name: name.clone(),
                satisfied: None,
            });
            if let Some(&u) = self.soname_units.get(&name) {
                self.link.closure[edge as usize].satisfied = Some(self.link.sonames[u].file);
                continue;
            }
            if !search {
                continue;
            }
            let s = &self.files[needing as usize - 1];
            let unit = s.origin.arg;
            let s_path = s.path.clone();
            let s_input = s.origin.input;
            let seq0 = seq(0, 0);
            let m = Mention {
                input: s_input,
                arg: unit,
                state: MState {
                    as_needed: true,
                    whole_archive: false,
                    link_static: false,
                    copy_dt_needed: false,
                    group: None,
                    lib: false,
                },
                chain: Vec::new(),
                named_by: Vec::new(),
                entry: None,
                edge: Some(edge),
                via: Via::Closure,
                seq: seq0,
                place: Place::File(FileRef::new(text(&s_path))),
                require_elf: None,
            };
            let before = self.files.len();
            let found = if name.contains(&b'/') {
                let look = self.look;
                match look.open(&mut self.needs, &name, false) {
                    Got::Pending => return,
                    Got::Failed(e) => {
                        self.read_error(&m, &name, &e);
                        None
                    }
                    Got::File(b, f) => Some((name.clone(), b, f)),
                }
            } else {
                let dirs = self.closure_dirs(needing, &m, &mut refused_runpath);
                let candidates: Vec<Spelled> = dirs.iter().map(|d| join(d, &name)).collect();
                match self.search(
                    &m,
                    SearchForm::Name,
                    &name,
                    &candidates,
                    SearchRef::Closure(edge),
                    true,
                ) {
                    Searched::Pending => return,
                    Searched::Failed => None,
                    Searched::Found { path, blob, facts } => Some((path, blob, facts)),
                    Searched::NotFound { .. } => {
                        if reported.insert((name.clone(), needing)) {
                            let mut p = Problem::warning(
                                unit,
                                seq0,
                                Dk::NeededNotFound { name: text(&name), needed_by: text(&s_path) },
                            )
                            .at(Place::File(FileRef::new(text(&s_path))));
                            for d in &dirs {
                                p = p.note(format!("searched {}", text(d)));
                            }
                            self.problems.push(p);
                        }
                        None
                    }
                }
            };
            let Some((path, blob, f)) = found else { continue };
            if let Kind::Shared(sf) = &f.kind {
                let k_before = self.link.sonames.len();
                self.visit_shared(&m, path, blob, f, sf);
                if self.files.len() > before {
                    let id = u32::try_from(self.files.len()).unwrap_or(u32::MAX);
                    self.link.closure[edge as usize].satisfied = Some(id);
                    self.push_needed(id, &mut queue);
                } else if self.link.sonames.len() == k_before
                    && let Some(sn) = sf.soname
                    && let Some(&u) = self.soname_units.get(sn)
                {
                    self.link.closure[edge as usize].satisfied = Some(self.link.sonames[u].file);
                }
            } else {
                let k = self.take_k(unit);
                self.push_opened(&m, &path, OpenedKind::Other);
                let p = Problem::error(
                    unit,
                    seq(k, 0),
                    Dk::Malformed {
                        what: "a DT_NEEDED entry names a file that is not a shared object"
                            .to_owned(),
                    },
                )
                .at(Place::File(FileRef::new(text(&path))));
                self.problems.push(p);
            }
        }
    }

    fn push_needed(&self, file: u32, queue: &mut Vec<(u32, u32, Spelled)>) {
        let row = &self.files[file as usize - 1];
        let Reader::Shared(_) = row.reader else { return };
        let fs = fs_path(self.look.cwd, &row.path);
        let Some(Ok(b)) = self.look.caches.reads.get(&fs) else { return };
        let Some(Facts { kind: Kind::Shared(s), .. }) = self.look.caches.facts.get(&(*b, false))
        else {
            return;
        };
        for (i, n) in s.needed.iter().enumerate() {
            queue.push((file, u32::try_from(i).unwrap_or(u32::MAX), (*n).into()));
        }
    }

    /// The five steps of the closure search (§8.6). A refused
    /// `DT_RUNPATH` entry is reported once per needing file (`refused`).
    fn closure_dirs(
        &mut self,
        needing: u32,
        m: &Mention,
        refused: &mut HashSet<u32>,
    ) -> Vec<Spelled> {
        let first = refused.insert(needing);
        let sysroot = self.sysroot();
        let marked = |d: &[u8]| -> Option<Spelled> {
            match sysroot_prefix(d) {
                None => Some(d.into()),
                Some(rest) => match in_sysroot(sysroot, rest) {
                    InSysroot::Dir(x) => Some(x),
                    InSysroot::Ambiguous { .. } => None,
                },
            }
        };
        let mut dirs: Vec<Spelled> = Vec::new();
        for l in &self.plan.dynamic.rpath_link {
            dirs.extend(split_colon(l.as_bytes()).into_iter().filter_map(marked));
        }
        for l in &self.plan.dynamic.rpath {
            dirs.extend(split_colon(l.as_bytes()).into_iter().filter_map(marked));
        }
        for (_, d) in &self.link.rpath_from_r {
            dirs.extend(split_colon(d).into_iter().filter_map(marked));
        }
        let row = &self.files[needing as usize - 1];
        let s_path = row.path.clone();
        let origin_dir: Spelled = dir_of(&s_path).into();
        let fs = fs_path(self.look.cwd, &s_path);
        let runpath: Option<Spelled> = match self.look.caches.reads.get(&fs) {
            Some(Ok(b)) => match self.look.caches.facts.get(&(*b, false)) {
                Some(Facts { kind: Kind::Shared(s), .. }) => s.runpath.or(s.rpath).map(Into::into),
                _ => None,
            },
            _ => None,
        };
        if let Some(rp) = runpath {
            for entry in split_colon(&rp) {
                let has = |v: &[u8]| entry.windows(v.len()).any(|w| w == v);
                let refused = if has(b"$LIB") || has(b"${LIB}") {
                    Some("$LIB in DT_RUNPATH")
                } else if has(b"$PLATFORM") || has(b"${PLATFORM}") {
                    Some("$PLATFORM in DT_RUNPATH")
                } else {
                    None
                };
                if let Some(name) = refused {
                    if first {
                        let p = Problem::refused(m.arg, m.seq, RefusalClass::Feature, name, false)
                            .at(m.place.clone());
                        self.problems.push(p);
                    }
                    continue;
                }
                let origin = if origin_dir.is_empty() { &b"."[..] } else { &origin_dir[..] };
                let replaced = replace(&replace(entry, b"${ORIGIN}", origin), b"$ORIGIN", origin);
                let had_origin = replaced != entry;
                let d: Spelled = match sysroot {
                    Some(s) if !had_origin && replaced.starts_with(b"/") => {
                        search::under_sysroot(s, &replaced)
                    }
                    _ => replaced.into(),
                };
                dirs.push(d);
            }
        }
        dirs.push(if origin_dir.is_empty() { (*b".").into() } else { origin_dir });
        dirs.extend(self.cli_dirs.iter().cloned());
        dirs
    }

    // ---------------------------------------------------------- final

    fn finish_checks(&mut self) {
        // §3.1 rule 4, when no input failed: a file that cannot be opened
        // or is refused may be the one that names the target, and its
        // error is the one to report (WP-027c ruling).
        if self.target.is_none()
            && !self.plan.inputs.is_empty()
            && !self.problems.iter().any(Problem::is_error)
        {
            self.problems.push(Problem::error(0, 0, Dk::NoTarget));
        }
        // §3.2: the scripts' OUTPUT_FORMAT, OUTPUT_ARCH, TARGET.
        if let Some(t) = &self.target {
            let machine = t.target.machine;
            let late = std::mem::take(&mut self.late);
            for (template, check) in late {
                let (ok, problem) = match &check {
                    LateCheck::Format(n) => {
                        let ok = &n[..] == bfd_name(machine).as_bytes();
                        let p = if search::is_known_bfd(n) {
                            Problem::error(
                                template.unit,
                                template.seq,
                                Dk::IncompatibleFile {
                                    expected: bfd_name(machine).to_owned(),
                                    found: text(n),
                                },
                            )
                            .note(format!("the target was set by {}", t.by))
                        } else {
                            Problem::refused(
                                template.unit,
                                template.seq,
                                RefusalClass::FileFormat,
                                text(n),
                                false,
                            )
                        };
                        (ok, p)
                    }
                    LateCheck::Arch(n) => (
                        &n[..] == arch_name(machine).as_bytes(),
                        Problem::error(
                            template.unit,
                            template.seq,
                            Dk::IncompatibleFile {
                                expected: arch_name(machine).to_owned(),
                                found: text(n),
                            },
                        )
                        .note(format!("the target was set by {}", t.by)),
                    ),
                    LateCheck::Target(n) => (
                        &n[..] == bfd_name(machine).as_bytes(),
                        Problem::refused(
                            template.unit,
                            template.seq,
                            RefusalClass::FileFormat,
                            text(n),
                            false,
                        ),
                    ),
                };
                if !ok {
                    let mut p = problem;
                    p.place = template.place;
                    let notes = std::mem::take(&mut p.context);
                    p.context = template.context;
                    p.context.extend(notes);
                    self.problems.push(p);
                }
            }
        }
        self.exclude_libs();
    }

    /// §10.3 `exclude_libs`.
    fn exclude_libs(&mut self) {
        let plan = self.plan;
        let mut all = false;
        let mut names: Vec<(u32, Vec<u8>)> = Vec::new();
        for s in &plan.symbols {
            if s.role != SymbolRole::ExcludeLibs {
                continue;
            }
            for n in s.value.as_bytes().split(|&b| b == b',' || b == b':') {
                if n.is_empty() {
                    continue;
                }
                if n == b"ALL" {
                    all = true;
                }
                names.push((s.arg, n.to_vec()));
            }
        }
        if names.is_empty() {
            return;
        }
        for (arg, n) in &names {
            if n == b"ALL" {
                continue;
            }
            let gnu_only = self.archives.iter().any(|a| {
                let last = file_name(&a.path);
                last.strip_suffix(b".a") == Some(&n[..]) && last != &n[..]
            });
            if gnu_only {
                self.problems.push(
                    Problem::error(
                        *arg,
                        0,
                        Dk::InvalidOptionValue {
                            option: "--exclude-libs".to_owned(),
                            value: text(n),
                        },
                    )
                    .at(Place::argument(format!("--exclude-libs {}", text(n))))
                    .note("GNU ld matches an archive name without its .a; lld and mold do not"),
                );
            }
        }
        for f in &mut self.files {
            let Some(mem) = &f.member else { continue };
            let last = file_name(&self.archives[mem.archive as usize].path);
            if all || names.iter().any(|(_, n)| &n[..] == last) {
                f.state.exclude_libs = true;
            }
        }
    }

    fn finish(mut self) -> Loaded<'s> {
        crate::problem::sort(&mut self.problems);
        Loaded {
            target: self.target,
            files: self.files,
            archives: self.archives,
            link: self.link,
            problems: self.problems,
            waves: 0,
            timing: Timing::default(),
        }
    }
}

/// The D-43a row of a shared object that binds a definition locally
/// (`spec/dyn.md` D-43a): `DT_SYMBOLIC` or `DF_SYMBOLIC`, else the
/// `NONPREEMPTIBLE` rows of its `.note.mason` table (`spec/odr.md` O-51,
/// O-60), else none. A table that does not parse binds every definition
/// (mortar cannot tell which). Nothing is allocated for a shared object
/// that binds nothing locally.
fn local_binding(file: FileId, s: &SharedFacts<'_>) -> Option<LocalBinding> {
    use mortar_elf::consts::DF_SYMBOLIC;
    use mortar_meta::table::{NONPREEMPTIBLE, Table};
    let binds = if s.symbolic || s.flags & DF_SYMBOLIC != 0 {
        BindsLocally::Symbolic
    } else {
        match Table::parse(s.mason_note?) {
            Ok(table) => {
                let mut rows: Vec<u32> = table
                    .iter()
                    .filter(|row| row.flags & NONPREEMPTIBLE != 0)
                    .map(|row| row.dynsym)
                    .collect();
                if rows.is_empty() {
                    return None;
                }
                rows.sort_unstable();
                rows.dedup();
                BindsLocally::Rows(rows.into_boxed_slice())
            }
            Err(e) => BindsLocally::Unreadable(e.reason().into_boxed_str()),
        }
    };
    let loads = s
        .loads
        .iter()
        .map(|l| LoadSpan { vaddr: l.vaddr, memsz: l.memsz, writable: l.writable })
        .collect();
    Some(LocalBinding { file, binds, loads, relro: s.relro })
}

/// [`Walk::member_facts`].
enum MemberGot<'c, 's> {
    Pending,
    Facts { facts: &'c Facts<'s>, blob: Option<Blob>, base: u64, len: u64, thin: Option<Spelled> },
    Failed(MemberError),
}

enum MemberError {
    Read(Spelled, ReadError),
    Malformed(String),
}

/// A thin member's path: relative to the archive's directory unless
/// absolute (§7.2).
fn thin_path(archive: &[u8], stored: &[u8]) -> Spelled {
    if search::is_absolute(stored) {
        return stored.into();
    }
    join(dir_of(archive), stored)
}

fn machine_of_ident(i: ElfIdent) -> Option<Machine> {
    match (i.class, i.e_machine, i.big_endian) {
        (2, EM_AARCH64, false) => Some(Machine::AArch64),
        (2, EM_X86_64, false) => Some(Machine::X86_64),
        (1, EM_386, false) => Some(Machine::I386),
        _ => None,
    }
}

fn argument_text(spec: &InputSpec) -> String {
    match spec {
        InputSpec::Path(p) => p.to_string_lossy().into_owned(),
        InputSpec::Library(n) => format!("-l{}", n.to_string_lossy()),
        InputSpec::LibraryFile(n) => format!("-l:{}", n.to_string_lossy()),
        InputSpec::JustSymbols(n) => format!("-R {}", n.to_string_lossy()),
    }
}

fn script_place(f: &ScriptFile<'_>, offset: usize) -> Place {
    let (line, column) = mortar_script::position(f.bytes, offset);
    Place::script(text(&f.path), line, column)
}

fn ambiguous(unit: u32, at: u64, spelling: &str, concatenated: &[u8], joined: &[u8]) -> Problem {
    Problem::error(unit, at, Dk::AmbiguousSysrootPath { spelling: spelling.to_owned() })
        .note(format!("GNU ld and mold read it as {}", text(concatenated)))
        .note(format!("lld reads it as {}", text(joined)))
        .note("write the directory with a leading '/' after '='")
}

/// §13 for a read error.
fn read_problem(unit: u32, at: u64, path: &[u8], e: &ReadError) -> Problem {
    let p = text(path);
    match e {
        ReadError::Missing => Problem::error(
            unit,
            at,
            Dk::CannotOpenFile { path: p, reason: "No such file or directory".to_owned() },
        ),
        ReadError::Os { reason, .. } => {
            Problem::error(unit, at, Dk::CannotOpenFile { path: p, reason: reason.clone() })
        }
        ReadError::Directory => Problem::error(
            unit,
            at,
            Dk::CannotOpenFile {
                path: p,
                reason: format!("Is a directory; {}", mortar_diag::NOT_RECOGNIZED),
            },
        ),
        ReadError::NotRegular(what) => {
            Problem::error(unit, at, Dk::Malformed { what: (*what).to_owned() })
        }
        ReadError::Changed => Problem::error(unit, at, Dk::InputChanged),
        ReadError::TooLarge(name) => {
            Problem::refused(unit, at, RefusalClass::Feature, *name, false)
        }
        ReadError::OutOfRange => Problem::error(
            unit,
            at,
            Dk::Malformed { what: "read past the end of the file".to_owned() },
        ),
    }
}

/// §2.1, §2.2, §2.4 for a file or member the bytes condemn.
fn bad_problem(unit: u32, at: u64, b: &Bad) -> Problem {
    match b {
        Bad::Refused { class, name, note, gnu_phrase } => {
            let p = Problem::refused(unit, at, *class, name.clone(), *gnu_phrase);
            match note {
                Some(n) => p.note(*n),
                None => p,
            }
        }
        Bad::Malformed(t) => {
            Problem::new(unit, at, Severity::Error, Dk::Malformed { what: t.clone() })
        }
    }
}

fn replace(hay: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(hay.len());
    let mut i = 0;
    while i < hay.len() {
        if hay[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(hay[i]);
            i += 1;
        }
    }
    out
}

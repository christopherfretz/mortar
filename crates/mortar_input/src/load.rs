//! `load` (§1): the walk (§10.1), its diagnostics handed to the
//! collector (§13), and `Inputs` built from its rows: the files, the
//! symbol and COMDAT keys numbered by first mention (`spec/model.md`
//! §2.3), the archives and the per-link part (§10.4).

use std::collections::HashMap;
use std::path::PathBuf;

use mortar_core::Column;
use mortar_core::id::{ArchiveId, ComdatId, FileId, IdRange, Span, SymbolId};
use mortar_core::inputs::{
    Archive, ArchiveKind, ClosureEdge, Comdat, ContentHash, Directive, DirectiveKind, EdgeOutcome,
    Errno, File, FileBytes, GroupOrdinal, InputFlags, Inputs, LinkInputs, LinkTarget, Member,
    Opened, OpenedKind, Origin, Probe, ProbeOutcome, Reader, ScriptEntry, SearchRef, SonameUnit,
    StoreRef, SymbolKey, TargetName,
};
use mortar_core::intern::{self, SymName, TooManyKeys};
use mortar_core::par::Pool;
use mortar_core::plan::Plan;
use mortar_diag::{Collector, Diagnostic, Key, Kind, RefusalClass};

use crate::problem::PHASE;
use crate::steps::Steps;
use crate::store::Store;
use crate::walk::{self, FileRow, Loaded, Options, RowKind, WalkEnv};

/// What the phase reads besides the `Plan` (§4.7): the working directory,
/// and the pool its parallel parts run on (the thread count of
/// `--threads`, and the scheduling seed the tests of §12 set).
#[derive(Clone, Debug)]
pub struct LoadEnv {
    pub cwd: PathBuf,
    pub pool: Pool,
}

/// §1: finds, reads and loads every file of the link. Every diagnostic of
/// the phase goes to `diag` (§13, phase 20); `None` when one of them is an
/// error (`spec/diag.md` §7). The `Inputs` borrow from `store`.
pub fn load<'s>(
    plan: &Plan,
    env: &LoadEnv,
    store: &'s Store,
    diag: &Collector,
) -> Option<Inputs<'s>> {
    load_steps(plan, env, store, diag, &mut Steps::default())
}

/// [`load`], recording where the time went into `steps`.
pub fn load_steps<'s>(
    plan: &Plan,
    env: &LoadEnv,
    store: &'s Store,
    diag: &Collector,
    steps: &mut Steps,
) -> Option<Inputs<'s>> {
    steps.restart();
    let loaded = walk::walk(
        plan,
        &WalkEnv { cwd: env.cwd.clone() },
        store,
        &env.pool,
        Options { closure: true },
    );
    let t = loaded.timing;
    for (name, d) in [
        ("walk.passes", t.walk),
        ("walk.stat", t.stat),
        ("walk.canon", t.canon),
        ("walk.read", t.read),
        ("walk.facts", t.facts),
        ("walk.hash", t.hash),
    ] {
        steps.time(name, d);
    }
    steps.step("walk");
    let st = store.stats();
    for (name, ns) in
        ["read.a", "read.b", "read.c", "read.d", "read.entries"].iter().zip(st.wave_ns)
    {
        steps.time(name, std::time::Duration::from_nanos(ns));
    }
    steps.count("reads", st.reads);
    steps.count("bytes_read", st.bytes);
    steps.count("bytes_resident", st.resident);
    steps.count("opens", st.opens);
    steps.count("waves", u64::from(loaded.waves));
    steps.count("files", loaded.files.len() as u64);
    steps.count("members", loaded.files.iter().filter(|f| f.member.is_some()).count() as u64);
    steps.count("globals", loaded.files.iter().map(|f| u64::from(f.globals)).sum());
    steps.count("groups", loaded.files.iter().map(|f| u64::from(f.groups)).sum());
    steps.count("sections", loaded.files.iter().map(|f| u64::from(f.sections)).sum());
    for p in &loaded.problems {
        diag.emit(p.key(), p.diagnostic());
    }
    if loaded.failed() {
        return None;
    }
    let Some(choice) = &loaded.target else {
        // No input names a target: there is nothing to load either.
        diag.emit(Key::new(PHASE, 0, 0), Diagnostic::error(Kind::NoTarget));
        return None;
    };
    let Ok(inputs) = build(&loaded, store, &env.pool, choice.target, steps) else {
        let what = "more than 2^32 - 1 distinct symbol keys";
        diag.emit(Key::new(PHASE, 0, 0), Diagnostic::refused(RefusalClass::Feature, what));
        return None;
    };
    let checked = inputs.check();
    steps.step("check");
    steps.count("symbols", u64::from(inputs.symbols.len()));
    steps.count("comdats", u64::from(inputs.comdats.len()));
    if let Err(broken) = checked {
        let what = format!("the input phase built an invalid table: {broken}");
        diag.emit(Key::new(PHASE, u32::MAX, 0), Diagnostic::error(Kind::InternalError { what }));
        return None;
    }
    drop(loaded);
    steps.step("drop");
    Some(inputs)
}

/// Bytes the tables borrow, kept in the store once per distinct value.
struct Kept<'s> {
    store: &'s Store,
    seen: HashMap<Box<[u8]>, &'s [u8]>,
}

impl<'s> Kept<'s> {
    fn get(&mut self, bytes: &[u8]) -> &'s [u8] {
        if bytes.is_empty() {
            return &[];
        }
        if let Some(&kept) = self.seen.get(bytes) {
            return kept;
        }
        let kept = self.store.keep(bytes);
        self.seen.insert(bytes.into(), kept);
        kept
    }
}

/// `Inputs` from the rows of a walk that found no error.
#[allow(clippy::too_many_lines, reason = "the tables of `Inputs` in order, each a few lines")]
fn build<'s>(
    loaded: &Loaded<'s>,
    store: &'s Store,
    pool: &Pool,
    target: LinkTarget,
    steps: &mut Steps,
) -> Result<Inputs<'s>, TooManyKeys> {
    steps.restart();
    let mut kept = Kept { store, seen: HashMap::new() };
    let rows = &loaded.files;
    let link = &loaded.link;
    let origin = |o: &walk::Origin| -> Origin {
        match o.edge.and_then(|e| link.closure.get(e as usize)) {
            Some(edge) => Origin::Closure { needing: FileId(edge.needing), index: edge.index },
            None => Origin::Input { input: o.input, arg: o.arg, entry: o.entry },
        }
    };

    // Files: FileId 0 is the internal file (`spec/model.md` §2.1).
    let mut files: Vec<File<'s>> = Vec::with_capacity(rows.len() + 1);
    files.push(File {
        path: &[],
        member: None,
        origin: Origin::Internal,
        state: InputFlags::empty(),
        reader: Reader::Internal,
        bytes: None,
        sections: 0,
        syms: Span { start: 0, len: 0 },
        hash: ContentHash(0),
    });
    let mut at = 0u32;
    for row in rows {
        let member = row.member.as_ref().map(|m| Member {
            archive: ArchiveId(m.archive),
            name: kept.get(&m.name),
            ordinal: m.ordinal,
            offset: m.header_offset,
            thin: m.thin_path.as_ref().map(|p| kept.get(p)),
        });
        files.push(File {
            path: kept.get(&row.path),
            member,
            origin: origin(&row.origin),
            state: flags(row),
            reader: row.reader,
            bytes: Some(FileBytes {
                blob: Store::model_blob(row.blob),
                base: row.base,
                len: row.len,
            }),
            sections: if row.kind == RowKind::Object { row.sections } else { 0 },
            syms: Span { start: at, len: row.globals },
            hash: row.hash,
        });
        at = at.saturating_add(row.globals);
    }
    steps.step("build.files");

    // Symbol keys and COMDAT signatures, numbered by first mention.
    let counts: Vec<u32> = std::iter::once(0).chain(rows.iter().map(|r| r.globals)).collect();
    let symbols = intern::number::<SymName<'s>, SymbolId, _, _>(pool, &counts, |u| {
        u.checked_sub(1).and_then(|i| rows.get(i)).map_or(Keys::None, symbol_keys)
    })?;
    steps.step("build.intern_symbols");
    let counts: Vec<u32> = std::iter::once(0).chain(rows.iter().map(|r| r.groups)).collect();
    let comdats = intern::number::<&'s [u8], ComdatId, _, _>(pool, &counts, |u| {
        u.checked_sub(1).and_then(|i| rows.get(i)).map_or(Keys::None, signatures)
    })?;
    steps.step("build.intern_comdats");
    let symbol_rows: Vec<SymbolKey<'s>> = symbols
        .names
        .iter()
        .map(|(id, key)| SymbolKey {
            name: key.name,
            version: key.version,
            hash: symbols.names.hash(id),
        })
        .collect();
    let comdat_rows: Vec<Comdat<'s>> =
        comdats.names.iter().map(|(_, name)| Comdat { name }).collect();

    steps.step("build.key_rows");
    // Archives: each mention's members are contiguous files.
    let mut archives: Vec<Archive<'s>> = Vec::with_capacity(loaded.archives.len());
    for (a, row) in loaded.archives.iter().enumerate() {
        let mut ids = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.member.as_ref().is_some_and(|m| m.archive as usize == a))
            .map(|(i, _)| file_id(i));
        let members = if let Some(first) = ids.next() {
            IdRange::new(first, FileId(ids.next_back().unwrap_or(first).0 + 1))
        } else {
            let next = rows.iter().position(|r| r.opened > row.opened);
            IdRange::empty(next.map_or(FileId(files_len(rows)), file_id))
        };
        archives.push(Archive {
            path: kept.get(&row.path),
            kind: if row.thin { ArchiveKind::Thin } else { ArchiveKind::Gnu },
            origin: origin(&row.origin),
            members,
        });
    }

    steps.step("build.archives");
    let link = link_inputs(loaded, &mut kept, &origin);
    steps.step("build.link");
    Ok(Inputs {
        target,
        files: column(files),
        sym_map: symbols.map.into_boxed_slice(),
        symbols: column(symbol_rows),
        comdats: column(comdat_rows),
        archives: column(archives),
        link,
        store: StoreRef(store),
    })
}

fn file_id(row: usize) -> FileId {
    FileId(u32::try_from(row + 1).unwrap_or(u32::MAX))
}

fn files_len(rows: &[FileRow<'_>]) -> u32 {
    u32::try_from(rows.len() + 1).unwrap_or(u32::MAX)
}

/// A column of at most 2^32 - 1 rows: the walk numbers files, archives
/// and keys in `u32`, so none can have more.
fn column<I: mortar_core::id::Id, T>(rows: Vec<T>) -> Column<I, T> {
    let mut out = Column::new();
    for row in rows {
        if out.push(row).is_err() {
            break;
        }
    }
    out
}

fn flags(row: &FileRow<'_>) -> InputFlags {
    let s = &row.state;
    let mut f = InputFlags::empty();
    for (on, flag) in [
        (s.as_needed, InputFlags::AS_NEEDED),
        (s.whole_archive, InputFlags::WHOLE_ARCHIVE),
        (s.lazy && row.member.is_none(), InputFlags::LAZY),
        (s.link_static, InputFlags::STATIC),
        (s.exclude_libs, InputFlags::EXCLUDE_LIBS),
        (s.copy_dt_needed, InputFlags::COPY_DT_NEEDED),
    ] {
        if on {
            f.insert(flag);
        }
    }
    f
}

/// A file's keys, streamed to the interner: an object's from the names
/// and signatures its facts pass read (§6), a shared object's from its
/// table and version slots.
enum Keys<'r, 's, K> {
    None,
    Names(std::slice::Iter<'r, &'s [u8]>),
    Signatures(std::slice::Iter<'r, &'s [u8]>),
    Shared(Box<dyn Iterator<Item = K> + 'r>),
}

impl<'s> Iterator for Keys<'_, 's, SymName<'s>> {
    type Item = SymName<'s>;

    #[inline]
    fn next(&mut self) -> Option<SymName<'s>> {
        match self {
            Keys::Names(names) => names.next().map(|n| SymName::parse(n)),
            Keys::Shared(keys) => keys.next(),
            Keys::None | Keys::Signatures(_) => None,
        }
    }
}

impl<'s> Iterator for Keys<'_, 's, &'s [u8]> {
    type Item = &'s [u8];

    #[inline]
    fn next(&mut self) -> Option<&'s [u8]> {
        match self {
            Keys::Signatures(signatures) => signatures.next().copied(),
            Keys::Shared(keys) => keys.next(),
            Keys::None | Keys::Names(_) => None,
        }
    }
}

/// The keys of a file's non-local symbol-table entries, in table order
/// (§6, §8.1; the facts pass checked every name).
fn symbol_keys<'r, 's>(row: &'r FileRow<'s>) -> Keys<'r, 's, SymName<'s>> {
    match row.reader {
        Reader::Object(_) => Keys::Names(row.keys.names.iter()),
        Reader::Shared(dso) => {
            let Ok(Some(table)) = dso.symbols() else { return Keys::None };
            let versions = &row.versions;
            Keys::Shared(Box::new(
                table
                    .globals()
                    .map(move |(i, s)| versions.key(&dso, i.0, table.name(&s).unwrap_or_default())),
            ))
        }
        Reader::Internal => Keys::None,
    }
}

/// The effective signatures of a file's `SHT_GROUP` sections, in section
/// order (§6; `spec/resolve.md` §6.1), as the facts pass read them.
fn signatures<'r, 's>(row: &'r FileRow<'s>) -> Keys<'r, 's, &'s [u8]> {
    match row.reader {
        Reader::Object(_) => Keys::Signatures(row.keys.signatures.iter()),
        Reader::Shared(_) | Reader::Internal => Keys::None,
    }
}

/// §10.4 from the walk's per-link table.
#[allow(clippy::too_many_lines, reason = "one conversion per column of §10.4")]
fn link_inputs<'s>(
    loaded: &Loaded<'s>,
    kept: &mut Kept<'s>,
    origin: &dyn Fn(&walk::Origin) -> Origin,
) -> LinkInputs<'s> {
    let link = &loaded.link;
    let rows = &loaded.files;
    let mut group: Column<FileId, Option<GroupOrdinal>> = Column::new();
    for g in std::iter::once(None).chain(link.group.iter().copied()) {
        let _ = group.push(g.map(GroupOrdinal));
    }
    let sonames: Box<[SonameUnit]> = link
        .sonames
        .iter()
        .map(|u| SonameUnit {
            file: FileId(u.file),
            mentions: u
                .mentions
                .iter()
                .filter_map(|&o| link.opened.get(o as usize))
                .map(|o| origin(&o.origin))
                .collect(),
            as_needed: u.as_needed,
            copy_dt_needed: u.copy_dt_needed,
        })
        .collect();
    let closure: Box<[ClosureEdge]> = link
        .closure
        .iter()
        .map(|e| ClosureEdge {
            needing: FileId(e.needing),
            index: e.index,
            outcome: e
                .satisfied
                .map_or(EdgeOutcome::NotFound, |f| EdgeOutcome::Satisfied(FileId(f))),
        })
        .collect();

    // The file each opened row became.
    let mut file_of: HashMap<u32, FileId> = HashMap::new();
    for (i, row) in rows.iter().enumerate() {
        let own = row.member.as_ref().is_none_or(|m| m.thin_path.is_some());
        if own {
            file_of.entry(row.opened).or_insert(file_id(i));
        }
    }
    let archive_of: HashMap<u32, ArchiveId> = loaded
        .archives
        .iter()
        .enumerate()
        .map(|(a, row)| (row.opened, ArchiveId(u32::try_from(a).unwrap_or(u32::MAX))))
        .collect();
    let unit_of: HashMap<u32, FileId> = link
        .sonames
        .iter()
        .flat_map(|u| u.mentions.iter().map(move |&o| (o, FileId(u.file))))
        .collect();
    let opened: Box<[Opened<'s>]> = link
        .opened
        .iter()
        .enumerate()
        .map(|(i, o)| {
            let i = u32::try_from(i).unwrap_or(u32::MAX);
            let kind = match o.kind {
                walk::OpenedKind::Object | walk::OpenedKind::ThinMember => {
                    file_of.get(&i).map_or(OpenedKind::Ignored, |&f| OpenedKind::Object(f))
                }
                walk::OpenedKind::Shared { loaded: true } => {
                    file_of.get(&i).map_or(OpenedKind::Ignored, |&f| OpenedKind::Shared(f))
                }
                walk::OpenedKind::Shared { loaded: false } => {
                    unit_of.get(&i).map_or(OpenedKind::Ignored, |&f| OpenedKind::SharedNotLoaded(f))
                }
                walk::OpenedKind::Archive => {
                    archive_of.get(&i).map_or(OpenedKind::Ignored, |&a| OpenedKind::Archive(a))
                }
                walk::OpenedKind::Script | walk::OpenedKind::Include => OpenedKind::Script,
                walk::OpenedKind::Empty => OpenedKind::Empty,
                walk::OpenedKind::NestedArchive | walk::OpenedKind::Other => OpenedKind::Ignored,
            };
            Opened { path: kept.get(&o.path), kind, origin: origin(&o.origin) }
        })
        .collect();
    let probes: Box<[Probe<'s>]> = link
        .probes
        .iter()
        .map(|p| Probe {
            path: kept.get(&p.path),
            search: match p.search {
                walk::SearchRef::Input(i) => SearchRef::Input(i),
                walk::SearchRef::ScriptEntry(e) => SearchRef::Entry(e),
                walk::SearchRef::Closure(e) => SearchRef::Edge(e),
            },
            outcome: match &p.outcome {
                walk::ProbeOutcome::Absent => ProbeOutcome::Absent,
                walk::ProbeOutcome::Skipped(walk::SkipWhy::Elf(ident)) => {
                    ProbeOutcome::Skipped(TargetName::Elf {
                        machine: ident.e_machine,
                        class: ident.class,
                    })
                }
                walk::ProbeOutcome::Skipped(walk::SkipWhy::Format(name)) => {
                    ProbeOutcome::Skipped(TargetName::Format(kept.get(name)))
                }
                walk::ProbeOutcome::Found(o) => ProbeOutcome::Found(*o),
                // Not an error of the system's (a FIFO, a file that
                // changed): no errno, 0.
                walk::ProbeOutcome::Error(e) => ProbeOutcome::Error(Errno(e.unwrap_or(0))),
            },
        })
        .collect();
    let script_entries: Box<[ScriptEntry]> = link
        .script_entries
        .iter()
        .map(|e| ScriptEntry { script: e.script, offset: e.offset, parent: e.parent })
        .collect();
    let directives: Box<[Directive<'s>]> = link
        .directives
        .iter()
        .map(|d| Directive {
            kind: match d.command {
                "OUTPUT" => DirectiveKind::Output,
                "ENTRY" => DirectiveKind::Entry,
                _ => DirectiveKind::Extern,
            },
            value: kept.get(&d.value),
            script: d.script,
            offset: d.offset,
        })
        .collect();
    let rpath_from_r: Box<[(u32, &'s [u8])]> =
        link.rpath_from_r.iter().map(|(arg, dir)| (*arg, kept.get(dir))).collect();
    let local_binding = link.local_binding.clone().into_boxed_slice();
    LinkInputs {
        group,
        sonames,
        closure,
        opened,
        probes,
        script_entries,
        directives,
        rpath_from_r,
        local_binding,
    }
}

/// What `--dependency-file` lists and `-t` prints (§10.4): the distinct
/// found paths of `opened`, in its order.
#[must_use]
pub fn dependency_paths<'s>(link: &LinkInputs<'s>) -> Vec<&'s [u8]> {
    let mut seen = std::collections::HashSet::new();
    link.opened.iter().map(|o| o.path).filter(|p| seen.insert(*p)).collect()
}

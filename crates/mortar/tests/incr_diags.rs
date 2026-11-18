//! U-9's named tests (`spec/incr.md` IN-75a; WP-221): an update's
//! warnings equal the full link's standard error of the same inputs.
//!
//! Each test links real objects twice in full (the state's link A and the
//! update's inputs B), makes A's `DIAGS` rows by the one row function
//! (`mortar_incr::update::diags::row`, as capture will), describes the
//! change A -> B as the update's earlier steps would (`ChangedFiles`,
//! `ResolveDelta::affected`, and as `fresh` the warnings those steps raise
//! over the files they read, taken from B's full link and cut to those
//! files), and checks that `diags` + `emit` print B's full link's standard
//! error byte for byte, and store what B's capture stores. GCC objects, on
//! AArch64 and x86-64. The full links run in this process
//! (`mortar::run`); the state is format 4's capture as
//! `MemoryState::from_format4` gives it (FILES, SECTIONS, SYMBOLS,
//! `KEY_REFS`).

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::Path;

use mortar::front::{Env, Front, front};
use mortar::timing::Timer;
use mortar::{Outcome, run as link_run};
use mortar_core::par::Pool;
use mortar_core::{FileId, SectionId, SymbolId};
use mortar_diag::{Collector, Context, Diagnostic, Key, Place, Policy, Relation, Style};
use mortar_elf::consts::SHT_RELA;
use mortar_incr::capture::{Keys, Link, capture};
use mortar_incr::format::{SectionRow, Str, Strings};
use mortar_incr::update::diags::{DiagRef, DiagRow, Subject, diags, emit, row};
use mortar_incr::update::odr::OdrUpdate;
use mortar_incr::update::resolve::ResolveDelta;
use mortar_incr::update::rows::{FileRow, file_flags};
use mortar_incr::update::testing::MemoryState;
use mortar_incr::update::view::{Change, ChangedFile, ChangedFiles, OdrRows};
use mortar_input::store::{Store, StorePolicy};
use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, run, tools_required};

const MAIN: &str = "int main(void) { return 0; }\n";

/// glibc's `link_warning` form: `.gnu.warning.<sym>`, its name ending in
/// an assembler comment so the section is not allocated.
fn warned(name: &str, text: &str) -> String {
    format!(
        "int {name}(int x) {{ return x + 1; }}\n\
         static const char w_{name}[] __attribute__((used, section(\".gnu.warning.{name}\\n\\t#\"))) = \"{text}\";\n"
    )
}

/// A file that calls each of `names` (declared here).
fn caller(fn_name: &str, names: &[&str]) -> String {
    let decls = names.iter().map(|n| ["int ", n, "(int);\n"].concat()).collect::<String>();
    let calls =
        names.iter().enumerate().map(|(i, n)| [" + ", n, "(", &i.to_string(), ")"].concat());
    format!("{decls}int {fn_name}(void) {{ return 0{}; }}\n", calls.collect::<String>())
}

fn must(program: &Path, args: &[OsString]) {
    let o = run(program, args);
    assert!(o.success(), "{} {args:?}: {}", program.display(), o.stderr_text());
}

struct Fixture<'a> {
    scratch: &'a Scratch,
    arch: Arch,
    gcc: std::path::PathBuf,
    ar: std::path::PathBuf,
}

impl Fixture<'_> {
    /// `name.o` from C (or C++ when `name` ends in `.cc`) source `src`.
    fn object(&self, name: &str, src: &str) -> String {
        let (stem, cxx) = name.strip_suffix(".cc").map_or((name, false), |s| (s, true));
        let source = self.scratch.write(&format!("{stem}{}", if cxx { ".cc" } else { ".c" }), src);
        let obj = format!("{stem}.o");
        let mut args: Vec<OsString> =
            ["-O0", "-fno-pic", "-fno-asynchronous-unwind-tables"].map(OsString::from).to_vec();
        if cxx {
            args.extend(["-x", "c++", "-fno-exceptions"].map(OsString::from));
        }
        args.extend([
            "-fno-stack-protector".into(),
            "-c".into(),
            source.into_os_string(),
            "-o".into(),
            self.scratch.join(&obj).into_os_string(),
        ]);
        must(&self.gcc, &args);
        obj
    }

    fn archive(&self, name: &str, members: &[&str]) {
        let _ = std::fs::remove_file(self.scratch.join(name));
        let mut args: Vec<OsString> = vec!["rcs".into(), self.scratch.join(name).into_os_string()];
        args.extend(members.iter().map(|m| self.scratch.join(m).into_os_string()));
        must(&self.ar, &args);
    }

    /// The link line of `inputs` (in the scratch directory).
    fn line(&self, inputs: &[&str], extra: &[&str]) -> Vec<OsString> {
        let emulation = match self.arch {
            Arch::Aarch64 => "-maarch64linux",
            _ => "-melf_x86_64",
        };
        let mut args: Vec<OsString> =
            [emulation, "-static", "-e", "main"].map(OsString::from).to_vec();
        args.extend(extra.iter().map(OsString::from));
        args.extend(inputs.iter().map(|i| self.scratch.join(i).into_os_string()));
        args.extend(["-o".into(), self.scratch.join("out").into_os_string()]);
        args
    }
}

fn fixture(scratch: &Scratch, arch: Arch) -> Option<Fixture<'_>> {
    Some(Fixture { scratch, arch, gcc: arch.tool("gcc")?, ar: arch.tool("ar")? })
}

/// One warning of a full link, with what capture knows of it: its key,
/// its model, the key it is about and the file holding its
/// `.gnu.warning.<key>` (a reference-site warning), its references with
/// their places (every one: the fixtures list all), the files it names.
#[derive(Debug, Clone)]
struct Captured {
    key: Key,
    d: Diagnostic,
    symbol: Option<Vec<u8>>,
    holder: Option<FileId>,
    refs: Vec<(DiagRef, Place)>,
    files: Vec<FileId>,
}

/// A full link: its standard error and status, its warnings as capture
/// sees them, its state's rows.
struct Full {
    stderr: String,
    failed: bool,
    warnings: Vec<Captured>,
    state: MemoryState,
}

const PROG: &str = "mortar";

fn front_of<'s>(args: &[OsString], cwd: &Path, store: &'s Store) -> Front<'s> {
    let parsed = mortar_cli::parse(args);
    assert!(!parsed.failed(), "{:?}", parsed.diagnostics);
    let diag = Collector::new(Policy::default());
    let f = front(
        parsed.plan,
        Env { cwd, pool: Pool::new(2), replay: None, slack: true, state5: false },
        store,
        &diag,
        &mut Timer::new(false),
    );
    let report = diag.finish();
    match f {
        Some(f) if !report.failed() => f,
        _ => panic!("{}", report.render(&Style::plain(PROG))),
    }
}

fn display_index(f: &Front<'_>) -> BTreeMap<String, FileId> {
    (0..f.inputs.files.len())
        .map(|n| {
            let id = FileId(n);
            (format!("{:?}", f.inputs.files[id].display()), id)
        })
        .collect()
}

/// The reference at `place` as the scan numbers it: section index × 2^32
/// + the relocation's index in that section's `SHT_RELA`.
fn reference(f: &Front<'_>, files: &BTreeMap<String, FileId>, place: &Place) -> DiagRef {
    let Place::Section { file, section, offset } = place else { panic!("{place:?}") };
    let id = files[&format!("{file:?}")];
    let mortar_core::inputs::Reader::Object(o) = f.inputs.files[id].reader else { panic!() };
    let index = o
        .sections()
        .find(|s| s.name().is_ok_and(|n| n == section.as_bytes()))
        .expect("the section")
        .index()
        .0;
    let rela = o
        .sections()
        .find(|s| s.sh_type() == SHT_RELA && s.info_section().is_ok_and(|i| i.0 == index))
        .expect("its relocations");
    let record = rela
        .rela()
        .expect("RELA")
        .iter()
        .position(|r| r.r_offset == *offset)
        .expect("the relocation") as u64;
    DiagRef { file: id, seq: (u64::from(index) << 32) + record, offset: *offset }
}

#[allow(clippy::too_many_lines, reason = "one link and its capture, in order")]
fn link(fx: &Fixture<'_>, inputs: &[&str], extra: &[&str]) -> Full {
    let args = fx.line(inputs, extra);
    let invocation = link_run(PROG, &args);
    let (report, failed) = match invocation.outcome {
        Outcome::Linked { report, .. } => (report, false),
        Outcome::Failed(report) => (report, true),
        Outcome::Done(_) => panic!("no link"),
    };
    let stderr = report.render(&Style::plain(PROG));
    // The model and the state: the same link without --fatal-warnings.
    let plain_args = fx.line(inputs, &[]);
    let store = Store::new(StorePolicy::from_env());
    let f = front_of(&plain_args, fx.scratch.path(), &store);
    let Outcome::Linked { report: plain, .. } = link_run(PROG, &plain_args).outcome else {
        panic!("the plain link failed")
    };
    let files = display_index(&f);
    let mut warnings = Vec::new();
    for (key, d) in plain.warnings() {
        let mut c = Captured {
            key,
            d: d.clone(),
            symbol: None,
            holder: None,
            refs: Vec::new(),
            files: Vec::new(),
        };
        if d.id() == "gnu-warning" && key.phase == 70 {
            let mortar_diag::Kind::GnuWarning { text } = d.kind() else { panic!() };
            for &(sym, holder, section) in &f.gnu_warnings {
                let mortar_core::inputs::Reader::Object(o) = f.inputs.files[holder].reader else {
                    continue;
                };
                let data = o.section(section).and_then(|s| s.data()).expect("resident");
                let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
                if &data[..end] == text.as_bytes() {
                    c.symbol = Some(f.inputs.symbols[sym].name.to_vec());
                    c.holder = Some(holder);
                }
            }
            assert!(c.symbol.is_some(), "the key of {d:?}");
            for ctx in d.context() {
                match ctx {
                    Context::At(Relation::ReferencedBy, p) => {
                        c.refs.push((reference(&f, &files, p), p.clone()));
                    }
                    other => panic!("a fixture lists every reference: {other:?}"),
                }
            }
            assert_eq!(c.refs.first().map(|r| (r.0.file.0, r.0.seq)), Some((key.unit, key.seq)));
        } else {
            for p in d.place().into_iter().chain(d.context().iter().filter_map(|c| match c {
                Context::At(_, p) => Some(p),
                Context::Note(_) => None,
            })) {
                let (Place::File(file) | Place::Section { file, .. } | Place::Symbol { file, .. }) =
                    p
                else {
                    continue;
                };
                c.files.extend(files.get(&format!("{file:?}")).copied());
            }
        }
        warnings.push(c);
    }
    // A warned key no reference reaches: its dormant row (ruling 5's
    // "message from the row" when a later update adds a reference in a
    // file it reads while the holder is unchanged; WP-221 §2.4).
    for &(sym, holder, section) in &f.gnu_warnings {
        let name = f.inputs.symbols[sym].name.to_vec();
        if warnings.iter().any(|c| c.symbol.as_ref() == Some(&name)) {
            continue;
        }
        let mortar_core::inputs::Reader::Object(o) = f.inputs.files[holder].reader else {
            continue;
        };
        let data = o.section(section).and_then(|s| s.data()).expect("resident");
        let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
        let text = String::from_utf8_lossy(&data[..end]).into_owned();
        warnings.push(Captured {
            key: Key { phase: 70, unit: holder.0, seq: u64::MAX },
            d: Diagnostic::warning(mortar_diag::Kind::GnuWarning { text }),
            symbol: Some(name),
            holder: Some(holder),
            refs: Vec::new(),
            files: Vec::new(),
        });
    }
    let link = Link {
        inputs: &f.inputs,
        resolution: &f.resolution,
        assignment: &f.assignment,
        split: &f.merge.split,
    };
    let keys = Keys::for_capture(&link, &f.pool).expect("keys");
    let mut strings = Strings::default();
    let mut state =
        capture(&link, &keys, &f.layout, &mut strings, &|_| Some([0; 16]), f.arch.get())
            .expect("capture");
    state.strings = strings.into_bytes();
    // FILES.hash: the input file's bytes hashed, with a member's span (capture is
    // given the content hash; a rewritten archive changes all its members here).
    for (n, r) in state.files.iter_mut().enumerate() {
        let id = FileId(u32::try_from(n).expect("small"));
        if let Some(b) = f.inputs.files[id].bytes {
            let h = mortar_core::hash::hash_bytes(
                &std::fs::read(file_path(&f, id)).unwrap_or_default(),
            );
            let mut x = [0u8; 16];
            x[..8].copy_from_slice(&h.to_le_bytes());
            x[8..].copy_from_slice(&(b.base ^ b.len).to_le_bytes());
            r.hash = x;
        }
    }
    let mut state = MemoryState::from_format4(&state);
    // A member the link did not extract: `LAZY` (format 5's flag; format
    // 4 stores none).
    for (n, r) in state.files.iter_mut().enumerate() {
        let id = FileId(u32::try_from(n).expect("small"));
        if r.member == 1 && !f.resolution.file_live[id].is_live() {
            r.flags |= file_flags::LAZY;
        }
    }
    Full { stderr, failed, warnings, state }
}

/// The file an input was read from (its archive for a member).
fn file_path(f: &Front<'_>, id: FileId) -> std::path::PathBuf {
    std::path::PathBuf::from(String::from_utf8_lossy(f.inputs.files[id].path).into_owned())
}

fn s(m: &MemoryState, x: Str) -> Vec<u8> {
    m.strings[x.off as usize..(x.off + x.len) as usize].to_vec()
}

fn identity(m: &MemoryState, r: &FileRow) -> (Vec<u8>, Vec<u8>) {
    (s(m, r.path), if r.member == 1 { s(m, r.member_name) } else { Vec::new() })
}

fn live(r: &FileRow) -> bool {
    r.flags & (file_flags::LAZY | file_flags::GONE) == 0
}

fn key_name(m: &MemoryState, k: u32) -> Vec<u8> {
    s(m, m.symbols[k as usize].name)
}

/// The update A -> B as the steps before U-9 give it, in the update's id
/// space (A's rows, appeared files appended).
struct Update {
    changed: ChangedFiles,
    resolved: ResolveDelta,
    /// B's `FileId` -> the update's.
    file: Vec<FileId>,
    /// The update's rank of each of its files.
    rank: BTreeMap<FileId, u64>,
    /// The update's keys of names A's `SYMBOLS` lacks.
    names: BTreeMap<Vec<u8>, u32>,
    /// A's key count (the first new key).
    keys: u32,
}

impl Update {
    #[allow(clippy::too_many_lines, reason = "U2's and U4's outputs, emulated in one pass")]
    fn new(a: &Full, b: &Full) -> Self {
        let (ma, mb) = (&a.state, &b.state);
        let by_id: BTreeMap<_, FileId> =
            ma.files.iter().enumerate().map(|(n, r)| (identity(ma, r), FileId(n as u32))).collect();
        let mut rank: BTreeMap<FileId, u64> =
            ma.files.iter().enumerate().map(|(n, r)| (FileId(n as u32), r.rank)).collect();
        let mut changed = ChangedFiles::default();
        let mut file = Vec::new();
        let mut next = ma.files.len() as u32;
        let mut pending: Vec<FileId> = Vec::new();
        let mut last = 0u64;
        let base = ma.strings.len() as u32;
        let mut strings: Vec<u8> = Vec::new();
        let mut put = |b: &[u8]| -> Str {
            let off = base + strings.len() as u32;
            strings.extend_from_slice(b);
            Str { off, len: b.len() as u32 }
        };
        let mut entries: Vec<(FileId, Change, FileRow)> = Vec::new();
        for r in &mb.files {
            let id = identity(mb, r);
            if let Some(&u) = by_id.get(&id) {
                file.push(u);
                let old = &ma.files[u.0 as usize];
                let at = old.rank;
                for p in pending.drain(..) {
                    let mid = last + (at - last) / 2;
                    rank.insert(p, mid);
                    last = mid;
                }
                last = at;
                let change = match (live(old), live(r)) {
                    (true, true) if old.hash != r.hash => Some(Change::Changed),
                    (false, true) => Some(Change::Extracted),
                    (true, false) => Some(Change::NoLongerExtracted),
                    _ => None,
                };
                if let Some(c) = change {
                    entries.push((u, c, FileRow { rank: at, ..old.clone() }));
                }
            } else {
                let u = FileId(next);
                next += 1;
                file.push(u);
                pending.push(u);
                let row = FileRow { path: put(&id.0), member_name: put(&id.1), ..r.clone() };
                entries.push((u, Change::Appeared, row));
                changed.appeared.push(u);
            }
        }
        for p in pending {
            last += 1 << 20;
            rank.insert(p, last);
        }
        let in_b: BTreeSet<FileId> = file.iter().copied().collect();
        for (n, r) in ma.files.iter().enumerate() {
            let u = FileId(n as u32);
            if live(r) && !in_b.contains(&u) {
                let c = if r.member == 1 { Change::NoLongerExtracted } else { Change::Gone };
                entries.push((u, c, r.clone()));
            }
        }
        // IN-72 item 2a: an unchanged file whose COMDAT copy is kept or
        // stops being kept is drawn in.
        let keepers = |m: &MemoryState, map: &dyn Fn(u32) -> FileId| -> BTreeMap<Vec<u8>, FileId> {
            let mut out: BTreeMap<Vec<u8>, FileId> = BTreeMap::new();
            for r in m.sections.iter().filter(|r: &&SectionRow| {
                r.anchor_kind == 1 && r.place_chunk != mortar_incr::format::NONE
            }) {
                out.entry(s(m, r.anchor)).or_insert_with(|| map(r.file));
            }
            out
        };
        let ka = keepers(ma, &|f| FileId(f));
        let kb = keepers(mb, &|f| file[f as usize]);
        let listed: BTreeSet<FileId> = entries.iter().map(|e| e.0).collect();
        let mut drawn = BTreeSet::new();
        for (sig, k) in ka.iter().chain(&kb) {
            if ka.get(sig) != kb.get(sig) && !listed.contains(k) && (k.0 as usize) < ma.files.len()
            {
                drawn.insert(*k);
            }
        }
        for d in drawn {
            entries.push((d, Change::DrawnIn, ma.files[d.0 as usize].clone()));
        }
        entries.sort_by_key(|e| e.0);
        for (u, change, mut row) in entries {
            row.rank = rank[&u];
            changed.files.push(ChangedFile {
                file: u,
                change,
                row,
                first_section: SectionId(u32::MAX),
                sections: Vec::new(),
                keys: Vec::new(),
                first_global: 0,
                mentions: Vec::new(),
                values: Vec::new(),
                index: Vec::new(),
                edges: Vec::new(),
                meta_units: 0,
                debug: false,
                versioned: 0,
                odr: OdrRows::default(),
            });
        }
        changed.strings = strings;
        let touched: BTreeSet<FileId> = changed.files.iter().map(|c| c.file).collect();
        // IN-71: every key a touched file mentions, old version or new.
        let mut affected = BTreeSet::new();
        for r in &ma.key_refs {
            if touched.contains(&FileId(r.file)) {
                affected.insert(r.key);
            }
        }
        let mut new_keys = ma.symbols.len() as u32;
        let mut names: BTreeMap<Vec<u8>, u32> = BTreeMap::new();
        for r in &mb.key_refs {
            if touched.contains(&file[r.file as usize]) {
                let name = key_name(mb, r.key);
                let id = ma.view().find_key(&name, b"").expect("find").map_or_else(
                    || {
                        *names.entry(name).or_insert_with(|| {
                            new_keys += 1;
                            new_keys - 1
                        })
                    },
                    |k| k.0,
                );
                affected.insert(id);
            }
        }
        let resolved = ResolveDelta {
            affected: affected.into_iter().map(SymbolId).collect(),
            ..ResolveDelta::default()
        };
        Self { changed, resolved, file, rank, names, keys: ma.symbols.len() as u32 }
    }

    /// The update's key of a name (A's row, else a new key's).
    fn key(&self, a: &MemoryState, name: &[u8]) -> SymbolId {
        key_of(a, name).unwrap_or_else(|| {
            SymbolId(self.names.get(name).copied().unwrap_or(self.keys + self.names.len() as u32))
        })
    }

    fn touched(&self) -> BTreeSet<FileId> {
        self.changed.files.iter().map(|c| c.file).collect()
    }
}

/// The update's (or A's own) key of a symbol name.
fn key_of(a: &MemoryState, name: &[u8]) -> Option<SymbolId> {
    a.view().find_key(name, b"").expect("find")
}

/// A warning's row in an id space: files through `file`, keys by name in
/// `a` (the state's SYMBOLS), references kept where `keep` says.
fn row_of(
    c: &Captured,
    keyed: &dyn Fn(&[u8]) -> SymbolId,
    file: &dyn Fn(FileId) -> FileId,
    rank: &dyn Fn(FileId) -> u64,
    keep: &dyn Fn(FileId) -> bool,
    holder: bool,
) -> Option<DiagRow> {
    let mut subjects = Vec::new();
    let mut d = c.d.clone();
    let mut key = c.key;
    let mut refs = Vec::new();
    if let Some(name) = &c.symbol {
        subjects.push(Subject::Key(keyed(name)));
        if holder {
            subjects.extend(c.holder.map(|h| Subject::File(file(h))));
        }
        let mut kept: Vec<(DiagRef, Place)> = c
            .refs
            .iter()
            .map(|(r, p)| (DiagRef { file: file(r.file), ..*r }, p.clone()))
            .filter(|(r, _)| keep(r.file))
            .collect();
        kept.sort_by_key(|(r, _)| (rank(r.file), r.seq));
        let mut head = Diagnostic::warning(d.kind().clone());
        for (_, p) in kept.iter().take(3) {
            head = head.with(Relation::ReferencedBy, p.clone());
        }
        if kept.len() > 3 {
            head = head.note(format!("referenced {} more times", kept.len() - 3));
        }
        d = head;
        let dormant = Key { phase: 70, unit: c.holder.map_or(0, |h| file(h).0), seq: u64::MAX };
        key = kept.first().map_or(dormant, |(r, _)| Key { phase: 70, unit: r.file.0, seq: r.seq });
        refs = kept.into_iter().map(|(r, _)| r).collect();
    } else {
        subjects.extend(c.files.iter().map(|&f| Subject::File(file(f))));
        if [30, 40, 45, 50, 60, 70, 100].contains(&key.phase) {
            key.unit = file(FileId(key.unit)).0;
        }
    }
    row(key, &d, subjects, refs)
}

/// A's stored `DIAGS`.
fn stored(a: &Full) -> MemoryState {
    let mut m = a.state.clone();
    let rank = |f: FileId| a.state.files[f.0 as usize].rank;
    m.diags = a
        .warnings
        .iter()
        .map(|c| {
            let keyed = |n: &[u8]| key_of(&a.state, n).expect("A's key");
            row_of(c, &keyed, &|f| f, &rank, &|_| true, true).expect("a row")
        })
        .collect();
    m
}

/// The fresh rows of the update A -> B: B's warnings over the files the
/// update reads (the steps' results, cut to those files: §2.3's table).
fn fresh(a: &Full, b: &Full, u: &Update) -> Vec<DiagRow> {
    let touched = u.touched();
    let file = |f: FileId| u.file[f.0 as usize];
    let rank = |f: FileId| u.rank[&f];
    let keyed = |n: &[u8]| u.key(&a.state, n);
    let mut out = Vec::new();
    for c in &b.warnings {
        if let Some(name) = &c.symbol {
            let holder_read = c.holder.is_some_and(|h| touched.contains(&file(h)));
            let affected = u.resolved.affected.binary_search(&keyed(name)).is_ok();
            let reads = c.refs.iter().any(|(r, _)| touched.contains(&file(r.file)));
            if !(holder_read || (affected && reads)) {
                continue;
            }
            out.extend(row_of(c, &keyed, &file, &rank, &|f| touched.contains(&f), holder_read));
        } else if c.files.iter().any(|&f| touched.contains(&file(f))) {
            out.extend(row_of(c, &keyed, &file, &rank, &|_| true, true));
        }
    }
    out
}

/// The update's report of `rows` under `policy`, rendered.
fn report(m: &MemoryState, u: &Update, rows: &[DiagRow], policy: Policy) -> (String, bool) {
    let view = m.view();
    let collector = Collector::new(policy);
    emit(&view, &u.changed, &u.resolved, rows, &collector).expect("emit");
    let r = collector.finish();
    (r.render(&Style::plain(PROG)), r.failed())
}

/// One hop A -> B: the update's standard error equals B's full link's,
/// and its rows equal B's capture's (mapped into the update's id space).
fn hop(a: &Full, b: &Full, policy: Policy) -> Vec<DiagRow> {
    let m = stored(a);
    let u = Update::new(a, b);
    let view = m.view();
    let fr = fresh(a, b, &u);
    let got = diags(&view, &u.changed, &u.resolved, &OdrUpdate::default(), fr).expect("an update");
    let (text, failed) = report(&m, &u, &got, policy);
    assert_eq!(text, b.stderr, "the update's standard error");
    assert_eq!(failed, b.failed, "the update's status");
    // What the patch stores is what B's capture stores.
    let file = |f: FileId| u.file[f.0 as usize];
    let rank = |f: FileId| u.rank[&f];
    let keyed = |n: &[u8]| u.key(&a.state, n);
    let mut want: Vec<DiagRow> = b
        .warnings
        .iter()
        .filter_map(|c| row_of(c, &keyed, &file, &rank, &|_| true, true))
        .collect();
    let norm = |v: &mut Vec<DiagRow>| {
        for r in v.iter_mut() {
            r.subjects.retain(|s| matches!(s, Subject::Key(_)));
        }
        v.sort_by(|x, y| (&x.code, &x.text, x.key).cmp(&(&y.code, &y.text, y.key)));
    };
    let mut have = got.clone();
    norm(&mut want);
    norm(&mut have);
    assert_eq!(have, want, "the stored rows");
    got
}

/// A's own rows, emitted with no change, print A's standard error (the
/// capture's rows and the order's mapping).
fn replay(a: &Full) {
    let m = stored(a);
    let u = Update::new(a, a);
    assert!(u.changed.files.is_empty());
    let view = m.view();
    let got = diags(&view, &u.changed, &u.resolved, &OdrUpdate::default(), Vec::new())
        .expect("an update");
    assert_eq!(report(&m, &u, &got, Policy::default()).0, a.stderr, "A replayed");
}

/// Each 64-bit target with GCC and ar, or a named skip.
fn each(test: &str, body: &dyn Fn(&Fixture<'_>)) {
    let mut ran = 0;
    for arch in [Arch::Aarch64, Arch::X86_64] {
        let scratch = Scratch::new(&format!("incr-diags-{test}-{}", arch.name()));
        let Some(fx) = fixture(&scratch, arch) else {
            assert!(!tools_required(), "{arch:?}: gcc and ar are required");
            continue;
        };
        body(&fx);
        ran += 1;
    }
    if ran == 0 {
        mortar_testutil::named_skip(&format!("incr_diags::{test}"), "no GCC for either target");
    }
}

// spec: incr.md IN-75a (warning_repeated_by_update: an unchanged file that
// draws a warning in the full link: the update prints it too)
#[test]
fn warning_repeated_by_update() {
    each("repeated", &|fx| {
        fx.object("main", MAIN);
        fx.object("w", &warned("old_api", "old_api is deprecated"));
        fx.object("u", &caller("u", &["old_api"]));
        fx.object("x", "int x(void) { return 1; }\n");
        let inputs = ["main.o", "u.o", "x.o", "w.o"];
        let a = link(fx, &inputs, &[]);
        assert_eq!(a.warnings.len(), 1, "{}", a.stderr);
        replay(&a);
        fx.object("x", "int x(void) { return 2; }\n");
        let b = link(fx, &inputs, &[]);
        let u = Update::new(&a, &b);
        assert_eq!(u.changed.files.len(), 1, "x.o alone changed");
        assert!(fresh(&a, &b, &u).is_empty(), "nothing recomputed");
        assert_eq!(hop(&a, &b, Policy::default()).len(), 1);
    });
}

// spec: incr.md IN-75a (warning_gone_with_its_file: the file that drew the
// warning leaves the link; the warning goes with it)
#[test]
fn warning_gone_with_its_file() {
    each("gone", &|fx| {
        fx.object("main", MAIN);
        fx.object("w", &warned("old_api", "old_api is deprecated"));
        fx.object("u", &caller("u", &["old_api"]));
        fx.object("v", &caller("v", &["old_api"]));
        let a = link(fx, &["main.o", "u.o", "v.o", "w.o"], &[]);
        assert!(a.stderr.contains("v.o"), "{}", a.stderr);
        replay(&a);
        // v.o goes: the warning stays, without its line.
        let b = link(fx, &["main.o", "u.o", "w.o"], &[]);
        hop(&a, &b, Policy::default());
        // u.o goes too: no warning.
        let c = link(fx, &["main.o", "w.o"], &[]);
        assert_eq!(c.stderr, "");
        assert!(hop(&b, &c, Policy::default()).iter().all(|r| r.ref_count == 0));
        // The holder goes (with its referrer's call): nothing is left.
        fx.object("u", &caller("u", &[]));
        let d = link(fx, &["main.o", "u.o"], &[]);
        hop(&b, &d, Policy::default());
    });
}

// spec: incr.md IN-75a (gnu_warning_reference_removed: the .gnu.warning
// case, three hops; the scenario tests/incr-update/scenarios/
// gnu_warning_reference_removed is the same series for the lane)
#[test]
fn gnu_warning_reference_removed() {
    each("gnuw", &|fx| {
        fx.object("main", MAIN);
        fx.object("u", &caller("u", &["w_both"]));
        fx.object("w1", &warned("w_once", "w_once is deprecated; use w_both"));
        fx.object("w2", &warned("w_both", "w_both is referenced"));
        fx.archive("lib.a", &["w1.o", "w2.o"]);
        let inputs = ["main.o", "c.o", "u.o", "lib.a"];
        let calls = |on: bool| {
            fx.object("c", &if on { caller("c", &["w_once", "w_both"]) } else { caller("c", &[]) });
            link(fx, &inputs, &[])
        };
        let a = calls(true);
        assert_eq!(a.warnings.len(), 2, "{}", a.stderr);
        replay(&a);
        let b = calls(false);
        assert_eq!(b.warnings.len(), 1, "{}", b.stderr);
        hop(&a, &b, Policy::default());
        let c = calls(true);
        hop(&b, &c, Policy::default());
        let d = calls(false);
        hop(&c, &d, Policy::default());
    });
}

// spec: incr.md IN-75a, IN-71, IN-72 item 2a (WP-221 ruling 4: a COMDAT
// copy revived in an unchanged file draws the file in: its references to
// a warned key are read again; "referenced by" names it, as the full
// link's)
#[test]
fn gnu_warning_reference_in_revived_section() {
    each("revived", &|fx| {
        fx.object("main", MAIN);
        fx.object("w", &warned("w_api", "w_api is deprecated"));
        let inline = "extern \"C\" int w_api(int);\ninline int use_w() { return w_api(7); }\n";
        fx.object("k.cc", &format!("{inline}int k() {{ return use_w(); }}\n"));
        fx.object("o.cc", &format!("{inline}int o() {{ return use_w(); }}\n"));
        let inputs = ["main.o", "k.o", "o.o", "w.o"];
        let a = link(fx, &inputs, &[]);
        assert!(a.stderr.contains("k.o:(.text._Z5use_wv"), "{}", a.stderr);
        replay(&a);
        // The keeper's file drops its copy: o.o's is revived.
        fx.object("k.cc", "int k() { return 0; }\n");
        let b = link(fx, &inputs, &[]);
        assert!(b.stderr.contains("o.o:(.text._Z5use_wv"), "{}", b.stderr);
        let u = Update::new(&a, &b);
        let drawn: Vec<Vec<u8>> = u
            .changed
            .files
            .iter()
            .filter(|c| c.change == Change::DrawnIn)
            .map(|c| s(&a.state, c.row.path))
            .collect();
        assert!(
            drawn.len() == 1 && drawn[0].ends_with(b"/o.o"),
            "o.o drawn in: {:?}",
            drawn.iter().map(|d| String::from_utf8_lossy(d).into_owned()).collect::<Vec<_>>()
        );
        hop(&a, &b, Policy::default());
        // And back: k.o's copy is kept again, o.o's dies.
        fx.object("k.cc", &format!("{inline}int k() {{ return use_w(); }}\n"));
        let back = link(fx, &inputs, &[]);
        hop(&b, &back, Policy::default());
    });
}

// spec: incr.md IN-75a (warning_order_after_appeared_file: an appeared
// file before a warned one: the order follows the new ranks)
#[test]
fn warning_order_after_appeared_file() {
    each("order", &|fx| {
        fx.object("main", MAIN);
        fx.object("w", &format!("{}{}", warned("k1", "k1 is old"), warned("k2", "k2 is old")));
        fx.object("u1", &caller("u1", &["k1"]));
        fx.object("u2", &caller("u2", &["k2"]));
        fx.object("n", &caller("n", &["k2"]));
        let a = link(fx, &["main.o", "u1.o", "u2.o", "w.o"], &[]);
        assert!(a.stderr.find("k1 is old") < a.stderr.find("k2 is old"), "{}", a.stderr);
        replay(&a);
        let b = link(fx, &["main.o", "n.o", "u1.o", "u2.o", "w.o"], &[]);
        assert!(b.stderr.find("k2 is old") < b.stderr.find("k1 is old"), "{}", b.stderr);
        let u = Update::new(&a, &b);
        assert_eq!(u.changed.appeared.len(), 1);
        hop(&a, &b, Policy::default());
    });
}

// spec: incr.md IN-75a, diag.md §6.3 (fatal_warnings_update_fails_like_full_link:
// a state under --fatal-warnings holds no warning; an update that raises
// one fails with the full link's standard error and status; WP-221
// ruling 11: the join abandons the update before its first write)
#[test]
fn fatal_warnings_update_fails_like_full_link() {
    each("fatal", &|fx| {
        fx.object("main", MAIN);
        fx.object("w", &warned("old_api", "old_api is deprecated"));
        fx.object("c", &caller("c", &[]));
        let inputs = ["main.o", "c.o", "w.o"];
        let fatal = ["--fatal-warnings"];
        let a = link(fx, &inputs, &fatal);
        assert!(!a.failed && a.stderr.is_empty(), "{}", a.stderr);
        fx.object("c", &caller("c", &["old_api"]));
        let b = link(fx, &inputs, &fatal);
        assert!(b.failed, "the full link fails: {}", b.stderr);
        let policy = Policy { fatal_warnings: true, ..Policy::default() };
        hop(&a, &b, policy);
    });
}

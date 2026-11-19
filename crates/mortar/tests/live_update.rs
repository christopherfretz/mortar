//! WP-238 Part 3: the update's liveness walk (`mortar_incr::update::live`)
//! on real `--gc-sections` links captured in incremental mode, AArch64 and
//! x86-64.
//!
//! What is tested here, and what is not:
//! - the OLD walk and S4's check: on every scenario's state, `live` with
//!   no change gives the empty delta and the stored hash;
//! - the NEW walk's overlay: each file of the state laid over as a
//!   drawn-in file (its rows fresh, at new ids, its keys re-resolved to
//!   the new ids) gives the same live set, the same hash (S4 is id-free),
//!   and exactly the delta "the file's old ids dead, its new ids live";
//! - F11 on the reader's side: a key re-resolved to a stored section that
//!   was not its definition is the named refusal `def-unread`;
//! - mutations of the stored rows (one removed, retargeted, offset
//!   changed per edge kind; the stored hash changed): `state-corrupt` by
//!   the hash before any delta, or the row is shown redundant (the walk's
//!   result is unchanged);
//! - `--gc-sections` updates still fall back end to end with the join's
//!   `update-not-implemented (live)`.
//!
//! NOT tested here: pairs (v0, v1) of different inputs. The join builds no
//! `ChangedFiles` from new bytes yet (`crate::update::run`'s U4 is "the
//! next lines of M2a"), so `extract` and `resolve` cannot be called on a
//! real change without a second producer of the change.

mod support;

use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::PathBuf;

use mortar::front::{Env, Front, front};
use mortar::timing::Timer;
use mortar_core::incr::{IncrReason, OptionKeys, Unimplemented};
use mortar_core::{FileId, SectionId, SymbolId};
use mortar_diag::{Collector, Policy};
use mortar_incr::format::def_kind;
use mortar_incr::update::live::{StoredWalk, live, live_hash, live_why, walk_stored};
use mortar_incr::update::resolve::ResolveDelta;
use mortar_incr::update::rows::{EdgeRow, FileTable, edge_kind};
use mortar_incr::update::testing::MemoryState;
use mortar_incr::update::view::{Change, ChangedFile, ChangedFiles, OdrRows};
use mortar_testutil::{Scratch, tools_required};
use support::{Source, build_by, store, with_output};

/// A static `--gc-sections` link line in its scratch directory.
struct Line {
    _scratch: Scratch,
    label: String,
    dir: PathBuf,
    args: Vec<String>,
}

impl Line {
    fn args(&self, out: &str) -> Vec<OsString> {
        let mut a = with_output(&self.args, &self.dir.join(out));
        a.extend(["--incremental=full", "--incremental-state"].iter().map(OsString::from));
        a.push(format!("{out}.mortar-state").into());
        a
    }
}

/// The two 64-bit targets' C and C++ drivers.
const TARGETS: [(&str, &str, &str); 2] =
    [("aarch64", "gcc", "g++"), ("x86-64", "x86_64-linux-gnu-gcc", "x86_64-linux-gnu-g++")];

fn line(label: &str, cc: &str, flags: &[&str], sources: &[Source], extra: &[&str]) -> Option<Line> {
    let s = Scratch::new(&format!("wp238p3-{}", label.replace([' ', '/'], "-")));
    let Some(b) = build_by(&s, cc, flags, sources) else {
        assert!(!tools_required(), "{cc} is required");
        return None;
    };
    let mut args = b.args;
    args.push("--gc-sections".into());
    args.extend(extra.iter().map(|e| (*e).to_owned()));
    Some(Line { _scratch: s, label: label.to_owned(), dir: b.dir, args })
}

/// The state `l`'s incremental-mode link writes (decoded) with the
/// `META.live_hash` the capture wrote, and its option keys.
fn capture(l: &Line) -> (MemoryState, OptionKeys) {
    let (m, options, _) = capture_raw(l);
    (m, options)
}

/// [`capture`], and S4's hash of the FULL mark over the decoded state
/// (P0-3's capture side: the mark's live bits and `live_positions`, a link
/// section mapped to its file's first `SECTIONS` row plus its `sh_index`,
/// the reader's numbering), the value the written hash must have.
fn capture_raw(l: &Line) -> (MemoryState, OptionKeys, [u8; 16]) {
    let inv = mortar::invoke_in("mortar", &l.args("out"), &l.dir);
    assert!(
        matches!(inv.outcome, mortar::Outcome::Linked { .. }),
        "{}: the link failed: {:?}\n{}",
        l.label,
        inv.outcome,
        inv.incremental
    );
    let bytes = std::fs::read(l.dir.join("out.mortar-state")).expect("the state");
    let m = mortar_incr::state5::read::decode(&bytes)
        .unwrap_or_else(|e| panic!("{}: the state: {e:?}", l.label));
    let store = store();
    let f = front5(l, &store);
    let options = mortar_resolve::tables::option_keys(&f.plan, &f.inputs.link.directives);
    let Some(Ok((_, at))) = f.incr5.as_ref().and_then(|i| i.live_rows.as_ref()) else {
        panic!("{}: the front made no live rows", l.label)
    };
    // The state's `SECTIONS` rows are not the link's ids (file-keyed
    // ranges with slack): a link section is its file's first row plus
    // its `sh_index`.
    let r = &f.resolution;
    let row = |s: SectionId| -> u32 {
        let (file, index) = r.sh_index(s).expect("an index");
        m.file_index[file.0 as usize].ranges[FileTable::Sections as usize].first + index.0
    };
    let mut live = vec![false; m.sections.len()];
    for (i, &v) in f.liveness.live.as_slice().iter().enumerate() {
        if v {
            live[row(SectionId(u32::try_from(i).expect("ids"))) as usize] = true;
        }
    }
    let mut pieces: Vec<(SectionId, Vec<u32>)> =
        at.iter().map(|(s, v)| (SectionId(row(*s)), v.clone())).collect();
    pieces.sort();
    let full = live_hash(&m.view(), &live, &pieces)
        .unwrap_or_else(|e| panic!("{}: {}", l.label, e.report()));
    (m, options, full)
}

fn front5<'s>(l: &Line, store: &'s mortar_input::store::Store) -> Front<'s> {
    let parsed = mortar_cli::parse(&l.args("front"));
    assert!(!parsed.failed(), "{:?}", parsed.diagnostics);
    let diag = Collector::new(Policy::default());
    let pool =
        mortar_core::par::Pool::try_new(mortar_core::par::Pool::available()).expect("a pool");
    let env = Env { cwd: &l.dir, pool, replay: None, slack: true, state5: true };
    let f = front(parsed.plan, env, store, &diag, &mut Timer::new(false));
    assert!(!diag.finish().failed(), "{}", l.label);
    f.expect("the front")
}

/// The scenarios: (label, C++?, flags, sources, extra link arguments).
type Scenario = (&'static str, bool, Vec<&'static str>, Vec<Source>, Vec<&'static str>);

fn scenarios() -> Vec<Scenario> {
    let fs = vec!["-O1", "-g", "-ffunction-sections", "-fdata-sections"];
    vec![
        (
            // A function with its last caller; a dead one; merged strings,
            // one referenced only from the dead function.
            "calls",
            false,
            fs.clone(),
            vec![
                Source {
                    name: "a.c",
                    text: "extern int f1(int);\nint main(int c, char **v){ return f1(c); }\n",
                },
                Source {
                    name: "b.c",
                    text: "extern int puts(const char *);\n\
                           __attribute__((noinline)) int f2(int x){ puts(\"live\"); return x + 1; }\n\
                           __attribute__((noinline)) int f3(int x){ puts(\"dead\"); return x + 2; }\n\
                           int f1(int x){ return f2(x); }\n",
                },
            ],
            vec![],
        ),
        (
            // `-init` and a start/stop named from a non-allocated section.
            "roots",
            false,
            fs.clone(),
            vec![
                Source { name: "a.c", text: "int main(void){ return 0; }\n" },
                Source {
                    name: "u.c",
                    text: "int myinit(void){ return 40; }\n\
                           __attribute__((section(\"ureg\"), used)) const int ureg_a = 10;\n\
                           int helper(void){ return 0; }\n\
                           asm(\".pushsection .myinfo,\\\"\\\",%progbits\\n.quad __start_ureg\\n\
                           .quad __stop_ureg\\n.popsection\");\n",
                },
            ],
            vec!["-init=myinit"],
        ),
        (
            // COMDAT keepers (inline functions in two files), an FDE'd
            // function with an LSDA and a personality.
            "comdat-eh",
            true,
            vec!["-O1", "-ffunction-sections", "-fdata-sections"],
            vec![
                Source {
                    name: "a.cc",
                    text: "inline int twice(int x){ return x * 2; }\n\
                           int g(int);\n\
                           int main(int c, char **){ try { return g(twice(c)); } catch (int e) { return e; } }\n",
                },
                Source {
                    name: "b.cc",
                    text: "inline int twice(int x){ return x * 2; }\n\
                           int g(int x){ if (x > 100) throw x; return twice(x); }\n\
                           int unused(int x){ return twice(x) + 1; }\n",
                },
            ],
            vec![],
        ),
        (
            // `SHF_LINK_ORDER` metadata of a dead function and a live one.
            "linkorder",
            false,
            fs,
            vec![
                Source {
                    name: "a.c",
                    text: "extern int keepf(void);\nint main(void){ return keepf(); }\n",
                },
                Source {
                    name: "u.c",
                    text: "int keepf(void){ return 3; }\nint dropf(void){ return 4; }\n\
                           asm(\".section .meta_k,\\\"ao\\\",%progbits,.text.keepf\\n.quad 1\\n\
                           .section .meta_d,\\\"ao\\\",%progbits,.text.dropf\\n.quad 2\\n.previous\");\n",
                },
            ],
            vec![],
        ),
    ]
}

/// Every scenario's capture on both targets.
fn captured() -> Vec<(String, MemoryState, OptionKeys)> {
    let mut out = Vec::new();
    for (arch, cc, cxx) in TARGETS {
        for (label, cplus, flags, sources, extra) in scenarios() {
            let label = format!("{label} {arch}");
            let driver = if cplus { cxx } else { cc };
            let Some(l) = line(&label, driver, &flags, &sources, &extra) else { continue };
            let (m, o) = capture(&l);
            out.push((label, m, o));
        }
    }
    out
}

/// The stored hash and walk of `m`.
fn stored(m: &MemoryState, o: &OptionKeys) -> StoredWalk {
    walk_stored(&m.view(), o).unwrap_or_else(|e| panic!("the stored walk: {}", e.1))
}

/// With no change, `live` gives the empty delta and the stored hash.
#[test]
fn unchanged_update_is_empty_with_the_stored_hash() {
    for (label, m, o) in captured() {
        let d = live(&m.view(), &ChangedFiles::default(), &ResolveDelta::default(), &o)
            .unwrap_or_else(|e| panic!("{label}: {}", e.report()));
        assert!(d.now_live.is_empty() && d.now_dead.is_empty(), "{label}: {d:?}");
        assert!(d.drawn_in.is_empty() && d.pieces.is_empty(), "{label}: {d:?}");
        assert_eq!(d.live_hash, m.meta.live_hash, "{label}");
        assert_ne!(d.live_hash, [0; 16], "{label}");
    }
}

/// `m`'s file `f` laid over as drawn in: its sections at new ids from the
/// state's count, its rows fresh (keys by position in its list), every
/// key defined in it re-resolved to the new ids.
fn drawn_in(m: &MemoryState, f: FileId) -> (ChangedFiles, ResolveDelta, u32, u32, u32) {
    let range = m.file_index[f.0 as usize].ranges[FileTable::Sections as usize];
    let base = u32::try_from(m.sections.len()).expect("sections");
    let move_s = |s: u32| -> u32 {
        assert!(range.first <= s && s < range.first + range.count, "a row outside its file");
        s - range.first + base
    };
    let mut keys: Vec<SymbolId> = Vec::new();
    let mut edges = Vec::new();
    for (_, e) in m.edges.iter().filter(|(g, _)| *g == f) {
        let mut e = *e;
        e.source = move_s(e.source);
        match edge_kind::target(e.kind) {
            Some(edge_kind::Target::Key) => {
                let p = keys.iter().position(|k| k.0 == e.target).unwrap_or_else(|| {
                    keys.push(SymbolId(e.target));
                    keys.len() - 1
                });
                e.target = u32::try_from(p).expect("keys");
            }
            Some(edge_kind::Target::Section) => e.target = move_s(e.target),
            _ => {}
        }
        edges.push(e);
    }
    let mut resolved = Vec::new();
    for (k, r) in m.resolved.iter().enumerate() {
        if r.kind == def_kind::SECTION
            && range.first <= r.section
            && r.section < range.first + range.count
        {
            let mut r = *r;
            r.section = move_s(r.section);
            resolved.push((SymbolId(u32::try_from(k).expect("keys")), r));
        }
    }
    let sections = m.sections[range.first as usize..(range.first + range.count) as usize].to_vec();
    let file = ChangedFile {
        file: f,
        change: Change::DrawnIn,
        row: m.files[f.0 as usize].clone(),
        first_section: SectionId(base),
        sections,
        keys: Vec::new(),
        first_global: 0,
        mentions: Vec::new(),
        values: Vec::new(),
        index: Vec::new(),
        edges,
        meta_units: 0,
        debug: false,
        versioned: 0,
        odr: OdrRows::default(),
    };
    let changed = ChangedFiles { files: vec![file], ..ChangedFiles::default() };
    let delta = ResolveDelta {
        affected: resolved.iter().map(|r| r.0).collect(),
        key_ids: vec![(f, keys)],
        resolved,
        ..ResolveDelta::default()
    };
    (changed, delta, range.first, range.count, base)
}

/// The new walk over an overlay that changes nothing but the ids gives the
/// same live set and hash, and exactly the file's ids swapped as delta.
#[test]
fn drawn_in_overlay_equals_the_stored_walk() {
    let mut files_checked = 0;
    for (label, m, o) in captured() {
        let old = stored(&m, &o);
        // The scenario's own objects, and the six files with the most rows
        // (the walk over a `MemoryState` reads every file's rows by a scan).
        let mut by_rows: Vec<(usize, FileId)> = (0..m.files.len())
            .map(|f| FileId(u32::try_from(f).expect("files")))
            .map(|f| (m.edges.iter().filter(|(g, _)| *g == f).count(), f))
            .collect();
        by_rows.sort_unstable_by(|a, b| b.cmp(a));
        let own = |f: FileId| {
            let path = mortar_incr::update::view::StateRows::string(
                &*m.view(),
                m.files[f.0 as usize].path,
            )
            .unwrap_or_default();
            path.ends_with(b".c.o") || path.ends_with(b".cc.o")
        };
        let mut chosen: Vec<FileId> = by_rows.iter().take(6).map(|x| x.1).collect();
        chosen.extend(by_rows.iter().map(|x| x.1).filter(|&f| own(f)));
        chosen.sort_unstable();
        chosen.dedup();
        for f in chosen {
            let (changed, delta, first, count, base) = drawn_in(&m, f);
            if count == 0 {
                continue;
            }
            files_checked += 1;
            let d = live_why(&m.view(), &changed, &delta, &o)
                .unwrap_or_else(|e| panic!("{label} file {}: {}", f.0, e.1));
            assert_eq!(d.live_hash, m.meta.live_hash, "{label} file {}: S4 is id-free", f.0);
            assert!(d.drawn_in.is_empty(), "{label} file {}: {:?}", f.0, d.drawn_in);
            let was: Vec<SectionId> =
                (first..first + count).filter(|&s| old.live[s as usize]).map(SectionId).collect();
            let now: Vec<SectionId> = was.iter().map(|s| SectionId(s.0 - first + base)).collect();
            assert_eq!(d.now_dead, was, "{label} file {}", f.0);
            assert_eq!(d.now_live, now, "{label} file {}", f.0);
            let pieces: Vec<(SectionId, Vec<u32>)> = old
                .pieces
                .iter()
                .filter(|(s, _)| first <= s.0 && s.0 < first + count)
                .map(|(s, v)| (SectionId(s.0 - first + base), v.clone()))
                .collect();
            assert_eq!(d.pieces, pieces, "{label} file {}", f.0);
        }
    }
    assert!(files_checked > 0 || !tools_required(), "no file was checked");
}

/// F11 on the reader's side: a key re-resolved to a stored section that
/// was not its definition (a file whose rows were not re-read) is refused
/// by name, never a silent miss.
#[test]
fn a_definition_moved_to_an_unread_file_is_refused() {
    let mut tried = 0;
    for (label, m, o) in captured() {
        // A key defined in a section; another stored section of another
        // file to move it to.
        let Some((k, r)) = m
            .resolved
            .iter()
            .enumerate()
            .find(|(_, r)| r.kind == def_kind::SECTION)
            .map(|(k, r)| (k, *r))
        else {
            continue;
        };
        let owner = m.sections[r.section as usize].file;
        let Some(other) = m.sections.iter().position(|s| s.file != owner) else { continue };
        let mut moved = r;
        moved.section = u32::try_from(other).expect("sections");
        moved.value = 0;
        let k = SymbolId(u32::try_from(k).expect("keys"));
        let delta = ResolveDelta {
            affected: vec![k],
            resolved: vec![(k, moved)],
            ..ResolveDelta::default()
        };
        // The key must be read by the walk: root it.
        let mut o = o.clone();
        let name =
            mortar_incr::update::view::StateRows::string(&*m.view(), m.symbols[k.0 as usize].name)
                .expect("a name");
        o.undefined.push(mortar_core::incr::OptionKey { name: name.into(), arg: 0 });
        let got = live_why(&m.view(), &ChangedFiles::default(), &delta, &o);
        match got {
            Err((IncrReason::UpdateNotImplemented(Some(Unimplemented::Live)), d)) => {
                assert!(d.starts_with("def-unread:"), "{label}: {d}");
            }
            // The extra root changed the old walk: the stored hash no
            // longer agrees (the same `-u` would be options-changed).
            Err((IncrReason::StateCorrupt(_), _)) => continue,
            other => panic!("{label}: {other:?}"),
        }
        tried += 1;
    }
    assert!(tried > 0 || !tools_required(), "no input reached the reader's check");
}

/// A state made without the rows (an incremental-mode link without
/// `--gc-sections`: no `EDGES`, `META.live_hash` zero) read by an update
/// under `--gc-sections` is refused by name.
#[test]
fn a_state_without_the_live_hash_is_refused() {
    let mut tried = 0;
    for (arch, cc, _) in TARGETS {
        let (label, _, flags, sources, extra) = scenarios().swap_remove(0);
        let label = format!("{label} {arch} no-gc");
        let Some(mut l) = line(&label, cc, &flags, &sources, &extra) else { continue };
        // The `--gc-sections` link's option keys, then the state of the
        // same inputs linked without it.
        let (_, o) = capture(&l);
        l.args.retain(|a| a != "--gc-sections");
        let inv = mortar::invoke_in("mortar", &l.args("plain"), &l.dir);
        assert!(matches!(inv.outcome, mortar::Outcome::Linked { .. }), "{label}: {inv:?}");
        let bytes = std::fs::read(l.dir.join("plain.mortar-state")).expect("the state");
        let m = mortar_incr::state5::read::decode(&bytes).expect("the state");
        assert_eq!(m.meta.live_hash, [0; 16], "{label}");
        assert!(m.edges.is_empty(), "{label}");
        let e = live_why(&m.view(), &ChangedFiles::default(), &ResolveDelta::default(), &o)
            .expect_err(&label);
        assert!(e.1.starts_with("no-live-hash:"), "{label}: {}", e.1);
        assert_eq!(e.0, IncrReason::UpdateNotImplemented(Some(Unimplemented::Live)));
        tried += 1;
    }
    assert!(tried > 0 || !tools_required(), "no state made");
}

/// The outcome of `live` on a mutated state: `state-corrupt` before any
/// delta, or (`false`) the mutated rows walk to the same live set (the row
/// is redundant). Anything else fails.
fn judged(label: &str, m: &MemoryState, o: &OptionKeys, before: &StoredWalk) -> bool {
    match live_why(&m.view(), &ChangedFiles::default(), &ResolveDelta::default(), o) {
        Err((IncrReason::StateCorrupt(_), _)) => true,
        Ok(d) => {
            let after = walk_stored(&m.view(), o).map_err(|e| e.1);
            assert_eq!(after.as_ref(), Ok(before), "{label}: a mutation walked to another set");
            assert_eq!(d.live_hash, m.meta.live_hash, "{label}");
            false
        }
        Err(e) => panic!("{label}: {}", e.1),
    }
}

/// IN-63b's mutations per edge kind: one stored row removed, retargeted,
/// its offset changed; and the stored hash changed. Each is caught by the
/// hash, or the row is redundant (the table names which, per kind).
#[test]
fn mutations_are_caught_by_the_hash_or_redundant() {
    const KINDS: usize = edge_kind::LAST as usize + 1;
    // Per kind: rows seen, and (caught, redundant) per mutation.
    let mut rows = [0usize; KINDS];
    let mut table = [[(0usize, 0usize); 3]; KINDS];
    let mut hash_caught = 0;
    for (label, m, o) in captured() {
        let before = stored(&m, &o);
        let mut h = m.clone();
        h.meta.live_hash[0] ^= 1;
        assert!(
            judged(&format!("{label} hash"), &h, &o, &before),
            "{label}: a changed hash passed"
        );
        hash_caught += 1;
        for (k, row) in rows.iter_mut().enumerate() {
            let at: Vec<usize> =
                (0..m.edges.len()).filter(|&i| usize::from(m.edges[i].1.kind) == k).collect();
            *row += at.len();
            // The first and the last row of the kind.
            let picks: Vec<usize> =
                [at.first(), at.last()].into_iter().flatten().copied().collect();
            for i in picks {
                let (f, e) = m.edges[i];
                let mut tries: Vec<(usize, MemoryState)> = Vec::new();
                let mut cut = m.clone();
                cut.edges.remove(i);
                tries.push((0, cut));
                if let Some(t) = retarget(&m, f, e) {
                    let mut r = m.clone();
                    r.edges[i].1.target = t;
                    tries.push((1, r));
                }
                if edge_kind::target(e.kind) == Some(edge_kind::Target::Section) {
                    let mut r = m.clone();
                    r.edges[i].1.offset = if e.offset == 0 { u32::MAX } else { 0 };
                    tries.push((2, r));
                }
                for (what, mutated) in tries {
                    let l = format!("{label} kind {k} row {i} mutation {what}");
                    if e.kind == edge_kind::REFUSE {
                        continue;
                    }
                    if judged(&l, &mutated, &o, &before) {
                        table[k][what].0 += 1;
                    } else {
                        table[k][what].1 += 1;
                    }
                }
            }
        }
    }
    let mut out = String::from("kind rows | removed caught/redundant | retargeted | offset\n");
    for k in 0..KINDS {
        let t = table[k];
        let _ = writeln!(
            out,
            "{k:>2} {:>6} | {}/{} | {}/{} | {}/{}",
            rows[k], t[0].0, t[0].1, t[1].0, t[1].1, t[2].0, t[2].1
        );
    }
    let _ = writeln!(out, "hash changed: caught {hash_caught}");
    eprintln!("{out}");
    if let Ok(p) = std::env::var("WP238_MUTATION_TABLE") {
        std::fs::write(p, &out).expect("the table");
    }
}

/// A different target of the row's class in `f`'s file, if any.
fn retarget(m: &MemoryState, f: FileId, e: EdgeRow) -> Option<u32> {
    match edge_kind::target(e.kind)? {
        edge_kind::Target::Section => {
            let r = m.file_index[f.0 as usize].ranges[FileTable::Sections as usize];
            (r.first..r.first + r.count).find(|&s| s != e.target && s != e.source)
        }
        edge_kind::Target::Key => (0..u32::try_from(m.symbols.len()).ok()?)
            .find(|&k| k != e.target && m.resolved[k as usize].kind == def_kind::SECTION),
        _ => None,
    }
}

/// The join still refuses every `--gc-sections` update before `live`
/// (its own line; lifting it is the join's act): end to end.
#[test]
fn a_gc_update_falls_back_end_to_end() {
    let (_, cc, _) = TARGETS[0];
    let src = [
        Source { name: "a.c", text: "extern int f(void);\nint main(void){ return f(); }\n" },
        Source { name: "b.c", text: "int f(void){ return 1; }\nint g(void){ return 2; }\n" },
    ];
    let Some(l) = line("e2e", cc, &["-O1", "-ffunction-sections"], &src, &[]) else { return };
    let first = mortar::invoke_in("mortar", &l.args("out"), &l.dir);
    assert!(matches!(first.outcome, mortar::Outcome::Linked { .. }), "{:?}", first.outcome);
    // A new b.o (a new inode and content).
    let b = l.dir.join("b.c");
    std::fs::write(&b, "int f(void){ return 3; }\nint g(void){ return 2; }\n").expect("b.c");
    let gcc = mortar_testutil::tool(cc).expect("cc");
    let o = std::process::Command::new(gcc)
        .args(["-O1", "-ffunction-sections", "-c", "b.c", "-o", "b.c.o.new"])
        .current_dir(&l.dir)
        .output()
        .expect("cc");
    assert!(o.status.success());
    std::fs::rename(l.dir.join("b.c.o.new"), l.dir.join("b.c.o")).expect("rename");
    let mut args = with_output(&l.args, &l.dir.join("out"));
    args.extend(
        ["--incremental=update", "--incremental-state", "out.mortar-state"].map(OsString::from),
    );
    let second = mortar::invoke_in("mortar", &args, &l.dir);
    let text = format!("{:?}\n{}", second.outcome, second.incremental);
    assert!(text.contains("update-not-implemented (live)"), "{text}");
}

/// The capture's WRITTEN `META.live_hash` (WP-238 Part 3b) is S4's hash
/// of the full mark over the written rows, non-zero, on every scenario
/// and both targets. (Before Part 3b the capture hashed a `MemoryState`
/// whose `FILE_INDEX` had no `SECTIONS` ranges, `compact::normalize`'s,
/// and wrote zeros.)
#[test]
fn the_written_live_hash_is_the_full_marks() {
    let mut tried = 0;
    for (arch, cc, cxx) in TARGETS {
        for (label, cplus, flags, sources, extra) in scenarios() {
            let label = format!("{label} {arch} written");
            let driver = if cplus { cxx } else { cc };
            let Some(l) = line(&label, driver, &flags, &sources, &extra) else { continue };
            let (m, _, full) = capture_raw(&l);
            assert_ne!(full, [0; 16], "{label}");
            assert_eq!(m.meta.live_hash, full, "{label}: the written META.live_hash");
            tried += 1;
        }
    }
    assert!(tried > 0 || !tools_required(), "no scenario linked");
}

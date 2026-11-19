//! WP-238 Part 4: the update's liveness walk (`mortar_incr::update::live`)
//! on v0 -> v1 pairs of real static `--gc-sections` links, through the
//! join's own `ChangedFiles` and `ResolveDelta` (`update::changed::u5`),
//! AArch64 and x86-64.
//!
//! The oracle of every pair is the FULL mark of each link: `live`'s
//! `now_live` / `now_dead` must be exactly the difference of v0's mark
//! (stored ids) and v1's (the update's ids: a changed file's sections at
//! its `first_section`), `drawn_in` exactly the unchanged files with a
//! section in that difference or a split section whose marked offsets
//! changed, `pieces` exactly v1's marked offsets of those files' and the
//! changed files' live split sections. A pair the join or `live` cannot
//! take falls back BY NAME: it is listed with its words, never counted.
//!
//! - P3b-3: a `START_STOP` row that is the ONLY reason a section is live
//!   (a C-identifier section with no `SHF_GNU_RETAIN`, named by nothing
//!   but `__start_ssreg`): removed or retargeted, the walk loses the
//!   section and S4 refuses the state; v0 -> v1 removing the last live
//!   `__start_` use kills it (`now_dead`, its file `drawn_in`).
//! - P3-5: an `EDGES` offset into a split section moved onto another
//!   piece, over every kind with such rows in these states; a `REFUSE`
//!   row through the join (stored, and fresh in the changed file); the
//!   FDE whose initial location names a LOCAL non-section symbol.
//! - The operator's configuration: static C++ with `-g
//!   -ffunction-sections -fdata-sections`, `--gc-sections`: a function's
//!   last caller removed, a new call to a dead function, a COMDAT
//!   function's last use removed, a `.gcc_except_table`'s owner dying.

mod support;

use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::PathBuf;

use mortar::front::{Env, Front, front};
use mortar::timing::Timer;
use mortar_core::incr::{IncrReason, OptionKeys};
use mortar_core::{FileId, SectionId};
use mortar_diag::{Collector, Policy};
use mortar_incr::format::def_kind;
use mortar_incr::update::live::{StoredWalk, live_why, walk_stored};
use mortar_incr::update::resolve::ResolveDelta;
use mortar_incr::update::rows::{FileTable, edge_kind};
use mortar_incr::update::testing::MemoryState;
use mortar_incr::update::view::ChangedFiles;
use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, tools_required};
use support::{Source, build_by, store, with_output};

/// A static `--gc-sections` link line in its scratch directory.
struct Line {
    scratch: Scratch,
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

/// Per 64-bit target: its label, C and C++ drivers (by host), and the
/// DWARF return-address column.
fn targets() -> [(&'static str, String, String, u32); 2] {
    [Arch::Aarch64, Arch::X86_64].map(|a| {
        let aarch64 = a == Arch::Aarch64;
        (
            if aarch64 { "aarch64" } else { "x86-64" },
            a.gnu("gcc"),
            a.gnu("g++"),
            if aarch64 { 30 } else { 16 },
        )
    })
}

fn line(label: &str, cc: &str, flags: &[&str], sources: &[Source], extra: &[&str]) -> Option<Line> {
    let s = Scratch::new(&format!("wp238p4-{}", label.replace([' ', '/'], "-")));
    let Some(b) = build_by(&s, cc, flags, sources) else {
        assert!(!tools_required(), "{cc} is required");
        return None;
    };
    let mut args = b.args;
    args.push("--gc-sections".into());
    args.extend(extra.iter().map(|e| (*e).to_owned()));
    Some(Line { scratch: s, label: label.to_owned(), dir: b.dir, args })
}

/// Links `l` in incremental mode to `out` and decodes the state written.
fn link(l: &Line, out: &str) -> MemoryState {
    let inv = mortar::invoke_in("mortar", &l.args(out), &l.dir);
    assert!(
        matches!(inv.outcome, mortar::Outcome::Linked { .. }),
        "{} {out}: the link failed: {:?}\n{}",
        l.label,
        inv.outcome,
        inv.incremental
    );
    let bytes = std::fs::read(l.dir.join(format!("{out}.mortar-state"))).expect("the state");
    mortar_incr::state5::read::decode(&bytes)
        .unwrap_or_else(|e| panic!("{} {out}: the state: {e:?}", l.label))
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

/// The full mark of `f` in the update's ids (a section of a file in
/// `fresh` at `fresh`'s first section plus its `sh_index`, any other at
/// its file's first `SECTIONS` row of `m` plus its `sh_index`): the live
/// sections (sorted) and the marked offsets of the live split sections.
type Mark = (Vec<SectionId>, Vec<(SectionId, Vec<u32>)>);

fn full_mark(f: &Front<'_>, m: &MemoryState, fresh: &[(FileId, u32)]) -> Mark {
    let r = &f.resolution;
    let id = |s: SectionId| -> SectionId {
        let (file, index) = r.sh_index(s).expect("an index");
        let first = fresh.iter().find(|(g, _)| *g == file).map_or_else(
            || m.file_index[file.0 as usize].ranges[FileTable::Sections as usize].first,
            |&(_, first)| first,
        );
        SectionId(first + index.0)
    };
    let mut live = Vec::new();
    for (i, &v) in f.liveness.live.as_slice().iter().enumerate() {
        if v {
            live.push(id(SectionId(u32::try_from(i).expect("ids"))));
        }
    }
    live.sort_unstable();
    let Some(Ok((_, at))) = f.incr5.as_ref().and_then(|i| i.live_rows.as_ref()) else {
        panic!("the front made no live rows")
    };
    let mut pieces: Vec<(SectionId, Vec<u32>)> =
        at.iter().map(|(s, v)| (id(*s), v.clone())).collect();
    pieces.sort();
    (live, pieces)
}

/// A pair: v0's and v1's sources (same names), the objects v1 changes
/// (by their `.o` names), section names that must die / become live.
struct Pair {
    name: &'static str,
    cxx: bool,
    flags: Vec<&'static str>,
    v0: Vec<Source>,
    v1: Vec<Source>,
    changed: Vec<&'static str>,
    extra: Vec<&'static str>,
    dies: Vec<&'static str>,
    lives: Vec<&'static str>,
}

/// What a pair gave: counted (the sizes of `now_live`, `now_dead`,
/// `drawn_in`, `pieces`), or a named fall-back.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Counted(usize, usize, usize, usize),
    Fallback(String),
}

/// `p` on one target: v0 linked (its state and full mark), v1 built and
/// linked (its capture and full mark), U4-U5 on v1's front against v0's
/// state, `live` on the join's change; the delta checked against the two
/// marks. `None`: the driver is missing.
#[allow(clippy::too_many_lines, reason = "one pair, its oracle in order")]
fn run_pair(arch: &str, cc: &str, cxx: &str, p: &Pair) -> Option<Verdict> {
    let label = format!("{} {arch}", p.name);
    let driver = if p.cxx { cxx } else { cc };
    let l = line(&label, driver, &p.flags, &p.v0, &p.extra)?;
    let m0 = link(&l, "out");
    let store0 = store();
    let f0 = front5(&l, &store0);
    let (old, old_pieces) = full_mark(&f0, &m0, &[]);
    drop(f0);
    drop(store0);
    build_by(&l.scratch, driver, &p.flags, &p.v1).expect("v1 builds");
    let m1 = link(&l, "out1");
    // One store per link (B18-1: the compiler rewrites an object in
    // place, keeping its inode).
    let store1 = store();
    let f1 = front5(&l, &store1);
    let ids: Vec<FileId> = p
        .changed
        .iter()
        .map(|o| {
            f1.inputs
                .files
                .iter()
                .find(|(_, x)| x.path.ends_with(o.as_bytes()))
                .map(|(id, _)| id)
                .unwrap_or_else(|| panic!("{label}: no {o}"))
        })
        .collect();
    let view = m0.view();
    let (changed, delta) =
        match mortar::update::changed::u5(&view, &ids, &[], &f1, &m1, &mut |_| {}) {
            Err(e) => return Some(Verdict::Fallback(format!("u5: {}", e.report()))),
            Ok(None) => panic!("{label}: u5 saw no change"),
            Ok(Some(x)) => x,
        };
    let options = mortar_resolve::tables::option_keys(&f1.plan, &f1.inputs.link.directives);
    let d = match live_why(&view, &changed, &delta, &options) {
        Err((_, why)) => return Some(Verdict::Fallback(format!("live: {why}"))),
        Ok(d) => d,
    };
    let fresh: Vec<(FileId, u32)> =
        changed.files.iter().map(|c| (c.file, c.first_section.0)).collect();
    let (new, new_pieces) = full_mark(&f1, &m0, &fresh);
    let now_live: Vec<SectionId> =
        new.iter().filter(|s| old.binary_search(s).is_err()).copied().collect();
    let now_dead: Vec<SectionId> =
        old.iter().filter(|s| new.binary_search(s).is_err()).copied().collect();
    // Section names in the update's space; the file of an id.
    let base = u32::try_from(m0.sections.len()).expect("sections");
    let fresh_of = |s: SectionId| {
        changed
            .files
            .iter()
            .find(|c| s.0 >= c.first_section.0 && s.0 - c.first_section.0 < c.sections.len() as u32)
    };
    let name = |s: SectionId| -> String {
        let n = if s.0 >= base {
            let c = fresh_of(s).expect("a fresh section");
            changed.string(&view, c.sections[(s.0 - c.first_section.0) as usize].name)
        } else {
            view.string(m0.sections[s.0 as usize].name)
        };
        String::from_utf8_lossy(&n.expect("a name")).into_owned()
    };
    let replaced: Vec<FileId> = changed.files.iter().map(|c| c.file).collect();
    let mut drawn: Vec<FileId> = now_live
        .iter()
        .chain(&now_dead)
        .filter(|s| s.0 < base)
        .map(|s| FileId(m0.sections[s.0 as usize].file))
        .filter(|f| !replaced.contains(f))
        .collect();
    for (s, v) in &new_pieces {
        if s.0 < base
            && let Ok(i) = old_pieces.binary_search_by_key(s, |x| x.0)
            && old_pieces[i].1 != *v
        {
            drawn.push(FileId(m0.sections[s.0 as usize].file));
        }
    }
    drawn.sort_unstable();
    drawn.dedup();
    let pieces: Vec<(SectionId, Vec<u32>)> = new_pieces
        .iter()
        .filter(|(s, _)| s.0 >= base || drawn.contains(&FileId(m0.sections[s.0 as usize].file)))
        .cloned()
        .collect();
    let names = |v: &[SectionId]| v.iter().map(|&s| name(s)).collect::<Vec<_>>();
    assert_eq!(
        d.now_live,
        now_live,
        "{label}: now_live {:?} vs {:?}",
        names(&d.now_live),
        names(&now_live)
    );
    assert_eq!(
        d.now_dead,
        now_dead,
        "{label}: now_dead {:?} vs {:?}",
        names(&d.now_dead),
        names(&now_dead)
    );
    assert_eq!(d.drawn_in, drawn, "{label}: drawn_in");
    assert_eq!(d.pieces, pieces, "{label}: pieces");
    // The pair exercises what it says.
    let (dead, livened) = (names(&now_dead), names(&now_live));
    for n in &p.dies {
        assert!(dead.iter().any(|x| x == n), "{label}: {n} not in now_dead {dead:?}");
        assert!(!livened.iter().any(|x| x == n), "{label}: {n} in now_live {livened:?}");
    }
    for n in &p.lives {
        assert!(livened.iter().any(|x| x == n), "{label}: {n} not in now_live {livened:?}");
    }
    Some(Verdict::Counted(d.now_live.len(), d.now_dead.len(), d.drawn_in.len(), d.pieces.len()))
}

/// Runs `pairs` on both targets; the table (`WP238_PAIR_TABLE`); the
/// verdicts by pair and target.
fn run_all(pairs: &[Pair]) -> Vec<(String, Verdict)> {
    let mut out = Vec::new();
    for (arch, cc, cxx, _) in targets() {
        for p in pairs {
            if let Some(v) = run_pair(arch, &cc, &cxx, p) {
                out.push((format!("{} {arch}", p.name), v));
            }
        }
    }
    let mut table = String::from("pair | now_live now_dead drawn_in pieces | or fall-back\n");
    for (l, v) in &out {
        let _ = match v {
            Verdict::Counted(a, b, c, d) => writeln!(table, "{l} | {a} {b} {c} {d}"),
            Verdict::Fallback(w) => writeln!(table, "{l} | FALLBACK {w}"),
        };
    }
    eprintln!("{table}");
    if let Ok(p) = std::env::var("WP238_PAIR_TABLE") {
        let old = std::fs::read_to_string(&p).unwrap_or_default();
        std::fs::write(p, old + &table).expect("the table");
    }
    out
}

// ---- P3b-3: a START_STOP row as the only reason ----

/// `ssreg`: a C-identifier section with no `SHF_GNU_RETAIN` (no `used`).
const SS_R: &str = "__attribute__((section(\"ssreg\"))) const int ss_a = 7;\n\
__attribute__((section(\"ssreg\"))) const int ss_b = 8;\n";
/// v0: the live `count` reads `__start_ssreg`; the dead `dead_user` too.
const SS_M0: &str = "extern const int __start_ssreg[];\n\
__attribute__((noinline)) int count(void){ return __start_ssreg[0]; }\n\
__attribute__((noinline)) int dead_user(void){ return __start_ssreg[1]; }\n\
int main(void){ return count(); }\n";
/// v1: `count` is dead (the last LIVE use gone); `dead_user` unchanged.
const SS_M1: &str = "extern const int __start_ssreg[];\n\
__attribute__((noinline)) int count(void){ return __start_ssreg[0]; }\n\
__attribute__((noinline)) int dead_user(void){ return __start_ssreg[1]; }\n\
int main(void){ return 0; }\n";

const FS: [&str; 3] = ["-O1", "-ffunction-sections", "-fdata-sections"];

fn ss_flags(g: bool) -> Vec<&'static str> {
    let mut f = FS.to_vec();
    if g {
        f.push("-g");
    }
    f
}

fn judged(m: &MemoryState, o: &OptionKeys) -> Result<(), IncrReason> {
    live_why(&m.view(), &ChangedFiles::default(), &ResolveDelta::default(), o)
        .map(|_| ())
        .map_err(|e| e.0)
}

// spec: incr.md IN-71a (WP-238 P3b-3): the `START_STOP` row from the live
// `count` is the only reason `ssreg` is live: without it (removed, or
// retargeted to another defined key) the stored walk does not mark
// `ssreg`, and the state is refused `state-corrupt` by S4 before any
// delta. A row from the dead `dead_user` does not keep it. With and
// without `-g`, AArch64 and x86-64.
#[test]
fn a_start_stop_row_alone_keeps_its_section() {
    let mut ran = 0;
    for (arch, cc, _, _) in targets() {
        for g in [false, true] {
            let label = format!("startstop-only {arch}{}", if g { " -g" } else { "" });
            let src = [Source { name: "m.c", text: SS_M0 }, Source { name: "r.c", text: SS_R }];
            let Some(l) = line(&label, &cc, &ss_flags(g), &src, &[]) else { continue };
            let m = link(&l, "out");
            let store0 = store();
            let f = front5(&l, &store0);
            let o = mortar_resolve::tables::option_keys(&f.plan, &f.inputs.link.directives);
            let before: StoredWalk =
                walk_stored(&m.view(), &o).unwrap_or_else(|e| panic!("{label}: {}", e.1));
            assert_eq!(judged(&m, &o), Ok(()), "{label}: as written");
            let view = m.view();
            let ssreg: Vec<usize> = (0..m.sections.len())
                .filter(|&s| view.string(m.sections[s].name).is_ok_and(|n| n == b"ssreg"))
                .collect();
            assert_eq!(ssreg.len(), 1, "{label}: ssreg sections");
            let ss = ssreg[0];
            assert!(before.live[ss], "{label}: ssreg is not live");
            // The row from a live section, and the one from the dead one.
            let rows: Vec<usize> = (0..m.edges.len())
                .filter(|&i| m.edges[i].1.kind == edge_kind::START_STOP)
                .collect();
            let live_rows: Vec<usize> = rows
                .iter()
                .copied()
                .filter(|&i| before.live[m.edges[i].1.source as usize])
                .collect();
            assert_eq!(
                live_rows.len(),
                1,
                "{label}: START_STOP rows {rows:?}, from live sections {live_rows:?}"
            );
            assert!(rows.len() > live_rows.len(), "{label}: no START_STOP row from dead_user");
            let i = live_rows[0];
            // Removed.
            let mut cut = m.clone();
            cut.edges.remove(i);
            let w = walk_stored(&cut.view(), &o).unwrap_or_else(|e| panic!("{label}: {}", e.1));
            assert!(!w.live[ss], "{label}: ssreg live without its START_STOP row");
            assert!(
                matches!(judged(&cut, &o), Err(IncrReason::StateCorrupt(_))),
                "{label}: the row removed: {:?}",
                judged(&cut, &o)
            );
            // Retargeted to another defined key (not a `__start_`/`__stop_`).
            let k = (0..u32::try_from(m.symbols.len()).expect("keys"))
                .find(|&k| {
                    k != m.edges[i].1.target && m.resolved[k as usize].kind == def_kind::SECTION
                })
                .expect("a defined key");
            let mut re = m.clone();
            re.edges[i].1.target = k;
            let w = walk_stored(&re.view(), &o).unwrap_or_else(|e| panic!("{label}: {}", e.1));
            assert!(!w.live[ss], "{label}: ssreg live with its row retargeted");
            assert!(
                matches!(judged(&re, &o), Err(IncrReason::StateCorrupt(_))),
                "{label}: the row retargeted: {:?}",
                judged(&re, &o)
            );
            ran += 1;
        }
    }
    assert!(ran == 4 || !tools_required(), "{ran} of 4 states");
}

fn startstop_pairs() -> Vec<Pair> {
    [false, true]
        .map(|g| Pair {
            name: if g { "startstop-last-use -g" } else { "startstop-last-use" },
            cxx: false,
            flags: ss_flags(g),
            v0: vec![Source { name: "m.c", text: SS_M0 }, Source { name: "r.c", text: SS_R }],
            v1: vec![Source { name: "m.c", text: SS_M1 }, Source { name: "r.c", text: SS_R }],
            changed: vec!["m.c.o"],
            extra: vec![],
            dies: vec!["ssreg", ".text.count"],
            lives: vec![],
        })
        .into_iter()
        .collect()
}

// spec: incr.md IN-71a (WP-238 P3b-3): v0 -> v1 removing the last live
// `__start_ssreg` use (a reference from the dead `dead_user` stays):
// `ssreg` (an unchanged file's) is in `now_dead`, its file drawn in,
// exactly the full marks' difference.
#[test]
fn removing_the_last_start_use_kills_its_section() {
    let out = run_all(&startstop_pairs());
    for (l, v) in &out {
        assert!(matches!(v, Verdict::Counted(..)), "{l}: {v:?}");
    }
    assert!(out.len() == 4 || !tools_required(), "{} of 4 pairs", out.len());
}

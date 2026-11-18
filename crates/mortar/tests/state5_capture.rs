//! WP-244: the format-5 state captured from a real link (`spec/incr.md`
//! IN-18e; WP-239 S4-1..S4-5, R-2, R-4a), behind WP-244's switch
//! (`mortar::invoke_state5`; a `testhooks` build's `MORTAR_INCR_STATE5`):
//! deterministic, decoded by U-2's reader; per-file demand ORs to the
//! link's slot lists; the stored facts equal the walker's afresh; a
//! `--defsym` alias has its target's facts; `OUTSECS` is stable across
//! full links of unchanged inputs; an injected fragment collision refuses
//! the state by name and removes the previous one.
//!
//! `MORTAR_WP244_SERIES=<file>` adds real link lines to the facts and
//! demand tests (one `<directory>\t<response file>` per line, as
//! `tests/incr-design/wp244/` writes them for WP-207's M.1 series and
//! WP-239's slot series).

mod support;

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;

use mortar::front::{Env, Front, front};
use mortar::incremental::State5;
use mortar::timing::Timer;
use mortar_core::id::{SymKey, SymbolId};
use mortar_core::layout::SlotKind;
use mortar_diag::{Collector, Policy};
use mortar_incr::state5::read::decode;
use mortar_incr::update::rows::sym_kind;
use mortar_incr::update::testing::MemoryState;
use mortar_scan::{ScanInputs, ScanOptions, Walker};
use mortar_testutil::{Scratch, tools_required};
use support::{Built, Driver, build, build_by, store, with_output};

/// A link line: its directory and arguments (`-o` naming `out`).
struct Line {
    _scratch: Option<Scratch>,
    label: String,
    dir: PathBuf,
    args: Vec<String>,
}

impl Line {
    fn of(label: &str, scratch: Scratch, built: Built) -> Self {
        Self { _scratch: Some(scratch), label: label.to_owned(), dir: built.dir, args: built.args }
    }

    fn args(&self, out: &str, extra: &[&str]) -> Vec<OsString> {
        let mut a = with_output(&self.args, &self.dir.join(out));
        a.extend(["--incremental=full", "--incremental-state"].iter().map(OsString::from));
        a.push(format!("{out}.mortar-state").into());
        a.extend(extra.iter().map(OsString::from));
        a
    }
}

/// The C++ program (exceptions, COMDAT, TLS, IFUNCs) on both 64-bit
/// targets, GCC's objects.
fn lines() -> Vec<Line> {
    let mut v = Vec::new();
    let s = Scratch::new("wp244-a64");
    match build(&s, Driver::Gxx, &["-O1"], support::CXX_PROGRAM) {
        Some(b) => v.push(Line::of("aarch64 g++", s, b)),
        None => assert!(!tools_required(), "{} is required", Driver::Gxx.tool()),
    }
    let s = Scratch::new("wp244-x64");
    match build_by(&s, "x86_64-linux-gnu-g++", &["-O1"], support::CXX_PROGRAM) {
        Some(b) => v.push(Line::of("x86-64 g++", s, b)),
        None => assert!(!tools_required(), "x86_64-linux-gnu-g++ is required"),
    }
    v
}

/// [`lines`], and the real link lines `MORTAR_WP244_SERIES` names (each
/// response file's words, `-o` removed).
fn lines_and_series() -> Vec<Line> {
    let mut v = lines();
    let Some(list) = std::env::var_os("MORTAR_WP244_SERIES") else { return v };
    let text = std::fs::read_to_string(&list).expect("MORTAR_WP244_SERIES");
    for l in text.lines().filter(|l| !l.trim().is_empty()) {
        let (dir, resp) = l.split_once('\t').expect("<directory>\\t<response file>");
        let words = std::fs::read_to_string(resp).expect("a response file");
        let mut args = Vec::new();
        let mut it = words.split_whitespace();
        while let Some(w) = it.next() {
            if w == "-o" {
                it.next();
                continue;
            }
            args.push(w.to_owned());
        }
        args.extend(["-o".to_owned(), "out".to_owned()]);
        v.push(Line { _scratch: None, label: resp.to_owned(), dir: dir.into(), args });
    }
    v
}

/// The link of `line` to `out` with the switch on: its `incremental:`
/// lines and the format-5 state's bytes (`None`: none written).
fn link5(line: &Line, out: &str, extra: &[&str], collide: bool) -> (String, Option<Vec<u8>>) {
    let path = line.dir.join(format!("{out}.state5"));
    let inv = mortar::invoke_state5(
        "mortar",
        &line.args(out, extra),
        &line.dir,
        State5 { path: path.clone(), collide },
    );
    let ok = matches!(inv.outcome, mortar::Outcome::Linked { .. });
    assert!(ok, "{}: the link failed: {:?}", line.label, inv.outcome);
    (inv.incremental, std::fs::read(&path).ok())
}

/// The front of `line` as an incremental-mode link with the switch on.
fn front5<'s>(line: &Line, store: &'s mortar_input::store::Store) -> Front<'s> {
    let parsed = mortar_cli::parse(&with_output(&line.args, &line.dir.join("front")));
    assert!(!parsed.failed(), "{:?}", parsed.diagnostics);
    let diag = Collector::new(Policy::default());
    let pool =
        mortar_core::par::Pool::try_new(mortar_core::par::Pool::available()).expect("a pool");
    let env = Env { cwd: &line.dir, pool, replay: None, slack: true, state5: true };
    let f = front(parsed.plan, env, store, &diag, &mut Timer::new(false));
    let report = diag.finish();
    assert!(!report.failed(), "{}: {report:?}", line.label);
    f.expect("the front")
}

fn string(m: &MemoryState, s: mortar_incr::format::Str) -> Vec<u8> {
    m.strings[s.off as usize..(s.off + s.len) as usize].to_vec()
}

/// Each global `SYMBOLS` row's `SymbolId` in `f`, by name and version.
fn ids_of(m: &MemoryState, f: &Front<'_>) -> Vec<Option<SymbolId>> {
    let mut by_name: HashMap<(Vec<u8>, Vec<u8>), SymbolId> = HashMap::new();
    for (id, k) in f.inputs.symbols.iter() {
        by_name.insert((k.name.to_vec(), k.version.unwrap_or_default().to_vec()), id);
    }
    m.symbols
        .iter()
        .map(|r| match r.kind {
            sym_kind::GLOBAL | sym_kind::VERSIONED => {
                by_name.get(&(string(m, r.name), string(m, r.version))).copied()
            }
            _ => None,
        })
        .collect()
}

/// Clears what is the link's own moment, not its inputs': `STAT.stamp_ns`
/// (when the link took its stats) and the `OUTPUT` row's inode and times
/// (the output file the link just wrote). Format 4's columns too; IN-6's
/// "function of the inputs" holds for every other byte.
fn clear_moment(m: &mut MemoryState) {
    for r in &mut m.stat {
        r.stamp_ns = 0;
        if r.tag == mortar_incr::format::stat_tag::OUTPUT {
            (r.ino, r.mtime_ns, r.ctime_ns) = (0, 0, 0);
        }
    }
}

/// `bytes` decoded, [`clear_moment`], as a comparable string.
fn unstamped(bytes: &[u8]) -> String {
    let mut m = decode(bytes).expect("the state decodes");
    clear_moment(&mut m);
    format!("{m:?}")
}

/// The tables in which two states differ, for a failing determinism test.
fn differing(a: &[u8], b: &[u8]) -> String {
    let (Ok(mut a), Ok(mut b)) = (decode(a), decode(b)) else { return "undecodable".into() };
    clear_moment(&mut a);
    clear_moment(&mut b);
    let mut out = Vec::new();
    macro_rules! cmp {
        ($($f:ident),*) => {$(
            let (x, y) = (format!("{:?}", a.$f), format!("{:?}", b.$f));
            if x != y {
                let at = x.bytes().zip(y.bytes()).position(|(p, q)| p != q).unwrap_or(0);
                let lo = at.saturating_sub(200);
                out.push(format!(
                    "{}: ..{}.. vs ..{}..",
                    stringify!($f),
                    &x[lo..(at + 200).min(x.len())],
                    &y[lo..(at + 200).min(y.len())]
                ));
            }
        )*};
    }
    cmp!(
        plan_hash,
        meta,
        probes,
        outsecs,
        chunks,
        order,
        segments,
        slots,
        sizes,
        sym_place,
        defined,
        patches,
        redirects,
        sized,
        layout,
        leaves,
        files,
        file_index,
        stat,
        strings,
        sections,
        mentions,
        index,
        edges,
        frag_uses,
        symtab,
        regions,
        key_refs,
        groups,
        frags,
        holes,
        commons,
        copies,
        odr_classes,
        odr_refs,
        diags,
        names,
        symbols,
        resolved,
        symtab_keys
    );
    out.join("\n")
}

// spec: incr.md IN-6 (a state is a function of the inputs), IN-18e; WP-244
// "Determinism": two captures of one link are byte-equal, with 1 and with
// many threads (but the link's moment: [`clear_moment`]); the state
// decodes.
#[test]
fn capture5_is_deterministic_and_decodes() {
    for line in lines() {
        let (err, one) = link5(&line, "t1", &["--threads=1"], false);
        let one = one.unwrap_or_else(|| panic!("{}: no format-5 state: {err}", line.label));
        let (_, again) = link5(&line, "t1", &["--threads=1"], false);
        let (_, many) = link5(&line, "t1", &["--threads=8"], false);
        assert!(
            again.as_deref().map(unstamped) == Some(unstamped(&one)),
            "{}: two captures differ: {}",
            line.label,
            differing(&one, again.as_deref().unwrap_or_default())
        );
        assert!(
            many.as_deref().map(unstamped) == Some(unstamped(&one)),
            "{}: 1 and 8 threads differ: {}",
            line.label,
            differing(&one, many.as_deref().unwrap_or_default())
        );
        let m = decode(&one).unwrap_or_else(|e| panic!("{}: {e}", line.label));
        assert!(!m.key_refs.is_empty() && !m.mentions.is_empty() && !m.frags.is_empty());
        assert!(m.files.iter().any(|f| f.eh_records > 0), "{}: no .eh_frame range", line.label);
        assert!(m.resolved.iter().any(|r| r.facts != 0), "{}: no facts", line.label);
    }
}

// spec: incr.md §7 `state-collision` (WP-239 R-2: the STATE is refused,
// never the link; no state is written and a previous one is removed)
#[test]
fn injected_collision_refuses_the_state() {
    let Some(line) = lines().into_iter().next() else { return };
    let (_, first) = link5(&line, "c", &[], false);
    assert!(first.is_some(), "no first state");
    let (err, second) = link5(&line, "c", &[], true);
    assert!(
        err.lines().any(|l| l.starts_with("incremental: no state written: state-collision")),
        "{err}"
    );
    assert!(second.is_none(), "a state was left behind");
    assert!(line.dir.join("c").exists(), "the link itself failed");
}

/// What `slot` needs of a file's demand.
fn needs(flags: mortar_core::demands::DemandFlags, kind: SlotKind) -> bool {
    flags.intersects(mortar::front::slot_demand(kind))
}

const KINDS: [SlotKind; 6] = [
    SlotKind::Got,
    SlotKind::Plt,
    SlotKind::Iplt,
    SlotKind::TlsDesc,
    SlotKind::TlsGd,
    SlotKind::TlsIe,
];

// spec: WP-239 S4-4, S4-5 (the OR of the files' demands is the link-wide
// column; `SLOTS.refs` is the number of live files that demand the slot,
// a local's 1)
#[test]
fn file_demands_or_to_slots() {
    for line in lines_and_series() {
        let st = store();
        let f = front5(&line, &st);
        let i5 = f.incr5.as_ref().expect("the format-5 data");
        let d = &f.scanned.demands;
        let mut global = vec![mortar_core::demands::DemandFlags::empty(); d.global.len() as usize];
        let mut local = HashMap::new();
        let mut refs: HashMap<(usize, SymKey), u32> = HashMap::new();
        for fd in &i5.demands {
            assert!(fd.demands.windows(2).all(|w| w[0].key < w[1].key), "not sorted by key");
            for x in &fd.demands {
                assert!(!x.flags.is_empty(), "an empty demand row");
                match x.key.kind() {
                    mortar_core::id::SymKind::Global(id) => global[id.0 as usize] |= x.flags,
                    mortar_core::id::SymKind::Local(l) => {
                        *local.entry(l).or_insert(mortar_core::demands::DemandFlags::empty()) |=
                            x.flags;
                    }
                }
                for (k, kind) in KINDS.iter().enumerate() {
                    if needs(x.flags, *kind) {
                        *refs.entry((k, x.key)).or_default() += 1;
                    }
                }
            }
        }
        for (id, &flags) in d.global.iter() {
            assert_eq!(global[id.0 as usize], flags, "{}: key {id:?}", line.label);
        }
        let want: HashMap<_, _> = d.local.iter().copied().collect();
        assert_eq!(local, want, "{}: the locals' demand", line.label);
        // SLOTS.refs as the capture stores them.
        let (_, bytes) = link5(&line, "slots", &[], false);
        let m = decode(&bytes.expect("a state")).expect("decodes");
        let ids = ids_of(&m, &f);
        let mut checked = 0;
        for r in &m.slots {
            let kind = KINDS[r.kind as usize];
            let want = match ids[r.symbol as usize] {
                Some(id) => refs.get(&(r.kind as usize, SymKey::global(id))).copied().unwrap_or(0),
                None => 1,
            };
            assert_eq!(r.refs, want, "{}: SLOTS {kind:?} row {}", line.label, r.symbol);
            assert!(r.refs > 0, "{}: a slot no live file demands", line.label);
            checked += 1;
        }
        eprintln!("{}: {checked} SLOTS rows checked", line.label);
    }
}

/// The walker of `f`'s link, made afresh (as the image phase makes it).
fn walker<'a, 's>(f: &'a Front<'s>, inputs: &'a ScanInputs<'a, 's>) -> Walker<'a, 's> {
    let opts = ScanOptions {
        output: mortar::front::output_kind(f.plan.kind),
        dynamic: false,
        sections: None,
        proof: f.arch.proof(),
    };
    Walker::new(f.arch.get(), inputs, opts).with_weak_dyn(f.scanned.demands.weak_dyn.clone())
}

fn scan_inputs<'a, 's>(f: &'a Front<'s>) -> ScanInputs<'a, 's> {
    ScanInputs {
        inputs: &f.inputs,
        resolution: &f.resolution,
        liveness: &f.liveness,
        assignment: Some(&f.assignment),
        comdat_mismatch: &f.comdat_mismatch,
        gnu_warnings: &f.gnu_warnings,
        closure_only: &f.closure_only,
        eh: None,
    }
}

// spec: WP-239 R-4a (`RESOLVED.facts` = `pack(Walker::facts(SymKey::global(id)))`,
// equal afresh), over the C++ program on both targets and, with
// MORTAR_WP244_SERIES, WP-207's M.1 series and WP-239's slot series
#[test]
fn stored_facts_equal_afresh_over_m1_and_slot_series() {
    for line in lines_and_series() {
        let (_, bytes) = link5(&line, "facts", &[], false);
        let m = decode(&bytes.expect("a state")).expect("decodes");
        let st = store();
        let f = front5(&line, &st);
        let si = scan_inputs(&f);
        let w = walker(&f, &si);
        let ids = ids_of(&m, &f);
        let mut n = 0;
        for (row, id) in ids.iter().enumerate() {
            let Some(id) = *id else { continue };
            let afresh = w.facts(SymKey::global(id));
            let stored = m.resolved[row].facts;
            assert_eq!(
                mortar_arch::SymFacts::unpack(stored),
                Some(afresh),
                "{}: {}",
                line.label,
                String::from_utf8_lossy(&string(&m, m.symbols[row].name))
            );
            n += 1;
        }
        assert!(n > 0);
        eprintln!("{}: {n} keys' facts equal afresh", line.label);
    }
}

// spec: WP-239 R-4a; resolve.md `--defsym` (an alias takes its target's
// facts: kind, type, TLS)
#[test]
fn defsym_alias_follows_target_facts() {
    for line in lines() {
        let (_, bytes) = link5(
            &line,
            "alias",
            &["--defsym=wp244_alias=main", "--defsym=wp244_tls=depth"],
            false,
        );
        let m = decode(&bytes.expect("a state")).expect("decodes");
        let facts = |name: &[u8]| {
            let row = m
                .symbols
                .iter()
                .position(|r| r.kind != sym_kind::LOCAL && string(&m, r.name) == name)
                .unwrap_or_else(|| {
                    panic!("{}: no key {}", line.label, String::from_utf8_lossy(name))
                });
            mortar_arch::SymFacts::unpack(m.resolved[row].facts).expect("unpacks")
        };
        for (alias, target) in [(&b"wp244_alias"[..], &b"main"[..]), (b"wp244_tls", b"depth")] {
            let (a, t) = (facts(alias), facts(target));
            assert_eq!((a.kind, a.sym_type, a.tls), (t.kind, t.sym_type, t.tls), "{}", line.label);
        }
    }
}

// spec: WP-244 item 5 (`OUTSECS` rows are stable across full links of
// unchanged inputs), IN-6
#[test]
fn outsecs_rows_are_stable_across_full_links() {
    for line in lines() {
        let rows = |out: &str| {
            let (_, bytes) = link5(&line, out, &[], false);
            let m = decode(&bytes.expect("a state")).expect("decodes");
            m.outsecs.iter().map(|o| (string(&m, o.name), o.ordinal)).collect::<Vec<_>>()
        };
        let a = rows("o1");
        assert!(!a.is_empty());
        assert_eq!(a, rows("o2"), "{}", line.label);
        assert_eq!(a, rows("o1"), "{}", line.label);
    }
}

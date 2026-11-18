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
        State5 { path: path.clone(), collide, rows: None },
    );
    let ok = matches!(inv.outcome, mortar::Outcome::Linked { .. });
    assert!(ok, "{}: the link failed: {:?}", line.label, inv.outcome);
    (inv.incremental, std::fs::read(&path).ok())
}

/// The link of `line` to `out` with the switch on: the state's rows,
/// normalized, before encoding (seam 6's columns, which format 5's bytes
/// do not hold until U-2 encodes them).
fn rows5(line: &Line, out: &str, extra: &[&str]) -> MemoryState {
    let sink: mortar::incremental::Rows5Sink = std::sync::Arc::default();
    let inv = mortar::invoke_state5(
        "mortar",
        &line.args(out, extra),
        &line.dir,
        State5 {
            path: line.dir.join(format!("{out}.state5")),
            collide: false,
            rows: Some(sink.clone()),
        },
    );
    let ok = matches!(inv.outcome, mortar::Outcome::Linked { .. });
    assert!(ok, "{}: the link failed: {:?}", line.label, inv.outcome);
    let m = sink.lock().expect("the sink").take();
    m.unwrap_or_else(|| panic!("{}: no rows: {}", line.label, inv.incremental))
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

/// The output's `.note.gnu.property` properties, ascending (ELF64 LE):
/// `(pr_type, data)`; empty without the section.
fn output_properties(path: &std::path::Path) -> Vec<(u32, Vec<u8>)> {
    let b = std::fs::read(path).expect("the output");
    let u16_at = |o: usize| u16::from_le_bytes(b[o..o + 2].try_into().unwrap()) as usize;
    let u32_at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let u64_at = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap()) as usize;
    assert_eq!(&b[..5], b"\x7fELF\x02", "an ELF64 output");
    let (shoff, shentsize, shnum, shstrndx) =
        (u64_at(0x28), u16_at(0x3a), u16_at(0x3c), u16_at(0x3e));
    let sh = |i: usize| shoff + i * shentsize;
    let strs = u64_at(sh(shstrndx) + 0x18);
    let mut out = Vec::new();
    for i in 0..shnum {
        let name = strs + u32_at(sh(i)) as usize;
        let end = b[name..].iter().position(|&c| c == 0).unwrap() + name;
        if &b[name..end] != b".note.gnu.property" {
            continue;
        }
        let (off, size) = (u64_at(sh(i) + 0x18), u64_at(sh(i) + 0x20));
        let mut at = off;
        while at < off + size {
            let (namesz, descsz, typ) =
                (u32_at(at) as usize, u32_at(at + 4) as usize, u32_at(at + 8));
            let desc = at + 12 + namesz.next_multiple_of(4);
            if typ == 5 && &b[at + 12..at + 12 + namesz] == b"GNU\0" {
                let mut p = desc;
                while p < desc + descsz {
                    let (t, n) = (u32_at(p), u32_at(p + 4) as usize);
                    out.push((t, b[p + 8..p + 8 + n].to_vec()));
                    p += 8 + n.next_multiple_of(8);
                }
            }
            at = desc + descsz.next_multiple_of(8);
        }
    }
    out.sort();
    out
}

// spec: incr.md IN-18e (`FILES.stack`, `FILES.properties`, `eh_fdes`;
// WP-214 seam 6, ruling 3; IN-72 items 5 and 10): the join's merges over
// the stored columns give the output's `PT_GNU_STACK` flags, its
// `.note.gnu.property` bytes and its `.eh_frame_hdr` FDE count, read
// from the output's bytes.
#[test]
fn seam6_file_columns_remake_the_outputs_notes() {
    use mortar_incr::update::rows::{decode_properties, file_flags, file_stack};
    use mortar_layout::stack::{StackNote, executable_stack};
    for line in lines() {
        let m = rows5(&line, "notes", &["--eh-frame-hdr"]);
        let out = line.dir.join("notes");
        let live_objects: Vec<&mortar_incr::update::rows::FileRow> =
            m.files.iter().filter(|f| f.kind == 1 && f.flags & file_flags::LAZY == 0).collect();
        // L-84 over the stored bytes, as the join runs it.
        let notes: Vec<(String, StackNote)> = live_objects
            .iter()
            .map(|f| {
                let n = match f.stack {
                    file_stack::ABSENT => StackNote::Absent,
                    file_stack::NON_EXEC => StackNote::NonExec,
                    file_stack::EXEC => StackNote::Exec,
                    b => panic!("FILES.stack {b}"),
                };
                (String::from_utf8_lossy(&string(&m, f.path)).into_owned(), n)
            })
            .collect();
        assert!(notes.iter().any(|n| n.1 == StackNote::NonExec), "{}: no stack note", line.label);
        let (exec, _) = executable_stack(&notes, None);
        let summary = support::summarize_elf(&out);
        let seg = summary.segments.iter().find(|g| g.p_type == "GNU_STACK").expect("PT_GNU_STACK");
        assert_eq!(exec, seg.flags.contains('E'), "{}: PT_GNU_STACK {}", line.label, seg.flags);
        // S-95/S-96 over the stored properties (every live object, those
        // without as "every property absent").
        let per_object: Vec<std::collections::BTreeMap<u32, Vec<u8>>> = live_objects
            .iter()
            .map(|f| {
                decode_properties(&string(&m, f.properties)).expect("decodes").into_iter().collect()
            })
            .collect();
        let with = per_object.iter().filter(|p| !p.is_empty()).count();
        let store = store();
        let f = front5(&line, &store);
        let arch = f.arch.get();
        let processor = |t: u32| arch.property_merge(t);
        let diag = Collector::new(Policy::default());
        let plan = mortar_synth::PropertyPlan::merged(
            &per_object,
            arch.facts().class,
            &processor,
            false,
            &diag,
        )
        .expect("the merge");
        let merged: Vec<(u32, Vec<u8>)> =
            plan.properties().iter().map(|(t, v)| (*t, v.to_vec())).collect();
        assert_eq!(merged, output_properties(&out), "{}: .note.gnu.property", line.label);
        // IN-72 item 5: the stored FDE counts are `.eh_frame_hdr`'s table.
        let fdes: u64 = m.files.iter().map(|f| u64::from(f.eh_fdes)).sum();
        assert!(m.files.iter().all(|f| f.eh_fdes <= f.eh_records), "{}", line.label);
        assert_eq!(Some(fdes), summary.hdr_fdes, "{}: .eh_frame_hdr's FDEs", line.label);
        assert_eq!(fdes, summary.fdes, "{}: .eh_frame's FDEs", line.label);
        eprintln!(
            "{}: {} live objects, {with} with properties ({} merged), {fdes} FDEs, exec stack {exec}",
            line.label,
            live_objects.len(),
            merged.len()
        );
    }
}

// spec: incr.md IN-18d, IN-18e (a file's locals are ONE run of `SYMBOLS`
// rows, by index, named by `FILE_INDEX.locals`; WP-214 seam 6, ruling 1)
#[test]
fn seam6_file_locals_are_one_run() {
    let mut total = 0;
    for line in lines() {
        let m = rows5(&line, "locals", &[]);
        let mut covered = 0u32;
        for (file, idx) in m.file_index.iter().enumerate() {
            let r = idx.locals;
            let run = &m.symbols[r.first as usize..(r.first + r.count) as usize];
            assert!(run.iter().all(|s| s.kind == sym_kind::LOCAL && s.file == file as u32));
            assert!(run.windows(2).all(|w| w[0].at_index < w[1].at_index), "{}", line.label);
            covered += r.count;
        }
        let locals = m.symbols.iter().filter(|s| s.kind == sym_kind::LOCAL).count();
        assert_eq!(covered as usize, locals, "{}: locals outside their runs", line.label);
        // A state holds the local keys the link named (a relocation's
        // symbol): x86-64 GCC's objects name sections instead.
        eprintln!("{}: {locals} local keys", line.label);
        total += locals;
    }
    assert!(total > 0, "no local key in any line");
}

// spec: incr.md IN-18e (`FRAGS.size`: the fragment's bytes, WP-242 C via
// seam 6): each stored fragment's size is the full link's, at its offset.
#[test]
fn seam6_frag_sizes_are_the_links() {
    for line in lines() {
        let m = rows5(&line, "frags", &[]);
        let store = store();
        let f = front5(&line, &store);
        let mut by_place: HashMap<(u32, u64), u32> = HashMap::new();
        for (frag, &off) in f.layout.merge.frag_off.iter() {
            if off == mortar_core::layout::MergeMap::NONE {
                continue;
            }
            let fr = &f.merge.split.frags[frag];
            let out = f.assignment.out(fr.section).expect("an output section");
            by_place.insert((out.0, off), fr.size);
        }
        assert!(!m.frags.is_empty());
        for r in &m.frags {
            assert!(r.size > 0, "{}: an empty fragment", line.label);
            assert_eq!(by_place.get(&(r.out, r.offset)), Some(&r.size), "{}: {r:?}", line.label);
        }
    }
}

// spec: incr.md IN-18e (`SYMBOLS.option`, WP-214 seam 6 ruling 2: set on
// the rows `Plan::option_keys` names; such a key is never dead, IN-71b)
#[test]
fn seam6_option_bit_marks_the_option_keys() {
    for line in lines() {
        let m = rows5(&line, "opt", &["--defsym=wp244_alias=main", "--wrap=wp244_nowhere"]);
        let named: Vec<&[u8]> = vec![
            b"wp244_alias",
            b"main",
            b"wp244_nowhere",
            b"__wrap_wp244_nowhere",
            b"__real_wp244_nowhere",
        ];
        let mut seen = Vec::new();
        for r in m.symbols.iter().filter(|r| r.option) {
            assert!(matches!(r.kind, sym_kind::GLOBAL | sym_kind::VERSIONED));
            seen.push(string(&m, r.name));
        }
        seen.sort();
        let mut want: Vec<Vec<u8>> = named.iter().map(|n| n.to_vec()).collect();
        want.sort();
        assert_eq!(seen, want, "{}: the option rows", line.label);
    }
}

/// One file's stored rows, keys and files by name (comparable across
/// links): its `FILES` row less what moves with other files (rank, round,
/// the eh offset), its `SECTIONS`, `MENTIONS`, `FRAG_USE` and `KEY_REFS`.
fn file_rows(m: &MemoryState, file: usize) -> Vec<(&'static str, String)> {
    let key = |k: u32| {
        let r = &m.symbols[k as usize];
        (r.kind, string(m, r.name), string(m, r.version))
    };
    let mut out = Vec::new();
    let mut f = m.files[file].clone();
    let path = (string(m, f.path), string(m, f.member_name));
    (f.rank, f.round, f.eh_offset, f.path, f.member_name) =
        (0, 0, 0, mortar_incr::format::Str::default(), mortar_incr::format::Str::default());
    out.push(("FILES", format!("{path:?} {f:?}")));
    for s in m.sections.iter().filter(|s| s.file == file as u32) {
        let mut s = s.clone();
        let anchor = string(m, s.anchor);
        s.anchor = mortar_incr::format::Str::default();
        out.push(("SECTIONS", format!("{anchor:?} {s:?}")));
    }
    // A section by its position in its file (a global row number moves
    // when an earlier file's sections change).
    let first = m.sections.iter().position(|s| s.file == file as u32).unwrap_or(0);
    for (_, r) in m.mentions.iter().filter(|r| r.0.0 == file as u32) {
        let sec = if r.section == u32::MAX { None } else { Some(r.section as usize - first) };
        out.push(("MENTIONS", format!("{:?} {:?} {sec:?}", key(r.symbol), MentionKey(r))));
    }
    for (_, r) in m.frag_uses.iter().filter(|r| r.0.0 == file as u32) {
        out.push(("FRAG_USE", format!("{r:?}")));
    }
    let mut refs: Vec<String> = m
        .key_refs
        .iter()
        .filter(|r| r.file == file as u32)
        .map(|r| {
            format!("{:?} {} {} {} {} {}", key(r.key), r.def, r.bind, r.typ, r.flags, r.demand)
        })
        .collect();
    refs.sort();
    out.extend(refs.into_iter().map(|r| ("KEY_REFS", r)));
    out
}

/// A `MENTIONS` row without its key and section (given beside it).
struct MentionKey<'a>(&'a mortar_incr::update::rows::MentionRow);

impl std::fmt::Debug for MentionKey<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let r = self.0;
        write!(f, "{} {} {} {}", r.kind, r.bind, r.typ, r.other)
    }
}

// WP-244 experiment "M.1 series stability": across the hops of a series
// (`MORTAR_WP244_HOPS=<dir>` holding h0.state5, h1.state5, ...), the
// stored rows of every file whose content hash is unchanged are equal
// (keys and files by name; a section's id is its file's position, so it
// is compared as stored). Prints per hop the files compared and the
// tables in which an unchanged file's rows differ.
#[test]
#[ignore = "an experiment over a series' states: MORTAR_WP244_HOPS"]
fn experiment_unchanged_files_rows_are_stable() {
    let Some(dir) = std::env::var_os("MORTAR_WP244_HOPS") else { return };
    let dir = PathBuf::from(dir);
    let mut hops = Vec::new();
    for n in 0.. {
        let Ok(b) = std::fs::read(dir.join(format!("h{n}.state5"))) else { break };
        hops.push(decode(&b).expect("decodes"));
    }
    let mut bad = 0;
    for (n, w) in hops.windows(2).enumerate() {
        let (a, b) = (&w[0], &w[1]);
        let by_id = |m: &MemoryState| -> HashMap<(Vec<u8>, Vec<u8>, u32), usize> {
            m.files
                .iter()
                .enumerate()
                .map(|(i, f)| ((string(m, f.path), string(m, f.member_name), f.member_ordinal), i))
                .collect()
        };
        let (ia, ib) = (by_id(a), by_id(b));
        let (mut same, mut differ) = (0, std::collections::BTreeMap::<&str, usize>::new());
        for (id, &fa) in &ia {
            let Some(&fb) = ib.get(id) else { continue };
            if a.files[fa].hash != b.files[fb].hash {
                continue;
            }
            same += 1;
            let (ra, rb) = (file_rows(a, fa), file_rows(b, fb));
            if ra != rb {
                let tables: std::collections::BTreeSet<&str> = ra
                    .iter()
                    .filter(|r| !rb.contains(r))
                    .chain(rb.iter().filter(|r| !ra.contains(r)))
                    .map(|r| r.0)
                    .collect();
                for t in tables {
                    *differ.entry(t).or_default() += 1;
                }
                if bad < 5 {
                    let first = ra.iter().zip(&rb).find(|(x, y)| x != y);
                    eprintln!(
                        "hop {n}->{}: {:?}: {first:?}",
                        n + 1,
                        String::from_utf8_lossy(&id.0)
                    );
                }
                bad += 1;
            }
        }
        eprintln!("hop {n}->{}: {same} unchanged files, differing per table {differ:?}", n + 1);
    }
    assert_eq!(bad, 0, "unchanged files whose stored rows differ");
}

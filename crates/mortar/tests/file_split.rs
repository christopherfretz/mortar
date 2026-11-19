//! WP-255d3: the per-file split (`mortar::update::file_split::split_of`)
//! over a link's own format-5 state gives, for EVERY object file of the
//! link, the whole front's split of that file (phase 60's `Phase`: the
//! pieces of each split merge section, the sections placed whole, the
//! parsed `.eh_frame` records and their relocations), and the stored rows
//! agree (`FRAG_USE`: the pieces of the file's live split sections;
//! `FILES.eh_*`): `file_split_equals_the_whole_front`. The section facts
//! (kind, kept, output section) are taken FROM THE WHOLE FRONT (the
//! supervisor's ruling "After 255d3's M1"): the test proves the split, not
//! the facts. With `--gc-sections` a kept merge section that liveness drops
//! is split too (phase 60 runs before liveness).
//!
//! `one_string_edited_is_one_piece`: the same link with one string of one
//! object edited at the same size: exactly that piece differs.
//!
//! The links: the C++ program of `support` with an archive of two members
//! (with and without `--gc-sections`), the C program with an object of
//! merged strings, and the Fortran program; AArch64 and x86-64.

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::PathBuf;

use mortar::front::{Env, Front, front};
use mortar::incremental::State5;
use mortar::timing::Timer;
use mortar::update::file_front::{FrontCache, Ids, front_of};
use mortar::update::file_split::{FileSplit, SectionFact, split_of};
use mortar_core::id::{FileId, SectionId, SymbolId};
use mortar_core::inputs::Reader;
use mortar_core::par::Pool;
use mortar_core::resolution::SectionKind;
use mortar_diag::{Collector, Policy};
use mortar_elf::SectionIndex;
use mortar_elf::consts::{SHF_COMPRESSED, SHF_EXCLUDE, SHF_MERGE};
use mortar_incr::update::rows::FileTable;
use mortar_incr::update::view::StateView;
use mortar_merge::eh::{Code, EhReloc, Kind, Record, Target};
use mortar_merge::phase::ArchRelocs;
use mortar_testutil::{Scratch, run, tool, tools_required};
use support::{Driver, Source, build, build_by, with_output};

struct Line {
    _scratch: Scratch,
    label: String,
    dir: PathBuf,
    args: Vec<String>,
}

fn pool() -> Pool {
    Pool::try_new(Pool::available()).expect("a pool")
}

impl Line {
    fn args(&self, out: &str) -> Vec<OsString> {
        let mut a = with_output(&self.args, &self.dir.join(out));
        a.extend(["--incremental=full", "--incremental-state"].iter().map(OsString::from));
        a.push(format!("{out}.mortar-state").into());
        a
    }

    /// The link with the format-5 switch: its state, as the update reads it.
    fn state(&self) -> StateView<'static> {
        let path = self.dir.join("out.state5");
        let inv = mortar::invoke_state5(
            "mortar",
            &self.args("out"),
            &self.dir,
            State5 { path: path.clone(), collide: false, rows: None },
        );
        let ok = matches!(inv.outcome, mortar::Outcome::Linked { .. });
        assert!(ok, "{}: the link failed: {:?}", self.label, inv.outcome);
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|_| panic!("{}: no state: {}", self.label, inv.incremental));
        mortar_incr::state5::read::open_bytes(bytes).expect("the state opens")
    }

    /// The whole front, as an incremental-mode link with the switch.
    fn front<'s>(&self, store: &'s mortar_input::store::Store) -> Front<'s> {
        let parsed = mortar_cli::parse(&with_output(&self.args, &self.dir.join("front")));
        assert!(!parsed.failed(), "{:?}", parsed.diagnostics);
        let diag = Collector::new(Policy::default());
        let env = Env { cwd: &self.dir, pool: pool(), replay: None, slack: true, state5: true };
        let f = front(parsed.plan, env, store, &diag, &mut Timer::new(false));
        let report = diag.finish();
        assert!(!report.failed(), "{}: {report:?}", self.label);
        f.expect("the front")
    }
}

const TARGETS: [(&str, bool); 2] = [("aarch64", true), ("x86-64", false)];

fn compiler(a64: bool, lang: &str) -> String {
    match (a64, lang) {
        (true, "c++") => Driver::Gxx.tool(),
        (true, "c") => Driver::Gcc.tool(),
        (true, _) => Driver::Gfortran.tool(),
        (false, "c++") => "x86_64-linux-gnu-g++".into(),
        (false, "c") => "x86_64-linux-gnu-gcc".into(),
        (false, _) => "x86_64-linux-gnu-gfortran".into(),
    }
}

fn line_of(label: String, a64: bool, lang: &str, sources: &[Source]) -> Option<Line> {
    let s = Scratch::new(&format!("wp255d3-{}", if a64 { "a64" } else { "x64" }));
    let cc = compiler(a64, lang);
    let built = match (a64, lang) {
        (true, "c++") => build(&s, Driver::Gxx, &["-O1"], sources),
        (true, "c") => build(&s, Driver::Gcc, &["-O1"], sources),
        (true, _) => build(&s, Driver::Gfortran, &["-O1"], sources),
        (false, _) => build_by(&s, &cc, &["-O1"], sources),
    };
    let Some(b) = built else {
        // x86-64 Fortran is optional on this host; every other is required.
        assert!(!tools_required() || (!a64 && lang == "fortran"), "{cc} is required");
        eprintln!("{label}: {cc} missing: skipped");
        return None;
    };
    Some(Line { _scratch: s, label, dir: b.dir, args: b.args })
}

const M1: &str =
    "int wp255d3_m1_data = 1;\nint wp255d3_m1(int x) { return x + wp255d3_m1_data; }\n";
const M2: &str = "static volatile int wp255d3_m1_data = 3;\n\
                  int wp255d3_m2(int x) { return x * wp255d3_m1_data + 7; }\n";

/// The C++ program with `lib2.a` (two members, both extracted).
fn cxx_line(a64: bool, gc: bool) -> Option<Line> {
    let label = format!("{} g++{}", TARGETS[usize::from(!a64)].0, if gc { " gc" } else { "" });
    let mut line = line_of(label, a64, "c++", support::CXX_PROGRAM)?;
    let cc = tool(&compiler(a64, "c")).expect("the C compiler");
    for (name, text) in [("m1", M1), ("m2", M2)] {
        let src = line.dir.join(format!("{name}.c"));
        std::fs::write(&src, text).expect("source");
        let obj = line.dir.join(format!("{name}.o"));
        let args: Vec<OsString> = vec![
            "-O1".into(),
            "-c".into(),
            src.into_os_string(),
            "-o".into(),
            obj.into_os_string(),
        ];
        let o = run(&cc, &args);
        assert!(o.success(), "{}", o.stderr_text());
    }
    let ar = tool("ar").expect("ar");
    let lib = line.dir.join("lib2.a");
    let args: Vec<OsString> = vec![
        "rcs".into(),
        lib.clone().into_os_string(),
        line.dir.join("m1.o").into_os_string(),
        line.dir.join("m2.o").into_os_string(),
    ];
    let o = run(&ar, &args);
    assert!(o.success(), "ar: {}", o.stderr_text());
    line.args.extend(["-u", "wp255d3_m2", "-u", "wp255d3_m1"].map(String::from));
    line.args.push(lib.to_string_lossy().into_owned());
    if gc {
        line.args.push("--gc-sections".into());
    }
    Some(line)
}

const STRINGS: &str = "const char *wp255d3_s(int i) {\n\
    static const char *const t[] = { \"wp255d3-alpha\", \"wp255d3-beta\", \"wp255d3-gamma\" };\n\
    return t[i % 3];\n}\n";
const STRINGS_EDITED: &str = "const char *wp255d3_s(int i) {\n\
    static const char *const t[] = { \"wp255d3-alpha\", \"wp255d3-betX\", \"wp255d3-gamma\" };\n\
    return t[i % 3];\n}\n";

fn c_line(a64: bool, strings: &'static str) -> Option<Line> {
    let label = format!("{} gcc", TARGETS[usize::from(!a64)].0);
    let sources = [support::C_PROGRAM[0], Source { name: "wp255d3s.c", text: strings }];
    line_of(label, a64, "c", &sources)
}

fn fortran_line(a64: bool) -> Option<Line> {
    let label = format!("{} gfortran", TARGETS[usize::from(!a64)].0);
    line_of(label, a64, "fortran", support::FORTRAN_PROGRAM)
}

/// The whole front's facts of `file`: its `Link::section` content.
fn facts_of<'f>(
    f: &'f Front<'_>,
    file: FileId,
) -> impl Fn(SectionIndex) -> SectionFact + Sync + 'f {
    move |i| match f.resolution.section_id(file, i) {
        Some(id) => {
            let row = f.resolution.sections[id];
            SectionFact { kind: row.kind, kept: row.is_kept(), out: f.assignment.out(id) }
        }
        None => SectionFact { kind: SectionKind::Null, kept: false, out: None },
    }
}

/// A section by `(file, index)`: the state's `SECTIONS` ids and the
/// front's `SectionId`s are different numberings (the record's M2).
type At = (u32, u32);

/// A symbol by name and version (the state's keys and the front's
/// `SymbolId`s are compared by name).
type Name = (Vec<u8>, Vec<u8>);

/// A relocation target or record field with its ids made comparable.
#[derive(Debug, Clone, PartialEq, Eq)]
enum T {
    Section(At, u64),
    Absolute(u64),
    Undefined(Name, bool),
    Elsewhere(Name),
}

/// The two id spaces of one comparison.
struct Spaces<'a, 'f, 's> {
    f: &'a Front<'f>,
    view: &'a StateView<'s>,
}

impl Spaces<'_, '_, '_> {
    fn front_at(&self, s: SectionId) -> At {
        let (file, i) = self.f.resolution.sh_index(s).expect("a front section");
        (file.0, i.0)
    }

    fn state_at(&self, s: SectionId) -> At {
        let row = self.view.section(s).expect("SECTIONS");
        let first = self.view.file_index(FileId(row.file)).expect("FILE_INDEX").ranges
            [FileTable::Sections as usize]
            .first;
        (row.file, s.0 - first)
    }

    /// The front's id of a stored section.
    fn front_id(&self, s: SectionId) -> SectionId {
        let (file, i) = self.state_at(s);
        self.f.resolution.section_id(FileId(file), SectionIndex(i)).expect("a front id")
    }

    fn front_name(&self, id: SymbolId) -> Name {
        let k = &self.f.inputs.symbols[id];
        (k.name.to_vec(), k.version.unwrap_or_default().to_vec())
    }

    fn state_name(&self, id: SymbolId) -> Name {
        let row = self.view.symbol(id).expect("SYMBOLS");
        (self.view.string(row.name).expect("name"), self.view.string(row.version).expect("version"))
    }

    fn target(&self, t: Target, state: bool) -> T {
        let name = |id| if state { self.state_name(id) } else { self.front_name(id) };
        match t {
            Target::Section { section, value } => {
                let at = if state { self.state_at(section) } else { self.front_at(section) };
                T::Section(at, value)
            }
            Target::Absolute(v) => T::Absolute(v),
            Target::Undefined { symbol, weak } => T::Undefined(name(symbol), weak),
            Target::Elsewhere(id) => T::Elsewhere(name(id)),
        }
    }

    /// A record without its CIE class (numbered over the whole link in the
    /// front, over the file in `split_of`) and with its code section by
    /// `(file, index)` (in `value`'s place: `SectionId(index)` of `file`).
    fn record(&self, r: &Record, state: bool) -> (Record, Option<At>) {
        let mut r = *r;
        let mut at = None;
        match &mut r.kind {
            Kind::Cie(c) => c.class = 0,
            Kind::Fde(fde) => {
                if let Code::Section { section, .. } = &mut fde.code {
                    at =
                        Some(if state { self.state_at(*section) } else { self.front_at(*section) });
                    *section = SectionId(0);
                }
            }
        }
        (r, at)
    }

    fn reloc(&self, r: &EhReloc, state: bool) -> (u64, u32, i64, u8, T) {
        (r.offset, r.r_type, r.addend, r.width, self.target(r.target, state))
    }
}

#[derive(Debug, Default)]
struct Seen {
    files: u32,
    merge_sections: u32,
    pieces: u32,
    whole: u32,
    eh_sections: u32,
    records: u32,
    eh_relocs: u32,
    frag_uses: u32,
    dead_split: u32,
    /// `FILES.eh_records` summed (checked exactly per file).
    eh_kept: u32,
}

/// The split of one object file against the whole front's and the rows.
#[allow(clippy::too_many_lines, reason = "one comparison after the other")]
fn compare(
    line: &Line,
    f: &Front<'_>,
    view: &StateView<'_>,
    cache: &mut FrontCache<'_>,
    file: FileId,
    seen: &mut Seen,
) -> FileSplit {
    let what = |s: &str| {
        let p = String::from_utf8_lossy(f.inputs.files[file].path);
        format!("{}: file {} ({p}): {s}", line.label, file.0)
    };
    let ff = front_of(cache, view, file, Ids::Stored).unwrap_or_else(|e| panic!("{e:?}"));
    let facts = facts_of(f, file);
    let arch = f.arch.get();
    let got = split_of(ff, view, &facts, &ArchRelocs(arch), &f.pool, None)
        .unwrap_or_else(|e| panic!("{}", what(&format!("{e:?}"))));
    seen.files += 1;
    let x = Spaces { f, view };
    let Reader::Object(obj) = f.inputs.files[file].reader else { panic!("not an object") };
    let ids: BTreeSet<SectionId> = (0..obj.section_count())
        .filter_map(|k| f.resolution.section_id(file, SectionIndex(k)))
        .collect();
    let split = &f.merge.split;

    // The pieces: the whole front's `merge_of` rows of the file's sections.
    let mut want = Vec::new();
    let mut frag_of = BTreeMap::new();
    for row in split.merge_of.iter().filter(|r| ids.contains(&r.section)) {
        seen.merge_sections += 1;
        for p in row.pieces.slots() {
            frag_of.insert((row.section, u64::from(split.piece_off[p])), split.piece_frag[p]);
            let frag = split.frags[split.piece_frag[p]];
            let at = x.front_at(row.section);
            want.push((at, u64::from(split.piece_off[p]), u64::from(frag.size)));
        }
    }
    let have: Vec<_> =
        got.pieces.iter().map(|p| (x.state_at(p.section), p.offset, p.size)).collect();
    assert_eq!(have, want, "{}", what("pieces"));
    seen.pieces += have.len() as u32;
    for p in &got.pieces {
        let kept = f.assignment.out(x.front_id(p.section));
        assert_eq!(Some(p.out), kept, "{}", what("a piece's output section"));
    }
    let whole: Vec<_> = f
        .merge
        .whole
        .iter()
        .filter(|(s, _)| ids.contains(s))
        .map(|&(s, w)| (x.front_at(s), w))
        .collect();
    let got_whole: Vec<_> = got.whole.iter().map(|&(s, w)| (x.state_at(s), w)).collect();
    assert_eq!(got_whole, whole, "{}", what("placed whole"));
    seen.whole += whole.len() as u32;

    // `.eh_frame`: the whole front's sections, records and relocations.
    let eh = &f.merge.eh;
    let rows: Vec<usize> =
        (0..eh.sections.len()).filter(|&i| ids.contains(&eh.sections[i].section)).collect();
    assert_eq!(got.eh.sections.len(), rows.len(), "{}", what("eh sections"));
    for (k, &i) in rows.iter().enumerate() {
        let (w, g) = (&eh.sections[i], &got.eh.sections[k]);
        assert_eq!(
            (x.state_at(g.section), g.size, g.terminator),
            (x.front_at(w.section), w.size, w.terminator),
            "{}",
            what("eh section")
        );
        let wr: Vec<_> = eh.records[w.records.slots()].iter().map(|r| x.record(r, false)).collect();
        let gr: Vec<_> =
            got.eh.records[g.records.slots()].iter().map(|r| x.record(r, true)).collect();
        assert_eq!(gr, wr, "{}", what("eh records"));
        let wl: Vec<_> = f.merge.eh_relocs[i].iter().map(|r| x.reloc(r, false)).collect();
        let gl: Vec<_> = got.eh_relocs[k].iter().map(|r| x.reloc(r, true)).collect();
        assert_eq!(gl, wl, "{}", what("eh relocations"));
        seen.eh_sections += 1;
        seen.records += gr.len() as u32;
        seen.eh_relocs += gl.len() as u32;
    }

    // The stored rows. `FRAG_USE`: one row per (out, class, hash) of the
    // pieces of the file's LIVE split sections whose fragment is live
    // (`Liveness::frag_live`), `.debug_*` left out (the capture's rule).
    let mut from_split = BTreeSet::new();
    let mut dead: BTreeMap<SectionId, u32> = BTreeMap::new();
    for p in &got.pieces {
        let id = x.front_id(p.section);
        if !f.liveness.is_live(id) {
            *dead.entry(p.section).or_insert(0) += 1;
            continue;
        }
        if !f.liveness.frag_live[frag_of[&(id, p.offset)]] {
            continue;
        }
        let (_, i) = f.resolution.sh_index(id).expect("index");
        if obj.section(i).expect("section").name().expect("name").starts_with(b".debug_") {
            continue;
        }
        from_split.insert((p.out.0, p.class.word(), p.hash));
    }
    let stored: BTreeSet<_> = view
        .frag_uses_of(file)
        .expect("FRAG_USE")
        .iter()
        .map(|r| (r.out, r.class, r.hash))
        .collect();
    assert_eq!(from_split, stored, "{}", what("FRAG_USE"));
    seen.frag_uses += stored.len() as u32;
    // A kept merge section that liveness drops is split all the same.
    seen.dead_split += dead.len() as u32;
    // `FILES.eh_*`, exactly (S-41, S-42, S-44 with IN-34a's per-file
    // CIEs): the split's FDEs whose code section is live in the whole
    // front (no `--icf` here: nothing is folded), and one CIE per identity
    // class (the split numbers them over the file) that a live FDE uses.
    let (mut fdes, mut classes) = (0u32, BTreeSet::new());
    for g in &got.eh.sections {
        let records = &got.eh.records[g.records.slots()];
        for r in records {
            let Some(fde) = r.fde() else { continue };
            let Code::Section { section, .. } = fde.code else { continue };
            if !f.liveness.is_live(x.front_id(section)) {
                continue;
            }
            fdes += 1;
            let cie = records[fde.cie as usize].cie().expect("an FDE's CIE");
            classes.insert(cie.class);
        }
    }
    let records = fdes + classes.len() as u32;
    let row = &ff.row;
    assert_eq!((row.eh_records, row.eh_fdes), (records, fdes), "{}", what("FILES.eh_*"));
    if records == 0 {
        assert_eq!(row.eh_capacity, 0, "{}", what("FILES.eh_* empty"));
    }
    seen.eh_kept += records;
    got
}

fn objects(f: &Front<'_>) -> Vec<FileId> {
    (1..f.inputs.files.len())
        .map(FileId)
        .filter(|&id| matches!(f.inputs.files[id].reader, Reader::Object(_)))
        .collect()
}

fn check(line: &Line) -> Seen {
    let view = line.state();
    let store = support::store();
    let f = line.front(&store);
    let fresh = support::store();
    let mut cache = FrontCache::new(&fresh, pool(), &line.dir);
    let mut seen = Seen::default();
    for file in objects(&f) {
        compare(line, &f, &view, &mut cache, file, &mut seen);
    }
    eprintln!("{}: {seen:?}", line.label);
    assert!(seen.pieces > 0 && seen.records > 0 && seen.frag_uses > 0, "{}: {seen:?}", line.label);
    assert!(seen.eh_kept > 0, "{}: no kept .eh_frame record", line.label);
    seen
}

#[test]
fn file_split_equals_the_whole_front() {
    for (_, a64) in TARGETS {
        for gc in [false, true] {
            let Some(line) = cxx_line(a64, gc) else { continue };
            let seen = check(&line);
            assert!(seen.eh_relocs > 0 && seen.merge_sections > 0, "{}: {seen:?}", line.label);
            if gc {
                assert!(seen.dead_split > 0, "{}: no dead merge section split", line.label);
            } else {
                assert_eq!(seen.dead_split, 0, "{}: dead without --gc-sections", line.label);
            }
        }
        if let Some(line) = c_line(a64, STRINGS) {
            check(&line);
        }
        if let Some(line) = fortran_line(a64) {
            check(&line);
        }
    }
}

/// The split of the strings object of `line`.
fn strings_split(line: &Line) -> FileSplit {
    let view = line.state();
    let store = support::store();
    let f = line.front(&store);
    let fresh = support::store();
    let mut cache = FrontCache::new(&fresh, pool(), &line.dir);
    let file = objects(&f)
        .into_iter()
        .find(|&id| f.inputs.files[id].path.ends_with(b"wp255d3s.c.o"))
        .expect("the strings object");
    compare(line, &f, &view, &mut cache, file, &mut Seen::default())
}

#[test]
fn one_string_edited_is_one_piece() {
    for (_, a64) in TARGETS {
        let (Some(a), Some(b)) = (c_line(a64, STRINGS), c_line(a64, STRINGS_EDITED)) else {
            continue;
        };
        let (x, y) = (strings_split(&a), strings_split(&b));
        assert_eq!(x.pieces.len(), y.pieces.len(), "{}: piece count", a.label);
        let mut differ = 0;
        for (p, q) in x.pieces.iter().zip(&y.pieces) {
            assert_eq!(
                (p.offset, p.size, p.out, p.class),
                (q.offset, q.size, q.out, q.class),
                "{}: a piece moved",
                a.label
            );
            differ += u32::from(p.hash != q.hash);
        }
        assert_eq!(differ, 1, "{}: {differ} pieces differ", a.label);
        assert!(x.pieces.len() >= 3, "{}: the strings are pieces", a.label);
    }
}

/// S-6's uncompressed shapes, each an `SHF_MERGE` section with
/// `sh_entsize > 0` that is placed whole: writable, code, TLS, the target
/// of a relocation section, empty, `sh_entsize` above 2^16; and an
/// `SHF_EXCLUDE` section with a relocation section of its own (`.rela`
/// is never `Excluded`, resolve.md §10: the `Reloc` row is first; it
/// applies to nothing, `reloc_fate`).
const S6: &str = r#"int wp255d2d_sym = 5;
__asm__(".section .rodata.wp255d2d_w,\"awMS\",%progbits,1\n.asciz \"w1\"\n"
        ".section .text.wp255d2d_x,\"axMS\",%progbits,1\n.asciz \"x1\"\n"
        ".section .tdata.wp255d2d_t,\"aTM\",%progbits,4\n.word 7\n"
        ".section .rodata.wp255d2d_r,\"aM\",%progbits,8\n.quad wp255d2d_sym\n"
        ".section .rodata.wp255d2d_e,\"aMS\",%progbits,1\n"
        ".section .rodata.wp255d2d_b,\"aM\",%progbits,131072\n.zero 131072\n"
        ".section .wp255d2d_ex,\"e\",%progbits\n.quad wp255d2d_sym\n"
        ".text\n");
int wp255d2d_use(void) { return wp255d2d_sym; }
"#;

/// The names `S6` gives its sections, and the kind the whole link gives each.
const S6_KINDS: [(&[u8], SectionKind); 7] = [
    (b".rodata.wp255d2d_w", SectionKind::Regular),
    (b".text.wp255d2d_x", SectionKind::Regular),
    (b".tdata.wp255d2d_t", SectionKind::Regular),
    (b".rodata.wp255d2d_r", SectionKind::Regular),
    (b".rodata.wp255d2d_e", SectionKind::Regular),
    (b".rodata.wp255d2d_b", SectionKind::Regular),
    (b".wp255d2d_ex", SectionKind::Excluded),
];

/// D3-5: `Unit::Whole` is unreachable for an uncompressed merge section
/// (S-6 makes every such section `Regular` in resolve, so phase 60 never
/// meets it), and a relocation section applying to an excluded section
/// changes nothing in the split. The whole front's kinds by name; the
/// split of every object equal to the whole front's (`compare`), with no
/// section placed whole.
#[test]
fn merge_sections_placed_whole_are_regular() {
    for (_, a64) in TARGETS {
        let label = format!("{} gcc S-6", TARGETS[usize::from(!a64)].0);
        let sources = [support::C_PROGRAM[0], Source { name: "wp255d2d6.c", text: S6 }];
        let Some(line) = line_of(label, a64, "c", &sources) else { continue };
        let store = support::store();
        let f = line.front(&store);
        let file = objects(&f)
            .into_iter()
            .find(|&id| f.inputs.files[id].path.ends_with(b"wp255d2d6.c.o"))
            .expect("the S-6 object");
        let Reader::Object(obj) = f.inputs.files[file].reader else { panic!("not an object") };
        let mut found = 0;
        let mut relocs = 0;
        for k in 1..obj.section_count() {
            let s = obj.section(SectionIndex(k)).expect("section");
            let name = s.name().expect("name");
            let kind = f.resolution.section_id(file, SectionIndex(k)).map(|id| {
                let row = f.resolution.sections[id];
                (row.kind, row.reloc)
            });
            if let Some(&(_, want)) = S6_KINDS.iter().find(|(n, _)| *n == name) {
                // As written: `SHF_MERGE` with `sh_entsize > 0` (`Merge` by
                // §10's first rows), or `SHF_EXCLUDE`.
                let merge = s.flags() & SHF_MERGE != 0 && s.header().sh_entsize > 0;
                let shape = if want == SectionKind::Excluded {
                    s.flags() & SHF_EXCLUDE != 0
                } else {
                    merge && s.flags() & SHF_COMPRESSED == 0
                };
                assert!(shape, "{}: {} as written", line.label, String::from_utf8_lossy(name));
                let (kind, reloc) = kind.expect("a row");
                assert_eq!(kind, want, "{}: {}", line.label, String::from_utf8_lossy(name));
                // The relocated one is S-6's by its relocation section;
                // the excluded one's applies to nothing (`reloc_fate`).
                let relocated = name == b".rodata.wp255d2d_r";
                assert_eq!(reloc.0 != 0, relocated, "{}: {:?}'s reloc", line.label, name);
                found += 1;
            }
            if name == b".rela.wp255d2d_ex" {
                let (kind, _) = kind.expect("a row");
                assert_eq!(
                    kind,
                    SectionKind::Reloc,
                    "{}: the excluded section's .rela",
                    line.label
                );
                relocs += 1;
            }
        }
        assert_eq!((found, relocs), (S6_KINDS.len(), 1), "{}: the S-6 sections", line.label);
        drop(f);
        let seen = check(&line);
        assert_eq!(seen.whole, 0, "{}: a section placed whole reached phase 60", line.label);
    }
}

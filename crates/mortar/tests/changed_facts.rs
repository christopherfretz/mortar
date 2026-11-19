//! WP-255d2c: a CHANGED file's section facts from their ONE producers
//! (`mortar::update::file_split::changed_facts`: resolve's per-object body
//! with the update's keepers, `assign::fate` with the state's `OUTSECS`),
//! and `split_of` on the changed file with THOSE facts.
//!
//! `changed_facts_and_split_of_an_edited_file`: `m1.o` of `lib2.a` edited
//! into C++ with string literals (a merge section), a COMDAT group kept
//! (`wp255d_inl3`, only here) and one dropped (`wp255d_inl`, kept by the
//! earlier member `m8.o`), and `.eh_frame`; opened through the OLD state
//! with `Ids::Update`'s new `FILES` row. The keepers are the full link's of
//! the new inputs (D2-6's producer is checked against the model's full
//! link in `mortar_incr`'s `update_resolve`). Each fact equals the whole
//! front's `Link::section` content for that section (kind, kept, output
//! section by NAME: the update's id is the state's `OUTSECS` row); the
//! split equals the whole front's pieces, sections placed whole,
//! `.eh_frame` sections, records and relocations for that file.
//!
//! On AArch64 and x86-64.
#![allow(clippy::too_many_lines, reason = "one comparison after the other")]

mod support;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

use mortar::front::{Env, Front, front};
use mortar::incremental::State5;
use mortar::timing::Timer;
use mortar::update::file_front::{FrontCache, Ids, front_of};
use mortar::update::file_split::{SectionFact, changed_facts, split_of};
use mortar_core::OutSecId;
use mortar_core::id::{FileId, SectionId, SymbolId};
use mortar_core::identity::Anchor;
use mortar_core::par::Pool;
use mortar_core::resolution::SectionKind;
use mortar_diag::{Collector, Policy};
use mortar_elf::SectionIndex;
use mortar_incr::update::rows::{FileRow, FileTable};
use mortar_incr::update::view::StateView;
use mortar_merge::eh::{Code, Kind, Record, Target};
use mortar_merge::phase::ArchRelocs;
use mortar_testutil::{Scratch, run, tool, tools_required};
use support::{Driver, build, build_by, with_output};

/// The edited `m1.o` (C++): strings (`.rodata.str1.1`, a merge section),
/// `wp255d_inl` (a COMDAT copy `m8.o` keeps: dropped here), `wp255d_inl3`
/// (kept here), `.eh_frame`.
const M1_SPLIT: &str = "extern \"C\" { int wp255d_m1_data = 1; }\n\
    inline __attribute__((noinline)) int wp255d_inl(int x) { return x * 3 + wp255d_m1_data; }\n\
    inline __attribute__((noinline)) int wp255d_inl3(int x) { return x * 7 + wp255d_m1_data; }\n\
    extern \"C\" const char *wp255d_str(int i) { static const char *const t[] = \
    { \"wp255d2c-alpha\", \"wp255d2c-beta\", \"wp255d2c-gamma\" }; return t[i % 3]; }\n\
    extern \"C\" int wp255d_m1(int x) { return wp255d_inl(x) + wp255d_inl3(x) + *wp255d_str(x); }\n";
/// The earlier member with the same `wp255d_inl` (same tokens: one ODR
/// definition): it keeps the group.
const M8: &str = "extern \"C\" { extern int wp255d_m1_data; }\n\
    inline __attribute__((noinline)) int wp255d_inl(int x) { return x * 3 + wp255d_m1_data; }\n\
    extern \"C\" int wp255d_m8(int x) { return wp255d_inl(x) + 8; }\n";

const M1: &str = "int wp255d_m1_data = 1;\nint wp255d_m1(int x) { return x + wp255d_m1_data; }\n";
const M2: &str = "static volatile int wp255d_m1_data = 3;\n\
                  int wp255d_m2(int x) { return x * wp255d_m1_data + 7; }\n";
/// `m1.o` edited (C++): `wp255d_m1` grown, a renamed function, an added
/// section, an added COMDAT group (`inline`, `noinline` so -O1 keeps it, used).
const M1_EDITED: &str = "extern \"C\" { int wp255d_m1_data = 1; }\n\
    inline __attribute__((noinline)) int wp255d_inl(int x) { return x * 3 + wp255d_m1_data; }\n\
    extern \"C\" int wp255d_m1_renamed(int x) { return x ^ 5; }\n\
    extern \"C\" __attribute__((section(\".text.wp255d2_added\"))) int wp255d_added(int x) \
    { return x + 11; }\n\
    extern \"C\" int wp255d_m1(int x) { int s = 0; for (int i = 0; i < x; i++) s += i * x; \
    return s + wp255d_inl(x) + wp255d_m1_renamed(x) + wp255d_added(x); }\n";
/// A member with versioned definitions and the three keys without a row.
const M4: &str = "__asm__(\".symver wp255d_v1, wp255d_ver@V1\");\n\
    __asm__(\".symver wp255d_v2, wp255d_ver@@V2\");\n\
    int wp255d_v1(void) { return 1; }\n\
    int wp255d_v2(void) { return 2; }\n\
    extern char _end[];\n\
    extern int _DYNAMIC[] __attribute__((weak));\n\
    extern int wp255d_nowhere __attribute__((weak));\n\
    long wp255d_m4(void) { return (long)_end + (_DYNAMIC != 0) + (&wp255d_nowhere != 0); }\n";
/// A member whose string section's last string is not terminated (S-8: the
/// full link refuses it; the split meets it as a problem).
const M6: &str = "__asm__(\".section .rodata.wp255d_str,\\\"aMS\\\",%progbits,1\\n\"\n\
    \"\\t.ascii \\\"ab\\\"\\n\\t.text\\n\");\n\
    int wp255d_m6(int x) { return x + 6; }\n";
/// An `SHF_LINK_ORDER` section outside the groups, linked to a COMDAT member.
const M3: &str = "__asm__(\".section .text.wp255d_g,\\\"axG\\\",%progbits,wp255d_g,comdat\\n\"\n\
    \"\\t.globl wp255d_g\\nwp255d_g:\\n\\tret\\n\"\n\
    \"\\t.section .wp255d_lo,\\\"ao\\\",%progbits,.text.wp255d_g\\n\\t.byte 1\\n\\t.text\\n\");\n\
    int wp255d_m3(int x) { return x + 2; }\n";

struct Line {
    _scratch: Scratch,
    label: String,
    dir: PathBuf,
    /// The C++ program's link line, without `lib2.a` and its `-u`s.
    base: Vec<String>,
    cc: PathBuf,
}

impl Line {
    fn compile(&self, name: &str, lang: &str, text: &str) {
        let src = self.dir.join(format!("{name}.src"));
        std::fs::write(&src, text).expect("source");
        let obj = self.dir.join(format!("{name}.o"));
        let args: Vec<OsString> = vec![
            "-x".into(),
            lang.into(),
            "-O1".into(),
            "-c".into(),
            src.into_os_string(),
            "-o".into(),
            obj.into_os_string(),
        ];
        let o = run(&self.cc, &args);
        assert!(o.success(), "{}: {name}: {}", self.label, o.stderr_text());
    }

    fn archive(&self, members: &[&str]) {
        let ar = tool("ar").expect("ar");
        let lib = self.dir.join("lib2.a");
        let _ = std::fs::remove_file(&lib);
        let mut args: Vec<OsString> = vec!["rcs".into(), lib.into_os_string()];
        args.extend(members.iter().map(|m| self.dir.join(m).into_os_string()));
        let o = run(&ar, &args);
        assert!(o.success(), "ar: {}", o.stderr_text());
    }

    /// The line with `-u` for each of `roots`, then `lib2.a`.
    fn args(&self, roots: &[&str]) -> Vec<String> {
        let mut a = self.base.clone();
        for r in roots {
            a.push("-u".into());
            a.push((*r).into());
        }
        a.push(self.dir.join("lib2.a").to_string_lossy().into_owned());
        a
    }
}

fn lines() -> Vec<Line> {
    let mut v = Vec::new();
    for (label, cc, a64) in [
        ("aarch64 g++", Driver::Gxx.tool(), true),
        ("x86-64 g++", "x86_64-linux-gnu-g++".into(), false),
    ] {
        let s = Scratch::new(if a64 { "wp255d2-a64" } else { "wp255d2-x64" });
        let built = if a64 {
            build(&s, Driver::Gxx, &["-O1"], support::CXX_PROGRAM)
        } else {
            build_by(&s, &cc, &["-O1"], support::CXX_PROGRAM)
        };
        let Some(b) = built else {
            assert!(!tools_required(), "{cc} is required");
            continue;
        };
        let cc = tool(&cc).expect("the compiler");
        let line = Line { _scratch: s, label: label.into(), dir: b.dir, base: b.args, cc };
        line.compile("m1", "c", M1);
        line.compile("m2", "c", M2);
        line.compile("m1e", "c++", M1_EDITED);
        line.compile("m3", "c", M3);
        line.compile("m4", "c", M4);
        line.compile("m6", "c", M6);
        line.compile("m1s", "c++", M1_SPLIT);
        line.compile("m8", "c++", M8);
        v.push(line);
    }
    v
}

fn pool() -> Pool {
    Pool::try_new(Pool::available()).expect("a pool")
}

/// The link of `args` with the format-5 switch; its state as the update
/// reads it.
fn state(line: &Line, args: &[String], out: &str) -> StateView<'static> {
    let mut a = with_output(args, &line.dir.join(out));
    a.extend(["--incremental=full", "--incremental-state"].iter().map(OsString::from));
    a.push(format!("{out}.mortar-state").into());
    let path = line.dir.join(format!("{out}.state5"));
    let inv = mortar::invoke_state5(
        "mortar",
        &a,
        &line.dir,
        State5 { path: path.clone(), collide: false, rows: None },
    );
    let ok = matches!(inv.outcome, mortar::Outcome::Linked { .. });
    assert!(ok, "{}: the link failed: {:?}", line.label, inv.outcome);
    let bytes = std::fs::read(&path).expect("the state");
    mortar_incr::state5::read::open_bytes(bytes).expect("the state opens")
}

/// The whole front of `args` (incremental mode, the switch).
fn whole<'s>(line: &Line, args: &[String], store: &'s mortar_input::store::Store) -> Front<'s> {
    let parsed = mortar_cli::parse(&with_output(args, &line.dir.join("front")));
    assert!(!parsed.failed(), "{:?}", parsed.diagnostics);
    let diag = Collector::new(Policy::default());
    let env = Env { cwd: &line.dir, pool: pool(), replay: None, slack: true, state5: true };
    let f = front(parsed.plan, env, store, &diag, &mut Timer::new(false));
    let report = diag.finish();
    assert!(!report.failed(), "{}: {report:?}", line.label);
    f.expect("the front")
}

/// The member of `lib2.a` whose name ends in `name`, in `view`.
fn member(view: &StateView<'_>, name: &str) -> FileId {
    let files = u32::try_from(view.count(mortar_incr::format::kind::FILES)).expect("FILES");
    (1..files)
        .map(FileId)
        .find(|&id| {
            let r = view.file(id).expect("FILES");
            r.member != 0
                && view.string(r.path).expect("path").ends_with(b"lib2.a")
                && view.string(r.member_name).expect("name").ends_with(name.as_bytes())
        })
        .unwrap_or_else(|| panic!("no member {name}"))
}

/// The front's file of the same path and member name as `file` of `view`.
fn same_file(f: &Front<'_>, view: &StateView<'_>, file: FileId) -> FileId {
    let r = view.file(file).expect("FILES");
    let (path, name) = (view.string(r.path).expect("path"), view.string(r.member_name).expect("m"));
    (1..f.inputs.files.len())
        .map(FileId)
        .find(|&id| {
            let x = &f.inputs.files[id];
            x.path == &path[..] && x.member.map_or(&b""[..], |m| m.name) == &name[..]
        })
        .expect("the file in the new front")
}

/// The state's file of the front's `file` (same path and member name).
fn state_file(f: &Front<'_>, view: &StateView<'_>, file: FileId) -> Option<FileId> {
    let x = &f.inputs.files[file];
    let member = x.member.map_or(&b""[..], |m| m.name);
    let files = u32::try_from(view.count(mortar_incr::format::kind::FILES)).expect("FILES");
    (1..files).map(FileId).find(|&id| {
        let r = view.file(id).expect("FILES");
        view.string(r.path).expect("path") == x.path
            && view.string(r.member_name).expect("member") == member
    })
}

/// A section by (state file, index): the update's ids of the changed file
/// (`first + k`), the stored ones of the others, the front's.
type At = (u32, u32);

#[derive(Debug, Clone, PartialEq, Eq)]
enum T {
    Section(At, u64),
    Absolute(u64),
    Undefined(Vec<u8>, bool),
    Elsewhere(Vec<u8>),
}

struct Ids2<'a, 'f, 's> {
    f: &'a Front<'f>,
    view: &'a StateView<'s>,
    file: FileId,
    first: u32,
}

impl Ids2<'_, '_, '_> {
    fn update_at(&self, s: SectionId) -> At {
        if s.0 >= self.first {
            return (self.file.0, s.0 - self.first);
        }
        let row = self.view.section(s).expect("SECTIONS");
        let first = self.view.file_index(FileId(row.file)).expect("FILE_INDEX").ranges
            [FileTable::Sections as usize]
            .first;
        (row.file, s.0 - first)
    }

    fn front_at(&self, s: SectionId) -> At {
        let (file, i) = self.f.resolution.sh_index(s).expect("a front section");
        (state_file(self.f, self.view, file).expect("the file in the state").0, i.0)
    }

    fn name(&self, id: SymbolId, update: bool) -> Vec<u8> {
        if update {
            let r = self.view.symbol(id).expect("SYMBOLS");
            self.view.string(r.name).expect("name")
        } else {
            self.f.inputs.symbols[id].name.to_vec()
        }
    }

    fn target(&self, t: Target, update: bool) -> T {
        match t {
            Target::Section { section, value } => {
                let at = if update { self.update_at(section) } else { self.front_at(section) };
                T::Section(at, value)
            }
            Target::Absolute(v) => T::Absolute(v),
            Target::Undefined { symbol, weak } => T::Undefined(self.name(symbol, update), weak),
            Target::Elsewhere(id) => T::Elsewhere(self.name(id, update)),
        }
    }

    fn record(&self, r: &Record, update: bool) -> (Record, Option<At>) {
        let mut r = *r;
        let mut at = None;
        match &mut r.kind {
            Kind::Cie(c) => c.class = 0,
            Kind::Fde(fde) => {
                if let Code::Section { section, .. } = &mut fde.code {
                    at = Some(if update {
                        self.update_at(*section)
                    } else {
                        self.front_at(*section)
                    });
                    *section = SectionId(0);
                }
            }
        }
        (r, at)
    }
}

#[test]
fn changed_facts_and_split_of_an_edited_file() {
    for line in lines() {
        let args = line.args(&["wp255d_m8", "wp255d_m2", "wp255d_m1"]);
        line.archive(&["m8.o", "m1.o", "m2.o"]);
        let old = state(&line, &args, "old");
        std::fs::copy(line.dir.join("m1s.o"), line.dir.join("m1.o")).expect("the edit");
        line.archive(&["m8.o", "m1.o", "m2.o"]);
        let new = state(&line, &args, "new");
        let store = support::store();
        let f = whole(&line, &args, &store);
        let file = member(&old, "m1.o");
        let captured = new.file(member(&new, "m1.o")).expect("FILES");
        let stored = old.file(file).expect("FILES");
        let row = FileRow { base: captured.base, len: captured.len, hash: captured.hash, ..stored };
        let first = first_free(&old);
        let fresh = support::store();
        let mut cache = FrontCache::new(&fresh, pool(), &line.dir);
        let ff =
            front_of(&mut cache, &old, file, Ids::Update { first_section: SectionId(first), row })
                .unwrap_or_else(|e| panic!("{}: {e:?}", line.label));
        let nf = same_file(&f, &old, file);
        // The keepers: the full link's of the new inputs, in the state's
        // file ids, of every signature the file's sections name.
        let mut keepers: Vec<(Vec<u8>, Option<FileId>)> = Vec::new();
        for s in &ff.sections {
            let Anchor::Comdat(sig) = s.anchor else { continue };
            if keepers.iter().any(|k| k.0 == sig) {
                continue;
            }
            let id = (0..f.inputs.comdats.len())
                .map(mortar_core::ComdatId)
                .find(|&c| f.inputs.comdats[c].name == sig)
                .expect("the signature in the new front");
            let k = f.resolution.comdat[id].get().map(|k| state_file(&f, &old, k).expect("keeper"));
            keepers.push((sig.to_vec(), k));
        }
        keepers.sort();
        let facts = changed_facts(ff, &old, &f.plan, &keepers)
            .unwrap_or_else(|e| panic!("{}: {e:?}", line.label));
        let what = |s: &str| format!("{}: m1.o: {s}", line.label);
        // Each fact against the whole front's `Link::section` content.
        let outsecs = old.outsecs().expect("OUTSECS");
        let state_out = |o: OutSecId| old.string(outsecs[o.0 as usize].name).expect("name");
        let (mut dropped, mut kept_comdat, mut same_ids) = (0, 0, true);
        assert_eq!(facts.len() as u32, ff.object.section_count(), "{}", what("one fact per index"));
        for (i, fact) in facts.iter().enumerate() {
            let id = f.resolution.section_id(nf, SectionIndex(i as u32)).expect("a front id");
            let r = f.resolution.sections[id];
            let want_out = f.assignment.out(id);
            assert_eq!(
                (fact.kind, fact.kept),
                (r.kind, r.is_kept()),
                "{}",
                what(&format!("section {i}"))
            );
            assert_eq!(
                fact.out.map(state_out),
                want_out.map(|o| f.assignment.outsecs[o].name.to_vec()),
                "{}",
                what(&format!("section {i}: output section"))
            );
            same_ids &= fact.out == want_out;
            if matches!(ff.sections[i].anchor, Anchor::Comdat(_)) {
                if fact.kept { kept_comdat += 1 } else { dropped += 1 }
            }
        }
        let null = SectionFact { kind: SectionKind::Null, kept: false, out: None };
        let by_index = |i: SectionIndex| facts.get(i.0 as usize).copied().unwrap_or(null);
        let got = split_of(ff, &old, &by_index, &ArchRelocs(f.arch.get()), &f.pool)
            .unwrap_or_else(|e| panic!("{}", what(&format!("{e:?}"))));
        let x = Ids2 { f: &f, view: &old, file, first };
        let range = f.resolution.section_base[nf];
        let mine = |s: SectionId| s.0 >= range.start.0 && s.0 < range.start.0 + range.len();
        let split = &f.merge.split;
        let mut want = Vec::new();
        for row in split.merge_of.iter().filter(|r| mine(r.section)) {
            for p in row.pieces.slots() {
                let frag = split.frags[split.piece_frag[p]];
                let out =
                    f.assignment.out(row.section).map(|o| f.assignment.outsecs[o].name.to_vec());
                want.push((
                    x.front_at(row.section),
                    u64::from(split.piece_off[p]),
                    u64::from(frag.size),
                    out,
                ));
            }
        }
        let have: Vec<_> = got
            .pieces
            .iter()
            .map(|p| (x.update_at(p.section), p.offset, p.size, Some(state_out(p.out))))
            .collect();
        assert_eq!(have, want, "{}", what("pieces"));
        let whole_want: Vec<_> = f
            .merge
            .whole
            .iter()
            .filter(|(s, _)| mine(*s))
            .map(|&(s, w)| (x.front_at(s), w))
            .collect();
        let whole_got: Vec<_> = got.whole.iter().map(|&(s, w)| (x.update_at(s), w)).collect();
        assert_eq!(whole_got, whole_want, "{}", what("placed whole"));
        let eh = &f.merge.eh;
        let rows: Vec<usize> =
            (0..eh.sections.len()).filter(|&i| mine(eh.sections[i].section)).collect();
        assert_eq!(got.eh.sections.len(), rows.len(), "{}", what("eh sections"));
        let mut records = 0;
        for (k, &i) in rows.iter().enumerate() {
            let (w, g) = (&eh.sections[i], &got.eh.sections[k]);
            assert_eq!(
                (x.update_at(g.section), g.size, g.terminator),
                (x.front_at(w.section), w.size, w.terminator),
                "{}",
                what("eh section")
            );
            let wr: Vec<_> =
                eh.records[w.records.slots()].iter().map(|r| x.record(r, false)).collect();
            let gr: Vec<_> =
                got.eh.records[g.records.slots()].iter().map(|r| x.record(r, true)).collect();
            assert_eq!(gr, wr, "{}", what("eh records"));
            let rel = |r: &mortar_merge::eh::EhReloc, u: bool| {
                (r.offset, r.r_type, r.addend, r.width, x.target(r.target, u))
            };
            let wl: Vec<_> = f.merge.eh_relocs[i].iter().map(|r| rel(r, false)).collect();
            let gl: Vec<_> = got.eh_relocs[k].iter().map(|r| rel(r, true)).collect();
            assert_eq!(gl, wl, "{}", what("eh relocations"));
            records += gr.len();
        }
        let seen: BTreeMap<&str, usize> = [
            ("sections", facts.len()),
            ("comdat dropped", dropped),
            ("comdat kept", kept_comdat),
            ("pieces", have.len()),
            ("whole", whole_got.len()),
            ("eh sections", rows.len()),
            ("eh records", records),
        ]
        .into_iter()
        .collect();
        eprintln!(
            "{}: m1.o edited: {seen:?}; output section ids equal the full link's: {same_ids}",
            line.label
        );
        assert!(dropped > 0 && kept_comdat > 0, "{}", what("a kept and a dropped COMDAT copy"));
        assert!(!have.is_empty() && records > 0, "{}", what("pieces and .eh_frame records"));
    }
}

fn first_free(view: &StateView<'_>) -> u32 {
    u32::try_from(view.count(mortar_incr::format::kind::SECTIONS)).expect("SECTIONS")
}

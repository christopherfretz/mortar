//! WP-255d2: `front_of` with `Ids::Update` (a changed or newly extracted
//! file in the update's id space), against the whole front of the NEW
//! inputs for that file, matched by identity (sections by index, keys by
//! name): `capture::file_rows` of the new link's front.
//!
//! - `update_front_of_an_extracted_member`: a member of `lib2.a` the old
//!   link did not extract, extracted by the new link (`-u`): the OLD
//!   state's `FILES` row (its bytes did not change), `Ids::Update`.
//! - `update_front_of_edited_files`: `m1.o` edited (a function renamed, one
//!   grown, a section added, a COMDAT group added: C++ `inline`), and the
//!   member with versioned definitions (`foo@V1`, `foo@@V2`), through the
//!   NEW link's state (`front_of` takes the file's path, `base`, `len` and
//!   hash from its view's `FILES` row: for edited bytes only a state that
//!   holds them names them; see the record, finding F-1).
//! - `update_link_order_outside_groups_falls_back`: an `SHF_LINK_ORDER`
//!   section outside the groups linked to a COMDAT member: whether it takes
//!   the group is the keeper's (whole-link): `update-not-implemented
//!   (resolve)` by name, never a guessed anchor.
//! - the keys a front has that the old state may have no `SYMBOLS` row for
//!   (`_DYNAMIC`, `_end`, an undefined weak with no definition): printed and
//!   asserted in the extraction test (`key: None`, no value invented).
//!
//! On AArch64 and x86-64 (the targets completion is judged on).

mod support;

use std::ffi::OsString;
use std::path::PathBuf;

use mortar::front::{Env, Front, front};
use mortar::incremental::State5;
use mortar::timing::Timer;
use mortar::update::file_front::{FrontCache, Ids, front_of};
use mortar_core::id::{FileId, SectionId, SymbolId};
use mortar_core::identity::Anchor;
use mortar_core::incr::{IncrReason, Unimplemented};
use mortar_core::par::Pool;
use mortar_diag::{Collector, Policy};
use mortar_incr::capture::{FileRows, Keys, Link, file_rows};
use mortar_incr::format::Strings;
use mortar_incr::update::view::StateView;
use mortar_testutil::{Scratch, run, tool, tools_required};
use support::{Driver, build, build_by, with_output};

const M1: &str = "int wp255d_m1_data = 1;\nint wp255d_m1(int x) { return x + wp255d_m1_data; }\n";
const M2: &str = "static volatile int wp255d_m1_data = 3;\n\
                  int wp255d_m2(int x) { return x * wp255d_m1_data + 7; }\n";
/// `m1.o` edited (C++): `wp255d_m1` grown, a renamed function, an added
/// section, an added COMDAT group (`inline`, used).
const M1_EDITED: &str = "extern \"C\" { int wp255d_m1_data = 1; }\n\
    inline int wp255d_inl(int x) { return x * 3 + wp255d_m1_data; }\n\
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
    (1..)
        .map(FileId)
        .take_while(|&id| view.file(id).is_ok())
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

fn anchor_row(strings: &mut Strings, a: Anchor<'_>) -> (u8, mortar_incr::format::Str) {
    match a {
        Anchor::None => (0, mortar_incr::format::Str::default()),
        Anchor::Comdat(n) => (1, strings.add(n)),
        Anchor::Global(n) => (2, strings.add(n)),
        Anchor::Local(n) => (3, strings.add(n)),
    }
}

/// The whole front's rows of `file` (its id in `f`).
fn rows_of(f: &Front<'_>, strings: &mut Strings, file: FileId) -> FileRows {
    let link = Link {
        inputs: &f.inputs,
        resolution: &f.resolution,
        assignment: &f.assignment,
        split: &f.merge.split,
    };
    let keys = Keys::for_capture(&link, &f.pool).expect("keys");
    let mut v =
        file_rows(&link, &f.liveness, &keys, &f.layout, strings, &[file], false).expect("rows");
    v.pop().expect("the file's rows")
}

/// What one comparison saw.
#[derive(Debug, Default)]
struct Seen {
    sections: u32,
    comdat: u32,
    keys: u32,
    new_keys: u32,
    versioned: u32,
}

/// `front_of(view, file, Ids::Update)` against the whole front `f`'s rows
/// of the same file, by identity.
fn compare(
    line: &Line,
    f: &Front<'_>,
    view: &StateView<'_>,
    file: FileId,
    first: u32,
    seen: &mut Seen,
) {
    let mut strings = Strings::default();
    let new_file = same_file(f, view, file);
    let want = rows_of(f, &mut strings, new_file);
    let fresh = support::store();
    let mut cache = FrontCache::new(&fresh, pool(), &line.dir);
    let ids = Ids::Update { first_section: SectionId(first) };
    let what = |s: &str| format!("{}: file {}: {s}", line.label, file.0);
    let ff = front_of(&mut cache, view, file, ids).unwrap_or_else(|e| panic!("{}", what(&format!("{e:?}"))));
    assert_eq!(ff.sections.len(), want.sections.len(), "{}", what("section count"));
    assert!(!want.sections.is_empty(), "{}", what("an extracted file has sections"));
    for (k, (s, w)) in ff.sections.iter().zip(&want.sections).enumerate() {
        let at = |x: &str| what(&format!("section {k}: {x}"));
        assert_eq!(s.id, SectionId(first + k as u32), "{}", at("the update's id"));
        let got = (s.sh_type, s.flags, s.size, s.align, s.offset);
        assert_eq!(got, (w.sh_type, w.sh_flags, w.size, w.align, w.offset), "{}", at("header"));
        assert_eq!(strings.add(s.name), w.name, "{}", at("name"));
        assert_eq!(anchor_row(&mut strings, s.anchor), (w.anchor_kind, w.anchor), "{}", at("anchor"));
        assert_eq!(s.ordinal, w.ordinal, "{}", at("ordinal"));
        assert!(s.place.is_none() && s.rela.is_none(), "{}", at("no place in the update's ids"));
        seen.sections += 1;
        seen.comdat += u32::from(matches!(s.anchor, Anchor::Comdat(_)));
    }
    assert_eq!(ff.first_global, want.first_global, "{}", what("first_global"));
    assert_eq!(ff.versioned, want.versioned, "{}", what("versioned"));
    seen.versioned += ff.versioned;
    assert_eq!(ff.keys.len(), want.mentions.len(), "{}", what("mention count"));
    for (k, (key, w)) in ff.keys.iter().zip(&want.mentions).enumerate() {
        let at = |x: &str| what(&format!("entry {}: {x}", want.first_global + k as u32));
        let sym = &f.inputs.symbols[SymbolId(w.symbol)];
        assert_eq!((key.name, key.version), (sym.name, sym.version), "{}", at("key name"));
        assert_eq!((key.st_value, key.st_size), want.values[k], "{}", at("value"));
        // The state's key, when it has one, is the one of that name.
        match key.key {
            Some(row) => {
                let r = view.symbol(row).expect("SYMBOLS");
                assert_eq!(view.string(r.name).expect("name"), key.name, "{}", at("the row's name"));
                let v = view.string(r.version).expect("version");
                assert_eq!(&v[..], key.version.unwrap_or_default(), "{}", at("the row's version"));
            }
            None => seen.new_keys += 1,
        }
        seen.keys += 1;
    }
}

fn first_free(view: &StateView<'_>) -> u32 {
    u32::try_from(view.count(mortar_incr::format::kind::SECTIONS)).expect("SECTIONS")
}

#[test]
fn update_front_of_an_extracted_member() {
    for line in lines() {
        line.archive(&["m1.o", "m2.o", "m4.o"]);
        let old = state(&line, &line.args(&["wp255d_m2", "wp255d_m1"]), "old");
        let m4 = member(&old, "m4.o");
        let new_args = line.args(&["wp255d_m2", "wp255d_m1", "wp255d_m4"]);
        let store = support::store();
        let f = whole(&line, &new_args, &store);
        let mut seen = Seen::default();
        compare(&line, &f, &old, m4, first_free(&old), &mut seen);
        eprintln!("{}: extracted m4.o: {seen:?}", line.label);
        assert!(seen.versioned >= 2 && seen.keys > 0, "{}: {seen:?}", line.label);
        // The keys a front has that the old state has no row for: the old
        // state answers `None` (a new key: U-3 allocates it and resolves
        // it); the front gives no value of its own. The full link's
        // resolution of each, for the record.
        let fresh = support::store();
        let mut cache = FrontCache::new(&fresh, pool(), &line.dir);
        let ff = front_of(&mut cache, &old, m4, Ids::Update { first_section: SectionId(first_free(&old)) })
            .expect("m4");
        for name in [&b"_DYNAMIC"[..], b"_end", b"wp255d_nowhere"] {
            let key = ff.keys.iter().find(|k| k.name == name).expect("the key is mentioned");
            let full = (0..f.inputs.symbols.len())
                .map(SymbolId)
                .find(|&s| f.inputs.symbols[s].name == name && f.inputs.symbols[s].version.is_none())
                .map(|s| f.resolution.def[s]);
            eprintln!(
                "{}: {}: old state's key {:?}, the full link's def {full:?}",
                line.label,
                String::from_utf8_lossy(name),
                key.key
            );
            assert_eq!(key.st_value, 0, "an undefined mention");
            if let Some(row) = key.key {
                let r = old.symbol(row).expect("SYMBOLS");
                assert_eq!(old.string(r.name).expect("name"), name);
            }
            assert!(full.is_some(), "the full link has the key");
        }
    }
}

#[test]
fn update_front_of_edited_files() {
    for line in lines() {
        line.archive(&["m1e.o", "m2.o", "m4.o"]);
        let args = line.args(&["wp255d_m2", "wp255d_m1", "wp255d_m4"]);
        let view = state(&line, &args, "new");
        let store = support::store();
        let f = whole(&line, &args, &store);
        let mut seen = Seen::default();
        for name in ["m1e.o", "m2.o", "m4.o"] {
            compare(&line, &f, &view, member(&view, name), first_free(&view), &mut seen);
        }
        eprintln!("{}: edited: {seen:?}", line.label);
        assert!(seen.comdat > 0 && seen.versioned >= 2, "{}: {seen:?}", line.label);
    }
}

#[test]
fn update_link_order_outside_groups_falls_back() {
    for line in lines() {
        line.archive(&["m1.o", "m2.o", "m3.o"]);
        let view = state(&line, &line.args(&["wp255d_m2", "wp255d_m1", "wp255d_m3"]), "lo");
        let m3 = member(&view, "m3.o");
        let fresh = support::store();
        let mut cache = FrontCache::new(&fresh, pool(), &line.dir);
        assert!(front_of(&mut cache, &view, m3, Ids::Stored).is_ok(), "{}: stored", line.label);
        let fresh = support::store();
        let mut cache = FrontCache::new(&fresh, pool(), &line.dir);
        let ids = Ids::Update { first_section: SectionId(first_free(&view)) };
        let r = front_of(&mut cache, &view, m3, ids).map(|x| x.file);
        let want = IncrReason::UpdateNotImplemented(Some(Unimplemented::Resolve));
        assert_eq!(r, Err(want), "{}", line.label);
    }
}

//! WP-255d: the per-file front (`mortar::update::file_front::front_of`)
//! over a link's own format-5 state gives, for EVERY object file of the
//! link (archive members, extracted or not, included), the rows the whole
//! front gives for that file (`capture::file_rows` over the front, and the
//! front's relocation sections): `file_front_equals_the_whole_front`. And
//! IN-24 / WP-255 §7: bytes that are not the ones `FILES.hash` names are
//! `input-changed` before any is used (a rewritten file; a member moved
//! inside its archive).
//!
//! The link: the C++ program of `support` (exceptions, COMDAT, TLS, the
//! static libstdc++ and libc archives) with an archive of two members
//! appended, on AArch64 and x86-64 (the targets completion is judged on).

mod support;

use std::ffi::OsString;
use std::path::PathBuf;

use mortar::front::{Env, Front, front};
use mortar::incremental::State5;
use mortar::timing::Timer;
use mortar::update::file_front::{FrontCache, Ids, front_of};
use mortar_core::id::{FileId, SectionId};
use mortar_core::identity::Anchor;
use mortar_core::incr::IncrReason;
use mortar_core::inputs::Reader;
use mortar_core::par::Pool;
use mortar_diag::{Collector, Policy};
use mortar_incr::capture::{FileRows, Keys, Link, file_rows};
use mortar_incr::format::{NONE, Strings};
use mortar_incr::update::rows::FileTable;
use mortar_incr::update::view::StateView;
use mortar_testutil::{Scratch, run, tool, tools_required};
use support::{Driver, build, build_by, with_output};

/// A link line, the compiler that made its objects, and its archive of
/// two members (`lib2.a`: `m1.o`, `m2.o`, in that order).
struct Line {
    _scratch: Scratch,
    label: String,
    dir: PathBuf,
    args: Vec<String>,
    cc: String,
}

const M1: &str = "int wp255d_m1_data = 1;\nint wp255d_m1(int x) { return x + wp255d_m1_data; }\n";
const M2: &str = "static volatile int wp255d_m1_data = 3;\n\
                  int wp255d_m2(int x) { return x * wp255d_m1_data + 7; }\n\
                  int wp255d_m2b(int x) { return x - 1; }\n";

impl Line {
    fn args(&self, out: &str) -> Vec<OsString> {
        let mut a = with_output(&self.args, &self.dir.join(out));
        a.extend(["--incremental=full", "--incremental-state"].iter().map(OsString::from));
        a.push(format!("{out}.mortar-state").into());
        a
    }

    /// `lib2.a` made of `members` (object names in the scratch).
    fn archive(&self, members: &[&str]) {
        let ar = tool("ar").expect("ar");
        let lib = self.dir.join("lib2.a");
        let _ = std::fs::remove_file(&lib);
        let mut args: Vec<OsString> = vec!["rcs".into(), lib.into_os_string()];
        args.extend(members.iter().map(|m| self.dir.join(m).into_os_string()));
        let o = run(&ar, &args);
        assert!(o.success(), "ar: {}", o.stderr_text());
    }
}

fn lines() -> Vec<Line> {
    let mut v = Vec::new();
    for (label, cc, a64) in [
        ("aarch64 g++", Driver::Gxx.tool(), true),
        ("x86-64 g++", "x86_64-linux-gnu-g++".into(), false),
    ] {
        let s = Scratch::new(if a64 { "wp255d-a64" } else { "wp255d-x64" });
        let built = if a64 {
            build(&s, Driver::Gxx, &["-O1"], support::CXX_PROGRAM)
        } else {
            build_by(&s, &cc, &["-O1"], support::CXX_PROGRAM)
        };
        let Some(b) = built else {
            assert!(!tools_required(), "{cc} is required");
            continue;
        };
        let compiler = tool(&cc).expect("the compiler");
        for (name, text) in [("m1", M1), ("m2", M2)] {
            let src = s.write(&format!("{name}.c"), text);
            let obj = s.join(format!("{name}.o"));
            let args: Vec<OsString> = vec![
                "-x".into(),
                "c".into(),
                "-O1".into(),
                "-c".into(),
                src.into_os_string(),
                "-o".into(),
                obj.into_os_string(),
            ];
            let o = run(&compiler, &args);
            assert!(o.success(), "{cc}: {}", o.stderr_text());
        }
        let mut args = b.args;
        // The archive last, both members extracted (`-u`): `m2.o`'s local
        // `wp255d_m1_data` is a twin of `m1.o`'s global.
        args.push("-u".into());
        args.push("wp255d_m2".into());
        args.push("-u".into());
        args.push("wp255d_m1".into());
        args.push(b.dir.join("lib2.a").to_string_lossy().into_owned());
        let line = Line { _scratch: s, label: label.into(), dir: b.dir, args, cc };
        line.archive(&["m1.o", "m2.o"]);
        v.push(line);
    }
    v
}

/// The link of `line` with the format-5 switch: its state, as the update
/// reads it (`state5::read::open_bytes` over the written bytes).
fn state(line: &Line) -> StateView<'static> {
    let path = line.dir.join("out.state5");
    let inv = mortar::invoke_state5(
        "mortar",
        &line.args("out"),
        &line.dir,
        State5 { path: path.clone(), collide: false, rows: None },
    );
    let ok = matches!(inv.outcome, mortar::Outcome::Linked { .. });
    assert!(ok, "{}: the link failed: {:?}", line.label, inv.outcome);
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|_| panic!("{}: no state: {}", line.label, inv.incremental));
    mortar_incr::state5::read::open_bytes(bytes).expect("the state opens")
}

fn pool() -> Pool {
    Pool::try_new(Pool::available()).expect("a pool")
}

/// The whole front of `line`, as an incremental-mode link with the switch.
fn front5<'s>(line: &Line, store: &'s mortar_input::store::Store) -> Front<'s> {
    let parsed = mortar_cli::parse(&with_output(&line.args, &line.dir.join("front")));
    assert!(!parsed.failed(), "{:?}", parsed.diagnostics);
    let diag = Collector::new(Policy::default());
    let env = Env { cwd: &line.dir, pool: pool(), replay: None, slack: true, state5: true };
    let f = front(parsed.plan, env, store, &diag, &mut Timer::new(false));
    let report = diag.finish();
    assert!(!report.failed(), "{}: {report:?}", line.label);
    f.expect("the front")
}

fn anchor_row(strings: &mut Strings, a: Anchor<'_>) -> (u8, mortar_incr::format::Str) {
    match a {
        Anchor::None => (0, mortar_incr::format::Str::default()),
        Anchor::Comdat(n) => (1, strings.add(n)),
        Anchor::Global(n) => (2, strings.add(n)),
        Anchor::Local(n) => (3, strings.add(n)),
    }
}

/// What one line's comparison saw.
#[derive(Debug, Default)]
struct Seen {
    files: u32,
    members: u32,
    lazy: u32,
    sections: u32,
    comdat: u32,
    relocated: u32,
    keys: u32,
    local_twins: u32,
}

/// `front_of` of `file` against the whole front's rows `want` of it.
#[allow(clippy::too_many_lines, reason = "one field after the other")]
fn compare(
    f: &Front<'_>,
    m: &StateView<'_>,
    cache: &mut FrontCache<'_>,
    strings: &mut Strings,
    want: &FileRows,
    seen: &mut Seen,
) {
    let view = m;
    let file = want.file;
    let what = |s: &str| {
        format!("file {} ({}): {s}", file.0, String::from_utf8_lossy(f.inputs.files[file].path))
    };
    let ff = front_of(cache, view, file, Ids::Stored)
        .unwrap_or_else(|e| panic!("{}", what(&format!("{e:?}"))));
    seen.files += 1;
    let stored = view.sections_of(file).expect("SECTIONS");
    let range = view.file_index(file).expect("FILE_INDEX").ranges[FileTable::Sections as usize];
    assert_eq!(ff.sections.len(), want.sections.len(), "{}", what("section count"));
    let Reader::Object(obj) = f.inputs.files[file].reader else { panic!("{}", what("no object")) };
    for (k, (s, w)) in ff.sections.iter().zip(&want.sections).enumerate() {
        let at = |x: &str| what(&format!("section {k}: {x}"));
        // The ids: the stored range's rows.
        assert_eq!(s.id, SectionId(range.first + k as u32), "{}", at("id"));
        assert_eq!(s.index.0 as usize, k, "{}", at("index"));
        // The capture's fields, the whole front's and the state's.
        let got = (s.sh_type, s.flags, s.size, s.align, s.offset);
        assert_eq!(got, (w.sh_type, w.sh_flags, w.size, w.align, w.offset), "{}", at("header"));
        let st = &stored[k];
        assert_eq!(
            got,
            (st.sh_type, st.sh_flags, st.size, st.align, st.offset),
            "{}",
            at("stored header")
        );
        assert_eq!(strings.add(s.name), w.name, "{}", at("name"));
        assert_eq!(anchor_row(strings, s.anchor), (w.anchor_kind, w.anchor), "{}", at("anchor"));
        assert_eq!(s.ordinal, w.ordinal, "{}", at("ordinal"));
        let place = (w.place_chunk != NONE).then_some((w.place_chunk, w.place_offset));
        assert_eq!(s.place.map(|(c, o)| (c.0, o)), place, "{}", at("place"));
        // The relocation table of a placed section: the whole front's.
        let id = SectionId(want.first_section + k as u32);
        let reloc = f.resolution.sections.get(id).map_or(0, |r| r.reloc.0);
        if s.place.is_some() {
            let t = s.rela.map(|t| (t.offset, t.count));
            let expect = (reloc != 0).then(|| {
                let r = obj.section(mortar_elf::SectionIndex(reloc)).expect("reloc section");
                let range = r.file_range().expect("range").unwrap_or(0..0);
                let entsize = if obj.class() == mortar_elf::Class::Elf64 { 24 } else { 12 };
                (range.start as u64, ((range.end - range.start) as u64 / entsize) as u32)
            });
            assert_eq!(t, expect, "{}", at("relocations"));
            seen.relocated += u32::from(t.is_some());
            let sf = ff.section_front(k).expect("a section front");
            assert_eq!((sf.size, sf.flags, sf.src), (s.size, s.flags, s.offset), "{}", at("front"));
        } else {
            assert!(s.rela.is_none(), "{}", at("relocations of an unplaced section"));
        }
        seen.sections += 1;
        seen.comdat += u32::from(matches!(s.anchor, Anchor::Comdat(_)));
    }
    assert_eq!(ff.first_global, want.first_global, "{}", what("first_global"));
    assert_eq!(ff.versioned, want.versioned, "{}", what("versioned"));
    // The keys: the whole front's `SymbolId`s by name, and the state's.
    let mentions = view.mentions_of(file).expect("MENTIONS");
    if want.mentions.is_empty() {
        seen.lazy += 1;
    } else {
        assert_eq!(want.mentions.len(), ff.keys.len(), "{}", what("mention count"));
        assert_eq!(mentions.len(), ff.keys.len(), "{}", what("stored mention count"));
        for (k, ((key, w), st)) in ff.keys.iter().zip(&want.mentions).zip(&mentions).enumerate() {
            let at = |x: &str| what(&format!("entry {}: {x}", want.first_global + k as u32));
            let sym = &f.inputs.symbols[mortar_core::id::SymbolId(w.symbol)];
            assert_eq!((key.name, key.version), (sym.name, sym.version), "{}", at("key name"));
            assert_eq!(key.key.map(|s| s.0), Some(st.symbol), "{}", at("stored key"));
            assert_eq!((key.st_value, key.st_size), want.values[k], "{}", at("value"));
            seen.keys += 1;
        }
    }
    // A local of a key's name (another file's global) is not mapped: the
    // mapped entries are the non-local ones alone (checked above), and
    // such a local is counted where one occurs.
    if let Ok(Some(t)) = ff.object.symbols() {
        let first = t.first_global().0;
        for (i, sym) in t.iter() {
            let Ok(n) = t.name(&sym) else { continue };
            let bind_local = sym.st_info >> 4 == 0;
            if i.0 > 0 && i.0 < first && bind_local && !n.is_empty() && !n.contains(&b'.') {
                seen.local_twins += u32::from(view.find_key(n, b"").expect("NAMES").is_some());
            }
        }
    }
    seen.members += u32::from(ff.row.member != 0);
}

#[test]
fn file_front_equals_the_whole_front() {
    for line in lines() {
        let m = state(&line);
        let store = support::store();
        let f = front5(&line, &store);
        let link = Link {
            inputs: &f.inputs,
            resolution: &f.resolution,
            assignment: &f.assignment,
            split: &f.merge.split,
        };
        let keys = Keys::for_capture(&link, &f.pool).expect("keys");
        let mut strings = Strings::default();
        let files: Vec<FileId> = (1..f.inputs.files.len())
            .map(FileId)
            .filter(|&id| matches!(f.inputs.files[id].reader, Reader::Object(_)))
            .collect();
        let want = file_rows(&link, &f.liveness, &keys, &f.layout, &mut strings, &files, false)
            .expect("file_rows");
        // A store of the update's own: nothing read by the full front.
        let fresh = support::store();
        let mut cache = FrontCache::new(&fresh, pool(), &line.dir);
        let view = &m;
        let mut seen = Seen::default();
        for w in &want {
            let row = view.file(w.file).expect("FILES");
            assert_eq!(
                view.string(row.path).expect("path"),
                f.inputs.files[w.file].path,
                "{}: FILES row {} is the front's file",
                line.label,
                w.file.0
            );
            compare(&f, &m, &mut cache, &mut strings, w, &mut seen);
        }
        // Each file once; the cache lists them in order.
        assert_eq!(cache.opened(), &files[..], "{}", line.label);
        let again = front_of(&mut cache, view, files[0], Ids::Stored).expect("cached").file;
        assert_eq!(again, files[0]);
        assert_eq!(cache.opened().len(), files.len(), "{}: opened twice", line.label);
        eprintln!("{}: {seen:?}", line.label);
        assert!(seen.members > 2 && seen.lazy > 0 && seen.comdat > 0, "{}: {seen:?}", line.label);
        assert!(
            seen.relocated > 0 && seen.keys > 0 && seen.local_twins > 0,
            "{}: {seen:?}",
            line.label
        );
        let _ = line.cc.len();
    }
}

/// The member `name` of `lib2.a` in the state.
fn member(m: &StateView<'_>, f: &Front<'_>, name: &str) -> FileId {
    let view = m;
    (1..f.inputs.files.len())
        .map(FileId)
        .find(|&id| {
            let r = view.file(id).expect("FILES");
            r.member != 0
                && view.string(r.path).expect("path").ends_with(b"lib2.a")
                && view.string(r.member_name).expect("name").ends_with(name.as_bytes())
        })
        .unwrap_or_else(|| panic!("no member {name}"))
}

/// The first object named `name` in the state.
fn object(m: &StateView<'_>, f: &Front<'_>, name: &str) -> FileId {
    let view = m;
    (1..f.inputs.files.len())
        .map(FileId)
        .find(|&id| {
            let r = view.file(id).expect("FILES");
            r.member == 0 && view.string(r.path).expect("path").ends_with(name.as_bytes())
        })
        .unwrap_or_else(|| panic!("no object {name}"))
}

fn is_input_changed(r: &Result<FileId, IncrReason>) -> bool {
    matches!(r, Err(IncrReason::InputChanged(_)))
}

#[test]
fn file_front_rewritten_file_is_input_changed() {
    for line in lines() {
        let m = state(&line);
        let store = support::store();
        let f = front5(&line, &store);
        let file = object(&m, &f, "x1.cpp.o");
        let view = &m;
        // Before: it opens.
        let fresh = support::store();
        let mut cache = FrontCache::new(&fresh, pool(), &line.dir);
        assert!(front_of(&mut cache, view, file, Ids::Stored).is_ok(), "{}", line.label);
        // Rewritten, same length, one byte flipped near its end.
        let path = line.dir.join(
            String::from_utf8_lossy(&view.string(view.file(file).unwrap().path).unwrap()).as_ref(),
        );
        let mut bytes = std::fs::read(&path).expect("the object");
        let at = bytes.len() - 1;
        bytes[at] ^= 0x5a;
        std::fs::write(&path, &bytes).expect("rewrite");
        let fresh = support::store();
        let mut cache = FrontCache::new(&fresh, pool(), &line.dir);
        let r = front_of(&mut cache, view, file, Ids::Stored).map(|x| x.file);
        assert!(is_input_changed(&r), "{}: {r:?}", line.label);
        assert!(cache.opened().is_empty(), "{}: a refused file is not opened", line.label);
    }
}

#[test]
fn file_front_member_moved_in_its_archive_is_input_changed() {
    for line in lines() {
        let m = state(&line);
        let store = support::store();
        let f = front5(&line, &store);
        let (m1, m2) = (member(&m, &f, "m1.o"), member(&m, &f, "m2.o"));
        let view = &m;
        let fresh = support::store();
        let mut cache = FrontCache::new(&fresh, pool(), &line.dir);
        if let Err(e) = front_of(&mut cache, view, m1, Ids::Stored) {
            panic!("{}: m1: {e:?}", line.label)
        }
        if let Err(e) = front_of(&mut cache, view, m2, Ids::Stored) {
            panic!("{}: m2: {e:?}", line.label)
        }
        // The same members, the other order: every `base` moves (IN-24).
        line.archive(&["m2.o", "m1.o"]);
        let fresh = support::store();
        let mut cache = FrontCache::new(&fresh, pool(), &line.dir);
        for id in [m1, m2] {
            let r = front_of(&mut cache, view, id, Ids::Stored).map(|x| x.file);
            assert!(is_input_changed(&r), "{}: member {}: {r:?}", line.label, id.0);
        }
    }
}

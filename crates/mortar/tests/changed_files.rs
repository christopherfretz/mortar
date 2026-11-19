//! WP-225 M2b-1: the U4 adapter (`mortar::update::changed::changed_files`)
//! judged by the READER: a changed file's `ChangedFile` (in the update's id
//! space) equals, row for row after mapping by identity (sections by their
//! identity, names by bytes, mentions by key name), what the stored
//! state's reader gives for that file: the unchanged file against the
//! state of the same inputs, an edited file (renamed, grown, an added
//! symbol) against the state the FULL link of the new inputs writes. A
//! wrong `Str` offset into `ChangedFiles::strings` is caught.

mod support;

use std::ffi::OsString;
use std::path::PathBuf;

use mortar::front::{Env, Front, front};
use mortar::timing::Timer;
use mortar::update::changed::{SectionKey, changed_files};
use mortar_core::{FileId, SectionId, SymbolId};
use mortar_diag::{Collector, Policy};
use mortar_incr::format::{NONE, index_kind};
use mortar_incr::update::rows::edge_kind;
use mortar_incr::update::testing::MemoryState;
use mortar_incr::update::view::{Change, ChangedFiles, StateView};
use mortar_testutil::{Scratch, tools_required};
use support::{Source, build_by, store, with_output};

/// The two 64-bit targets' C drivers.
const TARGETS: [(&str, &str); 2] = [("aarch64", "gcc"), ("x86-64", "x86_64-linux-gnu-gcc")];

const MAIN: &str = "int f(void);\nint counter;\nint main(void){ return f() + counter; }\n";
const A0: &str = "extern int counter;\nint g(void){ return 2; }\nint h(void){ return 3; }\n\
int k(void){ return g() + counter; }\nint f(void){ return k(); }\n";
/// The edits of `a.c`: `k` renamed, `g` grown, a symbol added.
const A_RENAMED: &str = "extern int counter;\nint g(void){ return 2; }\nint h(void){ return 3; }\n\
int k2(void){ return g() + counter; }\nint f(void){ return k2(); }\n";
const A_GROWN: &str = "extern int counter;\nint g(void){ int s = 0; for (int i = 0; i < counter; i++) s += i * i; return 2 + s; }\n\
int h(void){ return 3; }\nint k(void){ return g() + counter; }\nint f(void){ return k(); }\n";
const A_ADDED: &str = "extern int counter;\nint table[4] = {1, 2, 3, 4};\nint g(void){ return 2; }\n\
int h(void){ return 3; }\nint added(int x){ return table[x & 3]; }\n\
int k(void){ return g() + counter + added(1); }\nint f(void){ return k(); }\n";

const FLAGS: [&str; 3] = ["-O1", "-ffunction-sections", "-fdata-sections"];

/// One link line (`main.c`, `a.c` with `a`'s text) in its scratch.
struct Line {
    scratch: Scratch,
    dir: PathBuf,
    args: Vec<String>,
}

impl Line {
    fn build(label: &str, cc: &str, a: &str, extra: &[&str]) -> Option<Self> {
        let s = Scratch::new(&format!("wp225-m2b1-{}", label.replace([' ', '/'], "-")));
        let sources =
            [Source { name: "main.c", text: MAIN }, Source { name: "a.c", text: leak(a) }];
        let Some(b) = build_by(&s, cc, &FLAGS, &sources) else {
            assert!(!tools_required(), "{cc} is required");
            return None;
        };
        let mut args = b.args;
        args.extend(extra.iter().map(|e| (*e).to_owned()));
        Some(Self { scratch: s, dir: b.dir, args })
    }
    /// `a.c` rebuilt with `text` (the same object path).
    fn edit(&self, cc: &str, text: &str) {
        let sources =
            [Source { name: "main.c", text: MAIN }, Source { name: "a.c", text: leak(text) }];
        build_by(&self.scratch, cc, &FLAGS, &sources).expect("the edit builds");
    }
    fn args(&self, out: &str) -> Vec<OsString> {
        let mut a = with_output(&self.args, &self.dir.join(out));
        a.extend(["--incremental=full", "--incremental-state"].iter().map(OsString::from));
        a.push(format!("{out}.mortar-state").into());
        a
    }
    /// The full link's state (decoded): the stored state's reader.
    fn link(&self, out: &str) -> MemoryState {
        let inv = mortar::invoke_in("mortar", &self.args(out), &self.dir);
        assert!(matches!(inv.outcome, mortar::Outcome::Linked { .. }), "{:?}", inv.outcome);
        let bytes = std::fs::read(self.dir.join(format!("{out}.mortar-state"))).expect("state");
        mortar_incr::state5::read::decode(&bytes).expect("the state decodes")
    }
    fn front<'s>(&self, store: &'s mortar_input::store::Store) -> Front<'s> {
        let parsed = mortar_cli::parse(&self.args("front"));
        assert!(!parsed.failed(), "{:?}", parsed.diagnostics);
        let diag = Collector::new(Policy::default());
        let env = Env {
            cwd: &self.dir,
            pool: mortar_core::par::Pool::new(2),
            replay: None,
            slack: true,
            state5: true,
        };
        let f = front(parsed.plan, env, store, &diag, &mut Timer::new(false));
        assert!(!diag.finish().failed());
        f.expect("the front")
    }
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_owned().into_boxed_str())
}

/// The file whose path ends with `name`.
fn file_named(f: &Front<'_>, name: &str) -> FileId {
    let (id, _) = f
        .inputs
        .files
        .iter()
        .find(|(_, x)| x.path.ends_with(name.as_bytes()))
        .unwrap_or_else(|| panic!("no {name}"));
    id
}

/// A key target of the oracle: a key by name, or a section by identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Target {
    Key(Vec<u8>, Vec<u8>),
    Section(SectionKey),
    Value(u32),
}

/// THE ORACLE: `cf`'s file `file` (its `Str`s read over `base`, the view
/// it was made against) against what `reader` (a stored state's reader)
/// gives for that file: sections in order (identity and header, the
/// placement aside: U-4 places), `first_global`, mentions by key name
/// (the stored ones are one per key), `INDEX` and `EDGES` rows with
/// their targets by identity, the COMDAT copies by key name.
fn judge(
    cf: &ChangedFiles,
    base: &StateView<'_>,
    reader: &StateView<'_>,
    file: FileId,
) -> Result<(), String> {
    let e = |x: mortar_core::incr::IncrReason| x.report();
    let c = cf.files.iter().find(|c| c.file == file).ok_or("no ChangedFile")?;
    let s_cf = |s| cf.string(base, s).map_err(e);
    let s_rd = |s| reader.string(s).map_err(e);
    // Sections.
    let mine_key = |k: u32| -> Result<SectionKey, String> {
        let r = c
            .sections
            .get(k.checked_sub(c.first_section.0).ok_or("before")? as usize)
            .ok_or("past")?;
        Ok((r.file, r.anchor_kind, s_cf(r.anchor)?, s_cf(r.name)?, r.ordinal))
    };
    let theirs_key = |k: u32| -> Result<SectionKey, String> {
        let r = reader.section(SectionId(k)).map_err(e)?;
        Ok((r.file, r.anchor_kind, s_rd(r.anchor)?, s_rd(r.name)?, r.ordinal))
    };
    let stored = reader.sections_of(file).map_err(e)?;
    let head = |r: &mortar_incr::format::SectionRow| {
        (r.sh_type, r.sh_flags, r.size, r.align, r.offset, r.kind, r.bits, r.out)
    };
    let mut a = Vec::new();
    for r in &c.sections {
        a.push(((r.file, r.anchor_kind, s_cf(r.anchor)?, s_cf(r.name)?, r.ordinal), head(r)));
        if r.place_chunk != NONE {
            return Err("a placed fresh section".into());
        }
    }
    let mut b = Vec::new();
    for r in &stored {
        b.push(((r.file, r.anchor_kind, s_rd(r.anchor)?, s_rd(r.name)?, r.ordinal), head(r)));
    }
    if a != b {
        return Err(format!("SECTIONS differ:\n mine   {a:?}\n stored {b:?}"));
    }
    if c.first_global != reader.file_index(file).map_err(e)?.first_global {
        return Err("first_global differs".into());
    }
    // Mentions, one per key (the stored rows are), by name.
    let key_cf = |k: u32| -> Result<Target, String> {
        let n = c.keys.get(k as usize).ok_or("a key past keys")?;
        Ok(Target::Key(n.name.clone(), n.version.clone()))
    };
    let key_rd = |k: u32| -> Result<Target, String> {
        let s = reader.symbol(SymbolId(k)).map_err(e)?;
        Ok(Target::Key(s_rd(s.name)?, s_rd(s.version)?))
    };
    let sec = |k: u32,
               f: &dyn Fn(u32) -> Result<SectionKey, String>|
     -> Result<Option<SectionKey>, String> {
        if k == NONE { Ok(None) } else { f(k).map(Some) }
    };
    if c.values.len() != c.mentions.len() {
        return Err("values and mentions differ in length".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut a = Vec::new();
    for m in &c.mentions {
        if seen.insert(m.symbol) {
            a.push((key_cf(m.symbol)?, m.kind, m.bind, m.typ, m.other, sec(m.section, &mine_key)?));
        }
    }
    let mut b = Vec::new();
    for m in reader.mentions_of(file).map_err(e)? {
        b.push((key_rd(m.symbol)?, m.kind, m.bind, m.typ, m.other, sec(m.section, &theirs_key)?));
    }
    a.sort();
    b.sort();
    if a != b {
        return Err(format!("MENTIONS differ:\n mine   {a:?}\n stored {b:?}"));
    }
    // INDEX.
    let index_t = |kind: u8,
                   t: u32,
                   key: &dyn Fn(u32) -> Result<Target, String>,
                   s: &dyn Fn(u32) -> Result<SectionKey, String>| {
        match kind {
            index_kind::GLOBAL | index_kind::SLOT => key(t),
            _ => s(t).map(Target::Section),
        }
    };
    let mut a = Vec::new();
    for r in &c.index {
        a.push((mine_key(r.unit)?, r.kind, index_t(r.kind, r.target, &key_cf, &mine_key)?));
    }
    let mut b = Vec::new();
    for r in reader.index_of(file).map_err(e)? {
        b.push((theirs_key(r.unit)?, r.kind, index_t(r.kind, r.target, &key_rd, &theirs_key)?));
    }
    a.sort();
    a.dedup();
    b.sort();
    b.dedup();
    if a != b {
        return Err(format!("INDEX differs:\n mine   {a:?}\n stored {b:?}"));
    }
    // EDGES.
    let edge_t = |kind: u8,
                  t: u32,
                  key: &dyn Fn(u32) -> Result<Target, String>,
                  s: &dyn Fn(u32) -> Result<SectionKey, String>| {
        match edge_kind::target(kind) {
            Some(edge_kind::Target::Key) => key(t),
            Some(edge_kind::Target::Section) => s(t).map(Target::Section),
            _ => Ok(Target::Value(t)),
        }
    };
    let mut a = Vec::new();
    for r in &c.edges {
        a.push((
            mine_key(r.source)?,
            r.kind,
            edge_t(r.kind, r.target, &key_cf, &mine_key)?,
            r.offset,
        ));
    }
    let mut b = Vec::new();
    for r in reader.edges_of(file).map_err(e)? {
        b.push((
            theirs_key(r.source)?,
            r.kind,
            edge_t(r.kind, r.target, &key_rd, &theirs_key)?,
            r.offset,
        ));
    }
    a.sort();
    b.sort();
    if a != b {
        return Err(format!("EDGES differ:\n mine   {a:?}\n stored {b:?}"));
    }
    // COPIES, by key name.
    for x in &c.odr.copies {
        let n = c.keys.get(x.key as usize).ok_or("a copy's key past keys")?;
        let k =
            reader.find_key(&n.name, &n.version).map_err(e)?.ok_or("a copy's key not stored")?;
        let rows = reader.copies(k).map_err(e)?;
        if !rows
            .iter()
            .any(|r| (r.file, r.index, r.size, r.typ) == (file.0, x.index, x.size, x.typ))
        {
            return Err(format!("COPIES: {x:?} not stored"));
        }
    }
    Ok(())
}

/// The scenario's `ChangedFiles` for `a.c.o` over `view` from `f`.
fn adapt(f: &Front<'_>, view: &StateView<'_>, capture: &MemoryState) -> (ChangedFiles, FileId) {
    let a = file_named(f, "a.c.o");
    let cf = changed_files(f, view, &[(a, Change::Changed)], &[], capture)
        .unwrap_or_else(|e| panic!("changed_files: {}", e.report()));
    (cf, a)
}

// spec: WP-225 M2b-1 Task 1 (incr.md IN-70 U4; the record's "M2b-1"): the
// unchanged file's ChangedFile equals the stored reader's rows for it, by
// identity, with and without --gc-sections (EDGES); the fresh sections
// are past the state's SECTIONS, the strings past STRINGS.
#[test]
fn changed_file_of_the_unchanged_file_is_the_stored_rows() {
    let mut ran = 0;
    for (arch, cc) in TARGETS {
        for extra in [&[][..], &["--gc-sections"][..]] {
            let label = format!("same {arch} {extra:?}");
            let Some(l) = Line::build(&label, cc, A0, extra) else { continue };
            ran += 1;
            let m = l.link("out");
            let view = m.view();
            let store = store();
            let f = l.front(&store);
            let (cf, a) = adapt(&f, &view, &m);
            let c = &cf.files[0];
            assert_eq!(
                u64::from(c.first_section.0),
                view.count(mortar_incr::format::kind::SECTIONS)
            );
            assert!(!c.sections.is_empty() && !c.keys.is_empty() && !c.index.is_empty(), "{label}");
            assert_eq!(c.edges.is_empty(), extra.is_empty(), "{label}: EDGES only under gc");
            assert!(c.sections.iter().all(|s| s.name.len == 0
                || u64::from(s.name.off) >= view.count(mortar_incr::format::kind::STRINGS)));
            judge(&cf, &view, &view, a).unwrap_or_else(|e| panic!("{label}: {e}"));
            // The mutation: one name's offset moved by a byte is caught.
            let mut bad = cf.clone();
            let s = bad.files[0].sections.iter_mut().find(|s| s.name.len > 1).expect("a name");
            s.name.off += 1;
            assert!(judge(&bad, &view, &view, a).is_err(), "{label}: a wrong offset passed");
        }
    }
    assert!(ran == 4 || !tools_required(), "{ran} of 4 lines built");
}

// spec: WP-225 M2b-1 Task 1: an edited file's ChangedFile (made over the
// OLD state) gives the rows the FULL link of the new inputs stores for
// it: `k` renamed, `g` grown, a symbol (and a data object) added.
#[test]
fn changed_file_of_an_edit_is_the_full_links_rows() {
    let mut ran = 0;
    for (arch, cc) in TARGETS {
        for (what, text) in [("renamed", A_RENAMED), ("grown", A_GROWN), ("added", A_ADDED)] {
            let label = format!("{what} {arch}");
            let Some(l) = Line::build(&label, cc, A0, &[]) else { continue };
            ran += 1;
            let m0 = l.link("out");
            l.edit(cc, text);
            let m1 = l.link("out1");
            let store = store();
            let f = l.front(&store);
            let view0 = m0.view();
            let (cf, a) = adapt(&f, &view0, &m1);
            judge(&cf, &view0, &m1.view(), a).unwrap_or_else(|e| panic!("{label}: {e}"));
            // and it is not the OLD file's rows (the edit is seen).
            assert!(judge(&cf, &view0, &view0, a).is_err(), "{label}: the edit is invisible");
        }
    }
    assert!(ran == 6 || !tools_required(), "{ran} of 6 lines built");
}

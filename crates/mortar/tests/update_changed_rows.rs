//! WP-267 P-1: U4's rows from the changed file alone
//! (`mortar::update::changed_rows`).
//!
//! - `changed_rows_are_the_full_links_rows`: an edited object (grown,
//!   renamed, an added section, an added COMDAT group) given to
//!   `changed_files_of` over the state of the OLD inputs, then
//!   `facts_after_resolve` with the update's own election: every field it
//!   makes equals, field by field (names by bytes, keys by name, sections
//!   by index in the file, output sections by name), the row the FULL link
//!   of the NEW inputs stores for that file (an oracle that is not front
//!   1's rows). On the `state_producers` fixtures (`support/update.rs`).
//! - `changed_rows_refuse_by_name`: a changed archive member (M-1) is
//!   `update-not-implemented (resolve)`, `--wrap` is `(scan)`, an
//!   unchanged file (its bytes hash to the stored hash) has no row.
//! - `changed_rows_equal_the_fronts`: the real path under
//!   `MORTAR_UPDATE_FINDINGS`: each update's shadow line says `P-1: equal`
//!   (front 1's rows, field by field), or `refused by name` for `--wrap`.
//!
//! On AArch64 and x86-64.
#![allow(
    clippy::too_many_lines,
    clippy::many_single_char_names,
    reason = "one field after the other, each pair `mine`/`full`"
)]

mod support;
#[path = "support/update.rs"]
#[allow(dead_code, reason = "a shared helper file; this test uses part of it")]
mod update;

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use mortar::update::changed_rows::{changed_files_of, facts_after_resolve};
use mortar::update::file_front::FrontCache;
use mortar_core::FileId;
use mortar_core::incr::{IncrReason, Unimplemented};
use mortar_incr::format::section_bits::KEPT;
use mortar_incr::format::{Str, kind};
use mortar_incr::update::view::{ChangedFiles, StateView};
use mortar_testutil::run_with;
use support::with_output;
use update::{Line, lines, pool, state, whole};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

/// The object before the edit (C++: a COMDAT group, `p1_inl`).
const P1_V0: &str = "extern \"C\" { int p1_data = 4; }\n\
    inline __attribute__((noinline)) int p1_inl(int x) { return x * 5 + p1_data; }\n\
    extern \"C\" int p1_f(int x) { return p1_inl(x) + 1; }\n";
/// The edit: `p1_f` grown, a renamed function, an added section, an added
/// COMDAT group (`p1_inl2`), a new global, a `.note.gnu.property` (the
/// target's `FEATURE_1_AND`: `FILES.properties`).
const P1_V1: &str = "#ifdef __x86_64__\n#define P1_TYPE \"0xc0000002\"\n\
    #else\n#define P1_TYPE \"0xc0000000\"\n#endif\n\
    __asm__(\".section .note.gnu.property,\\\"a\\\",%note\\n\\t.p2align 3\\n\\t.long 4, 16, 5\\n\"\n\
    \"\\t.asciz \\\"GNU\\\"\\n\\t.long \" P1_TYPE \", 4, 1, 0\\n\\t.text\\n\");\n\
    extern \"C\" { int p1_data = 4; int p1_more = 2; }\n\
    inline __attribute__((noinline)) int p1_inl(int x) { return x * 5 + p1_data; }\n\
    inline __attribute__((noinline)) int p1_inl2(int x) { return x * 3 - p1_more; }\n\
    extern \"C\" int p1_renamed(int x) { return x ^ 7; }\n\
    extern \"C\" __attribute__((section(\".text.p1_added\"))) int p1_added(int x) \
    { return x + 9; }\n\
    extern \"C\" int p1_f(int x) { int s = 0; for (int i = 0; i < x; i++) s += i * x; \
    return s + p1_inl(x) + p1_inl2(x) + p1_renamed(x) + p1_added(x); }\n";

#[allow(clippy::needless_pass_by_value, reason = "a `map_err` argument")]
fn report(e: IncrReason) -> String {
    e.report()
}

/// The file of `view` whose path ends in `name` (not a member).
fn file_named(view: &StateView<'_>, name: &str) -> FileId {
    let files = u32::try_from(view.count(kind::FILES)).expect("FILES");
    (0..files)
        .map(FileId)
        .find(|&id| {
            let r = view.file(id).expect("FILES");
            r.member == 0 && view.string(r.path).expect("path").ends_with(name.as_bytes())
        })
        .unwrap_or_else(|| panic!("no file {name}"))
}

/// The plan of `args` (the update's line).
fn plan_of(line: &Line, args: &[String]) -> mortar_core::plan::Plan {
    let parsed = mortar_cli::parse(&with_output(args, &line.dir.join("plan")));
    assert!(!parsed.failed(), "{:?}", parsed.diagnostics);
    parsed.plan
}

/// The field-by-field check: each field named, every failure kept.
#[derive(Default)]
struct Fields {
    n: usize,
    bad: Vec<String>,
}

impl Fields {
    fn eq<T: PartialEq + std::fmt::Debug>(&mut self, what: &str, mine: &T, full: &T) {
        self.n += 1;
        if mine != full {
            self.bad.push(format!("{what}: mine {mine:?}, the full link's {full:?}"));
        }
    }
}

/// `cf`'s one file against what `s1` (the full link of the new inputs)
/// stores for `id`.
fn against_full_link(
    label: &str,
    s0: &StateView<'_>,
    s1: &StateView<'_>,
    cf: &ChangedFiles,
    id: FileId,
) -> Fields {
    let e = report;
    let mut x = Fields::default();
    assert_eq!(cf.files.len(), 1, "{label}: one changed file");
    let c = &cf.files[0];
    let mine_s = |s: Str| cf.string(s0, s).map_err(|e| e.report()).expect("a string of mine");
    let full_s = |s: Str| s1.string(s).map_err(|e| e.report()).expect("a string of the state");
    // FILES.
    let full = s1.file(id).map_err(e).expect("FILES");
    let r = &c.row;
    x.eq("file", &c.file, &id);
    x.eq("row.kind", &r.kind, &full.kind);
    x.eq("row.member", &r.member, &full.member);
    x.eq("row.flags", &r.flags, &full.flags);
    x.eq("row.path", &mine_s(r.path), &full_s(full.path));
    x.eq("row.mention", &r.mention, &full.mention);
    x.eq("row.member_name", &mine_s(r.member_name), &full_s(full.member_name));
    x.eq("row.member_ordinal", &r.member_ordinal, &full.member_ordinal);
    x.eq("row.base", &r.base, &full.base);
    x.eq("row.len", &r.len, &full.len);
    x.eq("row.hash", &r.hash, &full.hash);
    x.eq("row.rank", &r.rank, &full.rank);
    x.eq("row.round", &r.round, &full.round);
    x.eq("row.stack", &r.stack, &full.stack);
    x.eq("row.properties", &mine_s(r.properties), &full_s(full.properties));
    // The fresh ids: past the OLD state's SECTIONS.
    let first = u32::try_from(s0.count(kind::SECTIONS)).expect("SECTIONS");
    x.eq("first_section", &c.first_section.0, &first);
    let fi = s1.file_index(id).map_err(e).expect("FILE_INDEX");
    x.eq("first_global", &c.first_global, &fi.first_global);
    // SECTIONS, by index in the file; output sections by name.
    let rows = s1.sections_of(id).map_err(e).expect("SECTIONS");
    x.eq("sections.len", &c.sections.len(), &rows.len());
    let outs0 = s0.outsecs().map_err(e).expect("OUTSECS");
    let outs1 = s1.outsecs().map_err(e).expect("OUTSECS");
    let out_name = |outs: &[mortar_incr::format::OutSecRow], view: &StateView<'_>, o: u16| {
        (o != 0).then(|| {
            let r = &outs[usize::from(o) - 1];
            (view.string(r.name).expect("an OUTSECS name"), r.ordinal)
        })
    };
    for (k, (m, f)) in c.sections.iter().zip(&rows).enumerate() {
        x.eq(&format!("section {k} file"), &m.file, &f.file);
        x.eq(&format!("section {k} anchor_kind"), &m.anchor_kind, &f.anchor_kind);
        x.eq(&format!("section {k} anchor"), &mine_s(m.anchor), &full_s(f.anchor));
        x.eq(&format!("section {k} name"), &mine_s(m.name), &full_s(f.name));
        x.eq(&format!("section {k} ordinal"), &m.ordinal, &f.ordinal);
        x.eq(&format!("section {k} sh_type"), &m.sh_type, &f.sh_type);
        x.eq(&format!("section {k} sh_flags"), &m.sh_flags, &f.sh_flags);
        x.eq(&format!("section {k} size"), &m.size, &f.size);
        x.eq(&format!("section {k} align"), &m.align, &f.align);
        x.eq(&format!("section {k} offset"), &m.offset, &f.offset);
        x.eq(&format!("section {k} kind"), &m.kind, &f.kind);
        x.eq(&format!("section {k} KEPT"), &(m.bits & KEPT), &(f.bits & KEPT));
        x.eq(
            &format!("section {k} out"),
            &out_name(&outs0, s0, m.out),
            &out_name(&outs1, s1, f.out),
        );
    }
    // MENTIONS by position, keys by name, sections by index in the file.
    let ms = s1.mentions_of(id).map_err(e).expect("MENTIONS");
    x.eq("mentions.len", &c.mentions.len(), &ms.len());
    let first1 = fi.ranges[mortar_incr::update::rows::FileTable::Sections as usize].first;
    for (k, (m, f)) in c.mentions.iter().zip(&ms).enumerate() {
        let key = &c.keys[m.symbol as usize];
        let sym = s1.symbol(mortar_core::SymbolId(f.symbol)).map_err(e).expect("SYMBOLS");
        x.eq(
            &format!("mention {k} key"),
            &(key.name.clone(), key.version.clone()),
            &(full_s(sym.name), full_s(sym.version)),
        );
        x.eq(
            &format!("mention {k} (kind, bind, typ, other)"),
            &(m.kind, m.bind, m.typ, m.other),
            &(f.kind, f.bind, f.typ, f.other),
        );
        let at = |s: u32, base: u32| (s != mortar_incr::format::NONE).then(|| s - base);
        x.eq(
            &format!("mention {k} section"),
            &at(m.section, c.first_section.0),
            &at(f.section, first1),
        );
    }
    // COPIES (every `--odr` mode but off): each of mine is the full link's
    // row of the key by name, and the file has no other.
    let mut theirs = 0;
    for k in &c.keys {
        let Some(key) = s1.find_key(&k.name, &k.version).map_err(e).expect("find_key") else {
            continue;
        };
        theirs +=
            s1.copies(key).map_err(e).expect("COPIES").iter().filter(|r| r.file == id.0).count();
    }
    x.eq("odr.copies.len", &c.odr.copies.len(), &theirs);
    for cp in &c.odr.copies {
        let k = &c.keys[cp.key as usize];
        let key = s1.find_key(&k.name, &k.version).map_err(e).expect("find_key");
        let rows = key.map(|key| s1.copies(key).map_err(e).expect("COPIES")).unwrap_or_default();
        let full = rows.iter().find(|r| r.file == id.0).map(|r| (r.index, r.size, r.typ));
        x.eq("odr.copy", &Some((cp.index, cp.size, cp.typ)), &full);
    }
    x.eq("debug (no -g)", &c.debug, &false);
    x.eq("meta_units (GCC)", &c.meta_units, &0);
    x.eq("versioned", &c.versioned, &0);
    x.eq("index (P-1b)", &c.index.len(), &0);
    x.eq("edges (P-1b)", &c.edges.len(), &0);
    x
}

// spec: WP-267 §1 rows 6, 7a, 7b, 7d, 7e, 7h, 7i (incr.md IN-70 U4): the
// changed file's rows from its bytes alone are the full link's rows.
#[test]
fn changed_rows_are_the_full_links_rows() {
    let mut ran = 0;
    for line in lines(&[("p1", "c++", P1_V0)]) {
        let mut args = line.base.clone();
        args.push(line.dir.join("p1.o").to_string_lossy().into_owned());
        let s0 = state(&line, &args, "out0");
        line.compile("p1", "c++", P1_V1);
        let s1 = state(&line, &args, "out1");
        let id = file_named(&s0, "p1.o");
        let store = support::store();
        let mut cache = FrontCache::new(&store, pool(), &line.dir);
        let plan = plan_of(&line, &args);
        let mut cf = changed_files_of(&s0, &mut cache, &[id], &[], &plan, &line.dir)
            .unwrap_or_else(|e| panic!("{}: changed_files_of: {}", line.label, e.report()));
        // After resolve: the update's own election's keepers.
        let options = mortar_resolve::tables::option_keys(&plan, &[]);
        let ex = mortar_incr::update::extract::extract(&s0, &cf, &options).expect("extract");
        let d = match mortar_incr::update::resolve::resolve(&s0, &cf, &ex, &options)
            .unwrap_or_else(|e| panic!("{}: resolve: {}", line.label, e.report()))
        {
            mortar_incr::update::resolve::Resolution::Delta(d) => d,
            other @ mortar_incr::update::resolve::Resolution::DrawIn(_) => {
                panic!("{}: resolve: {other:?}", line.label)
            }
        };
        let f = whole(&line, &args, &store);
        facts_after_resolve(&s0, &cache, &mut cf, &plan, f.arch.get(), &d.keepers)
            .unwrap_or_else(|e| panic!("{}: facts_after_resolve: {}", line.label, e.report()));
        let x = against_full_link(&line.label, &s0, &s1, &cf, id);
        eprintln!("{}: {} fields compared, {} differ", line.label, x.n, x.bad.len());
        assert!(x.bad.is_empty(), "{}: {}", line.label, x.bad.join("\n"));
        // The comparison bites: a name one byte off, a mention's binding, the
        // hash and the properties each found by name.
        let mut bad = cf.clone();
        let b = &mut bad.files[0];
        b.sections[1].name.off += 1;
        b.mentions[0].bind ^= 1;
        b.row.hash[0] ^= 1;
        let props = b.row.properties.len;
        b.row.properties.len = props.saturating_sub(1);
        let y = against_full_link(&line.label, &s0, &s1, &bad, id);
        let named = |f: &str| y.bad.iter().any(|l| l.starts_with(f));
        assert!(
            named("section 1 name")
                && named("mention 0 (kind")
                && named("row.hash")
                && named("row.properties"),
            "{}: a mutation not caught: {:?}",
            line.label,
            y.bad
        );
        assert!(props > 0, "{}: no FILES.properties: the check cannot bite", line.label);
        // The checks bite: the edit added a COMDAT copy and sections.
        assert!(!cf.files[0].odr.copies.is_empty(), "{}: no COMDAT copy", line.label);
        let old = s0.sections_of(id).expect("SECTIONS").len();
        assert!(cf.files[0].sections.len() > old, "{}: the edit added no section", line.label);
        ran += 1;
    }
    if mortar_testutil::tools_required() {
        assert_eq!(ran, 2, "both targets ran");
    }
}

// spec: WP-267 §1 M-1, review §2 (`--wrap`): refused by name; an
// unchanged file has no row.
#[test]
fn changed_rows_refuse_by_name() {
    let mut ran = 0;
    for line in lines(&[("p1", "c++", P1_V0)]) {
        line.archive(&["m1.o", "m2.o"]);
        let mut args = line.args(&["wp255d_m1"]);
        args.push(line.dir.join("p1.o").to_string_lossy().into_owned());
        let s0 = state(&line, &args, "out0");
        let store = support::store();
        let plan = plan_of(&line, &args);
        let p1 = file_named(&s0, "p1.o");
        let mut cache = FrontCache::new(&store, pool(), &line.dir);
        // Unchanged: no row.
        let cf = changed_files_of(&s0, &mut cache, &[p1], &[], &plan, &line.dir)
            .unwrap_or_else(|e| panic!("{}: unchanged: {}", line.label, e.report()));
        assert!(cf.files.is_empty(), "{}: an unchanged file has a row", line.label);
        // `--wrap`: (scan), before any file is read.
        let mut wrapped = args.clone();
        wrapped.push("--wrap=p1_f".into());
        let wplan = plan_of(&line, &wrapped);
        let r = changed_files_of(&s0, &mut cache, &[p1], &[], &wplan, &line.dir);
        assert_eq!(
            r.err(),
            Some(IncrReason::UpdateNotImplemented(Some(Unimplemented::Scan))),
            "{}: --wrap",
            line.label
        );
        // M-1: a changed archive's member, (resolve).
        line.compile("m1", "c", &format!("{}int wp267_p1_extra = 5;\n", update::M1));
        line.archive(&["m1.o", "m2.o"]);
        let m1 = update::member(&s0, "m1.o");
        let r = changed_files_of(&s0, &mut cache, &[m1], &[], &plan, &line.dir);
        assert_eq!(
            r.err(),
            Some(IncrReason::UpdateNotImplemented(Some(Unimplemented::Resolve))),
            "{}: a changed member",
            line.label
        );
        ran += 1;
    }
    if mortar_testutil::tools_required() {
        assert_eq!(ran, 2, "both targets ran");
    }
}

/// One mortar run of `args` (output `out`, `extra` after), findings into
/// `findings`.
fn link(line: &Line, args: &[String], extra: &[&str], findings: &Path) {
    let mut a = with_output(args, &line.dir.join("u"));
    a.extend(extra.iter().map(OsString::from));
    let mut c = Command::new(MORTAR);
    c.args(&a).current_dir(&line.dir).env_remove("MORTAR_SCHED_SEED");
    c.env_remove("SOURCE_DATE_EPOCH").env("MORTAR_UPDATE_FINDINGS", findings);
    c.env("MORTAR_INCR_SMALL_OUTPUT", "0");
    let o = run_with(&mut c, b"", Duration::from_mins(2));
    assert!(o.success(), "{}: mortar {extra:?}: {}", line.label, o.stderr_text());
}

// spec: WP-267 §4 P-1 (`changed_rows_equal_the_fronts`), §6 R-1: on the
// real path, P-1's rows are front 1's, field by field.
#[test]
fn changed_rows_equal_the_fronts() {
    let mut ran = 0;
    for line in lines(&[("p1", "c++", P1_V0)]) {
        let mut args = line.base.clone();
        args.push(line.dir.join("p1.o").to_string_lossy().into_owned());
        let findings = line.dir.join("findings.txt");
        let one = |a: &[String], hop: usize| -> String {
            std::fs::write(&findings, b"").expect("findings");
            link(&line, a, &["--incremental"], &findings);
            let lines = std::fs::read_to_string(&findings).expect("findings");
            let p1: Vec<&str> =
                lines.lines().filter(|l| l.starts_with("P-1") || l.contains(" P-1 ")).collect();
            eprintln!("{}: hop {hop}: {p1:?}", line.label);
            assert_eq!(p1.len(), 1, "{}: hop {hop}: one P-1 line: {lines}", line.label);
            p1[0].to_owned()
        };
        link(&line, &args, &["--incremental=full"], &findings);
        for (hop, text) in [(1, P1_V1), (2, P1_V0), (3, P1_V1)] {
            line.compile("p1", "c++", text);
            let l = one(&args, hop);
            assert!(l.starts_with("P-1: equal (1 files"), "{}: hop {hop}: {l}", line.label);
        }
        // `--wrap`: refused by name (the update falls back by its own name).
        let mut wrapped = args.clone();
        wrapped.push("--wrap=p1_renamed".into());
        link(&line, &wrapped, &["--incremental=full"], &findings);
        line.compile("p1", "c++", P1_V0);
        let l = one(&wrapped, 4);
        assert!(
            l.starts_with("P-1: refused by name: update-not-implemented (scan)"),
            "{}: hop 4: {l}",
            line.label
        );
        ran += 1;
    }
    if mortar_testutil::tools_required() {
        assert_eq!(ran, 2, "both targets ran");
    }
}

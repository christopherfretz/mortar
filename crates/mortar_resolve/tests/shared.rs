//! Shared objects in a link (`spec/dyn.md` D-11 to D-21; `spec/resolve.md`
//! §3.2 rules 2-3, §3.5), over hand-built views: one test per rule, named
//! `d<rule>_<what>`. Every link runs under `check_schedules`.

#![allow(clippy::many_single_char_names, reason = "files and keys are short names here")]

mod support;

use mortar_core::par::check_schedules;
use mortar_core::resolution::FileLive;
use mortar_core::{FileId, SymEntry};
use mortar_elf::Machine;
use mortar_elf::consts::STV_HIDDEN;
use mortar_resolve::options::{Entry, MentionGroup, Options, SharedMention, ShlibUndefined};
use mortar_resolve::problem::{Severity, What};
use mortar_resolve::shared::Dynamic;
use mortar_resolve::{Def, Resolved, resolve};
use support::Model;

const A64: Machine = Machine::AArch64;

fn opts() -> Options {
    Options { entry: Entry::None, ..Options::default() }
}

fn link_with(m: &Model, o: &Options) -> Resolved {
    check_schedules(|pool| resolve(m, o, pool))
}

fn link(m: &Model) -> Resolved {
    link_with(m, &opts())
}

fn dynamic(r: &Resolved) -> &Dynamic {
    r.dynamic.as_ref().expect("a shared object is loaded")
}

fn ids(r: &Resolved) -> Vec<&'static str> {
    r.problems.iter().map(mortar_resolve::Problem::id).collect()
}

fn def(m: &Model, r: &Resolved, name: &str) -> Def {
    r.def[m.id(name).0 as usize]
}

fn row(r: &Resolved, f: FileId) -> FileLive {
    r.file_live[f.0 as usize]
}

const NEEDED: FileLive = FileLive::Shared { needed: true, used: false };
const USED: FileLive = FileLive::Shared { needed: false, used: true };

/// Mentions in `FileId` order at positions 1, 2, …, with `as_needed` as
/// the file's.
fn mentions(m: &Model, o: &mut Options, files: &[(FileId, u32)]) {
    o.shared.position = vec![u32::MAX; m.files.len()];
    for (f, file) in m.files.iter().enumerate() {
        if file.shared.is_none() && f > 0 {
            o.shared.position[f] = 0;
        }
    }
    for &(file, position) in files {
        let info = m.files[file.0 as usize].shared.unwrap();
        o.shared.mentions.push(SharedMention {
            file,
            position,
            as_needed: info.as_needed,
            group: MentionGroup::None,
        });
    }
    o.shared.mentions.sort_by_key(|m| m.position);
}

/// One more mention of `file` at `position` with its own state (a script
/// entry, WP-172: the state in force where its script was named,
/// `AS_NEEDED(…)` forcing it on).
fn mention_at(o: &mut Options, file: FileId, position: u32, as_needed: bool, group: Option<u32>) {
    let group = group.map_or(MentionGroup::None, MentionGroup::In);
    o.shared.mentions.push(SharedMention { file, position, as_needed, group });
    o.shared.mentions.sort_by_key(|m| m.position);
}

#[test]
fn d11_item1_not_as_needed_is_always_needed() {
    let mut m = Model::new(A64);
    m.obj("a.o");
    let lib = m.shlib("libx.so", false);
    m.def(lib, "x");
    let r = link(&m);
    assert!(ids(&r).is_empty());
    assert_eq!(row(&r, lib), NEEDED);
    assert_eq!(dynamic(&r).needed, vec![lib]);
}

#[test]
fn d11_as_needed_unreferenced_is_dead() {
    let mut m = Model::new(A64);
    m.obj("a.o");
    let lib = m.shlib("libx.so", true);
    m.def(lib, "x");
    let r = link(&m);
    assert_eq!(row(&r, lib), FileLive::Dead);
    assert!(dynamic(&r).needed.is_empty());
}

#[test]
fn d11_item2_weak_reference_alone_needs_nothing() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.weak_undef(a, "f");
    let lib = m.shlib("libf.so", true);
    m.def(lib, "f");
    let r = link(&m);
    assert_eq!(row(&r, lib), FileLive::Dead);
    assert_eq!(def(&m, &r, "f"), Def::Undefined { weak: true });
}

#[test]
fn d11_item2_giver_is_first_by_file_id() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.undef(a, "f");
    let l1 = m.shlib("lib1.so", true);
    let i1 = m.def(l1, "f");
    let l2 = m.shlib("lib2.so", true);
    m.def(l2, "f");
    let r = link(&m);
    assert!(ids(&r).is_empty());
    assert_eq!(row(&r, l1), NEEDED);
    assert_eq!(row(&r, l2), FileLive::Dead);
    assert_eq!(def(&m, &r, "f"), Def::Shared(SymEntry { file: l1, index: i1 }));
    assert!(dynamic(&r).imported[m.id("f").0 as usize]);
}

#[test]
fn d11_program_definition_gives_nothing() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.undef(a, "f");
    let b = m.obj("b.o");
    m.def(b, "f");
    let lib = m.shlib("libf.so", true);
    m.def(lib, "f");
    let r = link(&m);
    assert_eq!(row(&r, lib), FileLive::Dead);
    assert!(matches!(def(&m, &r, "f"), Def::Strong(_)));
}

#[test]
fn d11_item3_needed_object_needs_its_giver() {
    let mut m = Model::new(A64);
    m.obj("a.o");
    let x = m.shlib("libx.so", false);
    m.undef(x, "g");
    let s = m.shlib("libs.so", true);
    m.def(s, "g");
    let r = link(&m);
    assert_eq!(row(&r, x), NEEDED);
    assert_eq!(row(&r, s), NEEDED);
}

#[test]
fn d11_item3_reachable_giver_is_only_used() {
    let mut m = Model::new(A64);
    m.obj("a.o");
    let x = m.shlib("libx.so", false);
    m.undef(x, "g");
    let s = m.shlib("libs.so", true);
    m.def(s, "g");
    m.needs(x, Some(s));
    let r = link(&m);
    assert_eq!(row(&r, x), NEEDED);
    assert_eq!(row(&r, s), USED);
    assert_eq!(dynamic(&r).used, vec![s]);
    assert_eq!(dynamic(&r).needed, vec![x]);
}

#[test]
fn d12_closure_definition_stays_undefined_with_note() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.undef(a, "h");
    let x = m.shlib("libx.so", false);
    let w = m.closure_lib("libw.so");
    m.def(w, "h");
    m.needs(x, Some(w));
    let r = link(&m);
    assert_eq!(row(&r, w), USED);
    assert_eq!(def(&m, &r, "h"), Def::Undefined { weak: false });
    assert_eq!(dynamic(&r).closure_only, vec![(m.id("h"), w)]);
}

/// `m1.o --as-needed libu.so libg.a` and its variants (`spec/dyn.md` D-12,
/// the stages; WP-167): `a.o` calls `u` of `libu.so`, which refers to `g`,
/// which only `libg.a(g.o)` defines.
fn row1(as_needed: bool) -> (Model, FileId, FileId, FileId) {
    let mut m = Model::new(A64);
    let a = m.obj("m1.o");
    m.undef(a, "u");
    let u = m.shlib("libu.so", as_needed);
    m.def(u, "u");
    m.undef(u, "g");
    let ar = m.archive();
    let g = m.member(ar, "g.o");
    m.def(g, "g");
    (m, a, u, g)
}

#[test]
fn d12_needed_as_needed_object_extracts_in_a_second_stage() {
    for as_needed in [false, true] {
        let (m, _, u, g) = row1(as_needed);
        let mut o = opts();
        o.shared.shlib_undefined = ShlibUndefined::Error;
        let r = link_with(&m, &o);
        assert_eq!(row(&r, u), NEEDED);
        assert!(row(&r, g).is_live());
        // Only the last stage's diagnostics: stage 1's undefined `g` of
        // libu.so is not reported.
        assert!(r.problems.is_empty(), "{:?}", r.problems);
        assert!(dynamic(&r).exported[m.id("g").0 as usize]);
        assert!(matches!(def(&m, &r, "g"), Def::Strong(e) if e.file == g));
        // The member's pulling reference is libu.so's.
        assert!(matches!(row(&r, g), FileLive::Extracted(e) if e.file == u));
        assert_eq!(r.stages, if as_needed { 2 } else { 1 });
    }
}

#[test]
fn d12_unneeded_as_needed_object_extracts_nothing() {
    // `m0.o --as-needed libu.so libg.a`: libu.so is not needed (GNU ld, mold).
    let mut m = Model::new(A64);
    m.obj("m0.o");
    let u = m.shlib("libu.so", true);
    m.def(u, "u");
    m.undef(u, "g");
    let ar = m.archive();
    let g = m.member(ar, "g.o");
    m.def(g, "g");
    let r = link(&m);
    assert_eq!(row(&r, u), FileLive::Dead);
    assert!(!row(&r, g).is_live());
    assert_eq!(r.stages, 1);
    assert!(r.problems.is_empty(), "{:?}", r.problems);
}

#[test]
fn d12_weak_reference_and_live_definition_start_no_stage() {
    // A weak reference of libu.so extracts nothing; a key an object already
    // defines is no candidate (7b): one stage each.
    let mut m = Model::new(A64);
    let a = m.obj("m1.o");
    m.undef(a, "u");
    let u = m.shlib("libu.so", true);
    m.def(u, "u");
    m.weak_undef(u, "g");
    m.undef(u, "h");
    m.def(a, "h");
    let ar = m.archive();
    let g = m.member(ar, "g.o");
    m.def(g, "g");
    m.def(g, "h");
    let r = link(&m);
    assert_eq!(row(&r, u), NEEDED);
    assert!(!row(&r, g).is_live());
    assert_eq!(r.stages, 1);
    assert!(r.problems.is_empty(), "{:?}", r.problems);
}

#[test]
fn d12_object_needed_at_an_earlier_stage_stays_needed() {
    // 2a: `m1y.o --as-needed libuy.so libgy.a`, gy.o defines `g` and `y`:
    // libuy.so was needed for `y`; with gy.o live the program defines `y`,
    // and libuy.so stays needed (GNU ld and mold).
    let mut m = Model::new(A64);
    let a = m.obj("m1y.o");
    m.undef(a, "y");
    let u = m.shlib("libuy.so", true);
    m.def(u, "y");
    m.undef(u, "g");
    let ar = m.archive();
    let g = m.member(ar, "gy.o");
    m.def(g, "g");
    m.def(g, "y");
    let r = link(&m);
    assert_eq!(row(&r, u), NEEDED);
    assert_eq!(dynamic(&r).needed, vec![u]);
    assert!(matches!(def(&m, &r, "y"), Def::Strong(e) if e.file == g));
    assert_eq!(r.stages, 2);
    assert!(r.problems.is_empty(), "{:?}", r.problems);
}

#[test]
fn d12_stages_run_to_their_fixed_point() {
    // 2d: libu.so -> gv.o -> libvw.so (as-needed, needed only by gv.o) ->
    // libw.a(w.o): three stages.
    let mut m = Model::new(A64);
    let a = m.obj("m1.o");
    m.undef(a, "u");
    let u = m.shlib("libu.so", true);
    m.def(u, "u");
    m.undef(u, "g");
    let ar = m.archive();
    let gv = m.member(ar, "gv.o");
    m.def(gv, "g");
    m.undef(gv, "v");
    let v = m.shlib("libvw.so", true);
    m.def(v, "v");
    m.undef(v, "w");
    let ar2 = m.archive();
    let w = m.member(ar2, "w.o");
    m.def(w, "w");
    let r = link(&m);
    assert_eq!(row(&r, u), NEEDED);
    assert_eq!(row(&r, v), NEEDED);
    assert!(row(&r, gv).is_live());
    assert!(matches!(row(&r, w), FileLive::Extracted(e) if e.file == v));
    assert_eq!(dynamic(&r).needed, vec![u, v]);
    assert_eq!(r.stages, 3);
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    let d = dynamic(&r);
    assert!(d.exported[m.id("g").0 as usize] && d.exported[m.id("w").0 as usize]);
}

#[test]
fn d12_a_member_the_next_stage_extracts_is_no_item3_reason() {
    // 7a: `m1.o --as-needed libu.so libg.a libg2.so`: libg2.so also defines
    // `g`, after the member: the member is extracted and libg2.so is not
    // needed (all three). With libg2.so before the archive, `first(g)` is
    // libg2.so: nothing is extracted and D-11 item 3 makes it needed (GNU ld).
    for g2_first in [false, true] {
        let mut m = Model::new(A64);
        let a = m.obj("m1.o");
        m.undef(a, "u");
        let u = m.shlib("libu.so", true);
        m.def(u, "u");
        m.undef(u, "g");
        let g2 = if g2_first { Some(m.shlib("libg2.so", true)) } else { None };
        let ar = m.archive();
        let g = m.member(ar, "g.o");
        m.def(g, "g");
        let g2 = g2.unwrap_or_else(|| m.shlib("libg2.so", true));
        m.def(g2, "g");
        let r = link(&m);
        assert_eq!(row(&r, u), NEEDED);
        if g2_first {
            assert!(!row(&r, g).is_live());
            assert_eq!(row(&r, g2), NEEDED);
            assert_eq!(r.stages, 1);
        } else {
            assert!(row(&r, g).is_live());
            assert_eq!(row(&r, g2), FileLive::Dead);
            assert_eq!(dynamic(&r).needed, vec![u]);
            assert_eq!(r.stages, 2);
        }
        assert!(r.problems.is_empty(), "{:?}", r.problems);
    }
}

#[test]
fn d12_used_and_closure_references_extract_nothing_and_name_the_member() {
    // 5: libtop.so (needed) lists libuc.so, which refers to `g` of
    // libg.a(g.o). libuc.so used (on the line, --as-needed) or a closure
    // file: no edge; D-13's undefined-symbol carries the member.
    for closure in [false, true] {
        let mut m = Model::new(A64);
        let a = m.obj("m1t.o");
        m.undef(a, "t");
        let top = m.shlib("libtop.so", true);
        m.def(top, "t");
        let uc = if closure { m.closure_lib("libuc.so") } else { m.shlib("libuc.so", true) };
        m.def(uc, "u");
        m.undef(uc, "g");
        m.needs(top, Some(uc));
        let ar = m.archive();
        let g = m.member(ar, "g.o");
        m.def(g, "g");
        let mut o = opts();
        o.shared.shlib_undefined = ShlibUndefined::Error;
        let r = link_with(&m, &o);
        assert_eq!(row(&r, uc), USED);
        assert!(!row(&r, g).is_live());
        assert_eq!(r.stages, 1);
        assert_eq!(ids(&r), vec!["undefined-symbol"]);
        assert!(
            matches!(r.problems[0].what, What::ShlibUndefined { member: Some((f, c)), .. }
                if f == g && c == closure),
            "{:?}",
            r.problems
        );
    }
}

#[test]
fn d13_undefined_in_a_shared_object() {
    for (mode, want) in [
        (ShlibUndefined::Allow, None),
        (ShlibUndefined::Error, Some(Severity::Error)),
        (ShlibUndefined::Warn, Some(Severity::Warning)),
    ] {
        let mut m = Model::new(A64);
        m.obj("a.o");
        let x = m.shlib("libx.so", false);
        m.undef(x, "missing");
        m.weak_undef(x, "optional");
        let mut o = opts();
        o.shared.shlib_undefined = mode;
        let r = link_with(&m, &o);
        let got: Vec<_> = r.problems.iter().map(|p| (p.id(), p.severity())).collect();
        assert_eq!(got, want.map(|s| ("undefined-symbol", s)).into_iter().collect::<Vec<_>>());
    }
}

#[test]
fn d13_program_definition_satisfies_and_is_exported() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.def(a, "cb");
    m.def(a, "unrelated");
    let x = m.shlib("libx.so", false);
    m.undef(x, "cb");
    let mut o = opts();
    o.shared.shlib_undefined = ShlibUndefined::Error;
    let r = link_with(&m, &o);
    assert!(ids(&r).is_empty());
    let d = dynamic(&r);
    assert!(d.ref_dynamic[m.id("cb").0 as usize]);
    assert!(d.exported[m.id("cb").0 as usize]);
    assert!(!d.exported[m.id("unrelated").0 as usize]);
}

#[test]
fn d21_export_dynamic_exports_every_visible_definition() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.def(a, "f");
    let h = m.def(a, "h");
    m.set_visibility(a, h, STV_HIDDEN);
    m.shlib("libx.so", false);
    let mut o = opts();
    o.shared.export_dynamic = true;
    let r = link_with(&m, &o);
    let d = dynamic(&r);
    assert!(d.exported[m.id("f").0 as usize]);
    assert!(!d.exported[m.id("h").0 as usize]);
}

/// WP-148: a dynamic executable of objects alone exports by §9.6 too:
/// `--export-dynamic` every visible definition, `--export-dynamic-symbol`
/// its keys. Without either (or in a static link) there is no shared
/// stage, so a link that exports nothing pays nothing.
#[test]
fn d21_exports_without_a_shared_input() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.def(a, "f");
    m.def(a, "g");
    let h = m.def(a, "h");
    m.set_visibility(a, h, STV_HIDDEN);
    let mut o = opts();
    o.output.dynamic = true;
    assert!(link_with(&m, &o).dynamic.is_none());
    o.shared.export_symbols = vec![m.id("g")];
    let r = link_with(&m, &o);
    let d = r.dynamic.as_ref().expect("the line exports");
    assert!(!d.exported[m.id("f").0 as usize]);
    assert!(d.exported[m.id("g").0 as usize]);
    o.shared.export_dynamic = true;
    let r = link_with(&m, &o);
    let d = r.dynamic.as_ref().expect("the line exports");
    assert!(d.exported[m.id("f").0 as usize]);
    assert!(d.exported[m.id("g").0 as usize]);
    assert!(!d.exported[m.id("h").0 as usize]);
    // A static executable or static-pie (`spec/dyn.md` D-1): no stage.
    o.output.dynamic = false;
    assert!(link_with(&m, &o).dynamic.is_none());
}

#[test]
fn d21_program_definition_interposes_a_shared_one() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.def(a, "f");
    let x = m.shlib("libx.so", false);
    m.def(x, "f");
    let r = link(&m);
    let d = dynamic(&r);
    assert!(d.defined_dynamic[m.id("f").0 as usize]);
    assert!(d.exported[m.id("f").0 as usize]);
    assert!(!d.imported[m.id("f").0 as usize]);
}

#[test]
fn s3_5_hidden_program_definition_is_non_exported() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    let h = m.def(a, "h");
    m.set_visibility(a, h, STV_HIDDEN);
    let x = m.shlib("libx.so", false);
    m.undef(x, "h");
    let r = link(&m);
    assert_eq!(ids(&r), vec!["non-exported-symbol"]);
    assert!(!dynamic(&r).exported[m.id("h").0 as usize]);
}

#[test]
fn s3_2_rule2_common_yields_to_shared_data() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.common(a, "x", 4, 4);
    let lib = m.shlib("libx.so", true);
    let i = m.data(lib, "x", 4);
    let r = link(&m);
    assert_eq!(row(&r, lib), NEEDED);
    assert_eq!(def(&m, &r, "x"), Def::Shared(SymEntry { file: lib, index: i }));
    // The common is demoted: a reference of the shared definition,
    // allocated nowhere.
    assert!(dynamic(&r).imported[m.id("x").0 as usize]);
    assert!(r.common.is_empty());
}

#[test]
fn s3_2_rule3_linker_names_never_bind_shared() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.undef(a, "_end");
    let lib = m.shlib("libx.so", false);
    m.def(lib, "_end");
    let r = link(&m);
    assert!(matches!(def(&m, &r, "_end"), Def::Provide { .. }));
}

#[test]
fn d16_tls_mismatch_against_shared_definition() {
    let mut m = Model::new(A64);
    let a = m.obj("data.o");
    m.undef(a, "tv");
    let lib = m.shlib("libtv.so", false);
    m.tls(lib, "tv");
    let r = link(&m);
    assert_eq!(ids(&r), vec!["tls-mismatch"]);
}

#[test]
fn d19a_bind_follows_keeping_mentions_not_file_ids() {
    // libc.so (as-needed) at 1 with no reason yet, libA.so (not
    // as-needed) at 2, a.o at 3 referring to f, libc.so again at 4: the
    // giver is libc (first by FileId), its keeping mention 4, so the
    // loader order is libA, libc and f binds to libA.
    let mut m = Model::new(A64);
    let c = m.shlib("libc.so", true);
    m.def(c, "f");
    let la = m.shlib("libA.so", false);
    let ia = m.def(la, "f");
    let a = m.obj("a.o");
    m.undef(a, "f");
    let mut o = opts();
    mentions(&m, &mut o, &[(c, 1), (la, 2), (c, 4)]);
    o.shared.position[a.0 as usize] = 3;
    let r = link_with(&m, &o);
    assert!(ids(&r).is_empty(), "{:?}", ids(&r));
    assert_eq!(dynamic(&r).needed, vec![la, c]);
    assert_eq!(def(&m, &r, "f"), Def::Shared(SymEntry { file: la, index: ia }));
}

// WP-172, the cells of `spec/landed/dyn/WP-172.md` §2: a shared object
// mentioned again inside a linker script is placed by that mention's own
// state. Positions: x (as-needed) at 1, b (plain) at 2, then the script's
// mention of x.

/// `--as-needed libx.so --no-as-needed libb.so s_x.so` (`s_named_off`):
/// the script's entry is plain, x is needed there, after b (GNU ld).
#[test]
fn d19a_script_mention_plain_keeps_it_there() {
    let mut m = Model::new(A64);
    m.obj("a.o");
    // Merged over its mentions (on only if on at every one): off.
    let x = m.shlib("libx.so", false);
    let b = m.shlib("libb.so", false);
    let mut o = opts();
    mentions(&m, &mut o, &[(b, 2)]);
    mention_at(&mut o, x, 1, true, None);
    mention_at(&mut o, x, 3, false, None);
    let r = link_with(&m, &o);
    assert!(ids(&r).is_empty(), "{:?}", ids(&r));
    assert_eq!(dynamic(&r).needed, vec![b, x]);
}

/// The same script named under `--as-needed` (`s_named_on`, `s_push_on`):
/// every mention is as-needed and nothing refers to x: not needed.
#[test]
fn d19a_script_mention_as_needed_unreferenced_is_dead() {
    let mut m = Model::new(A64);
    m.obj("a.o");
    let x = m.shlib("libx.so", true);
    let b = m.shlib("libb.so", false);
    let mut o = opts();
    mentions(&m, &mut o, &[(x, 1), (b, 2)]);
    mention_at(&mut o, x, 3, true, None);
    let r = link_with(&m, &o);
    assert!(ids(&r).is_empty(), "{:?}", ids(&r));
    assert_eq!(dynamic(&r).needed, vec![b]);
    assert_eq!(row(&r, x), FileLive::Dead);
}

/// `… s_x.so liby.so s_x.so` (`s_twice_off`, the base refused it): the
/// first plain mention keeps x, before y.
#[test]
fn d19a_two_script_mentions_first_plain_keeps() {
    let mut m = Model::new(A64);
    m.obj("a.o");
    let x = m.shlib("libx.so", false);
    let b = m.shlib("libb.so", false);
    let y = m.shlib("liby.so", false);
    let mut o = opts();
    mentions(&m, &mut o, &[(b, 2), (y, 4)]);
    mention_at(&mut o, x, 1, true, None);
    mention_at(&mut o, x, 3, false, None);
    mention_at(&mut o, x, 5, false, None);
    let r = link_with(&m, &o);
    assert!(ids(&r).is_empty(), "{:?}", ids(&r));
    assert_eq!(dynamic(&r).needed, vec![b, x, y]);
}

/// `--as-needed libx.so --no-as-needed libb.so --as-needed g.so` with
/// `g.so` = `GROUP ( s_x.so use_x.o )` (`nested_group`, refused before):
/// the nested entry is as-needed and in the group of `use_x.o`, which
/// refers to x after it: kept at the script's mention, after b.
#[test]
fn d19a_nested_script_mention_in_group_keeps_it_there() {
    let mut m = Model::new(A64);
    let x = m.shlib("libx.so", true);
    let b = m.shlib("libb.so", false);
    let u = m.obj("use_x.o");
    m.undef(u, "fx");
    m.def(x, "fx");
    m.set_group(u, 1);
    let mut o = opts();
    mentions(&m, &mut o, &[(x, 1), (b, 2)]);
    mention_at(&mut o, x, 3, true, Some(1));
    o.shared.position[u.0 as usize] = 4;
    let r = link_with(&m, &o);
    assert!(ids(&r).is_empty(), "{:?}", ids(&r));
    assert_eq!(dynamic(&r).needed, vec![b, x]);
}

/// `g.so` = `INPUT ( libx.so ) GROUP ( liby.so use_x.o )`
/// (`input_then_group`): the `INPUT` entry is in no group, so no mention
/// of x qualifies (GNU ld refuses the line, `undefined reference`) and x
/// stays at its first mention, as lld and mold write it. The base took
/// the group of the script's other entries and wrote `b x`.
#[test]
fn d19a_input_entry_beside_a_group_is_in_no_group() {
    let mut m = Model::new(A64);
    let x = m.shlib("libx.so", true);
    let b = m.shlib("libb.so", false);
    let u = m.obj("use_x.o");
    m.undef(u, "fx");
    m.def(x, "fx");
    m.set_group(u, 1);
    let mut o = opts();
    mentions(&m, &mut o, &[(x, 1), (b, 2)]);
    mention_at(&mut o, x, 3, true, None);
    o.shared.position[u.0 as usize] = 4;
    let r = link_with(&m, &o);
    assert!(ids(&r).is_empty(), "{:?}", ids(&r));
    assert_eq!(dynamic(&r).needed, vec![x, b]);
}

#[test]
fn d11_hidden_reference_never_binds_shared() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    let i = m.undef(a, "f");
    m.set_visibility(a, i, STV_HIDDEN);
    let lib = m.shlib("libf.so", false);
    m.def(lib, "f");
    let r = link(&m);
    assert_eq!(def(&m, &r, "f"), Def::Undefined { weak: false });
}

#[test]
fn d27_versioned_reference_binds_the_version() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.undef(a, "f@V1");
    let lib = m.shlib("libf.so", true);
    let i = m.def(lib, "f@@V1");
    m.set_version(lib, i, 2);
    let r = link(&m);
    assert_eq!(row(&r, lib), NEEDED);
    assert_eq!(def(&m, &r, "f@V1"), Def::Shared(SymEntry { file: lib, index: i }));
}

/// WP-200 (resolve.md §6.2, the supervisor's ruling): `wa.o` keeps group
/// G, `wb.o`'s dropped copy defines a global `hx` that `libhx.so`
/// (`--as-needed`) defines too. The demoted mention makes the library
/// needed only if another reference does; with `z` from `a.o` it binds to
/// the library's `hx`.
#[test]
fn d11_item2_demoted_strong_mention_needs_nothing() {
    use mortar_elf::consts::{STB_GLOBAL, STT_FUNC};
    use mortar_elf::{SectionIndex, SymbolSection};
    for other in [false, true] {
        let mut m = Model::new(A64);
        let a = m.obj("a.o");
        m.undef(a, "g");
        if other {
            m.undef(a, "z");
        }
        let wa = m.obj("wa.o");
        m.comdat_def(wa, "G", "g");
        let wb = m.obj("wb.o");
        let s = m.comdat_def(wb, "G", "g");
        m.sym(wb, "hx", STB_GLOBAL, STT_FUNC, SymbolSection::Section(SectionIndex(s)), 4, 4);
        let lib = m.shlib("libhx.so", true);
        let i = m.def(lib, "hx");
        m.def(lib, "z");
        let r = link(&m);
        if other {
            assert_eq!(row(&r, lib), NEEDED);
            assert_eq!(def(&m, &r, "hx"), Def::Shared(SymEntry { file: lib, index: i }));
        } else {
            assert_eq!(row(&r, lib), FileLive::Dead);
            assert_eq!(def(&m, &r, "hx"), Def::Undefined { weak: false });
            assert_eq!(r.comdat_mismatch.len(), 1);
        }
    }
}

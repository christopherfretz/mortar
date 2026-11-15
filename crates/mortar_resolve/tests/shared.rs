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
            as_needed: Some(info.as_needed),
            group: MentionGroup::None,
        });
    }
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

#[test]
fn d12_references_of_as_needed_objects_do_not_extract() {
    // Q1: `m1.o --as-needed libu.so libg.a` (needed) is refused by name;
    // without --as-needed the member is extracted.
    for as_needed in [false, true] {
        let mut m = Model::new(A64);
        let a = m.obj("m1.o");
        m.undef(a, "f");
        let u = m.shlib("libu.so", as_needed);
        m.def(u, "f");
        m.undef(u, "g");
        let ar = m.archive();
        let g = m.member(ar, "g.o");
        m.def(g, "g");
        let r = link(&m);
        assert_eq!(row(&r, u), NEEDED);
        if as_needed {
            assert!(!row(&r, g).is_live());
            assert_eq!(ids(&r), vec!["refused"]);
            assert!(
                matches!(r.problems[0].what, What::AsNeededExtraction { member, .. } if member == g)
            );
        } else {
            assert!(row(&r, g).is_live());
            assert!(ids(&r).is_empty(), "{:?}", ids(&r));
            assert!(dynamic(&r).exported[m.id("g").0 as usize]);
        }
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

#[test]
fn d19a_unknown_as_needed_state_is_refused() {
    let mut m = Model::new(A64);
    m.obj("a.o");
    let x = m.shlib("libx.so", false);
    let mut o = opts();
    mentions(&m, &mut o, &[(x, 1)]);
    o.shared.mentions[0].as_needed = None;
    let r = link_with(&m, &o);
    assert_eq!(ids(&r), vec!["refused"]);
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

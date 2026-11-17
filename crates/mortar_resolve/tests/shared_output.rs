//! A shared object's own resolution (WP-117; `spec/dyn.md` D-110, D-112,
//! D-118), over hand-built views: the shared stage runs for a `Shared`
//! output with or without shared inputs, fills `exported` by D-110 and
//! `preemptible` by D-112. Every link runs under `check_schedules`.

#![allow(clippy::many_single_char_names, reason = "files and keys are short names here")]

mod support;

use mortar_core::par::check_schedules;
use mortar_core::plan::{OutputKind, Plan, Symbolic, SymbolicLine};
use mortar_elf::Machine;
use mortar_elf::consts::{STV_HIDDEN, STV_PROTECTED};
use mortar_resolve::options::{Entry, Options, Output, SharedOutput};
use mortar_resolve::shared::Dynamic;
use mortar_resolve::verdict::Unresolved;
use mortar_resolve::{Resolved, resolve};
use support::Model;

const A64: Machine = Machine::AArch64;

fn shared_opts(form: Symbolic) -> Options {
    Options {
        entry: Entry::None,
        output: Output { pie: false, dynamic: true },
        unresolved: Unresolved::Ignore,
        shared_output: Some(SharedOutput {
            symbolic: SymbolicLine { form, has_dynamic_list: false },
            defs: false,
        }),
        ..Options::default()
    }
}

fn link_with(m: &Model, o: &Options) -> Resolved {
    check_schedules(|pool| resolve(m, o, pool))
}

fn dynamic(r: &Resolved) -> &Dynamic {
    r.dynamic.as_ref().expect("the shared stage runs for a Shared output")
}

/// The library: a function, a datum, a weak function, a common, a hidden
/// and a protected function, an undefined and a weak undefined reference,
/// `_end` and `__start_mysec`.
fn library() -> Model {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.def(a, "f");
    m.data(a, "d", 8);
    m.weak(a, "w");
    m.common(a, "c", 8, 8);
    let h = m.def(a, "h");
    m.set_visibility(a, h, STV_HIDDEN);
    let p = m.def(a, "p");
    m.set_visibility(a, p, STV_PROTECTED);
    m.undef(a, "u");
    m.weak_undef(a, "wu");
    m.undef(a, "_end");
    m.weak_undef(a, "__start_mysec");
    m
}

fn column(m: &Model, col: &[bool]) -> Vec<&'static str> {
    ["f", "d", "w", "c", "h", "p", "u", "wu", "_end", "__start_mysec"]
        .into_iter()
        .filter(|n| col[m.id(n).0 as usize])
        .collect()
}

// spec: dyn.md D-110 (the export set: no shared input needed, -E changes
// nothing; hidden keys, undefined keys and `_end` without a shared
// reference are not exported; `__start_` always)
#[test]
fn d110_export_set_of_a_shared_object() {
    let m = library();
    for export_dynamic in [false, true] {
        let mut o = shared_opts(Symbolic::None);
        o.shared.export_dynamic = export_dynamic;
        let r = link_with(&m, &o);
        assert!(r.problems.is_empty(), "{:?}", r.problems);
        let d = dynamic(&r);
        assert_eq!(column(&m, &d.exported), ["f", "d", "w", "c", "p", "__start_mysec"]);
    }
}

// spec: dyn.md D-112 (the table), model.md §3.3 check (8)
#[test]
fn d112_preemptible_per_form() {
    let m = library();
    for (form, want) in [
        (Symbolic::None, &["f", "d", "w", "c"][..]),
        (Symbolic::Functions, &["d", "c"][..]),
        (Symbolic::All, &[][..]),
    ] {
        let r = link_with(&m, &shared_opts(form));
        let d = dynamic(&r);
        assert_eq!(column(&m, &d.preemptible), want, "{form:?}");
        for (k, &on) in d.preemptible.iter().enumerate() {
            assert!(!on || d.exported[k], "check (8): key {k}");
        }
    }
}

// spec: model.md §3.3 R-M10 (empty outside `Shared`): an executable's
// shared stage has no `preemptible` column, and a static link no stage
#[test]
fn r_m10_no_preemptible_outside_shared() {
    let mut m = Model::new(A64);
    let a = m.obj("a.o");
    m.def(a, "f");
    let x = m.shlib("libx.so", false);
    m.def(x, "g");
    let o = Options { entry: Entry::None, ..Options::default() };
    let r = link_with(&m, &o);
    assert!(dynamic(&r).preemptible.is_empty());
    let m = library();
    assert!(link_with(&m, &o).dynamic.is_none());
}

// spec: dyn.md D-118 (undefined references are allowed in a shared
// object unless `-z defs`), D-112 (the line's last form)
#[test]
fn d118_shared_output_of_a_plan() {
    let mut plan = Plan::default();
    assert_eq!(SharedOutput::of(&plan), None);
    assert_eq!(SharedOutput::unresolved(None, Unresolved::Error), Unresolved::Error);
    plan.kind = OutputKind::Shared;
    let out = SharedOutput::of(&plan).expect("a Shared plan");
    assert!(!out.defs);
    assert_eq!(out.symbolic.form, Symbolic::None);
    assert_eq!(SharedOutput::unresolved(Some(out), Unresolved::Error), Unresolved::Ignore);
    assert_eq!(SharedOutput::unresolved(Some(out), Unresolved::Warn), Unresolved::Ignore);
    plan.z.defs = Some(true);
    let out = SharedOutput::of(&plan).expect("a Shared plan");
    assert!(out.defs);
    assert_eq!(SharedOutput::unresolved(Some(out), Unresolved::Error), Unresolved::Error);
    assert_eq!(SharedOutput::unresolved(Some(out), Unresolved::Warn), Unresolved::Warn);
}

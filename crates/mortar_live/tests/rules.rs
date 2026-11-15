//! `spec/image.md` §3: liveness in M1 (I-1 to I-5), one test per rule,
//! on hand-built tables, and I-1 on a real link's resolution.

use std::ffi::OsString;
use std::process::Command;
use std::time::Duration;

use mortar_core::assignment::Assignment;
use mortar_core::column::Column;
use mortar_core::inputs::{Inputs, LinkInputs, LinkTarget, StoreRef};
use mortar_core::par::{Pool, check_schedules};
use mortar_core::resolution::{DropReason, Resolution, SectionKind, SectionRow, SectionState};
use mortar_core::split::{EhPiece, Frag, Split, SplitRow};
use mortar_core::{FragId, Liveness, OptId, SectionId, Span};
use mortar_elf::{Machine, SectionIndex};
use mortar_live::{LiveInputs, LiveOptions, eh_piece_live, liveness};

fn no_inputs() -> Inputs<'static> {
    Inputs {
        target: LinkTarget::of(Machine::AArch64),
        files: Column::new(),
        sym_map: Box::new([]),
        symbols: Column::new(),
        comdats: Column::new(),
        archives: Column::new(),
        link: LinkInputs::of_files(0),
        store: StoreRef::none(),
    }
}

fn row(kind: SectionKind, state: SectionState) -> SectionRow {
    SectionRow { kind, state, reloc: SectionIndex(0), comdat: OptId::NONE }
}

const KEPT: SectionState = SectionState::Kept;

/// Sections: 0 regular kept, 1 merge kept, 2 merge dropped (comdat),
/// 3 eh kept, 4 regular dropped (excluded), 5 merge kept.
fn tables() -> (Resolution, Split) {
    let mut r = Resolution::empty(0, 0, 0);
    r.sections = Column::from_vec(vec![
        row(SectionKind::Regular, KEPT),
        row(SectionKind::Merge, KEPT),
        row(SectionKind::Merge, SectionState::Dropped(DropReason::Comdat)),
        row(SectionKind::EhFrame, KEPT),
        row(SectionKind::Regular, SectionState::Dropped(DropReason::Excluded)),
        row(SectionKind::Merge, KEPT),
    ])
    .unwrap();
    let frag = |section| Frag { section: SectionId(section), offset: 0, size: 4, align: 0 };
    let fde = |cie, code: u32| EhPiece { cie, code: OptId::some(SectionId(code)) };
    let split = Split {
        merge_of: Box::new([
            SplitRow { section: SectionId(1), pieces: Span { start: 0, len: 2 } },
            SplitRow { section: SectionId(2), pieces: Span { start: 2, len: 2 } },
            SplitRow { section: SectionId(5), pieces: Span { start: 4, len: 1 } },
        ]),
        piece_off: Box::new([0, 4, 0, 4, 0]),
        // Fragments: 0 in 1 and 2; 1 in 1; 2 only in dropped 2; 3 in 5.
        piece_frag: Box::new([FragId(0), FragId(1), FragId(0), FragId(2), FragId(3)]),
        frags: Column::from_vec(vec![frag(1), frag(1), frag(2), frag(5)]).unwrap(),
        eh_of: Box::new([SplitRow { section: SectionId(3), pieces: Span { start: 0, len: 5 } }]),
        eh_off: Box::new([0, 16, 40, 64, 88]),
        // CIE 0 (FDEs to live 0 and dropped 4), CIE 2 (FDE to dropped 4
        // only), FDE without code.
        eh_piece: Box::new([
            EhPiece::CIE,
            fde(0, 0),
            EhPiece::CIE,
            fde(2, 4),
            EhPiece { cie: 0, code: OptId::NONE },
        ]),
    };
    (r, split)
}

type LiveResult = Result<Liveness, (mortar_diag::Key, mortar_diag::Diagnostic)>;

/// The columns of a table, to compare across schedules.
fn cols(l: &Liveness) -> (Vec<bool>, Vec<bool>) {
    (l.live.as_slice().to_vec(), l.frag_live.as_slice().to_vec())
}

#[allow(clippy::result_large_err, reason = "liveness's own result")]
fn run(r: &Resolution, split: &Split, gc: bool, pool: &Pool) -> LiveResult {
    let inputs = no_inputs();
    let a = Assignment::empty(r.sections.len());
    let t = LiveInputs { inputs: &inputs, resolution: r, assignment: &a, split, roots: &[] };
    liveness(&t, LiveOptions { gc_sections: gc, keep_exported: false }, pool)
}

#[test]
fn i1_live_exactly_when_kept() {
    let (r, split) = tables();
    let (live, _) = check_schedules(|pool| cols(&run(&r, &split, false, pool).unwrap()));
    assert_eq!(live, [true, true, false, true, false, true]);
    run(&r, &split, false, &Pool::new(1)).unwrap().check(&r, &split).unwrap();
}

#[test]
fn i2_every_fragment_of_a_live_merge_section() {
    let (r, split) = tables();
    let (_, frags) = check_schedules(|pool| cols(&run(&r, &split, false, pool).unwrap()));
    // Fragment 2 occurs only in the dropped section 2.
    assert_eq!(frags, [true, true, false, true]);
}

#[test]
fn i2_all_merge_sections_live_makes_all_fragments_live() {
    let (mut r, split) = tables();
    r.sections = Column::from_vec(
        r.sections.as_slice().iter().map(|s| SectionRow { state: KEPT, ..*s }).collect(),
    )
    .unwrap();
    let l = run(&r, &split, false, &Pool::new(2)).unwrap();
    assert_eq!(l.frag_live.as_slice(), [true; 4]);
}

#[test]
fn i3_fde_live_with_its_code_and_cie_with_a_live_fde() {
    let (r, split) = tables();
    let l = run(&r, &split, false, &Pool::new(1)).unwrap();
    let s = SectionId(3);
    let got: Vec<Option<bool>> = (0..6).map(|p| eh_piece_live(&l, &split, s, p)).collect();
    assert_eq!(got, [Some(true), Some(true), Some(false), Some(false), Some(false), None]);
    // Not an eh section.
    assert_eq!(eh_piece_live(&l, &split, SectionId(0), 0), None);
}

#[test]
fn i6_a_kept_section_without_a_place_is_a_root() {
    // `Assignment::empty` places nothing: every kept section is a root of
    // the mark (I-6, as without `--gc-sections`), and the result is the
    // no-gc one whatever the schedule.
    let (r, split) = tables();
    let with = check_schedules(|pool| cols(&run(&r, &split, true, pool).unwrap()));
    let without = cols(&run(&r, &split, false, &Pool::new(1)).unwrap());
    assert_eq!(with.0, without.0);
    run(&r, &split, true, &Pool::new(1)).unwrap().check(&r, &split).unwrap();
}

#[test]
fn i4_liveness_of_a_real_link_is_its_kept_sections() {
    let Some(gas) = mortar_testutil::tool("as") else {
        assert!(!mortar_testutil::tools_required(), "as is required");
        return;
    };
    let dir = mortar_testutil::Scratch::new("live-real");
    let src = "\t.section .text.a,\"axG\",%progbits,g,comdat\n\t.globl a\na:\tret\n\
               \t.section .rodata.str1.1,\"aMS\",%progbits,1\n\t.asciz \"x\"\n\
               \t.section .x,\"ae\",%progbits\n\t.byte 1\n\
               \t.text\n\t.globl _start\n_start:\tbl a\n";
    let mut objs = Vec::new();
    for n in ["a.o", "b.o"] {
        let s = dir.path().join(format!("{n}.s"));
        std::fs::write(&s, src.replace("_start", if n == "a.o" { "_start" } else { "b" })).unwrap();
        let o = dir.path().join(n);
        let mut c = Command::new(&gas);
        c.arg("-o").arg(&o).arg(&s);
        let out = mortar_testutil::run_with(&mut c, b"", Duration::from_mins(1));
        assert!(out.success(), "{}", out.stderr_text());
        objs.push(o);
    }
    let mut args: Vec<OsString> = vec!["-static".into(), "-e".into(), "_start".into()];
    args.extend(["-o".into(), "out".into()]);
    args.extend(objs.iter().map(|o| o.clone().into_os_string()));
    let plan = mortar_cli::parse(&args).plan;
    let store = mortar_input::store::Store::new(mortar_input::store::StorePolicy::Resident);
    let pool = Pool::new(2);
    let diag = mortar_diag::Collector::new(mortar_diag::Policy::default());
    let env = mortar_input::load::LoadEnv { cwd: dir.path().to_path_buf(), pool };
    let inputs = mortar_input::load::load(&plan, &env, &store, &diag).expect("load");
    let outcome = mortar_resolve::resolve_inputs(&inputs, &plan, &pool);
    let r = &outcome.resolution;
    let split = Split::default();
    let a = Assignment::empty(r.sections.len());
    let t =
        LiveInputs { inputs: &inputs, resolution: r, assignment: &a, split: &split, roots: &[] };
    let (live, _) =
        check_schedules(|pool| cols(&liveness(&t, LiveOptions::default(), pool).unwrap()));
    let kept: Vec<bool> = r.sections.as_slice().iter().map(SectionRow::is_kept).collect();
    assert_eq!(live, kept);
    let l = liveness(&t, LiveOptions::default(), &pool).unwrap();
    // The second copy of the group and both SHF_EXCLUDE sections are dead.
    assert!(kept.iter().filter(|k| !**k).count() >= 3);
    l.check(r, &split).unwrap();
}

//! U-9's tests on rows alone (`spec/incr.md` IN-75a; WP-221): the
//! fall-backs and the reference-site cases a real link cannot reach
//! cheaply. The named tests against a full link's standard error are in
//! `crates/mortar/tests/incr_diags.rs`.

use mortar_core::incr::{IncrReason, Unimplemented};
use mortar_core::{FileId, SectionId, SymbolId};
use mortar_diag::{Collector, Diagnostic, Key, Kind, Place, Policy, Relation, Style};
use mortar_incr::format::{SectionRow, Str};
use mortar_incr::update::diags::{DiagRef, DiagRow, REFS_CAP, Subject, diags, dormant, emit, row};
use mortar_incr::update::odr::OdrUpdate;
use mortar_incr::update::resolve::ResolveDelta;
use mortar_incr::update::rows::{
    FileIndexRow, FileRow, FileTable, KeyRefRow, RowRange, file_flags,
};
use mortar_incr::update::testing::MemoryState;
use mortar_incr::update::view::{Change, ChangedFile, ChangedFiles, OdrRows};

/// Four files (0 the linker's, 1..=3 objects `fN.o`), ranked by id, each
/// with the sections 0 and 1 (`.text`).
fn state() -> MemoryState {
    let mut m = MemoryState::default();
    let mut put = |b: &[u8]| {
        let off = m.strings.len() as u32;
        m.strings.extend_from_slice(b);
        Str { off, len: b.len() as u32 }
    };
    let text = put(b".text");
    let paths: Vec<Str> = (0..4).map(|n| put(format!("f{n}.o").as_bytes())).collect();
    for (n, path) in paths.into_iter().enumerate() {
        let n32 = n as u32;
        m.files.push(FileRow {
            rank: (n as u64) << 20,
            flags: file_flags::POSITIONAL,
            path,
            ..FileRow::default()
        });
        let mut fi = FileIndexRow::default();
        fi.ranges[FileTable::Sections as usize] =
            RowRange { first: 2 * n32, count: 2, capacity: 2 };
        m.file_index.push(fi);
        m.sections.push(SectionRow { file: n32, ..SectionRow::default() });
        m.sections.push(SectionRow { file: n32, name: text, ordinal: 1, ..SectionRow::default() });
    }
    m
}

fn changed(files: &[(u32, Change)], m: &MemoryState) -> ChangedFiles {
    let mut c = ChangedFiles::default();
    for &(f, change) in files {
        c.files.push(ChangedFile {
            file: FileId(f),
            change,
            row: m.files[f as usize].clone(),
            first_section: SectionId(u32::MAX),
            sections: Vec::new(),
            keys: Vec::new(),
            first_global: 0,
            mentions: Vec::new(),
            values: Vec::new(),
            index: Vec::new(),
            edges: Vec::new(),
            meta_units: 0,
            debug: false,
            versioned: 0,
            odr: OdrRows::default(),
        });
    }
    c
}

fn fall_back() -> Result<Vec<DiagRow>, IncrReason> {
    Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Diags)))
}

fn run(
    m: &MemoryState,
    change: &[(u32, Change)],
    affected: &[u32],
    fresh: Vec<DiagRow>,
) -> Result<Vec<DiagRow>, IncrReason> {
    let view = m.view();
    let resolved = ResolveDelta {
        affected: affected.iter().copied().map(SymbolId).collect(),
        ..ResolveDelta::default()
    };
    diags(&view, &changed(change, m), &resolved, &OdrUpdate::default(), fresh)
}

/// `K`'s `.gnu.warning` row: `count` references in `file`, the first
/// `stored` of them kept, its holder file 3.
fn gnu(count: u64, stored: usize, file: u32) -> DiagRow {
    let place =
        |n: u64| Place::section(mortar_diag::FileRef::new(format!("f{file}.o")), ".text", n * 4);
    let mut d = Diagnostic::warning(Kind::GnuWarning { text: "K is old".to_owned() });
    for n in 0..count.min(3) {
        d = d.with(Relation::ReferencedBy, place(n));
    }
    if count > 3 {
        d = d.note(format!("referenced {} more times", count - 3));
    }
    let refs = (0..stored as u64)
        .map(|n| DiagRef { file: FileId(file), seq: (1 << 32) + n, offset: n * 4 })
        .collect();
    let key = Key { phase: 70, unit: file, seq: 1 << 32 };
    row(key, &d, vec![Subject::Key(SymbolId(0)), Subject::File(FileId(3))], refs).expect("a row")
}

// spec: incr.md IN-75a (WP-221 ruling 3, A+: a row holding every
// reference never falls back; it is kept whole when nothing of it is
// touched)
#[test]
fn exact_row_is_kept_or_recomputed() {
    let mut m = state();
    m.diags = vec![gnu(5, 5, 1)];
    assert_eq!(run(&m, &[(2, Change::Changed)], &[], Vec::new()), Ok(m.diags.clone()));
    // The key affected (file 2 mentions it), no reference left in file 2:
    // the stored references stand, the row is rebuilt equal.
    let out = run(&m, &[(2, Change::Changed)], &[0], Vec::new()).expect("exact");
    assert_eq!(out, m.diags);
}

// spec: incr.md IN-75a (WP-221 ruling 3: a capped row whose references
// may move falls back; untouched, it is kept)
#[test]
fn capped_row_falls_back_when_its_key_is_affected() {
    let mut m = state();
    m.diags = vec![gnu(REFS_CAP as u64 + 6, REFS_CAP, 1)];
    assert_eq!(m.diags[0].refs.len(), REFS_CAP);
    assert_eq!(run(&m, &[(2, Change::Changed)], &[], Vec::new()), Ok(m.diags.clone()));
    assert_eq!(run(&m, &[(2, Change::Changed)], &[0], Vec::new()), fall_back());
    assert_eq!(run(&m, &[(1, Change::Changed)], &[0], Vec::new()), fall_back());
}

// spec: incr.md IN-75a (WP-221 ruling 5: a NEW holder of .gnu.warning.K
// while unchanged files refer to K: their places are unknown)
#[test]
fn new_holder_with_unchanged_referrers_falls_back() {
    let mut m = state();
    m.key_refs = vec![KeyRefRow { key: 0, file: 1, ..KeyRefRow::default() }];
    let head = Diagnostic::warning(Kind::GnuWarning { text: "K is old".to_owned() });
    let fresh = dormant(&head, SymbolId(0), FileId(3)).expect("a row");
    assert_eq!(run(&m, &[(3, Change::Changed)], &[], vec![fresh.clone()]), fall_back());
    // No unchanged referrer: the holder's dormant row is stored, never
    // printed.
    m.key_refs.clear();
    let out = run(&m, &[(3, Change::Changed)], &[], vec![fresh.clone()]).expect("exact");
    assert_eq!(out, [fresh]);
    let view = m.view();
    let collector = Collector::new(Policy::default());
    emit(&view, &changed(&[], &m), &ResolveDelta::default(), &out, &collector).expect("emit");
    assert_eq!(collector.finish().render(&Style::plain("mortar")), "");
}

// spec: incr.md IN-75a (WP-221 ruling 5: the holder gone or changed
// without the section: the row is dropped)
#[test]
fn holder_gone_drops_the_row() {
    let mut m = state();
    m.diags = vec![gnu(2, 2, 1)];
    assert_eq!(run(&m, &[(3, Change::Gone)], &[0], Vec::new()), Ok(Vec::new()));
    assert_eq!(run(&m, &[(3, Change::Changed)], &[0], Vec::new()), Ok(Vec::new()));
}

// spec: incr.md IN-75a (WP-221 §2.2, ruling 6: a warning the update
// cannot raise again falls back)
#[test]
fn link_scoped_codes_fall_back() {
    let mut m = state();
    let d = Diagnostic::warning(Kind::ArchiveBackref { name: "x".to_owned() });
    m.diags = vec![
        row(Key { phase: 30, unit: 0, seq: 0 }, &d, vec![Subject::Link], Vec::new())
            .expect("a row"),
    ];
    assert_eq!(run(&m, &[(2, Change::Changed)], &[], Vec::new()), fall_back());
}

// spec: incr.md IN-75a (WP-221 F-1, ruling 7: executable-stack names the
// first file in load order; its file changed and no earlier fresh note:
// fall back)
#[test]
fn executable_stack_recomputed() {
    let note = |f: u32| {
        let d = Diagnostic::warning(Kind::ExecutableStack { file: format!("f{f}.o") });
        row(
            Key { phase: 90, unit: 0, seq: u64::MAX - 1000 },
            &d,
            vec![Subject::File(FileId(f)), Subject::Link],
            Vec::new(),
        )
        .expect("a row")
    };
    let mut m = state();
    m.diags = vec![note(2)];
    assert_eq!(run(&m, &[(3, Change::Changed)], &[], Vec::new()), Ok(vec![note(2)]));
    assert_eq!(run(&m, &[(1, Change::Changed)], &[], vec![note(1)]), Ok(vec![note(1)]));
    assert_eq!(run(&m, &[(2, Change::Changed)], &[], vec![note(2)]), Ok(vec![note(2)]));
    assert_eq!(run(&m, &[(2, Change::Changed)], &[], Vec::new()), fall_back());
    assert_eq!(run(&m, &[(2, Change::Changed)], &[], vec![note(3)]), fall_back());
}

// spec: incr.md IN-75a, diag.md's subjects column (WP-221: filled; the
// column and the code agree both ways in mortar_diag's spec_tables)
#[test]
fn subjects_of_a_reference_site_warning() {
    use mortar_diag::subject::{SubjectKind, subjects};
    assert_eq!(subjects("undefined-symbol"), [SubjectKind::File, SubjectKind::Key]);
    assert_eq!(subjects("gnu-warning"), [SubjectKind::File, SubjectKind::Key]);
    assert!(subjects("duplicate-symbol").is_empty());
}

//! WP-255d2: `front_of` with `Ids::Update` (a changed or newly extracted
//! file in the update's id space), against the whole front of the NEW
//! inputs for that file, matched by identity (sections by index, keys by
//! name): `capture::file_rows` of the new link's front.
//!
//! - `update_front_of_an_extracted_member`: a member of `lib2.a` the old
//!   link did not extract, extracted by the new link (`-u`): the OLD
//!   state's `FILES` row (its bytes did not change), `Ids::Update`.
//! - `update_front_of_edited_files`: `m1.o` edited (a function renamed, one
//!   grown, a section added, a COMDAT group added: C++ `inline`) and
//!   re-archived (the other members move), opened through the OLD state
//!   with `Ids::Update`'s NEW `FILES` row (ruling D2-1): its `base`, `len`
//!   and `hash` are the full link's capture of the new archive (the one
//!   producer of a `FILES` row), its `Str`s the old state's. The member
//!   with versioned definitions (`foo@V1`, `foo@@V2`) included. The stored
//!   row's hash for the edited bytes is `input-changed`.
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
#[path = "support/update.rs"]
mod update;

use mortar::front::Front;
use mortar::update::file_front::{FrontCache, Ids, front_of};
use mortar_core::id::{FileId, SectionId, SymbolId};
use mortar_core::identity::Anchor;
use mortar_core::incr::{IncrReason, Unimplemented};
use mortar_incr::capture::{FileRows, Keys, Link, file_rows};
use mortar_incr::format::Strings;
use mortar_incr::update::rows::FileRow;
use mortar_incr::update::view::StateView;
use support::with_output;
use update::{Line, first_free, lines, member, pool, same_file, state, whole};

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

/// `front_of(view, file, Ids::Update)` (the file's bytes named by `row`)
/// against the whole front `f`'s rows of the same file, by identity.
fn compare(
    line: &Line,
    f: &Front<'_>,
    view: &StateView<'_>,
    (file, row): (FileId, FileRow),
    first: u32,
    seen: &mut Seen,
) {
    let mut strings = Strings::default();
    let new_file = same_file(f, view, file);
    let want = rows_of(f, &mut strings, new_file);
    let fresh = support::store();
    let mut cache = FrontCache::new(&fresh, pool(), &line.dir);
    let ids = Ids::Update { first_section: SectionId(first), row };
    let what = |s: &str| format!("{}: file {}: {s}", line.label, file.0);
    let ff = front_of(&mut cache, view, file, ids)
        .unwrap_or_else(|e| panic!("{}", what(&format!("{e:?}"))));
    assert_eq!(ff.sections.len(), want.sections.len(), "{}", what("section count"));
    assert!(!want.sections.is_empty(), "{}", what("an extracted file has sections"));
    for (k, (s, w)) in ff.sections.iter().zip(&want.sections).enumerate() {
        let at = |x: &str| what(&format!("section {k}: {x}"));
        assert_eq!(s.id, SectionId(first + k as u32), "{}", at("the update's id"));
        let got = (s.sh_type, s.flags, s.size, s.align, s.offset);
        assert_eq!(got, (w.sh_type, w.sh_flags, w.size, w.align, w.offset), "{}", at("header"));
        assert_eq!(strings.add(s.name), w.name, "{}", at("name"));
        assert_eq!(
            anchor_row(&mut strings, s.anchor),
            (w.anchor_kind, w.anchor),
            "{}",
            at("anchor")
        );
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
                assert_eq!(
                    view.string(r.name).expect("name"),
                    key.name,
                    "{}",
                    at("the row's name")
                );
                let v = view.string(r.version).expect("version");
                assert_eq!(&v[..], key.version.unwrap_or_default(), "{}", at("the row's version"));
            }
            None => seen.new_keys += 1,
        }
        seen.keys += 1;
    }
}

/// The record's example (ruling "After 255c3" item 2): the first key of
/// the old link's front whose `SymbolId` is not its `SYMBOLS` row (found
/// by name and version), and how many globals are at another row.
fn renumbering(line: &Line, args: &[String], view: &StateView<'_>) {
    let store = support::store();
    let f = whole(line, args, &store);
    let (mut first, mut other, mut rowless) = (None, 0, 0);
    for i in 0..f.inputs.symbols.len() {
        let sym = &f.inputs.symbols[SymbolId(i)];
        let row = view.find_key(sym.name, sym.version.unwrap_or_default()).expect("SYMBOLS");
        match row {
            Some(r) if r.0 != i => {
                other += 1;
                first = first.or(Some((i, r.0, sym.name)));
            }
            Some(_) => {}
            None => {
                rowless += 1;
                eprintln!("{}: no SYMBOLS row: {}", line.label, String::from_utf8_lossy(sym.name));
            }
        }
    }
    if let Some((i, r, name)) = first {
        eprintln!(
            "{}: the first renumbered key: {}: SymbolId {i}, SYMBOLS row {r}; {other} of {} \
             keys at another row, {rowless} without a row",
            line.label,
            String::from_utf8_lossy(name),
            f.inputs.symbols.len()
        );
    }
}

#[test]
fn update_front_of_an_extracted_member() {
    for line in lines(&[]) {
        line.archive(&["m1.o", "m2.o", "m4.o"]);
        let old_args = line.args(&["wp255d_m2", "wp255d_m1"]);
        let old = state(&line, &old_args, "old");
        let m4 = member(&old, "m4.o");
        renumbering(&line, &old_args, &old);
        let new_args = line.args(&["wp255d_m2", "wp255d_m1", "wp255d_m4"]);
        let store = support::store();
        let f = whole(&line, &new_args, &store);
        let mut seen = Seen::default();
        let row = old.file(m4).expect("FILES");
        compare(&line, &f, &old, (m4, row.clone()), first_free(&old), &mut seen);
        eprintln!("{}: extracted m4.o: {seen:?}", line.label);
        assert!(seen.versioned >= 2 && seen.keys > 0, "{}: {seen:?}", line.label);
        // The keys a front has that the old state has no row for: the old
        // state answers `None` (a new key: U-3 allocates it and resolves
        // it); the front gives no value of its own. The full link's
        // resolution of each, for the record.
        let fresh = support::store();
        let mut cache = FrontCache::new(&fresh, pool(), &line.dir);
        let ff = front_of(
            &mut cache,
            &old,
            m4,
            Ids::Update { first_section: SectionId(first_free(&old)), row },
        )
        .expect("m4");
        for name in [&b"_DYNAMIC"[..], b"_end", b"wp255d_nowhere"] {
            let key = ff.keys.iter().find(|k| k.name == name).expect("the key is mentioned");
            let full = (0..f.inputs.symbols.len())
                .map(SymbolId)
                .find(|&s| {
                    f.inputs.symbols[s].name == name && f.inputs.symbols[s].version.is_none()
                })
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
    for line in lines(&[]) {
        let args = line.args(&["wp255d_m2", "wp255d_m1", "wp255d_m4"]);
        line.archive(&["m1.o", "m2.o", "m4.o"]);
        let old = state(&line, &args, "old");
        // The edit: `m1.o` replaced by the C++ version, the archive rebuilt.
        std::fs::copy(line.dir.join("m1e.o"), line.dir.join("m1.o")).expect("the edit");
        line.archive(&["m1.o", "m2.o", "m4.o"]);
        let new = state(&line, &args, "new");
        let store = support::store();
        let f = whole(&line, &args, &store);
        let mut seen = Seen::default();
        let mut moved = 0;
        for name in ["m1.o", "m2.o", "m4.o"] {
            let file = member(&old, name);
            let stored = old.file(file).expect("FILES");
            let captured = new.file(member(&new, name)).expect("FILES");
            let row = FileRow {
                base: captured.base,
                len: captured.len,
                hash: captured.hash,
                ..stored.clone()
            };
            moved += u32::from(row != stored);
            if name == "m1.o" {
                // The stored row names the OLD bytes: never used for a
                // changed file.
                assert_ne!(stored.hash, row.hash, "{}: the edit changed m1.o", line.label);
                let fresh = support::store();
                let mut cache = FrontCache::new(&fresh, pool(), &line.dir);
                let ids = Ids::Update { first_section: SectionId(first_free(&old)), row: stored };
                let r = front_of(&mut cache, &old, file, ids).map(|x| x.file);
                assert!(
                    matches!(r, Err(IncrReason::InputChanged(_))),
                    "{}: the stored row: {r:?}",
                    line.label
                );
            }
            compare(&line, &f, &old, (file, row), first_free(&old), &mut seen);
        }
        eprintln!("{}: edited: {seen:?}, rows moved {moved}", line.label);
        assert!(seen.comdat > 0 && seen.versioned >= 2 && moved >= 2, "{}: {seen:?}", line.label);
    }
}

#[test]
fn update_link_order_outside_groups_falls_back() {
    for line in lines(&[]) {
        line.archive(&["m1.o", "m2.o", "m3.o"]);
        let view = state(&line, &line.args(&["wp255d_m2", "wp255d_m1", "wp255d_m3"]), "lo");
        let m3 = member(&view, "m3.o");
        let fresh = support::store();
        let mut cache = FrontCache::new(&fresh, pool(), &line.dir);
        assert!(front_of(&mut cache, &view, m3, Ids::Stored).is_ok(), "{}: stored", line.label);
        let fresh = support::store();
        let mut cache = FrontCache::new(&fresh, pool(), &line.dir);
        let row = view.file(m3).expect("FILES");
        let ids = Ids::Update { first_section: SectionId(first_free(&view)), row };
        let r = front_of(&mut cache, &view, m3, ids).map(|x| x.file);
        let want = IncrReason::UpdateNotImplemented(Some(Unimplemented::Resolve));
        assert_eq!(r, Err(want), "{}", line.label);
    }
}

/// Ruling D2-2 (WP-255d3 D3-3): a problem the split of an opened,
/// hash-checked file meets is `update-not-implemented (split)` (the input
/// did not change; the full link reports the problem), not
/// `input-changed`. `m6.o` (an unterminated string section, S-8) is a
/// member the old link did not extract: its stored `FILES` row hashes its
/// bytes; extracted, its split meets `Unterminated`. The facts are the
/// classification the full link would give the string section (`Merge`,
/// kept, placed); the full link of the line that extracts it fails.
#[test]
fn update_split_problem_falls_back_as_split() {
    use mortar::update::file_split::{SectionFact, split_of};
    use mortar_core::OutSecId;
    use mortar_core::resolution::SectionKind;
    use mortar_elf::SectionIndex;
    use mortar_elf::consts::SHF_MERGE;
    use mortar_merge::phase::ArchRelocs;
    for line in lines(&[]) {
        line.archive(&["m1.o", "m2.o", "m6.o"]);
        let old_args = line.args(&["wp255d_m2", "wp255d_m1"]);
        let old = state(&line, &old_args, "old");
        let m6 = member(&old, "m6.o");
        let row = old.file(m6).expect("FILES");
        let store = support::store();
        let f = whole(&line, &old_args, &store);
        let fresh = support::store();
        let mut cache = FrontCache::new(&fresh, pool(), &line.dir);
        let ids = Ids::Update { first_section: SectionId(first_free(&old)), row };
        let ff =
            front_of(&mut cache, &old, m6, ids).expect("m6 opens: its bytes are the stored ones");
        let merge: Vec<u32> = ff
            .sections
            .iter()
            .filter(|s| s.flags & SHF_MERGE != 0 && s.name == b".rodata.wp255d_str")
            .map(|s| s.index.0)
            .collect();
        assert_eq!(merge.len(), 1, "{}: the string section", line.label);
        let facts = |i: SectionIndex| {
            if merge.contains(&i.0) {
                SectionFact { kind: SectionKind::Merge, kept: true, out: Some(OutSecId(1)) }
            } else {
                SectionFact { kind: SectionKind::Null, kept: false, out: None }
            }
        };
        let r = split_of(ff, &old, &facts, &ArchRelocs(f.arch.get()), &f.pool, None).map(|x| x.file);
        let want = IncrReason::UpdateNotImplemented(Some(Unimplemented::Split));
        assert_eq!(r, Err(want.clone()), "{}", line.label);
        // The full link of the line that extracts it reports the problem.
        let a =
            with_output(&line.args(&["wp255d_m2", "wp255d_m1", "wp255d_m6"]), &line.dir.join("m6"));
        let inv = mortar::invoke_in("mortar", &a, &line.dir);
        assert!(
            !matches!(inv.outcome, mortar::Outcome::Linked { .. }),
            "{}: {:?}",
            line.label,
            inv.outcome
        );
        eprintln!("{}: m6.o: the split's {:?}; the full link {:?}", line.label, want, inv.outcome);
    }
}

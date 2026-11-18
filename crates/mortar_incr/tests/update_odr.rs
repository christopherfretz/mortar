//! U-8's tests (`spec/incr.md` IN-71d). WP-213 (U-1): the entry point's
//! "until it lands" value; U-8 replaces it.

use mortar_core::incr::{IncrReason, Unimplemented};
use mortar_core::plan::{Odr, OdrMode};
use mortar_core::{FileId, SectionId, SymbolId};
use mortar_elf::SymbolIndex;
use mortar_elf::consts::{STB_GLOBAL, STB_WEAK, STT_OBJECT};
use mortar_incr::format::{NONE, def_kind};
use mortar_incr::update::odr::{odr, stored_resolved};
use mortar_incr::update::resolve::ResolveDelta;
use mortar_incr::update::rows::{
    CommonRow, FileIndexRow, FileRow, KeyRefRow, MentionRow, ResolvedRow, SymbolRow,
};
use mortar_incr::update::testing::MemoryState;
use mortar_incr::update::view::{Change, ChangedFile, ChangedFiles, OdrRows};
use mortar_odr::foreign::{At, Resolved};

// spec: incr.md IN-71d (until U-8: units in a changed file are `meta`)
#[test]
fn odr_until_it_lands() {
    let m = MemoryState::default();
    let view = m.view();
    let resolved = ResolveDelta::default();
    let mut changed = ChangedFiles::default();
    let warn = Odr { mode: OdrMode::Warn, ..Odr::default() };
    assert_eq!(odr(&view, &changed, &resolved, &warn), Ok(Vec::new()));
    changed.files.push(ChangedFile {
        file: FileId(1),
        change: Change::Changed,
        row: FileRow::default(),
        first_section: SectionId(0),
        sections: Vec::new(),
        keys: Vec::new(),
        mentions: Vec::new(),
        values: Vec::new(),
        first_global: 0,
        index: Vec::new(),
        edges: Vec::new(),
        eh_records: Vec::new(),
        meta_units: 1,
        debug: false,
        versioned: 0,
        odr: OdrRows::default(),
    });
    assert_eq!(
        odr(&view, &changed, &resolved, &warn),
        Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Meta)))
    );
}

/// Files 0, 1, 2 in load order and file 3 APPENDED with a middle rank
/// (between 0 and 1: an appeared file, IN-18e); each file's non-local
/// entries start at `first_global` 5; key 0 is `c`, key 1 is `a`.
fn four_files() -> MemoryState {
    let mut m = MemoryState::default();
    let ranks = [0u64, 2 << 20, 4 << 20, 1 << 20];
    m.files = ranks.iter().map(|&rank| FileRow { rank, ..FileRow::default() }).collect();
    m.file_index =
        ranks.iter().map(|_| FileIndexRow { first_global: 5, ..FileIndexRow::default() }).collect();
    for name in [&b"c"[..], b"a"] {
        let name = m.intern(name);
        m.symbols.push(SymbolRow { name, ..SymbolRow::default() });
    }
    m.resolved = vec![ResolvedRow::default(); 2];
    m.name_rows();
    m
}

/// File `file`'s entries: an unrelated key first, then `key` of `kind`
/// and binding `bind` (so its index is `first_global` + 1 = 6).
fn mention(m: &mut MemoryState, file: u32, key: u32, kind: u8, bind: u8) {
    let other = MentionRow {
        symbol: 99,
        kind: def_kind::UNDEFINED,
        section: NONE,
        ..MentionRow::default()
    };
    let row = MentionRow { symbol: key, kind, bind, typ: STT_OBJECT, other: 0, section: NONE };
    m.mentions.push((FileId(file), other));
    m.mentions.push((FileId(file), row));
}

fn rank_of(m: &MemoryState) -> impl Fn(u32) -> Result<u64, IncrReason> + '_ {
    move |f| m.files.get(f as usize).map(|r| r.rank).ok_or(IncrReason::StateCorrupt(String::new()))
}

// spec: incr.md IN-71d (WP-220: W2's resolved common from COMMONS, by
// current rank, the appended middle-rank file winning)
#[test]
fn odr_resolved_common_by_current_rank() {
    let mut m = four_files();
    for file in [1, 3, 2] {
        m.commons.push(CommonRow { key: 0, file, size: 8, align: 8, tomb: false });
        mention(&mut m, file, 0, def_kind::COMMON, STB_GLOBAL);
    }
    m.resolved[0] = ResolvedRow {
        kind: def_kind::COMMON,
        typ: STT_OBJECT,
        section: NONE,
        ..ResolvedRow::default()
    };
    let rank = rank_of(&m);
    let view = m.view();
    let got = stored_resolved(&view, SymbolId(0), &rank).expect("rows");
    let at = At { file: FileId(3), index: SymbolIndex(6) };
    let want = Resolved { at, binding: STB_GLOBAL, sym_type: STT_OBJECT, common: true };
    assert_eq!(got, (true, Some(want)));
}

// spec: incr.md IN-71d (WP-220: W2's resolved absolute from KEY_REFS'
// definers: the strong one, else the least CURRENT rank)
#[test]
fn odr_resolved_absolute_of_several_definers() {
    let base = || {
        let mut m = four_files();
        m.resolved[1] = ResolvedRow {
            kind: def_kind::ABSOLUTE,
            typ: STT_OBJECT,
            section: NONE,
            ..ResolvedRow::default()
        };
        m
    };
    let refs = |m: &mut MemoryState, defs: &[(u32, u8)]| {
        for &(file, bind) in defs {
            m.key_refs.push(KeyRefRow {
                key: 1,
                file,
                def: def_kind::ABSOLUTE,
                bind,
                typ: STT_OBJECT,
                flags: 0,
                tomb: false,
            });
            mention(m, file, 1, def_kind::ABSOLUTE, bind);
        }
    };
    // Weak definers only: the least current rank is the appended file 3
    // (row number 3, rank between files 0 and 1), not file 1.
    let mut m = base();
    refs(&mut m, &[(1, STB_WEAK), (2, STB_WEAK), (3, STB_WEAK)]);
    let rank = rank_of(&m);
    let view = m.view();
    let (outside, r) = stored_resolved(&view, SymbolId(1), &rank).expect("rows");
    assert!(outside);
    let r = r.expect("a resolved definition");
    assert_eq!(
        (r.at, r.binding, r.common),
        (At { file: FileId(3), index: SymbolIndex(6) }, STB_WEAK, false)
    );
    // A strong definer wins over weak ones of lesser rank.
    let mut m = base();
    refs(&mut m, &[(1, STB_WEAK), (2, STB_GLOBAL), (3, STB_WEAK)]);
    let rank = rank_of(&m);
    let view = m.view();
    let (_, r) = stored_resolved(&view, SymbolId(1), &rank).expect("rows");
    assert_eq!(r.map(|r| (r.at.file, r.binding)), Some((FileId(2), STB_GLOBAL)));
}

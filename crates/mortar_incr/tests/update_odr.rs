//! U-8's tests (`spec/incr.md` IN-71d). WP-213 (U-1): the entry point's
//! "until it lands" value; U-8 replaces it.

use mortar_core::incr::{IncrReason, Unimplemented};
use mortar_core::plan::{Odr, OdrMode, OdrRuntime, OdrTable};
use mortar_core::{FileId, SectionId, SymbolId};
use mortar_elf::SymbolIndex;
use mortar_elf::consts::{STB_GLOBAL, STB_WEAK, STT_FUNC, STT_OBJECT};
use mortar_incr::format::{NONE, SectionRow, Str, def_kind};
use mortar_incr::update::diags::{DiagRow, Subject};
use mortar_incr::update::odr::{Now, OdrUpdate, odr, stored_resolved};
use mortar_incr::update::resolve::{NewKey, ResolveDelta};
use mortar_incr::update::rows::{
    CommonRow, CopyRow, FileIndexRow, FileRow, GroupRow, KeyRefRow, MentionRow, ResolvedRow,
    SymbolRow, sym_kind,
};
use mortar_incr::update::testing::MemoryState;
use mortar_incr::update::view::{
    Change, ChangedFile, ChangedFiles, KeyName, MentionValue, OdrRows,
};
use mortar_odr::foreign::{At, Resolved};
use mortar_odr::register::update_fallback;

// spec: incr.md IN-71d (U-8 landed: units in a changed file are read, not
// `meta`, without `--keep-mason-meta`; nothing changed gives no row)
#[test]
fn odr_units_without_keep_meta_are_read() {
    let m = MemoryState::default();
    let view = m.view();
    let resolved = ResolveDelta::default();
    let warn = Odr { mode: OdrMode::Warn, ..Odr::default() };
    assert_eq!(odr(&view, &ChangedFiles::default(), &resolved, &warn), Ok(OdrUpdate::default()));
    let (m, mut changed, d) = unreadable();
    changed.files[0].meta_units = 1;
    let view = m.view();
    assert!(matches!(odr(&view, &changed, &d, &warn), Err(IncrReason::StateCorrupt(_))));
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
                demand: 0,
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

// ---- step 2: W1/W2 in an update equal the full link's (WP-220) ----------
//
// A world is a link's inputs as rows: files in load order, each with its
// definitions (a COMDAT copy in a group, or a definition outside any
// group) and undefined references. `stored(w)` is the state a full link
// of `w` writes (the proof `tests/incr_odr.rs` in `mortar` shows the
// comparator's input built from such rows equals the full link's own);
// `delta(a, b)` is what U-2 and U-3 hand U-8 for the update from `a` to
// `b`. A file absent from a world keeps its `FILES` row with no content
// (a member not extracted; a gone file: its row's removal is U-9's id
// mapping, not U-8's); an appeared file is a new id past `a`'s count.

#[derive(Debug, Clone, PartialEq, Eq)]
struct Def {
    key: &'static str,
    /// The COMDAT signature, or `None`: a section outside any group.
    group: Option<&'static str>,
    size: u64,
    typ: u8,
    bind: u8,
    /// `false`: an undefined reference.
    defined: bool,
}

fn copy(key: &'static str, group: &'static str, size: u64) -> Def {
    Def { key, group: Some(group), size, typ: STT_OBJECT, bind: STB_WEAK, defined: true }
}

fn outside(key: &'static str, size: u64) -> Def {
    Def { key, group: None, size, typ: STT_OBJECT, bind: STB_GLOBAL, defined: true }
}

fn reference(key: &'static str) -> Def {
    Def { key, group: None, size: 0, typ: 0, bind: STB_GLOBAL, defined: false }
}

#[derive(Debug, Clone)]
struct InFile {
    path: &'static str,
    member: Option<&'static str>,
    rank: u64,
    /// `None`: absent (not extracted, gone, not yet appeared).
    defs: Option<Vec<Def>>,
}

#[derive(Debug, Clone)]
struct World {
    keys: Vec<&'static str>,
    files: Vec<InFile>,
}

const FIRST_GLOBAL: u32 = 3;

/// Each file's section ids: (its group or `None`, the `SECTIONS` id).
type Ids = Vec<Vec<(Option<&'static str>, u32)>>;

/// A file's sections: one per group in order of first appearance, then
/// one outside any group if it defines anything there.
fn sections_of(defs: &[Def]) -> Vec<Option<&'static str>> {
    let mut out: Vec<Option<&'static str>> = Vec::new();
    for d in defs.iter().filter(|d| d.defined) {
        if d.group.is_some() && !out.contains(&d.group) {
            out.push(d.group);
        }
    }
    if defs.iter().any(|d| d.defined && d.group.is_none()) {
        out.push(None);
    }
    out
}

impl World {
    fn key(&self, name: &str) -> u32 {
        let at = self.keys.iter().position(|k| *k == name).expect("a key of the world");
        u32::try_from(at).expect("a small world")
    }

    fn defs(&self, file: usize) -> &[Def] {
        self.files.get(file).and_then(|f| f.defs.as_deref()).unwrap_or_default()
    }

    /// The definition a key resolves to (resolve.md §3, as modelled): a
    /// strong definition outside any group, else any outside one, else
    /// the copy in the least-ranked file; by least rank each time.
    fn resolution(&self, key: &str) -> Option<(usize, Def)> {
        let mut best: Option<((u8, u64), usize, Def)> = None;
        for (f, file) in self.files.iter().enumerate() {
            for d in self.defs(f).iter().filter(|d| d.defined && d.key == key) {
                let class = match (d.group, d.bind) {
                    (None, b) if b != STB_WEAK => 0,
                    (None, _) => 1,
                    (Some(_), _) => 2,
                };
                let at = (class, file.rank);
                if best.as_ref().is_none_or(|b| at < b.0) {
                    best = Some((at, f, d.clone()));
                }
            }
        }
        best.map(|(_, f, d)| (f, d))
    }

    /// A group's keeper: the least-ranked file holding it.
    fn keeper(&self, group: &str) -> Option<usize> {
        (0..self.files.len())
            .filter(|&f| sections_of(self.defs(f)).contains(&Some(group)))
            .min_by_key(|&f| self.files[f].rank)
    }

    fn groups(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        for f in 0..self.files.len() {
            for g in sections_of(self.defs(f)).into_iter().flatten() {
                if !out.contains(&g) {
                    out.push(g);
                }
            }
        }
        out
    }
}

fn file_row(m: &mut MemoryState, f: &InFile) -> FileRow {
    let path = m.intern(f.path.as_bytes());
    let member_name = m.intern(f.member.unwrap_or_default().as_bytes());
    FileRow {
        rank: f.rank,
        path,
        member: u8::from(f.member.is_some()),
        member_name,
        ..FileRow::default()
    }
}

/// The state a full link of `w` writes; and each file's section ids.
fn stored(w: &World) -> (MemoryState, Ids) {
    let mut m = MemoryState::default();
    for name in &w.keys {
        let name = m.intern(name.as_bytes());
        m.symbols.push(SymbolRow { name, ..SymbolRow::default() });
    }
    let mut ids = Vec::new();
    for (f, file) in w.files.iter().enumerate() {
        let row = file_row(&mut m, file);
        m.files.push(row);
        let id = u32::try_from(f).expect("a small world");
        m.file_index.push(FileIndexRow { first_global: FIRST_GLOBAL, ..FileIndexRow::default() });
        let defs = w.defs(f);
        let mut here = Vec::new();
        for g in sections_of(defs) {
            let anchor = m.intern(g.unwrap_or_default().as_bytes());
            let at = u32::try_from(m.sections.len()).expect("a small world");
            m.sections.push(SectionRow {
                file: id,
                anchor_kind: u8::from(g.is_some()),
                anchor,
                ..SectionRow::default()
            });
            here.push((g, at));
            if let Some(g) = g {
                let signature = m.intern(g.as_bytes());
                m.groups.push(GroupRow { signature, file: id, tomb: false });
            }
        }
        for (k, d) in defs.iter().enumerate() {
            let key = w.key(d.key);
            let section = section_of(&here, d);
            let kind = if d.defined { def_kind::SECTION } else { def_kind::UNDEFINED };
            m.mentions.push((
                FileId(id),
                MentionRow { symbol: key, kind, bind: d.bind, typ: d.typ, other: 0, section },
            ));
            if d.group.is_some() {
                let index = FIRST_GLOBAL + u32::try_from(k).expect("a small world");
                m.copies.push(CopyRow {
                    key,
                    file: id,
                    index,
                    size: d.size,
                    typ: d.typ,
                    tomb: false,
                });
            }
        }
        ids.push(here);
    }
    m.resolved = w
        .keys
        .iter()
        .map(|k| match w.resolution(k) {
            Some((f, d)) => resolved_row(&d, section_of(&ids[f], &d)),
            None => ResolvedRow::default(),
        })
        .collect();
    m.name_rows();
    (m, ids)
}

fn section_of(here: &[(Option<&'static str>, u32)], d: &Def) -> u32 {
    if !d.defined {
        return NONE;
    }
    here.iter().find(|(g, _)| *g == d.group).map_or(NONE, |(_, at)| *at)
}

fn resolved_row(d: &Def, section: u32) -> ResolvedRow {
    ResolvedRow {
        kind: def_kind::SECTION,
        typ: d.typ,
        bind: d.bind,
        section,
        size: d.size,
        ..ResolvedRow::default()
    }
}

/// A stored string's bytes.
fn bytes(m: &MemoryState, s: Str) -> &[u8] {
    &m.strings[s.off as usize..(s.off + s.len) as usize]
}

/// `s` appended to the update's buffer, as a `Str` of its id space.
fn intern_new(changed: &mut ChangedFiles, base: usize, s: &[u8]) -> Str {
    let off = u32::try_from(base + changed.strings.len()).expect("small");
    changed.strings.extend_from_slice(s);
    Str { off, len: u32::try_from(s.len()).expect("small") }
}

/// What U-2 and U-3 hand U-8 for the update from `a` (stored as `sa`)
/// to `b`.
#[allow(clippy::too_many_lines, reason = "one model of U-2 and U-3, read top to bottom")]
fn delta(
    a: &World,
    b: &World,
    sa: &MemoryState,
    ids_a: &[Vec<(Option<&'static str>, u32)>],
) -> (ChangedFiles, ResolveDelta) {
    let base = sa.strings.len();
    let mut changed = ChangedFiles::default();
    let mut rd = ResolveDelta::default();
    let mut next_section = u32::try_from(sa.sections.len()).expect("small");
    let id_of = |name: &str| SymbolId(b.key(name));
    for k in &b.keys[a.keys.len()..] {
        rd.new_keys.push(NewKey {
            row: SymbolRow::default(),
            name: k.as_bytes().to_vec(),
            version: Vec::new(),
        });
    }
    // Section ids of the update: a changed file's new ones, else the
    // stored ones.
    let mut ids_now: Vec<Vec<(Option<&'static str>, u32)>> = ids_a.to_vec();
    ids_now.resize(b.files.len(), Vec::new());
    let mut touched = Vec::new();
    for (f, file) in b.files.iter().enumerate() {
        let old = a.files.get(f).and_then(|x| x.defs.as_ref());
        let new = file.defs.as_ref();
        let change = match (f < a.files.len(), old, new) {
            (false, _, Some(_)) => Change::Appeared,
            (true, None, Some(_)) => Change::Extracted,
            (true, Some(_), None) if file.member.is_some() => Change::NoLongerExtracted,
            (true, Some(_), None) => Change::Gone,
            (true, Some(was), Some(is)) if was != is => Change::Changed,
            _ => continue,
        };
        touched.push(f);
        let id = u32::try_from(f).expect("small");
        let defs = new.map(Vec::as_slice).unwrap_or_default();
        let first_section = SectionId(next_section);
        let mut sections = Vec::new();
        let mut here = Vec::new();
        for g in sections_of(defs) {
            let anchor = intern_new(&mut changed, base, g.unwrap_or_default().as_bytes());
            sections.push(SectionRow {
                file: id,
                anchor_kind: u8::from(g.is_some()),
                anchor,
                ..SectionRow::default()
            });
            here.push((g, next_section));
            next_section += 1;
        }
        let mut keys: Vec<&'static str> = Vec::new();
        for x in defs {
            if !keys.contains(&x.key) {
                keys.push(x.key);
            }
        }
        let pos = |name: &str| {
            u32::try_from(keys.iter().position(|k| *k == name).expect("a key")).expect("small")
        };
        let mut mentions = Vec::new();
        let mut copies = Vec::new();
        for (k, x) in defs.iter().enumerate() {
            let kind = if x.defined { def_kind::SECTION } else { def_kind::UNDEFINED };
            let section = section_of(&here, x);
            mentions.push(MentionRow {
                symbol: pos(x.key),
                kind,
                bind: x.bind,
                typ: x.typ,
                other: 0,
                section,
            });
            if x.group.is_some() {
                copies.push(CopyRow {
                    key: pos(x.key),
                    file: id,
                    index: FIRST_GLOBAL + u32::try_from(k).expect("small"),
                    size: x.size,
                    typ: x.typ,
                    tomb: false,
                });
            }
        }
        let row = if change == Change::Appeared {
            let path = intern_new(&mut changed, base, file.path.as_bytes());
            let member_name =
                intern_new(&mut changed, base, file.member.unwrap_or_default().as_bytes());
            FileRow {
                rank: file.rank,
                path,
                member: u8::from(file.member.is_some()),
                member_name,
                ..FileRow::default()
            }
        } else {
            FileRow { rank: file.rank, ..sa.files[f].clone() }
        };
        if change == Change::Appeared {
            changed.appeared.push(FileId(id));
        }
        let present = new.is_some();
        changed.files.push(ChangedFile {
            file: FileId(id),
            change,
            row,
            first_section,
            sections,
            keys: keys
                .iter()
                .map(|k| KeyName {
                    name: k.as_bytes().to_vec(),
                    version: Vec::new(),
                    kind: sym_kind::GLOBAL,
                    index: 0,
                })
                .collect(),
            first_global: if present { FIRST_GLOBAL } else { 0 },
            values: mentions.iter().map(|_| MentionValue::default()).collect(),
            mentions,
            index: Vec::new(),
            edges: Vec::new(),
            meta_units: 0,
            debug: false,
            versioned: 0,
            odr: OdrRows { copies },
        });
        rd.key_ids.push((FileId(id), keys.iter().map(|k| id_of(k)).collect()));
        // GROUPS: the stored file's groups out, the new ones in.
        for g in sections_of(a.defs(f)).into_iter().flatten() {
            let signature = sa
                .groups
                .iter()
                .find(|r| r.file == id && bytes(sa, r.signature) == g.as_bytes())
                .expect("a stored group row")
                .signature;
            rd.groups.delete.push(GroupRow { signature, file: id, tomb: false });
        }
        for g in sections_of(defs).into_iter().flatten() {
            let signature = intern_new(&mut changed, base, g.as_bytes());
            rd.groups.put.push(GroupRow { signature, file: id, tomb: false });
        }
        ids_now[f] = here;
    }
    // RESOLVED: every key of a changed file, and every key whose
    // resolution moved.
    for (k, name) in b.keys.iter().enumerate() {
        let mentioned =
            touched.iter().any(|&f| a.defs(f).iter().chain(b.defs(f)).any(|x| x.key == *name));
        let now = b.resolution(name);
        let before = if k < a.keys.len() { a.resolution(name) } else { None };
        if mentioned || now != before {
            let row = now.map_or_else(ResolvedRow::default, |(f, x)| {
                resolved_row(&x, section_of(&ids_now[f], &x))
            });
            rd.resolved.push((id_of(name), row));
        }
    }
    rd.affected = rd.resolved.iter().map(|r| r.0).collect();
    for g in a.groups().into_iter().chain(b.groups()) {
        if let (Some(old), Some(new)) = (a.keeper(g), b.keeper(g))
            && old != new
            && !rd.reelected.iter().any(|r| r.0 == g.as_bytes())
        {
            let file = |f: usize| FileId(u32::try_from(f).expect("small"));
            rd.reelected.push((g.as_bytes().to_vec(), file(old), file(new)));
        }
    }
    changed.files.sort_by_key(|f| f.file);
    (changed, rd)
}

/// The full link's W1/W2 rows of `m`: every key, in key order.
fn full(m: &MemoryState) -> Vec<DiagRow> {
    let view = m.view();
    let (changed, d) = (ChangedFiles::default(), ResolveDelta::default());
    let now = Now::new(&view, &changed, &d);
    let rank = |f: u32| now.rank(f);
    let mut out = Vec::new();
    for key in 0..u32::try_from(m.symbols.len()).expect("small") {
        let input = now.key_input(SymbolId(key), &rank).expect("rows");
        for (_, finding) in input.compare(true, true) {
            out.push(now.diag_row(finding).expect("rows"));
        }
    }
    out.sort_by_key(|r| r.key);
    out
}

fn key_of(row: &DiagRow) -> SymbolId {
    row.subjects
        .iter()
        .find_map(|s| if let Subject::Key(k) = s { Some(*k) } else { None })
        .expect("a key subject")
}

/// U-9's merge by the contract (WP-220 rulings-2 §1): every stored row
/// of a key in `got.keys` replaced by `got`'s rows, every other stays.
fn merge(stored: Vec<DiagRow>, got: OdrUpdate) -> Vec<DiagRow> {
    assert!(got.keys.windows(2).all(|w| w[0] < w[1]), "sorted, unique");
    assert!(got.rows.iter().all(|r| got.keys.contains(&key_of(r))), "rows of the keys");
    let mut out: Vec<DiagRow> =
        stored.into_iter().filter(|r| !got.keys.contains(&key_of(r))).collect();
    out.extend(got.rows);
    out.sort_by_key(|r| r.key);
    out
}

/// The update from `a` to `b` (its stored state's full rows and the
/// update's result).
fn update(a: &World, b: &World) -> (Vec<DiagRow>, OdrUpdate, ResolveDelta) {
    let (sa, ids_a) = stored(a);
    let (changed, d) = delta(a, b, &sa, &ids_a);
    let view = sa.view();
    let got = odr(&view, &changed, &d, &Odr::default()).expect("an update");
    (full(&sa), got, d)
}

/// One hop: the update's rows merged by its key set into the stored ones
/// equal the full link's of `b`; and give the full link's.
fn hop(a: &World, b: &World) -> Vec<DiagRow> {
    let (before, got, _) = update(a, b);
    let (sb, _) = stored(b);
    let want = full(&sb);
    assert_eq!(merge(before, got), want, "update from {a:#?}\nto {b:#?}");
    want
}

/// Three hops: `clean` -> `bad` (introduced) -> `clean` (removed) ->
/// `bad` (introduced again); the clean worlds have no finding, the bad
/// ones `findings`.
fn three_hops(clean: &World, bad: &World, findings: usize) {
    let (sc, _) = stored(clean);
    assert_eq!(full(&sc), Vec::new(), "the clean world has no finding");
    assert_eq!(hop(clean, bad).len(), findings, "introduced");
    assert_eq!(hop(bad, clean).len(), 0, "removed");
    assert_eq!(hop(clean, bad).len(), findings, "introduced again");
}

fn obj(path: &'static str, rank: u64, defs: Vec<Def>) -> InFile {
    InFile { path, member: None, rank, defs: Some(defs) }
}

fn with(w: &World, file: usize, defs: Option<Vec<Def>>) -> World {
    let mut w = w.clone();
    w.files[file].defs = defs;
    w
}

// spec: incr.md IN-71d (W1 in an update: a size mismatch of data copies in
// one group, introduced by a changed file, removed, introduced again)
#[test]
fn odr_update_size_mismatch_in_one_group() {
    let clean = World {
        keys: vec!["k", "other"],
        files: vec![
            obj("a.o", 0, vec![copy("k", "G", 8), outside("other", 4)]),
            obj("b.o", 1 << 20, vec![copy("k", "G", 8)]),
        ],
    };
    let bad = with(&clean, 1, Some(vec![copy("k", "G", 16)]));
    three_hops(&clean, &bad, 1);
}

// spec: incr.md IN-71d (W1/W2 in an update: a type mismatch)
#[test]
fn odr_update_type_mismatch() {
    let clean = World {
        keys: vec!["k"],
        files: vec![
            obj("a.o", 0, vec![copy("k", "G", 8)]),
            obj("b.o", 1 << 20, vec![copy("k", "G", 8)]),
        ],
    };
    let func = Def { typ: STT_FUNC, ..copy("k", "G", 8) };
    let bad = with(&clean, 0, Some(vec![func]));
    let (sb, _) = stored(&bad);
    let n = full(&sb).len();
    assert!(n > 0, "a type mismatch is a finding");
    three_hops(&clean, &bad, n);
}

// spec: incr.md IN-71d (a copy in a changed file, of a key new to the
// state: both copies come from changed files)
#[test]
fn odr_update_copy_in_changed_file_of_new_key() {
    let clean = World {
        keys: vec!["k"],
        files: vec![
            obj("a.o", 0, vec![copy("k", "G", 8)]),
            obj("b.o", 1 << 20, vec![copy("k", "G", 8)]),
        ],
    };
    let mut bad = with(&clean, 0, Some(vec![copy("k", "G", 8), copy("n", "N", 4)]));
    bad.files[1].defs = Some(vec![copy("k", "G", 8), copy("n", "N", 12)]);
    bad.keys.push("n");
    let mut clean2 = clean.clone();
    clean2.keys.push("n");
    // Introduced with a new key; removed and introduced again over the
    // key's (now stored) row.
    assert_eq!(hop(&clean, &bad).len(), 1, "introduced");
    assert_eq!(hop(&bad, &clean2).len(), 0, "removed");
    assert_eq!(hop(&clean2, &bad).len(), 1, "introduced again");
}

// spec: incr.md IN-71d (W2 in an update: copies in unchanged files whose
// key's resolved definition changed: a strong definition appears in a
// changed file)
#[test]
fn odr_update_unchanged_copies_resolved_definition_changed() {
    let clean = World {
        keys: vec!["v"],
        files: vec![
            obj("a.o", 0, vec![copy("v", "G", 8)]),
            obj("b.o", 1 << 20, vec![copy("v", "G", 8)]),
            obj("c.o", 2 << 20, vec![]),
        ],
    };
    let func = Def { typ: STT_FUNC, ..outside("v", 8) };
    let bad = with(&clean, 2, Some(vec![func]));
    let (sb, _) = stored(&bad);
    let n = full(&sb).len();
    assert!(n > 0, "a strong definition of another type is a finding");
    three_hops(&clean, &bad, n);
}

// spec: incr.md IN-71d (a copy in a member newly extracted, then no longer
// extracted, then extracted again)
#[test]
fn odr_update_member_extracted_and_no_longer() {
    let clean = World {
        keys: vec!["k"],
        files: vec![
            obj("a.o", 0, vec![copy("k", "G", 8)]),
            InFile { path: "lib.a", member: Some("m.o"), rank: 1 << 20, defs: None },
        ],
    };
    let bad = with(&clean, 1, Some(vec![copy("k", "G", 24)]));
    three_hops(&clean, &bad, 1);
}

// spec: incr.md IN-71d (a keeper change, WP-215's re-election, without a
// change of the copies: a lesser-ranked file takes the group with another
// key; the finding's "kept" copy moves)
#[test]
fn odr_update_keeper_change_without_copy_change() {
    let clean = World {
        keys: vec!["k", "j"],
        files: vec![
            obj("a.o", 0, vec![]),
            obj("b.o", 1 << 20, vec![copy("k", "G", 8)]),
            obj("c.o", 2 << 20, vec![copy("k", "G", 16)]),
        ],
    };
    let moved = with(&clean, 0, Some(vec![copy("j", "G", 4)]));
    // The contract: k is recomputed (no copy of k changed; U-3's
    // `affected` need not name it) and its stale row is replaced.
    let k = SymbolId(0);
    let (before, got, d) = update(&clean, &moved);
    assert!(got.keys.contains(&k));
    assert!(!d.affected.contains(&k), "only the keeper change makes k recomputed");
    let stale: Vec<DiagRow> = before.iter().filter(|r| key_of(r) == k).cloned().collect();
    assert_eq!(stale.len(), 1);
    let merged = merge(before, got);
    assert!(!merged.contains(&stale[0]), "the stale row is gone after the merge");
    let one = hop(&clean, &moved);
    assert_eq!(merged, one);
    assert_eq!(one.len(), 1, "k's finding stays, kept elsewhere");
    let (sc, _) = stored(&clean);
    assert_ne!(full(&sc), one, "the keeper's change shows in the text");
    assert_eq!(hop(&moved, &clean).len(), 1);
    assert_eq!(hop(&clean, &moved), one);
}

// spec: incr.md IN-71d (one file with one key in two groups)
#[test]
fn odr_update_one_key_in_two_groups_of_one_file() {
    let clean = World {
        keys: vec!["k"],
        files: vec![
            obj("a.o", 0, vec![copy("k", "G1", 8), copy("k", "G2", 8)]),
            obj("b.o", 1 << 20, vec![copy("k", "G1", 8), copy("k", "G2", 8)]),
        ],
    };
    let bad = with(&clean, 1, Some(vec![copy("k", "G1", 8), copy("k", "G2", 32)]));
    let (sb, _) = stored(&bad);
    let n = full(&sb).len();
    assert!(n > 0, "one group's copies of another size is a finding");
    three_hops(&clean, &bad, n);
}

// spec: incr.md IN-71d (two unchanged members' copies in conflict, made
// live by a changed file's reference: the finding appears as in the full
// link)
#[test]
fn odr_unchanged_pair_made_live_by_changed_file() {
    let member = |name, size| InFile {
        path: "lib.a",
        member: Some(name),
        rank: if name == "m.o" { 1 << 20 } else { 2 << 20 },
        defs: Some(vec![copy("k", "G", size), outside(name, 4)]),
    };
    let live = World {
        keys: vec!["use", "m.o", "n.o", "k"],
        files: vec![
            obj("main.o", 0, vec![reference("m.o"), reference("n.o")]),
            member("m.o", 8),
            member("n.o", 16),
        ],
    };
    let mut dead = with(&live, 0, Some(vec![reference("use")]));
    dead.files[1].defs = None;
    dead.files[2].defs = None;
    let (sd, _) = stored(&dead);
    assert_eq!(full(&sd), Vec::new());
    assert_eq!(hop(&dead, &live).len(), 1, "made live");
    assert_eq!(hop(&live, &dead).len(), 0, "dead again");
}

// spec: incr.md IN-71d (an appeared file of middle rank with a copy)
#[test]
fn odr_update_appeared_file_of_middle_rank() {
    let clean = World {
        keys: vec!["k"],
        files: vec![
            obj("a.o", 0, vec![copy("k", "G", 8)]),
            obj("b.o", 2 << 20, vec![copy("k", "G", 8)]),
        ],
    };
    let mut bad = clean.clone();
    bad.files.push(obj("new.o", 1 << 20, vec![copy("k", "G", 40)]));
    assert_eq!(hop(&clean, &bad).len(), 1);
}

// ---- the options-only fall-back (IN-71d) ----------------------------------

/// A changed file whose rows cannot be read: a key without its id. Any
/// read of rows fails; a decision from the options alone does not.
fn unreadable() -> (MemoryState, ChangedFiles, ResolveDelta) {
    let mut changed = ChangedFiles::default();
    changed.files.push(ChangedFile {
        file: FileId(7),
        change: Change::Changed,
        row: FileRow::default(),
        first_section: SectionId(0),
        sections: Vec::new(),
        keys: Vec::new(),
        mentions: vec![MentionRow::default()],
        values: Vec::new(),
        first_global: 0,
        index: Vec::new(),
        edges: Vec::new(),
        meta_units: 0,
        debug: false,
        versioned: 0,
        odr: OdrRows { copies: vec![CopyRow::default()] },
    });
    (MemoryState::default(), changed, ResolveDelta::default())
}

const META: Result<OdrUpdate, IncrReason> =
    Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Meta)));

fn under(o: Odr) -> Result<OdrUpdate, IncrReason> {
    let (m, changed, d) = unreadable();
    let view = m.view();
    odr(&view, &changed, &d, &o)
}

// spec: incr.md IN-71d (the rows are read: an unreadable state fails)
#[test]
fn odr_reads_rows_under_warn() {
    assert!(matches!(under(Odr::default()), Err(IncrReason::StateCorrupt(_))));
}

// spec: incr.md IN-71d (`--odr=off`: nothing runs, nothing is read)
#[test]
fn odr_off_reads_nothing() {
    assert_eq!(under(Odr { mode: OdrMode::Off, ..Odr::default() }), Ok(OdrUpdate::default()));
}

// spec: incr.md IN-71d (fall-back from the options alone: `--odr=strict`)
#[test]
fn odr_fallback_strict() {
    let o = Odr { mode: OdrMode::Strict, ..Odr::default() };
    assert_eq!(update_fallback(&o), Some("--odr=strict"));
    assert_eq!(under(o), META);
}

// spec: incr.md IN-71d (fall-back: `--odr-check-archives`)
#[test]
fn odr_fallback_check_archives() {
    let o = Odr { check_archives: true, ..Odr::default() };
    assert_eq!(update_fallback(&o), Some("--odr-check-archives"));
    assert_eq!(under(o), META);
}

// spec: incr.md IN-71d (fall-back: `--odr-table` other than off)
#[test]
fn odr_fallback_table() {
    for table in [OdrTable::Exported, OdrTable::Full] {
        let o = Odr { table, ..Odr::default() };
        assert_eq!(update_fallback(&o), Some("--odr-table"));
        assert_eq!(under(o), META);
    }
}

// spec: incr.md IN-71d (`odr_runtime_note_under_update`: `--odr-runtime`
// other than off falls back, by name)
#[test]
fn odr_runtime_note_under_update() {
    for runtime in [OdrRuntime::Report, OdrRuntime::Fatal] {
        let o = Odr { runtime, ..Odr::default() };
        assert_eq!(update_fallback(&o), Some("--odr-runtime"));
        assert_eq!(under(o), META);
    }
}

// spec: incr.md IN-71d (fall-back: `--odr-stats`)
#[test]
fn odr_fallback_stats() {
    let o = Odr { stats: true, ..Odr::default() };
    assert_eq!(update_fallback(&o), Some("--odr-stats"));
    assert_eq!(under(o), META);
}

// spec: incr.md IN-71d (fall-back: `--keep-mason-meta` with a changed file
// that carries units, or a gone one; without, the rows are read)
#[test]
fn odr_fallback_keep_meta() {
    let o = Odr { keep_meta: true, ..Odr::default() };
    assert_eq!(update_fallback(&o), None, "not a check: the update's own");
    let (m, mut changed, d) = unreadable();
    let view = m.view();
    assert!(matches!(odr(&view, &changed, &d, &o), Err(IncrReason::StateCorrupt(_))));
    changed.files[0].meta_units = 2;
    assert_eq!(odr(&view, &changed, &d, &o), META);
    changed.files[0].meta_units = 0;
    changed.files[0].change = Change::Gone;
    assert_eq!(odr(&view, &changed, &d, &o), META);
}

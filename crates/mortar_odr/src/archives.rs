//! O-23 (`spec/odr.md`, `--odr-check-archives`): the archive members that
//! the link did not extract are read too, and a member whose kind 2 row
//! disagrees with the row of the definition the link keeps is the warning
//! `odr-archive-shadow` (K17). WP-076.
//!
//! "Not extracted" is resolution's final answer (`file_live` is `Dead`):
//! a member that a later reference extracted is live and is compared by
//! the record checks, never here. A member's units are read only when the
//! option is on; its rows are anchored to the member's own defined
//! globals (O-4) and its definition is the one of the same symbol that the
//! link keeps ([`Cx::live_defs`]); rows of different classes, units of
//! different schemes (O-11) and C units (O-14) are not compared.

use mortar_core::inputs::Reader;
use mortar_core::par::Pool;
use mortar_core::{FileId, SymbolId};
use mortar_diag::{Diagnostic, Key, Kind, Place, Relation};
use mortar_elf::SectionIndex;
use mortar_meta::read::{DefinitionRow, FileReader, Unit};
use mortar_meta::{flags, fnv1a64};

use crate::foreign::PHASE;
use crate::records::{Anchor, Counts, Cx, Entry, Info, Loaded, anchor, info_of, load, seq, tag};

/// The containers of the archive members that are not linked, sorted by
/// (file, section): MT-6's predicate on each member's section headers,
/// which the link holds already (the type and flags first, the name only
/// for a candidate).
fn containers(cx: &Cx<'_, '_, '_>, pool: &Pool) -> Vec<(FileId, SectionIndex)> {
    let files: Vec<FileId> = cx
        .inputs
        .archives
        .iter()
        .flat_map(|(_, a)| a.members.iter())
        .filter(|&f| cx.resolution.file_live.get(f).is_some_and(|l| !l.is_live()))
        .collect();
    pool.unit_cost(2_000)
        .map_collect(files.len(), |i, out: &mut Vec<(FileId, SectionIndex)>| {
            let file = files[i];
            let Some(f) = cx.inputs.files.get(file) else { return };
            if f.member.is_none() {
                return;
            }
            let Reader::Object(obj) = f.reader else { return };
            for sec in obj.sections() {
                if mortar_meta::is_candidate(sec.sh_type(), sec.flags())
                    && sec.name().is_ok_and(|n| n == mortar_meta::SECTION_NAME)
                {
                    out.push((file, sec.index()));
                }
            }
        })
        .items
}

/// One member's units, parsed.
struct Member<'l> {
    units: Vec<Unit<'l>>,
}

/// A disagreement found: the member's file, unit and row, and the keeper's
/// entry.
struct Shadow<'l> {
    member: usize,
    unit: usize,
    row: usize,
    sym: SymbolId,
    name: &'l [u8],
    keeper: Entry<'l>,
    /// O-32's case, reported under `--odr=strict` only.
    system: bool,
}

/// O-23 over the members that are not linked, against `live` (the keepers'
/// entries, sorted by symbol). The findings are keyed in phase 40 by the
/// member's `FileId` and the symbol.
pub(crate) fn check<'s: 'l, 'l>(
    cx: &Cx<'_, 's, 'l>,
    live: &[(SymbolId, Entry<'l>)],
    pool: &Pool,
) -> (Vec<(Key, Diagnostic)>, Counts) {
    let mut counts = Counts::default();
    let meta = containers(cx, pool);
    if meta.is_empty() {
        return (Vec::new(), counts);
    }
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut start = 0;
    for i in 1..=meta.len() {
        if i == meta.len() || meta[i].0 != meta[start].0 {
            runs.push((start, i));
            start = i;
        }
    }
    let loaded: Vec<Loaded<'s>> = pool
        .unit_cost(20_000)
        .map_collect(runs.len(), |u, out: &mut Vec<Loaded<'s>>| {
            let (a, b) = runs[u];
            if let Some(l) = load(cx.inputs, &meta[a..b]) {
                out.push(l);
            }
        })
        .items;
    counts.archive_members = loaded.len() as u64;
    counts.archive_bytes =
        loaded.iter().flat_map(|l| &l.sections).map(|(_, b)| b.len() as u64).sum();
    let members: Vec<Member<'_>> = pool
        .unit_cost(20_000)
        .map_collect(loaded.len(), |u, out: &mut Vec<Member<'_>>| {
            let mut reader = FileReader::new();
            for (attrs, bytes) in &loaded[u].sections {
                reader.section(*attrs, bytes);
            }
            // A unit that does not parse gives no rows: the member is not
            // part of the program, so nothing is reported for it (O-23).
            out.push(Member { units: reader.units });
        })
        .items;
    counts.archive_units = members.iter().map(|m| m.units.len() as u64).sum();
    let found = pool.unit_cost(20_000).map_collect(members.len(), |m, out: &mut Vec<Found<'_>>| {
        compare(cx, live, &loaded[m], &members[m], m, out);
    });
    let mut shadows = Vec::new();
    for f in found.items {
        match f {
            Found::Shadow(s) => shadows.push(s),
            Found::Compared => counts.archive_compared += 1,
            Found::Schemes => counts.pairs_not_compared += 1,
            Found::SystemStd => {
                counts.archive_compared += 1;
                counts.system_std += 1;
            }
        }
    }
    counts.archive_compared += shadows.len() as u64;
    let mut out: Vec<(Key, Diagnostic)> =
        shadows.iter().map(|s| diagnostic(cx, &loaded, &members, s)).collect();
    out.sort_by_key(|(k, _)| *k);
    (out, counts)
}

/// What comparing one member row gave.
enum Found<'l> {
    Shadow(Shadow<'l>),
    /// Compared, equal.
    Compared,
    /// Not compared: different schemes, or a unit without one (O-11).
    Schemes,
    /// O-32: every row `SYSTEM` and the units' `-std` differ, not reported
    /// below `--odr=strict`.
    SystemStd,
}

/// The kind 2 rows of member `m` against the keepers' rows: one result per
/// row compared, at most one shadow per (member, symbol).
fn compare<'s: 'l, 'l>(
    cx: &Cx<'_, 's, 'l>,
    live: &[(SymbolId, Entry<'l>)],
    loaded: &Loaded<'s>,
    member: &Member<'l>,
    m: usize,
    out: &mut Vec<Found<'l>>,
) {
    let map = cx.inputs.sym_map_of(loaded.file);
    let mut reported: Vec<SymbolId> = Vec::new();
    for (u, unit) in member.units.iter().enumerate() {
        let info = info_of(0, loaded.file, unit);
        if info.lang_c {
            continue;
        }
        for (i, r) in unit.definitions().iter().enumerate() {
            // The member's own defined global of the row's name (O-4): a
            // named row too must name a definition of this member.
            let key = if r.name == 0 { r.key } else { fnv1a64(unit.string(r.name)) };
            let Anchor::Name(name, index) = anchor(&loaded.syms, key, true) else { continue };
            if r.name != 0 && unit.string(r.name) != name {
                continue;
            }
            let Some(slot) = index.0.checked_sub(loaded.first_global) else { continue };
            let Some(&sym) = map.get(slot as usize) else { continue };
            let Ok(at) = live.binary_search_by_key(&sym, |(s, _)| *s) else { continue };
            let keeper = live[at].1;
            let Some(k) = cx.definition(&keeper) else { continue };
            if k.class != r.class {
                continue;
            }
            let kinfo = cx.info[keeper.unit as usize];
            if info.scheme.is_none() || info.scheme != kinfo.scheme {
                out.push(Found::Schemes);
                continue;
            }
            if k.hash == r.hash {
                out.push(Found::Compared);
                continue;
            }
            let system = system_std(&k, r, &info, &kinfo);
            if system && !cx.switches.system_std_reported {
                out.push(Found::SystemStd);
                continue;
            }
            if reported.contains(&sym) {
                out.push(Found::Compared);
                continue;
            }
            reported.push(sym);
            out.push(Found::Shadow(Shadow {
                member: m,
                unit: u,
                row: i,
                sym,
                name,
                keeper,
                system,
            }));
        }
    }
}

/// O-32: both rows `SYSTEM` and the units' `-std` differ.
fn system_std(k: &DefinitionRow, r: DefinitionRow, a: &Info<'_>, b: &Info<'_>) -> bool {
    k.flags & r.flags & flags::SYSTEM != 0 && a.std != b.std
}

/// The warning of one shadow: the member's row "defined differently in",
/// after the kept definition "defined in" (§9.1's record places).
fn diagnostic<'l>(
    cx: &Cx<'_, '_, 'l>,
    loaded: &[Loaded<'_>],
    members: &[Member<'_>],
    s: &Shadow<'l>,
) -> (Key, Diagnostic) {
    let file = loaded[s.member].file;
    let unit = &members[s.member].units[s.unit];
    let name = String::from_utf8_lossy(s.name).into_owned();
    let key = Key::new(PHASE, file.0, seq(tag::ARCHIVE, u64::from(s.sym.0)));
    let keeper_file = cx.file_of(&s.keeper);
    let row = unit.definitions().get(s.row);
    let source = row.and_then(|r| {
        let p = unit.string(r.file);
        (!p.is_empty()).then(|| (String::from_utf8_lossy(p).into_owned(), r.line))
    });
    let member_place = Place::record(cx.file_ref(file), Some(name.clone()), false, source);
    let mut d = Diagnostic::warning(Kind::OdrArchiveShadow { name })
        .with(Relation::DefinedIn, cx.place(&s.keeper, Some(keeper_file)))
        .with(Relation::DefinedDifferentlyIn, member_place);
    let flagged = |f: u16| {
        cx.definition(&s.keeper).is_some_and(|k| k.flags & f != 0)
            || row.is_some_and(|r| r.flags & f != 0)
    };
    if s.system {
        d = d.note(
            "every definition is in a system header, and the translation units' -std differ \
             (spec/odr.md O-32)",
        );
    }
    if flagged(flags::PARTIAL) {
        d = d.note(
            "a record is PARTIAL (its producer did not hash every construct; spec/odr.md O-15)",
        );
    }
    d = d.note(format!(
        "{} is not linked: an archive member is linked only to resolve an undefined reference; \
         with another link order, or a reference to another of its symbols, its definition is \
         the one linked (spec/odr.md O-23, K17)",
        cx.file_text(file)
    ));
    (key, d)
}

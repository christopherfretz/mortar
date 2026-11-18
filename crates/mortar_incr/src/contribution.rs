//! The test path's guard (WP-064a; `spec/incr.md` §5.2, the review's M4
//! and m2): what one changed file contributes to resolution and layout,
//! compared with the rows the state stored for it, before the test path
//! writes anything. The test path refills the changed file's own units
//! only; an edit that changes what another file's units or the layout
//! depend on is the update proper's (WP-064: delta resolution, IN-27's
//! refill set) and is a fall-back by name here:
//!
//! - `layout-changed`: the file's section count, a section's type, flags,
//!   size or alignment, its group (through its identity), the positions
//!   of its live merged fragments, or the identities (`FRAGS` rows)
//!   they are;
//! - `resolution-changed`: the set of global keys its symbol table names,
//!   or any one's binding, type, visibility (`st_other`), definedness or
//!   section; or, for each such key, how the link resolves it (kind,
//!   definer, section, value, size, binding, type, `st_other`); or a GOT
//!   slot or thunk keyed by one of its locals whose name it gives to more
//!   than one local (a section symbol: §7.1's identity is then a
//!   position, and the slot may now denote another symbol).
//!
//! The comparison reads the changed file's tables and, for each key it
//! names, the definer's one symbol entry; it walks `SLOTS` and `PATCHES`
//! once for the rows naming its locals: O(the changed file's rows + the
//! slots and patches).

use mortar_core::hash::HashMap;
use mortar_core::id::{FileId, FragId, Id as _, SymKey, SymbolId};
use mortar_core::inputs::Reader;
use mortar_core::layout::{Layout, MergeMap};

use crate::capture::{Link, Positional, Translation, header_fields};
use crate::format::{MentionRow, NONE, ResolvedRow, State};
use crate::tables::{file_mentions, resolved_fields, section_of};

/// Why the changed file's contribution differs: `layout-changed` or
/// `resolution-changed` with its detail (WP-213: the one reason type).
pub type Changed = mortar_core::incr::IncrReason;

fn layout(detail: String) -> Changed {
    Changed::LayoutChanged(detail)
}

fn resolution(detail: String) -> Changed {
    Changed::ResolutionChanged(detail)
}

/// Compares what `file` (`FILES` row `row`) contributes in the new link
/// (`link`, its replayed `layout`) with what the state `s` stored for it,
/// through the replay's translation `tr`. `Ok` when nothing that another
/// file's bytes or the layout depend on differs.
#[allow(clippy::too_many_lines)] // one block per kind of row
pub fn compare(
    link: &Link<'_, '_>,
    s: &State,
    pos: &Positional,
    tr: &Translation,
    layout_now: &Layout,
    file: FileId,
    row: u32,
) -> Result<(), Changed> {
    let inputs = link.inputs;
    let res = link.resolution;
    let name = String::from_utf8_lossy(inputs.files[file].path).into_owned();
    // Sections: the count, then each stored row's section and header.
    let (first, count) = pos.sections_of(row).unwrap_or((0, 0));
    let sections = res.section_base[file].len();
    if count != sections {
        return Err(layout(format!("{name}: {sections} sections, the state has {count}")));
    }
    let mut hit = vec![false; sections as usize];
    for k in first..first + count {
        let x = &s.sections[k as usize];
        let at = tr.sections.get(k as usize).copied().flatten().and_then(|id| res.sh_index(id));
        let Some((f, index)) = at.filter(|(f, _)| *f == file) else {
            return Err(layout(format!(
                "{name}: the section {} of the state is not one of its sections",
                String::from_utf8_lossy(s.str(x.name))
            )));
        };
        let i = index.0 as usize;
        if hit.get(i).copied().unwrap_or(true) {
            return Err(layout(format!("{name}: section {i} is named twice")));
        }
        hit[i] = true;
        let (ty, fl, size, align, _) = header_fields(inputs, f, index);
        if (ty, fl, size, align) != (x.sh_type, x.sh_flags, x.size, x.align) {
            return Err(layout(format!(
                "{name}: section {} ({i}) changed type, flags, size or alignment",
                String::from_utf8_lossy(s.str(x.name))
            )));
        }
    }
    // Merged fragments: the live ones' positions, (section, offset).
    let lo = s.frag_at.partition_point(|r| r.section < first);
    let hi = s.frag_at.partition_point(|r| r.section < first + count);
    let mut stored: Vec<(u32, u32)> = Vec::with_capacity(hi - lo);
    for r in &s.frag_at[lo..hi] {
        let id = tr.sections.get(r.section as usize).copied().flatten();
        let index = id.and_then(|id| res.sh_index(id)).map_or(NONE, |(_, i)| i.0);
        stored.push((index, r.offset));
    }
    stored.sort_unstable();
    let mut now: Vec<(u32, u32)> = Vec::new();
    for (i, &off) in layout_now.merge.frag_off.as_slice().iter().enumerate() {
        if off == MergeMap::NONE {
            continue;
        }
        let Some(f) = link.split.frags.get(FragId::from_index(i as u32)) else { continue };
        if let Some((x, index)) = res.sh_index(f.section)
            && x == file
        {
            now.push((index.0, f.offset));
        }
    }
    now.sort_unstable();
    if stored != now {
        return Err(layout(format!(
            "{name}: its live merged fragments are not where they were ({} now, {} stored)",
            now.len(),
            stored.len()
        )));
    }
    // The FRAGS rows its live fragments are (the identities they merge
    // into), as a multiset: equal when its strings only moved inside it
    // (the review's s2). Otherwise another file's copy of an identity with
    // ordinals is ranked among the live copies (IN-18a `FRAGS`) and may
    // move (the review's s4, M1), and an identity it no longer has leaves
    // its bytes in the output with no line of the map (the review's W6,
    // m2): the full link's to place.
    let mut rows_was: Vec<u32> =
        s.frag_at[lo..hi].iter().map(|r| u32::try_from(r.frag).unwrap_or(NONE)).collect();
    let mut rows_now: Vec<u32> = tr
        .frag_rows
        .iter()
        .filter(|(fid, _)| {
            link.split
                .frags
                .get(*fid)
                .and_then(|f| res.sh_index(f.section))
                .is_some_and(|(x, _)| x == file)
        })
        .map(|&(_, r)| r)
        .collect();
    rows_was.sort_unstable();
    rows_now.sort_unstable();
    if rows_was != rows_now {
        return Err(layout(format!(
            "{name}: its merged fragments are not the identities they were ({} now, {} stored): a copy added or dropped moves another file's copy ranked among them, or leaves a dead fragment the full link would not place",
            rows_now.len(),
            rows_was.len()
        )));
    }
    let Reader::Object(object) = inputs.files[file].reader else { return Ok(()) };
    let table = object.symbols().map_err(|_| resolution(format!("{name}: its symbol table")))?;
    // Its locals that a slot or a thunk names: a local's identity is its
    // name and its ordinal among the file's locals of that name (§7.1),
    // which for a name the file gives to more than one local (every
    // section symbol's empty name) is a position. A slot (the GOT is not
    // refilled) or a thunk keyed by such a local may now denote another
    // symbol: not judged here.
    if let Some(t) = &table {
        let first_global = t.first_global().0;
        let local_name =
            |i: u32| t.get(mortar_elf::SymbolIndex(i)).ok().and_then(|y| t.name(&y).ok());
        let keyed = s.slots.iter().map(|r| r.symbol).chain(
            s.patches
                .iter()
                .filter(|p| {
                    p.kind == crate::format::patch_kind::THUNK
                        || p.kind == crate::format::patch_kind::THUNK_PLT
                })
                .map(|p| p.symbol),
        );
        let mut wanted: Vec<&[u8]> = Vec::new();
        for r in keyed {
            let Some(y) = s.symbols.get(r as usize) else { continue };
            if y.kind != crate::format::sym_kind::LOCAL || y.file != row {
                continue;
            }
            if let Some(mortar_core::id::SymKind::Local(e)) =
                tr.symbols.get(r as usize).copied().flatten().map(SymKey::kind)
                && let Some(n) = local_name(e.index.0)
            {
                wanted.push(n);
            }
        }
        if !wanted.is_empty() {
            wanted.sort_unstable();
            wanted.dedup();
            let mut count = vec![0u32; wanted.len()];
            for i in 0..first_global {
                if let Some(n) = local_name(i)
                    && let Ok(k) = wanted.binary_search(&n)
                {
                    count[k] += 1;
                }
            }
            if let Some(k) = count.iter().position(|&c| c > 1) {
                return Err(resolution(format!(
                    "{name}: a GOT slot or a thunk names one of its {} locals called \"{}\", whose identity is a position (§7.1)",
                    count[k],
                    String::from_utf8_lossy(wanted[k])
                )));
            }
        }
    }
    // Mentions: the keys its symbol table names and how.
    let mut new: Vec<MentionRow> = match &table {
        Some(t) => file_mentions(inputs, res, file, t),
        None => Vec::new(),
    };
    new.sort_by_key(|m| m.symbol);
    new.dedup_by_key(|m| m.symbol);
    let lo = s.mentions.partition_point(|m| m.file < row);
    let hi = s.mentions.partition_point(|m| m.file <= row);
    let mut old: Vec<(MentionRow, u32)> = Vec::with_capacity(hi - lo);
    for m in &s.mentions[lo..hi] {
        let key = tr.symbols.get(m.symbol as usize).copied().flatten();
        let Some(id) = key.and_then(SymKey::as_global) else {
            return Err(resolution(format!(
                "{name}: it no longer names {}",
                String::from_utf8_lossy(
                    s.symbols.get(m.symbol as usize).map_or(&b""[..], |y| s.str(y.name))
                )
            )));
        };
        let section = if m.section == NONE {
            NONE
        } else {
            tr.sections.get(m.section as usize).copied().flatten().map_or(NONE - 1, |x| x.0)
        };
        old.push((MentionRow { file: file.0, symbol: id.0, section, ..*m }, m.symbol));
    }
    old.sort_by_key(|m| m.0.symbol);
    let key_name = |id: u32| {
        inputs
            .symbols
            .get(SymbolId(id))
            .map_or_else(String::new, |k| String::from_utf8_lossy(k.name).into_owned())
    };
    if old.len() != new.len() {
        return Err(resolution(format!(
            "{name}: its symbol table names {} global keys, the state {}",
            new.len(),
            old.len()
        )));
    }
    for ((o, _), n) in old.iter().zip(&new) {
        if o != n {
            return Err(resolution(format!(
                "{name}: the global {} changed its key, binding, type, visibility, definedness or section",
                key_name(n.symbol.max(o.symbol))
            )));
        }
    }
    // How each key it names is resolved.
    let commons: HashMap<SymbolId, u64> =
        res.common.iter().rev().map(|&(k, shape)| (k, shape.size)).collect();
    let table_of = |f: FileId| match inputs.files.get(f).map(|x| x.reader) {
        Some(Reader::Object(o)) => o.symbols().ok().flatten(),
        _ => None,
    };
    let row_of_section = |f: FileId, sec| section_of(res, f, sec);
    for (o, key_row) in &old {
        let id = SymbolId(o.symbol);
        let Some(&def) = res.def.get(id) else {
            return Err(resolution(format!("{name}: {} has no resolution", key_name(o.symbol))));
        };
        let def = link.assignment.def(res, id).unwrap_or(def);
        let now = resolved_fields(link, &table_of, id, def, &row_of_section, &commons)
            .map_err(|e| resolution(format!("{name}: {e}")))?;
        let at = s.resolved.partition_point(|r| r.symbol < *key_row);
        let Some(was) = s.resolved.get(at).filter(|r| r.symbol == *key_row && r.kind <= 5) else {
            return Err(resolution(format!("{name}: {} was not resolved", key_name(o.symbol))));
        };
        let file_now = if was.file == NONE {
            NONE
        } else {
            tr.files.get(was.file as usize).copied().flatten().map_or(NONE - 1, |f| f.0)
        };
        let section_now = if was.section == NONE {
            NONE
        } else {
            tr.sections.get(was.section as usize).copied().flatten().map_or(NONE - 1, |x| x.0)
        };
        let was = ResolvedRow { symbol: NONE, file: file_now, section: section_now, ..*was };
        if was != now {
            return Err(resolution(format!(
                "{name}: {} is resolved differently (definer, kind, section, value, size, binding, type or visibility)",
                key_name(o.symbol)
            )));
        }
    }
    Ok(())
}

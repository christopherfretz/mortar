//! The test path's guard (WP-064a; `spec/incr.md` §5.2, the review's M4
//! and m2): what one changed file contributes to resolution and layout,
//! compared with the rows the state stored for it, before the test path
//! writes anything. The test path refills the changed file's own units
//! only; an edit that changes what another file's units or the layout
//! depend on is the update proper's (WP-064: delta resolution, IN-27's
//! refill set) and is a fall-back by name here:
//!
//! - `layout-changed`: the file's section count, a section's type, flags,
//!   size or alignment, its group (through its identity), or the
//!   positions of its live merged fragments;
//! - `resolution-changed`: the set of global keys its symbol table names,
//!   or any one's binding, type, visibility (`st_other`), definedness or
//!   section; or, for each such key, how the link resolves it (kind,
//!   definer, section, value, size, binding, type, `st_other`).
//!
//! The comparison reads the changed file's tables and, for each key it
//! names, the definer's one symbol entry: O(the changed file's rows).

use mortar_core::hash::HashMap;
use mortar_core::id::{FileId, FragId, Id as _, SymbolId};
use mortar_core::inputs::Reader;
use mortar_core::layout::{Layout, MergeMap};

use crate::capture::{Link, Positional, Translation, header_fields};
use crate::format::{MentionRow, NONE, ResolvedRow, State};
use crate::tables::{file_mentions, resolved_fields, section_of};

/// Why the changed file's contribution differs: `(reason, detail)`, the
/// reason a §7 name.
pub type Changed = (&'static str, String);

fn layout(detail: String) -> Changed {
    ("layout-changed", detail)
}

fn resolution(detail: String) -> Changed {
    ("resolution-changed", detail)
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
    let n = res.section_base[file].len();
    if count != n {
        return Err(layout(format!("{name}: {n} sections, the state has {count}")));
    }
    let mut hit = vec![false; n as usize];
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
        let (t, fl, size, align, _) = header_fields(inputs, f, index);
        if (t, fl, size, align) != (x.sh_type, x.sh_flags, x.size, x.align) {
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
    // Mentions: the keys its symbol table names and how.
    let Reader::Object(object) = inputs.files[file].reader else { return Ok(()) };
    let table = object
        .symbols()
        .map_err(|_| resolution(format!("{name}: its symbol table")))?;
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
        let Some(id) = key.and_then(|k| k.as_global()) else {
            return Err(resolution(format!(
                "{name}: it no longer names {}",
                String::from_utf8_lossy(s.symbols.get(m.symbol as usize).map_or(&b""[..], |y| s.str(y.name)))
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
        inputs.symbols.get(SymbolId(id)).map_or_else(String::new, |k| {
            String::from_utf8_lossy(k.name).into_owned()
        })
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
    let section_row = |f: FileId, sec| section_of(res, f, sec);
    for (o, key_row) in &old {
        let id = SymbolId(o.symbol);
        let Some(&def) = res.def.get(id) else {
            return Err(resolution(format!("{name}: {} has no resolution", key_name(o.symbol))));
        };
        let def = link.assignment.def(res, id).unwrap_or(def);
        let now = resolved_fields(link, &table_of, id, def, &section_row, &commons)
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

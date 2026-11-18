//! `.symtab` and `.strtab` per file in an update (`spec/incr.md` IN-49a):
//! each changed or drawn-in file's block rewritten in place or moved to the
//! tail reserve, the global area kept dense. Not `crate::symtab::SymtabPlan`
//! (the full link's). WP-213 (U-1): the types [`FileSymbols`],
//! [`LocalSymbol`], [`GlobalChange`] and the entry point [`plan`];
//! **filled by U-5** (the function only).

use std::collections::HashMap;

use mortar_core::incr::{
    IncrReason, LayoutDelta, PlaceNow, StoredLayout, SymtabBlock, SymtabEntry, SymtabPlan,
    SymtabTables, Unimplemented,
};
use mortar_core::{FileId, SectionId, SymbolId};
use mortar_elf::consts::{SHN_ABS, STT_TLS};

use crate::symtab::{block_room, fixed_name};

/// One local entry of a file's block (S-74), as read from the file's
/// symbol table by the join. Bytes and indices as ELF gives them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalSymbol<'a> {
    /// The name (no terminator); empty for an unnamed entry.
    pub name: &'a [u8],
    /// `st_info`, `st_other`.
    pub info: u8,
    pub other: u8,
    /// Its section in the update's id space, or `None` (absolute or
    /// `STT_FILE`).
    pub section: Option<SectionId>,
    /// `st_value`: the offset in `section` (bytes), or the absolute value.
    pub value: u64,
    /// `st_size` (bytes).
    pub size: u64,
}

/// A file whose block is rewritten (IN-49a: changed, or drawn-in by IN-72
/// item 2a), with its local entries in S-74's order. Written by the join;
/// read by U-5. Key: `file`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSymbols<'a> {
    /// Its `FILES` row (the update's id space).
    pub file: FileId,
    pub locals: Vec<LocalSymbol<'a>>,
}

/// What happens to a global's entry (IN-49a).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalKind {
    /// New: appended at the area's end.
    Added,
    /// Removed: the area's last entry takes its index.
    Removed,
    /// Demoted to a local of its definer's block (S-74).
    Demoted,
    /// Promoted back to the area (appended).
    Promoted,
    /// Same index, new value, section, size, binding or name.
    Changed,
}

/// A global whose `.symtab` entry the update changes (IN-49a; WP-213 added
/// it to `plan`'s inputs: WP-211 §C gave U-5 no globals). Written by the
/// join from U-3's `ResolveDelta`; read by U-5. Key: `key`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlobalChange<'a> {
    /// The `SYMBOLS` row.
    pub key: SymbolId,
    pub kind: GlobalKind,
    pub name: &'a [u8],
    pub info: u8,
    pub other: u8,
    /// Its definition's section in the update's id space, or `None`.
    pub section: Option<SectionId>,
    pub value: u64,
    pub size: u64,
}

/// Plan the update's `.symtab` and `.strtab` writes (WP-211 §C; IN-49a).
/// Called by the join after U-4 (IN-70 U7).
///
/// What it reads (IN-49a "what an update reads and writes"): the stored
/// global area and table sizes once (`symtab_tables`), the `SYMTAB` row of
/// each file of `files` (`symtab_block`), and the place of each section
/// its locals name (`LayoutDelta::place_of`, once a section); nothing
/// else of the state, never the previous output (U-6 copies and remaps
/// what [`SymtabPlan::moves`] and [`SymtabPlan::remap`] name).
///
/// - Each file of `files` gets its block rewritten from its start: in
///   place when its entries fit the block's capacity (the rest of it
///   fillers), else moved to the local tail reserve with capacity `count +
///   max(2, count/8)` (or what is left of the reserve, when the entries
///   still fit), its old place fillers. Its names go to its `.strtab`
///   region when they fit its capacity, else to a new region at the
///   table's end with `max(16, len/8)` slack; what no entry names of a
///   region is a dead range. A file with no entry and no block has none.
/// - `Added` and `Promoted` globals are appended to the global area, in
///   key order, their names at `.strtab`'s end.
/// - Every growth step makes the rest of both areas a [`remap`] range.
///
/// Not built (v1, `update-not-implemented (symtab)`, by name): a global
/// that is `Removed`, `Demoted` or `Changed` (its stored `.symtab` index
/// and `.strtab` offset, its `SYMTAB_KEYS` row, reach `plan` through
/// neither `StoredLayout` nor `GlobalChange`: WP-217's seam finding; the
/// removal rule itself is [`area_after_removals`]); a symbol whose section
/// has no place (merged pieces, `.eh_frame`) or no address (a
/// non-allocated chunk); a global with no section (absolute, undefined
/// and common are not told apart by `GlobalChange`).
///
/// [`remap`]: SymtabPlan::remap
///
/// # Errors
/// `symtab-exhausted` (the tail reserve, `.symtab`'s or `.strtab`'s file
/// slack), `state-corrupt` (stored rows that contradict each other, a
/// chunk the placement does not have), `update-not-implemented (symtab)`
/// (above).
pub fn plan(
    stored: &StoredLayout<'_>,
    layout: &LayoutDelta,
    files: &[FileSymbols<'_>],
    globals: &[GlobalChange<'_>],
) -> Result<SymtabPlan, IncrReason> {
    let (area, tables) = stored.symtab_tables()?;
    let entries = u64::from(area.first) + u64::from(area.count);
    if area.first != tables.sh_info || entries == 0 || tables.symtab_size % entries != 0 {
        return Err(corrupt("SYMTAB: the global area does not start at sh_info"));
    }
    let entsize = tables.symtab_size / entries;
    if entsize != 16 && entsize != 24 {
        return Err(corrupt("SYMTAB: .symtab's size is not a whole number of entries"));
    }
    if globals
        .iter()
        .any(|g| matches!(g.kind, GlobalKind::Removed | GlobalKind::Demoted | GlobalKind::Changed))
    {
        return Err(not_built());
    }
    let mut order: Vec<&FileSymbols<'_>> = files.iter().collect();
    order.sort_by_key(|f| f.file);
    if order.windows(2).any(|w| w[0].file == w[1].file) {
        debug_assert!(false, "a file given twice");
        return Err(not_built());
    }
    let mut cx = Cx { stored, layout, places: HashMap::new() };
    let mut out = SymtabPlan::default();
    let mut tail = (tables.tail_first, tables.tail_capacity);
    let str_limit = tables.strtab_size.saturating_add(tables.strtab_slack);
    let mut str_end = tables.strtab_size;
    for fs in order {
        let old = stored.symtab_block(fs.file)?;
        let mut names = Vec::new();
        let mut rows = Vec::with_capacity(fs.locals.len());
        for l in &fs.locals {
            let (shndx, value) = cx.value(l.section, l.value, l.info)?;
            let name = fixed_name(l.name).map_or_else(
                || {
                    let at = names.len() as u64;
                    names.extend_from_slice(l.name);
                    names.push(0);
                    Name::Region(at)
                },
                Name::Fixed,
            );
            rows.push((name, l.info, l.other, shndx, value, l.size));
        }
        let Ok(n) = u32::try_from(rows.len()) else {
            return Err(exhausted("a block of more than 2^32 entries"));
        };
        let bytes = names.len() as u64;
        // .symtab: in place, or the tail reserve.
        let (first, capacity) = match old {
            Some(b) if n <= b.capacity => (b.first, b.capacity),
            None if n == 0 => continue,
            _ => {
                let want = n.saturating_add(block_room(n, 0).0);
                let cap = if want <= tail.1 {
                    want
                } else if n <= tail.1 {
                    tail.1
                } else {
                    return Err(exhausted(".symtab's local tail reserve"));
                };
                let first = tail.0;
                tail = (tail.0 + cap, tail.1 - cap);
                if let Some(b) = old
                    && b.capacity > 0
                {
                    out.fillers.push((b.first, b.first + b.capacity));
                }
                (first, cap)
            }
        };
        // .strtab: the file's region, or a new one at the end.
        let (str_at, str_cap) = match old {
            Some(b) if bytes <= b.strtab_capacity => (b.strtab_offset, b.strtab_capacity),
            _ => {
                let cap = bytes + block_room(n.max(1), bytes).1;
                let at = str_end;
                str_end = at.saturating_add(cap);
                if str_end > str_limit {
                    return Err(exhausted(".strtab's file slack"));
                }
                if let Some(b) = old
                    && b.strtab_capacity > 0
                {
                    out.strtab_dead.push((b.strtab_offset, b.strtab_capacity));
                }
                (at, cap)
            }
        };
        if bytes < str_cap {
            out.strtab_dead.push((str_at + bytes, str_cap - bytes));
        }
        if bytes > 0 {
            out.strtab.push((str_at, names));
        }
        for (k, (name, info, other, shndx, value, size)) in rows.into_iter().enumerate() {
            let name = match name {
                Name::Fixed(o) => o,
                Name::Region(at) => st_name(str_at + at)?,
            };
            out.entries.push(SymtabEntry {
                index: first + k as u32,
                name,
                info,
                other,
                shndx,
                value,
                size,
            });
        }
        if n < capacity {
            out.fillers.push((first + n, first + capacity));
        }
        out.blocks.push((
            fs.file,
            SymtabBlock {
                first,
                count: n,
                capacity,
                strtab_offset: str_at,
                strtab_len: bytes,
                strtab_capacity: str_cap,
            },
        ));
    }
    // The global area: appended globals, in key order.
    let mut appended: Vec<&GlobalChange<'_>> = globals
        .iter()
        .filter(|g| matches!(g.kind, GlobalKind::Added | GlobalKind::Promoted))
        .collect();
    appended.sort_by_key(|g| g.key);
    let old_end = area.first + area.count;
    let mut end = old_end;
    let mut buf: Vec<u8> = Vec::new();
    let names_at = str_end;
    for g in appended {
        let Some(_) = g.section else { return Err(not_built()) };
        let (shndx, value) = cx.value(g.section, g.value, g.info)?;
        let name = match fixed_name(g.name) {
            Some(o) => o,
            None => {
                let at = names_at + buf.len() as u64;
                buf.extend_from_slice(g.name);
                buf.push(0);
                st_name(at)?
            }
        };
        out.entries.push(SymtabEntry {
            index: end,
            name,
            info: g.info,
            other: g.other,
            shndx,
            value,
            size: g.size,
        });
        out.globals.push((g.key, end, name));
        end = end.checked_add(1).ok_or_else(|| exhausted(".symtab's entries"))?;
    }
    str_end = names_at.saturating_add(buf.len() as u64);
    if str_end > str_limit {
        return Err(exhausted(".strtab's file slack"));
    }
    if !buf.is_empty() {
        out.strtab.push((names_at, buf));
    }
    let symtab_total = tables.symtab_size.saturating_add(tables.symtab_slack);
    let symtab_size = u64::from(end) * entsize;
    if symtab_size > symtab_total {
        return Err(exhausted(".symtab's file slack"));
    }
    out.global_area = SymtabBlock {
        first: area.first,
        count: end - area.first,
        capacity: end - area.first,
        strtab_offset: area.strtab_offset,
        strtab_len: str_end.saturating_sub(area.strtab_offset),
        strtab_capacity: str_end.saturating_sub(area.strtab_offset),
    };
    out.tables = SymtabTables {
        sh_info: tables.sh_info,
        tail_first: tail.0,
        tail_capacity: tail.1,
        symtab_size,
        symtab_slack: symtab_total - symtab_size,
        strtab_size: str_end,
        strtab_slack: str_limit - str_end,
    };
    out.entries.sort_by_key(|e| e.index);
    out.fillers = merged(std::mem::take(&mut out.fillers));
    out.strtab.sort_by_key(|w| w.0);
    out.strtab_dead.sort_unstable();
    // IN-49a: a growth step moves sections of files this update does not
    // read; their entries' values are remapped through the placement.
    if !layout.growth_steps.is_empty() {
        let mut written: Vec<(u32, u32)> =
            out.blocks.iter().map(|(_, b)| (b.first, b.first + b.capacity)).collect();
        written.extend(out.fillers.iter().copied());
        written.push((old_end, end));
        out.remap = complement(merged(written), 1, end);
    }
    Ok(out)
}

/// A name of a planned entry: in the fixed table (S-86), or at a byte
/// offset in its block's names.
#[derive(Debug, Clone, Copy)]
enum Name {
    Fixed(u32),
    Region(u64),
}

/// What [`plan`] reads of the placement, with each section's place read
/// once.
struct Cx<'s, 'a> {
    stored: &'s StoredLayout<'a>,
    layout: &'s LayoutDelta,
    places: HashMap<SectionId, PlaceNow>,
}

impl Cx<'_, '_> {
    /// `(st_shndx, st_value)` of `value` bytes into `section` after the
    /// update (S-81, S-82): its chunk's new address plus its place's
    /// offset; `SHN_ABS` and the value for no section.
    fn value(
        &mut self,
        section: Option<SectionId>,
        value: u64,
        info: u8,
    ) -> Result<(u32, u64), IncrReason> {
        let Some(id) = section else { return Ok((u32::from(SHN_ABS), value)) };
        let place = match self.places.get(&id) {
            Some(p) => *p,
            None => {
                let p = self.layout.place_of(self.stored, id)?;
                self.places.insert(id, p);
                p
            }
        };
        let (PlaceNow::Here(p) | PlaceNow::Stored(p)) = place else { return Err(not_built()) };
        // Placed nowhere (merged pieces, `.eh_frame`): v1 does not plan it.
        let Some((chunk, offset)) = p.get() else { return Err(not_built()) };
        let Some(c) = self.layout.placement.chunks.get(chunk) else {
            return Err(corrupt(format!(
                "SECTIONS: section {} in chunk {}, which is none",
                id.0, chunk.0
            )));
        };
        let Some(shndx) = c.shndx else {
            return Err(corrupt(format!("chunk {} has no section header", chunk.0)));
        };
        // A non-allocated chunk's offset is not its section's (v1).
        let Some(addr) = c.addr else { return Err(not_built()) };
        let Some(at) = addr.checked_add(offset).and_then(|a| a.checked_add(value)) else {
            return Err(corrupt(format!("section {}: an address past 2^64", id.0)));
        };
        if info & 0xf == STT_TLS {
            let Some(tls) = self.layout.placement.tls else { return Err(not_built()) };
            return Ok((shndx, at.wrapping_sub(tls.start)));
        }
        Ok((shndx, at))
    }
}

/// The global area after removing the entries at `removed` (IN-49a: a
/// removed global's entry is taken by the area's last entry, one removal
/// after another), for an area of `[first, end)`: the new end, and the
/// moves `(from, to)` in previous-output indices, sorted by `to`. The
/// removals are applied from the highest index down, so every move reads
/// an entry the previous output has where it was; a removed entry that is
/// the last is dropped without a move.
///
/// `plan` does not call it yet: the removed globals' stored indices do
/// not reach `plan` (WP-217's seam finding).
#[must_use]
pub fn area_after_removals(first: u32, end: u32, removed: &[u32]) -> (u32, Vec<(u32, u32)>) {
    let mut removed: Vec<u32> =
        removed.iter().copied().filter(|&r| r >= first && r < end).collect();
    removed.sort_unstable();
    removed.dedup();
    // Which previous entry is at an index now, where that changed.
    let mut at: HashMap<u32, u32> = HashMap::new();
    let mut end = end;
    for &r in removed.iter().rev() {
        let last = end - 1;
        if r != last {
            let from = at.remove(&last).unwrap_or(last);
            at.insert(r, from);
        } else {
            at.remove(&last);
        }
        end = last;
    }
    let mut moves: Vec<(u32, u32)> = at
        .into_iter()
        .filter(|&(to, from)| to < end && to != from)
        .map(|(to, f)| (f, to))
        .collect();
    moves.sort_by_key(|m| m.1);
    (end, moves)
}

/// Sorted, merged `[start, end)` ranges (empty ones dropped).
fn merged(mut v: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
    v.retain(|r| r.0 < r.1);
    v.sort_unstable();
    let mut out: Vec<(u32, u32)> = Vec::with_capacity(v.len());
    for r in v {
        match out.last_mut() {
            Some(l) if r.0 <= l.1 => l.1 = l.1.max(r.1),
            _ => out.push(r),
        }
    }
    out
}

/// `[lo, hi)` minus sorted, merged `ranges`.
fn complement(ranges: Vec<(u32, u32)>, lo: u32, hi: u32) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let mut at = lo;
    for (s, e) in ranges {
        if s > at {
            out.push((at, s.min(hi)));
        }
        at = at.max(e);
    }
    if at < hi {
        out.push((at, hi));
    }
    out.retain(|r| r.0 < r.1);
    out
}

fn st_name(at: u64) -> Result<u32, IncrReason> {
    u32::try_from(at).map_err(|_| exhausted(".strtab past 4 GiB"))
}

fn not_built() -> IncrReason {
    IncrReason::UpdateNotImplemented(Some(Unimplemented::Symtab))
}

fn exhausted(what: &str) -> IncrReason {
    IncrReason::SymtabExhausted(what.to_owned())
}

fn corrupt(what: impl Into<String>) -> IncrReason {
    IncrReason::StateCorrupt(what.into())
}

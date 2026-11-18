//! `.symtab` and `.strtab` per file in an update (`spec/incr.md` IN-49a):
//! each changed or drawn-in file's block rewritten in place or moved to the
//! tail reserve, the global area kept dense. Not `crate::symtab::SymtabPlan`
//! (the full link's). WP-213 (U-1): the types [`FileSymbols`],
//! [`LocalSymbol`], [`GlobalChange`] and the entry point [`plan`];
//! **filled by U-5**. WP-217's second part (the supervisor's rulings 1 to
//! 3 and items 5 to 7): [`GlobalChange::stored`] and [`GlobalPlace`],
//! [`moved_keys`], [`FileSymbols::eh`] and [`FileSymbols::pieces`].

use std::collections::HashMap;

use mortar_core::column::Column;
use mortar_core::incr::{
    EhRange, EhRecordIn, IncrReason, LayoutDelta, MergedPiece, PlaceNow, StoredFrag, StoredLayout,
    SymtabBlock, SymtabEntry, SymtabPlan, SymtabTables, Unimplemented,
};
use mortar_core::layout::{Chunk, ChunkKind, DefValue, Defined};
use mortar_core::{ChunkId, FileId, OutSecId, SectionId, SymbolId};
use mortar_merge::EhFrames;
use mortar_merge::refs::eh_ref;

use crate::eh::{EhValue, EhWho};
use crate::symtab::{
    InternalValue, SymAddrs, Where, block_room, fixed_name, is_mapping, resolve_at,
};

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

/// A changed or drawn-in file's `.eh_frame` as the join read it for the
/// delta layout (IN-72 item 5; WP-217 ruling 5): what [`plan`] needs to
/// give a local of an input `.eh_frame` (an AArch64 `$d` at a record's
/// start, S-76) the value S-50 gives it in the full link, by the full
/// link's own function (`mortar_merge::refs::eh_ref`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileEh<'a> {
    /// `mortar_merge::eh::parse` of the file's `.eh_frame` sections: the
    /// frames its `EhFile` was made from (`sections[s].section` in the
    /// update's id space; record `j` of a section is its `EhRecordIn::piece`
    /// `j`).
    pub frames: &'a EhFrames,
    /// Its `EhFile::records`: the kept records in S-46 order, the order of
    /// its `EhRange::records` in `LayoutDelta::eh`.
    pub kept: &'a [EhRecordIn],
}

/// A file whose block is rewritten (IN-49a: changed, or drawn-in by IN-72
/// item 2a), with its local entries in S-74's order. Written by the join;
/// read by U-5. Key: `file`.
///
/// `locals` are the entries S-76 and S-78 keep in the full link, decided
/// by the join as the full link's keep rule decides them (`STT_SECTION`
/// dropped, a dead section's locals dropped, `-x`/`-X`, a `.L` local of a
/// split merge section dropped), with ONE exception that only the update's
/// layout decides: a local of an input `.eh_frame` is given as read, and
/// [`plan`] drops it when S-50 does (a mapping symbol of a removed record).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSymbols<'a> {
    /// Its `FILES` row (the update's id space).
    pub file: FileId,
    pub locals: Vec<LocalSymbol<'a>>,
    /// Its `.eh_frame` (`None`: it has none, or no local in it).
    pub eh: Option<FileEh<'a>>,
    /// Its merged pieces: `LayoutInput::pieces`' row of the file, in
    /// `(section, offset)` order (empty: no local in a split section).
    pub pieces: &'a [MergedPiece],
}

/// What happens to a global's entry (IN-49a).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalKind {
    /// New: appended at the area's end.
    Added,
    /// Removed: the area's last entry takes its index.
    Removed,
    /// Demoted to a local of its definer's block (S-74): it leaves the
    /// area as a removed one does; the join gives it among its definer's
    /// [`FileSymbols::locals`].
    Demoted,
    /// Promoted back to the area (appended).
    Promoted,
    /// Same key, same index: a new value, section, size, type, binding or
    /// visibility. The name is the key's and does not change; the entry
    /// keeps its stored `.strtab` offset.
    Changed,
}

/// Where a global is defined (WP-217 ruling 3), as the join reads it from
/// the resolution's `def_kind` (`spec/synth.md`'s `SymDef`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalPlace {
    /// `GlobalChange::value` bytes into this input section (the update's id
    /// space).
    Section(SectionId),
    /// `SHN_ABS`, `GlobalChange::value` as is.
    Absolute,
    /// Undefined (a referenced weak undefined): `SHN_UNDEF`, 0.
    Undefined,
    /// A common symbol. An update does not place a common in v1 (R-2 of
    /// WP-216's rulings: the layout answers `update-not-implemented
    /// (common)` before [`plan`] is called); one that reaches [`plan`] is
    /// refused, `update-not-implemented (symtab)`.
    Common,
    /// Linker-defined (layout §13): the value the update's placement gives
    /// it (`Placement::defined`, IN-71c).
    Linker,
}

/// A key with no `.symtab` global entry in `SYMTAB_KEYS`' index column
/// (`mortar_incr::update::rows::SymtabKeyRow`'s `NONE`).
pub const NO_ENTRY: u32 = u32::MAX;

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
    /// Where it is defined (WP-217 ruling 3).
    pub place: GlobalPlace,
    pub value: u64,
    pub size: u64,
    /// Its stored `SYMTAB_KEYS` row (IN-18e): `(.symtab index, .strtab
    /// offset)`, the index [`NO_ENTRY`] for a key the previous output has
    /// no global entry for (WP-217 ruling 1). The join reads it through
    /// `mortar_incr::update::view`'s `symtab_key(key)`. `None` for `Added`
    /// and `Promoted`; required for `Removed`, `Demoted` and `Changed` (a
    /// `None` there is `state-corrupt`, naming the key).
    pub stored: Option<(u32, u32)>,
}

/// Plan the update's `.symtab` and `.strtab` writes (WP-211 §C; IN-49a).
/// Called by the join after U-4 (IN-70 U7).
///
/// What it reads (IN-49a "what an update reads and writes"): the stored
/// global area and table sizes once (`symtab_tables`), the `SYMTAB` row of
/// each file of `files` (`symtab_block`), the place of each section its
/// entries name (`LayoutDelta::place_of`, once a section), the `FRAGS` row
/// of each fragment a local of a split section lies in (`frags_of`, once a
/// fragment, when `LayoutDelta::frags` does not have it); the placement's
/// whole chunk table once when an entry has a section (each output
/// section's start and size, as the full link's image derives them);
/// nothing else of the state, never the previous output (U-6 copies and
/// remaps what [`SymtabPlan::moves`] and [`SymtabPlan::remap`] name).
///
/// - Each file of `files` gets its block rewritten from its start: in
///   place when its entries fit the block's capacity (the rest of it
///   fillers), else moved to the local tail reserve with capacity `count +
///   max(2, count/8)` (or what is left of the reserve, when the entries
///   still fit), its old place fillers. Its names go to its `.strtab`
///   region when they fit its capacity, else to a new region at the
///   table's end with `max(16, len/8)` slack; what no entry names of a
///   region is a dead range. A file with no entry and no block has none.
/// - Every entry's `st_shndx` and `st_value` come from the full link's
///   `SymtabPlan::resolve` (S-81, S-82, S-122 to S-124) over this update's
///   placement: a section's place (allocated: its chunk's address; not
///   allocated: the chunk's offset in its output section, as the full
///   link's image gives it), a local of an input `.eh_frame` by S-50
///   (`eh_ref`, its file's records as the update places them, dropped
///   when S-50 drops it), a local of a split merge section by S-19 (its
///   piece's fragment), a linker-defined global by the placement's
///   `defined`.
/// - The global area: `Removed` and `Demoted` globals leave it, each
///   taken by the area's last entry ([`area_after_removals`]; the moves
///   are [`SymtabPlan::moves`], copies from the previous output; their
///   keys' `SYMTAB_KEYS` rows are [`moved_keys`]', which the caller
///   appends to [`SymtabPlan::globals`]); a `Changed` global is rewritten
///   at its index (or where a move puts it), its name where it is; `Added`
///   and `Promoted` globals are appended, in key order, their names at
///   `.strtab`'s end. [`SymtabPlan::globals`] has a row for each global
///   whose index or name offset changed, a removed or demoted one with
///   [`NO_ENTRY`] and 0.
/// - Every growth step makes the rest of both areas a [`remap`] range
///   (a moved entry's new index included: U-6 copies the moves, then
///   remaps).
///
/// Not built (v1, `update-not-implemented (symtab)`, by name): a local
/// whose section the update does not place (dead, not kept; the join
/// drops a dead section's locals, S-76); a local of an `.eh_frame` whose
/// file's records this update does not place, or whose S-50 value is the
/// next file's record, or whose fate depends on a relocation (rule 4); a
/// local of a split section whose fragment has no row (`.debug_*`, WP-239
/// R-1) or no live user; a global in an `.eh_frame` or a split section; a
/// common.
///
/// [`remap`]: SymtabPlan::remap
///
/// # Errors
/// `symtab-exhausted` (the tail reserve, `.symtab`'s or `.strtab`'s file
/// slack), `state-corrupt` (stored rows that contradict each other, a
/// chunk the placement does not have, a `SYMTAB_KEYS` row missing or
/// outside the global area), `update-not-implemented (symtab)` (above).
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
    let old_end =
        u32::try_from(entries).map_err(|_| corrupt("SYMTAB: a global area past 2^32 entries"))?;
    let mut keys: Vec<SymbolId> = globals.iter().map(|g| g.key).collect();
    keys.sort_unstable();
    if keys.windows(2).any(|w| w[0] == w[1]) {
        debug_assert!(false, "a global given twice");
        return Err(not_built());
    }
    let (removed, changed) = stored_rows(globals, area.first, old_end)?;
    let (kept_end, mut moves) = area_after_removals(area.first, old_end, &removed);
    let mut order: Vec<&FileSymbols<'_>> = files.iter().collect();
    order.sort_by_key(|f| f.file);
    if order.windows(2).any(|w| w[0].file == w[1].file) {
        debug_assert!(false, "a file given twice");
        return Err(not_built());
    }
    let mut cx = Cx::new(stored, layout);
    let mut out = SymtabPlan::default();
    let mut tail = (tables.tail_first, tables.tail_capacity);
    let str_limit = tables.strtab_size.saturating_add(tables.strtab_slack);
    let mut str_end = tables.strtab_size;
    for fs in order {
        rewrite_file(&mut cx, fs, &mut out, &mut tail, &mut str_end, str_limit)?;
    }
    // The global area: removed and demoted entries left it; changed ones
    // in place (or where the move of their entry puts them).
    let mut written = write_changed(&mut cx, changed, &mut moves, &mut out)?;
    for g in globals {
        if matches!(g.kind, GlobalKind::Removed | GlobalKind::Demoted)
            && g.stored.is_some_and(|s| s.0 != NO_ENTRY)
        {
            out.globals.push((g.key, NO_ENTRY, 0));
        }
    }
    out.moves = moves;
    // Appended globals, in key order.
    let mut appended: Vec<&GlobalChange<'_>> = globals
        .iter()
        .filter(|g| matches!(g.kind, GlobalKind::Added | GlobalKind::Promoted))
        .collect();
    appended.sort_by_key(|g| g.key);
    let mut end = kept_end;
    let names_at = str_end;
    let buf = append_globals(&mut cx, &appended, &mut out, &mut end, names_at)?;
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
    out.globals.sort_unstable();
    // IN-49a: a growth step moves sections of files this update does not
    // read; their entries' values are remapped through the placement.
    if !layout.growth_steps.is_empty() {
        written.extend(out.blocks.iter().map(|(_, b)| (b.first, b.first + b.capacity)));
        written.extend(out.fillers.iter().copied());
        written.push((kept_end, end));
        out.remap = complement(merged(written), 1, end);
    }
    Ok(out)
}

/// A changed global with its stored index and `.strtab` offset.
type Changed<'g, 'a> = (&'g GlobalChange<'a>, u32, u32);

/// WP-217 ruling 1: the stored `SYMTAB_KEYS` rows of `globals` checked
/// against the global area `[first, old_end)`: the indices removed and
/// demoted globals leave, and the changed globals with theirs.
fn stored_rows<'g, 'a>(
    globals: &'g [GlobalChange<'a>],
    first: u32,
    old_end: u32,
) -> Result<(Vec<u32>, Vec<Changed<'g, 'a>>), IncrReason> {
    let mut removed = Vec::new();
    let mut changed: Vec<Changed<'g, 'a>> = Vec::new();
    for g in globals {
        if !matches!(g.kind, GlobalKind::Removed | GlobalKind::Demoted)
            && g.place == GlobalPlace::Common
        {
            return Err(not_built());
        }
        match (g.kind, g.stored) {
            (GlobalKind::Added | GlobalKind::Promoted, None)
            | (GlobalKind::Removed | GlobalKind::Demoted, Some((NO_ENTRY, _))) => {}
            (GlobalKind::Added | GlobalKind::Promoted, Some(_)) => {
                debug_assert!(false, "an appended global with a stored row");
                return Err(not_built());
            }
            (_, None) => {
                return Err(corrupt(format!("SYMTAB_KEYS: key {} has no row", g.key.0)));
            }
            (GlobalKind::Changed, Some((NO_ENTRY, _))) => {
                return Err(corrupt(format!(
                    "SYMTAB_KEYS: key {} is changed and has no .symtab entry",
                    g.key.0
                )));
            }
            (_, Some((i, _))) if i < first || i >= old_end => {
                return Err(corrupt(format!(
                    "SYMTAB_KEYS: key {}'s index {i} is outside the global area [{}, {old_end})",
                    g.key.0, first
                )));
            }
            (GlobalKind::Changed, Some((i, name))) => changed.push((g, i, name)),
            (_, Some((i, _))) => removed.push(i),
        }
    }
    let mut taken: Vec<u32> = removed.iter().copied().chain(changed.iter().map(|c| c.1)).collect();
    taken.sort_unstable();
    if taken.windows(2).any(|w| w[0] == w[1]) {
        return Err(corrupt("SYMTAB_KEYS: two keys have one .symtab index"));
    }
    Ok((removed, changed))
}

/// The changed globals' entries (at their index, or at the `to` of the
/// move of their entry, which is then dropped from `moves`); the index
/// ranges written.
fn write_changed(
    cx: &mut Cx<'_, '_>,
    mut changed: Vec<Changed<'_, '_>>,
    moves: &mut Vec<(u32, u32)>,
    out: &mut SymtabPlan,
) -> Result<Vec<(u32, u32)>, IncrReason> {
    changed.sort_by_key(|c| c.1);
    let mut written: Vec<(u32, u32)> = Vec::new();
    for (g, i, name) in changed {
        let to = match moves.iter().position(|m| m.0 == i) {
            Some(k) => moves.remove(k).1,
            None => i,
        };
        let at = cx.global_where(g)?;
        let (shndx, value) = cx.resolve(g.name, g.info, at)?;
        out.entries.push(SymtabEntry {
            index: to,
            name,
            info: g.info,
            other: g.other,
            shndx,
            value,
            size: g.size,
        });
        written.push((to, to + 1));
        if to != i {
            out.globals.push((g.key, to, name));
        }
    }
    Ok(written)
}

/// The `SYMTAB_KEYS` rows of the entries [`plan`] moved (WP-217 ruling 2;
/// review-in77 M-6): for each move `(from, to)` of `moves`, the previous
/// output's entry at `from` as the caller read it, `(from, st_name, name
/// bytes)` in `read` (U-6 reads it for the copy), the key `find_key` gives
/// for the name, which must have the stored row `symtab_key` gives, exactly
/// `(from, st_name)`; the row `(key, to, st_name)` to append to
/// [`SymtabPlan::globals`]. No reverse table is stored and none is scanned:
/// one name lookup and one row read a move.
///
/// # Errors
/// `update-not-implemented (symtab)` when the name does not identify the
/// entry (no key by that name: a versioned pair, a name no key has; a key
/// whose stored row is another), decided before the first write; what
/// `find_key` and `symtab_key` return; `state-corrupt` when `read` has no
/// entry at a move's `from`.
pub fn moved_keys(
    moves: &[(u32, u32)],
    read: &[(u32, u32, &[u8])],
    mut find_key: impl FnMut(&[u8]) -> Result<Option<SymbolId>, IncrReason>,
    mut symtab_key: impl FnMut(SymbolId) -> Result<(u32, u32), IncrReason>,
) -> Result<Vec<(SymbolId, u32, u32)>, IncrReason> {
    let mut rows = Vec::with_capacity(moves.len());
    for &(from, to) in moves {
        let Some(&(_, st_name, name)) = read.iter().find(|e| e.0 == from) else {
            return Err(corrupt(format!(".symtab entry {from}: not read for its move")));
        };
        let Some(key) = find_key(name)? else { return Err(not_built()) };
        if symtab_key(key)? != (from, st_name) {
            return Err(not_built());
        }
        rows.push((key, to, st_name));
    }
    Ok(rows)
}

/// One file's block of an update (IN-49a): in place or in the tail
/// reserve, its names in its region or a new one at `.strtab`'s end.
fn rewrite_file(
    cx: &mut Cx<'_, '_>,
    fs: &FileSymbols<'_>,
    out: &mut SymtabPlan,
    tail: &mut (u32, u32),
    str_end: &mut u64,
    str_limit: u64,
) -> Result<(), IncrReason> {
    let old = cx.stored.symtab_block(fs.file)?;
    let mut names = Vec::new();
    let mut rows = Vec::with_capacity(fs.locals.len());
    for l in &fs.locals {
        let Some(at) = cx.local_where(fs, l)? else { continue };
        let (shndx, value) = cx.resolve(l.name, l.info, at)?;
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
        None if n == 0 => return Ok(()),
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
            *tail = (tail.0 + cap, tail.1 - cap);
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
            let at = *str_end;
            *str_end = at.saturating_add(cap);
            if *str_end > str_limit {
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
    Ok(())
}

/// The appended globals' entries (at `*end` on), in the order given; their
/// names' bytes, at `names_at` in `.strtab`.
fn append_globals(
    cx: &mut Cx<'_, '_>,
    appended: &[&GlobalChange<'_>],
    out: &mut SymtabPlan,
    end: &mut u32,
    names_at: u64,
) -> Result<Vec<u8>, IncrReason> {
    let mut buf: Vec<u8> = Vec::new();
    for g in appended {
        let at = cx.global_where(g)?;
        let (shndx, value) = cx.resolve(g.name, g.info, at)?;
        let name = if let Some(o) = fixed_name(g.name) {
            o
        } else {
            let at = names_at + buf.len() as u64;
            buf.extend_from_slice(g.name);
            buf.push(0);
            st_name(at)?
        };
        out.entries.push(SymtabEntry {
            index: *end,
            name,
            info: g.info,
            other: g.other,
            shndx,
            value,
            size: g.size,
        });
        out.globals.push((g.key, *end, name));
        *end = end.checked_add(1).ok_or_else(|| exhausted(".symtab's entries"))?;
    }
    Ok(buf)
}

/// A name of a planned entry: in the fixed table (S-86), or at a byte
/// offset in its block's names.
#[derive(Debug, Clone, Copy)]
enum Name {
    Fixed(u32),
    Region(u64),
}

/// S-50's "after" when the next kept record is not the file's: no value of
/// an update (a symbol that takes it is refused).
const AFTER_THE_FILE: u64 = u64::MAX;

/// A fragment's key: output section, class word, hash (`FRAGS`).
type FragKey = (OutSecId, u32, [u8; 16]);

/// What [`plan`] reads of the placement, each row once.
struct Cx<'s, 'a> {
    stored: &'s StoredLayout<'a>,
    layout: &'s LayoutDelta,
    places: HashMap<SectionId, PlaceNow>,
    /// `LayoutDelta::eh`'s range of a file, by file (built on first need).
    eh_ranges: Option<HashMap<FileId, &'s EhRange>>,
    /// `LayoutDelta::frags` by key (built on first need).
    new_frags: Option<HashMap<FragKey, StoredFrag>>,
    stored_frags: HashMap<FragKey, Option<StoredFrag>>,
    addrs: Addrs<'s>,
}

impl<'s, 'a> Cx<'s, 'a> {
    fn new(stored: &'s StoredLayout<'a>, layout: &'s LayoutDelta) -> Self {
        Self {
            stored,
            layout,
            places: HashMap::new(),
            eh_ranges: None,
            new_frags: None,
            stored_frags: HashMap::new(),
            addrs: Addrs {
                chunks: &layout.placement.chunks,
                defined: &layout.placement.defined,
                tls: layout.placement.tls.map(|t| t.start),
                out: None,
                bases: HashMap::new(),
                merged: HashMap::new(),
                eh: None,
            },
        }
    }

    /// `(st_shndx, st_value)` of an entry of kind `at` (prepared by
    /// [`Cx::local_where`] or [`Cx::global_where`]): the full link's
    /// `SymtabPlan::resolve` over the update's placement.
    fn resolve(&mut self, name: &[u8], info: u8, at: Where) -> Result<(u32, u64), IncrReason> {
        if !matches!(at, Where::Absolute(_) | Where::Undefined) {
            self.addrs.out_sections();
        }
        // Every place was checked when prepared: what `resolve` refuses
        // here is S-122, a TLS symbol without a template, or a linker
        // symbol without a value: the full link would stop on it too.
        resolve_at(&self.addrs, name, info, at).map_err(|_| not_built())
    }

    /// A local's kind (S-76, S-50, S-19), or `None` when S-50 drops it.
    fn local_where(
        &mut self,
        fs: &FileSymbols<'_>,
        l: &LocalSymbol<'_>,
    ) -> Result<Option<Where>, IncrReason> {
        let Some(id) = l.section else { return Ok(Some(Where::Absolute(l.value))) };
        if let Some(eh) = fs.eh
            && let Some(s) = eh.frames.sections.iter().position(|x| x.section == id)
        {
            return self.eh_where(fs.file, eh, s, l);
        }
        let lo = fs.pieces.partition_point(|p| p.section < id);
        let hi = fs.pieces.partition_point(|p| p.section <= id);
        if lo < hi {
            return self.merged_where(id, &fs.pieces[lo..hi], l.value).map(Some);
        }
        self.placed(id)?;
        Ok(Some(Where::Section { section: id, value: l.value }))
    }

    /// A global's kind (S-81, layout §13).
    fn global_where(&mut self, g: &GlobalChange<'_>) -> Result<Where, IncrReason> {
        Ok(match g.place {
            GlobalPlace::Section(id) => {
                self.placed(id)?;
                Where::Section { section: id, value: g.value }
            }
            GlobalPlace::Absolute => Where::Absolute(g.value),
            GlobalPlace::Undefined => Where::Undefined,
            GlobalPlace::Linker => Where::Internal(g.key),
            GlobalPlace::Common => return Err(not_built()),
        })
    }

    /// Records the place of a section placed whole (S-81): its chunk and
    /// offset after the update.
    fn placed(&mut self, id: SectionId) -> Result<(), IncrReason> {
        if self.addrs.bases.contains_key(&id) {
            return Ok(());
        }
        let place = if let Some(p) = self.places.get(&id) {
            *p
        } else {
            let p = self.layout.place_of(self.stored, id)?;
            self.places.insert(id, p);
            p
        };
        // Placed nowhere (dead, not kept; a split section has pieces):
        // v1 does not plan it.
        let (PlaceNow::Here(p) | PlaceNow::Stored(p)) = place else { return Err(not_built()) };
        let Some((chunk, offset)) = p.get() else { return Err(not_built()) };
        let Some(c) = self.layout.placement.chunks.get(chunk) else {
            return Err(corrupt(format!(
                "SECTIONS: section {} in chunk {}, which is none",
                id.0, chunk.0
            )));
        };
        if c.shndx.is_none() {
            return Err(corrupt(format!("chunk {} has no section header", chunk.0)));
        }
        self.addrs.bases.insert(id, (chunk, offset));
        Ok(())
    }

    /// S-50 for a local of input `.eh_frame` section row `s` of the file's
    /// frames: `mortar_merge::refs::eh_ref`, the full link's function
    /// (`EhPlan::value`), with the kept records' offsets this update gives
    /// them (`LayoutDelta::eh`).
    fn eh_where(
        &mut self,
        file: FileId,
        eh: FileEh<'_>,
        row: usize,
        local: &LocalSymbol<'_>,
    ) -> Result<Option<Where>, IncrReason> {
        let layout = self.layout;
        // The file's records are not placed by this update.
        let Some(layout_eh) = &layout.eh else { return Err(not_built()) };
        let ranges = self.eh_ranges.get_or_insert_with(|| {
            layout_eh.ranges.iter().map(|r| (r.file, r)).collect::<HashMap<_, _>>()
        });
        let Some(range) = ranges.get(&file).copied() else { return Err(not_built()) };
        if range.records.len() != eh.kept.len() {
            return Err(corrupt(format!(
                ".eh_frame: file {}'s range has {} records, its file {}",
                file.0,
                range.records.len(),
                eh.kept.len()
            )));
        }
        let sec = &eh.frames.sections[row];
        let first_record = sec.records.start;
        let kept = |global: u32| {
            let piece = global.checked_sub(first_record)?;
            let at =
                eh.kept.iter().position(|rec| rec.section == sec.section && rec.piece == piece)?;
            range.records.get(at).copied()
        };
        // S-50 rule 3's "next kept record after E's records in output
        // order": the file's next one, when E has a kept record and a
        // later section of the file has one; else not this update's to give.
        let after = eh
            .kept
            .iter()
            .rposition(|rec| rec.section == sec.section)
            .and_then(|at| range.records.get(at + 1).copied())
            .unwrap_or(AFTER_THE_FILE);
        let Ok(offset) = i64::try_from(local.value) else { return Err(not_built()) };
        let value = |who| eh_ref(eh.frames, row, offset, who, kept, after).map_err(|_| not_built());
        let got = if is_mapping(local.name) {
            value(EhWho::Mapping)?
        } else {
            // S-50 rule 4 drops a local no relocation refers to; the join
            // does not say: refused when that decides it.
            let dropped = value(EhWho::Symbol { droppable: true })?;
            if dropped != value(EhWho::Symbol { droppable: false })? {
                return Err(not_built());
            }
            dropped
        };
        let at = match got {
            EhValue::Dropped => return Ok(None),
            EhValue::Offset(AFTER_THE_FILE) => return Err(not_built()),
            EhValue::Offset(at) => at,
        };
        let Some(chunk) = layout.placement.chunks.get(layout_eh.chunk) else {
            return Err(corrupt(format!(".eh_frame: chunk {} is none", layout_eh.chunk.0)));
        };
        let (Some(shndx), Some(addr)) = (chunk.shndx, chunk.addr) else {
            return Err(corrupt(format!(".eh_frame: chunk {} has no address", layout_eh.chunk.0)));
        };
        self.addrs.eh = Some((shndx, addr));
        Ok(Some(Where::Eh(at)))
    }

    /// S-19 for a named local at `v` of a split merge section, `pieces`
    /// its pieces (`(section, offset)` order): the piece holding byte `v`
    /// (the last when `v` is the section's size, as
    /// `mortar_merge::refs::named_symbol`), its fragment's place.
    fn merged_where(
        &mut self,
        id: SectionId,
        pieces: &[MergedPiece],
        v: u64,
    ) -> Result<Where, IncrReason> {
        let Some(last) = pieces.last() else { return Err(not_built()) };
        let size = last.offset.saturating_add(last.size);
        let p = if v == size {
            last
        } else {
            let k = pieces.partition_point(|p| p.offset <= v);
            match k.checked_sub(1).map(|k| &pieces[k]) {
                Some(p) if v < p.offset.saturating_add(p.size) => p,
                // Outside the section (the full link refuses it) or in no
                // piece.
                _ => return Err(not_built()),
            }
        };
        let within = v - p.offset;
        let key = (p.out, p.class.word(), p.hash);
        let new = self.new_frags.get_or_insert_with(|| {
            self.layout.frags.iter().map(|f| ((f.out, f.class, f.hash), *f)).collect()
        });
        let frag = if let Some(f) = new.get(&key) {
            Some(*f)
        } else if let Some(f) = self.stored_frags.get(&key) {
            *f
        } else {
            let f = self.stored.frags_of(key.0, key.1, key.2)?;
            self.stored_frags.insert(key, f);
            f
        };
        self.addrs.out_sections();
        let Some(chunk) = self.addrs.merged_chunk(p.out) else {
            return Err(corrupt(format!("output section {} has no merged chunk", p.out.0)));
        };
        let Some(frag) = frag else {
            // A `.debug_*` split section's fragments have no row (WP-239
            // R-1): v1 does not plan its locals.
            if self.addrs.chunks.get(chunk).is_some_and(|c| c.addr.is_none()) {
                return Err(not_built());
            }
            return Err(corrupt(format!(
                "FRAGS: no row for a piece of section {} (output section {})",
                id.0, p.out.0
            )));
        };
        if frag.refs == 0 {
            return Err(not_built());
        }
        let Some(at) = frag.offset.checked_add(within).and_then(|o| self.addrs.chunk_at(chunk, o))
        else {
            return Err(corrupt(format!("FRAGS: a fragment of chunk {} has no place", chunk.0)));
        };
        self.addrs.merged.insert((id, v), at);
        Ok(Where::Section { section: id, value: v })
    }
}

/// An output section as the full link's image derives it from the chunks
/// (`mortar_image::link`'s `out_sections`, L-86): its first chunk's
/// address (0 when not allocated) and file offset, the size to the end of
/// its last chunk's `size`.
#[derive(Debug, Clone, Copy, Default)]
struct OutRow {
    addr: u64,
    offset: u64,
    size: u64,
}

/// The output sections by `.symtab` section index, and each output
/// section's merged chunk.
type OutSections = (Vec<Option<OutRow>>, HashMap<OutSecId, ChunkId>);

/// [`SymAddrs`] over an update's placement: the answers the full link's
/// image gives (`mortar_image::link`'s `LinkTables`), for the entries
/// [`Cx`] prepared.
struct Addrs<'s> {
    chunks: &'s Column<ChunkId, Chunk>,
    defined: &'s [Defined],
    tls: Option<u64>,
    /// The output sections by index, and the merged chunks by output
    /// section (one pass over the chunks, on first need).
    out: Option<OutSections>,
    /// A section placed whole: its chunk and offset.
    bases: HashMap<SectionId, (ChunkId, u64)>,
    /// A local of a split section at `(section, value)`: its place.
    merged: HashMap<(SectionId, u64), (u32, u64)>,
    /// The `.eh_frame` chunk's section index and address.
    eh: Option<(u32, u64)>,
}

impl Addrs<'_> {
    /// Builds [`Addrs::out`] (once): `mortar_image::link`'s
    /// `out_sections`, the same fold over the chunks.
    fn out_sections(&mut self) {
        if self.out.is_some() {
            return;
        }
        let mut rows: Vec<Option<OutRow>> = Vec::new();
        let mut ends: Vec<u64> = Vec::new();
        let mut merged = HashMap::new();
        for (id, c) in self.chunks.iter() {
            if let ChunkKind::Merged(o) = c.kind {
                merged.entry(o).or_insert(id);
            }
            let Some(n) = c.shndx else { continue };
            let n = n as usize;
            if rows.len() <= n {
                rows.resize(n + 1, None);
                ends.resize(n + 1, 0);
            }
            let start = c.addr.unwrap_or(c.offset);
            let end = start.saturating_add(c.size);
            match &mut rows[n] {
                Some(s) => {
                    let first = if c.addr.is_some() { s.addr } else { s.offset };
                    if start < first {
                        s.addr = c.addr.unwrap_or(0);
                        s.offset = c.offset;
                    }
                    ends[n] = ends[n].max(end);
                    let first = if c.addr.is_some() { s.addr } else { s.offset };
                    s.size = ends[n] - first;
                }
                slot @ None => {
                    *slot =
                        Some(OutRow { addr: c.addr.unwrap_or(0), offset: c.offset, size: c.size });
                    ends[n] = end;
                }
            }
        }
        self.out = Some((rows, merged));
    }

    fn merged_chunk(&self, out: OutSecId) -> Option<ChunkId> {
        self.out.as_ref()?.1.get(&out).copied()
    }

    /// `mortar_image::link`'s `chunk_at`: the section index and value of
    /// `offset` bytes into chunk `c` (allocated: its address; not: its
    /// offset in its output section).
    fn chunk_at(&self, c: ChunkId, offset: u64) -> Option<(u32, u64)> {
        let chunk = self.chunks.get(c)?;
        let shndx = match chunk.kind {
            ChunkKind::FileHeader | ChunkKind::ProgramHeaders => 1,
            _ => chunk.shndx?,
        };
        let base = if let Some(a) = chunk.addr {
            a
        } else {
            let sec = (*self.out.as_ref()?.0.get(shndx as usize)?)?;
            chunk.offset.checked_sub(sec.offset)?
        };
        Some((shndx, base.checked_add(offset)?))
    }
}

impl SymAddrs for Addrs<'_> {
    fn section(&self, section: SectionId, value: u64) -> Option<(u32, u64)> {
        if let Some(at) = self.merged.get(&(section, value)) {
            return Some(*at);
        }
        let (c, at) = *self.bases.get(&section)?;
        self.chunk_at(c, at.checked_add(value)?)
    }

    fn eh_frame(&self) -> Option<(u32, u64)> {
        self.eh
    }

    fn common(&self, _: SymbolId) -> Option<(u32, u64)> {
        None
    }

    fn internal(&self, id: SymbolId) -> Option<InternalValue> {
        let at = self.defined.binary_search_by_key(&id, |row| row.symbol).ok()?;
        Some(match self.defined[at].value {
            DefValue::Absolute(v) => InternalValue::Absolute(v),
            DefValue::At { chunk, offset } => {
                let (shndx, addr) = self.chunk_at(chunk, offset)?;
                InternalValue::At { shndx, addr }
            }
        })
    }

    fn patch(&self, _: usize) -> Option<(u32, u64)> {
        None
    }

    fn tls_start(&self) -> Option<u64> {
        self.tls
    }

    fn section_range(&self, shndx: u32) -> Option<(u64, u64)> {
        let s = (*self.out.as_ref()?.0.get(shndx as usize)?)?;
        (shndx > 0).then_some((s.addr, s.size))
    }
}

/// The global area after removing the entries at `removed` (IN-49a: a
/// removed global's entry is taken by the area's last entry, one removal
/// after another), for an area of `[first, end)`: the new end, and the
/// moves `(from, to)` in previous-output indices, sorted by `to`. The
/// removals are applied from the highest index down, so every move reads
/// an entry the previous output has where it was; a removed entry that is
/// the last is dropped without a move.
/// [`plan`] calls it with the stored indices of its `Removed` and
/// `Demoted` globals (WP-217 ruling 1).
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
        if r == last {
            at.remove(&last);
        } else {
            let from = at.remove(&last).unwrap_or(last);
            at.insert(r, from);
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

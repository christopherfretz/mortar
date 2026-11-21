//! The delta layout (`spec/incr.md` IN-72 items 1-12, the growth step;
//! IN-70 step U7): the update's `Placement` from the stored one,
//! never a fresh one. Not `mortar_incr::delta` (the floor's refill
//! report). WP-213 (U-1): the entry point [`layout`]; filled by U-4
//! (WP-216). Its types are `mortar_core::incr`'s (`StoredLayout`,
//! `SectionChange`, `LayoutDelta`): this crate does not depend on
//! `mortar_incr`.
//!
//! What it reads (IN-18c, O(change)): the whole small tables (`CHUNKS`,
//! `ORDER`, `SEGMENTS`, `OUTSECS`, `PATCHES`, `REDIRECTS`, `SYM_PLACE`,
//! `DEFINED`, `LAYOUT`; `SLOTS` only when a file of `changes` has a new
//! version or an old or new slot demand, item 7, `slots`), and per file of
//! `changes` its `SECTIONS`, `REGIONS`, `FRAG_USE` and `KEY_REFS` ranges
//! and its `FILES` row's `.eh_frame` columns, the `FRAGS` row of each
//! fragment key whose count changes (item 6, `frags`), and `HOLES` of the
//! chunks a touched region, a new region or a dead or revived fragment is
//! in, or of an output section that lost a member (item 10), or of
//! `.eh_frame` when a file of `changes` or `input.eh` has records or a
//! range (item 5, `eh`). Nothing else.
//!
//! What it gives (`spec/landed/incr/WP-216.md` §2): `placement` has the
//! stored whole tables, updated (chunks grown or moved by the growth steps,
//! the segments derived again, the TLS template and the entry moved with
//! them; `defined` is the stored `DEFINED`, whole and unchanged: its
//! chunk-relative values follow their chunks, and a row whose value is a
//! chunk's end is not moved when the update grows that chunk, WP-242
//! finding G); the placement is SPARSE (WP-216 R-3), read through
//! `LayoutDelta::place_of` and `holes_of` only: `files` names the files
//! whose sections `placement.place` places, `hole_chunks` the chunks whose
//! holes `placement.holes` replaces; `sized` is empty (not a row
//! `LayoutRows` gives).
//!
//! Every refusal is a reason of `spec/incr.md` §7 by name; what this
//! package cannot do (IN-72 item 4's rewrite; item 5's whole rewrite,
//! `delta/eh.rs`; item 6's dead string, WP-242 finding C; item 7's
//! recounted, revived or retired PLT/IPLT slot, findings B and F) is
//! `update-not-implemented (layout)`, never a guess; item 7's new slot is
//! `reserve-exhausted` (finding D). Commons are not placed by an update in
//! v1 (WP-216 R-2: the join's `update-not-implemented (common)`).

use std::collections::{BTreeMap, BTreeSet};

use mortar_core::column::Column;
use mortar_core::id::{ChunkId, FileId, OutSecId, SectionId};
use mortar_core::incr::{
    GrowthStep, IncrReason, LayoutDelta, LayoutInput, SectionChange, StoredLayout, StoredRegion,
    StoredSection, Unimplemented,
};
use mortar_core::layout::{
    Chunk, ChunkKind, DefValue, Defined, Hole, HoleKind, LoadClass, Place, Placement,
    PlacementKind, Segment, SegmentRules, derive_segments,
};
use mortar_elf::Machine;
use mortar_elf::consts::{
    PT_GNU_STACK, PT_LOAD, SHF_ALLOC, SHF_EXCLUDE, SHF_EXECINSTR, SHF_LINK_ORDER, SHF_MERGE,
    SHF_TLS, SHF_WRITE, SHF_X86_64_LARGE, SHT_AARCH64_ATTRIBUTES, SHT_GROUP, SHT_NOBITS, SHT_NOTE,
    SHT_PROGBITS, SHT_REL, SHT_RELA, SHT_STRTAB, SHT_SYMTAB, SHT_SYMTAB_SHNDX, SHT_X86_64_UNWIND,
};

use crate::assign::{matches_prefix, output_name};
use crate::headers::Header;
use crate::order::text_category;
use crate::slack::{self, Slack};
use crate::target::Target;

mod eh;
mod frags;
mod slots;

/// Place the update's sections in the stored layout (WP-211 §C; IN-72).
/// `input.changes`: every section of every changed, appeared, drawn-in or
/// newly extracted file (sorted by id), plus the sections whose liveness
/// or keeping flips (the join builds them from U-3's results); per such
/// file, `input.eh`, `input.pieces` and `input.demands` (items 5, 6, 7).
/// Called by the join (IN-70 U7), after its `update-not-implemented
/// (common)` check (R-2).
///
/// A change whose `old` is its own id is a stored row standing for itself
/// (a drawn-in file's section, or a gone file's with `live` false); any
/// other change is a section of the new version of its file, and that
/// file's stored rows are its old version (`spec/landed/incr/WP-216.md`
/// §2).
///
/// # Errors
/// IN-72's reasons (`reserve-exhausted`, `island-exhausted`,
/// `new-output-section`, `property-changed`, `link-order-moved`,
/// `tls-layout-changed`, `state-corrupt`), and `update-not-implemented
/// (layout)` / `(debug)` for what U-4 does not place
/// (`spec/landed/incr/WP-216.md` §4).
pub fn layout(
    stored: &StoredLayout<'_>,
    input: &LayoutInput<'_>,
    target: &Target,
) -> Result<LayoutDelta, IncrReason> {
    let changes = input.changes;
    debug_assert!(changes.windows(2).all(|w| w[0].id < w[1].id), "changes sorted by id");
    let mut files: BTreeMap<FileId, Vec<usize>> = BTreeMap::new();
    for (i, c) in changes.iter().enumerate() {
        files.entry(c.file).or_default().push(i);
    }
    let ids: Vec<FileId> = files.keys().copied().collect();
    let base = Base::read(stored, target)?;
    // Item 5 (WP-243): its whole rewrite reads every `FILES` row with a
    // range (`eh_ranges`, seam 6), only when it runs.
    let all = || stored.eh_ranges();
    let eh = eh::item_5(stored, input.eh, &ids, &base.chunks, Some(&all))?;
    let mut u = Update::new(&base, changes, input.debug);
    for (&file, members) in &files {
        u.file(stored, file, members)?;
    }
    u.merged = frags::item_6(&base, &mut u.chunks, stored, input, &ids, &u.pieces)?;
    u.link_order()?;
    let file_of: Vec<slots::FileOf> = ids
        .iter()
        .map(|&file| slots::FileOf { file, changed: u.changed.contains(&file) })
        .collect();
    u.slots =
        slots::item_7(stored, &file_of, input.demands, &base.chunks, target.class, target.machine)?;
    u.overflow()?;
    u.growth()?;
    u.holes(stored)?;
    u.left_empty(stored)?;
    if let Some(o) = &eh {
        u.chunks[o.layout.chunk].size = o.size;
        if let Some((h, size)) = o.hdr {
            u.chunks[h].size = size;
        }
    }
    u.beside()?;
    let mut delta = u.finish()?;
    delta.files = files.into_keys().collect();
    if let Some(o) = eh {
        let c = o.layout.chunk;
        let Err(at) = delta.hole_chunks.binary_search(&c) else {
            debug_assert!(false, "the .eh_frame chunk is item 5's alone");
            return Err(not_implemented());
        };
        delta.hole_chunks.insert(at, c);
        let mut holes = std::mem::take(&mut delta.placement.holes).into_vec();
        holes.extend(o.holes);
        holes.sort_by_key(|h| (h.chunk, h.offset));
        delta.placement.holes = holes.into_boxed_slice();
        delta.holes.extend(o.made);
        delta.holes.sort_by_key(|h| (h.chunk, h.offset));
        delta.eh = Some(o.layout);
    }
    Ok(delta)
}

// ---- small helpers ----------------------------------------------------------

fn corrupt(what: impl Into<String>) -> IncrReason {
    IncrReason::StateCorrupt(what.into())
}

fn not_implemented() -> IncrReason {
    IncrReason::UpdateNotImplemented(Some(Unimplemented::Layout))
}

fn text(name: &[u8]) -> String {
    String::from_utf8_lossy(name).into_owned()
}

/// `x` rounded up to `a` (a power of two), or `None` past `u64`.
fn round_up(x: u64, a: u64) -> Option<u64> {
    x.checked_next_multiple_of(a.max(1))
}

/// A section's alignment in bytes, or `None` when it is not a power of
/// two (the reader gives one; anything else is refused, not guessed).
fn align_bytes(align: u64) -> Option<u64> {
    let a = align.max(1);
    a.is_power_of_two().then_some(a)
}

/// The `.text` category of a section (layout.md L-53, as the full link's
/// region key takes it: code only).
fn category(name: &[u8], flags: u64) -> u8 {
    if flags & SHF_EXECINSTR != 0 { text_category(name) } else { 0 }
}

/// Item 10: a section aligned past its chunk's alignment is admitted when
/// the chunk's stored address (and file offset) is already a multiple of
/// it, up to the page (the `PT_LOAD`'s alignment stays); the chunk's
/// alignment rises. Otherwise "an output section's alignment growing past
/// its stored address's": `reserve-exhausted`.
fn admit(
    chunks: &mut Column<ChunkId, Chunk>,
    c: ChunkId,
    align: u64,
    page: u64,
    name: &[u8],
) -> Result<(), IncrReason> {
    let ch = &mut chunks[c];
    if align <= 1u64 << ch.align.min(63) {
        return Ok(());
    }
    let at_addr = ch.addr.is_none_or(|a| a.is_multiple_of(align));
    let at_offset = !ch.in_file() || ch.offset.is_multiple_of(align);
    if at_addr && at_offset && align <= page {
        ch.align = align.trailing_zeros() as u8;
        return Ok(());
    }
    Err(IncrReason::ReserveExhausted(format!(
        "{}: alignment {align} past its output section's address",
        text(name)
    )))
}

// ---- the stored whole tables -------------------------------------------------

/// The whole tables, read once, and what is derived from them.
struct Base {
    machine: Machine,
    address_limit: u128,
    chunks: Column<ChunkId, Chunk>,
    order: Vec<ChunkId>,
    segments: Vec<Segment>,
    rules: SegmentRules,
    /// Output sections by name.
    outs: BTreeMap<Vec<u8>, Vec<OutSecId>>,
    names: BTreeMap<OutSecId, Vec<u8>>,
    /// Each output section's chunks, in order.
    of_out: BTreeMap<OutSecId, Vec<ChunkId>>,
    /// The `PT_LOAD` runs (layout.md L-65, as `derive_segments` cuts
    /// them): members in order.
    runs: Vec<Vec<ChunkId>>,
    /// Each chunk's position in `order`.
    at: Vec<usize>,
    /// `CHUNKS.members` (WP-255g), one per chunk; `None` when the stored
    /// layout gives no count.
    members: Option<Vec<u32>>,
    rest: Rest,
}

/// The whole tables `layout` passes through.
struct Rest {
    sym_place: Vec<mortar_core::layout::SymPlace>,
    defined: Vec<Defined>,
    patches: Vec<mortar_core::layout::Patch>,
    redirects: Vec<mortar_core::layout::Redirect>,
    tls: Option<mortar_core::layout::Tls>,
    entry: Option<u64>,
}

impl Base {
    fn read(stored: &StoredLayout<'_>, target: &Target) -> Result<Self, IncrReason> {
        let rows = stored.chunks()?;
        if rows.iter().enumerate().any(|(i, (id, _))| id.0 as usize != i) {
            return Err(corrupt("CHUNKS ids are not dense"));
        }
        let chunks: Column<ChunkId, Chunk> =
            Column::from_vec(rows.into_iter().map(|(_, c)| c).collect())
                .map_err(|_| corrupt("CHUNKS too large"))?;
        let order = stored.order()?;
        let n = chunks.len() as usize;
        let mut at = vec![usize::MAX; n];
        for (k, &c) in order.iter().enumerate() {
            match at.get_mut(c.0 as usize) {
                Some(slot) if *slot == usize::MAX => *slot = k,
                _ => return Err(corrupt("ORDER is not every chunk once")),
            }
        }
        if order.len() != n {
            return Err(corrupt("ORDER is not every chunk once"));
        }
        let segments = stored.segments()?;
        let rules = infer_rules(&chunks, &order, &segments, target)?;
        let mut outs: BTreeMap<Vec<u8>, Vec<OutSecId>> = BTreeMap::new();
        let mut names = BTreeMap::new();
        for (id, name) in stored.outsecs()? {
            outs.entry(name.clone()).or_default().push(id);
            names.insert(id, name);
        }
        let mut of_out: BTreeMap<OutSecId, Vec<ChunkId>> = BTreeMap::new();
        for &c in &order {
            if let Some(o) = chunks[c].kind.out() {
                of_out.entry(o).or_default().push(c);
            }
        }
        // PT_LOAD runs, as `derive_segments` cuts them.
        let mut runs: Vec<(LoadClass, Vec<ChunkId>)> = Vec::new();
        for &c in &order {
            let Some(class) = chunks[c].run_class(runs.last().map(|r| r.0)) else { continue };
            match runs.last_mut() {
                Some((last, members)) if *last == class => members.push(c),
                _ => runs.push((class, vec![c])),
            }
        }
        let members = match stored.chunk_members() {
            Ok(m) if m.len() == n => Some(m),
            Ok(m) => {
                return Err(corrupt(format!("CHUNKS.members: {} counts, {n} chunks", m.len())));
            }
            Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Layout))) => None,
            Err(e) => return Err(e),
        };
        let (tls, entry) = stored.tls_and_entry()?;
        let rest = Rest {
            sym_place: stored.sym_place()?,
            defined: stored.defined()?,
            patches: stored.patches()?,
            redirects: stored.redirects()?,
            tls,
            entry,
        };
        Ok(Self {
            machine: target.machine,
            address_limit: target.address_limit(),
            chunks,
            order,
            segments,
            rules,
            outs,
            names,
            of_out,
            runs: runs.into_iter().map(|(_, m)| m).collect(),
            at,
            members,
            rest,
        })
    }

    fn chunk(&self, c: ChunkId) -> Result<&Chunk, IncrReason> {
        self.chunks.get(c).ok_or_else(|| corrupt(format!("a place names chunk {}", c.0)))
    }

    /// The address of `(chunk, offset)` in the stored layout.
    fn addr(&self, c: ChunkId, offset: u64) -> Result<u64, IncrReason> {
        let ch = self.chunk(c)?;
        ch.addr
            .and_then(|a| a.checked_add(offset))
            .ok_or_else(|| corrupt(format!("chunk {} has no address", c.0)))
    }

    /// The output section an input section goes to (layout.md L-36, L-44
    /// to L-48), among the stored ones: by name, then (several of one
    /// name) the one whose flags agree.
    fn out_named(&self, name: &[u8], flags: u64) -> Option<OutSecId> {
        let ids = self.outs.get(name)?;
        let key = flags & (SHF_WRITE | SHF_EXECINSTR | SHF_TLS);
        ids.iter()
            .copied()
            .find(|o| {
                self.first_chunk(*o)
                    .is_some_and(|c| c.flags & (SHF_WRITE | SHF_EXECINSTR | SHF_TLS) == key)
            })
            .or_else(|| ids.first().copied())
    }

    fn first_chunk(&self, out: OutSecId) -> Option<&Chunk> {
        self.of_out.get(&out).and_then(|v| v.first()).and_then(|&c| self.chunks.get(c))
    }

    /// What the update does with a section of this shape (IN-72's items by
    /// kind). `placed_before`: its previous version had a place (a
    /// `SHF_MERGE` section placed whole, synth.md S-6). `debug`: the
    /// caller runs the debug placement after the layout
    /// ([`LayoutInput::debug`], IN-50); otherwise a debug section is
    /// refused.
    fn fate(&self, s: &Shape<'_>, placed_before: bool, debug: bool) -> Fate {
        let alloc = s.flags & SHF_ALLOC != 0;
        let name = s.name;
        // Never placed (resolve.md §10's kinds that are not placed, and
        // the sections that are consumed: L-33 to L-35).
        if s.index == 0
            || matches!(s.sh_type, SHT_SYMTAB | SHT_SYMTAB_SHNDX | SHT_REL | SHT_RELA | SHT_GROUP)
            || (s.sh_type == SHT_STRTAB && !alloc)
            || s.flags & SHF_EXCLUDE != 0
            || name == b".note.GNU-stack"
            || name == b".note.gnu.build-id"
            || name == b".note.gnu.property"
            || (0x6fff_4c00..=0x6fff_4cff).contains(&s.sh_type)
            || (self.machine == Machine::AArch64 && s.sh_type == SHT_AARCH64_ATTRIBUTES)
            || name.starts_with(b".gnu.lto_")
            || matches_prefix(name, b".gnu.warning")
        {
            return Fate::Ignore;
        }
        // Item 5: `.eh_frame` (IN-34a) is placed nowhere, as in the full
        // link: its records are the synthetic chunk's (`delta/eh.rs`).
        if name == b".eh_frame"
            || (self.machine == Machine::X86_64 && s.sh_type == SHT_X86_64_UNWIND)
        {
            return Fate::Ignore;
        }
        // Debug first: a merged `.debug_*` section has no fragments (R-1).
        if !alloc && (name.starts_with(b".debug_") || name.starts_with(b".zdebug")) {
            return if debug && !name.starts_with(b".zdebug") {
                Fate::Debug
            } else {
                Fate::Refuse(Refusal::Debug)
            };
        }
        // Item 6: merged pieces. A section of this shape is placed whole
        // when its previous version had a place (synth.md S-6); otherwise
        // its pieces are `frags`' (WP-242).
        if s.flags & SHF_MERGE != 0
            && s.sh_type == SHT_PROGBITS
            && s.flags & (SHF_WRITE | SHF_EXECINSTR | SHF_TLS) == 0
            && s.size > 0
            && !placed_before
        {
            return Fate::Pieces;
        }
        let oname: &[u8] = if self.machine == Machine::X86_64 && s.flags & SHF_X86_64_LARGE != 0 {
            match large_output(s) {
                Some(n) => n,
                None => return Fate::Refuse(Refusal::NoOutput),
            }
        } else {
            output_name(&Header {
                name,
                sh_type: s.sh_type,
                flags: s.flags,
                size: s.size,
                align: 0,
                link: None,
                entsize: 0,
            })
        };
        let out = self.out_named(oname, s.flags);
        let first = out.and_then(|o| self.first_chunk(o));
        // L-159: an empty allocated section whose output section has no
        // chunk (its live members are all empty) is placed beside a
        // neighbour chunk, as the full link places it (M2b2b-2).
        if first.is_none() && alloc && s.size == 0 {
            return Fate::Beside;
        }
        let (Some(out), Some(first)) = (out, first) else {
            return Fate::Refuse(Refusal::NoOutput);
        };
        let bits = SHF_ALLOC | SHF_WRITE | SHF_EXECINSTR | SHF_TLS;
        if s.flags & bits & !first.flags != 0
            || (s.sh_type != SHT_NOBITS && first.sh_type == SHT_NOBITS)
        {
            return Fate::Refuse(Refusal::Property);
        }
        if !alloc {
            return Fate::Fixed { out, why: Why::Packed };
        }
        if s.flags & SHF_TLS != 0 {
            return Fate::Fixed { out, why: Why::Tls };
        }
        if s.sh_type == SHT_NOTE {
            return Fate::Fixed { out, why: Why::Note };
        }
        if s.flags & SHF_LINK_ORDER != 0 {
            return Fate::Fixed { out, why: Why::LinkOrder };
        }
        match slack::class(oname, first.flags) {
            slack::Class::Tolerant => Fate::Region { out, cat: category(name, s.flags) },
            slack::Class::Packed | slack::Class::None => Fate::Fixed { out, why: Why::Packed },
        }
    }
}

/// `DEFINED` after the update (WP-245 step 4): a row's chunk-relative
/// value follows its chunk; a row at the END of a chunk whose size the
/// update changes (an overflow region or a merged append at its tail, a
/// whole `.eh_frame` or a resized `.eh_frame_hdr`) moves to the new end,
/// as every full link puts an end row (layout.md §13.1: `_end`,
/// `_edata`, `etext`, `__stop_*`, `*_array_end` are `At { chunk, offset:
/// size }`). A row the update cannot place by that rule is refused,
/// never kept: at or past the smaller of the two sizes and not the old
/// end (a `--defsym` offset past an end symbol), or at 0 of a chunk that
/// was empty (a start and an end are the same row).
fn end_rows(
    stored: &[Defined],
    old: &Column<ChunkId, Chunk>,
    new: &Column<ChunkId, Chunk>,
) -> Result<Vec<Defined>, IncrReason> {
    let mut out = stored.to_vec();
    for d in &mut out {
        let DefValue::At { chunk, offset } = d.value else { continue };
        let (Some(o), Some(n)) = (old.get(chunk), new.get(chunk)) else {
            return Err(corrupt("a DEFINED row of no chunk"));
        };
        if o.size == n.size || offset < o.size.min(n.size) {
            continue;
        }
        if offset != o.size || o.size == 0 {
            return Err(end_row_refused());
        }
        d.value = DefValue::At { chunk, offset: n.size };
    }
    Ok(out)
}

/// A `DEFINED` row in a resized chunk that is neither inside both sizes
/// nor at the old end (`end_rows`).
fn end_row_refused() -> IncrReason {
    not_implemented()
}

/// A growth step of the last chunk of a `PT_LOAD` (item 3) that cannot
/// end at the chunk's size: L-9 would put the segment's memory end in no
/// section (P3-1).
fn last_chunk_refused() -> IncrReason {
    not_implemented()
}

/// L-36's large output section by flags and type, or `None` where the full
/// link refuses the input.
fn large_output(s: &Shape<'_>) -> Option<&'static [u8]> {
    let typed = match s.sh_type {
        SHT_PROGBITS => true,
        SHT_NOBITS => s.flags & SHF_WRITE != 0 && s.flags & SHF_EXECINSTR == 0,
        _ => false,
    };
    if s.flags & SHF_TLS != 0 || s.flags & SHF_ALLOC == 0 || !typed {
        return None;
    }
    Some(if s.flags & SHF_EXECINSTR != 0 {
        b".ltext"
    } else if s.sh_type == SHT_NOBITS {
        b".lbss"
    } else if s.flags & SHF_WRITE != 0 {
        b".ldata"
    } else {
        b".lrodata"
    })
}

/// The segment rules the stored segments were derived with: the class of
/// the target, `PT_GNU_STACK`'s flags, PIE when the file header is at 0,
/// and the first maximum page size (the target's, then each `PT_LOAD`'s
/// alignment) that derives exactly the stored segments.
fn infer_rules(
    chunks: &Column<ChunkId, Chunk>,
    order: &[ChunkId],
    segments: &[Segment],
    target: &Target,
) -> Result<SegmentRules, IncrReason> {
    let stack = segments
        .iter()
        .find(|s| s.p_type == PT_GNU_STACK)
        .map(|s| s.flags)
        .ok_or_else(|| corrupt("SEGMENTS has no PT_GNU_STACK"))?;
    let pie = chunks.iter().any(|(_, c)| c.kind == ChunkKind::FileHeader && c.addr == Some(0));
    let mut pages = vec![target.max_page];
    for s in segments.iter().filter(|s| s.p_type == PT_LOAD) {
        let p = 1u64 << s.align.min(63);
        if !pages.contains(&p) {
            pages.push(p);
        }
    }
    for max_page in pages {
        let rules = SegmentRules { class: target.class, max_page, stack, pie };
        if derive_segments(chunks, order, &rules) == segments {
            return Ok(rules);
        }
    }
    Err(corrupt("SEGMENTS is not derived from CHUNKS and ORDER"))
}

// ---- what a section is to the update ---------------------------------------

/// The facts `Base::fate` reads.
struct Shape<'a> {
    index: u32,
    name: &'a [u8],
    sh_type: u32,
    flags: u64,
    size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Why {
    /// L-142, and a non-alloc section that is not debug: kept in place
    /// at its size, else item 4's rewrite (not done: no member index).
    Packed,
    /// Item 8.
    Tls,
    /// Item 10: an allocated note.
    Note,
    /// Item 4: `SHF_LINK_ORDER`.
    LinkOrder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    Debug,
    NoOutput,
    Property,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fate {
    /// No place in any link.
    Ignore,
    /// A hole-tolerant output section's `(O, c)` (IN-30): item 2.
    Region { out: OutSecId, cat: u8 },
    /// Kept at its place and size, or refused by `why`.
    Fixed { out: OutSecId, why: Why },
    /// Merged pieces (item 6, `frags`): the section is not placed.
    Pieces,
    /// A `.debug_*` section left to the debug placement (IN-50; only when
    /// [`LayoutInput::debug`] says it runs): neither placed nor refused
    /// here.
    Debug,
    /// Placed by a part of IN-72 the update cannot do.
    Refuse(Refusal),
    /// L-159: an empty section of an output section with no chunk, placed
    /// at its previous place (the end, or the start, of the neighbour
    /// chunk L-159 chose), which follows that chunk's end ([`Update::beside`]).
    Beside,
}

impl Refusal {
    fn reason(self, name: &[u8]) -> IncrReason {
        match self {
            Self::Debug => IncrReason::UpdateNotImplemented(Some(Unimplemented::Debug)),
            Self::NoOutput => IncrReason::NewOutputSection(text(name)),
            Self::Property => IncrReason::PropertyChanged(format!("{}: flags or type", text(name))),
        }
    }
}

impl Why {
    fn reason(self, name: &[u8], what: &str) -> IncrReason {
        let detail = format!("{} {what}", text(name));
        match self {
            Self::Packed => not_implemented(),
            Self::Tls => IncrReason::TlsLayoutChanged(detail),
            Self::Note => IncrReason::PropertyChanged(detail),
            Self::LinkOrder => IncrReason::LinkOrderMoved(detail),
        }
    }
}

// ---- the update --------------------------------------------------------------

/// One region in absolute addresses.
#[derive(Debug, Clone, Copy)]
struct Reg {
    row: StoredRegion,
    start: u64,
    end: u64,
}

/// One file's `(O, c)` with what the update placed in it.
#[derive(Debug, Default)]
struct Group {
    regions: Vec<Reg>,
    /// Occupied `[start, end)` by start (non-empty ranges only).
    occupied: BTreeMap<u64, u64>,
    /// Sections phase 2 could not place, in input order.
    overflow: Vec<usize>,
}

impl Group {
    /// The end of the range that `[a, e)` overlaps, if any.
    fn clash(&self, a: u64, e: u64) -> Option<u64> {
        if a == e {
            // An empty section is free anywhere but strictly inside
            // another.
            return self
                .occupied
                .range(..a)
                .next_back()
                .filter(|&(_, &end)| end > a)
                .map(|(_, &x)| x);
        }
        self.occupied.range(..e).next_back().filter(|&(_, &end)| end > a).map(|(_, &x)| x)
    }

    fn take(&mut self, a: u64, e: u64) {
        if e > a {
            self.occupied.insert(a, e);
        }
    }

    /// Phase 2 (ii): the lowest aligned address in the regions, in their
    /// order, where `size` fits.
    fn first_fit(&self, size: u64, align: u64) -> Option<u64> {
        for r in &self.regions {
            let mut a = round_up(r.start, align)?;
            loop {
                let e = a.checked_add(size)?;
                if e > r.end {
                    break;
                }
                match self.clash(a, e) {
                    None => return Some(a),
                    Some(end) => a = round_up(end.max(a + 1), align)?,
                }
            }
        }
        None
    }
}

/// A touched chunk: its touched regions' ranges and its new holes.
type Touched = (Vec<(u64, u64)>, Vec<Hole>);

/// The update's working state.
struct Update<'b, 'c> {
    base: &'b Base,
    changes: &'c [SectionChange<'c>],
    /// [`LayoutInput::debug`]: debug sections are `Fate::Debug`.
    debug: bool,
    chunks: Column<ChunkId, Chunk>,
    /// Per change: its previous version's row (its `old`, or itself).
    prev: Vec<Option<StoredSection>>,
    /// Per change: its new place.
    place: Vec<Option<(ChunkId, u64)>>,
    /// Per change: its fate (`None`: not placed now).
    fate: Vec<Fate>,
    /// Old rows of changed files no change matched.
    orphans: Vec<StoredSection>,
    /// Files with a new version.
    changed: BTreeSet<FileId>,
    groups: BTreeMap<(FileId, OutSecId, u8), Group>,
    /// A file's regions, every `(O, c)`, as read (for the next ordinal).
    opened: Vec<StoredRegion>,
    /// Holes this update adds outside a region (the padding before an
    /// overflow region).
    pads: Vec<Hole>,
    steps: Vec<(u64, GrowthStep)>,
    shift: Vec<u64>,
    /// Every hole of the touched chunks, and those this update made.
    holes: Vec<Hole>,
    made: Vec<Hole>,
    /// The touched chunks (`holes` replaces their stored holes), sorted.
    hole_chunks: Vec<ChunkId>,
    /// The live, kept changes whose pieces are item 6's.
    pieces: BTreeSet<SectionId>,
    /// Item 6's result.
    merged: frags::Merged,
    /// Item 7's lists and `DeadSlot` holes (`slots`).
    slots: slots::Out,
}

impl<'b, 'c> Update<'b, 'c> {
    fn new(base: &'b Base, changes: &'c [SectionChange<'c>], debug: bool) -> Self {
        let n = changes.len();
        Self {
            base,
            changes,
            debug,
            chunks: base.chunks.clone(),
            prev: vec![None; n],
            place: vec![None; n],
            fate: vec![Fate::Ignore; n],
            orphans: Vec::new(),
            changed: BTreeSet::new(),
            groups: BTreeMap::new(),
            opened: Vec::new(),
            pads: Vec::new(),
            steps: Vec::new(),
            shift: vec![0; base.chunks.len() as usize],
            holes: Vec::new(),
            made: Vec::new(),
            hole_chunks: Vec::new(),
            pieces: BTreeSet::new(),
            merged: frags::Merged::default(),
            slots: slots::Out::default(),
        }
    }

    /// Items 1, 2 (keep and place), 4, 8, 10 for one file.
    #[allow(clippy::too_many_lines, reason = "the phases of item 2 in order")]
    fn file(
        &mut self,
        stored: &StoredLayout<'_>,
        file: FileId,
        members: &[usize],
    ) -> Result<(), IncrReason> {
        let base = self.base;
        let rows = stored.sections_of(file)?;
        let by_id: BTreeMap<SectionId, usize> =
            rows.iter().enumerate().map(|(k, r)| (r.id, k)).collect();
        if by_id.len() != rows.len() || rows.iter().any(|r| r.file != file) {
            return Err(corrupt(format!("SECTIONS range of file {}", file.0)));
        }
        let mut matched = vec![false; rows.len()];
        let mut is_changed = false;
        for &i in members {
            let c = &self.changes[i];
            if c.old != Some(c.id) {
                is_changed = true;
            }
            if let Some(o) = c.old {
                let Some(&k) = by_id.get(&o) else {
                    return Err(corrupt(format!(
                        "section {} of file {} matches row {} of another file",
                        c.id.0, file.0, o.0
                    )));
                };
                if std::mem::replace(&mut matched[k], true) {
                    return Err(corrupt(format!("row {} matched twice", o.0)));
                }
                self.prev[i] = Some(rows[k].clone());
            }
        }
        if !is_changed && matched.iter().any(|m| !m) {
            // The join gives every section of a drawn-in file (`layout`'s
            // contract); a region of a file given in part would be filled
            // over a live section.
            debug_assert!(false, "drawn-in file {} given in part", file.0);
            return Err(not_implemented());
        }
        if is_changed {
            self.changed.insert(file);
            for (k, r) in rows.iter().enumerate() {
                if !matched[k] && r.place.get().is_some() {
                    self.orphans.push(r.clone());
                }
            }
        }
        // The regions of the file, by (O, c), in ordinal order.
        let regions = stored.regions_of(file)?;
        for r in &regions {
            if r.file != file {
                return Err(corrupt(format!("REGIONS range of file {}", file.0)));
            }
            let start = base.addr(r.chunk, r.offset)?;
            let end = start
                .checked_add(r.capacity)
                .ok_or_else(|| corrupt(format!("region of file {} overflows", file.0)))?;
            match base.chunk(r.chunk)?.kind {
                ChunkKind::Body(p) if p.out == r.out => {}
                _ => return Err(corrupt(format!("a region of file {} not in a body", file.0))),
            }
            self.groups.entry((file, r.out, r.category)).or_default().regions.push(Reg {
                row: *r,
                start,
                end,
            });
        }
        self.opened.extend(regions.iter().copied());
        for g in self.groups.range_mut((file, OutSecId(0), 0)..=(file, OutSecId(u32::MAX), u8::MAX))
        {
            g.1.regions.sort_by_key(|r| r.row.ordinal);
        }

        // Fates; items 4, 8, 10 for the fixed ones.
        let mut by_group: BTreeMap<(OutSecId, u8), Vec<usize>> = BTreeMap::new();
        for &i in members {
            let c = self.changes[i];
            let prev = self.prev[i].as_ref();
            let placed_before = prev.is_some_and(|p| p.place.get().is_some());
            let shape = Shape {
                index: c.index,
                name: c.key.name,
                sh_type: c.sh_type,
                flags: c.sh_flags,
                size: c.size,
            };
            let fate = base.fate(&shape, placed_before, self.debug);
            let here = c.live && c.kept;
            if !here {
                continue;
            }
            if align_bytes(c.align).is_none() {
                return Err(not_implemented());
            }
            match fate {
                Fate::Ignore | Fate::Debug => {}
                Fate::Pieces => {
                    self.pieces.insert(c.id);
                }
                Fate::Refuse(r) => return Err(r.reason(c.key.name)),
                Fate::Fixed { out, why } => {
                    // Kept at its place and size, or refused.
                    let keep = prev.and_then(|p| {
                        let (ch, off) = p.place.get()?;
                        let chunk = base.chunks.get(ch)?;
                        let same = chunk.kind.out() == Some(out)
                            && p.size == c.size
                            && chunk
                                .addr
                                .unwrap_or(0)
                                .checked_add(off)?
                                .is_multiple_of(align_bytes(c.align)?);
                        same.then_some((ch, off))
                    });
                    match keep {
                        Some(at) => {
                            let align = align_bytes(c.align).unwrap_or(1);
                            admit(&mut self.chunks, at.0, align, base.rules.max_page, c.key.name)?;
                            self.place[i] = Some(at);
                        }
                        None => return Err(why.reason(c.key.name, "changed size or place")),
                    }
                }
                Fate::Region { out, cat } => by_group.entry((out, cat)).or_default().push(i),
                Fate::Beside => match prev.and_then(|p| p.place.get()) {
                    // Settled once the chunks' sizes are (`beside`).
                    Some(at) => {
                        base.chunk(at.0)?;
                        self.place[i] = Some(at);
                    }
                    None => return Err(Refusal::NoOutput.reason(c.key.name)),
                },
            }
            self.fate[i] = fate;
        }

        // Item 2, per (O, c): keep, then place.
        let page = base.rules.max_page;
        for ((out, cat), mut list) in by_group {
            list.sort_by_key(|&i| self.changes[i].index);
            let grp = self.groups.entry((file, out, cat)).or_default();
            // Where each section's previous version was in this (O, c).
            let mut was: Vec<Option<(u64, usize)>> = Vec::with_capacity(list.len());
            for &i in &list {
                let row = self.prev[i].as_ref();
                let before = match row.and_then(|row| row.place.get().map(|at| (row, at))) {
                    Some((row, (ch, off)))
                        if base.chunk(ch)?.kind.out() == Some(out)
                            && matches!(base.chunk(ch)?.kind, ChunkKind::Body(_))
                            && category(&row.name, row.sh_flags) == cat =>
                    {
                        let addr = base.addr(ch, off)?;
                        let pos = grp
                            .regions
                            .iter()
                            .position(|r| r.start <= addr && addr < r.end.max(r.start + 1));
                        // A section the stored layout placed in a body of
                        // `(O, c)` is in one of its file's regions there
                        // (IN-30: every run of a file's members is one).
                        // With none, the region's capacity is unknown, so
                        // neither keep nor place can be applied: refused,
                        // never sent to the overflow phase (which would
                        // move a section that did not grow, M2b2b-3).
                        if pos.is_none() {
                            return Err(not_implemented());
                        }
                        pos.map(|pos| (addr, pos))
                    }
                    _ => None,
                };
                was.push(before);
            }
            // Keep.
            let mut rest = Vec::new();
            for (pos, &i) in list.iter().enumerate() {
                let c = &self.changes[i];
                let align = align_bytes(c.align).unwrap_or(1);
                let old_size = self.prev[i].as_ref().map_or(0, |p| p.size);
                match was[pos] {
                    Some((a, r)) if c.size <= old_size && a.is_multiple_of(align) => {
                        let end = a.checked_add(c.size).ok_or_else(not_implemented)?;
                        if grp.clash(a, end).is_some() || end > grp.regions[r].end {
                            // Two previous versions overlapping: the stored
                            // rows are not a layout.
                            return Err(corrupt(format!("sections of file {} overlap", file.0)));
                        }
                        grp.take(a, end);
                        let reg = &grp.regions[r];
                        admit(&mut self.chunks, reg.row.chunk, align, page, c.key.name)?;
                        self.place[i] = Some((reg.row.chunk, reg.row.offset + (a - reg.start)));
                    }
                    _ => rest.push(pos),
                }
            }
            // Place.
            for pos in rest {
                let i = list[pos];
                let c = &self.changes[i];
                let align = align_bytes(c.align).unwrap_or(1);
                let mut got = None;
                if let Some((a, r)) = was[pos]
                    && a.is_multiple_of(align)
                    && let Some(end) = a.checked_add(c.size)
                    && end <= grp.regions[r].end
                    && grp.clash(a, end).is_none()
                {
                    got = Some((a, r));
                }
                if got.is_none()
                    && let Some(a) = grp.first_fit(c.size, align)
                {
                    let r = grp
                        .regions
                        .iter()
                        .position(|r| r.start <= a && a + c.size <= r.end)
                        .ok_or_else(not_implemented)?;
                    got = Some((a, r));
                }
                match got {
                    Some((a, r)) => {
                        let reg = &grp.regions[r];
                        admit(&mut self.chunks, reg.row.chunk, align, page, c.key.name)?;
                        let at = (reg.row.chunk, reg.row.offset + (a - reg.start));
                        grp.take(a, a + c.size);
                        self.place[i] = Some(at);
                    }
                    None => grp.overflow.push(i),
                }
            }
        }
        Ok(())
    }

    /// L-159's empty sections ([`Fate::Beside`]), once every chunk has its
    /// size: the full link puts them at the END of the last chunk before
    /// their output section in order (of the same rank and load class),
    /// else at the START of the first one after it, with no alignment. The
    /// update does not search for that neighbour again: the chunk order is
    /// the stored one (an output section that appears or empties is refused
    /// elsewhere), so it is the previous place's chunk. A place at that
    /// chunk's old end moves to its new end; one at its start stays; any
    /// other place, or one in a chunk that was empty and changed size (a
    /// start and an end are the same offset, as `end_rows`), is refused.
    fn beside(&mut self) -> Result<(), IncrReason> {
        for i in 0..self.changes.len() {
            if self.fate[i] != Fate::Beside {
                continue;
            }
            let Some((c, off)) = self.place[i] else { continue };
            let old = self.base.chunk(c)?.size;
            let new = self.chunks.get(c).ok_or_else(not_implemented)?.size;
            let at = if old == new {
                off
            } else if off == old && old != 0 {
                new
            } else if off == 0 && old != 0 {
                0
            } else {
                return Err(not_implemented());
            };
            if at > new {
                return Err(not_implemented());
            }
            self.place[i] = Some((c, at));
        }
        Ok(())
    }

    /// Item 4's `SHF_LINK_ORDER` rule, and the killed fixed sections.
    fn link_order(&mut self) -> Result<(), IncrReason> {
        // A killed fixed section (items 4, 8, 10): its old place cannot
        // become a hole.
        let base = self.base;
        let mut killed_fixed: Vec<(StoredSection, Why)> = Vec::new();
        let mut fixed_rows: Vec<&StoredSection> = self.orphans.iter().collect();
        for (i, p) in self.prev.iter().enumerate() {
            if let Some(p) = p
                && self.place[i].is_none()
                && p.place.get().is_some()
            {
                fixed_rows.push(p);
            }
        }
        for p in fixed_rows {
            let shape = Shape {
                index: p.index,
                name: &p.name,
                sh_type: p.sh_type,
                flags: p.sh_flags,
                size: p.size,
            };
            match base.fate(&shape, true, self.debug) {
                Fate::Fixed { why, .. } => killed_fixed.push((p.clone(), why)),
                // A dead `.eh_frame` section is item 5's (`eh`), a dead
                // merged section's pieces item 6's (`frags`); a killed
                // debug section is G-1's.
                // With the debug placement (`Fate::Debug`) the killed
                // debug section is listed in `killed` for it.
                Fate::Refuse(Refusal::Debug) => return Err(Refusal::Debug.reason(&p.name)),
                _ => {}
            }
        }
        if let Some((p, why)) = killed_fixed.first() {
            return Err(why.reason(&p.name, "died"));
        }
        // Item 4: a live `SHF_LINK_ORDER` section in a file whose code
        // moved, died or revived (its order follows its target's).
        for (i, c) in self.changes.iter().enumerate() {
            if !matches!(self.fate[i], Fate::Fixed { why: Why::LinkOrder, .. })
                || self.place[i].is_none()
            {
                continue;
            }
            let moved = self
                .changes
                .iter()
                .enumerate()
                .any(|(j, d)| d.file == c.file && self.region_section_changed(j))
                || self.orphans.iter().any(|o| o.file == c.file);
            if moved {
                return Err(Why::LinkOrder.reason(c.key.name, "follows a section that moved"));
            }
        }
        Ok(())
    }

    /// Whether change `j` is a region section whose place differs from its
    /// previous version's (or which appeared or died).
    fn region_section_changed(&self, j: usize) -> bool {
        let prev = self.prev[j].as_ref().and_then(|p| p.place.get());
        let now = self.place[j];
        match (prev, now) {
            (None, None) => false,
            (Some(a), Some(b)) => a != b,
            _ => true,
        }
    }

    /// Item 2 phase 3 and item 3's opening: one new region per (file, O,
    /// c) with sections left, in file order, then (O, c) order, at O's
    /// tail.
    fn overflow(&mut self) -> Result<(), IncrReason> {
        let base = self.base;
        let keys: Vec<(FileId, OutSecId, u8)> =
            self.groups.iter().filter(|(_, g)| !g.overflow.is_empty()).map(|(k, _)| *k).collect();
        for key in keys {
            let (file, out, cat) = key;
            let last =
                *base.of_out.get(&out).and_then(|v| v.last()).ok_or_else(|| {
                    IncrReason::NewOutputSection(format!("output section {}", out.0))
                })?;
            let chunk = *base.chunk(last)?;
            let name = base.names.get(&out).map_or_else(String::new, |n| text(n));
            if !matches!(chunk.kind, ChunkKind::Body(_)) {
                // The tail reserve is the island's or the merged chunk's
                // (chunk.rs `capacities`): no region can open in it
                // (`spec/landed/incr/WP-216.md` §5 F-3).
                return Err(IncrReason::ReserveExhausted(format!(
                    "{name}: its tail is in a {} chunk",
                    if matches!(chunk.kind, ChunkKind::Patches(_)) {
                        "patch island"
                    } else {
                        "merged"
                    }
                )));
            }
            let g = self.groups.get_mut(&key).ok_or_else(not_implemented)?;
            let pending = std::mem::take(&mut g.overflow);
            let max_align = pending
                .iter()
                .map(|&i| align_bytes(self.changes[i].align).unwrap_or(1))
                .max()
                .unwrap_or(1);
            admit(&mut self.chunks, last, max_align, base.rules.max_page, name.as_bytes())?;
            let caddr = chunk.addr.ok_or_else(|| corrupt("a body chunk without an address"))?;
            let size = self.chunks[last].size;
            let tail = caddr.checked_add(size).ok_or_else(not_implemented)?;
            let start = round_up(tail, max_align).ok_or_else(not_implemented)?;
            let mut at = start;
            for &i in &pending {
                let c = &self.changes[i];
                at = round_up(at, align_bytes(c.align).unwrap_or(1)).ok_or_else(not_implemented)?;
                self.place[i] = Some((last, at - caddr));
                let e = at.checked_add(c.size).ok_or_else(not_implemented)?;
                g.take(at, e);
                at = e;
            }
            let need = at - start;
            let log = max_align.trailing_zeros() as u8;
            let capacity = need + Slack::INCREMENTAL.region(need, log);
            let ordinal = g.regions.iter().map(|r| r.row.ordinal + 1).max().unwrap_or(0);
            let row = StoredRegion {
                file,
                out,
                category: cat,
                ordinal,
                chunk: last,
                offset: start - caddr,
                capacity,
            };
            let end = start.checked_add(capacity).ok_or_else(not_implemented)?;
            g.regions.push(Reg { row, start, end });
            if start > tail {
                self.pads.push(Hole {
                    chunk: last,
                    offset: size,
                    size: start - tail,
                    kind: HoleKind::Fill,
                });
            }
            self.chunks[last].size = row.offset + capacity;
        }
        Ok(())
    }

    /// The alignment a growth step keeps: the largest of the chunks that
    /// follow in the `PT_LOAD`, and the page when the grown chunk or one
    /// that follows is RELRO.
    fn step_align(&self, relro: bool, followers: &[ChunkId], page: u64) -> u64 {
        let relro = relro || followers.iter().any(|&f| self.chunks[f].relro);
        let a = followers.iter().map(|&f| 1u64 << self.chunks[f].align.min(63)).max().unwrap_or(1);
        if relro { a.max(page) } else { a }
    }

    /// Item 3: a chunk grown past its capacity takes a growth step; every
    /// later chunk of its `PT_LOAD` moves by it.
    fn growth(&mut self) -> Result<(), IncrReason> {
        let base = self.base;
        let page = base.rules.max_page.max(1);
        let mut touched_runs = BTreeSet::new();
        for (r, members) in base.runs.iter().enumerate() {
            let mut shift = 0u64;
            for (k, &c) in members.iter().enumerate() {
                self.shift[c.0 as usize] = shift;
                let ch = self.chunks[c];
                if ch.size <= ch.capacity {
                    continue;
                }
                let out = ch.kind.out().ok_or_else(not_implemented)?;
                let need = ch.size - ch.capacity;
                let osize: u64 = base
                    .of_out
                    .get(&out)
                    .map_or(0, |v| v.iter().map(|&x| self.chunks[x].size).sum());
                let followers = &members[k + 1..];
                let a = self.step_align(ch.relro, followers, page);
                // L-9 (P3-1, WP-262 C): the last chunk of a `PT_LOAD` that
                // takes memory ends the segment's memory, which must be a
                // section's end; the full link gives it no slack (L-87's
                // incremental policy, layout.md §18), and neither does its
                // growth step: the step is what it needs. A step the
                // alignment of what follows would round past that (an
                // empty chunk after it, or the RELRO page) is refused.
                let last = !followers.iter().any(|&f| self.chunks[f].takes_memory());
                let slack = if last { 0 } else { slack::TAIL_MIN.max(osize / slack::TAIL_DIV) };
                let step = round_up(need + slack, a)
                    .ok_or_else(|| IncrReason::ReserveExhausted("growth step overflows".into()))?;
                if last && step != need {
                    return Err(last_chunk_refused());
                }
                self.chunks[c].capacity = ch.capacity + step;
                self.steps.push((ch.addr.unwrap_or(0) + shift, GrowthStep { out, step }));
                shift = shift.checked_add(step).ok_or_else(not_implemented)?;
                touched_runs.insert(r);
            }
        }
        // Move the chunks, then check each grown run against the next.
        for r in &touched_runs {
            let members = &base.runs[*r];
            for &c in members {
                let s = self.shift[c.0 as usize];
                if s == 0 {
                    continue;
                }
                let ch = &mut self.chunks[c];
                ch.addr = ch.addr.and_then(|x| x.checked_add(s));
                ch.offset = ch.offset.checked_add(s).ok_or_else(not_implemented)?;
                if ch.addr.is_none() {
                    return Err(IncrReason::ReserveExhausted(
                        "growth step past the address space".into(),
                    ));
                }
            }
            let mem_end: u128 = members
                .iter()
                .map(|&c| &self.chunks[c])
                .filter(|c| c.takes_memory())
                .filter_map(|c| c.addr.map(|a| u128::from(a) + u128::from(c.capacity)))
                .max()
                .unwrap_or(0);
            let limit: u128 = match base.runs.get(r + 1).and_then(|m| m.first()) {
                Some(&next) => {
                    let a = base.chunks[next].addr.unwrap_or(0);
                    u128::from(a - a % page)
                }
                None => base.address_limit,
            };
            let name = |r: usize| {
                base.names
                    .get(&base.chunks[base.runs[r][0]].kind.out().unwrap_or(OutSecId(u32::MAX)))
                    .map_or_else(|| "a PT_LOAD".to_owned(), |n| text(n))
            };
            if mem_end > limit {
                return Err(IncrReason::ReserveExhausted(format!(
                    "the PT_LOAD of {}: segment reserve (memory)",
                    name(*r)
                )));
            }
            let last_at = members.iter().map(|c| base.at[c.0 as usize]).max().unwrap_or(0);
            let file_end: u128 = members
                .iter()
                .map(|&c| &self.chunks[c])
                .filter(|c| c.in_file() && c.capacity > 0)
                .map(|c| u128::from(c.offset) + u128::from(c.capacity))
                .max()
                .unwrap_or(0);
            let next_file = base.order[last_at + 1..]
                .iter()
                .map(|&c| &base.chunks[c])
                .find(|c| c.in_file() && c.capacity > 0)
                .map(|c| u128::from(c.offset));
            if next_file.is_some_and(|n| file_end > n) {
                return Err(IncrReason::ReserveExhausted(format!(
                    "the PT_LOAD of {}: segment reserve (file)",
                    name(*r)
                )));
            }
        }
        Ok(())
    }

    /// Holes (IN-3): every free range of a touched region is a fill hole
    /// (a section's freed tail exactly like slack, IN-3a; P-I5), with the
    /// padding before a new region; the stored holes of the touched
    /// chunks elsewhere are kept.
    fn holes(&mut self, stored: &StoredLayout<'_>) -> Result<(), IncrReason> {
        let mut by_chunk: BTreeMap<ChunkId, Touched> = BTreeMap::new();
        for g in self.groups.values() {
            for r in &g.regions {
                let caddr = r.start - r.row.offset;
                let entry = by_chunk.entry(r.row.chunk).or_default();
                entry.0.push((r.row.offset, r.row.offset + r.row.capacity));
                let mut at = r.start;
                for (&s, &e) in g.occupied.range(r.start..r.end) {
                    if s > at {
                        entry.1.push(Hole {
                            chunk: r.row.chunk,
                            offset: at - caddr,
                            size: s - at,
                            kind: HoleKind::Fill,
                        });
                    }
                    at = at.max(e);
                }
                if r.end > at {
                    entry.1.push(Hole {
                        chunk: r.row.chunk,
                        offset: at - caddr,
                        size: r.end - at,
                        kind: HoleKind::Fill,
                    });
                }
            }
        }
        for h in &self.pads {
            by_chunk.entry(h.chunk).or_default().1.push(*h);
        }
        // Item 6: dead constants' fills; revived fragments' holes go.
        for c in self.merged.chunks() {
            by_chunk.entry(c).or_default();
        }
        for h in &self.merged.made {
            by_chunk.entry(h.chunk).or_default().1.push(*h);
        }
        // Item 7: retired slots' `DeadSlot` holes; revived slots' go.
        for c in self.slots.chunks() {
            by_chunk.entry(c).or_default();
        }
        for h in &self.slots.made {
            by_chunk.entry(h.chunk).or_default().1.push(*h);
        }
        let mut all = Vec::new();
        let mut made = Vec::new();
        self.hole_chunks = by_chunk.keys().copied().collect();
        for (chunk, (ranges, new)) in by_chunk {
            let old = stored.holes_in(chunk)?;
            let mut kept = Vec::new();
            let gone = |h: &Hole| self.merged.gone.contains(h) || self.slots.gone.contains(h);
            for h in old.iter().filter(|h| !gone(h)) {
                // The parts of a stored hole outside every touched region.
                let mut pieces = vec![(h.offset, h.offset + h.size)];
                for &(s, e) in &ranges {
                    pieces = pieces
                        .into_iter()
                        .flat_map(|(a, b)| {
                            let mut v = Vec::new();
                            if a < s.min(b) {
                                v.push((a, s.min(b)));
                            }
                            if e.max(a) < b {
                                v.push((e.max(a), b));
                            }
                            v
                        })
                        .collect();
                }
                kept.extend(pieces.into_iter().map(|(a, b)| Hole { offset: a, size: b - a, ..*h }));
            }
            let mut holes: Vec<Hole> = kept.into_iter().chain(new).filter(|h| h.size > 0).collect();
            holes.sort_by_key(|h| (h.offset, h.size));
            holes.dedup();
            let size = self.chunks[chunk].size;
            let overlap = holes.windows(2).any(|w| w[0].offset + w[0].size > w[1].offset);
            if overlap || holes.last().is_some_and(|h| h.offset + h.size > size) {
                debug_assert!(false, "delta holes of chunk {} overlap or pass its size", chunk.0);
                return Err(not_implemented());
            }
            self.region_pads(chunk, &mut holes)?;
            for h in &holes {
                if !old.contains(h) {
                    made.push(*h);
                }
            }
            all.extend(holes);
        }
        self.holes = all;
        self.made = made;
        Ok(())
    }

    /// S2-1 (ruling "After 255e2's hand-back" item 1, option (b)): the
    /// padding before a touched region that keeps its start is a `Fill`
    /// hole `[previous region end, region start)` once the item at the
    /// start no longer explains it (its first member's alignment dropped,
    /// or the start is free). `holes` is the chunk's final hole list
    /// (sorted, disjoint); the pad is inserted in order.
    ///
    /// The previous region's end is not a row the delta reads (REGIONS is
    /// file-keyed). It is the end of the last hole before the start, by
    /// invariant I: in a tolerant body chunk, every region with bytes that
    /// is followed by another region ends at a `Fill` hole ending exactly
    /// at its end, or is followed by a `Fill` hole starting at its end (a
    /// pad). Then `[e, start)`, `e` the end of the last hole ending at or
    /// before the start, holds no live byte: a live byte there is in a
    /// region R' before the start with no hole ending in (that byte,
    /// start], which I excludes. I holds for a full link's state (a
    /// region followed by another in its part has `Slack::region`'s whole
    /// slack, at least 64 bytes; only a part's last region is clipped,
    /// `chunk.rs` `close`) and this function keeps it: a touched region
    /// with bytes followed by another, or the region an overflow region
    /// is appended after, with no such hole is `update-not-implemented
    /// (layout)` (a section fitted exactly to a region's end), never a
    /// state in which a later pad could cover a live byte. A pad as long
    /// as the chunk's alignment or more is not padding: none is made (the
    /// clip plan then refuses the byte by name).
    fn region_pads(&self, chunk: ChunkId, holes: &mut Vec<Hole>) -> Result<(), IncrReason> {
        let ch = self.chunks.get(chunk).ok_or_else(not_implemented)?;
        // Regions live in body chunks only (a merged chunk's tail grows by
        // item 6, not by a region).
        if !matches!(ch.kind, ChunkKind::Body(_)) {
            return Ok(());
        }
        let old_size = self.base.chunk(chunk)?.size;
        let chunk_align = 1u64 << ch.align.min(63);
        let ends_or_starts_at = |hs: &[Hole], x: u64| {
            hs.iter().any(|h| h.offset + h.size == x || (h.offset == x && h.size > 0))
        };
        let mut pads = Vec::new();
        for g in self.groups.values() {
            for r in g.regions.iter().filter(|r| r.row.chunk == chunk) {
                let (start, end) = (r.row.offset, r.row.offset + r.row.capacity);
                // I, kept: a touched region with bytes followed by another.
                if r.row.capacity > 0 && end < ch.size && !ends_or_starts_at(holes, end) {
                    return Err(not_implemented());
                }
                if start == 0 {
                    continue;
                }
                let k = holes.partition_point(|h| h.offset + h.size <= start);
                let Some(e) = k.checked_sub(1).map(|j| holes[j].offset + holes[j].size) else {
                    continue;
                };
                if e == start || holes.get(k).is_some_and(|h| h.offset < start) {
                    continue;
                }
                // The alignment of the member (with bytes) at the start.
                let first = self
                    .changes
                    .iter()
                    .enumerate()
                    .filter(|(i, c)| {
                        c.size > 0
                            && matches!(self.fate[*i], Fate::Region { .. })
                            && self.place[*i] == Some((chunk, start))
                    })
                    .map(|(_, c)| align_bytes(c.align).unwrap_or(1))
                    .max();
                let explained = first.is_some_and(|a| round_up(e, a) == Some(start));
                if explained || start - e >= chunk_align {
                    continue;
                }
                pads.push(Hole { chunk, offset: e, size: start - e, kind: HoleKind::Fill });
            }
        }
        // I, kept: the region an overflow region is appended after (its
        // pad, `overflow`'s, is in `holes` and starts at the old size).
        if ch.size > old_size && old_size > 0 && !ends_or_starts_at(holes, old_size) {
            return Err(not_implemented());
        }
        if !pads.is_empty() {
            holes.extend(pads);
            holes.sort_by_key(|h| (h.offset, h.size));
            holes.dedup();
            if holes.windows(2).any(|w| w[0].offset + w[0].size > w[1].offset) {
                debug_assert!(false, "a pad of chunk {} overlaps a hole", chunk.0);
                return Err(not_implemented());
            }
        }
        Ok(())
    }

    /// Item 10's "an output section left empty" (WP-216 F-6): an output
    /// section that lost a member in this update (a section of a file of
    /// `changes` placed in it before and not now) and in which this
    /// update places none is `new-output-section`, unless one of its
    /// chunks still holds a live byte. No member count is stored, so the
    /// proof is by bytes: in a chunk, every byte is a live section's, a
    /// hole (a region's slack, a dead section, IN-3), or alignment padding
    /// before a member or a region, which is shorter than the chunk's
    /// alignment. A run of non-hole bytes at least the chunk's alignment
    /// long therefore holds a live section; a non-body chunk (merged,
    /// synthetic) with bytes is live. Anything else is taken as empty: an
    /// output section whose only members left are empty sections is
    /// refused too (a refusal, never a kept empty output section). An
    /// empty section the update places does not count as placing a member
    /// (WP-242: the corner of F-6, a full link gives an output section
    /// whose live members are all empty no chunk, L-159).
    ///
    /// Why the proof never keeps an empty output section (WP-242, F-6's
    /// corner): between two holes (or a hole and the chunk's ends) with no
    /// live byte between them, the bytes are padding before empty members
    /// and regions only, each placed at the next multiple of its
    /// alignment; for powers of two, rounding up by `a` then by `b` is
    /// rounding up by `max(a, b)`, so the run is shorter than the largest
    /// alignment in it, which is at most the chunk's (`admit` only raises
    /// it). Two paddings around a zero-capacity region of dead empty
    /// sections are one such run.
    fn left_empty(&self, stored: &StoredLayout<'_>) -> Result<(), IncrReason> {
        let base = self.base;
        let out_of = |at: Option<(ChunkId, u64)>| {
            at.and_then(|(c, _)| base.chunks.get(c)).and_then(|c| c.kind.out())
        };
        let mut placed: BTreeSet<OutSecId> = BTreeSet::new();
        let mut lost: BTreeSet<OutSecId> = BTreeSet::new();
        let is_debug = |shape: &Shape<'_>| base.fate(shape, true, self.debug) == Fate::Debug;
        for (i, now) in self.place.iter().enumerate() {
            // The debug placement's output sections (IN-50) are its own,
            // live or dead.
            let c = &self.changes[i];
            let shape = Shape {
                index: c.index,
                name: c.key.name,
                sh_type: c.sh_type,
                flags: c.sh_flags,
                size: c.size,
            };
            if is_debug(&shape) {
                continue;
            }
            let now = out_of(*now);
            if self.changes[i].size > 0 {
                placed.extend(now);
            }
            let before = out_of(self.prev[i].as_ref().and_then(|p| p.place.get()));
            if let Some(o) = before
                && now != Some(o)
            {
                lost.insert(o);
            }
        }
        let debug_orphan = |o: &StoredSection| {
            let shape = Shape {
                index: o.index,
                name: &o.name,
                sh_type: o.sh_type,
                flags: o.sh_flags,
                size: o.size,
            };
            is_debug(&shape)
        };
        lost.extend(
            self.orphans.iter().filter(|o| !debug_orphan(o)).filter_map(|o| out_of(o.place.get())),
        );
        // Item 6: a fragment fell to 0 / one rose or was appended.
        lost.extend(self.merged.died.iter().copied());
        placed.extend(self.merged.rose.iter().copied());
        for out in lost.difference(&placed) {
            if !self.holds_live_byte(stored, *out)? {
                let name = base.names.get(out).map_or_else(String::new, |n| text(n));
                return Err(IncrReason::NewOutputSection(format!(
                    "output section {name} left empty"
                )));
            }
        }
        Ok(())
    }

    /// Whether a chunk of `out` holds a run of non-hole bytes at least its
    /// alignment long, or is a non-body chunk with bytes ([`Self::left_empty`]).
    /// A merged chunk with no hole and bytes holds a live fragment (its
    /// bytes are fragments and the padding before them, L-147); with holes
    /// (dead fragments) it takes the body's proof: padding before a
    /// fragment is shorter than its alignment, which is at most the
    /// chunk's.
    fn holds_live_byte(
        &self,
        stored: &StoredLayout<'_>,
        out: OutSecId,
    ) -> Result<bool, IncrReason> {
        for &c in self.base.of_out.get(&out).map_or(&[][..], Vec::as_slice) {
            let chunk = &self.chunks[c];
            if chunk.size == 0 {
                continue;
            }
            let mut holes: Vec<Hole> = match chunk.kind {
                ChunkKind::Body(_) | ChunkKind::Merged(_) => {
                    if self.hole_chunks.binary_search(&c).is_ok() {
                        self.holes.iter().filter(|h| h.chunk == c).copied().collect()
                    } else {
                        stored.holes_in(c)?
                    }
                }
                _ => return Ok(true),
            };
            if matches!(chunk.kind, ChunkKind::Merged(_)) && holes.is_empty() {
                return Ok(true);
            }
            holes.sort_by_key(|h| h.offset);
            let align = 1u64 << chunk.align.min(63);
            let mut at = 0u64;
            for h in holes.iter().chain(std::iter::once(&Hole {
                chunk: c,
                offset: chunk.size,
                size: 0,
                kind: HoleKind::Fill,
            })) {
                if h.offset.saturating_sub(at) >= align {
                    return Ok(true);
                }
                at = at.max(h.offset.saturating_add(h.size));
            }
        }
        Ok(false)
    }

    /// The segments derived again, the TLS template and the entry moved
    /// with their chunks, and the lists of IN-72's result.
    #[allow(clippy::too_many_lines, reason = "the result's fields in order")]
    fn finish(self) -> Result<LayoutDelta, IncrReason> {
        let base = self.base;
        let members = self.members()?;
        let segments = derive_segments(&self.chunks, &base.order, &base.rules);
        let kinds = |v: &[Segment]| {
            let mut t: Vec<u32> = v.iter().map(|s| s.p_type).collect();
            t.sort_unstable();
            t
        };
        if kinds(&segments) != kinds(&base.segments) {
            return Err(IncrReason::NewOutputSection("a segment appeared or emptied".into()));
        }
        // The chunk an address of the stored layout is in, and its shift.
        let shift_at = |a: u64| -> u64 {
            base.order
                .iter()
                .find(|&&c| {
                    let ch = &base.chunks[c];
                    ch.takes_memory()
                        && ch.addr.is_some_and(|s| {
                            s <= a && u128::from(a) < u128::from(s) + u128::from(ch.capacity)
                        })
                })
                .map_or(0, |&c| self.shift[c.0 as usize])
        };
        let tls = base.rest.tls.map(|mut t| {
            let first =
                base.order.iter().find(|&&c| base.chunks[c].is_alloc() && base.chunks[c].is_tls());
            t.start += first.map_or(0, |&c| self.shift[c.0 as usize]);
            t
        });
        let entry = base.rest.entry.map(|e| e + shift_at(e));

        // moved / killed / revived, by identity (`spec/landed/incr/WP-216.md` §2).
        let now = |at: Option<(ChunkId, u64)>| -> Option<(Option<u64>, ChunkId, u64)> {
            let (c, off) = at?;
            Some((self.chunks[c].addr.map(|a| a + off), c, off))
        };
        let before = |p: &StoredSection| -> Option<(Option<u64>, ChunkId, u64)> {
            let (c, off) = p.place.get()?;
            Some((base.chunks.get(c)?.addr.map(|a| a + off), c, off))
        };
        let (mut moved, mut killed, mut revived) = (Vec::new(), Vec::new(), Vec::new());
        let mut top = 0u32;
        for (i, c) in self.changes.iter().enumerate() {
            top = top.max(c.id.0);
            let prev = self.prev[i].as_ref();
            if let Some(p) = prev {
                top = top.max(p.id.0);
            }
            match (prev.and_then(before), now(self.place[i])) {
                (Some(b), Some(n)) => {
                    let differs = match (b.0, n.0) {
                        (Some(x), Some(y)) => x != y,
                        _ => (b.1, b.2) != (n.1, n.2),
                    };
                    if differs {
                        moved.push(c.id);
                    }
                }
                (None, Some(_)) => revived.push(c.id),
                (Some(_), None) => killed.extend(prev.map(|p| p.id)),
                (None, None) => {}
            }
        }
        for o in &self.orphans {
            top = top.max(o.id.0);
            killed.push(o.id);
        }
        moved.sort_unstable();
        killed.sort_unstable();
        killed.dedup();
        revived.sort_unstable();

        let len = if self.changes.is_empty() && self.orphans.is_empty() { 0 } else { top + 1 };
        let mut place: Column<SectionId, Place> = Column::filled(len, Place::NONE);
        for (i, c) in self.changes.iter().enumerate() {
            if let Some((ch, off)) = self.place[i] {
                place[c.id] = Place::at(ch, off);
            }
        }
        let mut steps = self.steps;
        steps.sort_by_key(|s| s.0);
        let mut regions_opened: Vec<StoredRegion> = self
            .groups
            .values()
            .flat_map(|g| g.regions.iter().map(|r| r.row))
            .filter(|r| !self.opened.contains(r))
            .collect();
        regions_opened.sort_unstable();
        let mut holes = self.holes;
        holes.sort_by_key(|h| (h.chunk, h.offset));
        let mut made = self.made;
        made.sort_by_key(|h| (h.chunk, h.offset));
        let defined = end_rows(&base.rest.defined, &base.chunks, &self.chunks)?;
        Ok(LayoutDelta {
            placement: Placement {
                kind: PlacementKind::Loadable,
                chunks: self.chunks,
                order: base.order.clone().into_boxed_slice(),
                segments: Column::from_vec(segments).map_err(|_| not_implemented())?,
                place,
                sym_place: base.rest.sym_place.clone().into_boxed_slice(),
                defined: defined.into_boxed_slice(),
                patches: base.rest.patches.clone().into_boxed_slice(),
                redirects: base.rest.redirects.clone().into_boxed_slice(),
                sized: Box::default(),
                tls,
                entry,
                holes: holes.into_boxed_slice(),
            },
            moved,
            killed,
            revived,
            holes: made,
            regions_opened,
            growth_steps: steps.into_iter().map(|s| s.1).collect(),
            files: Vec::new(),
            hole_chunks: self.hole_chunks,
            eh: None,
            frags: self.merged.rows,
            slots_appended: Vec::new(),
            slots_dead: self.slots.dead,
            slots_revived: self.slots.revived,
            slots_recounted: self.slots.recounted,
            members,
        })
    }

    /// [`LayoutDelta::members`] (WP-255g): the stored `CHUNKS.members`,
    /// -1 for each stored row of a change or an orphan that was a member
    /// (placed in an in-file `Body` chunk, not `SHT_NOBITS`, with bytes:
    /// what `layout_plan::member_front` gives a row), +1 for each change
    /// placed now as one. A debug placement change (`Fate::Debug`, placed
    /// by the update's debug step, not here) makes its old chunk and every
    /// non-allocated chunk unknown. Empty without a stored count.
    ///
    /// # Errors
    /// `state-corrupt` for a count that falls below 0 (the stored count
    /// is not the stored members').
    fn members(&self) -> Result<Vec<Option<u32>>, IncrReason> {
        let base = self.base;
        let Some(stored) = &base.members else { return Ok(Vec::new()) };
        let mut n: Vec<Option<i64>> = stored.iter().map(|&m| Some(i64::from(m))).collect();
        let member = |at: Option<(ChunkId, u64)>, sh_type: u32, size: u64| {
            let (c, _) = at?;
            let ch = base.chunks.get(c)?;
            (ch.in_file()
                && matches!(ch.kind, ChunkKind::Body(_))
                && sh_type != SHT_NOBITS
                && size > 0)
                .then_some(c.0 as usize)
        };
        let mut debug = false;
        for (i, c) in self.changes.iter().enumerate() {
            let prev = self.prev[i].as_ref();
            if self.fate[i] == Fate::Debug {
                debug = true;
                if let Some(k) = prev.and_then(|p| p.place.get()).map(|(c, _)| c.0 as usize) {
                    n[k] = None;
                }
                continue;
            }
            if let Some(k) = prev.and_then(|p| member(p.place.get(), p.sh_type, p.size)) {
                n[k] = n[k].map(|x| x - 1);
            }
            if let Some(k) = member(self.place[i], c.sh_type, c.size) {
                n[k] = n[k].map(|x| x + 1);
            }
        }
        for o in &self.orphans {
            if let Some(k) = member(o.place.get(), o.sh_type, o.size) {
                n[k] = n[k].map(|x| x - 1);
            }
        }
        if debug {
            for (c, ch) in base.chunks.iter() {
                if !ch.is_alloc() {
                    n[c.0 as usize] = None;
                }
            }
        }
        n.into_iter()
            .enumerate()
            .map(|(k, x)| match x {
                None => Ok(None),
                Some(x) => u32::try_from(x).map(Some).map_err(|_| {
                    corrupt(format!("CHUNKS.members: chunk {k} counts {x} after the update"))
                }),
            })
            .collect()
    }
}

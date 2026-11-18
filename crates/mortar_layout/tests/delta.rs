//! U-4's tests (`spec/incr.md` IN-72; WP-216): the delta layout over
//! first links of the `World` harness in incremental mode (slack on),
//! captured into a `MemoryLayout`. Each update's result is laid over the
//! stored placement and judged by `Layout::check` against the edited link
//! (its own assignment, liveness and headers); hops chain, the next one
//! reading the state the previous one gave.
#![allow(clippy::many_single_char_names)] // the harness's w, a, p, f, s, h, c

mod common;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::{Sec, World, sec};
use mortar_core::assignment::Assignment;
use mortar_core::column::Column;
use mortar_core::demands::DemandFlags;
use mortar_core::id::{ChunkId, FileId, OutSecId, SectionId};
use mortar_core::identity::{Anchor, SectionKey};
use mortar_core::incr::{
    EhFile, EhRecordIn, FileDemands, FilePieces, IncrReason, LayoutDelta, LayoutInput, LayoutRows,
    MemoryLayout, MergeClass, MergedPiece, PlaceNow, SectionChange, SlotDemand, StoredEhRange,
    StoredFragUse, StoredKeyRef, StoredLayout, StoredRegion, StoredSection, StoredSlot,
    SymtabBlock, SymtabTables, Unimplemented,
};
use mortar_core::layout::{
    Chunk, ChunkKind, Defined, Hole, HoleKind, Patch, Place, Placement, Redirect, Segment,
    SlotKind, SymPlace, Tls,
};
use mortar_core::par::Pool;
use mortar_elf::Machine;
use mortar_elf::consts::{
    SHF_ALLOC, SHF_EXECINSTR, SHF_LINK_ORDER, SHF_MERGE, SHF_STRINGS, SHF_TLS, SHF_WRITE,
    SHT_NOBITS, SHT_NOTE, SHT_PROGBITS,
};
use mortar_layout::Placed;
use mortar_layout::delta::layout;
use mortar_layout::order::text_category;
use mortar_layout::slack::{self, Slack};

// ---- the harness ------------------------------------------------------------

const AX: u64 = SHF_ALLOC | SHF_EXECINSTR;
const WA: u64 = SHF_ALLOC | SHF_WRITE;

fn text(name: &'static [u8], size: u64) -> Sec {
    sec(name, SHT_PROGBITS, AX, size, 2)
}
fn data(name: &'static [u8], size: u64) -> Sec {
    sec(name, SHT_PROGBITS, WA, size, 3)
}
fn bss(name: &'static [u8], size: u64) -> Sec {
    sec(name, SHT_NOBITS, WA, size, 3)
}
fn rodata(name: &'static [u8], size: u64) -> Sec {
    sec(name, SHT_PROGBITS, SHF_ALLOC, size, 3)
}

/// A link's stored state: the world it was linked from, its placement and
/// its regions, as `LayoutRows`.
struct State {
    world: World,
    assignment: Assignment<'static>,
    placed: Placed,
    regions: Vec<StoredRegion>,
    mem: MemoryLayout,
    /// Per file of `world`, the anchor of each section (index 1..).
    anchors: Vec<Vec<Anchor<'static>>>,
}

fn world(machine: Machine, files: &[Vec<Sec>]) -> World {
    let mut w = World::new(machine, files, &[]);
    w.options.slack = Slack::INCREMENTAL;
    w
}

fn no_anchors(files: &[Vec<Sec>]) -> Vec<Vec<Anchor<'static>>> {
    files.iter().map(|f| vec![Anchor::None; f.len()]).collect()
}

/// The first link (incremental mode) of `files`.
fn first(machine: Machine, files: &[Vec<Sec>]) -> State {
    first_with(world(machine, files), no_anchors(files))
}

fn first_with(w: World, anchors: Vec<Vec<Anchor<'static>>>) -> State {
    let (assignment, placed) = w.run(&Pool::sequential()).expect("first link");
    w.check(&assignment, &placed).expect("the first link passes Layout::check");
    let regions = regions_of_full_link(&w, &assignment, &placed.placement);
    let mem = capture(&w, &assignment, &placed.placement, &regions);
    State { world: w, assignment, placed, regions, mem, anchors }
}

fn nfiles(w: &World) -> u32 {
    w.resolution.section_base.len() - 1
}

fn file_len(w: &World, f: u32) -> u32 {
    let r = w.resolution.section_base[FileId(f)];
    r.end.0 - r.start.0 - 1
}

/// A full link's regions (IN-30): per body chunk of a hole-tolerant
/// output section, each run of one file's members of one category, to the
/// end of the fill hole after it.
fn regions_of_full_link(w: &World, a: &Assignment<'static>, p: &Placement) -> Vec<StoredRegion> {
    let mut members: BTreeMap<ChunkId, Vec<(u64, u64, u32, u8)>> = BTreeMap::new();
    for f in 1..=nfiles(w) {
        for i in 1..=file_len(w, f) {
            let s = w.sid(f, i);
            let Some((c, off)) = p.place[s].get() else { continue };
            let h = &w.headers[s];
            let cat = if h.flags & SHF_EXECINSTR != 0 { text_category(h.name) } else { 0 };
            members.entry(c).or_default().push((off, off + h.size, f, cat));
        }
    }
    let mut out = Vec::new();
    let mut ordinals: BTreeMap<(u32, OutSecId, u8), u32> = BTreeMap::new();
    for (c, mut list) in members {
        let ch = &p.chunks[c];
        let ChunkKind::Body(part) = ch.kind else { continue };
        let o = &a.outsecs[part.out];
        if ch.flags & SHF_ALLOC == 0 || slack::class(o.name, o.flags) != slack::Class::Tolerant {
            continue;
        }
        list.sort_unstable();
        let mut k = 0;
        while k < list.len() {
            let (start, _, f, cat) = list[k];
            let mut end = list[k].1;
            let mut j = k + 1;
            while j < list.len() && (list[j].2, list[j].3) == (f, cat) {
                end = end.max(list[j].1);
                j += 1;
            }
            if let Some(h) = p.holes.iter().find(|h| h.chunk == c && h.offset == end) {
                end = h.offset + h.size;
            }
            let n = ordinals.entry((f, part.out, cat)).or_default();
            out.push(StoredRegion {
                file: FileId(f),
                out: part.out,
                category: cat,
                ordinal: *n,
                chunk: c,
                offset: start,
                capacity: end - start,
            });
            *n += 1;
            k = j;
        }
    }
    out
}

/// The rows a state would give.
fn capture(
    w: &World,
    a: &Assignment<'static>,
    p: &Placement,
    regions: &[StoredRegion],
) -> MemoryLayout {
    let mut sections = Vec::new();
    for f in 1..=nfiles(w) {
        let mut seen: BTreeMap<&[u8], u32> = BTreeMap::new();
        for i in 1..=file_len(w, f) {
            let s = w.sid(f, i);
            let h = &w.headers[s];
            let n = seen.entry(h.name).or_default();
            sections.push(StoredSection {
                id: s,
                file: FileId(f),
                index: i,
                anchor_kind: 0,
                anchor: Vec::new(),
                name: h.name.to_vec(),
                ordinal: *n,
                sh_type: h.sh_type,
                sh_flags: h.flags,
                size: h.size,
                align: 1u64 << h.align,
                place: p.place.get(s).copied().unwrap_or(Place::NONE),
            });
            *n += 1;
        }
    }
    MemoryLayout {
        chunks: p.chunks.as_slice().to_vec(),
        order: p.order.to_vec(),
        segments: p.segments.as_slice().to_vec(),
        outsecs: a.outsecs.iter().map(|(_, o)| o.name.to_vec()).collect(),
        sections,
        regions: regions.to_vec(),
        holes: p.holes.to_vec(),
        frags: Vec::new(),
        frag_uses: Vec::new(),
        eh_ranges: Vec::new(),
        key_refs: Vec::new(),
        slots: Vec::new(),
        patches: p.patches.to_vec(),
        redirects: p.redirects.to_vec(),
        sym_place: p.sym_place.to_vec(),
        defined: p.defined.to_vec(),
        tls: p.tls,
        entry: p.entry,
        symtab_blocks: Vec::new(),
        global_area: SymtabBlock::default(),
        symtab_tables: SymtabTables::default(),
    }
}

/// An edit: the new files, which are changed (new bytes) and which drawn
/// in, and the sections (file, index, 1-based) dead or not kept after it.
#[derive(Clone, Default)]
struct Edit {
    files: Vec<Vec<Sec>>,
    changed: Vec<u32>,
    drawn: Vec<u32>,
    dead: Vec<(u32, u32)>,
    not_kept: Vec<(u32, u32)>,
    anchors: Option<Vec<Vec<Anchor<'static>>>>,
}

impl Edit {
    fn new(files: Vec<Vec<Sec>>) -> Self {
        Self { files, ..Self::default() }
    }
    fn changed(mut self, f: &[u32]) -> Self {
        self.changed = f.to_vec();
        self
    }
    fn drawn(mut self, f: &[u32]) -> Self {
        self.drawn = f.to_vec();
        self
    }
    fn dead(mut self, s: &[(u32, u32)]) -> Self {
        self.dead = s.to_vec();
        self
    }
    fn not_kept(mut self, s: &[(u32, u32)]) -> Self {
        self.not_kept = s.to_vec();
        self
    }
}

/// The update's changes for `edit` over `prev`, and the new world's id of
/// each change (for the overlay).
fn changes_of(prev: &State, edit: &Edit) -> (Vec<SectionChange<'static>>, Vec<SectionId>) {
    let w = &prev.world;
    let n1 = w.headers.len();
    let mut next = n1;
    let w2 = world(w.target.machine, &edit.files);
    let anchors2 = edit.anchors.clone().unwrap_or_else(|| no_anchors(&edit.files));
    let mut out: Vec<(SectionChange<'static>, SectionId)> = Vec::new();
    for f in 1..=edit.files.len() as u32 {
        let secs = &edit.files[f as usize - 1];
        let changed = edit.changed.contains(&f);
        if !changed && !edit.drawn.contains(&f) {
            continue;
        }
        // The old version's identities: (anchor, name) -> ordinal -> id.
        let mut old: BTreeMap<(Anchor<'static>, &[u8], u32), SectionId> = BTreeMap::new();
        if f <= nfiles(w) {
            let mut seen: BTreeMap<(Anchor<'static>, &[u8]), u32> = BTreeMap::new();
            for i in 1..=file_len(w, f) {
                let s = w.sid(f, i);
                let name = w.headers[s].name;
                let anchor = prev.anchors[f as usize - 1][i as usize - 1];
                let n = seen.entry((anchor, name)).or_default();
                old.insert((anchor, name, *n), s);
                *n += 1;
            }
        }
        let mut seen: BTreeMap<(Anchor<'static>, &[u8]), u32> = BTreeMap::new();
        for (k, s) in secs.iter().enumerate() {
            let i = k as u32 + 1;
            let anchor = anchors2[f as usize - 1][k];
            let n = seen.entry((anchor, s.name)).or_default();
            let ordinal = *n;
            *n += 1;
            let (id, old_id) = if changed {
                let id = SectionId(next);
                next += 1;
                (id, old.get(&(anchor, s.name, ordinal)).copied())
            } else {
                let id = w.sid(f, i);
                (id, Some(id))
            };
            let c = SectionChange {
                id,
                file: FileId(f),
                index: i,
                key: SectionKey { anchor, name: s.name },
                ordinal,
                old: old_id,
                sh_type: s.ty,
                sh_flags: s.flags,
                size: s.size,
                align: 1u64 << s.align,
                live: !edit.dead.contains(&(f, i)),
                kept: !edit.not_kept.contains(&(f, i)),
            };
            out.push((c, w2.sid(f, i)));
        }
    }
    out.sort_by_key(|(c, _)| c.id);
    out.into_iter().unzip()
}

/// The new world of `edit`.
fn world_of(prev: &State, edit: &Edit) -> World {
    let mut w2 = world(prev.world.target.machine, &edit.files);
    for &(f, i) in edit.dead.iter().chain(&edit.not_kept) {
        let s = w2.sid(f, i);
        w2.liveness.live[s] = false;
    }
    w2
}

/// One update: `layout` over `prev`'s rows, laid over its placement and
/// checked. `Err` is the update's reason.
fn hop(prev: &State, edit: &Edit) -> Result<(State, LayoutDelta), IncrReason> {
    let (changes, ids2) = changes_of(prev, edit);
    let delta = layout(&StoredLayout::new(&prev.mem), &input(&changes), &prev.world.target)?;
    let next = overlay(prev, edit, &changes, &ids2, &delta);
    Ok((next, delta))
}

/// The input of `layout` with `changes` and no `.eh_frame` records,
/// merged pieces or slot demand.
fn input<'a>(changes: &'a [SectionChange<'a>]) -> LayoutInput<'a> {
    LayoutInput { changes, ..LayoutInput::default() }
}

/// The overlay reads the sparse placement through the seam's one reader
/// (`place_of`, `holes_of`; WP-216 R-3), for every section and chunk.
fn overlay(
    prev: &State,
    edit: &Edit,
    changes: &[SectionChange<'static>],
    ids2: &[SectionId],
    delta: &LayoutDelta,
) -> State {
    let w = &prev.world;
    let p1 = &prev.placed.placement;
    let w2 = world_of(prev, edit);
    let (a2, placed2) = w2.run(&Pool::sequential()).expect("the edited link links");
    let a2 = in_stored_ids(a2, &prev.assignment);
    let d = &delta.placement;
    let stored = StoredLayout::new(&prev.mem);
    let mut files: Vec<FileId> = changes.iter().map(|c| c.file).collect();
    files.sort_unstable();
    files.dedup();
    assert_eq!(delta.files, files, "the placed files, sorted");
    // Places, in the new world's ids.
    let mut place: Column<SectionId, Place> = Column::filled(w2.headers.len(), Place::NONE);
    let by_new: BTreeMap<SectionId, SectionId> =
        changes.iter().zip(ids2).map(|(c, &s2)| (s2, c.id)).collect();
    for f in 1..=nfiles(&w2) {
        for i in 1..=file_len(&w2, f) {
            let s2 = w2.sid(f, i);
            let id = by_new.get(&s2).copied().unwrap_or_else(|| w.sid(f, i));
            place[s2] = match delta.place_of(&stored, id).expect("the reader") {
                PlaceNow::Here(p) | PlaceNow::Stored(p) => p,
                PlaceNow::NotPlaced => Place::NONE,
            };
        }
    }
    // Holes: every chunk's, through the reader.
    assert!(delta.hole_chunks.windows(2).all(|w| w[0] < w[1]), "hole_chunks sorted");
    let mut holes: Vec<Hole> = Vec::new();
    for (c, _) in d.chunks.iter() {
        holes.extend(delta.holes_of(&stored, c).expect("the reader"));
    }
    holes.sort_by_key(|h| (h.chunk, h.offset));
    let placement = Placement {
        kind: d.kind,
        chunks: d.chunks.clone(),
        order: d.order.clone(),
        segments: d.segments.clone(),
        place,
        sym_place: d.sym_place.clone(),
        defined: d.defined.clone(),
        patches: d.patches.clone(),
        redirects: d.redirects.clone(),
        sized: p1.sized.clone(),
        tls: d.tls,
        entry: d.entry,
        holes: holes.into_boxed_slice(),
    };
    let placed = Placed { placement, rules: prev.placed.rules, ..placed2 };
    if let Err(v) = w2.check(&a2, &placed) {
        common::dump(&placed);
        panic!("the update's placement fails Layout::check: {v:?}");
    }
    let mut regions = prev.regions.clone();
    regions.extend(delta.regions_opened.iter().copied());
    let mem = capture(&w2, &a2, &placed.placement, &regions);
    let anchors = edit.anchors.clone().unwrap_or_else(|| no_anchors(&edit.files));
    State { world: w2, assignment: a2, placed, regions, mem, anchors }
}

/// `a` with the stored output section ids (the update keeps them; a fresh
/// assignment numbers them by first mention).
fn in_stored_ids(mut a: Assignment<'static>, stored: &Assignment<'static>) -> Assignment<'static> {
    use mortar_core::assignment::OutOf;
    let map: Vec<OutSecId> = a
        .outsecs
        .iter()
        .map(|(_, o)| {
            stored
                .outsecs
                .iter()
                .find(|(_, x)| x.name == o.name)
                .map(|(id, _)| id)
                .expect("an output section the stored link has")
        })
        .collect();
    assert_eq!(a.outsecs.len(), stored.outsecs.len(), "the same output sections");
    for o in a.out_of.as_mut_slice() {
        if let OutOf::Out(id) = o {
            *id = map[id.0 as usize];
        }
    }
    let mut outs = stored.outsecs.clone();
    for (old, o) in a.outsecs.iter() {
        outs[map[old.0 as usize]] = *o;
    }
    a.outsecs = outs;
    a
}

impl State {
    fn addr(&self, f: u32, i: u32) -> Option<u64> {
        self.placed.placement.section_addr(self.world.sid(f, i), 0)
    }
    fn files(&self) -> Vec<Vec<Sec>> {
        (1..=nfiles(&self.world))
            .map(|f| {
                (1..=file_len(&self.world, f))
                    .map(|i| {
                        let h = &self.world.headers[self.world.sid(f, i)];
                        Sec {
                            name: h.name,
                            ty: h.sh_type,
                            flags: h.flags,
                            size: h.size,
                            align: h.align,
                            link: 0,
                        }
                    })
                    .collect()
            })
            .collect()
    }
    fn out(&self, name: &[u8]) -> OutSecId {
        self.assignment.outsecs.iter().find(|(_, o)| o.name == name).map(|(id, _)| id).unwrap()
    }
}

fn not_implemented() -> IncrReason {
    IncrReason::UpdateNotImplemented(Some(Unimplemented::Layout))
}

// ---- item 2: keep, place, overflow ------------------------------------------

fn two_text_files() -> Vec<Vec<Sec>> {
    vec![
        vec![text(b".text.a", 100), text(b".text.b", 50), data(b".data", 16)],
        vec![text(b".text.c", 200), data(b".data", 8)],
    ]
}

// spec: incr.md IN-72 item 2 (keep: a shrunk section keeps its start; its
// freed tail is a fill hole exactly like slack, IN-3a, P-I5)
#[test]
fn hole_inside_kept_shrunk_section() {
    let s0 = first(Machine::X86_64, &two_text_files());
    let a0 = s0.addr(1, 1).unwrap();
    let b0 = s0.addr(1, 2).unwrap();
    // hop 1: a shrinks to 60.
    let mut files = s0.files();
    files[0][0].size = 60;
    let (s1, d1) = hop(&s0, &Edit::new(files.clone()).changed(&[1])).unwrap();
    assert_eq!(s1.addr(1, 1), Some(a0));
    assert_eq!(s1.addr(1, 2), Some(b0));
    assert!(d1.moved.is_empty() && d1.killed.is_empty() && d1.revived.is_empty(), "{d1:?}");
    let (chunk, off) = s1.placed.placement.place[s1.world.sid(1, 1)].get().unwrap();
    let freed = Hole { chunk, offset: off + 60, size: 40, kind: HoleKind::Fill };
    assert!(d1.holes.contains(&freed), "{:?}", d1.holes);
    assert!(s1.placed.placement.holes.contains(&freed));
    // hop 2: shrinks again; hop 3: grows back in place (its bytes are free).
    files[0][0].size = 20;
    let (s2, _) = hop(&s1, &Edit::new(files.clone()).changed(&[1])).unwrap();
    assert_eq!(s2.addr(1, 1), Some(a0));
    files[0][0].size = 100;
    let (s3, d3) = hop(&s2, &Edit::new(files).changed(&[1])).unwrap();
    assert_eq!(s3.addr(1, 1), Some(a0));
    assert!(d3.moved.is_empty());
    assert!(
        !s3.placed
            .placement
            .holes
            .iter()
            .any(|h| h.chunk == chunk && h.offset < off + 100 && h.offset + h.size > off)
    );
}

// spec: incr.md IN-72 item 2 (WP-211, review-wp207 §10: three sections of
// a region, the middle one grows; the others keep their addresses)
#[test]
fn region_with_three_sections_one_grows() {
    let files = vec![
        vec![data(b".data.a", 16), data(b".data.b", 16), data(b".data.c", 16)],
        vec![data(b".data.d", 16)],
    ];
    let s0 = first(Machine::X86_64, &files);
    let [a0, b0, c0] = [1, 2, 3].map(|i| s0.addr(1, i).unwrap());
    let mut files = s0.files();
    files[0][1].size = 40;
    let (s1, d1) = hop(&s0, &Edit::new(files.clone()).changed(&[1])).unwrap();
    assert_eq!(s1.addr(1, 1), Some(a0));
    assert_eq!(s1.addr(1, 3), Some(c0));
    let b1 = s1.addr(1, 2).unwrap();
    assert_eq!(b1, c0 + 16, "the lowest aligned free address after c");
    assert_eq!(s1.addr(2, 1), s0.addr(2, 1), "the other file never moves");
    assert_eq!(d1.moved.len(), 1);
    assert!(d1.regions_opened.is_empty() && d1.growth_steps.is_empty());
    // b's old bytes are a hole now.
    assert!(s1.placed.placement.holes.iter().any(|h| {
        let base = s1.placed.placement.chunks[h.chunk].addr.unwrap();
        base + h.offset == b0 && h.size == 16
    }));
    // hop 2: c grows into b's old place? No: c keeps (grows in place
    // only into free bytes after it, which b now holds), so it moves to the
    // lowest free address: b's old place is too small, the slack after b.
    files[0][2].size = 24;
    let (s2, _) = hop(&s1, &Edit::new(files.clone()).changed(&[1])).unwrap();
    assert_eq!(s2.addr(1, 1), Some(a0));
    assert_eq!(s2.addr(1, 2), Some(b1));
    // hop 3: back to the first version: b and c shrink and keep their places.
    files[0][1].size = 16;
    files[0][2].size = 16;
    let (s3, d3) = hop(&s2, &Edit::new(files).changed(&[1])).unwrap();
    assert!(d3.moved.is_empty());
    assert_eq!(s3.addr(1, 2), Some(b1));
}

// spec: incr.md IN-72 item 2 (M-4 (g): alignment padding counts against a
// region's capacity)
#[test]
fn alignment_padding_counts() {
    let mut a = text(b".text.a", 100);
    a.align = 0;
    let mut b = text(b".text.b", 10);
    b.align = 0;
    let mut big = text(b".text.z", 8);
    big.align = 5; // the output section's alignment is 32.
    // `.data` after `.text`: the RX PT_LOAD is not the last (the last
    // has no file reserve, WP-216 F-4).
    let s0 = first(Machine::X86_64, &[vec![a, b], vec![big, data(b".data", 8)]]);
    let a0 = s0.addr(1, 1).unwrap();
    // The region: 110 bytes and 64 of slack. b grows to 60 at alignment
    // 32: its old address (a0 + 100) is not aligned; the lowest aligned
    // address after a is a0 + 128 (when a0 is), and 128 + 60 > 174, so it
    // overflows, though 100 + 60 bytes would fit.
    assert_eq!(a0 % 32, 0, "the region starts aligned (the test's premise)");
    let mut files = s0.files();
    files[0][1].size = 60;
    files[0][1].align = 5;
    let (s1, d1) = hop(&s0, &Edit::new(files).changed(&[1])).unwrap();
    assert_eq!(d1.regions_opened.len(), 1, "{d1:?}");
    let r = d1.regions_opened[0];
    let b1 = s1.addr(1, 2).unwrap();
    let rstart = s1.placed.placement.chunks[r.chunk].addr.unwrap() + r.offset;
    assert_eq!(b1, rstart);
    assert_eq!(b1 % 32, 0);
    assert_eq!(s1.addr(1, 1), Some(a0));
}

// spec: incr.md IN-72 item 2 phase 3 (M-4 (a): a section in an (O, c)
// where the file had no region opens one at the tail)
#[test]
fn new_section_without_region_opens_tail_region() {
    // `.bss` after `.data`: `.data` is not its PT_LOAD's last chunk, so it
    // has its tail (address.rs: the last has none).
    let files = vec![
        vec![text(b".text.a", 100)],
        vec![text(b".text.c", 200), data(b".data", 8), bss(b".bss", 8)],
    ];
    let s0 = first(Machine::X86_64, &files);
    let data_out = s0.out(b".data");
    let mut files = s0.files();
    files[0].push(data(b".data", 16));
    let (s1, d1) = hop(&s0, &Edit::new(files.clone()).changed(&[1])).unwrap();
    assert_eq!(d1.regions_opened.len(), 1);
    let r = d1.regions_opened[0];
    assert_eq!((r.file, r.out, r.category, r.ordinal), (FileId(1), data_out, 0, 0));
    assert_eq!(r.capacity, 16 + 64);
    assert!(d1.revived.len() == 1, "the new section gained a place: {d1:?}");
    assert_eq!(s1.addr(2, 2), s0.addr(2, 2));
    assert_eq!(s1.addr(1, 1), s0.addr(1, 1));
    // hop 2: it grows inside its new region; hop 3: it goes, a hole.
    let d_addr = s1.addr(1, 2).unwrap();
    files[0][1].size = 70;
    let (s2, d2) = hop(&s1, &Edit::new(files.clone()).changed(&[1])).unwrap();
    assert_eq!(s2.addr(1, 2), Some(d_addr));
    assert!(d2.regions_opened.is_empty());
    files[0].pop();
    let (s3, d3) = hop(&s2, &Edit::new(files).changed(&[1])).unwrap();
    assert_eq!(d3.killed.len(), 1);
    let (chunk, off) = (r.chunk, r.offset);
    assert!(
        s3.placed
            .placement
            .holes
            .iter()
            .any(|h| h.chunk == chunk && h.offset == off && h.size == r.capacity)
    );
}

// spec: incr.md IN-72 item 3 (M-4 (b): several files' new regions in one
// tail are opened in load order of the files)
#[test]
fn two_files_overflow_in_load_order() {
    let files = vec![
        vec![text(b".text.a", 100)],
        vec![text(b".text.b", 100), data(b".data", 8)],
        vec![text(b".text.c", 100)],
    ];
    let s0 = first(Machine::X86_64, &files);
    let mut files = s0.files();
    files[0][0].size = 400;
    files[2][0].size = 300;
    let (s1, d1) = hop(&s0, &Edit::new(files.clone()).changed(&[3, 1])).unwrap();
    assert_eq!(d1.regions_opened.len(), 2);
    let [r1, r3] = [d1.regions_opened[0], d1.regions_opened[1]];
    assert_eq!((r1.file, r3.file), (FileId(1), FileId(3)));
    assert!(r1.offset < r3.offset, "file 1's region first");
    assert!(s1.addr(1, 1).unwrap() < s1.addr(3, 1).unwrap());
    assert_eq!(s1.addr(2, 1), s0.addr(2, 1));
    // The tail of `.text` is the last chunk of its PT_LOAD: a growth step.
    assert_eq!(d1.growth_steps.len(), 1);
    // hops 2, 3: both shrink back, keeping their overflow places; then a
    // new section of file 3 takes its first region again (phase 2 (ii)).
    files[0][0].size = 100;
    files[2][0].size = 100;
    let (s2, d2) = hop(&s1, &Edit::new(files.clone()).changed(&[1, 3])).unwrap();
    assert!(d2.moved.is_empty() && d2.regions_opened.is_empty());
    files[2].push(text(b".text.d", 80));
    let (s3, _) = hop(&s2, &Edit::new(files).changed(&[3])).unwrap();
    assert_eq!(s3.addr(3, 2), s0.addr(3, 1), "the first region, free again, is reused");
}

// spec: incr.md IN-72 item 2 (M-4 (c): a file keeps every region it had;
// an old region a section left is reused through phase 2)
#[test]
fn old_region_reused_after_overflow() {
    let s0 = first(Machine::X86_64, &two_text_files());
    let a0 = s0.addr(1, 1).unwrap();
    let mut files = s0.files();
    // hop 1: a and b grow past the region: both overflow, region 0 empty.
    files[0][0].size = 300;
    files[0][1].size = 250;
    let (s1, d1) = hop(&s0, &Edit::new(files.clone()).changed(&[1])).unwrap();
    assert_eq!(d1.regions_opened.len(), 1);
    assert_eq!(d1.regions_opened[0].ordinal, 1);
    assert_ne!(s1.addr(1, 1), Some(a0));
    // hop 2: a new section fits region 0 (first in the regions' order).
    files[0].push(text(b".text.n", 120));
    let (s2, d2) = hop(&s1, &Edit::new(files.clone()).changed(&[1])).unwrap();
    assert_eq!(s2.addr(1, 4), Some(a0));
    assert!(d2.regions_opened.is_empty());
    // hop 3: a shrinks: kept in region 1.
    files[0][0].size = 50;
    let (s3, d3) = hop(&s2, &Edit::new(files).changed(&[1])).unwrap();
    assert_eq!(s3.addr(1, 1), s1.addr(1, 1));
    assert!(d3.moved.is_empty());
}

// spec: incr.md IN-72 item 3 (the growth step: A is the largest alignment
// of the chunks after it in its PT_LOAD, the maximum page size when the
// RELRO end moves)
#[test]
fn growth_step_alignment_and_relro() {
    let mut aligned = rodata(b".lrodata_like", 64);
    aligned.name = b".gcc_except_table";
    aligned.align = 6;
    let files = vec![
        vec![rodata(b".rodata", 64), aligned, sec(b".data.rel.ro", SHT_PROGBITS, WA, 64, 3)],
        vec![rodata(b".rodata", 32), data(b".data", 8)],
    ];
    let s0 = first(Machine::X86_64, &files);
    let page = s0.placed.rules.max_page;
    // .data.rel.ro grows past its region and its tail (4096): a growth
    // step, a multiple of the page (the RELRO end moves).
    let mut files = s0.files();
    files[0][2].size = 64 + 8192;
    let (s1, d1) = hop(&s0, &Edit::new(files.clone()).changed(&[1])).unwrap();
    assert_eq!(d1.growth_steps.len(), 1, "{d1:?}");
    let g = d1.growth_steps[0];
    assert_eq!(g.out, s0.out(b".data.rel.ro"));
    assert_eq!(g.step % page, 0);
    // .rodata grows past its tail: the step is a multiple of the largest
    // alignment after it in the R PT_LOAD.
    let r_out = s0.out(b".rodata");
    let followers: Vec<u8> = {
        let p = &s1.placed.placement;
        let last = p.order.iter().rposition(|&c| p.chunks[c].kind.out() == Some(r_out)).unwrap();
        let class = p.chunks[p.order[last]].load_class();
        p.order[last + 1..]
            .iter()
            .map(|&c| &p.chunks[c])
            .take_while(|c| c.load_class() == class)
            .map(|c| c.align)
            .collect()
    };
    files[0][0].size = 64 + 8192;
    let (s2, d2) = hop(&s1, &Edit::new(files.clone()).changed(&[1])).unwrap();
    let g = d2.growth_steps.iter().find(|g| g.out == r_out).expect("a step for .rodata");
    let a = followers.iter().map(|&x| 1u64 << x).max().unwrap_or(1);
    assert_eq!(g.step % a, 0, "step {:#x} by {a}", g.step);
    // hop 3: both shrink back: nothing moves, nothing steps.
    files[0][0].size = 64;
    files[0][2].size = 64;
    let (_, d3) = hop(&s2, &Edit::new(files).changed(&[1])).unwrap();
    assert!(d3.growth_steps.is_empty() && d3.moved.is_empty());
}

// spec: incr.md IN-72 item 10 (an output section's alignment growing past
// its stored address's: `reserve-exhausted`)
#[test]
fn alignment_growth_is_reserve_exhausted() {
    let s0 = first(Machine::X86_64, &two_text_files());
    let mut files = s0.files();
    files[0][1].align = 12;
    let e = hop(&s0, &Edit::new(files).changed(&[1])).err().unwrap();
    assert_eq!(e.name(), "reserve-exhausted", "{e:?}");
}

// spec: incr.md IN-72 item 6 (merged inputs take no space in a region: a
// SHF_MERGE section's pieces are item 6's, which the seam does not carry)
#[test]
fn merged_input_not_in_region() {
    let s0 = first(Machine::X86_64, &two_text_files());
    let mut files = s0.files();
    files[0].push(sec(b".rodata.str1.1", SHT_PROGBITS, SHF_ALLOC | SHF_MERGE | SHF_STRINGS, 12, 0));
    files[1].push(rodata(b".rodata", 8));
    let s0 = first(Machine::X86_64, &{
        let mut f = two_text_files();
        f[1].push(rodata(b".rodata", 8));
        f
    });
    let e = hop(&s0, &Edit::new(files).changed(&[1])).err().unwrap();
    assert_eq!(e, not_implemented(), "item 6: refused by name, never placed in a region");
}

// spec: incr.md IN-72 item 6 (a merged constant dies and revives: its
// fragment is rewritten from the piece; not in the seam: the fall-back)
#[test]
fn merged_constant_dies_revives() {
    let cst = sec(b".rodata.cst8", SHT_PROGBITS, SHF_ALLOC | SHF_MERGE, 8, 3);
    let mut base = two_text_files();
    base[1].push(rodata(b".rodata", 8));
    let s0 = first(Machine::X86_64, &base);
    let mut files = s0.files();
    files[0].push(cst);
    let e = hop(&s0, &Edit::new(files).changed(&[1])).err().unwrap();
    assert_eq!(e, not_implemented());
}

// spec: incr.md IN-71, IN-72 (COMDAT election runs before item 2: a copy
// not kept takes no region space; the keeper moving to an unchanged file
// draws it in and its copy is placed by item 2)
#[test]
fn comdat_election_before_region() {
    let files = vec![
        vec![text(b".text.a", 100), text(b".text.inl", 40)],
        vec![text(b".text.b", 100), text(b".text.inl", 40)],
    ];
    let w = {
        let mut w = world(Machine::X86_64, &files);
        let s = w.sid(2, 2);
        w.liveness.live[s] = false; // not kept: file 1 keeps the group.
        w
    };
    let s0 = first_with(w, no_anchors(&files));
    assert!(s0.addr(2, 2).is_none());
    // hop 1: file 2 changes; its copy grows but is still not kept.
    let mut files = s0.files();
    files[1][1].size = 400;
    let (s1, d1) = hop(&s0, &Edit::new(files.clone()).changed(&[2]).not_kept(&[(2, 2)])).unwrap();
    assert!(d1.regions_opened.is_empty(), "a copy not kept takes no space: {d1:?}");
    assert!(s1.addr(2, 2).is_none());
    assert_eq!(s1.addr(2, 1), s0.addr(2, 1));
    let _ = s1;
}

// spec: incr.md IN-72 item 2a (R8: a COMDAT keeper that moves to an
// unchanged file's copy draws it in; its copy is placed by phases 2-3)
#[test]
fn comdat_keeper_moves_to_unchanged_file() {
    let files = vec![
        vec![text(b".text.a", 100), text(b".text.inl", 40)],
        vec![text(b".text.b", 100), text(b".text.inl", 40)],
    ];
    let w = {
        let mut w = world(Machine::X86_64, &files);
        let s = w.sid(2, 2);
        w.liveness.live[s] = false;
        w
    };
    let s0 = first_with(w, no_anchors(&files));
    // hop 1: file 1 drops its copy; file 2 (unchanged) is drawn in, its
    // copy now kept.
    let mut files = s0.files();
    files[0].pop();
    let (s1, d1) = hop(&s0, &Edit::new(files.clone()).changed(&[1]).drawn(&[2])).unwrap();
    assert!(s1.addr(2, 2).is_some());
    assert_eq!(s1.addr(2, 1), s0.addr(2, 1), "the drawn-in file's other section keeps its place");
    assert!(d1.revived.contains(&s0.world.sid(2, 2)));
    assert_eq!(d1.killed.len(), 1);
    // hop 2: file 1 takes its copy back and keeps it; file 2 drawn in, its
    // copy not kept: killed, a hole.
    files[0].push(text(b".text.inl", 40));
    let (s2, d2) =
        hop(&s1, &Edit::new(files.clone()).changed(&[1]).drawn(&[2]).not_kept(&[(2, 2)])).unwrap();
    assert!(s2.addr(2, 2).is_none());
    assert!(d2.killed.contains(&s1.world.sid(2, 2)));
    assert!(s2.addr(1, 2).is_some());
    // hop 3: nothing flips; file 2 drawn in for another reason: no moves.
    let (_, d3) = hop(&s2, &Edit::new(files).drawn(&[2]).not_kept(&[(2, 2)])).unwrap();
    assert!(d3.moved.is_empty() && d3.killed.is_empty() && d3.revived.is_empty());
}

// spec: incr.md IN-72 item 2a (W8, R6: a section killed by
// --gc-sections and revived; the first link without it, too)
#[test]
fn gc_section_dies_then_revives() {
    let s0 = first(Machine::X86_64, &two_text_files());
    let b0 = s0.addr(1, 2).unwrap();
    let files = s0.files();
    // hop 1: b dies (file 1 drawn in).
    let (s1, d1) = hop(&s0, &Edit::new(files.clone()).drawn(&[1]).dead(&[(1, 2)])).unwrap();
    assert_eq!(d1.killed, vec![s0.world.sid(1, 2)]);
    assert!(s1.addr(1, 2).is_none());
    // hop 2: it revives where it was (the lowest free address).
    let (s2, d2) = hop(&s1, &Edit::new(files.clone()).drawn(&[1])).unwrap();
    assert_eq!(d2.revived, vec![s1.world.sid(1, 2)]);
    assert_eq!(s2.addr(1, 2), Some(b0));
    // The first link without it: it revives into the region's slack.
    let w = {
        let mut w = world(Machine::X86_64, &files);
        let s = w.sid(1, 2);
        w.liveness.live[s] = false;
        w
    };
    let t0 = first_with(w, no_anchors(&files));
    assert!(t0.addr(1, 2).is_none());
    let (t1, d) = hop(&t0, &Edit::new(files).drawn(&[1])).unwrap();
    assert!(d.regions_opened.is_empty(), "50 bytes fit the 64 of slack");
    assert_eq!(t1.addr(1, 2), Some(t0.addr(1, 1).unwrap() + 100));
}

// spec: incr.md IN-72 item 2a (S-74: a demoted global draws its file in;
// its sections keep their places)
#[test]
fn n2_visibility_demotes_unchanged_definer() {
    let s0 = first(Machine::X86_64, &two_text_files());
    let files = s0.files();
    let mut s = s0;
    for _ in 0..3 {
        let (next, d) = hop(&s, &Edit::new(files.clone()).drawn(&[2])).unwrap();
        assert!(d.moved.is_empty() && d.killed.is_empty() && d.revived.is_empty());
        assert!(d.regions_opened.is_empty() && d.growth_steps.is_empty());
        assert_eq!(next.addr(2, 1), s.addr(2, 1));
        s = next;
    }
}

// ---- items 4, 8, 10 ----------------------------------------------------------

// spec: incr.md IN-72 item 10 (review-in72 m-2: an allocated note changing
// size is `property-changed`; the same size keeps its place)
#[test]
fn note_abi_tag_size_change() {
    let note = sec(b".note.ABI-tag", SHT_NOTE, SHF_ALLOC, 32, 2);
    let mut files = two_text_files();
    files[0].push(note);
    let s0 = first(Machine::X86_64, &files);
    let n0 = s0.addr(1, 4);
    assert!(n0.is_some());
    let (s1, _) = hop(&s0, &Edit::new(files.clone()).changed(&[1])).unwrap();
    assert_eq!(s1.addr(1, 4), n0);
    files[0][3].size = 36;
    let e = hop(&s1, &Edit::new(files).changed(&[1])).err().unwrap();
    assert_eq!(e.name(), "property-changed", "{e:?}");
}

// spec: incr.md IN-72 item 8 (a TLS section that changes size:
// `tls-layout-changed`)
#[test]
fn tls_size_change_is_tls_layout_changed() {
    let tdata = sec(b".tdata", SHT_PROGBITS, WA | SHF_TLS, 16, 3);
    let mut files = two_text_files();
    files[1].push(tdata);
    let s0 = first(Machine::X86_64, &files);
    let (s1, _) = hop(&s0, &Edit::new(files.clone()).changed(&[2])).unwrap();
    assert_eq!(s1.placed.placement.tls, s0.placed.placement.tls);
    files[1][2].size = 24;
    let e = hop(&s1, &Edit::new(files).changed(&[2])).err().unwrap();
    assert_eq!(e.name(), "tls-layout-changed", "{e:?}");
}

// spec: incr.md IN-72 item 4 (WP-211: an SHF_LINK_ORDER section whose
// target dies: `link-order-moved`)
#[test]
fn link_order_target_dies() {
    let mut lo = sec(b"__patchable_function_entries", SHT_PROGBITS, WA | SHF_LINK_ORDER, 8, 3);
    lo.link = 2;
    let mut files = two_text_files();
    files[0].push(lo);
    let s0 = first(Machine::X86_64, &files);
    let (_, d) = hop(&s0, &Edit::new(files.clone()).drawn(&[1])).unwrap();
    assert!(d.moved.is_empty());
    let e = hop(&s0, &Edit::new(files.clone()).drawn(&[1]).dead(&[(1, 2)])).err().unwrap();
    assert_eq!(e.name(), "link-order-moved", "{e:?}");
    let e = hop(&s0, &Edit::new(files).drawn(&[1]).dead(&[(1, 2), (1, 4)])).err().unwrap();
    assert_eq!(e.name(), "link-order-moved", "{e:?}");
}

// spec: incr.md IN-72 item 5 (WP-211: an unchanged file's FDE whose
// function dies; WP-243: the file is drawn in, its `EhFile` has no record
// left (its CIE goes with its last FDE), its range becomes a hole merged
// with the pad before it; without its `EhFile`, refused)
#[test]
fn cross_file_eh_frame_fde_target_dies() {
    let eh = sec(b".eh_frame", SHT_PROGBITS, SHF_ALLOC, 56, 3);
    let mut files = two_text_files();
    files[1].push(eh);
    let (s0, c) = first_with_eh(&files);
    let edit = Edit::new(files).drawn(&[2]).dead(&[(2, 1)]);
    let (changes, _) = changes_of(&s0, &edit);
    let stored = StoredLayout::new(&s0.mem);
    let t = &s0.world.target;
    assert_eq!(layout(&stored, &input(&changes), t).err(), Some(not_implemented()));
    let gone = [eh_file(2, &[])];
    let d = layout(&stored, &LayoutInput { eh: &gone, ..input(&changes) }, t).unwrap();
    assert!(d.eh.as_ref().is_some_and(|e| !e.whole && e.ranges.is_empty()));
    let pad = Hole { chunk: c, offset: 64, size: 72, kind: HoleKind::EhPad };
    assert_eq!(d.holes_of(&stored, c).unwrap(), vec![pad]);
    // The `.eh_frame` input section is placed nowhere, as in the full link.
    let sid = s0.world.sid(2, 3);
    assert_eq!(d.place_of(&stored, sid).unwrap(), PlaceNow::NotPlaced);
}

// spec: incr.md IN-72 item 10 (a section with no output section in the
// stored layout: `new-output-section`)
#[test]
fn section_without_output_is_new_output_section() {
    let s0 = first(Machine::X86_64, &two_text_files());
    let mut files = s0.files();
    files[0].push(sec(b"my_section", SHT_PROGBITS, WA, 8, 3));
    let (changes, _) = changes_of(&s0, &Edit::new(files).changed(&[1]));
    let e = layout(&StoredLayout::new(&s0.mem), &input(&changes), &s0.world.target).err().unwrap();
    assert_eq!(e.name(), "new-output-section", "{e:?}");
}

// spec: incr.md IN-72 item 9 (the island keeps its first-link capacity;
// `_etext` (DEFINED) does not move in U7 but by a growth step; a tail in
// the island cannot take a region: reserve-exhausted, WP-216 F-3)
#[test]
fn thunk_growth_moves_etext() {
    let files =
        vec![vec![text(b".text.a", 100), text(b".text.b", 40)], vec![text(b".text.c", 200)]];
    let s0 = first(Machine::AArch64, &files);
    let p0 = &s0.placed.placement;
    let island =
        p0.order.iter().copied().find(|&c| matches!(p0.chunks[c].kind, ChunkKind::Patches(_)));
    let island = island.expect("an AArch64 .text has an island");
    let mut files = s0.files();
    files[0][1].size = 60; // grows in place inside the region's slack.
    let (s1, d1) = hop(&s0, &Edit::new(files.clone()).changed(&[1])).unwrap();
    assert_eq!(s1.placed.placement.chunks[island], p0.chunks[island], "the island is not grown");
    assert_eq!(s1.placed.placement.defined, p0.defined);
    assert!(d1.growth_steps.is_empty());
    files[0][1].size = 4000;
    let e = hop(&s1, &Edit::new(files).changed(&[1])).err().unwrap();
    assert_eq!(e.name(), "reserve-exhausted", "{e:?}");
}

// ---- IN-32's L10: grow and shrink past each capacity ------------------------

// spec: incr.md IN-32 (L10: a section grows past its region, the tail and
// the segment reserve in turn, and shrinks back)
#[test]
fn l10_grow_and_shrink_past_each_capacity() {
    // `.fini` after `.text` in the RX PT_LOAD: `.text` keeps its tail;
    // `.data` after it: the RX PT_LOAD has its reserve.
    let files = vec![
        vec![text(b".text", 256)],
        vec![text(b".text", 256), text(b".fini", 16), data(b".data", 8), bss(b".bss", 8)],
    ];
    let s0 = first(Machine::X86_64, &files);
    let mut files = s0.files();
    let mut s = s0;
    // In the region's slack (256 + 64).
    files[0][0].size = 310;
    let (n, d) = hop(&s, &Edit::new(files.clone()).changed(&[1])).unwrap();
    assert!(d.regions_opened.is_empty() && d.growth_steps.is_empty());
    s = n;
    // Past the region, inside the tail.
    files[0][0].size = 1000;
    let (n, d) = hop(&s, &Edit::new(files.clone()).changed(&[1])).unwrap();
    assert_eq!(d.regions_opened.len(), 1);
    assert!(d.growth_steps.is_empty(), "{d:?}");
    s = n;
    // Past the tail: a growth step.
    files[0][0].size = 8000;
    let (n, d) = hop(&s, &Edit::new(files.clone()).changed(&[1])).unwrap();
    assert_eq!(d.growth_steps.len(), 1, "{d:?}");
    s = n;
    // Past the segment reserve: reserve-exhausted.
    files[0][0].size = 1 << 24;
    let e = hop(&s, &Edit::new(files.clone()).changed(&[1])).err().unwrap();
    assert_eq!(e.name(), "reserve-exhausted");
    // Shrink back: kept where it is.
    files[0][0].size = 256;
    let at = s.addr(1, 1);
    let (n, d) = hop(&s, &Edit::new(files).changed(&[1])).unwrap();
    assert!(d.moved.is_empty() && d.growth_steps.is_empty());
    assert_eq!(n.addr(1, 1), at);
}

// ---- O(change), determinism -------------------------------------------------

/// `LayoutRows` that counts the per-key reads.
struct Counting<'a> {
    inner: &'a MemoryLayout,
    sections: AtomicUsize,
    regions: AtomicUsize,
    holes: AtomicUsize,
    frags: AtomicUsize,
    /// `FILES`' eh columns, `FRAG_USE` and `KEY_REFS` ranges.
    ranges: AtomicUsize,
    /// `SLOTS` (whole).
    slots: AtomicUsize,
}

impl<'a> Counting<'a> {
    fn new(inner: &'a MemoryLayout) -> Self {
        Self {
            inner,
            sections: AtomicUsize::new(0),
            regions: AtomicUsize::new(0),
            holes: AtomicUsize::new(0),
            frags: AtomicUsize::new(0),
            ranges: AtomicUsize::new(0),
            slots: AtomicUsize::new(0),
        }
    }
}

impl LayoutRows for Counting<'_> {
    fn chunks(&self) -> Result<Vec<(ChunkId, Chunk)>, IncrReason> {
        self.inner.chunks()
    }
    fn order(&self) -> Result<Vec<ChunkId>, IncrReason> {
        self.inner.order()
    }
    fn segments(&self) -> Result<Vec<Segment>, IncrReason> {
        self.inner.segments()
    }
    fn outsecs(&self) -> Result<Vec<(OutSecId, Vec<u8>)>, IncrReason> {
        self.inner.outsecs()
    }
    fn sections_of(&self, file: FileId) -> Result<Vec<StoredSection>, IncrReason> {
        self.sections.fetch_add(1, Ordering::Relaxed);
        self.inner.sections_of(file)
    }
    fn regions_of(&self, file: FileId) -> Result<Vec<StoredRegion>, IncrReason> {
        self.regions.fetch_add(1, Ordering::Relaxed);
        self.inner.regions_of(file)
    }
    fn holes_in(&self, chunk: ChunkId) -> Result<Vec<Hole>, IncrReason> {
        self.holes.fetch_add(1, Ordering::Relaxed);
        self.inner.holes_in(chunk)
    }
    fn frags_of(
        &self,
        out: OutSecId,
        class: u32,
        hash: [u8; 16],
    ) -> Result<Option<mortar_core::incr::StoredFrag>, IncrReason> {
        self.frags.fetch_add(1, Ordering::Relaxed);
        self.inner.frags_of(out, class, hash)
    }
    fn frag_uses_of(&self, file: FileId) -> Result<Vec<StoredFragUse>, IncrReason> {
        self.ranges.fetch_add(1, Ordering::Relaxed);
        self.inner.frag_uses_of(file)
    }
    fn eh_range_of(&self, file: FileId) -> Result<Option<StoredEhRange>, IncrReason> {
        self.ranges.fetch_add(1, Ordering::Relaxed);
        self.inner.eh_range_of(file)
    }
    fn key_refs_of(&self, file: FileId) -> Result<Vec<StoredKeyRef>, IncrReason> {
        self.ranges.fetch_add(1, Ordering::Relaxed);
        self.inner.key_refs_of(file)
    }
    fn section(&self, id: SectionId) -> Result<Option<StoredSection>, IncrReason> {
        self.inner.section(id)
    }
    fn slots(&self) -> Result<Vec<StoredSlot>, IncrReason> {
        self.slots.fetch_add(1, Ordering::Relaxed);
        self.inner.slots()
    }
    fn patches(&self) -> Result<Vec<Patch>, IncrReason> {
        self.inner.patches()
    }
    fn redirects(&self) -> Result<Vec<Redirect>, IncrReason> {
        self.inner.redirects()
    }
    fn sym_place(&self) -> Result<Vec<SymPlace>, IncrReason> {
        self.inner.sym_place()
    }
    fn defined(&self) -> Result<Vec<Defined>, IncrReason> {
        self.inner.defined()
    }
    fn tls_and_entry(&self) -> Result<(Option<Tls>, Option<u64>), IncrReason> {
        self.inner.tls_and_entry()
    }
    fn symtab_block(&self, file: FileId) -> Result<Option<SymtabBlock>, IncrReason> {
        self.inner.symtab_block(file)
    }
    fn symtab_tables(&self) -> Result<(SymtabBlock, SymtabTables), IncrReason> {
        self.inner.symtab_tables()
    }
}

// spec: incr.md IN-72 (rows of unchanged files are read only where the
// delta reaches them; IN-18c: O(change))
#[test]
fn reads_only_what_changed() {
    let files: Vec<Vec<Sec>> = (0..60)
        .map(|_| {
            vec![text(b".text", 64), data(b".data", 16), bss(b".bss", 32), rodata(b".rodata", 8)]
        })
        .collect();
    let s0 = first(Machine::X86_64, &files);
    let mut files = s0.files();
    files[29][0].size = 70;
    files[29][1].size = 600; // overflows .data: a new region.
    let (changes, _) = changes_of(&s0, &Edit::new(files).changed(&[30]).drawn(&[7]));
    let rows = Counting::new(&s0.mem);
    let d = layout(&StoredLayout::new(&rows), &input(&changes), &s0.world.target).unwrap();
    assert_eq!(rows.sections.load(Ordering::Relaxed), 2, "one SECTIONS range per file");
    assert_eq!(rows.regions.load(Ordering::Relaxed), 2, "one REGIONS range per file");
    // The chunks of the two files' regions (.text, .data, .bss, .rodata)
    // and the tail chunk of .data: at most 4.
    assert!(rows.holes.load(Ordering::Relaxed) <= 4, "{}", rows.holes.load(Ordering::Relaxed));
    assert_eq!(rows.frags.load(Ordering::Relaxed), 0);
    assert_eq!(rows.ranges.load(Ordering::Relaxed), 6, "three ranges per file (items 5-7)");
    assert_eq!(rows.slots.load(Ordering::Relaxed), 1, "SLOTS once: a file has a new version");
    assert_eq!(d.regions_opened.len(), 1);
    // A file drawn in alone, with no slot demand old or new: SLOTS unread.
    let (changes, _) = changes_of(&s0, &Edit::new(s0.files()).drawn(&[7]));
    let rows = Counting::new(&s0.mem);
    layout(&StoredLayout::new(&rows), &input(&changes), &s0.world.target).unwrap();
    assert_eq!(rows.slots.load(Ordering::Relaxed), 0);
    assert_eq!(rows.frags.load(Ordering::Relaxed), 0);
}

// spec: incr.md IN-72 item 6 (IN-18c: the `FRAGS` row of each key whose
// count changes, and nothing of the other files' fragments)
#[test]
fn item6_reads_only_changed_keys() {
    let model: Vec<MFile> = (0..40).map(|_| mfile(&[1, 2])).collect();
    let s0 = mfirst(&model, &[]);
    let mut next = model.clone();
    next[19] = mfile(&[1, 3]);
    let (changes, pieces) = minput(&s0, &next, &[20]);
    let rows = Counting::new(&s0.mem);
    let inp = LayoutInput { changes: &changes, pieces: &pieces, ..LayoutInput::default() };
    let d = layout(&StoredLayout::new(&rows), &inp, &s0.world.target).unwrap();
    assert_eq!(rows.frags.load(Ordering::Relaxed), 2, "keys 2 (40 to 39) and 3 (new)");
    assert_eq!(rows.ranges.load(Ordering::Relaxed), 3, "one file's three ranges");
    assert_eq!(rows.sections.load(Ordering::Relaxed), 1);
    assert_eq!(rows.holes.load(Ordering::Relaxed), 0, "no fragment died or revived");
    assert_eq!(d.frags.len(), 2, "{:?}", d.frags);
}

// spec: incr.md IN-72 (determinism, IN-6: no dependence on the thread
// count)
#[test]
fn delta_layout_same_on_1_and_n_threads() {
    let (s0, hops) = m1_world("o2fs");
    let (changes, _) = changes_of(&s0, &hops[3]);
    let one = layout(&StoredLayout::new(&s0.mem), &input(&changes), &s0.world.target);
    let many: Vec<_> = std::thread::scope(|sc| {
        let h: Vec<_> = (0..4)
            .map(|_| {
                sc.spawn(|| layout(&StoredLayout::new(&s0.mem), &input(&changes), &s0.world.target))
            })
            .collect();
        h.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert!(one.is_ok(), "{one:?}");
    for m in many {
        assert_eq!(m, one);
    }
    // And the stored layout of a parallel first link gives the same delta.
    let w = world(Machine::X86_64, &s0.files());
    let (a, placed) = w.run(&Pool::new(4)).unwrap();
    let regions = regions_of_full_link(&w, &a, &placed.placement);
    let mem = capture(&w, &a, &placed.placement, &regions);
    assert_eq!(mem, s0.mem, "the first link is the same on 4 threads");
}

// ---- the M.1 series (IN-72's table) -----------------------------------------

/// The fixture (`tests/data/m1.txt`, from `tests/data/m1_extract.py`):
/// per directory, main.o and the five strutil.o versions.
struct Obj {
    secs: Vec<(u32, Sec, Anchor<'static>)>,
    syms: Vec<(&'static [u8], u32, u64)>,
}

fn leak(s: &str) -> &'static [u8] {
    Box::leak(s.as_bytes().to_vec().into_boxed_slice())
}

fn m1_fixture(dir: &str) -> BTreeMap<String, Obj> {
    let text = include_str!("data/m1.txt");
    let mut out = BTreeMap::new();
    let mut in_dir = false;
    let mut cur: Option<String> = None;
    for l in text.lines() {
        let p: Vec<&str> = l.split(' ').collect();
        match p[0] {
            "dir" => in_dir = p[1] == dir,
            "file" if in_dir => {
                cur = Some(p[1].to_owned());
                out.insert(p[1].to_owned(), Obj { secs: Vec::new(), syms: Vec::new() });
            }
            "sec" if in_dir => {
                let o = out.get_mut(cur.as_ref().unwrap()).unwrap();
                let flags = u64::from_str_radix(p[4].trim_start_matches("0x"), 16).unwrap();
                let align: u64 = p[6].parse().unwrap();
                let s = Sec {
                    name: leak(p[2]),
                    ty: p[3].parse().unwrap(),
                    flags: flags & !0x200, // SHF_GROUP: the World has no groups.
                    size: p[5].parse().unwrap(),
                    align: align.trailing_zeros() as u8,
                    link: 0,
                };
                let anchor = match p[7].split_once(':') {
                    Some(("comdat", n)) => Anchor::Comdat(leak(n)),
                    Some(("global", n)) => Anchor::Global(leak(n)),
                    Some(("local", n)) => Anchor::Local(leak(n)),
                    _ => Anchor::None,
                };
                o.secs.push((p[1].parse().unwrap(), s, anchor));
            }
            "sym" if in_dir => {
                let o = out.get_mut(cur.as_ref().unwrap()).unwrap();
                o.syms.push((leak(p[1]), p[2].parse().unwrap(), p[3].parse().unwrap()));
            }
            _ => {}
        }
    }
    out
}

/// The sections the model places (`model_region.py`): allocated
/// PROGBITS/NOBITS, not empty, not merged, not `.eh_frame`, in a
/// hole-tolerant output section; with their ELF index.
fn m1_sections(o: &Obj) -> (Vec<Sec>, Vec<Anchor<'static>>, Vec<u32>) {
    const TOL: [&[u8]; 6] =
        [b".text", b".rodata", b".data.rel.ro", b".data", b".bss", b".gcc_except_table"];
    let mut secs = Vec::new();
    let mut anchors = Vec::new();
    let mut index = Vec::new();
    for &(i, s, a) in &o.secs {
        let out = TOL.iter().find(|t| mortar_layout::assign::matches_prefix(s.name, t));
        if s.size == 0 || s.flags & SHF_MERGE != 0 || s.name == b".eh_frame" || out.is_none() {
            continue;
        }
        if s.ty != SHT_PROGBITS && s.ty != SHT_NOBITS {
            continue;
        }
        secs.push(s);
        anchors.push(a);
        index.push(i);
    }
    (secs, anchors, index)
}

/// The first link of `main.o`, `strutil_base.o` and a stand-in for the rest of
/// protoc's files (its `.data.rel.ro`, `.data` and `.bss`, which addfn's
/// new `.data.rel.ro` needs, M-4 (a)), on x86-64 (no islands, so `.text`'s
/// tail is its body's), and the five hops' edits.
fn m1_world(dir: &str) -> (State, Vec<Edit>) {
    let fx = m1_fixture(dir);
    let (main, main_a, _) = m1_sections(&fx["main"]);
    let version = |v: &str| m1_sections(&fx[&format!("strutil_{v}")]);
    let (base, base_a, _) = version("base");
    let rest = vec![
        sec(b".data.rel.ro", SHT_PROGBITS, WA, 4096, 3),
        data(b".data", 4096),
        bss(b".bss", 4096),
    ];
    let rest_a = vec![Anchor::None; rest.len()];
    let files = vec![main.clone(), base, rest.clone()];
    let anchors = vec![main_a.clone(), base_a, rest_a.clone()];
    let s0 = first_with(world(Machine::X86_64, &files), anchors);
    let hops = ["grow1", "grow2", "rmfn", "addfn", "base"]
        .iter()
        .map(|v| {
            let (secs, a, _) = version(v);
            let mut e = Edit::new(vec![main.clone(), secs, rest.clone()]).changed(&[2]);
            e.anchors = Some(vec![main_a.clone(), a, rest_a.clone()]);
            e
        })
        .collect();
    (s0, hops)
}

/// The global definitions of strutil's version `v` and their addresses.
fn m1_defs(fx: &BTreeMap<String, Obj>, v: &str, s: &State) -> BTreeMap<&'static [u8], u64> {
    let o = &fx[&format!("strutil_{v}")];
    let (_, _, index) = m1_sections(o);
    let mut out = BTreeMap::new();
    for &(name, shndx, value) in &o.syms {
        if let Some(k) = index.iter().position(|&i| i == shndx)
            && let Some(a) = s.addr(2, k as u32 + 1)
        {
            out.insert(name, a + value);
        }
    }
    out
}

// spec: incr.md IN-72 item 2 (the model's series, review-in72's protobuf
// strutil.cc builds: definitions whose address moves per hop)
#[test]
fn edit_moves_only_changed_sections() {
    for (dir, expect) in [("o0g", [82, 80, 79, 0, 0]), ("o2fs", [1, 0, 0, 0, 0])] {
        let fx = m1_fixture(dir);
        let (mut s, hops) = m1_world(dir);
        let versions = ["base", "grow1", "grow2", "rmfn", "addfn", "base"];
        let mut prev = m1_defs(&fx, versions[0], &s);
        let main_at = s.addr(1, 1);
        let mut got = Vec::new();
        for (k, e) in hops.iter().enumerate() {
            let (next, d) = hop(&s, e).unwrap_or_else(|r| panic!("{dir} hop {}: {r:?}", k + 1));
            let now = m1_defs(&fx, versions[k + 1], &next);
            let moved = now.iter().filter(|(n, a)| prev.get(*n).is_some_and(|p| p != *a)).count();
            got.push(moved);
            assert_eq!(next.addr(1, 1), main_at, "main.o never moves");
            eprintln!(
                "{dir} hop {} ({}): {moved} of {} moved; regions opened {}, steps {}",
                k + 1,
                versions[k + 1],
                now.len(),
                d.regions_opened.len(),
                d.growth_steps.len()
            );
            prev = now;
            s = next;
        }
        assert_eq!(got, expect, "{dir}");
    }
}

// ---- item 7: slots (WP-242) -------------------------------------------------

fn gkey(id: u32) -> mortar_core::id::SymKey {
    mortar_core::id::SymKey::global(mortar_core::id::SymbolId(id))
}

fn slot(kind: SlotKind, key: mortar_core::id::SymKey, index: u32, refs: u32) -> StoredSlot {
    StoredSlot { kind, key, index, refs }
}

fn key_ref(id: u32, file: u32, demand: DemandFlags) -> StoredKeyRef {
    StoredKeyRef { key: mortar_core::id::SymbolId(id), file: FileId(file), demand }
}

/// The first link of `two_text_files` with `slots` and `key_refs` stored,
/// and file 1 changed (its `.text.a` grown) with its new slot demand.
fn slots_hop(
    machine: Machine,
    slots: &[StoredSlot],
    key_refs: &[StoredKeyRef],
    demand: &[(mortar_core::id::SymKey, DemandFlags)],
) -> Result<LayoutDelta, IncrReason> {
    let mut s0 = first(machine, &two_text_files());
    s0.mem.slots = slots.to_vec();
    s0.mem.key_refs = key_refs.to_vec();
    let mut files = s0.files();
    files[0][0].size = 60;
    let (changes, _) = changes_of(&s0, &Edit::new(files).changed(&[1]));
    let demands = [FileDemands {
        file: FileId(1),
        demands: demand.iter().map(|&(key, flags)| SlotDemand { key, flags }).collect(),
    }];
    let inp = LayoutInput { demands: &demands, ..input(&changes) };
    layout(&StoredLayout::new(&s0.mem), &inp, &s0.world.target)
}

// spec: incr.md IN-72 item 7 (two IRELATIVE slots added by one update: in
// `SymKey` order after every stored IPLT slot, S-64/S-113; the order is
// `delta::slots`' `two_iplt_appended_in_key_order`, against the full
// link's rows). The seam carries no region capacity for slots (S-63:
// capacity = count; R-M4 not built, WP-242 finding D), so the update
// refuses by name, naming the kind.
#[test]
fn two_ifuncs_added_irelative_order() {
    const IPLT: DemandFlags = DemandFlags::IPLT;
    for m in [Machine::X86_64, Machine::AArch64] {
        // Given in key order (the join's contract): 3 before 5.
        let e = slots_hop(m, &[], &[], &[(gkey(3), IPLT), (gkey(5), IPLT)]).unwrap_err();
        assert_eq!(e.name(), "reserve-exhausted", "{e:?}");
        assert!(e.report().contains("IPLT"), "{e:?}");
        // Not in key order: the join's contract broken, refused.
        let e = slots_hop(m, &[], &[], &[(gkey(5), IPLT), (gkey(3), IPLT)]).unwrap_err();
        assert_eq!(e, not_implemented());
        // An IPLT slot retired changes `.rela.plt`'s size and
        // `__rela_iplt_end` (finding F): refused.
        let e = slots_hop(m, &[slot(SlotKind::Iplt, gkey(3), 0, 1)], &[key_ref(3, 1, IPLT)], &[])
            .unwrap_err();
        assert_eq!(e, not_implemented());
    }
}

// spec: incr.md IN-72 item 7 (a slot whose `refs` falls to 0 is retired:
// `slots_dead`, written 0, S-113; `refs` counts FILES, WP-239 S4-4)
#[test]
fn item7_got_retired_and_counted() {
    const GOT: DemandFlags = DemandFlags::GOT;
    const IE: DemandFlags = DemandFlags::GOT_TPREL;
    for m in [Machine::X86_64, Machine::AArch64] {
        // File 1 alone needed GOT[0] (key 3): retired.
        let stored = [slot(SlotKind::Got, gkey(3), 0, 1), slot(SlotKind::Got, gkey(4), 1, 1)];
        let refs = [key_ref(3, 1, GOT), key_ref(4, 2, GOT)];
        let d = slots_hop(m, &stored, &refs, &[]).unwrap();
        assert_eq!(d.slots_dead, vec![slot(SlotKind::Got, gkey(3), 0, 0)]);
        assert!(d.slots_appended.is_empty());
        // Unchanged demand: nothing.
        let d = slots_hop(m, &stored, &refs, &[(gkey(3), GOT)]).unwrap();
        assert!(d.slots_dead.is_empty() && d.slots_appended.is_empty(), "{d:?}");
        // GOT|GOT_TPREL in one row, once per file per kind: both retired.
        let stored = [slot(SlotKind::Got, gkey(3), 0, 1), slot(SlotKind::TlsIe, gkey(3), 0, 1)];
        let d = slots_hop(m, &stored, &[key_ref(3, 1, GOT.union(IE))], &[]).unwrap();
        assert_eq!(
            d.slots_dead,
            vec![slot(SlotKind::Got, gkey(3), 0, 0), slot(SlotKind::TlsIe, gkey(3), 0, 0)]
        );
        // Shared by both files (refs 2): file 1 drops it, refs 1. No
        // field of `LayoutDelta` carries a live slot's count (finding B).
        let shared = [slot(SlotKind::Got, gkey(3), 0, 2)];
        let both = [key_ref(3, 1, GOT), key_ref(3, 2, GOT)];
        assert_eq!(slots_hop(m, &shared, &both, &[]).unwrap_err(), not_implemented());
        // Drawn in on a live slot: refs 2, the same.
        let one = [slot(SlotKind::Got, gkey(3), 0, 1)];
        let e = slots_hop(m, &one, &[key_ref(3, 2, GOT)], &[(gkey(3), GOT)]).unwrap_err();
        assert_eq!(e, not_implemented());
        // A dead slot revived in place: the same.
        let dead = [slot(SlotKind::Got, gkey(3), 0, 0)];
        assert_eq!(slots_hop(m, &dead, &[], &[(gkey(3), GOT)]).unwrap_err(), not_implemented());
        // A stored demand with no slot: corrupt.
        let e = slots_hop(m, &[], &[key_ref(3, 1, GOT)], &[]).unwrap_err();
        assert_eq!(e.name(), "state-corrupt", "{e:?}");
    }
}

// spec: incr.md IN-28b (a local's slot is its file's alone: a changed
// file's old local slots all die, its new local demand is a new slot)
#[test]
fn item7_local_slots_belong_to_their_file() {
    const GOT: DemandFlags = DemandFlags::GOT;
    let local = |file: u32, index: u32| {
        mortar_core::id::SymKey::local(mortar_core::id::LocalSym {
            file: FileId(file),
            index: mortar_elf::SymbolIndex(index),
        })
    };
    for m in [Machine::X86_64, Machine::AArch64] {
        let stored =
            [slot(SlotKind::Got, local(1, 7), 0, 1), slot(SlotKind::Got, local(2, 7), 1, 1)];
        // File 1 changed, no local demand now: its slot dies, file 2's stays.
        let d = slots_hop(m, &stored, &[], &[]).unwrap();
        assert_eq!(d.slots_dead, vec![slot(SlotKind::Got, local(1, 7), 0, 0)]);
        // The same local index demanded again is a new slot (no identity
        // across versions): appended, past capacity.
        let e = slots_hop(m, &stored, &[], &[(local(1, 7), GOT)]).unwrap_err();
        assert_eq!(e.name(), "reserve-exhausted", "{e:?}");
        // Another file's local: no file demands one.
        let e = slots_hop(m, &stored, &[], &[(local(2, 7), GOT)]).unwrap_err();
        assert_eq!(e, not_implemented());
    }
}

// ---- items 5, 6, 7 at their edges (WP-242, WP-243, joined by WP-245) -------

// spec: incr.md IN-72 items 5, 6, 7 (item 5 is WP-243's, items 6 and 7
// WP-242's; what each still refuses at the edge of its input)
#[test]
fn items_5_6_7_refused_wholesale() {
    let s0 = first(Machine::X86_64, &two_text_files());
    let mut files = s0.files();
    files[0][0].size = 60;
    let (changes, _) = changes_of(&s0, &Edit::new(files).changed(&[1]));
    let t = &s0.world.target;
    let f1 = FileId(1);
    let sid = SectionId(1);
    let run = |mem: &MemoryLayout, inp: &LayoutInput<'_>| layout(&StoredLayout::new(mem), inp, t);
    assert!(run(&s0.mem, &input(&changes)).is_ok());
    // The new version's.
    let eh = [EhFile {
        file: f1,
        rank: 1,
        records: vec![EhRecordIn { section: sid, piece: 0, size: 24, cie: true }],
    }];
    let pieces = [FilePieces {
        file: f1,
        pieces: vec![MergedPiece {
            section: sid,
            offset: 0,
            size: 4,
            out: OutSecId(0),
            class: MergeClass { strings: true, entsize: 1, align: 0 },
            hash: [7; 16],
        }],
    }];
    let demands = [FileDemands {
        file: f1,
        demands: vec![SlotDemand {
            key: mortar_core::id::SymKey::global(mortar_core::id::SymbolId(3)),
            flags: DemandFlags::GOT,
        }],
    }];
    let with = |i: LayoutInput<'_>| run(&s0.mem, &i).err();
    // Item 5 (WP-243): records with no `.eh_frame` chunk in the stored
    // link.
    let no_eh = IncrReason::NewOutputSection(".eh_frame".into());
    assert_eq!(with(LayoutInput { eh: &eh, ..input(&changes) }), Some(no_eh));
    assert_eq!(with(LayoutInput { pieces: &pieces, ..input(&changes) }), Some(not_implemented()));
    // Item 7 is built (WP-242): a new slot is past its region's capacity.
    let e = with(LayoutInput { demands: &demands, ..input(&changes) }).unwrap();
    assert_eq!(e.name(), "reserve-exhausted", "{e:?}");
    // Empty lists are no demand.
    let none = [EhFile { file: f1, rank: 1, records: Vec::new() }];
    assert!(with(LayoutInput { eh: &none, ..input(&changes) }).is_none());
    // The old version's.
    let old = |f: &dyn Fn(&mut MemoryLayout)| {
        let mut m = s0.mem.clone();
        f(&mut m);
        run(&m, &input(&changes)).err()
    };
    // A range and no `EhFile` for it (WP-243).
    let r = StoredEhRange { offset: 0, capacity: 40, records: 1 };
    assert_eq!(old(&|m| m.eh_ranges.push((f1, r))), Some(not_implemented()));
    let fu = StoredFragUse { file: f1, out: OutSecId(0), class: 0, hash: [1; 16] };
    // Item 6 is built (WP-242): a use with no `FRAGS` row is corrupt.
    assert_eq!(old(&|m| m.frag_uses.push(fu)).map(|e| e.name()), Some("state-corrupt"));
    let key = mortar_core::id::SymbolId(3);
    let got = DemandFlags::GOT;
    let slot = StoredKeyRef { key, file: f1, demand: got };
    // Item 7 is built: a stored demand with no `SLOTS` row is corrupt.
    assert_eq!(old(&|m| m.key_refs.push(slot)).map(|e| e.name()), Some("state-corrupt"));
    // A mention with no demand is no slot; another file's rows are not read.
    let plain = StoredKeyRef { key, file: f1, demand: DemandFlags::empty() };
    assert!(old(&|m| m.key_refs.push(plain)).is_none());
    assert!(old(&|m| m.eh_ranges.push((FileId(2), r))).is_none());
}

// ---- item 10: an output section left empty (F-6) ----------------------------

// spec: incr.md IN-72 item 10 (an output section left empty:
// `new-output-section`; WP-216 F-6, detected by bytes)
#[test]
fn output_section_left_empty_is_new_output_section() {
    // `.rodata` only in file 1: its section dies, the output section is
    // left empty.
    let files =
        vec![vec![text(b".text.a", 100), rodata(b".rodata", 64)], vec![text(b".text.c", 200)]];
    let s0 = first(Machine::X86_64, &files);
    let e = hop(&s0, &Edit::new(s0.files()).changed(&[1]).dead(&[(1, 2)])).err().unwrap();
    assert_eq!(e.name(), "new-output-section", "{e:?}");
    assert!(e.report().contains(".rodata"), "{e:?}");
    // The same file's section gone from its new version.
    let mut gone = s0.files();
    gone[0].pop();
    let e = hop(&s0, &Edit::new(gone).changed(&[1])).err().unwrap();
    assert_eq!(e.name(), "new-output-section", "{e:?}");
    // `.rodata` in both files: one dies, the other keeps the output
    // section; the update succeeds.
    let files = vec![
        vec![text(b".text.a", 100), rodata(b".rodata", 64)],
        vec![text(b".text.c", 200), rodata(b".rodata", 64)],
    ];
    let s0 = first(Machine::X86_64, &files);
    let (_, d) = hop(&s0, &Edit::new(s0.files()).changed(&[1]).dead(&[(1, 2)])).unwrap();
    assert_eq!(d.killed.len(), 1, "{d:?}");
}

// spec: incr.md IN-72 item 10 (WP-216 F-6's corner, WP-242: a
// zero-capacity region holding only dead empty sections between two
// paddings; the byte proof never keeps the output section: the paddings
// together are shorter than the largest alignment, at most the chunk's)
#[test]
fn f6_corner_empty_region_between_paddings() {
    for m in [Machine::X86_64, Machine::AArch64] {
        // .data: 1 byte (align 1), an empty section (align 8), 8 bytes
        // (align 16), one file each.
        let files = vec![
            vec![text(b".text.a", 16), sec(b".data", SHT_PROGBITS, WA, 1, 0)],
            vec![text(b".text.b", 16), sec(b".data", SHT_PROGBITS, WA, 0, 3)],
            vec![text(b".text.c", 16), sec(b".data", SHT_PROGBITS, WA, 8, 4)],
        ];
        let s0 = first(m, &files);
        let e = s0.addr(2, 2).unwrap();
        assert!(e > s0.addr(1, 2).unwrap() + 1, "a padding before the empty section");
        assert!(s0.addr(3, 2).unwrap() > e, "and one after it");
        // Every member dead: refused.
        let all = Edit::new(s0.files()).changed(&[1, 2, 3]).dead(&[(1, 2), (2, 2), (3, 2)]);
        let r = hop(&s0, &all).err().unwrap();
        assert_eq!(r.name(), "new-output-section", "{m:?} {r:?}");
        // The two with bytes dead, the empty one live: a full link has no
        // chunk for it (L-159); refused, never kept.
        let two = Edit::new(s0.files()).changed(&[1, 3]).dead(&[(1, 2), (3, 2)]);
        let r = hop(&s0, &two).err().unwrap();
        assert_eq!(r.name(), "new-output-section", "{m:?} {r:?}");
        // One live byte left (file 1's): 1 byte in a 16-aligned chunk is
        // not proof of a live section, so it is refused too (the error
        // F-6 accepts: toward a refusal).
        let one = Edit::new(s0.files()).changed(&[3]).dead(&[(3, 2)]);
        let r = hop(&s0, &one).err().unwrap();
        assert_eq!(r.name(), "new-output-section", "{m:?} {r:?}");
    }
}

// ---- R-3: the one reader ----------------------------------------------------

// spec: incr.md IN-18c, IN-72 (WP-216 R-3: the sparse placement names its
// files and touched chunks; a section of another file reads as stored)
#[test]
fn sparse_placement_names_files_and_touched_chunks() {
    let s0 = first(Machine::X86_64, &two_text_files());
    let mut files = s0.files();
    files[0][1].size = 30;
    let (changes, _) = changes_of(&s0, &Edit::new(files).changed(&[1]));
    let stored = StoredLayout::new(&s0.mem);
    let d = layout(&stored, &input(&changes), &s0.world.target).unwrap();
    assert_eq!(d.files, [FileId(1)]);
    assert!(!d.hole_chunks.is_empty());
    let other = s0.world.sid(2, 1);
    let at = s0.placed.placement.place[other];
    assert_eq!(d.place_of(&stored, other).unwrap(), PlaceNow::Stored(at));
    let mine = changes.iter().find(|c| c.key.name == b".text.b").unwrap().id;
    assert!(matches!(d.place_of(&stored, mine).unwrap(), PlaceNow::Here(_)));
    // A chunk the update did not touch has its stored holes.
    let untouched = s0
        .placed
        .placement
        .order
        .iter()
        .copied()
        .find(|c| d.hole_chunks.binary_search(c).is_err())
        .unwrap();
    assert_eq!(d.holes_of(&stored, untouched).unwrap(), s0.mem.holes_in(untouched).unwrap());
}

// ---- item 6: merged pieces (WP-242) -----------------------------------------
//
// The model: every file has `.rodata.cst8` (1: constants),
// `.rodata.str1.1` (2: strings) and a second `.rodata.cst8` (3: a COMDAT
// copy, kept or not), nothing else (so an update is item 6's alone). A file's pieces are those of its live, kept merged
// sections. The first link is a full link of the World with those merged
// sections split (one `Merged` chunk of `.rodata`); an update is `layout`
// over the stored rows with every section of each file of the update
// given as its own row (a merged section's pieces are not a row's place,
// so the file's version is its pieces), and its delta is applied to the
// rows. After every hop the rows are compared with the rows a model
// computes from scratch for the new files: `FRAGS` keys and refs (the live
// ones; a dead range keeps its row at 0), `FRAG_USE`; and the merged
// chunk is checked as `Layout::check`'s §4.2 would (every live fragment
// inside the chunk, aligned, no two overlapping, no hole over one; every
// dead constant a fill hole of its bytes).

const CST: MergeClass = MergeClass { strings: false, entsize: 8, align: 3 };
const STR: MergeClass = MergeClass { strings: true, entsize: 1, align: 0 };

/// A merged piece's content: a constant (8 bytes) or a string of `size`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Pc {
    strings: bool,
    id: u8,
    size: u64,
}

impl Pc {
    fn class(self) -> MergeClass {
        if self.strings { STR } else { CST }
    }
    fn hash(self) -> [u8; 16] {
        let mut h = [0; 16];
        h[0] = self.id;
        h[1] = u8::from(self.strings);
        h[2] = self.size as u8;
        h
    }
}

/// One file of the model.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct MFile {
    live: bool,
    cst: Vec<u8>,
    strs: Vec<(u8, u64)>,
    inl: Vec<u8>,
    inl_kept: bool,
}

fn mfile(cst: &[u8]) -> MFile {
    MFile { live: true, cst: cst.to_vec(), ..MFile::default() }
}

impl MFile {
    /// Per merged section index (1, 2, 3): live, kept, its pieces.
    fn sections(&self) -> [(u32, bool, bool, Vec<Pc>); 3] {
        let cst = self.cst.iter().map(|&id| Pc { strings: false, id, size: 8 }).collect();
        let strs = self.strs.iter().map(|&(id, size)| Pc { strings: true, id, size }).collect();
        let inl: Vec<Pc> = self.inl.iter().map(|&id| Pc { strings: false, id, size: 8 }).collect();
        [
            (1, self.live && !self.cst.is_empty(), true, cst),
            (2, self.live && !self.strs.is_empty(), true, strs),
            (3, self.live && !self.inl.is_empty(), self.inl_kept, inl),
        ]
    }
    /// Its live pieces, in (section, offset) order.
    fn pieces(&self) -> Vec<(u32, u64, Pc)> {
        let mut v = Vec::new();
        for (i, live, kept, list) in self.sections() {
            if !(live && kept) {
                continue;
            }
            let mut off = 0;
            for p in list {
                v.push((i, off, p));
                off += p.size;
            }
        }
        v
    }
}

type FKey = (OutSecId, u32, [u8; 16]);

/// A merged link's stored state.
struct MState {
    world: World,
    mem: MemoryLayout,
    model: Vec<MFile>,
    rodata: OutSecId,
    /// Files new to the state (no rows yet: they appear in an update).
    absent: Vec<u32>,
    /// Every content's size.
    sizes: BTreeMap<FKey, u64>,
}

fn mworld(machine: Machine, model: &[MFile]) -> World {
    let ro = SHF_ALLOC | SHF_MERGE;
    let mut files: Vec<Vec<Sec>> = model
        .iter()
        .map(|_| {
            vec![
                sec(b".rodata.cst8", SHT_PROGBITS, ro, 64, 3),
                sec(b".rodata.str1.1", SHT_PROGBITS, ro | SHF_STRINGS, 64, 0),
                sec(b".rodata.cst8", SHT_PROGBITS, ro, 64, 3),
            ]
        })
        .collect();
    // A last file (not in the model, never in an update): code and data
    // after `.rodata`'s `PT_LOAD`, as in a program.
    files.push(vec![text(b".text", 64), data(b".data", 64)]);
    let mut w = world(machine, &files);
    for f in 1..=model.len() as u32 {
        for i in 1..=3 {
            let s = w.sid(f, i);
            w.resolution.sections[s].kind = mortar_core::resolution::SectionKind::Merge;
        }
    }
    w
}

/// The first link (or a compaction: a full link) of `model`; `absent`
/// files are dead in it and have no rows.
fn mfirst(model: &[MFile], absent: &[u32]) -> MState {
    mfirst_on(Machine::X86_64, model, absent)
}

fn mfirst_on(machine: Machine, model: &[MFile], absent: &[u32]) -> MState {
    use mortar_core::split::Frag;
    let mut w = mworld(machine, model);
    // Liveness, and the fragments by first occurrence.
    let mut frags: Vec<(Pc, SectionId, u64)> = Vec::new();
    for (k, m) in model.iter().enumerate() {
        let f = k as u32 + 1;
        let here = m.live && !absent.contains(&f);
        for (i, live, kept, _) in m.sections() {
            let s = w.sid(f, i);
            w.liveness.live[s] = here && live && kept;
        }
        if !here {
            continue;
        }
        for (i, off, p) in m.pieces() {
            if !frags.iter().any(|(q, _, _)| *q == p) {
                frags.push((p, w.sid(f, i), off));
            }
        }
    }
    // Offsets: strings, then constants, each at its alignment.
    let mut off_of = vec![0u64; frags.len()];
    let mut at = 0u64;
    for strings in [true, false] {
        for (k, (p, _, _)) in frags.iter().enumerate() {
            if p.strings == strings {
                let a = 1u64 << p.class().align;
                at = at.next_multiple_of(a);
                off_of[k] = at;
                at += p.size;
            }
        }
    }
    w.split.frags = Column::from_vec(
        frags
            .iter()
            .map(|(p, s, off)| Frag {
                section: *s,
                offset: *off as u32,
                size: p.size as u32,
                align: p.class().align,
            })
            .collect(),
    )
    .unwrap();
    w.merge.frag_off = Column::from_vec(off_of.clone()).unwrap();
    w.liveness.frag_live = Column::filled(frags.len() as u32, true);
    let (a, placed) = w.run(&Pool::sequential()).expect("first link");
    w.check(&a, &placed).expect("the first link passes Layout::check");
    let regions = regions_of_full_link(&w, &a, &placed.placement);
    let mut mem = capture(&w, &a, &placed.placement, &regions);
    let rodata =
        a.outsecs.iter().find(|(_, o)| o.name == b".rodata").map(|(id, _)| id).expect(".rodata");
    mem.sections.retain(|s| !absent.contains(&s.file.0));
    let (refs, uses) = mmodel(model, absent, rodata);
    for (k, (p, _, _)) in frags.iter().enumerate() {
        let key = (rodata, p.class().word(), p.hash());
        mem.frags.push(mortar_core::incr::StoredFrag {
            out: key.0,
            class: key.1,
            hash: key.2,
            offset: off_of[k],
            refs: refs[&key],
        });
    }
    mem.frags.sort_by_key(|f| (f.out, f.class, f.hash));
    mem.frag_uses = uses.into_iter().collect();
    let mut sizes = BTreeMap::new();
    for (p, _, _) in &frags {
        sizes.insert((rodata, p.class().word(), p.hash()), p.size);
    }
    let st =
        MState { world: w, mem, model: model.to_vec(), rodata, absent: absent.to_vec(), sizes };
    mcheck(&st);
    st
}

/// The rows a full link of `model` gives, from scratch: `FRAGS` refs by
/// key (files naming it), `FRAG_USE`.
fn mmodel(
    model: &[MFile],
    absent: &[u32],
    out: OutSecId,
) -> (BTreeMap<FKey, u32>, std::collections::BTreeSet<StoredFragUse>) {
    let mut refs: BTreeMap<FKey, u32> = BTreeMap::new();
    let mut uses = std::collections::BTreeSet::new();
    for (k, m) in model.iter().enumerate() {
        let f = k as u32 + 1;
        if !m.live || absent.contains(&f) {
            continue;
        }
        let mut mine = std::collections::BTreeSet::new();
        for (_, _, p) in m.pieces() {
            mine.insert((out, p.class().word(), p.hash()));
        }
        for key in mine {
            *refs.entry(key).or_default() += 1;
            uses.insert(StoredFragUse { file: FileId(f), out: key.0, class: key.1, hash: key.2 });
        }
    }
    (refs, uses)
}

/// The rows against the model, and the merged chunk as §4.2 would check
/// it.
fn mcheck(st: &MState) {
    let (refs, uses) = mmodel(&st.model, &st.absent, st.rodata);
    let live: BTreeMap<FKey, u32> = st
        .mem
        .frags
        .iter()
        .filter(|f| f.refs > 0)
        .map(|f| ((f.out, f.class, f.hash), f.refs))
        .collect();
    assert_eq!(live, refs, "FRAGS (live keys, refs) = the model's");
    let have: std::collections::BTreeSet<StoredFragUse> =
        st.mem.frag_uses.iter().copied().collect();
    assert_eq!(have.len(), st.mem.frag_uses.len(), "FRAG_USE rows distinct");
    assert_eq!(have, uses, "FRAG_USE = the model's");
    let keys: Vec<FKey> = st.mem.frags.iter().map(|f| (f.out, f.class, f.hash)).collect();
    assert!(keys.windows(2).all(|w| w[0] < w[1]), "FRAGS keys distinct, sorted");
    // The merged chunk.
    let (c, ch) = st
        .mem
        .chunks
        .iter()
        .enumerate()
        .find(|(_, c)| c.kind == ChunkKind::Merged(st.rodata))
        .expect("the merged chunk");
    let c = ChunkId(c as u32);
    assert!(ch.size <= ch.capacity, "size within capacity");
    let holes: Vec<Hole> = st.mem.holes.iter().filter(|h| h.chunk == c).copied().collect();
    let mut runs: Vec<(u64, u64)> = Vec::new();
    for f in &st.mem.frags {
        let size = st.sizes[&(f.out, f.class, f.hash)];
        let align = 1u64 << ((f.class >> 24) & 0x7f);
        assert!(f.offset.is_multiple_of(align) && align <= 1u64 << ch.align, "aligned");
        assert!(f.offset + size <= ch.size, "inside the chunk");
        if f.refs > 0 {
            runs.push((f.offset, f.offset + size));
            assert!(
                !holes.iter().any(|h| h.offset < f.offset + size && f.offset < h.offset + h.size),
                "no hole over a live fragment"
            );
        } else if f.class >> 31 == 0 {
            let fill = Hole { chunk: c, offset: f.offset, size: 8, kind: HoleKind::Fill };
            assert!(holes.contains(&fill), "a dead constant is a fill hole");
        }
    }
    runs.sort_unstable();
    assert!(runs.windows(2).all(|w| w[0].1 <= w[1].0), "no two live fragments overlap");
}

/// A merged update: `next` the new model, `files` the files of the update
/// (1-based); those in `st.absent` appear. `extra` adjusts the input.
#[allow(clippy::too_many_lines, reason = "the input, then the delta applied row by row")]
/// The update's changes and pieces for `files` of `next`.
fn minput<'a>(
    st: &'a MState,
    next: &[MFile],
    files: &[u32],
) -> (Vec<SectionChange<'a>>, Vec<FilePieces>) {
    let w = &st.world;
    let mut changes = Vec::new();
    let mut pieces = Vec::new();
    for &f in files {
        let m = &next[f as usize - 1];
        let appear = st.absent.contains(&f);
        let mut fp = FilePieces { file: FileId(f), pieces: Vec::new() };
        let secs = m.sections();
        for i in 1..=3u32 {
            let s = w.sid(f, i);
            let h = &w.headers[s];
            let x = &secs[i as usize - 1];
            let (live, kept) = (x.1, x.2);
            changes.push(SectionChange {
                id: s,
                file: FileId(f),
                index: i,
                key: SectionKey { anchor: Anchor::None, name: h.name },
                ordinal: u32::from(i == 3),
                old: (!appear).then_some(s),
                sh_type: h.sh_type,
                sh_flags: h.flags,
                size: h.size,
                align: 1u64 << h.align,
                live,
                kept,
            });
        }
        for (i, off, p) in m.pieces() {
            fp.pieces.push(MergedPiece {
                section: w.sid(f, i),
                offset: off,
                size: p.size,
                out: st.rodata,
                class: p.class(),
                hash: p.hash(),
            });
        }
        pieces.push(fp);
    }
    changes.sort_by_key(|c| c.id);
    (changes, pieces)
}

fn mhop_with(
    st: &MState,
    next: &[MFile],
    files: &[u32],
    extra: &dyn Fn(&mut Vec<FilePieces>),
) -> Result<(MState, LayoutDelta), IncrReason> {
    let w = &st.world;
    let (changes, mut pieces) = minput(st, next, files);
    extra(&mut pieces);
    let stored = StoredLayout::new(&st.mem);
    let inp = LayoutInput { changes: &changes, pieces: &pieces, ..LayoutInput::default() };
    let d = layout(&stored, &inp, &w.target)?;
    // The delta applied to the rows.
    let mut mem = st.mem.clone();
    mem.chunks = d.placement.chunks.as_slice().to_vec();
    mem.segments = d.placement.segments.as_slice().to_vec();
    mem.defined = d.placement.defined.to_vec();
    mem.tls = d.placement.tls;
    mem.entry = d.placement.entry;
    mem.holes.retain(|h| d.hole_chunks.binary_search(&h.chunk).is_err());
    for &c in &d.hole_chunks {
        mem.holes.extend(d.holes_of(&stored, c).unwrap());
    }
    mem.holes.sort_by_key(|h| (h.chunk, h.offset));
    for r in &d.frags {
        match mem.frags.iter_mut().find(|f| (f.out, f.class, f.hash) == (r.out, r.class, r.hash)) {
            Some(f) => *f = *r,
            None => mem.frags.push(*r),
        }
    }
    mem.frags.sort_by_key(|f| (f.out, f.class, f.hash));
    mem.frag_uses.retain(|u| !files.contains(&u.file.0));
    for fp in &pieces {
        let mut keys = std::collections::BTreeSet::new();
        for p in &fp.pieces {
            keys.insert((p.out, p.class.word(), p.hash));
        }
        mem.frag_uses.extend(keys.into_iter().map(|k| StoredFragUse {
            file: fp.file,
            out: k.0,
            class: k.1,
            hash: k.2,
        }));
    }
    mem.frag_uses.sort();
    for c in &changes {
        let place = match d.place_of(&stored, c.id).unwrap() {
            PlaceNow::Here(p) | PlaceNow::Stored(p) => p,
            PlaceNow::NotPlaced => Place::NONE,
        };
        if let Some(s) = mem.sections.iter_mut().find(|s| s.id == c.id) {
            s.place = place;
        } else {
            let h = &w.headers[c.id];
            mem.sections.push(StoredSection {
                id: c.id,
                file: c.file,
                index: c.index,
                anchor_kind: 0,
                anchor: Vec::new(),
                name: h.name.to_vec(),
                ordinal: c.ordinal,
                sh_type: h.sh_type,
                sh_flags: h.flags,
                size: h.size,
                align: 1u64 << h.align,
                place,
            });
        }
    }
    mem.sections.sort_by_key(|s| s.id);
    mem.regions.extend(d.regions_opened.iter().copied());
    let mut sizes = st.sizes.clone();
    for m in next {
        for (_, _, p) in m.pieces() {
            sizes.insert((st.rodata, p.class().word(), p.hash()), p.size);
        }
    }
    let absent = st.absent.iter().copied().filter(|f| !files.contains(f)).collect();
    let n = MState {
        world: st.world.clone(),
        mem,
        model: next.to_vec(),
        rodata: st.rodata,
        absent,
        sizes,
    };
    mcheck(&n);
    Ok((n, d))
}

fn mhop(st: &MState, next: &[MFile], files: &[u32]) -> Result<(MState, LayoutDelta), IncrReason> {
    mhop_with(st, next, files, &|_| {})
}

/// The `FRAGS` row of constant `id` in a state.
fn frag_row(st: &MState, id: u8) -> mortar_core::incr::StoredFrag {
    let p = Pc { strings: false, id, size: 8 };
    *st.mem.frags.iter().find(|f| f.hash == p.hash() && f.class == CST.word()).unwrap()
}

// spec: incr.md IN-72 item 6 (a changed file: its old uses -1, its new
// distinct fragments +1; a new key appended at the merged chunk's tail; a
// constant at 0 is a fill hole; FRAGS.refs exact, IN-78)
#[test]
fn item6_changed_file_counts_exact() {
    for m in [Machine::X86_64, Machine::AArch64] {
        item6_changed_file_counts_exact_on(m);
    }
}

fn item6_changed_file_counts_exact_on(m: Machine) {
    let s0 = mfirst_on(m, &[mfile(&[1, 2, 2]), mfile(&[2, 3]), mfile(&[3])], &[]);
    assert_eq!(frag_row(&s0, 2).refs, 2, "counted per FILE, not per piece");
    let tail = s0.mem.chunks.iter().find(|c| c.kind == ChunkKind::Merged(s0.rodata)).unwrap().size;
    // File 1: drops 1, keeps 2, adds 4 twice.
    let (s1, d) = mhop(&s0, &[mfile(&[2, 4, 4]), mfile(&[2, 3]), mfile(&[3])], &[1]).unwrap();
    assert_eq!(frag_row(&s1, 1).refs, 0, "a dead range keeps its row");
    assert_eq!(frag_row(&s1, 2), frag_row(&s0, 2), "unchanged count: no row");
    let four = frag_row(&s1, 4);
    assert_eq!((four.refs, four.offset), (1, tail.next_multiple_of(8)), "appended at the tail");
    assert_eq!(d.frags.len(), 2, "{:?}", d.frags);
    assert!(
        d.holes.iter().any(|h| h.offset == frag_row(&s0, 1).offset && h.kind == HoleKind::Fill)
    );
    assert!(d.moved.is_empty() && d.killed.is_empty() && d.revived.is_empty());
}

// spec: incr.md IN-72 item 6 (an appeared file, a gone file; a drawn-in
// archive member, then no longer extracted)
#[test]
fn item6_appeared_gone_drawn_in() {
    for m in [Machine::X86_64, Machine::AArch64] {
        item6_appeared_gone_drawn_in_on(m);
    }
}

fn item6_appeared_gone_drawn_in_on(m: Machine) {
    let base = [mfile(&[1, 2]), mfile(&[2]), mfile(&[2, 5]), mfile(&[6])];
    // File 4 is new to the state; file 3 a member not extracted.
    let mut first = base.clone();
    first[2].live = false;
    let s0 = mfirst_on(m, &first, &[4]);
    assert!(s0.mem.frag_uses.iter().all(|u| u.file != FileId(3) && u.file != FileId(4)));
    // Appeared (4) and drawn in (3) together.
    let (s1, _) = mhop(&s0, &base, &[3, 4]).unwrap();
    assert_eq!(frag_row(&s1, 2).refs, 3);
    // File 1 gone; file 3 no longer extracted.
    let mut next = base.clone();
    next[0].live = false;
    next[2].live = false;
    let (s2, d) = mhop(&s1, &next, &[1, 3]).unwrap();
    assert_eq!(frag_row(&s2, 2).refs, 1);
    assert_eq!(frag_row(&s2, 1).refs, 0);
    assert_eq!(frag_row(&s2, 5).refs, 0);
    assert!(d.killed.is_empty() && d.moved.is_empty(), "merged sections are not placed: {d:?}");
}

// spec: incr.md IN-72 item 6 (a COMDAT keeper moving to another file: the
// old keeper's pieces contribute nothing, the new keeper's count once)
#[test]
fn item6_comdat_keeper_moves() {
    for m in [Machine::X86_64, Machine::AArch64] {
        item6_comdat_keeper_moves_on(m);
    }
}

fn item6_comdat_keeper_moves_on(m: Machine) {
    let mut a = mfile(&[1]);
    a.inl = vec![7, 8];
    a.inl_kept = true;
    let mut b = mfile(&[8]);
    b.inl = vec![7, 8];
    let s0 = mfirst_on(m, &[a.clone(), b.clone()], &[]);
    assert_eq!((frag_row(&s0, 7).refs, frag_row(&s0, 8).refs), (1, 2));
    a.inl_kept = false;
    b.inl_kept = true;
    let (s1, d) = mhop(&s0, &[a, b], &[1, 2]).unwrap();
    // 7: one keeper either way; 8: file 1 named it only through its copy,
    // file 2 through both: 2 -> 1.
    assert_eq!((frag_row(&s1, 7).refs, frag_row(&s1, 8).refs), (1, 1));
    assert_eq!(d.frags.len(), 1, "{:?}", d.frags);
}

// spec: incr.md IN-72 item 6 (three hops: edit, inverse, edit again; a
// fragment whose count rises from 0 revives at its offset, its fill hole
// gone, rewritten by U-6; then edit A, then B; a compaction between two
// edits drops the dead ranges, counts unchanged)
#[test]
fn item6_three_hops_and_compaction() {
    for m in [Machine::X86_64, Machine::AArch64] {
        item6_three_hops_and_compaction_on(m);
    }
}

fn item6_three_hops_and_compaction_on(m: Machine) {
    let v0 = [mfile(&[1, 2]), mfile(&[2, 3])];
    let v1 = [mfile(&[2, 4]), mfile(&[2, 3])];
    let s0 = mfirst_on(m, &v0, &[]);
    let (s1, _) = mhop(&s0, &v1, &[1]).unwrap();
    let dead = frag_row(&s1, 1);
    assert_eq!(dead.refs, 0);
    // Inverse: 1 revives in place (rewritten from the piece), 4 dies.
    let (s2, d2) = mhop(&s1, &v0, &[1]).unwrap();
    let back = frag_row(&s2, 1);
    assert_eq!((back.refs, back.offset), (1, dead.offset), "revived at its offset");
    let was = s1.mem.frags.iter().find(|f| f.hash == back.hash).unwrap();
    assert_eq!(was.refs, 0, "U-6 reads 'rises from 0' from the stored row");
    assert!(d2.frags.contains(&back));
    assert!(!s2.mem.holes.iter().any(|h| h.offset == back.offset && h.size == 8));
    // Edit again: 4 revives, nothing appended.
    let size2 = s2.mem.chunks.iter().find(|c| c.kind == ChunkKind::Merged(s0.rodata)).unwrap().size;
    let (s3, _) = mhop(&s2, &v1, &[1]).unwrap();
    let size3 = s3.mem.chunks.iter().find(|c| c.kind == ChunkKind::Merged(s0.rodata)).unwrap().size;
    assert_eq!(size2, size3, "no append");
    // Edit A then B.
    let v4 = [mfile(&[2, 4, 9]), mfile(&[2, 3])];
    let (s4, _) = mhop(&s3, &v4, &[1]).unwrap();
    let v5 = [mfile(&[2, 4, 9]), mfile(&[9])];
    let (s5, _) = mhop(&s4, &v5, &[2]).unwrap();
    assert_eq!(frag_row(&s5, 9).refs, 2);
    assert_eq!(frag_row(&s5, 2).refs, 1);
    // A compaction (a full link of the same files) between two edits.
    let c = mfirst_on(m, &v5, &[]);
    assert!(c.mem.frags.iter().all(|f| f.refs > 0), "dead ranges gone");
    let live = |s: &MState| -> Vec<(FKey, u32)> {
        s.mem
            .frags
            .iter()
            .filter(|f| f.refs > 0)
            .map(|f| ((f.out, f.class, f.hash), f.refs))
            .collect()
    };
    assert_eq!(live(&c), live(&s5), "counts unchanged");
    let (c1, _) = mhop(&c, &v1, &[1, 2]).unwrap();
    assert_eq!(frag_row(&c1, 9).refs, 0);
}

// spec: incr.md IN-72 item 6 (strings: a new string appended; a string at
// 0 is refused by name, finding C: its length is not stored; a dead string
// that revives takes back its `DeadFragment` hole)
#[test]
fn item6_strings() {
    let mut a = mfile(&[1]);
    a.strs = vec![(1, 5)];
    let mut b = mfile(&[2]);
    b.strs = vec![(1, 5)];
    let s0 = mfirst(&[a.clone(), b.clone()], &[]);
    // Appended.
    let mut a1 = a.clone();
    a1.strs.push((2, 3));
    let (s1, d) = mhop(&s0, &[a1, b.clone()], &[1]).unwrap();
    assert_eq!(d.frags.len(), 1);
    assert_eq!(d.frags[0].refs, 1);
    // Falls to 0: refused.
    let mut a2 = a.clone();
    a2.strs.clear();
    let mut b2 = b.clone();
    b2.strs.clear();
    assert_eq!(mhop(&s1, &[a2, b2], &[1, 2]).err(), Some(not_implemented()));
    // A dead string (as a state with its hole would hold it) revives.
    let mut s = mfirst(&[a.clone(), b.clone()], &[]);
    let hash = Pc { strings: true, id: 1, size: 5 }.hash();
    let r = s.mem.frags.iter_mut().find(|f| f.hash == hash).unwrap();
    r.refs = 0;
    let at = r.offset;
    let c = s.mem.chunks.iter().position(|c| c.kind == ChunkKind::Merged(s.rodata)).unwrap();
    let hole = Hole { chunk: ChunkId(c as u32), offset: at, size: 5, kind: HoleKind::DeadFragment };
    s.mem.holes.push(hole);
    s.mem.holes.sort_by_key(|h| (h.chunk, h.offset));
    s.mem.frag_uses.retain(|u| u.hash != hash);
    let mut b3 = b.clone();
    b3.strs.clear();
    s.model = vec![MFile { strs: Vec::new(), ..a.clone() }, b3.clone()];
    let (s4, d4) = mhop(&s, &[a, b3], &[1]).unwrap();
    assert!(d4.frags.iter().any(|f| f.hash == hash && f.refs == 1 && f.offset == at));
    assert!(!s4.mem.holes.contains(&hole), "its hole gone");
}

// spec: incr.md IN-72 item 6 ("its tail exhausted: reserve-exhausted"; R-1:
// a `.debug_*` piece is never counted, `(debug)`; a merged section whose
// pieces the join did not give is refused, never guessed)
#[test]
fn item6_refusals() {
    let s0 = mfirst(&[mfile(&[1]), mfile(&[2])], &[]);
    // Past the merged chunk's capacity: item 3's growth step (the
    // segment's reserve), and past that `reserve-exhausted`.
    let many: Vec<u8> = (10..250).collect();
    let (_, d) = mhop(&s0, &[mfile(&many), mfile(&[2])], &[1]).unwrap();
    assert_eq!(d.growth_steps.len(), 1, "{:?}", d.growth_steps);
    assert_eq!(d.growth_steps[0].out, s0.rodata);
    let mut huge = mfile(&[1]);
    huge.strs = vec![(1, 1 << 40)];
    let e = mhop(&s0, &[huge, mfile(&[2])], &[1]).err().unwrap();
    assert_eq!(e.name(), "reserve-exhausted", "{e:?}");
    // A piece of a section the update does not name.
    let e = mhop_with(&s0, &[mfile(&[1]), mfile(&[2])], &[1], &|p| {
        p[0].pieces[0].section = SectionId(10_000);
    })
    .err();
    assert_eq!(e, Some(not_implemented()), "a piece of no section of the update");
    // No pieces given for a live merged section.
    let e = mhop_with(&s0, &[mfile(&[1]), mfile(&[2])], &[1], &|p| p.clear()).err();
    assert_eq!(e, Some(not_implemented()));
}

// spec: incr.md IN-72 item 6 (R-1: `.debug_*` has no fragments; a piece of
// one is `(debug)`, never counted)
#[test]
fn item6_debug_piece_is_debug() {
    let files = vec![vec![text(b".text", 16)], vec![text(b".text", 16)]];
    let s0 = first(Machine::X86_64, &files);
    let mut next = s0.files();
    next[0].push(sec(b".debug_str", SHT_PROGBITS, SHF_MERGE | SHF_STRINGS, 32, 0));
    let (changes, _) = changes_of(&s0, &Edit::new(next).changed(&[1]));
    let dbg = changes.iter().find(|c| c.key.name == b".debug_str").unwrap().id;
    let pieces = [FilePieces {
        file: FileId(1),
        pieces: vec![MergedPiece {
            section: dbg,
            offset: 0,
            size: 4,
            out: OutSecId(0),
            class: STR,
            hash: [3; 16],
        }],
    }];
    let rows = Counting::new(&s0.mem);
    let inp = LayoutInput { pieces: &pieces, ..input(&changes) };
    let e = layout(&StoredLayout::new(&rows), &inp, &s0.world.target).err();
    assert_eq!(e, Some(IncrReason::UpdateNotImplemented(Some(Unimplemented::Debug))));
    assert_eq!(rows.frags.load(Ordering::Relaxed), 0, "never counted");
}

// ---- item 5: `.eh_frame` (WP-243) --------------------------------------------

/// A first link of two text files with an `.eh_frame` chunk whose ranges
/// are file 1's (24 + 40 bytes, capacity 80) and file 2's (40, capacity
/// 56), as the capture stores them.
fn first_with_eh(files: &[Vec<Sec>]) -> (State, ChunkId) {
    use mortar_core::layout::Synth;
    let mut w = world(Machine::X86_64, files);
    w.synthetic = vec![mortar_layout::chunk::SynthChunk {
        synth: Synth { name: b".eh_frame", address_dependent: false },
        sh_type: SHT_PROGBITS,
        flags: SHF_ALLOC,
        align: 3,
        size: 140,
    }];
    let mut s0 = first_with(w, no_anchors(files));
    let eh = s0
        .mem
        .chunks
        .iter()
        .position(|c| matches!(c.kind, ChunkKind::Synthetic(s) if s.name == b".eh_frame"))
        .map(|i| ChunkId(i as u32))
        .expect("the .eh_frame chunk");
    s0.mem.eh_ranges = vec![
        (FileId(1), StoredEhRange { offset: 0, capacity: 80, records: 2 }),
        (FileId(2), StoredEhRange { offset: 80, capacity: 56, records: 1 }),
    ];
    let pad = |offset, size| Hole { chunk: eh, offset, size, kind: HoleKind::EhPad };
    s0.mem.holes.extend([pad(64, 16), pad(120, 16)]);
    s0.mem.holes.sort_by_key(|h| (h.chunk, h.offset));
    (s0, eh)
}

fn eh_file(file: u32, sizes: &[u64]) -> EhFile {
    EhFile {
        file: FileId(file),
        rank: u64::from(file) << 20,
        records: sizes
            .iter()
            .enumerate()
            .map(|(i, &size)| EhRecordIn {
                section: SectionId(1),
                piece: i as u32,
                size,
                cie: i == 0,
            })
            .collect(),
    }
}

// spec: incr.md IN-34a, IN-72 item 5 (WP-243: a changed file's records
// fit its range: rewritten in place, the rest a pad, through `layout`;
// IN-18c: one `FILES` eh read per file of the update, one `HOLES` read of
// the `.eh_frame` chunk)
#[test]
fn eh_frame_range_rewritten_in_place() {
    let (s0, eh) = first_with_eh(&two_text_files());
    let mut files = s0.files();
    files[0][0].size = 60;
    let (changes, _) = changes_of(&s0, &Edit::new(files).changed(&[1]));
    let t = &s0.world.target;
    let plain = Counting::new(&s0.mem);
    let base = layout(&StoredLayout::new(&plain), &input(&changes), t);
    // A file with a range and no `EhFile`: its records are not known.
    assert_eq!(base.err(), Some(not_implemented()));
    let ehf = [eh_file(1, &[24, 40, 8])];
    let rows = Counting::new(&s0.mem);
    let inp = LayoutInput { eh: &ehf, ..input(&changes) };
    let d = layout(&StoredLayout::new(&rows), &inp, t).unwrap();
    assert_eq!(rows.ranges.load(Ordering::Relaxed), 3, "one FILES eh read for the one file");
    let without = {
        let mut m = s0.mem.clone();
        m.eh_ranges.clear();
        m.holes.retain(|h| h.chunk != eh);
        let r = Counting::new(&m);
        layout(&StoredLayout::new(&r), &input(&changes), t).unwrap();
        r.holes.load(Ordering::Relaxed)
    };
    assert_eq!(rows.holes.load(Ordering::Relaxed), without + 1, "one HOLES read of .eh_frame");
    let got = d.eh.clone().expect("item 5's layout");
    assert_eq!(got.chunk, eh);
    assert!(!got.whole);
    assert_eq!(got.ranges.len(), 1);
    assert_eq!(got.ranges[0].file, FileId(1));
    assert_eq!((got.ranges[0].offset, got.ranges[0].capacity), (0, 80));
    assert_eq!(got.ranges[0].records, vec![0, 24, 64]);
    let stored = StoredLayout::new(&s0.mem);
    let pad = |offset, size| Hole { chunk: eh, offset, size, kind: HoleKind::EhPad };
    assert_eq!(d.holes_of(&stored, eh).unwrap(), vec![pad(72, 8), pad(120, 16)]);
    assert!(d.hole_chunks.binary_search(&eh).is_ok());
    assert!(d.holes.contains(&pad(72, 8)) && !d.holes.contains(&pad(120, 16)));
    assert_eq!(d.placement.chunks[eh], s0.placed.placement.chunks[eh], "the chunk is unchanged");
    // The same with the file gone: its range is a hole, merged with the
    // pad before it.
    let none = [eh_file(2, &[])];
    let mut files = s0.files();
    files[1][0].size = 50;
    let (changes2, _) = changes_of(&s0, &Edit::new(files).changed(&[2]));
    let d2 = layout(&stored, &LayoutInput { eh: &none, ..input(&changes2) }, t).unwrap();
    assert!(d2.eh.as_ref().unwrap().ranges.is_empty());
    assert_eq!(d2.holes_of(&stored, eh).unwrap(), vec![pad(64, 72)]);
    // A file with records and no range: the whole rewrite, which needs
    // every `FILES` row with a range (WP-243 F-1).
    let grown = [eh_file(1, &[24, 40, 40])];
    assert_eq!(
        layout(&stored, &LayoutInput { eh: &grown, ..inp }, t).err(),
        Some(not_implemented())
    );
}

// spec: incr.md IN-72 item 5 (WP-243: no `.eh_frame` in the update and
// none stored: nothing read past the `FILES` row, `eh` is `None`)
#[test]
fn eh_frame_untouched_is_none() {
    let (s0, eh) = first_with_eh(&two_text_files());
    let mut files = s0.files();
    files[0][0].size = 60;
    let (changes, _) = changes_of(&s0, &Edit::new(files).changed(&[1]));
    let mut m = s0.mem.clone();
    m.eh_ranges.retain(|e| e.0 != FileId(1));
    let d = layout(&StoredLayout::new(&m), &input(&changes), &s0.world.target).unwrap();
    assert_eq!(d.eh, None);
    assert!(d.hole_chunks.binary_search(&eh).is_err());
}

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
use mortar_core::id::{ChunkId, FileId, OutSecId, SectionId};
use mortar_core::identity::{Anchor, SectionKey};
use mortar_core::incr::{
    IncrReason, LayoutDelta, LayoutInput, LayoutRows, MemoryLayout, SectionChange, StoredLayout,
    StoredRegion, StoredSection, StoredSlot, SymtabBlock, SymtabTables, Unimplemented,
};
use mortar_core::layout::{
    Chunk, ChunkKind, Defined, Hole, HoleKind, Patch, Place, Placement, Redirect, Segment,
    SymPlace, Tls,
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
        frag_at: Vec::new(),
        eh_records: Vec::new(),
        uses: Vec::new(),
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
    let delta = layout(
        &StoredLayout::new(&prev.mem),
        &LayoutInput { changes: &changes, ..LayoutInput::default() },
        &prev.world.target,
    )?;
    let next = overlay(prev, edit, &changes, &ids2, &delta);
    Ok((next, delta))
}

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
    // Places, in the new world's ids.
    let mut place: Column<SectionId, Place> = Column::filled(w2.headers.len(), Place::NONE);
    let by_new: BTreeMap<SectionId, SectionId> =
        changes.iter().zip(ids2).map(|(c, &s2)| (s2, c.id)).collect();
    for f in 1..=nfiles(&w2) {
        for i in 1..=file_len(&w2, f) {
            let s2 = w2.sid(f, i);
            place[s2] = match by_new.get(&s2) {
                Some(&id) => d.place.get(id).copied().unwrap_or(Place::NONE),
                None => p1.place[w.sid(f, i)],
            };
        }
    }
    // Holes: the delta's for the chunks it touched.
    let files: Vec<FileId> = changes.iter().map(|c| c.file).collect();
    let mut touched: Vec<ChunkId> = prev
        .regions
        .iter()
        .filter(|r| files.contains(&r.file))
        .map(|r| r.chunk)
        .chain(delta.regions_opened.iter().map(|r| r.chunk))
        .chain(d.holes.iter().map(|h| h.chunk))
        .collect();
    touched.sort_unstable();
    touched.dedup();
    let mut holes: Vec<Hole> = p1
        .holes
        .iter()
        .filter(|h| touched.binary_search(&h.chunk).is_err())
        .chain(d.holes.iter())
        .copied()
        .collect();
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
// function dies; `.eh_frame` ranges are not in the seam: the fall-back)
#[test]
fn cross_file_eh_frame_fde_target_dies() {
    let eh = sec(b".eh_frame", SHT_PROGBITS, SHF_ALLOC, 56, 3);
    let mut files = two_text_files();
    files[1].push(eh);
    let s0 = first(Machine::X86_64, &files);
    let e = hop(&s0, &Edit::new(files).drawn(&[2]).dead(&[(2, 1)])).err().unwrap();
    assert_eq!(e, not_implemented());
}

// spec: incr.md IN-72 item 10 (a section with no output section in the
// stored layout: `new-output-section`)
#[test]
fn section_without_output_is_new_output_section() {
    let s0 = first(Machine::X86_64, &two_text_files());
    let mut files = s0.files();
    files[0].push(sec(b"my_section", SHT_PROGBITS, WA, 8, 3));
    let (changes, _) = changes_of(&s0, &Edit::new(files).changed(&[1]));
    let e = layout(
        &StoredLayout::new(&s0.mem),
        &LayoutInput { changes: &changes, ..LayoutInput::default() },
        &s0.world.target,
    )
    .err()
    .unwrap();
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
}

impl<'a> Counting<'a> {
    fn new(inner: &'a MemoryLayout) -> Self {
        Self {
            inner,
            sections: AtomicUsize::new(0),
            regions: AtomicUsize::new(0),
            holes: AtomicUsize::new(0),
            frags: AtomicUsize::new(0),
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
        hash: [u8; 16],
    ) -> Result<Vec<mortar_core::incr::StoredFrag>, IncrReason> {
        self.frags.fetch_add(1, Ordering::Relaxed);
        self.inner.frags_of(out, hash)
    }
    fn slots(&self) -> Result<Vec<StoredSlot>, IncrReason> {
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
    let d = layout(
        &StoredLayout::new(&rows),
        &LayoutInput { changes: &changes, ..LayoutInput::default() },
        &s0.world.target,
    )
    .unwrap();
    assert_eq!(rows.sections.load(Ordering::Relaxed), 2, "one SECTIONS range per file");
    assert_eq!(rows.regions.load(Ordering::Relaxed), 2, "one REGIONS range per file");
    // The chunks of the two files' regions (.text, .data, .bss, .rodata)
    // and the tail chunk of .data: at most 4.
    assert!(rows.holes.load(Ordering::Relaxed) <= 4, "{}", rows.holes.load(Ordering::Relaxed));
    assert_eq!(rows.frags.load(Ordering::Relaxed), 0);
    assert_eq!(d.regions_opened.len(), 1);
}

// spec: incr.md IN-72 (determinism, IN-6: no dependence on the thread
// count)
#[test]
fn delta_layout_same_on_1_and_n_threads() {
    let (s0, hops) = m1_world("o2fs");
    let (changes, _) = changes_of(&s0, &hops[3]);
    let one = layout(
        &StoredLayout::new(&s0.mem),
        &LayoutInput { changes: &changes, ..LayoutInput::default() },
        &s0.world.target,
    );
    let many: Vec<_> = std::thread::scope(|sc| {
        let h: Vec<_> = (0..4)
            .map(|_| {
                sc.spawn(|| {
                    layout(
                        &StoredLayout::new(&s0.mem),
                        &LayoutInput { changes: &changes, ..LayoutInput::default() },
                        &s0.world.target,
                    )
                })
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

// Item 11 (a winning common is a section of its winning file) needs each
// common's SymbolId, size and alignment in the update's input, and
// `SectionChange` carries none: `LayoutDelta::commons` stays empty
// (WP-216.md, R-2). These tests wait for that ruling.

#[test]
#[ignore = "item 11: no common input in SectionChange (WP-216.md R-2)"]
fn n4_common_grows() {}

#[test]
#[ignore = "item 11: no common input in SectionChange (WP-216.md R-2)"]
fn common_appears() {}

#[test]
#[ignore = "item 11: no common input in SectionChange (WP-216.md R-2)"]
fn two_commons_one_grows() {}

#[test]
#[ignore = "item 11: no common input in SectionChange (WP-216.md R-2)"]
fn common_winner_moves_to_unchanged_file() {}

// IN-28b's slots: the order of IRELATIVE slots needs the update's slot
// demand (which IFUNCs a changed file now needs), and the input has none
// (WP-216.md R-4).
#[test]
#[ignore = "IN-28b slots: no slot demand input (WP-216.md R-4)"]
fn two_ifuncs_added_irelative_order() {}

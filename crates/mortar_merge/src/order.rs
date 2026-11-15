//! Output order and offsets of the merged fragments (`spec/synth.md`
//! §3.7, S-24 to S-27, S-29): one `Merged(OutSecId)` chunk per output
//! section with a live fragment (layout L-85).

use std::ops::Range;

use mortar_core::par::Pool;
use mortar_core::{Column, FragId, MergeMap, OutSecId, Span, Split};

use crate::merge::MergeMeta;

/// A `Merged(OutSecId)` chunk (S-26): its size is the end of its last
/// fragment, its alignment the largest of its fragments'.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MergedChunk {
    pub out: OutSecId,
    pub size: u64,
    /// log2 of the alignment.
    pub align: u8,
    /// Its fragments in offset order: a range of [`Order::order`].
    pub frags: Span,
}

/// The output order of the live fragments (S-24) and their offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Order {
    /// `frag_off`: offsets in the fragment's chunk; `MergeMap::NONE` for
    /// a dead fragment.
    pub map: MergeMap,
    /// The chunks, in `OutSecId` order.
    pub chunks: Box<[MergedChunk]>,
    /// The live fragments, chunk after chunk, each chunk's in offset
    /// order.
    pub order: Box<[FragId]>,
}

impl Order {
    /// The fragments of chunk `c` that overlap the chunk byte range
    /// `range`, in offset order: a binary search, so writing any byte range
    /// of a merged chunk costs `O(range + log n)` (`spec/image.md` §5.2,
    /// §13 "divisible"). Bytes between them are zero padding (S-27).
    #[must_use]
    pub fn frags_in(&self, split: &Split, c: usize, range: Range<u64>) -> &[FragId] {
        let span = self.chunks[c].frags;
        let all = &self.order[span.start as usize..(span.start + span.len) as usize];
        let end = |f: &FragId| self.map.frag_off[*f] + u64::from(split.frags[*f].size);
        let a = all.partition_point(|f| end(f) <= range.start);
        let b = all.partition_point(|f| self.map.frag_off[*f] < range.end);
        &all[a..b.max(a)]
    }
}

/// S-24: the sort key of a fragment: output section, then strings before
/// constants, entsize ascending, alignment ascending, then `FragId`.
fn key(m: MergeMeta, f: FragId) -> (OutSecId, bool, u32, u8, FragId) {
    (m.out, !m.strings, m.entsize, m.align, f)
}

/// The identity of every fragment (through its representative's row):
/// `meta` is per `split.merge_of` row. The representatives are in row
/// order (S-15), so one walk gives them.
#[must_use]
pub fn frag_meta(split: &Split, meta: &[MergeMeta]) -> Vec<MergeMeta> {
    let mut out = Vec::with_capacity(split.frags.len() as usize);
    let mut row = 0usize;
    for (_, frag) in split.frags.iter() {
        while split.merge_of[row].section != frag.section {
            row += 1;
        }
        out.push(meta[row]);
    }
    out
}

/// S-24 to S-26, S-29: the offset of every live fragment in its output
/// section's `Merged` chunk, and the chunks. `meta` is per
/// `split.merge_of` row (`phase::Phase::meta`);
/// `frag_live` is `Liveness.frag_live`. The sort is parallel; the aligned
/// prefix sum is one sequential pass (S-29). Padding is zero bytes
/// (S-27), written by the image phase.
#[must_use]
pub fn offsets(
    pool: &Pool,
    split: &Split,
    meta: &[MergeMeta],
    frag_live: &Column<FragId, bool>,
) -> Order {
    let n = split.frags.len();
    let mut steps = mortar_core::wp032b_steps::Steps::new("order");
    let meta_of = frag_meta(split, meta);
    let mut live: Vec<FragId> =
        (0..n).map(FragId).filter(|&f| frag_live.get(f).copied().unwrap_or(false)).collect();
    steps.step("meta", u64::from(n), "frag");
    pool.unit_cost(25).sort_by_key(&mut live, |&f| key(meta_of[f.0 as usize], f));
    steps.step("sort", live.len() as u64, "frag");
    let mut off = vec![MergeMap::NONE; n as usize];
    let mut chunks: Vec<MergedChunk> = Vec::new();
    for (i, &f) in live.iter().enumerate() {
        let m = meta_of[f.0 as usize];
        let size = u64::from(split.frags[f].size);
        let chunk = match chunks.last_mut() {
            Some(c) if c.out == m.out => c,
            _ => {
                let frags = Span { start: i as u32, len: 0 };
                chunks.push(MergedChunk { out: m.out, size: 0, align: 0, frags });
                chunks.last_mut().unwrap_or_else(|| unreachable!("just pushed"))
            }
        };
        let at = chunk.size.next_multiple_of(1u64 << m.align);
        off[f.0 as usize] = at;
        chunk.size = at + size;
        chunk.align = chunk.align.max(m.align);
        chunk.frags.len += 1;
    }
    steps.step("offsets", live.len() as u64, "frag");
    let frag_off = Column::from_vec(off)
        .unwrap_or_else(|_| unreachable!("one row per fragment, which fit u32"));
    Order { map: MergeMap { frag_off }, chunks: chunks.into(), order: live.into() }
}

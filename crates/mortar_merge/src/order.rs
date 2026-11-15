//! Output order and offsets of the merged fragments (`spec/synth.md`
//! §3.7, S-24 to S-27, S-29): one `Merged(OutSecId)` chunk per output
//! section with a live fragment (layout L-85).

use mortar_core::par::Pool;
use mortar_core::{Column, FragId, MergeMap, OutSecId};

use crate::merge::{MergeMeta, Merged};

/// A `Merged(OutSecId)` chunk (S-26): its size is the end of its last
/// fragment, its alignment the largest of its fragments'.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MergedChunk {
    pub out: OutSecId,
    pub size: u64,
    /// log2 of the alignment.
    pub align: u8,
}

/// S-24: the sort key of a fragment: output section, then strings before
/// constants, entsize ascending, alignment ascending, then `FragId`.
fn key(m: MergeMeta, f: FragId) -> (OutSecId, bool, u32, u8, FragId) {
    (m.out, !m.strings, m.entsize, m.align, f)
}

/// S-24 to S-26, S-29: the offset of every live fragment in its output
/// section's `Merged` chunk (`MergeMap::NONE` for a dead one), and the
/// chunks in `OutSecId` order. `frag_live` is `Liveness.frag_live`. The
/// sort is parallel; the aligned prefix sum is one sequential pass (S-29).
/// Padding is zero bytes (S-27), written by the image phase.
#[must_use]
pub fn offsets(
    pool: &Pool,
    merged: &Merged,
    frag_live: &Column<FragId, bool>,
) -> (MergeMap, Vec<MergedChunk>) {
    let n = merged.frags.len();
    // The meta of each fragment: through its representative's row; the
    // representatives are in row order, so one walk gives them.
    let mut meta_of: Vec<MergeMeta> = Vec::with_capacity(n as usize);
    let mut row = 0usize;
    for (_, frag) in merged.frags.iter() {
        while merged.merge_of[row].section != frag.section {
            row += 1;
        }
        meta_of.push(merged.meta[row]);
    }
    let mut live: Vec<FragId> =
        (0..n).map(FragId).filter(|&f| frag_live.get(f).copied().unwrap_or(false)).collect();
    pool.sort_by_key(&mut live, |&f| key(meta_of[f.0 as usize], f));
    let mut off = vec![MergeMap::NONE; n as usize];
    let mut chunks: Vec<MergedChunk> = Vec::new();
    for &f in &live {
        let m = meta_of[f.0 as usize];
        let size = u64::from(merged.frags[f].size);
        let chunk = match chunks.last_mut() {
            Some(c) if c.out == m.out => c,
            _ => {
                chunks.push(MergedChunk { out: m.out, size: 0, align: 0 });
                chunks.last_mut().unwrap_or_else(|| unreachable!("just pushed"))
            }
        };
        let at = chunk.size.next_multiple_of(1u64 << m.align);
        off[f.0 as usize] = at;
        chunk.size = at + size;
        chunk.align = chunk.align.max(m.align);
    }
    let frag_off = Column::from_vec(off)
        .unwrap_or_else(|_| unreachable!("one row per fragment, which fit u32"));
    (MergeMap { frag_off }, chunks)
}

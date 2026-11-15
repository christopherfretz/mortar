//! `.gnu.hash` (`spec/dyn.md` D-20, D-26; `spec/synth.md` S-102): the
//! GNU hash table over the hashed `.dynsym` entries of a [`DynsymPlan`].
//!
//! For `n` hashed entries and `W` the word size in bits:
//!
//! - `nbuckets = max(1, ⌊n/4⌋)` (the plan's, [`DynsymPlan::nbuckets`]);
//! - `bloom_shift = 26`;
//! - `maskwords` = the least power of two strictly greater than
//!   `⌊12n/W⌋`;
//! - two Bloom bits per hashed entry, `h mod W` and `(h >> 26) mod W`, in
//!   word `(h / W) mod maskwords`;
//! - bucket `b` holds the least `.dynsym` index of the entries of bucket
//!   `b` (0 for an empty one); the chain word of each hashed entry is
//!   `h & !1`, with bit 0 set on the last entry of its bucket;
//! - `symoffset` = the index of the first hashed entry (the `.dynsym`
//!   count when `n` is 0).
//!
//! The size is a function of `n` and the class (S-108); the bytes are
//! written whole (S-4: the Bloom words need every entry), with the names
//! hashed in parallel.

use std::ops::Range;

use mortar_core::par::Pool;
use mortar_elf::Class;

use crate::dynsym::{DynSymSource, DynsymPlan, elf64};
use crate::error::{SynthError, internal};
use crate::image::{Division, FillCx, SizeCx, SynthChunk};

/// D-26's `bloom_shift`.
pub const BLOOM_SHIFT: u32 = 26;

/// The GNU hash function (Bernstein's: `h × 33 + c` from 5381, modulo
/// 2^32), as glibc's `dl_new_hash`.
#[must_use]
pub fn gnu_hash(name: &[u8]) -> u32 {
    name.iter().fold(5381u32, |h, &c| h.wrapping_mul(33).wrapping_add(u32::from(c)))
}

/// D-26's `nbuckets` for `n` hashed entries: `max(1, ⌊n/4⌋)`.
#[must_use]
pub fn nbuckets_for(n: u32) -> u32 {
    (n / 4).max(1)
}

/// D-26's `maskwords` for `n` hashed entries and words of `w` bits: the
/// least power of two strictly greater than `⌊12n/w⌋` (1 when that is 0).
#[must_use]
pub fn maskwords_for(n: u32, w: u32) -> u32 {
    let q = (12 * u64::from(n)) / u64::from(w);
    // The least power of two > q; q < 2^34, so this fits u64 and, for
    // any n a `.dynsym` can hold (n < 2^32 / 24), u32.
    let p = (q + 1).next_power_of_two();
    u32::try_from(p).unwrap_or(u32::MAX)
}

/// The header fields and table sizes of a `.gnu.hash` (D-26).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GnuHashShape {
    /// Hashed entries.
    pub n: u32,
    pub nbuckets: u32,
    pub symoffset: u32,
    pub maskwords: u32,
    /// Bytes per Bloom word: 8 (ELF64) or 4 (ELF32).
    pub word: u32,
}

impl GnuHashShape {
    /// The shape of `plan`'s table for an output of `class`.
    #[must_use]
    pub fn of(plan: &DynsymPlan, class: Class) -> Self {
        let n = plan.order().len() as u32 - plan.unhashed();
        let word = class.word_size() as u32;
        Self {
            n,
            nbuckets: plan.nbuckets().max(1),
            symoffset: 1 + plan.unhashed(),
            maskwords: maskwords_for(n, word * 8),
            word,
        }
    }

    /// The size (S-108): `16 + (W/8) × maskwords + 4 × nbuckets + 4 × n`.
    #[must_use]
    pub fn size(&self) -> u64 {
        16 + u64::from(self.word) * u64::from(self.maskwords)
            + 4 * u64::from(self.nbuckets)
            + 4 * u64::from(self.n)
    }

    fn buckets_at(&self) -> usize {
        16 + (self.word * self.maskwords) as usize
    }

    fn chain_at(&self) -> usize {
        self.buckets_at() + 4 * self.nbuckets as usize
    }
}

/// The table's bytes for `plan`, whose hashed entries have the hashes
/// `hashes` (in `.dynsym` order), into `out` (exactly the shape's size).
/// Fails when the hashed entries are not sorted by bucket (a plan that is
/// not D-22's order, e.g. a broken fixture).
///
/// # Errors
///
/// `internal-error` when the plan is not sorted by bucket or `out` has the
/// wrong size.
pub fn write_table(shape: &GnuHashShape, hashes: &[u32], out: &mut [u8]) -> Result<(), SynthError> {
    if out.len() as u64 != shape.size() || hashes.len() != shape.n as usize {
        return Err(internal(".gnu.hash: the buffer or the hashes do not match the table's shape"));
    }
    out.fill(0);
    let le32 =
        |out: &mut [u8], at: usize, v: u32| out[at..at + 4].copy_from_slice(&v.to_le_bytes());
    le32(out, 0, shape.nbuckets);
    le32(out, 4, shape.symoffset);
    le32(out, 8, shape.maskwords);
    le32(out, 12, BLOOM_SHIFT);
    let w = shape.word * 8;
    let mut bloom = vec![0u64; shape.maskwords as usize];
    for &h in hashes {
        let word = ((h / w) % shape.maskwords) as usize;
        bloom[word] |= 1u64 << (h % w) | 1u64 << ((h >> BLOOM_SHIFT) % w);
    }
    let width = shape.word as usize;
    for (i, b) in bloom.iter().enumerate() {
        let at = 16 + i * width;
        out[at..at + width].copy_from_slice(&b.to_le_bytes()[..width]);
    }
    let nb = shape.nbuckets;
    let mut last_bucket = None;
    for (i, &h) in hashes.iter().enumerate() {
        let b = h % nb;
        let index = shape.symoffset + i as u32;
        if last_bucket.is_some_and(|l| b < l) {
            return Err(internal(format!(
                ".gnu.hash: .dynsym entry {index} is out of bucket order (spec/dyn.md D-22)"
            )));
        }
        if last_bucket != Some(b) {
            le32(out, shape.buckets_at() + 4 * b as usize, index);
        }
        last_bucket = Some(b);
        let last = hashes.get(i + 1).is_none_or(|&next| next % nb != b);
        le32(out, shape.chain_at() + 4 * i, (h & !1) | u32::from(last));
    }
    Ok(())
}

/// The hashes of the hashed entries of `plan`, in `.dynsym` order.
#[must_use]
pub fn hashes(plan: &DynsymPlan, src: &dyn DynSymSource, pool: &Pool) -> Vec<u32> {
    let hashed = &plan.order()[plan.unhashed() as usize..];
    pool.unit_cost(40).map(hashed.len(), |i| gnu_hash(src.symbol(hashed[i])))
}

/// The `.gnu.hash` chunk over a plan and the names of its keys.
#[derive(Clone, Copy)]
pub struct GnuHashChunk<'a> {
    pub plan: &'a DynsymPlan,
    pub src: &'a dyn DynSymSource,
    pub class: Class,
}

impl std::fmt::Debug for GnuHashChunk<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GnuHashChunk").field("plan", self.plan).finish_non_exhaustive()
    }
}

impl GnuHashChunk<'_> {
    fn shape(&self) -> GnuHashShape {
        GnuHashShape::of(self.plan, self.class)
    }
}

impl SynthChunk for GnuHashChunk<'_> {
    fn size(&self, _: &SizeCx<'_>) -> u64 {
        self.shape().size()
    }

    /// Whole: every Bloom word depends on every hashed entry.
    fn division(&self) -> Division {
        Division::Whole { max_size: self.shape().size() }
    }

    fn write(&self, cx: &FillCx<'_>, range: Range<u64>, out: &mut [u8]) -> Result<(), SynthError> {
        elf64(self.class)?;
        let shape = self.shape();
        crate::dynsym::whole_range(&range, out, shape.size(), |buf| {
            write_table(&shape, &hashes(self.plan, self.src, cx.pool), buf)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // D-26: the closed forms.
    #[test]
    fn closed_forms() {
        assert_eq!(gnu_hash(b""), 5381);
        assert_eq!(gnu_hash(b"printf"), 0x156b_2bb8);
        assert_eq!((nbuckets_for(0), nbuckets_for(7), nbuckets_for(8)), (1, 1, 2));
        assert_eq!(maskwords_for(0, 64), 1);
        assert_eq!(maskwords_for(5, 64), 1);
        assert_eq!(maskwords_for(6, 64), 2);
        assert_eq!(maskwords_for(11, 64), 4);
        assert_eq!(maskwords_for(1000, 64), 256);
        assert_eq!(maskwords_for(1000, 32), 512);
    }
}

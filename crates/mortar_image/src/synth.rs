//! The synthetic-chunk interface of `spec/image.md` §13.
//!
//! Stand-in: `mortar_synth` (WP-025) owns this trait. It is declared here
//! exactly as §13 states it, so that WP-025's chunks drop in; the stand-in
//! is deleted when WP-025 merges.

use std::ops::Range;

use mortar_arch::{FieldError, FillCx, RefName};

/// What `size` reads (§13): the layout's view. Empty until WP-025 states
/// it.
#[derive(Debug, Clone, Copy, Default)]
pub struct SizeCx<'a> {
    _tables: std::marker::PhantomData<&'a ()>,
}

/// How a chunk may be split between jobs (I-104).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Division {
    /// Any range, at `O(range + log chunk + 2^10)`.
    Divisible,
    /// One job writes the whole chunk into one buffer; `max_size` is the
    /// declared bound (I-92's `X`).
    Whole { max_size: u64 },
}

/// A failure of `write` (I-106): reported at `(100, 0, ChunkId)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SynthError {
    /// A field's value (`relocation-out-of-range`, `relocation-misaligned`):
    /// at chunk offset `offset`, the field `what`, for `target`.
    Field { error: FieldError, offset: u64, what: RefName, target: String },
    /// `internal-error`.
    Internal(String),
}

/// One synthetic chunk (§13).
pub trait SynthChunk: Sync {
    /// The chunk's size.
    fn size(&self, cx: &SizeCx<'_>) -> u64;
    /// `Divisible` or `Whole { max_size }` (I-104).
    fn division(&self) -> Division;
    /// The bytes `[range.start, range.end)` of the chunk into `out`
    /// (`out.len() == range.len()`); a function of the tables and the range
    /// only (I-103).
    fn write(&self, cx: &FillCx<'_>, range: Range<u64>, out: &mut [u8]) -> Result<(), SynthError>;
    /// The range filled with zeros now and written by the sealing pass
    /// (the build id's descriptor, I-107).
    fn sealed(&self) -> Option<Range<u64>>;
}

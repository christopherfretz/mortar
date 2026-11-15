//! Split and merge (`spec/synth.md` §2-§4, phase 60): cutting `SHF_MERGE`
//! sections and `.eh_frame` into pieces, and the deterministic parallel
//! merge of equal pieces into fragments.
//!
//! - [`cut`]: which `Merge` sections are split (S-6) and their pieces
//!   (S-7 to S-11);
//! - [`merge()`]: equal pieces into fragments numbered by first occurrence
//!   (S-13 to S-15, S-28);
//! - [`order`]: the fragments' output order and offsets (S-24 to S-29);
//! - [`eh`]: `.eh_frame` records, their CIEs and code sections (S-34 to
//!   S-40, S-43, S-45), the record list `mortar_synth` consumes;
//! - [`refs`]: references into split sections (S-17 to S-22, S-50);
//! - [`problem`]: what is refused or malformed, with its key and wording.
#![forbid(unsafe_code)]

pub mod cut;
pub mod eh;
pub mod merge;
pub mod order;
pub mod phase;
pub mod problem;
pub mod refs;

pub use eh::{EhFrames, EhIn, EhReloc};
pub use merge::{MergeIn, MergeMeta, Merged, merge};
pub use problem::{PHASE, Problem, What};

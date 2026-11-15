//! Split and merge (`spec/synth.md` §2-§4, phase 60): cutting `SHF_MERGE`
//! sections and `.eh_frame` into pieces, and the deterministic parallel
//! merge of equal pieces into fragments.
//!
//! - [`cut`]: which `Merge` sections are split (S-6) and their pieces
//!   (S-7 to S-11);
//! - [`merge`]: equal pieces into fragments numbered by first occurrence
//!   (S-13 to S-15, S-28);
//! - [`problem`]: what is refused or malformed, with its key and wording.
#![forbid(unsafe_code)]

pub mod cut;
pub mod merge;
pub mod problem;

pub use merge::{MergeIn, MergeMeta, Merged, merge};
pub use problem::{PHASE, Problem, What};

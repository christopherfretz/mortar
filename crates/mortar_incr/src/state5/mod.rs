//! The state's format version 5 (`spec/incr.md` IN-18c..IN-18f, IN-46b):
//! read by block and lazily ([`open`]), planned before W1
//! ([`plan_patch`]) and patched in place at W5 ([`patch`]),
//! compacted ([`compact`]), compared canonically ([`canon`], IN-78).
//! WP-213 (U-1) created the files and entry points; **filled by U-2**,
//! which also owns `crate::format` and `crate::capture` after U-1. The
//! row types are `crate::update::rows` (U-1's).

pub mod canon;
pub mod compact;
pub mod read;
pub mod write;

pub use canon::{Canon, CanonDiff, canon, compare};
pub use compact::compact;
pub use read::{LayoutOf, open};
pub use write::{PatchPlan, patch, plan_patch};

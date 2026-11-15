//! The input phase (`spec/input.md`): which files a link names, how they
//! are found, sniffed, read and loaded into `Inputs`. WP-021 fills this
//! crate.
//!
//! - [`store`]: the two stores of §11, staged (the default) and resident
//!   (parallel positioned reads, the `input-changed` checks, the §1 byte
//!   API).
//! - [`staged`]: what the staged store keeps resident (§11.1 (b)).
//! - [`facts`]: what a file is, from its bytes (§2), and the per-file
//!   analysis the walk consumes (§6, §7.1, §8.1).
#![forbid(unsafe_code)]

pub use load::{LoadEnv, dependency_paths, load};
pub use store::{Store, StorePolicy};

pub mod facts;
pub mod load;
pub mod problem;
pub mod search;
pub mod staged;
pub mod store;
pub mod walk;

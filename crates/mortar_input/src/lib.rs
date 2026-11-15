//! The input phase (`spec/input.md`): which files a link names, how they
//! are found, sniffed, read and loaded into `Inputs`. WP-021 fills this
//! crate.
//!
//! - [`store`]: the resident store of §11 (parallel chunked reads, the
//!   `input-changed` check, the §1 byte API).
//! - [`facts`]: what a file is, from its bytes (§2), and the per-file
//!   analysis the walk consumes (§6, §7.1, §8.1).
#![forbid(unsafe_code)]

pub use load::{LoadEnv, dependency_paths, load};
pub use store::{Store, StorePolicy};

pub mod facts;
pub mod load;
pub mod problem;
pub mod search;
pub mod store;
pub mod walk;

//! The incremental update (`spec/incr.md` §5.10, IN-70..IN-79): the
//! update's steps that live in `mortar_incr`, one file per package of the
//! cut (`spec/landed/incr/WP-207.md` §C, amended by WP-211 §C). WP-213
//! (U-1) created every file with its entry point, returning the value it
//! has "until it lands"; each package fills ITS function in ITS file. The
//! orchestration (IN-70's order) is `mortar::update` (U-10).
//!
//! | file | package | entry point |
//! |---|---|---|
//! | `view.rs`, `rows.rs`, `testing.rs` | U-1 | `StateView`, `StateRows`, rows, `MemoryState` |
//! | `resolve.rs`, `extract.rs`, `live.rs`, `defined.rs`, `refill.rs` | U-3 | `resolve`, `extract`, `live`, `recompute`, `refill_set` |
//! | `odr.rs` | U-8 | `odr` |
//! | `diags.rs` | U-9 | `diags` |
//! | `debug.rs` | G-1 | (none yet) |
//! | `split.rs` | G-2 | (none yet) |

pub mod debug;
pub mod defined;
pub mod diags;
pub mod extract;
pub mod live;
pub mod odr;
pub mod refill;
pub mod resolve;
pub mod rows;
pub mod split;
pub mod testing;
pub mod view;

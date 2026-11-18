//! The incremental update's orchestration (`spec/incr.md` IN-70: steps
//! U1..U10 in order) and its writes. WP-213 (U-1) created the files:
//! `write.rs` (**U-7**: IN-76, IN-77), `probe.rs` (**W-1**: IN-76a). The
//! orchestration itself (calling every entry point in IN-70's order, and
//! `Start::Update`) is **U-10**'s and is not wired: today's floor
//! (`crate::incremental`) runs exactly as before.

pub mod probe;
pub mod write;

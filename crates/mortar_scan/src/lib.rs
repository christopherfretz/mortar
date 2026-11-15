//! The scan driver (`spec/image.md` §4, phase 70): runs `Arch::scan` over
//! every relocation of every live section in parallel, merges the demands
//! deterministically, and turns reference events into one diagnostic per
//! key. WP-026s fills this crate.
#![forbid(unsafe_code)]

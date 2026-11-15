//! Layout (`spec/layout.md`): input sections to output sections, section
//! order, segments, addresses, linker-defined symbols, and the driver of
//! the range-extension fixed point of `spec/arch.md` §6.
//!
//! The stages of §7, each a function of the tables before it:
//!
//! | stage | module | writes |
//! |---|---|---|
//! | A. assign | [`assign`] | `Assignment` (then [`assign::finish`] after liveness) |
//! | B. order | [`order`] | the members of each output section |
//! | C. chunk | [`chunk`] | chunks, `order`, places within chunks |
//! | D. address | [`address`] | addresses, offsets, segments, `tls` |
//! | E. fixed point | [`fixed`] | patches and redirects; D again |
//! | F. symbols | [`symbols`] | `defined`, `sym_place`, `entry` |
#![forbid(unsafe_code)]

pub mod address;
pub mod assign;
pub mod chunk;
pub mod diag;
pub mod fixed;
pub mod headers;
pub mod order;
pub mod rank;
pub mod run;
pub mod stack;
pub mod symbols;
pub mod target;

pub use diag::LayoutDiag;
pub use fixed::{RangeExtension, Sites};
pub use headers::{Header, Headers};
pub use run::{Link, Placed, place};
pub use target::{Options, Target};

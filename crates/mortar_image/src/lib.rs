//! The image (`spec/image.md` §5-§11, phase 100): every byte of the output
//! file is filled window by window into per-worker buffers and written at
//! its file offset; the output file's protocol (a temporary, a lock, the
//! rename); tombstones; the map file; `--time-phases`.
//!
//! The surface:
//! - [`plan::Builder`] / [`plan::Plan`]: the fill plan (I-27) and its jobs
//!   (I-31) as data; [`layout_plan::from_layout`] builds one from the
//!   model's tables.
//! - [`fill::write`]: fills the plan's jobs in parallel and hands each
//!   window to a [`fill::Sink`] (I-33, I-63).
//! - [`output::Output`]: the output file (§7), a [`fill::Sink`].
//! - [`map::write_map`]: `mortar-map 1` (§9).
//! - [`phases`]: `--time-phases` lines (I-88) and the memory bound (I-92).
//!
//! Stand-ins for interfaces other packages own (each named in
//! `spec/landed/image/WP-026.md`): [`synth::SynthChunk`] (§13, WP-025),
//! the clipped `apply` wrapper, `target::Target` and the tombstone table
//! (WP-023b), [`plan::RecOrder`] (I-29, WP-026s).
#![forbid(unsafe_code)]
// A `Diagnostic` is the error value of every fallible step here, returned
// once per link: its size costs nothing on the success path. The spec's
// names (`a`, `b`, `r`, `P`, `S`) are kept in the fill arithmetic.
#![allow(clippy::result_large_err, clippy::many_single_char_names, clippy::cast_precision_loss)]

pub mod digest;
pub mod fill;
pub mod headers;
pub mod layout_plan;
pub mod map;
pub mod output;
pub mod phases;
pub mod plan;
mod reloc;
pub mod source;
pub mod synth;
pub mod target;
pub mod tombstone;

/// `W = IMAGE_BLOCK` (I-31, I-75): the window, and the build id's leaf.
pub const IMAGE_BLOCK: u64 = mortar_digest::IMAGE_BLOCK;

/// The phase ordinal of fill (model §8).
pub const PHASE_IMAGE: u16 = 100;
/// The phase ordinal of the output file (I-68, request R-4).
pub const PHASE_OUTPUT: u16 = 105;

pub use fill::{Image, Sink, write};
pub use output::{Output, OutputOptions};
pub use plan::{Builder, Plan};

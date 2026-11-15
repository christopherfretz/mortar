//! The image (`spec/image.md` §5-§7, §9-§11, phase 100): copy and relocate
//! every chunk into window buffers written at their file offsets, debug
//! sections and tombstones, the output file, the map file. WP-026 fills
//! this crate.
#![forbid(unsafe_code)]

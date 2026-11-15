//! Synthetic sections (`spec/synth.md` §5 onward, phase 80): `.eh_frame`
//! and `.eh_frame_hdr`, `.symtab` and the string tables, notes, the GOT and
//! the IPLT of static links, each sized as data before layout and written
//! by range for the image phase. WP-025 fills this crate.
#![forbid(unsafe_code)]

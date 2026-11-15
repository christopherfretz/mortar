//! Split and merge (`spec/synth.md` §2-§4, phase 60): cutting `SHF_MERGE`
//! sections and `.eh_frame` into pieces, and the deterministic parallel
//! merge of equal pieces into fragments. WP-025m fills this crate.
#![forbid(unsafe_code)]

//! Liveness (`spec/image.md` §3, phase 50): which sections and pieces are
//! live. M1: every section resolution keeps; later `--gc-sections` as a
//! parallel deterministic mark. WP-026s fills this crate.
#![forbid(unsafe_code)]

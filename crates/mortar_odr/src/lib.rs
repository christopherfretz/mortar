//! One-definition-rule checks (`spec/odr.md`): the checks on foreign
//! objects (W1-W4), the record checks on Mason units, and the tables for
//! dynamic checks. Depends on `mortar_core` and `mortar_meta`.
//!
//! The shell of the crate; the checks for foreign objects and the
//! `--odr=` switches land with WP-072, the record checks with WP-073.
#![forbid(unsafe_code)]

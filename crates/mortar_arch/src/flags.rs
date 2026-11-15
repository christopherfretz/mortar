//! The demand flags of §4.2: per `SymKey` and per output.
//!
//! The model adopted them (`spec/model.md` §3.7, amendment M-1), so they are
//! `mortar_core`'s types, re-exported: scan raises exactly the flags
//! `Demands` stores.

pub use mortar_core::demands::{DemandFlags, GlobalFlags};

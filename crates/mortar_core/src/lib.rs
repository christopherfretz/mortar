//! The shared vocabulary of the phases (`spec/model.md`): ids, tables, the
//! link plan, the deterministic parallel helpers. No algorithms.
//!
//! WP-020 fills this crate in two parts that do not share files: the ids,
//! columns, tables and the plan (`id` and the modules that part adds here);
//! and `par`, `hash` and `intern` (§6 and the numbering of §2.3).
//!
//! - [`id`] and [`column`]: ids, id ranges, [`SymKey`], and [`Column`], a
//!   `Vec` indexed by an id, with the split of §6 rule 1; [`idset`]: a set
//!   of ids as bits ([`IdSet`], the dynamic columns of `Resolution`).
//! - [`plan`]: the command line as data (§3.1), written by `mortar_cli`.
//! - The derived tables (§3): [`inputs`], [`resolution`], [`assignment`],
//!   [`split`], [`liveness`], [`demands`]. Each states the invariants it can
//!   check alone in a `check` method.
//! - The decision tables (§4), [`layout`], and their validity,
//!   [`Layout::check`] ([`check`]).
//! - [`identity`]: what survives between links (§7.1), and the dependees
//!   of the dependency index (§7.2).
//! - [`records`]: the record index the scan writes and fill reads (I-29).
//! - [`seed`]: a malformed scheduling seed as a diagnostic (§6).

mod flags;
mod table;

pub mod assignment;
pub mod check;
pub mod column;
pub mod demands;
pub mod hash;
pub mod id;
pub mod identity;
pub mod idset;
pub mod inputs;
pub mod intern;
pub mod layout;
pub mod liveness;
pub mod par;
pub mod place;
pub mod plan;
pub mod records;
pub mod resolution;
pub mod seed;
pub mod split;
pub mod wp032b_steps;

pub use assignment::Assignment;
pub use check::{CheckContext, Rule, Violation};
pub use column::{Column, SplitError};
pub use demands::Demands;
pub use id::{
    ArchiveId, ChunkId, ComdatId, FileId, FragId, Id, IdOverflow, IdRange, LocalSym, OptId,
    OutSecId, SectionId, SegmentId, Span, SymEntry, SymKey, SymKind, SymbolId,
};
pub use idset::IdSet;
pub use inputs::Inputs;
pub use layout::{
    CopyAlloc, CopyPlace, DynStr, Folds, Layout, MergeMap, Placement, PlacementKind,
    RelocatableLayout, Slots, VerIndex,
};
pub use liveness::Liveness;
pub use records::RecIndex;
pub use resolution::Resolution;
pub use split::Split;
pub use table::Broken;

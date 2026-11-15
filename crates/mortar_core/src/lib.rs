//! The shared vocabulary of the phases (`spec/model.md`): ids, tables, the
//! link plan, the deterministic parallel helpers. No algorithms.
//!
//! WP-020 fills this crate in two parts that do not share files: the ids,
//! columns, tables and the plan (`id` and the modules that part adds here);
//! and `par`, `hash` and `intern` (§6 and the numbering of §2.3).

pub mod hash;
pub mod id;
pub mod intern;
pub mod par;

pub use id::{
    ArchiveId, ChunkId, ComdatId, FileId, FragId, Id, OutSecId, SectionId, SegmentId, SymbolId,
};

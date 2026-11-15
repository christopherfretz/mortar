//! Synthetic sections (`spec/synth.md` §4.2 to §9, phase 80): what the
//! linker writes that no input section holds whole.
//!
//! Each section has two steps, both pure functions of the tables:
//!
//! - a **plan** step, run in phase 80 before layout, that fixes the
//!   section's order and size from `Split`, `Liveness`, `Demands`, `Slots`
//!   and the `Plan` (S-2, S-108). Plans are values; nothing in them
//!   depends on an address;
//! - a **write** step, run by the image phase once addresses are known,
//!   that fills any byte range of the section (S-4) in O(range + log
//!   size), from the plan, the inputs and an address oracle, through
//!   [`image::SynthChunk`].
//!
//! | module | sections | rules |
//! |---|---|---|
//! | [`eh`] | `.eh_frame` | S-41 to S-52 |
//! | [`hdr`] | `.eh_frame_hdr` | S-57 to S-62 |
//! | [`got`] | `.got`, `.got.plt`, the IPLT in `.plt`, `.rela.plt` | S-63 to S-72 |
//! | [`symtab`] | `.symtab`, `.strtab`, `.symtab_shndx` | S-73 to S-89, S-121 to S-124 |
//! | [`shstrtab`] | `.shstrtab` | S-90 |
//! | [`notes`] | `.note.gnu.build-id`, `.note.gnu.property`, notes of mixed alignment, `.comment`'s string | S-91 to S-100, S-126 |
//! | [`chunks`] | the S-3 list: names, types, flags, alignment; `Slots.sizes` | S-3, S-108 |
//! | [`image`] | the chunk interface of `spec/image.md` §13 over every plan | S-4, I-103 to I-108 |
//!
//! Inputs come through narrow traits ([`eh::EhSource`],
//! [`symtab::SymSource`], [`symtab::Sections`]) so that this crate depends
//! on the tables, not on `mortar_merge`'s or `mortar_image`'s types; the
//! address oracles ([`got::SlotValues`], [`symtab::SymAddrs`]) are what the
//! image phase answers.
//!
//! Diagnostics are phase 80 (`spec/model.md` §8): unit 0, `seq` the
//! `SymbolId`, or `SectionId × 2^32 + record` for one about an input
//! `.eh_frame` record. A plan or write that fails has emitted its errors
//! to the [`mortar_diag::Collector`] it was given and returns
//! [`error::Failed`].
#![forbid(unsafe_code)]

pub mod chunks;
pub mod eh;
pub mod error;
pub mod got;
pub mod hdr;
pub mod image;
pub mod notes;
pub mod out;
pub mod shstrtab;
pub mod symtab;

pub use chunks::{SynthKind, chunk_ids};
pub use eh::{EhPlan, EhSource, EhValue, Referrer};
pub use error::{Failed, PHASE, SynthError};
pub use got::{GotPlan, SlotValues};
pub use hdr::HdrPlan;
pub use image::{Division, FillCx, SizeCx, SynthChunk};
pub use notes::{BuildIdPlan, PropertyPlan};
pub use out::Window;
pub use shstrtab::ShstrtabPlan;
pub use symtab::{SymAddrs, SymSource, SymtabPlan};

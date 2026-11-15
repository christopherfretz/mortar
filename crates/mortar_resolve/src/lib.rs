//! Symbol resolution (`spec/resolve.md`): file liveness as reachability,
//! the definition of every key, COMDAT keepers, the fate of sections.
//!
//! Stage A (this crate's algorithms) reads an abstract [`View`] of the
//! inputs, so that the rules are tested on hand-built tables and on real
//! objects alike; [`resolve`] runs them:
//!
//! 1. [`live::targets`]: `first`/`cfirst` (§4.1), over every file.
//! 2. the closure (§4.1, §4.2): parallel breadth-first rounds;
//!    [`live::oracle`] is the sequential definition.
//! 3. the COMDAT keepers (§6.1) over the live files.
//! 4. one pass over the live objects: section rows (§10), demotion
//!    (§6.2), and the per-key cells `def` is a function of (§3).
//! 5. the rules over several mentions of one key (§3.3, §3.4, §3.8,
//!    §6.3), on the few keys the pass flagged.
//! 6. the linker's own symbols (§8), roots that must be defined, the
//!    entry (§5.5), and under `--warn-backrefs` the simulation of GNU ld
//!    (§4.6, [`gnu`]).
//!
//! Every result is a function of the inputs: per-key cells are atomic
//! minima and maxima over a total order (`spec/model.md` §6), lists are
//! sorted before they are read.
#![forbid(unsafe_code)]

pub mod comdat;
pub mod gnu;
pub mod linker;
pub mod live;
pub mod mention;
pub mod options;
pub mod problem;
pub mod resolve;
pub mod sections;
pub mod tables;
pub mod verdict;
pub mod view;

pub use options::Options;
pub use problem::{Problem, Severity, What};
pub use resolve::{Def, EntryPoint, Props, Resolved, resolve};
pub use tables::{InputsView, Outcome, resolve_inputs};
pub use view::View;

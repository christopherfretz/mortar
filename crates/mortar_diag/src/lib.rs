//! mortar's diagnostics: the model, the vocabulary of refusals, the
//! collector and the text renderer. The contract is `spec/diag.md`.
//!
//! Every crate of mortar reports through this one, and it depends on
//! nothing. The parts:
//!
//! - [`Diagnostic`]: a [`Severity`], a [`Kind`] with a stable id, an
//!   optional [`Place`] and [`Context`] lines. Callers say *what* and
//!   *where* in structure; no caller formats text.
//! - [`Diagnostic::refused`] with a [`RefusalClass`]: how mortar says "not
//!   implemented" by name (`docs/ARCHITECTURE.md`, principle 1).
//! - [`Collector`]: takes diagnostics from any thread with a [`Key`] and
//!   yields a [`Report`] whose order depends only on what was emitted. It
//!   applies the [`Policy`] (`--error-limit`, `--fatal-warnings`, `-w`)
//!   and answers "has the link failed?" with one atomic load.
//! - [`Style`]: the program name, color and the demangling hook, for
//!   [`Report::render`] and [`Diagnostic::render`].
//! - [`stream`]: the one writer to standard error and standard output; a
//!   failed write never panics (`spec/cli.md` §3.15).
//!
//! ```
//! use mortar_diag::{Collector, Diagnostic, Key, Policy, RefusalClass, Style};
//!
//! let diags = Collector::new(Policy::default());
//! diags.emit(
//!     Key::new(0, 0, 0),
//!     Diagnostic::refused(RefusalClass::Option, "--plugin").note("link-time optimization"),
//! );
//! assert!(diags.has_errors());
//! let report = diags.finish();
//! assert_eq!(
//!     report.render(&Style::plain("ld")),
//!     "ld: error: unsupported option: --plugin\n>>> link-time optimization\n",
//! );
//! ```

mod collect;
mod model;
mod render;
pub mod stream;

pub use collect::{Collector, DEFAULT_ERROR_LIMIT, Key, Policy, Report};
pub use model::{
    Context, Diagnostic, FileRef, IncompatibleReason, Kind, Place, RefusalClass, Relation, Severity,
};
pub use render::{Demangle, NOT_RECOGNIZED, Style, no_demangling, os_reason};

//! Input bytes for fill: only through the store's interface (I-35 step 1,
//! I-81), from every worker at once, so that the staged store (WP-021b)
//! drops in behind [`Inputs`].

use std::ops::Range;

use mortar_core::inputs::CopyError;
use mortar_core::{FileId, Inputs};
use mortar_diag::{Diagnostic, FileRef, Kind, Place};

/// The bytes of input files by file offset (`Inputs::resident`,
/// `Inputs::copy_to`).
pub trait Source: Sync {
    /// The bytes, when they lie in one resident part.
    fn resident(&self, file: FileId, range: Range<u64>) -> Option<&[u8]>;
    /// Copies the bytes into `out` (as long as the range).
    fn copy_to(&self, file: FileId, range: Range<u64>, out: &mut [u8]) -> Result<(), CopyError>;
}

impl Source for Inputs<'_> {
    fn resident(&self, file: FileId, range: Range<u64>) -> Option<&[u8]> {
        Inputs::resident(self, file, range)
    }

    fn copy_to(&self, file: FileId, range: Range<u64>, out: &mut [u8]) -> Result<(), CopyError> {
        Inputs::copy_to(self, file, range, out)
    }
}

/// Fills `out` with the bytes `range` of `file`: a copy from the resident
/// slice, or one `copy_to`.
pub(crate) fn read(
    src: &dyn Source,
    file: FileId,
    range: Range<u64>,
    out: &mut [u8],
) -> Result<(), CopyError> {
    match src.resident(file, range.clone()) {
        Some(bytes) if bytes.len() == out.len() => {
            out.copy_from_slice(bytes);
            Ok(())
        }
        _ => src.copy_to(file, range, out),
    }
}

/// The diagnostic of a failed copy: `input-changed` (`spec/input.md`
/// §11.5), `cannot-open-file` for a read error, `internal-error` for a
/// range the tables promised.
pub(crate) fn copy_error(err: CopyError, path: &FileRef) -> Diagnostic {
    match err {
        CopyError::Changed => Diagnostic::error(Kind::InputChanged).at(Place::file(path.path())),
        CopyError::Read(errno) => {
            let reason = mortar_diag::os_reason(&std::io::Error::from_raw_os_error(errno.0));
            Diagnostic::error(Kind::CannotOpenFile { path: path.path().to_owned(), reason })
        }
        CopyError::OutOfRange => Diagnostic::error(Kind::InternalError {
            what: format!("fill: a copy outside {} (model §4.4)", path.path()),
        }),
    }
}

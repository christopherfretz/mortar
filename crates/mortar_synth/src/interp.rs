//! `.interp` (`spec/dyn.md` D-31; `spec/synth.md` S-106, S-108): the
//! program interpreter's path and a NUL.
//!
//! **Plan** ([`InterpPlan::new`], phase 80): the `-dynamic-linker` path as
//! the command line gave it. There is no default interpreter (D-31: a
//! dynamic output without one is refused by `mortar_input`, input §8.4),
//! so the plan exists exactly when the command line names a path, in
//! `Exec` and `Pie` (never in a `StaticPie`, D-1). An input section named
//! `.interp` is refused by layout (L-39, request R-L2), not here.
//!
//! **Write**: the path and its NUL, `len(path) + 1` bytes (S-108); one
//! unit.

use std::ops::Range;

use crate::error::{SynthError, feature};
use crate::image::{Division, FillCx, SizeCx, SynthChunk};

/// The refused name of an empty `-dynamic-linker` path: an `.interp` of a
/// lone NUL names no file, and no reference's behaviour for it was
/// measured (D-31 is silent; WP-045 hand-back).
pub const EMPTY: &str = "an empty -dynamic-linker path";
/// The refused name of a path with a NUL byte: `.interp` could not hold
/// it (the loader reads up to the first NUL).
pub const NUL: &str = "a -dynamic-linker path containing a NUL byte";

/// `.interp`'s content: the path, then a NUL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterpPlan {
    /// The path and its terminating NUL.
    bytes: Box<[u8]>,
}

impl InterpPlan {
    /// The plan for the `-dynamic-linker` `path` (the last one the command
    /// line gave, cli §3.6), as written: no normalisation, no search.
    ///
    /// # Errors
    ///
    /// `refused` (`feature`): an empty path ([`EMPTY`]) or one containing a
    /// NUL byte ([`NUL`]).
    pub fn new(path: &[u8]) -> Result<Self, SynthError> {
        if path.is_empty() {
            return Err(feature(EMPTY).into());
        }
        if path.contains(&0) {
            return Err(feature(NUL).into());
        }
        let mut bytes = Vec::with_capacity(path.len() + 1);
        bytes.extend_from_slice(path);
        bytes.push(0);
        Ok(Self { bytes: bytes.into() })
    }

    /// The path, without its NUL.
    #[must_use]
    pub fn path(&self) -> &[u8] {
        &self.bytes[..self.bytes.len() - 1]
    }

    /// The section's bytes: the path and its NUL.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The size (S-108): `len(path) + 1`.
    #[must_use]
    pub fn size(&self) -> u64 {
        self.bytes.len() as u64
    }
}

impl SynthChunk for InterpPlan {
    fn size(&self, _: &SizeCx<'_>) -> u64 {
        InterpPlan::size(self)
    }

    fn division(&self) -> Division {
        Division::Whole { max_size: InterpPlan::size(self) }
    }

    fn write(&self, _: &FillCx<'_>, range: Range<u64>, out: &mut [u8]) -> Result<(), SynthError> {
        let size = InterpPlan::size(self);
        if range.start > range.end
            || range.end > size
            || out.len() as u64 != range.end - range.start
        {
            return Err(crate::error::internal(format!(
                "a synthetic chunk of {size} bytes written at {}..{} into {} bytes",
                range.start,
                range.end,
                out.len()
            )));
        }
        out.copy_from_slice(&self.bytes[range.start as usize..range.end as usize]);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mortar_diag::{Kind, RefusalClass};

    // dyn.md D-31, synth S-106/S-108: the path as written and one NUL.
    #[test]
    fn the_path_and_a_nul() {
        let p = InterpPlan::new(b"/lib/ld-linux-aarch64.so.1").unwrap();
        assert_eq!(p.bytes(), b"/lib/ld-linux-aarch64.so.1\0");
        assert_eq!(p.path(), b"/lib/ld-linux-aarch64.so.1");
        assert_eq!(InterpPlan::size(&p), 27);
        // Kept as written: no normalisation of `..` or `//`.
        let q = InterpPlan::new(b"lib//../ld.so").unwrap();
        assert_eq!(q.bytes(), b"lib//../ld.so\0");
    }

    // An empty path and a NUL inside are refused by name.
    #[test]
    fn refusals() {
        for (path, name) in [(&b""[..], EMPTY), (&b"/lib\0/ld.so"[..], NUL)] {
            let d = InterpPlan::new(path).unwrap_err().into_diagnostic();
            match d.kind() {
                Kind::Refused { class: RefusalClass::Feature, name: n, .. } => assert_eq!(n, name),
                k => panic!("{k:?}"),
            }
        }
    }
}

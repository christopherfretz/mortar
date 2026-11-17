//! I-R5's REL branch (`spec/image.md` I-R5, `spec/arch.md` A-R2): at `-r`
//! on a REL target (i386), a record against an `STT_SECTION` symbol gets
//! its new addend `A + delta` written into the field by `write_addend`.
//! Owner: **RL-9** (x86-64 and i386, REL), which owns this file and the
//! arch crates' `-r` parts and never edits `relocatable.rs`: RL-4 wrote
//! the call site there.
//!
//! WP-181 (RL-1) wrote this file with [`write`]'s signature and its
//! refusal; RL-9 fills [`write`] here and may add private items to this
//! file.

use mortar_arch::{Arch, Howto};
use mortar_diag::Diagnostic;

/// Writes `A + delta` into `field`, the output copy of a REL record's
/// field: `A` is `arch.read_addend(howto, field)` (the input's addend),
/// `delta` I-R5's piece offset of the target section, and the sum is
/// written by `arch.write_addend` with the type's `Implicit` description
/// (A-R2).
///
/// # Errors
///
/// `relocation-out-of-range` when `A + delta` does not fit the field
/// (A-R2), without a place: the caller adds `.at` the record. Until RL-9
/// lands: `unsupported feature: relocatable output (-r)`, with a note
/// naming REL.
pub fn write(
    arch: &dyn Arch,
    howto: &Howto,
    field: &mut [u8],
    delta: i64,
) -> Result<(), Diagnostic> {
    let _ = (arch, howto, field, delta);
    Err(mortar_diag::relocatable::refused_relocatable_output()
        .note("REL relocations at -r (i386) are RL-9's (spec/arch.md A-R2)"))
}

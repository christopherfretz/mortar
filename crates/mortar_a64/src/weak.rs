//! AArch64's column of `spec/dyn.md` D-25's class table (a64.md §7.1):
//! how a relocation uses an undefined weak key in `exec` or `pie`, which
//! decides per key whether it is `weakdyn` or `weak0`.
//!
//! What is written for a `weak0` key is a64.md §7 in every output kind;
//! this module only classifies the use.

use mortar_arch::WeakClass;

/// a64.md §7.1 "GOT": `GOTPCREL32`, `GOT_LD_PREL19`, `LD64_GOTOFF_LO15`,
/// `ADR_GOT_PAGE`, `LD64_GOT_LO12_NC`, `LD64_GOTPAGE_LO15` (R5 is off for a
/// `weakdyn` key: it relaxes only `local` targets, §8.5).
pub const GOT: [u32; 6] = [315, 309, 310, 311, 312, 313];
/// a64.md §7.1 "branch": `CALL26`, `JUMP26`, `PLT32`.
pub const BRANCH: [u32; 3] = [283, 282, 314];
/// a64.md §7.1 "absolute word in a writable place": `ABS64`.
pub const ABS64: u32 = 257;
/// a64.md §7.1 "no use of the key": `NONE`.
pub const NONE: u32 = 0;

/// The class of a relocation of `r_type` against an undefined weak key,
/// its place in a section that is `writable` after layout's rules (RELRO
/// included): `None` for `NONE` (no use of the key), and
/// [`WeakClass::Unfillable`] for every type §7.1 does not list in another
/// class: `ABS64` in a non-writable place, the narrow absolute and
/// PC-relative types, the `LO12` group, `CONDBR19`, `TSTBR14`, `GOTREL*`,
/// every TLS type, and any type §7.1 does not name at all (256, the
/// withdrawn `NONE`, included: D-25 "a type not listed is unfillable").
#[must_use]
pub const fn weak_class(r_type: u32, writable: bool) -> Option<WeakClass> {
    match r_type {
        NONE => None,
        ABS64 if writable => Some(WeakClass::AbsWord),
        315 | 309 | 310 | 311 | 312 | 313 => Some(WeakClass::Got),
        283 | 282 | 314 => Some(WeakClass::Branch),
        _ => Some(WeakClass::Unfillable),
    }
}

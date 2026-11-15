//! References from sections in the output into sections that are not
//! (§6.1, I-45..I-48, I-112).
//!
//! The classification and the values are `mortar_arch::dead`'s (R-5,
//! WP-023b); the image truncates the value to the field (I-48) and owns
//! the `.debug_aranges` length (I-112).

use mortar_arch::Arch;
pub use mortar_arch::dead::{DeadRef, dead_reference, is_debug_name};

/// The section whose tuples get a zero length after a tombstone (I-112).
pub const ARANGES: &[u8] = b".debug_aranges";

/// The value written in place of a field of `width` bytes holding a
/// reference to a dead target from section `name` (`alloc`: it has
/// `SHF_ALLOC`): the arch's tombstone truncated to the field (I-47, I-48)
/// for [`DeadRef::Tombstone`], 0 for [`DeadRef::Zero`]; `None` for
/// [`DeadRef::Discarded`] (the scan reported it, I-46) and
/// [`DeadRef::DeadFde`] (a dead FDE is not written).
#[must_use]
pub fn field(arch: &dyn Arch, name: &[u8], alloc: bool, width: u8) -> Option<u64> {
    let ones = if width >= 8 { u64::MAX } else { (1u64 << (8 * u32::from(width))) - 1 };
    match dead_reference(name, alloc) {
        DeadRef::Tombstone => Some(arch.tombstone(name) & ones),
        DeadRef::Zero => Some(0),
        DeadRef::Discarded | DeadRef::DeadFde => None,
    }
}

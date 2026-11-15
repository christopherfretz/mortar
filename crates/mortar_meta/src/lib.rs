//! The Mason metadata container (`spec/meta.md`): the reader and the writer
//! of the `.mason.meta` section's units, shared in format with kiln, hod
//! and tuck. Depends on `mortar_elf` only.
//!
//! - [`is_container`]: MT-6's predicate, the one place that decides what a
//!   container is (resolve's section table and the input store use it).
//! - [`read`]: the reader. It borrows the section's bytes, validates a unit
//!   whole when it opens it (MT-14), never panics, and decodes rows on
//!   access (no copy of the rows).
//! - [`write`]: the canonical writer (MT-38): the same rows in any order
//!   give the same bytes.
//! - [`anchor`]: the name of an anchored row (MT-12, `spec/odr.md` O-4).
//!
//! This is the format's single implementation: the test producer
//! (`cargo xtask meta`) is a front end to [`write`], the dumper to [`read`].
#![forbid(unsafe_code)]

pub mod anchor;
pub mod read;
pub mod write;

use mortar_elf::consts::{SHF_EXCLUDE, SHT_PROGBITS};

pub use read::{FileReader, Finding, Malformation, SectionAttrs, Unit, read_section};
pub use write::{UnitBuilder, WriteError};

/// The container's section name (MT-1).
pub const SECTION_NAME: &[u8] = b".mason.meta";
/// The container's `sh_addralign` (MT-1).
pub const SECTION_ALIGN: u64 = 8;
/// A unit's first four bytes (MT-10).
pub const MAGIC: [u8; 4] = *b"MASN";
/// The major version this reader reads and this writer writes (MT-35).
pub const MAJOR: u16 = 1;
/// The minor version this writer writes and this reader knows (MT-36).
pub const MINOR: u16 = 0;
/// The unit header's size in 1.0 (MT-10).
pub const HEADER_SIZE: u32 = 32;
/// A block header's size (MT-11).
pub const BLOCK_HEADER_SIZE: u32 = 16;

/// The `producer` field (MT-10).
pub mod producer {
    pub const KILN: u16 = 1;
    pub const HOD: u16 = 2;
    pub const MORTAR: u16 = 3;
    /// The test producer of `meta.md` §12 (`cargo xtask meta write`).
    pub const TEST: u16 = 4;
    pub const OTHER: u16 = 0xffff;
}

/// Record kinds (`meta.md` §7) and their v1 row sizes.
pub mod kind {
    /// TU summary (MT-26).
    pub const SUMMARY: u16 = 0x0001;
    /// ODR definitions (`spec/odr.md` O-15).
    pub const DEFINITIONS: u16 = 0x0002;
    /// Type definitions (`spec/odr.md` O-17).
    pub const TYPES: u16 = 0x0003;
    /// Layout details (`spec/odr.md` O-19): not rows; its encoding is
    /// WP-073's.
    pub const DETAILS: u16 = 0x0004;
    /// Declarations (`spec/odr.md` O-35).
    pub const DECLARATIONS: u16 = 0x0005;
    /// The experimental range, skipped by every release reader (MT-25).
    pub const EXPERIMENTAL: core::ops::RangeInclusive<u16> = 0x7f00..=0x7fff;

    pub const SUMMARY_ROW: u32 = 16;
    pub const DEFINITION_ROW: u32 = 48;
    pub const TYPE_ROW: u32 = 56;
    pub const DECLARATION_ROW: u32 = 40;

    /// The v1 row size of a kind this reader knows; `Some(0)` for kind 4
    /// (not rows), `None` for a kind it does not know.
    #[must_use]
    pub const fn row_size(kind: u16) -> Option<u32> {
        match kind {
            SUMMARY => Some(SUMMARY_ROW),
            DEFINITIONS => Some(DEFINITION_ROW),
            TYPES => Some(TYPE_ROW),
            DETAILS => Some(0),
            DECLARATIONS => Some(DECLARATION_ROW),
            _ => None,
        }
    }
}

/// Flag bits of the rows (`spec/odr.md` O-15, O-17, O-35; MT-26).
pub mod flags {
    /// Kind 1: the key is an ABI key (O-30).
    pub const ENFORCE: u32 = 1 << 0;

    /// Kind 2.
    pub const VAGUE: u16 = 1 << 0;
    pub const IMPLICIT_INST: u16 = 1 << 1;
    pub const EXPLICIT_INST: u16 = 1 << 2;
    pub const EXPLICIT_SPEC: u16 = 1 << 3;
    pub const USES_INTERNAL: u16 = 1 << 4;
    pub const PARTIAL: u16 = 1 << 5;
    pub const INLINED_ONLY: u16 = 1 << 6;
    pub const GMF: u16 = 1 << 7;
    pub const SYSTEM: u16 = 1 << 8;

    /// Kind 3.
    pub const TYPE_ENUM: u16 = 1 << 0;
    pub const TYPE_UNION: u16 = 1 << 1;
    pub const TYPE_POLYMORPHIC: u16 = 1 << 2;
    pub const TYPE_HAS_VBASES: u16 = 1 << 3;
    pub const TYPE_PARTIAL: u16 = 1 << 4;
    pub const TYPE_SYSTEM: u16 = 1 << 5;

    /// Kind 5.
    pub const DECL_DEFINITION: u16 = 1 << 0;
    pub const DECL_VARIABLE: u16 = 1 << 1;
    pub const DECL_EXTERN_C: u16 = 1 << 2;
    pub const DECL_NOEXCEPT: u16 = 1 << 3;
}

/// Kind 2's `class` (`spec/odr.md` O-15).
pub mod class {
    pub const FUNCTION: u16 = 1;
    pub const VARIABLE: u16 = 2;
}

/// Kind 3's `details` when the type has no kind 4 entry (O-17).
pub const NO_DETAILS: u32 = 0xffff_ffff;

/// MT-6: whether a section is a container. The cheap tests (type, flags)
/// come first, so a link without containers pays one integer compare per
/// section and a name compare only for `PROGBITS` sections whose flags are
/// exactly `SHF_EXCLUDE` (rare: GCC's `.gnu.lto_*` are the one common
/// case). The other attributes of MT-1 are the reader's ([`read`]).
#[inline]
#[must_use]
pub fn is_container(name: &[u8], sh_type: u32, sh_flags: u64) -> bool {
    sh_flags == SHF_EXCLUDE && sh_type == SHT_PROGBITS && name == SECTION_NAME
}

/// MT-13: FNV-1a 64 over `bytes` (a name without its NUL).
#[must_use]
pub const fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut i = 0;
    while i < bytes.len() {
        h ^= bytes[i] as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: meta.md MT-13 (the published FNV-1a 64 test vectors)
    #[test]
    fn fnv1a64_vectors() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    // spec: meta.md MT-6
    #[test]
    fn predicate() {
        assert!(is_container(b".mason.meta", SHT_PROGBITS, SHF_EXCLUDE));
        assert!(!is_container(b".mason.meta", SHT_PROGBITS, SHF_EXCLUDE | 2));
        assert!(!is_container(b".mason.meta", SHT_PROGBITS, 0));
        assert!(!is_container(b".mason.meta", 7, SHF_EXCLUDE));
        assert!(!is_container(b".mason.metax", SHT_PROGBITS, SHF_EXCLUDE));
        assert!(!is_container(b".gnu.lto_main", SHT_PROGBITS, SHF_EXCLUDE));
    }
}

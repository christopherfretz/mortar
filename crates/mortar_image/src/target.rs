//! What fill needs of the target beyond `mortar_arch::Arch` on main: the
//! derived `max_reach` (I-111), the fill patterns (I-41, `spec/layout.md`
//! L-82), the class (I-47) and the address-sized absolute type (I-112).
//!
//! Stand-in: `Facts.max_reach` and `Facts.code_fill` are request R-2 of
//! `spec/image.md` (WP-023b). Until they land the values are stated here,
//! per machine, from the same spec text; the test `i111_max_reach_bounds_
//! every_write` checks the AArch64 value against `apply`.

use mortar_elf::{Class, Machine};

use crate::plan::Pattern;

/// The facts of one target that fill reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub machine: Machine,
    pub class: Class,
    /// `(before, after)`: how far any write window extends before a
    /// field's first byte and past its last (I-111).
    pub max_reach: (u64, u64),
    /// The trap pattern for code padding (`Facts.code_fill`, I-41).
    pub trap: Pattern,
    /// The no-op between inputs of `.init` and `.fini` (L-82).
    pub nop: Pattern,
    /// The class's address-sized absolute type (I-112): `R_AARCH64_ABS64`,
    /// `R_X86_64_64`, `R_386_32`.
    pub abs_addr: u32,
}

impl Target {
    /// The facts of `machine` (I-111's values; L-82's patterns).
    #[must_use]
    pub const fn of(machine: Machine) -> Self {
        match machine {
            Machine::AArch64 => Self {
                machine,
                class: Class::Elf64,
                max_reach: (0, 4),
                trap: Pattern::ZERO4,
                // `nop` = 0xd503201f, little-endian.
                nop: Pattern::word([0x1f, 0x20, 0x03, 0xd5]),
                abs_addr: 257,
            },
            Machine::X86_64 => Self {
                machine,
                class: Class::Elf64,
                max_reach: (4, 8),
                trap: Pattern::byte(0xcc),
                nop: Pattern::byte(0x90),
                abs_addr: 1,
            },
            Machine::I386 => Self {
                machine,
                class: Class::Elf32,
                max_reach: (4, 8),
                trap: Pattern::byte(0xcc),
                nop: Pattern::byte(0x90),
                abs_addr: 1,
            },
        }
    }

    /// Whether the class is `ELFCLASS64`.
    #[must_use]
    pub const fn is_64(&self) -> bool {
        matches!(self.class, Class::Elf64)
    }
}

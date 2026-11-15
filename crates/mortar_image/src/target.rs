//! What the plan needs of the target: the arch's derived `max_reach`
//! (I-111) and fill patterns (I-41, `spec/layout.md` L-82) from
//! `mortar_arch::Facts`, the class (I-47) and the address-sized absolute
//! type (I-112), which `Facts` does not carry.

use mortar_arch::Facts;
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
    /// The facts of the arch (`Facts.max_reach`, derived by the arch,
    /// I-111; `code_fill` and `code_nop`, L-82), with the address-sized
    /// absolute type of its machine (I-112).
    ///
    /// # Panics
    /// If a pattern is not 1 or 4 bytes long (no arch has one).
    #[must_use]
    pub fn from_facts(f: &Facts) -> Self {
        let pattern = |b: &[u8]| match *b {
            [x] => Pattern::byte(x),
            [a, b, c, d] => Pattern::word([a, b, c, d]),
            _ => panic!("fill pattern of {} bytes", b.len()),
        };
        Self {
            machine: f.machine,
            class: f.class,
            max_reach: (u64::from(f.max_reach.before), u64::from(f.max_reach.after)),
            trap: pattern(f.code_fill),
            nop: pattern(f.code_nop),
            abs_addr: match f.machine {
                Machine::AArch64 => 257,
                Machine::X86_64 | Machine::I386 => 1,
            },
        }
    }

    /// Whether the class is `ELFCLASS64`.
    #[must_use]
    pub const fn is_64(&self) -> bool {
        matches!(self.class, Class::Elf64)
    }
}

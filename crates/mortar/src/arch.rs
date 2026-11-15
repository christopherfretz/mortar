//! The architecture of a link (WP-029b): chosen from the inputs'
//! `e_machine` (input's target, `spec/input.md` §4.4, which already
//! refuses a file of another machine), and handed to every phase as the
//! `&dyn Arch` they take.

use mortar_a64::A64;
use mortar_arch::Arch;
use mortar_elf::Machine;
use mortar_scan::{CodeProof, NoProof};
use mortar_x64::X64;

use crate::proof::A64Proof;

/// The architectures mortar links.
#[derive(Debug, Clone, Copy)]
pub enum LinkArch {
    A64(A64),
    X64(X64),
}

impl LinkArch {
    /// The architecture of `machine` (`fix_843419`: AArch64's erratum
    /// option), or `None` for a machine mortar does not link yet.
    #[must_use]
    pub fn of(machine: Machine, fix_843419: bool) -> Option<Self> {
        match machine {
            Machine::AArch64 => Some(Self::A64(A64::new(mortar_arch::Status::M1, fix_843419))),
            Machine::X86_64 => Some(Self::X64(X64::M1)),
            Machine::I386 => None,
        }
    }

    /// The architecture as every phase takes it.
    #[must_use]
    pub fn get(&self) -> &dyn Arch {
        match self {
            Self::A64(a) => a,
            Self::X64(x) => x,
        }
    }

    /// The instruction proof of the scan (AArch64's mapping symbols;
    /// x86-64 has none and none of its rules reads one).
    #[must_use]
    pub fn proof(&self) -> &'static dyn CodeProof {
        match self {
            Self::A64(_) => &A64Proof,
            Self::X64(_) => &NoProof,
        }
    }

    /// The `FEATURE_1_AND` property type of the machine and the bit that
    /// marks its branch-target PLT (AArch64 BTI, x86-64 IBT).
    #[must_use]
    pub const fn feature_1(&self) -> (u32, u32) {
        match self {
            Self::A64(_) => (mortar_a64::FEATURE_1_AND, mortar_a64::FEATURE_1_BTI),
            Self::X64(_) => (mortar_x64::FEATURE_1_AND, mortar_x64::FEATURE_1_IBT),
        }
    }
}

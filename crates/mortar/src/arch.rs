//! The architecture of a link (WP-029b): chosen from the inputs'
//! `e_machine` (input's target, `spec/input.md` §4.4, which already
//! refuses a file of another machine), and handed to every phase as the
//! `&dyn Arch` they take.

use mortar_a64::A64;
use mortar_arch::Arch;
use mortar_elf::Machine;
use mortar_scan::{CodeProof, NoProof};
use mortar_x64::X64;
use mortar_x86::X86;

use crate::proof::A64Proof;

/// The architectures mortar links.
#[derive(Debug, Clone, Copy)]
pub enum LinkArch {
    A64(A64),
    X64(X64),
    I386(X86),
}

impl LinkArch {
    /// The architecture of `machine` (`fix_843419`: AArch64's erratum
    /// option), or `None` for a machine mortar does not link yet.
    /// `status` is `crate::dynamic::arch_status`'s: `M2` only behind the
    /// dynamic guard (WP-041 step 0e). x86-64 and i386 have M1 tables
    /// only until WP-048 (and M4 for i386).
    #[must_use]
    pub fn of(machine: Machine, fix_843419: bool, status: mortar_arch::Status) -> Option<Self> {
        match machine {
            Machine::AArch64 => Some(Self::A64(A64::new(status, fix_843419))),
            Machine::X86_64 => Some(Self::X64(X64::M1)),
            Machine::I386 => Some(Self::I386(X86::M1)),
        }
    }

    /// The architecture as every phase takes it.
    #[must_use]
    pub fn get(&self) -> &dyn Arch {
        match self {
            Self::A64(a) => a,
            Self::X64(x) => x,
            Self::I386(x) => x,
        }
    }

    /// The instruction proof of the scan (AArch64's mapping symbols;
    /// x86-64 and i386 have none and none of their rules reads one).
    #[must_use]
    pub fn proof(&self) -> &'static dyn CodeProof {
        match self {
            Self::A64(_) => &A64Proof,
            Self::X64(_) | Self::I386(_) => &NoProof,
        }
    }

    /// The `FEATURE_1_AND` property type of the machine and the bit that
    /// marks its branch-target PLT (AArch64 BTI, x86-64 and i386 IBT).
    #[must_use]
    pub const fn feature_1(&self) -> (u32, u32) {
        match self {
            Self::A64(_) => (mortar_a64::FEATURE_1_AND, mortar_a64::FEATURE_1_BTI),
            Self::X64(_) => (mortar_x64::FEATURE_1_AND, mortar_x64::FEATURE_1_IBT),
            Self::I386(_) => (mortar_x86::FEATURE_1_AND, mortar_x86::FEATURE_1_IBT),
        }
    }
}

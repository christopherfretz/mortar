//! Which architectures write a dynamic output (WP-041 step 0e,
//! `spec/landed/dyn/WP-040.md` §2; lifted by WP-053): the only place that
//! asks for an architecture of [`Status::M2`]. The command-line gates of a
//! dynamic output (`spec/dyn.md` §2.7: `-dynamic-linker`, `-pie`,
//! `-Bdynamic`) are `supported` rows of the option table since WP-053, so
//! the guard that refused them is gone; an option is refused by its row
//! alone (`mortar_cli::Parsed::refusals`).
//!
//! What this module does not own: the input walk's refusal of a shared
//! object in a link that names no interpreter (`spec/input.md` §8.4)
//! stays, because `spec/dyn.md` D-31 keeps it at M2; `-shared` (a shared
//! object as output) is M4 and stays a refused row of the table; a static
//! pie is refused where the layout reads the kind (WP-052).

use mortar_arch::Status;
use mortar_cli::Plan;
use mortar_core::plan::OutputKind;
use mortar_diag::{Diagnostic, RefusalClass};
use mortar_elf::Machine;

/// The architecture status of a link of `plan` on `machine`:
/// [`Status::M2`] for a dynamic output (`Exec`, `Pie`) on an architecture
/// whose join has landed (AArch64, WP-053), `M1` for every other output,
/// so a static link's path is unchanged. A dynamic output on another
/// architecture is refused by name: x86-64 until its join (WP-055), i386
/// for all of M2 (ELFCLASS32 dynamic output is M4).
///
/// # Errors
///
/// The refusal of a dynamic output on `machine`.
#[allow(clippy::result_large_err, reason = "the front's diagnostic, made once")]
pub fn arch_status(plan: &Plan, machine: Machine) -> Result<Status, Diagnostic> {
    let dynamic = matches!(plan.kind, OutputKind::Executable | OutputKind::PieExecutable);
    if !dynamic {
        return Ok(Status::M1);
    }
    match machine {
        Machine::AArch64 => Ok(Status::M2),
        Machine::X86_64 => {
            Err(Diagnostic::refused(RefusalClass::Feature, "dynamically linked output on x86-64")
                .note("x86-64 dynamic executables arrive with WP-055 (docs/status/m2.md)"))
        }
        Machine::I386 => {
            Err(Diagnostic::refused(RefusalClass::Feature, "dynamically linked output on i386")
                .note("ELFCLASS32 dynamic output is M4 (docs/status/m2.md)"))
        }
    }
}

pub use crate::dynlink::DynTables;

#[cfg(test)]
mod tests {
    use super::arch_status;
    use mortar_arch::Status;
    use mortar_cli::Plan;
    use mortar_core::plan::OutputKind;
    use mortar_elf::Machine;

    // spec: WP-040 step 0e (Status::M2 only behind the guard), WP-053
    // (AArch64 only; x86-64 and i386 dynamic output refused by name)
    #[test]
    fn m2_only_behind_the_guard() {
        for kind in [
            OutputKind::Executable,
            OutputKind::PieExecutable,
            OutputKind::StaticExecutable,
            OutputKind::StaticPie,
        ] {
            let plan = Plan { kind, ..Plan::default() };
            let dynamic = matches!(kind, OutputKind::Executable | OutputKind::PieExecutable);
            let m2 = dynamic;
            let a64 = arch_status(&plan, Machine::AArch64).ok();
            assert_eq!(a64 == Some(Status::M2), m2, "{kind:?}");
            for other in [Machine::X86_64, Machine::I386] {
                let s = arch_status(&plan, other);
                if m2 {
                    assert!(s.is_err(), "{kind:?} on {other:?} is refused by name");
                } else {
                    assert_eq!(s.ok(), Some(Status::M1), "{kind:?} on {other:?}");
                }
            }
        }
    }
}

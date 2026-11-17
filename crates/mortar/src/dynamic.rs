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
//! object as output, M4) is a dynamic output on AArch64 since WP-123 and
//! on x86-64 since WP-127, and refused by name on i386 (WP-130); a static
//! pie on i386 is refused where the layout reads the kind
//! (`mortar_layout::target`; written on AArch64 and x86-64 since WP-052).

use mortar_arch::Status;
use mortar_cli::Plan;
use mortar_core::plan::OutputKind;
use mortar_diag::{Diagnostic, RefusalClass};
use mortar_elf::Machine;

/// The architecture status of a link of `plan` on `machine`:
/// [`Status::M2`] for a dynamic output (`Exec`, `Pie`) on an architecture
/// whose join has landed (AArch64, WP-053; x86-64, WP-055), `M1` for
/// every other output, so a static link's path is unchanged. A static-pie
/// is `M1` too (`spec/dyn.md` D-2, WP-052): nothing in it is shared, so
/// its relocations take the static tables' outcomes in the `Pie` column,
/// and only its `.dynamic` machinery is the dynamic link's. A dynamic
/// output on i386 is refused by name for all of M2 (ELFCLASS32 dynamic
/// output is M4).
///
/// # Errors
///
/// The refusal of a dynamic output on `machine`.
#[allow(clippy::result_large_err, reason = "the front's diagnostic, made once")]
pub fn arch_status(plan: &Plan, machine: Machine) -> Result<Status, Diagnostic> {
    // `spec/dyn.md` D-100: a shared object is a dynamic output, joined on
    // AArch64 (WP-123) and x86-64 (WP-127; x64.md X-59 to X-62); i386 is
    // WP-130's (record `spec/landed/dyn/WP-110.md` §G.4).
    if plan.kind == OutputKind::Shared {
        return match machine {
            Machine::AArch64 | Machine::X86_64 => Ok(Status::M2),
            Machine::I386 => Err(Diagnostic::refused(
                RefusalClass::Feature,
                "ELFCLASS32 shared object",
            )
            .note("dyn.md D-130: i386 shared objects are WP-130's (docs/status/options.md, -shared)")),
        };
    }
    let dynamic = matches!(plan.kind, OutputKind::Executable | OutputKind::PieExecutable);
    if !dynamic {
        return Ok(Status::M1);
    }
    match machine {
        Machine::AArch64 | Machine::X86_64 => Ok(Status::M2),
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
    // (AArch64), WP-055 (x86-64); i386 dynamic output refused by name
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
            for joined in [Machine::AArch64, Machine::X86_64] {
                let s = arch_status(&plan, joined).ok();
                assert_eq!(s == Some(Status::M2), m2, "{kind:?} on {joined:?}");
                assert!(s.is_some(), "{kind:?} on {joined:?}");
            }
            let s = arch_status(&plan, Machine::I386);
            if m2 {
                assert!(s.is_err(), "{kind:?} on i386 is refused by name");
            } else {
                assert_eq!(s.ok(), Some(Status::M1), "{kind:?} on i386");
            }
        }
    }

    // spec: dyn.md D-100 (WP-123, WP-127): a shared object is M2 on
    // AArch64 and x86-64, and refused by name, class `feature`, on i386
    // (WP-130)
    #[test]
    fn d100_shared_on_aarch64_and_x86_64() {
        let plan = Plan { kind: OutputKind::Shared, ..Plan::default() };
        assert_eq!(arch_status(&plan, Machine::AArch64).ok(), Some(Status::M2));
        assert_eq!(arch_status(&plan, Machine::X86_64).ok(), Some(Status::M2));
        let d = arch_status(&plan, Machine::I386).err();
        assert_eq!(
            d.as_ref().and_then(|d| d.refusal()).map(|r| r.1),
            Some("ELFCLASS32 shared object")
        );
    }
}

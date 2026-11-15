//! The dynamic guard (WP-041, M2 step 0e; `spec/landed/dyn/WP-040.md`
//! §2): the ONE place that decides whether this build of mortar writes a
//! dynamic output. While M2 is being built it refuses the command-line
//! gates of a dynamic output (`spec/dyn.md` §2.7, the census's ranked
//! refusals: `-dynamic-linker`, `-pie`, `-Bdynamic`) with exactly the
//! text the command line gave before (`unsupported option: <spelling>`,
//! keyed on the argument), and it is the only place that asks for an
//! architecture of [`Status::M2`]. The M2 join flips [`DYNAMIC_OUTPUT`]
//! and the packages lift their gates here, not in the option table.
//!
//! What the guard does not own: the input walk's refusal of a shared
//! object in a link that names no interpreter (`spec/input.md` §8.4) stays,
//! because `spec/dyn.md` D-31 keeps it at M2; `-shared` (a shared object
//! as output) is M4 and stays a refused row of the table.

use mortar_arch::Status;
use mortar_cli::table::{KindSet, Lands, StateOp};
use mortar_cli::{Parsed, Plan, Use};
use mortar_core::plan::OutputKind;
use mortar_diag::{Diagnostic, Key, RefusalClass};
use mortar_elf::Machine;

/// Whether mortar writes `Exec` and `Pie` outputs: `true` since the M2
/// join (WP-053), on the architectures [`arch_status`] admits; the
/// command line's gates pass, and the architecture is judged once the
/// inputs name it.
pub const DYNAMIC_OUTPUT: bool = true;

/// Whether a refused use of the command line is a dynamic gate, which the
/// guard reports instead of the command line: `-pie` (`--pic-executable`),
/// `-dynamic-linker` (`-I`, `--dynamic-linker`), `-Bdynamic` (`-dy`,
/// `-call_shared`).
#[must_use]
pub fn is_gate(u: &Use) -> bool {
    matches!(
        u.row().lands,
        Lands::Kind(KindSet::Pie) | Lands::DynamicLinker | Lands::State(StateOp::Static(false))
    )
}

/// What the guard decided.
#[derive(Debug)]
pub struct Guard {
    /// The refusals of the gates, as the command line keyed and worded
    /// them (`mortar_cli::Use::refusal`); any of them stops the line with
    /// the refusal of linking, as every refused option does.
    pub refusals: Vec<(Key, Diagnostic)>,
}

/// `spec/landed/dyn/WP-040.md` step 0e: the guard over the plan and the
/// gate uses among the refused ones (`Parsed::refused_uses`, filtered by
/// [`is_gate`], so each gate is reported once, at its first use, as
/// before). While [`DYNAMIC_OUTPUT`] is `false` every gate is refused.
#[must_use]
pub fn dynamic_guard(plan: &Plan, gates: &[&Use]) -> Guard {
    let _ = plan;
    let refusals =
        if DYNAMIC_OUTPUT { Vec::new() } else { gates.iter().map(|u| u.refusal()).collect() };
    Guard { refusals }
}

/// Every refused option of `parsed`: the command line's refusals, except
/// that [`dynamic_guard`] reports the dynamic gates among them. Each is
/// keyed on its argument, so the report is the one the command line alone
/// gave; a non-empty list stops the line (`crate::invoke_in`).
#[must_use]
pub fn option_refusals(plan: &Plan, parsed: &Parsed) -> Vec<(Key, Diagnostic)> {
    let (gates, others): (Vec<&Use>, Vec<&Use>) =
        parsed.refused_uses().into_iter().partition(|u| is_gate(u));
    let mut refusals: Vec<(Key, Diagnostic)> = others.into_iter().map(Use::refusal).collect();
    refusals.extend(dynamic_guard(plan, &gates).refusals);
    refusals
}

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
pub fn arch_status(plan: &Plan, machine: Machine) -> Result<Status, Diagnostic> {
    let dynamic = matches!(plan.kind, OutputKind::Executable | OutputKind::PieExecutable);
    if !(DYNAMIC_OUTPUT && dynamic) {
        return Ok(Status::M1);
    }
    match machine {
        Machine::AArch64 => Ok(Status::M2),
        Machine::X86_64 => Err(Diagnostic::refused(
            RefusalClass::Feature,
            "dynamically linked output on x86-64",
        )
        .note("x86-64 dynamic executables arrive with WP-055 (docs/status/m2.md)")),
        Machine::I386 => Err(Diagnostic::refused(
            RefusalClass::Feature,
            "dynamically linked output on i386",
        )
        .note("ELFCLASS32 dynamic output is M4 (docs/status/m2.md)")),
    }
}

pub use crate::dynlink::DynTables;

#[cfg(test)]
mod tests {
    use super::{DYNAMIC_OUTPUT, arch_status, is_gate};
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
            let m2 = DYNAMIC_OUTPUT && dynamic;
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

    // The gates are the census's options and nothing else.
    #[test]
    fn the_gates() {
        let args =
            ["-pie", "-no-pie", "-dynamic-linker", "/l", "-Bdynamic", "-Bstatic", "-shared", "x.o"];
        let args: Vec<std::ffi::OsString> = args.iter().map(std::ffi::OsString::from).collect();
        let parsed = mortar_cli::parse(&args);
        let gates: Vec<&str> =
            parsed.uses.iter().filter(|u| is_gate(u)).map(|u| u.spelling.as_str()).collect();
        assert_eq!(gates, ["-pie", "-dynamic-linker", "-Bdynamic"]);
    }
}

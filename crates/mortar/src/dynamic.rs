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
use mortar_diag::{Diagnostic, Key};
use mortar_synth::DynsymPlan;

/// Whether mortar writes `Exec` and `Pie` outputs. `false` until the M2
/// join: every gate below is refused.
pub const DYNAMIC_OUTPUT: bool = false;

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

/// The architecture status of a link of `plan`: [`Status::M2`] only for
/// a dynamic output, and only once the guard lets one through; `M1`
/// otherwise, so a static link's path is unchanged.
#[must_use]
pub fn arch_status(plan: &Plan) -> Status {
    let dynamic = matches!(plan.kind, OutputKind::Executable | OutputKind::PieExecutable);
    if DYNAMIC_OUTPUT && dynamic { Status::M2 } else { Status::M1 }
}

/// The plans of a dynamic output's tables, which a static link does not
/// have (`Front.dyn_tables` is `None`). Step 0 holds `.dynsym`'s plan;
/// the M2 packages add theirs (`spec/landed/dyn/WP-040.md` §3).
#[derive(Debug, Clone, Default)]
pub struct DynTables {
    /// D-21, D-22, D-26 (`mortar_synth::dynsym_plan`).
    pub dynsym: DynsymPlan,
}

#[cfg(test)]
mod tests {
    use super::{DYNAMIC_OUTPUT, arch_status, is_gate};
    use mortar_arch::Status;
    use mortar_cli::Plan;
    use mortar_core::plan::OutputKind;

    // spec: WP-040 step 0e (Status::M2 only behind the guard)
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
            assert_eq!(arch_status(&plan) == Status::M2, m2, "{kind:?}");
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

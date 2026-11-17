//! `--odr-runtime=report|fatal` (`spec/cli.md` §3.9, `spec/odr.md` O-53a;
//! WP-198): the load-time one-definition-rule check linked into the
//! output. The runtime object of the output's machine and kind is an
//! ordinary input after every input of the line, named [`NAME`]; the line
//! is the one of `--odr-table=exported -z now` (unless `-z lazy` or a
//! table is named), and `.note.mason` carries the runtime's note
//! (`spec/odr.md` §10.2, type `NT_MASON_ODR_RT`).
//!
//! A link without the option reads one field of the plan
//! ([`OdrRuntime::Off`]) and nothing else of this module: the objects'
//! bytes are in the binary's read-only data, never touched by it.

use mortar_core::plan::{OdrRuntime, OdrTable, OutputKind, Plan};
use mortar_diag::{Diagnostic, Key, RefusalClass};
use mortar_elf::Machine;
use mortar_input::walk::Embedded;

/// The name the runtime input has in -Map, the diagnostics and the tables
/// (it is no file).
pub const NAME: &str = "<mortar:odr-runtime>";

/// One embedded runtime object: its machine, its flavour (`so` for a
/// shared object, `exe` for an executable), its bytes, and the SHA-256 of
/// the bytes (checked by a test: the objects are built by
/// `cargo xtask odr-runtime`, committed under `runtime/odr/obj/`). The
/// runtime id of the note is the index of the row in [`OBJECTS`].
#[derive(Debug)]
pub struct Object {
    pub machine: Machine,
    pub shared: bool,
    pub bytes: &'static [u8],
    pub sha256: &'static str,
}

const A64_SO: &[u8] = include_bytes!("../../../runtime/odr/obj/mason_odr_rt-aarch64-so.o");
const A64_EXE: &[u8] = include_bytes!("../../../runtime/odr/obj/mason_odr_rt-aarch64-exe.o");
const X64_SO: &[u8] = include_bytes!("../../../runtime/odr/obj/mason_odr_rt-x86_64-so.o");
const X64_EXE: &[u8] = include_bytes!("../../../runtime/odr/obj/mason_odr_rt-x86_64-exe.o");
const I386_SO: &[u8] = include_bytes!("../../../runtime/odr/obj/mason_odr_rt-i386-so.o");
const I386_EXE: &[u8] = include_bytes!("../../../runtime/odr/obj/mason_odr_rt-i386-exe.o");

/// The runtime objects, in runtime-id order. The i386 rows are embedded
/// (D-130) but no link uses them: i386 is refused by name
/// (`odr-runtime-unsupported-target`) until the runtime is validated
/// there.
pub static OBJECTS: [Object; 6] = [
    Object {
        machine: Machine::AArch64,
        shared: true,
        bytes: A64_SO,
        sha256: "e8c4652ca7549503d8ea6eaefa7d905287288e6d964e56aef509cc810581573a",
    },
    Object {
        machine: Machine::AArch64,
        shared: false,
        bytes: A64_EXE,
        sha256: "673be2a6231debbca4d3d27b659af896cd944d635e624293cc4c0165ef8fd117",
    },
    Object {
        machine: Machine::X86_64,
        shared: true,
        bytes: X64_SO,
        sha256: "161782f05b2bedb6bf8b006ccd0c5c54499011f9a053a63449da029b559c2bca",
    },
    Object {
        machine: Machine::X86_64,
        shared: false,
        bytes: X64_EXE,
        sha256: "df63a18fdb61f3c9d2e1d3ff82b230b2c51a12e136b1bed81d5839f4e60ff8e8",
    },
    Object {
        machine: Machine::I386,
        shared: true,
        bytes: I386_SO,
        sha256: "5b8e7a983e5b3f429b1b9cc4b8ace16fbb71eb0846a8c4a42bf48f2d63773497",
    },
    Object {
        machine: Machine::I386,
        shared: false,
        bytes: I386_EXE,
        sha256: "6b9364537ced20548d6a1c0e538c59511b6e4c199a052a5588f43e6213d9ebe5",
    },
];

/// The machines the runtime is linked for.
const SUPPORTED: [Machine; 2] = [Machine::AArch64, Machine::X86_64];

/// The objects a link may add, per flavour: the supported machines only.
static SHARED: [(Machine, &[u8]); 2] = [(Machine::AArch64, A64_SO), (Machine::X86_64, X64_SO)];
static EXEC: [(Machine, &[u8]); 2] = [(Machine::AArch64, A64_EXE), (Machine::X86_64, X64_EXE)];

/// The plan a link with the runtime runs (`spec/cli.md` §3.9): the table
/// is implied (`--odr-table=exported`, an explicit `off` is refused on the
/// command line), and the output binds now unless `-z lazy` is given.
/// `None` without the option: the plan as it is.
#[must_use]
pub fn effective(plan: &Plan) -> Option<Plan> {
    if plan.diagnostics.odr.runtime == OdrRuntime::Off || plan.kind == OutputKind::Relocatable {
        return None;
    }
    let mut p = plan.clone();
    if p.diagnostics.odr.table == OdrTable::Off {
        p.diagnostics.odr.table = OdrTable::Exported;
    }
    if p.z.now.is_none() {
        p.z.now = Some(true);
    }
    Some(p)
}

/// The runtime input of the link, after every input of the line: its
/// argument index is one past the last input's.
#[must_use]
pub fn embedded(plan: &Plan) -> Option<Embedded> {
    if plan.diagnostics.odr.runtime == OdrRuntime::Off {
        return None;
    }
    let objects: &'static [(Machine, &'static [u8])] = match plan.kind {
        OutputKind::Shared => &SHARED,
        OutputKind::Executable | OutputKind::PieExecutable => &EXEC,
        // Refused (static) or a no-op (`-r`) on the command line.
        OutputKind::StaticExecutable | OutputKind::StaticPie | OutputKind::Relocatable => {
            return None;
        }
    };
    let arg = plan.inputs.iter().map(|i| i.arg).max().map_or(0, |a| a.saturating_add(1));
    Some(Embedded { name: NAME.as_bytes(), arg, objects })
}

/// The refusal of a link for a machine the runtime is not linked for
/// (`odr-runtime-unsupported-target`), keyed at the input phase.
#[must_use]
pub fn refuse_target(plan: &Plan, machine: Machine) -> Option<(Key, Diagnostic)> {
    if plan.diagnostics.odr.runtime == OdrRuntime::Off
        || plan.kind == OutputKind::Relocatable
        || SUPPORTED.contains(&machine)
    {
        return None;
    }
    let d =
        Diagnostic::refused(RefusalClass::Feature, "odr-runtime-unsupported-target").note(format!(
            "--odr-runtime={} links the load-time check on AArch64 and x86-64 only; this link is \
             for {}",
            plan.diagnostics.odr.runtime.word(),
            machine.name()
        ));
    Some((Key::new(mortar_input::problem::PHASE, u32::MAX, 0), d))
}

/// The runtime id of the object a link adds: its index in [`OBJECTS`].
#[must_use]
pub fn runtime_id(machine: Machine, shared: bool) -> Option<u32> {
    OBJECTS
        .iter()
        .position(|o| o.machine == machine && o.shared == shared)
        .and_then(|i| u32::try_from(i).ok())
}

/// The note's mode word.
#[must_use]
pub fn mode_word(runtime: OdrRuntime) -> Option<u32> {
    match runtime {
        OdrRuntime::Off => None,
        OdrRuntime::Report => Some(mortar_odr::RT_MODE_REPORT),
        OdrRuntime::Fatal => Some(mortar_odr::RT_MODE_FATAL),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(d: &[u8]) -> String {
        use std::fmt::Write as _;
        d.iter().fold(String::new(), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
    }

    #[test]
    fn digests_are_the_committed_objects() {
        for o in &OBJECTS {
            assert_eq!(
                hex(&mortar_digest::sha256(o.bytes)),
                o.sha256,
                "{:?} {}",
                o.machine,
                o.shared
            );
        }
    }

    #[test]
    fn runtime_ids_are_table_indices() {
        assert_eq!(runtime_id(Machine::AArch64, true), Some(0));
        assert_eq!(runtime_id(Machine::X86_64, false), Some(3));
        assert_eq!(runtime_id(Machine::I386, false), Some(5));
        for (m, b) in SHARED.iter().chain(EXEC.iter()) {
            assert!(OBJECTS.iter().any(|o| o.machine == *m && o.bytes == *b));
        }
    }

    #[test]
    fn a_plain_plan_is_untouched() {
        assert!(effective(&Plan::default()).is_none());
        assert!(embedded(&Plan::default()).is_none());
    }
}

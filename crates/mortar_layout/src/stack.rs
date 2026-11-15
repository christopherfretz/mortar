//! `PT_GNU_STACK` (`spec/layout.md` L-84): whether the stack is
//! executable, and the `executable-stack` warning.

use mortar_core::inputs::{Inputs, Reader};
use mortar_core::resolution::{FileLive, Resolution};
use mortar_elf::consts::SHF_EXECINSTR;

use crate::diag::LayoutDiag;

/// One object file's `.note.GNU-stack`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackNote {
    /// No `.note.GNU-stack` section.
    Absent,
    /// A note without `SHF_EXECINSTR` (`""`).
    NonExec,
    /// A note with `SHF_EXECINSTR` (`"x"`).
    Exec,
}

/// The notes of every object file linked in (L-84: live or not, as long
/// as the file takes part: an archive member never extracted does not),
/// in `FileId` (load) order, each with its display name.
#[must_use]
pub fn notes(inputs: &Inputs<'_>, resolution: &Resolution) -> Vec<(String, StackNote)> {
    let mut out = Vec::new();
    for (file, row) in inputs.files.iter() {
        let Reader::Object(object) = row.reader else { continue };
        if resolution.file_live.get(file).is_none_or(|l| *l == FileLive::Dead) {
            continue;
        }
        let mut note = StackNote::Absent;
        for s in object.sections() {
            if s.name().ok() == Some(b".note.GNU-stack") {
                note = if s.flags() & SHF_EXECINSTR != 0 {
                    StackNote::Exec
                } else {
                    StackNote::NonExec
                };
                break;
            }
        }
        out.push((display(&row.display()), note));
    }
    out
}

/// A file as diagnostics name it: `path` or `archive(member)`.
fn display(r: &mortar_diag::FileRef) -> String {
    match r.member() {
        Some(m) => format!("{}({m})", r.path()),
        None => r.path().to_owned(),
    }
}

/// L-84: whether `PT_GNU_STACK` gets `PF_X`, and the `executable-stack`
/// warning (no option, a note asks for it): it names the first such file
/// in load order.
#[must_use]
pub fn executable_stack(
    notes: &[(String, StackNote)],
    option: Option<bool>,
) -> (bool, Option<LayoutDiag>) {
    let first_exec = notes.iter().find(|(_, n)| *n == StackNote::Exec);
    match option {
        Some(exec) => (exec, None),
        None => (
            first_exec.is_some(),
            first_exec.map(|(file, _)| crate::diag::executable_stack(file)),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: layout.md §11.6 L-84 (the PT_GNU_STACK table and its warning)
    #[test]
    fn l84_table() {
        let f = |i: u32| format!("f{i}.o");
        let none = [(f(1), StackNote::Absent), (f(2), StackNote::Absent)];
        let nonexec = [(f(1), StackNote::NonExec), (f(2), StackNote::Absent)];
        let exec = [(f(1), StackNote::NonExec), (f(2), StackNote::Exec), (f(3), StackNote::Exec)];
        assert_eq!(executable_stack(&none, None), (false, None));
        assert_eq!(executable_stack(&none, Some(true)), (true, None));
        assert_eq!(executable_stack(&none, Some(false)), (false, None));
        assert_eq!(executable_stack(&nonexec, None), (false, None));
        assert_eq!(executable_stack(&nonexec, Some(true)), (true, None));
        let warn = crate::diag::executable_stack(&f(2));
        assert_eq!(executable_stack(&exec, None), (true, Some(warn)));
        assert_eq!(executable_stack(&exec, Some(true)), (true, None));
        assert_eq!(executable_stack(&exec, Some(false)), (false, None));
        assert_eq!(executable_stack(&[], None), (false, None));
    }
}

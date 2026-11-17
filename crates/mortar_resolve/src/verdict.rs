//! Two functions other phases apply per key: the output binding of
//! `spec/resolve.md` §3.7 (`mortar_synth`) and the verdict of §5.2 for a
//! surviving reference (the relocation scan, phase 70).

use mortar_core::SymbolId;
use mortar_elf::consts::{STB_GLOBAL, STB_GNU_UNIQUE, STB_LOCAL, STB_WEAK, STV_HIDDEN};

use crate::resolve::{Def, Props};

/// A key's `.symtab` entry in an M1 (static) output (§3.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymtabEntry {
    /// Binding and visibility.
    Entry { binding: u8, visibility: u8 },
    /// §8 decides (`Provide`).
    Linker,
    /// Not in `.symtab`: undefined (whether a live relocation uses it is
    /// not resolution's to know; an unused one is absent, E15).
    Absent,
}

/// §3.7's `.symtab` column. `unique`: the definition is
/// `STB_GNU_UNIQUE`.
#[must_use]
pub fn symtab_entry(def: Def, props: Props, unique: bool) -> SymtabEntry {
    let visibility = props.visibility;
    match def {
        Def::Undefined { .. } | Def::Shared(_) => SymtabEntry::Absent,
        Def::Provide { .. } => SymtabEntry::Linker,
        _ if visibility == STV_HIDDEN => SymtabEntry::Entry { binding: STB_LOCAL, visibility },
        Def::Strong(_) => SymtabEntry::Entry {
            binding: if unique { STB_GNU_UNIQUE } else { STB_GLOBAL },
            visibility,
        },
        Def::Common(_) | Def::Defsym(_) => SymtabEntry::Entry { binding: STB_GLOBAL, visibility },
        Def::Weak(_) => SymtabEntry::Entry { binding: STB_WEAK, visibility },
    }
}

/// The binding of a key's `.symtab` entry in a `-r` output
/// (`spec/resolve.md` RV-R3, RV-R6): the definition's, weak or global,
/// whatever the merged visibility (a hidden key is not demoted to local in
/// an `ET_REL`); an undefined key's is weak when every reference is.
/// `None` for the definitions a `-r` does not write as a key of its own
/// (a shared object's, refused at `-r`; a linker-defined or `--defsym`
/// symbol, which RV-R8 and RL-11 decide).
#[must_use]
pub fn binding_at_r(def: Def) -> Option<u8> {
    match def {
        Def::Undefined { weak: true } | Def::Weak(_) => Some(STB_WEAK),
        Def::Undefined { weak: false } | Def::Strong(_) | Def::Common(_) => Some(STB_GLOBAL),
        Def::Provide { .. } | Def::Shared(_) | Def::Defsym(_) => None,
    }
}

/// `--unresolved-symbols` and `--warn-unresolved-symbols` (§5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Unresolved {
    /// The default: an error.
    #[default]
    Error,
    /// `--warn-unresolved-symbols` (the last of it and
    /// `--error-unresolved-symbols` wins).
    Warn,
    /// `=ignore-all` or `=ignore-in-object-files`.
    Ignore,
}

/// What the scan does with a surviving reference to a key (§5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Verdict {
    /// The key is defined: its value.
    Defined,
    /// Resolves to zero (weak undefined, or ignored).
    Zero,
    /// `undefined-symbol` as a warning; the value is zero.
    Warning,
    /// `undefined-symbol`, an error.
    Error,
}

/// The verdict for a surviving reference to `key` in an executable.
/// `ignored` holds the `--ignore-unresolved-symbol` keys, sorted.
#[must_use]
pub fn verdict(
    key: SymbolId,
    def: Def,
    props: Props,
    mode: Unresolved,
    ignored: &[SymbolId],
) -> Verdict {
    let Def::Undefined { weak } = def else {
        return Verdict::Defined;
    };
    if weak {
        return Verdict::Zero;
    }
    if props.visibility == STV_HIDDEN {
        // It cannot be satisfied at run time, whatever the options.
        return Verdict::Error;
    }
    if ignored.binary_search(&key).is_ok() {
        return Verdict::Zero;
    }
    match mode {
        Unresolved::Error => Verdict::Error,
        Unresolved::Warn => Verdict::Warning,
        Unresolved::Ignore => Verdict::Zero,
    }
}

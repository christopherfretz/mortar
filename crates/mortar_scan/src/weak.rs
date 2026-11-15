//! The weak classes (`spec/dyn.md` D-25): which undefined weak keys of
//! an `Exec` or `Pie` are `WeakDyn` and which stay `Weak0`, decided per
//! **key** (arch.md §2.1: `target_kind` is a property of the key, never
//! of the relocation class) from every live relocation that names it,
//! before the scan's demands, which read the decision.
//!
//! Step 0 (WP-041) states the contract and the place in the driver; the
//! walk that fills [`WeakUses`] from `Arch::weak_use` is WP-046's. Until
//! then [`weak_classes`] returns no weak-dynamic key, so every undefined
//! weak key is `Weak0`, as at M1.

use mortar_arch::{Arch, OutputKind, WeakClass};
use mortar_core::{IdSet, SectionId, SymbolId};
use mortar_diag::{Diagnostic, Key};

use crate::{ScanInputs, ScanOptions};

/// What the weak classes need of the section mapping (D-25: "a writable
/// place is a place whose output section has `SHF_WRITE` after layout.md's
/// section rules, known from the section mapping, before addresses";
/// RELRO counts as writable).
pub trait SectionMap: Sync {
    /// Whether the output section that live input section `section` maps
    /// to is writable.
    fn writable(&self, section: SectionId) -> bool;
}

/// The uses of one undefined weak key, over its live relocations
/// (`spec/dyn.md` D-25's table, by [`WeakClass`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WeakUses {
    pub got: bool,
    pub abs_word: bool,
    pub branch: bool,
    /// The first unfillable use in input order, as the scan key of its
    /// relocation: where `weak-undefined-pc-in-pie` is reported.
    pub unfillable: Option<Key>,
}

/// What [`WeakUses`] make of a key (D-25).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeakVerdict {
    /// Undefined weak, value 0.
    Weak0,
    /// In `.dynsym`, preemptible: GOT slots get `GLOB_DAT`, absolute
    /// words a symbolic record, branches a PLT entry.
    WeakDyn,
    /// `weak-undefined-pc-in-pie`, at the given key (`Pie` only).
    Refused(Key),
}

impl WeakUses {
    /// Adds one relocation's use (`None`: the relocation does not use the
    /// key); `at` is its scan key, the order of input.
    pub fn add(&mut self, class: Option<WeakClass>, at: Key) {
        match class {
            Some(WeakClass::Got) => self.got = true,
            Some(WeakClass::AbsWord) => self.abs_word = true,
            Some(WeakClass::Branch) => self.branch = true,
            Some(WeakClass::Unfillable) if self.unfillable.is_none_or(|k| at < k) => {
                self.unfillable = Some(at);
            }
            Some(WeakClass::Unfillable) | None => {}
        }
    }

    /// D-25: `WeakDyn` in `Exec` and `Pie` with a GOT use or an absolute
    /// word in a writable place and no unfillable use; in `Pie` an
    /// unfillable use is refused, once per key; `Weak0` otherwise (every
    /// output without `.dynamic`, static PIE included, and an `Exec` key
    /// with an unfillable use). `dynamic`: the output has `.dynamic`
    /// ([`ScanOptions::dynamic`]).
    #[must_use]
    pub fn verdict(&self, output: OutputKind, dynamic: bool) -> WeakVerdict {
        let dynamic = dynamic && matches!(output, OutputKind::Exec | OutputKind::Pie);
        match self.unfillable {
            Some(at) if dynamic && output == OutputKind::Pie => WeakVerdict::Refused(at),
            None if dynamic && (self.got || self.abs_word) => WeakVerdict::WeakDyn,
            _ => WeakVerdict::Weak0,
        }
    }
}

/// The decision for the whole link.
#[derive(Debug, Clone, Default)]
pub struct WeakKeys {
    /// The `WeakDyn` keys: `Demands.weak_dyn` (`spec/model.md` §3.7).
    pub weak_dyn: IdSet<SymbolId>,
    /// One `weak-undefined-pc-in-pie` per refused key, at its first
    /// unfillable relocation.
    pub refused: Vec<(Key, Diagnostic)>,
}

/// `spec/dyn.md` D-25: the weak classes of every undefined weak key of
/// the link, from each live relocation that names it through
/// `arch.weak_use` and `sections`. Runs after liveness and before the
/// scan's walk (the demands read [`WeakKeys::weak_dyn`]). Step 0: no
/// weak-dynamic key, no refusal; a static link never has one.
#[must_use]
pub fn weak_classes(
    arch: &dyn Arch,
    t: &ScanInputs<'_, '_>,
    opts: &ScanOptions<'_>,
    sections: Option<&dyn SectionMap>,
) -> WeakKeys {
    let _ = (arch, t, opts, sections);
    WeakKeys::default()
}

#[cfg(test)]
mod tests {
    use super::{WeakUses, WeakVerdict};
    use mortar_arch::{OutputKind, WeakClass};
    use mortar_diag::Key;

    // spec: dyn.md D-25 (the per-key decision)
    #[test]
    fn d25_verdicts() {
        let (k1, k2) = (Key::new(70, 1, 5), Key::new(70, 2, 0));
        let mut got = WeakUses::default();
        got.add(Some(WeakClass::Got), k1);
        got.add(Some(WeakClass::Branch), k1);
        got.add(None, k1);
        assert_eq!(got.verdict(OutputKind::Exec, true), WeakVerdict::WeakDyn);
        assert_eq!(got.verdict(OutputKind::Pie, true), WeakVerdict::WeakDyn);
        assert_eq!(got.verdict(OutputKind::Static, false), WeakVerdict::Weak0);
        assert_eq!(got.verdict(OutputKind::Pie, false), WeakVerdict::Weak0, "static PIE");
        let mut branch = WeakUses::default();
        branch.add(Some(WeakClass::Branch), k1);
        assert_eq!(branch.verdict(OutputKind::Exec, true), WeakVerdict::Weak0, "no GOT, no word");
        let mut pc = got;
        pc.add(Some(WeakClass::Unfillable), k2);
        pc.add(Some(WeakClass::Unfillable), k1);
        assert_eq!(pc.verdict(OutputKind::Pie, true), WeakVerdict::Refused(k1), "first in input");
        assert_eq!(pc.verdict(OutputKind::Exec, true), WeakVerdict::Weak0);
        assert_eq!(pc.verdict(OutputKind::Pie, false), WeakVerdict::Weak0, "static PIE");
    }
}

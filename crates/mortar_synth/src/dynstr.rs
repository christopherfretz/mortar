//! `.dynstr`'s plan (`spec/dyn.md` D-24): the offset of every name the
//! dynamic sections write, as `mortar_core::DynStr` in `Slots`. Owned by
//! WP-M2-DYNSYM; WP-041 fixed the signature and left [`dynstr_plan`] a
//! stub. `.dynamic` and the version writer read offsets from the
//! `DynStr`; they never build strings.

use mortar_core::id::{FileId, SymbolId};
use mortar_core::{DynStr, VerIndex};

use crate::dynsym::DynsymPlan;

/// The names `.dynstr` holds, as the inputs give them (a WP-041 addition
/// to the cut's signature: the plan cannot name what it cannot read).
pub trait DynNames {
    /// The name of a `.dynsym` entry's key (without a version).
    fn symbol(&self, key: SymbolId) -> &[u8];
    /// The `DT_NEEDED` name of a needed shared object: its soname, else
    /// its name as written (D-19a).
    fn needed(&self, file: FileId) -> &[u8];
    /// The name of version `version` of shared object `file` (D-27).
    fn version(&self, file: FileId, version: VerIndex) -> &[u8];
}

/// D-24: byte 0 NUL, then each distinct string once in order of first
/// use: the `.dynsym` names in `.dynsym` order, the `DT_NEEDED` names in
/// `needed` order, the version names of `verneed` in its order, the run
/// path; no tail merging.
///
/// **Stub** (WP-041): returns the empty `DynStr`; WP-M2-DYNSYM fills it. It
/// is never called while dynamic output is refused.
#[must_use]
pub fn dynstr_plan(
    dynsym: &DynsymPlan,
    needed: &[FileId],
    verneed: &[(FileId, Box<[VerIndex]>)],
    rpath: Option<&[u8]>,
    names: &dyn DynNames,
) -> DynStr {
    let _ = (dynsym, needed, verneed, rpath, names);
    DynStr::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Names;
    impl DynNames for Names {
        fn symbol(&self, _: SymbolId) -> &[u8] {
            b"puts"
        }
        fn needed(&self, _: FileId) -> &[u8] {
            b"libc.so.6"
        }
        fn version(&self, _: FileId, _: VerIndex) -> &[u8] {
            b"GLIBC_2.17"
        }
    }

    // dyn.md D-24: the signature a package builds against, with a fixture
    // plan and fixture names; the stub's plan is empty.
    #[test]
    fn the_signature_with_fixtures() {
        let plan = DynsymPlan::new(vec![SymbolId(1)].into(), 1, 1).unwrap();
        let verneed = [(FileId(1), vec![VerIndex(2)].into_boxed_slice())];
        let d = dynstr_plan(&plan, &[FileId(1)], &verneed, Some(b"/opt/lib"), &Names);
        assert!(d.is_empty());
    }
}

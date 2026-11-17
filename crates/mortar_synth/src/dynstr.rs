//! `.dynstr` (`spec/dyn.md` D-24; `spec/synth.md` S-103): the offset of
//! every name the dynamic sections write, as `mortar_core::DynStr` in
//! `Slots`, and the section's bytes. Owned by WP-M2-DYNSYM. `.dynamic`
//! and the version writer read offsets from the `DynStr`; they never
//! build strings.
//!
//! Byte 0 is NUL; then each distinct string once, in order of first use:
//! the `.dynsym` names in `.dynsym` order, the `DT_NEEDED` names in
//! `Slots.needed` order, the version names of `.gnu.version_r` in its
//! order, the run path. No tail merging: the size is `1 + Σ (string + 1)`
//! over the distinct strings and every offset a prefix sum (S-108).
//!
//! A shared object (`spec/dyn.md` D-116) places its `-soname` string after
//! the `DT_NEEDED` names ([`dynstr_plan_shared`]); its offset is not a
//! field of `DynStr` (a core type) but travels beside it, to the writer
//! ([`DynstrChunk::soname`]) and to `.dynamic` (`DynValue::SonameName`).
//! The `Verdef` names (D-116, WP-125) follow the soname, the base's
//! first ([`dynstr_plan_versioned`]); their offsets are in the
//! [`crate::version::VerdefPlan`], which the writer reads
//! ([`DynstrChunk::verdef`]).

use std::collections::HashMap;
use std::ops::Range;

use mortar_core::id::{FileId, SymbolId};
use mortar_core::{DynStr, VerIndex};

use crate::dynsym::{DynsymPlan, whole_range};
use crate::error::{SynthError, internal};
use crate::image::{Division, FillCx, SizeCx, SynthChunk};

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
    /// The `vd_hash` shared object `file` writes for `version` (D-14), if
    /// the inputs keep it; `.gnu.version_r` checks it against the ELF hash
    /// of the name. `None`: not kept, nothing to check.
    fn version_hash(&self, file: FileId, version: VerIndex) -> Option<u32> {
        let _ = (file, version);
        None
    }
}

/// D-24: byte 0 NUL, then each distinct string once in order of first
/// use: the `.dynsym` names in `.dynsym` order, the `DT_NEEDED` names in
/// `needed` order, the version names of `verneed` in its order, the run
/// path; no tail merging. A function of the plan, the lists and the
/// names alone.
#[must_use]
pub fn dynstr_plan(
    dynsym: &DynsymPlan,
    needed: &[FileId],
    verneed: &[(FileId, Box<[VerIndex]>)],
    rpath: Option<&[u8]>,
    names: &dyn DynNames,
) -> DynStr {
    dynstr_plan_shared(dynsym, needed, None, verneed, rpath, names).0
}

/// D-116: [`dynstr_plan`] with a shared object's `-soname` string placed
/// after the `DT_NEEDED` names and before the version names (one copy of
/// each distinct string: a soname equal to an earlier string shares its
/// offset). Returns the table and the soname's offset (`None` without
/// `-soname`). With `soname` `None` it is exactly [`dynstr_plan`].
#[must_use]
pub fn dynstr_plan_shared(
    dynsym: &DynsymPlan,
    needed: &[FileId],
    soname: Option<&[u8]>,
    verneed: &[(FileId, Box<[VerIndex]>)],
    rpath: Option<&[u8]>,
    names: &dyn DynNames,
) -> (DynStr, Option<u32>) {
    dynstr_plan_versioned(dynsym, needed, soname, None, verneed, rpath, names)
}

/// D-116: [`dynstr_plan_shared`] with the `Verdef` names of `verdef`
/// placed after the soname, the base's first (each distinct string once:
/// a base named by the soname shares its offset); their offsets are
/// written into `verdef`. With `verdef` `None` it is exactly
/// [`dynstr_plan_shared`].
#[must_use]
pub fn dynstr_plan_versioned(
    dynsym: &DynsymPlan,
    needed: &[FileId],
    soname: Option<&[u8]>,
    verdef: Option<&mut crate::version::VerdefPlan>,
    verneed: &[(FileId, Box<[VerIndex]>)],
    rpath: Option<&[u8]>,
    names: &dyn DynNames,
) -> (DynStr, Option<u32>) {
    // `Strings` borrows its strings: the names are copied out of the plan
    // that gets their offsets.
    let verdef_names: Vec<Box<[u8]>> =
        verdef.as_ref().map(|v| v.defs.iter().map(|d| d.name.clone()).collect()).unwrap_or_default();
    let mut t = Strings { seen: HashMap::with_capacity(dynsym.order().len() + 16), size: 1 };
    let symbols: Box<[u32]> = dynsym.order().iter().map(|&k| t.add(names.symbol(k))).collect();
    let needed_offs: Box<[u32]> = needed.iter().map(|&f| t.add(names.needed(f))).collect();
    let soname_off = soname.map(|s| t.add(s));
    let verdef_offs: Vec<u32> = verdef_names.iter().map(|n| t.add(n)).collect();
    let versions: Box<[Box<[u32]>]> = verneed
        .iter()
        .map(|(f, vs)| vs.iter().map(|&v| t.add(names.version(*f, v))).collect())
        .collect();
    let runpath = rpath.map(|r| t.add(r));
    let size = t.size;
    drop(t);
    if let Some(v) = verdef {
        for (d, o) in v.defs.iter_mut().zip(verdef_offs) {
            d.name_off = o;
        }
    }
    let table = DynStr {
        symbols,
        needed: needed_offs,
        versions,
        runpath,
        size: size.min(u64::from(u32::MAX)) as u32,
    };
    (table, soname_off)
}

/// The distinct strings so far and the size they take.
struct Strings<'a> {
    seen: HashMap<&'a [u8], u32>,
    size: u64,
}

impl<'a> Strings<'a> {

    /// The offset of `s`, appended at the end on its first use. An offset
    /// past `u32` is clamped; `dynsym::chunk_sizes` refuses such a table.
    fn add(&mut self, s: &'a [u8]) -> u32 {
        let size = &mut self.size;
        *self.seen.entry(s).or_insert_with(|| {
            let at = *size;
            *size += s.len() as u64 + 1;
            at.min(u64::from(u32::MAX)) as u32
        })
    }
}

/// The `.dynstr` chunk: written whole (one job), each list's names at
/// their offsets (a string named twice is written twice at the same
/// offset, with the same bytes).
#[derive(Clone, Copy)]
pub struct DynstrChunk<'a> {
    pub plan: &'a DynsymPlan,
    pub needed: &'a [FileId],
    pub verneed: &'a [(FileId, Box<[VerIndex]>)],
    pub rpath: Option<&'a [u8]>,
    /// A shared object's `-soname` string and its offset
    /// ([`dynstr_plan_shared`]); `None` otherwise.
    pub soname: Option<(&'a [u8], u32)>,
    /// A shared object's version definitions, their names' offsets set
    /// ([`dynstr_plan_versioned`]); `None` otherwise.
    pub verdef: Option<&'a crate::version::VerdefPlan>,
    pub dynstr: &'a DynStr,
    pub names: &'a (dyn DynNames + Sync),
}

impl std::fmt::Debug for DynstrChunk<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DynstrChunk").field("dynstr", self.dynstr).finish_non_exhaustive()
    }
}

impl DynstrChunk<'_> {
    /// The size (S-108).
    #[must_use]
    pub fn size(&self) -> u64 {
        u64::from(self.dynstr.size)
    }

    /// The whole section into `out` (exactly [`Self::size`] bytes).
    ///
    /// # Errors
    ///
    /// `internal-error` when the offsets do not match the lists or a
    /// string does not fit.
    pub fn write_all(&self, out: &mut [u8]) -> Result<(), SynthError> {
        let d = self.dynstr;
        let shapes = d.symbols.len() == self.plan.order().len()
            && d.needed.len() == self.needed.len()
            && d.versions.len() == self.verneed.len()
            && d.versions.iter().zip(self.verneed).all(|(o, v)| o.len() == v.1.len())
            && d.runpath.is_some() == self.rpath.is_some()
            && out.len() as u64 == self.size();
        if !shapes {
            return Err(internal(".dynstr: the offsets do not match the lists they name"));
        }
        out.fill(0);
        let mut put = |at: u32, s: &[u8]| -> Result<(), SynthError> {
            let at = at as usize;
            let end = at + s.len();
            if at == 0 || end >= out.len() {
                return Err(internal(".dynstr: a string outside the section"));
            }
            out[at..end].copy_from_slice(s);
            Ok(())
        };
        for (&k, &o) in self.plan.order().iter().zip(&d.symbols) {
            put(o, self.names.symbol(k))?;
        }
        for (&f, &o) in self.needed.iter().zip(&d.needed) {
            put(o, self.names.needed(f))?;
        }
        if let Some((s, o)) = self.soname {
            put(o, s)?;
        }
        for d in self.verdef.map_or(&[][..], |v| &v.defs) {
            put(d.name_off, &d.name)?;
        }
        for ((f, vs), offs) in self.verneed.iter().zip(&d.versions) {
            for (&v, &o) in vs.iter().zip(offs) {
                put(o, self.names.version(*f, v))?;
            }
        }
        if let (Some(r), Some(o)) = (self.rpath, d.runpath) {
            put(o, r)?;
        }
        Ok(())
    }
}

impl SynthChunk for DynstrChunk<'_> {
    fn size(&self, _: &SizeCx<'_>) -> u64 {
        DynstrChunk::size(self)
    }

    /// Whole: the strings are placed by first use, which only the whole
    /// list knows.
    fn division(&self) -> Division {
        Division::Whole { max_size: DynstrChunk::size(self) }
    }

    fn write(&self, _: &FillCx<'_>, range: Range<u64>, out: &mut [u8]) -> Result<(), SynthError> {
        whole_range(&range, out, DynstrChunk::size(self), |buf| self.write_all(buf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Names;
    impl DynNames for Names {
        fn symbol(&self, k: SymbolId) -> &[u8] {
            match k.0 {
                1 => b"puts",
                2 => b"main",
                _ => b"libc.so.6",
            }
        }
        fn needed(&self, _: FileId) -> &[u8] {
            b"libc.so.6"
        }
        fn version(&self, _: FileId, _: VerIndex) -> &[u8] {
            b"GLIBC_2.17"
        }
    }

    // dyn.md D-24: first use, each distinct string once (a symbol named
    // like the needed library shares its bytes), no tail merging.
    #[test]
    fn first_use_and_sharing() {
        let plan =
            DynsymPlan::new(vec![SymbolId(1), SymbolId(2), SymbolId(3)].into(), 1, 1).unwrap();
        let verneed = [(FileId(1), vec![VerIndex(2)].into_boxed_slice())];
        let d = dynstr_plan(&plan, &[FileId(1)], &verneed, Some(b"/opt/lib"), &Names);
        assert_eq!(&d.symbols[..], [1, 6, 11]);
        assert_eq!(&d.needed[..], [11]);
        assert_eq!(&d.versions[0][..], [21]);
        assert_eq!(d.runpath, Some(32));
        assert_eq!(d.size, 41);
        let chunk = DynstrChunk {
            plan: &plan,
            needed: &[FileId(1)],
            verneed: &verneed,
            rpath: Some(b"/opt/lib"),
            soname: None,
            verdef: None,
            dynstr: &d,
            names: &Names,
        };
        let mut out = vec![0xaa; 41];
        chunk.write_all(&mut out).unwrap();
        assert_eq!(&out[..], b"\0puts\0main\0libc.so.6\0GLIBC_2.17\0/opt/lib\0");
    }
}

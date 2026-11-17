//! `.gnu.version`, `.gnu.version_d` and `.gnu.version_r` (`spec/dyn.md`
//! D-27, D-115; `spec/synth.md` S-103). Owned by WP-M2-DYNSYM;
//! `.gnu.version_d` WP-125.
//!
//! **Version definitions** ([`VerdefPlan`], [`VerdefChunk`]; D-115, a
//! shared object with a named version node only): index 1 the base
//! (`VER_FLG_BASE`, named by the soname, else the output's basename), then
//! one per node in script order, indices 2…; each `Verdef` (20 bytes)
//! followed by its `Verdaux` entries (8 bytes each): its own name first,
//! then its parents' in the order written. An export's `.gnu.version` is
//! its resolution version (`SymProps.version`: the node's index, with
//! `VERSYM_HIDDEN` for a non-default `foo@V`; 0 is written 1), and the
//! `vna_other` numbers continue after the last `Verdef` index.
//!
//! **Plan** ([`verneed_plan`]): `Slots.verneed`, one row per needed shared
//! object that provides a version some `.dynsym` entry with a shared
//! definition binds to, in `Slots.needed` order, its versions by
//! increasing definition index (the library's own order).
//!
//! **Write**: `.gnu.version` ([`VersymChunk`], divisible) holds one `u16`
//! per `.dynsym` entry: 0 for the null entry, 1 for exports, `WeakDyn` keys
//! and unversioned shared definitions, the need's `vna_other` for a
//! versioned one; bit 15 never set. `.gnu.version_r` ([`VerneedChunk`],
//! whole) holds each `Verneed` followed directly by its `Vernaux`
//! entries; `vn_file` the `DT_NEEDED` name's `.dynstr` offset, `vna_hash`
//! the ELF hash of the version name, `vna_flags` 0, `vna_other` numbered
//! from 2 in output order.

use std::ops::Range;

use mortar_core::id::{FileId, SymbolId};
use mortar_core::resolution::Def;
use mortar_core::{DynStr, VerIndex};
use mortar_diag::{Collector, Diagnostic};
use mortar_elf::Class;

use crate::dynsym::{DynsymInputs, DynsymPlan, check_range, elf64, whole_range};
use crate::error::{Failed, SynthError, feature, internal, key, malformed};
use crate::image::{Division, FillCx, SizeCx, SynthChunk};
use crate::out::Window;

/// `Slots.verneed`'s shape: per needed object, its versions.
pub type Verneed = [(FileId, Box<[VerIndex]>)];

/// The System V ABI's ELF hash (`vd_hash`, `vna_hash`).
#[must_use]
pub fn elf_hash(name: &[u8]) -> u32 {
    let mut h: u32 = 0;
    for &c in name {
        h = (h << 4).wrapping_add(u32::from(c));
        let g = h & 0xf000_0000;
        if g != 0 {
            h ^= g >> 24;
        }
        h &= !g;
    }
    h
}

/// The version a `.dynsym` entry binds to: `(file, version)` for a key
/// with a shared definition whose version index is 2 or more.
fn bound(t: &DynsymInputs<'_>, k: SymbolId) -> Option<(FileId, VerIndex)> {
    let Some(Def::Shared(e)) = t.resolution.def.get(k).copied() else { return None };
    let v = t.src.bound_version(e)?;
    (v.0 & 0x7fff >= 2).then_some((e.file, VerIndex(v.0 & 0x7fff)))
}

/// D-27's `Slots.verneed` for `plan`: per needed object (in `needed`
/// order) that provides a version an entry with a shared definition
/// (import, canonical PLT, copy alias) binds to, those versions by
/// increasing index.
///
/// # Errors
///
/// [`Failed`], with a refusal under `(80, 0, SymbolId)`, when an entry
/// binds to a version of a shared object that is not needed (it has no
/// `DT_NEEDED` name for a `Verneed`).
pub fn verneed_plan(
    plan: &DynsymPlan,
    t: &DynsymInputs<'_>,
    needed: &[FileId],
    diag: &Collector,
) -> Result<Box<Verneed>, Failed> {
    let mut rows: Vec<(usize, VerIndex)> = Vec::new();
    let mut failed = false;
    for &k in plan.order() {
        let Some((file, v)) = bound(t, k) else { continue };
        if let Some(at) = needed.iter().position(|&n| n == file) {
            rows.push((at, v));
        } else {
            {
                let name = String::from_utf8_lossy(t.src.symbol(k)).into_owned();
                diag.emit(
                    key(u64::from(k.0)),
                    feature(format!(
                        "versioned reference to {name} bound in a shared object that is not \
                         needed (spec/dyn.md D-27)"
                    )),
                );
                failed = true;
            }
        }
    }
    if failed {
        return Err(Failed);
    }
    rows.sort_unstable();
    rows.dedup();
    let mut out: Vec<(FileId, Box<[VerIndex]>)> = Vec::new();
    for group in rows.chunk_by(|a, b| a.0 == b.0) {
        out.push((needed[group[0].0], group.iter().map(|r| r.1).collect()));
    }
    Ok(out.into())
}

/// The first `vna_other`: 2, or after the last `Verdef` index when the
/// output defines `verdefs` versions (D-115; the base included).
#[must_use]
pub fn first_other(verdefs: u16) -> u16 {
    if verdefs == 0 { 2 } else { verdefs + 1 }
}

/// `vna_other` of every `(file, version)` of `verneed`, sorted: numbered
/// from [`first_other`] in output order.
fn others(verneed: &Verneed, verdefs: u16) -> Vec<((FileId, VerIndex), u16)> {
    let first = first_other(verdefs);
    let mut v: Vec<((FileId, VerIndex), u16)> = verneed
        .iter()
        .flat_map(|(f, vs)| vs.iter().map(move |&x| (*f, x)))
        .enumerate()
        .map(|(i, fv)| (fv, first + i as u16))
        .collect();
    v.sort_unstable();
    v
}

/// `.gnu.version`'s size (S-108): 2 bytes per `.dynsym` entry; 0 (no
/// section) without `.gnu.version_r`.
#[must_use]
pub fn versym_size(plan: &DynsymPlan, verneed: &Verneed) -> u64 {
    versym_size_with(plan, verneed, 0)
}

/// [`versym_size`] for an output that defines `verdefs` versions: 0 (no
/// section) without `.gnu.version_r` and `.gnu.version_d`.
#[must_use]
pub fn versym_size_with(plan: &DynsymPlan, verneed: &Verneed, verdefs: u16) -> u64 {
    if verneed.is_empty() && verdefs == 0 { 0 } else { 2 * u64::from(plan.count()) }
}

/// One `Verdef` of [`VerdefPlan`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerdefEntry {
    /// The version's name (the base's: the soname or the basename).
    pub name: Box<[u8]>,
    /// The parents' `Verdef` indices (2…), in the order written.
    pub parents: Box<[u16]>,
    /// The name's `.dynstr` offset (set by
    /// `crate::dynstr::dynstr_plan_versioned`).
    pub name_off: u32,
}

/// D-115: the output's version definitions, the base first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VerdefPlan {
    pub defs: Box<[VerdefEntry]>,
}

impl VerdefPlan {
    /// The base named `base`, then `nodes` in script order, each its name
    /// and its parents as indices of earlier nodes. No node: no section.
    #[must_use]
    pub fn new(base: &[u8], nodes: &[(&[u8], &[usize])]) -> Self {
        if nodes.is_empty() {
            return Self::default();
        }
        let mut defs = Vec::with_capacity(nodes.len() + 1);
        defs.push(VerdefEntry { name: base.into(), parents: Box::new([]), name_off: 0 });
        for (name, parents) in nodes {
            defs.push(VerdefEntry {
                name: (*name).into(),
                parents: parents.iter().map(|&p| (p + 2).min(0x7fff) as u16).collect(),
                name_off: 0,
            });
        }
        Self { defs: defs.into() }
    }

    /// The number of `Verdef` entries (`DT_VERDEFNUM`, `sh_info`); 0
    /// without the section.
    #[must_use]
    pub fn count(&self) -> u16 {
        self.defs.len().min(0x7fff) as u16
    }

    /// `.gnu.version_d`'s size (S-108): 20 bytes per `Verdef`, 8 per
    /// `Verdaux`.
    #[must_use]
    pub fn size(&self) -> u64 {
        self.defs.iter().map(|d| 20 + 8 * (1 + d.parents.len() as u64)).sum()
    }
}

/// `VER_FLG_BASE`.
const VER_FLG_BASE: u16 = 1;

/// The `.gnu.version_d` chunk: whole. `plan` `None`: no section (an
/// executable).
#[derive(Clone, Copy, Debug)]
pub struct VerdefChunk<'a> {
    pub plan: Option<&'a VerdefPlan>,
    pub class: Class,
}

impl VerdefChunk<'_> {
    /// The size (S-108).
    #[must_use]
    pub fn size(&self) -> u64 {
        self.plan.map_or(0, VerdefPlan::size)
    }

    /// The whole section into `out` (exactly [`Self::size`] bytes).
    ///
    /// # Errors
    ///
    /// `internal-error` when `out` is not the section's size; a refusal
    /// for an ELF32 output.
    pub fn write_all(&self, out: &mut [u8]) -> Result<(), SynthError> {
        elf64(self.class)?;
        if out.len() as u64 != self.size() {
            return Err(internal(".gnu.version_d: the size does not match the plan"));
        }
        let le16 =
            |out: &mut [u8], at: usize, v: u16| out[at..at + 2].copy_from_slice(&v.to_le_bytes());
        let le32 =
            |out: &mut [u8], at: usize, v: u32| out[at..at + 4].copy_from_slice(&v.to_le_bytes());
        let defs = self.plan.map_or(&[][..], |p| &p.defs);
        let mut at = 0usize;
        for (i, d) in defs.iter().enumerate() {
            let cnt = 1 + d.parents.len();
            let len = 20 + 8 * cnt;
            le16(out, at, 1); // vd_version
            le16(out, at + 2, if i == 0 { VER_FLG_BASE } else { 0 });
            le16(out, at + 4, (i + 1) as u16); // vd_ndx
            le16(out, at + 6, cnt as u16);
            le32(out, at + 8, elf_hash(&d.name));
            le32(out, at + 12, 20); // vd_aux
            le32(out, at + 16, if i + 1 == defs.len() { 0 } else { len as u32 });
            let mut aux = at + 20;
            let names = std::iter::once(d.name_off).chain(d.parents.iter().map(|&p| {
                defs.get(usize::from(p) - 1).map_or(0, |x| x.name_off)
            }));
            for (j, off) in names.enumerate() {
                le32(out, aux, off); // vda_name
                le32(out, aux + 4, if j + 1 == cnt { 0 } else { 8 });
                aux += 8;
            }
            at += len;
        }
        Ok(())
    }
}

impl SynthChunk for VerdefChunk<'_> {
    fn size(&self, _: &SizeCx<'_>) -> u64 {
        VerdefChunk::size(self)
    }

    fn division(&self) -> Division {
        Division::Whole { max_size: VerdefChunk::size(self) }
    }

    fn write(&self, _: &FillCx<'_>, range: Range<u64>, out: &mut [u8]) -> Result<(), SynthError> {
        whole_range(&range, out, VerdefChunk::size(self), |buf| self.write_all(buf))
    }
}

/// `.gnu.version_r`'s size (S-108): 16 bytes per `Verneed` and per
/// `Vernaux`; 0 (no section) without versioned imports.
#[must_use]
pub fn verneed_size(verneed: &Verneed) -> u64 {
    verneed.iter().map(|(_, vs)| 16 * (1 + vs.len() as u64)).sum()
}

/// The `.gnu.version` chunk: divisible, one `u16` per entry.
#[derive(Clone, Copy)]
pub struct VersymChunk<'a> {
    pub plan: &'a DynsymPlan,
    pub inputs: DynsymInputs<'a>,
    pub verneed: &'a Verneed,
    /// The output's `Verdef` count ([`VerdefPlan::count`]; 0 none).
    pub verdefs: u16,
    pub class: Class,
}

impl std::fmt::Debug for VersymChunk<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VersymChunk").field("plan", self.plan).finish_non_exhaustive()
    }
}

impl VersymChunk<'_> {
    /// The size (S-108).
    #[must_use]
    pub fn size(&self) -> u64 {
        versym_size_with(self.plan, self.verneed, self.verdefs)
    }

    /// Fills the window's range.
    ///
    /// # Errors
    ///
    /// `internal-error` when an entry binds to a version that
    /// `verneed` lacks; a refusal for an ELF32 output.
    pub fn write_window(&self, out: &mut Window<'_>) -> Result<(), SynthError> {
        elf64(self.class)?;
        let others = others(self.verneed, self.verdefs);
        let order = self.plan.order();
        let first = out.start() / 2;
        let last = out.end().div_ceil(2).min(u64::from(self.plan.count()));
        for i in first..last {
            let v: u16 = if i == 0 {
                0
            } else {
                let k = order[i as usize - 1];
                match bound(&self.inputs, k) {
                    // D-115: an export of a shared object, its version.
                    None if self.inputs.shared => {
                        let v = self.inputs.resolution.props.get(k).map_or(0, |p| p.version);
                        if v == 0 { 1 } else { v }
                    }
                    None => 1,
                    Some(fv) => match others.binary_search_by_key(&fv, |r| r.0) {
                        Ok(at) => others[at].1,
                        Err(_) => {
                            return Err(internal(format!(
                                ".gnu.version: the version of .dynsym entry {i} is not in \
                                 .gnu.version_r"
                            )));
                        }
                    },
                }
            };
            out.put(i * 2, &v.to_le_bytes());
        }
        Ok(())
    }
}

impl SynthChunk for VersymChunk<'_> {
    fn size(&self, _: &SizeCx<'_>) -> u64 {
        VersymChunk::size(self)
    }

    fn division(&self) -> Division {
        Division::Divisible
    }

    fn write(&self, _: &FillCx<'_>, range: Range<u64>, out: &mut [u8]) -> Result<(), SynthError> {
        check_range(&range, out, VersymChunk::size(self))?;
        self.write_window(&mut Window::new(range.start, out))
    }
}

/// The `.gnu.version_r` chunk: whole (one job; a few entries per needed
/// object).
#[derive(Clone, Copy)]
pub struct VerneedChunk<'a> {
    pub needed: &'a [FileId],
    pub verneed: &'a Verneed,
    /// The output's `Verdef` count (`vna_other` continues after it).
    pub verdefs: u16,
    pub dynstr: &'a DynStr,
    pub names: &'a (dyn crate::dynstr::DynNames + Sync),
    pub class: Class,
}

impl std::fmt::Debug for VerneedChunk<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerneedChunk").field("verneed", &self.verneed).finish_non_exhaustive()
    }
}

impl VerneedChunk<'_> {
    /// The size (S-108).
    #[must_use]
    pub fn size(&self) -> u64 {
        verneed_size(self.verneed)
    }

    /// The whole section into `out` (exactly [`Self::size`] bytes).
    ///
    /// # Errors
    ///
    /// `internal-error` when a list does not match `.dynstr`'s offsets;
    /// `malformed-input` when a library's `vd_hash` is not the ELF hash of
    /// its version's name; a refusal for an ELF32 output.
    pub fn write_all(&self, out: &mut [u8]) -> Result<(), SynthError> {
        elf64(self.class)?;
        let d = self.dynstr;
        if out.len() as u64 != self.size()
            || d.versions.len() != self.verneed.len()
            || d.needed.len() != self.needed.len()
        {
            return Err(internal(".gnu.version_r: the lists do not match .dynstr"));
        }
        let le16 =
            |out: &mut [u8], at: usize, v: u16| out[at..at + 2].copy_from_slice(&v.to_le_bytes());
        let le32 =
            |out: &mut [u8], at: usize, v: u32| out[at..at + 4].copy_from_slice(&v.to_le_bytes());
        let mut at = 0usize;
        let mut other: u16 = first_other(self.verdefs);
        for (n, ((file, vs), offs)) in self.verneed.iter().zip(&d.versions).enumerate() {
            let Some(pos) = self.needed.iter().position(|f| f == file) else {
                return Err(internal(".gnu.version_r: a need of a file that is not needed"));
            };
            if offs.len() != vs.len() {
                return Err(internal(".gnu.version_r: the versions do not match .dynstr"));
            }
            let cnt = vs.len();
            let last_need = n + 1 == self.verneed.len();
            le16(out, at, 1); // vn_version
            le16(out, at + 2, cnt as u16);
            le32(out, at + 4, d.needed[pos]);
            le32(out, at + 8, 16); // vn_aux
            le32(out, at + 12, if last_need { 0 } else { 16 + 16 * cnt as u32 });
            at += 16;
            for (j, (&v, &name_off)) in vs.iter().zip(offs.iter()).enumerate() {
                let name = self.names.version(*file, v);
                let hash = elf_hash(name);
                if let Some(vd) = self.names.version_hash(*file, v)
                    && vd != hash
                {
                    return Err(SynthError::from(malformed::<Diagnostic>(format!(
                        "version {} of a needed shared object has vd_hash {vd:#x}, not the ELF \
                         hash of its name {hash:#x}",
                        String::from_utf8_lossy(name)
                    ))));
                }
                le32(out, at, hash);
                le16(out, at + 4, 0); // vna_flags
                le16(out, at + 6, other);
                le32(out, at + 8, name_off);
                le32(out, at + 12, if j + 1 == cnt { 0 } else { 16 });
                other += 1;
                at += 16;
            }
        }
        Ok(())
    }
}

impl SynthChunk for VerneedChunk<'_> {
    fn size(&self, _: &SizeCx<'_>) -> u64 {
        VerneedChunk::size(self)
    }

    fn division(&self) -> Division {
        Division::Whole { max_size: VerneedChunk::size(self) }
    }

    fn write(&self, _: &FillCx<'_>, range: Range<u64>, out: &mut [u8]) -> Result<(), SynthError> {
        whole_range(&range, out, VerneedChunk::size(self), |buf| self.write_all(buf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elf_hash_values() {
        assert_eq!(elf_hash(b""), 0);
        assert_eq!(elf_hash(b"GLIBC_2.2.5"), 0x0969_1a75);
        assert_eq!(elf_hash(b"GLIBC_2.17"), 0x0696_9197);
    }
}

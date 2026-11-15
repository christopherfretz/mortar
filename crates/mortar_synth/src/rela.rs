//! `.rela.dyn` (`spec/dyn.md` D-40): its size, its order and the encoding
//! of its records. The records themselves are the image's
//! (`mortar_image::dynrel`): their offsets and `RELATIVE` addends are
//! addresses.
//!
//! **Plan** ([`RelaPlan::new`], before addresses): the count of each part
//! from the scan's per-section counts (`Demands.dyn_relocs`, model §3.7),
//! the GOT's records ([`GotPlan::got_records`]) and the copies (one `COPY`
//! each, D-42): the size, and `DT_RELACOUNT`.
//!
//! **Order** (D-40): every `RELATIVE` first, by `r_offset`; then every
//! other record by (`.dynsym` index, `r_offset`). `IRELATIVE` is never in
//! `.rela.dyn` of an `Exec` or `Pie` (D-41: `.rela.plt`). The `RELATIVE`
//! part is the concatenation, in address order, of each unit's records
//! sorted by offset ([`RelaPlan::relative_starts`]: a unit's slice starts
//! at a prefix sum of the counts, so the units are filled in parallel and
//! the bytes do not depend on the number of threads); the other part is
//! small and sorted whole ([`sort_symbolic`]).

use mortar_arch::RelForm;
use mortar_core::SectionId;
use mortar_core::demands::{Demands, DynKind};
use mortar_elf::Class;

use crate::error::{SynthError, internal};
use crate::got::GotPlan;
use crate::out::Window;

/// One record of `.rela.dyn` (or `.rela.plt`), as it is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DynRecord {
    pub r_offset: u64,
    /// `r_sym`: the `.dynsym` index (0 for `RELATIVE`).
    pub r_sym: u32,
    pub r_type: u32,
    pub addend: i64,
}

/// A unit of the `RELATIVE` part: the records of one input section, or
/// the GOT's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RelUnit {
    Section(SectionId),
    Got,
}

/// The plan of `.rela.dyn`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelaPlan {
    class: Class,
    rel: RelForm,
    /// Per relocated section with records: its `RELATIVE` count and its
    /// other count, sorted by section.
    sections: Box<[(SectionId, u32, u32)]>,
    got_relative: u32,
    got_other: u32,
    copies: u32,
    relative: u64,
    other: u64,
}

impl RelaPlan {
    /// The plan from the scan's counts, the GOT plan and the number of
    /// copy allocations (`Slots.copies`). An `IRELATIVE` counted in a
    /// section of a dynamic output is an internal error (D-41).
    pub fn new(
        class: Class,
        rel: RelForm,
        demands: &Demands,
        got: &GotPlan,
        copies: usize,
    ) -> Result<Self, SynthError> {
        let mut sections: Vec<(SectionId, u32, u32)> = Vec::with_capacity(demands.dyn_relocs.len());
        let (mut relative, mut other) = (0u64, 0u64);
        for &(s, c) in &demands.dyn_relocs {
            if c.get(DynKind::IRelative) != 0 {
                return Err(internal(format!(
                    "section {} counts an IRELATIVE record for .rela.dyn (spec/dyn.md D-41)",
                    s.0
                )));
            }
            let r = c.get(DynKind::Relative);
            let o = c.get(DynKind::Symbolic).saturating_add(c.get(DynKind::Tls));
            relative += u64::from(r);
            other += u64::from(o);
            sections.push((s, r, o));
        }
        sections.sort_unstable_by_key(|row| row.0);
        if sections.windows(2).any(|w| w[0].0 == w[1].0) {
            return Err(internal("Demands.dyn_relocs names a section twice"));
        }
        let got_relative = got.got_relative();
        let got_other = got.got_records().len() as u32 - got_relative;
        let copies = u32::try_from(copies).map_err(|_| internal::<SynthError>("copies"))?;
        relative += u64::from(got_relative);
        other += u64::from(got_other) + u64::from(copies);
        Ok(Self {
            class,
            rel,
            sections: sections.into(),
            got_relative,
            got_other,
            copies,
            relative,
            other,
        })
    }

    /// The bytes of one record.
    #[must_use]
    pub fn entsize(&self) -> u64 {
        entsize(self.class, self.rel)
    }

    /// The size of `.rela.dyn`; 0 means no chunk.
    #[must_use]
    pub fn size(&self) -> u64 {
        (self.relative + self.other) * self.entsize()
    }

    /// `DT_RELACOUNT` (D-40): the `RELATIVE` records.
    #[must_use]
    pub fn relative_count(&self) -> u64 {
        self.relative
    }

    /// The records after the `RELATIVE` ones.
    #[must_use]
    pub fn other_count(&self) -> u64 {
        self.other
    }

    /// The `RELATIVE` count of `unit`.
    #[must_use]
    pub fn relative_of(&self, unit: RelUnit) -> u32 {
        match unit {
            RelUnit::Got => self.got_relative,
            RelUnit::Section(s) => self
                .sections
                .binary_search_by_key(&s, |row| row.0)
                .map_or(0, |at| self.sections[at].1),
        }
    }

    /// The other records' count of `unit` (for the GOT: its non-`RELATIVE`
    /// records; the copies are counted apart, [`Self::copies`]).
    #[must_use]
    pub fn other_of(&self, unit: RelUnit) -> u32 {
        match unit {
            RelUnit::Got => self.got_other,
            RelUnit::Section(s) => self
                .sections
                .binary_search_by_key(&s, |row| row.0)
                .map_or(0, |at| self.sections[at].2),
        }
    }

    /// The copy allocations' records.
    #[must_use]
    pub fn copies(&self) -> u32 {
        self.copies
    }

    /// The sections with records, by section.
    pub fn sections(&self) -> impl Iterator<Item = SectionId> + '_ {
        self.sections.iter().map(|row| row.0)
    }

    /// D-40 step 1: given every unit with `RELATIVE` records in address
    /// order (each section's and the GOT's place in the output), the
    /// index of each one's first record: the prefix sums of their counts.
    /// An error when the units do not cover the plan's count.
    pub fn relative_starts(&self, order: &[RelUnit]) -> Result<Vec<u64>, SynthError> {
        let mut at = 0u64;
        let mut out = Vec::with_capacity(order.len());
        for &u in order {
            out.push(at);
            at += u64::from(self.relative_of(u));
        }
        if at != self.relative {
            return Err(internal(format!(
                ".rela.dyn: the units in address order hold {at} RELATIVE records, the plan {}",
                self.relative
            )));
        }
        Ok(out)
    }
}

/// The bytes of one record of `class` and `form`.
#[must_use]
pub const fn entsize(class: Class, rel: RelForm) -> u64 {
    match (rel, class) {
        (RelForm::Rela, Class::Elf64) => 24,
        (RelForm::Rela, Class::Elf32) => 12,
        (RelForm::Rel, Class::Elf64) => 16,
        (RelForm::Rel, Class::Elf32) => 8,
    }
}

/// D-40 step 2's order: (`.dynsym` index, `r_offset`), then the rest of
/// the record (two records at one place against one key are not written).
pub fn sort_symbolic(records: &mut [DynRecord]) {
    records.sort_unstable_by_key(|r| (r.r_sym, r.r_offset, r.r_type, r.addend));
}

/// Encodes `r` at record index `index` of the window's section (records
/// outside the window are clipped by [`Window`]).
pub fn put(class: Class, rel: RelForm, out: &mut Window<'_>, index: u64, r: &DynRecord) {
    let e = entsize(class, rel);
    let off = index * e;
    if off + e <= out.start() || off >= out.end() {
        return;
    }
    let (w, shift) = match class {
        Class::Elf64 => (8u8, 32),
        Class::Elf32 => (4u8, 8),
    };
    let ww = u64::from(w);
    out.word(off, w, r.r_offset);
    out.word(off + ww, w, u64::from(r.r_sym) << shift | u64::from(r.r_type));
    if rel == RelForm::Rela {
        out.word(off + 2 * ww, w, r.addend.cast_unsigned());
    }
}

/// Encodes `records` as consecutive records into `buf` (whole records;
/// the parallel fill's per-unit slice).
pub fn encode(class: Class, rel: RelForm, records: &[DynRecord], buf: &mut [u8]) {
    let e = entsize(class, rel) as usize;
    let w = match class {
        Class::Elf64 => 8,
        Class::Elf32 => 4,
    };
    for (r, out) in records.iter().zip(buf.chunks_exact_mut(e)) {
        let info = match class {
            Class::Elf64 => u64::from(r.r_sym) << 32 | u64::from(r.r_type),
            Class::Elf32 => u64::from(r.r_sym) << 8 | u64::from(r.r_type),
        };
        let words = [r.r_offset, info, r.addend.cast_unsigned()];
        let n = if rel == RelForm::Rela { 3 } else { 2 };
        for (k, v) in words.iter().take(n).enumerate() {
            out[k * w..(k + 1) * w].copy_from_slice(&v.to_le_bytes()[..w]);
        }
    }
}

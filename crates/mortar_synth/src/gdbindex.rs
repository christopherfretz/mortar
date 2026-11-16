//! `.gdb_index` (`spec/synth.md` §17, S-G1 to S-G9; WP-097): gdb's name
//! and address index, version 7, complete or refused by name.
//!
//! The plan ([`GdbIndexPlan::new`]) is built in phase 80 from what
//! [`crate::gdbread`] reads: the CU list as (input section, offset,
//! length), the kept ranges as (input address, length), and the symbol
//! table and constant pool as bytes, which need no address (S-G7a). The
//! write fills the header, the CU list and the address area from the
//! layout and copies the rest.

use std::collections::HashMap;
use std::ops::Range;

use mortar_core::par::Pool;
use mortar_core::{FileId, SectionId};

use crate::eh::CodeRef;
use crate::error::{SynthError, internal};
use crate::gdbread::{self, File, ReadError, Source};
use crate::image::{Division, FillCx, SizeCx, SynthChunk, check};

/// What S-G3 does with a debug-bearing unit without a name set: the one
/// place this decision is made (supervisor's ruling, 2026-11-16: refuse;
/// the operator may later make it "warn and write no index", which is a
/// change of this constant, of [`incomplete`], of S-G3's text and of the
/// tests of the refusal).
pub const INCOMPLETE: Incomplete = Incomplete::Refuse;

/// The choices for S-G3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Incomplete {
    /// Refuse the link: `gdb-index-incomplete`.
    Refuse,
}

/// The version written (S-G1).
pub const VERSION: u32 = 7;

/// gdb's `mapped_index_string_hash` for versions 5 and later (S-G7).
#[must_use]
pub fn hash(name: &[u8]) -> u32 {
    name.iter().fold(0u32, |h, &c| {
        h.wrapping_mul(67).wrapping_add(u32::from(c.to_ascii_lowercase())).wrapping_sub(113)
    })
}

/// The number of slots for `n` names (S-G7): `max(1024, P)`, `P` the
/// smallest power of two strictly greater than `⌊4n/3⌋`. `None` when it
/// does not fit.
#[must_use]
pub fn table_size(n: u64) -> Option<u64> {
    let q = n.checked_mul(4)? / 3;
    let p = (q + 1).checked_next_power_of_two()?;
    Some(p.max(1024))
}

/// Inserts `hashes` (in name order) into a table of `size` slots (a power
/// of two) by gdb's probe sequence (S-G7): each slot's name number, or
/// `u32::MAX` when empty.
#[must_use]
pub fn place(hashes: &[u32], size: u64) -> Vec<u32> {
    let mut slots = vec![u32::MAX; size as usize];
    let mask = (size - 1) as u32;
    for (i, &h) in hashes.iter().enumerate() {
        let mut slot = h & mask;
        let step = (h.wrapping_mul(17) & mask) | 1;
        while slots[slot as usize] != u32::MAX {
            slot = (slot + step) & mask;
        }
        slots[slot as usize] = i as u32;
    }
    slots
}

/// One CU-list row: the unit's input section and its offset and length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexUnit {
    pub section: SectionId,
    pub offset: u64,
    pub length: u64,
}

/// One address-area row: the unit, the range's start in an input section
/// and its length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexRange {
    pub cu: u32,
    pub start: CodeRef,
    pub len: u64,
}

/// The plan of `.gdb_index` (S-G7a).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GdbIndexPlan {
    pub units: Vec<IndexUnit>,
    pub ranges: Vec<IndexRange>,
    /// The slot count `T`.
    pub slots: u64,
    /// The symbol table and the CU vectors, as written.
    pub table: Vec<u8>,
    /// The names, each with its NUL, in name order: the end of the pool.
    pub names: Vec<u8>,
}

/// A refusal of S-G (§17.6) or an input error, with the file it is about.
pub type PlanError = ReadError;

impl GdbIndexPlan {
    /// Reads every file of `src` and builds the plan (S-G1 to S-G7):
    /// `Ok(None)` when the output has no unit (S-G1).
    ///
    /// # Errors
    ///
    /// The first problem by file order (§17.6), S-G3's included.
    pub fn new(src: &Source<'_, '_>, pool: &Pool) -> Result<Option<Self>, PlanError> {
        let n = src.link.files() as usize;
        let files: Vec<Result<File<'_>, ReadError>> =
            pool.map(n, |f| gdbread::read_file(src, FileId(f as u32)));
        let mut read = Vec::with_capacity(n);
        for (f, r) in files.into_iter().enumerate() {
            let file = r?;
            check_sets(FileId(f as u32), &file)?;
            read.push(file);
        }
        Self::from_files(&read)
    }

    /// The plan of files already read (in file order) and checked.
    ///
    /// # Errors
    ///
    /// S-G3 and the limits of S-G4 and S-G7.
    pub fn from_files(files: &[File<'_>]) -> Result<Option<Self>, PlanError> {
        let mut units = Vec::new();
        let mut ranges = Vec::new();
        let mut names = Names::default();
        for (f, file) in files.iter().enumerate() {
            let fid = FileId(f as u32);
            // The sets of each unit: (pubnames, pubtypes) positions.
            let mut of: HashMap<(SectionId, u64), [Option<usize>; 2]> = HashMap::new();
            for (i, s) in file.sets.iter().enumerate() {
                of.entry(s.unit).or_default()[usize::from(s.types)] = Some(i);
            }
            for u in &file.units {
                let cu =
                    u32::try_from(units.len()).ok().filter(|&c| c < 1 << 24).ok_or_else(|| {
                        too_large(fid, "2^24 units or more: a CU index has 24 bits (S-G4)")
                    })?;
                let sets = of.remove(&(u.section, u.offset)).unwrap_or_default();
                if u.bearing && sets.iter().any(Option::is_none) {
                    return Err(incomplete(fid, file, u, sets[0].is_none()));
                }
                for set in sets.iter().flatten() {
                    let s = &file.sets[*set];
                    let bytes: &[u8] = &file.bytes[s.bytes as usize];
                    for e in &s.entries {
                        let name = &bytes[e.start as usize..(e.start + e.len) as usize];
                        names.add(name, cu | u32::from(e.attributes) << 24);
                    }
                }
                units.push(IndexUnit { section: u.section, offset: u.offset, length: u.length });
                let r = &file.ranges[u.first as usize..(u.first + u.count) as usize];
                ranges.extend(r.iter().map(|r| IndexRange { cu, start: r.start, len: r.len }));
            }
            // A set naming an offset that is no unit's start.
            if let Some((&(_, off), v)) = of.iter().min_by_key(|(k, _)| **k) {
                let s = &file.sets[v.iter().flatten().copied().min().unwrap_or(0)];
                return Err(ReadError::Refused {
                    name: gdbread::MALFORMED_SET,
                    file: fid,
                    at: Some((s.section, s.at)),
                    note: format!(
                        "a name set naming offset {off:#x} of .debug_info, where no unit starts"
                    ),
                });
            }
        }
        if units.is_empty() {
            return Ok(None);
        }
        let (table, slots) =
            names.table().ok_or_else(|| too_large(FileId(0), "the symbol table"))?;
        let plan = Self { units, ranges, slots, table, names: names.bytes };
        if plan.size() > u64::from(u32::MAX) {
            return Err(too_large(FileId(0), "a .gdb_index of 4 GiB or more (S-G7)"));
        }
        Ok(Some(plan))
    }

    /// The size (S-G7a).
    #[must_use]
    pub fn size(&self) -> u64 {
        self.area_end() + self.table.len() as u64 + self.names.len() as u64
    }

    /// The end of the address area: the symbol table's offset.
    fn area_end(&self) -> u64 {
        24 + 16 * self.units.len() as u64 + 20 * self.ranges.len() as u64
    }

    /// The whole chunk (S-G7).
    ///
    /// # Errors
    ///
    /// `internal-error` when the layout has no offset for a unit's section
    /// or no address for a kept range's start (the plan counted it), or
    /// the CU list is not in output order.
    pub fn bytes(&self, cx: &FillCx<'_>) -> Result<Vec<u8>, SynthError> {
        let mut out = Vec::with_capacity(self.size() as usize);
        let cu_list = 24u32;
        let tu_list = cu_list + 16 * self.units.len() as u32;
        let symtab = self.area_end() as u32;
        let pool = symtab + 8 * self.slots as u32;
        for v in [VERSION, cu_list, tu_list, tu_list, symtab, pool] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        let mut last: Option<u64> = None;
        for u in &self.units {
            let base = (cx.section_offset)(u.section).ok_or_else(|| {
                internal::<SynthError>("a .gdb_index unit's section has no offset")
            })?;
            let at = base + u.offset;
            if last.is_some_and(|l| at <= l) {
                return Err(internal("the .gdb_index CU list is not in output order"));
            }
            last = Some(at);
            out.extend_from_slice(&at.to_le_bytes());
            out.extend_from_slice(&u.length.to_le_bytes());
        }
        for r in &self.ranges {
            let low = (cx.code_addr)(r.start).ok_or_else(|| {
                internal::<SynthError>("a .gdb_index range's start has no address")
            })?;
            let high = low
                .checked_add(r.len)
                .ok_or_else(|| internal::<SynthError>("a .gdb_index range wraps"))?;
            out.extend_from_slice(&low.to_le_bytes());
            out.extend_from_slice(&high.to_le_bytes());
            out.extend_from_slice(&r.cu.to_le_bytes());
        }
        out.extend_from_slice(&self.table);
        out.extend_from_slice(&self.names);
        if out.len() as u64 != self.size() {
            return Err(internal("the .gdb_index bytes differ from its size"));
        }
        Ok(out)
    }
}

impl SynthChunk for GdbIndexPlan {
    fn size(&self, _: &SizeCx<'_>) -> u64 {
        GdbIndexPlan::size(self)
    }

    fn division(&self) -> Division {
        Division::Whole { max_size: GdbIndexPlan::size(self) }
    }

    fn write(&self, cx: &FillCx<'_>, range: Range<u64>, out: &mut [u8]) -> Result<(), SynthError> {
        check(&range, out, GdbIndexPlan::size(self))?;
        let all = self.bytes(cx)?;
        out.copy_from_slice(&all[range.start as usize..range.end as usize]);
        Ok(())
    }
}

/// The distinct names in first-occurrence order, held once, and their
/// `(name, value)` pairs (S-G4, S-G7).
#[derive(Debug, Default)]
struct Names {
    /// Every name with its NUL.
    bytes: Vec<u8>,
    /// Each name's start in `bytes`, and its length.
    at: Vec<(u32, u32)>,
    hashes: Vec<u32>,
    /// By a 64-bit hash of the bytes: the first name, then `next`.
    first: HashMap<u64, u32>,
    next: Vec<u32>,
    pairs: Vec<(u32, u32)>,
}

impl Names {
    fn add(&mut self, name: &[u8], value: u32) {
        let key = fnv(name);
        let mut i = self.first.get(&key).copied().unwrap_or(u32::MAX);
        while i != u32::MAX {
            let (s, n) = self.at[i as usize];
            if &self.bytes[s as usize..(s + n) as usize] == name {
                self.pairs.push((i, value));
                return;
            }
            i = self.next[i as usize];
        }
        let id = self.at.len() as u32;
        let start = self.bytes.len() as u32;
        self.bytes.extend_from_slice(name);
        self.bytes.push(0);
        self.at.push((start, name.len() as u32));
        self.hashes.push(hash(name));
        self.next.push(self.first.insert(key, id).unwrap_or(u32::MAX));
        self.pairs.push((id, value));
    }

    /// The symbol table and the CU vectors (S-G7), and the slot count.
    fn table(&mut self) -> Option<(Vec<u8>, u64)> {
        let n = self.at.len();
        let slots = table_size(n as u64)?;
        let mut pairs = std::mem::take(&mut self.pairs);
        pairs.sort_unstable();
        pairs.dedup();
        // Each name's vector offset from the pool's start.
        let mut vec_at = vec![0u32; n];
        let mut counts = vec![0u32; n];
        for &(id, _) in &pairs {
            counts[id as usize] += 1;
        }
        let mut off = 0u64;
        for (i, &c) in counts.iter().enumerate() {
            vec_at[i] = u32::try_from(off).ok()?;
            off += 4 + 4 * u64::from(c);
        }
        let vectors = off;
        let total = 8 * slots + vectors;
        let mut out = Vec::with_capacity(usize::try_from(total).ok()?);
        for &slot in &place(&self.hashes, slots) {
            let (name, vec) = if slot == u32::MAX {
                (0, 0)
            } else {
                let name = vectors + u64::from(self.at[slot as usize].0);
                (u32::try_from(name).ok()?, vec_at[slot as usize])
            };
            out.extend_from_slice(&name.to_le_bytes());
            out.extend_from_slice(&vec.to_le_bytes());
        }
        let mut p = 0;
        for (i, &c) in counts.iter().enumerate() {
            out.extend_from_slice(&c.to_le_bytes());
            for &(id, v) in &pairs[p..p + c as usize] {
                debug_assert_eq!(id as usize, i);
                out.extend_from_slice(&v.to_le_bytes());
            }
            p += c as usize;
        }
        Some((out, slots))
    }
}

/// FNV-1a, 64 bits: the names' identity map (not written anywhere).
fn fnv(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325, |h, &c| (h ^ u64::from(c)).wrapping_mul(0x100_0000_01b3))
}

fn too_large(file: FileId, what: &str) -> ReadError {
    ReadError::Refused { name: gdbread::TOO_LARGE, file, at: None, note: what.to_owned() }
}

/// S-G3's refusal of `unit` of `file` (see [`INCOMPLETE`]).
fn incomplete(fid: FileId, file: &File<'_>, unit: &gdbread::Unit, names: bool) -> ReadError {
    match INCOMPLETE {
        Incomplete::Refuse => {}
    }
    let missing = if names { ".debug_gnu_pubnames" } else { ".debug_gnu_pubtypes" };
    let mut note = format!(
        "the unit at {:#x} of its .debug_info has debug information and no {missing} set: \
         gdb would not find its names through the index; compile it with -ggnu-pubnames, or \
         link without --gdb-index (spec/synth.md S-G3)",
        unit.offset
    );
    if file.debug_names {
        note.push_str("; its .debug_names (-gpubnames) is not read (S-G4a)");
    }
    if file.old_pubnames {
        note.push_str("; its .debug_pubnames has no attribute byte and is not read (S-G4a)");
    }
    ReadError::Refused { name: gdbread::INCOMPLETE, file: fid, at: None, note }
}

/// Two sets of one kind naming one unit (S-G4a).
fn check_sets(fid: FileId, file: &File<'_>) -> Result<(), ReadError> {
    let mut seen: HashMap<(SectionId, u64, bool), usize> = HashMap::new();
    for (i, s) in file.sets.iter().enumerate() {
        if seen.insert((s.unit.0, s.unit.1, s.types), i).is_some() {
            return Err(ReadError::Refused {
                name: gdbread::MALFORMED_SET,
                file: fid,
                at: Some((s.section, s.at)),
                note: format!("a second set naming the unit at {:#x} of .debug_info", s.unit.1),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // S-G7: gdb's hash (values computed with gdb 16.3's formula by hand
    // and checked against `gdb-add-index`'s table in tests/gdb_add_index.rs).
    #[test]
    fn the_hash_is_gdbs() {
        assert_eq!(hash(b""), 0);
        // 'a' = 97: 97 - 113 = -16.
        assert_eq!(hash(b"a"), 97u32.wrapping_sub(113));
        assert_eq!(hash(b"A"), hash(b"a"));
        assert_eq!(hash(b"ab"), (97u32.wrapping_sub(113)).wrapping_mul(67) + 98 - 113);
        // Only ASCII is folded.
        assert_ne!(hash(&[0xc4]), hash(&[0xe4]));
    }

    #[test]
    fn the_size_rule() {
        assert_eq!(table_size(0), Some(1024));
        assert_eq!(table_size(767), Some(1024));
        // ⌊768 × 4 / 3⌋ = 1024: the next power above it is 2048.
        assert_eq!(table_size(768), Some(2048));
        assert_eq!(table_size(1535), Some(2048));
        assert_eq!(table_size(1536), Some(4096));
    }

    #[test]
    fn probing_skips_taken_slots() {
        // Equal hashes: the second goes `step` further.
        let h = 5u32;
        let slots = place(&[h, h], 1024);
        let step = ((h * 17) & 1023) | 1;
        assert_eq!(slots[5], 0);
        assert_eq!(slots[(5 + step) as usize], 1);
        assert_eq!(slots.iter().filter(|&&s| s != u32::MAX).count(), 2);
    }

    #[test]
    fn vectors_are_sorted_and_unique() {
        let mut n = Names::default();
        n.add(b"x", 2 | 0x30 << 24);
        n.add(b"y", 1);
        n.add(b"x", 1 | 0x30 << 24);
        n.add(b"x", 2 | 0x30 << 24);
        let (t, slots) = n.table().unwrap();
        assert_eq!(slots, 1024);
        let pool = &t[8 * 1024..];
        let w = |i: usize| u32::from_le_bytes(pool[4 * i..4 * i + 4].try_into().unwrap());
        // x: 2 values; y: 1.
        assert_eq!((w(0), w(1), w(2)), (2, 1 | 0x30 << 24, 2 | 0x30 << 24));
        assert_eq!((w(3), w(4)), (1, 1));
        assert_eq!(n.bytes, b"x\0y\0");
        // x's slot names offset 20 (after 5 words of vectors) and vector 0.
        let s = (hash(b"x") & 1023) as usize;
        assert_eq!(&t[8 * s..8 * s + 8], &[20, 0, 0, 0, 0, 0, 0, 0]);
    }
}

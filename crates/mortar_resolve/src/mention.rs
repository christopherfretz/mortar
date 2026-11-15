//! What one mention is for resolution (§2, §3.1, §3.4, §1.2): its class,
//! and what is wrong with it.
//!
//! A fault is reported only for a live file (a lazy member that stays
//! lazy takes no part in the link). The class of a faulty mention is
//! still defined, so that `first`/`cfirst` (§4.1), which read every file,
//! stay a function of the inputs: a link with a fault fails anyway, and
//! the best-effort class only decides which further diagnostics it shows.

use std::sync::atomic::{AtomicU8, AtomicU16, AtomicU32, AtomicU64, Ordering::Relaxed};

use mortar_core::SymEntry;
use mortar_elf::SymbolSection;
use mortar_elf::consts::{
    STB_GLOBAL, STB_GNU_UNIQUE, STB_WEAK, STT_COMMON, STT_FILE, STT_FUNC, STT_GNU_IFUNC,
    STT_NOTYPE, STT_OBJECT, STT_SECTION, STT_TLS, STV_DEFAULT, STV_HIDDEN, STV_INTERNAL,
    STV_PROTECTED,
};

use crate::problem::{MalformedSymbol, What};
use crate::view::Mention;

/// The class of a mention (§3.1, §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// `SHN_UNDEF`; `weak` for `STB_WEAK`.
    Undefined { weak: bool },
    /// Defined, `STB_GLOBAL` or `STB_GNU_UNIQUE`.
    Strong,
    /// `SHN_COMMON`, `STB_GLOBAL`.
    Common,
    /// Defined, `STB_WEAK`.
    Weak,
}

impl Class {
    /// Whether the mention defines its key (defined or common: what
    /// `first(K)` counts, §4.1).
    #[must_use]
    pub const fn is_candidate(self) -> bool {
        !matches!(self, Self::Undefined { .. })
    }
}

/// Whether `m` is a data definition (§2): defined (not common), and not
/// `STT_FUNC` or `STT_GNU_IFUNC`.
#[must_use]
pub fn is_data(m: &Mention) -> bool {
    matches!(m.place, SymbolSection::Section(_) | SymbolSection::Absolute)
        && !matches!(m.sym_type, STT_FUNC | STT_GNU_IFUNC)
}

/// The class of `m`, and its fault if it has one.
#[must_use]
pub fn classify(m: &Mention, entry: SymEntry) -> (Class, Option<What>) {
    let weak = m.binding == STB_WEAK;
    let mut fault = match m.binding {
        STB_GLOBAL | STB_WEAK | STB_GNU_UNIQUE => None,
        binding => Some(What::SymbolBinding { entry, binding }),
    };
    let type_fault = match m.sym_type {
        STT_NOTYPE | STT_OBJECT | STT_FUNC | STT_COMMON | STT_TLS | STT_GNU_IFUNC => None,
        STT_SECTION | STT_FILE => {
            Some(What::Malformed { entry, why: MalformedSymbol::NonLocalSectionOrFile })
        }
        sym_type => Some(What::SymbolType { entry, sym_type }),
    };
    fault = fault.or(type_fault);
    let class = match m.place {
        SymbolSection::Undefined => {
            if m.binding == STB_GNU_UNIQUE {
                fault = fault
                    .or(Some(What::Malformed { entry, why: MalformedSymbol::UndefinedUnique }));
            }
            Class::Undefined { weak }
        }
        SymbolSection::Common => {
            if weak {
                fault = fault.or(Some(What::WeakCommon { entry }));
            } else if m.value != 0 && !m.value.is_power_of_two() {
                // Alignment 0 is alignment 1 (GNU ld and mold; lld refuses).
                fault = fault.or(Some(What::Malformed {
                    entry,
                    why: MalformedSymbol::CommonAlignment(m.value),
                }));
            }
            Class::Common
        }
        SymbolSection::Absolute | SymbolSection::Section(_) => {
            if weak {
                Class::Weak
            } else {
                Class::Strong
            }
        }
        SymbolSection::Reserved(shndx) => {
            // SHN_X86_64_LCOMMON (M4) and every other reserved index: the
            // mention contributes nothing.
            fault = fault.or(Some(What::ReservedSection { entry, shndx }));
            Class::Undefined { weak: true }
        }
    };
    (class, fault)
}

/// The constraint rank of a visibility (§3.5): smaller is more
/// constraining; `STV_INTERNAL` is treated as `STV_HIDDEN`.
#[must_use]
pub const fn vis_rank(visibility: u8) -> u8 {
    match visibility {
        STV_INTERNAL | STV_HIDDEN => 0,
        STV_PROTECTED => 1,
        _ => 2,
    }
}

/// The visibility of a constraint rank: hidden, protected, default.
#[must_use]
pub const fn rank_vis(rank: u8) -> u8 {
    match rank {
        0 => STV_HIDDEN,
        1 => STV_PROTECTED,
        _ => STV_DEFAULT,
    }
}

/// `log2` of a common's alignment, a power of two (0 when it is not, and
/// for alignment 0, which is alignment 1).
#[must_use]
pub const fn align_log2(value: u64) -> u8 {
    if value.is_power_of_two() { value.trailing_zeros() as u8 } else { 0 }
}

// ---- atomics (spec/model.md §6 rule 2) -------------------------------------

/// `n` atomics holding `value`.
pub(crate) fn atomics_u64(n: usize, value: u64) -> Vec<AtomicU64> {
    (0..n).map(|_| AtomicU64::new(value)).collect()
}

pub(crate) fn atomics_u32(n: usize, value: u32) -> Vec<AtomicU32> {
    (0..n).map(|_| AtomicU32::new(value)).collect()
}

pub(crate) fn atomics_u16(n: usize) -> Vec<AtomicU16> {
    (0..n).map(|_| AtomicU16::new(0)).collect()
}

pub(crate) fn atomics_u8(n: usize, value: u8) -> Vec<AtomicU8> {
    (0..n).map(|_| AtomicU8::new(value)).collect()
}

/// An atomic minimum that skips the write when it would not lower the
/// cell (most updates do not, and a load does not take the cache line).
#[inline]
pub(crate) fn min_u64(cell: &AtomicU64, value: u64) {
    if value < cell.load(Relaxed) {
        cell.fetch_min(value, Relaxed);
    }
}

#[inline]
pub(crate) fn min_u32(cell: &AtomicU32, value: u32) {
    if value < cell.load(Relaxed) {
        cell.fetch_min(value, Relaxed);
    }
}

#[inline]
pub(crate) fn max_u64(cell: &AtomicU64, value: u64) {
    if value > cell.load(Relaxed) {
        cell.fetch_max(value, Relaxed);
    }
}

#[inline]
pub(crate) fn min_u8(cell: &AtomicU8, value: u8) {
    if value < cell.load(Relaxed) {
        cell.fetch_min(value, Relaxed);
    }
}

#[inline]
pub(crate) fn max_u8(cell: &AtomicU8, value: u8) {
    if value > cell.load(Relaxed) {
        cell.fetch_max(value, Relaxed);
    }
}

/// Sets `bits` in `cell`; returns the bits that were set before.
#[inline]
pub(crate) fn or_u16(cell: &AtomicU16, bits: u16) -> u16 {
    let old = cell.load(Relaxed);
    if old & bits == bits { old } else { cell.fetch_or(bits, Relaxed) }
}

/// A bitset of atomics.
#[derive(Debug)]
pub(crate) struct Bits(Vec<AtomicU64>);

impl Bits {
    pub(crate) fn new(n: usize) -> Self {
        Self(atomics_u64(n.div_ceil(64), 0))
    }

    /// Sets bit `i`; whether this call set it.
    #[inline]
    pub(crate) fn set(&self, i: usize) -> bool {
        let mask = 1u64 << (i % 64);
        let cell = &self.0[i / 64];
        if cell.load(Relaxed) & mask != 0 {
            return false;
        }
        cell.fetch_or(mask, Relaxed) & mask == 0
    }

    #[inline]
    pub(crate) fn get(&self, i: usize) -> bool {
        self.0[i / 64].load(Relaxed) & (1u64 << (i % 64)) != 0
    }
}

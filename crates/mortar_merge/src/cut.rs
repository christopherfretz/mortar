//! Which `Merge` sections are split, and their pieces (`spec/synth.md`
//! §3.1, §3.2): S-5 to S-12.

use mortar_elf::consts::{SHF_EXECINSTR, SHF_TLS, SHF_WRITE};

use crate::problem::What;

/// The largest entry size that is merged (S-6).
pub const MAX_ENTSIZE: u64 = 1 << 16;

/// Why a `Merge` section is placed whole instead of split (S-6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Whole {
    /// `SHF_WRITE` or `SHF_EXECINSTR`.
    WritableOrCode,
    /// The target of a relocation section.
    Relocated,
    /// `SHF_TLS`.
    Tls,
    /// `sh_addralign` not a power of two, or `sh_entsize` above 2^16.
    Shape,
    /// `SHF_COMPRESSED` (a non-alloc `.debug_str` from `-gz`): its bytes
    /// are not the pieces' until decompressed, which the store does not do
    /// yet; placing whole is always correct (S-5).
    Compressed,
}

/// S-6: whether a section of fate `Merge` (an `SHF_MERGE` section with
/// `sh_entsize > 0`, resolve.md §10) is placed whole, and why. `align` is
/// `sh_addralign` as written (0 and 1 both mean none); `relocated` whether
/// a relocation section applies to it.
#[must_use]
pub fn placed_whole(flags: u64, align: u64, entsize: u64, relocated: bool) -> Option<Whole> {
    if flags & (SHF_WRITE | SHF_EXECINSTR) != 0 {
        Some(Whole::WritableOrCode)
    } else if relocated {
        Some(Whole::Relocated)
    } else if flags & SHF_TLS != 0 {
        Some(Whole::Tls)
    } else if !(align == 0 || align.is_power_of_two()) || entsize == 0 || entsize > MAX_ENTSIZE {
        Some(Whole::Shape)
    } else {
        None
    }
}

/// log2 of a section's alignment (`sh_addralign` 0 is 1). The caller has
/// checked it is a power of two (S-6).
#[must_use]
pub fn align_log2(align: u64) -> u8 {
    align.max(1).trailing_zeros() as u8
}

/// The mask with the high bit of every byte set (zero-byte search).
const HIGHS: u64 = 0x8080_8080_8080_8080;
const LOWS: u64 = 0x0101_0101_0101_0101;

/// The position of the first zero byte of `bytes` at or after `from`. Reads
/// eight bytes at a time (a word has a zero byte exactly when
/// `(w - 0x01..01) & !w & 0x80..80` is not zero; the lowest set bit is the
/// first zero byte, the borrow only reaching bytes above it).
#[must_use]
pub fn next_zero(bytes: &[u8], from: usize) -> Option<usize> {
    let mut at = from;
    let tail = bytes.get(from..)?;
    let mut words = tail.chunks_exact(8);
    for w in &mut words {
        let x = u64::from_le_bytes(w.try_into().unwrap_or([1; 8]));
        let z = x.wrapping_sub(LOWS) & !x & HIGHS;
        if z != 0 {
            return Some(at + (z.trailing_zeros() / 8) as usize);
        }
        at += 8;
    }
    words.remainder().iter().position(|&b| b == 0).map(|p| at + p)
}

/// Cuts a split `Merge` section into pieces: pushes each piece's offset to
/// `out` (S-9, S-10), after the checks of S-7 and S-8. Returns the problem
/// and the offset it is at when the section is malformed; nothing is
/// pushed then.
///
/// - S-7: the size is a multiple of `entsize`;
/// - S-9: a string piece is a maximal run of `entsize`-byte characters
///   ending with the first all-zero character, compared at multiples of
///   `entsize` from the start;
/// - S-8: the last character of a string section is zero;
/// - S-10: a constant piece is each `entsize`-byte entry.
pub fn cut(
    bytes: &[u8],
    strings: bool,
    entsize: usize,
    out: &mut Vec<u32>,
) -> Result<(), (What, u64)> {
    let size = bytes.len();
    debug_assert!(entsize > 0, "cut: entsize 0 is not split (S-5)");
    if !size.is_multiple_of(entsize) {
        return Err((What::NotMultipleOfEntsize, 0));
    }
    if u32::try_from(size).is_err() {
        return Err((What::SectionTooLarge, 0));
    }
    if size == 0 {
        return Ok(());
    }
    if !strings {
        out.extend((0..size).step_by(entsize).map(|o| o as u32));
        return Ok(());
    }
    if bytes[size - entsize..].iter().any(|&b| b != 0) {
        // The start of the last, unterminated, string.
        let mut start = size - entsize;
        while start >= entsize && bytes[start - entsize..start].iter().any(|&b| b != 0) {
            start -= entsize;
        }
        return Err((What::Unterminated, start as u64));
    }
    let first = out.len();
    if entsize == 1 {
        let mut start = 0;
        while start < size {
            out.push(start as u32);
            // The last byte is zero, so a terminator is always found.
            start = next_zero(bytes, start).map_or(size, |z| z + 1);
        }
    } else {
        out.push(0);
        for end in (entsize..size).step_by(entsize) {
            if bytes[end - entsize..end].iter().all(|&b| b == 0) {
                out.push(end as u32);
            }
        }
    }
    debug_assert!(out.len() > first, "a non-empty string section has a piece");
    Ok(())
}

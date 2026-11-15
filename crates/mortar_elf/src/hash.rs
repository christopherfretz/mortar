//! The number of dynamic symbols as the hash tables give it (`spec/elf.md`
//! §6.6): the only statement of the count in a file without section
//! headers.

use crate::raw::Class;

fn word(bytes: &[u8], index: u64) -> Option<u32> {
    let at = usize::try_from(index.checked_mul(4)?).ok()?;
    Some(u32::from_le_bytes(*bytes.get(at..)?.first_chunk::<4>()?))
}

/// The symbol count of a `DT_HASH` table (gABI "Hash Table"): `nchain`.
/// `bytes` run from the table to the end of its segment's file contents;
/// `None` when the header and the buckets and chains it states do not fit
/// in them. The words are 4 bytes in both classes.
pub(crate) fn sysv_count(bytes: &[u8]) -> Option<u32> {
    let nbucket = word(bytes, 0)?;
    let nchain = word(bytes, 1)?;
    let words = 2 + u64::from(nbucket) + u64::from(nchain);
    if words.checked_mul(4)? > bytes.len() as u64 {
        return None;
    }
    Some(nchain)
}

/// The symbol count of a `DT_GNU_HASH` table (the GNU hash table as glibc
/// and binutils define it): one past the last symbol of the chain that
/// starts at the highest bucket, or `symoffset` when every bucket is
/// empty. `None` when the table does not fit in `bytes`, a bucket is below
/// `symoffset`, or the last chain does not end within `bytes`. The bloom
/// filter words are 4 bytes in `ELFCLASS32` and 8 in `ELFCLASS64`; the
/// other words are 4 bytes in both.
pub(crate) fn gnu_count(bytes: &[u8], class: Class) -> Option<u32> {
    let nbuckets = u64::from(word(bytes, 0)?);
    let symoffset = word(bytes, 1)?;
    let bloom_size = u64::from(word(bytes, 2)?);
    let bloom_words = bloom_size.checked_mul(class.word_size() as u64 / 4)?;
    let buckets_at = 4u64.checked_add(bloom_words)?;
    let chains_at = buckets_at.checked_add(nbuckets)?;
    if chains_at.checked_mul(4)? > bytes.len() as u64 {
        return None;
    }
    let mut last = 0u32;
    for bucket in 0..nbuckets {
        let value = word(bytes, buckets_at + bucket)?;
        if value != 0 && value < symoffset {
            return None;
        }
        last = last.max(value);
    }
    if last == 0 {
        return Some(symoffset);
    }
    // Walk the last chain to the entry whose low bit marks its end; the
    // walk ends within `bytes`, so it is bounded by the file.
    let mut symbol = last;
    loop {
        let entry = word(bytes, chains_at + u64::from(symbol - symoffset))?;
        if entry & 1 != 0 {
            return symbol.checked_add(1);
        }
        symbol = symbol.checked_add(1)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    /// §6.6: `nchain`, when the stated table fits.
    #[test]
    fn s6_6_sysv_count_is_nchain() {
        assert_eq!(sysv_count(&words(&[1, 3, 0, 0, 1, 2])), Some(3));
        assert_eq!(sysv_count(&words(&[1, 3, 0, 0, 1])), None);
        assert_eq!(sysv_count(&words(&[u32::MAX, u32::MAX])), None);
        assert_eq!(sysv_count(&words(&[0])), None);
    }

    /// §6.6: the end of the last chain; `symoffset` when every bucket is
    /// empty; buckets below `symoffset` and chains that run off the end
    /// are refused.
    #[test]
    fn s6_6_gnu_count_is_the_end_of_the_last_chain() {
        // nbuckets 2, symoffset 3, bloom 1 (one 4-byte word in ELF32),
        // shift 6; buckets 3 and 5; chains for symbols 3..=6.
        let table = words(&[2, 3, 1, 6, 0, 3, 5, 10, 11, 20, 21]);
        assert_eq!(gnu_count(&table, Class::Elf32), Some(7));
        // In ELF64 the bloom word is 8 bytes: the same words read
        // differently, and here the chain does not end.
        assert_eq!(gnu_count(&table, Class::Elf64), None);
        assert_eq!(gnu_count(&words(&[1, 4, 1, 6, 0, 0]), Class::Elf32), Some(4));
        assert_eq!(gnu_count(&words(&[1, 4, 1, 6, 0, 2, 1]), Class::Elf32), None);
        assert_eq!(gnu_count(&words(&[1, 1, 1, 6, 0, 1, 0, 0]), Class::Elf32), None);
        assert_eq!(gnu_count(&words(&[u32::MAX, 1, u32::MAX, 6]), Class::Elf64), None);
        // A chain that ends at the largest index has no count.
        let mut last = words(&[1, u32::MAX, 1, 6, 0, u32::MAX, 1]);
        assert_eq!(gnu_count(&last, Class::Elf32), None);
        last.truncate(24);
        assert_eq!(gnu_count(&last, Class::Elf32), None);
    }
}

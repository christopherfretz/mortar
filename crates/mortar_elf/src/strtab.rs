//! String tables (`spec/elf.md` §3.3).

use crate::error::Error;

/// A string table: NUL-terminated byte strings addressed by offset (gABI
/// "String Table"). `Copy`, and borrows only the input bytes.
#[derive(Debug, Clone, Copy)]
pub struct StrTab<'a> {
    bytes: &'a [u8],
    section: u32,
}

impl<'a> StrTab<'a> {
    /// A table over `bytes`. `section` is the index errors name: the
    /// section the bytes are the contents of, or 0 for bytes that are not
    /// a section's.
    pub fn new(bytes: &'a [u8], section: u32) -> Self {
        StrTab { bytes, section }
    }

    /// The table of a file without one: it holds only the empty string, at
    /// offset 0.
    pub fn empty() -> Self {
        StrTab { bytes: &[], section: 0 }
    }

    /// The table's bytes.
    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    /// The index of the section the table is the contents of (0 if none).
    pub fn section(&self) -> u32 {
        self.section
    }

    /// The string at `offset`, without its terminator.
    ///
    /// An offset at or past the end of the table is
    /// [`Error::StringOutOfBounds`], except offset 0 of an empty table,
    /// which is the empty string; a string that reaches the end of the
    /// table without a NUL is [`Error::StringNotTerminated`].
    #[inline]
    pub fn get(&self, offset: u32) -> Result<&'a [u8], Error> {
        let Some(rest) = self.bytes.get(offset as usize..).filter(|rest| !rest.is_empty()) else {
            if offset == 0 {
                return Ok(&[]);
            }
            return Err(Error::StringOutOfBounds {
                section: self.section,
                offset,
                size: self.bytes.len() as u64,
            });
        };
        match nul_position(rest) {
            Some(end) => Ok(&rest[..end]),
            None => Err(Error::StringNotTerminated { section: self.section, offset }),
        }
    }
}

/// The index of the first zero byte, eight bytes at a time.
#[inline]
fn nul_position(bytes: &[u8]) -> Option<usize> {
    const LOW: u64 = 0x0101_0101_0101_0101;
    const HIGH: u64 = 0x8080_8080_8080_8080;
    let (words, tail) = bytes.as_chunks::<8>();
    for (i, word) in words.iter().enumerate() {
        let w = u64::from_le_bytes(*word);
        // The lowest set bit marks the first zero byte: a borrow can only
        // set the high bit of a byte above a zero byte.
        let zeros = w.wrapping_sub(LOW) & !w & HIGH;
        if zeros != 0 {
            return Some(i * 8 + (zeros.trailing_zeros() / 8) as usize);
        }
    }
    tail.iter().position(|&b| b == 0).map(|p| words.len() * 8 + p)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §3.3: the string at an offset ends at the first NUL at or after it.
    #[test]
    fn s3_3_strings_end_at_the_first_nul() {
        let table = StrTab::new(b"\0.text\0.data\0", 5);
        assert_eq!(table.get(0), Ok(&b""[..]));
        assert_eq!(table.get(1), Ok(&b".text"[..]));
        assert_eq!(table.get(3), Ok(&b"ext"[..]));
        assert_eq!(table.get(6), Ok(&b""[..]));
        assert_eq!(table.get(7), Ok(&b".data"[..]));
        assert_eq!(table.get(12), Ok(&b""[..]));
    }

    /// §3.3: offsets past the end and unterminated strings are errors that
    /// name the section and the offset; an empty table holds the empty
    /// string at offset 0.
    #[test]
    fn s3_3_bad_offsets_are_errors() {
        let table = StrTab::new(b"\0ab\0cd", 9);
        assert_eq!(table.get(6), Err(Error::StringOutOfBounds { section: 9, offset: 6, size: 6 }));
        assert_eq!(
            table.get(u32::MAX),
            Err(Error::StringOutOfBounds { section: 9, offset: u32::MAX, size: 6 })
        );
        assert_eq!(table.get(4), Err(Error::StringNotTerminated { section: 9, offset: 4 }));
        assert_eq!(table.get(5), Err(Error::StringNotTerminated { section: 9, offset: 5 }));
        assert_eq!(StrTab::empty().get(0), Ok(&b""[..]));
        assert_eq!(
            StrTab::empty().get(1),
            Err(Error::StringOutOfBounds { section: 0, offset: 1, size: 0 })
        );
    }

    /// §3.3: the word-at-a-time search finds the same terminator as a
    /// byte-at-a-time one, for every position and length around the word
    /// size, including bytes with the high bit set next to the NUL.
    #[test]
    fn s3_3_nul_search_agrees_with_a_byte_loop() {
        for len in 0..40usize {
            for nul in 0..=len {
                for fill in [b'a', 0x80, 0xff, 0x01] {
                    let mut bytes = vec![fill; len];
                    if nul < len {
                        bytes[nul] = 0;
                    }
                    assert_eq!(
                        nul_position(&bytes),
                        bytes.iter().position(|&b| b == 0),
                        "len {len} nul {nul} fill {fill:#x}"
                    );
                }
            }
        }
    }
}

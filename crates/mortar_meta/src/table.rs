//! The `.note.mason` table (`spec/odr.md` O-50 to O-52): the encoder a
//! dynamic output's table is written with, and the reader of a shared
//! object's table (O-24). The table is its own format, not a container
//! (`.mason.meta`): one note, owner `MasonMeta`, type
//! [`NT_MASON_ODR`], whose descriptor is a header and fixed rows.
//!
//! The reader never panics: whatever the bytes, it returns the table or a
//! [`TableError`] that names what is wrong (`meta-malformed`, O-51).

use mortar_elf::SharedObject;
use mortar_elf::consts::PT_NOTE;

/// The section's name (O-52).
pub const SECTION_NAME: &[u8] = b".note.mason";
/// The section's and the segment's alignment (O-52).
pub const ALIGN: u64 = 8;
/// The note's owner, with its NUL (`n_namesz` 10, O-52).
pub const OWNER: &[u8] = b"MasonMeta\0";
/// `n_type` (`NT_MASON_ODR`, O-52).
pub const NT_MASON_ODR: u32 = 0x4d4f_4401;
/// The descriptor's offset in the note under both note layouts (O-52).
pub const DESC_OFFSET: usize = 24;
/// The table version this encoder writes and this reader reads (O-51).
pub const VERSION: u32 = 1;
/// The header's size (O-51).
pub const HEADER_SIZE: usize = 16;
/// A row's size (O-51).
pub const ROW_SIZE: u32 = 24;
/// Row flag bit 15 (O-51): the module binds its own copy.
pub const NONPREEMPTIBLE: u16 = 1 << 15;

/// One row (O-51).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Row {
    /// FNV-1a 64 of the `.dynsym` name (MT-13, O-51).
    pub key: u64,
    /// The first 8 bytes of the keeper's kind 2 hash, as stored.
    pub hash: [u8; 8],
    /// The symbol's `.dynsym` index.
    pub dynsym: u32,
    /// O-15's flags, plus [`NONPREEMPTIBLE`].
    pub flags: u16,
}

/// The note's bytes (O-52) for `rows`, sorted here by (key, dynsym) so the
/// bytes do not depend on the order given. Little-endian: every target of
/// mortar is.
#[must_use]
pub fn note(rows: &[Row]) -> Vec<u8> {
    let mut rows = rows.to_vec();
    rows.sort_unstable_by_key(|r| (r.key, r.dynsym));
    let count = u32::try_from(rows.len()).unwrap_or(u32::MAX);
    let desc = HEADER_SIZE + rows.len() * ROW_SIZE as usize;
    let mut out = Vec::with_capacity(DESC_OFFSET + desc);
    out.extend_from_slice(&(OWNER.len() as u32).to_le_bytes());
    out.extend_from_slice(&u32::try_from(desc).unwrap_or(u32::MAX).to_le_bytes());
    out.extend_from_slice(&NT_MASON_ODR.to_le_bytes());
    out.extend_from_slice(OWNER);
    out.resize(DESC_OFFSET, 0);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&ROW_SIZE.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for r in &rows {
        out.extend_from_slice(&r.key.to_le_bytes());
        out.extend_from_slice(&r.hash);
        out.extend_from_slice(&r.dynsym.to_le_bytes());
        out.extend_from_slice(&r.flags.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
    }
    out
}

/// What makes a table malformed (O-51).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableError {
    /// The descriptor is shorter than the header.
    Short { size: usize },
    /// A version this reader does not read.
    Version(u32),
    /// A row size other than 24.
    RowSize(u32),
    /// More rows than the descriptor holds.
    Count { count: u32, room: usize },
    /// Rows not sorted by key.
    Unsorted { row: u32 },
    /// A second `MasonMeta` table note.
    Twice,
    /// A row's `.dynsym` index outside `.dynsym` (the caller's check).
    Dynsym { row: u32, index: u32 },
}

impl TableError {
    /// The reason, as `meta-malformed` writes it.
    #[must_use]
    pub fn reason(&self) -> String {
        match self {
            Self::Short { size } => {
                format!("the .note.mason table's descriptor has {size} bytes, less than its header")
            }
            Self::Version(v) => format!("the .note.mason table has version {v}, not 1"),
            Self::RowSize(s) => format!("the .note.mason table's rows are {s} bytes, not 24"),
            Self::Count { count, room } => {
                format!("the .note.mason table counts {count} rows and its descriptor holds {room}")
            }
            Self::Unsorted { row } => {
                format!("the .note.mason table's row {row} is out of key order")
            }
            Self::Twice => "two .note.mason tables".to_owned(),
            Self::Dynsym { row, index } => format!(
                "the .note.mason table's row {row} names .dynsym entry {index}, which does not exist"
            ),
        }
    }
}

/// A table, borrowed from its descriptor.
#[derive(Debug, Clone, Copy)]
pub struct Table<'a> {
    rows: &'a [u8],
    count: u32,
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    let mut w = [0; 4];
    if let Some(s) = b.get(at..at + 4) {
        w.copy_from_slice(s);
    }
    u32::from_le_bytes(w)
}

impl<'a> Table<'a> {
    /// Parses a descriptor (O-51's header and rows).
    ///
    /// # Errors
    ///
    /// What is malformed.
    pub fn parse(desc: &'a [u8]) -> Result<Self, TableError> {
        if desc.len() < HEADER_SIZE {
            return Err(TableError::Short { size: desc.len() });
        }
        let version = u32_at(desc, 0);
        if version != VERSION {
            return Err(TableError::Version(version));
        }
        let count = u32_at(desc, 4);
        let row_size = u32_at(desc, 8);
        if row_size != ROW_SIZE {
            return Err(TableError::RowSize(row_size));
        }
        let room = (desc.len() - HEADER_SIZE) / ROW_SIZE as usize;
        if count as usize > room {
            return Err(TableError::Count { count, room });
        }
        let end = HEADER_SIZE + count as usize * ROW_SIZE as usize;
        let t = Self { rows: &desc[HEADER_SIZE..end], count };
        let mut last = 0;
        for i in 0..count {
            let k = t.get(i).map_or(0, |r| r.key);
            if k < last {
                return Err(TableError::Unsorted { row: i });
            }
            last = k;
        }
        Ok(t)
    }

    /// The number of rows.
    #[must_use]
    pub fn len(&self) -> u32 {
        self.count
    }

    /// Whether there is no row.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Row `i`.
    #[must_use]
    pub fn get(&self, i: u32) -> Option<Row> {
        let at = i as usize * ROW_SIZE as usize;
        let b = self.rows.get(at..at + ROW_SIZE as usize)?;
        let mut key = [0; 8];
        key.copy_from_slice(&b[0..8]);
        let mut hash = [0; 8];
        hash.copy_from_slice(&b[8..16]);
        Some(Row {
            key: u64::from_le_bytes(key),
            hash,
            dynsym: u32_at(b, 16),
            flags: u16::from_le_bytes([b[20], b[21]]),
        })
    }

    /// The rows, in key order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = Row> + '_ {
        (0..self.count).map(|i| self.get(i).unwrap_or_default())
    }
}

/// The table of a shared object (O-52): the `MasonMeta` note of type
/// [`NT_MASON_ODR`] in any of its `PT_NOTE` segments; `None` without one.
/// A `PT_NOTE` that does not parse is not this format's and is passed
/// over; a second table is malformed.
///
/// # Errors
///
/// The table is malformed.
pub fn find<'a>(so: &SharedObject<'a>) -> Result<Option<Table<'a>>, TableError> {
    let mut found = None;
    for segment in so.program_headers().iter().filter(|s| s.p_type() == PT_NOTE) {
        let Ok(notes) = segment.notes() else { continue };
        for note in notes {
            let Ok(note) = note else { break };
            if note.n_type != NT_MASON_ODR || note.name_bytes() != OWNER {
                continue;
            }
            if found.is_some() {
                return Err(TableError::Twice);
            }
            found = Some(Table::parse(note.desc())?);
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(key: u64, dynsym: u32) -> Row {
        Row { key, hash: [key as u8; 8], dynsym, flags: 1 }
    }

    // odr.md O-51, O-52: the encoding, sorted, read back.
    #[test]
    fn encodes_and_reads_back() {
        let rows = [row(9, 3), row(2, 7), row(9, 1)];
        let n = note(&rows);
        assert_eq!(&n[0..4], &10u32.to_le_bytes());
        assert_eq!(&n[4..8], &(16u32 + 72).to_le_bytes());
        assert_eq!(&n[8..12], &NT_MASON_ODR.to_le_bytes());
        assert_eq!(&n[12..22], OWNER);
        assert_eq!(n.len(), DESC_OFFSET + 16 + 72);
        assert_eq!(n.len() % 8, 0);
        let t = Table::parse(&n[DESC_OFFSET..]).unwrap();
        let got: Vec<Row> = t.iter().collect();
        assert_eq!(got, [row(2, 7), row(9, 1), row(9, 3)]);
        // Deterministic: any order of the same rows, the same bytes.
        assert_eq!(note(&[rows[2], rows[0], rows[1]]), n);
        // An empty table is a header.
        assert_eq!(Table::parse(&note(&[])[DESC_OFFSET..]).unwrap().len(), 0);
    }

    // odr.md O-51: every truncation and every single-byte change of a
    // table is a table or a named error, never a panic.
    #[test]
    fn mutations_never_panic() {
        let n = note(&[row(1, 1), row(5, 2), row(5, 3)]);
        let desc = &n[DESC_OFFSET..];
        for len in 0..desc.len() {
            let r = Table::parse(&desc[..len]);
            if len < HEADER_SIZE {
                assert_eq!(r.unwrap_err(), TableError::Short { size: len });
            } else {
                assert!(matches!(r, Err(TableError::Count { .. })), "{len}");
            }
        }
        for at in 0..desc.len() {
            for v in [0u8, 1, 0x7f, 0x80, 0xff] {
                let mut m = desc.to_vec();
                m[at] = v;
                if let Ok(t) = Table::parse(&m) {
                    assert!(t.iter().count() == t.len() as usize);
                } else {
                    let e = Table::parse(&m).unwrap_err();
                    assert!(!e.reason().is_empty());
                }
            }
        }
        assert_eq!(
            Table::parse(&[2, 0, 0, 0, 0, 0, 0, 0, 24, 0, 0, 0, 0, 0, 0, 0]).unwrap_err(),
            TableError::Version(2)
        );
        let mut m = desc.to_vec();
        m[8] = 16;
        assert_eq!(Table::parse(&m).unwrap_err(), TableError::RowSize(16));
        let mut m = desc.to_vec();
        m[HEADER_SIZE] = 9; // row 0's key above row 1's
        assert_eq!(Table::parse(&m).unwrap_err(), TableError::Unsorted { row: 1 });
    }
}

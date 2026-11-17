//! The `.note.mason` table (`spec/odr.md` O-50 to O-52): the encoder a
//! dynamic output's table is written with, and the reader of a shared
//! object's table (O-24). The table is its own format, not a container
//! (`.mason.meta`): one note, owner `MasonMeta`, type
//! [`NT_MASON_ODR`], whose descriptor is a header and fixed rows.
//!
//! `--odr-table=full` (WP-076) adds a second note of the same owner, type
//! [`NT_MASON_ODR_FULL`], after the first in the same section: the hidden
//! vague definitions and the kind 3 types ([`full_note`], [`FullTable`]).
//! A reader of the first note ([`find`], O-24) passes it over.
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
/// `n_type` of `=full`'s second note (`NT_MASON_ODR_FULL`, O-52; WP-076).
pub const NT_MASON_ODR_FULL: u32 = 0x4d4f_4402;
/// A row's size in `=full`'s note (O-51).
pub const FULL_ROW_SIZE: u32 = 32;

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

/// One row of `=full`'s note (O-51, WP-076): a hidden vague definition
/// (`layout`, `size` and `align` zero) or a kind 3 type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FullRow {
    /// MT-13's key: FNV-1a 64 of the symbol's name (a definition) or of
    /// the `<type>` encoding (a type).
    pub key: u64,
    /// The first 8 bytes of the row's hash (kind 2 or kind 3), as stored.
    pub hash: [u8; 8],
    /// O-18's layout hash (a type); zero for a definition.
    pub layout: u64,
    /// O-17's size (a type); zero for a definition.
    pub size: u32,
    /// O-17's alignment (a type); zero for a definition.
    pub align: u16,
    /// O-15's flags (a definition) or O-17's (a type).
    pub flags: u16,
}

/// `=full`'s note (O-52, WP-076): owner `MasonMeta`, type
/// [`NT_MASON_ODR_FULL`]; descriptor `{u32 version = 1; u32 definitions;
/// u32 row_size = 32; u32 types}`, then the definitions' rows and the
/// types' rows, each array sorted (by key, then the rest of the row) here,
/// so the bytes do not depend on the order given.
#[must_use]
pub fn full_note(definitions: &[FullRow], types: &[FullRow]) -> Vec<u8> {
    let mut defs = definitions.to_vec();
    defs.sort_unstable();
    let mut types = types.to_vec();
    types.sort_unstable();
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    let desc = HEADER_SIZE + (defs.len() + types.len()) * FULL_ROW_SIZE as usize;
    let mut out = Vec::with_capacity(DESC_OFFSET + desc);
    out.extend_from_slice(&(OWNER.len() as u32).to_le_bytes());
    out.extend_from_slice(&u32::try_from(desc).unwrap_or(u32::MAX).to_le_bytes());
    out.extend_from_slice(&NT_MASON_ODR_FULL.to_le_bytes());
    out.extend_from_slice(OWNER);
    out.resize(DESC_OFFSET, 0);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&count(defs.len()).to_le_bytes());
    out.extend_from_slice(&FULL_ROW_SIZE.to_le_bytes());
    out.extend_from_slice(&count(types.len()).to_le_bytes());
    for r in defs.iter().chain(&types) {
        out.extend_from_slice(&r.key.to_le_bytes());
        out.extend_from_slice(&r.hash);
        out.extend_from_slice(&r.layout.to_le_bytes());
        out.extend_from_slice(&r.size.to_le_bytes());
        out.extend_from_slice(&r.align.to_le_bytes());
        out.extend_from_slice(&r.flags.to_le_bytes());
    }
    out
}

/// `n_type` of the runtime note (`NT_MASON_ODR_RT`, O-52, O-53a; WP-198),
/// the third note of `.note.mason`, written by `--odr-runtime=report|fatal`.
pub const NT_MASON_ODR_RT: u32 = 0x4d4f_4403;
/// The runtime note's mode word for `report` (O-53a).
pub const RT_MODE_REPORT: u32 = 1;
/// The runtime note's mode word for `fatal` (O-53a).
pub const RT_MODE_FATAL: u32 = 2;

/// The runtime note (O-52, O-53a; WP-198): owner `MasonMeta`, type
/// [`NT_MASON_ODR_RT`], descriptor `{u32 version = 1; u32 mode; u32 flags
/// = 0; u32 runtime id}`; 40 bytes, a multiple of 8, so a note after it
/// stays aligned under both note layouts.
#[must_use]
pub fn runtime_note(mode: u32, runtime_id: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(DESC_OFFSET + 16);
    out.extend_from_slice(&(OWNER.len() as u32).to_le_bytes());
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&NT_MASON_ODR_RT.to_le_bytes());
    out.extend_from_slice(OWNER);
    out.resize(DESC_OFFSET, 0);
    for word in [VERSION, mode, 0, runtime_id] {
        out.extend_from_slice(&word.to_le_bytes());
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
    /// A row size other than 32 in `=full`'s note.
    FullRowSize(u32),
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
            Self::FullRowSize(s) => {
                format!("the .note.mason full table's rows are {s} bytes, not 32")
            }
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

/// `=full`'s note (WP-076), borrowed from its descriptor.
#[derive(Debug, Clone, Copy)]
pub struct FullTable<'a> {
    rows: &'a [u8],
    definitions: u32,
    types: u32,
}

impl<'a> FullTable<'a> {
    /// Parses `=full`'s descriptor: its header, then its definitions and
    /// its types, each sorted by key.
    ///
    /// # Errors
    ///
    /// What is malformed: the reasons of [`Table::parse`], the row size
    /// being [`TableError::FullRowSize`]; the count is that of both arrays,
    /// and an unsorted row is numbered from the first definition.
    pub fn parse(desc: &'a [u8]) -> Result<Self, TableError> {
        if desc.len() < HEADER_SIZE {
            return Err(TableError::Short { size: desc.len() });
        }
        let version = u32_at(desc, 0);
        if version != VERSION {
            return Err(TableError::Version(version));
        }
        let definitions = u32_at(desc, 4);
        let row_size = u32_at(desc, 8);
        if row_size != FULL_ROW_SIZE {
            return Err(TableError::FullRowSize(row_size));
        }
        let types = u32_at(desc, 12);
        let room = (desc.len() - HEADER_SIZE) / FULL_ROW_SIZE as usize;
        let total = definitions as usize + types as usize;
        if total > room {
            return Err(TableError::Count {
                count: u32::try_from(total).unwrap_or(u32::MAX),
                room,
            });
        }
        let end = HEADER_SIZE + total * FULL_ROW_SIZE as usize;
        let t = Self { rows: &desc[HEADER_SIZE..end], definitions, types };
        for (first, n) in [(0, definitions), (definitions, types)] {
            let mut last = 0;
            for i in first..first + n {
                let k = t.row(i).map_or(0, |r| r.key);
                if k < last {
                    return Err(TableError::Unsorted { row: i });
                }
                last = k;
            }
        }
        Ok(t)
    }

    fn row(&self, i: u32) -> Option<FullRow> {
        let at = i as usize * FULL_ROW_SIZE as usize;
        let b = self.rows.get(at..at + FULL_ROW_SIZE as usize)?;
        let u64_at = |o: usize| {
            let mut w = [0; 8];
            w.copy_from_slice(&b[o..o + 8]);
            u64::from_le_bytes(w)
        };
        let mut hash = [0; 8];
        hash.copy_from_slice(&b[8..16]);
        Some(FullRow {
            key: u64_at(0),
            hash,
            layout: u64_at(16),
            size: u32_at(b, 24),
            align: u16::from_le_bytes([b[28], b[29]]),
            flags: u16::from_le_bytes([b[30], b[31]]),
        })
    }

    /// The hidden vague definitions, in key order.
    pub fn definitions(&self) -> impl ExactSizeIterator<Item = FullRow> + '_ {
        (0..self.definitions).map(|i| self.row(i).unwrap_or_default())
    }

    /// The types, in key order.
    pub fn types(&self) -> impl ExactSizeIterator<Item = FullRow> + '_ {
        let d = self.definitions;
        (0..self.types).map(move |i| self.row(d + i).unwrap_or_default())
    }
}

/// `=full`'s note of a shared object (WP-076): the `MasonMeta` note of
/// type [`NT_MASON_ODR_FULL`] in any of its `PT_NOTE` segments, as
/// [`find`] reads the first.
///
/// # Errors
///
/// The note is malformed, or there are two.
pub fn find_full<'a>(so: &SharedObject<'a>) -> Result<Option<FullTable<'a>>, TableError> {
    let mut found = None;
    for segment in so.program_headers().iter().filter(|s| s.p_type() == PT_NOTE) {
        let Ok(notes) = segment.notes() else { continue };
        for note in notes {
            let Ok(note) = note else { break };
            if note.n_type != NT_MASON_ODR_FULL || note.name_bytes() != OWNER {
                continue;
            }
            if found.is_some() {
                return Err(TableError::Twice);
            }
            found = Some(FullTable::parse(note.desc())?);
        }
    }
    Ok(found)
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
    // spec: odr.md §10.2 (WP-198): the runtime's note, 40 bytes: namesz
    // 10, descsz 16, type 0x4d4f4403, "MasonMeta", the descriptor at 24.
    #[test]
    fn runtime_note_bytes() {
        let n = runtime_note(RT_MODE_FATAL, 3);
        assert_eq!(n.len(), 40);
        let w = |o: usize| u32::from_le_bytes(n[o..o + 4].try_into().unwrap());
        assert_eq!((w(0), w(4), w(8)), (10, 16, 0x4d4f_4403));
        assert_eq!(&n[12..24], b"MasonMeta\0\0\0");
        assert_eq!((w(24), w(28), w(32), w(36)), (1, 2, 0, 3));
    }

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

    fn full(key: u64, layout: u64) -> FullRow {
        FullRow { key, hash: [key as u8; 8], layout, size: 8, align: 4, flags: 1 }
    }

    // odr.md O-51, O-52 (WP-076): =full's note, sorted per array, read
    // back; deterministic in the rows' order.
    #[test]
    fn full_encodes_and_reads_back() {
        let defs = [
            FullRow { key: 7, hash: [7; 8], flags: 1, ..FullRow::default() },
            FullRow { key: 3, hash: [3; 8], flags: 9, ..FullRow::default() },
        ];
        let types = [full(5, 2), full(1, 9), full(5, 1)];
        let n = full_note(&defs, &types);
        assert_eq!(&n[0..4], &10u32.to_le_bytes());
        assert_eq!(&n[4..8], &(16u32 + 5 * 32).to_le_bytes());
        assert_eq!(&n[8..12], &NT_MASON_ODR_FULL.to_le_bytes());
        assert_eq!(&n[12..22], OWNER);
        assert_eq!(n.len() % 8, 0);
        let t = FullTable::parse(&n[DESC_OFFSET..]).unwrap();
        assert_eq!(t.definitions().map(|r| r.key).collect::<Vec<_>>(), [3, 7]);
        assert_eq!(t.types().collect::<Vec<_>>(), [full(1, 9), full(5, 1), full(5, 2)]);
        assert_eq!(full_note(&[defs[1], defs[0]], &[types[2], types[0], types[1]]), n);
        // The first note's reader does not take it for a table.
        assert_eq!(Table::parse(&n[DESC_OFFSET..]).unwrap_err(), TableError::RowSize(32));
        let empty = full_note(&[], &[]);
        let t = FullTable::parse(&empty[DESC_OFFSET..]).unwrap();
        assert_eq!((t.definitions().len(), t.types().len()), (0, 0));
    }

    // odr.md O-51 (WP-076): every truncation and every single-byte change
    // of =full's note is a table or a named error, never a panic.
    #[test]
    fn full_mutations_never_panic() {
        let defs = [FullRow { key: 2, hash: [2; 8], flags: 1, ..FullRow::default() }];
        let n = full_note(&defs, &[full(1, 1), full(4, 4)]);
        let desc = &n[DESC_OFFSET..];
        for len in 0..desc.len() {
            let r = FullTable::parse(&desc[..len]);
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
                match FullTable::parse(&m) {
                    Ok(t) => {
                        let rows = t.definitions().count() + t.types().count();
                        assert!(rows * 32 + HEADER_SIZE <= m.len());
                    }
                    Err(e) => assert!(!e.reason().is_empty()),
                }
            }
        }
        let mut m = desc.to_vec();
        m[8] = 24;
        assert_eq!(FullTable::parse(&m).unwrap_err(), TableError::FullRowSize(24));
        // The definitions and the types are each sorted; the types may
        // start below the last definition.
        let mut m = desc.to_vec();
        m[HEADER_SIZE + 32] = 9; // type 0's key above type 1's
        assert_eq!(FullTable::parse(&m).unwrap_err(), TableError::Unsorted { row: 2 });
        // A count that overflows u32 when summed is a count error.
        let mut m = desc.to_vec();
        m[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        m[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(FullTable::parse(&m), Err(TableError::Count { .. })));
    }
}

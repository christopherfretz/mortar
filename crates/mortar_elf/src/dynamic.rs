//! The dynamic table (`spec/elf.md` §6.4): every entry as a widened
//! record, and typed accessors for the tags a linker reads.

use core::iter::FusedIterator;

use crate::consts::{
    DT_FLAGS, DT_FLAGS_1, DT_NEEDED, DT_NULL, DT_RPATH, DT_RUNPATH, DT_SONAME, DT_STRTAB,
};
use crate::error::Error;
use crate::raw::{Dyn, Table, TableIter};
use crate::strtab::StrTab;

/// The dynamic table of a shared object: the entries up to and including
/// the first `DT_NULL`, and the string table its string-valued tags are
/// offsets into. `Copy`, and borrows only the input bytes.
#[derive(Debug, Clone, Copy)]
pub struct DynamicTable<'a> {
    entries: Table<'a, Dyn>,
    /// `None` when the table has no string table (no `DT_STRTAB`).
    strings: Result<Option<StrTab<'a>>, Error>,
    offset: u64,
}

impl<'a> DynamicTable<'a> {
    /// The table over the records in `records`, found at file offset
    /// `offset`: the entries up to the first `DT_NULL`, which is part of
    /// the table. A table without `DT_NULL` is an error.
    pub(crate) fn new(
        records: Table<'a, Dyn>,
        strings: Result<Option<StrTab<'a>>, Error>,
        offset: u64,
    ) -> Result<Self, Error> {
        let Some(null) = records.iter().position(|entry| entry.d_tag == DT_NULL) else {
            return Err(Error::DynamicNotTerminated { entries: records.len() as u64 });
        };
        let entries = records.slice(0, null + 1).unwrap_or(records);
        Ok(DynamicTable { entries, strings, offset })
    }

    /// The number of entries, the terminating `DT_NULL` included.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The file offset of the table.
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// The entries as a table, the terminating `DT_NULL` included.
    pub fn entries(&self) -> Table<'a, Dyn> {
        self.entries
    }

    /// The entries in order, the terminating `DT_NULL` included.
    pub fn iter(&self) -> TableIter<'a, Dyn> {
        self.entries.iter()
    }

    /// The string table the string-valued tags (`DT_NEEDED`, `DT_SONAME`,
    /// `DT_RPATH`, `DT_RUNPATH`, ...) are offsets into, or `None` when the
    /// table has none.
    pub fn strings(&self) -> Result<Option<StrTab<'a>>, Error> {
        self.strings
    }

    /// The value of the one entry with tag `d_tag`, or `None` when there is
    /// none. A tag that appears twice is an error: for the tags that may
    /// repeat (`DT_NEEDED`, `DT_AUXILIARY`, `DT_FILTER`), use
    /// [`DynamicTable::values`].
    pub fn value(&self, d_tag: i64) -> Result<Option<u64>, Error> {
        let mut found: Option<(u32, u64)> = None;
        for (index, entry) in (0u32..).zip(self.entries.iter()) {
            if entry.d_tag == d_tag {
                if let Some((first, _)) = found {
                    return Err(Error::DuplicateDynamicTag { d_tag, first, second: index });
                }
                found = Some((index, entry.d_val));
            }
        }
        Ok(found.map(|(_, value)| value))
    }

    /// The values of every entry with tag `d_tag`, in order.
    pub fn values(&self, d_tag: i64) -> impl Iterator<Item = u64> + use<'a> {
        self.entries.iter().filter(move |entry| entry.d_tag == d_tag).map(|entry| entry.d_val)
    }

    /// The string at `offset` of the dynamic string table, for an entry of
    /// tag `d_tag`.
    pub fn string_at(&self, d_tag: i64, offset: u64) -> Result<&'a [u8], Error> {
        let Some(strings) = self.strings? else {
            return Err(Error::MissingDynamicTag { d_tag: DT_STRTAB, needed_by: d_tag });
        };
        match u32::try_from(offset) {
            Ok(offset) => strings.get(offset),
            Err(_) => Err(Error::StringOutOfBounds {
                section: strings.section(),
                offset: u32::MAX,
                size: strings.bytes().len() as u64,
            }),
        }
    }

    /// The string of the one entry with the string-valued tag `d_tag`, or
    /// `None` when there is none.
    pub fn string(&self, d_tag: i64) -> Result<Option<&'a [u8]>, Error> {
        match self.value(d_tag)? {
            Some(offset) => self.string_at(d_tag, offset).map(Some),
            None => Ok(None),
        }
    }

    /// `DT_SONAME`: the name the object is known by.
    pub fn soname(&self) -> Result<Option<&'a [u8]>, Error> {
        self.string(DT_SONAME)
    }

    /// `DT_RPATH`, the search path (deprecated by `DT_RUNPATH`).
    pub fn rpath(&self) -> Result<Option<&'a [u8]>, Error> {
        self.string(DT_RPATH)
    }

    /// `DT_RUNPATH`, the search path.
    pub fn runpath(&self) -> Result<Option<&'a [u8]>, Error> {
        self.string(DT_RUNPATH)
    }

    /// `DT_NEEDED`: the names of the objects this one needs, in table
    /// order. A name that cannot be read is an error item; iteration goes
    /// on after it.
    pub fn needed(&self) -> Needed<'a> {
        Needed { entries: self.entries.iter(), table: *self }
    }

    /// `DT_FLAGS` (`DF_*`), 0 when the table has none.
    pub fn flags(&self) -> Result<u64, Error> {
        Ok(self.value(DT_FLAGS)?.unwrap_or(0))
    }

    /// `DT_FLAGS_1` (`DF_1_*`), 0 when the table has none.
    pub fn flags_1(&self) -> Result<u64, Error> {
        Ok(self.value(DT_FLAGS_1)?.unwrap_or(0))
    }
}

impl<'a> IntoIterator for &DynamicTable<'a> {
    type Item = Dyn;
    type IntoIter = TableIter<'a, Dyn>;

    fn into_iter(self) -> TableIter<'a, Dyn> {
        self.iter()
    }
}

/// The `DT_NEEDED` names of a [`DynamicTable`] ([`DynamicTable::needed`]).
#[derive(Debug, Clone)]
pub struct Needed<'a> {
    entries: TableIter<'a, Dyn>,
    table: DynamicTable<'a>,
}

impl<'a> Iterator for Needed<'a> {
    type Item = Result<&'a [u8], Error>;

    fn next(&mut self) -> Option<Self::Item> {
        let entry = self.entries.find(|entry| entry.d_tag == DT_NEEDED)?;
        Some(self.table.string_at(DT_NEEDED, entry.d_val))
    }
}

impl FusedIterator for Needed<'_> {}

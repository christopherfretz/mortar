//! Symbol versions (`spec/elf.md` §6.7; the LSB "Symbol Versioning"
//! chapter): the version table, the definition and need chains, and which
//! version a dynamic symbol carries.

use core::iter::FusedIterator;

use crate::consts::{
    VER_DEF_CURRENT, VER_FLG_BASE, VER_FLG_WEAK, VER_NDX_GLOBAL, VER_NDX_LOCAL, VER_NEED_CURRENT,
};
use crate::error::{Error, VersionFault, VersionRecord};
use crate::raw::{Record, Table, Verdaux, Verdef, Vernaux, Verneed, Versym};
use crate::strtab::StrTab;
use crate::symbol::SymbolIndex;

/// The bytes a chain of version records is in, where they are in the file,
/// how many records the file says it holds, and the string table of their
/// names.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Chain<'a> {
    /// From the first record to the end of the section, or of the
    /// segment's file contents.
    pub(crate) bytes: &'a [u8],
    /// The file offset of `bytes[0]`.
    pub(crate) base: u64,
    /// `sh_info`, or `DT_VERDEFNUM` / `DT_VERNEEDNUM`.
    pub(crate) count: u64,
    pub(crate) strings: StrTab<'a>,
}

impl<'a> Chain<'a> {
    fn fault(&self, record: VersionRecord, at: usize, fault: VersionFault) -> Error {
        Error::BadVersion { record, offset: self.base.saturating_add(at as u64), fault }
    }

    /// Record `T` at `at`, or the error that it does not fit.
    fn record<T: Record>(&self, record: VersionRecord, at: usize) -> Result<T, Error> {
        self.bytes
            .get(at..)
            .and_then(T::decode32)
            .ok_or_else(|| self.fault(record, at, VersionFault::Truncated))
    }

    /// Checks that `count` records of `size` bytes can be in the bytes from
    /// `at` on: no walk visits more records than the bytes can hold.
    fn fits(&self, record: VersionRecord, at: usize, count: u64, size: usize) -> Result<(), Error> {
        let room = self.bytes.len().saturating_sub(at) as u64 / size as u64;
        if count > room {
            return Err(self.fault(record, at, VersionFault::TooMany(count)));
        }
        Ok(())
    }

    fn name(&self, offset: u32) -> Result<&'a [u8], Error> {
        self.strings.get(offset)
    }
}

/// A walk of `count` records linked by next-offsets from `at`.
#[derive(Debug, Clone)]
struct Walk {
    at: usize,
    left: u64,
    failed: bool,
}

impl Walk {
    /// Moves past the record just read, whose next-offset is `next`. An
    /// offset that overflows leads past the end of the table, where the
    /// next read fails.
    fn advance(&mut self, next: u32) {
        self.left -= 1;
        self.at = self.at.saturating_add(next as usize);
    }
}

/// The version definitions of a shared object, in chain order
/// ([`crate::SharedObject::version_definitions`]). An item is a definition
/// or the error that ends the iteration.
#[derive(Debug, Clone)]
pub struct VersionDefinitions<'a> {
    chain: Chain<'a>,
    walk: Walk,
}

impl<'a> VersionDefinitions<'a> {
    pub(crate) fn new(chain: Chain<'a>) -> Result<Self, Error> {
        chain.fits(VersionRecord::Definition, 0, chain.count, Verdef::SIZE32)?;
        Ok(VersionDefinitions { chain, walk: Walk { at: 0, left: chain.count, failed: false } })
    }

    /// The number of definitions the file says there are.
    pub fn declared(&self) -> u64 {
        self.chain.count
    }
}

impl<'a> Iterator for VersionDefinitions<'a> {
    type Item = Result<VersionDefinition<'a>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.walk.left == 0 || self.walk.failed {
            return None;
        }
        let at = self.walk.at;
        let record: Verdef = match self.chain.record(VersionRecord::Definition, at) {
            Ok(record) => record,
            Err(error) => {
                self.walk.failed = true;
                return Some(Err(error));
            }
        };
        let fault = if record.vd_version != VER_DEF_CURRENT {
            Some(VersionFault::Revision(record.vd_version))
        } else if self.walk.left > 1 && record.vd_next == 0 {
            Some(VersionFault::ChainEnds)
        } else {
            None
        };
        if let Some(fault) = fault {
            self.walk.failed = true;
            return Some(Err(self.chain.fault(VersionRecord::Definition, at, fault)));
        }
        self.walk.advance(record.vd_next);
        Some(Ok(VersionDefinition { record, at, chain: self.chain }))
    }
}

impl FusedIterator for VersionDefinitions<'_> {}

/// One version definition. `Copy`, and borrows only the input bytes.
#[derive(Debug, Clone, Copy)]
pub struct VersionDefinition<'a> {
    record: Verdef,
    at: usize,
    chain: Chain<'a>,
}

impl<'a> VersionDefinition<'a> {
    /// The record as stored.
    pub fn record(&self) -> &Verdef {
        &self.record
    }

    /// The record's file offset.
    pub fn offset(&self) -> u64 {
        self.chain.base.saturating_add(self.at as u64)
    }

    /// `vd_ndx`: the version index symbols of this version carry.
    pub fn index(&self) -> u16 {
        self.record.vd_ndx
    }

    /// `vd_flags` (`VER_FLG_*`).
    pub fn flags(&self) -> u16 {
        self.record.vd_flags
    }

    /// `VER_FLG_BASE`: the definition of the file itself, whose name is its
    /// `DT_SONAME`.
    pub fn is_base(&self) -> bool {
        self.record.vd_flags & VER_FLG_BASE != 0
    }

    /// `VER_FLG_WEAK`.
    pub fn is_weak(&self) -> bool {
        self.record.vd_flags & VER_FLG_WEAK != 0
    }

    /// The version's name: its first `Verdaux`. A definition without one
    /// is an error.
    pub fn name(&self) -> Result<&'a [u8], Error> {
        match self.names()?.next() {
            Some(name) => name,
            None => Err(self.chain.fault(VersionRecord::Definition, self.at, VersionFault::NoName)),
        }
    }

    /// The `vd_cnt` names in chain order: the version's own, then the
    /// versions it inherits from. An item is a name or the error that ends
    /// the iteration.
    pub fn names(&self) -> Result<VersionNames<'a>, Error> {
        let start = self.at.checked_add(self.record.vd_aux as usize);
        let Some(start) = start.filter(|start| *start <= self.chain.bytes.len()) else {
            return Err(self.chain.fault(
                VersionRecord::DefinitionName,
                self.at.saturating_add(self.record.vd_aux as usize),
                VersionFault::Truncated,
            ));
        };
        let count = u64::from(self.record.vd_cnt);
        self.chain.fits(VersionRecord::DefinitionName, start, count, Verdaux::SIZE32)?;
        Ok(VersionNames { chain: self.chain, walk: Walk { at: start, left: count, failed: false } })
    }
}

/// The names of a [`VersionDefinition`].
#[derive(Debug, Clone)]
pub struct VersionNames<'a> {
    chain: Chain<'a>,
    walk: Walk,
}

impl<'a> Iterator for VersionNames<'a> {
    type Item = Result<&'a [u8], Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.walk.left == 0 || self.walk.failed {
            return None;
        }
        let at = self.walk.at;
        let record: Verdaux = match self.chain.record(VersionRecord::DefinitionName, at) {
            Ok(record) => record,
            Err(error) => {
                self.walk.failed = true;
                return Some(Err(error));
            }
        };
        if self.walk.left > 1 && record.vda_next == 0 {
            self.walk.failed = true;
            return Some(Err(self.chain.fault(
                VersionRecord::DefinitionName,
                at,
                VersionFault::ChainEnds,
            )));
        }
        self.walk.advance(record.vda_next);
        Some(self.chain.name(record.vda_name))
    }
}

impl FusedIterator for VersionNames<'_> {}

/// The version needs of a shared object, one per file, in chain order
/// ([`crate::SharedObject::version_needs`]). An item is a need or the
/// error that ends the iteration.
#[derive(Debug, Clone)]
pub struct VersionNeeds<'a> {
    chain: Chain<'a>,
    walk: Walk,
}

impl<'a> VersionNeeds<'a> {
    pub(crate) fn new(chain: Chain<'a>) -> Result<Self, Error> {
        chain.fits(VersionRecord::Need, 0, chain.count, Verneed::SIZE32)?;
        Ok(VersionNeeds { chain, walk: Walk { at: 0, left: chain.count, failed: false } })
    }

    /// The number of files the file says it needs versions of.
    pub fn declared(&self) -> u64 {
        self.chain.count
    }
}

impl<'a> Iterator for VersionNeeds<'a> {
    type Item = Result<VersionNeed<'a>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.walk.left == 0 || self.walk.failed {
            return None;
        }
        let at = self.walk.at;
        let record: Verneed = match self.chain.record(VersionRecord::Need, at) {
            Ok(record) => record,
            Err(error) => {
                self.walk.failed = true;
                return Some(Err(error));
            }
        };
        let fault = if record.vn_version != VER_NEED_CURRENT {
            Some(VersionFault::Revision(record.vn_version))
        } else if self.walk.left > 1 && record.vn_next == 0 {
            Some(VersionFault::ChainEnds)
        } else {
            None
        };
        if let Some(fault) = fault {
            self.walk.failed = true;
            return Some(Err(self.chain.fault(VersionRecord::Need, at, fault)));
        }
        self.walk.advance(record.vn_next);
        Some(Ok(VersionNeed { record, at, chain: self.chain }))
    }
}

impl FusedIterator for VersionNeeds<'_> {}

/// The versions needed from one file. `Copy`, and borrows only the input
/// bytes.
#[derive(Debug, Clone, Copy)]
pub struct VersionNeed<'a> {
    record: Verneed,
    at: usize,
    chain: Chain<'a>,
}

impl<'a> VersionNeed<'a> {
    /// The record as stored.
    pub fn record(&self) -> &Verneed {
        &self.record
    }

    /// The record's file offset.
    pub fn offset(&self) -> u64 {
        self.chain.base.saturating_add(self.at as u64)
    }

    /// `vn_file`: the name of the file (its `DT_NEEDED` name).
    pub fn file(&self) -> Result<&'a [u8], Error> {
        self.chain.name(self.record.vn_file)
    }

    /// The `vn_cnt` versions needed from the file, in chain order. An item
    /// is an entry or the error that ends the iteration.
    pub fn entries(&self) -> Result<VersionNeedEntries<'a>, Error> {
        let start = self.at.checked_add(self.record.vn_aux as usize);
        let Some(start) = start.filter(|start| *start <= self.chain.bytes.len()) else {
            return Err(self.chain.fault(
                VersionRecord::NeedEntry,
                self.at.saturating_add(self.record.vn_aux as usize),
                VersionFault::Truncated,
            ));
        };
        let count = u64::from(self.record.vn_cnt);
        self.chain.fits(VersionRecord::NeedEntry, start, count, Vernaux::SIZE32)?;
        Ok(VersionNeedEntries {
            chain: self.chain,
            walk: Walk { at: start, left: count, failed: false },
        })
    }
}

/// The entries of a [`VersionNeed`].
#[derive(Debug, Clone)]
pub struct VersionNeedEntries<'a> {
    chain: Chain<'a>,
    walk: Walk,
}

impl<'a> Iterator for VersionNeedEntries<'a> {
    type Item = Result<VersionNeedEntry<'a>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.walk.left == 0 || self.walk.failed {
            return None;
        }
        let at = self.walk.at;
        let record: Vernaux = match self.chain.record(VersionRecord::NeedEntry, at) {
            Ok(record) => record,
            Err(error) => {
                self.walk.failed = true;
                return Some(Err(error));
            }
        };
        if self.walk.left > 1 && record.vna_next == 0 {
            self.walk.failed = true;
            return Some(Err(self.chain.fault(
                VersionRecord::NeedEntry,
                at,
                VersionFault::ChainEnds,
            )));
        }
        self.walk.advance(record.vna_next);
        Some(Ok(VersionNeedEntry { record, at, chain: self.chain }))
    }
}

impl FusedIterator for VersionNeedEntries<'_> {}

/// One needed version. `Copy`, and borrows only the input bytes.
#[derive(Debug, Clone, Copy)]
pub struct VersionNeedEntry<'a> {
    record: Vernaux,
    at: usize,
    chain: Chain<'a>,
}

impl<'a> VersionNeedEntry<'a> {
    /// The record as stored.
    pub fn record(&self) -> &Vernaux {
        &self.record
    }

    /// The record's file offset.
    pub fn offset(&self) -> u64 {
        self.chain.base.saturating_add(self.at as u64)
    }

    /// `vna_other`: the version index symbols that need this version carry.
    pub fn index(&self) -> u16 {
        self.record.vna_other
    }

    /// `vna_flags` (`VER_FLG_*`).
    pub fn flags(&self) -> u16 {
        self.record.vna_flags
    }

    /// `VER_FLG_WEAK`: a weak reference to the version.
    pub fn is_weak(&self) -> bool {
        self.record.vna_flags & VER_FLG_WEAK != 0
    }

    /// `vna_name`: the version's name.
    pub fn name(&self) -> Result<&'a [u8], Error> {
        self.chain.name(self.record.vna_name)
    }
}

/// Which version a version index names ([`Versions::version`]).
#[derive(Debug, Clone, Copy)]
pub enum Version<'a> {
    /// `VER_NDX_LOCAL` (0): the symbol is local to the object.
    Local,
    /// `VER_NDX_GLOBAL` (1): the symbol is global and has no version (its
    /// version is the object's base definition).
    Global,
    /// A version this object defines (`.gnu.version_d`).
    Defined(VersionDefinition<'a>),
    /// A version this object needs from another file (`.gnu.version_r`).
    Needed { need: VersionNeed<'a>, entry: VersionNeedEntry<'a> },
}

/// The version a dynamic symbol carries ([`Versions::of`]).
#[derive(Debug, Clone, Copy)]
pub struct SymbolVersion<'a> {
    /// The symbol's entry of the version table.
    pub versym: Versym,
    /// The version its index names.
    pub version: Version<'a>,
}

impl<'a> SymbolVersion<'a> {
    /// Whether this is the default version of a defined name, the one an
    /// unversioned reference binds to (`name@@VERSION`): a version the
    /// object defines, without the hidden bit. A hidden one is
    /// `name@VERSION`.
    pub fn is_default(&self) -> bool {
        matches!(self.version, Version::Defined(_)) && !self.versym.hidden()
    }

    /// The version's name, or `None` for `Local` and `Global`.
    pub fn name(&self) -> Result<Option<&'a [u8]>, Error> {
        match self.version {
            Version::Local | Version::Global => Ok(None),
            Version::Defined(definition) => definition.name().map(Some),
            Version::Needed { entry, .. } => entry.name().map(Some),
        }
    }
}

/// The symbol versions of a shared object: its version table and its
/// definition and need chains ([`crate::SharedObject::versions`]). `Copy`,
/// and borrows only the input bytes.
#[derive(Debug, Clone, Copy)]
pub struct Versions<'a> {
    pub(crate) versym: Option<Table<'a, Versym>>,
    pub(crate) definitions: Option<Chain<'a>>,
    pub(crate) needs: Option<Chain<'a>>,
}

impl<'a> Versions<'a> {
    /// The version table, one entry per dynamic symbol, or `None` when the
    /// object has none.
    pub fn versym(&self) -> Option<Table<'a, Versym>> {
        self.versym
    }

    /// The version definitions, or `None` when the object has none.
    pub fn definitions(&self) -> Result<Option<VersionDefinitions<'a>>, Error> {
        self.definitions.map(VersionDefinitions::new).transpose()
    }

    /// The version needs, or `None` when the object has none.
    pub fn needs(&self) -> Result<Option<VersionNeeds<'a>>, Error> {
        self.needs.map(VersionNeeds::new).transpose()
    }

    /// What version index `index` names: 0 and 1 are `Local` and `Global`;
    /// any other index must be the `vd_ndx` of exactly one definition or
    /// the `vna_other` of exactly one need entry. `symbol` is for the error
    /// that no version has the index. Walks both chains: the work is
    /// bounded by their size.
    pub fn version(&self, symbol: SymbolIndex, index: u16) -> Result<Version<'a>, Error> {
        match index {
            VER_NDX_LOCAL => return Ok(Version::Local),
            VER_NDX_GLOBAL => return Ok(Version::Global),
            _ => {}
        }
        let mut found: Option<Version<'a>> = None;
        let mut keep = |version: Version<'a>| {
            if found.is_some() {
                return Err(Error::AmbiguousVersionIndex { index });
            }
            found = Some(version);
            Ok(())
        };
        for definition in self.definitions()?.into_iter().flatten() {
            let definition = definition?;
            if definition.index() == index {
                keep(Version::Defined(definition))?;
            }
        }
        for need in self.needs()?.into_iter().flatten() {
            let need = need?;
            for entry in need.entries()? {
                let entry = entry?;
                if entry.index() == index {
                    keep(Version::Needed { need, entry })?;
                }
            }
        }
        found.ok_or(Error::UnknownVersionIndex { symbol: symbol.0, index })
    }

    /// The version dynamic symbol `symbol` carries, or `None` when the
    /// object has no version table.
    pub fn of(&self, symbol: SymbolIndex) -> Result<Option<SymbolVersion<'a>>, Error> {
        let Some(table) = self.versym else { return Ok(None) };
        let Some(versym) = table.get(symbol.0 as usize) else {
            return Err(Error::SymbolIndexOutOfRange {
                index: symbol.0,
                count: table.len() as u32,
            });
        };
        let version = self.version(symbol, versym.index())?;
        Ok(Some(SymbolVersion { versym, version }))
    }
}

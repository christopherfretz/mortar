//! The canonical writer (`meta.md` MT-10, MT-11, MT-12, MT-38).
//!
//! A [`UnitBuilder`] takes rows in any order and [`UnitBuilder::finish`]
//! lays them out one way: blocks in kind order, rows sorted as §7 says
//! (ties broken by every other field, so that the order is total), strings
//! in the order the sorted rows first reference them (blocks in kind
//! order, fields in row order), offset 0 the empty string, zero padding.
//! The same rows in any order give the same bytes.

use core::fmt;

use crate::{BLOCK_HEADER_SIZE, HEADER_SIZE, MAGIC, MAJOR, MINOR, NO_DETAILS, kind};

/// A kind 1 row (MT-26).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Summary {
    pub name: Vec<u8>,
    pub value: Vec<u8>,
    pub flags: u32,
}

/// A kind 2 row (`spec/odr.md` O-15). `name: None` is anchored: `key`
/// is then the anchor's FNV-1a 64, which the writer does not check.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Definition {
    pub key: u64,
    pub hash: [u8; 16],
    pub name: Option<Vec<u8>>,
    pub class: u16,
    pub flags: u16,
    pub module: Vec<u8>,
    pub file: Vec<u8>,
    pub line: u32,
}

/// A kind 3 row (`spec/odr.md` O-17).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Type {
    pub key: u64,
    pub hash: [u8; 16],
    pub layout: u64,
    pub name: Vec<u8>,
    pub size: u32,
    pub align: u16,
    pub flags: u16,
    pub file: Vec<u8>,
    pub line: u32,
    /// [`NO_DETAILS`]: none.
    pub details: u32,
}

/// A kind 5 row (`spec/odr.md` O-35). `name: None` is anchored.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Declaration {
    pub key: u64,
    pub ty: u64,
    pub size: u64,
    pub name: Option<Vec<u8>>,
    pub type_name: Vec<u8>,
    pub flags: u16,
}

/// A block written as given (the forward-compatibility tests of §8 and
/// the experimental range): `kind` must not be one of 1-5 in the same
/// unit as rows of that kind.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RawBlock {
    pub kind: u16,
    pub version: u16,
    pub row_size: u32,
    pub row_count: u32,
    /// The body after the header; padded with zeros to a multiple of 8.
    pub body: Vec<u8>,
}

/// Why a unit cannot be written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteError {
    /// A string holds a NUL byte (MT-12: strings are NUL-terminated).
    Nul(Vec<u8>),
    /// Two kind 1 rows of one name (MT-26: names unique).
    DuplicateSummary(Vec<u8>),
    /// A kind given twice (rows and a raw block, or two raw blocks).
    DuplicateKind(u16),
    /// The unit would not fit the `u32` fields.
    TooLarge,
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nul(s) => write!(f, "string {:?} holds a NUL byte", String::from_utf8_lossy(s)),
            Self::DuplicateSummary(s) => {
                write!(f, "kind 1 name {:?} given twice", String::from_utf8_lossy(s))
            }
            Self::DuplicateKind(k) => write!(f, "kind {k:#06x} given twice"),
            Self::TooLarge => f.write_str("the unit exceeds 4 GiB"),
        }
    }
}

impl std::error::Error for WriteError {}

/// The rows of one unit, in any order.
#[derive(Debug, Clone, Default)]
pub struct UnitBuilder {
    pub producer: u16,
    pub summary: Vec<Summary>,
    pub definitions: Vec<Definition>,
    pub types: Vec<Type>,
    /// Kind 4's body (WP-073 fixes its encoding); written when `Some`.
    pub details: Option<Vec<u8>>,
    pub declarations: Vec<Declaration>,
    pub raw: Vec<RawBlock>,
}

/// The string table under construction (MT-12, MT-38).
struct Strings {
    table: Vec<u8>,
    seen: std::collections::HashMap<Vec<u8>, u32>,
}

impl Strings {
    fn new() -> Self {
        Self { table: vec![0], seen: std::collections::HashMap::new() }
    }

    fn add(&mut self, s: &[u8]) -> Result<u32, WriteError> {
        if s.is_empty() {
            return Ok(0);
        }
        if s.contains(&0) {
            return Err(WriteError::Nul(s.to_vec()));
        }
        if let Some(&at) = self.seen.get(s) {
            return Ok(at);
        }
        let at = u32::try_from(self.table.len()).map_err(|_| WriteError::TooLarge)?;
        self.table.extend_from_slice(s);
        self.table.push(0);
        self.seen.insert(s.to_vec(), at);
        Ok(at)
    }
}

fn put16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn opt(name: Option<&Vec<u8>>) -> &[u8] {
    name.map_or(b"", Vec::as_slice)
}

impl UnitBuilder {
    #[must_use]
    pub fn new(producer: u16) -> Self {
        Self { producer, ..Self::default() }
    }

    /// The canonical unit (MT-38).
    ///
    /// # Errors
    /// [`WriteError`]: a string with a NUL, a kind 1 name twice, a kind
    /// twice, or a unit past 4 GiB.
    #[allow(clippy::too_many_lines)] // one layout, read top to bottom
    pub fn finish(&self) -> Result<Vec<u8>, WriteError> {
        let mut summary = self.summary.clone();
        summary.sort();
        if let Some(w) = summary.windows(2).find(|w| w[0].name == w[1].name) {
            return Err(WriteError::DuplicateSummary(w[0].name.clone()));
        }
        let mut definitions = self.definitions.clone();
        definitions.sort_by(|a, b| {
            (a.key, opt(a.name.as_ref()), a.class, a.flags, a.hash, &a.module, &a.file, a.line).cmp(
                &(
                    b.key,
                    opt(b.name.as_ref()),
                    b.class,
                    b.flags,
                    b.hash,
                    &b.module,
                    &b.file,
                    b.line,
                ),
            )
        });
        let mut types = self.types.clone();
        types.sort_by(|a, b| {
            (a.key, &a.name, a.hash, a.layout, a.size, a.align, a.flags, &a.file, a.line, a.details)
                .cmp(&(
                    b.key, &b.name, b.hash, b.layout, b.size, b.align, b.flags, &b.file, b.line,
                    b.details,
                ))
        });
        let mut declarations = self.declarations.clone();
        declarations.sort_by(|a, b| {
            (a.key, a.flags, opt(a.name.as_ref()), a.ty, a.size, &a.type_name).cmp(&(
                b.key,
                b.flags,
                opt(b.name.as_ref()),
                b.ty,
                b.size,
                &b.type_name,
            ))
        });

        // Blocks, in kind order; strings as the sorted rows reference them.
        let mut strings = Strings::new();
        let mut blocks: Vec<(u16, Vec<u8>)> = Vec::new();
        let mut header = |k: u16, version: u16, row_size: u32, rows: usize, body: Vec<u8>| {
            let mut b = Vec::with_capacity(body.len() + 24);
            put16(&mut b, k);
            put16(&mut b, version);
            let size = (BLOCK_HEADER_SIZE as usize + body.len()).next_multiple_of(8);
            put32(&mut b, u32::try_from(size).map_err(|_| WriteError::TooLarge)?);
            put32(&mut b, row_size);
            put32(&mut b, u32::try_from(rows).map_err(|_| WriteError::TooLarge)?);
            b.extend_from_slice(&body);
            b.resize(size, 0);
            blocks.push((k, b));
            Ok::<(), WriteError>(())
        };
        if !summary.is_empty() {
            let mut body = Vec::with_capacity(summary.len() * 16);
            for r in &summary {
                put32(&mut body, strings.add(&r.name)?);
                put32(&mut body, strings.add(&r.value)?);
                put32(&mut body, r.flags);
                put32(&mut body, 0);
            }
            header(kind::SUMMARY, 1, kind::SUMMARY_ROW, summary.len(), body)?;
        }
        if !definitions.is_empty() {
            let mut body = Vec::with_capacity(definitions.len() * 48);
            for r in &definitions {
                put64(&mut body, r.key);
                body.extend_from_slice(&r.hash);
                put32(&mut body, strings.add(opt(r.name.as_ref()))?);
                put16(&mut body, r.class);
                put16(&mut body, r.flags);
                put32(&mut body, strings.add(&r.module)?);
                put32(&mut body, strings.add(&r.file)?);
                put32(&mut body, r.line);
                put32(&mut body, 0);
            }
            header(kind::DEFINITIONS, 1, kind::DEFINITION_ROW, definitions.len(), body)?;
        }
        if !types.is_empty() {
            let mut body = Vec::with_capacity(types.len() * 56);
            for r in &types {
                put64(&mut body, r.key);
                body.extend_from_slice(&r.hash);
                put64(&mut body, r.layout);
                put32(&mut body, strings.add(&r.name)?);
                put32(&mut body, r.size);
                put16(&mut body, r.align);
                put16(&mut body, r.flags);
                put32(&mut body, strings.add(&r.file)?);
                put32(&mut body, r.line);
                put32(&mut body, r.details);
            }
            header(kind::TYPES, 1, kind::TYPE_ROW, types.len(), body)?;
        }
        if let Some(details) = &self.details {
            header(kind::DETAILS, 1, 0, 0, details.clone())?;
        }
        if !declarations.is_empty() {
            let mut body = Vec::with_capacity(declarations.len() * 40);
            for r in &declarations {
                put64(&mut body, r.key);
                put64(&mut body, r.ty);
                put64(&mut body, r.size);
                put32(&mut body, strings.add(opt(r.name.as_ref()))?);
                put32(&mut body, strings.add(&r.type_name)?);
                put16(&mut body, r.flags);
                put16(&mut body, 0);
                put32(&mut body, 0);
            }
            header(kind::DECLARATIONS, 1, kind::DECLARATION_ROW, declarations.len(), body)?;
        }
        for r in &self.raw {
            if blocks.iter().any(|(k, _)| *k == r.kind) {
                return Err(WriteError::DuplicateKind(r.kind));
            }
            let body = r.body.clone();
            let mut b = Vec::new();
            put16(&mut b, r.kind);
            put16(&mut b, r.version);
            let size = (BLOCK_HEADER_SIZE as usize + body.len()).next_multiple_of(8);
            put32(&mut b, u32::try_from(size).map_err(|_| WriteError::TooLarge)?);
            put32(&mut b, r.row_size);
            put32(&mut b, r.row_count);
            b.extend_from_slice(&body);
            b.resize(size, 0);
            blocks.push((r.kind, b));
        }
        blocks.sort_by_key(|(k, _)| *k);

        let blocks_len: usize = blocks.iter().map(|(_, b)| b.len()).sum();
        let has_strings = strings.table.len() > 1;
        let strtab_offset = HEADER_SIZE as usize + blocks_len;
        let strtab_size = if has_strings { strings.table.len() } else { 0 };
        let unit_size = (strtab_offset + strtab_size).next_multiple_of(8);
        let unit_size32 = u32::try_from(unit_size).map_err(|_| WriteError::TooLarge)?;
        let mut out = Vec::with_capacity(unit_size);
        out.extend_from_slice(&MAGIC);
        put16(&mut out, MAJOR);
        put16(&mut out, MINOR);
        put32(&mut out, unit_size32);
        put32(&mut out, HEADER_SIZE);
        put32(&mut out, u32::try_from(blocks.len()).map_err(|_| WriteError::TooLarge)?);
        if has_strings {
            put32(&mut out, strtab_offset as u32);
            put32(&mut out, strtab_size as u32);
        } else {
            put32(&mut out, 0);
            put32(&mut out, 0);
        }
        put16(&mut out, self.producer);
        put16(&mut out, 0);
        for (_, b) in &blocks {
            out.extend_from_slice(b);
        }
        if has_strings {
            out.extend_from_slice(&strings.table);
        }
        out.resize(unit_size, 0);
        Ok(out)
    }
}

impl Type {
    /// A row with no details entry and the key of `name`.
    #[must_use]
    pub fn named(name: &[u8]) -> Self {
        Self {
            key: crate::fnv1a64(name),
            hash: [0; 16],
            layout: 0,
            name: name.to_vec(),
            size: 0,
            align: 0,
            flags: 0,
            file: Vec::new(),
            line: 0,
            details: NO_DETAILS,
        }
    }
}

/// Concatenate units as a section's contents (MT-10, MT-4): each starts
/// at an 8-aligned offset (units are multiples of 8, so no padding is
/// needed between them; `pad` adds that many zero words before each unit
/// after the first, as `-r` may).
#[must_use]
pub fn section(units: &[Vec<u8>], pad: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, u) in units.iter().enumerate() {
        if i > 0 {
            out.resize(out.len() + 8 * pad, 0);
        }
        out.extend_from_slice(u);
    }
    out
}

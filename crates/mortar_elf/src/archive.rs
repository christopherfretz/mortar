//! Archives (`spec/archive.md`): the `ar` container a linker reads static
//! libraries from. Regular (`!<arch>\n`) and thin (`!<thin>\n`) archives,
//! GNU and BSD member names, the GNU, 64-bit GNU and BSD symbol indexes.
//!
//! The reader works on the bytes of the archive file and borrows from them.
//! [`Archive::parse`] examines the special members at the front (the symbol
//! index and the long-name table) and nothing else; ordinary members are
//! decoded one header at a time, by [`Archive::members`] in file order or by
//! [`Archive::member_at`] from an offset the symbol index gives. Nothing is
//! allocated, in `parse` or afterwards.
//!
//! Members are not interpreted: what a member contains, and which members a
//! link takes, is decided elsewhere. For a thin archive the reader gives the
//! stored path of each member and the caller reads the file.

use std::fmt;
use std::iter::FusedIterator;

use crate::view::View;

/// The first eight bytes of a regular archive (`spec/archive.md` §2).
pub const MAGIC: &[u8; 8] = b"!<arch>\n";
/// The first eight bytes of a thin archive (`spec/archive.md` §2).
pub const THIN_MAGIC: &[u8; 8] = b"!<thin>\n";
/// The size of a member header in bytes (`spec/archive.md` §3).
pub const HEADER_LEN: u64 = 60;

const HEADER: usize = 60;
const NAME_FIELD: std::ops::Range<usize> = 0..16;
const SIZE_FIELD: std::ops::Range<usize> = 48..58;
const TERMINATOR_FIELD: std::ops::Range<usize> = 58..60;

/// Whether members are stored in the archive or named by it (§2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `!<arch>\n`: each member's bytes follow its header.
    Regular,
    /// `!<thin>\n`: each ordinary member is a path to a file.
    Thin,
}

/// An archive, borrowed from the bytes of its file: the whole file, or a
/// parts view of it in which the front of the archive (the magic and the
/// special members) and every member header are resident
/// ([`Archive::parse_view`]).
#[derive(Debug, Clone, Copy)]
pub struct Archive<'a> {
    bytes: View<'a>,
    kind: Kind,
    /// The symbol index member, read or failed (§6.5).
    index: Option<Result<SymbolIndex<'a>, Error>>,
    long_names: Option<&'a [u8]>,
    /// Offset of the first ordinary member's header; the length of the
    /// archive when there is none.
    first_member: u64,
}

impl<'a> Archive<'a> {
    /// The kind of archive `bytes` starts as, from the magic alone (§2).
    pub fn sniff(bytes: &[u8]) -> Option<Kind> {
        if bytes.starts_with(MAGIC) {
            Some(Kind::Regular)
        } else if bytes.starts_with(THIN_MAGIC) {
            Some(Kind::Thin)
        } else {
            None
        }
    }

    /// Reads the magic and the special members at the front of the archive
    /// (§5). The cost does not depend on the number of members or symbols.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        Self::parse_view(View::whole(bytes))
    }

    /// [`Archive::parse`] over a parts view: the same checks and errors as
    /// long as the bytes the reader decodes are resident (the magic, the
    /// special members, the member headers and BSD names);
    /// [`Error::NotResident`] otherwise. Over a parts view, iterate with
    /// [`Archive::headers`]: [`Archive::members`] gives a member's content
    /// as a slice, which needs the whole content resident.
    pub fn parse_view(bytes: View<'a>) -> Result<Self, Error> {
        let magic = bytes
            .prefix(MAGIC.len() as u64)
            .ok_or(Error::NotResident { offset: 0, size: MAGIC.len() as u64 })?;
        let kind = Self::sniff(magic).ok_or(Error::NotAnArchive)?;
        let mut archive =
            Self { bytes, kind, index: None, long_names: None, first_member: MAGIC.len() as u64 };
        let len = bytes.len();
        let mut at = MAGIC.len() as u64;
        if at < len {
            let head = archive.head(at)?;
            match head.special {
                Some(Special::LongNames) => {
                    archive.long_names = Some(archive.body(&head)?);
                    at = head.next;
                }
                Some(Special::Index(index_kind)) => {
                    // A malformed index does not fail the archive: the
                    // linker does not need it (§6.5).
                    let body = archive.body(&head)?;
                    archive.index = Some(SymbolIndex::parse(index_kind, at, body));
                    at = head.next;
                    if at < len {
                        let second = archive.head(at)?;
                        match second.special {
                            Some(Special::LongNames) => {
                                archive.long_names = Some(archive.body(&second)?);
                                at = second.next;
                            }
                            Some(Special::Index(IndexKind::Gnu)) if index_kind.is_gnu() => {
                                return Err(Error::CoffArchive { offset: at });
                            }
                            _ => {}
                        }
                    }
                }
                None => {}
            }
        }
        archive.first_member = at;
        Ok(archive)
    }

    /// Regular or thin.
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// The symbol index; `None` when the archive has none (§6.4),
    /// `Some(Err(BadIndex))` when its framing is malformed (§6.5).
    pub fn symbol_index(&self) -> Option<Result<SymbolIndex<'a>, Error>> {
        self.index
    }

    /// The offset of the first ordinary member's header (the length of
    /// the archive when there is none): where the front ends (§5).
    pub fn first_member(&self) -> u64 {
        self.first_member
    }

    /// The ordinary members in file order (§7). The iterator ends after the
    /// first error.
    pub fn members(&self) -> Members<'a> {
        Members { archive: *self, next: self.first_member }
    }

    /// The headers of the ordinary members in file order, without their
    /// contents (§7): what a parts view can give. The iterator ends after
    /// the first error.
    pub fn headers(&self) -> Headers<'a> {
        Headers { archive: *self, next: self.first_member }
    }

    /// The ordinary member whose header starts at `header_offset`, which is
    /// what the symbol index gives (§7).
    pub fn member_at(&self, header_offset: u64) -> Result<Member<'a>, Error> {
        let first = self.first_member;
        let end = self.bytes.len();
        if header_offset < first || header_offset >= end {
            return Err(Error::NoMemberAt { offset: header_offset, first, end });
        }
        if !header_offset.is_multiple_of(2) {
            return Err(Error::OddMemberOffset { offset: header_offset });
        }
        self.ordinary(header_offset).map(|(member, _)| member)
    }

    /// The stored bytes of a special member.
    fn body(&self, head: &Head<'a>) -> Result<&'a [u8], Error> {
        let Some((start, size)) = head.content else { return Ok(&[]) };
        self.bytes.get(start, size).ok_or(Error::NotResident { offset: start, size })
    }

    /// Decodes the header at `offset` (less than the length of the archive)
    /// without resolving a long-name reference.
    fn head(&self, offset: u64) -> Result<Head<'a>, Error> {
        let at = offset;
        let len = self.bytes.len();
        let rest = len.saturating_sub(offset);
        if rest < HEADER_LEN {
            return Err(Error::TruncatedHeader { offset: at, available: rest });
        }
        let raw = self
            .bytes
            .get(offset, HEADER_LEN)
            .ok_or(Error::NotResident { offset, size: HEADER_LEN })?;
        let terminator = &raw[TERMINATOR_FIELD];
        if terminator != b"`\n" {
            return Err(Error::BadHeaderTerminator { offset: at, found: array(terminator) });
        }
        let size_field = &raw[SIZE_FIELD];
        let size = decimal(trim_spaces(size_field))
            .ok_or(Error::BadSize { offset: at, field: array(size_field) })?;
        let form = self.name_form(at, &raw[NAME_FIELD])?;

        // The bytes after the header belong to the member unless it is an
        // ordinary member of a thin archive.
        let after_header = offset + HEADER_LEN;
        if self.kind == Kind::Thin && !matches!(form, Form::Special(_)) {
            let name = match form {
                Form::Short(name) => Name::Direct(name),
                Form::Long { name_offset, origin } => Name::Long { name_offset, origin },
                Form::Special(_) | Form::Bsd(_) => {
                    unreachable!("thin archive: special excluded above, BSD names refused")
                }
            };
            return Ok(Head { special: None, name, size, content: None, next: after_header });
        }
        let available = len - after_header;
        if size > available {
            return Err(Error::TruncatedMember { offset: at, size, available });
        }
        let end = after_header + size;
        // The last member's padding byte may be missing (§3).
        let next = (end + (end & 1)).min(len);

        let (special, name, content_start, content_size) = match form {
            Form::Special(special) => (Some(special), Name::Direct(&[]), after_header, size),
            Form::Short(name) => (None, Name::Direct(name), after_header, size),
            Form::Long { name_offset, origin } => {
                (None, Name::Long { name_offset, origin }, after_header, size)
            }
            Form::Bsd(name_len) => {
                if name_len > size {
                    return Err(Error::BsdNameTooLong { offset: at, name_len, size });
                }
                let padded = self
                    .bytes
                    .get(after_header, name_len)
                    .ok_or(Error::NotResident { offset: after_header, size: name_len })?;
                let name = padded.iter().position(|&b| b == 0).map_or(padded, |nul| &padded[..nul]);
                (
                    bsd_index_named(name),
                    Name::Direct(name),
                    after_header + name_len,
                    size - name_len,
                )
            }
        };
        Ok(Head {
            special,
            name,
            size: content_size,
            content: Some((content_start, content_size)),
            next,
        })
    }

    /// Classifies a header's name field (§4).
    fn name_form(&self, at: u64, field: &'a [u8]) -> Result<Form<'a>, Error> {
        let trimmed = trim_spaces(field);
        if trimmed == b"__.SYMDEF SORTED" {
            return Ok(Form::Special(Special::Index(IndexKind::Bsd)));
        }
        // A field that starts with `/` or `#1/` ends at its first space:
        // GNU ar leaves a `/` in the last byte of some (§4).
        let word = field.iter().position(|&b| b == b' ').map_or(field, |space| &field[..space]);
        let special = match word {
            b"/" => Some(Special::Index(IndexKind::Gnu)),
            b"/SYM64/" => Some(Special::Index(IndexKind::Gnu64)),
            b"//" => Some(Special::LongNames),
            _ => None,
        };
        if let Some(special) = special {
            return Ok(Form::Special(special));
        }
        if let Some(length) = word.strip_prefix(b"#1/") {
            let length = decimal(length)
                .ok_or(Error::BadBsdNameLength { offset: at, field: array(field) })?;
            if self.kind == Kind::Thin {
                return Err(Error::BsdNameInThinArchive { offset: at });
            }
            return Ok(Form::Bsd(length));
        }
        if let Some(reference) = word.strip_prefix(b"/") {
            let bad = Error::BadLongNameReference { offset: at, field: array(field) };
            let mut parts = reference.splitn(2, |&b| b == b':');
            let name_offset = parts.next().and_then(decimal).ok_or(bad)?;
            let origin = match parts.next() {
                None => None,
                Some(origin) => Some(decimal(origin).ok_or(bad)?),
            };
            if origin.is_some() && self.kind == Kind::Regular {
                return Err(Error::NestedMemberInRegularArchive {
                    offset: at,
                    field: array(field),
                });
            }
            return Ok(Form::Long { name_offset, origin });
        }
        if field.contains(&0) {
            return Err(Error::NulInName { offset: at, field: array(field) });
        }
        let name = if let Some(slash) = field.iter().position(|&b| b == b'/') {
            &field[..slash]
        } else if trimmed.contains(&b' ') {
            return Err(Error::AmbiguousName { offset: at, field: array(field) });
        } else {
            trimmed
        };
        Ok(bsd_index_named(name).map_or(Form::Short(name), Form::Special))
    }

    /// The name at `name_offset` of the long-name table (§4.2), for the
    /// header at `at`.
    fn long_name(&self, at: u64, name_offset: u64) -> Result<&'a [u8], Error> {
        let table = self.long_names.ok_or(Error::NoLongNameTable { offset: at, name_offset })?;
        let tail = usize::try_from(name_offset)
            .ok()
            .and_then(|start| table.get(start..))
            .filter(|tail| !tail.is_empty())
            .ok_or(Error::LongNameOutOfRange {
                offset: at,
                name_offset,
                table_size: table.len() as u64,
            })?;
        tail.iter()
            .position(|&b| b == b'\n')
            .and_then(|newline| tail[..newline].strip_suffix(b"/"))
            .ok_or(Error::LongNameUnterminated { offset: at, name_offset })
    }

    /// The header of the ordinary member at `offset` and the offset of the
    /// header after it.
    fn entry(&self, offset: u64) -> Result<(MemberHeader<'a>, u64), Error> {
        let at = offset;
        let head = self.head(offset)?;
        if let Some(special) = head.special {
            return Err(Error::MisplacedSpecialMember { offset: at, special });
        }
        let (name, origin) = match head.name {
            Name::Direct(name) => (name, None),
            Name::Long { name_offset, origin } => (self.long_name(at, name_offset)?, origin),
        };
        let location = match (head.content, origin) {
            (Some((start, _)), _) => Location::Inline { offset: start },
            (None, None) => Location::External { path: name },
            (None, Some(header_offset)) => {
                Location::ExternalMember { archive_path: name, header_offset }
            }
        };
        Ok((MemberHeader { header_offset: at, name, size: head.size, location }, head.next))
    }

    /// The ordinary member at `offset` and the offset of the header after
    /// it.
    fn ordinary(&self, offset: u64) -> Result<(Member<'a>, u64), Error> {
        let (header, next) = self.entry(offset)?;
        let content = match header.location {
            Location::Inline { offset } => {
                let size = header.size;
                let data =
                    self.bytes.get(offset, size).ok_or(Error::NotResident { offset, size })?;
                Content::Inline { offset, data }
            }
            Location::External { path } => Content::External { path },
            Location::ExternalMember { archive_path, header_offset } => {
                Content::ExternalMember { archive_path, header_offset }
            }
        };
        let member = Member {
            header_offset: header.header_offset,
            name: header.name,
            size: header.size,
            content,
        };
        Ok((member, next))
    }
}

/// What a header's name field is, before any lookup.
enum Form<'a> {
    Special(Special),
    /// The name itself.
    Short(&'a [u8]),
    /// `#1/<length>`: the name is the first `length` bytes after the header.
    Bsd(u64),
    /// `/<name_offset>` or, in a thin archive, `/<name_offset>:<origin>`.
    Long {
        name_offset: u64,
        origin: Option<u64>,
    },
}

enum Name<'a> {
    Direct(&'a [u8]),
    Long { name_offset: u64, origin: Option<u64> },
}

/// A decoded header.
struct Head<'a> {
    special: Option<Special>,
    name: Name<'a>,
    /// The size of the content: the header's size, less a BSD name.
    size: u64,
    /// Where the content starts in the archive and its size; `None` for an
    /// ordinary member of a thin archive.
    content: Option<(u64, u64)>,
    /// Offset of the next header, at most the length of the archive.
    next: u64,
}

/// A BSD symbol index is a member with one of four names (§6.3).
fn bsd_index_named(name: &[u8]) -> Option<Special> {
    match name {
        b"__.SYMDEF" | b"__.SYMDEF SORTED" => Some(Special::Index(IndexKind::Bsd)),
        b"__.SYMDEF_64" | b"__.SYMDEF_64 SORTED" => Some(Special::Index(IndexKind::Bsd64)),
        _ => None,
    }
}

/// The header of an ordinary member (§7): everything [`Member`] says but
/// its content's bytes. [`Archive::headers`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemberHeader<'a> {
    pub header_offset: u64,
    /// The member's name. In a thin archive, the stored path (§7.2).
    pub name: &'a [u8],
    /// The size of the member's content in bytes.
    pub size: u64,
    pub location: Location<'a>,
}

/// Where the bytes of a member are, without them ([`MemberHeader`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Location<'a> {
    /// In the archive: `size` bytes starting at `offset`.
    Inline { offset: u64 },
    /// Thin archive: the whole file at `path` ([`Content::External`]).
    External { path: &'a [u8] },
    /// Thin archive, a member of a regular archive it refers to
    /// ([`Content::ExternalMember`]).
    ExternalMember { archive_path: &'a [u8], header_offset: u64 },
}

/// An ordinary member (§7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Member<'a> {
    header_offset: u64,
    name: &'a [u8],
    size: u64,
    content: Content<'a>,
}

impl<'a> Member<'a> {
    /// The offset of the member's header in the archive: what the symbol
    /// index refers to and [`Archive::member_at`] takes.
    pub fn header_offset(&self) -> u64 {
        self.header_offset
    }

    /// The member's name. In a thin archive, the stored path (§7.2).
    pub fn name(&self) -> &'a [u8] {
        self.name
    }

    /// The size of the member's content in bytes.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Where the member's content is.
    pub fn content(&self) -> Content<'a> {
        self.content
    }

    /// The content of a member stored in the archive; `None` in a thin
    /// archive.
    pub fn data(&self) -> Option<&'a [u8]> {
        match self.content {
            Content::Inline { data, .. } => Some(data),
            Content::External { .. } | Content::ExternalMember { .. } => None,
        }
    }
}

/// Where the bytes of a member are (§7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Content<'a> {
    /// In the archive: `data` is the `size` bytes starting at `offset`.
    Inline { offset: u64, data: &'a [u8] },
    /// Thin archive: the whole file at `path`, which is relative to the
    /// directory of the archive unless it starts with `/`.
    External { path: &'a [u8] },
    /// Thin archive, a member of a regular archive it refers to (§7.3): the
    /// member whose header is at `header_offset` of the archive file at
    /// `archive_path`.
    ExternalMember { archive_path: &'a [u8], header_offset: u64 },
}

/// The ordinary members of an archive in file order.
#[derive(Debug, Clone)]
pub struct Members<'a> {
    archive: Archive<'a>,
    next: u64,
}

impl<'a> Iterator for Members<'a> {
    type Item = Result<Member<'a>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        let end = self.archive.bytes.len();
        if self.next >= end {
            return None;
        }
        Some(match self.archive.ordinary(self.next) {
            Ok((member, next)) => {
                self.next = next;
                Ok(member)
            }
            Err(error) => {
                self.next = end;
                Err(error)
            }
        })
    }
}

impl FusedIterator for Members<'_> {}

/// The headers of the ordinary members of an archive in file order
/// ([`Archive::headers`]).
#[derive(Debug, Clone)]
pub struct Headers<'a> {
    archive: Archive<'a>,
    next: u64,
}

impl<'a> Iterator for Headers<'a> {
    type Item = Result<MemberHeader<'a>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        let end = self.archive.bytes.len();
        if self.next >= end {
            return None;
        }
        Some(match self.archive.entry(self.next) {
            Ok((header, next)) => {
                self.next = next;
                Ok(header)
            }
            Err(error) => {
                self.next = end;
                Err(error)
            }
        })
    }
}

impl FusedIterator for Headers<'_> {}

/// What a staged reader must keep resident of the header at `offset` of a
/// regular archive of `len` bytes, decoded from `raw`, the header's bytes
/// (60, or fewer at the end of the archive): `Ok` with the header's span
/// when the reader accepts the header's framing, `Err` with the error the
/// reader gives at that header otherwise (`spec/input.md` §11.3).
pub fn header_span(offset: u64, raw: &[u8], len: u64) -> Result<HeaderSpan, Error> {
    if raw.len() < HEADER || len.saturating_sub(offset) < HEADER_LEN {
        return Err(Error::TruncatedHeader { offset, available: len.saturating_sub(offset) });
    }
    let raw = &raw[..HEADER];
    let terminator = &raw[TERMINATOR_FIELD];
    if terminator != b"`\n" {
        return Err(Error::BadHeaderTerminator { offset, found: array(terminator) });
    }
    let size_field = &raw[SIZE_FIELD];
    let size = decimal(trim_spaces(size_field))
        .ok_or(Error::BadSize { offset, field: array(size_field) })?;
    let after_header = offset + HEADER_LEN;
    let available = len - after_header;
    if size > available {
        return Err(Error::TruncatedMember { offset, size, available });
    }
    let field = &raw[NAME_FIELD];
    let word = field.iter().position(|&b| b == b' ').map_or(field, |space| &field[..space]);
    let trimmed = trim_spaces(field);
    let bsd_name = word.strip_prefix(b"#1/").and_then(decimal).unwrap_or(0).min(size);
    let special = matches!(word, b"/" | b"/SYM64/" | b"//")
        || trimmed == b"__.SYMDEF SORTED"
        || bsd_index_named(trimmed).is_some()
        || bsd_name > 0;
    let end = after_header + size;
    Ok(HeaderSpan {
        content: after_header,
        size,
        front: if special { size } else { bsd_name },
        next: (end + (end & 1)).min(len),
    })
}

/// [`header_span`]'s answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeaderSpan {
    /// Where the member's stored bytes start (after the 60-byte header).
    pub content: u64,
    /// The number of stored bytes.
    pub size: u64,
    /// How many of the stored bytes the reader decodes itself and must
    /// find resident: all of a special member (a symbol index or the
    /// long-name table, and a BSD-named member, whose name may make it an
    /// index), the BSD name of any other member, 0 for an ordinary one.
    pub front: u64,
    /// The offset of the next header.
    pub next: u64,
}

/// The form of a symbol index (§6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexKind {
    /// `/`: big-endian 32-bit words.
    Gnu,
    /// `/SYM64/`: big-endian 64-bit words.
    Gnu64,
    /// `__.SYMDEF` and `__.SYMDEF SORTED`: little-endian 32-bit words.
    Bsd,
    /// `__.SYMDEF_64` and `__.SYMDEF_64 SORTED`: little-endian 64-bit words.
    Bsd64,
}

impl IndexKind {
    fn is_gnu(self) -> bool {
        matches!(self, Self::Gnu | Self::Gnu64)
    }

    /// The size of a word of the index in bytes.
    fn word(self) -> usize {
        match self {
            Self::Gnu | Self::Bsd => 4,
            Self::Gnu64 | Self::Bsd64 => 8,
        }
    }

    /// The word at `at` of `bytes`.
    fn read(self, bytes: &[u8], at: usize) -> Option<u64> {
        let field = bytes.get(at..at.checked_add(self.word())?)?;
        let push = |value: u64, &byte: &u8| (value << 8) | u64::from(byte);
        Some(if self.is_gnu() {
            field.iter().fold(0, push)
        } else {
            field.iter().rev().fold(0, push)
        })
    }
}

impl fmt::Display for IndexKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Gnu => "GNU, \"/\"",
            Self::Gnu64 => "GNU 64-bit, \"/SYM64/\"",
            Self::Bsd => "BSD, \"__.SYMDEF\"",
            Self::Bsd64 => "BSD 64-bit, \"__.SYMDEF_64\"",
        })
    }
}

/// The symbol index of an archive (§6): for each symbol the archive's
/// writer listed, its name and the offset of the header of the member that
/// defines it.
#[derive(Debug, Clone, Copy)]
pub struct SymbolIndex<'a> {
    kind: IndexKind,
    header_offset: u64,
    len: usize,
    /// GNU: one word per entry. BSD: two words per entry.
    table: &'a [u8],
    /// The names: consecutive (GNU) or addressed by the entries (BSD).
    names: &'a [u8],
}

impl<'a> SymbolIndex<'a> {
    /// Checks the framing of an index member: that the table and the names
    /// it announces fit in `body`. The names are checked as they are read.
    fn parse(kind: IndexKind, header_offset: u64, body: &'a [u8]) -> Result<Self, Error> {
        let bad = |problem| Error::BadIndex { offset: header_offset, kind, problem };
        let word = kind.word();
        let size = body.len() as u64;
        let first = kind.read(body, 0).ok_or_else(|| bad(IndexProblem::NoCount { size }))?;
        let after_first = &body[word..];
        // GNU: the number of entries. BSD: the size of the table in bytes.
        let entry = if kind.is_gnu() { word } else { 2 * word };
        let table_bytes = if kind.is_gnu() { first.checked_mul(entry as u64) } else { Some(first) };
        let table = table_bytes
            .and_then(|bytes| usize::try_from(bytes).ok())
            .and_then(|bytes| after_first.split_at_checked(bytes));
        let Some((table, rest)) = table else {
            return Err(bad(IndexProblem::TableOutOfRange { announced: first, size }));
        };
        if !table.len().is_multiple_of(entry) {
            return Err(bad(IndexProblem::TableNotWhole { bytes: first }));
        }
        let names = if kind.is_gnu() {
            rest
        } else {
            let announced = kind.read(rest, 0).ok_or_else(|| bad(IndexProblem::NoNamesSize))?;
            let available = &rest[word..];
            usize::try_from(announced).ok().and_then(|bytes| available.get(..bytes)).ok_or_else(
                || {
                    bad(IndexProblem::NamesOutOfRange {
                        announced,
                        available: available.len() as u64,
                    })
                },
            )?
        };
        Ok(Self { kind, header_offset, len: table.len() / entry, table, names })
    }

    /// The form of the index.
    pub fn kind(&self) -> IndexKind {
        self.kind
    }

    /// The number of entries.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the index has no entries.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The member header offset of entry `entry`, in constant time; `None`
    /// when `entry` is not less than [`len`](Self::len).
    pub fn member_offset(&self, entry: usize) -> Option<u64> {
        if entry >= self.len {
            return None;
        }
        let word = self.kind.word();
        let at = if self.kind.is_gnu() { entry * word } else { entry * 2 * word + word };
        self.kind.read(self.table, at)
    }

    /// The entries in the order of the index. The iterator ends after the
    /// first error.
    pub fn iter(&self) -> Symbols<'a> {
        Symbols { index: *self, next: 0, unread: self.names }
    }

    fn error(&self, problem: IndexProblem) -> Error {
        Error::BadIndex { offset: self.header_offset, kind: self.kind, problem }
    }
}

impl<'a> IntoIterator for SymbolIndex<'a> {
    type Item = Result<Symbol<'a>, Error>;
    type IntoIter = Symbols<'a>;

    fn into_iter(self) -> Symbols<'a> {
        self.iter()
    }
}

impl<'a> IntoIterator for &SymbolIndex<'a> {
    type Item = Result<Symbol<'a>, Error>;
    type IntoIter = Symbols<'a>;

    fn into_iter(self) -> Symbols<'a> {
        self.iter()
    }
}

/// One entry of the symbol index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Symbol<'a> {
    /// The symbol's name, without its terminating NUL.
    pub name: &'a [u8],
    /// The offset of the header of the member that defines it, for
    /// [`Archive::member_at`].
    pub member_offset: u64,
}

/// The entries of a symbol index.
#[derive(Debug, Clone)]
pub struct Symbols<'a> {
    index: SymbolIndex<'a>,
    next: usize,
    /// GNU: the names of the entries not yet read.
    unread: &'a [u8],
}

impl<'a> Symbols<'a> {
    fn read(&mut self, entry: usize) -> Result<Symbol<'a>, Error> {
        let index = self.index;
        let unterminated = IndexProblem::NameUnterminated { entry: entry as u64 };
        let member_offset = index.member_offset(entry).unwrap_or_else(|| {
            unreachable!("entry {entry} of {}: the table's size was checked", index.len)
        });
        let names = if index.kind.is_gnu() {
            self.unread
        } else {
            let word = index.kind.word();
            let name_offset = index.kind.read(index.table, entry * 2 * word).unwrap_or_else(|| {
                unreachable!("entry {entry} of {}: the table's size was checked", index.len)
            });
            usize::try_from(name_offset)
                .ok()
                .and_then(|start| index.names.get(start..))
                .filter(|tail| !tail.is_empty())
                .ok_or_else(|| {
                    index.error(IndexProblem::NameOutOfRange { entry: entry as u64, name_offset })
                })?
        };
        let nul = names.iter().position(|&b| b == 0).ok_or_else(|| index.error(unterminated))?;
        if index.kind.is_gnu() {
            self.unread = &names[nul + 1..];
        }
        Ok(Symbol { name: &names[..nul], member_offset })
    }
}

impl<'a> Iterator for Symbols<'a> {
    type Item = Result<Symbol<'a>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next >= self.index.len {
            return None;
        }
        let result = self.read(self.next);
        self.next = if result.is_ok() { self.next + 1 } else { self.index.len };
        Some(result)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.index.len - self.next))
    }
}

impl FusedIterator for Symbols<'_> {}

/// A special member (§5), as an error names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Special {
    /// A symbol index.
    Index(IndexKind),
    /// The long-name table, `//`.
    LongNames,
}

impl fmt::Display for Special {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Index(kind) => write!(f, "a symbol index ({kind})"),
            Self::LongNames => f.write_str("a long-name table (\"//\")"),
        }
    }
}

/// What is wrong with a symbol index ([`Error::BadIndex`], §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexProblem {
    /// The member is too small for its first word.
    NoCount { size: u64 },
    /// The table the first word announces (GNU: that many entries; BSD:
    /// that many bytes) does not fit in the member.
    TableOutOfRange { announced: u64, size: u64 },
    /// BSD: the size of the table is not a whole number of entries.
    TableNotWhole { bytes: u64 },
    /// BSD: no word for the size of the names follows the table.
    NoNamesSize,
    /// BSD: the names do not fit in the member.
    NamesOutOfRange { announced: u64, available: u64 },
    /// The name of an entry is not terminated by a NUL inside the names.
    NameUnterminated { entry: u64 },
    /// BSD: the name offset of an entry is outside the names.
    NameOutOfRange { entry: u64, name_offset: u64 },
}

impl fmt::Display for IndexProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::NoCount { size } => {
                write!(f, "its {size} bytes are too few for the first word")
            }
            Self::TableOutOfRange { announced, size } => {
                write!(
                    f,
                    "the table its first word announces ({announced}) does not fit in its {size} bytes"
                )
            }
            Self::TableNotWhole { bytes } => {
                write!(f, "the table size {bytes} is not a whole number of entries")
            }
            Self::NoNamesSize => f.write_str("the size of the names is missing after the table"),
            Self::NamesOutOfRange { announced, available } => {
                write!(f, "{announced} bytes of names do not fit in the {available} that remain")
            }
            Self::NameUnterminated { entry } => {
                write!(f, "the name of entry {entry} is not terminated")
            }
            Self::NameOutOfRange { entry, name_offset } => {
                write!(f, "the name offset {name_offset} of entry {entry} is outside the names")
            }
        }
    }
}

/// Why an archive cannot be read (§8). `offset` is a byte offset in the
/// archive: of the member header the error is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The file does not start with an archive magic (§2).
    NotAnArchive,
    /// The archive ends inside a member header (§3).
    TruncatedHeader { offset: u64, available: u64 },
    /// The header does not end with `` `\n `` (§3).
    BadHeaderTerminator { offset: u64, found: [u8; 2] },
    /// The size field is not a decimal number (§3).
    BadSize { offset: u64, field: [u8; 10] },
    /// The member's bytes run past the end of the archive (§3).
    TruncatedMember { offset: u64, size: u64, available: u64 },
    /// The name field holds a NUL byte (§4.1).
    NulInName { offset: u64, field: [u8; 16] },
    /// The name field has no terminating `/` and a space inside the name
    /// (§4.1).
    AmbiguousName { offset: u64, field: [u8; 16] },
    /// The name field starts with `/` and is not `/<offset>` (§4.2).
    BadLongNameReference { offset: u64, field: [u8; 16] },
    /// A long-name reference in an archive without a long-name table
    /// (§4.2).
    NoLongNameTable { offset: u64, name_offset: u64 },
    /// A long-name reference past the end of the long-name table (§4.2).
    LongNameOutOfRange { offset: u64, name_offset: u64, table_size: u64 },
    /// The long name is not terminated by `/\n` (§4.2).
    LongNameUnterminated { offset: u64, name_offset: u64 },
    /// `/<offset>:<origin>` in a regular archive (§7.3).
    NestedMemberInRegularArchive { offset: u64, field: [u8; 16] },
    /// The name field starts with `#1/` and is not `#1/<length>` (§4.3).
    BadBsdNameLength { offset: u64, field: [u8; 16] },
    /// The BSD name is longer than the member (§4.3).
    BsdNameTooLong { offset: u64, name_len: u64, size: u64 },
    /// A BSD name in a thin archive (§4.3).
    BsdNameInThinArchive { offset: u64 },
    /// A special member after the front of the archive (§5).
    MisplacedSpecialMember { offset: u64, special: Special },
    /// A second `/` member after the symbol index: a COFF archive (§5).
    CoffArchive { offset: u64 },
    /// The symbol index whose header is at `offset` is malformed (§6).
    BadIndex { offset: u64, kind: IndexKind, problem: IndexProblem },
    /// [`Archive::member_at`]: the offset is outside the ordinary members
    /// (§7).
    NoMemberAt { offset: u64, first: u64, end: u64 },
    /// [`Archive::member_at`]: the offset is odd (§7).
    OddMemberOffset { offset: u64 },
    /// Over a parts view: bytes the reader decodes are not resident (an
    /// internal error of the store that made the view).
    NotResident { offset: u64, size: u64 },
}

impl fmt::Display for Error {
    #[allow(clippy::too_many_lines)] // one arm per error
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAnArchive => {
                f.write_str("not an archive: no \"!<arch>\\n\" or \"!<thin>\\n\" at the start")
            }
            Self::TruncatedHeader { offset, available } => write!(
                f,
                "member header at offset {offset}: the archive ends after {available} of the \
                 header's {HEADER_LEN} bytes"
            ),
            Self::BadHeaderTerminator { offset, found } => write!(
                f,
                "member header at offset {offset}: ends with \"{}\", not \"`\\n\"",
                found.escape_ascii()
            ),
            Self::BadSize { offset, field } => write!(
                f,
                "member header at offset {offset}: size field \"{}\" is not a decimal number",
                shown(field)
            ),
            Self::TruncatedMember { offset, size, available } => write!(
                f,
                "member header at offset {offset}: the member's {size} bytes run past the end \
                 of the archive, which has {available} more"
            ),
            Self::NulInName { offset, field } => write!(
                f,
                "member header at offset {offset}: name field \"{}\" contains a NUL byte",
                shown(field)
            ),
            Self::AmbiguousName { offset, field } => write!(
                f,
                "member header at offset {offset}: name field \"{}\" has a space in the name \
                 and no terminating \"/\"",
                shown(field)
            ),
            Self::BadLongNameReference { offset, field } => write!(
                f,
                "member header at offset {offset}: name field \"{}\" is not a long-name \
                 reference \"/<decimal offset>\"",
                shown(field)
            ),
            Self::NoLongNameTable { offset, name_offset } => write!(
                f,
                "member header at offset {offset}: the name is at offset {name_offset} of the \
                 long-name table, and the archive has no long-name table (\"//\")"
            ),
            Self::LongNameOutOfRange { offset, name_offset, table_size } => write!(
                f,
                "member header at offset {offset}: the name is at offset {name_offset} of the \
                 long-name table, which has {table_size} bytes"
            ),
            Self::LongNameUnterminated { offset, name_offset } => write!(
                f,
                "member header at offset {offset}: the name at offset {name_offset} of the \
                 long-name table is not terminated by \"/\\n\""
            ),
            Self::NestedMemberInRegularArchive { offset, field } => write!(
                f,
                "member header at offset {offset}: name field \"{}\" refers to a member of \
                 another archive, which only a thin archive may do",
                shown(field)
            ),
            Self::BadBsdNameLength { offset, field } => write!(
                f,
                "member header at offset {offset}: name field \"{}\" is not a BSD name \
                 \"#1/<decimal length>\"",
                shown(field)
            ),
            Self::BsdNameTooLong { offset, name_len, size } => write!(
                f,
                "member header at offset {offset}: the BSD name length {name_len} is larger \
                 than the member's size {size}"
            ),
            Self::BsdNameInThinArchive { offset } => write!(
                f,
                "member header at offset {offset}: a BSD name (\"#1/<length>\") in a thin \
                 archive"
            ),
            Self::MisplacedSpecialMember { offset, special } => write!(
                f,
                "member header at offset {offset}: {special} that is not at the start of the \
                 archive"
            ),
            Self::CoffArchive { offset } => write!(
                f,
                "member header at offset {offset}: a second \"/\" member after the symbol \
                 index: a COFF archive, which is not supported"
            ),
            Self::BadIndex { offset, kind, problem } => {
                write!(f, "symbol index at offset {offset} ({kind}): {problem}")
            }
            Self::NoMemberAt { offset, first, end } => write!(
                f,
                "no member header at offset {offset}: the members are at offsets {first} to \
                 {end}"
            ),
            Self::OddMemberOffset { offset } => {
                write!(f, "no member header at offset {offset}: member headers are at even offsets")
            }
            Self::NotResident { offset, size } => {
                write!(f, "{size} bytes at offset {offset} are not resident (an input store error)")
            }
        }
    }
}

impl std::error::Error for Error {}

/// A header field as an error message shows it: escaped, without the padding.
fn shown(field: &[u8]) -> std::slice::EscapeAscii<'_> {
    trim_spaces(field).escape_ascii()
}

/// `field` without its trailing spaces.
fn trim_spaces(field: &[u8]) -> &[u8] {
    let end = field.iter().rposition(|&b| b != b' ').map_or(0, |last| last + 1);
    &field[..end]
}

/// The value of a field of decimal digits; `None` when it is empty, holds
/// anything else, or does not fit.
fn decimal(digits: &[u8]) -> Option<u64> {
    if digits.is_empty() {
        return None;
    }
    digits.iter().try_fold(0u64, |value, &digit| {
        if !digit.is_ascii_digit() {
            return None;
        }
        value.checked_mul(10)?.checked_add(u64::from(digit - b'0'))
    })
}

/// A header field copied for an error value: `N` is the field's size.
fn array<const N: usize>(field: &[u8]) -> [u8; N] {
    let mut copy = [b' '; N];
    let len = field.len().min(N);
    copy[..len].copy_from_slice(&field[..len]);
    copy
}

#[cfg(test)]
mod tests {
    use super::{decimal, trim_spaces};

    /// `spec/archive.md` §3: a numeric field is decimal digits only.
    #[test]
    fn decimal_fields() {
        assert_eq!(decimal(b"0"), Some(0));
        assert_eq!(decimal(b"1296"), Some(1296));
        assert_eq!(decimal(b"18446744073709551615"), Some(u64::MAX));
        assert_eq!(decimal(b"18446744073709551616"), None);
        assert_eq!(decimal(b""), None);
        assert_eq!(decimal(b"12x4"), None);
        assert_eq!(decimal(b"+4"), None);
        assert_eq!(decimal(b" 4"), None);
        assert_eq!(decimal(b"-1"), None);
    }

    /// `spec/archive.md` §3: fields are padded with spaces on the right.
    #[test]
    fn space_padding() {
        assert_eq!(trim_spaces(b"12  "), b"12");
        assert_eq!(trim_spaces(b" 12"), b" 12");
        assert_eq!(trim_spaces(b"    "), b"");
        assert_eq!(trim_spaces(b"a b "), b"a b");
        assert_eq!(trim_spaces(b"12\n "), b"12\n");
    }
}

//! Archives built by hand, byte by byte, for the tests of `spec/archive.md`.
//!
//! The builder is written from the spec, not from the reader: the L1 test
//! `hand_built_fixtures_are_read_alike_by_the_tools` checks that `ar` and
//! `llvm-ar` read what it writes.

pub const REGULAR: &[u8] = b"!<arch>\n";
pub const THIN: &[u8] = b"!<thin>\n";

/// A member header (§3): `name` and `size` as the fields hold them, padded
/// with spaces; the other fields as a deterministic `ar` writes them.
pub fn header_raw(name: &[u8], size: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(60);
    for (field, width) in
        [(name, 16), (b"0".as_slice(), 12), (b"0", 6), (b"0", 6), (b"644", 8), (size, 10)]
    {
        assert!(field.len() <= width, "field {:?} is wider than {width}", field.escape_ascii());
        out.extend_from_slice(field);
        out.resize(out.len() + width - field.len(), b' ');
    }
    out.extend_from_slice(b"`\n");
    out
}

/// A member header with a decimal size.
pub fn header(name: &[u8], size: u64) -> Vec<u8> {
    header_raw(name, size.to_string().as_bytes())
}

/// A header, the member's bytes and, after an odd number of them, the
/// padding newline (§3).
pub fn member(name: &[u8], data: &[u8]) -> Vec<u8> {
    let mut out = header(name, data.len() as u64);
    out.extend_from_slice(data);
    if out.len() % 2 == 1 {
        out.push(b'\n');
    }
    out
}

/// Concatenates the parts of an archive.
pub fn concat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

/// The body of a GNU symbol index (§6.1, §6.2): `(name, member header
/// offset)` per entry, in words of 4 or 8 big-endian bytes.
pub fn gnu_index(symbols: &[(&[u8], u64)], wide: bool) -> Vec<u8> {
    let word = |value: u64| {
        if wide { value.to_be_bytes().to_vec() } else { (value as u32).to_be_bytes().to_vec() }
    };
    let mut out = word(symbols.len() as u64);
    for (_, offset) in symbols {
        out.extend(word(*offset));
    }
    for (name, _) in symbols {
        out.extend_from_slice(name);
        out.push(0);
    }
    if out.len() % 2 == 1 {
        out.push(0);
    }
    out
}

/// The body of a BSD symbol index (§6.3) in words of 4 or 8 little-endian
/// bytes, padded with NULs to a multiple of 8 bytes.
pub fn bsd_index(symbols: &[(&[u8], u64)], wide: bool) -> Vec<u8> {
    let word = |value: u64| {
        if wide { value.to_le_bytes().to_vec() } else { (value as u32).to_le_bytes().to_vec() }
    };
    let mut names = Vec::new();
    let mut table = Vec::new();
    for (name, offset) in symbols {
        table.extend(word(names.len() as u64));
        table.extend(word(*offset));
        names.extend_from_slice(name);
        names.push(0);
    }
    let mut out = word(table.len() as u64);
    out.extend(table);
    out.extend(word(names.len() as u64));
    out.extend(names);
    out.resize(out.len().next_multiple_of(8), 0);
    out
}

/// A member with a BSD name (§4.3): the name, padded with NULs so that the
/// content starts at a multiple of 8 bytes after an aligned header, then
/// the content.
pub fn bsd_member(name: &[u8], data: &[u8]) -> Vec<u8> {
    let mut stored = name.to_vec();
    stored.resize((name.len() + 3) / 8 * 8 + 4, 0);
    let field = format!("#1/{}", stored.len());
    stored.extend_from_slice(data);
    member(field.as_bytes(), &stored)
}

/// How a fixture spells member names.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Names {
    /// Short names end with `/`; names of more than 15 bytes, and names
    /// with a `/`, go to the long-name table.
    Gnu,
    /// Every name is a `#1/<length>` name.
    Bsd,
}

/// The symbol index of a fixture.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Index {
    None,
    Gnu,
    Gnu64,
    Bsd,
    Bsd64,
}

/// A fixture: the bytes of the archive and the header offset of each
/// member.
pub struct Fixture {
    pub bytes: Vec<u8>,
    pub offsets: Vec<u64>,
}

/// Builds an archive of `members` (`(name, content)`) with an index of
/// `symbols` (`(name, position of the defining member)`). A thin archive
/// stores every name in the long-name table and no content.
pub fn archive(
    thin: bool,
    names: Names,
    index: Index,
    members: &[(&[u8], &[u8])],
    symbols: &[(&[u8], usize)],
) -> Fixture {
    // Nothing's size depends on the offsets: lay out with zeros, then with
    // the offsets of that layout.
    let zeros = vec![0; members.len()];
    let draft = lay_out(thin, names, index, members, symbols, &zeros);
    let fixture = lay_out(thin, names, index, members, symbols, &draft.offsets);
    assert_eq!(fixture.offsets, draft.offsets, "the layout must not depend on the offsets");
    fixture
}

fn lay_out(
    thin: bool,
    names: Names,
    index: Index,
    members: &[(&[u8], &[u8])],
    symbols: &[(&[u8], usize)],
    offsets: &[u64],
) -> Fixture {
    let entries: Vec<(&[u8], u64)> =
        symbols.iter().map(|&(name, member)| (name, offsets[member])).collect();
    let mut bytes = if thin { THIN.to_vec() } else { REGULAR.to_vec() };
    match index {
        Index::None => {}
        Index::Gnu => bytes.extend(member(b"/", &gnu_index(&entries, false))),
        Index::Gnu64 => bytes.extend(member(b"/SYM64/", &gnu_index(&entries, true))),
        Index::Bsd => bytes.extend(bsd_member(b"__.SYMDEF", &bsd_index(&entries, false))),
        Index::Bsd64 => bytes.extend(bsd_member(b"__.SYMDEF_64", &bsd_index(&entries, true))),
    }
    let in_table =
        |name: &[u8]| names == Names::Gnu && (thin || name.len() > 15 || name.contains(&b'/'));
    let mut table = Vec::new();
    let mut fields = Vec::new();
    for &(name, _) in members {
        if in_table(name) {
            fields.push(format!("/{}", table.len()).into_bytes());
            table.extend_from_slice(name);
            table.extend_from_slice(b"/\n");
        } else {
            fields.push([name, b"/"].concat());
        }
    }
    if !table.is_empty() {
        if table.len() % 2 == 1 {
            table.push(b'\n');
        }
        bytes.extend(member(b"//", &table));
    }
    let mut placed = Vec::new();
    for (&(name, data), field) in members.iter().zip(&fields) {
        placed.push(bytes.len() as u64);
        if thin {
            bytes.extend(header(field, data.len() as u64));
        } else if names == Names::Bsd {
            bytes.extend(bsd_member(name, data));
        } else {
            bytes.extend(member(field, data));
        }
    }
    Fixture { bytes, offsets: placed }
}

/// The members every seed fixture holds: an odd size, a long name, a name
/// with a space, a name of the maximum short length, a duplicate name.
pub const SEED_MEMBERS: [(&[u8], &[u8]); 6] = [
    (b"a.o", b"AAAAAAAA"),
    (b"odd.txt", b"odd"),
    (b"a_member_name_longer_than_the_field.o", b"LONGNAME"),
    (b"sp ace.o", b"SP"),
    (b"fifteen_chars.o", b""),
    (b"a.o", b"SECOND"),
];

/// The index every seed fixture holds: the same symbol twice, two symbols
/// of one member, an empty name.
pub const SEED_SYMBOLS: [(&[u8], usize); 6] =
    [(b"alpha", 0), (b"alpha2", 0), (b"_ZN4long4nameEv", 2), (b"beta", 3), (b"", 4), (b"alpha", 5)];

/// One fixture per form the reader knows, by a name for messages.
pub fn seeds() -> Vec<(&'static str, Fixture)> {
    let build = |thin, names, index| archive(thin, names, index, &SEED_MEMBERS, &SEED_SYMBOLS);
    vec![
        ("gnu", build(false, Names::Gnu, Index::Gnu)),
        ("gnu-sym64", build(false, Names::Gnu, Index::Gnu64)),
        ("gnu-no-index", build(false, Names::Gnu, Index::None)),
        ("bsd", build(false, Names::Bsd, Index::Bsd)),
        ("bsd-64", build(false, Names::Bsd, Index::Bsd64)),
        ("bsd-names-sym64", build(false, Names::Bsd, Index::Gnu64)),
        ("thin", build(true, Names::Gnu, Index::Gnu)),
    ]
}

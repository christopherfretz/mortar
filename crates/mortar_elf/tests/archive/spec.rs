//! `spec/archive.md`, section by section, on hand-built bytes. A test's
//! name starts with the section it checks (`s4_2_` is §4.2).

use mortar_elf::archive::{
    Archive, Content, Error, IndexKind, IndexProblem, Kind, Member, Special, Symbol,
};

use crate::build::{
    Index, Names, REGULAR, SEED_MEMBERS, SEED_SYMBOLS, THIN, archive, bsd_index, bsd_member,
    concat, gnu_index, header, header_raw, member,
};

fn parse(bytes: &[u8]) -> Archive<'_> {
    Archive::parse(bytes).unwrap_or_else(|e| panic!("parse: {e}"))
}

fn members<'a>(archive: &Archive<'a>) -> Vec<Member<'a>> {
    archive.members().collect::<Result<_, _>>().unwrap_or_else(|e| panic!("members: {e}"))
}

fn names<'a>(archive: &Archive<'a>) -> Vec<&'a [u8]> {
    members(archive).iter().map(Member::name).collect()
}

fn symbols<'a>(archive: &Archive<'a>) -> Vec<Symbol<'a>> {
    let index = archive.symbol_index().expect("the archive has an index");
    index.iter().collect::<Result<_, _>>().unwrap_or_else(|e| panic!("index: {e}"))
}

/// The error of the first member that fails.
fn member_error(bytes: &[u8]) -> Error {
    let archive = parse(bytes);
    archive.members().find_map(Result::err).expect("a member must fail")
}

/// A field of an error value: the bytes, padded with spaces.
fn field<const N: usize>(text: &[u8]) -> [u8; N] {
    let mut out = [b' '; N];
    out[..text.len()].copy_from_slice(text);
    out
}

// §2 The file

#[test]
fn s2_magic_selects_regular_or_thin() {
    assert_eq!(Archive::sniff(REGULAR), Some(Kind::Regular));
    assert_eq!(Archive::sniff(THIN), Some(Kind::Thin));
    assert_eq!(parse(REGULAR).kind(), Kind::Regular);
    assert_eq!(parse(THIN).kind(), Kind::Thin);
    let regular = concat(&[REGULAR, &member(b"a.o/", b"data")]);
    assert_eq!(Archive::sniff(&regular), Some(Kind::Regular));
}

#[test]
fn s2_anything_else_is_not_an_archive() {
    let not: [&[u8]; 8] = [
        b"",
        b"!<arch>",
        b"!<arch>\r\n",
        b"!<ARCH>\n",
        b"<bigaf>\n",
        b"\x7fELF\x02\x01\x01\0",
        b"/* GNU ld script */",
        b" !<arch>\n",
    ];
    for bytes in not {
        assert_eq!(Archive::sniff(bytes), None, "{:?}", bytes.escape_ascii().to_string());
        assert_eq!(Archive::parse(bytes).unwrap_err(), Error::NotAnArchive);
    }
}

#[test]
fn s2_an_archive_of_the_magic_alone_is_empty() {
    for magic in [REGULAR, THIN] {
        let archive = parse(magic);
        assert!(archive.symbol_index().is_none());
        assert_eq!(archive.members().count(), 0);
        assert_eq!(
            archive.member_at(8).unwrap_err(),
            Error::NoMemberAt { offset: 8, first: 8, end: 8 }
        );
    }
}

// §3 Member headers

#[test]
fn s3_a_header_gives_name_size_and_content() {
    let bytes = concat(&[REGULAR, &member(b"a.o/", b"12345678"), &member(b"b.o/", b"xy")]);
    let archive = parse(&bytes);
    let found = members(&archive);
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].header_offset(), 8);
    assert_eq!(found[0].name(), b"a.o");
    assert_eq!(found[0].size(), 8);
    assert_eq!(found[0].content(), Content::Inline { offset: 68, data: b"12345678" });
    assert_eq!(found[0].data(), Some(b"12345678".as_slice()));
    assert_eq!(found[1].header_offset(), 76);
    assert_eq!(found[1].content(), Content::Inline { offset: 136, data: b"xy" });
}

#[test]
fn s3_date_owner_and_mode_are_not_interpreted() {
    let mut bytes = concat(&[REGULAR, &member(b"a.o/", b"data")]);
    // The `//` header of GNU ar leaves them blank; anything else is as good.
    bytes[8 + 16..8 + 48].fill(b' ');
    assert_eq!(names(&parse(&bytes)), [b"a.o"]);
    bytes[8 + 16..8 + 48].fill(0xff);
    assert_eq!(names(&parse(&bytes)), [b"a.o"]);
}

#[test]
fn s3_a_header_cut_short_is_an_error() {
    let whole = concat(&[REGULAR, &member(b"a.o/", b"data")]);
    for available in 1..60 {
        // As the first header: reported by `parse`.
        let error = Archive::parse(&whole[..8 + available]).unwrap_err();
        assert_eq!(error, Error::TruncatedHeader { offset: 8, available: available as u64 });
        // After a member: reported by the walk, at that header's offset.
        let bytes = concat(&[&whole, &whole[8..8 + available]]);
        let error = member_error(&bytes);
        assert_eq!(error, Error::TruncatedHeader { offset: 72, available: available as u64 });
    }
}

#[test]
fn s3_the_header_ends_with_grave_newline() {
    let mut bytes = concat(&[REGULAR, &member(b"a.o/", b"data")]);
    bytes[8 + 58] = b'\'';
    let error = Archive::parse(&bytes).unwrap_err();
    assert_eq!(error, Error::BadHeaderTerminator { offset: 8, found: *b"'\n" });
    assert_eq!(error.to_string(), "member header at offset 8: ends with \"\\'\\n\", not \"`\\n\"");
}

#[test]
fn s3_the_size_is_decimal_digits_padded_with_spaces() {
    let bad: [&[u8]; 8] = [b"12x4", b"", b" 4", b"+4", b"-1", b"0x10", b"4\0", b"1 2"];
    for size in bad {
        let bytes = concat(&[REGULAR, &header_raw(b"a.o/", size), b"data"]);
        let error = Archive::parse(&bytes).unwrap_err();
        assert_eq!(error, Error::BadSize { offset: 8, field: field(size) });
    }
    let bytes = concat(&[REGULAR, &member(b"a.o/", b"ok"), &header_raw(b"b.o/", b"12x4"), b"data"]);
    assert_eq!(
        member_error(&bytes).to_string(),
        "member header at offset 70: size field \"12x4\" is not a decimal number"
    );
    // The widest field: ten digits.
    let bytes = concat(&[REGULAR, &header_raw(b"a.o/", b"0000000004"), b"data"]);
    assert_eq!(members(&parse(&bytes))[0].size(), 4);
}

#[test]
fn s3_a_member_must_fit_in_the_archive() {
    let bytes = concat(&[REGULAR, &header(b"a.o/", 5), b"data"]);
    let error = Archive::parse(&bytes).unwrap_err();
    assert_eq!(error, Error::TruncatedMember { offset: 8, size: 5, available: 4 });
    let bytes = concat(&[REGULAR, &header(b"a.o/", 9_999_999_999), b"data"]);
    let error = Archive::parse(&bytes).unwrap_err();
    assert_eq!(error, Error::TruncatedMember { offset: 8, size: 9_999_999_999, available: 4 });
}

#[test]
fn s3_an_odd_member_is_followed_by_one_padding_byte() {
    for pad in [b'\n', 0, b'x'] {
        let bytes =
            concat(&[REGULAR, &header(b"odd/", 3), b"odd", &[pad], &member(b"even/", b"even")]);
        let archive = parse(&bytes);
        let found = members(&archive);
        assert_eq!(found[0].data(), Some(b"odd".as_slice()));
        assert_eq!(found[0].size(), 3);
        assert_eq!(found[1].header_offset(), 72);
        assert_eq!(found[1].name(), b"even");
    }
    // Without the padding the next header would be misread.
    let bytes = concat(&[REGULAR, &header(b"odd/", 3), b"odd", &member(b"even/", b"even")]);
    assert!(matches!(member_error(&bytes), Error::BadHeaderTerminator { offset: 72, .. }));
}

#[test]
fn s3_the_last_members_padding_may_be_missing() {
    let bytes = concat(&[REGULAR, &header(b"odd/", 3), b"odd"]);
    let archive = parse(&bytes);
    let found = members(&archive);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].data(), Some(b"odd".as_slice()));
}

#[test]
fn s3_bytes_after_the_last_member_are_a_truncated_header() {
    let bytes = concat(&[REGULAR, &member(b"a.o/", b"data"), b"\n"]);
    assert_eq!(member_error(&bytes), Error::TruncatedHeader { offset: 72, available: 1 });
}

// §4.1 Short names

#[test]
fn s4_1_a_short_name_ends_at_the_first_slash() {
    let bytes = concat(&[
        REGULAR,
        &member(b"a.o/", b""),
        &member(b"sp ace.o/", b""),
        &member(b"fifteen_chars.o/", b""),
        &member(b"a/b.o/", b""),
        &member(b"tab\tname/", b""),
        &member(b" lead/", b""),
        &member(b"x.o/trailing", b""),
    ]);
    let expected: [&[u8]; 7] =
        [b"a.o", b"sp ace.o", b"fifteen_chars.o", b"a", b"tab\tname", b" lead", b"x.o"];
    assert_eq!(names(&parse(&bytes)), expected);
}

#[test]
fn s4_1_a_name_without_slash_ends_at_the_padding() {
    let bytes = concat(&[
        REGULAR,
        &member(b"x.txt", b""),
        &member(b"sixteen_bytes_.o", b""),
        &member(b"", b""),
    ]);
    let expected: [&[u8]; 3] = [b"x.txt", b"sixteen_bytes_.o", b""];
    assert_eq!(names(&parse(&bytes)), expected);
}

#[test]
fn s4_1_a_space_in_a_name_without_slash_is_refused() {
    let bytes = concat(&[REGULAR, &member(b"a.o/", b""), &member(b"y z", b"")]);
    let error = member_error(&bytes);
    assert_eq!(error, Error::AmbiguousName { offset: 68, field: field(b"y z") });
    assert_eq!(
        error.to_string(),
        "member header at offset 68: name field \"y z\" has a space in the name and no \
         terminating \"/\""
    );
}

#[test]
fn s4_1_a_nul_in_the_name_field_is_refused() {
    for name in [b"ab\0cd/".as_slice(), b"ab\0"] {
        let bytes = concat(&[REGULAR, &member(b"a.o/", b""), &member(name, b"")]);
        assert_eq!(member_error(&bytes), Error::NulInName { offset: 68, field: field(name) });
    }
}

#[test]
fn s4_1_members_may_share_a_name() {
    let bytes =
        concat(&[REGULAR, &member(b"a.o/", b"one"), &member(b"b.o/", b""), &member(b"a.o/", b"3")]);
    let archive = parse(&bytes);
    let found = members(&archive);
    assert_eq!(names(&archive), [b"a.o", b"b.o", b"a.o"]);
    assert_eq!(found[0].data(), Some(b"one".as_slice()));
    assert_eq!(found[2].data(), Some(b"3".as_slice()));
    assert_ne!(found[0].header_offset(), found[2].header_offset());
    assert_eq!(archive.member_at(found[2].header_offset()).unwrap(), found[2]);
}

// §4.2 Long names

/// Three entries and, as GNU `ar` writes it, a newline to an even size.
const LONG_NAMES: &[u8] = b"a_long_member_name.o/\ndir/sub dir/b.o/\n/abs/c.o/\n\n";

/// The offset of the header that follows `front` at the start of an
/// archive.
fn offset_after(front: &[&[u8]]) -> u64 {
    (8 + front.iter().map(|part| part.len()).sum::<usize>()) as u64
}

#[test]
fn s4_2_a_reference_names_an_entry_of_the_long_name_table() {
    let bytes = concat(&[
        REGULAR,
        &member(b"//", LONG_NAMES),
        &member(b"/0", b"first"),
        &member(b"/22", b"second"),
        &member(b"/39", b"third"),
        // Into the middle of an entry: the rest of the entry.
        &member(b"/7", b"fourth"),
        &member(b"/0", b"again"),
    ]);
    let archive = parse(&bytes);
    let expected: [&[u8]; 5] = [
        b"a_long_member_name.o",
        b"dir/sub dir/b.o",
        b"/abs/c.o",
        b"member_name.o",
        b"a_long_member_name.o",
    ];
    assert_eq!(names(&archive), expected);
    assert_eq!(members(&archive)[1].data(), Some(b"second".as_slice()));
}

#[test]
fn s4_a_field_ends_at_its_first_space() {
    // GNU ar writes the first of these for a thin member whose name is 15
    // bytes long.
    let fields: [&[u8]; 3] = [b"/22            /", b"/22 /", b"/22 x:9"];
    for field in fields {
        let bytes = concat(&[REGULAR, &member(b"//", LONG_NAMES), &member(field, b"data")]);
        assert_eq!(names(&parse(&bytes)), [b"dir/sub dir/b.o"]);
    }
    // The special names and BSD names end there too.
    let bytes = concat(&[REGULAR, &member(b"// x", LONG_NAMES), &member(b"/0 x", b"data")]);
    assert_eq!(names(&parse(&bytes)), [b"a_long_member_name.o"]);
    let bytes = concat(&[REGULAR, &member(b"#1/4 x", b"a.o\0data")]);
    assert_eq!(names(&parse(&bytes)), [b"a.o"]);
}

#[test]
fn s4_2_a_reference_needs_a_long_name_table() {
    let bytes = concat(&[REGULAR, &member(b"a.o/", b""), &member(b"/0", b"")]);
    let error = member_error(&bytes);
    assert_eq!(error, Error::NoLongNameTable { offset: 68, name_offset: 0 });
    // As the first member: `parse` does not look names up, the walk does.
    let bytes = concat(&[REGULAR, &member(b"/0", b"")]);
    assert_eq!(member_error(&bytes), Error::NoLongNameTable { offset: 8, name_offset: 0 });
}

#[test]
fn s4_2_a_reference_must_be_inside_the_table() {
    let table_size = LONG_NAMES.len() as u64;
    for name_offset in [table_size, table_size + 1, 999_999_999_999_999] {
        let reference = format!("/{name_offset}");
        let bytes =
            concat(&[REGULAR, &member(b"//", LONG_NAMES), &member(reference.as_bytes(), b"")]);
        let offset = offset_after(&[&member(b"//", LONG_NAMES)]);
        let error = member_error(&bytes);
        assert_eq!(error, Error::LongNameOutOfRange { offset, name_offset, table_size });
    }
}

#[test]
fn s4_2_an_entry_ends_with_slash_newline() {
    let tables: [&[u8]; 4] =
        [b"no_terminator_at_all.o", b"no_newline_after.o/ ", b"no_slash_before.o\n\n", b"\n\n"];
    for table in tables {
        let bytes = concat(&[REGULAR, &member(b"//", table), &member(b"/0", b"")]);
        let offset = offset_after(&[&member(b"//", table)]);
        assert_eq!(member_error(&bytes), Error::LongNameUnterminated { offset, name_offset: 0 });
    }
    // A reference to the newline that ends an entry is not an empty name.
    let table = member(b"//", b"abcdefghijklmnopq.o/\n");
    let bytes = concat(&[REGULAR, &table, &member(b"/20", b"")]);
    let offset = offset_after(&[&table]);
    assert_eq!(member_error(&bytes), Error::LongNameUnterminated { offset, name_offset: 20 });
}

#[test]
fn s4_2_a_name_field_starting_with_slash_is_a_reference() {
    let bad: [&[u8]; 5] = [b"/x", b"/12x", b"/1/2", b"/-1", b"/foo.o/"];
    for name in bad {
        let bytes = concat(&[REGULAR, &member(b"//", LONG_NAMES), &member(name, b"")]);
        let offset = offset_after(&[&member(b"//", LONG_NAMES)]);
        let error = member_error(&bytes);
        assert_eq!(error, Error::BadLongNameReference { offset, field: field(name) });
    }
}

// §4.3 BSD names

#[test]
fn s4_3_a_bsd_name_is_the_start_of_the_members_bytes() {
    let bytes = concat(&[
        REGULAR,
        &member(b"#1/4", b"a.o\0CONTENT"),
        &member(b"#1/12", b"sp ace.o\0\0\0\0odd"),
        // No padding: the name is all of its bytes.
        &member(b"#1/20", b"twenty_bytes_long_.o"),
        &member(b"#1/0", b"empty name"),
    ]);
    let archive = parse(&bytes);
    let found = members(&archive);
    let expected: [&[u8]; 4] = [b"a.o", b"sp ace.o", b"twenty_bytes_long_.o", b""];
    assert_eq!(names(&archive), expected);
    assert_eq!(found[0].size(), 7);
    assert_eq!(found[0].content(), Content::Inline { offset: 8 + 60 + 4, data: b"CONTENT" });
    assert_eq!(found[1].data(), Some(b"odd".as_slice()));
    assert_eq!(found[2].size(), 0);
    assert_eq!(found[3].data(), Some(b"empty name".as_slice()));
}

#[test]
fn s4_3_a_bsd_name_length_is_decimal_and_at_most_the_size() {
    let bad: [&[u8]; 4] = [b"#1/", b"#1/x", b"#1/4x", b"#1/ 4"];
    for name in bad {
        let bytes = concat(&[REGULAR, &member(name, b"a.o\0data")]);
        let error = Archive::parse(&bytes).unwrap_err();
        assert_eq!(error, Error::BadBsdNameLength { offset: 8, field: field(name) });
    }
    let bytes = concat(&[REGULAR, &member(b"#1/9", b"a.o\0data")]);
    let error = Archive::parse(&bytes).unwrap_err();
    assert_eq!(error, Error::BsdNameTooLong { offset: 8, name_len: 9, size: 8 });
    let bytes = concat(&[REGULAR, &member(b"#1/9999999999999", b"a.o\0data")]);
    let error = Archive::parse(&bytes).unwrap_err();
    assert_eq!(error, Error::BsdNameTooLong { offset: 8, name_len: 9_999_999_999_999, size: 8 });
}

#[test]
fn s4_3_a_bsd_name_in_a_thin_archive_is_refused() {
    let bytes = concat(&[THIN, &header(b"#1/4", 8)]);
    assert_eq!(Archive::parse(&bytes).unwrap_err(), Error::BsdNameInThinArchive { offset: 8 });
}

// §5 Special members

#[test]
fn s5_special_members_are_not_members() {
    let index = gnu_index(&[(b"sym", 0)], false);
    let bytes = concat(&[
        REGULAR,
        &member(b"/", &index),
        &member(b"//", LONG_NAMES),
        &member(b"a.o/", b"data"),
        &member(b"/0", b"more"),
    ]);
    let archive = parse(&bytes);
    assert_eq!(names(&archive), [b"a.o".as_slice(), b"a_long_member_name.o"]);
    let first = members(&archive)[0].header_offset();
    assert_eq!(first, (8 + 60 + index.len() + 60 + LONG_NAMES.len()) as u64);
    // Not through the index either.
    for special in [8, 8 + 60 + index.len() as u64] {
        let error = archive.member_at(special).unwrap_err();
        assert_eq!(error, Error::NoMemberAt { offset: special, first, end: bytes.len() as u64 });
    }
}

#[test]
fn s5_the_long_name_table_may_come_first() {
    let bytes = concat(&[REGULAR, &member(b"//", LONG_NAMES), &member(b"/0", b"data")]);
    let archive = parse(&bytes);
    assert!(archive.symbol_index().is_none());
    assert_eq!(names(&archive), [b"a_long_member_name.o"]);
}

#[test]
fn s5_an_archive_may_hold_special_members_only() {
    let bytes = concat(&[REGULAR, &member(b"/", &gnu_index(&[], false))]);
    let archive = parse(&bytes);
    assert_eq!(archive.symbol_index().map(|index| index.len()), Some(0));
    assert_eq!(archive.members().count(), 0);
    let bytes = concat(&[REGULAR, &member(b"/", &gnu_index(&[], false)), &member(b"//", b"")]);
    assert_eq!(parse(&bytes).members().count(), 0);
}

#[test]
fn s5_a_special_member_elsewhere_is_refused() {
    let index = gnu_index(&[], false);
    let bsd = bsd_index(&[], false);
    let late: [(Vec<u8>, Special); 7] = [
        (member(b"/", &index), Special::Index(IndexKind::Gnu)),
        (member(b"/SYM64/", &gnu_index(&[], true)), Special::Index(IndexKind::Gnu64)),
        (member(b"//", LONG_NAMES), Special::LongNames),
        (member(b"__.SYMDEF", &bsd), Special::Index(IndexKind::Bsd)),
        (member(b"__.SYMDEF SORTED", &bsd), Special::Index(IndexKind::Bsd)),
        (bsd_member(b"__.SYMDEF SORTED", &bsd), Special::Index(IndexKind::Bsd)),
        (bsd_member(b"__.SYMDEF_64", &bsd_index(&[], true)), Special::Index(IndexKind::Bsd64)),
    ];
    for (late_member, special) in &late {
        let bytes = concat(&[REGULAR, &member(b"a.o/", b"data"), late_member]);
        let error = member_error(&bytes);
        assert_eq!(error, Error::MisplacedSpecialMember { offset: 72, special: *special });
    }
    // An index after the long-name table is late too.
    let bytes = concat(&[REGULAR, &member(b"//", b""), &member(b"/", &index)]);
    let error = member_error(&bytes);
    let special = Special::Index(IndexKind::Gnu);
    assert_eq!(error, Error::MisplacedSpecialMember { offset: 68, special });
    assert_eq!(
        error.to_string(),
        "member header at offset 68: a symbol index (GNU, \"/\") that is not at the start of \
         the archive"
    );
}

#[test]
fn s5_a_second_index_member_is_a_coff_archive() {
    let index = gnu_index(&[], false);
    for first in [b"/".as_slice(), b"/SYM64/"] {
        let wide = gnu_index(&[], first.len() > 1);
        let bytes =
            concat(&[REGULAR, &member(first, &wide), &member(b"/", &index), &member(b"a.o/", b"")]);
        let offset = (8 + 60 + wide.len()) as u64;
        assert_eq!(Archive::parse(&bytes).unwrap_err(), Error::CoffArchive { offset });
    }
}

#[test]
fn s5_parse_reports_the_headers_it_examines() {
    let index = gnu_index(&[], false);
    // The header after the index is examined to see whether it is the
    // long-name table.
    let bytes = concat(&[REGULAR, &member(b"/", &index), &header_raw(b"a.o/", b"x")]);
    assert!(matches!(Archive::parse(&bytes).unwrap_err(), Error::BadSize { offset: 72, .. }));
    // The one after that is not.
    let bytes =
        concat(&[REGULAR, &member(b"/", &index), &member(b"//", b""), &header_raw(b"a.o/", b"x")]);
    assert!(matches!(member_error(&bytes), Error::BadSize { offset: 132, .. }));
    // Neither is the second header of an archive without an index.
    let bytes = concat(&[REGULAR, &member(b"a.o/", b""), &header_raw(b"b.o/", b"x")]);
    assert!(matches!(member_error(&bytes), Error::BadSize { offset: 68, .. }));
    // The first header is examined, and its name with it; only the lookup
    // of a long name is left to the walk.
    let bytes = concat(&[REGULAR, &member(b"/x", b"")]);
    let error = Archive::parse(&bytes).unwrap_err();
    assert_eq!(error, Error::BadLongNameReference { offset: 8, field: field(b"/x") });
    let bytes = concat(&[REGULAR, &member(b"y z", b"")]);
    let error = Archive::parse(&bytes).unwrap_err();
    assert_eq!(error, Error::AmbiguousName { offset: 8, field: field(b"y z") });
}

// §6 The symbol index

fn seed(names: Names, index: Index) -> crate::build::Fixture {
    archive(false, names, index, &SEED_MEMBERS, &SEED_SYMBOLS)
}

/// The index of a seed fixture gives the seed's symbols, each with the
/// header offset of its member, and `member_at` finds that member.
fn check_seed_index(names: Names, index: Index, kind: IndexKind) {
    let fixture = seed(names, index);
    let archive = parse(&fixture.bytes);
    let index = archive.symbol_index().expect("an index");
    assert_eq!(index.kind(), kind);
    assert_eq!(index.len(), SEED_SYMBOLS.len());
    assert!(!index.is_empty());
    let found = symbols(&archive);
    assert_eq!(found.len(), SEED_SYMBOLS.len());
    for (entry, (symbol, (name, member))) in found.iter().zip(SEED_SYMBOLS).enumerate() {
        assert_eq!(symbol.name, name);
        assert_eq!(symbol.member_offset, fixture.offsets[member]);
        assert_eq!(index.member_offset(entry), Some(symbol.member_offset));
        let defined_by = archive.member_at(symbol.member_offset).unwrap();
        assert_eq!(defined_by.name(), SEED_MEMBERS[member].0);
        assert_eq!(defined_by.data(), Some(SEED_MEMBERS[member].1));
    }
    assert_eq!(index.member_offset(SEED_SYMBOLS.len()), None);
    let mut entries = 0;
    for symbol in &index {
        assert!(symbol.is_ok());
        entries += 1;
    }
    assert_eq!(entries, SEED_SYMBOLS.len());
    let listed: Vec<&[u8]> = SEED_MEMBERS.iter().map(|&(name, _)| name).collect();
    assert_eq!(self::names(&archive), listed);
}

#[test]
fn s6_1_gnu_index() {
    check_seed_index(Names::Gnu, Index::Gnu, IndexKind::Gnu);
    // The words are big-endian: 1 entry, at offset 0x0102.
    let body = [0, 0, 0, 1, 0, 0, 1, 2, b's', 0];
    let bytes = concat(&[REGULAR, &member(b"/", &body)]);
    let expected = [Symbol { name: b"s", member_offset: 0x0102 }];
    assert_eq!(symbols(&parse(&bytes)), expected);
}

#[test]
fn s6_1_bytes_after_the_last_name_are_ignored() {
    let mut body = gnu_index(&[(b"one", 100), (b"two", 200)], false);
    body.extend_from_slice(b"\0\0padding");
    let bytes = concat(&[REGULAR, &member(b"/", &body)]);
    let expected =
        [Symbol { name: b"one", member_offset: 100 }, Symbol { name: b"two", member_offset: 200 }];
    assert_eq!(symbols(&parse(&bytes)), expected);
}

#[test]
fn s6_1_an_index_may_be_empty() {
    let bytes = concat(&[REGULAR, &member(b"/", &[0, 0, 0, 0]), &member(b"a.o/", b"")]);
    let archive = parse(&bytes);
    let index = archive.symbol_index().expect("an empty index is an index");
    assert!(index.is_empty());
    assert_eq!(index.iter().count(), 0);
    assert_eq!(index.member_offset(0), None);
}

#[test]
fn s6_2_gnu_64_bit_index() {
    check_seed_index(Names::Gnu, Index::Gnu64, IndexKind::Gnu64);
    check_seed_index(Names::Bsd, Index::Gnu64, IndexKind::Gnu64);
    // Offsets past 4 GiB are what the form is for.
    let body = gnu_index(&[(b"far", 0x0001_0000_0000_0010)], true);
    let bytes = concat(&[REGULAR, &member(b"/SYM64/", &body)]);
    let archive = parse(&bytes);
    assert_eq!(symbols(&archive), [Symbol { name: b"far", member_offset: 0x0001_0000_0000_0010 }]);
    let error = archive.member_at(0x0001_0000_0000_0010).unwrap_err();
    assert!(matches!(error, Error::NoMemberAt { offset: 0x0001_0000_0000_0010, .. }));
}

#[test]
fn s6_3_bsd_index() {
    check_seed_index(Names::Bsd, Index::Bsd, IndexKind::Bsd);
    check_seed_index(Names::Bsd, Index::Bsd64, IndexKind::Bsd64);
    // The words are little-endian; entries address their names.
    let entries: [(&[u8], u64); 2] = [(b"one", 0x0102), (b"two", 0x0304)];
    let body = bsd_index(&entries, false);
    assert_eq!(&body[..12], [16, 0, 0, 0, 0, 0, 0, 0, 2, 1, 0, 0]);
    let expected = [
        Symbol { name: b"one", member_offset: 0x0102 },
        Symbol { name: b"two", member_offset: 0x0304 },
    ];
    // Every spelling of the member's name.
    let spellings = [
        member(b"__.SYMDEF", &body),
        member(b"__.SYMDEF/", &body),
        member(b"__.SYMDEF SORTED", &body),
        bsd_member(b"__.SYMDEF", &body),
        bsd_member(b"__.SYMDEF SORTED", &body),
    ];
    for spelling in &spellings {
        let bytes = concat(&[REGULAR, spelling, &member(b"a.o", b"data")]);
        let archive = parse(&bytes);
        assert_eq!(archive.symbol_index().map(|index| index.kind()), Some(IndexKind::Bsd));
        assert_eq!(symbols(&archive), expected);
        assert_eq!(names(&archive), [b"a.o"]);
    }
    // In a thin archive the index is stored like any special member.
    let bytes = concat(&[THIN, &member(b"__.SYMDEF", &body), &header(b"a.o/", 4)]);
    let archive = parse(&bytes);
    assert_eq!(symbols(&archive), expected);
    assert_eq!(members(&archive)[0].content(), Content::External { path: b"a.o" });
    let wide = bsd_index(&entries, true);
    for spelling in [
        member(b"__.SYMDEF_64", &wide),
        bsd_member(b"__.SYMDEF_64", &wide),
        bsd_member(b"__.SYMDEF_64 SORTED", &wide),
    ] {
        let bytes = concat(&[REGULAR, &spelling]);
        let archive = parse(&bytes);
        assert_eq!(archive.symbol_index().map(|index| index.kind()), Some(IndexKind::Bsd64));
        assert_eq!(symbols(&archive), expected);
    }
}

#[test]
fn s6_3_bsd_entries_may_share_and_reorder_names() {
    // Two entries with one name, and a table that is not in name order.
    let mut body = Vec::new();
    body.extend_from_slice(&24u32.to_le_bytes());
    for (name_offset, member_offset) in [(4u32, 300u32), (0, 100), (4, 200)] {
        body.extend_from_slice(&name_offset.to_le_bytes());
        body.extend_from_slice(&member_offset.to_le_bytes());
    }
    body.extend_from_slice(&8u32.to_le_bytes());
    body.extend_from_slice(b"one\0two\0");
    let bytes = concat(&[REGULAR, &bsd_member(b"__.SYMDEF", &body)]);
    let expected = [
        Symbol { name: b"two", member_offset: 300 },
        Symbol { name: b"one", member_offset: 100 },
        Symbol { name: b"two", member_offset: 200 },
    ];
    assert_eq!(symbols(&parse(&bytes)), expected);
}

#[test]
fn s6_4_no_index_is_not_an_error() {
    let fixture = seed(Names::Gnu, Index::None);
    let archive = parse(&fixture.bytes);
    assert!(archive.symbol_index().is_none());
    assert_eq!(members(&archive).len(), SEED_MEMBERS.len());
    for (member, offset) in members(&archive).iter().zip(&fixture.offsets) {
        assert_eq!(member.header_offset(), *offset);
    }
}

fn index_error(name: &[u8], body: &[u8]) -> Error {
    let bytes = concat(&[REGULAR, &member(name, body)]);
    match Archive::parse(&bytes) {
        Err(error) => error,
        Ok(archive) => archive
            .symbol_index()
            .expect("an index")
            .iter()
            .find_map(Result::err)
            .expect("the index must fail"),
    }
}

#[test]
fn s6_5_a_gnu_index_must_hold_what_it_announces() {
    let bad = |kind, problem| Error::BadIndex { offset: 8, kind, problem };
    for size in 0..4 {
        let error = index_error(b"/", &[0; 3][..size]);
        assert_eq!(error, bad(IndexKind::Gnu, IndexProblem::NoCount { size: size as u64 }));
    }
    assert_eq!(
        index_error(b"/SYM64/", &[0; 7]),
        bad(IndexKind::Gnu64, IndexProblem::NoCount { size: 7 })
    );
    // One entry too many for the member, and counts whose table size
    // overflows.
    let error = index_error(b"/", &[0, 0, 0, 2, 0, 0, 0, 8]);
    assert_eq!(error, bad(IndexKind::Gnu, IndexProblem::TableOutOfRange { announced: 2, size: 8 }));
    assert_eq!(
        error.to_string(),
        "symbol index at offset 8 (GNU, \"/\"): the table its first word announces (2) does \
         not fit in its 8 bytes"
    );
    let error = index_error(b"/", &[0xff; 8]);
    let problem = IndexProblem::TableOutOfRange { announced: 0xffff_ffff, size: 8 };
    assert_eq!(error, bad(IndexKind::Gnu, problem));
    for count in [u64::MAX, 1 << 61, 1 << 62, 3] {
        let mut body = count.to_be_bytes().to_vec();
        body.extend_from_slice(&[0; 16]);
        let problem = IndexProblem::TableOutOfRange { announced: count, size: 24 };
        assert_eq!(index_error(b"/SYM64/", &body), bad(IndexKind::Gnu64, problem));
    }
}

#[test]
fn s6_5_a_name_must_be_terminated() {
    // Three entries, names for two and a half.
    let body = concat(&[&[0, 0, 0, 3], &[0; 12], b"one\0two\0thr"]);
    let bytes = concat(&[REGULAR, &member(b"/", &body)]);
    let archive = parse(&bytes);
    let index = archive.symbol_index().unwrap();
    assert_eq!(index.len(), 3);
    let mut entries = index.iter();
    assert_eq!(entries.next(), Some(Ok(Symbol { name: b"one", member_offset: 0 })));
    assert_eq!(entries.next(), Some(Ok(Symbol { name: b"two", member_offset: 0 })));
    let problem = IndexProblem::NameUnterminated { entry: 2 };
    let error = Error::BadIndex { offset: 8, kind: IndexKind::Gnu, problem };
    assert_eq!(entries.next(), Some(Err(error)));
    assert_eq!(
        error.to_string(),
        "symbol index at offset 8 (GNU, \"/\"): the name of entry 2 is not terminated"
    );
    // The iterator ends after the error.
    assert_eq!(entries.next(), None);
    assert_eq!(entries.next(), None);
}

#[test]
fn s6_5_a_bsd_index_must_hold_what_it_announces() {
    let bad = |problem| Error::BadIndex { offset: 8, kind: IndexKind::Bsd, problem };
    let words = |words: &[u32]| -> Vec<u8> { words.iter().flat_map(|w| w.to_le_bytes()).collect() };
    assert_eq!(index_error(b"__.SYMDEF", &[0; 3]), bad(IndexProblem::NoCount { size: 3 }));
    let error = index_error(b"__.SYMDEF", &words(&[16, 0, 0, 0]));
    assert_eq!(error, bad(IndexProblem::TableOutOfRange { announced: 16, size: 16 }));
    let error = index_error(b"__.SYMDEF", &words(&[12, 0, 0, 0, 0]));
    assert_eq!(error, bad(IndexProblem::TableNotWhole { bytes: 12 }));
    let error = index_error(b"__.SYMDEF", &words(&[8, 0, 0]));
    assert_eq!(error, bad(IndexProblem::NoNamesSize));
    let error = index_error(b"__.SYMDEF", &words(&[8, 0, 0, 5, 0]));
    assert_eq!(error, bad(IndexProblem::NamesOutOfRange { announced: 5, available: 4 }));
    // An entry whose name is outside the names, and one that runs off them.
    let body = concat(&[&words(&[8, 4, 0, 4]), b"abc\0"]);
    let error = index_error(b"__.SYMDEF", &body);
    assert_eq!(error, bad(IndexProblem::NameOutOfRange { entry: 0, name_offset: 4 }));
    let body = concat(&[&words(&[8, 1, 0, 4]), b"abcd"]);
    let error = index_error(b"__.SYMDEF", &body);
    assert_eq!(error, bad(IndexProblem::NameUnterminated { entry: 0 }));
    // 64-bit sizes that do not fit anything.
    let body = concat(&[&u64::MAX.to_le_bytes(), &[0; 16]]);
    let problem = IndexProblem::TableOutOfRange { announced: u64::MAX, size: 24 };
    let error = Error::BadIndex { offset: 8, kind: IndexKind::Bsd64, problem };
    assert_eq!(index_error(b"__.SYMDEF_64", &body), error);
}

#[test]
fn s6_6_an_index_of_a_hundred_thousand_symbols() {
    let count = 100_000u32;
    let mut body = count.to_be_bytes().to_vec();
    for entry in 0..count {
        body.extend_from_slice(&(entry * 2).to_be_bytes());
    }
    for entry in 0..count {
        body.extend_from_slice(format!("_ZN6symbol{entry}Ev\0").as_bytes());
    }
    let bytes = concat(&[REGULAR, &member(b"/", &body), &member(b"a.o/", b"")]);
    let archive = parse(&bytes);
    let index = archive.symbol_index().unwrap();
    assert_eq!(index.len(), 100_000);
    assert_eq!(index.member_offset(99_999), Some(199_998));
    let mut seen = 0u32;
    for symbol in index {
        let symbol = symbol.unwrap();
        assert_eq!(symbol.member_offset, u64::from(seen) * 2);
        assert!(symbol.name.starts_with(b"_ZN6symbol"));
        seen += 1;
    }
    assert_eq!(seen, count);
    let last = index.iter().nth(99_999).unwrap().unwrap();
    assert_eq!(last.name, b"_ZN6symbol99999Ev");
}

// §7 Members

#[test]
fn s7_1_member_at_gives_what_the_walk_gives() {
    for (label, fixture) in crate::build::seeds() {
        let archive = parse(&fixture.bytes);
        let walked = members(&archive);
        assert_eq!(walked.len(), SEED_MEMBERS.len(), "{label}");
        for (member, offset) in walked.iter().zip(&fixture.offsets) {
            assert_eq!(member.header_offset(), *offset, "{label}");
            assert_eq!(archive.member_at(*offset).unwrap(), *member, "{label}");
        }
    }
}

#[test]
fn s7_1_member_at_refuses_what_is_not_a_member_header() {
    let fixture = seed(Names::Gnu, Index::Gnu);
    let archive = parse(&fixture.bytes);
    let first = fixture.offsets[0];
    let end = fixture.bytes.len() as u64;
    for offset in [0, 8, first - 2, end, end + 2, u64::MAX] {
        let error = archive.member_at(offset).unwrap_err();
        assert_eq!(error, Error::NoMemberAt { offset, first, end });
    }
    assert_eq!(
        archive.member_at(end).unwrap_err().to_string(),
        format!("no member header at offset {end}: the members are at offsets {first} to {end}")
    );
    let error = archive.member_at(first + 1).unwrap_err();
    assert_eq!(error, Error::OddMemberOffset { offset: first + 1 });
    // An even offset inside a member is read as a header, and is not one.
    let error = archive.member_at(first + 2).unwrap_err();
    assert!(matches!(error, Error::BadHeaderTerminator { .. }), "{error}");
}

#[test]
fn s7_2_a_thin_archive_names_files() {
    let table = b"a.o/\nsub/dir/b.o/\n/abs/c.o/\n";
    let bytes = concat(&[
        THIN,
        // The second member's header: 8 + (60 + 12) + (60 + 28) + 60.
        &member(b"/", &gnu_index(&[(b"sym", 228)], false)),
        &member(b"//", table),
        &header(b"/0", 1296),
        &header(b"/5", 3),
        &header(b"/18", 9_999_999_999),
    ]);
    let archive = parse(&bytes);
    assert_eq!(archive.kind(), Kind::Thin);
    let found = members(&archive);
    let expected: [(&[u8], u64); 3] =
        [(b"a.o", 1296), (b"sub/dir/b.o", 3), (b"/abs/c.o", 9_999_999_999)];
    assert_eq!(found.len(), 3);
    for (position, (member, (path, size))) in found.iter().zip(expected).enumerate() {
        assert_eq!(member.name(), path);
        assert_eq!(member.size(), size);
        assert_eq!(member.content(), Content::External { path });
        assert_eq!(member.data(), None);
        // Headers follow one another: no content, no padding.
        assert_eq!(member.header_offset(), found[0].header_offset() + 60 * position as u64);
    }
    let symbol = symbols(&archive)[0];
    assert_eq!(archive.member_at(symbol.member_offset).unwrap().name(), b"sub/dir/b.o");
}

#[test]
fn s7_2_a_thin_member_may_have_a_short_name() {
    let bytes = concat(&[THIN, &header(b"a.o/", 7), &header(b"b.o", 9)]);
    let archive = parse(&bytes);
    let found = members(&archive);
    assert_eq!(found[0].content(), Content::External { path: b"a.o" });
    assert_eq!(found[1].content(), Content::External { path: b"b.o" });
    assert_eq!(found[1].size(), 9);
}

#[test]
fn s7_3_a_thin_archive_may_name_a_member_of_another_archive() {
    let table = b"inner.a/\nb.o/\n\n";
    let bytes = concat(&[
        THIN,
        &member(b"//", table),
        &header(b"/0:102", 1296),
        &header(b"/0:1458", 1240),
        &header(b"/9", 5),
    ]);
    let archive = parse(&bytes);
    let found = members(&archive);
    assert_eq!(found[0].name(), b"inner.a");
    assert_eq!(found[0].size(), 1296);
    let nested = Content::ExternalMember { archive_path: b"inner.a", header_offset: 102 };
    assert_eq!(found[0].content(), nested);
    let nested = Content::ExternalMember { archive_path: b"inner.a", header_offset: 1458 };
    assert_eq!(found[1].content(), nested);
    assert_eq!(found[1].data(), None);
    assert_eq!(found[2].content(), Content::External { path: b"b.o" });
    for name in [b"/0:".as_slice(), b"/0:x", b"/:102", b"/0:1:2"] {
        let bytes = concat(&[THIN, &member(b"//", table), &header(name, 1)]);
        let error = member_error(&bytes);
        assert_eq!(error, Error::BadLongNameReference { offset: 84, field: field(name) });
    }
}

#[test]
fn s7_3_only_a_thin_archive_may_do_so() {
    let bytes = concat(&[REGULAR, &member(b"//", b"inner.a/\n"), &member(b"/0:102", b"data")]);
    let error = member_error(&bytes);
    assert_eq!(error, Error::NestedMemberInRegularArchive { offset: 78, field: field(b"/0:102") });
}

// §8 Errors

#[test]
fn s8_every_error_says_what_and_where() {
    let name = field::<16>(b"/x");
    let kind = IndexKind::Bsd64;
    let problems = [
        IndexProblem::NoCount { size: 1 },
        IndexProblem::TableOutOfRange { announced: 9, size: 8 },
        IndexProblem::TableNotWhole { bytes: 9 },
        IndexProblem::NoNamesSize,
        IndexProblem::NamesOutOfRange { announced: 9, available: 1 },
        IndexProblem::NameUnterminated { entry: 3 },
        IndexProblem::NameOutOfRange { entry: 3, name_offset: 9 },
    ];
    let mut errors = vec![
        Error::TruncatedHeader { offset: 1234, available: 7 },
        Error::BadHeaderTerminator { offset: 1234, found: *b"xy" },
        Error::BadSize { offset: 1234, field: field(b"12x4") },
        Error::TruncatedMember { offset: 1234, size: 9, available: 1 },
        Error::NulInName { offset: 1234, field: name },
        Error::AmbiguousName { offset: 1234, field: name },
        Error::BadLongNameReference { offset: 1234, field: name },
        Error::NoLongNameTable { offset: 1234, name_offset: 5 },
        Error::LongNameOutOfRange { offset: 1234, name_offset: 5, table_size: 2 },
        Error::LongNameUnterminated { offset: 1234, name_offset: 5 },
        Error::NestedMemberInRegularArchive { offset: 1234, field: name },
        Error::BadBsdNameLength { offset: 1234, field: name },
        Error::BsdNameTooLong { offset: 1234, name_len: 9, size: 8 },
        Error::BsdNameInThinArchive { offset: 1234 },
        Error::MisplacedSpecialMember { offset: 1234, special: Special::LongNames },
        Error::CoffArchive { offset: 1234 },
        Error::NoMemberAt { offset: 1234, first: 8, end: 100 },
        Error::OddMemberOffset { offset: 1234 },
    ];
    errors.extend(problems.map(|problem| Error::BadIndex { offset: 1234, kind, problem }));
    let mut messages: Vec<String> = errors.iter().map(ToString::to_string).collect();
    for message in &messages {
        assert!(message.contains("at offset 1234"), "{message}");
        assert!(!message.ends_with('.') && !message.contains('\n'), "{message}");
    }
    messages.push(Error::NotAnArchive.to_string());
    assert_eq!(
        messages.last().unwrap(),
        "not an archive: no \"!<arch>\\n\" or \"!<thin>\\n\" at the start"
    );
    // No two errors read alike.
    messages.sort();
    messages.dedup();
    assert_eq!(messages.len(), errors.len() + 1);
    assert_eq!(
        Error::BadSize { offset: 1234, field: field(b"12x4") }.to_string(),
        "member header at offset 1234: size field \"12x4\" is not a decimal number"
    );
    let error: Box<dyn std::error::Error> = Box::new(Error::NotAnArchive);
    assert!(error.source().is_none());
}

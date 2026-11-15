//! The format's tests (`spec/meta.md` §12): framing round trips, every
//! malformation of MT-14, forward compatibility (MT-25, MT-35, MT-36),
//! determinism (MT-38).

use mortar_meta::read::{BlockFault, FileReader, Malformation, SectionAttrs};
use mortar_meta::write::{Declaration, Definition, RawBlock, Summary, Type, section};
use mortar_meta::{Finding, UnitBuilder, class, flags, fnv1a64, kind, producer, read_section};

type Mutation = (&'static str, Box<dyn Fn(&mut Vec<u8>)>);

fn attrs() -> SectionAttrs {
    SectionAttrs::conforming(5)
}

fn s(x: &str) -> Vec<u8> {
    x.as_bytes().to_vec()
}

/// A unit with rows of every kind, given out of order.
fn sample(seed: u64) -> UnitBuilder {
    let mut b = UnitBuilder::new(producer::TEST);
    b.summary.push(Summary { name: s("std"), value: s("c++20"), flags: 0 });
    b.summary.push(Summary { name: s("abi.char"), value: s("signed"), flags: flags::ENFORCE });
    b.summary.push(Summary { name: s("producer"), value: s("test"), flags: 0 });
    for (i, n) in ["_Z1fv", "_Z1gv", "_ZN2ns1hEi", "_Z1fv"].iter().enumerate() {
        b.definitions.push(Definition {
            key: fnv1a64(n.as_bytes()),
            hash: [i as u8 ^ seed as u8; 16],
            name: (i % 2 == 1).then(|| s(n)),
            class: class::FUNCTION,
            flags: flags::VAGUE,
            module: Vec::new(),
            file: s("a.cc"),
            line: 10 + i as u32,
        });
    }
    let mut t = Type::named(b"N2ns1SE");
    t.size = 8;
    t.align = 4;
    t.file = s("a.h");
    b.types.push(t);
    b.types.push(Type::named(b"N2ns1TE"));
    b.declarations.push(Declaration {
        key: fnv1a64(b"_Z1gv"),
        ty: 1,
        size: 0,
        name: None,
        type_name: s("FvvE"),
        flags: 0,
    });
    b.declarations.push(Declaration {
        key: fnv1a64(b"_Z1gv"),
        ty: 1,
        size: 0,
        name: None,
        type_name: s("FvvE"),
        flags: flags::DECL_DEFINITION,
    });
    b
}

fn malformed(f: &[Finding]) -> Vec<&Malformation> {
    f.iter()
        .filter_map(|f| match f {
            Finding::Malformed { what, .. } => Some(what),
            Finding::Version { .. } => None,
        })
        .collect()
}

// spec: meta.md T-MT-10, T-MT-11, T-MT-12 (framing round trip)
#[test]
fn round_trip() {
    let bytes = sample(0).finish().unwrap();
    assert_eq!(bytes.len() % 8, 0);
    let (units, findings) = read_section(attrs(), &bytes);
    assert!(findings.is_empty(), "{findings:?}");
    let [u] = units.as_slice() else { panic!("{units:?}") };
    assert_eq!(u.producer, producer::TEST);
    assert_eq!(u.offset, 0);
    assert_eq!(u.section, 5);
    assert_eq!(u.summary().len(), 3);
    assert_eq!(u.summary_value(b"std"), Some((b"c++20".as_slice(), 0)));
    assert_eq!(u.summary_value(b"abi.char"), Some((b"signed".as_slice(), flags::ENFORCE)));
    assert_eq!(u.summary_value(b"nope"), None);
    let defs = u.definitions();
    assert_eq!(defs.len(), 4);
    let r = defs.with_key(fnv1a64(b"_Z1fv"));
    assert_eq!(r.len(), 2, "two rows of one key: anchored and named");
    let names: Vec<&[u8]> = r.map(|i| u.string(defs.get(i).unwrap().name)).collect();
    assert_eq!(names, [b"".as_slice(), b"_Z1fv"], "sorted by key, then name bytes");
    assert_eq!(u.types().len(), 2);
    let t = u.types().get(u.types().with_key(fnv1a64(b"N2ns1SE")).start).unwrap();
    assert_eq!((t.size, t.align, u.string(t.file)), (8, 4, b"a.h".as_slice()));
    let d = u.declarations();
    assert_eq!(d.len(), 2);
    assert_eq!(d.get(0).unwrap().flags, 0, "sorted by key then flags");
    assert_eq!(d.get(1).unwrap().flags, flags::DECL_DEFINITION);
    assert!(u.details().is_none());
}

// spec: meta.md T-MT-38 (the same rows in any order, the same bytes)
#[test]
fn canonical() {
    let a = sample(3);
    let mut b = a.clone();
    b.summary.reverse();
    b.definitions.reverse();
    b.types.reverse();
    b.declarations.reverse();
    b.definitions.rotate_left(1);
    assert_eq!(a.finish().unwrap(), b.finish().unwrap());
    // Strings in order of first reference by the sorted rows: kind 1's
    // first name is the smallest one, `abi.char`, at offset 1.
    let bytes = a.finish().unwrap();
    let (units, _) = read_section(attrs(), &bytes);
    let (_, table) = units[0].strtab();
    assert!(table.starts_with(b"\0abi.char\0signed\0producer\0test\0std\0c++20\0"));
    // Empty builder: a header and nothing else.
    let empty = UnitBuilder::new(producer::TEST).finish().unwrap();
    assert_eq!(empty.len(), 32);
    let (units, findings) = read_section(attrs(), &empty);
    assert_eq!((units.len(), findings.len()), (1, 0));
}

// spec: meta.md T-MT-26 (kind 1 names unique)
#[test]
fn summary_names_unique() {
    let mut b = UnitBuilder::new(producer::TEST);
    b.summary.push(Summary { name: s("std"), value: s("c++20"), flags: 0 });
    b.summary.push(Summary { name: s("std"), value: s("c++17"), flags: 0 });
    assert!(b.finish().is_err());
    let mut c = UnitBuilder::new(producer::TEST);
    c.summary.push(Summary { name: s("a\0b"), value: s(""), flags: 0 });
    assert!(c.finish().is_err());
}

// spec: meta.md T-MT-10 (zero padding between units), T-MT-4 (units of
// several sections are one sequence)
#[test]
fn padding_and_several_sections() {
    let one = sample(1).finish().unwrap();
    let two = sample(2).finish().unwrap();
    let joined = section(&[one.clone(), two.clone()], 3);
    let (units, findings) = read_section(attrs(), &joined);
    assert!(findings.is_empty());
    assert_eq!(units.iter().map(|u| u.offset).collect::<Vec<_>>(), [0, one.len() as u64 + 24]);
    // Leading and trailing padding.
    let mut padded = vec![0u8; 16];
    padded.extend_from_slice(&one);
    padded.extend_from_slice(&[0; 8]);
    let (units, findings) = read_section(attrs(), &padded);
    assert_eq!((units.len(), findings.len()), (1, 0));
    // Two sections of one file.
    let mut r = FileReader::new();
    r.section(SectionAttrs::conforming(3), &one);
    r.section(SectionAttrs::conforming(9), &two);
    assert_eq!(r.units.iter().map(|u| u.section).collect::<Vec<_>>(), [3, 9]);
    assert!(r.findings.is_empty());
}

fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn put16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

/// The offset of the block of kind `k` in a unit written by the writer.
fn block_at(unit: &[u8], k: u16) -> usize {
    let (units, _) = read_section(attrs(), unit);
    units[0].blocks().find(|b| b.kind == k).unwrap().at as usize
}

/// Every malformation of MT-14 between two good units: exactly one
/// `meta-malformed`, and the good units around it are read.
// spec: meta.md T-MT-14
#[test]
fn every_malformation_is_one_finding_and_neighbours_are_read() {
    let good = sample(7).finish().unwrap();
    let bad = sample(8).finish().unwrap();
    let k2 = block_at(&bad, kind::DEFINITIONS);
    let k1 = block_at(&bad, kind::SUMMARY);
    let k5 = block_at(&bad, kind::DECLARATIONS);
    let strtab = u32::from_le_bytes(bad[20..24].try_into().unwrap()) as usize;
    let strsize = u32::from_le_bytes(bad[24..28].try_into().unwrap()) as usize;
    let cases: Vec<Mutation> = vec![
        ("block past the unit", Box::new(move |u| put32(u, k2 + 4, 0x10_0000))),
        ("block_size not a multiple of 8", Box::new(move |u| put32(u, k2 + 4, 52))),
        ("rows past the block", Box::new(move |u| put32(u, k2 + 12, 1000))),
        ("row_size not a multiple of 8", Box::new(move |u| put32(u, k2 + 8, 44))),
        ("row_size below v1", Box::new(move |u| put32(u, k2 + 8, 40))),
        ("version 0", Box::new(move |u| put16(u, k2 + 2, 0))),
        ("string past the table", Box::new(move |u| put32(u, k1 + 16, 0xffff))),
        (
            "string without NUL",
            Box::new(move |u| {
                let last = strtab + strsize - 1;
                u[last] = b'x';
            }),
        ),
        (
            "kind 1 unsorted",
            Box::new(move |u| {
                // Swap the name references of rows 0 and 1.
                let (a, b) = (k1 + 16, k1 + 32);
                let x = u[a..a + 4].to_vec();
                let y = u[b..b + 4].to_vec();
                u[a..a + 4].copy_from_slice(&y);
                u[b..b + 4].copy_from_slice(&x);
            }),
        ),
        (
            "kind 2 unsorted",
            Box::new(move |u| {
                for i in 0..8 {
                    u[k2 + 16 + i] = 0xff;
                }
            }),
        ),
        ("kind 5 unsorted", Box::new(move |u| put16(u, k5 + 16 + 32, 0xff))),
        ("blocks unsorted", Box::new(move |u| put16(u, k2, 0x0009))),
        ("header_size below 32", Box::new(|u| put32(u, 12, 24))),
        ("strtab past the unit", Box::new(move |u| put32(u, 24, 0x10_0000))),
        ("strtab in the header", Box::new(|u| put32(u, 20, 8))),
    ];
    for (what, mutate) in &cases {
        let mut b = bad.clone();
        mutate(&mut b);
        let bytes = section(&[good.clone(), b, good.clone()], 1);
        let (units, findings) = read_section(attrs(), &bytes);
        assert_eq!(findings.len(), 1, "{what}: {findings:?}");
        assert_eq!(findings[0].id(), "meta-malformed", "{what}");
        assert_eq!(units.len(), 2, "{what}: the neighbours are read");
        let Finding::Malformed { unit: Some(at), .. } = &findings[0] else { panic!("{what}") };
        assert_eq!(*at, good.len() as u64 + 8, "{what}: names the unit's offset");
        assert!(findings[0].reason().contains("MT-1"), "{what}: {}", findings[0].reason());
    }
}

/// The faults that hide where the next unit starts end the section with
/// one finding; the units before them are read.
// spec: meta.md T-MT-14 (bad magic, bad unit_size)
#[test]
fn faults_that_end_the_section() {
    let good = sample(1).finish().unwrap();
    for (what, tail, want) in [
        ("magic", b"MASX\x01\0\0\0\x20\0\0\0".to_vec(), Malformation::Magic),
        ("not padding", vec![0, 0, 0, 0, 0, 0, 0, 1], Malformation::Magic),
        (
            "unit_size odd",
            b"MASN\x01\0\0\0\x21\0\0\0\0\0\0\0".to_vec(),
            Malformation::UnitSize { size: 0x21 },
        ),
        (
            "unit_size past",
            b"MASN\x01\0\0\0\0\0\x10\0\0\0\0\0".to_vec(),
            Malformation::UnitSize { size: 0x10_0000 },
        ),
        (
            "unit_size zero",
            b"MASN\x01\0\0\0\0\0\0\0\0\0\0\0".to_vec(),
            Malformation::UnitSize { size: 0 },
        ),
        ("truncated", vec![0, 0, 0], Malformation::Truncated { left: 3 }),
    ] {
        let mut bytes = good.clone();
        bytes.extend_from_slice(&tail);
        if what != "truncated" {
            bytes.extend_from_slice(&good);
        }
        let (units, findings) = read_section(attrs(), &bytes);
        assert_eq!(units.len(), 1, "{what}");
        assert_eq!(malformed(&findings), [&want], "{what}");
        assert!(want.ends_section());
    }
}

// spec: meta.md MT-1 (the attributes the reader checks)
#[test]
fn section_attributes() {
    let good = sample(1).finish().unwrap();
    for a in [
        SectionAttrs { addralign: 1, ..attrs() },
        SectionAttrs { entsize: 8, ..attrs() },
        SectionAttrs { link: 1, ..attrs() },
        SectionAttrs { info: 2, ..attrs() },
    ] {
        let (units, findings) = read_section(a, &good);
        assert!(units.is_empty());
        assert_eq!(findings.len(), 1);
        assert!(matches!(findings[0], Finding::Malformed { unit: None, .. }));
    }
}

// spec: meta.md T-MT-25 (unknown kind skipped silently; experimental kind
// skipped; version 2 with a longer row read as version 1)
#[test]
fn unknown_kinds_and_later_versions() {
    let mut b = sample(4);
    b.raw.push(RawBlock { kind: 0x0010, version: 1, row_size: 8, row_count: 1, body: vec![7; 8] });
    b.raw.push(RawBlock { kind: 0x7f00, version: 9, row_size: 0, row_count: 0, body: vec![1; 5] });
    b.raw.push(RawBlock { kind: 0x0100, version: 1, row_size: 0, row_count: 0, body: vec![] });
    let bytes = b.finish().unwrap();
    let (units, findings) = read_section(attrs(), &bytes);
    assert!(findings.is_empty(), "{findings:?}");
    assert_eq!(units[0].blocks().count(), 4 + 3);
    assert_eq!(units[0].definitions().len(), 4);

    // Kind 2 at version 2: rows of 56 bytes, the v1 prefix then 8 more.
    let mut v2 = UnitBuilder::new(producer::TEST);
    let mut body = Vec::new();
    for (i, n) in [b"_Z1av".as_slice(), b"_Z1bv"].iter().enumerate() {
        let mut row = vec![0u8; 56];
        row[..8].copy_from_slice(&fnv1a64(n).to_le_bytes());
        row[28..30].copy_from_slice(&class::VARIABLE.to_le_bytes());
        row[40..44].copy_from_slice(&(i as u32 + 1).to_le_bytes());
        row[48..56].copy_from_slice(&[0xee; 8]);
        body.push((fnv1a64(n), row));
    }
    body.sort();
    v2.raw.push(RawBlock {
        kind: kind::DEFINITIONS,
        version: 2,
        row_size: 56,
        row_count: 2,
        body: body.into_iter().flat_map(|(_, r)| r).collect(),
    });
    let bytes = v2.finish().unwrap();
    let (units, findings) = read_section(attrs(), &bytes);
    assert!(findings.is_empty(), "{findings:?}");
    let d = units[0].definitions();
    assert_eq!((d.len(), d.row_size()), (2, 56));
    assert_eq!(units[0].version(kind::DEFINITIONS), Some(2));
    assert!(d.iter().all(|r| r.class == class::VARIABLE && r.line > 0));
}

// spec: meta.md T-MT-35 (major 2 skipped with one meta-version per file)
#[test]
fn other_major() {
    let good = sample(1).finish().unwrap();
    let mut two = sample(2).finish().unwrap();
    put16(&mut two, 4, 2);
    // A major-2 unit whose body is anything at all: only magic, major and
    // unit_size are read.
    for b in &mut two[12..] {
        *b = 0xa5;
    }
    let bytes = section(&[two.clone(), good.clone(), two.clone()], 0);
    let mut r = FileReader::new();
    r.section(attrs(), &bytes);
    r.section(SectionAttrs::conforming(6), &two);
    assert_eq!(r.units.len(), 1);
    assert_eq!(r.findings, [Finding::Version { section: 5, unit: 0, major: 2 }]);
}

// spec: meta.md T-MT-36 (minor 1 with a longer header is read)
#[test]
fn later_minor() {
    let base = sample(5).finish().unwrap();
    // Rebuild with a 40-byte header: 8 more header bytes, every offset +8.
    let mut u = Vec::new();
    u.extend_from_slice(&base[..32]);
    u.extend_from_slice(&[0x11; 8]);
    u.extend_from_slice(&base[32..]);
    put16(&mut u, 6, 1);
    let size = u.len() as u32;
    put32(&mut u, 8, size);
    put32(&mut u, 12, 40);
    let strtab = u32::from_le_bytes(u[20..24].try_into().unwrap());
    put32(&mut u, 20, strtab + 8);
    let (units, findings) = read_section(attrs(), &u);
    assert!(findings.is_empty(), "{findings:?}");
    assert_eq!((units[0].minor, units[0].header_size), (1, 40));
    let (want, _) = read_section(attrs(), &base);
    assert_eq!(units[0].definitions().len(), want[0].definitions().len());
    assert_eq!(units[0].summary_value(b"std"), want[0].summary_value(b"std"));
}

// spec: meta.md T-MT-11 (block framing faults are reported with the fault)
#[test]
fn block_fault_names_the_fault() {
    let mut u = sample(1).finish().unwrap();
    let k2 = block_at(&u, kind::DEFINITIONS);
    put32(&mut u, k2 + 8, 40);
    let (_, findings) = read_section(attrs(), &u);
    assert!(matches!(
        malformed(&findings)[..],
        [Malformation::Block { fault: BlockFault::ShortRow(40), .. }]
    ));
}

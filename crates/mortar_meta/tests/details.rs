//! Kind 4, layout details (`spec/odr.md` O-19; WP-145): the typed reader
//! of an entry, its malformations (`meta-malformed` by MT-14's rules,
//! found when the entry is read), forward compatibility (unknown item
//! kinds, a set `reserved`, a later block version), and a deterministic
//! fuzz over every truncation and every single byte set to 0xff.

use mortar_meta::details::{self, Fault, item};
use mortar_meta::read::{Malformation, SectionAttrs, Unit};
use mortar_meta::write::{Detail, RawBlock, Type};
use mortar_meta::{NO_DETAILS, UnitBuilder, kind, producer, read_section};

fn member(name: &str, ty: &str, bits: u64) -> Detail {
    Detail { name: name.into(), ty: ty.into(), value: bits, width: 0, flags: item::MEMBER }
}

/// A unit with three typed entries (one empty) and a row without one.
fn sample() -> UnitBuilder {
    let mut b = UnitBuilder::new(producer::TEST);
    let mut s = Type::named(b"1S");
    s.items = Some(vec![
        Detail { value: 0, flags: item::VPTR, ..Detail::default() },
        member("x", "i", 64),
        Detail { width: 3, flags: item::MEMBER | item::BITFIELD, ..member("y", "j", 96) },
        Detail { name: b"_ZN1S1fEv".to_vec(), value: 0, flags: item::SLOT, ..Detail::default() },
    ]);
    let mut d = Type::named(b"1D");
    d.items = Some(vec![
        Detail {
            ty: b"1S".to_vec(),
            value: 0,
            flags: item::BASE | item::VIRTUAL,
            ..Detail::default()
        },
        Detail { name: b"later".to_vec(), ty: b"?".to_vec(), value: 7, width: 1, flags: 9 },
        member("z", "l", 128),
    ]);
    let mut e = Type::named(b"1E");
    e.items = Some(vec![
        Detail { ty: b"i".to_vec(), flags: item::UNDERLYING, ..Detail::default() },
        Detail {
            name: b"A".to_vec(),
            value: u64::MAX,
            flags: item::ENUMERATOR,
            ..Detail::default()
        },
    ]);
    let mut empty = Type::named(b"1Z");
    empty.items = Some(Vec::new());
    b.types.extend([s, d, e, empty, Type::named(b"1N")]);
    b
}

fn attrs() -> SectionAttrs {
    SectionAttrs::conforming(3)
}

/// The rows' `details`, by name.
fn details_of<'a>(u: &Unit<'a>) -> Vec<(&'a [u8], u32)> {
    u.types().iter().map(|r| (u.string(r.name), r.details)).collect()
}

// spec: odr.md O-19 (the entry: count, items of 24 bytes, kinds 0-5,
// BITFIELD, VIRTUAL; strings through the unit's table)
#[test]
fn entries_read_back() {
    let bytes = sample().finish().unwrap();
    let (units, findings) = read_section(attrs(), &bytes);
    assert!(findings.is_empty(), "{findings:?}");
    let u = &units[0];
    let rows = details_of(u);
    let at = |n: &[u8]| rows.iter().find(|r| r.0 == n).unwrap().1;
    assert_eq!(at(b"1N"), NO_DETAILS);
    for (n, _) in &rows {
        assert!(at(n) == NO_DETAILS || at(n) % 8 == 0);
    }
    let s = u.detail_entry(at(b"1S")).unwrap().unwrap();
    let items: Vec<_> = s.items().collect();
    assert_eq!(items.len(), 4);
    assert_eq!(items[0].kind(), item::VPTR);
    assert_eq!(
        (u.string(items[1].name), u.string(items[1].ty), items[1].value),
        (&b"x"[..], &b"i"[..], 64)
    );
    assert!(items[2].bitfield() && items[2].width == 3);
    assert_eq!((items[3].kind(), u.string(items[3].name)), (item::SLOT, &b"_ZN1S1fEv"[..]));
    // An unknown item kind is skipped (O-19), and its strings are not
    // checked.
    let d = u.detail_entry(at(b"1D")).unwrap().unwrap();
    assert_eq!(d.count, 3);
    assert_eq!(d.all_items().count(), 3);
    let known: Vec<_> = d.items().collect();
    assert_eq!(known.len(), 2);
    assert!(known[0].is_virtual() && known[0].kind() == item::BASE);
    assert_eq!(u.string(known[1].name), b"z");
    let e = u.detail_entry(at(b"1E")).unwrap().unwrap();
    assert_eq!(e.items().map(|i| i.value).collect::<Vec<_>>(), [0, u64::MAX]);
    // count 0 is an entry.
    let z = u.detail_entry(at(b"1Z")).unwrap().unwrap();
    assert_eq!((z.count, z.items().count()), (0, 0));
}

/// The unit with kind 4's body replaced by `body` (the same strings).
fn with_body(body: &[u8]) -> Vec<u8> {
    let mut bytes = sample().finish().unwrap();
    let (start, len) = {
        let (units, _) = read_section(attrs(), &bytes);
        let old = units[0].details().unwrap();
        (old.as_ptr() as usize - bytes.as_ptr() as usize, old.len())
    };
    assert!(body.len() <= len);
    let mut new = body.to_vec();
    new.resize(len, 0);
    bytes[start..start + len].copy_from_slice(&new);
    bytes
}

/// The offset of 1S's entry (four items, the vptr first).
fn s_at() -> u32 {
    let bytes = sample().finish().unwrap();
    let (units, _) = read_section(attrs(), &bytes);
    details_of(&units[0]).iter().find(|r| r.0 == b"1S").unwrap().1
}

fn body() -> Vec<u8> {
    let bytes = sample().finish().unwrap();
    let (units, _) = read_section(attrs(), &bytes);
    units[0].details().unwrap().to_vec()
}

fn fault(bytes: &[u8], offset: u32) -> Result<(), Malformation> {
    let (units, findings) = read_section(attrs(), bytes);
    assert!(findings.is_empty(), "kind 4 is not read when the unit opens: {findings:?}");
    units[0].detail_entry(offset).map(|_| ())
}

// spec: odr.md O-19, meta.md MT-14 (every malformation names the block
// and the offset; the unit itself opens: kind 4 is read on a finding)
#[test]
fn malformed_entries() {
    let b = body();
    let len = b.len() as u32;
    let block = {
        let bytes = sample().finish().unwrap();
        let (units, _) = read_section(attrs(), &bytes);
        units[0].blocks().find(|h| h.kind == kind::DETAILS).unwrap().at
    };
    let bytes = sample().finish().unwrap();
    let o = s_at();
    let at = o as usize;
    let details = |fault| Err(Malformation::Details { block: Some(block), offset: o, fault });
    assert_eq!(
        fault(&bytes, 4),
        Err(Malformation::Details { block: Some(block), offset: 4, fault: Fault::Misaligned })
    );
    assert_eq!(
        fault(&bytes, len),
        Err(Malformation::Details {
            block: Some(block),
            offset: len,
            fault: Fault::Header { len }
        })
    );
    assert_eq!(
        fault(&bytes, u32::MAX - 7),
        Err(Malformation::Details {
            block: Some(block),
            offset: u32::MAX - 7,
            fault: Fault::Header { len }
        })
    );
    // count past the block, including the largest count (no overflow).
    for count in [len / 24, u32::MAX] {
        let mut c = b.clone();
        c[at..at + 4].copy_from_slice(&count.to_le_bytes());
        assert_eq!(fault(&with_body(&c), o), details(Fault::Items { count, len }));
    }
    // A string offset past the table, in a known item: name, then type.
    for field in [8usize + 24, 12 + 24] {
        let mut c = b.clone();
        c[at + field..at + field + 4].copy_from_slice(&0x7fff_fff0u32.to_le_bytes());
        assert_eq!(
            fault(&with_body(&c), o),
            details(Fault::String { item: 1, offset: 0x7fff_fff0 })
        );
    }
    // The text names the block, the entry and the rule.
    let m = fault(&bytes, 4).unwrap_err();
    assert_eq!(m.rule(), "odr.md O-19, meta.md MT-14");
    assert_eq!(
        m.to_string(),
        format!("kind 4 block at offset {block}: entry at offset 4: not a multiple of 8")
    );
    assert!(!m.ends_section());
    // No kind 4 block at all.
    let mut u = UnitBuilder::new(producer::TEST);
    let mut t = Type::named(b"1S");
    t.details = 0;
    u.types.push(t);
    let bytes = u.finish().unwrap();
    let m = fault(&bytes, 0).unwrap_err();
    assert_eq!(m, Malformation::Details { block: None, offset: 0, fault: Fault::NoBlock });
    assert!(m.to_string().contains("the unit has no kind 4 block"), "{m}");
}

// spec: odr.md O-19 (a reader ignores a set `reserved` and an item kind
// it does not know), meta.md MT-25 (a later version of the block is
// skipped)
#[test]
fn forward_compatibility() {
    let at = s_at() as usize;
    let mut b = body();
    b[at + 4..at + 8].copy_from_slice(&0xdead_beefu32.to_le_bytes());
    let bytes = with_body(&b);
    let (units, _) = read_section(attrs(), &bytes);
    let e = units[0].detail_entry(at as u32).unwrap().unwrap();
    assert_eq!(e.reserved, 0xdead_beef);
    assert_eq!(e.items().count(), 4);
    // Unknown flag bits above bit 5 do not change the kind.
    let mut b = body();
    b[at + 8 + 20..at + 8 + 24].copy_from_slice(&(item::VPTR | 0x8000_0000).to_le_bytes());
    let bytes = with_body(&b);
    let (units, _) = read_section(attrs(), &bytes);
    let first = units[0].detail_entry(at as u32).unwrap().unwrap().items().next().unwrap();
    assert_eq!(first.kind(), item::VPTR);
    // Version 2 of the block: skipped, not malformed.
    let mut u = UnitBuilder::new(producer::TEST);
    let mut t = Type::named(b"1S");
    t.details = 0;
    u.types.push(t);
    u.raw.push(RawBlock {
        kind: kind::DETAILS,
        version: 2,
        row_size: 0,
        row_count: 0,
        body: vec![0; 8],
    });
    let bytes = u.finish().unwrap();
    let (units, findings) = read_section(attrs(), &bytes);
    assert!(findings.is_empty());
    assert!(units[0].detail_entry(0).unwrap().is_none());
}

// WP-145: deterministic fuzz. Every prefix of a valid body and every
// single byte of it set to 0xff, read at every row's offset and every
// multiple of 8: `Ok` or a kind 4 malformation, never a panic; and the
// same for every byte of the whole unit set to 0xff.
#[test]
fn truncations_and_bit_flips() {
    let b = body();
    let bytes = sample().finish().unwrap();
    let (units, _) = read_section(attrs(), &bytes);
    let strtab = units[0].strtab().1.to_vec();
    let mut offsets: Vec<u32> = details_of(&units[0]).iter().map(|r| r.1).collect();
    offsets.extend((0..=b.len() as u32 + 8).step_by(8));
    offsets.push(u32::MAX);
    let mut outcomes = [0usize; 2];
    let mut check = |body: &[u8]| {
        for &o in &offsets {
            match details::entry(body, &strtab, o) {
                Ok(e) => {
                    outcomes[0] += 1;
                    let _ = e.items().map(|i| (i.name, i.ty, i.value)).count();
                }
                Err(_) => outcomes[1] += 1,
            }
        }
    };
    for n in 0..=b.len() {
        check(&b[..n]);
    }
    for i in 0..b.len() {
        let mut c = b.clone();
        c[i] = 0xff;
        check(&c);
    }
    assert!(outcomes[0] > 0 && outcomes[1] > 0, "{outcomes:?}");
    // Whole units: whatever opens, every row's entry reads or is
    // malformed.
    for i in 0..bytes.len() {
        let mut c = bytes.clone();
        c[i] = 0xff;
        let (units, _) = read_section(attrs(), &c);
        for u in &units {
            for r in u.types().iter() {
                match u.detail_entry(r.details) {
                    Ok(_) | Err(Malformation::Details { .. }) => {}
                    Err(other) => panic!("byte {i}: {other:?}"),
                }
            }
        }
    }
}

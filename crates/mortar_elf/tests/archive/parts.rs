//! `spec/elf.md` §3.0 for archives: the header chain over a parts view,
//! and `header_span`, what a staged store keeps resident of a header.

use mortar_elf::archive::{self, Archive, Location};
use mortar_elf::{Part, View};

use crate::build::seeds;

/// The parts of a regular archive a staged store keeps: the magic, every
/// header, and what `header_span` says the reader decodes.
fn staged_parts(bytes: &[u8]) -> Vec<Part> {
    let len = bytes.len() as u64;
    let mut ranges: Vec<(u64, u64)> = vec![(0, 8)];
    let mut at = 8u64;
    while at < len {
        let raw = &bytes[at as usize..(at + 60).min(len) as usize];
        let span = archive::header_span(at, raw, len).unwrap();
        ranges.push((at, span.content + span.front));
        at = span.next;
    }
    let mut merged: Vec<(u64, u64)> = Vec::new();
    for (lo, hi) in ranges {
        match merged.last_mut() {
            Some(last) if lo <= last.1 => last.1 = last.1.max(hi),
            _ => merged.push((lo, hi)),
        }
    }
    merged
        .into_iter()
        .map(|(lo, hi)| Part { offset: lo, bytes: bytes[lo as usize..hi as usize].into() })
        .collect()
}

/// §3.0: over the headers alone, `headers()` lists what `members()` lists
/// over the whole archive; `members()` is `NotResident` at the first
/// member with content.
#[test]
fn s3_0_archive_headers_over_parts() {
    for (name, fixture) in seeds() {
        let whole = Archive::parse(&fixture.bytes).unwrap();
        if whole.kind() == archive::Kind::Thin {
            continue;
        }
        let parts = staged_parts(&fixture.bytes);
        let view = View::parts(&parts, fixture.bytes.len() as u64);
        let staged = Archive::parse_view(view).unwrap();
        assert_eq!(staged.first_member(), whole.first_member(), "{name}");
        let headers: Vec<_> = staged.headers().map(Result::unwrap).collect();
        let members: Vec<_> = whole.members().map(Result::unwrap).collect();
        assert_eq!(headers.len(), members.len(), "{name}");
        for (h, m) in headers.iter().zip(&members) {
            assert_eq!((h.header_offset, h.name, h.size), (m.header_offset(), m.name(), m.size()));
            let Location::Inline { offset } = h.location else { panic!("{name}") };
            let data = m.data().unwrap();
            assert_eq!(&fixture.bytes[offset as usize..offset as usize + data.len()], data);
        }
        assert_eq!(
            staged.symbol_index().map(|i| i.map(|i| i.len()).ok()),
            whole.symbol_index().map(|i| i.map(|i| i.len()).ok()),
            "{name}"
        );
        // `members()` reads every member's content: `NotResident` at the
        // first that is not resident (a BSD-named member is).
        let resident = |m: &archive::Member<'_>| match m.content() {
            archive::Content::Inline { offset, data } => {
                view.get(offset, data.len() as u64).is_some()
            }
            _ => true,
        };
        let first_missing = members.iter().position(|m| !resident(m));
        let got: Vec<_> = staged.members().collect();
        match first_missing {
            Some(i) => {
                assert!(got[..i].iter().all(Result::is_ok), "{name}");
                assert!(matches!(got[i], Err(archive::Error::NotResident { .. })), "{name}");
            }
            None => assert!(got.iter().all(Result::is_ok), "{name}"),
        }
    }
}

/// §3.0: over the whole archive, `headers()` agrees with `members()`, and
/// `header_span` with both.
#[test]
fn s3_0_header_span_agrees() {
    for (name, fixture) in seeds() {
        let whole = Archive::parse(&fixture.bytes).unwrap();
        let headers: Vec<_> = whole.headers().map(Result::unwrap).collect();
        let members: Vec<_> = whole.members().map(Result::unwrap).collect();
        assert_eq!(headers.len(), members.len(), "{name}");
        if whole.kind() == archive::Kind::Thin {
            continue;
        }
        let len = fixture.bytes.len() as u64;
        for (h, m) in headers.iter().zip(&members) {
            let at = h.header_offset;
            let raw = &fixture.bytes[at as usize..(at + 60).min(len) as usize];
            let span = archive::header_span(at, raw, len).unwrap();
            let Location::Inline { offset } = h.location else { panic!() };
            assert_eq!(span.content + span.size, offset + m.size(), "{name}");
            // A BSD-named member is kept whole (its name may make it an
            // index); any other, its header and nothing of its content.
            if span.front != span.size {
                assert_eq!(span.content + span.front, offset, "{name}");
            }
        }
    }
}

/// §3.0: `header_span` gives the reader's error for a damaged header.
#[test]
fn s3_0_header_span_errors() {
    let (_, fixture) = seeds().remove(0);
    let len = fixture.bytes.len() as u64;
    let at = fixture.offsets[0];
    let mut raw = fixture.bytes[at as usize..at as usize + 60].to_vec();
    raw[58] = b'x';
    let span = archive::header_span(at, &raw, len);
    assert!(matches!(span, Err(archive::Error::BadHeaderTerminator { .. })), "{span:?}");
    let short = archive::header_span(at, &raw[..10], len);
    assert!(matches!(short, Err(archive::Error::TruncatedHeader { .. })), "{short:?}");
}

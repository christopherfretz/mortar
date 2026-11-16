//! The inputs read by position (`spec/plumb.md` §8.1, WP-109 §6.3): the
//! verdict through a [`Source`] is the verdict on the whole bytes, and a
//! source that cannot give what it promised is a finding naming the file,
//! never a panic.

mod common;

use common::{Linker, Size, corpus};
use mortar_plumb::{Ctx, HostFiles, Input, Source};

/// A source over whole bytes that fails every read of file `bad` (a file
/// that shrank, or an I/O error, after it was opened).
struct Failing<'a> {
    inner: &'a Vec<Input<'a>>,
    bad: usize,
}

impl Source for Failing<'_> {
    fn count(&self) -> usize {
        self.inner.count()
    }
    fn path(&self, i: usize) -> &str {
        self.inner.path(i)
    }
    fn len(&self, i: usize) -> u64 {
        Source::len(self.inner, i)
    }
    fn read_at(&self, i: usize, offset: u64, buf: &mut [u8]) -> Result<(), String> {
        if i == self.bad {
            return Err(format!("{}: the read at {offset:#x} failed", self.inner.path(i)));
        }
        self.inner.read_at(i, offset, buf)
    }
}

// spec/plumb.md §8.1: whole bytes as a source give exactly the bytes asked
// for, and a read past the end is an error naming the file.
#[test]
fn s8_1_whole_bytes_source() {
    let inputs = [Input { path: "a.o", bytes: b"0123456789" }];
    let src: &[Input<'_>] = &inputs;
    assert_eq!(src.count(), 1);
    assert_eq!(src.path(0), "a.o");
    assert_eq!(Source::len(&src, 0), 10);
    let mut b = [0u8; 4];
    src.read_at(0, 3, &mut b).unwrap();
    assert_eq!(&b, b"3456");
    let e = src.read_at(0, 8, &mut b).unwrap_err();
    assert!(e.contains("a.o") && e.contains("past its end"), "{e}");
    assert!(src.read_at(0, u64::MAX, &mut b).is_err());
    assert!(src.read_at(1, 0, &mut b).is_err());
}

// spec/plumb.md §8.1: the verdict through a source is the verdict on the
// whole bytes, the whole report (every finding, every count).
#[test]
fn s8_1_verify_from_is_the_verdict_on_whole_bytes() {
    let Some(c) = corpus(Size::Small) else { return };
    for link in &c.links {
        let map = link.parse_map();
        let files = link.inputs(&map);
        let out = link.output_bytes();
        let inputs: Vec<Input<'_>> =
            files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
        let ctx = Ctx { line: None, files: &HostFiles };
        let whole = mortar_plumb::verify_with(&out, &inputs, &map, &ctx);
        let from = mortar_plumb::verify_from(&out, &inputs, &map, &ctx);
        assert_eq!(format!("{whole:?}"), format!("{from:?}"), "{}", link.label());
    }
}

// spec/plumb.md §8.1: a file the source cannot read is an I2 finding that
// names it, not a panic and not a silent gap.
#[test]
fn s8_1_a_failing_read_is_a_finding() {
    let Some(c) = corpus(Size::Small) else { return };
    let link = c.get("start", Linker::Bfd);
    let map = link.parse_map();
    let files = link.inputs(&map);
    let out = link.output_bytes();
    let inputs: Vec<Input<'_>> = files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
    let ctx = Ctx { line: None, files: &HostFiles };
    for bad in 0..inputs.len() {
        let src = Failing { inner: &inputs, bad };
        let r = mortar_plumb::verify_from(&out, &src, &map, &ctx);
        let path = inputs[bad].path;
        assert!(
            r.findings.iter().any(|f| f.rule.id() == "I2"
                && f.message.contains(path)
                && f.message.contains("failed")),
            "{path}: {:?}",
            r.findings
        );
    }
}

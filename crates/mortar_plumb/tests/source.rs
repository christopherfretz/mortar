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

/// A source over whole bytes whose file `bad` gives its first `good`
/// reads and fails every later one (a file that changed after the
/// verifier read its skeleton).
struct Later<'a> {
    inner: &'a Vec<Input<'a>>,
    bad: usize,
    good: usize,
    reads: std::sync::atomic::AtomicUsize,
}

impl Source for Later<'_> {
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
        if i == self.bad
            && self.reads.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= self.good
        {
            return Err(format!("{}: the read at {offset:#x} failed", self.inner.path(i)));
        }
        self.inner.read_at(i, offset, buf)
    }
}

// spec/plumb.md §8.1: a read that fails after the skeletons were read (the
// file changed under the verifier) is an I2 finding naming the file; the
// rest of the verdict stands. Never a panic, never a silent pass: when no
// read failed the verdict is the whole bytes'.
#[test]
fn s8_1_a_read_failing_later_is_a_finding() {
    let Some(c) = corpus(Size::Small) else { return };
    let link = c.get("start", Linker::Bfd);
    let map = link.parse_map();
    let files = link.inputs(&map);
    let out = link.output_bytes();
    let inputs: Vec<Input<'_>> = files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
    let ctx = Ctx { line: None, files: &HostFiles };
    let whole = format!("{:?}", mortar_plumb::verify_from(&out, &inputs, &map, &ctx));
    let mut later = 0;
    for bad in 0..inputs.len() {
        for good in 0..12 {
            let src = Later { inner: &inputs, bad, good, reads: 0.into() };
            let r = mortar_plumb::verify_from(&out, &src, &map, &ctx);
            let path = inputs[bad].path;
            let named = r.findings.iter().any(|f| {
                f.rule.id() == "I2" && f.message.contains(path) && f.message.contains("failed")
            });
            if named {
                later += usize::from(good > 0);
            } else {
                assert_eq!(
                    format!("{r:?}"),
                    whole,
                    "{path}, {good} reads: no I2 and not the verdict"
                );
            }
        }
    }
    assert!(later > 0, "no read failed after the first");
}

// spec/plumb.md §8.1: an input cut short since the link (a file that
// changed size) is judged as it is: findings, never a panic.
#[test]
fn s8_1_a_file_cut_short_is_judged() {
    let Some(c) = corpus(Size::Small) else { return };
    let link = c.get("start", Linker::Bfd);
    let map = link.parse_map();
    let files = link.inputs(&map);
    let out = link.output_bytes();
    let ctx = Ctx { line: None, files: &HostFiles };
    for bad in 0..files.len() {
        for cut in [1, 2, 3] {
            let inputs: Vec<Input<'_>> = files
                .iter()
                .enumerate()
                .map(|(i, (p, b))| Input {
                    path: p,
                    bytes: if i == bad { &b[..b.len() * cut / 4] } else { b },
                })
                .collect();
            let r = mortar_plumb::verify_from(&out, &inputs, &map, &ctx);
            assert!(!r.findings.is_empty(), "{} cut to {cut}/4: no finding", files[bad].0);
        }
    }
}

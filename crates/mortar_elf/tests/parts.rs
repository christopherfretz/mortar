//! `spec/elf.md` §3.0: the readers over a parts view (`View`). Whatever
//! parts are resident, a reader gives the whole file's answer for what
//! they hold, `Error::NotResident` for what they do not, and never panics.

mod support;

use mortar_elf::consts::SHT_NOBITS;
use mortar_elf::{Error, Object, Part, View};
use support::{Rng, TARGETS, sample};

/// The parts of `bytes` holding exactly `ranges` (sorted, disjoint).
fn parts_of(bytes: &[u8], ranges: &[(usize, usize)]) -> Vec<Part> {
    ranges.iter().map(|&(lo, hi)| Part { offset: lo as u64, bytes: bytes[lo..hi].into() }).collect()
}

/// §3.0: `View::get` is `Some` exactly for a range inside one part.
#[test]
fn s3_0_view_get() {
    let bytes: Vec<u8> = (0..=255).collect();
    let parts = parts_of(&bytes, &[(0, 16), (32, 64), (100, 200)]);
    let view = View::parts(&parts, 256);
    assert_eq!(view.len(), 256);
    assert_eq!(view.get(0, 16), Some(&bytes[..16]));
    assert_eq!(view.get(40, 8), Some(&bytes[40..48]));
    assert_eq!(view.get(10, 10), None, "straddles a part's end");
    assert_eq!(view.get(20, 4), None, "between parts");
    assert_eq!(view.get(200, 1), None, "after the last");
    assert_eq!(view.get(256, 0), Some(&[][..]), "empty, at the end");
    assert_eq!(view.get(20, 0), Some(&[][..]), "empty, between parts");
    assert_eq!(view.get(257, 0), None, "past the end");
    assert_eq!(view.get(u64::MAX, 2), None);
    assert!(view.contains(20, 4) && !view.contains(250, 7));
    assert_eq!(view.as_slice(), None);
    assert!(!view.is_whole());
    let sub = view.sub(32, 100).unwrap();
    assert_eq!((sub.base(), sub.len()), (32, 100));
    assert_eq!(sub.get(0, 32), Some(&bytes[32..64]));
    assert_eq!(sub.get(68, 32), Some(&bytes[100..132]));
    assert_eq!(sub.get(60, 10), None);
    assert!(view.sub(250, 7).is_none());
    // One part holding everything is the whole file.
    let one = parts_of(&bytes, &[(0, 256)]);
    let whole = View::parts(&one, 256);
    assert!(whole.is_whole());
    assert_eq!(whole.as_slice(), Some(&bytes[..]));
    assert_eq!(View::parts(&[], 0).as_slice(), Some(&[][..]));
}

/// §3.0: an object whose every section but one is resident parses as the
/// whole file does; the one is `NotResident` at its own range.
#[test]
fn s3_0_object_over_parts() {
    for (class, e_machine) in TARGETS {
        let built = sample(class, e_machine);
        let whole = Object::parse(&built.bytes).unwrap();
        for hole in whole.sections() {
            let Ok(Some(r)) = hole.file_range() else { continue };
            if r.is_empty() || hole.sh_type() == SHT_NOBITS {
                continue;
            }
            let len = built.bytes.len();
            let parts = parts_of(&built.bytes, &[(0, r.start), (r.end, len)]);
            let parts: Vec<Part> = parts.into_iter().filter(|p| !p.bytes.is_empty()).collect();
            let view = View::parts(&parts, len as u64);
            // The hole may hold the file header or the section headers.
            let object = match Object::parse_view(view) {
                Ok(object) => object,
                Err(e) => {
                    assert!(matches!(e, Error::NotResident { .. }), "{e}");
                    continue;
                }
            };
            assert_eq!(object.data().len(), len as u64);
            for (x, y) in object.sections().zip(whole.sections()) {
                assert_eq!(x.header(), y.header());
                if x.index() == hole.index() {
                    assert!(!x.is_resident());
                    assert_eq!(
                        x.data(),
                        Err(Error::NotResident {
                            offset: r.start as u64,
                            size: (r.end - r.start) as u64
                        })
                    );
                } else {
                    assert_eq!(x.data(), y.data(), "section {}", x.index().0);
                }
            }
        }
    }
}

/// §3.0: a section header table that is not resident is `NotResident`;
/// one out of bounds is the bounds error, as over the whole file.
#[test]
fn s3_0_headers_not_resident() {
    for (class, e_machine) in TARGETS {
        let built = sample(class, e_machine);
        let parts = parts_of(&built.bytes, &[(0, 64)]);
        let view = View::parts(&parts, built.bytes.len() as u64);
        assert!(matches!(Object::parse_view(view), Err(Error::NotResident { .. })));
        let short = View::parts(&parts, built.shoff as u64 + 1);
        assert_eq!(
            Object::parse_view(short).err().map(|e| e.to_string()),
            Object::parse(&built.bytes[..=built.shoff]).err().map(|e| e.to_string())
        );
    }
}

/// §3.0: random parts of the samples never panic, and every answer a
/// reader gives over them is the whole file's.
#[test]
fn s3_0_random_parts_agree() {
    let mut rng = Rng(0x05ee_d0f0_021b);
    for (class, e_machine) in TARGETS {
        let built = sample(class, e_machine);
        let len = built.bytes.len() as u64;
        let whole = Object::parse(&built.bytes).unwrap();
        for _ in 0..300 {
            let mut cuts: Vec<u64> = (0..=rng.below(8)).map(|_| rng.below(len + 1)).collect();
            cuts.extend([0, len]);
            cuts.sort_unstable();
            cuts.dedup();
            let ranges: Vec<(usize, usize)> = cuts
                .windows(2)
                .enumerate()
                .filter(|(i, _)| i % 2 == 0 || rng.below(4) == 0)
                .map(|(_, w)| (w[0] as usize, w[1] as usize))
                .collect();
            // Merge adjacent ranges: parts are not adjacent.
            let mut merged: Vec<(usize, usize)> = Vec::new();
            for (lo, hi) in ranges {
                match merged.last_mut() {
                    Some(last) if last.1 == lo => last.1 = hi,
                    _ => merged.push((lo, hi)),
                }
            }
            let parts = parts_of(&built.bytes, &merged);
            let Ok(object) = Object::parse_view(View::parts(&parts, len)) else { continue };
            let _ = support::walk(&built.bytes);
            if let Ok(Some(table)) = object.symbols() {
                assert_eq!(Some(table.len()), whole.symbols().unwrap().map(|t| t.len()));
            }
            for (x, y) in object.sections().zip(whole.sections()) {
                if let Ok(data) = x.data() {
                    assert_eq!(Ok(data), y.data());
                }
                let _ = (x.name(), x.notes().map(Iterator::count), x.group().map(|g| g.len()));
                let _ = (x.rela().map(|r| r.len()), x.compression(), x.merge());
            }
        }
    }
}

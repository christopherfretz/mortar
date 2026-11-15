//! L1 tests of S-6 to S-15 and S-28: cutting and merging, against a
//! sequential oracle under every schedule.

use std::collections::HashMap;

use mortar_core::par::{Pool, check_schedules};
use mortar_core::split::Frag;
use mortar_core::{FileId, FragId, OutSecId, SectionId};
use mortar_elf::consts::{SHF_EXECINSTR, SHF_MERGE, SHF_STRINGS, SHF_TLS, SHF_WRITE};
use mortar_merge::cut::{self, Whole, placed_whole};
use mortar_merge::{MergeIn, MergeMeta, Merged, Problem, What, merge};

fn cut_ok(bytes: &[u8], strings: bool, entsize: usize) -> Vec<u32> {
    let mut v = Vec::new();
    cut::cut(bytes, strings, entsize, &mut v).expect("cuts");
    v
}

#[test]
fn s6_placed_whole_conditions() {
    let m = SHF_MERGE | SHF_STRINGS;
    assert_eq!(placed_whole(m, 1, 1, false), None);
    assert_eq!(placed_whole(m, 0, 1, false), None);
    assert_eq!(placed_whole(m | SHF_WRITE, 1, 1, false), Some(Whole::WritableOrCode));
    assert_eq!(placed_whole(m | SHF_EXECINSTR, 1, 1, false), Some(Whole::WritableOrCode));
    assert_eq!(placed_whole(m, 1, 1, true), Some(Whole::Relocated));
    assert_eq!(placed_whole(m | SHF_TLS, 1, 1, false), Some(Whole::Tls));
    assert_eq!(placed_whole(m, 3, 1, false), Some(Whole::Shape));
    assert_eq!(placed_whole(m, 1, 1 << 16, false), None);
    assert_eq!(placed_whole(m, 1, (1 << 16) + 1, false), Some(Whole::Shape));
}

#[test]
fn s7_size_not_multiple_of_entsize() {
    let mut v = Vec::new();
    assert_eq!(cut::cut(&[1, 2, 3], false, 2, &mut v), Err((What::NotMultipleOfEntsize, 0)));
    assert_eq!(cut::cut(b"ab\0", true, 2, &mut v), Err((What::NotMultipleOfEntsize, 0)));
    assert!(v.is_empty());
}

#[test]
fn s8_unterminated_string_points_at_its_start() {
    let mut v = Vec::new();
    assert_eq!(cut::cut(b"ab\0cd", true, 1, &mut v), Err((What::Unterminated, 3)));
    assert_eq!(cut::cut(b"abc", true, 1, &mut v), Err((What::Unterminated, 0)));
    // entsize 2: "a\0\0\0" then "b\0c\0" (no all-zero character).
    assert_eq!(cut::cut(b"a\0\0\0b\0c\0", true, 2, &mut v), Err((What::Unterminated, 4)));
    assert!(v.is_empty());
}

#[test]
fn s9_strings_end_at_the_first_zero_character() {
    assert_eq!(cut_ok(b"ab\0\0c\0", true, 1), [0, 3, 4]);
    assert_eq!(cut_ok(b"\0", true, 1), [0]);
    assert_eq!(cut_ok(b"", true, 1), Vec::<u32>::new());
    // Long strings cross the 8-byte search.
    let mut long = vec![b'x'; 37];
    long.push(0);
    long.extend_from_slice(b"yz\0");
    assert_eq!(cut_ok(&long, true, 1), [0, 38]);
    // entsize 2: zero bytes inside a character do not end a string, and
    // characters are taken at multiples of entsize.
    assert_eq!(cut_ok(b"a\0b\0\0\0\0\0c\0\0\0", true, 2), [0, 6, 8]);
    assert_eq!(cut_ok(b"\0a\0\0", true, 2), [0]);
    // entsize 4.
    assert_eq!(cut_ok(b"a\0\0\0\0\0\0\0\0\0\0\0", true, 4), [0, 8]);
}

#[test]
fn s9_next_zero_matches_a_byte_scan() {
    let bytes: Vec<u8> = (0..300u32)
        .map(|i| if i % 37 == 5 || i % 53 == 0 { 0 } else { (i % 251 + 1) as u8 })
        .collect();
    for from in 0..bytes.len() {
        let want = bytes[from..].iter().position(|&b| b == 0).map(|p| from + p);
        assert_eq!(cut::next_zero(&bytes, from), want, "from {from}");
    }
    // 0x80 bytes and 0x01 bytes next to a zero (borrow cases).
    let b = [0x80, 0x01, 0x00, 0x01, 0x80, 0x80, 0x01, 0x01, 0x00];
    assert_eq!(cut::next_zero(&b, 0), Some(2));
    assert_eq!(cut::next_zero(&b, 3), Some(8));
    assert_eq!(cut::next_zero(&[1u8; 8], 0), None);
}

#[test]
fn s10_constants_every_entsize() {
    assert_eq!(cut_ok(&[0; 24], false, 8), [0, 8, 16]);
    assert_eq!(cut_ok(&[0; 4], false, 4), [0]);
    assert_eq!(cut_ok(&[], false, 4), Vec::<u32>::new());
}

/// One input section for the tests.
#[derive(Clone)]
struct Sec {
    file: u32,
    index: u32,
    out: u32,
    strings: bool,
    entsize: u32,
    align: u8,
    bytes: Vec<u8>,
}

fn inputs(secs: &[Sec]) -> Vec<MergeIn<'_>> {
    secs.iter()
        .enumerate()
        .map(|(i, s)| MergeIn {
            section: SectionId(i as u32 * 2 + 1),
            file: FileId(s.file),
            index: s.index,
            out: OutSecId(s.out),
            strings: s.strings,
            entsize: s.entsize,
            align: s.align,
            bytes: &s.bytes,
        })
        .collect()
}

/// The sequential oracle: S-9/S-10 by a byte scan, S-13 identity by a map,
/// S-14/S-15 by walking occurrences in key order.
fn oracle(secs: &[MergeIn<'_>]) -> Merged {
    let mut merge_of = Vec::new();
    let mut piece_off = Vec::new();
    let mut piece_frag = Vec::new();
    let mut frags = Vec::new();
    let mut meta = Vec::new();
    let mut seen: HashMap<(MergeMeta, Vec<u8>), FragId> = HashMap::new();
    for s in secs {
        let e = s.entsize as usize;
        let mut offs = Vec::new();
        let mut at = 0;
        while at < s.bytes.len() {
            offs.push(at);
            if s.strings {
                let mut j = at;
                while s.bytes[j..j + e].iter().any(|&b| b != 0) {
                    j += e;
                }
                at = j + e;
            } else {
                at += e;
            }
        }
        if offs.is_empty() {
            continue;
        }
        let m = MergeMeta { out: s.out, strings: s.strings, entsize: s.entsize, align: s.align };
        merge_of.push(mortar_core::split::SplitRow {
            section: s.section,
            pieces: mortar_core::Span { start: piece_off.len() as u32, len: offs.len() as u32 },
        });
        meta.push(m);
        for (j, &o) in offs.iter().enumerate() {
            let end = offs.get(j + 1).copied().unwrap_or(s.bytes.len());
            let bytes = s.bytes[o..end].to_vec();
            let next = FragId(frags.len() as u32);
            let f = *seen.entry((m, bytes)).or_insert_with(|| {
                frags.push(Frag {
                    section: s.section,
                    offset: o as u32,
                    size: (end - o) as u32,
                    align: s.align,
                });
                next
            });
            piece_off.push(o as u32);
            piece_frag.push(f);
        }
    }
    Merged {
        merge_of: merge_of.into(),
        piece_off: piece_off.into(),
        piece_frag: piece_frag.into(),
        frags: mortar_core::Column::from_vec(frags).unwrap(),
        meta: meta.into(),
    }
}

fn check(secs: &[Sec]) -> Merged {
    let ins = inputs(secs);
    let want = oracle(&ins);
    let got = check_schedules(|pool| merge(pool, &ins).expect("merges"));
    assert_eq!(got, want, "the parallel merge equals the sequential oracle");
    got
}

fn strs(file: u32, index: u32, bytes: &[u8]) -> Sec {
    Sec { file, index, out: 0, strings: true, entsize: 1, align: 0, bytes: bytes.to_vec() }
}

#[test]
fn s13_s14_s15_equal_strings_merge_to_the_first_occurrence() {
    let got =
        check(&[strs(0, 3, b"foo\0bar\0"), strs(1, 3, b"bar\0baz\0foo\0"), strs(2, 4, b"\0foo\0")]);
    // FragIds by first occurrence: foo, bar, baz, "".
    assert_eq!(got.piece_frag.iter().map(|f| f.0).collect::<Vec<_>>(), [0, 1, 1, 2, 0, 3, 0]);
    assert_eq!(got.frags.len(), 4);
    let f = got.frags.get(FragId(2)).unwrap();
    assert_eq!((f.section, f.offset, f.size), (SectionId(3), 4, 4));
}

#[test]
fn s13_identity_includes_output_section_class_and_alignment() {
    let mut a = strs(0, 1, b"ab\0");
    let mut b = a.clone();
    b.index = 2;
    b.out = 1; // another output section
    let mut c = a.clone();
    c.index = 3;
    c.align = 2; // another alignment
    let mut d = a.clone();
    d.index = 4;
    d.strings = false; // constants of entsize 3 with the same bytes
    d.entsize = 3;
    a.index = 0;
    let got = check(&[a.clone(), b, c, d, a]);
    assert_eq!(got.piece_frag.iter().map(|f| f.0).collect::<Vec<_>>(), [0, 1, 2, 3, 0]);
}

#[test]
fn s23_no_tail_merging() {
    let got = check(&[strs(0, 1, b"abc\0bc\0c\0")]);
    assert_eq!(got.frags.len(), 3);
}

#[test]
fn s10_constants_merge() {
    let c = |file, bytes: &[u8]| Sec {
        file,
        index: 5,
        out: 2,
        strings: false,
        entsize: 4,
        align: 2,
        bytes: bytes.to_vec(),
    };
    let got =
        check(&[c(0, &[1, 0, 0, 0, 2, 0, 0, 0]), c(1, &[2, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0])]);
    assert_eq!(got.piece_frag.iter().map(|f| f.0).collect::<Vec<_>>(), [0, 1, 1, 0, 0]);
}

#[test]
fn s5_empty_sections_have_no_row() {
    let got = check(&[strs(0, 1, b""), strs(0, 2, b"a\0")]);
    assert_eq!(got.merge_of.len(), 1);
    assert_eq!(got.merge_of[0].section, SectionId(3));
}

#[test]
fn s28_many_sections_and_collisions_equal_the_oracle() {
    // A deterministic pseudo-random corpus: many sections, heavy sharing,
    // several classes, so every shard and range sees work.
    let mut x: u64 = 0x1234_5678;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let mut secs = Vec::new();
    for i in 0..300u32 {
        let entsize = [1u32, 1, 2, 4][(next() % 4) as usize];
        let strings = next() % 5 != 0;
        let mut bytes = Vec::new();
        for _ in 0..(next() % 40) {
            if strings {
                let len = next() % 4;
                for _ in 0..len {
                    let mut ch = vec![0u8; entsize as usize];
                    ch[0] = b'a' + (next() % 3) as u8;
                    bytes.extend(ch);
                }
                bytes.extend(vec![0u8; entsize as usize]);
            } else {
                for _ in 0..entsize {
                    bytes.push((next() % 3) as u8);
                }
            }
        }
        secs.push(Sec {
            file: i / 7,
            index: i % 7 + 1,
            out: (next() % 2) as u32,
            strings,
            entsize,
            align: (next() % 2) as u8,
            bytes,
        });
    }
    let got = check(&secs);
    assert!((got.frags.len() as usize) < got.piece_frag.len() / 4, "the corpus shares pieces");
}

#[test]
fn s7_s8_problems_are_reported_sorted_and_nothing_else() {
    let mut bad = strs(4, 9, b"ab");
    bad.index = 9;
    let mut odd = strs(1, 2, b"ab\0");
    odd.strings = false;
    odd.entsize = 2;
    let secs = [strs(0, 1, b"x\0"), bad, odd];
    let ins = inputs(&secs);
    let got = check_schedules(|pool| merge(pool, &ins));
    let Err(problems) = got else { panic!("refused") };
    assert_eq!(
        problems,
        [
            Problem {
                file: FileId(1),
                section: 2,
                piece: 0,
                offset: 0,
                what: What::NotMultipleOfEntsize
            },
            Problem { file: FileId(4), section: 9, piece: 0, offset: 0, what: What::Unterminated },
        ]
    );
}

#[test]
fn s7_s8_diagnostic_text_and_key() {
    use mortar_diag::{FileRef, Key};
    let p = Problem { file: FileId(4), section: 9, piece: 0, offset: 3, what: What::Unterminated };
    assert_eq!(p.key(), Key::new(60, 4, 9 << 32));
    let d = p.diagnostic(FileRef::new("a.o"), ".rodata.str1.1");
    let text = d.render(&mortar_diag::Style::plain("ld"));
    assert!(text.contains("string is not null terminated"), "{text}");
    assert!(text.contains("a.o:(.rodata.str1.1+0x3)"), "{text}");
    let p = Problem { what: What::NotMultipleOfEntsize, ..p };
    let text =
        p.diagnostic(FileRef::new("a.o"), ".rodata.cst8").render(&mortar_diag::Style::plain("ld"));
    assert!(text.contains("section size is not a multiple of sh_entsize"), "{text}");
}

#[test]
fn s28_thread_count_is_not_an_input() {
    let secs = [strs(0, 1, b"q\0r\0"), strs(1, 1, b"r\0q\0")];
    let ins = inputs(&secs);
    let one = merge(&Pool::sequential(), &ins).unwrap();
    for t in [3, 16, 100] {
        assert_eq!(merge(&Pool::sequential().with_threads(t), &ins).unwrap(), one);
    }
}

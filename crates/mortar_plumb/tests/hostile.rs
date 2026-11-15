//! Hostile input (`spec/plumb.md` §9.4): truncated and mutated outputs,
//! inputs and maps give findings or errors, never a panic.

mod common;

use common::{Linker, Size, corpus, verify_with};
use mortar_plumb::Format;

/// A small deterministic generator (xorshift64*).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Byte positions worth corrupting: the headers, then a sample of the rest.
fn positions(len: usize, rng: &mut Rng, extra: usize) -> Vec<usize> {
    let mut v: Vec<usize> = (0..len.min(512)).collect();
    v.extend((0..extra).map(|_| rng.below(len)));
    v
}

// spec/plumb.md §9.4: the output truncated anywhere.
#[test]
fn s9_4_truncated_outputs() {
    let Some(c) = corpus(Size::Small) else { return };
    for case in ["start", "types", "veneer"] {
        for linker in Linker::ALL {
            let link = c.get(case, linker);
            let map = link.parse_map();
            let files = link.inputs(&map);
            let out = link.output_bytes();
            let step = (out.len() / 400).max(1);
            for len in (0..out.len().min(1024)).chain((1024..out.len()).step_by(step)) {
                let r = verify_with(&out[..len], &files, &map);
                assert!(
                    !r.clean() || len == out.len(),
                    "{} truncated to {len} is clean",
                    link.label()
                );
            }
        }
    }
}

// spec/plumb.md §9.4: the output with bytes overwritten.
#[test]
fn s9_4_mutated_outputs() {
    let Some(c) = corpus(Size::Small) else { return };
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for case in ["start", "types", "got", "erratum"] {
        for linker in Linker::ALL {
            let link = c.get(case, linker);
            let map = link.parse_map();
            let files = link.inputs(&map);
            let out = link.output_bytes();
            for at in positions(out.len(), &mut rng, 600) {
                for v in [0x00, 0xff, out[at] ^ 0x80] {
                    let mut m = out.clone();
                    m[at] = v;
                    let _ = verify_with(&m, &files, &map);
                }
            }
        }
    }
}

// spec/plumb.md §9.4: the inputs truncated and mutated.
#[test]
fn s9_4_mutated_inputs() {
    let Some(c) = corpus(Size::Small) else { return };
    let mut rng = Rng(0x0123_4567_89ab_cdef);
    for case in ["types", "got"] {
        for linker in Linker::ALL {
            let link = c.get(case, linker);
            let map = link.parse_map();
            let files = link.inputs(&map);
            let out = link.output_bytes();
            for i in 0..files.len() {
                let len = files[i].1.len();
                for cut in (0..len).step_by((len / 200).max(1)) {
                    let mut f = files.clone();
                    f[i].1.truncate(cut);
                    let r = verify_with(&out, &f, &map);
                    assert!(!r.clean(), "{}: input {} cut to {cut} is clean", link.label(), f[i].0);
                }
                for at in positions(len, &mut rng, 400) {
                    let mut f = files.clone();
                    f[i].1[at] ^= 1 << rng.below(8);
                    let _ = verify_with(&out, &f, &map);
                }
            }
        }
    }
}

// spec/plumb.md §9.4, §3.7: every map format reader, on truncated and
// mutated maps: an error value or a map, which then verifies without panic.
#[test]
fn s9_4_mutated_maps() {
    let Some(c) = corpus(Size::Small) else { return };
    let mut rng = Rng(0xdead_beef_cafe_f00d);
    let alphabet = b" \t\n0123456789abcdefx().:+*-/%_ABCZ[]";
    for case in ["start", "types"] {
        for linker in Linker::ALL {
            let link = c.get(case, linker);
            let base = link.parse_map();
            let files = link.inputs(&base);
            let out = link.output_bytes();
            let mortar = mortar_plumb::map::mortar::write(&base);
            for (format, text) in
                [(link.linker.format(), link.map_text()), (Format::Mortar, mortar)]
            {
                let bytes = text.as_bytes();
                let try_map = |b: &[u8]| {
                    let t = String::from_utf8_lossy(b);
                    if let Ok(m) = mortar_plumb::map::parse(format, &t) {
                        let _ = verify_with(&out, &files, &m);
                    }
                };
                let step = (bytes.len() / 150).max(1);
                for cut in (0..bytes.len()).step_by(step) {
                    try_map(&bytes[..cut]);
                }
                for _ in 0..400 {
                    let mut m = bytes.to_vec();
                    for _ in 0..=rng.below(3) {
                        let at = rng.below(m.len());
                        m[at] = alphabet[rng.below(alphabet.len())];
                    }
                    try_map(&m);
                }
            }
        }
    }
}

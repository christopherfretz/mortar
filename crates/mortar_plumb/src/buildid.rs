//! B1, mortar's build id (`spec/plumb.md` §6.7, `spec/image.md` I-72,
//! I-114, request R-8): the verifier recomputes the id of an output whose
//! map is mortar's. SHA-1 and SHA-256 are written from FIPS 180-4 and
//! `L128` from image.md I-114, not taken from `mortar_digest`.

// FIPS 180-4 and I-114 name their variables a, b, c, ...; so does this.
#![allow(clippy::many_single_char_names)]

use mortar_elf::consts::{SHF_ALLOC, SHT_NOTE};

use crate::image::Image;
use crate::opts::{BuildId, Options};
use crate::report::{Coverage, Finding, Rule};

const NT_GNU_BUILD_ID: u32 = 3;
/// `IMAGE_BLOCK`: the leaf size (I-72).
const B: usize = 1 << 20;
const TAG: &[u8] = b"mortar-build-id-2\0";

// ---- FIPS 180-4 SHA-1 ----

/// SHA-1 of `data` (FIPS 180-4 §6.1).
pub(crate) fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476, 0xc3d2_e1f0];
    for block in Padded::new(data) {
        let mut w = [0u32; 80];
        for (t, c) in block.chunks_exact(4).enumerate() {
            w[t] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        for t in 16..80 {
            w[t] = (w[t - 3] ^ w[t - 8] ^ w[t - 14] ^ w[t - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (t, &wt) in w.iter().enumerate() {
            let (f, k) = match t {
                0..=19 => ((b & c) | (!b & d), 0x5a82_7999),
                20..=39 => (b ^ c ^ d, 0x6ed9_eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc),
                _ => (b ^ c ^ d, 0xca62_c1d6),
            };
            let tmp =
                a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(wt);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = tmp;
        }
        for (x, v) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(v);
        }
    }
    let mut out = [0u8; 20];
    for (o, x) in out.chunks_exact_mut(4).zip(h) {
        o.copy_from_slice(&x.to_be_bytes());
    }
    out
}

// ---- FIPS 180-4 SHA-256 ----

const K256: [u32; 64] = [
    0x428a_2f98,
    0x7137_4491,
    0xb5c0_fbcf,
    0xe9b5_dba5,
    0x3956_c25b,
    0x59f1_11f1,
    0x923f_82a4,
    0xab1c_5ed5,
    0xd807_aa98,
    0x1283_5b01,
    0x2431_85be,
    0x550c_7dc3,
    0x72be_5d74,
    0x80de_b1fe,
    0x9bdc_06a7,
    0xc19b_f174,
    0xe49b_69c1,
    0xefbe_4786,
    0x0fc1_9dc6,
    0x240c_a1cc,
    0x2de9_2c6f,
    0x4a74_84aa,
    0x5cb0_a9dc,
    0x76f9_88da,
    0x983e_5152,
    0xa831_c66d,
    0xb003_27c8,
    0xbf59_7fc7,
    0xc6e0_0bf3,
    0xd5a7_9147,
    0x06ca_6351,
    0x1429_2967,
    0x27b7_0a85,
    0x2e1b_2138,
    0x4d2c_6dfc,
    0x5338_0d13,
    0x650a_7354,
    0x766a_0abb,
    0x81c2_c92e,
    0x9272_2c85,
    0xa2bf_e8a1,
    0xa81a_664b,
    0xc24b_8b70,
    0xc76c_51a3,
    0xd192_e819,
    0xd699_0624,
    0xf40e_3585,
    0x106a_a070,
    0x19a4_c116,
    0x1e37_6c08,
    0x2748_774c,
    0x34b0_bcb5,
    0x391c_0cb3,
    0x4ed8_aa4a,
    0x5b9c_ca4f,
    0x682e_6ff3,
    0x748f_82ee,
    0x78a5_636f,
    0x84c8_7814,
    0x8cc7_0208,
    0x90be_fffa,
    0xa450_6ceb,
    0xbef9_a3f7,
    0xc671_78f2,
];

/// SHA-256 of `data` (FIPS 180-4 §6.2).
pub(crate) fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09_e667,
        0xbb67_ae85,
        0x3c6e_f372,
        0xa54f_f53a,
        0x510e_527f,
        0x9b05_688c,
        0x1f83_d9ab,
        0x5be0_cd19,
    ];
    for block in Padded::new(data) {
        let mut w = [0u32; 64];
        for (t, c) in block.chunks_exact(4).enumerate() {
            w[t] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        for t in 16..64 {
            let s0 = w[t - 15].rotate_right(7) ^ w[t - 15].rotate_right(18) ^ (w[t - 15] >> 3);
            let s1 = w[t - 2].rotate_right(17) ^ w[t - 2].rotate_right(19) ^ (w[t - 2] >> 10);
            w[t] = w[t - 16].wrapping_add(s0).wrapping_add(w[t - 7]).wrapping_add(s1);
        }
        let mut v = h;
        for t in 0..64 {
            let [a, b, c, d, e, f, g, hh] = v;
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K256[t]).wrapping_add(w[t]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            v = [t1.wrapping_add(t2), a, b, c, d.wrapping_add(t1), e, f, g];
        }
        for (x, y) in h.iter_mut().zip(v) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 32];
    for (o, x) in out.chunks_exact_mut(4).zip(h) {
        o.copy_from_slice(&x.to_be_bytes());
    }
    out
}

/// The 64-byte blocks of a message padded as FIPS 180-4 §5.1.1 says.
struct Padded<'a> {
    data: &'a [u8],
    at: usize,
    tail: Vec<u8>,
    tail_at: Option<usize>,
}

impl<'a> Padded<'a> {
    fn new(data: &'a [u8]) -> Self {
        let whole = data.len() / 64 * 64;
        let mut tail = data[whole..].to_vec();
        tail.push(0x80);
        while tail.len() % 64 != 56 {
            tail.push(0);
        }
        tail.extend_from_slice(&((data.len() as u64).wrapping_mul(8)).to_be_bytes());
        Padded { data: &data[..whole], at: 0, tail, tail_at: None }
    }
}

impl Iterator for Padded<'_> {
    type Item = [u8; 64];
    fn next(&mut self) -> Option<[u8; 64]> {
        let mut b = [0u8; 64];
        if self.at < self.data.len() {
            b.copy_from_slice(&self.data[self.at..self.at + 64]);
            self.at += 64;
            return Some(b);
        }
        let t = self.tail_at.unwrap_or(0);
        if t >= self.tail.len() {
            return None;
        }
        b.copy_from_slice(&self.tail[t..t + 64]);
        self.tail_at = Some(t + 64);
        Some(b)
    }
}

// ---- image.md I-114 L128 ----

const P1: u64 = 0x9E37_79B1_85EB_CA87;
const P2: u64 = 0xC2B2_AE3D_27D4_EB4F;
const P3: u64 = 0x1656_67B1_9E37_79F9;
const P4: u64 = 0x85EB_CA77_C2B2_AE63;

fn round(v: u64, w: u64) -> u64 {
    v.wrapping_add(w.wrapping_mul(P2)).rotate_left(31).wrapping_mul(P1)
}

fn fmix(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(P2);
    x ^= x >> 29;
    x = x.wrapping_mul(P3);
    x ^= x >> 32;
    x
}

/// `L128` (image.md I-114).
pub(crate) fn l128(d: &[u8]) -> [u8; 16] {
    let mut v = [P1.wrapping_add(P2), P2, 0, 0u64.wrapping_sub(P1)];
    let stripe = |v: &mut [u64; 4], s: &[u8]| {
        for (k, lane) in v.iter_mut().enumerate() {
            let mut w = [0u8; 8];
            w.copy_from_slice(&s[8 * k..8 * k + 8]);
            *lane = round(*lane, u64::from_le_bytes(w));
        }
    };
    let mut chunks = d.chunks_exact(32);
    for s in &mut chunks {
        stripe(&mut v, s);
    }
    let rest = chunks.remainder();
    if !rest.is_empty() {
        let mut s = [0u8; 32];
        s[..rest.len()].copy_from_slice(rest);
        stripe(&mut v, &s);
    }
    let n = d.len() as u64;
    let a = fmix(
        v[0].rotate_left(1)
            ^ v[1].rotate_left(7)
            ^ v[2].rotate_left(12)
            ^ v[3].rotate_left(18)
            ^ n.wrapping_mul(P4),
    );
    let b = fmix(round(round(round(round(n, v[3]), v[2]), v[1]), v[0]) ^ a.wrapping_mul(P3));
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&a.to_le_bytes());
    out[8..].copy_from_slice(&b.to_le_bytes());
    out
}

/// The three trees of I-72.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tree {
    /// `L128` leaves, SHA-256 root (bare and `fast`).
    L128,
    /// SHA-1 leaves and root (`sha1`).
    Sha1,
    /// SHA-256 leaves and root (`sha256`).
    Sha256,
}

/// The root of I-72's tree over `f`.
#[cfg(test)]
pub(crate) fn tree(t: Tree, f: &[u8]) -> Vec<u8> {
    tree_zeroed(t, f, 0..0)
}

/// The root of I-72's tree over `f` with the bytes `zero` read as zeros:
/// only the leaves that overlap `zero` are copied.
pub fn tree_zeroed(t: Tree, f: &[u8], zero: std::ops::Range<usize>) -> Vec<u8> {
    let mut root = Vec::with_capacity(34 + f.len().div_ceil(B) * 32);
    root.extend_from_slice(TAG);
    root.extend_from_slice(&(f.len() as u64).to_le_bytes());
    root.extend_from_slice(&(B as u64).to_le_bytes());
    for (i, leaf) in f.chunks(B).enumerate() {
        let (lo, hi) = (i * B, i * B + leaf.len());
        let copy;
        let leaf = if zero.start < hi && lo < zero.end {
            let mut v = leaf.to_vec();
            v[zero.start.max(lo) - lo..zero.end.min(hi) - lo].fill(0);
            copy = v;
            &copy[..]
        } else {
            leaf
        };
        match t {
            Tree::L128 => root.extend_from_slice(&l128(leaf)),
            Tree::Sha1 => root.extend_from_slice(&sha1(leaf)),
            Tree::Sha256 => root.extend_from_slice(&sha256(leaf)),
        }
    }
    match t {
        Tree::L128 | Tree::Sha256 => sha256(&root).to_vec(),
        Tree::Sha1 => sha1(&root).to_vec(),
    }
}

/// The `NT_GNU_BUILD_ID` descriptors of the allocated notes: (address,
/// file offset, length). A note that runs past its section ends the walk
/// (S11 reports it).
fn ids(image: &Image<'_>) -> Vec<(u64, usize, usize)> {
    let mut found = Vec::new();
    for (i, s) in image.sections.iter().enumerate().skip(1) {
        if s.shdr.sh_type != SHT_NOTE || s.shdr.sh_flags & SHF_ALLOC == 0 {
            continue;
        }
        let Some(data) = image.section_data(i) else { continue };
        let Ok(base) = usize::try_from(s.shdr.sh_offset) else { continue };
        let mut at = 0usize;
        while let Some(h) = data.get(at..at + 12) {
            let word = |k: usize| u32::from_le_bytes([h[k], h[k + 1], h[k + 2], h[k + 3]]);
            let (namesz, descsz, ty) = (word(0) as usize, word(4) as usize, word(8));
            let Some(desc) =
                namesz.checked_next_multiple_of(4).and_then(|n| n.checked_add(at + 12))
            else {
                break;
            };
            let Some(end) = desc.checked_add(descsz).filter(|&e| e <= data.len()) else { break };
            if ty == NT_GNU_BUILD_ID && data.get(at + 12..at + 12 + namesz) == Some(b"GNU\0") {
                found.push((s.shdr.sh_addr.wrapping_add(desc as u64), base + desc, descsz));
            }
            let Some(next) = end.checked_next_multiple_of(4) else { break };
            at = next;
        }
    }
    found
}

/// The trees an id of `len` bytes may be the prefix of (I-72).
fn trees_for(len: usize) -> &'static [Tree] {
    match len {
        8 => &[Tree::L128],
        20 => &[Tree::L128, Tree::Sha1],
        32 => &[Tree::Sha256],
        _ => &[],
    }
}

/// Rewrites every 8-, 20- or 32-byte build id of the ELF file `file` as
/// mortar would have written it (`style`'s tree; the bare style for 8 and
/// 20 bytes when `style` does not fit the length) and returns how many it
/// rewrote. For tests and tools that turn another linker's output into
/// one B1 accepts; the verifier itself never calls it.
pub fn seal(file: &mut [u8], style: Tree) -> usize {
    let found = match Image::read(file) {
        Ok(image) => ids(&image),
        Err(_) => return 0,
    };
    let mut n = 0;
    for (_, off, len) in found {
        let trees = trees_for(len);
        let Some(&t) = trees.iter().find(|&&t| t == style).or(trees.first()) else { continue };
        file[off..off + len].fill(0);
        let root = tree_zeroed(t, file, 0..0);
        file[off..off + len].copy_from_slice(&root[..len]);
        n += 1;
    }
    n
}

/// The file ranges of the build-id descriptors of `file` (every
/// `NT_GNU_BUILD_ID` note of an allocated `SHT_NOTE` section, whatever
/// its length): the bytes [`reseal`] rewrites.
pub fn id_ranges(file: &[u8]) -> Vec<std::ops::Range<usize>> {
    match Image::read(file) {
        Ok(image) => ids(&image).into_iter().map(|(_, off, len)| off..off + len).collect(),
        Err(_) => Vec::new(),
    }
}

/// Reseals mortar's build id after an edit of `file`, as a linker that
/// wrote the edited bytes would have sealed them (§6.7, §9.5, §12.3):
/// under mortar's map only, by the tree its style names (`L128` when it
/// names none); a `given` or unknown style, and every other linker's id
/// (not mortar's scheme, never judged), are left alone. The number of
/// ids rewritten. One implementation for the mutation tool and the
/// sweep.
pub fn reseal(file: &mut [u8], map: &crate::Map) -> usize {
    if map.format != crate::Format::Mortar {
        return 0;
    }
    match Style::of(map, None) {
        Style::Tree(t, _) => seal(file, t),
        Style::Unstated => seal(file, Tree::L128),
        Style::Given | Style::Unknown => 0,
    }
}

/// The style mortar's map states for the build id (§6.7): the target
/// token of its `chunk synthetic .note.gnu.build-id` record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// The line asks for no note (B1 then finds the note by its
    /// presence): any tree that fits the length is accepted.
    Unstated,
    /// `default`, `sha1`, `sha256` or `fast` (image.md I-69): the tree
    /// and the length.
    Tree(Tree, usize),
    /// `given`: the bytes came from the command line (`0xHEX`); not judged.
    Given,
    /// A token this version does not know: not judged, and said so.
    Unknown,
}

impl Style {
    /// The style of `map`'s build id: its line's (§3.2a, the last of
    /// `--build-id[=STYLE]` and `--no-build-id`). A line that asks for no
    /// note, or a map that is not mortar's, gives [`Style::Unstated`]:
    /// [`line_rules`] judges the presence.
    pub fn of(map: &crate::Map, line: Option<&[String]>) -> Style {
        crate::opts::of(map, line).and_then(|o| Self::asked(&o.build_id)).unwrap_or(Style::Unstated)
    }

    /// The style a line asks for; `None`: no note (I-69).
    pub fn asked(b: &BuildId) -> Option<Style> {
        match b {
            BuildId::None => None,
            BuildId::Given => Some(Style::Given),
            BuildId::Style(s) => Some(Self::named(s)),
        }
    }

    fn named(token: &str) -> Style {
        match token {
            "default" => Style::Tree(Tree::L128, 20),
            "fast" => Style::Tree(Tree::L128, 8),
            "sha1" => Style::Tree(Tree::Sha1, 20),
            "sha256" => Style::Tree(Tree::Sha256, 32),
            "given" => Style::Given,
            _ => Style::Unknown,
        }
    }
}

/// The target token of `map`'s `chunk synthetic .note.gnu.build-id`.
fn chunk_style(map: &crate::Map) -> Option<&str> {
    map.chunks
        .iter()
        .find(|c| c.kind == crate::map::ChunkKind::Synthetic && c.name == ".note.gnu.build-id")?
        .target
        .as_ref()
        .map(|t| t.0.as_str())
}

/// B1's line clauses (§6.7, §3.2a), for a mortar map with `arg`
/// records: the output has a build-id note exactly when the line asks
/// for one, and a style the map's chunk states is the line's.
pub(crate) fn line_rules(image: &Image<'_>, map: &crate::Map, opts: &Options) -> Vec<Finding> {
    let mut out = Vec::new();
    let found = ids(image);
    let asked = Style::asked(&opts.build_id);
    match (&asked, found.first()) {
        (None, Some(&(addr, _, len))) => out.push(
            Finding::new(
                Rule::BuildId,
                format!(
                    "a {len}-byte build id the line does not ask for (no --build-id, or \
                     --build-id=none or --no-build-id last; spec/image.md I-69)"
                ),
            )
            .at(addr),
        ),
        (Some(_), None) => out.push(Finding::new(
            Rule::BuildId,
            "no build-id note; the line asks for one (--build-id, spec/image.md I-69)",
        )),
        _ => {}
    }
    if let (Some(a), Some(t)) = (&asked, chunk_style(map))
        && Style::named(t) != *a
    {
        out.push(Finding::new(
            Rule::BuildId,
            format!("the map's build-id chunk states the style '{t}'; the line asks for {a:?}"),
        ));
    }
    out
}

/// B1 (§6.7), for mortar's map only: the id of an `NT_GNU_BUILD_ID` note
/// is I-72's function of the output, by the tree the map's style names
/// (or, when the map names none, any tree that fits an 8-, 20- or 32-byte
/// id).
pub(crate) fn check(image: &Image<'_>, cov: &mut Coverage, style: Style) -> Vec<Finding> {
    let mut out = Vec::new();
    for (addr, off, len) in ids(image) {
        if off.checked_add(len).is_none_or(|e| e > image.data.len()) {
            continue;
        }
        let one;
        let trees = match style {
            Style::Unstated => trees_for(len),
            Style::Given => {
                cov.build_id = Some(format!("{len}-byte id given on the command line, not judged"));
                continue;
            }
            Style::Unknown => {
                cov.build_id = Some(format!(
                    "{len}-byte id of a style the map names and this verifier does not know, not judged"
                ));
                continue;
            }
            Style::Tree(t, want) => {
                if want != len {
                    cov.build_id = Some(format!("{len}-byte id wrong"));
                    let mut x = Finding::new(
                        Rule::BuildId,
                        format!("the build id has {len} bytes; its style ({t:?} tree) writes {want} (spec/image.md I-69)"),
                    )
                    .at(addr);
                    x.expected = Some(want as u64);
                    x.found = Some(len as u64);
                    out.push(x);
                    continue;
                }
                one = [t];
                &one[..]
            }
        };
        if trees.is_empty() {
            cov.build_id = Some(format!("{len}-byte id not judged"));
            continue;
        }
        let id = &image.data[off..off + len];
        let matched =
            trees.iter().find(|&&t| tree_zeroed(t, image.data, off..off + len)[..len] == *id);
        if let Some(t) = matched {
            cov.build_id = Some(format!("{len}-byte id checked ({t:?} tree)"));
        } else {
            cov.build_id = Some(format!("{len}-byte id wrong"));
            let mut x = Finding::new(
                Rule::BuildId,
                format!(
                    "the {len}-byte build id is not spec/image.md I-72's function of the output"
                ),
            )
            .at(addr);
            let mut w = [0u8; 8];
            w.copy_from_slice(&id[..8]);
            x.found = Some(u64::from_be_bytes(w));
            out.push(x);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().fold(String::new(), |mut s, x| {
            let _ = write!(s, "{x:02x}");
            s
        })
    }

    fn pattern(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i % 251) as u8).collect()
    }

    // FIPS 180-4's examples (and the empty message).
    #[test]
    fn s6_7_sha_vectors() {
        assert_eq!(hex(&sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(
            hex(&sha1(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    // spec/image.md §8.4: L128.
    #[test]
    fn s6_7_l128_vectors() {
        let cases: &[(Vec<u8>, &str)] = &[
            (Vec::new(), "166e5d750a9987ba700b8ae8169c08f1"),
            (b"abc".to_vec(), "34d9933929a69d420194f31701b62a6e"),
            (pattern(1), "a9b2c13446ed2ae78d9276aa7e6a453c"),
            (pattern(7), "d8ba3acb3828bf91099bcd2dc39f4672"),
            (pattern(8), "8b8a2b39f5629b5caaa295deca45fa41"),
            (pattern(31), "30de9b5bf64bf64bc7885eaab7d8ddd5"),
            (pattern(32), "cf8b6fc0c63e046e161c68c0466a00e5"),
            (pattern(33), "70094889eb36eb1e8e4f9a079d66511b"),
            (pattern(63), "5bf246f31e37031982482cf3c6f97f75"),
            (pattern(64), "53d570ba718049ac4882965680303b32"),
            (pattern(65), "b15eb71a90c7abc48e43144b543f840e"),
            (pattern(4096), "f570b658e524275c4140b619d155cab9"),
            (pattern(B - 1), "cb9e12add796f4ed147b7108555b66f7"),
            (pattern(B), "c6e1dfc9cc045a72cf3a72032a298d32"),
            (pattern(B + 1), "1234f10fe0f80d6c59c2724939331490"),
        ];
        for (d, want) in cases {
            assert_eq!(hex(&l128(d)), *want, "L128 of {} bytes", d.len());
        }
    }

    // spec/image.md §8.4: the roots of I-72.
    #[test]
    fn s6_7_tree_vectors() {
        let rows: &[(usize, &str, &str, &str)] = &[
            (
                0,
                "598ec1d4dd3c72cee90920b43faf73a539ad84fd3eb148f8b1c5f4362be50bcb",
                "e17559e60537aa8405a216c63d1680f54fe53601",
                "598ec1d4dd3c72cee90920b43faf73a539ad84fd3eb148f8b1c5f4362be50bcb",
            ),
            (
                1,
                "2df0b93caf760623c40f3781b8e01397e19f7230094ebf68d67a4fa922ca5082",
                "f78bec69d0b07408559e32b98a419a7b111caed9",
                "5b94dbbfe23e7efbfd1ccc5a410d926b54a06d828aa57ba9efa5cc4de7e0ed62",
            ),
            (
                B + 1,
                "be75a9986f7fe460609c23a5432a777d0c5c9106721d52b09522cfd3a5162b09",
                "9c9df507ac24501067432862ecf325e57b4aa0a4",
                "2b2777fb577eeeed88d735f2577239e248be71cbf97b4170d1ccf34a91aad675",
            ),
        ];
        for &(n, l, s1, s2) in rows {
            let f = pattern(n);
            assert_eq!(hex(&tree(Tree::L128, &f)), l, "L128 tree of {n}");
            assert_eq!(hex(&tree(Tree::Sha1, &f)), s1, "SHA-1 tree of {n}");
            assert_eq!(hex(&tree(Tree::Sha256, &f)), s2, "SHA-256 tree of {n}");
        }
        let mut f = pattern(3 * B + 5);
        f[B - 6..B + 14].fill(0);
        assert_eq!(
            hex(&tree(Tree::L128, &f)),
            "31ff17b9820dd9f21410ccd27c9bcd3993f325613f8c070cad73c613c8faed25"
        );
    }
}

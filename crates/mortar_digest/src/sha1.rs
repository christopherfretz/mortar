//! SHA-1, from FIPS 180-4 §6.1 (also RFC 3174).

use crate::block::{BLOCK, Blocks, be32};

/// Bytes of a SHA-1 digest.
pub const SHA1_LEN: usize = 20;

const H0: [u32; 5] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];

/// A SHA-1 computation fed in pieces.
#[derive(Clone, Debug)]
pub struct Sha1 {
    h: [u32; 5],
    blocks: Blocks,
}

impl Default for Sha1 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha1 {
    /// The digest of no bytes so far.
    pub const fn new() -> Self {
        Self { h: H0, blocks: Blocks::new() }
    }

    /// Appends `data` to the message.
    pub fn update(&mut self, data: &[u8]) {
        let h = &mut self.h;
        self.blocks.update(data, |b| compress(h, b));
    }

    /// The digest of the message.
    pub fn finish(mut self) -> [u8; SHA1_LEN] {
        let h = &mut self.h;
        self.blocks.finish(|b| compress(h, b));
        let mut out = [0; SHA1_LEN];
        for (o, w) in out.chunks_exact_mut(4).zip(self.h) {
            o.copy_from_slice(&w.to_be_bytes());
        }
        out
    }
}

/// SHA-1 of `data`.
pub fn sha1(data: &[u8]) -> [u8; SHA1_LEN] {
    let mut s = Sha1::new();
    s.update(data);
    s.finish()
}

/// Schedule word `i` (§6.1.2 step 1), kept as the last 16 words.
#[inline]
fn word(w: &mut [u32; 16], i: usize) -> u32 {
    if i >= 16 {
        let x = w[(i + 13) & 15] ^ w[(i + 8) & 15] ^ w[(i + 2) & 15] ^ w[i & 15];
        w[i & 15] = x.rotate_left(1);
    }
    w[i & 15]
}

/// Steps `t .. t + 20` of §6.1.2 step 3, with the group's `f(b, c, d)` and
/// `K`. Instead of moving the five variables every step, their roles
/// rotate: step `i` writes the variable that is `e` at that step, and five
/// steps bring the roles back.
macro_rules! group {
    ($w:ident, $t:expr, $k:expr, $f:expr, $a:ident, $b:ident, $c:ident, $d:ident, $e:ident) => {
        let mut i = $t;
        while i < $t + 20 {
            group!(@step $w, i, $k, $f, $a, $b, $c, $d, $e);
            group!(@step $w, i + 1, $k, $f, $e, $a, $b, $c, $d);
            group!(@step $w, i + 2, $k, $f, $d, $e, $a, $b, $c);
            group!(@step $w, i + 3, $k, $f, $c, $d, $e, $a, $b);
            group!(@step $w, i + 4, $k, $f, $b, $c, $d, $e, $a);
            i += 5;
        }
    };
    (@step $w:ident, $i:expr, $k:expr, $f:expr, $a:ident, $b:ident, $c:ident, $d:ident, $e:ident) => {
        let wi = word(&mut $w, $i);
        $e = $a
            .rotate_left(5)
            .wrapping_add($f($b, $c, $d))
            .wrapping_add($e)
            .wrapping_add($k)
            .wrapping_add(wi);
        $b = $b.rotate_left(30);
    };
}

#[inline]
fn ch(x: u32, y: u32, z: u32) -> u32 {
    (x & y) | (!x & z)
}
#[inline]
fn parity(x: u32, y: u32, z: u32) -> u32 {
    x ^ y ^ z
}
#[inline]
fn maj(x: u32, y: u32, z: u32) -> u32 {
    (x & y) | (x & z) | (y & z)
}

/// Processes whole blocks (`blocks.len()` a multiple of 64).
#[allow(clippy::many_single_char_names)] // FIPS 180-4's a, b, c, …
fn compress(h: &mut [u32; 5], blocks: &[u8]) {
    debug_assert!(blocks.len().is_multiple_of(BLOCK), "sha1: partial block");
    for block in blocks.chunks_exact(BLOCK) {
        let mut w = [0u32; 16];
        for (t, wt) in w.iter_mut().enumerate() {
            *wt = be32(block, t);
        }
        let [mut a, mut b, mut c, mut d, mut e] = *h;
        group!(w, 0, 0x5A82_7999, ch, a, b, c, d, e);
        group!(w, 20, 0x6ED9_EBA1, parity, a, b, c, d, e);
        group!(w, 40, 0x8F1B_BCDC, maj, a, b, c, d, e);
        group!(w, 60, 0xCA62_C1D6, parity, a, b, c, d, e);
        for (hi, v) in h.iter_mut().zip([a, b, c, d, e]) {
            *hi = hi.wrapping_add(v);
        }
    }
}

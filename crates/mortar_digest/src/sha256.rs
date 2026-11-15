//! SHA-256, from FIPS 180-4 §6.2.

use crate::block::{BLOCK, Blocks, be32};

/// Bytes of a SHA-256 digest.
pub const SHA256_LEN: usize = 32;

/// §5.3.3: the first 32 bits of the fractional parts of the square roots
/// of the first eight primes.
const H0: [u32; 8] = [
    0x6A09_E667,
    0xBB67_AE85,
    0x3C6E_F372,
    0xA54F_F53A,
    0x510E_527F,
    0x9B05_688C,
    0x1F83_D9AB,
    0x5BE0_CD19,
];

/// §4.2.2: the first 32 bits of the fractional parts of the cube roots of
/// the first sixty-four primes.
pub(crate) const K: [u32; 64] = [
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

/// The initial hash value, for the test that derives it from the primes.
#[cfg(test)]
pub(crate) const INITIAL: [u32; 8] = H0;

/// A SHA-256 computation fed in pieces.
#[derive(Clone, Debug)]
pub struct Sha256 {
    h: [u32; 8],
    blocks: Blocks,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
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
    pub fn finish(mut self) -> [u8; SHA256_LEN] {
        let h = &mut self.h;
        self.blocks.finish(|b| compress(h, b));
        let mut out = [0; SHA256_LEN];
        for (o, w) in out.chunks_exact_mut(4).zip(self.h) {
            o.copy_from_slice(&w.to_be_bytes());
        }
        out
    }
}

/// SHA-256 of `data`.
pub fn sha256(data: &[u8]) -> [u8; SHA256_LEN] {
    let mut s = Sha256::new();
    s.update(data);
    s.finish()
}

/// Schedule word `i` (§6.2.2 step 1), kept as the last 16 words.
#[inline]
fn word(w: &mut [u32; 16], i: usize) -> u32 {
    if i >= 16 {
        let w15 = w[(i + 1) & 15];
        let w2 = w[(i + 14) & 15];
        let s0 = w15.rotate_right(7) ^ w15.rotate_right(18) ^ (w15 >> 3);
        let s1 = w2.rotate_right(17) ^ w2.rotate_right(19) ^ (w2 >> 10);
        w[i & 15] = s1.wrapping_add(w[(i + 9) & 15]).wrapping_add(s0).wrapping_add(w[i & 15]);
    }
    w[i & 15]
}

/// Steps `i .. i + 8` of §6.2.2 step 3. The eight variables' roles rotate
/// instead of their values: step `i` adds `T1` into the variable that is
/// `d` and writes `T1 + T2` into the one that is `h`, which becomes `a`.
macro_rules! eight {
    ($w:ident, $i:expr, $a:ident, $b:ident, $c:ident, $d:ident, $e:ident, $f:ident, $g:ident, $h:ident) => {
        eight!(@step $w, $i, $a, $b, $c, $d, $e, $f, $g, $h);
        eight!(@step $w, $i + 1, $h, $a, $b, $c, $d, $e, $f, $g);
        eight!(@step $w, $i + 2, $g, $h, $a, $b, $c, $d, $e, $f);
        eight!(@step $w, $i + 3, $f, $g, $h, $a, $b, $c, $d, $e);
        eight!(@step $w, $i + 4, $e, $f, $g, $h, $a, $b, $c, $d);
        eight!(@step $w, $i + 5, $d, $e, $f, $g, $h, $a, $b, $c);
        eight!(@step $w, $i + 6, $c, $d, $e, $f, $g, $h, $a, $b);
        eight!(@step $w, $i + 7, $b, $c, $d, $e, $f, $g, $h, $a);
    };
    (@step $w:ident, $i:expr, $a:ident, $b:ident, $c:ident, $d:ident, $e:ident, $f:ident, $g:ident, $h:ident) => {
        let wi = word(&mut $w, $i);
        let s1 = $e.rotate_right(6) ^ $e.rotate_right(11) ^ $e.rotate_right(25);
        let ch = ($e & $f) ^ (!$e & $g);
        let t1 = $h.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[$i]).wrapping_add(wi);
        let s0 = $a.rotate_right(2) ^ $a.rotate_right(13) ^ $a.rotate_right(22);
        let maj = ($a & $b) ^ ($a & $c) ^ ($b & $c);
        $d = $d.wrapping_add(t1);
        $h = t1.wrapping_add(s0.wrapping_add(maj));
    };
}

/// Processes whole blocks (`blocks.len()` a multiple of 64).
#[allow(clippy::many_single_char_names)] // FIPS 180-4's a, b, c, …
fn compress(h: &mut [u32; 8], blocks: &[u8]) {
    debug_assert!(blocks.len().is_multiple_of(BLOCK), "sha256: partial block");
    for block in blocks.chunks_exact(BLOCK) {
        let mut w = [0u32; 16];
        for (t, wt) in w.iter_mut().enumerate() {
            *wt = be32(block, t);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
        let mut i = 0;
        while i < 64 {
            eight!(w, i, a, b, c, d, e, f, g, hh);
            i += 8;
        }
        for (hi, v) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *hi = hi.wrapping_add(v);
        }
    }
}

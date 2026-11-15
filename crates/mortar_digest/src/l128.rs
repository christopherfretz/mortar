//! `L128`, the build id's 128-bit leaf hash (`spec/image.md` I-114).

/// Bytes of an `L128` digest.
pub const L128_LEN: usize = 16;

const P1: u64 = 0x9E37_79B1_85EB_CA87;
const P2: u64 = 0xC2B2_AE3D_27D4_EB4F;
const P3: u64 = 0x1656_67B1_9E37_79F9;
const P4: u64 = 0x85EB_CA77_C2B2_AE63;

/// Bytes of one stripe.
const STRIPE: usize = 32;

#[inline]
fn round(v: u64, w: u64) -> u64 {
    v.wrapping_add(w.wrapping_mul(P2)).rotate_left(31).wrapping_mul(P1)
}

#[inline]
fn fmix(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(P2);
    x ^= x >> 29;
    x = x.wrapping_mul(P3);
    x ^= x >> 32;
    x
}

#[inline]
fn le64(b: &[u8]) -> u64 {
    u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

/// The four lanes after the initial value.
const V0: [u64; 4] = [P1.wrapping_add(P2), P2, 0, 0u64.wrapping_sub(P1)];

/// Rounds every whole stripe of `data` (`data.len()` a multiple of 32).
#[inline]
fn stripes(v: &mut [u64; 4], data: &[u8]) {
    let [mut v0, mut v1, mut v2, mut v3] = *v;
    for s in data.chunks_exact(STRIPE) {
        v0 = round(v0, le64(&s[0..8]));
        v1 = round(v1, le64(&s[8..16]));
        v2 = round(v2, le64(&s[16..24]));
        v3 = round(v3, le64(&s[24..32]));
    }
    *v = [v0, v1, v2, v3];
}

/// The final step, once the last (zero-padded) stripe is rounded.
fn digest(v: [u64; 4], n: u64) -> [u8; L128_LEN] {
    let [v0, v1, v2, v3] = v;
    let a = fmix(
        v0.rotate_left(1)
            ^ v1.rotate_left(7)
            ^ v2.rotate_left(12)
            ^ v3.rotate_left(18)
            ^ n.wrapping_mul(P4),
    );
    let b = fmix(round(round(round(round(n, v3), v2), v1), v0) ^ a.wrapping_mul(P3));
    let mut out = [0; L128_LEN];
    out[..8].copy_from_slice(&a.to_le_bytes());
    out[8..].copy_from_slice(&b.to_le_bytes());
    out
}

/// `L128` of `data`.
pub fn l128(data: &[u8]) -> [u8; L128_LEN] {
    let mut v = V0;
    let whole = data.len() - data.len() % STRIPE;
    stripes(&mut v, &data[..whole]);
    let rest = &data[whole..];
    if !rest.is_empty() {
        let mut last = [0; STRIPE];
        last[..rest.len()].copy_from_slice(rest);
        stripes(&mut v, &last);
    }
    digest(v, data.len() as u64)
}

/// An `L128` computation fed in pieces.
#[derive(Clone, Debug)]
pub struct L128 {
    v: [u64; 4],
    buf: [u8; STRIPE],
    used: usize,
    total: u64,
}

impl Default for L128 {
    fn default() -> Self {
        Self::new()
    }
}

impl L128 {
    /// The digest of no bytes so far.
    pub const fn new() -> Self {
        Self { v: V0, buf: [0; STRIPE], used: 0, total: 0 }
    }

    /// Appends `data` to the input.
    pub fn update(&mut self, mut data: &[u8]) {
        self.total = self.total.wrapping_add(data.len() as u64);
        if self.used > 0 {
            let take = (STRIPE - self.used).min(data.len());
            self.buf[self.used..self.used + take].copy_from_slice(&data[..take]);
            self.used += take;
            data = &data[take..];
            if self.used < STRIPE {
                return;
            }
            stripes(&mut self.v, &self.buf);
            self.used = 0;
        }
        let whole = data.len() - data.len() % STRIPE;
        stripes(&mut self.v, &data[..whole]);
        let rest = &data[whole..];
        self.buf[..rest.len()].copy_from_slice(rest);
        self.used = rest.len();
    }

    /// The digest of the input.
    pub fn finish(mut self) -> [u8; L128_LEN] {
        if self.used > 0 {
            self.buf[self.used..].fill(0);
            stripes(&mut self.v, &self.buf);
        }
        digest(self.v, self.total)
    }
}

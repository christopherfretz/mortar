//! The buffering shared by SHA-1 and SHA-256 (FIPS 180-4 §5.1.1, §6):
//! 64-byte blocks, the message padded with `0x80`, zeros, and the
//! message's length in bits as a big-endian 64-bit integer.

/// Bytes of one block.
pub(crate) const BLOCK: usize = 64;

/// Bytes not yet making a whole block, and the message's length so far.
#[derive(Clone, Debug)]
pub(crate) struct Blocks {
    buf: [u8; BLOCK],
    used: usize,
    total: u64,
}

impl Blocks {
    pub(crate) const fn new() -> Self {
        Self { buf: [0; BLOCK], used: 0, total: 0 }
    }

    /// Feeds `data`; `compress` is called with a whole number of blocks
    /// (one or more), in message order.
    pub(crate) fn update(&mut self, mut data: &[u8], mut compress: impl FnMut(&[u8])) {
        self.total = self.total.wrapping_add(data.len() as u64);
        if self.used > 0 {
            let take = (BLOCK - self.used).min(data.len());
            self.buf[self.used..self.used + take].copy_from_slice(&data[..take]);
            self.used += take;
            data = &data[take..];
            if self.used < BLOCK {
                return;
            }
            compress(&self.buf);
            self.used = 0;
        }
        let whole = data.len() - data.len() % BLOCK;
        if whole > 0 {
            compress(&data[..whole]);
        }
        let rest = &data[whole..];
        self.buf[..rest.len()].copy_from_slice(rest);
        self.used = rest.len();
    }

    /// Pads the message and calls `compress` on the last one or two blocks.
    pub(crate) fn finish(mut self, mut compress: impl FnMut(&[u8])) {
        let bits = self.total.wrapping_mul(8);
        self.buf[self.used] = 0x80;
        self.buf[self.used + 1..].fill(0);
        if self.used + 1 > BLOCK - 8 {
            compress(&self.buf);
            self.buf.fill(0);
        }
        self.buf[BLOCK - 8..].copy_from_slice(&bits.to_be_bytes());
        compress(&self.buf);
    }
}

/// The `i`th big-endian word of a block.
#[inline]
pub(crate) fn be32(block: &[u8], i: usize) -> u32 {
    u32::from_be_bytes([block[4 * i], block[4 * i + 1], block[4 * i + 2], block[4 * i + 3]])
}

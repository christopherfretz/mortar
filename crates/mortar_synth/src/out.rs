//! A byte range of a chunk being written (S-4): every write step fills a
//! [`Window`], so the image phase can split a chunk into ranges and fill
//! them in parallel.

/// The bytes `start..start + buf.len()` of a chunk. Writes outside it are
/// clipped, so a writer can emit whole fields and records and only the
/// part inside the window lands.
#[derive(Debug)]
pub struct Window<'a> {
    start: u64,
    buf: &'a mut [u8],
}

impl<'a> Window<'a> {
    /// The window of `buf` at chunk offset `start`.
    #[must_use]
    pub fn new(start: u64, buf: &'a mut [u8]) -> Self {
        Self { start, buf }
    }

    /// The chunk offset of the first byte.
    #[must_use]
    pub fn start(&self) -> u64 {
        self.start
    }

    /// One past the chunk offset of the last byte.
    #[must_use]
    pub fn end(&self) -> u64 {
        self.start + self.buf.len() as u64
    }

    /// Whether `at..at + len` meets the window.
    #[must_use]
    pub fn meets(&self, at: u64, len: u64) -> bool {
        at < self.end() && at.saturating_add(len) > self.start
    }

    /// Writes `bytes` at chunk offset `at`, clipped to the window.
    pub fn put(&mut self, at: u64, bytes: &[u8]) {
        let len = bytes.len() as u64;
        if !self.meets(at, len) {
            return;
        }
        let from = self.start.max(at);
        let to = self.end().min(at + len);
        let src = &bytes[(from - at) as usize..(to - at) as usize];
        self.buf[(from - self.start) as usize..(to - self.start) as usize].copy_from_slice(src);
    }

    /// Writes `len` zero bytes at `at`, clipped.
    pub fn zero(&mut self, at: u64, len: u64) {
        if !self.meets(at, len) {
            return;
        }
        let from = self.start.max(at);
        let to = self.end().min(at.saturating_add(len));
        self.buf[(from - self.start) as usize..(to - self.start) as usize].fill(0);
    }

    /// Writes a little-endian word of `width` bytes (4 or 8) at `at`.
    pub fn word(&mut self, at: u64, width: u8, value: u64) {
        let bytes = value.to_le_bytes();
        self.put(at, &bytes[..usize::from(width)]);
    }
}

/// `align_up(x, a)` for a power of two `a` (synth.md §1).
#[must_use]
pub const fn align_up(x: u64, a: u64) -> u64 {
    (x + a - 1) & !(a - 1)
}

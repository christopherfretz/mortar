//! The build id's tree (I-72, I-73): a leaf per window, hashed by the
//! worker that filled it before the window is written, and a root over the
//! leaves on one thread.
//!
//! Stand-in: `mortar_digest` (WP-026h) owns `L128`, SHA-1 and SHA-256. Fill
//! needs only this trait; the tests use a trivial hasher.

/// The tag of the definition (I-72, I-75).
pub const TAG: &[u8] = b"mortar-build-id-2\0";

/// One hash style (I-72's table): the leaf function `H`, the root `R`, the
/// id's length `L`.
pub trait LeafHasher: Sync {
    /// `H(window)`, appended to `out` (16, 20 or 32 bytes).
    fn leaf(&self, window: &[u8], out: &mut Vec<u8>);
    /// `R(message)`.
    fn root(&self, message: &[u8]) -> Vec<u8>;
    /// `L`.
    fn id_len(&self) -> usize;
}

/// `root[0..L]` of I-72 from the leaves in window order (`leaves` is their
/// concatenation), for a file of `n` bytes in windows of `b`.
#[must_use]
pub fn build_id(hasher: &dyn LeafHasher, n: u64, b: u64, leaves: &[u8]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(TAG.len() + 16 + leaves.len());
    msg.extend_from_slice(TAG);
    msg.extend_from_slice(&n.to_le_bytes());
    msg.extend_from_slice(&b.to_le_bytes());
    msg.extend_from_slice(leaves);
    let mut root = hasher.root(&msg);
    root.truncate(hasher.id_len());
    root
}

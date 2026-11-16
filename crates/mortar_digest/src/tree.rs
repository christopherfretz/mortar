//! The build-id tree (`spec/image.md` I-72, I-73): one leaf digest per
//! `IMAGE_BLOCK` bytes of the output, computed by any worker in any order,
//! and a root over the parameters and the leaves in index order.

use std::fmt;
use std::ops::Range;

use crate::l128::{L128, L128_LEN};
use crate::sha1::{SHA1_LEN, Sha1};
use crate::sha256::{SHA256_LEN, Sha256};

/// `B`: bytes of one leaf, equal to the image's window `W` (I-75).
pub const IMAGE_BLOCK: u64 = 1 << 20;

/// The tag that starts the root's input; it names the definition's
/// version (I-72, I-75).
pub const TREE_TAG: &[u8; 18] = b"mortar-build-id-2\0";

/// Longest digest of any function here.
const MAX_LEN: usize = 32;

/// A leaf function `H` and its root function `R` (I-72's table).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TreeFn {
    /// `H = L128`, `R = SHA-256`: bare `--build-id` and `fast`.
    L128,
    /// `H = R = SHA-1`: `sha1`.
    Sha1,
    /// `H = R = SHA-256`: `sha256`.
    Sha256,
}

impl TreeFn {
    /// Bytes of one leaf digest.
    pub const fn leaf_len(self) -> usize {
        match self {
            Self::L128 => L128_LEN,
            Self::Sha1 => SHA1_LEN,
            Self::Sha256 => SHA256_LEN,
        }
    }

    /// Bytes of the root digest.
    pub const fn root_len(self) -> usize {
        match self {
            Self::L128 | Self::Sha256 => SHA256_LEN,
            Self::Sha1 => SHA1_LEN,
        }
    }
}

/// `m = ⌈file_size / B⌉`, the number of leaves.
pub const fn leaf_count(file_size: u64) -> u64 {
    file_size.div_ceil(IMAGE_BLOCK)
}

/// The file range leaf `index` covers: `[i·B, min((i+1)·B, n))`. Empty
/// when `index` is not below [`leaf_count`].
pub fn leaf_range(file_size: u64, index: u64) -> Range<u64> {
    let lo = index.saturating_mul(IMAGE_BLOCK).min(file_size);
    let hi = index.saturating_add(1).saturating_mul(IMAGE_BLOCK).min(file_size);
    lo..hi
}

/// A leaf's digest, with the function and the number of bytes it hashed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Leaf {
    f: TreeFn,
    len: u64,
    digest: [u8; MAX_LEN],
}

impl Leaf {
    /// The function that made this digest.
    pub const fn tree_fn(&self) -> TreeFn {
        self.f
    }

    /// How many bytes were hashed.
    pub const fn hashed_len(&self) -> u64 {
        self.len
    }

    /// The digest (`tree_fn().leaf_len()` bytes).
    pub fn as_bytes(&self) -> &[u8] {
        &self.digest[..self.f.leaf_len()]
    }

    /// A leaf kept from an earlier hash (`spec/incr.md` IN-18a `LEAVES`,
    /// I-76): leaf `index` of a file of `file_size` bytes, whose digest is
    /// the first `f.leaf_len()` bytes of `digest`; `None` when `digest` is
    /// shorter or `index` is past the file.
    #[must_use]
    pub fn kept(f: TreeFn, file_size: u64, index: u64, digest: &[u8]) -> Option<Self> {
        let n = f.leaf_len();
        let r = leaf_range(file_size, index);
        if digest.len() < n || r.start >= r.end {
            return None;
        }
        let mut d = [0u8; MAX_LEN];
        d[..n].copy_from_slice(&digest[..n]);
        Some(Self { f, len: r.end - r.start, digest: d })
    }
}

#[derive(Clone, Debug)]
enum State {
    L128(L128),
    Sha1(Sha1),
    Sha256(Sha256),
}

/// The digest of one leaf, fed in pieces (a worker fills its window in
/// several steps and may hash each as it is done).
#[derive(Clone, Debug)]
pub struct LeafHasher {
    state: State,
    len: u64,
}

impl LeafHasher {
    /// A leaf of no bytes so far.
    pub const fn new(f: TreeFn) -> Self {
        let state = match f {
            TreeFn::L128 => State::L128(L128::new()),
            TreeFn::Sha1 => State::Sha1(Sha1::new()),
            TreeFn::Sha256 => State::Sha256(Sha256::new()),
        };
        Self { state, len: 0 }
    }

    /// Appends `data` to the leaf.
    pub fn update(&mut self, data: &[u8]) {
        self.len += data.len() as u64;
        match &mut self.state {
            State::L128(s) => s.update(data),
            State::Sha1(s) => s.update(data),
            State::Sha256(s) => s.update(data),
        }
    }

    /// Appends `count` zero bytes (a sealed range, I-73).
    pub fn update_zeros(&mut self, mut count: u64) {
        const ZEROS: [u8; 256] = [0; 256];
        while count > 0 {
            let take = count.min(ZEROS.len() as u64) as usize;
            self.update(&ZEROS[..take]);
            count -= take as u64;
        }
    }

    /// The leaf's digest.
    pub fn finish(self) -> Leaf {
        let mut digest = [0; MAX_LEN];
        let f = match self.state {
            State::L128(s) => {
                digest[..L128_LEN].copy_from_slice(&s.finish());
                TreeFn::L128
            }
            State::Sha1(s) => {
                digest[..SHA1_LEN].copy_from_slice(&s.finish());
                TreeFn::Sha1
            }
            State::Sha256(s) => {
                digest[..SHA256_LEN].copy_from_slice(&s.finish());
                TreeFn::Sha256
            }
        };
        Leaf { f, len: self.len, digest }
    }
}

/// The digest of one leaf's bytes, in one call.
pub fn leaf(f: TreeFn, data: &[u8]) -> Leaf {
    let mut h = LeafHasher::new(f);
    h.update(data);
    h.finish()
}

/// The root digest `R(TREE_TAG ‖ le64(n) ‖ le64(B) ‖ leaf_0 ‖ …)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Root {
    f: TreeFn,
    digest: [u8; MAX_LEN],
}

impl Root {
    /// The function of the tree.
    pub const fn tree_fn(&self) -> TreeFn {
        self.f
    }

    /// The digest (`tree_fn().root_len()` bytes).
    pub fn as_bytes(&self) -> &[u8] {
        &self.digest[..self.f.root_len()]
    }
}

/// What is wrong with a tree's leaves. Every one is a bug of the caller's
/// (the image hands each window to the column once, I-73).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeError {
    /// The leaves of a file this large do not fit in memory.
    TooLarge { file_size: u64 },
    /// A leaf index not below the leaf count.
    Index { index: u64, count: u64 },
    /// A leaf made by another function than the column's.
    WrongFn { index: u64, expected: TreeFn, got: TreeFn },
    /// A leaf that hashed another number of bytes than its range has.
    WrongLength { index: u64, expected: u64, got: u64 },
    /// A leaf set twice.
    Duplicate { index: u64 },
    /// The root asked for while a leaf is missing (the lowest one).
    Missing { index: u64 },
    /// A root made by another function than the style's.
    WrongRoot { expected: TreeFn, got: TreeFn },
}

impl fmt::Display for TreeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { file_size } => {
                write!(f, "build-id tree: a file of {file_size} bytes has too many leaves")
            }
            Self::Index { index, count } => {
                write!(f, "build-id tree: leaf {index} of {count}")
            }
            Self::WrongFn { index, expected, got } => {
                write!(f, "build-id tree: leaf {index} is a {got:?} digest, expected {expected:?}")
            }
            Self::WrongLength { index, expected, got } => {
                write!(f, "build-id tree: leaf {index} hashed {got} bytes, expected {expected}")
            }
            Self::Duplicate { index } => write!(f, "build-id tree: leaf {index} set twice"),
            Self::Missing { index } => write!(f, "build-id tree: leaf {index} is missing"),
            Self::WrongRoot { expected, got } => {
                write!(f, "build-id tree: a {got:?} root, expected {expected:?}")
            }
        }
    }
}

impl std::error::Error for TreeError {}

/// The leaves of one file, set in any order, and the root once all are
/// set. Kept as one byte column, `leaf_len` bytes per leaf (I-73, I-76).
#[derive(Clone, Debug)]
pub struct LeafColumn {
    f: TreeFn,
    file_size: u64,
    digests: Vec<u8>,
    filled: Vec<bool>,
    unfilled: usize,
}

impl LeafColumn {
    /// An empty column for a file of `file_size` bytes.
    pub fn new(f: TreeFn, file_size: u64) -> Result<Self, TreeError> {
        let count = usize::try_from(leaf_count(file_size))
            .ok()
            .filter(|c| c.checked_mul(f.leaf_len()).is_some())
            .ok_or(TreeError::TooLarge { file_size })?;
        Ok(Self {
            f,
            file_size,
            digests: vec![0; count * f.leaf_len()],
            filled: vec![false; count],
            unfilled: count,
        })
    }

    /// The column's function.
    pub const fn tree_fn(&self) -> TreeFn {
        self.f
    }

    /// The file size `n`.
    pub const fn file_size(&self) -> u64 {
        self.file_size
    }

    /// The number of leaves, `m`.
    pub fn len(&self) -> u64 {
        self.filled.len() as u64
    }

    /// Whether the file has no leaves (`n = 0`).
    pub fn is_empty(&self) -> bool {
        self.filled.is_empty()
    }

    /// Whether every leaf is set.
    pub fn is_complete(&self) -> bool {
        self.unfilled == 0
    }

    /// Sets leaf `index`, checking its function and the number of bytes
    /// it hashed against [`leaf_range`].
    pub fn set(&mut self, index: u64, leaf: &Leaf) -> Result<(), TreeError> {
        let count = self.len();
        if index >= count {
            return Err(TreeError::Index { index, count });
        }
        if leaf.f != self.f {
            return Err(TreeError::WrongFn { index, expected: self.f, got: leaf.f });
        }
        let r = leaf_range(self.file_size, index);
        let expected = r.end - r.start;
        if leaf.len != expected {
            return Err(TreeError::WrongLength { index, expected, got: leaf.len });
        }
        let i = index as usize;
        if self.filled[i] {
            return Err(TreeError::Duplicate { index });
        }
        let w = self.f.leaf_len();
        self.digests[i * w..(i + 1) * w].copy_from_slice(leaf.as_bytes());
        self.filled[i] = true;
        self.unfilled -= 1;
        Ok(())
    }

    /// The root over every leaf (I-72).
    pub fn root(&self) -> Result<Root, TreeError> {
        if let Some(i) = self.filled.iter().position(|&set| !set) {
            return Err(TreeError::Missing { index: i as u64 });
        }
        Ok(root_of(self.f, self.file_size, IMAGE_BLOCK, &self.digests))
    }
}

/// `R(TREE_TAG ‖ le64(n) ‖ le64(b) ‖ leaves)`, `leaves` concatenated.
pub(crate) fn root_of(f: TreeFn, n: u64, b: u64, leaves: &[u8]) -> Root {
    let mut digest = [0; MAX_LEN];
    match f {
        TreeFn::L128 | TreeFn::Sha256 => {
            let mut s = Sha256::new();
            s.update(TREE_TAG);
            s.update(&n.to_le_bytes());
            s.update(&b.to_le_bytes());
            s.update(leaves);
            digest[..SHA256_LEN].copy_from_slice(&s.finish());
        }
        TreeFn::Sha1 => {
            let mut s = Sha1::new();
            s.update(TREE_TAG);
            s.update(&n.to_le_bytes());
            s.update(&b.to_le_bytes());
            s.update(leaves);
            digest[..SHA1_LEN].copy_from_slice(&s.finish());
        }
    }
    Root { f, digest }
}

/// The tree's root over a whole file in memory, on one thread, with the
/// bytes in `zeroed` read as zeros (the descriptor, I-72). The reference
/// definition the parallel column is checked against, and what a verifier
/// recomputes. `zeroed` is clipped to the file.
pub fn tree_root(f: TreeFn, file: &[u8], zeroed: Range<usize>) -> Root {
    let size = file.len();
    let z_lo = zeroed.start.min(size);
    let z_hi = zeroed.end.clamp(z_lo, size);
    let mut leaves = Vec::with_capacity(leaf_count(size as u64) as usize * f.leaf_len());
    for lo in (0..size).step_by(IMAGE_BLOCK as usize) {
        let hi = (lo + IMAGE_BLOCK as usize).min(size);
        let mut h = LeafHasher::new(f);
        // The parts of [lo, hi) before, inside and after the zeroed range.
        let (zs, ze) = (z_lo.clamp(lo, hi), z_hi.clamp(lo, hi));
        h.update(&file[lo..zs]);
        h.update_zeros((ze - zs) as u64);
        h.update(&file[ze..hi]);
        leaves.extend_from_slice(h.finish().as_bytes());
    }
    root_of(f, size as u64, IMAGE_BLOCK, &leaves)
}

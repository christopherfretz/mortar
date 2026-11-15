//! The styles of `--build-id` that this crate computes (`spec/image.md`
//! I-69, I-70). Parsing the option, and refusing `md5` (and `uuid` in M1),
//! is the command line's; `none` writes no note and needs nothing here.

use std::fmt;

use crate::tree::{Root, TreeError, TreeFn};

/// A style whose id is the root of a tree (I-72), truncated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TreeStyle {
    /// Bare `--build-id`: the `L128` tree, 20 bytes.
    Default,
    /// `--build-id=fast`: the bare id's first 8 bytes.
    Fast,
    /// `--build-id=sha1`: the SHA-1 tree, 20 bytes.
    Sha1,
    /// `--build-id=sha256`: the SHA-256 tree, 32 bytes.
    Sha256,
}

impl TreeStyle {
    /// The tree's functions.
    pub const fn tree_fn(self) -> TreeFn {
        match self {
            Self::Default | Self::Fast => TreeFn::L128,
            Self::Sha1 => TreeFn::Sha1,
            Self::Sha256 => TreeFn::Sha256,
        }
    }

    /// `L`, the id's length in bytes.
    pub const fn id_len(self) -> usize {
        match self {
            Self::Default | Self::Sha1 => 20,
            Self::Fast => 8,
            Self::Sha256 => 32,
        }
    }

    /// `id = root[0 .. L]`.
    pub fn id(self, root: &Root) -> Result<&[u8], TreeError> {
        if root.tree_fn() != self.tree_fn() {
            return Err(TreeError::WrongRoot { expected: self.tree_fn(), got: root.tree_fn() });
        }
        Ok(&root.as_bytes()[..self.id_len()])
    }
}

/// `--build-id=uuid` (M2): 16 random bytes with RFC 4122's version 4 and
/// variant bits set, as mold does (I-69). The caller reads the random
/// bytes (`/dev/urandom`).
pub const fn uuid_v4(random: [u8; 16]) -> [u8; 16] {
    let mut id = random;
    id[6] = (id[6] & 0x0F) | 0x40;
    id[8] = (id[8] & 0x3F) | 0x80;
    id
}

/// `HEX` of `--build-id=0xHEX` (I-70) is not one or more pairs of
/// hexadecimal digits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HexError;

impl fmt::Display for HexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("expected an even number of hexadecimal digits")
    }
}

impl std::error::Error for HexError {}

/// The bytes of `HEX` (the digits after `0x`), either case (I-70).
pub fn parse_hex(digits: &str) -> Result<Vec<u8>, HexError> {
    let d = digits.as_bytes();
    if d.is_empty() || !d.len().is_multiple_of(2) {
        return Err(HexError);
    }
    let nibble = |c: u8| match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        b'A'..=b'F' => Ok(c - b'A' + 10),
        _ => Err(HexError),
    };
    d.chunks_exact(2).map(|p| Ok(nibble(p[0])? << 4 | nibble(p[1])?)).collect()
}

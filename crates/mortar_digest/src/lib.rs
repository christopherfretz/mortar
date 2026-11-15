//! Digests (`spec/image.md` §8): SHA-1 and SHA-256 (FIPS 180-4, written
//! from the standard), the 128-bit leaf hash `L128` (I-114), and the
//! build-id tree over fixed-size leaves (I-72, I-73). No dependencies.
//!
//! The image's workers hash each window with a [`LeafHasher`] (fed in as
//! many pieces as the fill makes) and hand the [`Leaf`] to the one
//! [`LeafColumn`] of the link, in any order; once every leaf is set the
//! column gives the [`Root`], and the [`TreeStyle`] truncates it to the id.
//! [`tree_root`] is the same function on one thread over a file in memory.
//!
//! Every function here is pure and portable: no hardware instructions
//! (which would need `unsafe` intrinsics), no dependence on the thread
//! count or the order leaves complete in.
#![forbid(unsafe_code)]

mod block;
mod l128;
mod sha1;
mod sha256;
mod style;
mod tree;

pub use l128::{L128, L128_LEN, l128};
pub use sha1::{SHA1_LEN, Sha1, sha1};
pub use sha256::{SHA256_LEN, Sha256, sha256};
pub use style::{HexError, TreeStyle, parse_hex, uuid_v4};
pub use tree::{
    IMAGE_BLOCK, Leaf, LeafColumn, LeafHasher, Root, TREE_TAG, TreeError, TreeFn, leaf, leaf_count,
    leaf_range, tree_root,
};

#[cfg(test)]
mod tests;

//! Fed in pieces == in one call, for every split point of small inputs
//! and random multi-piece splits of larger ones (`spec/image.md` I-33:
//! a worker hashes a window in several steps).

mod common;

use common::Rng;
use mortar_digest::{L128, LeafHasher, Sha1, Sha256, TreeFn, l128, leaf, sha1, sha256};

fn all_three(parts: &[&[u8]]) -> ([u8; 20], [u8; 32], [u8; 16]) {
    let (mut a, mut b, mut c) = (Sha1::new(), Sha256::new(), L128::new());
    for p in parts {
        a.update(p);
        b.update(p);
        c.update(p);
    }
    (a.finish(), b.finish(), c.finish())
}

fn one_shot(d: &[u8]) -> ([u8; 20], [u8; 32], [u8; 16]) {
    (sha1(d), sha256(d), l128(d))
}

#[test]
fn every_split_point_of_two_pieces() {
    let mut rng = Rng(7);
    for n in 0..=200 {
        let d = rng.bytes(n);
        let want = one_shot(&d);
        for k in 0..=n {
            assert_eq!(all_three(&[&d[..k], &d[k..]]), want, "n = {n}, split at {k}");
        }
    }
}

#[test]
fn every_pair_of_split_points() {
    let mut rng = Rng(8);
    for n in [0, 1, 31, 32, 33, 55, 56, 63, 64, 65, 96, 119, 120, 128, 130] {
        let d = rng.bytes(n);
        let want = one_shot(&d);
        for i in 0..=n {
            for j in i..=n {
                let got = all_three(&[&d[..i], &[], &d[i..j], &d[j..]]);
                assert_eq!(got, want, "n = {n}, splits {i}, {j}");
            }
        }
    }
}

#[test]
fn byte_at_a_time_and_random_pieces() {
    let mut rng = Rng(9);
    for n in [0, 1, 64, 1000, 4099, 70_001] {
        let d = rng.bytes(n);
        let want = one_shot(&d);
        let singles: Vec<&[u8]> = d.chunks(1).collect();
        assert_eq!(all_three(&singles), want, "n = {n}, bytes one at a time");
        let mut parts = Vec::new();
        let mut at = 0;
        while at < n {
            let len = (rng.below(300) as usize).min(n - at);
            parts.push(&d[at..at + len]);
            at += len;
        }
        assert_eq!(all_three(&parts), want, "n = {n}, random pieces");
    }
}

#[test]
fn leaf_hasher_in_pieces_and_with_zeros() {
    let mut rng = Rng(10);
    let mut d = rng.bytes(5000);
    for f in [TreeFn::L128, TreeFn::Sha1, TreeFn::Sha256] {
        let mut h = LeafHasher::new(f);
        h.update(&d[..1000]);
        h.update_zeros(777);
        h.update(&d[1777..]);
        let got = h.finish();
        let mut z = d.clone();
        z[1000..1777].fill(0);
        assert_eq!(got, leaf(f, &z), "{f:?}");
        assert_eq!(got.hashed_len(), 5000);
        assert_eq!(got.as_bytes().len(), f.leaf_len());
    }
    d.clear();
    assert_eq!(leaf(TreeFn::L128, &d).as_bytes(), l128(b""));
    assert_eq!(leaf(TreeFn::Sha1, &d).as_bytes(), sha1(b""));
    assert_eq!(leaf(TreeFn::Sha256, &d).as_bytes(), sha256(b""));
}

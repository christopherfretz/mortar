//! The build-id tree (`spec/image.md` I-72, I-73): any order of leaf
//! completion and any piece-wise feeding give the same id; the id depends
//! on every byte outside the descriptor, on the length and on the style.

mod common;

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::Rng;
use mortar_digest::{
    IMAGE_BLOCK, Leaf, LeafColumn, LeafHasher, TreeError, TreeFn, TreeStyle, leaf, leaf_count,
    leaf_range, tree_root,
};

const FNS: [TreeFn; 3] = [TreeFn::L128, TreeFn::Sha1, TreeFn::Sha256];
const B: usize = IMAGE_BLOCK as usize;

/// Leaf `i` of `file`, fed in random pieces.
fn leaf_in_pieces(f: TreeFn, file: &[u8], i: u64, rng: &mut Rng) -> Leaf {
    let r = leaf_range(file.len() as u64, i);
    let bytes = &file[r.start as usize..r.end as usize];
    let mut h = LeafHasher::new(f);
    let mut at = 0;
    while at < bytes.len() {
        let len = (1 + rng.below(200_000) as usize).min(bytes.len() - at);
        h.update(&bytes[at..at + len]);
        at += len;
    }
    h.finish()
}

#[test]
fn leaf_ranges() {
    assert_eq!(leaf_count(0), 0);
    assert_eq!(leaf_count(1), 1);
    assert_eq!(leaf_count(IMAGE_BLOCK), 1);
    assert_eq!(leaf_count(IMAGE_BLOCK + 1), 2);
    let n = 3 * IMAGE_BLOCK + 5;
    assert_eq!(leaf_count(n), 4);
    assert_eq!(leaf_range(n, 0), 0..IMAGE_BLOCK);
    assert_eq!(leaf_range(n, 3), 3 * IMAGE_BLOCK..n);
    assert!(leaf_range(n, 4).is_empty());
    assert!(leaf_range(n, u64::MAX).is_empty());
}

/// Leaves set in many orders, each fed in random pieces, on one thread
/// and on several, give the reference root.
#[test]
fn any_completion_order_gives_the_same_id() {
    let mut rng = Rng(0xA11_0DE5);
    let file = rng.bytes(5 * B + 123);
    let n = file.len() as u64;
    for f in FNS {
        let want = tree_root(f, &file, 0..0);
        // L128 is cheap: many orders. SHA is slow in a debug build: few.
        let trials = if f == TreeFn::L128 { 12 } else { 2 };
        for t in 0..trials {
            let mut order: Vec<u64> = (0..leaf_count(n)).collect();
            if t == 1 {
                order.reverse();
            } else if t > 1 {
                rng.shuffle(&mut order);
            }
            let mut col = LeafColumn::new(f, n).unwrap();
            for &i in &order {
                assert!(col.root().is_err(), "root before the last leaf");
                col.set(i, &leaf_in_pieces(f, &file, i, &mut rng)).unwrap();
            }
            assert!(col.is_complete());
            assert_eq!(col.root().unwrap(), want, "{f:?}, order {order:?}");
        }
    }
}

#[test]
fn workers_on_threads_give_the_same_id() {
    let mut rng = Rng(0x7EAD);
    let file = rng.bytes(9 * B + 1);
    let n = file.len() as u64;
    let want = tree_root(TreeFn::L128, &file, 0..0);
    for threads in [1, 2, 3, 8] {
        let col = Mutex::new(LeafColumn::new(TreeFn::L128, n).unwrap());
        let next = AtomicUsize::new(0);
        std::thread::scope(|s| {
            for w in 0..threads {
                let (col, next, file) = (&col, &next, &file);
                s.spawn(move || {
                    let mut rng = Rng(w as u64);
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed) as u64;
                        if i >= leaf_count(n) {
                            break;
                        }
                        let l = leaf_in_pieces(TreeFn::L128, file, i, &mut rng);
                        col.lock().unwrap().set(i, &l).unwrap();
                    }
                });
            }
        });
        assert_eq!(col.into_inner().unwrap().root().unwrap(), want, "{threads} threads");
    }
}

/// Flipping any byte outside the zeroed range changes the id; inside it,
/// nothing does.
#[test]
fn the_id_depends_on_every_byte() {
    let mut rng = Rng(0xF11B);
    let mut file = rng.bytes(300);
    let zeroed = 100..120;
    for f in FNS {
        let base = tree_root(f, &file, zeroed.clone());
        for i in 0..file.len() {
            for bit in [0x01, 0x80] {
                file[i] ^= bit;
                let got = tree_root(f, &file, zeroed.clone());
                file[i] ^= bit;
                if zeroed.contains(&i) {
                    assert_eq!(got, base, "{f:?}: byte {i} is in the descriptor");
                } else {
                    assert_ne!(got, base, "{f:?}: flipping byte {i} bit {bit:#x}");
                }
            }
        }
    }
}

/// Bytes on both sides of a leaf boundary (L128 only: SHA is slow in a
/// debug build, and its tree differs only in the functions).
#[test]
fn the_id_depends_on_bytes_at_leaf_boundaries() {
    let mut file = vec![0u8; 2 * B + 3];
    let base = tree_root(TreeFn::L128, &file, 0..0);
    let mut seen = vec![base];
    for i in [0, 1, B - 1, B, B + 1, 2 * B - 1, 2 * B, 2 * B + 2] {
        file[i] = 1;
        let id = tree_root(TreeFn::L128, &file, 0..0);
        file[i] = 0;
        assert!(!seen.contains(&id), "byte {i}");
        seen.push(id);
    }
}

/// A trailing zero byte, a whole zero leaf more, or a byte less changes
/// the id (the length is in the root and in `L128`).
#[test]
fn the_id_depends_on_the_length() {
    for f in FNS {
        let mut seen = Vec::new();
        for n in [0, 1, 2, 31, 32, 33, 64, B - 1, B, B + 1, 2 * B] {
            let id = tree_root(f, &vec![0u8; n], 0..0);
            assert!(!seen.contains(&id), "{f:?}: zeros of length {n}");
            seen.push(id);
        }
    }
}

/// The styles give different ids for the same file (except `fast`, the
/// bare id's prefix).
#[test]
fn the_id_depends_on_the_style() {
    let file = vec![5u8; 1000];
    let ids: Vec<Vec<u8>> = [TreeStyle::Default, TreeStyle::Sha1, TreeStyle::Sha256]
        .into_iter()
        .map(|s| s.id(&tree_root(s.tree_fn(), &file, 0..0)).unwrap().to_vec())
        .collect();
    assert_ne!(ids[0], ids[1]);
    assert_ne!(ids[0][..], ids[2][..20]);
    assert_ne!(ids[1][..], ids[2][..20]);
}

#[test]
fn a_column_refuses_wrong_leaves() {
    let n = 2 * IMAGE_BLOCK + 10;
    let file = vec![3u8; n as usize];
    let mut col = LeafColumn::new(TreeFn::L128, n).unwrap();
    assert_eq!(col.len(), 3);
    let l0 = leaf(TreeFn::L128, &file[..B]);
    let l2 = leaf(TreeFn::L128, &file[2 * B..]);
    assert_eq!(col.set(3, &l0), Err(TreeError::Index { index: 3, count: 3 }));
    assert_eq!(
        col.set(0, &leaf(TreeFn::Sha1, &file[..B])),
        Err(TreeError::WrongFn { index: 0, expected: TreeFn::L128, got: TreeFn::Sha1 })
    );
    assert_eq!(
        col.set(0, &l2),
        Err(TreeError::WrongLength { index: 0, expected: IMAGE_BLOCK, got: 10 })
    );
    assert_eq!(
        col.set(2, &l0),
        Err(TreeError::WrongLength { index: 2, expected: 10, got: IMAGE_BLOCK })
    );
    col.set(2, &l2).unwrap();
    assert_eq!(col.set(2, &l2), Err(TreeError::Duplicate { index: 2 }));
    assert_eq!(col.root(), Err(TreeError::Missing { index: 0 }));
    col.set(0, &l0).unwrap();
    assert_eq!(col.root(), Err(TreeError::Missing { index: 1 }));
    col.set(1, &l0).unwrap();
    assert_eq!(col.root().unwrap(), tree_root(TreeFn::L128, &file, 0..0));
    assert!(!col.is_empty());

    let empty = LeafColumn::new(TreeFn::Sha1, 0).unwrap();
    assert!(empty.is_empty() && empty.is_complete());
    assert_eq!(empty.root().unwrap(), tree_root(TreeFn::Sha1, &[], 0..0));
}

/// A zeroed range past the end, or reversed, is clipped, not a panic.
#[test]
fn zeroed_ranges_are_clipped() {
    let file = vec![9u8; 100];
    let plain = tree_root(TreeFn::L128, &file, 0..0);
    assert_eq!(tree_root(TreeFn::L128, &file, 100..200), plain);
    assert_eq!(tree_root(TreeFn::L128, &file, 500..600), plain);
    #[allow(clippy::reversed_empty_ranges)]
    let reversed = tree_root(TreeFn::L128, &file, 50..40);
    assert_eq!(reversed, plain);
    let mut z = file.clone();
    z[90..].fill(0);
    assert_eq!(tree_root(TreeFn::L128, &file, 90..1000), tree_root(TreeFn::L128, &z, 0..0));
}

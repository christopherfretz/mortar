//! Tests of what is not public: the constants and the root's parameters.

use crate::sha256::{INITIAL, K};
use crate::tree::{IMAGE_BLOCK, TreeFn, root_of};

fn primes(count: usize) -> Vec<u32> {
    let mut out = Vec::new();
    let mut p = 2u32;
    while out.len() < count {
        if (2..p).take_while(|d| d * d <= p).all(|d| !p.is_multiple_of(d)) {
            out.push(p);
        }
        p += 1;
    }
    out
}

fn frac32(x: f64) -> u32 {
    ((x - x.floor()) * 4_294_967_296.0) as u32
}

/// FIPS 180-4 §4.2.2 and §5.3.3: the constants are derived from the
/// primes, so a mistyped one is found here and not only by a vector.
#[test]
fn sha256_constants_are_the_primes_roots() {
    let p = primes(64);
    for (i, &k) in K.iter().enumerate() {
        assert_eq!(k, frac32(f64::from(p[i]).cbrt()), "K[{i}]");
    }
    for (i, &h) in INITIAL.iter().enumerate() {
        assert_eq!(h, frac32(f64::from(p[i]).sqrt()), "H0[{i}]");
    }
}

/// I-72: the root depends on `n` and `B`, not only on the leaves.
#[test]
fn root_depends_on_the_parameters() {
    let leaves = [7u8; 32];
    for f in [TreeFn::L128, TreeFn::Sha1, TreeFn::Sha256] {
        let base = root_of(f, 5, IMAGE_BLOCK, &leaves);
        assert_ne!(base, root_of(f, 6, IMAGE_BLOCK, &leaves), "{f:?} n");
        assert_ne!(base, root_of(f, 5, IMAGE_BLOCK / 2, &leaves), "{f:?} B");
        assert_ne!(base, root_of(f, 5, IMAGE_BLOCK, &leaves[..31]), "{f:?} leaves");
    }
}

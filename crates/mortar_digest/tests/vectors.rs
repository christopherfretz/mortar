//! Fixed vectors: FIPS 180-4 / RFC 3174 for SHA-1 and SHA-256, and
//! `spec/image.md` §8.4's for `L128` and the trees (I-72, I-114).

mod common;

use common::hex;
use mortar_digest::{
    IMAGE_BLOCK, Sha1, Sha256, TreeFn, TreeStyle, l128, parse_hex, sha1, sha256, tree_root, uuid_v4,
};

/// The vectors' pattern (§8.4): byte `i` is `i mod 251`.
fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % 251) as u8).collect()
}

const B: usize = IMAGE_BLOCK as usize;
const ABC448: &[u8] = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
const ABC896: &[u8] = b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu";

#[test]
fn sha1_standard_vectors() {
    assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    assert_eq!(hex(&sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
    assert_eq!(hex(&sha1(ABC448)), "84983e441c3bd26ebaae4aa1f95129e5e54670f1");
    assert_eq!(hex(&sha1(ABC896)), "a49b2446a02c645bf419f995b67091253a04a259");
    // RFC 3174 §7.3, TEST4: "01234567" × 8, ten times.
    assert_eq!(hex(&sha1(&b"01234567".repeat(80))), "dea356a2cddd90c7a7ecedc5ebb563934f460452");
}

#[test]
fn sha256_standard_vectors() {
    assert_eq!(
        hex(&sha256(b"")),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        hex(&sha256(b"abc")),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        hex(&sha256(ABC448)),
        "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
    );
    assert_eq!(
        hex(&sha256(ABC896)),
        "cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1"
    );
}

/// The million-`a` vectors, fed in uneven pieces.
#[test]
fn million_a() {
    let a = vec![b'a'; 1_000_000];
    assert_eq!(hex(&sha1(&a)), "34aa973cd4c4daa4f61eeb2bdbad27316534016f");
    assert_eq!(
        hex(&sha256(&a)),
        "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
    );
    let (mut s1, mut s2) = (Sha1::new(), Sha256::new());
    for piece in a.chunks(997) {
        s1.update(piece);
        s2.update(piece);
    }
    assert_eq!(s1.finish(), sha1(&a));
    assert_eq!(s2.finish(), sha256(&a));
}

/// `spec/image.md` §8.4, `L128` (I-114).
#[test]
fn l128_vectors() {
    assert_eq!(hex(&l128(b"")), "166e5d750a9987ba700b8ae8169c08f1");
    assert_eq!(hex(&l128(b"abc")), "34d9933929a69d420194f31701b62a6e");
    let table: [(usize, &str); 13] = [
        (1, "a9b2c13446ed2ae78d9276aa7e6a453c"),
        (7, "d8ba3acb3828bf91099bcd2dc39f4672"),
        (8, "8b8a2b39f5629b5caaa295deca45fa41"),
        (31, "30de9b5bf64bf64bc7885eaab7d8ddd5"),
        (32, "cf8b6fc0c63e046e161c68c0466a00e5"),
        (33, "70094889eb36eb1e8e4f9a079d66511b"),
        (63, "5bf246f31e37031982482cf3c6f97f75"),
        (64, "53d570ba718049ac4882965680303b32"),
        (65, "b15eb71a90c7abc48e43144b543f840e"),
        (4096, "f570b658e524275c4140b619d155cab9"),
        (B - 1, "cb9e12add796f4ed147b7108555b66f7"),
        (B, "c6e1dfc9cc045a72cf3a72032a298d32"),
        (B + 1, "1234f10fe0f80d6c59c2724939331490"),
    ];
    for (n, want) in table {
        assert_eq!(hex(&l128(&pattern(n))), want, "L128 of pattern({n})");
    }
}

/// `spec/image.md` §8.4, the trees' roots (I-72) for `n` = 0, 1, `B − 1`,
/// `B`, `B + 1`, `3B + 5`, and `3B + 5` with `[B − 6, B + 14)` zeroed (a
/// descriptor across a leaf boundary).
#[test]
fn tree_vectors() {
    let sizes = [0, 1, B - 1, B, B + 1, 3 * B + 5];
    let table: [(TreeFn, [&str; 7]); 3] = [
        (
            TreeFn::L128,
            [
                "598ec1d4dd3c72cee90920b43faf73a539ad84fd3eb148f8b1c5f4362be50bcb",
                "2df0b93caf760623c40f3781b8e01397e19f7230094ebf68d67a4fa922ca5082",
                "8cea389497e938a07c9945e7e8e7e43f0c6098838f993d330e0e963b2fced212",
                "059fe2c890954621d038b090876901341dcdaf92703e0a0bd27fef29ad98147f",
                "be75a9986f7fe460609c23a5432a777d0c5c9106721d52b09522cfd3a5162b09",
                "4570d21375cf658f56f2d1c4f6c81b52e44c63ddc657cb18604b04470513b883",
                "31ff17b9820dd9f21410ccd27c9bcd3993f325613f8c070cad73c613c8faed25",
            ],
        ),
        (
            TreeFn::Sha1,
            [
                "e17559e60537aa8405a216c63d1680f54fe53601",
                "f78bec69d0b07408559e32b98a419a7b111caed9",
                "add0d032914772ae22c9296d55dfd93ae2297cb2",
                "0e2d57f84202376f665ed443e840c6e47b5c276e",
                "9c9df507ac24501067432862ecf325e57b4aa0a4",
                "0b5813d4604084d007aefb48b83fa4530f15931e",
                "756ee545d2b48c55d0a776066dcaf96c8c02a8c6",
            ],
        ),
        (
            TreeFn::Sha256,
            [
                "598ec1d4dd3c72cee90920b43faf73a539ad84fd3eb148f8b1c5f4362be50bcb",
                "5b94dbbfe23e7efbfd1ccc5a410d926b54a06d828aa57ba9efa5cc4de7e0ed62",
                "c6b45677bcf57dffff98e8adebf7554fbd34e1ea80259f0a1436cb3e289839d0",
                "99a6c7fa4d6cfed58f895c66aab64909f814a11ccc9406bfbe21ba0b5a3352e8",
                "2b2777fb577eeeed88d735f2577239e248be71cbf97b4170d1ccf34a91aad675",
                "a5d24a056a18d9f63ec9f63f3aa6589b8abcd847a7167b101d12069d30d28e9d",
                "80b19e7d7a46828ca58fe5e4f7368db79c755c9e90207f9d34b752db211a860e",
            ],
        ),
    ];
    let big = pattern(3 * B + 5);
    for (f, want) in table {
        for (i, &n) in sizes.iter().enumerate() {
            assert_eq!(hex(tree_root(f, &big[..n], 0..0).as_bytes()), want[i], "{f:?} n = {n}");
        }
        let zeroed = tree_root(f, &big, B - 6..B + 14);
        assert_eq!(hex(zeroed.as_bytes()), want[6], "{f:?} zeroed");
        let mut z = big.clone();
        z[B - 6..B + 14].fill(0);
        assert_eq!(tree_root(f, &z, 0..0), zeroed, "{f:?}: zeroed range = zeros written");
    }
}

/// I-69 and I-72's table: each style's length and its prefix of the root.
#[test]
fn styles() {
    let file = pattern(B + 1);
    let fast_root = tree_root(TreeFn::L128, &file, 0..0);
    let bare = TreeStyle::Default.id(&fast_root).unwrap();
    let fast = TreeStyle::Fast.id(&fast_root).unwrap();
    assert_eq!(bare.len(), 20);
    assert_eq!(fast, &bare[..8], "fast is the bare id's first 8 bytes");
    assert_eq!(bare, &fast_root.as_bytes()[..20]);
    let s1 = tree_root(TreeFn::Sha1, &file, 0..0);
    assert_eq!(TreeStyle::Sha1.id(&s1).unwrap(), s1.as_bytes());
    assert_eq!(TreeStyle::Sha1.id(&s1).unwrap().len(), 20);
    let s2 = tree_root(TreeFn::Sha256, &file, 0..0);
    assert_eq!(TreeStyle::Sha256.id(&s2).unwrap().len(), 32);
    assert!(TreeStyle::Sha1.id(&s2).is_err(), "a root of another function is refused");
    assert!(TreeStyle::Default.id(&s1).is_err());
    for s in [TreeStyle::Default, TreeStyle::Fast, TreeStyle::Sha1, TreeStyle::Sha256] {
        assert_eq!(s.id_len() % 4, 0, "I-71: every hash style's L is a multiple of 4");
    }
}

/// I-69 (M2): version 4, variant 10x, the other 122 bits as given.
#[test]
fn uuid_bits() {
    assert_eq!(hex(&uuid_v4([0xFF; 16])), "ffffffffffff4fffbfffffffffffffff");
    assert_eq!(hex(&uuid_v4([0; 16])), "00000000000040008000000000000000");
    let r: [u8; 16] = core::array::from_fn(|i| (i as u8).wrapping_mul(37).wrapping_add(11));
    let u = uuid_v4(r);
    assert_eq!(u[6] >> 4, 4);
    assert_eq!(u[8] >> 6, 0b10);
    for i in (0..16).filter(|&i| i != 6 && i != 8) {
        assert_eq!(u[i], r[i]);
    }
    assert_eq!(u[6] & 0x0F, r[6] & 0x0F);
    assert_eq!(u[8] & 0x3F, r[8] & 0x3F);
}

/// I-70's table: `HEX` after `0x`.
#[test]
fn hex_values() {
    assert_eq!(parse_hex("ABCD").unwrap(), [0xAB, 0xCD]);
    assert_eq!(parse_hex("abcd").unwrap(), [0xAB, 0xCD]);
    assert_eq!(parse_hex("00").unwrap(), [0]);
    assert_eq!(parse_hex("0123456789abcdefABCDEF").unwrap().len(), 11);
    for bad in ["abc", "0", "", "zz", "0g", " 00", "00 ", "+0", "é0"] {
        let e = parse_hex(bad).unwrap_err();
        assert_eq!(e.to_string(), "expected an even number of hexadecimal digits", "{bad:?}");
    }
}

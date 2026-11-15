//! `spec/script.md` §4 (L1): no input bytes make the parser panic or
//! recurse without bound. Truncations of real scripts at every length, a
//! deterministic mutation sweep, and constructed extremes.

use mortar_script::lex::{Lexer, Mode, TokenKind};
use mortar_script::{looks_like_script, parse, position};

/// Seeds: the shapes of the system's text libraries and of GNU ld's
/// default scripts, inline so the test is the same on every machine.
const SEEDS: &[&[u8]] = &[
    b"/* GNU ld script\n   Use the shared library, but some functions are only in\n   the static library, so try that secondarily.  */\nOUTPUT_FORMAT(elf64-littleaarch64)\nGROUP ( /lib/aarch64-linux-gnu/libc.so.6 /usr/lib/aarch64-linux-gnu/libc_nonshared.a  AS_NEEDED ( /lib/ld-linux-aarch64.so.1 ) )\n",
    b"/* GNU ld script\n*/\nOUTPUT_FORMAT(elf64-x86-64)\nGROUP ( /usr/x86_64-linux-gnu/lib/libm-2.41.a /usr/x86_64-linux-gnu/lib/libmvec.a )\n",
    b"/* GNU ld script */\nGROUP ( libgcc_s.so.1 -lgcc -l:libx.a =/usr/lib/a.so \"$SYSROOT/q\" )\n",
    b"OUTPUT_FORMAT(\"elf64-littleaarch64\", \"elf64-bigaarch64\", \"elf64-littleaarch64\")\nOUTPUT_ARCH(aarch64)\nENTRY(_start)\nSEARCH_DIR(\"=/usr/local/lib/aarch64-linux-gnu\"); SEARCH_DIR(\"=/lib\");\nEXTERN(a b, c) INCLUDE x.ld; TARGET(elf32-i386) STARTUP(crt0.o) OUTPUT(a.out)\nSECTIONS\n{\n  . = SEGMENT_START(\"text-segment\", 0x400000) + SIZEOF_HEADERS;\n}\n",
    b"x = 1; # c\nPROVIDE(y = 0x10K);\n",
];

/// The system's text libraries too, when present.
fn corpus() -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = SEEDS.iter().map(|s| s.to_vec()).collect();
    for path in [
        "/usr/lib/aarch64-linux-gnu/libc.so",
        "/usr/x86_64-linux-gnu/lib/libc.so",
        "/usr/i686-linux-gnu/lib/libc.so",
        "/usr/lib/gcc/aarch64-linux-gnu/14/libgcc_s.so",
        "/usr/lib/aarch64-linux-gnu/libm.so",
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            out.push(bytes);
        }
    }
    out
}

/// Everything a caller does with untrusted bytes: parse, sniff, lex every
/// mode to the end, place every error.
fn exercise(bytes: &[u8]) {
    let result = parse(bytes, "t.ld");
    if let Err(d) = &result {
        assert!(d.place().is_some(), "an error without a place: {d:?}");
    }
    let _ = looks_like_script(bytes);
    for mode in [Mode::Script, Mode::InputList, Mode::Expression] {
        let mut lexer = Lexer::new(bytes);
        let mut last = 0;
        for _ in 0..=bytes.len() + 1 {
            match lexer.next(mode) {
                Ok(t) if t.kind == TokenKind::Eof => break,
                Ok(t) => {
                    assert!(t.span.start >= last && t.span.end > t.span.start, "no progress");
                    last = t.span.end;
                }
                Err(e) => {
                    assert!(e.offset <= bytes.len());
                    let _ = position(bytes, e.offset);
                    break;
                }
            }
        }
    }
}

// spec: script.md §4 — every prefix of every corpus script.
#[test]
fn truncations_at_every_length() {
    for bytes in corpus() {
        for len in 0..=bytes.len() {
            exercise(&bytes[..len]);
        }
    }
}

/// xorshift64*: a tiny deterministic generator, so a failure reproduces.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Fragments a mutation inserts: the bytes that switch modes and open or
/// close constructs.
const FRAGMENTS: &[&[u8]] = &[
    b"(",
    b")",
    b"\"",
    b"/*",
    b"*/",
    b"#",
    b"\n",
    b",",
    b";",
    b"=",
    b"-l",
    b"$",
    b"0x",
    b"AS_NEEDED(",
    b"INPUT(",
    b"GROUP(",
    b"EXTERN(",
    b"OUTPUT_FORMAT(",
    b"ENTRY(",
    b"INCLUDE ",
    b"SECTIONS",
    b"\0",
    b"\xff",
    b"\xc3\xa9",
    b"\x0c",
    b"999999999999999999999",
    b"ffh",
    b"+=",
    b"<<=",
];

// spec: script.md §4 — a deterministic sweep of byte flips, deletions,
// insertions and splices over the corpus.
#[test]
fn mutation_sweep() {
    let corpus = corpus();
    let mut rng = Rng(0x6d6f_7274_6172_0009);
    for round in 0..20_000 {
        let mut bytes = corpus[round % corpus.len()].clone();
        for _ in 0..=rng.below(4) {
            let at = rng.below(bytes.len() + 1);
            match rng.below(5) {
                0 if !bytes.is_empty() => {
                    let i = at.min(bytes.len() - 1);
                    bytes[i] = rng.next() as u8;
                }
                1 if at < bytes.len() => {
                    let end = (at + rng.below(16)).min(bytes.len());
                    bytes.drain(at..end);
                }
                2 => {
                    let fragment = FRAGMENTS[rng.below(FRAGMENTS.len())];
                    bytes.splice(at..at, fragment.iter().copied());
                }
                3 => {
                    let other = &corpus[rng.below(corpus.len())];
                    let from = rng.below(other.len());
                    let to = (from + rng.below(64)).min(other.len());
                    bytes.splice(at..at, other[from..to].iter().copied());
                }
                _ => bytes.truncate(at),
            }
        }
        exercise(&bytes);
    }
}

// spec: script.md §4, §2.3 — absurd nesting is an error, not a stack
// overflow; long and degenerate inputs end.
#[test]
fn extremes() {
    let deep = format!("INPUT({}", "AS_NEEDED(".repeat(200_000));
    assert!(parse(deep.as_bytes(), "t.ld").is_err());
    let parens = "(".repeat(200_000);
    assert!(parse(parens.as_bytes(), "t.ld").is_err());
    let semis = ";".repeat(1 << 20);
    assert!(parse(semis.as_bytes(), "t.ld").unwrap().commands.is_empty());
    let many = "INPUT(a.o) ".repeat(50_000);
    assert_eq!(parse(many.as_bytes(), "t.ld").unwrap().commands.len(), 50_000);
    let long_name = format!("ENTRY({})", "a".repeat(1 << 20));
    assert!(parse(long_name.as_bytes(), "t.ld").is_ok());
    let comment = format!("/*{}", "*".repeat(1 << 20));
    assert!(parse(comment.as_bytes(), "t.ld").is_err());
    let string = format!("ENTRY(\"{}", "x".repeat(1 << 20));
    assert!(parse(string.as_bytes(), "t.ld").is_err());
    for byte in 0..=255u8 {
        exercise(&[byte]);
        exercise(&[b'I', b'N', b'P', b'U', b'T', b'(', byte, b')']);
        exercise(&[b'E', b'X', b'T', b'E', b'R', b'N', b'(', b'a', byte, b')']);
    }
}

//! `spec/script.md` §9.1, §4: no bytes make the version-script or
//! dynamic-list parser panic or recurse; every error is placed inside its
//! file. Every prefix of the 213 scripts of `exp3/vs3/`, a fixed-seed
//! mutation sweep of them (no dependency), and constructed extremes.

use std::path::Path;

use mortar_diag::Place;
use mortar_script::lex::{Lexer, Mode, TokenKind};
use mortar_script::version::{SourceFile, parse_dynamic_list, parse_version_script};
use mortar_script::{DynamicList, VersionScript};

/// The Python string literals of a line (escapes `\\`, `\'`, `\"`, `\n`).
fn literals(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\'' && c != '"' {
            continue;
        }
        let mut s = String::new();
        while let Some(x) = chars.next() {
            match x {
                '\\' => match chars.next() {
                    Some('n') => s.push('\n'),
                    Some(e) => s.push(e),
                    None => break,
                },
                q if q == c => break,
                x => s.push(x),
            }
        }
        out.push(s);
    }
    out
}

/// The scripts of `exp3/vs3/` (each file of a multi-file case on its
/// own), and the dynamic-list shapes of WP-110's `comments/`.
fn seeds() -> Vec<Vec<u8>> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/shared-design/exp3/vs3");
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).expect(name);
    let mut out: Vec<Vec<u8>> = Vec::new();
    for line in read("scripts_author.txt").lines() {
        if let Some((_, s)) = line.split_once('|') {
            out.push(s.as_bytes().to_vec());
        }
    }
    for name in ["scripts_rev.py", "dups.py", "expdyn.py", "atat.py"] {
        for line in read(name).lines() {
            out.extend(
                literals(line).into_iter().filter(|s| s.contains('{')).map(String::into_bytes),
            );
        }
    }
    for i in 0..=42 {
        out.push(read(&format!("g{i:02}.map")).into_bytes());
    }
    // The 213 cases less the 20 `-E` repeats, each file of a case alone.
    assert!(out.len() >= 195, "the vs3 scripts: {}", out.len());
    out.push(b"# head\n{\n  foo; #bar;\n  /* block\n */ baz; extern \"C++\" { ns::*; \"f(int)\" }; # end\n};\n".to_vec());
    out.push(b"V1 { global: extern \"C\" { foo; }; local: *; };\nV2 { \"q\"; } V1;\n".to_vec());
    out
}

/// The byte offset of a 1-based line and byte column, if it is inside
/// `bytes` (the end of the file included).
fn offset_of(bytes: &[u8], line: u32, column: u32) -> Option<usize> {
    let mut start = 0usize;
    for _ in 1..line {
        start += bytes[start..].iter().position(|&b| b == b'\n')? + 1;
    }
    let offset = start + usize::try_from(column).ok()? - 1;
    let line_end =
        bytes[start..].iter().position(|&b| b == b'\n').map_or(bytes.len(), |e| start + e);
    (offset <= line_end).then_some(offset)
}

fn check_error(d: &mortar_diag::Diagnostic, files: &[SourceFile<'_>]) {
    assert_eq!(d.id(), "version-script-syntax", "{d:?}");
    let Some(Place::Script { path, line, column }) = d.place() else { panic!("no place: {d:?}") };
    let file = files.iter().find(|f| f.path == path).unwrap_or_else(|| panic!("{path}"));
    assert!(
        offset_of(file.bytes, *line, *column).is_some(),
        "{path}:{line}:{column} outside the file: {d:?}"
    );
}

fn check_script(s: &VersionScript, files: &[SourceFile<'_>]) {
    for (i, n) in s.nodes.iter().enumerate() {
        let len = files[n.file as usize].bytes.len();
        assert!(n.span.start <= n.span.end && n.span.end <= len);
        for p in &n.parents {
            assert!(p.node < i);
            assert!(!p.name.text.is_empty() && p.name.span.end <= len);
        }
        for e in &n.entries {
            assert!(!e.pattern.text.is_empty());
            assert!(n.span.start <= e.pattern.span.start && e.pattern.span.end <= n.span.end);
        }
    }
    let anonymous = s.nodes.iter().filter(|n| n.name.is_none()).count();
    assert!(anonymous == 0 || s.nodes.len() == 1, "an anonymous node alone");
}

fn check_list(l: &DynamicList, files: &[SourceFile<'_>]) {
    let longest = files.iter().map(|f| f.bytes.len()).max().unwrap_or(0);
    for e in &l.entries {
        assert!(!e.pattern.text.is_empty() && e.pattern.span.end <= longest);
    }
}

/// What a caller does with untrusted bytes: parse as a version script
/// (alone and after another file), as a dynamic list, and lex to the end.
fn exercise(bytes: &[u8], other: &[u8]) {
    let one = [SourceFile { path: "v.map", bytes }];
    match parse_version_script(&one) {
        Ok(s) => check_script(&s, &one),
        Err(d) => check_error(&d, &one),
    }
    let two = [SourceFile { path: "a.map", bytes: other }, SourceFile { path: "b.map", bytes }];
    match parse_version_script(&two) {
        Ok(s) => check_script(&s, &two),
        Err(d) => check_error(&d, &two),
    }
    let dynamic = [SourceFile { path: "d.list", bytes }];
    match parse_dynamic_list(&dynamic) {
        Ok(l) => check_list(&l, &dynamic),
        Err(d) => check_error(&d, &dynamic),
    }
    let mut lexer = Lexer::new(bytes);
    let mut last = 0;
    for _ in 0..=bytes.len() + 1 {
        match lexer.next(Mode::Version) {
            Ok(t) if t.kind == TokenKind::Eof => break,
            Ok(t) => {
                assert!(t.span.start >= last && t.span.end > t.span.start, "no progress");
                last = t.span.end;
            }
            Err(e) => {
                assert!(e.offset <= bytes.len());
                break;
            }
        }
    }
}

// spec: script.md §9.1, §4 (every prefix of every seed)
#[test]
fn version_truncations_at_every_length() {
    let seeds = seeds();
    for (i, bytes) in seeds.iter().enumerate() {
        let other = &seeds[(i + 1) % seeds.len()];
        for len in 0..=bytes.len() {
            exercise(&bytes[..len], other);
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
        usize::try_from(self.next() % (n.max(1) as u64)).unwrap_or(0)
    }
}

const FRAGMENTS: &[&[u8]] = &[
    b"{",
    b"}",
    b";",
    b":",
    b"::",
    b"\"",
    b"#",
    b"/*",
    b"*/",
    b"//",
    b"\\",
    b"*",
    b"[",
    b"]",
    b"\0",
    b"\n",
    b" ",
    b"\xff",
    b"\x0c",
    b"global:",
    b"local:",
    b"extern \"C++\" {",
    b"extern \"C\" {",
    b"extern \"Java\" {",
    b"extern",
    b"};",
    b"} V1;",
    b"V1 {",
    b"{ ",
    b"\"\"",
    b"V9",
    b"1V",
    b"ns::~X*",
];

// spec: script.md §9.1, §4 (20000 mutations of the seeds, fixed seed:
// byte changes, deletions, inserted fragments, splices)
#[test]
fn version_mutation_sweep() {
    let seeds = seeds();
    let mut rng = Rng(0x5eed_0125_7e75_10a5);
    for _ in 0..20_000 {
        let mut bytes = seeds[rng.below(seeds.len())].clone();
        for _ in 0..=rng.below(4) {
            let at = rng.below(bytes.len() + 1);
            match rng.below(5) {
                0 if at < bytes.len() => bytes[at] = rng.next().to_le_bytes()[0],
                1 if at < bytes.len() => {
                    let end = (at + 1 + rng.below(8)).min(bytes.len());
                    bytes.drain(at..end);
                }
                2 | 3 => {
                    let f = FRAGMENTS[rng.below(FRAGMENTS.len())];
                    bytes.splice(at..at, f.iter().copied());
                }
                _ => {
                    let other = &seeds[rng.below(seeds.len())];
                    let from = rng.below(other.len());
                    bytes.truncate(at);
                    bytes.extend_from_slice(&other[from..]);
                }
            }
        }
        let other = &seeds[rng.below(seeds.len())];
        exercise(&bytes, other);
    }
}

// spec: script.md §9.1, §4 (every byte alone, as an entry, as a node
// name; no recursion, so megabytes of openers and of names are linear)
#[test]
fn version_constructed_extremes() {
    let empty: &[u8] = b"V0 { };";
    for b in 0..=255u8 {
        exercise(&[b], empty);
        exercise(&[b"V1 { ".as_slice(), &[b], b"; };"].concat(), empty);
        exercise(&[b"{ ".as_slice(), &[b], b"; };"].concat(), empty);
        exercise(&[&[b], b" { a; };".as_slice()].concat(), empty);
    }
    let mega = 1 << 20;
    exercise(&vec![b'{'; mega], empty);
    exercise(&vec![b':'; mega], empty);
    exercise(&vec![b'a'; mega], empty);
    exercise(&b"V1 { extern \"C++\" { ".repeat(mega / 20), empty);
    exercise(&[b"V1 { ".as_slice(), &b"a; ".repeat(mega / 3), b"};"].concat(), empty);
    exercise(&[b"/*".as_slice(), &vec![b'x'; mega]].concat(), empty);
    exercise(&[b"V1 { \"".as_slice(), &vec![b'x'; mega]].concat(), empty);
    // Many nodes, each the parent of the next: linear in the nodes (the
    // names are looked up in a map).
    let mut chain = b"V0 { a; };\n".to_vec();
    for i in 1..50_000 {
        chain.extend_from_slice(format!("V{i} {{ a{i}; }} V{};\n", i - 1).as_bytes());
    }
    let s = parse_version_script(&[SourceFile { path: "v.map", bytes: &chain }]).unwrap();
    assert_eq!(s.nodes.len(), 50_000);
    assert_eq!(s.nodes[49_999].parents[0].node, 49_998);
}

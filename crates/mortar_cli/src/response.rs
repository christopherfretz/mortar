//! Response files: `@file` (`spec/cli.md` §2.6). The quoting rules are
//! GNU ld's (libiberty's), found by experiment with GNU ld 2.44.

use std::ffi::OsString;
use std::io::Read as _;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use crate::PHASE;
use mortar_diag::{Diagnostic, Key, Kind, Place};

/// How many response files one command line may expand (§2.6.3). GNU ld
/// expands 1999 and fails on the 2000th.
pub const LIMIT: u32 = 1999;

/// A response file that was expanded: its path as the `@file` argument
/// spelled it, and the device and inode of the file read (`spec/cli.md`
/// §2.6.4: a file the link writes must not be one of these).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseFile {
    pub path: PathBuf,
    pub dev: u64,
    pub ino: u64,
}

/// What [`expand_recorded`] returns.
#[derive(Debug, Clone, Default)]
pub struct Expanded {
    /// The expanded command line.
    pub args: Vec<OsString>,
    /// The errors, keyed on the position of the `@file` argument.
    pub diagnostics: Vec<(Key, Diagnostic)>,
    /// Every response file read, in the order it was expanded (a file
    /// named twice is listed twice).
    pub files: Vec<ResponseFile>,
}

/// Expands every `@file` argument in place, recursively (§2.6). An
/// argument whose file cannot be opened stays as it is; a directory is an
/// error. Errors key on the position of the `@file` argument in the
/// expanded command line.
#[must_use]
pub fn expand(args: &[OsString]) -> (Vec<OsString>, Vec<(Key, Diagnostic)>) {
    let e = expand_recorded(args);
    (e.args, e.diagnostics)
}

/// [`expand`], also recording each file read (§2.6.4).
#[must_use]
pub fn expand_recorded(args: &[OsString]) -> Expanded {
    let mut out: Vec<OsString> = args.to_vec();
    let mut diagnostics = Vec::new();
    let mut files = Vec::new();
    let mut expanded = 0u32;
    let mut i = 0;
    while i < out.len() {
        let bytes = out[i].as_bytes();
        if bytes.len() < 2 || bytes[0] != b'@' {
            i += 1;
            continue;
        }
        let path = Path::new(std::ffi::OsStr::from_bytes(&bytes[1..])).to_path_buf();
        // One `open` and one `fstat`: the identity recorded is the file's
        // whose bytes are read.
        let opened = std::fs::File::open(&path).and_then(|f| f.metadata().map(|m| (f, m)));
        let is_dir = match &opened {
            Ok((_, m)) => m.is_dir(),
            Err(_) => path.is_dir(),
        };
        if is_dir {
            let key = Key::new(PHASE, index(i), 0);
            diagnostics.push((
                key,
                Diagnostic::error(Kind::CannotOpenFile {
                    path: path.to_string_lossy().into_owned(),
                    reason: "is a directory".to_owned(),
                })
                .at(Place::argument(out[i].to_string_lossy())),
            ));
            out.remove(i);
            continue;
        }
        // GNU ld: a file that cannot be read is an ordinary argument.
        let read = opened.and_then(|(f, m)| {
            let mut contents = Vec::with_capacity(usize::try_from(m.len()).unwrap_or(0));
            // Through `Take`: `File::read_to_end` would `fstat` again for
            // its size hint, which the capacity already gives.
            f.take(u64::MAX).read_to_end(&mut contents).map(|_| (contents, m))
        });
        let Ok((contents, meta)) = read else {
            i += 1;
            continue;
        };
        if expanded == LIMIT {
            diagnostics.push((
                Key::new(PHASE, index(i), 0),
                Diagnostic::error(Kind::ResponseFileLimit { limit: LIMIT })
                    .at(Place::argument(out[i].to_string_lossy())),
            ));
            out.remove(i);
            break;
        }
        expanded += 1;
        files.push(ResponseFile { path, dev: meta.dev(), ino: meta.ino() });
        let words = split(&contents);
        out.splice(i..=i, words.into_iter().map(OsString::from_vec));
        // Re-scan from `i`: the file's own `@file` arguments expand too.
    }
    Expanded { args: out, diagnostics, files }
}

fn index(i: usize) -> u32 {
    u32::try_from(i).unwrap_or(u32::MAX)
}

/// GNU ld's separators (§2.6.1).
const fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | b'\x0b' | b'\x0c')
}

/// Splits a response file into arguments (§2.6.1):
///
/// - Arguments are separated by space, tab, LF, CR, VT and FF.
/// - Outside quotes a backslash takes the next byte literally, and a
///   backslash-newline is removed.
/// - Inside single quotes every byte is literal.
/// - Inside double quotes a backslash escapes only `$`, `` ` ``, `"`, `\`
///   and newline (removed); before any other byte both bytes are kept.
/// - Quoted and unquoted parts join into one argument; `''` is an empty
///   argument. An unterminated quote runs to the end of the file.
/// - Nothing else is special: no `#` comments, no `$VAR`, `~` or globs.
#[must_use]
pub fn split(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut words = Vec::new();
    let mut word = Vec::new();
    let mut in_word = false;
    let mut i = 0;
    while i < bytes.len() {
        let byte = bytes[i];
        match byte {
            _ if is_space(byte) => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
                i += 1;
            }
            b'\\' => {
                in_word = true;
                if let Some(&next) = bytes.get(i + 1)
                    && next != b'\n'
                {
                    word.push(next);
                }
                i += 2;
            }
            b'\'' => {
                in_word = true;
                i += 1;
                while i < bytes.len() && bytes[i] != b'\'' {
                    word.push(bytes[i]);
                    i += 1;
                }
                i += 1;
            }
            b'"' => {
                in_word = true;
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    match (bytes[i], bytes.get(i + 1)) {
                        (b'\\', Some(b'\n')) => i += 2,
                        (b'\\', Some(&next @ (b'$' | b'`' | b'"' | b'\\'))) => {
                            word.push(next);
                            i += 2;
                        }
                        (other, _) => {
                            word.push(other);
                            i += 1;
                        }
                    }
                }
                i += 1;
            }
            _ => {
                in_word = true;
                word.push(byte);
                i += 1;
            }
        }
    }
    if in_word {
        words.push(word);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::split;

    fn words(text: &str) -> Vec<String> {
        split(text.as_bytes()).into_iter().map(|w| String::from_utf8(w).unwrap()).collect()
    }

    // spec: cli.md §2.6.1 (separators)
    #[test]
    fn separators() {
        assert_eq!(words("a b\tc\nd\re\x0bf\x0cg"), ["a", "b", "c", "d", "e", "f", "g"]);
        assert_eq!(words("  a   b  "), ["a", "b"]);
        assert!(words("").is_empty());
        assert!(words(" \n\t").is_empty());
    }

    // spec: cli.md §2.6.1 (backslash outside quotes)
    #[test]
    fn backslash_outside_quotes() {
        assert_eq!(words(r"a\ b"), ["a b"]);
        assert_eq!(words(r"a\\b"), [r"a\b"]);
        assert_eq!(words("a\\\nb"), ["ab"]);
        assert_eq!(words(r"\'x\'"), ["'x'"]);
        assert_eq!(words(r"\n"), ["n"]);
    }

    // spec: cli.md §2.6.1 (single quotes)
    #[test]
    fn single_quotes_are_literal() {
        assert_eq!(words(r"'a b\c'"), [r"a b\c"]);
        assert_eq!(words(r#"'a"b'"#), [r#"a"b"#]);
        assert_eq!(words("''"), [""]);
        assert_eq!(words("'' x"), ["", "x"]);
    }

    // spec: cli.md §2.6.1 (double quotes)
    #[test]
    fn double_quotes() {
        assert_eq!(words(r#""a b""#), ["a b"]);
        assert_eq!(words(r#""a\"b""#), [r#"a"b"#]);
        assert_eq!(words(r#""a\\b""#), [r"a\b"]);
        assert_eq!(words(r#""\$x\`""#), ["$x`"]);
        assert_eq!(words(r#""a\nb""#), [r"a\nb"]);
        assert_eq!(words("\"a\\\nb\""), ["ab"]);
        assert_eq!(words(r#""a'b""#), ["a'b"]);
    }

    // spec: cli.md §2.6.1 (parts join; unterminated quotes; nothing else)
    #[test]
    fn parts_join_and_nothing_else_is_special() {
        assert_eq!(words(r#"a'b c'"d e"f"#), ["ab cd ef"]);
        assert_eq!(words("'abc"), ["abc"]);
        assert_eq!(words("\"abc"), ["abc"]);
        assert_eq!(words("# not a comment"), ["#", "not", "a", "comment"]);
        assert_eq!(words("$HOME ~ *.o a;b"), ["$HOME", "~", "*.o", "a;b"]);
    }
}

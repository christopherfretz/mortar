//! Captured links (`spec/harness.md` "bench", "Capture" and "Replay"): the
//! recording wrapper that stands in for the linker under a compiler driver,
//! what it records, the response file a workload is replayed from, and the
//! dependency file a reference linker writes for the census of a workload.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// A link as the wrapper recorded it: where the driver ran the linker, and
/// with which arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capture {
    pub cwd: PathBuf,
    pub args: Vec<String>,
}

impl Capture {
    /// Reads a record of the wrapper: the working directory and each
    /// argument, each ended by a NUL.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let Some(body) = bytes.strip_suffix(&[0]) else {
            return Err("the record does not end with a NUL (the wrapper was cut short)".into());
        };
        let mut fields = Vec::new();
        for field in body.split(|b| *b == 0) {
            let text =
                std::str::from_utf8(field).map_err(|_| "an argument is not UTF-8".to_owned())?;
            fields.push(text.to_owned());
        }
        let cwd = PathBuf::from(fields.remove(0));
        if !cwd.is_absolute() {
            return Err(format!("the recorded directory `{}` is not absolute", cwd.display()));
        }
        if let Some(at) = fields.iter().find(|arg| arg.starts_with('@')) {
            return Err(format!(
                "the driver passed a response file of its own (`{at}`): its contents are not \
                 recorded"
            ));
        }
        Ok(Self { cwd, args: fields })
    }

    /// The output the link names: the value of its last `-o <file>`,
    /// `-o<file>`, `--output <file>` or `--output=<file>`.
    pub fn output(&self) -> Option<&str> {
        let mut found = None;
        let mut args = self.args.iter();
        while let Some(arg) = args.next() {
            if arg == "-o" || arg == "--output" {
                found = args.next().map(String::as_str).or(found);
            } else if let Some(joined) = arg.strip_prefix("--output=") {
                found = Some(joined);
            } else if let Some(joined) = arg.strip_prefix("-o").filter(|rest| !rest.is_empty()) {
                // `-omagic` and its relatives are spelled with two dashes
                // by the drivers; a joined `-o<file>` is GNU ld's reading.
                found = Some(joined);
            }
        }
        found
    }
}

/// The text of the recording wrapper, a `/bin/sh` script named `ld` in a
/// directory the driver searches first (`COMPILER_PATH`): it writes the
/// directory and the arguments it was run with to a new file in `records`,
/// each ended by a NUL, then runs `real` with the same arguments, or exits
/// with status 0 when there is none (the harness does not need the output).
pub fn wrapper_script(records: &Path, real: Option<&Path>) -> String {
    let mut script = String::from("#!/bin/sh\n");
    let _ = writeln!(
        script,
        "record=$(mktemp {}/link.XXXXXXXX) || exit 125",
        sh_quote(&records.to_string_lossy())
    );
    script.push_str(
        "{ printf '%s\\0' \"$PWD\"; for a in \"$@\"; do printf '%s\\0' \"$a\"; done; } \
         > \"$record\" || exit 125\n",
    );
    match real {
        Some(real) => {
            let _ = writeln!(script, "exec {} \"$@\"", sh_quote(&real.to_string_lossy()));
        }
        None => script.push_str("exit 0\n"),
    }
    script
}

/// `text` as one word of a `/bin/sh` command.
pub fn sh_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// Whether `byte` may stand unquoted in a response file.
fn is_plain(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&byte)
}

/// The response file of `args`: one argument per line, written so that GNU
/// ld, lld and mold read the same arguments back. An argument of plain
/// characters stands as it is; any other is enclosed in double quotes with
/// a backslash before each backslash and double quote, the one form the
/// three tokenizers agree on (mold does not join a quoted part to an
/// unquoted one).
pub fn write_response(args: &[String]) -> String {
    let mut text = String::new();
    for arg in args {
        if !arg.is_empty() && arg.bytes().all(is_plain) {
            text.push_str(arg);
        } else {
            text.push('"');
            for c in arg.chars() {
                if c == '\\' || c == '"' {
                    text.push('\\');
                }
                text.push(c);
            }
            text.push('"');
        }
        text.push('\n');
    }
    text
}

/// Reads a response file by GNU ld's rules (libiberty's `buildargv`):
/// blanks separate arguments; single and double quotes enclose blanks and
/// each other and may start or end inside an argument; a backslash makes
/// the next character literal, inside quotes too. A quote that is not
/// closed, or a backslash at the end, is an error. An argument that starts
/// with `@` is returned as it is, not expanded.
pub fn parse_response(text: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current: Option<String> = None;
    let mut quote: Option<char> = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            let next = chars.next().ok_or("a backslash ends the file")?;
            current.get_or_insert_default().push(next);
        } else if quote == Some(c) {
            quote = None;
        } else if quote.is_some() {
            current.get_or_insert_default().push(c);
        } else if c == '\'' || c == '"' {
            quote = Some(c);
            current.get_or_insert_default();
        } else if c.is_ascii_whitespace() {
            args.extend(current.take());
        } else {
            current.get_or_insert_default().push(c);
        }
    }
    if let Some(open) = quote {
        return Err(format!("a {open} quote is not closed"));
    }
    args.extend(current);
    Ok(args)
}

/// The prerequisites of the first rule of a dependency file as the
/// reference linkers write it (`--dependency-file`, make's format): the
/// files the link read, in the order and as often as the linker lists
/// them. A backslash before a line end continues the rule, before a blank
/// makes it part of the name, and `$$` is one `$`.
pub fn parse_depfile(text: &str) -> Result<Vec<String>, String> {
    let rule = text.split("\n\n").next().unwrap_or("");
    let start = rule
        .char_indices()
        .find(|&(at, c)| c == ':' && rule[at + 1..].starts_with([' ', '\t', '\n', '\\']))
        .map(|(at, _)| at + 1)
        .or_else(|| rule.trim_end().ends_with(':').then_some(rule.len()))
        .ok_or("no rule in the dependency file")?;
    let mut files = Vec::new();
    let mut current = String::new();
    let mut chars = rule[start..].chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'\n') => {
                chars.next();
            }
            '\\' if chars.peek() == Some(&' ') => {
                chars.next();
                current.push(' ');
            }
            '$' if chars.peek() == Some(&'$') => {
                chars.next();
                current.push('$');
            }
            c if c.is_ascii_whitespace() => {
                if !current.is_empty() {
                    files.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        files.push(current);
    }
    Ok(files)
}

/// Whether `text` matches `pattern`, in which `*` stands for any run of
/// characters and every other character for itself.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let Some((head, rest)) = pattern.split_once('*') else {
        return pattern == text;
    };
    let Some(mut text) = text.strip_prefix(head) else {
        return false;
    };
    let mut parts: Vec<&str> = rest.split('*').collect();
    let tail = parts.pop().unwrap_or("");
    for part in parts {
        match text.find(part) {
            Some(at) => text = &text[at + part.len()..],
            None => return false,
        }
    }
    text.ends_with(tail)
}

// spec/harness.md "bench", "Capture" and "Replay".
#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| (*a).to_owned()).collect()
    }

    // "Capture": the wrapper's record is the directory and the arguments,
    // NUL-ended; what cannot be replayed is refused.
    #[test]
    fn capture_records_are_parsed() {
        let capture = Capture::parse(b"/w/build\0-pie\0-o\0out\0a b.o\0\0-lc\0").unwrap();
        assert_eq!(capture.cwd, Path::new("/w/build"));
        assert_eq!(capture.args, strings(&["-pie", "-o", "out", "a b.o", "", "-lc"]));
        assert_eq!(capture.output(), Some("out"));
        assert_eq!(Capture::parse(b"/w\0").unwrap().args, Vec::<String>::new());

        let cut = Capture::parse(b"/w\0-o\0ou").unwrap_err();
        assert!(cut.contains("cut short"), "{cut}");
        assert!(Capture::parse(b"").is_err());
        assert!(Capture::parse(b"relative\0a.o\0").unwrap_err().contains("not absolute"));
        assert!(Capture::parse(b"/w\0\xff.o\0").unwrap_err().contains("UTF-8"));
        let nested = Capture::parse(b"/w\0@/tmp/cc1.args\0").unwrap_err();
        assert!(nested.contains("@/tmp/cc1.args"), "{nested}");
    }

    // "Capture": the output a link names, in each spelling; the last wins.
    #[test]
    fn the_output_of_a_link() {
        let output = |args: &[&str]| {
            let capture = Capture { cwd: "/w".into(), args: strings(args) };
            capture.output().map(str::to_owned)
        };
        assert_eq!(output(&["a.o", "-o", "x"]), Some("x".into()));
        assert_eq!(output(&["-o", "x", "--output", "y"]), Some("y".into()));
        assert_eq!(output(&["--output=dir/z", "a.o"]), Some("dir/z".into()));
        assert_eq!(output(&["-ox"]), Some("x".into()));
        assert_eq!(output(&["a.o", "-lc"]), None);
        assert_eq!(output(&["-o"]), None);
    }

    // "Capture": the wrapper records, then runs the real linker or ends.
    #[test]
    fn the_wrapper_script() {
        let recording = wrapper_script(Path::new("/c/it's"), None);
        assert!(recording.starts_with("#!/bin/sh\n"), "{recording}");
        assert!(recording.contains("mktemp '/c/it'\\''s'/link.XXXXXXXX"), "{recording}");
        assert!(recording.ends_with("exit 0\n"));
        let linking = wrapper_script(Path::new("/c"), Some(Path::new("/usr/bin/ld.bfd")));
        assert!(linking.ends_with("exec '/usr/bin/ld.bfd' \"$@\"\n"), "{linking}");
    }

    // "Replay": a response file is one argument per line, and reading it
    // gives the arguments back, whatever is in them.
    #[test]
    fn response_files_round_trip() {
        let args = strings(&[
            "-pie",
            "--hash-style=gnu",
            "-L/usr/lib/gcc/aarch64-linux-gnu/14/../../..",
            "a b.o",
            "",
            "quote\"d",
            "back\\slash",
            "it's",
            "tab\tand\nline",
            "$HOME",
            "naïve.o",
        ]);
        let text = write_response(&args);
        let expected = "-pie\n--hash-style=gnu\n-L/usr/lib/gcc/aarch64-linux-gnu/14/../../..\n\
                        \"a b.o\"\n\"\"\n\"quote\\\"d\"\n\"back\\\\slash\"\n\"it's\"\n\
                        \"tab\tand\nline\"\n\"$HOME\"\n\"naïve.o\"\n";
        assert_eq!(text, expected);
        assert_eq!(parse_response(&text).unwrap(), args);
        assert_eq!(write_response(&[]), "");
    }

    // "Replay": GNU ld's rules for a response file somebody else wrote.
    #[test]
    fn response_files_are_parsed_by_gnu_rules() {
        let parse = |text: &str| parse_response(text).unwrap();
        assert_eq!(parse("a.o  b.o\n\t-lc\r\n"), strings(&["a.o", "b.o", "-lc"]));
        assert_eq!(parse(""), Vec::<String>::new());
        assert_eq!(parse("'a b' \"c d\" e\\ f"), strings(&["a b", "c d", "e f"]));
        // Quotes join what is around them and enclose each other.
        assert_eq!(parse("-L'my dir'/lib \"it's\" '\"'"), strings(&["-Lmy dir/lib", "it's", "\""]));
        // A backslash is an escape inside quotes of both kinds.
        assert_eq!(parse("'a\\'b' \"c\\\\d\""), strings(&["a'b", "c\\d"]));
        assert_eq!(parse("'' x"), strings(&["", "x"]));
        assert_eq!(parse("@nested.rsp"), strings(&["@nested.rsp"]));
        assert!(parse_response("'open").unwrap_err().contains("not closed"));
        assert!(parse_response("a\\").unwrap_err().contains("backslash"));
    }

    // "Census": the first rule of a dependency file, as each of the
    // reference linkers lays it out.
    #[test]
    fn dependency_files_are_parsed() {
        let bfd = "out: \\\n  /usr/lib/crt1.o \\\n  hello.o \\\n  /usr/lib/libc.so\n\n\
                   /usr/lib/crt1.o:\n\nhello.o:\n";
        assert_eq!(
            parse_depfile(bfd).unwrap(),
            strings(&["/usr/lib/crt1.o", "hello.o", "/usr/lib/libc.so"])
        );
        let mold = "/s/out: /usr/lib/crt1.o hello.o my\\ lib.a cost$$.o\n\n/usr/lib/crt1.o:\n";
        assert_eq!(
            parse_depfile(mold).unwrap(),
            strings(&["/usr/lib/crt1.o", "hello.o", "my lib.a", "cost$.o"])
        );
        assert_eq!(parse_depfile("out:\n").unwrap(), Vec::<String>::new());
        assert!(parse_depfile("").is_err());
        assert!(parse_depfile("no rule here\n").is_err());
    }

    // "Capture": the pattern that selects the link to capture.
    #[test]
    fn globs() {
        for (pattern, text, matches) in [
            ("out", "out", true),
            ("out", "out2", false),
            ("protoc*", "protoc-3.21.12.0", true),
            ("protoc*", "libprotoc.so", false),
            ("mortar-*", "mortar-0a1b2c", true),
            ("mortar-*", "mortar", false),
            ("*", "", true),
            ("a*b*c", "aXbYc", true),
            ("a*b*c", "abc", true),
            ("a*b*c", "acb", false),
            ("*.so", "libc.so", true),
            ("a*a", "a", false),
        ] {
            assert_eq!(glob_match(pattern, text), matches, "{pattern} {text}");
        }
    }
}

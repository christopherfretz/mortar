//! `spec/cli.md` §3.15 (WP-095): every byte a shipped crate prints goes
//! through `mortar_diag::stream`, whose writes never panic. A source scan
//! of `crates/*/src` (examples, tests and `mortar_testutil` are not
//! shipped) fails on a raw `print!`, `println!`, `eprint!`, `eprintln!`,
//! `dbg!`, a `stdout()`/`stderr()` call or an `io::Stdout`/`io::Stderr`
//! (or `...Lock`) type anywhere but
//! the writer itself: those panic on `EPIPE`/`ENOSPC`, and mortar builds
//! with `panic = "abort"`.

use std::path::{Path, PathBuf};

/// The one file allowed to touch the standard streams.
const WRITER: &str = "crates/mortar_diag/src/stream.rs";
/// Crates under `crates/` that are not shipped.
const NOT_SHIPPED: &[&str] = &["mortar_testutil"];

/// The raw uses on one line of Rust, outside a `//` comment.
fn raw_uses(line: &str) -> Vec<&'static str> {
    let code = line.trim_start();
    if code.starts_with("//") {
        return Vec::new();
    }
    let bytes = code.as_bytes();
    let starts_word = |at: usize| {
        at == 0 || {
            let c = bytes[at - 1];
            !(c.is_ascii_alphanumeric() || c == b'_' || c == b'.')
        }
    };
    let mut found = Vec::new();
    for (name, follow) in [
        ("println", "!"),
        ("print", "!"),
        ("eprintln", "!"),
        ("eprint", "!"),
        ("dbg", "!"),
        ("stdout", "()"),
        ("stderr", "()"),
        ("io::Stdout", ""),
        ("io::Stderr", ""),
        ("io::StdoutLock", ""),
        ("io::StderrLock", ""),
    ] {
        let mut from = 0;
        while let Some(i) = code[from..].find(name) {
            let at = from + i;
            let end = at + name.len();
            let rest = &code[end..];
            let ends_word =
                rest.as_bytes().first().is_none_or(|c| !(c.is_ascii_alphanumeric() || *c == b'_'));
            if starts_word(at) && rest.starts_with(follow) && (!follow.is_empty() || ends_word) {
                found.push(name);
            }
            from = end;
        }
    }
    found
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

// spec: cli.md §3.15
#[test]
fn the_scan_sees_every_raw_form() {
    for line in [
        r#"    eprintln!("x");"#,
        r#"println!("{x}")"#,
        "print!(\"a\");",
        "eprint!(\"a\");",
        "let _ = dbg!(x);",
        "let _ = std::io::stderr().write_all(b);",
        "let mut o = stdout();",
        "fn f(e: &mut std::io::Stderr) {}",
        "let l: std::io::StdoutLock<'_> = x;",
    ] {
        assert!(!raw_uses(line).is_empty(), "missed: {line}");
    }
    for line in [
        "// eprintln!(\"x\") in a comment",
        "/// `print!` in a doc comment",
        "mortar_diag::stream::to_stderr(&s);",
        "let _ = to_stdout(&text);",
        "command.stdout(Stdio::piped());",
        "out.stdout_text()",
        "let reprint = 1;",
        "stderr_is_terminal()",
        "Some(Report::Stderr) => x,",
        "PrintMap::Stdout => {",
    ] {
        assert!(raw_uses(line).is_empty(), "false hit: {line}: {:?}", raw_uses(line));
    }
}

// spec: cli.md §3.15
#[test]
fn shipped_crates_print_only_through_the_writer() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    let crates = std::fs::read_dir(root.join("crates")).expect("crates/ is listed");
    for krate in crates.flatten() {
        let name = krate.file_name();
        if NOT_SHIPPED.iter().any(|n| name == **n) {
            continue;
        }
        rust_files(&krate.path().join("src"), &mut files);
    }
    files.sort();
    assert!(files.len() > 50, "the scan found only {} files", files.len());
    let writer = root.join(WRITER);
    assert!(files.contains(&writer), "{WRITER} is scanned");
    let mut hits = Vec::new();
    for file in files.iter().filter(|f| **f != writer) {
        let text = std::fs::read_to_string(file).expect("a source file reads");
        for (n, line) in text.lines().enumerate() {
            for name in raw_uses(line) {
                let shown = file.strip_prefix(&root).unwrap_or(file).display().to_string();
                hits.push(format!("{shown}:{}: {name}", n + 1));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "print through mortar_diag::stream (spec/cli.md §3.15), not:\n{}",
        hits.join("\n")
    );
}

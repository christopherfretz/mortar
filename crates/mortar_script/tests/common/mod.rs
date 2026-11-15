//! Helpers shared by the tests of `mortar_script`.
#![allow(dead_code)]

use mortar_diag::{Diagnostic, Kind, Place};
use mortar_script::{CommandKind, FileForm, InputItem, Script, parse};

/// What a failed parse said: the id, the line and column, and the refused
/// name or the malformed statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Said {
    pub id: &'static str,
    pub line: u32,
    pub column: u32,
    pub text: String,
}

pub fn said(d: &Diagnostic) -> Said {
    let Some(Place::Script { line, column, path }) = d.place() else {
        panic!("not placed in a script: {d:?}");
    };
    assert_eq!(path, "t.ld");
    let text = match d.kind() {
        Kind::Malformed { what } => what.clone(),
        Kind::Refused { name, .. } => name.clone(),
        other => panic!("unexpected kind {other:?}"),
    };
    Said { id: d.id(), line: *line, column: *column, text }
}

/// Parses, expecting success.
pub fn ok(src: &str) -> Script {
    parse(src.as_bytes(), "t.ld").unwrap_or_else(|d| panic!("{src:?}: {d:?}"))
}

/// Parses, expecting a refusal of `name` at `line:column`.
pub fn refused(src: &str, name: &str, line: u32, column: u32) {
    let d = parse(src.as_bytes(), "t.ld").expect_err(src);
    assert_eq!(said(&d), Said { id: "refused", line, column, text: name.to_owned() }, "{src:?}");
    let (class, _) = d.refusal().unwrap();
    assert_eq!(class.id(), "script-command");
}

/// Parses, expecting a syntax error at `line:column` whose statement
/// contains `fragment`.
pub fn malformed(src: &[u8], line: u32, column: u32, fragment: &str) -> Diagnostic {
    let d = parse(src, "t.ld").expect_err(&String::from_utf8_lossy(src));
    let s = said(&d);
    assert_eq!(
        (s.id, s.line, s.column),
        ("malformed-input", line, column),
        "{:?}: {}",
        String::from_utf8_lossy(src),
        s.text
    );
    assert!(
        s.text.contains(fragment),
        "{:?}: {:?} lacks {fragment:?}",
        String::from_utf8_lossy(src),
        s.text
    );
    *d
}

/// A compact rendering of an input list, for structural assertions:
/// `path`, `=path` (sysroot), `-lname`, `-l:file`, `AS_NEEDED(...)`,
/// a quoted path in double quotes.
pub fn items(list: &[InputItem]) -> String {
    let mut out = Vec::new();
    for item in list {
        out.push(match item {
            InputItem::File(f) => {
                let name = String::from_utf8_lossy(&f.name);
                let shown = match f.form {
                    FileForm::Path => name.into_owned(),
                    FileForm::Sysroot => format!("={name}"),
                    FileForm::Library => format!("-l{name}"),
                    FileForm::LibraryVerbatim => format!("-l:{name}"),
                };
                if f.spelling.quoted { format!("\"{shown}\"") } else { shown }
            }
            InputItem::AsNeeded { items: inner, .. } => format!("AS_NEEDED({})", items(inner)),
        });
    }
    out.join(" ")
}

/// A compact rendering of a whole script, one command per entry.
pub fn summary(script: &Script) -> Vec<String> {
    let w = |w: &mortar_script::Word| w.to_string_lossy();
    script
        .commands
        .iter()
        .map(|c| match &c.kind {
            CommandKind::Input(l) => format!("INPUT({})", items(l)),
            CommandKind::Group(l) => format!("GROUP({})", items(l)),
            CommandKind::OutputFormat { default, big: None, little: None } => {
                format!("OUTPUT_FORMAT({})", w(default))
            }
            CommandKind::OutputFormat { default, big, little } => format!(
                "OUTPUT_FORMAT({},{},{})",
                w(default),
                big.as_ref().map(w).unwrap_or_default(),
                little.as_ref().map(w).unwrap_or_default()
            ),
            CommandKind::OutputArch(x) => format!("OUTPUT_ARCH({})", w(x)),
            CommandKind::Target(x) => format!("TARGET({})", w(x)),
            CommandKind::SearchDir(x) => format!("SEARCH_DIR({})", w(x)),
            CommandKind::Output(x) => format!("OUTPUT({})", w(x)),
            CommandKind::Entry(x) => format!("ENTRY({})", w(x)),
            CommandKind::Extern(xs) => {
                format!("EXTERN({})", xs.iter().map(w).collect::<Vec<_>>().join(" "))
            }
            CommandKind::Startup(x) => format!("STARTUP({})", w(x)),
            CommandKind::Include(x) => format!("INCLUDE {}", w(x)),
        })
        .collect()
}

//! The exact text of every kind (`spec/diag.md` §3). One test per kind, in
//! the order of the table of §3.1; a new kind's test is inserted at its
//! place (§3.3).

use mortar_diag::{Diagnostic, FileRef, Kind, Place, Relation, Style, os_reason};

fn s(text: &str) -> String {
    text.to_owned()
}

/// The text of the kind as an error and as a warning, program `ld`: the
/// two differ in the severity word only.
fn text(kind: Kind) -> String {
    let error = Diagnostic::error(kind.clone()).render(&Style::plain("ld"));
    let warning = Diagnostic::warning(kind).render(&Style::plain("ld"));
    let message = error.strip_prefix("ld: error: ").expect("an error's head line");
    assert_eq!(warning, format!("ld: warning: {message}"));
    error
}

// -- command line -------------------------------------------------------

// spec: diag.md §3.1 `invalid-option-value`
#[test]
fn invalid_option_value() {
    let kind = Kind::InvalidOptionValue { option: s("--hash-style"), value: s("frob") };
    assert_eq!(kind.id(), "invalid-option-value");
    assert_eq!(text(kind.clone()), "ld: error: --hash-style: invalid value 'frob'\n");
    let diag = Diagnostic::error(kind).note("expected one of: sysv, gnu, both");
    assert_eq!(
        diag.render(&Style::plain("mortar")),
        "mortar: error: --hash-style: invalid value 'frob'\n\
         >>> expected one of: sysv, gnu, both\n"
    );
}

// spec: diag.md §3.1 `missing-argument`
#[test]
fn missing_argument() {
    let kind = Kind::MissingArgument { option: s("-o") };
    assert_eq!(kind.id(), "missing-argument");
    assert_eq!(text(kind), "ld: error: -o: missing argument\n");
}

// spec: diag.md §3.1 `no-input-files`
#[test]
fn no_input_files() {
    assert_eq!(Kind::NoInputFiles.id(), "no-input-files");
    assert_eq!(text(Kind::NoInputFiles), "ld: error: no input files\n");
}

// spec: diag.md §3.1 `unknown-option`
#[test]
fn unknown_option() {
    let kind = Kind::UnknownOption { option: s("--frobnicate") };
    assert_eq!(kind.id(), "unknown-option");
    assert_eq!(text(kind), "ld: error: unknown argument '--frobnicate'\n");
}

// -- files --------------------------------------------------------------

// spec: diag.md §3.1 `cannot-open-file`
#[test]
fn cannot_open_file() {
    let kind = Kind::CannotOpenFile { path: s("nosuch.o"), reason: s("No such file or directory") };
    assert_eq!(kind.id(), "cannot-open-file");
    assert_eq!(text(kind.clone()), "ld: error: cannot open nosuch.o: No such file or directory\n");
    let diag = Diagnostic::error(kind).with(Relation::NamedBy, Place::script("libc.so", 5, 9));
    assert_eq!(
        diag.render(&Style::plain("ld")),
        "ld: error: cannot open nosuch.o: No such file or directory\n\
         >>> named by libc.so:5:9\n"
    );
}

// spec: diag.md §3.1 `cannot-open-output`
#[test]
fn cannot_open_output() {
    let kind = Kind::CannotOpenOutput { path: s("/no/dir/out"), reason: s("Permission denied") };
    assert_eq!(kind.id(), "cannot-open-output");
    assert_eq!(text(kind), "ld: error: cannot open output file /no/dir/out: Permission denied\n");
}

// spec: diag.md §3.1 `library-not-found`, §3.2 (the name is the text after `-l`)
#[test]
fn library_not_found() {
    let kind = Kind::LibraryNotFound { name: s("nosuch") };
    assert_eq!(kind.id(), "library-not-found");
    assert_eq!(text(kind), "ld: error: unable to find library -lnosuch\n");
    assert_eq!(
        text(Kind::LibraryNotFound { name: s(":libx.a") }),
        "ld: error: unable to find library -l:libx.a\n"
    );
}

// spec: diag.md §3.2 (`{reason}` is the system's message, without Rust's
// ` (os error N)`)
#[test]
fn os_reason_is_the_system_message() {
    // ENOENT is 2 on every Linux architecture.
    let error = std::io::Error::from_raw_os_error(2);
    assert!(error.to_string().ends_with(" (os error 2)"));
    assert_eq!(os_reason(&error), "No such file or directory");
    // An error that is not the system's keeps its own text.
    let custom = std::io::Error::other("stream ended (os error 2)");
    assert_eq!(os_reason(&custom), "stream ended (os error 2)");
    // A real failure, through the kind.
    let error = std::fs::read("/nonexistent-mortar-diag/x.o").expect_err("no such file");
    let kind = Kind::CannotOpenFile { path: s("x.o"), reason: os_reason(&error) };
    assert_eq!(text(kind), "ld: error: cannot open x.o: No such file or directory\n");
}

// -- file formats -------------------------------------------------------

// spec: diag.md §3.1 `malformed-input`, §3.2 (emitted with a place)
#[test]
fn malformed_input() {
    let kind = Kind::Malformed {
        what: s("section header table goes past the end of the file: e_shoff = 0x2e8"),
    };
    assert_eq!(kind.id(), "malformed-input");
    assert_eq!(
        text(kind.clone()),
        "ld: error: section header table goes past the end of the file: e_shoff = 0x2e8\n"
    );
    let diag = Diagnostic::error(kind).at(Place::file("trunc.o"));
    assert_eq!(
        diag.render(&Style::plain("ld")),
        "ld: error: trunc.o: section header table goes past the end of the file: \
         e_shoff = 0x2e8\n"
    );
}

// spec: diag.md §3.1 `unknown-file-type`, §3.2 (emitted with a place)
#[test]
fn unknown_file_type() {
    assert_eq!(Kind::UnknownFileType.id(), "unknown-file-type");
    assert_eq!(text(Kind::UnknownFileType), "ld: error: unknown file type\n");
    let diag = Diagnostic::error(Kind::UnknownFileType).at(FileRef::in_archive("libx.a", "y.o"));
    assert_eq!(diag.render(&Style::plain("ld")), "ld: error: libx.a(y.o): unknown file type\n");
}

// -- symbols ------------------------------------------------------------

// spec: diag.md §3.1 `duplicate-symbol`
#[test]
fn duplicate_symbol() {
    let kind = Kind::DuplicateSymbol { name: s("twice") };
    assert_eq!(kind.id(), "duplicate-symbol");
    assert_eq!(text(kind.clone()), "ld: error: duplicate symbol: twice\n");
    let diag = Diagnostic::error(kind)
        .with(Relation::DefinedIn, Place::section(FileRef::new("dup1.o"), ".text", 0))
        .with(
            Relation::DefinedIn,
            Place::section(FileRef::in_archive("libdup.a", "dup2.o"), ".text", 0),
        );
    assert_eq!(
        diag.render(&Style::plain("ld")),
        "ld: error: duplicate symbol: twice\n\
         >>> defined in dup1.o:(.text+0x0)\n\
         >>> defined in libdup.a(dup2.o):(.text+0x0)\n"
    );
}

// spec: diag.md §3.1 `undefined-symbol`
#[test]
fn undefined_symbol() {
    let kind = Kind::UndefinedSymbol { name: s("missing") };
    assert_eq!(kind.id(), "undefined-symbol");
    assert_eq!(text(kind.clone()), "ld: error: undefined symbol: missing\n");
    let diag = Diagnostic::error(kind)
        .with(Relation::ReferencedBy, Place::section(FileRef::new("undef.o"), ".text", 0xc))
        .with(Relation::ReferencedBy, Place::symbol(FileRef::new("undef.o"), "_start"));
    assert_eq!(
        diag.render(&Style::plain("ld.mortar")),
        "ld.mortar: error: undefined symbol: missing\n\
         >>> referenced by undef.o:(.text+0xc)\n\
         >>> referenced by undef.o:(_start)\n"
    );
}

// spec: diag.md §3.3 (a field that holds a symbol name is `Kind::symbol`)
#[test]
fn symbol_kinds_report_their_symbol() {
    assert_eq!(Kind::UndefinedSymbol { name: s("_Z1fv") }.symbol(), Some("_Z1fv"));
    assert_eq!(Kind::DuplicateSymbol { name: s("_Z1gv") }.symbol(), Some("_Z1gv"));
    assert_eq!(Kind::LibraryNotFound { name: s("m") }.symbol(), None);
    assert_eq!(Kind::NoInputFiles.symbol(), None);
}

// -- refusals -----------------------------------------------------------
// `refused` has a file of its own: tests/refusal.rs.

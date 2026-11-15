//! The exact text of every kind (`spec/diag.md` §3). One test per kind, in
//! the order of the table of §3.1; a new kind's test is inserted at its
//! place (§3.3).

use mortar_diag::{
    Diagnostic, FileRef, IncompatibleReason, Kind, Place, Relation, Severity, Style, os_reason,
};

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

// spec: diag.md §3.1 `conflicting-options`
#[test]
fn conflicting_options() {
    let kind = Kind::ConflictingOptions { first: s("-r"), second: s("-shared") };
    assert_eq!(kind.id(), "conflicting-options");
    assert_eq!(text(kind), "ld: error: -r and -shared cannot be used together\n");
}

// spec: diag.md §3.1 `response-file-limit`
#[test]
fn response_file_limit() {
    let kind = Kind::ResponseFileLimit { limit: 1999 };
    assert_eq!(kind.id(), "response-file-limit");
    assert_eq!(text(kind), "ld: error: too many response files: more than 1999 expanded\n");
}

// spec: diag.md §3.1 `unbalanced-option`
#[test]
fn unbalanced_option() {
    let kind = Kind::UnbalancedOption { option: s("--pop-state"), counterpart: s("--push-state") };
    assert_eq!(kind.id(), "unbalanced-option");
    assert_eq!(text(kind), "ld: error: --pop-state without a matching --push-state\n");
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

// spec: diag.md §3.1 `undefined-symbol`, §3.2 (one per reference, the
// reference's place in the head line; GNU ld's phrase), §5.3
#[test]
fn undefined_symbol() {
    let kind = Kind::UndefinedSymbol { name: s("missing") };
    assert_eq!(kind.id(), "undefined-symbol");
    assert_eq!(text(kind.clone()), "ld: error: undefined reference to 'missing'\n");
    let diag = Diagnostic::error(kind.clone()).at(Place::section(
        FileRef::in_archive("libu.a", "undef.o"),
        ".text",
        0xc,
    ));
    assert_eq!(
        diag.render(&Style::plain("ld.mortar")),
        "ld.mortar: error: libu.a(undef.o):(.text+0xc): undefined reference to 'missing'\n"
    );
    // The phrase build tools search for is in the line, as GNU ld prints it.
    assert!(diag.render(&Style::plain("ld")).contains("undefined reference to "));
    // Under --warn-unresolved-symbols the same line is a warning.
    let warning = Diagnostic::warning(kind).at(Place::section(FileRef::new("a.o"), ".data", 8));
    assert_eq!(
        warning.render(&Style::plain("ld")),
        "ld: warning: a.o:(.data+0x8): undefined reference to 'missing'\n"
    );
}

// spec: diag.md §3.3 (a field that holds a symbol name is `Kind::symbol`)
#[test]
fn symbol_kinds_report_their_symbol() {
    assert_eq!(Kind::UndefinedSymbol { name: s("_Z1fv") }.symbol(), Some("_Z1fv"));
    assert_eq!(Kind::DuplicateSymbol { name: s("_Z1gv") }.symbol(), Some("_Z1gv"));
    let incompatible = Kind::RelocationIncompatible {
        r_type: s("R_AARCH64_ABS32"),
        name: s("_Z1hv"),
        reason: IncompatibleReason::RecompileFpic,
    };
    assert_eq!(incompatible.symbol(), Some("_Z1hv"));
    let range = Kind::RelocationOutOfRange {
        r_type: s("R_AARCH64_CALL26"),
        value: 0,
        min: 0,
        max: 0,
        name: s("_Z1iv"),
    };
    assert_eq!(range.symbol(), Some("_Z1iv"));
    let misaligned = Kind::RelocationMisaligned {
        r_type: s("R_AARCH64_LDST64_ABS_LO12_NC"),
        value: 4,
        align: 8,
    };
    assert_eq!(misaligned.symbol(), None);
    assert_eq!(Kind::InternalError { what: s("_Z1jv") }.symbol(), None);
    assert_eq!(Kind::LibraryNotFound { name: s("m") }.symbol(), None);
    assert_eq!(Kind::NoInputFiles.symbol(), None);
}

// -- relocations --------------------------------------------------------

// spec: diag.md §3.1 `relocation-incompatible`, §3.2 (every reason's words)
#[test]
fn relocation_incompatible() {
    let kind = |reason| Kind::RelocationIncompatible {
        r_type: s("R_AARCH64_ADR_PREL_PG_HI21"),
        name: s("ext_var"),
        reason,
    };
    assert_eq!(kind(IncompatibleReason::RecompileFpic).id(), "relocation-incompatible");
    let words = [
        "recompile with -fPIC",
        "recompile with -fPIE",
        "it is in a read-only section",
        "the addend must be 0",
        "it is not thread-local",
        "it is thread-local",
        "local-exec TLS in a shared object",
        "the instruction does not match the relocation",
        "not allowed in a non-allocated section",
    ];
    assert_eq!(IncompatibleReason::ALL.len(), words.len());
    for (reason, words) in IncompatibleReason::ALL.into_iter().zip(words) {
        assert_eq!(reason.text(), words);
        assert_eq!(
            text(kind(reason)),
            format!(
                "ld: error: relocation R_AARCH64_ADR_PREL_PG_HI21 cannot be used against \
                 symbol 'ext_var'; {words}\n"
            )
        );
    }
    let diag = Diagnostic::error(kind(IncompatibleReason::RecompileFpie)).at(Place::section(
        FileRef::new("pie.o"),
        ".text",
        0x10,
    ));
    assert_eq!(
        diag.render(&Style::plain("ld")),
        "ld: error: pie.o:(.text+0x10): relocation R_AARCH64_ADR_PREL_PG_HI21 cannot be used \
         against symbol 'ext_var'; recompile with -fPIE\n"
    );
}

// spec: diag.md §3.1 `relocation-misaligned` (the value in decimal)
#[test]
fn relocation_misaligned() {
    let kind = Kind::RelocationMisaligned {
        r_type: s("R_AARCH64_LDST64_ABS_LO12_NC"),
        value: 0x41_0004,
        align: 8,
    };
    assert_eq!(kind.id(), "relocation-misaligned");
    assert_eq!(
        text(kind),
        "ld: error: improper alignment for relocation R_AARCH64_LDST64_ABS_LO12_NC: 4259844 is \
         not aligned to 8 bytes\n"
    );
}

// spec: diag.md §3.1 `relocation-out-of-range` (value and bounds in
// decimal, negative ones signed; the target's name through the hook)
#[test]
fn relocation_out_of_range() {
    let kind = Kind::RelocationOutOfRange {
        r_type: s("R_AARCH64_CONDBR19"),
        value: -1_048_580,
        min: -1_048_576,
        max: 1_048_575,
        name: s("far_away"),
    };
    assert_eq!(kind.id(), "relocation-out-of-range");
    assert_eq!(
        text(kind.clone()),
        "ld: error: relocation R_AARCH64_CONDBR19 out of range: -1048580 is not in [-1048576, \
         1048575]; references 'far_away'\n"
    );
    let diag = Diagnostic::error(kind).at(Place::section(FileRef::new("b.o"), ".text", 0x20));
    assert_eq!(
        diag.render(&Style::plain("ld")),
        "ld: error: b.o:(.text+0x20): relocation R_AARCH64_CONDBR19 out of range: -1048580 is \
         not in [-1048576, 1048575]; references 'far_away'\n"
    );
    // A section symbol's target: the section and offset (spec/arch.md §7).
    let section = Kind::RelocationOutOfRange {
        r_type: s("R_X86_64_PC32"),
        value: 2_147_483_648,
        min: -2_147_483_648,
        max: 2_147_483_647,
        name: s(".rodata+0x40"),
    };
    assert_eq!(
        text(section),
        "ld: error: relocation R_X86_64_PC32 out of range: 2147483648 is not in [-2147483648, \
         2147483647]; references '.rodata+0x40'\n"
    );
}

// -- refusals -----------------------------------------------------------
// `refused` has a file of its own: tests/refusal.rs.

// -- internal -----------------------------------------------------------

// spec: diag.md §3.1 `internal-error`, §2.1 (a bug of mortar's found in its
// own data; always an error, also when asked for a warning)
#[test]
fn internal_error() {
    let kind = Kind::InternalError {
        what: s("invalid layout: §4.4 rule 1: a live section has no place"),
    };
    assert_eq!(kind.id(), "internal-error");
    let error = Diagnostic::error(kind.clone()).note("section 7");
    assert_eq!(
        error.render(&Style::plain("ld")),
        "ld: error: internal error in mortar: invalid layout: §4.4 rule 1: a live section has \
         no place\n\
         >>> section 7\n"
    );
    let asked = Diagnostic::warning(kind.clone());
    assert_eq!(asked.severity(), Severity::Error);
    assert_eq!(Diagnostic::new(Severity::Warning, kind).severity(), Severity::Error);
}

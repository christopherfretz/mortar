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

// spec: diag.md §3.1 `ambiguous-sysroot-path`
#[test]
fn ambiguous_sysroot_path() {
    let kind = Kind::AmbiguousSysrootPath { spelling: s("=lib") };
    assert_eq!(kind.id(), "ambiguous-sysroot-path");
    assert_eq!(text(kind), "ld: error: ambiguous sysroot path '=lib'\n");
}

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

// spec: diag.md §8.3 (a directory as input: the reason carries GNU ld's
// `file format not recognized`)
#[test]
fn cannot_open_file_for_a_directory() {
    let kind = Kind::CannotOpenFile {
        path: s("dir"),
        reason: s("Is a directory; file format not recognized"),
    };
    assert_eq!(
        text(kind),
        "ld: error: cannot open dir: Is a directory; file format not recognized\n"
    );
}

// spec: diag.md §3.1 `cannot-open-output`
#[test]
fn cannot_open_output() {
    let kind = Kind::CannotOpenOutput { path: s("/no/dir/out"), reason: s("Permission denied") };
    assert_eq!(kind.id(), "cannot-open-output");
    assert_eq!(text(kind), "ld: error: cannot open output file /no/dir/out: Permission denied\n");
}

// spec: diag.md §3.1 `cannot-write-output`; image.md I-64
#[test]
fn cannot_write_output() {
    let kind = Kind::CannotWriteOutput { path: s("out"), reason: s("No space left on device") };
    assert_eq!(kind.id(), "cannot-write-output");
    assert_eq!(text(kind), "ld: error: cannot write output file out: No space left on device\n");
}

// spec: diag.md §3.1 `duplicate-soname`
#[test]
fn duplicate_soname() {
    let kind = Kind::DuplicateSoname { soname: s("libm.so.6") };
    assert_eq!(kind.id(), "duplicate-soname");
    let diag = Diagnostic::warning(kind.clone())
        .at(Place::file("b/libm.so"))
        .note("loaded from a/libm.so");
    assert_eq!(
        text(kind),
        "ld: error: DT_SONAME 'libm.so.6' is already loaded; this file is not used\n"
    );
    assert_eq!(
        diag.render(&Style::plain("ld")),
        "ld: warning: b/libm.so: DT_SONAME 'libm.so.6' is already loaded; this file is not \
         used\n>>> loaded from a/libm.so\n"
    );
}

// spec: diag.md §3.1 `ignored-member`
#[test]
fn ignored_member() {
    let kind = Kind::IgnoredMember { what: s("ELF shared object") };
    assert_eq!(kind.id(), "ignored-member");
    let diag = Diagnostic::warning(kind.clone()).at(FileRef::in_archive("libx.a", "y.so"));
    assert_eq!(
        text(kind),
        "ld: error: archive member is not a relocatable object: ELF shared object\n"
    );
    assert_eq!(
        diag.render(&Style::plain("ld")),
        "ld: warning: libx.a(y.so): archive member is not a relocatable object: ELF shared \
         object\n"
    );
}

// spec: diag.md §3.1 `incompatible-file`
#[test]
fn incompatible_file() {
    let kind = Kind::IncompatibleFile { expected: s("aarch64"), found: s("x86_64") };
    assert_eq!(kind.id(), "incompatible-file");
    let diag = Diagnostic::error(kind.clone()).at(Place::file("x64.o"));
    assert_eq!(text(kind), "ld: error: incompatible file: aarch64 is expected but got x86_64\n");
    assert_eq!(
        diag.render(&Style::plain("ld")),
        "ld: error: x64.o: incompatible file: aarch64 is expected but got x86_64\n"
    );
}

// spec: diag.md §3.1 `input-changed`
#[test]
fn input_changed() {
    assert_eq!(Kind::InputChanged.id(), "input-changed");
    let diag = Diagnostic::error(Kind::InputChanged).at(Place::file("a.o"));
    assert_eq!(text(Kind::InputChanged), "ld: error: file changed while it was read\n");
    assert_eq!(
        diag.render(&Style::plain("ld")),
        "ld: error: a.o: file changed while it was read\n"
    );
}

// spec: diag.md §3.1 `library-not-found`, §3.2 (the name is the text after `-l`)
#[test]
fn library_not_found() {
    let kind = Kind::LibraryNotFound { name: s("nosuch") };
    assert_eq!(kind.id(), "library-not-found");
    // GNU ld's line, verbatim after the head (§8.3).
    assert_eq!(text(kind), "ld: error: cannot find -lnosuch: No such file or directory\n");
    assert_eq!(
        text(Kind::LibraryNotFound { name: s(":libx.a") }),
        "ld: error: cannot find -l:libx.a: No such file or directory\n"
    );
}

// spec: diag.md §3.1 `needed-not-found`
#[test]
fn needed_not_found() {
    let kind = Kind::NeededNotFound { name: s("libz.so.1"), needed_by: s("libpng.so") };
    assert_eq!(kind.id(), "needed-not-found");
    assert_eq!(
        text(kind),
        "ld: error: libz.so.1, needed by libpng.so, not found (try using -rpath or -rpath-link)\n"
    );
}

// spec: diag.md §3.1 `no-target`
#[test]
fn no_target() {
    assert_eq!(Kind::NoTarget.id(), "no-target");
    assert_eq!(
        text(Kind::NoTarget),
        "ld: error: target emulation unknown: -m or at least one .o file required\n"
    );
}

// spec: diag.md §3.1 `script-cycle`
#[test]
fn script_cycle() {
    assert_eq!(Kind::ScriptCycle.id(), "script-cycle");
    let diag = Diagnostic::error(Kind::ScriptCycle)
        .at(Place::script("a.ld", 1, 7))
        .with(Relation::NamedBy, Place::script("b.ld", 2, 1));
    assert_eq!(text(Kind::ScriptCycle), "ld: error: linker script names itself\n");
    assert_eq!(
        diag.render(&Style::plain("ld")),
        "ld: error: a.ld:1:7: linker script names itself\n>>> named by b.ld:2:1\n"
    );
}

// spec: diag.md §3.1 `skipped-incompatible`, §8.3 (GNU ld's line)
#[test]
fn skipped_incompatible() {
    let kind = Kind::SkippedIncompatible { path: s("x64/libm.so"), search: s("-lm") };
    assert_eq!(kind.id(), "skipped-incompatible");
    assert_eq!(text(kind), "ld: error: skipping incompatible x64/libm.so when searching for -lm\n");
    let kind = Kind::SkippedIncompatible { path: s("sd/q.o"), search: s("q.o") };
    assert_eq!(
        Diagnostic::warning(kind).render(&Style::plain("ld")),
        "ld: warning: skipping incompatible sd/q.o when searching for q.o\n"
    );
}

// spec: diag.md §3.1 `static-link-of-shared-object`
#[test]
fn static_link_of_shared_object() {
    let kind = Kind::StaticLinkOfSharedObject { path: s("libc.so.6") };
    assert_eq!(kind.id(), "static-link-of-shared-object");
    assert_eq!(text(kind), "ld: error: attempted static link of dynamic object libc.so.6\n");
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

// spec: diag.md §3.1 `cet-report` (spec/x64.md X-43, R-11): the severity
// is the option's; one per (object, property)
#[test]
fn cet_report() {
    let kind = Kind::CetReport { path: s("an.o"), property: s("IBT") };
    assert_eq!(kind.id(), "cet-report");
    assert_eq!(kind.symbol(), None);
    assert_eq!(
        Diagnostic::warning(kind.clone()).render(&Style::plain("ld")),
        "ld: warning: an.o: missing IBT property (-z cet-report)\n"
    );
    assert_eq!(text(kind), "ld: error: an.o: missing IBT property (-z cet-report)\n");
    let shstk = Kind::CetReport { path: s("libx.a(y.o)"), property: s("SHSTK") };
    assert_eq!(text(shstk), "ld: error: libx.a(y.o): missing SHSTK property (-z cet-report)\n");
}

// spec: diag.md §3.1 `unknown-file-type`, §3.2 (emitted with a place)
// spec: diag.md §3.1 `unknown-property` (synth.md S-95, R-D2)
#[test]
fn unknown_property() {
    let kind = Kind::UnknownProperty { path: s("a.o"), pr_type: 0xc000_0002 };
    assert_eq!(kind.id(), "unknown-property");
    assert_eq!(
        text(kind),
        "ld: error: a.o: unsupported GNU_PROPERTY_TYPE 0xc0000002; property dropped\n"
    );
}

#[test]
fn unknown_file_type() {
    assert_eq!(Kind::UnknownFileType.id(), "unknown-file-type");
    assert_eq!(
        text(Kind::UnknownFileType),
        "ld: error: file not recognized: file format not recognized\n"
    );
    // At the file, the line is GNU ld's (§8.3).
    let diag = Diagnostic::error(Kind::UnknownFileType).at(FileRef::in_archive("libx.a", "y.o"));
    assert_eq!(
        diag.render(&Style::plain("ld")),
        "ld: error: libx.a(y.o): file not recognized: file format not recognized\n"
    );
}

// -- symbols ------------------------------------------------------------

/// The text of a symbol kind whose message is `{prefix}{name}{suffix}`.
fn symbol_text(kind: Kind) -> String {
    assert!(kind.symbol().is_some(), "{kind:?} names a symbol");
    text(kind)
}

// spec: diag.md §3.1 `archive-backref`
#[test]
fn archive_backref() {
    let kind = Kind::ArchiveBackref { name: s("helper") };
    assert_eq!(kind.id(), "archive-backref");
    assert_eq!(symbol_text(kind), "ld: error: backward reference to an archive member: helper\n");
}

// spec: diag.md §3.1 `archive-choice`
#[test]
fn archive_choice() {
    let kind = Kind::ArchiveChoice { name: s("f") };
    assert_eq!(kind.id(), "archive-choice");
    assert_eq!(symbol_text(kind), "ld: error: GNU ld would choose another definition of f\n");
}

// spec: diag.md §3.1 `comdat-mismatch`
#[test]
fn comdat_mismatch() {
    let kind = Kind::ComdatMismatch { name: s("x"), signature: s("_Z1gv") };
    assert_eq!(kind.id(), "comdat-mismatch");
    assert_eq!(
        symbol_text(kind),
        "ld: error: symbol x is defined only in a discarded copy of COMDAT group _Z1gv\n"
    );
}

// spec: diag.md §3.1 `common-alignment`
#[test]
fn common_alignment() {
    let kind = Kind::CommonAlignment { name: s("x") };
    assert_eq!(kind.id(), "common-alignment");
    assert_eq!(
        symbol_text(kind),
        "ld: error: definition of x is less aligned than its common symbols\n"
    );
}

// spec: diag.md §3.1 `common-larger-than-definition`
#[test]
fn common_larger_than_definition() {
    let kind = Kind::CommonLargerThanDefinition { name: s("x") };
    assert_eq!(kind.id(), "common-larger-than-definition");
    assert_eq!(symbol_text(kind), "ld: error: common symbol x is larger than its definition\n");
}

// spec: diag.md §3.1 `common-overridden`
#[test]
fn common_overridden() {
    let kind = Kind::CommonOverridden { name: s("x") };
    assert_eq!(kind.id(), "common-overridden");
    assert_eq!(symbol_text(kind), "ld: error: common symbol x is overridden by a definition\n");
}

// spec: diag.md §3.1 `discarded-section-reference`
#[test]
fn discarded_section_reference() {
    let kind = Kind::DiscardedSectionReference { name: s("g") };
    assert_eq!(kind.id(), "discarded-section-reference");
    assert_eq!(
        symbol_text(kind),
        "ld: error: relocation refers to a symbol in a discarded section: g\n"
    );
}

// spec: diag.md §3.1 `duplicate-symbol`
#[test]
fn duplicate_symbol() {
    let kind = Kind::DuplicateSymbol { name: s("twice") };
    assert_eq!(kind.id(), "duplicate-symbol");
    assert_eq!(
        text(kind.clone()),
        "ld: error: duplicate symbol: twice (multiple definition of `twice')\n"
    );
    let diag = Diagnostic::error(kind)
        .with(Relation::DefinedIn, Place::section(FileRef::new("dup1.o"), ".text", 0))
        .with(
            Relation::DefinedIn,
            Place::section(FileRef::in_archive("libdup.a", "dup2.o"), ".text", 0),
        );
    assert_eq!(
        diag.render(&Style::plain("ld")),
        "ld: error: duplicate symbol: twice (multiple definition of `twice')\n\
         >>> defined in dup1.o:(.text+0x0)\n\
         >>> defined in libdup.a(dup2.o):(.text+0x0)\n"
    );
}

// spec: diag.md §3.1 `entry-not-found`
#[test]
fn entry_not_found() {
    let kind = Kind::EntryNotFound { name: s("nosuch") };
    assert_eq!(kind.id(), "entry-not-found");
    assert_eq!(
        Diagnostic::warning(kind.clone()).render(&Style::plain("ld")),
        "ld: warning: cannot find entry symbol nosuch\n"
    );
    assert_eq!(symbol_text(kind), "ld: error: cannot find entry symbol nosuch\n");
}

// spec: diag.md §3.1 `gnu-warning` (the section's text, as it is)
#[test]
fn gnu_warning() {
    let kind = Kind::GnuWarning { text: s("the use of `tmpnam' is dangerous") };
    assert_eq!(kind.id(), "gnu-warning");
    assert_eq!(kind.symbol(), None);
    assert_eq!(text(kind), "ld: error: the use of `tmpnam' is dangerous\n");
}

// spec: diag.md §3.1 `linker-symbol-redefined`
#[test]
fn linker_symbol_redefined() {
    let kind = Kind::LinkerSymbolRedefined { name: s("__ehdr_start") };
    assert_eq!(kind.id(), "linker-symbol-redefined");
    assert_eq!(
        symbol_text(kind),
        "ld: error: cannot redefine linker-defined symbol: __ehdr_start\n"
    );
}

// spec: diag.md §3.1 `multiple-common`
#[test]
fn multiple_common() {
    let kind = Kind::MultipleCommon { name: s("x") };
    assert_eq!(kind.id(), "multiple-common");
    assert_eq!(symbol_text(kind), "ld: error: multiple common symbols: x\n");
}

// spec: diag.md §3.1 `non-exported-symbol`
#[test]
fn non_exported_symbol() {
    let kind = Kind::NonExportedSymbol { name: s("x") };
    assert_eq!(kind.id(), "non-exported-symbol");
    assert_eq!(
        symbol_text(kind),
        "ld: error: non-exported symbol referenced by a shared object: x\n"
    );
}

// spec: diag.md §3.1 `required-symbol`
#[test]
fn required_symbol() {
    let kind = Kind::RequiredSymbol { name: s("x") };
    assert_eq!(kind.id(), "required-symbol");
    assert_eq!(symbol_text(kind), "ld: error: required symbol not defined: x\n");
}

// spec: diag.md §3.1 `tls-mismatch`
#[test]
fn tls_mismatch() {
    let kind = Kind::TlsMismatch { name: s("t") };
    assert_eq!(kind.id(), "tls-mismatch");
    assert_eq!(symbol_text(kind), "ld: error: TLS attribute mismatch: t\n");
}

// spec: diag.md §3.1 `undefined-symbol`, §3.2 (one per key, its references
// as context; GNU ld's phrase), §8.3
#[test]
fn undefined_symbol() {
    let kind = Kind::UndefinedSymbol { name: s("missing") };
    assert_eq!(kind.id(), "undefined-symbol");
    assert_eq!(
        text(kind.clone()),
        "ld: error: undefined symbol: missing (undefined reference to `missing')\n"
    );
    let diag = Diagnostic::error(kind.clone())
        .with(
            Relation::ReferencedBy,
            Place::section(FileRef::in_archive("libu.a", "undef.o"), ".text", 0xc),
        )
        .with(Relation::ReferencedBy, Place::section(FileRef::new("b.o"), ".text", 0x4))
        .note("referenced 2 more times");
    assert_eq!(
        diag.render(&Style::plain("ld.mortar")),
        "ld.mortar: error: undefined symbol: missing (undefined reference to `missing')\n\
         >>> referenced by libu.a(undef.o):(.text+0xc)\n\
         >>> referenced by b.o:(.text+0x4)\n\
         >>> referenced 2 more times\n"
    );
    // The phrase build tools search for is in the line, as GNU ld prints it.
    assert!(diag.render(&Style::plain("ld")).contains("undefined reference to `missing'"));
    // Under --warn-unresolved-symbols the same line is a warning.
    assert_eq!(
        Diagnostic::warning(kind).render(&Style::plain("ld")),
        "ld: warning: undefined symbol: missing (undefined reference to `missing')\n"
    );
}

// spec: diag.md §3.1 `undefined-version`
#[test]
fn undefined_version() {
    let kind = Kind::UndefinedVersion { name: s("foo"), version: s("V2") };
    assert_eq!(kind.id(), "undefined-version");
    assert_eq!(symbol_text(kind), "ld: error: version V2 of foo is not defined\n");
}

// spec: diag.md §3.1 `version-reassigned`
#[test]
fn version_reassigned() {
    let kind = Kind::VersionReassigned { name: s("foo"), first: s("V1"), second: s("V2") };
    assert_eq!(kind.id(), "version-reassigned");
    assert_eq!(symbol_text(kind), "ld: error: foo is assigned to version V1 and V2; V1 is kept\n");
}

// spec: diag.md §3.1 `versioned-export` (dyn.md D-27)
#[test]
fn versioned_export() {
    let kind = Kind::VersionedExport { name: s("foo"), version: s("V2") };
    assert_eq!(kind.id(), "versioned-export");
    assert_eq!(
        symbol_text(kind),
        "ld: error: versioned definition foo@@V2 cannot be exported (version definitions are \
         not supported)\n"
    );
}

// -- one-definition rule ------------------------------------------------

// spec: diag.md §3.1, the one-definition rule's kinds that name a symbol
#[test]
fn odr_kinds_with_a_symbol() {
    type Row = (fn(String) -> Kind, &'static str, &'static str);
    let rows: [Row; 4] = [
        (
            |name| Kind::OdrContentsMismatch { name },
            "odr-contents-mismatch",
            "x has different contents in different files",
        ),
        (
            |name| Kind::OdrSizeMismatch { name },
            "odr-size-mismatch",
            "x has different sizes in different files",
        ),
        (
            |name| Kind::OdrStrongAndComdat { name },
            "odr-strong-and-comdat",
            "x has a non-inline definition and an inline copy",
        ),
        (
            |name| Kind::OdrSymbolTypeMismatch { name },
            "odr-symbol-type-mismatch",
            "x is a function in one file and data in another",
        ),
    ];
    for (make, id, message) in rows {
        let kind = make(s("x"));
        assert_eq!(kind.id(), id);
        assert_eq!(kind.symbol(), Some("x"));
        assert_eq!(symbol_text(kind), format!("ld: error: {message}\n"));
    }
}

// spec: diag.md §3.1 `canonical-plt-in-pie` (dyn.md D-44)
#[test]
fn canonical_plt_in_pie() {
    let kind = Kind::CanonicalPltInPie { r_type: s("R_AARCH64_ADR_PREL_PG_HI21"), name: s("puts") };
    assert_eq!(kind.id(), "canonical-plt-in-pie");
    assert_eq!(
        symbol_text(kind),
        "ld: error: relocation R_AARCH64_ADR_PREL_PG_HI21 against symbol `puts' cannot be used \
         in a position-independent executable; recompile with -fPIE, or link with -no-pie\n"
    );
}

// spec: diag.md §3.1 `copy-relocation-against-protected` (dyn.md D-18)
#[test]
fn copy_relocation_against_protected() {
    let kind = Kind::CopyRelocationAgainstProtected { name: s("pv"), file: s("d/libprot.so") };
    assert_eq!(kind.id(), "copy-relocation-against-protected");
    assert_eq!(
        symbol_text(kind),
        "ld: error: copy relocation against non-copyable protected symbol `pv' in d/libprot.so\n"
    );
}

// spec: diag.md §3.1 `copy-relocation-of-zero-size` (dyn.md D-42)
#[test]
fn copy_relocation_of_zero_size() {
    let kind = Kind::CopyRelocationOfZeroSize { name: s("zsym") };
    assert_eq!(kind.id(), "copy-relocation-of-zero-size");
    assert_eq!(symbol_text(kind), "ld: error: cannot create a copy relocation for symbol zsym\n");
}

// spec: diag.md §3.1 `text-relocation` (dyn.md D-47)
#[test]
fn text_relocation() {
    let kind = Kind::TextRelocation { r_type: s("R_X86_64_32S"), name: s("ro_pfa") };
    assert_eq!(kind.id(), "text-relocation");
    assert_eq!(
        symbol_text(kind),
        "ld: error: relocation R_X86_64_32S against symbol `ro_pfa' can not be used when making \
         a PIE object; recompile with -fPIE\n"
    );
}

// spec: diag.md §3.1 `weak-undefined-pc-in-pie` (dyn.md D-25)
#[test]
fn weak_undefined_pc_in_pie() {
    let kind = Kind::WeakUndefinedPcInPie { r_type: s("R_X86_64_PC32"), name: s("v") };
    assert_eq!(kind.id(), "weak-undefined-pc-in-pie");
    assert_eq!(
        symbol_text(kind),
        "ld: error: relocation R_X86_64_PC32 against undefined weak symbol `v' cannot be used in \
         a position-independent executable; recompile with -fPIE\n"
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
    assert_eq!(Kind::NeededNotFound { name: s("libm.so"), needed_by: s("a.so") }.symbol(), None);
    assert_eq!(
        Kind::ComdatMismatch { name: s("_Z1kv"), signature: s("g") }.symbol(),
        Some("_Z1kv")
    );
    assert_eq!(
        Kind::VersionReassigned { name: s("_Z1lv"), first: s("V1"), second: s("V2") }.symbol(),
        Some("_Z1lv")
    );
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

// spec: diag.md §3.1 `relocation-out-of-range` (GNU ld's phrase, then
// lld's value and bounds in decimal, negative ones signed; the target's
// name through the hook)
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
        "ld: error: relocation truncated to fit: R_AARCH64_CONDBR19 against 'far_away': \
         -1048580 is not in [-1048576, 1048575]\n"
    );
    let diag = Diagnostic::error(kind).at(Place::section(FileRef::new("b.o"), ".text", 0x20));
    assert_eq!(
        diag.render(&Style::plain("ld")),
        "ld: error: b.o:(.text+0x20): relocation truncated to fit: R_AARCH64_CONDBR19 against \
         'far_away': -1048580 is not in [-1048576, 1048575]\n"
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
        "ld: error: relocation truncated to fit: R_X86_64_PC32 against '.rodata+0x40': \
         2147483648 is not in [-2147483648, 2147483647]\n"
    );
}

// -- layout -------------------------------------------------------------

// spec: diag.md §3.1 `executable-stack` (GNU ld's words; a warning, layout.md L-84)
#[test]
fn executable_stack() {
    let kind = Kind::ExecutableStack { file: s("libx.a(y.o)") };
    assert_eq!(kind.id(), "executable-stack");
    assert_eq!(
        Diagnostic::warning(kind).render(&Style::plain("ld")),
        "ld: warning: libx.a(y.o): requires executable stack (because the .note.GNU-stack \
         section is executable)\n"
    );
}

// spec: diag.md §3.1 `output-too-large` (layout.md L-73)
#[test]
fn output_too_large() {
    let kind = Kind::OutputTooLarge { what: s("chunk 7"), limit: s("0x100000000") };
    assert_eq!(kind.id(), "output-too-large");
    assert_eq!(text(kind), "ld: error: output too large: chunk 7 exceeds 0x100000000\n");
}

// spec: diag.md §3.1 `section-conflict` (layout.md L-47, L-49, L-61)
#[test]
fn section_conflict() {
    let kind = Kind::SectionConflict { name: s(".tdata"), first: s("TLS"), second: s("non-TLS") };
    assert_eq!(kind.id(), "section-conflict");
    assert_eq!(
        text(kind),
        "ld: error: section .tdata: TLS and non-TLS inputs cannot be combined\n"
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

// -- the wording policy -------------------------------------------------

// spec: diag.md §8.3 (each GNU ld phrase of the table is in the message of
// every kind the table names for it)
#[test]
fn wording_policy_phrases() {
    let at =
        |kind: Kind| Diagnostic::error(kind).at(Place::file("f.o")).render(&Style::plain("ld"));
    let rows: [(&str, Vec<String>); 10] = [
        ("undefined reference to `x'", vec![at(Kind::UndefinedSymbol { name: s("x") })]),
        ("multiple definition of `x'", vec![at(Kind::DuplicateSymbol { name: s("x") })]),
        ("cannot find -lm", vec![at(Kind::LibraryNotFound { name: s("m") })]),
        (
            "skipping incompatible p/libm.a when searching for -lm",
            vec![at(Kind::SkippedIncompatible { path: s("p/libm.a"), search: s("-lm") })],
        ),
        (
            "file not recognized: file format not recognized",
            vec![
                at(Kind::UnknownFileType),
                Diagnostic::refused_unrecognized_format("Mach-O").render(&Style::plain("ld")),
            ],
        ),
        (
            "No such file or directory",
            vec![
                at(Kind::LibraryNotFound { name: s("m") }),
                at(Kind::CannotOpenFile {
                    path: s("a.o"),
                    reason: os_reason(&std::io::Error::from_raw_os_error(2)),
                }),
            ],
        ),
        (
            "attempted static link of dynamic object",
            vec![at(Kind::StaticLinkOfSharedObject { path: s("l.so") })],
        ),
        (
            "needed by l.so, not found (try using -rpath or -rpath-link)",
            vec![at(Kind::NeededNotFound { name: s("libz.so.1"), needed_by: s("l.so") })],
        ),
        ("cannot find entry symbol x", vec![at(Kind::EntryNotFound { name: s("x") })]),
        (
            "relocation truncated to fit: R_X86_64_PC32 against",
            vec![at(Kind::RelocationOutOfRange {
                r_type: s("R_X86_64_PC32"),
                value: 1 << 32,
                min: -(1 << 31),
                max: (1 << 31) - 1,
                name: s("far"),
            })],
        ),
    ];
    for (phrase, messages) in rows {
        for message in messages {
            assert!(message.contains(phrase), "{message:?} lacks {phrase:?}");
        }
    }
}

//! The text form (`spec/diag.md` §2.3, §2.4, §5).

use std::sync::atomic::{AtomicUsize, Ordering};

use mortar_diag::{
    Collector, Diagnostic, FileRef, IncompatibleReason, Key, Kind, Place, Policy, RefusalClass,
    Relation, Style, no_demangling,
};

fn plain(diagnostic: &Diagnostic) -> String {
    diagnostic.render(&Style::plain("ld"))
}

fn undefined(name: &str) -> Diagnostic {
    Diagnostic::error(Kind::UndefinedSymbol { name: name.to_owned() })
}

/// A stand-in for a demangler: two names it knows.
fn toy_demangler(name: &str) -> Option<String> {
    match name {
        "_ZN2ns1fEic" => Some("ns::f(int, char)".to_owned()),
        "_ZNK2ns1S1mEv" => Some("ns::S::m() const".to_owned()),
        _ => None,
    }
}

fn demangled(diagnostic: &Diagnostic) -> String {
    diagnostic.render(&Style { prog: "ld", color: false, demangle: &toy_demangler })
}

// spec: diag.md §2.3 (each form, and a file inside the section and symbol
// forms is either file form)
#[test]
fn place_forms() {
    let object = FileRef::new("dir/foo.o");
    let member = FileRef::in_archive("/usr/lib/libx.a", "y.o");
    let text = |place: Place| {
        let line = plain(&Diagnostic::error(Kind::NoInputFiles).with(Relation::In, place));
        line.strip_prefix("ld: error: no input files\n>>> in ")
            .and_then(|rest| rest.strip_suffix('\n'))
            .expect("one context line")
            .to_owned()
    };
    assert_eq!(text(Place::File(object.clone())), "dir/foo.o");
    assert_eq!(text(Place::file("dir/foo.o")), "dir/foo.o");
    assert_eq!(text(member.clone().into()), "/usr/lib/libx.a(y.o)");
    assert_eq!(text(Place::section(object.clone(), ".text", 0x10)), "dir/foo.o:(.text+0x10)");
    assert_eq!(
        text(Place::section(object.clone(), ".data.rel.ro", 0)),
        "dir/foo.o:(.data.rel.ro+0x0)"
    );
    assert_eq!(
        text(Place::section(member.clone(), ".text.startup", 0xdead_beef_0123)),
        "/usr/lib/libx.a(y.o):(.text.startup+0xdeadbeef0123)"
    );
    assert_eq!(
        text(Place::section(object.clone(), ".bss", u64::MAX)),
        "dir/foo.o:(.bss+0xffffffffffffffff)"
    );
    assert_eq!(text(Place::symbol(object, "main")), "dir/foo.o:(main)");
    assert_eq!(text(Place::symbol(member, "_start")), "/usr/lib/libx.a(y.o):(_start)");
    assert_eq!(text(Place::argument("--entry=main")), "argument '--entry=main'");
    assert_eq!(text(Place::argument("-e main")), "argument '-e main'");
    assert_eq!(text(Place::script("/usr/lib/libc.so", 5, 9)), "/usr/lib/libc.so:5:9");
    assert_eq!(text(Place::script("v.map", 1, 1)), "v.map:1:1");
}

// spec: diag.md §2.3 (`FileRef::from_path` replaces bytes that are not
// UTF-8 with U+FFFD)
#[cfg(unix)]
#[test]
fn path_that_is_not_utf8() {
    use std::os::unix::ffi::OsStrExt;
    let path = std::path::Path::new(std::ffi::OsStr::from_bytes(b"caf\xe9.o"));
    let file = FileRef::from_path(path);
    assert_eq!(file.path(), "caf\u{fffd}.o");
    assert_eq!(
        plain(&Diagnostic::error(Kind::UnknownFileType).at(file)),
        "ld: error: caf\u{fffd}.o: file not recognized: file format not recognized\n"
    );
    assert_eq!(FileRef::from_path(std::path::Path::new("a/b.o")), FileRef::new("a/b.o"));
}

// spec: diag.md §2.4 (the words of each relation; notes; lines in the
// order the caller added them)
#[test]
fn relations_and_notes_in_the_order_given() {
    let diag = Diagnostic::error(Kind::DuplicateSymbol { name: "x".to_owned() })
        .with(Relation::ReferencedBy, Place::file("r.o"))
        .note("a fact")
        .with(Relation::DefinedIn, Place::file("d.o"))
        .with(Relation::NamedBy, Place::argument("-lx"))
        .with(Relation::In, Place::file("i.o"))
        .note("another fact")
        .with(Relation::DefinedIn, Place::file("a.o"));
    assert_eq!(
        plain(&diag),
        "ld: error: duplicate symbol: x (multiple definition of `x')\n\
         >>> referenced by r.o\n\
         >>> a fact\n\
         >>> defined in d.o\n\
         >>> named by argument '-lx'\n\
         >>> in i.o\n\
         >>> another fact\n\
         >>> defined in a.o\n"
    );
    assert_eq!(Relation::DefinedIn.phrase(), "defined in");
    assert_eq!(Relation::In.phrase(), "in");
    assert_eq!(Relation::NamedBy.phrase(), "named by");
    assert_eq!(Relation::ReferencedBy.phrase(), "referenced by");
}

// spec: diag.md §5.1 (the program name is the caller's), §5.3 (the place
// starts the head line)
#[test]
fn head_line_place() {
    let diag = Diagnostic::warning(Kind::Malformed { what: "odd size".to_owned() });
    for prog in ["mortar", "ld", "ld.mortar", "aarch64-linux-gnu-ld"] {
        assert_eq!(diag.render(&Style::plain(prog)), format!("{prog}: warning: odd size\n"));
    }
    let file = FileRef::in_archive("libx.a", "y.o");
    let places = [
        (Place::File(file.clone()), "libx.a(y.o)"),
        (Place::section(file.clone(), ".symtab", 0x48), "libx.a(y.o):(.symtab+0x48)"),
        (Place::symbol(file, "f"), "libx.a(y.o):(f)"),
        (Place::argument("@rsp"), "argument '@rsp'"),
        (Place::script("x.ld", 3, 14), "x.ld:3:14"),
    ];
    for (place, text) in places {
        assert_eq!(plain(&diag.clone().at(place)), format!("ld: warning: {text}: odd size\n"));
    }
}

// spec: diag.md §5.4 (`None`: the name as it is)
#[test]
fn demangling_none_prints_the_name() {
    let diag = undefined("_ZN2ns1fEic")
        .with(Relation::ReferencedBy, Place::symbol(FileRef::new("a.o"), "_ZNK2ns1S1mEv"));
    let expected = "ld: error: undefined symbol: _ZN2ns1fEic (undefined reference to `_ZN2ns1fEic')\n\
                    >>> referenced by a.o:(_ZNK2ns1S1mEv)\n";
    assert_eq!(plain(&diag), expected);
    assert_eq!(
        diag.render(&Style { prog: "ld", color: false, demangle: &no_demangling }),
        expected
    );
    assert_eq!(no_demangling("_Z1fv"), None);
    // A name the hook does not know.
    assert_eq!(
        demangled(&undefined("main")),
        "ld: error: undefined symbol: main (undefined reference to `main')\n"
    );
}

// spec: diag.md §5.4 (`Some` of the name itself is no demangling)
#[test]
fn demangling_to_the_same_name_adds_no_line() {
    let echo = |name: &str| Some(name.to_owned());
    let diag = undefined("printf").with(Relation::ReferencedBy, Place::file("a.o"));
    assert_eq!(
        diag.render(&Style { prog: "ld", color: false, demangle: &echo }),
        "ld: error: undefined symbol: printf (undefined reference to `printf')\n>>> referenced by a.o\n"
    );
}

// spec: diag.md §5.4 (the shown name, then the mangled name as the first
// context line; symbol places get no such line)
#[test]
fn demangling_shows_the_mangled_name() {
    let diag = undefined("_ZN2ns1fEic")
        .with(Relation::ReferencedBy, Place::section(FileRef::new("cxx.o"), ".text", 0x18))
        .with(Relation::ReferencedBy, Place::symbol(FileRef::new("cxx.o"), "_ZNK2ns1S1mEv"));
    assert_eq!(
        demangled(&diag),
        "ld: error: undefined symbol: ns::f(int, char) (undefined reference to `ns::f(int, char)')\n\
         >>> mangled name: _ZN2ns1fEic\n\
         >>> referenced by cxx.o:(.text+0x18)\n\
         >>> referenced by cxx.o:(ns::S::m() const)\n"
    );
    let duplicate = Diagnostic::warning(Kind::DuplicateSymbol { name: "_ZNK2ns1S1mEv".to_owned() })
        .with(Relation::DefinedIn, Place::file("a.o"));
    assert_eq!(
        demangled(&duplicate),
        "ld: warning: duplicate symbol: ns::S::m() const (multiple definition of \
         `ns::S::m() const')\n\
         >>> mangled name: _ZNK2ns1S1mEv\n\
         >>> defined in a.o\n"
    );
    // Only symbols go through the hook: a refused construct, a file or a
    // section that happens to be spelled like one does not.
    let other = Diagnostic::refused(RefusalClass::Feature, "_ZN2ns1fEic")
        .at(Place::section(FileRef::new("_ZN2ns1fEic"), "_ZN2ns1fEic", 0))
        .note("_ZN2ns1fEic");
    assert_eq!(demangled(&other), plain(&other));
}

// spec: diag.md §5.4 (called at rendering time only, once per occurrence)
#[test]
fn demangling_hook_is_called_only_when_rendering() {
    let calls = AtomicUsize::new(0);
    let counting = |name: &str| {
        calls.fetch_add(1, Ordering::Relaxed);
        toy_demangler(name)
    };
    let collector = Collector::new(Policy::default());
    collector.emit(
        Key::new(0, 0, 0),
        undefined("_ZN2ns1fEic")
            .with(Relation::ReferencedBy, Place::symbol(FileRef::new("a.o"), "_ZNK2ns1S1mEv")),
    );
    collector.emit(Key::new(0, 0, 0), undefined("_ZN2ns1fEic"));
    collector.emit(Key::new(0, 0, 1), Diagnostic::error(Kind::NoInputFiles));
    let report = collector.finish();
    assert_eq!(calls.load(Ordering::Relaxed), 0, "collecting and ordering do not demangle");
    let text = report.render(&Style { prog: "ld", color: false, demangle: &counting });
    // The name twice in each message (lld's head, GNU ld's phrase), once in
    // the symbol place.
    assert_eq!(calls.load(Ordering::Relaxed), 5);
    assert_eq!(text.matches("ns::f(int, char)").count(), 4);
}

// spec: diag.md §5.5
#[test]
fn control_characters_are_made_visible() {
    let hostile = "a\nld: error: forged\x1b[2J\t\r\0\u{7f}\u{85}b";
    let shown = "a\\u{a}ld: error: forged\\u{1b}[2J\\u{9}\\u{d}\\u{0}\\u{7f}\\u{85}b";
    let file = FileRef::in_archive(hostile, hostile);
    let diag = undefined(hostile)
        .at(Place::section(file.clone(), hostile, 1))
        .with(Relation::ReferencedBy, Place::symbol(file, hostile))
        .with(Relation::NamedBy, Place::argument(hostile))
        .with(Relation::In, Place::script(hostile, 1, 2))
        .note(hostile);
    let echo_hostile = |_: &str| Some(format!("demangled\n{hostile}"));
    let text = diag.render(&Style { prog: hostile, color: false, demangle: &echo_hostile });
    assert_eq!(
        text,
        format!(
            "{shown}: error: {shown}({shown}):({shown}+0x1): undefined symbol: demangled\\u{{a}}{shown} (undefined reference to `demangled\\u{{a}}{shown}')\n\
             >>> mangled name: {shown}\n\
             >>> referenced by {shown}({shown}):(demangled\\u{{a}}{shown})\n\
             >>> named by argument '{shown}'\n\
             >>> in {shown}:1:2\n\
             >>> {shown}\n"
        )
    );
    // The fields of the other kinds.
    let h = || hostile.to_owned();
    for (kind, expected) in [
        (
            Kind::InvalidOptionValue { option: h(), value: h() },
            format!("{shown}: invalid value '{shown}'"),
        ),
        (Kind::MissingArgument { option: h() }, format!("{shown}: missing argument")),
        (Kind::UnknownOption { option: h() }, format!("unknown argument '{shown}'")),
        (Kind::CannotOpenFile { path: h(), reason: h() }, format!("cannot open {shown}: {shown}")),
        (
            Kind::CannotOpenOutput { path: h(), reason: h() },
            format!("cannot open output file {shown}: {shown}"),
        ),
        (
            Kind::LibraryNotFound { name: h() },
            format!("cannot find -l{shown}: No such file or directory"),
        ),
        (Kind::Malformed { what: h() }, shown.to_owned()),
        (
            Kind::DuplicateSymbol { name: h() },
            format!("duplicate symbol: {shown} (multiple definition of `{shown}')"),
        ),
        (
            Kind::RelocationIncompatible {
                r_type: h(),
                name: h(),
                reason: IncompatibleReason::ThreadLocal,
            },
            format!(
                "relocation {shown} cannot be used against symbol '{shown}'; it is thread-local"
            ),
        ),
        (
            Kind::RelocationMisaligned { r_type: h(), value: -8, align: 16 },
            format!("improper alignment for relocation {shown}: -8 is not aligned to 16 bytes"),
        ),
        (
            Kind::RelocationOutOfRange { r_type: h(), value: 9, min: -1, max: 8, name: h() },
            format!("relocation {shown} out of range: 9 is not in [-1, 8]; references '{shown}'"),
        ),
        (
            Kind::ExecutableStack { file: h() },
            format!(
                "{shown}: requires executable stack (because the .note.GNU-stack section is \
                 executable)"
            ),
        ),
        (
            Kind::OutputTooLarge { what: h(), limit: h() },
            format!("output too large: {shown} exceeds {shown}"),
        ),
        (
            Kind::SectionConflict { name: h(), first: h(), second: h() },
            format!("section {shown}: {shown} and {shown} inputs cannot be combined"),
        ),
        (
            Kind::Refused { class: RefusalClass::SectionType, name: h(), not_recognized: false },
            format!("unsupported section type: {shown}"),
        ),
        (Kind::InternalError { what: h() }, format!("internal error in mortar: {shown}")),
    ] {
        assert_eq!(plain(&Diagnostic::error(kind)), format!("ld: error: {expected}\n"));
    }
    // Text that is not a control character is not touched: a backslash,
    // quotes, non-ASCII letters, a space.
    let calm = "d\u{e9}j\u{e0} vu\\u{a} 'x' \"y\" \u{4e2d}\u{6587}";
    assert_eq!(
        plain(&Diagnostic::error(Kind::Malformed { what: calm.to_owned() })),
        format!("ld: error: {calm}\n")
    );
}

/// One diagnostic of every kind, plain and with every kind of context,
/// and with fields that try to break the line structure.
fn samples() -> Vec<Diagnostic> {
    let s = |text: &str| text.to_owned();
    let kinds = |x: &str| {
        vec![
            Kind::InvalidOptionValue { option: s(x), value: s(x) },
            Kind::MissingArgument { option: s(x) },
            Kind::NoInputFiles,
            Kind::UnknownOption { option: s(x) },
            Kind::CannotOpenFile { path: s(x), reason: s(x) },
            Kind::CannotOpenOutput { path: s(x), reason: s(x) },
            Kind::AmbiguousSysrootPath { spelling: s(x) },
            Kind::DuplicateSoname { soname: s(x) },
            Kind::IgnoredMember { what: s(x) },
            Kind::IncompatibleFile { expected: s(x), found: s(x) },
            Kind::InputChanged,
            Kind::LibraryNotFound { name: s(x) },
            Kind::NeededNotFound { name: s(x), needed_by: s(x) },
            Kind::NoTarget,
            Kind::ScriptCycle,
            Kind::SkippedIncompatible { path: s(x), search: s(x) },
            Kind::StaticLinkOfSharedObject { path: s(x) },
            Kind::Malformed { what: s(x) },
            Kind::UnknownFileType,
            Kind::ArchiveBackref { name: s(x) },
            Kind::ArchiveChoice { name: s(x) },
            Kind::ComdatMismatch { name: s(x), signature: s(x) },
            Kind::CommonAlignment { name: s(x) },
            Kind::CommonLargerThanDefinition { name: s(x) },
            Kind::CommonOverridden { name: s(x) },
            Kind::DiscardedSectionReference { name: s(x) },
            Kind::DuplicateSymbol { name: s(x) },
            Kind::EntryNotFound { name: s(x) },
            Kind::GnuWarning { text: s(x) },
            Kind::LinkerSymbolRedefined { name: s(x) },
            Kind::MultipleCommon { name: s(x) },
            Kind::NonExportedSymbol { name: s(x) },
            Kind::RequiredSymbol { name: s(x) },
            Kind::TlsMismatch { name: s(x) },
            Kind::UndefinedSymbol { name: s(x) },
            Kind::UndefinedVersion { name: s(x), version: s(x) },
            Kind::VersionReassigned { name: s(x), first: s(x), second: s(x) },
            Kind::RelocationIncompatible {
                r_type: s(x),
                name: s(x),
                reason: IncompatibleReason::NonAlloc,
            },
            Kind::RelocationMisaligned { r_type: s(x), value: i64::MIN, align: u64::MAX },
            Kind::RelocationOutOfRange {
                r_type: s(x),
                value: i64::MAX,
                min: i64::MIN,
                max: 0,
                name: s(x),
            },
            Kind::ExecutableStack { file: s(x) },
            Kind::OutputTooLarge { what: s(x), limit: s(x) },
            Kind::SectionConflict { name: s(x), first: s(x), second: s(x) },
            Kind::Refused { class: RefusalClass::RelocationType, name: s(x), not_recognized: true },
            Kind::InternalError { what: s(x) },
        ]
    };
    let mut all = Vec::new();
    for x in ["_ZN2ns1fEic", "", "two\nlines", "\n>>> forged", "\r\nld: error: forged\n\n", ">>> x"]
    {
        for kind in kinds(x) {
            assert!(Kind::IDS.contains(&kind.id()));
            let file = FileRef::in_archive(x, x);
            all.push(Diagnostic::error(kind.clone()));
            all.push(Diagnostic::warning(kind.clone()).at(Place::section(file.clone(), x, 7)));
            all.push(
                Diagnostic::error(kind)
                    .at(Place::script(x, 1, 1))
                    .with(Relation::ReferencedBy, Place::symbol(file.clone(), x))
                    .with(Relation::DefinedIn, Place::File(file))
                    .with(Relation::NamedBy, Place::argument(x))
                    .note(x),
            );
        }
    }
    all
}

// spec: diag.md §5.2 (properties 1 to 3)
#[test]
fn every_line_is_a_head_or_a_context_line() {
    let samples = samples();
    assert!(samples.len() >= 3 * Kind::IDS.len());
    let style = Style { prog: "ld", color: false, demangle: &toy_demangler };
    for diag in &samples {
        let text = diag.render(&style);
        assert_eq!(text, diag.render(&style), "the same value renders to the same bytes");
        assert!(text.ends_with('\n'), "{text:?}");
        let lines: Vec<&str> = text.split_terminator('\n').collect();
        let head = format!("ld: {}: ", diag.severity().word());
        assert!(lines[0].starts_with(&head), "{text:?}");
        for line in &lines[1..] {
            assert!(line.starts_with(">>> "), "{text:?}");
        }
        let mangled_line = usize::from(text.contains("\n>>> mangled name: "));
        assert_eq!(lines.len(), 1 + mangled_line + diag.context().len(), "{text:?}");
    }
    // A whole report: as many head lines as diagnostics, no blank line.
    let collector = Collector::new(Policy { error_limit: 0, ..Policy::default() });
    for (index, diag) in samples.iter().enumerate() {
        collector.emit(Key::new(0, 0, index as u64), diag.clone());
    }
    let text = collector.finish().render(&style);
    let heads = text.lines().filter(|line| !line.starts_with(">>> ")).count();
    assert_eq!(heads, samples.len());
    assert!(text.lines().all(|l| l.starts_with("ld: error: ")
        || l.starts_with("ld: warning: ")
        || l.starts_with(">>> ")));
    assert!(!text.contains("\n\n") && !text.starts_with('\n'));
}

// spec: diag.md §5.6 (no escape sequence without color)
#[test]
fn color_off_has_no_escape_sequence() {
    for diag in samples() {
        assert!(!diag.render(&Style::plain("ld")).contains('\x1b'));
    }
}

// spec: diag.md §5.6 (the exact bytes)
#[test]
fn color_on_exact_bytes() {
    let colored = |diag: &Diagnostic| {
        diag.render(&Style { prog: "ld", color: true, demangle: &toy_demangler })
    };
    let error = undefined("_ZN2ns1fEic").with(Relation::ReferencedBy, Place::file("a.o"));
    assert_eq!(
        colored(&error).as_bytes(),
        b"ld: \x1b[0;31merror:\x1b[0m undefined symbol: ns::f(int, char) (undefined reference to `ns::f(int, char)')\n\
          >>> mangled name: _ZN2ns1fEic\n\
          >>> referenced by a.o\n"
    );
    let warning =
        Diagnostic::warning(Kind::Malformed { what: "odd size".to_owned() }).at(Place::file("a.o"));
    assert_eq!(colored(&warning).as_bytes(), b"ld: \x1b[0;35mwarning:\x1b[0m a.o: odd size\n");
}

// spec: diag.md §5.6 (the limit line is colored as an error; removing the
// sequences gives the uncolored rendering)
#[test]
fn color_stripped_is_the_plain_text() {
    let collector = Collector::new(Policy { error_limit: 2, ..Policy::default() });
    for (index, diag) in samples().into_iter().enumerate() {
        collector.emit(Key::new(0, 0, index as u64), diag);
    }
    let report = collector.finish();
    assert!(report.truncated());
    let plain = report.render(&Style::plain("ld"));
    let colored = report.render(&Style { prog: "ld", color: true, demangle: &no_demangling });
    assert_ne!(colored, plain);
    assert!(colored.ends_with(
        "ld: \x1b[0;31merror:\x1b[0m too many errors emitted, stopping now \
         (use --error-limit=0 to see all errors)\n"
    ));
    let stripped =
        colored.replace("\x1b[0;31m", "").replace("\x1b[0;35m", "").replace("\x1b[0m", "");
    assert_eq!(stripped, plain);
    let heads = plain.lines().filter(|line| !line.starts_with(">>> ")).count();
    assert_eq!(colored.matches("\x1b[0m").count(), heads);
    assert_eq!(colored.matches('\x1b').count(), 2 * heads);
}

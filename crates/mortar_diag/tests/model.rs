//! The model (`spec/diag.md` §2).

use mortar_diag::{
    Collector, Context, Diagnostic, FileRef, Key, Kind, Place, Policy, RefusalClass, Relation,
    Report, Severity, Style,
};

// spec: diag.md §2.5 (a diagnostic is `Send`, `Sync` and owns its data),
// §6.6 (the collector and the report)
const _: () = {
    const fn shared_between_threads<T: Send + Sync + 'static>() {}
    const fn shared_while_borrowing<T: Send + Sync>() {}
    shared_between_threads::<Diagnostic>();
    shared_between_threads::<Kind>();
    shared_between_threads::<Place>();
    shared_between_threads::<FileRef>();
    shared_between_threads::<Context>();
    shared_between_threads::<Key>();
    shared_between_threads::<Policy>();
    shared_between_threads::<Collector>();
    shared_between_threads::<Report>();
    shared_while_borrowing::<Style<'_>>();
};

// spec: diag.md §2.5, §6.6. The assertions above are checked when this
// file compiles; this test says so in the test list and moves real values.
#[test]
fn types_are_send_and_sync() {
    let collector = Collector::default();
    let diagnostic = Diagnostic::error(Kind::NoInputFiles);
    std::thread::scope(|scope| {
        let collector = &collector;
        scope.spawn(move || collector.emit(Key::new(0, 0, 0), diagnostic));
    });
    let report = std::thread::spawn(move || collector.finish()).join().expect("no panic");
    assert_eq!(report.diagnostics().len(), 1);
}

// spec: diag.md §2.1
#[test]
fn severity_words_and_order() {
    assert_eq!(Severity::Error.word(), "error");
    assert_eq!(Severity::Warning.word(), "warning");
    assert!(Severity::Warning < Severity::Error);
}

// spec: diag.md §2.5
#[test]
fn builder_and_accessors() {
    let file = FileRef::in_archive("libx.a", "y.o");
    assert_eq!(file.path(), "libx.a");
    assert_eq!(file.member(), Some("y.o"));
    assert_eq!(FileRef::new("foo.o").member(), None);

    let diag = Diagnostic::warning(Kind::UndefinedSymbol { name: "f".to_owned() })
        .at(file.clone())
        .with(Relation::ReferencedBy, Place::section(file.clone(), ".text", 0x10))
        .note("first of 3")
        .with(Relation::DefinedIn, Place::file("z.o"));
    assert_eq!(diag.severity(), Severity::Warning);
    assert_eq!(diag.id(), "undefined-symbol");
    assert_eq!(diag.kind(), &Kind::UndefinedSymbol { name: "f".to_owned() });
    assert_eq!(diag.kind().symbol(), Some("f"));
    assert_eq!(diag.refusal(), None);
    assert_eq!(diag.place(), Some(&Place::File(file.clone())));
    assert_eq!(
        diag.context(),
        [
            Context::At(
                Relation::ReferencedBy,
                Place::Section { file, section: ".text".to_owned(), offset: 0x10 }
            ),
            Context::Note("first of 3".to_owned()),
            Context::At(Relation::DefinedIn, Place::File(FileRef::new("z.o"))),
        ]
    );

    let bare = Diagnostic::error(Kind::NoInputFiles);
    assert_eq!(bare.severity(), Severity::Error);
    assert_eq!(bare.place(), None);
    assert!(bare.context().is_empty());
    assert_eq!(Diagnostic::new(Severity::Error, Kind::NoInputFiles), bare);

    let refused = Diagnostic::refused(RefusalClass::Machine, "EM_RISCV");
    assert_eq!(refused.refusal(), Some((RefusalClass::Machine, "EM_RISCV")));
}

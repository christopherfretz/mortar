//! Refusals (`spec/diag.md` §4).

use mortar_diag::{
    Collector, Diagnostic, FileRef, Key, Kind, NOT_RECOGNIZED, Place, Policy, RefusalClass,
    Severity, Style,
};

// spec: diag.md §4, §4.1 (one wording for every class)
#[test]
fn every_class_renders_uniformly() {
    let expected = [
        (RefusalClass::Option, "option", "ld: error: unsupported option: NAME\n"),
        (RefusalClass::OptionValue, "option-value", "ld: error: unsupported option value: NAME\n"),
        (RefusalClass::Emulation, "emulation", "ld: error: unsupported emulation: NAME\n"),
        (RefusalClass::FileFormat, "file-format", "ld: error: unsupported file format: NAME\n"),
        (RefusalClass::Machine, "machine", "ld: error: unsupported machine: NAME\n"),
        (RefusalClass::ObjectType, "object-type", "ld: error: unsupported object type: NAME\n"),
        (RefusalClass::SectionType, "section-type", "ld: error: unsupported section type: NAME\n"),
        (RefusalClass::SectionFlag, "section-flag", "ld: error: unsupported section flag: NAME\n"),
        (
            RefusalClass::RelocationType,
            "relocation-type",
            "ld: error: unsupported relocation type: NAME\n",
        ),
        (RefusalClass::SymbolType, "symbol-type", "ld: error: unsupported symbol type: NAME\n"),
        (
            RefusalClass::SymbolBinding,
            "symbol-binding",
            "ld: error: unsupported symbol binding: NAME\n",
        ),
        (
            RefusalClass::CompressionType,
            "compression-type",
            "ld: error: unsupported compression type: NAME\n",
        ),
        (
            RefusalClass::ScriptCommand,
            "script-command",
            "ld: error: unsupported linker script command: NAME\n",
        ),
        (RefusalClass::Feature, "feature", "ld: error: unsupported feature: NAME\n"),
    ];
    assert_eq!(expected.map(|(class, ..)| class), RefusalClass::ALL);
    for (class, id, text) in expected {
        let diag = Diagnostic::refused(class, "NAME");
        assert_eq!(diag.id(), "refused");
        assert_eq!(class.id(), id);
        assert_eq!(diag.render(&Style::plain("ld")), text);
        assert_eq!(text, format!("ld: error: unsupported {}: NAME\n", class.noun()));
    }
}

// spec: diag.md §4.2 (a test reads the class and the name, not the text)
#[test]
fn class_and_name_are_data() {
    let diag = Diagnostic::refused(RefusalClass::RelocationType, "R_AARCH64_TLSDESC_CALL");
    assert_eq!(diag.id(), "refused");
    assert_eq!(diag.refusal(), Some((RefusalClass::RelocationType, "R_AARCH64_TLSDESC_CALL")));
    assert_eq!(
        diag.kind(),
        &Kind::Refused {
            class: RefusalClass::RelocationType,
            name: "R_AARCH64_TLSDESC_CALL".to_owned(),
            not_recognized: false,
        }
    );
    assert_eq!(diag.kind().symbol(), None);
    assert_eq!(Diagnostic::error(Kind::NoInputFiles).refusal(), None);
}

// spec: diag.md §4.2 (the place of the construct), §4 (the form)
#[test]
fn refusal_with_a_place() {
    let reloc = Diagnostic::refused(RefusalClass::RelocationType, "0x999").at(Place::section(
        FileRef::new("badrel.o"),
        ".rela.text",
        0x18,
    ));
    assert_eq!(
        reloc.render(&Style::plain("ld")),
        "ld: error: badrel.o:(.rela.text+0x18): unsupported relocation type: 0x999\n"
    );
    let command = Diagnostic::refused(RefusalClass::ScriptCommand, "PHDRS")
        .at(Place::script("link.ld", 12, 1));
    assert_eq!(
        command.render(&Style::plain("ld")),
        "ld: error: link.ld:12:1: unsupported linker script command: PHDRS\n"
    );
    let option = Diagnostic::refused(RefusalClass::Option, "--plugin")
        .note("link-time optimization is not implemented");
    assert_eq!(
        option.render(&Style::plain("ld")),
        "ld: error: unsupported option: --plugin\n\
         >>> link-time optimization is not implemented\n"
    );
}

// spec: diag.md §8.3 (a format GNU ld does not recognize either: its phrase
// after the refusal's own text)
#[test]
fn a_format_gnu_ld_does_not_recognize_carries_its_phrase() {
    let diag = Diagnostic::refused_unrecognized_format("Mach-O").at(Place::file("a.o"));
    assert_eq!(diag.refusal(), Some((RefusalClass::FileFormat, "Mach-O")));
    assert_eq!(
        diag.render(&Style::plain("ld")),
        "ld: error: a.o: unsupported file format: Mach-O; file not recognized: file format not \
         recognized\n"
    );
    assert!(diag.render(&Style::plain("ld")).ends_with(&format!("{NOT_RECOGNIZED}\n")));
    // A format GNU ld reads (through its plugin) has no such phrase.
    let lto = Diagnostic::refused(RefusalClass::FileFormat, "GCC LTO object");
    assert_eq!(
        lto.render(&Style::plain("ld")),
        "ld: error: unsupported file format: GCC LTO object\n"
    );
}

// spec: diag.md §4.3
#[test]
fn a_refusal_is_never_a_warning() {
    let kind = Kind::Refused {
        class: RefusalClass::SectionFlag,
        name: "SHF_GNU_RETAIN".to_owned(),
        not_recognized: false,
    };
    for diag in [
        Diagnostic::warning(kind.clone()),
        Diagnostic::new(Severity::Warning, kind.clone()),
        Diagnostic::error(kind),
        Diagnostic::refused(RefusalClass::SectionFlag, "SHF_GNU_RETAIN"),
    ] {
        assert_eq!(diag.severity(), Severity::Error);
        assert_eq!(
            diag.render(&Style::plain("ld")),
            "ld: error: unsupported section flag: SHF_GNU_RETAIN\n"
        );
    }
}

// spec: diag.md §4.3 (`-w` never hides a refusal)
#[test]
fn no_warnings_does_not_hide_a_refusal() {
    let collector = Collector::new(Policy { no_warnings: true, ..Policy::default() });
    let kind = Kind::Refused {
        class: RefusalClass::Option,
        name: "--plugin".to_owned(),
        not_recognized: false,
    };
    collector.emit(Key::new(0, 0, 0), Diagnostic::warning(kind));
    assert!(collector.has_errors());
    let report = collector.finish();
    assert!(report.failed());
    assert_eq!(report.render(&Style::plain("ld")), "ld: error: unsupported option: --plugin\n");
}

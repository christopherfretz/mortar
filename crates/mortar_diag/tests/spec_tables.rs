//! The tables of `spec/diag.md` and the code say the same thing: the spec
//! is the list of ids, and a kind cannot be added to one and not the
//! other.

use mortar_diag::{IncompatibleReason, Kind, RefusalClass, Relation};

const SPEC: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../spec/diag.md"));

/// The rows of the first table after the heading that starts with
/// `heading`, as cells without their backticks.
fn table(heading: &str) -> Vec<Vec<String>> {
    let start = SPEC.find(&format!("\n{heading}")).unwrap_or_else(|| panic!("no {heading}"));
    let rows: Vec<Vec<String>> = SPEC[start..]
        .lines()
        .skip(2)
        .skip_while(|line| !line.starts_with('|'))
        .take_while(|line| line.starts_with('|'))
        .skip(2) // the header and the rule under it
        .map(|line| {
            line.trim_matches('|').split('|').map(|cell| cell.trim().replace('`', "")).collect()
        })
        .collect();
    assert!(!rows.is_empty(), "no table under {heading}");
    rows
}

fn is_kebab_case(id: &str) -> bool {
    !id.is_empty()
        && id.split('-').all(|word| {
            !word.is_empty() && word.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

// spec: diag.md §2.2 (an id is lower-case words joined by `-`, unique),
// §3.1 (the table is the list of kinds, in order)
#[test]
fn kind_ids_match_the_spec_table() {
    let rows = table("### 3.1 ");
    let ids: Vec<&str> = rows.iter().map(|row| row[0].as_str()).collect();
    assert_eq!(ids, Kind::IDS);
    for (index, id) in Kind::IDS.iter().enumerate() {
        assert!(is_kebab_case(id), "{id}");
        assert!(!Kind::IDS[..index].contains(id), "{id} is listed twice");
    }
    assert!(!is_kebab_case("Undefined-symbol") && !is_kebab_case("a--b") && !is_kebab_case("a_b"));
}

/// The kinds that can be emitted as a warning, by default or under an
/// option (diag.md §3.2 says which in prose; the table has no severity
/// column, since severity is not part of a kind, §2.2). WP-212 read each
/// from where the linker raises it.
const WARNING_KINDS: &[&str] = &[
    "skipped-incompatible",
    "ignored-member",
    "duplicate-soname",
    "needed-not-found",
    "cet-report",
    "malformed-input",
    "unknown-property",
    "archive-backref",
    "archive-choice",
    "common-alignment",
    "common-larger-than-definition",
    "common-overridden",
    "copy-of-locally-bound-definition",
    "entry-not-found",
    "gnu-warning",
    "multiple-common",
    "undefined-symbol",
    "version-reassigned",
    "version-script-undefined-name",
    "versioned-definition-hidden",
    "meta-malformed",
    "odr-abi-mismatch",
    "odr-archive-shadow",
    "odr-contents-mismatch",
    "odr-declaration-mismatch",
    "odr-internal-reference",
    "odr-isa-mismatch",
    "odr-size-mismatch",
    "odr-strong-and-comdat",
    "odr-symbol-type-mismatch",
    "odr-type-mismatch",
    "odr-violation",
    "odr-vtable-rtti-mismatch",
    "executable-stack",
    "incremental-not-reproducible",
    "incremental-unsupported",
];

/// The subjects of one row of §3.1's table: `None` for `—`, else its
/// words, each of the closed set.
fn subjects(cell: &str) -> Option<Vec<&str>> {
    if cell == "—" {
        return None;
    }
    Some(cell.split(',').map(str::trim).collect())
}

// spec: diag.md §3.1 (the subjects column), incr.md IN-75a: every kind
// that can be a warning has subjects, each from {file, key, output
// section, link}; a kind that is never one has `—`
#[test]
fn warning_kinds_have_subjects() {
    const SET: [&str; 4] = ["file", "key", "output section", "link"];
    let rows = table("### 3.1 ");
    for id in WARNING_KINDS {
        assert!(Kind::IDS.contains(id), "{id} is not a kind");
    }
    for row in &rows {
        let (id, cell) = (row[0].as_str(), row.last().map(String::as_str).unwrap_or(""));
        assert_eq!(row.len(), 5, "{id}: the subjects column is missing");
        match subjects(cell) {
            None => assert!(!WARNING_KINDS.contains(&id), "{id} can be a warning: it needs subjects"),
            Some(words) => {
                assert!(!words.is_empty(), "{id}: empty subjects");
                for (i, word) in words.iter().enumerate() {
                    assert!(SET.contains(word), "{id}: '{word}' is not a subject");
                    assert!(!words[..i].contains(word), "{id}: '{word}' twice");
                }
                assert!(WARNING_KINDS.contains(&id), "{id} has subjects but is not a warning kind");
            }
        }
    }
    // The checker itself: `—`, a word outside the set, an empty cell.
    assert_eq!(subjects("—"), None);
    assert_eq!(subjects("file, key"), Some(vec!["file", "key"]));
    assert!(subjects("section").is_some_and(|w| !w.iter().all(|w| SET.contains(w))));
    assert!(subjects("").is_some_and(|w| !w.iter().all(|w| SET.contains(w))));
}

// spec: diag.md §4.1
#[test]
fn refusal_classes_match_the_spec_table() {
    let rows = table("### 4.1 ");
    assert_eq!(rows.len(), RefusalClass::ALL.len());
    for (row, class) in rows.iter().zip(RefusalClass::ALL) {
        assert_eq!(row[0], class.id());
        assert_eq!(row[1], format!("{class:?}"));
        assert_eq!(row[2], class.noun());
        assert!(is_kebab_case(class.id()));
    }
    let mut ids: Vec<&str> = RefusalClass::ALL.iter().map(|class| class.id()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), RefusalClass::ALL.len());
}

// spec: diag.md §2.4 (the relations and their words: lower-case, no colon)
#[test]
fn relations_match_the_spec_table() {
    let rows = table("The relations and their words:");
    assert_eq!(rows.len(), Relation::ALL.len());
    for (row, relation) in rows.iter().zip(Relation::ALL) {
        assert_eq!(row[0], format!("{relation:?}"));
        assert_eq!(row[1], relation.phrase());
        assert!(relation.phrase().bytes().all(|b| b.is_ascii_lowercase() || b == b' '));
    }
}

// spec: diag.md §3.2 (the reasons of `relocation-incompatible`: the table
// is the list, in order, with its words)
#[test]
fn incompatible_reasons_match_the_spec_table() {
    // The table is indented under its list item.
    let start = SPEC.find("  | variant | words |").expect("the table of reasons");
    let rows: Vec<Vec<String>> = SPEC[start..]
        .lines()
        .map(str::trim_start)
        .take_while(|line| line.starts_with('|'))
        .skip(2)
        .map(|line| {
            line.trim_matches('|').split('|').map(|cell| cell.trim().replace('`', "")).collect()
        })
        .collect();
    assert_eq!(rows.len(), IncompatibleReason::ALL.len());
    for (row, reason) in rows.iter().zip(IncompatibleReason::ALL) {
        assert_eq!(row[0], format!("{reason:?}"));
        assert_eq!(row[1], reason.text());
    }
}

// spec: diag.md §4.4 (the refusals and diagnostics of a `-r` link: each
// a kind of §3.1 or a refusal of a §4.1 class; the names and texts as
// written there, WP-181)
#[test]
fn relocatable_rows_match_the_spec_table() {
    use mortar_diag::relocatable::{self, Entry, ROWS};
    use mortar_diag::{Diagnostic, Style};
    let rows = table("### 4.4 ");
    let ids: Vec<&str> = rows.iter().map(|row| row[0].as_str()).collect();
    let code: Vec<&str> = ROWS.iter().map(|row| row.id).collect();
    assert_eq!(ids, code);
    let plain = |d: Diagnostic| d.render(&Style::plain("ld"));
    for (row, entry) in rows.iter().zip(ROWS) {
        let (kind, text, until) = (&row[1], &row[2], &row[4]);
        assert_eq!(until, entry.until, "{}", entry.id);
        match entry.entry {
            Entry::Refusal { class, names } => {
                assert_eq!(*kind, format!("refused, {}", class.id()), "{}", entry.id);
                if text.starts_with("as in a final link") {
                    continue;
                }
                for name in names {
                    let head = format!("unsupported {}: {name}", class.noun());
                    // The text spells the first name; the others are in the id.
                    assert!(
                        text.starts_with(&head) || (*name != names[0] && entry.id.contains(name)),
                        "{}: {text} does not start {head}",
                        entry.id
                    );
                    if !name.contains('{') {
                        let built = plain(Diagnostic::refused(class, *name));
                        assert_eq!(built, format!("ld: error: {head}\n"));
                    }
                }
            }
            Entry::Kind(id) | Entry::Warning(id) => {
                assert!(Kind::IDS.contains(&id), "{id}");
                if matches!(entry.entry, Entry::Kind(_)) {
                    assert!(kind.starts_with(id), "{}: {kind}", entry.id);
                } else {
                    assert!(kind.contains("warning"), "{}: {kind}", entry.id);
                }
            }
            Entry::AsFinal => {
                assert!(
                    kind.starts_with("as ") || kind.starts_with("refused as "),
                    "{}: {kind}",
                    entry.id
                );
            }
            Entry::NotBuilt(id) => {
                // Not a kind of §3.1: when it becomes one, it is built.
                assert!(!Kind::IDS.contains(&id), "{id} is a kind now: build it");
                assert!(kind.contains(id), "{}: {kind}", entry.id);
            }
        }
    }
    // The conflict's text and its `{second}` list.
    let conflict = &rows[1];
    assert!(conflict[2].starts_with("-r and {second} cannot be used together"));
    for second in relocatable::CONFLICTS_WITH_R {
        assert!(conflict[2].contains(second), "{second}");
    }
    assert_eq!(
        plain(relocatable::conflicting_with_r("-shared")),
        "ld: error: -r and -shared cannot be used together\n"
    );
    // The names built from an option, and their notes.
    for option in relocatable::OPTIONS_REFUSED_WITH_R {
        let name = relocatable::with_r(option);
        assert!(ids.contains(&name.as_str()), "{name}");
    }
    let tail = ROWS.iter().find(|row| row.id.starts_with("-d with -r")).expect("C-R3's row");
    for option in relocatable::OPTIONS_REFUSED_UNTIL {
        assert!(tail.id.contains(&relocatable::with_r(option)), "{option}");
    }
    let z = &rows.iter().find(|row| row[0] == "-z KEYWORD with -r").expect("the -z row")[2];
    for keyword in relocatable::Z_KEYWORDS_REFUSED_WITH_R {
        // `x86-64-v2/-v3/-v4` in the spec.
        let short = keyword.strip_prefix("x86-64").unwrap_or(keyword);
        assert!(z.contains(keyword.trim_end_matches('=')) || z.contains(short), "{keyword}");
    }
    assert_eq!(
        plain(relocatable::refused_z_with_r("force-bti")),
        "ld: error: unsupported option: -z force-bti with -r\n"
    );
    for (id, note) in [
        ("-T with -r", relocatable::SCRIPT_NOTE),
        ("--gc-sections with -r", relocatable::GC_SECTIONS_NOTE),
        ("--wrap with -r", relocatable::WRAP_NOTE),
        (relocatable::NO_INPUT_OF_A_KNOWN_MACHINE, relocatable::NO_INPUT_OF_A_KNOWN_MACHINE_NOTE),
    ] {
        let text = &rows.iter().find(|row| row[0] == id).expect(id)[2];
        assert!(text.contains(&format!("(note: {note})")), "{id}: {text}");
    }
    assert_eq!(
        plain(relocatable::refused_relocatable_output()),
        "ld: error: unsupported feature: relocatable output (-r)\n"
    );
}

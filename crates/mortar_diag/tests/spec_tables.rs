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

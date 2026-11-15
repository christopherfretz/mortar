//! `spec/model.md` §6: a malformed `MORTAR_SCHED_SEED` is an error of the
//! command-line phase, `invalid-option-value`.

use mortar_core::par::{BadSeed, SEED_VAR, parse_seed};
use mortar_core::seed::SEED_KEY;
use mortar_diag::{Context, Diagnostic, Key, Kind, Severity};

// spec: model.md §6 (a seed is a decimal integer below 2^64, white space around it allowed)
#[test]
fn s6_seed_values() {
    assert_eq!(parse_seed("42"), Ok(42));
    assert_eq!(parse_seed(" 7\n"), Ok(7));
    assert_eq!(parse_seed("18446744073709551615"), Ok(u64::MAX));
    for bad in ["", "x", "-1", "0x10", "18446744073709551616", "1 2"] {
        assert_eq!(parse_seed(bad), Err(BadSeed(bad.to_owned())), "{bad:?}");
    }
}

// spec: model.md §6 (a malformed seed is `invalid-option-value` naming the variable and
// the value, with a note), §8 (key (10, 0, 0))
#[test]
fn s6_malformed_seed_is_a_command_line_error() {
    let diag = Diagnostic::from(parse_seed("abc").unwrap_err());
    assert_eq!(diag.id(), "invalid-option-value");
    assert_eq!(diag.severity(), Severity::Error);
    assert_eq!(
        diag.kind(),
        &Kind::InvalidOptionValue { option: SEED_VAR.to_owned(), value: "abc".to_owned() }
    );
    assert_eq!(SEED_VAR, "MORTAR_SCHED_SEED");
    assert_eq!(
        diag.context(),
        &[Context::Note("expected a decimal integer below 2^64".to_owned())]
    );
    assert_eq!(SEED_KEY, Key::new(10, 0, 0));
}

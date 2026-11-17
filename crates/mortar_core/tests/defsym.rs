//! `spec/resolve.md` §7.2: the `--defsym` grammar (WP-153).

use mortar_core::plan::{Defsym, DefsymExpr, DefsymRefusal};

fn parse(v: &str) -> Result<(String, DefsymExpr<'_>), DefsymRefusal> {
    Defsym::parse(v.as_bytes()).map(|d| (String::from_utf8_lossy(d.sym).into_owned(), d.expr))
}

#[test]
fn s7_2_numbers_are_hexadecimal_or_decimal_without_a_leading_zero() {
    assert_eq!(parse("k=0x1234"), Ok(("k".into(), DefsymExpr::Number(0x1234))));
    assert_eq!(parse("k=0X1234"), Ok(("k".into(), DefsymExpr::Number(0x1234))));
    assert_eq!(parse("k=4660"), Ok(("k".into(), DefsymExpr::Number(4660))));
    assert_eq!(parse("k=0"), Ok(("k".into(), DefsymExpr::Number(0))));
    assert_eq!(parse("k=0xffffffffffffffff"), Ok(("k".into(), DefsymExpr::Number(u64::MAX))));
    // GNU ld reads 011064 as octal, lld and mold as decimal.
    assert_eq!(parse("k=011064"), Err(DefsymRefusal::LeadingZero));
    assert_eq!(parse("k=00"), Err(DefsymRefusal::LeadingZero));
    assert!(DefsymRefusal::LeadingZero.note().contains("write it in hexadecimal"));
    // Wider than 64 bits, or a bare prefix.
    assert_eq!(parse("k=0x10000000000000000"), Err(DefsymRefusal::Expression));
    assert_eq!(parse("k=0x"), Err(DefsymRefusal::Expression));
}

#[test]
fn s7_2_a_symbol_is_an_alias_and_a_symbol_plus_a_number_an_offset() {
    assert_eq!(parse("a=b"), Ok(("a".into(), DefsymExpr::Symbol(b"b"))));
    assert_eq!(parse("a = b"), Ok(("a".into(), DefsymExpr::Symbol(b"b"))));
    assert_eq!(parse("a=_Z1fv"), Ok(("a".into(), DefsymExpr::Symbol(b"_Z1fv"))));
    assert_eq!(parse("a=b.c$d"), Ok(("a".into(), DefsymExpr::Symbol(b"b.c$d"))));
    assert_eq!(parse("a=b+4"), Ok(("a".into(), DefsymExpr::Offset(b"b", 4))));
    assert_eq!(parse("a=b - 0x10"), Ok(("a".into(), DefsymExpr::Offset(b"b", -16))));
    assert_eq!(DefsymExpr::Offset(b"b", 1).symbol(), Some(&b"b"[..]));
    assert_eq!(DefsymExpr::Number(1).symbol(), None);
}

#[test]
fn s7_2_every_other_expression_is_refused() {
    for v in [
        "k=1k",
        "k=1M",
        "k=0x10K",
        "k=0x1234+4",
        "k=0x1000-1",
        "k=1<<4",
        "k=(0x1000)",
        "k=ALIGN(16)",
        "k=b+b",
        "k=4+b",
        "k=b*2",
        "k=-1",
        "k=.",
    ] {
        assert_eq!(parse(v), Err(DefsymRefusal::Expression), "{v}");
    }
    assert_eq!(parse("k=b+010"), Err(DefsymRefusal::LeadingZero));
    for v in ["k", "=1", "k=", "k= "] {
        assert_eq!(parse(v), Err(DefsymRefusal::Shape), "{v}");
    }
}

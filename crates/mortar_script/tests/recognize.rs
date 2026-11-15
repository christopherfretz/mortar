//! `spec/script.md` §3: recognizing a script among the inputs.

use mortar_script::{looks_like_script, parse};

// spec: script.md §3.4 — text is a script candidate, the empty file
// included; bytes beyond ASCII may be in it (a comment may hold UTF-8).
#[test]
fn text_is_a_candidate() {
    for bytes in [
        &b""[..],
        b" ",
        b";",
        b"/* GNU ld script */\nGROUP ( libgcc_s.so.1 -lgcc )\n",
        b"\t\r\n",
        b"/* \xc3\xa9 */ ;",
        b"garbage",
        b"\xef\xbb\xbf;",
    ] {
        assert!(looks_like_script(bytes), "{bytes:?}");
    }
}

// spec: script.md §3.4 — the formats with readers of their own, and bytes
// that cannot be script text, are not candidates.
#[test]
fn other_formats_and_binary_are_not() {
    for bytes in [
        &b"\x7fELF\x02\x01\x01\x00"[..],
        b"!<arch>\n",
        b"!<arch>\nfoo.o/",
        b"!<thin>\n",
        b"\0",
        b"INPUT(a.o)\0junk",
        b"\x01\x02\x03\x04",
        b"\x0c;",
        b"\x0b;",
        b"\x1b[31m",
        b"\x7f",
        b"BC\xc0\xde\x35\x14\x00\x00",
    ] {
        assert!(!looks_like_script(bytes), "{bytes:?}");
    }
}

// spec: script.md §3.4 — a candidate need not parse: the parser then says
// what is wrong, where.
#[test]
fn a_candidate_may_still_be_malformed() {
    let bytes = b"not an object";
    assert!(looks_like_script(bytes));
    let d = parse(bytes, "t.ld").unwrap_err();
    assert_eq!(d.id(), "malformed-input");
}

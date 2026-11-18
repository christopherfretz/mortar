//! WP-247 items d and k: `FILES.flags` bits that are a function of a
//! file's bytes (`capture::object_flags`): `EH_FIXED` from the CIEs of
//! `.eh_frame` (IN-34a's sentence, exactly) on hand-made records. The
//! objects (`DEBUG_EXPORTS`, and both bits through a real link) are in
//! `crates/mortar/tests/state5_capture.rs`.

use mortar_incr::capture::eh_fixed;

/// A CIE record (length, id 0, version 1, `aug`, code align 4, data
/// align -8, return register 30, then `data` as the augmentation data).
fn cie(aug: &[u8], data: &[u8]) -> Vec<u8> {
    let mut body = vec![0, 0, 0, 0, 1];
    body.extend_from_slice(aug);
    body.push(0);
    body.extend_from_slice(&[4, 0x78, 30]);
    if aug.first() == Some(&b'z') {
        body.push(u8::try_from(data.len()).expect("small"));
    }
    body.extend_from_slice(data);
    while body.len() % 4 != 0 {
        body.push(0); // DW_CFA_nop
    }
    let mut r = u32::try_from(body.len()).expect("small").to_le_bytes().to_vec();
    r.extend_from_slice(&body);
    r
}

/// An FDE record pointing back at offset 0 (its fields are not read).
fn fde(at: usize) -> Vec<u8> {
    let mut r = 16u32.to_le_bytes().to_vec();
    r.extend_from_slice(&u32::try_from(at + 4).expect("small").to_le_bytes());
    r.extend_from_slice(&[0; 12]);
    r
}

fn section(records: &[Vec<u8>]) -> Vec<u8> {
    let mut v: Vec<u8> = records.concat();
    v.extend_from_slice(&[0; 4]); // the terminator
    v
}

// spec: incr.md IN-34a ("Only the encodings 0x1b (pcrel sdata4) and 0x9b
// (indirect pcrel sdata4) are moved"); WP-247 item d (EH_FIXED iff a CIE
// gives an FDE, LSDA or personality encoding other than 0x1b / 0x9b)
#[test]
fn eh_fixed_is_ins_34a_sentence() {
    // GCC's: zR 0x1b; zPLR 0x9b + 4-byte pointer, 0x1b, 0x1b
    let gcc = cie(b"zR", &[0x1b]);
    let cxx = cie(b"zPLR", &[0x9b, 0, 0, 0, 0, 0x1b, 0x1b]);
    for b64 in [true, false] {
        assert!(!eh_fixed(&section(std::slice::from_ref(&gcc)), b64));
        assert!(!eh_fixed(&section(&[gcc.clone(), fde(gcc.len()), cxx.clone()]), b64));
        assert!(!eh_fixed(&[], b64), "no records");
        assert!(!eh_fixed(&section(&[]), b64), "a terminator only");
    }
    // omitted L and P give none
    assert!(!eh_fixed(&section(&[cie(b"zPLR", &[0xff, 0xff, 0x1b])]), true));
    // S (signal frame) and B carry no data
    assert!(!eh_fixed(&section(&[cie(b"zRS", &[0x1b])]), true));
    // other encodings, in each place
    let fixed = [
        ("R absptr", cie(b"zR", &[0x00])),
        ("R udata4", cie(b"zR", &[0x03])),
        ("R pcrel sdata8", cie(b"zR", &[0x1c])),
        ("no R: absptr", cie(b"z", &[])),
        ("no augmentation: absptr", cie(b"", &[])),
        ("L absptr", cie(b"zLR", &[0x00, 0x1b])),
        ("P direct pcrel", cie(b"zPR", &[0x1b, 0, 0, 0, 0, 0x1b])),
        ("P absptr (8 bytes)", cie(b"zPR", &[0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0x1b])),
    ];
    for (what, c) in &fixed {
        assert!(eh_fixed(&section(&[gcc.clone(), c.clone()]), true), "{what}");
        assert!(eh_fixed(&section(&[c.clone(), fde(0), gcc.clone()]), true), "{what}, first");
    }
    // records the walk cannot read count as fixed
    let unreadable = [
        ("an unknown letter", cie(b"zXR", &[0x1b])),
        ("letters without z", cie(b"eh", &[])),
        ("a 64-bit length", vec![0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0]),
        ("a length past the bytes", vec![0x40, 0, 0, 0, 0, 0, 0, 0]),
        ("a LEB personality pointer", cie(b"zPR", &[0x01, 0, 0x1b])),
    ];
    for (what, c) in &unreadable {
        assert!(eh_fixed(&section(&[gcc.clone(), c.clone()]), true), "{what}");
    }
    // past the terminator nothing is read (the walk stops at length 0)
    let mut after = section(std::slice::from_ref(&gcc));
    after.extend_from_slice(&cie(b"zR", &[0x00]));
    assert!(!eh_fixed(&after, true));
}

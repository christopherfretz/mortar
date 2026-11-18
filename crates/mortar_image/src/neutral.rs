//! The bytes of a debug hole (`spec/incr.md` IN-51b): a neutral unit of
//! its section's grammar, which the sequential readers (`readelf`,
//! `llvm-dwarfdump`, gdb, lldb) pass over, and the neutral abbreviation
//! table the neutral `.debug_info` units name.

/// The neutral abbreviation table (IN-51b): code 1, `DW_TAG_compile_unit`,
/// no children, `DW_AT_lo_user` as `DW_FORM_block`, then the two zeros
/// ending the attribute list. The table ends at the next 0 code: its hole
/// is one byte longer, written 0 (`mortar_layout::slack::ABBREV_TABLE`).
pub const ABBREV_TABLE: [u8; 8] = [0x01, 0x11, 0x00, 0x80, 0x40, 0x09, 0x00, 0x00];

/// The length of `v` as a ULEB128.
fn uleb_len(mut v: u64) -> u64 {
    let mut n = 1;
    while v >= 0x80 {
        v >>= 7;
        n += 1;
    }
    n
}

/// `v` as a ULEB128 of exactly `n` bytes (`n` at least its length).
fn uleb_padded(out: &mut Vec<u8>, mut v: u64, n: u64) {
    for i in 0..n {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        out.push(if i + 1 < n { b | 0x80 } else { b });
    }
}

fn u16le(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn u32le(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// A `DW_FORM_block` from `out`'s end to byte `size`: its length `L` in
/// `n` ULEB bytes (padded with 0x80 continuation bytes when `L` needs
/// fewer), `L + n` = the rest, then `L` zeros. `None` when the rest is
/// below 2 (a 0-byte block makes readelf warn "Block ends prematurely").
fn block(out: &mut Vec<u8>, size: u64) -> Option<()> {
    let rest = size.checked_sub(out.len() as u64)?;
    let n = (1..=10u64).find(|&n| rest > n && uleb_len(rest - n) <= n)?;
    uleb_padded(out, rest - n, n);
    out.resize(usize::try_from(size).ok()?, 0);
    Some(())
}

/// The neutral `.debug_info` unit of `size` bytes that keeps
/// `DW_AT_addr_base` (IN-51, DWARF 5 only: a dead file's `.debug_addr`
/// table stays named): `size-4` (u32), 5 (u16), `DW_UT_compile` (1),
/// `addr_size`, `abbrev` (u32, the offset of
/// [`ABBREV_TABLE_ADDR_BASE`] in `.debug_abbrev`), code 1, `addr_base`
/// (u32: the dead table's offset plus its 8-byte header), then IN-51's
/// block to the end; 19 bytes at least. `None` below that.
#[must_use]
pub fn info_addr_base(size: u64, addr_size: u8, abbrev: u32, addr_base: u32) -> Option<Vec<u8>> {
    let length = u32::try_from(size.checked_sub(4)?).ok()?;
    let mut out = Vec::with_capacity(usize::try_from(size).ok()?);
    u32le(&mut out, length);
    u16le(&mut out, 5);
    out.extend_from_slice(&[1, addr_size]);
    u32le(&mut out, abbrev);
    out.push(1);
    u32le(&mut out, addr_base);
    block(&mut out, size)?;
    Some(out)
}

/// The neutral DWARF 4 type unit of `size` bytes in `.debug_types`
/// (IN-51b): `size-4` (u32), 4 (u16), `abbrev` (u32, the offset of the
/// table holding [`ABBREV_TYPE_UNIT`]), `addr_size`, `signature` (u64,
/// [`type_signature`] of the unit's offset), `type_offset` 23 (u32: the
/// unit's one DIE), code 2, then IN-51's block to the end; 26 bytes at
/// least. `None` below that.
#[must_use]
pub fn type_unit(size: u64, addr_size: u8, abbrev: u32, signature: u64) -> Option<Vec<u8>> {
    let length = u32::try_from(size.checked_sub(4)?).ok()?;
    let mut out = Vec::with_capacity(usize::try_from(size).ok()?);
    u32le(&mut out, length);
    u16le(&mut out, 4);
    u32le(&mut out, abbrev);
    out.push(addr_size);
    out.extend_from_slice(&signature.to_le_bytes());
    u32le(&mut out, 23);
    out.push(ABBREV_TYPE_UNIT[0]);
    block(&mut out, size)?;
    Some(out)
}

/// The neutral abbreviation table of a unit that keeps `DW_AT_addr_base`
/// (IN-51): code 1, `DW_TAG_compile_unit`, no children, `DW_AT_addr_base`
/// as `DW_FORM_sec_offset`, `DW_AT_lo_user` as `DW_FORM_block`, the two
/// zeros ending the attribute list. It is written at the start of the dead
/// file's `.debug_abbrev` range, whose next byte (0) ends the table: it
/// needs 11 bytes there.
pub const ABBREV_TABLE_ADDR_BASE: [u8; 10] =
    [0x01, 0x11, 0x00, 0x73, 0x17, 0x80, 0x40, 0x09, 0x00, 0x00];

/// The abbreviation of a neutral DWARF 4 type unit (IN-51b,
/// `.debug_types`): code 2, `DW_TAG_type_unit`, no children,
/// `DW_AT_lo_user` as `DW_FORM_block`, the two zeros ending its attribute
/// list.
pub const ABBREV_TYPE_UNIT: [u8; 8] = [0x02, 0x41, 0x00, 0x80, 0x40, 0x09, 0x00, 0x00];

/// The type signature of a neutral type unit at offset `at` of
/// `.debug_types`: `0xdead` in the top 16 bits, the offset below them, so
/// that no two neutral units of one output share a signature (a real
/// signature is 64 bits of a hash; `DW_FORM_ref_sig8` never names these).
#[must_use]
pub fn type_signature(at: u64) -> u64 {
    0xdead_0000_0000_0000 | (at & 0x0000_ffff_ffff_ffff)
}

/// The neutral unit of `size` bytes for debug section `name` (IN-51b), or
/// `None` when `size` is below the form's minimum, off its granule, or the
/// section has no neutral form here (`.debug_types`: [`type_unit`]; a
/// `.debug_info` unit with `DW_AT_addr_base`: [`info_addr_base`]).
/// `version` is the DWARF version of the unit (`.debug_info`,
/// `.debug_line` and `.debug_macro` write version 4 for 2 to 4, else 5;
/// the image writes 5, whatever the contributions around it, since a
/// unit's version is its own; the other headers are version 5, or 1 for
/// the `.debug_frame` CIE, whatever `version` is); `addr_size` the
/// target's address size; `abbrev` the offset of the neutral abbreviation
/// table in `.debug_abbrev`.
///
/// The forms, `L` the size, `A` the address size, multi-byte fields
/// little-endian:
/// - `.debug_abbrev`, `.debug_ranges`, `.debug_loc`, `.debug_macinfo`:
///   `L` zero bytes (an empty table, end-of-list pairs, end entries);
/// - `.debug_rnglists`, `.debug_loclists`: `L-4` (u32), 5 (u16), `A`, 0,
///   0 (u32 `offset_entry_count`), then zeros (`DW_RLE_end_of_list`,
///   `DW_LLE_end_of_list`); 12 at least;
/// - `.debug_str_offsets`: `L-4` (u32), 5 (u16), 0 (u16), zero entries;
///   8 at least, a multiple of 4;
/// - `.debug_addr`: `L-4` (u32), 5 (u16), `A`, 0, zero entries; 8 at
///   least, a multiple of `A`;
/// - `.debug_frame`: one CIE: `L-4` (u32), `ff ff ff ff` (`CIE_id`),
///   version 1, augmentation "" (0), code alignment 1, data alignment −4
///   (`7c`), return-address register 30 (`1e`), then `DW_CFA_nop` (0); 13
///   at least;
/// - `.debug_macro`: version (u16), flags 0, `DW_MACRO_undef` (2), line 0,
///   an inline string of `L-7` bytes `x` and its NUL, the 0 ending the
///   unit; 7 at least.
#[must_use]
pub fn unit(name: &[u8], size: u64, version: u16, addr_size: u8, abbrev: u32) -> Option<Vec<u8>> {
    let len = usize::try_from(size).ok()?;
    let length = u32::try_from(size.saturating_sub(4)).ok()?;
    let zeros =
        matches!(name, b".debug_abbrev" | b".debug_ranges" | b".debug_loc" | b".debug_macinfo");
    if size < 4 && !zeros {
        return None;
    }
    let v5 = !(2..=4).contains(&version);
    let mut out = Vec::with_capacity(len);
    match name {
        _ if zeros => {}
        b".debug_info" => {
            // The header, abbreviation code 1, then a `DW_FORM_block`
            // whose ULEB length makes it end exactly at the unit's end.
            u32le(&mut out, length);
            if v5 {
                u16le(&mut out, 5);
                out.extend_from_slice(&[1, addr_size]); // DW_UT_compile
                u32le(&mut out, abbrev);
            } else {
                u16le(&mut out, 4);
                u32le(&mut out, abbrev);
                out.push(addr_size);
            }
            out.push(1);
            block(&mut out, size)?;
        }
        b".debug_line" => {
            u32le(&mut out, length);
            // minimum_instruction_length, [maximum_operations_per_
            // instruction], default_is_stmt, line_base, line_range,
            // opcode_base 7, standard_opcode_lengths.
            let common: [u8; 11] = [1, 1, 1, 0xfb, 14, 7, 0, 1, 1, 1, 1];
            let lengths_tail: u8 = 0;
            if v5 {
                u16le(&mut out, 5);
                out.extend_from_slice(&[addr_size, 0]);
                u32le(&mut out, 20);
                out.extend_from_slice(&common);
                out.push(lengths_tail);
                // One format entry (DW_LNCT_path, DW_FORM_string) for
                // the directories and for the files, counts 0.
                out.extend_from_slice(&[1, 1, 0x08, 0, 1, 1, 0x08, 0]);
            } else {
                u16le(&mut out, 4);
                u32le(&mut out, 14);
                out.extend_from_slice(&common);
                out.push(lengths_tail);
                // The empty directory and file lists.
                out.extend_from_slice(&[0, 0]);
            }
            if out.len() > len {
                return None;
            }
            // DW_LNS_negate_stmt, which takes no operand, to the end.
            out.resize(len, 0x06);
        }
        b".debug_rnglists" | b".debug_loclists" => {
            u32le(&mut out, length);
            u16le(&mut out, 5);
            out.extend_from_slice(&[addr_size, 0]);
            u32le(&mut out, 0);
        }
        b".debug_str_offsets" => {
            if !size.is_multiple_of(4) {
                return None;
            }
            u32le(&mut out, length);
            u16le(&mut out, 5);
            u16le(&mut out, 0);
        }
        b".debug_addr" => {
            if size < 8 || addr_size == 0 || !size.is_multiple_of(u64::from(addr_size)) {
                return None;
            }
            u32le(&mut out, length);
            u16le(&mut out, 5);
            out.extend_from_slice(&[addr_size, 0]);
        }
        b".debug_frame" => {
            u32le(&mut out, length);
            u32le(&mut out, 0xffff_ffff);
            // version 1, "", code alignment 1, data alignment -4, the
            // return-address register 30; DW_CFA_nop (0) to the end.
            out.extend_from_slice(&[1, 0, 1, 0x7c, 30]);
        }
        b".debug_macro" => {
            if size < 7 {
                return None;
            }
            u16le(&mut out, if v5 { 5 } else { 4 });
            // flags 0 (32-bit offsets, no line offset, no opcode table),
            // DW_MACRO_undef, line 0, the string to 2 bytes from the end.
            out.extend_from_slice(&[0, 2, 0]);
            out.resize(len - 2, b'x');
            out.extend_from_slice(&[0, 0]);
        }
        _ => return None,
    }
    if out.len() > len {
        return None;
    }
    out.resize(len, 0);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: incr.md §6 IN-51b (.debug_info: a block unit ending at the unit end)
    #[test]
    fn in51b_info_units() {
        for size in 14..400u64 {
            let u = unit(b".debug_info", size, 4, 8, 0x40).unwrap();
            assert_eq!(u.len() as u64, size);
            assert_eq!(u64::from(u32::from_le_bytes(u[0..4].try_into().unwrap())), size - 4);
            assert_eq!(&u[4..6], &[4, 0]);
            assert_eq!(u[11], 1);
        }
        for size in 15..400u64 {
            let u = unit(b".debug_info", size, 5, 8, 0).unwrap();
            assert_eq!(u.len() as u64, size);
            assert_eq!(&u[4..8], &[5, 0, 1, 8]);
        }
        assert!(unit(b".debug_info", 13, 5, 8, 0).is_none());
        // A 2-byte ULEB length: 130 + 2 + 12 header bytes.
        let u = unit(b".debug_info", 144, 4, 8, 0).unwrap();
        assert_eq!(&u[12..14], &[0x82, 0x01]);
        // 129 bytes after the code: 127 in two bytes (0xff 0x00).
        let u = unit(b".debug_info", 141, 4, 8, 0).unwrap();
        assert_eq!(&u[12..14], &[0xff, 0x00]);
    }

    // spec: incr.md §6 IN-51b (.debug_line: header_length exact, 0x06 to the end)
    #[test]
    fn in51b_line_units() {
        let u = unit(b".debug_line", 32, 5, 8, 0).unwrap();
        assert_eq!(u.len(), 32);
        assert_eq!(u32::from_le_bytes(u[8..12].try_into().unwrap()), 20);
        assert_eq!(u[17], 7);
        let u = unit(b".debug_line", 40, 4, 8, 0).unwrap();
        assert_eq!(u32::from_le_bytes(u[6..10].try_into().unwrap()), 14);
        assert_eq!(&u[24..], &[6; 16]);
        assert!(unit(b".debug_line", 31, 5, 8, 0).is_none());
    }

    // spec: incr.md §6 IN-51b (rnglists, str_offsets, zero forms)
    #[test]
    fn in51b_other_units() {
        let u = unit(b".debug_rnglists", 20, 5, 4, 0).unwrap();
        assert_eq!(&u[..12], &[16, 0, 0, 0, 5, 0, 4, 0, 0, 0, 0, 0]);
        let u = unit(b".debug_str_offsets", 16, 5, 8, 0).unwrap();
        assert_eq!(&u[..8], &[12, 0, 0, 0, 5, 0, 0, 0]);
        assert!(unit(b".debug_str_offsets", 18, 5, 8, 0).is_none());
        assert_eq!(unit(b".debug_abbrev", 3, 5, 8, 0).unwrap(), vec![0; 3]);
        assert_eq!(unit(b".debug_ranges", 16, 5, 8, 0).unwrap(), vec![0; 16]);
        assert!(unit(b".debug_types", 32, 4, 8, 0).is_none());
        assert!(unit(b".debug_aranges", 32, 4, 8, 0).is_none());
        assert!(unit(b".debug_str", 32, 4, 8, 0).is_none());
    }

    // spec: incr.md §6 IN-51b (.debug_loclists: the rnglists header, zero body)
    #[test]
    fn in51b_loclists_units() {
        for a in [4u8, 8] {
            let u = unit(b".debug_loclists", 12, 5, a, 0).unwrap();
            assert_eq!(u, [8, 0, 0, 0, 5, 0, a, 0, 0, 0, 0, 0]);
            let u = unit(b".debug_loclists", 37, 4, a, 0).unwrap();
            assert_eq!(&u[..12], &[33, 0, 0, 0, 5, 0, a, 0, 0, 0, 0, 0]);
            assert_eq!(&u[12..], &[0; 25]);
        }
        assert!(unit(b".debug_loclists", 11, 5, 8, 0).is_none());
    }

    // spec: incr.md §6 IN-51b (.debug_loc, .debug_macinfo: zero bytes)
    #[test]
    fn in51b_zero_units() {
        for name in [&b".debug_loc"[..], b".debug_macinfo"] {
            for size in [1u64, 16, 17, 333] {
                assert_eq!(unit(name, size, 4, 8, 7).unwrap(), vec![0; size as usize]);
            }
        }
    }

    // spec: incr.md §6 IN-51b (.debug_addr: a v5 header, zeros, granule A)
    #[test]
    fn in51b_addr_units() {
        assert_eq!(unit(b".debug_addr", 8, 5, 8, 0).unwrap(), [4, 0, 0, 0, 5, 0, 8, 0]);
        let u = unit(b".debug_addr", 24, 5, 8, 0).unwrap();
        assert_eq!(u, [20, 0, 0, 0, 5, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let u = unit(b".debug_addr", 12, 5, 4, 0).unwrap();
        assert_eq!(u, [8, 0, 0, 0, 5, 0, 4, 0, 0, 0, 0, 0]);
        assert!(unit(b".debug_addr", 12, 5, 8, 0).is_none());
        assert!(unit(b".debug_addr", 4, 5, 4, 0).is_none());
        assert!(unit(b".debug_addr", 16, 5, 0, 0).is_none());
    }

    // spec: incr.md §6 IN-51b (.debug_frame: one version-1 CIE, DW_CFA_nop)
    #[test]
    fn in51b_frame_units() {
        let u = unit(b".debug_frame", 13, 4, 8, 0).unwrap();
        assert_eq!(u, [9, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 1, 0, 1, 0x7c, 30]);
        let u = unit(b".debug_frame", 40, 5, 4, 0).unwrap();
        assert_eq!(&u[..13], &[36, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 1, 0, 1, 0x7c, 30]);
        assert_eq!(&u[13..], &[0; 27]);
        assert!(unit(b".debug_frame", 12, 5, 8, 0).is_none());
    }

    // spec: incr.md §6 IN-51b (.debug_macro: one DW_MACRO_undef filling the unit)
    #[test]
    fn in51b_macro_units() {
        assert_eq!(unit(b".debug_macro", 7, 5, 8, 0).unwrap(), [5, 0, 0, 2, 0, 0, 0]);
        assert_eq!(unit(b".debug_macro", 7, 4, 8, 0).unwrap(), [4, 0, 0, 2, 0, 0, 0]);
        let u = unit(b".debug_macro", 12, 5, 4, 0).unwrap();
        assert_eq!(u, [5, 0, 0, 2, 0, b'x', b'x', b'x', b'x', b'x', 0, 0]);
        assert!(unit(b".debug_macro", 6, 5, 8, 0).is_none());
    }

    // spec: incr.md §6 IN-51 (the DW_AT_addr_base variant), IN-51b (.debug_types)
    #[test]
    fn in51_addr_base_and_type_units() {
        let u = info_addr_base(19, 8, 0x30, 0x48).unwrap();
        assert_eq!(u, [15, 0, 0, 0, 5, 0, 1, 8, 0x30, 0, 0, 0, 1, 0x48, 0, 0, 0, 1, 0]);
        assert!(info_addr_base(18, 8, 0, 0).is_none());
        for size in 19..300u64 {
            let u = info_addr_base(size, 8, 0, 8).unwrap();
            assert_eq!(u.len() as u64, size);
        }
        let sig = type_signature(0x1234);
        assert_eq!(sig, 0xdead_0000_0000_1234);
        let u = type_unit(26, 8, 0x40, sig).unwrap();
        let mut want = vec![22, 0, 0, 0, 4, 0, 0x40, 0, 0, 0, 8];
        want.extend_from_slice(&sig.to_le_bytes());
        want.extend_from_slice(&[23, 0, 0, 0, 2, 1, 0]);
        assert_eq!(u, want);
        assert!(type_unit(25, 8, 0, sig).is_none());
        for size in 26..300u64 {
            assert_eq!(type_unit(size, 4, 0, sig).unwrap().len() as u64, size);
        }
        // The tables end their attribute lists with two zeros.
        assert_eq!(&ABBREV_TABLE_ADDR_BASE[8..], &[0, 0]);
        assert_eq!(&ABBREV_TYPE_UNIT[..2], &[2, 0x41]);
    }

    /// Not a check: the bytes of the forms, for the scripts that place them
    /// into linked outputs and have the debuggers and dump tools judge them
    /// (`tests/incr-design/wp228/place.py --rust`). Reads requests, one per
    /// line, from the file `MORTAR_NEUTRAL_REQ` and writes one line of hex
    /// (or `none`) per request to `MORTAR_NEUTRAL_OUT`:
    /// `unit NAME SIZE VERSION ADDR_SIZE ABBREV`,
    /// `addrbase SIZE ADDR_SIZE ABBREV ADDR_BASE`,
    /// `type SIZE ADDR_SIZE ABBREV OFFSET`, `table compile|addrbase|type`.
    #[test]
    #[ignore = "run by tests/incr-design/wp228/place.py --rust"]
    fn emit_forms() {
        let (Some(req), Some(out)) =
            (std::env::var_os("MORTAR_NEUTRAL_REQ"), std::env::var_os("MORTAR_NEUTRAL_OUT"))
        else {
            return;
        };
        let text = std::fs::read_to_string(req).unwrap();
        let mut lines = String::new();
        for line in text.lines() {
            let w: Vec<&str> = line.split_whitespace().collect();
            let n = |i: usize| -> u64 { w[i].parse().unwrap() };
            let bytes = match w.first().copied() {
                Some("unit") => unit(w[1].as_bytes(), n(2), n(3) as u16, n(4) as u8, n(5) as u32),
                Some("addrbase") => info_addr_base(n(1), n(2) as u8, n(3) as u32, n(4) as u32),
                Some("type") => type_unit(n(1), n(2) as u8, n(3) as u32, type_signature(n(4))),
                Some("table") => Some(match w[1] {
                    "compile" => ABBREV_TABLE.to_vec(),
                    "addrbase" => ABBREV_TABLE_ADDR_BASE.to_vec(),
                    _ => ABBREV_TYPE_UNIT.to_vec(),
                }),
                _ => None,
            };
            match bytes {
                Some(b) => {
                    for x in b {
                        lines.push_str(&format!("{x:02x}"));
                    }
                }
                None => lines.push_str("none"),
            }
            lines.push('\n');
        }
        std::fs::write(out, lines).unwrap();
    }
}

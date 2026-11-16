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

/// The neutral unit of `size` bytes for debug section `name` (IN-51b), or
/// `None` when `size` is below the form's minimum or the section has no
/// neutral form. `version` is the DWARF version of the unit
/// (`.debug_info` and `.debug_line` write version 4 for 2 to 4, else 5;
/// the image writes 5, whatever the contributions around it, since a
/// unit's version is its own); `addr_size` the target's address size;
/// `abbrev` the offset of the neutral abbreviation table in
/// `.debug_abbrev`.
#[must_use]
pub fn unit(name: &[u8], size: u64, version: u16, addr_size: u8, abbrev: u32) -> Option<Vec<u8>> {
    let len = usize::try_from(size).ok()?;
    let length = u32::try_from(size.saturating_sub(4)).ok()?;
    if size < 4 && !matches!(name, b".debug_abbrev" | b".debug_ranges") {
        return None;
    }
    let v5 = !(2..=4).contains(&version);
    let mut out = Vec::with_capacity(len);
    match name {
        b".debug_abbrev" | b".debug_ranges" => {}
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
            let rest = size.checked_sub(out.len() as u64)?;
            // The block's length L in n ULEB bytes (padded with 0x80
            // continuation bytes when L needs fewer), L + n = rest.
            let n = (1..=10u64).find(|&n| rest >= n && uleb_len(rest - n) <= n)?;
            uleb_padded(&mut out, rest - n, n);
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
        b".debug_rnglists" => {
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
        assert!(unit(b".debug_frame", 16, 5, 8, 0).is_none());
    }
}

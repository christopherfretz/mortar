//! The neutral forms of the debug sections (`spec/plumb.md` §6.8, P-I6;
//! `spec/incr.md` IN-51, IN-51b), judged from the output's bytes and the
//! map's `neutral` lines alone.
//!
//! Every byte this module expects is IN-51b's table, written out here
//! from the spec (never from the linker's writer): a `neutral` range is a
//! sequence of its section's forms tiling it, each exact. The checks that
//! join sections are here too: the one 38-byte abbreviation table at the
//! end of `.debug_abbrev` (the map's `neutral` line there names it), the
//! units' abbreviation offset `T`, and the address bases of codes 3 and 4
//! (each names a neutral `.debug_addr` table of the matching form).
//!
//! The judgement is a pure function of a [`View`] so that the forms and
//! their mutations are tested byte by byte (the tests below), apart from
//! any link.
#![allow(clippy::many_single_char_names)] // L, A, T, B, D, k: IN-51b's notation
#![allow(clippy::too_many_lines)] // the test table: one row per (form, mutation)

use std::collections::{BTreeMap, BTreeSet};

/// IN-51's neutral abbreviation table: codes 1 to 4 and the table's end.
pub(crate) const TABLE: [u8; 38] = [
    0x01, 0x11, 0x00, 0x80, 0x40, 0x09, 0x00, 0x00, // 1 compile_unit: lo_user block
    0x02, 0x41, 0x00, 0x80, 0x40, 0x09, 0x00, 0x00, // 2 type_unit: lo_user block
    0x03, 0x11, 0x00, 0x73, 0x17, 0x80, 0x40, 0x09, 0x00, 0x00, // 3 + addr_base
    0x04, 0x11, 0x00, 0xb3, 0x42, 0x17, 0x80, 0x40, 0x09, 0x00, 0x00, // 4 + GNU_addr_base
    0x00,
];

/// The sections IN-51b gives a neutral form (P-I2's list).
pub(crate) const SECTIONS: &[&str] = &[
    ".debug_info",
    ".debug_types",
    ".debug_abbrev",
    ".debug_line",
    ".debug_rnglists",
    ".debug_loclists",
    ".debug_str_offsets",
    ".debug_addr",
    ".debug_ranges",
    ".debug_loc",
    ".debug_frame",
    ".debug_macro",
    ".debug_macinfo",
];

/// A `neutral` line resolved to its section: `[lo, hi)` are offsets in
/// the section; `id` is the caller's.
#[derive(Debug, Clone)]
pub(crate) struct NRange<'a> {
    pub sec: &'a str,
    pub lo: usize,
    pub hi: usize,
    pub id: usize,
}

/// What the judgement reads.
pub(crate) struct View<'a> {
    /// `A`: the output's address size (8 in ELFCLASS64, 4 in ELFCLASS32).
    pub asz: u8,
    /// The debug sections' bytes, by name.
    pub data: BTreeMap<&'a str, &'a [u8]>,
    /// The `neutral` lines of the debug sections.
    pub ranges: Vec<NRange<'a>>,
    /// The map says `incremental` (IN-51: every incremental-mode output
    /// with a `.debug_abbrev` holds the table).
    pub incremental: bool,
}

/// A form that is not IN-51b's: the range (`None`: the output as a
/// whole), the section offset, the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Bad {
    pub id: Option<usize>,
    pub off: usize,
    pub msg: String,
}

/// An address base a neutral unit keeps (codes 3 and 4).
struct Base {
    code: u8,
    value: u64,
    /// The unit's offset in `.debug_info`, and its range.
    unit: usize,
    id: usize,
}

fn u16_at(b: &[u8], p: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(p..p + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], p: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(p..p + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], p: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(p..p + 8)?.try_into().ok()?))
}

/// The first byte of `b[from..]` that is not `want`: its index in `b`.
fn first_not(b: &[u8], from: usize, want: u8) -> Option<usize> {
    b.get(from..)?.iter().position(|&x| x != want).map(|i| from + i)
}

/// IN-51b's `B`: the `k`-byte ULEB128 of `n` and `n` zeros, `k` the
/// smallest with `n + k = rest` and `n >= 1`.
pub(crate) fn block(rest: usize) -> Option<Vec<u8>> {
    let k = (1..=10).find(|&k| rest > k && uleb_len((rest - k) as u64) <= k)?;
    let n = (rest - k) as u64;
    let mut v: Vec<u8> =
        (0..k).map(|i| ((n >> (7 * i)) & 0x7f) as u8 | if i + 1 < k { 0x80 } else { 0 }).collect();
    v.resize(rest, 0);
    Some(v)
}

fn uleb_len(n: u64) -> usize {
    let mut k = 1;
    let mut n = n >> 7;
    while n != 0 {
        k += 1;
        n >>= 7;
    }
    k
}

/// The block that ends unit `u` at `p` (the bytes after the DIE's code
/// and its address base).
fn check_block(u: &[u8], p: usize, unit_at: usize) -> Result<(), (usize, String)> {
    let rest = u.len().saturating_sub(p);
    let Some(want) = block(rest) else {
        return Err((
            unit_at + p,
            format!(
                "a neutral unit's block holds at least one byte and ends the unit (incr.md IN-51): {rest} bytes left for it"
            ),
        ));
    };
    let k = want.iter().position(|&x| x & 0x80 == 0).map_or(1, |i| i + 1);
    if u[p..p + k] != want[..k] {
        return Err((
            unit_at + p,
            format!(
                "a neutral unit's block length is {:02x?}, not IN-51b's B {:02x?} (the smallest ULEB width ending the block at the unit's end)",
                &u[p..p + k],
                &want[..k]
            ),
        ));
    }
    if let Some(i) = first_not(u, p + k, 0) {
        return Err((unit_at + i, format!("a neutral unit's block is zero: byte {:#04x}", u[i])));
    }
    Ok(())
}

/// One length-prefixed form: `u32(L - 4)` and `L` within `[p, hi)`.
fn sized(b: &[u8], p: usize, hi: usize, min: usize) -> Result<usize, (usize, String)> {
    let Some(len) = u32_at(b, p).filter(|_| p + 4 <= hi) else {
        return Err((p, format!("{} bytes left: no form fits (minimum {min})", hi - p)));
    };
    if len >= 0xffff_fff0 {
        return Err((p, format!("a neutral form has a 32-bit length, not {len:#x}")));
    }
    let l = len as usize + 4;
    if p + l > hi {
        return Err((p, format!("length {len:#x} runs past the range ({:#x} bytes left)", hi - p)));
    }
    if l < min {
        return Err((
            p,
            format!("a form of {l} bytes is below the minimum {min} (incr.md IN-51b)"),
        ));
    }
    Ok(l)
}

/// `b[p..p + want.len()]` is `want`, byte for byte.
fn exact(b: &[u8], p: usize, want: &[u8], what: &str) -> Result<(), (usize, String)> {
    for (i, &w) in want.iter().enumerate() {
        match b.get(p + i) {
            Some(&x) if x == w => {}
            Some(&x) => {
                return Err((
                    p + i,
                    format!("{what}: byte {x:#04x}, not {w:#04x} (incr.md IN-51b)"),
                ));
            }
            None => return Err((p + i, format!("{what}: the form is shorter than its header"))),
        }
    }
    Ok(())
}

/// The ranges of section `name`.
fn of<'v, 'a>(v: &'v View<'a>, name: &'v str) -> impl Iterator<Item = &'v NRange<'a>> + 'v {
    v.ranges.iter().filter(move |r| r.sec == name)
}

/// Judges every neutral range of `v`.
#[allow(clippy::too_many_lines)] // one pass per section of IN-51b's table
pub(crate) fn judge(v: &View<'_>) -> Vec<Bad> {
    let mut out = Vec::new();
    let a = usize::from(v.asz);
    let mut bad = |id: Option<usize>, (off, msg): (usize, String)| {
        if out.len() < 10_000 {
            out.push(Bad { id, off, msg });
        }
    };

    // The one table (IN-51): the last 38 bytes of `.debug_abbrev`, inside
    // one `neutral` line.
    let mut table: Option<usize> = None;
    let abbrev = v.data.get(".debug_abbrev").copied();
    let needs_table =
        v.incremental || v.ranges.iter().any(|r| matches!(r.sec, ".debug_info" | ".debug_types"));
    if let Some(ab) = abbrev
        && needs_table
    {
        let t = ab.len().checked_sub(TABLE.len());
        let line = t.and_then(|t| of(v, ".debug_abbrev").find(|r| r.lo <= t && r.hi == ab.len()));
        match (t, line) {
            (Some(t), Some(r)) => {
                if let Some(i) = (0..TABLE.len()).find(|&i| ab[t + i] != TABLE[i]) {
                    bad(
                        Some(r.id),
                        (
                            t + i,
                            format!(
                                "the last 38 bytes of .debug_abbrev are IN-51's neutral abbreviation table: byte {:#04x}, not {:#04x}",
                                ab[t + i],
                                TABLE[i]
                            ),
                        ),
                    );
                }
                table = Some(t);
            }
            _ => bad(
                None,
                (
                    ab.len(),
                    "an incremental-mode output with .debug_abbrev holds IN-51's 38-byte neutral abbreviation table at the section's end, in a 'neutral' line ending there (incr.md IN-51)".into(),
                ),
            ),
        }
    }
    // Zero bytes in every other neutral `.debug_abbrev` byte.
    if let Some(ab) = abbrev {
        for r in of(v, ".debug_abbrev") {
            let skip = table.filter(|&t| r.lo <= t && t < r.hi);
            for (p, &byte) in ab.iter().enumerate().take(r.hi).skip(r.lo) {
                if skip.is_some_and(|t| p >= t) {
                    break;
                }
                if byte != 0 {
                    bad(
                        Some(r.id),
                        (
                            p,
                            format!(
                                "a neutral .debug_abbrev range is zero bytes but IN-51's one table at the section's end: byte {byte:#04x}"
                            ),
                        ),
                    );
                    break;
                }
            }
        }
    }
    let t_want = table.map(|t| t as u64);
    let name_t = |t: u32, at: usize| -> Result<(), (usize, String)> {
        match t_want {
            Some(w) if u64::from(t) == w => Ok(()),
            Some(w) => Err((
                at,
                format!(
                    "a neutral unit names the abbreviation table at {t:#x}, not IN-51's at {w:#x}"
                ),
            )),
            None => {
                Err((at, format!("a neutral unit names abbreviation table {t:#x}: no IN-51 table")))
            }
        }
    };

    // `.debug_info`: block units, codes 1, 3 (v5) and 4 (v2-v4).
    let mut bases: Vec<Base> = Vec::new();
    if let Some(info) = v.data.get(".debug_info").copied() {
        for r in of(v, ".debug_info") {
            let mut p = r.lo;
            while p < r.hi {
                let res = sized(info, p, r.hi, 14).and_then(|l| {
                    let u = &info[p..p + l];
                    let ver = u16_at(u, 4).unwrap_or(0);
                    let (t, asz, code_at) = match ver {
                        5 => {
                            exact(u, 6, &[1], "a neutral DWARF 5 unit is DW_UT_compile (1)")
                                .map_err(|(o, m)| (p + o, m))?;
                            (u32_at(u, 8).unwrap_or(0), u[7], 12)
                        }
                        2..=4 => (u32_at(u, 6).unwrap_or(0), u[10], 11),
                        _ => {
                            return Err((
                                p + 4,
                                format!("a neutral .debug_info unit has version 2 to 5, not {ver}"),
                            ));
                        }
                    };
                    if usize::from(asz) != a {
                        return Err((
                            p + if ver == 5 { 7 } else { 10 },
                            format!("a neutral unit's address size is the output's ({a}), not {asz}"),
                        ));
                    }
                    name_t(t, p + if ver == 5 { 8 } else { 6 })?;
                    let code = u[code_at];
                    let mut q = code_at + 1;
                    match (code, ver) {
                        (1, _) => {}
                        (3, 5) | (4, 2..=4) => {
                            let Some(value) = u32_at(u, q).filter(|_| q + 4 <= l) else {
                                return Err((p + q, "the unit is shorter than its address base".into()));
                            };
                            bases.push(Base { code, value: value.into(), unit: p, id: r.id });
                            q += 4;
                        }
                        (3, _) => {
                            return Err((
                                p + code_at,
                                format!("abbreviation code 3 (DW_AT_addr_base) is in a DWARF 5 unit only, not version {ver} (incr.md IN-51b)"),
                            ));
                        }
                        (4, _) => {
                            return Err((
                                p + code_at,
                                "abbreviation code 4 (DW_AT_GNU_addr_base) is in a DWARF 2-4 unit only, not version 5 (incr.md IN-51b)".into(),
                            ));
                        }
                        _ => {
                            return Err((
                                p + code_at,
                                format!("a neutral unit's DIE has abbreviation code 1, 3 or 4, not {code:#04x}"),
                            ));
                        }
                    }
                    check_block(u, q, p)?;
                    Ok(l)
                });
                match res {
                    Ok(l) => p += l,
                    Err(e) => {
                        bad(Some(r.id), e);
                        break;
                    }
                }
            }
        }
    }

    // `.debug_types`: version-4 type units, code 2, the scheme's signature.
    if let Some(ty) = v.data.get(".debug_types").copied() {
        for r in of(v, ".debug_types") {
            let mut p = r.lo;
            while p < r.hi {
                let res = sized(ty, p, r.hi, 26).and_then(|l| {
                    let u = &ty[p..p + l];
                    exact(u, 4, &[4, 0], "a neutral .debug_types unit is version 4")
                        .map_err(|(o, m)| (p + o, m))?;
                    name_t(u32_at(u, 6).unwrap_or(0), p + 6)?;
                    if usize::from(u[10]) != a {
                        return Err((p + 10, format!("a neutral unit's address size is the output's ({a}), not {}", u[10])));
                    }
                    let want = 0xdead_u64 << 48 | p as u64;
                    let s = u64_at(u, 11).unwrap_or(0);
                    if s != want || (p as u64) >> 48 != 0 {
                        return Err((
                            p + 11,
                            format!("a neutral type unit's signature is 0xdead << 48 | its offset ({want:#x}), not {s:#x} (incr.md IN-51b)"),
                        ));
                    }
                    let mut tail = 23u32.to_le_bytes().to_vec();
                    tail.push(2);
                    exact(u, 19, &tail, "a neutral type unit's type_offset is 23 and its DIE code 2")
                        .map_err(|(o, m)| (p + o, m))?;
                    check_block(u, 24, p)?;
                    Ok(l)
                });
                match res {
                    Ok(l) => p += l,
                    Err(e) => {
                        bad(Some(r.id), e);
                        break;
                    }
                }
            }
        }
    }

    // `.debug_addr`: the v5 header form, or zeros where a code-4 unit's
    // base names the start.
    let gnu: BTreeSet<u64> = bases.iter().filter(|b| b.code == 4).map(|b| b.value).collect();
    let mut points = gnu.clone();
    points.extend(bases.iter().filter(|b| b.code == 3).map(|b| b.value.wrapping_sub(8)));
    let mut v5_starts: BTreeSet<u64> = BTreeSet::new();
    let mut gnu_starts: BTreeSet<u64> = BTreeSet::new();
    if let Some(ad) = v.data.get(".debug_addr").copied() {
        for r in of(v, ".debug_addr") {
            let mut p = r.lo;
            while p < r.hi {
                let res = if gnu.contains(&(p as u64)) {
                    let end = points
                        .range(p as u64 + 1..)
                        .next()
                        .map_or(r.hi, |&e| usize::try_from(e).unwrap_or(r.hi).min(r.hi));
                    let l = end - p;
                    if l < a || !l.is_multiple_of(a) {
                        Err((
                            p,
                            format!(
                                "a headerless neutral .debug_addr table is a multiple of {a} bytes, at least {a}, not {l}"
                            ),
                        ))
                    } else if let Some(i) = first_not(&ad[..end], p, 0) {
                        Err((
                            i,
                            format!(
                                "a headerless neutral .debug_addr table (named by a DW_AT_GNU_addr_base) is zero bytes: byte {:#04x}",
                                ad[i]
                            ),
                        ))
                    } else {
                        gnu_starts.insert(p as u64);
                        Ok(l)
                    }
                } else {
                    sized(ad, p, r.hi, 8).and_then(|l| {
                        exact(ad, p + 4, &[5, 0, v.asz, 0], "a neutral .debug_addr table's header is 05 00 A 00")?;
                        if (l - 8) % a != 0 {
                            return Err((p, format!("a neutral .debug_addr table holds whole {a}-byte entries: {} bytes", l - 8)));
                        }
                        if let Some(i) = first_not(&ad[..p + l], p + 8, 0) {
                            return Err((i, format!("a neutral .debug_addr table's entries are zero: byte {:#04x}", ad[i])));
                        }
                        v5_starts.insert(p as u64);
                        Ok(l)
                    })
                };
                match res {
                    Ok(l) => p += l,
                    Err(e) => {
                        bad(Some(r.id), e);
                        break;
                    }
                }
            }
        }
    }
    for b in &bases {
        let ok = if b.code == 3 {
            b.value.checked_sub(8).is_some_and(|d| v5_starts.contains(&d))
        } else {
            gnu_starts.contains(&b.value)
        };
        if !ok {
            let what = if b.code == 3 {
                "DW_AT_addr_base is 8 past the start of a neutral .debug_addr table in v5 form"
            } else {
                "DW_AT_GNU_addr_base is the start of a headerless neutral .debug_addr table"
            };
            bad(
                Some(b.id),
                (b.unit, format!("the neutral unit's {what} (incr.md IN-51), not {:#x}", b.value)),
            );
        }
    }

    // The other sections: each range tiled by its forms.
    for r in &v.ranges {
        let Some(d) = v.data.get(r.sec).copied() else { continue };
        let zeros = |min: usize| -> Result<(), (usize, String)> {
            let l = r.hi - r.lo;
            if l < min {
                return Err((
                    r.lo,
                    format!("a neutral {} range is at least {min} zero bytes, not {l}", r.sec),
                ));
            }
            match first_not(&d[..r.hi], r.lo, 0) {
                Some(i) => {
                    Err((i, format!("a neutral {} range is zero bytes: byte {:#04x}", r.sec, d[i])))
                }
                None => Ok(()),
            }
        };
        let res = match r.sec {
            ".debug_ranges" | ".debug_loc" => zeros(2 * a),
            ".debug_macinfo" => zeros(1),
            ".debug_line" | ".debug_rnglists" | ".debug_loclists" | ".debug_str_offsets"
            | ".debug_frame" | ".debug_macro" => {
                let mut p = r.lo;
                let mut res = Ok(());
                while p < r.hi {
                    match form(r.sec, d, p, r.hi, v.asz) {
                        Ok(l) => p += l,
                        Err(e) => {
                            res = Err(e);
                            break;
                        }
                    }
                }
                res
            }
            ".debug_info" | ".debug_types" | ".debug_abbrev" | ".debug_addr" => Ok(()),
            other => Err((r.lo, format!("'{other}' has no neutral form (incr.md IN-51b)"))),
        };
        if let Err(e) = res {
            bad(Some(r.id), e);
        }
    }
    out
}

/// One form of a section with self-contained forms at `p`: its length.
fn form(name: &str, d: &[u8], p: usize, hi: usize, asz: u8) -> Result<usize, (usize, String)> {
    let body =
        |from: usize, end: usize, fill: u8, what: &str| match first_not(&d[..end], from, fill) {
            Some(i) => Err((i, format!("{what}: byte {:#04x}, not {fill:#04x}", d[i]))),
            None => Ok(()),
        };
    match name {
        ".debug_line" => {
            let l = sized(d, p, hi, 24)?;
            let ver = u16_at(d, p + 4).unwrap_or(0);
            let (head, min): (Vec<u8>, usize) = if ver == 5 {
                let mut h = vec![5, 0, asz, 0, 20, 0, 0, 0];
                h.extend_from_slice(&[1, 1, 1, 0xfb, 0x0e, 7, 0, 1, 1, 1, 1, 0]);
                h.extend_from_slice(&[1, 1, 8, 0, 1, 1, 8, 0]);
                (h, 32)
            } else {
                let mut h = vec![4, 0, 14, 0, 0, 0];
                h.extend_from_slice(&[1, 1, 1, 0xfb, 0x0e, 7, 0, 1, 1, 1, 1, 0, 0, 0]);
                (h, 24)
            };
            if l < min {
                return Err((
                    p,
                    format!(
                        "a neutral line table of version {ver} is at least {min} bytes, not {l}"
                    ),
                ));
            }
            exact(d, p + 4, &head, "a neutral line table's header")?;
            body(p + min, p + l, 6, "a neutral line table's program is DW_LNS_negate_stmt")?;
            Ok(l)
        }
        ".debug_rnglists" | ".debug_loclists" => {
            let l = sized(d, p, hi, 12)?;
            exact(
                d,
                p + 4,
                &[5, 0, asz, 0, 0, 0, 0, 0],
                "a neutral list table's header is 05 00 A 00 u32(0)",
            )?;
            body(p + 12, p + l, 0, "a neutral list table's body is zero")?;
            Ok(l)
        }
        ".debug_str_offsets" => {
            let l = sized(d, p, hi, 8)?;
            exact(
                d,
                p + 4,
                &[5, 0, 0, 0],
                "a neutral .debug_str_offsets table's header is 05 00 00 00",
            )?;
            if l % 4 != 0 {
                return Err((
                    p,
                    format!(
                        "a neutral .debug_str_offsets table holds whole 4-byte entries: {l} bytes"
                    ),
                ));
            }
            body(p + 8, p + l, 0, "a neutral .debug_str_offsets table's entries are zero")?;
            Ok(l)
        }
        ".debug_frame" => {
            let l = sized(d, p, hi, 13)?;
            exact(
                d,
                p + 4,
                &[0xff, 0xff, 0xff, 0xff, 1, 0, 1, 0x7c, 0x1e],
                "a neutral .debug_frame record is IN-51b's CIE (version 1, \"\", 1, -4, column 30)",
            )?;
            body(p + 13, p + l, 0, "a neutral CIE's instructions are DW_CFA_nop")?;
            Ok(l)
        }
        ".debug_macro" => {
            // u16(V) 00 02 00, 'x' bytes, 00 00.
            let ver = u16_at(d, p).filter(|_| p + 7 <= hi);
            match ver {
                Some(4 | 5) => {}
                Some(v) => {
                    return Err((
                        p,
                        format!("a neutral .debug_macro unit is version 4 or 5, not {v}"),
                    ));
                }
                None => {
                    return Err((p, format!("{} bytes left: no form fits (minimum 7)", hi - p)));
                }
            }
            exact(
                d,
                p + 2,
                &[0, 2, 0],
                "a neutral .debug_macro unit is flags 0, DW_MACRO_undef, line 0",
            )?;
            let s = p + 5;
            let e = (s..hi).find(|&i| d[i] != b'x').unwrap_or(hi);
            exact(d, e, &[0, 0], "a neutral .debug_macro unit's string of 'x' ends with 00 00")
                .and_then(|()| {
                    if e + 2 > hi {
                        Err((e, "the unit runs past the range".into()))
                    } else {
                        Ok(())
                    }
                })?;
            Ok(e + 2 - p)
        }
        _ => Err((p, format!("'{name}' has no neutral form (incr.md IN-51b)"))),
    }
}

#[cfg(test)]
mod tests {
    //! IN-51b's table, both directions: every form ACCEPTED at its
    //! minimum, minimum + 1 (+ granule), an odd and a long length, at
    //! address sizes 4 and 8; every mutation REJECTED with an offset in
    //! the form it broke. The writers below are IN-51b's bytes, written
    //! out from the spec's notation.

    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Form {
        Info5,
        Info4,
        Info5Base,
        Info4Base,
        InfoType5,
        Types,
        Abbrev,
        Line5,
        Line4,
        Rnglists,
        Loclists,
        StrOffsets,
        Addr5,
        AddrGnu,
        Ranges,
        Loc,
        Frame,
        Macro5,
        Macro4,
        Macinfo,
    }
    use Form::*;

    /// One per row of IN-51b's table that has a form (the type-unit row
    /// of `.debug_info`, the two list sections and the two `.debug_macro`
    /// versions apart).
    const FORMS: [Form; 20] = [
        Info5, Info4, Info5Base, Info4Base, InfoType5, Types, Abbrev, Line5, Line4, Rnglists,
        Loclists, StrOffsets, Addr5, AddrGnu, Ranges, Loc, Frame, Macro5, Macro4, Macinfo,
    ];

    /// Live bytes before the form under test (outside every range).
    const PRE: usize = 16;
    /// The table's offset in the fixture's `.debug_abbrev` (6 live
    /// bytes, a 2-byte zero line, the table).
    const T: u32 = 8;

    impl Form {
        fn sec(self) -> &'static str {
            match self {
                Info5 | Info4 | Info5Base | Info4Base | InfoType5 => ".debug_info",
                Types => ".debug_types",
                Abbrev => ".debug_abbrev",
                Line5 | Line4 => ".debug_line",
                Rnglists => ".debug_rnglists",
                Loclists => ".debug_loclists",
                StrOffsets => ".debug_str_offsets",
                Addr5 | AddrGnu => ".debug_addr",
                Ranges => ".debug_ranges",
                Loc => ".debug_loc",
                Frame => ".debug_frame",
                Macro5 | Macro4 => ".debug_macro",
                Macinfo => ".debug_macinfo",
            }
        }
        /// (minimum, granule), IN-51b's last column.
        fn min(self, a: usize) -> (usize, usize) {
            match self {
                Info5 | InfoType5 => (15, 1),
                Info4 => (14, 1),
                Info5Base => (19, 1),
                Info4Base => (18, 1),
                Types => (26, 1),
                Abbrev | Macinfo => (1, 1),
                Line5 => (32, 1),
                Line4 => (24, 1),
                Rnglists | Loclists => (12, 1),
                StrOffsets => (8, 4),
                Addr5 => (8, a),
                AddrGnu => (a, a),
                Ranges | Loc => (2 * a, 1),
                Frame => (13, 1),
                Macro5 | Macro4 => (7, 1),
            }
        }
        /// The header after the length field: `[from, to)` of the form.
        fn header(self) -> Option<(usize, usize)> {
            Some(match self {
                Info5 | InfoType5 | Frame => (4, 13),
                Info4 | Rnglists | Loclists => (4, 12),
                Info5Base => (4, 17),
                Info4Base => (4, 16),
                Types | Line4 => (4, 24),
                Line5 => (4, 32),
                StrOffsets | Addr5 => (4, 8),
                Macro5 | Macro4 => (0, 5),
                Abbrev | AddrGnu | Ranges | Loc | Macinfo => return None,
            })
        }
        fn sized(self) -> bool {
            !matches!(self, Abbrev | AddrGnu | Ranges | Loc | Macinfo | Macro5 | Macro4)
        }
    }

    fn u32le(v: usize) -> [u8; 4] {
        u32::try_from(v).unwrap().to_le_bytes()
    }

    /// IN-51b's `B` for `rest` bytes, written apart from [`block`].
    fn b(rest: usize) -> Vec<u8> {
        let k = (1..=10)
            .find(|&k| rest > k && (rest - k) >> (7 * k) == 0)
            .expect("a block of at least one byte");
        let n = rest - k;
        let mut v: Vec<u8> = (0..k)
            .map(|i| ((n >> (7 * i)) & 0x7f) as u8 | if i + 1 < k { 0x80 } else { 0 })
            .collect();
        v.resize(rest, 0);
        v
    }

    /// The form's `l` bytes at section offset `at`; `d` the address
    /// table a base names.
    fn write(f: Form, l: usize, a: u8, at: usize, d: usize) -> Vec<u8> {
        let mut v = Vec::new();
        if f.sized() {
            v.extend(u32le(l - 4));
        }
        match f {
            Info5 | InfoType5 | Info5Base => {
                v.extend([5, 0, 1, a]);
                v.extend(T.to_le_bytes());
                if f == Info5Base {
                    v.push(3);
                    v.extend(u32le(d + 8));
                } else {
                    v.push(1);
                }
            }
            Info4 | Info4Base => {
                v.extend([4, 0]);
                v.extend(T.to_le_bytes());
                v.push(a);
                if f == Info4Base {
                    v.push(4);
                    v.extend(u32le(d));
                } else {
                    v.push(1);
                }
            }
            Types => {
                v.extend([4, 0]);
                v.extend(T.to_le_bytes());
                v.push(a);
                v.extend((0xdead_u64 << 48 | at as u64).to_le_bytes());
                v.extend(23u32.to_le_bytes());
                v.push(2);
            }
            Line5 => {
                v.extend([5, 0, a, 0]);
                v.extend(20u32.to_le_bytes());
                v.extend([1, 1, 1, 0xfb, 0x0e, 7, 0, 1, 1, 1, 1, 0, 1, 1, 8, 0, 1, 1, 8, 0]);
                v.resize(l, 6);
            }
            Line4 => {
                v.extend([4, 0]);
                v.extend(14u32.to_le_bytes());
                v.extend([1, 1, 1, 0xfb, 0x0e, 7, 0, 1, 1, 1, 1, 0, 0, 0]);
                v.resize(l, 6);
            }
            Rnglists | Loclists => v.extend([5, 0, a, 0, 0, 0, 0, 0]),
            StrOffsets => v.extend([5, 0, 0, 0]),
            Addr5 => v.extend([5, 0, a, 0]),
            Frame => v.extend([0xff, 0xff, 0xff, 0xff, 1, 0, 1, 0x7c, 0x1e]),
            Macro5 | Macro4 => {
                v.extend([if f == Macro5 { 5 } else { 4 }, 0, 0, 2, 0]);
                v.resize(l - 2, b'x');
                v.extend([0, 0]);
            }
            Abbrev | AddrGnu | Ranges | Loc | Macinfo => {}
        }
        if matches!(f, Info5 | InfoType5 | Info5Base | Info4 | Info4Base | Types) {
            let rest = l - v.len();
            v.extend(b(rest));
        }
        v.resize(l, 0);
        assert_eq!(v.len(), l);
        v
    }

    /// A fixture: sections, `neutral` lines, the address size.
    #[derive(Clone)]
    struct Case {
        secs: BTreeMap<&'static str, Vec<u8>>,
        ranges: Vec<(&'static str, usize, usize)>,
        a: u8,
    }

    impl Case {
        fn judge(&self) -> Vec<Bad> {
            let view = View {
                asz: self.a,
                data: self.secs.iter().map(|(k, v)| (*k, v.as_slice())).collect(),
                ranges: self
                    .ranges
                    .iter()
                    .enumerate()
                    .map(|(id, &(sec, lo, hi))| NRange { sec, lo, hi, id })
                    .collect(),
                incremental: true,
            };
            judge(&view)
        }
        fn put(&mut self, sec: &'static str, at: usize, bytes: &[u8], range: bool) {
            let s = self.secs.entry(sec).or_default();
            assert!(s.len() <= at);
            s.resize(at, 0xaa);
            s.extend_from_slice(bytes);
            if range {
                self.ranges.push((sec, at, at + bytes.len()));
            }
        }
    }

    /// The fixture of form `f` of `l` bytes: `(case, offset of the form)`.
    fn case(f: Form, l: usize, a: u8) -> (Case, usize) {
        let mut c = Case { secs: BTreeMap::new(), ranges: Vec::new(), a };
        let mut ab = vec![1, 0x11, 0, 0, 0, 0];
        if f == Abbrev {
            // The form is the zero line before the table: T moves.
            let at = ab.len();
            ab.resize(at + l, 0);
            c.ranges.push((".debug_abbrev", at, at + l));
            c.ranges.push((".debug_abbrev", at + l, at + l + TABLE.len()));
            ab.extend(TABLE);
            c.secs.insert(".debug_abbrev", ab);
            return (c, at);
        }
        ab.extend([0, 0]);
        ab.extend(TABLE);
        c.secs.insert(".debug_abbrev", ab);
        c.ranges.push((".debug_abbrev", 6, 8));
        c.ranges.push((".debug_abbrev", 8, 8 + TABLE.len()));
        match f {
            Info5Base => c.put(".debug_addr", 0, &write(Addr5, 8 + usize::from(a), a, 0, 0), true),
            Info4Base => c.put(".debug_addr", 0, &write(AddrGnu, a.into(), a, 0, 0), true),
            AddrGnu => c.put(".debug_info", 0, &write(Info4Base, 18, a, 0, PRE), true),
            _ => {}
        }
        c.put(f.sec(), PRE, &write(f, l, a, PRE, 0), true);
        c.secs.get_mut(f.sec()).unwrap().extend([0xbb; 3]);
        (c, PRE)
    }

    /// The lengths accepted: minimum, minimum + 1 (+ granule), odd, long.
    fn lengths(f: Form, a: usize) -> Vec<usize> {
        let (m, g) = f.min(a);
        let odd = if g == 1 { m + 6 + usize::from(m % 2 == 0) } else { m + 3 * g };
        vec![m, m + g, odd, if g == 1 { 301 } else { 304 }]
    }

    /// The mutations of form `f` (of `l` bytes at `at` in `c`): rows
    /// `(name, case)`; each must be a finding.
    fn mutations(f: Form, l: usize, a: u8) -> Vec<(String, Case, usize)> {
        let (c0, at) = case(f, l, a);
        let sec = f.sec();
        let mut rows = Vec::new();
        let mut row = |name: String, m: &dyn Fn(&mut Case)| {
            let mut c = c0.clone();
            m(&mut c);
            rows.push((name, c, at));
        };
        if let Some((from, to)) = f.header() {
            for i in from..to {
                row(format!("header byte {i} changed"), &|c| {
                    c.secs.get_mut(sec).unwrap()[at + i] ^= 0x40;
                });
            }
        }
        if f.sized() {
            for (name, delta) in [("length + 1", 1i64), ("length - 1", -1)] {
                row(name.into(), &|c| {
                    let s = c.secs.get_mut(sec).unwrap();
                    let v = u32::from_le_bytes(s[at..at + 4].try_into().unwrap());
                    let v = v.wrapping_add_signed(delta as i32);
                    s[at..at + 4].copy_from_slice(&v.to_le_bytes());
                });
            }
        }
        // One other byte in the body (the block's zeros, the 06, 78, 00
        // runs): the middle of the form's tail and its last byte.
        let body_from = match f {
            Macro5 | Macro4 => 5,
            _ => f.header().map_or(0, |h| h.1),
        };
        let body_to = if matches!(f, Macro5 | Macro4) { l - 2 } else { l };
        if body_to > body_from {
            for (name, i) in [
                ("body: a middle byte", usize::midpoint(body_from, body_to)),
                ("body: the last byte", body_to - 1),
            ] {
                row(name.into(), &|c| {
                    let s = c.secs.get_mut(sec).unwrap();
                    s[at + i] = if s[at + i] == 1 { 2 } else { 1 };
                });
            }
        }
        if matches!(f, Macro5 | Macro4) {
            row("the end 00 00 changed".into(), &|c| {
                c.secs.get_mut(sec).unwrap()[at + l - 1] = 1;
            });
        }
        // The block one byte short, a null entry after it.
        let die = match f {
            Info5 | InfoType5 => Some(13),
            Info4 => Some(12),
            Info5Base => Some(17),
            Info4Base => Some(16),
            Types => Some(24),
            _ => None,
        };
        if let Some(p) = die.filter(|&p| l >= p + 3) {
            row("the block one byte short, a null entry after it".into(), &|c| {
                let s = c.secs.get_mut(sec).unwrap();
                let mut short = b(l - p - 1);
                short.push(0);
                s[at + p..at + l].copy_from_slice(&short);
            });
        }
        // The range below its minimum (zero forms; a sized form below its
        // minimum is the length rows'), and a wrong granule.
        let (m, g) = f.min(usize::from(a));
        if !f.sized() && f != Macro5 && f != Macro4 && m > 1 && l == m {
            row("the range below the minimum".into(), &|c| {
                let r = c.ranges.iter_mut().find(|r| r.0 == sec && r.1 == at).unwrap();
                r.2 -= 1;
            });
        }
        if g > 1 && l == m {
            row("a length off the granule".into(), &|c| {
                let s = c.secs.get_mut(sec).unwrap();
                if f.sized() {
                    let v = u32::from_le_bytes(s[at..at + 4].try_into().unwrap()) + 1;
                    s[at..at + 4].copy_from_slice(&v.to_le_bytes());
                    s.insert(at + l, 0);
                } else {
                    s.insert(at + l, 0);
                }
                let r = c.ranges.iter_mut().find(|r| r.0 == sec && r.1 == at).unwrap();
                r.2 += 1;
            });
        }
        match f {
            Info5 | Info4 if l >= if f == Info5 { 19 } else { 18 } => {
                let (code, name) = if f == Info5 {
                    (4, "code 4 in a v5 unit")
                } else {
                    (3, "code 3 in a v4 unit")
                };
                row(name.into(), &|c| {
                    let s = c.secs.get_mut(sec).unwrap();
                    let p = if f == Info5 { 12 } else { 11 };
                    let mut u = s[at..at + p].to_vec();
                    u.push(code);
                    u.extend(u32le(0));
                    u.extend(b(l - p - 5));
                    s[at..at + l].copy_from_slice(&u);
                });
            }
            Info5Base => {
                row("a base 8 past no v5 table".into(), &|c| {
                    c.secs.get_mut(sec).unwrap()[at + 13..at + 17].copy_from_slice(&u32le(12));
                });
                row("the named table without its header".into(), &|c| {
                    let s = c.secs.get_mut(".debug_addr").unwrap();
                    s[..8].fill(0);
                });
            }
            Info4Base => {
                row("a base naming no neutral table".into(), &|c| {
                    c.secs.get_mut(sec).unwrap()[at + 12..at + 16].copy_from_slice(&u32le(1));
                });
                row("the named table with a v5 header".into(), &|c| {
                    let s = c.secs.get_mut(".debug_addr").unwrap();
                    s[..4].copy_from_slice(&u32le(usize::from(a) + 4));
                });
                row("the named table not neutral".into(), &|c| {
                    c.ranges.retain(|r| r.0 != ".debug_addr");
                });
            }
            AddrGnu => {
                row("a zero table no unit names".into(), &|c| {
                    let s = c.secs.get_mut(".debug_info").unwrap();
                    s[12..16].copy_from_slice(&u32le(0));
                    c.ranges.retain(|r| r.0 != ".debug_info");
                });
            }
            Types => {
                row("a signature off the scheme".into(), &|c| {
                    c.secs.get_mut(sec).unwrap()[at + 11] ^= 1;
                });
            }
            _ => {}
        }
        rows
    }

    #[test]
    fn in51b_every_form_is_accepted() {
        let mut n = 0;
        for a in [4u8, 8] {
            for f in FORMS {
                for l in lengths(f, a.into()) {
                    let (c, _) = case(f, l, a);
                    let got = c.judge();
                    assert!(got.is_empty(), "{f:?} of {l} bytes, A={a}: {got:#?}");
                    n += 1;
                }
            }
        }
        assert_eq!(n, 2 * FORMS.len() * 4);
    }

    #[test]
    fn in51b_every_mutation_is_a_finding() {
        let mut per: BTreeMap<String, usize> = BTreeMap::new();
        let mut total = 0;
        for a in [4u8, 8] {
            for f in FORMS {
                for l in lengths(f, a.into()) {
                    for (name, c, at) in mutations(f, l, a) {
                        let got = c.judge();
                        assert!(!got.is_empty(), "{f:?} of {l} bytes, A={a}, {name}: accepted");
                        // The finding names a range and an offset at or
                        // near the form (a base check names its unit).
                        assert!(
                            got.iter().any(|b| b.id.is_some() && b.off <= at + l + 8),
                            "{f:?} {l} A={a} {name}: {got:#?}"
                        );
                        *per.entry(format!("{f:?}")).or_default() += 1;
                        total += 1;
                    }
                }
            }
        }
        // Every form has rows, and every section the judgement knows has a
        // form here: a form added later without rows fails.
        for f in FORMS {
            assert!(per.get(&format!("{f:?}")).is_some_and(|&k| k >= 8), "{f:?}: {per:?}");
        }
        for s in SECTIONS {
            assert!(FORMS.iter().any(|f| f.sec() == *s), "{s}: no form in the table");
        }
        assert_eq!(FORMS.len(), 20);
        // The table's size, as spec/landed/plumb/WP-253.md records it: a
        // change to the table restates it there.
        assert_eq!(total, 1916, "mutation rows: {per:?}");
    }

    /// A second form after the first in one range, every pair of
    /// versions: "either version's form in any neutral range".
    #[test]
    fn in51b_versions_mix_in_one_range() {
        for (x, y) in
            [(Info5, Info4), (Info4, Info5), (Line5, Line4), (Line4, Line5), (Macro5, Macro4)]
        {
            let (mut c, at) = case(x, 40, 8);
            let second = write(y, 33, 8, at + 40, 0);
            let s = c.secs.get_mut(x.sec()).unwrap();
            s.truncate(at + 40);
            s.extend(&second);
            let r = c.ranges.iter_mut().find(|r| r.0 == x.sec() && r.1 == at).unwrap();
            r.2 += 33;
            assert!(c.judge().is_empty(), "{x:?} then {y:?}: {:#?}", c.judge());
        }
    }

    /// The one table: each byte changed, an abbreviation missing, not at
    /// the section's end, no line naming it, a second table elsewhere.
    #[test]
    fn in51_the_one_table() {
        let (c0, at) = case(Info5, 20, 8);
        assert!(c0.judge().is_empty());
        let mut rows: Vec<(String, Case)> = Vec::new();
        for i in 0..TABLE.len() {
            let mut c = c0.clone();
            c.secs.get_mut(".debug_abbrev").unwrap()[8 + i] ^= 0x40;
            rows.push((format!("table byte {i}"), c));
        }
        // Code 2's abbreviation missing (the table 8 bytes shorter).
        let mut c = c0.clone();
        let ab = c.secs.get_mut(".debug_abbrev").unwrap();
        ab.drain(16..24);
        let n = ab.len();
        c.ranges.retain(|r| r.0 != ".debug_abbrev" || r.1 != 8);
        c.ranges.push((".debug_abbrev", 8, n));
        rows.push(("an abbreviation missing".into(), c));
        // Not at the end: one live byte after it.
        let mut c = c0.clone();
        c.secs.get_mut(".debug_abbrev").unwrap().push(0);
        rows.push(("a byte after the table".into(), c));
        // The table one byte early in its line.
        let mut c = c0.clone();
        let ab = c.secs.get_mut(".debug_abbrev").unwrap();
        ab.remove(8);
        ab.push(0);
        rows.push(("the table one byte early".into(), c));
        // No line names it.
        let mut c = c0.clone();
        c.ranges.retain(|r| r.0 != ".debug_abbrev" || r.1 != 8);
        rows.push(("no line names the table".into(), c));
        // A second table in a zero range.
        let mut c = c0.clone();
        let ab = c.secs.get_mut(".debug_abbrev").unwrap();
        let mut v = vec![1, 0x11, 0, 0, 0, 0];
        v.extend(TABLE);
        v.extend(TABLE);
        *ab = v;
        c.ranges.retain(|r| r.0 != ".debug_abbrev");
        c.ranges.push((".debug_abbrev", 6, 44));
        c.ranges.push((".debug_abbrev", 44, 82));
        rows.push(("a second table elsewhere".into(), c));
        // The unit naming the second table, not the one at the end.
        let mut c2 = rows.last().unwrap().1.clone();
        c2.secs.get_mut(".debug_abbrev").unwrap()[6..44].fill(0);
        c2.secs.get_mut(".debug_info").unwrap()[at + 8..at + 12]
            .copy_from_slice(&6u32.to_le_bytes());
        rows.push(("a unit naming another offset".into(), c2));
        for (name, c) in rows {
            assert!(!c.judge().is_empty(), "{name}: accepted");
        }
        // No table at all in an incremental output with .debug_abbrev,
        // even without a dead unit.
        let mut c = Case { secs: BTreeMap::new(), ranges: Vec::new(), a: 8 };
        c.secs.insert(".debug_abbrev", vec![1, 0x11, 0, 0, 0, 0]);
        assert!(c.judge().iter().any(|b| b.id.is_none()));
        // ... and with it, nothing.
        c.secs.get_mut(".debug_abbrev").unwrap().extend(TABLE);
        c.ranges.push((".debug_abbrev", 6, 44));
        assert!(c.judge().is_empty(), "{:#?}", c.judge());
    }

    /// [`block`] is IN-51b's `B`: the smallest width, the padded ULEB.
    #[test]
    fn in51b_block_widths() {
        for rest in 2..20_000 {
            assert_eq!(block(rest).unwrap(), b(rest), "{rest}");
        }
        assert_eq!(block(129).unwrap()[..2], [0xff, 0x00]);
        assert_eq!(block(130).unwrap()[..2], [0x80, 0x01]);
        assert!(block(1).is_none());
    }
}

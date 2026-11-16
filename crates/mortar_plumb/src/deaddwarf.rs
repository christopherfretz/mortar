//! Dead ranges of the debug sections (`spec/plumb.md` §6.8;
//! `spec/incr.md` IN-3a checks 1 and 2, IN-51, IN-51b, IN-53).
//!
//! - **P-I4**: no DWARF form offset of a live unit points into a dead
//!   range: the abbreviation offset, `DW_FORM_ref_addr`, `DW_FORM_strp`,
//!   `DW_FORM_line_strp` (in `.debug_info` and in the `.debug_line`
//!   headers live units name), `DW_FORM_sec_offset` by attribute
//!   (`DW_AT_stmt_list`, the range and location lists, the four `_base`
//!   attributes, the macro tables), the entries of a live
//!   `.debug_str_offsets` table, `.debug_aranges`' unit offsets and a
//!   live `.debug_frame` FDE's CIE pointer.
//! - **P-I6**: a `neutral` range is a sequence of IN-51b's forms of its
//!   section; a `contribution` range is a chain of whole units (records)
//!   of its section with every address operand the tombstone (image.md
//!   I-47), and in a position-independent output none is in an
//!   address-bearing section (IN-53).
//!
//! A live unit the walk cannot read is a P-I4 finding naming it: the
//! check refuses rather than passes what it could not judge.

use mortar_elf::Class;
use mortar_elf::consts::SHF_ALLOC;

use crate::dead::{DeadSet, Range, position_independent};
use crate::image::Image;
use crate::locate::Obj;
use crate::map::{DeadKind, Map};
use crate::report::{Finding, Rule};

fn finding(n: u8, msg: String) -> Finding {
    Finding::new(Rule::Incr(n), msg)
}

/// A little-endian reader over a section's bytes.
#[derive(Clone)]
struct Rd<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Rd<'a> {
    fn new(b: &'a [u8], p: usize) -> Self {
        Rd { b, p }
    }
    fn n(&mut self, w: usize) -> Option<u64> {
        let s = self.b.get(self.p..self.p.checked_add(w)?)?;
        self.p += w;
        Some(s.iter().rev().fold(0u64, |a, &x| a << 8 | u64::from(x)))
    }
    fn u8(&mut self) -> Option<u8> {
        self.n(1).map(|v| v as u8)
    }
    fn u16(&mut self) -> Option<u16> {
        self.n(2).map(|v| v as u16)
    }
    fn u32(&mut self) -> Option<u32> {
        self.n(4).map(|v| v as u32)
    }
    fn skip(&mut self, n: u64) -> Option<()> {
        let n = usize::try_from(n).ok()?;
        let e = self.p.checked_add(n)?;
        (e <= self.b.len()).then(|| self.p = e)
    }
    fn uleb(&mut self) -> Option<u64> {
        let mut v = 0u64;
        for i in 0..10 {
            let b = *self.b.get(self.p)?;
            self.p += 1;
            if i < 9 || b <= 1 {
                v |= u64::from(b & 0x7f) << (7 * i);
            }
            if b & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }
    fn sleb(&mut self) -> Option<i64> {
        let mut v = 0i64;
        let mut shift = 0;
        loop {
            let b = *self.b.get(self.p)?;
            self.p += 1;
            if shift < 64 {
                v |= i64::from(b & 0x7f) << shift;
            }
            shift += 7;
            if b & 0x80 == 0 {
                if shift < 64 && b & 0x40 != 0 {
                    v |= -1i64 << shift;
                }
                return Some(v);
            }
            if shift > 70 {
                return None;
            }
        }
    }
    fn cstr(&mut self) -> Option<()> {
        let n = self.b.get(self.p..)?.iter().position(|&c| c == 0)?;
        self.p += n + 1;
        Some(())
    }
    /// An initial length: `(length, offset size)`.
    fn initial_length(&mut self) -> Option<(u64, usize)> {
        match self.u32()? {
            0xffff_ffff => Some((self.n(8)?, 8)),
            v if v >= 0xffff_fff0 => None,
            v => Some((u64::from(v), 4)),
        }
    }
}

/// The debug sections of the output, by name.
struct Debug<'a> {
    image: &'a Image<'a>,
}

impl<'a> Debug<'a> {
    fn index(&self, name: &str) -> Option<usize> {
        self.image.sections.iter().position(|s| s.name == name && s.shdr.sh_flags & SHF_ALLOC == 0)
    }
    fn data(&self, name: &str) -> Option<(usize, &'a [u8])> {
        let i = self.index(name)?;
        Some((i, self.image.section_data(i)?))
    }
}

/// I-47's tombstone of section `name` in an output of class `class`.
fn tombstone(name: &str, class: Class, width: usize) -> u64 {
    let ones = if width >= 8 { u64::MAX } else { (1u64 << (8 * width)) - 1 };
    match name {
        ".debug_ranges" | ".debug_loc" => 1,
        ".debug_rnglists" | ".debug_loclists" | ".debug_addr" => 0,
        ".debug_frame" => ones,
        _ if class == Class::Elf32 => 0,
        _ => ones,
    }
}

/// The checks of the debug sections' dead ranges.
pub(crate) fn check(image: &Image<'_>, _objs: &[Obj<'_>], _map: &Map, set: &DeadSet, out: &mut Vec<Finding>) {
    let dbg = Debug { image };
    let debug_ranges: Vec<&Range> = set
        .ranges
        .iter()
        .filter(|r| image.sections[r.sec].shdr.sh_flags & SHF_ALLOC == 0)
        .collect();
    for r in &debug_ranges {
        let name = image.sections[r.sec].name.as_str();
        let Some(data) = image.section_data(r.sec) else {
            continue;
        };
        let lo = (r.start - image.sections[r.sec].shdr.sh_addr) as usize;
        let hi = lo + (r.end - r.start) as usize;
        let Some(bytes) = data.get(lo..hi) else { continue };
        let res = match r.kind {
            Some(DeadKind::Neutral) => neutral(image, set, name, bytes, lo),
            Some(DeadKind::Contribution) => contribution(image, name, bytes, lo),
            _ => Ok(()),
        };
        if let Err(m) = res {
            out.push(finding(6, format!("{}: {m}", r.describe(image))).at(r.start));
        }
    }
    if debug_ranges.is_empty() && !set.ranges.iter().any(|r| crate::dead::is_eh_frame(image, r.sec)) {
        return;
    }
    live_references(&dbg, set, out);
}

/// Whether section `name`'s offset `off` lies in one of its dead ranges.
fn dead_at<'s>(dbg: &Debug<'_>, set: &'s DeadSet, name: &str, off: u64) -> Option<&'s Range> {
    let i = dbg.index(name)?;
    set.in_section(i, dbg.image.sections[i].shdr.sh_addr.wrapping_add(off))
}

// ---------------------------------------------------------------------
// Check 2: the forms of a dead range.

/// A `neutral` range: one or more of IN-51b's forms of `name`, tiling it.
fn neutral(image: &Image<'_>, set: &DeadSet, name: &str, b: &[u8], base: usize) -> Result<(), String> {
    match name {
        ".debug_abbrev" => neutral_abbrev(b, base),
        ".debug_ranges" => match b.iter().position(|&x| x != 0) {
            None if b.len() >= 16 => Ok(()),
            None => Err(format!("a neutral .debug_ranges range is at least 16 zero bytes, not {}", b.len())),
            Some(i) => Err(format!("a neutral .debug_ranges range is zero bytes: byte at offset {:#x}", base + i)),
        },
        ".debug_info" | ".debug_line" | ".debug_rnglists" | ".debug_loclists" | ".debug_str_offsets"
        | ".debug_addr" | ".debug_frame" => {
            let mut p = 0usize;
            while p < b.len() {
                let n = neutral_unit(image, set, name, &b[p..], base + p)
                    .map_err(|m| format!("at offset {:#x}: {m}", base + p))?;
                p += n;
            }
            Ok(())
        }
        _ => Err(format!("'{name}' has no neutral form (incr.md IN-51b)")),
    }
}

/// `.debug_abbrev`: zero bytes (empty tables) and IN-51's two tables.
fn neutral_abbrev(b: &[u8], base: usize) -> Result<(), String> {
    const PLAIN: &[u8] = &[1, 0x11, 0, 0x80, 0x40, 9, 0, 0];
    const ADDR: &[u8] = &[1, 0x11, 0, 0x73, 0x17, 0x80, 0x40, 9, 0, 0];
    let mut p = 0;
    while p < b.len() {
        if b[p] == 0 {
            p += 1;
        } else if b[p..].starts_with(PLAIN) {
            p += PLAIN.len();
        } else if b[p..].starts_with(ADDR) {
            p += ADDR.len();
        } else {
            return Err(format!(
                "a neutral .debug_abbrev range is zero bytes and IN-51's tables: byte {:#04x} at offset {:#x}",
                b[p],
                base + p
            ));
        }
    }
    Ok(())
}

/// One neutral unit at the start of `b`: its size.
fn neutral_unit(image: &Image<'_>, set: &DeadSet, name: &str, b: &[u8], off: usize) -> Result<usize, String> {
    let mut r = Rd::new(b, 0);
    let len = match r.u32() {
        Some(v) if v < 0xffff_fff0 => v as usize,
        Some(_) => return Err("a neutral form has a 32-bit initial length (incr.md IN-51b)".into()),
        None => return Err("a unit header runs past the range".into()),
    };
    let end = len.checked_add(4).filter(|&e| e <= b.len()).ok_or_else(|| {
        format!("unit_length {len:#x} runs past the range ({:#x} bytes left)", b.len())
    })?;
    let body = &b[..end];
    let mut r = Rd::new(body, 4);
    let short = || "the unit is shorter than its header".to_string();
    match name {
        ".debug_info" => {
            let v = r.u16().ok_or_else(short)?;
            let abbrev = match v {
                5 => {
                    let ut = r.u8().ok_or_else(short)?;
                    if ut != 1 {
                        return Err(format!("a neutral unit is DW_UT_compile (1), not unit type {ut}"));
                    }
                    let _asz = r.u8().ok_or_else(short)?;
                    r.u32().ok_or_else(short)?
                }
                2..=4 => {
                    let a = r.u32().ok_or_else(short)?;
                    let _asz = r.u8().ok_or_else(short)?;
                    a
                }
                _ => return Err(format!("unit version {v}")),
            };
            let code = r.uleb().ok_or_else(short)?;
            if code != 1 {
                return Err(format!("a neutral unit's DIE has abbreviation code 1, not {code}"));
            }
            // The table the unit names: IN-51's, with or without
            // `DW_AT_addr_base`.
            let dbg = Debug { image };
            let table = dbg.data(".debug_abbrev").and_then(|(_, d)| d.get(abbrev as usize..));
            let with_base = match table {
                Some(t) if t.starts_with(&[1, 0x11, 0, 0x80, 0x40, 9, 0, 0]) => false,
                Some(t) if t.starts_with(&[1, 0x11, 0, 0x73, 0x17, 0x80, 0x40, 9, 0, 0]) => true,
                _ => {
                    return Err(format!(
                        "a neutral unit's abbreviation table (.debug_abbrev+{abbrev:#x}) is IN-51's"
                    ));
                }
            };
            if with_base {
                let a = r.u32().ok_or_else(short)?;
                if dead_at(&dbg, set, ".debug_addr", u64::from(a)).is_none() {
                    return Err(format!(
                        "a neutral unit's DW_AT_addr_base ({a:#x}) names the file's dead .debug_addr table (incr.md IN-51), not a dead range"
                    ));
                }
            }
            let blen = r.uleb().ok_or_else(short)?;
            if blen == 0 {
                return Err("a neutral unit's block holds at least one byte (incr.md IN-51)".into());
            }
            if r.p as u64 + blen != end as u64 {
                return Err(format!(
                    "the block ({blen:#x} bytes from unit offset {:#x}) ends at the unit's end ({end:#x}), with nothing after it",
                    r.p
                ));
            }
        }
        ".debug_line" => {
            let v = r.u16().ok_or_else(short)?;
            if v == 5 {
                r.skip(2).ok_or_else(short)?;
            } else if !(2..=4).contains(&v) {
                return Err(format!("line table version {v}"));
            }
            let hl = r.u32().ok_or_else(short)? as usize;
            let prog = r.p.checked_add(hl).filter(|&e| e <= end).ok_or_else(short)?;
            r.skip(if v >= 4 { 5 } else { 4 }).ok_or_else(short)?;
            let ob = r.u8().ok_or_else(short)?;
            if ob != 7 {
                return Err(format!("a neutral line table has opcode_base 7, not {ob}"));
            }
            r.skip(6).ok_or_else(short)?;
            let lists: &[u8] = if v == 5 { &[1, 1, 8, 0, 1, 1, 8, 0] } else { &[0, 0] };
            if body.get(r.p..r.p + lists.len()) != Some(lists) {
                return Err("a neutral line table's directory and file lists are empty (incr.md IN-51b)".into());
            }
            if r.p + lists.len() != prog {
                return Err("a neutral line table's header_length is exact".into());
            }
            if let Some(i) = body[prog..].iter().position(|&x| x != 6) {
                return Err(format!(
                    "a neutral line table's program is DW_LNS_negate_stmt (0x06): byte {:#04x} at offset {:#x}",
                    body[prog + i],
                    off + prog + i
                ));
            }
        }
        ".debug_rnglists" | ".debug_loclists" => {
            let v = r.u16().ok_or_else(short)?;
            let _asz = r.u8().ok_or_else(short)?;
            let _seg = r.u8().ok_or_else(short)?;
            let cnt = r.u32().ok_or_else(short)?;
            if v != 5 || cnt != 0 {
                return Err(format!("a neutral list table is version 5 with offset_entry_count 0, not {v} and {cnt}"));
            }
            zeros(body, r.p, off)?;
        }
        ".debug_str_offsets" => {
            let v = r.u16().ok_or_else(short)?;
            let _pad = r.u16().ok_or_else(short)?;
            if v != 5 || !(end - 8).is_multiple_of(4) {
                return Err(format!("a neutral .debug_str_offsets table is version 5 with 4-byte entries, not version {v}"));
            }
            zeros(body, r.p, off)?;
        }
        ".debug_addr" => {
            let v = r.u16().ok_or_else(short)?;
            let asz = r.u8().ok_or_else(short)?;
            let _seg = r.u8().ok_or_else(short)?;
            if v != 5 || asz != 8 || !(end - 8).is_multiple_of(8) {
                return Err(format!("a neutral .debug_addr table is version 5 with 8-byte addresses, not version {v}, size {asz}"));
            }
            zeros(body, r.p, off)?;
        }
        ".debug_frame" => {
            let id = r.u32().ok_or_else(short)?;
            let v = r.u8().ok_or_else(short)?;
            let aug = r.u8().ok_or_else(short)?;
            if id != 0xffff_ffff || v != 1 || aug != 0 {
                return Err("a neutral .debug_frame record is a CIE of version 1 with an empty augmentation".into());
            }
            r.uleb().ok_or_else(short)?;
            r.sleb().ok_or_else(short)?;
            r.u8().ok_or_else(short)?;
            zeros(body, r.p, off)?;
        }
        _ => return Err(format!("'{name}' has no neutral form")),
    }
    Ok(end)
}

fn zeros(b: &[u8], from: usize, off: usize) -> Result<(), String> {
    match b.get(from..).and_then(|t| t.iter().position(|&x| x != 0)) {
        None => Ok(()),
        Some(i) => Err(format!("the body is zero: byte {:#04x} at offset {:#x}", b[from + i], off + from + i)),
    }
}

/// A `contribution` range: whole units of `name` as a compiler wrote
/// them, every address operand the tombstone (IN-53).
fn contribution(image: &Image<'_>, name: &str, b: &[u8], base: usize) -> Result<(), String> {
    let class = image.class();
    let pie = position_independent(image);
    let address_bearing = matches!(name, ".debug_rnglists" | ".debug_loclists" | ".debug_addr")
        || (name == ".debug_line" && class == Class::Elf32);
    if pie && address_bearing {
        return Err(format!(
            "in a position-independent output a dead '{name}' contribution takes the neutral form, not the compiler's bytes (incr.md IN-53)"
        ));
    }
    match name {
        ".debug_line" | ".debug_rnglists" | ".debug_loclists" | ".debug_str_offsets" | ".debug_addr" => {
            let mut p = 0usize;
            while p < b.len() {
                let mut r = Rd::new(b, p);
                let (len, osz) = r.initial_length().ok_or_else(|| format!("no unit starts at offset {:#x}", base + p))?;
                let start = r.p;
                let end = start
                    .checked_add(usize::try_from(len).map_err(|_| "unit_length")?)
                    .filter(|&e| e <= b.len())
                    .ok_or_else(|| format!("the unit at offset {:#x} runs past the range", base + p))?;
                let unit = &b[..end];
                let v = r.u16().unwrap_or(0);
                let ok = match name {
                    ".debug_line" => (2..=5).contains(&v),
                    _ => v == 5,
                };
                if !ok {
                    return Err(format!("the unit at offset {:#x} has version {v}", base + p));
                }
                let tomb = |w| tombstone(name, class, w);
                let res = match name {
                    ".debug_line" => line_addresses(unit, start, end, v, osz, tomb(addr_width(class))),
                    ".debug_addr" => {
                        let asz = r.u8().unwrap_or(0) as usize;
                        r.skip(1);
                        let mut q = r.p;
                        let mut res = Ok(());
                        while asz > 0 && q + asz <= end {
                            let val = Rd::new(unit, q).n(asz).unwrap_or(0);
                            if val != tomb(asz) {
                                res = Err(format!("address {val:#x} at offset {:#x} is not the tombstone", base + q));
                                break;
                            }
                            q += asz;
                        }
                        res
                    }
                    ".debug_rnglists" | ".debug_loclists" => {
                        let asz = r.u8().unwrap_or(0) as usize;
                        lists(unit, r.p + 5, end, name == ".debug_loclists", osz, asz, tomb(asz))
                    }
                    _ => Ok(()),
                };
                res.map_err(|m| format!("unit at offset {:#x}: {m}", base + p))?;
                p = end;
            }
            Ok(())
        }
        ".debug_frame" => {
            let mut p = 0usize;
            let w = addr_width(class);
            while p < b.len() {
                let mut r = Rd::new(b, p);
                let (len, osz) = r.initial_length().ok_or_else(|| format!("no record starts at offset {:#x}", base + p))?;
                let end = r
                    .p
                    .checked_add(usize::try_from(len).map_err(|_| "length")?)
                    .filter(|&e| e <= b.len() && len > 0)
                    .ok_or_else(|| format!("the record at offset {:#x} runs past the range", base + p))?;
                let id = r.n(osz).unwrap_or(0);
                let cie = id == if osz == 8 { u64::MAX } else { 0xffff_ffff };
                if !cie {
                    let loc = r.n(w).ok_or("short FDE")?;
                    if loc != tombstone(name, class, w) {
                        return Err(format!(
                            "the FDE at offset {:#x} has initial_location {loc:#x}, not the tombstone",
                            base + p
                        ));
                    }
                }
                p = end;
            }
            Ok(())
        }
        ".debug_ranges" => {
            // Pairs of addresses; a base selection's base is the
            // tombstone. Other pairs may be offsets from a base the dead
            // unit names (not checked: §6.8, open question).
            let w = addr_width(class);
            let ones = if w == 8 { u64::MAX } else { 0xffff_ffff };
            if !b.len().is_multiple_of(2 * w) {
                return Err(format!("a .debug_ranges contribution is pairs of {w}-byte words"));
            }
            for (k, pair) in b.chunks_exact(2 * w).enumerate() {
                let mut r = Rd::new(pair, 0);
                let (x, y) = (r.n(w).unwrap_or(0), r.n(w).unwrap_or(0));
                if x == ones && y != 1 {
                    return Err(format!(
                        "the base selection at offset {:#x} selects {y:#x}, not the tombstone 1",
                        base + k * 2 * w
                    ));
                }
            }
            Ok(())
        }
        ".debug_loc" | ".debug_macro" => Ok(()),
        _ => Err(format!(
            "a whole dead contribution is left as written only in .debug_line, .debug_str_offsets, .debug_addr, .debug_rnglists, .debug_loclists, .debug_ranges, .debug_loc, .debug_macro and .debug_frame (incr.md IN-53), not '{name}'"
        )),
    }
}

fn addr_width(class: Class) -> usize {
    if class == Class::Elf64 { 8 } else { 4 }
}

/// A line table's `DW_LNE_set_address` operands are the tombstone.
fn line_addresses(u: &[u8], start: usize, end: usize, v: u16, osz: usize, tomb: u64) -> Result<(), String> {
    let mut r = Rd::new(u, start + 2);
    if v == 5 {
        r.skip(2).ok_or("short header")?;
    }
    let hl = r.n(osz).ok_or("short header")? as usize;
    let prog = r.p.checked_add(hl).filter(|&e| e <= end).ok_or("header_length runs past the unit")?;
    r.skip(if v >= 4 { 5 } else { 4 }).ok_or("short header")?;
    let ob = r.u8().ok_or("short header")?;
    let lens: Vec<u8> = (1..ob.max(1)).map_while(|_| r.u8()).collect();
    let mut r = Rd::new(&u[..end], prog);
    while r.p < end {
        let op = r.u8().ok_or("short program")?;
        if op == 0 {
            let n = r.uleb().ok_or("short extended opcode")?;
            let at = r.p;
            if n == 0 {
                return Err("an extended opcode of length 0".into());
            }
            let sub = r.u8().ok_or("short extended opcode")?;
            if sub == 2 {
                let w = (n - 1) as usize;
                let a = r.n(w).ok_or("short DW_LNE_set_address")?;
                let ones = if w >= 8 { u64::MAX } else { (1u64 << (8 * w)) - 1 };
                let want = if tomb == u64::MAX { ones } else { tomb };
                if a != want {
                    return Err(format!("DW_LNE_set_address {a:#x} is not the tombstone {want:#x}"));
                }
            }
            r.p = at;
            r.skip(n).ok_or("an extended opcode runs past the unit")?;
        } else if op < ob {
            if op == 9 {
                r.skip(2).ok_or("short DW_LNS_fixed_advance_pc")?;
            } else {
                for _ in 0..lens.get(usize::from(op) - 1).copied().unwrap_or(0) {
                    r.uleb().ok_or("short standard opcode")?;
                }
            }
        }
    }
    Ok(())
}

/// The entries of a v5 list table's body: address operands are the
/// tombstone.
fn lists(u: &[u8], header_end: usize, end: usize, loc: bool, osz: usize, asz: usize, tomb: u64) -> Result<(), String> {
    let mut r = Rd::new(u, header_end - 4);
    let cnt = r.u32().ok_or("short header")? as usize;
    if !matches!(asz, 4 | 8) {
        return Err(format!("address size {asz}"));
    }
    let mut r = Rd::new(&u[..end], header_end);
    r.skip((cnt as u64).saturating_mul(osz as u64)).ok_or("the offset array runs past the unit")?;
    let addr = |r: &mut Rd<'_>| -> Result<(), String> {
        let a = r.n(asz).ok_or("short entry")?;
        if a == tomb { Ok(()) } else { Err(format!("address {a:#x} is not the tombstone {tomb:#x}")) }
    };
    while r.p < end {
        let e = r.u8().ok_or("short entry")?;
        let short = || "short entry".to_string();
        let expr = |r: &mut Rd<'_>| -> Result<(), String> {
            let n = r.uleb().ok_or_else(short)?;
            r.skip(n).ok_or_else(short)
        };
        match (loc, e) {
            (_, 0) => {}
            (_, 1) => {
                r.uleb().ok_or_else(short)?;
            }
            (_, 2..=4) => {
                r.uleb().ok_or_else(short)?;
                r.uleb().ok_or_else(short)?;
                if loc {
                    expr(&mut r)?;
                }
            }
            (false, 5) | (true, 6) => addr(&mut r)?,
            (true, 5) => expr(&mut r)?,
            (false, 6) | (true, 7) => {
                addr(&mut r)?;
                addr(&mut r)?;
                if loc {
                    expr(&mut r)?;
                }
            }
            (false, 7) | (true, 8) => {
                addr(&mut r)?;
                r.uleb().ok_or_else(short)?;
                if loc {
                    expr(&mut r)?;
                }
            }
            _ => return Err(format!("unknown list entry kind {e:#04x} at unit offset {:#x}", r.p - 1)),
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------
// Check 1: what live units name.

/// An abbreviation: tag aside, whether it has children and its
/// attributes `(name, form, implicit const)`.
struct Abbrev {
    children: bool,
    attrs: Vec<(u64, u64)>,
}

fn abbrevs(d: &[u8], off: usize) -> Option<std::collections::HashMap<u64, Abbrev>> {
    let mut m = std::collections::HashMap::new();
    let mut r = Rd::new(d, off);
    loop {
        let code = r.uleb()?;
        if code == 0 {
            return Some(m);
        }
        let _tag = r.uleb()?;
        let children = r.u8()? != 0;
        let mut attrs = Vec::new();
        loop {
            let (a, f) = (r.uleb()?, r.uleb()?);
            if a == 0 && f == 0 {
                break;
            }
            if f == 0x21 {
                r.sleb()?;
            }
            attrs.push((a, f));
        }
        m.insert(code, Abbrev { children, attrs });
        if m.len() > 1_000_000 {
            return None;
        }
    }
}

/// What a form's value names, when it is an offset into a section.
enum Named {
    Nothing,
    Offset(&'static str, u64),
    /// A `sec_offset` (or v2/v3 `data4`/`data8` pointer) of attribute
    /// `at`.
    SecOffset(u64, u64),
}

struct UnitCtx {
    version: u16,
    osz: usize,
    asz: usize,
}

/// Reads one attribute value of form `form`.
fn form_value(r: &mut Rd<'_>, form: u64, u: &UnitCtx) -> Option<Named> {
    let off = |r: &mut Rd<'_>| r.n(u.osz);
    Some(match form {
        0x01 => {
            r.n(u.asz)?;
            Named::Nothing
        }
        0x03 => {
            let n = r.u16()?;
            r.skip(u64::from(n))?;
            Named::Nothing
        }
        0x04 => {
            let n = r.u32()?;
            r.skip(u64::from(n))?;
            Named::Nothing
        }
        0x05 | 0x12 | 0x26 | 0x2a => {
            r.n(2)?;
            Named::Nothing
        }
        0x06 => Named::SecOffset(u64::MAX - 4, r.n(4)?),
        0x07 => Named::SecOffset(u64::MAX - 8, r.n(8)?),
        0x08 => {
            r.cstr()?;
            Named::Nothing
        }
        0x09 | 0x18 => {
            let n = r.uleb()?;
            r.skip(n)?;
            Named::Nothing
        }
        0x0a => {
            let n = r.u8()?;
            r.skip(u64::from(n))?;
            Named::Nothing
        }
        0x0b | 0x0c | 0x11 | 0x25 | 0x29 => {
            r.n(1)?;
            Named::Nothing
        }
        0x0d => {
            r.sleb()?;
            Named::Nothing
        }
        0x0f | 0x15 | 0x1a | 0x1b | 0x22 | 0x23 | 0x1f01 | 0x1f02 => {
            r.uleb()?;
            Named::Nothing
        }
        0x0e => Named::Offset(".debug_str", off(r)?),
        0x1f => Named::Offset(".debug_line_str", off(r)?),
        0x10 => {
            let w = if u.version == 2 { u.asz } else { u.osz };
            Named::Offset(".debug_info", r.n(w)?)
        }
        0x13 | 0x1c | 0x28 | 0x2c => {
            r.n(4)?;
            Named::Nothing
        }
        0x14 | 0x20 | 0x24 => {
            r.n(8)?;
            Named::Nothing
        }
        0x27 | 0x2b => {
            r.n(3)?;
            Named::Nothing
        }
        0x17 => Named::SecOffset(0, off(r)?),
        0x19 | 0x21 => Named::Nothing,
        0x1e => {
            r.n(16)?;
            Named::Nothing
        }
        0x1d | 0x1f20 | 0x1f21 => {
            off(r)?;
            Named::Nothing
        }
        0x16 => {
            let f = r.uleb()?;
            if f == 0x16 {
                return None;
            }
            return form_value(r, f, u);
        }
        _ => return None,
    })
}

/// The section a `sec_offset` of attribute `at` points into.
fn sec_offset_target(at: u64, version: u16) -> Option<&'static str> {
    Some(match at {
        0x10 => ".debug_line",
        0x55 | 0x2c => {
            if version >= 5 {
                ".debug_rnglists"
            } else {
                ".debug_ranges"
            }
        }
        0x02 | 0x19 | 0x2a | 0x38 | 0x40 | 0x48 | 0x4a | 0x4d | 0x2e => {
            if version >= 5 {
                ".debug_loclists"
            } else {
                ".debug_loc"
            }
        }
        0x72 => ".debug_str_offsets",
        0x73 => ".debug_addr",
        0x74 => ".debug_rnglists",
        0x8c => ".debug_loclists",
        0x79 | 0x2119 => ".debug_macro",
        0x43 => ".debug_macinfo",
        _ => return None,
    })
}

/// IN-3a check 1 over the debug sections (P-I4) and the frame tables'
/// CIE pointers (P-I3 for `.eh_frame`).
fn live_references(dbg: &Debug<'_>, set: &DeadSet, out: &mut Vec<Finding>) {
    let image = dbg.image;
    let say = |msg: String, out: &mut Vec<Finding>| {
        if out.len() < 10_000 {
            out.push(finding(4, msg));
        }
    };
    let mut lines: Vec<u64> = Vec::new();
    let mut str_offsets: Vec<u64> = Vec::new();
    if let Some((_, info)) = dbg.data(".debug_info") {
        let abbrev = dbg.data(".debug_abbrev").map_or(&[][..], |(_, d)| d);
        let mut p = 0usize;
        let mut units = 0u32;
        while p < info.len() {
            units += 1;
            if units > 10_000_000 {
                break;
            }
            let mut r = Rd::new(info, p);
            let Some((len, osz)) = r.initial_length() else {
                say(format!("cannot read the .debug_info unit at {p:#x}: its length"), out);
                break;
            };
            let Some(end) = usize::try_from(len).ok().and_then(|l| r.p.checked_add(l)).filter(|&e| e <= info.len())
            else {
                say(format!("cannot read the .debug_info unit at {p:#x}: it runs past the section"), out);
                break;
            };
            let unit_at = p;
            p = end;
            if dead_at(dbg, set, ".debug_info", unit_at as u64).is_some() {
                continue;
            }
            let v = r.u16().unwrap_or(0);
            let (ab, asz, ut) = match v {
                5 => {
                    let ut = r.u8().unwrap_or(0);
                    let asz = r.u8().unwrap_or(0);
                    (r.n(osz).unwrap_or(u64::MAX), asz, ut)
                }
                2..=4 => {
                    let ab = r.n(osz).unwrap_or(u64::MAX);
                    (ab, r.u8().unwrap_or(0), 1)
                }
                _ => {
                    say(format!("cannot walk the live .debug_info unit at {unit_at:#x}: version {v}"), out);
                    continue;
                }
            };
            // Skeleton/split/type units carry an id or signature first.
            match ut {
                1 | 3 => {}
                4 | 5 => {
                    r.skip(8);
                }
                2 | 6 => {
                    r.skip(8 + osz as u64);
                }
                _ => {}
            }
            if let Some(d) = dead_at(dbg, set, ".debug_abbrev", ab) {
                say(format!("{}: the live .debug_info unit at {unit_at:#x} names it as its abbreviation table", d.describe(image)), out);
                continue;
            }
            let Some(table) = usize::try_from(ab).ok().and_then(|a| abbrevs(abbrev, a)) else {
                say(format!("cannot walk the live .debug_info unit at {unit_at:#x}: its abbreviation table at {ab:#x}"), out);
                continue;
            };
            let ctx = UnitCtx { version: v, osz, asz: usize::from(asz) };
            let mut r = Rd::new(&info[..end], r.p);
            let mut depth = 0i64;
            while r.p < end {
                let die_at = r.p;
                let Some(code) = r.uleb() else { break };
                if code == 0 {
                    depth -= 1;
                    continue;
                }
                let Some(a) = table.get(&code) else {
                    say(format!("cannot walk the live .debug_info unit at {unit_at:#x}: abbreviation code {code} at {die_at:#x}"), out);
                    break;
                };
                let mut bad = false;
                for &(at, form) in &a.attrs {
                    let Some(val) = form_value(&mut r, form, &ctx) else {
                        say(format!("cannot walk the live .debug_info unit at {unit_at:#x}: form {form:#x} of the DIE at {die_at:#x}"), out);
                        bad = true;
                        break;
                    };
                    let target = match val {
                        Named::Nothing => None,
                        Named::Offset(s, o) => Some((s, o)),
                        Named::SecOffset(kind, o) => {
                            // data4/data8 are pointers only before DWARF 4,
                            // and only for these attributes.
                            let pointer = kind == 0
                                || (v < 4 && matches!(at, 0x10 | 0x55 | 0x43 | 0x02 | 0x40));
                            if pointer { sec_offset_target(at, v).map(|s| (s, o)) } else { None }
                        }
                    };
                    let Some((s, o)) = target else { continue };
                    if at == 0x10 && s == ".debug_line" {
                        lines.push(o);
                    }
                    if at == 0x72 {
                        str_offsets.push(o);
                    }
                    if let Some(d) = dead_at(dbg, set, s, o) {
                        say(
                            format!(
                                "{}: the live .debug_info unit at {unit_at:#x} names {s}+{o:#x} (attribute {at:#x}, form {form:#x}) in the DIE at {die_at:#x}",
                                d.describe(image)
                            ),
                            out,
                        );
                    }
                }
                if bad {
                    break;
                }
                if a.children {
                    depth += 1;
                }
                let _ = depth;
            }
        }
    }
    // The `.debug_line` headers live units name: their string forms.
    if let Some((_, line)) = dbg.data(".debug_line") {
        for &o in &lines {
            if let Err(m) = line_header_strings(dbg, set, line, o, out) {
                say(format!("cannot read the live .debug_line table at {o:#x}: {m}"), out);
            }
        }
    }
    // The live `.debug_str_offsets` tables' entries.
    if let Some((_, so)) = dbg.data(".debug_str_offsets") {
        for &b in &str_offsets {
            let Some(h) = usize::try_from(b).ok().and_then(|b| b.checked_sub(8)) else { continue };
            let mut r = Rd::new(so, h);
            let Some(len) = r.u32().filter(|&l| l < 0xffff_fff0) else { continue };
            let end = (h + 4 + len as usize).min(so.len());
            let mut q = h + 8;
            while q + 4 <= end {
                let s = Rd::new(so, q).n(4).unwrap_or(0);
                if let Some(d) = dead_at(dbg, set, ".debug_str", s) {
                    say(format!("{}: the live .debug_str_offsets entry at {q:#x} names .debug_str+{s:#x}", d.describe(image)), out);
                }
                q += 4;
            }
        }
    }
    // `.debug_aranges`' unit offsets.
    if let Some((_, ar)) = dbg.data(".debug_aranges") {
        let mut p = 0usize;
        while p < ar.len() {
            let mut r = Rd::new(ar, p);
            let Some((len, osz)) = r.initial_length() else { break };
            let Some(end) = usize::try_from(len).ok().and_then(|l| r.p.checked_add(l)) else { break };
            r.skip(2);
            if let Some(o) = r.n(osz)
                && let Some(d) = dead_at(dbg, set, ".debug_info", o)
            {
                say(format!("{}: the .debug_aranges set at {p:#x} names unit .debug_info+{o:#x}", d.describe(image)), out);
            }
            if end <= p {
                break;
            }
            p = end;
        }
    }
    // Live FDEs' CIE pointers.
    frame_cie_pointers(dbg, set, ".debug_frame", out);
    if let Some(i) = image.sections.iter().position(|s| s.name == ".eh_frame") {
        eh_cie_pointers(image, i, set, out);
    }
}

/// A v5 line table header's `strp`/`line_strp` entries (v2–v4 headers
/// hold strings inline).
fn line_header_strings(dbg: &Debug<'_>, set: &DeadSet, d: &[u8], off: u64, out: &mut Vec<Finding>) -> Result<(), String> {
    let mut r = Rd::new(d, usize::try_from(off).map_err(|_| "offset")?);
    let (_, osz) = r.initial_length().ok_or("its length")?;
    let v = r.u16().ok_or("short")?;
    if v != 5 {
        return Ok(());
    }
    let asz = r.u8().ok_or("short")?;
    r.skip(1).ok_or("short")?;
    r.n(osz).ok_or("short")?;
    r.skip(5).ok_or("short")?;
    let ob = r.u8().ok_or("short")?;
    r.skip(u64::from(ob.saturating_sub(1))).ok_or("short")?;
    let ctx = UnitCtx { version: 5, osz, asz: usize::from(asz) };
    for _list in 0..2 {
        let nf = r.u8().ok_or("short")?;
        let mut fmt = Vec::new();
        for _ in 0..nf {
            fmt.push((r.uleb().ok_or("short")?, r.uleb().ok_or("short")?));
        }
        let n = r.uleb().ok_or("short")?;
        for _ in 0..n {
            for &(ct, form) in &fmt {
                match form_value(&mut r, form, &ctx).ok_or_else(|| format!("form {form:#x}"))? {
                    Named::Offset(s, o) => {
                        if let Some(dd) = dead_at(dbg, set, s, o) {
                            out.push(finding(
                                4,
                                format!(
                                    "{}: the live .debug_line table at {off:#x} names {s}+{o:#x} (content type {ct:#x})",
                                    dd.describe(dbg.image)
                                ),
                            ));
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

/// `.debug_frame`: a live FDE's CIE pointer (a section offset) is not in
/// a dead range.
fn frame_cie_pointers(dbg: &Debug<'_>, set: &DeadSet, name: &str, out: &mut Vec<Finding>) {
    let Some((_, d)) = dbg.data(name) else { return };
    let mut p = 0usize;
    while p < d.len() {
        let mut r = Rd::new(d, p);
        let Some((len, osz)) = r.initial_length() else { break };
        let Some(end) = usize::try_from(len).ok().and_then(|l| r.p.checked_add(l)) else { break };
        if len == 0 || end <= p {
            break;
        }
        if dead_at(dbg, set, name, p as u64).is_none()
            && let Some(id) = r.n(osz)
            && id != if osz == 8 { u64::MAX } else { 0xffff_ffff }
            && let Some(dd) = dead_at(dbg, set, name, id)
        {
            out.push(finding(
                4,
                format!("{}: the live {name} FDE at {p:#x} names a CIE at {id:#x}", dd.describe(dbg.image)),
            ));
        }
        p = end;
    }
}

/// `.eh_frame`: a live FDE's CIE pointer (relative to the field) is not
/// in a dead range (P-I3).
fn eh_cie_pointers(image: &Image<'_>, i: usize, set: &DeadSet, out: &mut Vec<Finding>) {
    let Some(d) = image.section_data(i) else { return };
    let base = image.sections[i].shdr.sh_addr;
    let mut p = 0usize;
    while p + 4 <= d.len() {
        let mut r = Rd::new(d, p);
        let Some((len, osz)) = r.initial_length() else { break };
        if len == 0 {
            break;
        }
        let Some(end) = usize::try_from(len).ok().and_then(|l| r.p.checked_add(l)) else { break };
        let at = base + p as u64;
        let field = base + r.p as u64;
        if set.in_section(i, at).is_none()
            && let Some(id) = r.n(osz)
            && id != 0
        {
            let cie = field.wrapping_sub(id);
            if let Some(dd) = set.in_section(i, cie) {
                out.push(
                    finding(3, format!("{}: the live .eh_frame FDE at {at:#x} names a CIE at {cie:#x}", dd.describe(image)))
                        .at(at),
                );
            }
        }
        if end <= p {
            break;
        }
        p = end;
    }
}

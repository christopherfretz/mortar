//! The symbol tables of an incremental-mode output (`spec/incr.md`
//! IN-49a; `spec/plumb.md` §6.8, §6.9) and the decision records of its
//! map (§3.2).
//!
//! - **fillers**: the entries of `.symtab`'s local area that hold room
//!   for later locals, all of one form ([`Table::classify`]); P-E3 passes
//!   over them in a side whose map says `incremental` and reports one
//!   elsewhere, and reports an entry one field away from the form
//!   (a near-filler) naming the field (`crate::equiv`);
//! - **file slack**: the zero bytes after `.symtab` and `.strtab`, listed
//!   as `hole` lines past the section's end ([`slack`]): P-I2 for one that
//!   runs into what follows, P-I5 for a byte that is not zero;
//! - **dead names**: `dead .strtab ... strings` lines: P-I3 for an entry
//!   other than a filler that names a string inside one (their NUL ends
//!   are P-I6, `crate::deaddwarf`'s walk of the non-allocated ranges);
//! - **decision records**: a `region` P-I2 outside its output section or
//!   meeting another region of its section, a `symblock` P-I2 past
//!   `.symtab`'s local area; the other records are read, not judged.

use std::fmt::Write as _;

use mortar_elf::consts::{SHN_UNDEF, SHT_NOBITS, SHT_SYMTAB, SHT_SYMTAB_SHNDX, STT_SECTION};

use crate::dead::{DeadSet, Range};
use crate::image::{Image, OutSym};
use crate::map::{DeadRange, Map};
use crate::report::{Finding, Rule};

fn finding(n: u8, msg: String) -> Finding {
    Finding::new(Rule::Incr(n), msg)
}

/// What an entry of `.symtab` is to IN-49a's filler form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Form {
    /// The form exactly.
    Filler,
    /// The form but for one field, named.
    Near(&'static str),
}

/// `.symtab` read for the filler form.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Table<'a> {
    /// `.symtab`'s section index and `sh_info` (the first non-local).
    pub sec: usize,
    pub info: u32,
    /// `.symtab_shndx`'s words, when there is one for `.symtab`.
    shndx: Option<&'a [u8]>,
    /// Section 1's `sh_addr` (the form's `st_value`).
    sec1: Option<u64>,
}

impl<'a> Table<'a> {
    /// The output's `.symtab`, when it has one.
    pub fn of(image: &Image<'a>) -> Option<Table<'a>> {
        let sec = image.sections.iter().position(|s| s.shdr.sh_type == SHT_SYMTAB)?;
        let info = image.sections[sec].shdr.sh_info;
        let shndx = image
            .sections
            .iter()
            .position(|s| s.shdr.sh_type == SHT_SYMTAB_SHNDX && s.shdr.sh_link as usize == sec)
            .and_then(|i| image.section_data(i));
        let sec1 = image.sections.get(1).map(|s| s.shdr.sh_addr);
        Some(Table { sec, info, shndx, sec1 })
    }

    /// The fields in which entry `s` differs from the filler form (§6.9):
    /// `st_name` 0, `st_info` 0x03, `st_other` 0, `st_shndx` 1, `st_value`
    /// section 1's `sh_addr`, `st_size` 0, and 0 in `.symtab_shndx`.
    fn differ(&self, s: &OutSym) -> impl Iterator<Item = &'static str> {
        let y = &s.sym;
        let ext = self.shndx.map(|d| {
            let o = s.index as usize * 4;
            d.get(o..o + 4).map_or(u32::MAX, |w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
        });
        [
            (y.st_name != 0, "st_name"),
            (y.st_info != STT_SECTION, "st_info"),
            (y.st_other != 0, "st_other"),
            (y.st_shndx != 1, "st_shndx"),
            (Some(y.st_value) != self.sec1, "st_value"),
            (y.st_size != 0, "st_size"),
            (ext.is_some_and(|w| w != 0), ".symtab_shndx"),
        ]
        .into_iter()
        .filter(|(d, _)| *d)
        .map(|(_, f)| f)
    }

    /// Entry `s` against the form: the form exactly, or but for one field.
    /// Symbol 0 is neither.
    pub fn classify(&self, s: &OutSym) -> Option<Form> {
        if s.index == 0 {
            return None;
        }
        let mut d = self.differ(s);
        match (d.next(), d.next()) {
            (None, _) => Some(Form::Filler),
            (Some(f), None) => Some(Form::Near(f)),
            _ => None,
        }
    }

    /// Whether the side writes another linker's section symbols (GNU ld's,
    /// one per output section): an unnamed `STT_SECTION` entry of a
    /// section other than 1 whose value is that section's `sh_addr`. The
    /// filler form is then section 1's own symbol, not a filler (§6.9).
    pub fn section_symbols(image: &Image<'_>) -> Option<u16> {
        image.symbols.iter().skip(1).find_map(|s| {
            let y = &s.sym;
            let k = y.st_shndx;
            (y.st_name == 0
                && y.sym_type() == STT_SECTION
                && k != SHN_UNDEF
                && k != 1
                && k < 0xff00
                && image.sections.get(usize::from(k)).is_some_and(|c| c.shdr.sh_addr == y.st_value))
            .then_some(k)
        })
    }
}

/// The room the file has after section `sec` (§6.8): from its `sh_offset`
/// to the next thing in the file, the next section in file order (one with
/// file bytes) or the section header table, whichever comes first; the end
/// of the file when neither follows.
fn room(image: &Image<'_>, sec: usize) -> u64 {
    let s = &image.sections[sec].shdr;
    let mut end = image.data.len() as u64;
    for (j, o) in image.sections.iter().enumerate() {
        let t = &o.shdr;
        if j != sec
            && j != 0
            && t.sh_type != SHT_NOBITS
            && t.sh_size > 0
            && t.sh_offset > s.sh_offset
        {
            end = end.min(t.sh_offset);
        }
    }
    let h = image.header.e_shoff;
    if h > s.sh_offset {
        end = end.min(h);
    }
    end.saturating_sub(s.sh_offset)
}

/// Whether line `d` of output section `sec` has the file slack's shape
/// (§6.8; IN-49a): a `hole` of `.symtab` or `.strtab` starting at its
/// `sh_size`, in a map that says `incremental`. `Some(Err)` when it runs
/// past the room the file has (P-I2).
pub(crate) fn slack(
    image: &Image<'_>,
    incremental: bool,
    d: &DeadRange,
    sec: usize,
) -> Option<Result<(), String>> {
    let s = &image.sections[sec];
    if !incremental
        || d.kind.is_some()
        || !matches!(s.name.as_str(), ".symtab" | ".strtab")
        || d.offset != s.shdr.sh_size
    {
        return None;
    }
    let room = room(image, sec);
    Some(match d.offset.checked_add(d.size) {
        Some(e) if d.size > 0 && e <= room => Ok(()),
        _ => Err(format!(
            "map line {}: the file slack of '{}' [{:#x}, +{:#x}) is empty or runs past the next section or the section header table (room {room:#x} from its sh_offset)",
            d.line, s.name, d.offset, d.size
        )),
    })
}

/// P-I5 for the file slack (zero bytes), P-I3 for `.strtab`'s dead names,
/// P-I2 for the decision records judged; the coverage line of the
/// decision records (§7.2) when the map has any.
pub(crate) fn check(
    image: &Image<'_>,
    map: &Map,
    set: &DeadSet,
    out: &mut Vec<Finding>,
) -> Option<String> {
    for r in set.ranges.iter().filter(|r| r.slack) {
        zero(image, r, out);
    }
    dead_names(image, set, out);
    decisions(image, map, out)
}

fn zero(image: &Image<'_>, r: &Range, out: &mut Vec<Finding>) {
    let s = &image.sections[r.sec].shdr;
    let off = r.start - s.sh_addr;
    let at = s.sh_offset.checked_add(off);
    let bytes = at.and_then(|a| {
        let a = usize::try_from(a).ok()?;
        image.data.get(a..a.checked_add(usize::try_from(r.end - r.start).ok()?)?)
    });
    let msg = match bytes {
        None => Some("its bytes are not in the file".to_string()),
        Some(b) => b.iter().position(|&x| x != 0).map(|i| {
            format!(
                "byte {:#x} at file offset {:#x} is not zero (the file slack is zero bytes, IN-49a)",
                b[i],
                at.unwrap_or(0) + i as u64
            )
        }),
    };
    if let Some(m) = msg {
        out.push(finding(5, format!("{}: {m}", r.describe(image))));
    }
}

/// P-I3 for `.strtab` (§6.8): no `.symtab` entry other than a filler has
/// an `st_name` inside a `.strtab` dead range.
fn dead_names(image: &Image<'_>, set: &DeadSet, out: &mut Vec<Finding>) {
    let Some(t) = Table::of(image) else { return };
    let strtab = image.sections[t.sec].shdr.sh_link as usize;
    let Some(s) = image.sections.get(strtab) else { return };
    if !set.ranges.iter().any(|r| r.sec == strtab && !r.slack) {
        return;
    }
    let base = s.shdr.sh_addr;
    for y in image.symbols.iter().skip(1) {
        if t.classify(y) == Some(Form::Filler) {
            continue;
        }
        let Some(r) = set.in_section(strtab, base.wrapping_add(u64::from(y.sym.st_name))) else {
            continue;
        };
        if r.slack {
            continue;
        }
        out.push(finding(
            3,
            format!(
                "{}: .symtab entry {} ('{}') names .strtab+{:#x} in it",
                r.describe(image),
                y.index,
                y.name,
                y.sym.st_name
            ),
        ));
    }
}

/// §3.2's judgement of the decision records (P-I2) and their coverage
/// line.
fn decisions(image: &Image<'_>, map: &Map, out: &mut Vec<Finding>) -> Option<String> {
    let d = &map.decisions;
    if d.is_empty() {
        return None;
    }
    let mut by_sec: Vec<(usize, u64, u64, u32)> = Vec::new();
    for r in &d.regions {
        let found: Vec<usize> = image
            .sections
            .iter()
            .enumerate()
            .filter(|(i, s)| *i > 0 && s.name == r.out)
            .map(|(i, _)| i)
            .collect();
        let &[sec] = found.as_slice() else {
            out.push(finding(
                2,
                format!(
                    "map line {}: region '{}' names {} output sections, not one",
                    r.line,
                    r.out,
                    found.len()
                ),
            ));
            continue;
        };
        let s = &image.sections[sec].shdr;
        let end = r.addr.checked_add(r.capacity);
        let hi = s.sh_addr.saturating_add(s.sh_size);
        if r.addr < s.sh_addr || end.is_none_or(|e| e > hi) {
            out.push(
                finding(
                    2,
                    format!(
                        "map line {}: region [{:#x}, +{:#x}) of file {} is not inside '{}' [{:#x}, {hi:#x}]",
                        r.line, r.addr, r.capacity, r.file, r.out, s.sh_addr
                    ),
                )
                .at(r.addr),
            );
            continue;
        }
        by_sec.push((sec, r.addr, r.addr + r.capacity, r.line));
    }
    // Two regions of one output section do not meet (IN-30: a file's
    // region is its own).
    by_sec.sort_unstable();
    // The region reaching furthest so far in the section.
    let mut reach: Option<(usize, u64, u64, u32)> = None;
    for &b in &by_sec {
        let a = match reach {
            Some(a) if a.0 == b.0 => a,
            _ => {
                reach = Some(b);
                continue;
            }
        };
        if b.2 > a.2 {
            reach = Some(b);
        }
        if b.1 < a.2 && b.1 < b.2 && a.1 < a.2 {
            out.push(
                finding(
                    2,
                    format!(
                        "map line {}: region [{:#x}, {:#x}) of '{}' meets the region of line {} [{:#x}, {:#x})",
                        b.3, b.1, b.2, image.sections[b.0].name, a.3, a.1, a.2
                    ),
                )
                .at(b.1),
            );
        }
    }
    // A symbol block lies in `.symtab`'s local area (IN-49a).
    if let Some(t) = Table::of(image) {
        for b in &d.symblocks {
            if b.first.saturating_add(b.capacity) > u64::from(t.info) {
                out.push(finding(
                    2,
                    format!(
                        "map line {}: symbol block [{}, +{}) runs past .symtab's local area (sh_info {})",
                        b.line, b.first, b.capacity, t.info
                    ),
                ));
            }
        }
    }
    let mut line = String::new();
    let _ = write!(
        line,
        "decision records: {} read; region, symblock judged (place only); capacity, order, slot, redirect, frag not judged",
        d.len()
    );
    Some(line)
}

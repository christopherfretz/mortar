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
//!   meeting another region of its section, a `symblock` P-I2 out of
//!   place (past `.symtab`'s local area, its `.strtab` region outside
//!   `.strtab`, meeting another block) and P-I7 for its entries and names
//!   against the output's tables (WP-268); the other records are read,
//!   not judged.

use std::fmt::Write as _;

use mortar_elf::consts::{
    SHN_UNDEF, SHT_NOBITS, SHT_SYMTAB, SHT_SYMTAB_SHNDX, STB_LOCAL, STT_SECTION,
};

use crate::dead::{DeadSet, Range};
use crate::image::{Image, OutSym};
use crate::map::{DeadRange, Map, SymBlock};
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
    symblocks(image, map, out);
    let mut line = String::new();
    let _ = write!(
        line,
        "decision records: {} read; region judged (place only), symblock judged (place, names); capacity, order, slot, redirect, frag not judged",
        d.len()
    );
    Some(line)
}

/// S-86's shared names: the fixed table's strings, which entries of any
/// block may name.
const SHARED: [&[u8]; 4] = [b"", b"$x", b"$d", b"__erratum_843419_veneer"];

/// The string at `off` of `strtab` and the offset of its NUL.
fn string_at(strtab: &[u8], off: u64) -> Option<(&[u8], u64)> {
    let o = usize::try_from(off).ok()?;
    let rest = strtab.get(o..)?;
    let n = rest.iter().position(|&b| b == 0)?;
    Some((&rest[..n], off + n as u64))
}

fn file_name(b: &SymBlock) -> String {
    b.file.map_or_else(|| "-".to_string(), |f| f.to_string())
}

/// §3.2's place of each `symblock` (P-I2) and §6.8's judgement of its
/// entries and names against the output's `.symtab` and `.strtab`
/// (P-I7, WP-268; IN-49a, I-78a, S-74, S-79, S-86).
fn symblocks(image: &Image<'_>, map: &Map, out: &mut Vec<Finding>) {
    let blocks = &map.decisions.symblocks;
    if blocks.is_empty() {
        return;
    }
    let Some(t) = Table::of(image) else {
        for b in blocks {
            out.push(finding(
                2,
                format!(
                    "map line {}: symbol block of file {} in an output with no .symtab",
                    b.line,
                    file_name(b)
                ),
            ));
        }
        return;
    };
    let strtab_sec = image.sections[t.sec].shdr.sh_link as usize;
    let strtab = image.section_data(strtab_sec).unwrap_or(&[]);
    // P-I7 only of records all in place: out of place, they state no
    // blocks to judge the tables against.
    if let Some(sound) = places(blocks, u64::from(t.info), strtab.len() as u64, out) {
        entries(image, map, &t, strtab, &sound, out);
    }
}

/// §3.2's place of each `symblock` (P-I2): the records when all are in
/// place.
fn places<'m>(
    blocks: &'m [SymBlock],
    info: u64,
    strsize: u64,
    out: &mut Vec<Finding>,
) -> Option<Vec<&'m SymBlock>> {
    // P-I2: each record's place, alone.
    let mut sound: Vec<&SymBlock> = Vec::new();
    for b in blocks {
        let why = if b.first == 0 {
            Some("starts at entry 0 (the blocks follow it, S-74)".to_string())
        } else if b.count > b.capacity {
            Some(format!("has count {} above its capacity {}", b.count, b.capacity))
        } else if b.first.checked_add(b.capacity).is_none_or(|e| e > info) {
            Some(format!("runs past .symtab's local area (sh_info {info})"))
        } else if b.strtab_off.checked_add(b.strtab_cap).is_none_or(|e| e > strsize) {
            Some(format!(
                "has its .strtab region [{:#x}, +{:#x}) outside .strtab (size {strsize:#x})",
                b.strtab_off, b.strtab_cap
            ))
        } else {
            None
        };
        match why {
            Some(w) => out.push(finding(
                2,
                format!(
                    "map line {}: symbol block [{}, +{}) of file {} {w}",
                    b.line,
                    b.first,
                    b.capacity,
                    file_name(b)
                ),
            )),
            None => sound.push(b),
        }
    }
    // P-I2: one record per file; no two share an entry or a string byte.
    let mut placed_ok = sound.len() == blocks.len();
    let mut by_file: Vec<&SymBlock> = sound.clone();
    by_file.sort_by_key(|b| (b.file, b.line));
    for w in by_file.windows(2) {
        if w[0].file == w[1].file {
            placed_ok = false;
            out.push(finding(
                2,
                format!(
                    "map line {}: a second symbol block of file {} (the first at line {})",
                    w[1].line,
                    file_name(w[1]),
                    w[0].line
                ),
            ));
        }
    }
    let meet = |key: fn(&SymBlock) -> (u64, u64), what: &str, out: &mut Vec<Finding>| {
        let mut v: Vec<&SymBlock> = sound.iter().copied().filter(|b| key(b).1 > 0).collect();
        v.sort_by_key(|b| (key(b).0, b.line));
        let mut ok = true;
        let mut reach: Option<&SymBlock> = None;
        for &b in &v {
            if let Some(a) = reach {
                let (ao, az) = key(a);
                let (bo, bz) = key(b);
                if bo < ao + az {
                    ok = false;
                    out.push(finding(
                        2,
                        format!(
                            "map line {}: symbol block of file {} shares {what} [{bo:#x}, +{bz:#x}) with the block of line {} [{ao:#x}, +{az:#x})",
                            b.line,
                            file_name(b),
                            a.line
                        ),
                    ));
                }
                if bo + bz > ao + az {
                    reach = Some(b);
                }
            } else {
                reach = Some(b);
            }
        }
        ok
    };
    placed_ok &= meet(|b| (b.first, b.capacity), "entries", out);
    placed_ok &= meet(|b| (b.strtab_off, b.strtab_cap), ".strtab bytes", out);
    placed_ok.then_some(sound)
}

/// §6.8's P-I7: the blocks' entries and names against `.symtab` and
/// `.strtab`, every record in place.
fn entries(
    image: &Image<'_>,
    map: &Map,
    t: &Table<'_>,
    strtab: &[u8],
    sound: &[&SymBlock],
    out: &mut Vec<Finding>,
) {
    let info = u64::from(t.info);
    let pi7 = |b: &SymBlock, msg: String| {
        finding(
            7,
            format!(
                "map line {}: symbol block [{}, +{}) of file {}: {msg}",
                b.line,
                b.first,
                b.count,
                file_name(b)
            ),
        )
    };
    let filler = |y: &OutSym| t.classify(y) == Some(Form::Filler);
    // The block (its place in `sound`) whose region holds `off`.
    let region_of = |off: u64| {
        sound.iter().position(|b| b.strtab_off <= off && off - b.strtab_off < b.strtab_cap)
    };
    // The block each entry is one of the `<count>` entries of.
    let mut owner: Vec<Option<usize>> = vec![None; image.symbols.len()];
    // Whether an entry is in a block's `<capacity>` (judged above).
    let mut held = vec![false; image.symbols.len()];
    for (bi, b) in sound.iter().enumerate() {
        let lo = usize::try_from(b.first).unwrap_or(usize::MAX);
        let hi = usize::try_from(b.first + b.capacity).unwrap_or(usize::MAX);
        let mid = usize::try_from(b.first + b.count).unwrap_or(usize::MAX);
        for (k, y) in image.symbols.iter().enumerate().take(hi).skip(lo) {
            let e = y.index;
            held[k] = true;
            if k < mid {
                owner[k] = Some(bi);
                if y.sym.binding() != STB_LOCAL {
                    out.push(pi7(b, format!("entry {e} ('{}') is not STB_LOCAL", y.name)));
                } else if filler(y) {
                    out.push(pi7(b, format!("entry {e} is a filler, not a local of the block")));
                    continue;
                }
                let at = u64::from(y.sym.st_name);
                let end = b.strtab_off + b.strtab_cap;
                let named = string_at(strtab, at);
                let inside = at >= b.strtab_off && named.is_some_and(|(_, nul)| nul < end);
                let shared =
                    named.is_some_and(|(n, _)| SHARED.contains(&n)) && region_of(at).is_none();
                if !inside && !shared {
                    out.push(pi7(
                        b,
                        format!(
                            "entry {e} ('{}') names .strtab+{at:#x}, not a string inside the block's region [{:#x}, {end:#x}) nor a shared name of the fixed table",
                            y.name, b.strtab_off
                        ),
                    ));
                }
            } else if !filler(y) {
                out.push(pi7(
                    b,
                    format!(
                        "entry {e} ('{}') is in the block's capacity past its count and not a filler",
                        y.name
                    ),
                ));
            }
        }
    }
    // P-I7 4: no other entry names a string inside a block's region.
    for (k, y) in image.symbols.iter().enumerate().skip(1) {
        if filler(y) {
            continue;
        }
        let Some(bi) = region_of(u64::from(y.sym.st_name)) else { continue };
        if owner[k] == Some(bi) {
            continue;
        }
        let b = sound[bi];
        out.push(pi7(
            b,
            format!(
                "entry {} ('{}', {}) names .strtab+{:#x}, inside the block's region [{:#x}, +{:#x}), and is not one of the block's entries",
                y.index,
                y.name,
                if y.sym.binding() == STB_LOCAL { "local" } else { "not local" },
                y.sym.st_name,
                b.strtab_off,
                b.strtab_cap
            ),
        ));
    }
    // P-I7 5: every entry of the local area but the fillers is a block's.
    if map.incremental {
        let n = usize::try_from(info).unwrap_or(usize::MAX);
        for (k, y) in image.symbols.iter().enumerate().take(n).skip(1) {
            if !held[k] && !filler(y) {
                out.push(finding(
                    7,
                    format!(
                        ".symtab entry {} ('{}') of the local area is neither a filler nor in any symbol block of the map",
                        y.index, y.name
                    ),
                ));
            }
        }
    }
}

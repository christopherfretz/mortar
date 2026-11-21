//! Split DWARF in an update: the unit kinds of a changed file (`spec/incr.md`
//! IN-55, IN-55a as narrowed by IN-55 option (A); WP-229, G-2). WP-213
//! (U-1) made the file and its `mod` line; G-2 fills it (entry points ruled
//! by the supervisor, `briefs/wp-229-rulings.md` 7-11). Until G-3 joins
//! it, an update with a changed file carrying debug sections is
//! `update-not-implemented (debug)` (the join's check on
//! `ChangedFile::debug`).
//!
//! **What it answers.** For one version of a changed file, from section
//! names, COMDAT membership and `.debug_info` unit headers alone (no
//! relocation, no DIE read), first match wins:
//! 1. `debug-name-index` (IN-55): a non-empty `.debug_pubnames`,
//!    `.debug_pubtypes` or `.debug_names`. The GNU sets
//!    (`.debug_gnu_pubnames`/`pubtypes`) are NOT a reason: an
//!    incremental-mode output leaves them out (option (A),
//!    `mortar_layout::incr_discard`).
//! 2. `debug-unit-kind` (IN-55a): a non-empty `.debug_types` (DWARF 4 type
//!    units); a `.debug_*.dwo` section (`-gsplit-dwarf=single`: the split
//!    units inside the object); a `.debug_info` in a COMDAT group (DWARF 5
//!    type units, g++ and clang++); a unit whose type is not
//!    `DW_UT_compile` (1) or `DW_UT_skeleton` (4) (type, partial,
//!    split-compile, split-type, an unknown type); a 64-bit DWARF unit
//!    (WP-228 B2-6); a header that cannot be read (a version outside 2..=5,
//!    a length past its section, contents that cannot be read from the
//!    file).
//! 3. Otherwise [`Handled`]: compile units (DWARF 2 to 5) and skeleton
//!    units are ordinary per-file contributions for G-1 (IN-50..IN-57).
//!
//! Callers (the join, G-3, at U2/U4; any `Err` ends the update before its
//! first write): [`classify_object`] on the NEW bytes of each changed,
//! appeared, drawn-in or newly extracted file when the caller holds them
//! whole, [`classify_input`] when they are the front's (the store's parts
//! view); [`classify_rows`] on the stored `SECTIONS` rows (the OLD
//! version) of each changed, gone or no longer extracted file. Cost O(the
//! file's section headers + its `.debug_info` unit headers).
//!
//! **Residency (WP-278).** The staged store (`spec/input.md` §11.1 (b))
//! keeps a section larger than 64 KiB of an object larger than 64 KiB on
//! disk, and `.debug_info` is such a section in any real-sized `-g`
//! object: [`Section::data`] over the parts view is `NotResident` for it.
//! [`classify_input`] then reads the unit headers from the file through
//! `Inputs::copy_to`, the full link's read of a section that is not
//! resident: one positioned read of at most 7 bytes per unit, never the
//! section (nor the file) whole.

use mortar_core::id::FileId;
use mortar_core::incr::IncrReason;
use mortar_core::inputs::{Inputs, Reader};
use mortar_elf::consts::{SHF_ALLOC, SHT_GROUP, SHT_NOBITS};
use mortar_elf::{Object, Section, SectionIndex};

use super::view::{ChangedFiles, Rows, StateView};
use crate::format::SectionRow;

/// What IN-55 and IN-55a let through for one version of a file: the
/// counts G-1 reads. From [`classify_rows`] (names alone) the unit counts
/// are 0: only the new bytes have unit headers to count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Handled {
    /// Compile units (`DW_UT_compile`, or any unit of DWARF 2 to 4).
    pub compile_units: u32,
    /// `DW_UT_skeleton` units (DWARF 5 split DWARF).
    pub skeletons: u32,
    /// A DWARF 2-4 unit in a file with a non-empty `.debug_addr`: a DWARF 4
    /// split-DWARF compile unit (IN-55a: the header cannot tell; the
    /// section does, measured in WP-229's record).
    pub dwarf4_split: bool,
    /// A non-empty `.debug_gnu_pubnames` or `.debug_gnu_pubtypes`: left out
    /// of the output (option (A)), so no reason.
    pub gnu_index: bool,
}

/// IN-55's name indexes that still fall back (`debug-name-index`).
pub const NAME_INDEXES: [&[u8]; 3] = [b".debug_pubnames", b".debug_pubtypes", b".debug_names"];

/// `DW_UT_compile`.
const DW_UT_COMPILE: u8 = 1;
/// `DW_UT_skeleton`.
const DW_UT_SKELETON: u8 = 4;

/// The `DW_UT_*` name of a unit type, for the reason's detail.
fn unit_type_name(t: u8) -> String {
    match t {
        2 => "DW_UT_type".to_owned(),
        3 => "DW_UT_partial".to_owned(),
        5 => "DW_UT_split_compile".to_owned(),
        6 => "DW_UT_split_type".to_owned(),
        t => format!("unit type {t:#x}"),
    }
}

fn unit_kind(what: impl Into<String>) -> IncrReason {
    IncrReason::DebugUnitKind(what.into())
}

/// The answer of one section from its name, flags, size and COMDAT
/// membership (both versions read sections this way): `Err` for rules 1
/// and 2's name tests, `Ok(true)` for a non-empty GNU set.
fn by_name(name: &[u8], flags: u64, sh_type: u32, size: u64, comdat: bool) -> Rows<bool> {
    let text = || String::from_utf8_lossy(name).into_owned();
    if flags & SHF_ALLOC != 0 || sh_type == SHT_NOBITS || !name.starts_with(b".debug_") {
        return Ok(false);
    }
    if size > 0 && NAME_INDEXES.contains(&name) {
        return Err(IncrReason::DebugNameIndex(text()));
    }
    if size > 0 && name == b".debug_types" {
        return Err(unit_kind(".debug_types: DWARF 4 type units"));
    }
    if name.ends_with(b".dwo") {
        return Err(unit_kind(format!("{}: split units inside the object", text())));
    }
    if comdat && name == b".debug_info" {
        return Err(unit_kind(".debug_info in a COMDAT group: type units"));
    }
    Ok(size > 0 && gnu_set(name))
}

/// IN-55 option (A)'s two names (`mortar_layout::incr_discard::LEFT_OUT`;
/// `mortar_incr` does not depend on `mortar_layout`, so the two names are
/// repeated here and a test holds them equal to the spec's).
fn gnu_set(name: &[u8]) -> bool {
    name == b".debug_gnu_pubnames" || name == b".debug_gnu_pubtypes"
}

/// Rule 1 before rule 2 over a whole file: the first name-index section
/// wins over any earlier unit-kind section.
fn first_reason(found: &mut Option<IncrReason>, r: IncrReason) {
    let better = matches!(r, IncrReason::DebugNameIndex(_))
        && !matches!(found, Some(IncrReason::DebugNameIndex(_)));
    if found.is_none() || better {
        *found = Some(r);
    }
}

/// The unit headers of one `.debug_info` section held whole: counts into
/// `h`, or the reason ([`units_by`] over the slice).
fn units(data: &[u8], h: &mut Handled, v4: &mut bool) -> Rows<()> {
    let mut at = |p: u64, out: &mut [u8]| -> Result<(), String> {
        let src = usize::try_from(p)
            .ok()
            .and_then(|p| data.get(p..p.checked_add(out.len())?))
            .ok_or_else(|| "past the section".to_owned())?;
        out.copy_from_slice(src);
        Ok(())
    };
    // A read inside the slice cannot fail: `units_by` reads within `len`.
    units_by(data.len() as u64, &mut at, h, v4).unwrap_or_else(|e| Err(unit_kind(e)))
}

/// The most bytes of a unit header the walk reads: `unit_length` (4),
/// `version` (2) and DWARF 5's `unit_type` (1).
const HEADER: u64 = 7;

/// The unit headers of one `.debug_info` section of `len` bytes, read
/// through `at` (bytes `p..p + out.len()` of the section into `out`; at
/// most [`HEADER`] bytes per unit, at each unit's start): counts into `h`,
/// or the reason. Reads `unit_length` (a 64-bit unit is
/// `debug-unit-kind`), `version` and, for DWARF 5, `unit_type`; never a
/// DIE. The outer `Err` is a read that failed, with `at`'s words.
fn units_by(
    len: u64,
    at: &mut dyn FnMut(u64, &mut [u8]) -> Result<(), String>,
    h: &mut Handled,
    v4: &mut bool,
) -> Result<Rows<()>, String> {
    let bad = |what: &str| unit_kind(format!(".debug_info: unreadable unit header ({what})"));
    let mut head = [0u8; HEADER as usize];
    let mut p = 0u64;
    while p < len {
        // `len - p` is at least 1: at most HEADER bytes, as a usize.
        let n = (len - p).min(HEADER) as usize;
        at(p, &mut head[..n])?;
        if n < 4 {
            return Ok(Err(bad("truncated unit_length")));
        }
        let unit = u64::from(u32::from_le_bytes([head[0], head[1], head[2], head[3]]));
        if unit == 0xffff_ffff {
            // WP-228 B2-6 (B1-5): a 64-bit DWARF unit in a changed file is
            // `debug-unit-kind` in v1 (the update's offsets and neutral
            // forms are 32-bit words).
            return Ok(Err(unit_kind(".debug_info: a 64-bit DWARF unit")));
        } else if unit >= 0xffff_fff0 {
            return Ok(Err(bad("reserved unit_length")));
        }
        let body = p + 4;
        let Some(end) = body.checked_add(unit).filter(|&e| e <= len && unit >= 3) else {
            return Ok(Err(bad("a unit past its section")));
        };
        // `unit >= 3` and `end <= len`: the HEADER bytes are all read.
        let version = u16::from_le_bytes([head[4], head[5]]);
        match version {
            2..=4 => {
                h.compile_units += 1;
                *v4 = true;
            }
            5 => match head[6] {
                DW_UT_COMPILE => h.compile_units += 1,
                DW_UT_SKELETON => h.skeletons += 1,
                t => return Ok(Err(unit_kind(format!(".debug_info: {}", unit_type_name(t))))),
            },
            v => return Ok(Err(bad(&format!("version {v}")))),
        }
        p = end;
    }
    Ok(Ok(()))
}

/// How [`classify`] reads a `.debug_info` whose contents are not resident:
/// bytes `range` of the object (offsets in it) into `out`.
type CopyFn<'c> = &'c dyn Fn(std::ops::Range<u64>, &mut [u8]) -> Result<(), String>;

/// One `.debug_info`'s unit headers into `h`, or the rule's reason (the
/// inner `Err`): from its resident bytes, else (over a parts view) through
/// `copy`. The outer `Err` is `section i: unreadable (why)`: contents out
/// of bounds, not resident without `copy`, or a copy that failed.
fn section_units(
    s: &Section<'_>,
    i: u32,
    copy: Option<CopyFn<'_>>,
    h: &mut Handled,
    v4: &mut bool,
) -> Result<Rows<()>, IncrReason> {
    let unreadable = |why: String| unit_kind(format!("section {i}: unreadable ({why})"));
    match s.data() {
        Ok(data) => Ok(units(data, h, v4)),
        Err(e @ mortar_elf::Error::NotResident { .. }) => {
            let Some(copy) = copy else { return Err(unreadable(e.to_string())) };
            let range = match s.file_range() {
                Ok(Some(r)) => r.start as u64..r.end as u64,
                Ok(None) => return Ok(Ok(())),
                Err(e) => return Err(unreadable(e.to_string())),
            };
            let base = range.start;
            let mut at = |p: u64, out: &mut [u8]| -> Result<(), String> {
                let start = base + p;
                copy(start..start + out.len() as u64, out)
            };
            units_by(range.end - base, &mut at, h, v4)
                .map_err(|why| unreadable(format!("copy: {why}")))
        }
        Err(e) => Err(unreadable(e.to_string())),
    }
}

/// IN-55/IN-55a on the NEW bytes of a changed, appeared, drawn-in or
/// newly extracted file: section names, COMDAT membership and the unit
/// headers of every non-grouped `.debug_info`.
///
/// For an object the caller holds whole (`Object::parse`); over the
/// store's parts view a `.debug_info` on disk is `unreadable` here:
/// [`classify_input`] reads it.
///
/// # Errors
/// `debug-name-index` or `debug-unit-kind` with the section and why (the
/// join names the file); a section the reader cannot read is
/// `debug-unit-kind` (conservative: the full link reports it as its own
/// diagnostic).
pub fn classify_object(object: &Object<'_>) -> Result<Handled, IncrReason> {
    classify(object, None)
}

/// [`classify_object`] on the object of file `file` of `inputs` (the
/// front's, read through the store, `spec/input.md` §11): a `.debug_info`
/// the staged store keeps on disk has its unit headers copied from the file
/// by `Inputs::copy_to` (WP-278), as the full link reads a section that is
/// not resident.
///
/// # Errors
/// [`classify_object`]'s; `debug-unit-kind` for a file that is not an
/// object, and for a copy that fails (`section N: unreadable (copy: ..)`:
/// the file changed or could not be read).
pub fn classify_input(inputs: &Inputs<'_>, file: FileId) -> Result<Handled, IncrReason> {
    let Some(Reader::Object(object)) = inputs.files.get(file).map(|f| &f.reader) else {
        return Err(unit_kind(format!("file {}: not an object of the link", file.0)));
    };
    let copy = |r: std::ops::Range<u64>, out: &mut [u8]| {
        inputs.copy_to(file, r, out).map_err(|e| format!("{e:?}"))
    };
    classify(object, Some(&copy))
}

/// [`classify_object`] with `copy` for a `.debug_info` that is not
/// resident.
fn classify(object: &Object<'_>, copy: Option<CopyFn<'_>>) -> Result<Handled, IncrReason> {
    let unreadable = |i: u32| unit_kind(format!("section {i}: unreadable"));
    let n = object.section_count();
    let mut in_comdat = vec![false; n as usize];
    for i in 1..n {
        let s = object.section(SectionIndex(i)).map_err(|_| unreadable(i))?;
        if s.sh_type() != SHT_GROUP {
            continue;
        }
        let g = s.group().map_err(|_| unreadable(i))?;
        if !g.is_comdat() {
            continue;
        }
        for m in g.members() {
            let m = m.map_err(|_| unreadable(i))?;
            if let Some(slot) = in_comdat.get_mut(m.0 as usize) {
                *slot = true;
            }
        }
    }
    let mut h = Handled::default();
    let mut found = None;
    let (mut v4, mut addr) = (false, false);
    for i in 1..n {
        let s = object.section(SectionIndex(i)).map_err(|_| unreadable(i))?;
        let name = s.name().map_err(|_| unreadable(i))?;
        let comdat = in_comdat[i as usize];
        match by_name(name, s.flags(), s.sh_type(), s.size(), comdat) {
            Err(r) => first_reason(&mut found, r),
            Ok(gnu) => h.gnu_index |= gnu,
        }
        let debug = s.flags() & SHF_ALLOC == 0 && s.sh_type() != SHT_NOBITS;
        addr |= debug && name == b".debug_addr" && s.size() > 0;
        if debug && name == b".debug_info" && !comdat && found.is_none() {
            // A section that cannot be read ends the walk (`?`), as before.
            if let Err(r) = section_units(&s, i, copy, &mut h, &mut v4)? {
                first_reason(&mut found, r);
            }
        }
    }
    if let Some(r) = found {
        return Err(r);
    }
    h.dwarf4_split = v4 && addr;
    Ok(h)
}

/// IN-55/IN-55a on the OLD version of a changed, gone or no longer
/// extracted file: its stored `SECTIONS` rows (names by bytes through
/// [`ChangedFiles::string`], flags, type, size, COMDAT anchor). Without
/// the old bytes it cannot read unit headers, so it sees rule 1 and rule
/// 2's name tests only (what that misses: WP-229's record, ruling 9); its
/// unit counts are 0.
///
/// # Errors
/// As [`classify_object`]; `state-corrupt` when a name cannot be read.
pub fn classify_rows(
    view: &StateView<'_>,
    changed: &ChangedFiles,
    rows: &[SectionRow],
) -> Result<Handled, IncrReason> {
    let mut h = Handled::default();
    let mut found = None;
    for r in rows {
        let name = changed.string(view, r.name)?;
        match by_name(&name, r.sh_flags, r.sh_type, r.size, r.anchor_kind == 1) {
            Err(reason) => first_reason(&mut found, reason),
            Ok(gnu) => h.gnu_index |= gnu,
        }
    }
    found.map_or(Ok(h), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit5(t: u8) -> Vec<u8> {
        // unit_length 8: version 5, unit_type, address size 8, abbrev 0.
        let mut v = 8u32.to_le_bytes().to_vec();
        v.extend([5, 0, t, 8, 0, 0, 0, 0]);
        v
    }

    fn unit4() -> Vec<u8> {
        let mut v = 7u32.to_le_bytes().to_vec();
        v.extend([4, 0, 0, 0, 0, 0, 8]);
        v
    }

    // spec: incr.md IN-55a (unit headers), as narrowed by option (A).
    #[test]
    fn unit_headers() {
        let (mut h, mut v4) = (Handled::default(), false);
        let data = [unit5(1), unit5(4), unit4()].concat();
        units(&data, &mut h, &mut v4).unwrap();
        assert_eq!((h.compile_units, h.skeletons, v4), (2, 1, true));
        for (t, why) in
            [(2, "DW_UT_type"), (3, "DW_UT_partial"), (5, "split_compile"), (6, "split_type")]
        {
            let e = units(&unit5(t), &mut h, &mut v4).unwrap_err();
            assert!(matches!(&e, IncrReason::DebugUnitKind(d) if d.contains(why)), "{e:?}");
        }
        // WP-228 B2-6: a 64-bit unit is `debug-unit-kind`.
        let mut long = vec![0xff; 4];
        long.extend(8u64.to_le_bytes());
        long.extend([5, 0, 1, 8, 0, 0, 0, 0]);
        let e = units(&long, &mut h, &mut v4).unwrap_err();
        assert!(matches!(&e, IncrReason::DebugUnitKind(d) if d.contains("64-bit")), "{e:?}");
        for bad in [
            vec![1, 0],
            100u32.to_le_bytes().to_vec(),
            [unit5(1), vec![0xf0, 0xff, 0xff, 0xff]].concat(),
        ] {
            assert!(matches!(units(&bad, &mut h, &mut v4), Err(IncrReason::DebugUnitKind(_))));
        }
        let mut v6 = unit5(1);
        v6[4] = 6;
        assert!(
            matches!(units(&v6, &mut h, &mut v4), Err(IncrReason::DebugUnitKind(d)) if d.contains("version 6"))
        );
    }

    // spec: incr.md IN-55a (WP-278): the walk over a reader (a section the
    // staged store keeps on disk) reads at most 7 bytes at each unit's
    // start and nothing else, answers what the walk over the slice
    // answers, and says when a read fails (the outer `Err`).
    #[test]
    fn unit_headers_through_a_reader() {
        let data = [unit5(1), unit5(4), unit4()].concat();
        let len = data.len() as u64;
        let mut reads = Vec::new();
        let (mut h, mut v4) = (Handled::default(), false);
        {
            let mut at = |p: u64, out: &mut [u8]| -> Result<(), String> {
                reads.push((p, out.len()));
                out.copy_from_slice(&data[p as usize..p as usize + out.len()]);
                Ok(())
            };
            assert_eq!(units_by(len, &mut at, &mut h, &mut v4), Ok(Ok(())));
        }
        assert_eq!(reads, [(0, 7), (12, 7), (24, 7)]);
        assert_eq!((h.compile_units, h.skeletons, v4), (2, 1, true));
        let mut fail = |_: u64, _: &mut [u8]| -> Result<(), String> { Err("Changed".into()) };
        assert_eq!(units_by(len, &mut fail, &mut h, &mut v4), Err("Changed".to_owned()));
        // A rule's reason through a reader is the slice's.
        let t = unit5(2);
        let mut at = |p: u64, out: &mut [u8]| -> Result<(), String> {
            out.copy_from_slice(&t[p as usize..p as usize + out.len()]);
            Ok(())
        };
        let got = units_by(t.len() as u64, &mut at, &mut h, &mut v4).expect("read");
        assert_eq!(got, units(&t, &mut h, &mut v4));
        assert!(matches!(got, Err(IncrReason::DebugUnitKind(d)) if d.contains("DW_UT_type")));
    }

    // spec: incr.md IN-55 (name indexes before unit kinds), option (A).
    #[test]
    fn names() {
        assert_eq!(by_name(b".debug_gnu_pubnames", 0, 1, 4, false), Ok(true));
        assert_eq!(by_name(b".debug_gnu_pubtypes", 0, 1, 0, false), Ok(false));
        assert_eq!(by_name(b".debug_gnu_pubnames", SHF_ALLOC, 1, 4, false), Ok(false));
        assert!(matches!(
            by_name(b".debug_names", 0, 1, 4, false),
            Err(IncrReason::DebugNameIndex(_))
        ));
        assert_eq!(by_name(b".debug_names", 0, 1, 0, false), Ok(false));
        assert!(matches!(
            by_name(b".debug_types", 0, 1, 4, false),
            Err(IncrReason::DebugUnitKind(_))
        ));
        assert!(matches!(
            by_name(b".debug_info.dwo", 0x8000_0000, 1, 4, false),
            Err(IncrReason::DebugUnitKind(_))
        ));
        assert!(matches!(
            by_name(b".debug_info", 0, 1, 4, true),
            Err(IncrReason::DebugUnitKind(_))
        ));
        assert_eq!(by_name(b".debug_info", 0, 1, 4, false), Ok(false));
        let mut found = None;
        first_reason(&mut found, unit_kind("a"));
        first_reason(&mut found, IncrReason::DebugNameIndex("b".into()));
        first_reason(&mut found, unit_kind("c"));
        assert_eq!(found, Some(IncrReason::DebugNameIndex("b".into())));
    }
}

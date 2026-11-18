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
//!    split-compile, split-type, an unknown type); a header that cannot be
//!    read (a version outside 2..=5, a length past its section, contents
//!    not resident).
//! 3. Otherwise [`Handled`]: compile units (DWARF 2 to 5) and skeleton
//!    units are ordinary per-file contributions for G-1 (IN-50..IN-57).
//!
//! Callers (the join, G-3, at U2/U4; any `Err` ends the update before its
//! first write): [`classify_object`] on the NEW bytes of each changed,
//! appeared, drawn-in or newly extracted file; [`classify_rows`] on the
//! stored `SECTIONS` rows (the OLD version) of each changed, gone or no
//! longer extracted file. Cost O(the file's section headers + its
//! `.debug_info` unit headers).

use mortar_core::incr::IncrReason;
use mortar_elf::consts::{SHF_ALLOC, SHT_GROUP, SHT_NOBITS};
use mortar_elf::{Object, SectionIndex};

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

/// The unit headers of one `.debug_info` section: counts into `h`, or the
/// reason. Reads `unit_length` (32- or 64-bit DWARF), `version` and, for
/// DWARF 5, `unit_type`; never a DIE.
fn units(data: &[u8], h: &mut Handled, v4: &mut bool) -> Rows<()> {
    let bad = |what: &str| unit_kind(format!(".debug_info: unreadable unit header ({what})"));
    let u32_at = |p: usize| -> Option<u32> {
        Some(u32::from_le_bytes(data.get(p..p.checked_add(4)?)?.try_into().ok()?))
    };
    let mut p = 0usize;
    while p < data.len() {
        let mut len = u64::from(u32_at(p).ok_or_else(|| bad("truncated unit_length"))?);
        p += 4;
        if len == 0xffff_ffff {
            let lo = u64::from(u32_at(p).ok_or_else(|| bad("truncated unit_length"))?);
            let hi = u64::from(u32_at(p + 4).ok_or_else(|| bad("truncated unit_length"))?);
            len = lo | (hi << 32);
            p += 8;
        } else if len >= 0xffff_fff0 {
            return Err(bad("reserved unit_length"));
        }
        let end = usize::try_from(len)
            .ok()
            .and_then(|l| p.checked_add(l))
            .filter(|&e| e <= data.len() && len >= 3)
            .ok_or_else(|| bad("a unit past its section"))?;
        let version = u16::from_le_bytes([data[p], data[p + 1]]);
        match version {
            2..=4 => {
                h.compile_units += 1;
                *v4 = true;
            }
            5 => match data[p + 2] {
                DW_UT_COMPILE => h.compile_units += 1,
                DW_UT_SKELETON => h.skeletons += 1,
                t => return Err(unit_kind(format!(".debug_info: {}", unit_type_name(t)))),
            },
            v => return Err(bad(&format!("version {v}"))),
        }
        p = end;
    }
    Ok(())
}

/// IN-55/IN-55a on the NEW bytes of a changed, appeared, drawn-in or
/// newly extracted file: section names, COMDAT membership and the unit
/// headers of every non-grouped `.debug_info`.
///
/// # Errors
/// `debug-name-index` or `debug-unit-kind` with the section and why (the
/// join names the file); a section the reader cannot read is
/// `debug-unit-kind` (conservative: the full link reports it as its own
/// diagnostic).
pub fn classify_object(object: &Object<'_>) -> Result<Handled, IncrReason> {
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
            let data = s.data().map_err(|_| unreadable(i))?;
            if let Err(r) = units(data, &mut h, &mut v4) {
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
        let mut long = vec![0xff; 4];
        long.extend(8u64.to_le_bytes());
        long.extend([5, 0, 1, 8, 0, 0, 0, 0]);
        units(&long, &mut h, &mut v4).unwrap();
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

//! Bound uses in a shared object (`spec/dyn.md` D-117: the table, the
//! address-level rule and its boundary rule; M4, WP-117).
//!
//! Non-PIC code can reach an exported datum without naming it: GCC's
//! AArch64 `-fPIE` reads its own exported `int g` through the section
//! anchor (`ADR_PREL_PG_HI21 .data + 8`). The table judges a use by the
//! key its relocation names, so such a use would link and bind `g` inside
//! the library while `g` stays preemptible. This module is D-117's
//! **address-level rule**: per object O, its *preemptible data
//! definitions* (an interval `[st_value, st_value + max(st_size, 1))` in
//! a section T without `SHF_EXECINSTR`, for each global of O defined in T
//! whose key `Resolution.preemptible` holds), and per relocation of O
//! whose symbol is local to O and defined in such a T, the address it
//! names (`st_value + A`), judged against the intervals with the
//! **boundary rule**.
//!
//! The facts of a file are built once, on the first record that needs
//! them ([`crate::view::View::bound_facts`]), and only in a `Shared`
//! AArch64 link: an executable never reaches this module (D-117: "Neither
//! changes a link of an executable"). x86 (WP-127, WP-130) judges only
//! the exact forms of D-117's x86 scope and is not here yet: its
//! `Shared` output is refused before the scan.
//!
//! The analysis of D-117 ("Bound use through a register", WP-119's
//! `crate::regflow`) joins here: [`analysis`] is its hook, and its
//! findings are judged by the same [`judge`].

use std::borrow::Cow;

use mortar_core::{FileId, SymKind, SymbolId};
use mortar_diag::{Diagnostic, Key, Place};
use mortar_elf::consts::{SHF_EXECINSTR, SHT_REL, SHT_RELA, STT_SECTION};
use mortar_elf::{Object, SectionIndex, SymbolIndex, SymbolSection};

use crate::view::{Raw, ReadFail, View, contents, rec_table};

/// `R_AARCH64_NONE` and its withdrawn alias (no `SIZE` type on AArch64).
const NONE_TYPES: [u32; 2] = [0, 256];

/// The boundary rule's "access at the address" (D-117 (i)): the AArch64
/// load/store `:lo12:` forms `LDST8`, `LDST16`, `LDST32`, `LDST64`,
/// `LDST128_ABS_LO12_NC`, and `LD_PREL_LO19`.
const ACCESS_TYPES: [u32; 6] = [278, 284, 285, 286, 299, 273];

/// The GOT forms of the self-GOT test (D-117 (ii)): `MOVW_GOTOFF_G*`
/// (300-306), `GOT_LD_PREL19`, `LD64_GOTOFF_LO15`, `ADR_GOT_PAGE`,
/// `LD64_GOT_LO12_NC`, `LD64_GOTPAGE_LO15` (309-313).
const fn is_got_form(r_type: u32) -> bool {
    matches!(r_type, 300..=306 | 309..=313)
}

/// One preemptible data definition of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PreDef {
    section: u32,
    lo: u64,
    hi: u64,
    /// The defining symbol (its name is the diagnostic's).
    index: SymbolIndex,
    key: SymbolId,
}

/// What the rule knows of one object (`None` from [`FileFacts::of`] when
/// it defines no preemptible data: the common case, which costs one walk
/// of its globals).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileFacts {
    /// Sorted by `(section, lo)`.
    defs: Vec<PreDef>,
    /// `(section, st_value + st_size)` of every local of the file with a
    /// non-zero size, in a section that holds a preemptible data
    /// definition; sorted.
    ends: Vec<(u32, u64)>,
    /// D-117 (ii): a relocation of a GOT form names one of the file's own
    /// preemptible data definitions by a non-local symbol.
    pub self_got: bool,
    /// The analysis's verdict (WP-119, `crate::regflow`), run once per
    /// file with the facts: per bound use, the code section it is in and
    /// its keyed `non-pic-in-shared-object` error; or, when the analysis
    /// could not run, the diagnostic of why (reported for each code
    /// section of the file the scan walks: a file whose code is all dead
    /// puts no use in the output).
    analysed: Result<Vec<(u32, Key, Diagnostic)>, Diagnostic>,
}

/// A bound use: the rule's (or the analysis's) verdict on one named
/// address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Bound {
    /// The defining symbol of the datum the address lies in.
    pub def: SymbolIndex,
    /// The address is a boundary address (D-117 "The boundary"): the
    /// note names `-fdata-sections`.
    pub boundary: bool,
    /// The object reaches its exported data through the GOT (it was
    /// compiled `-fPIC`): the use may be an address formed before the
    /// start of a following static object (D-117 "Before the start of a
    /// local object"); the note adds that remedy.
    pub self_got: bool,
}

impl FileFacts {
    /// The facts of object `file`, or `None` when it has no preemptible
    /// data definition (or is not an object).
    pub fn of(view: &View<'_, '_>, file: FileId) -> Option<Self> {
        let prep = view.prep(file)?;
        let obj = prep.obj?;
        let table = prep.table()?;
        let map = view.inputs.sym_map_of(file);
        let first = table.first_global().0;
        let preemptible = &view.resolution.preemptible;
        if preemptible.is_empty() {
            return None;
        }
        let data_section =
            |i: SectionIndex| obj.section(i).is_ok_and(|s| s.flags() & SHF_EXECINSTR == 0);
        let mut defs = Vec::new();
        for (index, sym) in table.globals() {
            let Some(&key) = map.get((index.0 - first) as usize) else { continue };
            if !preemptible.contains(key) {
                continue;
            }
            let Ok(SymbolSection::Section(t)) = table.section_of(index, &sym) else { continue };
            if !data_section(t) {
                continue;
            }
            let hi = sym.st_value.saturating_add(sym.st_size.max(1));
            defs.push(PreDef { section: t.0, lo: sym.st_value, hi, index, key });
        }
        if defs.is_empty() {
            return None;
        }
        defs.sort_unstable_by_key(|d| (d.section, d.lo, d.index.0));
        let mut ends = Vec::new();
        for (index, sym) in table.locals() {
            if sym.st_size == 0 || sym.sym_type() == STT_SECTION {
                continue;
            }
            let Ok(SymbolSection::Section(t)) = table.section_of(index, &sym) else { continue };
            if defs.iter().any(|d| d.section == t.0) {
                ends.push((t.0, sym.st_value.saturating_add(sym.st_size)));
            }
        }
        ends.sort_unstable();
        ends.dedup();
        let mut keys: Vec<SymbolId> = defs.iter().map(|d| d.key).collect();
        keys.sort_unstable();
        keys.dedup();
        let own = |r_sym: u32| {
            r_sym >= first
                && map.get((r_sym - first) as usize).is_some_and(|k| keys.binary_search(k).is_ok())
        };
        let analysed = analyse(view, file, &obj, &defs);
        let mut buf = Vec::new();
        let mut self_got = false;
        for s in obj.sections() {
            if !matches!(s.sh_type(), SHT_RELA | SHT_REL) {
                continue;
            }
            // A table that cannot be read is the walk's to report.
            let Ok((recs, _, _)) = rec_table(view, file, &obj, s.index(), &mut buf) else {
                continue;
            };
            let n = u32::try_from(recs.len()).unwrap_or(u32::MAX);
            if (0..n).filter_map(|i| recs.raw(i)).any(|r| is_got_form(r.r_type) && own(r.r_sym)) {
                self_got = true;
                break;
            }
        }
        Some(Self { defs, ends, self_got, analysed })
    }

    /// Whether section `section` of the file holds a preemptible data
    /// definition (the pre-filter of a record).
    pub fn has(&self, section: u32) -> bool {
        self.defs.iter().any(|d| d.section == section)
    }
}

/// D-117's verdict on the address `addr` (an input offset in section
/// `section` of the file) named by a use; `access` when the use is an
/// access at the address (the boundary rule's (i)). The address is a
/// bound use when it lies in a preemptible data definition's interval,
/// unless it is a boundary address of that definition (its `st_value`
/// and the end of a sized local of the section) used neither as an
/// access nor by an object without a self-GOT.
pub(crate) fn judge(f: &FileFacts, section: u32, addr: u64, access: bool) -> Option<Bound> {
    // The first definition (by start) whose interval holds the address;
    // two overlapping definitions (aliases) give the first one's name.
    let d = f.defs.iter().find(|d| d.section == section && d.lo <= addr && addr < d.hi)?;
    let boundary = addr == d.lo && f.ends.binary_search(&(section, addr)).is_ok();
    if boundary && !access && f.self_got {
        return None;
    }
    Some(Bound { def: d.index, boundary, self_got: f.self_got })
}

/// The address-level rule for one relocation of the file at a place in
/// an `SHF_ALLOC` section named `place` (D-117 "A use"): its symbol is
/// local to the file and defined in a section that holds a preemptible
/// data definition, its type is not `NONE`, its place not `.eh_frame` or
/// `.gcc_except_table*`. `addend` is the record's `A`.
pub(crate) fn rule(
    view: &View<'_, '_>,
    f: &FileFacts,
    key: mortar_core::SymKey,
    raw: &Raw,
    addend: i64,
    place: &[u8],
) -> Option<Bound> {
    if NONE_TYPES.contains(&raw.r_type)
        || place == b".eh_frame"
        || place.starts_with(b".gcc_except_table")
    {
        return None;
    }
    let SymKind::Local(e) = key.kind() else { return None };
    let table = view.prep(e.file)?.table()?;
    let sym = table.get(e.index).ok()?;
    let SymbolSection::Section(t) = table.section_of(e.index, &sym).ok()? else { return None };
    if !f.has(t.0) {
        return None;
    }
    let addr = sym.st_value.wrapping_add(addend as u64);
    judge(f, t.0, addr, ACCESS_TYPES.contains(&raw.r_type))
}

/// D-117's analysis (WP-119's [`crate::regflow::analyse`]) of object
/// `obj` (`file`) for its preemptible data definitions `defs`: each bound
/// use as the code section it is in and its error (`BoundUse::diagnostic`,
/// placed at the instruction), keyed on the record of the code section's
/// relocation table that formed the address (record 0 when that record
/// applies to another section: a literal word outside the code), or why
/// the analysis could not run. The analysis's own boundary rule ("access"
/// per item 6, self-GOT) is its verdict; nothing is judged again.
fn analyse<'s>(
    view: &View<'_, 's>,
    file: FileId,
    obj: &Object<'s>,
    defs: &[PreDef],
) -> Result<Vec<(u32, Key, Diagnostic)>, Diagnostic> {
    let fref = view.inputs.files[file].display();
    let preemptible: Vec<SymbolIndex> = defs.iter().map(|d| d.index).collect();
    let mut buf = Vec::new();
    let mut bytes = |i: SectionIndex| -> Result<Cow<'_, [u8]>, ReadFail> {
        let s = obj.section(i).map_err(|e| ReadFail::Malformed(e.to_string()))?;
        if let Ok(d) = s.data() {
            return Ok(Cow::Borrowed(d));
        }
        contents(view, file, &s, &mut buf).map(|b| Cow::Owned(b.to_vec()))
    };
    let analysis = match crate::regflow::analyse(obj, &preemptible, &mut bytes) {
        Ok(a) => a,
        Err(crate::regflow::Fault::Malformed(what)) => {
            return Err(ReadFail::Malformed(what).diagnostic(&fref));
        }
        Err(crate::regflow::Fault::Contents(i, fail)) => {
            let name = view.section_name(file, i.0);
            return Err(fail.diagnostic(&fref).at(Place::section(fref.clone(), name, 0)));
        }
    };
    let mut out = Vec::with_capacity(analysis.uses.len());
    let mut rbuf = Vec::new();
    for u in &analysis.uses {
        let mut record = 0;
        if u.origin.section == u.section {
            for s in obj.sections() {
                if s.sh_type() != SHT_RELA {
                    continue;
                }
                let Ok((recs, _, target)) = rec_table(view, file, obj, s.index(), &mut rbuf) else {
                    continue;
                };
                if target != u.section {
                    continue;
                }
                let n = u32::try_from(recs.len()).unwrap_or(u32::MAX);
                // The last record at the offset (the analysis's reading).
                if let Some(i) = (0..n)
                    .rev()
                    .find(|&i| recs.raw(i).is_some_and(|r| r.r_offset == u.origin.offset))
                {
                    record = i;
                }
                break;
            }
        }
        let key = mortar_arch::cx::scan_key(file, u.section.0, record);
        out.push((u.section.0, key, u.diagnostic(&fref)));
    }
    Ok(out)
}

/// **The hook of D-117's analysis** (WP-119, `crate::regflow`; joined by
/// WP-123): the keyed errors of the bound uses in code section `section`
/// of `file`, whose relocation records the walk is reading. Called once
/// per code section (on the walk of its first block), only in a `Shared`
/// AArch64 link and only for a file whose [`FileFacts`] exist (D-117 item
/// 1's pre-filter is then the analysis's own). The analysis ran once for
/// the file when its facts were made; a bound use is refused,
/// `non-pic-in-shared-object`, at the instruction, with the note naming
/// the function, the offset and the relocation that formed the address.
/// When the analysis could not run, its diagnostic is the result for
/// every code section.
pub(crate) fn analysis(
    _view: &View<'_, '_>,
    file: FileId,
    facts: &FileFacts,
    section: u32,
) -> Vec<(Key, Diagnostic)> {
    match &facts.analysed {
        Ok(uses) => uses
            .iter()
            .filter(|(s, _, _)| *s == section)
            .map(|(_, k, d)| (*k, d.clone()))
            .collect(),
        Err(d) => vec![(mortar_arch::cx::scan_key(file, section, 0), d.clone())],
    }
}

/// The notes of a bound use's `non-pic-in-shared-object` diagnostic
/// (D-117: the definition, the remedy; the boundary's `-fdata-sections`;
/// before the start of an object, "do not form an address before an
/// object").
pub(crate) fn notes(d: Diagnostic, b: Bound, def: &str, named: &str) -> Diagnostic {
    let d = d.note(format!(
        "the relocation names {named}, inside the exported preemptible definition `{def}' \
         of this object: the code binds it without the GOT (spec/dyn.md D-117)"
    ));
    if b.boundary {
        return d.note(
            "the address is also one past the end of a static object of the section: if the \
             code means that end, compile with -fdata-sections (each object in its own section)",
        );
    }
    if b.self_got {
        return d
            .note(
                "this object reaches its exported data through the GOT (it was compiled \
                 -fPIC): if the code forms an address before a static object placed after \
                 the definition, compile with -fdata-sections (each object in its own section)",
            )
            .note("do not form an address before an object");
    }
    d
}

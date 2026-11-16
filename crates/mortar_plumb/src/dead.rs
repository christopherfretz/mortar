//! Holes and dead ranges of an incremental link (`spec/plumb.md` §6.8;
//! `spec/incr.md` IN-3, IN-3a, IN-60).
//!
//! An incremental update leaves bytes a full link of the current inputs
//! would not write: holes (allocated room, holding their section's fill)
//! and dead ranges (what an edit killed). mortar's map lists them (`hole`
//! and `dead` lines, §3.2). For a map that claims IN-60's coverage
//! ([`Map::claims_coverage`]) this module checks, from the output and the
//! map alone:
//!
//! - **P-I1** coverage: every byte of an allocated output section is
//!   covered exactly once by a `place`, a `piece`, a synthetic chunk, a
//!   `hole` or a `dead` line, alignment padding aside;
//! - **P-I2** that each line names a range its section admits;
//! - **P-I3** and **P-I4** IN-3a check 1: nothing live names the range
//!   (relocations, symbols, the GOT, dynamic relocations, `e_entry`,
//!   `.eh_frame_hdr`; DWARF form offsets, [`crate::deaddwarf`]);
//! - **P-I5** IN-3a check 3: an allocated range holds its fill;
//! - **P-I6** IN-3a check 2: a range is well formed for its section's
//!   grammar ([`crate::deaddwarf`] for the debug sections).
//!
//! A map without such lines pays nothing: [`check`] is not called.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use mortar_elf::consts::{
    EM_386, EM_AARCH64, EM_X86_64, ET_DYN, SHF_ALLOC, SHF_EXECINSTR, SHF_MERGE, SHF_STRINGS,
    SHF_TLS, SHN_ABS, SHN_COMMON, SHN_UNDEF, SHN_XINDEX, SHT_DYNSYM, SHT_FINI_ARRAY,
    SHT_INIT_ARRAY, SHT_NOBITS, SHT_NOTE, SHT_PREINIT_ARRAY, SHT_REL, SHT_RELA, SHT_X86_64_UNWIND,
    STT_FILE, STT_SECTION, STT_TLS,
};
use mortar_elf::{Class, SymbolIndex, SymbolSection};

use crate::image::Image;
use crate::locate::{Obj, Place};
use crate::map::{DeadKind, DeadRange, Map};
use crate::reloc::Engine;
use crate::report::{Finding, Rule};

/// A `hole` or `dead` line resolved against the output.
#[derive(Debug, Clone)]
pub(crate) struct Range {
    /// The output section's index.
    pub sec: usize,
    /// `[start, end)` in the section's address space: `sh_addr` plus the
    /// line's offset (an offset, for a non-alloc section in an
    /// executable).
    pub start: u64,
    pub end: u64,
    pub kind: Option<DeadKind>,
    pub line: u32,
}

impl Range {
    /// How a finding names the range.
    pub fn describe(&self, image: &Image<'_>) -> String {
        let s = &image.sections[self.sec];
        let off = self.start - s.shdr.sh_addr;
        let what = match self.kind {
            None => "hole".to_string(),
            Some(k) => format!("dead range ({})", k.name()),
        };
        format!(
            "{what} {}+{off:#x} [{:#x}, +{:#x}) (map line {})",
            s.name,
            self.start,
            self.end - self.start,
            self.line
        )
    }
}

/// The resolved lines, by section and start.
#[derive(Debug, Default)]
pub(crate) struct DeadSet {
    pub ranges: Vec<Range>,
}

impl DeadSet {
    /// The range of section `sec` holding address `a`.
    pub fn in_section(&self, sec: usize, a: u64) -> Option<&Range> {
        let i = self.ranges.partition_point(|r| (r.sec, r.start) <= (sec, a));
        let r = self.ranges.get(i.checked_sub(1)?)?;
        (r.sec == sec && a < r.end).then_some(r)
    }

    /// The range of an allocated section, other than a thread-local
    /// `NOBITS` one (whose addresses are TLS offsets laid over the
    /// sections after it), that holds address `a`.
    pub fn in_memory(&self, image: &Image<'_>, a: u64) -> Option<&Range> {
        self.ranges.iter().find(|r| {
            r.start <= a && a < r.end && {
                let s = &image.sections[r.sec].shdr;
                s.sh_flags & SHF_ALLOC != 0
                    && !(s.sh_flags & SHF_TLS != 0 && s.sh_type == SHT_NOBITS)
            }
        })
    }
}

/// What covers a byte of an allocated output section (IN-60).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ItemKind {
    /// An input section placed whole.
    Place,
    /// A piece of an input section (merged, `.eh_frame`).
    Piece { merge: bool, eh: bool, strings: bool },
    /// Linker-made content.
    Chunk,
    /// A common symbol or a copy relocation's room.
    Common,
    /// A `hole` or `dead` line (its index in [`DeadSet::ranges`]).
    Dead(usize),
}

#[derive(Debug, Clone, Copy)]
struct Item {
    start: u64,
    end: u64,
    /// The alignment the item demands of its address (1 when unknown).
    align: u64,
    /// The largest alignment that may pad before it: `align`, or for a
    /// merged piece the largest of its block's.
    group: u64,
    kind: ItemKind,
}

/// Per-section counts of what the coverage walk found (the measurement
/// of `cargo xtask plumb --coverage`, §6.8).
#[derive(Debug, Clone, Default)]
pub struct SectionCover {
    pub name: String,
    pub size: u64,
    pub nobits: bool,
    /// Bytes covered by one line, by more than one (allowed overlaps of
    /// merge or `.eh_frame` pieces), and by none.
    pub once: u64,
    pub shared: u64,
    pub overlap: u64,
    /// Gaps that are alignment padding before the next item: bytes, and
    /// how many of those bytes do not hold the section's fill.
    pub padding: u64,
    pub padding_not_fill: u64,
    /// `.eh_frame`'s terminator, the last word.
    pub terminator: u64,
    /// Gaps at the end of the section, and other gaps.
    pub tail: u64,
    pub other: u64,
    /// The section has no line at all.
    pub unlisted: bool,
    /// Synthetic chunks taken as containers of the pieces inside them.
    pub containers: u32,
}

/// The coverage of every allocated section (IN-60), with findings for a
/// map that claims it.
#[derive(Debug, Clone, Default)]
pub struct Cover {
    pub sections: Vec<SectionCover>,
}

impl Cover {
    /// One line per section, for `--coverage`.
    pub fn render(&self) -> String {
        let mut s = String::new();
        for c in &self.sections {
            let _ = writeln!(
                s,
                "{} size={:#x}{} once={} shared={} overlap={} padding={} padding-not-fill={} terminator={} tail={} other={}{}{}",
                c.name,
                c.size,
                if c.nobits { " nobits" } else { "" },
                c.once,
                c.shared,
                c.overlap,
                c.padding,
                c.padding_not_fill,
                c.terminator,
                c.tail,
                c.other,
                if c.unlisted { " unlisted" } else { "" },
                if c.containers > 0 {
                    format!(" containers={}", c.containers)
                } else {
                    String::new()
                },
            );
        }
        s
    }
}

/// Whether an output section is packed (layout.md L-142; incr.md IN-30):
/// read as a dense array or a sequence, so it has no hole. `.eh_frame`
/// (holes are padding CIEs, IN-34a) and the debug sections (dead ranges
/// are records of their grammar) are judged apart.
pub(crate) fn packed(name: &str, sh_type: u32) -> bool {
    let prefixed = |p: &str| name == p || name.starts_with(&format!("{p}."));
    matches!(sh_type, SHT_INIT_ARRAY | SHT_FINI_ARRAY | SHT_PREINIT_ARRAY | SHT_NOTE)
        || [".init_array", ".fini_array", ".preinit_array", ".ctors", ".dtors"]
            .iter()
            .any(|p| prefixed(p))
        || matches!(name, ".init" | ".fini" | ".tm_clone_table" | ".eh_frame_hdr")
        || name.starts_with(".note")
        || c_identifier(name)
}

/// A section reachable through `__start_`/`__stop_` symbols has a name
/// that is a C identifier (gABI practice, layout.md L-142).
fn c_identifier(name: &str) -> bool {
    let mut b = name.bytes();
    b.next().is_some_and(|c| c == b'_' || c.is_ascii_alphabetic())
        && b.all(|c| c == b'_' || c.is_ascii_alphanumeric())
}

pub(crate) fn is_eh_frame(image: &Image<'_>, sec: usize) -> bool {
    let s = &image.sections[sec];
    s.name == ".eh_frame"
        || (image.header.e_machine == EM_X86_64 && s.shdr.sh_type == SHT_X86_64_UNWIND)
}

fn finding(n: u8, msg: String) -> Finding {
    Finding::new(Rule::Incr(n), msg)
}

/// Resolves the lines: P-I2 for a line that names no output section,
/// lies outside it, is empty, or overlaps another line.
pub(crate) fn resolve(image: &Image<'_>, lines: &[DeadRange], out: &mut Vec<Finding>) -> DeadSet {
    let mut set = DeadSet::default();
    for d in lines {
        let found: Vec<usize> = image
            .sections
            .iter()
            .enumerate()
            .filter(|(i, s)| *i > 0 && s.name == d.section)
            .map(|(i, _)| i)
            .collect();
        let &[sec] = found.as_slice() else {
            out.push(finding(
                2,
                format!(
                    "map line {}: '{}' names {} output sections, not one",
                    d.line,
                    d.section,
                    found.len()
                ),
            ));
            continue;
        };
        let s = &image.sections[sec].shdr;
        if d.size == 0 || d.offset.checked_add(d.size).is_none_or(|e| e > s.sh_size) {
            out.push(finding(
                2,
                format!(
                    "map line {}: [{:#x}, +{:#x}) is empty or outside '{}' (size {:#x})",
                    d.line, d.offset, d.size, d.section, s.sh_size
                ),
            ));
            continue;
        }
        let start = s.sh_addr.wrapping_add(d.offset);
        set.ranges.push(Range { sec, start, end: start + d.size, kind: d.kind, line: d.line });
    }
    set.ranges.sort_by_key(|r| (r.sec, r.start, r.end, r.line));
    let mut keep: Vec<Range> = Vec::with_capacity(set.ranges.len());
    for r in std::mem::take(&mut set.ranges) {
        if let Some(prev) = keep.last()
            && prev.sec == r.sec
            && r.start < prev.end
        {
            let d = r.describe(image);
            out.push(finding(2, format!("{d} overlaps {}", prev.describe(image))).at(r.start));
            continue;
        }
        keep.push(r);
    }
    set.ranges = keep;
    set
}

/// The allocated output section an item of `[a, a + size)` is in:
/// containing it, a thread-local `NOBITS` one only for a `.tbss` input,
/// one of the chunk's name first.
fn owner(image: &Image<'_>, a: u64, size: u64, tbss: bool, name: Option<&str>) -> Option<usize> {
    let end = a.checked_add(size)?;
    let mut best = None;
    for (i, s) in image.sections.iter().enumerate() {
        let sh = &s.shdr;
        if sh.sh_flags & SHF_ALLOC == 0 || sh.sh_size == 0 {
            continue;
        }
        if a < sh.sh_addr || end > sh.sh_addr.saturating_add(sh.sh_size) {
            continue;
        }
        let tls_nobits = sh.sh_flags & SHF_TLS != 0 && sh.sh_type == SHT_NOBITS;
        if tls_nobits != tbss {
            continue;
        }
        if name.is_some_and(|n| n == s.name) {
            return Some(i);
        }
        best.get_or_insert(i);
    }
    best
}

/// The fill a byte of section `sec` holds (IN-3, layout.md L-82): the
/// trap fill in code (`UDF #0` words on AArch64, `int3` on x86), zero
/// elsewhere.
fn fill_byte(image: &Image<'_>, sec: usize) -> u8 {
    let s = &image.sections[sec].shdr;
    if s.sh_flags & SHF_EXECINSTR != 0 && image.header.e_machine != EM_AARCH64 { 0xcc } else { 0 }
}

/// The items of each allocated section.
#[allow(clippy::too_many_lines)] // one block per kind of line
fn items(image: &Image<'_>, objs: &[Obj<'_>], map: &Map, set: &DeadSet) -> Vec<Vec<Item>> {
    let mut per: Vec<Vec<Item>> = vec![Vec::new(); image.sections.len()];
    for o in objs {
        let machine = o.elf.machine();
        for s in &o.secs {
            if !s.alloc() || s.consumed() {
                continue;
            }
            let align = s.shdr.sh_addralign.max(1);
            match &s.place {
                // An empty section is an item too: its alignment can be
                // what pads before it.
                Place::Whole(a) => {
                    if let Some(i) = owner(image, *a, s.shdr.sh_size, s.tbss(), None) {
                        per[i].push(Item {
                            start: *a,
                            end: *a + s.shdr.sh_size,
                            align,
                            group: 0,
                            kind: ItemKind::Place,
                        });
                    }
                }
                Place::Pieces(ps) => {
                    let kind = ItemKind::Piece {
                        merge: s.merge(),
                        eh: s.is_eh_frame(machine),
                        strings: s.shdr.sh_flags & SHF_STRINGS != 0,
                    };
                    for p in ps {
                        if p.size == 0 {
                            continue;
                        }
                        if let Some(i) = owner(image, p.addr, p.size, s.tbss(), None) {
                            // Every piece keeps its section's alignment
                            // (measured: mortar aligns each string of an
                            // aligned string section).
                            per[i].push(Item {
                                start: p.addr,
                                end: p.addr + p.size,
                                align,
                                group: 0,
                                kind,
                            });
                        }
                    }
                }
                _ => {}
            }
        }
    }
    // Common symbols (no map line names them, §6.8): the output symbol
    // of a name no placed section of an input defines, with the largest
    // alignment its common definitions ask.
    let listed = map.chunks.iter().any(|c| c.kind == crate::map::ChunkKind::Common);
    for (name, align) in if listed { Vec::new() } else { commons(objs) } {
        let Some(sym) = image.global(&name) else { continue };
        let (a, size) = (sym.sym.st_value, sym.sym.st_size.max(1));
        if let Some(i) = owner(image, a, size, false, None) {
            per[i].push(Item { start: a, end: a + size, align, group: 0, kind: ItemKind::Common });
        }
    }
    // Copy relocations' room (mortar's `copy` records, §3.2): the
    // alignment the shared object's symbol asks is not in the map; the
    // address's own, up to 64, stands for it.
    for c in map.dynamic.iter().flat_map(|d| &d.copies) {
        if c.size == 0 {
            continue;
        }
        if let Some(i) = owner(image, c.addr, c.size, false, None) {
            let align = 1u64 << c.addr.trailing_zeros().min(6);
            per[i].push(Item {
                start: c.addr,
                end: c.addr + c.size,
                align,
                group: 0,
                kind: ItemKind::Common,
            });
        }
    }
    for c in &map.chunks {
        let Some(a) = c.addr else { continue };
        if c.size == 0 {
            continue;
        }
        if let Some(i) = owner(image, a, c.size, false, Some(&c.name)) {
            per[i].push(Item {
                start: a,
                end: a + c.size,
                align: 1,
                group: 0,
                kind: ItemKind::Chunk,
            });
        }
    }
    for (k, r) in set.ranges.iter().enumerate() {
        if image.sections[r.sec].alloc() {
            per[r.sec].push(Item {
                start: r.start,
                end: r.end,
                align: 1,
                group: 0,
                kind: ItemKind::Dead(k),
            });
        }
    }
    // The pieces of merged sections are laid out as one block per output
    // section, aligned to the largest alignment among them (measured: a
    // `.rodata.str1.1` piece after a 16-aligned one starts 16-aligned).
    for v in &mut per {
        let group = v
            .iter()
            .filter(|it| matches!(it.kind, ItemKind::Piece { merge: true, .. }))
            .map(|it| it.align)
            .max()
            .unwrap_or(1);
        for it in v.iter_mut() {
            if matches!(it.kind, ItemKind::Piece { merge: true, .. }) {
                it.group = group;
            }
        }
        v.sort_by_key(|it| (it.start, it.end));
    }
    per
}

/// The common symbols of the inputs no placed section defines, with
/// the largest alignment asked (`st_value` of a common symbol, gABI).
fn commons(objs: &[Obj<'_>]) -> Vec<(String, u64)> {
    use mortar_elf::consts::{SHN_X86_64_LCOMMON, STB_GLOBAL, STB_WEAK};
    let mut common: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    let mut defined: BTreeSet<String> = BTreeSet::new();
    for o in objs {
        let Some(t) = &o.symtab else { continue };
        for (i, sym) in t.iter() {
            if !matches!(sym.binding(), STB_GLOBAL | STB_WEAK) {
                continue;
            }
            let Ok(name) = t.name(&sym) else { continue };
            let name = String::from_utf8_lossy(name).into_owned();
            if sym.st_shndx == SHN_COMMON || sym.st_shndx == SHN_X86_64_LCOMMON {
                let a = common.entry(name).or_insert(1);
                *a = (*a).max(sym.st_value.max(1));
            } else if let Ok(SymbolSection::Section(k)) = t.section_of(i, &sym)
                && o.secs
                    .get(k.0 as usize)
                    .is_some_and(|s| matches!(s.place, Place::Whole(_) | Place::Pieces(_)))
            {
                defined.insert(name);
            }
        }
    }
    common.into_iter().filter(|(n, _)| !defined.contains(n)).collect()
}

/// IN-60's coverage over every allocated output section. Findings
/// (P-I1) only when `claim`.
#[allow(clippy::too_many_lines)] // one walk, its gap and overlap cases in order
pub(crate) fn coverage(
    image: &Image<'_>,
    objs: &[Obj<'_>],
    map: &Map,
    set: &DeadSet,
    claim: bool,
    out: &mut Vec<Finding>,
) -> Cover {
    let per = items(image, objs, map, set);
    let mut cover = Cover::default();
    for (si, s) in image.sections.iter().enumerate() {
        let sh = &s.shdr;
        if si == 0 || sh.sh_flags & SHF_ALLOC == 0 || sh.sh_size == 0 {
            continue;
        }
        let nobits = sh.sh_type == SHT_NOBITS;
        let mut c =
            SectionCover { name: s.name.clone(), size: sh.sh_size, nobits, ..Default::default() };
        let mut its = per[si].clone();
        let eh = is_eh_frame(image, si);
        // A synthetic chunk with pieces or lines inside it is their
        // container (mortar lists `.eh_frame` as one chunk and its input
        // records as pieces): the parts are the coverage.
        let containers: Vec<usize> = its
            .iter()
            .enumerate()
            .filter(|(_, c)| c.kind == ItemKind::Chunk)
            .filter(|(_, ch)| {
                its.iter().any(|p| {
                    matches!(p.kind, ItemKind::Piece { .. } | ItemKind::Dead(_))
                        && ch.start <= p.start
                        && p.end <= ch.end
                })
            })
            .map(|(i, _)| i)
            .collect();
        c.containers = containers.len() as u32;
        for &i in containers.iter().rev() {
            its.remove(i);
        }
        if its.is_empty() {
            c.unlisted = true;
            if claim && !nobits {
                out.push(
                    finding(
                        1,
                        format!(
                            "no line of the map covers output section '{}' [{:#x}, +{:#x})",
                            s.name, sh.sh_addr, sh.sh_size
                        ),
                    )
                    .at(sh.sh_addr),
                );
            }
            c.tail = sh.sh_size;
            cover.sections.push(c);
            continue;
        }
        let lo = sh.sh_addr;
        let hi = sh.sh_addr + sh.sh_size;
        let fill = fill_byte(image, si);
        // The walk: `cur` is the end of what is covered so far; `last` the
        // item that reaches it.
        let mut cur = lo;
        let mut last: Option<Item> = None;
        let mut said_other = 0u32;
        for it in &its {
            let (st, en) = (it.start.max(lo), it.end.min(hi));
            if st > cur {
                let gap = st - cur;
                // After common symbols and copies, whose layout the map
                // does not record, the section's own alignment may pad
                // (measured: mortar aligns what follows a copy block).
                let after_common = last.is_some_and(|l| l.kind == ItemKind::Common);
                let top = if after_common { sh.sh_addralign.max(it.group) } else { it.group };
                let aligned = it.align.is_power_of_two()
                    && std::iter::successors(Some(it.align), |p| p.checked_mul(2))
                        .take_while(|&p| p <= top.max(it.align))
                        .any(|p| cur.next_multiple_of(p) == st);
                if aligned {
                    c.padding += gap;
                    let bytes = if nobits { None } else { image.bytes(cur, gap) };
                    let bad = bytes.map_or(0, |b| b.iter().filter(|&&x| x != fill).count() as u64);
                    c.padding_not_fill += bad;
                    if claim && bad > 0 {
                        out.push(
                            finding(
                                1,
                                format!(
                                    "alignment padding [{cur:#x}, +{gap:#x}) of '{}' holds bytes other than the section's fill",
                                    s.name
                                ),
                            )
                            .at(cur),
                        );
                    }
                } else {
                    c.other += gap;
                    if claim && said_other < 16 {
                        said_other += 1;
                        out.push(
                            finding(
                                1,
                                format!(
                                    "[{cur:#x}, +{gap:#x}) of '{}' is covered by no line of the map",
                                    s.name
                                ),
                            )
                            .at(cur),
                        );
                    }
                }
                cur = st;
            }
            if st < cur && en > st {
                // Overlap with what came before.
                let shared_ok = matches!(
                    (last.map(|l| l.kind), it.kind),
                    (
                        Some(ItemKind::Piece { merge: true, .. }),
                        ItemKind::Piece { merge: true, .. }
                    ) | (Some(ItemKind::Piece { eh: true, .. }), ItemKind::Piece { eh: true, .. })
                );
                let n = en.min(cur) - st;
                if shared_ok {
                    c.shared += n;
                } else {
                    c.overlap += n;
                    if claim {
                        let what = |k: Option<ItemKind>| match k {
                            Some(ItemKind::Dead(j)) => set.ranges[j].describe(image),
                            Some(ItemKind::Place) => "a placed input section".to_string(),
                            Some(ItemKind::Piece { .. }) => {
                                "a piece of an input section".to_string()
                            }
                            Some(ItemKind::Chunk) => "a chunk".to_string(),
                            Some(ItemKind::Common) => "a common symbol or copy".to_string(),
                            None => String::new(),
                        };
                        out.push(
                            finding(
                                1,
                                format!(
                                    "[{st:#x}, +{n:#x}) of '{}' is covered twice: by {} and by {}",
                                    s.name,
                                    what(last.map(|l| l.kind)),
                                    what(Some(it.kind))
                                ),
                            )
                            .at(st),
                        );
                    }
                }
            }
            if en > cur {
                c.once += en - cur.max(st);
                cur = en;
                last = Some(*it);
            }
        }
        if cur < hi {
            let gap = hi - cur;
            // `.eh_frame`'s terminator is the linker's, the last word
            // (synth.md S-48; incr.md IN-34a: always last).
            let term = eh && gap == 4 && image.bytes(cur, 4).is_some_and(|b| b == [0, 0, 0, 0]);
            if term {
                c.terminator += 4;
            } else {
                c.tail += gap;
                if claim {
                    out.push(
                        finding(
                            1,
                            format!(
                                "the end of '{}' [{cur:#x}, +{gap:#x}) is covered by no line of the map",
                                s.name
                            ),
                        )
                        .at(cur),
                    );
                }
            }
        }
        cover.sections.push(c);
    }
    cover
}

/// The checks of a map that claims coverage (§6.8).
pub(crate) fn check(e: &mut Engine<'_, '_>, map: &Map) {
    let image = e.image;
    let mut out = Vec::new();
    let set = resolve(image, &map.dead, &mut out);
    let _ = coverage(image, e.objs, map, &set, true, &mut out);
    let ends = live_ends(image, e.objs, map, false);
    admit(image, e.objs, map, &set, &mut out);
    fill(image, &set, &mut out);
    let labels = (live_ends(image, e.objs, map, true), end_labels(e.objs));
    named_by_symbols(image, &set, &labels, &mut out);
    named_by_relocations(e, &set, &mut out);
    named_by_tables(image, &set, &ends, &mut out);
    crate::deaddwarf::check(image, e.objs, map, &set, &mut out);
    e.findings.append(&mut out);
}

/// The ends of every live item of the allocated sections, by address: an
/// address that is a live input section's end belongs to that section
/// (IN-3a check 1, the re-review's N3).
pub(crate) fn live_ends(
    image: &Image<'_>,
    objs: &[Obj<'_>],
    map: &Map,
    empty: bool,
) -> BTreeSet<u64> {
    let set = DeadSet::default();
    let per = items(image, objs, map, &set);
    per.iter()
        .flatten()
        .filter(|i| !matches!(i.kind, ItemKind::Dead(_)) && (empty || i.end > i.start))
        .map(|i| i.end)
        .collect()
}

/// The names of the inputs' symbols defined at the end of a placed
/// section (`st_value` = `sh_size`): labels such as `x_end` that the N3
/// rule gives to the section they end.
fn end_labels(objs: &[Obj<'_>]) -> BTreeSet<String> {
    let mut v = BTreeSet::new();
    for o in objs {
        let Some(t) = &o.symtab else { continue };
        for (i, sym) in t.iter() {
            let Ok(SymbolSection::Section(k)) = t.section_of(i, &sym) else { continue };
            let Some(s) = o.secs.get(k.0 as usize) else { continue };
            if matches!(s.place, Place::Whole(_) | Place::Pieces(_))
                && sym.st_value == s.shdr.sh_size
                && let Ok(n) = t.name(&sym)
            {
                v.insert(String::from_utf8_lossy(n).into_owned());
            }
        }
    }
    v
}

/// The sections with a neutral form (incr.md IN-51, IN-51b).
const NEUTRAL: &[&str] = &[
    ".debug_info",
    ".debug_abbrev",
    ".debug_line",
    ".debug_rnglists",
    ".debug_loclists",
    ".debug_str_offsets",
    ".debug_addr",
    ".debug_ranges",
    ".debug_frame",
];

/// The sections where a whole dead contribution may keep its bytes
/// (incr.md IN-53).
const AS_WRITTEN: &[&str] = &[
    ".debug_line",
    ".debug_str_offsets",
    ".debug_addr",
    ".debug_rnglists",
    ".debug_loclists",
    ".debug_ranges",
    ".debug_loc",
    ".debug_macro",
    ".debug_frame",
];

/// P-I2: a range where its section admits none, or of a kind its section
/// does not have.
fn admit(image: &Image<'_>, objs: &[Obj<'_>], map: &Map, set: &DeadSet, out: &mut Vec<Finding>) {
    for r in &set.ranges {
        let s = &image.sections[r.sec];
        let sh = &s.shdr;
        let alloc = sh.sh_flags & SHF_ALLOC != 0;
        let eh = is_eh_frame(image, r.sec);
        let why: Option<String> = match r.kind {
            _ if s.name == ".debug_aranges" => Some(
                "'.debug_aranges' has no dead range: it is rewritten whole (incr.md IN-54)".into(),
            ),
            None if !alloc => Some("a hole is in an allocated section only (IN-3)".into()),
            None if !eh && packed(&s.name, sh.sh_type) => Some(format!(
                "'{}' is a packed section (layout.md L-142, incr.md IN-30): it has no hole",
                s.name
            )),
            None if !eh && sh.sh_flags & SHF_TLS != 0 && r.end != sh.sh_addr + sh.sh_size => {
                Some("a thread-local section has holes only at its end (layout.md L-143)".into())
            }
            Some(DeadKind::Cie) if !eh => Some("a 'cie' range is in .eh_frame only".into()),
            Some(DeadKind::Strings) => strings_section(image, objs, map, r.sec)
                .err()
                .map(|m| format!("a 'strings' range {m}")),
            Some(DeadKind::Neutral) if alloc || !NEUTRAL.contains(&s.name.as_str()) => {
                Some(format!(
                    "a 'neutral' range is in a section with a neutral form only (incr.md IN-51, IN-51b: {})",
                    NEUTRAL.join(", ")
                ))
            }
            Some(DeadKind::Contribution) if alloc || !AS_WRITTEN.contains(&s.name.as_str()) => {
                Some(format!(
                    "a 'contribution' range is in a section where a dead contribution may be left as written only (incr.md IN-53: {})",
                    AS_WRITTEN.join(", ")
                ))
            }
            None | Some(_) => None,
        };
        if let Some(why) = why {
            out.push(finding(2, format!("{}: {why}", r.describe(image))).at(r.start));
        }
    }
}

/// Whether section `sec` holds merged strings (a `strings` range may be
/// there): a non-executable section with a live piece of an
/// `SHF_MERGE|SHF_STRINGS` input, or, non-alloc, the output section of a
/// `merged` record of one.
fn strings_section(
    image: &Image<'_>,
    objs: &[Obj<'_>],
    map: &Map,
    sec: usize,
) -> Result<(), String> {
    let s = &image.sections[sec];
    if s.shdr.sh_flags & SHF_EXECINSTR != 0 {
        return Err("is in an executable section".into());
    }
    let alloc = s.shdr.sh_flags & SHF_ALLOC != 0;
    let lo = s.shdr.sh_addr;
    let hi = lo + s.shdr.sh_size;
    let merged_strings =
        |sh: &mortar_elf::Shdr| sh.sh_flags & (SHF_MERGE | SHF_STRINGS) == SHF_MERGE | SHF_STRINGS;
    let found = if alloc {
        objs.iter().flat_map(|o| &o.secs).any(|x| {
            x.alloc()
                && merged_strings(&x.shdr)
                && match &x.place {
                    Place::Pieces(ps) => ps.iter().any(|p| lo <= p.addr && p.addr < hi),
                    Place::Whole(a) => lo <= *a && *a < hi,
                    _ => false,
                }
        })
    } else {
        s.name == ".debug_str"
            || s.name == ".debug_line_str"
            || map.nonalloc.iter().any(|n| n.offset.is_none() && n.out == s.name)
    };
    if found { Ok(()) } else { Err("is in a section with no merged strings".into()) }
}

/// P-I5 (allocated ranges hold their fill, IN-3a check 3) and the
/// allocated part of P-I6 (`strings` and `cie` ranges).
fn fill(image: &Image<'_>, set: &DeadSet, out: &mut Vec<Finding>) {
    for r in &set.ranges {
        let s = &image.sections[r.sec];
        let sh = &s.shdr;
        if sh.sh_flags & SHF_ALLOC == 0 {
            continue;
        }
        let len = r.end - r.start;
        let eh = is_eh_frame(image, r.sec);
        let msg: Option<(u8, String)> = match r.kind {
            None if sh.sh_type == SHT_NOBITS => None,
            None if eh => padding_cie(image, r).err().map(|m| (5, m)),
            None if matches!(sh.sh_type, SHT_RELA | SHT_REL) => {
                rel_fill(image, r).err().map(|m| (5, m))
            }
            None => match image.bytes(r.start, len) {
                None => Some((5, "its bytes are not in the output's file image".into())),
                Some(b) => {
                    let f = fill_byte(image, r.sec);
                    b.iter().position(|&x| x != f).map(|i| {
                        (
                            5,
                            format!(
                                "byte {:#x} at {:#x} is not the fill {f:#04x} ({})",
                                b[i],
                                r.start + i as u64,
                                if sh.sh_flags & SHF_EXECINSTR != 0 {
                                    "code holds the trap fill, layout.md L-82"
                                } else {
                                    "data holds zero"
                                }
                            ),
                        )
                    })
                }
            },
            Some(DeadKind::Strings) => match image.bytes(r.start, len) {
                Some(b) if b.last() == Some(&0) => None,
                Some(_) => Some((6, "the range does not end with a string's NUL".into())),
                None if sh.sh_type == SHT_NOBITS => None,
                None => Some((6, "its bytes are not in the output's file image".into())),
            },
            Some(DeadKind::Cie) if eh => unused_cies(image, r).err().map(|m| (6, m)),
            Some(_) => None,
        };
        if let Some((n, m)) = msg {
            out.push(finding(n, format!("{}: {m}", r.describe(image))).at(r.start));
        }
    }
}

/// The return-address register of IN-34a's padding CIE.
fn ra_register(image: &Image<'_>) -> u8 {
    match image.header.e_machine {
        EM_AARCH64 => 30,
        EM_386 => 8,
        _ => 16,
    }
}

/// IN-34a's padding CIE, exactly: `length` = hole − 4, `CIE_id` 0,
/// version 1, augmentation "", code alignment 1, data alignment −4, the
/// machine's return-address register, `DW_CFA_nop` to the end; at least
/// 16 bytes and a multiple of 4.
fn padding_cie(image: &Image<'_>, r: &Range) -> Result<(), String> {
    let len = r.end - r.start;
    if len < 16 || !len.is_multiple_of(4) {
        return Err(format!(
            "an .eh_frame hole is one padding CIE of at least 16 bytes, a multiple of 4 (incr.md IN-34a), not {len}"
        ));
    }
    let b = image.bytes(r.start, len).ok_or("its bytes are not in the output's file image")?;
    let want_len = (len - 4) as u32;
    let mut head = Vec::with_capacity(13);
    head.extend_from_slice(&want_len.to_le_bytes());
    head.extend_from_slice(&[0, 0, 0, 0, 1, 0, 1, 0x7c, ra_register(image)]);
    if b[..13] != head[..] {
        return Err(format!(
            "an .eh_frame hole is one padding CIE (incr.md IN-34a): its first 13 bytes are {:02x?}, not {head:02x?}",
            &b[..13]
        ));
    }
    if let Some(i) = b[13..].iter().position(|&x| x != 0) {
        return Err(format!(
            "the padding CIE's instructions are DW_CFA_nop: byte {:#x} at {:#x}",
            b[13 + i],
            r.start + 13 + i as u64
        ));
    }
    Ok(())
}

/// A `cie` range: one or more whole CIEs, nothing else.
fn unused_cies(image: &Image<'_>, r: &Range) -> Result<(), String> {
    let len = r.end - r.start;
    let b = image.bytes(r.start, len).ok_or("its bytes are not in the output's file image")?;
    let mut p = 0usize;
    while p < b.len() {
        let l = b.get(p..p + 4).map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]));
        let Some(l) = l.filter(|&l| l != 0 && l != 0xffff_ffff) else {
            return Err(format!("no CIE starts at {:#x}", r.start + p as u64));
        };
        let id = b.get(p + 4..p + 8).map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]));
        if id != Some(0) || b.get(p + 8).is_none_or(|&v| v != 1 && v != 3) {
            return Err(format!("the record at {:#x} is not a CIE", r.start + p as u64));
        }
        p += 4 + l as usize;
    }
    if p != b.len() {
        return Err("the last CIE runs past the range".into());
    }
    Ok(())
}

/// A hole of an allocated relocation section (IN-3, dyn.md §14): whole
/// records, `RELATIVE` ones aimed at one scratch word inside
/// `DT_RELACOUNT`'s prefix, `R_*_NONE` after it.
fn rel_fill(image: &Image<'_>, r: &Range) -> Result<(), String> {
    let s = &image.sections[r.sec].shdr;
    let rela = s.sh_type == SHT_RELA;
    let w: u64 = if image.class() == Class::Elf64 { 8 } else { 4 };
    let ent = if rela { 3 * w } else { 2 * w };
    if !(r.start - s.sh_addr).is_multiple_of(ent) || !(r.end - r.start).is_multiple_of(ent) {
        return Err(format!("a hole of a relocation section is whole records of {ent} bytes"));
    }
    let relative = relative_type(image.header.e_machine);
    let prefix = dyn_value(image, if rela { 0x6fff_fff9 } else { 0x6fff_fffa }).unwrap_or(0);
    // The prefix is the table `DT_RELA`/`DT_REL` points at (dyn.md §14).
    let owner = dyn_value(image, if rela { 7 } else { 17 }) == Some(s.sh_addr);
    let mut scratch: Option<u64> = None;
    let mut a = r.start;
    while a < r.end {
        let off = image.value(a, w as u8).ok_or("its bytes are not in the output's file image")?;
        let info =
            image.value(a + w, w as u8).ok_or("its bytes are not in the output's file image")?;
        let ty = if w == 8 { info & 0xffff_ffff } else { info & 0xff };
        let sym = if w == 8 { info >> 32 } else { info >> 8 };
        let index = (a - s.sh_addr) / ent;
        let in_prefix = owner && index < prefix;
        if in_prefix || ty != 0 {
            if relative != Some(ty as u32) || sym != 0 {
                return Err(format!(
                    "record {index} is not RELATIVE{}: type {ty}, symbol {sym}",
                    if in_prefix { " inside DT_RELACOUNT's prefix" } else { " or NONE" }
                ));
            }
            if *scratch.get_or_insert(off) != off {
                return Err(format!(
                    "the RELATIVE records of the hole aim at more than one word ({:#x} and {off:#x}), not one scratch word",
                    scratch.unwrap_or(0)
                ));
            }
        } else if sym != 0 {
            return Err(format!("record {index} is NONE with symbol {sym}"));
        }
        a += ent;
    }
    Ok(())
}

/// `R_*_RELATIVE` of the machine.
fn relative_type(machine: u16) -> Option<u32> {
    match machine {
        EM_AARCH64 => Some(1027),
        EM_X86_64 | EM_386 => Some(8),
        _ => None,
    }
}

/// `R_*_IRELATIVE` of the machine.
fn irelative_type(machine: u16) -> Option<u32> {
    match machine {
        EM_AARCH64 => Some(1032),
        EM_X86_64 => Some(37),
        EM_386 => Some(42),
        _ => None,
    }
}

/// The value of the first `.dynamic` entry with tag `tag`.
fn dyn_value(image: &Image<'_>, tag: u64) -> Option<u64> {
    let i = image.sections.iter().position(|s| s.name == ".dynamic")?;
    let d = image.section_data(i)?;
    let w = if image.class() == Class::Elf64 { 8 } else { 4 };
    for e in d.chunks_exact(2 * w) {
        let rd = |b: &[u8]| b.iter().rev().fold(0u64, |a, &x| a << 8 | u64::from(x));
        let t = rd(&e[..w]);
        if t == 0 {
            break;
        }
        if t == tag {
            return Some(rd(&e[w..]));
        }
    }
    None
}

/// Whether address `v` lies in an allocated dead range and is not the end
/// of a live item (N3): the range it names.
fn names_dead<'s>(
    image: &Image<'_>,
    set: &'s DeadSet,
    ends: &BTreeSet<u64>,
    v: u64,
) -> Option<&'s Range> {
    let r = set.in_memory(image, v)?;
    (!ends.contains(&v)).then_some(r)
}

/// IN-3a check 1 for symbols (P-I3): no defined symbol of `.symtab` or
/// `.dynsym` is in a dead range, unless an input defines a symbol of its
/// name at the end of its section and the value is the end of a live
/// item (a label at its section's end, N3).
fn named_by_symbols(
    image: &Image<'_>,
    set: &DeadSet,
    (ends, labels): &(BTreeSet<u64>, BTreeSet<String>),
    out: &mut Vec<Finding>,
) {
    let tls =
        image.phdrs.iter().find(|p| p.p_type == mortar_elf::consts::PT_TLS).map(|p| p.p_vaddr);
    let mut judge = |table: &str, name: &str, shndx: u16, info: u8, value: u64| {
        let ty = info & 0xf;
        if matches!(shndx, SHN_UNDEF | SHN_ABS | SHN_COMMON | SHN_XINDEX)
            || shndx >= 0xff00
            || matches!(ty, STT_SECTION | STT_FILE)
        {
            return;
        }
        let sec = usize::from(shndx);
        let Some(s) = image.sections.get(sec) else { return };
        let a = if ty == STT_TLS && image.header.e_type != mortar_elf::consts::ET_REL {
            match tls {
                Some(t) => t.wrapping_add(value),
                None => return,
            }
        } else {
            value
        };
        let Some(r) = set.in_section(sec, a) else { return };
        // A label an input defines at the end of its section, at the end
        // of a live item (N3): that section's.
        if s.shdr.sh_flags & SHF_ALLOC != 0 && ends.contains(&a) && labels.contains(name) {
            return;
        }
        out.push(
            finding(
                3,
                format!("{}: {table} symbol '{name}' ({a:#x}) is defined in it", r.describe(image)),
            )
            .at(a),
        );
    };
    for s in &image.symbols {
        judge(".symtab", &s.name, s.sym.st_shndx, s.sym.st_info, s.sym.st_value);
    }
    for (name, shndx, info, value) in dynsym(image) {
        judge(".dynsym", &name, shndx, info, value);
    }
}

/// `.dynsym`'s symbols: `(name, st_shndx, st_info, st_value)`.
fn dynsym(image: &Image<'_>) -> Vec<(String, u16, u8, u64)> {
    let Some(i) = image.sections.iter().position(|s| s.shdr.sh_type == SHT_DYNSYM) else {
        return Vec::new();
    };
    let Some(d) = image.section_data(i) else { return Vec::new() };
    let strs = image.section_data(image.sections[i].shdr.sh_link as usize).unwrap_or(&[]);
    let name = |o: usize| {
        strs.get(o..)
            .and_then(|t| t.split(|&b| b == 0).next())
            .map(|t| String::from_utf8_lossy(t).into_owned())
            .unwrap_or_default()
    };
    let r32 = |b: &[u8], o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let mut v = Vec::new();
    if image.class() == Class::Elf64 {
        for e in d.chunks_exact(24).skip(1) {
            let mut x = [0u8; 8];
            x.copy_from_slice(&e[8..16]);
            v.push((
                name(r32(e, 0) as usize),
                u16::from_le_bytes([e[6], e[7]]),
                e[4],
                u64::from_le_bytes(x),
            ));
        }
    } else {
        for e in d.chunks_exact(16).skip(1) {
            v.push((
                name(r32(e, 0) as usize),
                u16::from_le_bytes([e[14], e[15]]),
                e[12],
                u64::from(r32(e, 4)),
            ));
        }
    }
    v
}

/// IN-3a check 1 for relocations (P-I3), judged by what a record names
/// (N3): a record of a live section whose symbol's section (its base,
/// whatever the addend) is placed inside a dead range. Coverage (P-I1)
/// already reports the placement; this names the referrers.
fn named_by_relocations(e: &Engine<'_, '_>, set: &DeadSet, out: &mut Vec<Finding>) {
    let image = e.image;
    // The live input sections placed over a dead range.
    let mut bad: BTreeSet<(usize, usize)> = BTreeSet::new();
    for (oi, o) in e.objs.iter().enumerate() {
        for (si, s) in o.secs.iter().enumerate() {
            if !s.alloc() {
                continue;
            }
            let hit = match &s.place {
                Place::Whole(a) => over(image, set, *a, s.shdr.sh_size, s.tbss()),
                Place::Pieces(ps) => ps.iter().any(|p| over(image, set, p.addr, p.size, s.tbss())),
                _ => false,
            };
            if hit {
                bad.insert((oi, si));
            }
        }
    }
    if bad.is_empty() {
        return;
    }
    for (oi, o) in e.objs.iter().enumerate() {
        let Some(symtab) = o.symtab else { continue };
        for (si, s) in o.secs.iter().enumerate() {
            if !matches!(s.place, Place::Whole(_) | Place::Pieces(_)) || bad.contains(&(oi, si)) {
                continue;
            }
            let Some(rs) = s.relocs else { continue };
            let Some(recs) = o.records(rs) else { continue };
            for (off, sym, ty, _) in recs {
                let Ok(st) = symtab.get(SymbolIndex(sym)) else { continue };
                let base = match symtab.section_of(SymbolIndex(sym), &st) {
                    Ok(SymbolSection::Section(k)) => Some((oi, k.0 as usize)),
                    Ok(SymbolSection::Undefined) => {
                        let n =
                            String::from_utf8_lossy(symtab.name(&st).unwrap_or(b"")).into_owned();
                        e.chosen_def(&n).and_then(|(doi, di)| {
                            let dt = e.objs[doi].symtab?;
                            let ds = dt.get(SymbolIndex(di)).ok()?;
                            match dt.section_of(SymbolIndex(di), &ds).ok()? {
                                SymbolSection::Section(k) => Some((doi, k.0 as usize)),
                                _ => None,
                            }
                        })
                    }
                    _ => None,
                };
                if let Some(b) = base
                    && bad.contains(&b)
                {
                    let name =
                        String::from_utf8_lossy(symtab.name(&st).unwrap_or(b"")).into_owned();
                    out.push(
                        finding(
                            3,
                            format!(
                                "a relocation (type {ty}) names '{}' of {}, which is placed in a dead range",
                                if name.is_empty() { e.objs[b.0].secs[b.1].name.clone() } else { name },
                                e.objs[b.0].label()
                            ),
                        )
                        .site(o.site(si, off)),
                    );
                }
            }
        }
    }
}

/// Whether `[a, +len)` of a section's address space meets a dead range.
fn over(image: &Image<'_>, set: &DeadSet, a: u64, len: u64, tbss: bool) -> bool {
    set.ranges.iter().any(|r| {
        let s = &image.sections[r.sec].shdr;
        let tls_nobits = s.sh_flags & SHF_TLS != 0 && s.sh_type == SHT_NOBITS;
        s.sh_flags & SHF_ALLOC != 0
            && tls_nobits == tbss
            && a < r.end
            // An empty item (an input section or piece of size 0) covers
            // no byte: it is in a range only strictly inside it; at the
            // range's start or end it names the boundary (§6.8 P-I3, N3).
            && if len == 0 { r.start < a } else { r.start < a.saturating_add(len) }
    })
}

/// IN-3a check 1 for the synthetic and dynamic tables (P-I3): the GOT's
/// words, the allocated relocation sections (where they write, and the
/// target of a `RELATIVE`/`IRELATIVE` record), `.eh_frame_hdr`'s table,
/// and `e_entry`.
fn named_by_tables(image: &Image<'_>, set: &DeadSet, ends: &BTreeSet<u64>, out: &mut Vec<Finding>) {
    let w: u64 = if image.class() == Class::Elf64 { 8 } else { 4 };
    let machine = image.header.e_machine;
    let entry = image.header.e_entry;
    if entry != 0
        && let Some(r) = names_dead(image, set, ends, entry)
    {
        out.push(
            finding(3, format!("{}: e_entry ({entry:#x}) is in it", r.describe(image))).at(entry),
        );
    }
    for (i, s) in image.sections.iter().enumerate() {
        let sh = &s.shdr;
        if sh.sh_flags & SHF_ALLOC == 0 || sh.sh_type == SHT_NOBITS {
            continue;
        }
        if matches!(s.name.as_str(), ".got" | ".got.plt" | ".igot" | ".igot.plt") {
            let mut a = sh.sh_addr;
            while a + w <= sh.sh_addr + sh.sh_size {
                if set.in_section(i, a).is_none()
                    && let Some(v) = image.value(a, w as u8)
                    && let Some(r) = names_dead(image, set, ends, v)
                {
                    out.push(
                        finding(
                            3,
                            format!(
                                "{}: the {} word at {a:#x} holds {v:#x}",
                                r.describe(image),
                                s.name
                            ),
                        )
                        .at(a),
                    );
                }
                a += w;
            }
        }
        if matches!(sh.sh_type, SHT_RELA | SHT_REL) {
            let rela = sh.sh_type == SHT_RELA;
            let ent = if rela { 3 * w } else { 2 * w };
            let mut a = sh.sh_addr;
            while a + ent <= sh.sh_addr + sh.sh_size {
                if set.in_section(i, a).is_some() {
                    a += ent;
                    continue;
                }
                let (Some(off), Some(info)) =
                    (image.value(a, w as u8), image.value(a + w, w as u8))
                else {
                    break;
                };
                let ty = (if w == 8 { info & 0xffff_ffff } else { info & 0xff }) as u32;
                if ty != 0 {
                    // ET_DYN: r_offset and addends are link-time addresses.
                    if let Some(r) = set.in_memory(image, off) {
                        out.push(
                            finding(
                                3,
                                format!(
                                    "{}: the {} record at {a:#x} (type {ty}) writes at {off:#x}",
                                    r.describe(image),
                                    s.name
                                ),
                            )
                            .at(a),
                        );
                    }
                    if Some(ty) == relative_type(machine) || Some(ty) == irelative_type(machine) {
                        let target = if rela {
                            image.value(a + 2 * w, w as u8)
                        } else {
                            image.value(off, w as u8)
                        };
                        if let Some(t) = target
                            && let Some(r) = names_dead(image, set, ends, t)
                        {
                            out.push(
                                finding(
                                    3,
                                    format!(
                                        "{}: the {} record at {a:#x} (type {ty}) targets {t:#x}",
                                        r.describe(image),
                                        s.name
                                    ),
                                )
                                .at(a),
                            );
                        }
                    }
                }
                a += ent;
            }
        }
        if s.name == ".eh_frame_hdr" {
            eh_frame_hdr(image, i, set, ends, out);
        }
    }
}

/// `.eh_frame_hdr`'s table (LSB "Exception Frames"): no entry's initial
/// location or FDE is in a dead range.
fn eh_frame_hdr(
    image: &Image<'_>,
    i: usize,
    set: &DeadSet,
    ends: &BTreeSet<u64>,
    out: &mut Vec<Finding>,
) {
    let sh = &image.sections[i].shdr;
    let Some(d) = image.section_data(i) else { return };
    // version 1, eh_frame_ptr enc, fde_count enc, table enc: the usual
    // `pcrel|sdata4`, `udata4`, `datarel|sdata4`.
    if d.len() < 12 || d[0] != 1 || d[2] != 0x03 || d[3] != 0x3b {
        return;
    }
    let off = match d[1] {
        0x1b => 8usize,
        0x1c => 12,
        _ => return,
    };
    let Some(cnt) = d.get(off..off + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    else {
        return;
    };
    let base = sh.sh_addr;
    let eh = image.sections.iter().position(|s| s.name == ".eh_frame");
    for k in 0..cnt as usize {
        let at = off + 4 + 8 * k;
        let Some(e) = d.get(at..at + 8) else { break };
        let loc = base.wrapping_add(i64::from(i32::from_le_bytes([e[0], e[1], e[2], e[3]])) as u64);
        let fde = base.wrapping_add(i64::from(i32::from_le_bytes([e[4], e[5], e[6], e[7]])) as u64);
        if let Some(r) = names_dead(image, set, ends, loc) {
            out.push(
                finding(
                    3,
                    format!(
                        "{}: .eh_frame_hdr entry {k} names the location {loc:#x}",
                        r.describe(image)
                    ),
                )
                .at(base + at as u64),
            );
        }
        if let Some(ei) = eh
            && let Some(r) = set.in_section(ei, fde)
        {
            out.push(
                finding(
                    3,
                    format!(
                        "{}: .eh_frame_hdr entry {k} names an FDE at {fde:#x}",
                        r.describe(image)
                    ),
                )
                .at(base + at as u64),
            );
        }
    }
}

/// Whether the output is position-independent (`ET_DYN`): a dead debug
/// entry `[0, len)` names a real range there (incr.md IN-53).
pub(crate) fn position_independent(image: &Image<'_>) -> bool {
    image.header.e_type == ET_DYN
}

/// `cargo xtask plumb --coverage` (§6.8): the coverage table of a link
/// whatever its map claims, and the findings it would have under the
/// claim.
pub fn coverage_table(
    output: &[u8],
    inputs: &[crate::Input<'_>],
    map: &Map,
) -> Result<(Cover, Vec<Finding>), String> {
    let image = Image::read(output)?;
    let located = crate::locate::locate(map, inputs, &image);
    let mut out = Vec::new();
    let set = resolve(&image, &map.dead, &mut out);
    let cover = coverage(&image, &located.objs, map, &set, true, &mut out);
    Ok((cover, out))
}

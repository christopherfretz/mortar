//! `Layout::check` of a relocatable placement (`-r`; `spec/model.md` §4.6
//! MD-R1, `spec/layout.md` L-R10) [M4-r].
//!
//! Owner: RL-3 (layout at `-r`, record `spec/landed/layout/WP-154.md` §F),
//! which fills [`validate`] with L-R10's readings of §4.4's rules (rule 1
//! as written; rule 2 on file offsets only; rule 3 with no segments; rule 4
//! vacuous; rule 5 with no `defined`, no `sym_place` other than `-d`'s
//! commons, and no `entry`) and its added rules (one `SHT_GROUP` chunk per
//! kept group, its members `SHF_GROUP` output sections; one relocation
//! chunk exactly per output section with a relocated piece; no relocation
//! chunk targets a group or a relocation chunk). RL-3 may add private items
//! here and edits no other file of `mortar_core` (WP-181).
//!
//! WP-183 (RL-3). [`validate`] checks what the tables show. Two of the
//! added rules need facts the tables do not hold, and are checked with
//! them by [`validate_groups`] (the kept groups and their members, from
//! the inputs' `SHT_GROUP` sections) and [`validate_relocs`] (the output
//! records per output section, from the inputs' relocation sections).
//!
//! Rule 1 at `-r`: a section needs a place exactly when it is kept, live,
//! assigned to an output section and of a kind placed whole at `-r`:
//! `Regular`, `Nobits`, `Note`, `Debug`, and `Merge` (L-R5: mergeable
//! sections are not merged; `SectionKind::is_placed` is false for `Merge`
//! because a final link places its fragments). An `.eh_frame` input has
//! no place (its records are the synthetic `.eh_frame` chunk's), nor has
//! an input property note (consumed into the synthetic note).

use std::collections::BTreeSet;

use mortar_elf::Class;
use mortar_elf::consts::{SHF_GROUP, SHT_GROUP, SHT_REL, SHT_RELA};

use crate::check::{CheckContext, Rule, Row, Violation, aligned, end, fail, need};
use crate::id::{ChunkId, OutSecId, SectionId};
use crate::layout::{AllocKey, ChunkKind, Layout};
use crate::resolution::{Def, SectionKind};

type Check = Result<(), Violation>;

const R: Rule = Rule::Relocatable;

/// The synthetic chunks a `-r` output may have (MD-R1, S10).
const SYNTHETIC: [&[u8]; 7] = [
    b".note.gnu.property",
    b".note.gnu.build-id",
    b".eh_frame",
    b".symtab",
    b".symtab_shndx",
    b".strtab",
    b".shstrtab",
];

/// Whether a section of `kind` is placed whole at `-r` (L-R4, L-R5).
#[must_use]
pub const fn placed_at_r(kind: SectionKind) -> bool {
    matches!(
        kind,
        SectionKind::Regular
            | SectionKind::Nobits
            | SectionKind::Note
            | SectionKind::Debug
            | SectionKind::Merge
    )
}

/// The size of a relocation record of the output: 24 (ELF64 RELA) or 8
/// (ELF32 REL).
const fn reloc_entry(class: Class) -> u64 {
    match class {
        Class::Elf64 => 24,
        Class::Elf32 => 8,
    }
}

/// Whether a relocatable layout is valid (L-R10): `Ok`, or the first
/// violation, as `Layout::validate` answers.
///
/// # Errors
///
/// The first reading that does not hold, as a [`Rule::Relocatable`]
/// violation naming the reading.
pub fn validate(layout: &Layout, cx: &CheckContext<'_, '_>) -> Result<(), Violation> {
    let v = Validator { layout, cx };
    v.range()?;
    v.places()?;
    v.chunks()?;
    v.empty()?;
    v.bodies()?;
    v.groups()?;
    v.relocs()
}

/// L-R10's added group rule, with the kept groups: `kept` lists each kept
/// group's input `SHT_GROUP` section with its members that have a place,
/// sorted by section. Exactly one `Group(g)` chunk per kept group and none
/// other; the output section of every member has `SHF_GROUP` (its body
/// chunk and its `outsecs` row), and no other output section has it.
///
/// # Errors
///
/// The first difference.
pub fn validate_groups(
    layout: &Layout,
    cx: &CheckContext<'_, '_>,
    kept: &[(SectionId, &[SectionId])],
) -> Result<(), Violation> {
    let p = &layout.placement;
    let mut chunks: Vec<(SectionId, ChunkId)> = p
        .chunks
        .iter()
        .filter_map(|(id, c)| match c.kind {
            ChunkKind::Group(g) => Some((g, id)),
            _ => None,
        })
        .collect();
    chunks.sort_unstable();
    let want = kept.iter().map(|k| k.0);
    let have = chunks.iter().map(|c| c.0);
    if let Some((g, missing)) = crate::check::difference(want, have) {
        return fail(
            R,
            if missing {
                "a kept group has no SHT_GROUP chunk"
            } else {
                "an SHT_GROUP chunk names no kept group, or a group has two"
            },
            [Row::Section(g)],
        );
    }
    let mut members = BTreeSet::new();
    for &(_, list) in kept {
        for &m in list {
            let Some(o) = cx.assignment.out(m) else {
                return fail(R, "a group member has no output section", [Row::Section(m)]);
            };
            members.insert(o);
        }
    }
    for (o, row) in cx.assignment.outsecs.iter() {
        let member = members.contains(&o);
        need(
            (row.flags & SHF_GROUP != 0) == member,
            R,
            || {
                if member {
                    "a group member's output section has no SHF_GROUP".into()
                } else {
                    "an output section outside every group has SHF_GROUP".into()
                }
            },
            [],
        )?;
        for (id, c) in p.chunks.iter() {
            if c.kind == ChunkKind::Body(crate::layout::Part::first(o)) {
                need(
                    (c.flags & SHF_GROUP != 0) == member,
                    R,
                    || "a body chunk's SHF_GROUP differs from its output section's group".into(),
                    [Row::Chunk(id)],
                )?;
            }
        }
    }
    Ok(())
}

/// L-R10's added relocation rule, with the records: `records[o]` is the
/// number of output records of output section `o` (`spec/image.md` I-R5,
/// `NONE` records included). A `Relocs(o)` chunk exists exactly when it is
/// not 0, and holds that many records.
///
/// # Errors
///
/// The first difference.
pub fn validate_relocs(
    layout: &Layout,
    cx: &CheckContext<'_, '_>,
    records: &[u64],
) -> Result<(), Violation> {
    let p = &layout.placement;
    need(
        records.len() == cx.assignment.outsecs.len() as usize,
        R,
        || "the record counts are not per output section".into(),
        [],
    )?;
    let mut chunk_of = vec![None; records.len()];
    for (id, c) in p.chunks.iter() {
        if let ChunkKind::Relocs(o) = c.kind
            && let Some(slot) = chunk_of.get_mut(o.0 as usize)
        {
            *slot = Some(id);
        }
    }
    for (o, &n) in records.iter().enumerate() {
        match chunk_of[o] {
            None => need(
                n == 0,
                R,
                || format!("output section {o} has a relocated piece and no relocation chunk"),
                [],
            )?,
            Some(id) => {
                need(
                    n != 0,
                    R,
                    || "a relocation chunk for an output section with no record".into(),
                    [Row::Chunk(id)],
                )?;
                need(
                    p.chunks[id].size == n * reloc_entry(cx.class),
                    R,
                    || "a relocation chunk's size is not its records'".into(),
                    [Row::Chunk(id)],
                )?;
            }
        }
    }
    Ok(())
}

struct Validator<'a, 't, 'n> {
    layout: &'a Layout,
    cx: &'a CheckContext<'t, 'n>,
}

impl Validator<'_, '_, '_> {
    /// The ids the placement names exist.
    fn range(&self) -> Check {
        let p = &self.layout.placement;
        let cx = self.cx;
        need(
            p.place.len() == cx.resolution.sections.len(),
            R,
            || "place is not per section".into(),
            [],
        )?;
        for (id, c) in p.chunks.iter() {
            let ok = match c.kind {
                ChunkKind::Body(part) | ChunkKind::Patches(part) => cx.assignment.outsecs.has(part.out),
                ChunkKind::Merged(o) | ChunkKind::Relocs(o) => cx.assignment.outsecs.has(o),
                ChunkKind::Group(g) => cx.resolution.sections.has(g),
                _ => true,
            };
            need(ok, R, || "a chunk names a row that does not exist".into(), [Row::Chunk(id)])?;
        }
        for (s, place) in p.place.iter() {
            if let Some((c, _)) = place.get() {
                need(
                    p.chunks.has(c),
                    R,
                    || "a place names a chunk that does not exist".into(),
                    [Row::Section(s)],
                )?;
            }
        }
        for &c in &p.order {
            need(p.chunks.has(c), R, || "order names a chunk that does not exist".into(), [])?;
        }
        Ok(())
    }

    /// Rule 1 as written, with `-r`'s placed kinds (module doc).
    fn places(&self) -> Check {
        let cx = self.cx;
        let p = &self.layout.placement;
        let mut taken: Vec<(ChunkId, u64, u128, SectionId)> = Vec::new();
        for (s, row) in cx.resolution.sections.iter() {
            let out = cx.assignment.out(s);
            let live = cx.liveness.live.get(s).copied().unwrap_or(false);
            let needs = row.is_kept() && live && placed_at_r(row.kind) && out.is_some();
            let Some((chunk, off)) = p.place[s].get() else {
                need(!needs, R, || "rule 1: a live section has no place".into(), [Row::Section(s)])?;
                continue;
            };
            need(
                needs,
                R,
                || "rule 1: a section that needs no place has one".into(),
                [Row::Section(s)],
            )?;
            let c = &p.chunks[chunk];
            need(
                out.is_some_and(|o| c.kind == ChunkKind::Body(crate::layout::Part::first(o))),
                R,
                || "rule 1: the section is not in the body of its output section".into(),
                [Row::Section(s), Row::Chunk(chunk)],
            )?;
            let Some(shape) = cx.shapes.shape(s) else {
                return fail(
                    R,
                    "rule 1: the section's size and alignment cannot be read",
                    [Row::Section(s)],
                );
            };
            need(
                end(off, shape.size) <= c.size.into(),
                R,
                || "rule 1: the section ends past its chunk's size".into(),
                [Row::Section(s), Row::Chunk(chunk)],
            )?;
            need(
                aligned(off, shape.align) && c.align >= shape.align,
                R,
                || "rule 1: the section is misaligned".into(),
                [Row::Section(s), Row::Chunk(chunk)],
            )?;
            taken.push((chunk, off, end(off, shape.size), s));
        }
        // Rule 5's `-d` commons are the only allocations (L-R4).
        for (at, row) in p.sym_place.iter().enumerate() {
            let me = Row::Entry("sym_place", at as u32);
            let common = match row.key {
                AllocKey::Common(k) => matches!(cx.resolution.def.get(k), Some(Def::Common(_))),
                AllocKey::Copy(_) => false,
            };
            need(common, R, || "rule 5: an allocation that is not a common".into(), [me])?;
            let c = p.chunks.get(row.chunk);
            need(
                c.is_some_and(|c| {
                    matches!(c.kind, ChunkKind::Body(_)) && end(row.offset, row.size) <= c.size.into()
                }),
                R,
                || "rule 1: a common's allocation is not inside a body chunk".into(),
                [me, Row::Chunk(row.chunk)],
            )?;
        }
        taken.sort_unstable();
        for pair in taken.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            need(
                !(a.0 == b.0 && u128::from(b.1) < a.2 && b.2 > u128::from(b.1)),
                R,
                || "rule 1: two places overlap".into(),
                [Row::Section(a.3), Row::Section(b.3)],
            )?;
        }
        Ok(())
    }

    /// Rule 2 on file offsets only (L-R7, L-R8).
    fn chunks(&self) -> Check {
        let p = &self.layout.placement;
        let mut seen = vec![false; p.chunks.len() as usize];
        for (at, &c) in p.order.iter().enumerate() {
            need(
                !seen[c.0 as usize],
                R,
                || "rule 2: a chunk twice in order".into(),
                [Row::Chunk(c), Row::Entry("order", at as u32)],
            )?;
            seen[c.0 as usize] = true;
        }
        if let Some(c) = seen.iter().position(|s| !s) {
            return fail(R, "rule 2: a chunk missing from order", [Row::Chunk(ChunkId(c as u32))]);
        }
        let first = p.order.first().map(|&c| (c, &p.chunks[c]));
        need(
            first.is_some_and(|(_, c)| c.kind == ChunkKind::FileHeader && c.offset == 0),
            R,
            || "rule 2: the file header is not first at offset 0".into(),
            first.map(|f| Row::Chunk(f.0)),
        )?;
        let last = p.order.last().map(|&c| (c, &p.chunks[c]));
        need(
            last.is_some_and(|(_, c)| c.kind == ChunkKind::SectionHeaders),
            R,
            || "rule 2: the section header table is not last (L-R8)".into(),
            last.map(|l| Row::Chunk(l.0)),
        )?;
        let mut names = Vec::new();
        for (id, c) in p.chunks.iter() {
            let me = [Row::Chunk(id)];
            need(c.addr.is_none(), R, || "rule 2: a chunk has an address (L-R7)".into(), me)?;
            need(c.size <= c.capacity, R, || "rule 2: size exceeds capacity".into(), me)?;
            need(
                !c.in_file() || aligned(c.offset, c.align),
                R,
                || "rule 2: the chunk's offset is misaligned".into(),
                me,
            )?;
            let header = matches!(c.kind, ChunkKind::FileHeader | ChunkKind::SectionHeaders);
            need(
                header == c.shndx.is_none(),
                R,
                || "rule 2: a header with a section index, or a section without one".into(),
                me,
            )?;
            if let ChunkKind::Synthetic(s) = c.kind {
                need(
                    SYNTHETIC.contains(&s.name) && !s.address_dependent,
                    R,
                    || "MD-R1: a synthetic chunk a -r output does not have".into(),
                    me,
                )?;
                names.push((s.name, id));
            }
        }
        names.sort_unstable();
        for pair in names.windows(2) {
            need(
                pair[0].0 != pair[1].0,
                R,
                || "MD-R1: a synthetic chunk twice".into(),
                [Row::Chunk(pair[0].1), Row::Chunk(pair[1].1)],
            )?;
        }
        for name in [&b".symtab"[..], b".strtab", b".shstrtab"] {
            need(
                names.iter().any(|n| n.0 == name),
                R,
                || format!("MD-R1: no {} chunk", String::from_utf8_lossy(name)),
                [],
            )?;
        }
        let mut file_end: Option<(u128, ChunkId)> = None;
        for &id in &p.order {
            let c = &p.chunks[id];
            if !c.in_file() || c.capacity == 0 {
                continue;
            }
            if let Some((stop, prev)) = file_end {
                need(
                    u128::from(c.offset) >= stop,
                    R,
                    || "rule 2: chunks overlap in the file, or order is not file order".into(),
                    [Row::Chunk(prev), Row::Chunk(id)],
                )?;
            }
            file_end = Some((end(c.offset, c.capacity), id));
        }
        // L-86: section indexes dense from 1 in order, one per chunk here.
        let mut next = 0u32;
        for &id in &p.order {
            let Some(index) = p.chunks[id].shndx else { continue };
            next += 1;
            need(
                index == next,
                R,
                || format!("rule 2: section index {index} is not the next in order"),
                [Row::Chunk(id)],
            )?;
        }
        Ok(())
    }

    /// Rules 3, 4 and 5 (L-R7): no segment, no TLS, no patch, no
    /// `defined`, no `sized`, no entry; no chunk of a loadable output.
    fn empty(&self) -> Check {
        let p = &self.layout.placement;
        need(p.segments.is_empty(), R, || "rule 3: a -r layout has segments".into(), [])?;
        need(p.tls.is_none(), R, || "rule 3: a -r layout has a TLS template".into(), [])?;
        need(
            p.patches.is_empty() && p.redirects.is_empty(),
            R,
            || "rule 4: a -r layout has patches".into(),
            [],
        )?;
        need(p.defined.is_empty(), R, || "rule 5: a -r layout has defined rows".into(), [])?;
        need(p.sized.is_empty(), R, || "rule 5: a -r layout has sized rows".into(), [])?;
        need(p.entry.is_none(), R, || "rule 5: a -r layout has an entry".into(), [])?;
        need(p.holes.is_empty(), R, || "MD-R1: a -r layout has holes".into(), [])?;
        for (id, c) in p.chunks.iter() {
            let ok = match c.kind {
                ChunkKind::ProgramHeaders | ChunkKind::Merged(_) | ChunkKind::Patches(_) => false,
                ChunkKind::Body(part) => part.part == 0,
                _ => true,
            };
            need(ok, R, || "MD-R1: a chunk a -r output does not have".into(), [Row::Chunk(id)])?;
        }
        Ok(())
    }

    /// MD-R1: one `Body(o, 0)` per output section with a placed piece, of
    /// its type and flags; the output section of the `.eh_frame` inputs
    /// has none (the synthetic `.eh_frame` chunk is its content).
    fn bodies(&self) -> Check {
        let cx = self.cx;
        let p = &self.layout.placement;
        let n = cx.assignment.outsecs.len() as usize;
        let mut body: Vec<Option<ChunkId>> = vec![None; n];
        for (id, c) in p.chunks.iter() {
            if let ChunkKind::Body(part) = c.kind {
                let slot = &mut body[part.out.0 as usize];
                need(slot.is_none(), R, || "MD-R1: two bodies of one output section".into(), [
                    Row::Chunk(id),
                ])?;
                *slot = Some(id);
                let row = &cx.assignment.outsecs[part.out];
                need(
                    c.sh_type == row.sh_type && c.flags == row.flags,
                    R,
                    || "MD-R1: a body chunk's type or flags are not its output section's".into(),
                    [Row::Chunk(id)],
                )?;
            }
        }
        let eh = p
            .chunks
            .iter()
            .any(|(_, c)| matches!(c.kind, ChunkKind::Synthetic(s) if s.name == b".eh_frame"));
        let mut content = vec![false; n];
        for (s, row) in cx.resolution.sections.iter() {
            let Some(o) = cx.assignment.out(s) else { continue };
            let live = row.is_kept() && cx.liveness.live.get(s).copied().unwrap_or(false);
            if !live {
                continue;
            }
            content[o.0 as usize] = true;
            if row.kind == SectionKind::EhFrame {
                need(
                    eh && body[o.0 as usize].is_none(),
                    R,
                    || "S1: an .eh_frame input's output section is not the .eh_frame chunk".into(),
                    [Row::Section(s)],
                )?;
            } else {
                need(
                    body[o.0 as usize].is_some(),
                    R,
                    || "MD-R1: an output section with a live piece has no body".into(),
                    [Row::Section(s)],
                )?;
            }
        }
        for (o, b) in body.iter().enumerate() {
            if let Some(id) = b {
                need(
                    content[o],
                    R,
                    || "MD-R1: a body of an output section with no live piece".into(),
                    [Row::Chunk(*id)],
                )?;
            }
        }
        Ok(())
    }

    /// L-R10/L-R3 as far as the tables show: a `Group(g)` chunk names an
    /// `SHT_GROUP` input section, once, and is a list of words.
    fn groups(&self) -> Check {
        let cx = self.cx;
        let p = &self.layout.placement;
        let mut named = BTreeSet::new();
        for (id, c) in p.chunks.iter() {
            let ChunkKind::Group(g) = c.kind else { continue };
            let me = [Row::Chunk(id), Row::Section(g)];
            need(
                cx.resolution.sections[g].kind == SectionKind::Group,
                R,
                || "L-R3: a group chunk names a section that is not a group".into(),
                me,
            )?;
            need(named.insert(g), R, || "L-R10: a group has two group chunks".into(), me)?;
            need(
                c.sh_type == SHT_GROUP && c.size >= 4 && c.size % 4 == 0 && c.align == 2,
                R,
                || "L-R3: a group chunk is not SHT_GROUP words".into(),
                me,
            )?;
        }
        Ok(())
    }

    /// L-R10: one relocation chunk per output section at most, for an
    /// output section that has content and a piece with a relocation
    /// section (never a group or a relocation chunk); right after its
    /// target (L-R8).
    fn relocs(&self) -> Check {
        let cx = self.cx;
        let p = &self.layout.placement;
        let n = cx.assignment.outsecs.len() as usize;
        let mut has_reloc = vec![false; n];
        let mut has_content = vec![false; n];
        for (s, row) in cx.resolution.sections.iter() {
            let Some(o) = cx.assignment.out(s) else { continue };
            if row.is_kept() && cx.liveness.live.get(s).copied().unwrap_or(false) {
                has_content[o.0 as usize] = true;
                has_reloc[o.0 as usize] |= row.reloc.0 != 0;
            }
        }
        let (want_type, entry) = match cx.class {
            Class::Elf64 => (SHT_RELA, reloc_entry(Class::Elf64)),
            Class::Elf32 => (SHT_REL, reloc_entry(Class::Elf32)),
        };
        let mut seen = vec![false; n];
        for (at, &id) in p.order.iter().enumerate() {
            let c = &p.chunks[id];
            let ChunkKind::Relocs(o) = c.kind else { continue };
            let me = [Row::Chunk(id)];
            let k = o.0 as usize;
            need(!seen[k], R, || "L-R10: two relocation chunks of one output section".into(), me)?;
            seen[k] = true;
            need(
                has_content[k],
                R,
                || "L-R10: a relocation chunk targets no output section with content".into(),
                me,
            )?;
            need(
                has_reloc[k],
                R,
                || "L-R10: a relocation chunk for an output section with no relocated piece".into(),
                me,
            )?;
            need(
                c.sh_type == want_type && c.size > 0 && c.size % entry == 0,
                R,
                || "S6: a relocation chunk's type or size".into(),
                me,
            )?;
            let before = at.checked_sub(1).map(|b| &p.chunks[p.order[b]]);
            let target = before.is_some_and(|b| match b.kind {
                ChunkKind::Body(part) => part.out == o,
                ChunkKind::Synthetic(s) => s.name == b".eh_frame" && eh_out(cx) == Some(o),
                _ => false,
            });
            need(
                target,
                R,
                || "L-R8: a relocation chunk is not right after its target".into(),
                me,
            )?;
        }
        Ok(())
    }
}

/// The output section of the live `.eh_frame` inputs.
fn eh_out(cx: &CheckContext<'_, '_>) -> Option<OutSecId> {
    cx.resolution
        .sections
        .iter()
        .find(|(s, row)| row.kind == SectionKind::EhFrame && cx.assignment.out(*s).is_some())
        .and_then(|(s, _)| cx.assignment.out(s))
}

//! Stage B (`spec/layout.md` §9): the placed input sections of each
//! output section, in order.
//!
//! The default order is `SectionId` order (a stable partition by output
//! section). Three exceptions sort, each by a key that ends in
//! `SectionId`: `.text` categories (L-53), `.init_array`/`.fini_array`
//! priorities (L-54), and `SHF_LINK_ORDER` inputs (L-57).
//! `--sort-section` (L-53a) adds a key inside each of those groups.

use mortar_core::assignment::Assignment;
use mortar_core::column::Column;
use mortar_core::id::{OutSecId, SectionId};
use mortar_core::layout::Folds;
use mortar_core::liveness::Liveness;
use mortar_core::par::Pool;
use mortar_core::plan::SortSection;
use mortar_core::resolution::Resolution;
use mortar_elf::consts::SHF_LINK_ORDER;

use crate::assign::matches_prefix;
use crate::diag::{LayoutDiag, feature};
use crate::headers::Headers;
use crate::rank::Rank;

/// The members of every output section, in stage B order: `members[o]`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ordered {
    pub members: Column<OutSecId, Vec<SectionId>>,
}

/// Whether a section is placed in its output section's body: kept, live,
/// of a placed kind, assigned, and not folded into another (model §4.4
/// rule 1; a folded section has its target's place).
#[must_use]
pub fn is_member(
    s: SectionId,
    assignment: &Assignment<'_>,
    resolution: &Resolution,
    liveness: &Liveness,
    folds: &Folds,
) -> bool {
    let row = &resolution.sections[s];
    row.is_kept()
        && row.kind.is_placed()
        && liveness.live.get(s).copied().unwrap_or(false)
        && assignment.out(s).is_some()
        && !folds.is_folded(s)
}

/// The `.text` category of an input name (L-53): 0 unlikely, 1 exit, 2
/// startup, 3 hot, 4 sorted, 5 the rest.
#[must_use]
pub fn text_category(name: &[u8]) -> u8 {
    const CATS: [&[u8]; 4] = [b".text.unlikely", b".text.exit", b".text.startup", b".text.hot"];
    if let Some(i) = CATS.iter().position(|p| matches_prefix(name, p)) {
        return i as u8;
    }
    if name.starts_with(b".text.sorted.") { 4 } else { 5 }
}

/// The priority of an `.init_array`/`.fini_array` input (L-54): `Some(n)`
/// for `.init_array.NNNNN`, `None` for the unnumbered.
///
/// # Errors
///
/// `refused` (`feature`, `init_array priority`) for a suffix that is not a
/// decimal number below 65536.
pub fn priority(name: &[u8], out: &[u8]) -> Result<Option<u16>, LayoutDiag> {
    let Some(rest) = name.strip_prefix(out) else { return Ok(None) };
    let Some(digits) = rest.strip_prefix(b".") else { return Ok(None) };
    let bad = || feature("init_array priority");
    if digits.is_empty() || digits.len() > 5 || !digits.iter().all(u8::is_ascii_digit) {
        return Err(bad());
    }
    let n = digits.iter().fold(0u32, |n, d| n * 10 + u32::from(d - b'0'));
    u16::try_from(n).map(Some).map_err(|_| bad())
}

/// One output section's sort: its name, members and result.
type Work<'a> = (&'a [u8], Vec<SectionId>, Result<(), LayoutDiag>);

/// Stage B.
///
/// Parallel: the gather of members per file (ordered collection), and the
/// per-output-section sorts; the partition itself is one sequential pass
/// (count, prefix sum, scatter) over the gathered pairs.
///
/// # Errors
///
/// L-54's refusal, per output section in `OutSecId` order.
#[allow(clippy::too_many_arguments)]
pub fn order(
    assignment: &Assignment<'_>,
    headers: &Headers<'_>,
    resolution: &Resolution,
    liveness: &Liveness,
    folds: &Folds,
    ranks: &Column<OutSecId, Rank>,
    sort_section: Option<SortSection>,
    pool: &Pool,
) -> Result<Ordered, Vec<LayoutDiag>> {
    let files = resolution.section_base.len() as usize;
    let gathered =
        pool.unit_cost(150).map_collect(files, |f, out: &mut Vec<(OutSecId, SectionId)>| {
            let range = resolution.section_base[mortar_core::id::FileId(f as u32)];
            for i in 0..range.len() {
                let s = SectionId(range.start.0 + i);
                if is_member(s, assignment, resolution, liveness, folds)
                    && let Some(o) = assignment.out(s)
                {
                    out.push((o, s));
                }
            }
        });
    let outs = assignment.outsecs.len() as usize;
    let mut counts = vec![0usize; outs];
    for &(o, _) in &gathered.items {
        counts[o.0 as usize] += 1;
    }
    let mut members: Vec<Vec<SectionId>> = counts.iter().map(|&n| Vec::with_capacity(n)).collect();
    for &(o, s) in &gathered.items {
        members[o.0 as usize].push(s);
    }
    // Per output section sorts (L-53, L-54), parallel over output sections.
    let mut work: Vec<Work<'_>> = members
        .into_iter()
        .enumerate()
        .map(|(o, m)| (assignment.outsecs[OutSecId(o as u32)].name, m, Ok(())))
        .collect();
    pool.unit_cost(60000)
        .for_each_mut(&mut work, |_, (name, m, r)| *r = sort_named(name, m, headers, sort_section));
    let mut members = Vec::with_capacity(work.len());
    let mut results = Vec::with_capacity(work.len());
    for (_, m, r) in work {
        members.push(m);
        results.push(r);
    }
    let errors: Vec<LayoutDiag> = results.into_iter().filter_map(Result::err).collect();
    if !errors.is_empty() {
        return Err(errors);
    }
    link_order(&mut members, headers, assignment, ranks);
    Ok(Ordered { members: Column::from_vec(members).unwrap_or_default() })
}

/// The output sections GNU ld's default scripts name, whose statements
/// `--sort-section` sorts (L-53a): every one but `.init` and `.fini`
/// (`SORT_NONE`). Any other output section is an orphan there and is not
/// sorted. `.lrodata`, `.ldata` and `.lbss` are in the x86-64 scripts
/// only; they are listed for every target.
const SCRIPT_SECTIONS: [&[u8]; 28] = [
    b".text",
    b".rodata",
    b".rodata1",
    b".data",
    b".data1",
    b".data.rel.ro",
    b".bss",
    b".tdata",
    b".tbss",
    b".preinit_array",
    b".init_array",
    b".fini_array",
    b".ctors",
    b".dtors",
    b".jcr",
    b".eh_frame",
    b".gcc_except_table",
    b".gnu_extab",
    b".exception_ranges",
    b".sframe",
    b".interp",
    b".comment",
    b".note.gnu.build-id",
    b".note.ABI-tag",
    b".note.package",
    b".lrodata",
    b".ldata",
    b".lbss",
];

/// Whether `--sort-section` sorts the inputs of output section `name`
/// (L-53a).
#[must_use]
pub fn sorted_by_sort_section(name: &[u8]) -> bool {
    SCRIPT_SECTIONS.contains(&name)
}

/// L-53a's key between two inputs of one group: name (bytewise) or
/// alignment (descending). `Equal` without `--sort-section`.
fn section_key(
    a: SectionId,
    b: SectionId,
    headers: &Headers<'_>,
    by: Option<SortSection>,
) -> std::cmp::Ordering {
    match by {
        None => std::cmp::Ordering::Equal,
        Some(SortSection::Name) => headers[a].name.cmp(headers[b].name),
        Some(SortSection::Alignment) => headers[b].align.cmp(&headers[a].align),
    }
}

/// L-53, L-54 and L-53a for one output section.
fn sort_named(
    name: &[u8],
    m: &mut [SectionId],
    headers: &Headers<'_>,
    sort_section: Option<SortSection>,
) -> Result<(), LayoutDiag> {
    let by = sort_section.filter(|_| sorted_by_sort_section(name));
    if name == b".text" {
        m.sort_by(|&a, &b| {
            let (na, nb) = (headers[a].name, headers[b].name);
            let (ca, cb) = (text_category(na), text_category(nb));
            ca.cmp(&cb)
                .then_with(
                    || if ca == 4 && cb == 4 { na.cmp(nb) } else { std::cmp::Ordering::Equal },
                )
                .then_with(|| section_key(a, b, headers, by))
                .then(a.cmp(&b))
        });
    } else if name == b".init_array" || name == b".fini_array" {
        let mut keyed = Vec::with_capacity(m.len());
        for &s in m.iter() {
            let p = priority(headers[s].name, name)?;
            // Numbered before unnumbered; ties in SectionId order.
            keyed.push((p.map_or((1, 0), |n| (0, n)), s));
        }
        // The numbered have their own sort (SORT_BY_INIT_PRIORITY), which
        // `--sort-section` does not replace; the unnumbered take its key.
        keyed.sort_by(|&(ka, a), &(kb, b)| {
            ka.cmp(&kb)
                .then_with(|| {
                    if ka.0 == 1 {
                        section_key(a, b, headers, by)
                    } else {
                        std::cmp::Ordering::Equal
                    }
                })
                .then(a.cmp(&b))
        });
        for (slot, (_, s)) in m.iter_mut().zip(keyed) {
            *slot = s;
        }
    } else if name == b".bss" || name == b".lbss" {
        // L-44a (WP-203): an input named `COMMON` is placed with the
        // commons (L-88), after every other input (GNU ld's
        // `*(.bss ...) *(COMMON)`); L-53a sorts each of the two groups.
        let common = |s: SectionId| headers[s].name == b"COMMON";
        if by.is_some() || m.iter().any(|&s| common(s)) {
            m.sort_by(|&a, &b| {
                common(a)
                    .cmp(&common(b))
                    .then_with(|| section_key(a, b, headers, by))
                    .then(a.cmp(&b))
            });
        }
    } else if by.is_some() {
        m.sort_by(|&a, &b| section_key(a, b, headers, by).then(a.cmp(&b)));
    }
    Ok(())
}

/// L-57: the `SHF_LINK_ORDER` members of each output section are sorted
/// among themselves, in the slots they occupy, by (rank of the linked
/// section's output section, its `OutSecId`, the linked section's position
/// in stage B order), then `SectionId`. Positions are those after L-53 and
/// L-54, before this sort.
fn link_order(
    members: &mut [Vec<SectionId>],
    headers: &Headers<'_>,
    assignment: &Assignment<'_>,
    ranks: &Column<OutSecId, Rank>,
) {
    let any = members.iter().flatten().any(|&s| headers[s].flags & SHF_LINK_ORDER != 0);
    if !any {
        return;
    }
    let mut position = vec![u32::MAX; headers.len() as usize];
    for m in members.iter() {
        for (i, &s) in m.iter().enumerate() {
            position[s.0 as usize] = i as u32;
        }
    }
    for m in members.iter_mut() {
        let slots: Vec<usize> =
            (0..m.len()).filter(|&i| headers[m[i]].flags & SHF_LINK_ORDER != 0).collect();
        if slots.is_empty() {
            continue;
        }
        let mut keyed: Vec<((Rank, u32, u32), SectionId)> = slots
            .iter()
            .map(|&i| {
                let s = m[i];
                let key = headers[s].link.and_then(|l| assignment.out(l)).map_or(
                    (Rank { rank: u8::MAX, within: u8::MAX }, u32::MAX, u32::MAX),
                    |o| {
                        let l = headers[s].link.unwrap_or(s);
                        (ranks[o], o.0, position[l.0 as usize])
                    },
                );
                (key, s)
            })
            .collect();
        keyed.sort_unstable();
        for (&i, (_, s)) in slots.iter().zip(keyed) {
            m[i] = s;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headers::Header;

    // spec: layout.md §9 L-53 (.text categories)
    #[test]
    fn l53_categories() {
        assert_eq!(text_category(b".text.unlikely"), 0);
        assert_eq!(text_category(b".text.unlikely.f"), 0);
        assert_eq!(text_category(b".text.exit.f"), 1);
        assert_eq!(text_category(b".text.startup.main"), 2);
        assert_eq!(text_category(b".text.hot"), 3);
        assert_eq!(text_category(b".text.sorted.0001"), 4);
        assert_eq!(text_category(b".text"), 5);
        assert_eq!(text_category(b".text.f"), 5);
        assert_eq!(text_category(b".text.hotter"), 5);
    }

    // spec: layout.md §9 L-53a (the output sections --sort-section sorts)
    #[test]
    fn l53a_script_sections() {
        for n in [&b".text"[..], b".rodata", b".data", b".bss", b".tdata", b".tbss", b".init_array"]
        {
            assert!(sorted_by_sort_section(n), "{n:?}");
        }
        for n in [&b".init"[..], b".fini", b".text.f", b"my_section", b".got", b".debug_info"] {
            assert!(!sorted_by_sort_section(n), "{n:?}");
        }
    }

    // spec: layout.md L-44a (WP-203): an input `COMMON` after every other
    // input of `.bss`/`.lbss`, with the commons (L-88); L-53a inside each
    // group; other output sections and `COMMON.x` untouched.
    #[test]
    fn l44a_common_after_the_bss_inputs() {
        let h = |name: &'static [u8], align: u8| Header { name, align, ..Header::EMPTY };
        let headers: Headers<'static> = Column::from_vec(vec![
            Header::EMPTY,
            h(b"COMMON", 3),     // 1 (file 1)
            h(b".bss", 0),       // 2
            h(b".bss.counter", 2), // 3
            h(b"COMMON.x", 4),   // 4
            h(b".bss.a", 4),     // 5 (file 2)
            h(b"COMMON", 2),     // 6
            h(b".bss", 3),       // 7
        ])
        .unwrap();
        let ids = |v: &[u32]| v.iter().map(|&i| SectionId(i)).collect::<Vec<_>>();
        let sorted = |out: &[u8], v: &[u32], by| {
            let mut m = ids(v);
            sort_named(out, &mut m, &headers, by).unwrap();
            m.iter().map(|s| s.0).collect::<Vec<_>>()
        };
        let all = [1, 2, 3, 5, 6, 7];
        assert_eq!(sorted(b".bss", &all, None), [2, 3, 5, 7, 1, 6]);
        assert_eq!(sorted(b".lbss", &all, None), [2, 3, 5, 7, 1, 6]);
        assert_eq!(sorted(b".bss", &[2, 3, 5, 7], None), [2, 3, 5, 7]);
        assert_eq!(
            sorted(b".bss", &all, Some(SortSection::Name)),
            [2, 7, 5, 3, 1, 6] // .bss .bss .bss.a .bss.counter | COMMON COMMON
        );
        assert_eq!(
            sorted(b".bss", &all, Some(SortSection::Alignment)),
            [5, 7, 3, 2, 1, 6] // 4 3 2 0 | 3 2
        );
        // Not `.bss`: input order (an orphan `COMMON` output section, or
        // `COMMON.x` is not `COMMON`).
        assert_eq!(sorted(b"COMMON", &[1, 6], None), [1, 6]);
        assert_eq!(sorted(b"my_bss", &[1, 2, 4], None), [1, 2, 4]);
        assert_eq!(sorted(b".bss", &[1, 2, 4], None), [2, 4, 1]);
    }

    // spec: layout.md §9.1 L-54 (init priorities: decimal 0..65535, else refused)
    #[test]
    fn l54_priority() {
        let p = |n: &[u8]| priority(n, b".init_array");
        assert_eq!(p(b".init_array"), Ok(None));
        assert_eq!(p(b".init_array.00100"), Ok(Some(100)));
        assert_eq!(p(b".init_array.65535"), Ok(Some(65535)));
        for bad in
            [&b".init_array.65536"[..], b".init_array.x", b".init_array.", b".init_array.123456"]
        {
            let e = p(bad).unwrap_err();
            assert_eq!(e.refusal().map(|r| r.1), Some("init_array priority"), "{bad:?}");
        }
    }
}

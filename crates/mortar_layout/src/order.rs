//! Stage B (`spec/layout.md` §9): the placed input sections of each
//! output section, in order.
//!
//! The default order is `SectionId` order (a stable partition by output
//! section). Three exceptions sort, each by a key that ends in
//! `SectionId`: `.text` categories (L-53), `.init_array`/`.fini_array`
//! priorities (L-54), and `SHF_LINK_ORDER` inputs (L-57).

use mortar_core::assignment::Assignment;
use mortar_core::column::Column;
use mortar_core::id::{OutSecId, SectionId};
use mortar_core::layout::Folds;
use mortar_core::liveness::Liveness;
use mortar_core::par::Pool;
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
    pool: &Pool,
) -> Result<Ordered, Vec<LayoutDiag>> {
    let files = resolution.section_base.len() as usize;
    let gathered = pool.map_collect(files, |f, out: &mut Vec<(OutSecId, SectionId)>| {
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
    pool.for_each_mut(&mut work, |_, (name, m, r)| *r = sort_named(name, m, headers));
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

/// L-53 and L-54 for one output section.
fn sort_named(name: &[u8], m: &mut [SectionId], headers: &Headers<'_>) -> Result<(), LayoutDiag> {
    if name == b".text" {
        m.sort_by(|&a, &b| {
            let key = |s: SectionId| {
                let n = headers[s].name;
                let c = text_category(n);
                (c, if c == 4 { n } else { &b""[..] }, s)
            };
            key(a).cmp(&key(b))
        });
    } else if name == b".init_array" || name == b".fini_array" {
        let mut keyed = Vec::with_capacity(m.len());
        for &s in m.iter() {
            let p = priority(headers[s].name, name)?;
            // Numbered before unnumbered; ties in SectionId order.
            keyed.push((p.map_or((1, 0), |n| (0, n)), s));
        }
        keyed.sort_unstable();
        for (slot, (_, s)) in m.iter_mut().zip(keyed) {
            *slot = s;
        }
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

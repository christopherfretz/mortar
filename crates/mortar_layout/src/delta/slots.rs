//! IN-72 item 7 (slots, S-113; IN-28b): the update's `SLOTS` rows from the
//! stored ones (whole, with `refs`), each file's old `KEY_REFS.demand` and
//! its new version's slot demand (`LayoutInput::demands`). WP-242.
//!
//! `SLOTS.refs` counts FILES (WP-239 S4-4) and is exact after the update
//! (IN-78): a file's old demand subtracts one from each slot its
//! `KEY_REFS.demand` needs, its new demand adds one. Which flags need
//! which kind is [`SlotKind::demand`], the full link's slot planning's own
//! table (`mortar::front::slots_of`, `Layout::check`'s §4.3). A local's
//! slot (IN-28b) is its file's alone: `KEY_REFS` names globals only, so a
//! file's old local demand is its live local slots; a CHANGED file's
//! locals have no identity across versions, so its old local slots all die
//! and its new local demand is appended. A file of `changes` that
//! `input.demands` does not name demands nothing in its new version
//! (`LayoutInput`'s contract, as for pieces).
//!
//! The plan ([`plan`]) is the exact rows. What the seam can carry of it
//! ([`item_7`]): a slot retired (`refs` to 0) is `LayoutDelta::slots_dead`
//! (written 0, S-113); an appended slot is past its kind's region
//! capacity, which no stored row gives (S-63: "a region's capacity is its
//! count"; R-M4's stored capacity is not built): `reserve-exhausted`; a
//! live slot whose `refs` changes, or a dead one that revives, has no field
//! in `LayoutDelta` (WP-242 finding B): `update-not-implemented (layout)`.

use std::collections::{BTreeMap, BTreeSet};

use mortar_core::demands::DemandFlags;
use mortar_core::id::{FileId, SymKey, SymKind};
use mortar_core::incr::{FileDemands, IncrReason, StoredLayout, StoredSlot};
use mortar_core::layout::SlotKind;

use super::{corrupt, not_implemented};

/// One file of the update: its id, and whether it has a new version
/// (changed: its locals have no identity across versions, IN-28b).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct FileOf {
    pub file: FileId,
    pub changed: bool,
}

/// The exact change to `SLOTS` (IN-78).
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Plan {
    /// Stored rows whose `refs` changes, with the new row, by `(kind,
    /// index)`: retired (`refs` 0), revived (from 0), or recounted.
    pub changed: Vec<(StoredSlot, StoredSlot)>,
    /// New rows, per kind (in `SlotKind::ALL` order) in `SymKey` order, at
    /// the next indices.
    pub appended: Vec<StoredSlot>,
}

/// What the delta carries of the plan: `(slots_dead, slots_appended)`.
///
/// # Errors
/// [`plan`]'s; `reserve-exhausted` for an appended slot; `update-not-implemented
/// (layout)` for a refs change the seam has no field for (finding B).
pub(super) fn item_7(
    stored: &StoredLayout<'_>,
    files: &[FileOf],
    demands: &[FileDemands],
) -> Result<(Vec<StoredSlot>, Vec<StoredSlot>), IncrReason> {
    let p = plan(stored, files, demands)?;
    if let Some(s) = p.appended.first() {
        return Err(IncrReason::ReserveExhausted(format!(
            "{} slots: a new slot past its region's capacity (its count, S-63)",
            kind_name(s.kind)
        )));
    }
    if p.changed.iter().any(|(_, now)| now.refs > 0) {
        return Err(refs_unreported());
    }
    if p.changed.iter().any(|(s, _)| matches!(s.kind, SlotKind::Plt | SlotKind::Iplt)) {
        return Err(rela_plt_refused());
    }
    let dead = p.changed.into_iter().map(|(_, now)| now).collect();
    Ok((dead, Vec::new()))
}

/// A live slot whose `refs` changes, or a dead one that revives: no field
/// of `LayoutDelta` carries it (WP-242 finding B).
fn refs_unreported() -> IncrReason {
    not_implemented()
}

/// A PLT or IPLT slot retired: S-113 removes its relocation from
/// `.rela.plt` (rewritten whole, no holes), so `.rela.plt`'s size and
/// `__rela_iplt_end` (a `DEFINED` row the delta passes through unchanged)
/// move with the count; the delta gives neither (WP-242 finding F).
fn rela_plt_refused() -> IncrReason {
    not_implemented()
}

const fn kind_name(k: SlotKind) -> &'static str {
    match k {
        SlotKind::Got => "GOT",
        SlotKind::Plt => "PLT",
        SlotKind::Iplt => "IPLT",
        SlotKind::TlsDesc => "TLS descriptor",
        SlotKind::TlsGd => "TLS GD",
        SlotKind::TlsIe => "TLS IE",
    }
}

/// The kinds `flags` needs.
fn kinds(flags: DemandFlags) -> impl Iterator<Item = SlotKind> {
    SlotKind::ALL.into_iter().filter(move |k| flags.intersects(k.demand()))
}

/// The exact change to `SLOTS` for the files of the update (sorted).
/// `SLOTS` is read (whole) only when a file of the update has an old or a
/// new demand, or a new version (its locals' slots).
///
/// # Errors
/// `state-corrupt`: `SLOTS` with two live rows of one `(kind, key)` or
/// indices that are not every position of a kind once, a count below 0, a
/// `KEY_REFS` range naming another file; `update-not-implemented (layout)`:
/// a demand on another file's local, demand lists not sorted by key (the
/// join's contract).
pub(super) fn plan(
    stored: &StoredLayout<'_>,
    files: &[FileOf],
    demands: &[FileDemands],
) -> Result<Plan, IncrReason> {
    // The old global demand of each file.
    let mut olds: Vec<BTreeMap<SymKey, DemandFlags>> = Vec::with_capacity(files.len());
    for f in files {
        let mut old: BTreeMap<SymKey, DemandFlags> = BTreeMap::new();
        for r in stored.key_refs_of(f.file)? {
            if r.file != f.file {
                return Err(corrupt(format!("KEY_REFS range of file {}", f.file.0)));
            }
            let e = old.entry(SymKey::global(r.key)).or_insert(DemandFlags::empty());
            *e |= r.demand;
        }
        old.retain(|_, d| SlotKind::ALL.iter().any(|k| d.intersects(k.demand())));
        olds.push(old);
    }
    let any = files.iter().any(|f| f.changed)
        || olds.iter().any(|o| !o.is_empty())
        || demands.iter().any(|d| !d.demands.is_empty());
    if !any {
        return Ok(Plan::default());
    }
    let rows = Rows::read(&stored.slots()?)?;
    let live = &rows.live;

    // Net change per (kind, key); `fresh`: a changed file's new local
    // demand, always appended (IN-28b).
    let mut net: BTreeMap<(SlotKind, SymKey), i64> = BTreeMap::new();
    let mut fresh: BTreeSet<(SlotKind, SymKey)> = BTreeSet::new();
    let given: BTreeMap<FileId, &FileDemands> = demands.iter().map(|d| (d.file, d)).collect();
    if given.len() != demands.len() {
        return Err(not_implemented());
    }
    for d in demands {
        if files.binary_search_by_key(&d.file, |f| f.file).is_err() && !d.demands.is_empty() {
            debug_assert!(false, "demand of file {} not in changes", d.file.0);
            return Err(not_implemented());
        }
    }
    for (f, old) in files.iter().zip(&olds) {
        let file = f.file;
        // Old: the globals' `KEY_REFS.demand`, the locals' live slots.
        let mut old_local: BTreeSet<(SlotKind, SymKey)> = BTreeSet::new();
        for &(kind, key) in live.keys() {
            if let SymKind::Local(e) = key.kind()
                && e.file == file
            {
                old_local.insert((kind, key));
            }
        }
        // New.
        let mut new: BTreeMap<SymKey, DemandFlags> = BTreeMap::new();
        let mut new_local: BTreeSet<(SlotKind, SymKey)> = BTreeSet::new();
        if let Some(d) = given.get(&file) {
            if d.demands.windows(2).any(|w| w[0].key >= w[1].key) {
                return Err(not_implemented());
            }
            for sd in &d.demands {
                match sd.key.kind() {
                    SymKind::Global(_) => {
                        new.insert(sd.key, sd.flags);
                    }
                    SymKind::Local(e) if e.file == file => {
                        new_local.extend(kinds(sd.flags).map(|k| (k, sd.key)));
                    }
                    // Another file's local: no file demands one.
                    SymKind::Local(_) => return Err(not_implemented()),
                }
            }
        }
        // Globals: +1 / -1 per kind the flags need, once per file.
        let keys: BTreeSet<SymKey> = old.keys().chain(new.keys()).copied().collect();
        for key in keys {
            let o = old.get(&key).copied().unwrap_or(DemandFlags::empty());
            let n = new.get(&key).copied().unwrap_or(DemandFlags::empty());
            for kind in SlotKind::ALL {
                let d =
                    i64::from(n.intersects(kind.demand())) - i64::from(o.intersects(kind.demand()));
                if d != 0 {
                    *net.entry((kind, key)).or_default() += d;
                }
            }
        }
        // Locals (IN-28b).
        if f.changed {
            for k in &old_local {
                *net.entry(*k).or_default() -= 1;
            }
            fresh.extend(new_local);
        } else {
            for k in old_local.difference(&new_local) {
                *net.entry(*k).or_default() -= 1;
            }
            for k in new_local.difference(&old_local) {
                *net.entry(*k).or_default() += 1;
            }
        }
    }
    rows.settle(&net, fresh)
}

/// The stored `SLOTS` rows: the live one of each `(kind, key)`, the last
/// dead one, and each kind's count (its next index).
struct Rows {
    live: BTreeMap<(SlotKind, SymKey), StoredSlot>,
    dead: BTreeMap<(SlotKind, SymKey), StoredSlot>,
    count: BTreeMap<SlotKind, u32>,
}

impl Rows {
    fn read(slots: &[StoredSlot]) -> Result<Self, IncrReason> {
        let mut live: BTreeMap<(SlotKind, SymKey), StoredSlot> = BTreeMap::new();
        let mut dead: BTreeMap<(SlotKind, SymKey), StoredSlot> = BTreeMap::new();
        let mut indices: BTreeMap<SlotKind, BTreeSet<u32>> = BTreeMap::new();
        for s in slots {
            if !indices.entry(s.kind).or_default().insert(s.index) {
                return Err(corrupt(format!(
                    "SLOTS: {} index {} twice",
                    kind_name(s.kind),
                    s.index
                )));
            }
            if s.refs > 0 {
                if live.insert((s.kind, s.key), *s).is_some() {
                    return Err(corrupt(format!(
                        "SLOTS: two live {} slots of one key",
                        kind_name(s.kind)
                    )));
                }
            } else {
                dead.insert((s.kind, s.key), *s);
            }
        }
        let mut count: BTreeMap<SlotKind, u32> = BTreeMap::new();
        for (k, set) in &indices {
            let n = u32::try_from(set.len()).map_err(|_| corrupt("SLOTS too large"))?;
            if set.last().is_some_and(|&l| l + 1 != n) {
                return Err(corrupt(format!("SLOTS: {} indices not dense", kind_name(*k))));
            }
            count.insert(*k, n);
        }
        Ok(Self { live, dead, count })
    }

    /// The plan from the net change per `(kind, key)` and the changed
    /// files' new locals (`fresh`, always appended).
    fn settle(
        mut self,
        net: &BTreeMap<(SlotKind, SymKey), i64>,
        fresh: BTreeSet<(SlotKind, SymKey)>,
    ) -> Result<Plan, IncrReason> {
        let (live, dead) = (&self.live, &self.dead);
        let mut out = Plan::default();
        let mut append: BTreeSet<(SlotKind, SymKey)> = BTreeSet::new();
        for (&(kind, key), &d) in net {
            if d == 0 {
                continue;
            }
            match live.get(&(kind, key)) {
                Some(s) => {
                    let after = i64::from(s.refs) + d;
                    if after < 0 {
                        return Err(corrupt(format!("SLOTS.refs below 0 ({})", kind_name(kind))));
                    }
                    let refs = u32::try_from(after).map_err(|_| corrupt("SLOTS.refs past u32"))?;
                    out.changed.push((*s, StoredSlot { refs, ..*s }));
                }
                None if d < 0 => {
                    return Err(corrupt(format!(
                        "a {} demand left a slot SLOTS does not hold",
                        kind_name(kind)
                    )));
                }
                None => match dead.get(&(kind, key)) {
                    // A global's dead slot revives in place (its identity is
                    // the key); a local's only for a file without a new
                    // version (`fresh` is never here).
                    Some(s) => {
                        let refs = u32::try_from(d).map_err(|_| corrupt("SLOTS.refs past u32"))?;
                        out.changed.push((*s, StoredSlot { refs, ..*s }));
                    }
                    None => {
                        append.insert((kind, key));
                    }
                },
            }
        }
        for k in fresh {
            append.insert(k);
        }
        // Appended per kind in `SymKey` order (S-64's order within the new
        // ones; S-113: after every stored slot of the kind).
        for (kind, key) in append {
            let n = self.count.entry(kind).or_insert(0);
            let refs = u32::try_from(net.get(&(kind, key)).copied().unwrap_or(0).max(1))
                .map_err(|_| corrupt("SLOTS.refs past u32"))?;
            out.appended.push(StoredSlot { kind, key, index: *n, refs });
            *n += 1;
        }
        out.changed.sort_by_key(|(s, _)| (s.kind, s.index));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    //! Exactness (WP-242): every hop's plan applied to the stored rows
    //! equals the rows a full link of the new files gives, from scratch:
    //! `SLOTS` live rows (kind, key, refs), indices dense with the stored
    //! ones kept and the appended ones after them in `SymKey` order; the
    //! dead rows stay at 0 (a compaction drops them, counts unchanged).

    use super::*;
    use mortar_core::id::{LocalSym, SymbolId};
    use mortar_core::incr::{MemoryLayout, SlotDemand, StoredKeyRef};
    use mortar_elf::SymbolIndex;

    const GOT: DemandFlags = DemandFlags::GOT;
    const PLT: DemandFlags = DemandFlags::PLT;
    const IPLT: DemandFlags = DemandFlags::IPLT;
    const IE: DemandFlags = DemandFlags::GOT_TPREL;

    fn g(id: u32) -> SymKey {
        SymKey::global(SymbolId(id))
    }
    fn l(file: u32, index: u32) -> SymKey {
        SymKey::local(LocalSym { file: FileId(file), index: SymbolIndex(index) })
    }

    /// Per file (1-based): `None` not live, else its demand by key.
    type Model = Vec<Option<Vec<(SymKey, DemandFlags)>>>;

    /// The from-scratch rows of a full link: per kind, the needed keys in
    /// `SymKey` order, refs = live files whose demand needs it; and
    /// `KEY_REFS` of the globals.
    fn scratch(m: &Model) -> MemoryLayout {
        let mut refs: BTreeMap<(SlotKind, SymKey), u32> = BTreeMap::new();
        let mut key_refs = Vec::new();
        for (k, f) in m.iter().enumerate() {
            let Some(d) = f else { continue };
            for &(key, flags) in d {
                for kind in kinds(flags) {
                    *refs.entry((kind, key)).or_default() += 1;
                }
                if let Some(id) = key.as_global() {
                    let file = FileId(k as u32 + 1);
                    key_refs.push(StoredKeyRef { key: id, file, demand: flags });
                }
            }
        }
        let mut slots = Vec::new();
        let mut next: BTreeMap<SlotKind, u32> = BTreeMap::new();
        for ((kind, key), r) in refs {
            let n = next.entry(kind).or_default();
            slots.push(StoredSlot { kind, key, index: *n, refs: r });
            *n += 1;
        }
        MemoryLayout { slots, key_refs, ..MemoryLayout::default() }
    }

    fn demands_of(m: &Model, files: &[u32]) -> Vec<FileDemands> {
        files
            .iter()
            .map(|&f| FileDemands {
                file: FileId(f),
                demands: m[f as usize - 1]
                    .iter()
                    .flatten()
                    .map(|&(key, flags)| SlotDemand { key, flags })
                    .collect(),
            })
            .collect()
    }

    /// One hop: the plan for `files` (`(file, changed)`), applied.
    fn hop(mem: &MemoryLayout, next: &Model, files: &[(u32, bool)]) -> (MemoryLayout, Plan) {
        let of: Vec<FileOf> =
            files.iter().map(|&(f, changed)| FileOf { file: FileId(f), changed }).collect();
        let ids: Vec<u32> = files.iter().map(|f| f.0).collect();
        let d = demands_of(next, &ids);
        let p = plan(&StoredLayout::new(mem), &of, &d).expect("the plan");
        let mut out = mem.clone();
        for (was, now) in &p.changed {
            let row = out.slots.iter_mut().find(|s| **s == *was).expect("a stored row");
            *row = *now;
        }
        out.slots.extend(p.appended.iter().copied());
        out.key_refs.retain(|r| !ids.contains(&r.file.0));
        for fd in &d {
            for sd in &fd.demands {
                if let Some(id) = sd.key.as_global() {
                    out.key_refs.push(StoredKeyRef { key: id, file: fd.file, demand: sd.flags });
                }
            }
        }
        out.key_refs.sort();
        check(&out, mem, next);
        (out, p)
    }

    /// The rows against the model; the stored rows' indices kept, the
    /// appended ones after them in `SymKey` order.
    fn check(now: &MemoryLayout, before: &MemoryLayout, m: &Model) {
        let fresh = scratch(m);
        let live = |v: &[StoredSlot]| -> Vec<(SlotKind, SymKey, u32)> {
            let mut x: Vec<_> =
                v.iter().filter(|s| s.refs > 0).map(|s| (s.kind, s.key, s.refs)).collect();
            x.sort_unstable();
            x
        };
        assert_eq!(live(&now.slots), live(&fresh.slots), "live SLOTS = the model's");
        for kind in SlotKind::ALL {
            let mut idx: Vec<u32> =
                now.slots.iter().filter(|s| s.kind == kind).map(|s| s.index).collect();
            idx.sort_unstable();
            assert!(idx.iter().enumerate().all(|(i, &x)| x == i as u32), "dense");
            let old = before.slots.iter().filter(|s| s.kind == kind).count() as u32;
            let new: Vec<SymKey> = {
                let mut v: Vec<&StoredSlot> =
                    now.slots.iter().filter(|s| s.kind == kind && s.index >= old).collect();
                v.sort_by_key(|s| s.index);
                v.into_iter().map(|s| s.key).collect()
            };
            assert!(new.windows(2).all(|w| w[0] < w[1]), "appended in SymKey order");
        }
        for s in &before.slots {
            assert!(
                now.slots.iter().any(|t| (t.kind, t.key, t.index) == (s.kind, s.key, s.index)),
                "a stored slot keeps its index"
            );
        }
    }

    fn files(v: &[Option<&[(SymKey, DemandFlags)]>]) -> Model {
        v.iter().map(|f| f.map(<[_]>::to_vec)).collect()
    }

    // spec: incr.md IN-72 item 7 (a changed file: old demand -1 per kind,
    // new +1; a slot at 0 retired; a new one appended; IN-28b: a changed
    // file's local slots die and its new local demand is appended)
    #[test]
    fn changed_file_exact() {
        let m0 =
            files(&[Some(&[(g(1), GOT), (g(2), GOT | PLT), (l(1, 5), GOT)]), Some(&[(g(2), GOT)])]);
        let s0 = scratch(&m0);
        let m1 = files(&[Some(&[(g(2), PLT), (g(3), IE), (l(1, 5), GOT)]), Some(&[(g(2), GOT)])]);
        let (s1, p) = hop(&s0, &m1, &[(1, true)]);
        // g1 GOT retired; g2 GOT 2 -> 1; l(1,5) dead then appended; g3 IE
        // appended.
        assert_eq!(p.appended.len(), 2, "{p:?}");
        assert!(p.changed.iter().any(|(s, n)| s.key == g(1) && n.refs == 0));
        let dead_local = s1.slots.iter().filter(|s| s.key == l(1, 5)).count();
        assert_eq!(dead_local, 2, "the old local's slot stays dead, the new one appended");
        // The same file without a new version (liveness only): locals kept.
        let (_, p) = hop(&s0, &m0, &[(1, false)]);
        assert_eq!(p, Plan::default());
    }

    // spec: incr.md IN-72 item 7 (an appeared file, a gone file, a drawn-in
    // member, then no longer extracted; a COMDAT keeper moving: the old
    // keeper's demand contributes nothing)
    #[test]
    fn appeared_gone_drawn_comdat() {
        let a: &[(SymKey, DemandFlags)] = &[(g(1), GOT), (g(4), IPLT)];
        let b: &[(SymKey, DemandFlags)] = &[(g(1), GOT), (g(5), GOT)];
        let c: &[(SymKey, DemandFlags)] = &[(g(5), GOT), (g(6), IE)];
        let m0 = files(&[Some(a), Some(b), None, None]);
        let s0 = scratch(&m0);
        // File 3 drawn in, file 4 appears.
        let m1 = files(&[Some(a), Some(b), Some(c), Some(&[(g(1), GOT)])]);
        let (s1, _) = hop(&s0, &m1, &[(3, false), (4, true)]);
        // File 1 gone; file 3 no longer extracted.
        let m2 = files(&[None, Some(b), None, Some(&[(g(1), GOT)])]);
        let (s2, p) = hop(&s1, &m2, &[(1, true), (3, false)]);
        assert!(
            p.changed.iter().any(|(s, n)| s.key == g(4) && s.kind == SlotKind::Iplt && n.refs == 0)
        );
        // A COMDAT keeper moving from file 2 to file 4: the copy's demand
        // (g(5)) leaves file 2's list and joins file 4's.
        let m3 = files(&[None, Some(&[(g(1), GOT)]), None, Some(&[(g(1), GOT), (g(5), GOT)])]);
        let (_, p) = hop(&s2, &m3, &[(2, false), (4, false)]);
        assert_eq!(p, Plan::default(), "the count is unchanged");
    }

    // spec: incr.md IN-72 item 7 (three hops: edit, inverse, edit; A then
    // B; a compaction between two edits)
    #[test]
    fn three_hops_and_compaction() {
        let v0 = files(&[Some(&[(g(1), GOT), (g(2), GOT)]), Some(&[(g(2), GOT)])]);
        let v1 = files(&[Some(&[(g(2), GOT), (g(3), GOT)]), Some(&[(g(2), GOT)])]);
        let s0 = scratch(&v0);
        let (s1, _) = hop(&s0, &v1, &[(1, true)]);
        // Inverse: g1 revives in place, g3 retired.
        let (s2, p) = hop(&s1, &v0, &[(1, true)]);
        assert!(p.appended.is_empty());
        let g1 = s2.slots.iter().find(|s| s.key == g(1)).unwrap();
        assert_eq!((g1.index, g1.refs), (0, 1), "revived at its index");
        let (s3, p) = hop(&s2, &v1, &[(1, true)]);
        assert!(p.appended.is_empty(), "g3 revives");
        // A then B.
        let v4 = files(&[Some(&[(g(2), GOT), (g(3), GOT), (g(7), GOT)]), Some(&[(g(2), GOT)])]);
        let (s4, _) = hop(&s3, &v4, &[(1, true)]);
        let v5 = files(&[Some(&[(g(2), GOT), (g(3), GOT), (g(7), GOT)]), Some(&[(g(7), GOT)])]);
        let (s5, _) = hop(&s4, &v5, &[(2, true)]);
        // A compaction: dead rows gone, counts unchanged; then an edit.
        let c = scratch(&v5);
        assert!(c.slots.iter().all(|s| s.refs > 0));
        let live = |m: &MemoryLayout| -> Vec<(SlotKind, SymKey, u32)> {
            let mut v: Vec<_> =
                m.slots.iter().filter(|s| s.refs > 0).map(|s| (s.kind, s.key, s.refs)).collect();
            v.sort_unstable();
            v
        };
        assert_eq!(live(&c), live(&s5));
        hop(&c, &v0, &[(1, true), (2, true)]);
    }

    // spec: incr.md IN-72 item 7, S-113 (two IFUNCs added: their IPLT
    // slots append in SymKey order, after every stored one; IN-72: past
    // the region's capacity, which is its count, `reserve-exhausted`)
    #[test]
    fn two_iplt_appended_in_key_order() {
        let m0 = files(&[Some(&[(g(4), IPLT)]), Some(&[])]);
        let s0 = scratch(&m0);
        let m1 = files(&[Some(&[(g(4), IPLT)]), Some(&[(g(2), IPLT), (g(9), IPLT)])]);
        let (_, p) = hop(&s0, &m1, &[(2, true)]);
        let got: Vec<(SymKey, u32)> = p.appended.iter().map(|s| (s.key, s.index)).collect();
        assert_eq!(got, [(g(2), 1), (g(9), 2)], "after g(4), by key, not by the old order");
        let of = [FileOf { file: FileId(2), changed: true }];
        let e = item_7(&StoredLayout::new(&s0), &of, &demands_of(&m1, &[2])).err().unwrap();
        assert_eq!(e.name(), "reserve-exhausted");
        assert!(e.report().contains("IPLT"), "{e:?}");
    }

    // spec: WP-239 S4-4 (the state checked: two live rows of one key, a
    // count below 0)
    #[test]
    fn corrupt_state_refused() {
        let m0 = files(&[Some(&[(g(1), GOT)])]);
        let mut s0 = scratch(&m0);
        s0.slots.push(StoredSlot { kind: SlotKind::Got, key: g(1), index: 1, refs: 1 });
        let of = [FileOf { file: FileId(1), changed: true }];
        let e = plan(&StoredLayout::new(&s0), &of, &[]).err().unwrap();
        assert_eq!(e.name(), "state-corrupt");
        let mut s1 = scratch(&m0);
        s1.slots[0].refs = 0;
        let e = plan(&StoredLayout::new(&s1), &of, &[]).err().unwrap();
        assert_eq!(e.name(), "state-corrupt");
    }
}

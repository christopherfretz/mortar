//! Applying a clip's relocations (I-35 step 2, I-36, §6.1).
//!
//! `apply` writes through a clipped `SectionOut` (R-2, WP-023b): the
//! window buffer holds the clip `[a, b)`, and writes outside it are
//! discarded by the arch. `RelocAt.site` is "the bytes of the input
//! section". When the whole section is at hand (resident, or the
//! `.eh_frame` source's bytes) the site is the section. Otherwise the
//! site is a *snapshot*: the clip's own input bytes, taken from the window
//! buffer before any record is applied (so the clip is read from the input
//! once, I-35 step 1), plus the few bytes on either side that the clip's
//! records may read, read from the store; section offset `base` of the
//! snapshot is 0 of the site, the records, the proof runs and the
//! `SectionOut`, and `redirect` adds `base` back.
//!
//! Records come from the plan (decoded) or from the input's `SHT_RELA`
//! table, read for the clip (I-82 "read twice"): resident tables are read
//! in place; a table that is not resident is copied whole when it is
//! small, when the clip is the whole section, or when its records are not
//! sorted, and otherwise located by binary search with one 8-byte read
//! per probe, then copied for the clip's records only.
//!
//! A section whose checks are fill's (`RecIndex::deferred`, I-14: the
//! scan did not read its records) is checked here as it is applied: each
//! record whose `r_offset` is in the clip (the last clip: or past it) is
//! decoded, classified and scanned as the scan would, and the windows of
//! the clip's records are checked for overlap (§3.4). A fault is not
//! reported here: the section is flagged ([`Scratch::flagged`]) and its
//! diagnostics are the scan's walk of it (`link::write`). Its order is
//! the plan's ([`Plan::deferred_order`]) or found from the records read
//! for the clip, and its `leads` bit is taken as set (exact: a section
//! without a lead type recognizes no sequence either way).

use mortar_arch::cx::fill_key;
use mortar_arch::diag::RelocPlace;
use mortar_arch::seq::{disjoint_in_order, overlaps};
use mortar_arch::{
    DemandOut, DemandSink, DynRole, Field, FillCx, FillTables, GlobalFlags, PlaceFacts, ProofRun,
    Records, RefName, Reloc, RelocAt, ScanCx, ScanError, ScanTables, SectionOut, Site, SlotKind,
    SymFacts, SymNames, SymValue, TargetKind, UnitWindow,
};
use mortar_core::demands::DemandFlags;
use mortar_core::records::Order;
use mortar_core::{FileId, SectionId, SymEntry, SymKey};
use mortar_diag::{Diagnostic, Key, Kind};
use mortar_elf::Class;
use mortar_elf::SymbolIndex;
use mortar_elf::raw::{Record, Rela};

const _: () = assert!(Rela::SIZE64 == 24 && Rela::SIZE32 == 12);

use crate::fill::{Image, Tables};
use crate::plan::{InputUnit, Plan, RecRows, RelaTable, RelocSection};
use crate::source;
use crate::tombstone;

/// Bytes of the site any record reads before its `r_offset`, and from
/// its `r_offset` on: the widest a record's `classify` and `apply` read
/// on any target of M1 (AArch64: R4, the GD sequence, reads its lead's
/// three words and the `nop` after the call, `[o, o + 16)` from any of
/// its members; `spec/a64.md` §8.4). The arch states no read bound
/// (`max_reach` bounds writes, I-111), so this is the image's, checked by
/// `i111_sequence_across_window_boundaries` and the e2e links.
pub(crate) const SITE_BACK: u64 = 16;
pub(crate) const SITE_FWD: u64 = 32;

/// Records decoded beyond the clip's on either side, so that a sequence
/// member finds its lead by record index (`spec/arch.md` §3.5): more than
/// any recognized sequence's extent (AArch64: 3).
const SEQ_MARGIN: u32 = 8;

/// A table that is not resident is copied whole up to this size.
pub(crate) const WHOLE_TABLE: u64 = 256 * 1024;

/// A worker's reused buffers.
#[derive(Debug, Default)]
pub(crate) struct Scratch {
    site: Vec<u8>,
    proof: Vec<ProofRun>,
    picks: Vec<u32>,
    /// Raw records copied from a table that is not resident.
    raw: Vec<u8>,
    /// Decoded records `[first, first + len)` of the current table.
    recs: Vec<Reloc>,
    /// A deferred section's (I-14): `r_sym` of each record of `recs`,
    /// `u32::MAX` for one whose symbol has no key.
    syms: Vec<u32>,
    /// A deferred section's order, found for the clip.
    perm: Vec<u32>,
    /// A deferred clip's windows (§3.4 overlap) and their pairs.
    windows: Vec<UnitWindow>,
    pairs: Vec<(u32, u32)>,
    /// The deferred sections a fault was found in (I-14), in the order
    /// found; taken by the job.
    pub flagged: Vec<SectionId>,
    /// Recent keys' values (and a deferred record's facts), direct-mapped.
    memo: Memo,
}

/// A worker's memo of [`Tables::record_sym`] (and [`Tables::scan_facts`]
/// at a mention) by key: both are functions of the key (and the mention)
/// alone over one link, and a section's records name few keys many
/// times (a `.debug_info`'s: its file's `.debug_str` section symbol).
#[derive(Debug, Default)]
struct Memo {
    slots: Vec<Option<MemoSlot>>,
}

#[derive(Debug, Clone, Copy)]
struct MemoSlot {
    key: SymKey,
    value: SymValue,
    split: bool,
    /// The facts at a mention, when asked for.
    at: Option<(Option<SymEntry>, Option<SymFacts>)>,
}

impl Memo {
    const SLOTS: usize = 1024;

    fn slot(&mut self, key: SymKey) -> &mut Option<MemoSlot> {
        if self.slots.is_empty() {
            self.slots.resize(Self::SLOTS, None);
        }
        let h = match key.kind() {
            mortar_core::SymKind::Global(id) => id.0,
            mortar_core::SymKind::Local(e) => e.file.0.wrapping_mul(0x9e37_79b9) ^ e.index.0,
        };
        let i = (h.wrapping_mul(0x85eb_ca6b) >> 22) as usize % Self::SLOTS;
        &mut self.slots[i]
    }

    /// [`Tables::record_sym`].
    fn record_sym(&mut self, tables: &dyn Tables, key: SymKey) -> (SymValue, bool) {
        let slot = self.slot(key);
        if let Some(m) = slot
            && m.key == key
        {
            return (m.value, m.split);
        }
        let (value, split) = tables.record_sym(key);
        *slot = Some(MemoSlot { key, value, split, at: None });
        (value, split)
    }

    /// [`Tables::record_sym_at`].
    fn record_sym_at(
        &mut self,
        tables: &dyn Tables,
        key: SymKey,
        mention: Option<SymEntry>,
    ) -> (SymValue, bool, Option<SymFacts>) {
        let slot = self.slot(key);
        if let Some(m) = slot
            && m.key == key
            && let Some((at, facts)) = m.at
            && at == mention
        {
            return (m.value, m.split, facts);
        }
        let (value, split, facts) = tables.record_sym_at(key, mention);
        *slot = Some(MemoSlot { key, value, split, at: Some((mention, facts)) });
        (value, split, facts)
    }
}

/// One clip of a relocated section: `[a, b)` in section offsets, held by
/// the buffer `relocate` is given.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Clip<'p> {
    pub file: FileId,
    /// The section's index in its file.
    pub index: u32,
    pub section: SectionId,
    pub name: &'p [u8],
    pub alloc: bool,
    /// The input section's `SHF_EXECINSTR` (`SectionOut::exec`).
    pub exec: bool,
    pub section_size: u64,
    /// File offset of the section's byte 0 in its input.
    pub sec_src: u64,
    /// `P` of section offset 0 (wrapping: for an `.eh_frame` record it is
    /// the address its input offset 0 would have).
    pub addr: u64,
    pub rs: &'p RelocSection,
    pub a: u64,
    pub b: u64,
}

impl<'p> Clip<'p> {
    /// The clip `[a, b)` of input unit `u`.
    pub(crate) fn of_input(
        plan: &'p Plan,
        u: &InputUnit,
        rs: &'p RelocSection,
        a: u64,
        b: u64,
    ) -> Self {
        Self {
            file: u.file,
            index: u.index,
            section: u.section,
            name: plan.name(u.name),
            alloc: u.alloc,
            exec: u.exec,
            section_size: u.section_size,
            sec_src: u.src - u.start,
            addr: u.addr,
            rs,
            a,
            b,
        }
    }
}

/// Records `[first, first + recs.len())` of a section of `total`, with
/// `r_offset` shifted by `−base`.
struct View<'a> {
    recs: &'a [Reloc],
    first: u32,
    total: u32,
    base: u64,
}

impl View<'_> {
    fn at(&self, i: u32) -> Option<&Reloc> {
        self.recs.get(i.checked_sub(self.first)? as usize)
    }
}

impl Records for View<'_> {
    fn len(&self) -> u32 {
        self.total
    }

    fn get(&self, i: u32) -> Option<Reloc> {
        let mut r = *self.at(i)?;
        r.r_offset = r.r_offset.wrapping_sub(self.base);
        Some(r)
    }
}

/// The tables with `redirect` taking shifted offsets, and with the value
/// of the record's key given (computed once per record, with `s` the
/// split target's where there is one: S-17, S-50).
struct ShiftTables<'a> {
    inner: &'a dyn Tables,
    section: SectionId,
    base: u64,
    value: Option<(SymKey, SymValue)>,
}

impl SymNames for ShiftTables<'_> {
    fn name(&self, key: SymKey) -> &[u8] {
        self.inner.name(key)
    }
}

impl FillTables for ShiftTables<'_> {
    fn sym(&self, key: SymKey) -> SymValue {
        match self.value {
            Some((k, v)) if k == key => v,
            _ => self.inner.sym(key),
        }
    }

    fn slot(&self, key: SymKey, kind: SlotKind) -> Option<u64> {
        self.inner.slot(key, kind)
    }

    fn plt(&self, key: SymKey) -> Option<u64> {
        self.inner.plt(key)
    }

    fn flags(&self, key: SymKey) -> DemandFlags {
        self.inner.flags(key)
    }

    fn redirect(&self, section: SectionId, offset: u64) -> Option<u64> {
        let offset = if section == self.section { offset.wrapping_add(self.base) } else { offset };
        self.inner.redirect(section, offset)
    }
}

fn internal(what: String) -> Diagnostic {
    Diagnostic::error(Kind::InternalError { what })
}

/// `r_offset` of record `i` of a raw table.
fn raw_offset(raw: &[u8], class: Class, i: u32) -> u64 {
    match class {
        Class::Elf64 => {
            let at = i as usize * 24;
            raw.get(at..at + 8)
                .map_or(u64::MAX, |b| u64::from_le_bytes(b.try_into().unwrap_or([0xff; 8])))
        }
        Class::Elf32 => {
            let at = i as usize * 12;
            raw.get(at..at + 4).map_or(u64::MAX, |b| {
                u64::from(u32::from_le_bytes(b.try_into().unwrap_or([0xff; 4])))
            })
        }
    }
}

/// The records `[lo, hi)` of raw table `raw` (record `lo` first), keys
/// named by the image's `keys`, appended to `out`.
/// With `syms` (a deferred section, I-14) each record's `r_sym` goes
/// there, and a symbol without a key is the record's fault, not an
/// internal error (its key is then symbol 0's; the link fails).
fn decode(
    img: &Image<'_>,
    t: &RelaTable,
    raw: &[u8],
    out: &mut Vec<Reloc>,
    mut syms: Option<&mut Vec<u32>>,
) -> Result<(), Diagnostic> {
    let size = t.entsize() as usize;
    for b in raw.chunks_exact(size) {
        let r = match t.class {
            Class::Elf64 => Rela::decode64(b),
            Class::Elf32 => Rela::decode32(b),
        }
        .ok_or_else(|| internal("fill: a relocation record cut short".into()))?;
        let key = match (img.keys.key(t.file, r.r_sym), syms.as_deref_mut()) {
            (Some(key), Some(syms)) => {
                syms.push(r.r_sym);
                key
            }
            (Some(key), None) => key,
            (None, Some(syms)) => {
                syms.push(u32::MAX);
                SymKey::local(SymEntry { file: t.file, index: SymbolIndex(0) })
            }
            (None, None) => {
                return Err(internal(format!(
                    "fill: file {}: symbol {} has no key (model §3.3)",
                    t.file.0, r.r_sym
                )));
            }
        };
        out.push(Reloc { r_offset: r.r_offset, key, r_type: r.r_type, addend: r.r_addend });
    }
    Ok(())
}

/// The order of `n` records with offsets `off` (I-29), found from them:
/// `Sorted`, or their indexes stably sorted by `r_offset` in `perm`.
fn local_order(n: u32, off: impl Fn(u32) -> u64, perm: &mut Vec<u32>) -> Order<'_> {
    if (1..n).all(|i| off(i - 1) <= off(i)) {
        return Order::Sorted;
    }
    perm.clear();
    perm.extend(0..n);
    perm.sort_by_key(|&i| off(i));
    Order::Permuted(perm)
}

/// The order of the records of table `t` (I-29), found from them, for a
/// deferred section (I-14) with a large table cut into several clips
/// ([`Plan::deferred_order`]): `None` when sorted, else the stable
/// permutation. One read of the table, so that its clips need not each
/// read it whole.
pub(crate) fn table_order(
    src: &dyn source::Source,
    plan: &Plan,
    t: &RelaTable,
) -> Result<Option<Box<[u32]>>, Diagnostic> {
    let all = t.range(0, t.count);
    let mut raw = Vec::new();
    let bytes = if let Some(bytes) = src.resident(t.file, all.clone()) {
        bytes
    } else {
        raw.resize((all.end - all.start) as usize, 0);
        source::read(src, t.file, all, &mut raw)
            .map_err(|e| source::copy_error(e, &plan.path(t.file)))?;
        &raw
    };
    let mut perm = Vec::new();
    Ok(match local_order(t.count, |i| raw_offset(bytes, t.class, i), &mut perm) {
        Order::Sorted => None,
        Order::Permuted(p) => Some(p.into()),
    })
}

/// The picks of a clip of table `t` (record indexes in record order,
/// into `picks`) and the records they index, decoded into `recs` from
/// index `first`: returns `first`.
///
/// A deferred section's (I-14) order is the plan's, or found from the
/// records (then all of them are read); `syms` gets their `r_sym`.
#[allow(clippy::too_many_arguments)]
fn load(
    img: &Image<'_>,
    t: &RelaTable,
    c: &Clip<'_>,
    lo: Option<u64>,
    hi: u64,
    picks: &mut Vec<u32>,
    raw: &mut Vec<u8>,
    recs: &mut Vec<Reloc>,
    deferred: Option<(&mut Vec<u32>, &mut Vec<u32>)>,
) -> Result<u32, Diagnostic> {
    let t = *t;
    let (known, mut syms, perm) = match deferred {
        Some((syms, perm)) => {
            syms.clear();
            (img.plan.deferred_order(c.section), Some(syms), Some(perm))
        }
        None => (Some(img.records.get(c.section)), None, None),
    };
    let after_lo = |off: u64| lo.is_none_or(|lo| off > lo);
    recs.clear();
    if t.count == 0 {
        return Ok(0);
    }
    let all = t.range(0, t.count);
    let path = || img.plan.path(t.file);
    let mut empty = Vec::new();
    let perm = perm.unwrap_or(&mut empty);
    if let Some(bytes) = img.source.resident(t.file, all.clone()) {
        let off = |i| raw_offset(bytes, t.class, i);
        let order = match known {
            Some(o) => o,
            None => local_order(t.count, off, perm),
        };
        pick(order, t.count, off, after_lo, hi, picks);
        let (w0, w1) = window(order, picks, t.count);
        let e = t.entsize() as usize;
        decode(img, &t, &bytes[w0 as usize * e..w1 as usize * e], recs, syms.as_deref_mut())?;
        return Ok(w0);
    }
    let whole = c.a == 0 && c.b >= c.section_size;
    if whole || all.end - all.start <= WHOLE_TABLE || !matches!(known, Some(Order::Sorted)) {
        raw.resize((all.end - all.start) as usize, 0);
        source::read(img.source, t.file, all, raw).map_err(|e| source::copy_error(e, &path()))?;
        let off = |i| raw_offset(raw, t.class, i);
        let order = match known {
            Some(o) => o,
            None => local_order(t.count, off, perm),
        };
        pick(order, t.count, off, after_lo, hi, picks);
        let (w0, w1) = window(order, picks, t.count);
        let e = t.entsize() as usize;
        decode(img, &t, &raw[w0 as usize * e..w1 as usize * e], recs, syms.as_deref_mut())?;
        return Ok(w0);
    }
    // Sorted and large: probe the bounds, then read the clip's records.
    let mut probe_err = None;
    let mut probe = |i: u32| -> u64 {
        let r = t.range(i, i + 1);
        let mut b = [0u8; 8];
        let n = if t.class == Class::Elf64 { 8 } else { 4 };
        match source::read(img.source, t.file, r.start..r.start + n, &mut b[..n as usize]) {
            Ok(()) if n == 8 => u64::from_le_bytes(b),
            Ok(()) => u64::from(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            Err(e) => {
                probe_err.get_or_insert(e);
                u64::MAX
            }
        }
    };
    let i0 = partition(t.count, |i| !after_lo(probe(i)));
    let i1 = partition(t.count, |i| probe(i) < hi).max(i0);
    if let Some(e) = probe_err {
        return Err(source::copy_error(e, &path()));
    }
    picks.extend(i0..i1);
    let (w0, w1) = window(Order::Sorted, picks, t.count);
    let r = t.range(w0, w1);
    raw.resize((r.end - r.start) as usize, 0);
    source::read(img.source, t.file, r, raw).map_err(|e| source::copy_error(e, &path()))?;
    decode(img, &t, raw, recs, syms)?;
    Ok(w0)
}

/// The first `i` in `[0, n)` for which `pred` is false (`pred` is true
/// on a prefix).
fn partition(n: u32, mut pred: impl FnMut(u32) -> bool) -> u32 {
    let (mut lo, mut hi) = (0u32, n);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if pred(mid) {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo
}

/// The records whose write window may intersect the clip: those with
/// `lo < r_offset < hi` (I-35: `lo = a − after − 8`, `hi = b + before`),
/// in record order, by binary search in the scan's order (I-29).
fn pick(
    order: Order<'_>,
    n: u32,
    off: impl Fn(u32) -> u64,
    after_lo: impl Fn(u64) -> bool,
    hi: u64,
    picks: &mut Vec<u32>,
) {
    match order {
        Order::Sorted => {
            let i0 = partition(n, |i| !after_lo(off(i)));
            let i1 = partition(n, |i| off(i) < hi);
            picks.extend(i0..i1.max(i0));
        }
        Order::Permuted(perm) => {
            let k0 = perm.partition_point(|&i| !after_lo(off(i)));
            let k1 = perm.partition_point(|&i| off(i) < hi);
            picks.extend_from_slice(&perm[k0..k1.max(k0)]);
            picks.sort_unstable();
        }
    }
}

/// The records to decode for `picks`: theirs and [`SEQ_MARGIN`] on either
/// side; every record of an unsorted section.
fn window(order: Order<'_>, picks: &[u32], n: u32) -> (u32, u32) {
    match (order, picks.first(), picks.last()) {
        (Order::Sorted, Some(&f), Some(&l)) => {
            (f.saturating_sub(SEQ_MARGIN), l.saturating_add(SEQ_MARGIN + 1).min(n))
        }
        (Order::Sorted, _, _) => (0, 0),
        (Order::Permuted(_), _, _) => (0, n),
    }
}

/// Applies the relocations of clip `c` to `buf` (which holds its bytes,
/// `buf.len() == b − a`). `site` is the whole input section when the
/// caller has it. Field errors of records whose `r_offset` lies in
/// `[a, b)` go to `errs` (I-36); a failed read of the input is returned.
/// A deferred section's (I-14) records are checked first; a fault flags
/// the section in `s.flagged`.
#[allow(clippy::too_many_lines)] // one record loop, the spec's steps in order
pub(crate) fn relocate(
    img: &Image<'_>,
    s: &mut Scratch,
    c: &Clip<'_>,
    site: Option<&[u8]>,
    buf: &mut [u8],
    errs: &mut Vec<(Key, Diagnostic)>,
) -> Result<(), Diagnostic> {
    let plan = img.plan;
    let (a, b) = (c.a, c.b);
    let aranges = c.name == tombstone::ARANGES;
    let w = img.arch.facts().max_reach;
    let mut reach = (u64::from(w.before), u64::from(w.after));
    if aranges {
        // I-112: the length after the field is written too.
        reach.1 = reach.1.max(8);
    }
    let lo = a.checked_sub(reach.1 + 8);
    let deferred = img.records.is_deferred(c.section);
    // I-14: the last clip of a deferred section checks the records past
    // its end too (each such record is a fault).
    let last = deferred && b >= c.section_size;
    let hi = if last { u64::MAX } else { b.saturating_add(reach.0) };
    let Scratch {
        site: site_buf,
        proof,
        picks,
        raw,
        recs,
        syms,
        perm,
        windows,
        pairs,
        flagged,
        memo,
    } = s;
    picks.clear();
    syms.clear();
    windows.clear();
    let (recs, first): (&[Reloc], u32) = match &c.rs.rows {
        RecRows::Decoded(r) => {
            let order = img.records.get(c.section);
            let after_lo = |off: u64| lo.is_none_or(|lo| off > lo);
            pick(order, r.len() as u32, |i| r[i as usize].r_offset, after_lo, hi, picks);
            (r, 0)
        }
        RecRows::Rela(t) => {
            let checks = deferred.then_some((&mut *syms, &mut *perm));
            let first = load(img, t, c, lo, hi, picks, raw, recs, checks)?;
            (recs, first)
        }
    };
    if picks.is_empty() {
        return Ok(());
    }
    // The site: the whole section, or the snapshot of the clip with the
    // bytes on either side its records may read.
    let whole = site.or_else(|| img.source.resident(c.file, c.sec_src..c.sec_src + c.section_size));
    let (input, base): (&[u8], u64) = if let Some(bytes) = whole {
        (bytes, 0)
    } else {
        let base = a.saturating_sub(SITE_BACK + reach.1 + 8);
        let end = b.saturating_add(SITE_FWD + reach.0).min(c.section_size).max(b);
        site_buf.clear();
        site_buf.resize((end - base) as usize, 0);
        let path = || plan.path(c.file);
        let (pre, rest) = site_buf.split_at_mut((a - base) as usize);
        let (mid, post) = rest.split_at_mut((b - a) as usize);
        mid.copy_from_slice(buf);
        if !pre.is_empty() {
            source::read(img.source, c.file, c.sec_src + base..c.sec_src + a, pre)
                .map_err(|e| source::copy_error(e, &path()))?;
        }
        if !post.is_empty() {
            source::read(img.source, c.file, c.sec_src + b..c.sec_src + end, post)
                .map_err(|e| source::copy_error(e, &path()))?;
        }
        (&site_buf[..], base)
    };
    let total = c.rs.len();
    let view = View { recs, first, total, base };
    proof.clear();
    proof.extend(c.rs.proof.iter().filter(|p| p.end > base).map(|p| ProofRun {
        start: p.start.saturating_sub(base),
        end: p.end - base,
        ..*p
    }));
    let leads = deferred || img.records.leads(c.section);
    let mut fault = false;
    for &i in picks.iter() {
        let Some(&rec) = view.at(i) else {
            let what = format!("fill: record {i} of section {} not decoded", c.section.0);
            return Err(internal(what));
        };
        let r = rec.r_offset;
        let shifted = r.wrapping_sub(base);
        let reports = (a..b).contains(&r) || (last && r >= b);
        let site = Site { bytes: input, offset: shifted };
        // I-14: `syms` is empty unless the section is deferred.
        let r_sym = syms.get((i - first) as usize).copied();
        if r_sym == Some(u32::MAX) {
            fault |= reports;
            continue;
        }
        let howto = match img.arch.classify(rec.r_type, site) {
            Ok(h) => h,
            Err(_) if deferred => {
                fault |= reports;
                continue;
            }
            Err(refusal) => {
                if reports {
                    let what = format!(
                        "fill: relocation type {} of {} record {i} refused at fill after scan ({refusal:?})",
                        rec.r_type,
                        plan.path(c.file).path()
                    );
                    errs.push((fill_key(c.file, c.index, i), internal(what)));
                }
                continue;
            }
        };
        let at = RelocAt {
            file: c.file,
            section: c.section,
            index: i,
            r_offset: shifted,
            key: rec.key,
            r_type: rec.r_type,
            addend: Some(rec.addend),
            howto: &howto,
            site,
            records: &view,
            proof,
            leads,
        };
        // A deferred record's value and its check's facts: one read.
        let (mut sym, split) = if deferred {
            let mention = r_sym.map(|s| SymEntry { file: c.file, index: SymbolIndex(s) });
            let (sym, split, facts) = memo.record_sym_at(img.tables, rec.key, mention);
            let check = check(img, c, &at, mention, facts);
            fault |= check.fault && reports;
            if check.errored {
                continue;
            }
            if let Some(w) = img.arch.unit_window(img.output, &at, img.tables) {
                windows.push(w);
            }
            (sym, split)
        } else {
            memo.record_sym(img.tables, rec.key)
        };
        let mut out = SectionOut::new(buf, a - base, c.addr.wrapping_add(base), c.alloc, c.name)
            .with_exec(c.exec);
        let value = match howto.field {
            Field::Data { bytes: width } if !sym.live => {
                tombstone::field(img.arch, c.name, c.alloc, width).map(|v| (v, width))
            }
            _ => None,
        };
        let result = if let Some((v, width)) = value {
            tomb(img, &mut out, &view, aranges, v, width, i, &rec, shifted, c.section_size);
            Ok(())
        } else {
            if split {
                match img.tables.split_target(rec.key, rec.addend) {
                    None => sym = img.tables.sym(rec.key),
                    Some(Ok(v)) => sym.s = v.wrapping_sub(rec.addend.cast_unsigned()),
                    Some(Err(d)) => {
                        if reports {
                            errs.push((fill_key(c.file, c.index, i), d));
                        }
                        continue;
                    }
                }
            }
            let shift = ShiftTables {
                inner: img.tables,
                section: c.section,
                base,
                value: Some((rec.key, sym)),
            };
            let cx =
                FillCx { tables: &shift, output: img.output, got_base: img.got_base, tls: img.tls };
            img.arch.apply(&cx, &at, &mut out)
        };
        if let Err(e) = result
            && reports
        {
            let path = plan.path(c.file);
            let section = String::from_utf8_lossy(c.name);
            let place = RelocPlace {
                file: c.file,
                path: &path,
                section_index: c.index,
                section: &section,
                record: i,
                r_offset: r,
            };
            let target = String::from_utf8_lossy(img.tables.name(rec.key)).into_owned();
            errs.push(mortar_arch::diag::fill(e, &place, RefName::Named(howto.name), &target));
        }
    }
    // §3.4: the windows of the clip's records without a fault of their
    // own, every pair that overlaps among them (each such pair meets in
    // some clip, whose picks hold both records).
    if deferred && !disjoint_in_order(windows) {
        pairs.clear();
        overlaps(windows, pairs);
        fault |= !pairs.is_empty();
    }
    if fault {
        flagged.push(c.section);
    }
    Ok(())
}

/// The facts of a deferred record's scan (I-14): the scan's, through
/// [`Tables::scan_facts`]; `missing` when the tables have none (the
/// record is then a fault, reported as the tables can).
struct CheckTables<'a> {
    tables: &'a dyn Tables,
    key: SymKey,
    mention: Option<SymEntry>,
    /// `key`'s facts at `mention`, read with its value
    /// ([`Tables::record_sym_at`]).
    facts: Option<SymFacts>,
    missing: std::sync::atomic::AtomicBool,
}

impl SymNames for CheckTables<'_> {
    fn name(&self, key: SymKey) -> &[u8] {
        self.tables.name(key)
    }
}

impl ScanTables for CheckTables<'_> {
    fn sym(&self, key: SymKey) -> SymFacts {
        if key == self.key
            && let Some(f) = self.facts
        {
            return f;
        }
        let mention = if key == self.key { self.mention } else { None };
        self.tables.scan_facts(key, mention).unwrap_or_else(|| {
            self.missing.store(true, std::sync::atomic::Ordering::Relaxed);
            SymFacts {
                kind: TargetKind::Abs,
                sym_type: 0,
                st_other: 0,
                tls: false,
                dead: false,
                comdat_only: false,
                gnu_warning: false,
            }
        })
    }
}

/// What `arch.scan` said of a deferred record (I-14).
#[derive(Debug, Default)]
struct Check {
    /// The record's type is one of `Arch::nonalloc_demand_types`: its
    /// demands were raised by the scan.
    demands: bool,
    record: u32,
    /// An error of the record itself: no window, not applied.
    errored: bool,
    /// Anything the scan would report: an error (of any record), an event,
    /// or a demand the scan did not raise.
    fault: bool,
}

impl DemandOut for Check {
    fn raise(&mut self, _: SymKey, flags: DemandFlags) {
        self.fault |= !flags.is_empty() && !self.demands;
    }

    fn raise_global(&mut self, flags: GlobalFlags) {
        self.fault |= !flags.is_empty() && !self.demands;
    }

    fn count(&mut self, _: DynRole, n: u32) {
        self.fault |= n != 0 && !self.demands;
    }

    fn error(&mut self, record: u32, _: ScanError) {
        self.fault = true;
        self.errored |= record == self.record;
    }

    fn reference(&mut self, _: u32, _: mortar_arch::EventClass, _: SymKey) {
        self.fault = true;
    }
}

/// `arch.scan` of a deferred record (I-14), as the scan's walk calls it
/// (§4.1 step 2: the record's own mention for an undefined key's type).
fn check(
    img: &Image<'_>,
    c: &Clip<'_>,
    at: &RelocAt<'_>,
    mention: Option<SymEntry>,
    facts: Option<SymFacts>,
) -> Check {
    let tables = CheckTables {
        tables: img.tables,
        key: at.key,
        mention,
        facts,
        missing: std::sync::atomic::AtomicBool::new(false),
    };
    let mut out = Check {
        demands: img.arch.nonalloc_demand_types().contains(&at.r_type),
        record: at.index,
        ..Check::default()
    };
    // A deferred section is not `SHF_ALLOC`, where `writable` is not read
    // (`spec/arch.md` §4.3 "Non-alloc places").
    let place = PlaceFacts { alloc: c.alloc, writable: false };
    let cx = ScanCx { tables: &tables, output: img.output, place, name: c.name };
    img.arch.scan(&cx, at, &mut DemandSink::new(&mut out));
    if tables.missing.into_inner() {
        out.fault = true;
        out.errored = true;
    }
    out
}

/// Writes the tombstone `v` (I-47, I-48) and, in `.debug_aranges`, the
/// tuple's zero length (I-112), into `out` at shifted offset `at`.
#[allow(clippy::too_many_arguments)]
fn tomb(
    img: &Image<'_>,
    out: &mut SectionOut<'_>,
    view: &View<'_>,
    aranges: bool,
    v: u64,
    width: u8,
    i: u32,
    rec: &Reloc,
    at: u64,
    section_size: u64,
) {
    let w = usize::from(width);
    out.write(at, &v.to_le_bytes()[..w]);
    if aranges && rec.r_type == img.plan.target.abs_addr && img.tables.target_alloc(rec.key) {
        let (lo, hi) = (rec.r_offset + u64::from(width), rec.r_offset + 2 * u64::from(width));
        // Every decoded record: the following ones of a sorted section
        // (the window reaches past the clip's), all of an unsorted one.
        let occupied = view
            .recs
            .iter()
            .enumerate()
            .any(|(k, x)| k as u32 + view.first != i && (lo..hi).contains(&x.r_offset));
        if !occupied && hi <= section_size {
            out.write(at + u64::from(width), &[0u8; 8][..w]);
        }
    }
}

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

use mortar_arch::cx::fill_key;
use mortar_arch::diag::RelocPlace;
use mortar_arch::{
    Field, FillCx, FillTables, ProofRun, Records, RefName, Reloc, RelocAt, SectionOut, Site,
    SlotKind, SymNames, SymValue,
};
use mortar_core::demands::DemandFlags;
use mortar_core::records::Order;
use mortar_core::{FileId, SectionId, SymKey};
use mortar_diag::{Diagnostic, Key, Kind};
use mortar_elf::Class;
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
const WHOLE_TABLE: u64 = 256 * 1024;

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
/// of one key replaced (a reference into `.eh_frame`, S-50).
struct ShiftTables<'a> {
    inner: &'a dyn Tables,
    section: SectionId,
    base: u64,
    value: Option<(SymKey, u64)>,
}

impl SymNames for ShiftTables<'_> {
    fn name(&self, key: SymKey) -> &[u8] {
        self.inner.name(key)
    }
}

impl FillTables for ShiftTables<'_> {
    fn sym(&self, key: SymKey) -> SymValue {
        let mut v = self.inner.sym(key);
        if let Some((k, s)) = self.value
            && k == key
        {
            v.s = s;
        }
        v
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
fn decode(
    img: &Image<'_>,
    t: &RelaTable,
    raw: &[u8],
    out: &mut Vec<Reloc>,
) -> Result<(), Diagnostic> {
    let size = t.entsize() as usize;
    for b in raw.chunks_exact(size) {
        let r = match t.class {
            Class::Elf64 => Rela::decode64(b),
            Class::Elf32 => Rela::decode32(b),
        }
        .ok_or_else(|| internal("fill: a relocation record cut short".into()))?;
        let key = img.keys.key(t.file, r.r_sym).ok_or_else(|| {
            internal(format!("fill: file {}: symbol {} has no key (model §3.3)", t.file.0, r.r_sym))
        })?;
        out.push(Reloc { r_offset: r.r_offset, key, r_type: r.r_type, addend: r.r_addend });
    }
    Ok(())
}

/// The picks of a clip of table `t` (record indexes in record order,
/// into `picks`) and the records they index, decoded into `recs` from
/// index `first`: returns `first`.
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
) -> Result<u32, Diagnostic> {
    let t = *t;
    let order = img.records.get(c.section);
    let after_lo = |off: u64| lo.is_none_or(|lo| off > lo);
    recs.clear();
    if t.count == 0 {
        return Ok(0);
    }
    let all = t.range(0, t.count);
    let path = || img.plan.path(t.file);
    if let Some(bytes) = img.source.resident(t.file, all.clone()) {
        pick(order, t.count, |i| raw_offset(bytes, t.class, i), after_lo, hi, picks);
        let (w0, w1) = window(order, picks, t.count);
        let e = t.entsize() as usize;
        decode(img, &t, &bytes[w0 as usize * e..w1 as usize * e], recs)?;
        return Ok(w0);
    }
    let whole = c.a == 0 && c.b >= c.section_size;
    if whole || all.end - all.start <= WHOLE_TABLE || matches!(order, Order::Permuted(_)) {
        raw.resize((all.end - all.start) as usize, 0);
        source::read(img.source, t.file, all, raw).map_err(|e| source::copy_error(e, &path()))?;
        pick(order, t.count, |i| raw_offset(raw, t.class, i), after_lo, hi, picks);
        let (w0, w1) = window(order, picks, t.count);
        let e = t.entsize() as usize;
        decode(img, &t, &raw[w0 as usize * e..w1 as usize * e], recs)?;
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
    let (w0, w1) = window(order, picks, t.count);
    let r = t.range(w0, w1);
    raw.resize((r.end - r.start) as usize, 0);
    source::read(img.source, t.file, r, raw).map_err(|e| source::copy_error(e, &path()))?;
    decode(img, &t, raw, recs)?;
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
    let hi = b.saturating_add(reach.0);
    let Scratch { site: site_buf, proof, picks, raw, recs } = s;
    picks.clear();
    let (recs, first): (&[Reloc], u32) = match &c.rs.rows {
        RecRows::Decoded(r) => {
            let order = img.records.get(c.section);
            let after_lo = |off: u64| lo.is_none_or(|lo| off > lo);
            pick(order, r.len() as u32, |i| r[i as usize].r_offset, after_lo, hi, picks);
            (r, 0)
        }
        RecRows::Rela(t) => {
            let first = load(img, t, c, lo, hi, picks, raw, recs)?;
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
    let leads = img.records.leads(c.section);
    for &i in picks.iter() {
        let Some(&rec) = view.at(i) else {
            let what = format!("fill: record {i} of section {} not decoded", c.section.0);
            return Err(internal(what));
        };
        let r = rec.r_offset;
        let shifted = r.wrapping_sub(base);
        let reports = (a..b).contains(&r);
        let site = Site { bytes: input, offset: shifted };
        let howto = match img.arch.classify(rec.r_type, site) {
            Ok(h) => h,
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
            addend: rec.addend,
            howto: &howto,
            site,
            records: &view,
            proof,
            leads,
        };
        let mut out = SectionOut::new(buf, a - base, c.addr.wrapping_add(base), c.alloc, c.name)
            .with_exec(c.exec);
        let value = match howto.field {
            Field::Data { bytes: width }
                if !img.tables.sym(rec.key).live || !img.tables.piece_live(rec.key, rec.addend) =>
            {
                tombstone::field(img.arch, c.name, c.alloc, width).map(|v| (v, width))
            }
            _ => None,
        };
        let result = if let Some((v, width)) = value {
            tomb(img, &mut out, &view, aranges, v, width, i, &rec, shifted, c.section_size);
            Ok(())
        } else {
            let eh = match img.tables.split_target(rec.key, rec.addend) {
                None => None,
                Some(Ok(v)) => Some((rec.key, v.wrapping_sub(rec.addend.cast_unsigned()))),
                Some(Err(d)) => {
                    if reports {
                        errs.push((fill_key(c.file, c.index, i), d));
                    }
                    continue;
                }
            };
            let shift = ShiftTables { inner: img.tables, section: c.section, base, value: eh };
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
    Ok(())
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

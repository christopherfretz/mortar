//! Applying a clip's relocations (I-35 step 2, I-36, §6.1).
//!
//! Stand-in for request R-2 (WP-023b): on main, `SectionOut` is indexed
//! from section offset 0 and has no clip, and `RelocAt.site` is the whole
//! input section. The one private function [`relocate`] therefore runs
//! `apply` on a *shifted* view of each record: section offset `base =
//! r_offset − SITE_BACK` becomes 0 in the site, the records, the proof
//! runs and the `SectionOut`, and `redirect` adds `base` back. The view
//! covers `[base, r_offset + SITE_FWD)` of the section, which holds every
//! byte `apply` reads or writes (I-111's reach, the GD sequence's 16 bytes
//! before and after). When that range lies inside the clip, `apply` writes
//! straight into the window buffer; otherwise into a scratch copy, of
//! which only the bytes inside the clip are copied back.

use mortar_arch::diag::RelocPlace;
use mortar_arch::{
    Field, FillCx, FillTables, ProofRun, RefName, Reloc, RelocAt, Records, SectionOut, Site,
    SlotKind, SymNames, SymValue,
};
use mortar_arch::cx::fill_key;
use mortar_core::demands::DemandFlags;
use mortar_core::{SectionId, SymKey};
use mortar_diag::{Diagnostic, Key, Kind};

use crate::fill::{Image, Tables};
use crate::plan::{InputUnit, RecOrder, RelocSection};
use crate::source;
use crate::tombstone::{self, Rule, Tomb};

/// Bytes of the site before `r_offset`.
pub(crate) const SITE_BACK: u64 = 16;
/// Bytes of the site from `r_offset` on.
pub(crate) const SITE_FWD: u64 = 32;

/// A worker's reused buffers.
#[derive(Debug, Default)]
pub(crate) struct Scratch {
    site: Vec<u8>,
    out: Vec<u8>,
    proof: Vec<ProofRun>,
    picks: Vec<u32>,
}

/// Records with `r_offset` shifted by `−base`.
struct Shifted<'a> {
    records: &'a [Reloc],
    base: u64,
}

impl Records for Shifted<'_> {
    fn len(&self) -> u32 {
        self.records.len() as u32
    }

    fn get(&self, i: u32) -> Option<Reloc> {
        let mut r = *self.records.get(i as usize)?;
        r.r_offset = r.r_offset.wrapping_sub(self.base);
        Some(r)
    }
}

/// The tables with `redirect` taking shifted offsets.
struct ShiftTables<'a> {
    inner: &'a dyn Tables,
    section: SectionId,
    base: u64,
}

impl SymNames for ShiftTables<'_> {
    fn name(&self, key: SymKey) -> &[u8] {
        self.inner.name(key)
    }
}

impl FillTables for ShiftTables<'_> {
    fn sym(&self, key: SymKey) -> SymValue {
        self.inner.sym(key)
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

/// The records of `rs` whose write window may intersect `[a, b)`: those with
/// `a − after − 8 < r_offset < b + before` (I-35), in record order, into
/// `picks`.
fn pick(rs: &RelocSection, a: u64, b: u64, reach: (u64, u64), picks: &mut Vec<u32>) {
    picks.clear();
    let lo = a.checked_sub(reach.1 + 8);
    let hi = b.saturating_add(reach.0);
    let r = &rs.records;
    let after_lo = |off: u64| lo.is_none_or(|lo| off > lo);
    match &rs.order {
        RecOrder::Sorted => {
            let i0 = r.partition_point(|x| !after_lo(x.r_offset));
            let i1 = r.partition_point(|x| x.r_offset < hi);
            picks.extend(i0 as u32..i1.max(i0) as u32);
        }
        RecOrder::Permuted(perm) => {
            let k0 = perm.partition_point(|&i| !after_lo(r[i as usize].r_offset));
            let k1 = perm.partition_point(|&i| r[i as usize].r_offset < hi);
            picks.extend_from_slice(&perm[k0..k1.max(k0)]);
            picks.sort_unstable();
        }
    }
}

/// Applies the relocations of input unit `u` that touch the clip `[a, b)`
/// (section offsets) to `buf`, which holds the clip's bytes. Field errors
/// of records whose `r_offset` lies in `[a, b)` go to `errs` (I-36); a
/// failed read of the input is returned.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)] // one record loop, the spec's steps in order
pub(crate) fn relocate(
    img: &Image<'_>,
    s: &mut Scratch,
    u: &InputUnit,
    rs: &RelocSection,
    a: u64,
    b: u64,
    buf: &mut [u8],
    errs: &mut Vec<(Key, Diagnostic)>,
) -> Result<(), Diagnostic> {
    let plan = img.plan;
    let name = plan.name(u.name);
    let rule = tombstone::rule(name);
    let mut reach = plan.target.max_reach;
    if rule == Some(Rule::Debug(Tomb::Aranges)) {
        // I-112: the length after the field is written too.
        reach.1 = reach.1.max(8);
    }
    let Scratch { site: site_buf, out: out_buf, proof, picks } = s;
    pick(rs, a, b, reach, picks);
    let sec_src = u.src - u.start;
    let site_of = |r: u64| {
        let base = r.saturating_sub(SITE_BACK);
        let end = r.saturating_add(SITE_FWD).min(u.section_size).max(base.min(u.section_size));
        (base.min(end), end)
    };
    // The input bytes of every picked record's site: resident, or one read
    // of their union (the staged store; one read per record would cost a
    // system call per relocation).
    let (lo, hi) = picks.iter().fold((u64::MAX, 0), |(lo, hi), &i| {
        let (b0, e0) = site_of(rs.records[i as usize].r_offset);
        (lo.min(b0), hi.max(e0))
    });
    let resident = if picks.is_empty() { None } else { img.source.resident(u.file, sec_src + lo..sec_src + hi) };
    if resident.is_none() && !picks.is_empty() {
        site_buf.resize((hi - lo) as usize, 0);
        source::read(img.source, u.file, sec_src + lo..sec_src + hi, site_buf)
            .map_err(|e| source::copy_error(e, &plan.path(u.file)))?;
    }
    let input: &[u8] = resident.unwrap_or(site_buf);
    for &i in picks.iter() {
        let rec = rs.records[i as usize];
        let r = rec.r_offset;
        let (base, end) = site_of(r);
        let site: &[u8] = &input[(base - lo) as usize..(end - lo) as usize];
        let shifted = r - base;
        let howto = match img.arch.classify(rec.r_type, Site { bytes: site, offset: shifted }) {
            Ok(h) => h,
            Err(refusal) => {
                if (a..b).contains(&r) {
                    let what = format!(
                        "fill: relocation type {} of {} record {i} refused at fill after scan ({refusal:?})",
                        rec.r_type,
                        plan.path(u.file).path()
                    );
                    errs.push((fill_key(u.file, u.index, i), Diagnostic::error(Kind::InternalError { what })));
                }
                continue;
            }
        };
        proof.clear();
        proof.extend(rs.proof.iter().filter(|p| p.end > base && p.start < end).map(|p| ProofRun {
            start: p.start.saturating_sub(base),
            end: p.end - base,
            ..*p
        }));
        let records = Shifted { records: &rs.records, base };
        let at = RelocAt {
            file: u.file,
            section: u.section,
            index: i,
            r_offset: shifted,
            key: rec.key,
            r_type: rec.r_type,
            addend: rec.addend,
            howto: &howto,
            site: Site { bytes: site, offset: shifted },
            records: &records,
            proof,
        };
        let direct = base >= a && end <= b;
        let bytes: &mut [u8] = if direct {
            &mut buf[(base - a) as usize..(end - a) as usize]
        } else {
            out_buf.clear();
            out_buf.extend_from_slice(site);
            let (lo, hi) = (base.max(a), end.min(b));
            if lo < hi {
                out_buf[(lo - base) as usize..(hi - base) as usize]
                    .copy_from_slice(&buf[(lo - a) as usize..(hi - a) as usize]);
            }
            out_buf
        };
        let mut out = SectionOut { bytes, addr: u.addr.wrapping_add(base), alloc: u.alloc, name };
        let result = match (rule, howto.field) {
            (Some(rule), Field::Data { bytes: width })
                if (rule == Rule::Zero || !u.alloc) && !img.tables.sym(rec.key).live =>
            {
                tomb(img, &mut out, rs, rule, width, &rec, shifted);
                Ok(())
            }
            _ => {
                let shift = ShiftTables { inner: img.tables, section: u.section, base };
                let cx = FillCx { tables: &shift, output: img.output, got_base: img.got_base, tls: img.tls };
                img.arch.apply(&cx, &at, &mut out)
            }
        };
        if !direct {
            let (lo, hi) = (base.max(a), end.min(b));
            if lo < hi {
                buf[(lo - a) as usize..(hi - a) as usize]
                    .copy_from_slice(&out_buf[(lo - base) as usize..(hi - base) as usize]);
            }
        }
        if let Err(e) = result
            && (a..b).contains(&r)
        {
            let path = plan.path(u.file);
            let section = String::from_utf8_lossy(name);
            let place = RelocPlace {
                file: u.file,
                path: &path,
                section_index: u.index,
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

/// Writes the tombstone (I-47, I-48) and, in `.debug_aranges`, the tuple's
/// zero length (I-112), into `out` at shifted offset `at`.
fn tomb(
    img: &Image<'_>,
    out: &mut SectionOut<'_>,
    rs: &RelocSection,
    rule: Rule,
    width: u8,
    rec: &Reloc,
    at: u64,
) {
    let t = img.plan.target;
    let v = tombstone::value(rule, width, t.is_64()).to_le_bytes();
    let w = usize::from(width);
    let at = at as usize;
    if let Some(field) = out.bytes.get_mut(at..at + w) {
        field.copy_from_slice(&v[..w]);
    }
    if rule == Rule::Debug(Tomb::Aranges) && rec.r_type == t.abs_addr && img.tables.target_alloc(rec.key) {
        let (lo, hi) = (rec.r_offset + u64::from(width), rec.r_offset + 2 * u64::from(width));
        let occupied = match &rs.order {
            RecOrder::Sorted => {
                let k = rs.records.partition_point(|x| x.r_offset < lo);
                rs.records.get(k).is_some_and(|x| x.r_offset < hi)
            }
            RecOrder::Permuted(_) => rs.records.iter().any(|x| (lo..hi).contains(&x.r_offset)),
        };
        if !occupied && let Some(len) = out.bytes.get_mut(at + w..at + 2 * w) {
            len.fill(0);
        }
    }
}

//! P-P5, `.eh_frame` at `-r` (`spec/plumb.md` §14.5). Owner: **RL-5**.
//!
//! An input FDE is removed when its function is in a dropped section or
//! its range is 0 (L-R6); every live FDE has exactly one equivalent
//! output FDE (the same range, augmentation length and instructions
//! without their trailing nops, an equivalent LSDA and an equivalent
//! CIE), and every output FDE comes from a live input FDE. The matching
//! gives the record map P-P3 reads for the records of `.eh_frame`.

use std::collections::BTreeMap;

use crate::reloc::eh::{Cie, Fde, Frames, parse_as};
use crate::relocatable::{Place, Rel, Rv, Tgt};
use crate::report::{Finding, Rule, Via};

/// One FDE of an input or of the output, with its targets.
struct Rec {
    fde: Fde,
    /// The record's length (header included).
    len: u64,
    pc: Tgt,
    lsda: Option<Tgt>,
}

/// The length of the record at `off` (its length field included).
fn rec_len(b: &[u8], off: usize) -> u64 {
    let g = |o: usize, n: usize| {
        b.get(o..o + n).map_or(0, |s| {
            let mut a = [0u8; 8];
            a[..n].copy_from_slice(s);
            u64::from_le_bytes(a)
        })
    };
    let l = g(off, 4);
    if l == 0xffff_ffff { 12 + g(off + 4, 8) } else { 4 + l }
}

/// The record of a relocation section's list at `offset`.
fn rec_at(recs: &[Rel], offset: u64) -> Option<&Rel> {
    recs.iter().find(|r| r.offset == offset)
}

/// Whether two CIEs are equivalent (§14.5): version, augmentation,
/// alignments, return register, encodings, initial instructions; the
/// personality by its target (`pa`, `pb`).
fn cie_eq(a: &Cie, b: &Cie, pa: Option<&Tgt>, pb: Option<&Tgt>, rv: &Rv<'_>) -> Result<(), String> {
    if a.version != b.version {
        return Err(format!("the CIE's version ({} for {})", b.version, a.version));
    }
    if a.aug != b.aug {
        return Err(format!(
            "the CIE's augmentation ('{}' for '{}')",
            String::from_utf8_lossy(&b.aug),
            String::from_utf8_lossy(&a.aug)
        ));
    }
    if (a.code_align, a.data_align, a.ra) != (b.code_align, b.data_align, b.ra) {
        return Err("the CIE's alignments or return register".into());
    }
    if (a.fde_enc, a.lsda_enc) != (b.fde_enc, b.lsda_enc) {
        return Err("the CIE's pointer encodings".into());
    }
    if a.insns != b.insns {
        return Err("the CIE's initial instructions".into());
    }
    match (pa, pb) {
        (None, None) => Ok(()),
        (Some(x), Some(y)) if rv.equiv(x, y).is_some() => Ok(()),
        (Some(x), y) => Err(format!(
            "the CIE's personality ({} for {x})",
            y.map_or_else(|| "none".to_string(), ToString::to_string)
        )),
        (None, Some(y)) => Err(format!("the CIE's personality ({y} for none)")),
    }
}

/// The input FDEs of input `f`'s `.eh_frame` `s`.
fn input_frames(rv: &Rv<'_>, f: usize, s: usize) -> Result<(Frames, Vec<Rel>), String> {
    let e = &rv.ins[f].elf;
    let addr = if rv.class64 { 8 } else { 4 };
    let fr = parse_as(&e.secs[s].data, addr)?;
    let recs: Vec<Rel> = e.relocs_of(s).iter().flat_map(|&r| e.rels(r)).collect();
    Ok((fr, recs))
}

/// The pointer field's target in an input: its record, else the value
/// (an absolute pointer without a record is a value, PC-relative 0 is
/// "no function").
fn in_ptr(rv: &Rv<'_>, f: usize, data: &[u8], recs: &[Rel], off: usize, raw: u64) -> Tgt {
    match rec_at(recs, off as u64) {
        Some(r) => rv.in_target(f, r, data),
        None => Tgt::Abs(raw as i64),
    }
}

fn out_ptr(rv: &Rv<'_>, ors: Option<usize>, recs: &[Rel], off: usize, raw: u64) -> Tgt {
    match (ors, rec_at(recs, off as u64)) {
        (Some(rs), Some(r)) => rv.out_target(rs, r),
        _ => Tgt::Abs(raw as i64),
    }
}

/// P-P5 and the record map.
#[allow(clippy::too_many_lines)] // inputs, output, the match, leftovers
pub(crate) fn check(rv: &mut Rv<'_>) {
    // The output's FDEs.
    let mut outs: Vec<Rec> = Vec::new();
    let mut out_cies: Vec<(usize, Cie, Option<Tgt>)> = Vec::new();
    let mut out_fr = Frames::default();
    if let Some(o) = rv.out_eh {
        let o = o as usize;
        let data = rv.out.secs[o].data.clone();
        let ors = (1..rv.out.secs.len()).find(|&r| rv.out.secs[r].is_reloc() && rv.out.secs[r].info as usize == o);
        let recs = ors.map(|r| rv.out_rels.get(&r).cloned().unwrap_or_default()).unwrap_or_default();
        match parse_as(&data, if rv.class64 { 8 } else { 4 }) {
            Ok(fr) => {
                for d in &fr.fdes {
                    let pc = out_ptr(rv, ors, &recs, d.pc.off, d.pc.raw);
                    let lsda = d.lsda.map(|l| out_ptr(rv, ors, &recs, l.off, l.raw));
                    outs.push(Rec { fde: d.clone(), len: rec_len(&data, d.off), pc, lsda });
                }
                for (i, c) in fr.cies.iter().enumerate() {
                    let p = c.personality.map(|p| out_ptr(rv, ors, &recs, p.off, p.raw));
                    out_cies.push((i, c.clone(), p));
                }
                out_fr = fr;
            }
            Err(e) => {
                rv.p(5, format!("the output's .eh_frame cannot be read: {e}"));
                return;
            }
        }
    }
    let mut used = vec![false; outs.len()];
    // Output CIE offsets: the CIE of each output FDE is its index; the
    // offset is found from the FDE's CIE pointer.
    let out_cie_off = |d: &Fde| -> u64 {
        let data = rv.out_eh.map(|o| rv.out.secs[o as usize].data.as_slice()).unwrap_or(&[]);
        let id_at = d.off as u64 + if rec_len(data, d.off) > 4 && data.get(d.off..d.off + 4) == Some(&[0xff; 4]) { 12 } else { 4 };
        let id = data.get(id_at as usize..id_at as usize + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        id_at.saturating_sub(u64::from(id))
    };
    let mut removed_by_tgt: Vec<(Tgt, String)> = Vec::new();
    let mut maps: BTreeMap<(usize, usize), BTreeMap<u64, Option<u64>>> = BTreeMap::new();
    let mut findings: Vec<Finding> = Vec::new();
    let mut live = 0u64;
    let mut removed = 0u64;
    for f in 0..rv.ins.len() {
        for s in 1..rv.ins[f].elf.secs.len() {
            if rv.places[f][s] != Place::Framed {
                continue;
            }
            let (fr, recs) = match input_frames(rv, f, s) {
                Ok(x) => x,
                Err(e) => {
                    let site = rv.ins[f].site(s, 0);
                    findings.push(Finding::new(Rule::InputUnreadable, format!("{}: {e}", rv.ins[f].sname(s))).site(site));
                    continue;
                }
            };
            let data = rv.ins[f].elf.secs[s].data.clone();
            let mut rmap: BTreeMap<u64, Option<u64>> = BTreeMap::new();
            // CIE offsets of the input: walk the records.
            let mut cie_offs = Vec::new();
            {
                let mut pos = 0usize;
                let mut seen_fde = 0usize;
                while pos + 4 <= data.len() {
                    let l = rec_len(&data, pos);
                    if l <= 4 {
                        break;
                    }
                    if seen_fde < fr.fdes.len() && fr.fdes[seen_fde].off == pos {
                        seen_fde += 1;
                    } else {
                        cie_offs.push(pos);
                    }
                    pos += l as usize;
                }
            }
            let mut cie_out: BTreeMap<usize, u64> = BTreeMap::new();
            for d in &fr.fdes {
                let pc = in_ptr(rv, f, &data, &recs, d.pc.off, d.pc.raw);
                let lsda = d.lsda.map(|l| in_ptr(rv, f, &data, &recs, l.off, l.raw));
                let dead = matches!(pc, Tgt::Dropped(..)) || d.range == 0;
                if dead {
                    removed += 1;
                    rmap.insert(d.off as u64, None);
                    removed_by_tgt.push((pc, rv.ins[f].label()));
                    continue;
                }
                live += 1;
                let cie = &fr.cies[d.cie];
                let pcie = cie.personality.map(|p| in_ptr(rv, f, &data, &recs, p.off, p.raw));
                // The first unused output FDE of an equivalent function.
                let mut first_diff: Option<String> = None;
                let mut hit = None;
                for (k, o) in outs.iter().enumerate() {
                    if used[k] || rv.equiv(&pc, &o.pc).is_none() {
                        continue;
                    }
                    let oc = &out_fr.cies[o.fde.cie];
                    let opers = out_cies[o.fde.cie].2.clone();
                    let diff = if o.fde.range != d.range {
                        Some(format!("range {:#x} for {:#x}", o.fde.range, d.range))
                    } else if o.fde.aug_len != d.aug_len {
                        Some("the augmentation length".to_string())
                    } else if o.fde.insns != d.insns {
                        Some("the instructions".to_string())
                    } else if match (&lsda, &o.lsda) {
                        (None, None) => false,
                        (Some(a), Some(b)) => rv.equiv(a, b).is_none(),
                        _ => true,
                    } {
                        Some(format!(
                            "the LSDA ({} for {})",
                            o.lsda.as_ref().map_or_else(|| "none".into(), ToString::to_string),
                            lsda.as_ref().map_or_else(|| "none".into(), ToString::to_string)
                        ))
                    } else {
                        cie_eq(cie, oc, pcie.as_ref(), opers.as_ref(), rv).err()
                    };
                    match diff {
                        None => {
                            hit = Some(k);
                            break;
                        }
                        Some(d) => {
                            first_diff.get_or_insert(d);
                        }
                    }
                }
                match hit {
                    Some(k) => {
                        used[k] = true;
                        let o = &outs[k];
                        rmap.insert(d.off as u64, Some(o.fde.off as u64));
                        if let Some(&cpos) = cie_offs.iter().rev().find(|&&c| c < d.off).or(cie_offs.first()) {
                            // The FDE's own CIE: from its CIE pointer.
                            let cp = cie_of(&data, d.off).unwrap_or(cpos as u64);
                            cie_out.entry(cp as usize).or_insert_with(|| out_cie_off(&o.fde));
                        }
                    }
                    None => {
                        let site = rv.ins[f].site(s, d.off as u64);
                        let msg = match first_diff {
                            Some(x) => format!("P-P5 FDE for {pc} of {} differs: {x}", rv.ins[f].label()),
                            None => format!("FDE for {pc} of {} is not in the output", rv.ins[f].label()),
                        };
                        let msg = msg.trim_start_matches("P-P5 ").to_string();
                        findings.push(Finding::new(Rule::Rel(5), msg).site(site));
                        rmap.insert(d.off as u64, None);
                    }
                }
            }
            for c in cie_offs {
                rmap.insert(c as u64, cie_out.get(&c).copied());
            }
            maps.insert((f, s), rmap);
        }
    }
    // The output FDEs no live input FDE gives.
    for (k, o) in outs.iter().enumerate() {
        if used[k] {
            continue;
        }
        let from = removed_by_tgt
            .iter()
            .find(|(t, _)| match (t, &o.pc) {
                (Tgt::Dropped(..), _) => true,
                (a, b) => rv.equiv(a, b).is_some(),
            })
            .map_or_else(|| "no input FDE".to_string(), |(_, file)| format!("a removed FDE of {file}"));
        // A 0-range FDE kept as GNU ld keeps it? Not accepted: §14.5.
        let _ = o.len;
        findings.push(Finding::new(
            Rule::Rel(5),
            format!("output FDE at {:#x} for {} comes from {from}", o.fde.off, o.pc),
        ));
    }
    rv.cov.fdes_live = live;
    rv.cov.fdes_removed = removed;
    rv.report.coverage.fdes_checked += live;
    rv.report.coverage.fdes_dropped += removed;
    rv.report.findings.extend(findings);
    rv.frames = maps;
    if rv.mortar() {
        rframe(rv);
    }
    framed_records(rv);
}

/// The CIE an FDE at `off` points to.
fn cie_of(data: &[u8], off: usize) -> Option<u64> {
    let l = data.get(off..off + 4)?;
    let id_at = if l == [0xff; 4] { off + 12 } else { off + 4 };
    let id = data.get(id_at..id_at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))?;
    (id_at as u64).checked_sub(u64::from(id))
}

/// mortar's map states the record map with `rframe` (§14.2).
fn rframe(rv: &mut Rv<'_>) {
    let Some(rel) = rv.map.rel.as_ref() else { return };
    let mut fid: BTreeMap<u32, usize> = BTreeMap::new();
    for (i, f) in rv.ins.iter().enumerate() {
        fid.entry(f.map_file).or_insert(i);
    }
    let mut msgs = Vec::new();
    let mut stated: BTreeMap<(usize, usize, u64), Option<u64>> = BTreeMap::new();
    for r in &rel.frames {
        let Some(&f) = fid.get(&r.file) else { continue };
        let s = match &r.section {
            crate::map::SectionRef::Index(i) => *i as usize,
            crate::map::SectionRef::Name(n) => {
                rv.ins[f].elf.secs.iter().position(|x| &x.name == n).unwrap_or(0)
            }
        };
        stated.insert((f, s, r.input_offset), r.output_offset);
    }
    for (&(f, s), m) in &rv.frames {
        for (&off, &to) in m {
            let say = stated.remove(&(f, s, off));
            let show = |v: Option<Option<u64>>| match v {
                None => "nothing".to_string(),
                Some(None) => "removed".to_string(),
                Some(Some(o)) => format!("{o:#x}"),
            };
            if say != Some(to) {
                msgs.push(format!(
                    "rframe {} {off:#x} says {}, the record map gives {}",
                    rv.ins[f].sname(s),
                    show(say),
                    show(Some(to))
                ));
            }
        }
    }
    for ((f, s, off), _) in stated {
        msgs.push(format!("rframe {} {off:#x} names no input record", rv.ins[f].sname(s)));
    }
    // L-R6 and I-R6: the live records in (FileId, record) order, end to
    // end, a zero terminator exactly where an input has one.
    if let Some(o) = rv.out_eh {
        let mut cursor = 0u64;
        for (&(f, s), m) in &rv.frames {
            let data = &rv.ins[f].elf.secs[s].data;
            let mut off = 0usize;
            while off + 4 <= data.len() {
                let len = rec_len(data, off);
                let term = data[off..off + 4] == [0; 4];
                let live = match m.get(&(off as u64)) {
                    Some(to) => *to,
                    None if term => Some(cursor),
                    None => None,
                };
                if let Some(at) = live {
                    if at != cursor {
                        msgs.push(format!(
                            "record {} {off:#x} is at {at:#x}, L-R6 puts it at {cursor:#x}",
                            rv.ins[f].sname(s)
                        ));
                    } else if term && rv.out.secs[o as usize].data.get(at as usize..at as usize + 4) != Some(&[0u8; 4][..]) {
                        msgs.push(format!("the zero terminator of {} {off:#x} is not at {at:#x}", rv.ins[f].sname(s)));
                    }
                    cursor = at + len;
                }
                off = off.saturating_add(usize::try_from(len).unwrap_or(usize::MAX).max(4));
            }
        }
        let size = rv.out.secs[o as usize].size;
        if size != cursor {
            msgs.push(format!(".eh_frame is {size:#x} bytes, L-R6's records end at {cursor:#x}"));
        }
    }
    for m in msgs {
        rv.p(5, m);
    }
}

/// P-P3 for the records of `.eh_frame`: through the record map; a
/// removed record's records are dropped (§14.3); a CIE's records may
/// be shared by the inputs' CIEs that became one.
fn framed_records(rv: &mut Rv<'_>) {
    let Some(o) = rv.out_eh else { return };
    let keys: Vec<(usize, usize)> = rv.frames.keys().copied().collect();
    for (f, s) in keys {
        let data = rv.ins[f].elf.secs[s].data.clone();
        let m = rv.frames[&(f, s)].clone();
        for rs in rv.ins[f].elf.relocs_of(s) {
            let recs = rv.ins[f].elf.rels(rs);
            for (ri, r) in recs.iter().enumerate() {
                let Some((&start, &to)) = m.range(..=r.offset).next_back() else { continue };
                let Some(to) = to else {
                    rv.report.coverage.absent_records += 1;
                    continue;
                };
                let at = to + (r.offset - start);
                let t = rv.in_target(f, r, &data);
                rv.report.coverage.records += 1;
                // A shared CIE's record already matched: accept it again
                // when equivalent.
                let shared = rv.out_recs.get(&(o, at)).and_then(|c| {
                    c.iter().find(|&&(ors, oi)| {
                        rv.used[&(ors, oi)] && {
                            let orr = rv.out_rels[&ors][oi];
                            orr.ty == r.ty && rv.equiv(&t, &rv.out_target(ors, &orr)).is_some()
                        }
                    })
                });
                let fresh = rv.out_recs.get(&(o, at)).is_some_and(|c| c.iter().any(|k| !rv.used[k]));
                if shared.is_some() && !fresh {
                    rv.report.coverage.add_verified(Via::EhFrame, r.ty);
                    continue;
                }
                rv.match_record(f, s, rs, ri, r, &t, o, at, Via::EhFrame);
            }
        }
    }
}

//! P-P6, section groups at `-r` (`spec/plumb.md` §14.5), and the COMDAT
//! keepers §14.3 derives (RV-R7). Owner: **RL-5**.
//!
//! The keeper of a COMDAT signature is the group of the least `FileId`,
//! then the first by section index; every other copy's members are
//! dropped with their relocation sections. A kept group (and every
//! non-COMDAT group) is one `SHT_GROUP` of the output whose members are
//! the output sections of its members and their relocation sections,
//! none shared with another piece; `--force-group-allocation` leaves no
//! group at all.

use std::collections::{BTreeMap, BTreeSet};

use crate::relocatable::{GRP_COMDAT, Place, Rv, SHF_GROUP, SHT_GROUP, Sec, is_container};
use crate::report::{Finding, Rule};

/// The words of a group section: the flag word, then the members.
pub(crate) fn words(s: &Sec) -> (u32, Vec<u32>) {
    let mut w = s.data.chunks_exact(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let flag = w.next().unwrap_or(0);
    (flag, w.collect())
}

/// The signature of group section `g` of an ELF file.
pub(crate) fn signature(e: &crate::relocatable::Elf, g: usize) -> String {
    let s = &e.secs[g];
    if Some(s.link as usize) != e.symtab {
        return String::new();
    }
    e.syms
        .get(s.info as usize)
        .map(|y| {
            if y.ty() == crate::relocatable::STT_SECTION {
                e.secs.get(y.shndx as usize).map_or_else(String::new, |x| x.name.clone())
            } else {
                y.name.clone()
            }
        })
        .unwrap_or_default()
}

/// The keeper of each COMDAT signature.
pub(crate) fn keepers(rv: &mut Rv<'_>) {
    let mut k: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for f in 0..rv.ins.len() {
        let e = &rv.ins[f].elf;
        for g in 1..e.secs.len() {
            if e.secs[g].ty != SHT_GROUP || words(&e.secs[g]).0 & GRP_COMDAT == 0 {
                continue;
            }
            k.entry(signature(e, g)).or_insert((f, g));
        }
    }
    rv.keepers = k;
}

/// The sections of input `f` §14.3 drops as members of a COMDAT copy
/// that is not kept.
pub(crate) fn dead_members(rv: &Rv<'_>, f: usize) -> BTreeSet<usize> {
    let e = &rv.ins[f].elf;
    let mut dead = BTreeSet::new();
    for g in 1..e.secs.len() {
        if e.secs[g].ty != SHT_GROUP {
            continue;
        }
        let (flag, members) = words(&e.secs[g]);
        if flag & GRP_COMDAT == 0 || rv.keepers.get(&signature(e, g)) == Some(&(f, g)) {
            continue;
        }
        for m in members {
            dead.insert(m as usize);
            dead.extend(e.relocs_of(m as usize));
        }
    }
    dead
}

/// The output section of an input section, if it is placed.
fn out_of(rv: &Rv<'_>, f: usize, s: usize) -> Option<u32> {
    match rv.places[f].get(s)? {
        Place::Whole { out, .. } | Place::Merged { out } => Some(*out),
        Place::Framed => rv.out_eh,
        _ => None,
    }
}

/// P-P6.
#[allow(clippy::too_many_lines)] // the groups, then SHF_GROUP, then mortar's form
pub(crate) fn check(rv: &mut Rv<'_>) {
    let n = rv.out.secs.len();
    let out_groups: Vec<usize> = (1..n).filter(|&o| rv.out.secs[o].ty == SHT_GROUP).collect();
    rv.cov.groups = out_groups.len() as u64;
    let mut msgs: Vec<(u8, String)> = Vec::new();
    if rv.opts.force_group {
        if !out_groups.is_empty() {
            msgs.push((
                6,
                format!(
                    "the output has {} groups; --force-group-allocation leaves none",
                    out_groups.len()
                ),
            ));
        }
        for o in 1..n {
            if rv.out.secs[o].flags & SHF_GROUP != 0 {
                msgs.push((
                    6,
                    format!(
                        "section {o} {} has SHF_GROUP; --force-group-allocation leaves no group",
                        rv.out.secs[o].name
                    ),
                ));
            }
        }
        for (r, m) in msgs {
            rv.p(r, m);
        }
        return;
    }
    let symtab = rv.out.symtab.unwrap_or(0) as u32;
    // The output groups by signature.
    let mut by_sig: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for &o in &out_groups {
        by_sig.entry(signature(&rv.out, o)).or_default().push(o);
    }
    // Every output section a group lists.
    let mut listed: BTreeMap<u32, usize> = BTreeMap::new();
    for &o in &out_groups {
        for m in words(&rv.out.secs[o]).1 {
            if let Some(prev) = listed.insert(m, o) {
                msgs.push((
                    6,
                    format!(
                        "section {m} {} is a member of the groups in sections {prev} and {o}",
                        rv.out.secs.get(m as usize).map_or("?", |s| s.name.as_str())
                    ),
                ));
            }
        }
    }
    let mut claimed: BTreeSet<usize> = BTreeSet::new();
    // The kept groups, in FileId order.
    for f in 0..rv.ins.len() {
        let e = &rv.ins[f].elf;
        for g in 1..e.secs.len() {
            if e.secs[g].ty != SHT_GROUP {
                continue;
            }
            let (flag, members) = words(&e.secs[g]);
            let sig = signature(e, g);
            let comdat = flag & GRP_COMDAT != 0;
            if comdat && rv.keepers.get(&sig) != Some(&(f, g)) {
                continue;
            }
            // The output sections the group's members went to.
            let mut want: BTreeSet<u32> = BTreeSet::new();
            let mut member_out: Vec<(usize, u32)> = Vec::new();
            for &m in &members {
                if let Some(o) = out_of(rv, f, m as usize) {
                    want.insert(o);
                    member_out.push((m as usize, o));
                    for r in 1..n {
                        if rv.out.secs[r].is_reloc() && rv.out.secs[r].info == o {
                            want.insert(r as u32);
                        }
                    }
                }
            }
            let cands = by_sig.get(&sig).cloned().unwrap_or_default();
            if comdat && cands.len() > 1 {
                msgs.push((
                    6,
                    format!("COMDAT group {sig} is in the output {} times", cands.len()),
                ));
            }
            // Its output group: the candidate with the members of this
            // group, else the first unclaimed one.
            let hit = cands
                .iter()
                .copied()
                .filter(|o| !claimed.contains(o))
                .find(|&o| words(&rv.out.secs[o]).1.iter().any(|m| want.contains(m)))
                .or_else(|| cands.iter().copied().find(|o| !claimed.contains(o)));
            let Some(o) = hit else {
                if !want.is_empty() || members.is_empty() {
                    msgs.push((
                        6,
                        format!("group {sig} of {} is not in the output", rv.ins[f].label()),
                    ));
                }
                continue;
            };
            claimed.insert(o);
            rv.group_out.insert((f, g), o as u32);
            let os = rv.out.secs[o].clone();
            let (oflag, omembers) = words(&os);
            let mut bad = Vec::new();
            if os.flags != 0 {
                bad.push(format!("sh_flags is {:#x}, not 0", os.flags));
            }
            if os.link != symtab {
                bad.push(format!("sh_link is {}, not .symtab's {symtab}", os.link));
            }
            if os.entsize != 4 {
                bad.push(format!("sh_entsize is {}, not 4", os.entsize));
            }
            if oflag != flag {
                bad.push(format!("the flag word is {oflag:#x}, not {flag:#x}"));
            }
            let got: BTreeSet<u32> = omembers.iter().copied().collect();
            if got != want {
                let miss: Vec<String> = want.difference(&got).map(|m| m.to_string()).collect();
                let extra: Vec<String> = got.difference(&want).map(|m| m.to_string()).collect();
                bad.push(format!(
                    "its members are not its input members' output sections and their relocation sections (missing [{}], extra [{}])",
                    miss.join(", "),
                    extra.join(", ")
                ));
            }
            for x in bad {
                msgs.push((6, format!("group {sig} (section {o}): {x}")));
            }
            // No member shares its output section with another piece.
            for &(m, mo) in &member_out {
                for g2 in 0..rv.ins.len() {
                    for s2 in 0..rv.ins[g2].elf.secs.len() {
                        if (g2, s2) == (f, m) || out_of(rv, g2, s2) != Some(mo) {
                            continue;
                        }
                        if g2 == f && members.contains(&(s2 as u32)) {
                            continue;
                        }
                        msgs.push((
                            6,
                            format!(
                                "member {} of group {sig} shares output section {mo} with {}",
                                rv.ins[f].sname(m),
                                rv.ins[g2].sname(s2)
                            ),
                        ));
                    }
                }
            }
            if rv.mortar() {
                let mut order: Vec<u32> = Vec::new();
                for &(_, mo) in &member_out {
                    if order.contains(&mo) {
                        continue;
                    }
                    order.push(mo);
                    for r in 1..n {
                        if rv.out.secs[r].is_reloc() && rv.out.secs[r].info == mo {
                            order.push(r as u32);
                        }
                    }
                }
                if omembers != order {
                    msgs.push((6, format!("group {sig} (section {o}) lists its members out of L-R5's order (members in order, each relocation section after its target) (mortar)")));
                }
                if os.name != ".group" || os.align != 4 {
                    msgs.push((6, format!("group {sig} (section {o}) is named '{}' aligned {}, not .group aligned 4 (mortar)", os.name, os.align)));
                }
            }
            // P-P4: the signature symbol, when defined, is in a member.
            if let Some(y) = rv.out.syms.get(os.info as usize) {
                if y.in_section()
                    && !got.contains(&y.shndx)
                    && y.shndx as usize != o
                    && y.ty() != crate::relocatable::STT_SECTION
                {
                    msgs.push((
                        4,
                        format!(
                            "group signature {} is in section {}, not its group {o}",
                            y.name, y.shndx
                        ),
                    ));
                }
            } else {
                msgs.push((
                    6,
                    format!("group {sig} (section {o}): sh_info {} names no symbol", os.info),
                ));
            }
        }
    }
    for &o in &out_groups {
        if !claimed.contains(&o) {
            msgs.push((
                6,
                format!(
                    "group {} (section {o}) comes from no kept input group",
                    signature(&rv.out, o)
                ),
            ));
        }
    }
    // SHF_GROUP exactly on the members.
    for o in 1..n {
        let s = &rv.out.secs[o];
        if s.ty == SHT_GROUP || is_container(s) {
            continue;
        }
        let has = s.flags & SHF_GROUP != 0;
        let member = listed.contains_key(&(o as u32));
        if has != member {
            msgs.push((
                6,
                format!(
                    "section {o} {} {}",
                    s.name,
                    if has {
                        "has SHF_GROUP and no group lists it"
                    } else {
                        "is a group member without SHF_GROUP"
                    }
                ),
            ));
        }
    }
    for (r, m) in msgs {
        rv.push(Finding::new(Rule::Rel(r), m));
    }
}

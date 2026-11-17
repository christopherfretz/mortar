//! The record that owns a field of `.gcc_except_table` (`spec/plumb.md`
//! §5.7b; `spec/resolve.md` §6.3, WP-177; WP-201): the FDE whose LSDA
//! start is the greatest one not above the field's offset, over the FDEs
//! of every input file. The verifier derives it itself: its own parse of
//! the inputs' `.eh_frame`, its own liveness of each FDE (§6.4: dropped
//! when its initial location resolves into a section not in the output,
//! or its range is 0), its own COMDAT keeper. Nothing comes from a map's
//! account of the link beyond the placements every rule reads.
//!
//! A child of `reloc`: it resolves the pointer fields with the engine's
//! symbol resolution (§4.4).

use std::collections::BTreeMap;

use mortar_elf::consts::{SHF_EXCLUDE, STB_LOCAL};
use mortar_elf::{Machine, SymbolIndex, SymbolSection};

use super::eh::{Field, parse_as};
use super::{Engine, Target};

/// Whether an FDE is written (§6.4's live) or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Life {
    Live,
    Dead,
    /// Its initial location cannot be resolved (§6.4's unknown).
    Unknown,
}

/// The owner of a field of `.gcc_except_table`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Owner {
    /// An FDE that is written reads the LSDA (of two FDEs at one start,
    /// either).
    Live,
    /// Every FDE at the start is dropped: the LSDA is never read.
    Dead,
    /// No FDE has an LSDA in the section at or before the field.
    None,
    /// The verifier cannot name the owner (an `.eh_frame` it cannot read,
    /// an LSDA pointer it cannot resolve, an FDE whose liveness is not
    /// known): never taken as dead.
    Unknown(String),
}

/// The LSDA starts of every kept input `.eh_frame`'s FDEs, by table
/// section.
#[derive(Debug, Default)]
pub(crate) struct LsdaStarts {
    /// `(object, section)` -> start -> the life of its FDEs, combined:
    /// live if any is, else unknown if any is, else dead.
    starts: BTreeMap<(usize, usize), BTreeMap<u64, Life>>,
    /// Why no owner can be named in any table: an LSDA pointer that may
    /// name any table was not resolved, or an `.eh_frame` was not read.
    unknown: Option<String>,
}

impl LsdaStarts {
    pub(crate) fn add(&mut self, table: (usize, usize), start: u64, life: Life) {
        let e = self.starts.entry(table).or_default().entry(start).or_insert(life);
        *e = match (*e, life) {
            (Life::Live, _) | (_, Life::Live) => Life::Live,
            (Life::Unknown, _) | (_, Life::Unknown) => Life::Unknown,
            (Life::Dead, Life::Dead) => Life::Dead,
        };
    }

    pub(crate) fn unknown(&mut self, why: String) {
        self.unknown.get_or_insert(why);
    }

    /// The owner of the field at offset `r` of table section `table`.
    pub(crate) fn owner(&self, table: (usize, usize), r: u64) -> Owner {
        if let Some(why) = &self.unknown {
            return Owner::Unknown(why.clone());
        }
        let Some((&start, &life)) = self.starts.get(&table).and_then(|m| m.range(..=r).next_back())
        else {
            return Owner::None;
        };
        match life {
            Life::Live => Owner::Live,
            Life::Dead => Owner::Dead,
            Life::Unknown => Owner::Unknown(format!(
                "the FDE whose LSDA starts at {start:#x} has an initial location the verifier cannot resolve"
            )),
        }
    }
}

/// The relocation types of an `.eh_frame` pointer field (§6.4).
fn pointer(machine: Machine, ty: u32) -> bool {
    match machine {
        Machine::AArch64 => matches!(ty, 257 | 258 | 260 | 261),
        Machine::X86_64 => matches!(ty, 1 | 2 | 10 | 11 | 24),
        Machine::I386 => matches!(ty, 1 | 2),
    }
}

impl Engine<'_, '_> {
    /// The owner of the field at offset `r` of `.gcc_except_table` section
    /// `si` of object `oi`; the starts are found when a record first asks.
    pub(crate) fn lsda_owner(&mut self, oi: usize, si: usize, r: u64) -> Owner {
        let starts = match self.lsda_starts.take() {
            Some(s) => s,
            None => self.find_lsda_starts(),
        };
        let o = starts.owner((oi, si), r);
        self.lsda_starts = Some(starts);
        o
    }

    /// Every FDE of every input `.eh_frame` the link keeps (not a member
    /// of a COMDAT copy the verifier's keeper drops, not `SHF_EXCLUDE`)
    /// whose LSDA pointer has a relocation: its start (the pointer's
    /// target in its section, `S + A` there) and its life.
    fn find_lsda_starts(&mut self) -> LsdaStarts {
        let mut out = LsdaStarts::default();
        let objs = self.objs;
        let dropped = self.comdat.get_or_init(|| crate::fate::comdat_dropped(objs)).clone();
        let word = self.word() as usize;
        for (oi, o) in objs.iter().enumerate() {
            let machine = o.elf.machine();
            for (si, s) in o.secs.iter().enumerate() {
                if !(s.alloc() && s.is_eh_frame(machine))
                    || s.shdr.sh_flags & SHF_EXCLUDE != 0
                    || dropped.get(oi).and_then(|d| d.get(si)).copied().unwrap_or(false)
                {
                    continue;
                }
                let raw = match s.relocs.map(|rs| o.records(rs)) {
                    None => Vec::new(),
                    Some(Some(r)) => r,
                    Some(None) => {
                        out.unknown(format!(
                            "{}: the relocations of its .eh_frame cannot be read",
                            o.label()
                        ));
                        continue;
                    }
                };
                let data = o.data(si);
                let frames = match parse_as(&data, word) {
                    Ok(f) => f,
                    Err(m) => {
                        out.unknown(format!("{}: its .eh_frame cannot be read: {m}", o.label()));
                        continue;
                    }
                };
                let a64 = machine == Machine::AArch64;
                let relocs: BTreeMap<u64, (u32, u32, Option<i64>)> = raw
                    .iter()
                    .filter(|r| !(r.2 == 0 || a64 && r.2 == 256))
                    .map(|&(off, sym, ty, a)| (off, (sym, ty, a)))
                    .collect();
                for d in &frames.fdes {
                    let Some(l) = d.lsda else { continue };
                    // An LSDA pointer without a relocation is no start.
                    let Some(&(sym, ty, a)) = relocs.get(&(l.off as u64)) else { continue };
                    if !pointer(machine, ty) {
                        out.unknown(format!(
                            "{}:({}+{:#x}): an LSDA pointer of relocation type {ty}",
                            o.label(),
                            s.name,
                            l.off
                        ));
                        continue;
                    }
                    let a = a.unwrap_or_else(|| i64::from(l.raw as u32 as i32));
                    let Some((toi, tsi, start)) = self.lsda_target(oi, sym, a) else {
                        out.unknown(format!(
                            "{}:({}+{:#x}): an LSDA pointer whose target the verifier cannot place in an input section",
                            o.label(),
                            s.name,
                            l.off
                        ));
                        continue;
                    };
                    let life = if d.range == 0 {
                        Life::Dead
                    } else {
                        self.fde_life(oi, machine, d.pc, &relocs)
                    };
                    out.add((toi, tsi), start, life);
                }
            }
        }
        out
    }

    /// §6.4's liveness of an FDE by its initial location `pc`.
    fn fde_life(
        &mut self,
        oi: usize,
        machine: Machine,
        pc: Field,
        relocs: &BTreeMap<u64, (u32, u32, Option<i64>)>,
    ) -> Life {
        let Some(&(sym, ty, a)) = relocs.get(&(pc.off as u64)) else {
            // R-P7: 0 without a relocation describes no code; an absolute
            // value written in place is taken as written.
            return match (pc.raw, pc.enc & 0x70) {
                (0, _) => Life::Dead,
                (_, 0) => Life::Live,
                _ => Life::Unknown,
            };
        };
        if !pointer(machine, ty) {
            return Life::Unknown;
        }
        let a = a.unwrap_or_else(|| i64::from(pc.raw as u32 as i32));
        match self.resolve_at(oi, sym, a).0 {
            Target::Discarded(_) => Life::Dead,
            Target::Addr(_) | Target::UndefWeak => Life::Live,
            _ => Life::Unknown,
        }
    }

    /// The input section and offset an LSDA pointer against symbol `idx`
    /// of object `oi` plus `a` names: a local or section symbol's own
    /// section; a global's definition here, or the one the link chose
    /// (§4.4 step 2) when this object does not define it.
    fn lsda_target(&self, oi: usize, idx: u32, a: i64) -> Option<(usize, usize, u64)> {
        let t = self.objs[oi].symtab?;
        let s = t.get(SymbolIndex(idx)).ok()?;
        let at = |doi: usize, k: usize, v: u64| Some((doi, k, v.wrapping_add(a as u64)));
        match t.section_of(SymbolIndex(idx), &s).ok()? {
            SymbolSection::Section(k) if s.binding() == STB_LOCAL => {
                at(oi, k.0 as usize, s.st_value)
            }
            SymbolSection::Section(k) => {
                let name = String::from_utf8_lossy(t.name(&s).ok()?).into_owned();
                match self.chosen_def(&name) {
                    Some((doi, di)) => {
                        let dt = self.objs[doi].symtab?;
                        let ds = dt.get(SymbolIndex(di)).ok()?;
                        match dt.section_of(SymbolIndex(di), &ds).ok()? {
                            SymbolSection::Section(dk) => at(doi, dk.0 as usize, ds.st_value),
                            _ => None,
                        }
                    }
                    None => at(oi, k.0 as usize, s.st_value),
                }
            }
            SymbolSection::Undefined => {
                let name = String::from_utf8_lossy(t.name(&s).ok()?).into_owned();
                let (doi, di) = self.chosen_def(&name)?;
                let dt = self.objs[doi].symtab?;
                let ds = dt.get(SymbolIndex(di)).ok()?;
                match dt.section_of(SymbolIndex(di), &ds).ok()? {
                    SymbolSection::Section(dk) => at(doi, dk.0 as usize, ds.st_value),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

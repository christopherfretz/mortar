//! Non-alloc references to sections that are not in the output
//! (`spec/plumb.md` §5.8): the tombstone each linker writes.

use mortar_elf::consts::{SHF_GROUP, STB_LOCAL, STT_SECTION};
use mortar_elf::{Class, SymbolIndex, SymbolSection};

use super::{Engine, Rec, trunc};
use crate::a64::{Field, Op};
use crate::locate::Place;
use crate::map::Format;
use crate::report::{Finding, Reason, Rule, Site, Via};

/// GNU ld's debug names (`spec/resolve.md` §6.3).
pub(crate) fn is_debug_name(n: &str) -> bool {
    [".debug", ".zdebug", ".line", ".stab", ".gnu.linkonce.wi.", ".gnu.debuglto_.debug_"]
        .iter()
        .any(|p| n.starts_with(p))
        || n == ".gnu_debuglink"
}

/// Whether `r_type` is the class's absolute address-sized type of the
/// output's machine (`spec/image.md` I-112): `R_AARCH64_ABS64` on
/// `EM_AARCH64`, `R_X86_64_64` on `EM_X86_64`, `R_386_32` on `EM_386`.
fn is_abs_addr(machine: u16, r_type: u32) -> bool {
    use mortar_elf::consts::{EM_386, EM_AARCH64, EM_X86_64};
    match machine {
        EM_AARCH64 => r_type == 257,
        EM_X86_64 => r_type == crate::x64::R64,
        EM_386 => r_type == 1,
        _ => false,
    }
}

/// What I-47a (`spec/image.md`) says of a record into a dropped copy of
/// a non-alloc group member (§5.8, WP-250).
enum Kept {
    /// Filled against the kept copy: K's place.
    To(u64),
    /// The tombstone: why the condition fails.
    Fallback(&'static str),
    /// K is live but has no whole place (merged or as pieces): no address
    /// to judge against.
    NotLocatable,
}

/// GNU ld's condition met (§5.8): the first member of the keeper's group
/// whose symbols match, with the size equal: its place.
enum GnuKept {
    To(u64),
    None,
}

impl Engine<'_, '_> {
    /// I-47a's condition for dead section `k` of object `oi` (§5.8): `k`
    /// has no `SHF_ALLOC` and is a member of a COMDAT copy the verifier's
    /// keeper drops (§5.7a's keeper, `spec/resolve.md` §6.1); the keeper's
    /// copy of the group has exactly one member K of `k`'s name, without
    /// `SHF_ALLOC`, of `k`'s `sh_size`; K is in the output. `None`: the
    /// rule does not apply (an allocated target, or not a dropped copy).
    fn i47a(&self, oi: usize, k: usize) -> Option<Kept> {
        let o = &self.objs[oi];
        let d = o.secs.get(k)?;
        if d.alloc() || !self.comdat_dropped_section(oi, k) {
            return None;
        }
        let (gi, sig, _) = crate::fate::comdat_group_of(o, k)?;
        let &(ko, kg) = self.keepers().get(&sig)?;
        if (ko, kg) == (oi, gi) {
            return None;
        }
        let keeper = &self.objs[ko];
        let same: Vec<usize> = crate::fate::group_members(keeper, kg)
            .into_iter()
            .filter(|&m| keeper.secs.get(m).is_some_and(|s| s.name == d.name))
            .collect();
        let sized: Vec<usize> =
            same.iter().copied().filter(|&m| keeper.secs[m].shdr.sh_size == d.shdr.sh_size).collect();
        let m = match (same.len(), sized.as_slice()) {
            (0, _) => return Some(Kept::Fallback("the keeper's copy has no member of its name")),
            (_, []) => return Some(Kept::Fallback("the keeper's member of its name has another size")),
            (_, [m]) => *m,
            _ => {
                return Some(Kept::Fallback(
                    "the keeper's copy has more than one member of its name and size",
                ));
            }
        };
        let kept = &keeper.secs[m];
        if kept.alloc() {
            return Some(Kept::Fallback("the keeper's member of its name and size is SHF_ALLOC"));
        }
        Some(match kept.place {
            Place::Whole(a) => Kept::To(a),
            Place::Absent | Place::Discarded => {
                Kept::Fallback("the keeper's member of its name and size is not in the output")
            }
            _ => Kept::NotLocatable,
        })
    }

    /// GNU ld's kept section for dead group section `k` of `oi` (§5.8,
    /// `spec/landed/image/WP-249.md` §2.1): in the keeper's copy of the
    /// group, in the group's order, the first member whose defined
    /// symbols match `k`'s (the same count, above 0, and the same names,
    /// `st_info` and `st_other`, sorted by name: a section symbol and
    /// AArch64's `$d` count), when its size is `k`'s; its name is not
    /// compared.
    fn gnu_kept(&self, oi: usize, k: usize) -> GnuKept {
        let o = &self.objs[oi];
        let Some(d) = o.secs.get(k) else { return GnuKept::None };
        if d.shdr.sh_flags & SHF_GROUP == 0 {
            return GnuKept::None;
        }
        let Some((gi, sig, _)) = crate::fate::comdat_group_of(o, k) else { return GnuKept::None };
        let Some(&(ko, kg)) = self.keepers().get(&sig) else { return GnuKept::None };
        if (ko, kg) == (oi, gi) {
            return GnuKept::None;
        }
        let mine = defined_in(o, k);
        if mine.is_empty() {
            return GnuKept::None;
        }
        let keeper = &self.objs[ko];
        let Some(m) = crate::fate::group_members(keeper, kg)
            .into_iter()
            .find(|&m| keeper.secs.get(m).is_some_and(|s| !s.consumed()) && defined_in(keeper, m) == mine)
        else {
            return GnuKept::None;
        };
        match keeper.secs[m].place {
            Place::Whole(a) if keeper.secs[m].shdr.sh_size == d.shdr.sh_size => GnuKept::To(a),
            _ => GnuKept::None,
        }
    }

    /// A record of non-alloc section `si` of `oi` whose target is not in
    /// the output. `after`: a record has its field right after this one's.
    #[allow(clippy::many_single_char_names)] // w, f, k, v: the field's width and values
    #[allow(clippy::too_many_lines)] // one column per linker
    pub(super) fn tombstone(
        &mut self,
        oi: usize,
        si: usize,
        rec: &Rec,
        site: &Site,
        symbol: &str,
        after: bool,
    ) {
        let name = self.objs[oi].secs[si].name.clone();
        let Field::Data(w) = rec.def.field else {
            let f = Self::finding(
                Rule::DiscardedTarget,
                rec,
                site,
                symbol,
                "an instruction field in a non-alloc section refers to a section not in the output",
            );
            return self.fail(f);
        };
        let ones = trunc(u64::MAX, w);
        let class64 = self.image.class() == Class::Elf64;
        let ranges = name == ".debug_ranges" || name == ".debug_loc";
        // DWARF 5 entries that pair an address with a length.
        let paired = matches!(name.as_str(), ".debug_rnglists" | ".debug_loclists" | ".debug_addr");
        let debug = is_debug_name(&name);
        // The dead section the symbol is in, the symbol's offset there,
        // and whether it is a section symbol or a local one.
        let dead = self.objs[oi].symtab.and_then(|t| {
            let s = t.get(SymbolIndex(rec.sym)).ok()?;
            match t.section_of(SymbolIndex(rec.sym), &s).ok()? {
                SymbolSection::Section(k) => Some((
                    k.0 as usize,
                    s.st_value,
                    s.binding() == STB_LOCAL || s.sym_type() == STT_SECTION,
                )),
                _ => None,
            }
        });
        // X of the record against the kept copy at `a` (as for a live
        // target, §5.1): `None` for an operation other than an absolute
        // or PC-relative word.
        let x_at = |a: u64, value: u64| -> Option<i64> {
            let sa = a.wrapping_add(value).wrapping_add(rec.addend as u64);
            match rec.def.op {
                Op::Abs => Some(sa as i64),
                Op::Prel => Some(sa.wrapping_sub(rec.p) as i64),
                _ => None,
            }
        };
        // spec/image.md I-47a: a debug reference into a dropped copy of a
        // non-alloc group member is filled against the kept copy.
        let mut fallback = None;
        if self.format == Format::Mortar
            && debug
            && let Some((k, value, true)) = dead
            && let Some(kept) = self.i47a(oi, k)
        {
            match kept {
                Kept::To(a) => return self.kept_copy(rec, site, symbol, x_at(a, value)),
                Kept::NotLocatable => return self.not_checked(Reason::SectionNotLocatable, 1),
                Kept::Fallback(why) => fallback = Some(why),
            }
        }
        let mut want: Vec<u64> = match self.format {
            // spec/image.md I-47.
            Format::Mortar if ranges => vec![1],
            Format::Mortar if paired => vec![0],
            Format::Mortar if name == ".debug_aranges" || name == ".debug_frame" => vec![ones],
            Format::Mortar if debug => vec![if class64 { ones } else { 0 }],
            Format::Mortar if name == ".gcc_except_table" => vec![0],
            Format::Mortar => Vec::new(),
            // lld: 1 where 0 would end a DWARF 4 list, else 0.
            Format::Lld => vec![u64::from(ranges)],
            // mold: the same, but 0 + A against a non-alloc target
            // (WP-250: a dropped `.debug_macro` copy plus 4 holds 4).
            Format::Mold => {
                let nonalloc =
                    dead.is_some_and(|(k, _, _)| self.objs[oi].secs.get(k).is_some_and(|s| !s.alloc()));
                vec![if ranges {
                    1
                } else if nonalloc {
                    rec.addend as u64
                } else {
                    0
                }]
            }
            // GNU ld: its kept section (`gnu_kept`) + st_value + A when it
            // has one, else 1 in `.debug_ranges` and 0 elsewhere
            // (`.debug_loc` too: observed, §10.6).
            Format::Gnu => {
                let kept = dead.and_then(|(k, value, _)| match self.gnu_kept(oi, k) {
                    GnuKept::To(a) => Some(x_at(a, value)),
                    GnuKept::None => None,
                });
                match kept {
                    Some(Some(x)) => return self.kept_copy(rec, site, symbol, Some(x)),
                    Some(None) => return self.not_checked(Reason::SectionNotLocatable, 1),
                    None => vec![u64::from(name == ".debug_ranges")],
                }
            }
        };
        for v in &mut want {
            *v = trunc(*v, w);
        }
        let got = trunc(rec.out_word, w);
        if !want.contains(&got) {
            let mut f = Self::finding(
                if want.is_empty() { Rule::DiscardedTarget } else { Rule::Field },
                rec,
                site,
                symbol,
                if want.is_empty() {
                    "a non-alloc section without a tombstone refers to a section not in the output"
                } else {
                    "the reference to a section not in the output holds another value than the tombstone"
                },
            );
            if let Some(why) = fallback {
                f.message.push_str(&format!(
                    "; spec/image.md I-47a keeps the tombstone here: {why}"
                ));
            }
            f.expected = want.first().copied();
            f.found = Some(got);
            return self.fail(f);
        }
        // I-112: mortar writes the aranges tuple's length 0.
        let alloc_target = dead.is_some_and(|(k, _, _)| {
            self.objs[oi].secs.get(k).is_some_and(super::super::locate::Sec::alloc)
        });
        if self.format == Format::Mortar
            && name == ".debug_aranges"
            && is_abs_addr(self.image.header.e_machine, rec.def.r_type)
            && alloc_target
            && !after
            && let Some(out) = self.image.nonalloc_section(&name)
        {
            let at = rec.p.wrapping_add(u64::from(w));
            match self.image.nonalloc_value(out, at, w) {
                Some(0) => {
                    for k in (0..u64::from(w)).step_by(4) {
                        self.nonalloc_rewritten.insert((out, at + k));
                    }
                }
                found => {
                    let mut f = Finding::new(
                        Rule::Field,
                        "the .debug_aranges tuple of a section not in the output has a length other than 0 (spec/image.md I-112)",
                    )
                    .at(at)
                    .site(site.clone());
                    f.reloc = Some(rec.def.name.to_string());
                    f.expected = Some(0);
                    f.found = found;
                    return self.fail(f);
                }
            }
        }
        self.cov.add_verified(Via::Tombstone, rec.def.r_type);
    }

    /// A record filled against a kept copy (§5.8: I-47a for mortar, GNU
    /// ld's redirect): X is that of a live target there (`None`: an
    /// operation §5.8 does not judge against a kept copy).
    fn kept_copy(&mut self, rec: &Rec, site: &Site, symbol: &str, x: Option<i64>) {
        let Some(x) = x else { return self.not_checked(Reason::SymbolUnresolved, 1) };
        let w = rec.def.field.width();
        if !rec.def.check.holds(x) {
            let f = Self::finding(
                Rule::Overflow,
                rec,
                site,
                symbol,
                "the kept copy's place does not fit the field (spec/image.md I-47a, I-43)",
            );
            return self.fail(f);
        }
        let want = trunc(x as u64, w);
        let got = trunc(rec.out_word, w);
        if want != got {
            let mut f = Self::finding(
                Rule::Field,
                rec,
                site,
                symbol,
                if self.format == Format::Mortar {
                    "a reference into a dropped COMDAT copy of a non-alloc group member does not hold the kept copy's place + st_value + A (spec/image.md I-47a)"
                } else {
                    "a reference into a dropped COMDAT copy does not hold the linker's kept section + st_value + A (spec/plumb.md §5.8)"
                },
            );
            if got == trunc(u64::MAX, w) || got == 0 || got == 1 {
                f.message.push_str("; it holds a tombstone");
            }
            f.expected = Some(want);
            f.found = Some(got);
            return self.fail(f);
        }
        self.cov.add_verified(Via::KeptCopy, rec.def.r_type);
    }
}

/// The symbols object `o` defines in section `k` (§5.8, GNU ld's match):
/// `(name, st_info, st_other)`, sorted.
fn defined_in(o: &crate::locate::Obj<'_>, k: usize) -> Vec<(Vec<u8>, u8, u8)> {
    let Some(t) = o.symtab else { return Vec::new() };
    let mut v: Vec<(Vec<u8>, u8, u8)> = t
        .iter()
        .filter(|(i, s)| matches!(t.section_of(*i, s), Ok(SymbolSection::Section(x)) if x.0 as usize == k))
        .map(|(_, s)| (t.name(&s).unwrap_or_default().to_vec(), s.st_info, s.st_other))
        .collect();
    v.sort();
    v
}

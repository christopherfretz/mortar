//! Non-alloc references to sections that are not in the output
//! (`spec/plumb.md` §5.8): the tombstone each linker writes.

use std::collections::HashMap;

use mortar_elf::consts::SHF_GROUP;
use mortar_elf::{Class, SymbolIndex, SymbolSection};

use super::{Engine, Rec, trunc};
use crate::a64::Field;
use crate::locate::Place;
use crate::map::Format;
use crate::report::{Finding, Rule, Site, Via};

/// GNU ld's debug names (`spec/resolve.md` §6.3).
pub(crate) fn is_debug_name(n: &str) -> bool {
    [".debug", ".zdebug", ".line", ".stab", ".gnu.linkonce.wi.", ".gnu.debuglto_.debug_"]
        .iter()
        .any(|p| n.starts_with(p))
        || n == ".gnu_debuglink"
}

/// Whether `r_type` is the absolute address-sized type of the machine
/// (I-112): `R_AARCH64_ABS64` or `R_X86_64_64`.
fn is_abs64(r_type: u32) -> bool {
    r_type == 257 || r_type == crate::x64::R64
}

impl Engine<'_, '_> {
    /// GNU ld's redirect targets for a dropped group section: the places of
    /// the placed group sections of its name and size.
    fn redirect_targets(&mut self, name: &str, size: u64) -> Vec<u64> {
        let objs = self.objs;
        let m = self.redirects.get_or_insert_with(|| {
            let mut m: HashMap<(String, u64), Vec<u64>> = HashMap::new();
            for o in objs {
                for s in &o.secs {
                    if s.shdr.sh_flags & SHF_GROUP != 0
                        && let Place::Whole(a) = s.place
                    {
                        m.entry((s.name.clone(), s.shdr.sh_size)).or_default().push(a);
                    }
                }
            }
            m
        });
        m.get(&(name.to_string(), size)).cloned().unwrap_or_default()
    }

    /// A record of non-alloc section `si` of `oi` whose target is not in
    /// the output. `after`: a record has its field right after this one's.
    #[allow(clippy::many_single_char_names)] // w, f, k, v: the field's width and values
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
        // The dead section the symbol is in, and the symbol's offset there.
        let dead = self.objs[oi].symtab.and_then(|t| {
            let s = t.get(SymbolIndex(rec.sym)).ok()?;
            match t.section_of(SymbolIndex(rec.sym), &s).ok()? {
                SymbolSection::Section(k) => Some((k.0 as usize, s.st_value)),
                _ => None,
            }
        });
        let mut want: Vec<u64> = match self.format {
            // spec/image.md I-47.
            Format::Mortar if ranges => vec![1],
            Format::Mortar if paired => vec![0],
            Format::Mortar if name == ".debug_aranges" || name == ".debug_frame" => vec![ones],
            Format::Mortar if debug => vec![if class64 { ones } else { 0 }],
            Format::Mortar if name == ".gcc_except_table" => vec![0],
            Format::Mortar => Vec::new(),
            // lld and mold: 1 where 0 would end a DWARF 4 list, else 0.
            Format::Lld | Format::Mold => vec![u64::from(ranges)],
            // GNU ld: the kept copy of a group section when the sizes
            // are equal, else 1 in `.debug_ranges` and 0 elsewhere
            // (`.debug_loc` too: observed, §10.6).
            Format::Gnu => {
                let mut v = vec![u64::from(name == ".debug_ranges")];
                if let Some((k, value)) = dead {
                    let s = &self.objs[oi].secs[k];
                    if s.shdr.sh_flags & SHF_GROUP != 0 {
                        let (n, z) = (s.name.clone(), s.shdr.sh_size);
                        v.extend(
                            self.redirect_targets(&n, z)
                                .into_iter()
                                .map(|a| a.wrapping_add(value).wrapping_add(rec.addend as u64)),
                        );
                    }
                }
                v
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
            f.expected = want.first().copied();
            f.found = Some(got);
            return self.fail(f);
        }
        // I-112: mortar writes the aranges tuple's length 0.
        let alloc_target = dead.is_some_and(|(k, _)| {
            self.objs[oi].secs.get(k).is_some_and(super::super::locate::Sec::alloc)
        });
        if self.format == Format::Mortar
            && name == ".debug_aranges"
            && is_abs64(rec.def.r_type)
            && class64
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
}

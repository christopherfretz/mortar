//! A non-alloc word against a symbol only a shared library defines
//! (`spec/plumb.md` §5.8, "A target only a shared library defines";
//! WP-104, the RB-1 defect). Nothing relocates a non-alloc section at run
//! time, so the field holds the symbol's link-time value: the copy's
//! address when the output's `.dynsym` defines the name, the canonical
//! PLT entry's address in an `ET_EXEC` that gives it one, else 0.
//!
//! The references' other conventions are named in the finding's message
//! (`spec/plumb.md` §10.2, A37–A43) so that [`crate::anomaly::classify`]
//! accepts each from its own linker only.

use mortar_elf::SymbolIndex;
use mortar_elf::consts::{
    ET_EXEC, PT_TLS, STB_GLOBAL, STB_GNU_UNIQUE, STB_WEAK, STT_SECTION, STT_TLS,
};

use super::{Engine, Rec, Verdict, judge};
use crate::a64::{Field, Op};
use crate::dynmodel::Model;
use crate::dynout::DynOut;
use crate::hasher::HashMap;
use crate::image::Image;
use crate::report::{Finding, Rule, Site, Via};

/// The message of a field this rule rejects, before the name's suffix.
pub const WRONG: &str = "the relocated field holds the wrong value (a word against";
/// The rule's citation, ending the first part of the message.
pub const CITE: &str = "which only a shared library defines, holds its link-time value (copy, canonical PLT entry or 0) + A: spec/plumb.md §5.8)";
/// What the field holds, when it is a reference linker's convention.
pub const HOLDS_PLT: &str = "; it holds the library's PLT entry";
pub const HOLDS_NO_ADDEND: &str = "; it holds 0, the addend dropped";
pub const HOLDS_ST_VALUE: &str = "; it holds the library's st_value";
pub const HOLDS_TLS_BASE: &str = "; it holds A − the PT_TLS address";
/// An undefined weak's PC-relative non-alloc word computed with S = 0
/// where §4.4 makes S the place (AArch64; lld, A43).
pub const WEAK_ZERO: &str =
    "; it holds A − P, S = 0 for an undefined weak (spec/plumb.md §4.4, §5.8)";
/// P-D12's message for a non-TLS data word against a shared `STT_TLS`
/// definition, before the name.
pub const TLS_WORD: &str = "a non-TLS data word in a non-alloc section against";
pub const TLS_WORD_END: &str =
    "whose shared definition is STT_TLS (refused by name: spec/a64.md §13.2, spec/arch.md §4.3)";

/// The definition of a name in the first shared object of the loader's
/// order that defines it visibly.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Shared {
    /// Its `st_value` in the library (anomaly evidence only).
    pub value: u64,
    pub tls: bool,
}

/// What the rule reads, from the model and the output.
#[derive(Debug, Default, Clone)]
pub(crate) struct DsoWords {
    pub shared: HashMap<String, Shared>,
    /// The names the output's `.dynsym` defines: a copy allocation
    /// (P-D11 holds every alias of the library's definition there).
    pub copy: HashMap<String, u64>,
    /// `ET_EXEC`: the names whose `.dynsym` entry is undefined with a
    /// non-zero `st_value` (a canonical PLT entry, dyn.md D-44).
    pub canon: HashMap<String, u64>,
    /// The PLT entries whose word a `JUMP_SLOT` or `GLOB_DAT` record of
    /// the name sets (anomaly evidence only).
    pub plt: HashMap<String, Vec<u64>>,
    /// The output's `PT_TLS` address (anomaly evidence only).
    pub tls_vaddr: Option<u64>,
}

/// Builds the rule's inputs. `order` is the loader's search list after
/// the executable (§12.1); `plt` the output's entries (entry → word).
pub(crate) fn build(
    image: &Image<'_>,
    dout: &DynOut,
    model: &Model,
    order: &[usize],
    plt: &std::collections::BTreeMap<u64, u64>,
) -> DsoWords {
    let mut w = DsoWords::default();
    for &i in order {
        let Some(d) = model.dsos.get(i) else { continue };
        for (name, defs) in &d.defs {
            if w.shared.contains_key(name) {
                continue;
            }
            let pick = defs
                .iter()
                .find(|x| x.shndx != 0 && x.visible() && x.default)
                .or_else(|| defs.iter().find(|x| x.shndx != 0 && x.visible()));
            if let Some(x) = pick {
                w.shared.insert(name.clone(), Shared { value: x.value, tls: x.ty == STT_TLS });
            }
        }
    }
    for s in dout.syms.iter().skip(1) {
        if s.defined() {
            w.copy.entry(s.name.clone()).or_insert(s.st_value);
        }
    }
    if image.header.e_type == ET_EXEC {
        for s in dout.syms.iter().skip(1) {
            if !s.defined() && s.st_value != 0 {
                w.canon.entry(s.name.clone()).or_insert(s.st_value);
            }
        }
    }
    let m = image.header.e_machine;
    for r in &dout.rels {
        if r.sym == 0
            || !matches!(
                crate::loader::kind(m, r.ty),
                crate::loader::Kind::JumpSlot | crate::loader::Kind::GlobDat
            )
        {
            continue;
        }
        let Some(s) = dout.syms.get(r.sym as usize) else { continue };
        for (&entry, &word) in plt {
            if word == r.offset {
                w.plt.entry(s.name.clone()).or_default().push(entry);
            }
        }
    }
    w.tls_vaddr = image.phdrs.iter().find(|p| p.p_type == PT_TLS).map(|p| p.p_vaddr);
    w
}

impl Engine<'_, '_> {
    /// Judges a non-alloc record by §5.8's rule when its symbol is a name
    /// only a shared library defines; `false` when the rule does not
    /// apply (the record goes to the general procedure).
    pub(super) fn dso_word(&mut self, oi: usize, rec: &Rec, site: &Site, symbol: &str) -> bool {
        let Some(dw) = self.dso_words.as_ref() else { return false };
        if !matches!(rec.def.field, Field::Data(_)) {
            return false;
        }
        let op = rec.def.op;
        if !matches!(op, Op::Abs | Op::Prel | Op::DtpRel) {
            return false;
        }
        let Some(symtab) = self.objs[oi].symtab else { return false };
        let Ok(sym) = symtab.get(SymbolIndex(rec.sym)) else { return false };
        if !matches!(sym.binding(), STB_GLOBAL | STB_WEAK | STB_GNU_UNIQUE)
            || sym.sym_type() == STT_SECTION
        {
            return false;
        }
        let name = String::from_utf8_lossy(symtab.name(&sym).unwrap_or(b"")).into_owned();
        let Some(&sh) = dw.shared.get(&name) else { return false };
        // Items 1 and 2: the output or an input defines it.
        if self.image.global(&name).is_some_and(|g| g.sym.st_shndx != 0)
            || self.chosen_def(&name).is_some()
        {
            return false;
        }
        if sh.tls != (op == Op::DtpRel) {
            if !sh.tls {
                // A DTP offset of a non-TLS symbol: the general procedure.
                return false;
            }
            let f = Self::finding(
                Rule::Dyn(12),
                rec,
                site,
                symbol,
                format!("{TLS_WORD} '{name}', {TLS_WORD_END}"),
            );
            self.fail(f);
            return true;
        }
        let a = rec.addend as u64;
        let s = if op == Op::DtpRel {
            0
        } else {
            dw.copy.get(&name).or_else(|| dw.canon.get(&name)).copied().unwrap_or(0)
        };
        let at = |v: u64| if op == Op::Prel { v.wrapping_sub(rec.p) } else { v } as i64;
        let x = at(s.wrapping_add(a));
        let verdict = judge(rec, x);
        if let Verdict::Ok(_) = verdict {
            self.cov.add_verified(Via::Direct, rec.def.r_type);
            return true;
        }
        let enc = |v: u64| rec.def.field.encode(rec.input_word, at(v));
        let out = rec.out_word;
        let holds = if op != Op::DtpRel
            && dw.plt.get(&name).is_some_and(|es| es.iter().any(|&e| enc(e.wrapping_add(a)) == out))
        {
            HOLDS_PLT
        } else if op != Op::Prel && a != 0 && out == rec.def.field.encode(rec.input_word, 0) {
            HOLDS_NO_ADDEND
        } else if op != Op::DtpRel && enc(sh.value.wrapping_add(a)) == out {
            HOLDS_ST_VALUE
        } else if op == Op::DtpRel && dw.tls_vaddr.is_some_and(|tv| enc(a.wrapping_sub(tv)) == out)
        {
            HOLDS_TLS_BASE
        } else {
            ""
        };
        let mut f = Self::finding(
            Rule::Field,
            rec,
            site,
            symbol,
            format!("{WRONG} '{name}', {CITE}{holds}"),
        );
        f.expected = Some(rec.def.field.encode(rec.input_word, x));
        f.found = Some(out);
        self.fail(f);
        true
    }
}

/// A finding of this rule, by what the field holds: the suffix after
/// [`CITE`], `None` for any other finding.
pub fn holds(f: &Finding) -> Option<&str> {
    if f.rule != Rule::Field || !f.message.starts_with(WRONG) {
        return None;
    }
    f.message.split_once(CITE).map(|(_, rest)| rest)
}

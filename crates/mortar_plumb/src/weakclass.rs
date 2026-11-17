//! `spec/dyn.md` D-25's class of each undefined weak key, derived once
//! from the inputs' live relocations (`spec/plumb.md` P-D7, P-D9; WP-165):
//! each use is a GOT use, an absolute word in a writable place, a branch
//! or an unfillable use by D-25's table (a type the table does not list
//! is unfillable), and the uses together give the key's verdict in the
//! output's kind. Every rule that needs the class asks [`derive`].
//!
//! The rules on it (WP-165, after WP-155's ruling (A)):
//! - a key D-25 makes `WeakDyn` (or, in `Exec`, `WeakDyn` for its
//!   branches: the split key) is in `.dynsym` as an undefined entry, and
//!   one with a branch has a `JUMP_SLOT` (P-D7 [`STRICT_DYN`],
//!   [`STRICT_SPLIT`]; P-D9 [`NO_SLOT`]); for mortar every such key, for
//!   a reference the keys whose only other uses are unfillable (a branch
//!   alone, or the split key: the cells WP-155 measured);
//! - each unfillable use of the split key holds `Weak0`'s value (view 1
//!   judges it) and has no dynamic record on its field ([`SPLIT_RECORD`]);
//!   view 3 does not expect it to reach a preloaded definition
//!   ([`Classes::split_site`]).

use std::collections::{BTreeMap, HashSet};

use mortar_elf::consts::{EM_386, EM_AARCH64, EM_X86_64, SHF_WRITE, STB_LOCAL};

use crate::dynout::DynOut;
use crate::image::Image;
use crate::locate::{Obj, Place};
use crate::map::{Format, OutputKind};
use crate::report::{Finding, Rule, Site};

/// D-25's class of one relocation against an undefined weak key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Class {
    Got,
    /// An absolute word in a writable place (the OUTPUT section's
    /// `SHF_WRITE`).
    AbsWritable,
    Branch,
    Unfillable,
    /// A TLS type: unfillable, and D-25 refuses the key by name.
    Tls,
    /// No use of the key (`NONE`, `SIZE*`, `GOTPC*`).
    NoUse,
}

/// The verdict of D-25 for a key in an output kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// `WEAK UND` in `.dynsym`; every branch through a `JUMP_SLOT`.
    WeakDyn,
    /// `Exec` only: `WeakDyn` for its branches; each unfillable use keeps
    /// `Weak0`'s value and has no record (ruling (A)).
    Split,
    /// 0 everywhere (each use by the architecture's `weak0` table).
    Weak0,
    /// Refused by name (a TLS use; an unfillable use in a `Pie`).
    Refused,
}

/// The uses of one key.
#[derive(Debug, Clone, Default)]
pub(crate) struct Uses {
    pub got: u32,
    pub abs_writable: u32,
    pub branch: u32,
    pub unfillable: u32,
    pub tls: u32,
    /// Each unfillable use: its input site, its type and its output place
    /// with the field's width (when the map places it).
    pub unfillable_at: Vec<(Site, u32, Option<(u64, u64)>)>,
}

impl Uses {
    /// D-25's "Then:" items, in order.
    pub fn verdict(&self, kind: OutputKind) -> Verdict {
        if !matches!(kind, OutputKind::Exec | OutputKind::Pie) {
            return Verdict::Weak0;
        }
        if self.tls > 0 {
            return Verdict::Refused;
        }
        let some = self.got + self.abs_writable + self.branch > 0;
        if some && self.unfillable == 0 {
            return Verdict::WeakDyn;
        }
        if kind == OutputKind::Exec
            && self.branch > 0
            && self.unfillable > 0
            && self.got + self.abs_writable == 0
        {
            return Verdict::Split;
        }
        if kind == OutputKind::Pie && self.unfillable > 0 {
            return Verdict::Refused;
        }
        Verdict::Weak0
    }

    /// Whether a reference linker's output is held to the import (WP-165):
    /// the key's uses besides its branches are unfillable or none (a
    /// branch alone; the split key), the cells WP-155 measured. A key
    /// with a GOT use or a writable absolute word is held only in
    /// mortar's output (dyn.md D-25 records mold's 0 everywhere for it,
    /// and GNU ld's x86-64 `.plt.got`).
    pub fn branch_cell(&self) -> bool {
        self.branch > 0 && self.got + self.abs_writable == 0
    }
}

/// The keys' uses, by name.
#[derive(Debug, Default)]
pub(crate) struct Classes {
    pub keys: BTreeMap<String, Uses>,
    pub kind: Option<OutputKind>,
}

impl Classes {
    /// Whether `site` is an unfillable use of a split key named `key`:
    /// view 3 expects `Weak0`'s value there, not the preloaded
    /// definition.
    pub fn split_site(&self, key: &str, site: Option<&Site>) -> bool {
        let (Some(kind), Some(site)) = (self.kind, site) else { return false };
        self.keys.get(key).is_some_and(|u| {
            u.verdict(kind) == Verdict::Split && u.unfillable_at.iter().any(|(s, _, _)| s == site)
        })
    }
}

/// D-25's class of relocation type `ty` of machine `machine`; `writable`
/// is whether the place's output section is writable, `branch_op`
/// whether the bytes before the place are a branch opcode (x86 `PC32`).
pub(crate) fn class(machine: u16, ty: u32, writable: bool, branch_op: bool) -> Class {
    let abs = |w| if w { Class::AbsWritable } else { Class::Unfillable };
    match machine {
        EM_AARCH64 => match ty {
            0 => Class::NoUse,
            309..=313 | 315 => Class::Got,
            257 => abs(writable),
            282 | 283 | 314 => Class::Branch,
            512..=573 | 1028..=1031 => Class::Tls,
            _ => Class::Unfillable,
        },
        EM_X86_64 => match ty {
            0 | 26 | 29 | 32 | 33 => Class::NoUse,
            9 | 27 | 41 | 42 => Class::Got,
            1 => abs(writable),
            4 | 31 => Class::Branch,
            2 if branch_op => Class::Branch,
            16..=23 | 34..=36 | 44 | 45 | 47 | 48 | 50 | 51 => Class::Tls,
            _ => Class::Unfillable,
        },
        EM_386 => match ty {
            0 | 10 | 38 => Class::NoUse,
            3 | 43 => Class::Got,
            1 => abs(writable),
            4 => Class::Branch,
            2 if branch_op => Class::Branch,
            14..=19 | 24..=37 | 39..=41 => Class::Tls,
            _ => Class::Unfillable,
        },
        _ => Class::Unfillable,
    }
}

/// D-25's branch opcode before an x86 `PC32` at `off` of `body`: `E8`
/// or `E9` at −1, or `0F 80`…`0F 8F` at −2, inside the section.
pub(crate) fn branch_opcode(body: &[u8], off: u64) -> bool {
    let Ok(o) = usize::try_from(off) else { return false };
    (o >= 1 && matches!(body.get(o - 1), Some(0xe8 | 0xe9)))
        || (o >= 2
            && body.get(o - 2) == Some(&0x0f)
            && body.get(o - 1).is_some_and(|b| (0x80..=0x8f).contains(b)))
}

/// The width of a relocation's field, for the record test of a split
/// key's unfillable use.
fn width(machine: u16, ty: u32) -> u64 {
    match (machine, ty) {
        (EM_AARCH64, 257 | 260 | 307) | (EM_X86_64, 1 | 24 | 25) => 8,
        (EM_AARCH64, 259 | 262) | (EM_X86_64 | EM_386, 12 | 13) | (EM_386, 20 | 21) => 2,
        (EM_X86_64, 14 | 15) | (EM_386, 22 | 23) => 1,
        _ => 4,
    }
}

/// The output address of offset `off` of a section placed at `place`.
fn placed_at(place: &Place, off: u64) -> Option<u64> {
    match place {
        Place::Whole(a) => Some(a + off),
        Place::Pieces(ps) => ps
            .iter()
            .find(|p| p.input_offset <= off && off < p.input_offset + p.size.max(1))
            .map(|p| p.addr + (off - p.input_offset)),
        _ => None,
    }
}

/// The uses of every key of `names` in the live allocated sections of
/// `objs` (a section the map does not place, or discards, has none: a
/// branch `--gc-sections` collects is no use).
pub(crate) fn derive(
    image: &Image<'_>,
    objs: &[Obj<'_>],
    names: &HashSet<String>,
    kind: OutputKind,
) -> Classes {
    let machine = image.header.e_machine;
    let mut keys: BTreeMap<String, Uses> = BTreeMap::new();
    for o in objs {
        let Some(t) = o.symtab else { continue };
        for (si, s) in o.secs.iter().enumerate() {
            let Some(rs) = s.relocs else { continue };
            if matches!(s.place, Place::Absent | Place::Discarded) || !s.alloc() {
                continue;
            }
            let Some(raw) = o.records(rs) else { continue };
            let mut body = None;
            for (off, sym, ty, _) in raw {
                let Some(name) = t
                    .get(mortar_elf::SymbolIndex(sym))
                    .ok()
                    .filter(|x| x.st_shndx == 0 && x.binding() != STB_LOCAL)
                    .and_then(|x| t.name(&x).ok())
                    .map(|n| String::from_utf8_lossy(n).into_owned())
                    .filter(|n| names.contains(n))
                else {
                    continue;
                };
                let at = placed_at(&s.place, off);
                // The place's output section; a section whose pieces the
                // map does not place is judged by the output section of
                // its name, then by its own flags.
                let writable = at
                    .and_then(|a| image.alloc_section_of(a, 1))
                    .map(|(_, x)| x.shdr.sh_flags)
                    .or_else(|| {
                        image.sections.iter().find(|x| x.name == s.name).map(|x| x.shdr.sh_flags)
                    })
                    .unwrap_or(s.shdr.sh_flags)
                    & SHF_WRITE
                    != 0;
                let branch_op = ty == 2
                    && matches!(machine, EM_X86_64 | EM_386)
                    && branch_opcode(body.get_or_insert_with(|| o.data(si)), off);
                let u = keys.entry(name).or_default();
                match class(machine, ty, writable, branch_op) {
                    Class::Got => u.got += 1,
                    Class::AbsWritable => u.abs_writable += 1,
                    Class::Branch => u.branch += 1,
                    Class::Unfillable => {
                        u.unfillable += 1;
                        u.unfillable_at.push((
                            o.site(si, off),
                            ty,
                            at.map(|a| (a, width(machine, ty))),
                        ));
                    }
                    Class::Tls => u.tls += 1,
                    Class::NoUse => {}
                }
            }
        }
    }
    Classes { keys, kind: Some(kind) }
}

/// P-D7's message end for a `WeakDyn` key not in `.dynsym`.
pub(crate) const STRICT_DYN: &str = "and is not in .dynsym (D-25: WeakDyn)";
/// P-D7's message end for the split key not in `.dynsym`.
pub(crate) const STRICT_SPLIT: &str =
    "and is not in .dynsym (D-25 ruling (A): WeakDyn for its branches)";
/// P-D9's message end for a branch-reached `WeakDyn` key without a
/// `JUMP_SLOT`.
pub(crate) const NO_SLOT: &str = "has no JUMP_SLOT record (D-25: every branch through a PLT entry)";
/// P-D9's message end for a record on a split key's unfillable use.
pub(crate) const SPLIT_RECORD: &str =
    "(D-25 ruling (A): an unfillable use of the split key keeps Weak0's value and has no record)";

/// The import rules on the classes (P-D7, P-D9).
pub(crate) fn check(c: &Classes, out: &DynOut, machine: u16, format: Format) -> Vec<Finding> {
    let mut v = Vec::new();
    let Some(kind) = c.kind else { return v };
    let slot = match machine {
        EM_AARCH64 => 1026,
        EM_X86_64 | EM_386 => 7,
        _ => u32::MAX,
    };
    for (k, u) in &c.keys {
        let verdict = u.verdict(kind);
        if !matches!(verdict, Verdict::WeakDyn | Verdict::Split) {
            continue;
        }
        let entry = out.syms.iter().enumerate().skip(1).find(|(_, s)| s.name == *k);
        let mut push = |n: u8, m: String, at: Option<u64>| {
            let mut f = Finding::new(Rule::Dyn(n), m);
            f.addr = at;
            f.symbol = Some(k.clone());
            v.push(f);
        };
        let held = format == Format::Mortar || u.branch_cell();
        let uses = describe(u);
        match entry {
            None if held => {
                let end = if verdict == Verdict::Split { STRICT_SPLIT } else { STRICT_DYN };
                push(7, format!("'{k}' is an undefined weak key reached by {uses} {end}"), None);
            }
            Some((j, s)) if !s.defined() && u.branch > 0 && held => {
                let slotted = out.rels.iter().any(|r| r.ty == slot && r.sym as usize == j);
                if !slotted {
                    push(9, format!("'{k}', an undefined weak key reached by {uses}, {NO_SLOT}"), None);
                }
            }
            _ => {}
        }
        if verdict == Verdict::Split {
            for (site, ty, at) in &u.unfillable_at {
                let Some((p, w)) = *at else { continue };
                for r in out.rels.iter().filter(|r| p <= r.offset && r.offset < p + w) {
                    let name = mortar_elf::reloc::name(machine, *ty)
                        .map_or_else(|| ty.to_string(), str::to_string);
                    let mut f = Finding::new(
                        Rule::Dyn(9),
                        format!(
                            "a dynamic record (type {}) at {:#x} on an unfillable use ({name}) of '{k}' {SPLIT_RECORD}",
                            r.ty, r.offset
                        ),
                    );
                    f.addr = Some(p);
                    f.site = Some(site.clone());
                    f.reloc = Some(name);
                    f.symbol = Some(k.clone());
                    v.push(f);
                }
            }
        }
    }
    v
}

/// "a branch", "a branch and a GOT use", ... for the messages.
fn describe(u: &Uses) -> String {
    let mut w = Vec::new();
    if u.branch > 0 {
        w.push("a branch");
    }
    if u.got > 0 {
        w.push("a GOT use");
    }
    if u.abs_writable > 0 {
        w.push("an absolute word in a writable place");
    }
    if u.unfillable > 0 {
        w.push("an unfillable use");
    }
    let mut s = w.join(" and ");
    if u.unfillable == 0 {
        s.push_str(" and no unfillable use");
    }
    s.push(',');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uses(got: u32, abs: u32, branch: u32, unfill: u32) -> Uses {
        Uses { got, abs_writable: abs, branch, unfillable: unfill, ..Uses::default() }
    }

    // spec: dyn.md D-25 — the "Then:" items: a branch alone is WeakDyn
    // (WP-155); a branch and an unfillable use is the split key in Exec
    // and refused in Pie; a GOT use or a writable word beside an
    // unfillable use is Weak0 in Exec (W12).
    #[test]
    fn d25_verdicts() {
        use OutputKind::{Exec, Pie, Shared, Static};
        assert_eq!(uses(0, 0, 1, 0).verdict(Exec), Verdict::WeakDyn);
        assert_eq!(uses(0, 0, 1, 0).verdict(Pie), Verdict::WeakDyn);
        assert_eq!(uses(1, 0, 0, 0).verdict(Pie), Verdict::WeakDyn);
        assert_eq!(uses(0, 1, 0, 0).verdict(Exec), Verdict::WeakDyn);
        assert_eq!(uses(0, 0, 1, 1).verdict(Exec), Verdict::Split);
        assert_eq!(uses(0, 0, 1, 1).verdict(Pie), Verdict::Refused);
        assert_eq!(uses(1, 0, 1, 1).verdict(Exec), Verdict::Weak0);
        assert_eq!(uses(0, 1, 0, 1).verdict(Exec), Verdict::Weak0);
        assert_eq!(uses(0, 0, 0, 1).verdict(Exec), Verdict::Weak0);
        assert_eq!(uses(0, 0, 0, 1).verdict(Pie), Verdict::Refused);
        assert_eq!(uses(0, 0, 0, 0).verdict(Exec), Verdict::Weak0);
        assert_eq!(uses(0, 0, 1, 0).verdict(Static), Verdict::Weak0);
        assert_eq!(uses(0, 0, 1, 0).verdict(Shared), Verdict::Weak0);
        let mut t = uses(1, 0, 0, 0);
        t.tls = 1;
        assert_eq!(t.verdict(Exec), Verdict::Refused);
        assert!(uses(0, 0, 1, 1).branch_cell());
        assert!(!uses(1, 0, 1, 0).branch_cell());
    }

    // spec: dyn.md D-25 — the class table's columns, the writable place
    // and the branch-opcode test of x86 `PC32`.
    #[test]
    fn d25_class_table() {
        assert_eq!(class(EM_AARCH64, 283, false, false), Class::Branch);
        assert_eq!(class(EM_AARCH64, 280, false, false), Class::Unfillable); // CONDBR19
        assert_eq!(class(EM_AARCH64, 257, true, false), Class::AbsWritable);
        assert_eq!(class(EM_AARCH64, 257, false, false), Class::Unfillable);
        assert_eq!(class(EM_AARCH64, 311, false, false), Class::Got);
        assert_eq!(class(EM_AARCH64, 275, false, false), Class::Unfillable);
        assert_eq!(class(EM_AARCH64, 541, false, false), Class::Tls);
        assert_eq!(class(EM_X86_64, 2, false, true), Class::Branch);
        assert_eq!(class(EM_X86_64, 2, false, false), Class::Unfillable);
        assert_eq!(class(EM_X86_64, 10, true, false), Class::Unfillable);
        assert_eq!(class(EM_X86_64, 42, false, false), Class::Got);
        assert_eq!(class(EM_X86_64, 33, false, false), Class::NoUse);
        assert_eq!(class(EM_386, 43, false, false), Class::Got);
        assert_eq!(class(EM_386, 1, false, false), Class::Unfillable);
        assert_eq!(class(EM_386, 10, false, false), Class::NoUse);
        assert!(branch_opcode(&[0xe8, 0, 0, 0, 0], 1));
        assert!(branch_opcode(&[0x0f, 0x84, 0, 0, 0, 0], 2));
        assert!(!branch_opcode(&[0x8b, 0x05, 0, 0, 0, 0], 2));
        assert!(!branch_opcode(&[0, 0, 0, 0], 0));
    }
}

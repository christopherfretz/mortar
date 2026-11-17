//! P-P4, the output's `.symtab` at `-r` (`spec/plumb.md` §14.5).
//! Owner: **RL-5**.
//!
//! The locals come first and `sh_info` is the first non-local; every
//! key of the inputs (a global, weak, unique or common name, or an
//! undefined one) is in the output once, with its merged binding and
//! visibility, at the place of its definition; a local is an input's
//! local at the same place; no linker-defined symbol is added (RV-R8).

use std::collections::{BTreeMap, BTreeSet};

use crate::map::Format;
use crate::relocatable::{
    Place, Rv, SHN_ABS, SHN_COMMON, SHN_UNDEF, SHT_GROUP, SHT_NOBITS, SHT_REL, SHT_RELA, SHT_STRTAB,
    SHT_SYMTAB, SHT_SYMTAB_SHNDX, STB_GLOBAL, STB_GNU_UNIQUE, STB_WEAK, STT_COMMON, STT_FILE,
    STT_OBJECT, STT_SECTION, Sym, Tgt,
};
use crate::report::{Finding, Rule};

/// The merged view of one key over the inputs.
#[derive(Debug, Default)]
struct Key {
    /// First mention: (FileId, symbol index) — the `SymbolId` order.
    first: (usize, usize),
    /// The definition that wins: (file, symbol), strong before weak.
    def: Option<(usize, usize)>,
    abs: Option<i64>,
    unique: bool,
    common: Option<(u64, u64)>,
    /// Every live undefined mention is weak.
    all_weak_refs: bool,
    any_ref: bool,
    vis: u8,
}

/// The more constraining of two `STV_*` (internal, hidden, protected,
/// default).
fn tighter(a: u8, b: u8) -> u8 {
    let rank = |v: u8| match v {
        1 => 0,
        2 => 1,
        3 => 2,
        _ => 3,
    };
    if rank(a) <= rank(b) { a } else { b }
}

fn vis_name(v: u8) -> &'static str {
    match v {
        1 => "INTERNAL",
        2 => "HIDDEN",
        3 => "PROTECTED",
        _ => "DEFAULT",
    }
}

/// Whether an input symbol's section is live.
fn live_section(rv: &Rv<'_>, f: usize, s: &Sym) -> bool {
    !matches!(rv.places[f].get(s.shndx as usize), Some(Place::Dropped(_)) | None)
}

#[allow(clippy::too_many_lines)] // the keys, the table, the locals, mortar's order
pub(crate) fn check(rv: &mut Rv<'_>) {
    let mut msgs: Vec<String> = Vec::new();
    let mut accepted = 0u64;
    // The keys.
    let mut keys: BTreeMap<String, Key> = BTreeMap::new();
    for f in 0..rv.ins.len() {
        let e = &rv.ins[f].elf;
        for (i, s) in e.syms.iter().enumerate().skip(1) {
            if s.local() || s.ty() == STT_FILE {
                continue;
            }
            let k = keys.entry(s.name.clone()).or_insert_with(|| Key {
                first: (f, i),
                all_weak_refs: true,
                vis: 0,
                ..Key::default()
            });
            k.vis = tighter(k.vis, s.vis());
            if s.shndx == SHN_UNDEF {
                k.any_ref = true;
                k.all_weak_refs &= s.bind() == STB_WEAK;
            } else if s.shndx == SHN_COMMON {
                let (sz, al) = k.common.unwrap_or((0, 0));
                k.common = Some((sz.max(s.size), al.max(s.value)));
            } else if s.shndx == SHN_ABS {
                if k.def.is_none() && k.abs.is_none() {
                    k.abs = Some(s.value as i64);
                }
            } else if live_section(rv, f, s) {
                k.unique |= s.bind() == STB_GNU_UNIQUE;
                let strong = s.bind() != STB_WEAK;
                match k.def {
                    None => k.def = Some((f, i)),
                    Some((g, j)) if strong && rv.ins[g].elf.syms[j].bind() == STB_WEAK => k.def = Some((f, i)),
                    _ => {}
                }
            }
        }
    }
    for u in &rv.opts.undefined {
        keys.entry(u.clone()).or_insert_with(|| Key {
            first: (usize::MAX, 0),
            all_weak_refs: false,
            any_ref: true,
            ..Key::default()
        });
    }
    // The table's shape.
    let out = &rv.out;
    let first_global = out.first_global();
    let syms = out.syms.clone();
    if out.symtab.is_none() {
        if !keys.is_empty() {
            msgs.push("the output has no .symtab".into());
        }
    } else {
        for (i, s) in syms.iter().enumerate() {
            if (i < first_global) != s.local() {
                msgs.push(format!(
                    "symbol {i} {} is {} and .symtab's sh_info is {first_global} (the locals come first)",
                    s.name,
                    if s.local() { "local" } else { "not local" }
                ));
                break;
            }
        }
    }
    let nsec = out.secs.len() as u32;
    // .symtab_shndx.
    if let Some(x) = out.shndx_sec {
        if out.secs[x].size != syms.len() as u64 * 4 {
            msgs.push(format!(".symtab_shndx has {:#x} bytes for {} symbols", out.secs[x].size, syms.len()));
        }
    }
    for (i, s) in syms.iter().enumerate() {
        let needs = s.shndx >= 0xff00 && !matches!(s.shndx, SHN_ABS | SHN_COMMON);
        if needs != (s.raw_shndx == 0xffff) && s.shndx < nsec.max(0xff00) {
            msgs.push(format!("symbol {i} {}: st_shndx and .symtab_shndx disagree", s.name));
        }
        if s.ty() == STT_SECTION && (s.value != 0 || s.shndx == 0 || s.shndx >= nsec) {
            msgs.push(format!("section symbol {i} has value {:#x} and section {}", s.value, s.shndx));
        }
        if s.in_section() && s.shndx >= nsec {
            msgs.push(format!("symbol {i} {} is in section {}, which does not exist", s.name, s.shndx));
        }
    }
    // The globals: every key once.
    let mut by_name: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, s) in syms.iter().enumerate().skip(1) {
        if !s.local() {
            by_name.entry(s.name.as_str()).or_default().push(i);
        }
    }
    let mut judged = 0u64;
    for (name, k) in &keys {
        let hits = by_name.get(name.as_str()).cloned().unwrap_or_default();
        let defined = k.def.is_some() || k.abs.is_some() || k.common.is_some();
        if hits.is_empty() {
            // lld and mold omit a -u name nobody defines (§14.5).
            let only_u = k.first.0 == usize::MAX;
            if only_u && matches!(rv.format, Format::Lld | Format::Mold) {
                accepted += 1;
                continue;
            }
            // lld binds a plain undefined `foo` to a definition `foo@@V`
            // and writes no `UND foo` (RV-R6's lld form, §14.5 P-P3/P-P4).
            if !defined
                && rv.format == Format::Lld
                && keys.iter().any(|(n, d)| d.def.is_some() && n.strip_prefix(name.as_str()).is_some_and(|v| v.starts_with("@@")))
            {
                accepted += 1;
                continue;
            }
            // A key in a local of the output: hidden made local.
            if syms.iter().any(|s| s.local() && &s.name == name) && defined {
                msgs.push(format!("{name} is local; a hidden key stays global at -r (RV-R6)"));
            } else {
                msgs.push(format!("{name} is not in .symtab"));
            }
            continue;
        }
        if hits.len() > 1 {
            msgs.push(format!("{name} is in .symtab {} times", hits.len()));
        }
        judged += 1;
        let o = &syms[hits[0]];
        if o.vis() != k.vis {
            msgs.push(format!("{name} is {}, not {} (the merged visibility)", vis_name(o.vis()), vis_name(k.vis)));
        }
        if let Some((f, i)) = k.def {
            let d = &rv.ins[f].elf.syms[i];
            let want_bind = if k.unique { STB_GNU_UNIQUE } else { d.bind() };
            if o.bind() != want_bind {
                msgs.push(format!("{name} has binding {}, not {want_bind}", o.bind()));
            }
            if o.ty() != d.ty() {
                msgs.push(format!("{name} has type {}, not {}", o.ty(), d.ty()));
            }
            if o.size != d.size {
                msgs.push(format!("{name} has size {:#x}, not {:#x}", o.size, d.size));
            }
            match rv.at_place(f, d.shndx as usize, d.value as i64) {
                Tgt::Place(out, v) => {
                    if o.shndx != out || o.value as i64 != v {
                        msgs.push(format!("{name} is at {}+{:#x}, not {out}+{v:#x}", o.shndx, o.value));
                    }
                }
                Tgt::Merged(out, ..) if o.shndx == out => {}
                t => msgs.push(format!("{name} is at {}+{:#x}, not at {t}", o.shndx, o.value)),
            }
        } else if let Some(v) = k.abs {
            if o.shndx != SHN_ABS || o.value as i64 != v {
                msgs.push(format!("{name} is at {}+{:#x}, not ABS {v:#x}", o.shndx, o.value));
            }
        } else if let Some((size, align)) = k.common {
            let lld_d = rv.format == Format::Lld && rv.opts.define_common;
            if !rv.opts.define_common || lld_d || (rv.format == Format::Mold && rv.opts.define_common) {
                if o.shndx == SHN_COMMON && (lld_d || rv.opts.define_common) {
                    accepted += 1;
                }
                if o.shndx != SHN_COMMON {
                    msgs.push(format!("common {name} is {}, not SHN_COMMON", shn(o.shndx)));
                } else if o.size != size || o.value != align {
                    msgs.push(format!(
                        "common {name} has size {:#x} and alignment {:#x}, not the largest {size:#x} and {align:#x}",
                        o.size, o.value
                    ));
                }
            } else {
                let bss = rv.out.secs.get(o.shndx as usize);
                if !bss.is_some_and(|s| s.ty == SHT_NOBITS) || o.ty() != STT_OBJECT || o.size != size || (align > 1 && o.value % align != 0) {
                    msgs.push(format!(
                        "common {name} under -d is {} type {} size {:#x} at {:#x}, not an OBJECT of size {size:#x} in a NOBITS section aligned {align:#x}",
                        shn(o.shndx), o.ty(), o.size, o.value
                    ));
                } else if let Some(rc) = rv.map.rel.as_ref().and_then(|r| r.commons.iter().find(|c| &c.name == name)) {
                    if (rc.out, rc.offset, rc.size) != (o.shndx, o.value, o.size) {
                        msgs.push(format!("common {name} is at {}+{:#x}; rcommon says {}+{:#x}", o.shndx, o.value, rc.out, rc.offset));
                    }
                }
            }
        } else {
            if o.shndx != SHN_UNDEF {
                msgs.push(format!("{name} is defined in {}; no live input defines it", shn(o.shndx)));
            }
            let want_weak = k.any_ref && k.all_weak_refs;
            if (o.bind() == STB_WEAK) != want_weak {
                msgs.push(format!(
                    "undefined {name} is {}, not {} (WEAK iff every live mention is weak)",
                    if o.bind() == STB_WEAK { "WEAK" } else { "GLOBAL" },
                    if want_weak { "WEAK" } else { "GLOBAL" }
                ));
            }
        }
        let _ = defined;
    }
    // Globals that are no key: a linker-defined symbol (RV-R8).
    for (name, idx) in &by_name {
        if !keys.contains_key(*name) {
            for &i in idx {
                msgs.push(format!("{name} (symbol {i}) is no input's symbol: -r defines nothing (RV-R8)"));
            }
        }
    }
    // The locals: an input's local at the same place.
    let mut input_locals: BTreeMap<&str, Vec<Tgt>> = BTreeMap::new();
    for f in 0..rv.ins.len() {
        for s in rv.ins[f].elf.syms.iter().skip(1) {
            if !s.local() || matches!(s.ty(), STT_SECTION | STT_FILE) {
                continue;
            }
            let in_group = rv.ins[f].elf.secs.get(s.shndx as usize).is_some_and(|x| x.ty == SHT_GROUP);
            let t = if s.shndx == SHN_ABS {
                Tgt::Abs(s.value as i64)
            } else if in_group {
                // A local of a group section (gas's signature symbol):
                // in the output group of that group.
                match rv.group_out.get(&(f, s.shndx as usize)) {
                    Some(&o) => Tgt::Place(o, s.value as i64),
                    None => continue,
                }
            } else if s.in_section() {
                rv.at_place(f, s.shndx as usize, s.value as i64)
            } else {
                continue;
            };
            input_locals.entry(s.name.as_str()).or_default().push(t);
        }
    }
    let mut locals = 0u64;
    let mut unjudged = 0u64;
    for (i, s) in syms.iter().enumerate().skip(1) {
        if !s.local() || matches!(s.ty(), STT_SECTION | STT_FILE) {
            continue;
        }
        // A mapping symbol of a non-executable section marks data and
        // code for disassemblers; it is not judged for a reference
        // (§14.5; mortar's are, `.eh_frame`'s through the record map).
        let mapping = matches!(s.name.as_str(), "$x" | "$d" | "$t" | "$a")
            || s.name.starts_with("$x.")
            || s.name.starts_with("$d.");
        let exec = rv.out.secs.get(s.shndx as usize).is_some_and(|x| x.flags & crate::relocatable::SHF_EXECINSTR != 0);
        if mapping && !(s.in_section() && exec) && !rv.mortar() {
            unjudged += 1;
            continue;
        }
        locals += 1;
        let ot = if s.shndx == SHN_ABS { Tgt::Abs(s.value as i64) } else { Tgt::Place(s.shndx, s.value as i64) };
        let ok = input_locals.get(s.name.as_str()).is_some_and(|v| {
            v.iter().any(|t| match (t, &ot) {
                (Tgt::Merged(o, ..), Tgt::Place(p, _)) => o == p,
                _ => rv.equiv(t, &ot).is_some(),
            })
        });
        // A key made local (hidden at -r) is reported above.
        if !ok && !keys.contains_key(&s.name) {
            msgs.push(format!("local {} at {}+{:#x} is no input's local there (symbol {i})", s.name, s.shndx, s.value));
        }
    }
    if rv.mortar() {
        mortar_order(rv, &keys, &mut msgs);
    }
    rv.cov.symbols = judged + locals;
    rv.report.coverage.globals_checked += judged;
    rv.report.coverage.locals_checked += locals;
    rv.report.coverage.locals_unchecked += unjudged;
    // A per-linker symbol form is a symbol judged, not a relocation
    // record: §7.2's sum is over records.
    rv.report.coverage.globals_checked += accepted;
    for m in msgs {
        rv.push(Finding::new(Rule::Rel(4), m));
    }
    let _ = (STB_GLOBAL, STT_COMMON);
}

fn shn(x: u32) -> String {
    match x {
        SHN_UNDEF => "SHN_UNDEF".into(),
        SHN_ABS => "SHN_ABS".into(),
        SHN_COMMON => "SHN_COMMON".into(),
        x => format!("section {x}"),
    }
}

/// I-R4's order (mortar): the null entry; a section symbol per section
/// (but groups, relocation sections and the tables); per file its
/// `STT_FILE` entries and kept locals; the globals in `SymbolId` order.
fn mortar_order(rv: &Rv<'_>, keys: &BTreeMap<String, Key>, msgs: &mut Vec<String>) {
    let out = &rv.out;
    let want_secs: Vec<u32> = (1..out.secs.len())
        .filter(|&o| {
            let s = &out.secs[o];
            !matches!(s.ty, SHT_GROUP | SHT_REL | SHT_RELA | SHT_SYMTAB | SHT_SYMTAB_SHNDX | SHT_STRTAB)
        })
        .map(|o| o as u32)
        .collect();
    let got_secs: Vec<u32> = out
        .syms
        .iter()
        .skip(1)
        .take_while(|s| s.ty() == STT_SECTION)
        .map(|s| s.shndx)
        .collect();
    if got_secs != want_secs {
        msgs.push(format!(
            "the section symbols are for sections {got_secs:?}; I-R4 gives one per section in order: {want_secs:?} (mortar)"
        ));
    }
    let after = 1 + got_secs.len();
    let fg = out.first_global();
    if out.syms[after.min(out.syms.len())..fg.min(out.syms.len())].iter().any(|s| s.ty() == STT_SECTION) {
        msgs.push("a section symbol follows the file locals (I-R4) (mortar)".into());
    }
    let mut order: Vec<(&(usize, usize), &str)> = keys.iter().map(|(n, k)| (&k.first, n.as_str())).collect();
    order.sort();
    let want: Vec<&str> = order.into_iter().map(|(_, n)| n).collect();
    let got: Vec<&str> = out.syms.iter().skip(fg).map(|s| s.name.as_str()).collect();
    let want: Vec<&str> = want.into_iter().filter(|n| got.contains(n)).collect();
    if let Some(i) = got.iter().zip(&want).position(|(a, b)| a != b) {
        let _: BTreeSet<()> = BTreeSet::new();
        msgs.push(format!(
            "global {} is symbol {}; I-R4's SymbolId order puts {} there (mortar)",
            got[i],
            fg + i,
            want[i]
        ));
    }
}

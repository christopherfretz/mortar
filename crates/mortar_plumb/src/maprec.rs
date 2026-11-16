//! The allocation records of mortar's map (`spec/plumb.md` §3.2, §3.3
//! M6-M8; WP-096): facts of the layout the linker states and the
//! verifier checks against the output instead of inferring them.
//!
//! - **M6**: a `synthetic` chunk in an allocated `NOBITS` section (the
//!   linker-made `.pad.relro`) is that whole section;
//! - **M7**: a `common` record is a common symbol of the inputs, at the
//!   output's `.symtab` value and size, aligned as its definitions ask;
//! - **M8**: a `copy` record is a `COPY` record of the output, with the
//!   size of its symbol and the alignment of the copied definition.
//!
//! These run for every map of mortar's format, whatever it claims.

use std::collections::{BTreeMap, BTreeSet};

use mortar_elf::SymbolSection;
use mortar_elf::consts::{
    SHF_TLS, SHN_COMMON, SHN_LORESERVE, SHN_UNDEF, SHN_X86_64_LCOMMON, SHT_NOBITS, STB_GLOBAL,
    STB_WEAK,
};

use crate::image::Image;
use crate::locate::{Obj, Place};
use crate::map::{ChunkKind, Map};
use crate::report::{Finding, Rule};

fn finding(n: u8, msg: String) -> Finding {
    Finding::new(Rule::Map(n), msg)
}

/// The names the inputs give only common definitions (`SHN_COMMON`, or
/// x86-64 `SHN_X86_64_LCOMMON`, global or weak; no definition in a
/// section the map places), each with the largest alignment its common
/// definitions ask (their `st_value`, gABI "Symbol Values").
pub(crate) fn common_only(objs: &[Obj<'_>]) -> BTreeMap<String, u64> {
    let mut common: BTreeMap<String, u64> = BTreeMap::new();
    let mut defined: BTreeSet<String> = BTreeSet::new();
    for o in objs {
        let Some(t) = &o.symtab else { continue };
        for (i, sym) in t.iter() {
            if !matches!(sym.binding(), STB_GLOBAL | STB_WEAK) {
                continue;
            }
            let Ok(name) = t.name(&sym) else { continue };
            let name = String::from_utf8_lossy(name).into_owned();
            if sym.st_shndx == SHN_COMMON || sym.st_shndx == SHN_X86_64_LCOMMON {
                let a = common.entry(name).or_insert(1);
                *a = (*a).max(sym.st_value.max(1));
            } else if let Ok(SymbolSection::Section(k)) = t.section_of(i, &sym)
                && o.secs
                    .get(k.0 as usize)
                    .is_some_and(|s| matches!(s.place, Place::Whole(_) | Place::Pieces(_)))
            {
                defined.insert(name);
            }
        }
    }
    common.retain(|n, _| !defined.contains(n));
    common
}

/// M6 and M7 over a map of mortar's format.
pub(crate) fn check(image: &Image<'_>, objs: &[Obj<'_>], map: &Map) -> Vec<Finding> {
    let mut out = Vec::new();
    nobits_chunks(image, map, &mut out);
    commons(image, objs, map, &mut out);
    out
}

/// M6: a synthetic chunk in an allocated, not thread-local `NOBITS`
/// section is that section.
fn nobits_chunks(image: &Image<'_>, map: &Map, out: &mut Vec<Finding>) {
    for c in &map.chunks {
        let (ChunkKind::Synthetic, Some(a)) = (c.kind, c.addr) else { continue };
        let Some(s) = image.sections.iter().skip(1).find(|s| {
            let h = &s.shdr;
            s.alloc()
                && h.sh_type == SHT_NOBITS
                && h.sh_flags & SHF_TLS == 0
                && h.sh_size > 0
                && h.sh_addr <= a
                && a < h.sh_addr.saturating_add(h.sh_size)
        }) else {
            continue;
        };
        let h = &s.shdr;
        if c.name != s.name || a != h.sh_addr || c.size != h.sh_size {
            out.push(
                finding(
                    6,
                    format!(
                        "map line {}: the chunk '{}' [{a:#x}, +{:#x}) lies in the NOBITS section '{}' [{:#x}, +{:#x}) and is not that section",
                        c.line, c.name, c.size, s.name, h.sh_addr, h.sh_size
                    ),
                )
                .at(a),
            );
        }
    }
}

/// M7: each `common` record is a common symbol of the inputs, placed
/// where the output's `.symtab` says, aligned as its definitions ask.
fn commons(image: &Image<'_>, objs: &[Obj<'_>], map: &Map, out: &mut Vec<Finding>) {
    let only = common_only(objs);
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for r in &map.commons {
        let say = |msg: String| {
            finding(7, format!("map line {}: the common '{}' {msg}", r.line, r.name)).at(r.addr)
        };
        if !seen.insert(r.name.as_str()) {
            out.push(say("has a second record".into()));
            continue;
        }
        let Some(&want) = only.get(&r.name) else {
            out.push(say("is not a name the inputs give only common definitions".into()));
            continue;
        };
        if r.align != want {
            let mut f = say(format!(
                "is stated aligned to {:#x}; its common definitions ask {want:#x}",
                r.align
            ));
            f.expected = Some(want);
            f.found = Some(r.align);
            out.push(f);
        }
        if !r.addr.is_multiple_of(r.align) {
            out.push(say(format!(
                "at {:#x} is not a multiple of its alignment {:#x}",
                r.addr, r.align
            )));
        }
        // A stripped output: the address and size are not judged.
        if image.symbols.is_empty() {
            continue;
        }
        let Some(sym) = image.global(&r.name).map(|s| s.sym) else {
            out.push(say("is not a global symbol of the output's .symtab".into()));
            continue;
        };
        if sym.st_shndx == SHN_UNDEF || sym.st_shndx >= SHN_LORESERVE {
            out.push(say(format!(
                "is not defined in a section of the output (st_shndx {:#x})",
                sym.st_shndx
            )));
            continue;
        }
        for (what, stated, found) in
            [("address", r.addr, sym.st_value), ("size", r.size, sym.st_size)]
        {
            if stated != found {
                let mut f = say(format!(
                    "is stated at {what} {stated:#x}; the output's symbol has {found:#x}"
                ));
                f.expected = Some(found);
                f.found = Some(stated);
                out.push(f);
            }
        }
        let held = image.sections.get(usize::from(sym.st_shndx)).is_some_and(|s| {
            let h = &s.shdr;
            s.alloc()
                && h.sh_type == SHT_NOBITS
                && h.sh_addr <= r.addr
                && r.addr
                    .checked_add(r.size)
                    .is_some_and(|e| e <= h.sh_addr.saturating_add(h.sh_size))
        });
        if !held {
            out.push(say("is not inside the allocated NOBITS section its symbol names".into()));
        }
    }
}

/// A `COPY` record of the output (P-D11): its place, its symbol's name
/// and `.dynsym` size, and the copied definition's alignment when its
/// definition was found.
#[derive(Debug, Clone)]
pub(crate) struct ActualCopy {
    pub addr: u64,
    pub name: String,
    pub size: u64,
    pub align: Option<u64>,
}

/// M8: each `copy` record is a `COPY` record of the output. `actual` is
/// empty for an output with no dynamic section.
pub(crate) fn copies(map: &Map, actual: &[ActualCopy]) -> Vec<Finding> {
    let mut out = Vec::new();
    let stated = map.dynamic.iter().flat_map(|d| &d.copies);
    let mut seen: BTreeSet<u64> = BTreeSet::new();
    for c in stated {
        let say = |msg: String| {
            finding(8, format!("the copy record of '{}' at {:#x} {msg}", c.name, c.addr)).at(c.addr)
        };
        if !seen.insert(c.addr) {
            out.push(say("is the second record of that address".into()));
            continue;
        }
        let Some(a) = actual.iter().find(|a| a.addr == c.addr) else {
            out.push(say("names no COPY record of the output".into()));
            continue;
        };
        if a.name != c.name {
            out.push(say(format!("names '{}'; the COPY record there is '{}'s", c.name, a.name)));
        }
        if a.size != c.size {
            let mut f = say(format!(
                "is stated of size {:#x}; the COPY record's symbol has {:#x}",
                c.size, a.size
            ));
            f.expected = Some(a.size);
            f.found = Some(c.size);
            out.push(f);
        }
        let Some(stated) = c.align else { continue };
        if let Some(want) = a.align
            && stated != want
        {
            let mut f = say(format!(
                "is stated aligned to {stated:#x}; the copied definition's alignment is {want:#x} (P-D11, D-42)"
            ));
            f.expected = Some(want);
            f.found = Some(stated);
            out.push(f);
        }
        if !c.addr.is_multiple_of(stated) {
            out.push(say(format!("is not a multiple of its alignment {stated:#x}")));
        }
    }
    out
}

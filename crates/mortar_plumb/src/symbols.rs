//! The symbol table (`spec/plumb.md` §6.5): `.symtab`, `.strtab` and
//! `.shstrtab` well-formed (Y2); every global's attributes against its
//! definitions (Y3); locals against the input locals of their name (Y4); the symbols the
//! linker defines (Y5).

use std::collections::BTreeMap;

use crate::hasher::HashMap;

use mortar_elf::consts::{
    PT_LOAD, SHF_EXECINSTR, SHF_TLS, SHF_WRITE, SHF_X86_64_LARGE, SHN_ABS, SHN_LORESERVE,
    SHN_UNDEF, SHN_XINDEX, SHT_NOBITS, SHT_REL, SHT_RELA, SHT_STRTAB, SHT_SYMTAB, STB_GLOBAL,
    STB_GNU_UNIQUE, STB_LOCAL, STB_WEAK, STT_FILE, STT_FUNC, STT_GNU_IFUNC, STT_SECTION, STT_TLS,
    STV_DEFAULT, STV_HIDDEN, STV_INTERNAL, STV_PROTECTED,
};
use mortar_elf::{Class, Sym, SymbolIndex, SymbolSection};

use crate::image::{Image, OutSym};
use crate::locate::Place;
use crate::map::Format;
use crate::reloc::Engine;
use crate::report::{Finding, Rule};

fn y(n: u8, msg: impl Into<String>) -> Finding {
    Finding::new(Rule::SymbolTable(n), msg)
}

fn named(mut f: Finding, name: &str) -> Finding {
    f.symbol = Some(name.to_string());
    f
}

/// How constraining a visibility is (gABI "Symbol Visibility").
fn rank(v: u8) -> u8 {
    match v {
        STV_PROTECTED => 1,
        STV_HIDDEN => 2,
        STV_INTERNAL => 3,
        _ => 0,
    }
}

fn file_bytes<'a>(image: &Image<'a>, i: usize) -> Option<&'a [u8]> {
    let s = &image.sections.get(i)?.shdr;
    if s.sh_type == SHT_NOBITS {
        return None;
    }
    let start = usize::try_from(s.sh_offset).ok()?;
    image.data.get(start..start.checked_add(usize::try_from(s.sh_size).ok()?)?)
}

/// Y2: a string table begins and ends with a NUL (gABI "String Table").
fn strtab_ok(image: &Image<'_>, i: usize, what: &str, out: &mut Vec<Finding>) {
    let Some(s) = image.sections.get(i) else {
        return out.push(y(2, format!("{what} names section {i}, which does not exist")));
    };
    if s.shdr.sh_type != SHT_STRTAB {
        return out.push(y(2, format!("{what} '{}' is not SHT_STRTAB", s.name)));
    }
    match file_bytes(image, i) {
        Some(b) if b.first() == Some(&0) && b.last() == Some(&0) => {}
        Some(_) => out.push(y(2, format!("{what} '{}' does not begin and end with NUL", s.name))),
        None => out.push(y(2, format!("{what} '{}' is outside the file", s.name))),
    }
}

/// Y2 (§6.5): the tables themselves.
fn tables(image: &Image<'_>, out: &mut Vec<Finding>) {
    let h = &image.header.ehdr;
    let shstrndx = if h.e_shstrndx == SHN_XINDEX {
        image.sections.first().map_or(0, |s| s.shdr.sh_link as usize)
    } else {
        usize::from(h.e_shstrndx)
    };
    if shstrndx != 0 {
        strtab_ok(image, shstrndx, "e_shstrndx", out);
    }
    let Some(st) = image.sections.iter().find(|s| s.shdr.sh_type == SHT_SYMTAB) else {
        return;
    };
    let entsize = if image.class() == Class::Elf64 { 24 } else { 16 };
    if st.shdr.sh_entsize != entsize || st.shdr.sh_size % entsize != 0 {
        out.push(y(2, format!(".symtab entries are not {entsize} bytes")));
    }
    strtab_ok(image, st.shdr.sh_link as usize, ".symtab's sh_link", out);
    if let Some(z) = image.symbols.first() {
        let s = &z.sym;
        if s.st_name != 0
            || s.st_info != 0
            || s.st_other != 0
            || s.st_shndx != 0
            || s.st_value != 0
            || s.st_size != 0
        {
            out.push(y(2, "symbol 0 is not the null symbol"));
        }
    }
    // gABI "Symbol Table": the locals precede the others, and sh_info is
    // one greater than the last local's index.
    let first_global = image
        .symbols
        .iter()
        .position(|s| s.sym.binding() != STB_LOCAL)
        .unwrap_or(image.symbols.len());
    if let Some(late) = image.symbols[first_global..].iter().find(|s| s.sym.binding() == STB_LOCAL)
    {
        out.push(named(
            y(2, format!("local symbol {} follows the first global, {first_global}", late.index)),
            &late.name,
        ));
    }
    if st.shdr.sh_info as usize != first_global {
        let mut f = y(2, ".symtab's sh_info is not the index of its first non-local symbol");
        f.expected = Some(first_global as u64);
        f.found = Some(u64::from(st.shdr.sh_info));
        out.push(f);
    }
}

/// The address an input definition went to, and the output memory its
/// section occupies.
#[derive(Debug, Clone, Copy)]
struct Def {
    value: u64,
    /// `[start, end)` of the input section's place; `None` for `SHN_ABS`.
    within: Option<(u64, u64)>,
    sym: Sym,
}

/// Whether input symbol `di` of object `oi` is defined in a section that
/// has no address in the output: non-alloc, or discarded by the map.
fn not_in_memory(e: &Engine<'_, '_>, oi: usize, di: u32) -> bool {
    let obj = &e.objs[oi];
    let Some(table) = obj.symtab else { return false };
    let Ok(sym) = table.get(SymbolIndex(di)) else { return false };
    let Ok(SymbolSection::Section(idx)) = table.section_of(SymbolIndex(di), &sym) else {
        return false;
    };
    obj.secs
        .get(idx.0 as usize)
        .is_some_and(|sec| !sec.alloc() || matches!(sec.place, Place::Discarded))
}

#[allow(clippy::many_single_char_names)] // o, t, d, s, a, p: the ELF parts
fn placed_def(e: &Engine<'_, '_>, oi: usize, di: u32) -> Option<Def> {
    let o = &e.objs[oi];
    let t = o.symtab?;
    let d = t.get(SymbolIndex(di)).ok()?;
    match t.section_of(SymbolIndex(di), &d).ok()? {
        SymbolSection::Absolute => Some(Def { value: d.st_value, within: None, sym: d }),
        SymbolSection::Section(k) => {
            let s = o.secs.get(k.0 as usize)?;
            // A non-alloc section's addresses are not memory (§6.6).
            if !s.alloc() {
                return None;
            }
            // S-50 rule 1 (§4.4): a label at offset 0 of an input
            // `.eh_frame` (crtbeginT.o's `__EH_FRAME_BEGIN__`) is the
            // output section's start under mortar's map; a mapping symbol
            // stays at its record (S-76).
            if d.st_value == 0
                && !t.name(&d).is_ok_and(|n| n.starts_with(b"$"))
                && let Some(a) = e.eh_frame_start(oi, k.0 as usize)
            {
                return Some(Def { value: a, within: Some((a, a)), sym: d });
            }
            match &s.place {
                Place::Whole(a) => Some(Def {
                    value: a.wrapping_add(d.st_value),
                    within: Some((*a, a.wrapping_add(s.shdr.sh_size))),
                    sym: d,
                }),
                // The piece that holds it (lld's `.eh_frame`, §3.3).
                Place::Pieces(ps) => ps
                    .iter()
                    .find(|p| p.input_offset <= d.st_value && d.st_value < p.input_offset + p.size)
                    // A label at the section's end: the piece ending there
                    // (synth.md S-19).
                    .or_else(|| {
                        (d.st_value == s.shdr.sh_size)
                            .then(|| ps.iter().find(|p| p.input_offset + p.size == d.st_value))
                            .flatten()
                    })
                    .map(|p| Def {
                        value: p.addr + (d.st_value - p.input_offset),
                        within: Some((p.addr, p.addr + p.size)),
                        sym: d,
                    }),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The input section of a definition of `name` the output holds with an
/// address: placed whole or as pieces, or (mortar's map) empty without a
/// record (WP-143), allocated. `None` when every definition is elsewhere.
#[allow(clippy::many_single_char_names)] // e, o, t, d, s: the ELF parts
fn held_def(e: &Engine<'_, '_>, name: &str) -> Option<String> {
    e.defs.get(name)?.iter().find_map(|&(oi, di)| {
        let o = &e.objs[oi];
        let t = o.symtab?;
        let d = t.get(SymbolIndex(di)).ok()?;
        if d.binding() == STB_LOCAL {
            return None;
        }
        let SymbolSection::Section(k) = t.section_of(SymbolIndex(di), &d).ok()? else {
            return None;
        };
        let s = o.secs.get(k.0 as usize)?;
        // A foreign map's merged section may have lost every piece (lld
        // under --gc-sections): not known to be held.
        // Placed as pieces: the piece holding the label must be live
        // (mortar collects the dead pieces of a merged section under
        // --gc-sections; glibc's `__nptl_version`).
        let v = d.st_value;
        let held = match &s.place {
            Place::Whole(_) => true,
            Place::Pieces(ps) => ps.iter().any(|p| {
                (p.input_offset <= v && v < p.input_offset + p.size)
                    || (v == s.shdr.sh_size && p.input_offset + p.size == v)
            }),
            _ => e.format == Format::Mortar && s.empty_unrecorded(),
        };
        (s.alloc() && held).then(|| format!("section '{}' of {}", s.name, o.path))
    })
}

/// When every input definition of `name` is in an input `.eh_frame` and
/// the output has one: the first (strong first) definition's place, and
/// whether `value` is the one it must have (WP-143b). Under mortar's map
/// it is `synth.md` S-50's ([`Engine::eh_frame_value`]); under a
/// reference's, inside or at the end of the output `.eh_frame` (GNU ld
/// and lld write the end for a label after a terminator; mold drops it).
/// `None` when a definition is elsewhere, or there is no output
/// `.eh_frame` (then §4.4 step 0's rules hold).
#[allow(clippy::many_single_char_names)] // e, o, t, d, s: the ELF parts
fn eh_frame_label(
    e: &Engine<'_, '_>,
    name: &str,
    value: u64,
) -> Option<(String, Result<(), String>)> {
    let (addr, size) = e.output_eh_frame()?;
    let mut first = None;
    let mut strong = None;
    for &(oi, di) in e.defs.get(name)? {
        let o = &e.objs[oi];
        let t = o.symtab?;
        let d = t.get(SymbolIndex(di)).ok()?;
        let SymbolSection::Section(k) = t.section_of(SymbolIndex(di), &d).ok()? else {
            return None;
        };
        let k = k.0 as usize;
        if !o.secs.get(k)?.is_eh_frame(o.elf.machine()) {
            return None;
        }
        first.get_or_insert((oi, k, d.st_value));
        if d.binding() != STB_WEAK {
            strong.get_or_insert((oi, k, d.st_value));
        }
    }
    let (oi, k, r) = strong.or(first)?;
    let what = format!("'.eh_frame' of {} at offset {r:#x}", e.objs[oi].path);
    let verdict = match e.eh_frame_value(oi, k, r) {
        Some(want) if want == value => Ok(()),
        Some(want) => Err(format!("is at {value:#x}, not S-50's {want:#x} (synth.md S-50)")),
        None if addr <= value && value <= addr.saturating_add(size) => Ok(()),
        None => Err(format!(
            "is at {value:#x}, neither in nor at the end of the output's .eh_frame [{addr:#x}, {:#x}]",
            addr.saturating_add(size)
        )),
    };
    Some((what, verdict))
}

/// The name of the empty section without a record (WP-143) a definition
/// of `name` is in, when one is.
#[allow(clippy::many_single_char_names)] // e, o, t, d, s: the ELF parts
fn empty_def(e: &Engine<'_, '_>, name: &str) -> Option<String> {
    e.defs.get(name)?.iter().find_map(|&(oi, di)| {
        let o = &e.objs[oi];
        let t = o.symtab?;
        let d = t.get(SymbolIndex(di)).ok()?;
        let SymbolSection::Section(k) = t.section_of(SymbolIndex(di), &d).ok()? else {
            return None;
        };
        let s = o.secs.get(k.0 as usize)?;
        (s.empty_unrecorded() && s.shdr.sh_flags & SHF_TLS == 0).then(|| s.name.clone())
    })
}

/// The output symbol's address: a TLS symbol's value is its offset in the
/// TLS template (§4.7).
fn address(e: &Engine<'_, '_>, s: &Sym) -> Option<u64> {
    if s.sym_type() == STT_TLS {
        let Ok(Some(b)) = e.tls else { return None };
        return Some(b.start.wrapping_add(s.st_value));
    }
    Some(s.st_value)
}

/// Whether the section the output symbol names holds the definition's
/// input section (`SHN_ABS` for an absolute definition).
fn holds(image: &Image<'_>, o: &Sym, d: &Def) -> bool {
    match d.within {
        None => o.st_shndx == SHN_ABS,
        Some((lo, hi)) => image.sections.get(usize::from(o.st_shndx)).is_some_and(|s| {
            o.st_shndx != SHN_UNDEF
                && o.st_shndx < SHN_LORESERVE
                && s.alloc()
                && s.shdr.sh_addr <= lo
                && hi <= s.shdr.sh_addr.saturating_add(s.shdr.sh_size)
        }),
    }
}

/// Y3/Y4: an output symbol's type, size and section against the
/// definition it has the value of.
fn attributes(image: &Image<'_>, n: u8, out: &OutSym, d: &Def, fs: &mut Vec<Finding>) {
    let name = &out.name;
    let o = &out.sym;
    if o.sym_type() != d.sym.sym_type() {
        let mut f = y(n, format!("symbol '{name}' has another type than its definition"));
        f.expected = Some(u64::from(d.sym.sym_type()));
        f.found = Some(u64::from(o.sym_type()));
        fs.push(named(f.at(o.st_value), name));
    }
    if o.st_size != d.sym.st_size {
        let mut f = y(n, format!("symbol '{name}' has another size than its definition"));
        f.expected = Some(d.sym.st_size);
        f.found = Some(o.st_size);
        fs.push(named(f.at(o.st_value), name));
    }
    if !holds(image, o, d) {
        // An empty input section (crtend.o's `.tm_clone_table`) may leave
        // no output section to name (§10.2 A10).
        let empty = d.within.is_some_and(|(lo, hi)| lo == hi);
        let what = if empty { "a label of an empty section" } else { "symbol" };
        let mut f = y(
            n,
            format!("{what} '{name}' names a section that does not hold its definition's section"),
        );
        f.found = Some(u64::from(o.st_shndx));
        fs.push(named(f.at(o.st_value), name));
    }
}

/// Y3 (§6.5): every output symbol whose name the inputs define globally.
#[allow(clippy::too_many_lines, clippy::many_single_char_names)] // binding, visibility, then attributes
fn globals(e: &mut Engine<'_, '_>, fs: &mut Vec<Finding>) {
    let image = e.image;
    // The most constraining visibility of every non-local entry of a name,
    // defined or not.
    let mut vis: HashMap<&str, u8> = HashMap::default();
    for o in e.objs {
        let Some(t) = &o.symtab else { continue };
        for (_, s) in t.iter() {
            if s.binding() == STB_LOCAL || s.visibility() == 0 {
                continue;
            }
            let Ok(n) = std::str::from_utf8(t.name(&s).unwrap_or(b"")) else { continue };
            let v = vis.entry(n).or_insert(0);
            if rank(s.visibility()) > rank(*v) {
                *v = s.visibility();
            }
        }
    }
    let mut names: Vec<&String> = e.defs.keys().collect();
    names.sort();
    let (mut checked, mut unchecked) = (0u64, 0u64);
    for name in names {
        // §4.4b (WP-170): a `--defsym` symbol is `defsym_syms`'s, never
        // the replaced definition's.
        if e.defsym.defines(name) {
            continue;
        }
        let Some(out) = image.global(name).or_else(|| image.unique(name)) else {
            // Y3 (WP-143): a name an input defines globally in a section
            // the output holds is in a `.symtab` the output has.
            let v = vis.get(name.as_str()).copied().unwrap_or(0);
            if !image.symbols.is_empty()
                && !image.has_name(name)
                // `foo@@V2`: lld's and mold's `.symtab` name it `foo`;
                // mortar's keeps the versioned name (synth S-75, WP-150).
                && (e.format == Format::Mortar
                    || !name.split_once('@').is_some_and(|(base, _)| image.has_name(base)))
                && rank(v) < rank(STV_HIDDEN)
                && let Some(why) = held_def(e, name)
            {
                // §4.4a (WP-170): lld leaves a defined `__real_SYM` of a
                // `--wrap` word SYM out (A84), tagged.
                let tag = if name.strip_prefix("__real_").is_some_and(|x| e.wrap.contains(x)) {
                    crate::wrap::REAL_DROPPED
                } else {
                    ""
                };
                fs.push(named(
                    y(
                        3,
                        format!(
                            "symbol '{name}', defined in {why}, is not in the output's .symtab{tag}"
                        ),
                    ),
                    name,
                ));
            }
            continue;
        };
        if out.sym.st_shndx == SHN_UNDEF {
            continue;
        }
        let defs: Vec<(Def, u8)> = e.defs[name]
            .iter()
            .filter_map(|&(oi, di)| placed_def(e, oi, di))
            .map(|d| (d, d.sym.binding()))
            .collect();

        // An IFUNC in the GNU form is judged like any symbol; the lld and
        // mold form (a function at an IPLT entry) is Y1's (§4.8).
        let canonical_ifunc = defs.iter().any(|(d, _)| d.sym.sym_type() == STT_GNU_IFUNC)
            && out.sym.sym_type() != STT_GNU_IFUNC;
        if defs.is_empty()
            && let Some((what, want)) = eh_frame_label(e, name, out.sym.st_value)
        {
            // A label of an input `.eh_frame` (synth.md S-48a, S-50;
            // WP-143b): judged by S-50's value, not as "not in the output".
            match want {
                Ok(()) => checked += 1,
                Err(m) => {
                    let mut f = y(3, format!("symbol '{name}' of {what} {m}"));
                    f.found = Some(out.sym.st_value);
                    fs.push(named(f.at(out.sym.st_value), name));
                }
            }
            continue;
        }
        if defs.is_empty()
            && e.format == Format::Mortar
            && let Some((doi, _, k)) = e.placeless_def(name)
        {
            // Y3 (WP-143): every input definition is in a section not in
            // the output, yet the output defines the name.
            let sec = &e.objs[doi].secs[k].name;
            let mut f = y(
                3,
                format!(
                    "symbol '{name}' is defined in the output, but every input definition is in a section not in it ('{sec}')"
                ),
            );
            f.found = Some(out.sym.st_value);
            fs.push(named(f.at(out.sym.st_value), name));
            continue;
        }
        if defs.is_empty()
            && let Some(sec) = empty_def(e, name)
        {
            // An empty section's label is where its bytes would be: in or
            // at the end of an allocated output section (WP-143).
            if image.holds_or_ends(out.sym.st_value) {
                checked += 1;
            } else {
                let mut f = y(
                    3,
                    format!(
                        "symbol '{name}' of empty section '{sec}' is neither in nor at the end of an allocated output section"
                    ),
                );
                f.found = Some(out.sym.st_value);
                fs.push(named(f.at(out.sym.st_value), name));
            }
            continue;
        }
        if canonical_ifunc || defs.is_empty() {
            unchecked += 1;
            continue;
        }
        let Some(value) = address(e, &out.sym) else {
            unchecked += 1;
            continue;
        };
        let at: Vec<&Def> = defs.iter().filter(|(d, _)| d.value == value).map(|(d, _)| d).collect();
        let Some(d) = at.iter().find(|d| d.sym.binding() != STB_WEAK).or(at.first()) else {
            continue; // Y1's finding
        };
        checked += 1;
        // A strong definition overrides the weak ones (gABI "Symbol
        // Table", STB_WEAK).
        let strong: Vec<&Def> =
            defs.iter().filter(|(_, b)| *b != STB_WEAK).map(|(d, _)| d).collect();
        if !strong.is_empty() && !strong.iter().any(|s| s.value == value) {
            let mut f =
                y(3, format!("symbol '{name}' has a weak definition's value over a global one"));
            f.expected = Some(strong[0].value);
            f.found = Some(value);
            fs.push(named(f.at(value), name));
        }
        let v = vis.get(name.as_str()).copied().unwrap_or(0);
        let b = out.sym.binding();
        // synth S-77, S-121 (WP-169): a key the line makes local is
        // `STB_LOCAL` in mortar's `.symtab` with its own visibility. The
        // references' measured forms are named (WP-150 items 1 and 2):
        // `STB_LOCAL` with its own, `DEFAULT` (GNU ld, lld) or `HIDDEN`
        // (mold) visibility; GNU ld's executable `GLOBAL HIDDEN`.
        let made = e.made_local.keys.contains(name)
            || e.made_local.keys.contains(crate::shared::split_version(name).0);
        let mortar = e.format == Format::Mortar;
        let vis_ok = |x: u8| x == v || (!mortar && matches!(x, STV_DEFAULT | STV_HIDDEN));
        let bfd_hidden =
            e.format == Format::Gnu && e.made_local.exec && out.sym.visibility() == STV_HIDDEN;
        if made && rank(v) < rank(STV_HIDDEN) {
            if b != STB_LOCAL && !bfd_hidden {
                let mut f = y(
                    3,
                    format!(
                        "symbol '{name}', which the line makes local, is not STB_LOCAL (synth S-77)"
                    ),
                );
                f.expected = Some(u64::from(STB_LOCAL));
                f.found = Some(u64::from(b));
                fs.push(named(f.at(value), name));
            }
            if !vis_ok(out.sym.visibility()) {
                let mut f = y(
                    3,
                    if mortar {
                        format!(
                            "symbol '{name}', which the line makes local, does not keep its own st_other visibility (synth S-77)"
                        )
                    } else {
                        format!(
                            "symbol '{name}', which the line makes local, has neither its own st_other visibility nor the references' DEFAULT or HIDDEN (WP-150 item 1)"
                        )
                    },
                );
                f.expected = Some(u64::from(v));
                f.found = Some(u64::from(out.sym.visibility()));
                fs.push(named(f.at(value), name));
            }
        } else if rank(v) >= rank(STV_HIDDEN) {
            // gABI: converted to STB_LOCAL (GNU ld keeps the binding and
            // the visibility, §10.5).
            if b != STB_LOCAL && rank(out.sym.visibility()) < rank(STV_HIDDEN) {
                fs.push(named(
                    y(3, format!("hidden symbol '{name}' is neither local nor hidden")).at(value),
                    name,
                ));
            }
        } else {
            let want = if strong.is_empty() { STB_WEAK } else { d.sym.binding() };
            let fine = b == want || (want == STB_GNU_UNIQUE && b == STB_GLOBAL);
            if !fine {
                // §4.4a (WP-170): lld's form of a weak `SYM` a strong
                // `__real_SYM` binds to (A81), tagged for the anomaly.
                let tag = if want == STB_WEAK && b == STB_GLOBAL && strong_real(e, name) {
                    crate::wrap::REAL_STRONG
                } else {
                    ""
                };
                let mut f =
                    y(3, format!("symbol '{name}' has another binding than its definitions{tag}"));
                f.expected = Some(u64::from(want));
                f.found = Some(u64::from(b));
                fs.push(named(f.at(value), name));
            }
            if out.sym.visibility() != v {
                let mut f =
                    y(3, format!("symbol '{name}' has another visibility than its references"));
                f.expected = Some(u64::from(v));
                f.found = Some(u64::from(out.sym.visibility()));
                fs.push(named(f.at(value), name));
            }
        }
        attributes(image, 3, out, d, fs);
    }
    let mut names: Vec<&String> = e.commons.keys().collect();
    names.sort();
    for name in names {
        // A common the line makes local is a local of its name (S-77).
        let local = e.format == Format::Mortar && e.made_local.keys.contains(name.as_str());
        let Some(out) = image.global(name).or_else(|| local.then(|| image.unique(name)).flatten())
        else {
            continue;
        };
        if let Some(c) = commons(e, name) {
            checked += 1;
            common(image, out, c, fs);
        }
    }
    e.cov.globals_checked += checked;
    e.cov.globals_unchecked += unchecked;
}

/// A name defined only by common symbols (gABI `SHN_COMMON`; x86-64's
/// `SHN_X86_64_LCOMMON`, `spec/x64.md` X-55): the largest size and
/// alignment. `None` when a definition is not common.
fn commons(e: &Engine<'_, '_>, name: &str) -> Option<(u64, u64)> {
    let (mut size, mut align, mut any) = (0u64, 1u64, false);
    if e.defs.contains_key(name) {
        return None;
    }
    for &(oi, di) in e.commons.get(name)? {
        let d = e.objs[oi].symtab?.get(SymbolIndex(di)).ok()?;
        any = true;
        size = size.max(d.st_size);
        align = align.max(d.st_value);
    }
    any.then_some((size, align))
}

/// Y3 for a common symbol: the linker allocates it, `[value, value +
/// size)` in a writable `SHT_NOBITS` section, aligned to the largest
/// alignment (gABI "Symbol Table", `SHN_COMMON`). lld makes an x86-64
/// large common absolute and allocates nothing (§10.2 A14).
fn common(image: &Image<'_>, out: &OutSym, (size, align): (u64, u64), fs: &mut Vec<Finding>) {
    let (name, o) = (&out.name, &out.sym);
    let sec = (o.st_shndx != SHN_UNDEF && o.st_shndx < SHN_LORESERVE)
        .then(|| image.sections.get(usize::from(o.st_shndx)))
        .flatten();
    let holds = sec.is_some_and(|s| {
        s.alloc()
            && s.shdr.sh_type == SHT_NOBITS
            && s.shdr.sh_flags & SHF_WRITE != 0
            && s.shdr.sh_addr <= o.st_value
            && o.st_value.saturating_add(size) <= s.shdr.sh_addr.saturating_add(s.shdr.sh_size)
    });
    if !holds {
        let mut f = y(
            3,
            format!("common symbol '{name}' is not allocated in a writable SHT_NOBITS section"),
        );
        f.found = Some(u64::from(o.st_shndx));
        fs.push(named(f.at(o.st_value), name));
        return;
    }
    if o.st_size != size {
        let mut f =
            y(3, format!("common symbol '{name}' has another size than its largest definition"));
        f.expected = Some(size);
        f.found = Some(o.st_size);
        fs.push(named(f.at(o.st_value), name));
    }
    if align.is_power_of_two() && o.st_value % align != 0 {
        let mut f = y(3, format!("common symbol '{name}' is not aligned to {align:#x}"));
        f.found = Some(o.st_value);
        fs.push(named(f.at(o.st_value), name));
    }
}

/// The placed input locals of one name (Y4).
#[derive(Default)]
struct Placed {
    at: HashMap<u64, Vec<Def>>,
    count: usize,
    /// The value, when exactly one is placed.
    only: Option<u64>,
    /// Some input local of the name has no known place.
    unknown: bool,
    /// Some input local of the name is in a section with no place
    /// (P-1, `Obj::no_place`; mortar's map; WP-147).
    placeless: bool,
}

/// Whether input symbol `di` of object `oi` is in a section with no place
/// (§4.4 step 0's predicate, `Obj::no_place`).
#[allow(clippy::many_single_char_names)] // e, o, t, d, k: the ELF parts
fn def_no_place(e: &Engine<'_, '_>, oi: usize, di: u32) -> bool {
    let o = &e.objs[oi];
    let Some(t) = o.symtab else { return false };
    let Ok(d) = t.get(SymbolIndex(di)) else { return false };
    matches!(t.section_of(SymbolIndex(di), &d), Ok(SymbolSection::Section(k))
        if o.no_place(k.0 as usize, e.output_eh_frame().is_some()))
}

/// Y4 (§6.5): every output local whose name an input local has.
#[allow(clippy::many_single_char_names, clippy::too_many_lines)] // e, s, p, d, f; Y4 in order
fn locals(e: &mut Engine<'_, '_>, fs: &mut Vec<Finding>) {
    let image = e.image;
    let mut by_name: BTreeMap<&str, Vec<(usize, u32)>> = BTreeMap::new();
    for (oi, o) in e.objs.iter().enumerate() {
        let Some(t) = &o.symtab else { continue };
        for (i, s) in t.iter() {
            if s.binding() != STB_LOCAL || matches!(s.sym_type(), STT_SECTION | STT_FILE) {
                continue;
            }
            let Ok(n) = std::str::from_utf8(t.name(&s).unwrap_or(b"")) else { continue };
            if !n.is_empty() {
                by_name.entry(n).or_default().push((oi, i.0));
            }
        }
    }
    let mut placed: HashMap<&str, Placed> = HashMap::default();
    let (mut checked, mut unchecked) = (0u64, 0u64);
    for out in &image.symbols {
        let s = &out.sym;
        if out.index == 0
            || s.binding() != STB_LOCAL
            || matches!(s.sym_type(), STT_SECTION | STT_FILE)
            || out.name.is_empty()
            || s.st_shndx == SHN_UNDEF
        {
            continue;
        }
        let Some(ins) = by_name.get(out.name.as_str()) else { continue };
        // An output local in a non-alloc section has no address in memory,
        // as an input local there (Y4): GNU ld keeps Clang's `$d` of
        // `.comment` where the merged empty string lands (WP-121).
        if image
            .sections
            .get(usize::from(s.st_shndx))
            .is_some_and(|x| s.st_shndx < SHN_LORESERVE && !x.alloc())
        {
            unchecked += 1;
            continue;
        }
        // The name's placed input locals by value, computed once per name:
        // every object has its own `$x` and `$d`.
        let p = placed.entry(out.name.as_str()).or_insert_with(|| {
            let mut p = Placed::default();
            for &(oi, di) in ins {
                match placed_def(e, oi, di) {
                    Some(d) => {
                        p.only = if p.count == 0 { Some(d.value) } else { None };
                        p.count += 1;
                        p.at.entry(d.value).or_default().push(d);
                    }
                    // A definition in a section whose place is not known
                    // (GNU ld's rewritten `.eh_frame`, a merged section)
                    // could be the one; one in a non-alloc or discarded
                    // section has no address in the output (every
                    // object's `$d` of its debug sections; WP-028d).
                    // Under mortar's map, one in a section with no place
                    // is not in the output (P-1; WP-147).
                    None if e.format == Format::Mortar && def_no_place(e, oi, di) => {
                        p.placeless = true;
                    }
                    None => p.unknown |= !not_in_memory(e, oi, di),
                }
            }
            p
        });
        let unknown = p.unknown;
        // A mapping symbol may be the linker's own (a veneer's, a note's).
        let own_mapping = out.name.starts_with("$x") || out.name.starts_with("$d");
        if p.count == 0
            && !unknown
            && p.placeless
            && !own_mapping
            && !e.defs.contains_key(&out.name)
        {
            // `spec/resolve.md` §10.1 P-1: every input local of the name
            // is in a section with no place, so the output has no local of
            // it (before WP-136 mortar wrote a `.note.gnu.property`
            // local's offset in the note; WP-147).
            checked += 1;
            let mut f = y(
                4,
                format!(
                    "local symbol '{}' is in the output, but every input local of its name is in a section with no place (P-1 omits it)",
                    out.name
                ),
            );
            f.found = Some(s.st_value);
            fs.push(named(f.at(s.st_value), &out.name));
            continue;
        }
        let (false, Some(value)) = (p.count == 0 || e.defs.contains_key(&out.name), address(e, s))
        else {
            unchecked += 1;
            continue;
        };
        // Of the input locals of the name, the one the output entry is:
        // at its value, in its section, of its type and size; failing
        // that, one at its value in its section; failing that (and only
        // when every input local's place is known), any at its value.
        let same = |d: &&Def| d.value == value && holds(image, s, d);
        let fitting =
            |d: &&Def| same(d) && d.sym.sym_type() == s.sym_type() && d.sym.st_size == s.st_size;
        let defs = p.at.get(&value).map_or(&[][..], Vec::as_slice);
        let found = defs.iter().find(fitting).or_else(|| defs.iter().find(same));
        let found = if found.is_none() && !unknown { defs.first() } else { found };
        let Some(d) = found else {
            // A mapping symbol of a veneer or a patch: the linker's own,
            // in an output section but in no input section.
            let mapping = out.name.starts_with("$x") || out.name.starts_with("$d");
            // A local IFUNC in the lld and mold form: a function at an
            // IPLT entry of its resolver (§4.8).
            let canonical_ifunc = matches!(e.format, Format::Lld | Format::Mold)
                && s.sym_type() == STT_FUNC
                && p.at.iter().any(|(&r, ds)| {
                    ds.iter().any(|d| d.sym.sym_type() == STT_GNU_IFUNC)
                        && e.iplt.entries_of(r).contains(&value)
                });
            if canonical_ifunc {
                checked += 1;
                continue;
            }
            if unknown
                || mapping && !e.occupied_at(value) && image.alloc_section_of(value, 0).is_some()
            {
                unchecked += 1;
                continue;
            }
            checked += 1;
            let mut f = y(
                4,
                format!(
                    "local symbol '{}' is not where any input local of its name was placed",
                    out.name
                ),
            );
            f.expected = p.only;
            f.found = Some(value);
            fs.push(named(f.at(value), &out.name));
            continue;
        };
        checked += 1;
        attributes(image, 4, out, d, fs);
    }
    e.cov.locals_checked += checked;
    e.cov.locals_unchecked += unchecked;
}

/// Is `s` a C identifier (the sections `__start_`/`__stop_` name)?
fn c_identifier(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with(|c: char| c.is_ascii_digit())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// What a linker-defined symbol must be (§6.5, Y5).
enum Want {
    Exactly(u64),
    Within(u64, u64),
    /// No such section: the start and end symbols are equal.
    Equal,
}

#[allow(clippy::too_many_lines)] // the table of §6.5
fn linker_defined(e: &mut Engine<'_, '_>, fs: &mut Vec<Finding>) {
    let image = e.image;
    let sec = |n: &str| {
        image
            .sections
            .iter()
            .find(|s| s.name == n && s.alloc())
            .map(|s| (s.shdr.sh_addr, s.shdr.sh_addr.saturating_add(s.shdr.sh_size)))
    };
    let loads: Vec<_> = image.phdrs.iter().filter(|p| p.p_type == PT_LOAD).collect();
    let ehdr = loads.iter().find(|p| p.p_offset == 0).map(|p| p.p_vaddr);
    let alloc = || image.sections.iter().filter(|s| s.alloc() && s.shdr.sh_size > 0);
    let end = |s: &crate::image::OutSection| s.shdr.sh_addr.saturating_add(s.shdr.sh_size);
    let flag = |s: &crate::image::OutSection, f: u64| s.shdr.sh_flags & f != 0;
    let nobits = |s: &crate::image::OutSection| s.shdr.sh_type == SHT_NOBITS;
    let relro: Vec<(u64, u64)> = image
        .phdrs
        .iter()
        .filter(|p| p.p_type == mortar_elf::consts::PT_GNU_RELRO)
        .map(|p| (p.p_vaddr, p.p_vaddr.saturating_add(p.p_memsz)))
        .collect();
    let in_relro = |s: &crate::image::OutSection| {
        relro.iter().any(|&(lo, hi)| lo <= s.shdr.sh_addr && s.shdr.sh_addr < hi)
    };
    // The first allocated section at or above `a`, with `keep`.
    let next = |a: u64, keep: &dyn Fn(&crate::image::OutSection) -> bool| {
        alloc().filter(|s| s.shdr.sh_addr >= a && keep(s)).map(|s| s.shdr.sh_addr).min()
    };
    // The initialized data ends at E; the data and bss end at M.
    let data = || alloc().filter(|s| flag(s, SHF_WRITE) && !flag(s, SHF_TLS));
    // spec/x64.md X-54: an x86-64 large section (SHF_X86_64_LARGE) placed
    // after the small data and bss (GNU ld, lld) is not initialized data
    // for E; one placed before the bss (mold) is.
    let large = |s: &crate::image::OutSection| {
        e.image.header.e_machine == 62 && s.shdr.sh_flags & SHF_X86_64_LARGE != 0
    };
    let small_end = data().filter(|s| !large(s)).map(end).max();
    let e_end = data()
        .filter(|s| !nobits(s) && (!large(s) || small_end.is_none_or(|m| s.shdr.sh_addr < m)))
        .map(end)
        .max();
    let m_end = data().map(end).max();
    let text_end = image
        .sections
        .iter()
        .find(|s| s.name == ".text" && s.alloc())
        .map(end)
        .or_else(|| alloc().filter(|s| flag(s, SHF_EXECINSTR)).map(end).max());
    // synth.md R-P6: i386 is REL, with `__rel_iplt_*`; R-P5: the x86 GOT
    // base is `.got.plt`.
    let x86 = matches!(e.image.header.e_machine, 3 | 62);
    let rel_type = if e.image.header.e_machine == 3 { SHT_REL } else { SHT_RELA };
    let relas: Vec<_> = alloc().filter(|s| s.shdr.sh_type == rel_type).collect();
    let mut input_names: crate::hasher::HashSet<&str> = crate::hasher::HashSet::default();
    for o in e.objs {
        let Some(t) = &o.symtab else { continue };
        for (_, d) in t.iter() {
            if d.st_shndx != SHN_UNDEF
                && let Ok(n) = std::str::from_utf8(t.name(&d).unwrap_or(b""))
            {
                input_names.insert(n);
            }
        }
    }
    let defined_in_inputs = |n: &str| input_names.contains(n);
    let (mut checked, mut unjudged) = (0u64, Vec::new());
    for out in &image.symbols {
        let s = &out.sym;
        let name = out.name.as_str();
        if out.index == 0
            || name.is_empty()
            || s.st_shndx == SHN_UNDEF
            || matches!(s.sym_type(), STT_SECTION | STT_FILE)
            || defined_in_inputs(name)
        {
            continue;
        }
        let bounds = |n: &str, end: bool| -> Option<Want> {
            Some(match sec(n) {
                Some((lo, hi)) => Want::Exactly(if end { hi } else { lo }),
                None => Want::Equal,
            })
        };
        let want = match name {
            "__ehdr_start" => ehdr.map(Want::Exactly),
            "__executable_start" => loads.iter().map(|p| p.p_vaddr).min().map(Want::Exactly),
            "__init_array_start" => bounds(".init_array", false),
            "__init_array_end" => bounds(".init_array", true),
            "__fini_array_start" => bounds(".fini_array", false),
            "__fini_array_end" => bounds(".fini_array", true),
            "__preinit_array_start" => bounds(".preinit_array", false),
            "__preinit_array_end" => bounds(".preinit_array", true),
            "_GLOBAL_OFFSET_TABLE_" => {
                sec(if x86 { ".got.plt" } else { ".got" }).map(|(lo, hi)| {
                    // GNU ld's AArch64 `-z now` merges `.got.plt` into
                    // `.got` after the PLT's slots: the symbol is the word
                    // that holds `_DYNAMIC` (AArch64 ELF ABI GOT[0]).
                    let dynamic = sec(".dynamic").map(|(d, _)| d);
                    let at_dynamic = !x86
                        && e.format == Format::Gnu
                        && (lo..hi).contains(&s.st_value)
                        && dynamic.is_some_and(|d| image.value(s.st_value, 8) == Some(d));
                    Want::Exactly(if at_dynamic { s.st_value } else { lo })
                })
            }
            "__GNU_EH_FRAME_HDR" => sec(".eh_frame_hdr").map(|(lo, _)| Want::Exactly(lo)),
            "_edata" | "edata" => e_end.map(|v| Want::Within(v, next(v, &|_| true).unwrap_or(v))),
            "__bss_start" | "__bss_start__" => e_end.map(|v| {
                // layout §13.1: the first rank-8 `NOBITS` chunk; the RELRO
                // padding (mortar's `.pad.relro`, L-64; lld's
                // `.relro_padding`) is rank 7, inside `PT_GNU_RELRO`
                // (WP-169).
                let bss = next(v, &|s| {
                    nobits(s) && flag(s, SHF_WRITE) && !flag(s, SHF_TLS) && !in_relro(s)
                });
                Want::Within(v, bss.or_else(|| next(v, &|_| true)).unwrap_or(v))
            }),
            "_end" | "end" | "__end__" | "_bss_end__" | "__bss_end__" => {
                m_end.map(|m| Want::Within(m, m.saturating_add(7) & !7))
            }
            "_etext" | "etext" | "__etext" => text_end.map(|t| {
                let code = alloc().filter(|s| flag(s, SHF_EXECINSTR)).map(end).max().unwrap_or(t);
                Want::Within(t, next(t, &|s| !flag(s, SHF_EXECINSTR)).unwrap_or(code))
            }),
            "__rela_iplt_start" | "__rela_iplt_end" | "__rel_iplt_start" | "__rel_iplt_end"
                if name.starts_with("__rela_") == (rel_type == SHT_RELA) =>
            {
                // A dynamic output's loader applies its IRELATIVE records;
                // the C runtime's range is empty (stage 3; GNU ld and mold
                // give start = end, lld does not define them).
                let dynamic =
                    image.phdrs.iter().any(|p| p.p_type == mortar_elf::consts::PT_DYNAMIC);
                match relas.as_slice() {
                    _ if dynamic => Some(Want::Equal),
                    [] => Some(Want::Equal),
                    [r] => Some(Want::Exactly(if name.ends_with("_end") {
                        end(r)
                    } else {
                        r.shdr.sh_addr
                    })),
                    _ => None,
                }
            }
            "_DYNAMIC" => Some(Want::Exactly(sec(".dynamic").map_or(0, |(lo, _)| lo))),
            // At the TLS block's start; GNU ld's x86-64 executable's is at
            // TP, the offset align_up(p_memsz, p_align) (variant II,
            // §10.6); in a shared object GNU ld's is 0 too (X-61, WP-144).
            "_TLS_MODULE_BASE_" => Some(Want::Exactly(match e.tls {
                Ok(Some(b)) if x86 && e.format == Format::Gnu && !e.shared => b.tcb.wrapping_neg(),
                _ => 0,
            })),
            _ => {
                if let Some(hex) = name.strip_prefix("__CortexA53843419_") {
                    // lld's erratum 843419 patch (W8): at the patch of the
                    // site whose final word the name gives in hex, 8 bytes
                    // (the moved load or store and the branch back).
                    let site = u64::from_str_radix(hex, 16).ok();
                    let stub =
                        e.patch_stubs.iter().find(|(_, f)| Some(**f) == site).map(|(t, _)| *t);
                    match stub {
                        Some(t) if s.st_size == 8 => Some(Want::Exactly(t)),
                        Some(_) => {
                            fs.push(named(
                                y(5, format!("'{name}' has size {}, the patch 8", s.st_size)),
                                name,
                            ));
                            Some(Want::Equal)
                        }
                        // No patched site: the site's own verdict (C1, or
                        // erratum_not_judged) says what happened; the name
                        // labels dead bytes and is listed as not judged.
                        None => None,
                    }
                } else if let Some(x) = name.strip_prefix("__start_").filter(|x| c_identifier(x)) {
                    sec(x).map(|(lo, _)| Want::Exactly(lo))
                } else if let Some(x) = name.strip_prefix("__stop_").filter(|x| c_identifier(x)) {
                    sec(x).map(|(_, hi)| Want::Exactly(hi))
                } else {
                    None
                }
            }
        };
        let Some(want) = want else {
            unjudged.push(name.to_string());
            continue;
        };
        // _TLS_MODULE_BASE_ is the TLS block's start: offset 0 (§4.7).
        let value = s.st_value;
        checked += 1;
        let ok = match want {
            Want::Exactly(v) => value == v,
            Want::Within(lo, hi) => (lo..=hi).contains(&value),
            Want::Equal => {
                let other = match name.strip_suffix("_start") {
                    Some(b) => format!("{b}_end"),
                    None => format!("{}_start", name.trim_end_matches("_end")),
                };
                image.symbols.iter().filter(|o| o.name == other).all(|o| o.sym.st_value == value)
            }
        };
        if !ok {
            let mut f = y(5, format!("linker-defined symbol '{name}' has the wrong value"));
            if let Want::Exactly(v) | Want::Within(v, _) = want {
                f.expected = Some(v);
            }
            f.found = Some(value);
            fs.push(named(f.at(value), name));
        } else if name == "_TLS_MODULE_BASE_" && s.sym_type() != STT_TLS {
            fs.push(named(y(5, "_TLS_MODULE_BASE_ is not STT_TLS"), name));
        }
    }
    unjudged.sort();
    unjudged.dedup();
    e.cov.linker_defined_checked += checked;
    e.cov.linker_defined_not_judged = unjudged;
}

/// Whether `name` is a `--wrap` word and an input's undefined mention
/// `__real_name` is strong (§4.4a).
fn strong_real(e: &Engine<'_, '_>, name: &str) -> bool {
    if !e.wrap.contains(name) {
        return false;
    }
    let real = format!("__real_{name}");
    e.objs.iter().any(|o| {
        o.symtab.as_ref().is_some_and(|t| {
            t.iter().any(|(_, s)| {
                s.st_shndx == SHN_UNDEF
                    && s.binding() == STB_GLOBAL
                    && t.name(&s).is_ok_and(|n| n == real.as_bytes())
            })
        })
    })
}

/// Whether an input has a local symbol `name`.
fn input_local(e: &Engine<'_, '_>, name: &str) -> bool {
    e.objs.iter().any(|o| {
        o.symtab.as_ref().is_some_and(|t| {
            t.iter().any(|(_, s)| {
                s.binding() == STB_LOCAL && t.name(&s).is_ok_and(|n| n == name.as_bytes())
            })
        })
    })
}

/// Y3 (§4.4a, WP-170): `.symtab` names the definitions and the
/// references after redirection, so an entry `__real_SYM` of a `--wrap`
/// word SYM is there only when an input defines `__real_SYM`
/// (`spec/resolve.md` §7.1: an undefined `__real_SYM` binds to `SYM`).
fn wrap_real(e: &Engine<'_, '_>, fs: &mut Vec<Finding>) {
    if e.wrap.is_empty() {
        return;
    }
    for out in &e.image.symbols {
        let Some(sym) = out.name.strip_prefix("__real_") else { continue };
        if !e.wrap.contains(sym) || e.defs.contains_key(&out.name) {
            continue;
        }
        // A local of the name an input has is that input's (Y4's); mold
        // writes every global of a static executable `STB_LOCAL` (A9).
        if out.sym.binding() == STB_LOCAL && input_local(e, &out.name) {
            continue;
        }
        let mut f = y(
            3,
            format!(
                "symbol '{}' is in .symtab and no input defines it (resolve.md §7.1: an undefined __real_{sym} binds to {sym})",
                out.name
            ),
        );
        f.found = Some(out.sym.st_value);
        fs.push(named(f.at(out.sym.st_value), &out.name));
    }
}

/// The section index of the output section that holds `v` (or ends at
/// it): allocated, `SHF_TLS` for a TLS value when `tls`.
fn holds_value(image: &Image<'_>, shndx: u16, v: u64) -> bool {
    shndx != SHN_UNDEF
        && shndx < SHN_LORESERVE
        && image.sections.get(usize::from(shndx)).is_some_and(|s| {
            s.alloc() && s.shdr.sh_addr <= v && v <= s.shdr.sh_addr.saturating_add(s.shdr.sh_size)
        })
}

/// Y1 and Y3 of a `--defsym` symbol (§4.4b, WP-170 part 2): its
/// `.symtab` entry is `def[SYM]`, whatever an input defines: the value
/// (Y1); `SHN_ABS` and `STT_NOTYPE` for a number, `B`'s section and type
/// for an alias, `B`'s section and `STT_NOTYPE` for `B ± N`; size 0;
/// `STB_GLOBAL` (`STB_LOCAL` when the line makes it local) (Y3).
#[allow(clippy::too_many_lines)] // the attributes in order
fn defsym_syms(e: &mut Engine<'_, '_>, fs: &mut Vec<Finding>) {
    use crate::defsym::{Expr, OFFSET_ABS, REPLACED_SIZE, Value};
    if e.defsym.is_empty() {
        return;
    }
    let image = e.image;
    let all: Vec<(String, Expr)> =
        e.defsym.iter().map(|(s, x)| (s.to_string(), x.clone())).collect();
    let (mut checked, mut unchecked) = (0u64, 0u64);
    for (name, expr) in &all {
        let Some(out) = image.global(name).or_else(|| image.unique(name)) else {
            if !image.symbols.is_empty() {
                fs.push(named(
                    y(3, format!("symbol '{name}' of --defsym is not in the output's .symtab (resolve.md §7.2)")),
                    name,
                ));
            }
            continue;
        };
        let o = out.sym;
        if o.st_shndx == SHN_UNDEF {
            fs.push(named(
                y(
                    3,
                    format!(
                        "symbol '{name}' of --defsym is undefined in .symtab (resolve.md §7.2)"
                    ),
                )
                .at(o.st_value),
                name,
            ));
            continue;
        }
        let Some(value) = e.defsym_value(name) else { continue };
        let v = match value {
            Value::Addr(a) | Value::Ifunc(a) | Value::Tls(a) => a,
            Value::Unknown => {
                unchecked += 1;
                continue;
            }
        };
        checked += 1;
        // Y1: the value.
        let found = if o.sym_type() == STT_TLS { address(e, &o) } else { Some(o.st_value) };
        let ifunc_ok = matches!(value, Value::Ifunc(r) if e.format != Format::Mortar
            && o.sym_type() == STT_FUNC && e.iplt.entries_of(r).contains(&o.st_value));
        if found != Some(v) && !ifunc_ok {
            let mut f = Finding::new(
                Rule::SymbolValue,
                format!("symbol '{name}' is not where --defsym puts it (resolve.md §7.2)"),
            )
            .at(o.st_value);
            f.symbol = Some(name.clone());
            f.expected = Some(v);
            f.found = Some(o.st_value);
            fs.push(f);
        }
        // Y3: section and type.
        let b_type = |b: &str| {
            image
                .global(b)
                .or_else(|| image.unique(b))
                .filter(|s| s.sym.st_shndx != 0)
                .map(|s| s.sym.sym_type())
        };
        let (want_abs, want_type) = match expr {
            Expr::Number(_) => (true, Some(mortar_elf::consts::STT_NOTYPE)),
            Expr::Alias(b) => (false, b_type(b)),
            Expr::Offset(..) => (false, Some(mortar_elf::consts::STT_NOTYPE)),
        };
        let gnu_offset = matches!(expr, Expr::Offset(..))
            && (o.st_shndx == SHN_ABS || o.sym_type() != mortar_elf::consts::STT_NOTYPE);
        let tag = if gnu_offset { OFFSET_ABS } else { "" };
        let section_ok = if want_abs {
            o.st_shndx == SHN_ABS
        } else {
            match value {
                Value::Tls(_) => o.st_shndx != SHN_ABS && o.st_shndx < SHN_LORESERVE,
                _ => holds_value(image, o.st_shndx, o.st_value),
            }
        };
        if !section_ok {
            let mut f = y(
                3,
                if want_abs {
                    format!(
                        "symbol '{name}', a --defsym number, is not SHN_ABS (resolve.md §7.2){tag}"
                    )
                } else {
                    format!(
                        "symbol '{name}' of --defsym names a section that does not hold its value (resolve.md §7.2){tag}"
                    )
                },
            );
            f.found = Some(u64::from(o.st_shndx));
            fs.push(named(f.at(o.st_value), name));
        }
        if let Some(t) = want_type
            && o.sym_type() != t
            && !(ifunc_ok && t == STT_GNU_IFUNC)
        {
            let tag = if tag.is_empty() && e.defsym.several(name) {
                crate::defsym::EARLIER_TYPE
            } else {
                tag
            };
            let mut f = y(
                3,
                format!(
                    "symbol '{name}' of --defsym has another type than resolve.md §7.2 gives it{tag}"
                ),
            );
            f.expected = Some(u64::from(t));
            f.found = Some(u64::from(o.sym_type()));
            fs.push(named(f.at(o.st_value), name));
        }
        if o.st_size != 0 {
            let replaced = e.defs.get(name.as_str()).is_some_and(|ds| {
                ds.iter().any(|&(oi, di)| {
                    e.objs[oi]
                        .symtab
                        .and_then(|t| t.get(SymbolIndex(di)).ok())
                        .is_some_and(|d| d.st_size == o.st_size)
                })
            });
            let tag = if replaced { REPLACED_SIZE } else { "" };
            let mut f = y(
                3,
                format!("symbol '{name}' of --defsym has a size, not 0 (resolve.md §7.2){tag}"),
            );
            f.expected = Some(0);
            f.found = Some(o.st_size);
            fs.push(named(f.at(o.st_value), name));
        }
        let local = e.made_local.keys.contains(name.as_str());
        let want = if local { STB_LOCAL } else { STB_GLOBAL };
        if o.binding() != want {
            let mut f = y(
                3,
                format!(
                    "symbol '{name}' of --defsym has another binding than resolve.md §7.2 gives it"
                ),
            );
            f.expected = Some(u64::from(want));
            f.found = Some(u64::from(o.binding()));
            fs.push(named(f.at(o.st_value), name));
        }
    }
    e.cov.symbols_checked += checked;
    e.cov.symbols_unchecked += unchecked;
}

/// Y2 to Y5 (§6.5).
pub(crate) fn check(e: &mut Engine<'_, '_>) {
    let mut fs = Vec::new();
    tables(e.image, &mut fs);
    wrap_real(e, &mut fs);
    defsym_syms(e, &mut fs);
    globals(e, &mut fs);
    locals(e, &mut fs);
    linker_defined(e, &mut fs);
    e.findings.extend(fs);
}

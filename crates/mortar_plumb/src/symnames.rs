//! The names of `.symtab`'s entries against the inputs (`spec/plumb.md`
//! §6.5 Y5's closed set, Y6, Y7; WP-246, V-2).
//!
//! An incremental-mode output whose `.strtab` named the middle of a string
//! (`readed` for `__libc_single_threaded`) and four unnamed `STT_FILE`
//! entries verified with no finding: Y4 judged an output local only when an
//! input local had its name, Y5 listed every other name as "not judged",
//! and nothing judged a file symbol or a missing local. Here:
//!
//! - [`linker_name`]: the closed set of names a linker writes that no input
//!   has (`spec/resolve.md` §8.1's table, `spec/layout.md` L-99 and L-105,
//!   `spec/synth.md` S-80, the reference linkers' own, each with its
//!   source). Y5 judges a name outside it as a finding.
//! - [`locals`] (Y6): every input local the rules keep (`spec/synth.md`
//!   S-76, S-78, S-83) is in the output with its name, `st_info`,
//!   `st_other`, `st_size`, section and value, as a multiset per file;
//!   every output local is one of them, a demoted global (Y3's) or the
//!   linker's own (Y5's).
//! - [`files`] (Y7): every `STT_FILE` entry has a name, an input's.

use std::collections::BTreeMap;

use mortar_elf::consts::{
    SHN_ABS, SHN_LORESERVE, SHN_UNDEF, STB_LOCAL, STT_FILE, STT_SECTION, STT_TLS,
};
use mortar_elf::SymbolSection;

use crate::hasher::{HashMap, HashSet};
use crate::locate::Place;
use crate::map::{Format, Map};
use crate::opts::{Discard, Options};
use crate::reloc::Engine;
use crate::report::{Finding, Rule};

fn y(n: u8, msg: impl Into<String>) -> Finding {
    Finding::new(Rule::SymbolTable(n), msg)
}

fn named(mut f: Finding, name: &str) -> Finding {
    f.symbol = Some(name.to_string());
    f
}

/// Is `s` a C identifier (the sections `__start_`/`__stop_` name)?
fn c_identifier(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with(|c: char| c.is_ascii_digit())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `spec/synth.md` S-80: a thunk's or a patch's name,
/// `__<target>[+0x<a>|-0x<a>][@plt]_veneer` or `__erratum_843419_veneer`
/// (GNU ld's AArch64 form, which mortar writes).
fn veneer(name: &str) -> bool {
    name.len() > "___veneer".len() && name.starts_with("__")
        && name.ends_with("_veneer")
}

/// The closed set of names a linker writes into `.symtab` that no input
/// symbol has (§6.5 Y5, WP-246): the rule that names it, or `None`.
/// `machine` is `e_machine`. Every linker's own names are its own rows;
/// a name outside the set that no input has is a finding.
pub(crate) fn linker_name(name: &str, format: Format, machine: u16) -> Option<&'static str> {
    let aarch64 = machine == mortar_elf::consts::EM_AARCH64;
    // spec/resolve.md §8.1, the one list (with layout.md L-99, L-105).
    let common = match name {
        "_GLOBAL_OFFSET_TABLE_" | "_DYNAMIC" | "__ehdr_start" | "__executable_start" | "_etext"
        | "etext" | "__etext" | "_edata" | "edata" | "__bss_start" | "_end" | "end"
        | "__init_array_start" | "__init_array_end" | "__preinit_array_start"
        | "__preinit_array_end" | "__fini_array_start" | "__fini_array_end"
        | "__rela_iplt_start" | "__rela_iplt_end" | "__rel_iplt_start" | "__rel_iplt_end"
        | "_TLS_MODULE_BASE_" | "__GNU_EH_FRAME_HDR" => Some("resolve.md §8.1"),
        "__data_start" | "__end__" | "__bss_start__" | "__bss_end__" | "_bss_end__" if aarch64 => {
            Some("resolve.md §8.1 (AArch64)")
        }
        "__tdata_start" => Some("layout.md L-105"),
        _ => None,
    };
    if common.is_some() {
        return common;
    }
    if let Some(x) = name.strip_prefix("__start_").or_else(|| name.strip_prefix("__stop_"))
        && c_identifier(x)
    {
        return Some("resolve.md §8.1 (__start_/__stop_)");
    }
    match format {
        Format::Mortar => {
            if aarch64 && veneer(name) {
                return Some("synth.md S-80");
            }
            None
        }
        Format::Gnu => {
            if aarch64 && veneer(name) {
                return Some("GNU ld's AArch64 stubs (synth.md S-80)");
            }
            None
        }
        Format::Lld => {
            // lld's thunks and patches (§6.5, `__CortexA53843419_` judged by
            // W8), and its own `__dso_handle` (layout.md §13.1, E-24).
            if name.starts_with("__AArch64ADRPThunk_")
                || name.starts_with("__AArch64AbsLongThunk_")
                || name.starts_with("__CortexA53843419_")
            {
                return Some("lld's thunks and patches (§6.5)");
            }
            (name == "__dso_handle").then_some("lld defines __dso_handle (layout.md §13.1)")
        }
        Format::Mold => {
            // §6.5's not-judged list: mold's own.
            if name.contains('$') || name == "__start_EHDR" || name == "__start_PHDR" {
                return Some("mold's own (§6.5)");
            }
            (name == "__dso_handle").then_some("mold defines __dso_handle (layout.md §13.1)")
        }
    }
}

/// Where an expected local is: what the output entry's `st_shndx` must
/// hold.
#[derive(Debug, Clone, Copy)]
enum Where {
    Abs,
    /// An allocated input range `[lo, hi]`.
    Alloc(u64, u64),
    /// A non-allocated section.
    NonAlloc,
}

/// One input local the rules keep.
struct Want {
    file: usize,
    index: u32,
    at: Where,
    matched: bool,
}

/// The key a kept local and its output entry share.
type Key = (String, u8, u8, u64, u64);

/// Why an input local is not judged (Y6's coverage, by name).
const NJ_EH_FRAME: &str = "locals of .eh_frame (S-50 drops some by relocation)";
const NJ_EMPTY: &str = "labels of empty sections without a record (S-123)";
const NJ_MERGED: &str = "locals of sections merged without pieces";
const NJ_UNKNOWN: &str = "locals of sections whose place the map does not give";
const NJ_TLS: &str = "thread-local locals without a usable PT_TLS";
const NJ_UNDEF: &str = "undefined or common locals";
const NJ_LINE: &str = "every local: the link's -x/-X options are not known";

/// The options that decide which locals are kept: mortar's `arg` records,
/// or the caller's line.
fn options(e: &Engine<'_, '_>, map: &Map) -> Option<Options> {
    let line = e.caller_line.as_deref();
    crate::opts::of(map, line).or_else(|| {
        line.map(|w| Options::read(&w.iter().map(|x| x.as_bytes().to_vec()).collect::<Vec<_>>()))
    })
}

/// S-78: whether the `-x`/`-X` rule discards input local `name` (not
/// `STT_FILE` for `-X`), whose section is split (`Place::Pieces`) or not.
fn discarded(d: Discard, name: &str, file: bool, split: bool) -> bool {
    match d {
        Discard::All => true,
        Discard::None => false,
        Discard::Locals => !file && name.starts_with(".L"),
        Discard::Default => !file && split && name.starts_with(".L"),
    }
}

/// Y6 (§6.5, WP-246): the locals of each input file in the link.
#[allow(clippy::too_many_lines)] // the expected set, then the output's
pub(crate) fn locals(e: &mut Engine<'_, '_>, map: &Map, fs: &mut Vec<Finding>) {
    let image = e.image;
    if image.symbols.is_empty() {
        return;
    }
    let Some(opts) = options(e, map) else {
        e.cov.input_locals_not_judged.push((NJ_LINE.to_string(), 1));
        return;
    };
    let eh_out = e.output_eh_frame();
    let eh_index = image.sections.iter().position(|s| s.name == ".eh_frame" && s.alloc());
    let tls = match e.tls {
        Ok(Some(b)) => Some(b.start),
        _ => None,
    };
    let mut wants: Vec<Want> = Vec::new();
    let mut by_key: HashMap<Key, Vec<usize>> = HashMap::default();
    // Names of input locals not judged, with the output section an entry
    // of theirs would be in (`None`: any).
    let mut unjudged: HashMap<String, Vec<Option<usize>>> = HashMap::default();
    let mut nj: BTreeMap<&'static str, u64> = BTreeMap::new();
    let mut dropped = 0u64;
    for (oi, o) in e.objs.iter().enumerate() {
        let Some(t) = o.symtab else { continue };
        for (i, s) in t.iter() {
            if i.0 == 0 || s.binding() != STB_LOCAL || s.sym_type() == STT_SECTION {
                continue;
            }
            let name = String::from_utf8_lossy(t.name(&s).unwrap_or(b"")).into_owned();
            let file = s.sym_type() == STT_FILE;
            let mut skip = |why: &'static str, out: Option<usize>, name: String| {
                *nj.entry(why).or_default() += 1;
                unjudged.entry(name).or_default().push(out);
            };
            let (value, at, split) = match t.section_of(i, &s) {
                Ok(SymbolSection::Absolute) => (s.st_value, Where::Abs, false),
                Ok(SymbolSection::Section(k)) => {
                    let k = k.0 as usize;
                    let Some(sec) = o.secs.get(k) else {
                        skip(NJ_UNKNOWN, None, name);
                        continue;
                    };
                    if sec.is_eh_frame(o.elf.machine()) && eh_out.is_some() {
                        skip(NJ_EH_FRAME, eh_index, name);
                        continue;
                    }
                    match &sec.place {
                        Place::Whole(a) => {
                            let v = a.wrapping_add(s.st_value);
                            let at = if sec.alloc() {
                                Where::Alloc(*a, a.wrapping_add(sec.shdr.sh_size))
                            } else {
                                Where::NonAlloc
                            };
                            (v, at, false)
                        }
                        Place::Pieces(ps) => {
                            let r = s.st_value;
                            let p = ps
                                .iter()
                                .find(|p| p.input_offset <= r && r < p.input_offset + p.size)
                                .or_else(|| {
                                    (r == sec.shdr.sh_size)
                                        .then(|| ps.iter().find(|p| p.input_offset + p.size == r))
                                        .flatten()
                                });
                            match p {
                                Some(p) => {
                                    let at = if sec.alloc() {
                                        Where::Alloc(p.addr, p.addr + p.size)
                                    } else {
                                        Where::NonAlloc
                                    };
                                    (p.addr + (r - p.input_offset), at, true)
                                }
                                // S-76: its piece's fragment is not live.
                                None => {
                                    dropped += 1;
                                    continue;
                                }
                            }
                        }
                        Place::Discarded => {
                            dropped += 1;
                            continue;
                        }
                        Place::Absent if map.format == Format::Mortar && !sec.empty_unrecorded() => {
                            // §4.4 step 0: no place, so not in the output
                            // (S-76; P-1).
                            dropped += 1;
                            continue;
                        }
                        Place::Absent if sec.empty_unrecorded() => {
                            skip(NJ_EMPTY, None, name);
                            continue;
                        }
                        Place::Merged => {
                            skip(NJ_MERGED, None, name);
                            continue;
                        }
                        Place::Absent | Place::Rewritten | Place::Ambiguous => {
                            skip(NJ_UNKNOWN, None, name);
                            continue;
                        }
                    }
                }
                _ => {
                    skip(NJ_UNDEF, None, name);
                    continue;
                }
            };
            if discarded(opts.discard, &name, file, split) {
                dropped += 1;
                continue;
            }
            let value = if s.sym_type() == STT_TLS && !matches!(at, Where::Abs) {
                match tls {
                    Some(start) => value.wrapping_sub(start),
                    None => {
                        skip(NJ_TLS, None, name);
                        continue;
                    }
                }
            } else {
                value
            };
            let key = (name, s.st_info, s.st_other, s.st_size, value);
            by_key.entry(key).or_default().push(wants.len());
            wants.push(Want { file: oi, index: i.0, at, matched: false });
        }
    }
    // The output's locals.
    let mut extra = 0u64;
    let mut own = 0u64;
    let mut demoted = 0u64;
    let mut extra_nj = 0u64;
    let machine = image.header.e_machine;
    for out in &image.symbols {
        let s = &out.sym;
        if out.index == 0 || s.binding() != STB_LOCAL || s.sym_type() == STT_SECTION {
            continue;
        }
        let key = (out.name.clone(), s.st_info, s.st_other, s.st_size, s.st_value);
        let holds = |w: &Want| match w.at {
            Where::Abs => s.st_shndx == SHN_ABS,
            Where::Alloc(lo, hi) => {
                s.st_shndx != SHN_UNDEF
                    && s.st_shndx < SHN_LORESERVE
                    && image.sections.get(usize::from(s.st_shndx)).is_some_and(|x| {
                        x.alloc()
                            && x.shdr.sh_addr <= lo
                            && hi <= x.shdr.sh_addr.saturating_add(x.shdr.sh_size)
                    })
            }
            Where::NonAlloc => {
                s.st_shndx != SHN_UNDEF
                    && s.st_shndx < SHN_LORESERVE
                    && image.sections.get(usize::from(s.st_shndx)).is_some_and(|x| !x.alloc())
            }
        };
        if let Some(ix) = by_key.get(&key) {
            let free = ix.iter().copied().filter(|&w| !wants[w].matched);
            let pick = free.clone().find(|&w| holds(&wants[w])).or_else(|| free.clone().next());
            if let Some(w) = pick {
                wants[w].matched = true;
                if !holds(&wants[w]) {
                    let mut f = y(
                        6,
                        format!(
                            ".symtab entry {} ('{}') names section {}, which does not hold the input local's section ({}, entry {})",
                            out.index,
                            out.name,
                            s.st_shndx,
                            e.objs[wants[w].file].path,
                            wants[w].index
                        ),
                    );
                    f.found = Some(u64::from(s.st_shndx));
                    fs.push(named(f.at(s.st_value), &out.name));
                }
                continue;
            }
        }
        // Not a kept input local: a demoted global (Y3's), the linker's
        // own (Y5's: its block, S-74, S-80), or a local not judged.
        if e.defs.contains_key(&out.name)
            || e.commons.contains_key(&out.name)
            || e.made_local.keys.contains(&out.name)
            || e.defs.contains_key(crate::shared::split_version(&out.name).0)
        {
            demoted += 1;
            continue;
        }
        let mapping = out.name.starts_with("$x") || out.name.starts_with("$d");
        if linker_name(&out.name, e.format, machine).is_some()
            || e.defsym.defines(&out.name)
            || (mapping && !e.occupied_at(s.st_value))
        {
            own += 1;
            continue;
        }
        if unjudged.get(&out.name).is_some_and(|v| {
            v.iter().any(|x| x.is_none_or(|k| usize::from(s.st_shndx) == k))
        }) {
            extra_nj += 1;
            continue;
        }
        extra += 1;
        let mut f = y(
            6,
            format!(
                ".symtab entry {} ('{}', st_info {:#x}, st_other {:#x}, size {}, value {:#x}) is no kept input local (synth.md S-76)",
                out.index, out.name, s.st_info, s.st_other, s.st_size, s.st_value
            ),
        );
        f.found = Some(s.st_value);
        fs.push(named(f.at(s.st_value), &out.name));
    }
    let mut missing = 0u64;
    let mut by_file: BTreeMap<usize, Vec<(&Key, &Want)>> = BTreeMap::new();
    for (k, ix) in &by_key {
        for &w in ix {
            if !wants[w].matched {
                by_file.entry(wants[w].file).or_default().push((k, &wants[w]));
            }
        }
    }
    for (file, mut v) in by_file {
        v.sort_by_key(|(_, w)| w.index);
        for (k, w) in v {
            missing += 1;
            let mut f = y(
                6,
                format!(
                    "the local '{}' ({} entry {}, st_info {:#x}, st_other {:#x}, size {}, value {:#x}) is not in .symtab (synth.md S-76)",
                    k.0, e.objs[file].path, w.index, k.1, k.2, k.3, k.4
                ),
            );
            f.expected = Some(k.4);
            fs.push(named(f.at(k.4), &k.0));
        }
    }
    let c = &mut e.cov;
    c.input_locals_kept = wants.len() as u64;
    c.input_locals_dropped = dropped;
    c.input_locals_missing = missing;
    c.output_locals_extra = extra;
    c.output_locals_demoted = demoted;
    c.output_locals_own = own;
    if extra_nj > 0 {
        nj.insert("output locals named as an input local not judged", extra_nj);
    }
    c.input_locals_not_judged = nj.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
}

/// Y7 (§6.5, WP-246): every `STT_FILE` entry has a name, an input's
/// `STT_FILE` name (`spec/synth.md` S-83: copied in place; GNU ld
/// inserts one named after the object or member).
pub(crate) fn files(e: &mut Engine<'_, '_>, fs: &mut Vec<Finding>) {
    let image = e.image;
    let mut names: HashSet<String> = HashSet::default();
    for o in e.objs {
        if let Some(t) = o.symtab {
            for (_, s) in t.iter() {
                if s.sym_type() == STT_FILE {
                    names.insert(String::from_utf8_lossy(t.name(&s).unwrap_or(b"")).into_owned());
                }
            }
        }
        if e.format == Format::Gnu {
            let base = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
            names.insert(base(&o.path));
            if let Some(m) = &o.member {
                names.insert(base(m));
            }
        }
    }
    let mut judged = 0u64;
    for out in &image.symbols {
        if out.index == 0 || out.sym.sym_type() != STT_FILE {
            continue;
        }
        judged += 1;
        if out.name.is_empty() {
            fs.push(y(7, format!(".symtab entry {} is an STT_FILE with no name", out.index)));
        } else if !names.contains(&out.name) {
            fs.push(named(
                y(
                    7,
                    format!(
                        ".symtab entry {} is an STT_FILE named '{}', which no input's STT_FILE has (synth.md S-83)",
                        out.index, out.name
                    ),
                ),
                &out.name,
            ));
        }
    }
    e.cov.file_symbols_judged = judged;
}

/// Y5's closed set (§6.5, WP-246): of the names Y5 found no value rule
/// for, those [`linker_name`] or a `--defsym` key names stay "not judged";
/// every other is a finding, naming each entry of the name. So is an entry
/// with no name (section and file symbols aside) and an undefined entry
/// whose name no input mentions.
fn unexplained(e: &mut Engine<'_, '_>, map: &Map, fs: &mut Vec<Finding>) {
    let image = e.image;
    let machine = image.header.e_machine;
    let script = options(e, map).is_some_and(|o| o.script);
    let names = std::mem::take(&mut e.cov.linker_defined_not_judged);
    let mut kept = Vec::new();
    for n in names {
        if linker_name(&n, e.format, machine).is_some() || e.defsym.defines(&n) {
            kept.push(n);
            continue;
        }
        if script {
            kept.push(format!("{n} (the line has a script)"));
            continue;
        }
        for out in image.symbols.iter().filter(|o| o.name == n) {
            let mut f = y(
                5,
                format!(
                    ".symtab entry {} ('{n}') is defined, but no input symbol has the name and no rule names it the linker's (resolve.md §8.1)",
                    out.index
                ),
            );
            f.found = Some(out.sym.st_value);
            fs.push(named(f.at(out.sym.st_value), &n));
        }
    }
    e.cov.linker_defined_not_judged = kept;
    let mut mentioned: HashSet<&[u8]> = HashSet::default();
    for o in e.objs {
        if let Some(t) = &o.symtab {
            for (_, s) in t.iter() {
                if let Ok(n) = t.name(&s) {
                    mentioned.insert(n);
                }
            }
        }
    }
    for out in &image.symbols {
        let s = &out.sym;
        if out.index == 0 || matches!(s.sym_type(), STT_SECTION | STT_FILE) {
            continue;
        }
        if out.name.is_empty() {
            fs.push(y(
                5,
                format!(
                    ".symtab entry {} (st_info {:#x}, value {:#x}) has no name",
                    out.index, s.st_info, s.st_value
                ),
            ));
        } else if s.st_shndx == SHN_UNDEF
            && !mentioned.contains(out.name.as_bytes())
            && linker_name(&out.name, e.format, machine).is_none()
        {
            fs.push(named(
                y(
                    5,
                    format!(
                        ".symtab entry {} ('{}') is undefined, and no input mentions the name",
                        out.index, out.name
                    ),
                ),
                &out.name,
            ));
        }
    }
}

/// Y5's closed set, Y6 and Y7.
pub(crate) fn check(e: &mut Engine<'_, '_>, map: &Map) {
    let mut fs = Vec::new();
    unexplained(e, map, &mut fs);
    files(e, &mut fs);
    locals(e, map, &mut fs);
    e.findings.extend(fs);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s80_veneers_and_the_closed_set() {
        let a = mortar_elf::consts::EM_AARCH64;
        assert!(linker_name("__foo_veneer", Format::Mortar, a).is_some());
        assert!(linker_name("__erratum_843419_veneer", Format::Mortar, a).is_some());
        assert!(linker_name("__start_my_sec", Format::Mortar, a).is_some());
        assert!(linker_name("__start_.text", Format::Mortar, a).is_none());
        assert!(linker_name("readed", Format::Mortar, a).is_none());
        assert!(linker_name("__dso_handle", Format::Mortar, a).is_none());
        assert!(linker_name("__dso_handle", Format::Lld, a).is_some());
        assert!(linker_name("__data_start", Format::Mortar, 62).is_none());
    }
}

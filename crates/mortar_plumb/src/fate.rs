//! M10 (`spec/plumb.md` §3.3): a section of a live input that mortar's map
//! gives no place has a reason the inputs and the line give
//! (`spec/resolve.md` §10, `spec/image.md` I-6, I-78), derived here, not
//! taken from the linker (WP-143).

use mortar_elf::consts::{
    GRP_COMDAT, SHF_ALLOC, SHF_EXCLUDE, SHF_GNU_RETAIN, SHF_GROUP, SHF_LINK_ORDER, SHN_UNDEF,
    SHT_AARCH64_ATTRIBUTES, SHT_FINI_ARRAY, SHT_GROUP, SHT_INIT_ARRAY, SHT_LLVM_ADDRSIG,
    SHT_LLVM_CALL_GRAPH_PROFILE, SHT_LLVM_DEPENDENT_LIBRARIES, SHT_NOTE, SHT_PREINIT_ARRAY,
};
use mortar_elf::{Machine, SymbolIndex, SymbolSection};

use crate::locate::{Obj, Place, Sec};
use crate::map::{DiscardReason, Format, Map};
use crate::opts::Options;
use crate::reloc::debug::is_debug_name;
use crate::report::{Finding, Rule};

/// Why a section of a live object may be without a place, by
/// `spec/resolve.md` §10's rows (`None`: no row drops it).
fn dropped_by_row(o: &Obj<'_>, s: &Sec<'_>, opts: &Options) -> Option<&'static str> {
    let t = s.shdr.sh_type;
    let n = s.name.as_str();
    let debug = s.shdr.sh_flags & SHF_ALLOC == 0 && is_debug_name(n);
    if s.shdr.sh_flags & SHF_EXCLUDE != 0 {
        return Some("SHF_EXCLUDE (resolve.md §10)");
    }
    if n == ".mason.meta" {
        return Some("a .mason.meta container (resolve.md §10, meta.md MT-6)");
    }
    if n == ".note.GNU-stack" {
        return Some("consumed: .note.GNU-stack (resolve.md §10)");
    }
    if n == ".note.gnu.build-id" {
        return Some("excluded: .note.gnu.build-id (resolve.md §10, WP-135)");
    }
    if n == ".note.gnu.property" && t == SHT_NOTE {
        return Some("consumed into the output's property note (resolve.md §10, synth.md)");
    }
    if n.starts_with(".gnu.lto_") {
        return Some("excluded: .gnu.lto_* (resolve.md §10)");
    }
    if n == ".gnu.warning" || n.starts_with(".gnu.warning.") {
        return Some("consumed: .gnu.warning (resolve.md §10)");
    }
    if matches!(t, SHT_LLVM_ADDRSIG | SHT_LLVM_CALL_GRAPH_PROFILE | SHT_LLVM_DEPENDENT_LIBRARIES) {
        return Some("consumed: an SHT_LLVM_* section (resolve.md §10)");
    }
    if t == SHT_AARCH64_ATTRIBUTES && o.elf.machine() == Machine::AArch64 {
        return Some("consumed: SHT_AARCH64_ATTRIBUTES (resolve.md §10, layout.md L-35)");
    }
    if debug && opts.strip_debug {
        return Some("stripped under -S/-s (resolve.md §10)");
    }
    if opts.gdb_index
        && !opts.strip_debug
        && matches!(n, ".debug_gnu_pubnames" | ".debug_gnu_pubtypes")
    {
        return Some("consumed by --gdb-index (resolve.md §10, synth.md S-G6)");
    }
    None
}

/// The COMDAT copies that are not kept (`spec/resolve.md` §6.1: the
/// keeper of a signature is its group in the live file of least
/// `FileId`, the first by section index there; the map lists files in
/// `FileId` order, I-78): per object, the sections of its dropped
/// copies.
fn comdat_dropped(objs: &[Obj<'_>]) -> Vec<Vec<bool>> {
    let mut kept: std::collections::BTreeSet<Vec<u8>> = std::collections::BTreeSet::new();
    let mut out: Vec<Vec<bool>> = objs.iter().map(|o| vec![false; o.secs.len()]).collect();
    for (oi, o) in objs.iter().enumerate() {
        for (gi, g) in o.secs.iter().enumerate() {
            if g.shdr.sh_type != SHT_GROUP {
                continue;
            }
            let (flags, members) = group_words(o, gi);
            if flags & GRP_COMDAT == 0 {
                continue;
            }
            let Some(sig) = signature(o, SymbolIndex(g.shdr.sh_info)) else { continue };
            if kept.insert(sig) {
                continue;
            }
            for m in members {
                if let Some(d) = out[oi].get_mut(m) {
                    *d = true;
                }
            }
        }
    }
    out
}

/// The flag word and member indexes of `SHT_GROUP` section `gi` (gABI
/// "Section Groups": `Elf32_Word`s, the first the flags), read through
/// [`Obj::data`] (an object's contents may not be resident).
fn group_words(o: &Obj<'_>, gi: usize) -> (u32, Vec<usize>) {
    let data = o.data(gi);
    let mut words = data.chunks_exact(4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]));
    let flags = words.next().unwrap_or(0);
    (flags, words.map(|w| w as usize).filter(|&w| w != 0).collect())
}

/// A group's signature (`spec/resolve.md` §6.1 M1): its symbol's name, or
/// for a section symbol the name of its section.
#[allow(clippy::many_single_char_names)] // o, i, t, s, k: the ELF parts
fn signature(o: &Obj<'_>, i: SymbolIndex) -> Option<Vec<u8>> {
    let t = o.symtab?;
    let s = t.get(i).ok()?;
    let name = t.name(&s).ok()?;
    if !name.is_empty() {
        return Some(name.to_vec());
    }
    match t.section_of(i, &s).ok()? {
        SymbolSection::Section(k) => o.secs.get(k.0 as usize).map(|s| s.name.clone().into_bytes()),
        _ => None,
    }
}

/// Per object, whether section `si` is in a group with an allocated
/// member (`spec/image.md` I-6).
fn in_alloc_group(objs: &[Obj<'_>]) -> Vec<Vec<bool>> {
    let mut out: Vec<Vec<bool>> = objs.iter().map(|o| vec![false; o.secs.len()]).collect();
    for (oi, o) in objs.iter().enumerate() {
        for (gi, g) in o.secs.iter().enumerate() {
            if g.shdr.sh_type != SHT_GROUP {
                continue;
            }
            let (_, members) = group_words(o, gi);
            if members.iter().any(|&m| o.secs.get(m).is_some_and(Sec::alloc)) {
                for m in members {
                    if let Some(d) = out[oi].get_mut(m) {
                        *d = true;
                    }
                }
            }
        }
    }
    out
}

/// The names `__start_X`/`__stop_X` some input references (undefined):
/// the `X` (`spec/image.md` I-6, last root).
fn start_stop(objs: &[Obj<'_>]) -> std::collections::BTreeSet<Vec<u8>> {
    let mut out = std::collections::BTreeSet::new();
    for o in objs {
        let Some(t) = o.symtab else { continue };
        for (_, s) in t.iter() {
            if s.st_shndx != SHN_UNDEF {
                continue;
            }
            let Ok(n) = t.name(&s) else { continue };
            if let Some(x) = n.strip_prefix(b"__start_").or_else(|| n.strip_prefix(b"__stop_")) {
                out.insert(x.to_vec());
            }
        }
    }
    out
}

/// Whether a garbage collector may not drop `s` (`spec/image.md` I-6's
/// roots the inputs show; I-109: a `SHF_LINK_ORDER` section lives with
/// its link). `None` when it may be dropped.
fn gc_root(
    o: &Obj<'_>,
    s: &Sec<'_>,
    alloc_group: bool,
    ss: &std::collections::BTreeSet<Vec<u8>>,
) -> Option<&'static str> {
    let f = s.shdr.sh_flags;
    let n = s.name.as_str();
    if f & SHF_LINK_ORDER != 0 {
        let link = o.secs.get(s.shdr.sh_link as usize)?;
        return (!matches!(link.place, Place::Absent | Place::Discarded))
            .then_some("SHF_LINK_ORDER with its link target placed (image.md I-109)");
    }
    if f & SHF_GNU_RETAIN != 0 {
        return Some("SHF_GNU_RETAIN (image.md I-6)");
    }
    let startup =
        [".init_array", ".fini_array", ".preinit_array", ".init", ".fini", ".ctors", ".dtors"];
    if matches!(s.shdr.sh_type, SHT_INIT_ARRAY | SHT_FINI_ARRAY | SHT_PREINIT_ARRAY)
        || startup.iter().any(|p| n == *p || n.strip_prefix(p).is_some_and(|r| r.starts_with('.')))
    {
        return Some("a start-up table (image.md I-6)");
    }
    if (s.shdr.sh_type == SHT_NOTE || f & SHF_ALLOC == 0) && !alloc_group {
        return Some("a note or non-alloc section outside an allocated group (image.md I-6)");
    }
    if s.is_eh_frame(o.elf.machine()) {
        return Some(".eh_frame (image.md I-6)");
    }
    if ss.contains(n.as_bytes()) {
        return Some("a C-identifier section whose __start_/__stop_ is referenced (image.md I-6)");
    }
    None
}

/// Whether `--gc-sections`, `-T` or a group could be the reason, by the
/// record's word; `Err` the finding's text.
#[allow(clippy::too_many_arguments)]
fn judge_discard(
    o: &Obj<'_>,
    s: &Sec<'_>,
    reason: DiscardReason,
    opts: &Options,
    comdat: bool,
    alloc_group: bool,
    ss: &std::collections::BTreeSet<Vec<u8>>,
) -> Result<(), String> {
    match reason {
        DiscardReason::Comdat if comdat => Ok(()),
        DiscardReason::Comdat => Err(if s.shdr.sh_flags & SHF_GROUP == 0 {
            "discarded as 'comdat' but in no COMDAT group".into()
        } else {
            "discarded as 'comdat' but its group is the keeper (resolve.md §6.1: least FileId)"
                .into()
        }),
        DiscardReason::Gc if !opts.gc_sections => {
            Err("discarded as 'gc' but the line has no --gc-sections".into())
        }
        DiscardReason::Gc => match gc_root(o, s, alloc_group, ss) {
            Some(r) => Err(format!("discarded as 'gc' but it is a root: {r}")),
            None => Ok(()),
        },
        DiscardReason::Script if opts.script => Ok(()),
        DiscardReason::Script => Err("discarded as 'script' but the line has no -T".into()),
        DiscardReason::Unspecified => match dropped_by_row(o, s, opts) {
            Some(_) => Ok(()),
            None => Err("discarded as 'unspecified' but no row of resolve.md §10 drops it \
                         (SHF_EXCLUDE, -S/-s, .note.gnu.build-id, .gnu.lto_*)"
                .into()),
        },
    }
}

/// M10 over every section of every live object of mortar's map.
pub(crate) fn check(objs: &[Obj<'_>], map: &Map, opts: &Options) -> Vec<Finding> {
    let mut out = Vec::new();
    if map.format != Format::Mortar {
        return out;
    }
    let comdat = comdat_dropped(objs);
    let groups = in_alloc_group(objs);
    let ss = start_stop(objs);
    for (oi, o) in objs.iter().enumerate() {
        for (si, s) in o.secs.iter().enumerate() {
            if si == 0 || s.consumed() {
                continue;
            }
            let msg = match (&s.place, s.discard) {
                (Place::Discarded, Some((reason, _))) => {
                    match judge_discard(o, s, reason, opts, comdat[oi][si], groups[oi][si], &ss) {
                        Ok(()) => continue,
                        Err(m) => m,
                    }
                }
                // No record: a consumed section (I-78), an empty one (no
                // bytes, so no `place` or `discard` record need name it:
                // its labels are judged by §4.4 and Y3), or a copy of a
                // group not kept with no bytes.
                (Place::Absent, _) => {
                    if s.shdr.sh_size == 0
                        || comdat[oi][si]
                        || dropped_by_row(o, s, opts).is_some()
                        || s.is_eh_frame(o.elf.machine())
                    {
                        continue;
                    }
                    "has bytes, and the map neither places nor discards it; \
                     no row of resolve.md §10 consumes it"
                        .to_string()
                }
                _ => continue,
            };
            let line = s.discard.map_or(String::new(), |(_, l)| format!("line {l}: "));
            out.push(
                Finding::new(
                    Rule::Map(10),
                    format!(
                        "{line}section '{}' is not in the output without a reason: {msg}",
                        s.name
                    ),
                )
                .site(o.site(si, 0)),
            );
        }
    }
    out
}

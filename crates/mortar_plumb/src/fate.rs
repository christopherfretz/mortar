//! M10 (`spec/plumb.md` §3.3): a section of a live input that mortar's map
//! gives no place has a reason the inputs and the line give
//! (`spec/resolve.md` §10, `spec/image.md` I-6, I-78), derived here, not
//! taken from the linker (WP-143).

use mortar_elf::consts::{
    GRP_COMDAT, SHF_ALLOC, SHF_EXCLUDE, SHF_GNU_RETAIN, SHF_GROUP, SHF_LINK_ORDER, SHN_UNDEF,
    SHT_AARCH64_ATTRIBUTES, SHT_FINI_ARRAY, SHT_GROUP, SHT_INIT_ARRAY, SHT_LLVM_ADDRSIG,
    SHT_LLVM_CALL_GRAPH_PROFILE, SHT_LLVM_DEPENDENT_LIBRARIES, SHT_NOBITS, SHT_NOTE,
    SHT_PREINIT_ARRAY,
};
use mortar_elf::{Machine, SymbolIndex, SymbolSection};

use std::collections::{BTreeMap, BTreeSet};

use crate::locate::{Obj, Place, Sec};
use crate::map::{DiscardReason, Format, Map, OutputKind};
use crate::opts::Options;
use crate::reloc::debug::is_debug_name;
use crate::report::{Finding, Rule};
use crate::shared::{ExportRule, split_version};

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
    let mut kept: BTreeSet<Vec<u8>> = BTreeSet::new();
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
fn start_stop(objs: &[Obj<'_>]) -> BTreeSet<Vec<u8>> {
    let mut out = BTreeSet::new();
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
    ss: &BTreeSet<Vec<u8>>,
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

/// An object and a section index.
type At = (usize, usize);

/// The sections of the live objects that hold the definition of a key
/// the line roots (`spec/resolve.md` §4.2, §4.7; WP-147): the entry (`-e`,
/// else `_start` outside `-shared`), `-init`, `-fini`,
/// `--export-dynamic-symbol` (a name; a glob is not re-derived), and,
/// when the output exports (`exports`: `-shared`, or a dynamically
/// linked executable with `-E`), every global or weak of default or
/// protected visibility (no object makes its name hidden or internal)
/// that the one derivation of the export set exports: a name a version
/// script's `local:` takes out is not a root, a `.symver` key is one when
/// its node exports it (`shared::ExportRule`, D-114, resolve §9.7; WP-149). mortar refuses `-u`,
/// `--require-defined`, dynamic lists and `--exclude-libs`
/// (docs/status/options.md), so they are not in its line.
/// The definition of a key is its first strong one in object order,
/// else its first. Maps (object, section) to the name.
fn line_roots(
    objs: &[Obj<'_>],
    opts: &Options,
    exports: Option<&ExportRule>,
) -> BTreeMap<(usize, usize), String> {
    use mortar_elf::consts::{STB_LOCAL, STB_WEAK, STV_DEFAULT, STV_PROTECTED};
    let mut named: BTreeSet<String> = opts.named_roots.iter().cloned().collect();
    match &opts.entry {
        Some(e) => {
            named.insert(e.clone());
        }
        None if !opts.shared => {
            named.insert("_start".into());
        }
        None => {}
    }
    // Visibility is merged by base name, as the export set's is.
    let mut hidden: BTreeSet<String> = BTreeSet::new();
    // key -> (strong, first): (object, section)
    let mut defs: BTreeMap<String, (Option<At>, At)> = BTreeMap::new();
    for (oi, o) in objs.iter().enumerate() {
        let Some(t) = o.symtab else { continue };
        for (i, sym) in t.iter() {
            if sym.binding() == STB_LOCAL {
                continue;
            }
            let Ok(n) = t.name(&sym) else { continue };
            let n = String::from_utf8_lossy(n).into_owned();
            if !matches!(sym.visibility(), STV_DEFAULT | STV_PROTECTED) {
                hidden.insert(split_version(&n).0.to_string());
            }
            let Ok(SymbolSection::Section(k)) = t.section_of(i, &sym) else { continue };
            let at = (oi, k.0 as usize);
            let e = defs.entry(n).or_insert((None, at));
            if sym.binding() != STB_WEAK && e.0.is_none() {
                e.0 = Some(at);
            }
        }
    }
    let mut out = BTreeMap::new();
    for (n, (strong, first)) in defs {
        let why = if named.contains(&n) {
            format!("'{n}', which the line names (resolve.md §4.2, §4.7)")
        } else if exports.is_some_and(|r| r.exports(&n)) && !hidden.contains(split_version(&n).0) {
            format!("'{n}', an export (resolve.md §4.7)")
        } else {
            continue;
        };
        out.entry(strong.unwrap_or(first)).or_insert(why);
    }
    out
}

/// M10's export rule when stage 3 does not judge the output (no
/// `PT_DYNAMIC`; WP-147's maps whose `output` record says the output
/// exports): [`ExportRule::plain`] for `output shared`, or an executable's
/// with `-E`. A line with a version script has no export roots here: no
/// file of the line is read in this path, and stage 3 asks the one
/// derivation with the script (WP-149).
pub(crate) fn static_exports(map: &Map, opts: &Options) -> Option<ExportRule> {
    let d = map.dynamic.as_ref()?;
    let exports = match d.kind {
        Some(OutputKind::Shared) => true,
        Some(OutputKind::Exec | OutputKind::Pie) => opts.export_dynamic,
        _ => false,
    };
    let script = d.shared.as_ref().is_some_and(|s| !s.version_scripts.is_empty());
    (exports && !script).then(|| ExportRule::plain(map.format))
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
    ss: &BTreeSet<Vec<u8>>,
    root: Option<&String>,
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
        DiscardReason::Gc => match (gc_root(o, s, alloc_group, ss), root) {
            (Some(r), _) => Err(format!("discarded as 'gc' but it is a root: {r}")),
            (None, Some(n)) => Err(format!("discarded as 'gc' but it defines {n}: a root")),
            (None, None) => Ok(()),
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
pub(crate) fn check(
    objs: &[Obj<'_>],
    map: &Map,
    opts: &Options,
    exports: Option<&ExportRule>,
) -> Vec<Finding> {
    let mut out = Vec::new();
    if map.format != Format::Mortar {
        return out;
    }
    let comdat = comdat_dropped(objs);
    let groups = in_alloc_group(objs);
    let ss = start_stop(objs);
    let roots = line_roots(objs, opts, exports);
    for (oi, o) in objs.iter().enumerate() {
        for (si, s) in o.secs.iter().enumerate() {
            if si == 0 || s.consumed() {
                continue;
            }
            let msg = match (&s.place, s.discard) {
                (Place::Discarded, Some((reason, _))) => {
                    let root = roots.get(&(oi, si));
                    match judge_discard(
                        o,
                        s,
                        reason,
                        opts,
                        comdat[oi][si],
                        groups[oi][si],
                        &ss,
                        root,
                    ) {
                        Ok(()) => continue,
                        Err(m) => m,
                    }
                }
                // No record: a consumed section (I-78), an empty one (no
                // bytes, so no `place` or `discard` record need name it:
                // its labels are judged by §4.4 and Y3), or a copy of a
                // group not kept with no bytes.
                (Place::Absent, _) => {
                    // A `NOBITS` section has no bytes in the file: I-78's
                    // `discard` lists none, so a collected one has no
                    // record; `--gc-sections` and no root explain it.
                    let gc_nobits = s.shdr.sh_type == SHT_NOBITS
                        && opts.gc_sections
                        && gc_root(o, s, groups[oi][si], &ss).is_none()
                        && !roots.contains_key(&(oi, si));
                    if s.shdr.sh_size == 0
                        || gc_nobits
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

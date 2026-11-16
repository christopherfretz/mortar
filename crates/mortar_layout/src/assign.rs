//! Stage A (`spec/layout.md` §8): the output section of every input
//! section, written as `Assignment` (model §3.4).
//!
//! [`assign`] runs before liveness: it reads names, types and flags only,
//! and never liveness (L-43). [`finish`] runs after liveness and gives
//! each output section the type, flags and merge class of its live inputs
//! (L-47, L-49, L-50).

use std::collections::{HashMap, HashSet};

use mortar_core::assignment::{Assignment, MergeClass, OutOf, OutSec};
use mortar_core::column::Column;
use mortar_core::id::{OutSecId, SectionId, SymbolId};
use mortar_core::inputs::SymbolKey;
use mortar_core::liveness::Liveness;
use mortar_core::par::Pool;
use mortar_core::resolution::{Def, DropReason, InternalDef, Resolution, SectionState};
use mortar_diag::RefusalClass;
use mortar_elf::Machine;
use mortar_elf::consts::{
    SHF_ALLOC, SHF_EXECINSTR, SHF_GNU_RETAIN, SHF_MERGE, SHF_STRINGS, SHF_TLS, SHF_WRITE,
    SHF_X86_64_LARGE, SHT_AARCH64_ATTRIBUTES, SHT_FINI_ARRAY, SHT_INIT_ARRAY, SHT_NOBITS, SHT_NOTE,
    SHT_PREINIT_ARRAY, SHT_PROGBITS, SHT_X86_64_UNWIND,
};

use crate::diag::{LayoutDiag, feature, internal, refused, section_conflict, text};
use crate::headers::{Header, Headers};
use crate::target::{Options, Target};

/// The name rule's table (L-44): output name, prefixes in match order.
/// `.data.rel.ro` before `.data` and `.bss.rel.ro` before `.bss`.
const NAME_RULE: [&[u8]; 17] = [
    b".text",
    b".rodata",
    b".data.rel.ro",
    b".data",
    b".bss.rel.ro",
    b".bss",
    b".tdata",
    b".tbss",
    b".preinit_array",
    b".init_array",
    b".fini_array",
    b".gcc_except_table",
    b".eh_frame",
    b".init",
    b".fini",
    b".tm_clone_table",
    b".comment",
];

/// The names a synthetic section owns (L-39).
const SYNTHETIC_NAMES: [&[u8]; 14] = [
    b".got",
    b".got.plt",
    b".plt",
    b".iplt",
    b".rela.dyn",
    b".rela.plt",
    b".dynamic",
    b".dynsym",
    b".dynstr",
    b".hash",
    b".gnu.hash",
    b".eh_frame_hdr",
    b".relr.dyn",
    b".interp",
];

/// `N == P` or `N` starts with `P.` (L-44).
#[must_use]
pub fn matches_prefix(name: &[u8], prefix: &[u8]) -> bool {
    name == prefix
        || (name.len() > prefix.len() && name.starts_with(prefix) && name[prefix.len()] == b'.')
}

/// The output section name of an input section by the name rule (L-44 to
/// L-46, L-48): a table name, or the input's own name.
#[must_use]
pub fn output_name<'n>(h: &Header<'n>) -> &'n [u8] {
    // L-45: notes are not merged by name. L-46: each .debug_* is its own.
    if h.sh_type == SHT_NOTE || h.name.starts_with(b".debug_") {
        return h.name;
    }
    // §8.2: `.preinit_array.*` is an orphan (GNU's script, uniform rule).
    if h.name.starts_with(b".preinit_array.") {
        return h.name;
    }
    for out in NAME_RULE {
        if matches_prefix(h.name, out) {
            return out;
        }
    }
    h.name
}

/// What stage A does with one section (the table of §8.1, then L-44).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fate<'n> {
    None,
    Discard,
    Consumed,
    Out(&'n [u8]),
}

/// §8.1 for one section, then the name rule.
fn fate<'n>(
    h: &Header<'n>,
    row: &mortar_core::resolution::SectionRow,
    machine: Machine,
) -> Result<Fate<'n>, LayoutDiag> {
    // L-30, L-31, L-32: kinds without output (the reader's and
    // resolution's classification), and sections resolution dropped.
    if !row.kind.carries_output() {
        return Ok(Fate::None);
    }
    match row.state {
        SectionState::Kept => {}
        // L-42: a section of a discarded COMDAT group is assigned like any
        // other, without its refusals (it is not in the output).
        SectionState::Dropped(DropReason::Comdat) => {
            return Ok(match by_name(h, machine) {
                Ok(Fate::Out(name)) => Fate::Out(name),
                _ => Fate::None,
            });
        }
        SectionState::Dropped(DropReason::Script) => return Ok(Fate::Discard),
        SectionState::Dropped(_) => return Ok(Fate::None),
    }
    by_name(h, machine)
}

/// The rows of §8.1 that look at the section itself, then L-44.
fn by_name<'n>(h: &Header<'n>, machine: Machine) -> Result<Fate<'n>, LayoutDiag> {
    let n = h.name;
    // L-33: `.gnu.warning`, `.gnu.warning.SYM`.
    if matches_prefix(n, b".gnu.warning") {
        return Ok(Fate::Consumed);
    }
    // L-34: the property note is merged by synthesis.
    if n == b".note.gnu.property" {
        return Ok(Fate::Consumed);
    }
    // L-35: AArch64 build attributes.
    if machine == Machine::AArch64 && h.sh_type == SHT_AARCH64_ATTRIBUTES {
        return Ok(Fate::Consumed);
    }
    // L-36: an x86-64 large section goes to its large output section by
    // flags and type, whatever its name.
    if machine == Machine::X86_64 && h.flags & SHF_X86_64_LARGE != 0 {
        return large_output(h).map(Fate::Out);
    }
    // L-37.
    if n.starts_with(b".gnu.linkonce.") {
        return Err(feature(".gnu.linkonce sections"));
    }
    // L-38.
    if n.starts_with(b".zdebug") {
        return Err(refused(RefusalClass::CompressionType, ".zdebug"));
    }
    // L-39.
    if SYNTHETIC_NAMES.contains(&n) {
        return Err(feature(format!("input section {}", text(n))));
    }
    // L-40 (M1): non-empty `.ctors*`/`.dtors*`.
    if (matches_prefix(n, b".ctors") || matches_prefix(n, b".dtors")) && h.size > 0 {
        return Err(feature(".ctors section"));
    }
    Ok(Fate::Out(output_name(h)))
}

/// The four large output sections of L-36 (x86-64), in L-36a's order.
pub const LARGE_NAMES: [&[u8]; 4] = [b".lrodata", b".ltext", b".ldata", b".lbss"];

/// L-36: the large output section of an `SHF_X86_64_LARGE` input by its
/// flags and type: `.ltext` (exec), `.lbss` (`NOBITS`), `.ldata`
/// (writable `PROGBITS`), `.lrodata` (the rest). A TLS large section is
/// refused by name; so are the kinds no producer emits and L-36 does not
/// place: a non-allocated one, one of another `sh_type` (a note, an init
/// array, unwind tables), and a `NOBITS` one that is not writable data.
fn large_output<'n>(h: &Header<'n>) -> Result<&'n [u8], LayoutDiag> {
    let alloc = h.flags & SHF_ALLOC != 0;
    let typed = match h.sh_type {
        SHT_PROGBITS => true,
        SHT_NOBITS => h.flags & SHF_WRITE != 0 && h.flags & SHF_EXECINSTR == 0,
        _ => false,
    };
    if h.flags & SHF_TLS != 0 || !alloc || !typed {
        return Err(refused(RefusalClass::SectionFlag, "SHF_X86_64_LARGE"));
    }
    Ok(if h.flags & SHF_EXECINSTR != 0 {
        b".ltext"
    } else if h.sh_type == SHT_NOBITS {
        b".lbss"
    } else if h.flags & SHF_WRITE != 0 {
        b".ldata"
    } else {
        b".lrodata"
    })
}

/// Whether a section is a root of liveness by rule (model §3.4 `keep`):
/// `SHF_GNU_RETAIN` (L-97), the start-up arrays and `.init`/`.fini`, and
/// allocated notes. `spec/live.md` is not written; this is GNU's `KEEP`
/// set of its default script for the M1 sections.
fn keep_by_rule(h: &Header<'_>, out: &[u8]) -> bool {
    h.flags & SHF_GNU_RETAIN != 0
        || matches!(h.sh_type, SHT_INIT_ARRAY | SHT_FINI_ARRAY | SHT_PREINIT_ARRAY)
        || [&b".init"[..], b".fini", b".init_array", b".fini_array", b".preinit_array"]
            .contains(&out)
        || (h.sh_type == SHT_NOTE && h.flags & SHF_ALLOC != 0)
}

/// Whether the resolution has small commons and large ones (L-88,
/// L-36a): `(small, large)`.
fn common_kinds(resolution: &Resolution) -> (bool, bool) {
    let (mut small, mut large) = (false, false);
    for (s, d) in resolution.def.iter() {
        if matches!(d, Def::Common(_)) {
            if resolution.common_of(s).is_some_and(|c| c.large) {
                large = true;
            } else {
                small = true;
            }
        }
    }
    (small, large)
}

/// An output section's provisional row: the commons' `.bss` (and, with
/// large commons, `.lbss`) start as writable `NOBITS`; every other row
/// takes its first input's type and the OR of its inputs' flags.
fn provisional(name: &[u8], large: bool) -> OutSec<'_> {
    let lbss = large && name == b".lbss";
    OutSec {
        name,
        sh_type: if name == b".bss" || lbss { SHT_NOBITS } else { 0 },
        flags: if name == b".bss" {
            SHF_ALLOC | SHF_WRITE
        } else if lbss {
            SHF_ALLOC | SHF_WRITE | SHF_X86_64_LARGE
        } else {
            0
        },
        merge: None,
        keep: false,
        fill: None,
        constraint: None,
        script_position: None,
    }
}

/// The output of stage A.
#[derive(Debug, Clone)]
pub struct Assigned<'n> {
    pub assignment: Assignment<'n>,
}

/// Stage A: assigns every section (L-30 to L-48), interns the output
/// sections in first-appearance order (§7), adds `.bss` for commons when
/// no input names it (L-88), and confirms or withdraws the conditional
/// linker-defined symbols (resolve.md §8.1).
///
/// Parallel over files (one unit per file's section range, ordered
/// collection); the interning is one sequential pass over each file's
/// distinct names in `FileId` order.
///
/// # Errors
///
/// Every refusal of §8.1, in `SectionId` order (each with its section).
pub fn assign<'n>(
    headers: &Headers<'n>,
    resolution: &Resolution,
    symbols: &Column<SymbolId, SymbolKey<'_>>,
    target: &Target,
    options: &Options,
    pool: &Pool,
) -> Result<Assigned<'n>, Vec<(SectionId, LayoutDiag)>> {
    // Per file: each section's fate, and the file's distinct output names
    // in first-appearance order.
    struct FileFates<'n> {
        fates: Vec<Fate<'n>>,
        names: Vec<&'n [u8]>,
        errors: Vec<(SectionId, LayoutDiag)>,
    }
    let n = resolution.sections.len();
    if headers.len() != n {
        return Err(vec![(SectionId(0), internal("headers are not per section"))]);
    }
    let files = resolution.section_base.len() as usize;
    let per_file: Vec<FileFates<'n>> = pool.unit_cost(500).map(files, |f| {
        let range = resolution.section_base[mortar_core::id::FileId(f as u32)];
        let mut out = FileFates { fates: Vec::new(), names: Vec::new(), errors: Vec::new() };
        let mut seen: HashSet<&'n [u8]> = HashSet::new();
        for i in 0..range.len() {
            let s = SectionId(range.start.0 + i);
            let fate =
                fate(&headers[s], &resolution.sections[s], target.machine).unwrap_or_else(|d| {
                    out.errors.push((s, d));
                    Fate::None
                });
            if let Fate::Out(name) = fate
                && seen.insert(name)
            {
                out.names.push(name);
            }
            out.fates.push(fate);
        }
        out
    });
    let errors: Vec<(SectionId, LayoutDiag)> =
        per_file.iter().flat_map(|f| f.errors.iter().cloned()).collect();
    if !errors.is_empty() {
        return Err(errors);
    }
    // Interning (§7): ids after concatenation in FileId order.
    let mut ids: HashMap<&'n [u8], OutSecId> = HashMap::new();
    let mut names: Vec<&'n [u8]> = Vec::new();
    for f in &per_file {
        for &name in &f.names {
            ids.entry(name).or_insert_with(|| {
                names.push(name);
                OutSecId(names.len() as u32 - 1)
            });
        }
    }
    // L-88: small commons go to `.bss`, x86-64 large ones to `.lbss`.
    let (small, large) = common_kinds(resolution);
    for (has, name) in [(small, &b".bss"[..]), (large, &b".lbss"[..])] {
        if has && !ids.contains_key(name) {
            names.push(name);
            ids.insert(name, OutSecId(names.len() as u32 - 1));
        }
    }
    let mut assignment = Assignment::empty(n);
    // Provisional type and flags: the first input's type, the OR of the
    // flags of every input (finish() replaces them with the live inputs').
    // The commons' sections start as writable `NOBITS`; `.lbss` exists
    // only on x86-64 (a large common is x86-64's, resolve.md §3.3).
    let out_flags = out_flags(target.machine);
    let mut outsecs: Vec<OutSec<'n>> = names.iter().map(|&name| provisional(name, large)).collect();
    let mut s = 0u32;
    for f in &per_file {
        for fate in &f.fates {
            let id = SectionId(s);
            let h = &headers[id];
            assignment.out_of[id] = match *fate {
                Fate::None => OutOf::None,
                Fate::Discard => OutOf::Discard,
                Fate::Consumed => OutOf::Consumed,
                Fate::Out(name) => {
                    let o = ids[name];
                    let row = &mut outsecs[o.0 as usize];
                    if row.sh_type == 0 {
                        row.sh_type = h.sh_type;
                    }
                    row.flags |= h.flags & out_flags;
                    if resolution.sections[id].is_kept() && keep_by_rule(h, name) {
                        assignment.keep[id] = true;
                    }
                    OutOf::Out(o)
                }
            };
            s += 1;
        }
    }
    assignment.outsecs = Column::from_vec(outsecs)
        .map_err(|_| vec![(SectionId(0), internal("too many outputs"))])?;
    assignment.withdrawn = withdrawn(resolution, symbols, &assignment, options).into();
    Ok(Assigned { assignment })
}

/// The output flags of L-50, and on x86-64 `SHF_X86_64_LARGE` (L-36:
/// the large output sections keep it).
const fn out_flags(machine: Machine) -> u64 {
    let base = SHF_WRITE | SHF_ALLOC | SHF_EXECINSTR | SHF_TLS | SHF_GNU_RETAIN;
    if matches!(machine, Machine::X86_64) { base | SHF_X86_64_LARGE } else { base }
}

/// Whether a name is a C identifier (`__start_`/`__stop_`, L-48).
#[must_use]
pub fn is_c_identifier(name: &[u8]) -> bool {
    !name.is_empty()
        && !name[0].is_ascii_digit()
        && name.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'_')
}

/// The conditional linker-defined symbols withdrawn (resolve.md §8.1),
/// sorted: `__start_SEC`/`__stop_SEC` when no input section is assigned
/// to an output section `SEC` that is a C identifier; `__GNU_EH_FRAME_HDR`
/// when there is no `.eh_frame_hdr` (no `--eh-frame-hdr` or no
/// `.eh_frame`). `_TLS_MODULE_BASE_` is never withdrawn here: with no TLS
/// chunk L-98 refuses it by name.
fn withdrawn(
    resolution: &Resolution,
    symbols: &Column<SymbolId, SymbolKey<'_>>,
    assignment: &Assignment<'_>,
    options: &Options,
) -> Vec<SymbolId> {
    let has_out = |name: &[u8]| assignment.outsecs.iter().any(|(_, o)| o.name == name);
    let mut out = Vec::new();
    for (s, def) in resolution.def.iter() {
        if !matches!(def, Def::Internal(InternalDef::Provide { conditional: true })) {
            continue;
        }
        let Some(key) = symbols.get(s) else { continue };
        let name = key.name;
        let keep = if let Some(sec) =
            name.strip_prefix(b"__start_").or_else(|| name.strip_prefix(b"__stop_"))
        {
            is_c_identifier(sec) && has_out(sec)
        } else if name == b"__GNU_EH_FRAME_HDR" {
            options.eh_frame_hdr && has_out(b".eh_frame")
        } else {
            true
        };
        if !keep {
            out.push(s);
        }
    }
    out
}

/// What [`finish`] decides per output section from its live inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutInfo {
    /// Live members (input sections placed in it).
    pub live: u32,
    /// log2 of the largest live member alignment.
    pub align: u8,
}

/// The class word of a section for `section-conflict` (§20.4).
fn class_words(alloc: bool) -> &'static str {
    if alloc { "allocated" } else { "non-allocated" }
}

/// The merge class of the live inputs seen so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MergeAcc {
    Unseen,
    /// Every input so far has this class (`None`: not a merge input);
    /// a mixture is `Uniform(None)`.
    Uniform(Option<MergeClass>),
}

/// What [`finish`] accumulates per output section.
#[derive(Debug, Clone)]
struct Acc {
    ty: u32,
    all_unwind: bool,
    flags: u64,
    merge: MergeAcc,
    alloc: Option<bool>,
    tls: Option<bool>,
    large: Option<bool>,
    live: u32,
    align: u8,
    error: Option<(String, String)>,
}

impl Acc {
    const BLANK: Self = Self {
        ty: 0,
        all_unwind: true,
        flags: 0,
        merge: MergeAcc::Unseen,
        alloc: None,
        tls: None,
        large: None,
        live: 0,
        align: 0,
        error: None,
    };

    /// One live input `h`, in `SectionId` order.
    fn add(&mut self, h: &Header<'_>, target: &Target) {
        if self.error.is_some() {
            return;
        }
        self.live += 1;
        self.align = self.align.max(h.align);
        let alloc = h.flags & SHF_ALLOC != 0;
        let tls = h.flags & SHF_TLS != 0;
        match self.alloc {
            Some(was) if was != alloc => {
                self.error = Some((class_words(was).into(), class_words(alloc).into()));
                return;
            }
            _ => self.alloc = Some(alloc),
        }
        match self.tls {
            Some(was) if was != tls => {
                let word = |t: bool| if t { "TLS" } else { "non-TLS" };
                self.error = Some((word(was).into(), word(tls).into()));
                return;
            }
            _ => self.tls = Some(tls),
        }
        // L-36: a large output section holds large inputs only (an input
        // without the flag named like one, `.ldata` say, is not moved
        // after every small section by its name).
        let out_flags = out_flags(target.machine);
        let large = h.flags & out_flags & SHF_X86_64_LARGE != 0;
        match self.large {
            Some(was) if was != large => {
                let word = |l: bool| if l { "SHF_X86_64_LARGE" } else { "not SHF_X86_64_LARGE" };
                self.error = Some((word(was).into(), word(large).into()));
                return;
            }
            _ => self.large = Some(large),
        }
        self.flags |= h.flags & out_flags;
        let class = (h.flags & SHF_MERGE != 0)
            .then_some(MergeClass { strings: h.flags & SHF_STRINGS != 0, entsize: h.entsize });
        self.merge = MergeAcc::Uniform(match self.merge {
            MergeAcc::Unseen => class,
            MergeAcc::Uniform(prev) if prev == class => prev,
            MergeAcc::Uniform(_) => None,
        });
        self.all_unwind &= h.sh_type == SHT_X86_64_UNWIND;
        if self.ty == 0 {
            self.ty = h.sh_type;
            return;
        }
        match combine(self.ty, h.sh_type, target.machine) {
            Some(t) => self.ty = t,
            None => {
                self.error = Some((type_name(self.ty, target), type_name(h.sh_type, target)));
            }
        }
    }
}

/// Stage A, after liveness: each output section's `sh_type`, flags and
/// merge class from its live inputs (L-47, L-49, L-50), and the refusals
/// of mixtures (L-47, L-49, L-61), plus `W+X` (no rank holds it).
///
/// # Errors
///
/// The conflicts, in `OutSecId` order.
pub fn finish(
    assignment: &mut Assignment<'_>,
    headers: &Headers<'_>,
    liveness: &Liveness,
    target: &Target,
) -> Result<Column<OutSecId, OutInfo>, Vec<LayoutDiag>> {
    let mut acc = vec![Acc::BLANK; assignment.outsecs.len() as usize];
    for (s, out) in assignment.out_of.iter() {
        let Some(out) = out.out() else { continue };
        if liveness.live.get(s).copied().unwrap_or(false) {
            acc[out.0 as usize].add(&headers[s], target);
        }
    }
    let mut errors = Vec::new();
    let mut info = Vec::with_capacity(acc.len());
    for (i, a) in acc.iter().enumerate() {
        let row = &mut assignment.outsecs[OutSecId(i as u32)];
        info.push(OutInfo { live: a.live, align: a.align });
        let name = text(row.name);
        if let Some((first, second)) = &a.error {
            errors.push(section_conflict(name, first, second));
            continue;
        }
        if a.live == 0 {
            continue;
        }
        let mut ty = a.ty;
        // L-49: PROGBITS and NOBITS give PROGBITS; UNWIND only when all are.
        if ty == SHT_X86_64_UNWIND && !a.all_unwind {
            ty = SHT_PROGBITS;
        }
        row.sh_type = ty;
        row.flags = a.flags;
        row.merge = match a.merge {
            MergeAcc::Uniform(c) => c,
            MergeAcc::Unseen => None,
        };
        if a.flags & SHF_ALLOC != 0 && a.flags & SHF_WRITE != 0 && a.flags & SHF_EXECINSTR != 0 {
            errors.push(feature(format!("writable and executable output section {name}")));
        } else if ty == SHT_NOBITS && a.flags & SHF_EXECINSTR != 0 {
            // L-61: an exec NOBITS orphan.
            errors.push(section_conflict(name, "SHT_NOBITS", "executable"));
        }
    }
    if errors.is_empty() {
        Column::from_vec(info).map_err(|_| vec![internal("too many outputs")])
    } else {
        Err(errors)
    }
}

/// L-49's compatible mixtures, or `None`.
fn combine(a: u32, b: u32, machine: Machine) -> Option<u32> {
    let special = |t| matches!(t, SHT_INIT_ARRAY | SHT_FINI_ARRAY | SHT_PREINIT_ARRAY);
    let progbits_like = |t| matches!(t, SHT_PROGBITS | SHT_NOBITS);
    if a == b {
        return Some(a);
    }
    if progbits_like(a) && progbits_like(b) {
        return Some(SHT_PROGBITS);
    }
    if special(a) && b == SHT_PROGBITS {
        return Some(a);
    }
    if special(b) && a == SHT_PROGBITS {
        return Some(b);
    }
    if machine == Machine::X86_64 {
        let unwind = |t| t == SHT_X86_64_UNWIND;
        if (unwind(a) && b == SHT_PROGBITS) || (a == SHT_PROGBITS && unwind(b)) {
            // Kept as UNWIND while any input is; finish() lowers it to
            // PROGBITS unless every live input is UNWIND.
            return Some(SHT_X86_64_UNWIND);
        }
    }
    None
}

fn type_name(t: u32, target: &Target) -> String {
    let em = match target.machine {
        Machine::AArch64 => mortar_elf::consts::EM_AARCH64,
        Machine::X86_64 => mortar_elf::consts::EM_X86_64,
        Machine::I386 => mortar_elf::consts::EM_386,
    };
    mortar_elf::consts::sht_name(em, t).map_or_else(|| format!("{t:#x}"), str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(name: &'static [u8], sh_type: u32, flags: u64) -> Header<'static> {
        Header { name, sh_type, flags, size: 8, align: 3, link: None, entsize: 0 }
    }

    // spec: layout.md §8.2 L-44 (prefix match: N == P or N starts with P.)
    #[test]
    fn l44_prefix() {
        assert!(matches_prefix(b".text", b".text"));
        assert!(matches_prefix(b".text.unlikely.f", b".text"));
        assert!(!matches_prefix(b".textual", b".text"));
        assert!(!matches_prefix(b".tex", b".text"));
        let name = |n| output_name(&h(n, SHT_PROGBITS, SHF_ALLOC));
        assert_eq!(name(b".text.startup.main"), b".text");
        assert_eq!(name(b".data.rel.ro.local.x"), b".data.rel.ro");
        assert_eq!(name(b".data.x"), b".data");
        assert_eq!(name(b".data1"), b".data1", "an orphan (§8.2)");
        assert_eq!(name(b".rodata1"), b".rodata1");
        assert_eq!(name(b".bss.rel.ro"), b".bss.rel.ro");
        assert_eq!(name(b".init_array.00100"), b".init_array");
        assert_eq!(name(b".preinit_array.5"), b".preinit_array.5", "an orphan (§8.2)");
        assert_eq!(name(b".tm_clone_table"), b".tm_clone_table");
        assert_eq!(name(b"my_section"), b"my_section", "L-48 orphan");
    }

    // spec: layout.md §8.2 L-45, L-46 (notes and .debug_* keep their names)
    #[test]
    fn l45_l46_own_names() {
        assert_eq!(output_name(&h(b".note.ABI-tag", SHT_NOTE, SHF_ALLOC)), b".note.ABI-tag");
        assert_eq!(output_name(&h(b".text.note", SHT_NOTE, SHF_ALLOC)), b".text.note");
        assert_eq!(output_name(&h(b".debug_info", SHT_PROGBITS, 0)), b".debug_info");
    }

    // spec: layout.md §8.2 L-48 (orphan __start_/__stop_ names are C identifiers)
    #[test]
    fn l48_c_identifier() {
        assert!(is_c_identifier(b"my_section"));
        assert!(is_c_identifier(b"_x9"));
        assert!(!is_c_identifier(b".data"));
        assert!(!is_c_identifier(b"9x"));
        assert!(!is_c_identifier(b""));
    }

    // spec: layout.md §8.3 L-49 (compatible type mixtures)
    #[test]
    fn l49_combine() {
        use mortar_elf::consts::SHT_NOTE;
        let a = Machine::AArch64;
        assert_eq!(combine(SHT_PROGBITS, SHT_NOBITS, a), Some(SHT_PROGBITS));
        assert_eq!(combine(SHT_INIT_ARRAY, SHT_PROGBITS, a), Some(SHT_INIT_ARRAY));
        assert_eq!(combine(SHT_PROGBITS, SHT_FINI_ARRAY, a), Some(SHT_FINI_ARRAY));
        assert_eq!(combine(SHT_NOTE, SHT_PROGBITS, a), None);
        assert_eq!(combine(SHT_INIT_ARRAY, SHT_FINI_ARRAY, a), None);
        assert_eq!(combine(SHT_X86_64_UNWIND, SHT_PROGBITS, a), None, "x86-64 only");
        let x = Machine::X86_64;
        assert_eq!(combine(SHT_X86_64_UNWIND, SHT_PROGBITS, x), Some(SHT_X86_64_UNWIND));
    }
}

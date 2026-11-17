//! Layout at `-r` (`spec/layout.md` §24, `spec/model.md` §4.6 MD-R1).
//! Owner: **RL-3** (layout at `-r`): L-R1 to L-R5, L-R7, L-R8, L-R10;
//! MD-R1's chunks; L-R6's decisions (which FDEs are removed) as data.
//! RL-3 also owns the `-r` branches of `assign.rs` and `order.rs`, and
//! L-R10's checks in `mortar_core/src/check/relocatable.rs`. After it:
//! RL-7 (L-R6a, the stack-note function only), RL-8 (the group and eh
//! parts), RL-11 (`--unique`'s key, L-R11), RL-12 (the Mason container's
//! function only), one after the other.
//!
//! WP-183 (RL-3) filled [`layout`]. Its steps:
//! 1. the role of every input section (placed whole, an `.eh_frame`
//!    input, an input property note, a kept group, nothing);
//! 2. the kept groups and their members (L-R2, L-R3);
//! 3. `out_of` by L-R1's key (L-R1, L-R2) and the output sections' type,
//!    flags and alignment (L-R1a);
//! 4. the pieces within each output section (L-R4, L-R5);
//! 5. the removed `.eh_frame` records ([`eh_removed`], L-R6) and the
//!    relocation records per output section (`spec/image.md` I-R5);
//! 6. the chunks, L-R8's order and the section indexes;
//! 7. the `.symtab` entries (I-R4) and the sizes of the synthetic chunks;
//! 8. the file offsets (L-R8) and the section header table.
//!
//! Every size RL-4 must agree with is computed by a small pure function
//! of this module ([`eh_frame_size`], [`symtab_size`], [`strtab_size`],
//! [`shstrtab_size`], [`symtab_shndx_size`], [`relocs_size`],
//! [`group_size`]), from the rulings of the supervisor's `rl-sizes`
//! (S1..S16, record `spec/landed/layout/WP-183.md`).

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use mortar_core::assignment::{Assignment, MergeClass, OutOf, OutSec};
use mortar_core::column::Column;
use mortar_core::id::{ChunkId, FileId, OptId, OutSecId, SectionId, SymbolId};
use mortar_core::inputs::Reader;
use mortar_core::layout::{Chunk, ChunkKind, Layout, Part, Place, Placement, PlacementKind, Synth};
use mortar_core::par::Pool;
use mortar_core::plan::Plan;
use mortar_core::resolution::{Def, DropReason, RefFlags, SectionKind, SectionState};
use mortar_core::split::Split;
use mortar_core::{Inputs, Liveness, RelocatableLayout, Resolution};
use mortar_diag::RefusalClass;
use mortar_diag::relocatable::refused_with_r;
use mortar_elf::consts::{
    SHF_ALLOC, SHF_COMPRESSED, SHF_EXCLUDE, SHF_EXECINSTR, SHF_GNU_RETAIN, SHF_GROUP,
    SHF_INFO_LINK, SHF_LINK_ORDER, SHF_MERGE, SHF_STRINGS, SHF_TLS, SHF_WRITE, SHF_X86_64_LARGE,
    SHT_LLVM_ADDRSIG, SHT_LLVM_CALL_GRAPH_PROFILE, SHT_NOTE, SHT_PROGBITS, SHT_REL, SHT_RELA,
    SHT_STRTAB, SHT_SYMTAB, SHT_SYMTAB_SHNDX, SHT_X86_64_UNWIND, STT_FILE, STT_SECTION,
};
use mortar_elf::{Class, Machine, Object, SectionIndex, SymbolIndex, SymbolSection, SymbolTable};

use crate::assign::{combine, type_name};
use crate::diag::{feature, internal, refused, section_conflict, text};
use crate::headers::{Header, Headers, gather};
use crate::{LayoutDiag, Target};

/// The sizes of the two notes a `-r` output may get that `mortar_synth`
/// plans (`rl-sizes` S9, S10, S16): the join (RL-7) computes them with
/// `PropertyPlan::size` and the build-id plan's size, so that
/// `mortar_synth` stays their single source. `None`: no such note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NoteSizes {
    /// The merged `.note.gnu.property` (S10): `Some` exactly when the
    /// property plan over the live inputs' notes writes a note.
    pub property: Option<u64>,
    /// `.note.gnu.build-id` under `--build-id` (I-R8, S9).
    pub build_id: Option<u64>,
}

/// The layout of a `-r` link: the assignment by L-R1/L-R2 and the
/// placement of MD-R1 (`PlacementKind::Relocatable`, no segment, no
/// address, L-R8's order, file offsets), with L-R6's removed `.eh_frame`
/// records as data. `Layout::check` (L-R10) holds of the result. RL-7's
/// driver calls it at `-r` in place of `assign::assign` and `place`, and
/// hands the result to `mortar_image::relocatable::write`.
///
/// WP-183 added `split` (the `.eh_frame` records of L-R6) and `notes`
/// (the sizes `mortar_synth` plans) to WP-181's signature (`rl-sizes`
/// S16).
///
/// # Errors
///
/// Every diagnostic of the layout: `section-conflict` (L-R1a), the
/// refusals by name of what a later package lifts (`-d with -r`,
/// `--force-group-allocation with -r`, the Mason container, an
/// `.eh_frame` that would be two output sections, ...), and
/// `internal-error` when the tables break the `-r` contract (a `Stack` or
/// `SHF_EXCLUDE` section that resolution did not classify for `-r`,
/// `rl-sizes` S13).
#[allow(clippy::too_many_arguments, reason = "the tables of MD-R1, each read")]
pub fn layout<'s>(
    inputs: &Inputs<'s>,
    resolution: &Resolution,
    liveness: &Liveness,
    split: &Split,
    target: &Target,
    plan: &Plan,
    notes: NoteSizes,
    pool: &Pool,
) -> Result<RelocatableLayout<'s>, Vec<LayoutDiag>> {
    // L-R4: `-d`'s commons are RL-11's; L-R2/L-R3 under
    // `--force-group-allocation` are RL-8's.
    let mut refusals = Vec::new();
    if plan.define_common {
        refusals.push(LayoutDiag::from(refused_with_r("-d")));
    }
    if plan.force_group_allocation {
        refusals.push(LayoutDiag::from(refused_with_r("--force-group-allocation")));
    }
    if !refusals.is_empty() {
        return Err(refusals);
    }
    let headers = gather(inputs, resolution, pool).map_err(|d| vec![d])?;
    let objects = objects(inputs, resolution)?;
    let roles = roles(resolution, liveness, &headers, target)?;
    let groups = kept_groups(&objects, resolution, &roles)?;
    let assigned = assign(resolution, &headers, &roles, &groups, target)?;
    let removed = eh_removed(split, &assigned.eh_inputs, |s| {
        matches!(
            resolution.sections.get(s).map(|r| r.state),
            Some(SectionState::Dropped(DropReason::Comdat | DropReason::Stripped))
        )
    });
    let records = relocation_records(
        &objects,
        inputs,
        resolution,
        &headers,
        &assigned,
        &removed,
        target.class,
    )?;
    let removed_sizes = removed_record_sizes(&objects, inputs, resolution, &removed)?;
    build(&Build {
        inputs,
        objects: &objects,
        resolution,
        headers: &headers,
        groups: &groups,
        assigned,
        records,
        removed,
        removed_sizes,
        target,
        notes,
        pool,
    })
}

// ---------------------------------------------------------------------
// The pure sizes (rl-sizes S1..S9, S12): RL-7 makes one of RL-3's and
// RL-4's the single source and tests the two against each other.
// ---------------------------------------------------------------------

/// The fixed head of `.strtab` that `mortar_synth` writes (S-86, `rl-sizes`
/// S2): the empty name, `$x`, `$d` and `__erratum_843419_veneer`.
pub const STRTAB_FIXED: &[u8] = b"\0$x\0$d\0__erratum_843419_veneer\0";

/// The name of one `.symtab` entry as `.strtab` writes it (`rl-sizes` S2):
/// `name`, then `@` and `version` when the entry is an undefined or common
/// key with a version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryName<'a> {
    pub name: &'a [u8],
    pub version: Option<&'a [u8]>,
}

impl EntryName<'_> {
    /// The bytes the name takes in `.strtab`: 0 for a name of the fixed
    /// table, else its length and the NUL.
    #[must_use]
    pub fn strtab_bytes(&self) -> u64 {
        match self.version {
            Some(v) => self.name.len() as u64 + 1 + v.len() as u64 + 1,
            None if in_fixed_table(self.name) => 0,
            None => self.name.len() as u64 + 1,
        }
    }
}

fn in_fixed_table(name: &[u8]) -> bool {
    matches!(name, b"" | b"$x" | b"$d" | b"__erratum_843419_veneer")
}

/// `.strtab`'s size (`rl-sizes` S2): the 31 bytes of [`STRTAB_FIXED`],
/// then each entry's name and a NUL in `.symtab` order, except the names
/// of the fixed table, which point into it. No other sharing.
#[must_use]
pub fn strtab_size<'a>(names: impl IntoIterator<Item = EntryName<'a>>) -> u64 {
    STRTAB_FIXED.len() as u64 + names.into_iter().map(|n| n.strtab_bytes()).sum::<u64>()
}

/// The name of one section in `.shstrtab` (`rl-sizes` S3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionName<'a> {
    /// An output section's own name, `.group`, or a synthetic chunk's.
    Plain(&'a [u8]),
    /// A relocation section: `.rela` (`.rel` when `rel`, i386) followed by
    /// its target's name.
    Relocs { rel: bool, target: &'a [u8] },
}

impl SectionName<'_> {
    /// The name's bytes.
    #[must_use]
    pub fn bytes(&self) -> Cow<'_, [u8]> {
        match *self {
            Self::Plain(n) => Cow::Borrowed(n),
            Self::Relocs { rel, target } => {
                let prefix: &[u8] = if rel { b".rel" } else { b".rela" };
                Cow::Owned([prefix, target].concat())
            }
        }
    }
}

/// `.shstrtab`'s size (`rl-sizes` S3): a NUL, then each section's name and
/// a NUL in section index order, a name already written (exactly the same
/// bytes; the empty name is the leading NUL) not written again. No suffix
/// sharing.
#[must_use]
pub fn shstrtab_size<'a>(names: impl IntoIterator<Item = SectionName<'a>>) -> u64 {
    let mut written: HashSet<Vec<u8>> = HashSet::new();
    written.insert(Vec::new());
    let mut size = 1u64;
    for n in names {
        let bytes = n.bytes();
        if !written.contains(bytes.as_ref()) {
            size += bytes.len() as u64 + 1;
            written.insert(bytes.into_owned());
        }
    }
    size
}

/// The size of one `.symtab` entry: 24 (ELF64) or 16 (ELF32).
#[must_use]
pub const fn sym_entry_size(class: Class) -> u64 {
    match class {
        Class::Elf64 => 24,
        Class::Elf32 => 16,
    }
}

/// The number of `.symtab` entries (`rl-sizes` S4): the null entry, one
/// `STT_SECTION` symbol per section that has one, the kept locals with
/// the `STT_FILE` entries, and the written globals.
#[must_use]
pub const fn symtab_entries(section_symbols: u64, locals: u64, globals: u64) -> u64 {
    1 + section_symbols + locals + globals
}

/// `.symtab`'s size: entries × [`sym_entry_size`].
#[must_use]
pub const fn symtab_size(class: Class, entries: u64) -> u64 {
    entries * sym_entry_size(class)
}

/// `.symtab_shndx`'s size (`rl-sizes` S5): `Some(4 × entries)` when some
/// written entry's output section index is `SHN_LORESERVE` (0xff00) or
/// more (`needed`), else `None`: no such section.
#[must_use]
pub const fn symtab_shndx_size(needed: bool, entries: u64) -> Option<u64> {
    if needed { Some(4 * entries) } else { None }
}

/// Whether an output section index needs `.symtab_shndx` (S5).
#[must_use]
pub const fn needs_xindex(index: u32) -> bool {
    index >= 0xff00
}

/// The size of one output relocation record: 24 (`SHT_RELA`, ELF64) or 8
/// (`SHT_REL`, ELF32: i386).
#[must_use]
pub const fn reloc_entry_size(class: Class) -> u64 {
    match class {
        Class::Elf64 => 24,
        Class::Elf32 => 8,
    }
}

/// A `Relocs(o)` chunk's size (`rl-sizes` S6): `None` (no chunk) for no
/// record, else records × [`reloc_entry_size`].
#[must_use]
pub const fn relocs_size(class: Class, records: u64) -> Option<u64> {
    if records == 0 { None } else { Some(records * reloc_entry_size(class)) }
}

/// A `Group(g)` chunk's size (`rl-sizes` S8): 4 × (the flag word + the
/// member output sections + the `Relocs` chunks of those members).
#[must_use]
pub const fn group_size(members: u64, member_relocs: u64) -> u64 {
    4 * (1 + members + member_relocs)
}

/// The `.eh_frame` chunk's size (`rl-sizes` S12): the sum of the live
/// `EhFrame` inputs' `sh_size` minus the sizes of the removed records
/// (each 4 + its length word).
#[must_use]
pub fn eh_frame_size(
    input_sizes: impl IntoIterator<Item = u64>,
    removed_sizes: impl IntoIterator<Item = u64>,
) -> u64 {
    let total: u64 = input_sizes.into_iter().sum();
    total - removed_sizes.into_iter().sum::<u64>()
}

/// The size of the file header: 64 (ELF64) or 52 (ELF32).
#[must_use]
pub const fn file_header_size(class: Class) -> u64 {
    match class {
        Class::Elf64 => 64,
        Class::Elf32 => 52,
    }
}

/// The size of one section header: 64 (ELF64) or 40 (ELF32).
#[must_use]
pub const fn section_header_size(class: Class) -> u64 {
    match class {
        Class::Elf64 => 64,
        Class::Elf32 => 40,
    }
}

/// log2 of the word size: 3 (ELF64) or 2 (ELF32): the alignment of
/// `.symtab`, of the relocation sections and of the section header table.
#[must_use]
pub const fn word_align(class: Class) -> u8 {
    match class {
        Class::Elf64 => 3,
        Class::Elf32 => 2,
    }
}

// ---------------------------------------------------------------------
// L-R6's decisions.
// ---------------------------------------------------------------------

/// L-R6 as data: the FDEs to remove, as `(input .eh_frame section, offset
/// of the record)`, sorted. An FDE is removed when its code section
/// (`Split`'s `code`, from its first relocation) is `dropped`: a COMDAT
/// copy that lost (`spec/resolve.md` RV-R7) or a section `-S`-style
/// stripping removed. CIEs and terminators are never removed; an FDE
/// without a code section is kept. `eh_sections` are the live `EhFrame`
/// inputs.
#[must_use]
pub fn eh_removed(
    split: &Split,
    eh_sections: &[SectionId],
    dropped: impl Fn(SectionId) -> bool,
) -> Box<[(SectionId, u64)]> {
    let mut out = Vec::new();
    for &s in eh_sections {
        let Some(span) = split.eh_pieces(s) else { continue };
        for p in span.slots() {
            let piece = split.eh_piece[p];
            if piece.is_cie() {
                continue;
            }
            if piece.code.get().is_some_and(&dropped) {
                out.push((s, u64::from(split.eh_off[p])));
            }
        }
    }
    out.sort_unstable();
    out.into_boxed_slice()
}

// ---------------------------------------------------------------------
// Step 1: the roles.
// ---------------------------------------------------------------------

/// What `-r` does with an input section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    /// Nothing: not in the output (dropped, consumed, a table).
    None,
    /// Placed whole in an output section's body (L-R4, L-R5).
    Piece,
    /// A live `.eh_frame` input: its records go to the synthetic
    /// `.eh_frame` chunk (S1, S12).
    Eh,
    /// A live input `.note.gnu.property`, consumed into the synthetic note
    /// (S10).
    Property,
}

/// The flags an output section can carry at `-r` (L-R1a), on `machine`.
const fn known_flags(machine: Machine) -> u64 {
    let base = SHF_WRITE
        | SHF_ALLOC
        | SHF_EXECINSTR
        | SHF_MERGE
        | SHF_STRINGS
        | SHF_INFO_LINK
        | SHF_LINK_ORDER
        | SHF_GROUP
        | SHF_TLS
        | SHF_COMPRESSED
        | SHF_GNU_RETAIN
        | SHF_EXCLUDE;
    if matches!(machine, Machine::X86_64) { base | SHF_X86_64_LARGE } else { base }
}

/// Whether an `Excluded` section is one L-R1 drops at `-r` (or one a
/// refusal elsewhere covers: GCC's LTO sections, RV-R9).
fn dropped_at_r(h: &Header<'_>) -> bool {
    matches!(h.sh_type, SHT_LLVM_ADDRSIG | SHT_LLVM_CALL_GRAPH_PROFILE)
        || h.name == b".note.gnu.build-id"
        || h.name.starts_with(b".gnu.lto_")
        || h.name.starts_with(b".gnu.debuglto_")
}

fn roles(
    resolution: &Resolution,
    liveness: &Liveness,
    headers: &Headers<'_>,
    target: &Target,
) -> Result<Column<SectionId, Role>, Vec<LayoutDiag>> {
    let mut roles = Vec::with_capacity(resolution.sections.len() as usize);
    let mut errors = Vec::new();
    let mut meta = false;
    for (s, row) in resolution.sections.iter() {
        let h = &headers[s];
        let live = row.is_kept() && liveness.live.get(s).copied().unwrap_or(false);
        let role = match row.kind {
            // S13: what a final link consumes and `-r` copies reaches
            // layout with a placed kind; anything else is a broken
            // contract, never a silent drop.
            SectionKind::Stack => {
                errors.push(internal(format!(
                    "{} reached layout at -r as a stack note: resolution at -r copies it \
                     (spec/layout.md L-R1)",
                    text(h.name)
                )));
                Role::None
            }
            SectionKind::Excluded if h.flags & SHF_EXCLUDE != 0 && !dropped_at_r(h) => {
                errors.push(internal(format!(
                    "SHF_EXCLUDE section {} reached layout at -r excluded: resolution at -r \
                     copies it (spec/layout.md L-R1)",
                    text(h.name)
                )));
                Role::None
            }
            SectionKind::Meta => {
                meta = true;
                Role::None
            }
            SectionKind::EhFrame if live => Role::Eh,
            SectionKind::Note if live && h.name == b".note.gnu.property" => Role::Property,
            SectionKind::Regular
            | SectionKind::Nobits
            | SectionKind::Merge
            | SectionKind::Note
            | SectionKind::Debug
                if live =>
            {
                if let Err(e) = placeable(h, target) {
                    errors.push(e);
                }
                Role::Piece
            }
            _ => Role::None,
        };
        roles.push(role);
    }
    if meta {
        // MT-30 is RL-12's.
        errors.push(feature("the Mason container at -r (RL-12)"));
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Column::from_vec(roles).map_err(|_| vec![internal("too many sections")])
}

/// The refusals of §8.1 that stay at `-r` (L-R1): `.gnu.linkonce.*`
/// (L-37), `.zdebug` (L-38); and the flags L-R1a cannot carry.
fn placeable(h: &Header<'_>, target: &Target) -> Result<(), LayoutDiag> {
    if h.name.starts_with(b".gnu.linkonce.") {
        return Err(feature(".gnu.linkonce sections"));
    }
    if h.name.starts_with(b".zdebug") {
        return Err(refused(RefusalClass::CompressionType, ".zdebug"));
    }
    let unknown = h.flags & !known_flags(target.machine);
    if unknown != 0 {
        return Err(refused(
            RefusalClass::SectionFlag,
            format!("section flags {unknown:#x} of {} at -r", text(h.name)),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------
// The readers.
// ---------------------------------------------------------------------

/// Per file, its object and symbol table (`None` for a file with no
/// sections in the resolution: not a live object).
struct FileObject<'s> {
    object: Object<'s>,
    symbols: Option<SymbolTable<'s>>,
}

fn objects<'s>(
    inputs: &Inputs<'s>,
    resolution: &Resolution,
) -> Result<Vec<Option<FileObject<'s>>>, Vec<LayoutDiag>> {
    let mut out = Vec::with_capacity(resolution.section_base.len() as usize);
    for (f, range) in resolution.section_base.iter() {
        if range.is_empty() || !resolution.file_live.get(f).is_some_and(|l| l.is_live()) {
            out.push(None);
            continue;
        }
        let Some(Reader::Object(object)) = inputs.files.get(f).map(|x| x.reader) else {
            return Err(vec![internal(format!("file {} has sections but no object reader", f.0))]);
        };
        let symbols = object
            .symbols()
            .map_err(|e| vec![internal(format!("file {}: symbol table: {e}", f.0))])?;
        out.push(Some(FileObject { object, symbols }));
    }
    Ok(out)
}

/// The bytes `range` (offsets in the section) of section `index` of
/// `file`, resident or copied (a parts view, `spec/elf.md` §3.0).
fn section_bytes<'s>(
    inputs: &Inputs<'s>,
    object: &Object<'s>,
    file: FileId,
    index: SectionIndex,
    range: std::ops::Range<u64>,
) -> Result<Cow<'s, [u8]>, String> {
    let s = object.section(index).map_err(|e| e.to_string())?;
    let at = s.header().sh_offset;
    if let Ok(data) = s.data()
        && let Some(b) = data.get(range.start as usize..range.end as usize)
    {
        return Ok(Cow::Borrowed(b));
    }
    let mut buf = vec![0u8; (range.end - range.start) as usize];
    inputs
        .copy_to(file, at + range.start..at + range.end, &mut buf)
        .map_err(|e| format!("{e:?}"))?;
    Ok(Cow::Owned(buf))
}

// ---------------------------------------------------------------------
// Step 2: the kept groups (L-R2, L-R3).
// ---------------------------------------------------------------------

/// One kept group: its input `SHT_GROUP` section (the chunk's
/// `Group(section)`), its members in input order, and its signature.
#[derive(Debug, Clone)]
struct KeptGroup {
    section: SectionId,
    /// The members in the input member order (L-R3), each placed whole
    /// (relocation sections and members without a place left out).
    members: Vec<SectionId>,
    /// The signature symbol's index in the file's table.
    signature: SymbolIndex,
}

fn kept_groups(
    objects: &[Option<FileObject<'_>>],
    resolution: &Resolution,
    roles: &Column<SectionId, Role>,
) -> Result<Vec<KeptGroup>, Vec<LayoutDiag>> {
    let mut groups = Vec::new();
    let mut errors = Vec::new();
    for (s, row) in resolution.sections.iter() {
        if row.kind != SectionKind::Group {
            continue;
        }
        let Some((file, index)) = resolution.sh_index(s) else { continue };
        let Some(Some(fo)) = objects.get(file.0 as usize) else { continue };
        let bad = |e: String| internal(format!("file {} group section {}: {e}", file.0, index.0));
        let g = match fo.object.section(index).and_then(|x| x.group()) {
            Ok(g) => g,
            Err(e) => {
                errors.push(bad(e.to_string()));
                continue;
            }
        };
        let mut members = Vec::new();
        let mut comdat = OptId::NONE;
        for m in g.members() {
            let m = match m {
                Ok(m) => m,
                Err(e) => {
                    errors.push(bad(e.to_string()));
                    break;
                }
            };
            let Some(ms) = resolution.section_id(file, m) else {
                errors.push(bad(format!("member {} out of range", m.0)));
                break;
            };
            let mrow = resolution.sections[ms];
            if comdat.is_none() {
                comdat = mrow.comdat;
            }
            match roles[ms] {
                Role::Piece => members.push(ms),
                Role::Eh => errors.push(feature(".eh_frame in a section group at -r (RL-8)")),
                Role::Property => {
                    errors.push(feature(".note.gnu.property in a section group at -r"));
                }
                Role::None => {}
            }
        }
        // RV-R7: a COMDAT group is kept in the copy of its keeper; a group
        // that is not COMDAT is kept in every copy.
        let kept = if g.is_comdat() {
            match comdat.get() {
                Some(c) => resolution.comdat.get(c).and_then(|k| k.get()) == Some(file),
                // No member names the signature: a group whose members are
                // all kept or none, by the members' own states.
                None => members.iter().all(|&m| resolution.sections[m].is_kept()),
            }
        } else {
            true
        };
        if !kept {
            continue;
        }
        groups.push(KeptGroup { section: s, members, signature: g.signature() });
    }
    if errors.is_empty() { Ok(groups) } else { Err(errors) }
}

// ---------------------------------------------------------------------
// Step 3: the assignment (L-R1, L-R1a, L-R2).
// ---------------------------------------------------------------------

/// L-R1's key of a section that is not a group member.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key<'n> {
    name: &'n [u8],
    /// The output section of the `SHF_LINK_ORDER` section's `sh_link`.
    link: Option<OutSecId>,
    retain: bool,
}

/// What [`assign`] gives.
struct Assigned<'n> {
    assignment: Assignment<'n>,
    /// The output section of the live `.eh_frame` inputs (S1), if any.
    eh_out: Option<OutSecId>,
    /// The live `.eh_frame` inputs, in `SectionId` order.
    eh_inputs: Vec<SectionId>,
    /// The live input property notes, in `SectionId` order.
    property_inputs: Vec<SectionId>,
    /// Per output section, its pieces in `SectionId` order (for `.eh_frame`
    /// its inputs).
    pieces: Vec<Vec<SectionId>>,
    /// Per output section, log2 of its alignment (L-R1a, L-51).
    align: Vec<u8>,
    /// Per output section, whether it is a group member (L-R2).
    member: Vec<bool>,
}

/// L-R1a: what the live pieces of one output section accumulate.
#[derive(Debug, Clone)]
struct Acc {
    ty: u32,
    all_unwind: bool,
    flags: u64,
    /// Whether a piece has been seen, and the merge class all agree on.
    merge_seen: bool,
    merge: Option<MergeClass>,
    alloc: Option<bool>,
    tls: Option<bool>,
    large: Option<bool>,
    link_order: Option<bool>,
    exclude: Option<bool>,
    align: u8,
    error: Option<(String, String)>,
}

impl Acc {
    const BLANK: Self = Self {
        ty: 0,
        all_unwind: true,
        flags: 0,
        merge_seen: false,
        merge: None,
        alloc: None,
        tls: None,
        large: None,
        link_order: None,
        exclude: None,
        align: 0,
        error: None,
    };

    fn same(slot: &mut Option<bool>, now: bool, words: (&str, &str)) -> Option<(String, String)> {
        match *slot {
            Some(was) if was != now => {
                let w = |b: bool| if b { words.0 } else { words.1 };
                Some((w(was).to_owned(), w(now).to_owned()))
            }
            _ => {
                *slot = Some(now);
                None
            }
        }
    }

    fn add(&mut self, h: &Header<'_>, target: &Target) {
        if self.error.is_some() {
            return;
        }
        self.align = self.align.max(h.align);
        let checks = [
            (&mut self.alloc, h.flags & SHF_ALLOC != 0, ("allocated", "non-allocated")),
            (&mut self.tls, h.flags & SHF_TLS != 0, ("TLS", "non-TLS")),
            (
                &mut self.large,
                h.flags & SHF_X86_64_LARGE != 0 && target.machine == Machine::X86_64,
                ("SHF_X86_64_LARGE", "not SHF_X86_64_LARGE"),
            ),
            (
                &mut self.link_order,
                h.flags & SHF_LINK_ORDER != 0,
                ("SHF_LINK_ORDER", "not SHF_LINK_ORDER"),
            ),
            (&mut self.exclude, h.flags & SHF_EXCLUDE != 0, ("SHF_EXCLUDE", "not SHF_EXCLUDE")),
        ];
        for (slot, now, words) in checks {
            if let Some(e) = Self::same(slot, now, words) {
                self.error = Some(e);
                return;
            }
        }
        let mut kept = SHF_WRITE
            | SHF_ALLOC
            | SHF_EXECINSTR
            | SHF_TLS
            | SHF_GNU_RETAIN
            | SHF_LINK_ORDER
            | SHF_EXCLUDE;
        if target.machine == Machine::X86_64 {
            kept |= SHF_X86_64_LARGE;
        }
        self.flags |= h.flags & kept;
        let class = (h.flags & SHF_MERGE != 0)
            .then_some(MergeClass { strings: h.flags & SHF_STRINGS != 0, entsize: h.entsize });
        self.merge = if self.merge_seen && self.merge != class { None } else { class };
        self.merge_seen = true;
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

    /// The output section's type, flags and merge class, or the conflict.
    fn finish(&self, member: bool) -> Result<(u32, u64, Option<MergeClass>), (String, String)> {
        if let Some(e) = &self.error {
            return Err(e.clone());
        }
        let ty =
            if self.ty == SHT_X86_64_UNWIND && !self.all_unwind { SHT_PROGBITS } else { self.ty };
        let merge = self.merge;
        let mut flags = self.flags;
        if let Some(m) = merge {
            flags |= SHF_MERGE | if m.strings { SHF_STRINGS } else { 0 };
        }
        if member {
            flags |= SHF_GROUP;
        }
        Ok((ty, flags, merge))
    }
}

#[allow(clippy::too_many_lines, reason = "L-R1 and L-R1a, one role after another")]
fn assign<'n>(
    resolution: &Resolution,
    headers: &Headers<'n>,
    roles: &Column<SectionId, Role>,
    groups: &[KeptGroup],
    target: &Target,
) -> Result<Assigned<'n>, Vec<LayoutDiag>> {
    let n = resolution.sections.len();
    let mut assignment = Assignment::empty(n);
    let mut pieces: Vec<Vec<SectionId>> = Vec::new();
    let mut member_out: Vec<bool> = Vec::new();
    let mut errors = Vec::new();
    let new_out = |a: &mut Assignment<'n>,
                   pieces: &mut Vec<Vec<SectionId>>,
                   member_out: &mut Vec<bool>,
                   name: &'n [u8],
                   member: bool|
     -> Result<OutSecId, LayoutDiag> {
        let o = a
            .outsecs
            .push(OutSec {
                name,
                sh_type: 0,
                flags: 0,
                merge: None,
                keep: false,
                fill: None,
                constraint: None,
                script_position: None,
            })
            .map_err(|_| internal("too many output sections"))?;
        pieces.push(Vec::new());
        member_out.push(member);
        Ok(o)
    };
    // L-R2: each member of a kept group is an output section of its own.
    let mut is_member = vec![false; n as usize];
    for g in groups {
        for &m in &g.members {
            is_member[m.0 as usize] = true;
        }
    }
    let mut keys: HashMap<Key<'n>, OutSecId> = HashMap::new();
    let mut eh_out = None;
    let mut eh_inputs = Vec::new();
    let mut property_inputs = Vec::new();
    let mut link_order = Vec::new();
    for (s, &role) in roles.iter() {
        let h = &headers[s];
        match role {
            Role::None => {}
            Role::Property => {
                assignment.out_of[s] = OutOf::Consumed;
                property_inputs.push(s);
            }
            Role::Eh => {
                // S1: one `.eh_frame` output section; what would make two
                // is refused until a later package.
                if h.flags & (SHF_GNU_RETAIN | SHF_LINK_ORDER) != 0 {
                    errors.push(feature(".eh_frame with SHF_GNU_RETAIN or SHF_LINK_ORDER at -r"));
                    continue;
                }
                let o = match eh_out {
                    Some(o) => o,
                    None => match new_out(
                        &mut assignment,
                        &mut pieces,
                        &mut member_out,
                        b".eh_frame",
                        false,
                    ) {
                        Ok(o) => {
                            eh_out = Some(o);
                            o
                        }
                        Err(e) => return Err(vec![e]),
                    },
                };
                assignment.out_of[s] = OutOf::Out(o);
                pieces[o.0 as usize].push(s);
                eh_inputs.push(s);
            }
            Role::Piece if is_member[s.0 as usize] => {
                let o = new_out(&mut assignment, &mut pieces, &mut member_out, h.name, true)
                    .map_err(|e| vec![e])?;
                assignment.out_of[s] = OutOf::Out(o);
                pieces[o.0 as usize].push(s);
            }
            Role::Piece if h.flags & SHF_LINK_ORDER != 0 => link_order.push(s),
            Role::Piece => {
                let key = Key { name: h.name, link: None, retain: h.flags & SHF_GNU_RETAIN != 0 };
                let o = if let Some(&o) = keys.get(&key) {
                    o
                } else {
                    let o = new_out(&mut assignment, &mut pieces, &mut member_out, h.name, false)
                        .map_err(|e| vec![e])?;
                    keys.insert(key, o);
                    o
                };
                assignment.out_of[s] = OutOf::Out(o);
                pieces[o.0 as usize].push(s);
            }
        }
    }
    // L-R1: a `SHF_LINK_ORDER` section is keyed by the output section of
    // its linked-to section, which may itself be one (a chain): rounds
    // until nothing changes.
    let mut pending = link_order;
    while !pending.is_empty() {
        let mut next = Vec::new();
        let before = pending.len();
        for s in pending {
            let h = &headers[s];
            let target_out = h.link.and_then(|l| assignment.out(l));
            let Some(link) = target_out else {
                let waits = h.link.is_some_and(|l| {
                    headers.get(l).is_some_and(|lh| lh.flags & SHF_LINK_ORDER != 0)
                        && roles.get(l) == Some(&Role::Piece)
                        && assignment.out(l).is_none()
                });
                if waits {
                    next.push(s);
                } else {
                    errors.push(feature(format!(
                        "SHF_LINK_ORDER section {} whose linked-to section is not in the -r \
                         output",
                        text(h.name)
                    )));
                }
                continue;
            };
            let key = Key { name: h.name, link: Some(link), retain: h.flags & SHF_GNU_RETAIN != 0 };
            let o = if let Some(&o) = keys.get(&key) {
                o
            } else {
                let o = new_out(&mut assignment, &mut pieces, &mut member_out, h.name, false)
                    .map_err(|e| vec![e])?;
                keys.insert(key, o);
                o
            };
            assignment.out_of[s] = OutOf::Out(o);
            pieces[o.0 as usize].push(s);
        }
        if next.len() == before {
            errors.push(feature("a cycle of SHF_LINK_ORDER sections at -r"));
            break;
        }
        pending = next;
    }
    // L-R1a over the live pieces (every piece here is live).
    let mut align = Vec::with_capacity(pieces.len());
    for (i, list) in pieces.iter_mut().enumerate() {
        list.sort_unstable();
        let mut acc = Acc::BLANK;
        for &s in list.iter() {
            acc.add(&headers[s], target);
        }
        align.push(acc.align);
        let row = &mut assignment.outsecs[OutSecId(i as u32)];
        match acc.finish(member_out[i]) {
            Ok((ty, flags, merge)) => {
                row.sh_type = ty;
                row.flags = flags;
                row.merge = merge;
            }
            Err((first, second)) => errors.push(section_conflict(text(row.name), &first, &second)),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(Assigned {
        assignment,
        eh_out,
        eh_inputs,
        property_inputs,
        pieces,
        align,
        member: member_out,
    })
}

// ---------------------------------------------------------------------
// Step 5: the relocation records (I-R5, S6) and the removed records'
// sizes (S12).
// ---------------------------------------------------------------------

/// The relocation section of a section, as `(SectionId, input records)`.
fn reloc_section(
    resolution: &Resolution,
    headers: &Headers<'_>,
    s: SectionId,
    class: Class,
) -> Result<Option<(SectionId, u64)>, LayoutDiag> {
    let row = resolution.sections[s];
    if row.reloc.0 == 0 {
        return Ok(None);
    }
    let Some((file, _)) = resolution.sh_index(s) else { return Ok(None) };
    let Some(r) = resolution.section_id(file, row.reloc) else {
        return Err(internal(format!("section {}: relocation section out of range", s.0)));
    };
    let h = &headers[r];
    let (want, name) = match class {
        Class::Elf64 => (SHT_RELA, "SHT_REL"),
        Class::Elf32 => (SHT_REL, "SHT_RELA"),
    };
    if h.sh_type != want {
        return Err(refused(RefusalClass::SectionType, name));
    }
    let entsize = reloc_entry_size(class);
    if h.entsize != entsize || !h.size.is_multiple_of(entsize) {
        return Err(internal(format!(
            "relocation section {} of section {}: entsize {} size {}",
            text(h.name),
            s.0,
            h.entsize,
            h.size
        )));
    }
    Ok(Some((r, h.size / entsize)))
}

/// The records of each output section's `Relocs` chunk (S6, S1): every
/// record of every piece's relocation section, `NONE` included; for
/// `.eh_frame`, without the records in removed records.
#[allow(clippy::too_many_arguments, reason = "the tables the count reads")]
fn relocation_records(
    objects: &[Option<FileObject<'_>>],
    inputs: &Inputs<'_>,
    resolution: &Resolution,
    headers: &Headers<'_>,
    assigned: &Assigned<'_>,
    removed: &[(SectionId, u64)],
    class: Class,
) -> Result<Vec<u64>, Vec<LayoutDiag>> {
    let mut counts = vec![0u64; assigned.pieces.len()];
    let mut errors = Vec::new();
    for (o, list) in assigned.pieces.iter().enumerate() {
        for &s in list {
            match reloc_section(resolution, headers, s, class) {
                Ok(None) => {}
                Ok(Some((r, n))) => {
                    let gone = if Some(OutSecId(o as u32)) == assigned.eh_out {
                        match records_in_removed(objects, inputs, resolution, removed, s, r, class)
                        {
                            Ok(k) => k,
                            Err(e) => {
                                errors.push(e);
                                0
                            }
                        }
                    } else {
                        0
                    };
                    counts[o] += n - gone;
                }
                Err(e) => errors.push(e),
            }
        }
    }
    for &s in &assigned.property_inputs {
        if resolution.sections[s].reloc.0 != 0 {
            errors.push(feature(".note.gnu.property with relocations at -r"));
        }
    }
    if errors.is_empty() { Ok(counts) } else { Err(errors) }
}

/// The removed records of `eh` with their sizes, in offset order.
fn removed_of(removed: &[(SectionId, u64)], eh: SectionId) -> &[(SectionId, u64)] {
    let start = removed.partition_point(|r| r.0 < eh);
    let end = removed.partition_point(|r| r.0 <= eh);
    &removed[start..end]
}

/// The size of the record at `offset` of `eh` (S12: 4 + its length
/// word), read from the bytes.
fn record_size(
    objects: &[Option<FileObject<'_>>],
    inputs: &Inputs<'_>,
    resolution: &Resolution,
    eh: SectionId,
    offset: u64,
) -> Result<u64, LayoutDiag> {
    let (file, index) =
        resolution.sh_index(eh).ok_or_else(|| internal("an .eh_frame section with no file"))?;
    let Some(Some(fo)) = objects.get(file.0 as usize) else {
        return Err(internal("an .eh_frame section of a file that is not an object"));
    };
    let bytes = section_bytes(inputs, &fo.object, file, index, offset..offset + 4)
        .map_err(|e| internal(format!("reading an .eh_frame record: {e}")))?;
    let len = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    if len == u32::MAX {
        return Err(feature("64-bit .eh_frame record length"));
    }
    Ok(4 + u64::from(len))
}

/// The number of `r`'s records whose `r_offset` lies in a removed record
/// of `eh` (S1).
#[allow(clippy::too_many_arguments, reason = "the tables the count reads")]
fn records_in_removed(
    objects: &[Option<FileObject<'_>>],
    inputs: &Inputs<'_>,
    resolution: &Resolution,
    removed: &[(SectionId, u64)],
    eh: SectionId,
    r: SectionId,
    class: Class,
) -> Result<u64, LayoutDiag> {
    let gone = removed_of(removed, eh);
    if gone.is_empty() {
        return Ok(0);
    }
    let mut ranges = Vec::with_capacity(gone.len());
    for &(_, off) in gone {
        ranges.push((off, off + record_size(objects, inputs, resolution, eh, off)?));
    }
    let (file, index) =
        resolution.sh_index(r).ok_or_else(|| internal("a relocation section with no file"))?;
    let Some(Some(fo)) = objects.get(file.0 as usize) else {
        return Err(internal("a relocation section of a file that is not an object"));
    };
    let s = fo.object.section(index).map_err(|e| internal(e.to_string()))?;
    let bytes = section_bytes(inputs, &fo.object, file, index, 0..s.size())
        .map_err(|e| internal(format!("reading a relocation section: {e}")))?;
    let offsets: Vec<u64> = match class {
        Class::Elf64 => s
            .rela_in(&bytes)
            .map_err(|e| internal(e.to_string()))?
            .iter()
            .map(|x| x.r_offset)
            .collect(),
        Class::Elf32 => s
            .rel_in(&bytes)
            .map_err(|e| internal(e.to_string()))?
            .iter()
            .map(|x| x.r_offset)
            .collect(),
    };
    let inside = |at: u64| {
        let i = ranges.partition_point(|r| r.1 <= at);
        ranges.get(i).is_some_and(|r| r.0 <= at)
    };
    Ok(offsets.into_iter().filter(|&at| inside(at)).count() as u64)
}

/// The sizes of the removed records, in `removed`'s order.
fn removed_record_sizes(
    objects: &[Option<FileObject<'_>>],
    inputs: &Inputs<'_>,
    resolution: &Resolution,
    removed: &[(SectionId, u64)],
) -> Result<Vec<u64>, Vec<LayoutDiag>> {
    removed
        .iter()
        .map(|&(eh, off)| record_size(objects, inputs, resolution, eh, off))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| vec![e])
}

// ---------------------------------------------------------------------
// Steps 4, 6, 7, 8: chunks, order, symbols, offsets.
// ---------------------------------------------------------------------

struct Build<'a, 's> {
    inputs: &'a Inputs<'s>,
    objects: &'a [Option<FileObject<'s>>],
    resolution: &'a Resolution,
    headers: &'a Headers<'s>,
    groups: &'a [KeptGroup],
    assigned: Assigned<'s>,
    records: Vec<u64>,
    removed: Box<[(SectionId, u64)]>,
    removed_sizes: Vec<u64>,
    target: &'a Target,
    notes: NoteSizes,
    pool: &'a Pool,
}

const fn align_up(value: u64, align: u8) -> u64 {
    let a = 1u64 << align;
    (value + a - 1) & !(a - 1)
}

fn chunk(kind: ChunkKind, sh_type: u32, flags: u64, align: u8, size: u64) -> Chunk {
    Chunk {
        kind,
        sh_type,
        flags,
        align,
        size,
        capacity: size,
        addr: None,
        offset: 0,
        shndx: None,
        relro: false,
    }
}

const fn synth(name: &'static [u8]) -> ChunkKind {
    ChunkKind::Synthetic(Synth { name, address_dependent: false })
}

/// What one live file's symbol table gives `.symtab` (I-R4): its kept
/// locals and `STT_FILE` entries, their `.strtab` bytes, and whether one
/// needs `.symtab_shndx`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct LocalCount {
    entries: u64,
    strtab: u64,
    xindex: bool,
}

/// One unit of L-R8's order, sorted by its first piece.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Unit {
    Out(u32),
    Property,
    Group(u32),
}

#[allow(clippy::too_many_lines, reason = "MD-R1's chunks, L-R8's order and offsets in turn")]
fn build<'s>(b: &Build<'_, 's>) -> Result<RelocatableLayout<'s>, Vec<LayoutDiag>> {
    let class = b.target.class;
    let rel = class == Class::Elf32;
    let a = &b.assigned;
    let res = b.resolution;
    let headers = b.headers;
    let nout = a.pieces.len();

    // Step 4 (L-R4, L-R5): each piece at the next offset aligned to its
    // alignment, in (FileId, section index) order.
    let mut place: Column<SectionId, Place> = Column::filled(res.sections.len(), Place::NONE);
    let mut body_size = vec![0u64; nout];
    let mut offsets: Vec<Vec<u64>> = vec![Vec::new(); nout];
    for (o, list) in a.pieces.iter().enumerate() {
        if Some(OutSecId(o as u32)) == a.eh_out {
            continue;
        }
        let mut at = 0u64;
        for &s in list {
            let h = &headers[s];
            let off = align_up(at, h.align);
            offsets[o].push(off);
            at = off + h.size;
        }
        body_size[o] = at;
    }

    // The chunks, in their id order: the headers, the synthetic chunks in
    // S-3's order, Body(o) and Relocs(o) per output section, Group(g).
    let mut chunks: Vec<Chunk> = Vec::new();
    let push = |chunks: &mut Vec<Chunk>, c: Chunk| {
        chunks.push(c);
        ChunkId(chunks.len() as u32 - 1)
    };
    let fh = push(
        &mut chunks,
        chunk(ChunkKind::FileHeader, SHT_PROGBITS, 0, word_align(class), file_header_size(class)),
    );
    let sh =
        push(&mut chunks, chunk(ChunkKind::SectionHeaders, SHT_PROGBITS, 0, word_align(class), 0));
    let property = match (b.notes.property, a.property_inputs.first()) {
        (Some(size), Some(&first)) => Some((
            push(
                &mut chunks,
                chunk(synth(b".note.gnu.property"), SHT_NOTE, SHF_ALLOC, word_align(class), size),
            ),
            first,
        )),
        (Some(_), None) => {
            return Err(vec![internal("a property note with no input property note at -r")]);
        }
        (None, _) => None,
    };
    let build_id = b.notes.build_id.map(|size| {
        push(&mut chunks, chunk(synth(b".note.gnu.build-id"), SHT_NOTE, SHF_ALLOC, 2, size))
    });
    let eh = match a.eh_out {
        Some(o) => {
            let row = &a.assignment.outsecs[o];
            let input_sizes = a.pieces[o.0 as usize].iter().map(|&s| headers[s].size);
            let size = eh_frame_size(input_sizes, b.removed_sizes.iter().copied());
            Some(push(
                &mut chunks,
                chunk(synth(b".eh_frame"), row.sh_type, row.flags, a.align[o.0 as usize], size),
            ))
        }
        None => None,
    };
    let symtab = push(&mut chunks, chunk(synth(b".symtab"), SHT_SYMTAB, 0, word_align(class), 0));
    let strtab = push(&mut chunks, chunk(synth(b".strtab"), SHT_STRTAB, 0, 0, 0));
    let shstrtab = push(&mut chunks, chunk(synth(b".shstrtab"), SHT_STRTAB, 0, 0, 0));
    let mut body: Vec<Option<ChunkId>> = vec![None; nout];
    let mut relocs: Vec<Option<ChunkId>> = vec![None; nout];
    for o in 0..nout {
        let out = OutSecId(o as u32);
        let row = &a.assignment.outsecs[out];
        if Some(out) == a.eh_out {
            body[o] = eh;
        } else {
            let c = push(
                &mut chunks,
                chunk(
                    ChunkKind::Body(Part::first(out)),
                    row.sh_type,
                    row.flags,
                    a.align[o],
                    body_size[o],
                ),
            );
            body[o] = Some(c);
            for (k, &s) in a.pieces[o].iter().enumerate() {
                place[s] = Place::at(c, offsets[o][k]);
            }
        }
    }
    for (o, slot) in relocs.iter_mut().enumerate() {
        if let Some(size) = relocs_size(class, b.records[o]) {
            let mut flags = SHF_INFO_LINK;
            if a.member[o] {
                flags |= SHF_GROUP;
            }
            *slot = Some(push(
                &mut chunks,
                chunk(
                    ChunkKind::Relocs(OutSecId(o as u32)),
                    if rel { SHT_REL } else { SHT_RELA },
                    flags,
                    word_align(class),
                    size,
                ),
            ));
        }
    }
    let mut group_chunk = Vec::with_capacity(b.groups.len());
    for g in b.groups {
        let outs: Vec<OutSecId> = g.members.iter().filter_map(|&m| a.assignment.out(m)).collect();
        let member_relocs = outs.iter().filter(|o| relocs[o.0 as usize].is_some()).count() as u64;
        group_chunk.push(push(
            &mut chunks,
            chunk(
                ChunkKind::Group(g.section),
                mortar_elf::consts::SHT_GROUP,
                0,
                2,
                group_size(outs.len() as u64, member_relocs),
            ),
        ));
    }

    // L-R8's order: build-id; the output sections (and the property note)
    // by their first piece's (FileId, section index), each group right
    // before its first member, each relocation section right after its
    // target; then .symtab, .symtab_shndx, .strtab, .shstrtab.
    let mut units: Vec<(SectionId, Unit)> = Vec::with_capacity(nout + 1);
    for (o, list) in a.pieces.iter().enumerate() {
        units.push((list[0], Unit::Out(o as u32)));
    }
    if let Some((_, first)) = property {
        units.push((first, Unit::Property));
    }
    // A group's first member: the member output section that comes first.
    let mut groups_before: HashMap<u32, Vec<usize>> = HashMap::new();
    for (k, g) in b.groups.iter().enumerate() {
        let first = g
            .members
            .iter()
            .filter_map(|&m| a.assignment.out(m))
            .min_by_key(|o| a.pieces[o.0 as usize][0]);
        match first {
            Some(o) => groups_before.entry(o.0).or_default().push(k),
            // A kept group with no placed member stands at its own
            // position.
            None => units.push((g.section, Unit::Group(k as u32))),
        }
    }
    units.sort_unstable();
    let mut order: Vec<ChunkId> = Vec::with_capacity(chunks.len() + 1);
    order.push(fh);
    if let Some(c) = build_id {
        order.push(c);
    }
    for &(_, unit) in &units {
        match unit {
            Unit::Out(o) => {
                if let Some(gs) = groups_before.get(&o) {
                    order.extend(gs.iter().map(|&k| group_chunk[k]));
                }
                if let Some(c) = body[o as usize] {
                    order.push(c);
                }
                if let Some(c) = relocs[o as usize] {
                    order.push(c);
                }
            }
            Unit::Property => order.extend(property.map(|p| p.0)),
            Unit::Group(k) => order.push(group_chunk[k as usize]),
        }
    }
    order.push(symtab);
    let tail_at = order.len();
    order.push(strtab);
    order.push(shstrtab);

    // Section indexes, dense in order (L-86).
    let mut index_of = vec![0u32; chunks.len()];
    let mut next = 0u32;
    for &c in &order[1..] {
        next += 1;
        index_of[c.0 as usize] = next;
    }

    // Step 7: the `.symtab` entries (I-R4, S4) and `.strtab` (S2).
    let section_symbols: Vec<ChunkId> = order[1..tail_at]
        .iter()
        .copied()
        .filter(|&c| {
            !matches!(chunks[c.0 as usize].kind, ChunkKind::Group(_) | ChunkKind::Relocs(_))
                && c != symtab
        })
        .collect();
    let mut xindex = section_symbols.iter().any(|&c| needs_xindex(index_of[c.0 as usize]));
    // The output index of each input section's content.
    let mut out_index: Column<SectionId, u32> = Column::filled(res.sections.len(), 0);
    for (s, of) in a.assignment.out_of.iter() {
        if let Some(o) = of.out()
            && let Some(c) = body[o.0 as usize]
        {
            out_index[s] = index_of[c.0 as usize];
        }
    }
    let mut group_sig: HashMap<SectionId, (u32, u32)> = HashMap::new();
    for (k, g) in b.groups.iter().enumerate() {
        group_sig.insert(g.section, (g.signature.0, index_of[group_chunk[k].0 as usize]));
    }
    let locals = local_counts(b, &out_index, &group_sig)?;
    let mut nlocals = 0u64;
    let mut strtab_bytes = 0u64;
    for l in &locals {
        nlocals += l.entries;
        strtab_bytes += l.strtab;
        xindex |= l.xindex;
    }
    let globals = global_counts(b, &out_index)?;
    strtab_bytes += globals.strtab;
    xindex |= globals.xindex;
    let entries = symtab_entries(section_symbols.len() as u64, nlocals, globals.entries);
    chunks[symtab.0 as usize].size = symtab_size(class, entries);
    chunks[strtab.0 as usize].size = STRTAB_FIXED.len() as u64 + strtab_bytes;
    if let Some(size) = symtab_shndx_size(xindex, entries) {
        let c = push(&mut chunks, chunk(synth(b".symtab_shndx"), SHT_SYMTAB_SHNDX, 0, 2, size));
        order.insert(tail_at, c);
        index_of.push(0);
        for (k, &c) in order[1..].iter().enumerate() {
            index_of[c.0 as usize] = k as u32 + 1;
        }
    }

    // `.shstrtab` (S3), in index order.
    let shstr = {
        let names = order[1..].iter().map(|&c| {
            let ch = &chunks[c.0 as usize];
            match ch.kind {
                ChunkKind::Synthetic(s) => SectionName::Plain(s.name),
                ChunkKind::Group(_) => SectionName::Plain(b".group"),
                ChunkKind::Relocs(o) => {
                    SectionName::Relocs { rel, target: a.assignment.outsecs[o].name }
                }
                ChunkKind::Body(p) => SectionName::Plain(a.assignment.outsecs[p.out].name),
                _ => SectionName::Plain(b""),
            }
        });
        shstrtab_size(names)
    };
    chunks[shstrtab.0 as usize].size = shstr;

    // Step 8: offsets (L-R8), then the section header table.
    let mut at = file_header_size(class);
    for &c in &order[1..] {
        let ch = &mut chunks[c.0 as usize];
        ch.shndx = Some(index_of[c.0 as usize]);
        ch.offset = align_up(at, ch.align);
        if ch.in_file() {
            at = ch.offset + ch.size;
        }
        ch.capacity = ch.size;
    }
    let count = order.len() as u64 - 1;
    {
        let ch = &mut chunks[sh.0 as usize];
        ch.offset = align_up(at, word_align(class));
        ch.size = (count + 1) * section_header_size(class);
        ch.capacity = ch.size;
    }
    order.push(sh);

    let placement = Placement {
        kind: PlacementKind::Relocatable,
        chunks: Column::from_vec(chunks).map_err(|_| vec![internal("too many chunks")])?,
        order: order.into_boxed_slice(),
        place,
        ..Placement::default()
    };
    Ok(RelocatableLayout {
        assignment: a.assignment.clone(),
        layout: Layout { placement, ..Layout::default() },
        eh_removed: b.removed.clone(),
    })
}

/// I-R4's kept locals, per live file (in parallel over files).
#[allow(
    clippy::many_single_char_names,
    reason = "b, c, f, t, x: the build, the count, the file, the table, the section"
)]
fn local_counts(
    b: &Build<'_, '_>,
    out_index: &Column<SectionId, u32>,
    group_sig: &HashMap<SectionId, (u32, u32)>,
) -> Result<Vec<LocalCount>, Vec<LayoutDiag>> {
    let res = b.resolution;
    let eh_index = b
        .assigned
        .eh_out
        .and_then(|o| b.assigned.pieces.get(o.0 as usize))
        .and_then(|l| l.first())
        .map_or(0, |&s| out_index[s]);
    // The removed records with their sizes, for the locals inside them.
    let removed: Vec<(SectionId, u64, u64)> =
        b.removed.iter().zip(&b.removed_sizes).map(|(&(s, o), &n)| (s, o, o + n)).collect();
    let files = b.objects.len();
    let per: Vec<Result<LocalCount, String>> = b.pool.unit_cost(2000).map(files, |f| {
        let Some(Some(fo)) = b.objects.get(f) else { return Ok(LocalCount::default()) };
        let Some(t) = fo.symbols else { return Ok(LocalCount::default()) };
        let file = FileId(f as u32);
        let mut c = LocalCount::default();
        for (i, sym) in t.locals() {
            if i.0 == 0 || sym.sym_type() == STT_SECTION {
                continue;
            }
            let index = if sym.sym_type() == STT_FILE {
                Some(0)
            } else {
                match t.section_of(i, &sym).map_err(|e| e.to_string())? {
                    SymbolSection::Absolute => Some(0),
                    SymbolSection::Section(x) => {
                        let s = res
                            .section_id(file, x)
                            .ok_or_else(|| "a local's section out of range".to_owned())?;
                        local_index(
                            b,
                            s,
                            sym.st_value,
                            i.0,
                            out_index,
                            eh_index,
                            &removed,
                            group_sig,
                        )
                    }
                    _ => None,
                }
            };
            let Some(index) = index else { continue };
            let name = t.name(&sym).map_err(|e| e.to_string())?;
            c.entries += 1;
            c.strtab += EntryName { name, version: None }.strtab_bytes();
            c.xindex |= needs_xindex(index);
        }
        Ok(c)
    });
    per.into_iter()
        .enumerate()
        .map(|(f, r)| r.map_err(|e| internal(format!("file {f}: symbol table: {e}"))))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| vec![e])
}

/// The output index of a kept local defined in input section `s`, or
/// `None` when I-R4 drops it (0 for an entry with no section).
#[allow(clippy::too_many_arguments, reason = "I-R4's clauses each read a table")]
fn local_index(
    b: &Build<'_, '_>,
    s: SectionId,
    value: u64,
    sym: u32,
    out_index: &Column<SectionId, u32>,
    eh_index: u32,
    removed: &[(SectionId, u64, u64)],
    group_sig: &HashMap<SectionId, (u32, u32)>,
) -> Option<u32> {
    let h = &b.headers[s];
    // The type-unit signature of a kept group (L-R3).
    if let Some(&(signature, index)) = group_sig.get(&s) {
        return (signature == sym).then_some(index);
    }
    if b.assigned.eh_out.is_some() && b.assigned.eh_inputs.binary_search(&s).is_ok() {
        let start = removed.partition_point(|r| (r.0, r.2) <= (s, value));
        let inside = removed.get(start).is_some_and(|r| r.0 == s && r.1 <= value && value < r.2);
        return (!inside).then_some(eh_index);
    }
    let index = out_index[s];
    // The merged stack note is made by the `-r` itself (I-R4).
    if index == 0 || h.name == b".note.GNU-stack" {
        return None;
    }
    Some(index)
}

/// What the written globals give (S4 item 4, S14).
#[derive(Debug, Clone, Copy, Default)]
struct GlobalCount {
    entries: u64,
    strtab: u64,
    xindex: bool,
}

#[allow(
    clippy::many_single_char_names,
    reason = "b, c, e, k, t: the build, the count, the definition, the key, the table"
)]
fn global_counts(
    b: &Build<'_, '_>,
    out_index: &Column<SectionId, u32>,
) -> Result<GlobalCount, Vec<LayoutDiag>> {
    let res = b.resolution;
    let mut mentioned = vec![false; res.def.len() as usize];
    for (f, fo) in b.objects.iter().enumerate() {
        if fo.is_none() {
            continue;
        }
        for &k in b.inputs.sym_map_of(FileId(f as u32)) {
            if let Some(m) = mentioned.get_mut(k.0 as usize) {
                *m = true;
            }
        }
    }
    let mut c = GlobalCount::default();
    let mut errors = Vec::new();
    for (k, &is_mentioned) in mentioned.iter().enumerate() {
        let id = SymbolId(k as u32);
        let def = res.def[id];
        let written = match def {
            Def::Internal(_) => false,
            Def::Undefined { .. } => {
                is_mentioned || res.props[id].refs.contains(RefFlags::FROM_OBJECT)
            }
            _ => is_mentioned,
        };
        if !written {
            continue;
        }
        let key = &b.inputs.symbols[id];
        c.entries += 1;
        match def {
            Def::Defined(e) => {
                let Some(Some(fo)) = b.objects.get(e.file.0 as usize) else {
                    errors.push(internal(format!(
                        "key {k}: defined in a file that is not an object"
                    )));
                    continue;
                };
                let Some(t) = fo.symbols else {
                    errors.push(internal(format!("key {k}: defined in a file with no symbols")));
                    continue;
                };
                let found = t.get(e.index).and_then(|sym| {
                    let name = t.name(&sym)?;
                    let section = t.section_of(e.index, &sym)?;
                    Ok((name, section))
                });
                match found {
                    Ok((name, section)) => {
                        c.strtab += EntryName { name, version: None }.strtab_bytes();
                        if let SymbolSection::Section(x) = section {
                            let index = res.section_id(e.file, x).map_or(0, |s| out_index[s]);
                            if index == 0 {
                                errors.push(internal(format!(
                                    "global {} is defined in a section with no output at -r",
                                    text(key.name)
                                )));
                            }
                            c.xindex |= needs_xindex(index);
                        }
                    }
                    Err(e) => errors.push(internal(format!("key {k}: {e}"))),
                }
            }
            Def::Undefined { .. } | Def::Common(_) => {
                c.strtab += EntryName { name: key.name, version: key.version }.strtab_bytes();
            }
            Def::Shared(_) | Def::Internal(_) => {
                errors.push(internal(format!("key {}: a shared definition at -r", text(key.name))));
            }
        }
    }
    if errors.is_empty() { Ok(c) } else { Err(errors) }
}

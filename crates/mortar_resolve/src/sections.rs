//! The fate of sections (`spec/resolve.md` §10): `kind` from the file
//! alone (I-file), `state` from the kind and the COMDAT keepers (§6).

use mortar_core::resolution::{DropReason, SectionKind, SectionState};
use mortar_elf::Machine;
use mortar_elf::consts::{
    SHF_ALLOC, SHF_COMPRESSED, SHF_EXCLUDE, SHF_EXECINSTR, SHF_GNU_RETAIN, SHF_GROUP,
    SHF_INFO_LINK, SHF_LINK_ORDER, SHF_MERGE, SHF_STRINGS, SHF_TLS, SHF_WRITE, SHF_X86_64_LARGE,
    SHT_AARCH64_ATTRIBUTES, SHT_DYNAMIC, SHT_DYNSYM, SHT_FINI_ARRAY, SHT_GNU_HASH, SHT_GNU_VERDEF,
    SHT_GNU_VERNEED, SHT_GNU_VERSYM, SHT_GROUP, SHT_HASH, SHT_INIT_ARRAY, SHT_LLVM_ADDRSIG,
    SHT_LLVM_CALL_GRAPH_PROFILE, SHT_LLVM_DEPENDENT_LIBRARIES, SHT_NOBITS, SHT_NOTE,
    SHT_PREINIT_ARRAY, SHT_PROGBITS, SHT_REL, SHT_RELA, SHT_RELR, SHT_STRTAB, SHT_SYMTAB,
    SHT_SYMTAB_SHNDX, SHT_X86_64_UNWIND,
};

use crate::view::SectionInfo;

/// `SHF_AARCH64_PURECODE` (AArch64 ELF, "Section flags").
pub const SHF_AARCH64_PURECODE: u64 = 0x2000_0000;

/// Why a section is refused (§10's refusal rows).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SectionRefusal {
    /// An `SHT_LLVM_*` type that is not ignorable (`SHT_LLVM_ODRTAB`,
    /// `SHT_LLVM_LINKER_OPTIONS`, ...).
    LlvmType(u32),
    /// A dynamic-linking section type in a relocatable object.
    DynamicType(u32),
    /// Any other section type.
    Type(u32),
    /// Flag bits mortar does not implement (the unknown bits).
    Flags(u64),
    /// `.gnu.linkonce.*` (§6.6).
    Linkonce,
    /// `.zdebug*` (GNU-style compressed debug sections).
    Zdebug,
    /// `SHF_COMPRESSED` (GCC's `-gz`): refused until the store
    /// decompresses (layout.md L-52; WP-027c ruling).
    Compressed,
    /// A second relocation section for one kept section: GNU ld applies
    /// one of two `SHT_RELA` sections, lld refuses, mold asserts or drops
    /// one (§10). The variant holds the section it applies to.
    SecondReloc(u32),
    /// A relocation section of the form the machine does not use: an
    /// `SHT_RELA` section in an `EM_386` object (x86.md X86-2, §15.2 W12)
    /// or an `SHT_REL` one in an `EM_X86_64` (x64.md X-2) or
    /// `EM_AARCH64` object. The variant holds its `sh_type`.
    RelForm(u32),
}

/// What §10 makes of a section, besides its row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Note {
    None,
    /// `.gnu.warning`: a warning when the file is live.
    GnuWarning,
    /// `.gnu.warning.<sym>`: a warning per reference to `<sym>`; the slice
    /// is `<sym>`.
    GnuWarningSym,
    /// `SHF_GNU_RETAIN`: a root of `--gc-sections` (§4.7).
    Retain,
}

/// The flags §10 implements for a kept section on `machine`.
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
    match machine {
        Machine::AArch64 => base | SHF_AARCH64_PURECODE,
        Machine::X86_64 => base | SHF_X86_64_LARGE,
        Machine::I386 => base,
    }
}

/// Whether `name` is `.debug_*`, the `Debug` kind's names (§10).
fn is_debug_name(name: &[u8]) -> bool {
    name.starts_with(b".debug")
}

/// Whether `-S`/`-s` drop the section (§10): a non-allocated `.debug*`
/// section, of kind `Debug` or, for the string sections (`.debug_str`,
/// `.debug_line_str`: `SHF_MERGE|SHF_STRINGS`), `Merge`.
pub fn is_debug(s: &SectionInfo<'_>) -> bool {
    s.flags & SHF_ALLOC == 0 && is_debug_name(s.name)
}

/// The largest `sh_entsize` a `Merge` section is split by (synth.md S-6).
pub const MAX_MERGE_ENTSIZE: u64 = 1 << 16;

/// synth.md S-6: whether a section classified `Merge` is placed whole, as
/// `Regular`: it is writable or code, the target of a relocation section
/// (`relocated`), TLS, `SHF_COMPRESSED` (its bytes are not the pieces'
/// until decompressed, S-5), or of a shape merging does not take
/// (`sh_addralign` not a power of two, `sh_entsize` above 2^16), or
/// empty (`sh_size` 0: no piece could give a symbol defined in it a
/// value, so it is an ordinary empty section with a place, resolve.md
/// §10.1 P-1, WP-136). `mortar_merge::cut::placed_whole` is the same
/// predicate but for the size, kept for its bare-object tests; after this,
/// phase 60 sees no such section.
#[must_use]
pub fn placed_whole(s: &SectionInfo<'_>, relocated: bool) -> bool {
    s.flags & (SHF_WRITE | SHF_EXECINSTR | SHF_TLS | SHF_COMPRESSED) != 0
        || relocated
        || !(s.addralign == 0 || s.addralign.is_power_of_two())
        || s.entsize == 0
        || s.entsize > MAX_MERGE_ENTSIZE
        || s.size == 0
}

/// The kind of a section, by the first row of §10's table that matches,
/// leaving out the rows that need the COMDAT keeper (a member of a group
/// not kept takes its kind from the later rows; [`state`] decides its
/// state). `index` 0 is `Null`. The file-level row (`__gnu_lto_slim`) is
/// the caller's.
pub fn classify(
    machine: Machine,
    index: u32,
    s: &SectionInfo<'_>,
) -> Result<(SectionKind, Note), SectionRefusal> {
    use SectionKind as K;
    if index == 0 {
        return Ok((K::Null, Note::None));
    }
    let alloc = s.flags & SHF_ALLOC != 0;
    match s.sh_type {
        SHT_SYMTAB | SHT_SYMTAB_SHNDX if !alloc => return Ok((K::Symtab, Note::None)),
        SHT_STRTAB if !alloc => return Ok((K::Strtab, Note::None)),
        SHT_REL | SHT_RELA => {
            // X86-2, X-2: i386 relocates by REL only, the 64-bit targets by
            // RELA only; the other form is refused by name, never read
            // with the wrong addend.
            let rel = matches!(machine, Machine::I386);
            return if (s.sh_type == SHT_REL) == rel {
                Ok((K::Reloc, Note::None))
            } else {
                Err(SectionRefusal::RelForm(s.sh_type))
            };
        }
        SHT_GROUP => return Ok((K::Group, Note::None)),
        _ => {}
    }
    // spec: meta.md MT-6, amendment A-1: a Mason container, before the
    // `SHF_EXCLUDE` row it would otherwise match. A section of that name
    // that is not a container takes the rows below (the caller warns).
    if mortar_meta::is_container(s.name, s.sh_type, s.flags) {
        return Ok((K::Meta, Note::None));
    }
    if s.flags & SHF_EXCLUDE != 0 {
        return Ok((K::Excluded, Note::None));
    }
    let name = s.name;
    if name == b".note.GNU-stack" {
        return Ok((K::Stack, Note::None));
    }
    // WP-135: an input's build id (an `ld -r --build-id` object's) is
    // discarded in every link; the output's id is the linker's own
    // (image.md I-69), never an input's.
    if name == b".note.gnu.build-id" {
        return Ok((K::Excluded, Note::None));
    }
    if name == b".note.gnu.property" && s.sh_type == SHT_NOTE {
        return Ok((K::Note, retain(s)));
    }
    match s.sh_type {
        // `.deplibs` (WP-034): the library specifiers are not honoured
        // (GNU ld, mold); a library not loaded can only surface as a
        // named undefined symbol, never as a different output.
        SHT_LLVM_ADDRSIG | SHT_LLVM_CALL_GRAPH_PROFILE | SHT_LLVM_DEPENDENT_LIBRARIES => {
            return Ok((K::Excluded, Note::None));
        }
        // layout.md L-35 (§20.7): AArch64 build attributes are consumed;
        // the feature marking is the property note's (L-34).
        SHT_AARCH64_ATTRIBUTES if machine == Machine::AArch64 => {
            return Ok((K::Excluded, Note::None));
        }
        t if (0x6fff_4c00..=0x6fff_4cff).contains(&t) => {
            return Err(SectionRefusal::LlvmType(t));
        }
        _ => {}
    }
    if name.starts_with(b".gnu.lto_") {
        return Ok((K::Excluded, Note::None));
    }
    if name == b".gnu.warning" {
        return Ok((K::Excluded, Note::GnuWarning));
    }
    if name.starts_with(b".gnu.warning.") {
        return Ok((K::Excluded, Note::GnuWarningSym));
    }
    if name.starts_with(b".gnu.linkonce.") {
        return Err(SectionRefusal::Linkonce);
    }
    if name.starts_with(b".zdebug") {
        return Err(SectionRefusal::Zdebug);
    }
    if s.flags & SHF_COMPRESSED != 0 {
        return Err(SectionRefusal::Compressed);
    }
    let unknown = s.flags & !known_flags(machine);
    let kind =
        if (machine == Machine::X86_64 && s.sh_type == SHT_X86_64_UNWIND) || name == b".eh_frame" {
            K::EhFrame
        } else if s.flags & SHF_MERGE != 0 && matches!(s.sh_type, SHT_PROGBITS) {
            if s.entsize > 0 { K::Merge } else { K::Regular }
        } else if !alloc && is_debug_name(name) && s.sh_type == SHT_PROGBITS {
            K::Debug
        } else {
            match s.sh_type {
                SHT_NOTE => K::Note,
                SHT_NOBITS => K::Nobits,
                SHT_PROGBITS | SHT_INIT_ARRAY | SHT_FINI_ARRAY | SHT_PREINIT_ARRAY => K::Regular,
                SHT_STRTAB if alloc => K::Regular,
                SHT_DYNAMIC | SHT_DYNSYM | SHT_HASH | SHT_GNU_HASH | SHT_GNU_VERDEF
                | SHT_GNU_VERNEED | SHT_GNU_VERSYM | SHT_RELR => {
                    return Err(SectionRefusal::DynamicType(s.sh_type));
                }
                other => return Err(SectionRefusal::Type(other)),
            }
        };
    if unknown != 0 {
        return Err(SectionRefusal::Flags(unknown));
    }
    Ok((kind, retain(s)))
}

fn retain(s: &SectionInfo<'_>) -> Note {
    if s.flags & SHF_GNU_RETAIN != 0 { Note::Retain } else { Note::None }
}

/// The state of a section of kind `kind` (§10): `Dropped(Comdat)` for a
/// member of a group not kept that carries output, then by kind;
/// `excluded` is the reason for an `Excluded` kind ([`excluded_reason`]);
/// `strip_debug` (`-S`/`-s`, and the section is [`is_debug`]) drops the
/// section.
pub fn state(
    kind: SectionKind,
    excluded: DropReason,
    comdat_dropped: bool,
    strip_debug: bool,
) -> SectionState {
    use SectionKind as K;
    if comdat_dropped && kind.carries_output() {
        return SectionState::Dropped(DropReason::Comdat);
    }
    match kind {
        K::Null | K::Symtab | K::Strtab | K::Reloc | K::Group | K::Stack | K::Meta => {
            SectionState::Dropped(DropReason::Consumed)
        }
        K::Excluded => SectionState::Dropped(excluded),
        K::Debug | K::Merge if strip_debug => SectionState::Dropped(DropReason::Stripped),
        _ => SectionState::Kept,
    }
}

/// The reason an `Excluded` kind is dropped: `Consumed` for the kinds
/// §10 says the linker consumes (`.llvm_addrsig`, the call-graph profile,
/// `.deplibs`, `.gnu.warning*`, AArch64's `.ARM.attributes`), `Excluded` for
/// `SHF_EXCLUDE`, `.gnu.lto_*` and an input's `.note.gnu.build-id`.
pub fn excluded_reason(s: &SectionInfo<'_>, note: Note) -> DropReason {
    let consumed = matches!(note, Note::GnuWarning | Note::GnuWarningSym)
        || (s.flags & SHF_EXCLUDE == 0
            && matches!(
                s.sh_type,
                SHT_LLVM_ADDRSIG
                    | SHT_LLVM_CALL_GRAPH_PROFILE
                    | SHT_LLVM_DEPENDENT_LIBRARIES
                    | SHT_AARCH64_ATTRIBUTES
            ));
    if consumed { DropReason::Consumed } else { DropReason::Excluded }
}

//! References to targets in sections that are not live (§3.4 "Scan sees
//! only live sections", §3.11): how the referring section classifies
//! them (`spec/resolve.md` §6.3's table) and the tombstone written in
//! place of `X` (`spec/image.md` I-45 to I-48).

use mortar_elf::Class;

/// What a reference to a dead target does, by its referring section
/// (`spec/resolve.md` §6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeadRef {
    /// A non-alloc section with a debug name: accepted, the field holds
    /// [`tombstone`].
    Tombstone,
    /// Exactly `.gcc_except_table`, alloc or not: accepted, the field is 0.
    Zero,
    /// Exactly `.eh_frame`: accepted, the FDE is dead (`spec/model.md`
    /// §3.6) and no field of it is written; no event.
    DeadFde,
    /// Any other section: scan reports the event `DiscardedSection`.
    Discarded,
}

/// GNU ld's debug names (`spec/resolve.md` §6.3; `spec/image.md` I-45):
/// the prefixes `.debug`, `.zdebug`, `.line`, `.stab`,
/// `.gnu.linkonce.wi.`, `.gnu.debuglto_.debug_`, and exactly
/// `.gnu_debuglink`.
#[must_use]
pub fn is_debug_name(name: &[u8]) -> bool {
    const PREFIXES: [&[u8]; 6] =
        [b".debug", b".zdebug", b".line", b".stab", b".gnu.linkonce.wi.", b".gnu.debuglto_.debug_"];
    name == b".gnu_debuglink" || PREFIXES.iter().any(|p| name.starts_with(p))
}

/// The class of a reference to a dead target from the section `name`
/// (`alloc`: it has `SHF_ALLOC`), by `spec/resolve.md` §6.3's table. An
/// alloc section with a debug name is `Discarded`.
#[must_use]
pub fn dead_reference(name: &[u8], alloc: bool) -> DeadRef {
    if name == b".eh_frame" {
        DeadRef::DeadFde
    } else if name == b".gcc_except_table" {
        DeadRef::Zero
    } else if !alloc && is_debug_name(name) {
        DeadRef::Tombstone
    } else {
        DeadRef::Discarded
    }
}

/// The tombstone of §3.11 (`spec/image.md` I-47), before truncation to
/// the field: 1 for `.debug_ranges` and `.debug_loc`; 0 for
/// `.debug_rnglists`, `.debug_loclists` and `.debug_addr` (their entries
/// pair an address with a length, which all ones would wrap); all ones
/// for `.debug_aranges` and `.debug_frame`; for every other name all ones
/// in ELFCLASS64 and 0 in ELFCLASS32. Truncated to the field, all ones is
/// all ones in the field's width. Only a [`DeadRef::Tombstone`] section
/// asks; the length of an `.debug_aranges` tuple is the image's
/// (`spec/image.md` I-112), not part of this value.
#[must_use]
pub fn tombstone(section_name: &[u8], class: Class) -> u64 {
    match section_name {
        b".debug_ranges" | b".debug_loc" => 1,
        b".debug_rnglists" | b".debug_loclists" | b".debug_addr" => 0,
        b".debug_aranges" | b".debug_frame" => u64::MAX,
        _ => match class {
            Class::Elf64 => u64::MAX,
            Class::Elf32 => 0,
        },
    }
}

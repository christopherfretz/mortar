//! The GNU name index left out of an incremental-mode output (`spec/incr.md`
//! IN-55 option (A), decided by the operator 2026-11-17; WP-229, G-2).
//!
//! Every `-gsplit-dwarf` object (and every `-ggnu-pubnames` one) carries
//! `.debug_gnu_pubnames` and `.debug_gnu_pubtypes`. They are enumerated by
//! name, so a dead contribution's sets would still find a dead unit, and
//! rewriting them whole on each update is ruled out by size (IN-55). An
//! incremental-mode link (the full link and the update alike) therefore
//! writes neither: gdb and lldb do not read them (R14, WP-229's
//! transcripts), and the one linker feature that does, `--gdb-index`, is
//! refused in incremental mode (IN-58).
//!
//! [`discard`] runs once per incremental-mode full link, after resolution
//! and the headers, before assignment (the front's one call, keyed off the
//! incremental SESSION, never the plan's `--incremental`: a link that
//! `--incremental` with an unsupported option makes a plain one keeps the
//! sets). A plain link never calls it and pays nothing. The update applies
//! the same test, [`left_out`], to a changed file's new sections (the join,
//! G-3), so that the state it writes equals the capture of a full link
//! (IN-78).
//!
//! The state given is `Dropped(Stripped)`, the one `-S` gives a debug
//! section: the map lists the section (`discard <file> <section>
//! unspecified`, `spec/image.md` I-78), and every reader of that state
//! treats it as a section not in the output (the list is in
//! `spec/landed/incr/WP-229.md`).

use mortar_core::resolution::{DropReason, Resolution, SectionKind, SectionState};
use mortar_elf::consts::SHF_ALLOC;

use crate::headers::Headers;

/// IN-55 option (A): the input sections an incremental-mode output leaves
/// out, by name.
pub const LEFT_OUT: [&[u8]; 2] = [b".debug_gnu_pubnames", b".debug_gnu_pubtypes"];

/// Whether an input section named `name` is left out of an
/// incremental-mode output (when it is a kept, non-allocated debug
/// section: [`discard`]'s other conditions).
#[must_use]
pub fn left_out(name: &[u8]) -> bool {
    LEFT_OUT.contains(&name)
}

/// IN-55 option (A): every kept, non-allocated debug section (kind `Debug`,
/// or `Merge` as `-S` reads it) named in [`LEFT_OUT`] becomes
/// `Dropped(Stripped)`. A section in a COMDAT copy that is not kept stays
/// `Dropped(Comdat)`; one `-S` stripped stays as it is; an allocated
/// section of that name is not a debug section and is kept. A member of a
/// kept group is dropped alone, as `-S` drops one (the group's other
/// members and its keeper are untouched). O(sections); returns how many
/// sections it dropped.
pub fn discard(headers: &Headers<'_>, resolution: &mut Resolution) -> usize {
    let mut dropped = 0;
    let rows = resolution.sections.as_mut_slice().iter_mut();
    for (row, h) in rows.zip(headers.as_slice()) {
        let debug = matches!(row.kind, SectionKind::Debug | SectionKind::Merge);
        if row.state == SectionState::Kept && debug && h.flags & SHF_ALLOC == 0 && left_out(h.name)
        {
            row.state = SectionState::Dropped(DropReason::Stripped);
            dropped += 1;
        }
    }
    dropped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headers::Header;
    use mortar_core::column::Column;
    use mortar_core::id::OptId;
    use mortar_core::id::SectionId;
    use mortar_core::resolution::SectionRow;
    use mortar_elf::SectionIndex;

    fn header(name: &'static [u8], flags: u64) -> Header<'static> {
        Header { name, sh_type: 1, flags, size: 8, align: 0, link: None, entsize: 0 }
    }

    fn rows(kinds: &[(SectionKind, SectionState)]) -> Column<SectionId, SectionRow> {
        let mut c = Column::new();
        for &(kind, state) in kinds {
            c.push(SectionRow { kind, state, reloc: SectionIndex(0), comdat: OptId::NONE })
                .unwrap();
        }
        c
    }

    // spec: incr.md IN-55 option (A): only kept, non-alloc debug sections
    // of the two names; COMDAT and stripped states kept; alloc kept.
    #[test]
    fn left_out_only_kept_nonalloc_gnu_sets() {
        use SectionKind as K;
        let kept = SectionState::Kept;
        let comdat = SectionState::Dropped(DropReason::Comdat);
        let mut hs = Column::new();
        let mut kinds = Vec::new();
        for (name, flags, kind, state) in [
            (&b""[..], 0, K::Null, SectionState::Dropped(DropReason::Consumed)),
            (b".debug_gnu_pubnames", 0, K::Debug, kept),
            (b".debug_gnu_pubtypes", 0, K::Debug, kept),
            (b".debug_gnu_pubnames", 0, K::Debug, comdat),
            (b".debug_gnu_pubnames", SHF_ALLOC, K::Regular, kept),
            (b".debug_pubnames", 0, K::Debug, kept),
            (b".debug_names", 0, K::Debug, kept),
            (b".debug_info", 0, K::Debug, kept),
            (b".debug_gnu_pubtypes", 0, K::Merge, kept),
        ] {
            hs.push(header(name, flags)).unwrap();
            kinds.push((kind, state));
        }
        let mut res = Resolution::empty(0, 0, 0);
        res.sections = rows(&kinds);
        assert_eq!(discard(&hs, &mut res), 3);
        let states: Vec<_> = res.sections.iter().map(|(_, r)| r.state).collect();
        let stripped = SectionState::Dropped(DropReason::Stripped);
        assert_eq!(
            states,
            [
                SectionState::Dropped(DropReason::Consumed),
                stripped,
                stripped,
                comdat,
                kept,
                kept,
                kept,
                kept,
                stripped
            ]
        );
        // A second call drops nothing more.
        assert_eq!(discard(&hs, &mut res), 0);
        assert!(left_out(b".debug_gnu_pubtypes") && !left_out(b".debug_gnu_pubnames.dwo"));
    }
}

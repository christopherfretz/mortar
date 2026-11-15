//! `SHT_REL` relocations at fill (WP-030c, i386; `spec/x86.md` R-13,
//! `spec/arch.md` §3.3).
//!
//! The records of a `SHT_REL` table carry no addend: it is the relocated
//! field's input bytes (X86-4). The architecture reads it itself in
//! `apply`, from `RelocAt.site` (the input section, or the snapshot of
//! the clip and its margins that `reloc::relocate` takes before any
//! record is applied: R-13 (a), (d)) or through `SectionOut::input`; so
//! `RelocAt.addend` is `None`. The image needs the addend only where it
//! uses one itself: a section-symbol reference into a split section
//! selects its piece by `st_value + A` (synth.md S-17, X86-8), and the
//! value it hands `apply` is the piece's `S'` less `A`.
//!
//! These are the only REL paths of fill; the RELA paths dispatch here at
//! the record decode and at the addend, and are otherwise unchanged
//! (`reloc.rs`). A REL table is never deferred (I-14's fast path is
//! RELA's: `mortar_scan` walks every REL section).

use mortar_arch::{Arch, RelForm, Site};
use mortar_elf::Class;
use mortar_elf::raw::{Record, Rel, Rela};

/// The architecture's relocation sections are `SHT_REL`.
#[inline]
pub(crate) fn is_rel(arch: &dyn Arch) -> bool {
    arch.facts().rel_form == RelForm::Rel
}

/// One `SHT_REL` record, as the RELA decode's record with addend 0
/// (arch.md §3.5 `Records`: "a REL record's addend as whatever its caller
/// read, or 0").
#[inline]
pub(crate) fn decode(b: &[u8], class: Class) -> Option<Rela> {
    let r = match class {
        Class::Elf64 => Rel::decode64(b),
        Class::Elf32 => Rel::decode32(b),
    }?;
    Some(Rela { r_offset: r.r_offset, r_sym: r.r_sym, r_type: r.r_type, r_addend: 0 })
}

/// The implicit addend `A` of a classified record at `site` (X86-4,
/// arch.md §3.3): `read_addend` over the field's input bytes. `None` when
/// the field is not in `site` (the snapshot covers every picked record's
/// field: `SITE_BACK`/`SITE_FWD` beyond `max_reach`).
pub(crate) fn addend(arch: &dyn Arch, h: &mortar_arch::Howto, site: Site<'_>) -> Option<i64> {
    let width = usize::from(h.field.width());
    let field = site.at(width)?;
    Some(arch.read_addend(h, field))
}

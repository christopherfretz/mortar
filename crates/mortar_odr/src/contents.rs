//! W4's encoding of a copy's contents and its comparison (`spec/odr.md`
//! §8.1, "W4's comparison"; WP-146): the section's bytes, and its
//! relocations with every target encoded so that two units compare. A
//! global or weak target is its name and addend; a unit-local target (a
//! local symbol or a section symbol) is what it refers to (a string, a
//! merged entry) or, when that cannot be read, its kind (code or data)
//! only, never its place in the unit, which differs whenever the units'
//! other contents do (X21 names what that misses).
//!
//! Only W4 (`--odr=strict`) calls this: the default mode never reads a
//! byte here (O-42).

use std::borrow::Cow;

use mortar_elf::consts::{
    EM_386, EM_AARCH64, EM_X86_64, SHF_EXECINSTR, SHF_MERGE, SHF_STRINGS, SHT_NOBITS, SHT_REL,
    SHT_RELA, STB_LOCAL, STT_SECTION,
};
use mortar_elf::reloc::{aarch64, i386, x86_64};
use mortar_elf::{Object, SectionIndex, SymbolIndex, SymbolSection, SymbolTable};

/// A copy's section as W4 compares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Contents {
    bytes: Bytes,
    relocs: Vec<Reloc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Bytes {
    /// The bytes, with the fields of `SHT_REL` relocations to unit-local
    /// targets zeroed (their implicit addend is an address in the unit).
    Data(Vec<u8>),
    /// `SHT_NOBITS`: its size.
    Nobits(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Reloc {
    offset: u64,
    r_type: u32,
    target: Target,
}

/// What a relocation refers to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    /// A global, weak or unique symbol: by name and addend.
    Key { name: Vec<u8>, addend: i64 },
    /// The string of a `SHF_MERGE|SHF_STRINGS` section that starts at the
    /// target, without its terminator.
    Str { entsize: u64, bytes: Vec<u8>, code: bool },
    /// The entry of a `SHF_MERGE` section that holds the target, and the
    /// target's offset inside it.
    Entry { entsize: u64, bytes: Vec<u8>, at: u64, code: bool },
    /// Any other unit-local target: its kind only (the conservative path).
    Local { code: bool },
}

impl Target {
    /// Whether two targets agree (W4's rule, item 2).
    fn agrees(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Key { .. }, Self::Key { .. }) => self == other,
            // A key and a unit-local target: the conservative path (a
            // local alias of a global function is the same target).
            (Self::Key { .. }, _) | (_, Self::Key { .. }) => true,
            (Self::Str { .. }, Self::Str { .. }) | (Self::Entry { .. }, Self::Entry { .. }) => {
                self == other
            }
            (a, b) => a.code() == b.code(),
        }
    }

    const fn code(&self) -> bool {
        match self {
            Self::Key { .. } => false,
            Self::Str { code, .. } | Self::Entry { code, .. } | Self::Local { code } => *code,
        }
    }

    /// Both read by content (strings, or entries): a disagreement is in
    /// the data referred to.
    const fn by_content(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (Self::Str { .. }, Self::Str { .. }) | (Self::Entry { .. }, Self::Entry { .. })
        )
    }
}

/// Where two copies first differ, for the finding's note; `None` when
/// they are the same.
pub(crate) fn differ(a: &Contents, b: &Contents) -> Option<String> {
    let bytes = a.bytes != b.bytes;
    let relocs = if a.relocs.len() == b.relocs.len() {
        a.relocs.iter().zip(&b.relocs).find_map(|(x, y)| {
            if x.offset != y.offset || x.r_type != y.r_type {
                Some("its relocations".to_owned())
            } else if x.target.agrees(&y.target) {
                None
            } else if x.target.by_content(&y.target) {
                Some(format!("the data its relocation at offset {} refers to", x.offset))
            } else {
                Some("its relocations".to_owned())
            }
        })
    } else {
        Some("its relocations".to_owned())
    };
    match (bytes, relocs) {
        (false, None) => None,
        (true, None) => Some("its bytes".to_owned()),
        (false, Some(r)) => Some(r),
        (true, Some(r)) => Some(format!("its bytes and {r}")),
    }
}

/// The contents of `section` of `obj`, its relocations those of `reloc`
/// (`None`: it has none). `data` gives a section's bytes (borrowed when
/// resident). `None` when something cannot be read, or an `SHT_REL`
/// relocation to a unit-local target has a type whose field width W4's
/// table does not give: the copy takes no part.
pub(crate) fn read<'o>(
    obj: &Object<'o>,
    table: &SymbolTable<'o>,
    section: SectionIndex,
    reloc: Option<SectionIndex>,
    data: &dyn Fn(SectionIndex) -> Option<Cow<'o, [u8]>>,
) -> Option<Contents> {
    let sec = obj.section(section).ok()?;
    let mut bytes = if sec.sh_type() == SHT_NOBITS {
        Bytes::Nobits(sec.size())
    } else {
        Bytes::Data(data(section)?.into_owned())
    };
    let mut relocs = Vec::new();
    if let Some(reloc) = reloc {
        let machine = obj.header().ehdr.e_machine;
        let rsec = obj.section(reloc).ok()?;
        let raw = data(reloc)?;
        let mut pools = Pools { obj, data, read: Vec::new() };
        match rsec.sh_type() {
            SHT_RELA => {
                for r in rsec.rela_in(&raw).ok()? {
                    let target = pools.target(table, machine, r.r_sym, r.r_type, r.r_addend)?;
                    relocs.push(Reloc { offset: r.r_offset, r_type: r.r_type, target });
                }
            }
            SHT_REL => {
                for r in rsec.rel_in(&raw).ok()? {
                    let sym = table.get(SymbolIndex(r.r_sym)).ok()?;
                    let target = if sym.binding() == STB_LOCAL {
                        // The implicit addend is the field's bytes: read it
                        // for the lookup, then leave it out of the bytes.
                        let width = rel_width(machine, r.r_type)?;
                        let Bytes::Data(b) = &mut bytes else { return None };
                        let start = usize::try_from(r.r_offset).ok()?;
                        let field = b.get_mut(start..start.checked_add(width)?)?;
                        let addend = signed(field);
                        field.fill(0);
                        pools.target(table, machine, r.r_sym, r.r_type, addend)?
                    } else {
                        pools.target(table, machine, r.r_sym, r.r_type, 0)?
                    };
                    relocs.push(Reloc { offset: r.r_offset, r_type: r.r_type, target });
                }
            }
            _ => return None,
        }
    }
    // Ties keep the relocation section's order (a stable sort).
    relocs.sort_by_key(|r| (r.offset, r.r_type));
    Some(Contents { bytes, relocs })
}

/// A little-endian field, sign-extended.
fn signed(field: &[u8]) -> i64 {
    let mut word = [0u8; 8];
    word[..field.len()].copy_from_slice(field);
    let value = u64::from_le_bytes(word);
    let shift = 64 - 8 * field.len() as u32;
    ((value << shift) as i64) >> shift
}

/// The field width of an `SHT_REL` relocation type (W4's table); `None`
/// for one it does not give.
const fn rel_width(machine: u16, r_type: u32) -> Option<usize> {
    if machine != EM_386 {
        return None;
    }
    match r_type {
        i386::R_386_32
        | i386::R_386_PC32
        | i386::R_386_GOT32
        | i386::R_386_PLT32
        | i386::R_386_GOTOFF
        | i386::R_386_GOTPC
        | i386::R_386_GOT32X
        | i386::R_386_TLS_TPOFF
        | i386::R_386_TLS_IE
        | i386::R_386_TLS_GOTIE
        | i386::R_386_TLS_LE
        | i386::R_386_TLS_GD
        | i386::R_386_TLS_LDM
        | i386::R_386_TLS_GD_32
        | i386::R_386_TLS_LDM_32
        | i386::R_386_TLS_LDO_32
        | i386::R_386_TLS_IE_32
        | i386::R_386_TLS_LE_32
        | i386::R_386_TLS_DTPMOD32
        | i386::R_386_TLS_DTPOFF32
        | i386::R_386_TLS_TPOFF32
        | i386::R_386_TLS_GOTDESC
        | i386::R_386_TLS_DESC => Some(4),
        i386::R_386_16 | i386::R_386_PC16 => Some(2),
        i386::R_386_8 | i386::R_386_PC8 => Some(1),
        _ => None,
    }
}

/// Whether a relocation type's value is S + A, so that the target is the
/// symbol's value plus the addend.
const fn absolute(machine: u16, r_type: u32) -> bool {
    match machine {
        EM_AARCH64 => matches!(
            r_type,
            aarch64::R_AARCH64_ABS64 | aarch64::R_AARCH64_ABS32 | aarch64::R_AARCH64_ABS16
        ),
        EM_X86_64 => matches!(
            r_type,
            x86_64::R_X86_64_64
                | x86_64::R_X86_64_32
                | x86_64::R_X86_64_32S
                | x86_64::R_X86_64_16
                | x86_64::R_X86_64_8
        ),
        EM_386 => matches!(r_type, i386::R_386_32 | i386::R_386_16 | i386::R_386_8),
        _ => false,
    }
}

/// The target sections read so far, so that one pool is read once a copy.
struct Pools<'r, 'o> {
    obj: &'r Object<'o>,
    data: &'r dyn Fn(SectionIndex) -> Option<Cow<'o, [u8]>>,
    read: Vec<(SectionIndex, Cow<'o, [u8]>)>,
}

impl<'o> Pools<'_, 'o> {
    fn bytes(&mut self, section: SectionIndex) -> Option<&[u8]> {
        let at = if let Some(at) = self.read.iter().position(|(s, _)| *s == section) {
            at
        } else {
            self.read.push((section, (self.data)(section)?));
            self.read.len() - 1
        };
        Some(&self.read[at].1)
    }

    /// The target of relocation `r_sym`, `r_type`, `addend`.
    fn target(
        &mut self,
        table: &SymbolTable<'o>,
        machine: u16,
        r_sym: u32,
        r_type: u32,
        addend: i64,
    ) -> Option<Target> {
        let index = SymbolIndex(r_sym);
        let sym = table.get(index).ok()?;
        if sym.binding() != STB_LOCAL && sym.sym_type() != STT_SECTION {
            return Some(Target::Key { name: table.name(&sym).ok()?.to_vec(), addend });
        }
        let Ok(SymbolSection::Section(section)) = table.section_of(index, &sym) else {
            return Some(Target::Local { code: false });
        };
        let sec = self.obj.section(section).ok()?;
        let flags = sec.flags();
        let code = flags & SHF_EXECINSTR != 0;
        let entsize = sec.header().sh_entsize;
        let conservative = Some(Target::Local { code });
        if flags & SHF_MERGE == 0
            || entsize == 0
            || sec.sh_type() == SHT_NOBITS
            || !absolute(machine, r_type)
        {
            return conservative;
        }
        let place = i128::from(sym.st_value) + i128::from(addend);
        let Ok(p) = u64::try_from(place) else { return conservative };
        if p >= sec.size() {
            return conservative;
        }
        let Some(pool) = self.bytes(section) else { return conservative };
        let (Ok(p), Ok(e)) = (usize::try_from(p), usize::try_from(entsize)) else {
            return conservative;
        };
        if flags & SHF_STRINGS != 0 {
            if p % e != 0 {
                return conservative;
            }
            let end = (p..pool.len())
                .step_by(e)
                .find(|&at| pool.get(at..at + e).is_some_and(|c| c.iter().all(|b| *b == 0)));
            let Some(end) = end else { return conservative };
            Some(Target::Str { entsize, bytes: pool[p..end].to_vec(), code })
        } else {
            let base = p - p % e;
            let Some(entry) = pool.get(base..base + e) else { return conservative };
            Some(Target::Entry { entsize, bytes: entry.to_vec(), at: (p % e) as u64, code })
        }
    }
}

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

#[cfg(test)]
mod tests {
    //! W4's rule on constructed objects, both directions (the mutation
    //! rule): equal data referred to from different places is the same;
    //! different data, a different entry or a different place inside equal
    //! strings is a finding.
    use super::*;
    use mortar_elf::consts::{
        EM_386, EM_AARCH64, EM_X86_64, SHF_ALLOC, SHF_EXECINSTR, SHF_MERGE, SHF_STRINGS, SHF_WRITE,
        SHT_PROGBITS, SHT_REL, SHT_RELA, SHT_STRTAB, SHT_SYMTAB,
    };
    use mortar_elf::reloc::{aarch64, i386, x86_64};

    const STR: u32 = 1; // section symbol of .rodata.str1.1
    const CST: u32 = 2; // section symbol of .rodata.cst8
    const RO: u32 = 3; // section symbol of .rodata
    const TEXT: u32 = 4; // section symbol of .text
    const LV: u32 = 5; // named local object `lv` in .rodata
    const G: u32 = 6; // undefined global `g`

    /// (name, type, flags, contents, link, info, align, entsize).
    type Sec = (u32, u32, u64, Vec<u8>, u32, u32, u64, u64);

    /// One unit: the copy `.data.x` (`len` bytes) and its relocations
    /// (offset, type, symbol, addend: in the bytes for `SHT_REL`), with a
    /// string pool, an 8-byte constant pool, a plain `.rodata` and `.text`.
    struct Unit<'a> {
        machine: u16,
        strings: &'a [u8],
        cst8: &'a [u8],
        rodata: &'a [u8],
        len: usize,
        relocs: &'a [(u64, u32, u32, i64)],
    }

    impl Unit<'_> {
        #[allow(clippy::too_many_lines, reason = "one object, written out")]
        fn bytes(&self) -> Vec<u8> {
            let wide = self.machine != EM_386;
            let word = |out: &mut Vec<u8>, v: u64| {
                if wide {
                    out.extend_from_slice(&v.to_le_bytes());
                } else {
                    out.extend_from_slice(&(v as u32).to_le_bytes());
                }
            };
            let mut data = vec![0u8; self.len];
            let mut rel = Vec::new();
            for &(offset, r_type, sym, addend) in self.relocs {
                word(&mut rel, offset);
                if wide {
                    rel.extend_from_slice(
                        &((u64::from(sym) << 32) | u64::from(r_type)).to_le_bytes(),
                    );
                    rel.extend_from_slice(&addend.to_le_bytes());
                } else {
                    rel.extend_from_slice(&((sym << 8) | r_type).to_le_bytes());
                    let at = offset as usize;
                    data[at..at + 4].copy_from_slice(&(addend as i32).to_le_bytes());
                }
            }
            let strtab = b"\0lv\0g\0".to_vec();
            let mut symtab = Vec::new();
            // (name, info, shndx, value, size)
            let syms: [(u32, u8, u16, u64, u64); 7] = [
                (0, 0, 0, 0, 0),
                (0, 3, 3, 0, 0),
                (0, 3, 4, 0, 0),
                (0, 3, 5, 0, 0),
                (0, 3, 6, 0, 0),
                (1, 1, 5, 0, 4),
                (4, 0x10, 0, 0, 0),
            ];
            for (name, info, shndx, value, size) in syms {
                symtab.extend_from_slice(&name.to_le_bytes());
                if wide {
                    symtab.extend_from_slice(&[info, 0]);
                    symtab.extend_from_slice(&shndx.to_le_bytes());
                    symtab.extend_from_slice(&value.to_le_bytes());
                    symtab.extend_from_slice(&size.to_le_bytes());
                } else {
                    symtab.extend_from_slice(&(value as u32).to_le_bytes());
                    symtab.extend_from_slice(&(size as u32).to_le_bytes());
                    symtab.extend_from_slice(&[info, 0]);
                    symtab.extend_from_slice(&shndx.to_le_bytes());
                }
            }
            let (rel_name, rel_type, rel_size): (&[u8], u32, u64) =
                if wide { (b".rela.data.x", SHT_RELA, 24) } else { (b".rel.data.x", SHT_REL, 8) };
            let mut shstr = vec![0u8];
            let mut name = |n: &[u8]| {
                let at = shstr.len() as u32;
                shstr.extend_from_slice(n);
                shstr.push(0);
                at
            };
            // (name, type, flags, contents, link, info, align, entsize)
            let mut secs: Vec<Sec> = vec![
                (name(b".data.x"), SHT_PROGBITS, SHF_ALLOC | SHF_WRITE, data, 0, 0, 8, 0),
                (name(rel_name), rel_type, 0x40, rel, 7, 1, 8, rel_size),
                (
                    name(b".rodata.str1.1"),
                    SHT_PROGBITS,
                    SHF_ALLOC | SHF_MERGE | SHF_STRINGS,
                    self.strings.to_vec(),
                    0,
                    0,
                    1,
                    1,
                ),
                (
                    name(b".rodata.cst8"),
                    SHT_PROGBITS,
                    SHF_ALLOC | SHF_MERGE,
                    self.cst8.to_vec(),
                    0,
                    0,
                    8,
                    8,
                ),
                (name(b".rodata"), SHT_PROGBITS, SHF_ALLOC, self.rodata.to_vec(), 0, 0, 8, 0),
                (name(b".text"), SHT_PROGBITS, SHF_ALLOC | SHF_EXECINSTR, vec![0; 16], 0, 0, 4, 0),
                (name(b".symtab"), SHT_SYMTAB, 0, symtab, 8, 6, 8, if wide { 24 } else { 16 }),
                (name(b".strtab"), SHT_STRTAB, 0, strtab, 0, 0, 1, 0),
            ];
            let shstrndx = secs.len() as u16 + 1;
            let shstr_name = name(b".shstrtab");
            secs.push((shstr_name, SHT_STRTAB, 0, shstr, 0, 0, 1, 0));
            let ehsize: usize = if wide { 64 } else { 52 };
            let mut out = vec![0u8; ehsize];
            let mut headers = vec![0u8; if wide { 64 } else { 40 }];
            for (name, sh_type, flags, contents, link, info, align, entsize) in secs {
                while !out.len().is_multiple_of(8) {
                    out.push(0);
                }
                let offset = out.len() as u64;
                out.extend_from_slice(&contents);
                headers.extend_from_slice(&name.to_le_bytes());
                headers.extend_from_slice(&sh_type.to_le_bytes());
                word(&mut headers, flags);
                word(&mut headers, 0);
                word(&mut headers, offset);
                word(&mut headers, contents.len() as u64);
                headers.extend_from_slice(&link.to_le_bytes());
                headers.extend_from_slice(&info.to_le_bytes());
                word(&mut headers, align);
                word(&mut headers, entsize);
            }
            while !out.len().is_multiple_of(8) {
                out.push(0);
            }
            let shoff = out.len() as u64;
            out.extend_from_slice(&headers);
            let shnum = shstrndx + 1;
            let mut e = vec![0x7f, b'E', b'L', b'F', if wide { 2 } else { 1 }, 1, 1, 0];
            e.extend_from_slice(&[0; 8]);
            e.extend_from_slice(&1u16.to_le_bytes()); // ET_REL
            e.extend_from_slice(&self.machine.to_le_bytes());
            e.extend_from_slice(&1u32.to_le_bytes());
            word(&mut e, 0);
            word(&mut e, 0);
            word(&mut e, shoff);
            e.extend_from_slice(&0u32.to_le_bytes());
            for half in [ehsize as u16, 0, 0, if wide { 64 } else { 40 }, shnum, shstrndx] {
                e.extend_from_slice(&half.to_le_bytes());
            }
            out[..ehsize].copy_from_slice(&e);
            out
        }

        fn contents(&self) -> Option<Contents> {
            let bytes = self.bytes();
            let obj = Object::parse(&bytes).expect("a constructed object");
            let table = obj.symbols().expect("symbols").expect("a symbol table");
            let data = |s: SectionIndex| obj.section(s).ok()?.data().ok().map(Cow::Borrowed);
            read(&obj, &table, SectionIndex(1), Some(SectionIndex(2)), &data)
        }
    }

    /// A unit of `machine` whose copy holds one pointer: `reloc` at 0.
    fn one(machine: u16, strings: &[u8], reloc: (u32, u32, i64)) -> Option<Contents> {
        let cst8 = [1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0];
        let rodata = [0u8; 32];
        Unit {
            machine,
            strings,
            cst8: &cst8,
            rodata: &rodata,
            len: 16,
            relocs: &[(0, reloc.0, reloc.1, reloc.2)],
        }
        .contents()
    }

    fn compare(a: Option<Contents>, b: Option<Contents>) -> Option<String> {
        differ(&a.expect("first readable"), &b.expect("second readable"))
    }

    const ABS64: u32 = aarch64::R_AARCH64_ABS64;
    const REFERS: &str = "the data its relocation at offset 0 refers to";

    /// The kiln shape (`__bool_strings<char>::__true`): a section symbol of
    /// `.rodata.str1.1` plus an addend that differs with the unit's pool.
    #[test]
    fn same_string_at_different_places_is_the_same() {
        let a = one(EM_AARCH64, b"alpha\0beta\0true\0false\0", (ABS64, STR, 11));
        let b = one(EM_AARCH64, b"true\0false\0", (ABS64, STR, 0));
        assert_eq!(compare(a, b), None);
        let a = one(EM_AARCH64, b"alpha\0beta\0true\0false\0", (ABS64, STR, 16));
        let b = one(EM_AARCH64, b"true\0false\0", (ABS64, STR, 5));
        assert_eq!(compare(a, b), None);
        let a = one(EM_X86_64, b"omega\0true\0", (x86_64::R_X86_64_64, STR, 6));
        let b = one(EM_X86_64, b"true\0", (x86_64::R_X86_64_64, STR, 0));
        assert_eq!(compare(a, b), None);
    }

    #[test]
    fn different_string_is_a_finding() {
        let a = one(EM_AARCH64, b"alpha\0true\0", (ABS64, STR, 6));
        let b = one(EM_AARCH64, b"True\0", (ABS64, STR, 0));
        assert_eq!(compare(a, b).as_deref(), Some(REFERS));
        // At the same place too, which the first rule's name and addend
        // could not see.
        let a = one(EM_AARCH64, b"true\0", (ABS64, STR, 0));
        let b = one(EM_AARCH64, b"True\0", (ABS64, STR, 0));
        assert_eq!(compare(a, b).as_deref(), Some(REFERS));
    }

    /// `"xtrue"+1` is `"true"`; `"true"+1` is not.
    #[test]
    fn the_place_inside_equal_strings_counts() {
        let a = one(EM_AARCH64, b"xtrue\0", (ABS64, STR, 1));
        let b = one(EM_AARCH64, b"true\0", (ABS64, STR, 0));
        assert_eq!(compare(a, b), None);
        let a = one(EM_AARCH64, b"true\0", (ABS64, STR, 0));
        let b = one(EM_AARCH64, b"true\0", (ABS64, STR, 1));
        assert_eq!(compare(a, b).as_deref(), Some(REFERS));
    }

    #[test]
    fn merged_entries_by_their_bytes_and_the_place_inside() {
        // Entry 1 (the value 2) in both, at different places.
        let a = one(EM_AARCH64, b"\0", (ABS64, CST, 8));
        let b = Unit {
            machine: EM_AARCH64,
            strings: b"\0",
            cst8: &[2, 0, 0, 0, 0, 0, 0, 0],
            rodata: &[0; 8],
            len: 16,
            relocs: &[(0, ABS64, CST, 0)],
        }
        .contents();
        assert_eq!(compare(a, b), None);
        // A different entry.
        let a = one(EM_AARCH64, b"\0", (ABS64, CST, 0));
        let b = one(EM_AARCH64, b"\0", (ABS64, CST, 8));
        assert_eq!(compare(a, b).as_deref(), Some(REFERS));
        // The same entry, another place inside it.
        let a = one(EM_AARCH64, b"\0", (ABS64, CST, 8));
        let b = one(EM_AARCH64, b"\0", (ABS64, CST, 9));
        assert_eq!(compare(a, b).as_deref(), Some(REFERS));
    }

    /// The conservative path: any other unit-local target by its kind.
    #[test]
    fn other_local_targets_by_kind_only() {
        let a = one(EM_AARCH64, b"\0", (ABS64, RO, 16));
        let b = one(EM_AARCH64, b"\0", (ABS64, RO, 0));
        assert_eq!(compare(a, b), None);
        // GCC -O2's named local and Clang's section symbol.
        let a = one(EM_AARCH64, b"\0", (ABS64, LV, 0));
        let b = one(EM_AARCH64, b"\0", (ABS64, RO, 8));
        assert_eq!(compare(a, b), None);
        // A string in a mergeable pool and one in a plain `.rodata` (GCC
        // -O0 against Clang).
        let a = one(EM_AARCH64, b"true\0", (ABS64, STR, 0));
        let b = one(EM_AARCH64, b"\0", (ABS64, RO, 0));
        assert_eq!(compare(a, b), None);
        // Code against data is a finding.
        let a = one(EM_AARCH64, b"\0", (ABS64, TEXT, 0));
        let b = one(EM_AARCH64, b"\0", (ABS64, RO, 0));
        assert_eq!(compare(a, b).as_deref(), Some("its relocations"));
        // A type that is not absolute is not read by content.
        let prel = aarch64::R_AARCH64_PREL32;
        let a = one(EM_AARCH64, b"true\0", (prel, STR, 0));
        let b = one(EM_AARCH64, b"True\0", (prel, STR, 0));
        assert_eq!(compare(a, b), None);
        // A string without a terminator, or a place past the pool.
        let a = one(EM_AARCH64, b"true", (ABS64, STR, 0));
        let b = one(EM_AARCH64, b"True", (ABS64, STR, 0));
        assert_eq!(compare(a, b), None);
        let a = one(EM_AARCH64, b"true\0", (ABS64, STR, 9));
        let b = one(EM_AARCH64, b"True\0", (ABS64, STR, 0));
        assert_eq!(compare(a, b), None);
    }

    /// A key stays compared by name and addend; a key against a local is
    /// the conservative path (a local alias).
    #[test]
    fn keys_by_name_and_addend() {
        let a = one(EM_AARCH64, b"\0", (ABS64, G, 0));
        let b = one(EM_AARCH64, b"\0", (ABS64, G, 0));
        assert_eq!(compare(a, b), None);
        let a = one(EM_AARCH64, b"\0", (ABS64, G, 0));
        let b = one(EM_AARCH64, b"\0", (ABS64, G, 8));
        assert_eq!(compare(a, b).as_deref(), Some("its relocations"));
        let a = one(EM_AARCH64, b"\0", (ABS64, G, 0));
        let b = one(EM_AARCH64, b"\0", (ABS64, TEXT, 0));
        assert_eq!(compare(a, b), None);
    }

    /// i386: the implicit addend is in the bytes; for a unit-local target
    /// it is read for the lookup and left out of the byte comparison.
    #[test]
    fn rel_implicit_addends() {
        let r32 = i386::R_386_32;
        let a = one(EM_386, b"alpha\0true\0", (r32, STR, 6));
        let b = one(EM_386, b"true\0", (r32, STR, 0));
        assert_eq!(compare(a, b), None);
        let a = one(EM_386, b"alpha\0true\0", (r32, STR, 6));
        let b = one(EM_386, b"True\0", (r32, STR, 0));
        assert_eq!(compare(a, b).as_deref(), Some(REFERS));
        let a = one(EM_386, b"\0", (r32, RO, 24));
        let b = one(EM_386, b"\0", (r32, RO, 0));
        assert_eq!(compare(a, b), None);
        // A key's implicit addend stays in the bytes.
        let a = one(EM_386, b"\0", (r32, G, 0));
        let b = one(EM_386, b"\0", (r32, G, 4));
        assert_eq!(compare(a, b).as_deref(), Some("its bytes"));
        // A field width W4's table does not give: the copy takes no part.
        assert_eq!(one(EM_386, b"\0", (i386::R_386_RELATIVE, RO, 0)), None);
    }

    #[test]
    fn bytes_and_data_together() {
        let a = Unit {
            machine: EM_AARCH64,
            strings: b"true\0",
            cst8: &[0; 8],
            rodata: &[0; 8],
            len: 16,
            relocs: &[(8, ABS64, STR, 0)],
        };
        let mut b_bytes = a.contents().expect("readable");
        if let Bytes::Data(d) = &mut b_bytes.bytes {
            d[0] = 1;
        }
        let b = Unit { strings: b"True\0", ..a };
        let mut b = b.contents().expect("readable");
        b.bytes = b_bytes.bytes;
        assert_eq!(
            differ(&a.contents().expect("readable"), &b).as_deref(),
            Some("its bytes and the data its relocation at offset 8 refers to")
        );
    }
}

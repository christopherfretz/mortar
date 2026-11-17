//! The dynamic parts of the output as the loader finds them (stage 3,
//! `spec/plumb.md` §12): `PT_DYNAMIC`'s entries, and through their
//! addresses `.dynsym`, `.dynstr`, `.gnu.hash`, the version tables and the
//! relocation records. Everything is read through the output's memory
//! (the `PT_LOAD` segments), never through the section headers, except
//! where a rule is about a section header.
// ELF fields and addresses read best by the specs' short names.
#![allow(clippy::many_single_char_names, clippy::similar_names)]

use crate::image::Image;

pub const DT_NULL: u64 = 0;
pub const DT_NEEDED: u64 = 1;
pub const DT_PLTRELSZ: u64 = 2;
pub const DT_PLTGOT: u64 = 3;
pub const DT_HASH: u64 = 4;
pub const DT_STRTAB: u64 = 5;
pub const DT_SYMTAB: u64 = 6;
pub const DT_RELA: u64 = 7;
pub const DT_RELASZ: u64 = 8;
pub const DT_RELAENT: u64 = 9;
pub const DT_STRSZ: u64 = 10;
pub const DT_SYMENT: u64 = 11;
pub const DT_INIT: u64 = 12;
pub const DT_FINI: u64 = 13;
pub const DT_SONAME: u64 = 14;
pub const DT_RPATH: u64 = 15;
pub const DT_REL: u64 = 17;
pub const DT_RELSZ: u64 = 18;
pub const DT_RELENT: u64 = 19;
pub const DT_PLTREL: u64 = 20;
pub const DT_DEBUG: u64 = 21;
pub const DT_TEXTREL: u64 = 22;
pub const DT_JMPREL: u64 = 23;
pub const DT_BIND_NOW: u64 = 24;
pub const DT_INIT_ARRAY: u64 = 25;
pub const DT_FINI_ARRAY: u64 = 26;
pub const DT_INIT_ARRAYSZ: u64 = 27;
pub const DT_FINI_ARRAYSZ: u64 = 28;
pub const DT_RUNPATH: u64 = 29;
pub const DT_FLAGS: u64 = 30;
pub const DT_PREINIT_ARRAY: u64 = 32;
pub const DT_PREINIT_ARRAYSZ: u64 = 33;
pub const DT_RELRSZ: u64 = 35;
pub const DT_RELR: u64 = 36;
pub const DT_GNU_HASH: u64 = 0x6fff_fef5;
pub const DT_TLSDESC_PLT: u64 = 0x6fff_fef6;
pub const DT_TLSDESC_GOT: u64 = 0x6fff_fef7;
pub const DT_VERSYM: u64 = 0x6fff_fff0;
pub const DT_RELACOUNT: u64 = 0x6fff_fff9;
pub const DT_RELCOUNT: u64 = 0x6fff_fffa;
pub const DT_FLAGS_1: u64 = 0x6fff_fffb;
pub const DT_VERDEF: u64 = 0x6fff_fffc;
pub const DT_VERDEFNUM: u64 = 0x6fff_fffd;
pub const DT_VERNEED: u64 = 0x6fff_fffe;
pub const DT_VERNEEDNUM: u64 = 0x6fff_ffff;
pub const DT_AARCH64_BTI_PLT: u64 = 0x7000_0001;
pub const DT_AARCH64_PAC_PLT: u64 = 0x7000_0003;
pub const DT_AARCH64_VARIANT_PCS: u64 = 0x7000_0005;

pub const DF_TEXTREL: u64 = 4;
pub const DF_BIND_NOW: u64 = 8;
pub const DF_STATIC_TLS: u64 = 0x10;
pub const DF_1_NOW: u64 = 1;
pub const DF_ORIGIN: u64 = 1;
/// `vna_flags`: a weak version reference (gABI "Symbol Versioning").
pub const VER_FLG_WEAK: u16 = 2;
pub const DF_1_PIE: u64 = 0x0800_0000;

/// The name of a `DT_*` tag, for messages.
pub fn tag_name(tag: u64) -> String {
    let n = match tag {
        DT_NULL => "DT_NULL",
        DT_NEEDED => "DT_NEEDED",
        DT_PLTRELSZ => "DT_PLTRELSZ",
        DT_PLTGOT => "DT_PLTGOT",
        DT_HASH => "DT_HASH",
        DT_STRTAB => "DT_STRTAB",
        DT_SYMTAB => "DT_SYMTAB",
        DT_RELA => "DT_RELA",
        DT_RELASZ => "DT_RELASZ",
        DT_RELAENT => "DT_RELAENT",
        DT_STRSZ => "DT_STRSZ",
        DT_SYMENT => "DT_SYMENT",
        DT_INIT => "DT_INIT",
        DT_FINI => "DT_FINI",
        DT_SONAME => "DT_SONAME",
        // WP-161: the sweep names the field by it.
        16 => "DT_SYMBOLIC",
        DT_RPATH => "DT_RPATH",
        DT_REL => "DT_REL",
        DT_RELSZ => "DT_RELSZ",
        DT_RELENT => "DT_RELENT",
        DT_PLTREL => "DT_PLTREL",
        DT_DEBUG => "DT_DEBUG",
        DT_TEXTREL => "DT_TEXTREL",
        DT_JMPREL => "DT_JMPREL",
        DT_BIND_NOW => "DT_BIND_NOW",
        DT_INIT_ARRAY => "DT_INIT_ARRAY",
        DT_FINI_ARRAY => "DT_FINI_ARRAY",
        DT_INIT_ARRAYSZ => "DT_INIT_ARRAYSZ",
        DT_FINI_ARRAYSZ => "DT_FINI_ARRAYSZ",
        DT_RUNPATH => "DT_RUNPATH",
        DT_FLAGS => "DT_FLAGS",
        DT_PREINIT_ARRAY => "DT_PREINIT_ARRAY",
        DT_PREINIT_ARRAYSZ => "DT_PREINIT_ARRAYSZ",
        DT_RELR => "DT_RELR",
        DT_RELRSZ => "DT_RELRSZ",
        DT_GNU_HASH => "DT_GNU_HASH",
        DT_TLSDESC_PLT => "DT_TLSDESC_PLT",
        DT_TLSDESC_GOT => "DT_TLSDESC_GOT",
        DT_VERSYM => "DT_VERSYM",
        DT_RELACOUNT => "DT_RELACOUNT",
        DT_RELCOUNT => "DT_RELCOUNT",
        DT_FLAGS_1 => "DT_FLAGS_1",
        DT_VERDEF => "DT_VERDEF",
        DT_VERDEFNUM => "DT_VERDEFNUM",
        DT_VERNEED => "DT_VERNEED",
        DT_VERNEEDNUM => "DT_VERNEEDNUM",
        DT_AARCH64_BTI_PLT => "DT_AARCH64_BTI_PLT",
        DT_AARCH64_PAC_PLT => "DT_AARCH64_PAC_PLT",
        DT_AARCH64_VARIANT_PCS => "DT_AARCH64_VARIANT_PCS",
        _ => return format!("tag {tag:#x}"),
    };
    n.to_string()
}

/// A `.dynsym` entry, widened.
#[derive(Debug, Clone)]
pub struct DynSym {
    pub name: String,
    pub st_info: u8,
    pub st_other: u8,
    pub st_shndx: u16,
    pub st_value: u64,
    pub st_size: u64,
}

impl DynSym {
    pub fn binding(&self) -> u8 {
        self.st_info >> 4
    }
    pub fn sym_type(&self) -> u8 {
        self.st_info & 15
    }
    pub fn visibility(&self) -> u8 {
        self.st_other & 3
    }
    pub fn defined(&self) -> bool {
        self.st_shndx != 0
    }
}

/// A relocation record of `.rela.dyn` / `.rela.plt` (REL: `addend` is
/// `None`, the place holds it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DynRel {
    /// The record's own address (for messages).
    pub at: u64,
    pub offset: u64,
    pub ty: u32,
    pub sym: u32,
    pub addend: Option<i64>,
    /// In `DT_JMPREL`'s table.
    pub plt: bool,
}

/// A `Vernaux` of `.gnu.version_r`, with its `Verneed`'s file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Need {
    pub file: String,
    pub file_off: u32,
    pub name: String,
    pub name_off: u32,
    pub hash: u32,
    pub flags: u16,
    pub other: u16,
    /// The index of its `Verneed` in the chain.
    pub verneed: usize,
}

/// A `Verdef` of `.gnu.version_d` with its `Verdaux` names (D-115).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdef {
    pub version: u16,
    pub flags: u16,
    pub ndx: u16,
    pub cnt: u16,
    pub hash: u32,
    /// The `Verdaux` names in chain order: the version's own, then its
    /// parents'.
    pub names: Vec<String>,
    /// `vd_next`: 0 on the last `Verdef` (P-D27).
    pub next: u32,
    /// The `vda_next` of the last `Verdaux` read (0 when the chain ends
    /// where `vd_cnt` says).
    pub aux_next: u32,
}

/// The `.gnu.hash` table as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GnuHash {
    pub nbuckets: u32,
    pub symoffset: u32,
    pub bloom_size: u32,
    pub bloom_shift: u32,
    pub bloom: Vec<u64>,
    pub buckets: Vec<u32>,
    /// The chain words, from `symoffset` to the last entry.
    pub chain: Vec<u32>,
    /// The table's size in bytes.
    pub size: u64,
}

/// The dynamic view of the output.
#[derive(Debug, Default)]
pub struct DynOut {
    /// `PT_DYNAMIC`'s address and size.
    pub dynamic: Option<(u64, u64)>,
    /// The entries in order, up to and including the first `DT_NULL`.
    pub entries: Vec<(u64, u64)>,
    /// `DT_NULL` entries from the first one to the end of `PT_DYNAMIC`.
    pub nulls: usize,
    pub strtab: Option<(u64, u64)>,
    pub syms: Vec<DynSym>,
    /// The `.gnu.version` entries, one per `.dynsym` entry, when present.
    pub versym: Option<Vec<u16>>,
    pub needs: Vec<Need>,
    /// `(vn_file offset, file, vn_cnt)` per `Verneed`, in chain order.
    pub verneeds: Vec<(u32, String, u16)>,
    /// `DT_VERDEF`'s chain, `DT_VERDEFNUM` entries at most (D-115; WP-121).
    pub verdefs: Vec<Verdef>,
    pub gnu_hash: Option<GnuHash>,
    /// `DT_RELA`/`DT_REL` records, then `DT_JMPREL` records.
    pub rels: Vec<DynRel>,
    pub needed: Vec<String>,
    /// The `Verneed`/`Vernaux` chains' shape as glibc walks them
    /// (`vn_version` 1, `vn_cnt` entries, the last links 0, `vna_flags`
    /// at most `VER_FLG_WEAK`): P-D8 (WP-082).
    pub version_problems: Vec<String>,
    /// What could not be read, as messages (each a P-D6 finding).
    pub problems: Vec<String>,
}

impl DynOut {
    /// The value of the first entry with `tag`.
    pub fn get(&self, tag: u64) -> Option<u64> {
        self.entries.iter().find(|e| e.0 == tag).map(|e| e.1)
    }

    pub fn has(&self, tag: u64) -> bool {
        self.entries.iter().any(|e| e.0 == tag)
    }

    /// A string of `.dynstr` at `off`.
    pub fn string(&self, image: &Image<'_>, off: u64) -> Option<String> {
        let (base, size) = self.strtab?;
        if off >= size {
            return None;
        }
        read_cstr(image, base + off, size - off)
    }

    /// The `.dynsym` index of the first entry named `name`.
    pub fn sym_index(&self, name: &str) -> Option<usize> {
        self.syms.iter().position(|s| s.name == name)
    }

    /// The version a `.dynsym` entry's `.gnu.version` index names: `None`
    /// for 0/1 or no table; `(file, version)` for a `Vernaux`.
    pub fn need_of(&self, sym: usize) -> Option<&Need> {
        let v = *self.versym.as_ref()?.get(sym)? & 0x7fff;
        if v < 2 {
            return None;
        }
        self.needs.iter().find(|n| n.other == v)
    }

    /// Reads the dynamic view (stage 3). `None` when the output has no
    /// `PT_DYNAMIC`.
    #[allow(clippy::too_many_lines)] // one block per table
    pub fn read(image: &Image<'_>) -> Option<DynOut> {
        let word: u64 = match image.class() {
            mortar_elf::Class::Elf32 => 4,
            mortar_elf::Class::Elf64 => 8,
        };
        let w = word as u8;
        let pd = image.phdrs.iter().find(|p| p.p_type == mortar_elf::consts::PT_DYNAMIC)?;
        let mut d = DynOut { dynamic: Some((pd.p_vaddr, pd.p_memsz)), ..DynOut::default() };
        let n = pd.p_memsz / (2 * word);
        let mut ended = false;
        for i in 0..n {
            let a = pd.p_vaddr + i * 2 * word;
            let (Some(tag), Some(val)) = (image.value(a, w), image.value(a + word, w)) else {
                d.problems.push(format!("PT_DYNAMIC entry {i} is not in the output's memory"));
                break;
            };
            if ended {
                if tag == DT_NULL {
                    d.nulls += 1;
                }
                continue;
            }
            d.entries.push((tag, val));
            if tag == DT_NULL {
                ended = true;
                d.nulls = 1;
            }
        }
        if !ended {
            d.problems.push("PT_DYNAMIC has no DT_NULL entry".into());
        }
        let strtab = d.get(DT_STRTAB).zip(d.get(DT_STRSZ));
        d.strtab = strtab;
        if strtab.is_some_and(|(a, s)| image.bytes(a, s).is_none()) {
            d.problems.push("DT_STRTAB/DT_STRSZ is not in the output's file image".into());
            d.strtab = None;
        }
        for (tag, v) in d.entries.clone() {
            if tag == DT_NEEDED {
                match d.string(image, v) {
                    Some(s) => d.needed.push(s),
                    None => d.problems.push(format!("DT_NEEDED offset {v:#x} is not a string")),
                }
            }
        }
        // .dynsym: its count from .gnu.hash (the loader's way), else the
        // section header's size.
        let entsize: u64 = if word == 8 { 24 } else { 16 };
        let symtab = d.get(DT_SYMTAB);
        let gh = d.get(DT_GNU_HASH).and_then(|a| read_gnu_hash(image, a, word, symtab, entsize));
        let count = match (&gh, symtab) {
            (Some(g), _) => Some(u64::from(g.symoffset) + g.chain.len() as u64),
            (None, Some(a)) => image
                .sections
                .iter()
                .find(|s| s.shdr.sh_addr == a && s.shdr.sh_type == mortar_elf::consts::SHT_DYNSYM)
                .map(|s| s.shdr.sh_size / entsize),
            _ => None,
        };
        // The section header's count wins when it is larger: a table the
        // hash does not cover (unhashed entries only) is still read.
        let count = match (count, symtab) {
            (Some(c), Some(a)) => {
                let sh = image
                    .sections
                    .iter()
                    .find(|s| {
                        s.shdr.sh_addr == a && s.shdr.sh_type == mortar_elf::consts::SHT_DYNSYM
                    })
                    .map_or(0, |s| s.shdr.sh_size / entsize);
                Some(c.max(sh))
            }
            (c, _) => c,
        };
        d.gnu_hash = gh;
        if let (Some(a), Some(c)) = (symtab, count) {
            for i in 0..c {
                let e = a + i * entsize;
                let Some(b) = image.bytes(e, entsize) else {
                    d.problems.push(format!(".dynsym entry {i} is not in the output's file image"));
                    break;
                };
                let (name, info, other, shndx, value, size) = if word == 8 {
                    (
                        u32le(b, 0),
                        b[4],
                        b[5],
                        u16::from_le_bytes([b[6], b[7]]),
                        u64le(b, 8),
                        u64le(b, 16),
                    )
                } else {
                    (
                        u32le(b, 0),
                        b[12],
                        b[13],
                        u16::from_le_bytes([b[14], b[15]]),
                        u64::from(u32le(b, 4)),
                        u64::from(u32le(b, 8)),
                    )
                };
                let name = d.string(image, u64::from(name)).unwrap_or_else(|| {
                    d.problems.push(format!(".dynsym entry {i}: st_name is not a string"));
                    String::new()
                });
                d.syms.push(DynSym {
                    name,
                    st_info: info,
                    st_other: other,
                    st_shndx: shndx,
                    st_value: value,
                    st_size: size,
                });
            }
        }
        if let Some(a) = d.get(DT_VERSYM) {
            let n = d.syms.len() as u64;
            match image.bytes(a, 2 * n) {
                Some(b) => {
                    d.versym =
                        Some(b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect());
                }
                None => d.problems.push("DT_VERSYM is not in the output's file image".into()),
            }
        }
        if let Some(a) = d.get(DT_VERNEED) {
            let num = d.get(DT_VERNEEDNUM).unwrap_or(0);
            let mut at = a;
            for vi in 0..num {
                let Some(b) = image.bytes(at, 16) else {
                    d.problems.push("a Verneed is not in the output's file image".into());
                    break;
                };
                let version = u16::from_le_bytes([b[0], b[1]]);
                let cnt = u16::from_le_bytes([b[2], b[3]]);
                let file_off = u32le(b, 4);
                let aux = u32le(b, 8);
                let next = u32le(b, 12);
                let file = d.string(image, u64::from(file_off)).unwrap_or_default();
                d.verneeds.push((file_off, file.clone(), cnt));
                if version != 1 {
                    // glibc refuses the object ("unsupported version of Verneed
                    // record", `_dl_check_map_versions`).
                    d.version_problems
                        .push(format!("the Verneed of '{file}' has vn_version {version}, not 1"));
                }
                if vi + 1 == num && next != 0 {
                    d.version_problems
                        .push(format!("the last Verneed ('{file}') has vn_next {next:#x}, not 0"));
                }
                let mut x = at + u64::from(aux);
                let mut seen = 0u16;
                for _ in 0..cnt {
                    let Some(v) = image.bytes(x, 16) else {
                        d.problems.push("a Vernaux is not in the output's file image".into());
                        break;
                    };
                    let name_off = u32le(v, 8);
                    d.needs.push(Need {
                        file: file.clone(),
                        file_off,
                        name: d.string(image, u64::from(name_off)).unwrap_or_default(),
                        name_off,
                        hash: u32le(v, 0),
                        flags: u16::from_le_bytes([v[4], v[5]]),
                        other: u16::from_le_bytes([v[6], v[7]]),
                        verneed: vi as usize,
                    });
                    let flags = u16::from_le_bytes([v[4], v[5]]);
                    if flags & !VER_FLG_WEAK != 0 {
                        d.version_problems.push(format!(
                            "a Vernaux of '{file}' has vna_flags {flags:#x} (only VER_FLG_WEAK is a reference's)"
                        ));
                    }
                    seen += 1;
                    let nx = u32le(v, 12);
                    if seen == cnt && nx != 0 {
                        d.version_problems.push(format!(
                            "the last Vernaux of '{file}' has vna_next {nx:#x}, not 0"
                        ));
                    }
                    if nx == 0 {
                        break;
                    }
                    x += u64::from(nx);
                }
                if seen != cnt {
                    d.version_problems.push(format!(
                        "the Verneed of '{file}' has vn_cnt {cnt}; its Vernaux chain has {seen}"
                    ));
                }
                if next == 0 {
                    break;
                }
                at += u64::from(next);
            }
        }
        if let Some(a) = d.get(DT_VERDEF) {
            d.read_verdefs(image, a);
        }
        // Relocation records.
        let rela = word == 8;
        let tables = [
            (d.get(DT_RELA).or(d.get(DT_REL)), d.get(DT_RELASZ).or(d.get(DT_RELSZ)), false),
            (d.get(DT_JMPREL), d.get(DT_PLTRELSZ), true),
        ];
        for (a, sz, plt) in tables {
            let (Some(a), Some(sz)) = (a, sz) else { continue };
            let ent = if rela { 3 * word } else { 2 * word };
            let Some(b) = image.bytes(a, sz) else {
                d.problems.push(format!(
                    "the relocation table at {a:#x} ({sz:#x} bytes) is not in the output's file image"
                ));
                continue;
            };
            for (i, r) in b.chunks_exact(ent as usize).enumerate() {
                let at = a + i as u64 * ent;
                let (offset, info, addend) = if word == 8 {
                    (u64le(r, 0), u64le(r, 8), rela.then(|| u64le(r, 16) as i64))
                } else {
                    (u64::from(u32le(r, 0)), u64::from(u32le(r, 4)), None)
                };
                let (ty, sym) = if word == 8 {
                    ((info & 0xffff_ffff) as u32, (info >> 32) as u32)
                } else {
                    ((info & 0xff) as u32, (info >> 8) as u32)
                };
                d.rels.push(DynRel { at, offset, ty, sym, addend, plt });
            }
        }
        Some(d)
    }
}

impl DynOut {
    /// `DT_VERDEF`'s chain (D-115): what cannot be read is a problem.
    fn read_verdefs(&mut self, image: &Image<'_>, a: u64) {
        let num = self.get(DT_VERDEFNUM).unwrap_or(0);
        let mut at = a;
        for _ in 0..num.min(1 << 16) {
            let Some(b) = image.bytes(at, 20) else {
                self.problems.push("a Verdef is not in the output's file image".into());
                return;
            };
            let u16at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
            let mut v = Verdef {
                version: u16at(0),
                flags: u16at(2),
                ndx: u16at(4),
                cnt: u16at(6),
                hash: u32le(b, 8),
                names: Vec::new(),
                next: u32le(b, 16),
                aux_next: 0,
            };
            let aux = u32le(b, 12);
            let next = u32le(b, 16);
            let mut x = at.wrapping_add(u64::from(aux));
            for _ in 0..v.cnt {
                let Some(c) = image.bytes(x, 8) else {
                    self.problems.push("a Verdaux is not in the output's file image".into());
                    break;
                };
                v.names.push(self.string(image, u64::from(u32le(c, 0))).unwrap_or_default());
                let nx = u32le(c, 4);
                v.aux_next = nx;
                if nx == 0 {
                    break;
                }
                x = x.wrapping_add(u64::from(nx));
            }
            self.verdefs.push(v);
            if next == 0 {
                return;
            }
            at = at.wrapping_add(u64::from(next));
        }
    }

    /// The version a defined `.dynsym` entry's `.gnu.version` index names
    /// through `.gnu.version_d`: `(name, hidden)`, `None` for index 0/1,
    /// no table, or an index no `Verdef` has.
    pub fn def_version(&self, sym: usize) -> Option<(String, bool)> {
        let raw = *self.versym.as_ref()?.get(sym)?;
        let v = raw & 0x7fff;
        if v < 2 {
            return None;
        }
        let d = self.verdefs.iter().find(|d| d.ndx == v)?;
        Some((d.names.first()?.clone(), raw & 0x8000 != 0))
    }
}

fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn u64le(b: &[u8], o: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(a)
}

/// A NUL-terminated string at `addr`, at most `max` bytes.
pub fn read_cstr(image: &Image<'_>, addr: u64, max: u64) -> Option<String> {
    let mut out = Vec::new();
    let mut a = addr;
    let end = addr.checked_add(max)?;
    // Read in chunks inside one segment.
    while a < end {
        let n = (end - a).min(256);
        let chunk = image.bytes(a, n).or_else(|| image.bytes(a, 1))?;
        if let Some(z) = chunk.iter().position(|&c| c == 0) {
            out.extend_from_slice(&chunk[..z]);
            return Some(String::from_utf8_lossy(&out).into_owned());
        }
        out.extend_from_slice(chunk);
        a += chunk.len() as u64;
    }
    None
}

fn read_gnu_hash(
    image: &Image<'_>,
    a: u64,
    word: u64,
    symtab: Option<u64>,
    entsize: u64,
) -> Option<GnuHash> {
    let h = image.bytes(a, 16)?;
    let nbuckets = u32le(h, 0);
    let symoffset = u32le(h, 4);
    let bloom_size = u32le(h, 8);
    let bloom_shift = u32le(h, 12);
    let mut at = a + 16;
    // Read the words before sizing the vector: a hostile `bloom_size`
    // must fail the read, not allocate gigabytes (WP-082's sweep).
    let bb = image.bytes(at, u64::from(bloom_size) * word)?;
    let mut bloom = Vec::with_capacity(bb.len() / word as usize);
    for c in bb.chunks_exact(word as usize) {
        bloom.push(if word == 8 { u64le(c, 0) } else { u64::from(u32le(c, 0)) });
    }
    at += u64::from(bloom_size) * word;
    let bk = image.bytes(at, u64::from(nbuckets) * 4)?;
    let buckets: Vec<u32> = bk.chunks_exact(4).map(|c| u32le(c, 0)).collect();
    at += u64::from(nbuckets) * 4;
    // The chain runs from the largest bucket start to the last entry of
    // its chain (bit 0 set).
    let max = buckets.iter().copied().max().unwrap_or(0);
    let mut chain = Vec::new();
    if max >= symoffset && max != 0 {
        let mut i = symoffset;
        loop {
            let c = u32le(image.bytes(at + u64::from(i - symoffset) * 4, 4)?, 0);
            chain.push(c);
            if i >= max && c & 1 == 1 {
                break;
            }
            i += 1;
            if chain.len() > 1 << 26 {
                return None;
            }
        }
    }
    let _ = (symtab, entsize);
    let size = at - a + chain.len() as u64 * 4;
    Some(GnuHash { nbuckets, symoffset, bloom_size, bloom_shift, bloom, buckets, chain, size })
}

/// The GNU hash of a name (the loader's `dl_new_hash`).
pub fn gnu_hash(name: &[u8]) -> u32 {
    let mut h: u32 = 5381;
    for &c in name {
        h = h.wrapping_mul(33).wrapping_add(u32::from(c));
    }
    h
}

/// The System V ELF hash of a name (gABI "Hash Table"), as `vna_hash`.
pub fn elf_hash(name: &[u8]) -> u32 {
    let mut h: u32 = 0;
    for &c in name {
        h = (h << 4).wrapping_add(u32::from(c));
        let g = h & 0xf000_0000;
        if g != 0 {
            h ^= g >> 24;
        }
        h &= !g;
    }
    h
}

/// glibc's `.gnu.hash` lookup of `name` in the table, over `syms`: the
/// index found, or `None` (the Bloom filter rejects it or no chain entry
/// matches). Written from glibc's documented algorithm (`do_lookup_x`,
/// `dl_new_hash`).
pub fn gnu_lookup(g: &GnuHash, syms: &[DynSym], name: &str, word_bits: u32) -> Option<usize> {
    gnu_lookup_by(g, syms, name, word_bits, |_| true)
}

/// [`gnu_lookup`] taking the first chain entry of the name that `accept`
/// takes (glibc's `check_match` on the version).
pub fn gnu_lookup_by(
    g: &GnuHash,
    syms: &[DynSym],
    name: &str,
    word_bits: u32,
    accept: impl Fn(usize) -> bool,
) -> Option<usize> {
    let h = gnu_hash(name.as_bytes());
    if g.bloom_size == 0 || g.nbuckets == 0 {
        return None;
    }
    let bw = g.bloom[((h / word_bits) % g.bloom_size) as usize];
    let m1 = 1u64 << (h % word_bits);
    // A shift of 32 or more is undefined in glibc's C; a hostile table
    // must not panic the verifier (WP-082's sweep found it): read it as 0.
    let m2 = 1u64 << (h.checked_shr(g.bloom_shift).unwrap_or(0) % word_bits);
    if bw & m1 == 0 || bw & m2 == 0 {
        return None;
    }
    let mut i = g.buckets[(h % g.nbuckets) as usize];
    if i == 0 {
        return None;
    }
    loop {
        let c = *g.chain.get(i.checked_sub(g.symoffset)? as usize)?;
        if (c | 1) == (h | 1)
            && syms.get(i as usize).is_some_and(|s| s.name == name)
            && accept(i as usize)
        {
            return Some(i as usize);
        }
        if c & 1 == 1 {
            return None;
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_the_documented_functions() {
        assert_eq!(gnu_hash(b""), 5381);
        assert_eq!(gnu_hash(b"printf"), 0x156b_2bb8);
        assert_eq!(gnu_hash(b"exit"), 0x7c96_7e3f);
        assert_eq!(elf_hash(b"GLIBC_2.17"), 0x0696_9197);
        assert_eq!(elf_hash(b"GLIBC_2.2.5"), 0x0969_1a75);
    }
}

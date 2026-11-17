//! The verifier of an `ET_REL` output (`spec/plumb.md` §14, stage R).
//! Owner: **RL-5**: V-R2 to V-R11 as `spec/plumb.md` states them
//! (P-P2 to P-P11), written from the spec and never from RL-3's or RL-4's
//! code; the D.3 corpus as fixtures (`tests/relocatable.rs`). RL-5 owns
//! this file and `relsym.rs`, `releh.rs`, `relgroup.rs`.
//!
//! The entry point is [`verify`]: the output's bytes, the inputs (a
//! [`Source`]), the map and the caller's context (a reference's line).
//! [`crate::verify_with`] and [`crate::verify_from`] dispatch to it
//! (P-P9), and RL-6's `compose` calls it for the `-r` output it composes.
//!
//! The verifier recomputes every decision of the rules from the inputs and
//! the line (§14: which section is dropped, the COMDAT keepers, the
//! record map of `.eh_frame`); the map says only where each input
//! section went (§14.2).

// f, s, o, r, t, v, w: file, section, output section, record, target,
// value, width, as §14 names them throughout.
#![allow(clippy::many_single_char_names)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::Source;
use crate::dynverify::Ctx;
use crate::map::{Format, Map, SectionRef};
use crate::report::{Finding, Reason, RelCoverage, Report, Rewrite, Rule, Site, Via};
use crate::store::{Resolved, Store};

// ---- ELF constants (gABI) ----

pub(crate) const ET_REL: u16 = 1;
pub(crate) const EM_386: u16 = 3;
pub(crate) const EM_X86_64: u16 = 62;
pub(crate) const EM_AARCH64: u16 = 183;
pub(crate) const SHT_NULL: u32 = 0;
pub(crate) const SHT_PROGBITS: u32 = 1;
pub(crate) const SHT_SYMTAB: u32 = 2;
pub(crate) const SHT_STRTAB: u32 = 3;
pub(crate) const SHT_RELA: u32 = 4;
pub(crate) const SHT_NOTE: u32 = 7;
pub(crate) const SHT_NOBITS: u32 = 8;
pub(crate) const SHT_REL: u32 = 9;
pub(crate) const SHT_GROUP: u32 = 17;
pub(crate) const SHT_SYMTAB_SHNDX: u32 = 18;
pub(crate) const SHT_LLVM_CALL_GRAPH_PROFILE: u32 = 0x6fff_4c09;
pub(crate) const SHT_LLVM_ADDRSIG: u32 = 0x6fff_4c03;
pub(crate) const SHT_X86_64_UNWIND: u32 = 0x7000_0001;
pub(crate) const SHF_WRITE: u64 = 0x1;
pub(crate) const SHF_ALLOC: u64 = 0x2;
pub(crate) const SHF_EXECINSTR: u64 = 0x4;
pub(crate) const SHF_MERGE: u64 = 0x10;
pub(crate) const SHF_STRINGS: u64 = 0x20;
pub(crate) const SHF_INFO_LINK: u64 = 0x40;
pub(crate) const SHF_LINK_ORDER: u64 = 0x80;
pub(crate) const SHF_GROUP: u64 = 0x200;
pub(crate) const SHF_TLS: u64 = 0x400;
pub(crate) const SHF_COMPRESSED: u64 = 0x800;
pub(crate) const SHF_GNU_RETAIN: u64 = 0x20_0000;
pub(crate) const SHF_EXCLUDE: u64 = 0x8000_0000;
pub(crate) const SHN_UNDEF: u32 = 0;
pub(crate) const SHN_LORESERVE: u32 = 0xff00;
pub(crate) const SHN_ABS: u32 = 0xfff1;
pub(crate) const SHN_COMMON: u32 = 0xfff2;
pub(crate) const SHN_XINDEX: u32 = 0xffff;
pub(crate) const STB_LOCAL: u8 = 0;
pub(crate) const STB_GLOBAL: u8 = 1;
pub(crate) const STB_WEAK: u8 = 2;
pub(crate) const STB_GNU_UNIQUE: u8 = 10;
pub(crate) const STT_OBJECT: u8 = 1;
pub(crate) const STT_SECTION: u8 = 3;
pub(crate) const STT_FILE: u8 = 4;
pub(crate) const STT_COMMON: u8 = 5;
pub(crate) const STT_GNU_IFUNC: u8 = 10;
pub(crate) const GRP_COMDAT: u32 = 1;

// ---- a small, total ELF reader (little-endian ELF32 and ELF64) ----

/// The file header fields stage R reads.
#[derive(Debug, Clone, Default)]
pub(crate) struct Header {
    pub class64: bool,
    pub osabi: u8,
    pub e_type: u16,
    pub machine: u16,
    pub entry: u64,
    pub phoff: u64,
    pub shoff: u64,
    pub flags: u32,
    pub ehsize: u16,
    pub phentsize: u16,
    pub phnum: u16,
    pub shentsize: u16,
    /// The header's own fields, before the `SHN_XINDEX` escapes.
    pub shnum_field: u16,
    pub shstrndx_field: u16,
    pub shnum: usize,
    pub shstrndx: u32,
}

/// One section header and its bytes.
#[derive(Debug, Clone, Default)]
pub(crate) struct Sec {
    pub name: String,
    pub name_off: u32,
    pub ty: u32,
    pub flags: u64,
    pub addr: u64,
    pub offset: u64,
    pub size: u64,
    pub link: u32,
    pub info: u32,
    pub align: u64,
    pub entsize: u64,
    /// The file bytes (empty for `SHT_NOBITS`).
    pub data: Vec<u8>,
}

impl Sec {
    pub fn is_reloc(&self) -> bool {
        matches!(self.ty, SHT_REL | SHT_RELA)
    }
    pub fn nobits(&self) -> bool {
        self.ty == SHT_NOBITS
    }
}

/// One symbol, its section index resolved through `.symtab_shndx`.
#[derive(Debug, Clone, Default)]
pub(crate) struct Sym {
    pub name: String,
    pub value: u64,
    pub size: u64,
    pub info: u8,
    pub other: u8,
    /// The raw `st_shndx` (`SHN_XINDEX` when escaped).
    pub raw_shndx: u16,
    pub shndx: u32,
}

impl Sym {
    pub fn bind(&self) -> u8 {
        self.info >> 4
    }
    pub fn ty(&self) -> u8 {
        self.info & 0xf
    }
    pub fn vis(&self) -> u8 {
        self.other & 3
    }
    pub fn local(&self) -> bool {
        self.bind() == STB_LOCAL
    }
    /// Defined in a section (not undefined, absolute or common).
    pub fn in_section(&self) -> bool {
        self.shndx != SHN_UNDEF && self.shndx != SHN_ABS && self.shndx != SHN_COMMON
    }
}

/// One relocation record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rel {
    pub offset: u64,
    pub ty: u32,
    pub sym: u32,
    /// `r_addend` on RELA; `None` on REL (the field holds it).
    pub addend: Option<i64>,
}

/// An ELF relocatable file (an input, or the output), read whole.
#[derive(Debug, Clone, Default)]
pub(crate) struct Elf {
    pub h: Header,
    pub secs: Vec<Sec>,
    pub syms: Vec<Sym>,
    /// The `.symtab` section, and its `.symtab_shndx`.
    pub symtab: Option<usize>,
    pub shndx_sec: Option<usize>,
    #[allow(dead_code)] // the file length, for messages
    pub len: u64,
}

fn rd(b: &[u8], at: u64, n: usize) -> Option<u64> {
    let at = usize::try_from(at).ok()?;
    let s = b.get(at..at.checked_add(n)?)?;
    let mut a = [0u8; 8];
    a[..n].copy_from_slice(s);
    Some(u64::from_le_bytes(a))
}

fn cstr(b: &[u8], at: u32) -> Option<String> {
    let s = b.get(at as usize..)?;
    let end = s.iter().position(|&c| c == 0)?;
    Some(String::from_utf8_lossy(&s[..end]).into_owned())
}

impl Elf {
    /// Reads `b`: `Err` names what cannot be read (O1 for the output, I2
    /// for an input).
    #[allow(clippy::too_many_lines)] // the header, the sections, the symbols
    pub(crate) fn parse(b: &[u8]) -> Result<Elf, String> {
        if b.len() < 52 || &b[..4] != b"\x7fELF" {
            return Err("not an ELF file".into());
        }
        let class64 = match b[4] {
            1 => false,
            2 => true,
            c => return Err(format!("ELF class {c}")),
        };
        if b[5] != 1 {
            return Err("not little-endian".into());
        }
        let r = |at: u64, n: usize| {
            rd(b, at, n).ok_or_else(|| "the file header is cut short".to_string())
        };
        let mut h = Header { class64, osabi: b[7], ..Header::default() };
        h.e_type = r(16, 2)? as u16;
        h.machine = r(18, 2)? as u16;
        let (w, base) = if class64 { (8, 24u64) } else { (4, 24u64) };
        h.entry = r(base, w)?;
        h.phoff = r(base + w as u64, w)?;
        h.shoff = r(base + 2 * w as u64, w)?;
        let t = base + 3 * w as u64;
        h.flags = r(t, 4)? as u32;
        h.ehsize = r(t + 4, 2)? as u16;
        h.phentsize = r(t + 6, 2)? as u16;
        h.phnum = r(t + 8, 2)? as u16;
        h.shentsize = r(t + 10, 2)? as u16;
        h.shnum_field = r(t + 12, 2)? as u16;
        h.shstrndx_field = r(t + 14, 2)? as u16;
        let ent: u64 = if class64 { 64 } else { 40 };
        let mut elf = Elf { len: b.len() as u64, ..Elf::default() };
        if h.shoff == 0 {
            h.shnum = 0;
            elf.h = h;
            return Ok(elf);
        }
        if u64::from(h.shentsize) != ent {
            return Err(format!("e_shentsize is {}, not {ent}", h.shentsize));
        }
        let shdr = |i: u64| -> Result<Sec, String> {
            let at =
                h.shoff.checked_add(i.checked_mul(ent).ok_or("overflow")?).ok_or("overflow")?;
            let g = |o: u64, n: usize| {
                rd(b, at + o, n)
                    .ok_or_else(|| format!("section header {i} lies past the file's end"))
            };
            Ok(if class64 {
                Sec {
                    name_off: g(0, 4)? as u32,
                    ty: g(4, 4)? as u32,
                    flags: g(8, 8)?,
                    addr: g(16, 8)?,
                    offset: g(24, 8)?,
                    size: g(32, 8)?,
                    link: g(40, 4)? as u32,
                    info: g(44, 4)? as u32,
                    align: g(48, 8)?,
                    entsize: g(56, 8)?,
                    ..Sec::default()
                }
            } else {
                Sec {
                    name_off: g(0, 4)? as u32,
                    ty: g(4, 4)? as u32,
                    flags: g(8, 4)?,
                    addr: g(12, 4)?,
                    offset: g(16, 4)?,
                    size: g(20, 4)?,
                    link: g(24, 4)? as u32,
                    info: g(28, 4)? as u32,
                    align: g(32, 4)?,
                    entsize: g(36, 4)?,
                    ..Sec::default()
                }
            })
        };
        let s0 = shdr(0)?;
        h.shnum = if h.shnum_field == 0 { s0.size as usize } else { h.shnum_field as usize };
        h.shstrndx = if h.shstrndx_field == SHN_XINDEX as u16 {
            s0.link
        } else {
            u32::from(h.shstrndx_field)
        };
        if (h.shnum as u64).saturating_mul(ent) > b.len() as u64 {
            return Err(format!("{} section headers do not fit the file", h.shnum));
        }
        let mut secs = Vec::with_capacity(h.shnum);
        for i in 0..h.shnum as u64 {
            let mut s = shdr(i)?;
            if s.ty != SHT_NOBITS && s.ty != SHT_NULL {
                let d = usize::try_from(s.offset)
                    .ok()
                    .zip(usize::try_from(s.size).ok())
                    .and_then(|(o, n)| b.get(o..o.checked_add(n)?))
                    .ok_or_else(|| format!("section {i} lies past the file's end"))?;
                s.data = d.to_vec();
            }
            secs.push(s);
        }
        let names = secs.get(h.shstrndx as usize).map(|s| s.data.clone()).unwrap_or_default();
        for s in &mut secs {
            s.name = cstr(&names, s.name_off).unwrap_or_default();
        }
        elf.h = h;
        elf.symtab = secs.iter().position(|s| s.ty == SHT_SYMTAB);
        if let Some(st) = elf.symtab {
            elf.shndx_sec =
                secs.iter().position(|s| s.ty == SHT_SYMTAB_SHNDX && s.link as usize == st);
            let s = &secs[st];
            let strtab = secs.get(s.link as usize).map_or(&[][..], |s| s.data.as_slice());
            let es: usize = if class64 { 24 } else { 16 };
            let shx = elf.shndx_sec.map(|i| secs[i].data.as_slice());
            for (k, e) in s.data.chunks_exact(es).enumerate() {
                let g = |o: u64, n: usize| rd(e, o, n).unwrap_or(0);
                let (name, value, size, info, other, shndx) = if class64 {
                    (g(0, 4), g(8, 8), g(16, 8), g(4, 1), g(5, 1), g(6, 2))
                } else {
                    (g(0, 4), g(4, 4), g(8, 4), g(12, 1), g(13, 1), g(14, 2))
                };
                let raw = shndx as u16;
                let full = if raw == SHN_XINDEX as u16 {
                    shx.and_then(|x| rd(x, k as u64 * 4, 4)).map_or(SHN_XINDEX, |v| v as u32)
                } else {
                    u32::from(raw)
                };
                elf.syms.push(Sym {
                    name: cstr(strtab, name as u32).unwrap_or_default(),
                    value,
                    size,
                    info: info as u8,
                    other: other as u8,
                    raw_shndx: raw,
                    shndx: full,
                });
            }
        }
        elf.secs = secs;
        Ok(elf)
    }

    /// The records of relocation section `si`.
    pub(crate) fn rels(&self, si: usize) -> Vec<Rel> {
        let Some(s) = self.secs.get(si) else { return Vec::new() };
        let c64 = self.h.class64;
        let rela = s.ty == SHT_RELA;
        let es: usize = match (c64, rela) {
            (true, true) => 24,
            (true, false) => 16,
            (false, true) => 12,
            (false, false) => 8,
        };
        s.data
            .chunks_exact(es)
            .map(|e| {
                let g = |o: u64, n: usize| rd(e, o, n).unwrap_or(0);
                let (offset, info) = if c64 { (g(0, 8), g(8, 8)) } else { (g(0, 4), g(4, 4)) };
                let (sym, ty) = if c64 {
                    ((info >> 32) as u32, info as u32)
                } else {
                    ((info >> 8) as u32, info as u32 & 0xff)
                };
                let addend = rela
                    .then(|| if c64 { g(16, 8) as i64 } else { i64::from(g(8, 4) as u32 as i32) });
                Rel { offset, ty, sym, addend }
            })
            .collect()
    }

    /// The relocation sections that apply to section `si`.
    pub(crate) fn relocs_of(&self, si: usize) -> Vec<usize> {
        (0..self.secs.len())
            .filter(|&r| self.secs[r].is_reloc() && self.secs[r].info as usize == si && si != 0)
            .collect()
    }

    /// The `.symtab`'s `sh_info`: the first non-local.
    pub(crate) fn first_global(&self) -> usize {
        self.symtab.map_or(0, |s| self.secs[s].info as usize)
    }
}

// ---- the field of a relocation type ----

/// The field a relocation type writes, in the machine's table (§4.2,
/// §4.5): its width in bytes and the bits of the place it holds. `NONE`
/// writes nothing.
pub(crate) fn field(machine: u16, ty: u32) -> (u8, u64) {
    if ty == 0 {
        return (0, 0);
    }
    let f = match machine {
        EM_AARCH64 => crate::a64::def(ty).map(|d| d.field),
        EM_X86_64 => crate::x64::def(ty).map(|d| d.field),
        EM_386 => crate::x86::def(ty).map(|d| d.field),
        _ => None,
    };
    if let Some(f) = f {
        (f.width(), f.mask())
    } else {
        let w = match machine {
            EM_AARCH64 => crate::a64::unchecked_width(ty),
            EM_X86_64 => crate::x64::unchecked_width(ty),
            _ => crate::x86::unchecked_width(ty),
        };
        (w, if w >= 8 { u64::MAX } else { (1u64 << (8 * u32::from(w))) - 1 })
    }
}

/// The type's name.
pub(crate) fn tname(machine: u16, ty: u32) -> String {
    mortar_elf::reloc::name(machine, ty)
        .map_or_else(|| format!("relocation type {ty}"), str::to_string)
}

/// The implicit addend of a REL record (§4.5's `Implicit`; A-R2): the
/// field's bits, sign-extended from its width. Only i386's data fields
/// (and `NONE`, 0) are REL.
pub(crate) fn implicit(machine: u16, ty: u32, data: &[u8], at: u64) -> Option<i64> {
    let (w, _) = field(machine, ty);
    if w == 0 {
        return Some(0);
    }
    let v = rd(data, at, w as usize)?;
    let bits = 8 * u32::from(w);
    Some(if bits >= 64 { v as i64 } else { ((v << (64 - bits)) as i64) >> (64 - bits) })
}

// ---- the line (§14.1) ----

/// The options of the line stage R reads (§14.1). `known`: there is a
/// line; without one the clauses that need an option are not judged.
#[derive(Debug, Clone, Default)]
#[allow(clippy::struct_excessive_bools)] // independent options of the line
pub(crate) struct Opts {
    pub known: bool,
    pub relocatable: bool,
    pub define_common: bool,
    pub strip_debug: bool,
    pub strip_all: bool,
    pub discard_all: bool,
    pub undefined: Vec<String>,
    pub entry: Option<String>,
    /// The last of `--build-id[=STYLE]`/`--no-build-id`: `Some(style)`.
    pub build_id: Option<String>,
    pub execstack: Option<bool>,
    /// `--unique` (an empty pattern list: every section) or the patterns.
    pub unique: Option<Vec<String>>,
    pub force_group: bool,
    pub muldefs: bool,
    /// The input words the line marks binary (`-b binary`).
    pub binary: BTreeSet<String>,
    /// The input words in line order.
    pub inputs: Vec<String>,
}

impl Opts {
    pub(crate) fn read(words: Option<&[String]>) -> Opts {
        let Some(words) = words else { return Opts::default() };
        let t = crate::opts::Table::get();
        let bytes: Vec<Vec<u8>> = words.iter().map(|w| w.as_bytes().to_vec()).collect();
        let mut o = Opts { known: true, ..Opts::default() };
        let mut binary = false;
        let mut i = 0;
        while i < bytes.len() {
            let (w, used) = t.word(&bytes, i);
            match w {
                crate::opts::Word::Input => {
                    let word = words[i].clone();
                    if binary {
                        o.binary.insert(word.clone());
                    }
                    o.inputs.push(word);
                }
                crate::opts::Word::Opt { id, value, .. } => match id {
                    "-r" | "-Ur" => o.relocatable = true,
                    "-d" => o.define_common = true,
                    "-s" => {
                        o.strip_all = true;
                        o.strip_debug = true;
                    }
                    "-S" => o.strip_debug = true,
                    "-x" => o.discard_all = true,
                    "-u" => o.undefined.extend(value),
                    "-e" => o.entry = value,
                    "--build-id" => {
                        o.build_id = match value.as_deref() {
                            Some("none") => None,
                            Some(v) => Some(v.to_string()),
                            None => Some("default".into()),
                        };
                    }
                    "--no-build-id" => o.build_id = None,
                    "--unique" => {
                        let u = o.unique.get_or_insert_with(Vec::new);
                        match value {
                            Some(p) => u.push(p),
                            None => u.clear(),
                        }
                    }
                    "--force-group-allocation" => o.force_group = true,
                    "--allow-multiple-definition" => o.muldefs = true,
                    "-b" => binary = value.as_deref() == Some("binary"),
                    _ => {}
                },
                crate::opts::Word::Z { kw, .. } => match kw {
                    "execstack" => o.execstack = Some(true),
                    "noexecstack" => o.execstack = Some(false),
                    "muldefs" => o.muldefs = true,
                    _ => {}
                },
                crate::opts::Word::Unknown => {}
            }
            i += used.max(1);
        }
        o
    }

    /// Whether `--unique` makes section `name` its own output section
    /// (L-R11): every section for a bare `--unique`, else a pattern match.
    pub(crate) fn unique_of(&self, name: &str) -> bool {
        match &self.unique {
            None => false,
            Some(p) if p.is_empty() => true,
            Some(p) => p.iter().any(|g| glob(g.as_bytes(), name.as_bytes())),
        }
    }
}

/// A shell glob (`*`, `?`, `[...]` read as literal) over bytes.
fn glob(p: &[u8], s: &[u8]) -> bool {
    match (p.first(), s.first()) {
        (None, None) => true,
        (Some(b'*'), _) => glob(&p[1..], s) || (!s.is_empty() && glob(p, &s[1..])),
        (Some(b'?'), Some(_)) => glob(&p[1..], &s[1..]),
        (Some(a), Some(b)) if a == b => glob(&p[1..], &s[1..]),
        _ => false,
    }
}

// ---- the inputs (§14.1) ----

/// One input of the `-r`, in `FileId` order.
#[derive(Debug)]
pub(crate) struct In {
    /// The map's file index.
    pub map_file: u32,
    pub path: String,
    pub member: Option<String>,
    pub elf: Elf,
    /// A `-b binary` input (L-R9).
    pub binary: bool,
    /// Which copy of a path the line names more than once (0 first).
    pub copy: usize,
    /// The position of its word in the line, when known.
    pub line_pos: Option<usize>,
}

impl In {
    pub(crate) fn label(&self) -> String {
        match &self.member {
            Some(m) => format!("{}({m})", self.path),
            None => self.path.clone(),
        }
    }
    pub(crate) fn site(&self, sec: usize, offset: u64) -> Site {
        Site {
            file: self.path.clone(),
            member: self.member.clone(),
            section: self.elf.secs.get(sec).map(|s| s.name.clone()).unwrap_or_default(),
            offset,
        }
    }
    /// `{file}:({section})`.
    pub(crate) fn sname(&self, sec: usize) -> String {
        format!("{}:({})", self.label(), self.elf.secs.get(sec).map_or("?", |s| s.name.as_str()))
    }
}

/// L-R9's binary input: one `.data`, three globals.
fn binary_input(word: &str, bytes: &[u8], out: &Header) -> Elf {
    let m: String = word.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    let n = bytes.len() as u64;
    let data = Sec {
        name: ".data".into(),
        ty: SHT_PROGBITS,
        flags: SHF_ALLOC | SHF_WRITE,
        size: n,
        align: 1,
        data: bytes.to_vec(),
        ..Sec::default()
    };
    let g = |name: String, value: u64, shndx: u32| Sym {
        name,
        value,
        info: STB_GLOBAL << 4,
        raw_shndx: shndx as u16,
        shndx,
        ..Sym::default()
    };
    Elf {
        h: Header {
            class64: out.class64,
            e_type: ET_REL,
            machine: out.machine,
            ..Header::default()
        },
        secs: vec![Sec::default(), data],
        syms: vec![
            Sym::default(),
            g(format!("_binary_{m}_start"), 0, 1),
            g(format!("_binary_{m}_end"), n, 1),
            g(format!("_binary_{m}_size"), n, SHN_ABS),
        ],
        symtab: None,
        shndx_sec: None,
        len: n,
    }
}

// ---- places (§14.1, §14.2) ----

/// Why §14.3 drops a section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Drop {
    /// A member of a COMDAT copy that is not kept (RV-R7).
    Comdat,
    /// A debug section under `-S`/`-s` (I-R9).
    Strip,
    /// mortar drops it (L-R1): `.llvm_addrsig`, the call-graph profile.
    Mortar,
}

/// Where an input section is (§14.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Place {
    Whole { out: u32, off: u64 },
    Merged { out: u32 },
    Framed,
    Consumed,
    Dropped(Drop),
    Absent,
}

/// A target (§14.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Tgt {
    /// A global, weak, common or undefined name, with the addend.
    Name(String, i64),
    /// An effective place: output section and offset.
    Place(u32, i64),
    /// `SHN_ABS` plus the addend.
    Abs(i64),
    /// Into a section merged into output section `o`: the string or entry
    /// that must start at the output's effective place.
    /// The third field is the addend outside the entry: a non-section
    /// symbol's (lld's and GNU ld's reading: the entry is at the
    /// symbol, the addend applies after; x86-64's `lea .LC0(%rip)` is
    /// `.LC0 - 4`).
    Merged(u32, Vec<u8>, i64),
    /// Into a section not in the output: the section, `{file}:({section})`.
    Dropped(String, Option<(usize, usize, u64)>),
    /// The null symbol.
    Null,
    /// A symbol or section the reader cannot resolve.
    Bad(String),
}

impl std::fmt::Display for Tgt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let add = |a: i64| match a {
            0 => String::new(),
            a if a < 0 => format!("-{:#x}", a.unsigned_abs()),
            a => format!("+{a:#x}"),
        };
        match self {
            Tgt::Name(n, a) => write!(f, "{n}{}", add(*a)),
            Tgt::Place(o, v) => {
                write!(f, "section {o}{}", if *v == 0 { "+0x0".into() } else { add(*v) })
            }
            Tgt::Abs(v) => write!(f, "ABS{}", add(*v)),
            Tgt::Merged(o, b, x) => {
                write!(f, "the bytes {:?} in section {o}{}", String::from_utf8_lossy(b), add(*x))
            }
            Tgt::Dropped(s, _) => write!(f, "{s} (dropped)"),
            Tgt::Null => write!(f, "symbol 0"),
            Tgt::Bad(s) => write!(f, "{s}"),
        }
    }
}

/// Stage R's state: the output, the inputs in `FileId` order, the places.
pub(crate) struct Rv<'m> {
    pub out: Elf,
    pub ins: Vec<In>,
    pub map: &'m Map,
    pub format: Format,
    pub opts: Opts,
    pub machine: u16,
    pub class64: bool,
    /// `[file][section]`.
    pub places: Vec<Vec<Place>>,
    /// The drop §14.3 derives for a section the map nevertheless places.
    pub derived_drop: Vec<Vec<Option<Drop>>>,
    /// The keeper of each COMDAT signature: (file, group section).
    pub keepers: BTreeMap<String, (usize, usize)>,
    /// `.eh_frame`'s record map: (file, section) to input record offset
    /// to output offset (`None`: removed), filled by P-P5.
    pub frames: BTreeMap<(usize, usize), BTreeMap<u64, Option<u64>>>,
    /// The output `.eh_frame`.
    pub out_eh: Option<u32>,
    /// Output records by (target section, offset): (relocation section,
    /// index); and whether each is matched.
    pub out_recs: BTreeMap<(u32, u64), Vec<(usize, usize)>>,
    pub out_rels: BTreeMap<usize, Vec<Rel>>,
    pub used: BTreeMap<(usize, usize), bool>,
    /// Matched output record to its input record: (file, reloc section,
    /// index) — P-P3's matching, which mortar's order clause reads.
    pub matched: BTreeMap<(usize, usize), (usize, usize, usize)>,
    /// The output group of each kept input group: (file, group section).
    pub group_out: BTreeMap<(usize, usize), u32>,
    pub report: Report,
    pub cov: RelCoverage,
    /// The output's bytes as given (B1 reads them).
    pub raw: Vec<u8>,
}

impl Rv<'_> {
    pub(crate) fn mortar(&self) -> bool {
        self.format == Format::Mortar
    }
    pub(crate) fn push(&mut self, f: Finding) {
        self.report.findings.push(f);
    }
    pub(crate) fn p(&mut self, n: u8, msg: impl Into<String>) {
        self.report.findings.push(Finding::new(Rule::Rel(n), msg));
    }
    pub(crate) fn p_at(
        &mut self,
        n: u8,
        file: usize,
        sec: usize,
        off: u64,
        msg: impl Into<String>,
    ) {
        let site = self.ins[file].site(sec, off);
        self.report.findings.push(Finding::new(Rule::Rel(n), msg).site(site));
    }
    pub(crate) fn oname(&self, o: u32) -> &str {
        self.out.secs.get(o as usize).map_or("?", |s| s.name.as_str())
    }
    /// `{index} {name}` of an output section.
    pub(crate) fn osec(&self, o: u32) -> String {
        format!("{o} {}", self.oname(o))
    }
    /// The live inputs' sections a predicate selects, in `FileId` order.
    pub(crate) fn is_eh(&self, s: &Sec) -> bool {
        s.name == ".eh_frame" || (self.machine == EM_X86_64 && s.ty == SHT_X86_64_UNWIND)
    }
    /// Whether the referring section is debug information (§5.8's names).
    pub(crate) fn rel_kind(&self) -> u32 {
        if self.machine == EM_386 { SHT_REL } else { SHT_RELA }
    }

    /// The target of record `r` of input `f`'s relocation section `rs`
    /// (§14.4), the addend read from `data` (the relocated section's
    /// bytes) on REL.
    pub(crate) fn in_target(&self, f: usize, r: &Rel, data: &[u8]) -> Tgt {
        let e = &self.ins[f].elf;
        let a = match r.addend {
            Some(a) => a,
            None => implicit(self.machine, r.ty, data, r.offset).unwrap_or(0),
        };
        if r.sym == 0 {
            return Tgt::Null;
        }
        let Some(s) = e.syms.get(r.sym as usize) else {
            return Tgt::Bad(format!("symbol {} of {}", r.sym, self.ins[f].label()));
        };
        if !s.local() || s.shndx == SHN_UNDEF || s.shndx == SHN_COMMON {
            return Tgt::Name(s.name.clone(), a);
        }
        if s.shndx == SHN_ABS {
            return Tgt::Abs((s.value as i64).wrapping_add(a));
        }
        let t = s.shndx as usize;
        if s.ty() != STT_SECTION
            && let Tgt::Merged(o, b, _) = self.at_place(f, t, s.value as i64)
        {
            return Tgt::Merged(o, b, a);
        }
        let v = (s.value as i64).wrapping_add(a);
        self.at_place(f, t, v)
    }

    /// The effective place of input `f`'s section `t` plus `v` (§14.4).
    pub(crate) fn at_place(&self, f: usize, t: usize, v: i64) -> Tgt {
        let e = &self.ins[f].elf;
        match self.places[f].get(t) {
            Some(Place::Whole { out, off }) => Tgt::Place(*out, (*off as i64).wrapping_add(v)),
            Some(Place::Merged { out }) => {
                let Some(sec) = e.secs.get(t) else { return Tgt::Bad("section".into()) };
                Tgt::Merged(*out, merged_entry(sec, v), 0)
            }
            Some(Place::Framed) => {
                let rm = self.frames.get(&(f, t));
                let hit = rm.and_then(|m| m.range(..=v.max(0) as u64).next_back());
                match (self.out_eh, hit) {
                    (Some(o), Some((&start, &Some(to)))) => {
                        Tgt::Place(o, (to as i64).wrapping_add(v - start as i64))
                    }
                    _ => Tgt::Dropped(self.ins[f].sname(t), Some((f, t, v as u64))),
                }
            }
            _ => Tgt::Dropped(self.ins[f].sname(t), Some((f, t, v.max(0) as u64))),
        }
    }

    /// The target of output record `r` of relocation section `rs`.
    pub(crate) fn out_target(&self, rs: usize, r: &Rel) -> Tgt {
        let target = self.out.secs.get(rs).map_or(0, |s| s.info as usize);
        let data = self.out.secs.get(target).map_or(&[][..], |s| s.data.as_slice());
        let a = match r.addend {
            Some(a) => a,
            None => implicit(self.machine, r.ty, data, r.offset).unwrap_or(0),
        };
        if r.sym == 0 {
            return Tgt::Null;
        }
        let Some(s) = self.out.syms.get(r.sym as usize) else {
            return Tgt::Bad(format!("output symbol {}", r.sym));
        };
        if !s.local() || s.shndx == SHN_UNDEF || s.shndx == SHN_COMMON {
            return Tgt::Name(s.name.clone(), a);
        }
        if s.shndx == SHN_ABS {
            return Tgt::Abs((s.value as i64).wrapping_add(a));
        }
        Tgt::Place(s.shndx, (s.value as i64).wrapping_add(a))
    }

    /// The output's defined global `name@@V`, for a plain `name`.
    pub(crate) fn default_version_of(&self, name: &str) -> Option<&str> {
        self.out
            .syms
            .iter()
            .find(|y| {
                !y.local()
                    && y.shndx != SHN_UNDEF
                    && y.name.strip_prefix(name).is_some_and(|v| v.starts_with("@@"))
            })
            .map(|y| y.name.as_str())
    }

    /// §14.4's equivalence; `Some(true)` through a per-linker form.
    pub(crate) fn equiv(&self, inp: &Tgt, out: &Tgt) -> Option<bool> {
        match (inp, out) {
            // mortar binds a plain `foo` to the definition `foo@@V`: GNU
            // ld's `foo` record (and `UND foo`) is not mortar's form (the
            // supervisor's ruling on RV-R6, 2026-11-17).
            (Tgt::Name(a, x), Tgt::Name(b, y))
                if a == b && x == y && self.mortar() && self.default_version_of(a).is_some() =>
            {
                None
            }
            (Tgt::Name(a, x), Tgt::Name(b, y)) if a == b && x == y => Some(false),
            // lld binds a plain `foo` to `foo@@V` (RV-R6's lld form, which
            // mortar writes: required of mortar, per-linker for lld).
            (Tgt::Name(a, x), Tgt::Name(b, y))
                if matches!(self.format, Format::Lld | Format::Mortar)
                    && x == y
                    && !a.contains('@')
                    && b.strip_prefix(a.as_str()).is_some_and(|v| v.starts_with("@@")) =>
            {
                Some(!self.mortar())
            }
            (Tgt::Place(o, v), Tgt::Place(p, w)) if o == p && v == w => Some(false),
            (Tgt::Abs(v), Tgt::Abs(w)) if v == w => Some(false),
            (Tgt::Merged(o, want, x), Tgt::Place(p, w)) if o == p => {
                let d = self.out.secs.get(*p as usize).map_or(&[][..], |s| s.data.as_slice());
                let at = usize::try_from(w.wrapping_sub(*x)).ok()?;
                d.get(at..)?.starts_with(want).then_some(false)
            }
            (Tgt::Null, Tgt::Null) => Some(false),
            _ => None,
        }
    }
}

/// The NUL-terminated string (`SHF_STRINGS`) or the `sh_entsize`-byte
/// entry of a mergeable section at `v` (§14.4).
pub(crate) fn merged_entry(sec: &Sec, v: i64) -> Vec<u8> {
    let Ok(at) = usize::try_from(v) else { return Vec::new() };
    let Some(rest) = sec.data.get(at..) else { return Vec::new() };
    let es = (sec.entsize.max(1)) as usize;
    if sec.flags & SHF_STRINGS != 0 {
        let mut end = 0;
        while end + es <= rest.len() {
            let unit = &rest[end..end + es];
            end += es;
            if unit.iter().all(|&b| b == 0) {
                break;
            }
        }
        rest[..end].to_vec()
    } else {
        rest[..es.min(rest.len())].to_vec()
    }
}

/// The debug sections `-S` and `-s` drop (§3.3's list: GNU ld's names).
pub(crate) fn is_debug(name: &str) -> bool {
    name.starts_with(".debug")
        || name.starts_with(".zdebug")
        || name.starts_with(".gnu.debuglto_")
        || name.starts_with(".stab")
        || name == ".line"
        || name == ".gdb_index"
}

/// A Mason container (MT-6).
pub(crate) fn is_container(s: &Sec) -> bool {
    s.name == ".mason.meta" && s.ty == SHT_PROGBITS && s.flags == SHF_EXCLUDE
}

/// §14.1's consumed sections.
fn consumed(s: &Sec) -> bool {
    matches!(
        s.ty,
        SHT_NULL | SHT_GROUP | SHT_REL | SHT_RELA | SHT_SYMTAB | SHT_SYMTAB_SHNDX | SHT_STRTAB
    ) || matches!(s.name.as_str(), ".note.gnu.property" | ".note.GNU-stack" | ".note.gnu.build-id")
        || is_container(s)
}

// ---- the entry point (P-P9) ----

/// Whether stage R judges this output: `ET_REL`, or the map or the line
/// says `-r` (P-P9).
pub(crate) fn applies(output: &[u8], map: &Map, line: Option<&[String]>) -> bool {
    let et_rel =
        output.len() >= 18 && &output[..4] == b"\x7fELF" && output[16] == 1 && output[17] == 0;
    et_rel || map.rel.is_some() || (map.format != Format::Mortar && Opts::read(line).relocatable)
}

/// Verifies a `-r` output (§14): the output's bytes, the inputs the map
/// names (through `src`), the map, and the caller's context (a
/// reference's line in `ctx.line`; mortar's map has its own `arg`
/// records). Never panics; what it cannot read is a finding (O1, I1, I2).
/// The input set (§3.8) is not judged here: [`crate::verify_with`] and
/// [`crate::verify_from`] add it, as for every output kind.
pub fn verify(output: &[u8], src: &dyn Source, map: &Map, ctx: &Ctx<'_>) -> Report {
    let store = Store::read(src, &[map]);
    let mut report = verify_in(output, &store, 0, map, ctx.line.as_deref());
    crate::read_errors(&store, &mut report);
    report
}

/// [`verify`] over a store already read.
pub(crate) fn verify_in(
    output: &[u8],
    store: &Store<'_>,
    m: usize,
    map: &Map,
    line: Option<&[String]>,
) -> Report {
    with_rv(output, store, m, map, line, |_| ()).0
}

/// A foreign `-r` link's map written in mortar's form (§3.2, §14.2), from
/// the places stage R derives: `rplace` for every whole piece, `rframe`
/// for every input `.eh_frame` record, `rgroup` for every kept group and
/// `discard ... comdat` for every member of a copy not kept. It lets a
/// test read a reference's link as mortar's (§9.2 (t), as (l) reads
/// lld's); `None` when the output does not read. Only a link without
/// merged pieces has a mortar form: a merged piece gets no record.
#[doc(hidden)]
pub fn as_mortar_map(output: &[u8], src: &dyn Source, map: &Map, ctx: &Ctx<'_>) -> Option<String> {
    use crate::map::mortar::{HEADER, encode};
    use std::fmt::Write as _;
    let store = Store::read(src, &[map]);
    with_rv(output, &store, 0, map, ctx.line.as_deref(), |rv| {
        let mut t = format!("{HEADER}\n");
        for (i, f) in rv.ins.iter().enumerate() {
            let _ = write!(t, "file {i} ");
            encode(&f.path, &mut t);
            if let Some(mem) = &f.member {
                t.push(' ');
                encode(mem, &mut t);
                let off = rv.map.files[f.map_file as usize].member_offset.unwrap_or(0);
                let _ = write!(t, " {off:x}");
            }
            t.push('\n');
        }
        for (f, ps) in rv.places.iter().enumerate() {
            for (s, p) in ps.iter().enumerate() {
                match p {
                    Place::Whole { out, off } => {
                        let size = rv.ins[f].elf.secs[s].size;
                        let _ = writeln!(t, "rplace {f} {s} {out} {off:x} {size:x}");
                    }
                    Place::Dropped(Drop::Comdat) => {
                        let _ = writeln!(t, "discard {f} {s} comdat");
                    }
                    _ => {}
                }
            }
        }
        for (&(f, s), recs) in &rv.frames {
            for (&at, to) in recs {
                match to {
                    Some(o) => {
                        let _ = writeln!(t, "rframe {f} {s} {at:x} {o:x}");
                    }
                    None => {
                        let _ = writeln!(t, "rframe {f} {s} {at:x} -");
                    }
                }
            }
        }
        for (&(f, g), o) in &rv.group_out {
            let _ = writeln!(t, "rgroup {o} {f} {g}");
        }
        t.push_str("output relocatable\n");
        for w in ctx.line.as_deref().unwrap_or(&[]) {
            t.push_str("arg ");
            encode(w, &mut t);
            t.push('\n');
        }
        t.push_str("end\n");
        t
    })
    .1
}

/// The model of a `-r` link after stage R's rules ran, for P-P1
/// (`compose.rs`, RL-6): `k` reads the places, the record matching and
/// the `.eh_frame` record map; the report is stage R's own.
pub(crate) fn model<R>(
    output: &[u8],
    store: &Store<'_>,
    m: usize,
    map: &Map,
    line: Option<&[String]>,
    k: impl FnOnce(&Rv<'_>) -> R,
) -> (Report, Option<R>) {
    with_rv(output, store, m, map, line, |rv| k(rv))
}

/// Builds the model of a `-r` link, runs the rules, then `k` on it; the
/// report, and `k`'s result when the output reads.
fn with_rv<R>(
    output: &[u8],
    store: &Store<'_>,
    m: usize,
    map: &Map,
    line: Option<&[String]>,
    k: impl FnOnce(&mut Rv<'_>) -> R,
) -> (Report, Option<R>) {
    let src = store.src();
    let words: Option<Vec<String>> = if map.format == Format::Mortar {
        (!map.args.is_empty())
            .then(|| map.args.iter().map(|w| String::from_utf8_lossy(w).into_owned()).collect())
    } else {
        line.map(<[String]>::to_vec)
    };
    let opts = Opts::read(words.as_deref());
    let mut report = Report::default();
    // P-P9 and P-P7's header fields are judged on the header as written;
    // an output whose e_type is not ET_REL is then read as one.
    let mut bytes = output.to_vec();
    let not_rel = bytes.len() >= 18 && &bytes[..4] == b"\x7fELF" && bytes[16..18] != [1, 0];
    if not_rel {
        let kind = u16::from_le_bytes([bytes[16], bytes[17]]);
        let said = if map.format == Format::Mortar { "output relocatable" } else { "-r" };
        report.findings.push(Finding::new(
            Rule::Rel(9),
            format!("the output is e_type {kind}, not ET_REL; the map or the line says {said}"),
        ));
        bytes[16] = 1;
        bytes[17] = 0;
    } else if map.format == Format::Mortar && map.rel.is_none() {
        let kind = map
            .dynamic
            .as_ref()
            .and_then(|d| d.kind)
            .map_or("static (no 'output' record)", |k| k.name());
        report.findings.push(Finding::new(
            Rule::Rel(9),
            format!("the output is ET_REL and the map says {kind}"),
        ));
    } else if map.format != Format::Mortar && opts.known && !opts.relocatable {
        report.findings.push(Finding::new(
            Rule::Rel(9),
            "the output is ET_REL and the line has no -r, -i, --relocatable or -Ur",
        ));
    }
    let out = match Elf::parse(&bytes) {
        Ok(e) => e,
        Err(e) => {
            report.findings.push(Finding::new(Rule::OutputUnreadable, e));
            report.coverage.relocatable = Some(RelCoverage::default());
            return (report, None);
        }
    };
    if !matches!(out.h.machine, EM_AARCH64 | EM_X86_64 | EM_386) {
        report.findings.push(Finding::new(
            Rule::Rel(9),
            format!("e_machine {} is not a supported machine", out.h.machine),
        ));
    }
    let ins = read_inputs(src, store, m, map, &opts, &out.h, &mut report);
    let machine = out.h.machine;
    let class64 = out.h.class64;
    let mut rv = Rv {
        out,
        ins,
        map,
        format: map.format,
        opts,
        machine,
        class64,
        places: Vec::new(),
        derived_drop: Vec::new(),
        keepers: BTreeMap::new(),
        frames: BTreeMap::new(),
        out_eh: None,
        out_recs: BTreeMap::new(),
        out_rels: BTreeMap::new(),
        used: BTreeMap::new(),
        matched: BTreeMap::new(),
        group_out: BTreeMap::new(),
        report,
        cov: RelCoverage::default(),
        raw: output.to_vec(),
    };
    run(&mut rv);
    let r = k(&mut rv);
    let mut report = std::mem::take(&mut rv.report);
    report.coverage.relocatable = Some(rv.cov.clone());
    report.sort();
    (report, Some(r))
}

/// Reads the inputs the map names, in `FileId` order (§14.1).
#[allow(clippy::too_many_lines)] // one arm per kind of input
fn read_inputs(
    src: &dyn Source,
    store: &Store<'_>,
    m: usize,
    map: &Map,
    opts: &Opts,
    out: &Header,
    report: &mut Report,
) -> Vec<In> {
    let mut ins = Vec::new();
    for (i, f) in map.files.iter().enumerate() {
        let read = match store.resolved(m, i) {
            Resolved::Missing => {
                report.findings.push(Finding::new(
                    Rule::InputMissing,
                    format!("{f}: the map names it; it was not given"),
                ));
                continue;
            }
            Resolved::Error(e) => Err(e.clone()),
            Resolved::Object(k) => {
                let (at, _) = store.object(*k);
                let mut buf = vec![0u8; usize::try_from(at.len).unwrap_or(0)];
                src.read_at(at.file, at.base, &mut buf).map(|()| buf)
            }
        };
        let bytes = match read {
            Ok(b) => b,
            Err(e) => {
                // A binary input is not ELF: the store's skeleton refuses
                // it; it is read whole here.
                if f.member.is_none() && opts.binary.contains(&f.path) {
                    match (0..src.count()).find(|&k| src.path(k) == f.path) {
                        Some(k) => {
                            let mut buf = vec![0u8; usize::try_from(src.len(k)).unwrap_or(0)];
                            if src.read_at(k, 0, &mut buf).is_ok() {
                                buf
                            } else {
                                report
                                    .findings
                                    .push(Finding::new(Rule::InputUnreadable, format!("{f}: {e}")));
                                continue;
                            }
                        }
                        None => continue,
                    }
                } else {
                    report.findings.push(Finding::new(Rule::InputUnreadable, format!("{f}: {e}")));
                    continue;
                }
            }
        };
        let binary = f.member.is_none() && opts.binary.contains(&f.path);
        let elf = if binary {
            binary_input(&f.path, &bytes, out)
        } else {
            match Elf::parse(&bytes) {
                Ok(e)
                    if e.h.e_type == ET_REL
                        && e.h.machine == out.machine
                        && e.h.class64 == out.class64 =>
                {
                    e
                }
                // P-P7: the output's e_machine is the inputs'. An input of
                // the class but another machine names the output's header,
                // once; the input is then not judged.
                Ok(e) if e.h.e_type == ET_REL && e.h.class64 == out.class64 => {
                    let msg = format!("e_machine is {}, not {}", out.machine, e.h.machine);
                    if !report.findings.iter().any(|x| x.rule == Rule::Rel(7) && x.message == msg) {
                        report.findings.push(Finding::new(Rule::Rel(7), msg));
                    }
                    continue;
                }
                Ok(e) => {
                    report.findings.push(Finding::new(
                        Rule::InputUnreadable,
                        format!(
                            "{f}: not an ET_REL object of the output's machine (e_type {}, e_machine {})",
                            e.h.e_type, e.h.machine
                        ),
                    ));
                    continue;
                }
                Err(e) => {
                    report.findings.push(Finding::new(Rule::InputUnreadable, format!("{f}: {e}")));
                    continue;
                }
            }
        };
        // A path the line names n times is n inputs (a foreign map names
        // it once; its rows come copy after copy).
        let positions: Vec<usize> = if map.format != Format::Mortar && f.member.is_none() {
            opts.inputs.iter().enumerate().filter(|(_, w)| **w == f.path).map(|(k, _)| k).collect()
        } else {
            Vec::new()
        };
        let n = positions.len().max(1);
        for copy in 0..n {
            ins.push(In {
                map_file: i as u32,
                path: f.path.clone(),
                member: f.member.clone(),
                elf: elf.clone(),
                binary,
                copy,
                line_pos: positions.get(copy).copied(),
            });
        }
    }
    // FileId order (§14.1): mortar's map order; a foreign map's files by
    // the line's order of their words (GNU ld's `LOAD` lines without a
    // line): GNU ld names a file first in its discarded list.
    if map.format != Format::Mortar {
        let pos = |f: &In| -> usize {
            f.line_pos
                .or_else(|| opts.inputs.iter().position(|w| *w == f.path))
                .or_else(|| map.loads.iter().position(|w| *w == f.path))
                .unwrap_or(usize::MAX)
        };
        ins.sort_by_key(|f| {
            let mf = &map.files[f.map_file as usize];
            (pos(f), mf.member_offset.unwrap_or(0), f.map_file, f.copy)
        });
    }
    ins
}

/// The rules, in order: the places, the drops, then P-P5 (which gives
/// P-P3 the record map), P-P2, P-P3, P-P4, P-P6, P-P7, P-P8, P-P10,
/// P-P11.
fn run(rv: &mut Rv<'_>) {
    rv.out_eh = {
        let ehs: Vec<u32> = (1..rv.out.secs.len())
            .filter(|&i| rv.is_eh(&rv.out.secs[i]))
            .map(|i| i as u32)
            .collect();
        if ehs.len() > 1 {
            rv.push(Finding::new(
                Rule::Map(1),
                format!("the output has {} .eh_frame sections", ehs.len()),
            ));
        }
        ehs.first().copied()
    };
    crate::relgroup::keepers(rv);
    places(rv);
    index_out_records(rv);
    crate::releh::check(rv);
    content(rv);
    records(rv);
    crate::relgroup::check(rv);
    crate::relsym::check(rv);
    header(rv);
    container(rv);
    notes(rv);
    sections(rv);
}

// ---- the places (§14.2) ----

#[allow(clippy::too_many_lines)] // the derived drops, then each map form
fn places(rv: &mut Rv<'_>) {
    let n = rv.ins.len();
    rv.places = (0..n).map(|f| vec![Place::Absent; rv.ins[f].elf.secs.len()]).collect();
    rv.derived_drop = (0..n).map(|f| vec![None; rv.ins[f].elf.secs.len()]).collect();
    // §14.3, derived.
    for f in 0..n {
        let comdat_dead = crate::relgroup::dead_members(rv, f);
        for s in 0..rv.ins[f].elf.secs.len() {
            let sec = &rv.ins[f].elf.secs[s];
            let p = if consumed(sec) {
                Some(Place::Consumed)
            } else if comdat_dead.contains(&s) {
                Some(Place::Dropped(Drop::Comdat))
            } else if rv.opts.strip_debug && sec.flags & SHF_ALLOC == 0 && is_debug(&sec.name) {
                Some(Place::Dropped(Drop::Strip))
            } else if rv.mortar()
                && matches!(sec.ty, SHT_LLVM_ADDRSIG | SHT_LLVM_CALL_GRAPH_PROFILE)
            {
                Some(Place::Dropped(Drop::Mortar))
            } else if rv.is_eh(sec) {
                Some(Place::Framed)
            } else {
                None
            };
            if let Some(p) = p {
                if let Place::Dropped(d) = &p {
                    rv.derived_drop[f][s] = Some(d.clone());
                }
                rv.places[f][s] = p;
            }
        }
    }
    // The map's file index to FileId.
    let mut fid: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (i, f) in rv.ins.iter().enumerate() {
        fid.entry(f.map_file).or_default().push(i);
    }
    let nout = rv.out.secs.len() as u32;
    let mut seen: BTreeMap<(usize, usize), u32> = BTreeMap::new();
    let mut place = |rv: &mut Rv<'_>, f: usize, s: usize, p: Place, line: u32| {
        if let Some(prev) = seen.insert((f, s), line) {
            let msg =
                format!("{} has a second place (map lines {prev} and {line})", rv.ins[f].sname(s));
            let site = rv.ins[f].site(s, 0);
            rv.push(Finding::new(Rule::Map(2), msg).site(site));
            return;
        }
        match rv.places[f][s].clone() {
            Place::Consumed | Place::Framed => {}
            Place::Dropped(d) => {
                // The map places a section §14.3 drops: P-P2 says so.
                let why = match d {
                    Drop::Comdat => "a member of a COMDAT copy that is not kept (RV-R7)",
                    Drop::Strip => "a debug section under -S or -s (I-R9)",
                    Drop::Mortar => "mortar drops it (L-R1)",
                };
                let msg = format!("{} is in the output; §14.3 drops it: {why}", rv.ins[f].sname(s));
                rv.p_at(2, f, s, 0, msg);
                rv.places[f][s] = p;
            }
            _ => rv.places[f][s] = p,
        }
    };
    if rv.mortar() {
        let rel = rv.map.rel.clone().unwrap_or_default();
        for r in &rel.places {
            let Some(&f) = fid.get(&r.file).and_then(|v| v.first()) else { continue };
            let Some(s) = section_index(&rv.ins[f].elf, &r.section) else {
                rv.push(Finding::new(
                    Rule::Map(1),
                    format!(
                        "map line {}: rplace names no section of {}",
                        r.line,
                        rv.ins[f].label()
                    ),
                ));
                continue;
            };
            if r.out == 0 || r.out >= nout {
                rv.push(Finding::new(
                    Rule::Map(1),
                    format!("map line {}: rplace names output section {}, which is not a section header", r.line, r.out),
                ));
                continue;
            }
            let size = rv.ins[f].elf.secs[s].size;
            if r.size != size {
                rv.push(
                    Finding::new(Rule::Map(3), format!("map line {}: rplace gives {} the size {:#x}, its input size is {size:#x}", r.line, rv.ins[f].sname(s), r.size))
                        .site(rv.ins[f].site(s, 0)),
                );
            }
            place(rv, f, s, Place::Whole { out: r.out, off: r.offset }, r.line);
        }
        if !rel.frames.is_empty() && rv.out_eh.is_none() {
            rv.push(Finding::new(
                Rule::Map(1),
                "the map has rframe records and the output no .eh_frame",
            ));
        }
        rv.cov.pieces = rel.places.len() as u64;
    } else {
        foreign_places(rv, &fid, &mut place);
    }
    // Merged (a foreign map's): SHF_MERGE inputs without a row, into the
    // section of the block whose `<internal>` row names them (lld), or
    // into the output section of their name (mold, lld's others).
    if !rv.mortar() {
        let internal: BTreeMap<String, u32> = internal_rows(rv);
        for f in 0..n {
            for s in 0..rv.ins[f].elf.secs.len() {
                let sec = &rv.ins[f].elf.secs[s];
                if rv.places[f][s] != Place::Absent || sec.flags & SHF_MERGE == 0 {
                    continue;
                }
                // The section of its name and merge class, else of its name.
                let class = |x: &Sec| (x.flags & (SHF_MERGE | SHF_STRINGS), x.entsize);
                let o = internal.get(&sec.name).copied().or_else(|| {
                    let named: Vec<usize> = (1..rv.out.secs.len())
                        .filter(|&o| rv.out.secs[o].name == sec.name)
                        .collect();
                    named
                        .iter()
                        .copied()
                        .find(|&o| class(&rv.out.secs[o]) == class(sec))
                        .or_else(|| named.first().copied())
                        .map(|o| o as u32)
                });
                if let Some(o) = o {
                    rv.places[f][s] = Place::Merged { out: o };
                    rv.cov.merged += 1;
                }
            }
        }
    }
    // P-P2's first clause, and M10 at -r (mortar's map).
    for f in 0..n {
        for s in 1..rv.ins[f].elf.secs.len() {
            let sec = &rv.ins[f].elf.secs[s];
            match rv.places[f][s] {
                Place::Absent if sec.size > 0 => {
                    let msg = format!("{} is not in the output", rv.ins[f].sname(s));
                    rv.p_at(2, f, s, 0, msg);
                    if rv.mortar() {
                        let msg = format!(
                            "{} has bytes, no place and no reason of §14.3 to be dropped",
                            rv.ins[f].sname(s)
                        );
                        let site = rv.ins[f].site(s, 0);
                        rv.push(Finding::new(Rule::Map(10), msg).site(site));
                    }
                }
                Place::Framed => rv.cov.framed += 1,
                _ => {}
            }
        }
    }
    if !rv.mortar() {
        rv.cov.pieces =
            rv.places.iter().flatten().filter(|p| matches!(p, Place::Whole { .. })).count() as u64;
    }
    // M4 and M5 at -r: inside an output section of its name, aligned, no
    // two pieces of one output section overlapping.
    let mut by_out: BTreeMap<u32, Vec<(u64, u64, usize, usize)>> = BTreeMap::new();
    for f in 0..n {
        for s in 0..rv.ins[f].elf.secs.len() {
            if let Place::Whole { out, off } = rv.places[f][s] {
                let sec = &rv.ins[f].elf.secs[s];
                let size = sec.size;
                let Some(o) = rv.out.secs.get(out as usize) else { continue };
                let mut bad = Vec::new();
                if o.name != sec.name {
                    bad.push(format!("is in output section {}, of another name", rv.osec(out)));
                }
                if sec.align > 1 && off % sec.align != 0 {
                    bad.push(format!("is at offset {off:#x} of section {}, not a multiple of its alignment {:#x}", rv.osec(out), sec.align));
                }
                if off.checked_add(size).is_none_or(|e| e > o.size) {
                    bad.push(format!(
                        "at [{off:#x}, {:#x}) does not lie in section {} (size {:#x})",
                        off.saturating_add(size),
                        rv.osec(out),
                        o.size
                    ));
                }
                for b in bad {
                    let msg = format!("{} {b}", rv.ins[f].sname(s));
                    rv.p_at(2, f, s, 0, msg);
                }
                if size > 0 {
                    by_out.entry(out).or_default().push((off, size, f, s));
                }
            }
        }
    }
    for (o, mut v) in by_out {
        v.sort_unstable();
        for w in v.windows(2) {
            let ((a, az, fa, sa), (b, _, fb, sb)) = (w[0], w[1]);
            if a + az > b {
                let msg = format!(
                    "two pieces of output section {} overlap: {} at {a:#x} and {} at {b:#x}",
                    rv.osec(o),
                    rv.ins[fa].sname(sa),
                    rv.ins[fb].sname(sb)
                );
                let site = rv.ins[fb].site(sb, 0);
                rv.push(Finding::new(Rule::Map(5), msg).site(site));
            }
        }
    }
}

/// A map's section reference in an input.
fn section_index(e: &Elf, r: &SectionRef) -> Option<usize> {
    match r {
        SectionRef::Index(i) => (*i != 0 && (*i as usize) < e.secs.len()).then_some(*i as usize),
        SectionRef::Name(n) => e.secs.iter().position(|s| &s.name == n),
    }
}

/// lld's `<internal>` rows: the name of a merged input section to the
/// output section of its block.
fn internal_rows(rv: &Rv<'_>) -> BTreeMap<String, u32> {
    let mut m = BTreeMap::new();
    let headers = block_headers(rv);
    for (b, blk) in rv.map.blocks.iter().enumerate() {
        let end = rv.map.blocks.get(b + 1).map_or(rv.map.chunks.len(), |n| n.first_chunk);
        let Some(Some(o)) = headers.get(b) else { continue };
        for c in rv.map.chunks.get(blk.first_chunk..end).unwrap_or(&[]) {
            m.entry(c.name.clone()).or_insert(*o);
        }
    }
    m
}

/// The output section header of each block (§14.2: the k-th block of a
/// name is the k-th header of that name); `None` for a GNU ld block
/// without an address and a size.
fn block_headers(rv: &Rv<'_>) -> Vec<Option<u32>> {
    let mut by_name: BTreeMap<&str, Vec<u32>> = BTreeMap::new();
    for (i, s) in rv.out.secs.iter().enumerate().skip(1) {
        by_name.entry(s.name.as_str()).or_default().push(i as u32);
    }
    let mut k: BTreeMap<&str, usize> = BTreeMap::new();
    rv.map
        .blocks
        .iter()
        .map(|b| {
            if b.addr.is_none() || b.size.is_none() {
                return None;
            }
            let i = k.entry(b.name.as_str()).or_default();
            let h = by_name.get(b.name.as_str()).and_then(|v| v.get(*i)).copied();
            *i += 1;
            h
        })
        .collect()
}

type PlaceFn<'p> = dyn FnMut(&mut Rv<'_>, usize, usize, Place, u32) + 'p;

/// A foreign map's rows as places (§14.2).
#[allow(clippy::too_many_lines)] // §14.2's row matching, clause by clause
fn foreign_places(rv: &mut Rv<'_>, fid: &BTreeMap<u32, Vec<usize>>, place: &mut PlaceFn<'_>) {
    let headers = block_headers(rv);
    // M1: more blocks of a name than headers.
    let mut count: BTreeMap<&str, usize> = BTreeMap::new();
    for (b, blk) in rv.map.blocks.iter().enumerate() {
        if blk.addr.is_some() && blk.size.is_some() {
            *count.entry(blk.name.as_str()).or_default() += 1;
            if headers[b].is_none() {
                let have = rv.out.secs.iter().filter(|s| s.name == blk.name).count();
                let n = count[blk.name.as_str()];
                rv.push(Finding::new(
                    Rule::Map(1),
                    format!(
                        "map line {}: block {n} of {} has no output section header ({have} of that name)",
                        blk.line, blk.name
                    ),
                ));
            }
        }
    }
    // Each row's block: the last block that starts at or before it.
    // A row is the first section of its name, over the copies in order,
    // not yet named, of the row's size; else the first not yet named
    // (§14.2). GNU ld lists the rows in output order: a script-placed
    // section (`*(.debug_info)`, a CU) before the orphans of its name
    // (type units) that precede it in the input.
    let mut used: BTreeMap<(usize, String), BTreeSet<usize>> = BTreeMap::new();
    let mut b = 0usize;
    // A GNU ld block without an address carries its rows to no section;
    // rows under it fall to the last block with one.
    let mut cur: Option<(u32, u64)> = None;
    // mold's map gives every row of a non-allocated section the address
    // 0: its rows are laid end to end in map order, each aligned (§14.2).
    let mut cursor = 0u64;
    for (pi, p) in rv.map.placed.iter().enumerate() {
        while b < rv.map.blocks.len() && rv.map.blocks[b].first_placed <= pi {
            let blk = &rv.map.blocks[b];
            cur = match (headers[b], blk.addr) {
                (Some(o), Some(a)) => Some((o, a)),
                _ => None,
            };
            cursor = 0;
            b += 1;
        }
        let Some(copies) = fid.get(&p.file) else { continue };
        let SectionRef::Name(name) = &p.section else { continue };
        let f0 = copies[0];
        let e = &rv.ins[f0].elf;
        let cands: Vec<usize> = (1..e.secs.len()).filter(|&s| &e.secs[s].name == name).collect();
        if cands.is_empty() || cands.iter().all(|&s| consumed(&e.secs[s])) {
            continue;
        }
        let taken = used.entry((p.file as usize, name.clone())).or_default();
        let total = cands.len() * copies.len();
        let at = |k: usize| (copies[k / cands.len()], cands[k % cands.len()]);
        let free: Vec<usize> = (0..total).filter(|k| !taken.contains(k)).collect();
        // The row's offset in its output section for a candidate.
        let offset_of = |rv: &Rv<'_>, f: usize, s: usize| -> Option<u64> {
            let (o, base) = cur?;
            let unaddressed = rv.format == Format::Mold
                && rv.out.secs.get(o as usize).is_some_and(|x| x.flags & SHF_ALLOC == 0)
                && p.addr == 0;
            if unaddressed {
                Some(cursor.next_multiple_of(rv.ins[f].elf.secs[s].align.max(1)))
            } else {
                p.addr.checked_sub(base)
            }
        };
        let sized: Vec<usize> = free
            .iter()
            .copied()
            .filter(|&k| rv.ins[at(k).0].elf.secs[at(k).1].size == p.size)
            .collect();
        // Same-sized candidates (type units): the one whose bytes are the
        // output's there, else the first.
        let same_bytes = |k: usize| {
            let (f, s) = at(k);
            let sec = &rv.ins[f].elf.secs[s];
            let Some((o, _)) = cur else { return false };
            let Some(off) = offset_of(rv, f, s) else { return false };
            let d = rv.out.secs.get(o as usize).map_or(&[][..], |x| x.data.as_slice());
            let Some(got) = usize::try_from(off).ok().and_then(|a| d.get(a..a + sec.data.len()))
            else {
                return false;
            };
            // The bytes outside the input's relocated fields (REL fields
            // hold the output's addends).
            let mut mask = vec![true; sec.data.len()];
            for rs in rv.ins[f].elf.relocs_of(s) {
                for r in rv.ins[f].elf.rels(rs) {
                    let w = usize::from(field(rv.machine, r.ty).0);
                    let a = usize::try_from(r.offset).unwrap_or(usize::MAX);
                    for m in mask.iter_mut().skip(a).take(w) {
                        *m = false;
                    }
                }
            }
            got.iter().zip(&sec.data).zip(&mask).all(|((x, y), m)| !m || x == y)
        };
        let pick =
            if sized.len() > 1 { sized.iter().copied().find(|&k| same_bytes(k)) } else { None }
                .or_else(|| sized.first().copied())
                .or_else(|| free.first().copied());
        let Some(k) = pick else {
            let msg = format!(
                "map line {}: {} names a section the input has fewer of",
                p.line,
                rv.ins[f0].sname(cands[0])
            );
            rv.push(Finding::new(Rule::Map(2), msg));
            continue;
        };
        taken.insert(k);
        let (f, s) = at(k);
        let e = &rv.ins[f].elf;
        if rv.is_eh(&e.secs[s]) {
            // Framed: its records are judged one by one (P-P5).
            continue;
        }
        let Some((o, _)) = cur else {
            let msg =
                format!("map line {}: {} is under no output section", p.line, rv.ins[f].sname(s));
            rv.push(Finding::new(Rule::Map(1), msg));
            continue;
        };
        let Some(off) = offset_of(rv, f, s) else {
            let msg = format!(
                "map line {}: {} is before its output section's start",
                p.line,
                rv.ins[f].sname(s)
            );
            rv.push(Finding::new(Rule::Map(4), msg));
            continue;
        };
        let unaddressed = rv.format == Format::Mold
            && rv.out.secs.get(o as usize).is_some_and(|x| x.flags & SHF_ALLOC == 0)
            && p.addr == 0;
        if unaddressed {
            cursor = off + rv.ins[f].elf.secs[s].size;
        }
        place(rv, f, s, Place::Whole { out: o, off }, p.line);
    }
}

// ---- the output records ----

fn index_out_records(rv: &mut Rv<'_>) {
    for rs in 1..rv.out.secs.len() {
        if !rv.out.secs[rs].is_reloc() {
            continue;
        }
        let t = rv.out.secs[rs].info;
        let recs = rv.out.rels(rs);
        for (i, r) in recs.iter().enumerate() {
            rv.out_recs.entry((t, r.offset)).or_default().push((rs, i));
            rv.used.insert((rs, i), false);
        }
        rv.out_rels.insert(rs, recs);
    }
}

// ---- P-P2's bytes ----

/// §5.8's tombstone of a field of `width` bytes in section `name`, by
/// the writing linker's column.
pub(crate) fn tombstone(format: Format, name: &str, class64: bool, width: u8) -> u64 {
    let ones = if width >= 8 { u64::MAX } else { (1u64 << (8 * u32::from(width))) - 1 };
    match format {
        Format::Mortar => match name {
            ".debug_ranges" | ".debug_loc" => 1,
            ".debug_rnglists" | ".debug_loclists" | ".debug_addr" | ".gcc_except_table" => 0,
            ".debug_aranges" | ".debug_frame" => ones,
            n if is_debug(n) && class64 => ones,
            _ => 0,
        },
        Format::Lld | Format::Mold => u64::from(name == ".debug_ranges" || name == ".debug_loc"),
        Format::Gnu => u64::from(name == ".debug_ranges"),
    }
}

/// Whether §14.3 drops an input record for its target: a debug or
/// `.gcc_except_table` record into a dropped section (I-R7).
pub(crate) fn drops_record(name: &str, flags: u64, t: &Tgt) -> bool {
    matches!(t, Tgt::Dropped(..))
        && (flags & SHF_ALLOC == 0 && is_debug(name) || name == ".gcc_except_table")
}

/// The redirect GNU ld writes for a record into a dropped copy (§5.8):
/// the target's section of the same name and size in the keeper.
pub(crate) fn gnu_redirect(rv: &Rv<'_>, t: &Tgt) -> Option<Tgt> {
    let Tgt::Dropped(_, Some((f, s, v))) = t else { return None };
    let sec = rv.ins[*f].elf.secs.get(*s)?;
    if sec.flags & SHF_GROUP == 0 {
        return None;
    }
    for g in 0..rv.ins.len() {
        for k in 0..rv.ins[g].elf.secs.len() {
            let o = &rv.ins[g].elf.secs[k];
            if o.name == sec.name
                && o.size == sec.size
                && o.flags & SHF_GROUP != 0
                && let Place::Whole { out, off } = rv.places[g][k]
            {
                return Some(Tgt::Place(out, (off + v) as i64));
            }
        }
    }
    None
}

/// A relocated field of an input piece: (offset, width, mask, the values
/// the field may hold, relocation type).
type Field = (u64, u8, u64, Option<Vec<u64>>, u32);

/// P-P2: the bytes of every section placed whole.
#[allow(clippy::too_many_lines)] // the fields, then the bytes
fn content(rv: &mut Rv<'_>) {
    for f in 0..rv.ins.len() {
        for s in 1..rv.ins[f].elf.secs.len() {
            let Place::Whole { out, off } = rv.places[f][s] else { continue };
            let sec = &rv.ins[f].elf.secs[s];
            let Some(o) = rv.out.secs.get(out as usize) else { continue };
            if sec.nobits() && (o.nobits() || sec.size == 0) {
                continue;
            }
            let size = sec.size as usize;
            let input: Vec<u8> = if sec.nobits() { vec![0; size] } else { sec.data.clone() };
            let Some(got) =
                usize::try_from(off).ok().and_then(|d| o.data.get(d..d.checked_add(size)?))
            else {
                if o.nobits() && !sec.nobits() && sec.data.iter().any(|&b| b != 0) {
                    let msg = format!(
                        "{} has bytes and is in the NOBITS section {}",
                        rv.ins[f].sname(s),
                        rv.osec(out)
                    );
                    rv.p_at(2, f, s, 0, msg);
                }
                continue;
            };
            let got = got.to_vec();
            // The fields: (offset, width, mask, expected-in-field, type).
            let mut fields: Vec<Field> = Vec::new();
            for rs in rv.ins[f].elf.relocs_of(s) {
                let recs = rv.ins[f].elf.rels(rs);
                for r in recs {
                    let (w, mask) = field(rv.machine, r.ty);
                    if w == 0 {
                        continue;
                    }
                    let t = rv.in_target(f, &r, &input);
                    let want: Option<Vec<u64>>;
                    if drops_record(&sec.name, sec.flags, &t) {
                        let ts = tombstone(rv.format, &sec.name, rv.class64, w);
                        let inbits = rd(&input, r.offset, w as usize).unwrap_or(0) & mask;
                        let mut v = vec![ts];
                        // A per-linker record kept at the place: its input
                        // bits stand (GNU ld's redirect, lld's NONE).
                        let at = off + r.offset;
                        let kept = rv.out_recs.get(&(out, at)).is_some_and(|c| {
                            c.iter().any(|&(ors, oi)| {
                                let orr = rv.out_rels[&ors][oi];
                                match rv.format {
                                    Format::Lld => orr.ty == 0,
                                    Format::Gnu => orr.ty == r.ty,
                                    _ => false,
                                }
                            })
                        });
                        if kept || (rv.format == Format::Gnu && gnu_redirect(rv, &t).is_some()) {
                            v.push(inbits);
                        }
                        if rv.format == Format::Gnu {
                            v.push(inbits);
                        }
                        want = Some(v);
                    } else if r.addend.is_some() {
                        want = None; // RELA: the field equals the input's
                    } else {
                        // REL: the field holds the output's addend (P-P3).
                        fields.push((r.offset, w, mask, Some(Vec::new()), r.ty));
                        continue;
                    }
                    fields.push((r.offset, w, mask, want, r.ty));
                }
            }
            let mut covered = vec![false; size];
            let mut first_bits: Option<(u64, u32, u32)> = None;
            let mut tomb: Option<(u64, u64, u64)> = None;
            for &(at, w, mask, ref want, ty) in &fields {
                let Some(a) = usize::try_from(at).ok().filter(|&a| a + w as usize <= size) else {
                    continue;
                };
                for c in &mut covered[a..a + w as usize] {
                    *c = true;
                }
                let iv = rd(&input, at, w as usize).unwrap_or(0);
                let gv = rd(&got, at, w as usize).unwrap_or(0);
                let outside = (iv ^ gv)
                    & !mask
                    & if w >= 8 { u64::MAX } else { (1u64 << (8 * u32::from(w))) - 1 };
                if outside != 0 && first_bits.is_none() {
                    first_bits = Some((at, outside.count_ones(), ty));
                }
                match want {
                    None => {
                        if (iv ^ gv) & mask != 0 && first_bits.is_none() {
                            first_bits = Some((at, ((iv ^ gv) & mask).count_ones(), u32::MAX));
                        }
                    }
                    Some(v) if v.is_empty() => {}
                    Some(v) => {
                        if v.iter().all(|&x| x & mask != gv & mask) && tomb.is_none() {
                            tomb = Some((at, gv & mask, v[0]));
                        }
                    }
                }
            }
            if let Some((at, n, ty)) = first_bits {
                let msg = if ty == u32::MAX {
                    format!(
                        "{}+{at:#x}: {n} bits of a relocated field differ from the input's (RELA: a field holds its input bits at -r)",
                        rv.ins[f].sname(s)
                    )
                } else {
                    format!(
                        "{}+{at:#x}: {n} bits outside the {} field differ",
                        rv.ins[f].sname(s),
                        tname(rv.machine, ty)
                    )
                };
                rv.p_at(2, f, s, at, msg);
            }
            if let Some((at, v, t)) = tomb {
                let msg = format!(
                    "{}+{at:#x} holds {v:#x}, not the tombstone {t:#x} of a record into a dropped copy",
                    rv.ins[f].sname(s)
                );
                rv.p_at(2, f, s, at, msg);
            }
            let diff: Vec<usize> =
                (0..size).filter(|&i| !covered[i] && input[i] != got[i]).collect();
            if let Some(&first) = diff.first() {
                let msg = format!(
                    "{}+{first:#x}: {} bytes outside its relocated fields differ from the input's",
                    rv.ins[f].sname(s),
                    diff.len()
                );
                rv.p_at(2, f, s, first as u64, msg);
            }
        }
    }
    if rv.mortar() {
        mortar_pieces(rv);
    }
}

/// L-R1's key of an input section (mortar's P-P2 clause).
fn key(rv: &Rv<'_>, f: usize, s: usize) -> String {
    let sec = &rv.ins[f].elf.secs[s];
    let mut k = sec.name.clone();
    if sec.flags & SHF_LINK_ORDER != 0 {
        let to = match rv.places[f].get(sec.link as usize) {
            Some(Place::Whole { out, .. } | Place::Merged { out }) => format!("{out}"),
            _ => "?".into(),
        };
        let _ = write!(k, " link-order {to}");
    }
    if sec.flags & SHF_GNU_RETAIN != 0 {
        k.push_str(" retain");
    }
    if rv.opts.unique_of(&sec.name) {
        let _ = write!(k, " unique {f}:{s}");
    } else if sec.flags & SHF_GROUP != 0 && !rv.opts.force_group && rv.places[f][s] != Place::Absent
    {
        let _ = write!(k, " group {f}:{s}");
    }
    k
}

/// mortar's P-P2 clauses: L-R1's key, L-R4's order and offsets.
fn mortar_pieces(rv: &mut Rv<'_>) {
    let mut by_out: BTreeMap<u32, Vec<(usize, usize, u64)>> = BTreeMap::new();
    let mut by_key: BTreeMap<String, u32> = BTreeMap::new();
    for f in 0..rv.ins.len() {
        for s in 0..rv.ins[f].elf.secs.len() {
            let Place::Whole { out, off } = rv.places[f][s] else { continue };
            let k = key(rv, f, s);
            match by_key.get(&k) {
                Some(&o) if o != out => {
                    let msg = format!(
                        "{} is in output section {}, not {} (L-R1's key {k})",
                        rv.ins[f].sname(s),
                        rv.osec(out),
                        rv.osec(o)
                    );
                    rv.p_at(2, f, s, 0, msg);
                }
                Some(_) => {}
                None => {
                    by_key.insert(k, out);
                }
            }
            by_out.entry(out).or_default().push((f, s, off));
        }
    }
    let keys_of: BTreeMap<u32, BTreeSet<String>> = {
        let mut m: BTreeMap<u32, BTreeSet<String>> = BTreeMap::new();
        for (k, o) in &by_key {
            m.entry(*o).or_default().insert(k.clone());
        }
        m
    };
    for (o, keys) in &keys_of {
        if keys.len() > 1 {
            let ks: Vec<&str> = keys.iter().map(String::as_str).collect();
            rv.p(
                2,
                format!(
                    "output section {} holds the pieces of {} keys of L-R1 ({})",
                    rv.osec(*o),
                    keys.len(),
                    ks.join("; ")
                ),
            );
        }
    }
    for (o, pieces) in by_out {
        // Input order, each at the next aligned offset, the gaps zero.
        let mut end = 0u64;
        for &(f, s, off) in &pieces {
            let (align, size) = (rv.ins[f].elf.secs[s].align, rv.ins[f].elf.secs[s].size);
            let want = end.next_multiple_of(align.max(1));
            if off != want {
                let msg = format!(
                    "{} is at {off:#x} of section {}; L-R4 puts it at {want:#x} (input order, each aligned after the previous)",
                    rv.ins[f].sname(s),
                    rv.osec(o)
                );
                rv.p_at(2, f, s, 0, msg);
                break;
            }
            let gap_dirty = rv
                .out
                .secs
                .get(o as usize)
                .and_then(|x| x.data.get(end as usize..off as usize))
                .is_some_and(|g| g.iter().any(|&b| b != 0));
            if gap_dirty {
                rv.p(
                    2,
                    format!(
                        "the gap before {} in section {} is not zero (L-R4)",
                        rv.ins[f].sname(s),
                        rv.osec(o)
                    ),
                );
            }
            end = off + size;
        }
        let osize = rv.out.secs.get(o as usize).map_or(0, |x| x.size);
        if osize != end {
            rv.push(Finding::new(
                Rule::Rel(11),
                format!(
                    "section {}: sh_size is {osize:#x}, not {end:#x} (the last piece's end, L-R4)",
                    rv.osec(o)
                ),
            ));
        }
    }
}

// ---- P-P3 ----

/// The per-linker form of an output record at the place of an input
/// record §14.3 drops for its dropped target.
fn per_linker_dropped(rv: &Rv<'_>, r: &Rel, t: &Tgt, out_rec: &Rel, ot: &Tgt) -> bool {
    match rv.format {
        Format::Lld => out_rec.ty == 0,
        Format::Gnu => out_rec.ty == r.ty
            && gnu_redirect(rv, t).is_some_and(
                |g| matches!((&g, ot), (Tgt::Place(o, v), Tgt::Place(p, w)) if o == p && w == v),
            ),
        _ => false,
    }
}

/// P-P3: every input record of a placed section once, and no other.
#[allow(clippy::too_many_lines)] // the match, then the leftover records
pub(crate) fn records(rv: &mut Rv<'_>) {
    let machine = rv.machine;
    for f in 0..rv.ins.len() {
        for s in 1..rv.ins[f].elf.secs.len() {
            let (out, off) = match rv.places[f][s] {
                Place::Whole { out, off } => (out, off),
                Place::Merged { .. } => {
                    let n: u64 = rv.ins[f]
                        .elf
                        .relocs_of(s)
                        .iter()
                        .map(|&rs| rv.ins[f].elf.rels(rs).len() as u64)
                        .sum();
                    rv.report.coverage.records += n;
                    rv.report.coverage.add_not_checked(Reason::InMergeable, n);
                    continue;
                }
                _ => continue,
            };
            let data = rv.ins[f].elf.secs[s].data.clone();
            let (sname, sflags) = (rv.ins[f].elf.secs[s].name.clone(), rv.ins[f].elf.secs[s].flags);
            for rs in rv.ins[f].elf.relocs_of(s) {
                let recs = rv.ins[f].elf.rels(rs);
                for (ri, r) in recs.iter().enumerate() {
                    let t = rv.in_target(f, r, &data);
                    rv.report.coverage.records += 1;
                    let at = off + r.offset;
                    let cands = rv.out_recs.get(&(out, at)).cloned().unwrap_or_default();
                    if drops_record(&sname, sflags, &t) {
                        // §14.3 drops it: the field holds the tombstone
                        // (P-P2), or a per-linker record stands at it.
                        let mut accepted = false;
                        for &(ors, oi) in &cands {
                            if rv.used[&(ors, oi)] {
                                continue;
                            }
                            let orr = rv.out_rels[&ors][oi];
                            let ot = rv.out_target(ors, &orr);
                            if per_linker_dropped(rv, r, &t, &orr, &ot) {
                                rv.used.insert((ors, oi), true);
                                rv.matched.insert((ors, oi), (f, rs, ri));
                                accepted = true;
                                break;
                            }
                        }
                        if accepted {
                            rv.report.coverage.add_accepted(Rewrite::PerLinker, r.ty);
                        } else if let Some(&(ors, oi)) = cands.iter().find(|c| !rv.used[c]) {
                            let orr = rv.out_rels[&ors][oi];
                            let ot = rv.out_target(ors, &orr);
                            rv.used.insert((ors, oi), true);
                            rv.report.coverage.failed += 1;
                            let msg = format!(
                                "{}+{:#x} {} against {t} has an output record against {ot}, not {t} (a record into a dropped copy has no equivalent target: §14.3 drops it)",
                                rv.ins[f].sname(s),
                                r.offset,
                                tname(machine, r.ty)
                            );
                            rv.p_at(3, f, s, r.offset, msg);
                        } else {
                            rv.report.coverage.add_verified(Via::Tombstone, r.ty);
                        }
                        continue;
                    }
                    rv.match_record(f, s, rs, ri, r, &t, out, at, Via::Place);
                }
            }
        }
    }
    leftovers(rv);
    if rv.mortar() {
        mortar_records(rv);
    }
}

impl Rv<'_> {
    /// Matches input record `r` (index `ri` of `f`'s relocation section
    /// `rs`, applying to section `s`) at output place `(out, at)`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn match_record(
        &mut self,
        f: usize,
        s: usize,
        rs: usize,
        ri: usize,
        r: &Rel,
        t: &Tgt,
        out: u32,
        at: u64,
        via: Via,
    ) {
        let machine = self.machine;
        let cands = self.out_recs.get(&(out, at)).cloned().unwrap_or_default();
        // §14.6 (the supervisor's ruling on RL-5's question 15): a record
        // against a common, without a line, has the form `-d`/`-dc`/`-dp`
        // decides (a name, or the allocated place): its output record is
        // consumed and the record counted `options-unknown`.
        if !self.opts.known
            && self.ins[f].elf.syms.get(r.sym as usize).is_some_and(|y| y.shndx == SHN_COMMON)
        {
            if let Some(&(ors, oi)) = cands
                .iter()
                .find(|&&(ors, oi)| !self.used[&(ors, oi)] && self.out_rels[&ors][oi].ty == r.ty)
            {
                self.used.insert((ors, oi), true);
                self.matched.insert((ors, oi), (f, rs, ri));
            }
            self.report.coverage.add_not_checked(Reason::OptionsUnknown, 1);
            return;
        }
        let mut wrong_target: Option<Tgt> = None;
        let mut wrong_type: Option<u32> = None;
        for &(ors, oi) in &cands {
            if self.used[&(ors, oi)] {
                continue;
            }
            let orr = self.out_rels[&ors][oi];
            if orr.ty != r.ty {
                wrong_type.get_or_insert(orr.ty);
                continue;
            }
            let ot = self.out_target(ors, &orr);
            match self.equiv(t, &ot) {
                Some(per) => {
                    self.used.insert((ors, oi), true);
                    self.matched.insert((ors, oi), (f, rs, ri));
                    if per {
                        self.report.coverage.add_accepted(Rewrite::PerLinker, r.ty);
                    } else if matches!(t, Tgt::Merged(..)) {
                        self.report.coverage.add_verified(Via::String, r.ty);
                    } else {
                        self.report.coverage.add_verified(via, r.ty);
                    }
                    if self.mortar() {
                        self.mortar_form(f, s, r, t, ors, &orr);
                    }
                    return;
                }
                None => {
                    wrong_target.get_or_insert(ot);
                }
            }
        }
        self.report.coverage.failed += 1;
        let head = format!(
            "{}+{:#x} {} against {t}",
            self.ins[f].sname(s),
            r.offset,
            tname(machine, r.ty)
        );
        let msg = if let Some(ot) = wrong_target {
            // Consume it: it is this record's, wrong.
            if let Some(&(ors, oi)) = cands
                .iter()
                .find(|&&(ors, oi)| !self.used[&(ors, oi)] && self.out_rels[&ors][oi].ty == r.ty)
            {
                self.used.insert((ors, oi), true);
            }
            match (&ot, t) {
                (Tgt::Name(a, _), Tgt::Name(b, _)) if a == b && self.mortar() => {
                    let v = self.default_version_of(a).unwrap_or_default().to_string();
                    format!(
                        "{head} has an output record against {ot}; mortar's names {v} (RV-R6, lld's form)"
                    )
                }
                _ => format!("{head} has an output record against {ot}, not {t}"),
            }
        } else if let Some(ty) = wrong_type {
            if let Some(&(ors, oi)) = cands.iter().find(|c| !self.used[c]) {
                self.used.insert((ors, oi), true);
            }
            format!("{head} has an output record of type {}", tname(machine, ty))
        } else {
            let none = if r.ty == 0 {
                " (an R_*_NONE record is a reference: a --gc-sections edge the final link needs)"
            } else {
                ""
            };
            format!("{head} has no output record at {}+{at:#x}{none}", self.osec(out))
        };
        self.p_at(3, f, s, r.offset, msg);
    }

    /// mortar's target form (I-R5): a section symbol of `out(T)` with
    /// `A' = A + off(T)`; a global by its key; a local by the same local.
    fn mortar_form(&mut self, f: usize, s: usize, r: &Rel, t: &Tgt, ors: usize, orr: &Rel) {
        let Some(isym) = self.ins[f].elf.syms.get(r.sym as usize) else { return };
        let Some(osym) = self.out.syms.get(orr.sym as usize) else { return };
        let ok = match (isym.ty() == STT_SECTION, isym.local(), t) {
            (true, _, Tgt::Place(o, _)) => osym.ty() == STT_SECTION && osym.shndx == *o,
            (false, true, _) => osym.ty() != STT_SECTION && osym.name == isym.name,
            (false, false, _) => osym.name == isym.name && !osym.local(),
            _ => true,
        };
        if !ok {
            let msg = format!(
                "{}+{:#x} {}: the output record names {} '{}', not I-R5's form (mortar)",
                self.ins[f].sname(s),
                r.offset,
                tname(self.machine, r.ty),
                if osym.ty() == STT_SECTION { "the section symbol" } else { "the symbol" },
                osym.name
            );
            let _ = ors;
            self.p_at(3, f, s, r.offset, msg);
        }
    }
}

/// The output records no input record gives (P-P3's "no other records"),
/// with the per-linker forms.
fn leftovers(rv: &mut Rv<'_>) {
    let machine = rv.machine;
    let keys: Vec<(usize, usize)> = rv.used.iter().filter(|(_, u)| !**u).map(|(k, _)| *k).collect();
    for (ors, oi) in keys {
        let orr = rv.out_rels[&ors][oi];
        let tsec = rv.out.secs[ors].info;
        // GNU ld: a NONE record where it removed an FDE (L-R6).
        // (An output record, not an input one: §7.2's sum counts the
        // input records, so it is in none of its terms.)
        if rv.format == Format::Gnu && orr.ty == 0 && Some(tsec) == rv.out_eh {
            continue;
        }
        let ot = rv.out_target(ors, &orr);
        rv.push(Finding::new(
            Rule::Rel(3),
            format!(
                "output record {} at {}+{:#x} (against {ot}) comes from no input record",
                tname(machine, orr.ty),
                rv.osec(tsec),
                orr.offset
            ),
        ));
    }
}

/// mortar's P-P3 clauses (I-R5): records in the order of their pieces,
/// then the input order; no `NONE` record but the inputs'.
fn mortar_records(rv: &mut Rv<'_>) {
    let rss: Vec<usize> = rv.out_rels.keys().copied().collect();
    for ors in rss {
        let n = rv.out_rels[&ors].len();
        let mut prev: Option<(u64, usize, usize, usize)> = None;
        for oi in 0..n {
            let Some(&(f, rs, ri)) = rv.matched.get(&(ors, oi)) else { continue };
            let s = rv.ins[f].elf.secs[rs].info as usize;
            let off = match rv.places[f].get(s) {
                Some(Place::Whole { off, .. }) => *off,
                _ => rv.out_rels[&ors][oi].offset,
            };
            let key = (off, f, rs, ri);
            if let Some(p) = prev
                && (p.0, p.1, p.2, p.3) > key
                && !(p.1 == f && p.0 == off && (p.2, p.3) < (rs, ri))
            {
                rv.push(Finding::new(
                    Rule::Rel(3),
                    format!(
                        "output record {oi} of section {} is out of I-R5's order (its pieces' order, then the input order) (mortar)",
                        rv.osec(ors as u32)
                    ),
                ));
                break;
            }
            prev = Some(key);
        }
    }
}

// ---- P-P7 ----

#[allow(clippy::too_many_lines)] // field by field
fn header(rv: &mut Rv<'_>) {
    let h = rv.out.h.clone();
    let bad = |rv: &mut Rv<'_>, field: &str, x: String, y: String| {
        rv.p(7, format!("{field} is {x}, not {y}"));
    };
    if let Some(m) = rv.ins.first().map(|i| i.elf.h.machine)
        && h.machine != m
    {
        bad(rv, "e_machine", h.machine.to_string(), m.to_string());
    }
    let flags: BTreeSet<u32> = rv.ins.iter().filter(|i| !i.binary).map(|i| i.elf.h.flags).collect();
    if flags.len() <= 1 {
        let want = flags.first().copied().unwrap_or(0);
        if h.flags != want {
            bad(
                rv,
                "e_flags",
                format!("{:#x}", h.flags),
                format!("{want:#x} (A-R3's merge of the inputs')"),
            );
        }
    }
    if h.phoff != 0 {
        bad(rv, "e_phoff", format!("{:#x}", h.phoff), "0".into());
    }
    if h.phnum != 0 {
        bad(rv, "e_phnum", h.phnum.to_string(), "0".into());
    }
    if h.phentsize != 0 {
        bad(rv, "e_phentsize", h.phentsize.to_string(), "0".into());
    }
    if h.entry != 0 {
        // GNU ld and lld write -e SYM's offset in its section (L-R7).
        let per =
            !rv.mortar()
                && matches!(rv.format, Format::Gnu | Format::Lld)
                && rv.opts.entry.as_ref().is_some_and(|e| {
                    rv.out.syms.iter().any(|s| &s.name == e && s.value == h.entry)
                });
        if !per {
            bad(rv, "e_entry", format!("{:#x}", h.entry), "0".into());
        }
    }
    let gnu = rv.out.syms.iter().any(|s| s.ty() == STT_GNU_IFUNC || s.bind() == STB_GNU_UNIQUE);
    let want_abi = if gnu { 3 } else { 0 };
    // GNU ld also marks an output with an SHF_GNU_RETAIN section GNU
    // (measured; a reference's form, a question on P-P7's "iff").
    let retain = rv.out.secs.iter().any(|s| s.flags & SHF_GNU_RETAIN != 0);
    if h.osabi != want_abi && !(retain && !gnu && h.osabi == 3 && rv.format == Format::Gnu) {
        bad(
            rv,
            "EI_OSABI",
            h.osabi.to_string(),
            format!(
                "{want_abi} ({})",
                if gnu {
                    "an output symbol is STT_GNU_IFUNC or STB_GNU_UNIQUE"
                } else {
                    "no IFUNC or UNIQUE symbol"
                }
            ),
        );
    }
    match rv.out.secs.get(h.shstrndx as usize) {
        Some(s) if s.ty == SHT_STRTAB && h.shstrndx != 0 => {}
        _ => bad(rv, "e_shstrndx", h.shstrndx.to_string(), "the index of a SHT_STRTAB".into()),
    }
    let big = h.shnum >= SHN_LORESERVE as usize;
    if big && h.shnum_field != 0 {
        bad(
            rv,
            "e_shnum",
            h.shnum_field.to_string(),
            "0 (SHN_XINDEX's escape: 0xff00 sections or more)".into(),
        );
    }
    if !big && h.shstrndx_field == SHN_XINDEX as u16 && h.shstrndx < SHN_LORESERVE {
        bad(rv, "e_shstrndx", "SHN_XINDEX".into(), h.shstrndx.to_string());
    }
    // The file: every section with bytes in it, none overlapping.
    let ent: u64 = if h.class64 { 64 } else { 40 };
    let ehsize: u64 = if h.class64 { 64 } else { 52 };
    let mut spans: Vec<(u64, u64, String)> = vec![(0, ehsize, "the file header".into())];
    if h.shoff != 0 {
        spans.push((h.shoff, h.shoff + ent * h.shnum as u64, "the section header table".into()));
    }
    for (i, s) in rv.out.secs.iter().enumerate().skip(1) {
        if s.ty != SHT_NOBITS && s.size > 0 {
            spans.push((
                s.offset,
                s.offset.saturating_add(s.size),
                format!("section {i} {}", s.name),
            ));
        }
    }
    spans.sort();
    for w in spans.windows(2) {
        if w[0].1 > w[1].0 {
            let m = format!("{} and {} overlap in the file", w[0].2, w[1].2);
            rv.p(7, m);
        }
    }
    for (i, s) in rv.out.secs.iter().enumerate().skip(1) {
        let alloc = s.flags & SHF_ALLOC != 0;
        if s.addr != 0 && (alloc || rv.mortar()) {
            let m = format!("section {i} {}: sh_addr is {:#x}, not 0", s.name, s.addr);
            rv.report.findings.push(Finding::new(Rule::Rel(7), m));
        }
    }
    if rv.mortar() {
        mortar_order(rv);
    }
}

/// L-R8's header order and file offsets (mortar).
#[allow(clippy::too_many_lines)] // L-R8's order, section kind by kind
fn mortar_order(rv: &mut Rv<'_>) {
    let n = rv.out.secs.len();
    // The first piece of each output section.
    let mut first: BTreeMap<u32, (usize, usize)> = BTreeMap::new();
    for f in 0..rv.ins.len() {
        for s in 0..rv.ins[f].elf.secs.len() {
            let o = match rv.places[f][s] {
                Place::Whole { out, .. } | Place::Merged { out } => Some(out),
                Place::Framed => rv.out_eh,
                Place::Consumed => {
                    let sec = &rv.ins[f].elf.secs[s];
                    if matches!(sec.name.as_str(), ".note.gnu.property" | ".note.GNU-stack")
                        || is_container(sec)
                    {
                        (1..n).find(|&o| rv.out.secs[o].name == sec.name).map(|o| o as u32)
                    } else {
                        None
                    }
                }
                _ => None,
            };
            if let Some(o) = o {
                first.entry(o).or_insert((f, s));
            }
        }
    }
    let tail = |s: &Sec| matches!(s.ty, SHT_SYMTAB | SHT_SYMTAB_SHNDX) || (s.ty == SHT_STRTAB);
    let mut ordinary: Vec<(Option<(usize, usize)>, usize)> = Vec::new();
    let mut build_id = None;
    for o in 1..n {
        let s = &rv.out.secs[o];
        if s.is_reloc() || s.ty == SHT_GROUP || tail(s) {
            continue;
        }
        if s.name == ".note.gnu.build-id" {
            build_id = Some(o);
            continue;
        }
        ordinary.push((first.get(&(o as u32)).copied(), o));
    }
    ordinary.sort_by_key(|&(k, o)| (k.is_none(), k, o));
    let mut want: Vec<usize> = vec![0];
    want.extend(build_id);
    let mut placed_groups = BTreeSet::new();
    for &(_, o) in &ordinary {
        for g in 1..n {
            let gs = &rv.out.secs[g];
            if gs.ty == SHT_GROUP && !placed_groups.contains(&g) {
                let first_member = gs
                    .data
                    .chunks_exact(4)
                    .nth(1)
                    .map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]) as usize);
                if first_member == Some(o) {
                    want.push(g);
                    placed_groups.insert(g);
                }
            }
        }
        want.push(o);
        for r in 1..n {
            if rv.out.secs[r].is_reloc() && rv.out.secs[r].info as usize == o {
                want.push(r);
            }
        }
    }
    for name in [".symtab", ".symtab_shndx", ".strtab", ".shstrtab"] {
        if let Some(o) = (1..n).find(|&o| rv.out.secs[o].name == name && tail(&rv.out.secs[o])) {
            want.push(o);
        }
    }
    for (pos, &o) in want.iter().enumerate() {
        if pos != o {
            let m = format!(
                "section {o} {} is at position {o}, L-R8 puts it at {pos}",
                rv.out.secs[o].name
            );
            rv.p(7, m);
            break;
        }
    }
    // File offsets in header order, aligned; the header table last.
    let mut end = if rv.class64 { 64 } else { 52 };
    for o in 1..n {
        let s = &rv.out.secs[o];
        let a = s.align.max(1);
        if s.ty != SHT_NOBITS {
            if !s.offset.is_multiple_of(a) || s.offset < end {
                let m = format!(
                    "section {o} {} is at file offset {:#x}; L-R8 puts it at or after {end:#x}, aligned to {a:#x}",
                    s.name, s.offset
                );
                rv.p(7, m);
                return;
            }
            end = s.offset + s.size;
        }
    }
    let ta = if rv.class64 { 8 } else { 4 };
    let shoff = rv.out.h.shoff;
    if shoff < end || !shoff.is_multiple_of(ta) {
        rv.p(
            7,
            format!(
                "the section header table is at {shoff:#x}; L-R8 puts it last, aligned to {ta}"
            ),
        );
    }
}

// ---- P-P8 ----

fn container(rv: &mut Rv<'_>) {
    let outs: Vec<usize> =
        (1..rv.out.secs.len()).filter(|&o| is_container(&rv.out.secs[o])).collect();
    let mut inputs: Vec<(usize, usize)> = Vec::new();
    for f in 0..rv.ins.len() {
        for s in 0..rv.ins[f].elf.secs.len() {
            if is_container(&rv.ins[f].elf.secs[s]) {
                inputs.push((f, s));
            }
        }
    }
    if outs.len() > 1 {
        rv.p(8, format!("the output has {} containers", outs.len()));
        return;
    }
    let mut want = Vec::new();
    for &(f, s) in &inputs {
        let sec = crate::meta::read(&rv.ins[f].elf.secs[s].data);
        want.extend(crate::meta::kept(sec.units.iter()));
    }
    match outs.first() {
        None if !want.is_empty() => {
            rv.p(8, "the output has 0 containers; the live inputs have units (MT-30)");
        }
        None => {}
        Some(&o) => {
            let s = rv.out.secs[o].clone();
            if s.align != 8 {
                rv.p(8, format!("the container is aligned to {}, not 8", s.align));
            }
            // A reference concatenates the input containers by name, each
            // at a multiple of 8: kept_units' bytes unless an input has
            // bytes between its units (tests/meta's b.o: 8 zeros); that
            // copy is its form (§14.5 P-P8).
            let mut cat: Vec<u8> = Vec::new();
            for &(f, s) in &inputs {
                cat.resize(cat.len().next_multiple_of(8), 0);
                cat.extend_from_slice(&rv.ins[f].elf.secs[s].data);
            }
            if s.data != want && (rv.mortar() || s.data != cat) {
                let at = s
                    .data
                    .iter()
                    .zip(&want)
                    .position(|(a, b)| a != b)
                    .unwrap_or(s.data.len().min(want.len()));
                rv.p(8, format!("container bytes differ from kept_units at {at:#x}"));
            }
        }
    }
}

// ---- P-P10 ----

#[allow(clippy::too_many_lines)] // the three notes
fn notes(rv: &mut Rv<'_>) {
    // .note.GNU-stack.
    let mut any_x: Option<String> = None;
    let mut any_n = false;
    let mut absent = false;
    for i in &rv.ins {
        if i.binary {
            absent = true;
            continue;
        }
        let ns: Vec<&Sec> = i.elf.secs.iter().filter(|s| s.name == ".note.GNU-stack").collect();
        if ns.is_empty() {
            absent = true;
        }
        for s in ns {
            if s.flags & SHF_EXECINSTR != 0 {
                any_x.get_or_insert_with(|| i.label());
            } else {
                any_n = true;
            }
        }
    }
    let outs: Vec<Sec> =
        rv.out.secs.iter().filter(|s| s.name == ".note.GNU-stack").cloned().collect();
    let out_x = outs.iter().any(|s| s.flags & SHF_EXECINSTR != 0);
    if let Some(file) = &any_x
        && !out_x
    {
        let is = if outs.is_empty() { "absent" } else { "N" };
        rv.p(10, format!("an input asks for an executable stack ({file}) and the output's .note.GNU-stack is {is}"));
    }
    if rv.mortar() && rv.opts.known {
        let _ = absent;
        let aarch64 = rv.machine == EM_AARCH64;
        #[allow(clippy::match_same_arms)] // L-R6a's table, cell by cell
        let want = match (any_x.is_some(), any_n, rv.opts.execstack) {
            (true, _, _) => Some(true),
            (false, true, Some(true)) => Some(aarch64),
            (false, true, _) => Some(false),
            (false, false, Some(true)) => Some(true),
            (false, false, Some(false)) => Some(false),
            (false, false, None) => None,
        };
        let got = match outs.as_slice() {
            [] => None,
            [s] => {
                if s.size != 0
                    || s.ty != SHT_PROGBITS
                    || s.align > 1
                    || s.flags & !SHF_EXECINSTR != 0
                {
                    rv.p(10, "the output's .note.GNU-stack is not empty PROGBITS, alignment 1, flags 0 or SHF_EXECINSTR (L-R6a)");
                }
                Some(s.flags & SHF_EXECINSTR != 0)
            }
            _ => {
                rv.p(10, format!("the output has {} .note.GNU-stack sections", outs.len()));
                return;
            }
        };
        if got != want {
            let w = |v: Option<bool>| match v {
                None => "absent",
                Some(true) => "X",
                Some(false) => "N",
            };
            rv.p(
                10,
                format!(
                    "the output's .note.GNU-stack is {}, L-R6a's table gives {}",
                    w(got),
                    w(want)
                ),
            );
        }
    }
    // .note.gnu.property.
    let ins_owned: Vec<Vec<Vec<u8>>> = rv
        .ins
        .iter()
        .map(|i| {
            i.elf
                .secs
                .iter()
                .filter(|s| s.name == ".note.gnu.property" && s.ty == SHT_NOTE)
                .map(|s| s.data.clone())
                .collect()
        })
        .collect();
    let ins: Vec<Vec<&[u8]>> =
        ins_owned.iter().map(|v| v.iter().map(Vec::as_slice).collect()).collect();
    let out_notes: Vec<Sec> =
        rv.out.secs.iter().filter(|s| s.name == ".note.gnu.property").cloned().collect();
    let align = if rv.class64 { 8 } else { 4 };
    if out_notes.len() > 1 {
        rv.p(10, format!("the output has {} .note.gnu.property sections", out_notes.len()));
    } else {
        if let Some(s) = out_notes.first()
            && (s.ty != SHT_NOTE || s.flags & SHF_ALLOC == 0 || s.align != align)
        {
            rv.p(10, format!(".note.gnu.property is not an allocated SHT_NOTE aligned to {align}"));
        }
        let (msgs, judged, not_judged) = crate::property::check_rel(
            rv.format,
            rv.machine,
            align as usize,
            &ins,
            out_notes.first().map(|s| s.data.as_slice()),
        );
        rv.report.coverage.properties_judged += judged;
        rv.report.coverage.properties_not_judged += not_judged;
        for m in msgs {
            rv.p(10, m);
        }
    }
    // .note.gnu.build-id.
    let has = rv.out.secs.iter().any(|s| s.name == ".note.gnu.build-id" && s.ty == SHT_NOTE);
    if rv.opts.known {
        match (&rv.opts.build_id, has) {
            (Some(style), false) => {
                rv.p(10, format!("the line asks for a build id ({style}) and the output has none"));
            }
            (None, true) => rv.p(10, "the output has a build id and the line asks for none"),
            _ => {}
        }
    }
    if rv.mortar() && has {
        // B1 over the whole file (I-R8).
        match crate::image::Image::read(&rebuild_bytes(rv)) {
            Ok(img) => {
                let style = crate::buildid::Style::of(rv.map, None);
                let fs = crate::buildid::check(&img, &mut rv.report.coverage, style);
                rv.report.findings.extend(fs);
            }
            Err(e) => rv.p(10, format!("the build-id note cannot be read: {e}")),
        }
    }
}

/// The output's bytes as given (the image reader reads them again).
fn rebuild_bytes(rv: &Rv<'_>) -> Vec<u8> {
    rv.raw.clone()
}

// ---- P-P11 ----

/// The pieces of each output section: (file, section).
pub(crate) fn pieces_of(rv: &Rv<'_>) -> BTreeMap<u32, Vec<(usize, usize)>> {
    let mut m: BTreeMap<u32, Vec<(usize, usize)>> = BTreeMap::new();
    for f in 0..rv.ins.len() {
        for s in 0..rv.ins[f].elf.secs.len() {
            match rv.places[f][s] {
                Place::Whole { out, .. } | Place::Merged { out } => {
                    m.entry(out).or_default().push((f, s));
                }
                Place::Framed => {
                    if let Some(o) = rv.out_eh {
                        m.entry(o).or_default().push((f, s));
                    }
                }
                _ => {}
            }
        }
    }
    m
}

#[allow(clippy::too_many_lines)] // field by field
fn sections(rv: &mut Rv<'_>) {
    let pieces = pieces_of(rv);
    let n = rv.out.secs.len();
    let symtab = rv.out.symtab.unwrap_or(0) as u32;
    rv.cov.sections = n.saturating_sub(1) as u64;
    let made = |s: &Sec| {
        s.is_reloc()
            || matches!(s.ty, SHT_GROUP | SHT_SYMTAB | SHT_SYMTAB_SHNDX | SHT_STRTAB)
            || matches!(
                s.name.as_str(),
                ".note.GNU-stack" | ".note.gnu.property" | ".note.gnu.build-id"
            )
            || is_container(s)
    };
    let common_bss =
        !rv.map.rel.as_ref().is_none_or(|r| r.commons.is_empty()) || rv.opts.define_common;
    for o in 1..n {
        let os = rv.out.secs[o].clone();
        let mut bad: Vec<String> = Vec::new();
        let ps_owned: Vec<Sec> = pieces
            .get(&(o as u32))
            .map(|v| v.iter().map(|&(f, s)| rv.ins[f].elf.secs[s].clone()).collect())
            .unwrap_or_default();
        let ps: Vec<&Sec> = ps_owned.iter().collect();
        if ps.is_empty() {
            let eh4 = rv.format == Format::Mold && rv.is_eh(&os) && os.size == 4;
            let bss = common_bss && os.ty == SHT_NOBITS;
            if !made(&os) && !eh4 && !bss {
                rv.p(11, format!("section {o} {} comes from no input section", os.name));
                continue;
            }
            // The sections the -r makes.
            let want_link = match os.ty {
                SHT_REL | SHT_RELA | SHT_GROUP | SHT_SYMTAB_SHNDX => Some(symtab),
                SHT_SYMTAB => Some(os.link.max(1))
                    .filter(|&l| rv.out.secs.get(l as usize).is_some_and(|s| s.ty == SHT_STRTAB))
                    .or(Some(u32::MAX)),
                _ => None,
            };
            if let Some(l) = want_link
                && os.link != l
            {
                bad.push(format!(
                    "sh_link is {}, not {}",
                    os.link,
                    if l == u32::MAX { "a SHT_STRTAB".into() } else { l.to_string() }
                ));
            }
            let es = match (os.ty, rv.class64) {
                (SHT_SYMTAB | SHT_RELA, true) => Some(24),
                (SHT_SYMTAB, false) | (SHT_REL, true) => Some(16),
                (SHT_RELA, false) => Some(12),
                (SHT_REL, false) => Some(8),
                (SHT_GROUP | SHT_SYMTAB_SHNDX, _) => Some(4),
                _ => None,
            };
            if let Some(e) = es
                && os.entsize != e
            {
                bad.push(format!("sh_entsize is {}, not {e}", os.entsize));
            }
            if os.is_reloc() {
                let rk = rv.rel_kind();
                let llvm_target = rv
                    .out
                    .secs
                    .get(os.info as usize)
                    .is_some_and(|t| t.ty == SHT_LLVM_CALL_GRAPH_PROFILE);
                if os.ty != rk && !llvm_target {
                    bad.push(format!("sh_type is {:#x}, not {rk:#x} (A-R4)", os.ty));
                }
                if os.flags & SHF_INFO_LINK == 0 {
                    bad.push("SHF_INFO_LINK is not set".into());
                }
                if os.info == 0 || os.info as usize >= n {
                    bad.push(format!("sh_info is {}, which names no section", os.info));
                }
                if let Some(e) = es
                    && !os.size.is_multiple_of(e)
                {
                    bad.push(format!("sh_size {:#x} is not a multiple of {e}", os.size));
                }
                if os.size == 0 {
                    bad.push(
                        "holds no record (L-R10: a relocation section exists only with a record)"
                            .into(),
                    );
                }
            }
            if os.ty == SHT_SYMTAB && os.info as usize != rv.out.first_global() {
                bad.push(format!("sh_info is {}, not the first non-local", os.info));
            }
        } else {
            judge_pieces(rv, o, &os, &ps, &mut bad);
            if rv.is_eh(&os) || os.ty == SHT_NOBITS {
                // .eh_frame's size is the record map's; NOBITS, its pieces'.
            }
        }
        for b in bad {
            rv.p(11, format!("section {o} {}: {b}", os.name));
        }
    }
}

/// P-P11 for an output section that holds pieces.
#[allow(clippy::too_many_lines)] // field by field
fn judge_pieces(rv: &mut Rv<'_>, o: usize, os: &Sec, ps: &[&Sec], bad: &mut Vec<String>) {
    let llvm = matches!(ps[0].ty, SHT_LLVM_ADDRSIG | SHT_LLVM_CALL_GRAPH_PROFILE) && !rv.mortar();
    // sh_type by L-49.
    let types: BTreeSet<u32> = ps.iter().map(|p| p.ty).collect();
    let want_ty = if types.len() == 1 {
        Some(ps[0].ty)
    } else if types == BTreeSet::from([SHT_PROGBITS, SHT_NOBITS]) {
        Some(SHT_PROGBITS)
    } else {
        None
    };
    if let Some(t) = want_ty
        && os.ty != t
        && !(rv.is_eh(os)
            && rv.machine == EM_X86_64
            && matches!(os.ty, SHT_PROGBITS | SHT_X86_64_UNWIND))
    {
        bad.push(format!("sh_type is {:#x}, not {t:#x} (its pieces')", os.ty));
    }
    let or_bits = SHF_WRITE | SHF_ALLOC | SHF_EXECINSTR | SHF_TLS;
    let want_or = ps.iter().fold(0, |a, p| a | (p.flags & or_bits));
    if os.flags & or_bits != want_or {
        bad.push(format!(
            "sh_flags {:#x} has not the OR of its pieces' W, A, X, TLS ({want_or:#x})",
            os.flags
        ));
    }
    let class = |p: &Sec| {
        (p.flags & (SHF_MERGE | SHF_STRINGS), if p.flags & SHF_MERGE != 0 { p.entsize } else { 0 })
    };
    let classes: BTreeSet<(u64, u64)> = ps.iter().map(|p| class(p)).collect();
    let same_merge = classes.len() == 1 && ps[0].flags & SHF_MERGE != 0;
    if os.flags & SHF_MERGE != 0 {
        if os.entsize == 0 {
            bad.push("SHF_MERGE with sh_entsize 0".into());
        } else if !same_merge {
            bad.push("SHF_MERGE over pieces of different merge classes".into());
        } else if os.entsize != ps[0].entsize {
            bad.push(format!(
                "sh_entsize is {}, not {} (the merge class's)",
                os.entsize, ps[0].entsize
            ));
        }
    } else if rv.mortar() && same_merge {
        bad.push("SHF_MERGE is dropped though every piece has the same merge class (L-50)".into());
    }
    for (flag, nm) in [(SHF_LINK_ORDER, "SHF_LINK_ORDER"), (SHF_EXCLUDE, "SHF_EXCLUDE")] {
        let all = ps.iter().all(|p| p.flags & flag != 0);
        if (os.flags & flag != 0) != all {
            bad.push(format!(
                "{nm} is {} though {} piece has it",
                if all { "not set" } else { "set" },
                if all { "every" } else { "not every" }
            ));
        }
    }
    let pieces = pieces_of(rv);
    let list = pieces.get(&(o as u32)).cloned().unwrap_or_default();
    if os.flags & SHF_GNU_RETAIN == 0
        && let Some(&(f, s)) =
            list.iter().find(|&&(f, s)| rv.ins[f].elf.secs[s].flags & SHF_GNU_RETAIN != 0)
    {
        bad.push(format!(
            "holds the SHF_GNU_RETAIN piece {} without SHF_GNU_RETAIN",
            rv.ins[f].sname(s)
        ));
    }
    if os.flags & SHF_GNU_RETAIN != 0 && !ps.iter().any(|p| p.flags & SHF_GNU_RETAIN != 0) {
        bad.push("SHF_GNU_RETAIN is set and no piece has it".into());
    }
    if rv.mortar()
        && os.flags & SHF_GNU_RETAIN != 0
        && !ps.iter().all(|p| p.flags & SHF_GNU_RETAIN != 0)
    {
        bad.push(
            "SHF_GNU_RETAIN holds a piece without it (L-R1a: the flag is part of the key)".into(),
        );
    }
    if os.flags & SHF_COMPRESSED != 0 && rv.mortar() {
        bad.push("SHF_COMPRESSED (L-R1a: no output is compressed)".into());
    }
    let mut want_align = ps.iter().map(|p| p.align.max(1)).max().unwrap_or(1);
    // Under `-d` the commons allocated in this section are pieces too:
    // their alignment (an `SHN_COMMON` symbol's value) counts (RL-6).
    if rv.opts.define_common {
        for f in &rv.ins {
            for y in f.elf.syms.iter().filter(|y| y.shndx == SHN_COMMON) {
                if rv.out.syms.iter().any(|z| z.name == y.name && !z.local() && z.shndx == o as u32)
                {
                    want_align = want_align.max(y.value.max(1));
                }
            }
        }
    }
    if os.align.max(1) != want_align && !rv.is_eh(os) {
        bad.push(format!("sh_addralign is {}, not {want_align} (its largest piece's)", os.align));
    }
    // sh_size: at least the end of its last piece.
    let end = list
        .iter()
        .filter_map(|&(f, s)| match rv.places[f][s] {
            Place::Whole { off, .. } => Some(off + rv.ins[f].elf.secs[s].size),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    if os.size < end {
        bad.push(format!("sh_size {:#x} is less than its last piece's end {end:#x}", os.size));
    }
    // sh_link.
    if !llvm {
        let want_link: Option<u32> =
            if os.flags & SHF_LINK_ORDER != 0 || ps.iter().any(|p| p.flags & SHF_LINK_ORDER != 0) {
                let targets: BTreeSet<Option<u32>> = list
                    .iter()
                    .map(|&(f, s)| match rv.places[f].get(rv.ins[f].elf.secs[s].link as usize) {
                        Some(Place::Whole { out, .. } | Place::Merged { out }) => Some(*out),
                        _ => None,
                    })
                    .collect();
                let v: Vec<Option<u32>> = targets.into_iter().collect();
                if v.len() > 1 {
                    bad.push(format!(
                        "SHF_LINK_ORDER pieces link to {} and {}",
                        v[0].map_or("?".into(), |x| x.to_string()),
                        v[1].map_or("?".into(), |x| x.to_string())
                    ));
                    None
                } else {
                    v.first().copied().flatten()
                }
            } else {
                Some(0)
            };
        if let Some(l) = want_link
            && os.link != l
        {
            if l != 0 && os.link == 0 {
                bad.push("sh_link is 0".into());
            } else {
                bad.push(format!("sh_link is {}, not {l}", os.link));
            }
        }
        // sh_entsize.
        let ents: BTreeSet<u64> = ps.iter().map(|p| p.entsize).collect();
        let want_es = if same_merge || ents.len() == 1 { ps[0].entsize } else { 0 };
        // "the pieces' common value or 0"; an array of pointers
        // (SHT_INIT_ARRAY, SHT_FINI_ARRAY, SHT_PREINIT_ARRAY) may also
        // state the pointer size, as GNU ld does (a question on P-P11).
        let ptr = if rv.class64 { 8 } else { 4 };
        let array = !rv.mortar() && matches!(os.ty, 14..=16) && os.entsize == ptr;
        // The supervisor's ruling on RL-5's question 5: for mortar the
        // rule is exact (0 where the pieces agree on a value is a
        // finding); a reference may write 0, or GNU ld's pointer size on
        // the arrays.
        if os.flags & SHF_MERGE == 0
            && os.entsize != want_es
            && (rv.mortar() || (os.entsize != 0 && !array))
        {
            let exact =
                if rv.mortar() { " (mortar: the pieces' common value, exactly)" } else { "" };
            bad.push(format!("sh_entsize is {}, not {want_es}{exact}", os.entsize));
        }
    }
    // sh_info: the input's, when the pieces agree.
    let infos: BTreeSet<u32> = ps.iter().map(|p| p.info).collect();
    if infos.len() == 1 && os.info != ps[0].info && !llvm {
        bad.push(format!("sh_info is {}, not {} (its pieces')", os.info, ps[0].info));
    }
    // sh_name reads back as its pieces' name.
    if ps.iter().any(|p| p.name != os.name) && !rv.is_eh(os) {
        bad.push("its name is not its pieces' name".into());
    }
    if rv.mortar() && os.size != end && !rv.is_eh(os) && os.ty != SHT_NOBITS {
        // mortar's sh_size is exactly the last piece's end (judged in
        // P-P2's L-R4 clause).
    }
}

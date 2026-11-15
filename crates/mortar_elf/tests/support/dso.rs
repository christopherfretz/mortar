//! Shared objects for the tests of `spec/elf.md` §6: a writer for a small
//! `ET_DYN` file with one of everything the shared-object reader has an
//! accessor for (both classes, three hash styles), the section-stripped
//! form of any file, an owned view of everything the reader says about a
//! file, and a walk of every accessor that goes on after errors.

use std::fmt::Write as _;

use mortar_elf::consts::{
    DF_1_NOW, DF_BIND_NOW, DF_STATIC_TLS, DT_AUXILIARY, DT_FILTER, DT_FLAGS, DT_FLAGS_1,
    DT_GNU_HASH, DT_HASH, DT_NEEDED, DT_NULL, DT_RPATH, DT_RUNPATH, DT_SONAME, DT_STRSZ, DT_STRTAB,
    DT_SYMENT, DT_SYMTAB, DT_VERDEF, DT_VERDEFNUM, DT_VERNEED, DT_VERNEEDNUM, DT_VERSYM,
    EM_AARCH64, ET_DYN, GNU_PROPERTY_AARCH64_FEATURE_1_AND, GNU_PROPERTY_X86_FEATURE_1_AND,
    NT_GNU_PROPERTY_TYPE_0, PF_R, PF_W, PF_X, PT_DYNAMIC, PT_GNU_PROPERTY, PT_GNU_RELRO,
    PT_GNU_STACK, PT_LOAD, PT_NOTE, PT_TLS, SHF_ALLOC, SHF_EXECINSTR, SHF_TLS, SHF_WRITE,
    SHN_UNDEF, SHT_DYNAMIC, SHT_DYNSYM, SHT_GNU_HASH, SHT_GNU_VERDEF, SHT_GNU_VERNEED,
    SHT_GNU_VERSYM, SHT_HASH, SHT_NOBITS, SHT_NOTE, SHT_PROGBITS, SHT_STRTAB, STB_GLOBAL,
    STB_LOCAL, STB_WEAK, STT_FUNC, STT_NOTYPE, STT_OBJECT, STT_SECTION, STT_TLS, STV_DEFAULT,
    STV_PROTECTED, VER_FLG_BASE, VER_FLG_WEAK, VERSYM_HIDDEN,
};
use mortar_elf::{
    Class, Dyn, Ehdr, Entry, Error, FileHeader, Phdr, Record, SharedObject, Shdr, Sym, SymbolIndex,
    SymbolSection, Version,
};

use super::{enc_ehdr, enc_note, enc_property, enc_shdr, enc_sym, put, put16, put32, shdr_size};

// ---- encoders ---------------------------------------------------------------

pub fn enc_phdr(class: Class, p: &Phdr) -> Vec<u8> {
    let mut out = Vec::new();
    put32(&mut out, p.p_type);
    match class {
        Class::Elf32 => {
            for word in [p.p_offset, p.p_vaddr, p.p_paddr, p.p_filesz, p.p_memsz] {
                put32(&mut out, word as u32);
            }
            put32(&mut out, p.p_flags);
            put32(&mut out, p.p_align as u32);
        }
        Class::Elf64 => {
            put32(&mut out, p.p_flags);
            for word in [p.p_offset, p.p_vaddr, p.p_paddr, p.p_filesz, p.p_memsz, p.p_align] {
                out.extend_from_slice(&word.to_le_bytes());
            }
        }
    }
    out
}

pub fn enc_dyn(class: Class, d: &Dyn) -> Vec<u8> {
    let mut out = Vec::new();
    put(&mut out, class, d.d_tag as u64);
    put(&mut out, class, d.d_val);
    out
}

pub const fn phdr_size(class: Class) -> usize {
    match class {
        Class::Elf32 => 32,
        Class::Elf64 => 56,
    }
}

pub const fn dyn_size(class: Class) -> usize {
    match class {
        Class::Elf32 => 8,
        Class::Elf64 => 16,
    }
}

/// The System V ABI's `elf_hash`, for `vd_hash` and `vna_hash`.
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

/// The GNU hash function (Bernstein's, `h * 33 + c` from 5381).
pub fn gnu_hash(name: &[u8]) -> u32 {
    name.iter().fold(5381u32, |h, &c| h.wrapping_mul(33).wrapping_add(u32::from(c)))
}

// ---- the sample shared object -----------------------------------------------

/// Which hash tables the sample carries (`--hash-style`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashStyle {
    Sysv,
    Gnu,
    Both,
}

pub const HASH_STYLES: [HashStyle; 3] = [HashStyle::Sysv, HashStyle::Gnu, HashStyle::Both];

/// Where one part of a built shared object is.
#[derive(Debug, Clone, Copy)]
pub struct Part {
    pub name: &'static str,
    /// The section index.
    pub index: u32,
    pub offset: usize,
    pub size: usize,
    pub addr: u64,
}

/// A built shared object and where its parts are, for tests that damage
/// it.
#[derive(Debug, Clone)]
pub struct Dso {
    pub class: Class,
    pub bytes: Vec<u8>,
    pub parts: Vec<Part>,
    pub phoff: usize,
    pub phnum: u32,
    pub shoff: usize,
    pub shnum: u32,
    /// The number of entries in `.dynamic`, trailing `DT_NULL`s included.
    pub dynamic_entries: usize,
}

/// The program header indexes of the sample.
pub const PH_TEXT: u32 = 0;
pub const PH_DATA: u32 = 1;
pub const PH_DYNAMIC: u32 = 2;
pub const PH_NOTE: u32 = 3;
pub const PH_PROPERTY: u32 = 4;
pub const PH_TLS: u32 = 5;
pub const PH_RELRO: u32 = 6;
pub const PH_STACK: u32 = 7;
const PHNUM: u32 = 8;

/// The dynamic symbols of the sample, by name and index.
pub const SYM_SECTION: u32 = 1;
pub const SYM_BAZ: u32 = 2;
pub const SYM_WEAKREF: u32 = 3;
pub const SYM_FOO_1: u32 = 4;
pub const SYM_FOO_2: u32 = 5;
pub const SYM_BAR: u32 = 6;
pub const SYM_HID: u32 = 7;
pub const SYM_TLS: u32 = 8;
pub const SYM_PROT: u32 = 9;
pub const SYMBOLS: u32 = 10;
/// The first symbol the GNU hash table covers.
pub const SYMOFFSET: u32 = 4;

/// The distance from file offset to address in the writable segment.
pub const DATA_BIAS: u64 = 0x10000;

/// The strings of `.dynstr`, in order.
const STRINGS: [&str; 15] = [
    "libc.so.6",
    "libm.so.6",
    "libt.so",
    "/opt/rpath",
    "/opt/runpath",
    "baz",
    "weakref",
    "foo",
    "bar",
    "hid",
    "tls",
    "prot",
    "VER_1",
    "VER_2",
    "GLIBC_2.2",
];
/// Not in [`STRINGS`]: kept apart so the name `M_1` is last.
const LAST_STRING: &str = "M_1";

struct Strings(Vec<u8>);

impl Strings {
    fn new() -> Self {
        let mut bytes = vec![0u8];
        for s in STRINGS.iter().chain([&LAST_STRING]) {
            bytes.extend_from_slice(s.as_bytes());
            bytes.push(0);
        }
        Strings(bytes)
    }

    fn at(&self, s: &str) -> u32 {
        let needle: Vec<u8> = [&[0u8][..], s.as_bytes(), &[0]].concat();
        self.0.windows(needle.len()).position(|w| w == needle).expect("a string") as u32 + 1
    }
}

/// What the parts are made from: section indexes and addresses, which the
/// layout fixes, and the class and hash style.
#[derive(Debug, Default, Clone)]
struct Layout {
    index: Vec<(&'static str, u32)>,
    addr: Vec<(&'static str, u64)>,
}

impl Layout {
    fn index(&self, name: &str) -> u32 {
        self.index.iter().find(|(n, _)| *n == name).map_or(0, |(_, i)| *i)
    }

    fn addr(&self, name: &str) -> u64 {
        self.addr.iter().find(|(n, _)| *n == name).map_or(0, |(_, a)| *a)
    }
}

/// A section of the sample: its header (without name, offset, address and
/// size) and its contents.
struct Sec {
    name: &'static str,
    shdr: Shdr,
    link: Option<&'static str>,
    data: Vec<u8>,
    /// In the writable segment.
    writable: bool,
}

fn sym(st_name: u32, value: u64, size: u64, bind: u8, ty: u8, other: u8, shndx: u32) -> Sym {
    Sym {
        st_name,
        st_value: value,
        st_size: size,
        st_info: (bind << 4) | ty,
        st_other: other,
        st_shndx: shndx as u16,
    }
}

#[allow(clippy::too_many_lines)] // one section after another
fn sections(class: Class, e_machine: u16, hash: HashStyle, at: &Layout) -> Vec<Sec> {
    let strings = Strings::new();
    let s = |name: &str| strings.at(name);
    let word = class.word_size() as u64;
    let rodata = at.addr(".rodata");
    let symbols = [
        Sym::default(),
        sym(0, rodata, 0, STB_LOCAL, STT_SECTION, STV_DEFAULT, at.index(".rodata")),
        sym(s("baz"), 0, 0, STB_GLOBAL, STT_FUNC, STV_DEFAULT, u32::from(SHN_UNDEF)),
        sym(s("weakref"), 0, 0, STB_WEAK, STT_NOTYPE, STV_DEFAULT, u32::from(SHN_UNDEF)),
        sym(s("foo"), rodata, 4, STB_GLOBAL, STT_FUNC, STV_DEFAULT, at.index(".rodata")),
        sym(s("foo"), rodata + 4, 4, STB_GLOBAL, STT_FUNC, STV_DEFAULT, at.index(".rodata")),
        sym(s("bar"), at.addr(".data"), 16, STB_GLOBAL, STT_OBJECT, STV_DEFAULT, at.index(".data")),
        sym(s("hid"), rodata + 8, 4, STB_GLOBAL, STT_FUNC, STV_DEFAULT, at.index(".rodata")),
        sym(s("tls"), 0, 8, STB_GLOBAL, STT_TLS, STV_DEFAULT, at.index(".tdata")),
        sym(s("prot"), rodata + 12, 4, STB_GLOBAL, STT_OBJECT, STV_PROTECTED, at.index(".rodata")),
    ];
    let dynsym: Vec<u8> = symbols.iter().flat_map(|sym| enc_sym(class, sym)).collect();
    // nbucket 1; the bucket holds the last symbol, each chain entry the one
    // before it.
    let mut sysv = vec![1, SYMBOLS, SYMBOLS - 1];
    sysv.extend((0..SYMBOLS).map(|i| i.saturating_sub(1)));
    // nbuckets 1, symoffset 4, one bloom word (all bits set), shift 6;
    // bucket 0 starts at symbol 4; the chain runs to the last symbol.
    let mut gnu = super::words(&[1, SYMOFFSET, 1, 6]);
    gnu.resize(gnu.len() + word as usize, 0xff);
    gnu.extend(super::words(&[SYMOFFSET]));
    for (i, symbol) in (SYMOFFSET..SYMBOLS).zip(&symbols[SYMOFFSET as usize..]) {
        let name = STRINGS.iter().find(|n| s(n) == symbol.st_name).expect("a name");
        let end = u32::from(i == SYMBOLS - 1);
        gnu.extend(super::words(&[(gnu_hash(name.as_bytes()) & !1) | end]));
    }
    let versym: Vec<u8> = [0u16, 0, 4, 5, 2 | VERSYM_HIDDEN, 3, 1, 3 | VERSYM_HIDDEN, 2, 1]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let mut verdef = Vec::new();
    let defs: [(u16, u16, &[&str]); 3] =
        [(VER_FLG_BASE, 1, &["libt.so"]), (0, 2, &["VER_1"]), (0, 3, &["VER_2", "VER_1"])];
    for (i, (flags, ndx, names)) in defs.iter().enumerate() {
        put16(&mut verdef, 1);
        put16(&mut verdef, *flags);
        put16(&mut verdef, *ndx);
        put16(&mut verdef, names.len() as u16);
        put32(&mut verdef, elf_hash(names[0].as_bytes()));
        put32(&mut verdef, 20);
        let next = if i + 1 == defs.len() { 0 } else { 20 + 8 * names.len() as u32 };
        put32(&mut verdef, next);
        for (j, name) in names.iter().enumerate() {
            put32(&mut verdef, s(name));
            put32(&mut verdef, if j + 1 == names.len() { 0 } else { 8 });
        }
    }
    let mut verneed = Vec::new();
    let needs: [(&str, &str, u16, u16); 2] =
        [("libc.so.6", "GLIBC_2.2", 0, 4), ("libm.so.6", LAST_STRING, VER_FLG_WEAK, 5)];
    for (i, (file, name, flags, other)) in needs.iter().enumerate() {
        put16(&mut verneed, 1);
        put16(&mut verneed, 1);
        put32(&mut verneed, s(file));
        put32(&mut verneed, 16);
        put32(&mut verneed, if i + 1 == needs.len() { 0 } else { 32 });
        put32(&mut verneed, elf_hash(name.as_bytes()));
        put16(&mut verneed, *flags);
        put16(&mut verneed, *other);
        put32(&mut verneed, s(name));
        put32(&mut verneed, 0);
    }
    let (pr_type, feature) = if e_machine == EM_AARCH64 {
        (GNU_PROPERTY_AARCH64_FEATURE_1_AND, 3u32)
    } else {
        (GNU_PROPERTY_X86_FEATURE_1_AND, 3u32)
    };
    let note_align = word as usize;
    let note = enc_note(
        ELF_NOTE_GNU_Z,
        NT_GNU_PROPERTY_TYPE_0,
        &enc_property(class, pr_type, &feature.to_le_bytes()),
        note_align,
    );
    let mut dynamic: Vec<(i64, u64)> = vec![
        (DT_NEEDED, u64::from(s("libc.so.6"))),
        (DT_NEEDED, u64::from(s("libm.so.6"))),
        (DT_SONAME, u64::from(s("libt.so"))),
        (DT_RPATH, u64::from(s("/opt/rpath"))),
        (DT_RUNPATH, u64::from(s("/opt/runpath"))),
    ];
    if hash != HashStyle::Gnu {
        dynamic.push((DT_HASH, at.addr(".hash")));
    }
    if hash != HashStyle::Sysv {
        dynamic.push((DT_GNU_HASH, at.addr(".gnu.hash")));
    }
    dynamic.extend([
        (DT_STRTAB, at.addr(".dynstr")),
        (DT_STRSZ, strings.0.len() as u64),
        (DT_SYMTAB, at.addr(".dynsym")),
        (DT_SYMENT, super::sym_size(class)),
        (DT_VERSYM, at.addr(".gnu.version")),
        (DT_VERDEF, at.addr(".gnu.version_d")),
        (DT_VERDEFNUM, 3),
        (DT_VERNEED, at.addr(".gnu.version_r")),
        (DT_VERNEEDNUM, 2),
        (DT_FLAGS, DF_BIND_NOW | DF_STATIC_TLS),
        (DT_FLAGS_1, DF_1_NOW),
        (DT_NULL, 0),
        (DT_NULL, 0),
        (DT_NULL, 0),
    ]);
    let dynamic: Vec<u8> =
        dynamic.iter().flat_map(|&(d_tag, d_val)| enc_dyn(class, &Dyn { d_tag, d_val })).collect();

    let a = SHF_ALLOC;
    let shdr = |sh_type: u32, flags: u64, align: u64, entsize: u64, info: u32| Shdr {
        sh_type,
        sh_flags: flags,
        sh_addralign: align,
        sh_entsize: entsize,
        sh_info: info,
        ..Shdr::default()
    };
    let mut out = vec![
        Sec {
            name: ".dynsym",
            shdr: shdr(SHT_DYNSYM, a, word, super::sym_size(class), SYM_BAZ),
            link: Some(".dynstr"),
            data: dynsym,
            writable: false,
        },
        Sec {
            name: ".dynstr",
            shdr: shdr(SHT_STRTAB, a, 1, 0, 0),
            link: None,
            data: strings.0.clone(),
            writable: false,
        },
    ];
    if hash != HashStyle::Gnu {
        out.push(Sec {
            name: ".hash",
            shdr: shdr(SHT_HASH, a, 4, 4, 0),
            link: Some(".dynsym"),
            data: super::words(&sysv),
            writable: false,
        });
    }
    if hash != HashStyle::Sysv {
        out.push(Sec {
            name: ".gnu.hash",
            shdr: shdr(SHT_GNU_HASH, a, word, 0, 0),
            link: Some(".dynsym"),
            data: gnu,
            writable: false,
        });
    }
    out.extend([
        Sec {
            name: ".gnu.version",
            shdr: shdr(SHT_GNU_VERSYM, a, 2, 2, 0),
            link: Some(".dynsym"),
            data: versym,
            writable: false,
        },
        Sec {
            name: ".gnu.version_d",
            shdr: shdr(SHT_GNU_VERDEF, a, 4, 0, 3),
            link: Some(".dynstr"),
            data: verdef,
            writable: false,
        },
        Sec {
            name: ".gnu.version_r",
            shdr: shdr(SHT_GNU_VERNEED, a, 4, 0, 2),
            link: Some(".dynstr"),
            data: verneed,
            writable: false,
        },
        Sec {
            name: ".note.gnu.property",
            shdr: shdr(SHT_NOTE, a, word, 0, 0),
            link: None,
            data: note,
            writable: false,
        },
        Sec {
            name: ".rodata",
            shdr: shdr(SHT_PROGBITS, a | SHF_EXECINSTR, 16, 0, 0),
            link: None,
            data: vec![0xc3; 16],
            writable: false,
        },
        Sec {
            name: ".tdata",
            shdr: shdr(SHT_PROGBITS, a | (SHF_WRITE | SHF_TLS), 8, 0, 0),
            link: None,
            data: vec![7; 8],
            writable: true,
        },
        Sec {
            name: ".dynamic",
            shdr: shdr(SHT_DYNAMIC, a | SHF_WRITE, word, dyn_size(class) as u64, 0),
            link: Some(".dynstr"),
            data: dynamic,
            writable: true,
        },
        Sec {
            name: ".data",
            shdr: shdr(SHT_PROGBITS, a | SHF_WRITE, 16, 0, 0),
            link: None,
            data: vec![1; 16],
            writable: true,
        },
        Sec {
            name: ".bss",
            shdr: Shdr { sh_size: 16, ..shdr(SHT_NOBITS, a | SHF_WRITE, 16, 0, 0) },
            link: None,
            data: Vec::new(),
            writable: true,
        },
    ]);
    out
}

/// `GNU` and its terminating NUL, as a note's name is stored.
const ELF_NOTE_GNU_Z: &[u8] = b"GNU\0";

/// A shared object with one of everything the reader has an accessor for:
/// two `DT_NEEDED`, a `DT_SONAME`, `DT_RPATH` and `DT_RUNPATH`, the hash
/// tables of `hash`, ten dynamic symbols (a local section symbol, an
/// undefined versioned reference, a weak reference to a weak version,
/// `foo@VER_1`, `foo@@VER_2`, an unversioned object, a hidden-version
/// symbol, a TLS symbol and a protected object in a read-only segment),
/// three version definitions and two needs, and a GNU property note. Two
/// `PT_LOAD` segments: the read-only one at address = offset, the writable
/// one [`DATA_BIAS`] above its offset.
#[allow(clippy::too_many_lines)] // the layout, then the headers
pub fn dso_sample(class: Class, e_machine: u16, hash: HashStyle) -> Dso {
    let ehsize = if class == Class::Elf32 { 52 } else { 64 };
    let phoff = ehsize;
    let mut layout = Layout::default();
    let draft = sections(class, e_machine, hash, &layout);
    layout.index = (1u32..).zip(draft.iter().map(|sec| sec.name)).map(|(i, n)| (n, i)).collect();
    // Offsets: the read-only parts after the program headers, then the
    // writable ones, then .shstrtab and the section headers.
    let mut offset = phoff + PHNUM as usize * phdr_size(class);
    let mut placed = Vec::new();
    let mut text_end = 0;
    let mut data_start = 0;
    for sec in &draft {
        let align = sec.shdr.sh_addralign.max(1) as usize;
        if sec.writable && data_start == 0 {
            text_end = offset;
            offset = offset.next_multiple_of(16);
            data_start = offset;
        }
        offset = offset.next_multiple_of(align);
        let addr = if sec.writable { offset as u64 + DATA_BIAS } else { offset as u64 };
        placed.push((offset, addr));
        layout.addr.push((sec.name, addr));
        if sec.shdr.sh_type != SHT_NOBITS {
            offset += sec.data.len();
        }
    }
    let data_end = offset;
    let secs = sections(class, e_machine, hash, &layout);
    let mut bytes = vec![0u8; data_end];
    for ((sec, first), (at, _)) in secs.iter().zip(&draft).zip(&placed) {
        assert_eq!(sec.data.len(), first.data.len(), "sizes do not depend on the layout");
        bytes[*at..*at + sec.data.len()].copy_from_slice(&sec.data);
    }
    // .shstrtab and the section headers.
    let mut names = vec![0u8];
    let mut name_at = Vec::new();
    for sec in &secs {
        name_at.push(names.len() as u32);
        names.extend_from_slice(sec.name.as_bytes());
        names.push(0);
    }
    let shstrtab_name = names.len() as u32;
    names.extend_from_slice(b".shstrtab\0");
    let shstrtab_at = bytes.len();
    bytes.extend_from_slice(&names);
    bytes.resize(bytes.len().next_multiple_of(8), 0);
    let shoff = bytes.len();
    let shnum = secs.len() as u32 + 2;
    bytes.extend_from_slice(&enc_shdr(class, &Shdr::default()));
    let mut parts = Vec::new();
    for (i, (sec, &(at, addr))) in secs.iter().zip(&placed).enumerate() {
        let size =
            if sec.shdr.sh_type == SHT_NOBITS { sec.shdr.sh_size } else { sec.data.len() as u64 };
        let shdr = Shdr {
            sh_name: name_at[i],
            sh_addr: addr,
            sh_offset: at as u64,
            sh_size: size,
            sh_link: sec.link.map_or(0, |link| layout.index(link)),
            ..sec.shdr
        };
        bytes.extend_from_slice(&enc_shdr(class, &shdr));
        parts.push(Part {
            name: sec.name,
            index: i as u32 + 1,
            offset: at,
            size: size as usize,
            addr,
        });
    }
    bytes.extend_from_slice(&enc_shdr(
        class,
        &Shdr {
            sh_name: shstrtab_name,
            sh_type: SHT_STRTAB,
            sh_offset: shstrtab_at as u64,
            sh_size: names.len() as u64,
            sh_addralign: 1,
            ..Shdr::default()
        },
    ));
    let part = |name: &str| *parts.iter().find(|p| p.name == name).expect("a part");
    let (note, tdata, dynamic, bss) =
        (part(".note.gnu.property"), part(".tdata"), part(".dynamic"), part(".bss"));
    let load = |flags, offset: usize, filesz: usize, memsz: usize, bias| Phdr {
        p_type: PT_LOAD,
        p_flags: flags,
        p_offset: offset as u64,
        p_vaddr: offset as u64 + bias,
        p_paddr: offset as u64 + bias,
        p_filesz: filesz as u64,
        p_memsz: memsz as u64,
        p_align: 0x1000,
    };
    let over = |p_type, p_flags, part: Part, filesz: usize, memsz: usize, p_align| Phdr {
        p_type,
        p_flags,
        p_offset: part.offset as u64,
        p_vaddr: part.addr,
        p_paddr: part.addr,
        p_filesz: filesz as u64,
        p_memsz: memsz as u64,
        p_align,
    };
    let word = class.word_size() as u64;
    let phdrs = [
        load(PF_R | PF_X, 0, text_end, text_end, 0),
        load(
            PF_R | PF_W,
            data_start,
            data_end - data_start,
            bss.offset + bss.size - data_start,
            DATA_BIAS,
        ),
        over(PT_DYNAMIC, PF_R | PF_W, dynamic, dynamic.size, dynamic.size, word),
        over(PT_NOTE, PF_R, note, note.size, note.size, word),
        over(PT_GNU_PROPERTY, PF_R, note, note.size, note.size, word),
        over(PT_TLS, PF_R, tdata, 8, 16, 8),
        over(
            PT_GNU_RELRO,
            PF_R,
            tdata,
            dynamic.offset + dynamic.size - tdata.offset,
            dynamic.offset + dynamic.size - tdata.offset,
            1,
        ),
        Phdr { p_type: PT_GNU_STACK, p_flags: PF_R | PF_W, p_align: 16, ..Phdr::default() },
    ];
    for (i, phdr) in phdrs.iter().enumerate() {
        let encoded = enc_phdr(class, phdr);
        let at = phoff + i * phdr_size(class);
        bytes[at..at + encoded.len()].copy_from_slice(&encoded);
    }
    let ehdr = Ehdr {
        ei_version: 1,
        e_type: ET_DYN,
        e_machine,
        e_version: 1,
        e_phoff: phoff as u64,
        e_shoff: shoff as u64,
        e_ehsize: ehsize as u16,
        e_phentsize: phdr_size(class) as u16,
        e_phnum: PHNUM as u16,
        e_shentsize: shdr_size(class) as u16,
        e_shnum: shnum as u16,
        e_shstrndx: shnum as u16 - 1,
        ..Ehdr::default()
    };
    let header = enc_ehdr(class, &ehdr);
    bytes[..header.len()].copy_from_slice(&header);
    Dso {
        class,
        bytes,
        parts,
        phoff,
        phnum: PHNUM,
        shoff,
        shnum,
        dynamic_entries: dynamic.size / dyn_size(class),
    }
}

impl Dso {
    pub fn part(&self, name: &str) -> Part {
        *self.parts.iter().find(|p| p.name == name).expect("a part")
    }

    pub fn phdr_at(&self, index: u32) -> usize {
        self.phoff + index as usize * phdr_size(self.class)
    }

    pub fn shdr_at(&self, index: u32) -> usize {
        self.shoff + index as usize * shdr_size(self.class)
    }

    /// The file offset of dynamic entry `index`.
    pub fn dyn_at(&self, index: usize) -> usize {
        self.part(".dynamic").offset + index * dyn_size(self.class)
    }

    pub fn set_phdr(&mut self, index: u32, edit: impl FnOnce(&mut Phdr)) {
        let at = self.phdr_at(index);
        let mut phdr = Phdr::decode(self.class, &self.bytes[at..]).expect("a program header");
        edit(&mut phdr);
        let encoded = enc_phdr(self.class, &phdr);
        self.bytes[at..at + encoded.len()].copy_from_slice(&encoded);
    }

    pub fn set_shdr(&mut self, index: u32, edit: impl FnOnce(&mut Shdr)) {
        let at = self.shdr_at(index);
        let mut shdr = Shdr::decode(self.class, &self.bytes[at..]).expect("a section header");
        edit(&mut shdr);
        let encoded = enc_shdr(self.class, &shdr);
        self.bytes[at..at + encoded.len()].copy_from_slice(&encoded);
    }

    pub fn set_ehdr(&mut self, edit: impl FnOnce(&mut Ehdr)) {
        let mut ehdr = FileHeader::parse(&self.bytes).expect("header").ehdr;
        edit(&mut ehdr);
        let encoded = enc_ehdr(self.class, &ehdr);
        self.bytes[..encoded.len()].copy_from_slice(&encoded);
    }

    /// Overwrites dynamic entry `index`.
    pub fn set_dyn(&mut self, index: usize, edit: impl FnOnce(&mut Dyn)) {
        let at = self.dyn_at(index);
        let mut entry = Dyn::decode(self.class, &self.bytes[at..]).expect("a dynamic entry");
        edit(&mut entry);
        let encoded = enc_dyn(self.class, &entry);
        self.bytes[at..at + encoded.len()].copy_from_slice(&encoded);
    }

    /// The index of the first dynamic entry with tag `d_tag`.
    pub fn dyn_index(&self, d_tag: i64) -> usize {
        (0..self.dynamic_entries)
            .find(|&i| {
                Dyn::decode(self.class, &self.bytes[self.dyn_at(i)..]).expect("entry").d_tag
                    == d_tag
            })
            .expect("the tag")
    }

    /// The file without its section headers ([`strip_sections`]).
    pub fn stripped(&self) -> Vec<u8> {
        strip_sections(&self.bytes)
    }
}

/// What `llvm-objcopy --strip-sections` makes of an `ET_DYN` file, enough
/// for the reader: the file cut after the last byte a program header (or
/// the header tables) covers, and no section header table.
pub fn strip_sections(data: &[u8]) -> Vec<u8> {
    let header = FileHeader::parse(data).expect("header");
    let class = header.class;
    let ehdr = header.ehdr;
    let mut end = usize::from(ehdr.e_ehsize)
        .max(ehdr.e_phoff as usize + usize::from(ehdr.e_phnum) * usize::from(ehdr.e_phentsize));
    for i in 0..usize::from(ehdr.e_phnum) {
        let at = ehdr.e_phoff as usize + i * usize::from(ehdr.e_phentsize);
        let phdr = Phdr::decode(class, &data[at..]).expect("a program header");
        end = end.max((phdr.p_offset + phdr.p_filesz) as usize);
    }
    let mut out = data[..end.min(data.len())].to_vec();
    let stripped = Ehdr { e_shoff: 0, e_shnum: 0, e_shstrndx: 0, ..ehdr };
    let encoded = enc_ehdr(class, &stripped);
    out[..encoded.len()].copy_from_slice(&encoded);
    out
}

// ---- an owned view of what the reader says -------------------------------------

/// A dynamic symbol as the reader gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymView {
    pub name: String,
    pub sym: Sym,
    /// `section_of`.
    pub section: SymbolSection,
    pub version: Option<VerView>,
}

/// The version a dynamic symbol carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerView {
    pub index: u16,
    pub hidden: bool,
    /// `L` local, `G` global, `D` defined here, `N` needed.
    pub kind: char,
    pub name: Option<String>,
    /// For a needed version, the file it is needed from.
    pub file: Option<String>,
    pub default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefView {
    pub index: u16,
    pub flags: u16,
    pub hash: u32,
    pub names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeedView {
    pub file: String,
    /// (name, index, flags, hash) of each entry.
    pub entries: Vec<(String, u16, u16, u32)>,
}

/// Everything the reader says about a shared object, owned, for comparing
/// the two entry paths and for comparing with the tools.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DsoView {
    pub segments: Vec<Phdr>,
    /// The entries up to and including the first `DT_NULL`.
    pub dynamic: Vec<(i64, u64)>,
    pub dynamic_offset: u64,
    pub needed: Vec<String>,
    pub soname: Option<String>,
    pub rpath: Option<String>,
    pub runpath: Option<String>,
    pub flags: u64,
    pub flags_1: u64,
    pub first_global: u32,
    pub symbols: Vec<SymView>,
    pub definitions: Vec<DefView>,
    pub needs: Vec<NeedView>,
    /// The program properties: (`pr_type`, data).
    pub properties: Option<Vec<(u32, Vec<u8>)>>,
}

pub fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The view of `data` found the way `entry` says (or as `parse` chooses);
/// the first error stops it.
pub fn dso_view(data: &[u8], entry: Option<Entry>) -> Result<DsoView, Error> {
    let dso = match entry {
        Some(entry) => SharedObject::parse_by(data, entry)?,
        None => SharedObject::parse(data)?,
    };
    let mut view = DsoView {
        segments: dso.program_headers().iter().map(|s| *s.header()).collect(),
        ..DsoView::default()
    };
    if let Some(table) = dso.dynamic()? {
        view.dynamic = table.iter().map(|d| (d.d_tag, d.d_val)).collect();
        view.dynamic_offset = table.offset();
        view.needed = table.needed().map(|n| n.map(text)).collect::<Result<_, _>>()?;
        view.soname = table.soname()?.map(text);
        view.rpath = table.rpath()?.map(text);
        view.runpath = table.runpath()?.map(text);
        view.flags = table.flags()?;
        view.flags_1 = table.flags_1()?;
    }
    let versions = dso.versions()?;
    if let Some(symbols) = dso.symbols()? {
        view.first_global = symbols.first_global().0;
        for (index, sym) in symbols.iter() {
            let version = match versions.of(index)? {
                None => None,
                Some(v) => Some(VerView {
                    index: v.versym.index(),
                    hidden: v.versym.hidden(),
                    kind: match v.version {
                        Version::Local => 'L',
                        Version::Global => 'G',
                        Version::Defined(_) => 'D',
                        Version::Needed { .. } => 'N',
                    },
                    name: v.name()?.map(text),
                    file: match v.version {
                        Version::Needed { need, .. } => Some(text(need.file()?)),
                        _ => None,
                    },
                    default: v.is_default(),
                }),
            };
            view.symbols.push(SymView {
                name: text(symbols.name(&sym)?),
                sym,
                section: symbols.section_of(index, &sym)?,
                version,
            });
        }
    }
    for def in dso.version_definitions()?.into_iter().flatten() {
        let def = def?;
        view.definitions.push(DefView {
            index: def.index(),
            flags: def.flags(),
            hash: def.record().vd_hash,
            names: def.names()?.map(|n| n.map(text)).collect::<Result<_, _>>()?,
        });
    }
    for need in dso.version_needs()?.into_iter().flatten() {
        let need = need?;
        let mut entries = Vec::new();
        for entry in need.entries()? {
            let entry = entry?;
            entries.push((
                text(entry.name()?),
                entry.index(),
                entry.flags(),
                entry.record().vna_hash,
            ));
        }
        view.needs.push(NeedView { file: text(need.file()?), entries });
    }
    if let Some(note) = dso.gnu_property_note()? {
        let mut properties = Vec::new();
        for property in note.gnu_properties().into_iter().flatten() {
            let property = property?;
            properties.push((property.pr_type, property.data.to_vec()));
        }
        view.properties = Some(properties);
    }
    Ok(view)
}

impl DsoView {
    /// The view as text, one fact per line, for diffs in messages.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "{self:#?}");
        out
    }
}

// ---- a walk of every accessor ------------------------------------------------

/// What a full walk of a shared object found.
#[derive(Debug, Default, Clone)]
pub struct DsoWalk {
    pub segments: u64,
    pub dynamic: u64,
    pub symbols: u64,
    pub versions: u64,
    pub definitions: u64,
    pub needs: u64,
    pub notes: u64,
    pub errors: u64,
    pub first_error: Option<Error>,
    pub steps: u64,
}

impl DsoWalk {
    fn note<T>(&mut self, result: Result<T, Error>) -> Option<T> {
        self.steps += 1;
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                assert!(!error.to_string().is_empty());
                assert!(!error.to_string().contains('\n'));
                self.errors += 1;
                self.first_error.get_or_insert(error);
                None
            }
        }
    }
}

/// Parses `data` the way `entry` says and calls every accessor on
/// everything in it, going on after errors. The result of `parse` is the
/// result; the walk never panics for a correct reader.
#[allow(clippy::too_many_lines)] // one part after another
pub fn walk_dso(data: &[u8], entry: Option<Entry>) -> Result<DsoWalk, Error> {
    let dso = match entry {
        Some(entry) => SharedObject::parse_by(data, entry)?,
        None => SharedObject::parse(data)?,
    };
    let mut walk = DsoWalk::default();
    let _ =
        (dso.class(), dso.machine(), dso.header(), dso.data(), dso.entry(), dso.section_table());
    let segments = dso.program_headers();
    assert_eq!(segments.iter().len(), segments.len() as usize);
    for segment in &segments {
        walk.segments += 1;
        let _ = (segment.index(), segment.header(), segment.flags(), segment.is_readable());
        let _ = (
            segment.is_writable(),
            segment.is_executable(),
            segment.contains(segment.header().p_vaddr),
        );
        walk.note(segment.alignment());
        walk.note(segment.file_range());
        walk.note(segment.data());
        if let Some(notes) = walk.note(segment.notes()) {
            for note in notes {
                if let Some(note) = walk.note(note) {
                    walk.notes += 1;
                    for property in note.gnu_properties().into_iter().flatten() {
                        walk.note(property);
                    }
                }
            }
        }
        let p = segment.header();
        let _ = (segments.load_containing(p.p_vaddr), segments.file_range(p.p_vaddr, p.p_filesz));
        let _ = segments.file_bytes_from(p.p_vaddr);
    }
    assert!(segments.get(segments.len()).is_err());
    for p_type in
        [PT_LOAD, PT_DYNAMIC, PT_NOTE, PT_GNU_PROPERTY, PT_TLS, PT_GNU_RELRO, PT_GNU_STACK]
    {
        walk.note(segments.unique(p_type));
    }
    if let Some(Some(note)) = walk.note(dso.gnu_property_note()) {
        for property in note.gnu_properties().into_iter().flatten() {
            walk.note(property);
        }
    }
    if let Some(Some(table)) = walk.note(dso.dynamic()) {
        let _ = (table.len(), table.is_empty(), table.offset(), table.entries());
        for entry in &table {
            walk.dynamic += 1;
            walk.note(table.value(entry.d_tag));
            let _ = table.values(entry.d_tag).count();
            if matches!(
                entry.d_tag,
                DT_NEEDED | DT_SONAME | DT_RPATH | DT_RUNPATH | DT_AUXILIARY | DT_FILTER
            ) {
                walk.note(table.string_at(entry.d_tag, entry.d_val));
            }
        }
        for name in table.needed() {
            walk.note(name);
        }
        walk.note(table.soname());
        walk.note(table.rpath());
        walk.note(table.runpath());
        walk.note(table.flags());
        walk.note(table.flags_1());
        walk.note(table.strings());
    }
    let versions = walk.note(dso.versions());
    walk.note(dso.versym());
    if let Some(Some(symbols)) = walk.note(dso.symbols()) {
        assert!(symbols.first_global().0 <= symbols.len());
        assert_eq!(symbols.locals().len() + symbols.globals().len(), symbols.len() as usize);
        for (index, sym) in symbols.iter() {
            walk.symbols += 1;
            walk.note(symbols.name(&sym));
            walk.note(symbols.section_of(index, &sym));
            if let Some(versions) = versions
                && let Some(Some(version)) = walk.note(versions.of(index))
            {
                walk.versions += 1;
                let _ = version.is_default();
                walk.note(version.name());
            }
            let _ = segments.load_containing(sym.st_value);
        }
        assert!(symbols.get(SymbolIndex(symbols.len())).is_err());
    }
    if let Some(Some(definitions)) = walk.note(dso.version_definitions()) {
        let _ = definitions.declared();
        for definition in definitions {
            let Some(definition) = walk.note(definition) else { continue };
            walk.definitions += 1;
            let _ = (
                definition.offset(),
                definition.index(),
                definition.is_base(),
                definition.is_weak(),
            );
            walk.note(definition.name());
            if let Some(names) = walk.note(definition.names()) {
                for name in names {
                    walk.note(name);
                }
            }
        }
    }
    if let Some(Some(needs)) = walk.note(dso.version_needs()) {
        for need in needs {
            let Some(need) = walk.note(need) else { continue };
            walk.needs += 1;
            let _ = need.offset();
            walk.note(need.file());
            if let Some(entries) = walk.note(need.entries()) {
                for entry in entries {
                    if let Some(entry) = walk.note(entry) {
                        let _ = (entry.offset(), entry.index(), entry.flags(), entry.is_weak());
                        walk.note(entry.name());
                    }
                }
            }
        }
    }
    Ok(walk)
}

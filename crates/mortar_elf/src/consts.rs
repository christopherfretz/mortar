//! ELF constants a linker for AArch64, x86-64 and i386 Linux needs
//! (`spec/elf.md` §2).
//!
//! Names are the ones of the gABI and the processor supplements, in upper
//! case (`SHT_GNU_verdef` is [`SHT_GNU_VERDEF`]). A constant has the width
//! of the widened field it is compared with: `sh_type` is `u32`, `sh_flags`
//! `u64`, `st_shndx` `u16`, the symbol fields `u8`, `d_tag` `i64`.
//!
//! The `*_name` functions give the specification's name of a value for
//! diagnostics, or `None` for a value this module does not name; they are
//! not a statement of support. Relocation types are in [`crate::reloc`].

/// Defines constants and the function that names their values. Values
/// must be distinct: an alias is defined outside the list.
macro_rules! named {
    (
        $(#[$fn_meta:meta])*
        fn $func:ident($ty:ty);
        $( $(#[$meta:meta])* $name:ident = $value:expr; )*
    ) => {
        $( $(#[$meta])* pub const $name: $ty = $value; )*
        $(#[$fn_meta])*
        pub fn $func(value: $ty) -> Option<&'static str> {
            match value {
                $( $name => Some(stringify!($name)), )*
                _ => None,
            }
        }
    };
}
pub(crate) use named;

// ---- e_ident (gABI "ELF Identification") ---------------------------------

/// The four magic bytes, `e_ident[EI_MAG0..=EI_MAG3]`.
pub const ELFMAG: [u8; 4] = [0x7f, b'E', b'L', b'F'];
pub const EI_CLASS: usize = 4;
pub const EI_DATA: usize = 5;
pub const EI_VERSION: usize = 6;
pub const EI_OSABI: usize = 7;
pub const EI_ABIVERSION: usize = 8;
pub const EI_NIDENT: usize = 16;

pub const ELFCLASSNONE: u8 = 0;
pub const ELFCLASS32: u8 = 1;
pub const ELFCLASS64: u8 = 2;

pub const ELFDATANONE: u8 = 0;
pub const ELFDATA2LSB: u8 = 1;
pub const ELFDATA2MSB: u8 = 2;

pub const EV_NONE: u32 = 0;
pub const EV_CURRENT: u32 = 1;

pub const ELFOSABI_NONE: u8 = 0;
pub const ELFOSABI_GNU: u8 = 3;
pub const ELFOSABI_FREEBSD: u8 = 9;
pub const ELFOSABI_STANDALONE: u8 = 255;

// ---- e_type, e_machine (gABI "ELF Header") --------------------------------

named! {
    /// The name of an `e_type` value.
    fn et_name(u16);
    ET_NONE = 0;
    ET_REL = 1;
    ET_EXEC = 2;
    ET_DYN = 3;
    ET_CORE = 4;
}
pub const ET_LOOS: u16 = 0xfe00;
pub const ET_HIOS: u16 = 0xfeff;
pub const ET_LOPROC: u16 = 0xff00;
pub const ET_HIPROC: u16 = 0xffff;

named! {
    /// The name of an `e_machine` value: the three machines mortar links
    /// and the ones a refusal is most likely to have to name.
    fn em_name(u16);
    EM_NONE = 0;
    EM_SPARC = 2;
    EM_386 = 3;
    EM_68K = 4;
    EM_MIPS = 8;
    EM_PPC = 20;
    EM_PPC64 = 21;
    EM_S390 = 22;
    EM_ARM = 40;
    EM_SPARCV9 = 43;
    EM_IA_64 = 50;
    EM_X86_64 = 62;
    EM_AARCH64 = 183;
    EM_RISCV = 243;
    EM_BPF = 247;
    EM_LOONGARCH = 258;
}

// None of the three machines defines `e_flags` bits for Linux (`EF_*`):
// the field is exposed and not interpreted.

// ---- special section indexes (gABI "Sections") ----------------------------

pub const SHN_UNDEF: u16 = 0;
pub const SHN_LORESERVE: u16 = 0xff00;
pub const SHN_LOPROC: u16 = 0xff00;
pub const SHN_HIPROC: u16 = 0xff1f;
pub const SHN_LOOS: u16 = 0xff20;
pub const SHN_HIOS: u16 = 0xff3f;
pub const SHN_ABS: u16 = 0xfff1;
pub const SHN_COMMON: u16 = 0xfff2;
pub const SHN_XINDEX: u16 = 0xffff;
pub const SHN_HIRESERVE: u16 = 0xffff;
/// x86-64 psABI "Sections": a common symbol of the large code models.
pub const SHN_X86_64_LCOMMON: u16 = 0xff02;

// ---- section types ---------------------------------------------------------

named! {
    /// The name of a section type that means the same for every machine.
    /// See [`sht_name`].
    fn sht_generic_name(u32);
    SHT_NULL = 0;
    SHT_PROGBITS = 1;
    SHT_SYMTAB = 2;
    SHT_STRTAB = 3;
    SHT_RELA = 4;
    SHT_HASH = 5;
    SHT_DYNAMIC = 6;
    SHT_NOTE = 7;
    SHT_NOBITS = 8;
    SHT_REL = 9;
    SHT_SHLIB = 10;
    SHT_DYNSYM = 11;
    SHT_INIT_ARRAY = 14;
    SHT_FINI_ARRAY = 15;
    SHT_PREINIT_ARRAY = 16;
    SHT_GROUP = 17;
    SHT_SYMTAB_SHNDX = 18;
    /// Relative relocations, compact (gABI "Relocation", `SHT_RELR`).
    SHT_RELR = 19;
    /// LLVM's compact relocation format (experimental; LLVM "CREL").
    SHT_CREL = 0x4000_0014;
    SHT_GNU_INCREMENTAL_INPUTS = 0x6fff_4700;
    SHT_LLVM_ODRTAB = 0x6fff_4c00;
    SHT_LLVM_LINKER_OPTIONS = 0x6fff_4c01;
    SHT_LLVM_ADDRSIG = 0x6fff_4c03;
    SHT_LLVM_DEPENDENT_LIBRARIES = 0x6fff_4c04;
    SHT_LLVM_SYMPART = 0x6fff_4c05;
    SHT_LLVM_PART_EHDR = 0x6fff_4c06;
    SHT_LLVM_PART_PHDR = 0x6fff_4c07;
    SHT_LLVM_BB_ADDR_MAP_V0 = 0x6fff_4c08;
    SHT_LLVM_CALL_GRAPH_PROFILE = 0x6fff_4c09;
    SHT_LLVM_BB_ADDR_MAP = 0x6fff_4c0a;
    SHT_LLVM_OFFLOADING = 0x6fff_4c0b;
    SHT_LLVM_LTO = 0x6fff_4c0c;
    SHT_LLVM_JT_SIZES = 0x6fff_4c0d;
    SHT_LLVM_CFI_JUMP_TABLE = 0x6fff_4c0e;
    SHT_GNU_SFRAME = 0x6fff_fff4;
    SHT_GNU_ATTRIBUTES = 0x6fff_fff5;
    SHT_GNU_HASH = 0x6fff_fff6;
    SHT_GNU_LIBLIST = 0x6fff_fff7;
    SHT_CHECKSUM = 0x6fff_fff8;
    /// `SHT_GNU_verdef`.
    SHT_GNU_VERDEF = 0x6fff_fffd;
    /// `SHT_GNU_verneed`.
    SHT_GNU_VERNEED = 0x6fff_fffe;
    /// `SHT_GNU_versym`.
    SHT_GNU_VERSYM = 0x6fff_ffff;
}
pub const SHT_LOOS: u32 = 0x6000_0000;
pub const SHT_HIOS: u32 = 0x6fff_ffff;
pub const SHT_LOPROC: u32 = 0x7000_0000;
pub const SHT_HIPROC: u32 = 0x7fff_ffff;
pub const SHT_LOUSER: u32 = 0x8000_0000;
pub const SHT_HIUSER: u32 = 0xffff_ffff;

/// x86-64 psABI "Sections": unwind tables (`.eh_frame`).
pub const SHT_X86_64_UNWIND: u32 = 0x7000_0001;
/// AAELF64 "Sections": build attributes.
pub const SHT_AARCH64_ATTRIBUTES: u32 = 0x7000_0003;
/// AAELF64 `PAuth` extension: signed relative relocations.
pub const SHT_AARCH64_AUTH_RELR: u32 = 0x7000_0004;
pub const SHT_AARCH64_MEMTAG_GLOBALS_STATIC: u32 = 0x7000_0007;
pub const SHT_AARCH64_MEMTAG_GLOBALS_DYNAMIC: u32 = 0x7000_0008;

/// The name of a section type in an object for `e_machine`: the generic
/// names, and the processor-specific range read for that machine.
pub fn sht_name(e_machine: u16, sh_type: u32) -> Option<&'static str> {
    if let Some(name) = sht_generic_name(sh_type) {
        return Some(name);
    }
    match (e_machine, sh_type) {
        (EM_X86_64, SHT_X86_64_UNWIND) => Some("SHT_X86_64_UNWIND"),
        (EM_AARCH64, SHT_AARCH64_ATTRIBUTES) => Some("SHT_AARCH64_ATTRIBUTES"),
        (EM_AARCH64, SHT_AARCH64_AUTH_RELR) => Some("SHT_AARCH64_AUTH_RELR"),
        (EM_AARCH64, SHT_AARCH64_MEMTAG_GLOBALS_STATIC) => {
            Some("SHT_AARCH64_MEMTAG_GLOBALS_STATIC")
        }
        (EM_AARCH64, SHT_AARCH64_MEMTAG_GLOBALS_DYNAMIC) => {
            Some("SHT_AARCH64_MEMTAG_GLOBALS_DYNAMIC")
        }
        _ => None,
    }
}

// ---- section flags ---------------------------------------------------------

pub const SHF_WRITE: u64 = 0x1;
pub const SHF_ALLOC: u64 = 0x2;
pub const SHF_EXECINSTR: u64 = 0x4;
pub const SHF_MERGE: u64 = 0x10;
pub const SHF_STRINGS: u64 = 0x20;
pub const SHF_INFO_LINK: u64 = 0x40;
pub const SHF_LINK_ORDER: u64 = 0x80;
pub const SHF_OS_NONCONFORMING: u64 = 0x100;
pub const SHF_GROUP: u64 = 0x200;
pub const SHF_TLS: u64 = 0x400;
pub const SHF_COMPRESSED: u64 = 0x800;
/// GNU: the section is kept by section garbage collection.
pub const SHF_GNU_RETAIN: u64 = 0x20_0000;
pub const SHF_GNU_MBIND: u64 = 0x100_0000;
pub const SHF_MASKOS: u64 = 0x0ff0_0000;
pub const SHF_MASKPROC: u64 = 0xf000_0000;
/// The section is left out of executables and shared objects.
pub const SHF_EXCLUDE: u64 = 0x8000_0000;
/// x86-64 psABI "Sections": the section may exceed 2 GiB (large models).
pub const SHF_X86_64_LARGE: u64 = 0x1000_0000;

// ---- section groups (gABI "Section Groups") --------------------------------

pub const GRP_COMDAT: u32 = 0x1;
pub const GRP_MASKOS: u32 = 0x0ff0_0000;
pub const GRP_MASKPROC: u32 = 0xf000_0000;

// ---- compression (gABI "Section Compression") ------------------------------

named! {
    /// The name of a `ch_type` value.
    fn elfcompress_name(u32);
    ELFCOMPRESS_ZLIB = 1;
    ELFCOMPRESS_ZSTD = 2;
}

// ---- symbols (gABI "Symbol Table") ----------------------------------------

named! {
    /// The name of a symbol binding (`st_info >> 4`).
    fn stb_name(u8);
    STB_LOCAL = 0;
    STB_GLOBAL = 1;
    STB_WEAK = 2;
    STB_GNU_UNIQUE = 10;
}
pub const STB_LOOS: u8 = 10;
pub const STB_HIOS: u8 = 12;
pub const STB_LOPROC: u8 = 13;
pub const STB_HIPROC: u8 = 15;

named! {
    /// The name of a symbol type (`st_info & 0xf`).
    fn stt_name(u8);
    STT_NOTYPE = 0;
    STT_OBJECT = 1;
    STT_FUNC = 2;
    STT_SECTION = 3;
    STT_FILE = 4;
    STT_COMMON = 5;
    STT_TLS = 6;
    STT_GNU_IFUNC = 10;
}
pub const STT_LOOS: u8 = 10;
pub const STT_HIOS: u8 = 12;
pub const STT_LOPROC: u8 = 13;
pub const STT_HIPROC: u8 = 15;

named! {
    /// The name of a symbol visibility (`st_other & 3`).
    fn stv_name(u8);
    STV_DEFAULT = 0;
    STV_INTERNAL = 1;
    STV_HIDDEN = 2;
    STV_PROTECTED = 3;
}
/// The bits of `st_other` that hold the visibility.
pub const STV_MASK: u8 = 0x3;
/// AAELF64 "Symbol Table": the function may not follow the base procedure
/// call standard's register conventions.
pub const STO_AARCH64_VARIANT_PCS: u8 = 0x80;

/// The undefined symbol index.
pub const STN_UNDEF: u32 = 0;

// ---- notes -----------------------------------------------------------------

/// The owner of GNU notes, as stored (`n_namesz` is 4).
pub const ELF_NOTE_GNU: &[u8] = b"GNU";

pub const NT_GNU_ABI_TAG: u32 = 1;
pub const NT_GNU_HWCAP: u32 = 2;
pub const NT_GNU_BUILD_ID: u32 = 3;
pub const NT_GNU_GOLD_VERSION: u32 = 4;
pub const NT_GNU_PROPERTY_TYPE_0: u32 = 5;

/// The name of a note type of the owner `GNU`.
pub fn nt_gnu_name(n_type: u32) -> Option<&'static str> {
    match n_type {
        NT_GNU_ABI_TAG => Some("NT_GNU_ABI_TAG"),
        NT_GNU_HWCAP => Some("NT_GNU_HWCAP"),
        NT_GNU_BUILD_ID => Some("NT_GNU_BUILD_ID"),
        NT_GNU_GOLD_VERSION => Some("NT_GNU_GOLD_VERSION"),
        NT_GNU_PROPERTY_TYPE_0 => Some("NT_GNU_PROPERTY_TYPE_0"),
        _ => None,
    }
}

// Program properties (Linux gABI extensions, "Program Property").
pub const GNU_PROPERTY_STACK_SIZE: u32 = 1;
pub const GNU_PROPERTY_NO_COPY_ON_PROTECTED: u32 = 2;
pub const GNU_PROPERTY_UINT32_AND_LO: u32 = 0xb000_0000;
pub const GNU_PROPERTY_UINT32_AND_HI: u32 = 0xb000_7fff;
pub const GNU_PROPERTY_UINT32_OR_LO: u32 = 0xb000_8000;
pub const GNU_PROPERTY_UINT32_OR_HI: u32 = 0xb000_ffff;
pub const GNU_PROPERTY_1_NEEDED: u32 = GNU_PROPERTY_UINT32_OR_LO;
pub const GNU_PROPERTY_1_NEEDED_INDIRECT_EXTERN_ACCESS: u32 = 1;
pub const GNU_PROPERTY_LOPROC: u32 = 0xc000_0000;
pub const GNU_PROPERTY_HIPROC: u32 = 0xdfff_ffff;
pub const GNU_PROPERTY_LOUSER: u32 = 0xe000_0000;
pub const GNU_PROPERTY_HIUSER: u32 = 0xffff_ffff;

// AAELF64 "Program Property".
pub const GNU_PROPERTY_AARCH64_FEATURE_1_AND: u32 = 0xc000_0000;
pub const GNU_PROPERTY_AARCH64_FEATURE_1_BTI: u32 = 1;
pub const GNU_PROPERTY_AARCH64_FEATURE_1_PAC: u32 = 2;
pub const GNU_PROPERTY_AARCH64_FEATURE_1_GCS: u32 = 4;
pub const GNU_PROPERTY_AARCH64_FEATURE_PAUTH: u32 = 0xc000_0001;

// x86-64 psABI and i386 psABI "Program Property" (the two share the
// numbering).
pub const GNU_PROPERTY_X86_COMPAT_ISA_1_USED: u32 = 0xc000_0000;
pub const GNU_PROPERTY_X86_COMPAT_ISA_1_NEEDED: u32 = 0xc000_0001;
pub const GNU_PROPERTY_X86_UINT32_AND_LO: u32 = 0xc000_0002;
pub const GNU_PROPERTY_X86_UINT32_AND_HI: u32 = 0xc000_7fff;
pub const GNU_PROPERTY_X86_UINT32_OR_LO: u32 = 0xc000_8000;
pub const GNU_PROPERTY_X86_UINT32_OR_HI: u32 = 0xc000_ffff;
pub const GNU_PROPERTY_X86_UINT32_OR_AND_LO: u32 = 0xc001_0000;
pub const GNU_PROPERTY_X86_UINT32_OR_AND_HI: u32 = 0xc001_7fff;
pub const GNU_PROPERTY_X86_FEATURE_1_AND: u32 = 0xc000_0002;
pub const GNU_PROPERTY_X86_FEATURE_1_IBT: u32 = 1;
pub const GNU_PROPERTY_X86_FEATURE_1_SHSTK: u32 = 2;
pub const GNU_PROPERTY_X86_FEATURE_1_LAM_U48: u32 = 4;
pub const GNU_PROPERTY_X86_FEATURE_1_LAM_U57: u32 = 8;
pub const GNU_PROPERTY_X86_COMPAT_2_ISA_1_NEEDED: u32 = 0xc000_8000;
pub const GNU_PROPERTY_X86_FEATURE_2_NEEDED: u32 = 0xc000_8001;
pub const GNU_PROPERTY_X86_ISA_1_NEEDED: u32 = 0xc000_8002;
pub const GNU_PROPERTY_X86_COMPAT_2_ISA_1_USED: u32 = 0xc001_0000;
pub const GNU_PROPERTY_X86_FEATURE_2_USED: u32 = 0xc001_0001;
pub const GNU_PROPERTY_X86_ISA_1_USED: u32 = 0xc001_0002;
pub const GNU_PROPERTY_X86_ISA_1_BASELINE: u32 = 1;
pub const GNU_PROPERTY_X86_ISA_1_V2: u32 = 2;
pub const GNU_PROPERTY_X86_ISA_1_V3: u32 = 4;
pub const GNU_PROPERTY_X86_ISA_1_V4: u32 = 8;

// ---- program headers (gABI "Program Header"); for the shared-object reader --

named! {
    /// The name of a segment type that means the same for every machine.
    fn pt_name(u32);
    PT_NULL = 0;
    PT_LOAD = 1;
    PT_DYNAMIC = 2;
    PT_INTERP = 3;
    PT_NOTE = 4;
    PT_SHLIB = 5;
    PT_PHDR = 6;
    PT_TLS = 7;
    PT_GNU_EH_FRAME = 0x6474_e550;
    PT_GNU_STACK = 0x6474_e551;
    PT_GNU_RELRO = 0x6474_e552;
    PT_GNU_PROPERTY = 0x6474_e553;
    PT_GNU_SFRAME = 0x6474_e554;
}
pub const PT_LOOS: u32 = 0x6000_0000;
pub const PT_HIOS: u32 = 0x6fff_ffff;
pub const PT_LOPROC: u32 = 0x7000_0000;
pub const PT_HIPROC: u32 = 0x7fff_ffff;
pub const PT_AARCH64_MEMTAG_MTE: u32 = 0x7000_0002;

pub const PF_X: u32 = 0x1;
pub const PF_W: u32 = 0x2;
pub const PF_R: u32 = 0x4;

/// `e_phnum` when the count is in `sh_info` of section header 0.
pub const PN_XNUM: u16 = 0xffff;

// ---- the dynamic table (gABI "Dynamic Section") ---------------------------

named! {
    /// The name of a dynamic tag that means the same for every machine.
    fn dt_name(i64);
    DT_NULL = 0;
    DT_NEEDED = 1;
    DT_PLTRELSZ = 2;
    DT_PLTGOT = 3;
    DT_HASH = 4;
    DT_STRTAB = 5;
    DT_SYMTAB = 6;
    DT_RELA = 7;
    DT_RELASZ = 8;
    DT_RELAENT = 9;
    DT_STRSZ = 10;
    DT_SYMENT = 11;
    DT_INIT = 12;
    DT_FINI = 13;
    DT_SONAME = 14;
    DT_RPATH = 15;
    DT_SYMBOLIC = 16;
    DT_REL = 17;
    DT_RELSZ = 18;
    DT_RELENT = 19;
    DT_PLTREL = 20;
    DT_DEBUG = 21;
    DT_TEXTREL = 22;
    DT_JMPREL = 23;
    DT_BIND_NOW = 24;
    DT_INIT_ARRAY = 25;
    DT_FINI_ARRAY = 26;
    DT_INIT_ARRAYSZ = 27;
    DT_FINI_ARRAYSZ = 28;
    DT_RUNPATH = 29;
    DT_FLAGS = 30;
    DT_PREINIT_ARRAY = 32;
    DT_PREINIT_ARRAYSZ = 33;
    DT_SYMTAB_SHNDX = 34;
    DT_RELRSZ = 35;
    DT_RELR = 36;
    DT_RELRENT = 37;
    DT_GNU_HASH = 0x6fff_fef5;
    DT_TLSDESC_PLT = 0x6fff_fef6;
    DT_TLSDESC_GOT = 0x6fff_fef7;
    DT_VERSYM = 0x6fff_fff0;
    DT_RELACOUNT = 0x6fff_fff9;
    DT_RELCOUNT = 0x6fff_fffa;
    DT_FLAGS_1 = 0x6fff_fffb;
    DT_VERDEF = 0x6fff_fffc;
    DT_VERDEFNUM = 0x6fff_fffd;
    DT_VERNEED = 0x6fff_fffe;
    DT_VERNEEDNUM = 0x6fff_ffff;
    DT_AUXILIARY = 0x7fff_fffd;
    DT_FILTER = 0x7fff_ffff;
}
pub const DT_LOOS: i64 = 0x6000_000d;
pub const DT_HIOS: i64 = 0x6fff_f000;
pub const DT_LOPROC: i64 = 0x7000_0000;
pub const DT_HIPROC: i64 = 0x7fff_ffff;
// AAELF64 "Dynamic Section".
pub const DT_AARCH64_BTI_PLT: i64 = 0x7000_0001;
pub const DT_AARCH64_PAC_PLT: i64 = 0x7000_0003;
pub const DT_AARCH64_VARIANT_PCS: i64 = 0x7000_0005;
pub const DT_AARCH64_MEMTAG_MODE: i64 = 0x7000_0009;
pub const DT_AARCH64_MEMTAG_HEAP: i64 = 0x7000_000b;
pub const DT_AARCH64_MEMTAG_STACK: i64 = 0x7000_000c;
pub const DT_AARCH64_MEMTAG_GLOBALS: i64 = 0x7000_000d;
pub const DT_AARCH64_MEMTAG_GLOBALSSZ: i64 = 0x7000_000f;
pub const DT_AARCH64_AUTH_RELRSZ: i64 = 0x7000_0011;
pub const DT_AARCH64_AUTH_RELR: i64 = 0x7000_0012;
pub const DT_AARCH64_AUTH_RELRENT: i64 = 0x7000_0013;
// x86-64 psABI "Dynamic Section".
pub const DT_X86_64_PLT: i64 = 0x7000_0000;
pub const DT_X86_64_PLTSZ: i64 = 0x7000_0001;
pub const DT_X86_64_PLTENT: i64 = 0x7000_0003;

// DT_FLAGS.
pub const DF_ORIGIN: u64 = 0x1;
pub const DF_SYMBOLIC: u64 = 0x2;
pub const DF_TEXTREL: u64 = 0x4;
pub const DF_BIND_NOW: u64 = 0x8;
pub const DF_STATIC_TLS: u64 = 0x10;
// DT_FLAGS_1.
pub const DF_1_NOW: u64 = 0x1;
pub const DF_1_GLOBAL: u64 = 0x2;
pub const DF_1_GROUP: u64 = 0x4;
pub const DF_1_NODELETE: u64 = 0x8;
pub const DF_1_LOADFLTR: u64 = 0x10;
pub const DF_1_INITFIRST: u64 = 0x20;
pub const DF_1_NOOPEN: u64 = 0x40;
pub const DF_1_ORIGIN: u64 = 0x80;
pub const DF_1_DIRECT: u64 = 0x100;
pub const DF_1_INTERPOSE: u64 = 0x400;
pub const DF_1_NODEFLIB: u64 = 0x800;
pub const DF_1_NODUMP: u64 = 0x1000;
pub const DF_1_SYMINTPOSE: u64 = 0x80_0000;
pub const DF_1_PIE: u64 = 0x800_0000;

// ---- symbol versions (LSB "Symbol Versioning") ----------------------------

pub const VER_DEF_CURRENT: u16 = 1;
pub const VER_NEED_CURRENT: u16 = 1;
pub const VER_FLG_BASE: u16 = 0x1;
pub const VER_FLG_WEAK: u16 = 0x2;
pub const VER_FLG_INFO: u16 = 0x4;
pub const VER_NDX_LOCAL: u16 = 0;
pub const VER_NDX_GLOBAL: u16 = 1;
/// The bit of a `versym` entry that hides the symbol from default lookup.
pub const VERSYM_HIDDEN: u16 = 0x8000;
/// The bits of a `versym` entry that hold the version index.
pub const VERSYM_VERSION: u16 = 0x7fff;

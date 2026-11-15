//! Ranks (`spec/layout.md` §10, L-60, L-61): the position class of an
//! output section or a synthetic chunk, and its order inside the rank.

use mortar_elf::consts::{SHF_ALLOC, SHF_EXECINSTR, SHF_TLS, SHF_WRITE, SHT_NOBITS, SHT_NOTE};

/// A position: `(rank, within)`; chunks sort by it, then by `OutSecId`
/// (L-60).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Rank {
    pub rank: u8,
    pub within: u8,
}

impl Rank {
    const fn new(rank: u8, within: u8) -> Self {
        Self { rank, within }
    }
    /// The file and program headers.
    pub const HEADERS: Self = Self::new(0, 0);
    /// The RELRO padding: last of rank 7 (L-64).
    pub const RELRO_PADDING: Self = Self::new(7, 255);
    /// The section header table (L-63).
    pub const SECTION_HEADERS: Self = Self::new(11, 0);

    /// Whether chunks of this rank are in the RELRO run (rank 7).
    #[must_use]
    pub const fn is_relro(self) -> bool {
        self.rank == 7
    }
}

/// The rank-table class of a set of flags and a type: which ranks an
/// orphan of these flags may go to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    NonAlloc,
    Note,
    Ro,
    RoNobits,
    Exec,
    Tls,
    TlsNobits,
    Rw,
    RwNobits,
}

fn class(sh_type: u32, flags: u64) -> Class {
    let nobits = sh_type == SHT_NOBITS;
    if flags & SHF_ALLOC == 0 {
        Class::NonAlloc
    } else if flags & SHF_TLS != 0 {
        if nobits { Class::TlsNobits } else { Class::Tls }
    } else if flags & SHF_WRITE != 0 {
        if nobits { Class::RwNobits } else { Class::Rw }
    } else if flags & SHF_EXECINSTR != 0 {
        Class::Exec
    } else if sh_type == SHT_NOTE {
        Class::Note
    } else if nobits {
        Class::RoNobits
    } else {
        Class::Ro
    }
}

/// The named rows of the rank table: name, rank, within, and the class the
/// name's flags must have for the row to apply.
const NAMED: [(&[u8], u8, u8, Class); 30] = [
    (b".note.gnu.property", 2, 0, Class::Note),
    (b".note.gnu.build-id", 2, 1, Class::Note),
    (b".note.ABI-tag", 2, 2, Class::Note),
    (b".rela.dyn", 4, 0, Class::Ro),
    (b".rel.dyn", 4, 0, Class::Ro),
    (b".relr.dyn", 4, 1, Class::Ro),
    (b".rela.plt", 4, 2, Class::Ro),
    (b".rel.plt", 4, 2, Class::Ro),
    (b".rodata", 5, 0, Class::Ro),
    (b".gcc_except_table", 5, 2, Class::Ro),
    (b".eh_frame_hdr", 5, 3, Class::Ro),
    (b".eh_frame", 5, 4, Class::Ro),
    (b".init", 6, 0, Class::Exec),
    (b".plt", 6, 1, Class::Exec),
    (b".iplt", 6, 1, Class::Exec),
    (b".text", 6, 2, Class::Exec),
    (b".fini", 6, 4, Class::Exec),
    (b".tdata", 7, 0, Class::Tls),
    (b".tbss", 7, 2, Class::TlsNobits),
    (b".preinit_array", 7, 4, Class::Rw),
    (b".init_array", 7, 5, Class::Rw),
    (b".fini_array", 7, 6, Class::Rw),
    (b".data.rel.ro", 7, 7, Class::Rw),
    (b".dynamic", 7, 8, Class::Rw),
    (b".got", 7, 9, Class::Rw),
    (b".bss.rel.ro", 7, 11, Class::RwNobits),
    (b".got.plt", 8, 0, Class::Rw),
    (b".data", 8, 1, Class::Rw),
    (b".tm_clone_table", 8, 2, Class::Rw),
    (b".bss", 8, 4, Class::RwNobits),
];

/// The non-allocated sections of rank 10 (L-63).
const SYMBOL_TABLES: [&[u8]; 4] = [b".symtab", b".symtab_shndx", b".strtab", b".shstrtab"];

/// The rank of an output section or synthetic chunk by name, type and
/// flags (L-60, L-61). A named row applies only when the flags have the
/// row's class (a writable `.rodata` is a writable orphan), so a chunk's
/// rank never contradicts its segment.
#[must_use]
pub fn rank_of(name: &[u8], sh_type: u32, flags: u64) -> Rank {
    let c = class(sh_type, flags);
    if c == Class::NonAlloc {
        return match SYMBOL_TABLES.iter().position(|&n| n == name) {
            Some(i) => Rank::new(10, i as u8),
            None => Rank::new(9, 0),
        };
    }
    for (n, rank, within, want) in NAMED {
        if n == name && want == c {
            return Rank::new(rank, within);
        }
    }
    match c {
        Class::NonAlloc => Rank::new(9, 0),
        Class::Note => Rank::new(2, 3),
        Class::Ro => Rank::new(5, 1),
        Class::RoNobits => Rank::new(5, 5),
        Class::Exec => Rank::new(6, 3),
        Class::Tls => Rank::new(7, 1),
        Class::TlsNobits => Rank::new(7, 3),
        Class::Rw => Rank::new(8, 3),
        Class::RwNobits => Rank::new(8, 5),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mortar_elf::consts::{SHT_INIT_ARRAY, SHT_PROGBITS};

    const A: u64 = SHF_ALLOC;
    const AX: u64 = SHF_ALLOC | SHF_EXECINSTR;
    const WA: u64 = SHF_ALLOC | SHF_WRITE;
    const WAT: u64 = WA | SHF_TLS;

    // spec: layout.md §10 L-60 (the rank table), L-61 (RO NOBITS orphans end rank 5)
    #[test]
    fn l60_rank_table() {
        let r = |n: &[u8], t, f| rank_of(n, t, f);
        let order = [
            r(b".note.gnu.property", SHT_NOTE, A),
            r(b".note.ABI-tag", SHT_NOTE, A),
            r(b".note.stapsdt", SHT_NOTE, A),
            r(b".rela.plt", 4, A),
            r(b".rodata", SHT_PROGBITS, A),
            r(b"ro_orphan", SHT_PROGBITS, A),
            r(b".gcc_except_table", SHT_PROGBITS, A),
            r(b".eh_frame", SHT_PROGBITS, A),
            r(b"ro_nobits", SHT_NOBITS, A),
            r(b".init", SHT_PROGBITS, AX),
            r(b".iplt", SHT_PROGBITS, AX),
            r(b".text", SHT_PROGBITS, AX),
            r(b"exec_orphan", SHT_PROGBITS, AX),
            r(b".fini", SHT_PROGBITS, AX),
            r(b".tdata", SHT_PROGBITS, WAT),
            r(b"tls_orphan", SHT_PROGBITS, WAT),
            r(b".tbss", SHT_NOBITS, WAT),
            r(b"tls_nobits", SHT_NOBITS, WAT),
            r(b".preinit_array", 16, WA),
            r(b".init_array", SHT_INIT_ARRAY, WA),
            r(b".fini_array", 15, WA),
            r(b".data.rel.ro", SHT_PROGBITS, WA),
            r(b".got", SHT_PROGBITS, WA),
            Rank::RELRO_PADDING,
            r(b".got.plt", SHT_PROGBITS, WA),
            r(b".data", SHT_PROGBITS, WA),
            r(b".tm_clone_table", SHT_PROGBITS, WA),
            r(b"rw_orphan", SHT_PROGBITS, WA),
            r(b".bss", SHT_NOBITS, WA),
            r(b"rw_nobits", SHT_NOBITS, WA),
            r(b".comment", SHT_PROGBITS, 0),
            r(b".debug_info", SHT_PROGBITS, 0),
            r(b".symtab", 2, 0),
            r(b".strtab", 3, 0),
            r(b".shstrtab", 3, 0),
            Rank::SECTION_HEADERS,
        ];
        assert!(order.windows(2).all(|w| w[0] <= w[1]), "{order:?}");
        assert!(Rank::HEADERS < order[0]);
        assert!(r(b".tdata", SHT_PROGBITS, WAT).is_relro());
        assert!(!r(b".data", SHT_PROGBITS, WA).is_relro());
    }

    // spec: layout.md §10 L-60 (a named row applies only with its class)
    #[test]
    fn l60_name_needs_class() {
        assert_eq!(rank_of(b".rodata", SHT_PROGBITS, WA), rank_of(b"x", SHT_PROGBITS, WA));
        assert_eq!(rank_of(b".data", SHT_PROGBITS, A), rank_of(b"x", SHT_PROGBITS, A));
        assert_eq!(rank_of(b".bss", SHT_PROGBITS, WA), rank_of(b"x", SHT_PROGBITS, WA));
    }
}

//! Ranks (`spec/layout.md` §10, L-60, L-61): the position class of an
//! output section or a synthetic chunk, and its order inside the rank.

use mortar_elf::consts::{
    SHF_ALLOC, SHF_EXECINSTR, SHF_TLS, SHF_WRITE, SHF_X86_64_LARGE, SHT_NOBITS, SHT_NOTE,
};

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
const NAMED: [(&[u8], u8, u8, Class); 38] = [
    (b".interp", 1, 0, Class::Ro),
    (b".note.gnu.property", 2, 0, Class::Note),
    (b".note.gnu.build-id", 2, 1, Class::Note),
    (b".note.ABI-tag", 2, 2, Class::Note),
    (b".gnu.hash", 3, 0, Class::Ro),
    (b".dynsym", 3, 1, Class::Ro),
    (b".dynstr", 3, 2, Class::Ro),
    (b".gnu.version", 3, 3, Class::Ro),
    (b".gnu.version_d", 3, 4, Class::Ro),
    (b".gnu.version_r", 3, 5, Class::Ro),
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
    (b".dynbss", 8, 4, Class::RwNobits),
];

/// The output section a synthetic chunk joins when the output has one of
/// that name (`spec/dyn.md` D-42, D-60): `.dynbss` is the first chunk of
/// `.bss`; every other synthetic chunk joins the section of its own name
/// (`.eh_frame`, `.bss.rel.ro`).
#[must_use]
pub fn home(name: &[u8]) -> &[u8] {
    if name == b".dynbss" { b".bss" } else { name }
}

/// Whether a synthetic chunk leads its output section: the copy chunks
/// come before the inputs of the section they join (D-60: `.bss` "with
/// `.dynbss` first"; L-60 rank 7: the copy chunk of `.bss.rel.ro`, "then
/// input `.bss.rel.ro`"). Every other joining chunk follows the inputs.
#[must_use]
pub fn leads(name: &[u8]) -> bool {
    name == b".dynbss" || name == b".bss.rel.ro"
}

/// The options the rank of a name depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Z {
    /// `-z now`: `.got.plt` is RELRO, after `.got` (L-60 rank 7).
    pub now: bool,
    /// `-z relro` (the default). Under `-z norelro`, `.bss.rel.ro` is
    /// "placed as an ordinary writable section" (D-42, L-80): ranked as a
    /// writable `NOBITS` orphan (L-48), after `.bss`, where lld 19 puts it
    /// (measured, WP-049). Left in rank 7 it would be a `NOBITS` chunk
    /// before the `PROGBITS` of rank 8 in one `PT_LOAD` (L-69).
    pub relro: bool,
}

/// The `within` of `.lrodata` in rank 8 (L-36a): after every small rank-8
/// row (the largest is 5, writable `NOBITS` orphans).
const LARGE_WITHIN: u8 = 16;

/// The non-allocated sections of rank 10 (L-63).
const SYMBOL_TABLES: [&[u8]; 4] = [b".symtab", b".symtab_shndx", b".strtab", b".shstrtab"];

/// The rank of an output section or synthetic chunk by name, type and
/// flags (L-60, L-61). A named row applies only when the flags have the
/// row's class (a writable `.rodata` is a writable orphan), so a chunk's
/// rank never contradicts its segment.
#[must_use]
pub fn rank_of(name: &[u8], sh_type: u32, flags: u64) -> Rank {
    rank_with(name, sh_type, flags, false)
}

/// [`rank_of`] under `-z now` when `now`: `.got.plt` is then RELRO, after
/// `.got` (L-60's table; GNU ld, lld and mold all do this in a static
/// link: nothing is bound lazily).
#[must_use]
pub fn rank_with(name: &[u8], sh_type: u32, flags: u64, now: bool) -> Rank {
    rank_in(name, sh_type, flags, Z { now, relro: true })
}

/// [`rank_of`] under the options `z` (`rank_with`, and `.bss.rel.ro`
/// under `-z norelro`).
///
/// An x86-64 large output section (`SHF_X86_64_LARGE`, which layout keeps
/// only on x86-64) ends the allocated order, after every chunk of rank 8:
/// `.lrodata`, `.ltext`, `.ldata`, `.lbss` (L-36a), each class its own
/// `PT_LOAD` (`LoadClass::LargeR`, `LargeRx`, `LargeRw`).
#[must_use]
pub fn rank_in(name: &[u8], sh_type: u32, flags: u64, z: Z) -> Rank {
    let c = class(sh_type, flags);
    if flags & SHF_X86_64_LARGE != 0 {
        match c {
            Class::Ro | Class::RoNobits => return Rank::new(8, LARGE_WITHIN),
            Class::Exec => return Rank::new(8, LARGE_WITHIN + 1),
            Class::Rw => return Rank::new(8, LARGE_WITHIN + 2),
            Class::RwNobits => return Rank::new(8, LARGE_WITHIN + 3),
            // Refused before layout ranks them (L-36: TLS, non-alloc).
            Class::NonAlloc | Class::Note | Class::Tls | Class::TlsNobits => {}
        }
    }
    if z.now && name == b".got.plt" && c == Class::Rw {
        return Rank::new(7, 10);
    }
    if !z.relro && name == b".bss.rel.ro" && c == Class::RwNobits {
        return Rank::new(8, 5);
    }
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

    // spec: layout.md §8.2 L-36a (the large sections after every small alloc section, in order)
    #[test]
    fn l36a_large_sections_end_the_alloc_order() {
        const L: u64 = SHF_X86_64_LARGE;
        let order = [
            rank_of(b".bss", SHT_NOBITS, WA),
            rank_of(b"rw_nobits", SHT_NOBITS, WA),
            rank_of(b".lrodata", SHT_PROGBITS, A | L),
            rank_of(b".ltext", SHT_PROGBITS, AX | L),
            rank_of(b".ldata", SHT_PROGBITS, WA | L),
            rank_of(b".lbss", SHT_NOBITS, WA | L),
            rank_of(b".comment", SHT_PROGBITS, 0),
        ];
        assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");
        // By flags, whatever the name; never RELRO.
        assert_eq!(rank_of(b"x", SHT_PROGBITS, WA | L), order[4]);
        assert!(!order[4].is_relro());
        assert_eq!(rank_with(b".ldata", SHT_PROGBITS, WA | L, true), order[4]);
    }

    // spec: layout.md §10 L-60 (a named row applies only with its class)
    #[test]
    fn l60_name_needs_class() {
        assert_eq!(rank_of(b".rodata", SHT_PROGBITS, WA), rank_of(b"x", SHT_PROGBITS, WA));
        assert_eq!(rank_of(b".data", SHT_PROGBITS, A), rank_of(b"x", SHT_PROGBITS, A));
        assert_eq!(rank_of(b".bss", SHT_PROGBITS, WA), rank_of(b"x", SHT_PROGBITS, WA));
    }
}

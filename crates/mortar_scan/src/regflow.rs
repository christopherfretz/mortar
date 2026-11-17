//! The AArch64 analysis of `spec/dyn.md` D-117 ("Bound use through a
//! register", items 1-9; M4, WP-119): bound uses of exported preemptible
//! data that non-PIC code forms in a register and reaches through an
//! instruction immediate (`adrp x0, .data; add x0, x0, :lo12:.data; ldr
//! w1, [x0, #8]`), which no relocation names.
//!
//! [`analyse`] is a pure function of one relocatable object (its
//! sections, symbols and relocations) and of the symbols of that object
//! whose definitions the caller's D-112 makes preemptible. It reads
//! nothing else, writes nothing, and its result does not depend on the
//! order anything is visited in. Its oracle is
//! `tests/shared-design/exp3/s1d.py` (record `spec/landed/dyn/WP-110.md`
//! §G.1); where the oracle and the spec text differ, this follows the
//! text (record `spec/landed/dyn/WP-119.md` §3).
//!
//! # The join (WP-123)
//!
//! For a `Shared` output, per live `EM_AARCH64` relocatable object O:
//!
//! 1. `preemptible`: the indexes of O's symbol-table entries that are
//!    definitions whose key `Resolution.preemptible` holds (D-112). The
//!    analysis keeps those defined in a section of O without
//!    `SHF_EXECINSTR` (`STT_TLS` included): D-117's **preemptible data
//!    definitions**. Functions, commons and absolute symbols are dropped
//!    here, so the caller may pass every preemptible definition.
//! 2. `contents`: the bytes of a section of O, `sh_size` of them as in
//!    the file (`Walker::section_bytes` gives them: resident, or copied).
//!    The analysis asks for the relocation sections whose target is
//!    `SHF_ALLOC|SHF_EXECINSTR` (the pre-filter), and only when the
//!    pre-filter passes for the code of O's functions and for the
//!    relocation sections of the sections it reads literal words from;
//!    every relocation section only when a boundary address is met and
//!    "self-GOT" must be known.
//! 3. A non-empty [`Analysis::uses`] refuses the link:
//!    [`Analysis::diagnostics`] gives one `non-pic-in-shared-object`
//!    error per bound use (`spec/diag.md` §3.1, §3.2).
//!
//! The address-level rule (relocations whose own symbol and addend name
//! an address inside a preemptible data definition) is not here: it is
//! WP-117's `bound.rs`. The two are independent; a link is refused when
//! either finds a use.
//!
//! # Malformed input
//!
//! Every byte is untrusted and nothing here panics on any input. A word
//! that does not decode as one of the table's forms is the table's
//! "unallocated" or "any other word of the load/store group": it kills
//! every register (item 5). Code past the end of its section's contents
//! is not analysed: such a word could only kill every register, and the
//! words before it are judged exactly as they would be (no word has it
//! as a predecessor that matters to them). A symbol or relocation table
//! that does not parse is a [`Fault::Malformed`], whose text the caller
//! reports as the scan reports a malformed file; an `SHT_REL` section of
//! an AArch64 object is not read (AArch64 objects use `SHT_RELA`; the
//! scan refuses `SHT_REL` by name, `mortar_a64`).

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use mortar_diag::{Diagnostic, FileRef, Kind, Place};
use mortar_elf::consts::{
    EM_AARCH64, SHF_ALLOC, SHF_EXECINSTR, SHT_NOBITS, SHT_RELA, STB_GLOBAL, STB_GNU_UNIQUE,
    STB_LOCAL, STB_WEAK, STT_FUNC, STV_DEFAULT,
};
use mortar_elf::{Object, Rela, Section, SectionIndex, SymbolIndex, SymbolSection, SymbolTable};

/// `R_AARCH64_ABS64`: the literal word of item 4.
const R_ABS64: u32 = 257;
/// `R_AARCH64_LD_PREL_LO19`.
const R_LD_PREL_LO19: u32 = 273;
/// `R_AARCH64_ADR_PREL_LO21`.
const R_ADR_PREL_LO21: u32 = 274;
/// `R_AARCH64_ADD_ABS_LO12_NC`.
const R_ADD_ABS_LO12_NC: u32 = 277;
/// `R_AARCH64_LDST64_ABS_LO12_NC`.
const R_LDST64_ABS_LO12_NC: u32 = 286;

/// D-117's GOT forms on AArch64 (the boundary rule's "self-GOT"):
/// `MOVW_GOTOFF_G0`..`G3` (300-306), `GOT_LD_PREL19` (309),
/// `LD64_GOTOFF_LO15` (310), `ADR_GOT_PAGE` (311), `LD64_GOT_LO12_NC`
/// (312), `LD64_GOTPAGE_LO15` (313).
fn is_got_form(r_type: u32) -> bool {
    matches!(r_type, 300..=306 | 309..=313)
}

/// Why the analysis could not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fault<E> {
    /// A table of the object does not parse (the text says which).
    Malformed(String),
    /// The caller's `contents` failed for a section.
    Contents(SectionIndex, E),
}

/// An address the analysis tracks: an input offset `a` of section `T`,
/// a section holding a preemptible data definition (item 3). `a` is
/// signed and wide: a computed address may lie before its section, and
/// no sum of 64-bit offsets and 24-bit immediates along one path
/// overflows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Address {
    pub section: SectionIndex,
    pub offset: i128,
}

/// A relocation that formed a tracked address (item 4): the defining
/// form's own relocation, or for a literal form the `ABS64` relocation
/// of the literal word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    /// The section the relocation applies to, and its `r_offset`.
    pub section: SectionIndex,
    pub offset: u64,
    pub r_type: u32,
    /// The relocation's symbol (a local symbol defined in `address.section`)
    /// and addend.
    pub symbol: SymbolIndex,
    pub addend: i64,
    /// The address it names, `y.st_value + A`.
    pub address: Address,
}

/// How a judged address was reached (items 6 and 9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Reach {
    /// `ADD`/`SUB` (immediate) from a tracked register: an address
    /// computed, not accessed.
    Computed,
    /// The access address of a load or store whose base is tracked (the
    /// boundary rule's "access").
    Access,
    /// A pre- or post-index write-back of a tracked base: computed.
    WriteBack,
}

impl Reach {
    /// Whether this is an access at the address (the boundary rule).
    pub fn is_access(self) -> bool {
        matches!(self, Reach::Access)
    }
}

/// One bound use the analysis finds (item 9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundUse {
    /// The function (item 2): the first `STT_FUNC` symbol in table order
    /// of its triple, its name, and its section.
    pub function: SymbolIndex,
    pub function_name: String,
    pub section: SectionIndex,
    pub section_name: String,
    /// The instruction's section offset, and its offset in the function.
    pub offset: u64,
    pub function_offset: u64,
    pub reach: Reach,
    /// The judged address, and the name of its section.
    pub address: Address,
    pub address_section_name: String,
    /// The preemptible data definition whose interval holds the address:
    /// the first such definition of its section in symbol-table order.
    pub definition: SymbolIndex,
    pub definition_name: String,
    /// The address is a boundary address of the definition (D-117's
    /// boundary rule); it is a use because it is an access, or because
    /// the object has no self-GOT.
    pub boundary: bool,
    /// The relocation that formed the address, with its symbol's name and
    /// the name of its section's... the section it applies to.
    pub origin: Origin,
    pub origin_symbol_name: String,
    pub origin_section_name: String,
}

impl BoundUse {
    /// The address lies below the address its relocation formed: the
    /// code moved the base backward, as the "before the start of a local
    /// object" paragraph of D-117 describes (`tab - 1`).
    pub fn before_start(&self) -> bool {
        self.address.offset < self.origin.address.offset
    }

    /// The `non-pic-in-shared-object` error of this use (`spec/diag.md`
    /// §3.1, §3.2: for D-117's analysis the note names the function, the
    /// instruction's offset and the relocation that formed the address;
    /// at a boundary and before the start of an object the remedy note).
    pub fn diagnostic(&self, file: &FileRef) -> Diagnostic {
        let r_type = mortar_elf::reloc::name(EM_AARCH64, self.origin.r_type)
            .map_or_else(|| format!("R_AARCH64_<{}>", self.origin.r_type), str::to_owned);
        let kind = Kind::NonPicInSharedObject {
            r_type: r_type.clone(),
            name: self.definition_name.clone(),
            file: match file.member() {
                Some(m) => format!("{}({m})", file.path()),
                None => file.path().to_owned(),
            },
        };
        let how = match self.reach {
            Reach::Access => "accessed",
            Reach::Computed => "computed",
            Reach::WriteBack => "written back",
        };
        let mut d = Diagnostic::error(kind)
            .at(Place::section(file.clone(), self.section_name.clone(), self.offset))
            .note(format!(
                "the address {} is {how} by the instruction at {}+{:#x}, in a register formed by \
                 {r_type} against {} at {}; it lies inside {}",
                show(&self.address_section_name, self.address.offset),
                self.function_name,
                self.function_offset,
                with_addend(&self.origin_symbol_name, self.origin.addend),
                show(&self.origin_section_name, i128::from(self.origin.offset)),
                self.definition_name,
            ));
        if self.before_start() {
            d = d.note(
                "compile with -fdata-sections (each object in its own section), and do not form \
                 an address before an object",
            );
        } else if self.boundary {
            d = d.note("compile with -fdata-sections (each object in its own section)");
        }
        d
    }
}

fn show(section: &str, offset: i128) -> String {
    if offset < 0 { format!("{section}-{:#x}", -offset) } else { format!("{section}+{offset:#x}") }
}

fn with_addend(name: &str, addend: i64) -> String {
    match addend {
        0 => name.to_owned(),
        a if a < 0 => format!("{name}-{:#x}", i128::from(a).unsigned_abs()),
        a => format!("{name}+{a:#x}"),
    }
}

/// The analysis of one object.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Analysis {
    /// The object passed the pre-filter (item 1): the analysis ran.
    pub ran: bool,
    /// Functions (distinct triples, item 2) and their words analysed.
    pub functions: u64,
    pub words: u64,
    /// Words that kill every register for want of a form: data words,
    /// unallocated encodings and other load/store words.
    pub undecoded: u64,
    /// The bound uses, in (function, instruction, reach) order.
    pub uses: Vec<BoundUse>,
}

impl Analysis {
    /// Whether the object is refused.
    pub fn refused(&self) -> bool {
        !self.uses.is_empty()
    }

    /// The definitions the uses lie in, each once, by symbol index.
    pub fn definitions(&self) -> Vec<SymbolIndex> {
        let set: BTreeSet<SymbolIndex> = self.uses.iter().map(|u| u.definition).collect();
        set.into_iter().collect()
    }

    /// One `non-pic-in-shared-object` error per use, in order.
    pub fn diagnostics(&self, file: &FileRef) -> Vec<Diagnostic> {
        self.uses.iter().map(|u| u.diagnostic(file)).collect()
    }
}

/// The preemptible data definitions of the default line (D-112 with no
/// option: every `STV_DEFAULT` global, weak or unique definition): the
/// set `s1d.py` assumes, for tests and measurements. A link computes the
/// set from its `Resolution` instead.
pub fn default_line(obj: &Object<'_>) -> Result<Vec<SymbolIndex>, String> {
    let Some(table) = obj.symbols().map_err(|e| e.to_string())? else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    for (index, sym) in table.iter() {
        let bind = sym.binding();
        if matches!(bind, STB_GLOBAL | STB_WEAK | STB_GNU_UNIQUE)
            && sym.visibility() == STV_DEFAULT
            && matches!(table.section_of(index, &sym), Ok(SymbolSection::Section(_)))
        {
            out.push(index);
        }
    }
    Ok(out)
}

/// [`analyse`]'s `contents` for an object whose sections are all
/// resident (a whole file in memory).
pub fn resident<'a>(
    obj: &Object<'a>,
) -> impl FnMut(SectionIndex) -> Result<Cow<'a, [u8]>, mortar_elf::Error> + use<'a> {
    let obj = *obj;
    move |index| obj.section(index).and_then(|s| s.data()).map(Cow::Borrowed)
}

/// D-117's analysis of `obj` (items 1-9): its bound uses of the
/// preemptible data definitions among `preemptible` (see the module
/// documentation for both arguments). An object that is not `EM_AARCH64`,
/// or that the pre-filter passes over, gives an empty [`Analysis`] with
/// `ran` false and reads no code.
pub fn analyse<'a, E>(
    obj: &Object<'a>,
    preemptible: &[SymbolIndex],
    contents: &mut dyn FnMut(SectionIndex) -> Result<Cow<'a, [u8]>, E>,
) -> Result<Analysis, Fault<E>> {
    if obj.header().e_machine != EM_AARCH64 {
        return Ok(Analysis::default());
    }
    let malformed = |e: mortar_elf::Error| Fault::Malformed(e.to_string());
    let Some(table) = obj.symbols().map_err(malformed)? else { return Ok(Analysis::default()) };
    let sections: Vec<Section<'a>> = obj.sections().collect();
    let flags = |i: u32| sections.get(i as usize).map_or(0, Section::flags);

    // The preemptible data definitions, per section T, in symbol order.
    let mut defs: BTreeMap<u32, Vec<Def>> = BTreeMap::new();
    let mut wanted: Vec<SymbolIndex> = preemptible.to_vec();
    wanted.sort_unstable();
    wanted.dedup();
    for index in wanted {
        let Ok(sym) = table.get(index) else { continue };
        if sym.binding() == STB_LOCAL {
            continue;
        }
        if let Ok(SymbolSection::Section(t)) = table.section_of(index, &sym)
            && flags(t.0) & SHF_EXECINSTR == 0
            && (t.0 as usize) < sections.len()
        {
            defs.entry(t.0).or_default().push(Def {
                index,
                value: i128::from(sym.st_value),
                end: i128::from(sym.st_value) + i128::from(sym.st_size.max(1)),
            });
        }
    }
    if defs.is_empty() {
        return Ok(Analysis::default());
    }

    // The relocation sections of the object, by target.
    let mut relsecs: Vec<(u32, u32)> = Vec::new();
    for s in &sections {
        if s.sh_type() == SHT_RELA
            && let Ok(target) = s.info_section()
        {
            relsecs.push((s.index().0, target.0));
        }
    }

    let mut cx = Cx {
        table,
        sections: &sections,
        defs,
        relsecs,
        rels: BTreeMap::new(),
        contents,
        locals: None,
        self_got: None,
        origins: Vec::new(),
    };

    // Item 1, the pre-filter: a relocation of an SHF_ALLOC|SHF_EXECINSTR
    // section whose symbol is a local defined in some T.
    let exec = SHF_ALLOC | SHF_EXECINSTR;
    let targets: BTreeSet<u32> =
        cx.relsecs.iter().map(|&(_, t)| t).filter(|&t| flags(t) & exec == exec).collect();
    let mut pass = false;
    for t in &targets {
        let syms: Vec<u32> = cx.rels(*t)?.iter().map(|r| r.r_sym).collect();
        if syms.into_iter().any(|s| cx.local_in_t(s).is_some()) {
            pass = true;
            break;
        }
    }
    if !pass {
        return Ok(Analysis::default());
    }
    let mut out = Analysis { ran: true, ..Analysis::default() };

    // Item 2: the functions, distinct (section, start, end) triples, each
    // named by its first symbol in table order.
    let mut funcs: BTreeMap<(u32, u64, u64), SymbolIndex> = BTreeMap::new();
    for (index, sym) in table.iter() {
        if sym.sym_type() != STT_FUNC || sym.st_size == 0 {
            continue;
        }
        let Ok(SymbolSection::Section(s)) = table.section_of(index, &sym) else { continue };
        if flags(s.0) & SHF_EXECINSTR == 0 || s.0 as usize >= sections.len() {
            continue;
        }
        let Some(end) = sym.st_value.checked_add(sym.st_size) else { continue };
        funcs.entry((s.0, sym.st_value, end)).or_insert(index);
    }
    let mut code: Option<(u32, Cow<'a, [u8]>)> = None;
    for (&(sec, start, end), &name) in &funcs {
        out.functions += 1;
        if sections[sec as usize].sh_type() == SHT_NOBITS {
            continue;
        }
        if code.as_ref().is_none_or(|(s, _)| *s != sec) {
            let bytes = (cx.contents)(SectionIndex(sec))
                .map_err(|e| Fault::Contents(SectionIndex(sec), e))?;
            code = Some((sec, bytes));
        }
        let Some((_, bytes)) = code.as_ref() else { continue };
        let f = Function { section: sec, start, end, name };
        cx.function(&f, bytes, &mut out)?;
    }
    Ok(out)
}

/// A preemptible data definition: its symbol and interval.
#[derive(Debug, Clone, Copy)]
struct Def {
    index: SymbolIndex,
    value: i128,
    end: i128,
}

#[derive(Debug, Clone, Copy)]
struct Function {
    section: u32,
    start: u64,
    end: u64,
    name: SymbolIndex,
}

/// A tracked value: its address and the index of its origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Val {
    section: u32,
    offset: i128,
    origin: u32,
}

/// The state at a word (item 3): the tracked registers, sorted, each
/// once; register 31 never.
type State = Vec<(u8, Val)>;

fn get(st: &State, r: u8) -> Option<Val> {
    st.binary_search_by_key(&r, |e| e.0).ok().map(|i| st[i].1)
}

fn set(st: &mut State, r: u8, v: Val) {
    if r >= 31 {
        return;
    }
    match st.binary_search_by_key(&r, |e| e.0) {
        Ok(i) => st[i].1 = v,
        Err(i) => st.insert(i, (r, v)),
    }
}

fn kill(st: &mut State, r: u8) {
    if let Ok(i) = st.binary_search_by_key(&r, |e| e.0) {
        st.remove(i);
    }
}

fn kill_mask(st: &mut State, mask: u32) {
    st.retain(|e| mask & (1 << e.0) == 0);
}

/// x0-x18 and x30: what a call kills (item 7).
const CALL_KILLS: u32 = 0x0007_ffff | (1 << 30);

/// A load/store addressing mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Offset,
    Pre,
    Post,
}

/// A word, decoded by item 5's table (first match).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    /// `B` (`link` false) or `BL`, with its offset.
    B {
        link: bool,
        off: i64,
    },
    /// `B.cond`, `BC.cond`, `CBZ`, `CBNZ`, `TBZ`, `TBNZ`.
    Cond {
        off: i64,
    },
    /// `BR`* (opc 0000, 1000).
    Br,
    /// `BLR`* (opc 0001, 1001).
    Blr,
    /// `RET`*, `ERET`, `DRPS` (opc 0010, 0100, 0101).
    Ret,
    /// Other branch-register, system and branch-group words: kill Rd
    /// (`w[4:0]`) and x30.
    Sys {
        rd: u8,
    },
    /// `ADD`/`SUB` (immediate), `imm` already signed and shifted.
    AddSub {
        sf: bool,
        sub: bool,
        s: bool,
        imm: i64,
        rn: u8,
        rd: u8,
    },
    Adr {
        rd: u8,
    },
    Adrp {
        rd: u8,
    },
    /// `MOV` (register), 64-bit (`ORR Xd, XZR, Xm`).
    Mov {
        rm: u8,
        rd: u8,
    },
    Ldst(Ldst),
    /// `LDR` (literal): `rt` none for a SIMD register; `x64`, a 64-bit
    /// `LDR Xt` (opc 01, not SIMD).
    LdLit {
        off: i64,
        rt: Option<u8>,
        x64: bool,
    },
    /// Kills every register: unallocated encodings, the other words of the
    /// load/store group, data words.
    KillAll,
    /// Everything else: kills Rd (`w[4:0]`).
    Other {
        rd: u8,
    },
}

/// A load/store of the five forms of item 5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Ldst {
    off: i64,
    mode: Mode,
    rn: u8,
    /// The general registers it loads (killed), as a mask.
    kills: u32,
    prfm: bool,
    /// An unsigned-offset 64-bit `LDR Xt` (size 3, opc 01, not SIMD): the
    /// literal form of item 4 with `LDST64_ABS_LO12_NC`; its Rt.
    ldr_x: Option<u8>,
}

fn sx(v: u32, bits: u32) -> i64 {
    let v = i64::from(v);
    if v & (1 << (bits - 1)) != 0 { v - (1 << bits) } else { v }
}

/// Item 5's table, first match.
#[allow(clippy::too_many_lines, reason = "one table, row by row")]
fn decode(w: u32) -> Op {
    let field = |lo: u32, n: u32| (w >> lo) & ((1 << n) - 1);
    let rd = (w & 31) as u8;
    if field(26, 5) == 0b00101 {
        return Op::B { link: w >> 31 == 1, off: sx(field(0, 26), 26) * 4 };
    }
    if w & 0xff00_0000 == 0x5400_0000 {
        return Op::Cond { off: sx(field(5, 19), 19) * 4 };
    }
    if field(25, 6) == 0b01_1010 {
        return Op::Cond { off: sx(field(5, 19), 19) * 4 };
    }
    if field(25, 6) == 0b01_1011 {
        return Op::Cond { off: sx(field(5, 14), 14) * 4 };
    }
    if field(25, 7) == 0b110_1011 {
        return match field(21, 4) {
            0b0000 | 0b1000 => Op::Br,
            0b0001 | 0b1001 => Op::Blr,
            0b0010 | 0b0100 | 0b0101 => Op::Ret,
            _ => Op::Sys { rd },
        };
    }
    if field(23, 6) == 0b10_0010 {
        let imm = i64::from(field(10, 12)) << if field(22, 1) == 1 { 12 } else { 0 };
        return Op::AddSub {
            sf: w >> 31 == 1,
            sub: field(30, 1) == 1,
            s: field(29, 1) == 1,
            imm: if field(30, 1) == 1 { -imm } else { imm },
            rn: field(5, 5) as u8,
            rd,
        };
    }
    if field(24, 5) == 0b1_0000 {
        return if w >> 31 == 1 { Op::Adrp { rd } } else { Op::Adr { rd } };
    }
    if w & 0xffe0_ffe0 == 0xaa00_03e0 {
        return Op::Mov { rm: field(16, 5) as u8, rd };
    }
    let op0 = field(25, 4);
    if op0 & 0b0101 == 0b0100 {
        return decode_ldst(w);
    }
    if op0 & 0b1110 == 0b1010 {
        return Op::Sys { rd };
    }
    if matches!(op0, 0b0000 | 0b0001 | 0b0011) {
        return Op::KillAll;
    }
    Op::Other { rd }
}

/// The load/store group of item 5.
fn decode_ldst(w: u32) -> Op {
    let field = |lo: u32, n: u32| (w >> lo) & ((1 << n) - 1);
    let size = field(30, 2);
    let v = field(26, 1) == 1;
    let rn = field(5, 5) as u8;
    let rt = field(0, 5);
    let b2924 = field(24, 6) & 0b11_1011;
    let opc = field(22, 2);
    let mode_of = |m: u32| match m {
        1 => Mode::Post,
        3 => Mode::Pre,
        _ => Mode::Offset,
    };
    // Unsigned offset.
    if b2924 == 0b11_1001 {
        let (scale, load, prfm) = if v {
            (if size == 0 && opc >> 1 == 1 { 4 } else { size }, opc & 1 == 1, false)
        } else {
            let prfm = size == 3 && opc == 2;
            (size, opc != 0 && !prfm, prfm)
        };
        let ldr_x = (!v && size == 3 && opc == 1).then_some(rt as u8);
        return Op::Ldst(Ldst {
            off: i64::from(field(10, 12)) << scale,
            mode: Mode::Offset,
            rn,
            kills: if load && !v { 1 << rt } else { 0 },
            prfm,
            ldr_x,
        });
    }
    // The imm9 forms: unscaled, post-index, unprivileged, pre-index.
    if b2924 == 0b11_1000 && field(21, 1) == 0 {
        let mode = mode_of(field(10, 2));
        let (load, prfm) = if v {
            (opc & 1 == 1, false)
        } else {
            let prfm = size == 3 && opc == 2 && mode == Mode::Offset;
            (opc != 0 && !prfm, prfm)
        };
        return Op::Ldst(Ldst {
            off: sx(field(12, 9), 9),
            mode,
            rn,
            kills: if load && !v { 1 << rt } else { 0 },
            prfm,
            ldr_x: None,
        });
    }
    // Pair.
    if field(27, 3) == 0b101 && field(25, 1) == 0 {
        let opc = field(30, 2);
        let load = field(22, 1) == 1;
        let scale = if v {
            2 + opc
        } else if opc == 2 {
            3
        } else if opc == 1 && !load {
            4
        } else {
            2
        };
        let rt2 = field(10, 5);
        return Op::Ldst(Ldst {
            off: sx(field(15, 7), 7) << scale,
            mode: mode_of(field(23, 2)),
            rn,
            kills: if load && !v { (1 << rt) | (1 << rt2) } else { 0 },
            prfm: false,
            ldr_x: None,
        });
    }
    // Exclusive, ordered, compare-and-swap: kill Rt, Rt2, Rs and Rs+1.
    if field(24, 6) == 0b00_1000 {
        let rs = field(16, 5);
        let rt2 = field(10, 5);
        let mut kills = (1 << rt) | (1 << rt2) | (1 << rs);
        if rs < 31 {
            kills |= 1 << (rs + 1);
        }
        return Op::Ldst(Ldst { off: 0, mode: Mode::Offset, rn, kills, prfm: false, ldr_x: None });
    }
    // Atomic memory operations.
    if b2924 == 0b11_1000 && field(21, 1) == 1 && field(10, 2) == 0 {
        return Op::Ldst(Ldst {
            off: 0,
            mode: Mode::Offset,
            rn,
            kills: if v { 0 } else { 1 << rt },
            prfm: false,
            ldr_x: None,
        });
    }
    // LDR (literal).
    if b2924 == 0b01_1000 {
        return Op::LdLit {
            off: sx(field(5, 19), 19) * 4,
            rt: (!v).then_some(rt as u8),
            x64: !v && field(30, 2) == 1,
        };
    }
    Op::KillAll
}

/// A word of a function: its form, the relocation at it, and the value a
/// defining form gives (item 4).
#[derive(Debug, Clone, Copy)]
struct Word {
    op: Op,
    rel: Option<Rela>,
    def: Option<Val>,
}

struct Cx<'c, 'a, E> {
    table: SymbolTable<'a>,
    sections: &'c [Section<'a>],
    defs: BTreeMap<u32, Vec<Def>>,
    /// `(relocation section, target)` of every `SHT_RELA` section.
    relsecs: Vec<(u32, u32)>,
    /// Per target section, its relocations sorted by offset, the last
    /// record of each offset kept (as `s1d.py`'s table by offset).
    rels: BTreeMap<u32, Vec<Rela>>,
    contents: &'c mut dyn FnMut(SectionIndex) -> Result<Cow<'a, [u8]>, E>,
    /// Per T, the ends of its local symbols with a size (the boundary).
    locals: Option<BTreeMap<u32, BTreeSet<i128>>>,
    self_got: Option<bool>,
    origins: Vec<Origin>,
}

impl<E> Cx<'_, '_, E> {
    fn malformed(e: &mortar_elf::Error) -> Fault<E> {
        Fault::Malformed(e.to_string())
    }

    /// The records of every `SHT_RELA` section that applies to `target`,
    /// in section then record order.
    fn records(&mut self, target: u32) -> Result<Vec<Rela>, Fault<E>> {
        let mut out = Vec::new();
        for &(rs, t) in &self.relsecs {
            if t != target {
                continue;
            }
            let s = self.sections[rs as usize];
            let bytes = (self.contents)(SectionIndex(rs))
                .map_err(|e| Fault::Contents(SectionIndex(rs), e))?;
            let rela = s.rela_in(&bytes).map_err(|e| Self::malformed(&e))?;
            out.extend(rela.iter());
        }
        Ok(out)
    }

    /// The relocations of `target` by offset (the last of an offset).
    fn rels(&mut self, target: u32) -> Result<&[Rela], Fault<E>> {
        if !self.rels.contains_key(&target) {
            let mut v = self.records(target)?;
            // Stable: equal offsets keep record order, and the last wins.
            v.sort_by_key(|r| r.r_offset);
            let mut keep: Vec<Rela> = Vec::with_capacity(v.len());
            for r in v {
                match keep.last_mut() {
                    Some(last) if last.r_offset == r.r_offset => *last = r,
                    _ => keep.push(r),
                }
            }
            self.rels.insert(target, keep);
        }
        Ok(self.rels.get(&target).map_or(&[][..], Vec::as_slice))
    }

    fn rel_at(&mut self, section: u32, offset: u64) -> Result<Option<Rela>, Fault<E>> {
        let rels = self.rels(section)?;
        Ok(rels.binary_search_by_key(&offset, |r| r.r_offset).ok().map(|i| rels[i]))
    }

    /// A local symbol defined in some T: its section and value.
    fn local_in_t(&self, r_sym: u32) -> Option<(u32, i128)> {
        let index = SymbolIndex(r_sym);
        let sym = self.table.get(index).ok()?;
        if sym.binding() != STB_LOCAL {
            return None;
        }
        match self.table.section_of(index, &sym) {
            Ok(SymbolSection::Section(t)) if self.defs.contains_key(&t.0) => {
                Some((t.0, i128::from(sym.st_value)))
            }
            _ => None,
        }
    }

    /// The value a relocation against a local of T gives: `y.st_value +
    /// A`, with its origin recorded.
    fn origin(&mut self, section: u32, r: &Rela) -> Option<Val> {
        let (t, value) = self.local_in_t(r.r_sym)?;
        let offset = value + i128::from(r.r_addend);
        let origin = self.origins.len() as u32;
        self.origins.push(Origin {
            section: SectionIndex(section),
            offset: r.r_offset,
            r_type: r.r_type,
            symbol: SymbolIndex(r.r_sym),
            addend: r.r_addend,
            address: Address { section: SectionIndex(t), offset },
        });
        Some(Val { section: t, offset, origin })
    }

    /// The literal word at `(section, offset)`: its `ABS64` relocation
    /// against a local of T (item 4).
    fn literal(&mut self, section: u32, offset: i128) -> Result<Option<Val>, Fault<E>> {
        let Ok(offset) = u64::try_from(offset) else { return Ok(None) };
        if section as usize >= self.sections.len() {
            return Ok(None);
        }
        match self.rel_at(section, offset)? {
            Some(r) if r.r_type == R_ABS64 => Ok(self.origin(section, &r)),
            _ => Ok(None),
        }
    }

    /// The literal word a symbol z names, `z.st_value + A`.
    fn literal_by_symbol(&mut self, r: &Rela) -> Result<Option<Val>, Fault<E>> {
        let index = SymbolIndex(r.r_sym);
        let Ok(z) = self.table.get(index) else { return Ok(None) };
        let Ok(SymbolSection::Section(s)) = self.table.section_of(index, &z) else {
            return Ok(None);
        };
        self.literal(s.0, i128::from(z.st_value) + i128::from(r.r_addend))
    }

    /// The value of a defining form (item 4) at a word, if it is one.
    fn define(
        &mut self,
        section: u32,
        place: u64,
        op: Op,
        rel: Option<&Rela>,
    ) -> Result<Option<Val>, Fault<E>> {
        Ok(match (op, rel) {
            (Op::AddSub { sf: true, sub: false, s: false, rd, .. }, Some(r))
                if r.r_type == R_ADD_ABS_LO12_NC && rd != 31 =>
            {
                self.origin(section, r)
            }
            (Op::Adr { rd }, Some(r)) if r.r_type == R_ADR_PREL_LO21 && rd != 31 => {
                self.origin(section, r)
            }
            (Op::Ldst(Ldst { ldr_x: Some(_), .. }), Some(r))
                if r.r_type == R_LDST64_ABS_LO12_NC =>
            {
                self.literal_by_symbol(r)?
            }
            (Op::LdLit { x64: true, .. }, Some(r)) if r.r_type == R_LD_PREL_LO19 => {
                self.literal_by_symbol(r)?
            }
            (Op::LdLit { x64: true, off, .. }, None) => {
                self.literal(section, i128::from(place) + i128::from(off))?
            }
            _ => None,
        })
    }

    fn self_got(&mut self) -> Result<bool, Fault<E>> {
        if let Some(v) = self.self_got {
            return Ok(v);
        }
        // The names of the object's own preemptible data definitions.
        let mut own: BTreeSet<&[u8]> = BTreeSet::new();
        for d in self.defs.values().flatten() {
            if let Ok(sym) = self.table.get(d.index)
                && let Ok(name) = self.table.name(&sym)
            {
                own.insert(name);
            }
        }
        let targets: BTreeSet<u32> = self.relsecs.iter().map(|&(_, t)| t).collect();
        let mut found = false;
        'all: for t in targets {
            for r in self.records(t)? {
                if !is_got_form(r.r_type) {
                    continue;
                }
                let Ok(sym) = self.table.get(SymbolIndex(r.r_sym)) else { continue };
                if sym.binding() != STB_LOCAL
                    && self.table.name(&sym).is_ok_and(|n| own.contains(n))
                {
                    found = true;
                    break 'all;
                }
            }
        }
        self.self_got = Some(found);
        Ok(found)
    }

    fn local_ends(&mut self) -> &BTreeMap<u32, BTreeSet<i128>> {
        if self.locals.is_none() {
            let mut m: BTreeMap<u32, BTreeSet<i128>> = BTreeMap::new();
            for (index, sym) in self.table.locals() {
                if sym.binding() != STB_LOCAL || sym.st_size == 0 {
                    continue;
                }
                if let Ok(SymbolSection::Section(t)) = self.table.section_of(index, &sym)
                    && self.defs.contains_key(&t.0)
                {
                    m.entry(t.0)
                        .or_default()
                        .insert(i128::from(sym.st_value) + i128::from(sym.st_size));
                }
            }
            self.locals = Some(m);
        }
        self.locals.get_or_insert_with(BTreeMap::new)
    }

    /// Item 9: the definition a judged address lies in, and whether it
    /// is a boundary address, or `None` when it is not a bound use.
    fn judge(&mut self, v: Val, access: bool) -> Result<Option<(Def, bool)>, Fault<E>> {
        let Some(d) = self
            .defs
            .get(&v.section)
            .and_then(|ds| ds.iter().find(|d| d.value <= v.offset && v.offset < d.end))
            .copied()
        else {
            return Ok(None);
        };
        let boundary = v.offset == d.value
            && self.local_ends().get(&v.section).is_some_and(|e| e.contains(&v.offset));
        if boundary && !access && self.self_got()? {
            return Ok(None);
        }
        Ok(Some((d, boundary)))
    }

    #[allow(clippy::too_many_lines, reason = "items 2 and 8, in order")]
    fn function(&mut self, f: &Function, bytes: &[u8], out: &mut Analysis) -> Result<(), Fault<E>> {
        // The words that lie in the section's contents (see the module
        // documentation for the rest).
        let len = bytes.len() as u64;
        let span = f.end.min(len).saturating_sub(f.start.min(len));
        let n = usize::try_from(span / 4).unwrap_or(usize::MAX);
        let mut n = n.min((f.end - f.start) as usize / 4);
        // A word must fit wholly in the contents.
        while n > 0 && f.start + 4 * (n as u64) > len {
            n -= 1;
        }
        let marks = self.marks(f.section);
        let mut words: Vec<Word> = Vec::with_capacity(n);
        let mut any_def = false;
        let mut mark = 0usize;
        for i in 0..n {
            let place = f.start + 4 * i as u64;
            while mark < marks.len() && marks[mark].0 <= place {
                mark += 1;
            }
            let data = mark > 0 && !marks[mark - 1].1;
            let at = place as usize;
            let op = if data {
                Op::KillAll
            } else {
                match bytes.get(at..at + 4) {
                    Some(b) => decode(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
                    None => Op::KillAll,
                }
            };
            if data || matches!(op, Op::KillAll) {
                out.undecoded += 1;
            }
            let rel = self.rel_at(f.section, place)?;
            let def = if data { None } else { self.define(f.section, place, op, rel.as_ref())? };
            any_def |= def.is_some();
            words.push(Word { op, rel, def });
        }
        out.words += n as u64;
        if !any_def {
            // No defining form: every state is empty, nothing is judged.
            return Ok(());
        }

        // Item 8: successors, predecessors and leaders.
        let target = |i: usize, off: i64| -> Option<usize> {
            let t = i as i64 + off / 4;
            (0..n as i64).contains(&t).then_some(t as usize)
        };
        let mut succ: Vec<[Option<usize>; 2]> = Vec::with_capacity(n);
        let mut leader = vec![false; n];
        if n > 0 {
            leader[0] = true;
        }
        let mut has_br = false;
        for (i, w) in words.iter().enumerate() {
            let next = (i + 1 < n).then_some(i + 1);
            // A relocated branch's target is not followed.
            let jump = |off| if w.rel.is_some() { None } else { target(i, off) };
            let (s, branch, to) = match w.op {
                Op::B { link: false, off } => ([jump(off), None], true, jump(off)),
                Op::Cond { off } => ([next, jump(off)], true, jump(off)),
                Op::Br | Op::Ret => {
                    has_br |= w.op == Op::Br;
                    ([None, None], true, None)
                }
                _ => ([next, None], false, None),
            };
            // Leaders: every in-function target, the word after a branch.
            if let Some(t) = to {
                leader[t] = true;
            }
            if let (true, Some(next)) = (branch, next) {
                leader[next] = true;
            }
            succ.push(s);
        }
        let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, s) in succ.iter().enumerate() {
            for t in s.iter().flatten() {
                if !preds[*t].contains(&i) {
                    preds[*t].push(i);
                }
            }
        }

        // The greatest fixed point, by a worklist in address order.
        let mut outs: Vec<Option<State>> = vec![None; n];
        let mut work: BTreeSet<usize> = (0..n).collect();
        let mut cursor = 0usize;
        while let Some(&i) = work.range(cursor..).next().or_else(|| work.iter().next()) {
            work.remove(&i);
            cursor = i + 1;
            let Some(mut st) = Self::entry(i, has_br, &leader, &preds, &outs) else { continue };
            self.transfer(f, i, &words[i], &mut st, None)?;
            if outs[i].as_ref() != Some(&st) {
                outs[i] = Some(st);
                for t in succ[i].iter().flatten() {
                    work.insert(*t);
                }
            }
        }
        // The final pass, in address order, judging.
        for (i, word) in words.iter().enumerate() {
            let Some(mut st) = Self::entry(i, has_br, &leader, &preds, &outs) else { continue };
            self.transfer(f, i, word, &mut st, Some(&mut out.uses))?;
        }
        Ok(())
    }

    /// The entry state of word `i` (item 8), `None` while no predecessor
    /// is computed.
    fn entry(
        i: usize,
        has_br: bool,
        leader: &[bool],
        preds: &[Vec<usize>],
        outs: &[Option<State>],
    ) -> Option<State> {
        if i == 0 || (has_br && leader[i]) || preds[i].is_empty() {
            return Some(State::new());
        }
        let mut computed = preds[i].iter().filter_map(|&p| outs[p].as_ref());
        let mut st = computed.next()?.clone();
        for q in computed {
            st.retain_mut(|(r, v)| match get(q, *r) {
                Some(w) if w.section == v.section && w.offset == v.offset => {
                    v.origin = v.origin.min(w.origin);
                    true
                }
                _ => false,
            });
        }
        Some(st)
    }

    /// Items 5-7 at word `i` from state `st`; with `uses`, the judged
    /// addresses that are bound uses are recorded.
    #[allow(clippy::many_single_char_names, reason = "the state, word and function of one step")]
    fn transfer(
        &mut self,
        f: &Function,
        i: usize,
        w: &Word,
        st: &mut State,
        uses: Option<&mut Vec<BoundUse>>,
    ) -> Result<(), Fault<E>> {
        let place = f.start + 4 * i as u64;
        let mut judged: Vec<(Val, Reach)> = Vec::new();
        match w.op {
            Op::KillAll => st.clear(),
            Op::B { link: true, .. } | Op::Blr => kill_mask(st, CALL_KILLS),
            Op::B { link: false, .. } | Op::Cond { .. } | Op::Br | Op::Ret => {}
            Op::Sys { rd } => kill_mask(st, (1 << rd) | (1 << 30)),
            Op::AddSub { sf, imm, rn, rd, .. } => {
                if w.rel.is_some() {
                    match w.def {
                        Some(v) => set(st, rd, v),
                        None => kill(st, rd),
                    }
                } else if let (true, false, false, Some(v)) = (sf, rn == 31, rd == 31, get(st, rn))
                {
                    let nv = Val { offset: v.offset + i128::from(imm), ..v };
                    set(st, rd, nv);
                    judged.push((nv, Reach::Computed));
                } else {
                    kill(st, rd);
                }
            }
            Op::Adr { rd } => match w.def {
                Some(v) => set(st, rd, v),
                None => kill(st, rd),
            },
            Op::Adrp { rd } | Op::Other { rd } => kill(st, rd),
            Op::Mov { rm, rd } => match (w.rel, get(st, rm)) {
                (None, Some(v)) if rm != 31 => set(st, rd, v),
                _ => kill(st, rd),
            },
            Op::Ldst(l) => {
                if w.rel.is_some() {
                    // Not a propagation: it kills what it writes.
                    kill_mask(st, l.kills);
                    if l.mode != Mode::Offset {
                        kill(st, l.rn);
                    }
                    if let (Some(v), Some(rt)) = (w.def, l.ldr_x) {
                        set(st, rt, v);
                    }
                } else {
                    match get(st, l.rn).filter(|_| l.rn != 31) {
                        Some(v) => {
                            let at = if l.mode == Mode::Post {
                                v.offset
                            } else {
                                v.offset + i128::from(l.off)
                            };
                            if !l.prfm {
                                judged.push((Val { offset: at, ..v }, Reach::Access));
                            }
                            if l.mode != Mode::Offset {
                                let nv = Val { offset: v.offset + i128::from(l.off), ..v };
                                set(st, l.rn, nv);
                                judged.push((nv, Reach::WriteBack));
                            }
                        }
                        None => {
                            if l.mode != Mode::Offset {
                                kill(st, l.rn);
                            }
                        }
                    }
                    kill_mask(st, l.kills);
                }
            }
            Op::LdLit { rt, .. } => {
                if let Some(rt) = rt {
                    kill(st, rt);
                    if let Some(v) = w.def {
                        set(st, rt, v);
                    }
                }
            }
        }
        if let Some(uses) = uses {
            for (v, reach) in judged {
                if let Some((d, boundary)) = self.judge(v, reach.is_access())? {
                    let u = self.bound_use(f, place, reach, v, d, boundary);
                    uses.push(u);
                }
            }
        }
        Ok(())
    }

    fn name_of(&self, index: SymbolIndex) -> String {
        self.table
            .get(index)
            .ok()
            .and_then(|s| {
                if s.sym_type() == mortar_elf::consts::STT_SECTION {
                    return match self.table.section_of(index, &s) {
                        Ok(SymbolSection::Section(t)) => Some(self.section_name(t.0)),
                        _ => None,
                    };
                }
                self.table.name(&s).ok().map(|n| String::from_utf8_lossy(n).into_owned())
            })
            .unwrap_or_else(|| format!("<symbol {}>", index.0))
    }

    fn section_name(&self, index: u32) -> String {
        self.sections.get(index as usize).and_then(|s| s.name().ok()).map_or_else(
            || format!("<section {index}>"),
            |n| String::from_utf8_lossy(n).into_owned(),
        )
    }

    fn bound_use(
        &self,
        f: &Function,
        place: u64,
        reach: Reach,
        v: Val,
        d: Def,
        boundary: bool,
    ) -> BoundUse {
        let origin = self.origins.get(v.origin as usize).cloned().unwrap_or(Origin {
            section: SectionIndex(f.section),
            offset: place,
            r_type: 0,
            symbol: SymbolIndex(0),
            addend: 0,
            address: Address { section: SectionIndex(v.section), offset: v.offset },
        });
        BoundUse {
            function: f.name,
            function_name: self.name_of(f.name),
            section: SectionIndex(f.section),
            section_name: self.section_name(f.section),
            offset: place,
            function_offset: place - f.start,
            reach,
            address: Address { section: SectionIndex(v.section), offset: v.offset },
            address_section_name: self.section_name(v.section),
            definition: d.index,
            definition_name: self.name_of(d.index),
            boundary,
            origin_symbol_name: self.name_of(origin.symbol),
            origin_section_name: self.section_name(origin.section.0),
            origin,
        }
    }

    /// The mapping symbols of a section (item 2): `(value, code)` sorted,
    /// a `$x` after a `$d` of the same value.
    fn marks(&self, section: u32) -> Vec<(u64, bool)> {
        let mut out = Vec::new();
        for (index, sym) in self.table.locals() {
            if sym.binding() != STB_LOCAL {
                continue;
            }
            let Ok(SymbolSection::Section(s)) = self.table.section_of(index, &sym) else {
                continue;
            };
            if s.0 != section {
                continue;
            }
            let Ok(name) = self.table.name(&sym) else { continue };
            let code = match name {
                [b'$', c @ (b'x' | b'd')] | [b'$', c @ (b'x' | b'd'), b'.', ..] => *c == b'x',
                _ => continue,
            };
            out.push((sym.st_value, code));
        }
        out.sort_unstable();
        out
    }
}

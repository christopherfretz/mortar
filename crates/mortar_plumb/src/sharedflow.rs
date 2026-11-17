//! The verifier's own D-117 address clause for a shared object: the rule
//! and the AArch64 def-use analysis (`spec/plumb.md` P-D17; M4, WP-121).
//!
//! Written from `spec/dyn.md` D-117's text ("Bound use of exported data",
//! the boundary, "Before the start of a local object", and items 1-9 of
//! "Bound use through a register"), never from the linker's code. Its
//! oracle is `tests/shared-design/exp3/s1d.py` (record §G.1), compared off
//! the gate by the ignored test `oracle_lists` below.
//!
//! The inputs are read through [`Unit`], by position (§8.1): the symbol
//! table and the section headers are the object's skeleton; a section's
//! bytes and relocations are read when a rule asks for them, and an
//! object with no preemptible data definition reads nothing.
// ELF fields and A64 fields read best by the specs' short names.
#![allow(clippy::many_single_char_names, clippy::similar_names)]

use std::collections::{BTreeMap, BTreeSet, HashMap};

use mortar_elf::consts::{
    EM_386, EM_AARCH64, EM_X86_64, SHF_ALLOC, SHF_EXECINSTR, SHT_NOBITS, STB_LOCAL, STT_FUNC,
};
use mortar_elf::{Shdr, SymbolIndex, SymbolSection, SymbolTable};

use crate::locate::{Obj, Place, RawRecord};
use crate::report::{Finding, Rule};
use crate::store::Bytes;

/// An input object as the clause reads it.
pub(crate) trait Unit<'a> {
    fn machine(&self) -> u16;
    fn symtab(&self) -> Option<&SymbolTable<'a>>;
    fn count(&self) -> usize;
    fn shdr(&self, si: usize) -> &Shdr;
    fn sec_name(&self, si: usize) -> &str;
    /// The relocation section that applies to section `si`.
    fn reloc_sec(&self, si: usize) -> Option<u32>;
    /// Whether section `si` is in the link (not discarded).
    fn live(&self, si: usize) -> bool;
    fn bytes(&self, si: usize) -> Bytes<'a>;
    fn recs(&self, rs: u32) -> Option<Vec<RawRecord>>;
}

impl<'a> Unit<'a> for Obj<'a> {
    fn machine(&self) -> u16 {
        self.elf.header().e_machine
    }
    fn symtab(&self) -> Option<&SymbolTable<'a>> {
        self.symtab.as_ref()
    }
    fn count(&self) -> usize {
        self.secs.len()
    }
    fn shdr(&self, si: usize) -> &Shdr {
        &self.secs[si].shdr
    }
    fn sec_name(&self, si: usize) -> &str {
        &self.secs[si].name
    }
    fn reloc_sec(&self, si: usize) -> Option<u32> {
        self.secs[si].relocs
    }
    fn live(&self, si: usize) -> bool {
        !matches!(self.secs[si].place, Place::Discarded)
    }
    fn bytes(&self, si: usize) -> Bytes<'a> {
        self.data(si)
    }
    fn recs(&self, rs: u32) -> Option<Vec<RawRecord>> {
        self.records(rs)
    }
}

/// One preemptible data definition: `[lo, hi)` in its section's input
/// offsets.
#[derive(Debug, Clone)]
struct Def {
    lo: u64,
    hi: u64,
    name: String,
}

/// How a bound use was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum By {
    /// The relocation-level rule: the relocation's type.
    Rule { ty: u32 },
    /// The analysis (items 1-9): the function and the instruction's
    /// offset in it, whether it is an access, and the relocation that
    /// formed the address (its section, offset and type).
    Analysis { func: String, at: u64, access: bool, from: (usize, u64, u32) },
}

/// A bound use (D-117): the place, the named address, the definition.
#[derive(Debug, Clone)]
pub(crate) struct Hit {
    /// The section and input offset of the relocation or instruction.
    pub sec: usize,
    pub offset: u64,
    /// The section holding the definition, and the named address in it.
    pub t: usize,
    pub addr: u64,
    pub def: String,
    /// A boundary address (m-1) that is still a bound use.
    pub boundary: bool,
    pub by: By,
}

/// What the clause found in one object, with the oracle's counts.
#[derive(Debug, Default)]
pub(crate) struct Scan {
    /// The analysis's pre-filter passed (item 1).
    pub pre: bool,
    /// The object has a self-GOT (the boundary rule).
    pub selfgot: bool,
    pub hits: Vec<Hit>,
    /// Words decoded and functions analysed (item 2).
    pub words: u64,
    pub funcs: u64,
    /// x86 uses not judged: a section-relative PC-relative field in an
    /// `SHF_EXECINSTR` place that names an address in a definition (the
    /// M-1 ruling's recorded case).
    pub not_judged: u64,
}

const R_AARCH64_ABS64: u32 = 257;
const R_AARCH64_LD_PREL_LO19: u32 = 273;
const R_AARCH64_ADR_PREL_LO21: u32 = 274;
const R_AARCH64_ADD_ABS_LO12_NC: u32 = 277;
const R_AARCH64_LDST64_ABS_LO12_NC: u32 = 286;

/// AArch64 load/store forms: an access at the named address (m-1 (i)).
fn a64_access(ty: u32) -> bool {
    matches!(ty, 278 | 284 | 285 | 286 | 299 | 273)
}

/// The GOT forms of the boundary rule (m-1 (ii)).
fn got_form(machine: u16, ty: u32) -> bool {
    match machine {
        EM_AARCH64 => matches!(ty, 300..=306 | 309..=313),
        EM_X86_64 => matches!(ty, 3 | 9 | 41 | 42 | 27 | 28 | 30),
        EM_386 => matches!(ty, 3 | 43),
        _ => false,
    }
}

/// `R_*_NONE` and the `SIZE` types: never a use.
fn not_a_use(machine: u16, ty: u32) -> bool {
    ty == 0
        || match machine {
            EM_AARCH64 => ty == 256,
            EM_X86_64 => matches!(ty, 32 | 33),
            EM_386 => ty == 38,
            _ => false,
        }
}

/// The x86 scope (the M-1 ruling): whether a use's named address is
/// exact. `Some(width)` of a REL implicit addend for i386.
fn x86_exact(machine: u16, ty: u32, exec: bool) -> bool {
    match machine {
        // 64, 32, 32S, 16, 8; GOTOFF64; PC32, PC64, PC16, PC8 in data.
        EM_X86_64 => {
            matches!(ty, 1 | 10 | 11 | 12 | 14 | 25) || (!exec && matches!(ty, 2 | 24 | 13 | 15))
        }
        // 32, 16, 8; GOTOFF; PC32, PC16, PC8 in data.
        EM_386 => matches!(ty, 1 | 20 | 22 | 9) || (!exec && matches!(ty, 2 | 21 | 23)),
        _ => false,
    }
}

/// The width of an i386 REL field the exact forms use.
fn rel_width(ty: u32) -> usize {
    match ty {
        20 | 21 => 2,
        22 | 23 => 1,
        _ => 4,
    }
}

/// An object's preemptible data definitions and what the rules need of
/// its symbols.
struct Facts {
    /// By section T: definitions sorted by `lo`.
    defs: HashMap<usize, Vec<Def>>,
    /// By section T: `st_value + st_size` of its sized local symbols.
    ends: HashMap<usize, BTreeSet<u64>>,
    /// Names of the definitions (for the self-GOT test).
    names: BTreeSet<String>,
}

impl Facts {
    /// The definition of T whose interval holds `a`.
    fn def_at(&self, t: usize, a: u64) -> Option<&Def> {
        let v = self.defs.get(&t)?;
        // Intervals may overlap (aliases): the first by `lo`, then in
        // symbol-table order.
        v.iter().find(|d| d.lo <= a && a < d.hi)
    }

    /// Whether `a` is a boundary address of `d` in T (m-1).
    fn boundary(&self, t: usize, d: &Def, a: u64) -> bool {
        a == d.lo && self.ends.get(&t).is_some_and(|e| e.contains(&a))
    }
}

/// A symbol's section, when it is a regular one.
fn sec_of(t: &SymbolTable<'_>, i: SymbolIndex, s: &mortar_elf::Sym) -> Option<usize> {
    match t.section_of(i, s) {
        Ok(SymbolSection::Section(k)) => Some(k.0 as usize),
        _ => None,
    }
}

fn facts<'a>(u: &impl Unit<'a>, preemptible: &dyn Fn(&str) -> bool) -> Option<Facts> {
    let t = u.symtab()?;
    let n = u.count();
    let mut f = Facts { defs: HashMap::new(), ends: HashMap::new(), names: BTreeSet::new() };
    for (i, s) in t.iter() {
        if s.binding() == STB_LOCAL {
            continue;
        }
        let Some(k) = sec_of(t, i, &s) else { continue };
        if k >= n || u.shdr(k).sh_flags & SHF_EXECINSTR != 0 || !u.live(k) {
            continue;
        }
        let Ok(name) = t.name(&s) else { continue };
        let name = String::from_utf8_lossy(name).into_owned();
        if !preemptible(crate::shared::split_version(&name).0) {
            continue;
        }
        let hi = s.st_value.saturating_add(s.st_size.max(1));
        f.defs.entry(k).or_default().push(Def { lo: s.st_value, hi, name: name.clone() });
        f.names.insert(name);
    }
    if f.defs.is_empty() {
        return None;
    }
    for v in f.defs.values_mut() {
        // By address; aliases in symbol-table order.
        v.sort_by_key(|d| d.lo);
    }
    for (i, s) in t.locals() {
        if s.st_size == 0 {
            continue;
        }
        let Some(k) = sec_of(t, i, &s) else { continue };
        if f.defs.contains_key(&k) {
            f.ends.entry(k).or_default().insert(s.st_value.wrapping_add(s.st_size));
        }
    }
    Some(f)
}

/// A local symbol defined in a section holding a definition: the
/// section and `st_value`.
fn local_in_t(t: &SymbolTable<'_>, f: &Facts, sym: u32) -> Option<(usize, u64)> {
    let i = SymbolIndex(sym);
    let s = t.get(i).ok()?;
    if s.binding() != STB_LOCAL {
        return None;
    }
    let k = sec_of(t, i, &s)?;
    f.defs.contains_key(&k).then_some((k, s.st_value))
}

/// The bound uses of one object (D-117): the relocation-level rule on
/// every machine, and on AArch64 the analysis. `preemptible` is D-112's
/// verdict on a key, by its unversioned name.
pub(crate) fn scan<'a>(u: &impl Unit<'a>, preemptible: &dyn Fn(&str) -> bool) -> Scan {
    let mut out = Scan::default();
    let Some(f) = facts(u, preemptible) else { return out };
    let Some(t) = u.symtab() else { return out };
    let machine = u.machine();
    // Candidates of the rule: (hit, access); boundary ones are decided
    // once the self-GOT is known.
    let mut cands: Vec<(Hit, bool)> = Vec::new();
    for si in 0..u.count() {
        let sh = u.shdr(si);
        let Some(rs) = u.reloc_sec(si) else { continue };
        // The relocations of allocated places: a GOT form is never in a
        // non-allocated one, and no use is.
        if sh.sh_flags & SHF_ALLOC == 0 || !u.live(si) {
            continue;
        }
        let name = u.sec_name(si);
        let use_place = name != ".eh_frame" && !name.starts_with(".gcc_except_table");
        let exec = sh.sh_flags & SHF_EXECINSTR != 0;
        let Some(recs) = u.recs(rs) else { continue };
        let mut body: Option<Bytes<'a>> = None;
        for &(off, sym, ty, addend) in &recs {
            if got_form(machine, ty)
                && let Ok(s) = t.get(SymbolIndex(sym))
                && s.binding() != STB_LOCAL
                && let Ok(n) = t.name(&s)
                && f.names.contains(String::from_utf8_lossy(n).as_ref())
            {
                out.selfgot = true;
            }
            let Some((k, value)) = local_in_t(t, &f, sym) else { continue };
            if exec && machine == EM_AARCH64 {
                out.pre = true;
            }
            if not_a_use(machine, ty) {
                continue;
            }
            if !use_place {
                continue;
            }
            #[allow(clippy::single_match_else)] // the REL field, read only here
            let a = match addend {
                Some(a) => a,
                None => {
                    let b = body.get_or_insert_with(|| u.bytes(si));
                    let w = rel_width(ty);
                    let Some(p) = usize::try_from(off).ok().and_then(|o| b.get(o..o + w)) else {
                        continue;
                    };
                    let mut v = [0u8; 4];
                    v[..w].copy_from_slice(p);
                    let x = u32::from_le_bytes(v);
                    i64::from(match w {
                        1 => i32::from(x as u8 as i8),
                        2 => i32::from(x as u16 as i16),
                        _ => x as i32,
                    })
                }
            };
            let named = value.wrapping_add_signed(a);
            let Some(d) = f.def_at(k, named) else { continue };
            if machine != EM_AARCH64 && !x86_exact(machine, ty, exec) {
                out.not_judged += 1;
                continue;
            }
            let hit = Hit {
                sec: si,
                offset: off,
                t: k,
                addr: named,
                def: d.name.clone(),
                boundary: f.boundary(k, d, named),
                by: By::Rule { ty },
            };
            cands.push((hit, machine == EM_AARCH64 && a64_access(ty)));
        }
    }
    let selfgot = out.selfgot;
    let keep = |h: &Hit, access: bool| !h.boundary || access || !selfgot;
    out.hits.extend(cands.into_iter().filter(|(h, a)| keep(h, *a)).map(|(h, _)| h));
    if machine == EM_AARCH64 && out.pre {
        analyse(u, t, &f, &mut out);
    }
    out
}

// ---------------------------------------------------------------------------
// The analysis (D-117 items 1-9, AArch64)
// ---------------------------------------------------------------------------

/// A register's value: an address `(T, a)`, and the relocation that
/// formed it (section, offset, type) for the message.
#[derive(Debug, Clone, Copy)]
struct Val {
    t: usize,
    a: u64,
    from: (usize, u64, u32),
}

impl PartialEq for Val {
    /// The meet compares addresses only (item 8).
    fn eq(&self, o: &Val) -> bool {
        self.t == o.t && self.a == o.a
    }
}

/// A register in a state: no value, a value, or (before item 8's
/// iteration reaches it) every value, the top of the meet.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Reg {
    None,
    Top,
    Val(Val),
}

type State = [Reg; 31];

const EMPTY: State = [Reg::None; 31];
const TOP: State = [Reg::Top; 31];

fn kill_calls(s: &mut State) {
    for r in s.iter_mut().take(19) {
        *r = Reg::None;
    }
    s[30] = Reg::None;
}

fn kill(s: &mut State, r: u32) {
    set(s, r, None);
}

/// Register 31 never has a state.
fn set(s: &mut State, r: u32, v: Option<Val>) {
    if let Some(x) = s.get_mut(r as usize) {
        *x = v.map_or(Reg::None, Reg::Val);
    }
}

fn bits(w: u32, hi: u32, lo: u32) -> u32 {
    (w >> lo) & ((1 << (hi - lo + 1)) - 1)
}

fn sext(v: u32, width: u32) -> i64 {
    let sh = 64 - width;
    (i64::from(v) << sh) >> sh
}

/// The control-flow class of a word (item 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    /// Any other word: the next one.
    Next,
    /// `B`: the target (byte offset from the word), or none.
    Jump(Option<i64>),
    /// A conditional: the next word and the target, if any.
    Cond(Option<i64>),
    /// `BR`*: no successor, and the function's leaders start empty.
    Br,
    /// `RET`*, `ERET`, `DRPS`: no successor.
    Ret,
}

fn flow(w: u32, relocated: bool) -> Flow {
    let target = |imm: u32, width: u32| (!relocated).then(|| sext(imm, width) * 4);
    if bits(w, 30, 26) == 0b00101 {
        return if w >> 31 == 0 { Flow::Jump(target(bits(w, 25, 0), 26)) } else { Flow::Next };
    }
    if w & 0xff00_0000 == 0x5400_0000 || bits(w, 30, 25) == 0b01_1010 {
        return Flow::Cond(target(bits(w, 23, 5), 19));
    }
    if bits(w, 30, 25) == 0b01_1011 {
        return Flow::Cond(target(bits(w, 18, 5), 14));
    }
    if bits(w, 31, 25) == 0b110_1011 {
        return match bits(w, 24, 21) {
            0b0000 | 0b1000 => Flow::Br,
            0b0010 | 0b0100 | 0b0101 => Flow::Ret,
            _ => Flow::Next,
        };
    }
    Flow::Next
}

/// A load/store form of item 5.
#[derive(Debug, Clone, Copy)]
struct LdSt {
    rn: u32,
    /// The access's offset from Rn (0 for the forms without offset).
    offset: i64,
    /// Post-index: the access is at Rn, the write-back adds `offset`.
    post: bool,
    writeback: bool,
    /// `PRFM`/`PRFUM`: accesses nothing.
    prefetch: bool,
    /// General registers it loads (or, for exclusives, writes).
    kills: [Option<u32>; 4],
}

/// Item 5's load/store forms; `None` for any other word of the group.
fn ldst(w: u32) -> Option<LdSt> {
    let op = bits(w, 29, 24);
    let rn = bits(w, 9, 5);
    let rt = bits(w, 4, 0);
    let v = bits(w, 26, 26) == 1;
    let size = bits(w, 31, 30);
    let opc = bits(w, 23, 22);
    let load = !v && opc != 0;
    let mut l =
        LdSt { rn, offset: 0, post: false, writeback: false, prefetch: false, kills: [None; 4] };
    if op & 0b11_1011 == 0b11_1001 {
        // Unsigned offset.
        let scale = if v && size == 0 && opc & 2 != 0 { 4 } else { size };
        l.offset = i64::from(bits(w, 21, 10)) << scale;
        l.prefetch = !v && size == 3 && opc == 2;
        if load && !l.prefetch {
            l.kills[0] = Some(rt);
        }
        return Some(l);
    }
    if op & 0b11_1011 == 0b11_1000 && bits(w, 21, 21) == 0 {
        let mode = bits(w, 11, 10);
        l.offset = sext(bits(w, 20, 12), 9);
        l.post = mode == 0b01;
        l.writeback = mode == 0b01 || mode == 0b11;
        l.prefetch = !v && size == 3 && opc == 2 && mode == 0;
        if load && !l.prefetch {
            l.kills[0] = Some(rt);
        }
        return Some(l);
    }
    if bits(w, 29, 27) == 0b101 && bits(w, 25, 25) == 0 {
        // Pair: opc is w[31:30], L w[22].
        let popc = size;
        let scale = if v {
            2 + popc
        } else if popc == 0b10 {
            3
        } else if popc == 0b01 && bits(w, 22, 22) == 0 {
            4 // STGP
        } else {
            2
        };
        l.offset = sext(bits(w, 21, 15), 7) << scale;
        let mode = bits(w, 24, 23);
        l.post = mode == 0b01;
        l.writeback = mode == 0b01 || mode == 0b11;
        if !v && bits(w, 22, 22) == 1 {
            l.kills[0] = Some(rt);
            l.kills[1] = Some(bits(w, 14, 10));
        }
        return Some(l);
    }
    if op == 0b00_1000 {
        // Exclusive, ordered, compare-and-swap.
        let rs = bits(w, 20, 16);
        l.kills = [Some(rt), Some(bits(w, 14, 10)), Some(rs), Some(rs + 1)];
        return Some(l);
    }
    if op & 0b11_1011 == 0b11_1000 && bits(w, 21, 21) == 1 && bits(w, 11, 10) == 0 {
        // Atomic memory operations.
        if !v {
            l.kills[0] = Some(rt);
        }
        return Some(l);
    }
    None
}

/// `LDR` (literal): the general register it loads, and whether it is the
/// 64-bit `LDR Xt` (a defining form).
fn ldr_literal(w: u32) -> Option<(Option<u32>, bool)> {
    if bits(w, 29, 24) & 0b11_1011 != 0b01_1000 {
        return None;
    }
    let v = bits(w, 26, 26) == 1;
    let x = !v && bits(w, 31, 30) == 0b01;
    Some(((!v).then_some(bits(w, 4, 0)), x))
}

/// One section's relocations by offset, and its mapping symbols, read
/// once per object when the analysis runs.
#[derive(Default)]
struct SecInfo<'a> {
    relocs: BTreeMap<u64, (u32, u32, i64)>,
    bytes: Option<Bytes<'a>>,
}

struct Ctx<'u, 'a, U: Unit<'a>> {
    u: &'u U,
    t: &'u SymbolTable<'a>,
    f: &'u Facts,
    secs: HashMap<usize, SecInfo<'a>>,
}

impl<'a, U: Unit<'a>> Ctx<'_, 'a, U> {
    fn info(&mut self, si: usize) -> &SecInfo<'a> {
        if !self.secs.contains_key(&si) {
            let mut s = SecInfo::default();
            if let Some(rs) = self.u.reloc_sec(si)
                && let Some(recs) = self.u.recs(rs)
            {
                for (off, sym, ty, a) in recs {
                    // The first relocation at a word decides it.
                    s.relocs.entry(off).or_insert((sym, ty, a.unwrap_or(0)));
                }
            }
            s.bytes = Some(self.u.bytes(si));
            self.secs.insert(si, s);
        }
        &self.secs[&si]
    }

    /// The value an `ABS64` at `(sec, off)` gives, when it names a local
    /// of a section holding a definition (item 4's literal forms).
    fn literal(&mut self, sec: usize, off: u64) -> Option<Val> {
        let (sym, ty, a) = *self.info(sec).relocs.get(&off)?;
        if ty != R_AARCH64_ABS64 {
            return None;
        }
        let (t, v) = local_in_t(self.t, self.f, sym)?;
        Some(Val { t, a: v.wrapping_add_signed(a), from: (sec, off, ty) })
    }
}

/// The mapping symbols of every section, from the symbol table.
fn mapping_symbols(t: &SymbolTable<'_>) -> HashMap<usize, Vec<(u64, bool)>> {
    let mut m: HashMap<usize, Vec<(u64, bool)>> = HashMap::new();
    for (i, s) in t.locals() {
        let Ok(n) = t.name(&s) else { continue };
        let data = n == b"$d" || n.starts_with(b"$d.");
        let code = n == b"$x" || n.starts_with(b"$x.");
        if !(data || code) {
            continue;
        }
        if let Some(k) = sec_of(t, i, &s) {
            m.entry(k).or_default().push((s.st_value, data));
        }
    }
    for v in m.values_mut() {
        // Stable: at one offset the last symbol in table order decides.
        v.sort_by_key(|&(o, _)| o);
    }
    m
}

fn is_data(maps: &[(u64, bool)], off: u64) -> bool {
    let i = maps.partition_point(|&(o, _)| o <= off);
    i > 0 && maps[i - 1].1
}

/// The judged addresses of one word: `(address, access, from)`.
type Judged = Vec<(Val, bool)>;

/// Item 5-7: the effect of word `w` at `off` of section `si` on `s`; the
/// judged addresses when `judge` is set.
#[allow(clippy::too_many_lines)] // the decoding table, row by row
fn step<'a, U: Unit<'a>>(
    c: &mut Ctx<'_, 'a, U>,
    si: usize,
    off: u64,
    w: u32,
    data: bool,
    s: &mut State,
    judged: &mut Judged,
) {
    if data {
        *s = EMPTY;
        return;
    }
    let rel = c.info(si).relocs.get(&off).copied();
    let rd = bits(w, 4, 0);
    // Item 4: the defining forms (a relocation at the word naming a local
    // of a section holding a definition).
    if let Some((sym, ty, a)) = rel {
        let def = |c: &Ctx<'_, 'a, U>| {
            local_in_t(c.t, c.f, sym).map(|(t, v)| Val {
                t,
                a: v.wrapping_add_signed(a),
                from: (si, off, ty),
            })
        };
        let add_imm = bits(w, 28, 23) == 0b10_0010;
        if ty == R_AARCH64_ADD_ABS_LO12_NC
            && add_imm
            && w >> 29 == 0b100
            && rd != 31
            && let Some(v) = def(c)
        {
            s[rd as usize] = Reg::Val(v);
            return;
        }
        if ty == R_AARCH64_ADR_PREL_LO21
            && bits(w, 28, 24) == 0b1_0000
            && w >> 31 == 0
            && rd != 31
            && let Some(v) = def(c)
        {
            s[rd as usize] = Reg::Val(v);
            return;
        }
        let ldr_x_unsigned = bits(w, 29, 24) & 0b11_1011 == 0b11_1001
            && bits(w, 31, 30) == 3
            && bits(w, 23, 22) == 0b01
            && bits(w, 26, 26) == 0;
        let ldr_x_literal = ldr_literal(w).is_some_and(|(_, x)| x);
        if (ty == R_AARCH64_LDST64_ABS_LO12_NC && ldr_x_unsigned)
            || (ty == R_AARCH64_LD_PREL_LO19 && ldr_x_literal)
        {
            // z: any symbol, defined in a section of the object.
            let i = SymbolIndex(sym);
            let z = c.t.get(i).ok().and_then(|z| sec_of(c.t, i, &z).map(|k| (k, z.st_value)));
            let v = z.and_then(|(k, zv)| c.literal(k, zv.wrapping_add_signed(a)));
            set(s, rd, v);
            return;
        }
    } else if let Some((Some(rt), true)) = ldr_literal(w) {
        // The literal form without a relocation: z is the word's own
        // place plus imm19 x 4.
        let z = off.wrapping_add_signed(sext(bits(w, 23, 5), 19) * 4);
        let v = c.literal(si, z);
        set(s, rt, v);
        return;
    }
    let relocated = rel.is_some();
    // Item 5, first match.
    if bits(w, 30, 26) == 0b00101 {
        if w >> 31 == 1 {
            kill_calls(s);
        }
        return;
    }
    if w & 0xff00_0000 == 0x5400_0000 || matches!(bits(w, 30, 25), 0b01_1010 | 0b01_1011) {
        return;
    }
    if bits(w, 31, 25) == 0b110_1011 {
        match bits(w, 24, 21) {
            0b0000 | 0b1000 | 0b0010 | 0b0100 | 0b0101 => {}
            0b0001 | 0b1001 => kill_calls(s),
            _ => {
                kill(s, rd);
                s[30] = Reg::None;
            }
        }
        return;
    }
    if bits(w, 28, 23) == 0b10_0010 {
        // ADD/SUB (immediate); item 6 when not relocated.
        let rn = bits(w, 9, 5);
        let sf = w >> 31 == 1;
        let base = if rn == 31 { Reg::None } else { s[rn as usize] };
        match base {
            Reg::Val(v) if !relocated && sf && rd != 31 => {
                let imm = u64::from(bits(w, 21, 10)) << if bits(w, 22, 22) == 1 { 12 } else { 0 };
                let a = if bits(w, 30, 30) == 1 {
                    v.a.wrapping_sub(imm)
                } else {
                    v.a.wrapping_add(imm)
                };
                let nv = Val { a, ..v };
                judged.push((nv, false));
                s[rd as usize] = Reg::Val(nv);
            }
            // Every value plus an immediate is every value.
            Reg::Top if !relocated && sf && rd != 31 => s[rd as usize] = Reg::Top,
            _ => kill(s, rd),
        }
        return;
    }
    if bits(w, 28, 24) == 0b1_0000 {
        kill(s, rd);
        return;
    }
    if w & 0xffe0_ffe0 == 0xaa00_03e0 {
        let rm = bits(w, 20, 16);
        if rd != 31 {
            s[rd as usize] = if relocated {
                Reg::None
            } else {
                s.get(rm as usize).copied().unwrap_or(Reg::None)
            };
        }
        return;
    }
    if bits(w, 28, 25) & 0b0101 == 0b0100 {
        if let Some((rt, _)) = ldr_literal(w) {
            if let Some(rt) = rt {
                kill(s, rt);
            }
            return;
        }
        let Some(l) = ldst(w) else {
            *s = EMPTY;
            return;
        };
        let base = if l.rn == 31 { Reg::None } else { s[l.rn as usize] };
        match base {
            Reg::Val(v) if !relocated => {
                if !l.prefetch {
                    let at = if l.post { v.a } else { v.a.wrapping_add_signed(l.offset) };
                    judged.push((Val { a: at, ..v }, true));
                }
                if l.writeback {
                    let nv = Val { a: v.a.wrapping_add_signed(l.offset), ..v };
                    judged.push((nv, false));
                    s[l.rn as usize] = Reg::Val(nv);
                }
            }
            // A write-back of every value keeps every value.
            Reg::Top if !relocated => {}
            _ => {
                if l.writeback {
                    kill(s, l.rn);
                }
            }
        }
        for r in l.kills.into_iter().flatten() {
            kill(s, r);
        }
        return;
    }
    if bits(w, 28, 25) & 0b1110 == 0b1010 {
        kill(s, rd);
        s[30] = Reg::None;
        return;
    }
    if matches!(bits(w, 28, 25), 0b0000 | 0b0001 | 0b0011) {
        *s = EMPTY;
        return;
    }
    kill(s, rd);
}

/// A register keeps its value only when every predecessor has it (item
/// 8); the top is the meet's identity.
fn meet(a: &State, b: &State) -> State {
    let mut m = EMPTY;
    for r in 0..31 {
        m[r] = match (a[r], b[r]) {
            (Reg::Top, x) | (x, Reg::Top) => x,
            (Reg::Val(x), Reg::Val(y)) if x == y => Reg::Val(x),
            _ => Reg::None,
        };
    }
    m
}

/// Item 8's passes over the words before the judging pass: enough for
/// any function a compiler writes; a function that does not settle is
/// judged from the states of the last pass.
const MAX_PASSES: usize = 64;

/// Items 2-9 over one object that passed the pre-filter.
fn analyse<'a>(u: &impl Unit<'a>, t: &SymbolTable<'a>, f: &Facts, out: &mut Scan) {
    let mut c = Ctx { u, t, f, secs: HashMap::new() };
    let maps = mapping_symbols(t);
    // Item 2: the distinct functions, with a name for the messages.
    let mut funcs: BTreeMap<(usize, u64, u64), String> = BTreeMap::new();
    for (i, s) in t.iter() {
        if s.sym_type() != STT_FUNC || s.st_size == 0 {
            continue;
        }
        let Some(k) = sec_of(t, i, &s) else { continue };
        if k >= u.count() {
            continue;
        }
        let sh = u.shdr(k);
        if sh.sh_flags & SHF_EXECINSTR == 0 || sh.sh_type == SHT_NOBITS || !u.live(k) {
            continue;
        }
        let name = t.name(&s).map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_default();
        funcs.entry((k, s.st_value, s.st_value + s.st_size)).or_insert(name);
    }
    let none: Vec<(u64, bool)> = Vec::new();
    for ((si, lo, hi), name) in funcs {
        let n = usize::try_from((hi - lo) / 4).unwrap_or(0);
        let bytes = c.info(si).bytes.clone().unwrap_or(Bytes::Ref(&[]));
        let ms = maps.get(&si).unwrap_or(&none);
        let mut words = Vec::with_capacity(n);
        for k in 0..n {
            let off = lo + 4 * k as u64;
            let w = usize::try_from(off)
                .ok()
                .and_then(|o| bytes.get(o..o + 4))
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
            words.push((off, w, is_data(ms, off)));
        }
        out.funcs += 1;
        out.words += n as u64;
        let hits = function(&mut c, si, &words);
        for (k, v, access) in hits {
            let Some(d) = f.def_at(v.t, v.a) else { continue };
            let boundary = f.boundary(v.t, d, v.a);
            if boundary && !access && out.selfgot {
                continue;
            }
            let at = words[k].0;
            out.hits.push(Hit {
                sec: si,
                offset: at,
                t: v.t,
                addr: v.a,
                def: d.name.clone(),
                boundary,
                by: By::Analysis { func: name.clone(), at: at - lo, access, from: v.from },
            });
        }
    }
}

/// Items 5-8 over one function's words `(offset, word, is_data)`: the
/// judged addresses `(word index, value, access)` of the final pass.
#[allow(clippy::too_many_lines, clippy::needless_range_loop)] // item 8, in its order
fn function<'a, U: Unit<'a>>(
    c: &mut Ctx<'_, 'a, U>,
    si: usize,
    words: &[(u64, Option<u32>, bool)],
) -> Vec<(usize, Val, bool)> {
    let n = words.len();
    let relocated = |c: &mut Ctx<'_, 'a, U>, off: u64| c.info(si).relocs.contains_key(&off);
    // Successors and leaders (item 8). A word that cannot be read or is
    // data falls through.
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut leader = vec![false; n];
    let mut has_br = false;
    if n > 0 {
        leader[0] = true;
    }
    for k in 0..n {
        let (off, w, data) = words[k];
        let fl = match w {
            Some(w) if !data => flow(w, relocated(c, off)),
            _ => Flow::Next,
        };
        let inside = |d: i64| {
            let x = i64::try_from(k).ok()? + d / 4;
            usize::try_from(x).ok().filter(|&x| x < n && d % 4 == 0)
        };
        let next = (k + 1 < n).then_some(k + 1);
        match fl {
            Flow::Next => succ[k].extend(next),
            Flow::Jump(d) => {
                if let Some(x) = d.and_then(inside) {
                    succ[k].push(x);
                    leader[x] = true;
                }
            }
            Flow::Cond(d) => {
                succ[k].extend(next);
                if let Some(x) = d.and_then(inside) {
                    succ[k].push(x);
                    leader[x] = true;
                }
            }
            Flow::Br => has_br = true,
            Flow::Ret => {}
        }
        if !matches!(fl, Flow::Next)
            && let Some(x) = next
        {
            leader[x] = true;
        }
    }
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (k, ss) in succ.iter().enumerate() {
        for &x in ss {
            if !preds[x].contains(&k) {
                preds[x].push(k);
            }
        }
    }
    for p in &mut preds {
        p.sort_unstable();
    }
    let entry_of = |k: usize, exit: &[Option<State>]| -> State {
        if k == 0 || (has_br && leader[k]) || preds[k].is_empty() {
            return EMPTY;
        }
        // Predecessors not yet computed are ignored: with none computed,
        // the entry is the top (the greatest fixed point).
        let mut m: Option<State> = None;
        for &p in &preds[k] {
            if let Some(e) = &exit[p] {
                m = Some(match m {
                    None => *e,
                    Some(x) => meet(&x, e),
                });
            }
        }
        m.unwrap_or(TOP)
    };
    let mut exit: Vec<Option<State>> = vec![None; n];
    let mut scratch = Vec::new();
    for _ in 0..MAX_PASSES {
        let mut changed = false;
        for k in 0..n {
            let mut s = entry_of(k, &exit);
            let (off, w, data) = words[k];
            match w {
                Some(w) => step(c, si, off, w, data, &mut s, &mut scratch),
                None => s = EMPTY,
            }
            scratch.clear();
            if exit[k].as_ref() != Some(&s) {
                exit[k] = Some(s);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut out = Vec::new();
    for k in 0..n {
        let mut s = entry_of(k, &exit);
        let (off, w, data) = words[k];
        if let Some(w) = w {
            let mut j = Vec::new();
            step(c, si, off, w, data, &mut s, &mut j);
            out.extend(j.into_iter().map(|(v, a)| (k, v, a)));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// P-D17's address clause over a link
// ---------------------------------------------------------------------------

/// The address clause over every live object of a `Shared` link: each
/// bound use a P-D17 finding, naming the definition, the object, the
/// place and the relocation (D-117's diagnostic), with the remedy and,
/// for a boundary address or an address before an object, the
/// `-fdata-sections` note. The x86 code fields the scope does not judge
/// are counted in the second value.
pub(crate) fn address_clause(objs: &[Obj<'_>], sets: &crate::shared::Sets) -> (Vec<Finding>, u64) {
    let mut v = Vec::new();
    let mut not_judged = 0;
    let pre = |n: &str| sets.preemptible(n);
    for o in objs {
        let s = scan(o, &pre);
        not_judged += s.not_judged;
        let machine = o.machine();
        for h in &s.hits {
            v.push(finding(o, machine, h));
        }
    }
    (v, not_judged)
}

fn rname(machine: u16, ty: u32) -> String {
    mortar_elf::reloc::name(machine, ty)
        .map_or_else(|| format!("relocation type {ty}"), str::to_string)
}

fn finding(o: &Obj<'_>, machine: u16, h: &Hit) -> Finding {
    let sec = |k: usize| o.secs.get(k).map_or("?", |s| s.name.as_str());
    let named = format!("{}+{:#x}", sec(h.t), h.addr);
    let note = if h.boundary {
        " (a boundary address: put each object in its own section, -fdata-sections)"
    } else {
        " (or, for an address before an object, -fdata-sections; do not form an address before an object)"
    };
    let msg = match &h.by {
        By::Rule { ty } => format!(
            "the address clause (D-117): {} at {}+{:#x} of {} names {named}, inside the preemptible \
             '{}': a bound use, non-pic-in-shared-object; recompile with -fPIC{note}",
            rname(machine, *ty),
            sec(h.sec),
            h.offset,
            o.label(),
            h.def,
        ),
        By::Analysis { func, at, access, from } => format!(
            "the address clause (D-117 analysis): {func}+{at:#x} of {} {} {named}, inside the \
             preemptible '{}', from {} at {}+{:#x}: a bound use, non-pic-in-shared-object; \
             recompile with -fPIC{note}",
            o.label(),
            if *access { "accesses" } else { "computes" },
            h.def,
            rname(machine, from.2),
            sec(from.0),
            from.1,
        ),
    };
    Finding::new(Rule::Dyn(17), msg).site(o.site(h.sec, h.offset))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mortar_elf::Object;
    use mortar_elf::consts::{SHT_REL, SHT_RELA, STV_DEFAULT};

    /// An object read whole, for the oracle comparison: every section is
    /// live.
    struct Raw<'a> {
        elf: Object<'a>,
        symtab: Option<SymbolTable<'a>>,
        shdrs: Vec<Shdr>,
        names: Vec<String>,
        relocs: Vec<Option<u32>>,
    }

    impl<'a> Raw<'a> {
        fn parse(b: &'a [u8]) -> Option<Raw<'a>> {
            let elf = Object::parse(b).ok()?;
            let symtab = elf.symbols().ok()?;
            let mut shdrs = Vec::new();
            let mut names = Vec::new();
            for s in elf.sections() {
                shdrs.push(*s.header());
                names.push(String::from_utf8_lossy(s.name().unwrap_or(b"")).into_owned());
            }
            let mut relocs = vec![None; shdrs.len()];
            for (i, h) in shdrs.iter().enumerate() {
                let target = h.sh_info as usize;
                if matches!(h.sh_type, SHT_RELA | SHT_REL) && target < shdrs.len() && target != i {
                    relocs[target] = Some(i as u32);
                }
            }
            Some(Raw { elf, symtab, shdrs, names, relocs })
        }
    }

    impl<'a> Unit<'a> for Raw<'a> {
        fn machine(&self) -> u16 {
            self.elf.header().e_machine
        }
        fn symtab(&self) -> Option<&SymbolTable<'a>> {
            self.symtab.as_ref()
        }
        fn count(&self) -> usize {
            self.shdrs.len()
        }
        fn shdr(&self, si: usize) -> &Shdr {
            &self.shdrs[si]
        }
        fn sec_name(&self, si: usize) -> &str {
            &self.names[si]
        }
        fn reloc_sec(&self, si: usize) -> Option<u32> {
            self.relocs[si]
        }
        fn live(&self, _: usize) -> bool {
            true
        }
        fn bytes(&self, si: usize) -> Bytes<'a> {
            if self.shdrs[si].sh_type == SHT_NOBITS {
                return Bytes::Ref(&[]);
            }
            let d = self.elf.section(mortar_elf::SectionIndex(si as u32)).and_then(|s| s.data());
            Bytes::Ref(d.unwrap_or(&[]))
        }
        fn recs(&self, rs: u32) -> Option<Vec<RawRecord>> {
            let s = self.elf.section(mortar_elf::SectionIndex(rs)).ok()?;
            if s.sh_type() == SHT_RELA {
                Some(
                    s.rela()
                        .ok()?
                        .iter()
                        .map(|x| (x.r_offset, x.r_sym, x.r_type, Some(x.r_addend)))
                        .collect(),
                )
            } else {
                Some(s.rel().ok()?.iter().map(|x| (x.r_offset, x.r_sym, x.r_type, None)).collect())
            }
        }
    }

    /// The oracle's line for one object (`s1d.py --hits`'s format).
    fn report(label: &str, r: &Raw<'_>, x86: bool, out: &mut String) {
        use std::fmt::Write;
        let machine = r.machine();
        if machine != EM_AARCH64 && !x86 {
            return;
        }
        // The default line: every global, weak or unique default-visibility
        // definition is exported and preemptible.
        let mut pre: std::collections::HashSet<String> = std::collections::HashSet::new();
        if let Some(t) = r.symtab() {
            for (i, s) in t.iter() {
                if s.binding() != STB_LOCAL
                    && s.visibility() == STV_DEFAULT
                    && sec_of(t, i, &s).is_some()
                    && let Ok(n) = t.name(&s)
                {
                    pre.insert(String::from_utf8_lossy(n).into_owned());
                }
            }
        }
        let sc = scan(r, &|n: &str| pre.contains(n));
        let rule = sc.hits.iter().filter(|h| matches!(h.by, By::Rule { .. })).count();
        let ana = sc.hits.len() - rule;
        let _ = writeln!(
            out,
            "{label} pre={} rule={rule} ana={ana} words={} funcs={}{}",
            u8::from(sc.pre),
            sc.words,
            sc.funcs,
            if sc.selfgot { " selfgot" } else { "" }
        );
        let mut seen = std::collections::HashSet::new();
        for h in &sc.hits {
            let named = format!("{}+{:#x}", r.names[h.t], h.addr);
            let line = match &h.by {
                By::Rule { ty } => format!(
                    "    RULE {}+{:#x} reloc{ty} {named} {} {}",
                    r.names[h.sec],
                    h.offset,
                    h.def,
                    if h.boundary { "boundary" } else { "" }
                ),
                By::Analysis { func, at, access, .. } => format!(
                    "    ANA {func}+{at:#x} {} {named} {} {}",
                    if *access { "ldst" } else { "add" },
                    h.def,
                    if h.boundary { "boundary" } else { "" }
                ),
            };
            if seen.insert(line.clone()) {
                out.push_str(&line);
                out.push('\n');
            }
        }
    }

    /// Off the gate: `SHAREDFLOW_LIST` names a file of object and archive
    /// paths (`exp3/lists/*.lst`); every AArch64 object (and x86 ones with
    /// `SHAREDFLOW_X86`) is reported to `SHAREDFLOW_OUT` in `s1d.py
    /// --hits`'s format, for a diff against the oracle.
    #[test]
    #[ignore = "off the gate: the comparison with exp3/s1d.py"]
    fn oracle_lists() {
        let (Ok(list), Ok(dest)) =
            (std::env::var("SHAREDFLOW_LIST"), std::env::var("SHAREDFLOW_OUT"))
        else {
            return;
        };
        let x86 = std::env::var("SHAREDFLOW_X86").is_ok();
        let mut out = String::new();
        for path in std::fs::read_to_string(list).unwrap().split_whitespace() {
            let Ok(bytes) = std::fs::read(path) else {
                out.push_str(path);
                out.push_str(": cannot be read\n");
                continue;
            };
            if mortar_elf::archive::Archive::sniff(&bytes).is_some() {
                let Ok(a) = mortar_elf::archive::Archive::parse(&bytes) else { continue };
                for m in a.members().flatten() {
                    let Some(d) = m.data() else { continue };
                    if let Some(r) = Raw::parse(d) {
                        let label = format!("{path}({})", String::from_utf8_lossy(m.name()));
                        report(&label, &r, x86, &mut out);
                    }
                }
            } else if let Some(r) = Raw::parse(&bytes) {
                report(path, &r, x86, &mut out);
            }
        }
        std::fs::write(dest, out).unwrap();
    }

    #[test]
    fn d117_flow_classes() {
        // b .+8; bl; b.eq .-4; cbz; tbz; br x1; ret; blr x2.
        assert_eq!(flow(0x1400_0002, false), Flow::Jump(Some(8)));
        assert_eq!(flow(0x1400_0002, true), Flow::Jump(None));
        assert_eq!(flow(0x9400_0000, false), Flow::Next);
        assert_eq!(flow(0x54ff_ffe0, false), Flow::Cond(Some(-4)));
        assert_eq!(flow(0xb400_0040, false), Flow::Cond(Some(8)));
        assert_eq!(flow(0x3600_0040, false), Flow::Cond(Some(8)));
        assert_eq!(flow(0xd61f_0020, false), Flow::Br);
        assert_eq!(flow(0xd65f_03c0, false), Flow::Ret);
        assert_eq!(flow(0xd63f_0040, false), Flow::Next);
    }

    #[test]
    fn d117_load_store_forms() {
        // ldr w1, [x0, #8]: unsigned offset, scale 2.
        let l = ldst(0xb940_0801).unwrap();
        assert_eq!((l.rn, l.offset, l.kills[0]), (0, 8, Some(1)));
        // str x1, [x0, #-16]!: pre-index write-back, a store.
        let l = ldst(0xf81f_0c01).unwrap();
        assert_eq!((l.offset, l.writeback, l.post, l.kills[0]), (-16, true, false, None));
        // ldp x2, x3, [x0], #16: post-index pair, loads both.
        let l = ldst(0xa8c1_0c02).unwrap();
        assert_eq!((l.offset, l.post, l.kills[0], l.kills[1]), (16, true, Some(2), Some(3)));
        // prfm pldl1keep, [x0, #8]: accesses nothing.
        assert!(ldst(0xf980_0400).unwrap().prefetch);
        // ldr q0, [x0, #32]: 128-bit SIMD, scale 4, no general register.
        let l = ldst(0x3dc0_0800).unwrap();
        assert_eq!((l.offset, l.kills[0]), (32, None));
        // ldr x0, [x1, x2]: register offset, another word of the group.
        assert!(ldst(0xf862_6820).is_none());
    }
}

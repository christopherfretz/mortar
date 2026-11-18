//! The relocation engine (`spec/plumb.md` §4, §5): for every relocation
//! record of every placed input section, the value the place must hold,
//! recomputed from S, A and P, and compared with the output.
//!
//! One pass over the records. Lookups are by index, by binary search or by
//! hash; groups are kept in ordered maps so nothing depends on hash order.

use std::collections::BTreeMap;

use crate::hasher::HashMap;

use mortar_elf::consts::{
    PT_TLS, SHF_COMPRESSED, SHF_STRINGS, SHF_TLS, SHT_NOBITS, STB_GLOBAL, STB_GNU_UNIQUE, STB_WEAK,
    STT_GNU_IFUNC, STT_SECTION, STT_TLS,
};
use mortar_elf::{Machine, SymbolIndex, SymbolSection};

use crate::a64::{self, Check, Def, Field, Form, Op, Slot};
use crate::image::Image;
use crate::locate::{Obj, Place};
use crate::map::Format;
#[path = "debug.rs"]
pub(crate) mod debug;
#[path = "dsoword.rs"]
pub(crate) mod dsoword;
#[path = "eh.rs"]
pub(crate) mod eh;
#[path = "ehowner.rs"]
mod ehowner;
#[path = "ifunc.rs"]
mod ifunc;
#[path = "x64rec.rs"]
mod x64rec;
#[path = "x86rec.rs"]
mod x86rec;
pub(crate) use ifunc::IfKey;

use crate::report::{Coverage, Finding, Reason, Rewrite, Rule, Site, Via};

/// The width of a relocated place in bytes (§4.2).
pub(crate) fn width(machine: Machine, r_type: u32) -> u8 {
    match machine {
        Machine::AArch64 => {
            a64::def(r_type).map_or_else(|| a64::unchecked_width(r_type), |d| d.field.width())
        }
        Machine::X86_64 => crate::x64::def(r_type)
            .map_or_else(|| crate::x64::unchecked_width(r_type), |d| d.field.width()),
        Machine::I386 => crate::x86::def(r_type)
            .map_or_else(|| crate::x86::unchecked_width(r_type), |d| d.field.width()),
    }
}

/// The output memory occupied by placed input sections, as disjoint
/// sorted ranges (§5.4: a veneer, a patch or a GOT slot is outside them).
#[derive(Debug, Default)]
pub(crate) struct Occupied(Vec<(u64, u64)>);

impl Occupied {
    pub fn build(objs: &[Obj<'_>]) -> Occupied {
        let mut v: Vec<(u64, u64)> = Vec::new();
        for o in objs {
            for s in &o.secs {
                // `.tbss` is in the TLS template only: no memory of the image.
                if !s.alloc() || s.tbss() {
                    continue;
                }
                match &s.place {
                    Place::Whole(a) if s.shdr.sh_size > 0 => {
                        v.push((*a, a.saturating_add(s.shdr.sh_size)));
                    }
                    Place::Pieces(ps) => {
                        v.extend(
                            ps.iter()
                                .filter(|p| p.size > 0)
                                .map(|p| (p.addr, p.addr.saturating_add(p.size))),
                        );
                    }
                    _ => {}
                }
            }
        }
        v.sort_unstable();
        let mut out: Vec<(u64, u64)> = Vec::with_capacity(v.len());
        for (s, e) in v {
            match out.last_mut() {
                Some(last) if s <= last.1 => last.1 = last.1.max(e),
                _ => out.push((s, e)),
            }
        }
        Occupied(out)
    }

    pub fn contains(&self, addr: u64) -> bool {
        let i = self.0.partition_point(|r| r.0 <= addr);
        i > 0 && addr < self.0[i - 1].1
    }

    /// The parts of `[lo, hi)` no placed section occupies, in order.
    pub fn gaps(&self, lo: u64, hi: u64) -> Vec<(u64, u64)> {
        let mut v = Vec::new();
        let mut at = lo;
        let i = self.0.partition_point(|r| r.1 <= lo);
        for &(s, e) in &self.0[i..] {
            if s >= hi {
                break;
            }
            if s > at {
                v.push((at, s));
            }
            at = at.max(e);
        }
        if at < hi {
            v.push((at, hi));
        }
        v
    }
}

/// The output's static TLS block (§4.7): `PT_TLS` and the offset of its
/// start from the thread pointer.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TlsBlock {
    /// `p_vaddr` of `PT_TLS`.
    pub start: u64,
    /// TPREL of the block's first byte: `align_up(16, p_align)` on
    /// AArch64, `-align_up(p_memsz, p_align)` on x86-64 (two's complement).
    pub tcb: u64,
}

impl TlsBlock {
    /// The block of the output, if it has one `PT_TLS`; `Err` names why
    /// the TP offsets cannot be judged (§4.7).
    fn of(image: &Image<'_>) -> Result<Option<TlsBlock>, &'static str> {
        let mut tls = image.phdrs.iter().filter(|p| p.p_type == PT_TLS);
        let Some(t) = tls.next() else { return Ok(None) };
        if tls.next().is_some() {
            return Err("the output has more than one PT_TLS");
        }
        let align = t.p_align.max(1);
        if !align.is_power_of_two() {
            return Err("the PT_TLS alignment is not a power of two");
        }
        if t.p_vaddr % align != 0 {
            return Err("PT_TLS p_vaddr is not a multiple of p_align");
        }
        // Variant I (AArch64): the block starts align_up(16, p_align)
        // above TP. Variant II (x86-64, `spec/x64.md` X-44; i386,
        // `spec/x86.md` X86-57, the same on `%gs`): it ends at TP,
        // TpRel(x) = x - start - align_up(p_memsz, p_align).
        let tcb = if image.header.e_machine == mortar_elf::consts::EM_AARCH64 {
            16u64.next_multiple_of(align)
        } else {
            t.p_memsz
                .checked_next_multiple_of(align)
                .ok_or("PT_TLS p_memsz overflows")?
                .wrapping_neg()
        };
        Ok(Some(TlsBlock { start: t.p_vaddr, tcb }))
    }

    /// TPREL(v) for the address v of a TLS byte.
    pub fn tprel(self, v: u64) -> u64 {
        v.wrapping_sub(self.start).wrapping_add(self.tcb)
    }

    /// DTPREL(v).
    pub fn dtprel(self, v: u64) -> u64 {
        v.wrapping_sub(self.start)
    }
}

/// What a relocation's symbol resolves to (§4.4).
#[derive(Debug, Clone)]
enum Target {
    /// S.
    Addr(u64),
    /// S, the address of a thread-local symbol in the TLS template (§4.7).
    Tls(u64),
    /// An undefined weak symbol: S = 0, and the branch rewritings apply.
    UndefWeak,
    /// A symbol in a mergeable section: found from the fields (§5.6).
    Merge {
        oi: usize,
        sec: usize,
        value: u64,
        section_sym: bool,
    },
    Skip(Reason),
    /// The symbol's section is not in the output (R5).
    Discarded(String),
    /// A label of an empty section no record places and no output symbol
    /// gives (WP-143): the field must name an address in or at the end of
    /// an allocated output section.
    Empty(String),
    /// A section symbol plus addend outside its section, which is placed
    /// as pieces: no piece is named (`synth.md` S-18).
    Outside(String),
    /// An IFUNC: the resolver's address and the symbol (§4.8).
    Ifunc {
        resolver: u64,
        key: IfKey,
    },
}

/// One relocation record of a placed section.
#[derive(Debug, Clone)]
struct Rec {
    off: u64,
    sym: u32,
    def: &'static Def,
    addend: i64,
    p: u64,
    input_word: u64,
    out_word: u64,
    /// The out word is the erratum 843419 patch's (§5.3, W8).
    patched: bool,
}

/// A record waiting for its GOT group (§5.5).
#[derive(Debug, Clone)]
struct Pending {
    rec: Rec,
    site: Site,
    symbol: String,
    /// What the GOT entry of a GOT-indirect record holds.
    want: Want,
}

/// What a GOT entry must hold (§5.5, §4.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Want {
    /// One 8-byte word.
    Word(u64),
    /// Two 8-byte words (the module index and an offset).
    Pair(u64, u64),
}

impl Want {
    fn size(self) -> u64 {
        match self {
            Want::Word(_) => 8,
            Want::Pair(..) => 16,
        }
    }
    fn first(self) -> u64 {
        match self {
            Want::Word(v) | Want::Pair(v, _) => v,
        }
    }
}

/// Evidence of a GOT slot's or a piece's address from one record.
#[derive(Debug, Clone, Copy)]
enum Evidence {
    Exact(u64),
    Page(u64),
    Low(u64),
    /// `G - GOT` from a MOVW group: bits 31:16 (with the sign of a MOVN).
    MovHi(u64),
    /// `G - GOT` bits 15:0, from a `MOVK`.
    MovLo(u64),
    None,
}

#[derive(Debug, Default)]
struct Group {
    members: Vec<(Pending, Evidence)>,
}

impl Group {
    /// As [`Group::candidates`], with the GOT base for MOVW evidence.
    fn candidates_from(&self, got: Option<u64>) -> Vec<u64> {
        candidates_of(self.members.iter().map(|(_, e)| e), got)
    }

    /// Whether the evidence is one-sided: pages with no low part or the
    /// reverse, and no exact value.
    fn unpaired(&self) -> bool {
        let has = |f: fn(&Evidence) -> bool| self.members.iter().any(|(_, e)| f(e));
        !has(|e| matches!(e, Evidence::Exact(_)))
            && (has(|e| matches!(e, Evidence::Page(_))) != has(|e| matches!(e, Evidence::Low(_)))
                || has(|e| matches!(e, Evidence::MovHi(_)))
                    != has(|e| matches!(e, Evidence::MovLo(_))))
    }
}

/// A reference to a mergeable piece waiting for its group (§5.6): the
/// record and where it is, its site and symbol made again only for a
/// finding (WP-109: a [`Pending`] with its `Site` and name was some
/// 350 B a reference, and a `-g` link has tens of millions).
#[derive(Debug, Clone)]
struct PieceRef {
    rec: Rec,
    /// The referring object and section.
    oi: u32,
    si: u32,
    /// The record is half of an ADRP+ADD made NOP+ADR.
    w1: bool,
    /// The place is in an allocated section: only a loaded word is an
    /// address word of stage 3's view 2; a `.debug_*` place's address
    /// overlaps the image's.
    loaded: bool,
    ev: Evidence,
}

/// The references to one piece (§5.6).
#[derive(Debug, Default)]
struct PieceGroup {
    members: Vec<PieceRef>,
}

/// The addresses evidence allows, in order, without duplicates: every
/// exact value, then every page with every low part ([`Group`]'s).
fn candidates_of<'e>(evidence: impl Iterator<Item = &'e Evidence>, got: Option<u64>) -> Vec<u64> {
    {
        let mut v: Vec<u64> = Vec::new();
        let mut pages: Vec<u64> = Vec::new();
        let mut lows: Vec<u64> = Vec::new();
        let mut highs: Vec<u64> = Vec::new();
        let mut movk: Vec<u64> = Vec::new();
        for e in evidence {
            match *e {
                Evidence::Exact(x) => v.push(x),
                Evidence::Page(x) => pages.push(x),
                Evidence::Low(x) => lows.push(x),
                Evidence::MovHi(x) => highs.push(x),
                Evidence::MovLo(x) => movk.push(x),
                Evidence::None => {}
            }
        }
        if let Some(g) = got {
            for &h in &highs {
                for &l in &movk {
                    v.push(g.wrapping_add((h & !0xffff) | l));
                }
            }
        }
        pages.sort_unstable();
        pages.dedup();
        lows.sort_unstable();
        lows.dedup();
        for &pg in &pages {
            for &lo in &lows {
                v.push(pg | lo);
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        v.retain(|x| seen.insert(*x));
        v
    }
}

/// The mergeable sections that hold a definition other objects can name
/// (global, weak or unique binding), as `(object, section)` (WP-109).
fn shared_merge(objs: &[Obj<'_>]) -> std::collections::HashSet<(usize, usize)> {
    let mut out = std::collections::HashSet::new();
    for (oi, o) in objs.iter().enumerate() {
        let Some(t) = o.symtab else { continue };
        for (i, sym) in t.iter() {
            if sym.binding() == mortar_elf::consts::STB_LOCAL {
                continue;
            }
            if let Ok(SymbolSection::Section(k)) = t.section_of(i, &sym)
                && o.secs.get(k.0 as usize).is_some_and(crate::locate::Sec::merge)
            {
                out.insert((oi, k.0 as usize));
            }
        }
    }
    out
}

/// The verdict on one record (§5.2).
#[derive(Debug, Clone, Copy)]
enum Verdict {
    Ok(Option<Rewrite>),
    Overflow,
    Misaligned,
    Mismatch(u64),
}

fn judge(rec: &Rec, x: i64) -> Verdict {
    let def = rec.def;
    let expected = def.field.encode(rec.input_word, x);
    if !def.check.holds(x) {
        return Verdict::Overflow;
    }
    if x & def.field.align_mask() != 0 {
        return Verdict::Misaligned;
    }
    if rec.out_word == expected {
        return Verdict::Ok(rec.patched.then_some(Rewrite::Erratum843419Patch));
    }
    // W7 (§5.3): at an erratum 843419 site the ADRP may be an ADR of the
    // same page, with the same destination.
    if def.field == Field::AdrPage && a64::erratum_843419_adrp(rec.p) {
        let rd = rec.input_word as u32 & 31;
        let page = a64::page(rec.p).wrapping_add(x as u64);
        if a64::adr(rec.out_word as u32, rec.p) == Some((rd, page)) {
            return Verdict::Ok(Some(Rewrite::Erratum843419Adr));
        }
    }
    Verdict::Mismatch(expected)
}

/// X for a non-GOT operation (§4.1). For the TLS operations `sa` is
/// already TPREL(S+A) or DTPREL(S+A) (§4.7).
fn x_direct(op: Op, sa: u64, p: u64, got: Option<u64>) -> Option<i64> {
    Some(match op {
        Op::Abs | Op::TpRel | Op::DtpRel => sa as i64,
        Op::Prel => sa.wrapping_sub(p) as i64,
        Op::Page => a64::page(sa).wrapping_sub(a64::page(p)) as i64,
        Op::GotRel => sa.wrapping_sub(got?) as i64,
        Op::None | Op::Slot(..) | Op::DescCall | Op::Size | Op::GotPc => return None,
    })
}

/// X for a GOT operation given the slot G (§4.1); `a` is the record's
/// addend, which only the x86-64 forms add outside the slot.
fn x_got(op: Op, g: u64, p: u64, got: Option<u64>, a: i64) -> Option<i64> {
    let Op::Slot(_, form) = op else { return None };
    Some(match form {
        Form::PrelA => g.wrapping_add(a as u64).wrapping_sub(p) as i64,
        Form::OffA => g.wrapping_add(a as u64).wrapping_sub(got?) as i64,
        Form::AbsA => g.wrapping_add(a as u64) as i64,
        Form::Abs => g as i64,
        Form::Prel => g.wrapping_sub(p) as i64,
        Form::Page => a64::page(g).wrapping_sub(a64::page(p)) as i64,
        Form::Off => g.wrapping_sub(got?) as i64,
        Form::PageOff => g.wrapping_sub(a64::page(got?)) as i64,
    })
}

/// Whether a record is of an x86-64 type: AArch64's are 256 and above,
/// x86-64's below (`R_X86_64_NONE` is 0 and has no [`Def`]).
fn is_x64(rec: &Rec) -> bool {
    rec.def.r_type < 256
}

/// Whether a record is a branch, which may go through any IPLT entry of
/// its resolver (§4.8).
fn is_branch(rec: &Rec) -> bool {
    if is_x64(rec) {
        rec.def.r_type == crate::x64::PLT32
    } else {
        matches!(rec.def.r_type, JUMP26 | CALL26 | CONDBR19 | TSTBR14)
    }
}

/// The addend a GOT slot of this record holds: A on AArch64, none on
/// x86-64, whose forms add A outside the slot (`spec/x64.md` X-4).
fn slot_addend(rec: &Rec) -> u64 {
    match rec.def.op {
        Op::Slot(_, f) if f.addend_outside() => 0,
        _ => rec.addend as u64,
    }
}

const ADR_PREL_PG_HI21: u32 = 275;
const ADR_PREL_PG_HI21_NC: u32 = 276;
const ADD_ABS_LO12_NC: u32 = 277;
const TSTBR14: u32 = 279;
const CONDBR19: u32 = 280;
const JUMP26: u32 = 282;
const CALL26: u32 = 283;
const ADR_GOT_PAGE: u32 = 311;
const LD64_GOT_LO12_NC: u32 = 312;
const TLSGD_ADR_PAGE21: u32 = 513;
const TLSGD_ADD_LO12_NC: u32 = 514;
const TLSLD_ADR_PAGE21: u32 = 518;
const TLSLD_ADD_LO12_NC: u32 = 519;
const TLSIE_ADR_GOTTPREL_PAGE21: u32 = 541;
const TLSIE_LD64_GOTTPREL_LO12_NC: u32 = 542;
const TLS_GET_ADDR: &str = "__tls_get_addr";

/// What one record comes to.
enum Outcome {
    Accepted(Rewrite),
    Verified,
    Skip(Reason),
    Fail(Rule, String, Option<u64>),
}

/// A traditional GD or LD sequence's form (§4.7).
enum Seq {
    Relaxed(Rewrite),
    /// Not rewritten, or not a whole sequence: each record on its own.
    Kept,
    Bad(String, Option<u64>),
}

/// The keys the line makes local (`Engine::made_local`).
#[derive(Default)]
pub(crate) struct MadeLocal {
    /// The keys.
    pub(crate) keys: std::collections::BTreeSet<String>,
    /// They are an executable's excluded keys (GNU ld's `GLOBAL HIDDEN`
    /// form is named there, WP-150 item 1).
    pub(crate) exec: bool,
}

/// The engine's state over one verification.
pub(crate) struct Engine<'a, 'b> {
    pub(crate) image: &'b Image<'a>,
    pub(crate) objs: &'b [Obj<'a>],
    occupied: Occupied,
    /// Erratum 843419 patches recognised at a site (`erratum::check`):
    /// the patch's address -> the site's final word (WP-082: lld's
    /// `__CortexA53843419_<site>` symbols are judged against it).
    pub(crate) patch_stubs: BTreeMap<u64, u64>,
    /// Global and weak definitions of the inputs by name.
    pub(crate) defs: HashMap<String, Vec<(usize, u32)>>,
    /// Common definitions (`SHN_COMMON`; x86-64's `SHN_X86_64_LCOMMON`)
    /// by name, for Y3.
    pub(crate) commons: HashMap<String, Vec<(usize, u32)>>,
    got: Option<u64>,
    /// The output's `.got`, `[start, end)`: a GOT slot lies inside it when
    /// the output has one (§5.5).
    got_section: Option<(u64, u64)>,
    cache: Vec<Vec<Option<Target>>>,
    /// The static TLS block, or why TP offsets cannot be judged (§4.7).
    pub(crate) tls: Result<Option<TlsBlock>, &'static str>,
    got_groups: BTreeMap<(Slot, usize, u32, i64), Group>,
    /// Output words a specified rewriting changed that no relocation
    /// record covers (W12's last word): S12 accepts them (§6.2).
    pub(crate) rewritten_words: std::collections::BTreeSet<u64>,
    piece_groups: BTreeMap<(usize, usize, u64, i64), PieceGroup>,
    /// The mergeable sections `(object, section)` that hold a global,
    /// weak or unique definition: other objects may refer to their
    /// pieces. A piece of any other mergeable section is named only by
    /// its own object's records, so its group is decided once that
    /// object's sections are judged (WP-109), not at the end.
    shared_merge: std::collections::HashSet<(usize, usize)>,
    /// The object [`Engine::section`] last judged a section of, and
    /// whether every call so far was in object order.
    pieces_last: Option<usize>,
    pieces_early: bool,
    /// Allocated input `.eh_frame` sections, checked by record (§6.4).
    eh_sections: Vec<(usize, usize)>,
    /// The output's IRELATIVE records and IPLT entries (§4.8).
    pub(crate) iplt: ifunc::Iplt,
    ifunc_groups: BTreeMap<IfKey, ifunc::IfGroup>,
    /// The linker that wrote the map (its conventions, §10).
    pub(crate) format: Format,
    /// Words of non-alloc output sections a tombstone rule wrote outside
    /// every record (I-112's aranges length): `(section, address)`.
    pub(crate) nonalloc_rewritten: std::collections::BTreeSet<(usize, u64)>,
    /// The veneers W6 accepted, by start address, each with the first
    /// record that branches to it (for R9, §5.4).
    veneers: BTreeMap<u64, (Rec, Site, String, u64)>,
    pub cov: Coverage,
    pub findings: Vec<Finding>,
    /// Stage 3 (§12): the run-time address of each name the output
    /// imports, from the loader model (0: no definition, weak).
    pub(crate) dynbind: HashMap<String, u64>,
    /// Names bound to a library's thread-local definition: not judged.
    /// Names whose `COPY` record the loader binds to a library (the
    /// loader view's `copied`).
    pub(crate) copied: std::collections::HashSet<String>,
    pub(crate) dyn_tls: std::collections::HashSet<String>,
    /// Stage 3: the module index of each name in `dyn_tls`.
    pub(crate) dyn_tls_mod: HashMap<String, u64>,
    /// Stage 3: the output is dynamic (kept TLS sequences are the
    /// loader's to finish).
    pub(crate) dynamic: bool,
    /// Stage 3 (§12.5): the output is a shared object: no TLS sequence is
    /// relaxed, so a `DTPOFF` field is block-relative everywhere (X-46).
    pub(crate) shared: bool,
    /// The one derivation of the export set (`shared::ExportRule`):
    /// M10's export roots ask it; `None` when the output exports nothing
    /// (WP-147, WP-149).
    pub(crate) exports: Option<crate::shared::ExportRule>,
    /// The keys the line makes local (a version script's `local:`,
    /// `--exclude-libs`; synth S-77, S-121; WP-169): stage 3 fills it
    /// (`Sets::local` for a `Shared` output, the excluded keys of an
    /// executable); empty for a static output. Hidden and internal
    /// definitions are read from the inputs where they are asked.
    pub(crate) made_local: MadeLocal,
    /// The line's `--wrap` redirection (`wrap.rs`, WP-170): an undefined
    /// mention's key; empty without `--wrap`.
    pub(crate) wrap: crate::wrap::Wrap,
    /// The `--wrap` words of refused item 3's shape (`spec/resolve.md`
    /// §7.1): a definition in a dropped COMDAT copy, whose mentions a
    /// reference's output binds in three ways; not judged by name.
    pub(crate) wrap_comdat: BTreeMap<String, String>,
    /// The record being judged when it refers to a defined mention of a
    /// `--wrap` word (E13), with `__wrap_SYM`'s address: a finding there
    /// whose field `__wrap_SYM` gives is tagged [`crate::wrap::E13`].
    e13: Option<Box<(Rec, u64, String)>>,
    /// The line's `--defsym` assignments (`defsym.rs`, WP-170 part 2):
    /// `def[SYM]` of each; empty without `--defsym`.
    pub(crate) defsym: crate::defsym::Defsym,
    /// Stage 3 (§12.5): the keys a shared object's view takes from the
    /// loader (`dynbind`) although the output defines them: preemptible
    /// keys under an interposer, a preemptible IFUNC's resolved address.
    pub(crate) loader_keys: std::collections::HashSet<String>,
    /// Stage 3: what §5.8's rule for a target only a shared library
    /// defines reads (`None`: a static output).
    pub(crate) dso_words: Option<dsoword::DsoWords>,
    /// PLT entries: entry address -> `.got.plt` word (stage 3).
    pub(crate) plt: BTreeMap<u64, u64>,
    /// Stage 3, x86-64: [`crate::plt::x64_depths`] of the file (the
    /// engine's image is a loaded view, whose lazy words the loader
    /// rewrote); `None`: the engine's image is the file.
    pub(crate) plt_depths: Option<BTreeMap<u64, u64>>,
    /// Verified absolute words and GOT words: `(place, width, value,
    /// internal)`, for the bias view (P-D9 view 2).
    pub(crate) abs_words: Vec<(u64, u8, u64, bool)>,
    /// What the inputs' GOT-indirect references demand a GOT word hold
    /// (WP-101): `S + A` of every non-TLS GOT reference to a defined or
    /// weak-undefined target, relaxed or not, and the values of every TLS
    /// slot pushed; a linker-made GOT word must hold one of them (P-D9,
    /// `spec/plumb.md` §12.3).
    pub(crate) got_demands: std::collections::BTreeSet<u64>,
    /// Stage 3: each IFUNC resolver's canonical IPLT entry, `None` when
    /// every reference reaches its IRELATIVE slot.
    pub(crate) ifunc_chosen: BTreeMap<u64, Option<u64>>,
    /// The executable's extent `[lo, hi)`: a target inside it moves with
    /// the load bias.
    pub(crate) extent: (u64, u64),
    /// §6.9: each record's outcome, by section and offset, when the
    /// equivalence check runs the engine (`None` otherwise).
    pub(crate) trace: Option<crate::equiv::Trace>,
    /// §5.7a (WP-178): the copies of COMDAT groups not kept, the
    /// verifier's own keeper (`fate`), made when a record first asks.
    comdat: std::cell::OnceCell<Vec<Vec<bool>>>,
    /// §5.8 (WP-250): the keeper of each COMDAT signature, decided with
    /// `comdat`'s order.
    keepers: std::cell::OnceCell<crate::fate::Keepers>,
    /// §5.7a: the demoted weak mention `(object, symbol, key)` whose record
    /// is being judged as an undefined weak, to name the form of its R5.
    trial: Option<(usize, u32, String)>,
    /// §5.7a: the R5 of each such record whose GOT group decides the form,
    /// by place and type.
    demoted_pending: BTreeMap<(u64, u32), (Finding, bool)>,
    /// §5.7b (WP-201): the LSDA starts of the inputs' FDEs, found when a
    /// `.gcc_except_table` record first refers to a section not in the
    /// output.
    lsda_starts: Option<ehowner::LsdaStarts>,
    /// The caller's command line, when it gives one (§3.2a: the options
    /// of a mortar output come from it; WP-201).
    pub(crate) caller_line: Option<Vec<String>>,
}

/// The words §5.7a adds to R5 when the field holds what the reference
/// would hold as an undefined weak one (A103, `spec/plumb.md` §10.2).
/// Whether line word `w` names input path `p`: equal, or one a suffix of
/// the other at a `/` (the map's spelling and the line's may differ by a
/// directory).
fn names_path(w: &str, p: &str) -> bool {
    crate::line::same_file(w, p)
        || w.strip_suffix(p).is_some_and(|d| d.ends_with('/'))
        || p.strip_suffix(w).is_some_and(|d| d.ends_with('/'))
}

pub(crate) const DEMOTED_WEAK: &str = " (a weak mention defined in a dropped COMDAT copy, whose key nothing live defines, written as an undefined weak reference: resolve.md §6.3 case (b))";

/// The words §5.7b adds to R5 when a field of a written `.eh_frame` or
/// `.gcc_except_table` record that refers to a section not in the output
/// holds 0 (A105, `spec/plumb.md` §10.2).
pub(crate) const LIVE_ZERO: &str =
    " (a field of a written .eh_frame or .gcc_except_table record, written 0: resolve.md §6.3)";

/// The words §5.7b adds to R5 when such a field holds its value with the
/// target's address taken as 0 (`A`, or `A - P` PC-relative; A106).
pub(crate) const LIVE_AT_ZERO: &str = " (a field of a written .eh_frame or .gcc_except_table record, written as if its target were at address 0: resolve.md §6.3)";

/// The words §5.7b adds to R5 of a `.gcc_except_table` field before the
/// first LSDA start of its section.
pub(crate) const NO_LSDA_OWNER: &str = " (no FDE has an LSDA in this section at or before this place: the field's owner is not known, resolve.md §6.3)";

/// The form of a field of a written record that refers to a section not
/// in the output (§5.7b): [`LIVE_ZERO`], [`LIVE_AT_ZERO`] or none.
fn live_form(rec: &Rec) -> Option<&'static str> {
    let w = u32::from(rec.def.field.width()) * 8;
    let mask = if w >= 64 { u64::MAX } else { (1u64 << w) - 1 };
    let got = rec.out_word & mask;
    let at_zero = match rec.def.op {
        Op::Abs => rec.addend as u64,
        Op::Prel => (rec.addend as u64).wrapping_sub(rec.p),
        _ => return None,
    } & mask;
    if !matches!(rec.def.field, Field::Data(_)) {
        return None;
    }
    if got == 0 {
        Some(LIVE_ZERO)
    } else if got == at_zero {
        Some(LIVE_AT_ZERO)
    } else {
        None
    }
}

impl<'a, 'b> Engine<'a, 'b> {
    /// The sizes of the engine's tables (WP-109), under
    /// [`crate::prof::enabled`]: estimates, by element size.
    pub(crate) fn gauges(&self) {
        use std::mem::size_of;
        fn members<K>(m: &BTreeMap<K, Group>) -> usize {
            m.values().map(|g| g.members.capacity()).sum()
        }
        let g = |name: &str, len: usize, size: usize| {
            crate::prof::gauge(&format!("engine {name} ({len})"), (len * size) as u64);
        };
        g("symbol cache", self.cache.iter().map(Vec::len).sum(), size_of::<Option<Target>>());
        g("defs", self.defs.len(), size_of::<(String, Vec<(usize, u32)>)>() + 48);
        g("got groups", self.got_groups.len(), 64);
        g("got group members", members(&self.got_groups), size_of::<(Pending, Evidence)>());
        g("piece groups", self.piece_groups.len(), 64);
        let pieces = self.piece_groups.values().map(|g| g.members.capacity()).sum();
        g("piece group members", pieces, size_of::<PieceRef>());
        g("rewritten words", self.rewritten_words.len(), 16);
        g("abs words", self.abs_words.capacity(), size_of::<(u64, u8, u64, bool)>());
        g("got demands", self.got_demands.len(), 16);
        g("veneers", self.veneers.len(), 128);
        g("findings", self.findings.len(), size_of::<Finding>());
        g(
            "trace",
            self.trace.as_ref().map_or(0, |t| t.records.capacity()),
            size_of::<crate::equiv::Traced>(),
        );
        g("eh sections", self.eh_sections.len(), 16);
    }

    /// Whether an address is the executable's (moves with the bias).
    pub(crate) fn internal(&self, s: u64) -> bool {
        self.extent.0 <= s && s < self.extent.1
    }

    /// [`Engine::internal`] for the value `s` of symbol `idx` of object
    /// `oi`, which may also lie at the end of a `PT_LOAD`'s memory image,
    /// or past it up to the next multiple of 8 (GNU ld's `_end`, Y5's
    /// window), when the symbol is a segment-end key: a reference (a
    /// global of the input) to a name layout gives an end (`_end`,
    /// `_edata`, `__bss_start`, `_etext` and their aliases) that no input
    /// defines (the linker's, resolve §8) and whose output definition is
    /// `s` (GNU ld's AArch64 PIE GOT slot of `_end`, WP-150 §3a item 3).
    /// Only the key's own value is an address there: a word that holds
    /// anything else is still judged against it.
    pub(crate) fn internal_value(&self, s: u64, oi: usize, idx: u32) -> bool {
        if self.internal(s) {
            return true;
        }
        let at_end = self.image.phdrs.iter().any(|p| {
            let end = p.p_vaddr.saturating_add(p.p_memsz);
            p.p_type == mortar_elf::consts::PT_LOAD
                && p.p_memsz > 0
                && end <= s
                && s <= end.saturating_add(7) & !7
        });
        if !at_end || idx == 0 {
            return false;
        }
        let Some(t) = self.objs[oi].symtab else { return false };
        let Ok(sym) = t.get(SymbolIndex(idx)) else { return false };
        if !matches!(sym.binding(), STB_GLOBAL | STB_WEAK) {
            return false;
        }
        let name = String::from_utf8_lossy(t.name(&sym).unwrap_or(b"")).into_owned();
        let end_key = matches!(
            name.as_str(),
            "_end"
                | "end"
                | "__end__"
                | "_bss_end__"
                | "__bss_end__"
                | "_edata"
                | "edata"
                | "__bss_start"
                | "__bss_start__"
                | "_etext"
                | "etext"
                | "__etext"
        );
        end_key
            && self.chosen_def(&name).is_none()
            && self.image.symbols.iter().any(|out| {
                out.name == name
                    && out.sym.st_shndx != 0
                    && out.sym.sym_type() != STT_SECTION
                    && out.sym.st_value == s
            })
    }

    /// Whether symbol `idx` of object `oi` resolves to an absolute
    /// definition (`SHN_ABS`): its value is not an address of the image
    /// and no load bias moves it (gABI "Symbol Values"; dyn.md D-46's
    /// `Abs` row: the value, no record). Resolution follows §4.4: a local
    /// by its own section, a global by the chosen input definition, else
    /// (linker-defined) by the output's symbol of the name (stage 3,
    /// WP-089: glibc's `_nl_current_*_used`, mold's absolute
    /// `__rela_iplt_*`).
    pub(crate) fn absolute(&self, oi: usize, idx: u32) -> bool {
        let o = &self.objs[oi];
        let Some(t) = o.symtab else { return false };
        let Ok(sym) = t.get(SymbolIndex(idx)) else { return false };
        if idx == 0 {
            return false;
        }
        if !matches!(sym.binding(), STB_GLOBAL | STB_WEAK | STB_GNU_UNIQUE) {
            return matches!(t.section_of(SymbolIndex(idx), &sym), Ok(SymbolSection::Absolute));
        }
        let name = String::from_utf8_lossy(t.name(&sym).unwrap_or(b"")).into_owned();
        // §4.4b (WP-170): a `--defsym` number is `SHN_ABS`, never
        // relocated, in every output kind; an alias or `B ± N` is `B`'s.
        if !self.defsym.is_empty() {
            let key =
                if matches!(t.section_of(SymbolIndex(idx), &sym), Ok(SymbolSection::Undefined)) {
                    self.wrap.key(&name).into_owned()
                } else {
                    name.clone()
                };
            if let Some(a) = self.defsym_absolute(&key) {
                return a;
            }
        }
        if let Some((doi, di)) = self.chosen_def(&name) {
            return self.objs[doi].symtab.is_some_and(|dt| {
                dt.get(SymbolIndex(di)).is_ok_and(|ds| {
                    matches!(dt.section_of(SymbolIndex(di), &ds), Ok(SymbolSection::Absolute))
                })
            });
        }
        // A linker-defined symbol: GNU ld writes `_DYNAMIC` and
        // `_GLOBAL_OFFSET_TABLE_` as `SHN_ABS` on AArch64 although they
        // are addresses, so `SHN_ABS` counts only for a value no allocated
        // section holds (end inclusive; mold's absolute 0 `__rela_iplt_*`).
        self.image.global(&name).or_else(|| self.image.unique(&name)).is_some_and(|out| {
            let v = out.sym.st_value;
            out.sym.st_shndx == mortar_elf::consts::SHN_ABS
                && !self.image.sections.iter().any(|x| {
                    x.shdr.sh_flags & mortar_elf::consts::SHF_ALLOC != 0
                        && x.shdr.sh_addr <= v
                        && v <= x.shdr.sh_addr + x.shdr.sh_size
                })
        })
    }

    /// [`Engine::absolute`] of a `--defsym` key (§4.4b): `Some(true)` for
    /// a number, `B`'s for an alias or `B ± N` (one level: a chain is not
    /// derived), `None` for any other name.
    pub(crate) fn defsym_absolute(&self, key: &str) -> Option<bool> {
        use crate::defsym::Expr;
        Some(match self.defsym.get(key)? {
            Expr::Number(_) => true,
            Expr::Alias(b) | Expr::Offset(b, _) => {
                if let Some((doi, di)) = self.chosen_def(b) {
                    self.objs[doi].symtab.is_some_and(|dt| {
                        dt.get(SymbolIndex(di)).is_ok_and(|ds| {
                            matches!(
                                dt.section_of(SymbolIndex(di), &ds),
                                Ok(SymbolSection::Absolute)
                            )
                        })
                    })
                } else {
                    false
                }
            }
        })
    }

    /// `synth.md` S-50 rule 1 under mortar's map (§4.4, "offset 0 of an
    /// input `.eh_frame`"): the address offset 0 of section `sec` of
    /// object `oi` has when it is an input `.eh_frame` the map does not
    /// discard, the start of the output's allocated `.eh_frame`, whatever
    /// the input section holds. `None` under a foreign map (GNU ld and lld
    /// give a kept record's own place, §10.4), for any other section, or
    /// when the output has no `.eh_frame`.
    /// The output's allocated `.eh_frame`: its address and size.
    pub(crate) fn output_eh_frame(&self) -> Option<(u64, u64)> {
        self.image
            .sections
            .iter()
            .find(|x| x.name == ".eh_frame" && x.alloc())
            .map(|x| (x.shdr.sh_addr, x.shdr.sh_size))
    }

    /// `synth.md` S-50's value, under mortar's map, of offset `r` of input
    /// `.eh_frame` `sec` of object `oi`, for a symbol (WP-143b):
    /// rule 1 (`r = 0`: the output's start); rule 2 (`r` in a kept record:
    /// its piece); rule 3, and rule 4 for a symbol (the value of the
    /// record it lies in): the first kept record after `r` in output
    /// order — the pieces of `sec` after `r`, then those of the later
    /// input `.eh_frame` sections in `SectionId` order (S-46: object
    /// order, then section index) — else the final terminator, the
    /// output's last 4 bytes. `None` under a foreign map or without an
    /// output `.eh_frame`.
    pub(crate) fn eh_frame_value(&self, oi: usize, sec: usize, r: u64) -> Option<u64> {
        if self.format != Format::Mortar {
            return None;
        }
        let (addr, size) = self.output_eh_frame()?;
        if r == 0 {
            return Some(addr);
        }
        let pieces = |o: &Obj<'_>, k: usize| match &o.secs[k].place {
            Place::Pieces(ps) => ps.clone(),
            _ => Vec::new(),
        };
        let own = pieces(&self.objs[oi], sec);
        if let Some(p) = own.iter().find(|p| p.input_offset <= r && r < p.input_offset + p.size) {
            return Some(p.addr + (r - p.input_offset));
        }
        if let Some(p) = own.iter().filter(|p| p.input_offset >= r).min_by_key(|p| p.input_offset) {
            return Some(p.addr);
        }
        for (i, o) in self.objs.iter().enumerate().skip(oi) {
            for k in 0..o.secs.len() {
                if (i == oi && k <= sec) || !o.secs[k].is_eh_frame(o.elf.machine()) {
                    continue;
                }
                if let Some(p) = pieces(o, k).iter().min_by_key(|p| p.input_offset) {
                    return Some(p.addr);
                }
            }
        }
        Some(addr + size.saturating_sub(4))
    }

    pub(crate) fn eh_frame_start(&self, oi: usize, sec: usize) -> Option<u64> {
        if self.format != Format::Mortar {
            return None;
        }
        let o = &self.objs[oi];
        let s = o.secs.get(sec)?;
        if !s.alloc()
            || !s.is_eh_frame(o.elf.machine())
            || matches!(s.place, Place::Discarded | Place::Ambiguous)
        {
            return None;
        }
        self.image
            .sections
            .iter()
            .find(|x| x.name == ".eh_frame" && x.alloc())
            .map(|x| x.shdr.sh_addr)
    }

    /// S-50 rule 1 for a section symbol (§4.4): `st_value + A = 0` in an
    /// input `.eh_frame` is the output `.eh_frame`'s start, with nothing
    /// left to add.
    fn eh_frame_section_start(&self, oi: usize, idx: u32, addend: i64) -> Option<Target> {
        let table = self.objs[oi].symtab?;
        let sym = table.get(SymbolIndex(idx)).ok()?;
        if sym.sym_type() != STT_SECTION || sym.st_value.wrapping_add(addend as u64) != 0 {
            return None;
        }
        let Ok(SymbolSection::Section(k)) = table.section_of(SymbolIndex(idx), &sym) else {
            return None;
        };
        self.eh_frame_start(oi, k.0 as usize).map(Target::Addr)
    }

    /// A branch whose target is a PLT entry whose word holds `s` after
    /// loading (stage 3).
    pub(crate) fn plt_hop(&self, t: u64, s: u64) -> bool {
        self.plt.get(&t).is_some_and(|&slot| self.image.value(slot, self.word() as u8) == Some(s))
    }

    #[allow(clippy::too_many_lines)] // one line per field
    pub fn new(image: &'b Image<'a>, objs: &'b [Obj<'a>], format: Format) -> Self {
        let mut defs: HashMap<String, Vec<(usize, u32)>> = HashMap::default();
        let mut commons: HashMap<String, Vec<(usize, u32)>> = HashMap::default();
        for (oi, o) in objs.iter().enumerate() {
            let Some(t) = &o.symtab else { continue };
            let x64 = o.elf.machine() == Machine::X86_64;
            for (i, sym) in t.iter() {
                if matches!(sym.binding(), STB_GLOBAL | STB_WEAK)
                    && (sym.st_shndx == mortar_elf::consts::SHN_COMMON
                        || (x64 && sym.st_shndx == mortar_elf::consts::SHN_X86_64_LCOMMON))
                    && let Ok(name) = t.name(&sym)
                {
                    commons
                        .entry(String::from_utf8_lossy(name).into_owned())
                        .or_default()
                        .push((oi, i.0));
                    continue;
                }
                if !matches!(sym.binding(), STB_GLOBAL | STB_WEAK | STB_GNU_UNIQUE)
                    || !matches!(
                        t.section_of(i, &sym),
                        Ok(SymbolSection::Section(_) | SymbolSection::Absolute)
                    )
                {
                    continue;
                }
                let Ok(name) = t.name(&sym) else { continue };
                defs.entry(String::from_utf8_lossy(name).into_owned()).or_default().push((oi, i.0));
            }
        }
        let got = image
            .unique("_GLOBAL_OFFSET_TABLE_")
            .or_else(|| image.global("_GLOBAL_OFFSET_TABLE_"))
            .map(|s| s.sym.st_value)
            .or_else(|| {
                // A stripped output (WP-164): where Y5 puts the symbol, the
                // start of `.got.plt` on x86-64 (synth.md R-P5; measured:
                // mold's large-model GOTOFF64), of `.got` on AArch64.
                let x64 = image.header.e_machine == mortar_elf::consts::EM_X86_64;
                image
                    .sections
                    .iter()
                    .find(|s| x64 && s.name == ".got.plt")
                    .or_else(|| image.sections.iter().find(|s| s.name == ".got"))
                    .map(|s| s.shdr.sh_addr)
            });
        // spec/x86.md X86-47: mold computes the i386 GOT base (GOTPC,
        // GOTOFF, GOT32) against the start of `.got`, not
        // `_GLOBAL_OFFSET_TABLE_` (§5.11; its no-base GOT32 is W3).
        // In a stripped output GNU ld's and lld's base is the start of
        // `.got.plt` (X86-47), not `.got`.
        let sec = |n: &str| image.sections.iter().find(|s| s.name == n).map(|s| s.shdr.sh_addr);
        let i386 = image.header.e_machine == mortar_elf::consts::EM_386;
        let got = if i386 && format == Format::Mold {
            sec(".got").or(got)
        } else if i386 && image.unique("_GLOBAL_OFFSET_TABLE_").is_none() {
            image
                .global("_GLOBAL_OFFSET_TABLE_")
                .map(|s| s.sym.st_value)
                .or(sec(".got.plt"))
                .or(got)
        } else {
            got
        };
        let got_section = image
            .sections
            .iter()
            .find(|s| s.name == ".got")
            .map(|s| (s.shdr.sh_addr, s.shdr.sh_addr.saturating_add(s.shdr.sh_size)));
        let cache =
            objs.iter().map(|o| vec![None; o.symtab.map_or(0, |t| t.len() as usize)]).collect();
        let mut e = Engine {
            image,
            objs,
            occupied: Occupied::build(objs),
            patch_stubs: BTreeMap::new(),
            defs,
            commons,
            got,
            got_section,
            cache,
            tls: TlsBlock::of(image),
            got_groups: BTreeMap::new(),
            rewritten_words: std::collections::BTreeSet::new(),
            piece_groups: BTreeMap::new(),
            shared_merge: shared_merge(objs),
            pieces_last: None,
            pieces_early: true,
            eh_sections: Vec::new(),
            iplt: ifunc::Iplt::default(),
            ifunc_groups: BTreeMap::new(),
            format,
            nonalloc_rewritten: std::collections::BTreeSet::new(),
            veneers: BTreeMap::new(),
            cov: Coverage::default(),
            findings: Vec::new(),
            dynbind: HashMap::default(),
            dyn_tls: std::collections::HashSet::new(),
            copied: std::collections::HashSet::new(),
            dyn_tls_mod: HashMap::default(),
            dynamic: false,
            shared: false,
            exports: None,
            made_local: MadeLocal::default(),
            wrap: crate::wrap::Wrap::default(),
            wrap_comdat: BTreeMap::new(),
            e13: None,
            defsym: crate::defsym::Defsym::default(),
            loader_keys: std::collections::HashSet::new(),
            dso_words: None,
            plt: BTreeMap::new(),
            plt_depths: None,
            abs_words: Vec::new(),
            got_demands: std::collections::BTreeSet::new(),
            ifunc_chosen: BTreeMap::new(),
            extent: {
                let loads = image.phdrs.iter().filter(|p| p.p_type == mortar_elf::consts::PT_LOAD);
                let lo = loads.clone().map(|p| p.p_vaddr).min().unwrap_or(0);
                let hi = loads.map(|p| p.p_vaddr.saturating_add(p.p_memsz)).max().unwrap_or(0);
                (lo, hi)
            },
            trace: None,
            comdat: std::cell::OnceCell::new(),
            keepers: std::cell::OnceCell::new(),
            trial: None,
            demoted_pending: BTreeMap::new(),
            lsda_starts: None,
            caller_line: None,
        };
        e.build_iplt();
        e
    }

    /// Whether `addr` is in a placed input section (§5.4).
    pub(crate) fn occupied_at(&self, addr: u64) -> bool {
        self.occupied.contains(addr)
    }

    fn sym_name(&self, oi: usize, idx: u32) -> String {
        let o = &self.objs[oi];
        o.symtab.and_then(|t| t.get(SymbolIndex(idx)).ok().map(|s| (t, s))).map_or_else(
            String::new,
            |(t, s)| {
                if s.sym_type() == STT_SECTION {
                    t.section_of(SymbolIndex(idx), &s)
                        .ok()
                        .and_then(|ss| match ss {
                            SymbolSection::Section(k) => {
                                o.secs.get(k.0 as usize).map(|x| x.name.clone())
                            }
                            _ => None,
                        })
                        .unwrap_or_default()
                } else {
                    String::from_utf8_lossy(t.name(&s).unwrap_or(b"")).into_owned()
                }
            },
        )
    }

    /// The target of a symbol defined in section `sec` of object `oi` at
    /// `value` (§4.4).
    fn in_section(&self, oi: usize, sec: usize, value: u64, section_sym: bool) -> Target {
        let o = &self.objs[oi];
        let Some(s) = o.secs.get(sec) else { return Target::Skip(Reason::SymbolUnresolved) };
        // S-50 rule 1 for a named symbol at offset 0 (a section symbol's
        // offset includes its addend: `resolve_at`).
        if !section_sym
            && value == 0
            && let Some(a) = self.eh_frame_start(oi, sec)
        {
            return Target::Addr(a);
        }
        match &s.place {
            Place::Whole(a) => Target::Addr(a.wrapping_add(value)),
            Place::Pieces(ps) => ps
                .iter()
                // The piece holding byte `value` (S-17, S-19); a label at
                // the section's end, the piece ending there (S-19).
                .find(|p| p.input_offset <= value && value < p.input_offset + p.size)
                .or_else(|| {
                    (!section_sym && value == s.shdr.sh_size)
                        .then(|| ps.iter().find(|p| p.input_offset + p.size == value))
                        .flatten()
                })
                .map_or_else(
                    // A part of .eh_frame the linker dropped or rewrote.
                    || {
                        if s.is_eh_frame(o.elf.machine()) {
                            Target::Skip(Reason::EhFrame)
                        } else {
                            Target::Discarded(s.name.clone())
                        }
                    },
                    |p| Target::Addr(p.addr + (value - p.input_offset)),
                ),
            // An empty section has no pieces (WP-143): where its labels
            // are, only the output's symbols say (§4.4 steps 1 and 3).
            Place::Merged if s.shdr.sh_size == 0 => Target::Empty(s.name.clone()),
            Place::Merged => Target::Merge { oi, sec, value, section_sym },
            Place::Rewritten => Target::Skip(Reason::EhFrame),
            Place::Ambiguous => Target::Skip(Reason::SectionNotLocatable),
            // An input `.eh_frame` and no output one under mortar's map
            // (synth.md S-48a: records all dead, no terminator, no empty
            // input): nothing to refer to (WP-143b).
            Place::Absent
                if s.is_eh_frame(o.elf.machine())
                    && self.format == Format::Mortar
                    && self.output_eh_frame().is_none() =>
            {
                Target::Discarded(s.name.clone())
            }
            Place::Absent if s.is_eh_frame(o.elf.machine()) => Target::Skip(Reason::EhFrame),
            Place::Absent if s.empty_unrecorded() => Target::Empty(s.name.clone()),
            Place::Absent | Place::Discarded => Target::Discarded(s.name.clone()),
        }
    }

    /// The target of a record against symbol `idx` with addend `a`, and
    /// the addend still to add to it. For a section symbol of a section
    /// placed as pieces the addend selects the piece (`synth.md` S-17):
    /// the piece holding byte `st_value + A` is found and nothing is left
    /// to add. Any other symbol is resolved alone and keeps its addend
    /// (S-19: the addend of a named symbol does not choose the piece).
    fn resolve_at(&mut self, oi: usize, idx: u32, addend: i64) -> (Target, i64) {
        if let Some(t) = self.eh_frame_section_start(oi, idx, addend) {
            return (t, 0);
        }
        if let Some(t) = self.section_piece(oi, idx, addend) {
            return (t, 0);
        }
        (self.resolve(oi, idx), addend)
    }

    /// The S-17 target of section symbol `idx` plus `a`, when its section
    /// is placed as pieces; `None` for every other symbol.
    fn section_piece(&self, oi: usize, idx: u32, addend: i64) -> Option<Target> {
        let obj = &self.objs[oi];
        let table = obj.symtab?;
        let sym = table.get(SymbolIndex(idx)).ok()?;
        if sym.sym_type() != STT_SECTION {
            return None;
        }
        let Ok(SymbolSection::Section(k)) = table.section_of(SymbolIndex(idx), &sym) else {
            return None;
        };
        let sec = obj.secs.get(k.0 as usize)?;
        if !matches!(sec.place, Place::Pieces(_)) {
            return None;
        }
        let r = sym.st_value.wrapping_add(addend as u64);
        // S-18: outside the section, no piece is named.
        if r >= sec.shdr.sh_size {
            return Some(Target::Outside(sec.name.clone()));
        }
        Some(match self.in_section(oi, k.0 as usize, r, true) {
            Target::Addr(v) if sec.shdr.sh_flags & SHF_TLS != 0 => Target::Tls(v),
            t => t,
        })
    }

    fn resolve(&mut self, oi: usize, idx: u32) -> Target {
        if self.trial.as_ref().is_some_and(|t| (t.0, t.1) == (oi, idx)) {
            return self.resolve_uncached(oi, idx);
        }
        if let Some(t) = self.cache.get(oi).and_then(|c| c.get(idx as usize)).and_then(Clone::clone)
        {
            return t;
        }
        let t = self.resolve_uncached(oi, idx);
        if let Some(slot) = self.cache.get_mut(oi).and_then(|c| c.get_mut(idx as usize)) {
            *slot = Some(t.clone());
        }
        t
    }

    /// The address of a TLS output symbol: `st_value` is an offset in the
    /// TLS template in an executable (gABI "Symbol Values").
    fn tls_out(&self, st_value: u64) -> Target {
        match self.tls {
            Ok(Some(b)) => Target::Tls(b.start.wrapping_add(st_value)),
            _ => Target::Skip(Reason::Tls),
        }
    }

    /// Whether symbol `idx` of object `oi` is thread-local: `STT_TLS`, or
    /// defined in an `SHF_TLS` section (a section symbol or a local label
    /// of `.tdata`/`.tbss`).
    fn input_sym_is_tls(&self, oi: usize, idx: u32) -> bool {
        let o = &self.objs[oi];
        let Some(t) = o.symtab else { return false };
        let Ok(sym) = t.get(SymbolIndex(idx)) else { return false };
        if sym.sym_type() == STT_TLS {
            return true;
        }
        match t.section_of(SymbolIndex(idx), &sym) {
            Ok(SymbolSection::Section(k)) => {
                o.secs.get(k.0 as usize).is_some_and(|s| s.shdr.sh_flags & SHF_TLS != 0)
            }
            _ => false,
        }
    }

    fn resolve_uncached(&self, oi: usize, idx: u32) -> Target {
        let t = self.resolve_any(oi, idx);
        // A thread-local definition gives an address in the TLS template.
        match t {
            Target::Addr(a) if self.input_sym_is_tls(oi, idx) => Target::Tls(a),
            t => t,
        }
    }

    #[allow(clippy::too_many_lines)] // §4.4, its three steps in order
    fn resolve_any(&self, oi: usize, idx: u32) -> Target {
        let o = &self.objs[oi];
        let Some(t) = o.symtab else { return Target::Skip(Reason::SymbolUnresolved) };
        if idx == 0 {
            return Target::Addr(0);
        }
        let Ok(sym) = t.get(SymbolIndex(idx)) else {
            return Target::Skip(Reason::SymbolUnresolved);
        };
        let where_ = t.section_of(SymbolIndex(idx), &sym);
        let local = !matches!(sym.binding(), STB_GLOBAL | STB_WEAK | STB_GNU_UNIQUE);
        if sym.sym_type() == STT_GNU_IFUNC && local {
            return self.ifunc_target(oi, idx, IfKey::Local(oi, idx));
        }
        if local {
            return match where_ {
                Ok(SymbolSection::Section(k)) => {
                    self.in_section(oi, k.0 as usize, sym.st_value, sym.sym_type() == STT_SECTION)
                }
                Ok(SymbolSection::Absolute) => Target::Addr(sym.st_value),
                _ => Target::Skip(Reason::SymbolUnresolved),
            };
        }
        let name = String::from_utf8_lossy(t.name(&sym).unwrap_or(b"")).into_owned();
        // §4.4a (WP-170): refused item 3's shape in a reference's output.
        if self.wrap_comdat.contains_key(&name) {
            return Target::Skip(Reason::Wrap);
        }
        // §4.4a (WP-170): an undefined mention binds to its `--wrap` key.
        let name = if matches!(where_, Ok(SymbolSection::Undefined)) {
            self.wrap.key(&name).into_owned()
        } else {
            name
        };
        // §5.7a: a demoted weak mention judged as an undefined one.
        let undefined = matches!(where_, Ok(SymbolSection::Undefined))
            || self.trial.as_ref().is_some_and(|t| (t.0, t.1) == (oi, idx));
        let here_ifunc = (sym.sym_type() == STT_GNU_IFUNC
            && matches!(where_, Ok(SymbolSection::Section(_))))
        .then_some((oi, idx));
        self.global_target(name, undefined, sym.binding() == STB_WEAK, here_ifunc)
    }

    /// §4.4's lookup of a global `name` (its `--wrap` key) by a mention
    /// that is `undefined` or defined, `weak` or not; `here_ifunc`: the
    /// mention itself, when it is a defined IFUNC.
    #[allow(clippy::too_many_lines)] // §4.4, its steps in order
    fn global_target(
        &self,
        name: String,
        undefined: bool,
        weak: bool,
        here_ifunc: Option<(usize, u32)>,
    ) -> Target {
        // §4.7: `_TLS_MODULE_BASE_` is, by its definition in the TLS
        // descriptor ABI, the start of the module's TLS block, whatever
        // value the linker gives its symbol (A3).
        if name == "_TLS_MODULE_BASE_" && undefined {
            return self.tls_out(0);
        }
        // §12.5: in a shared object's views a key the loader binds (an
        // interposed key, a preemptible IFUNC's resolved address) is where
        // the view puts it, not an IPLT entry.
        let bound_by_loader = self.loader_keys.contains(&name);
        // §4.4b (WP-170): `def[SYM]` is the line's `--defsym`, whatever
        // an input or the output's `.symtab` says of `SYM`.
        if !bound_by_loader && !self.defsym.is_empty() {
            if let Some(t) = self.defsym_target(&name) {
                return t;
            }
            if self.defsym.unjudged(&name) {
                return Target::Skip(Reason::Defsym);
            }
        }
        // §4.4 step 0 (WP-143): a name every input definition of which is
        // in a section without a place (discarded, or absent from the
        // map) is that section's, whatever the output's symbol says: R5,
        // or §5.8's tombstone from a non-alloc place. Before WP-143 the
        // steps below looked at placed definitions only and ended in
        // `symbol-unresolved`, a count, never R5: a word left 0 passed.
        // The loader binds it when the output imports it and the loader
        // model finds a definition (a needed library's, or the copy of
        // one: resolve.md §6.2, WP-178); §5.7a's trial judges the name as
        // an undefined weak.
        let bound_dyn = self.copied.contains(&name)
            || self.dynbind.get(&name).is_some_and(|&a| a != 0)
                && (self.image.symbols.is_empty()
                    || self.image.global(&name).is_none_or(|o| o.sym.st_shndx == 0));
        let in_trial = self.trial.as_ref().is_some_and(|t| t.2 == name);
        if !bound_by_loader
            && !bound_dyn
            && !in_trial
            && let Some((doi, di, k)) = self.placeless_def(&name)
        {
            let value = self.objs[doi]
                .symtab
                .and_then(|dt| dt.get(SymbolIndex(di)).ok())
                .map_or(0, |d| d.st_value);
            return self.in_section(doi, k, value, false);
        }
        // An IFUNC by its placed definition, whatever form the output's
        // symbol has (§4.8).
        let chosen = self.chosen_def(&name);
        if !bound_by_loader
            && let Some((doi, di)) = chosen
            && self.objs[doi]
                .symtab
                .and_then(|dt| dt.get(SymbolIndex(di)).ok())
                .is_some_and(|d| d.sym_type() == STT_GNU_IFUNC)
        {
            return self.ifunc_target(doi, di, IfKey::Global(name));
        }
        if !bound_by_loader
            && chosen.is_none()
            && let Some((oi, idx)) = here_ifunc
        {
            return self.ifunc_target(oi, idx, IfKey::Global(name));
        }
        // Stage 3: a name the output imports is where the loader binds it.
        // A stripped output (no `.symtab`, WP-164) defines the name when
        // an input's placed definition or a common is chosen: its `.dynsym`
        // export is the loader's only for a key the loader binds.
        let undefined_out = bound_by_loader
            || if self.image.symbols.is_empty() {
                chosen.is_none() && !self.commons.contains_key(&name)
            } else {
                self.image.global(&name).is_none_or(|o| o.sym.st_shndx == 0)
            };
        if undefined_out && let Some(&a) = self.dynbind.get(&name) {
            if self.dyn_tls.contains(&name) {
                // Where the loader model puts the library's TLS byte.
                return Target::Tls(a);
            }
            return if a == 0 { Target::UndefWeak } else { Target::Addr(a) };
        }
        // 1. The output's global symbol of the name.
        if let Some(out) = self.image.global(&name) {
            if out.sym.sym_type() == STT_GNU_IFUNC && out.sym.st_shndx != 0 {
                return Target::Ifunc { resolver: out.sym.st_value, key: IfKey::Global(name) };
            }
            if out.sym.st_shndx != 0 {
                if out.sym.sym_type() == STT_TLS {
                    return self.tls_out(out.sym.st_value);
                }
                return Target::Addr(out.sym.st_value);
            }
            return if out.sym.binding() == STB_WEAK || weak {
                Target::UndefWeak
            } else {
                Target::Skip(Reason::SymbolUnresolved)
            };
        }
        // 2. The one placed strong definition, else the one placed weak one.
        if let Some((doi, di)) = chosen {
            let dt = self.objs[doi].symtab.expect("chosen from a symbol table");
            if let Ok(ds) = dt.get(SymbolIndex(di)) {
                let t = match dt.section_of(SymbolIndex(di), &ds) {
                    Ok(SymbolSection::Section(k)) => {
                        self.in_section(doi, k.0 as usize, ds.st_value, false)
                    }
                    Ok(SymbolSection::Absolute) => Target::Addr(ds.st_value),
                    _ => Target::Skip(Reason::SymbolUnresolved),
                };
                return match t {
                    Target::Addr(a) if self.input_sym_is_tls(doi, di) => Target::Tls(a),
                    t => t,
                };
            }
        }
        // 3. The one output symbol of the name, of any binding. Not in
        // §5.7a's trial: no definition of the key has a place, and an
        // output local of the name (an input's local, `objcopy
        // --localize-hidden`'s kept copy) binds no mention (WP-201).
        if !in_trial && let Some(out) = self.image.unique(&name) {
            if out.sym.sym_type() == STT_GNU_IFUNC && out.sym.st_shndx != 0 {
                return Target::Ifunc { resolver: out.sym.st_value, key: IfKey::Global(name) };
            }
            if out.sym.st_shndx != 0 {
                if out.sym.sym_type() == STT_TLS {
                    return self.tls_out(out.sym.st_value);
                }
                return Target::Addr(out.sym.st_value);
            }
        }
        // 4. An undefined weak: only when the output has a `.symtab`. A
        // stripped output (`-s`) does not say whether the linker defined
        // the name (`__rela_iplt_start`).
        if undefined && weak && !self.image.symbols.is_empty() {
            return Target::UndefWeak;
        }
        // A name defined only in empty sections without a record (WP-143).
        if let Some(sec) = self.empty_only(&name) {
            return Target::Empty(sec);
        }
        Target::Skip(Reason::SymbolUnresolved)
    }

    /// When every input definition of `name` is in an empty section that
    /// has no place record (an empty merged section, or a foreign map's
    /// empty mergeable section without a row), the first one's name.
    fn empty_only(&self, name: &str) -> Option<String> {
        let defs = self.defs.get(name)?;
        let mut first = None;
        for &(doi, di) in defs {
            let dt = self.objs[doi].symtab?;
            let ds = dt.get(SymbolIndex(di)).ok()?;
            let Ok(SymbolSection::Section(k)) = dt.section_of(SymbolIndex(di), &ds) else {
                return None;
            };
            let s = self.objs[doi].secs.get(k.0 as usize)?;
            let empty = s.empty_unrecorded()
                || (s.shdr.sh_size == 0 && s.alloc() && matches!(s.place, Place::Merged));
            if !empty {
                return None;
            }
            first.get_or_insert_with(|| s.name.clone());
        }
        first
    }

    /// A record against [`Target::Empty`]: the address its field names,
    /// when the type is an address-sized `S + A` or a 4- or 8-byte
    /// `S + A - P`, is in or at the end of an allocated output section
    /// (a label of an empty section is where its bytes would be; 0 or a
    /// gap is a finding). Other types are not checked.
    fn empty_record(&mut self, rec: &Rec, site: &Site, symbol: &str, sec: &str) {
        let a = rec.addend as u64;
        let width = if self.image.class() == mortar_elf::Class::Elf64 { 8 } else { 4 };
        let s = match (rec.def.op, rec.def.field) {
            (Op::Abs, Field::Data(w)) if w == width => trunc(rec.out_word.wrapping_sub(a), w),
            (Op::Prel, Field::Data(8)) => rec.p.wrapping_add(rec.out_word).wrapping_sub(a),
            (Op::Prel, Field::Data(4)) => {
                rec.p.wrapping_add(i64::from(rec.out_word as u32 as i32) as u64).wrapping_sub(a)
            }
            _ => return self.not_checked(Reason::SectionNotLocatable, 1),
        };
        let s = trunc(s, width);
        if self.image.holds_or_ends(s) {
            self.cov.add_verified(Via::Direct, rec.def.r_type);
            return;
        }
        let mut f = Self::finding(
            Rule::DiscardedTarget,
            rec,
            site,
            symbol,
            format!(
                "refers to empty section '{sec}' at {s:#x}, neither in nor at the end of an allocated output section"
            ),
        );
        f.found = Some(s);
        self.fail(f);
    }

    /// §4.4 step 0 (WP-143): when `name` has input definitions, every one
    /// of them in a section of an input object (none `SHN_ABS`, none
    /// common) and every such section without a place (`Absent` or
    /// `Discarded`, §3.3), the definition the name resolves to: the first
    /// strong one in object order, else the first. `None` otherwise.
    pub(crate) fn placeless_def(&self, name: &str) -> Option<(usize, u32, usize)> {
        let defs = self.defs.get(name)?;
        if self.commons.contains_key(name) {
            return None;
        }
        let mut first: Option<(usize, u32, usize)> = None;
        let mut strong: Option<(usize, u32, usize)> = None;
        for &(doi, di) in defs {
            let dt = self.objs[doi].symtab?;
            let ds = dt.get(SymbolIndex(di)).ok()?;
            let Ok(SymbolSection::Section(k)) = dt.section_of(SymbolIndex(di), &ds) else {
                return None;
            };
            let k = k.0 as usize;
            // P-1's predicate (`Obj::no_place`): an input `.eh_frame`
            // while the output has one has S-50's values (WP-143b).
            if !self.objs[doi].no_place(k, self.output_eh_frame().is_some()) {
                return None;
            }
            first.get_or_insert((doi, di, k));
            if ds.binding() != STB_WEAK {
                strong.get_or_insert((doi, di, k));
            }
        }
        strong.or(first)
    }

    /// §4.4 step 2's choice: the one placed strong definition of `name`,
    /// else the one placed weak one.
    pub(crate) fn chosen_def(&self, name: &str) -> Option<(usize, u32)> {
        {
            let defs = self.defs.get(name)?;
            let placed: Vec<(usize, u32, bool)> = defs
                .iter()
                .filter_map(|&(doi, di)| {
                    let dt = self.objs[doi].symtab?;
                    let ds = dt.get(SymbolIndex(di)).ok()?;
                    let live = match dt.section_of(SymbolIndex(di), &ds).ok()? {
                        SymbolSection::Section(k) => {
                            self.objs[doi].secs.get(k.0 as usize).is_some_and(|s| {
                                matches!(
                                    s.place,
                                    Place::Whole(_) | Place::Pieces(_) | Place::Merged
                                )
                            })
                        }
                        SymbolSection::Absolute => true,
                        _ => false,
                    };
                    live.then_some((doi, di, ds.binding() != STB_WEAK))
                })
                .collect();
            let strong: Vec<_> = placed.iter().filter(|d| d.2).collect();
            match (strong.as_slice(), placed.as_slice()) {
                ([one], _) => Some((one.0, one.1)),
                ([], [one]) => Some((one.0, one.1)),
                _ => None,
            }
        }
    }

    fn not_checked(&mut self, r: Reason, n: u64) {
        self.cov.add_not_checked(r, n);
    }

    fn fail(&mut self, mut f: Finding) {
        if let Some(b) = &self.e13
            && f.addr == Some(b.0.p)
            && matches!(f.rule, Rule::Field | Rule::Overflow | Rule::Misaligned)
            && self.e13_field(&b.0, b.1, &b.2)
        {
            f.message.push_str(crate::wrap::E13);
        }
        // §4.4b (WP-170): a PC-relative form against a `--defsym` number in
        // a position-independent output (GNU ld's relocated number, A87).
        if !self.defsym.is_empty()
            && f.rule == Rule::Field
            && f.message.ends_with("(spec/plumb.md §5.1a)")
            && f.symbol
                .as_deref()
                .is_some_and(|n| matches!(self.defsym.get(n), Some(crate::defsym::Expr::Number(_))))
        {
            f.message.push_str(crate::defsym::NUMBER_MOVED);
        }
        self.cov.failed += 1;
        self.findings.push(f);
    }

    /// §5.7a's COMDAT keeper in the order of the line's words `line` (the
    /// command line the verifier reads, never a mortar-written record of
    /// it): an object is ranked by the first word that names its path, an
    /// archive member by the first word that names its archive (its path,
    /// `-l<name>` for `lib<name>.a`, `-l:<file>`).
    /// Without a line the keeper follows `FileId` order.
    /// The line is also the one the options are read from (§3.2a).
    pub fn set_keeper_line(&mut self, line: Option<&[String]>) {
        self.caller_line = line.map(<[String]>::to_vec);
        let Some(words) = line else { return };
        let objs: Vec<&Obj<'a>> = self.objs.iter().collect();
        let rank = |o: &Obj<'a>| {
            if o.member.is_none() {
                return words.iter().position(|w| names_path(w, &o.path));
            }
            // An archive member is read where its archive is (WP-201): the
            // word naming the archive's path, or `-l<name>` / `-l:<file>`
            // naming its file name.
            let file = o.path.rsplit('/').next().unwrap_or(&o.path);
            words.iter().position(|w| {
                names_path(w, &o.path)
                    || w.strip_prefix("-l:").is_some_and(|f| f == file)
                    || w.strip_prefix("-l").is_some_and(|n| {
                        file.strip_prefix("lib").and_then(|f| f.strip_suffix(".a")) == Some(n)
                    })
            })
        };
        let (dropped, keepers) = crate::fate::comdat_ranked(&objs, rank);
        self.comdat = dropped.into();
        self.keepers = keepers.into();
    }

    /// The verifier's keeper of each COMDAT signature (§5.7a's, in the
    /// order `comdat` was decided in).
    pub(crate) fn keepers(&self) -> &crate::fate::Keepers {
        let objs = self.objs;
        self.keepers.get_or_init(|| {
            let all: Vec<&Obj<'a>> = objs.iter().collect();
            crate::fate::comdat_ranked(&all, |_| None).1
        })
    }

    /// Whether section `k` of object `oi` is in a COMDAT copy the
    /// verifier's keeper drops.
    pub(crate) fn comdat_dropped_section(&self, oi: usize, k: usize) -> bool {
        let objs = self.objs;
        let dropped = self.comdat.get_or_init(|| crate::fate::comdat_dropped(objs));
        dropped.get(oi).and_then(|d| d.get(k)).copied().unwrap_or(false)
    }

    /// Sets the line's `--defsym` assignments (§4.4b, WP-170 part 2).
    pub(crate) fn set_defsym(&mut self, d: crate::defsym::Defsym) {
        self.cov.defsym_not_judged = d.unjudged_text();
        self.defsym = d;
    }

    /// `def[name]` (§4.4b) when the line's `--defsym` of `name` is
    /// derived: a number is its value (modulo the address width: the
    /// three references truncate on i386, mortar refuses it, §7.2 item 3),
    /// an alias is `B`'s target, `B ± N` `B`'s address plus `N`. `B` is a
    /// root, looked up by its own name (no `--wrap` key).
    fn defsym_target(&self, name: &str) -> Option<Target> {
        use crate::defsym::Expr;
        let e = self.defsym.get(name)?;
        let narrow = self.image.class() != mortar_elf::Class::Elf64;
        Some(match e {
            Expr::Number(n) => Target::Addr(if narrow { n & 0xffff_ffff } else { *n }),
            Expr::Alias(b) => self.global_target(b.clone(), true, false, None),
            Expr::Offset(b, n) => match self.global_target(b.clone(), true, false, None) {
                Target::Addr(a) => Target::Addr(a.wrapping_add(*n as u64)),
                Target::Tls(a) => Target::Tls(a.wrapping_add(*n as u64)),
                Target::Ifunc { .. } => Target::Skip(Reason::Defsym),
                t => t,
            },
        })
    }

    /// `def[name]`'s value (§4.4b) for the `.symtab` rule; `None` when
    /// `name` has no derived `--defsym`.
    pub(crate) fn defsym_value(&self, name: &str) -> Option<crate::defsym::Value> {
        use crate::defsym::Value;
        Some(match self.defsym_target(name)? {
            Target::Addr(a) => Value::Addr(a),
            Target::Tls(a) => Value::Tls(a),
            Target::Ifunc { resolver, .. } => Value::Ifunc(resolver),
            _ => Value::Unknown,
        })
    }

    /// Sets the line's `--wrap` words (§4.4a, WP-170), and finds refused
    /// item 3's shape in a reference's output: a word with two or more
    /// input definitions, one of them in a group member (`SHF_GROUP`) the
    /// output does not place (a dropped COMDAT copy).
    pub(crate) fn set_wrap(&mut self, wrap: crate::wrap::Wrap) {
        self.wrap_comdat.clear();
        if self.format != Format::Mortar {
            for sym in wrap.syms() {
                let Some(defs) = self.defs.get(sym) else { continue };
                if defs.len() < 2 {
                    continue;
                }
                let dropped = defs.iter().find(|&&(doi, di)| {
                    self.objs[doi]
                        .symtab
                        .and_then(|t| t.get(SymbolIndex(di)).ok().map(|d| (t, d)))
                        .is_some_and(|(t, d)| match t.section_of(SymbolIndex(di), &d) {
                            Ok(SymbolSection::Section(k)) => {
                                let k = k.0 as usize;
                                self.objs[doi].secs.get(k).is_some_and(|x| {
                                    x.shdr.sh_flags & mortar_elf::consts::SHF_GROUP != 0
                                }) && self.objs[doi].no_place(k, self.output_eh_frame().is_some())
                            }
                            _ => false,
                        })
                });
                if let Some(&(doi, _)) = dropped {
                    self.wrap_comdat.insert(sym.to_string(), self.objs[doi].label());
                }
            }
        }
        self.wrap = wrap;
        self.cov.wrap_not_judged = self.wrap_not_judged();
    }

    /// The coverage words of the `--wrap` shapes not judged (§4.4a).
    pub(crate) fn wrap_not_judged(&self) -> Vec<String> {
        let mut v = self.wrap.unjudged_text();
        v.extend(self.wrap_comdat.iter().map(|(s, file)| {
            format!(
                "--wrap={s}: a definition in a dropped COMDAT copy ({file}); the redirection of this shape is not derived (resolve.md §7.1, refused item 3)"
            )
        }));
        v
    }

    /// `__wrap_SYM`'s address and name when symbol `idx` of object `oi`
    /// is a defined global mention of a `--wrap` word SYM (E13: not
    /// wrapped).
    fn e13_target(&self, oi: usize, idx: u32) -> Option<(u64, String)> {
        if self.wrap.is_empty() {
            return None;
        }
        let t = self.objs[oi].symtab?;
        let sym = t.get(SymbolIndex(idx)).ok()?;
        if !matches!(sym.binding(), STB_GLOBAL | STB_WEAK | STB_GNU_UNIQUE)
            || !matches!(t.section_of(SymbolIndex(idx), &sym), Ok(SymbolSection::Section(_)))
        {
            return None;
        }
        let name = std::str::from_utf8(t.name(&sym).ok()?).ok()?;
        if !self.wrap.contains(name) {
            return None;
        }
        let n = format!("__wrap_{name}");
        let w = self.image.global(&n)?;
        (w.sym.st_shndx != 0).then_some((w.sym.st_value, n))
    }

    /// Whether `rec`'s field holds what binding it to `__wrap_SYM` at `w`
    /// gives (E13's lld form: the defining object's reference wrapped),
    /// directly or, for a branch, through a PLT entry whose word holds
    /// `__wrap_SYM` (a preemptible `__wrap_SYM` of a shared object).
    fn e13_field(&self, rec: &Rec, w: u64, name: &str) -> bool {
        if self.e13_direct(rec, w) {
            return true;
        }
        if rec.def.op != Op::Prel {
            return false;
        }
        let bound = self.dynbind.get(name).copied();
        let t = if matches!(rec.def.r_type, JUMP26 | CALL26)
            && self.image.header.e_machine == mortar_elf::consts::EM_AARCH64
        {
            match a64::branch(rec.out_word as u32, rec.p) {
                Some((_, t)) => t,
                None => return false,
            }
        } else if rec.def.field == Field::Data(4) {
            rec.p.wrapping_sub(rec.addend as u64).wrapping_add(rec.out_word as u32 as i32 as u64)
        } else {
            return false;
        };
        [Some(w), bound].into_iter().flatten().any(|v| self.plt_hop(t, v))
    }

    fn e13_direct(&self, rec: &Rec, w: u64) -> bool {
        let sa = w.wrapping_add(rec.addend as u64);
        let x = match rec.def.op {
            Op::Abs => sa as i64,
            Op::Prel => sa.wrapping_sub(rec.p) as i64,
            _ => return false,
        };
        let x = if self.image.header.e_machine == mortar_elf::consts::EM_386
            && !matches!(rec.def.check, Check::Signed(32) | Check::Unsigned(32))
        {
            i64::from(x as i32)
        } else {
            x
        };
        matches!(judge(rec, x), Verdict::Ok(_))
    }

    fn finding(
        rule: Rule,
        rec: &Rec,
        site: &Site,
        symbol: &str,
        msg: impl Into<String>,
    ) -> Finding {
        let mut f = Finding::new(rule, msg).at(rec.p).site(site.clone());
        f.reloc = Some(rec.def.name.to_string());
        if !symbol.is_empty() {
            f.symbol = Some(symbol.to_string());
        }
        f
    }

    /// The instruction or data the program sees at a record's place
    /// (§5.3, W8: the erratum 843419 patch is followed).
    fn out_word(
        &self,
        p: u64,
        def: &Def,
        input_word: u64,
        nonalloc: Option<usize>,
    ) -> Option<(u64, bool)> {
        if let Some(i) = nonalloc {
            return Some((self.image.nonalloc_value(i, p, def.field.width())?, false));
        }
        let w = self.image.value(p, def.field.width())?;
        if def.field.width() != 4 || matches!(def.field, Field::Data(_)) {
            return Some((w, false));
        }
        let w32 = w as u32;
        if a64::erratum_843419_site(p)
            && a64::is_ldst_unsigned_imm(input_word as u32)
            && let Some(word) = self.erratum_patch(p, w32)
        {
            return Some((u64::from(word), true));
        }
        Some((w, false))
    }

    /// The word an erratum 843419 patch executes in place of the branch at
    /// `p`, if `word` is such a branch (§5.3, W8).
    pub fn erratum_patch(&self, p: u64, word: u32) -> Option<u32> {
        let (false, t) = a64::branch(word, p)? else { return None };
        if self.occupied.contains(t) {
            return None;
        }
        let moved = self.image.insn(t)?;
        let back = self.image.insn(t.wrapping_add(4))?;
        (a64::branch(back, t.wrapping_add(4)) == Some((false, p.wrapping_add(4)))
            && a64::is_ldst_unsigned_imm(moved))
        .then_some(moved)
    }

    /// Checks every record of section `si` of object `oi` (§5).
    #[allow(clippy::too_many_lines, clippy::many_single_char_names)] // §5.1, step by step
    pub fn section(&mut self, oi: usize, si: usize) {
        // The previous object's own piece groups are complete (WP-109).
        // Objects are judged in index order; any other order leaves
        // every group to `finish_pieces`.
        match self.pieces_last {
            Some(last) if last < oi && self.pieces_early => self.finish_pieces_of(last),
            Some(last) if last > oi => self.pieces_early = false,
            _ => {}
        }
        if self.pieces_last.is_none_or(|last| last < oi) {
            self.pieces_last = Some(oi);
        }
        let objs = self.objs;
        let o = &objs[oi];
        let s = &o.secs[si];
        if s.alloc() && s.is_eh_frame(o.elf.machine()) {
            // §6.4 (WP-201): an `.eh_frame` that is a member of a COMDAT
            // copy the keeper drops is not read by the link; its records
            // are absent.
            let dropped = self.comdat.get_or_init(|| crate::fate::comdat_dropped(objs));
            if dropped.get(oi).and_then(|d| d.get(si)).copied().unwrap_or(false) {
                let n = s.relocs.and_then(|rs| o.records(rs)).map_or(0, |r| r.len() as u64);
                self.cov.absent_records += n;
                return;
            }
            // §6.4: by CIE and FDE, after every other section.
            self.eh_sections.push((oi, si));
            return;
        }
        let Some(rs) = s.relocs else { return };
        let Some(raw) = o.records(rs) else {
            self.findings.push(
                Finding::new(
                    Rule::InputUnreadable,
                    format!("{}: its relocation section {rs} cannot be read", o.label()),
                )
                .site(o.site(si, 0)),
            );
            return;
        };
        let n = raw.len() as u64;
        let machine = o.elf.machine();
        match &s.place {
            // §5.8: mold's map lists no input rows of a compressed output
            // section; its records are named, not judged.
            Place::Absent
                if !s.alloc()
                    && self.image.nonalloc_section(&s.name).is_some_and(|i| {
                        self.image.sections[i].shdr.sh_flags & SHF_COMPRESSED != 0
                    }) =>
            {
                self.cov.records += n;
                return self.not_checked(Reason::Compressed, n);
            }
            Place::Absent | Place::Discarded => {
                self.cov.absent_records += n;
                return;
            }
            _ => {}
        }
        self.cov.records += n;
        // A non-alloc section is checked in the file bytes of the output
        // section of its name (§5.8).
        let nonalloc = if s.alloc() {
            None
        } else {
            let out = self.image.nonalloc_section(&s.name);
            if s.shdr.sh_flags & SHF_COMPRESSED != 0
                || out.is_some_and(|i| self.image.sections[i].shdr.sh_flags & SHF_COMPRESSED != 0)
            {
                return self.not_checked(Reason::Compressed, n);
            }
            let Some(i) = out else {
                return self.not_checked(Reason::SectionNotLocatable, n);
            };
            Some(i)
        };
        let reason = match &s.place {
            Place::Ambiguous => Some(Reason::SectionNotLocatable),
            Place::Merged => Some(Reason::InMergeable),
            Place::Rewritten | Place::Absent | Place::Discarded => Some(Reason::EhFrame),
            Place::Whole(_) | Place::Pieces(_) => None,
        };
        if let Some(r) = reason {
            return self.not_checked(r, n);
        }
        let x64 = machine == Machine::X86_64;
        let i386 = machine == Machine::I386;
        if i386 && raw.first().is_some_and(|r| r.3.is_some()) {
            // spec/x86.md X86-2: refused; the references differ (W12).
            let f = Finding::new(
                Rule::Insn,
                format!("{rs} is SHT_RELA in an EM_386 object (spec/x86.md X86-2)"),
            )
            .site(o.site(si, 0));
            self.fail(f);
            return self.not_checked(Reason::Type, n);
        }
        if i386 {
            self.x86_rel_overlaps(oi, si, &raw);
        }
        let place = s.place.clone();
        let data = o.data(si);
        let nobits = s.shdr.sh_type == SHT_NOBITS;
        // Records with their places; the pieces of a dropped part are not
        // in the output.
        let mut recs: Vec<Rec> = Vec::with_capacity(raw.len());
        for (off, sym, ty, addend) in raw {
            let Some(p) = place_at(&place, off) else {
                self.cov.records -= 1;
                self.cov.absent_records += 1;
                continue;
            };
            if ty == 0 || (!x64 && ty == 256) {
                self.cov.none += 1;
                continue;
            }
            let def = match machine {
                Machine::X86_64 => crate::x64::def(ty),
                Machine::I386 => crate::x86::def(ty),
                Machine::AArch64 => a64::def(ty),
            };
            let Some(def) = def else {
                // spec/x86.md X86-20: the same for i386 (W14).
                if i386 && crate::x86::dynamic_only(ty) {
                    let mut f = Finding::new(
                        Rule::Insn,
                        format!(
                            "relocation type {ty} is defined for dynamic relocations only, not for an object file (spec/x86.md X86-20)"
                        ),
                    )
                    .at(p)
                    .site(o.site(si, off));
                    f.found = Some(u64::from(ty));
                    self.fail(f);
                    continue;
                }
                // spec/x64.md X-14: the dynamic-only types have no static
                // value; GNU ld writes S + A for them (W3).
                if x64 && matches!(ty, 5..=8 | 16 | 36..=38) {
                    let mut f = Finding::new(
                        Rule::Insn,
                        format!(
                            "relocation type {ty} is defined for dynamic relocations only, not for an object file (spec/x64.md X-14)"
                        ),
                    )
                    .at(p)
                    .site(o.site(si, off));
                    f.found = Some(u64::from(ty));
                    self.fail(f);
                    continue;
                }
                self.not_checked(Reason::Type, 1);
                continue;
            };
            let w = usize::from(def.field.width());
            let site = o.site(si, off);
            let input = usize::try_from(off).ok().and_then(|i| data.get(i..i.checked_add(w)?));
            let input_word = match input {
                Some(b) => b.iter().rev().fold(0u64, |acc, &x| acc << 8 | u64::from(x)),
                None if nobits => 0,
                None => {
                    self.fail(
                        Finding::new(
                            Rule::InputUnreadable,
                            format!("the relocated place {off:#x} is outside the section"),
                        )
                        .site(site),
                    );
                    continue;
                }
            };
            // R8 (§4.3): the instruction is of the class the type is for.
            if def.field.width() == 4
                && !matches!(def.field, Field::Data(_))
                && !nobits
                && !a64::insn_class_ok(ty, input_word as u32)
            {
                let mut f = Finding::new(
                    Rule::Insn,
                    format!(
                        "the instruction {:#010x} is not of the class {} is defined for",
                        input_word as u32, def.name
                    ),
                )
                .at(p)
                .site(site);
                f.reloc = Some(def.name.into());
                self.fail(f);
                continue;
            }
            let addend = addend.unwrap_or_else(|| a64::decode(def.field, input_word, 0));
            let Some((out_word, patched)) = self.out_word(p, def, input_word, nonalloc) else {
                let mut f = Finding::new(
                    Rule::Place,
                    "the relocated place is not in the output's loaded memory",
                )
                .at(p)
                .site(site);
                f.reloc = Some(def.name.into());
                self.fail(f);
                continue;
            };
            recs.push(Rec { off, sym, def, addend, p, input_word, out_word, patched });
        }
        // Partners are found by offset (§5.3).
        let mut order: Vec<usize> = (0..recs.len()).collect();
        order.sort_by_key(|&i| (recs[i].off, i));
        let sorted: Vec<(u64, usize)> = order.iter().map(|&i| (recs[i].off, i)).collect();
        for i in 0..recs.len() {
            // §6.9 (P-E4): each record's outcome, when asked for.
            let before = self.trace.is_some().then(|| crate::equiv::tally(self));
            self.record(oi, si, &recs, &sorted, i);
            if let Some(b) = before {
                crate::equiv::traced(self, (oi, si, recs[i].off), &b);
            }
        }
    }

    /// §5.1 for one record, with §5.7a's form of R5 at a demoted weak
    /// mention whose key nothing live defines (WP-178): the record is R5
    /// (§4.4 step 0), and is judged as an undefined weak reference only
    /// to say whether its field holds that form (the words
    /// [`DEMOTED_WEAK`]); the trial's own outcome is not kept.
    fn record(&mut self, oi: usize, si: usize, recs: &[Rec], sorted: &[(u64, usize)], i: usize) {
        let rec = &recs[i];
        // §5.7b (WP-201): a field of `.gcc_except_table` that refers to a
        // section not in the output is judged by the FDE that owns it.
        let s = &self.objs[oi].secs[si];
        let mut owner = None;
        if s.alloc()
            && s.name == ".gcc_except_table"
            && let Target::Discarded(sec) = self.resolve_at(oi, rec.sym, rec.addend).0
        {
            match self.lsda_owner(oi, si, rec.off) {
                ehowner::Owner::Dead => return self.dead_lsda_field(oi, si, rec, &sec),
                o => owner = Some(o),
            }
        }
        let n = self.findings.len();
        self.record_owned(oi, si, recs, sorted, i);
        let Some(owner) = owner else { return };
        let form = live_form(rec);
        for f in &mut self.findings[n..] {
            if f.rule != Rule::DiscardedTarget || f.addr != Some(rec.p) {
                continue;
            }
            match &owner {
                ehowner::Owner::Live if !f.message.ends_with(DEMOTED_WEAK) => {
                    f.message.push_str(form.unwrap_or(""));
                }
                ehowner::Owner::None => f.message.push_str(NO_LSDA_OWNER),
                ehowner::Owner::Unknown(why) => {
                    f.message.push_str(" (the field's owner is not known: ");
                    f.message.push_str(why);
                    f.message.push(')');
                }
                _ => {}
            }
        }
    }

    /// §5.7b: a field of an LSDA no written FDE reads (its owner is dead).
    /// The record is not in the output in effect; under mortar's map the
    /// field is 0 (`spec/resolve.md` §6.3), which is checked.
    fn dead_lsda_field(&mut self, oi: usize, si: usize, rec: &Rec, sec: &str) {
        if self.format != Format::Mortar {
            self.cov.records -= 1;
            self.cov.absent_records += 1;
            return;
        }
        let w = u32::from(rec.def.field.width()) * 8;
        let got = if w >= 64 { rec.out_word } else { rec.out_word & ((1u64 << w) - 1) };
        if got == 0 {
            return self.cov.add_verified(Via::Tombstone, rec.def.r_type);
        }
        let site = self.objs[oi].site(si, rec.off);
        let symbol = self.sym_name(oi, rec.sym);
        let mut f = Self::finding(
            Rule::Field,
            rec,
            &site,
            &symbol,
            format!(
                "a field of an LSDA no written FDE reads, referring to section '{sec}', which is not in the output, holds another value than 0 (resolve.md §6.3)"
            ),
        );
        f.expected = Some(0);
        f.found = Some(got);
        self.fail(f);
    }

    /// §5.1 for one record, with §5.7a's form of R5 at a demoted weak
    /// mention (see [`Engine::record`]).
    fn record_owned(
        &mut self,
        oi: usize,
        si: usize,
        recs: &[Rec],
        sorted: &[(u64, usize)],
        i: usize,
    ) {
        let rec = &recs[i];
        let Some((name, sec)) = self.demoted_weak(oi, si, rec) else {
            return self.record_judged(oi, si, recs, sorted, i);
        };
        let site = self.objs[oi].site(si, rec.off);
        let symbol = self.sym_name(oi, rec.sym);
        let plain = Self::finding(
            Rule::DiscardedTarget,
            rec,
            &site,
            &symbol,
            format!("refers to section '{sec}', which is not in the output"),
        );
        let (cov, n) = (self.cov.clone(), self.findings.len());
        let members = |e: &Self| e.got_groups.values().map(|g| g.members.len()).sum::<usize>();
        let pending = members(self);
        self.trial = Some((oi, rec.sym, name));
        self.record_judged(oi, si, recs, sorted, i);
        self.trial = None;
        let deferred = members(self) > pending;
        let shown = |c: &Coverage| {
            (
                c.verified.values().sum::<u64>() + c.accepted.values().sum::<u64>(),
                c.not_checked.values().sum::<u64>(),
            )
        };
        let (was, now) = (shown(&cov), shown(&self.cov));
        let clean = self.findings.len() == n && now.1 == was.1;
        let weak_form = clean && now.0 > was.0;
        self.findings.truncate(n);
        self.cov = cov;
        if deferred {
            // The GOT group decides (`finish_got`); a trial that already
            // failed is plain R5 whatever it decides.
            self.demoted_pending.insert((rec.p, rec.def.r_type), (plain, clean));
            return;
        }
        let mut f = plain;
        if weak_form {
            f.message.push_str(DEMOTED_WEAK);
        }
        self.fail(f);
    }

    /// §5.7a: when `rec`, in an allocated section of object `oi`, is
    /// against a weak symbol of `oi` defined in a section of a COMDAT copy
    /// that is not kept (the verifier's keeper) and without a place, and
    /// §4.4 step 0 makes it R5 (no definition of its key has a place, and
    /// the loader does not bind it): the key and the section's name.
    fn demoted_weak(&mut self, oi: usize, si: usize, rec: &Rec) -> Option<(String, String)> {
        if !self.objs[oi].secs.get(si)?.alloc() {
            return None;
        }
        self.demoted_weak_sym(oi, rec.sym, rec.addend)
    }

    /// [`Engine::demoted_weak`] for symbol `sym` of object `oi` with addend
    /// `addend`, wherever the record is.
    pub(crate) fn demoted_weak_sym(
        &mut self,
        oi: usize,
        sym: u32,
        addend: i64,
    ) -> Option<(String, String)> {
        let t = self.objs[oi].symtab?;
        let s = t.get(SymbolIndex(sym)).ok()?;
        if s.binding() != STB_WEAK {
            return None;
        }
        let Ok(SymbolSection::Section(k)) = t.section_of(SymbolIndex(sym), &s) else {
            return None;
        };
        let k = k.0 as usize;
        let objs = self.objs;
        let dropped = self.comdat.get_or_init(|| crate::fate::comdat_dropped(objs));
        if !dropped.get(oi).and_then(|d| d.get(k)).copied().unwrap_or(false)
            || !self.objs[oi].no_place(k, self.output_eh_frame().is_some())
        {
            return None;
        }
        let name = String::from_utf8_lossy(t.name(&s).ok()?).into_owned();
        match self.resolve_at(oi, sym, addend).0 {
            Target::Discarded(sec) => Some((name, sec)),
            _ => None,
        }
    }

    #[allow(clippy::too_many_lines)] // §5.1 steps 2 to 7, in order
    fn record_judged(
        &mut self,
        oi: usize,
        si: usize,
        recs: &[Rec],
        sorted: &[(u64, usize)],
        i: usize,
    ) {
        let rec = &recs[i];
        let site = self.objs[oi].site(si, rec.off);
        let symbol = self.sym_name(oi, rec.sym);
        self.e13 = self.e13_target(oi, rec.sym).map(|(w, n)| Box::new((rec.clone(), w, n)));
        // A non-alloc reference to a section not in the output holds the
        // linker's tombstone (§5.8).
        if !self.objs[oi].secs[si].alloc()
            && let (Target::Discarded(_), _) = self.resolve_at(oi, rec.sym, rec.addend)
        {
            let after = find_at(
                recs,
                sorted,
                rec.off.checked_add(u64::from(rec.def.field.width())),
                |_| true,
            )
            .is_some();
            return self.tombstone(oi, si, rec, &site, &symbol, after);
        }
        // A non-alloc word against a name only a shared library defines
        // holds its link-time value (§5.8, WP-104).
        if !self.objs[oi].secs[si].alloc() && self.dso_word(oi, rec, &site, &symbol) {
            return;
        }
        // i386 (§4.5, §5.11).
        if self.image.header.e_machine == mortar_elf::consts::EM_386 {
            return self.x86_record(oi, si, recs, i, site, symbol);
        }
        // x86-64 (§4.5, §5.9).
        if is_x64(rec) {
            return self.x64_record(oi, si, recs, sorted, i, site, symbol);
        }
        let partner = |delta: i64, types: &[u32]| -> Option<&Rec> {
            find_at(recs, sorted, rec.off.checked_add_signed(delta), |r| {
                r.sym == rec.sym && r.addend == rec.addend && types.contains(&r.def.r_type)
            })
        };
        // The call of a traditional GD or LD sequence (§4.7, W12, W13).
        if rec.def.r_type == CALL26
            && symbol == TLS_GET_ADDR
            && self.tls_call(oi, si, recs, sorted, rec, &site, &symbol)
        {
            return;
        }
        if rec.def.op.is_tls() {
            return self.tls_record(oi, si, recs, sorted, rec, site, symbol);
        }
        let (target, addend) = self.resolve_at(oi, rec.sym, rec.addend);
        let sa = match target {
            Target::Skip(r) => return self.not_checked(r, 1),
            // A non-TLS type against a thread-local symbol (§4.7).
            Target::Tls(_) => return self.not_checked(Reason::Tls, 1),
            Target::Discarded(name) => {
                let f = Self::finding(
                    Rule::DiscardedTarget,
                    rec,
                    &site,
                    &symbol,
                    format!("refers to section '{name}', which is not in the output"),
                );
                return self.fail(f);
            }
            Target::Empty(sec) => return self.empty_record(rec, &site, &symbol, &sec),
            Target::Outside(name) => {
                if self.format != Format::Mortar {
                    return self.not_checked(Reason::PieceNotLocatable, 1);
                }
                let f = Self::finding(
                    Rule::MergePiece,
                    rec,
                    &site,
                    &symbol,
                    format!(
                        "a section-symbol reference outside section '{name}' (spec/synth.md S-18 refuses it)"
                    ),
                );
                return self.fail(f);
            }
            Target::Ifunc { resolver, key } => {
                return self.ifunc_record(oi, rec, site, symbol, resolver, key, &partner);
            }
            Target::Merge { oi: doi, sec, value, section_sym } => {
                return self.merge_record(
                    doi,
                    sec,
                    value,
                    section_sym,
                    rec,
                    &site,
                    &symbol,
                    (oi, si),
                    &partner,
                );
            }
            Target::Addr(s) => s.wrapping_add(addend as u64),
            // An undefined weak reference resolves to zero, or to the place
            // for a PC-relative type that is not a branch (§4.4; branches
            // have W4 and W5).
            Target::UndefWeak
                if rec.def.op == Op::Prel
                    && !matches!(rec.def.r_type, TSTBR14 | CONDBR19 | JUMP26 | CALL26) =>
            {
                rec.p.wrapping_add(rec.addend as u64)
            }
            Target::UndefWeak => rec.addend as u64,
        };
        let weak = matches!(target, Target::UndefWeak);
        let op = rec.def.op;
        if let Op::Slot(slot, _) = op {
            self.got_demands.insert(sa);
            if let Some(rw) = got_rewrite(rec, sa, &partner) {
                self.cov.add_accepted(rw, rec.def.r_type);
                return;
            }
            let Some(ev) = self.slot_evidence(rec) else {
                return self.not_checked(Reason::SymbolUnresolved, 1);
            };
            let pend = Pending { rec: rec.clone(), site, symbol, want: Want::Word(sa) };
            self.got_groups
                .entry((slot, oi, rec.sym, rec.addend))
                .or_default()
                .members
                .push((pend, ev));
            return;
        }
        let Some(x) = x_direct(op, sa, rec.p, self.got) else {
            return self.not_checked(Reason::SymbolUnresolved, 1);
        };
        let verdict = judge(rec, x);
        if let Verdict::Ok(rw) = verdict {
            // Stage 3's view 2 re-reads loaded words only: a non-alloc
            // place (`.debug_*`) is not in memory and no record moves it.
            if op == Op::Abs
                && matches!(rec.def.field, Field::Data(4 | 8))
                && !weak
                && self.objs[oi].secs[si].alloc()
            {
                let s = sa.wrapping_sub(addend as u64);
                let internal = self.internal_value(s, oi, rec.sym) && !self.absolute(oi, rec.sym);
                self.abs_words.push((rec.p, rec.def.field.width(), sa, internal));
            }
            match rw {
                Some(r) => self.cov.add_accepted(r, rec.def.r_type),
                None => self.cov.add_verified(Via::Direct, rec.def.r_type),
            }
            return;
        }
        if matches!(rec.def.r_type, JUMP26 | CALL26)
            && let Some((_, t)) = a64::branch(rec.out_word as u32, rec.p)
            && rec.out_word as u32 & 0xfc00_0000 == rec.input_word as u32 & 0xfc00_0000
            && self.plt_hop(t, sa)
        {
            return self.cov.add_verified(Via::Plt, rec.def.r_type);
        }
        if let Some(rw) = self.direct_rewrite(rec, sa, weak, &partner) {
            self.note_veneer(rw, rec, &site, &symbol, sa);
            self.cov.add_accepted(rw, rec.def.r_type);
            return;
        }
        let f = match verdict {
            Verdict::Overflow => {
                let mut f = Self::finding(
                    Rule::Overflow,
                    rec,
                    &site,
                    &symbol,
                    format!("the value {x:#x} fails the check {}", rec.def.check.describe()),
                );
                f.expected = Some(x as u64);
                f
            }
            Verdict::Misaligned => Self::finding(
                Rule::Misaligned,
                rec,
                &site,
                &symbol,
                format!("the value {x:#x} has low bits the field drops"),
            ),
            Verdict::Mismatch(e) => {
                // §5.8: lld's S = 0 for an undefined weak in a non-alloc
                // PC-relative word (A43), named for the anomaly.
                let msg = if weak
                    && op == Op::Prel
                    && !self.objs[oi].secs[si].alloc()
                    && rec.out_word
                        == rec.def.field.encode(rec.input_word, x.wrapping_sub(rec.p as i64))
                {
                    format!("the relocated field holds the wrong value{}", dsoword::WEAK_ZERO)
                } else {
                    "the relocated field holds the wrong value".to_string()
                };
                let mut f = Self::finding(Rule::Field, rec, &site, &symbol, msg);
                f.expected = Some(e);
                f.found = Some(rec.out_word);
                f
            }
            Verdict::Ok(_) => unreachable!("handled above"),
        };
        self.fail(f);
    }

    /// The address evidence of a GOT-indirect record (§5.5); `None` when
    /// the GOT base it needs is unknown.
    fn slot_evidence(&self, rec: &Rec) -> Option<Evidence> {
        let e = self.slot_evidence_wide(rec)?;
        // ELFCLASS32: an address is the value modulo 2^32.
        Some(match e {
            Evidence::Exact(v) if self.word() == 4 => Evidence::Exact(v & 0xffff_ffff),
            e => e,
        })
    }

    fn slot_evidence_wide(&self, rec: &Rec) -> Option<Evidence> {
        let Op::Slot(_, form) = rec.def.op else { return Some(Evidence::None) };
        let d = a64::decode(rec.def.field, rec.out_word, rec.p);
        Some(match (form, rec.def.field) {
            (Form::Prel, _) => Evidence::Exact(rec.p.wrapping_add(d as u64)),
            (Form::PrelA, _) => {
                Evidence::Exact(rec.p.wrapping_add(d as u64).wrapping_sub(rec.addend as u64))
            }
            (Form::OffA, _) => {
                Evidence::Exact(self.got?.wrapping_add(d as u64).wrapping_sub(rec.addend as u64))
            }
            (Form::AbsA, _) => Evidence::Exact((d as u64).wrapping_sub(rec.addend as u64)),
            (Form::Off, Field::MovwSigned(16) | Field::Movw(16)) => {
                // MOVN writes the complement (AAELF64 "MOVW" groups).
                let w = rec.out_word as u32;
                let v = d as u64;
                let movn = (w >> 29).trailing_zeros() >= 2;
                Evidence::MovHi(if movn { !v } else { v })
            }
            (Form::Off, Field::Movw(0)) => Evidence::MovLo(d as u64 & 0xffff),
            (Form::Off, _) => Evidence::Exact(self.got?.wrapping_add(d as u64)),
            (Form::PageOff, _) => Evidence::Exact(a64::page(self.got?).wrapping_add(d as u64)),
            (Form::Page, _) => page_evidence(rec),
            (Form::Abs, _) => Evidence::Low(d as u64 & 0xfff),
        })
    }

    /// S+A of a TLS record and the TLS block, or what to report (§4.7).
    fn tls_target(&mut self, oi: usize, rec: &Rec) -> Result<(TlsBlock, u64), Outcome> {
        let (t, a) = self.resolve_at(oi, rec.sym, rec.addend);
        let sa = match t {
            Target::Tls(s) => s.wrapping_add(a as u64),
            Target::Skip(r) => return Err(Outcome::Skip(r)),
            Target::UndefWeak => return self.weak0_tls(rec, a),
            Target::Merge { .. } | Target::Outside(_) => {
                return Err(Outcome::Skip(Reason::Tls));
            }
            Target::Addr(_) | Target::Ifunc { .. } => {
                return Err(Outcome::Fail(
                    Rule::Insn,
                    "a thread-local relocation against a symbol that is not thread-local".into(),
                    None,
                ));
            }
            Target::Discarded(name) => {
                return Err(Outcome::Fail(
                    Rule::DiscardedTarget,
                    format!("refers to section '{name}', which is not in the output"),
                    None,
                ));
            }
            Target::Empty(_) => return Err(Outcome::Skip(Reason::SectionNotLocatable)),
        };
        match self.tls {
            Ok(Some(b)) => Ok((b, sa)),
            Ok(None) => Err(Outcome::Fail(
                Rule::NoTls,
                "a thread-local relocation in an output without PT_TLS".into(),
                None,
            )),
            Err(_) => Err(Outcome::Skip(Reason::Tls)),
        }
    }

    /// An AArch64 thread-local undefined weak in a static output (§4.7,
    /// a64.md §7 "thread-local `weak0`"): a block whose TPREL and DTPREL
    /// of `S + A` are what the output's linker writes. mortar and lld:
    /// `A` (TP and DTP offset 0, no `PT_TLS` needed); GNU ld and mold, the
    /// formula with `S = 0` against `PT_TLS` (GNU ld's LE, `GD` and mold's
    /// `GD` stay unjudged: no single rule, a64.md §7's experiment).
    fn weak0_tls(&self, rec: &Rec, a: i64) -> Result<(TlsBlock, u64), Outcome> {
        let zero = TlsBlock { start: 0, tcb: 0 };
        if self.dynamic {
            return Err(Outcome::Skip(Reason::Tls));
        }
        let ie = matches!(rec.def.op, Op::Slot(Slot::TpRel | Slot::Desc, _) | Op::DescCall);
        let b = match self.format {
            Format::Mortar | Format::Lld => zero,
            Format::Mold if !matches!(rec.def.op, Op::Slot(Slot::Gd | Slot::Ld, _)) => {
                match self.tls {
                    Ok(Some(b)) => b,
                    _ => return Err(Outcome::Skip(Reason::Tls)),
                }
            }
            Format::Gnu if ie => match self.tls {
                Ok(Some(b)) => b,
                _ => return Err(Outcome::Skip(Reason::Tls)),
            },
            _ => return Err(Outcome::Skip(Reason::Tls)),
        };
        Ok((b, a as u64))
    }

    fn outcome(&mut self, rec: &Rec, site: &Site, symbol: &str, o: Outcome) {
        match o {
            Outcome::Accepted(rw) => self.cov.add_accepted(rw, rec.def.r_type),
            Outcome::Verified => self.cov.add_verified(Via::Direct, rec.def.r_type),
            Outcome::Skip(r) => self.not_checked(r, 1),
            Outcome::Fail(rule, msg, expected) => {
                let mut f = Self::finding(rule, rec, site, symbol, msg);
                f.expected = expected;
                if rule == Rule::Field {
                    f.found = Some(rec.out_word);
                }
                self.fail(f);
            }
        }
    }

    /// A record of a thread-local type (§4.7).
    #[allow(clippy::too_many_arguments)]
    fn tls_record(
        &mut self,
        oi: usize,
        si: usize,
        recs: &[Rec],
        sorted: &[(u64, usize)],
        rec: &Rec,
        site: Site,
        symbol: String,
    ) {
        let (b, sa) = match self.tls_target(oi, rec) {
            Ok(v) => v,
            Err(o) => return self.outcome(rec, &site, &symbol, o),
        };
        // What a slot of this reference holds, relaxed or not (WP-101,
        // `got_demands`): a relaxed sequence may leave its slot, or a GD
        // or descriptor relaxed to initial-exec loads a TPREL slot.
        match rec.def.op {
            Op::Slot(Slot::TpRel | Slot::Desc, _) | Op::DescCall => {
                self.demand(Want::Word(b.tprel(sa)));
            }
            Op::Slot(slot @ (Slot::Gd | Slot::Ld), _) => {
                let module = self.dyn_tls_mod.get(&symbol).copied().unwrap_or(1);
                let ld = slot == Slot::Ld;
                self.demand(Want::Word(b.tprel(sa)));
                self.demand(Want::Pair(module, if ld { 0 } else { b.dtprel(sa) }));
            }
            _ => {}
        }
        let o = match rec.def.op {
            Op::TpRel | Op::DtpRel => {
                let x = if rec.def.op == Op::TpRel { b.tprel(sa) } else { b.dtprel(sa) } as i64;
                match judge(rec, x) {
                    Verdict::Ok(Some(rw)) => Outcome::Accepted(rw),
                    Verdict::Ok(None) => Outcome::Verified,
                    v => {
                        let f = verdict_finding(rec, &site, &symbol, Some((v, x)));
                        return self.fail(f);
                    }
                }
            }
            Op::Slot(Slot::TpRel, _) => {
                let x = b.tprel(sa);
                if ie_to_le(rec, recs, x) {
                    Outcome::Accepted(Rewrite::TlsIeToLe)
                } else if !same_insn(rec) {
                    rewritten_badly()
                } else {
                    return self.push_slot(Slot::TpRel, oi, rec, site, symbol, Want::Word(x));
                }
            }
            Op::Slot(Slot::Desc, _) | Op::DescCall => match desc_seq(rec, recs, sorted) {
                None => Outcome::Skip(Reason::TlsSequence),
                Some(seq) => self.desc_verdict(&seq, b, sa),
            },
            Op::Slot(slot @ (Slot::Gd | Slot::Ld), _) => {
                let ld = slot == Slot::Ld;
                match self.gd_ld(oi, si, recs, sorted, rec, ld, b, sa) {
                    Seq::Relaxed(rw) => Outcome::Accepted(rw),
                    Seq::Bad(msg, e) => Outcome::Fail(Rule::Field, msg, e),
                    Seq::Kept if same_insn(rec) => {
                        let module = self.dyn_tls_mod.get(&symbol).copied().unwrap_or(1);
                        let want = Want::Pair(module, if ld { 0 } else { b.dtprel(sa) });
                        return self.push_slot(slot, oi, rec, site, symbol, want);
                    }
                    Seq::Kept => rewritten_badly(),
                }
            }
            Op::None
            | Op::Abs
            | Op::Prel
            | Op::Page
            | Op::GotRel
            | Op::Size
            | Op::GotPc
            | Op::Slot(Slot::Addr, _) => {
                unreachable!("not a thread-local operation")
            }
        };
        self.outcome(rec, &site, &symbol, o);
    }

    fn push_slot(
        &mut self,
        slot: Slot,
        oi: usize,
        rec: &Rec,
        site: Site,
        symbol: String,
        want: Want,
    ) {
        self.demand(want);
        let Some(ev) = self.slot_evidence(rec) else {
            return self.not_checked(Reason::SymbolUnresolved, 1);
        };
        let pend = Pending { rec: rec.clone(), site, symbol, want };
        self.got_groups
            .entry((slot, oi, rec.sym, rec.addend))
            .or_default()
            .members
            .push((pend, ev));
    }

    /// The four words of a TLSDESC sequence (§4.7): W10 to local-exec in
    /// either of its two forms, W11 to initial-exec.
    fn desc_verdict(&self, seq: &[&Rec; 4], b: TlsBlock, sa: u64) -> Outcome {
        let inw = seq.map(|r| r.input_word as u32);
        let outw = seq.map(|r| r.out_word as u32);
        let t = a64::ldr64_imm(inw[1]).map(|(t, n, _)| (t, n));
        let abi = a64::adrp(inw[0], 0).is_some_and(|(d, _)| d == 0)
            && t.is_some_and(|(_, n)| n == 0)
            && a64::add_imm(inw[2]).is_some_and(|(d, n, _)| d == 0 && n == 0)
            && t.is_some_and(|(t, _)| a64::blr(inw[3]) == Some(t));
        if !abi {
            return Outcome::Fail(
                Rule::Insn,
                "the TLSDESC sequence is not ADRP x0; LDR Xt, [x0]; ADD x0, x0; BLR Xt".into(),
                None,
            );
        }
        let x = b.tprel(sa);
        if x < 1 << 32 {
            let hi = a64::movz(0, x >> 16, 1);
            let lo = a64::movk(0, x & 0xffff, 0);
            if outw == [hi, lo, a64::NOP, a64::NOP] || outw == [a64::NOP, a64::NOP, hi, lo] {
                return Outcome::Accepted(Rewrite::TlsDescToLe);
            }
        }
        // W11: ADRP x0, G; LDR x0, [x0, :lo12:G], G holding TPREL, and
        // two NOPs, after (GNU ld, lld) or before (mold).
        for k in [0, 2] {
            let nops = if k == 0 { [2, 3] } else { [0, 1] };
            if let (Some((0, pg)), Some((0, 0, lo))) =
                (a64::adrp(outw[k], seq[k].p), a64::ldr64_imm(outw[k + 1]))
                && nops.iter().all(|&n| outw[n] == a64::NOP)
                && self.slot_ok(pg.wrapping_add(lo), Want::Word(x))
            {
                return Outcome::Accepted(Rewrite::TlsDescToIe);
            }
        }
        if seq[..3].iter().all(|r| same_insn(r)) && outw[3] == inw[3] && self.dynamic {
            // Kept for the loader: the descriptor the ADRP/LDR reach holds
            // the static-TLS resolver and TPREL once its record is applied.
            if let (Some((_, pg)), Some((_, _, lo))) =
                (a64::adrp(outw[0], seq[0].p), a64::ldr64_imm(outw[1]))
                && self.slot_ok(pg.wrapping_add(lo), Want::Pair(crate::loader::TLSDESC_MARK, x))
            {
                return Outcome::Verified;
            }
            return Outcome::Fail(
                Rule::GotSlot,
                "the TLS descriptor the kept sequence reaches is not resolved to this symbol"
                    .into(),
                Some(x),
            );
        }
        if seq[..3].iter().all(|r| same_insn(r)) && outw[3] == inw[3] {
            // Kept for the dynamic loader: no static executable needs it,
            // and the descriptor's dynamic relocation is stage 3's.
            return Outcome::Skip(Reason::Tls);
        }
        Outcome::Fail(
            Rule::Field,
            "the TLSDESC sequence is neither of W10's local-exec forms nor W11's initial-exec form"
                .into(),
            Some(x),
        )
    }

    /// Whether `g` is a GOT entry that holds `want` (§5.5).
    /// Words are the output class's (4 bytes on ELFCLASS32, a value
    /// compared modulo 2^32).
    fn slot_ok(&self, g: u64, want: Want) -> bool {
        let word = self.word();
        let mask = if word == 8 { u64::MAX } else { 0xffff_ffff };
        let size = if word == 8 { want.size() } else { want.size() / 2 };
        g.is_multiple_of(word)
            && !self.occupied.contains(g)
            && (self.got_section.is_none_or(|(lo, hi)| lo <= g && g.saturating_add(size) <= hi)
                // A reference's lazy TLS descriptor of a shared object is
                // in `.got.plt` (GNU ld; P-D26, WP-121).
                || (self.format != Format::Mortar
                    && matches!(want, Want::Pair(crate::loader::TLSDESC_MARK, _))
                    && self.image.sections.iter().any(|s| {
                        s.name == ".got.plt"
                            && s.shdr.sh_addr <= g
                            && g.saturating_add(size) <= s.shdr.sh_addr.saturating_add(s.shdr.sh_size)
                    })))
            && match want {
                Want::Word(v) => self.image.value(g, word as u8) == Some(v & mask),
                Want::Pair(m, v) => {
                    self.image.value(g, word as u8) == Some(m & mask)
                        && self.image.value(g.wrapping_add(word), word as u8) == Some(v & mask)
                }
            }
    }

    /// A traditional GD or LD sequence (§4.7): `ADRP x0; ADD x0, x0;
    /// BL __tls_get_addr; NOP`, relaxed to local-exec (W12, W13) or kept.
    #[allow(clippy::too_many_arguments)]
    fn gd_ld(
        &mut self,
        oi: usize,
        si: usize,
        recs: &[Rec],
        sorted: &[(u64, usize)],
        rec: &Rec,
        ld: bool,
        b: TlsBlock,
        sa: u64,
    ) -> Seq {
        let (t0, t1) = if ld {
            (TLSLD_ADR_PAGE21, TLSLD_ADD_LO12_NC)
        } else {
            (TLSGD_ADR_PAGE21, TLSGD_ADD_LO12_NC)
        };
        let same = |r: &Rec| r.sym == rec.sym && r.addend == rec.addend;
        let (head, second) = if rec.def.r_type == t0 {
            let Some(s) =
                find_at(recs, sorted, rec.off.checked_add(4), |r| r.def.r_type == t1 && same(r))
            else {
                return Seq::Kept;
            };
            (rec, s)
        } else {
            let Some(h) =
                find_at(recs, sorted, rec.off.checked_sub(4), |r| r.def.r_type == t0 && same(r))
            else {
                return Seq::Kept;
            };
            (h, rec)
        };
        let Some(call) = find_at(recs, sorted, head.off.checked_add(8), |r| {
            r.def.r_type == CALL26 && self.sym_name(oi, r.sym) == TLS_GET_ADDR
        }) else {
            return Seq::Kept;
        };
        let data = self.objs[oi].data(si);
        let in12 = usize::try_from(head.off + 12)
            .ok()
            .and_then(|k| data.get(k..k + 4))
            .map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]));
        let abi = a64::adrp(head.input_word as u32, 0).is_some_and(|(d, _)| d == 0)
            && a64::add_imm(second.input_word as u32).is_some_and(|(d, n, _)| d == 0 && n == 0)
            && in12 == Some(a64::NOP);
        if !abi {
            return Seq::Kept;
        }
        let p12 = head.p.wrapping_add(12);
        let Some(out12) = self.image.value(p12, 4) else { return Seq::Kept };
        let outw =
            [head.out_word as u32, second.out_word as u32, call.out_word as u32, out12 as u32];
        let (want, rw, x) = if ld {
            let Some(add) = a64::add_imm_any(0, 0, b.tcb) else { return Seq::Kept };
            ([a64::mrs_tpidr_el0(0), add, a64::NOP, a64::NOP], Rewrite::TlsLdToLe, b.tcb)
        } else {
            let x = b.tprel(sa);
            let w = [
                a64::movz(0, x >> 16, 1),
                a64::movk(0, x & 0xffff, 0),
                a64::mrs_tpidr_el0(1),
                a64::add_reg(0, 1, 0),
            ];
            (w, Rewrite::TlsGdToLe, x)
        };
        if outw == want && (ld || x < 1 << 32) {
            if !ld {
                self.rewritten_words.insert(p12);
            }
            return Seq::Relaxed(rw);
        }
        // tlsgd-to-ie (§5.3): ADRP x0, G; LDR x0, [x0, :lo12:G];
        // MRS x1, TPIDR_EL0; ADD x0, x1, x0, G holding S's TP offset
        // (in a dynamic output once its TLS_TPREL64 record is applied).
        if !ld
            && outw[2] == a64::mrs_tpidr_el0(1)
            && outw[3] == a64::add_reg(0, 1, 0)
            && let (Some((0, pg)), Some((0, 0, lo))) =
                (a64::adrp(outw[0], head.p), a64::ldr64_imm(outw[1]))
            && self.slot_ok(pg.wrapping_add(lo), Want::Word(x))
        {
            self.rewritten_words.insert(p12);
            return Seq::Relaxed(Rewrite::TlsGdToIe);
        }
        if same_insn(head)
            && same_insn(second)
            && a64::branch(outw[2], call.p).is_some_and(|(l, _)| l)
        {
            return Seq::Kept;
        }
        let what = if ld { "TLSLD" } else { "TLSGD" };
        Seq::Bad(
            format!(
                "the {what} sequence is rewritten, and not to {} (thread-pointer offset {x:#x})",
                if ld {
                    "the local-exec form of W13"
                } else {
                    "W12's local-exec form or the initial-exec form of tlsgd-to-ie"
                }
            ),
            Some(x),
        )
    }

    /// The `BL __tls_get_addr` of a GD or LD sequence: true when decided
    /// here (relaxed, or rewritten badly), false when it is an ordinary
    /// call (§4.7).
    #[allow(clippy::too_many_arguments)]
    fn tls_call(
        &mut self,
        oi: usize,
        si: usize,
        recs: &[Rec],
        sorted: &[(u64, usize)],
        rec: &Rec,
        site: &Site,
        symbol: &str,
    ) -> bool {
        let Some(second) = find_at(recs, sorted, rec.off.checked_sub(4), |r| {
            matches!(r.def.r_type, TLSGD_ADD_LO12_NC | TLSLD_ADD_LO12_NC)
        }) else {
            return false;
        };
        let ld = second.def.r_type == TLSLD_ADD_LO12_NC;
        let Ok((b, sa)) = self.tls_target(oi, second) else { return false };
        match self.gd_ld(oi, si, recs, sorted, second, ld, b, sa) {
            Seq::Kept => false,
            Seq::Relaxed(rw) => {
                self.cov.add_accepted(rw, rec.def.r_type);
                true
            }
            Seq::Bad(msg, e) => {
                self.outcome(rec, site, symbol, Outcome::Fail(Rule::Field, msg, e));
                true
            }
        }
    }

    /// Remembers the veneer a W6 record branches to (R9, §5.4).
    fn note_veneer(&mut self, rw: Rewrite, rec: &Rec, site: &Site, symbol: &str, sa: u64) {
        if rw != Rewrite::Veneer {
            return;
        }
        if let Some((_, t)) = a64::branch(rec.out_word as u32, rec.p) {
            self.veneers
                .entry(t)
                .or_insert_with(|| (rec.clone(), site.clone(), symbol.to_owned(), sa));
        }
    }

    /// R9 (§5.4; spec/a64.md §9): in an output whose
    /// `GNU_PROPERTY_AARCH64_FEATURE_1_AND` has BTI, the loader maps code
    /// as guarded pages, so a veneer that ends in `BR x16`/`BR x17` must
    /// land on `BTI c|j|jc`, `PACIASP` or `PACIBSP`. A veneer ending in a
    /// direct `B` (GNU ld's and lld's `bti c; b T` pad veneer) needs none.
    pub fn check_landing_pads(&mut self) {
        if self.image.header.e_machine != mortar_elf::consts::EM_AARCH64
            || crate::property::aarch64_feature_1_and(self.image) & 1 == 0
        {
            return;
        }
        let veneers = std::mem::take(&mut self.veneers);
        for (t, (rec, site, symbol, sa)) in veneers {
            let Some(hops) = self.veneer_hops(t, sa) else { continue };
            let Some(&(t, dest, _)) =
                hops.iter().find(|h| h.2 && !self.image.insn(h.1).is_some_and(a64::landing_pad))
            else {
                continue;
            };
            let word = self.image.insn(dest);
            let mut f = Self::finding(
                Rule::LandingPad,
                &rec,
                &site,
                &symbol,
                format!(
                    "the veneer at {t:#x} branches indirectly to {dest:#x}, which is not a landing pad (BTI c|j|jc, PACIASP, PACIBSP) in a BTI output"
                ),
            )
            .at(t);
            f.expected = Some(dest);
            f.found = word.map(u64::from);
            self.findings.push(f);
        }
    }

    /// The veneers from `t` to `sa` (W6, §5.4): `(start, destination,
    /// indirect)` of each. A veneer's destination outside every placed
    /// section may be another veneer (GNU ld's `__x_veneer` to its
    /// `__x_bti_veneer`, `bti c; b x`), or a landing pad just before the
    /// target that falls through to it (lld's `__AArch64BTIThunk_x`).
    /// `None` when the chain does not reach `sa` within 4 veneers.
    fn veneer_hops(&self, t: u64, sa: u64) -> Option<Vec<(u64, u64, bool)>> {
        let mut hops = Vec::new();
        let mut at = t;
        for _ in 0..4 {
            let (d, indirect) = a64::veneer_end(self.image, at)?;
            hops.push((at, d, indirect));
            if d == sa {
                return Some(hops);
            }
            if self.occupied.contains(d) {
                return None;
            }
            if d.wrapping_add(4) == sa && self.image.insn(d).is_some_and(a64::landing_pad) {
                return Some(hops);
            }
            at = d;
        }
        None
    }

    /// The rewritings of a non-GOT record (§5.3): W1, W4, W5, W6.
    fn direct_rewrite<'r>(
        &self,
        rec: &Rec,
        sa: u64,
        weak: bool,
        partner: &dyn Fn(i64, &[u32]) -> Option<&'r Rec>,
    ) -> Option<Rewrite> {
        let ty = rec.def.r_type;
        let out = rec.out_word as u32;
        let inp = rec.input_word as u32;
        if ty == JUMP26 || ty == CALL26 {
            if weak && out == a64::NOP {
                return Some(Rewrite::WeakBranchNop);
            }
            let (link, t) = a64::branch(out, rec.p)?;
            if out & 0xfc00_0000 != inp & 0xfc00_0000 {
                return None;
            }
            let _ = link;
            if weak && t == rec.p.wrapping_add(4) {
                return Some(Rewrite::WeakBranchNext);
            }
            if self.plt_hop(t, sa) {
                return None;
            }
            if !self.occupied.contains(t)
                && self.image.load_of(t, 4).is_some()
                && self.veneer_hops(t, sa).is_some()
            {
                return Some(Rewrite::Veneer);
            }
            return None;
        }
        // W1: ADRP Xd, sym; ADD Xd, Xd, :lo12:sym  ->  NOP; ADR Xd, sym.
        let regs_ok = |adrp_in: u32, add_in: u32| {
            let rd = adrp_in & 31;
            a64::adrp(adrp_in, 0).is_some()
                && a64::add_imm(add_in).is_some_and(|(d, n, _)| d == rd && n == rd)
        };
        if (ty == ADR_PREL_PG_HI21 || ty == ADR_PREL_PG_HI21_NC) && out == a64::NOP {
            let add = partner(4, &[ADD_ABS_LO12_NC])?;
            let rd = inp & 31;
            return (regs_ok(inp, add.input_word as u32)
                && a64::adr(add.out_word as u32, add.p) == Some((rd, sa)))
            .then_some(Rewrite::AdrpAddToAdr);
        }
        if ty == ADD_ABS_LO12_NC {
            let pg = partner(-4, &[ADR_PREL_PG_HI21, ADR_PREL_PG_HI21_NC])?;
            let rd = pg.input_word as u32 & 31;
            return (regs_ok(pg.input_word as u32, inp)
                && pg.out_word as u32 == a64::NOP
                && a64::adr(out, rec.p) == Some((rd, sa)))
            .then_some(Rewrite::AdrpAddToAdr);
        }
        None
    }

    /// A reference to a piece of a mergeable section: grouped by the piece
    /// it names, decided by content (§5.6).
    #[allow(clippy::too_many_arguments)]
    fn merge_record<'r>(
        &mut self,
        oi: usize,
        sec: usize,
        value: u64,
        section_sym: bool,
        rec: &Rec,
        site: &Site,
        symbol: &str,
        from: (usize, usize),
        partner: &dyn Fn(i64, &[u32]) -> Option<&'r Rec>,
    ) {
        let loaded = self.objs[from.0].secs[from.1].alloc();
        let (r, extra) = if section_sym {
            (value.wrapping_add(rec.addend as u64), 0)
        } else {
            (value, rec.addend)
        };
        // §5.6 step 1 (synth.md R-P2): a section-symbol reference outside
        // the section names no piece. mortar refuses it (S-18); a
        // reference linker writes its own choice, which is not judged.
        if section_sym && r >= self.objs[oi].secs[sec].shdr.sh_size {
            if self.format != Format::Mortar {
                return self.not_checked(Reason::PieceNotLocatable, 1);
            }
            let f = Self::finding(
                Rule::MergePiece,
                rec,
                site,
                symbol,
                "a section-symbol reference outside its mergeable section (spec/synth.md S-18 refuses it)",
            );
            return self.fail(f);
        }
        let ty = rec.def.r_type;
        let out = rec.out_word as u32;
        let mut w1 = false;
        let ev = match (rec.def.op, rec.def.field) {
            _ if rec.def.op.is_slot() => {
                return self.not_checked(Reason::PieceNotLocatable, 1);
            }
            (Op::Abs, Field::Data(w)) => Evidence::Exact(trunc(rec.out_word, w)),
            // i386's `GOTOFF` (x86-64's `GOTOFF64`): GOT plus the field,
            // modulo the address size (WP-028e).
            (Op::GotRel, Field::Data(4 | 8)) => {
                self.gotrel_address(rec).map_or(Evidence::None, Evidence::Exact)
            }
            (
                Op::Prel,
                Field::Data(_) | Field::Adr | Field::Imm19 | Field::Imm14 | Field::Imm26,
            ) => Evidence::Exact(
                rec.p.wrapping_add(a64::decode(rec.def.field, rec.out_word, rec.p) as u64),
            ),
            (Op::Page, _) if out == a64::NOP => {
                // W1 with a merged target: the partner's ADR gives the address.
                let add = partner(4, &[ADD_ABS_LO12_NC]);
                match add.and_then(|a| a64::adr(a.out_word as u32, a.p)) {
                    Some((d, t)) if d == rec.input_word as u32 & 31 => {
                        w1 = true;
                        Evidence::Exact(t)
                    }
                    // An ADRP made NOP is W1 only with its ADD made the
                    // ADR of its register; else the register is unset
                    // (WP-028d: was not checked as `piece-not-locatable`).
                    _ => {
                        let f = Self::finding(
                            Rule::Field,
                            rec,
                            site,
                            symbol,
                            "the ADRP is a NOP but the ADD after it is not the ADR of its register (W1)",
                        );
                        return self.fail(f);
                    }
                }
            }
            (Op::Page, _) => page_evidence(rec),
            (Op::Abs, Field::Lo12(_))
                if ty == ADD_ABS_LO12_NC && a64::adr(out, rec.p).is_some() =>
            {
                w1 = true;
                a64::adr(out, rec.p).map_or(Evidence::None, |(_, t)| Evidence::Exact(t))
            }
            (Op::Abs, Field::Lo12(_)) => {
                Evidence::Low(a64::decode(rec.def.field, rec.out_word, rec.p) as u64 & 0xfff)
            }
            _ => Evidence::None,
        };
        let (oi_from, si_from) = (from.0 as u32, from.1 as u32);
        let member = PieceRef { rec: rec.clone(), oi: oi_from, si: si_from, w1, loaded, ev };
        self.piece_groups.entry((oi, sec, r, extra)).or_default().members.push(member);
    }

    /// Records what a GOT slot of `want` holds ([`Self::got_demands`]).
    fn demand(&mut self, want: Want) {
        match want {
            Want::Word(v) => {
                self.got_demands.insert(v);
            }
            Want::Pair(m, v) => {
                self.got_demands.insert(m);
                self.got_demands.insert(v);
            }
        }
    }

    /// A GOT group with no pair (§5.5): its records are not checked, but
    /// a demoted weak mention's record is R5 whatever (§5.7a).
    fn got_unpaired(&mut self, g: &Group) {
        let key = |p: &Pending| (p.rec.p, p.rec.def.r_type);
        let demoted =
            g.members.iter().filter(|m| self.demoted_pending.contains_key(&key(&m.0))).count();
        self.not_checked(Reason::GotUnpaired, (g.members.len() - demoted) as u64);
        for (pend, _) in &g.members {
            if let Some((f, _)) = self.demoted_pending.remove(&key(pend)) {
                self.fail(f);
                continue;
            }
            let form = crate::equiv::Form::NotChecked(Reason::GotUnpaired);
            crate::equiv::reform(self, pend.rec.p, form);
        }
    }

    /// A GOT group's record that reaches no slot holding what it wants
    /// (§5.5): its finding.
    fn got_no_slot(&mut self, pend: &Pending, want: Want, cands: &[u64]) {
        let rec = &pend.rec;
        let mut f = Self::finding(
            Rule::GotSlot,
            rec,
            &pend.site,
            &pend.symbol,
            match want {
                Want::Word(v) if !rec.def.op.is_tls() => {
                    format!("the GOT-indirect reference reaches no slot that holds S+A ({v:#x})")
                }
                Want::Word(v) => format!(
                    "the GOT-indirect reference reaches no entry that holds the thread-pointer offset {v:#x}"
                ),
                Want::Pair(m, v) => format!(
                    "the GOT-indirect reference reaches no entry that holds the module index {m} and the offset {v:#x}"
                ),
            },
        );
        f.expected = Some(want.first());
        f.found = cands.first().and_then(|&c| self.image.value(c, self.word() as u8));
        self.fail(f);
    }

    /// Decides the GOT groups (§5.5).
    pub fn finish_got(&mut self) {
        let groups = std::mem::take(&mut self.got_groups);
        if self.trace.is_some() {
            let wanted =
                groups.values().flat_map(|g| g.members.iter().map(|m| m.0.rec.p)).collect();
            crate::equiv::index_places(self, &wanted);
        }
        for ((_, goi, gsym, _), g) in groups {
            let key = |p: &Pending| (p.rec.p, p.rec.def.r_type);
            if g.unpaired() {
                self.got_unpaired(&g);
                continue;
            }
            let want = g.members[0].0.want;
            let cands = g.candidates_from(self.got);
            let valid: Vec<u64> =
                cands.iter().copied().filter(|&c| self.slot_ok(c, want)).collect();
            let mut held: Option<(u64, crate::equiv::Tally)> = None;
            for (pend, _) in &g.members {
                // §6.9 (P-E4): the outcome of the member before, now known.
                if let Some((p, b)) = held.take() {
                    crate::equiv::retraced(self, p, &b);
                }
                held = self.trace.is_some().then(|| (pend.rec.p, crate::equiv::tally(self)));
                let rec = &pend.rec;
                if valid.is_empty()
                    && let Some((f, _)) = self.demoted_pending.remove(&key(pend))
                {
                    self.fail(f);
                    continue;
                }
                if valid.is_empty() {
                    self.got_no_slot(pend, want, &cands);
                    continue;
                }
                let mut last = None;
                let mut ok = None;
                for &gv in &valid {
                    let Some(x) = x_got(rec.def.op, gv, rec.p, self.got, rec.addend) else {
                        continue;
                    };
                    match judge(rec, x) {
                        Verdict::Ok(rw) => {
                            ok = Some(rw);
                            break;
                        }
                        v => last = Some((v, x)),
                    }
                }
                // §5.7a: the form of a demoted weak mention's R5 (its
                // slot's word is no verified word of view 2).
                if let Some((mut f, clean)) = self.demoted_pending.remove(&key(pend)) {
                    if clean && ok.is_some() {
                        f.message.push_str(DEMOTED_WEAK);
                    }
                    self.fail(f);
                    continue;
                }
                if ok.is_some()
                    && let Want::Word(v) = want
                    && !rec.def.op.is_tls()
                {
                    let w = self.word() as u8;
                    let internal = self.internal_value(v.wrapping_sub(slot_addend(rec)), goi, gsym)
                        && !matches!(self.resolve_any(goi, gsym), Target::UndefWeak)
                        && !self.absolute(goi, gsym);
                    for &gv in &valid {
                        self.abs_words.push((gv, w, v, internal));
                    }
                }
                match ok {
                    Some(Some(rw)) => self.cov.add_accepted(rw, rec.def.r_type),
                    Some(None) => self.cov.add_verified(Via::GotSlot, rec.def.r_type),
                    None => {
                        let f = verdict_finding(rec, &pend.site, &pend.symbol, last);
                        self.fail(f);
                    }
                }
            }
            if let Some((p, b)) = held {
                crate::equiv::retraced(self, p, &b);
            }
        }
        // §5.7a: a record no group decided is R5 as step 0 gives it.
        for (_, (f, _)) in std::mem::take(&mut self.demoted_pending) {
            self.fail(f);
        }
    }

    /// Decides the mergeable-piece groups (§5.6) not decided yet.
    pub fn finish_pieces(&mut self) {
        let groups = std::mem::take(&mut self.piece_groups);
        for (key, g) in groups {
            self.decide_pieces(key, &g);
        }
    }

    /// Decides the groups of object `oi`'s own mergeable sections, which
    /// no later object's record can name (WP-109), in key order.
    fn finish_pieces_of(&mut self, oi: usize) {
        let lo = (oi, 0, 0, i64::MIN);
        let hi = (oi, usize::MAX, u64::MAX, i64::MAX);
        let keys: Vec<_> = self
            .piece_groups
            .range(lo..=hi)
            .map(|(k, _)| *k)
            .filter(|k| !self.shared_merge.contains(&(k.0, k.1)))
            .collect();
        for key in keys {
            if let Some(g) = self.piece_groups.remove(&key) {
                self.decide_pieces(key, &g);
            }
        }
    }

    /// Decides one piece group: the references to offset `r` of mergeable
    /// section `sec` of object `oi` (§5.6).
    fn decide_pieces(&mut self, (oi, sec, r, extra): (usize, usize, u64, i64), g: &PieceGroup) {
        // §5.6 (WP-131 V4): a piece in an SHF_COMPRESSED output section is
        // not judged (no inflate); it is counted as such.
        let target = &self.objs[oi].secs[sec];
        if !target.alloc()
            && self
                .image
                .nonalloc_section(&target.name)
                .is_some_and(|i| self.image.sections[i].shdr.sh_flags & SHF_COMPRESSED != 0)
        {
            return self.not_checked(Reason::Compressed, g.members.len() as u64);
        }
        let cands = candidates_of(g.members.iter().map(|m| &m.ev), None);
        if cands.is_empty() {
            self.not_checked(Reason::PieceNotLocatable, g.members.len() as u64);
            return;
        }
        let valid: Vec<u64> = cands
            .iter()
            .copied()
            .filter(|&v| self.piece_matches(oi, sec, r, v.wrapping_sub(extra as u64)))
            .collect();
        for m in &g.members {
            let rec = &m.rec;
            let at = |e: &Self| {
                let (from, fsi) = (m.oi as usize, m.si as usize);
                (e.objs[from].site(fsi, rec.off), e.sym_name(from, rec.sym))
            };
            if valid.is_empty() {
                let (site, symbol) = at(self);
                let mut f = Self::finding(
                    Rule::MergePiece,
                    rec,
                    &site,
                    &symbol,
                    format!(
                        "the reference to offset {r:#x} of mergeable section '{}' reaches other bytes",
                        self.objs[oi].secs[sec].name
                    ),
                );
                f.found = cands.first().copied();
                self.fail(f);
                continue;
            }
            if m.w1 {
                // The NOP of a NOP+ADR, or the ADR itself: the ADR's
                // target is the piece.
                let ok = rec.out_word as u32 == a64::NOP
                    || a64::adr(rec.out_word as u32, rec.p)
                        .is_some_and(|(_, t)| valid.contains(&t));
                if ok {
                    self.cov.add_accepted(Rewrite::AdrpAddToAdr, rec.def.r_type);
                    continue;
                }
            }
            let mut last = None;
            let mut ok = None;
            for &v in &valid {
                let Some(x) = x_direct(rec.def.op, v, rec.p, self.got) else { continue };
                match judge(rec, x) {
                    Verdict::Ok(rw) => {
                        ok = Some(rw);
                        // Stage 3's view 2: an absolute word naming a
                        // piece moves with the executable.
                        let w = rec.def.field.width();
                        if rec.def.op == Op::Abs
                            && matches!(rec.def.field, Field::Data(4 | 8))
                            && m.loaded
                            && self.image.load_of(rec.p, u64::from(w)).is_some()
                        {
                            self.abs_words.push((rec.p, w, v, true));
                        }
                        break;
                    }
                    vv => last = Some((vv, x)),
                }
            }
            match ok {
                Some(Some(rw)) => self.cov.add_accepted(rw, rec.def.r_type),
                Some(None) => self.cov.add_verified(Via::Piece, rec.def.r_type),
                None => {
                    let (site, symbol) = at(self);
                    let f = verdict_finding(rec, &site, &symbol, last);
                    self.fail(f);
                }
            }
        }
    }

    /// Whether the output at `t` holds the piece of mergeable section
    /// `sec` that input offset `r` is in, from `r` to the piece's end
    /// (§5.6).
    fn piece_matches(&self, oi: usize, sec: usize, r: u64, t: u64) -> bool {
        let read = |t: u64, n: u64| {
            let s = &self.objs[oi].secs[sec];
            if s.alloc() {
                self.image.bytes(t, n)
            } else {
                self.image
                    .nonalloc_section(&s.name)
                    .and_then(|i| self.image.nonalloc_bytes(i, t, n))
            }
        };
        let s = &self.objs[oi].secs[sec];
        let data = self.objs[oi].data(sec);
        let Ok(r) = usize::try_from(r) else { return false };
        if r >= data.len() {
            return false;
        }
        let end = if s.shdr.sh_flags & SHF_STRINGS != 0 {
            let es = usize::try_from(s.shdr.sh_entsize.max(1)).unwrap_or(1);
            // The string ends at the first all-zero character at or after r.
            let start = r - (r % es);
            let mut e = start;
            loop {
                let Some(ch) = data.get(e..e + es) else { return false };
                e += es;
                if ch.iter().all(|&b| b == 0) {
                    break e;
                }
            }
        } else {
            let es = usize::try_from(s.shdr.sh_entsize.max(1)).unwrap_or(1);
            (r - (r % es) + es).min(data.len())
        };
        let want = &data[r..end];
        read(t, want.len() as u64).is_some_and(|b| b == want)
    }
}

/// The output place of input offset `off` of a section placed at
/// `place`: `None` when the offset is in no placed piece (§5.1 step 1).
pub(crate) fn place_at(place: &Place, off: u64) -> Option<u64> {
    match place {
        Place::Whole(a) => Some(a.wrapping_add(off)),
        Place::Pieces(ps) => ps
            .iter()
            .find(|q| q.input_offset <= off && off < q.input_offset + q.size)
            .map(|q| q.addr + (off - q.input_offset)),
        _ => None,
    }
}

/// The address evidence of an ADRP field (§5.5, §5.6): the page it
/// computes, or the page of a W7 ADR.
fn page_evidence(rec: &Rec) -> Evidence {
    let out = rec.out_word as u32;
    if let Some((_, pg)) = a64::adrp(out, rec.p) {
        return Evidence::Page(pg);
    }
    if a64::erratum_843419_adrp(rec.p)
        && let Some((_, t)) = a64::adr(out, rec.p)
        && t.trailing_zeros() >= 12
    {
        return Evidence::Page(t);
    }
    Evidence::None
}

/// The record at offset `off` that `ok` accepts (§5.3: partners are
/// found by offset).
fn find_at<'r>(
    recs: &'r [Rec],
    sorted: &[(u64, usize)],
    off: Option<u64>,
    ok: impl Fn(&Rec) -> bool,
) -> Option<&'r Rec> {
    let off = off?;
    let start = sorted.partition_point(|e| e.0 < off);
    sorted[start..].iter().take_while(|e| e.0 == off).map(|e| &recs[e.1]).find(|r| ok(r))
}

/// The four records of the TLSDESC sequence `rec` is in (§4.7): types
/// 562, 563, 564, 569 at consecutive words, one symbol and addend.
fn desc_seq<'r>(rec: &Rec, recs: &'r [Rec], sorted: &[(u64, usize)]) -> Option<[&'r Rec; 4]> {
    const T: [u32; 4] = [562, 563, 564, 569];
    let k = T.iter().position(|&t| t == rec.def.r_type)?;
    let head = rec.off.checked_sub(4 * k as u64)?;
    let at = |j: usize| {
        find_at(recs, sorted, head.checked_add(4 * j as u64), |r| {
            r.def.r_type == T[j] && r.sym == rec.sym && r.addend == rec.addend
        })
    };
    Some([at(0)?, at(1)?, at(2)?, at(3)?])
}

/// Whether the output instruction is the input's outside the field (the
/// record was relocated, not rewritten), allowing W7's ADR.
fn same_insn(rec: &Rec) -> bool {
    let m = rec.def.field.mask();
    rec.out_word & !m == rec.input_word & !m
        || (rec.def.field == Field::AdrPage
            && a64::erratum_843419_adrp(rec.p)
            && a64::adr(rec.out_word as u32, rec.p).is_some())
}

fn rewritten_badly() -> Outcome {
    Outcome::Fail(
        Rule::Field,
        "the instruction is rewritten into a form no specified rewriting allows".into(),
        None,
    )
}

/// W9 (§5.3): `ADRP Xd, :gottprel:S; LDR Xd, [Xd, :gottprel_lo12:S]`
/// made `MOVZ Xd, #TPREL[31:16], LSL #16; MOVK Xd, #TPREL[15:0]`. One
/// register throughout: with `LDR Xt, [Xd]` and t != d the rewriting does
/// not compute the value in Xt (review finding 1).
fn ie_to_le(rec: &Rec, recs: &[Rec], x: u64) -> bool {
    if x >= 1 << 32 {
        return false;
    }
    let hi = |r: u32| a64::movz(r, x >> 16, 1);
    let lo = |r: u32| a64::movk(r, x & 0xffff, 0);
    let peer = |ty: u32, ok: &dyn Fn(&Rec) -> bool| {
        recs.iter()
            .any(|r| r.sym == rec.sym && r.addend == rec.addend && r.def.r_type == ty && ok(r))
    };
    match rec.def.r_type {
        TLSIE_ADR_GOTTPREL_PAGE21 => {
            let Some((d, _)) = a64::adrp(rec.input_word as u32, 0) else { return false };
            rec.out_word as u32 == hi(d)
                && peer(TLSIE_LD64_GOTTPREL_LO12_NC, &|r| {
                    a64::ldr64_imm(r.input_word as u32).is_some_and(|(t, n, _)| t == d && n == d)
                        && r.out_word as u32 == lo(d)
                })
        }
        TLSIE_LD64_GOTTPREL_LO12_NC => {
            let Some((t, n, _)) = a64::ldr64_imm(rec.input_word as u32) else { return false };
            t == n
                && rec.out_word as u32 == lo(t)
                && peer(TLSIE_ADR_GOTTPREL_PAGE21, &|r| {
                    a64::adrp(r.input_word as u32, 0).is_some_and(|(d, _)| d == t)
                        && r.out_word as u32 == hi(t)
                })
        }
        _ => false,
    }
}

fn trunc(v: u64, w: u8) -> u64 {
    if w >= 8 { v } else { v & ((1u64 << (8 * u32::from(w))) - 1) }
}

/// The rewritings of a GOT-indirect pair (§5.3): W2 and W3.
fn got_rewrite<'r>(
    rec: &Rec,
    sa: u64,
    partner: &dyn Fn(i64, &[u32]) -> Option<&'r Rec>,
) -> Option<Rewrite> {
    let ty = rec.def.r_type;
    let (adrp_rec, ldr_rec) = match ty {
        ADR_GOT_PAGE => (rec, partner(4, &[LD64_GOT_LO12_NC])?),
        LD64_GOT_LO12_NC => (partner(-4, &[ADR_GOT_PAGE])?, rec),
        _ => return None,
    };
    let rd = adrp_rec.input_word as u32 & 31;
    let (t, n, _) = a64::ldr64_imm(ldr_rec.input_word as u32)?;
    if a64::adrp(adrp_rec.input_word as u32, 0).is_none() || n != rd || t != n {
        return None;
    }
    let o1 = adrp_rec.out_word as u32;
    let o2 = ldr_rec.out_word as u32;
    // W2: ADRP Xd, page(S+A); ADD Xd, Xd, #lo12(S+A).
    let add = 0x9100_0000 | ((sa & 0xfff) as u32) << 10 | rd << 5 | rd;
    if a64::adrp(o1, adrp_rec.p) == Some((rd, a64::page(sa))) && o2 == add {
        return Some(Rewrite::GotToAdrpAdd);
    }
    // W3: NOP; ADR Xd, S+A.
    if o1 == a64::NOP && a64::adr(o2, ldr_rec.p) == Some((rd, sa)) {
        return Some(Rewrite::GotToAdr);
    }
    None
}

fn verdict_finding(rec: &Rec, site: &Site, symbol: &str, last: Option<(Verdict, i64)>) -> Finding {
    match last {
        Some((Verdict::Overflow, x)) => Engine::finding(
            Rule::Overflow,
            rec,
            site,
            symbol,
            format!("the value {x:#x} fails the check {}", rec.def.check.describe()),
        ),
        Some((Verdict::Misaligned, x)) => Engine::finding(
            Rule::Misaligned,
            rec,
            site,
            symbol,
            format!("the value {x:#x} has low bits the field drops"),
        ),
        Some((Verdict::Mismatch(e), _)) => {
            let mut f = Engine::finding(
                Rule::Field,
                rec,
                site,
                symbol,
                "the relocated field holds the wrong value",
            );
            f.expected = Some(e);
            f.found = Some(rec.out_word);
            f
        }
        _ => Engine::finding(
            Rule::Field,
            rec,
            site,
            symbol,
            "the relocated field holds the wrong value",
        ),
    }
}

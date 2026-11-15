//! The relocation engine (`spec/plumb.md` §4, §5): for every relocation
//! record of every placed input section, the value the place must hold,
//! recomputed from S, A and P, and compared with the output.
//!
//! One pass over the records. Lookups are by index, by binary search or by
//! hash; groups are kept in ordered maps so nothing depends on hash order.

use std::collections::{BTreeMap, HashMap};

use mortar_elf::consts::{
    SHF_STRINGS, SHT_NOBITS, STB_GLOBAL, STB_GNU_UNIQUE, STB_WEAK, STT_GNU_IFUNC, STT_SECTION,
    STT_TLS,
};
use mortar_elf::{Machine, SymbolIndex, SymbolSection};

use crate::a64::{self, Def, Field, Op};
use crate::image::Image;
use crate::locate::{Obj, Place};
use crate::report::{Coverage, Finding, Reason, Rewrite, Rule, Site, Via};

/// The width of a relocated place in bytes (§4.2).
pub(crate) fn width(machine: Machine, r_type: u32) -> u8 {
    match machine {
        Machine::AArch64 => {
            a64::def(r_type).map_or_else(|| a64::unchecked_width(r_type), |d| d.field.width())
        }
        // Stage 1 checks AArch64 only; the widths of the other two
        // machines arrive with their tables (§4.5).
        Machine::X86_64 | Machine::I386 => 4,
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
                if !s.alloc() {
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
}

/// What a relocation's symbol resolves to (§4.4).
#[derive(Debug, Clone)]
enum Target {
    /// S.
    Addr(u64),
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

/// A record waiting for its group (GOT slot or mergeable piece).
#[derive(Debug, Clone)]
struct Pending {
    rec: Rec,
    site: Site,
    symbol: String,
    /// S+A for a GOT-indirect record.
    value: u64,
    /// The record is half of an ADRP+ADD made NOP+ADR (merge path).
    w1: bool,
}

/// Evidence of a GOT slot's or a piece's address from one record.
#[derive(Debug, Clone, Copy)]
enum Evidence {
    Exact(u64),
    Page(u64),
    Low(u64),
    None,
}

#[derive(Debug, Default)]
struct Group {
    members: Vec<(Pending, Evidence)>,
}

impl Group {
    /// The addresses the members' evidence allows, in order, without
    /// duplicates: every exact value, then every page with every low part.
    fn candidates(&self) -> Vec<u64> {
        let mut v: Vec<u64> = Vec::new();
        let mut pages: Vec<u64> = Vec::new();
        let mut lows: Vec<u64> = Vec::new();
        for (_, e) in &self.members {
            match *e {
                Evidence::Exact(x) => v.push(x),
                Evidence::Page(x) => pages.push(x),
                Evidence::Low(x) => lows.push(x),
                Evidence::None => {}
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

    /// Whether the evidence is one-sided: pages with no low part or the
    /// reverse, and no exact value.
    fn unpaired(&self) -> bool {
        let has = |f: fn(&Evidence) -> bool| self.members.iter().any(|(_, e)| f(e));
        !has(|e| matches!(e, Evidence::Exact(_)))
            && (has(|e| matches!(e, Evidence::Page(_))) != has(|e| matches!(e, Evidence::Low(_))))
    }
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

/// X for a non-GOT operation (§4.1).
fn x_direct(op: Op, sa: u64, p: u64, got: Option<u64>) -> Option<i64> {
    Some(match op {
        Op::Abs => sa as i64,
        Op::Prel => sa.wrapping_sub(p) as i64,
        Op::Page => a64::page(sa).wrapping_sub(a64::page(p)) as i64,
        Op::GotRel => sa.wrapping_sub(got?) as i64,
        Op::None | Op::Got | Op::GotPrel | Op::GotPage | Op::GotOff | Op::GotPageOff => {
            return None;
        }
    })
}

/// X for a GOT operation given the slot G (§4.1).
fn x_got(op: Op, g: u64, p: u64, got: Option<u64>) -> Option<i64> {
    Some(match op {
        Op::Got => g as i64,
        Op::GotPrel => g.wrapping_sub(p) as i64,
        Op::GotPage => a64::page(g).wrapping_sub(a64::page(p)) as i64,
        Op::GotOff => g.wrapping_sub(got?) as i64,
        Op::GotPageOff => g.wrapping_sub(a64::page(got?)) as i64,
        _ => return None,
    })
}

const ADR_PREL_PG_HI21: u32 = 275;
const ADR_PREL_PG_HI21_NC: u32 = 276;
const ADD_ABS_LO12_NC: u32 = 277;
const JUMP26: u32 = 282;
const CALL26: u32 = 283;
const ADR_GOT_PAGE: u32 = 311;
const LD64_GOT_LO12_NC: u32 = 312;

/// The engine's state over one verification.
pub(crate) struct Engine<'a, 'b> {
    pub(crate) image: &'b Image<'a>,
    pub(crate) objs: &'b [Obj<'a>],
    occupied: Occupied,
    /// Global and weak definitions of the inputs by name.
    pub(crate) defs: HashMap<String, Vec<(usize, u32)>>,
    got: Option<u64>,
    /// The output's `.got`, `[start, end)`: a GOT slot lies inside it when
    /// the output has one (§5.5).
    got_section: Option<(u64, u64)>,
    cache: Vec<Vec<Option<Target>>>,
    got_groups: BTreeMap<(usize, u32, i64), Group>,
    piece_groups: BTreeMap<(usize, usize, u64, i64), Group>,
    pub cov: Coverage,
    pub findings: Vec<Finding>,
}

impl<'a, 'b> Engine<'a, 'b> {
    pub fn new(image: &'b Image<'a>, objs: &'b [Obj<'a>]) -> Self {
        let mut defs: HashMap<String, Vec<(usize, u32)>> = HashMap::new();
        for (oi, o) in objs.iter().enumerate() {
            let Some(t) = &o.symtab else { continue };
            for (i, sym) in t.iter() {
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
            .or_else(|| image.sections.iter().find(|s| s.name == ".got").map(|s| s.shdr.sh_addr));
        let got_section = image
            .sections
            .iter()
            .find(|s| s.name == ".got")
            .map(|s| (s.shdr.sh_addr, s.shdr.sh_addr.saturating_add(s.shdr.sh_size)));
        let cache =
            objs.iter().map(|o| vec![None; o.symtab.map_or(0, |t| t.len() as usize)]).collect();
        Engine {
            image,
            objs,
            occupied: Occupied::build(objs),
            defs,
            got,
            got_section,
            cache,
            got_groups: BTreeMap::new(),
            piece_groups: BTreeMap::new(),
            cov: Coverage::default(),
            findings: Vec::new(),
        }
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
        match &s.place {
            Place::Whole(a) => Target::Addr(a.wrapping_add(value)),
            Place::Pieces(ps) => ps
                .iter()
                .find(|p| p.input_offset <= value && value <= p.input_offset + p.size)
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
            Place::Merged => Target::Merge { oi, sec, value, section_sym },
            Place::Rewritten => Target::Skip(Reason::EhFrame),
            Place::Ambiguous => Target::Skip(Reason::SectionNotLocatable),
            Place::Absent if s.is_eh_frame(o.elf.machine()) => Target::Skip(Reason::EhFrame),
            Place::Absent | Place::Discarded => Target::Discarded(s.name.clone()),
        }
    }

    fn resolve(&mut self, oi: usize, idx: u32) -> Target {
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

    fn resolve_uncached(&self, oi: usize, idx: u32) -> Target {
        let o = &self.objs[oi];
        let Some(t) = o.symtab else { return Target::Skip(Reason::SymbolUnresolved) };
        if idx == 0 {
            return Target::Addr(0);
        }
        let Ok(sym) = t.get(SymbolIndex(idx)) else {
            return Target::Skip(Reason::SymbolUnresolved);
        };
        if sym.sym_type() == STT_GNU_IFUNC {
            return Target::Skip(Reason::Ifunc);
        }
        if sym.sym_type() == STT_TLS {
            return Target::Skip(Reason::Tls);
        }
        let where_ = t.section_of(SymbolIndex(idx), &sym);
        if !matches!(sym.binding(), STB_GLOBAL | STB_WEAK | STB_GNU_UNIQUE) {
            return match where_ {
                Ok(SymbolSection::Section(k)) => {
                    self.in_section(oi, k.0 as usize, sym.st_value, sym.sym_type() == STT_SECTION)
                }
                Ok(SymbolSection::Absolute) => Target::Addr(sym.st_value),
                _ => Target::Skip(Reason::SymbolUnresolved),
            };
        }
        let name = String::from_utf8_lossy(t.name(&sym).unwrap_or(b"")).into_owned();
        // 1. The output's global symbol of the name.
        if let Some(out) = self.image.global(&name) {
            if out.sym.sym_type() == STT_GNU_IFUNC {
                return Target::Skip(Reason::Ifunc);
            }
            if out.sym.sym_type() == STT_TLS {
                return Target::Skip(Reason::Tls);
            }
            if out.sym.st_shndx != 0 {
                return Target::Addr(out.sym.st_value);
            }
            return if out.sym.binding() == STB_WEAK || sym.binding() == STB_WEAK {
                Target::UndefWeak
            } else {
                Target::Skip(Reason::SymbolUnresolved)
            };
        }
        // 2. The one placed strong definition, else the one placed weak one.
        if let Some(defs) = self.defs.get(&name) {
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
            let chosen = match (strong.as_slice(), placed.as_slice()) {
                ([one], _) => Some(**one),
                ([], [one]) => Some(*one),
                _ => None,
            };
            if let Some((doi, di, _)) = chosen {
                let dt = self.objs[doi].symtab.expect("filtered above");
                if let Ok(ds) = dt.get(SymbolIndex(di)) {
                    if ds.sym_type() == STT_GNU_IFUNC {
                        return Target::Skip(Reason::Ifunc);
                    }
                    return match dt.section_of(SymbolIndex(di), &ds) {
                        Ok(SymbolSection::Section(k)) => {
                            self.in_section(doi, k.0 as usize, ds.st_value, false)
                        }
                        Ok(SymbolSection::Absolute) => Target::Addr(ds.st_value),
                        _ => Target::Skip(Reason::SymbolUnresolved),
                    };
                }
            }
        }
        // 3. The one output symbol of the name, of any binding.
        if let Some(out) = self.image.unique(&name) {
            if out.sym.sym_type() == STT_GNU_IFUNC {
                return Target::Skip(Reason::Ifunc);
            }
            if out.sym.st_shndx != 0 {
                return Target::Addr(out.sym.st_value);
            }
        }
        if matches!(where_, Ok(SymbolSection::Undefined)) && sym.binding() == STB_WEAK {
            return Target::UndefWeak;
        }
        Target::Skip(Reason::SymbolUnresolved)
    }

    fn not_checked(&mut self, r: Reason, n: u64) {
        self.cov.add_not_checked(r, n);
    }

    fn fail(&mut self, f: Finding) {
        self.cov.failed += 1;
        self.findings.push(f);
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
    fn out_word(&self, p: u64, def: &Def, input_word: u64) -> Option<(u64, bool)> {
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
        let objs = self.objs;
        let o = &objs[oi];
        let s = &o.secs[si];
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
            Place::Absent | Place::Discarded if !(s.alloc() && s.is_eh_frame(machine)) => {
                self.cov.absent_records += n;
                return;
            }
            _ => {}
        }
        self.cov.records += n;
        if !s.alloc() {
            return self.not_checked(Reason::NonAlloc, n);
        }
        let reason = match &s.place {
            Place::Ambiguous => Some(Reason::SectionNotLocatable),
            Place::Merged => Some(Reason::InMergeable),
            Place::Rewritten | Place::Absent | Place::Discarded => Some(Reason::EhFrame),
            Place::Whole(_) | Place::Pieces(_) => None,
        };
        if let Some(r) = reason {
            return self.not_checked(r, n);
        }
        if machine != Machine::AArch64 {
            return self.not_checked(Reason::Machine, n);
        }
        let place = s.place.clone();
        let data = s.data;
        let nobits = s.shdr.sh_type == SHT_NOBITS;
        // Records with their places; the pieces of a dropped part are not
        // in the output.
        let mut recs: Vec<Rec> = Vec::with_capacity(raw.len());
        for (off, sym, ty, addend) in raw {
            let p = match &place {
                Place::Whole(a) => Some(a.wrapping_add(off)),
                Place::Pieces(ps) => ps
                    .iter()
                    .find(|q| q.input_offset <= off && off < q.input_offset + q.size)
                    .map(|q| q.addr + (off - q.input_offset)),
                _ => None,
            };
            let Some(p) = p else {
                self.cov.records -= 1;
                self.cov.absent_records += 1;
                continue;
            };
            if ty == 0 || ty == 256 {
                self.cov.none += 1;
                continue;
            }
            if a64::is_tls(ty) {
                self.not_checked(Reason::Tls, 1);
                continue;
            }
            let Some(def) = a64::def(ty) else {
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
            let addend = addend.unwrap_or_else(|| a64::decode(def.field, input_word, 0));
            let Some((out_word, patched)) = self.out_word(p, def, input_word) else {
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
            self.record(oi, si, &recs, &sorted, i);
        }
    }

    #[allow(clippy::too_many_lines)] // §5.1 steps 2 to 7, in order
    fn record(&mut self, oi: usize, si: usize, recs: &[Rec], sorted: &[(u64, usize)], i: usize) {
        let rec = &recs[i];
        let site = self.objs[oi].site(si, rec.off);
        let target = self.resolve(oi, rec.sym);
        let symbol = self.sym_name(oi, rec.sym);
        let partner = |delta: i64, types: &[u32]| -> Option<&Rec> {
            let want = rec.off.checked_add_signed(delta)?;
            let start = sorted.partition_point(|e| e.0 < want);
            sorted[start..].iter().take_while(|e| e.0 == want).map(|e| &recs[e.1]).find(|r| {
                r.sym == rec.sym && r.addend == rec.addend && types.contains(&r.def.r_type)
            })
        };
        let sa = match target {
            Target::Skip(r) => return self.not_checked(r, 1),
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
            Target::Merge { oi: doi, sec, value, section_sym } => {
                return self.merge_record(
                    doi,
                    sec,
                    value,
                    section_sym,
                    rec,
                    site,
                    symbol,
                    &partner,
                );
            }
            Target::Addr(s) => s.wrapping_add(rec.addend as u64),
            Target::UndefWeak => rec.addend as u64,
        };
        let weak = matches!(target, Target::UndefWeak);
        let op = rec.def.op;
        if matches!(op, Op::Got | Op::GotPrel | Op::GotPage | Op::GotOff | Op::GotPageOff) {
            if let Some(rw) = got_rewrite(rec, sa, &partner) {
                self.cov.add_accepted(rw, rec.def.r_type);
                return;
            }
            let ev = match op {
                Op::GotPrel => Evidence::Exact(rec.p.wrapping_add(a64::decode(
                    rec.def.field,
                    rec.out_word,
                    rec.p,
                ) as u64)),
                Op::GotOff | Op::GotPageOff => match self.got {
                    Some(g) => {
                        let base = if op == Op::GotOff { g } else { a64::page(g) };
                        Evidence::Exact(base.wrapping_add(a64::decode(
                            rec.def.field,
                            rec.out_word,
                            rec.p,
                        ) as u64))
                    }
                    None => return self.not_checked(Reason::SymbolUnresolved, 1),
                },
                Op::GotPage => page_evidence(rec),
                _ => Evidence::Low(a64::decode(rec.def.field, rec.out_word, rec.p) as u64 & 0xfff),
            };
            let pend = Pending { rec: rec.clone(), site, symbol, value: sa, w1: false };
            self.got_groups.entry((oi, rec.sym, rec.addend)).or_default().members.push((pend, ev));
            return;
        }
        let Some(x) = x_direct(op, sa, rec.p, self.got) else {
            return self.not_checked(Reason::SymbolUnresolved, 1);
        };
        let verdict = judge(rec, x);
        if let Verdict::Ok(rw) = verdict {
            match rw {
                Some(r) => self.cov.add_accepted(r, rec.def.r_type),
                None => self.cov.add_verified(Via::Direct, rec.def.r_type),
            }
            return;
        }
        if let Some(rw) = self.direct_rewrite(rec, sa, weak, &partner) {
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
                let mut f = Self::finding(
                    Rule::Field,
                    rec,
                    &site,
                    &symbol,
                    "the relocated field holds the wrong value",
                );
                f.expected = Some(e);
                f.found = Some(rec.out_word);
                f
            }
            Verdict::Ok(_) => unreachable!("handled above"),
        };
        self.fail(f);
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
            if !self.occupied.contains(t)
                && self.image.load_of(t, 4).is_some()
                && a64::run_veneer(self.image, t) == Some(sa)
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
        site: Site,
        symbol: String,
        partner: &dyn Fn(i64, &[u32]) -> Option<&'r Rec>,
    ) {
        let (r, extra) = if section_sym {
            (value.wrapping_add(rec.addend as u64), 0)
        } else {
            (value, rec.addend)
        };
        let ty = rec.def.r_type;
        let out = rec.out_word as u32;
        let mut w1 = false;
        let ev = match (rec.def.op, rec.def.field) {
            _ if matches!(
                rec.def.op,
                Op::Got | Op::GotPrel | Op::GotPage | Op::GotOff | Op::GotPageOff
            ) =>
            {
                return self.not_checked(Reason::PieceNotLocatable, 1);
            }
            (Op::Abs, Field::Data(w)) => Evidence::Exact(trunc(rec.out_word, w)),
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
                    _ => Evidence::None,
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
        let pend = Pending { rec: rec.clone(), site, symbol, value: 0, w1 };
        self.piece_groups.entry((oi, sec, r, extra)).or_default().members.push((pend, ev));
    }

    /// Decides the GOT groups (§5.5).
    pub fn finish_got(&mut self) {
        let groups = std::mem::take(&mut self.got_groups);
        for (_, g) in groups {
            if g.unpaired() {
                self.not_checked(Reason::GotUnpaired, g.members.len() as u64);
                continue;
            }
            let value = g.members[0].0.value;
            let cands = g.candidates();
            let valid: Vec<u64> = cands
                .iter()
                .copied()
                .filter(|&c| {
                    c % 8 == 0
                        && !self.occupied.contains(c)
                        && self
                            .got_section
                            .is_none_or(|(lo, hi)| lo <= c && c.saturating_add(8) <= hi)
                        && self.image.value(c, 8) == Some(value)
                })
                .collect();
            for (pend, _) in &g.members {
                let rec = &pend.rec;
                if valid.is_empty() {
                    let mut f = Self::finding(
                        Rule::GotSlot,
                        rec,
                        &pend.site,
                        &pend.symbol,
                        "the GOT-indirect reference reaches no slot that holds S+A",
                    );
                    f.expected = Some(value);
                    f.found = cands.first().and_then(|&c| self.image.value(c, 8));
                    self.fail(f);
                    continue;
                }
                let mut last = None;
                let mut ok = None;
                for &gv in &valid {
                    let Some(x) = x_got(rec.def.op, gv, rec.p, self.got) else { continue };
                    match judge(rec, x) {
                        Verdict::Ok(rw) => {
                            ok = Some(rw);
                            break;
                        }
                        v => last = Some((v, x)),
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
        }
    }

    /// Decides the mergeable-piece groups (§5.6).
    pub fn finish_pieces(&mut self) {
        let groups = std::mem::take(&mut self.piece_groups);
        for ((oi, sec, r, extra), g) in groups {
            let cands = g.candidates();
            if cands.is_empty() {
                self.not_checked(Reason::PieceNotLocatable, g.members.len() as u64);
                continue;
            }
            let valid: Vec<u64> = cands
                .iter()
                .copied()
                .filter(|&v| self.piece_matches(oi, sec, r, v.wrapping_sub(extra as u64)))
                .collect();
            for (pend, _) in &g.members {
                let rec = &pend.rec;
                if valid.is_empty() {
                    let mut f = Self::finding(
                        Rule::MergePiece,
                        rec,
                        &pend.site,
                        &pend.symbol,
                        format!(
                            "the reference to offset {r:#x} of mergeable section '{}' reaches other bytes",
                            self.objs[oi].secs[sec].name
                        ),
                    );
                    f.found = cands.first().copied();
                    self.fail(f);
                    continue;
                }
                if pend.w1 {
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
                            break;
                        }
                        vv => last = Some((vv, x)),
                    }
                }
                match ok {
                    Some(Some(rw)) => self.cov.add_accepted(rw, rec.def.r_type),
                    Some(None) => self.cov.add_verified(Via::Piece, rec.def.r_type),
                    None => {
                        let f = verdict_finding(rec, &pend.site, &pend.symbol, last);
                        self.fail(f);
                    }
                }
            }
        }
    }

    /// Whether the output at `t` holds the piece of mergeable section
    /// `sec` that input offset `r` is in, from `r` to the piece's end
    /// (§5.6).
    fn piece_matches(&self, oi: usize, sec: usize, r: u64, t: u64) -> bool {
        let s = &self.objs[oi].secs[sec];
        let Ok(r) = usize::try_from(r) else { return false };
        if r >= s.data.len() {
            return false;
        }
        let end = if s.shdr.sh_flags & SHF_STRINGS != 0 {
            let es = usize::try_from(s.shdr.sh_entsize.max(1)).unwrap_or(1);
            // The string ends at the first all-zero character at or after r.
            let start = r - (r % es);
            let mut e = start;
            loop {
                let Some(ch) = s.data.get(e..e + es) else { return false };
                e += es;
                if ch.iter().all(|&b| b == 0) {
                    break e;
                }
            }
        } else {
            let es = usize::try_from(s.shdr.sh_entsize.max(1)).unwrap_or(1);
            (r - (r % es) + es).min(s.data.len())
        };
        let want = &s.data[r..end];
        self.image.bytes(t, want.len() as u64).is_some_and(|b| b == want)
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

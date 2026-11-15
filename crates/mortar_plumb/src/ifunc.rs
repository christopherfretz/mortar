//! IFUNC in a static output (`spec/plumb.md` §4.8): the `IRELATIVE`
//! records (F1, F2), the IPLT entries that load their slots, and the
//! references to an IFUNC (F3, F4), whose S is an IPLT entry of its
//! resolver.
//!
//! The tables are sorted vectors and ordered maps: nothing depends on hash
//! order (§7.1).

use std::collections::{BTreeMap, BTreeSet};

use mortar_elf::consts::{
    EM_386, EM_AARCH64, EM_X86_64, ET_EXEC, PF_W, PF_X, PT_DYNAMIC, PT_INTERP, SHF_EXECINSTR,
    SHT_NOBITS, SHT_REL, SHT_RELA, STT_GNU_IFUNC,
};
use mortar_elf::{Class, SymbolIndex, SymbolSection};

use super::{
    Engine, Evidence, Group, Pending, Rec, Target, Verdict, Want, got_rewrite, is_branch, judge,
    slot_addend, verdict_finding, x_direct, x_got,
};
use crate::a64::{self, Op};
use crate::map::Format;
use crate::report::{Finding, Reason, Rewrite, Rule, Site, Via};

/// An IFUNC symbol: pointer equality holds per symbol (two aliases of one
/// resolver have an IPLT entry each, §4.8).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum IfKey {
    Global(String),
    /// A local symbol: object and symbol index.
    Local(usize, u32),
}

/// The output's `IRELATIVE` records and IPLT entries.
#[derive(Debug, Default)]
pub(crate) struct Iplt {
    /// `(slot, resolver)`, sorted.
    irel: Vec<(u64, u64)>,
    /// The IPLT entries of each resolver, sorted: an entry loads an
    /// `IRELATIVE` slot of the resolver and branches to what it holds.
    by_resolver: BTreeMap<u64, Vec<u64>>,
}

impl Iplt {
    /// The IPLT entries of `resolver`.
    pub fn entries_of(&self, resolver: u64) -> &[u64] {
        self.by_resolver.get(&resolver).map_or(&[], Vec::as_slice)
    }

    /// Whether `slot` is the slot of an `IRELATIVE` record of `resolver`.
    fn is_irel_slot_of(&self, slot: u64, resolver: u64) -> bool {
        let i = self.irel.partition_point(|r| r.0 < slot);
        self.irel[i..].iter().take_while(|r| r.0 == slot).any(|r| r.1 == resolver)
    }

    fn resolver_of(&self, slot: u64) -> Option<u64> {
        let i = self.irel.partition_point(|r| r.0 < slot);
        self.irel.get(i).filter(|r| r.0 == slot).map(|r| r.1)
    }
}

/// The entries an address reference accepts, each with its rewriting.
type Accepts = Vec<(u64, Option<Rewrite>)>;

/// A GOT member: the entries a slot it reaches holds, whether one is an
/// IRELATIVE slot of the resolver, and its verdict.
type GotMember = (Pending, Vec<u64>, bool, Option<(Verdict, i64)>);

/// The references to one IFUNC symbol, decided together (§4.8).
#[derive(Debug)]
pub(crate) struct IfGroup {
    resolver: u64,
    /// Address references (and W2/W3 GOT pairs): the entries each accepts.
    addrs: Vec<(Pending, Accepts)>,
    /// GOT-indirect references by object, symbol and addend (§5.5).
    gots: BTreeMap<(usize, u32, i64), Group>,
}

/// One relocation record of an allocated relocation section of the output.
struct Row {
    /// The record's own address.
    at: u64,
    offset: u64,
    sym: u32,
    ty: u32,
    addend: Option<i64>,
}

/// `BR Xn`: the register.
fn br(insn: u32) -> Option<u32> {
    (insn & 0xffff_fc1f == 0xd61f_0000).then_some((insn >> 5) & 31)
}

fn f(n: u8, msg: impl Into<String>) -> Finding {
    Finding::new(Rule::Ifunc(n), msg)
}

impl Engine<'_, '_> {
    /// Word size of the output's class.
    /// The address a `GotRel` field gives: GOT plus the field, modulo
    /// the address size (spec/x86.md W5's GOTOFF).
    pub(super) fn gotrel_address(&self, rec: &Rec) -> Option<u64> {
        if rec.def.op != Op::GotRel {
            return None;
        }
        let d = a64::decode(rec.def.field, rec.out_word, rec.p) as u64;
        let mask = if self.word() == 4 { u64::from(u32::MAX) } else { u64::MAX };
        Some(self.got?.wrapping_add(d) & mask)
    }

    pub(super) fn word(&self) -> u64 {
        if self.image.class() == Class::Elf64 { 8 } else { 4 }
    }

    /// The target of IFUNC definition `di` of object `doi` (§4.8).
    pub(super) fn ifunc_target(&self, doi: usize, di: u32, key: IfKey) -> Target {
        let Some(t) = self.objs[doi].symtab else { return Target::Skip(Reason::SymbolUnresolved) };
        let Ok(d) = t.get(SymbolIndex(di)) else { return Target::Skip(Reason::SymbolUnresolved) };
        let r = match t.section_of(SymbolIndex(di), &d) {
            Ok(SymbolSection::Section(k)) => self.in_section(doi, k.0 as usize, d.st_value, false),
            Ok(SymbolSection::Absolute) => Target::Addr(d.st_value),
            _ => return Target::Skip(Reason::SymbolUnresolved),
        };
        match r {
            Target::Addr(resolver) => Target::Ifunc { resolver, key },
            t => t,
        }
    }

    /// The records of the output's allocated `SHT_RELA`/`SHT_REL`
    /// sections, with the index of their section.
    fn output_rows(&self) -> Vec<Row> {
        let elf64 = self.image.class() == Class::Elf64;
        let mut rows = Vec::new();
        for s in &self.image.sections {
            let rela = s.shdr.sh_type == SHT_RELA;
            if !s.alloc() || !(rela || s.shdr.sh_type == SHT_REL) {
                continue;
            }
            let size: u64 = match (elf64, rela) {
                (true, true) => 24,
                (true, false) => 16,
                (false, true) => 12,
                (false, false) => 8,
            };
            let Some(bytes) = usize::try_from(s.shdr.sh_offset)
                .ok()
                .zip(usize::try_from(s.shdr.sh_size).ok())
                .and_then(|(o, n)| self.image.data.get(o..o.checked_add(n)?))
            else {
                continue;
            };
            let rd = |b: &[u8], o: usize, w: usize| {
                b[o..o + w].iter().rev().fold(0u64, |a, &x| a << 8 | u64::from(x))
            };
            for (i, e) in bytes.chunks_exact(size as usize).enumerate() {
                let at = s.shdr.sh_addr.wrapping_add(i as u64 * size);
                let row = if elf64 {
                    let info = rd(e, 8, 8);
                    Row {
                        at,
                        offset: rd(e, 0, 8),
                        sym: (info >> 32) as u32,
                        ty: info as u32,
                        addend: rela.then(|| rd(e, 16, 8) as i64),
                    }
                } else {
                    let info = rd(e, 4, 4);
                    Row {
                        at,
                        offset: rd(e, 0, 4),
                        sym: (info >> 8) as u32,
                        ty: (info & 0xff) as u32,
                        addend: rela.then(|| i64::from(rd(e, 8, 4) as u32 as i32)),
                    }
                };
                rows.push(row);
            }
        }
        rows
    }

    /// F1 and F2, and the IPLT entries (§4.8).
    #[allow(clippy::too_many_lines)]
    pub(super) fn build_iplt(&mut self) {
        let image = self.image;
        let irelative = match image.header.e_machine {
            EM_AARCH64 => 1032,
            EM_X86_64 => 37,
            EM_386 => 42,
            _ => return,
        };
        let word = self.word();
        let static_exec = image.header.e_type == ET_EXEC
            && !image.phdrs.iter().any(|p| matches!(p.p_type, PT_DYNAMIC | PT_INTERP));
        let rows = self.output_rows();
        let mut fs = Vec::new();
        // The resolvers: the places of the inputs' IFUNC definitions.
        let mut resolvers = BTreeSet::new();
        for (oi, o) in self.objs.iter().enumerate() {
            let Some(t) = o.symtab else { continue };
            for (i, d) in t.iter() {
                if d.sym_type() == STT_GNU_IFUNC
                    && let Target::Ifunc { resolver, .. } =
                        self.ifunc_target(oi, i.0, IfKey::Local(oi, i.0))
                {
                    resolvers.insert(resolver);
                }
            }
        }
        let mut irel = Vec::new();
        for r in &rows {
            if r.ty != irelative {
                if static_exec {
                    fs.push(
                        f(1, format!(
                            "a relocation of type {} in a static executable: only IRELATIVE is applied",
                            r.ty
                        ))
                        .at(r.at),
                    );
                }
                continue;
            }
            if r.sym != 0 {
                fs.push(f(1, "an IRELATIVE record names a symbol").at(r.at));
            }
            let resolver = match r.addend {
                Some(a) => a as u64,
                // REL: the slot holds the resolver (i386 psABI).
                None => image.value(r.offset, word as u8).unwrap_or(0),
            };
            irel.push((r.offset, resolver));
            self.cov.irelative_checked += 1;
            let slot = r.offset;
            let writable = image.load_of(slot, word).is_some_and(|p| p.p_flags & PF_W != 0);
            if slot % word != 0 || !writable || self.occupied_at(slot) {
                fs.push(
                    f(
                        2,
                        "the IRELATIVE slot is not an aligned word of writable memory outside the placed sections",
                    )
                    .at(slot),
                );
            }
            if !resolvers.contains(&resolver)
                || image.load_of(resolver, 4).is_none_or(|p| p.p_flags & PF_X == 0)
            {
                let mut x = f(
                    2,
                    "the IRELATIVE resolver is not the place of an IFUNC definition of the inputs",
                )
                .at(slot);
                x.found = Some(resolver);
                fs.push(x);
            }
        }
        irel.sort_unstable();
        for w in irel.windows(2) {
            if w[0].0 == w[1].0 {
                fs.push(f(2, "two IRELATIVE records have one slot").at(w[0].0));
            }
        }
        // The C runtime applies the records from __rela_iplt_start to
        // __rela_iplt_end (__rel_iplt_* with REL).
        if static_exec {
            let rel = rows.first().is_some_and(|r| r.addend.is_none());
            let (s, e) = if rel {
                ("__rel_iplt_start", "__rel_iplt_end")
            } else {
                ("__rela_iplt_start", "__rela_iplt_end")
            };
            let val = |n: &str| {
                image
                    .global(n)
                    .or_else(|| image.unique(n))
                    .filter(|o| o.sym.st_shndx != 0)
                    .map(|o| o.sym.st_value)
            };
            if let (Some(lo), Some(hi)) = (val(s), val(e)) {
                let size = match (word, rel) {
                    (8, false) => 24,
                    (8, true) => 16,
                    (_, false) => 12,
                    _ => 8,
                };
                let inside = rows.iter().filter(|r| lo <= r.at && r.at + size <= hi).count() as u64;
                if hi < lo || inside * size != hi - lo {
                    fs.push(
                        f(1, format!("{s}..{e} is not a run of the output's relocation records"))
                            .at(lo),
                    );
                }
                for r in rows.iter().filter(|r| r.ty == irelative) {
                    if !(lo <= r.at && r.at + size <= hi) {
                        fs.push(
                            f(
                                1,
                                format!("an IRELATIVE record outside {s}..{e}: the C runtime does not apply it"),
                            )
                            .at(r.at),
                        );
                    }
                }
            }
        }
        self.iplt.irel = irel;
        // The IPLT entries: linker-made code (outside the placed sections)
        // that loads an IRELATIVE slot and branches to what it holds.
        let mut entries: Vec<(u64, u64)> = Vec::new();
        for s in &image.sections {
            if !s.alloc() || s.shdr.sh_flags & SHF_EXECINSTR == 0 || s.shdr.sh_type == SHT_NOBITS {
                continue;
            }
            // Only bytes the file holds: a hostile `sh_size` reads nothing.
            if image.bytes(s.shdr.sh_addr, s.shdr.sh_size).is_none() {
                continue;
            }
            let (lo, hi) = (s.shdr.sh_addr, s.shdr.sh_addr.saturating_add(s.shdr.sh_size));
            for (g0, g1) in self.occupied.gaps(lo, hi) {
                let Some(b) = image.bytes(g0, g1 - g0) else { continue };
                match image.header.e_machine {
                    EM_AARCH64 => {
                        let mut a = g0.checked_next_multiple_of(4).unwrap_or(g1);
                        while a.checked_add(12).is_some_and(|e| e <= g1) {
                            // ADRP is the first word, after an optional
                            // `BTI c` (GNU ld's and lld's entries in a BTI
                            // output, spec/a64.md §10.3): the entry starts
                            // at the BTI, which is where a branch to it
                            // lands.
                            let k = (a - g0) as usize;
                            if b[k + 3] & 0x9f == 0x90
                                && let Some(slot) = self.a64_entry(a)
                                && self.iplt.resolver_of(slot).is_some()
                            {
                                let bti = k >= 4
                                    && a64::bti(u32::from_le_bytes([
                                        b[k - 4],
                                        b[k - 3],
                                        b[k - 2],
                                        b[k - 1],
                                    ]));
                                entries.push((if bti { a - 4 } else { a }, slot));
                            }
                            a += 4;
                        }
                    }
                    _ => {
                        for k in 0..b.len().saturating_sub(5) {
                            if b[k] != 0xff || b[k + 1] != 0x25 {
                                continue;
                            }
                            let d = u32::from_le_bytes([b[k + 2], b[k + 3], b[k + 4], b[k + 5]]);
                            let at = g0 + k as u64;
                            let slot = if word == 8 {
                                (at + 6).wrapping_add(i64::from(d as i32) as u64)
                            } else {
                                u64::from(d)
                            };
                            if self.iplt.resolver_of(slot).is_none() {
                                continue;
                            }
                            // The entry starts at an ENDBR64/ENDBR32 and a
                            // BND prefix before the jump, when present.
                            let mut start = k;
                            if start >= 1 && b[start - 1] == 0xf2 {
                                start -= 1;
                            }
                            if start >= 4
                                && b[start - 4..start - 1] == [0xf3, 0x0f, 0x1e]
                                && matches!(b[start - 1], 0xfa | 0xfb)
                            {
                                start -= 4;
                            }
                            entries.push((g0 + start as u64, slot));
                        }
                    }
                }
            }
        }
        entries.sort_unstable();
        entries.dedup();
        self.cov.iplt_entries = entries.len() as u64;
        if image.header.e_machine == EM_X86_64 && word == 8 {
            self.x64_iplt(&entries, static_exec, &mut fs);
        } else if image.header.e_machine == EM_386 {
            self.x86_iplt(&entries, &mut fs);
        }
        for (e, slot) in entries {
            let i = self.iplt.irel.partition_point(|r| r.0 < slot);
            let rs: Vec<u64> =
                self.iplt.irel[i..].iter().take_while(|r| r.0 == slot).map(|r| r.1).collect();
            for r in rs {
                self.iplt.by_resolver.entry(r).or_default().push(e);
            }
        }
        for v in self.iplt.by_resolver.values_mut() {
            v.sort_unstable();
            v.dedup();
        }
        self.findings.extend(fs);
    }

    /// F5, x86-64 (spec/x64.md X-37 to X-39): an IBT output's entries
    /// start with `endbr64`; mortar's entries are exactly X-37 (X-38 when
    /// IBT); a static executable's IFUNC words hold, before start-up
    /// applies the `IRELATIVE` records, what the linker that wrote it
    /// writes (mortar and lld 0, mold the resolver, GNU ld the entry + 6,
    /// or the entry itself when it starts with `endbr64`, and 0 for a word
    /// no entry loads).
    fn x64_iplt(&self, entries: &[(u64, u64)], static_exec: bool, fs: &mut Vec<Finding>) {
        use crate::x86op::ENDBR64;
        let image = self.image;
        let ibt = crate::property::x86_feature_1_and(image) & 1 != 0;
        let endbr = |e: u64| image.bytes(e, 4) == Some(&ENDBR64[..]);
        for &(e, slot) in entries {
            if ibt && !endbr(e) {
                fs.push(
                    f(
                        5,
                        "an IPLT entry of an IBT output does not start with ENDBR64: an indirect call to it faults (spec/x64.md X-38)",
                    )
                    .at(e),
                );
            }
            if self.format == Format::Mortar {
                let mut want = Vec::with_capacity(16);
                let jmp = if ibt {
                    want.extend_from_slice(&ENDBR64);
                    e + 4
                } else {
                    e
                };
                let rel = slot.wrapping_sub(jmp + 6) as u32;
                want.extend_from_slice(&[0xff, 0x25]);
                want.extend_from_slice(&rel.to_le_bytes());
                want.extend_from_slice(if ibt {
                    &[0x66, 0x0f, 0x1f, 0x44, 0x00, 0x00][..]
                } else {
                    &[0x66, 0x90][..]
                });
                if image.bytes(e, want.len() as u64) != Some(&want[..]) {
                    fs.push(
                        f(
                            5,
                            if ibt {
                                "the IPLT entry is not endbr64; jmp *word(%rip); nopw (spec/x64.md X-38)"
                            } else {
                                "the IPLT entry is not jmp *word(%rip); xchg %ax,%ax (spec/x64.md X-37)"
                            },
                        )
                        .at(e),
                    );
                }
            }
        }
        if !static_exec {
            return;
        }
        for &(slot, resolver) in &self.iplt.irel {
            let Some(w) = image.value(slot, 8) else { continue };
            let mut es = entries.iter().filter(|x| x.1 == slot).map(|x| x.0).peekable();
            let none = es.peek().is_none();
            let ok = match self.format {
                Format::Mortar | Format::Lld => w == 0,
                Format::Mold => w == resolver || (none && w == 0),
                Format::Gnu => {
                    if none {
                        w == 0
                    } else {
                        es.any(|e| w == if endbr(e) { e } else { e + 6 })
                    }
                }
            };
            if !ok {
                let mut x = f(
                    5,
                    "the IFUNC word does not hold, before start-up, what the linker writes there (spec/x64.md X-39)",
                )
                .at(slot);
                x.found = Some(w);
                fs.push(x);
            }
        }
    }

    /// The slot an AArch64 IPLT entry whose `ADRP` is at `a` loads: `ADRP
    /// Xn, slot; LDR Xt, [Xn, #lo12]`, then `BR Xt` (mold) or `ADD Xn, Xn,
    /// #lo12; BR Xt` (GNU ld, lld). A `BTI` before the `ADRP` is the
    /// entry's first word (the caller's scan).
    fn a64_entry(&self, a: u64) -> Option<u64> {
        let (n, pg) = a64::adrp(self.image.insn(a)?, a)?;
        let (t, n2, lo) = a64::ldr64_imm(self.image.insn(a + 4)?)?;
        if n2 != n {
            return None;
        }
        let i2 = self.image.insn(a + 8)?;
        let ok = br(i2) == Some(t)
            || (a64::add_imm(i2).is_some_and(|(d, m, _)| d == n && m == n)
                && self.image.insn(a + 12).and_then(br) == Some(t));
        ok.then_some(pg.wrapping_add(lo))
    }

    /// A slot that is an IRELATIVE slot of `resolver`: it holds the
    /// function the resolver picks (§4.8).
    fn resolved_slot(&self, g: u64, resolver: u64) -> bool {
        g.is_multiple_of(self.word())
            && !self.occupied_at(g)
            && self.iplt.is_irel_slot_of(g, resolver)
    }

    /// A record whose symbol is an IFUNC (§4.8).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ifunc_record<'r>(
        &mut self,
        oi: usize,
        rec: &Rec,
        site: Site,
        symbol: String,
        resolver: u64,
        key: IfKey,
        partner: &dyn Fn(i64, &[u32]) -> Option<&'r Rec>,
    ) {
        let entries = self.iplt.entries_of(resolver).to_vec();
        let a = rec.addend as u64;
        let slot_a = slot_addend(rec);
        // A GOT-indirect reference may load an IRELATIVE slot with no IPLT
        // entry (GNU ld's static x86-64 output, finish_ifunc).
        if entries.is_empty() && !rec.def.op.is_slot() {
            let mut x = Self::finding(
                Rule::Ifunc(4),
                rec,
                &site,
                &symbol,
                format!("refers to an IFUNC whose resolver {resolver:#x} has no IPLT entry"),
            );
            x.expected = Some(resolver);
            return self.fail(x);
        }
        let pend = Pending { rec: rec.clone(), site, symbol, want: Want::Word(0), w1: false };
        if let Op::Slot(..) = rec.def.op {
            let ok: Vec<(u64, Option<Rewrite>)> = entries
                .iter()
                .filter_map(|&e| {
                    got_rewrite(rec, e.wrapping_add(slot_a), partner).map(|r| (e, Some(r)))
                })
                .collect();
            let ev = if ok.is_empty() { self.slot_evidence(rec) } else { Some(Evidence::None) };
            let Some(ev) = ev else { return self.not_checked(Reason::SymbolUnresolved, 1) };
            // The page half of a pair that is neither an ADRP nor W2/W3's
            // form reaches nothing.
            if ok.is_empty() && matches!(ev, Evidence::None) {
                let mut x = Self::finding(
                    Rule::Ifunc(4),
                    rec,
                    &pend.site,
                    &pend.symbol,
                    "the GOT-indirect reference to an IFUNC is neither a GOT load nor a specified rewriting to an IPLT entry",
                );
                x.expected = entries.first().map(|e| e.wrapping_add(a));
                x.found = Some(rec.out_word);
                return self.fail(x);
            }
            let g = self.ifunc_groups.entry(key).or_insert_with(|| IfGroup {
                resolver,
                addrs: Vec::new(),
                gots: BTreeMap::new(),
            });
            if ok.is_empty() {
                g.gots.entry((oi, rec.sym, rec.addend)).or_default().members.push((pend, ev));
            } else {
                g.addrs.push((pend, ok));
            }
            return;
        }
        let branch = is_branch(rec);
        let mut ok: Vec<(u64, Option<Rewrite>)> = Vec::new();
        let mut last = None;
        for &e in &entries {
            let sa = e.wrapping_add(a);
            let Some(x) = x_direct(rec.def.op, sa, rec.p, self.got) else {
                return self.not_checked(Reason::SymbolUnresolved, 1);
            };
            match judge(rec, x) {
                Verdict::Ok(rw) => ok.push((e, rw)),
                v => {
                    if let Some(rw) = self.direct_rewrite(rec, sa, false, partner) {
                        ok.push((e, Some(rw)));
                    } else if last.is_none() {
                        last = Some((v, x));
                    }
                }
            }
        }
        if ok.is_empty() {
            let x = if let Some((v @ (Verdict::Overflow | Verdict::Misaligned), x)) = last {
                verdict_finding(&pend.rec, &pend.site, &pend.symbol, Some((v, x)))
            } else {
                let mut x = Self::finding(
                    Rule::Ifunc(4),
                    rec,
                    &pend.site,
                    &pend.symbol,
                    "the reference to an IFUNC reaches no IPLT entry of its resolver",
                );
                x.expected = Some(entries[0].wrapping_add(a));
                x.found = Some(self.gotrel_address(rec).unwrap_or(rec.out_word));
                x
            };
            return self.fail(x);
        }
        // A call may go through any entry of the resolver: each calls the
        // function it picks. An address must be the symbol's one address.
        if branch {
            match ok[0].1 {
                Some(rw) => {
                    self.note_veneer(rw, rec, &pend.site, &pend.symbol, ok[0].0.wrapping_add(a));
                    self.cov.add_accepted(rw, rec.def.r_type);
                }
                None => self.cov.add_verified(Via::Iplt, rec.def.r_type),
            }
            return;
        }
        self.ifunc_groups
            .entry(key)
            .or_insert_with(|| IfGroup { resolver, addrs: Vec::new(), gots: BTreeMap::new() })
            .addrs
            .push((pend, ok));
    }

    /// Decides the IFUNC groups (§4.8): one canonical address per symbol.
    #[allow(clippy::too_many_lines)]
    pub fn finish_ifunc(&mut self) {
        let groups = std::mem::take(&mut self.ifunc_groups);
        for (_, g) in groups {
            let entries = self.iplt.entries_of(g.resolver).to_vec();
            // GOT members: the entries a slot they reach holds, and whether
            // one is an IRELATIVE slot of the resolver.
            let mut gots: Vec<GotMember> = Vec::new();
            for (_, grp) in g.gots {
                if grp.unpaired() {
                    self.not_checked(Reason::GotUnpaired, grp.members.len() as u64);
                    continue;
                }
                let cands = grp.candidates_from(self.got);
                for (pend, _) in grp.members {
                    let (mut canon, mut resolved, mut last) = (Vec::new(), false, None);
                    let a = slot_addend(&pend.rec);
                    for &c in &cands {
                        let Some(x) =
                            x_got(pend.rec.def.op, c, pend.rec.p, self.got, pend.rec.addend)
                        else {
                            continue;
                        };
                        match judge(&pend.rec, x) {
                            Verdict::Ok(_) => {
                                canon.extend(
                                    entries.iter().copied().filter(|&e| {
                                        self.slot_ok(c, Want::Word(e.wrapping_add(a)))
                                    }),
                                );
                                resolved |= self.resolved_slot(c, g.resolver);
                            }
                            v => last = Some((v, x)),
                        }
                    }
                    canon.sort_unstable();
                    canon.dedup();
                    gots.push((pend, canon, resolved, last));
                }
            }
            // The canonical entry: the one most references take (the
            // lowest on a tie).
            let mut count: BTreeMap<u64, usize> = BTreeMap::new();
            for (_, ok) in &g.addrs {
                let es: BTreeSet<u64> = ok.iter().map(|o| o.0).collect();
                for e in es {
                    *count.entry(e).or_default() += 1;
                }
            }
            for (_, canon, _, _) in &gots {
                for &e in canon {
                    *count.entry(e).or_default() += 1;
                }
            }
            let chosen =
                count.iter().max_by(|x, y| x.1.cmp(y.1).then(y.0.cmp(x.0))).map(|(&e, _)| e);
            let unequal = |pend: &Pending, what: &str| {
                let mut x = Self::finding(
                    Rule::Ifunc(3),
                    &pend.rec,
                    &pend.site,
                    &pend.symbol,
                    format!(
                        "{what}, where the other references to the IFUNC take IPLT entry {:#x}: two addresses for one function",
                        chosen.unwrap_or(0)
                    ),
                );
                x.expected = chosen.map(|c| c.wrapping_add(pend.rec.addend as u64));
                x
            };
            for (pend, ok) in g.addrs {
                match ok.iter().find(|o| Some(o.0) == chosen) {
                    Some((_, Some(rw))) => self.cov.add_accepted(*rw, pend.rec.def.r_type),
                    Some((_, None)) => self.cov.add_verified(Via::Iplt, pend.rec.def.r_type),
                    None => {
                        let x = unequal(&pend, &format!("takes IPLT entry {:#x}", ok[0].0));
                        self.fail(x);
                    }
                }
            }
            for (pend, canon, resolved, last) in gots {
                let ty = pend.rec.def.r_type;
                if chosen.is_some_and(|c| canon.contains(&c)) || (resolved && chosen.is_none()) {
                    self.cov.add_verified(Via::Iplt, ty);
                } else if resolved {
                    let x = unequal(&pend, "loads the function its IRELATIVE slot resolves to");
                    self.fail(x);
                } else if let Some(&e) = canon.first() {
                    let x = unequal(&pend, &format!("loads IPLT entry {e:#x}"));
                    self.fail(x);
                } else if let Some((v @ (Verdict::Overflow | Verdict::Misaligned), x)) = last {
                    let x = verdict_finding(&pend.rec, &pend.site, &pend.symbol, Some((v, x)));
                    self.fail(x);
                } else {
                    let mut x = Self::finding(
                        Rule::Ifunc(4),
                        &pend.rec,
                        &pend.site,
                        &pend.symbol,
                        "the GOT-indirect reference to an IFUNC reaches no slot that holds an IPLT entry of its resolver or is its IRELATIVE slot",
                    );
                    x.expected = entries.first().copied();
                    self.fail(x);
                }
            }
        }
    }
}

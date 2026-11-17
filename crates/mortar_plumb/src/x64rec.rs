//! x86-64 records (`spec/plumb.md` §4.5, §5.9): the value of every type
//! of `spec/x64.md` §6.1, and the rewritings of §8 (mortar's) and §8.8
//! (each reference linker's), matched on the bytes of the record's window
//! (§8.6) in the input section and in the output.
//!
//! A window is read only where the input section has its bytes (X-8). A
//! rewriting is accepted only in the form the map's linker writes (§5.9's
//! table); under mortar's map only mortar's forms.

use mortar_elf::SymbolIndex;
use mortar_elf::consts::{
    ET_DYN, ET_EXEC, PT_DYNAMIC, PT_INTERP, SHF_EXECINSTR, SHN_ABS, SHN_UNDEF,
};

use super::{
    Engine, Pending, Rec, Target, TlsBlock, Verdict, Want, find_at, judge, verdict_finding,
};
use crate::a64::{Op, Slot};
use crate::map::Format;
use crate::report::{Finding, Reason, Rewrite, Rule, Site, Via};
use crate::x64::{self as x, rex_rb, rip_reg};
use crate::x86op::{self, CALL_DISP32, JMP_DISP32, imm_form, le};

const TLS_GET_ADDR: &str = "__tls_get_addr";
const TLS_MODULE_BASE: &str = "_TLS_MODULE_BASE_";

/// What a GOT-indirect reference's symbol is (`spec/x64.md` §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Defined in a placed section of the output.
    Local,
    /// `SHN_ABS`.
    Abs,
    /// Undefined weak: S = 0.
    Weak0,
}

/// A TLS sequence's verdict (§5.9).
enum SeqX {
    /// Rewritten to local-exec over `[lo, hi)`.
    Relaxed(Rewrite, u64, u64),
    /// Not a form the linker writes: the message, the bytes expected, and
    /// the output bytes the finding covers (for S12).
    Bad(String, Vec<u8>, Option<(u64, u64)>),
    /// Not judged: the symbol cannot be resolved (a stripped output).
    Unknown(Reason),
    /// Stage 3: kept for the loader, its GOT pair verified: the pair
    /// (module, offset) it holds.
    Kept(u64, u64),
}

/// Why T of a TLS reference is not known.
enum NoT {
    /// The symbol is not resolved: not judged.
    Unknown(Reason),
    /// The reference is wrong: the message.
    Bad(String),
}

/// The output bytes around `p` a rewriting of a record of type `ty` may
/// change (§5.9): exempted from S12 when the record is not judged.
fn rewrite_window(ty: u32, p: u64) -> Option<(u64, u64)> {
    match ty {
        x::GOTPCREL | x::GOTPCRELX | x::REX_GOTPCRELX => Some((p.wrapping_sub(3), p + 5)),
        x::GOTTPOFF | x::GOTPC32_TLSDESC => Some((p.wrapping_sub(3), p)),
        x::TLSDESC_CALL => Some((p, p + 2)),
        x::TLSGD | x::TLSLD => Some((p.wrapping_sub(4), p + 18)),
        _ => None,
    }
}

impl Engine<'_, '_> {
    /// One x86-64 record (§5.1 steps 2 to 7).
    #[allow(clippy::too_many_arguments, clippy::too_many_lines, clippy::many_single_char_names)]
    pub(super) fn x64_record(
        &mut self,
        oi: usize,
        si: usize,
        recs: &[Rec],
        sorted: &[(u64, usize)],
        i: usize,
        site: Site,
        symbol: String,
    ) {
        let rec = &recs[i];
        let ty = rec.def.r_type;
        let sec = &self.objs[oi].secs[si];
        let alloc = sec.alloc();
        let exec = sec.shdr.sh_flags & SHF_EXECINSTR != 0;
        // The follower of a TLS sequence: the lead's verdict (X-25, X-27).
        if symbol == TLS_GET_ADDR
            && alloc
            && let Some(lead) = Self::x64_lead_of(recs, sorted, rec)
        {
            match self.x64_seq(oi, si, recs, sorted, lead) {
                SeqX::Relaxed(rw, ..) => return self.cov.add_accepted(rw, ty),
                SeqX::Bad(..) => return self.not_checked(Reason::TlsSequence, 1),
                SeqX::Unknown(r) => return self.not_checked(r, 1),
                // A kept sequence's call is an ordinary call.
                SeqX::Kept(..) => {}
            }
        }
        // X-48 (mortar): a static output makes no TLS runtime call; a
        // branch or GOT load against `__tls_get_addr` that no GD/LD
        // sequence consumed survives as a call (spec/x64.md X-48).
        if symbol == TLS_GET_ADDR
            && alloc
            && exec
            && self.format == Format::Mortar
            && self.image.header.e_type == ET_EXEC
            && !self.image.phdrs.iter().any(|p| matches!(p.p_type, PT_DYNAMIC | PT_INTERP))
            && matches!(ty, x::PLT32 | x::PC32 | x::GOTPCREL | x::GOTPCRELX | x::REX_GOTPCRELX)
        {
            let f = Self::finding(
                Rule::Insn,
                rec,
                &site,
                &symbol,
                "a call to __tls_get_addr survives in a static output (spec/x64.md X-48)",
            );
            return self.fail(f);
        }
        match rec.def.op {
            Op::Slot(Slot::Gd | Slot::Ld, _) => {
                return match self.x64_seq(oi, si, recs, sorted, rec) {
                    SeqX::Relaxed(rw, lo, hi) => {
                        self.x64_rewrote(lo, hi);
                        self.cov.add_accepted(rw, ty);
                    }
                    SeqX::Bad(msg, want, span) => {
                        if let Some((lo, hi)) = span {
                            self.x64_rewrote(lo, hi);
                        }
                        let mut f = Self::finding(Rule::Field, rec, &site, &symbol, msg);
                        f.expected = (!want.is_empty()).then(|| le(&want));
                        self.fail(f);
                    }
                    SeqX::Unknown(r) => self.x64_unjudged(rec, r),
                    SeqX::Kept(m, o) => {
                        // What the pair holds (D-46's `got_demands`, as
                        // AArch64's): a bound key's static DTP offset is a
                        // word no record writes (P-D26, WP-144).
                        self.demand(Want::Pair(m, o));
                        self.cov.add_verified(Via::GotSlot, ty);
                    }
                };
            }
            Op::Slot(Slot::Desc | Slot::TpRel, _) | Op::DescCall | Op::TpRel => {
                return self.x64_tls(oi, si, rec, site, symbol);
            }
            Op::DtpRel => return self.x64_dtprel(oi, rec, &site, &symbol, alloc, exec),
            Op::None => {
                self.cov.none += 1;
                return;
            }
            _ => {}
        }
        let (target, addend) = self.resolve_at(oi, rec.sym, rec.addend);
        let (s, kind) = match target {
            Target::Skip(r) => return self.x64_unjudged(rec, r),
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
            Target::Outside(_) => {
                if self.format != Format::Mortar {
                    return self.not_checked(Reason::PieceNotLocatable, 1);
                }
                let f = Self::finding(
                    Rule::MergePiece,
                    rec,
                    &site,
                    &symbol,
                    "a section-symbol reference outside its mergeable section (spec/synth.md S-18 refuses it)",
                );
                return self.fail(f);
            }
            Target::Ifunc { resolver, key } => {
                return self.ifunc_record(oi, rec, site, symbol, resolver, key, &|_, _| None);
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
                    &|_, _| None,
                );
            }
            Target::Tls(s) => {
                // X-53: `64`/`32` against a thread-local symbol at a
                // non-alloc place is DtpRel(S + A); in alloc memory it is
                // not a TLS type (X-11).
                if alloc || rec.def.op != Op::Abs {
                    let f = Self::finding(
                        Rule::Insn,
                        rec,
                        &site,
                        &symbol,
                        "a type that is not thread-local against a thread-local symbol",
                    );
                    return self.fail(f);
                }
                return self.x64_debug_tls(rec, &site, &symbol, s.wrapping_add(addend as u64));
            }
            Target::Addr(s) => (s, if self.x64_abs(oi, rec.sym) { Kind::Abs } else { Kind::Local }),
            Target::UndefWeak => (0, Kind::Weak0),
        };
        let a = addend as u64;
        let p = rec.p;
        let xv = match rec.def.op {
            Op::Abs => s.wrapping_add(a),
            Op::Prel => s.wrapping_add(a).wrapping_sub(p),
            Op::GotRel => match self.got {
                Some(g) => s.wrapping_add(a).wrapping_sub(g),
                None => return self.not_checked(Reason::SymbolUnresolved, 1),
            },
            Op::GotPc => match self.got {
                Some(g) => g.wrapping_add(a).wrapping_sub(p),
                None => return self.not_checked(Reason::SymbolUnresolved, 1),
            },
            Op::Size => match self.x64_size(oi, rec.sym) {
                Some(z) => z.wrapping_add(a),
                None => return self.not_checked(Reason::SymbolUnresolved, 1),
            },
            Op::Slot(slot, _) => {
                self.got_demands.insert(s);
                if alloc && let Some(rw) = self.x64_got_form(oi, si, rec, s, kind) {
                    if rw == Rewrite::GotToLea && self.abs_in_pic(oi, rec.sym, kind) {
                        return self.fail(Self::pc_to_absolute(rec, &site, &symbol));
                    }
                    return self.cov.add_accepted(rw, ty);
                }
                if !self.x64_window_kept(oi, si, rec) {
                    let mut f = Self::finding(
                        Rule::Field,
                        rec,
                        &site,
                        &symbol,
                        format!(
                            "the instruction is neither the GOT-indirect one of the input nor a rewriting {} writes (spec/plumb.md §5.9)",
                            linker_name(self.format)
                        ),
                    );
                    f.found = Some(rec.out_word);
                    return self.fail(f);
                }
                let Some(ev) = self.slot_evidence(rec) else {
                    return self.x64_unjudged(rec, Reason::SymbolUnresolved);
                };
                let pend = Pending { rec: rec.clone(), site, symbol, want: Want::Word(s) };
                self.got_groups.entry((slot, oi, rec.sym, 0)).or_default().members.push((pend, ev));
                return;
            }
            _ => return self.not_checked(Reason::Type, 1),
        };
        // §5.1a (WP-088): in a position-independent output, S − P against
        // an absolute S is right at load bias 0 only.
        if rec.def.op == Op::Prel && alloc && self.abs_in_pic(oi, rec.sym, kind) {
            return self.fail(Self::pc_to_absolute(rec, &site, &symbol));
        }
        let failed = self.cov.failed;
        // Stage 3: a call or jump through a PLT entry whose word holds S.
        // (A weak undefined call through a PLT whose word holds 0 calls 0.)
        if matches!(ty, x::PLT32 | x::PC32)
            && !matches!(judge(rec, xv as i64), Verdict::Ok(_))
            && self
                .plt_hop(rec.p.wrapping_add(4).wrapping_add(rec.out_word as u32 as i32 as u64), s)
        {
            return self.cov.add_verified(Via::Plt, ty);
        }
        // `PLTOFF64` is `L + A − GOT` (x64.md §6.1): L a PLT entry whose
        // word holds S (a dynamic key, `-mcmodel=large` code).
        if ty == x::PLTOFF64
            && !matches!(judge(rec, xv as i64), Verdict::Ok(_))
            && let Some(g) = self.got
            && self.plt_hop(rec.out_word.wrapping_add(g).wrapping_sub(a), s)
        {
            return self.cov.add_verified(Via::Plt, ty);
        }
        self.x64_judge(rec, &site, &symbol, xv as i64);
        if rec.def.op == Op::Abs
            && rec.def.field.width() >= 4
            && kind != Kind::Weak0
            && self.cov.failed == failed
            && self.objs[oi].secs[si].alloc()
        {
            let internal = self.internal(s) && !self.absolute(oi, rec.sym);
            self.abs_words.push((rec.p, rec.def.field.width(), xv, internal));
        }
    }

    /// Whether a target of `kind`, symbol `idx` of object `oi`, is absolute
    /// in an `ET_DYN` output (a `Pie` or a static-pie: every address moves
    /// with the load bias). The chosen input definition decides as well as
    /// the output's global: mold makes the symbol local.
    fn abs_in_pic(&self, oi: usize, idx: u32, kind: Kind) -> bool {
        self.image.header.e_type == ET_DYN
            && (kind == Kind::Abs || (kind == Kind::Local && self.absolute(oi, idx)))
    }

    /// §5.1a: a PC-relative field (or a GOT load rewritten to one, X1)
    /// whose target is an absolute symbol, in an `ET_DYN` output.
    fn pc_to_absolute(rec: &Rec, site: &Site, symbol: &str) -> Finding {
        let mut f = Self::finding(
            Rule::Field,
            rec,
            site,
            symbol,
            "a PC-relative field to an absolute symbol in a position-independent output: right at load bias 0 only (spec/plumb.md §5.1a)",
        );
        f.found = Some(rec.out_word);
        f
    }

    /// A record not judged for `r`: the bytes a rewriting of it may change
    /// are not compared either (S12).
    fn x64_unjudged(&mut self, rec: &Rec, r: Reason) {
        if let Some((lo, hi)) = rewrite_window(rec.def.r_type, rec.p) {
            self.x64_rewrote(lo, hi);
        }
        self.not_checked(r, 1);
    }

    /// R1-R3 for a data value, the record counted as verified directly.
    pub(super) fn x64_judge(&mut self, rec: &Rec, site: &Site, symbol: &str, xv: i64) {
        match judge(rec, xv) {
            Verdict::Ok(_) => self.cov.add_verified(Via::Direct, rec.def.r_type),
            v => {
                let f = verdict_finding(rec, site, symbol, Some((v, xv)));
                self.fail(f);
            }
        }
    }

    /// `64`/`32` against a TLS symbol at a non-alloc place (X-53): the
    /// DTP offset; GNU ld and mold write the template address `t`
    /// (`spec/x64.md` W10), which is a finding §10.2 names.
    pub(super) fn x64_debug_tls(&mut self, rec: &Rec, site: &Site, symbol: &str, t: u64) {
        let Ok(Some(b)) = self.tls else { return self.not_checked(Reason::Tls, 1) };
        let want = b.dtprel(t) as i64;
        if let Verdict::Ok(_) = judge(rec, want) {
            return self.cov.add_verified(Via::Direct, rec.def.r_type);
        }
        if matches!(judge(rec, t as i64), Verdict::Ok(_)) {
            let mut f = Self::finding(
                Rule::Field,
                rec,
                site,
                symbol,
                if rec.def.r_type == crate::x86::R32 && self.word() == 4 {
                    "a TLS symbol's template address where the DTP offset belongs (spec/x86.md X86-62)"
                } else {
                    "a TLS symbol's template address where the DTP offset belongs (spec/x64.md X-53)"
                },
            );
            f.expected = Some(want as u64);
            f.found = Some(rec.out_word);
            return self.fail(f);
        }
        self.x64_judge(rec, site, symbol, want);
    }

    /// Whether symbol `idx` of object `oi` is absolute: `SHN_ABS` in the
    /// input, or, undefined there, in the output.
    pub(super) fn x64_abs(&self, oi: usize, idx: u32) -> bool {
        let o = &self.objs[oi];
        let Some(t) = o.symtab else { return false };
        let Ok(sym) = t.get(SymbolIndex(idx)) else { return false };
        match sym.st_shndx {
            SHN_ABS => true,
            SHN_UNDEF => {
                let name = String::from_utf8_lossy(t.name(&sym).unwrap_or(b""));
                self.image.global(&name).is_some_and(|g| g.sym.st_shndx == SHN_ABS)
            }
            _ => false,
        }
    }

    /// Z: the size of symbol `idx`'s definition (§6.1 `SIZE32`/`SIZE64`).
    pub(super) fn x64_size(&self, oi: usize, idx: u32) -> Option<u64> {
        let o = &self.objs[oi];
        let t = o.symtab?;
        let sym = t.get(SymbolIndex(idx)).ok()?;
        if sym.st_shndx != SHN_UNDEF {
            return Some(sym.st_size);
        }
        let name = String::from_utf8_lossy(t.name(&sym).ok()?);
        self.image.global(&name).filter(|g| g.sym.st_shndx != SHN_UNDEF).map(|g| g.sym.st_size)
    }

    /// The input and output bytes of `[P - before, P + width + after)`,
    /// when the input section holds them all (X-8) and the output too.
    pub(super) fn x64_window(
        &self,
        oi: usize,
        si: usize,
        rec: &Rec,
        before: u64,
        after: u64,
    ) -> Option<(Vec<u8>, &[u8])> {
        let data = self.objs[oi].data(si);
        let lo = rec.off.checked_sub(before)?;
        let hi = rec.off.checked_add(u64::from(rec.def.field.width()) + after)?;
        let inp = data.get(usize::try_from(lo).ok()?..usize::try_from(hi).ok()?)?.to_vec();
        let out = self.image.bytes(rec.p.checked_sub(before)?, hi - lo)?;
        Some((inp, out))
    }

    /// Marks output bytes `[lo, hi)` as written by a rewriting, for S12.
    pub(super) fn x64_rewrote(&mut self, lo: u64, hi: u64) {
        self.rewritten_words.extend(lo..hi);
    }

    /// Whether a GOT-indirect record's instruction is the input's: the
    /// bytes of its window outside the field are unchanged.
    fn x64_window_kept(&self, oi: usize, si: usize, rec: &Rec) -> bool {
        let before = match rec.def.r_type {
            x::REX_GOTPCRELX | x::GOTTPOFF | x::GOTPC32_TLSDESC => 3,
            x::GOTPCRELX | x::GOTPCREL => 2,
            _ => 0,
        };
        let before = before.min(rec.off);
        self.x64_window(oi, si, rec, before, 0)
            .is_none_or(|(i, o)| i[..before as usize] == o[..before as usize])
    }

    /// The GOTPCRELX rewritings of §5.9 the map's linker writes, for a
    /// symbol of `kind` at `s`: the one the output holds.
    #[allow(clippy::many_single_char_names)] // s, k, p, r: the psABI's names
    fn x64_got_form(
        &mut self,
        oi: usize,
        si: usize,
        rec: &Rec,
        s: u64,
        kind: Kind,
    ) -> Option<Rewrite> {
        let ty = rec.def.r_type;
        let k: u64 = match ty {
            x::REX_GOTPCRELX => 3,
            x::GOTPCRELX | x::GOTPCREL => 2,
            _ => return None,
        };
        let (inp, out) = self.x64_window(oi, si, rec, k, 0)?;
        let (inp, out) = (inp.clone(), out.to_vec());
        let k = k as usize;
        let (op, modrm) = (inp[k - 2], inp[k - 1]);
        let rex = (k == 3).then(|| inp[0]).filter(|r| r & 0xf0 == 0x40);
        if k == 3 && rex.is_none() {
            return None;
        }
        let a = rec.addend as u64;
        let pc = |t: u64| t.wrapping_add(a).wrapping_sub(rec.p) as i64;
        let fits = |v: i64| i32::try_from(v).is_ok();
        let f = self.format;
        let (gnu, lld, mold, mortar) =
            (f == Format::Gnu, f == Format::Lld, f == Format::Mold, f == Format::Mortar);
        let _ = &inp;
        let mut cands: Vec<(Rewrite, Vec<u8>)> = Vec::new();
        let mut push = |rw, head: &[u8], field: i64, tail: &[u8]| {
            let mut v = head.to_vec();
            v.extend_from_slice(&(field as u32).to_le_bytes());
            v.extend_from_slice(tail);
            cands.push((rw, v));
        };
        let prefix = |b: &[u8]| -> Vec<u8> {
            rex.map(|r| vec![r]).into_iter().flatten().chain(b.iter().copied()).collect()
        };
        let primed = |b: &[u8]| -> Vec<u8> {
            rex.map(|r| vec![rex_rb(r)]).into_iter().flatten().chain(b.iter().copied()).collect()
        };
        let reg = rip_reg(modrm);
        let local = kind == Kind::Local;
        let plain = ty == x::GOTPCREL;
        // X1: mov -> lea of the address: mortar and lld for a local
        // symbol, GNU ld also through a plain GOTPCREL, mold for every
        // REX_GOTPCRELX mov (X-22: an absolute or undefined weak one too).
        let x1 = match f {
            Format::Mortar | Format::Lld => local && !plain,
            Format::Gnu => local,
            Format::Mold => ty == x::REX_GOTPCRELX,
        };
        if x1 && op == 0x8b && reg.is_some() && fits(pc(s)) {
            push(Rewrite::GotToLea, &prefix(&[0x8d, modrm]), pc(s), &[]);
        }
        // X4 (GNU ld): mov -> mov $S.
        if gnu
            && !plain
            && op == 0x8b
            && let Some(r) = reg
        {
            let imm_ok = if rex.is_some_and(|r| r & 8 != 0) {
                i32::try_from(s as i64).is_ok()
            } else {
                u32::try_from(s).is_ok()
            };
            if imm_ok {
                let head = imm_form(op, r).expect("8b has an immediate form");
                push(Rewrite::GotToMovImm, &primed(&head), s as i64, &[]);
            }
        }
        // X2: call/jmp through the GOT -> direct.
        if !plain && op == 0xff && (modrm == CALL_DISP32 || modrm == JMP_DISP32) {
            let target_ok = local || ((gnu || mold) && kind == Kind::Weak0);
            if target_ok && (mortar || lld || gnu) && k == 2 {
                if modrm == CALL_DISP32 {
                    push(Rewrite::GotToDirect, &[0x67, 0xe8], pc(s), &[]);
                } else {
                    push(Rewrite::GotToDirect, &[0xe9], pc(s) + 1, &[0x90]);
                }
            }
            if target_ok && mold && k == 2 {
                let o = if modrm == CALL_DISP32 { 0xe8 } else { 0xe9 };
                push(Rewrite::GotToDirect, &[0x40, o], pc(s), &[]);
            }
        }
        // X3: ALU operation or test with a GOT operand -> immediate (static).
        let alu = x86op::alu(op);
        if !plain
            && (alu || op == 0x85)
            && let Some(r) = reg
        {
            let rexw = rex.is_some_and(|r| r & 8 != 0);
            let target_ok = local || (gnu && matches!(kind, Kind::Abs | Kind::Weak0));
            let linker_ok = ((mortar || lld) && rexw) || (gnu && (rexw || rex.is_none()));
            let imm_ok = if rexw { (0..1 << 31).contains(&s) } else { u32::try_from(s).is_ok() };
            if target_ok && linker_ok && imm_ok {
                let head = imm_form(op, r).expect("an ALU op or test");
                push(Rewrite::GotToImm, &primed(&head), s as i64, &[]);
            }
        }
        let start = rec.p - k as u64;
        let (rw, _) = cands.into_iter().find(|(_, v)| *v == out)?;
        self.x64_rewrote(start, rec.p + 4);
        Some(rw)
    }

    /// The lead of the TLS sequence whose follower `rec` is (X-30): a
    /// `TLSGD`/`TLSLD` at the offset its form puts the call's record.
    fn x64_lead_of<'r>(recs: &'r [Rec], sorted: &[(u64, usize)], rec: &Rec) -> Option<&'r Rec> {
        let deltas: &[(u64, u32)] = match rec.def.r_type {
            x::PLT32 | x::PC32 => &[(8, x::TLSGD), (5, x::TLSLD)],
            x::GOTPCRELX | x::GOTPCREL => &[(8, x::TLSGD), (6, x::TLSLD)],
            x::PLTOFF64 => &[(6, x::TLSGD), (6, x::TLSLD)],
            _ => return None,
        };
        deltas.iter().find_map(|&(d, t)| {
            find_at(recs, sorted, rec.off.checked_sub(d), |r| r.def.r_type == t)
        })
    }

    /// The follower of the sequence a lead starts, if the form has one.
    fn x64_follower<'r>(
        recs: &'r [Rec],
        sorted: &[(u64, usize)],
        lead: &Rec,
        delta: u64,
        types: &[u32],
    ) -> Option<&'r Rec> {
        find_at(recs, sorted, lead.off.checked_add(delta), |r| types.contains(&r.def.r_type))
    }

    /// A `TLSGD`/`TLSLD` sequence (X-25, X-27; §5.9): the input form, and
    /// the output against the LE forms the map's linker writes.
    #[allow(clippy::too_many_lines, clippy::many_single_char_names)]
    fn x64_seq(
        &self,
        oi: usize,
        si: usize,
        recs: &[Rec],
        sorted: &[(u64, usize)],
        lead: &Rec,
    ) -> SeqX {
        let gd = lead.def.r_type == x::TLSGD;
        let what = if gd { "general-dynamic" } else { "local-dynamic" };
        let unrec = || {
            SeqX::Bad(
                format!("an unrecognized {what} TLS sequence (spec/x64.md X-26)"),
                Vec::new(),
                None,
            )
        };
        let Ok(Some(b)) = self.tls else {
            return SeqX::Bad(
                "a TLS sequence in an output without a usable PT_TLS".into(),
                Vec::new(),
                None,
            );
        };
        let f = self.format;
        let mold = f == Format::Mold;
        let follows = |d, types: &[u32]| {
            Self::x64_follower(recs, sorted, lead, d, types).is_some_and(|r| {
                self.objs[oi]
                    .symtab
                    .and_then(|t| t.get(SymbolIndex(r.sym)).ok().map(|s| (t, s)))
                    .is_some_and(|(t, s)| t.name(&s).is_ok_and(|n| n == TLS_GET_ADDR.as_bytes()))
            })
        };
        // The forms: (before, after, rewritten-output).
        let data = self.objs[oi].data(si);
        let at = |k: i64, n: usize| -> Option<&[u8]> {
            let lo = usize::try_from((lead.off as i64).checked_add(k)?).ok()?;
            data.get(lo..lo.checked_add(n)?)
        };
        if lead.addend != -4 {
            return unrec();
        }
        // The TP offset of a GD target (X-18: an undefined weak's).
        let t = if gd {
            match self.x64_tls_value(oi, lead, b) {
                Ok(v) => v,
                Err(NoT::Bad(m)) => return SeqX::Bad(m, Vec::new(), None),
                Err(NoT::Unknown(r)) => return SeqX::Unknown(r),
            }
        } else {
            0
        };
        let size = b.tcb.wrapping_neg() as u32;
        let (before, after, want): (u64, u64, Vec<u8>) = if gd {
            let g16 = at(-4, 4) == Some(&[0x66, 0x48, 0x8d, 0x3d]);
            if g16
                && ((at(4, 4) == Some(&[0x66, 0x66, 0x48, 0xe8])
                    && follows(8, &[x::PLT32, x::PC32]))
                    || (at(4, 4) == Some(&[0x66, 0x48, 0xff, 0x15])
                        && follows(8, &[x::GOTPCRELX, x::GOTPCREL])))
            {
                (4, 8, x::gd_le(t as u32, mold).to_vec())
            } else if at(-3, 3) == Some(&[0x48, 0x8d, 0x3d])
                && at(4, 2) == Some(&[0x48, 0xb8])
                && at(14, 5).is_some_and(|v| {
                    x::rex_w(v[0]) && v[1] == 0x01 && v[2] & 0xc7 == 0xc0 && v[3..] == [0xff, 0xd0]
                })
                && follows(6, &[x::PLTOFF64])
            {
                let mut v = x::gd_le(t as u32, mold).to_vec();
                v.extend_from_slice(&x::NOPW6);
                (3, 15, v)
            } else {
                return unrec();
            }
        } else if at(-3, 3) != Some(&[0x48, 0x8d, 0x3d]) {
            return unrec();
        } else if at(4, 1) == Some(&[0xe8]) && follows(5, &[x::PLT32, x::PC32]) {
            let v = if mold {
                [&[0x31, 0xc0, 0x64, 0x48, 0x8b, 0x00, 0x48, 0x2d][..], &size.to_le_bytes()]
                    .concat()
            } else {
                [&[0x66, 0x66, 0x66][..], &x::MOV_FS0_RAX].concat()
            };
            (3, 5, v)
        } else if at(4, 2) == Some(&[0xff, 0x15]) && follows(6, &[x::GOTPCRELX, x::GOTPCREL]) {
            let v = if mold {
                [&[0x48, 0x31, 0xc0, 0x64, 0x48, 0x8b, 0x00, 0x48, 0x2d][..], &size.to_le_bytes()]
                    .concat()
            } else {
                [&[0x66, 0x66, 0x66, 0x66][..], &x::MOV_FS0_RAX].concat()
            };
            (3, 6, v)
        } else if at(4, 2) == Some(&[0x48, 0xb8])
            && at(14, 5).is_some_and(|v| {
                x::rex_w(v[0]) && v[1] == 0x01 && v[2] & 0xc7 == 0xc0 && v[3..] == [0xff, 0xd0]
            })
            && follows(6, &[x::PLTOFF64])
        {
            let v = if mold {
                [
                    &x::MOV_FS0_RAX[..],
                    &[0x48, 0x2d],
                    &size.to_le_bytes(),
                    &[0x0f, 0x1f, 0x80, 0, 0, 0, 0],
                ]
                .concat()
            } else {
                [&x::NOP13[..], &x::MOV_FS0_RAX].concat()
            };
            (3, 15, v)
        } else {
            return unrec();
        };
        let lo = lead.p - before;
        let hi = lead.p + 4 + after;
        let out = self.image.bytes(lo, hi - lo);
        let rw = if gd { Rewrite::TlsGdToLe } else { Rewrite::TlsLdToLe };
        if out == Some(&want[..]) {
            return SeqX::Relaxed(rw, lo, hi);
        }
        if self.dynamic && gd && before == 4 && after == 8 {
            // GNU ld's initial-exec form of a 16-byte GD sequence:
            // `mov %fs:0, %rax; add G(%rip), %rax`, G holding T.
            if let Some(o) = out
                && o[..12] == [0x64, 0x48, 0x8b, 0x04, 0x25, 0, 0, 0, 0, 0x48, 0x03, 0x05]
            {
                let rel = i32::from_le_bytes([o[12], o[13], o[14], o[15]]);
                let g = hi.wrapping_add_signed(i64::from(rel));
                if self.slot_ok(g, Want::Word(t)) {
                    return SeqX::Relaxed(Rewrite::TlsGdToIe, lo, hi);
                }
            }
        }
        // Kept for the loader: the input's bytes, the `lea` reaching a GOT
        // pair that holds the module index and the DTP offset.
        // The bytes another record of the sequence relocates (the call's
        // displacement) are that record's to judge (WP-121: a kept GD or
        // LD sequence, which a shared object keeps, D-119).
        let machine = self.objs[oi].elf.machine();
        let others: Vec<(u64, u64)> = recs
            .iter()
            .filter(|r| {
                r.off != lead.off
                    && r.off + 8 > lead.off.saturating_sub(before)
                    && r.off < lead.off + 4 + after
            })
            .map(|r| (r.off, r.off + u64::from(crate::reloc::width(machine, r.def.r_type))))
            .collect();
        let kept = |o: &[u8], i: &[u8]| {
            (0..o.len()).all(|k| {
                let off = lead.off.saturating_sub(before) + k as u64;
                (before as usize..before as usize + 4).contains(&k)
                    || others.iter().any(|&(a, z)| a <= off && off < z)
                    || o[k] == i[k]
            })
        };
        if self.dynamic
            && let (Some(o), Some(i)) = (out, at(-(before as i64), (hi - lo) as usize))
            && kept(o, i)
        {
            let rel = i32::from_le_bytes([
                o[before as usize],
                o[before as usize + 1],
                o[before as usize + 2],
                o[before as usize + 3],
            ]);
            let g = (lead.p + 4).wrapping_add_signed(i64::from(rel));
            let name = self.sym_name(oi, lead.sym);
            let module = self.dyn_tls_mod.get(&name).copied().unwrap_or(1);
            let off = if gd { t.wrapping_sub(b.tcb) } else { 0 };
            if self.slot_ok(g, Want::Pair(module, off)) {
                return SeqX::Kept(module, off);
            }
            return SeqX::Bad(
                format!(
                    "the kept {what} sequence reaches no GOT pair holding module {module} and offset {off:#x}"
                ),
                Vec::new(),
                None,
            );
        }
        let n = want.len();
        if f == Format::Lld && gd {
            // spec/x64.md W4: lld writes the 16-byte form from P - 4 over a
            // 22-byte sequence, and that write may reach the sequence
            // before it.
            let w4 = before == 3
                && self.image.bytes(lead.p - 4, 16) == Some(&x::gd_le(t as u32, false)[..]);
            let next =
                find_at(recs, sorted, Some(lead.off + 4 + after + 4), |r| r.def.r_type == x::TLSGD)
                    .or_else(|| {
                        find_at(recs, sorted, Some(lead.off + 4 + after + 3), |r| {
                            r.def.r_type == x::TLSGD
                        })
                    });
            let clipped = next.is_some_and(|r2| {
                let k = (r2.p - 4).saturating_sub(lo) as usize;
                k < n && out.is_some_and(|o| o[..k] == want[..k])
            });
            if w4 || clipped {
                return SeqX::Bad(
                    format!(
                        "lld's 16-byte local-exec form written from P - 4 over a 22-byte general-dynamic sequence{} (spec/x64.md W4)",
                        if w4 { "" } else { " overwrote this sequence's end" }
                    ),
                    want,
                    Some((lo - u64::from(w4), hi)),
                );
            }
        }
        SeqX::Bad(
            format!(
                "the {n}-byte {what} sequence is not the local-exec form {} writes (spec/plumb.md §5.9)",
                linker_name(f)
            ),
            want,
            Some((lo, hi)),
        )
    }

    /// T, the TP offset a relaxed GD, IE or descriptor sequence of
    /// `rec`'s symbol loads (X-18, X-23, X-25, X-47), per the map's linker.
    fn x64_tls_value(&self, oi: usize, rec: &Rec, b: TlsBlock) -> Result<u64, NoT> {
        match self.resolve_uncached(oi, rec.sym) {
            Target::Tls(s) => {
                let name = self.sym_name(oi, rec.sym);
                if name == TLS_MODULE_BASE && rec.def.r_type == x::GOTPC32_TLSDESC {
                    // X-47: 0, or mold's TpRel(tls.start).
                    return Ok(if self.format == Format::Mold { b.tprel(b.start) } else { 0 });
                }
                Ok(b.tprel(s))
            }
            // X-18: lld's and mortar's 0; GNU ld's and mold's TpRel(0).
            Target::UndefWeak => Ok(match self.format {
                Format::Gnu | Format::Mold => b.tprel(0),
                _ => 0,
            }),
            Target::Skip(r) => Err(NoT::Unknown(r)),
            _ => Err(NoT::Bad(
                "a thread-local type against a symbol that is not thread-local".into(),
            )),
        }
    }

    /// `GOTTPOFF`, `GOTPC32_TLSDESC`, `TLSDESC_CALL`, `TPOFF32`/`64`.
    #[allow(clippy::too_many_lines)]
    fn x64_tls(&mut self, oi: usize, si: usize, rec: &Rec, site: Site, symbol: String) {
        let ty = rec.def.r_type;
        let b = match self.tls {
            Ok(Some(b)) => b,
            Ok(None) => {
                let f = Self::finding(
                    Rule::NoTls,
                    rec,
                    &site,
                    &symbol,
                    "a thread-local relocation in an output without PT_TLS",
                );
                return self.fail(f);
            }
            Err(_) => return self.x64_unjudged(rec, Reason::Tls),
        };
        if ty == x::TLSDESC_CALL {
            // T2: `call *(%rax)` -> `xchg %ax,%ax`.
            let Some((inp, out)) = self.x64_window(oi, si, rec, 0, 2) else {
                return self.not_checked(Reason::TlsSequence, 1);
            };
            if inp != [0xff, 0x10] {
                let f = Self::finding(
                    Rule::Insn,
                    rec,
                    &site,
                    &symbol,
                    "TLSDESC_CALL does not mark a `call *(%rax)`",
                );
                return self.fail(f);
            }
            if out == [0x66, 0x90] {
                self.x64_rewrote(rec.p, rec.p + 2);
                return self.cov.add_accepted(Rewrite::TlsDescToLe, ty);
            }
            if self.dynamic && out == inp {
                // Kept: the descriptor is judged at its GOTPC32_TLSDESC.
                return self.cov.add_verified(Via::Direct, ty);
            }
            let mut f = Self::finding(
                Rule::Field,
                rec,
                &site,
                &symbol,
                "a TLS descriptor call in a static output is not `66 90` (spec/x64.md X-28)",
            );
            f.expected = Some(0x9066);
            return self.fail(f);
        }
        let t = match self.x64_tls_value(oi, rec, b) {
            Ok(t) => t,
            Err(NoT::Bad(m)) => {
                let f = Self::finding(Rule::Insn, rec, &site, &symbol, m);
                return self.fail(f);
            }
            Err(NoT::Unknown(r)) => return self.x64_unjudged(rec, r),
        };
        if rec.def.op == Op::TpRel {
            // TPOFF32/TPOFF64: TpRel(S + A); an undefined weak's (X-18).
            let xv = t.wrapping_add(rec.addend as u64);
            return self.x64_judge(rec, &site, &symbol, xv as i64);
        }
        let Some((inp, out)) = self.x64_window(oi, si, rec, 3, 0) else {
            return self.not_checked(Reason::TlsSequence, 1);
        };
        let (inp, out) = (inp.clone(), out.to_vec());
        let (rex, op, modrm) = (inp[0], inp[1], inp[2]);
        let reg = rip_reg(modrm).filter(|_| rex & 0xf0 == 0x40 && rec.addend == -4);
        let f = self.format;
        let tb = (t as u32).to_le_bytes();
        let mut cands: Vec<Vec<u8>> = Vec::new();
        if let Some(r) = reg {
            let p = rex_rb(rex);
            match (ty, op) {
                // T1, I1: `mov $T, %reg`. mold relaxes a symbol's `mov`
                // references only when none of its references is an `add`
                // (which it never relaxes): otherwise the symbol has a GOT
                // slot and every reference keeps it (observed, §10.1).
                (x::GOTPC32_TLSDESC, 0x8d) | (x::GOTTPOFF, 0x8b) => {
                    cands.push([&[p][..], &imm_form(0x8b, r).unwrap(), &tb].concat());
                }
                // I2: mortar's `add $T`; GNU ld's and lld's `lea T(%reg)`
                // (`add $T` for %rsp and %r12, which need a SIB).
                (x::GOTTPOFF, 0x03) => match f {
                    Format::Mortar => {
                        cands.push([&[p][..], &imm_form(0x03, r).unwrap(), &tb].concat());
                    }
                    Format::Gnu | Format::Lld if r == 4 => {
                        cands.push([&[p][..], &imm_form(0x03, r).unwrap(), &tb].concat());
                    }
                    Format::Gnu | Format::Lld => {
                        let both = rex | ((rex >> 2) & 1);
                        cands.push([&[both, 0x8d, 0x80 | r << 3 | r][..], &tb].concat());
                    }
                    Format::Mold => {}
                },
                _ => {}
            }
        }
        if cands.contains(&out) {
            self.x64_rewrote(rec.p - 3, rec.p + 4);
            let rw = if ty == x::GOTTPOFF { Rewrite::TlsIeToLe } else { Rewrite::TlsDescToLe };
            return self.cov.add_accepted(rw, ty);
        }
        if ty == x::GOTPC32_TLSDESC && self.dynamic && reg.is_some() && crate::x64::rex_w(rex) {
            // Kept (the descriptor pair holds the resolver and T), or
            // relaxed to initial-exec, `mov G(%rip), %reg` (GNU ld, lld,
            // mold), G holding T. The psABI's TLSDESC -> IE rewrites the
            // `lea` into a `mov` with the REX prefix and the ModRM kept, so
            // the destination is any of the sixteen registers (WP-082: GCC
            // 14 schedules `__once_call`'s into %rbp; GNU ld, lld and mold
            // write `48 8b 2d` and the programs run).
            let ie = out[0] == inp[0] && out[1] == 0x8b && out[2] == inp[2];
            // Kept: the instruction's three bytes are the input's (the
            // window ends with the field, which the link writes; X-61,
            // WP-144: comparing it too made every kept descriptor R1).
            let kept = out[..3] == inp[..3];
            if kept || ie {
                let Some(ev) = self.slot_evidence(rec) else {
                    return self.x64_unjudged(rec, Reason::SymbolUnresolved);
                };
                // A descriptor the loader resolves names the module's
                // block: `_TLS_MODULE_BASE_`'s is its start (`r_sym` 0,
                // addend 0; X-61), not X-47's relaxed `T = 0`.
                let t = if self.sym_name(oi, rec.sym) == TLS_MODULE_BASE {
                    b.tprel(b.start)
                } else {
                    t
                };
                let (slot, want) = if ie {
                    self.x64_rewrote(rec.p - 3, rec.p);
                    (Slot::TpRel, Want::Word(t))
                } else {
                    (Slot::Desc, Want::Pair(crate::loader::TLSDESC_MARK, t))
                };
                self.demand(want);
                let pend = Pending { rec: rec.clone(), site, symbol, want };
                self.got_groups.entry((slot, oi, rec.sym, 0)).or_default().members.push((pend, ev));
                return;
            }
        }
        if ty == x::GOTPC32_TLSDESC {
            let mut f = Self::finding(
                Rule::Field,
                rec,
                &site,
                &symbol,
                format!(
                    "a TLS descriptor is not the local-exec form {} writes (spec/x64.md X-28), nor, in a dynamic output, kept or the initial-exec `mov` (X-58)",
                    linker_name(f)
                ),
            );
            f.expected = cands.first().map(|c| le(c));
            f.found = Some(le(&out));
            return self.fail(f);
        }
        // GOTTPOFF kept: a GOT slot that holds T (X-24).
        if inp[..3] != out[..3] {
            let mut f = Self::finding(
                Rule::Field,
                rec,
                &site,
                &symbol,
                format!(
                    "the initial-exec instruction is neither the input's nor a local-exec form {} writes (spec/plumb.md §5.9)",
                    linker_name(f)
                ),
            );
            f.expected = cands.first().map(|c| le(c));
            f.found = Some(le(&out));
            return self.fail(f);
        }
        self.demand(Want::Word(t));
        let Some(ev) = self.slot_evidence(rec) else {
            return self.x64_unjudged(rec, Reason::SymbolUnresolved);
        };
        let pend = Pending { rec: rec.clone(), site, symbol, want: Want::Word(t) };
        self.got_groups.entry((Slot::TpRel, oi, rec.sym, 0)).or_default().members.push((pend, ev));
    }

    /// `DTPOFF32`/`DTPOFF64` (X-46): `TpRel` or `DtpRel` by section and linker.
    pub(super) fn x64_dtprel(
        &mut self,
        oi: usize,
        rec: &Rec,
        site: &Site,
        symbol: &str,
        alloc: bool,
        exec: bool,
    ) {
        let b = match self.tls {
            Ok(Some(b)) => b,
            Ok(None) => {
                let f = Self::finding(
                    Rule::NoTls,
                    rec,
                    site,
                    symbol,
                    "a thread-local relocation in an output without PT_TLS",
                );
                return self.fail(f);
            }
            Err(_) => return self.not_checked(Reason::Tls, 1),
        };
        let (t, a) = self.resolve_at(oi, rec.sym, rec.addend);
        let s = match t {
            Target::Tls(s) => s.wrapping_add(a as u64),
            Target::UndefWeak => {
                // X-18: A; the references' choices are not specified.
                let a = rec.addend;
                if self.format == Format::Mortar || matches!(judge(rec, a), Verdict::Ok(_)) {
                    return self.x64_judge(rec, site, symbol, a);
                }
                return self.not_checked(Reason::Tls, 1);
            }
            Target::Skip(r) => return self.not_checked(r, 1),
            _ => {
                let f = Self::finding(
                    Rule::Insn,
                    rec,
                    site,
                    symbol,
                    "a thread-local relocation against a symbol that is not thread-local",
                );
                return self.fail(f);
            }
        };
        let tp = match self.format {
            _ if !alloc || self.shared => false,
            Format::Mortar | Format::Gnu => exec,
            Format::Lld => true,
            Format::Mold => false,
        };
        let xv = if tp { b.tprel(s) } else { b.dtprel(s) };
        self.x64_judge(rec, site, symbol, xv as i64);
    }
}

pub(super) fn linker_name(f: Format) -> &'static str {
    match f {
        Format::Mortar => "mortar",
        Format::Gnu => "GNU ld",
        Format::Lld => "lld",
        Format::Mold => "mold",
    }
}

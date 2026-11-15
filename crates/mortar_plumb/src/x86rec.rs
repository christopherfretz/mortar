//! i386 records (`spec/plumb.md` §4.5, §5.11): the value of every type of
//! `spec/x86.md` §6.1, X86-13's operand readings of `GOT32`/`GOT32X`, and
//! the rewritings of §8 and §9 (G1–G5, I1–I5, D1–D3, L1–L2, T1–T2) in
//! the form the map's linker writes them (§5.11's table), matched on the
//! bytes of the record's window in the input section and in the output.
//!
//! Values are computed modulo 2^32 (`spec/x86.md` §3.1): a 32-bit field
//! takes the low 32 bits and a narrow check applies to the 32-bit reduced
//! value read as two's complement; `TLS_LE` and `SIZE32` are checked as
//! computed (X86-38, X86-12). The addend is the input field (X86-4,
//! read in `Engine::section`).

use mortar_elf::SymbolIndex;
use mortar_elf::consts::{ET_EXEC, PT_DYNAMIC, PT_INTERP, SHF_EXECINSTR};

use super::{Engine, Pending, Rec, Target, TlsBlock, Want};
use crate::a64::{Check, Def, Op, Slot};
use crate::map::Format;
use crate::report::{Reason, Rewrite, Rule, Site};
use crate::x86::{self as x};
use crate::x86op::{self, ModRm, imm_form, le};

use super::x64rec::linker_name;

/// The i386 TLS runtime entry (three underscores: the GNU `%eax`
/// convention, `spec/x86.md` §9).
const TLS_GET_ADDR: &str = "___tls_get_addr";
const TLS_MODULE_BASE: &str = "_TLS_MODULE_BASE_";

/// What a GOT-indirect reference's symbol is (`spec/x86.md` §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Defined in a placed section of the output.
    Local,
    /// `SHN_ABS`.
    Abs,
    /// Undefined weak: S = 0.
    Weak0,
}

/// A TLS sequence's verdict.
enum SeqX {
    /// Rewritten to local-exec over `[lo, hi)`.
    Relaxed(Rewrite, u64, u64),
    /// Not a form the linker writes: the message, the bytes expected, and
    /// the output bytes the finding covers (for S12).
    Bad(String, Vec<u8>, Option<(u64, u64)>),
    /// Not judged.
    Unknown(Reason),
}

/// Why T of a TLS reference is not known.
enum NoT {
    Unknown(Reason),
    Bad(String),
}

/// X86-13's reading of a `GOT32`/`GOT32X` operand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operand {
    /// A base register: `G + A − GOT`.
    Base,
    /// No base register: `G + A`.
    None,
    /// The bytes do not settle it (the `ModRM` and `SIB` readings differ),
    /// or there is no instruction to read: refused by name.
    Refused(&'static str),
}

/// X86-13: the operand of a `GOT32`/`GOT32X` whose field starts at
/// section offset `off` of `data`, in a section that is executable or
/// not.
fn operand(data: &[u8], off: u64, exec: bool) -> Operand {
    if !exec {
        return Operand::Refused(
            "a GOT32 or GOT32X outside an executable section has no instruction to read (spec/x86.md X-13 step 1)",
        );
    }
    let Some(b1) =
        usize::try_from(off).ok().and_then(|o| o.checked_sub(1)).and_then(|o| data.get(o))
    else {
        return Operand::Refused(
            "a GOT32 or GOT32X at section offset 0 has no ModRM (spec/x86.md X-13 step 2)",
        );
    };
    let by_modrm = if x86op::disp32_reg(*b1).is_some() { Operand::None } else { Operand::Base };
    let b2 = usize::try_from(off).ok().and_then(|o| o.checked_sub(2)).and_then(|o| data.get(o));
    if let Some(&b2) = b2 {
        let m = ModRm::of(b2);
        if m.rm == 4 && (m.md == 0 || m.md == 2) {
            let by_sib = if m.md == 0 && b1 & 7 == 5 { Operand::None } else { Operand::Base };
            if by_sib != by_modrm {
                return Operand::Refused(
                    "the GOT32/GOT32X operand reads as a based ModRM and as a SIB with a different base (spec/x86.md X-13 step 3, W1)",
                );
            }
        }
    }
    by_modrm
}

/// The value to check: reduced modulo 2^32 and read as two's complement,
/// except for the checks that apply before the reduction (§3.1).
fn reduce(def: &Def, xv: i64) -> i64 {
    match def.check {
        Check::Signed(32) | Check::Unsigned(32) => xv,
        _ => i64::from(xv as i32),
    }
}

impl Engine<'_, '_> {
    /// Whether the output is a static executable (no `PT_DYNAMIC`, no
    /// `PT_INTERP`, `ET_EXEC`).
    fn x86_static_exec(&self) -> bool {
        self.image.header.e_type == ET_EXEC
            && !self.image.phdrs.iter().any(|p| matches!(p.p_type, PT_DYNAMIC | PT_INTERP))
    }

    /// Whether symbol `idx` of object `oi` is named `name`.
    fn x86_named(&self, oi: usize, idx: u32, name: &str) -> bool {
        self.objs[oi]
            .symtab
            .and_then(|t| t.get(SymbolIndex(idx)).ok().map(|s| (t, s)))
            .is_some_and(|(t, s)| t.name(&s).is_ok_and(|n| n == name.as_bytes()))
    }

    /// The input bytes `[off + k, off + k + n)` of section `si`.
    fn x86_in(&self, oi: usize, si: usize, off: u64, k: i64, n: usize) -> Option<&[u8]> {
        let data = self.objs[oi].secs[si].data;
        let lo = usize::try_from((off as i64).checked_add(k)?).ok()?;
        data.get(lo..lo.checked_add(n)?)
    }

    /// One i386 record (§5.1 steps 2 to 7).
    #[allow(clippy::too_many_arguments, clippy::too_many_lines, clippy::many_single_char_names)]
    pub(super) fn x86_record(
        &mut self,
        oi: usize,
        si: usize,
        recs: &[Rec],
        i: usize,
        site: Site,
        symbol: String,
    ) {
        let rec = &recs[i];
        let ty = rec.def.r_type;
        let sec = &self.objs[oi].secs[si];
        let alloc = sec.alloc();
        let exec = sec.shdr.sh_flags & SHF_EXECINSTR != 0;
        // The call of a GD or LD sequence: the lead's verdict (X86-31).
        if symbol == TLS_GET_ADDR
            && alloc
            && i > 0
            && let Some(lead) = Self::x86_lead_of(recs, i)
        {
            return match self.x86_seq(oi, si, recs, i - 1, lead) {
                SeqX::Relaxed(rw, ..) => self.cov.add_accepted(rw, ty),
                SeqX::Bad(..) => self.not_checked(Reason::TlsSequence, 1),
                SeqX::Unknown(r) => self.not_checked(r, 1),
            };
        }
        // X86-39 (mortar): no TLS runtime call survives in a static output.
        if symbol == TLS_GET_ADDR
            && alloc
            && exec
            && self.format == Format::Mortar
            && self.x86_static_exec()
            && matches!(ty, x::PLT32 | x::PC32 | x::GOT32 | x::GOT32X)
        {
            let f = Self::finding(
                Rule::Insn,
                rec,
                &site,
                &symbol,
                "a call to ___tls_get_addr survives in a static output (spec/x86.md X86-39)",
            );
            return self.fail(f);
        }
        match rec.def.op {
            Op::Slot(Slot::Gd | Slot::Ld, _) => {
                return match self.x86_seq(oi, si, recs, i, rec) {
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
                    SeqX::Unknown(r) => self.x86_unjudged(rec, r),
                };
            }
            Op::Slot(Slot::Desc | Slot::TpRel, _) | Op::DescCall | Op::TpRel => {
                return self.x86_tls(oi, si, rec, site, symbol);
            }
            Op::DtpRel => return self.x64_dtprel(oi, rec, &site, &symbol, alloc, exec),
            Op::None => {
                self.cov.none += 1;
                return;
            }
            // X86-30: GOTPC ignores its symbol.
            Op::GotPc => {
                let Some(g) = self.got else {
                    return self.not_checked(Reason::SymbolUnresolved, 1);
                };
                let xv = g.wrapping_add(rec.addend as u64).wrapping_sub(rec.p) as i64;
                return self.x86_judge(rec, &site, &symbol, xv);
            }
            _ => {}
        }
        // X86-13: the operand of a GOT32/GOT32X decides its value.
        let nobase;
        let rec = if matches!(ty, x::GOT32 | x::GOT32X) {
            match operand(self.objs[oi].secs[si].data, rec.off, exec) {
                Operand::Refused(why) => {
                    self.x64_rewrote(rec.p.wrapping_sub(2), rec.p + 4);
                    let mut f = Self::finding(Rule::Insn, rec, &site, &symbol, why);
                    f.found = Some(rec.out_word);
                    return self.fail(f);
                }
                Operand::Base => rec,
                Operand::None => {
                    let mut r = rec.clone();
                    r.def = if ty == x::GOT32 { &x::GOT32_NOBASE } else { &x::GOT32X_NOBASE };
                    nobase = r;
                    &nobase
                }
            }
        } else {
            rec
        };
        let (target, addend) = self.resolve_at(oi, rec.sym, rec.addend);
        let (s, kind) = match target {
            Target::Skip(r) => return self.x86_unjudged(rec, r),
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
            Target::Outside(_) => {
                if self.format != Format::Mortar {
                    return self.x86_unjudged(rec, Reason::PieceNotLocatable);
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
            // A GOT slot of a merged piece is not located (§5.6); its
            // instruction may still be rewritten.
            Target::Merge { .. } if rec.def.op.is_slot() => {
                return self.x86_unjudged(rec, Reason::PieceNotLocatable);
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
                    &|_, _| None,
                );
            }
            Target::Tls(s) => {
                // X86-62: `32` against a TLS symbol at a non-alloc place is
                // DtpRel(S + A); in alloc memory it is not a TLS type
                // (X86-17).
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
            Op::Size => match self.x64_size(oi, rec.sym) {
                Some(z) => z.wrapping_add(a),
                None => return self.not_checked(Reason::SymbolUnresolved, 1),
            },
            Op::Slot(slot, _) => {
                if alloc && let Some(rw) = self.x86_got_form(oi, si, rec, s, kind) {
                    return self.cov.add_accepted(rw, ty);
                }
                // The instruction is the input's: the two bytes before
                // the field are unchanged (X86-40's read window).
                let k = rec.off.min(2);
                let kept = self
                    .x64_window(oi, si, rec, k, 0)
                    .is_none_or(|(i, o)| i[..k as usize] == o[..k as usize]);
                if !kept {
                    let mut f = Self::finding(
                        Rule::Field,
                        rec,
                        &site,
                        &symbol,
                        format!(
                            "the instruction is neither the GOT-indirect one of the input nor a rewriting {} writes (spec/plumb.md §5.11)",
                            linker_name(self.format)
                        ),
                    );
                    f.found = Some(rec.out_word);
                    return self.fail(f);
                }
                let Some(ev) = self.slot_evidence(rec) else {
                    return self.x86_unjudged(rec, Reason::SymbolUnresolved);
                };
                let pend =
                    Pending { rec: rec.clone(), site, symbol, want: Want::Word(s), w1: false };
                self.got_groups.entry((slot, oi, rec.sym, 0)).or_default().members.push((pend, ev));
                return;
            }
            _ => return self.not_checked(Reason::Type, 1),
        };
        self.x86_judge(rec, &site, &symbol, xv as i64);
    }

    /// R1–R3 for a data value reduced per §3.1.
    fn x86_judge(&mut self, rec: &Rec, site: &Site, symbol: &str, xv: i64) {
        self.x64_judge(rec, site, symbol, reduce(rec.def, xv));
    }

    /// The bytes a rewriting of an i386 record may change (§5.11): S12
    /// does not compare them when the record is not judged.
    fn x86_unjudged(&mut self, rec: &Rec, r: Reason) {
        let span = match rec.def.r_type {
            x::GOT32X | x::TLS_IE | x::TLS_GOTIE | x::TLS_GOTDESC => Some((2, 4)),
            x::TLS_DESC_CALL => Some((0, 2)),
            x::TLS_GD | x::TLS_LDM => Some((3, 10)),
            _ => None,
        };
        if let Some((b, a)) = span {
            self.x64_rewrote(rec.p.wrapping_sub(b), rec.p + a);
        }
        self.not_checked(r, 1);
    }

    /// The `GOT32X` rewritings of `spec/x86.md` §8 the map's linker
    /// writes, for a symbol of `kind` at `s`: the one the output holds.
    /// GNU ld (and mortar) in a static executable: G1–G5 (X86-25); GNU ld
    /// in static-pie: G1 as `lea`, G4, G5 (X86-27's measurement); mold:
    /// G1 as `lea` with a base register (X86-27). lld rewrites none.
    #[allow(clippy::many_single_char_names)]
    fn x86_got_form(
        &mut self,
        oi: usize,
        si: usize,
        rec: &Rec,
        s: u64,
        kind: Kind,
    ) -> Option<Rewrite> {
        if rec.def.r_type != x::GOT32X || rec.addend != 0 || rec.off < 2 {
            return None;
        }
        let (inp, out) = self.x64_window(oi, si, rec, 2, 0)?;
        let (op, modrm) = (inp[0], inp[1]);
        let out = out.to_vec();
        let m = ModRm::of(modrm);
        let nobase = m.disp32_only();
        // X86-25: mod 00 rm 101 (none) or mod 10 rm ≠ 100 (base).
        if !(nobase || (m.md == 2 && m.rm != 4)) {
            return None;
        }
        let f = self.format;
        let static_exec = self.x86_static_exec();
        let defined = kind != Kind::Weak0;
        let p = rec.p;
        let mut cands: Vec<(Rewrite, Vec<u8>)> = Vec::new();
        let mut push = |rw, head: &[u8], field: u64, tail: &[u8]| {
            let mut v = head.to_vec();
            v.extend_from_slice(&(field as u32).to_le_bytes());
            v.extend_from_slice(tail);
            cands.push((rw, v));
        };
        let gnu_static = static_exec && matches!(f, Format::Gnu | Format::Mortar);
        let gnu_pie = !static_exec && f == Format::Gnu;
        if gnu_static {
            // G1–G3: an immediate S (0 for weak0).
            if let Some(head) = imm_form(op, m.reg) {
                let rw = if op == 0x8b { Rewrite::GotToMovImm } else { Rewrite::GotToImm };
                push(rw, &head, s, &[]);
            }
        }
        if (gnu_static || gnu_pie) && op == 0xff && defined {
            // G4: `67 e8 rel32`; G5: `e9 rel32 90` from P − 2.
            if m.reg == 2 {
                push(Rewrite::GotToDirect, &[0x67, 0xe8], s.wrapping_sub(p + 4), &[]);
            } else if m.reg == 4 {
                push(Rewrite::GotToDirect, &[0xe9], s.wrapping_sub(p + 3), &[0x90]);
            }
        }
        // G1 as `lea S − GOT(%reg)`: GNU ld in static-pie for a symbol
        // whose address moves with the image; mold in both, for any
        // symbol, which in static-pie is wrong for an absolute or
        // undefined weak one (W16): not accepted there.
        let lea_ok = match f {
            Format::Gnu => gnu_pie && kind == Kind::Local,
            Format::Mold => static_exec || kind == Kind::Local,
            _ => false,
        };
        if lea_ok
            && op == 0x8b
            && !nobase
            && let Some(g) = self.got
        {
            push(Rewrite::GotToLea, &[0x8d, modrm], s.wrapping_sub(g), &[]);
        }
        let (rw, _) = cands.into_iter().find(|(_, v)| *v == out)?;
        self.x64_rewrote(p - 2, p + 4);
        Some(rw)
    }

    /// The lead of the sequence whose call `recs[i]` is (X86-31): the
    /// record with the previous index, a `TLS_GD`/`TLS_LDM` 5 or 6 bytes
    /// before it.
    fn x86_lead_of(recs: &[Rec], i: usize) -> Option<&Rec> {
        let lead = recs.get(i.checked_sub(1)?)?;
        let d = recs[i].off.checked_sub(lead.off)?;
        (matches!(lead.def.r_type, x::TLS_GD | x::TLS_LDM) && (d == 5 || d == 6)).then_some(lead)
    }

    /// T, the TP offset a relaxed GD, IE or descriptor sequence of
    /// `rec`'s symbol loads (X86-23, X86-63), per the map's linker.
    fn x86_tls_value(&self, oi: usize, rec: &Rec, b: TlsBlock) -> Result<u64, NoT> {
        match self.resolve_uncached(oi, rec.sym) {
            Target::Tls(s) => {
                if rec.def.r_type == x::TLS_GOTDESC && self.x86_named(oi, rec.sym, TLS_MODULE_BASE)
                {
                    // X86-63: 0, or mold's TpRel(tls.start).
                    return Ok(if self.format == Format::Mold { b.tprel(b.start) } else { 0 });
                }
                Ok(b.tprel(s))
            }
            // X86-23: lld's and mortar's 0; GNU ld's TpRel(0).
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

    /// A `TLS_GD`/`TLS_LDM` sequence led by `recs[li]` (X86-31, X86-34,
    /// X86-35): the input form, and the output against the LE forms the
    /// map's linker writes.
    #[allow(clippy::too_many_lines)]
    fn x86_seq(&self, oi: usize, si: usize, recs: &[Rec], li: usize, lead: &Rec) -> SeqX {
        let gd = lead.def.r_type == x::TLS_GD;
        let what = if gd { "general-dynamic" } else { "local-dynamic" };
        let unrec = || {
            SeqX::Bad(
                format!("an unrecognized {what} TLS sequence (spec/x86.md X86-36)"),
                Vec::new(),
                None,
            )
        };
        let b = match self.tls {
            Ok(Some(b)) => b,
            Ok(None) => {
                return SeqX::Bad(
                    "a TLS sequence in an output without PT_TLS".into(),
                    Vec::new(),
                    None,
                );
            }
            Err(_) => return SeqX::Unknown(Reason::Tls),
        };
        let f = self.format;
        let mold = f == Format::Mold;
        let at = |k: i64, n: usize| self.x86_in(oi, si, lead.off, k, n);
        // The call: the record with the next index (X86-31).
        let call = recs.get(li + 1).filter(|c| self.x86_named(oi, c.sym, TLS_GET_ADDR));
        let call_at = |d: u64, types: &[u32], a: i64| {
            call.is_some_and(|c| {
                c.off == lead.off + d && types.contains(&c.def.r_type) && c.addend == a
            })
        };
        let direct = |d| call_at(d, &[x::PLT32, x::PC32], -4);
        let indirect = || {
            call_at(6, &[x::GOT32, x::GOT32X], 0)
                && at(4, 2).is_some_and(|v| v[0] == 0xff && v[1] & 0xf8 == 0x90 && v[1] & 7 != 4)
        };
        // The lead's operand: `8d` ModRM `10 000 rm`, rm ≠ 100.
        let based = at(-2, 2).is_some_and(|v| v[0] == 0x8d && v[1] & 0xf8 == 0x80 && v[1] & 7 != 4);
        let t = if gd {
            match self.x86_tls_value(oi, lead, b) {
                Ok(v) => v,
                Err(NoT::Bad(m)) => return SeqX::Bad(m, Vec::new(), None),
                Err(NoT::Unknown(r)) => return SeqX::Unknown(r),
            }
        } else {
            0
        };
        let size = b.tcb.wrapping_neg() as u32;
        // (before, after, the LE bytes).
        let (before, after, want): (u64, u64, Vec<u8>) = if gd {
            if lead.addend != 0 {
                return unrec();
            }
            let le = x::gd_le(t as u32, mold).to_vec();
            if at(-3, 3) == Some(&[0x8d, 0x04, 0x1d]) && at(4, 1) == Some(&[0xe8]) && direct(5) {
                // D1.
                (3, 5, le)
            } else if based && indirect() {
                // D2.
                (2, 6, le)
            } else if based && at(4, 1) == Some(&[0xe8]) && direct(5) && at(9, 1) == Some(&[0x90]) {
                // D3. mold writes its 12 bytes from P − 3 (W4).
                if mold {
                    let lo = lead.p - 3;
                    if self.image.bytes(lo, 12) == Some(&le[..]) {
                        return SeqX::Bad(
                            "mold's local-exec form written from P − 3 over a based general-dynamic sequence with a direct call: the previous instruction's last byte is overwritten (spec/x86.md W4)".into(),
                            le,
                            Some((lo, lead.p + 10)),
                        );
                    }
                }
                (2, 6, le)
            } else {
                return unrec();
            }
        } else if !based {
            return unrec();
        } else if at(4, 1) == Some(&[0xe8]) && direct(5) {
            // L1.
            let v = if mold {
                [&x::MOV_GS0_EAX[..], &[0x2d], &size.to_le_bytes()].concat()
            } else {
                [&x::MOV_GS0_EAX[..], &x::NOP5_L1].concat()
            };
            (2, 5, v)
        } else if indirect() {
            // L2.
            let v = if mold {
                [&x::MOV_GS0_EAX[..], &[0x2d], &size.to_le_bytes(), &[0x90]].concat()
            } else {
                [&x::MOV_GS0_EAX[..], &x::NOP6_L2].concat()
            };
            (2, 6, v)
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
        SeqX::Bad(
            format!(
                "the {}-byte {what} sequence is not the local-exec form {} writes (spec/plumb.md §5.11)",
                want.len(),
                linker_name(f)
            ),
            want,
            Some((lo, hi)),
        )
    }

    /// `TLS_IE`, `TLS_GOTIE`, `TLS_GOTDESC`, `TLS_DESC_CALL`, `TLS_LE`.
    #[allow(clippy::too_many_lines, clippy::many_single_char_names)]
    fn x86_tls(&mut self, oi: usize, si: usize, rec: &Rec, site: Site, symbol: String) {
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
            Err(_) => return self.x86_unjudged(rec, Reason::Tls),
        };
        if ty == x::TLS_DESC_CALL {
            // T2: `call *(%eax)` -> `xchg %ax,%ax`.
            let Some((inp, out)) = self.x64_window(oi, si, rec, 0, 2) else {
                return self.not_checked(Reason::TlsSequence, 1);
            };
            if inp != [0xff, 0x10] {
                let f = Self::finding(
                    Rule::Insn,
                    rec,
                    &site,
                    &symbol,
                    "TLS_DESC_CALL does not mark a `call *(%eax)` (spec/x86.md X86-36)",
                );
                return self.fail(f);
            }
            if out == [0x66, 0x90] {
                self.x64_rewrote(rec.p, rec.p + 2);
                return self.cov.add_accepted(Rewrite::TlsDescToLe, ty);
            }
            let mut f = Self::finding(
                Rule::Field,
                rec,
                &site,
                &symbol,
                "a TLS descriptor call in a static output is not `66 90` (spec/x86.md X86-37)",
            );
            f.expected = Some(0x9066);
            return self.fail(f);
        }
        let t = match self.x86_tls_value(oi, rec, b) {
            Ok(t) => t,
            Err(NoT::Bad(m)) => {
                let f = Self::finding(Rule::Insn, rec, &site, &symbol, m);
                return self.fail(f);
            }
            Err(NoT::Unknown(r)) => return self.x86_unjudged(rec, r),
        };
        if rec.def.op == Op::TpRel {
            // TLS_LE: TpRel(S + A), s(32) before the reduction (X86-38).
            let xv = t.wrapping_add(rec.addend as u64);
            return self.x64_judge(rec, &site, &symbol, xv as i64);
        }
        // I1 reads one byte before the field, the others two.
        let i1 =
            ty == x::TLS_IE && rec.off >= 1 && self.x86_in(oi, si, rec.off, -1, 1) == Some(&[0xa1]);
        let k: u64 = if i1 { 1 } else { 2 };
        let Some((inp, out)) = self.x64_window(oi, si, rec, k.min(rec.off), 0) else {
            return self.not_checked(Reason::TlsSequence, 1);
        };
        let (inp, out) = (inp.to_vec(), out.to_vec());
        if inp.len() < k as usize + 4 {
            return self.x86_unjudged(rec, Reason::TlsSequence);
        }
        let f = self.format;
        let tb = (t as u32).to_le_bytes();
        let mut cands: Vec<Vec<u8>> = Vec::new();
        let (op, m) = (inp[0], ModRm::of(inp[1]));
        let a0 = rec.addend == 0;
        match ty {
            // I1: `movl x@indntpoff, %eax` -> `movl $T, %eax`.
            x::TLS_IE if i1 && a0 && f != Format::Mold => cands.push([&[0xb8][..], &tb].concat()),
            // I2, I3: `8b`/`03` ModRM `00 reg 101`.
            x::TLS_IE
                if a0 && f != Format::Mold && m.disp32_only() && matches!(op, 0x8b | 0x03) =>
            {
                cands.push([&imm_form(op, m.reg).unwrap()[..], &tb].concat());
            }
            // I4, I5: `8b`/`03` ModRM `10 reg rm`, rm ≠ 100. lld writes
            // I5 as `lea T(%reg), %reg` (X86-32).
            x::TLS_GOTIE if a0 && f != Format::Mold && m.md == 2 && m.rm != 4 => match op {
                0x03 if f == Format::Lld => {
                    cands.push([&[0x8d, 0x80 | m.reg << 3 | m.reg][..], &tb].concat());
                }
                0x8b | 0x03 => cands.push([&imm_form(op, m.reg).unwrap()[..], &tb].concat()),
                _ => {}
            },
            // T1: `8d` ModRM `10 reg rm`, rm ≠ 100 -> `leal T, %reg`;
            // mold's `nop; movl $T, %reg`.
            x::TLS_GOTDESC if op == 0x8d && m.md == 2 && m.rm != 4 => {
                if f == Format::Mold {
                    cands.push([&[0x90, 0xb8 | m.reg][..], &tb].concat());
                } else {
                    cands.push([&[0x8d, 0x05 | m.reg << 3][..], &tb].concat());
                }
            }
            _ => {}
        }
        if cands.contains(&out) {
            self.x64_rewrote(rec.p - k, rec.p + 4);
            let rw = if ty == x::TLS_GOTDESC { Rewrite::TlsDescToLe } else { Rewrite::TlsIeToLe };
            return self.cov.add_accepted(rw, ty);
        }
        if ty == x::TLS_GOTDESC {
            let mut f = Self::finding(
                Rule::Field,
                rec,
                &site,
                &symbol,
                format!(
                    "a TLS descriptor in a static output is not the local-exec form {} writes (spec/x86.md X86-37)",
                    linker_name(f)
                ),
            );
            f.expected = cands.first().map(|c| le(c));
            f.found = Some(le(&out));
            return self.fail(f);
        }
        // IE kept: a GOT slot that holds T (X86-33; mold, X86-32).
        if inp[..k as usize] != out[..k as usize] {
            let mut f = Self::finding(
                Rule::Field,
                rec,
                &site,
                &symbol,
                format!(
                    "the initial-exec instruction is neither the input's nor a local-exec form {} writes (spec/plumb.md §5.11)",
                    linker_name(f)
                ),
            );
            f.expected = cands.first().map(|c| le(c));
            f.found = Some(le(&out));
            return self.fail(f);
        }
        let Some(ev) = self.slot_evidence(rec) else {
            return self.x86_unjudged(rec, Reason::SymbolUnresolved);
        };
        let pend = Pending { rec: rec.clone(), site, symbol, want: Want::Word(t), w1: false };
        self.got_groups.entry((Slot::TpRel, oi, rec.sym, 0)).or_default().members.push((pend, ev));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x86_13_operand_readings() {
        // `8b 83` (base %ebx), `8b 05`/`8b 0d` (none), `ff a3` (base).
        assert_eq!(operand(&[0x8b, 0x83, 0, 0, 0, 0], 2, true), Operand::Base);
        assert_eq!(operand(&[0x8b, 0x0d, 0, 0, 0, 0], 2, true), Operand::None);
        assert_eq!(operand(&[0xff, 0xa3, 0, 0, 0, 0], 2, true), Operand::Base);
        // W1: `8b 84 0d`: the SIB 0d has base %ebp, the ModRM reading none.
        assert!(matches!(operand(&[0x8b, 0x84, 0x0d, 0, 0, 0, 0], 3, true), Operand::Refused(_)));
        // `disp32(,%ecx,1)`: `8b 04 0d`: SIB base 101 with mod 00, ModRM 0d: none twice.
        assert_eq!(operand(&[0x8b, 0x04, 0x0d, 0, 0, 0, 0], 3, true), Operand::None);
        assert!(matches!(operand(&[0, 0, 0, 0], 0, true), Operand::Refused(_)));
        assert!(matches!(operand(&[0x8b, 0x83, 0, 0, 0, 0], 2, false), Operand::Refused(_)));
    }
}

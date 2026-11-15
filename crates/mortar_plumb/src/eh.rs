//! `.eh_frame` and `.eh_frame_hdr` (`spec/plumb.md` §6.4): the CIEs and
//! FDEs of the output, parsed as the LSB ("Exception Frames") and DWARF
//! ("Call Frame Information") define them, against the live FDEs of the
//! inputs; and the search table of `.eh_frame_hdr`.
//!
//! A child of `reloc`: it resolves the inputs' pointer fields with the
//! engine's symbol resolution (§4.4).

use std::collections::BTreeMap;

use mortar_elf::Machine;
use mortar_elf::consts::PT_GNU_EH_FRAME;

use super::{Engine, Target};
use crate::report::{Finding, Reason, Rule, Via};

/// `DW_EH_PE_omit`.
const OMIT: u8 = 0xff;

/// A byte reader; every read is checked.
struct Rd<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Rd<'a> {
    fn new(b: &'a [u8], pos: usize) -> Self {
        Rd { b, pos }
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let s = self.b.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|s| s[0])
    }
    fn uint(&mut self, n: usize) -> Option<u64> {
        let s = self.take(n)?;
        Some(s.iter().rev().fold(0u64, |acc, &x| acc << 8 | u64::from(x)))
    }
    fn sint(&mut self, n: usize) -> Option<i64> {
        let v = self.uint(n)?;
        let shift = 64 - 8 * n as u32;
        Some(((v << shift) as i64) >> shift)
    }
    fn uleb(&mut self) -> Option<u64> {
        let mut v = 0u64;
        for i in 0..10 {
            let b = self.u8()?;
            v |= u64::from(b & 0x7f) << (7 * i);
            if b & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }
    fn sleb(&mut self) -> Option<i64> {
        let mut v = 0i64;
        let mut shift = 0;
        loop {
            let b = self.u8()?;
            if shift < 64 {
                v |= i64::from(b & 0x7f) << shift;
            }
            shift += 7;
            if b & 0x80 == 0 {
                if shift < 64 && b & 0x40 != 0 {
                    v |= -1i64 << shift;
                }
                return Some(v);
            }
            if shift > 70 {
                return None;
            }
        }
    }
    fn cstr(&mut self) -> Option<&'a [u8]> {
        let rest = self.b.get(self.pos..)?;
        let n = rest.iter().position(|&c| c == 0)?;
        self.pos += n + 1;
        Some(&rest[..n])
    }
    /// The raw value of a pointer of encoding `enc` (its format, the low
    /// nibble), sign- or zero-extended.
    fn ptr(&mut self, enc: u8) -> Option<u64> {
        Some(match enc & 0x0f {
            0x00 | 0x04 | 0x0c => self.uint(8)?,
            0x01 => self.uleb()?,
            0x02 => self.uint(2)?,
            0x03 => self.uint(4)?,
            0x09 => self.sleb()? as u64,
            0x0a => self.sint(2)? as u64,
            0x0b => self.sint(4)? as u64,
            _ => return None,
        })
    }
}

/// A pointer field: where it is in its section and its raw value.
#[derive(Debug, Clone, Copy)]
struct Field {
    off: usize,
    raw: u64,
    enc: u8,
}

#[derive(Debug, Clone)]
struct Cie {
    version: u8,
    aug: Vec<u8>,
    code_align: u64,
    data_align: i64,
    ra: u64,
    fde_enc: u8,
    lsda_enc: u8,
    personality: Option<Field>,
    insns: Vec<u8>,
}

#[derive(Debug, Clone)]
struct Fde {
    off: usize,
    cie: usize,
    pc: Field,
    range: u64,
    /// The length of the augmentation data, for a `z` CIE.
    aug_len: Option<u64>,
    lsda: Option<Field>,
    insns: Vec<u8>,
}

#[derive(Debug, Default)]
struct Frames {
    cies: Vec<Cie>,
    fdes: Vec<Fde>,
    /// Offset of a zero terminator, if any.
    terminator: Option<usize>,
}

/// Instructions without their trailing `DW_CFA_nop` padding.
fn strip(b: &[u8]) -> Vec<u8> {
    let n = b.iter().rposition(|&x| x != 0).map_or(0, |i| i + 1);
    b[..n].to_vec()
}

/// Parses a `.eh_frame` (LSB "The `.eh_frame` section"). The error names the
/// first offset that cannot be read.
#[allow(clippy::too_many_lines)] // the record format, field by field
fn parse(b: &[u8]) -> Result<Frames, String> {
    let mut f = Frames::default();
    let mut cie_at: BTreeMap<usize, usize> = BTreeMap::new();
    let mut pos = 0usize;
    let bad = |at: usize, what: &str| format!("offset {at:#x}: {what}");
    while pos < b.len() {
        if f.terminator.is_some() {
            if b[pos..].iter().all(|&x| x == 0) {
                break;
            }
            return Err(bad(pos, "a record after the zero terminator"));
        }
        let mut r = Rd::new(b, pos);
        let Some(len32) = r.uint(4) else {
            if b[pos..].iter().all(|&x| x == 0) {
                break;
            }
            return Err(bad(pos, "a truncated length"));
        };
        if len32 == 0 {
            f.terminator = Some(pos);
            pos += 4;
            continue;
        }
        let (len, idw) = if len32 == 0xffff_ffff {
            (r.uint(8).ok_or_else(|| bad(pos, "a truncated 64-bit length"))?, 8)
        } else {
            (len32, 4)
        };
        let body = r.pos;
        let end = usize::try_from(len)
            .ok()
            .and_then(|l| body.checked_add(l))
            .filter(|&e| e <= b.len())
            .ok_or_else(|| bad(pos, "a record longer than the section"))?;
        let rec = &b[..end];
        let mut r = Rd::new(rec, body);
        let id = r.uint(idw).ok_or_else(|| bad(pos, "a truncated CIE id"))?;
        if id == 0 {
            let version = r.u8().ok_or_else(|| bad(pos, "no CIE version"))?;
            if !matches!(version, 1 | 3) {
                return Err(bad(pos, &format!("CIE version {version}")));
            }
            let aug = r.cstr().ok_or_else(|| bad(pos, "no augmentation string"))?.to_vec();
            let code_align = r.uleb().ok_or_else(|| bad(pos, "no code alignment"))?;
            let data_align = r.sleb().ok_or_else(|| bad(pos, "no data alignment"))?;
            let ra = if version == 1 { r.u8().map(u64::from) } else { r.uleb() }
                .ok_or_else(|| bad(pos, "no return address register"))?;
            let mut cie = Cie {
                version,
                aug: aug.clone(),
                code_align,
                data_align,
                ra,
                fde_enc: 0,
                lsda_enc: OMIT,
                personality: None,
                insns: Vec::new(),
            };
            if aug.first() == Some(&b'z') {
                let alen = r.uleb().ok_or_else(|| bad(pos, "no augmentation length"))?;
                let astart = r.pos;
                for &c in &aug[1..] {
                    match c {
                        b'R' => cie.fde_enc = r.u8().ok_or_else(|| bad(pos, "no R encoding"))?,
                        b'L' => cie.lsda_enc = r.u8().ok_or_else(|| bad(pos, "no L encoding"))?,
                        b'P' => {
                            let enc = r.u8().ok_or_else(|| bad(pos, "no P encoding"))?;
                            let off = r.pos;
                            let raw = r.ptr(enc).ok_or_else(|| bad(pos, "a bad personality"))?;
                            cie.personality = Some(Field { off, raw, enc });
                        }
                        // signal frame; AArch64 pointer authentication with
                        // the B key; MTE-tagged frames (no data).
                        b'S' | b'B' | b'G' => {}
                        _ => {
                            return Err(bad(
                                pos,
                                &format!("augmentation '{}'", String::from_utf8_lossy(&aug)),
                            ));
                        }
                    }
                }
                let aend = usize::try_from(alen)
                    .ok()
                    .and_then(|l| astart.checked_add(l))
                    .filter(|&e| e <= end && e >= r.pos)
                    .ok_or_else(|| bad(pos, "augmentation data longer than stated"))?;
                r.pos = aend;
            } else if !aug.is_empty() {
                return Err(bad(pos, &format!("augmentation '{}'", String::from_utf8_lossy(&aug))));
            }
            cie.insns = strip(&rec[r.pos..end]);
            cie_at.insert(pos, f.cies.len());
            f.cies.push(cie);
        } else {
            let idpos = body;
            let cpos = usize::try_from(id)
                .ok()
                .and_then(|i| idpos.checked_sub(i))
                .ok_or_else(|| bad(pos, "a CIE pointer before the section"))?;
            let &ci = cie_at
                .get(&cpos)
                .ok_or_else(|| bad(pos, &format!("CIE pointer to {cpos:#x}, which is no CIE")))?;
            let cie = &f.cies[ci];
            let off = r.pos;
            let raw = r.ptr(cie.fde_enc).ok_or_else(|| bad(pos, "a bad initial location"))?;
            let pc = Field { off, raw, enc: cie.fde_enc };
            let range = r.ptr(cie.fde_enc & 0x0f).ok_or_else(|| bad(pos, "a bad address range"))?;
            let mut lsda = None;
            let mut aug_len = None;
            if cie.aug.first() == Some(&b'z') {
                let alen = r.uleb().ok_or_else(|| bad(pos, "no augmentation length"))?;
                aug_len = Some(alen);
                let astart = r.pos;
                if cie.lsda_enc != OMIT {
                    let off = r.pos;
                    let raw = r.ptr(cie.lsda_enc).ok_or_else(|| bad(pos, "a bad LSDA pointer"))?;
                    lsda = Some(Field { off, raw, enc: cie.lsda_enc });
                }
                r.pos = usize::try_from(alen)
                    .ok()
                    .and_then(|l| astart.checked_add(l))
                    .filter(|&e| e <= end && e >= r.pos)
                    .ok_or_else(|| bad(pos, "augmentation data longer than stated"))?;
            }
            let insns = strip(&rec[r.pos..end]);
            f.fdes.push(Fde { off: pos, cie: ci, pc, range, aug_len, lsda, insns });
        }
        pos = end;
    }
    Ok(f)
}

/// The value a pointer of the output means (the address of the field is
/// `at`): absolute or PC-relative; the indirection bit is ignored, so a
/// personality is the address of the slot that holds it.
fn decode(enc: u8, raw: u64, at: u64) -> Option<u64> {
    match enc & 0x70 {
        0x00 => Some(raw),
        0x10 => Some(at.wrapping_add(raw)),
        _ => None,
    }
}

/// What an input pointer field resolves to.
enum In {
    Value(u64),
    /// The target's section is not in the output: the FDE is dropped.
    Dead,
    Unknown(Reason),
}

/// One output FDE, decoded.
struct OutFde {
    addr: u64,
    pc: u64,
    range: u64,
    aug_len: Option<u64>,
    lsda: Option<u64>,
    personality: Option<u64>,
    cie: CieKey,
    insns: Vec<u8>,
}

/// What two CIEs must share (the encodings may differ; the values not).
#[derive(Debug, Clone, PartialEq, Eq)]
struct CieKey {
    version: u8,
    aug: Vec<u8>,
    code_align: u64,
    data_align: i64,
    ra: u64,
    insns: Vec<u8>,
    has_lsda: bool,
}

impl CieKey {
    fn of(c: &Cie) -> Self {
        CieKey {
            version: c.version,
            aug: c.aug.clone(),
            code_align: c.code_align,
            data_align: c.data_align,
            ra: c.ra,
            insns: c.insns.clone(),
            has_lsda: c.lsda_enc != OMIT,
        }
    }
}

fn e(n: u8, msg: impl Into<String>) -> Finding {
    Finding::new(Rule::EhFrame(n), msg)
}

impl Engine<'_, '_> {
    /// §6.4: the output's `.eh_frame` against the inputs', then
    /// `.eh_frame_hdr`.
    #[allow(clippy::too_many_lines, clippy::many_single_char_names)] // §6.4 steps 1 to 5
    pub fn check_eh_frame(&mut self) {
        let sections = std::mem::take(&mut self.eh_sections);
        let image = self.image;
        let out_sec = image.sections.iter().find(|s| s.name == ".eh_frame" && s.alloc());
        let out = out_sec.and_then(|s| image.bytes(s.shdr.sh_addr, s.shdr.sh_size).map(|b| (s, b)));
        let parsed = match out {
            Some((s, b)) => match parse(b) {
                Ok(f) => Some((s.shdr.sh_addr, b, f)),
                Err(m) => {
                    self.findings.push(
                        e(1, format!("the output's .eh_frame cannot be read: {m}"))
                            .at(s.shdr.sh_addr),
                    );
                    None
                }
            },
            None => None,
        };
        // The output FDEs, by initial location.
        let mut outs: Vec<OutFde> = Vec::new();
        if let Some((base, _, f)) = &parsed {
            for d in &f.fdes {
                let c = &f.cies[d.cie];
                let at = |fl: Field| base.wrapping_add(fl.off as u64);
                let Some(pc) = decode(d.pc.enc, d.pc.raw, at(d.pc)) else {
                    self.findings.push(
                        e(
                            1,
                            format!(
                                "an FDE pointer encoding {:#x} the check does not read",
                                d.pc.enc
                            ),
                        )
                        .at(base + d.off as u64),
                    );
                    continue;
                };
                outs.push(OutFde {
                    addr: base + d.off as u64,
                    pc,
                    range: d.range,
                    aug_len: d.aug_len,
                    lsda: d.lsda.and_then(|l| decode(l.enc, l.raw, at(l))),
                    personality: c.personality.and_then(|p| decode(p.enc, p.raw, at(p))),
                    cie: CieKey::of(c),
                    insns: d.insns.clone(),
                });
            }
        }
        let mut by_pc: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
        for (i, o) in outs.iter().enumerate() {
            by_pc.entry(o.pc).or_default().push(i);
        }
        let mut used = vec![false; outs.len()];
        let mut unknown_inputs = false;
        let mut input_terminator = false;
        let objs = self.objs;
        for (oi, si) in sections {
            let o = &objs[oi];
            let s = &o.secs[si];
            let raw = match s.relocs.map(|rs| o.records(rs)) {
                None => Vec::new(),
                Some(Some(r)) => r,
                Some(None) => {
                    self.findings.push(
                        Finding::new(
                            Rule::InputUnreadable,
                            "its relocation section cannot be read",
                        )
                        .site(o.site(si, 0)),
                    );
                    continue;
                }
            };
            let n = raw.len() as u64;
            self.cov.records += n;
            if o.elf.machine() != Machine::AArch64 {
                self.not_checked(Reason::Machine, n);
                continue;
            }
            let frames = match parse(s.data) {
                Ok(f) => f,
                Err(m) => {
                    self.fail_n(
                        Finding::new(
                            Rule::InputUnreadable,
                            format!("its .eh_frame cannot be read: {m}"),
                        )
                        .site(o.site(si, 0)),
                        n,
                    );
                    continue;
                }
            };
            input_terminator |= frames.terminator.is_some();
            // synth.md R-P7: an `R_*_NONE` record is no relocation.
            let is_none = |ty: u32| ty == 0 || ty == 256;
            let nones = raw.iter().filter(|r| is_none(r.2)).count() as u64;
            self.cov.none += nones;
            let relocs: BTreeMap<u64, (u32, u32, i64)> = raw
                .iter()
                .filter(|r| !is_none(r.2))
                .map(|&(off, sym, ty, a)| (off, (sym, ty, a.unwrap_or(0))))
                .collect();
            let mut claimed: BTreeMap<u64, bool> =
                raw.iter().filter(|r| is_none(r.2)).map(|r| (r.0, true)).collect();
            let mut resolve = |this: &mut Self, fl: Field| -> In {
                let Some(&(sym, ty, a)) = relocs.get(&(fl.off as u64)) else {
                    // An absolute pointer written in place; a PC-relative
                    // one without a relocation means nothing here.
                    return if fl.enc & 0x70 == 0 && fl.raw != 0 {
                        In::Value(fl.raw)
                    } else {
                        In::Unknown(Reason::EhFrame)
                    };
                };
                claimed.insert(fl.off as u64, true);
                if !matches!(ty, 257 | 258 | 260 | 261) {
                    return In::Unknown(Reason::Type);
                }
                match this.resolve(oi, sym) {
                    Target::Addr(v) => In::Value(v.wrapping_add(a as u64)),
                    Target::UndefWeak => In::Value(a as u64),
                    Target::Discarded(_) => In::Dead,
                    Target::Skip(r) => In::Unknown(r),
                    Target::Tls(_) => In::Unknown(Reason::Tls),
                    Target::Ifunc { .. } => In::Unknown(Reason::Ifunc),
                    Target::Merge { .. } => In::Unknown(Reason::InMergeable),
                }
            };
            // The CIEs' personalities.
            let mut pers: Vec<Option<In>> = Vec::new();
            for c in &frames.cies {
                pers.push(c.personality.map(|p| resolve(self, p)));
            }
            // Records the FDEs and CIEs decide: an FDE's own and its CIE's.
            let fde_records = |d: &Fde| -> u64 {
                [Some(d.pc.off), d.lsda.map(|l| l.off)]
                    .into_iter()
                    .flatten()
                    .filter(|off| relocs.contains_key(&(*off as u64)))
                    .count() as u64
            };
            let mut cie_live = vec![false; frames.cies.len()];
            for d in &frames.fdes {
                let k = fde_records(d);
                // R-P7: an initial location of 0 with no relocation
                // describes no code of the link: dropped.
                let pc = if d.pc.raw == 0 && !relocs.contains_key(&(d.pc.off as u64)) {
                    In::Dead
                } else {
                    resolve(self, d.pc)
                };
                let lsda = d.lsda.map(|l| resolve(self, l));
                let site = o.site(si, d.off as u64);
                let want_pc = match pc {
                    In::Dead => {
                        // Dropped with its function: absent from the output.
                        self.cov.records -= k;
                        self.cov.absent_records += k;
                        self.cov.fdes_dropped += 1;
                        continue;
                    }
                    In::Unknown(r) => {
                        unknown_inputs = true;
                        self.not_checked(r, k);
                        continue;
                    }
                    In::Value(v) => v,
                };
                cie_live[d.cie] = true;
                let want_lsda = match lsda {
                    None => None,
                    Some(In::Value(v)) => Some(v),
                    Some(In::Dead) => {
                        self.fail_n(
                            e(3, "the FDE's LSDA is in a section that is not in the output")
                                .at(want_pc)
                                .site(site),
                            k,
                        );
                        continue;
                    }
                    Some(In::Unknown(r)) => {
                        unknown_inputs = true;
                        self.not_checked(r, k);
                        continue;
                    }
                };
                let want_pers = match &pers[d.cie] {
                    None => None,
                    Some(In::Value(v)) => Some(*v),
                    Some(_) => {
                        unknown_inputs = true;
                        self.not_checked(Reason::EhFrame, k);
                        continue;
                    }
                };
                let key = CieKey::of(&frames.cies[d.cie]);
                let cands = by_pc.get(&want_pc).map_or(&[][..], Vec::as_slice);
                let same = |x: &OutFde| {
                    x.range == d.range
                        && x.aug_len == d.aug_len
                        && x.insns == d.insns
                        && x.cie == key
                        && x.lsda == want_lsda
                        && x.personality == want_pers
                };
                if let Some(&i) = cands.iter().find(|&&i| !used[i] && same(&outs[i])) {
                    used[i] = true;
                    self.cov.fdes_checked += 1;
                    for _ in 0..k {
                        self.cov.add_verified(Via::EhFrame, 261);
                    }
                    continue;
                }
                let f = match cands.iter().find(|&&i| !used[i]) {
                    None => {
                        let mut f =
                            e(2, "a live FDE of the input is not in the output's .eh_frame");
                        f.expected = Some(want_pc);
                        f
                    }
                    Some(&i) => {
                        used[i] = true;
                        let x = &outs[i];
                        let what = if x.range != d.range {
                            "its address range"
                        } else if x.insns != d.insns || x.aug_len != d.aug_len {
                            "its call frame instructions"
                        } else if x.cie != key {
                            "its CIE"
                        } else if x.lsda != want_lsda {
                            "its LSDA"
                        } else {
                            "its personality routine"
                        };
                        let mut f =
                            e(3, format!("the output's FDE for this input FDE differs in {what}"))
                                .at(x.addr);
                        f.expected = want_lsda.filter(|_| what == "its LSDA");
                        f.found = x.lsda.filter(|_| what == "its LSDA");
                        f
                    }
                };
                self.fail_n(f.site(site), k);
            }
            // A personality relocation is decided with the CIE's FDEs.
            for (ci, c) in frames.cies.iter().enumerate() {
                let Some(p) = c.personality else { continue };
                if !relocs.contains_key(&(p.off as u64)) {
                    continue;
                }
                match (&pers[ci], cie_live[ci]) {
                    (_, false) => {
                        self.cov.records -= 1;
                        self.cov.absent_records += 1;
                    }
                    (Some(In::Value(_)), true) => self.cov.add_verified(Via::EhFrame, 261),
                    (Some(In::Unknown(r)), true) => self.not_checked(*r, 1),
                    _ => self.not_checked(Reason::EhFrame, 1),
                }
            }
            // A relocation at no pointer field.
            for &(off, ..) in &raw {
                if !claimed.contains_key(&off) {
                    self.fail(
                        Finding::new(
                            Rule::InputUnreadable,
                            "a .eh_frame relocation at no pointer field of a CIE or FDE",
                        )
                        .site(o.site(si, off)),
                    );
                }
            }
        }
        // The zero terminator an input gives (crtend.o's `__FRAME_END__`)
        // ends the frames `__register_frame_info` walks.
        if let Some((base, _, f)) = &parsed
            && input_terminator
            && f.terminator.is_none()
        {
            self.findings.push(
                e(1, "an input ends .eh_frame with a zero terminator and the output's has none")
                    .at(*base),
            );
        }
        // Output FDEs no live input FDE gives.
        for (i, x) in outs.iter().enumerate() {
            if used[i] {
                continue;
            }
            if x.range == 0 || x.pc == 0 {
                self.cov.fdes_neutralised += 1;
            } else if !unknown_inputs {
                let mut f = e(4, "an FDE of the output's .eh_frame that no live input FDE gives")
                    .at(x.addr);
                f.found = Some(x.pc);
                self.findings.push(f);
            }
        }
        self.check_eh_frame_hdr(parsed.as_ref().map(|p| p.0), &outs);
    }

    fn fail_n(&mut self, f: Finding, n: u64) {
        self.cov.failed += n;
        self.findings.push(f);
    }

    /// §6.4 step 6: `.eh_frame_hdr` (LSB "The `.eh_frame_hdr` section").
    #[allow(clippy::too_many_lines)] // the header, then the table
    fn check_eh_frame_hdr(&mut self, base: Option<u64>, outs: &[OutFde]) {
        let image = self.image;
        let sec = image.sections.iter().find(|s| s.name == ".eh_frame_hdr");
        let ph = image.phdrs.iter().find(|p| p.p_type == PT_GNU_EH_FRAME);
        let (sec, ph) = match (sec, ph) {
            (None, None) => return,
            (Some(s), Some(p)) => (s, p),
            (Some(s), None) => {
                return self
                    .findings
                    .push(e(5, ".eh_frame_hdr and no PT_GNU_EH_FRAME").at(s.shdr.sh_addr));
            }
            (None, Some(p)) => {
                return self
                    .findings
                    .push(e(5, "PT_GNU_EH_FRAME and no .eh_frame_hdr").at(p.p_vaddr));
            }
        };
        let hdr = sec.shdr.sh_addr;
        if ph.p_vaddr != hdr || ph.p_memsz != sec.shdr.sh_size {
            self.findings
                .push(e(5, "PT_GNU_EH_FRAME does not cover exactly .eh_frame_hdr").at(ph.p_vaddr));
        }
        let Some(b) = image.bytes(hdr, sec.shdr.sh_size) else {
            return self.findings.push(e(5, ".eh_frame_hdr is not in the file image").at(hdr));
        };
        let mut r = Rd::new(b, 0);
        let (Some(version), Some(ptr_enc), Some(count_enc), Some(table_enc)) =
            (r.u8(), r.u8(), r.u8(), r.u8())
        else {
            return self.findings.push(e(5, ".eh_frame_hdr is truncated").at(hdr));
        };
        if version != 1 {
            return self.findings.push(e(5, format!(".eh_frame_hdr version {version}")).at(hdr));
        }
        // datarel is relative to the start of .eh_frame_hdr.
        let value = |enc: u8, raw: u64, at: u64| match enc & 0x70 {
            0x30 => Some(hdr.wrapping_add(raw)),
            _ => decode(enc, raw, at),
        };
        let at = hdr + r.pos as u64;
        let Some(ptr) = r.ptr(ptr_enc).and_then(|raw| value(ptr_enc, raw, at)) else {
            return self.findings.push(e(5, "eh_frame_ptr cannot be read").at(hdr));
        };
        let Some(base) = base else { return };
        if ptr != base {
            let mut f = e(5, "eh_frame_ptr is not the address of .eh_frame").at(hdr);
            f.expected = Some(base);
            f.found = Some(ptr);
            self.findings.push(f);
        }
        if count_enc == OMIT || table_enc == OMIT {
            return;
        }
        let at = hdr + r.pos as u64;
        let Some(count) = r.ptr(count_enc).and_then(|raw| value(count_enc, raw, at)) else {
            return self.findings.push(e(5, "fde_count cannot be read").at(hdr));
        };
        let live: Vec<&OutFde> = outs.iter().filter(|x| x.range != 0 && x.pc != 0).collect();
        let fde_at: BTreeMap<u64, &OutFde> = outs.iter().map(|x| (x.addr, x)).collect();
        let mut table: Vec<(u64, u64)> = Vec::new();
        for _ in 0..count.min(b.len() as u64) {
            let a1 = hdr + r.pos as u64;
            let loc = r.ptr(table_enc).and_then(|raw| value(table_enc, raw, a1));
            let a2 = hdr + r.pos as u64;
            let fde = r.ptr(table_enc).and_then(|raw| value(table_enc, raw, a2));
            let (Some(loc), Some(fde)) = (loc, fde) else {
                return self
                    .findings
                    .push(e(5, "the search table is longer than .eh_frame_hdr").at(hdr));
            };
            table.push((loc, fde));
        }
        if count != live.len() as u64 {
            let mut f = e(5, "fde_count is not the number of FDEs in .eh_frame").at(hdr);
            f.expected = Some(live.len() as u64);
            f.found = Some(count);
            self.findings.push(f);
        }
        for w in table.windows(2) {
            if w[0].0 >= w[1].0 {
                let f = e(5, "the search table is not sorted by initial location").at(w[1].0);
                self.findings.push(f);
                break;
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for &(loc, fde) in &table {
            match fde_at.get(&fde) {
                Some(x) if x.pc == loc => {
                    seen.insert(fde);
                    self.cov.eh_frame_hdr_entries += 1;
                }
                Some(x) => {
                    let mut f = e(5, "a search table entry's location is not its FDE's").at(loc);
                    f.expected = Some(x.pc);
                    f.found = Some(loc);
                    self.findings.push(f);
                }
                None => {
                    let mut f = e(5, "a search table entry points at no FDE").at(loc);
                    f.found = Some(fde);
                    self.findings.push(f);
                }
            }
        }
        for x in live {
            if !seen.contains(&x.addr) {
                let mut f = e(5, "an FDE of .eh_frame is not in the search table").at(x.addr);
                f.expected = Some(x.pc);
                self.findings.push(f);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec/plumb.md §6.4: the LEB128 forms and the pointer formats.
    #[test]
    fn s6_4_leb128_and_pointers() {
        let b = [0xe5, 0x8e, 0x26, 0x7f, 0x80, 0x7f];
        let mut r = Rd::new(&b, 0);
        assert_eq!(r.uleb(), Some(624_485));
        assert_eq!(r.sleb(), Some(-1));
        assert_eq!(r.sleb(), Some(-128));
        let b = 0xffff_fff0u32.to_le_bytes();
        assert_eq!(Rd::new(&b, 0).ptr(0x1b), Some((-16i64) as u64));
        assert_eq!(Rd::new(&b, 0).ptr(0x03), Some(0xffff_fff0));
        assert_eq!(Rd::new(&b, 0).ptr(0x05), None);
        assert_eq!(decode(0x1b, (-16i64) as u64, 0x1000), Some(0xff0));
        assert_eq!(decode(0x9b, 8, 0x1000), Some(0x1008));
        assert_eq!(decode(0x20, 8, 0x1000), None);
    }

    // spec/plumb.md §6.4: every augmentation the check reads, `G`
    // (MTE-tagged frames) among them; an unknown one cannot be read.
    #[test]
    fn s6_4_augmentations() {
        let cie = |aug: &[u8], data: &[u8]| {
            let mut body = vec![0u8, 0, 0, 0, 1];
            body.extend(aug);
            body.extend([0, 4, 0x78, 30, data.len() as u8]);
            body.extend(data);
            while body.len() % 4 != 0 {
                body.push(0);
            }
            let mut b = (body.len() as u32).to_le_bytes().to_vec();
            b.extend(body);
            parse(&b)
        };
        let f =
            cie(b"zPLRSBG", &[0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x1b, 0x1b])
                .unwrap();
        assert_eq!(f.cies[0].personality.map(|p| p.raw), Some(0x8877_6655_4433_2211));
        assert_eq!((f.cies[0].lsda_enc, f.cies[0].fde_enc), (0x1b, 0x1b));
        assert!(cie(b"zX", &[]).unwrap_err().contains("augmentation"));
        assert!(cie(b"eh", &[]).unwrap_err().contains("augmentation"));
        // Augmentation data shorter than its fields.
        assert!(cie(b"zR", &[0x1b]).is_ok());
        assert!(cie(b"zPR", &[0x00, 1]).is_err());
    }

    // spec/plumb.md §6.4: a CIE, an FDE pointing at it, the terminator.
    #[test]
    fn s6_4_records_parse() {
        let mut b: Vec<u8> = Vec::new();
        // CIE: length, id 0, version 1, "zR", code 4, data -8, ra 30, augdata 1: 0x1b, insns.
        let cie = [0u8, 0, 0, 0, 1, b'z', b'R', 0, 4, 0x78, 30, 1, 0x1b, 0x0c, 0x1f, 0];
        b.extend((cie.len() as u32).to_le_bytes());
        b.extend(cie);
        b.extend([0, 0]); // DW_CFA_nop padding to a multiple of 4
        let fixed = b.len() - 4;
        b[0..4].copy_from_slice(&(fixed as u32).to_le_bytes());
        let fde_at = b.len();
        let mut fde: Vec<u8> = Vec::new();
        fde.extend(((fde_at + 4) as u32).to_le_bytes()); // CIE pointer
        fde.extend((-0x100i32).to_le_bytes()); // pc
        fde.extend(0x40u32.to_le_bytes()); // range
        fde.extend([0, 0x44, 0, 0]); // aug len 0, advance, nops
        b.extend((fde.len() as u32).to_le_bytes());
        b.extend(fde);
        b.extend([0, 0, 0, 0]);
        let f = parse(&b).unwrap();
        assert_eq!(f.cies.len(), 1);
        assert_eq!(f.cies[0].fde_enc, 0x1b);
        assert_eq!(f.cies[0].insns, vec![0x0c, 0x1f]);
        assert_eq!(f.fdes.len(), 1);
        assert_eq!(f.fdes[0].range, 0x40);
        assert_eq!(f.fdes[0].insns, vec![0x44]);
        assert_eq!(f.terminator, Some(b.len() - 4));
        // A CIE pointer to no CIE.
        let mut bad = b.clone();
        bad[fde_at + 4..fde_at + 8].copy_from_slice(&(fde_at as u32).to_le_bytes());
        assert!(parse(&bad).unwrap_err().contains("no CIE"));
        // A record after the terminator.
        let mut bad = b.clone();
        bad.extend([4, 0, 0, 0, 1, 0, 0, 0]);
        assert!(parse(&bad).is_err());
    }
}

//! The loader model of stage 3 (`spec/dyn.md` P-D9, P-D10): the output's
//! dynamic records applied as ld.so applies them, in a view (a load bias
//! and the addresses the libraries' definitions get), giving the bytes the
//! program sees after loading. Stage 2's engine then judges every input
//! relocation against those bytes.

use std::collections::HashMap;

use mortar_elf::consts::{EM_AARCH64, EM_X86_64, PF_W, PT_GNU_RELRO, PT_LOAD, PT_TLS, STB_WEAK};

use crate::dynmodel::Model;
use crate::dynout::DynOut;
use crate::image::Image;
use crate::report::{Finding, Rule};

/// The relocation types the loader applies, by machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    None,
    Abs,
    Copy,
    GlobDat,
    JumpSlot,
    Relative,
    DtpMod,
    DtpOff,
    TpOff,
    TlsDesc,
    Irelative,
    Unknown,
}

pub fn kind(machine: u16, ty: u32) -> Kind {
    match (machine, ty) {
        (EM_AARCH64, 0) | (EM_X86_64, 0) => Kind::None,
        (EM_AARCH64, 257) | (EM_X86_64, 1) => Kind::Abs,
        (EM_AARCH64, 1024) | (EM_X86_64, 5) => Kind::Copy,
        (EM_AARCH64, 1025) | (EM_X86_64, 6) => Kind::GlobDat,
        (EM_AARCH64, 1026) | (EM_X86_64, 7) => Kind::JumpSlot,
        (EM_AARCH64, 1027) | (EM_X86_64, 8) => Kind::Relative,
        (EM_AARCH64, 1028) | (EM_X86_64, 16) => Kind::DtpMod,
        (EM_AARCH64, 1029) | (EM_X86_64, 17) => Kind::DtpOff,
        (EM_AARCH64, 1030) | (EM_X86_64, 18) => Kind::TpOff,
        (EM_AARCH64, 1031) | (EM_X86_64, 36) => Kind::TlsDesc,
        (EM_AARCH64, 1032) | (EM_X86_64, 37) => Kind::Irelative,
        _ => Kind::Unknown,
    }
}

/// The address a library's byte 0 gets in every view: far from the
/// executable and from each other.
pub fn lib_base(i: usize) -> u64 {
    0x7f00_0000_0000 + ((i as u64) << 32)
}

/// One view's parameters.
#[derive(Debug, Clone, Default)]
pub struct Params {
    /// The executable's load bias (0 for `Exec`).
    pub bias: u64,
    /// `WeakDyn` keys defined at fresh addresses (view 3).
    pub fresh: HashMap<String, u64>,
}

/// What a symbol of `.dynsym` comes to at load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bound {
    /// The executable's own definition (or canonical PLT).
    Exec(u64),
    /// Library `i`'s definition: its run-time address and `st_size`.
    Lib { i: usize, addr: u64, size: u64, tls: bool },
    /// A preloaded definition (view 3).
    Fresh(u64),
    /// No definition: a weak reference reads 0.
    Weak0,
    /// No definition of a strong reference: the load fails.
    Missing,
}

impl Bound {
    pub fn addr(self) -> u64 {
        match self {
            Bound::Exec(a) | Bound::Fresh(a) | Bound::Lib { addr: a, .. } => a,
            Bound::Weak0 | Bound::Missing => 0,
        }
    }
}

/// The loader's world for one output.
#[derive(Debug)]
pub struct Loader<'m> {
    pub model: &'m Model,
    /// The search list after the executable (the image's `DT_NEEDED`
    /// resolved, then breadth first).
    pub order: Vec<usize>,
    pub machine: u16,
}

impl Loader<'_> {
    /// Looks up `.dynsym` entry `j` of the output as ld.so would.
    /// `plt` is a `JUMP_SLOT` lookup, which skips the executable
    /// (`ELF_RTYPE_CLASS_PLT`).
    pub fn bind(&self, out: &DynOut, j: usize, plt: bool, p: &Params) -> Bound {
        let Some(s) = out.syms.get(j) else { return Bound::Missing };
        if s.defined() {
            let v = if s.st_shndx == mortar_elf::consts::SHN_ABS {
                s.st_value
            } else {
                s.st_value.wrapping_add(p.bias)
            };
            return Bound::Exec(v);
        }
        // A canonical PLT: the executable's entry is the address for every
        // lookup but the PLT's own.
        if !plt && s.st_value != 0 {
            return Bound::Exec(s.st_value.wrapping_add(p.bias));
        }
        if let Some(&f) = p.fresh.get(&s.name) {
            return Bound::Fresh(f);
        }
        let ver = out.need_of(j).map(|n| n.name.as_str());
        match self.model.lookup(&self.order, &s.name, ver) {
            Some((i, d)) => Bound::Lib {
                i,
                addr: lib_base(i).wrapping_add(d.value),
                size: d.size,
                tls: d.ty == mortar_elf::consts::STT_TLS,
            },
            None if s.binding() == STB_WEAK => Bound::Weak0,
            None => Bound::Missing,
        }
    }
}

/// The patched bytes of one view, and what applying the records found.
#[derive(Debug)]
pub struct View {
    pub bytes: Vec<u8>,
    pub findings: Vec<Finding>,
    /// Runtime address of every undefined `.dynsym` name (for the engine).
    pub dynbind: HashMap<String, u64>,
    /// Names bound to a library's thread-local definition.
    pub dyn_tls: std::collections::HashSet<String>,
}

fn file_off(image: &Image<'_>, addr: u64, len: u64) -> Option<usize> {
    image.phdrs.iter().filter(|p| p.p_type == PT_LOAD).find_map(|p| {
        let off = addr.checked_sub(p.p_vaddr)?;
        (off.checked_add(len)? <= p.p_filesz).then(|| (p.p_offset + off) as usize)
    })
}

/// Applies the output's records in view `p`.
#[allow(clippy::too_many_lines)] // one arm per record kind
pub fn apply(image: &Image<'_>, out: &DynOut, ld: &Loader<'_>, p: &Params, with_findings: bool) -> View {
    let mut v = View {
        bytes: image.data.to_vec(),
        findings: Vec::new(),
        dynbind: HashMap::new(),
        dyn_tls: std::collections::HashSet::new(),
    };
    let m = ld.machine;
    let f = |msg: String, at: u64| Finding::new(Rule::Dyn(9), msg).at(at);
    let tls = image.phdrs.iter().find(|q| q.p_type == PT_TLS);
    let tcb = tls.map(|t| {
        let align = t.p_align.max(1);
        if m == EM_AARCH64 {
            16u64.next_multiple_of(align)
        } else {
            t.p_memsz.next_multiple_of(align).wrapping_neg()
        }
    });
    let tls_start = tls.map_or(0, |t| t.p_vaddr);
    let relro: Vec<(u64, u64)> = image
        .phdrs
        .iter()
        .filter(|q| q.p_type == PT_GNU_RELRO)
        .map(|q| (q.p_vaddr, q.p_vaddr + q.p_memsz))
        .collect();
    for (j, s) in out.syms.iter().enumerate() {
        if j == 0 || s.defined() {
            continue;
        }
        let b = ld.bind(out, j, false, p);
        if let Bound::Lib { tls: true, .. } = b {
            v.dyn_tls.insert(s.name.clone());
        }
        if !matches!(b, Bound::Missing) {
            v.dynbind.insert(s.name.clone(), b.addr());
        }
    }
    let word: u64 = 8;
    for r in &out.rels {
        let k = kind(m, r.ty);
        let a = r.addend.unwrap_or(0);
        let name = || out.syms.get(r.sym as usize).map_or_else(String::new, |s| s.name.clone());
        if with_findings
            && !matches!(k, Kind::None | Kind::Copy)
            && let Some(lp) = image.load_of(r.offset, word)
            && lp.p_flags & PF_W == 0
            && !relro.iter().any(|&(lo, hi)| lo <= r.offset && r.offset + word <= hi)
        {
            v.findings.push(f(
                format!(
                    "a dynamic record (type {}) writes a non-writable place outside PT_GNU_RELRO: a text relocation (D-47)",
                    r.ty
                ),
                r.offset,
            ));
        }
        let sym = |plt: bool| -> Bound {
            if r.sym == 0 { Bound::Exec(0) } else { ld.bind(out, r.sym as usize, plt, p) }
        };
        let value: Option<u64> = match k {
            Kind::None => None,
            Kind::Relative | Kind::Irelative => Some(p.bias.wrapping_add(a as u64)),
            Kind::Abs | Kind::GlobDat | Kind::JumpSlot => {
                let b = sym(k == Kind::JumpSlot);
                if b == Bound::Missing {
                    if with_findings {
                        let mut x = f(
                            format!("the loader finds no definition of '{}': the load fails", name()),
                            r.offset,
                        );
                        x.symbol = Some(name());
                        v.findings.push(x);
                    }
                    None
                } else {
                    let add = if k == Kind::Abs || m == EM_AARCH64 { a as u64 } else { 0 };
                    Some(b.addr().wrapping_add(add))
                }
            }
            Kind::Copy => {
                if with_findings {
                    let b = sym(true);
                    let own = out.syms.get(r.sym as usize);
                    match (b, own) {
                        (Bound::Lib { size, i, .. }, Some(o)) => {
                            if o.st_size != size {
                                let mut x = f(
                                    format!(
                                        "COPY of '{}' allocates {} bytes; the loader binds the definition of {} ({} bytes)",
                                        o.name, o.st_size, ld.model.dsos[i].soname, size
                                    ),
                                    r.offset,
                                );
                                x.symbol = Some(o.name.clone());
                                x.expected = Some(size);
                                x.found = Some(o.st_size);
                                v.findings.push(x);
                            }
                            if o.st_value.wrapping_add(p.bias) != r.offset.wrapping_add(p.bias) {
                                v.findings.push(f(
                                    format!("COPY of '{}' is not at its .dynsym value", o.name),
                                    r.offset,
                                ));
                            }
                        }
                        (Bound::Missing | Bound::Weak0, _) => v.findings.push(f(
                            format!("COPY of '{}': the loader finds no library definition", name()),
                            r.offset,
                        )),
                        _ => v.findings.push(f(
                            format!("COPY of '{}' is not bound to a library definition", name()),
                            r.offset,
                        )),
                    }
                }
                None
            }
            Kind::DtpMod => match sym(false) {
                Bound::Exec(_) => Some(1),
                Bound::Lib { i, .. } => Some(2 + i as u64),
                _ => Some(0),
            },
            Kind::DtpOff => match sym(false) {
                Bound::Exec(s) if r.sym != 0 => {
                    Some(s.wrapping_sub(p.bias).wrapping_sub(tls_start).wrapping_add(a as u64))
                }
                Bound::Lib { i, addr, .. } => {
                    Some(addr.wrapping_sub(lib_base(i)).wrapping_add(a as u64))
                }
                _ => Some(a as u64),
            },
            Kind::TpOff => match (sym(false), tcb) {
                (Bound::Exec(s), Some(t)) => {
                    let off = if r.sym == 0 { 0 } else { s.wrapping_sub(p.bias).wrapping_sub(tls_start) };
                    Some(t.wrapping_add(off).wrapping_add(a as u64))
                }
                // A library's block: an offset the model invents; the
                // engine does not judge TLS imports (Reason::Dynamic).
                (Bound::Lib { i, addr, .. }, _) => Some(
                    0x1000_0000u64
                        .wrapping_mul(i as u64 + 1)
                        .wrapping_add(addr.wrapping_sub(lib_base(i)))
                        .wrapping_add(a as u64),
                ),
                _ => None,
            },
            Kind::TlsDesc | Kind::Unknown => {
                if with_findings && k == Kind::Unknown {
                    v.findings.push(f(format!("a dynamic record of unknown type {}", r.ty), r.offset));
                }
                None
            }
        };
        let Some(val) = value else { continue };
        if let Some(o) = file_off(image, r.offset, word) {
            v.bytes[o..o + 8].copy_from_slice(&val.to_le_bytes());
        } else if with_findings {
            v.findings.push(f(
                format!("a dynamic record (type {}) at a place outside the file image: not modelled", r.ty),
                r.offset,
            ));
        }
    }
    v
}

/// The `.got.plt` word a PLT entry at `addr` jumps through, decoded from
/// its instructions (AArch64: `[bti c] adrp x16; ldr x17, [x16, #lo]`;
/// x86-64: `[endbr64] [mov $i, %r11d | push] [bnd] jmp *slot(%rip)`).
pub fn plt_slot(image: &Image<'_>, addr: u64) -> Option<u64> {
    match image.header.e_machine {
        EM_AARCH64 => {
            let mut a = addr;
            let mut w = image.insn(a)?;
            if w == 0xd503_245f || w == 0xd503_249f || w == 0xd503_24df {
                a += 4;
                w = image.insn(a)?;
            }
            let (rd, page) = crate::a64::adrp(w, a)?;
            let (rt, rn, off) = crate::a64::ldr64_imm(image.insn(a + 4)?)?;
            (rd == 16 && rn == 16 && rt == 17).then_some(page.wrapping_add(off))
        }
        EM_X86_64 => {
            let b = image.bytes(addr, 16)?;
            let mut i = 0;
            if b[..4] == [0xf3, 0x0f, 0x1e, 0xfa] {
                i = 4;
            }
            if b[i..i + 2] == [0x41, 0xbb] {
                i += 6;
            }
            if b[i] == 0xf2 {
                i += 1;
            }
            if i + 6 > b.len() || b[i..i + 2] != [0xff, 0x25] {
                return None;
            }
            let rel = i32::from_le_bytes([b[i + 2], b[i + 3], b[i + 4], b[i + 5]]);
            Some((addr + i as u64 + 6).wrapping_add_signed(i64::from(rel)))
        }
        _ => None,
    }
}

/// Every PLT-like entry of the output: entry address → slot, over the
/// sections `.plt`, `.plt.sec`, `.plt.got`, `.iplt` scanned at 4-byte
/// steps (AArch64) or at 8/16-byte steps (x86-64).
pub fn plt_entries(image: &Image<'_>) -> std::collections::BTreeMap<u64, u64> {
    let mut out = std::collections::BTreeMap::new();
    for s in &image.sections {
        if !matches!(s.name.as_str(), ".plt" | ".plt.sec" | ".plt.got" | ".iplt") {
            continue;
        }
        let (lo, hi) = (s.shdr.sh_addr, s.shdr.sh_addr + s.shdr.sh_size);
        let step = if image.header.e_machine == EM_AARCH64 { 4 } else { 8 };
        let mut a = lo;
        while a < hi {
            if let Some(slot) = plt_slot(image, a) {
                // An x86-64 lazy stub's own `push`/`jmp` is not an entry;
                // the decoder starts only at an `endbr64`, `mov` or `jmp`.
                out.insert(a, slot);
            }
            a += step;
        }
    }
    out
}

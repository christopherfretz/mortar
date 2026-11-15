//! The mutation classes of stage 3 (`spec/plumb.md` §9.5, §12): a
//! dynamic output's tables broken one field at a time. Each class names
//! the P-D rule meant to catch it; the kill table names every survivor.
//! ELF64 only: dynamic outputs are AArch64 and x86-64 (`spec/dyn.md` §1).

use mortar_elf::consts::{PT_DYNAMIC, PT_GNU_RELRO, PT_INTERP};

use crate::dynout::{
    DT_FLAGS_1, DT_GNU_HASH, DT_INIT_ARRAYSZ, DT_JMPREL, DT_NEEDED, DT_PLTGOT, DT_PLTRELSZ,
    DT_RELA, DT_RELAENT, DT_RELASZ, DT_STRSZ, DT_SYMENT, DT_SYMTAB, DT_VERNEED, DT_VERNEEDNUM,
    DT_VERSYM, DynOut,
};
use crate::image::Image;
use crate::loader::{Kind, kind};

/// The classes, in the table's order, each with the rule meant to catch
/// it.
pub const CLASSES: &[(&str, &str)] = &[
    ("dyn-needed", "P-D6"),
    ("dyn-tag", "P-D6"),
    ("dyn-rel-offset", "P-D9"),
    ("dyn-rel-drop", "P-D9"),
    ("dyn-rel-addend", "P-D9"),
    ("dyn-rel-symbol", "P-D9"),
    ("dyn-rel-order", "P-D9 (D-40, D-41)"),
    ("dynsym-value", "P-D5, P-D7"),
    ("dynsym-visibility", "P-D7"),
    ("gnu-hash", "P-D5"),
    ("versym", "P-D8"),
    ("version-need", "P-D8"),
    ("interp", "P-D4"),
    ("relro", "P-D4"),
    ("lazy-slot", "P-D10"),
    ("copy-size", "P-D11"),
];

/// A candidate: its class, its description and its byte edits.
pub type Cand = (&'static str, String, Vec<(usize, Vec<u8>)>);

fn off(image: &Image<'_>, addr: u64, len: u64) -> Option<usize> {
    image.phdrs.iter().filter(|p| p.p_type == mortar_elf::consts::PT_LOAD).find_map(|p| {
        let o = addr.checked_sub(p.p_vaddr)?;
        (o.checked_add(len)? <= p.p_filesz).then(|| (p.p_offset + o) as usize)
    })
}

fn rd(b: &[u8], o: usize, w: usize) -> u64 {
    b[o..o + w].iter().rev().fold(0u64, |a, &x| a << 8 | u64::from(x))
}

fn wr(v: u64, w: usize) -> Vec<u8> {
    v.to_le_bytes()[..w].to_vec()
}

/// Every candidate of the dynamic classes in `image` (none for a static
/// output or an ELF32 one).
#[allow(clippy::too_many_lines)] // one block per class
pub fn candidates(image: &Image<'_>) -> Vec<Cand> {
    let mut c: Vec<Cand> = Vec::new();
    if image.class() != mortar_elf::Class::Elf64 {
        return c;
    }
    let Some(out) = DynOut::read(image) else { return c };
    let b = image.data;
    let m = image.header.e_machine;
    // .dynamic's entries.
    if let Some(pd) = image.phdrs.iter().find(|p| p.p_type == PT_DYNAMIC) {
        let base = pd.p_offset as usize;
        let needed: Vec<u64> = out.entries.iter().filter(|e| e.0 == DT_NEEDED).map(|e| e.1).collect();
        for (i, &(tag, val)) in out.entries.iter().enumerate() {
            let o = base + 16 * i + 8;
            if o + 8 > b.len() {
                break;
            }
            if tag == DT_NEEDED {
                let other = needed.iter().find(|&&v| v != val).copied().unwrap_or(val + 3);
                c.push(("dyn-needed", format!("DT_NEEDED #{i}: another name"), vec![(o, wr(other, 8))]));
            }
            let w = match tag {
                DT_STRSZ | DT_PLTRELSZ | DT_RELASZ | DT_INIT_ARRAYSZ => val + 8,
                DT_SYMENT | DT_RELAENT => val * 2,
                DT_SYMTAB | DT_RELA | DT_JMPREL | DT_GNU_HASH | DT_VERSYM | DT_VERNEED | DT_PLTGOT => {
                    val + 8
                }
                DT_VERNEEDNUM => val + 1,
                DT_FLAGS_1 => val ^ 1,
                _ => continue,
            };
            c.push((
                "dyn-tag",
                format!("{} {val:#x} -> {w:#x}", crate::dynout::tag_name(tag)),
                vec![(o, wr(w, 8))],
            ));
        }
    }
    // The records.
    let mut rel_offs: Vec<(usize, bool)> = Vec::new();
    for r in &out.rels {
        let Some(o) = off(image, r.at, 24) else { continue };
        rel_offs.push((o, r.plt));
        let k = kind(m, r.ty);
        let what = format!("{} record at {:#x} for {:#x}", r.ty, r.at, r.offset);
        c.push(("dyn-rel-offset", format!("{what}: offset + 8"), vec![(o, wr(r.offset + 8, 8))]));
        // R_*_NONE: the record dropped in place.
        c.push(("dyn-rel-drop", format!("{what}: dropped"), vec![(o + 8, wr(u64::from(r.sym) << 32, 8))]));
        if let Some(a) = r.addend
            && !matches!(k, Kind::Copy | Kind::JumpSlot)
        {
            c.push((
                "dyn-rel-addend",
                format!("{what}: addend + 8"),
                vec![(o + 16, wr(a.wrapping_add(8) as u64, 8))],
            ));
        }
        if r.sym != 0 {
            let other = (1..out.syms.len() as u32)
                .find(|&j| j != r.sym && out.syms[j as usize].sym_type() == out.syms[r.sym as usize].sym_type())
                .unwrap_or(0);
            c.push((
                "dyn-rel-symbol",
                format!("{what}: symbol {} -> {}", r.sym, other),
                vec![(o + 8, wr(u64::from(other) << 32 | u64::from(r.ty), 8))],
            ));
        }
    }
    // Order: two neighbouring records of different kinds swapped.
    for w in out.rels.windows(2) {
        let (x, y) = (&w[0], &w[1]);
        if x.plt != y.plt || kind(m, x.ty) == kind(m, y.ty) {
            continue;
        }
        let (Some(ox), Some(oy)) = (off(image, x.at, 24), off(image, y.at, 24)) else { continue };
        c.push((
            "dyn-rel-order",
            format!("records at {:#x} and {:#x} swapped", x.at, y.at),
            vec![(ox, b[oy..oy + 24].to_vec()), (oy, b[ox..ox + 24].to_vec())],
        ));
    }
    // .dynsym.
    if let Some(symtab) = out.get(DT_SYMTAB) {
        for (j, s) in out.syms.iter().enumerate().skip(1) {
            let Some(o) = off(image, symtab + 24 * j as u64, 24) else { continue };
            if s.defined() {
                c.push((
                    "dynsym-value",
                    format!("'{}' value + 8", s.name),
                    vec![(o + 8, wr(s.st_value + 8, 8))],
                ));
                c.push((
                    "dynsym-visibility",
                    format!("'{}' hidden", s.name),
                    vec![(o + 5, vec![(s.st_other & !3) | 2])],
                ));
            }
        }
        // copy-size: the definition a COPY record copies, its size doubled.
        for r in &out.rels {
            if kind(m, r.ty) == Kind::Copy
                && let Some(s) = out.syms.get(r.sym as usize)
                && let Some(o) = off(image, symtab + 24 * u64::from(r.sym), 24)
            {
                c.push((
                    "copy-size",
                    format!("'{}' size {} -> {}", s.name, s.st_size, s.st_size * 2 + 8),
                    vec![(o + 16, wr(s.st_size * 2 + 8, 8))],
                ));
            }
        }
    }
    // .gnu.hash: each bucket, and the chain's low bits.
    if let (Some(h), Some(addr)) = (&out.gnu_hash, out.get(DT_GNU_HASH)) {
        let bk = addr + 16 + 8 * u64::from(h.bloom_size);
        for (i, &v) in h.buckets.iter().enumerate() {
            if v == 0 {
                continue;
            }
            if let Some(o) = off(image, bk + 4 * i as u64, 4) {
                c.push(("gnu-hash", format!("bucket {i} {v} -> {}", v + 1), vec![(o, wr(u64::from(v) + 1, 4))]));
            }
        }
        let ch = bk + 4 * u64::from(h.nbuckets);
        for (i, &v) in h.chain.iter().enumerate() {
            if let Some(o) = off(image, ch + 4 * i as u64, 4) {
                c.push(("gnu-hash", format!("chain {i}: end bit flipped"), vec![(o, wr(u64::from(v ^ 1), 4))]));
            }
        }
    }
    // .gnu.version: an import's index moved to another.
    if let (Some(vs), Some(addr)) = (&out.versym, out.get(DT_VERSYM)) {
        let max = vs.iter().copied().max().unwrap_or(1);
        for (j, &v) in vs.iter().enumerate().skip(1) {
            let to = if v >= max { 1 } else { v + 1 };
            if let Some(o) = off(image, addr + 2 * j as u64, 2) {
                c.push(("versym", format!("entry {j}: {v} -> {to}"), vec![(o, wr(u64::from(to), 2))]));
            }
        }
    }
    // .gnu.version_r: each Vernaux's hash and index.
    if let Some(mut vn) = out.get(DT_VERNEED).and_then(|a| off(image, a, 16)) {
        for _ in 0..out.get(DT_VERNEEDNUM).unwrap_or(0).min(64) {
            let (cnt, aux, next) = (rd(b, vn + 2, 2), rd(b, vn + 8, 4) as usize, rd(b, vn + 12, 4) as usize);
            let mut a = vn + aux;
            for _ in 0..cnt.min(64) {
                if a + 16 > b.len() {
                    break;
                }
                let (hash, other) = (rd(b, a, 4), rd(b, a + 6, 2));
                c.push(("version-need", format!("vernaux at {a:#x}: hash"), vec![(a, wr(hash ^ 0x10, 4))]));
                c.push(("version-need", format!("vernaux at {a:#x}: index {other} -> 1"), vec![(a + 6, wr(1, 2))]));
                let n = rd(b, a + 12, 4) as usize;
                if n == 0 {
                    break;
                }
                a += n;
            }
            if next == 0 {
                break;
            }
            vn += next;
        }
    }
    // Program headers.
    let (phoff, phent) = (image.header.e_phoff as usize, usize::from(image.header.e_phentsize));
    for (i, p) in image.phdrs.iter().enumerate() {
        let h = phoff + i * phent;
        if p.p_type == PT_INTERP && p.p_filesz >= 2 {
            let o = (p.p_offset + p.p_filesz - 2) as usize;
            c.push(("interp", "PT_INTERP's last letter".into(), vec![(o, vec![b[o] ^ 0x20])]));
        }
        if p.p_type == PT_GNU_RELRO {
            for d in [8u64, 0x1000] {
                if p.p_memsz > d {
                    c.push((
                        "relro",
                        format!("PT_GNU_RELRO memsz - {d:#x}"),
                        vec![(h + 0x28, wr(p.p_memsz - d, 8)), (h + 0x20, wr(p.p_filesz.saturating_sub(d), 8))],
                    ));
                }
            }
        }
    }
    // The lazy words of JUMP_SLOT records.
    for r in out.rels.iter().filter(|r| r.plt && kind(m, r.ty) == Kind::JumpSlot) {
        if let Some(o) = off(image, r.offset, 8) {
            let v = rd(b, o, 8);
            if v != 0 {
                c.push(("lazy-slot", format!("slot {:#x}: {v:#x} + 4", r.offset), vec![(o, wr(v + 4, 8))]));
            }
        }
    }
    c
}

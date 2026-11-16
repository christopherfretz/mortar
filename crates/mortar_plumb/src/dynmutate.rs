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
/// it. Read by `mutate`'s test that keeps the two lists in step.
#[allow(dead_code)]
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
    ("dynsym-unique", "P-D7"),
    ("gnu-hash", "P-D5"),
    ("versym", "P-D8"),
    ("version-need", "P-D8"),
    ("interp", "P-D4"),
    ("relro", "P-D4"),
    ("lazy-slot", "P-D10"),
    ("copy-size", "P-D11"),
    ("copy-relro", "P-D11"),
    ("copy-section", "P-D11 (Format::Mortar)"),
    ("plt0-disp1", "P-D10"),
    ("plt0-disp2", "P-D10"),
    ("plt-index", "P-D10"),
    ("plt-back-jump", "P-D10"),
    ("spie-rel-drop", "P-D9 (view 2)"),
    ("spie-rel-extra", "P-D9 (§12.4)"),
    ("spie-irel-twice", "P-D15"),
    ("spie-interp", "P-D1, P-D4"),
    ("spie-needed", "P-D14"),
    ("spie-relacount", "P-D6"),
    ("spie-dynamic-word", "P-D14"),
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

/// The place of a copy at `at` (`size` bytes) mutated (WP-088):
/// `copy-relro` moves `PT_GNU_RELRO`'s end so that the copy's membership
/// flips (a read-only definition's copy left writable after relocation,
/// or a writable one's made read-only); `copy-section` renames the
/// output section holding it (its name's last letter's case), which only
/// `Format::Mortar`'s chunk rule reads (D-42; equivalent for a
/// reference).
fn copy_place(image: &Image<'_>, at: u64, size: u64, name: &str, c: &mut Vec<Cand>) {
    let (phoff, phent) = (image.header.e_phoff as usize, usize::from(image.header.e_phentsize));
    if let Some((i, p)) = image.phdrs.iter().enumerate().find(|(_, p)| p.p_type == PT_GNU_RELRO) {
        let h = phoff + i * phent;
        let inside = p.p_vaddr <= at && at + size <= p.p_vaddr + p.p_memsz;
        let memsz = if inside {
            Some(at - p.p_vaddr)
        } else if at >= p.p_vaddr {
            Some(at + size - p.p_vaddr)
        } else {
            None
        };
        if let Some(m) = memsz {
            c.push((
                "copy-relro",
                format!(
                    "PT_GNU_RELRO memsz {:#x} -> {m:#x}: the copy of '{name}' {}",
                    p.p_memsz,
                    if inside { "left out" } else { "taken in" }
                ),
                vec![(h + 0x28, wr(m, 8)), (h + 0x20, wr(p.p_filesz.min(m), 8))],
            ));
        }
    }
    let strtab = image.sections.get(usize::from(image.header.e_shstrndx));
    if let (Some((_, sec)), Some(t)) = (image.alloc_section_of(at, size), strtab)
        && !sec.name.is_empty()
    {
        let o = (t.shdr.sh_offset + u64::from(sec.shdr.sh_name)) as usize + sec.name.len() - 1;
        if let Some(&x) = image.data.get(o)
            && x.is_ascii_alphabetic()
        {
            c.push((
                "copy-section",
                format!("{} (the copy of '{name}') renamed", sec.name),
                vec![(o, vec![x ^ 0x20])],
            ));
        }
    }
}

/// Every candidate of the dynamic classes in `image` (none for a static
/// output or an ELF32 one).
#[allow(clippy::too_many_lines, clippy::many_single_char_names)] // one block per class
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
        let needed: Vec<u64> =
            out.entries.iter().filter(|e| e.0 == DT_NEEDED).map(|e| e.1).collect();
        for (i, &(tag, val)) in out.entries.iter().enumerate() {
            let o = base + 16 * i + 8;
            if o + 8 > b.len() {
                break;
            }
            if tag == DT_NEEDED {
                let other = needed.iter().find(|&&v| v != val).copied().unwrap_or(val + 3);
                c.push((
                    "dyn-needed",
                    format!("DT_NEEDED #{i}: another name"),
                    vec![(o, wr(other, 8))],
                ));
            }
            let w = match tag {
                DT_SYMENT | DT_RELAENT => val * 2,
                DT_STRSZ | DT_PLTRELSZ | DT_RELASZ | DT_INIT_ARRAYSZ | DT_SYMTAB | DT_RELA
                | DT_JMPREL | DT_GNU_HASH | DT_VERSYM | DT_VERNEED | DT_PLTGOT => val + 8,
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
        c.push((
            "dyn-rel-drop",
            format!("{what}: dropped"),
            vec![(o + 8, wr(u64::from(r.sym) << 32, 8))],
        ));
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
                .find(|&j| {
                    j != r.sym
                        && out.syms[j as usize].sym_type() == out.syms[r.sym as usize].sym_type()
                })
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
        // Only RELATIVE (D-40, DT_RELACOUNT) and IRELATIVE (D-41) have
        // a place; two others swapped are the same load.
        let placed = |k| matches!(k, Kind::Relative | Kind::Irelative);
        let (kx, ky) = (kind(m, x.ty), kind(m, y.ty));
        if x.plt != y.plt || kx == ky || !(placed(kx) || placed(ky)) {
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
            if s.defined() && s.sym_type() != mortar_elf::consts::STT_SECTION {
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
            // dynsym-unique: an exported STB_GNU_UNIQUE definition made
            // STB_GLOBAL in `.dynsym` and in its `.symtab` entry (WP-054
            // case (a), where both were written global).
            if s.defined() && s.binding() == 10 {
                let global = (1 << 4) | (s.st_info & 0xf);
                let mut e = vec![(o + 4, vec![global])];
                let symtab = image
                    .sections
                    .iter()
                    .find(|x| x.shdr.sh_type == mortar_elf::consts::SHT_SYMTAB);
                if let (Some(t), Some(x)) =
                    (symtab, image.global(&s.name).or_else(|| image.unique(&s.name)))
                    && x.sym.binding() == 10
                {
                    e.push((
                        (t.shdr.sh_offset + 24 * u64::from(x.index)) as usize + 4,
                        vec![global],
                    ));
                }
                c.push(("dynsym-unique", format!("'{}' unique -> global", s.name), e));
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
                copy_place(image, r.offset, s.st_size.max(1), &s.name, &mut c);
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
                c.push((
                    "gnu-hash",
                    format!("bucket {i} {v} -> {}", v + 1),
                    vec![(o, wr(u64::from(v) + 1, 4))],
                ));
            }
        }
        let ch = bk + 4 * u64::from(h.nbuckets);
        for (i, &v) in h.chain.iter().enumerate() {
            if let Some(o) = off(image, ch + 4 * i as u64, 4) {
                c.push((
                    "gnu-hash",
                    format!("chain {i}: end bit flipped"),
                    vec![(o, wr(u64::from(v ^ 1), 4))],
                ));
            }
        }
    }
    // .gnu.version: an import's index moved to another.
    if let (Some(vs), Some(addr)) = (&out.versym, out.get(DT_VERSYM)) {
        let max = vs.iter().copied().max().unwrap_or(1);
        for (j, &v) in vs.iter().enumerate().skip(1) {
            let to = if v >= max { 1 } else { v + 1 };
            if let Some(o) = off(image, addr + 2 * j as u64, 2) {
                c.push((
                    "versym",
                    format!("entry {j}: {v} -> {to}"),
                    vec![(o, wr(u64::from(to), 2))],
                ));
            }
        }
    }
    // .gnu.version_r: each Vernaux's hash and index.
    if let Some(mut vn) = out.get(DT_VERNEED).and_then(|a| off(image, a, 16)) {
        for _ in 0..out.get(DT_VERNEEDNUM).unwrap_or(0).min(64) {
            let (cnt, aux, next) =
                (rd(b, vn + 2, 2), rd(b, vn + 8, 4) as usize, rd(b, vn + 12, 4) as usize);
            let mut a = vn + aux;
            for _ in 0..cnt.min(64) {
                if a + 16 > b.len() {
                    break;
                }
                let (hash, other) = (rd(b, a, 4), rd(b, a + 6, 2));
                c.push((
                    "version-need",
                    format!("vernaux at {a:#x}: hash"),
                    vec![(a, wr(hash ^ 0x10, 4))],
                ));
                c.push((
                    "version-need",
                    format!("vernaux at {a:#x}: index {other} -> 1"),
                    vec![(a + 6, wr(1, 2))],
                ));
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
                        vec![
                            (h + 0x28, wr(p.p_memsz - d, 8)),
                            (h + 0x20, wr(p.p_filesz.saturating_sub(d), 8)),
                        ],
                    ));
                }
            }
        }
    }
    // The lazy words of JUMP_SLOT records (under -z now too: the shapes
    // and words stay, D-49, and P-D10 judges them there).
    for r in out.rels.iter().filter(|r| r.plt && kind(m, r.ty) == Kind::JumpSlot) {
        if let Some(o) = off(image, r.offset, 8) {
            let v = rd(b, o, 8);
            if v != 0 {
                c.push((
                    "lazy-slot",
                    format!("slot {:#x}: {v:#x} + 4", r.offset),
                    vec![(o, wr(v + 4, 8))],
                ));
            }
        }
    }
    plt_classes(image, &out, &mut c);
    if crate::spie::is_static_pie(image) {
        spie_classes(image, &out, &mut c);
    }
    c
}

/// The static-pie classes (`spec/plumb.md` §12.4, WP-089).
#[allow(clippy::too_many_lines, clippy::many_single_char_names)] // one block per class
fn spie_classes(image: &Image<'_>, out: &DynOut, c: &mut Vec<Cand>) {
    use crate::dynout::{DT_DEBUG, DT_RELACOUNT};
    let b = image.data;
    let m = image.header.e_machine;
    let recs = crate::spie::records(image);
    let dyn_off = image.phdrs.iter().find(|p| p.p_type == PT_DYNAMIC).map(|p| p.p_offset as usize);
    let entry = |tag: u64| -> Option<usize> {
        let i = out.entries.iter().position(|e| e.0 == tag)?;
        Some(dyn_off? + 16 * i)
    };
    // spie-rel-drop: a RELATIVE record removed for real: the records after
    // it in its section move down one, the section and DT_RELASZ shrink
    // by one record, and DT_RELACOUNT by one when the record was counted.
    let rela = out.get(DT_RELA).unwrap_or(0);
    let relasz = out.get(DT_RELASZ).unwrap_or(0);
    let count = out.get(DT_RELACOUNT);
    let shoff = image.header.e_shoff as usize;
    for (si, s) in image.sections.iter().enumerate() {
        if !(s.alloc() && s.shdr.sh_type == 4 && s.shdr.sh_addr == rela && s.shdr.sh_size == relasz)
        {
            continue;
        }
        let Some(so) = off(image, s.shdr.sh_addr, s.shdr.sh_size) else { continue };
        let n = (s.shdr.sh_size / 24) as usize;
        // At most 32 records, evenly spaced: each mutant moves the table.
        let rel: Vec<usize> = (0..n)
            .filter(|&i| kind(m, recs[&(s.shdr.sh_addr + 24 * i as u64)].ty) == Kind::Relative)
            .collect();
        let step = rel.len().div_ceil(32).max(1);
        for &i in rel.iter().step_by(step) {
            let r = recs[&(s.shdr.sh_addr + 24 * i as u64)];
            let mut moved = b[so + 24 * (i + 1)..so + 24 * n].to_vec();
            moved.extend_from_slice(&[0; 24]);
            let mut e = vec![(so + 24 * i, moved)];
            if let Some(o) = entry(DT_RELASZ) {
                e.push((o + 8, wr(relasz - 24, 8)));
            }
            if let (Some(k), Some(o)) = (count, entry(DT_RELACOUNT))
                && (i as u64) < k
            {
                e.push((o + 8, wr(k - 1, 8)));
            }
            e.push((shoff + 64 * si + 32, wr(s.shdr.sh_size - 24, 8)));
            c.push(("spie-rel-drop", format!("RELATIVE record for {:#x} removed", r.offset), e));
        }
    }
    // spie-rel-extra: an R_*_NONE record made a RELATIVE on a word of
    // .data no record writes, its addend the word's content (view 1 sees
    // nothing; only the address-word rule can).
    let written: std::collections::BTreeSet<u64> = recs.values().map(|r| r.offset).collect();
    let target = image.sections.iter().find(|s| s.name == ".data").and_then(|d| {
        (0..d.shdr.sh_size / 8)
            .map(|k| d.shdr.sh_addr.next_multiple_of(8) + 8 * k)
            .find(|a| !written.contains(a) && image.value(*a, 8).is_some_and(|v| v != 0))
    });
    let relative = match m {
        mortar_elf::consts::EM_AARCH64 => 1027,
        _ => 8,
    };
    if let Some(t) = target {
        let content = image.value(t, 8).unwrap_or(0);
        for r in recs.values().filter(|r| kind(m, r.ty) == Kind::None) {
            let Some(o) = off(image, r.at, 24) else { continue };
            let mut rec = wr(t, 8);
            rec.extend(wr(relative, 8));
            rec.extend(wr(content, 8));
            c.push((
                "spie-rel-extra",
                format!("NONE record at {:#x} made RELATIVE for {t:#x} (a .data word)", r.at),
                vec![(o, rec)],
            ));
        }
    }
    // spie-irel-twice: the C runtime's range made to cover an IRELATIVE
    // record, where .symtab defines both bounds.
    let sym = |n: &str| image.global(n).or_else(|| image.unique(n)).filter(|o| o.sym.st_shndx != 0);
    let symtab = image.sections.iter().find(|x| x.shdr.sh_type == mortar_elf::consts::SHT_SYMTAB);
    if let (Some(lo), Some(hi), Some(t)) =
        (sym("__rela_iplt_start"), sym("__rela_iplt_end"), symtab)
    {
        for r in recs.values().filter(|r| kind(m, r.ty) == Kind::Irelative).take(2) {
            let at = |x: u32| (t.shdr.sh_offset + 24 * u64::from(x)) as usize + 8;
            c.push((
                "spie-irel-twice",
                format!("__rela_iplt_start..end set to the IRELATIVE record at {:#x}", r.at),
                vec![(at(lo.index), wr(r.at, 8)), (at(hi.index), wr(r.at + 24, 8))],
            ));
        }
    }
    // spie-interp: a PT_NOTE made a PT_INTERP.
    let (phoff, phent) = (image.header.e_phoff as usize, usize::from(image.header.e_phentsize));
    if let Some(i) = image.phdrs.iter().position(|p| p.p_type == mortar_elf::consts::PT_NOTE) {
        c.push((
            "spie-interp",
            "a PT_NOTE made a PT_INTERP".into(),
            vec![(phoff + i * phent, wr(u64::from(PT_INTERP), 4))],
        ));
    }
    // spie-needed: DT_DEBUG made a DT_NEEDED of .dynstr's empty name.
    if let Some(o) = entry(DT_DEBUG) {
        c.push(("spie-needed", "DT_DEBUG made DT_NEEDED".into(), vec![(o, wr(DT_NEEDED, 8))]));
    }
    // spie-relacount: DT_RELACOUNT off by one either way.
    if let (Some(k), Some(o)) = (count, entry(DT_RELACOUNT)) {
        for (d, to) in [("+ 1", k + 1), ("- 1", k.saturating_sub(1))] {
            c.push(("spie-relacount", format!("DT_RELACOUNT {k} {d}"), vec![(o + 8, wr(to, 8))]));
        }
    }
    // spie-dynamic-word: the GOT's _DYNAMIC word changed.
    if let Some(g) = sym("_GLOBAL_OFFSET_TABLE_")
        && let Some(o) = off(image, g.sym.st_value, 8)
    {
        let v = rd(b, o, 8);
        for to in [0, v.wrapping_add(8)] {
            c.push((
                "spie-dynamic-word",
                format!("the word at _GLOBAL_OFFSET_TABLE_ {v:#x} -> {to:#x}"),
                vec![(o, wr(to, 8))],
            ));
        }
    }
}

/// The PLT header and the way back to it (WP-082): sites found by
/// P-D10's own walk of each lazy path (`plt::traces`); a site shared by
/// several paths (the header) is mutated once.
#[allow(clippy::many_single_char_names)] // instruction fields
fn plt_classes(image: &Image<'_>, out: &DynOut, c: &mut Vec<Cand>) {
    use crate::plt::Step;
    let b = image.data;
    let a64 = image.header.e_machine == mortar_elf::consts::EM_AARCH64;
    let mut done = std::collections::BTreeSet::new();
    // A little-endian i32 at `addr + at` changed by `d`.
    let rel = |addr: u64, at: u64, d: i64| -> Option<(usize, Vec<u8>)> {
        let o = off(image, addr + at, 4)?;
        let v = i64::from(i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]));
        Some((o, wr(v.wrapping_add(d) as u64, 4)))
    };
    // An A64 instruction's imm12 (bits 10..21) plus `d`.
    let imm12 = |addr: u64, d: u32| -> Option<(usize, Vec<u8>)> {
        let o = off(image, addr, 4)?;
        let w = u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let f = ((w >> 10) & 0xfff).wrapping_add(d) & 0xfff;
        Some((o, (w & !(0xfff << 10) | f << 10).to_le_bytes().to_vec()))
    };
    for (e, slot, trace) in crate::plt::traces(image, out) {
        for &(step, at, lazy) in &trace {
            let mut push = |class: &'static str, what: String, edit: Option<(usize, Vec<u8>)>| {
                if let Some(edit) = edit
                    && done.insert((class, at, edit.1.clone()))
                {
                    c.push((class, what, vec![edit]));
                }
            };
            match (step, lazy, a64) {
                (Step::PushWord, true, false) => {
                    for d in [8, -8] {
                        push(
                            "plt0-disp1",
                            format!("PLT0's push at {at:#x}: word {d:+}"),
                            rel(at, 2, d),
                        );
                    }
                }
                (Step::Ldr, true, true) => {
                    push(
                        "plt0-disp1",
                        format!("the header's ldr at {at:#x}: offset + 8"),
                        imm12(at, 1),
                    );
                }
                (Step::JmpInd, true, false) => {
                    for d in [8, -8] {
                        push(
                            "plt0-disp2",
                            format!("PLT0's jmp at {at:#x}: word {d:+}"),
                            rel(at, 2, d),
                        );
                    }
                }
                (Step::Add, true, true) => {
                    push("plt0-disp2", format!("the header's add at {at:#x}: + 8"), imm12(at, 8));
                }
                (Step::PushImm, true, false) => {
                    push(
                        "plt-index",
                        format!("the push at {at:#x} (entry {e:#x}): n + 1"),
                        rel(at, 1, 1),
                    );
                }
                (Step::MovR11, false, false) => {
                    push(
                        "plt-index",
                        format!("the mov at {at:#x} (entry {e:#x}): n + 1"),
                        rel(at, 2, 1),
                    );
                }
                (Step::Add, false, true) => {
                    push(
                        "plt-index",
                        format!("the entry {e:#x}'s add at {at:#x}: + 8 (slot {slot:#x})"),
                        imm12(at, 8),
                    );
                }
                (Step::Jmp, true, false) => {
                    for d in [16, -16] {
                        push(
                            "plt-back-jump",
                            format!("the jmp at {at:#x} (entry {e:#x}): {d:+}"),
                            rel(at, 1, d),
                        );
                    }
                }
                _ => {}
            }
        }
    }
}

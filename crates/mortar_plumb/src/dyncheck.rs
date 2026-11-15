//! Stage 3's static checks of a dynamic image (`spec/dyn.md` §12: P-D1,
//! P-D4..P-D8, P-D10..P-D13, and P-D9's record-order part). Where the
//! references differ by design (`dyn.md` §13.1) a reference output is
//! judged by its own linker's form; `Format::Mortar` is judged by the
//! D rules exactly (`spec/plumb.md` §12.4, "per-linker forms").

use std::collections::{BTreeMap, BTreeSet, HashMap};

use mortar_elf::consts::{
    EM_AARCH64, ET_DYN, ET_EXEC, PF_W, PT_DYNAMIC, PT_GNU_RELRO, PT_INTERP, PT_LOAD, PT_PHDR,
    STB_WEAK, STT_FUNC, STT_GNU_IFUNC, STT_SECTION, STV_PROTECTED,
};

use crate::dynmodel::Model;
use crate::dynout::{
    self, DF_1_NOW, DF_1_PIE, DF_BIND_NOW, DT_FLAGS, DT_FLAGS_1, DT_GNU_HASH, DT_JMPREL, DT_NEEDED,
    DT_NULL, DT_PLTREL, DT_PLTRELSZ, DT_RELA, DT_RELACOUNT, DT_RELAENT, DT_RELASZ, DT_RPATH,
    DT_RUNPATH, DT_STRSZ, DT_STRTAB, DT_SYMENT, DT_SYMTAB, DT_TEXTREL, DT_VERNEED, DT_VERNEEDNUM,
    DT_VERSYM, DynOut,
};
use crate::image::Image;
use crate::line::Line;
use crate::loader::{Bound, Kind, Loader, Params, kind};
use crate::locate::Obj;
use crate::map::{Format, OutputKind};
use crate::report::{Finding, Rule};

fn fd(n: u8, msg: impl Into<String>) -> Finding {
    Finding::new(Rule::Dyn(n), msg)
}

fn sym_finding(n: u8, name: &str, msg: impl Into<String>) -> Finding {
    let mut f = fd(n, msg);
    f.symbol = Some(name.to_string());
    f
}

/// The section of the output named `name`: `(addr, size)`.
fn sec(image: &Image<'_>, name: &str) -> Option<(u64, u64)> {
    image.sections.iter().find(|s| s.name == name).map(|s| (s.shdr.sh_addr, s.shdr.sh_size))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn check(
    image: &Image<'_>,
    out: &DynOut,
    line: &Line,
    model: &Model,
    ld: &Loader<'_>,
    objs: &[Obj<'_>],
    format: Format,
) -> Vec<Finding> {
    let mut v = Vec::new();
    for p in &out.problems {
        v.push(fd(6, p.clone()));
    }
    let _ = objs;
    pd1(image, out, line, &mut v);
    pd4(image, out, line, format, &mut v);
    pd5(image, out, format, &mut v);
    pd6(image, out, line, model, format, &mut v);
    pd7(image, out, model, ld, format, &mut v);
    pd8(out, model, ld, &mut v);
    pd9_order(out, ld.machine, &mut v);
    pd10(image, out, ld.machine, &mut v);
    pd11_13(image, out, model, ld, &mut v);
    pd12(out, ld, format, &mut v);
    v
}

fn pd1(image: &Image<'_>, out: &DynOut, line: &Line, v: &mut Vec<Finding>) {
    let h = &image.header;
    let interp = image.phdrs.iter().any(|p| p.p_type == PT_INTERP);
    let pie = out.get(DT_FLAGS_1).is_some_and(|f| f & DF_1_PIE != 0);
    let found = match (h.e_type, interp, pie) {
        (ET_DYN, true, true) => Some(OutputKind::Pie),
        (ET_DYN, false, _) => Some(OutputKind::StaticPie),
        (ET_EXEC, _, false) => Some(OutputKind::Exec),
        _ => None,
    };
    let want = line.kind.unwrap_or(OutputKind::Exec);
    if found != Some(want) && !(want == OutputKind::Exec && h.e_type == ET_EXEC) {
        v.push(fd(
            1,
            format!(
                "the headers (e_type {}, PT_INTERP {}, DF_1_PIE {}) are not the D-1 row of the command line's kind '{}'",
                h.e_type,
                interp,
                pie,
                want.name()
            ),
        ));
    }
    if want == OutputKind::Pie && h.e_type == ET_DYN && !pie {
        v.push(fd(1, "a PIE without DF_1_PIE in DT_FLAGS_1 (D-1)"));
    }
}

#[allow(clippy::too_many_lines)] // one block per segment
fn pd4(image: &Image<'_>, out: &DynOut, line: &Line, format: Format, v: &mut Vec<Finding>) {
    let h = &image.header;
    let okind = line.kind.unwrap_or(OutputKind::Exec);
    let dynamic_exec = matches!(okind, OutputKind::Exec | OutputKind::Pie);
    let first_load = image
        .phdrs
        .iter()
        .filter(|p| p.p_type == PT_LOAD)
        .min_by_key(|p| p.p_vaddr)
        .copied();
    // PT_PHDR.
    match image.phdrs.iter().find(|p| p.p_type == PT_PHDR) {
        Some(p) => {
            let size = u64::from(h.e_phnum) * u64::from(h.e_phentsize);
            // lld reserves room for one more header (its form).
            let covers = if format == Format::Mortar { p.p_filesz == size } else { p.p_filesz >= size };
            if p.p_offset != h.e_phoff || !covers {
                v.push(fd(4, "PT_PHDR does not cover exactly the program header table"));
            }
            if first_load
                .is_none_or(|l| p.p_vaddr < l.p_vaddr || p.p_vaddr + size > l.p_vaddr + l.p_filesz)
            {
                v.push(fd(4, "PT_PHDR is not inside the first PT_LOAD"));
            }
        }
        None if dynamic_exec => v.push(fd(4, "no PT_PHDR in a dynamic executable (D-61)")),
        None => {}
    }
    // PT_INTERP (W10).
    let pi = image.phdrs.iter().find(|p| p.p_type == PT_INTERP);
    match (pi, &line.interp) {
        (None, _) if dynamic_exec => v.push(fd(
            4,
            "no PT_INTERP in a dynamic executable: the kernel cannot start it (D-31, W10)",
        )),
        (Some(p), want) => {
            let s = image
                .bytes(p.p_vaddr, p.p_filesz)
                .map(|b| String::from_utf8_lossy(b.split(|&c| c == 0).next().unwrap_or(b)).into_owned());
            match (s, want) {
                (None, _) => v.push(fd(4, "PT_INTERP is not in the file image")),
                (Some(s), None) => v.push(fd(
                    4,
                    format!(
                        "PT_INTERP holds '{s}' and the command line names no -dynamic-linker: a linker default (D-31, W10)"
                    ),
                )),
                (Some(s), Some(w)) if &s != w => {
                    v.push(fd(4, format!("PT_INTERP holds '{s}', the command line names '{w}' (W10)")));
                }
                _ => {}
            }
            if let Some((a, z)) = sec(image, ".interp")
                && (a != p.p_vaddr || z != p.p_filesz)
            {
                v.push(fd(4, "PT_INTERP does not cover exactly .interp"));
            }
        }
        _ => {}
    }
    // PT_DYNAMIC: exactly .dynamic, writable.
    if let Some(p) = image.phdrs.iter().find(|p| p.p_type == PT_DYNAMIC) {
        if let Some((a, z)) = sec(image, ".dynamic")
            && (a != p.p_vaddr || z != p.p_memsz)
        {
            v.push(fd(4, "PT_DYNAMIC does not cover exactly .dynamic"));
        }
        let writable =
            image.load_of(p.p_vaddr, p.p_memsz.max(1)).is_some_and(|l| l.p_flags & PF_W != 0);
        if !writable {
            v.push(fd(4, ".dynamic is not in a writable PT_LOAD (D-33)"));
        }
    }
    // PT_GNU_RELRO.
    let relro: Vec<(u64, u64)> = image
        .phdrs
        .iter()
        .filter(|p| p.p_type == PT_GNU_RELRO)
        .map(|p| (p.p_vaddr, p.p_vaddr + p.p_memsz))
        .collect();
    let inside = |a: u64, z: u64| relro.iter().any(|&(lo, hi)| lo <= a && a + z <= hi);
    let now = out.get(DT_FLAGS).is_some_and(|f| f & DF_BIND_NOW != 0)
        || out.get(DT_FLAGS_1).is_some_and(|f| f & DF_1_NOW != 0)
        || out.has(dynout::DT_BIND_NOW);
    if !line.relro {
        if !relro.is_empty() {
            v.push(fd(4, "PT_GNU_RELRO under -z norelro"));
        }
    } else {
        if relro.is_empty() {
            v.push(fd(4, "no PT_GNU_RELRO (the line does not say -z norelro)"));
        }
        let mut must = vec![".dynamic", ".got", ".bss.rel.ro", ".data.rel.ro"];
        if line.now {
            must.push(".got.plt");
        }
        for n in must {
            if let Some((a, z)) = sec(image, n)
                && z > 0
                && !relro.is_empty()
                && !inside(a, z)
            {
                v.push(fd(4, format!("{n} is not inside PT_GNU_RELRO")));
            }
        }
        if format == Format::Mortar {
            let maxpage = if h.e_machine == EM_AARCH64 { 0x10000 } else { 0x1000 };
            for &(_, hi) in &relro {
                if hi % maxpage != 0 {
                    v.push(fd(4, "PT_GNU_RELRO does not end on a maximum page boundary (D-61)"));
                }
            }
        }
    }
    // A lazily bound slot must stay writable after RELRO.
    if !now {
        for r in out.rels.iter().filter(|r| r.plt && kind(image.header.e_machine, r.ty) == Kind::JumpSlot) {
            if inside(r.offset, 8) {
                v.push(
                    fd(
                        4,
                        "a lazily bound JUMP_SLOT word is inside PT_GNU_RELRO: the resolver cannot write it (D-49)",
                    )
                    .at(r.offset),
                );
            }
        }
    }
}

const ABSENT: &[&str] = &[
    "",
    "mortar_plumb_absent_name",
    "printf_",
    "_main",
    "__libc_start_main_",
    "a",
    "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
];

fn pd5(image: &Image<'_>, out: &DynOut, format: Format, v: &mut Vec<Finding>) {
    let Some(g) = &out.gnu_hash else {
        if !out.syms.is_empty() {
            v.push(fd(5, "no DT_GNU_HASH (D-20)"));
        }
        return;
    };
    let bits = if image.class() == mortar_elf::Class::Elf64 { 64 } else { 32 };
    for (j, s) in out.syms.iter().enumerate().skip(1) {
        let hashed = s.defined() || s.st_value != 0;
        if (j as u32) < g.symoffset {
            if hashed && s.defined() && s.sym_type() != STT_SECTION {
                v.push(sym_finding(
                    5,
                    &s.name,
                    format!(".dynsym entry {j} is defined and below symoffset {}: the loader cannot find it", g.symoffset),
                ));
            }
            continue;
        }
        if s.sym_type() == STT_SECTION || !hashed {
            continue;
        }
        match dynout::gnu_lookup(g, &out.syms, &s.name, bits) {
            Some(i) if i == j => {}
            Some(i) => v.push(sym_finding(5, &s.name, format!("the loader's lookup finds entry {i}, not {j}"))),
            None => v.push(sym_finding(5, &s.name, format!("the loader's lookup does not find .dynsym entry {j}"))),
        }
    }
    for a in ABSENT {
        if out.syms.iter().any(|s| s.name == *a) {
            continue;
        }
        if let Some(i) = dynout::gnu_lookup(g, &out.syms, a, bits) {
            v.push(fd(5, format!("the lookup of the absent name '{a}' finds entry {i}")));
        }
    }
    if format == Format::Mortar {
        let n = out.syms.len() as u32 - g.symoffset.min(out.syms.len() as u32);
        let nb = (n / 4).max(1);
        let w = bits;
        let mut mw = 1u32;
        while mw <= 12 * n / w {
            mw <<= 1;
        }
        if g.nbuckets != nb || g.bloom_shift != 26 || g.bloom_size != mw {
            v.push(fd(
                5,
                format!(
                    ".gnu.hash header (nbuckets {}, maskwords {}, shift {}) is not D-26's ({nb}, {mw}, 26)",
                    g.nbuckets, g.bloom_size, g.bloom_shift
                ),
            ));
        }
    }
}

#[allow(clippy::too_many_lines)] // one block per tag family
fn pd6(image: &Image<'_>, out: &DynOut, line: &Line, model: &Model, format: Format, v: &mut Vec<Finding>) {
    // DT_NEEDED: the set D-11 computes, in the linker's own order rule.
    let expect: &[usize] = match format {
        Format::Gnu | Format::Mortar => &model.order_d19a,
        Format::Lld | Format::Mold => &model.order_first,
    };
    let want: Vec<&str> = expect.iter().map(|&i| model.dsos[i].soname.as_str()).collect();
    let have: Vec<&str> = out.needed.iter().map(String::as_str).collect();
    let ws: BTreeSet<&str> = want.iter().copied().collect();
    let hs: BTreeSet<&str> = have.iter().copied().collect();
    for m in ws.difference(&hs) {
        v.push(fd(6, format!("DT_NEEDED lacks '{m}', which D-11 makes needed (W9)")));
    }
    for m in hs.difference(&ws) {
        v.push(fd(6, format!("DT_NEEDED names '{m}', which D-11 does not make needed")));
    }
    if ws == hs && want != have {
        v.push(fd(
            6,
            format!(
                "DT_NEEDED order is {have:?}; {} gives {want:?}",
                if matches!(format, Format::Gnu | Format::Mortar) {
                    "D-19a"
                } else {
                    "the first-mention order"
                }
            ),
        ));
    }
    // Values that name sections.
    let pairs: [(u64, &str, bool); 7] = [
        (DT_STRTAB, ".dynstr", false),
        (DT_SYMTAB, ".dynsym", false),
        (DT_GNU_HASH, ".gnu.hash", false),
        (DT_VERSYM, ".gnu.version", false),
        (DT_VERNEED, ".gnu.version_r", false),
        (DT_STRSZ, ".dynstr", true),
        (DT_JMPREL, ".rela.plt", false),
    ];
    for (tag, name, size) in pairs {
        if let (Some(val), Some((a, z))) = (out.get(tag), sec(image, name)) {
            let want = if size { z } else { a };
            if val != want {
                v.push(fd(6, format!("{} is {val:#x}, {name} is at {want:#x}", dynout::tag_name(tag))));
            }
        }
    }
    if let (Some(z), Some((_, s))) = (out.get(DT_PLTRELSZ), sec(image, ".rela.plt"))
        && z != s
    {
        v.push(fd(6, format!("DT_PLTRELSZ is {z:#x}, .rela.plt's size {s:#x}")));
    }
    if out.has(DT_JMPREL) && out.get(DT_PLTREL) != Some(DT_RELA) {
        v.push(fd(6, "DT_PLTREL is not DT_RELA"));
    }
    if out.has(DT_RELA) && out.get(DT_RELAENT) != Some(24) {
        v.push(fd(6, "DT_RELAENT is not 24"));
    }
    if out.has(DT_SYMTAB) && out.get(DT_SYMENT) != Some(24) {
        v.push(fd(6, "DT_SYMENT is not 24"));
    }
    if let Some(n) = out.get(DT_VERNEEDNUM)
        && n != out.verneeds.len() as u64
    {
        v.push(fd(6, format!("DT_VERNEEDNUM is {n}, the chain has {}", out.verneeds.len())));
    }
    if !out.has(dynout::DT_DEBUG) {
        v.push(fd(6, "no DT_DEBUG (D-30)"));
    }
    if out.has(DT_TEXTREL) || out.get(DT_FLAGS).is_some_and(|f| f & dynout::DF_TEXTREL != 0) {
        v.push(fd(6, "DT_TEXTREL: text relocations (D-47, W6)"));
    }
    // Flags.
    let kind = line.kind.unwrap_or(OutputKind::Exec);
    let f1 = out.get(DT_FLAGS_1).unwrap_or(0);
    let fl = out.get(DT_FLAGS).unwrap_or(0);
    if line.now {
        if fl & DF_BIND_NOW == 0 && !out.has(dynout::DT_BIND_NOW) {
            v.push(fd(6, "-z now and DT_FLAGS has no DF_BIND_NOW (D-49)"));
        }
        if f1 & DF_1_NOW == 0 {
            v.push(fd(6, "-z now and DT_FLAGS_1 has no DF_1_NOW (D-49)"));
        }
    } else if fl & DF_BIND_NOW != 0 || f1 & DF_1_NOW != 0 || out.has(dynout::DT_BIND_NOW) {
        v.push(fd(6, "DF_BIND_NOW or DF_1_NOW without -z now (D-49)"));
    }
    if (kind == OutputKind::Pie) != (f1 & DF_1_PIE != 0) {
        v.push(fd(6, "DF_1_PIE does not match the output kind (D-30)"));
    }
    // Run path.
    let rp = (!line.rpath.is_empty()).then(|| line.rpath.join(":"));
    let (tag, other) = if line.new_dtags { (DT_RUNPATH, DT_RPATH) } else { (DT_RPATH, DT_RUNPATH) };
    let got = out.get(tag).and_then(|o| out.string(image, o));
    if got != rp && !(format != Format::Mortar && rp.is_none() && got.is_none()) {
        v.push(fd(
            6,
            format!("{} is {got:?}, the command line gives {rp:?}", dynout::tag_name(tag)),
        ));
    }
    if out.has(other) && format == Format::Mortar {
        v.push(fd(6, format!("{} written (D-30)", dynout::tag_name(other))));
    }
    // D-30's order and six DT_NULL: mortar's form.
    if format == Format::Mortar {
        if out.nulls != 6 {
            v.push(fd(6, format!("{} DT_NULL entries, D-30 writes 6", out.nulls)));
        }
        let order = d30_rank();
        let ranks: Vec<usize> =
            out.entries.iter().filter(|e| e.0 != DT_NULL).map(|e| *order.get(&e.0).unwrap_or(&99)).collect();
        if ranks.windows(2).any(|w| w[0] > w[1]) {
            v.push(fd(6, "the .dynamic tags are not in D-30's order"));
        }
        if ranks.contains(&99) {
            v.push(fd(6, "a tag D-30 does not write"));
        }
    }
    let _ = (DT_RELASZ, DT_RELACOUNT);
}

fn d30_rank() -> HashMap<u64, usize> {
    use crate::dynout::*;
    [
        DT_NEEDED,
        DT_RUNPATH,
        DT_RPATH,
        DT_INIT,
        DT_FINI,
        DT_PREINIT_ARRAY,
        DT_PREINIT_ARRAYSZ,
        DT_INIT_ARRAY,
        DT_INIT_ARRAYSZ,
        DT_FINI_ARRAY,
        DT_FINI_ARRAYSZ,
        DT_GNU_HASH,
        DT_STRTAB,
        DT_SYMTAB,
        DT_STRSZ,
        DT_SYMENT,
        DT_DEBUG,
        DT_PLTGOT,
        DT_PLTRELSZ,
        DT_PLTREL,
        DT_JMPREL,
        DT_RELA,
        DT_RELASZ,
        DT_RELAENT,
        DT_AARCH64_BTI_PLT,
        DT_AARCH64_VARIANT_PCS,
        DT_FLAGS,
        DT_FLAGS_1,
        DT_VERNEED,
        DT_VERNEEDNUM,
        DT_VERSYM,
        DT_RELACOUNT,
    ]
    .iter()
    .enumerate()
    .map(|(i, t)| (*t, i))
    .collect()
}

/// P-D7: the `.dynsym` set and fields (D-21, D-23).
#[allow(clippy::too_many_lines)] // imports, exports, fields
fn pd7(image: &Image<'_>, out: &DynOut, model: &Model, ld: &Loader<'_>, format: Format, v: &mut Vec<Finding>) {
    let by_name: HashMap<&str, usize> =
        out.syms.iter().enumerate().skip(1).map(|(i, s)| (s.name.as_str(), i)).collect();
    let nu: BTreeSet<usize> = model.needed.union(&model.used).copied().collect();
    let order: Vec<usize> = ld.order.iter().copied().filter(|i| nu.contains(i)).collect();
    // Imports: referenced, not defined by the program, bound in C.
    for (name, k) in &model.keys {
        if k.defined || !(k.strong_ref || k.weak_ref) {
            continue;
        }
        let bound = model.bind_in(&order, name, k.version.as_deref());
        match (bound, by_name.get(name.as_str())) {
            (Some((i, d)), None) => {
                // A key resolved but absent from .dynsym: its uses must
                // then not need it; a reference that reaches it is P-D9's.
                if k.strong_ref {
                    v.push(sym_finding(
                        7,
                        name,
                        format!(
                            "'{name}' is defined by {} and referenced by the program, and is not in .dynsym (D-21)",
                            model.dsos[i].soname
                        ),
                    ));
                }
                let _ = d;
            }
            (None, Some(&j)) if !out.syms[j].defined() && k.strong_ref => {
                // W1: the reference linker bound a hidden entry.
                let hidden = model.dsos.iter().any(|d| {
                    d.defs.get(name).is_some_and(|x| x.iter().any(|y| !y.visible()))
                });
                v.push(sym_finding(
                    7,
                    name,
                    format!(
                        "'{name}' is imported and no needed or used object defines it visibly{}",
                        if hidden { " (only a STV_HIDDEN entry: W1)" } else { "" }
                    ),
                ));
            }
            (Some((i, d)), Some(&j)) => {
                let s = &out.syms[j];
                if !s.defined() {
                    if s.st_value == 0 {
                        let want_ty = if matches!(d.ty, STT_FUNC | STT_GNU_IFUNC) { STT_FUNC } else { d.ty };
                        if format == Format::Mortar && s.sym_type() != want_ty {
                            v.push(sym_finding(7, name, "an import's type is not its definition's (D-23)"));
                        }
                    }
                    let want_weak = !k.strong_ref;
                    if format == Format::Mortar && (s.binding() == STB_WEAK) != want_weak {
                        v.push(sym_finding(7, name, "an import's binding is not STB_WEAK iff no strong reference (D-23)"));
                    }
                    let _ = i;
                }
            }
            _ => {}
        }
    }
    // Exports: program definitions referenced (or defined) by N ∪ U, or
    // every one under -E.
    for (name, k) in &model.keys {
        if !k.defined || k.hidden {
            continue;
        }
        let want = model.ref_dynamic.contains(name) || model.def_dynamic.contains(name);
        if !want {
            continue;
        }
        let Some(&j) = by_name.get(name.as_str()) else {
            v.push(sym_finding(
                7,
                name,
                format!("'{name}' is defined by the program and referenced by a needed or used object, and is not exported (D-12, W8)"),
            ));
            continue;
        };
        let s = &out.syms[j];
        if !s.defined() && s.st_value == 0 {
            v.push(sym_finding(7, name, format!("'{name}' is exported undefined")));
        }
    }
    // An export of an IFUNC at the implementation, not the IPLT entry:
    // the .symtab says the resolver.
    for s in out.syms.iter().skip(1) {
        if !s.defined() {
            continue;
        }
        if let Some(o) = image.global(&s.name)
            && o.sym.sym_type() == STT_GNU_IFUNC
            && s.sym_type() == STT_FUNC
            && s.st_value == o.sym.st_value
        {
            v.push(sym_finding(
                7,
                &s.name,
                format!("'{}' is exported at its IFUNC resolver, not a canonical IPLT entry (W13)", s.name),
            ));
        }
    }
    if format == Format::Mortar {
        for s in out.syms.iter().skip(1) {
            if matches!(s.name.as_str(), "_DYNAMIC" | "_GLOBAL_OFFSET_TABLE_" | "__dso_handle")
                || s.sym_type() == STT_SECTION
                || s.binding() == mortar_elf::consts::STB_LOCAL
                || matches!(s.visibility(), 1 | 2)
            {
                v.push(sym_finding(7, &s.name, format!("'{}' is never in .dynsym (D-21)", s.name)));
            }
        }
        let mut seen = BTreeSet::new();
        for s in out.syms.iter().skip(1) {
            if !seen.insert(&s.name) {
                v.push(sym_finding(7, &s.name, "a name twice in .dynsym"));
            }
        }
    }
}

/// P-D8: versions.
fn pd8(out: &DynOut, model: &Model, ld: &Loader<'_>, v: &mut Vec<Finding>) {
    let Some(vs) = &out.versym else { return };
    let mut others = BTreeSet::new();
    for n in &out.needs {
        if !others.insert(n.other) {
            v.push(fd(8, format!("vna_other {} twice", n.other)));
        }
        if n.hash != dynout::elf_hash(n.name.as_bytes()) {
            v.push(fd(8, format!("vna_hash of '{}' is not its ELF hash", n.name)));
        }
        if !out.needed.contains(&n.file) {
            v.push(fd(8, format!("vn_file '{}' is not a DT_NEEDED name", n.file)));
        }
    }
    for (j, s) in out.syms.iter().enumerate().skip(1) {
        let Some(&x) = vs.get(j) else { break };
        if x & 0x8000 != 0 {
            v.push(sym_finding(8, &s.name, "a .gnu.version entry with bit 15 set (D-27)"));
        }
        if s.defined() || x & 0x7fff < 2 {
            continue;
        }
        let Some(need) = out.need_of(j) else {
            v.push(sym_finding(8, &s.name, format!("version index {x} names no Vernaux")));
            continue;
        };
        // What the program asked for: a `.symver` version, else the
        // default version of the definition the loader binds.
        let k = model.keys.get(&s.name);
        let asked = k.and_then(|k| k.version.clone());
        let bound = ld
            .model
            .bind_in(&ld.order, &s.name, asked.as_deref())
            .or_else(|| ld.model.lookup(&ld.order, &s.name, asked.as_deref()));
        let Some((i, d)) = bound else { continue };
        let want = asked.clone().or_else(|| d.version.clone());
        if want.as_deref() != Some(need.name.as_str()) {
            v.push(sym_finding(
                8,
                &s.name,
                format!(
                    "'{}' requires version '{}'; the program's reference asks for {:?} (resolve §9.2)",
                    s.name, need.name, want
                ),
            ));
        }
        if model.dsos[i].soname != need.file {
            v.push(sym_finding(
                8,
                &s.name,
                format!(
                    "'{}@{}' is required of '{}'; the loader binds it in '{}'",
                    s.name, need.name, need.file, model.dsos[i].soname
                ),
            ));
        }
    }
}

/// P-D9 (D-40): `RELATIVE` records lead `.rela.dyn`, sorted by place, and
/// `DT_RELACOUNT` counts them.
fn pd9_order(out: &DynOut, machine: u16, v: &mut Vec<Finding>) {
    let dynr: Vec<_> = out.rels.iter().filter(|r| !r.plt).collect();
    let lead = dynr.iter().take_while(|r| kind(machine, r.ty) == Kind::Relative).count();
    let total = dynr.iter().filter(|r| kind(machine, r.ty) == Kind::Relative).count();
    if lead != total {
        v.push(fd(9, "a RELATIVE record after a record of another type in .rela.dyn (D-40)"));
    }
    if dynr[..lead].windows(2).any(|w| w[0].offset > w[1].offset) {
        v.push(fd(9, "the RELATIVE records are not sorted by place (D-40)"));
    }
    if let Some(c) = out.get(DT_RELACOUNT)
        && c != lead as u64
    {
        v.push(fd(9, format!("DT_RELACOUNT is {c}; {lead} RELATIVE records lead .rela.dyn (D-40)")));
    }
}

/// P-D10: PLT entries, their words, the lazy word, IRELATIVE order (H1).
fn pd10(image: &Image<'_>, out: &DynOut, machine: u16, v: &mut Vec<Finding>) {
    let entries = crate::loader::plt_entries(image);
    let mut by_slot: BTreeMap<u64, Vec<(usize, Kind)>> = BTreeMap::new();
    let jmprel: Vec<_> = out.rels.iter().filter(|r| r.plt).collect();
    for (i, r) in out.rels.iter().enumerate() {
        by_slot.entry(r.offset).or_default().push((i, kind(machine, r.ty)));
    }
    let mut slots_reached = BTreeSet::new();
    // PLT0 jumps through `.got.plt[2]`, which the loader fills.
    let resolver = out.get(dynout::DT_PLTGOT).map(|g| g + 16);
    for (&e, &slot) in &entries {
        if Some(slot) == resolver {
            continue;
        }
        slots_reached.insert(slot);
        let rs = by_slot.get(&slot).map_or(&[][..], Vec::as_slice);
        let n = rs
            .iter()
            .filter(|(_, k)| matches!(k, Kind::JumpSlot | Kind::Irelative | Kind::GlobDat))
            .count();
        if n != 1 {
            v.push(
                fd(10, format!("the PLT entry jumps through {slot:#x}, which has {n} JUMP_SLOT/IRELATIVE/GLOB_DAT records"))
                    .at(e),
            );
        }
    }
    let now = out.get(DT_FLAGS).is_some_and(|f| f & DF_BIND_NOW != 0)
        || out.get(DT_FLAGS_1).is_some_and(|f| f & DF_1_NOW != 0);
    let plt0 = sec(image, ".plt").map(|s| s.0);
    for (idx, r) in jmprel.iter().enumerate() {
        if kind(machine, r.ty) != Kind::JumpSlot {
            continue;
        }
        if !slots_reached.contains(&r.offset) {
            v.push(fd(10, "a JUMP_SLOT word no PLT entry jumps through").at(r.offset));
        }
        if now {
            continue;
        }
        let Some(lazy) = image.value(r.offset, 8) else { continue };
        let ok = if machine == EM_AARCH64 {
            Some(lazy) == plt0
        } else {
            x64_lazy_ok(image, &entries, r.offset, lazy, idx as u64, plt0)
        };
        if !ok {
            let mut f = fd(10, "the JUMP_SLOT word's initial content is not the lazy target (D-49)").at(r.offset);
            f.found = Some(lazy);
            v.push(f);
        }
    }
    // H1: every IRELATIVE after every JUMP_SLOT, in application order.
    let last_js = out.rels.iter().rposition(|r| kind(machine, r.ty) == Kind::JumpSlot);
    if let Some(l) = last_js
        && let Some(ir) = out.rels.iter().position(|r| kind(machine, r.ty) == Kind::Irelative)
        && ir < l
    {
        v.push(
            fd(10, "an IRELATIVE record is applied before a JUMP_SLOT: a resolver that calls through the PLT jumps to an unbound slot (D-41, H1)")
                .at(out.rels[ir].offset),
        );
    }
}

/// x86-64's lazy target: `entry + 6`, whose `push $i; jmp PLT0` names
/// this record (GNU ld, lld), or PLT0 with `mov $i, %r11d` in the entry
/// (mold).
fn x64_lazy_ok(
    image: &Image<'_>,
    entries: &BTreeMap<u64, u64>,
    slot: u64,
    lazy: u64,
    idx: u64,
    plt0: Option<u64>,
) -> bool {
    if let Some(b) = image.bytes(lazy, 10) {
        let mut i = 0;
        if b[..4] == [0xf3, 0x0f, 0x1e, 0xfa] {
            i = 4;
        }
        if b[i] == 0x68 {
            let n = u32::from_le_bytes([b[i + 1], b[i + 2], b[i + 3], b[i + 4]]);
            return u64::from(n) == idx;
        }
    }
    if Some(lazy) == plt0 {
        return entries.iter().filter(|(_, s)| **s == slot).any(|(&e, _)| {
            image.bytes(e, 10).is_some_and(|b| {
                let i = if b[..4] == [0xf3, 0x0f, 0x1e, 0xfa] { 4 } else { 0 };
                b[i..i + 2] == [0x41, 0xbb]
                    && u64::from(u32::from_le_bytes([b[i + 2], b[i + 3], b[i + 4], b[i + 5]])) == idx
            })
        });
    }
    false
}

/// P-D11 (copies) and P-D13 (protected).
#[allow(clippy::too_many_lines)] // one block per property
fn pd11_13(image: &Image<'_>, out: &DynOut, model: &Model, ld: &Loader<'_>, v: &mut Vec<Finding>) {
    let p = Params::default();
    let relro: Vec<(u64, u64)> = image
        .phdrs
        .iter()
        .filter(|q| q.p_type == PT_GNU_RELRO)
        .map(|q| (q.p_vaddr, q.p_vaddr + q.p_memsz))
        .collect();
    let mut allocs: Vec<(u64, u64, String)> = Vec::new();
    let mut copies: BTreeMap<u64, (usize, u64, String)> = BTreeMap::new();
    for r in &out.rels {
        if kind(ld.machine, r.ty) != Kind::Copy {
            continue;
        }
        let Some(s) = out.syms.get(r.sym as usize) else { continue };
        let b = ld.bind_copy(out, r.sym as usize, &p);
        let Bound::Lib { i, .. } = b else { continue };
        let ver = out.need_of(r.sym as usize).map(|n| n.name.as_str());
        let Some((_, d)) = model.lookup(&ld.order, &s.name, ver) else { continue };
        if d.visibility() == STV_PROTECTED {
            v.push(sym_finding(13, &s.name, format!("a COPY of '{}', whose definition is STV_PROTECTED (D-18, W5)", s.name)).at(r.offset));
        }
        if s.st_size == 0 || d.size == 0 {
            v.push(sym_finding(11, &s.name, format!("a COPY of '{}' of size 0 (W4)", s.name)).at(r.offset));
        }
        if s.st_size != d.size {
            let mut f = sym_finding(11, &s.name, format!("the copy of '{}' has size {}, its definition {}", s.name, s.st_size, d.size)).at(r.offset);
            f.expected = Some(d.size);
            f.found = Some(s.st_size);
            v.push(f);
        }
        let align = model.dsos[i].copy_align(d.shndx, d.value);
        if !r.offset.is_multiple_of(align) {
            v.push(sym_finding(11, &s.name, format!("the copy of '{}' at {:#x} is not aligned to {align} (D-42)", s.name, r.offset)).at(r.offset));
        }
        let ro = model.dsos[i].read_only_at(d.value);
        let in_relro = relro.iter().any(|&(lo, hi)| lo <= r.offset && r.offset + s.st_size.max(1) <= hi);
        if ro != in_relro {
            v.push(sym_finding(
                11,
                &s.name,
                format!(
                    "the copy of '{}' is {} RELRO; its definition is {} (D-42)",
                    s.name,
                    if in_relro { "inside" } else { "outside" },
                    if ro { "read-only" } else { "writable" }
                ),
            ).at(r.offset));
        }
        if image.load_of(r.offset, s.st_size.max(1)).is_none_or(|l| l.p_flags & PF_W == 0) {
            v.push(sym_finding(11, &s.name, "a copy outside writable memory").at(r.offset));
        }
        // Aliases at the same (file, value) are defined at the copy (W2).
        for (name, defs) in &model.dsos[i].defs {
            if name == &s.name {
                continue;
            }
            if !defs.iter().any(|x| x.visible() && x.value == d.value && x.shndx != 0) {
                continue;
            }
            let ok = out.syms.iter().any(|y| &y.name == name && y.defined() && y.st_value == r.offset);
            if !ok {
                v.push(sym_finding(
                    11,
                    name,
                    format!("'{name}', an alias of the copied '{}', is not defined at the copy (D-21, W2)", s.name),
                ).at(r.offset));
                v.push(sym_finding(7, name, format!("the copy alias '{name}' is not in .dynsym (W2)")));
            }
        }
        allocs.push((r.offset, s.st_size, s.name.clone()));
        copies.insert(r.offset, (i, d.value, s.name.clone()));
    }
    // A definition of the executable that no input gives must be a copy
    // or an alias of one (W4: GNU ld defines a zero-size symbol at
    // another symbol's copy).
    for s in out.syms.iter().skip(1) {
        if !s.defined() || model.keys.get(&s.name).is_some_and(|k| k.defined) {
            continue;
        }
        if s.sym_type() == STT_SECTION || s.st_shndx == mortar_elf::consts::SHN_ABS {
            continue;
        }
        let Some((i, value, copied)) = copies.get(&s.st_value) else {
            if model.dsos.iter().any(|d| d.def(&s.name, None).is_some()) {
                v.push(sym_finding(
                    11,
                    &s.name,
                    format!("'{}' is defined by no input and is not at a copy: the libraries' definition is replaced (D-42)", s.name),
                ));
            }
            continue;
        };
        let same = model.dsos[*i].defs.get(&s.name).is_some_and(|x| x.iter().any(|y| y.visible() && y.value == *value));
        if !same {
            v.push(sym_finding(
                11,
                &s.name,
                format!(
                    "'{}' is defined at the copy of '{copied}', which its own definition does not alias: the program and the libraries read another object's bytes (D-42, W4)",
                    s.name
                ),
            ).at(s.st_value));
        }
    }
    allocs.sort();
    for w in allocs.windows(2) {
        if w[0].0 + w[0].1 > w[1].0 {
            v.push(sym_finding(11, &w[1].2, format!("the copies of '{}' and '{}' overlap", w[0].2, w[1].2)).at(w[1].0));
        }
    }
    // Overlap with another definition of the executable.
    for (a, z, n) in &allocs {
        for o in &image.symbols {
            if o.sym.st_shndx == 0
                || o.sym.st_size == 0
                || o.sym.st_value == *a
                || o.sym.sym_type() == STT_SECTION
                || matches!(o.sym.sym_type(), mortar_elf::consts::STT_TLS)
            {
                continue;
            }
            if o.sym.st_value < a + z && *a < o.sym.st_value + o.sym.st_size {
                v.push(sym_finding(11, n, format!("the copy of '{n}' overlaps '{}'", o.name)).at(*a));
            }
        }
    }
    // P-D13: a canonical PLT of a protected function.
    for (j, s) in out.syms.iter().enumerate().skip(1) {
        if s.defined() || s.st_value == 0 {
            continue;
        }
        let ver = out.need_of(j).map(|n| n.name.as_str());
        if let Some((_, d)) = model.lookup(&ld.order, &s.name, ver)
            && d.visibility() == STV_PROTECTED
        {
            v.push(sym_finding(13, &s.name, format!("a canonical PLT of '{}', whose definition is STV_PROTECTED (D-18, W5)", s.name)));
        }
    }
}

/// P-D12: TLS records.
fn pd12(out: &DynOut, ld: &Loader<'_>, format: Format, v: &mut Vec<Finding>) {
    let p = Params::default();
    for r in &out.rels {
        let k = kind(ld.machine, r.ty);
        if !matches!(k, Kind::TpOff | Kind::DtpMod | Kind::DtpOff | Kind::TlsDesc) {
            continue;
        }
        if r.sym == 0 {
            if format == Format::Mortar {
                v.push(fd(12, "a TLS record against the executable's own block (D-48: local-exec)").at(r.offset));
            }
            continue;
        }
        let Some(s) = out.syms.get(r.sym as usize) else { continue };
        match ld.bind(out, r.sym as usize, false, &p) {
            Bound::Lib { tls: false, .. } => v.push(sym_finding(
                12,
                &s.name,
                format!("a TLS record against '{}', whose definition is not STT_TLS (D-16)", s.name),
            ).at(r.offset)),
            Bound::Exec(_) if format == Format::Mortar => v.push(sym_finding(
                12,
                &s.name,
                "a TLS record against an executable-defined symbol (D-48)",
            ).at(r.offset)),
            Bound::Missing => v.push(sym_finding(12, &s.name, "a TLS record against a name no object defines").at(r.offset)),
            _ => {}
        }
        if k == Kind::TlsDesc && format == Format::Mortar {
            v.push(sym_finding(12, &s.name, "a TLSDESC record (D-48: descriptors are relaxed)").at(r.offset));
        }
    }
    let _ = DT_NEEDED;
}

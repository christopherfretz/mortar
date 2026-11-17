//! The field rules WP-101 added (`spec/plumb.md` §12.3, "The field rules
//! of WP-101"): what the byte sweep showed no rule judged once mortar's
//! build id stopped answering every flip. Each comes from `spec/dyn.md`,
//! the gABI or the psABI; the ones marked **(mortar)** hold only
//! `Format::Mortar`, where `spec/dyn.md` gives mortar a single answer.
// ELF fields read best by the specs' short names.
#![allow(clippy::many_single_char_names)]

use mortar_elf::consts::{
    EM_AARCH64, EM_X86_64, STB_GLOBAL, STB_WEAK, STT_FUNC, STT_GNU_IFUNC, STT_NOTYPE,
};

use crate::dynmodel::Model;
use crate::dynout::{self, DT_NULL, DynOut};
use crate::image::Image;
use crate::line::Line;
use crate::loader::{Kind, Loader, kind};
use crate::locate::Obj;
use crate::map::{Format, OutputKind};
use crate::report::{Finding, Rule};

fn fd(n: u8, msg: impl Into<String>) -> Finding {
    Finding::new(Rule::Dyn(n), msg)
}

fn sym_fd(n: u8, name: &str, msg: impl Into<String>) -> Finding {
    let mut f = fd(n, msg);
    f.symbol = Some(name.to_string());
    f
}

/// Every rule of this file.
#[allow(clippy::too_many_arguments)]
pub(crate) fn check(
    image: &Image<'_>,
    out: &DynOut,
    line: &Line,
    model: &Model,
    ld: &Loader<'_>,
    objs: &[Obj<'_>],
    format: Format,
    v: &mut Vec<Finding>,
) {
    versym_zero(out, v);
    if format != Format::Mortar {
        return;
    }
    if line.kind != Some(OutputKind::StaticPie) {
        imports(image, out, line, model, ld, objs, v);
        copies(image, out, model, ld, v);
        got_header(image, out, v);
    }
    bloom(image, out, v);
    spare_nulls(image, out, v);
    places(image, out, v);
    got_plt_header(image, out, v);
    tables(image, out, v);
    a64_entry_add(image, v);
}

/// P-D10 **(mortar)**, a64.md §10.3 and §10.4: every AArch64 PLT and IPLT
/// entry of mortar's is `[bti c;] adrp x16, G; ldr x17, [x16, :lo12:G];
/// add x16, x16, :lo12:G; br x17`. §12.2's walk judges the `adrp`, `ldr`
/// and `br` of every entry and the `add` where the lazy path reads `x16`;
/// an IPLT entry's `x16` reaches only the IFUNC's chosen function, where
/// `IP0` is a scratch register (AAPCS64), so its `add` is held to the
/// shape here, by mortar's single answer. (GNU ld and lld write the same
/// `add`, mold none.)
fn a64_entry_add(image: &Image<'_>, v: &mut Vec<Finding>) {
    if image.header.e_machine != EM_AARCH64 {
        return;
    }
    let mut seen = std::collections::BTreeSet::new();
    for (&e, &slot) in &crate::loader::plt_entries(image) {
        let mut a = e;
        if image.insn(a).is_some_and(crate::a64::bti) {
            a += 4;
        }
        if !seen.insert(a) {
            continue;
        }
        let Some(w) = image.insn(a + 8) else { continue };
        let want = slot & 0xfff;
        match crate::a64::add_imm(w) {
            Some((16, 16, imm)) if imm == want => {}
            _ => {
                let mut f = fd(
                    10,
                    format!(
                        "the PLT entry at {e:#x} has {w:#010x} where a64.md §10.3 puts `add x16, x16, #{want:#x}` (the low 12 bits of its word {slot:#x})"
                    ),
                )
                .at(a + 8);
                f.found = Some(u64::from(w));
                v.push(f);
            }
        }
    }
}

/// P-D10 **(mortar)**, D-32 and D-49: `.got.plt`'s three reserved words
/// in every kind (a static-pie's included, which §12.2's walk does not
/// reach): x86-64 `[0]` is `_DYNAMIC`, AArch64's 0, and `[1]`, `[2]` 0; no
/// record writes one; `DT_PLTGOT` is `.got.plt`'s address (D-30 row 17).
fn got_plt_header(image: &Image<'_>, out: &DynOut, v: &mut Vec<Finding>) {
    let m = image.header.e_machine;
    if m != EM_AARCH64 && m != EM_X86_64 {
        return;
    }
    let Some(g) = image.sections.iter().find(|s| s.name == ".got.plt") else { return };
    let at = g.shdr.sh_addr;
    if let Some(p) = out.get(dynout::DT_PLTGOT)
        && p != at
    {
        let mut f = fd(10, format!("DT_PLTGOT is {p:#x}, .got.plt is at {at:#x} (D-30)"));
        f.expected = Some(at);
        f.found = Some(p);
        v.push(f);
    }
    if g.shdr.sh_size < 24 {
        return;
    }
    let dyn_at = out.dynamic.map_or(0, |d| d.0);
    for k in 0..3u64 {
        let a = at + 8 * k;
        let want = if k == 0 && m == EM_X86_64 { dyn_at } else { 0 };
        let w = image.value(a, 8);
        if w != Some(want) {
            let mut f = fd(10, format!(".got.plt[{k}] does not hold {want:#x} (D-32, D-49)")).at(a);
            f.expected = Some(want);
            f.found = w;
            v.push(f);
        }
        if out.rels.iter().any(|r| r.offset == a) {
            v.push(fd(10, format!("a record writes .got.plt[{k}], a reserved word (D-49)")).at(a));
        }
    }
}

/// **(mortar)** D-26 and D-41: `.gnu.hash`'s `symoffset` is the index of
/// the first hashed entry (the `.dynsym` count with none); a `JUMP_SLOT`
/// record is in `DT_JMPREL` only (P-D5, P-D10).
fn tables(image: &Image<'_>, out: &DynOut, v: &mut Vec<Finding>) {
    if let Some(g) = &out.gnu_hash {
        let first = out
            .syms
            .iter()
            .enumerate()
            .skip(1)
            .find(|(_, s)| s.defined() || s.st_value != 0)
            .map_or(out.syms.len(), |(j, _)| j);
        if g.symoffset as usize != first {
            let mut f = fd(
                5,
                format!(
                    ".gnu.hash symoffset is {}; the first hashed entry is {first} (D-26)",
                    g.symoffset
                ),
            );
            f.expected = Some(first as u64);
            f.found = Some(u64::from(g.symoffset));
            v.push(f);
        }
    }
    let m = image.header.e_machine;
    for r in out.rels.iter().filter(|r| !r.plt && kind(m, r.ty) == Kind::JumpSlot) {
        v.push(fd(10, "a JUMP_SLOT record outside DT_JMPREL (D-41)").at(r.offset));
    }
}

/// P-D8 (gABI "Symbol Versioning", `VER_NDX_LOCAL`): a defined global or
/// weak entry with
/// version index 0 is local to the object, not available to another (musl
/// skips it; glibc does not), so an export with it is a definition the
/// program means to give and a loader may not. Every format.
fn versym_zero(out: &DynOut, v: &mut Vec<Finding>) {
    let Some(vs) = &out.versym else { return };
    for (j, s) in out.syms.iter().enumerate().skip(1) {
        // A local entry (GNU ld's `STT_SECTION` ones) is index 0 by right.
        if s.defined()
            && s.binding() != mortar_elf::consts::STB_LOCAL
            && vs.get(j).is_some_and(|&x| x.trailing_zeros() >= 15)
        {
            v.push(sym_fd(
                8,
                &s.name,
                format!(
                    "'{}' is defined with version index 0 (VER_NDX_LOCAL: not available outside the executable, gABI)",
                    s.name
                ),
            ));
        }
    }
}

/// P-D7 and P-D8 **(mortar)**: an import's fields are D-23's row and its
/// version index D-27's: `st_size` 0; `STB_WEAK` exactly when no input
/// references it strongly; the type `STT_FUNC` for a function (or IFUNC)
/// definition or a canonical PLT, else the definition's, `STT_NOTYPE`
/// when nothing defines it (`WeakDyn`); `STO_AARCH64_VARIANT_PCS`
/// exactly when the definition has it, or (`WeakDyn`) a live reference;
/// version index 1 when the binding is unversioned.
fn imports(
    image: &Image<'_>,
    out: &DynOut,
    line: &Line,
    model: &Model,
    ld: &Loader<'_>,
    objs: &[Obj<'_>],
    v: &mut Vec<Finding>,
) {
    let aarch64 = image.header.e_machine == EM_AARCH64;
    for (j, s) in out.syms.iter().enumerate().skip(1) {
        if s.defined() {
            continue;
        }
        let Some(key) = model.keys.get(&s.name) else { continue };
        let mut bad = Vec::new();
        if s.st_size != 0 {
            bad.push(format!("st_size {} (D-23: 0)", s.st_size));
        }
        let strong = key.strong_ref || line.undefined.contains(&s.name);
        let want_bind = if strong { STB_GLOBAL } else { STB_WEAK };
        if s.binding() != want_bind {
            bad.push(format!(
                "binding {} (D-23: {want_bind}, the {} reference)",
                s.binding(),
                if strong { "strong" } else { "weak-only" }
            ));
        }
        let asked = key.version.as_deref();
        // In `Shared` the closure is not searched (input §8.6) and
        // `bind(k)` ranges over N alone (D-10, WP-137): an import N does
        // not define is D-111 item 3's `STT_NOTYPE`.
        let def = ld
            .model
            .bind_in(&ld.order, &s.name, asked)
            .or_else(|| {
                if ld.model.shared { None } else { ld.model.lookup(&ld.order, &s.name, asked) }
            })
            .map(|(_, d)| d);
        let want_type = match def {
            _ if s.st_value != 0 => STT_FUNC,
            Some(d) if d.ty == STT_FUNC || d.ty == STT_GNU_IFUNC => STT_FUNC,
            Some(d) => d.ty,
            None => STT_NOTYPE,
        };
        if s.sym_type() != want_type {
            bad.push(format!("type {} (D-23: {want_type})", s.sym_type()));
        }
        if aarch64 {
            let want_vpcs = match def {
                Some(d) => d.other & 0x80 != 0,
                None => objs.iter().any(|o| {
                    o.symtab.is_some_and(|t| {
                        t.iter().any(|(_, x)| {
                            x.st_shndx == 0
                                && x.st_other & 0x80 != 0
                                && t.name(&x).is_ok_and(|n| n == s.name.as_bytes())
                        })
                    })
                }),
            };
            if (s.st_other & 0x80 != 0) != want_vpcs {
                bad.push(format!(
                    "STO_AARCH64_VARIANT_PCS {} (D-23: {})",
                    if want_vpcs { "missing" } else { "set" },
                    if def.is_some() { "the definition's" } else { "a live reference's" }
                ));
            }
        }
        if !bad.is_empty() {
            v.push(sym_fd(7, &s.name, format!("the import '{}' has {}", s.name, bad.join(", "))));
        }
        // D-27: unversioned is index 1 (0 is gABI's VER_NDX_LOCAL).
        if let Some(&x) = out.versym.as_ref().and_then(|vs| vs.get(j))
            && x == 0
        {
            v.push(sym_fd(
                8,
                &s.name,
                format!("the import '{}' has version index 0 (D-27: 1 when unversioned)", s.name),
            ));
        }
    }
}

/// P-D7 **(mortar)**, D-23's copy row: a defined `.dynsym` entry at a
/// `COPY` record's place (the copy and each alias of it) has the shared
/// definition's type and `st_size`, and `STV_DEFAULT`. (The binding is
/// left to the existing export rules: the model does not keep a shared
/// definition's binding.)
fn copies(image: &Image<'_>, out: &DynOut, model: &Model, ld: &Loader<'_>, v: &mut Vec<Finding>) {
    let m = image.header.e_machine;
    let places: std::collections::BTreeSet<u64> =
        out.rels.iter().filter(|r| kind(m, r.ty) == Kind::Copy).map(|r| r.offset).collect();
    if places.is_empty() {
        return;
    }
    for s in out.syms.iter().skip(1) {
        if !s.defined() || !places.contains(&s.st_value) {
            continue;
        }
        let asked = model.keys.get(&s.name).and_then(|k| k.version.as_deref());
        let Some((_, d)) = ld
            .model
            .bind_in(&ld.order, &s.name, asked)
            .or_else(|| ld.model.lookup(&ld.order, &s.name, asked))
        else {
            continue;
        };
        let mut bad = Vec::new();
        if s.sym_type() != d.ty {
            bad.push(format!("type {} (the definition's {})", s.sym_type(), d.ty));
        }
        if s.st_size != d.size {
            bad.push(format!("st_size {} (the definition's {})", s.st_size, d.size));
        }
        if s.st_other & 3 != 0 {
            bad.push(format!("visibility {} (STV_DEFAULT)", s.st_other & 3));
        }
        if !bad.is_empty() {
            v.push(sym_fd(
                7,
                &s.name,
                format!("the copy '{}' has {} (D-23, the copy row)", s.name, bad.join(", ")),
            ));
        }
    }
}

/// P-D10 **(mortar)**, D-32: AArch64's `.got[0]` holds `_DYNAMIC`'s
/// link-time address and no record writes it (x86-64's `.got.plt[0]` is
/// §12.2's reserved word, every format).
fn got_header(image: &Image<'_>, out: &DynOut, v: &mut Vec<Finding>) {
    if image.header.e_machine != EM_AARCH64 {
        return;
    }
    let (Some((dyn_at, _)), Some(g)) =
        (out.dynamic, image.sections.iter().find(|s| s.name == ".got"))
    else {
        return;
    };
    if g.shdr.sh_size < 8 {
        return;
    }
    let at = g.shdr.sh_addr;
    let w = image.value(at, 8);
    if w != Some(dyn_at) {
        let mut f = fd(10, ".got[0] does not hold _DYNAMIC (D-32)").at(at);
        f.expected = Some(dyn_at);
        f.found = w;
        v.push(f);
    }
    if out.rels.iter().any(|r| r.offset == at) {
        v.push(
            fd(10, "a record writes .got[0], which holds _DYNAMIC's link-time address (D-32)")
                .at(at),
        );
    }
}

/// P-D5 **(mortar)**, D-26: the Bloom words are exactly the two bits of
/// each hashed entry (a cleared bit is every format's: the lookup of the
/// name fails; a bit no entry sets only sends more lookups to the
/// chains, so only mortar's table is held to none).
fn bloom(image: &Image<'_>, out: &DynOut, v: &mut Vec<Finding>) {
    let Some(g) = &out.gnu_hash else { return };
    let w: u32 = if image.class() == mortar_elf::Class::Elf64 { 64 } else { 32 };
    if g.bloom_size == 0 || g.bloom.len() != g.bloom_size as usize {
        return;
    }
    let mut want = vec![0u64; g.bloom.len()];
    for s in out.syms.iter().skip(g.symoffset as usize) {
        let h = dynout::gnu_hash(s.name.as_bytes());
        let word = ((h / w) % g.bloom_size) as usize;
        want[word] |= 1u64 << (h % w);
        want[word] |= 1u64 << ((h >> g.bloom_shift.min(31)) % w);
    }
    for (i, (&have, &want)) in g.bloom.iter().zip(&want).enumerate() {
        if have != want {
            let mut f = fd(
                5,
                format!(
                    ".gnu.hash Bloom word {i} is {have:#x}; D-26's bits of the hashed names give {want:#x}"
                ),
            );
            f.expected = Some(want);
            f.found = Some(have);
            v.push(f);
        }
    }
}

/// P-D6 **(mortar)**, D-30 row 32: the six `DT_NULL` entries are all zero
/// (the terminator's value is every format's; the spare entries' values
/// no loader reads, gABI: `d_un` of `DT_NULL` is ignored).
fn spare_nulls(image: &Image<'_>, out: &DynOut, v: &mut Vec<Finding>) {
    let Some((at, size)) = out.dynamic else { return };
    let e = if image.class() == mortar_elf::Class::Elf64 { 16u64 } else { 8 };
    let half = (e / 2) as u8;
    let first = out.entries.len().saturating_sub(1) as u64;
    let mut k = first;
    while (k + 1) * e <= size {
        let a = at + k * e;
        let (Some(tag), Some(val)) = (image.value(a, half), image.value(a + e / 2, half)) else {
            break;
        };
        if tag != DT_NULL {
            break;
        }
        if val != 0 {
            v.push(
                fd(6, format!("DT_NULL entry {k} has the value {val:#x} (D-30: 0)")).at(a + e / 2),
            );
        }
        k += 1;
    }
}

/// P-D9 **(mortar)**, D-46: under RELA a record's place holds, in the
/// file, `S + A` for `RELATIVE` (the record's addend) and 0 for
/// `GLOB_DAT` and the symbolic word type (AAELF64 `R_AARCH64_ABS64`,
/// x86-64 `R_X86_64_64`), and D-48's TLS slot 0, and both words of a
/// `TLSDESC` record's descriptor 0 (D-119). Every RELA type of the
/// two machines but a lazy `JUMP_SLOT` ignores the place's contents
/// (AAELF64 and the x86-64 psABI compute each from S, A and the base),
/// so for another linker the bytes are equivalent (§12.3's table).
fn places(image: &Image<'_>, out: &DynOut, v: &mut Vec<Finding>) {
    let m = image.header.e_machine;
    if m != EM_AARCH64 && m != EM_X86_64 {
        return;
    }
    for r in &out.rels {
        let k = kind(m, r.ty);
        let want = match k {
            // The IPLT word (a64.md §10.2 and §10.4: the resolver's
            // address; x64.md X-39: 0), in either table.
            Kind::Irelative if m == EM_AARCH64 => r.addend.unwrap_or(0) as u64,
            Kind::Irelative => 0,
            _ if r.plt => continue,
            Kind::Relative => r.addend.unwrap_or(0) as u64,
            Kind::GlobDat | Kind::Abs | Kind::TpOff => 0,
            // D-119: a descriptor's two words, place 0 under RELA (the
            // loader writes both; WP-161, the sweep's survivors).
            Kind::TlsDesc => {
                for at in [r.offset, r.offset + 8] {
                    let Some(have) = image.value(at, 8) else { continue };
                    if have != 0 {
                        let mut f = fd(
                            9,
                            format!(
                                "word {} of a TlsDesc record's descriptor holds {have:#x} in the file; D-119 writes 0",
                                (at - r.offset) / 8
                            ),
                        )
                        .at(at);
                        f.expected = Some(0);
                        f.found = Some(have);
                        v.push(f);
                    }
                }
                continue;
            }
            _ => continue,
        };
        let Some(have) = image.value(r.offset, 8) else { continue };
        if have != want {
            let mut f = fd(
                9,
                format!(
                    "the place of a {k:?} record holds {have:#x} in the file; {} writes {want:#x}",
                    match (k, m) {
                        (Kind::Irelative, EM_AARCH64) => "a64.md §10.2",
                        (Kind::Irelative, _) => "x64.md X-39",
                        _ => "D-46",
                    }
                ),
            )
            .at(r.offset);
            f.expected = Some(want);
            f.found = Some(have);
            v.push(f);
        }
    }
}

/// P-D9 (WP-101), every format: a `RELATIVE` record on a word outside the
/// placed sections that no input's absolute use makes an address is
/// derived from a GOT slot an input demands (D-40, D-46): its place is a
/// word of `.got` and its addend the `S + A` some GOT-indirect reference
/// of the inputs asks a slot to hold, relaxed or not, or an IPLT entry
/// (D-45: an IFUNC's slot holds its entry). `None` when it is; else why
/// not.
pub(crate) fn linker_word(
    image: &Image<'_>,
    offset: u64,
    addend: u64,
    demands: &std::collections::BTreeSet<u64>,
) -> Option<String> {
    let word = if image.class() == mortar_elf::Class::Elf64 { 8 } else { 4 };
    let got = image.sections.iter().find(|s| {
        s.name == ".got"
            && s.shdr.sh_addr <= offset
            && offset < s.shdr.sh_addr.saturating_add(s.shdr.sh_size)
    });
    let Some(got) = got else {
        return Some(
            "a RELATIVE record on a linker-made word outside .got: no input relocation and no GOT slot demands it (D-40)"
                .into(),
        );
    };
    if !(offset - got.shdr.sh_addr).is_multiple_of(word) {
        return Some("a RELATIVE record on no word of .got (the place is not a slot)".into());
    }
    let iplt = image.sections.iter().any(|s| {
        s.name == ".iplt" && s.shdr.sh_addr <= addend && addend < s.shdr.sh_addr + s.shdr.sh_size
    });
    if demands.contains(&addend) || iplt {
        None
    } else {
        Some(format!(
            "a RELATIVE record on a .got slot with addend {addend:#x}, which no GOT-indirect reference of the inputs asks a slot to hold (D-46)"
        ))
    }
}

/// The IPLT entries (D-45): every PLT-like entry (`loader::plt_entries`)
/// whose word carries an `IRELATIVE` record, at its first word (a `bti
/// c` before the `adrp` included).
pub(crate) fn iplt_entries(image: &Image<'_>, out: &DynOut) -> std::collections::BTreeSet<u64> {
    let m = image.header.e_machine;
    let irel: std::collections::BTreeSet<u64> =
        out.rels.iter().filter(|r| kind(m, r.ty) == Kind::Irelative).map(|r| r.offset).collect();
    crate::loader::plt_entries(image)
        .into_iter()
        .filter(|(_, slot)| irel.contains(slot))
        .map(|(e, _)| e)
        .collect()
}

/// P-D9 **(mortar)**, D-46: a `.got` word no record writes (AArch64's
/// `.got[0]` aside, `got_header`'s) holds what an input's GOT reference
/// asks of a slot: `S` of a local or absolute target in `Exec`, 0 of a
/// `Weak0` one, a thread-pointer offset; a word nothing asks for is a slot
/// without a demand.
pub(crate) fn unrecorded_got(
    image: &Image<'_>,
    out: &DynOut,
    demands: &std::collections::BTreeSet<u64>,
) -> Vec<Finding> {
    let mut v = Vec::new();
    let m = image.header.e_machine;
    let Some(g) = image.sections.iter().find(|s| s.name == ".got") else { return v };
    if g.shdr.sh_type == mortar_elf::consts::SHT_NOBITS {
        return v;
    }
    let (lo, hi) = (g.shdr.sh_addr, g.shdr.sh_addr.saturating_add(g.shdr.sh_size));
    let mut written = std::collections::BTreeSet::new();
    for r in &out.rels {
        written.insert(r.offset);
        if matches!(kind(m, r.ty), Kind::TlsDesc) {
            written.insert(r.offset + 8);
        }
    }
    let mut at = lo;
    while at + 8 <= hi {
        let first = at == lo && m == EM_AARCH64;
        if !first && !written.contains(&at) {
            let w = image.value(at, 8).unwrap_or(0);
            if !demands.contains(&w) {
                let mut f = fd(
                    9,
                    format!(
                        "the .got word at {at:#x} holds {w:#x} and no record writes it; no GOT-indirect reference of the inputs asks a slot to hold that (D-46)"
                    ),
                )
                .at(at);
                f.found = Some(w);
                v.push(f);
            }
        }
        at += 8;
    }
    v
}

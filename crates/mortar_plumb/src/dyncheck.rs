//! Stage 3's static checks of a dynamic image (`spec/plumb.md` §12: P-D1,
//! P-D4..P-D8, P-D10..P-D13, P-D24, and P-D9's record-order part). Where the
//! references differ by design (`dyn.md` §13.1) a reference output is
//! judged by its own linker's form; `Format::Mortar` is judged by the
//! D rules exactly (`spec/plumb.md` §12.1, "a reference to its own
//! form"). A static-pie's own rules are `crate::spie`'s (§12.4).
// ELF fields and addresses read best by the specs' short names.
#![allow(clippy::many_single_char_names, clippy::similar_names)]

use std::collections::{BTreeMap, BTreeSet, HashMap};

use mortar_elf::consts::{
    EM_AARCH64, ET_DYN, ET_EXEC, PF_W, PT_DYNAMIC, PT_GNU_RELRO, PT_INTERP, PT_LOAD, PT_PHDR,
    STB_WEAK, STT_FUNC, STT_GNU_IFUNC, STT_SECTION, STV_PROTECTED,
};

use crate::dynmodel::Model;
use crate::dynout::{
    self, DF_1_NOW, DF_1_PIE, DF_BIND_NOW, DT_FINI_ARRAY, DT_FINI_ARRAYSZ, DT_FLAGS, DT_FLAGS_1,
    DT_GNU_HASH, DT_INIT_ARRAY, DT_INIT_ARRAYSZ, DT_JMPREL, DT_NEEDED, DT_NULL, DT_PLTREL,
    DT_PLTRELSZ, DT_PREINIT_ARRAY, DT_PREINIT_ARRAYSZ, DT_RELA, DT_RELACOUNT, DT_RELAENT,
    DT_RELASZ, DT_RPATH, DT_RUNPATH, DT_STRSZ, DT_STRTAB, DT_SYMENT, DT_SYMTAB, DT_TEXTREL,
    DT_VERNEED, DT_VERNEEDNUM, DT_VERSYM, DynOut,
};
use crate::image::Image;
use crate::line::Line;
use crate::loader::{Bound, Kind, Loader, Params, kind};
use crate::locate::Obj;
use crate::map::{Format, Map, OutputKind};
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
    map: &Map,
) -> Vec<Finding> {
    let format = map.format;
    let mut v = Vec::new();
    for p in &out.problems {
        v.push(fd(6, p.clone()));
    }
    pd1(image, out, line, &mut v);
    pd4(image, out, line, map, &mut v);
    if let Some((m, at)) = stack(image, line.execstack, objs, format) {
        v.push(fd(4, m).at(at));
    }
    pd5(image, out, format, &mut v);
    pd6(image, out, line, model, format, &mut v);
    pd6_shlib_undefined(model, line, &mut v);
    let spie = line.kind == Some(OutputKind::StaticPie);
    if spie {
        // §12.4: a static-pie's .dynsym and records are P-D14's.
        crate::spie::check(
            image,
            out,
            line.entry.as_deref(),
            // A reference's `--dynamic-list` exports too (D-1: no effect
            // in mortar's; WP-161).
            line.export_dynamic || line.export_symbols || !line.dynamic_lists.is_empty(),
            format,
            &mut v,
        );
    } else {
        pd7(image, out, model, ld, format, &mut v);
        // P-D14's entry rule for an Exec or a Pie (WP-088).
        crate::spie::entry_point(image, line.entry.as_deref(), 4, &mut v);
    }
    pd8(out, model, ld, &mut v);
    pd9_order(out, ld.machine, &mut v);
    pd10(image, out, ld.machine, format, spie, &mut v);
    let copies = pd11_13(image, out, model, ld, format, &mut v);
    pd24(out, line, model, ld, &mut v);
    if format == Format::Mortar {
        // M8 (§3.3): the map's copy records are the output's copies.
        v.extend(crate::maprec::copies(map, &copies));
    }
    pd12(image, out, ld, objs, format, &mut v);
    fields(image, out, model, line, objs, format, &mut v);
    crate::dynfields::check(image, out, line, model, ld, objs, format, &mut v);
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
                "the headers (e_type {}, PT_INTERP {}, DF_1_PIE {}) are not the D-1 row of the command line's kind '{}'{}",
                h.e_type,
                interp,
                pie,
                want.name(),
                if !interp && line.interp.is_none() {
                    " (no -dynamic-linker, no PT_INTERP: D-31, W10)"
                } else {
                    ""
                }
            ),
        ));
    }
    if want == OutputKind::Pie && h.e_type == ET_DYN && !pie {
        v.push(fd(1, "a PIE without DF_1_PIE in DT_FLAGS_1 (D-1)"));
    }
}

/// P-D4, `PT_GNU_STACK`'s `PF_X` (WP-088): the line's last `-z execstack`
/// sets it and `-z noexecstack` clears it; with neither it is set only
/// when an input object's `.note.GNU-stack` is `SHF_EXECINSTR` (GNU ld
/// and mortar set it then, lld and mold never: under mortar's map it is
/// set exactly then). An executable stack the link did not ask for is a
/// permission the program does not need; one it asked for and did not
/// get faults in the code that needed it (a nested function's
/// trampoline). With no `PT_GNU_STACK` nothing is judged here. The
/// same rule is S10's for a static output of mortar's map (§3.2a).
pub(crate) fn stack(
    image: &Image<'_>,
    execstack: Option<bool>,
    objs: &[Obj<'_>],
    format: Format,
) -> Option<(String, u64)> {
    let p = image.phdrs.iter().find(|p| p.p_type == mortar_elf::consts::PT_GNU_STACK)?;
    let x = p.p_flags & mortar_elf::consts::PF_X != 0;
    let note = objs.iter().find(|o| {
        o.secs.iter().any(|s| {
            s.name == ".note.GNU-stack" && s.shdr.sh_flags & mortar_elf::consts::SHF_EXECINSTR != 0
        })
    });
    let why = match (execstack, x) {
        (Some(true), false) => Some("PT_GNU_STACK is not PF_X under -z execstack".to_string()),
        (Some(false), true) => Some("PT_GNU_STACK is PF_X under -z noexecstack".to_string()),
        (None, true) if note.is_none() => Some(
            "PT_GNU_STACK is PF_X: no -z execstack and no input's .note.GNU-stack is executable"
                .to_string(),
        ),
        (None, false) if format == Format::Mortar => note.map(|o| {
            format!(
                "PT_GNU_STACK is not PF_X and {}'s .note.GNU-stack is executable (L-84)",
                o.label()
            )
        }),
        _ => None,
    };
    why.map(|m| (m, p.p_vaddr))
}

#[allow(clippy::too_many_lines)] // one block per segment
fn pd4(image: &Image<'_>, out: &DynOut, line: &Line, map: &Map, v: &mut Vec<Finding>) {
    let format = map.format;
    let h = &image.header;
    let okind = line.kind.unwrap_or(OutputKind::Exec);
    let dynamic_exec = matches!(okind, OutputKind::Exec | OutputKind::Pie);
    let first_load =
        image.phdrs.iter().filter(|p| p.p_type == PT_LOAD).min_by_key(|p| p.p_vaddr).copied();
    // PT_PHDR.
    match image.phdrs.iter().find(|p| p.p_type == PT_PHDR) {
        Some(p) => {
            let size = u64::from(h.e_phnum) * u64::from(h.e_phentsize);
            // lld reserves room for one more header (its form).
            let covers =
                if format == Format::Mortar { p.p_filesz == size } else { p.p_filesz >= size };
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
        (None, None) if dynamic_exec => v.push(fd(
            4,
            "no PT_INTERP in a dynamic executable and the command line names no -dynamic-linker: the kernel cannot start it (D-31, W10)",
        )),
        (None, Some(_)) if dynamic_exec => v.push(fd(
            4,
            "no PT_INTERP in a dynamic executable: the kernel cannot start it (D-31)",
        )),
        (Some(p), want) => {
            let s = image.bytes(p.p_vaddr, p.p_filesz).map(|b| {
                String::from_utf8_lossy(b.split(|&c| c == 0).next().unwrap_or(b)).into_owned()
            });
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
    // ld.so protects [start, end) with both rounded down to the page
    // (`_dl_protect_relro`): what must be read-only is inside the
    // rounded-down range (4 KiB, the smallest page of both machines).
    let inside = |a: u64, z: u64| relro.iter().any(|&(lo, hi)| lo <= a && a + z <= hi & !0xfff);
    let now = out.get(DT_FLAGS).is_some_and(|f| f & DF_BIND_NOW != 0)
        || out.get(DT_FLAGS_1).is_some_and(|f| f & DF_1_NOW != 0)
        || out.has(dynout::DT_BIND_NOW);
    if line.relro {
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
        // maxpage is S2's: the line's last `-z max-page-size` (0: the
        // default); `-z common-page-size` changes nothing (L-74, L-75).
        if let Some(o) = crate::opts::of(map) {
            let default = if h.e_machine == EM_AARCH64 { 0x1_0000 } else { 0x1000 };
            let maxpage = o.max_page.filter(|&n| n != 0).unwrap_or(default);
            for &(_, hi) in &relro {
                if hi % maxpage != 0 {
                    v.push(fd(
                        4,
                        format!(
                            "PT_GNU_RELRO ends at {hi:#x}, not on a maximum page boundary \
                             (maxpage {maxpage:#x}; D-61)"
                        ),
                    ));
                }
            }
        }
    } else if !relro.is_empty() {
        v.push(fd(4, "PT_GNU_RELRO under -z norelro"));
    }
    // A lazily bound slot must stay writable after RELRO.
    if !now {
        for r in out
            .rels
            .iter()
            .filter(|r| r.plt && kind(image.header.e_machine, r.ty) == Kind::JumpSlot)
        {
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
        // glibc's `do_lookup_x` checks each chain entry's version
        // (`check_match`): the lookup of entry j is by its name and its
        // `.gnu.version` index, so `foo@V1` (hidden) and `foo@@V2` are
        // each found (D-115; WP-121).
        let ver = |i: usize| out.versym.as_ref().and_then(|x| x.get(i)).map(|x| x & 0x7fff);
        match dynout::gnu_lookup_by(g, &out.syms, &s.name, bits, |i| ver(i) == ver(j)) {
            Some(i) if i == j => {}
            Some(i) => v.push(sym_finding(
                5,
                &s.name,
                format!("the loader's lookup finds entry {i}, not {j}"),
            )),
            None => v.push(sym_finding(
                5,
                &s.name,
                format!("the loader's lookup does not find .dynsym entry {j}"),
            )),
        }
    }
    // The chains' shape: entries grouped by bucket, each bucket word the
    // first entry of its group, the end bit set exactly on a group's last
    // entry (a lookup of a name absent from a group walks to that bit).
    let nb = g.nbuckets.max(1);
    let n = out.syms.len();
    let bucket = |j: usize| dynout::gnu_hash(out.syms[j].name.as_bytes()) % nb;
    for (k, &c) in g.chain.iter().enumerate() {
        let j = g.symoffset as usize + k;
        if j >= n {
            break;
        }
        let last = j + 1 == n || bucket(j + 1) != bucket(j);
        if (c & 1 == 1) != last {
            v.push(sym_finding(
                5,
                &out.syms[j].name,
                format!(
                    ".gnu.hash chain word of entry {j} {} the end bit; its bucket's group {} there",
                    if c & 1 == 1 { "has" } else { "lacks" },
                    if last { "ends" } else { "does not end" }
                ),
            ));
        }
        if (c | 1) != (dynout::gnu_hash(out.syms[j].name.as_bytes()) | 1) {
            v.push(sym_finding(
                5,
                &out.syms[j].name,
                format!(".gnu.hash chain word of entry {j} is not its name's hash"),
            ));
        }
        let first = j == g.symoffset as usize || bucket(j - 1) != bucket(j);
        if first && g.buckets.get(bucket(j) as usize) != Some(&(j as u32)) {
            v.push(sym_finding(
                5,
                &out.syms[j].name,
                format!("entry {j} starts its bucket's group; the bucket word does not name it"),
            ));
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

/// P-D6, D-13 (WP-088, dyn.md W16): unless `--allow-shlib-undefined`,
/// every strong undefined reference of a member of N ∪ U is satisfied by
/// a non-hidden program definition or by a visible definition, of the
/// version it requires, in a member of N ∪ U. A link that leaves one
/// unsatisfied is one D-13 refuses; lld and mold make it, dropping the
/// library that defines it from `DT_NEEDED`, and the program fails at
/// load (`symbol lookup error`, exit 127). One finding per name.
fn pd6_shlib_undefined(model: &Model, line: &Line, v: &mut Vec<Finding>) {
    if line.allow_shlib_undefined {
        return;
    }
    let set: BTreeSet<usize> = model.needed.union(&model.used).copied().collect();
    let mut seen = BTreeSet::new();
    for &i in &set {
        for r in model.dsos[i].refs.iter().filter(|r| !r.weak) {
            let key = model.keys.get(&r.name);
            // An executable's definition `--exclude-libs` takes out of
            // every export is as a hidden one (D-110 item 1, WP-161).
            let excluded = !model.shared && key.is_some_and(|k| k.defined && k.excluded);
            if key.is_some_and(|k| k.defined && !k.hidden) && !excluded
                || set.iter().any(|&j| model.dsos[j].def(&r.name, r.version.as_deref()).is_some())
                || !seen.insert(r.name.clone())
            {
                continue;
            }
            v.push(sym_finding(
                6,
                &r.name,
                if excluded {
                    format!(
                        "'{}', a strong reference of {} (in N ∪ U), is defined by no member of N ∪ U, and the program's definition is in an archive member --exclude-libs names, which no export gives, D-110 item 1 (D-13: the link is refused)",
                        r.name, model.dsos[i].path
                    )
                } else {
                    format!(
                        "'{}', a strong reference of {} (in N ∪ U), is defined by no member of N ∪ U and not by the program (D-13: the link is refused)",
                        r.name, model.dsos[i].path
                    )
                },
            ));
        }
    }
}

#[allow(clippy::too_many_lines)] // one block per tag family
fn pd6(
    image: &Image<'_>,
    out: &DynOut,
    line: &Line,
    model: &Model,
    format: Format,
    v: &mut Vec<Finding>,
) {
    // DT_NEEDED: the set D-11 computes, in the linker's own order rule.
    // A reference is held to its own neededness where D-11 records it:
    // GNU ld leaves out an object that binds nothing (`gnu_unbound`, either
    // way), mold adds the givers of weak-only keys (E-shlib 2.2a).
    let have: Vec<&str> = out.needed.iter().map(String::as_str).collect();
    let expect: Vec<usize> = match format {
        Format::Mortar => model.order_d19a.clone(),
        Format::Gnu => model
            .order_d19a
            .iter()
            .copied()
            .filter(|i| {
                !model.gnu_unbound.contains(i) || have.contains(&model.dsos[*i].soname.as_str())
            })
            .collect(),
        Format::Lld => model.order_first.clone(),
        Format::Mold => model
            .c
            .iter()
            .copied()
            .filter(|i| model.needed.contains(i) || model.mold_weak.contains(i))
            .collect(),
    };
    let want: Vec<&str> = expect.iter().map(|&i| model.dsos[i].soname.as_str()).collect();
    let ws: BTreeSet<&str> = want.iter().copied().collect();
    let hs: BTreeSet<&str> = have.iter().copied().collect();
    for m in ws.difference(&hs) {
        let item3 = model.by_soname(m).is_some_and(|i| model.item3.contains(&i));
        v.push(fd(
            6,
            if item3 {
                format!(
                    "DT_NEEDED lacks '{m}', which D-11 item 3 makes needed (an under-linked library's provider; E-shlib 2.3b)"
                )
            } else {
                format!("DT_NEEDED lacks '{m}', which D-11 makes needed (W9)")
            },
        ));
    }
    for m in hs.difference(&ws) {
        // W1: the linker took a STV_HIDDEN entry for a definition.
        let hidden = model.by_soname(m).is_some_and(|i| {
            model.dsos[i].defs.iter().any(|(name, defs)| {
                defs.iter().any(|d| !d.visible())
                    && model.keys.get(name).is_some_and(|k| k.strong_ref && !k.defined)
            })
        });
        v.push(fd(
            6,
            format!(
                "DT_NEEDED names '{m}', which D-11 does not make needed{}",
                if hidden { " (only a STV_HIDDEN entry: W1)" } else { "" }
            ),
        ));
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
                v.push(fd(
                    6,
                    format!("{} is {val:#x}, {name} is at {want:#x}", dynout::tag_name(tag)),
                ));
            }
        }
    }
    // Address and size pairs the loader walks.
    for (at, sz, name) in [
        (DT_RELA, DT_RELASZ, ".rela.dyn"),
        (DT_INIT_ARRAY, DT_INIT_ARRAYSZ, ".init_array"),
        (DT_FINI_ARRAY, DT_FINI_ARRAYSZ, ".fini_array"),
        (DT_PREINIT_ARRAY, DT_PREINIT_ARRAYSZ, ".preinit_array"),
    ] {
        if let (Some(a), Some(z)) = (out.get(at), out.get(sz))
            && let Some((sa, ss)) = sec(image, name)
            && (a, z) != (sa, ss)
        {
            v.push(fd(
                6,
                format!(
                    "{} {a:#x}, {} {z:#x}: {name} is [{sa:#x}, +{ss:#x})",
                    dynout::tag_name(at),
                    dynout::tag_name(sz)
                ),
            ));
        }
    }
    // ld.so applies the first DT_RELACOUNT records as RELATIVE, whatever
    // their type.
    if let Some(n) = out.get(DT_RELACOUNT) {
        let m = image.header.e_machine;
        let lead = out
            .rels
            .iter()
            .filter(|r| !r.plt)
            .take_while(|r| kind(m, r.ty) == Kind::Relative)
            .count() as u64;
        if n > lead || (format == Format::Mortar && n != lead) {
            v.push(fd(
                6,
                format!(
                    "DT_RELACOUNT is {n}, the table starts with {lead} RELATIVE records (D-40)"
                ),
            ));
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
    if matches!(kind, OutputKind::Pie | OutputKind::StaticPie) != (f1 & DF_1_PIE != 0) {
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
        let ranks: Vec<usize> = out
            .entries
            .iter()
            .filter(|e| e.0 != DT_NULL)
            .map(|e| *order.get(&e.0).unwrap_or(&99))
            .collect();
        if ranks.windows(2).any(|w| w[0] > w[1]) {
            v.push(fd(6, "the .dynamic tags are not in D-30's order"));
        }
        if ranks.contains(&99) {
            v.push(fd(6, "a tag D-30 does not write"));
        }
    }
}

#[allow(clippy::wildcard_imports)] // every tag of D-30's table
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
fn pd7(
    image: &Image<'_>,
    out: &DynOut,
    model: &Model,
    ld: &Loader<'_>,
    format: Format,
    v: &mut Vec<Finding>,
) {
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
                let hidden = model
                    .dsos
                    .iter()
                    .any(|d| d.defs.get(name).is_some_and(|x| x.iter().any(|y| !y.visible())));
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
                        let want_ty =
                            if matches!(d.ty, STT_FUNC | STT_GNU_IFUNC) { STT_FUNC } else { d.ty };
                        if format == Format::Mortar && s.sym_type() != want_ty {
                            v.push(sym_finding(
                                7,
                                name,
                                "an import's type is not its definition's (D-23)",
                            ));
                        }
                    }
                    let want_weak = !k.strong_ref;
                    if format == Format::Mortar && (s.binding() == STB_WEAK) != want_weak {
                        v.push(sym_finding(
                            7,
                            name,
                            "an import's binding is not STB_WEAK iff no strong reference (D-23)",
                        ));
                    }
                    let _ = i;
                }
            }
            _ => {}
        }
    }
    // Exports: program definitions referenced (or defined) by N ∪ U, and
    // those the line's `--dynamic-list` lists (resolve §9.6). A definition
    // `--exclude-libs` excludes is exported by nothing (D-110 item 1; a
    // shared object's reference to it is P-D6's D-13 finding).
    for (name, k) in &model.keys {
        if !k.defined || k.hidden {
            continue;
        }
        // (A `Shared` output's export set is P-D19's.)
        if k.excluded && !model.shared {
            if let Some(&j) = by_name.get(name.as_str())
                && out.syms[j].defined()
            {
                v.push(sym_finding(
                    7,
                    name,
                    format!("'{name}' is exported, and its definition is in an archive member --exclude-libs names, which no export gives (D-110 item 1)"),
                ));
            }
            continue;
        }
        let dynamic = model.ref_dynamic.contains(name) || model.def_dynamic.contains(name);
        if !dynamic && !k.listed {
            continue;
        }
        let Some(&j) = by_name.get(name.as_str()) else {
            v.push(sym_finding(
                7,
                name,
                if dynamic {
                    format!("'{name}' is defined by the program and referenced by a needed or used object, and is not exported (D-12, W8)")
                } else {
                    format!("'{name}' is defined by the program and listed by the line's --dynamic-list, and is not exported (resolve.md §9.6, D-21)")
                },
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
        if !s.defined() || s.sym_type() == STT_SECTION {
            continue;
        }
        // gABI: a hidden or internal definition is made local by the link
        // editor; a global one in .dynsym is a lookup's answer.
        if matches!(s.visibility(), 1 | 2) && s.binding() != mortar_elf::consts::STB_LOCAL {
            v.push(sym_finding(
                7,
                &s.name,
                format!(
                    "'{}' is exported with visibility {} (gABI: made local)",
                    s.name,
                    s.visibility()
                ),
            ));
        }
        // The export is the program's definition (its .symtab's, when the
        // output keeps one; an IFUNC's is the resolver, judged below).
        // Two `.symver` definitions of one name (`foo@V1`, `foo@@V2`, a
        // library's, D-115) are two globals of the name: any one at the
        // export's value is its definition (WP-121).
        let at_value = || {
            image.symbols.iter().any(|o| {
                o.name == s.name
                    && o.sym.binding() != mortar_elf::consts::STB_LOCAL
                    && o.sym.st_value == s.st_value
            })
        };
        if let Some(o) = image.global(&s.name)
            && o.sym.st_shndx != 0
            && o.sym.sym_type() != STT_GNU_IFUNC
            && o.sym.st_value != s.st_value
            && !at_value()
        {
            v.push(sym_finding(
                7,
                &s.name,
                format!(
                    "'{}' is exported at {:#x}, the program defines it at {:#x}",
                    s.name, s.st_value, o.sym.st_value
                ),
            ));
        }
        if let Some(o) = image.global(&s.name)
            && o.sym.sym_type() == STT_GNU_IFUNC
            && s.sym_type() == STT_FUNC
            && s.st_value == o.sym.st_value
        {
            v.push(sym_finding(
                7,
                &s.name,
                format!(
                    "'{}' is exported at its IFUNC resolver, not a canonical IPLT entry (W13)",
                    s.name
                ),
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
    }
    // A name twice in `.dynsym` (every linker, WP-149): two versions of one
    // name are two keys (D-115, `.symver` `foo@V1` and `foo@@V2`), so a
    // name may stand twice exactly when its entries' `.gnu.version`
    // indices differ and at most one of its definitions is not hidden (an
    // unversioned lookup takes the one default). An import's index names
    // a `Vernaux` and is never hidden: two imports of one name under two
    // needed versions are two keys too.
    // (GNU ld's AArch64 libraries keep nameless STB_LOCAL section symbols
    // in `.dynsym`: no lookup sees a local entry.)
    let mut by: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (j, s) in out.syms.iter().enumerate().skip(1) {
        if s.binding() == mortar_elf::consts::STB_LOCAL {
            continue;
        }
        by.entry(s.name.as_str()).or_default().push(j);
    }
    for (name, js) in by.into_iter().filter(|e| e.1.len() > 1) {
        let idx = |j: usize| out.versym.as_ref().and_then(|vs| vs.get(j).copied());
        let raws: Vec<Option<u16>> = js.iter().map(|&j| idx(j)).collect();
        let distinct = raws.iter().all(Option::is_some)
            && raws.iter().map(|r| r.map(|x| x & 0x7fff)).collect::<BTreeSet<_>>().len()
                == raws.len();
        let defaults = js
            .iter()
            .filter(|&&j| out.syms[j].defined() && idx(j).is_none_or(|x| x & 0x8000 == 0))
            .count();
        if !distinct || defaults > 1 {
            v.push(sym_finding(
                7,
                name,
                format!(
                    "a name twice in .dynsym: '{name}' at indices {js:?} with .gnu.version {raws:?} (two entries of one name need distinct version indices, at most one definition not hidden: D-115)"
                ),
            ));
        }
    }
}

/// P-D8: versions.
#[allow(clippy::too_many_lines)] // one pass per entry kind
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
        // Index 0 of an import is an unversioned lookup too (glibc reads
        // `l_versions[0]` with hash 0; WP-101: it escaped this check).
        if !s.defined() && x & 0x7fff <= 1 {
            // Unversioned: D-27 records the version of a versioned
            // definition (an asked `.symver` one, or the default).
            let asked = model.keys.get(&s.name).and_then(|k| k.version.clone());
            let bound = ld.model.bind_in(&ld.order, &s.name, asked.as_deref());
            // A weak import whose definition is in a library the output
            // does not need (`--as-needed` dropped it; reached through
            // another's dependencies) has no Verneed to name (D-27: one
            // per needed object; vn_file a DT_NEEDED name): unversioned,
            // as GNU ld writes it (WP-121, `cxx-weak-only-libc`).
            let unneeded = |i: usize| {
                s.binding() == STB_WEAK && !out.needed.contains(&ld.model.dsos[i].soname)
            };
            if bound.is_none()
                && s.binding() != STB_WEAK
                && let Some(want) = &asked
            {
                // The reference asked for a version (`.symver`): index 1
                // drops it whatever the loader finds (W42, resolve §9.2).
                v.push(sym_finding(
                    8,
                    &s.name,
                    format!(
                        "'{}' is imported unversioned; the input reference asks for '{want}' (resolve §9.2)",
                        s.name
                    ),
                ));
            } else if let Some((i, d)) = bound
                && !unneeded(i)
                && let Some(want) = asked.or_else(|| d.version.clone())
            {
                v.push(sym_finding(
                    8,
                    &s.name,
                    format!(
                        "'{}' is imported unversioned; its definition is '{want}' (D-27)",
                        s.name
                    ),
                ));
            }
        }
        if s.defined() && x & 0x7fff >= 2 && s.binding() != mortar_elf::consts::STB_LOCAL {
            // An exported definition standing for a library's (a COPY, an
            // interposition): glibc's versioned lookup skips it unless its
            // index names the version the library defines (D-27).
            let Some(need) = out.need_of(j) else {
                v.push(sym_finding(8, &s.name, format!("version index {x} names no Vernaux")));
                continue;
            };
            if ld.model.lookup(&ld.order, &s.name, Some(&need.name)).is_none() {
                let lib =
                    ld.model.lookup(&ld.order, &s.name, None).and_then(|(_, d)| d.version.clone());
                v.push(sym_finding(
                    8,
                    &s.name,
                    format!(
                        "'{}' is exported as version '{}', which no library defines it under (its default: {:?}) (D-27)",
                        s.name, need.name, lib
                    ),
                ));
            }
            continue;
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
        let Some((i, d)) = bound else {
            // Nothing defines it: the reference names a version its
            // Vernaux file does not define the name under (D-27).
            v.push(sym_finding(
                8,
                &s.name,
                format!(
                    "'{}' asks for '{}' of '{}', which defines no '{}' (D-27)",
                    s.name, need.name, need.file, s.name
                ),
            ));
            continue;
        };
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
        v.push(fd(
            9,
            format!("DT_RELACOUNT is {c}; {lead} RELATIVE records lead .rela.dyn (D-40)"),
        ));
    }
}

/// P-D10: PLT entries, their words, the lazy word, IRELATIVE order (H1).
fn pd10(
    image: &Image<'_>,
    out: &DynOut,
    machine: u16,
    format: Format,
    spie: bool,
    v: &mut Vec<Finding>,
) {
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
        // A static-pie's entry of an undefined weak function jumps through
        // a word holding 0 that no record writes: it reaches 0 at any
        // bias, as the call does in a static executable (§12.4).
        let weak0 = spie && n == 0 && slot % 8 == 0 && image.value(slot, 8) == Some(0);
        if n != 1 && !weak0 {
            v.push(
                fd(10, format!("the PLT entry jumps through {slot:#x}, which has {n} JUMP_SLOT/IRELATIVE/GLOB_DAT records"))
                    .at(e),
            );
        }
    }
    // A canonical PLT entry (an import's address, D-18) jumps through the
    // slot of that import: a call through it reaches the definition.
    for s in out.syms.iter().skip(1).filter(|s| !s.defined() && s.st_value != 0) {
        let Some(slot) = entries.get(&s.st_value).copied() else { continue };
        let names: Vec<&str> = by_slot
            .get(&slot)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .filter(|(_, k)| matches!(k, Kind::JumpSlot | Kind::GlobDat))
            .filter_map(|&(i, _)| out.syms.get(out.rels[i].sym as usize))
            .map(|x| x.name.as_str())
            .collect();
        if !names.is_empty() && !names.contains(&s.name.as_str()) {
            v.push(
                fd(
                    10,
                    format!(
                        "the canonical PLT entry of '{}' jumps through {slot:#x}, which binds {}",
                        s.name,
                        names.join(", ")
                    ),
                )
                .at(s.st_value),
            );
        }
    }
    for r in jmprel.iter().filter(|r| kind(machine, r.ty) == Kind::JumpSlot) {
        if !slots_reached.contains(&r.offset) {
            v.push(fd(10, "a JUMP_SLOT word no PLT entry jumps through").at(r.offset));
        }
    }
    // The header, the lazy words and the way back (WP-082). A static-pie
    // binds nothing lazily: glibc relocates it with lazy 0 (§12.4).
    if !spie {
        crate::plt::check(image, out, &entries, format, v);
    }
    // H1 (D-41): every IRELATIVE after every symbolic record, in
    // application order, so a resolver that calls through the PLT or GOT,
    // or reads a copied object, runs after those are bound.
    let symbolic = |k: Kind| matches!(k, Kind::JumpSlot | Kind::GlobDat | Kind::Copy | Kind::Abs);
    let last = out.rels.iter().rposition(|r| symbolic(kind(machine, r.ty)));
    if let Some(l) = last
        && let Some(ir) = out.rels.iter().position(|r| kind(machine, r.ty) == Kind::Irelative)
        && ir < l
    {
        let what = if out.rels[ir..].iter().any(|r| kind(machine, r.ty) == Kind::JumpSlot) {
            "a JUMP_SLOT: a resolver that calls through the PLT jumps to an unbound slot"
        } else {
            "a symbolic record: a resolver that reads through the GOT or a copied object sees it unbound"
        };
        v.push(
            fd(10, format!("an IRELATIVE record is applied before {what} (D-41, H1)"))
                .at(out.rels[ir].offset),
        );
    }
}

/// P-D11 (copies) and P-D13 (protected).
#[allow(clippy::too_many_lines)] // one block per property
fn pd11_13(
    image: &Image<'_>,
    out: &DynOut,
    model: &Model,
    ld: &Loader<'_>,
    format: Format,
    v: &mut Vec<Finding>,
) -> Vec<crate::maprec::ActualCopy> {
    let mut actual = Vec::new();
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
        actual.push(crate::maprec::ActualCopy {
            addr: r.offset,
            name: s.name.clone(),
            size: s.st_size,
            align: None,
        });
        let b = ld.bind_copy(out, r.sym as usize, &p);
        let Bound::Lib { i, .. } = b else { continue };
        let ver = out.need_of(r.sym as usize).map(|n| n.name.as_str());
        let Some((_, d)) = model.lookup(&ld.order, &s.name, ver) else { continue };
        if d.visibility() == STV_PROTECTED {
            v.push(
                sym_finding(
                    13,
                    &s.name,
                    format!("a COPY of '{}', whose definition is STV_PROTECTED (D-18, W5)", s.name),
                )
                .at(r.offset),
            );
        }
        if s.st_size == 0 || d.size == 0 {
            v.push(
                sym_finding(11, &s.name, format!("a COPY of '{}' of size 0 (W4)", s.name))
                    .at(r.offset),
            );
        }
        if s.st_size != d.size {
            let mut f = sym_finding(
                11,
                &s.name,
                format!(
                    "the copy of '{}' has size {}, its definition {}",
                    s.name, s.st_size, d.size
                ),
            )
            .at(r.offset);
            f.expected = Some(d.size);
            f.found = Some(s.st_size);
            v.push(f);
        }
        let align = model.dsos[i].copy_align(d.shndx, d.value);
        if let Some(a) = actual.last_mut() {
            a.align = Some(align);
        }
        if !r.offset.is_multiple_of(align) {
            v.push(
                sym_finding(
                    11,
                    &s.name,
                    format!(
                        "the copy of '{}' at {:#x} is not aligned to {align} (D-42)",
                        s.name, r.offset
                    ),
                )
                .at(r.offset),
            );
        }
        let ro = model.dsos[i].read_only_at(d.value);
        let in_relro =
            relro.iter().any(|&(lo, hi)| lo <= r.offset && r.offset + s.st_size.max(1) <= hi);
        // Without PT_GNU_RELRO (`-z norelro`, D-42) nothing is read-only
        // after relocation: every reference then copies a read-only
        // definition into ordinary writable memory (GNU ld `.data.rel.ro`,
        // lld `.bss.rel.ro`, mold `.copyrel`; WP-088), and only the
        // writable place below and mortar's section are judged. A missing
        // PT_GNU_RELRO the line asks for is P-D4's.
        if !relro.is_empty() && ro != in_relro {
            v.push(
                sym_finding(
                    11,
                    &s.name,
                    format!(
                        "the copy of '{}' is {} RELRO; its definition is {} (D-42)",
                        s.name,
                        if in_relro { "inside" } else { "outside" },
                        if ro { "read-only" } else { "writable" }
                    ),
                )
                .at(r.offset),
            );
        }
        if image.load_of(r.offset, s.st_size.max(1)).is_none_or(|l| l.p_flags & PF_W == 0) {
            v.push(sym_finding(11, &s.name, "a copy outside writable memory").at(r.offset));
        }
        // mortar's chunk by the output section, with or without RELRO
        // (D-42: `.bss.rel.ro` for a read-only definition, else `.dynbss`,
        // the first chunk of `.bss`).
        if format == Format::Mortar {
            let want = if ro { ".bss.rel.ro" } else { ".bss" };
            let found =
                image.alloc_section_of(r.offset, s.st_size.max(1)).map(|(_, o)| o.name.as_str());
            if found != Some(want) {
                v.push(
                    sym_finding(
                        11,
                        &s.name,
                        format!(
                            "the copy of '{}' is in {}; its definition is {}, so D-42's section is {want}",
                            s.name,
                            found.unwrap_or("no allocated section"),
                            if ro { "read-only" } else { "writable" }
                        ),
                    )
                    .at(r.offset),
                );
            }
        }
        // Aliases at the same (file, value) are defined at the copy (W2).
        for (name, defs) in &model.dsos[i].defs {
            if name == &s.name {
                continue;
            }
            if !defs.iter().any(|x| x.visible() && x.value == d.value && x.shndx != 0) {
                continue;
            }
            let ok =
                out.syms.iter().any(|y| &y.name == name && y.defined() && y.st_value == r.offset);
            if !ok {
                v.push(sym_finding(
                    11,
                    name,
                    format!("'{name}', an alias of the copied '{}', is not defined at the copy (D-21, W2)", s.name),
                ).at(r.offset));
                v.push(sym_finding(
                    7,
                    name,
                    format!("the copy alias '{name}' is not in .dynsym (W2)"),
                ));
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
        let same = model.dsos[*i]
            .defs
            .get(&s.name)
            .is_some_and(|x| x.iter().any(|y| y.visible() && y.value == *value));
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
            v.push(
                sym_finding(
                    11,
                    &w[1].2,
                    format!("the copies of '{}' and '{}' overlap", w[0].2, w[1].2),
                )
                .at(w[1].0),
            );
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
                v.push(
                    sym_finding(11, n, format!("the copy of '{n}' overlaps '{}'", o.name)).at(*a),
                );
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
            v.push(sym_finding(
                13,
                &s.name,
                format!(
                    "a canonical PLT of '{}', whose definition is STV_PROTECTED (D-18, W5)",
                    s.name
                ),
            ));
        }
    }
    actual
}

/// How a library binds its definitions to itself, as P-D24 reads it.
enum SelfBound {
    /// `DT_SYMBOLIC`, or `DF_SYMBOLIC` in the last `DT_FLAGS`.
    Symbolic,
    /// The `.dynsym` indices of its `.note.mason` table's rows with
    /// `NONPREEMPTIBLE` (odr.md O-51, O-60).
    Rows(BTreeSet<u32>),
    /// A table note that does not parse (why): every definition counts as
    /// bound (nothing tells which is not).
    Unreadable(String),
}

/// What P-D24 reads of one library, again from its bytes: program
/// headers and `.dynamic` only, never its section headers.
struct SelfBinding<'a> {
    so: mortar_elf::SharedObject<'a>,
    bound: SelfBound,
    /// `(p_vaddr, p_memsz, PF_W)` of every `PT_LOAD`.
    loads: Vec<(u64, u64, bool)>,
    /// `(p_vaddr, p_memsz)` of every `PT_GNU_RELRO`.
    relro: Vec<(u64, u64)>,
}

/// O-52's table note: owner `MasonMeta`, type `NT_MASON_ODR`.
const ODR_NOTE_OWNER: &[u8] = b"MasonMeta\0";
const NT_MASON_ODR: u32 = 0x4d4f_4401;
/// O-51's `NONPREEMPTIBLE` flag bit.
const ROW_NONPREEMPTIBLE: u16 = 0x8000;
const DT_SYMBOLIC_TAG: i64 = 16;
const DT_FLAGS_TAG: i64 = 30;
const DF_SYMBOLIC_BIT: u64 = 0x2;

/// O-51's table: the `.dynsym` indices of the rows with `NONPREEMPTIBLE`,
/// or why the descriptor is not a table (header `{version 1, count,
/// row_size 24, reserved}`, rows `{key u64, hash [8], dynsym u32, flags
/// u16, reserved u16}` sorted by key).
fn nonpreemptible_rows(desc: &[u8]) -> Result<BTreeSet<u32>, String> {
    let u32_at = |b: &[u8], at: usize| {
        b.get(at..at + 4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
    };
    if desc.len() < 16 {
        return Err(format!("a descriptor of {} bytes holds no header", desc.len()));
    }
    let (version, count, row_size) =
        (u32_at(desc, 0).unwrap_or(0), u32_at(desc, 4).unwrap_or(0), u32_at(desc, 8).unwrap_or(0));
    if version != 1 {
        return Err(format!("version {version}"));
    }
    if row_size != 24 {
        return Err(format!("row_size {row_size}"));
    }
    let room = (desc.len() - 16) / 24;
    if count as usize > room {
        return Err(format!("{count} rows in room for {room}"));
    }
    let mut rows = BTreeSet::new();
    let mut last = 0u64;
    for n in 0..count as usize {
        let r = &desc[16 + 24 * n..16 + 24 * (n + 1)];
        let mut k = [0u8; 8];
        k.copy_from_slice(&r[..8]);
        let key = u64::from_le_bytes(k);
        if key < last {
            return Err(format!("row {n} out of key order"));
        }
        last = key;
        if u16::from_le_bytes([r[20], r[21]]) & ROW_NONPREEMPTIBLE != 0 {
            rows.insert(u32_at(r, 16).unwrap_or(0));
        }
    }
    Ok(rows)
}

/// The library's own binding (P-D24), or `None` when it binds nothing
/// to itself (or is not readable through its `.dynamic`, which the model
/// reports).
fn self_binding(bytes: &[u8]) -> Option<SelfBinding<'_>> {
    use mortar_elf::consts::PT_NOTE;
    let so = mortar_elf::SharedObject::parse_by(bytes, mortar_elf::Entry::Dynamic).ok()?;
    let dynamic = so.dynamic().ok()??;
    let mut symbolic = false;
    let mut flags = 0;
    for e in &dynamic {
        match e.d_tag {
            DT_SYMBOLIC_TAG => symbolic = true,
            DT_FLAGS_TAG => flags = e.d_val,
            _ => {}
        }
    }
    let mut loads = Vec::new();
    let mut relro = Vec::new();
    let mut desc = None;
    for seg in &so.program_headers() {
        let h = seg.header();
        match h.p_type {
            PT_LOAD => loads.push((h.p_vaddr, h.p_memsz, h.p_flags & PF_W != 0)),
            PT_GNU_RELRO => relro.push((h.p_vaddr, h.p_memsz)),
            PT_NOTE if desc.is_none() => {
                let Ok(notes) = seg.notes() else { continue };
                for n in notes {
                    let Ok(n) = n else { break };
                    if n.n_type == NT_MASON_ODR && n.name_bytes() == ODR_NOTE_OWNER {
                        desc = Some(n.desc());
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    let bound = if symbolic || flags & DF_SYMBOLIC_BIT != 0 {
        SelfBound::Symbolic
    } else {
        match nonpreemptible_rows(desc?) {
            Ok(rows) if rows.is_empty() => return None,
            Ok(rows) => SelfBound::Rows(rows),
            Err(why) => SelfBound::Unreadable(why),
        }
    };
    Some(SelfBinding { so, bound, loads, relro })
}

/// Where P-D24 reports a copied definition at `value`: in a writable
/// `PT_LOAD` outside every `PT_GNU_RELRO`, in none, or in a library one of
/// whose `PT_LOAD`s ends past 2^64 (`p_vaddr + p_memsz` overflows: the
/// place is unknown). `None`: read-only or RELRO, two equal constants. A
/// `PT_GNU_RELRO` whose end overflows covers nothing; overlapping
/// `PT_LOAD`s count as writable when any is.
fn two_objects_at(
    loads: &[(u64, u64, bool)],
    relro: &[(u64, u64)],
    value: u64,
) -> Option<&'static str> {
    let mut overflow = false;
    let mut found: Option<bool> = None;
    for &(at, size, w) in loads {
        match at.checked_add(size) {
            None => overflow = true,
            Some(end) if at <= value && value < end => found = Some(found.unwrap_or(false) || w),
            Some(_) => {}
        }
    }
    let in_relro = relro
        .iter()
        .any(|&(at, size)| at.checked_add(size).is_some_and(|end| at <= value && value < end));
    match found {
        _ if overflow => Some("a library whose PT_LOAD ends past the address space"),
        None => Some("no PT_LOAD of its library"),
        Some(true) if !in_relro => Some("a writable PT_LOAD of its library, outside PT_GNU_RELRO"),
        Some(_) => None,
    }
}

/// P-D24 (request P-R9; `spec/dyn.md` D-43a), added to P-D11 for `Exec`
/// and `Pie`: a `COPY` of a definition its library binds to itself whose
/// `st_value` lies in a writable `PT_LOAD` of that library outside every
/// `PT_GNU_RELRO`, or in no `PT_LOAD`, gives the program and the library
/// two objects where the program has one. A copy of read-only or RELRO
/// data is two equal constants and is not reported. A `PT_LOAD` whose
/// `p_vaddr + p_memsz` overflows leaves the place unknown (reported); a
/// `PT_GNU_RELRO` whose end overflows covers nothing.
fn pd24(out: &DynOut, line: &Line, model: &Model, ld: &Loader<'_>, v: &mut Vec<Finding>) {
    if !matches!(line.kind.unwrap_or(OutputKind::Exec), OutputKind::Exec | OutputKind::Pie) {
        return;
    }
    let mut libs: HashMap<usize, Option<SelfBinding<'_>>> = HashMap::new();
    for r in &out.rels {
        if kind(ld.machine, r.ty) != Kind::Copy {
            continue;
        }
        let Some(s) = out.syms.get(r.sym as usize) else { continue };
        let ver = out.need_of(r.sym as usize).map(|n| n.name.as_str());
        let Some((i, d)) = model.lookup(&ld.order, &s.name, ver) else { continue };
        let lib = &model.dsos[i];
        let Some(b) = libs.entry(i).or_insert_with(|| self_binding(&lib.bytes)) else { continue };
        let how = match &b.bound {
            SelfBound::Symbolic => {
                "it is linked -Bsymbolic (DT_SYMBOLIC or DF_SYMBOLIC)".to_owned()
            }
            SelfBound::Unreadable(why) => {
                format!("its .note.mason table does not parse ({why}), so every definition counts")
            }
            SelfBound::Rows(rows) => {
                // The definition's `.dynsym` entries (through DT_SYMTAB
                // and the hash table's count), by name and value.
                let Ok(Some(table)) = b.so.symbols() else { continue };
                let marked = table.iter().any(|(j, y)| {
                    y.st_shndx != 0
                        && y.st_value == d.value
                        && table.name(&y).is_ok_and(|n| n == s.name.as_bytes())
                        && rows.contains(&j.0)
                });
                if !marked {
                    continue;
                }
                "its .note.mason row is NONPREEMPTIBLE".to_owned()
            }
        };
        let Some(place) = two_objects_at(&b.loads, &b.relro, d.value) else { continue };
        v.push(
            sym_finding(
                24,
                &s.name,
                format!(
                    "a COPY of '{}', which {} binds to itself ({how}), and whose definition at {:#x} lies in {place}: the program and the library use two objects (D-43a, W30)",
                    s.name, lib.path, d.value
                ),
            )
            .at(r.offset),
        );
    }
}

/// D-48's kept descriptor (a64 R2, WP-082): the slot pairs that a kept
/// AArch64 large-model TLSDESC sequence addresses, with its symbol, and
/// the names some recognised (small-model) TLSDESC sequence uses. The
/// large form is `movz Xd, #:tlsdesc_off_g1:S; movk Xd,
/// #:tlsdesc_off_g0_nc:S` (relocations 565, 566), then `ldr`/`add` off the
/// GOT base (567, 568) and `blr` (569): the pair is at GOT + the 32-bit
/// offset the two instructions write, GOT being `_GLOBAL_OFFSET_TABLE_`
/// (the psABI's `G(GTLSDESC(S+A)) - GOT`).
fn a64_large_desc(
    image: &Image<'_>,
    objs: &[Obj<'_>],
) -> (BTreeMap<u64, String>, BTreeSet<String>) {
    let mut pairs = BTreeMap::new();
    let mut small = BTreeSet::new();
    let got = image
        .unique("_GLOBAL_OFFSET_TABLE_")
        .or_else(|| image.global("_GLOBAL_OFFSET_TABLE_"))
        .map(|s| s.sym.st_value)
        .or_else(|| sec(image, ".got").map(|g| g.0));
    for o in objs {
        let Some(t) = o.symtab else { continue };
        for sc in &o.secs {
            let Some(recs) = sc.relocs.and_then(|r| o.records(r)) else { continue };
            let name = |i: u32| {
                t.get(mortar_elf::SymbolIndex(i))
                    .ok()
                    .and_then(|x| t.name(&x).ok().map(|n| String::from_utf8_lossy(n).into_owned()))
            };
            for &(off, sym, ty, addend) in &recs {
                if (560..=564).contains(&ty) {
                    small.extend(name(sym));
                }
                if ty != 565 || addend.unwrap_or(0) != 0 {
                    continue;
                }
                let has = |at: Option<u64>, want: u32| {
                    recs.iter().any(|&(o2, s2, t2, _)| {
                        t2 == want && s2 == sym && at.is_none_or(|a| a == o2)
                    })
                };
                if !(has(Some(off + 4), 566) && has(None, 567) && has(None, 569)) {
                    continue;
                }
                let crate::locate::Place::Whole(base) = sc.place else { continue };
                let (Some(hi), Some(lo), Some(g), Some(n)) =
                    (image.insn(base + off), image.insn(base + off + 4), got, name(sym))
                else {
                    continue;
                };
                // MOVZ Xd, #imm16, LSL #16 and MOVK Xd, #imm16 (one Xd).
                let rd = hi & 0x1f;
                if hi & 0xffe0_0000 != 0xd2a0_0000
                    || lo & 0xffe0_0000 != 0xf280_0000
                    || lo & 0x1f != rd
                {
                    continue;
                }
                let at = u64::from((hi >> 5) & 0xffff) << 16 | u64::from((lo >> 5) & 0xffff);
                pairs.insert(g.wrapping_add(at), n);
            }
        }
    }
    (pairs, small)
}

/// P-D12: TLS records.
fn pd12(
    image: &Image<'_>,
    out: &DynOut,
    ld: &Loader<'_>,
    objs: &[Obj<'_>],
    format: Format,
    v: &mut Vec<Finding>,
) {
    let p = Params::default();
    let large = if ld.machine == EM_AARCH64 && format == Format::Mortar {
        Some(a64_large_desc(image, objs))
    } else {
        None
    };
    for r in &out.rels {
        let k = kind(ld.machine, r.ty);
        if !matches!(k, Kind::TpOff | Kind::DtpMod | Kind::DtpOff | Kind::TlsDesc) {
            continue;
        }
        if r.sym == 0 {
            if format == Format::Mortar {
                v.push(
                    fd(12, "a TLS record against the executable's own block (D-48: local-exec)")
                        .at(r.offset),
                );
            }
            continue;
        }
        let Some(s) = out.syms.get(r.sym as usize) else { continue };
        match ld.bind(out, r.sym as usize, false, &p) {
            Bound::Lib { tls: false, .. } => v.push(
                sym_finding(
                    12,
                    &s.name,
                    format!(
                        "a TLS record against '{}', whose definition is not STT_TLS (D-16)",
                        s.name
                    ),
                )
                .at(r.offset),
            ),
            Bound::Exec(_) if format == Format::Mortar => v.push(
                sym_finding(
                    12,
                    &s.name,
                    "a TLS record against an executable-defined symbol (D-48)",
                )
                .at(r.offset),
            ),
            Bound::Missing => v.push(
                sym_finding(12, &s.name, "a TLS record against a name no object defines")
                    .at(r.offset),
            ),
            _ => {}
        }
        // D-48: descriptors are relaxed, except AArch64's large form
        // (a64 R2), which keeps its record: right when the record is the
        // pair the sequence addresses and no recognised sequence uses the
        // name (that one relaxes, and the name then has an IE slot).
        let kept = large.as_ref().is_some_and(|(pairs, small)| {
            pairs.get(&r.offset) == Some(&s.name)
                && !small.contains(&s.name)
                && r.addend.unwrap_or(0) == 0
        });
        if k == Kind::TlsDesc && format == Format::Mortar && !kept {
            v.push(
                sym_finding(
                    12,
                    &s.name,
                    "a TLSDESC record (D-48: descriptors are relaxed; only AArch64's large-model sequence keeps one, at the pair it addresses)",
                )
                .at(r.offset),
            );
        }
    }
    pd12_tls_definition(out, ld, objs, &p, v);
    let _ = DT_NEEDED;
}

/// P-D12, D-16's other direction (WP-088, dyn.md W14): an input's
/// non-TLS reference (a type of the machine's table that is not
/// thread-local, from an allocated section) to an undefined name that the
/// loader binds to a library's `STT_TLS` definition. lld writes the GOT
/// word as a `TPREL64` slot the code reads as an address, mold a
/// `GLOB_DAT` that glibc resolves to the library's base plus the TLS
/// offset (the program reads the ELF header): both run wrong. One finding
/// per name.
fn pd12_tls_definition(
    out: &DynOut,
    ld: &Loader<'_>,
    objs: &[Obj<'_>],
    p: &Params,
    v: &mut Vec<Finding>,
) {
    let def = |ty: u32| match ld.machine {
        EM_AARCH64 => crate::a64::def(ty),
        mortar_elf::consts::EM_X86_64 => crate::x64::def(ty),
        _ => None,
    };
    let mut seen = BTreeSet::new();
    for o in objs {
        let Some(t) = o.symtab else { continue };
        for (si, sc) in o.secs.iter().enumerate().filter(|(_, sc)| sc.alloc()) {
            let Some(recs) = sc.relocs.and_then(|r| o.records(r)) else { continue };
            for &(off, sym, ty, _) in &recs {
                if def(ty).is_none_or(|d| d.op.is_tls() || d.op == crate::a64::Op::None) {
                    continue;
                }
                let Ok(x) = t.get(mortar_elf::SymbolIndex(sym)) else { continue };
                if sym == 0 || x.st_shndx != mortar_elf::consts::SHN_UNDEF {
                    continue;
                }
                let name = String::from_utf8_lossy(t.name(&x).unwrap_or(b"")).into_owned();
                if seen.contains(&name) {
                    continue;
                }
                let Some(j) = out.syms.iter().position(|s| s.name == name) else { continue };
                if let Bound::Lib { tls: true, .. } = ld.bind(out, j, false, p) {
                    let mut f = sym_finding(
                        12,
                        &name,
                        format!(
                            "a non-TLS reference to '{name}', whose shared definition is STT_TLS (D-16)"
                        ),
                    );
                    f.site = Some(o.site(si, off));
                    v.push(f);
                    seen.insert(name);
                }
            }
        }
    }
}

/// The fields WP-082's byte sweep found judged by no rule (`spec/plumb.md`
/// §12.3): each is something glibc reads, or a value every reference
/// writes one way that a loader would misread otherwise.
#[allow(clippy::too_many_lines)] // one block per table
fn fields(
    image: &Image<'_>,
    out: &DynOut,
    model: &Model,
    line: &Line,
    objs: &[Obj<'_>],
    format: Format,
    v: &mut Vec<Finding>,
) {
    use mortar_elf::consts::{STB_GLOBAL, STV_DEFAULT};
    const STB_GNU_UNIQUE: u8 = 10;
    // The names the inputs define only STB_GNU_UNIQUE (dyn.md D-23's
    // export row: the output binding is `.symtab`'s, unique).
    let unique = {
        let (mut u, mut other) = (BTreeSet::new(), BTreeSet::new());
        for o in objs {
            let Some(t) = &o.symtab else { continue };
            for (_, x) in t.iter() {
                if x.st_shndx == 0 || !matches!(x.binding(), STB_GLOBAL | STB_WEAK | STB_GNU_UNIQUE)
                {
                    continue;
                }
                let n = String::from_utf8_lossy(t.name(&x).unwrap_or(b"")).into_owned();
                if x.binding() == STB_GNU_UNIQUE {
                    u.insert(n)
                } else {
                    other.insert(n)
                };
            }
        }
        &u - &other
    };
    let m = image.header.e_machine;
    // P-D8: the version tables' shape.
    for p in &out.version_problems {
        v.push(fd(8, p.clone()));
    }
    if let Some(vs) = &out.versym
        && vs.first().is_some_and(|&x| x != 0)
    {
        v.push(fd(8, ".gnu.version entry 0 (the null symbol's) is not 0"));
    }
    // P-D7: the null symbol and every entry's fields.
    if let Some(z) = out.syms.first()
        && (!z.name.is_empty()
            || z.st_info != 0
            || z.st_other != 0
            || z.st_shndx != 0
            || z.st_value != 0
            || z.st_size != 0)
    {
        v.push(fd(7, ".dynsym entry 0 is not all zero (gABI: the undefined symbol)"));
    }
    let entries = crate::loader::plt_entries(image);
    for (j, s) in out.syms.iter().enumerate().skip(1) {
        let extra = s.st_other & !3;
        let variant_pcs = m == EM_AARCH64 && extra == 0x80;
        if extra != 0 && !variant_pcs {
            v.push(sym_finding(
                7,
                &s.name,
                format!(
                    "'{}' has st_other {:#x}: bits the psABI does not define",
                    s.name, s.st_other
                ),
            ));
        }
        // AAELF64: a JUMP_SLOT of a variant-PCS function needs
        // DT_AARCH64_VARIANT_PCS (the loader must not bind it lazily).
        if variant_pcs
            && !out.has(dynout::DT_AARCH64_VARIANT_PCS)
            && out.rels.iter().any(|r| r.sym as usize == j && kind(m, r.ty) == Kind::JumpSlot)
        {
            v.push(sym_finding(
                7,
                &s.name,
                format!(
                    "'{}' is STO_AARCH64_VARIANT_PCS with a JUMP_SLOT and there is no DT_AARCH64_VARIANT_PCS",
                    s.name
                ),
            ));
        }
        let b = s.binding();
        if !s.defined() {
            // glibc looks a reference up only when it is STV_DEFAULT;
            // any other binds it in the executable itself (at 0).
            if s.visibility() != STV_DEFAULT {
                v.push(sym_finding(
                    7,
                    &s.name,
                    format!(
                        "'{}' is imported with visibility {}: the loader binds it to the executable's undefined entry",
                        s.name,
                        s.visibility()
                    ),
                ));
            }
            if b != STB_GLOBAL && b != STB_WEAK {
                v.push(sym_finding(
                    7,
                    &s.name,
                    format!("'{}' is imported with binding {b} (not GLOBAL or WEAK)", s.name),
                ));
            }
            let refd = |n: &str| model.keys.get(n).is_some_and(|k| k.strong_ref || k.weak_ref);
            // GNU ld also imports a library's alias (same value) of a
            // referenced definition, as `_environ` beside `environ`.
            let alias = || {
                model.dsos.iter().any(|d| {
                    d.defs.get(&s.name).is_some_and(|mine| {
                        mine.iter().any(|m| {
                            m.shndx != 0
                                && d.defs.iter().any(|(n, x)| {
                                    n != &s.name
                                        && refd(n)
                                        && x.iter().any(|y| y.shndx != 0 && y.value == m.value)
                                })
                        })
                    })
                })
            };
            let referenced = refd(&s.name) || line.undefined.contains(&s.name) || alias();
            if !referenced {
                v.push(sym_finding(
                    7,
                    &s.name,
                    format!("'{}' is imported and no input references it (D-21)", s.name),
                ));
            }
            // A non-zero value is a canonical PLT claim (D-18): the
            // loader binds every library's reference to that address.
            if s.st_value != 0 && !entries.contains_key(&s.st_value) {
                v.push(
                    sym_finding(
                        10,
                        &s.name,
                        format!(
                            "'{}' is imported with st_value {:#x}, which is no PLT entry (D-18)",
                            s.name, s.st_value
                        ),
                    )
                    .at(s.st_value),
                );
            }
            continue;
        }
        // The section that holds the value is the one the entry names
        // (a section symbol: at that section's start).
        let named = image.sections.get(usize::from(s.st_shndx));
        // (A TLS value is an offset in the TLS segment: not judged here;
        // mold's export of an IFUNC at its IPLT entry keeps the
        // resolver's section, which no loader reads.)
        if s.st_shndx < mortar_elf::consts::SHN_LORESERVE
            && s.sym_type() != mortar_elf::consts::STT_TLS
            && !entries.contains_key(&s.st_value)
        {
            let holds = named.is_some_and(|x| {
                let (lo, hi) = (x.shdr.sh_addr, x.shdr.sh_addr + x.shdr.sh_size);
                if s.sym_type() == STT_SECTION {
                    s.st_value == lo
                } else {
                    x.shdr.sh_flags & mortar_elf::consts::SHF_ALLOC != 0
                        && lo <= s.st_value
                        && s.st_value <= hi
                }
            });
            if !holds {
                v.push(sym_finding(
                    7,
                    &s.name,
                    format!(
                        "'{}' (st_value {:#x}) names section {}, which does not hold it",
                        s.name, s.st_value, s.st_shndx
                    ),
                ));
            }
        }
        if s.sym_type() == STT_SECTION {
            continue;
        }
        if b != STB_GLOBAL && b != STB_WEAK && b != STB_GNU_UNIQUE {
            v.push(sym_finding(7, &s.name, format!("'{}' is exported with binding {b}", s.name)));
        }
        // An exported unique definition stays unique, so glibc's unique
        // table holds the program's instance (WP-054 case (a): an
        // RTLD_DEEPBIND library otherwise binds its own). EI_OSABI is
        // S1's (mortar; lld and mold write 0 with a unique export).
        if unique.contains(&s.name) && b != STB_GNU_UNIQUE {
            v.push(sym_finding(
                7,
                &s.name,
                format!("'{}' is defined STB_GNU_UNIQUE and exported with binding {b}", s.name),
            ));
        }
        // The export is the program's definition: its .symtab entry's
        // section, size, binding and type (an IFUNC's is P-D7's above).
        // `.symtab` may hold several definitions of the name: the
        // `.symver` definitions `foo@V1` and `foo@@V2` of a library, which
        // GNU ld names so and lld and mortar both name `foo` (WP-149). The
        // export's definition is a non-local `.symtab` symbol of its key
        // (the plain name, or the name with the entry's version, `@@` for
        // a default and `@` for a hidden one) at the export's value; the
        // fields are compared with the first such one. With none at the
        // value (the address rule above reports it), with the first
        // `.symtab` global of the plain name, as before.
        let ver = out.def_version(j);
        let keyed = |o: &&crate::image::OutSym| {
            o.sym.binding() != mortar_elf::consts::STB_LOCAL
                && o.sym.st_shndx != 0
                && (o.name == s.name
                    || ver.as_ref().is_some_and(|(n, hid)| {
                        o.name == format!("{}{}{n}", s.name, if *hid { "@" } else { "@@" })
                    }))
        };
        let def = image
            .symbols
            .iter()
            .filter(keyed)
            .find(|o| o.sym.st_value == s.st_value)
            .or_else(|| image.global(&s.name));
        if let Some(o) = def
            && o.sym.st_shndx != 0
            && o.sym.sym_type() != STT_GNU_IFUNC
        {
            let mut diff = Vec::new();
            if o.sym.st_size != s.st_size {
                diff.push(format!("size {} (.symtab {})", s.st_size, o.sym.st_size));
            }
            if o.sym.st_shndx != s.st_shndx {
                diff.push(format!("section {} (.symtab {})", s.st_shndx, o.sym.st_shndx));
            }
            if o.sym.st_info != s.st_info {
                diff.push(format!("st_info {:#x} (.symtab {:#x})", s.st_info, o.sym.st_info));
            }
            if !diff.is_empty() {
                v.push(sym_finding(
                    7,
                    &s.name,
                    format!("'{}' is exported with {}", s.name, diff.join(", ")),
                ));
            }
        }
    }
    // P-D5: a bucket names a hashed entry or is 0.
    if let Some(g) = &out.gnu_hash {
        for (i, &b) in g.buckets.iter().enumerate() {
            if b != 0 && b < g.symoffset {
                v.push(fd(
                    5,
                    format!(
                        "bucket {i} is {b}, below symoffset {}: the loader reads before the chain",
                        g.symoffset
                    ),
                ));
            }
        }
    }
    // P-D9 and P-D10: record fields the loader reads or the psABI fixes.
    for r in &out.rels {
        let k = kind(m, r.ty);
        if matches!(k, Kind::Relative | Kind::Irelative) && r.sym != 0 {
            v.push(
                fd(9, format!("a RELATIVE or IRELATIVE record names symbol {}", r.sym))
                    .at(r.offset),
            );
        }
        if m == mortar_elf::consts::EM_X86_64
            && matches!(k, Kind::GlobDat | Kind::JumpSlot | Kind::Copy)
            && let Some(a) = r.addend.filter(|&a| a != 0)
        {
            // x86-64 psABI: GLOB_DAT and JUMP_SLOT are S, COPY has no
            // value; glibc's lazy path adds a JUMP_SLOT's addend.
            v.push(
                fd(9, format!("a {k:?} record with addend {a:#x} (the x86-64 psABI computes S)"))
                    .at(r.offset),
            );
        }
        // A static-pie is relocated with lazy 0 (§12.4): its DT_JMPREL is
        // P-D15's.
        if r.plt
            && line.kind != Some(OutputKind::StaticPie)
            && !matches!(k, Kind::JumpSlot | Kind::Irelative | Kind::TlsDesc)
        {
            v.push(
                fd(
                    10,
                    format!(
                        "a {k:?} record in DT_JMPREL: lazy binding refuses it (glibc's elf_machine_lazy_rel)"
                    ),
                )
                .at(r.offset),
            );
        }
    }
    // P-D6: tags once, known, present iff their section exists (D-30).
    let mut count: BTreeMap<u64, usize> = BTreeMap::new();
    for &(t, _) in &out.entries {
        *count.entry(t).or_default() += 1;
    }
    for (&t, &n) in &count {
        if n > 1 && t != DT_NEEDED && t != DT_NULL {
            v.push(fd(6, format!("{} appears {n} times", dynout::tag_name(t))));
        }
        if dynout::tag_name(t).starts_with("tag ") {
            v.push(fd(6, format!("a tag {t:#x} no executable carries (D-30)")));
        }
    }
    if let Some(&(_, val)) = out.entries.iter().find(|e| e.0 == DT_NULL)
        && val != 0
    {
        v.push(fd(6, format!("the DT_NULL entry has the value {val:#x}")));
    }
    if out.get(dynout::DT_DEBUG).is_some_and(|d| d != 0) {
        v.push(fd(6, "DT_DEBUG is not 0 in the file (the loader writes it; D-30)"));
    }
    let present = |name: &str| sec(image, name).is_some();
    // -init/-fini name the functions DT_INIT/DT_FINI hold instead of the
    // sections' starts (D-30 rows 3-4).
    let named = |opt: &Option<String>| {
        opt.as_ref().map(|n| image.global(n).or_else(|| image.unique(n)).map(|x| x.sym.st_value))
    };
    let (init, fini) = (named(&line.init), named(&line.fini));
    let mut rows: Vec<(&[u64], &str)> = Vec::new();
    if init.is_none() {
        rows.push((&[dynout::DT_INIT], ".init"));
    }
    if fini.is_none() {
        rows.push((&[dynout::DT_FINI], ".fini"));
    }
    rows.extend::<[(&[u64], &str); 7]>([
        (&[DT_PREINIT_ARRAY, DT_PREINIT_ARRAYSZ], ".preinit_array"),
        (&[DT_INIT_ARRAY, DT_INIT_ARRAYSZ], ".init_array"),
        (&[DT_FINI_ARRAY, DT_FINI_ARRAYSZ], ".fini_array"),
        (&[DT_GNU_HASH], ".gnu.hash"),
        (&[DT_VERSYM], ".gnu.version"),
        (&[DT_VERNEED, DT_VERNEEDNUM], ".gnu.version_r"),
        (&[DT_JMPREL, DT_PLTRELSZ, DT_PLTREL], ".rela.plt"),
    ]);
    for (tags, name) in rows {
        let want = present(name);
        for &t in tags {
            if out.has(t) != want {
                v.push(fd(
                    6,
                    format!(
                        "{} is {}, and {name} {} (D-30)",
                        dynout::tag_name(t),
                        if want { "missing" } else { "present" },
                        if want { "exists" } else { "does not exist" }
                    ),
                ));
            }
        }
    }
    for &(t, name, opt) in &[(dynout::DT_INIT, ".init", init), (dynout::DT_FINI, ".fini", fini)] {
        // With the option: the named function when the output defines
        // it (an absent function leaves the tag out, GNU ld; not judged).
        let want = match opt {
            Some(Some(f)) => Some(f),
            Some(None) => continue,
            None => sec(image, name).map(|(a, _)| a),
        };
        if let (Some(val), Some(a)) = (out.get(t), want)
            && val != a
        {
            v.push(fd(
                6,
                format!("{} is {val:#x}, {name} is at {a:#x} (D-30)", dynout::tag_name(t)),
            ));
        } else if let Some(val) = out.get(t)
            && !image.load_of(val, 1).is_some_and(|p| p.p_flags & mortar_elf::consts::PF_X != 0)
        {
            // The loader calls it (D-30 rows 3-4, D-102): `-init`/`-fini`
            // naming a data object is refused by mortar; the references
            // write its address and the program crashes (WP-161).
            v.push(fd(
                6,
                format!(
                    "{} is {val:#x}, which is not in an executable segment: the loader calls it (D-30 rows 3-4)",
                    dynout::tag_name(t)
                ),
            ));
        }
    }
    if present(".rela.dyn") {
        for t in [DT_RELA, DT_RELASZ, DT_RELAENT] {
            if !out.has(t) {
                v.push(fd(
                    6,
                    format!("{} is missing, and .rela.dyn exists (D-30)", dynout::tag_name(t)),
                ));
            }
        }
    }
    for t in [DT_STRTAB, DT_STRSZ, DT_SYMTAB, DT_SYMENT] {
        if !out.has(t) {
            v.push(fd(6, format!("{} is missing (D-30: always)", dynout::tag_name(t))));
        }
    }
    let fl = out.get(DT_FLAGS).unwrap_or(0);
    let f1 = out.get(DT_FLAGS_1).unwrap_or(0);
    let fl_ok = if format == Format::Mortar {
        if line.now { DF_BIND_NOW } else { 0 }
    } else {
        // DF_TEXTREL (4): GNU ld and mold under -z notext (D-47 refuses
        // the link for mortar; P-D4/P-D9 judge the records).
        dynout::DF_ORIGIN | 4 | DF_BIND_NOW | dynout::DF_STATIC_TLS
    };
    if fl & !fl_ok != 0 {
        v.push(fd(6, format!("DT_FLAGS {fl:#x} has bits {:#x} (D-30)", fl & !fl_ok)));
    }
    if format == Format::Mortar && out.has(DT_FLAGS) != line.now {
        v.push(fd(6, "DT_FLAGS is present iff -z now (D-30)"));
    }
    if f1 & !(DF_1_NOW | DF_1_PIE) != 0 {
        v.push(fd(
            6,
            format!("DT_FLAGS_1 {f1:#x} has bits {:#x} (D-30)", f1 & !(DF_1_NOW | DF_1_PIE)),
        ));
    }
    if format == Format::Mortar {
        let rel = out.rels.iter().any(|r| !r.plt && kind(m, r.ty) == Kind::Relative);
        if out.has(DT_RELACOUNT) != rel {
            v.push(fd(6, "DT_RELACOUNT is present iff some RELATIVE record (D-30)"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{nonpreemptible_rows, two_objects_at};

    fn table(version: u32, count: u32, row_size: u32, rows: &[(u64, u32, u16)]) -> Vec<u8> {
        let mut b = Vec::new();
        for w in [version, count, row_size, 0] {
            b.extend_from_slice(&w.to_le_bytes());
        }
        for &(key, dynsym, flags) in rows {
            b.extend_from_slice(&key.to_le_bytes());
            b.extend_from_slice(&[0; 8]);
            b.extend_from_slice(&dynsym.to_le_bytes());
            b.extend_from_slice(&flags.to_le_bytes());
            b.extend_from_slice(&[0; 2]);
        }
        b
    }

    #[test]
    fn pd24_rows_with_nonpreemptible() {
        let t = table(1, 3, 24, &[(1, 5, 0x8001), (2, 6, 1), (3, 7, 0x8000)]);
        assert_eq!(nonpreemptible_rows(&t).unwrap().into_iter().collect::<Vec<_>>(), [5, 7]);
        assert!(nonpreemptible_rows(&table(1, 0, 24, &[])).unwrap().is_empty());
    }

    #[test]
    fn pd24_malformed_tables() {
        assert!(nonpreemptible_rows(&[0; 15]).is_err(), "short");
        assert!(nonpreemptible_rows(&table(2, 0, 24, &[])).is_err(), "version");
        assert!(nonpreemptible_rows(&table(1, 0, 32, &[])).is_err(), "row size");
        assert!(nonpreemptible_rows(&table(1, 2, 24, &[(1, 5, 0)])).is_err(), "count");
        assert!(nonpreemptible_rows(&table(1, u32::MAX, 24, &[])).is_err(), "huge count");
        let t = table(1, 2, 24, &[(2, 5, 0), (1, 6, 0)]);
        assert!(nonpreemptible_rows(&t).is_err(), "key order");
        // Every truncation is a table or an error, never a panic.
        let t = table(1, 2, 24, &[(1, 5, 0x8000), (2, 6, 0)]);
        for n in 0..t.len() {
            let _ = nonpreemptible_rows(&t[..n]);
        }
    }

    #[test]
    fn pd24_places() {
        let loads = [(0, 0x1000, false), (0x1f000, 0x2000, true)];
        let relro = [(0x1f000, 0x1000)];
        assert_eq!(two_objects_at(&loads, &relro, 0x800), None, ".rodata");
        assert_eq!(two_objects_at(&loads, &relro, 0x1f800), None, "RELRO");
        assert!(two_objects_at(&loads, &relro, 0x20000).unwrap().starts_with("a writable"));
        assert!(two_objects_at(&loads, &[], 0x1f800).unwrap().starts_with("a writable"));
        assert!(two_objects_at(&loads, &relro, 0x21000).unwrap().starts_with("no PT_LOAD"));
        assert!(two_objects_at(&[], &relro, 0x10).unwrap().starts_with("no PT_LOAD"));
        // Overflowing headers: an unknown place, or no RELRO.
        let hostile = [(0, 0x1000, false), (u64::MAX - 1, 4, false)];
        assert!(two_objects_at(&hostile, &[], 0x10).unwrap().contains("past the address space"));
        let w = [(0x1000, 0x1000, true)];
        assert!(two_objects_at(&w, &[(0x1000, u64::MAX)], 0x1010).is_some());
        // Overlapping PT_LOADs: a writable one decides.
        let both = [(0, 0x2000, false), (0x1000, 0x1000, true)];
        assert!(two_objects_at(&both, &[], 0x1800).is_some());
    }
}

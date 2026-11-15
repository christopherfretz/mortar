//! x86.md §16 (stage A of x64.md §15): `apply` in the reference linkers'
//! layouts.
//!
//! Each program is linked by lld (22, else 19) `-static` and, where its
//! forms are legal there, `-static-pie`, with `--emit-relocs`, and by GNU
//! ld (`i686-linux-gnu-gcc`, `-fuse-ld=bfd`) as the run reference.
//!
//! **lld's output, input rebuilt** ([`compare`]). lld keeps the input types
//! in the records it emits, but its bytes are relaxed and every field holds
//! the value `X`, not the REL addend (X86-4, X86-7). For each record:
//!
//! 1. A relaxed window is rebuilt from lld's output form ([`invert`]), the
//!    inverse of the output columns of §8 and §9 (D1–D3, L1–L2, I1–I5,
//!    T1–T2; lld relaxes no `GOT32X`, §15.1). What the output forgot is
//!    chosen canonically: the base register of a D2/D3/L1/L2 lead and its
//!    `call *` partner, and of I4/I5/T1, becomes `%ebx` (rm = 011); no form's
//!    output depends on it (X86-34: both instructions are replaced).
//! 2. The implicit addend of a relaxed record is the one its form requires
//!    (X86-3): 0 for a lead, an IE/GOTIE field and a T1 field; −4 for a
//!    `PLT32`/`PC32` partner, 0 for a `GOT32`/`GOT32X` partner (§9's
//!    table, the "when" column). It is written into the rebuilt field.
//! 3. Every other field holds `X = X0 + A`, with `X0` the §6.1 row's value
//!    at `A = 0`, computed here from the spec ([`x0`]) with `S`, `P`, `G`,
//!    `GOT` and the TLS block read from lld's image (§3.1: `TpRel`,
//!    `DtpRel`; X86-61 for `TLS_LDO_32`; X86-13's `ModRM` reading for the
//!    `GOT32` base). So `A = X − X0` modulo `2^(8·width)`, sign-extended
//!    (X86-4, X86-6), and it is written back into the field. `G` is the
//!    slot lld's field addresses when the word there holds the slot's
//!    value (then `A = 0`), else a `.got` word holding that value.
//!    The derived addends are tallied per type; a GOT, IE or TLS-lead
//!    record whose derived `A` is not 0 is a mismatch (no producer writes
//!    one, §2.2).
//!
//! The rebuilt bytes are mortar's input: every record is applied again by
//! `mortar_x86` (`addend: None`, the addend read from the rebuilt field,
//! X86-5), and the bytes of each record's unit window are compared with
//! lld's: **ok**, **documented** (a §15.1 row, with mortar's bytes checked
//! against the spec's form: X86-25 G1–G5 where lld keeps the GOT, X86-32
//! I5 `add` where lld writes `lea`, X86-50 the static-pie IFUNC address
//! where lld uses its `ff a3` entry, W6), **ifunc** (an IFUNC whose IPLT
//! entry was not found), **emit** (a record that does not describe its
//! field: `.eh_frame`, which lld rewrites, a symbol lld dropped) or
//! **mismatch** (there must be none).
//!
//! **True input** ([`compare_object`]). The compiled objects are applied in
//! the reference's layout from their real input bytes (sections found by
//! the unique symbols they define, and confirmed by their bytes outside
//! every relocation window). In lld's layout this also checks the
//! reconstruction: for every record of the object, `S + A` (`A` alone for
//! `GOTPC`) from step 2 or 3 must equal the object's own.
//!
//! **Runs.** lld's image with mortar's bytes written in (rebuilt input,
//! and true input for the sites) is run under qemu-i386; its output must
//! equal GNU ld's output of the same objects. The sites
//! (`tests/sites/mod.rs`) check every form and print "N checks, 0
//! failures".
#![allow(
    clippy::many_single_char_names,
    clippy::too_many_lines,
    clippy::too_many_arguments,
    clippy::struct_excessive_bools,
    clippy::similar_names
)]

mod elf;
mod sites;
mod support;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

const SHF_MERGE: u64 = 0x10;

use elf::{
    Image, Rel, SHF_ALLOC, SHF_EXECINSTR, SHF_TLS, SHN_ABS, SHT_NOBITS, SHT_REL, STB_WEAK,
    STT_GNU_IFUNC, STT_SECTION, STT_TLS, Sym,
};
use mortar_arch::{
    Arch, DemandFlags, FieldError, FillCx, OutputKind, Reloc, RelocAt, SectionOut, Site, SlotKind,
    TargetKind, TlsBlock, UnitWindow,
};
use mortar_core::{FileId, SectionId};
use mortar_testutil::{Outcome, Scratch, run, tool};
use mortar_x86::X86;
use sites::Set;
use support::{Tables, key};

// ---------------------------------------------------------------- tools and builds

struct Tools {
    gcc: PathBuf,
    gxx: PathBuf,
    gfortran: Option<PathBuf>,
    clang: Option<PathBuf>,
    lld_dir: PathBuf,
    qemu: PathBuf,
}

fn tools() -> Option<Tools> {
    let lld = tool("ld.lld")?;
    // `-fuse-ld=lld -B<dir>/` needs a directory holding `ld.lld`.
    let lld_dir = lld.parent()?.to_path_buf();
    let lld_dir = if lld_dir.join("ld.lld").is_file() {
        lld_dir
    } else {
        [PathBuf::from("/usr/lib/llvm-22/bin"), PathBuf::from("/usr/lib/llvm-19/bin")]
            .into_iter()
            .find(|d| d.join("ld.lld").is_file())?
    };
    let gfortran = tool("i686-linux-gnu-gfortran");
    let clang = tool("clang");
    Some(Tools {
        gcc: tool("i686-linux-gnu-gcc")?,
        gxx: tool("i686-linux-gnu-g++")?,
        gfortran,
        clang,
        lld_dir,
        qemu: tool("qemu-i386")?,
    })
}

fn must(what: &str, o: &Outcome) {
    assert!(o.success(), "{what} failed: {}", o.stderr_text());
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cc {
    Gcc,
    Gxx,
    Gfortran,
    Clang,
}

struct Build {
    name: &'static str,
    /// `(compiler, file, flags)` per object; a sites build's first object
    /// is the sites.
    objects: &'static [(Cc, &'static str, &'static [&'static str])],
    driver: Cc,
    link: &'static [&'static str],
    pie: bool,
    /// lld links it (not T1 into a register other than `%eax`, X86-37).
    lld: bool,
    /// GNU ld links it (not L1 with a base other than `%ebx`, X86-35; not
    /// the thread-local `weak0`, whose value GNU ld writes as `TpRel(0)`,
    /// X86-23).
    bfd: bool,
    sites: bool,
}

const fn b(
    name: &'static str,
    objects: &'static [(Cc, &'static str, &'static [&'static str])],
    driver: Cc,
    link: &'static [&'static str],
    pie: bool,
) -> Build {
    Build { name, objects, driver, link, pie, lld: true, bfd: true, sites: false }
}

const MAIN_N: (Cc, &str, &[&str]) = (Cc::Gcc, "relaxmain.c", &["-O1", "-fno-pic"]);
const MAIN_P: (Cc, &str, &[&str]) = (Cc::Gcc, "relaxmain.c", &["-O1", "-fPIE"]);
const PIC: &[&str] = &["-O2", "-fPIC"];

const BUILDS: &[Build] = &[
    Build {
        sites: true,
        ..b("sites", &[(Cc::Gcc, "sites-common.s", &[]), MAIN_N], Cc::Gcc, &[], false)
    },
    Build {
        sites: true,
        ..b("sites-pie", &[(Cc::Gcc, "sites-common-pic.s", &[]), MAIN_P], Cc::Gcc, &[], true)
    },
    Build {
        sites: true,
        bfd: false,
        ..b("sites-lld", &[(Cc::Gcc, "sites-lld.s", &[]), MAIN_N], Cc::Gcc, &[], false)
    },
    Build {
        sites: true,
        bfd: false,
        ..b("sites-lld-pie", &[(Cc::Gcc, "sites-lld-pic.s", &[]), MAIN_P], Cc::Gcc, &[], true)
    },
    Build {
        sites: true,
        lld: false,
        ..b("sites-bfd", &[(Cc::Gcc, "sites-bfd-pic.s", &[]), MAIN_N], Cc::Gcc, &[], false)
    },
    Build {
        sites: true,
        lld: false,
        ..b("sites-bfd-pie", &[(Cc::Gcc, "sites-bfd-pic.s", &[]), MAIN_P], Cc::Gcc, &[], true)
    },
    b("gd-pic", &[(Cc::Gcc, "gd.c", PIC)], Cc::Gcc, &[], false),
    b("gd-pie", &[(Cc::Gcc, "gd.c", PIC)], Cc::Gcc, &[], true),
    b(
        "gd-noplt",
        &[(Cc::Gcc, "gd.c", &["-O1", "-fPIC", "-fno-plt", "-ftls-model=local-dynamic"])],
        Cc::Gcc,
        &[],
        false,
    ),
    b(
        "gd-noplt-got32",
        &[(Cc::Gcc, "gd.c", &["-O1", "-fPIC", "-fno-plt", "-Wa,-mrelax-relocations=no"])],
        Cc::Gcc,
        &[],
        false,
    ),
    b(
        "gd-gnu2",
        &[(Cc::Gcc, "gd.c", &["-O2", "-fPIC", "-mtls-dialect=gnu2"])],
        Cc::Gcc,
        &[],
        false,
    ),
    b(
        "gd-ld-gnu2",
        &[(Cc::Gcc, "gd.c", &["-O2", "-fPIC", "-ftls-model=local-dynamic", "-mtls-dialect=gnu2"])],
        Cc::Gcc,
        &[],
        true,
    ),
    b("gd-clang", &[(Cc::Clang, "gd.c", PIC)], Cc::Gcc, &[], false),
    b("p", &[(Cc::Gcc, "p.c", &["-O2", "-g"])], Cc::Gcc, &["-pthread"], false),
    b(
        "p-noplt",
        &[(Cc::Gcc, "p.c", &["-O2", "-fno-pic", "-fno-plt"])],
        Cc::Gcc,
        &["-pthread"],
        false,
    ),
    // X86-50: GCC `-fPIE` takes `&sel` by `GOTOFF`, refused against an
    // IFUNC in static-pie (by GNU ld too).
    b("p-pie", &[(Cc::Gcc, "p.c", &["-O2", "-fPIE", "-DNOIFUNC"])], Cc::Gcc, &["-pthread"], true),
    b("q", &[(Cc::Gxx, "q.cc", &["-O2", "-g"])], Cc::Gxx, &["-pthread"], false),
    b("q-pie", &[(Cc::Gxx, "q.cc", &["-O2", "-fPIE"])], Cc::Gxx, &["-pthread"], true),
    b("f", &[(Cc::Gfortran, "f.f90", &["-O2"])], Cc::Gfortran, &[], false),
];

/// The sites' assembly and number of checks, by file name.
fn sites_file(name: &str) -> Option<(String, usize)> {
    Some(match name {
        "sites-common.s" => sites::sites_s(Set::Common, false),
        "sites-common-pic.s" => sites::sites_s(Set::Common, true),
        "sites-lld.s" => sites::sites_s(Set::Lld, false),
        "sites-lld-pic.s" => sites::sites_s(Set::Lld, true),
        "sites-bfd-pic.s" => sites::sites_s(Set::Bfd, true),
        _ => return None,
    })
}

fn source(name: &str) -> String {
    if let Some((s, _)) = sites_file(name) {
        return s;
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn has(t: &Tools, cc: Cc) -> bool {
    match cc {
        Cc::Gfortran => t.gfortran.is_some(),
        Cc::Clang => t.clang.is_some(),
        _ => true,
    }
}

fn compile(t: &Tools, dir: &Scratch, b: &Build) -> Vec<PathBuf> {
    let mut objs = Vec::new();
    for (i, &(cc, file, flags)) in b.objects.iter().enumerate() {
        let src = dir.write(&format!("{}-{i}-{file}", b.name), source(file));
        let obj = dir.join(format!("{}-{i}.o", b.name));
        let mut args: Vec<String> = Vec::new();
        let exe = match cc {
            Cc::Gcc => &t.gcc,
            Cc::Gxx => &t.gxx,
            Cc::Gfortran => {
                args.push(format!("-J{}", dir.path().display()));
                t.gfortran.as_ref().unwrap()
            }
            Cc::Clang => {
                args.push("--target=i686-linux-gnu".into());
                t.clang.as_ref().unwrap()
            }
        };
        args.push("-c".into());
        args.extend(flags.iter().map(|s| (*s).to_string()));
        args.extend([src.display().to_string(), "-o".into(), obj.display().to_string()]);
        must(&format!("compile {} {file}", b.name), &run(exe, &args));
        objs.push(obj);
    }
    objs
}

fn link(t: &Tools, b: &Build, objs: &[PathBuf], out: &Path, lld: bool) -> Outcome {
    let driver = match b.driver {
        Cc::Gxx => &t.gxx,
        Cc::Gfortran => t.gfortran.as_ref().unwrap(),
        _ => &t.gcc,
    };
    let mut args: Vec<String> =
        if b.pie { vec!["-static-pie".into()] } else { vec!["-static".into(), "-no-pie".into()] };
    if lld {
        args.extend([
            "-fuse-ld=lld".into(),
            format!("-B{}/", t.lld_dir.display()),
            "-Wl,--emit-relocs".into(),
        ]);
    } else {
        args.push("-fuse-ld=bfd".into());
    }
    args.push("-Wl,--build-id=none".into());
    args.extend(b.link.iter().map(|s| (*s).to_string()));
    args.extend(objs.iter().map(|o| o.display().to_string()));
    args.extend(["-o".into(), out.display().to_string()]);
    run(driver, &args)
}

// ---------------------------------------------------------------- the spec's terms

const R_32: u32 = 1;
const PC32: u32 = 2;
const GOT32: u32 = 3;
const PLT32: u32 = 4;
const GOTOFF: u32 = 9;
const GOTPC: u32 = 10;
const TLS_IE: u32 = 15;
const TLS_GOTIE: u32 = 16;
const TLS_LE: u32 = 17;
const TLS_GD: u32 = 18;
const TLS_LDM: u32 = 19;
const TLS_LDO_32: u32 = 32;
const SIZE32: u32 = 38;
const TLS_GOTDESC: u32 = 39;
const TLS_DESC_CALL: u32 = 40;
const GOT32X: u32 = 43;
const IRELATIVE: u32 = 42;

/// §3.3: the field's width.
const fn width(r_type: u32) -> usize {
    match r_type {
        1..=4 | 9 | 10 | 15..=19 | 32 | 38 | 39 | 43 => 4,
        20 | 21 => 2,
        22 | 23 => 1,
        _ => 0,
    }
}

/// §3.1 (variant II, §13).
fn tp_rel(x: u64, t: TlsBlock) -> u64 {
    let size = t.size.div_ceil(t.align.max(1)) * t.align.max(1);
    x.wrapping_sub(t.start).wrapping_sub(size)
}

fn dtp_rel(x: u64, t: TlsBlock) -> u64 {
    x.wrapping_sub(t.start)
}

/// X86-13 step 2: the `ModRM` at `P − 1` reads as no base (`00 reg 101`).
fn no_base(v: &[u8], p: usize) -> bool {
    p >= 1 && v[p - 1] & 0xC7 == 0x05
}

fn mask(w: usize) -> u64 {
    if w >= 8 { u64::MAX } else { (1u64 << (8 * w)) - 1 }
}

fn sign_extend(x: u64, w: usize) -> i64 {
    match w {
        1 => i64::from(x as u8 as i8),
        2 => i64::from(x as u16 as i16),
        _ => i64::from(x as u32 as i32),
    }
}

fn field(v: &[u8], p: usize, w: usize) -> Option<u64> {
    let b = v.get(p..p.checked_add(w)?)?;
    let mut x = [0u8; 8];
    x[..w].copy_from_slice(b);
    Some(u64::from_le_bytes(x))
}

fn put_field(v: &mut [u8], p: usize, w: usize, a: i64) {
    v[p..p + w].copy_from_slice(&a.to_le_bytes()[..w]);
}

// ---------------------------------------------------------------- symbols

/// An IFUNC's IPLT entry and resolver (§11.3), found from the image's
/// `IRELATIVE` words and the `ff 25 <word>` / `ff a3 <word − GOT>` entries
/// that jump through them (X86-48, X86-49).
#[derive(Clone, Copy, Debug)]
struct Ifn {
    entry: u64,
    resolver: u64,
}

/// The 4-byte word of an allocated `PROGBITS` section at `addr`.
fn word_at(img: &Image, addr: u64) -> Option<u64> {
    let s = img.sections.iter().find(|s| {
        s.flags & SHF_ALLOC != 0
            && s.kind != SHT_NOBITS
            && s.addr <= addr
            && addr + 4 <= s.addr + s.size as u64
    })?;
    field(img.data(s), (addr - s.addr) as usize, 4)
}

fn got_base(img: &Image, syms: &[Sym]) -> Option<u64> {
    syms.iter()
        .find(|s| s.name == "_GLOBAL_OFFSET_TABLE_")
        .map(|s| s.value)
        .or_else(|| img.section(".got.plt").map(|s| s.addr))
}

/// Every IFUNC of `img`, by entry and by resolver address.
fn ifuncs(img: &Image, got: Option<u64>) -> BTreeMap<u64, Ifn> {
    let mut out = BTreeMap::new();
    let plts: Vec<_> = img
        .sections
        .iter()
        .filter(|s| s.flags & SHF_EXECINSTR != 0 && s.name.contains("plt"))
        .collect();
    for rs in img.sections.iter().filter(|s| s.kind == SHT_REL && s.flags & SHF_ALLOC != 0) {
        for r in img.rels(rs).into_iter().filter(|r| r.r_type == IRELATIVE) {
            let Some(resolver) = word_at(img, r.offset) else { continue };
            let abs = (r.offset as u32).to_le_bytes();
            let rel = got.map(|g| (r.offset.wrapping_sub(g) as u32).to_le_bytes());
            for s in &plts {
                let d = img.data(s);
                for o in 0..d.len().saturating_sub(5) {
                    let hit = (d[o] == 0xff && d[o + 1] == 0x25 && d[o + 2..o + 6] == abs)
                        || (d[o] == 0xff
                            && d[o + 1] == 0xa3
                            && rel.is_some_and(|x| d[o + 2..o + 6] == x));
                    if hit {
                        let ibt = o >= 4 && d[o - 4..o] == [0xf3, 0x0f, 0x1e, 0xfb];
                        let entry = s.addr + o as u64 - if ibt { 4 } else { 0 };
                        let f = Ifn { entry, resolver };
                        out.insert(entry, f);
                        out.insert(resolver, f);
                    }
                }
            }
        }
    }
    out
}

/// The symbol's IFUNC, if it is one: GNU ld keeps `STT_GNU_IFUNC` at the
/// resolver; lld writes `STT_FUNC` at the IPLT entry (its canonical
/// address in both outputs).
fn ifunc_of(s: &Sym, ifn: &BTreeMap<u64, Ifn>) -> Option<Ifn> {
    if s.shndx == 0 || s.shndx == SHN_ABS {
        return None;
    }
    let f = ifn.get(&s.value)?;
    (s.info & 0xF == STT_GNU_IFUNC || f.entry == s.value).then_some(*f)
}

/// Enters an output symbol of the image into `t` under `k`: its kind and
/// `S` as mortar's tables give them (§4, §7, §11.3, §13).
fn enter(
    t: &mut Tables,
    k: mortar_core::SymKey,
    img: &Image,
    s: &Sym,
    ifn: &BTreeMap<u64, Ifn>,
    output: OutputKind,
) {
    let ty = s.info & 0xF;
    let tls_base = img.tls.map_or(0, |t| t.start);
    let in_tls = img.sections.get(usize::from(s.shndx)).is_some_and(|x| x.flags & SHF_TLS != 0);
    if s.shndx == 0 {
        // lld writes a hidden undefined weak as an undefined local.
        let kind = if matches!(s.info >> 4, STB_WEAK | 0) {
            TargetKind::Weak0
        } else {
            TargetKind::Undefined
        };
        if ty == STT_TLS {
            t.tls_sym(k, 0, kind);
        } else {
            t.sym(k, 0, kind);
        }
    } else if ty == STT_TLS {
        // `_TLS_MODULE_BASE_` is an `SHN_ABS` TLS symbol at offset 0.
        t.tls_sym(k, tls_base + s.value, TargetKind::Local);
    } else if let Some(f) = ifunc_of(s, ifn) {
        // X86-50: in static the entry; in static-pie the resolved
        // function (its `IRELATIVE` word), with `PLT32` to the entry.
        let v = if output == OutputKind::Pie { f.resolver } else { f.entry };
        t.sym(k, v, TargetKind::Ifunc);
        t.plts.insert(k, f.entry);
        t.flags.insert(k, DemandFlags::IPLT);
    } else if s.shndx == SHN_ABS {
        t.sym(k, s.value, TargetKind::Abs);
    } else if in_tls {
        // A section symbol of `.tdata`/`.tbss`: an address.
        t.tls_sym(k, s.value, TargetKind::Local);
    } else {
        t.sym(k, s.value, TargetKind::Local);
    }
    t.syms.get_mut(&k).unwrap().size = s.size;
    t.names.insert(k, s.name.as_bytes().to_vec());
}

// ---------------------------------------------------------------- rebuilding the input

/// Rebuilds in `v` (lld's bytes) the input bytes of a site lld relaxed,
/// for the record of `r_type` at `p` whose next record is `next`: the
/// inverse of the output column of x86.md §8/§9, with the form's addends
/// in the fields. Returns the form and whether the next record (the
/// partner) was rebuilt too; `None` when the bytes are not relaxed.
fn invert(
    v: &mut [u8],
    p: usize,
    r_type: u32,
    next: Option<(u32, usize)>,
) -> Option<(&'static str, bool)> {
    const GS0_SUB: [u8; 8] = [0x65, 0xa1, 0, 0, 0, 0, 0x81, 0xe8];
    const M4: [u8; 4] = (-4i32).to_le_bytes();
    let at = |v: &[u8], a: usize, b: &[u8]| v.get(a..a + b.len()) == Some(b);
    let partner = |types: &[u32], delta: usize| {
        next.is_some_and(|(t, q)| types.contains(&t) && q == p + delta)
    };
    match r_type {
        TLS_GD => {
            // D1: `8d 04 1d <0> e8 <−4>` at `P − 3`.
            if p >= 3 && at(v, p - 3, &GS0_SUB) && partner(&[PLT32, PC32], 5) {
                v[p - 3..p + 9].copy_from_slice(&[
                    0x8d, 0x04, 0x1d, 0, 0, 0, 0, 0xe8, M4[0], M4[1], M4[2], M4[3],
                ]);
                return Some(("D1", true));
            }
            if p < 2 || !at(v, p - 2, &GS0_SUB) {
                return None;
            }
            if partner(&[GOT32, GOT32X], 6) {
                // D2: `8d 83 <0> ff 93 <0>`.
                v[p - 2..p + 10].copy_from_slice(&[0x8d, 0x83, 0, 0, 0, 0, 0xff, 0x93, 0, 0, 0, 0]);
                Some(("D2", true))
            } else if partner(&[PLT32, PC32], 5) {
                // D3: `8d 83 <0> e8 <−4> 90`.
                v[p - 2..p + 10].copy_from_slice(&[
                    0x8d, 0x83, 0, 0, 0, 0, 0xe8, M4[0], M4[1], M4[2], M4[3], 0x90,
                ]);
                Some(("D3", true))
            } else {
                None
            }
        }
        TLS_LDM => {
            if p < 2 {
                return None;
            }
            if at(v, p - 2, &[0x65, 0xa1, 0, 0, 0, 0, 0x90, 0x8d, 0x74, 0x26, 0x00])
                && partner(&[PLT32, PC32], 5)
            {
                v[p - 2..p + 9]
                    .copy_from_slice(&[0x8d, 0x83, 0, 0, 0, 0, 0xe8, M4[0], M4[1], M4[2], M4[3]]);
                Some(("L1", true))
            } else if at(v, p - 2, &[0x65, 0xa1, 0, 0, 0, 0, 0x8d, 0xb6, 0, 0, 0, 0])
                && partner(&[GOT32, GOT32X], 6)
            {
                v[p - 2..p + 10].copy_from_slice(&[0x8d, 0x83, 0, 0, 0, 0, 0xff, 0x93, 0, 0, 0, 0]);
                Some(("L2", true))
            } else {
                None
            }
        }
        TLS_GOTDESC => {
            let (op, m) = (*v.get(p.checked_sub(2)?)?, v[p - 1]);
            (op == 0x8d && m & 0xC7 == 0x05).then(|| {
                v[p - 2..p + 4].copy_from_slice(&[0x8d, 0x83 | (m & 0x38), 0, 0, 0, 0]);
                ("T1", false)
            })
        }
        TLS_DESC_CALL => at(v, p, &[0x66, 0x90]).then(|| {
            v[p..p + 2].copy_from_slice(&[0xff, 0x10]);
            ("T2", false)
        }),
        TLS_IE => {
            if *v.get(p.checked_sub(1)?)? == 0xb8 {
                v[p - 1..p + 4].copy_from_slice(&[0xa1, 0, 0, 0, 0]);
                return Some(("I1", false));
            }
            let (op, m) = (*v.get(p.checked_sub(2)?)?, v[p - 1]);
            if m & 0xF8 != 0xC0 {
                return None;
            }
            let (name, op) = match op {
                0xc7 => ("I2", 0x8b),
                0x81 => ("I3", 0x03),
                _ => return None,
            };
            v[p - 2..p + 4].copy_from_slice(&[op, 0x05 | ((m & 7) << 3), 0, 0, 0, 0]);
            Some((name, false))
        }
        TLS_GOTIE => {
            let (op, m) = (*v.get(p.checked_sub(2)?)?, v[p - 1]);
            let (name, op, reg) = match op {
                0xc7 if m & 0xF8 == 0xC0 => ("I4", 0x8b, m & 7),
                0x81 if m & 0xF8 == 0xC0 => ("I5", 0x03, m & 7),
                // lld's I5: `lea T(%reg), %reg` (§15.1).
                0x8d if m & 0xC0 == 0x80 && (m >> 3) & 7 == m & 7 && m & 7 != 4 => {
                    ("I5 lea (lld)", 0x03, m & 7)
                }
                _ => return None,
            };
            v[p - 2..p + 4].copy_from_slice(&[op, 0x83 | (reg << 3), 0, 0, 0, 0]);
            Some((name, false))
        }
        _ => None,
    }
}

/// The value the §6.1 row of `r_type` gives at `A = 0` (`X0`), modulo
/// `2^32`. `slot` is `G` (or `G(GTPREL(S))`) for the GOT and IE rows.
fn x0(
    r_type: u32,
    sv: mortar_arch::SymValue,
    p: u64,
    got: Option<u64>,
    tls: Option<TlsBlock>,
    slot: Option<u64>,
    exec: bool,
    alloc: bool,
    nobase: bool,
) -> Option<u64> {
    let weak0 = sv.kind == TargetKind::Weak0;
    let s = if weak0 { 0 } else { sv.s };
    let tls_off = |f: fn(u64, TlsBlock) -> u64| if weak0 { Some(0) } else { tls.map(|t| f(s, t)) };
    Some(
        match r_type {
            // X86-62: `32`/`16`/`8` against TLS at a non-alloc place.
            1 | 20 | 22 if sv.tls && !alloc => tls_off(dtp_rel)?,
            1 | 20 | 22 => s,
            2 | 21 | 23 => s.wrapping_sub(p),
            // `L`: the IPLT entry of an IFUNC in static-pie (X86-50).
            PLT32 => {
                let l = if sv.kind == TargetKind::Ifunc { slot.unwrap_or(s) } else { s };
                l.wrapping_sub(p)
            }
            GOTOFF => s.wrapping_sub(got?),
            GOTPC => got?.wrapping_sub(p),
            GOT32 | GOT32X if nobase => slot?,
            GOT32 | GOT32X | TLS_GOTIE => slot?.wrapping_sub(got?),
            TLS_IE => slot?,
            TLS_LE => tls_off(tp_rel)?,
            // X86-61.
            TLS_LDO_32 if exec && alloc => tls_off(tp_rel)?,
            TLS_LDO_32 => tls_off(dtp_rel)?,
            SIZE32 => sv.size,
            _ => return None,
        } & 0xFFFF_FFFF,
    )
}

/// The slot `G` a GOT or IE record of lld's uses: the word its field
/// addresses (with `A = 0`) when that word holds one of `wants`, else a
/// word of `.got`/`.got.plt` holding one.
fn find_slot(
    img: &Image,
    words: &BTreeMap<u64, Vec<u64>>,
    cand: u64,
    wants: &[u64],
) -> Option<u64> {
    let in_got = |a: u64| {
        img.sections.iter().any(|s| {
            (s.name == ".got" || s.name == ".got.plt")
                && s.addr <= a
                && a + 4 <= s.addr + s.size as u64
        })
    };
    if in_got(cand) && word_at(img, cand).is_some_and(|w| wants.contains(&w)) {
        return Some(cand);
    }
    wants.iter().find_map(|w| words.get(w).map(|c| c[0]))
}

/// The values a slot of `kind` for `k` may hold in the reference: 0 for
/// `weak0`; `S`; for an IFUNC its IPLT entry or its resolver (the word an
/// `IRELATIVE` resolves, X86-28); `TpRel(S)` for `GTPREL` (§3.1).
fn slot_wants(
    t: &Tables,
    ifn: &BTreeMap<u64, Ifn>,
    k: mortar_core::SymKey,
    kind: SlotKind,
    tls: Option<TlsBlock>,
) -> Vec<u64> {
    let sv = t.syms[&k];
    match (kind, sv.kind) {
        (_, TargetKind::Weak0) => vec![0],
        (SlotKind::Addr, TargetKind::Ifunc) => {
            let mut v = vec![sv.s];
            if let Some(f) = t.plts.get(&k).and_then(|e| ifn.get(e)) {
                v.extend([f.entry, f.resolver]);
            }
            v
        }
        (SlotKind::Addr, _) => vec![sv.s],
        _ => tls.map(|t| vec![tp_rel(sv.s, t) & 0xFFFF_FFFF]).unwrap_or_default(),
    }
}

// ---------------------------------------------------------------- the spec's relaxed forms

/// G1–G5 (x86.md §8's output column) for the `GOT32X` input at `p`.
fn spec_g(v: &[u8], p: usize, s: u64, paddr: u64) -> Option<[u8; 6]> {
    let (op, m) = (*v.get(p.checked_sub(2)?)?, v[p - 1]);
    if !(m & 0xC7 == 0x05 || (m & 0xC0 == 0x80 && m & 7 != 4)) {
        return None;
    }
    let reg = (m >> 3) & 7;
    let i = (s as u32).to_le_bytes();
    Some(match op {
        0x8b => [0xc7, 0xc0 | reg, i[0], i[1], i[2], i[3]],
        0x03 | 0x0b | 0x13 | 0x1b | 0x23 | 0x2b | 0x33 | 0x3b => {
            [0x81, 0xc0 | (op & 0x38) | reg, i[0], i[1], i[2], i[3]]
        }
        0x85 => [0xf7, 0xc0 | reg, i[0], i[1], i[2], i[3]],
        0xff if reg == 2 => {
            let r = (s.wrapping_sub(paddr + 4) as u32).to_le_bytes();
            [0x67, 0xe8, r[0], r[1], r[2], r[3]]
        }
        0xff if reg == 4 => {
            let r = (s.wrapping_sub(paddr + 3) as u32).to_le_bytes();
            [0xe9, r[0], r[1], r[2], r[3], 0x90]
        }
        _ => return None,
    })
}

/// `T` of the IE/GD/TLSDESC forms: `TpRel(S)`, 0 for `weak0` (X86-23).
fn t_value(sv: mortar_arch::SymValue, t: TlsBlock) -> u64 {
    if sv.kind == TargetKind::Weak0 { 0 } else { tp_rel(sv.s, t) }
}

/// I5 (§9's output column): `81 (c0 | reg) T` for the `TLS_GOTIE` input.
fn spec_i5(v: &[u8], p: usize, t: u64) -> Option<[u8; 6]> {
    let (op, m) = (*v.get(p.checked_sub(2)?)?, v[p - 1]);
    (op == 0x03).then(|| {
        let t = (t as u32).to_le_bytes();
        [0x81, 0xc0 | ((m >> 3) & 7), t[0], t[1], t[2], t[3]]
    })
}

// ---------------------------------------------------------------- comparing

#[derive(Default, Debug, Clone, Copy)]
struct Tally {
    ok: u32,
    documented: u32,
    ifunc: u32,
    emit: u32,
    mismatch: u32,
}

#[derive(Default)]
struct Report {
    by_type: BTreeMap<u32, Tally>,
    /// The forms lld (or the reference) wrote, by name.
    forms: BTreeMap<&'static str, u32>,
    /// The documented differences, by rule.
    documented: BTreeMap<&'static str, u32>,
    mismatches: Vec<String>,
    /// The derived addends, by type and value (the most frequent kept).
    addends: BTreeMap<u32, BTreeMap<i64, u32>>,
    /// The reconstruction checked against the objects' own bytes.
    recon_ok: u32,
    recon_bad: Vec<String>,
    /// Object sections not found in a reference (C programs only).
    unlocated: Vec<String>,
    /// True-input records whose target is in an `SHF_MERGE` section: its
    /// pieces are not placed at a linear offset, so the object's section
    /// symbol does not give `S` (the rebuilt-input pass covers them).
    merged: u32,
    /// IFUNCs found (IPLT entry and resolver) in the images.
    ifuncs: u32,
}

/// The `S + A` (`A` for `GOTPC`) of a record, modulo its field width: what
/// the reconstruction must agree on with the object.
#[derive(Clone, Copy)]
struct Recon {
    r_type: u32,
    sa: u64,
}

fn name(t: u32) -> &'static str {
    mortar_elf::reloc::i386::name(t).unwrap_or("?")
}

fn sa(r_type: u32, s: u64, a: i64) -> u64 {
    let x = if r_type == GOTPC { a as u64 } else { s.wrapping_add(a as u64) };
    x & mask(width(r_type).max(1))
}

type Patches = Vec<(usize, Vec<u8>)>;

/// x86.md §16 on lld's output: every record of every section rebuilt,
/// applied and compared. Returns mortar's bytes of the allocated sections
/// and the reconstruction, by place.
fn compare(
    label: &str,
    img: &Image,
    output: OutputKind,
    rep: &mut Report,
) -> (Patches, BTreeMap<u64, Recon>) {
    let syms = img.symbols();
    let tls = img.tls;
    let got = got_base(img, &syms);
    let ifn = ifuncs(img, got);
    rep.ifuncs += ifn.values().filter(|f| ifn.contains_key(&f.entry)).count() as u32 / 2;
    let words = img.got_words();
    let mut tables = Tables::default();
    for (i, s) in syms.iter().enumerate() {
        enter(&mut tables, key(u32::try_from(i).unwrap()), img, s, &ifn, output);
    }
    let x = X86::M1;
    let mut patches = Vec::new();
    let mut recon = BTreeMap::new();
    for rs in
        img.sections.iter().filter(|s| s.kind == SHT_REL && s.flags & SHF_ALLOC == 0 && s.info != 0)
    {
        let sec = &img.sections[rs.info as usize];
        let rels = img.rels(rs);
        if sec.kind == SHT_NOBITS || sec.name == ".eh_frame" {
            for r in &rels {
                rep.by_type.entry(r.r_type).or_default().emit += 1;
            }
            continue;
        }
        let lld = img.data(sec).to_vec();
        let alloc = sec.flags & SHF_ALLOC != 0;
        let exec = sec.flags & SHF_EXECINSTR != 0;
        let base = if alloc { sec.addr } else { 0 };
        let recs: Vec<Reloc> = rels
            .iter()
            .map(|r| Reloc {
                r_offset: r.offset.wrapping_sub(base),
                key: key(r.sym),
                r_type: r.r_type,
                addend: 0,
            })
            .collect();
        let n = recs.len();
        // 1, 2: the relaxed windows, with their forms' addends.
        let mut view = lld.clone();
        let mut forms: Vec<Option<&'static str>> = vec![None; n];
        let mut fixed = vec![false; n];
        for i in 0..n {
            let p = recs[i].r_offset as usize;
            if fixed[i] || p >= view.len() {
                continue;
            }
            let next = recs.get(i + 1).map(|r| (r.r_type, r.r_offset as usize));
            if let Some((f, pair)) = invert(&mut view, p, recs[i].r_type, next) {
                forms[i] = Some(f);
                *rep.forms.entry(f).or_default() += 1;
                fixed[i] = true;
                if pair {
                    fixed[i + 1] = true;
                }
            }
        }
        // 3: every other field's addend, from lld's value.
        for i in 0..n {
            let r = recs[i];
            let w = width(r.r_type);
            let p = r.r_offset as usize;
            let paddr = sec.addr.wrapping_add(r.r_offset);
            if rels[i].sym == 0 || w == 0 {
                continue;
            }
            let sv = tables.syms[&r.key];
            if fixed[i] {
                let a = field(&view, p, w).map_or(0, |f| sign_extend(f, w));
                recon.insert(
                    paddr,
                    Recon {
                        r_type: r.r_type,
                        sa: sa(r.r_type, if sv.kind == TargetKind::Weak0 { 0 } else { sv.s }, a),
                    },
                );
                continue;
            }
            let Some(fv) = field(&lld, p, w) else { continue };
            let nobase = no_base(&view, p);
            // The slot (the IPLT entry for an IFUNC's `PLT32` in static-pie).
            let slot = match r.r_type {
                GOT32 | GOT32X | TLS_IE | TLS_GOTIE => {
                    let kind = if r.r_type == TLS_IE || r.r_type == TLS_GOTIE {
                        SlotKind::TpOff { negated: false }
                    } else {
                        SlotKind::Addr
                    };
                    let wants = slot_wants(&tables, &ifn, r.key, kind, tls);
                    let cand = if nobase || r.r_type == TLS_IE {
                        fv
                    } else {
                        fv.wrapping_add(got.unwrap_or(0)) & 0xFFFF_FFFF
                    };
                    let g = find_slot(img, &words, cand, &wants);
                    if let Some(g) = g {
                        tables.slots.entry((r.key, kind)).or_insert(g);
                    }
                    tables.slots.get(&(r.key, kind)).copied()
                }
                PLT32 => tables.plts.get(&r.key).copied().filter(|_| output == OutputKind::Pie),
                _ => None,
            };
            let Some(x0) = x0(r.r_type, sv, paddr, got, tls, slot, exec, alloc, nobase) else {
                continue;
            };
            let a = sign_extend(fv.wrapping_sub(x0) & mask(w), w);
            put_field(&mut view, p, w, a);
            let hist = rep.addends.entry(r.r_type).or_default();
            if hist.len() < 64 || hist.contains_key(&a) {
                *hist.entry(a).or_default() += 1;
            }
            if alloc {
                recon.insert(
                    paddr,
                    Recon {
                        r_type: r.r_type,
                        sa: sa(r.r_type, if sv.kind == TargetKind::Weak0 { 0 } else { sv.s }, a),
                    },
                );
            }
            // §2.2: no producer gives a GOT, IE or TLS-lead record an addend.
            if a != 0
                && matches!(
                    r.r_type,
                    GOT32 | GOT32X | TLS_IE | TLS_GOTIE | TLS_GD | TLS_LDM | TLS_GOTDESC
                )
            {
                rep.by_type.entry(r.r_type).or_default().mismatch += 1;
                rep.mismatches.push(format!(
                    "{label}: {} {}+{:#x} against `{}`: derived A = {a} (field {fv:#x})",
                    name(r.r_type),
                    sec.name,
                    r.r_offset,
                    syms[rels[i].sym as usize].name
                ));
                fixed[i] = true;
            }
        }
        // Apply every record to the rebuilt input.
        let cx = FillCx { tables: &tables, output, got_base: got, tls };
        let mut mine = view.clone();
        let mut results = Vec::with_capacity(n);
        let mut windows: Vec<Option<UnitWindow>> = Vec::with_capacity(n);
        for (i, r) in recs.iter().enumerate() {
            let site = Site { bytes: &view, offset: r.r_offset };
            let h = match x.classify(r.r_type, site) {
                Ok(h) => h,
                Err(e) => {
                    results.push(Err(FieldError::Refused(e)));
                    windows.push(None);
                    continue;
                }
            };
            let at = RelocAt {
                file: FileId(1),
                section: SectionId(rs.info),
                index: i as u32,
                r_offset: r.r_offset,
                key: r.key,
                r_type: r.r_type,
                addend: None,
                howto: &h,
                site,
                records: &recs.as_slice(),
                proof: &[],
                leads: true,
            };
            windows.push(x.unit_window(output, &at, &tables));
            if rels[i].sym == 0 && r.r_type != 0 {
                results.push(Ok(()));
                continue;
            }
            let mut out =
                SectionOut::whole(&mut mine, sec.addr, alloc, sec.name.as_bytes()).with_exec(exec);
            results.push(x.apply(&cx, &at, &mut out));
        }
        let mut restore: Vec<(usize, usize)> = Vec::new();
        for (i, r) in recs.iter().enumerate() {
            let rel = rels[i];
            let tally = rep.by_type.entry(r.r_type).or_default();
            let p = r.r_offset as usize;
            let w = width(r.r_type);
            let (a, b) = windows[i].map_or((p, (p + w).min(lld.len())), |u| {
                (u.start as usize, (u.end as usize).min(lld.len()))
            });
            if rel.sym == 0 && r.r_type != 0 {
                tally.emit += 1;
                restore.push((a, b));
                continue;
            }
            if fixed[i]
                && forms[i].is_none()
                && rel.sym != 0
                && !matches!(r.r_type, PLT32 | PC32 | GOT32 | GOT32X)
            {
                // A derived-addend mismatch, reported above.
                restore.push((a, b));
                continue;
            }
            let sv = tables.syms[&r.key];
            let s = if sv.kind == TargetKind::Weak0 { 0 } else { sv.s };
            let paddr = sec.addr.wrapping_add(r.r_offset);
            let what = || {
                format!(
                    "{label}: {} {}+{:#x} against `{}`{}",
                    name(r.r_type),
                    sec.name,
                    r.r_offset,
                    syms[rel.sym as usize].name,
                    forms[i].map_or(String::new(), |f| format!(" (lld {f})"))
                )
            };
            let pie_ifunc = output == OutputKind::Pie
                && sv.kind == TargetKind::Ifunc
                && alloc
                && matches!(r.r_type, R_32 | PC32 | GOTOFF);
            if let Err(e) = results[i] {
                if pie_ifunc {
                    tally.documented += 1;
                    *rep.documented.entry("X86-50 static-pie IFUNC (lld W6)").or_default() += 1;
                } else {
                    tally.mismatch += 1;
                    rep.mismatches.push(format!("{}: mortar error {e:?}", what()));
                }
                restore.push((a, b));
                continue;
            }
            if mine[a..b] == lld[a..b] {
                tally.ok += 1;
                continue;
            }
            // §15.1 rows, mortar's bytes checked against the spec's form.
            let doc: Option<(&'static str, bool)> = if pie_ifunc {
                Some(("X86-50 static-pie IFUNC (lld W6)", false))
            } else if r.r_type == GOT32X
                && output == OutputKind::Static
                && p >= 2
                && spec_g(&view, p, s, paddr).is_some_and(|g| mine[p - 2..p + 4] == g)
                && lld[p - 2..p] == view[p - 2..p]
            {
                Some(("X86-25 G1-G5 (lld keeps the GOT)", true))
            } else if forms[i] == Some("I5 lea (lld)")
                && tls
                    .and_then(|t| spec_i5(&view, p, t_value(sv, t)))
                    .is_some_and(|g| mine[p - 2..p + 4] == g)
            {
                Some(("X86-32 I5 add (lld lea)", true))
            } else {
                None
            };
            if let Some((d, keep)) = doc {
                tally.documented += 1;
                *rep.documented.entry(d).or_default() += 1;
                if !keep {
                    restore.push((a, b));
                }
            } else {
                tally.mismatch += 1;
                rep.mismatches.push(format!(
                    "{}: input {:02x?} mortar {:02x?} lld {:02x?}",
                    what(),
                    &view[a..b],
                    &mine[a..b],
                    &lld[a..b]
                ));
                restore.push((a, b));
            }
        }
        for (a, b) in restore {
            mine[a..b].copy_from_slice(&lld[a..b]);
        }
        if alloc && mine != lld {
            patches.push((sec.offset, mine));
        }
    }
    (patches, recon)
}

/// The object `obj` applied by mortar in a reference's layout from its
/// real input bytes: each allocated section found by a symbol it defines
/// (unique in the image) and confirmed by its bytes outside the records'
/// windows; every record applied and its unit window compared with the
/// reference's. With `recon`, also checks lld's rebuilt addends. Returns
/// the patches.
fn compare_object(
    label: &str,
    img: &Image,
    obj: &Image,
    output: OutputKind,
    rep: &mut Report,
    recon: Option<&BTreeMap<u64, Recon>>,
    strict: bool,
) -> Patches {
    let osyms = obj.symbols();
    let isyms = img.symbols();
    let tls = img.tls;
    let tls_base = tls.map_or(0, |t| t.start);
    let got = got_base(img, &isyms);
    let ifn = ifuncs(img, got);
    let words = img.got_words();
    let mut count: BTreeMap<&str, u32> = BTreeMap::new();
    for s in isyms.iter().filter(|s| !s.name.is_empty() && s.shndx != 0) {
        *count.entry(s.name.as_str()).or_default() += 1;
    }
    let unique: BTreeMap<&str, &Sym> = isyms
        .iter()
        .filter(|s| !s.name.is_empty() && s.shndx != 0 && count[s.name.as_str()] == 1)
        .map(|s| (s.name.as_str(), s))
        .collect();
    // Any definition, for an undefined reference (globals are unique).
    let mut defined: BTreeMap<&str, &Sym> = BTreeMap::new();
    for s in isyms.iter().filter(|s| !s.name.is_empty()) {
        let e = defined.entry(s.name.as_str()).or_insert(s);
        if e.shndx == 0 && s.shndx != 0 || (s.info >> 4 != 0 && e.info >> 4 == 0 && s.shndx != 0) {
            *e = s;
        }
    }
    let merged = |sym: u32| {
        osyms.get(sym as usize).is_some_and(|s| {
            obj.sections
                .get(usize::from(s.shndx))
                .is_some_and(|x| s.shndx < 0xff00 && x.flags & SHF_MERGE != 0)
        })
    };
    let in_tls = |i: u16| obj.sections.get(usize::from(i)).is_some_and(|x| x.flags & SHF_TLS != 0);
    // The output address (a TLS section: its offset in the block) of each
    // object section, by a symbol it defines.
    let mut sec_addr: BTreeMap<u16, u64> = BTreeMap::new();
    for s in &osyms {
        if s.name.is_empty() || s.shndx == 0 || s.shndx >= 0xff00 || s.info & 0xF == STT_SECTION {
            continue;
        }
        if let Some(o) = unique.get(s.name.as_str()) {
            sec_addr.entry(s.shndx).or_insert(o.value.wrapping_sub(s.value));
        }
    }
    let mut tables = Tables::default();
    for (i, s) in osyms.iter().enumerate() {
        let k = key(u32::try_from(i).unwrap());
        if s.shndx == SHN_ABS {
            tables.sym(k, s.value, TargetKind::Abs);
        } else if s.shndx == 0 {
            match defined.get(s.name.as_str()) {
                Some(o) if o.shndx != 0 => enter(&mut tables, k, img, o, &ifn, output),
                _ if s.info >> 4 == STB_WEAK => {
                    if s.info & 0xF == STT_TLS {
                        tables.tls_sym(k, 0, TargetKind::Weak0);
                    } else {
                        tables.sym(k, 0, TargetKind::Weak0);
                    }
                }
                _ => {
                    tables.sym(k, 0, TargetKind::Undefined);
                }
            }
        } else if let (false, Some(o)) = (s.info & 0xF == STT_SECTION, unique.get(s.name.as_str()))
        {
            enter(&mut tables, k, img, o, &ifn, output);
        } else if let Some(&a) = sec_addr.get(&s.shndx) {
            let v = a.wrapping_add(s.value);
            if in_tls(s.shndx) {
                tables.tls_sym(k, tls_base + v, TargetKind::Local);
            } else {
                tables.sym(k, v, TargetKind::Local);
            }
        } else {
            tables.sym(k, 0, TargetKind::Undefined);
        }
        tables.names.insert(k, s.name.as_bytes().to_vec());
    }
    let x = X86::M1;
    let mut patches = Vec::new();
    for rs in obj.sections.iter().filter(|s| s.kind == SHT_REL) {
        let sec = &obj.sections[rs.info as usize];
        if sec.flags & SHF_ALLOC == 0 || sec.name == ".eh_frame" || sec.kind == SHT_NOBITS {
            continue;
        }
        let rels: Vec<Rel> = obj.rels(rs);
        let osec = sec_addr.get(&(rs.info as u16)).and_then(|&addr| {
            img.sections
                .iter()
                .find(|o| {
                    o.flags & SHF_ALLOC != 0
                        && o.kind != SHT_NOBITS
                        && o.addr <= addr
                        && addr + sec.size as u64 <= o.addr + o.size as u64
                })
                .map(|o| (addr, o))
        });
        let view = obj.data(sec).to_vec();
        // Located, and the bytes outside every record's `max_reach` agree.
        let located = osec.map(|(addr, o)| {
            let off = o.offset + (addr - o.addr) as usize;
            let reference = img.bytes[off..off + view.len()].to_vec();
            let mut cover = vec![false; view.len()];
            for r in &rels {
                let p = r.offset as usize;
                for c in cover.iter_mut().take((p + 10).min(view.len())).skip(p.saturating_sub(3)) {
                    *c = true;
                }
            }
            let same = view.iter().zip(&reference).zip(&cover).all(|((a, b), c)| *c || a == b);
            (addr, off, reference, same)
        });
        let Some((addr, file_off, reference, true)) = located else {
            assert!(!strict, "{label}: {} not located in the reference", sec.name);
            rep.unlocated.push(format!("{label}: {}", sec.name));
            continue;
        };
        let relocs: Vec<Reloc> = rels
            .iter()
            .map(|r| Reloc { r_offset: r.offset, key: key(r.sym), r_type: r.r_type, addend: 0 })
            .collect();
        // GOT slots: the one the reference's field still addresses (an
        // unrelaxed load), else (second pass) a word of the value.
        for (pass, r) in [false, true].into_iter().flat_map(|p| relocs.iter().map(move |r| (p, r)))
        {
            let kind = match r.r_type {
                GOT32 | GOT32X => SlotKind::Addr,
                TLS_IE | TLS_GOTIE => SlotKind::TpOff { negated: false },
                _ => continue,
            };
            let wants = slot_wants(&tables, &ifn, r.key, kind, tls);
            if pass {
                if let Some(g) = wants.iter().find_map(|w| words.get(w).map(|c| c[0])) {
                    tables.slots.entry((r.key, kind)).or_insert(g);
                }
                continue;
            }
            let p = r.r_offset as usize;
            let a = field(&view, p, 4).map_or(0, |f| sign_extend(f, 4));
            let own = (p >= 2 && reference[p - 2..p] == view[p - 2..p])
                .then(|| field(&reference, p, 4))
                .flatten()
                .map(|f| {
                    let g = f.wrapping_sub(a as u64);
                    let g = if no_base(&view, p) || r.r_type == TLS_IE {
                        g
                    } else {
                        g.wrapping_add(got.unwrap_or(0))
                    };
                    g & 0xFFFF_FFFF
                })
                .filter(|g| word_at(img, *g).is_some_and(|w| wants.contains(&w)));
            if let Some(g) = own {
                tables.slots.entry((r.key, kind)).or_insert(g);
            }
        }
        let cx = FillCx { tables: &tables, output, got_base: got, tls };
        let mut mine = view.clone();
        let exec = sec.flags & SHF_EXECINSTR != 0;
        let mut done = Vec::new();
        for (i, r) in relocs.iter().enumerate() {
            let site = Site { bytes: &view, offset: r.r_offset };
            let h = x.classify(r.r_type, site).unwrap_or_else(|e| panic!("{label}: {e:?}"));
            let at = RelocAt {
                file: FileId(1),
                section: SectionId(rs.info),
                index: i as u32,
                r_offset: r.r_offset,
                key: r.key,
                r_type: r.r_type,
                addend: None,
                howto: &h,
                site,
                records: &relocs.as_slice(),
                proof: &[],
                leads: true,
            };
            let mut out =
                SectionOut::whole(&mut mine, addr, true, sec.name.as_bytes()).with_exec(exec);
            let res = x.apply(&cx, &at, &mut out);
            done.push((res, x.unit_window(output, &at, &tables).unwrap()));
        }
        let mut restore = Vec::new();
        for (i, (res, u)) in done.iter().enumerate() {
            let r = relocs[i];
            let sv = tables.syms[&r.key];
            let s = if sv.kind == TargetKind::Weak0 { 0 } else { sv.s };
            let p = r.r_offset as usize;
            let paddr = addr + r.r_offset;
            let w = width(r.r_type);
            if merged(rels[i].sym) {
                rep.merged += 1;
                restore.push((u.start as usize, u.end as usize));
                continue;
            }
            // The reconstruction of lld's records, against the object's.
            if let Some(rc) = recon.and_then(|m| m.get(&paddr)) {
                let a = field(&view, p, w).map_or(0, |f| sign_extend(f, w));
                if rc.r_type == r.r_type && rc.sa == sa(r.r_type, s, a) {
                    rep.recon_ok += 1;
                } else {
                    rep.recon_bad.push(format!(
                        "{label}: {} {}+{:#x}: object S+A {:#x}, rebuilt {} {:#x}",
                        name(r.r_type),
                        sec.name,
                        r.r_offset,
                        sa(r.r_type, s, a),
                        name(rc.r_type),
                        rc.sa
                    ));
                }
            }
            let tally = rep.by_type.entry(r.r_type).or_default();
            let (a, b) = (u.start as usize, u.end as usize);
            let what = || {
                format!(
                    "{label}: {} {}+{:#x} against `{}`",
                    name(r.r_type),
                    sec.name,
                    r.r_offset,
                    osyms.get(rels[i].sym as usize).map_or("", |s| s.name.as_str())
                )
            };
            let op = |v: &[u8], back: usize| p.checked_sub(back).and_then(|o| v.get(o)).copied();
            if let Err(e) = res {
                // X86-27: GNU ld relaxed the static-pie load (`lea`, G4,
                // G5) and made no slot; mortar keeps the GOT.
                if *e == FieldError::MissingSlot
                    && r.r_type == GOT32X
                    && output == OutputKind::Pie
                    && reference[p - 2..p] != view[p - 2..p]
                {
                    tally.documented += 1;
                    *rep.documented
                        .entry("X86-27 static-pie keeps the GOT (GNU relaxes)")
                        .or_default() += 1;
                } else {
                    tally.mismatch += 1;
                    rep.mismatches.push(format!("{}: mortar error {e:?}", what()));
                }
                restore.push((a, b));
                continue;
            }
            if mine[a..b] == reference[a..b] {
                tally.ok += 1;
                continue;
            }
            let doc: Option<&'static str> = match r.r_type {
                GOT32X
                    if output == OutputKind::Static
                        && reference[p - 2..p] == view[p - 2..p]
                        && spec_g(&view, p, s, paddr).is_some_and(|g| mine[p - 2..p + 4] == g) =>
                {
                    Some("X86-25 G1-G5 (lld keeps the GOT)")
                }
                GOT32X if output == OutputKind::Pie && mine[p - 2..p] == view[p - 2..p] => {
                    Some("X86-27 static-pie keeps the GOT (GNU relaxes)")
                }
                TLS_GOTIE
                    if op(&reference, 2) == Some(0x8d)
                        && tls
                            .and_then(|t| spec_i5(&view, p, t_value(sv, t)))
                            .is_some_and(|g| mine[p - 2..p + 4] == g) =>
                {
                    Some("X86-32 I5 add (lld lea)")
                }
                _ => None,
            };
            if let Some(d) = doc {
                tally.documented += 1;
                *rep.documented.entry(d).or_default() += 1;
            } else {
                tally.mismatch += 1;
                rep.mismatches.push(format!(
                    "{}: input {:02x?} mortar {:02x?} reference {:02x?}",
                    what(),
                    &view[a..b],
                    &mine[a..b],
                    &reference[a..b]
                ));
                restore.push((a, b));
            }
        }
        for (a, b) in restore {
            mine[a..b].copy_from_slice(&reference[a..b]);
        }
        patches.push((file_off, mine));
    }
    patches
}

// ---------------------------------------------------------------- running

fn qemu(t: &Tools, exe: &Path) -> (bool, String) {
    let o = run(
        "sh",
        [
            "-c".as_ref(),
            "ulimit -c 0; exec \"$0\" \"$1\"".as_ref(),
            t.qemu.as_os_str(),
            exe.as_os_str(),
        ],
    );
    (o.success(), o.stdout_text())
}

fn write_exe(dir: &Scratch, name: &str, img: &Image, patches: &[(usize, Vec<u8>)]) -> PathBuf {
    let mut bytes = img.bytes.clone();
    for (off, data) in patches {
        bytes[*off..*off + data.len()].copy_from_slice(data);
    }
    let exe = dir.join(name);
    std::fs::write(&exe, &bytes).unwrap();
    must("chmod", &run("chmod", ["+x".as_ref(), exe.as_os_str()]));
    exe
}

fn print(rep: &Report, what: &str) {
    eprintln!("== {what}");
    eprintln!("forms the reference wrote: {:?}", rep.forms);
    eprintln!("documented: {:?}", rep.documented);
    eprintln!(
        "{:<4} {:<22} {:>7} {:>6} {:>6} {:>6} {:>6}",
        "type", "name", "ok", "doc", "ifunc", "emit", "mism"
    );
    let mut total = Tally::default();
    for (ty, tl) in &rep.by_type {
        eprintln!(
            "{ty:<4} {:<22} {:>7} {:>6} {:>6} {:>6} {:>6}",
            name(*ty),
            tl.ok,
            tl.documented,
            tl.ifunc,
            tl.emit,
            tl.mismatch
        );
        total.ok += tl.ok;
        total.documented += tl.documented;
        total.ifunc += tl.ifunc;
        total.emit += tl.emit;
        total.mismatch += tl.mismatch;
    }
    eprintln!(
        "{:<4} {:<22} {:>7} {:>6} {:>6} {:>6} {:>6}",
        "all", "", total.ok, total.documented, total.ifunc, total.emit, total.mismatch
    );
    for (ty, h) in &rep.addends {
        let mut v: Vec<_> = h.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1));
        let top: Vec<String> = v.iter().take(6).map(|(a, n)| format!("{a}:{n}")).collect();
        eprintln!("  derived A {:<20} {}", name(*ty), top.join(" "));
    }
    if rep.recon_ok > 0 || !rep.recon_bad.is_empty() {
        eprintln!(
            "reconstruction checked against the objects: {} agree, {} differ",
            rep.recon_ok,
            rep.recon_bad.len()
        );
    }
    if rep.ifuncs > 0 {
        eprintln!("IFUNCs located (IPLT entry, resolver): {}", rep.ifuncs);
    }
    if rep.merged > 0 {
        eprintln!("records into SHF_MERGE sections (not compared here): {}", rep.merged);
    }
    if !rep.unlocated.is_empty() {
        eprintln!("object sections not located (skipped): {:?}", rep.unlocated);
    }
    for m in rep.mismatches.iter().take(60) {
        eprintln!("MISMATCH {m}");
    }
    for m in rep.recon_bad.iter().take(30) {
        eprintln!("RECON {m}");
    }
}

#[test]
fn s16_apply_in_reference_layouts() {
    let Some(t) = tools() else {
        assert!(
            !mortar_testutil::tools_required(),
            "MORTAR_REQUIRE_TOOLS: a reference tool is missing"
        );
        eprintln!("skipped: i686-linux-gnu-gcc/g++, ld.lld or qemu-i386 missing");
        return;
    };
    let start = Instant::now();
    // `MORTAR_KEEP_SCRATCH`: the directory is kept (and named) for
    // inspection. A failing run also keeps it (the panic skips the drop).
    let dir = std::mem::ManuallyDrop::new(Scratch::new("x86-diff"));
    let keep = std::env::var_os("MORTAR_KEEP_SCRATCH").is_some();
    if keep {
        eprintln!("scratch {}", dir.path().display());
    }
    let mut lld_rep = Report::default();
    let mut site_rep = Report::default();
    let mut obj_rep = Report::default();
    let mut failures = Vec::new();
    for b in BUILDS {
        if !b.objects.iter().all(|o| has(&t, o.0)) || !has(&t, b.driver) {
            eprintln!("{:<16} skipped (no compiler)", b.name);
            continue;
        }
        let objs = compile(&t, &dir, b);
        let output = if b.pie { OutputKind::Pie } else { OutputKind::Static };
        let checks = if b.sites { sites_file(b.objects[0].1).unwrap().1 } else { 0 };
        let mut line = format!("{:<16}", b.name);
        if b.sites {
            let _ = write!(line, " [{checks} checks]");
        }
        let (_, bfd_out) = if b.bfd {
            let exe = dir.join(format!("{}.bfd", b.name));
            must(&format!("GNU ld {}", b.name), &link(&t, b, &objs, &exe, false));
            let r = qemu(&t, &exe);
            let _ = write!(line, " bfd {:<5}", r.0);
            if !r.0 {
                failures.push(format!("{} GNU ld: {}", b.name, r.1));
            }
            r
        } else {
            (false, String::new())
        };
        let check =
            |who: &str, ok: bool, out: &str, failures: &mut Vec<String>, reference: &str| {
                let good = if b.sites {
                    ok && out.contains(&format!("{checks} checks, 0 failures"))
                } else {
                    ok && out == reference
                };
                if !good {
                    failures.push(format!("{} {who}: {}", b.name, &out[..out.len().min(2000)]));
                }
                good
            };
        if b.sites && b.bfd {
            let bfd = Image::read(&dir.join(format!("{}.bfd", b.name)));
            let p = compare_object(
                &format!("bfd {}", b.name),
                &bfd,
                &Image::read(&objs[0]),
                output,
                &mut site_rep,
                None,
                true,
            );
            let exe = write_exe(&dir, &format!("{}.bfd-mortar-obj", b.name), &bfd, &p);
            let (ok, out) = qemu(&t, &exe);
            let good =
                check("mortar in GNU ld's layout (object)", ok, &out, &mut failures, &bfd_out);
            let _ = write!(line, " | mortar-obj-in-bfd {good:<5}");
        }
        if b.lld {
            let exe = dir.join(format!("{}.lld", b.name));
            must(&format!("lld {}", b.name), &link(&t, b, &objs, &exe, true));
            let (lld_ok, lld_out) = qemu(&t, &exe);
            let lld_good = if b.sites {
                lld_ok && lld_out.contains(&format!("{checks} checks, 0 failures"))
            } else {
                lld_ok && (!b.bfd || lld_out == bfd_out)
            };
            let _ = write!(line, " | lld {lld_good:<5}");
            if !lld_good {
                let fails = lld_out.lines().filter(|l| l.starts_with("FAIL")).count();
                let _ = write!(line, " ({fails} FAIL lines)");
            }
            let img = Image::read(&exe);
            let (p, recon) = compare(b.name, &img, output, &mut lld_rep);
            let mexe = write_exe(&dir, &format!("{}.lld-mortar", b.name), &img, &p);
            let (ok, out) = qemu(&t, &mexe);
            let reference = if b.bfd { bfd_out.clone() } else { lld_out.clone() };
            let good = check(
                "mortar in lld's layout (rebuilt input)",
                ok,
                &out,
                &mut failures,
                &reference,
            );
            let _ = write!(line, " mortar-in-lld {good:<5}");
            if b.sites {
                let obj = Image::read(&objs[0]);
                let p = compare_object(
                    &format!("lld {}", b.name),
                    &img,
                    &obj,
                    output,
                    &mut site_rep,
                    Some(&recon),
                    true,
                );
                let mexe = write_exe(&dir, &format!("{}.lld-mortar-obj", b.name), &img, &p);
                let (ok, out) = qemu(&t, &mexe);
                let good =
                    check("mortar in lld's layout (object)", ok, &out, &mut failures, &reference);
                let _ = write!(line, " mortar-obj-in-lld {good:<5}");
            } else {
                for o in &objs {
                    compare_object(
                        &format!("lld {}", b.name),
                        &img,
                        &Image::read(o),
                        output,
                        &mut obj_rep,
                        Some(&recon),
                        false,
                    );
                }
            }
        }
        eprintln!("{line}");
    }
    print(&lld_rep, "every record of lld's outputs, input rebuilt");
    print(&site_rep, "the sites objects in GNU ld's and lld's layouts (true input)");
    print(&obj_rep, "the compiled C/C++/Fortran objects in lld's layout (true input)");
    for f in &failures {
        eprintln!("RUN FAILED {f}");
    }
    eprintln!("wall time {:.1?}", start.elapsed());
    if !keep {
        drop(std::mem::ManuallyDrop::into_inner(dir));
    }
    for r in [&lld_rep, &site_rep, &obj_rep] {
        assert!(r.mismatches.is_empty(), "{} mismatches", r.mismatches.len());
        assert!(r.recon_bad.is_empty(), "{} reconstruction differences", r.recon_bad.len());
    }
    let total: u32 = lld_rep.by_type.values().map(|t| t.ok).sum();
    assert!(total > 50_000, "{total} records compared");
    assert!(failures.is_empty(), "runs failed:\n{}", failures.join("\n"));
}

//! x64.md §15 steps 1–4 for stage A: `apply` in lld's layout.
//!
//! Each program is linked by lld 22 (`-static`, and `-static-pie` for the
//! relaxation sites) with `--emit-relocs`. lld keeps the input types in
//! the records it emits, but its output bytes are relaxed; the input bytes
//! of each relaxed site are rebuilt from lld's form (`invert`, the forms
//! of §8 and §8.8), then every record of the section is applied again by
//! mortar, with `S`, `P`, `G` and the TLS template read from lld's image,
//! and the bytes of each record's unit window are compared with lld's:
//!
//! - **ok**: the same bytes;
//! - **documented**: a difference §15 lists (D1, the `GOTTPOFF` `add`:
//!   mortar `add $T`, lld `lea`; W4, the 22-byte GD, which lld corrupts);
//! - **ifunc**: against an `STT_GNU_IFUNC`, whose IPLT this harness does
//!   not locate (the IPLT bytes are tested in `table.rs`);
//! - **emit**: a record that does not describe its field (`.eh_frame`,
//!   which lld rewrites; a symbol lld dropped);
//! - **mismatch**: anything else. There must be none.
//!
//! Then lld's image with mortar's bytes written in is run under
//! `qemu-x86_64`: its output must equal GNU ld's output of the same
//! objects (the relaxation sites print "N checks, 0 failures"). GNU ld's
//! own `--emit-relocs` output is classified as §15 step 3 says.

#![allow(clippy::many_single_char_names, clippy::too_many_lines, clippy::struct_excessive_bools)]

mod elf;
mod sites;
mod support;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use elf::{
    Image, SHF_ALLOC, SHF_EXECINSTR, SHN_ABS, SHT_NOBITS, SHT_RELA, STB_WEAK, STT_GNU_IFUNC,
    STT_TLS, u64_at,
};
use mortar_arch::{
    Arch, DemandClass, FieldError, FillCx, Howto, OutputKind, Reloc, RelocAt, SectionOut, Site,
    SlotKind, TargetKind,
};
use mortar_core::{FileId, SectionId};
use mortar_testutil::{Scratch, run, tool};
use mortar_x64::{X64, table};
use support::{Tables, key};

struct Tools {
    gcc: PathBuf,
    gxx: PathBuf,
    gfortran: PathBuf,
    clang: PathBuf,
    lld_dir: PathBuf,
    qemu: PathBuf,
}

fn tools() -> Option<Tools> {
    let lld = tool("ld.lld")?;
    // `-fuse-ld=lld -B<dir>/` needs a directory holding `ld.lld`.
    let lld_dir = [PathBuf::from("/usr/lib/llvm-22/bin"), PathBuf::from("/usr/lib/llvm-19/bin")]
        .into_iter()
        .find(|d| d.join("ld.lld").is_file())
        .unwrap_or_else(|| lld.parent().unwrap().to_path_buf());
    Some(Tools {
        gcc: tool("x86_64-linux-gnu-gcc")?,
        gxx: tool("x86_64-linux-gnu-g++")?,
        gfortran: tool("x86_64-linux-gnu-gfortran")?,
        clang: tool("clang")?,
        lld_dir,
        qemu: tool("qemu-x86_64")?,
    })
}

fn must(what: &str, o: &mortar_testutil::Outcome) {
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
    /// `(compiler, file, flags)` per object; the first object of a sites
    /// build is the sites.
    objects: &'static [(Cc, &'static str, &'static [&'static str])],
    driver: Cc,
    link: &'static [&'static str],
    pie: bool,
    /// lld links it (not the 22-byte LD form).
    lld: bool,
    /// GNU ld links it: not the sites with a `DTPOFF64` in code (GNU ld
    /// 2.44 asserts), nor static-pie with a `PC32` follower ("read-only
    /// segment has dynamic relocations").
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

const PIC: &[&str] = &["-O2", "-fPIC"];
const SITES: &[(Cc, &str, &[&str])] =
    &[(Cc::Gcc, "relax.s", &[]), (Cc::Gcc, "relaxmain.c", &["-O1"])];
const BUILDS: &[Build] = &[
    Build { sites: true, bfd: false, ..b("sites", SITES, Cc::Gcc, &[], false) },
    Build {
        sites: true,
        bfd: false,
        ..b(
            "sites-pie",
            &[(Cc::Gcc, "relax.s", &[]), (Cc::Gcc, "relaxmain.c", &["-O1", "-fPIE"])],
            Cc::Gcc,
            &[],
            true,
        )
    },
    Build {
        sites: true,
        lld: false,
        ..b(
            "sites-bfd",
            &[(Cc::Gcc, "relax22.s", &[]), (Cc::Gcc, "relaxmain.c", &["-O1"])],
            Cc::Gcc,
            &[],
            false,
        )
    },
    b("gd-pic", &[(Cc::Gcc, "gd.c", PIC)], Cc::Gcc, &[], false),
    b("gd-large", &[(Cc::Gcc, "gd.c", &["-O2", "-fPIC", "-mcmodel=large"])], Cc::Gcc, &[], false),
    b(
        "gd-gnu2",
        &[(Cc::Gcc, "gd.c", &["-O2", "-fPIC", "-mtls-dialect=gnu2"])],
        Cc::Gcc,
        &[],
        false,
    ),
    b(
        "gd-ld-noplt",
        &[(
            Cc::Gcc,
            "gd.c",
            &[
                "-O1",
                "-fPIC",
                "-fno-plt",
                "-Wa,-mrelax-relocations=no",
                "-ftls-model=local-dynamic",
            ],
        )],
        Cc::Gcc,
        &[],
        false,
    ),
    b(
        "gd-clang-ld-gnu2",
        &[(
            Cc::Clang,
            "gd.c",
            &["-O2", "-fPIC", "-ftls-model=local-dynamic", "-mtls-dialect=gnu2"],
        )],
        Cc::Gcc,
        &[],
        false,
    ),
    b(
        "a-pic",
        &[(Cc::Gcc, "a.c", PIC), (Cc::Clang, "d.c", &["-O2", "-fPIC", "-mcmodel=large"])],
        Cc::Gcc,
        &[],
        false,
    ),
    // WP-084: the medium and large models' sections and relocations
    // (`GOTOFF64`, `GOTPC64`, `GOTPC32`, `PLTOFF64`, `GOT64`, `PC64`).
    b(
        "large-medium",
        &[(Cc::Gcc, "large.c", &["-O2", "-mcmodel=medium", "-fno-pic"])],
        Cc::Gcc,
        &[],
        false,
    ),
    b(
        "large-gcc",
        &[(Cc::Gcc, "large.c", &["-O2", "-mcmodel=large", "-fPIC"])],
        Cc::Gcc,
        &[],
        false,
    ),
    b(
        "large-clang",
        &[(Cc::Clang, "large.c", &["-O2", "-mcmodel=large", "-fno-pic"])],
        Cc::Gcc,
        &[],
        false,
    ),
    b(
        "large-medium-pie",
        &[(Cc::Gcc, "large.c", &["-O2", "-mcmodel=medium", "-fPIE"])],
        Cc::Gcc,
        &[],
        true,
    ),
    b("p", &[(Cc::Gcc, "p.c", &["-O2", "-g"])], Cc::Gcc, &["-pthread"], false),
    b("q", &[(Cc::Gxx, "q.cc", &["-O2", "-g"])], Cc::Gxx, &["-pthread"], false),
    b("f", &[(Cc::Gfortran, "f.f90", &["-O2"])], Cc::Gfortran, &[], false),
    b(
        "tlslayout",
        &[(Cc::Clang, "tlslayout.c", &["-O2", "-DALIGN=64", "-DTDATA"])],
        Cc::Gcc,
        &[],
        false,
    ),
];

fn source(name: &str) -> String {
    match name {
        "relax.s" => return sites::relax_s(false).0,
        "relax22.s" => return sites::relax_s(true).0,
        _ => {}
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
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
                &t.gfortran
            }
            Cc::Clang => {
                args.push("--target=x86_64-linux-gnu".into());
                &t.clang
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

fn link(t: &Tools, b: &Build, objs: &[PathBuf], out: &Path, lld: bool) -> mortar_testutil::Outcome {
    let driver = match b.driver {
        Cc::Gxx => &t.gxx,
        Cc::Gfortran => &t.gfortran,
        _ => &t.gcc,
    };
    let mut args: Vec<String> =
        if b.pie { vec!["-static-pie".into()] } else { vec!["-static".into(), "-no-pie".into()] };
    if lld {
        args.extend(["-fuse-ld=lld".into(), format!("-B{}/", t.lld_dir.display())]);
    } else {
        args.push("-fuse-ld=bfd".into());
    }
    args.extend(["-Wl,--emit-relocs", "-Wl,--build-id=none"].map(String::from));
    args.extend(b.link.iter().map(|s| (*s).to_string()));
    args.extend(objs.iter().map(|o| o.display().to_string()));
    args.extend(["-o".into(), out.display().to_string()]);
    run(driver, &args)
}

// ---------------------------------------------------------------- rebuilding the input

const fn rip(m: u8) -> bool {
    m & 0xC7 == 5
}

/// REX of an input instruction whose register was in ModRM.rm in lld's
/// output (`48` → `48`, `49` → `4c`): the inverse of `REX'`.
fn rex_in(rex: u8) -> Option<u8> {
    match rex {
        0x48 => Some(0x48),
        0x49 => Some(0x4c),
        _ => None,
    }
}

/// Rebuilds in `v` (lld's bytes) the input bytes of a site lld relaxed,
/// for the record of `r_type` at `p` whose next record is `next`. Returns
/// the form lld wrote, or `None` when the bytes are not relaxed.
fn invert(v: &mut [u8], p: usize, r_type: u32, next: Option<(u32, usize)>) -> Option<&'static str> {
    let get = |v: &[u8], a: usize, n: usize| -> Option<Vec<u8>> {
        v.get(a.checked_sub(0)?..a.checked_add(n)?).map(<[u8]>::to_vec)
    };
    match r_type {
        42 => {
            let b = get(v, p.checked_sub(3)?, 3)?;
            let (rex, op, m) = (b[0], b[1], b[2]);
            let (form, bytes) = if matches!(rex, 0x48 | 0x4c) && op == 0x8d && rip(m) {
                ("G1 lea", [rex, 0x8b, m])
            } else if op == 0x81 && m & 0xC0 == 0xC0 {
                let n = (m >> 3) & 7;
                ("G5 81", [rex_in(rex)?, (n << 3) | 3, ((m & 7) << 3) | 5])
            } else if op == 0xf7 && m & 0xF8 == 0xC0 {
                ("G6 f7", [rex_in(rex)?, 0x85, ((m & 7) << 3) | 5])
            } else if op == 0xc7 && m & 0xF8 == 0xC0 {
                ("mov $imm", [rex_in(rex)?, 0x8b, ((m & 7) << 3) | 5])
            } else {
                return None;
            };
            v[p - 3..p].copy_from_slice(&bytes);
            Some(form)
        }
        41 => {
            let b = get(v, p.checked_sub(2)?, 6)?;
            let (form, bytes): (&str, [u8; 2]) = match b[..2] {
                [0x8d, m] if rip(m) => ("G2 lea", [0x8b, m]),
                [0x67, 0xe8] => ("G3 67 e8", [0xff, 0x15]),
                [0xe9, _] if b[5] == 0x90 => ("G4 e9 90", [0xff, 0x25]),
                _ => return None,
            };
            v[p - 2..p].copy_from_slice(&bytes);
            v[p..p + 4].fill(0);
            Some(form)
        }
        22 => {
            let b = get(v, p.checked_sub(3)?, 3)?;
            let (rex, op, m) = (b[0], b[1], b[2]);
            let (form, bytes) = if op == 0xc7 && m & 0xF8 == 0xC0 {
                ("I1 c7", [rex_in(rex)?, 0x8b, ((m & 7) << 3) | 5])
            } else if op == 0x8d && m & 0xC0 == 0x80 && matches!(rex, 0x48 | 0x4d) {
                let r = if rex == 0x4d { 0x4c } else { 0x48 };
                ("I2 lea", [r, 0x03, (m & 0x38) | 5])
            } else if op == 0x81 && m & 0xF8 == 0xC0 {
                ("I2 81", [rex_in(rex)?, 0x03, ((m & 7) << 3) | 5])
            } else {
                return None;
            };
            v[p - 3..p].copy_from_slice(&bytes);
            Some(form)
        }
        19 => {
            let (nt, np) = next?;
            let s = p.checked_sub(4)?;
            if get(v, s, 5)? != [0x64, 0x48, 0x8b, 0x04, 0x25] {
                return None;
            }
            let mut gd = vec![0x66, 0x48, 0x8d, 0x3d, 0, 0, 0, 0];
            match (nt, np.checked_sub(p)?) {
                (4 | 2, 8) => gd.extend([0x66, 0x66, 0x48, 0xe8, 0, 0, 0, 0]),
                (41 | 9, 8) => gd.extend([0x66, 0x48, 0xff, 0x15, 0, 0, 0, 0]),
                (31, 6) => {
                    // W4: lld wrote 16 bytes from `P − 4` and left the
                    // `add` and the `call`; the byte at `P − 4` is lost.
                    let tail = get(v, p + 12, 7)?;
                    let mut large = vec![0x48, 0x8d, 0x3d, 0, 0, 0, 0, 0x48, 0xb8];
                    large.extend([0; 8]);
                    large.extend([tail[2], tail[3], tail[4], tail[5], tail[6]]);
                    v[p - 3..p + 19].copy_from_slice(&large);
                    return Some("GD22 (W4)");
                }
                _ => return None,
            }
            v[s..s + 16].copy_from_slice(&gd);
            Some("GD16")
        }
        20 => {
            let (nt, np) = next?;
            let s = p.checked_sub(3)?;
            let (form, input): (&str, Vec<u8>) = match (nt, np.checked_sub(p)?) {
                (4 | 2, 5) if get(v, s, 4)? == [0x66, 0x66, 0x66, 0x64] => {
                    ("LD12", vec![0x48, 0x8d, 0x3d, 0, 0, 0, 0, 0xe8, 0, 0, 0, 0])
                }
                (41 | 9, 6) if get(v, s, 5)? == [0x66, 0x66, 0x66, 0x66, 0x64] => {
                    ("LD13", vec![0x48, 0x8d, 0x3d, 0, 0, 0, 0, 0xff, 0x15, 0, 0, 0, 0])
                }
                _ => return None,
            };
            v[s..s + input.len()].copy_from_slice(&input);
            Some(form)
        }
        34 => {
            let b = get(v, p.checked_sub(3)?, 3)?;
            if b[1] != 0xc7 || b[2] & 0xF8 != 0xC0 {
                return None;
            }
            v[p - 3..p].copy_from_slice(&[rex_in(b[0])?, 0x8d, ((b[2] & 7) << 3) | 5]);
            Some("T1 c7")
        }
        35 => {
            if get(v, p, 2)? != [0x66, 0x90] {
                return None;
            }
            v[p..p + 2].copy_from_slice(&[0xff, 0x10]);
            Some("T2 66 90")
        }
        _ => None,
    }
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
    /// lld's relaxed forms seen, by name.
    forms: BTreeMap<&'static str, u32>,
    mismatches: Vec<String>,
}

struct Rec {
    r: Reloc,
    sym: usize,
}

/// Mortar's bytes of every section of `img` that has records, after
/// comparing each record with lld's, as `(file offset, bytes)`.
fn compare(
    label: &str,
    img: &Image,
    output: OutputKind,
    rep: &mut Report,
) -> Vec<(usize, Vec<u8>)> {
    let syms = img.symbols();
    let got = img.got_words();
    let tls = img.tls;
    let got_base = syms
        .iter()
        .find(|s| s.name == "_GLOBAL_OFFSET_TABLE_")
        .map(|s| s.value)
        .or_else(|| img.sections.iter().find(|s| s.name == ".got.plt").map(|s| s.addr));
    let mut tables = Tables::default();
    for (i, s) in syms.iter().enumerate() {
        let k = key(u32::try_from(i).unwrap());
        let ty = s.info & 0xF;
        let kind = if s.shndx == 0 {
            // lld writes a hidden undefined weak as an undefined local.
            if matches!(s.info >> 4, STB_WEAK | 0) {
                TargetKind::Weak0
            } else {
                TargetKind::Undefined
            }
        } else if s.shndx == SHN_ABS {
            TargetKind::Abs
        } else if ty == STT_GNU_IFUNC {
            TargetKind::Ifunc
        } else {
            TargetKind::Local
        };
        if ty == STT_TLS {
            let base = tls.map_or(0, |t| t.start);
            let v = if kind == TargetKind::Weak0 { 0 } else { base + s.value };
            tables.tls_sym(k, v, kind);
        } else {
            tables.sym(k, s.value, kind);
        }
        tables.syms.get_mut(&k).unwrap().size = s.size;
        tables.names.insert(k, s.name.as_bytes().to_vec());
    }
    let x = X64::M1;
    let cx_tls = tls;
    let mut patches = Vec::new();
    for rs in img.sections.iter().filter(|s| s.kind == SHT_RELA && s.info != 0) {
        let sec = &img.sections[rs.info as usize];
        if sec.kind == SHT_NOBITS || sec.name == ".eh_frame" {
            let n = img.data(rs).len() as u32 / 24;
            for e in img.data(rs).chunks_exact(24) {
                rep.by_type.entry((u64_at(e, 8) & 0xFFFF_FFFF) as u32).or_default().emit += 1;
            }
            let _ = n;
            continue;
        }
        let lld = img.data(sec).to_vec();
        let base = if sec.flags & SHF_ALLOC != 0 { sec.addr } else { 0 };
        let mut recs: Vec<Rec> = Vec::new();
        for e in img.data(rs).chunks_exact(24) {
            let info = u64_at(e, 8);
            let r_offset = u64_at(e, 0).wrapping_sub(base);
            let sym = (info >> 32) as usize;
            recs.push(Rec {
                r: Reloc {
                    r_offset,
                    key: key(sym as u32),
                    r_type: (info & 0xFFFF_FFFF) as u32,
                    addend: u64_at(e, 16) as i64,
                },
                sym,
            });
        }
        // The input bytes: lld's, with each relaxed site rebuilt.
        let mut view = lld.clone();
        let mut forms: Vec<Option<&'static str>> = vec![None; recs.len()];
        for i in 0..recs.len() {
            let r = recs[i].r;
            let next = recs.get(i + 1).map(|n| (n.r.r_type, n.r.r_offset as usize));
            let Ok(p) = usize::try_from(r.r_offset) else { continue };
            if p >= view.len() {
                continue;
            }
            forms[i] = invert(&mut view, p, r.r_type, next);
            if let Some(f) = forms[i] {
                *rep.forms.entry(f).or_default() += 1;
            }
        }
        // GOT slots: the slot lld's bytes still load from, when they do
        // (its word is the slot's value in a static output; static-pie
        // leaves it 0 for a `RELATIVE`), else the `.got` word holding the
        // slot's value.
        for (i, rec) in recs.iter().enumerate() {
            let Some(h) = table::howto(rec.r.r_type) else { continue };
            let mortar_arch::Base::G(kind) = h.value.base else { continue };
            let sv = tables.syms[&rec.r.key];
            let want = match kind {
                SlotKind::Addr => Some(if sv.kind == TargetKind::Weak0 { 0 } else { sv.s }),
                SlotKind::TpOff { negated: false } => match (sv.kind, tls) {
                    (TargetKind::Weak0, _) => Some(0),
                    (_, Some(t)) => Some(x.tls().tp_rel(sv.s, t) as u64),
                    _ => None,
                },
                _ => None,
            };
            let Some(want) = want else { continue };
            let p = rec.r.r_offset as usize;
            let used = (forms[i].is_none() && p + 4 <= lld.len()).then(|| {
                let f = i32::from_le_bytes(lld[p..p + 4].try_into().unwrap());
                (sec.addr + rec.r.r_offset)
                    .wrapping_add(i64::from(f) as u64)
                    .wrapping_sub(rec.r.addend as u64)
            });
            let used = used.filter(|u| match got_word(img, *u) {
                Some(w) => w == want || output == OutputKind::Pie,
                None => false,
            });
            let Some(slot) = used.or_else(|| got.get(&want).map(|c| c[0])) else { continue };
            tables.slots.entry((rec.r.key, kind)).or_insert(slot);
        }
        let relocs: Vec<Reloc> = recs.iter().map(|r| r.r).collect();
        let cx = FillCx { tables: &tables, output, got_base, tls: cx_tls };
        let mut mine = view.clone();
        let alloc = sec.flags & SHF_ALLOC != 0;
        let exec = sec.flags & SHF_EXECINSTR != 0;
        let mut results: Vec<Result<(), FieldError>> = Vec::with_capacity(recs.len());
        let mut howtos = Vec::with_capacity(recs.len());
        for (i, rec) in recs.iter().enumerate() {
            let site = Site { bytes: &view, offset: rec.r.r_offset };
            let h = match x.classify(rec.r.r_type, site) {
                Ok(h) => h,
                Err(r) => {
                    results.push(Err(FieldError::Refused(r)));
                    howtos.push(None);
                    continue;
                }
            };
            let at = RelocAt {
                file: FileId(1),
                section: SectionId(rs.info),
                index: i as u32,
                r_offset: rec.r.r_offset,
                key: rec.r.key,
                r_type: rec.r.r_type,
                addend: Some(rec.r.addend),
                howto: &h,
                site,
                records: &relocs.as_slice(),
                proof: &[],
                leads: true,
            };
            let name = sec.name.as_bytes();
            let mut out = SectionOut::whole(&mut mine, sec.addr, alloc, name).with_exec(exec);
            results.push(x.apply(&cx, &at, &mut out));
            howtos.push(Some(h));
        }
        // lld's W4: the `64` it writes at `P − 4` of a GD22 lead.
        let w4: Vec<usize> = recs
            .iter()
            .zip(&forms)
            .filter(|(_, f)| **f == Some("GD22 (W4)"))
            .map(|(r, _)| r.r.r_offset as usize - 4)
            .collect();
        // Records not compared keep lld's bytes in the patched image.
        let mut restore: Vec<(usize, usize)> = Vec::new();
        let field = |i: usize| {
            let o = recs[i].r.r_offset as usize;
            let w = howtos[i].map_or(4, |h: Howto| usize::from(h.field.width()));
            (o, (o + w).min(lld.len()))
        };
        for (i, rec) in recs.iter().enumerate() {
            let tally = rep.by_type.entry(rec.r.r_type).or_default();
            let s = &syms.get(rec.sym);
            let where_ = || {
                format!(
                    "{label}: {} {}+{:#x} against `{}`{}",
                    mortar_elf::reloc::x86_64::name(rec.r.r_type).unwrap_or("?"),
                    sec.name,
                    rec.r.r_offset,
                    s.map_or("", |s| s.name.as_str()),
                    forms[i].map_or(String::new(), |f| format!(" (lld {f})"))
                )
            };
            if rec.sym == 0 && rec.r.r_type != 0 {
                tally.emit += 1;
                restore.push(field(i));
                continue;
            }
            if s.is_some_and(|s| s.info & 0xF == STT_GNU_IFUNC) {
                tally.ifunc += 1;
                restore.push(field(i));
                continue;
            }
            let Some(h) = howtos[i] else {
                tally.mismatch += 1;
                rep.mismatches.push(format!("{}: refused {:?}", where_(), results[i]));
                continue;
            };
            // static-pie: an absolute word gets a `RELATIVE`, whose field
            // lld leaves as its addend's value or 0.
            if output == OutputKind::Pie && h.class == DemandClass::AbsWord && alloc {
                tally.emit += 1;
                restore.push(field(i));
                continue;
            }
            if let Err(e) = results[i] {
                tally.mismatch += 1;
                rep.mismatches.push(format!("{}: mortar error {e:?}", where_()));
                restore.push(field(i));
                continue;
            }
            let at = RelocAt {
                file: FileId(1),
                section: SectionId(rs.info),
                index: i as u32,
                r_offset: rec.r.r_offset,
                key: rec.r.key,
                r_type: rec.r.r_type,
                addend: Some(rec.r.addend),
                howto: &h,
                site: Site { bytes: &view, offset: rec.r.r_offset },
                records: &relocs.as_slice(),
                proof: &[],
                leads: true,
            };
            let u = x.unit_window(output, &at, &tables).unwrap();
            let (a, b) = (u.start as usize, (u.end as usize).min(lld.len()));
            if mine[a..b] == lld[a..b] {
                tally.ok += 1;
            } else if matches!(forms[i], Some("I2 lea" | "GD22 (W4)"))
                || only_w4(&mine[a..b], &lld[a..b], a, &w4)
            {
                tally.documented += 1;
            } else {
                tally.mismatch += 1;
                rep.mismatches.push(format!(
                    "{}: mortar {:02x?} lld {:02x?}",
                    where_(),
                    &mine[a..b],
                    &lld[a..b]
                ));
            }
        }
        for (a, b) in restore {
            mine[a..b].copy_from_slice(&lld[a..b]);
        }
        if mine != lld {
            patches.push((sec.offset, mine));
        }
    }
    patches
}

/// The sites object applied by mortar in a reference's layout, from its
/// real input bytes: each allocated section of `obj` is found in `img` by
/// a symbol it defines, every record of it applied, and each record's
/// unit window compared with the reference's bytes. Returns the patches.
fn compare_object(
    label: &str,
    img: &Image,
    obj: &Image,
    output: OutputKind,
    rep: &mut Report,
) -> Vec<(usize, Vec<u8>)> {
    let osyms = obj.symbols();
    let isyms = img.symbols();
    let tls = img.tls;
    let got = img.got_words();
    let by_name: BTreeMap<&str, &elf::Sym> =
        isyms.iter().filter(|s| !s.name.is_empty()).map(|s| (s.name.as_str(), s)).collect();
    // Output address of each object section, by a named symbol in it.
    let mut sec_addr: BTreeMap<u16, u64> = BTreeMap::new();
    for s in &osyms {
        if s.name.is_empty() || s.shndx == 0 || s.shndx >= 0xff00 || s.info & 0xF == 3 {
            continue;
        }
        if let Some(o) = by_name.get(s.name.as_str()) {
            sec_addr.entry(s.shndx).or_insert(o.value.wrapping_sub(s.value));
        }
    }
    let tls_base = tls.map_or(0, |t| t.start);
    let mut tables = Tables::default();
    for (i, s) in osyms.iter().enumerate() {
        let k = key(u32::try_from(i).unwrap());
        let ty = s.info & 0xF;
        let in_tls =
            obj.sections.get(usize::from(s.shndx)).is_some_and(|x| x.flags & elf::SHF_TLS != 0);
        let (kind, v, thread_local) = if s.shndx == SHN_ABS {
            (TargetKind::Abs, s.value, false)
        } else if s.shndx == 0 {
            match by_name.get(s.name.as_str()) {
                Some(o) if o.shndx != 0 => {
                    let t = o.info & 0xF == STT_TLS;
                    let kind = if o.info & 0xF == STT_GNU_IFUNC {
                        TargetKind::Ifunc
                    } else {
                        TargetKind::Local
                    };
                    (kind, if t { tls_base + o.value } else { o.value }, t)
                }
                _ if s.name == "_TLS_MODULE_BASE_" => (TargetKind::Local, tls_base, true),
                _ if s.info >> 4 == STB_WEAK => (TargetKind::Weak0, 0, ty == STT_TLS),
                _ => (TargetKind::Undefined, 0, false),
            }
        } else {
            let Some(&a) = sec_addr.get(&s.shndx) else {
                tables.sym(k, 0, TargetKind::Undefined);
                continue;
            };
            let v = a.wrapping_add(s.value);
            // A TLS section's "address" is its offset in the block.
            if in_tls {
                (TargetKind::Local, tls_base + v, true)
            } else {
                (TargetKind::Local, v, false)
            }
        };
        if thread_local {
            tables.tls_sym(k, v, kind)
        } else {
            tables.sym(k, v, kind)
        };
        tables.names.insert(k, s.name.as_bytes().to_vec());
    }
    let x = X64::M1;
    let got_base = by_name
        .get("_GLOBAL_OFFSET_TABLE_")
        .map(|s| s.value)
        .or_else(|| img.sections.iter().find(|s| s.name == ".got.plt").map(|s| s.addr));
    let mut patches = Vec::new();
    for rs in obj.sections.iter().filter(|s| s.kind == SHT_RELA) {
        let sec = &obj.sections[rs.info as usize];
        if sec.flags & SHF_ALLOC == 0 || sec.name == ".eh_frame" {
            continue;
        }
        let Some(&addr) = sec_addr.get(&(rs.info as u16)) else {
            panic!("{label}: no output address for {}", sec.name)
        };
        let osec = img
            .sections
            .iter()
            .find(|o| {
                o.flags & SHF_ALLOC != 0
                    && o.kind != SHT_NOBITS
                    && o.addr <= addr
                    && addr < o.addr + o.size as u64
            })
            .unwrap_or_else(|| panic!("{label}: {} at {addr:#x} not in the image", sec.name));
        let file_off = osec.offset + (addr - osec.addr) as usize;
        let view = obj.data(sec).to_vec();
        let reference = img.bytes[file_off..file_off + view.len()].to_vec();
        let relocs: Vec<Reloc> = obj
            .data(rs)
            .chunks_exact(24)
            .map(|e| {
                let info = u64_at(e, 8);
                Reloc {
                    r_offset: u64_at(e, 0),
                    key: key((info >> 32) as u32),
                    r_type: (info & 0xFFFF_FFFF) as u32,
                    addend: u64_at(e, 16) as i64,
                }
            })
            .collect();
        let sym_of: Vec<usize> =
            obj.data(rs).chunks_exact(24).map(|e| (u64_at(e, 8) >> 32) as usize).collect();
        // The slot each still-GOT-loading reference site uses.
        let ref_slot: BTreeMap<u64, u64> = relocs
            .iter()
            .filter(|r| matches!(r.r_type, 9 | 41 | 42))
            .filter_map(|r| {
                let p = usize::try_from(r.r_offset).ok()?;
                let f = reference.get(p..p + 4)?;
                (p >= 2 && reference[p - 2] == view[p - 2]).then_some(())?;
                let f = i32::from_le_bytes(f.try_into().unwrap());
                let g = (addr + r.r_offset)
                    .wrapping_add(i64::from(f) as u64)
                    .wrapping_sub(r.addend as u64);
                got_word(img, g).map(|_| (r.r_offset, g))
            })
            .collect();
        // GOT slots: the reference's `.got` word holding the value.
        for rec in &relocs {
            let Some(h) = table::howto(rec.r_type) else { continue };
            let mortar_arch::Base::G(kind) = h.value.base else { continue };
            let sv = tables.syms[&rec.key];
            let want = match (kind, sv.kind) {
                (_, TargetKind::Weak0) => 0,
                (SlotKind::Addr, _) => sv.s,
                (SlotKind::TpOff { negated: false }, _) => match tls {
                    Some(t) => x.tls().tp_rel(sv.s, t) as u64,
                    None => continue,
                },
                _ => continue,
            };
            // The slot the reference's bytes load from (its word is 0
            // under a static-pie `RELATIVE`), else a word of the value.
            let own = ref_slot
                .get(&rec.r_offset)
                .copied()
                .filter(|g| got_word(img, *g) == Some(want) || output == OutputKind::Pie);
            if let Some(g) = own {
                tables.slots.insert((rec.key, kind), g);
            } else if let Some(c) = got.get(&want) {
                tables.slots.entry((rec.key, kind)).or_insert(c[0]);
            }
        }
        let cx = FillCx { tables: &tables, output, got_base, tls };
        let mut mine = view.clone();
        let exec = sec.flags & SHF_EXECINSTR != 0;
        let mut windows = Vec::new();
        // lld's W4: `P − 4` of each GD22 lead (the reference starts the
        // rewrite one byte early).
        let w4: Vec<usize> = relocs
            .iter()
            .filter(|r| r.r_type == 19)
            .map(|r| r.r_offset as usize)
            .filter(|&p| {
                p >= 4 && reference.get(p - 4..p + 1) == Some(&[0x64, 0x48, 0x8b, 0x04, 0x25][..])
            })
            .map(|p| p - 4)
            .collect();
        let mut restore = Vec::new();
        for (i, rec) in relocs.iter().enumerate() {
            let site = Site { bytes: &view, offset: rec.r_offset };
            let h = x.classify(rec.r_type, site).unwrap_or_else(|r| panic!("{label}: {r:?}"));
            let at = RelocAt {
                file: FileId(1),
                section: SectionId(rs.info),
                index: i as u32,
                r_offset: rec.r_offset,
                key: rec.key,
                r_type: rec.r_type,
                addend: Some(rec.addend),
                howto: &h,
                site,
                records: &relocs.as_slice(),
                proof: &[],
                leads: true,
            };
            let mut out =
                SectionOut::whole(&mut mine, addr, true, sec.name.as_bytes()).with_exec(exec);
            let res = x.apply(&cx, &at, &mut out);
            windows.push((res, x.unit_window(output, &at, &tables).unwrap()));
        }
        for (i, (res, u)) in windows.iter().enumerate() {
            let rec = relocs[i];
            let name = osyms.get(sym_of[i]).map_or("", |s| s.name.as_str());
            let tally = rep.by_type.entry(rec.r_type).or_default();
            let (a, b) = (u.start as usize, u.end as usize);
            let what = format!(
                "{label}: {} {}+{:#x} against `{name}`",
                mortar_elf::reloc::x86_64::name(rec.r_type).unwrap_or("?"),
                sec.name,
                rec.r_offset
            );
            let p = rec.r_offset as usize;
            let op = |v: &[u8], back: usize| p.checked_sub(back).and_then(|o| v.get(o)).copied();
            if let Err(e) = res {
                // X-22: GNU ld rewrote the `abs` GOT load to `mov $S` and
                // made no slot, so mortar's form has none in this layout.
                if *e == FieldError::MissingSlot
                    && matches!(rec.r_type, 41 | 42)
                    && op(&reference, 2) == Some(0xc7)
                {
                    tally.documented += 1;
                    *rep.forms.entry("GNU mov $imm, no slot").or_default() += 1;
                } else {
                    tally.mismatch += 1;
                    rep.mismatches.push(format!("{what}: mortar error {e:?}"));
                }
                restore.push((a, b));
                continue;
            }
            if output == OutputKind::Pie && rec.r_type == 1 {
                tally.emit += 1;
                restore.push((a, b));
                continue;
            }
            if mine[a..b] == reference[a..b] {
                tally.ok += 1;
                continue;
            }
            let weak0_tls =
                tables.syms[&rec.key].kind == TargetKind::Weak0 && tables.syms[&rec.key].tls;
            // §8.8's reference forms where they differ from mortar's.
            let doc = match rec.r_type {
                _ if only_w4(&mine[a..b], &reference[a..b], a, &w4) => Some("lld W4 64 at P-4"),
                19 if w4.contains(&(p - 4)) && mine[a..b].starts_with(&[0x64]) => {
                    Some("lld W4 GD22")
                }
                // X-18: GNU ld's `TpRel(0)` for a thread-local `weak0`.
                22 | 23 if weak0_tls && op(&reference, 2) == Some(0xc7) => {
                    Some("GNU TpRel(0) weak0")
                }
                // D1: the `GOTTPOFF` `add` as `lea`.
                22 if op(&view, 2) == Some(0x03) && op(&reference, 2) == Some(0x8d) => {
                    Some("D1 lea")
                }
                // GNU ld: `mov $S` for a `mov` (and `abs`), `mov $0` /
                // `cmp $0` for `weak0`.
                41 | 42 if op(&reference, 2) == Some(0xc7) => Some("GNU mov $imm"),
                9 if op(&reference, 2) == Some(0x81) => Some("GNU cmp $0"),
                _ => None,
            };
            if let Some(d) = doc {
                tally.documented += 1;
                *rep.forms.entry(d).or_default() += 1;
            } else {
                tally.mismatch += 1;
                rep.mismatches.push(format!(
                    "{what}: mortar {:02x?} reference {:02x?}",
                    &mine[a..b],
                    &reference[a..b]
                ));
            }
        }
        for (a, b) in restore {
            mine[a..b].copy_from_slice(&reference[a..b]);
        }
        patches.push((file_off, mine));
    }
    patches
}

/// Whether `mine` and the reference (at section offset `a`) differ only
/// at bytes lld's W4 clobbered: `P − 4` of a GD22 lead, which it sets to
/// `64` (x64.md §8.3).
fn only_w4(mine: &[u8], reference: &[u8], a: usize, w4: &[usize]) -> bool {
    mine.iter()
        .zip(reference)
        .enumerate()
        .all(|(k, (m, r))| m == r || (*r == 0x64 && w4.contains(&(a + k))))
}

/// The `.got` word at `addr`, if `addr` is one.
fn got_word(img: &Image, addr: u64) -> Option<u64> {
    let s = img
        .sections
        .iter()
        .find(|s| s.name == ".got" && s.addr <= addr && addr + 8 <= s.addr + s.size as u64)?;
    let o = (addr - s.addr) as usize;
    o.is_multiple_of(8).then(|| u64_at(&img.data(s)[o..o + 8], 0))
}

/// x64.md §15 step 3: GNU ld's `--emit-relocs` records of `obj`'s
/// sections, each paired with the object's record at the same place, by
/// `(input type, output type, GNU's form)`.
fn gnu_converted(img: &Image, obj: &Image, out: &mut BTreeMap<(u32, u32, &'static str), u32>) {
    let osyms = obj.symbols();
    let isyms = img.symbols();
    let mut count: BTreeMap<&str, u32> = BTreeMap::new();
    for s in isyms.iter().filter(|s| !s.name.is_empty() && s.shndx != 0) {
        *count.entry(s.name.as_str()).or_default() += 1;
    }
    let by_name: BTreeMap<&str, &elf::Sym> = isyms
        .iter()
        .filter(|s| !s.name.is_empty() && s.shndx != 0 && count[s.name.as_str()] == 1)
        .map(|s| (s.name.as_str(), s))
        .collect();
    let mut sec_addr: BTreeMap<u16, u64> = BTreeMap::new();
    for s in &osyms {
        if s.name.is_empty() || s.shndx == 0 || s.shndx >= 0xff00 || s.info & 0xF == STT_TLS {
            continue;
        }
        if let Some(o) = by_name.get(s.name.as_str()) {
            sec_addr.entry(s.shndx).or_insert(o.value.wrapping_sub(s.value));
        }
    }
    // GNU's emitted records, by address.
    let mut emitted: BTreeMap<u64, u32> = BTreeMap::new();
    for rs in img.sections.iter().filter(|s| s.kind == SHT_RELA && s.info != 0) {
        if img.sections[rs.info as usize].flags & SHF_ALLOC == 0 {
            continue;
        }
        for e in img.data(rs).chunks_exact(24) {
            emitted.insert(u64_at(e, 0), (u64_at(e, 8) & 0xFFFF_FFFF) as u32);
        }
    }
    for rs in obj.sections.iter().filter(|s| s.kind == SHT_RELA) {
        let sec = &obj.sections[rs.info as usize];
        if sec.flags & SHF_ALLOC == 0 || sec.name == ".eh_frame" {
            continue;
        }
        let Some(&addr) = sec_addr.get(&(rs.info as u16)) else { continue };
        let Some(osec) = img.sections.iter().find(|o| {
            o.flags & SHF_ALLOC != 0
                && o.kind != SHT_NOBITS
                && o.addr <= addr
                && addr < o.addr + o.size as u64
        }) else {
            continue;
        };
        let file_off = osec.offset + (addr - osec.addr) as usize;
        let reference = &img.bytes[file_off..file_off + obj.data(sec).len()];
        for e in obj.data(rs).chunks_exact(24) {
            let p = u64_at(e, 0) as usize;
            let tin = (u64_at(e, 8) & 0xFFFF_FFFF) as u32;
            let op = |back: usize| p.checked_sub(back).and_then(|o| reference.get(o)).copied();
            let at = |k: usize, b: &[u8]| {
                p.checked_sub(k).and_then(|o| reference.get(o..o + b.len())) == Some(b)
            };
            let Some(&tout) = emitted.get(&(addr + p as u64)) else {
                // G4: `ff 25` → `e9 disp32 90` from `P − 2`, the `PC32`
                // at the displacement, one byte earlier.
                let f = if tin == 41 && op(2) == Some(0xe9) && reference.get(p + 3) == Some(&0x90) {
                    emitted.get(&(addr + p as u64 - 1)).map(|t| (*t, "jmp; nop"))
                } else {
                    None
                };
                let (t, f) = f.unwrap_or((0, "dropped"));
                *out.entry((tin, t, f)).or_default() += 1;
                continue;
            };
            let form = match (tin, tout) {
                // GNU keeps the TLS types of the sites it relaxes.
                (22, 22) if op(2) == Some(0xc7) => "I1 mov $T",
                (22, 22) if op(2) == Some(0x8d) => "I2 lea",
                (22, 22) if op(2) == Some(0x81) => "I2 add $T",
                (22, 22) if matches!(op(2), Some(0x8b | 0x03)) => "IE kept",
                (19, 19) if at(4, &[0x64, 0x48, 0x8b, 0x04, 0x25]) => "GD -> LE",
                (20, 20) if at(3, &[0x66, 0x66, 0x66, 0x64, 0x48, 0x8b, 0x04, 0x25]) => {
                    "LD12 -> LE"
                }
                (20, 20) if at(3, &[0x66, 0x66, 0x66, 0x66, 0x64, 0x48, 0x8b, 0x04, 0x25]) => {
                    "LD13 -> LE"
                }
                (19, 19)
                    if at(3, &[0x64, 0x48, 0x8b, 0x04, 0x25])
                        && reference.get(p + 13..p + 19)
                            == Some(&[0x66, 0x0f, 0x1f, 0x44, 0, 0][..]) =>
                {
                    "GD22 -> LE"
                }
                (20, 20) if at(3, &[0x66, 0x66, 0x66, 0x66, 0x2e, 0x0f, 0x1f, 0x84]) => {
                    "LD22 -> LE"
                }
                (34, 34) if op(2) == Some(0xc7) => "T1 mov $T",
                (35, 35) if reference.get(p..p + 2) == Some(&[0x66, 0x90][..]) => "T2 nop",
                (19 | 20 | 34 | 35, _) => "?",
                _ if tin == tout => "kept",
                (9 | 41 | 42, 2) if op(2) == Some(0x8d) => "lea",
                (41, 2) if op(2) == Some(0x67) && op(1) == Some(0xe8) => "addr32 call",
                (41 | 42, 10 | 11) if op(2) == Some(0xc7) => "mov $imm",
                (42, 10 | 11) if op(2) == Some(0x81) => "binop $imm",
                (42, 10 | 11) if op(2) == Some(0xf7) => "test $imm",
                (22, 23) if op(2) == Some(0xc7) => "mov $T",
                (22, 23) if op(2) == Some(0x8d) => "lea T",
                (22, 23) if op(2) == Some(0x81) => "add $T",
                _ => "?",
            };
            *out.entry((tin, tout, form)).or_default() += 1;
        }
    }
}

fn qemu(t: &Tools, exe: &Path) -> (bool, String) {
    // No core file: some reference programs crash by design (W4).
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
    eprintln!("{what}: forms {:?}", rep.forms);
    eprintln!(
        "{:<4} {:<32} {:>6} {:>6} {:>6} {:>6} {:>6}",
        "type", "name", "ok", "doc", "ifunc", "emit", "mism"
    );
    for (ty, tl) in &rep.by_type {
        eprintln!(
            "{ty:<4} {:<32} {:>6} {:>6} {:>6} {:>6} {:>6}",
            mortar_elf::reloc::x86_64::name(*ty).unwrap_or("?"),
            tl.ok,
            tl.documented,
            tl.ifunc,
            tl.emit,
            tl.mismatch
        );
    }
    for m in rep.mismatches.iter().take(60) {
        eprintln!("MISMATCH {m}");
    }
}

#[test]
fn s15_apply_in_reference_layouts() {
    let Some(t) = tools() else { return };
    let dir = Scratch::new("x64-diff");
    let mut lld_rep = Report::default();
    let mut site_rep = Report::default();
    let mut failures = Vec::new();
    let mut gnu: BTreeMap<(u32, u32, &'static str), u32> = BTreeMap::new();
    for b in BUILDS {
        let objs = compile(&t, &dir, b);
        let output = if b.pie { OutputKind::Pie } else { OutputKind::Static };
        let bfd_exe = dir.join(format!("{}.bfd", b.name));
        let (bfd_ok, bfd_out) = if b.bfd {
            must(&format!("bfd {}", b.name), &link(&t, b, &objs, &bfd_exe, false));
            let r = qemu(&t, &bfd_exe);
            if !r.0 {
                eprintln!("REF bfd {}: {}", b.name, &r.1[..r.1.len().min(300)]);
            }
            r
        } else {
            (false, String::new())
        };
        if b.bfd {
            let bfd = Image::read(&bfd_exe);
            for o in &objs {
                gnu_converted(&bfd, &Image::read(o), &mut gnu);
            }
        }
        let mut line = format!("{:<18} bfd {bfd_ok:<5}", b.name);
        let check = |who: &str, ok: bool, out: &str, failures: &mut Vec<String>| {
            let good = if b.sites {
                let k = sites::relax_s(b.name.ends_with("bfd")).1;
                ok && out.contains(&format!("{k} checks, 0 failures"))
            } else {
                ok && bfd_ok && out == bfd_out
            };
            if !good {
                failures.push(format!("{} {who}: {out}", b.name));
            }
            good
        };
        if b.sites && b.bfd {
            let obj = Image::read(&objs[0]);
            let bfd = Image::read(&bfd_exe);
            let p = compare_object(&format!("bfd {}", b.name), &bfd, &obj, output, &mut site_rep);
            let exe = write_exe(&dir, &format!("{}.bfd-mortar", b.name), &bfd, &p);
            let (ok, out) = qemu(&t, &exe);
            let good = check("mortar in GNU ld's layout", ok, &out, &mut failures);
            let _ = write!(line, " | mortar-in-bfd {good:<5}");
        }
        if b.lld {
            let lld_exe = dir.join(format!("{}.lld", b.name));
            must(&format!("lld {}", b.name), &link(&t, b, &objs, &lld_exe, true));
            let (lld_ok, lld_out) = qemu(&t, &lld_exe);
            if !lld_ok {
                eprintln!("REF lld {}: {}", b.name, &lld_out[..lld_out.len().min(300)]);
            }
            let img = Image::read(&lld_exe);
            let w4 = |r: &Report| r.forms.get("GD22 (W4)").copied().unwrap_or(0);
            let before = w4(&lld_rep);
            let p = compare(b.name, &img, output, &mut lld_rep);
            let exe = write_exe(&dir, &format!("{}.lld-mortar", b.name), &img, &p);
            let _ = write!(line, " | lld {lld_ok:<5}");
            if !lld_ok && !b.sites && w4(&lld_rep) > before {
                // W4 clobbered a byte of an instruction no record covers
                // (`sub $0x64, %rsp`): lld's input bytes are lost there.
                line += " mortar-in-lld (lld W4-corrupt, not run)";
            } else {
                let (ok, out) = qemu(&t, &exe);
                let good = check("mortar in lld's layout (rebuilt input)", ok, &out, &mut failures);
                let _ = write!(line, " mortar-in-lld {good:<5}");
            }
            if b.sites {
                let obj = Image::read(&objs[0]);
                let p =
                    compare_object(&format!("lld {}", b.name), &img, &obj, output, &mut site_rep);
                let exe = write_exe(&dir, &format!("{}.lld-mortar-obj", b.name), &img, &p);
                let (ok, out) = qemu(&t, &exe);
                let good = check("mortar in lld's layout (object)", ok, &out, &mut failures);
                let _ = write!(line, " mortar-obj-in-lld {good:<5}");
            }
        }
        eprintln!("{line}");
    }
    print(&lld_rep, "every record of lld's outputs, input rebuilt");
    print(&site_rep, "the sites object in GNU ld's and lld's layouts");
    eprintln!("GNU ld --emit-relocs, the compiled objects' records: (in, out, form) count");
    let name = |t: u32| mortar_elf::reloc::x86_64::name(t).unwrap_or("-");
    for ((i, o, f), n) in &gnu {
        eprintln!("  {:<26} -> {:<26} {f:<12} {n}", name(*i), name(*o));
    }
    // §15 step 3: GNU converts only the GOT loads (to `PC32`, `32`,
    // `32S`), each in a §8.8 GNU form, and keeps every other type.
    let bad: Vec<_> = gnu
        .keys()
        .filter(|(i, o, f)| {
            *f == "?"
                || *f == "dropped"
                || (i != o && !matches!((i, o), (9 | 41 | 42, 2 | 10 | 11)))
        })
        .collect();
    assert!(bad.is_empty(), "GNU ld records outside §8.8: {bad:?}");
    for f in &failures {
        eprintln!("RUN FAILED {f}");
    }
    assert!(lld_rep.mismatches.is_empty(), "{} mismatches (lld)", lld_rep.mismatches.len());
    assert!(site_rep.mismatches.is_empty(), "{} mismatches (sites)", site_rep.mismatches.len());
    let total: u32 = lld_rep.by_type.values().map(|t| t.ok).sum();
    assert!(total > 10_000, "{total} records compared");
    assert!(failures.is_empty(), "runs failed:\n{}", failures.join("\n"));
}

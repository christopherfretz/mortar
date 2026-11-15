//! Differential tests against the reference linkers (`spec/synth.md`
//! §16): on objects GCC and Clang make for the three targets,
//!
//! - X-6/X-7: the merged strings of an output section, in mortar's order
//!   (S-24), are GNU ld's after removing the strings GNU tail-merged away
//!   (S-23: mortar does not tail-merge);
//! - S-17/S-19: every reference from `.data` into a string section
//!   designates the bytes ld, ld.lld and mold put at the address they
//!   wrote.
//!
//! Tools that are missing skip a case unless `MORTAR_REQUIRE_TOOLS` is
//! set.
mod support;

use std::path::{Path, PathBuf};

use mortar_core::par::Pool;
use mortar_core::{Column, OutSecId, SectionId};
use mortar_elf::consts::{STT_OBJECT, STT_SECTION};
use mortar_elf::{Machine, SectionIndex, SymbolSection};
use mortar_merge::order::offsets;
use mortar_merge::phase::{self, Phase};
use mortar_merge::refs::{named_symbol, section_symbol};
use mortar_testutil::{Scratch, run, tool, tools_required};
use support::{Objects, relocs};

const A: &str = r#"
const char *a_tbl[] = { "alpha", "beta", "gamma", "alpha" + 2, "eta", "", "beta" + 3, "mu" };
const char *fa(int i) { static const char *t[] = { "one", "two", "ta" }; return t[i]; }
"#;
const B: &str = r#"
const char *b_tbl[] = { "gamma", "delta", "ta", "alpha", "beta" + 1, "zeta" + 2, "nu", "mu" };
"#;
const C: &str = r#"
const __WCHAR_TYPE__ *c_tbl[] = { L"wide", L"de", L"wide" + 1 };
const char *c_str[] = { "delta", "pi", "zeta" };
"#;

struct Target {
    machine: Machine,
    triple: &'static str,
    gcc: &'static str,
    ld: &'static str,
}

const TARGETS: [Target; 3] = [
    Target {
        machine: Machine::AArch64,
        triple: "aarch64-linux-gnu",
        gcc: "aarch64-linux-gnu-gcc",
        ld: "aarch64-linux-gnu-ld",
    },
    Target {
        machine: Machine::X86_64,
        triple: "x86_64-linux-gnu",
        gcc: "x86_64-linux-gnu-gcc",
        ld: "x86_64-linux-gnu-ld",
    },
    Target {
        machine: Machine::I386,
        triple: "i686-linux-gnu",
        gcc: "i686-linux-gnu-gcc",
        ld: "i686-linux-gnu-ld",
    },
];

fn need(name: &str) -> Option<PathBuf> {
    let t = tool(name);
    assert!(t.is_some() || !tools_required(), "{name} is required");
    t
}

/// Compiles the three sources with `cc` (plus `args`) into `dir`.
fn compile(dir: &Path, cc: &Path, args: &[&str]) -> Option<Vec<PathBuf>> {
    let mut objs = Vec::new();
    for (name, src) in [("a", A), ("b", B), ("c", C)] {
        let c = dir.join(format!("{name}.c"));
        std::fs::write(&c, src).unwrap();
        let o = dir.join(format!("{name}.o"));
        let mut a: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
        a.extend(["-c".into(), c.display().to_string(), "-o".into(), o.display().to_string()]);
        let out = run(cc, &a);
        assert!(out.success(), "{} {a:?}: {}", cc.display(), out.stderr_text());
        objs.push(o);
    }
    Some(objs)
}

/// A linked executable's sections and symbols (ELF32 or ELF64 LE).
struct Exe {
    bytes: Vec<u8>,
    /// (name, addr, offset, size)
    sections: Vec<(String, u64, u64, u64)>,
    symbols: Vec<(String, u64)>,
}

fn u(b: &[u8], at: usize, w: usize) -> u64 {
    let mut v = [0u8; 8];
    v[..w].copy_from_slice(&b[at..at + w]);
    u64::from_le_bytes(v)
}

impl Exe {
    fn read(path: &Path) -> Self {
        let bytes = std::fs::read(path).unwrap();
        let wide = bytes[4] == 2;
        let (shoff, shentsize, shnum, shstrndx) = if wide {
            (u(&bytes, 0x28, 8), u(&bytes, 0x3a, 2), u(&bytes, 0x3c, 2), u(&bytes, 0x3e, 2))
        } else {
            (u(&bytes, 0x20, 4), u(&bytes, 0x2e, 2), u(&bytes, 0x30, 2), u(&bytes, 0x32, 2))
        };
        let hdr = |i: u64| -> (u64, u32, u64, u64, u64, u32) {
            let h = (shoff + i * shentsize) as usize;
            if wide {
                (
                    u(&bytes, h, 4),
                    u(&bytes, h + 4, 4) as u32,
                    u(&bytes, h + 0x10, 8),
                    u(&bytes, h + 0x18, 8),
                    u(&bytes, h + 0x20, 8),
                    u(&bytes, h + 0x28, 4) as u32,
                )
            } else {
                (
                    u(&bytes, h, 4),
                    u(&bytes, h + 4, 4) as u32,
                    u(&bytes, h + 0xc, 4),
                    u(&bytes, h + 0x10, 4),
                    u(&bytes, h + 0x14, 4),
                    u(&bytes, h + 0x18, 4) as u32,
                )
            }
        };
        let strs = hdr(shstrndx);
        let cstr = |off: u64| -> String {
            let s = &bytes[off as usize..];
            String::from_utf8_lossy(&s[..s.iter().position(|&b| b == 0).unwrap()]).into_owned()
        };
        let mut sections = Vec::new();
        let mut symbols = Vec::new();
        for i in 0..shnum {
            let (name, ty, addr, off, size, link) = hdr(i);
            sections.push((cstr(strs.3 + name), addr, off, size));
            if ty == 2 {
                let strtab = hdr(u64::from(link));
                let ent = if wide { 24 } else { 16 };
                for k in 0..size / ent {
                    let e = (off + k * ent) as usize;
                    let (n, v) = if wide {
                        (u(&bytes, e, 4), u(&bytes, e + 8, 8))
                    } else {
                        (u(&bytes, e, 4), u(&bytes, e + 4, 4))
                    };
                    symbols.push((cstr(strtab.3 + n), v));
                }
            }
        }
        Self { bytes, sections, symbols }
    }

    fn symbol(&self, name: &str) -> Option<u64> {
        self.symbols.iter().find(|s| s.0 == name).map(|s| s.1)
    }

    /// The bytes at address `addr`, `len` of them.
    fn at(&self, addr: u64, len: usize) -> Option<&[u8]> {
        let s = self.sections.iter().find(|s| s.1 != 0 && s.1 <= addr && addr < s.1 + s.3)?;
        let off = (s.2 + addr - s.1) as usize;
        self.bytes.get(off..off + len)
    }
}

fn phase_of(objs: &Objects) -> Phase {
    let r = relocs(objs.machine);
    phase::run(&Pool::new(3), objs, r.as_ref()).unwrap_or_else(|p| panic!("{p:?}"))
}

fn load(paths: &[PathBuf]) -> Objects {
    Objects::new(
        paths.iter().map(|p| (p.display().to_string(), std::fs::read(p).unwrap())).collect(),
    )
}

/// One reference from a data section into a split string section: the
/// global array symbol and offset it is at, and the bytes mortar maps it
/// to (the rest of its piece from the designated byte).
struct Ref {
    symbol: String,
    at: u64,
    bytes: Vec<u8>,
}

/// Every reference from the objects' data sections into split sections,
/// mapped by S-17 or S-19.
fn refs(objs: &Objects, p: &Phase) -> (Vec<Ref>, usize) {
    let mut out = Vec::new();
    let mut skipped = 0;
    let rep = |f: mortar_core::FragId| {
        let frag = p.split.frags[f];
        let file = objs.file_of(frag.section);
        let o = objs.object(file);
        let index = frag.section.0 - objs.id(file, 0).0;
        let d = o.section(SectionIndex(index)).unwrap().data().unwrap();
        d[frag.offset as usize..(frag.offset + frag.size) as usize].to_vec()
    };
    for f in 0..objs.len() {
        let o = objs.object(f);
        let syms = o.symbols().unwrap().unwrap();
        for s in o.sections() {
            if !(s.sh_type() == mortar_elf::consts::SHT_RELA
                || s.sh_type() == mortar_elf::consts::SHT_REL)
            {
                continue;
            }
            let target = o.section(SectionIndex(s.header().sh_info)).unwrap();
            let tname = target.name().unwrap();
            if !tname.starts_with(b".data") {
                continue;
            }
            let data = target.data().unwrap();
            let recs: Vec<(u64, u32, i64)> = if s.sh_type() == mortar_elf::consts::SHT_RELA {
                s.rela()
                    .unwrap()
                    .records()
                    .iter()
                    .map(|r| (r.r_offset, r.r_sym, r.r_addend))
                    .collect()
            } else {
                s.rel()
                    .unwrap()
                    .records()
                    .iter()
                    .map(|r| {
                        (
                            r.r_offset,
                            r.r_sym,
                            i64::from(i32::from_le_bytes(
                                data[r.r_offset as usize..r.r_offset as usize + 4]
                                    .try_into()
                                    .unwrap(),
                            )),
                        )
                    })
                    .collect()
            };
            for (off, sym, a) in recs {
                let st = syms.get(mortar_elf::SymbolIndex(sym)).unwrap();
                let Ok(SymbolSection::Section(i)) =
                    syms.section_of(mortar_elf::SymbolIndex(sym), &st)
                else {
                    continue;
                };
                let id: SectionId = objs.id(f, i.0);
                if p.split.merge_pieces(id).is_none() {
                    continue;
                }
                let size = o.section(i).unwrap().size();
                let (piece, within) = if st.st_info & 0xf == STT_SECTION {
                    let pr = section_symbol(&p.split, id, size, st.st_value as i64 + a).unwrap();
                    (pr.frag, pr.within as i64)
                } else {
                    let pr = named_symbol(&p.split, id, size, st.st_value).unwrap();
                    (pr.frag, pr.within as i64 + a)
                };
                let bytes = rep(piece);
                if within < 0 || within as usize >= bytes.len() {
                    skipped += 1;
                    continue;
                }
                // The array symbol holding the word (global, or a local
                // `static` table).
                let holder = syms.iter().find(|(gi, g)| {
                    g.st_info & 0xf == STT_OBJECT
                        && syms.section_of(*gi, g).ok()
                            == Some(SymbolSection::Section(target.index()))
                        && g.st_value <= off
                        && off < g.st_value + g.st_size
                });
                let (_, g) = holder.expect("an array symbol holds the word");
                out.push(Ref {
                    symbol: String::from_utf8_lossy(syms.name(&g).unwrap()).into_owned(),
                    at: off - g.st_value,
                    bytes: bytes[within as usize..].to_vec(),
                });
            }
        }
    }
    (out, skipped)
}

fn link(ld: &Path, extra: &[&str], objs: &[PathBuf], out: &Path) -> Option<Exe> {
    let mut a: Vec<String> = extra.iter().map(|s| (*s).to_owned()).collect();
    a.extend(["-static".into(), "-e".into(), "0".into(), "-o".into(), out.display().to_string()]);
    a.extend(objs.iter().map(|o| o.display().to_string()));
    let r = run(ld, &a);
    assert!(r.success(), "{} {a:?}: {}", ld.display(), r.stderr_text());
    Some(Exe::read(out))
}

fn compilers(t: &Target) -> Vec<(String, PathBuf, Vec<&'static str>)> {
    let mut v = Vec::new();
    if let Some(g) = need(t.gcc) {
        v.push((t.gcc.to_owned(), g, vec![]));
    }
    if let Some(c) = need("clang-19") {
        let target: &'static str = Box::leak(format!("--target={}", t.triple).into_boxed_str());
        v.push(("clang-19".into(), c, vec![target]));
    }
    v
}

const FLAGS: [&[&str]; 3] =
    [&["-O2", "-fno-pic"], &["-O1", "-fPIC"], &["-O2", "-fmerge-all-constants", "-fno-pic"]];

#[test]
fn s17_s19_piece_mapping_matches_ld_lld_and_mold() {
    let s = Scratch::new("merge-diff-map");
    let mut table: Vec<String> = Vec::new();
    for t in &TARGETS {
        let linkers: Vec<(&str, PathBuf, Vec<&str>)> = [
            (t.ld, need(t.ld), vec![]),
            ("ld.lld", need("ld.lld"), vec![]),
            ("mold", need("mold"), vec![]),
        ]
        .into_iter()
        .filter_map(|(n, p, e)| p.map(|p| (n, p, e)))
        .collect();
        for (cname, cc, cargs) in compilers(t) {
            for (k, flags) in FLAGS.iter().enumerate() {
                let dir = s.join(format!("{}-{cname}-{k}", t.triple));
                std::fs::create_dir_all(&dir).unwrap();
                let mut args = cargs.clone();
                args.extend_from_slice(flags);
                let Some(paths) = compile(&dir, &cc, &args) else { continue };
                let objs = load(&paths);
                let p = phase_of(&objs);
                let (rs, skipped) = refs(&objs, &p);
                assert!(rs.len() >= 10, "{} {cname} {flags:?}: {} references", t.triple, rs.len());
                for (lname, ld, extra) in &linkers {
                    let exe = link(ld, extra, &paths, &dir.join(format!("out-{lname}"))).unwrap();
                    let word = if t.machine == Machine::I386 { 4 } else { 8 };
                    for r in &rs {
                        let base = exe
                            .symbol(&r.symbol)
                            .unwrap_or_else(|| panic!("{lname}: {}", r.symbol));
                        let ptr = u(exe.at(base + r.at, word).unwrap(), 0, word);
                        let theirs = exe
                            .at(ptr, r.bytes.len())
                            .unwrap_or_else(|| panic!("{lname}: {ptr:#x}"));
                        assert_eq!(
                            theirs,
                            &r.bytes[..],
                            "{} {cname} {flags:?} {lname}: {}+{}",
                            t.triple,
                            r.symbol,
                            r.at
                        );
                    }
                }
                table.push(format!("{} {cname} {flags:?}: {} references equal in {} linkers ({skipped} one-past-the-end skipped)", t.triple, rs.len(), linkers.len()));
            }
        }
    }
    eprintln!("{}", table.join("\n"));
}

/// The 1-byte strings of an output section, as mortar orders them, with
/// their alignment.
fn our_strings(objs: &Objects, p: &Phase, out: OutSecId) -> Vec<(Vec<u8>, usize)> {
    let live = Column::filled(p.split.frags.len(), true);
    let o = offsets(&Pool::sequential(), &p.split, &p.meta, &live);
    let metas = mortar_merge::order::frag_meta(&p.split, &p.meta);
    let c = o.chunks.iter().position(|c| c.out == out).unwrap();
    let span = o.chunks[c].frags;
    o.order[span.start as usize..(span.start + span.len) as usize]
        .iter()
        .filter(|f| metas[f.0 as usize].strings && metas[f.0 as usize].entsize == 1)
        .map(|&f| {
            let frag = p.split.frags[f];
            let file = objs.file_of(frag.section);
            let d = objs
                .object(file)
                .section(SectionIndex(frag.section.0 - objs.id(file, 0).0))
                .unwrap()
                .data()
                .unwrap()
                .to_vec();
            (
                d[frag.offset as usize..(frag.offset + frag.size) as usize].to_vec(),
                1 << metas[f.0 as usize].align,
            )
        })
        .collect()
}

#[test]
fn x6_x7_string_order_is_gnu_minus_tail_merging() {
    let s = Scratch::new("merge-diff-order");
    let mut table = Vec::new();
    for t in &TARGETS {
        let Some(ld) = need(t.ld) else { continue };
        for (cname, cc, cargs) in compilers(t) {
            let dir = s.join(format!("{}-{cname}", t.triple));
            std::fs::create_dir_all(&dir).unwrap();
            let mut args = cargs.clone();
            args.extend_from_slice(&["-O2", "-fno-pic"]);
            let Some(mut paths) = compile(&dir, &cc, &args) else { continue };
            paths.truncate(2); // a.c and b.c: 1-byte strings only
            let objs = load(&paths);
            let p = phase_of(&objs);
            let ours = our_strings(&objs, &p, OutSecId(0));
            // GNU keeps a string unless it is a proper suffix of another
            // starting at an offset of its alignment there.
            let kept: Vec<&Vec<u8>> = ours
                .iter()
                .filter(|(x, a)| {
                    !ours.iter().any(|(y, _)| {
                        y.len() > x.len() && y.ends_with(x) && (y.len() - x.len()) % a == 0
                    })
                })
                .map(|(x, _)| x)
                .collect();
            let exe = link(&ld, &[], &paths, &dir.join("out")).unwrap();
            let ro = exe.sections.iter().find(|s| s.0 == ".rodata").expect(".rodata");
            let bytes = &exe.bytes[ro.2 as usize..(ro.2 + ro.3) as usize];
            // GNU's order: where each kept string stands alone (after a
            // NUL or at the start; GNU pads to the section alignment and
            // the output section holds other data too).
            let alone = |x: &[u8]| -> Vec<usize> {
                (0..=bytes.len().saturating_sub(x.len()))
                    .filter(|&i| &bytes[i..i + x.len()] == x && (i == 0 || bytes[i - 1] == 0))
                    .collect()
            };
            let mut placed: Vec<(usize, &Vec<u8>)> = Vec::new();
            // The empty string cannot be told from padding.
            for x in kept.iter().filter(|x| x.len() > 1) {
                let at = alone(x);
                assert_eq!(
                    at.len(),
                    1,
                    "{} {cname}: {:?} in GNU's .rodata",
                    t.triple,
                    String::from_utf8_lossy(x)
                );
                placed.push((at[0], x));
            }
            let mut theirs = placed.clone();
            theirs.sort_by_key(|p| p.0);
            let shown = |v: &[(usize, &Vec<u8>)]| {
                v.iter().map(|x| String::from_utf8_lossy(x.1).into_owned()).collect::<Vec<_>>()
            };
            assert_eq!(
                shown(&placed),
                shown(&theirs),
                "{} {cname}: mortar's order minus tail merging, GNU's",
                t.triple
            );
            table.push(format!(
                "{} {cname}: {} fragments, {} after GNU's tail merge, same order",
                t.triple,
                ours.len(),
                kept.len()
            ));
        }
    }
    eprintln!("{}", table.join("\n"));
}

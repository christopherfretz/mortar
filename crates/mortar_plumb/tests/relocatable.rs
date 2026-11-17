//! Stage R (`spec/plumb.md` §14, WP-185): the verifier of `-r` outputs on
//! the references' outputs, built at test time.
//!
//! The fixtures are WP-154's D.3 C and C++ components
//! (`tests/relocatable/`, copied from `tests/relocatable-design/exp/corpus`),
//! the `-O0 -g` debugging corpus (`exp/debug`), `exp/none-overlap`, the 14
//! cases of `exp/rl0/run.sh` and `exp/rl0/none.sh`, compiled and partially
//! linked by GNU ld, lld and mold for AArch64, then x86-64, then i386.
//! GNU ld's and lld's outputs pass, or show the anomalies §10.2 names;
//! mold's fail by name (§10.2 A90-A102 and the A12x it adds). Every
//! finding of a reference is a named anomaly, and the anomalies of each
//! link are the table's.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Instant;

use mortar_plumb::{Ctx, Files, Format, Input, Map, Report};
use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, run_with, tool};

const C1: &str = include_str!("relocatable/c1.c");
const C2: &str = include_str!("relocatable/c2.c");
const C3: &str = include_str!("relocatable/c3.c");
const C4: &str = include_str!("relocatable/c4.c");
const X1: &str = include_str!("relocatable/x1.cc");
const X2: &str = include_str!("relocatable/x2.cc");
const REC_H: &str = include_str!("relocatable/rec.h");

struct Dir<'a>(&'a Path);

impl Files for Dir<'_> {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(path)).ok()
    }
}

/// The tools of one architecture.
struct Tools {
    arch: Arch,
    gcc: PathBuf,
    gxx: PathBuf,
    clang: Option<PathBuf>,
    bfd: PathBuf,
    lld: PathBuf,
    mold: PathBuf,
}

impl Tools {
    fn find(arch: Arch) -> Option<Tools> {
        let gcc = arch.tool("gcc")?;
        let gxx = arch.tool("g++")?;
        let bfd = arch.tool("ld.bfd")?;
        let lld = tool("ld.lld")?;
        let mold = tool("mold")?;
        let clang = tool("clang");
        Some(Tools { arch, gcc, gxx, clang, bfd, lld, mold })
    }

    fn emulation(&self) -> &'static str {
        match self.arch {
            Arch::Aarch64 => "aarch64linux",
            Arch::X86_64 => "elf_x86_64",
            Arch::I386 => "elf_i386",
        }
    }

    fn triple(&self) -> &'static str {
        self.arch.triple()
    }
}

/// The three references.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Ref {
    Bfd,
    Lld,
    Mold,
}

impl Ref {
    const ALL: [Ref; 3] = [Ref::Bfd, Ref::Lld, Ref::Mold];
    fn name(self) -> &'static str {
        match self {
            Ref::Bfd => "bfd",
            Ref::Lld => "lld",
            Ref::Mold => "mold",
        }
    }
    fn format(self) -> Format {
        match self {
            Ref::Bfd => Format::Gnu,
            Ref::Lld => Format::Lld,
            Ref::Mold => Format::Mold,
        }
    }
}

fn run_ok(dir: &Path, program: &Path, args: &[String]) -> Result<(), String> {
    let mut c = std::process::Command::new(program);
    c.args(args).current_dir(dir);
    let o = run_with(&mut c, b"", std::time::Duration::from_mins(2));
    if o.success() {
        Ok(())
    } else {
        Err(format!("{} {}: {}", program.display(), args.join(" "), o.stderr_text()))
    }
}

/// A compiler of a case.
#[derive(Clone, Copy)]
enum Cc {
    Gcc,
    Gxx,
    Clang,
    /// The assembler through gcc (`gcc -c x.s`).
    As,
    ClangAs,
}

/// One source of a case: its compiler, its name, its text, its flags.
struct Src {
    cc: Cc,
    name: &'static str,
    text: String,
    flags: &'static [&'static str],
    obj: &'static str,
}

/// One partial link of a case: the objects in link order and the
/// options before them.
struct Case {
    name: String,
    srcs: Vec<Src>,
    inputs: Vec<&'static str>,
    opts: Vec<&'static str>,
}

fn src(cc: Cc, name: &'static str, text: &str, flags: &'static [&'static str], obj: &'static str) -> Src {
    Src { cc, name, text: text.to_string(), flags, obj }
}

const D3: &[&str] = &["-O1", "-g", "-fcommon", "-ffunction-sections", "-fdata-sections"];
const DEBUG: &[&str] = &["-g", "-O0", "-ffunction-sections"];
const O1: &[&str] = &["-O1"];
const O0: &[&str] = &["-O0"];
const NONE: &[&str] = &[];

/// The cases of an architecture (§14.1's fixtures).
#[allow(clippy::too_many_lines)] // the corpus, one case after the other
fn cases(t: &Tools) -> Vec<Case> {
    let a64 = t.arch == Arch::Aarch64;
    let mut v = Vec::new();
    let mut comps = vec![(Cc::Gcc, Cc::Gxx, "gcc")];
    if t.clang.is_some() && t.arch != Arch::I386 {
        comps.push((Cc::Clang, Cc::Clang, "clang"));
    }
    // D.3: the C component (and its ifunc and ctors variants), the C++.
    for (cc, cxx, cn) in comps {
        let c = |n: &'static str, s: &str, o: &'static str| src(cc, n, s, D3, o);
        v.push(Case {
            name: format!("d3-c-{cn}"),
            srcs: vec![c("c1.c", C1, "c1.o"), c("c2.c", C2, "c2.o")],
            inputs: vec!["c1.o", "c2.o"],
            opts: vec![],
        });
        v.push(Case {
            name: format!("d3-cifunc-{cn}"),
            srcs: vec![c("c1.c", C1, "c1.o"), c("c2.c", C2, "c2.o"), c("c4.c", C4, "c4.o")],
            inputs: vec!["c1.o", "c2.o", "c4.o"],
            opts: vec![],
        });
        v.push(Case {
            name: format!("d3-cctors-{cn}"),
            srcs: vec![c("c1.c", C1, "c1.o"), c("c2.c", C2, "c2.o"), c("c3.c", C3, "c3.o")],
            inputs: vec!["c1.o", "c2.o", "c3.o"],
            opts: vec![],
        });
        let x = |n: &'static str, s: &str, o: &'static str| src(cxx, n, s, D3, o);
        v.push(Case {
            name: format!("d3-cxx-{cn}"),
            srcs: vec![x("x1.cc", X1, "x1.o"), x("x2.cc", X2, "x2.o")],
            inputs: vec!["x1.o", "x2.o"],
            opts: vec![],
        });
    }
    // The debugging corpus (exp/debug): -O0 -g.
    v.push(Case {
        name: "debug-cxx".into(),
        srcs: vec![src(Cc::Gxx, "x1.cc", X1, DEBUG, "x1.o"), src(Cc::Gxx, "x2.cc", X2, DEBUG, "x2.o")],
        inputs: vec!["x1.o", "x2.o"],
        opts: vec![],
    });
    // exp/none-overlap: an -O0 COMDAT .text with its own FDE in both.
    v.push(Case {
        name: "none-overlap".into(),
        srcs: vec![
            src(Cc::Gxx, "g1.cc", "inline int f(int x){static int k; return k += x;}\nint g1(int x){return f(x);}\n", O0, "g1.o"),
            src(
                Cc::Gxx,
                "g2.cc",
                "int g3(int x){return x;}\ninline int f(int x){static int k; return k += x;}\nint g2(int x){return f(x)+g3(1);}\n",
                O0,
                "g2.o",
            ),
        ],
        inputs: vec!["g1.o", "g2.o"],
        opts: vec![],
    });
    // exp/rl0/none.sh: a NONE record, the reference of a --gc-sections edge.
    let (none_ty, ret) = match t.arch {
        Arch::Aarch64 => ("R_AARCH64_NONE", "ret"),
        Arch::X86_64 => ("R_X86_64_NONE", "ret"),
        Arch::I386 => ("R_386_NONE", "ret"),
    };
    v.push(Case {
        name: "none".into(),
        srcs: vec![
            src(Cc::As, "s.s", &format!("  .text\n  .globl _start\n_start:\n  .reloc ., {none_ty}, keepme\n  {ret}\n"), NONE, "s.o"),
            src(Cc::As, "k.s", &format!("  .section .text.keepme,\"ax\"\n  .globl keepme\nkeepme:\n  {ret}\n"), NONE, "k.o"),
        ],
        inputs: vec!["s.o", "k.o"],
        opts: vec![],
    });
    if t.clang.is_some() {
        // rl0 13 and none.sh's second part: clang's call-graph profile.
        v.push(Case {
            name: "rl0-13-cgprofile".into(),
            srcs: vec![src(Cc::ClangAs, "cg.s", "  .text\n  .globl cga, cgb\ncga: ret\ncgb: ret\n  .cg_profile cga, cgb, 10\n", NONE, "cg.o")],
            inputs: vec!["cg.o"],
            opts: vec![],
        });
    }
    // rl0 1: maps; 12: STT_SECTION values (the same link).
    v.push(Case {
        name: "rl0-1-maps".into(),
        srcs: vec![
            src(Cc::Gcc, "m1.c", "static int s1(int x) { return x + 1; }\nint g1(int x) { return s1(x) * 2; }\nint d1 = 5; const char *str1 = \"hello\";\n", O1, "m1.o"),
            src(Cc::Gcc, "m2.c", "int g1(int);\nint g2(int x) { return g1(x) + 7; }\nint d2 = 9; const char *str2 = \"hello\"; int b2;\n", O1, "m2.o"),
        ],
        inputs: vec!["m1.o", "m2.o"],
        opts: vec![],
    });
    // rl0 3: groups (g3.s is AArch64 assembly: a non-COMDAT group).
    let g = "inline int shared_inline(int x) { return x * 3; }\n";
    let mut groups = Case {
        name: "rl0-3-groups".into(),
        srcs: vec![
            src(Cc::Gxx, "g1.cc", &format!("{g}int use1(int x) {{ return shared_inline(x); }}\n"), O1, "g1.o"),
            src(Cc::Gxx, "g2.cc", &format!("{g}int use2(int x) {{ return shared_inline(x) + 1; }}\n"), O1, "g2.o"),
        ],
        inputs: vec!["g1.o", "g2.o"],
        opts: vec![],
    };
    if a64 {
        groups.srcs.push(src(Cc::As, "g3.s", "  .section .text.plain,\"axG\",%progbits,plaingrp\npg:\n  bl use1\n  ret\n", NONE, "g3.o"));
        groups.inputs.extend(["g3.o", "g3.o"]);
    }
    v.push(groups);
    // rl0 4: empty input sections.
    v.push(Case {
        name: "rl0-4-empty".into(),
        srcs: vec![
            src(Cc::Gcc, "e1.c", "int e1(void) { return 1; }\n", O1, "e1.o"),
            src(Cc::As, "e2.s", &format!("  .section .data.empty,\"aw\",%progbits\n  .section .bss.empty,\"aw\",%nobits\n  .text\n  {ret}\n"), NONE, "e2.o"),
        ],
        inputs: vec!["e1.o", "e2.o"],
        opts: vec![],
    });
    // rl0 6: properties.
    let p = "int PNAME(int x) { return x + 1; }\n";
    if a64 {
        v.push(Case {
            name: "rl0-6-property".into(),
            srcs: vec![
                src(Cc::Gcc, "pa.c", &p.replace("PNAME", "pa"), &["-O1", "-mbranch-protection=standard"], "pa.o"),
                src(Cc::Gcc, "pc.c", &p.replace("PNAME", "pc"), &["-O1", "-mbranch-protection=pac-ret"], "pc.o"),
                src(Cc::Gcc, "pd.c", &p.replace("PNAME", "pd"), &["-O1", "-mbranch-protection=none"], "pd.o"),
            ],
            inputs: vec!["pa.o", "pc.o", "pd.o"],
            opts: vec![],
        });
    } else {
        v.push(Case {
            name: "rl0-6-property".into(),
            srcs: vec![
                src(Cc::Gcc, "xa.c", &p.replace("PNAME", "xa"), &["-O1", "-fcf-protection=full"], "xa.o"),
                src(Cc::Gcc, "xb.c", &p.replace("PNAME", "xb"), &["-O1", "-fcf-protection=full"], "xb.o"),
            ],
            inputs: vec!["xa.o", "xb.o"],
            opts: vec![],
        });
    }
    // rl0 7: SHF_GNU_RETAIN and plain pieces of one name.
    v.push(Case {
        name: "rl0-7-retain".into(),
        srcs: vec![
            src(Cc::As, "k1.s", &format!("  .section .text.k,\"axR\",%progbits\n  .globl k1\nk1:\n  {ret}\n"), NONE, "k1.o"),
            src(Cc::As, "k2.s", &format!("  .section .text.k,\"ax\",%progbits\n  .globl k2\nk2:\n  {ret}\n"), NONE, "k2.o"),
        ],
        inputs: vec!["k1.o", "k2.o"],
        opts: vec![],
    });
    // rl0 8: debug and .eh_frame records into a dropped COMDAT copy.
    v.push(Case {
        name: "rl0-8-dropdebug".into(),
        srcs: vec![
            src(Cc::Gxx, "d1.cc", &format!("{g}int use1(int x) {{ return shared_inline(x); }}\n"), &["-O0", "-g"], "d1.o"),
            src(Cc::Gxx, "d2.cc", &format!("{g}int use2(int x) {{ return shared_inline(x) + 1; }}\n"), &["-O0", "-g"], "d2.o"),
        ],
        inputs: vec!["d1.o", "d2.o"],
        opts: vec![],
    });
    // rl0 9: SHF_LINK_ORDER pieces linked to different sections.
    let l = "int LNAME(int x) { return x + 2; }\n";
    v.push(Case {
        name: "rl0-9-linkorder".into(),
        srcs: vec![
            src(Cc::Gcc, "l1.c", &l.replace("LNAME", "lf"), &["-O1", "-ffunction-sections", "-fpatchable-function-entry=2"], "l1.o"),
            src(Cc::Gcc, "l2.c", &l.replace("LNAME", "lg"), &["-O1", "-ffunction-sections", "-fpatchable-function-entry=2"], "l2.o"),
        ],
        inputs: vec!["l1.o", "l2.o"],
        opts: vec![],
    });
    // rl0 10: SHF_MERGE pieces of different sh_entsize.
    v.push(Case {
        name: "rl0-10-entsize".into(),
        srcs: vec![
            src(Cc::As, "z1.s", "  .section .rodata.cst,\"aM\",%progbits,4\n  .long 1\n", NONE, "z1.o"),
            src(Cc::As, "z2.s", "  .section .rodata.cst,\"aM\",%progbits,8\n  .quad 2\n", NONE, "z2.o"),
        ],
        inputs: vec!["z1.o", "z2.o"],
        opts: vec![],
    });
    // rl0 11: PROGBITS and NOBITS pieces of one name.
    v.push(Case {
        name: "rl0-11-nobits".into(),
        srcs: vec![
            src(Cc::As, "b1.s", "  .section .foo,\"aw\",%progbits\n  .long 1\n", NONE, "b1.o"),
            src(Cc::As, "b2.s", "  .section .foo,\"aw\",%nobits\n  .zero 8\n", NONE, "b2.o"),
        ],
        inputs: vec!["b1.o", "b2.o"],
        opts: vec![],
    });
    // rl0 14: one input file twice, with retained and plain .text.r.
    v.push(Case {
        name: "rl0-14-dupmaps".into(),
        srcs: vec![src(Cc::As, "dm.s", &format!("  .section .text.r,\"axR\",%progbits\n  {ret}\n  .section .text.r,\"ax\",%progbits\n  {ret}\n"), NONE, "dm.o")],
        inputs: vec!["dm.o"],
        opts: vec![],
    });
    if a64 {
        // rl0 5: a relocation section of NONE records only.
        v.push(Case {
            name: "rl0-5-noneonly".into(),
            srcs: vec![
                src(Cc::As, "n1.s", "  .text\n  .globl n1\nn1:\n  ret\n  .reloc 0, R_AARCH64_NONE, n1\n  .reloc 0, R_AARCH64_NONE, other\n", NONE, "n1.o"),
                src(Cc::As, "n2.s", "  .text\n  .globl n2\nn2:\n  ret\n", NONE, "n2.o"),
            ],
            inputs: vec!["n1.o", "n2.o"],
            opts: vec![],
        });
    }
    if t.arch == Arch::I386 {
        // rl0 2: i386 REL, section-symbol, local and global records.
        v.push(Case {
            name: "rl0-2-rel386".into(),
            srcs: vec![
                src(Cc::As, "r1.s", "  .text\n  nop\n  nop\n  nop\n  .data\n  .long 1\n  .byte 7\n", NONE, "r1.o"),
                src(
                    Cc::As,
                    "r2.s",
                    "  .text\n  .globl f2\nf2:\n  call gsym\n  call lfun\nlfun:\n  ret\n  .data\n  .balign 4\n  .long 2\nlab:\n  .long 3\n  .long lab\n  .long .text+1\n  .long gsym+4\n  .long lab-.\n  .word lab\n",
                    NONE,
                    "r2.o",
                ),
            ],
            inputs: vec!["r1.o", "r2.o"],
            opts: vec![],
        });
    }
    v
}

/// Compiles the sources of `case` into `dir`.
fn build(t: &Tools, case: &Case, dir: &Path) -> Result<(), String> {
    std::fs::write(dir.join("rec.h"), REC_H).map_err(|e| e.to_string())?;
    for s in &case.srcs {
        std::fs::write(dir.join(s.name), &s.text).map_err(|e| e.to_string())?;
        let clang_args = t.arch.clang_args();
        let (prog, mut args): (PathBuf, Vec<String>) = match s.cc {
            Cc::Gcc | Cc::As => (t.gcc.clone(), Vec::new()),
            Cc::Gxx => (t.gxx.clone(), Vec::new()),
            Cc::Clang | Cc::ClangAs => {
                let c = t.clang.clone().ok_or("clang")?;
                let c = if s.name.ends_with(".cc") {
                    c.with_file_name(c.file_name().map_or_else(OsString::new, |n| {
                        n.to_string_lossy().replacen("clang", "clang++", 1).into()
                    }))
                } else {
                    c
                };
                let mut a = vec![format!("--target={}", t.triple())];
                if clang_args.is_empty() {
                    a.clear();
                    a.push(format!("--target={}", t.triple()));
                }
                (c, a)
            }
        };
        args.extend(s.flags.iter().map(|f| (*f).to_string()));
        args.extend(["-c".into(), s.name.into(), "-o".into(), s.obj.into()]);
        run_ok(dir, &prog, &args)?;
    }
    Ok(())
}

/// A reference's `-r` of `case`: its line (the words after the program).
fn link(t: &Tools, r: Ref, case: &Case, dir: &Path) -> Result<Vec<String>, String> {
    let out = format!("{}.{}.o", case.name, r.name());
    let map = format!("{}.{}.map", case.name, r.name());
    let mut words: Vec<String> = Vec::new();
    if r != Ref::Bfd {
        words.extend(["-m".into(), t.emulation().into()]);
    }
    words.extend(["-r".into(), "-o".into(), out, format!("-Map={map}")]);
    words.extend(case.opts.iter().map(|s| (*s).to_string()));
    words.extend(case.inputs.iter().map(|s| (*s).to_string()));
    let prog = match r {
        Ref::Bfd => &t.bfd,
        Ref::Lld => &t.lld,
        Ref::Mold => &t.mold,
    };
    run_ok(dir, prog, &words)?;
    Ok(words)
}

/// Stage R on a reference's `-r` of `case`.
fn verify(r: Ref, case: &Case, dir: &Path, line: Vec<String>) -> (Report, Vec<u8>) {
    let out = std::fs::read(dir.join(format!("{}.{}.o", case.name, r.name()))).unwrap();
    let text = std::fs::read_to_string(dir.join(format!("{}.{}.map", case.name, r.name()))).unwrap();
    let map: Map = mortar_plumb::map::parse(r.format(), &text)
        .unwrap_or_else(|e| panic!("{} {}: map line {}: {}", case.name, r.name(), e.line, e.message));
    let files: Vec<(String, Vec<u8>)> = map
        .paths()
        .into_iter()
        .filter_map(|p| std::fs::read(dir.join(p)).ok().map(|b| (p.to_string(), b)))
        .collect();
    let inputs: Vec<Input<'_>> = files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
    let ctx = Ctx { line: Some(line), files: &Dir(dir) };
    (mortar_plumb::verify_with(&out, &inputs, &map, &ctx), out)
}

/// The anomaly id of a classification ("A90: ...").
fn id(class: &str) -> &str {
    class.split(':').next().unwrap_or(class)
}

/// What one architecture's references show: per (case, linker), the
/// anomaly ids; the unclassified findings, as text.
struct Seen {
    anomalies: BTreeMap<(String, &'static str), BTreeSet<String>>,
    unclassified: Vec<String>,
    refused: Vec<String>,
    lines: Vec<String>,
}

fn references(arch: Arch) -> Option<Seen> {
    let t = Tools::find(arch)?;
    let start = Instant::now();
    let scratch = Scratch::new(&format!("plumb-rel-{}", arch.name()));
    let mut seen = Seen { anomalies: BTreeMap::new(), unclassified: Vec::new(), refused: Vec::new(), lines: Vec::new() };
    let mut build_time = std::time::Duration::ZERO;
    let mut link_time = std::time::Duration::ZERO;
    let mut verify_time = std::time::Duration::ZERO;
    for case in cases(&t) {
        let dir = scratch.join(&case.name);
        std::fs::create_dir_all(&dir).unwrap();
        let b = Instant::now();
        build(&t, &case, &dir).unwrap_or_else(|e| panic!("{} [{}]: {e}", case.name, arch.name()));
        build_time += b.elapsed();
        for r in Ref::ALL {
            // mold 2.37.1 crashes (SIGSEGV) on an -r whose input defines an
            // IFUNC (c4.c): not run, so that no core file is written.
            if r == Ref::Mold && case.name.starts_with("d3-cifunc") {
                seen.refused.push(format!("{} mold: not run (mold 2.37.1 crashes on an IFUNC at -r)", case.name));
                continue;
            }
            let l = Instant::now();
            let linked = link(&t, r, &case, &dir);
            link_time += l.elapsed();
            let line = match linked {
                Ok(l) => l,
                Err(e) => {
                    seen.refused.push(format!("{} {}: {}", case.name, r.name(), e.lines().next().unwrap_or("")));
                    continue;
                }
            };
            let v = Instant::now();
            let (report, out) = verify(r, &case, &dir, line);
            verify_time += v.elapsed();
            let rc = report.coverage.relocatable.clone().unwrap_or_default();
            seen.lines.push(format!(
                "{} {} [{}]: {} findings; relocatable: pieces {}, merged {}, framed {}; groups {}; FDEs {} live, {} removed; symbols {}; sections {}",
                case.name,
                r.name(),
                arch.name(),
                report.findings.len(),
                rc.pieces,
                rc.merged,
                rc.framed,
                rc.groups,
                rc.fdes_live,
                rc.fdes_removed,
                rc.symbols,
                rc.sections
            ));
            let key = (case.name.clone(), r.name());
            let set = seen.anomalies.entry(key).or_default();
            for f in &report.findings {
                match mortar_plumb::anomaly::classify(r.format(), &out, f) {
                    Some(c) => {
                        set.insert(id(c).to_string());
                    }
                    None => seen.unclassified.push(format!(
                        "{} {} [{}]: {} {}{}",
                        case.name,
                        r.name(),
                        arch.name(),
                        f.rule.id(),
                        f.message,
                        f.site.as_ref().map_or_else(String::new, |s| format!(" ({}:{}+{:#x})", s.file, s.section, s.offset))
                    )),
                }
            }
        }
    }
    eprintln!(
        "relocatable [{}]: built in {:.1?}, linked in {:.1?}, verified in {:.1?}, {:.1?} in all",
        arch.name(),
        build_time,
        link_time,
        verify_time,
        start.elapsed()
    );
    Some(seen)
}

/// The anomalies each reference's `-r` shows (§10.2); a link not listed
/// shows none.
/// The named anomalies each reference shows, per case (measured with
/// GNU ld 2.44, lld 19.1.7 and mold 2.37.1; `spec/plumb.md` §10.2). A pair
/// that is not listed shows none: GNU ld shows none anywhere.
fn expected(arch: Arch) -> BTreeMap<(&'static str, &'static str), &'static [&'static str]> {
    const LLD: &str = "lld";
    const MOLD: &str = "mold";
    let i386 = arch == Arch::I386;
    let mut t: Vec<(&'static str, &'static str, &'static [&'static str])> = Vec::new();
    // What every target shows.
    t.extend([
        ("debug-cxx", LLD, &["A123", "A126", "A91"][..]),
        ("debug-cxx", MOLD, &["A122", "A123", "A125", "A97"][..]),
        ("none-overlap", LLD, &["A123", "A126", "A91"][..]),
        ("none-overlap", MOLD, &["A122", "A123"][..]),
        ("rl0-10-entsize", LLD, &["A100"][..]),
        ("rl0-14-dupmaps", MOLD, &["A99"][..]),
        ("rl0-7-retain", MOLD, &["A99"][..]),
        ("rl0-8-dropdebug", LLD, &["A126", "A91"][..]),
        ("rl0-9-linkorder", MOLD, &["A101"][..]),
        ("d3-cxx-gcc", MOLD, &["A122", "A123"][..]),
        ("d3-c-gcc", MOLD, &["A90"][..]),
        ("d3-cctors-gcc", MOLD, &["A124", "A90"][..]),
    ]);
    if i386 {
        // gcc's `__x86.get_pc_thunk.*` COMDAT copies: lld leaves a
        // `NONE` record where it dropped their FDEs.
        t.extend([
            ("d3-c-gcc", LLD, &["A120", "A126", "A91"][..]),
            ("d3-cctors-gcc", LLD, &["A120", "A126", "A91"][..]),
            ("d3-cifunc-gcc", LLD, &["A120", "A123", "A126", "A91"][..]),
            ("d3-cxx-gcc", LLD, &["A123", "A126", "A91"][..]),
            ("rl0-8-dropdebug", MOLD, &["A122", "A97"][..]),
        ]);
    } else {
        t.extend([
            ("d3-c-gcc", LLD, &["A120"][..]),
            ("d3-cctors-gcc", LLD, &["A120"][..]),
            ("d3-cifunc-gcc", LLD, &["A120", "A123"][..]),
            ("d3-cxx-gcc", LLD, &["A123"][..]),
            ("d3-c-clang", LLD, &["A120"][..]),
            ("d3-c-clang", MOLD, &["A90"][..]),
            ("d3-cctors-clang", LLD, &["A120"][..]),
            ("d3-cctors-clang", MOLD, &["A124", "A90"][..]),
            ("d3-cifunc-clang", LLD, &["A120", "A123"][..]),
            ("d3-cxx-clang", MOLD, &["A122"][..]),
            ("rl0-13-cgprofile", MOLD, &["A102"][..]),
            ("rl0-8-dropdebug", MOLD, &["A97"][..]),
        ]);
    }
    if arch == Arch::Aarch64 {
        t.push(("rl0-3-groups", MOLD, &["A125", "A92"][..]));
    }
    t.into_iter().map(|(c, l, a)| ((c, l), a)).collect()
}

fn judge(arch: Arch) {
    let Some(seen) = references(arch) else { return };
    for l in &seen.lines {
        eprintln!("{l}");
    }
    for r in &seen.refused {
        eprintln!("refused: {r}");
    }
    for ((case, linker), set) in &seen.anomalies {
        if !set.is_empty() {
            eprintln!("anomalies {case} {linker}: {set:?}");
        }
    }
    assert!(
        seen.unclassified.is_empty(),
        "[{}] {} findings on the references' -r outputs are no named anomaly:\n{}",
        arch.name(),
        seen.unclassified.len(),
        seen.unclassified.join("\n")
    );
    let want = expected(arch);
    let mut diffs = Vec::new();
    for ((case, linker), got) in &seen.anomalies {
        let w: BTreeSet<String> = want
            .get(&(case.as_str(), *linker))
            .map(|v| v.iter().map(|s| (*s).to_string()).collect())
            .unwrap_or_default();
        if &w != got {
            diffs.push(format!("{case} {linker}: shows {got:?}, the table says {w:?}"));
        }
    }
    assert!(diffs.is_empty(), "[{}] anomalies differ from the table:\n{}", arch.name(), diffs.join("\n"));
}

// spec: plumb.md §14 — the references' -r outputs on AArch64.
#[test]
fn s14_references_aarch64() {
    judge(Arch::Aarch64);
}

// spec: plumb.md §14 — the references' -r outputs on x86-64.
#[test]
fn s14_references_x86_64() {
    judge(Arch::X86_64);
}

// spec: plumb.md §14 — the references' -r outputs on i386.
#[test]
fn s14_references_i386() {
    judge(Arch::I386);
}

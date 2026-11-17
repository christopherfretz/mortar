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
//! link are the table's. The mutations of §9.2 (t) and one per field
//! the rules read (V-R12) start from GNU ld's outputs.

// e, f, s, o, r, k, y: the editor, file, section, output section, record,
// symbol index, symbol, throughout.
#![allow(clippy::many_single_char_names)]

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
    /// A checked-in object, copied: `text` is its path under the crate.
    Copy,
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

fn src(
    cc: Cc,
    name: &'static str,
    text: &str,
    flags: &'static [&'static str],
    obj: &'static str,
) -> Src {
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
        srcs: vec![
            src(Cc::Gxx, "x1.cc", X1, DEBUG, "x1.o"),
            src(Cc::Gxx, "x2.cc", X2, DEBUG, "x2.o"),
        ],
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
            src(
                Cc::As,
                "s.s",
                &format!(
                    "  .text\n  .globl _start\n_start:\n  .reloc ., {none_ty}, keepme\n  {ret}\n"
                ),
                NONE,
                "s.o",
            ),
            src(
                Cc::As,
                "k.s",
                &format!("  .section .text.keepme,\"ax\"\n  .globl keepme\nkeepme:\n  {ret}\n"),
                NONE,
                "k.o",
            ),
        ],
        inputs: vec!["s.o", "k.o"],
        opts: vec![],
    });
    if t.clang.is_some() {
        // rl0 13 and none.sh's second part: clang's call-graph profile.
        v.push(Case {
            name: "rl0-13-cgprofile".into(),
            srcs: vec![src(
                Cc::ClangAs,
                "cg.s",
                "  .text\n  .globl cga, cgb\ncga: ret\ncgb: ret\n  .cg_profile cga, cgb, 10\n",
                NONE,
                "cg.o",
            )],
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
            src(
                Cc::Gxx,
                "g1.cc",
                &format!("{g}int use1(int x) {{ return shared_inline(x); }}\n"),
                O1,
                "g1.o",
            ),
            src(
                Cc::Gxx,
                "g2.cc",
                &format!("{g}int use2(int x) {{ return shared_inline(x) + 1; }}\n"),
                O1,
                "g2.o",
            ),
        ],
        inputs: vec!["g1.o", "g2.o"],
        opts: vec![],
    };
    if a64 {
        groups.srcs.push(src(
            Cc::As,
            "g3.s",
            "  .section .text.plain,\"axG\",%progbits,plaingrp\npg:\n  bl use1\n  ret\n",
            NONE,
            "g3.o",
        ));
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
                src(
                    Cc::Gcc,
                    "pa.c",
                    &p.replace("PNAME", "pa"),
                    &["-O1", "-mbranch-protection=standard"],
                    "pa.o",
                ),
                src(
                    Cc::Gcc,
                    "pc.c",
                    &p.replace("PNAME", "pc"),
                    &["-O1", "-mbranch-protection=pac-ret"],
                    "pc.o",
                ),
                src(
                    Cc::Gcc,
                    "pd.c",
                    &p.replace("PNAME", "pd"),
                    &["-O1", "-mbranch-protection=none"],
                    "pd.o",
                ),
            ],
            inputs: vec!["pa.o", "pc.o", "pd.o"],
            opts: vec![],
        });
    } else {
        v.push(Case {
            name: "rl0-6-property".into(),
            srcs: vec![
                src(
                    Cc::Gcc,
                    "xa.c",
                    &p.replace("PNAME", "xa"),
                    &["-O1", "-fcf-protection=full"],
                    "xa.o",
                ),
                src(
                    Cc::Gcc,
                    "xb.c",
                    &p.replace("PNAME", "xb"),
                    &["-O1", "-fcf-protection=full"],
                    "xb.o",
                ),
            ],
            inputs: vec!["xa.o", "xb.o"],
            opts: vec![],
        });
    }
    // rl0 7: SHF_GNU_RETAIN and plain pieces of one name.
    v.push(Case {
        name: "rl0-7-retain".into(),
        srcs: vec![
            src(
                Cc::As,
                "k1.s",
                &format!("  .section .text.k,\"axR\",%progbits\n  .globl k1\nk1:\n  {ret}\n"),
                NONE,
                "k1.o",
            ),
            src(
                Cc::As,
                "k2.s",
                &format!("  .section .text.k,\"ax\",%progbits\n  .globl k2\nk2:\n  {ret}\n"),
                NONE,
                "k2.o",
            ),
        ],
        inputs: vec!["k1.o", "k2.o"],
        opts: vec![],
    });
    // rl0 8: debug and .eh_frame records into a dropped COMDAT copy.
    v.push(Case {
        name: "rl0-8-dropdebug".into(),
        srcs: vec![
            src(
                Cc::Gxx,
                "d1.cc",
                &format!("{g}int use1(int x) {{ return shared_inline(x); }}\n"),
                &["-O0", "-g"],
                "d1.o",
            ),
            src(
                Cc::Gxx,
                "d2.cc",
                &format!("{g}int use2(int x) {{ return shared_inline(x) + 1; }}\n"),
                &["-O0", "-g"],
                "d2.o",
            ),
        ],
        inputs: vec!["d1.o", "d2.o"],
        opts: vec![],
    });
    // rl0 9: SHF_LINK_ORDER pieces linked to different sections.
    let l = "int LNAME(int x) { return x + 2; }\n";
    v.push(Case {
        name: "rl0-9-linkorder".into(),
        srcs: vec![
            src(
                Cc::Gcc,
                "l1.c",
                &l.replace("LNAME", "lf"),
                &["-O1", "-ffunction-sections", "-fpatchable-function-entry=2"],
                "l1.o",
            ),
            src(
                Cc::Gcc,
                "l2.c",
                &l.replace("LNAME", "lg"),
                &["-O1", "-ffunction-sections", "-fpatchable-function-entry=2"],
                "l2.o",
            ),
        ],
        inputs: vec!["l1.o", "l2.o"],
        opts: vec![],
    });
    // rl0 10: SHF_MERGE pieces of different sh_entsize.
    v.push(Case {
        name: "rl0-10-entsize".into(),
        srcs: vec![
            src(
                Cc::As,
                "z1.s",
                "  .section .rodata.cst,\"aM\",%progbits,4\n  .long 1\n",
                NONE,
                "z1.o",
            ),
            src(
                Cc::As,
                "z2.s",
                "  .section .rodata.cst,\"aM\",%progbits,8\n  .quad 2\n",
                NONE,
                "z2.o",
            ),
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
    // A versioned definition and a plain reference (RV-R6; A95's form).
    v.push(Case {
        name: "symver".into(),
        srcs: vec![
            src(
                Cc::Gcc,
                "sv1.c",
                "int foo_v1(int x) { return x; }\n__asm__(\".symver foo_v1, foo@@V1\");\n",
                O1,
                "sv1.o",
            ),
            src(
                Cc::Gcc,
                "sv2.c",
                "int foo(int);\nint bar(int x) { return foo(x); }\n",
                O1,
                "sv2.o",
            ),
        ],
        inputs: vec!["sv1.o", "sv2.o"],
        opts: vec![],
    });
    // GCC's type units: local `wi.` signatures defined in their groups (L-R3).
    v.push(Case {
        name: "debug-types".into(),
        srcs: vec![
            src(Cc::Gxx, "x1.cc", X1, &["-g", "-O0", "-fdebug-types-section"], "x1.o"),
            src(Cc::Gxx, "x2.cc", X2, &["-g", "-O0", "-fdebug-types-section"], "x2.o"),
        ],
        inputs: vec!["x1.o", "x2.o"],
        opts: vec![],
    });
    // An input that asks for an executable stack (L-R6a's X).
    v.push(Case {
        name: "execstack".into(),
        srcs: vec![
            src(Cc::As, "xs.s", &format!("  .section .note.GNU-stack,\"x\",%progbits\n  .text\n  .globl xs\nxs:\n  {ret}\n"), NONE, "xs.o"),
            src(Cc::Gcc, "xn.c", "int xn(int x) { return x; }\n", O1, "xn.o"),
        ],
        inputs: vec!["xs.o", "xn.o"],
        opts: vec![],
    });
    if t.arch != Arch::I386 {
        // Two objects with Mason containers (tests/meta, WP-078): P-P8.
        let arch = t.arch.name();
        v.push(Case {
            name: "meta".into(),
            srcs: vec![
                Src {
                    cc: Cc::Copy,
                    name: "a.o",
                    text: format!("tests/meta/{arch}/a.o"),
                    flags: NONE,
                    obj: "a.o",
                },
                Src {
                    cc: Cc::Copy,
                    name: "b.o",
                    text: format!("tests/meta/{arch}/b.o"),
                    flags: NONE,
                    obj: "b.o",
                },
            ],
            inputs: vec!["a.o", "b.o"],
            opts: vec![],
        });
    }
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
        if let Cc::Copy = s.cc {
            let from = Path::new(env!("CARGO_MANIFEST_DIR")).join(&s.text);
            std::fs::copy(&from, dir.join(s.obj))
                .map_err(|e| format!("{}: {e}", from.display()))?;
            continue;
        }
        std::fs::write(dir.join(s.name), &s.text).map_err(|e| e.to_string())?;
        let clang_args = t.arch.clang_args();
        let (prog, mut args): (PathBuf, Vec<String>) = match s.cc {
            Cc::Gcc | Cc::As => (t.gcc.clone(), Vec::new()),
            Cc::Copy => unreachable!(),
            Cc::Gxx => (t.gxx.clone(), Vec::new()),
            Cc::Clang | Cc::ClangAs => {
                let c = t.clang.clone().ok_or("clang")?;
                let c = if Path::new(s.name).extension().is_some_and(|x| x == "cc") {
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
    let text =
        std::fs::read_to_string(dir.join(format!("{}.{}.map", case.name, r.name()))).unwrap();
    let map: Map = mortar_plumb::map::parse(r.format(), &text).unwrap_or_else(|e| {
        panic!("{} {}: map line {}: {}", case.name, r.name(), e.line, e.message)
    });
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

#[allow(clippy::too_many_lines)] // build, link, verify, count, classify
fn references(arch: Arch) -> Option<Seen> {
    let t = Tools::find(arch)?;
    let start = Instant::now();
    let scratch = Scratch::new(&format!("plumb-rel-{}", arch.name()));
    let mut seen = Seen {
        anomalies: BTreeMap::new(),
        unclassified: Vec::new(),
        refused: Vec::new(),
        lines: Vec::new(),
    };
    let mut build_time = std::time::Duration::ZERO;
    let mut link_time = std::time::Duration::ZERO;
    let mut verify_time = std::time::Duration::ZERO;
    let mut totals: BTreeMap<&'static str, [u64; 5]> = BTreeMap::new();
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
                seen.refused.push(format!(
                    "{} mold: not run (mold 2.37.1 crashes on an IFUNC at -r)",
                    case.name
                ));
                continue;
            }
            let l = Instant::now();
            let linked = link(&t, r, &case, &dir);
            link_time += l.elapsed();
            let line = match linked {
                Ok(l) => l,
                Err(e) => {
                    seen.refused.push(format!(
                        "{} {}: {}",
                        case.name,
                        r.name(),
                        e.lines().next().unwrap_or("")
                    ));
                    continue;
                }
            };
            let v = Instant::now();
            let (report, out) = verify(r, &case, &dir, line);
            verify_time += v.elapsed();
            let rc = report.coverage.relocatable.clone().unwrap_or_default();
            seen.lines.push(format!(
                "{} {} [{}]: {} findings; records {}: verified {:?}, accepted {:?}, not checked {:?}; relocatable: pieces {}, merged {}, framed {}; groups {}; FDEs {} live, {} removed; symbols {}; sections {}",
                case.name,
                r.name(),
                arch.name(),
                report.findings.len(),
                report.coverage.records,
                report.coverage.verified,
                report.coverage.accepted,
                report.coverage.not_checked,
                rc.pieces,
                rc.merged,
                rc.framed,
                rc.groups,
                rc.fdes_live,
                rc.fdes_removed,
                rc.symbols,
                rc.sections
            ));
            let c = &report.coverage;
            assert_eq!(
                c.records,
                c.verified_total() + c.accepted_total() + c.not_checked_total() + c.failed,
                "{} {} [{}]: §7.2's sum",
                case.name,
                r.name(),
                arch.name()
            );
            let tot = totals.entry(r.name()).or_insert([0u64; 5]);
            tot[4] += c.failed;
            tot[0] += report.coverage.records;
            tot[1] += report.coverage.verified_total();
            tot[2] += report.coverage.accepted_total();
            tot[3] += report.coverage.not_checked_total();
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
                        f.site.as_ref().map_or_else(String::new, |s| format!(
                            " ({}:{}+{:#x})",
                            s.file, s.section, s.offset
                        ))
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
    for (r, [n, v, a, x, f]) in &totals {
        eprintln!(
            "relocatable [{}] {r}: records {n}: verified {v}, accepted {a}, not checked {x}, failed {f}",
            arch.name()
        );
    }
    Some(seen)
}

/// The named anomalies each reference shows, per case (measured with
/// GNU ld 2.44, lld 22.1.8 and mold 2.37.1; `spec/plumb.md` §10.2). A pair
/// that is not listed shows none: GNU ld shows none anywhere.
fn expected(arch: Arch) -> BTreeMap<(&'static str, &'static str), &'static [&'static str]> {
    const LLD: &str = "lld";
    const MOLD: &str = "mold";
    let i386 = arch == Arch::I386;
    let mut t: Vec<(&'static str, &'static str, &'static [&'static str])> = Vec::new();
    // What every target shows.
    t.extend([
        ("debug-cxx", LLD, &["A122", "A125", "A91"][..]),
        ("debug-cxx", MOLD, &["A121", "A122", "A124", "A97"][..]),
        ("none-overlap", LLD, &["A122", "A125", "A91"][..]),
        ("none-overlap", MOLD, &["A121", "A122"][..]),
        ("rl0-10-entsize", LLD, &["A100"][..]),
        ("rl0-14-dupmaps", MOLD, &["A99"][..]),
        ("rl0-7-retain", MOLD, &["A99"][..]),
        ("rl0-8-dropdebug", LLD, &["A125", "A91"][..]),
        ("rl0-9-linkorder", MOLD, &["A101"][..]),
        ("d3-cxx-gcc", MOLD, &["A121", "A122"][..]),
        ("d3-c-gcc", MOLD, &["A90"][..]),
        ("d3-cctors-gcc", MOLD, &["A123", "A90"][..]),
        ("debug-types", LLD, &["A122", "A125", "A126", "A91"][..]),
        ("debug-types", MOLD, &["A121", "A122", "A124", "A97"][..]),
        ("execstack", LLD, &["A93"][..]),
        ("symver", MOLD, &["A95"][..]),
    ]);
    if i386 {
        // gcc's `__x86.get_pc_thunk.*` COMDAT copies: lld leaves a
        // `NONE` record where it dropped their FDEs.
        t.extend([
            ("d3-c-gcc", LLD, &["A120", "A125", "A91"][..]),
            ("d3-cctors-gcc", LLD, &["A120", "A125", "A91"][..]),
            ("d3-cifunc-gcc", LLD, &["A120", "A122", "A125", "A91"][..]),
            ("d3-cxx-gcc", LLD, &["A122", "A125", "A91"][..]),
            ("rl0-8-dropdebug", MOLD, &["A121", "A97"][..]),
        ]);
    } else {
        t.extend([
            ("d3-c-gcc", LLD, &["A120"][..]),
            ("d3-cctors-gcc", LLD, &["A120"][..]),
            ("d3-cifunc-gcc", LLD, &["A120", "A122"][..]),
            ("d3-cxx-gcc", LLD, &["A122"][..]),
            ("d3-c-clang", LLD, &["A120"][..]),
            ("d3-c-clang", MOLD, &["A90"][..]),
            ("d3-cctors-clang", LLD, &["A120"][..]),
            ("d3-cctors-clang", MOLD, &["A123", "A90"][..]),
            ("d3-cifunc-clang", LLD, &["A120", "A122"][..]),
            ("d3-cxx-clang", MOLD, &["A121"][..]),
            ("rl0-13-cgprofile", MOLD, &["A102"][..]),
            ("rl0-8-dropdebug", MOLD, &["A97"][..]),
        ]);
    }
    if !i386 {
        t.push(("meta", LLD, &["A125", "A91"][..]));
    }
    if arch == Arch::Aarch64 {
        t.push(("rl0-3-groups", MOLD, &["A124", "A92"][..]));
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
    assert!(
        diffs.is_empty(),
        "[{}] anomalies differ from the table:\n{}",
        arch.name(),
        diffs.join("\n")
    );
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

// ---- the mutations (§9.2 (t), V-R12) ----

#[path = "relocatable/ed.rs"]
mod ed;
use ed::{Ed, R};

/// GNU ld's `-r` output of one case: what the mutations start from.
struct Base {
    name: String,
    dir: PathBuf,
    line: Vec<String>,
    out: Vec<u8>,
    map: Map,
    files: Vec<(String, Vec<u8>)>,
    /// The link in mortar's map form (`as_mortar_map`).
    mortar: String,
    mortar_map: Map,
}

impl Base {
    fn new(t: &Tools, case: &Case, dir: &Path) -> Base {
        std::fs::create_dir_all(dir).unwrap();
        build(t, case, dir).unwrap_or_else(|e| panic!("{}: {e}", case.name));
        let line = link(t, Ref::Bfd, case, dir).unwrap_or_else(|e| panic!("{}: {e}", case.name));
        let out = std::fs::read(dir.join(format!("{}.bfd.o", case.name))).unwrap();
        let text = std::fs::read_to_string(dir.join(format!("{}.bfd.map", case.name))).unwrap();
        let map = mortar_plumb::map::parse(Format::Gnu, &text).unwrap();
        let files: Vec<(String, Vec<u8>)> = map
            .paths()
            .into_iter()
            .filter_map(|p| std::fs::read(dir.join(p)).ok().map(|b| (p.to_string(), b)))
            .collect();
        let mortar = {
            let inputs: Vec<Input<'_>> =
                files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
            let src: &[Input<'_>] = &inputs;
            let ctx = Ctx { line: Some(line.clone()), files: &Dir(dir) };
            mortar_plumb::relocatable::as_mortar_map(&out, &src, &map, &ctx)
                .expect("the output reads")
        };
        let mortar_map = mortar_plumb::map::parse(Format::Mortar, &mortar).unwrap_or_else(|e| {
            panic!("{}: mortar form, line {}: {}\n{mortar}", case.name, e.line, e.message)
        });
        Base {
            name: case.name.clone(),
            dir: dir.to_path_buf(),
            line,
            out,
            map,
            files,
            mortar,
            mortar_map,
        }
    }

    /// The findings on `out`, as `rule message`, read with GNU ld's map or
    /// with mortar's form.
    fn findings(&self, out: &[u8], mortar: bool) -> (BTreeSet<String>, Report) {
        let inputs: Vec<Input<'_>> =
            self.files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
        let ctx = Ctx { line: Some(self.line.clone()), files: &Dir(&self.dir) };
        let map = if mortar { &self.mortar_map } else { &self.map };
        let r = mortar_plumb::verify_with(out, &inputs, map, &ctx);
        (r.findings.iter().map(|f| format!("{} {}", f.rule.id(), f.message)).collect(), r)
    }

    fn records(&self, kind: &str) -> Vec<Vec<String>> {
        self.mortar
            .lines()
            .map(|l| l.split(' ').map(str::to_string).collect::<Vec<_>>())
            .filter(|w| w[0] == kind)
            .collect()
    }

    /// `rplace`: (file, section, out, offset, size).
    fn pieces(&self) -> Vec<(usize, usize, usize, u64, u64)> {
        let h = |s: &str| u64::from_str_radix(s, 16).unwrap();
        self.records("rplace")
            .iter()
            .map(|w| {
                (
                    w[1].parse().unwrap(),
                    w[2].parse().unwrap(),
                    w[3].parse().unwrap(),
                    h(&w[4]),
                    h(&w[5]),
                )
            })
            .collect()
    }

    /// `rframe`: (file, section, input offset) -> output offset.
    fn frames(&self) -> BTreeMap<(usize, usize, u64), Option<u64>> {
        let h = |s: &str| u64::from_str_radix(s, 16).unwrap();
        self.records("rframe")
            .iter()
            .map(|w| {
                (
                    (w[1].parse().unwrap(), w[2].parse().unwrap(), h(&w[3])),
                    (w[4] != "-").then(|| h(&w[4])),
                )
            })
            .collect()
    }

    fn discarded(&self) -> BTreeSet<(usize, usize)> {
        self.records("discard")
            .iter()
            .map(|w| (w[1].parse().unwrap(), w[2].parse().unwrap()))
            .collect()
    }

    /// Input `f` of mortar's form.
    fn input(&self, f: usize) -> Ed {
        let w = self.records("file").into_iter().find(|w| w[1] == f.to_string()).unwrap();
        Ed::new(&std::fs::read(self.dir.join(&w[2])).unwrap())
    }
}

/// One mutant's result.
struct Mutant {
    id: String,
    base: String,
    what: String,
    want: Vec<&'static str>,
    /// The rules of the findings the mutant adds.
    rules: BTreeSet<String>,
    /// New findings that are a recorded anomaly (none may be).
    anomalies: Vec<String>,
    first: String,
}

impl Mutant {
    fn caught(&self) -> bool {
        self.want.iter().all(|w| self.rules.contains(*w)) && self.anomalies.is_empty()
    }
}

/// Applies `f` to `start` (else the base's output), and compares the
/// findings with those of `start`. `None` when `f` finds no site.
fn mutant(
    b: &Base,
    id: &str,
    want: &[&'static str],
    mortar: bool,
    start: Option<&[u8]>,
    f: impl FnOnce(&mut Ed) -> Option<String>,
) -> Option<Mutant> {
    let start = start.unwrap_or(&b.out);
    let mut e = Ed::new(start);
    let what = f(&mut e)?;
    let (before, _) = b.findings(start, mortar);
    let (after, report) = b.findings(&e.b, mortar);
    if std::env::var_os("RL5_DEBUG").is_some_and(|v| v == id) {
        eprintln!("DEBUG {id} before: {before:#?}\nDEBUG {id} after: {after:#?}");
    }
    let fresh: Vec<&String> = after.difference(&before).collect();
    let format = if mortar { Format::Mortar } else { Format::Gnu };
    let anomalies = report
        .findings
        .iter()
        .filter(|x| !before.contains(&format!("{} {}", x.rule.id(), x.message)))
        .filter_map(|x| {
            mortar_plumb::anomaly::classify(format, &e.b, x).map(|c| id_of(c).to_string())
        })
        .collect();
    Some(Mutant {
        id: id.to_string(),
        base: format!("{}{}", b.name, if mortar { " (as mortar's)" } else { "" }),
        what,
        want: want.to_vec(),
        rules: fresh.iter().map(|s| s.split(' ').next().unwrap_or("").to_string()).collect(),
        anomalies,
        first: fresh.first().map_or_else(String::new, |s| (*s).clone()),
    })
}

fn id_of(class: &str) -> &str {
    id(class)
}

/// The width of a data relocation's field, by type: 4 or 8 (0: not one).
fn data_width(arch: Arch, ty: u32) -> usize {
    match (arch, ty) {
        (Arch::Aarch64, 257) | (Arch::X86_64, 1) => 8,
        (Arch::Aarch64, 258) | (Arch::X86_64, 10 | 11) | (Arch::I386, 1) => 4,
        _ => 0,
    }
}

fn is_rel_text(e: &Ed, rs: usize) -> bool {
    let n = e.name(e.sh(rs).info as usize);
    n == ".text" || n.starts_with(".text.")
}

/// The mutations of one target.
#[allow(clippy::too_many_lines)] // one mutation after the other, as §9.2 (t) lists them
fn mutations(arch: Arch) -> Option<Vec<Mutant>> {
    let t = Tools::find(arch)?;
    let scratch = Scratch::new(&format!("plumb-relmut-{}", arch.name()));
    let all = cases(&t);
    let get = |name: &str, opts: &[&'static str]| -> Option<Base> {
        let c = all.iter().find(|c| c.name == name)?;
        let case = Case {
            name: if opts.is_empty() { c.name.clone() } else { format!("{}-unique", c.name) },
            srcs: c
                .srcs
                .iter()
                .map(|s| Src {
                    cc: s.cc,
                    name: s.name,
                    text: s.text.clone(),
                    flags: s.flags,
                    obj: s.obj,
                })
                .collect(),
            inputs: c.inputs.clone(),
            opts: opts.to_vec(),
        };
        Some(Base::new(&t, &case, &scratch.join(&case.name)))
    };
    let mut v: Vec<Mutant> = Vec::new();
    macro_rules! push {
        ($m:expr, $id:expr $(,)?) => {
            match $m {
                Some(m) => v.push(m),
                None => v.push(Mutant {
                    id: $id.to_string(),
                    base: String::new(),
                    what: "no site".into(),
                    want: vec!["(a site)"],
                    rules: BTreeSet::new(),
                    anomalies: Vec::new(),
                    first: String::new(),
                }),
            }
        };
    }
    let i386 = arch == Arch::I386;

    let c = get("d3-c-gcc", &[])?;
    // (1) a record's r_offset moved by 4.
    push!(
        mutant(&c, "(1)", &["P-P3"], false, None, |e| {
            let rs =
                e.reloc_secs().into_iter().find(|&r| is_rel_text(e, r) && !e.rels(r).is_empty())?;
            let mut rel = e.rels(rs);
            rel[0].offset += 4;
            e.set_rels(rs, &rel);
            Some(format!("{} record 0: r_offset + 4", e.name(rs)))
        }),
        "(1)",
    );
    // (2) a section-symbol record's addend without its piece offset.
    let pieces = c.pieces();
    push!(
        mutant(&c, "(2)", &["P-P3"], false, None, |e| {
            for rs in e.reloc_secs() {
                let tgt = e.sh(rs).info as usize;
                for (k, r) in e.rels(rs).into_iter().enumerate() {
                    let y = e.sym(r.sym as usize);
                    if y.ty() != ed::STT_SECTION || data_width(arch, r.ty) == 0 {
                        continue;
                    }
                    let o = y.shndx as usize;
                    let a = match r.addend {
                        Some(a) => a,
                        None => e.rd(e.sh(tgt).offset as usize + r.offset as usize, 4) as i64,
                    };
                    let Some(p) = pieces.iter().find(|p| {
                        p.2 == o && p.3 > 0 && (p.3 as i64) <= a && a < (p.3 + p.4) as i64
                    }) else {
                        continue;
                    };
                    let a2 = a - p.3 as i64;
                    if r.addend.is_some() {
                        let mut rel = e.rels(rs);
                        rel[k].addend = Some(a2);
                        e.set_rels(rs, &rel);
                        return Some(format!(
                            "{} record {k}: r_addend {a:#x} -> {a2:#x}",
                            e.name(rs)
                        ));
                    }
                    let at = e.sh(tgt).offset as usize + r.offset as usize;
                    e.wr(at, 4, a2 as u64);
                    return Some(format!("{} record {k}: the field {a:#x} -> {a2:#x}", e.name(rs)));
                }
            }
            None
        }),
        "(2)",
    );
    // (3) a piece's bytes zeroed; a piece written over its neighbour.
    push!(
        mutant(&c, "(3a)", &["P-P2"], false, None, |e| {
            let p = pieces.iter().find(|p| e.sh(p.2).flags & ed::SHF_EXECINSTR != 0 && p.4 > 0)?;
            let mut d = e.data(p.2);
            let r = p.3 as usize..(p.3 + p.4) as usize;
            if d[r.clone()].iter().all(|&x| x == 0) {
                return None;
            }
            d[r].fill(0);
            e.set_data(p.2, &d);
            Some(format!("section {} [{:#x}, +{:#x}) zeroed", e.name(p.2), p.3, p.4))
        }),
        "(3a)",
    );
    push!(
        mutant(&c, "(3b)", &["P-P2"], false, None, |e| {
            for a in &pieces {
                let Some(b) = pieces.iter().find(|b| b.2 == a.2 && b.3 > a.3 && b.4 > 0) else {
                    continue;
                };
                if a.4 == 0 || e.sh(a.2).ty == 8 {
                    continue;
                }
                let mut d = e.data(a.2);
                let n = a.4.min(b.4) as usize;
                let src = d[a.3 as usize..a.3 as usize + n].to_vec();
                if d[b.3 as usize..b.3 as usize + n] == src[..] {
                    continue;
                }
                d[b.3 as usize..b.3 as usize + n].copy_from_slice(&src);
                e.set_data(a.2, &d);
                return Some(format!(
                    "section {}: the piece at {:#x} written over the one at {:#x}",
                    e.name(a.2),
                    a.3,
                    b.3
                ));
            }
            None
        }),
        "(3b)",
    );
    // (5) a hidden global demoted to STB_LOCAL, moved below sh_info.
    push!(
        mutant(&c, "(5)", &["P-P4"], false, None, |e| {
            let k = (1..e.nsyms()).find(|&k| e.sym(k).other & 3 == 2 && e.sym(k).bind() != 0)?;
            let name = e.sym_name(k);
            let g = e.first_global();
            if k != g {
                e.swap_syms(k, g);
            }
            let mut y = e.sym(g);
            y.info &= 0xf;
            e.set_sym(g, &y);
            let st = e.symtab();
            let mut h = e.sh(st);
            h.info += 1;
            e.set_sh(st, &h);
            Some(format!("hidden {name} made STB_LOCAL, sh_info + 1"))
        }),
        "(5)",
    );
    // (6) a common's st_shndx set to SHN_ABS (mold's A90 form).
    push!(
        mutant(&c, "(6)", &["P-P4"], false, None, |e| {
            let k = (1..e.nsyms()).find(|&k| e.sym(k).shndx == 0xfff2)?;
            let mut y = e.sym(k);
            y.shndx = 0xfff1;
            e.set_sym(k, &y);
            Some(format!("common {}: SHN_ABS", e.sym_name(k)))
        }),
        "(6)",
    );
    // (9) an R_*_NONE record at a live record's offset, read as mortar's.
    push!(
        mutant(&c, "(9)", &["P-P3"], true, None, |e| {
            let rs =
                e.reloc_secs().into_iter().find(|&r| is_rel_text(e, r) && !e.rels(r).is_empty())?;
            let mut rel = e.rels(rs);
            let r0 = rel[0].clone();
            rel.push(R { offset: r0.offset, sym: 0, ty: 0, addend: r0.addend.map(|_| 0) });
            e.set_rels(rs, &rel);
            Some(format!("{}: a NONE record at {:#x}", e.name(rs), r0.offset))
        }),
        "(9)",
    );
    // (11) an opcode bit flipped under a branch or page record.
    let flips: &[(&str, u32)] = match arch {
        Arch::Aarch64 => &[("(11a) CALL26", 283), ("(11b) ADR_PREL_PG_HI21", 275)],
        Arch::X86_64 | Arch::I386 => &[("(11) PLT32/PC32 call", 4), ("(11) PC32 call", 2)],
    };
    let mut flipped = false;
    for &(id, ty) in flips {
        let m = mutant(&c, id, &["P-P2"], false, None, |e| {
            for rs in e.reloc_secs() {
                let tgt = e.sh(rs).info as usize;
                for r in e.rels(rs) {
                    if r.ty != ty {
                        continue;
                    }
                    let at = e.sh(tgt).offset as usize + r.offset as usize;
                    if arch == Arch::Aarch64 {
                        e.b[at + 3] ^= 0x80;
                        return Some(format!("{}+{:#x}: bit 31 flipped", e.name(tgt), r.offset));
                    }
                    if r.offset > 0 && e.b[at - 1] == 0xe8 {
                        e.b[at - 1] ^= 1;
                        return Some(format!("{}+{:#x}: call e8 -> e9", e.name(tgt), r.offset - 1));
                    }
                }
            }
            None
        });
        if arch == Arch::Aarch64 {
            push!(m, id);
        } else if let Some(m) = m
            && !flipped
        {
            v.push(m);
            flipped = true;
        }
    }
    assert!(arch == Arch::Aarch64 || flipped, "[{}] mutation (11): no call record", arch.name());
    // (20) e_type ET_EXEC; e_phnum 1; EI_OSABI flipped.
    push!(
        mutant(&c, "(20a) e_type", &["P-P9"], false, None, |e| {
            e.set_type(2);
            Some("e_type ET_EXEC".into())
        }),
        "(20a)",
    );
    push!(
        mutant(&c, "(20b) e_phnum", &["P-P7"], false, None, |e| {
            e.set_phnum(1);
            Some("e_phnum 1".into())
        }),
        "(20b)",
    );
    push!(
        mutant(&c, "(20c) EI_OSABI", &["P-P7"], false, None, |e| {
            e.b[7] ^= 3;
            Some(format!("EI_OSABI {}", e.b[7]))
        }),
        "(20c)",
    );

    // (4) a second copy of a COMDAT group kept.
    let x = get("d3-cxx-gcc", &[])?;
    push!(
        mutant(&x, "(4)", &["P-P6", "P-P11"], false, None, |e| {
            let g = (1..e.shnum())
                .find(|&i| e.sh(i).ty == ed::SHT_GROUP && e.data(i)[..4] == [1, 0, 0, 0])?;
            let d = e.data(g);
            let members: Vec<usize> = d[4..]
                .chunks(4)
                .map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]) as usize)
                .collect();
            let mut new: BTreeMap<usize, usize> = BTreeMap::new();
            let is_rel: Vec<bool> =
                members.iter().map(|&m| matches!(e.sh(m).ty, ed::SHT_RELA | ed::SHT_REL)).collect();
            for (&m, _) in members.iter().zip(&is_rel).filter(|(_, r)| !**r) {
                let (h, d) = (e.sh(m), e.data(m));
                new.insert(m, e.add(&h, &d));
            }
            for (&m, _) in members.iter().zip(&is_rel).filter(|(_, r)| **r) {
                let mut h = e.sh(m);
                h.info = *new.get(&(h.info as usize))? as u32;
                let d = e.data(m);
                new.insert(m, e.add(&h, &d));
            }
            let mut gd = vec![1u8, 0, 0, 0];
            for m in &members {
                gd.extend_from_slice(&(new[m] as u32).to_le_bytes());
            }
            let h = e.sh(g);
            e.add(&h, &gd);
            Some(format!("group section {g} copied with its {} members", members.len()))
        }),
        "(4)",
    );

    // (7) a .symver name stripped to its base.
    let sv = get("symver", &[])?;
    push!(
        mutant(&sv, "(7)", &["P-P4"], false, None, |e| {
            let k = (1..e.nsyms()).find(|&k| e.sym_name(k).contains("@@"))?;
            let name = e.sym_name(k);
            let at = e.sym_name_at(k) + name.find('@')?;
            e.b[at] = 0;
            Some(format!("{name} named {}", &name[..name.find('@')?]))
        }),
        "(7)",
    );

    // (8) the FDE of a dropped copy kept; (12) a tombstone written 0;
    // (14) a local $d left at a removed FDE's offset.
    let dbg = get("debug-cxx", &[])?;
    let frames = dbg.frames();
    push!(
        mutant(&dbg, "(8)", &["P-P5"], false, None, |e| {
            let (&(f, s, at), _) = frames.iter().find(|(_, o)| o.is_none())?;
            let inp = dbg.input(f);
            let d = inp.data(s);
            let len =
                u64::from(u32::from_le_bytes(d[at as usize..at as usize + 4].try_into().ok()?)) + 4;
            let ptr =
                u64::from(u32::from_le_bytes(d[at as usize + 4..at as usize + 8].try_into().ok()?));
            let cie_out = (*frames.get(&(f, s, at + 4 - ptr))?)?;
            let eh = e.find(".eh_frame")?;
            let mut od = e.data(eh);
            let pos = if od.ends_with(&[0, 0, 0, 0]) { od.len() - 4 } else { od.len() };
            let mut fde = d[at as usize..(at + len) as usize].to_vec();
            fde[4..8].copy_from_slice(&((pos as u64 + 4 - cie_out) as u32).to_le_bytes());
            od.splice(pos..pos, fde);
            e.set_data(eh, &od);
            // Its records, against the kept copy's section.
            let rin = inp.relocs_of(s).first().copied()?;
            let rout = *e.relocs_of(eh).first()?;
            let mut rel = e.rels(rout);
            for r in inp.rels(rin).into_iter().filter(|r| r.offset >= at && r.offset < at + len) {
                let y = inp.sym(r.sym as usize);
                let sym = if y.ty() == ed::STT_SECTION {
                    let name = inp.name(y.shndx as usize);
                    let o = e.find(&name)?;
                    e.section_sym(o)? as u32
                } else {
                    e.sym_by_name(&inp.sym_name(r.sym as usize))? as u32
                };
                rel.push(R {
                    offset: pos as u64 + (r.offset - at),
                    sym,
                    ty: r.ty,
                    addend: r.addend,
                });
            }
            e.set_rels(rout, &rel);
            Some(format!("the removed FDE {f}:{s}+{at:#x} copied to .eh_frame+{pos:#x}"))
        }),
        "(8)",
    );
    if !i386 {
        // The fields of the debug records into a dropped copy hold
        // mortar's tombstone first (I-47: all ones in .debug_info).
        let dead = dbg.discarded();
        let mut fields: Vec<(usize, u64, usize)> = Vec::new();
        for (f, _) in dbg.records("file").iter().enumerate() {
            let inp = dbg.input(f);
            for s in inp.all(".debug_info") {
                let Some(p) = dbg.pieces().into_iter().find(|p| p.0 == f && p.1 == s) else {
                    continue;
                };
                for rs in inp.relocs_of(s) {
                    for r in inp.rels(rs) {
                        let y = inp.sym(r.sym as usize);
                        let w = data_width(arch, r.ty);
                        if w > 0 && dead.contains(&(f, y.shndx as usize)) {
                            fields.push((p.2, p.3 + r.offset, w));
                        }
                    }
                }
            }
        }
        assert!(
            !fields.is_empty(),
            "[{}] mutation (12): no debug record into a dropped copy",
            arch.name()
        );
        let mut e = Ed::new(&dbg.out);
        for &(o, off, w) in &fields {
            let at = e.sh(o).offset as usize + off as usize;
            e.wr(at, w, u64::MAX);
        }
        let tomb = e.b.clone();
        push!(
            mutant(&dbg, "(12)", &["P-P2"], true, Some(&tomb), |e| {
                let (o, off, w) = fields[0];
                let at = e.sh(o).offset as usize + off as usize;
                e.wr(at, w, 0);
                Some(format!("{}+{off:#x}: the tombstone written 0", e.name(o)))
            }),
            "(12)",
        );
    }
    if arch == Arch::Aarch64 {
        push!(
            mutant(&dbg, "(14)", &["P-P4"], true, None, |e| {
                let eh = e.find(".eh_frame")?;
                // A removed FDE's input offset that no kept record has as
                // its output offset (else an input's own $d may be there).
                let outs: BTreeSet<u64> = frames.values().filter_map(|o| *o).collect();
                let (&(_, _, at), _) =
                    frames.iter().find(|((_, _, a), o)| o.is_none() && !outs.contains(a))?;
                let k = (1..e.first_global()).find(|&k| {
                    e.sym_name(k).starts_with("$d")
                        && e.sym(k).shndx as usize == eh
                        && e.sym(k).value != at
                })?;
                let mut y = e.sym(k);
                y.value = at;
                e.set_sym(k, &y);
                Some(format!("local $d of .eh_frame moved to {at:#x}"))
            }),
            "(14)",
        );
    }

    // (10) the container's bytes appended twice.
    if !i386 {
        let m = get("meta", &[])?;
        push!(
            mutant(&m, "(10)", &["P-P8"], false, None, |e| {
                let o = e.find(".mason.meta")?;
                let mut d = e.data(o);
                d.extend(d.clone());
                e.set_data(o, &d);
                Some(".mason.meta appended to itself".into())
            }),
            "(10)",
        );
    }

    // (13) a global's st_value without its piece offset.
    let maps = get("rl0-1-maps", &[])?;
    let mp = maps.pieces();
    push!(
        mutant(&maps, "(13)", &["P-P4"], false, None, |e| {
            for k in e.first_global()..e.nsyms() {
                let y = e.sym(k);
                let Some(p) = mp.iter().find(|p| {
                    p.2 == y.shndx as usize
                        && p.3 > 0
                        && p.3 <= y.value
                        && y.value < p.3 + p.4.max(1)
                }) else {
                    continue;
                };
                let mut y2 = y.clone();
                y2.value -= p.3;
                e.set_sym(k, &y2);
                return Some(format!(
                    "{}: st_value {:#x} -> {:#x}",
                    e.sym_name(k),
                    y.value,
                    y2.value
                ));
            }
            None
        }),
        "(13)",
    );

    // (15) on a --unique output, a record moved into the other .rela.text.
    // Its own case: two inputs whose .text both have records on every
    // target (rl0 case 1's m1.c has none on x86-64 and i386).
    let un = {
        let case = Case {
            name: "unique".into(),
            srcs: vec![
                src(
                    Cc::Gcc,
                    "u1.c",
                    "int ext(int);\nint f1(int x) { return ext(x) + 1; }\n",
                    O1,
                    "u1.o",
                ),
                src(
                    Cc::Gcc,
                    "u2.c",
                    "int ext(int);\nint f2(int x) { return ext(x) + 2; }\n",
                    O1,
                    "u2.o",
                ),
            ],
            inputs: vec!["u1.o", "u2.o"],
            opts: vec!["--unique=.text"],
        };
        Base::new(&t, &case, &scratch.join("unique"))
    };
    push!(
        mutant(&un, "(15)", &["P-P3"], false, None, |e| {
            let rss: Vec<usize> = e
                .reloc_secs()
                .into_iter()
                .filter(|&r| e.name(e.sh(r).info as usize) == ".text" && !e.rels(r).is_empty())
                .collect();
            let (a, b) = (*rss.first()?, *rss.get(1)?);
            let mut rb = e.rels(b);
            let r = rb.remove(0);
            let mut ra = e.rels(a);
            ra.push(r);
            e.set_rels(b, &rb);
            e.set_rels(a, &ra);
            Some(format!("record 0 of section {b} moved into section {a}"))
        }),
        "(15)",
    );

    // (16) sh_link of an SHF_LINK_ORDER section set to 0.
    let lo = get("rl0-9-linkorder", &[])?;
    push!(
        mutant(&lo, "(16)", &["P-P11"], false, None, |e| {
            let i = (1..e.shnum()).find(|&i| e.sh(i).flags & ed::SHF_LINK_ORDER != 0)?;
            let mut h = e.sh(i);
            h.link = 0;
            e.set_sh(i, &h);
            Some(format!("{}: sh_link 0", e.name(i)))
        }),
        "(16)",
    );
    // (17) SHF_GNU_RETAIN cleared on a section holding a retained piece.
    let rt = get("rl0-7-retain", &[])?;
    push!(
        mutant(&rt, "(17)", &["P-P11"], false, None, |e| {
            let i = (1..e.shnum()).find(|&i| e.sh(i).flags & ed::SHF_GNU_RETAIN != 0)?;
            let mut h = e.sh(i);
            h.flags &= !ed::SHF_GNU_RETAIN;
            e.set_sh(i, &h);
            Some(format!("{}: SHF_GNU_RETAIN cleared", e.name(i)))
        }),
        "(17)",
    );
    // (18) a type-unit group signature pointed at a member.
    let ty = get("debug-types", &[])?;
    push!(
        mutant(&ty, "(18)", &["P-P4"], false, None, |e| {
            let k = (1..e.first_global()).find(|&k| {
                let n = e.sym_name(k);
                (n.starts_with("wi.") || n.starts_with("wt."))
                    && e.sh(e.sym(k).shndx as usize).ty == ed::SHT_GROUP
            })?;
            let g = e.sym(k).shndx as usize;
            let d = e.data(g);
            let m = u32::from_le_bytes(d[4..8].try_into().ok()?);
            let mut y = e.sym(k);
            y.shndx = m as u16;
            e.set_sym(k, &y);
            Some(format!("{}: st_shndx {g} -> {m}", e.sym_name(k)))
        }),
        "(18)",
    );
    // (19) the .note.GNU-stack flags cleared over an X input.
    let xs = get("execstack", &[])?;
    push!(
        mutant(&xs, "(19)", &["P-P10"], false, None, |e| {
            let i = e.find(".note.GNU-stack")?;
            let mut h = e.sh(i);
            if h.flags & ed::SHF_EXECINSTR == 0 {
                return None;
            }
            h.flags = 0;
            e.set_sh(i, &h);
            Some(".note.GNU-stack: flags 0".into())
        }),
        "(19)",
    );
    // (22) an input R_*_NONE record dropped.
    let nn = get("none", &[])?;
    push!(
        mutant(&nn, "(22)", &["P-P3"], false, None, |e| {
            let rs = e.reloc_secs().into_iter().find(|&r| e.rels(r).iter().any(|r| r.ty == 0))?;
            let rel: Vec<R> = e.rels(rs).into_iter().filter(|r| r.ty != 0).collect();
            e.set_rels(rs, &rel);
            Some(format!("{}: its NONE record dropped ({} left)", e.name(rs), rel.len()))
        }),
        "(22)",
    );
    // Field by field: one mutant for every field the rules read that
    // (1)-(22) leave alone (brief WP-185 item 4), on d3-c-gcc and, for the
    // groups, d3-cxx-gcc.
    fields(arch, &c, &x, &mut v);
    Some(v)
}

const SHF_GROUP: u64 = 0x200;

/// A `.text` relocation section with two records or more.
fn text_rels(e: &Ed) -> Option<usize> {
    e.reloc_secs().into_iter().find(|&r| is_rel_text(e, r) && e.rels(r).len() >= 2)
}

/// A defined global function with a size.
fn global_func(e: &Ed) -> Option<usize> {
    (e.first_global()..e.nsyms()).find(|&k| {
        let y = e.sym(k);
        y.ty() == 2 && y.shndx != 0 && y.shndx < 0xff00 && y.size > 0
    })
}

/// An executable section with bytes.
fn text_sec(e: &Ed) -> Option<usize> {
    (1..e.shnum()).find(|&i| e.sh(i).flags & ed::SHF_EXECINSTR != 0 && e.sh(i).size > 0)
}

/// The first COMDAT group: (index, members).
fn comdat(e: &Ed) -> Option<(usize, Vec<usize>)> {
    let g = (1..e.shnum())
        .find(|&i| e.sh(i).ty == ed::SHT_GROUP && e.data(i).get(..4) == Some(&[1, 0, 0, 0][..]))?;
    let d = e.data(g);
    Some((
        g,
        d[4..].chunks(4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]) as usize).collect(),
    ))
}

/// The records of `.eh_frame`'s bytes: (offset, size, is a CIE).
fn eh_records(d: &[u8]) -> Vec<(usize, usize, bool)> {
    let mut v = Vec::new();
    let mut at = 0;
    while at + 8 <= d.len() {
        let len = u32::from_le_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]]) as usize;
        if len == 0 || len == 0xffff_ffff {
            break;
        }
        let id = u32::from_le_bytes([d[at + 4], d[at + 5], d[at + 6], d[at + 7]]);
        v.push((at, 4 + len, id == 0));
        at += 4 + len;
    }
    v
}

/// The field mutations (brief WP-185 item 4).
#[allow(clippy::too_many_lines)] // one mutant after the other
fn fields(arch: Arch, c: &Base, x: &Base, v: &mut Vec<Mutant>) {
    let mut push = |m: Option<Mutant>, id: &str| match m {
        Some(m) => v.push(m),
        None => v.push(Mutant {
            id: id.to_string(),
            base: String::new(),
            what: "no site".into(),
            want: vec!["(a site)"],
            rules: BTreeSet::new(),
            anomalies: Vec::new(),
            first: String::new(),
        }),
    };
    // Relocation records: dropped, symbol, type, addend, duplicated.
    push(
        mutant(c, "(f1) record dropped", &["P-P3"], false, None, |e| {
            let rs = text_rels(e)?;
            let mut rel = e.rels(rs);
            let r = rel.remove(0);
            e.set_rels(rs, &rel);
            Some(format!("{}: the record at {:#x} dropped", e.name(rs), r.offset))
        }),
        "(f1)",
    );
    push(
        mutant(c, "(f2) record symbol", &["P-P3"], false, None, |e| {
            let rs = text_rels(e)?;
            let mut rel = e.rels(rs);
            let was = rel[0].sym;
            // A defined symbol in another section: not the same place.
            let at = e.sym(was as usize).shndx;
            let k = (1..e.nsyms()).find(|&k| {
                let y = e.sym(k);
                y.shndx != 0
                    && y.shndx < 0xff00
                    && y.shndx != at
                    && y.ty() != 4
                    && y.ty() != ed::STT_SECTION
            })?;
            rel[0].sym = k as u32;
            e.set_rels(rs, &rel);
            Some(format!("{}: record 0's symbol {was} -> {k} ({})", e.name(rs), e.sym_name(k)))
        }),
        "(f2)",
    );
    push(
        mutant(c, "(f3) record type", &["P-P3"], false, None, |e| {
            let rs = text_rels(e)?;
            let mut rel = e.rels(rs);
            let abs = match arch {
                Arch::Aarch64 => 257,
                Arch::X86_64 | Arch::I386 => 1,
            };
            let k = rel.iter().position(|r| r.ty != abs && r.ty != 0)?;
            let was = rel[k].ty;
            rel[k].ty = abs;
            e.set_rels(rs, &rel);
            Some(format!("{}: record {k}'s type {was} -> {abs}", e.name(rs)))
        }),
        "(f3)",
    );
    push(
        mutant(c, "(f4) record addend", &["P-P3"], false, None, |e| {
            let rs = text_rels(e)?;
            let mut rel = e.rels(rs);
            if let Some(a) = rel[0].addend {
                rel[0].addend = Some(a + 4);
                e.set_rels(rs, &rel);
                return Some(format!("{}: record 0's r_addend {a:#x} + 4", e.name(rs)));
            }
            // REL: the addend is the field (i386's 32-bit fields).
            let tgt = e.sh(rs).info as usize;
            let at = e.sh(tgt).offset as usize + rel[0].offset as usize;
            let a = e.rd(at, 4);
            e.wr(at, 4, a.wrapping_add(4));
            Some(format!("{}: record 0's field {a:#x} + 4", e.name(rs)))
        }),
        "(f4)",
    );
    push(
        mutant(c, "(f5) record duplicated", &["P-P3"], false, None, |e| {
            let rs = text_rels(e)?;
            let mut rel = e.rels(rs);
            rel.push(rel[0].clone());
            e.set_rels(rs, &rel);
            Some(format!("{}: record 0 written twice", e.name(rs)))
        }),
        "(f5)",
    );
    // Symbols: size, binding, visibility, section, name, type; a section
    // symbol missing.
    let sym = |id: &str, f: fn(&mut ed::Sym, &Ed) -> String| {
        mutant(c, id, &["P-P4"], false, None, |e| {
            let k = global_func(e)?;
            let mut y = e.sym(k);
            let what = f(&mut y, e);
            e.set_sym(k, &y);
            Some(format!("{}: {what}", e.sym_name(k)))
        })
    };
    push(
        sym("(f6) st_size", |y, _| {
            y.size += 1;
            "st_size + 1".into()
        }),
        "(f6)",
    );
    push(
        sym("(f7) binding", |y, _| {
            y.info = (2 << 4) | (y.info & 0xf);
            "STB_WEAK".into()
        }),
        "(f7)",
    );
    push(
        sym("(f8) visibility", |y, _| {
            y.other = (y.other & !3) | 3;
            "STV_PROTECTED".into()
        }),
        "(f8)",
    );
    push(
        sym("(f9) st_shndx", |y, e| {
            let was = y.shndx;
            let o = (1..e.shnum())
                .find(|&i| i != was as usize && e.sh(i).flags & 2 != 0 && e.sh(i).ty == 1)
                .unwrap_or(1);
            y.shndx = o as u16;
            format!("st_shndx {was} -> {o}")
        }),
        "(f9)",
    );
    push(
        sym("(f10) st_type", |y, _| {
            y.info = (y.info & 0xf0) | 1;
            "STT_OBJECT".into()
        }),
        "(f10)",
    );
    push(
        mutant(c, "(f11) st_name", &["P-P4"], false, None, |e| {
            let k = global_func(e)?;
            let name = e.sym_name(k);
            let at = e.sym_name_at(k);
            e.b[at] = if e.b[at] == b'Z' { b'Y' } else { b'Z' };
            Some(format!("{name}: its name's first byte changed"))
        }),
        "(f11)",
    );
    push(
        mutant(c, "(f12) section symbol", &["P-P4"], false, None, |e| {
            let t = text_sec(e)?;
            let k = e.section_sym(t)?;
            let mut y = e.sym(k);
            y.info &= 0xf0;
            e.set_sym(k, &y);
            Some(format!("{}'s section symbol {k} made STT_NOTYPE", e.name(t)))
        }),
        "(f12)",
    );
    // .eh_frame: a live FDE dropped; a terminator added.
    push(
        mutant(c, "(f13) live FDE dropped", &["P-P5"], false, None, |e| {
            let eh = e.find(".eh_frame")?;
            let d = e.data(eh);
            let &(at, n, _) = eh_records(&d).iter().rev().find(|r| !r.2)?;
            let mut d2 = d[..at].to_vec();
            d2.extend_from_slice(&d[at + n..]);
            e.set_data(eh, &d2);
            for rs in e.relocs_of(eh) {
                let rel: Vec<R> = e
                    .rels(rs)
                    .into_iter()
                    .filter(|r| !(at as u64..(at + n) as u64).contains(&r.offset))
                    .map(|mut r| {
                        if r.offset >= (at + n) as u64 {
                            r.offset -= n as u64;
                        }
                        r
                    })
                    .collect();
                e.set_rels(rs, &rel);
            }
            Some(format!(".eh_frame: the FDE at {at:#x} ({n} bytes) and its records dropped"))
        }),
        "(f13)",
    );
    push(
        mutant(c, "(f14) terminator", &["P-P5"], true, None, |e| {
            // At the front, the records and their relocation records moved
            // by 4 (CIE pointers are relative): appended, it would fill
            // the 4 bytes of padding GNU ld trims on AArch64 and match
            // L-R6's size by chance.
            let eh = e.find(".eh_frame")?;
            let mut d = vec![0u8; 4];
            d.extend(e.data(eh));
            e.set_data(eh, &d);
            for rs in e.relocs_of(eh) {
                let rel: Vec<R> = e
                    .rels(rs)
                    .into_iter()
                    .map(|mut r| {
                        r.offset += 4;
                        r
                    })
                    .collect();
                e.set_rels(rs, &rel);
            }
            Some(".eh_frame: a zero terminator written before its first record".into())
        }),
        "(f14)",
    );
    // Groups: the flag word, the member list, the signature, a member
    // without SHF_GROUP, the group's sh_link and sh_entsize.
    push(
        mutant(x, "(f15) group flag word", &["P-P6"], false, None, |e| {
            let (g, _) = comdat(e)?;
            let mut d = e.data(g);
            d[0] = 0;
            e.set_data(g, &d);
            Some(format!("group {g}: GRP_COMDAT cleared"))
        }),
        "(f15)",
    );
    push(
        mutant(x, "(f16) group member list", &["P-P6"], false, None, |e| {
            let (g, m) = comdat(e)?;
            let mut d = e.data(g);
            d.truncate(d.len() - 4);
            e.set_data(g, &d);
            Some(format!("group {g}: its last member {} left out", m.last()?))
        }),
        "(f16)",
    );
    push(
        mutant(x, "(f17) group signature", &["P-P6"], false, None, |e| {
            let (g, _) = comdat(e)?;
            let mut h = e.sh(g);
            let was = h.info;
            let k = (1..e.nsyms() as u32)
                .rev()
                .find(|&k| k != was && e.sym(k as usize).ty() != ed::STT_SECTION)?;
            h.info = k;
            e.set_sh(g, &h);
            Some(format!("group {g}: sh_info {was} -> {k}"))
        }),
        "(f17)",
    );
    push(
        mutant(x, "(f18) member without SHF_GROUP", &["P-P6"], false, None, |e| {
            let (_, m) = comdat(e)?;
            let mut h = e.sh(m[0]);
            h.flags &= !SHF_GROUP;
            e.set_sh(m[0], &h);
            Some(format!("{}: SHF_GROUP cleared", e.name(m[0])))
        }),
        "(f18)",
    );
    push(
        mutant(x, "(f19) group sh_link", &["P-P11"], false, None, |e| {
            let (g, _) = comdat(e)?;
            let mut h = e.sh(g);
            h.link = 0;
            e.set_sh(g, &h);
            Some(format!("group {g}: sh_link 0"))
        }),
        "(f19)",
    );
    push(
        mutant(x, "(f20) group sh_entsize", &["P-P11"], false, None, |e| {
            let (g, _) = comdat(e)?;
            let mut h = e.sh(g);
            h.entsize = 8;
            e.set_sh(g, &h);
            Some(format!("group {g}: sh_entsize 8"))
        }),
        "(f20)",
    );
    // The header: e_machine, e_flags, e_entry.
    push(
        mutant(c, "(f21) e_machine", &["P-P7"], false, None, |e| {
            let was = e.rd(18, 2);
            let m = if was == 62 { 183 } else { 62 };
            e.wr(18, 2, m);
            Some(format!("e_machine {was} -> {m}"))
        }),
        "(f21)",
    );
    push(
        mutant(c, "(f22) e_flags", &["P-P7"], false, None, |e| {
            let at = if e.c64 { 48 } else { 36 };
            e.wr(at, 4, 1);
            Some("e_flags 1".into())
        }),
        "(f22)",
    );
    push(
        mutant(c, "(f23) e_entry", &["P-P7"], false, None, |e| {
            let w = if e.c64 { 8 } else { 4 };
            e.wr(24, w, 0x10);
            Some("e_entry 0x10".into())
        }),
        "(f23)",
    );
    // The notes: a property note no input has.
    push(
        mutant(c, "(f24) property note", &["P-P10"], false, None, |e| {
            if e.find(".note.gnu.property").is_some() {
                return None;
            }
            let (al, pr): (usize, u32) = match arch {
                Arch::Aarch64 => (8, 0xc000_0000),
                Arch::X86_64 => (8, 0xc000_0002),
                Arch::I386 => (4, 0xc000_0002),
            };
            let mut desc = Vec::new();
            desc.extend_from_slice(&pr.to_le_bytes());
            desc.extend_from_slice(&4u32.to_le_bytes());
            desc.extend_from_slice(&1u32.to_le_bytes());
            while desc.len() % al != 0 {
                desc.push(0);
            }
            let mut d = Vec::new();
            d.extend_from_slice(&4u32.to_le_bytes());
            d.extend_from_slice(&(desc.len() as u32).to_le_bytes());
            d.extend_from_slice(&5u32.to_le_bytes());
            d.extend_from_slice(b"GNU\0");
            d.extend_from_slice(&desc);
            let mut h = e.sh(1);
            h.name = e.add_name(".note.gnu.property");
            h.ty = 7;
            h.flags = 2;
            h.align = al as u64;
            (h.entsize, h.link, h.info, h.addr) = (0, 0, 0, 0);
            e.add(&h, &d);
            Some("an allocated property note (FEATURE_1_AND 1) added".into())
        }),
        "(f24)",
    );
    // Section headers, field by field.
    let hdr = |id: &str,
               want: &'static str,
               pick: fn(&Ed) -> Option<usize>,
               f: fn(&mut ed::Sh) -> String| {
        mutant(c, id, &[want], false, None, |e| {
            let i = pick(e)?;
            let mut h = e.sh(i);
            let what = f(&mut h);
            e.set_sh(i, &h);
            Some(format!("{}: {what}", e.name(i)))
        })
    };
    push(
        hdr("(f25) sh_type", "P-P11", text_sec, |h| {
            h.ty = 7;
            "sh_type SHT_NOTE".into()
        }),
        "(f25)",
    );
    push(
        hdr("(f26) sh_flags", "P-P11", text_sec, |h| {
            h.flags &= !ed::SHF_EXECINSTR;
            "SHF_EXECINSTR cleared".into()
        }),
        "(f26)",
    );
    push(
        hdr("(f27) sh_addralign", "P-P11", text_sec, |h| {
            h.align = if h.align > 1 { 1 } else { 16 };
            format!("sh_addralign {}", h.align)
        }),
        "(f27)",
    );
    push(
        hdr(
            "(f28) sh_entsize",
            "P-P11",
            |e| Some(e.symtab()),
            |h| {
                h.entsize = 1;
                "sh_entsize 1".into()
            },
        ),
        "(f28)",
    );
    push(
        hdr("(f29) sh_link", "P-P11", text_rels, |h| {
            h.link = 0;
            "sh_link 0".into()
        }),
        "(f29)",
    );
    push(
        hdr(
            "(f30) symtab sh_link",
            "P-P11",
            |e| Some(e.symtab()),
            |h| {
                h.link = 0;
                "sh_link 0".into()
            },
        ),
        "(f30)",
    );
    push(
        hdr("(f31) sh_info", "P-P3", text_rels, |h| {
            h.info = h.link;
            "sh_info .symtab".into()
        }),
        "(f31)",
    );
    push(
        hdr("(f32) sh_addr", "P-P7", text_sec, |h| {
            h.addr = 0x1000;
            "sh_addr 0x1000".into()
        }),
        "(f32)",
    );
    push(
        hdr("(f33) sh_size", "P-P2", text_sec, |h| {
            h.size = 0;
            "sh_size 0 (its pieces lost)".into()
        }),
        "(f33)",
    );
    push(
        mutant(c, "(f34) sh_name", &["P-P11"], false, None, |e| {
            let t = text_sec(e)?;
            let o = (1..e.shnum()).find(|&i| e.name(i) != e.name(t) && e.sh(i).ty == 1)?;
            let mut h = e.sh(t);
            h.name = e.sh(o).name;
            e.set_sh(t, &h);
            Some(format!("section {t}: sh_name of {}", e.name(o)))
        }),
        "(f34)",
    );
    push(
        mutant(c, "(f35) section duplicated", &["P-P11"], false, None, |e| {
            let t = text_sec(e)?;
            let (h, d) = (e.sh(t), e.data(t));
            e.add(&h, &d);
            Some(format!("{} appended a second time, without its records", e.name(t)))
        }),
        "(f35)",
    );
    // An SHF_MERGE section's entry size (the merged strings).
    push(
        mutant(c, "(f36) merge sh_entsize", &["P-P11"], false, None, |e| {
            let i = (1..e.shnum()).find(|&i| e.sh(i).flags & 0x10 != 0 && e.sh(i).entsize > 0)?;
            let mut h = e.sh(i);
            h.entsize *= 2;
            e.set_sh(i, &h);
            Some(format!("{}: sh_entsize {}", e.name(i), h.entsize))
        }),
        "(f36)",
    );
}

fn judge_mutations(arch: Arch) {
    let start = Instant::now();
    let Some(v) = mutations(arch) else { return };
    let mut survived = Vec::new();
    for m in &v {
        let state = if m.caught() { "caught" } else { "SURVIVED" };
        eprintln!(
            "mutation {} [{}] {}: {} -> {state} by {:?} (want {:?}){}; first: {}",
            m.id,
            arch.name(),
            m.base,
            m.what,
            m.rules,
            m.want,
            if m.anomalies.is_empty() {
                String::new()
            } else {
                format!(", anomalies {:?}", m.anomalies)
            },
            m.first
        );
        if !m.caught() {
            survived.push(m.id.clone());
        }
    }
    eprintln!(
        "mutations [{}]: {} applied, {} caught, {} survived ({:.1?})",
        arch.name(),
        v.len(),
        v.len() - survived.len(),
        survived.len(),
        start.elapsed()
    );
    assert!(survived.is_empty(), "[{}] mutations survived: {survived:?}", arch.name());
}

// spec: plumb.md §9.2 (t) — V-R12's mutations on AArch64.
#[test]
fn s9_2_t_mutations_aarch64() {
    judge_mutations(Arch::Aarch64);
}

// spec: plumb.md §9.2 (t) — V-R12's mutations on x86-64.
#[test]
fn s9_2_t_mutations_x86_64() {
    judge_mutations(Arch::X86_64);
}

// spec: plumb.md §9.2 (t) — V-R12's mutations on i386 (REL).
#[test]
fn s9_2_t_mutations_i386() {
    judge_mutations(Arch::I386);
}

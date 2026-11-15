//! The reference corpus (`spec/plumb.md` §9.1): programs compiled at test
//! time with gcc, g++, gfortran and clang-22, each linked by GNU ld, lld 22
//! and mold with a map file. Nothing binary is checked in.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use mortar_plumb::{Format, Input, Map, Report};
use mortar_testutil::{Scratch, tool};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Linker {
    Bfd,
    Lld,
    Mold,
}

impl Linker {
    pub const ALL: [Linker; 3] = [Linker::Bfd, Linker::Lld, Linker::Mold];

    pub fn name(self) -> &'static str {
        match self {
            Linker::Bfd => "bfd",
            Linker::Lld => "lld",
            Linker::Mold => "mold",
        }
    }

    pub fn format(self) -> Format {
        match self {
            Linker::Bfd => Format::Gnu,
            Linker::Lld => Format::Lld,
            Linker::Mold => Format::Mold,
        }
    }
}

/// One link of the corpus.
#[derive(Debug)]
pub struct Link {
    pub case: &'static str,
    pub linker: Linker,
    pub dir: PathBuf,
    pub output: PathBuf,
    pub map: PathBuf,
}

impl Link {
    pub fn output_bytes(&self) -> Vec<u8> {
        std::fs::read(&self.output).unwrap()
    }

    pub fn map_text(&self) -> String {
        std::fs::read_to_string(&self.map).unwrap()
    }

    pub fn parse_map(&self) -> Map {
        mortar_plumb::map::parse(self.linker.format(), &self.map_text())
            .unwrap_or_else(|e| panic!("{}: line {}: {}", self.map.display(), e.line, e.message))
    }

    /// The files the map names, read from the link's directory.
    pub fn inputs(&self, map: &Map) -> Vec<(String, Vec<u8>)> {
        map.paths()
            .into_iter()
            .filter_map(|p| {
                let path = self.dir.join(p);
                std::fs::read(&path).ok().map(|b| (p.to_string(), b))
            })
            .collect()
    }

    pub fn verify(&self) -> Report {
        let map = self.parse_map();
        verify_with(&self.output_bytes(), &self.inputs(&map), &map)
    }

    pub fn label(&self) -> String {
        format!("{}/{}", self.case, self.linker.name())
    }
}

pub fn verify_with(output: &[u8], files: &[(String, Vec<u8>)], map: &Map) -> Report {
    let inputs: Vec<Input<'_>> = files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
    mortar_plumb::verify(output, &inputs, map)
}

// ---- sources ----

/// A freestanding `_start`: a write and an exit, a TLS variable (so the
/// output has `PT_TLS`), a sized object.
pub const START_S: &str = r#"
	.text
	.globl _start
	.type _start,%function
_start:
	mov  x0, #1
	adrp x1, msg
	add  x1, x1, :lo12:msg
	adrp x2, len
	ldr  x2, [x2, :lo12:len]
	mov  x8, #64
	svc  #0
	mov  x0, #0
	mov  x8, #93
	svc  #0
	.size _start, .-_start
	.section .rodata
	.globl msg
	.type msg,%object
msg:	.ascii "hello\n"
	.size msg, 6
	.data
	.balign 8
	.globl len
	.type len,%object
len:	.quad 6
	.size len, 8
	.section .tdata,"awT",%progbits
	.globl tv
	.type tv,%object
tv:	.word 7
	.size tv, 4
	.section .note.mortar,"a",%note
	.balign 4
	.word 7, 4, 1
	.asciz "mortar"
	.balign 4
	.word 0
	.section .note.GNU-stack,"",%progbits
"#;

pub const HELLO_C: &str = r#"
#include <stdio.h>
#include <string.h>
static const char *names[] = { "zero", "one", "two" };
int counter;
int main(int argc, char **argv) {
    (void)argv;
    printf("hello %s %zu\n", names[argc % 3], strlen(names[argc % 3]));
    counter += argc;
    return counter > 100;
}
"#;

pub const EH_CC: &str = r#"
#include <iostream>
#include <stdexcept>
#include <string>
static int thrower(int x) {
    if (x > 0) throw std::runtime_error("boom " + std::to_string(x));
    return x;
}
int main(int argc, char **) {
    try {
        thrower(argc);
    } catch (const std::exception &e) {
        std::cout << "caught: " << e.what() << std::endl;
    }
    return 0;
}
"#;

pub const HELLO_F90: &str = r"
program hello
  implicit none
  integer :: i
  real :: s
  s = 0.0
  do i = 1, 10
    s = s + real(i) / 3.0
  end do
  print *, 'hello', s
end program hello
";

pub const GC_C: &str = r#"
#include <stdio.h>
int used_data = 3;
int unused_data = 4;
const char unused_str[] = "never referenced";
__attribute__((noinline)) int used(int x) { return x * used_data; }
__attribute__((noinline)) int unused(int x) { return x + unused_data + unused_str[x]; }
int main(int argc, char **argv) { (void)argv; printf("%d\n", used(argc)); return 0; }
"#;

/// COMDAT: the same inline function and template instance in two units.
pub const COMDAT_H: &str = r"
inline int twice(int x) { return 2 * x; }
template <typename T> struct Box { T v; T get() const { return v + twice(1); } };
";
pub const COMDAT_A: &str = r#"
#include "comdat.h"
int from_b(int);
int main(int argc, char **) { Box<int> b{argc}; return b.get() + from_b(argc) + twice(argc); }
"#;
pub const COMDAT_B: &str = r#"
#include "comdat.h"
int from_b(int x) { Box<int> b{x}; return b.get() + twice(x); }
"#;

/// A branch out of range of B/BL: the linker must add a veneer.
pub const FAR_S: &str = r#"
	.text
	.globl _start
	.type _start,%function
_start:
	bl farfunc
	b  farfunc
	mov x8, #93
	svc #0
	.section .fartext,"ax",%progbits
	.globl farfunc
	.type farfunc,%function
farfunc:
	bl _start
	ret
"#;

/// GOT-indirect references in the forms the linkers relax and keep.
pub const GOT_S: &str = r"
	.text
	.globl _start
	.type _start,%function
_start:
	adrp x0, :got:gvar
	ldr  x0, [x0, :got_lo12:gvar]
	adrp x1, :got:lvar
	ldr  x1, [x1, :got_lo12:lvar]
	adrp x2, :got:gfunc
	nop
	ldr  x2, [x2, :got_lo12:gfunc]
	adrp x3, :got:gvar
	ldr  x4, [x3, :got_lo12:gvar]
	adrp x5, :got:wundef
	ldr  x5, [x5, :got_lo12:wundef]
	bl   wundef
	b    wundef
	mov  x8, #93
	svc  #0
	.globl gfunc
	.type gfunc,%function
gfunc:	ret
	.weak wundef
	.data
	.balign 8
	.globl gvar
gvar:	.quad 1
lvar:	.quad 2
";

/// Erratum 843419: an ADRP at an address ending in 0xff8 followed by a
/// load and a load through its register.
pub const E843_S: &str = r"
	.text
	.globl _start
	.balign 4096
_start:
	.rept 1022
	nop
	.endr
	adrp x0, gvar
	ldr  x1, [x2]
	ldr  x3, [x0, :lo12:gvar]
	mov x8, #93
	svc #0
	.data
	.balign 8
	.globl gvar
gvar:	.quad 1, 2
";

/// The definitions the per-type files refer to.
pub const TYPES_DEFS_S: &str = r"
	.text
	.globl _start, func, near
	.type _start,%function
_start:	mov x8, #93
	svc #0
	.type func,%function
func:	ret
near:	ret
	.data
	.balign 16
	.globl gvar
gvar:	.quad 1, 2
	.globl small, mid, big, neg
	.set small, 0x1234
	.set mid, 0x12345678
	.set big, 0x123456789abc
	.set neg, -0x1234
";

/// The stage-1 types every reference linker accepts (§9.1).
pub const TYPES_ALL_S: &str = r"
	.text
	.globl t_all
t_all:
	movz x0, #:abs_g0:small
	movk x0, #:abs_g0_nc:gvar
	movz x0, #:abs_g1:mid
	movk x0, #:abs_g1_nc:gvar
	movz x0, #:abs_g2:big
	movk x0, #:abs_g2_nc:gvar
	movk x0, #:abs_g3:gvar
	ldr  x2, near
	adr  x2, near
	adrp x3, gvar
	adrp x3, :pg_hi21_nc:gvar
	add  x3, x3, :lo12:gvar
	ldrb w4, [x3, :lo12:gvar]
	ldrh w4, [x3, :lo12:gvar]
	ldr  w4, [x3, :lo12:gvar]
	ldr  x4, [x3, :lo12:gvar]
	ldr  q4, [x3, :lo12:gvar]
	b.eq near
	cbz  x5, near
	bl   func
	b    func
	adrp x6, :got:gvar
	ldr  x6, [x6, :got_lo12:gvar]
	ldr  x7, [x7, #:gotpage_lo15:gvar]
	ret
	.data
	.balign 8
	.quad gvar
	.quad gvar - .
	.word gvar - .
	.hword gvar - .
";

/// The types GNU ld and lld accept and mold refuses (§10).
pub const TYPES_MORE_S: &str = r"
	.text
	.globl t_more
t_more:
	movz x0, #:abs_g0_s:neg
	movz x0, #:abs_g1_s:neg
	movz x0, #:abs_g2_s:neg
	movz x1, #:prel_g0:near
	movk x1, #:prel_g0_nc:gvar
	movz x1, #:prel_g1:gvar
	movk x1, #:prel_g1_nc:gvar
	movz x1, #:prel_g2:gvar
	movk x1, #:prel_g2_nc:gvar
	movz x1, #:prel_g3:gvar
	tbz  x5, #3, near
	ldr  x7, :got:gvar
	ret
	.data
	.balign 4
	.word gvar
	.hword small
";

/// The type only GNU ld accepts (§10).
pub const TYPES_BFD_S: &str = r"
	.text
	.globl t_bfd
t_bfd:
	ldr x0, [x0, #:gotoff_lo15:gvar]
	ret
";

/// The types each reference linker is given, by name (§9.1, §10).
pub fn types_for(linker: Linker) -> Vec<&'static str> {
    let mut v = vec![
        "R_AARCH64_ABS64",
        "R_AARCH64_PREL64",
        "R_AARCH64_PREL32",
        "R_AARCH64_PREL16",
        "R_AARCH64_MOVW_UABS_G0",
        "R_AARCH64_MOVW_UABS_G0_NC",
        "R_AARCH64_MOVW_UABS_G1",
        "R_AARCH64_MOVW_UABS_G1_NC",
        "R_AARCH64_MOVW_UABS_G2",
        "R_AARCH64_MOVW_UABS_G2_NC",
        "R_AARCH64_MOVW_UABS_G3",
        "R_AARCH64_LD_PREL_LO19",
        "R_AARCH64_ADR_PREL_LO21",
        "R_AARCH64_ADR_PREL_PG_HI21",
        "R_AARCH64_ADR_PREL_PG_HI21_NC",
        "R_AARCH64_ADD_ABS_LO12_NC",
        "R_AARCH64_LDST8_ABS_LO12_NC",
        "R_AARCH64_LDST16_ABS_LO12_NC",
        "R_AARCH64_LDST32_ABS_LO12_NC",
        "R_AARCH64_LDST64_ABS_LO12_NC",
        "R_AARCH64_LDST128_ABS_LO12_NC",
        "R_AARCH64_CONDBR19",
        "R_AARCH64_JUMP26",
        "R_AARCH64_CALL26",
        "R_AARCH64_ADR_GOT_PAGE",
        "R_AARCH64_LD64_GOT_LO12_NC",
        "R_AARCH64_LD64_GOTPAGE_LO15",
    ];
    if linker != Linker::Mold {
        v.extend([
            "R_AARCH64_ABS32",
            "R_AARCH64_ABS16",
            "R_AARCH64_MOVW_SABS_G0",
            "R_AARCH64_MOVW_SABS_G1",
            "R_AARCH64_MOVW_SABS_G2",
            "R_AARCH64_MOVW_PREL_G0",
            "R_AARCH64_MOVW_PREL_G0_NC",
            "R_AARCH64_MOVW_PREL_G1",
            "R_AARCH64_MOVW_PREL_G1_NC",
            "R_AARCH64_MOVW_PREL_G2",
            "R_AARCH64_MOVW_PREL_G2_NC",
            "R_AARCH64_MOVW_PREL_G3",
            "R_AARCH64_TSTBR14",
            "R_AARCH64_GOT_LD_PREL19",
        ]);
    }
    if linker == Linker::Bfd {
        v.push("R_AARCH64_LD64_GOTOFF_LO15");
    }
    v
}

// ---- building ----

struct Tools {
    gcc: PathBuf,
    gxx: PathBuf,
    gfortran: PathBuf,
    clang: PathBuf,
    lld: PathBuf,
}

fn tools() -> Option<Tools> {
    let need = ["ld.bfd", "mold"];
    if need.iter().any(|t| tool(t).is_none()) {
        return None;
    }
    Some(Tools {
        gcc: tool("gcc")?,
        gxx: tool("g++")?,
        gfortran: tool("gfortran")?,
        clang: tool("clang-22")?,
        lld: tool("ld.lld-22")?,
    })
}

fn run_in(dir: &Path, program: &Path, args: &[&str]) {
    let mut c = Command::new(program);
    c.current_dir(dir).args(args);
    let out = mortar_testutil::run_with(&mut c, b"", mortar_testutil::DEFAULT_TIMEOUT);
    assert!(
        out.success(),
        "{} {} (in {}) failed:\n{}",
        program.display(),
        args.join(" "),
        dir.display(),
        out.stderr_text()
    );
}

/// What a case links.
struct Case {
    name: &'static str,
    driver: fn(&Tools) -> &Path,
    objects: Vec<&'static str>,
    flags: Vec<&'static str>,
    per_linker: fn(Linker) -> Vec<&'static str>,
}

fn none(_: Linker) -> Vec<&'static str> {
    Vec::new()
}

/// The corpus, built once per test binary (§9.1).
pub struct Corpus {
    _scratch: Scratch,
    pub links: Vec<Link>,
}

impl Corpus {
    pub fn get(&self, case: &str, linker: Linker) -> &Link {
        self.links.iter().find(|l| l.case == case && l.linker == linker).unwrap()
    }
}

/// Which cases to build: all, or the small freestanding ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    All,
    Small,
}

pub fn corpus(size: Size) -> Option<&'static Corpus> {
    static ALL: OnceLock<Option<Corpus>> = OnceLock::new();
    static SMALL: OnceLock<Option<Corpus>> = OnceLock::new();
    let cell = if size == Size::All { &ALL } else { &SMALL };
    cell.get_or_init(|| build(size)).as_ref()
}

#[allow(clippy::too_many_lines, clippy::items_after_statements)] // the corpus, case by case
fn build(size: Size) -> Option<Corpus> {
    let t = tools()?;
    let scratch = Scratch::new("plumb-corpus");
    let dir = scratch.path().to_path_buf();
    let w = |name: &str, text: &str| {
        scratch.write(name, text);
    };
    w("start.s", START_S);
    w("far.s", FAR_S);
    w("got.s", GOT_S);
    w("e843.s", E843_S);
    w("tdefs.s", TYPES_DEFS_S);
    w("tall.s", TYPES_ALL_S);
    w("tmore.s", TYPES_MORE_S);
    w("tbfd.s", TYPES_BFD_S);
    for (src, obj) in [
        ("start.s", "start.o"),
        ("far.s", "far.o"),
        ("got.s", "got.o"),
        ("e843.s", "e843.o"),
        ("tdefs.s", "tdefs.o"),
        ("tall.s", "tall.o"),
        ("tmore.s", "tmore.o"),
        ("tbfd.s", "tbfd.o"),
    ] {
        run_in(&dir, &t.gcc, &["-c", src, "-o", obj]);
    }
    if size == Size::All {
        w("hello.c", HELLO_C);
        w("eh.cc", EH_CC);
        w("hello.f90", HELLO_F90);
        w("gc.c", GC_C);
        w("comdat.h", COMDAT_H);
        w("comdat_a.cc", COMDAT_A);
        w("comdat_b.cc", COMDAT_B);
        run_in(&dir, &t.gcc, &["-O2", "-c", "hello.c", "-o", "hello.o"]);
        run_in(
            &dir,
            &t.clang,
            &["--target=aarch64-linux-gnu", "-O2", "-c", "hello.c", "-o", "hello-clang.o"],
        );
        run_in(&dir, &t.gxx, &["-O2", "-c", "eh.cc", "-o", "eh.o"]);
        run_in(&dir, &t.gfortran, &["-O2", "-c", "hello.f90", "-o", "hello-f.o"]);
        run_in(
            &dir,
            &t.gcc,
            &["-O2", "-ffunction-sections", "-fdata-sections", "-c", "gc.c", "-o", "gc.o"],
        );
        run_in(&dir, &t.gxx, &["-O0", "-c", "comdat_a.cc", "-o", "comdat_a.o"]);
        run_in(&dir, &t.gxx, &["-O0", "-c", "comdat_b.cc", "-o", "comdat_b.o"]);
    }
    // lld through the gcc driver: a directory with `ld.lld` for -B.
    let lldbin = dir.join("lldbin");
    std::fs::create_dir_all(&lldbin).unwrap();
    std::os::unix::fs::symlink(&t.lld, lldbin.join("ld.lld")).unwrap();

    fn gcc(t: &Tools) -> &Path {
        &t.gcc
    }
    fn gxx(t: &Tools) -> &Path {
        &t.gxx
    }
    fn gf(t: &Tools) -> &Path {
        &t.gfortran
    }
    let free = vec!["-static", "-nostdlib"];
    let mut cases = vec![
        Case {
            name: "start",
            driver: gcc,
            objects: vec!["start.o"],
            flags: free.clone(),
            per_linker: none,
        },
        Case {
            name: "veneer",
            driver: gcc,
            objects: vec!["far.o"],
            flags: free.clone(),
            per_linker: |l| match l {
                Linker::Bfd => vec!["-Wl,--section-start=.fartext=0x20000000"],
                _ => vec!["-Wl,-Ttext=0x400000", "-Wl,--section-start=.fartext=0x20000000"],
            },
        },
        Case {
            name: "got",
            driver: gcc,
            objects: vec!["got.o"],
            flags: free.clone(),
            per_linker: none,
        },
        Case {
            name: "erratum",
            driver: gcc,
            objects: vec!["e843.o"],
            flags: vec!["-static", "-nostdlib", "-Wl,--fix-cortex-a53-843419"],
            per_linker: none,
        },
        Case {
            name: "types",
            driver: gcc,
            objects: vec!["tdefs.o", "tall.o"],
            flags: free.clone(),
            per_linker: |l| match l {
                Linker::Bfd => vec!["tmore.o", "tbfd.o"],
                Linker::Lld => vec!["tmore.o"],
                Linker::Mold => vec![],
            },
        },
    ];
    if size == Size::All {
        cases.extend([
            Case {
                name: "c-hello",
                driver: gcc,
                objects: vec!["hello.o"],
                flags: vec!["-static"],
                per_linker: none,
            },
            Case {
                name: "c-hello-clang",
                driver: gcc,
                objects: vec!["hello-clang.o"],
                flags: vec!["-static"],
                per_linker: none,
            },
            Case {
                name: "cxx-eh",
                driver: gxx,
                objects: vec!["eh.o"],
                flags: vec!["-static"],
                per_linker: none,
            },
            Case {
                name: "fortran",
                driver: gf,
                objects: vec!["hello-f.o"],
                flags: vec!["-static"],
                per_linker: none,
            },
            Case {
                name: "gc-sections",
                driver: gcc,
                objects: vec!["gc.o"],
                flags: vec!["-static", "-Wl,--gc-sections"],
                per_linker: none,
            },
            Case {
                name: "comdat",
                driver: gxx,
                objects: vec!["comdat_a.o", "comdat_b.o"],
                flags: vec!["-static"],
                per_linker: none,
            },
        ]);
    }
    let mut links = Vec::new();
    for case in &cases {
        for linker in Linker::ALL {
            let out = format!("{}.{}", case.name, linker.name());
            let map = format!("{out}.map");
            let mut args: Vec<String> = Vec::new();
            match linker {
                Linker::Bfd => args.push("-fuse-ld=bfd".into()),
                Linker::Lld => {
                    args.extend(["-B".into(), lldbin.display().to_string(), "-fuse-ld=lld".into()]);
                }
                Linker::Mold => args.push("-fuse-ld=mold".into()),
            }
            args.extend(case.flags.iter().map(|s| (*s).to_string()));
            args.extend(case.objects.iter().map(|s| (*s).to_string()));
            args.extend((case.per_linker)(linker).iter().map(|s| (*s).to_string()));
            args.extend(["-o".into(), out.clone(), format!("-Wl,-Map,{map}")]);
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            run_in(&dir, (case.driver)(&t), &refs);
            links.push(Link {
                case: case.name,
                linker,
                dir: dir.clone(),
                output: dir.join(&out),
                map: dir.join(&map),
            });
        }
    }
    Some(Corpus { _scratch: scratch, links })
}

// ---- known anomalies of the reference linkers (§10.2) ----

/// Whether a finding on a reference link is a reference linker's own
/// defect that `spec/plumb.md` §10.2 records, by name. Everything else
/// fails the corpus test.
pub fn known_anomaly(link: &Link, f: &mortar_plumb::Finding) -> Option<&'static str> {
    if f.rule == mortar_plumb::Rule::GotSlot
        && f.symbol.as_deref().is_some_and(|s| s.starts_with('.'))
        && f.found.zip(f.expected).is_some_and(|(found, want)| found < want)
    {
        return Some(
            "A6: GNU ld, lld and mold fill the GOT slot of a section symbol plus an addend without the addend",
        );
    }
    if link.linker != Linker::Mold || f.rule != mortar_plumb::Rule::Structure(8) {
        return None;
    }
    let sym = f.symbol.as_deref()?;
    if sym == "_TLS_MODULE_BASE_" {
        return Some("A3: mold's _TLS_MODULE_BASE_ is an address, or there is no PT_TLS");
    }
    let addr = f.addr?;
    if addr >= 1 << 32 {
        return Some("A2: mold gives an alias of an IFUNC the value 2^32 + x");
    }
    let bytes = link.output_bytes();
    let h = mortar_elf::FileHeader::parse(&bytes).ok()?;
    let table = mortar_elf::SectionTable::parse(&bytes, &h).ok()?;
    let in_plt = table.iter().any(|s| {
        s.name().is_ok_and(|n| n == b".plt.got" || n == b".iplt" || n == b".plt")
            && (s.header().sh_addr..s.header().sh_addr + s.header().sh_size).contains(&addr)
    });
    in_plt.then_some(
        "A1: mold's canonical IFUNC symbol is in .plt.got but names .text with the resolver's size",
    )
}

/// The findings of a report on `link` that are not recorded anomalies.
pub fn unexpected<'r>(link: &Link, r: &'r Report) -> Vec<&'r mortar_plumb::Finding> {
    r.findings.iter().filter(|f| known_anomaly(link, f).is_none()).collect()
}

/// Asserts that a report on `link` has no finding but recorded anomalies.
pub fn assert_clean(link: &Link, r: &Report) {
    assert!(
        unexpected(link, r).is_empty(),
        "{}:\n{}{}",
        link.label(),
        r.render(&link.label()),
        r.summary()
    );
}

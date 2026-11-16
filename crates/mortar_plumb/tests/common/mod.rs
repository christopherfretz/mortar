//! The reference corpus (`spec/plumb.md` §9.1): programs compiled at test
//! time with gcc, g++, gfortran and clang-22, each linked by GNU ld, lld 22
//! and mold with a map file. Nothing binary is checked in.
//!
//! The corpus is AArch64 (`spec/harness.md` "A test's subject decides its
//! tools"): the AArch64 GNU tools by [`Arch::gnu`]'s names, clang with
//! `--target`, and the programs run natively on an AArch64 host and under
//! qemu elsewhere.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use mortar_plumb::{Format, Input, Map, Report};
use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, tool};

/// The architecture of the corpus.
pub const ARCH: Arch = Arch::Aarch64;

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

    /// Runs the output with no arguments: its exit status, `None` if it
    /// ended by a signal or did not end in time.
    pub fn run(&self) -> Option<i32> {
        let mut c = ARCH.command(&self.output, None);
        c.current_dir(&self.dir);
        let out = mortar_testutil::run_with(&mut c, b"", mortar_testutil::DEFAULT_TIMEOUT);
        out.status.and_then(|s| s.code())
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

/// A BTI-marked object (WP-028d): calls beyond B/BL's reach to a target
/// that begins with a landing pad (`padded`) and to one that does not
/// (`padless`), and an IFUNC whose IPLT entry begins with `bti c` (GNU
/// ld's and lld's form in a BTI output) reached by a call and a GOT load.
/// GNU ld reaches `padless` through `__padless_veneer` and
/// `__padless_bti_veneer` (`bti c; b padless`), lld 22 through a
/// `__AArch64BTIThunk_padless` landing pad that falls through to it;
/// mold writes no property note on AArch64 (§6.6).
pub const BTI_S: &str = r#"
	.text
	.globl _start
	.type _start,%function
_start:
	bti c
	bl padded
	bl padless
	bl f
	adrp x1, :got:f
	ldr x1, [x1, :got_lo12:f]
	blr x1
	mov x8, #93
	svc #0
	.globl f
	.type f, %gnu_indirect_function
f:	bti c
	adr x0, impl
	ret
impl:	bti c
	mov x0, #42
	ret
	.section .fartext,"ax",%progbits
	.globl padded
	.type padded,%function
padded:	bti c
	ret
	.globl padless
	.type padless,%function
padless:	nop
	ret
	.section .note.gnu.property,"a"
	.p2align 3
	.word 4, 16, 5
	.asciz "GNU"
	.word 0xc0000000, 4, 1, 0
	.section .note.GNU-stack,"",@progbits
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
/// Two units sharing strings of a mergeable section (`synth.md` S-17,
/// S-19), referred to by section symbol plus addend and by a local label
/// plus addend. No string is a suffix of another, so each has one place.
pub const PIECES_A_S: &str = r#"
	.text
	.globl _start
	.type _start,%function
_start:
	adrp x0, .rodata.str1.1+4
	add  x0, x0, :lo12:.rodata.str1.1+4
	adrp x1, .Lpb+2
	add  x1, x1, :lo12:.Lpb+2
	bl   other
	mov  x0, #0
	mov  x8, #93
	svc  #0
	.size _start, .-_start
	.section .rodata.str1.1,"aMS",%progbits,1
.Lpa:	.asciz "alpha"
.Lpb:	.asciz "bravo!"
	.asciz "charlie"
	.data
	.balign 8
	.xword .rodata.str1.1+6
	.xword .rodata.str1.1+13
	.xword .Lpa+1
"#;

pub const PIECES_B_S: &str = r#"
	.text
	.globl other
	.type other,%function
other:
	adrp x2, .rodata.str1.1+8
	add  x2, x2, :lo12:.rodata.str1.1+8
	ret
	.size other, .-other
	.section .rodata.str1.1,"aMS",%progbits,1
	.asciz "zulu"
	.asciz "charlie"
	.asciz "yankee"
	.asciz "alpha"
	.data
	.balign 8
	.xword .rodata.str1.1+0
	.xword .rodata.str1.1+5
	.xword .rodata.str1.1+20
	.xword .rodata.str1.1+14
"#;

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
    // GNU ld through the driver: the AArch64 one's `-fuse-ld=bfd`.
    if ARCH.tool("ld.bfd").is_none() || tool("mold").is_none() {
        return None;
    }
    Some(Tools {
        gcc: ARCH.tool("gcc")?,
        gxx: ARCH.tool("g++")?,
        gfortran: ARCH.tool("gfortran")?,
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
    /// The linkers that link it (lld refuses the traditional TLS forms).
    linkers: &'static [Linker],
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
    /// The stage-2 cases: thread-local storage (§4.7), call frame
    /// information (§6.4) and the review's cases (§10.2).
    Tls,
}

pub fn corpus(size: Size) -> Option<&'static Corpus> {
    static ALL: OnceLock<Option<Corpus>> = OnceLock::new();
    static SMALL: OnceLock<Option<Corpus>> = OnceLock::new();
    static TLS: OnceLock<Option<Corpus>> = OnceLock::new();
    let cell = match size {
        Size::All => &ALL,
        Size::Small => &SMALL,
        Size::Tls => &TLS,
    };
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
    w("pieces_a.s", PIECES_A_S);
    w("pieces_b.s", PIECES_B_S);
    w("bti.s", BTI_S);
    for (src, obj) in if size == Size::Tls {
        &[][..]
    } else {
        &[
            ("start.s", "start.o"),
            ("far.s", "far.o"),
            ("got.s", "got.o"),
            ("e843.s", "e843.o"),
            ("tdefs.s", "tdefs.o"),
            ("tall.s", "tall.o"),
            ("tmore.s", "tmore.o"),
            ("tbfd.s", "tbfd.o"),
            ("pieces_a.s", "pieces_a.o"),
            ("pieces_b.s", "pieces_b.o"),
            ("bti.s", "bti.o"),
        ][..]
    } {
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
    let mut cases = if size == Size::Tls {
        tls_cases(&t, &scratch)
    } else {
        vec![
            Case {
                name: "start",
                linkers: &Linker::ALL,
                driver: gcc,
                objects: vec!["start.o"],
                flags: free.clone(),
                per_linker: none,
            },
            Case {
                name: "veneer",
                linkers: &Linker::ALL,
                driver: gcc,
                objects: vec!["far.o"],
                flags: free.clone(),
                per_linker: |l| match l {
                    Linker::Bfd => vec!["-Wl,--section-start=.fartext=0x20000000"],
                    _ => vec!["-Wl,-Ttext=0x400000", "-Wl,--section-start=.fartext=0x20000000"],
                },
            },
            Case {
                name: "bti",
                linkers: &Linker::ALL,
                driver: gcc,
                objects: vec!["bti.o"],
                flags: free.clone(),
                per_linker: |l| match l {
                    Linker::Bfd => vec!["-Wl,--section-start=.fartext=0x20000000"],
                    _ => vec!["-Wl,-Ttext=0x400000", "-Wl,--section-start=.fartext=0x20000000"],
                },
            },
            Case {
                name: "got",
                linkers: &Linker::ALL,
                driver: gcc,
                objects: vec!["got.o"],
                flags: free.clone(),
                per_linker: none,
            },
            Case {
                name: "erratum",
                linkers: &Linker::ALL,
                driver: gcc,
                objects: vec!["e843.o"],
                flags: vec!["-static", "-nostdlib", "-Wl,--fix-cortex-a53-843419"],
                per_linker: none,
            },
            Case {
                name: "types",
                linkers: &Linker::ALL,
                driver: gcc,
                objects: vec!["tdefs.o", "tall.o"],
                flags: free.clone(),
                per_linker: |l| match l {
                    Linker::Bfd => vec!["tmore.o", "tbfd.o"],
                    Linker::Lld => vec!["tmore.o"],
                    Linker::Mold => vec![],
                },
            },
            Case {
                name: "pieces",
                linkers: &Linker::ALL,
                driver: gcc,
                objects: vec!["pieces_a.o", "pieces_b.o"],
                flags: free.clone(),
                per_linker: none,
            },
        ]
    };
    if size == Size::All {
        cases.extend([
            Case {
                name: "c-hello",
                linkers: &Linker::ALL,
                driver: gcc,
                objects: vec!["hello.o"],
                flags: vec!["-static"],
                per_linker: none,
            },
            Case {
                name: "c-hello-clang",
                linkers: &Linker::ALL,
                driver: gcc,
                objects: vec!["hello-clang.o"],
                flags: vec!["-static"],
                per_linker: none,
            },
            Case {
                name: "cxx-eh",
                linkers: &Linker::ALL,
                driver: gxx,
                objects: vec!["eh.o"],
                flags: vec!["-static"],
                per_linker: none,
            },
            Case {
                name: "cxx-eh-hdr",
                linkers: &Linker::ALL,
                driver: gxx,
                objects: vec!["eh.o"],
                flags: vec!["-static", "-Wl,--eh-frame-hdr"],
                per_linker: none,
            },
            Case {
                name: "fortran",
                linkers: &Linker::ALL,
                driver: gf,
                objects: vec!["hello-f.o"],
                flags: vec!["-static"],
                per_linker: none,
            },
            Case {
                name: "gc-sections",
                linkers: &Linker::ALL,
                driver: gcc,
                objects: vec!["gc.o"],
                flags: vec!["-static", "-Wl,--gc-sections"],
                per_linker: none,
            },
            Case {
                name: "comdat",
                linkers: &Linker::ALL,
                driver: gxx,
                objects: vec!["comdat_a.o", "comdat_b.o"],
                flags: vec!["-static"],
                per_linker: none,
            },
        ]);
    }
    let mut links = Vec::new();
    for case in &cases {
        for &linker in case.linkers {
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

// ---- the thread-local corpus (§4.7, §10.2) ----

/// Thread-local variables of three alignments, local-exec in the unit
/// that defines them (gcc, not PIC).
pub const TLS_DEF_C: &str = r"
__thread long t_a = 1;
__thread long t_b __attribute__((aligned(64))) = 2;
__thread int t_c;
static __thread volatile long t_s = 5;
long le_sum(void) { t_c = 3; return t_a + t_b + t_c + t_s; }
";

/// Initial-exec references (gcc, not PIC, external variables).
pub const TLS_IE_C: &str = r"
extern __thread long t_a, t_b;
extern __thread int t_c;
long ie_sum(void) { return t_a + t_b + t_c; }
";

/// General- and local-dynamic references: compiled `-fPIC` as TLSDESC
/// (gcc, clang) and as the traditional sequences (gcc).
pub const TLS_DYN_C: &str = r"
extern __thread long t_a, t_b;
extern __thread int t_c;
static __thread volatile long d_s = 7;
static __thread volatile long d_t __attribute__((aligned(32))) = 8;
long SUM(void) { return t_a + t_b + t_c + d_s + d_t; }
";

pub const TLS_MAIN_C: &str = r"
long le_sum(void), ie_sum(void), desc_sum(void), clang_sum(void);
#ifdef TRAD
long trad_sum(void);
#endif
int main(void) {
  int bad = 0;
  if (le_sum() != 11) bad |= 1;
  if (ie_sum() != 6) bad |= 2;
  if (desc_sum() != 21) bad |= 4;
  if (clang_sum() != 21) bad |= 8;
#ifdef TRAD
  if (trad_sum() != 21) bad |= 16;
#endif
  return bad;
}
";

/// The TLS types every reference linker accepts, and the variables they
/// name (`.tdata` aligned to 16, so the TCB gap is 16 for all three).
pub const TLS_TYPES_ALL_S: &str = r#"
	.text
	.globl _start
	.type _start,%function
_start:
	mrs  x0, tpidr_el0
	add  x1, x0, #:tprel_hi12:le1, lsl #12
	add  x1, x1, #:tprel_lo12_nc:le1
	movz x2, #:tprel_g2:le2
	movk x2, #:tprel_g1_nc:le2
	movk x2, #:tprel_g0_nc:le2
	movz x2, #:tprel_g1:le1
	movk x2, #:tprel_g0_nc:le1
	movz x2, #:tprel_g0:le1
	adrp x3, :gottprel:ie1
	ldr  x3, [x3, #:gottprel_lo12:ie1]
	adrp x0, :tlsdesc:gd1
	ldr  x1, [x0, #:tlsdesc_lo12:gd1]
	add  x0, x0, #:tlsdesc_lo12:gd1
	.tlsdesccall gd1
	blr  x1
	mov x8, #93
	svc #0
	.globl __tls_get_addr
	.type __tls_get_addr,%function
__tls_get_addr: ret
	.section .tdata,"awT",%progbits
	.balign 16
	.globl le1, le2, ie1, gd1, gd2, ld1, ie3
le1: .quad 1
le2: .quad 2
ie1: .quad 3
gd1: .quad 4
gd2: .quad 5
ld1: .quad 6
ie3: .quad 7
	.section .tbss,"awT",%nobits
	.balign 16
tb: .zero 32
"#;

/// The checked `ADD` form and the traditional general-dynamic sequence:
/// GNU ld and mold.
pub const TLS_TYPES_NOT_LLD_S: &str = r"
	.text
	.globl t_not_lld
t_not_lld:
	add  x12, x0, #:tprel_lo12:le1
	adrp x0, :tlsgd:gd2
	add  x0, x0, :tlsgd_lo12:gd2
	bl   __tls_get_addr
	nop
	ret
";

/// The unchecked load/store forms: GNU ld and lld.
pub const TLS_TYPES_NOT_MOLD_S: &str = r"
	.text
	.globl t_not_mold
t_not_mold:
	mrs  x9, tpidr_el0
	ldrb w10, [x9, #:tprel_lo12_nc:le1]
	ldrh w10, [x9, #:tprel_lo12_nc:le1]
	ldr  w10, [x9, #:tprel_lo12_nc:le1]
	ldr  x10, [x9, #:tprel_lo12_nc:le1]
	ret
";

/// What only GNU ld accepts: the checked load/store forms, the literal
/// and MOVW initial-exec forms, local-dynamic with every DTPREL form, and
/// a TLSDESC of `_TLS_MODULE_BASE_` (review finding 6).
pub const TLS_TYPES_BFD_S: &str = r"
	.text
	.globl t_bfd
t_bfd:
	mrs  x9, tpidr_el0
	ldrb w10, [x9, #:tprel_lo12:le1]
	ldrh w10, [x9, #:tprel_lo12:le1]
	ldr  w10, [x9, #:tprel_lo12:le1]
	ldr  x10, [x9, #:tprel_lo12:le1]
	ldr  x6, :gottprel:ie3
	movz x7, #:gottprel_g1:ie3
	movk x7, #:gottprel_g0_nc:ie3
	adrp x0, :tlsldm:ld1
	add  x0, x0, :tlsldm_lo12_nc:ld1
	bl   __tls_get_addr
	nop
	add  x1, x0, #:dtprel_hi12:ld1, lsl #12
	add  x1, x1, #:dtprel_lo12_nc:ld1
	add  x1, x0, #:dtprel_lo12:ld1
	movz x2, #:dtprel_g2:ld1
	movk x2, #:dtprel_g1_nc:ld1
	movk x2, #:dtprel_g0_nc:ld1
	movz x2, #:dtprel_g1:ld1
	movz x2, #:dtprel_g0:ld1
	ldrb w3, [x0, #:dtprel_lo12:ld1]
	ldrb w3, [x0, #:dtprel_lo12_nc:ld1]
	ldrh w3, [x0, #:dtprel_lo12:ld1]
	ldrh w3, [x0, #:dtprel_lo12_nc:ld1]
	ldr  w3, [x0, #:dtprel_lo12:ld1]
	ldr  w3, [x0, #:dtprel_lo12_nc:ld1]
	ldr  x3, [x0, #:dtprel_lo12:ld1]
	ldr  x3, [x0, #:dtprel_lo12_nc:ld1]
	adrp x0, :tlsdesc:_TLS_MODULE_BASE_
	ldr  x1, [x0, #:tlsdesc_lo12:_TLS_MODULE_BASE_]
	add  x0, x0, #:tlsdesc_lo12:_TLS_MODULE_BASE_
	.tlsdesccall _TLS_MODULE_BASE_
	blr  x1
	mrs  x9, tpidr_el0
	add  x0, x0, x9
	add  x0, x0, #:dtprel_lo12_nc:ld1
	ret
";

/// The TLS types each reference linker is given (§4.7, §10).
pub fn tls_types_for(linker: Linker) -> Vec<&'static str> {
    let mut v = vec![
        "R_AARCH64_TLSLE_MOVW_TPREL_G2",
        "R_AARCH64_TLSLE_MOVW_TPREL_G1",
        "R_AARCH64_TLSLE_MOVW_TPREL_G1_NC",
        "R_AARCH64_TLSLE_MOVW_TPREL_G0",
        "R_AARCH64_TLSLE_MOVW_TPREL_G0_NC",
        "R_AARCH64_TLSLE_ADD_TPREL_HI12",
        "R_AARCH64_TLSLE_ADD_TPREL_LO12_NC",
        "R_AARCH64_TLSIE_ADR_GOTTPREL_PAGE21",
        "R_AARCH64_TLSIE_LD64_GOTTPREL_LO12_NC",
        "R_AARCH64_TLSDESC_ADR_PAGE21",
        "R_AARCH64_TLSDESC_LD64_LO12",
        "R_AARCH64_TLSDESC_ADD_LO12",
        "R_AARCH64_TLSDESC_CALL",
    ];
    if linker != Linker::Lld {
        v.extend([
            "R_AARCH64_TLSLE_ADD_TPREL_LO12",
            "R_AARCH64_TLSGD_ADR_PAGE21",
            "R_AARCH64_TLSGD_ADD_LO12_NC",
        ]);
    }
    if linker != Linker::Mold {
        v.extend([
            "R_AARCH64_TLSLE_LDST8_TPREL_LO12_NC",
            "R_AARCH64_TLSLE_LDST16_TPREL_LO12_NC",
            "R_AARCH64_TLSLE_LDST32_TPREL_LO12_NC",
            "R_AARCH64_TLSLE_LDST64_TPREL_LO12_NC",
        ]);
    }
    if linker == Linker::Bfd {
        v.extend([
            "R_AARCH64_TLSLE_LDST8_TPREL_LO12",
            "R_AARCH64_TLSLE_LDST16_TPREL_LO12",
            "R_AARCH64_TLSLE_LDST32_TPREL_LO12",
            "R_AARCH64_TLSLE_LDST64_TPREL_LO12",
            "R_AARCH64_TLSIE_LD_GOTTPREL_PREL19",
            "R_AARCH64_TLSIE_MOVW_GOTTPREL_G1",
            "R_AARCH64_TLSIE_MOVW_GOTTPREL_G0_NC",
            "R_AARCH64_TLSLD_ADR_PAGE21",
            "R_AARCH64_TLSLD_ADD_LO12_NC",
            "R_AARCH64_TLSLD_ADD_DTPREL_HI12",
            "R_AARCH64_TLSLD_ADD_DTPREL_LO12",
            "R_AARCH64_TLSLD_ADD_DTPREL_LO12_NC",
            "R_AARCH64_TLSLD_MOVW_DTPREL_G2",
            "R_AARCH64_TLSLD_MOVW_DTPREL_G1",
            "R_AARCH64_TLSLD_MOVW_DTPREL_G1_NC",
            "R_AARCH64_TLSLD_MOVW_DTPREL_G0",
            "R_AARCH64_TLSLD_MOVW_DTPREL_G0_NC",
            "R_AARCH64_TLSLD_LDST8_DTPREL_LO12",
            "R_AARCH64_TLSLD_LDST8_DTPREL_LO12_NC",
            "R_AARCH64_TLSLD_LDST16_DTPREL_LO12",
            "R_AARCH64_TLSLD_LDST16_DTPREL_LO12_NC",
            "R_AARCH64_TLSLD_LDST32_DTPREL_LO12",
            "R_AARCH64_TLSLD_LDST32_DTPREL_LO12_NC",
            "R_AARCH64_TLSLD_LDST64_DTPREL_LO12",
            "R_AARCH64_TLSLD_LDST64_DTPREL_LO12_NC",
        ]);
    }
    v
}

/// Local-dynamic in a block aligned to `ALIGN`: exits 42 when the
/// variable is read right (§10.2 A7).
pub const TLS_LD_S: &str = r#"
	.text
	.globl main
	.type main,%function
main:
	stp x29, x30, [sp, #-16]!
	adrp x0, :tlsldm:v2
	add  x0, x0, :tlsldm_lo12_nc:v2
	bl   __tls_get_addr
	nop
	add  x0, x0, #:dtprel_hi12:v2, lsl #12
	add  x0, x0, #:dtprel_lo12_nc:v2
	ldr  x0, [x0]
	ldp x29, x30, [sp], #16
	ret
	.section .tdata,"awT",%progbits
	.balign ALIGN
v1: .quad 5
v2: .quad 42
	.section .note.GNU-stack,"",%progbits
"#;

/// Initial-exec with the address and the value in different registers:
/// exits 42 when the variable is read right (review finding 1, §10.2 A8).
pub const TLS_IE_REGS_S: &str = r#"
	.text
	.globl main
	.type main,%function
main:
	mov  x5, #0
	adrp x4, :gottprel:iv
	ldr  x5, [x4, #:gottprel_lo12:iv]
	mrs  x6, tpidr_el0
	ldr  x0, [x6, x5]
	ret
	.section .tdata,"awT",%progbits
	.balign 8
	// TPREL(iv) >= 2^16: the MOVZ half matters.
	.zero 0x10000
	.globl iv
iv: .quad 42
	.section .note.GNU-stack,"",%progbits
"#;

/// A 64-bit GOT relocation on a 32-bit load (review finding 4, R8).
pub const GOT_LDR_W_S: &str = r"
	.text
	.globl _start
_start:
	adrp x0, :got:gv
	ldr  w0, [x0, #:got_lo12:gv]
	mov x8, #93
	svc #0
	.data
	.balign 8
	.globl gv
gv: .quad 1
";

/// Call frame information of every augmentation the assembler writes
/// (`zR`, `zPLR`, `zRS`, `zRB`) and an FDE `--gc-sections` drops
/// (`spec/plumb.md` §6.4).
pub const EH_FRAMES_S: &str = r#"	.text
	.globl _start
	.type _start,%function
_start:
	.cfi_startproc
	bl f1
	bl f2
	bl f3
	mov x0, #0
	mov x8, #93
	svc #0
	.cfi_endproc
	.size _start, .-_start

	.type f1,%function
f1:
	.cfi_startproc
	.cfi_personality 0x9b, DW.ref.pers
	.cfi_lsda 0x1b, .Llsda1
	stp x29, x30, [sp, -16]!
	.cfi_def_cfa_offset 16
	.cfi_offset 29, -16
	.cfi_offset 30, -8
	mov x29, sp
	ldp x29, x30, [sp], 16
	.cfi_restore 30
	.cfi_restore 29
	.cfi_def_cfa_offset 0
	ret
	.cfi_endproc
	.size f1, .-f1

	.type f2,%function
f2:
	.cfi_startproc
	.cfi_signal_frame
	nop
	ret
	.cfi_endproc
	.size f2, .-f2

	.type f3,%function
f3:
	.cfi_startproc
	.cfi_b_key_frame
	hint #27
	.cfi_negate_ra_state
	hint #31
	ret
	.cfi_endproc
	.size f3, .-f3

	.type pers,%function
pers:
	.cfi_startproc
	ret
	.cfi_endproc
	.size pers, .-pers

	.section .text.dead,"ax",%progbits
	.type dead,%function
dead:
	.cfi_startproc
	.cfi_def_cfa_offset 32
	ret
	.cfi_endproc
	.size dead, .-dead

	.section .gcc_except_table,"a",%progbits
.Llsda1:
	.byte 0xff, 0xff, 0x01, 0x00

	.section .data.rel.local.DW.ref.pers,"aw",%progbits
	.align 3
	.hidden DW.ref.pers
	.type DW.ref.pers,%object
	.size DW.ref.pers, 8
DW.ref.pers:
	.xword pers
"#;

/// PC-relative references to an undefined weak symbol (review finding
/// 10): S is the place (AAELF64), so X is the addend.
pub const WEAK_PREL_S: &str = r"
	.text
	.globl _start
_start:
	adr x1, wu
	ldr x2, wu
	mov x8, #93
	svc #0
	.weak wu
	.data
	.balign 8
	.quad wu - . + 16
	.word wu - . + 8
	.hword wu - .
";

/// A global and a local IFUNC of one resolver (§4.8): calls, ADRP+ADD,
/// GOT pairs and data words to each. `_start` applies the IRELATIVE
/// records between `__rela_iplt_start` and `__rela_iplt_end` itself (there
/// is no C library), checks both calls return 42, and exits 0.
pub const IFUNC_S: &str = r"
	.text
	.type impl, %function
impl:	mov x0, #42
	ret
	.type res, %function
res:	adrp x0, impl
	add x0, x0, :lo12:impl
	ret
	.globl gf
	.type gf, %gnu_indirect_function
	.set gf, res
	.type lf, %gnu_indirect_function
	.set lf, res
	.globl _start
	.type _start, %function
_start:
	adrp x19, __rela_iplt_start
	add x19, x19, :lo12:__rela_iplt_start
	adrp x20, __rela_iplt_end
	add x20, x20, :lo12:__rela_iplt_end
1:	cmp x19, x20
	b.hs 2f
	ldr x21, [x19]
	ldr x22, [x19, #16]
	blr x22
	str x0, [x21]
	add x19, x19, #24
	b 1b
2:	bl gf
	cmp x0, #42
	b.ne 3f
	bl lf
	cmp x0, #42
	b.ne 3f
	adrp x1, gf
	add x1, x1, :lo12:gf
	adrp x2, :got:gf
	ldr x2, [x2, :got_lo12:gf]
	adrp x3, :got:lf
	ldr x3, [x3, :got_lo12:lf]
	adrp x4, lf
	add x4, x4, :lo12:lf
	mov x0, #0
	mov x8, #93
	svc #0
3:	mov x0, #1
	mov x8, #93
	svc #0
	.data
	.balign 8
	.xword gf
	.xword lf
";

/// An IFUNC in C with address-of from two objects and a data word (§4.8).
pub const IFUNC_PICK_C: &str = r#"
static int impl_one(void) { return 1; }
static int impl_two(void) { return 2; }
static int (*resolve_pick(void))(void) {
  return impl_one == impl_two ? impl_one : impl_two;
}
int pick(void) __attribute__((ifunc("resolve_pick")));
int (*pick_address(void))(void) { return pick; }
int (*pick_data)(void) = pick;
"#;

pub const IFUNC_MAIN_C: &str = r#"
#include <stdio.h>
int pick(void);
int (*pick_address(void))(void);
extern int (*pick_data)(void);
int (*mp)(void) = pick;
int main(void) {
  int (*pointer)(void) = pick;
  printf("pick %d, through a pointer %d, same address %d %d %d\n", pick(), pointer(),
         pointer == pick_address(), mp == pointer, pick_data == pointer);
  return pick() == 2 && pointer == pick_address() && mp == pointer && pick_data == pointer ? 0 : 1;
}
"#;

/// Two copies of a COMDAT `big` with debug information; the second is
/// dropped, so its DWARF refers to a section not in the output (§5.8).
pub const DEBUG_A_CC: &str = r"
inline int big(int x) { return x * 3 + 1; }
int a(int y) { return big(y); }
";

pub const DEBUG_B_CC: &str = r"
inline int big(int x) { return x * 3 + 1; }
int a(int y);
int main(int c, char **) { return big(c) + a(c) - 8; }
";

/// A function `--gc-sections` collects, with DWARF 4 ranges and location
/// lists (§5.8).
pub const DEBUG_GC_C: &str = r"
int dead1(int x) { int y = x * 7; for (int i = 0; i < x; i++) y += i * x; return y; }
int live(int x) { int y = x * 5; for (int i = 0; i < x; i++) y ^= i + x; return y; }
int main(int c, char **v) { (void)v; return live(c) - 4; }
";

/// `_start` with a BTI and PAC property note, calling `f` and `g` (§6.6).
pub const PROP_START_S: &str = r#"
	.text
	.globl _start
	.type _start, %function
_start:	hint #34
	bl f
	bl g
	mov x0, #0
	mov x8, #93
	svc #0
	.section .note.gnu.property, "a"
	.balign 8
	.word 4, 16, 5
	.asciz "GNU"
	.word 0xc0000000, 4, 3, 0
"#;

/// Two property notes in one section, BTI and PAC then BTI only
/// (synth.md S-126, X-33).
pub const PROP_TWO_S: &str = r#"
	.text
	.globl f
	.type f, %function
f:	hint #34
	ret
	.section .note.gnu.property, "a"
	.balign 8
	.word 4, 16, 5
	.asciz "GNU"
	.word 0xc0000000, 4, 3, 0
	.word 4, 16, 5
	.asciz "GNU"
	.word 0xc0000000, 4, 1, 0
"#;

pub const PROP_F_C: &str = "int f(int x) { return x + 1; }\n";
pub const PROP_G_C: &str = "int g(int x) { return x * 2; }\n";

const NOT_MOLD: &[Linker] = &[Linker::Bfd, Linker::Lld];
const BFD: &[Linker] = &[Linker::Bfd];

#[allow(clippy::too_many_lines, clippy::items_after_statements)] // the cases, one by one
fn tls_cases(t: &Tools, scratch: &Scratch) -> Vec<Case> {
    let dir = scratch.path();
    let w = |name: &str, text: &str| {
        scratch.write(name, text);
    };
    w("tdef.c", TLS_DEF_C);
    w("tie.c", TLS_IE_C);
    w("tdesc.c", &TLS_DYN_C.replace("SUM", "desc_sum"));
    w("ttrad.c", &TLS_DYN_C.replace("SUM", "trad_sum"));
    w("tclang.c", &TLS_DYN_C.replace("SUM", "clang_sum").replace("d_", "c_"));
    w("tmain.c", TLS_MAIN_C);
    w("tt-all.s", TLS_TYPES_ALL_S);
    w("tt-nlld.s", TLS_TYPES_NOT_LLD_S);
    w("tt-nmold.s", TLS_TYPES_NOT_MOLD_S);
    w("tt-bfd.s", TLS_TYPES_BFD_S);
    w("tld64.s", &TLS_LD_S.replace("ALIGN", "64"));
    w("tld8.s", &TLS_LD_S.replace("ALIGN", "8"));
    w("tieregs.s", TLS_IE_REGS_S);
    w("gotw.s", GOT_LDR_W_S);
    w("weakprel.s", WEAK_PREL_S);
    w("ehf.s", EH_FRAMES_S);
    w("ifs.s", IFUNC_S);
    w("ipick.c", IFUNC_PICK_C);
    w("imain.c", IFUNC_MAIN_C);
    w("dbga.cc", DEBUG_A_CC);
    w("dbgb.cc", DEBUG_B_CC);
    w("dbggc.c", DEBUG_GC_C);
    w("pstart.s", PROP_START_S);
    w("ptwo.s", PROP_TWO_S);
    w("pf.c", PROP_F_C);
    w("pg.c", PROP_G_C);
    let gcc = |args: &[&str]| run_in(dir, &t.gcc, args);
    gcc(&["-O2", "-c", "tdef.c", "-o", "tdef.o"]);
    gcc(&["-O2", "-c", "tie.c", "-o", "tie.o"]);
    gcc(&["-O2", "-fPIC", "-mtls-dialect=desc", "-c", "tdesc.c", "-o", "tdesc.o"]);
    gcc(&["-O2", "-fPIC", "-mtls-dialect=trad", "-c", "ttrad.c", "-o", "ttrad.o"]);
    gcc(&["-O2", "-c", "tmain.c", "-o", "tmain.o"]);
    gcc(&["-O2", "-c", "ipick.c", "-o", "ipick.o"]);
    gcc(&["-O2", "-c", "imain.c", "-o", "imain.o"]);
    run_in(dir, &t.gxx, &["-g", "-O0", "-c", "dbga.cc", "-o", "dbga.o"]);
    run_in(dir, &t.gxx, &["-g", "-O0", "-c", "dbgb.cc", "-o", "dbgb.o"]);
    for (src, obj) in [("dbga.cc", "dbga-clang.o"), ("dbgb.cc", "dbgb-clang.o")] {
        run_in(dir, &t.clang, &["--target=aarch64-linux-gnu", "-g", "-O0", "-c", src, "-o", obj]);
    }
    gcc(&["-gdwarf-4", "-O2", "-ffunction-sections", "-c", "dbggc.c", "-o", "dbggc.o"]);
    for (src, obj, bp) in [
        ("pf.c", "pf.o", "standard"),
        ("pg.c", "pg.o", "standard"),
        ("pg.c", "pg-bti.o", "bti"),
        ("pg.c", "pg-none.o", "none"),
    ] {
        gcc(&["-O2", &format!("-mbranch-protection={bp}"), "-c", src, "-o", obj]);
    }
    gcc(&["-O2", "-DTRAD", "-c", "tmain.c", "-o", "tmain-trad.o"]);
    run_in(
        dir,
        &t.clang,
        &["--target=aarch64-linux-gnu", "-O2", "-fPIC", "-c", "tclang.c", "-o", "tclang.o"],
    );
    for (src, obj) in [
        ("tt-all.s", "tt-all.o"),
        ("tt-nlld.s", "tt-nlld.o"),
        ("tt-nmold.s", "tt-nmold.o"),
        ("tt-bfd.s", "tt-bfd.o"),
        ("tld64.s", "tld64.o"),
        ("tld8.s", "tld8.o"),
        ("tieregs.s", "tieregs.o"),
        ("gotw.s", "gotw.o"),
        ("weakprel.s", "weakprel.o"),
        ("ehf.s", "ehf.o"),
        ("ifs.s", "ifs.o"),
        ("pstart.s", "pstart.o"),
        ("ptwo.s", "ptwo.o"),
    ] {
        gcc(&["-c", src, "-o", obj]);
    }
    fn gcc_driver(t: &Tools) -> &Path {
        &t.gcc
    }
    fn gxx_driver(t: &Tools) -> &Path {
        &t.gxx
    }
    let free = vec!["-static", "-nostdlib"];
    vec![
        Case {
            name: "tls-c",
            linkers: &Linker::ALL,
            driver: gcc_driver,
            objects: vec!["tdef.o", "tie.o", "tdesc.o", "tclang.o"],
            flags: vec!["-static"],
            per_linker: |l| match l {
                Linker::Lld => vec!["tmain.o"],
                _ => vec!["tmain-trad.o", "ttrad.o"],
            },
        },
        Case {
            name: "tls-types",
            linkers: &Linker::ALL,
            driver: gcc_driver,
            objects: vec!["tt-all.o"],
            flags: free.clone(),
            per_linker: |l| match l {
                Linker::Bfd => vec!["tt-nlld.o", "tt-nmold.o", "tt-bfd.o"],
                Linker::Lld => vec!["tt-nmold.o"],
                Linker::Mold => vec!["tt-nlld.o"],
            },
        },
        Case {
            name: "tls-ld-64",
            linkers: BFD,
            driver: gcc_driver,
            objects: vec!["tld64.o"],
            flags: vec!["-static"],
            per_linker: none,
        },
        Case {
            name: "tls-ld-8",
            linkers: BFD,
            driver: gcc_driver,
            objects: vec!["tld8.o"],
            flags: vec!["-static"],
            per_linker: none,
        },
        Case {
            name: "tls-ie-regs",
            linkers: &Linker::ALL,
            driver: gcc_driver,
            objects: vec!["tieregs.o"],
            flags: vec!["-static"],
            per_linker: none,
        },
        Case {
            name: "got-ldr-w",
            linkers: &Linker::ALL,
            driver: gcc_driver,
            objects: vec!["gotw.o"],
            flags: free.clone(),
            per_linker: none,
        },
        Case {
            name: "eh-frames",
            linkers: &Linker::ALL,
            driver: gcc_driver,
            objects: vec!["ehf.o"],
            flags: vec!["-static", "-nostdlib", "-Wl,--eh-frame-hdr", "-Wl,--gc-sections"],
            per_linker: none,
        },
        Case {
            name: "ifunc",
            linkers: &Linker::ALL,
            driver: gcc_driver,
            objects: vec!["ifs.o"],
            flags: free.clone(),
            per_linker: none,
        },
        Case {
            name: "ifunc-c",
            linkers: &Linker::ALL,
            driver: gcc_driver,
            objects: vec!["ipick.o", "imain.o"],
            flags: vec!["-static"],
            per_linker: none,
        },
        Case {
            name: "debug-comdat",
            linkers: &Linker::ALL,
            driver: gxx_driver,
            objects: vec!["dbga.o", "dbgb.o"],
            flags: vec!["-static"],
            per_linker: none,
        },
        Case {
            name: "debug-comdat-clang",
            linkers: &Linker::ALL,
            driver: gxx_driver,
            objects: vec!["dbga-clang.o", "dbgb-clang.o"],
            flags: vec!["-static"],
            per_linker: none,
        },
        Case {
            name: "debug-gc",
            linkers: &Linker::ALL,
            driver: gcc_driver,
            objects: vec!["dbggc.o"],
            flags: vec!["-static", "-Wl,--gc-sections"],
            per_linker: none,
        },
        Case {
            name: "debug-zlib",
            linkers: &Linker::ALL,
            driver: gcc_driver,
            objects: vec!["dbggc.o"],
            flags: vec!["-static", "-Wl,--gc-sections", "-Wl,--compress-debug-sections=zlib"],
            per_linker: none,
        },
        Case {
            name: "prop-and",
            linkers: &Linker::ALL,
            driver: gcc_driver,
            objects: vec!["pstart.o", "pf.o", "pg.o"],
            flags: free.clone(),
            per_linker: none,
        },
        Case {
            name: "prop-bti",
            linkers: &Linker::ALL,
            driver: gcc_driver,
            objects: vec!["pstart.o", "pf.o", "pg-bti.o"],
            flags: free.clone(),
            per_linker: none,
        },
        Case {
            name: "prop-none",
            linkers: &Linker::ALL,
            driver: gcc_driver,
            objects: vec!["pstart.o", "pf.o", "pg-none.o"],
            flags: free.clone(),
            per_linker: none,
        },
        Case {
            name: "prop-two",
            linkers: &Linker::ALL,
            driver: gcc_driver,
            objects: vec!["pstart.o", "ptwo.o", "pg.o"],
            flags: free.clone(),
            per_linker: none,
        },
        Case {
            name: "weak-prel",
            linkers: NOT_MOLD,
            driver: gcc_driver,
            objects: vec!["weakprel.o"],
            flags: free,
            per_linker: none,
        },
    ]
}

// ---- known anomalies of the reference linkers (§10.2) ----

/// Whether a finding on a reference link is a reference linker's own
/// defect that `spec/plumb.md` §10.2 records, by name. Everything else
/// fails the corpus test.
pub fn known_anomaly(link: &Link, f: &mortar_plumb::Finding) -> Option<&'static str> {
    // The output is read only for the one class that needs its sections.
    let format = link.linker.format();
    mortar_plumb::anomaly::classify(format, &[], f).or_else(|| {
        // S8, Y1 for i386's A2 (its value is 16 bytes before .plt.got),
        // and the static-pie classes A30-A32 (the output's kind).
        matches!(
            f.rule,
            mortar_plumb::Rule::Structure(8)
                | mortar_plumb::Rule::SymbolValue
                | mortar_plumb::Rule::Dyn(9 | 10)
                | mortar_plumb::Rule::Field
                | mortar_plumb::Rule::Ifunc(3)
        )
            .then(|| mortar_plumb::anomaly::classify(format, &link.output_bytes(), f))
            .flatten()
    })
}

/// The findings of a report on `link` that are not recorded anomalies.
pub fn unexpected<'r>(link: &Link, r: &'r Report) -> Vec<&'r mortar_plumb::Finding> {
    r.findings.iter().filter(|f| known_anomaly(link, f).is_none()).collect()
}

/// Asserts that a report on `link` has no finding but recorded anomalies.
pub fn assert_clean(link: &Link, r: &Report) {
    let bad = unexpected(link, r);
    assert!(
        bad.is_empty(),
        "{}: {} findings not recorded as anomalies, the first:\n{:?}\n{}",
        link.label(),
        bad.len(),
        bad.first(),
        r.summary()
    );
}

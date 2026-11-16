//! Words in a non-alloc section against symbols only a shared library
//! defines (`spec/arch.md` §4.3 "Non-alloc places", `spec/a64.md` §13.2,
//! `spec/x64.md` X-53; WP-113), through the compiler driver on AArch64
//! and x86-64:
//!
//! - `S` is the copy's address when the output gives the symbol a copy
//!   relocation, the canonical PLT entry's when it gives it one, and 0
//!   otherwise: an ordinary PLT entry (a function only called, a
//!   library's IFUNC) is not the symbol's address. The word is `0 + A`,
//!   or `0 + A − P`. Before WP-113 mortar wrote the PLT stub.
//! - `DTPOFF64`/`DTPOFF32` (x86-64) and `TLS_DTPREL64` (AArch64) against
//!   a library's TLS variable write `0 + A`, with or without a `PT_TLS`
//!   (before: an internal error without one, `A − start` with one); an
//!   absolute word against it is refused by name.
//!
//! The bytes are asserted directly, and against GNU ld 2.44's where it
//! writes the ruling's words (AArch64 PIE words; x86-64 `DTPOFF*` without
//! a `PT_TLS`). The
//! same shapes as link tests (L3-judged) wait for the verifier's rule
//! (WP-104): `spec/landed/dyn/WP-113.md`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mortar_testutil::{Outcome, Scratch, run_with, tool};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");
const SECTION: &str = ".debug_mortar_dsoref";

struct Target {
    name: &'static str,
    gcc: &'static str,
    progbits: &'static str,
}

const AARCH64: Target =
    Target { name: "aarch64", gcc: "aarch64-linux-gnu-gcc", progbits: "%progbits" };
const X86_64: Target =
    Target { name: "x86_64", gcc: "x86_64-linux-gnu-gcc", progbits: "@progbits" };

const LIB: &str = "void dfunc(void) {}\nvoid cfunc(void) {}\nvoid afunc(void) {}\n\
    int dvar[8] = {5};\nint cvar = 9;\n__thread int tvar = 3;\n\
    static int impl(void) { return 1; }\nstatic void *res(void) { return impl; }\n\
    int ifn(void) __attribute__((ifunc(\"res\")));\n";
/// Calls dfunc and ifn (ordinary PLT entries), takes afunc's address and
/// reads cvar: through the GOT under `-fPIC`; a canonical PLT entry and a
/// copy under `-fno-pic` in a non-PIE.
const MAIN: &str = "void dfunc(void);\nint ifn(void);\nvoid afunc(void);\nextern int cvar;\n\
    void *volatile p;\nint main(void) { dfunc(); p = (void *)afunc; return ifn() + cvar - 10; }\n";
const OWN_TLS: &str = "__thread int mine = 7;\nint getmine(void) { return mine; }\n";

/// The words: dfunc, ifn, dvar, dvar+16 (64); dfunc − . (64, 32);
/// dfunc+8, afunc, cvar, cvar+4 (64); cfunc (32).
fn words_s(t: &Target) -> String {
    format!(
        ".section {SECTION},\"\",{}\n.quad dfunc\n.quad ifn\n.quad dvar\n.quad dvar+16\n\
         .quad dfunc - .\n.long dfunc - .\n.quad dfunc+8\n.quad afunc\n.quad cvar\n\
         .quad cvar+4\n.long cfunc\n",
        t.progbits
    )
}

struct Cx {
    scratch: Scratch,
    _bin: Scratch,
    b: String,
    gcc: PathBuf,
}

impl Cx {
    /// The library, `pie.o` (`-fPIC`), `exec.o` (`-fno-pic`), `own.o` (a
    /// TLS variable of the executable's) and the `files` assembled;
    /// `None` when the target's compiler is missing.
    fn new(label: &str, t: &Target, files: &[(&str, String)]) -> Option<Self> {
        let gcc = tool(t.gcc)?;
        let bin = Scratch::new(&format!("{label}-{}-bin", t.name));
        std::os::unix::fs::symlink(MORTAR, bin.join("ld")).unwrap();
        let b = format!("-B{}/", bin.path().display());
        let cx = Self { scratch: Scratch::new(&format!("{label}-{}", t.name)), _bin: bin, b, gcc };
        cx.scratch.write("lib.c", LIB);
        cx.scratch.write("main.c", MAIN);
        cx.scratch.write("own.c", OWN_TLS);
        cx.ok(&[
            "-O1",
            "-fPIC",
            "-shared",
            "-fuse-ld=bfd",
            "-Wl,-soname,libv.so",
            "lib.c",
            "-o",
            "libv.so",
        ]);
        cx.ok(&["-O1", "-fPIC", "-c", "main.c", "-o", "pie.o"]);
        cx.ok(&["-O1", "-fno-pic", "-c", "main.c", "-o", "exec.o"]);
        cx.ok(&["-O1", "-fPIC", "-c", "own.c", "-o", "own.o"]);
        for (name, text) in files {
            cx.scratch.write(&format!("{name}.s"), text);
            cx.ok(&["-c", &format!("{name}.s"), "-o", &format!("{name}.o")]);
        }
        Some(cx)
    }

    fn gcc(&self, args: &[&str]) -> Outcome {
        let mut c = Command::new(&self.gcc);
        c.args(args).current_dir(self.scratch.path());
        run_with(&mut c, b"", Duration::from_mins(2))
    }

    fn ok(&self, args: &[&str]) {
        let o = self.gcc(args);
        assert!(o.success(), "gcc {args:?}: {}", o.stderr_text());
    }

    /// Links `objects` against libv.so as `mode` (`-pie`, `-no-pie`) with
    /// mortar, or with GNU ld when `bfd`, to `out`.
    fn link(&self, bfd: bool, mode: &str, objects: &[&str], out: &str) -> Outcome {
        let linker = if bfd { "-fuse-ld=bfd" } else { self.b.as_str() };
        let mut a = vec![linker, mode];
        a.extend_from_slice(objects);
        a.extend(["-L.", "-lv", "-o", out]);
        self.gcc(&a)
    }

    fn linked(&self, bfd: bool, mode: &str, objects: &[&str], out: &str) -> Elf {
        let o = self.link(bfd, mode, objects, out);
        assert!(o.success(), "link {mode} {objects:?} (bfd {bfd}): {}", o.stderr_text());
        Elf(std::fs::read(self.scratch.join(out)).unwrap())
    }

    fn path(&self) -> &Path {
        self.scratch.path()
    }
}

/// An ELF64 little-endian file, read just enough for these tests.
struct Elf(Vec<u8>);

impl Elf {
    fn u16(&self, o: usize) -> usize {
        usize::from(u16::from_le_bytes(self.0[o..o + 2].try_into().unwrap()))
    }
    fn u32(&self, o: usize) -> u32 {
        u32::from_le_bytes(self.0[o..o + 4].try_into().unwrap())
    }
    fn u64(&self, o: usize) -> u64 {
        u64::from_le_bytes(self.0[o..o + 8].try_into().unwrap())
    }
    fn at(&self, o: u64) -> usize {
        usize::try_from(o).unwrap()
    }

    /// (`sh_name`, `sh_offset`, `sh_size`, `sh_link`) of every section.
    fn sections(&self) -> Vec<(u32, u64, u64, u32)> {
        let (off, size, n) = (self.at(self.u64(0x28)), self.u16(0x3a), self.u16(0x3c));
        (0..n)
            .map(|i| {
                let s = off + i * size;
                (self.u32(s), self.u64(s + 0x18), self.u64(s + 0x20), self.u32(s + 0x28))
            })
            .collect()
    }

    fn cstr(&self, o: u64) -> &[u8] {
        let o = self.at(o);
        let end = self.0[o..].iter().position(|&b| b == 0).unwrap();
        &self.0[o..o + end]
    }

    fn section(&self, name: &str) -> &[u8] {
        let all = self.sections();
        let strtab = all[self.u16(0x3e)].1;
        let &(_, off, size, _) = all
            .iter()
            .find(|s| self.cstr(strtab + u64::from(s.0)) == name.as_bytes())
            .unwrap_or_else(|| panic!("no section {name}"));
        &self.0[self.at(off)..self.at(off + size)]
    }

    /// `st_value` of `name` in `.dynsym`.
    fn dynsym(&self, name: &str) -> u64 {
        let all = self.sections();
        let strtab = all[self.u16(0x3e)].1;
        let &(_, off, size, link) =
            all.iter().find(|s| self.cstr(strtab + u64::from(s.0)) == b".dynsym").unwrap();
        let dynstr = all[link as usize].1;
        (0..size / 24)
            .map(|i| self.at(off + i * 24))
            .find(|&e| self.cstr(dynstr + u64::from(self.u32(e))) == name.as_bytes())
            .map(|e| self.u64(e + 8))
            .unwrap_or_else(|| panic!("no .dynsym entry {name}"))
    }

    /// `p_vaddr` of the `PT_TLS` program header, if any.
    fn tls_start(&self) -> Option<u64> {
        let (off, size, n) = (self.at(self.u64(0x20)), self.u16(0x36), self.u16(0x38));
        (0..n).map(|i| off + i * size).find(|&p| self.u32(p) == 7).map(|p| self.u64(p + 0x10))
    }
}

/// The section's words: 64-bit at `q`, 32-bit at `l`.
fn q(w: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(w[o..o + 8].try_into().unwrap())
}
fn l(w: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(w[o..o + 4].try_into().unwrap())
}

/// The ruling's words of [`words_s`] with `afunc` and `cvar` given (0 in
/// a PIE): 0 + A, 0 + A − P, the canonical entry, the copy.
fn expected(afunc: u64, cvar: u64) -> Vec<u8> {
    let mut v = Vec::new();
    for x in [0u64, 0, 0, 16, 0u64.wrapping_sub(0x20)] {
        v.extend(x.to_le_bytes());
    }
    v.extend(0u32.wrapping_sub(0x28).to_le_bytes());
    for x in [8u64, afunc, cvar, cvar + 4] {
        v.extend(x.to_le_bytes());
    }
    v.extend(0u32.to_le_bytes());
    v
}

// spec: arch.md §4.3 "Non-alloc places" (WP-113): in a PIE from `-fPIC`
// code, no word against libv.so's symbols has an address: each is 0 + A
// or 0 + A − P, the called function and the library IFUNC included
// (mortar wrote their PLT stubs). GNU ld writes the same bytes on
// AArch64 (on x86-64 it writes the stubs).
#[test]
fn pie_words_are_zero_plus_addend() {
    for t in [&AARCH64, &X86_64] {
        let Some(cx) = Cx::new("nonalloc-pie", t, &[("words", words_s(t))]) else { continue };
        let out = cx.linked(false, "-pie", &["pie.o", "words.o"], "pie");
        assert_eq!(out.section(SECTION), expected(0, 0).as_slice(), "{}", t.name);
        if t.name == "aarch64" {
            let bfd = cx.linked(true, "-pie", &["pie.o", "words.o"], "pie.bfd");
            assert_eq!(out.section(SECTION), bfd.section(SECTION), "GNU ld's words");
        }
    }
}

// spec: arch.md §4.3 "Non-alloc places", dyn.md D-43, D-44 (WP-113): in
// a non-PIE from `-fno-pic` code, afunc has a canonical PLT entry and cvar
// a copy: their words are those addresses (the `.dynsym` values the
// loader binds every module to); dfunc and ifn, only called, have
// ordinary PLT entries and their words stay 0 + A (lld writes the same
// words but drops the addends; GNU ld writes the stubs).
#[test]
fn exec_words_take_the_copy_and_the_canonical_entry_only() {
    for t in [&AARCH64, &X86_64] {
        let Some(cx) = Cx::new("nonalloc-exec", t, &[("words", words_s(t))]) else { continue };
        let out = cx.linked(false, "-no-pie", &["exec.o", "words.o"], "exec");
        let (afunc, cvar) = (out.dynsym("afunc"), out.dynsym("cvar"));
        assert!(afunc != 0 && cvar != 0, "{}: no canonical entry or copy", t.name);
        assert_eq!(out.dynsym("dfunc"), 0, "{}: dfunc is not canonical", t.name);
        assert_eq!(out.section(SECTION), expected(afunc, cvar).as_slice(), "{}", t.name);
        // The canonical entry and the copy are where the program's own
        // code reaches them: the link that names them is the oracle.
        let w = out.section(SECTION);
        assert_eq!((q(w, 0x34), q(w, 0x3c)), (afunc, cvar));
        assert_eq!(l(w, 0x4c), 0, "cfunc, never referenced by code");
    }
}

/// `DTPOFF64`, `DTPOFF32`, `DTPOFF64 + 8`, `DTPOFF32 + 8` against tvar.
const DTPOFF_S: &str = ".section .debug_mortar_dsoref,\"\",@progbits\n.quad tvar@dtpoff\n\
    .long tvar@dtpoff\n.quad tvar@dtpoff+8\n.long tvar@dtpoff+8\n";
/// `TLS_DTPREL64` (1029) against tvar, A 0 and 8, by `.reloc` (no
/// assembler syntax writes it in data; no compiler emits it, a64.md §13.2).
const DTPREL_S: &str = ".type tvar,%tls_object\n.section .debug_mortar_dsoref,\"\",%progbits\n\
    .reloc ., R_AARCH64_TLS_DTPREL64, tvar\n.xword 0\n\
    .reloc ., R_AARCH64_TLS_DTPREL64, tvar+8\n.xword 0\n";

// spec: arch.md §4.3 "Non-alloc places", x64.md X-53, X-46 (WP-113):
// `DTPOFF64`/`DTPOFF32` against libv.so's TLS variable, in a link with no
// TLS of its own (mortar stopped with "internal error: no PT_TLS in the
// tables"), write 0 + A, and so do they in a link with a PT_TLS (the
// symbol is not in this module's block; the supervisor's ruling). GNU ld
// 2.44 (and mold 2.37) write these bytes without a PT_TLS; with one they
// write A − its start, a number that names nothing (lld writes 0).
#[test]
fn dtpoff_against_a_library_tls_variable() {
    let Some(cx) = Cx::new("nonalloc-dtpoff", &X86_64, &[("dtp", DTPOFF_S.to_owned())]) else {
        return;
    };
    for (mode, main) in [("-pie", "pie.o"), ("-no-pie", "exec.o")] {
        for own in [false, true] {
            let objs: Vec<&str> =
                if own { vec![main, "own.o", "dtp.o"] } else { vec![main, "dtp.o"] };
            let out = cx.linked(false, mode, &objs, "tls");
            let start = out.tls_start();
            assert_eq!(start.is_some(), own, "{mode} PT_TLS");
            let w = out.section(SECTION);
            assert_eq!((q(w, 0), l(w, 8)), (0, 0), "{mode} {own}");
            assert_eq!((q(w, 12), l(w, 20)), (8, 8), "{mode} {own}");
            let bfd = cx.linked(true, mode, &objs, "tls.bfd");
            let bfd_base = bfd.tls_start().unwrap_or(0);
            let b = bfd.section(SECTION);
            assert_eq!(
                (q(b, 0).wrapping_add(bfd_base), q(b, 12).wrapping_add(bfd_base)),
                (0, 8),
                "GNU ld's DtpRel(0 + A), which mortar does not follow with a PT_TLS ({mode} {own})"
            );
            if !own {
                assert_eq!(w, b, "GNU ld's bytes ({mode})");
            }
        }
    }
}

// spec: arch.md §4.3 "Non-alloc places", a64.md §13.2 (WP-113): the same
// rule for AArch64's `TLS_DTPREL64`, written by hand (every reference
// refuses it): 0 + A with or without a PT_TLS (mortar's internal error
// before without one).
#[test]
fn dtprel64_against_a_library_tls_variable() {
    let Some(cx) = Cx::new("nonalloc-dtprel", &AARCH64, &[("dtp", DTPREL_S.to_owned())]) else {
        return;
    };
    for (own, objs) in [(false, &["pie.o", "dtp.o"][..]), (true, &["pie.o", "own.o", "dtp.o"])] {
        let out = cx.linked(false, "-pie", objs, "tls");
        assert_eq!(out.tls_start().is_some(), own);
        let w = out.section(SECTION);
        assert_eq!((q(w, 0), q(w, 8)), (0, 8), "own {own}");
    }
}

// spec: arch.md §4.3 "Non-alloc places" (WP-113): an absolute word
// against a TLS symbol only a library defines keeps the TLS mismatch
// rule: refused by name (`relocation-incompatible`, "it is
// thread-local"), on both targets, with and without a PT_TLS.
#[test]
fn absolute_word_against_a_library_tls_variable_is_refused() {
    for t in [&AARCH64, &X86_64] {
        let tls_object = if t.name == "aarch64" { "%tls_object" } else { "@tls_object" };
        let abs = format!(
            ".type tvar,{tls_object}\n.section {SECTION},\"\",{}\n.quad tvar\n",
            t.progbits
        );
        let Some(cx) = Cx::new("nonalloc-tls-abs", t, &[("abs", abs)]) else { continue };
        for objs in [&["pie.o", "abs.o"][..], &["pie.o", "own.o", "abs.o"]] {
            let o = cx.link(false, "-pie", objs, "abs");
            assert!(!o.success(), "{}: linked", t.name);
            let e = o.stderr_text();
            assert!(e.contains("tvar") && e.contains("it is thread-local"), "{}: {e}", t.name);
            assert!(!e.contains("internal error"), "{}: {e}", t.name);
            assert!(!cx.path().join("abs").exists(), "a failed link left its output");
        }
    }
}

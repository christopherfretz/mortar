//! Support for the front's differential tests (WP-027a): programs built
//! with the system compilers, linked by GNU ld, lld and mold, and laid out
//! by `mortar::front`; each link summarized the same way.
//!
//! The programs are AArch64's on every host ([`build`], [`AARCH64`]): the
//! AArch64 toolchain by its plain names on an AArch64 host and as
//! `aarch64-linux-gnu-*` elsewhere, the programs run natively or under
//! `qemu-aarch64` (`spec/harness.md` "Hosted CI", "A test's subject
//! decides its tools").

#![allow(dead_code, reason = "each test binary uses part of it")]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use mortar::front::{Env, Front, front};
use mortar::timing::Timer;
use mortar_core::par::Pool;
use mortar_diag::{Collector, Policy, Report};
use mortar_input::store::{Store, StorePolicy};
use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, run, tool};

/// The architecture of the programs these tests build and link.
pub const AARCH64: Arch = Arch::Aarch64;

pub mod plumb;

pub const SHT_NOBITS: u32 = 8;
pub const SHF_ALLOC: u64 = 2;

/// A source file of a test program.
#[derive(Debug, Clone, Copy)]
pub struct Source {
    pub name: &'static str,
    pub text: &'static str,
}

/// A static C program: IFUNC (its own and glibc's), TLS, stdio.
pub const C_PROGRAM: &[Source] = &[Source {
    name: "c1.c",
    text: r#"#include <stdio.h>
#include <stdlib.h>
#include <string.h>
static __thread int tls_counter = 3;
__thread char tls_buf[100];
static int impl_a(void) { return 1; }
static void *resolve_f(void) { return (void *)impl_a; }
int f(void) __attribute__((ifunc("resolve_f")));
const char *names[] = { "alpha", "beta", "gamma" };
int main(int argc, char **argv) {
    char *b = malloc(64);
    strcpy(b, argv[0]);
    tls_buf[argc] = 1;
    printf("%s %d %d %s\n", b, f(), tls_counter + argc + tls_buf[1], names[argc % 3]);
    free(b);
    return 0;
}
"#,
}];

/// A static C++ program: exceptions, COMDAT (one template in two units),
/// `thread_local`, the library's IFUNCs.
pub const CXX_PROGRAM: &[Source] = &[
    Source {
        name: "x1.cpp",
        text: r#"#include <cstdio>
#include <stdexcept>
#include <string>
#include <vector>
template <class T> T twice(T x) { return x + x; }
int other(int);
thread_local int depth = 0;
struct E : std::runtime_error { using std::runtime_error::runtime_error; };
static int thrower(int n) { ++depth; if (n > 2) throw E("deep"); return n; }
int main(int argc, char **) {
    std::vector<std::string> v{"a", "b"};
    try { thrower(argc + 5); } catch (const E &e) {
        std::printf("%s %d %zu %d %d\n", e.what(), depth, v.size(), twice(argc), other(argc));
    }
    return 0;
}
"#,
    },
    Source {
        name: "x2.cpp",
        text: r#"#include <string>
template <class T> T twice(T x) { return x + x; }
inline std::string label(int n) { return std::to_string(n) + "!"; }
int other(int x) { return twice(x) + static_cast<int>(label(x).size()); }
"#,
    },
];

/// A static Fortran program: allocatable arrays and list-directed output.
pub const FORTRAN_PROGRAM: &[Source] = &[Source {
    name: "f1.f90",
    text: r"program p
  implicit none
  real, allocatable :: a(:)
  integer :: n
  n = command_argument_count() + 10
  allocate(a(n))
  a = 2.0
  print *, sum(a), n
end program p
",
}];

/// A compiler driver and its language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Driver {
    Gcc,
    Gxx,
    Clang,
    Clangxx,
    Gfortran,
    /// Clang 22: it writes AArch64 build attributes (`.ARM.attributes`)
    /// under `-mbranch-protection` (`spec/layout.md` L-35).
    Clang22,
}

impl Driver {
    /// The driver's name for [`AARCH64`] on this host.
    pub fn tool(self) -> String {
        match self {
            Self::Gcc => AARCH64.gnu("gcc"),
            Self::Gxx => AARCH64.gnu("g++"),
            Self::Gfortran => AARCH64.gnu("gfortran"),
            Self::Clang => "clang".into(),
            Self::Clangxx => "clang++".into(),
            Self::Clang22 => "clang-22".into(),
        }
    }

    /// What the driver is given first, to compile and to link for
    /// [`AARCH64`]: Clang's `--target` off an AArch64 host.
    pub fn target_args(self) -> Vec<String> {
        match self {
            Self::Clang | Self::Clangxx | Self::Clang22 => AARCH64.clang_args(),
            Self::Gcc | Self::Gxx | Self::Gfortran => Vec::new(),
        }
    }
}

/// The objects of a program and the driver's static link line.
#[derive(Debug)]
pub struct Built {
    pub dir: PathBuf,
    /// The linker's arguments, without its name, `-o` naming `out`.
    pub args: Vec<String>,
}

/// Splits a `-###`/`-v` command line: words, `"..."` with `\"` and `\\`.
fn words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        let mut w = String::new();
        if c == '"' {
            chars.next();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => {
                        if let Some(n) = chars.next() {
                            w.push(n);
                        }
                    }
                    '"' => break,
                    c => w.push(c),
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                w.push(c);
                chars.next();
            }
        }
        out.push(w);
    }
    out
}

/// Compiles `sources` with `driver` and `flags` in `scratch`, for
/// [`AARCH64`], and asks the driver for its static link line. `None` when
/// a tool is missing.
pub fn build(
    scratch: &Scratch,
    driver: Driver,
    flags: &[&str],
    sources: &[Source],
) -> Option<Built> {
    let target = driver.target_args();
    let mut all: Vec<&str> = target.iter().map(String::as_str).collect();
    all.extend_from_slice(flags);
    build_by(scratch, &driver.tool(), &all, sources)
}

/// [`build`] with the driver named `cc` (a cross driver such as
/// `x86_64-linux-gnu-gcc`, WP-029b).
pub fn build_by(scratch: &Scratch, cc: &str, flags: &[&str], sources: &[Source]) -> Option<Built> {
    let name = cc;
    let cc = tool(cc)?;
    let mut objects = Vec::new();
    for s in sources {
        let src = scratch.write(s.name, s.text);
        let obj = scratch.join(format!("{}.o", s.name));
        let mut args: Vec<OsString> = flags.iter().map(OsString::from).collect();
        args.extend(["-c".into(), src.into_os_string(), "-o".into(), obj.clone().into_os_string()]);
        let o = run(&cc, &args);
        assert!(o.success(), "{name} {args:?}: {}", o.stderr_text());
        objects.push(obj);
    }
    let mut args: Vec<OsString> = flags.iter().map(OsString::from).collect();
    args.push("-static".into());
    args.push("-###".into());
    args.extend(objects.iter().map(|o| o.clone().into_os_string()));
    args.extend(["-o".into(), "out".into()]);
    let o = run(&cc, &args);
    assert!(o.success(), "{name} -###: {}", o.stderr_text());
    let text = o.stderr_text();
    let line = text
        .lines()
        .rev()
        .find(|l| {
            let w = words(l);
            w.first().is_some_and(|p| {
                p.ends_with("collect2")
                    || p.ends_with("/ld")
                    || p.ends_with("ld.bfd")
                    || p.ends_with("-linux-gnu-ld")
            })
        })
        .unwrap_or_else(|| panic!("no link line in {text}"));
    let mut args = words(line);
    args.remove(0);
    Some(Built { dir: scratch.path().to_path_buf(), args })
}

/// The link line with `-o` naming `out`.
pub fn with_output(args: &[String], out: &Path) -> Vec<OsString> {
    let mut v: Vec<OsString> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "-o" {
            it.next();
            v.push("-o".into());
            v.push(out.as_os_str().to_owned());
        } else {
            v.push(a.into());
        }
    }
    v
}

/// One section header row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sec {
    pub name: String,
    pub sh_type: u32,
    pub flags: u64,
    pub addr: u64,
    pub size: u64,
    pub align: u64,
}

/// One program header row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seg {
    pub p_type: String,
    pub flags: String,
    pub vaddr: u64,
    pub memsz: u64,
    pub filesz: u64,
}

/// What the differential tables compare of a link.
#[derive(Debug, Clone, Default)]
pub struct Summary {
    pub sections: Vec<Sec>,
    pub segments: Vec<Seg>,
    /// Defined symbols' values, by name (globals and the linker's).
    pub symbols: BTreeMap<String, u64>,
    pub symtab_entries: u64,
    pub got_bytes: u64,
    pub irelative: u64,
    pub cies: u64,
    pub fdes: u64,
    pub hdr_fdes: Option<u64>,
    pub entry: u64,
    /// The `.symtab` entries after entry 0: `(type, binding, st_shndx
    /// as readelf shows it, name)`; types and bindings as readelf names
    /// them (`IFUNC` for `STT_GNU_IFUNC`).
    pub entries: Vec<(String, String, String, String)>,
}

impl Summary {
    pub fn section(&self, name: &str) -> Option<&Sec> {
        self.sections.iter().find(|s| s.name == name)
    }
}

fn hex(s: &str) -> u64 {
    u64::from_str_radix(s.trim_start_matches("0x"), 16).unwrap_or_else(|_| panic!("hex {s:?}"))
}

fn sh_type(t: &str) -> u32 {
    match t {
        "NULL" => 0,
        "PROGBITS" => 1,
        "SYMTAB" => 2,
        "STRTAB" => 3,
        "RELA" => 4,
        "NOTE" => 7,
        "NOBITS" => 8,
        "INIT_ARRAY" => 14,
        "FINI_ARRAY" => 15,
        "PREINIT_ARRAY" => 16,
        "GROUP" => 17,
        "SYMTAB_SHNDX" => 18,
        "AARCH64_ATTRIBUTES" => 0x7000_0003,
        "X86_64_UNWIND" => 0x7000_0001,
        _ => panic!("section type {t}"),
    }
}

fn sh_flags(f: &str) -> u64 {
    f.chars()
        .map(|c| match c {
            'W' => 1,
            'A' => 2,
            'X' => 4,
            'M' => 0x10,
            'S' => 0x20,
            'I' => 0x40,
            'L' => 0x80,
            'G' => 0x200,
            'T' => 0x400,
            'R' => 0x10_0000,
            _ => 0,
        })
        .sum()
}

/// A reference linker's output, summarized with readelf.
pub fn summarize_elf(path: &Path) -> Summary {
    let readelf = tool("readelf").unwrap_or_else(|| panic!("readelf"));
    let r = |args: &[&str]| {
        let mut a: Vec<OsString> = args.iter().map(OsString::from).collect();
        a.push(path.as_os_str().to_owned());
        let o = run(&readelf, &a);
        assert!(o.success(), "readelf {args:?}: {}", o.stderr_text());
        o.stdout_text()
    };
    let mut s = Summary::default();
    for line in r(&["-SW"]).lines() {
        let Some(rest) = line.trim_start().strip_prefix('[') else { continue };
        let Some((idx, rest)) = rest.split_once(']') else { continue };
        if idx.trim().parse::<u32>().map_or(true, |i| i == 0) {
            continue;
        }
        let t: Vec<&str> = rest.split_whitespace().collect();
        // name type addr off size es [flg] lk inf al
        let (flags, tail) = if t.len() == 10 { (t[6], &t[7..]) } else { ("", &t[6..]) };
        s.sections.push(Sec {
            name: t[0].to_owned(),
            sh_type: sh_type(t[1]),
            flags: sh_flags(flags),
            addr: hex(t[2]),
            size: hex(t[4]),
            align: tail[2].parse().unwrap_or(0),
        });
    }
    let mut in_ph = false;
    for line in r(&["-lW"]).lines() {
        if line.trim_start().starts_with("Entry point") {
            s.entry = hex(line.split_whitespace().last().unwrap_or("0"));
        }
        if line.trim_start().starts_with("Type") {
            in_ph = true;
            continue;
        }
        if in_ph {
            let t: Vec<&str> = line.split_whitespace().collect();
            if t.is_empty() {
                in_ph = false;
                continue;
            }
            if t.len() < 7 || !t[1].starts_with("0x") {
                continue;
            }
            // Type Offset VirtAddr PhysAddr FileSiz MemSiz Flg... Align
            let flags = t[6..t.len() - 1].concat();
            s.segments.push(Seg {
                p_type: t[0].to_owned(),
                flags,
                vaddr: hex(t[2]),
                filesz: hex(t[4]),
                memsz: hex(t[5]),
            });
        }
    }
    for line in r(&["-sW"]).lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.len() >= 7 && t[0].ends_with(':') && t[0] != "0:" && t[1].len() == 16 {
            // readelf shows an OS-specific type or binding (STT_GNU_IFUNC,
            // STB_GNU_UNIQUE) as `<OS specific>: 10`: one word here.
            let line = line.replace("<OS specific>: ", "OS");
            let t: Vec<&str> = line.split_whitespace().collect();
            let (typ, rest) = (t[3].to_owned(), &t[4..]);
            let bind = rest[0].to_owned();
            // Bind, Vis, then `[...]` notes (`[VARIANT_PCS]`), Ndx, Name.
            let mut i = 2;
            while rest.get(i).is_some_and(|w| w.starts_with('[') || w.ends_with(']')) {
                i += 1;
            }
            let ndx = rest.get(i).map_or_else(String::new, |w| (*w).to_owned());
            let name = rest.get(i + 1).map_or_else(String::new, |w| (*w).to_owned());
            s.entries.push((typ, bind, ndx, name));
        }
        if t.len() < 8 || !t[0].ends_with(':') || t[6] == "UND" {
            continue;
        }
        if t[4] == "GLOBAL" || t[4] == "WEAK" || t[7].starts_with("__") {
            s.symbols.insert(t[7].to_owned(), hex(t[1]));
        }
    }
    s.symtab_entries = s.section(".symtab").map_or(0, |x| x.size / 24);
    s.got_bytes = s
        .sections
        .iter()
        .filter(|x| x.name == ".got" || x.name == ".got.plt")
        .map(|x| x.size)
        .sum();
    s.irelative =
        s.sections.iter().filter(|x| x.name.starts_with(".rela")).map(|x| x.size / 24).sum();
    for line in r(&["--debug-dump=frames"]).lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.len() >= 4 && t[3] == "CIE" {
            s.cies += 1;
        }
        if t.len() >= 4 && t[3] == "FDE" {
            s.fdes += 1;
        }
    }
    s.hdr_fdes = s.section(".eh_frame_hdr").map(|h| (h.size - 12) / 8);
    s
}

/// The front's layout of the link line, or its report.
pub fn front_of<'s>(
    args: &[OsString],
    cwd: &Path,
    store: &'s Store,
    pool: Pool,
) -> Result<Front<'s>, Report> {
    let parsed = mortar_cli::parse(args);
    assert!(!parsed.failed(), "{:?}", parsed.diagnostics);
    let diag = Collector::new(Policy::default());
    let f = front(parsed.plan, Env { cwd, pool }, store, &diag, &mut Timer::new(false));
    let report = diag.finish();
    match f {
        Some(f) if !report.failed() => Ok(f),
        _ => Err(report),
    }
}

pub fn store() -> Store {
    Store::new(StorePolicy::from_env())
}

/// The front's layout, summarized as [`summarize_elf`] summarizes a file.
pub fn summarize_front(f: &Front<'_>) -> Summary {
    let mut s = Summary::default();
    for r in f.sections() {
        s.sections.push(Sec {
            name: String::from_utf8_lossy(&r.name).into_owned(),
            sh_type: r.sh_type,
            flags: r.flags,
            addr: r.addr.unwrap_or(0),
            size: r.size,
            align: r.align,
        });
    }
    let p = &f.layout.placement;
    for (_, g) in p.segments.iter() {
        s.segments.push(Seg {
            p_type: segment_name(g.p_type),
            flags: segment_flags(g.flags),
            vaddr: g.addr,
            filesz: g.filesz,
            memsz: g.memsz,
        });
    }
    for (id, key) in f.inputs.symbols.iter() {
        if let Some(v) = p.defined_of(id) {
            s.symbols.insert(String::from_utf8_lossy(key.name).into_owned(), v);
        }
    }
    s.symtab_entries = f.synth.symtab.as_ref().map_or(0, |t| t.symtab_size() / 24);
    s.got_bytes = f.synth.got.got_size() + f.synth.got.got_plt_size();
    s.irelative = f.synth.got.rela_plt_size() / 24;
    s.fdes = u64::from(f.eh.fde_count());
    s.cies = f.eh.records().len() as u64 - s.fdes;
    s.hdr_fdes = f.synth.hdr.as_ref().map(|h| h.entries() as u64);
    s.entry = p.entry.unwrap_or(0);
    s
}

pub fn segment_name(t: u32) -> String {
    match t {
        1 => "LOAD".into(),
        4 => "NOTE".into(),
        6 => "PHDR".into(),
        7 => "TLS".into(),
        0x6474_e550 => "GNU_EH_FRAME".into(),
        0x6474_e551 => "GNU_STACK".into(),
        0x6474_e552 => "GNU_RELRO".into(),
        0x6474_e553 => "GNU_PROPERTY".into(),
        t => format!("{t:#x}"),
    }
}

pub fn segment_flags(f: u32) -> String {
    let mut s = String::new();
    if f & 4 != 0 {
        s.push('R');
    }
    if f & 2 != 0 {
        s.push('W');
    }
    if f & 1 != 0 {
        s.push('E');
    }
    s
}

/// The reference links of `built`: `(linker, summary)` for each found.
pub fn references(built: &Built) -> Vec<(&'static str, Summary)> {
    let mut out = Vec::new();
    let bfd = AARCH64.gnu("ld.bfd");
    for (name, exe) in [("bfd", bfd.as_str()), ("lld", "ld.lld"), ("mold", "mold")] {
        let Some(ld) = tool(exe) else { continue };
        let path = built.dir.join(format!("out.{name}"));
        let args = with_output(&built.args, &path);
        let mut cmd = std::process::Command::new(&ld);
        cmd.args(&args).current_dir(&built.dir);
        let o = mortar_testutil::run_with(&mut cmd, &[], std::time::Duration::from_mins(2));
        assert!(o.success(), "{exe}: {}", o.stderr_text());
        out.push((name, summarize_elf(&path)));
    }
    out
}

/// `exe`'s link of `built` with `extra` appended, or `None` without the
/// tool.
pub fn reference_with(built: &Built, exe: &str, extra: &[&str]) -> Option<Summary> {
    let ld = tool(exe)?;
    let path = built.dir.join(format!("out.{exe}.extra"));
    let mut args = with_output(&built.args, &path);
    args.extend(extra.iter().map(OsString::from));
    let mut cmd = std::process::Command::new(&ld);
    cmd.args(&args).current_dir(&built.dir);
    let o = mortar_testutil::run_with(&mut cmd, &[], std::time::Duration::from_mins(2));
    assert!(o.success(), "{exe} {extra:?}: {}", o.stderr_text());
    Some(summarize_elf(&path))
}

/// A table of the sections of each link, side by side, by name.
pub fn table(rows: &[(&str, &Summary)]) -> String {
    use std::fmt::Write;
    let mut names: Vec<String> = Vec::new();
    for (_, s) in rows {
        for sec in &s.sections {
            if !names.contains(&sec.name) {
                names.push(sec.name.clone());
            }
        }
    }
    let mut t = String::new();
    let _ = write!(t, "{:<24}", "section");
    for (n, _) in rows {
        let _ = write!(t, " {n:>14}");
    }
    t.push('\n');
    for name in &names {
        let _ = write!(t, "{name:<24}");
        for (_, s) in rows {
            match s.section(name) {
                Some(x) => {
                    let _ = write!(t, " {:>8x}/{:<5}", x.size, x.align);
                }
                None => {
                    let _ = write!(t, " {:>14}", "-");
                }
            }
        }
        t.push('\n');
    }
    for (label, f) in [
        ("symtab entries", (|s: &Summary| s.symtab_entries) as fn(&Summary) -> u64),
        ("got bytes", |s| s.got_bytes),
        ("irelative", |s| s.irelative),
        ("cie", |s| s.cies),
        ("fde", |s| s.fdes),
        ("hdr fde", |s| s.hdr_fdes.unwrap_or(0)),
    ] {
        let _ = write!(t, "{label:<24}");
        for (_, s) in rows {
            let _ = write!(t, " {:>14}", f(s));
        }
        t.push('\n');
    }
    for (n, s) in rows {
        let _ = write!(t, "{n} segments:");
        for g in &s.segments {
            let _ = write!(t, " {}:{}", g.p_type, g.flags);
        }
        t.push('\n');
    }
    t
}

/// The front's placed input sections, one line each: `addr size name file`
/// (allocated ones, by address), for comparison with a GNU map file.
pub fn places(f: &Front<'_>) -> String {
    use std::fmt::Write;
    let placement = &f.layout.placement;
    let mut rows: Vec<(u64, u64, String, String)> = Vec::new();
    for (sid, place) in placement.place.iter() {
        let Some((chunk, off)) = place.get() else { continue };
        let Some(addr) = placement.chunks.get(chunk).and_then(|c| c.addr) else { continue };
        let header = &f.headers[sid];
        let Some((file, _)) = f.resolution.sh_index(sid) else { continue };
        let shown = f.inputs.files[file].display();
        let name = match shown.member() {
            Some(m) => format!("{}({m})", shown.path()),
            None => shown.path().to_owned(),
        };
        rows.push((
            addr + off,
            header.size,
            String::from_utf8_lossy(header.name).into_owned(),
            name,
        ));
    }
    rows.sort();
    let mut out = String::new();
    for (addr, size, name, file) in rows {
        let _ = writeln!(out, "{addr:#x} {size:#x} {name} {file}");
    }
    out
}

/// The names `.strtab` holds, in entry order (`spec/synth.md` S-86).
pub fn strtab_names(f: &Front<'_>) -> Vec<String> {
    let Some(plan) = &f.synth.symtab else { return Vec::new() };
    let mut buf = vec![0u8; plan.strtab_size() as usize];
    f.with_sym_cx(|cx| {
        plan.write_strtab(cx, &mut mortar_synth::out::Window::new(0, &mut buf)).expect("strtab");
    })
    .expect("sym cx");
    buf.split(|&b| b == 0).skip(1).map(|n| String::from_utf8_lossy(n).into_owned()).collect()
}

/// Addresses that place everything at 0, in section 1 for an input
/// section's symbol, 2 for `.eh_frame`'s, 3 for a common, 4 for a patch:
/// enough to read the entries' names, types, bindings and kinds.
struct AnyAddrs<'f, 's>(&'f Front<'s>);

impl mortar_synth::symtab::SymAddrs for AnyAddrs<'_, '_> {
    fn section(&self, s: mortar_core::SectionId, _: u64) -> Option<(u32, u64)> {
        // 9: a merge section (placed by fragments); 8: any other section
        // with no place in the layout.
        let f = self.0;
        let placed = f.layout.placement.place.get(s).and_then(|p| p.get()).is_some();
        let merge = f.resolution.sections[s].kind == mortar_core::resolution::SectionKind::Merge;
        Some((
            if placed {
                1
            } else if merge {
                9
            } else {
                8
            },
            0,
        ))
    }
    fn eh_frame(&self) -> Option<(u32, u64)> {
        Some((2, 0))
    }
    fn common(&self, _: mortar_core::SymbolId) -> Option<(u32, u64)> {
        Some((3, 0))
    }
    fn internal(&self, _: mortar_core::SymbolId) -> Option<mortar_synth::symtab::InternalValue> {
        Some(mortar_synth::symtab::InternalValue::Absolute(0))
    }
    fn patch(&self, _: usize) -> Option<(u32, u64)> {
        Some((4, 0))
    }
    fn tls_start(&self) -> Option<u64> {
        Some(0)
    }
    fn section_range(&self, _: u32) -> Option<(u64, u64)> {
        Some((0, u64::MAX))
    }
}

/// The `.symtab` entries as `(type, binding, st_shndx, name)` under
/// `AnyAddrs`, entry 0 left out.
pub fn symtab_entries(f: &Front<'_>) -> Vec<(u8, u8, u16, String)> {
    let Some(plan) = &f.synth.symtab else { return Vec::new() };
    let mut strtab = vec![0u8; plan.strtab_size() as usize];
    let mut symtab = vec![0u8; plan.symtab_size() as usize];
    f.with_sym_cx(|cx| {
        plan.write_strtab(cx, &mut mortar_synth::out::Window::new(0, &mut strtab)).expect("strtab");
        plan.write_symtab(cx, &AnyAddrs(f), &mut mortar_synth::out::Window::new(0, &mut symtab))
            .expect("symtab");
    })
    .expect("sym cx");
    symtab
        .chunks_exact(24)
        .skip(1)
        .map(|e| {
            let name = u32::from_le_bytes([e[0], e[1], e[2], e[3]]) as usize;
            let end =
                strtab[name..].iter().position(|&b| b == 0).map_or(strtab.len(), |p| name + p);
            let shndx = u16::from_le_bytes([e[6], e[7]]);
            (e[4] & 0xf, e[4] >> 4, shndx, String::from_utf8_lossy(&strtab[name..end]).into_owned())
        })
        .collect()
}

/// The address of global `name`'s definition in an input section.
pub fn global_addr(f: &Front<'_>, name: &str) -> Option<u64> {
    use mortar_core::layout::EntryDef;
    use mortar_core::resolution::Def;
    let (id, _) = f.inputs.symbols.iter().find(|(_, k)| k.name == name.as_bytes())?;
    let Def::Defined(e) = *f.resolution.def.get(id)? else { return None };
    match f.inputs.entry_def(e)? {
        EntryDef::Absolute(v) => Some(v),
        EntryDef::Section(index, value) => {
            f.layout.placement.section_addr(f.resolution.section_id(e.file, index)?, value)
        }
    }
}

/// `a − b` and `b − a` of two multisets, for a readable failure.
pub fn multiset_diff(a: &[&str], b: &[&str]) -> String {
    let mut count: BTreeMap<&str, i64> = BTreeMap::new();
    for x in a {
        *count.entry(x).or_default() += 1;
    }
    for x in b {
        *count.entry(x).or_default() -= 1;
    }
    count
        .into_iter()
        .filter(|c| c.1 != 0)
        .map(|(k, v)| format!("{k}:{v:+}"))
        .collect::<Vec<_>>()
        .join(" ")
}

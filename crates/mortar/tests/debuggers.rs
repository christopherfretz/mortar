//! Debugger compatibility (`docs/VALIDATION.md` L6): programs in C, C++
//! and Fortran, compiled by GCC and Clang (Fortran: gfortran) with DWARF 4
//! and 5 at `-O0` and `-O2`, linked by mortar and by GNU ld from the same
//! objects and the driver's static link line. The same scripted gdb and
//! lldb sessions run on both outputs and their normalized transcripts must
//! be equal; the static checks (`llvm-dwarfdump --verify`, gdb's
//! `maint check-symtabs` and `info functions`, subprogram addresses, and
//! the tombstones of discarded COMDAT copies) hold mortar's output to the
//! spec and to GNU ld's output.
//!
//! Every difference that normalization removes, or that a check allows,
//! names the spec rule that makes it right:
//!
//! - addresses, process ids and the paths of the two outputs: layout is
//!   each linker's own (`docs/VALIDATION.md` L4: bytes are not compared);
//! - lldb's `PT_LOAD[n]`: mortar puts the headers and the read-only data
//!   in one R segment before the code (`spec/layout.md` L-62), GNU ld puts
//!   `.text` in the first segment;
//! - which of several aliases at one address a debugger names (lldb's
//!   `__libc_start_main` against GNU ld's `__libc_start_main_impl`):
//!   mortar writes a hidden definition as a local, in its file's block, in
//!   `SymbolId` order (`spec/synth.md` S-74, S-77, the gABI's "Symbol
//!   Visibility"); GNU keeps hidden symbols global, so the order of the
//!   aliases differs. Transcripts name each alias set by its least name;
//! - the debug information of a discarded COMDAT copy: mortar writes the
//!   tombstone (`spec/image.md` I-45, I-47, I-112), GNU ld redirects it to
//!   the kept copy (I-47 "GNU ld's redirect to the kept copy was
//!   rejected"). With GNU ld the debuggers see one function three times
//!   (lldb: `3 matches`; gdb's `info line` names a discarded copy's file
//!   and line). Those commands are held to the kept copy on mortar's
//!   output instead of being compared ([`comdat_kept_copy`]).

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mortar_testutil::{Outcome, Scratch, run_with, tool, tools_required};
use support::{AARCH64, Built, Driver, Source, build, with_output};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

/// The tombstone of an address field in ELFCLASS64 (`spec/image.md` I-47).
const DEAD: u64 = u64::MAX;

/// The tombstone of an address that a DWARF 5 list entry pairs with a
/// length (`.debug_rnglists`, `.debug_loclists`, and `.debug_addr`, which
/// Clang's `DW_RLE_startx_length` indexes): 0, `spec/image.md` I-47. A
/// subprogram's `DW_AT_low_pc` read through `.debug_addr` is then 0 too:
/// no code lies there (the non-PIE base, `spec/layout.md` L-68).
const DEAD_PAIRED: u64 = 0;

// ---------------------------------------------------------------------
// The programs.

/// C: a call chain over three objects (`main` → `level1` → `level2` →
/// `level3`), a global, a `__thread` variable, a constructor function. The
/// `pthread_create` that never runs links glibc's thread support, without
/// which gdb cannot read TLS in a static program (`libthread_db` finds no
/// symbols to work with).
const C_SOURCES: &[Source] = &[
    Source {
        name: "dc_main.c",
        text: r#"#include <pthread.h>
#include <stdio.h>
int g_count = 7;
__thread int t_val = 11;
int level1(int);
static int ctor_ran;
static volatile int seed = 1;
__attribute__((constructor)) static void init_c(void) { ctor_ran = g_count + seed; }
static void *noop(void *p) { return p; }
int main(int argc, char **argv) {
    (void)argv;
    if (argc > 5) {
        pthread_t t;
        pthread_create(&t, 0, noop, 0);
        pthread_join(t, 0);
    }
    int total = 0;
    for (int i = 0; i < 2; ++i)
        total += level1(argc + i);
    printf("%d %d\n", total, ctor_ran);
    return 0;
}
"#,
    },
    Source {
        name: "dc_a.c",
        text: r"int level2(int);
__attribute__((noinline)) int level1(int x) {
    int local1 = x * 2;
    int r = level2(local1);
    return r * 3 + local1;
}
",
    },
    Source {
        name: "dc_b.c",
        text: r"extern int g_count;
extern __thread int t_val;
__attribute__((noinline)) int level3(int z) {
    int local3 = z + t_val;
    return local3 * g_count;
}
__attribute__((noinline)) int level2(int y) {
    int local2 = y + g_count;
    int r = level3(local2);
    return r + 1;
}
",
    },
];

/// C++: the C chain, `twice<int>` (a COMDAT function, group
/// `_Z5twiceIiET_S0_`) defined and used in every object, a
/// `thread_local`, a global with a constructor, and an exception thrown
/// in `dx_b.cpp` and caught in `main`.
const CXX_SOURCES: &[Source] = &[
    Source {
        name: "dx_main.cpp",
        text: r#"#include <cstdio>
#include <pthread.h>
template <class T> __attribute__((noinline)) T twice(T x) { return x + x; }
int g_count = 7;
thread_local int t_val = 11;
int level1(int);
struct Oops { int code; };
static int ctor_ran;
static volatile int seed = 1;
struct Init {
    __attribute__((noinline)) Init() { ctor_ran = g_count + seed; }
};
Init g_init;
static void *noop(void *p) { return p; }
int main(int argc, char **) {
    if (argc > 5) {
        pthread_t t;
        pthread_create(&t, nullptr, noop, nullptr);
        pthread_join(t, nullptr);
    }
    int total = twice(argc);
    for (int i = 0; i < 2; ++i)
        total += level1(argc + i);
    try {
        total += level1(100);
    } catch (const Oops &o) {
        total += o.code;
    }
    std::printf("%d %d\n", total, ctor_ran);
    return 0;
}
"#,
    },
    Source {
        name: "dx_a.cpp",
        text: r"template <class T> __attribute__((noinline)) T twice(T x) { return x + x; }
int level2(int);
__attribute__((noinline)) int level1(int x) {
    int local1 = twice(x);
    int r = level2(local1);
    return r * 3 + local1;
}
",
    },
    Source {
        name: "dx_b.cpp",
        text: r"template <class T> __attribute__((noinline)) T twice(T x) { return x + x; }
struct Oops { int code; };
extern int g_count;
extern thread_local int t_val;
__attribute__((noinline)) int level3(int z) {
    int local3 = z + t_val;
    if (z > 100)
        throw Oops{local3};
    return local3 * g_count;
}
__attribute__((noinline)) int level2(int y) {
    int local2 = twice(y) + g_count;
    int r = level3(local2);
    return r + 1;
}
",
    },
];

/// Fortran: the chain over four objects (one function each, so that `-O2`
/// inlines none of them: lldb breaks on symbols only) and a COMMON block for the
/// global (Fortran has no `thread_local` and no static constructor).
const FORTRAN_SOURCES: &[Source] = &[
    Source {
        name: "df_main.f90",
        text: r"program p
  implicit none
  integer :: g_count
  common /shared/ g_count
  integer :: i, r, total
  g_count = 7
  total = 0
  r = 0
  do i = 1, 2
    call level1(command_argument_count() + i, r)
    total = total + r
  end do
  print *, total
end program p
",
    },
    Source {
        name: "df_a.f90",
        text: r"subroutine level1(x, r)
  implicit none
  integer, intent(in) :: x
  integer, intent(out) :: r
  integer :: local1, t
  t = 0
  local1 = x * 2
  call level2(local1, t)
  r = t * 3 + local1
end subroutine level1
",
    },
    Source {
        name: "df_b.f90",
        text: r"subroutine level2(y, r)
  implicit none
  integer, intent(in) :: y
  integer, intent(out) :: r
  integer :: g_count
  common /shared/ g_count
  integer :: local2, t
  t = 0
  local2 = y + g_count
  call level3(local2, t)
  r = t + 1
end subroutine level2
",
    },
    Source {
        name: "df_c.f90",
        text: r"subroutine level3(z, r)
  implicit none
  integer, intent(in) :: z
  integer, intent(out) :: r
  integer :: g_count
  common /shared/ g_count
  integer :: local3
  local3 = z + 11
  r = local3 * g_count
end subroutine level3
",
    },
];

/// What a program offers a debugger session.
#[derive(Debug, Clone, Copy)]
struct Program {
    sources: &'static [Source],
    /// gdb's and lldb's name of the deepest function, in a non-main object.
    deep: (&'static str, &'static str),
    /// The same for the function the session steps through.
    step: (&'static str, &'static str),
    /// A static constructor to break in.
    ctor: Option<&'static str>,
    /// Whether lldb has a plug-in for the language. It has none for
    /// Fortran ("Inspection of frame variables will be limited"): `frame
    /// variable` finds nothing and `source list -n` fails, which ends a
    /// batch session.
    lldb_lang: bool,
    /// A `thread_local` / `__thread` variable.
    tls: Option<&'static str>,
    /// The COMDAT function's symbol, and the source of its kept copy.
    comdat: Option<(&'static str, &'static str)>,
    throws: bool,
}

const C: Program = Program {
    sources: C_SOURCES,
    deep: ("level3", "level3"),
    step: ("level2", "level2"),
    ctor: Some("init_c"),
    lldb_lang: true,
    tls: Some("t_val"),
    comdat: None,
    throws: false,
};

const CXX: Program = Program {
    sources: CXX_SOURCES,
    deep: ("level3", "level3"),
    step: ("level2", "level2"),
    ctor: Some("Init::Init"),
    lldb_lang: true,
    tls: Some("t_val"),
    comdat: Some(("_Z5twiceIiET_S0_", "dx_main.cpp")),
    throws: true,
};

/// lldb has no Fortran plug-in: it breaks on the linkage names.
const FORTRAN: Program = Program {
    sources: FORTRAN_SOURCES,
    deep: ("level3", "level3_"),
    step: ("level2", "level2_"),
    ctor: None,
    lldb_lang: false,
    tls: None,
    comdat: None,
    throws: false,
};

/// The matrix of compiler flags: DWARF 4 and 5, `-O0` and `-O2`.
const FLAGS: &[(&str, &[&str])] = &[
    ("d4-O0", &["-g", "-gdwarf-4", "-O0"]),
    ("d4-O2", &["-g", "-gdwarf-4", "-O2"]),
    ("d5-O0", &["-g", "-gdwarf-5", "-O0"]),
    ("d5-O2", &["-g", "-gdwarf-5", "-O2"]),
];

// ---------------------------------------------------------------------
// Tools, links and runs.

/// The debug tools, or `None` (after a note) when one is missing.
struct Tools {
    gdb: PathBuf,
    lldb: PathBuf,
    dwarfdump: PathBuf,
    ld: PathBuf,
}

/// The programs are AArch64's on every host ([`support::AARCH64`]): off
/// an AArch64 host gdb is `gdb-multiarch`, which reads them (the static
/// checks); the sessions, which run them, are named skips there.
fn tools() -> Option<Tools> {
    let found = (tool(AARCH64.gdb()), tool("lldb"), tool("llvm-dwarfdump"), AARCH64.tool("ld.bfd"));
    let (Some(gdb), Some(lldb), Some(dwarfdump), Some(ld)) = found else {
        eprintln!("note: gdb, lldb, llvm-dwarfdump or ld.bfd is missing; skipping");
        assert!(!tools_required(), "the debugger tests need gdb, lldb, llvm-dwarfdump, ld.bfd");
        return None;
    };
    Some(Tools { gdb, lldb, dwarfdump, ld })
}

/// The map option mortar's link writes its machine map with: the one
/// place to change when the option is renamed.
fn map_option(map: &Path) -> OsString {
    format!("--mortar-map={}", map.display()).into()
}

/// The two links of one set of objects.
struct Pair {
    dir: PathBuf,
    ours: PathBuf,
    theirs: PathBuf,
    /// Both links ran with `--gc-sections` (WP-031).
    gc: bool,
}

fn linker(exe: &Path, args: &[OsString], dir: &Path) -> Outcome {
    run_with(Command::new(exe).args(args).current_dir(dir), b"", Duration::from_mins(5))
}

/// Links `built` with mortar (to `mortar/prog`, its map beside it) and
/// GNU ld (to `bfd/prog`), both with `--gc-sections` when `gc`; mortar's
/// output goes through the verifier.
fn link(t: &Tools, label: &str, built: &Built, gc: bool) -> Pair {
    let ours = built.dir.join("mortar").join("prog");
    let theirs = built.dir.join("bfd").join("prog");
    for out in [&ours, &theirs] {
        std::fs::create_dir_all(out.parent().expect("a directory")).expect("create a directory");
    }
    let map = ours.with_extension("map");
    let extra: &[OsString] = if gc { &[OsString::from("--gc-sections")] } else { &[] };
    let mut args = with_output(&built.args, &ours);
    args.push(map_option(&map));
    args.extend_from_slice(extra);
    let m = linker(Path::new(MORTAR), &args, &built.dir);
    assert!(m.success(), "{label}: mortar: {}", m.stderr_text());
    assert_eq!(m.stderr_text(), "", "{label}: mortar warns");
    let mut args = with_output(&built.args, &theirs);
    args.extend_from_slice(extra);
    let b = linker(&t.ld, &args, &built.dir);
    assert!(b.success(), "{label}: ld.bfd: {}", b.stderr_text());
    support::plumb::clean(&ours, &map, &built.dir);
    Pair { dir: built.dir.clone(), ours, theirs, gc }
}

/// The incremental lane (WP-062; `spec/incr.md` IN-30, IN-50, IN-51b):
/// `ours` is mortar's incremental-mode first link of `built` (regions with
/// slack, debug contributions in size classes with neutral units between
/// them), `theirs` mortar's plain link of the same objects. The
/// incremental output goes through the verifier.
fn link_incr(label: &str, built: &Built) -> Pair {
    let ours = built.dir.join("incr").join("prog");
    let theirs = built.dir.join("plain").join("prog");
    for out in [&ours, &theirs] {
        std::fs::create_dir_all(out.parent().expect("a directory")).expect("create a directory");
    }
    let map = ours.with_extension("map");
    let mut args = with_output(&built.args, &ours);
    args.push(map_option(&map));
    args.push("--incremental=full".into());
    let m = linker(Path::new(MORTAR), &args, &built.dir);
    assert!(m.success(), "{label}: mortar --incremental=full: {}", m.stderr_text());
    assert_eq!(m.stderr_text(), "", "{label}: mortar --incremental=full warns");
    let p = linker(Path::new(MORTAR), &with_output(&built.args, &theirs), &built.dir);
    assert!(p.success(), "{label}: mortar: {}", p.stderr_text());
    let mut findings = support::plumb::verify(&ours, &map, &built.dir);
    findings.retain(|f| !support::plumb::s50_rule1_waiting_for_wp091(f));
    assert!(findings.is_empty(), "plumb on {}: {findings:#?}", ours.display());
    Pair { dir: built.dir.clone(), ours, theirs, gc: false }
}

/// Runs a debugger or dump tool in `dir`, with debuginfod off.
fn tool_run(exe: &Path, args: &[String], dir: &Path, limit: Duration) -> Outcome {
    // One stream, in the order the tool wrote it.
    let mut c = Command::new("sh");
    c.arg("-c").arg("exec \"$0\" \"$@\" 2>&1").arg(exe).args(args);
    c.current_dir(dir).env("DEBUGINFOD_URLS", "").env("TERM", "dumb");
    run_with(&mut c, b"", limit)
}

fn text(o: &Outcome) -> String {
    let mut s = o.stdout_text();
    s.push_str(&o.stderr_text());
    s
}

// ---------------------------------------------------------------------
// The ELF facts the checks need.

/// An output's sections and function/object symbols.
struct Elf {
    /// `(name, address, size, executable)` of every allocated section.
    alloc: Vec<(String, u64, u64, bool)>,
    /// Defined symbols' values by name.
    values: BTreeMap<String, u64>,
    /// Each name that has an alias at its address and type, to the least
    /// name of its alias set (S-74, S-77: the order of the aliases is the
    /// linker's).
    aliases: BTreeMap<String, String>,
}

fn elf(path: &Path) -> Elf {
    let summary = support::summarize_elf(path);
    let alloc = summary
        .sections
        .iter()
        .filter(|s| s.flags & support::SHF_ALLOC != 0)
        .map(|s| (s.name.clone(), s.addr, s.size, s.flags & 4 != 0))
        .collect();
    let readelf = tool("readelf").expect("readelf");
    let o = mortar_testutil::run(&readelf, [OsString::from("-sW"), path.into()]);
    assert!(o.success(), "readelf -sW: {}", o.stderr_text());
    let mut sets: BTreeMap<(u64, String), BTreeSet<String>> = BTreeMap::new();
    let mut values = BTreeMap::new();
    for line in o.stdout_text().lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        // Num: Value Size Type Bind Vis Ndx Name
        if t.len() < 8 || !t[0].ends_with(':') || t[6] == "UND" || t[1].len() != 16 {
            continue;
        }
        let Ok(value) = u64::from_str_radix(t[1], 16) else { continue };
        let name = t[7].to_owned();
        values.insert(name.clone(), value);
        if value != 0 && matches!(t[3], "FUNC" | "OBJECT" | "IFUNC") {
            sets.entry((value, t[3].to_owned())).or_default().insert(name);
        }
    }
    let mut aliases = BTreeMap::new();
    for set in sets.values().filter(|s| s.len() > 1) {
        let least = set.first().expect("a name").clone();
        for n in set.iter().skip(1) {
            aliases.insert(n.clone(), least.clone());
        }
    }
    Elf { alloc, values, aliases }
}

impl Elf {
    fn exec_section(&self, a: u64) -> Option<(u64, u64)> {
        self.alloc.iter().find(|s| s.3 && a >= s.1 && a < s.1 + s.2).map(|s| (s.1, s.1 + s.2))
    }

    fn in_exec(&self, a: u64, end: u64) -> bool {
        self.exec_section(a).is_some_and(|(_, e)| end <= e)
    }

    fn in_alloc(&self, a: u64) -> bool {
        self.alloc.iter().any(|s| a >= s.1 && a <= s.1 + s.2)
    }
}

// ---------------------------------------------------------------------
// Normalization.

/// A transcript with the differences the module's rules explain taken
/// out: the output's path and the scratch directory, hex numbers, process
/// and thread ids, lldb's segment index, and alias names.
fn normalize(raw: &str, prog: &Path, dir: &Path, elf: &Elf) -> String {
    let raw = raw
        .replace(&prog.display().to_string(), "<prog>")
        .replace(&dir.display().to_string(), "<dir>");
    let mut out = String::new();
    for line in raw.lines() {
        let line = line.trim_end();
        // lldb reports a missing language plug-in on its event thread,
        // once, whenever that thread runs: it came in one of two runs on
        // the same kind of binary, unrelated to the linker.
        if line.contains("This version of LLDB has no plugin for the language") {
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0;
        let mut last_word = String::new();
        while i < chars.len() {
            let c = chars[i];
            if c == '0' && chars.get(i + 1) == Some(&'x') {
                let mut j = i + 2;
                while j < chars.len() && chars[j].is_ascii_hexdigit() {
                    j += 1;
                }
                out.push_str("0x?");
                i = j;
                last_word.clear();
                continue;
            }
            if c.is_ascii_alphabetic() || c == '_' {
                let mut j = i;
                while j < chars.len()
                    && (chars[j].is_ascii_alphanumeric() || matches!(chars[j], '_' | '.' | '$'))
                {
                    j += 1;
                }
                let word: String = chars[i..j].iter().collect();
                // A symbol a debugger names for an address: lldb's
                // `module`symbol`, gdb's `in symbol (` frame.
                let named = (i > 0 && chars[i - 1] == '`')
                    || (out.ends_with(" in ") && chars[j..].starts_with(&[' ', '(']));
                match elf.aliases.get(&word) {
                    Some(least) if named => out.push_str(least),
                    _ => out.push_str(&word),
                }
                if word.ends_with("PT_LOAD") && chars.get(j) == Some(&'[') {
                    // L-62: mortar's code is in a later segment.
                    let mut k = j + 1;
                    while k < chars.len() && chars[k].is_ascii_digit() {
                        k += 1;
                    }
                    out.push_str("[?");
                    i = k;
                    last_word.clear();
                    continue;
                }
                last_word = word;
                i = j;
                continue;
            }
            if c == ' '
                && matches!(last_word.as_str(), "process" | "Process" | "LWP")
                && chars.get(i + 1).is_some_and(char::is_ascii_digit)
            {
                let mut j = i + 1;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    j += 1;
                }
                out.push_str(" N");
                i = j;
                last_word.clear();
                continue;
            }
            if !c.is_whitespace() {
                last_word.clear();
            }
            out.push(c);
            i += 1;
        }
        out.push('\n');
    }
    out
}

/// Panics with the first differing lines when two transcripts differ.
fn same(label: &str, what: &str, ours: &str, theirs: &str) {
    if ours == theirs {
        return;
    }
    let (a, b): (Vec<&str>, Vec<&str>) = (ours.lines().collect(), theirs.lines().collect());
    let first = a.iter().zip(&b).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len()));
    let from = first.saturating_sub(3);
    panic!(
        "{label}: {what} differs at line {}:\n--- mortar\n{}\n--- GNU ld\n{}\n\
         === mortar, whole\n{ours}",
        first + 1,
        a[from..a.len().min(first + 8)].join("\n"),
        b[from..b.len().min(first + 8)].join("\n"),
    );
}

// ---------------------------------------------------------------------
// The sessions.

fn gdb_args(p: &Program, comdat: Option<u64>) -> Vec<String> {
    let mut ex: Vec<String> = Vec::new();
    let mut e = |s: &str| ex.push(s.to_owned());
    for s in ["set pagination off", "set width 0", "set confirm off", "set print pretty off"] {
        e(s);
    }
    if let Some(ctor) = p.ctor {
        e(&format!("break {ctor}"));
    }
    e(&format!("tbreak {}", p.deep.0));
    if p.throws {
        e("catch throw");
    }
    e("run > /dev/null 2>&1");
    if p.ctor.is_some() {
        e("bt");
        e("continue");
    }
    // A breakpoint in a non-main object, a backtrace through >= 3 frames.
    e("bt");
    e("print z");
    e("print g_count");
    if let Some(tls) = p.tls {
        e(&format!("print {tls}"));
    }
    e("next");
    e("print local3");
    e(&format!("info line {}", p.deep.0));
    e(&format!("list {}", p.deep.0));
    if let Some(a) = comdat {
        e(&format!("info symbol {a:#x}"));
    }
    e(&format!("tbreak {}", p.step.0));
    e("continue");
    // Step over the call to level3.
    for _ in 0..3 {
        e("next");
    }
    e("continue");
    e("bt");
    e("continue");
    let mut args: Vec<String> =
        ["-batch", "-nx", "-iex", "set debuginfod enabled off"].map(String::from).to_vec();
    for x in ex {
        args.push("-ex".into());
        args.push(x);
    }
    args
}

fn lldb_args(p: &Program, comdat: Option<u64>) -> Vec<String> {
    let mut o: Vec<String> = Vec::new();
    let mut e = |s: &str| o.push(s.to_owned());
    // Disabling ASLR needs `personality`, which a container may refuse.
    e("settings set target.disable-aslr false");
    if let Some(ctor) = p.ctor {
        e(&format!("breakpoint set -n {ctor}"));
    }
    e(&format!("breakpoint set -n {} -o true", p.deep.1));
    if p.throws {
        e("breakpoint set -E c++");
    }
    e("process launch -o /dev/null -e /dev/null");
    if p.ctor.is_some() {
        e("bt");
        e("continue");
    }
    e("bt");
    if p.lldb_lang {
        e("frame variable z");
        e("target variable g_count");
    }
    e("thread step-over");
    if p.lldb_lang {
        e("frame variable local3");
    }
    e(&format!("image lookup -n {}", p.deep.1));
    if p.lldb_lang {
        e(&format!("source list -n {}", p.deep.1));
    }
    if let Some(a) = comdat {
        e(&format!("image lookup -a {a:#x}"));
    }
    e(&format!("breakpoint set -n {} -o true", p.step.1));
    e("continue");
    for _ in 0..3 {
        e("thread step-over");
    }
    e("continue");
    // lldb's batch mode stops at a command that fails: `bt` after the
    // process has exited would, so only a program that stops again (at
    // the throw) has more.
    if p.throws {
        e("bt");
        e("continue");
    }
    let mut args: Vec<String> = vec!["-b".into(), "-x".into()];
    for x in o {
        args.push("-o".into());
        args.push(x);
    }
    args
}

/// The session's transcript on one output, normalized.
fn session(exe: &Path, args: &[String], prog: &Path, dir: &Path, elf: &Elf) -> String {
    let mut a = args.to_vec();
    a.push(prog.display().to_string());
    let o = tool_run(exe, &a, dir, Duration::from_mins(2));
    assert!(!o.timed_out(), "{} timed out on {}", exe.display(), prog.display());
    normalize(&text(&o), prog, dir, elf)
}

/// How many lines of `t` begin, after spaces, with each prefix.
fn count(t: &str, prefix: &str) -> usize {
    t.lines().filter(|l| l.trim_start().starts_with(prefix)).count()
}

/// Asserts that the sessions did what they were written for on mortar's
/// output (an equal pair of transcripts that both stop early proves
/// nothing).
fn sessions_did_their_work(label: &str, p: &Program, gc: bool, gdb: &str, lldb: &str) {
    let deep_file = p.sources.last().expect("a source").name;
    // gdb: a frame #2 and #3 with a file and line, the deep function hit.
    assert!(
        gdb.lines().any(|l| l.starts_with("#3 ") && l.contains(" at "))
            && gdb.contains(&format!("/{deep_file}:")),
        "{label}: gdb's backtrace from {} has fewer than 4 frames with lines:\n{gdb}",
        p.deep.0
    );
    assert!(gdb.contains("$3 = "), "{label}: gdb printed fewer than 3 values:\n{gdb}");
    if p.tls.is_some() && !gc {
        assert!(gdb.contains("$3 = 11"), "{label}: gdb did not read the TLS variable:\n{gdb}");
    } else if p.tls.is_some() {
        // Under --gc-sections GNU ld and mortar both collect the sections of
        // glibc's `_thread_db_*` descriptors (libc.a(pthread_create.o)'s
        // `.rodata`, which nothing references), and libthread_db then
        // cannot work in a static program: the transcripts are equal
        // (compared above), and this is what they say.
        assert!(
            gdb.contains("Cannot find thread-local variables on this target"),
            "{label}: gdb read TLS under --gc-sections:\n{gdb}"
        );
    }
    assert!(
        count(gdb, "Breakpoint ") + count(gdb, "Temporary breakpoint ") >= 2,
        "{label}: gdb stopped too few times:\n{gdb}"
    );
    if let Some(ctor) = p.ctor {
        assert!(
            gdb.contains(&format!("{ctor} (")),
            "{label}: gdb did not stop in the constructor:\n{gdb}"
        );
    }
    if p.throws {
        assert!(gdb.contains("Catchpoint "), "{label}: gdb's catch throw did not stop:\n{gdb}");
        let after = gdb.rsplit("exception thrown").next().unwrap_or("");
        assert!(
            after.lines().filter(|l| l.starts_with('#')).count() >= 4,
            "{label}: gdb's backtrace from the throw is short:\n{gdb}"
        );
    }
    assert!(gdb.contains("exited normally"), "{label}: the program did not finish under gdb");
    assert!(
        lldb.lines().any(|l| l.contains("frame #3: ") && l.contains(" at ")),
        "{label}: lldb's backtrace has fewer than 4 frames with lines:\n{lldb}"
    );
    assert!(
        lldb.contains("exited with status = 0"),
        "{label}: the program did not finish under lldb:\n{lldb}"
    );
    if p.throws {
        assert!(
            lldb.contains("stop reason = breakpoint 3."),
            "{label}: lldb's C++ exception breakpoint did not stop:\n{lldb}"
        );
    }
}

/// gdb's `info line` and lldb's `image lookup -n` of the COMDAT function
/// on mortar's output name the kept copy only (the first object's), once.
/// GNU ld redirects the discarded copies' debug information to the kept
/// copy, so its output answers with every copy (`spec/image.md` I-47, the
/// rejected redirect): these are not compared.
// spec: image.md I-45, I-47 (tombstones of discarded COMDAT copies)
fn comdat_kept_copy(label: &str, t: &Tools, pair: &Pair, sym: &str, kept: &str) {
    let gdb = |prog: &Path| {
        let args: Vec<String> = [
            "-batch",
            "-nx",
            "-iex",
            "set debuginfod enabled off",
            "-ex",
            &format!("info line '{sym}'"),
        ]
        .map(String::from)
        .into_iter()
        .chain([prog.display().to_string()])
        .collect();
        text(&tool_run(&t.gdb, &args, &pair.dir, Duration::from_mins(1)))
    };
    let lldb = |prog: &Path| {
        let args: Vec<String> = [
            "-b",
            "-x",
            "-o",
            &format!("image lookup -s {sym}"),
            "-o",
            "image lookup -n twice<int>",
        ]
        .map(String::from)
        .into_iter()
        .chain([prog.display().to_string()])
        .collect();
        text(&tool_run(&t.lldb, &args, &pair.dir, Duration::from_mins(1)))
    };
    let (g, l) = (gdb(&pair.ours), lldb(&pair.ours));
    assert!(
        g.contains(&format!("/{kept}\"")),
        "{label}: gdb's `info line` of the COMDAT function does not name the kept copy \
         ({kept}):\n{g}\nGNU ld's output:\n{}",
        gdb(&pair.theirs)
    );
    let lookup = l.rsplit("image lookup -n").next().unwrap_or("");
    assert!(
        lookup.contains("1 match found") && lookup.contains(&format!("at {kept}:")),
        "{label}: lldb finds the COMDAT function other than once, at the kept copy:\n{l}\n\
         GNU ld's output:\n{}",
        lldb(&pair.theirs)
    );
}

/// Builds `p` with `driver` over the flag matrix, links each with both
/// linkers, and runs `check` on each pair (with the label and the flag
/// set's tag). A missing tool skips, after a note.
fn each(name: &str, driver: Driver, p: &Program, check: impl Fn(&str, &str, &Tools, &Pair)) {
    each_gc(name, driver, p, false, check);
}

/// The `--gc-sections` variant of a program (WP-031): every function and
/// object in a section of its own, and one more source whose functions
/// nothing calls, so that both linkers collect code with debug
/// information. Fortran has no such source (gfortran's flags are the
/// same; the collected sections are the run-time library's).
fn gc_sources(p: &Program) -> Vec<Source> {
    let mut v = p.sources.to_vec();
    let unused = match p.sources.first().and_then(|s| Path::new(s.name).extension()) {
        Some(e) if e == "cpp" => Some(Source {
            name: "gc_unused.cpp",
            text: "struct Gone { int v; };\n\
                   __attribute__((noinline)) int gc_unused_throw(int x) { if (x) throw Gone{x}; return 1; }\n\
                   __attribute__((noinline)) int gc_unused(int x) { try { return gc_unused_throw(x); } \
                   catch (const Gone &g) { return g.v; } }\n\
                   int gc_unused_data[64] = {1};\n",
        }),
        Some(e) if e == "c" => Some(Source {
            name: "gc_unused.c",
            text: "__attribute__((noinline)) int gc_unused(int x) { return x * 5 + 1; }\n\
                   int gc_unused_data[64] = {1};\n",
        }),
        _ => None,
    };
    v.extend(unused);
    v
}

/// [`each`], with `--gc-sections` on both links when `gc`.
fn each_gc(
    name: &str,
    driver: Driver,
    p: &Program,
    gc: bool,
    check: impl Fn(&str, &str, &Tools, &Pair),
) {
    each_with(name, driver, p, gc, |t, label, built| link(t, label, built, gc), check);
}

/// [`each`] on the incremental lane ([`link_incr`]).
fn each_incr(name: &str, driver: Driver, p: &Program, check: impl Fn(&str, &str, &Tools, &Pair)) {
    each_with(name, driver, p, false, |_, label, built| link_incr(label, built), check);
}

/// Builds `p` over the flag matrix (the `--gc-sections` variant when
/// `gc`), makes each pair with `pair`, and runs `check` on it.
fn each_with(
    name: &str,
    driver: Driver,
    p: &Program,
    gc: bool,
    pair: impl Fn(&Tools, &str, &Built) -> Pair,
    check: impl Fn(&str, &str, &Tools, &Pair),
) {
    let Some(t) = tools() else { return };
    let sources = if gc { gc_sources(p) } else { p.sources.to_vec() };
    for (tag, flags) in FLAGS {
        let label = format!("{name}-{tag}");
        let scratch = Scratch::new(&label);
        let mut flags = flags.to_vec();
        if gc {
            flags.extend(["-ffunction-sections", "-fdata-sections"]);
        }
        let Some(built) = build(&scratch, driver, &flags, &sources) else {
            assert!(!tools_required(), "{} is required", driver.tool());
            eprintln!("note: {} is missing; skipping {name}", driver.tool());
            return;
        };
        let pair = pair(&t, &label, &built);
        check(&label, tag, &t, &pair);
    }
}

/// The gdb and lldb sessions on both outputs: equal transcripts, and the
/// sessions did their work.
// spec: VALIDATION.md L6 (debuggers: equal transcripts against GNU ld)
fn sessions(name: &str, driver: Driver, p: &Program) {
    sessions_gc(name, driver, p, false);
}

/// [`sessions`], with `--gc-sections` on both links when `gc`.
///
/// A session runs the program under gdb and lldb: a process of the host's
/// own architecture. Off an AArch64 host it is a named skip (the static
/// checks, [`dwarf`], run on every host).
fn sessions_gc(name: &str, driver: Driver, p: &Program, gc: bool) {
    if !AARCH64.is_host() {
        mortar_testutil::named_skip(
            &format!("debuggers::{name}"),
            "gdb and lldb sessions run the AArch64 program: on an AArch64 host only",
        );
        return;
    }
    each_gc(name, driver, p, gc, |label, _, t, pair| session_pair(label, t, pair, p, false));
}

/// The incremental lane's sessions: mortar's incremental-mode output
/// against its plain output, equal transcripts (WP-062: the neutral debug
/// units and the slack are invisible to gdb and lldb).
// spec: incr.md IN-50, IN-51b (debuggers read past the neutral units)
fn sessions_incr(name: &str, driver: Driver, p: &Program) {
    if !AARCH64.is_host() {
        mortar_testutil::named_skip(
            &format!("debuggers::{name}"),
            "gdb and lldb sessions run the AArch64 program: on an AArch64 host only",
        );
        return;
    }
    each_incr(name, driver, p, |label, _, t, pair| session_pair(label, t, pair, p, true));
}

/// lldb's `<section> + <offset>` with the offset masked: on the
/// incremental lane a function's offset in `.text` is its region's
/// (`spec/incr.md` IN-30: each file's code is followed by slack), so it
/// differs from the plain link's as its address does.
fn section_offsets_masked(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut rest = t;
    while let Some(at) = rest.find(" + ") {
        let (head, tail) = rest.split_at(at + 3);
        out.push_str(head);
        let digits = tail.chars().take_while(char::is_ascii_digit).count();
        let word = head[..head.len() - 3].rsplit(['(', ' ']).next();
        let in_section = word.is_some_and(|w| w.contains("PT_LOAD"));
        if digits > 0 && in_section {
            out.push('?');
            rest = &tail[digits..];
        } else {
            rest = tail;
        }
    }
    out.push_str(rest);
    out
}

/// The gdb and lldb sessions on one pair, compared; with
/// [`section_offsets_masked`] on the incremental lane (`incr`).
fn session_pair(label: &str, t: &Tools, pair: &Pair, p: &Program, incr: bool) {
    {
        let (eo, et) = (elf(&pair.ours), elf(&pair.theirs));
        let addr = |e: &Elf| p.comdat.map(|(sym, _)| e.values[sym]);
        let g_ours = session(&t.gdb, &gdb_args(p, addr(&eo)), &pair.ours, &pair.dir, &eo);
        let g_theirs = session(&t.gdb, &gdb_args(p, addr(&et)), &pair.theirs, &pair.dir, &et);
        let l_ours = session(&t.lldb, &lldb_args(p, addr(&eo)), &pair.ours, &pair.dir, &eo);
        let l_theirs = session(&t.lldb, &lldb_args(p, addr(&et)), &pair.theirs, &pair.dir, &et);
        // WP-027e: the all-ones tombstone in a `DW_RLE_start_length`
        // entry made gdb drop the CU's ranges, and `next` then said this.
        assert!(
            !g_ours.contains("not in symtab"),
            "{label}: gdb lost a CU's ranges (spec/image.md I-47):\n{g_ours}"
        );
        same(label, "the gdb session", &g_ours, &g_theirs);
        if incr {
            let (a, b) = (section_offsets_masked(&l_ours), section_offsets_masked(&l_theirs));
            same(label, "the lldb session", &a, &b);
        } else {
            same(label, "the lldb session", &l_ours, &l_theirs);
        }
        sessions_did_their_work(label, p, pair.gc, &g_ours, &l_ours);
        if let Some((sym, kept)) = p.comdat {
            comdat_kept_copy(label, t, pair, sym, kept);
        }
    }
}

// ---------------------------------------------------------------------
// The static checks.

/// The last `0x...` number in `s`.
fn last_hex(s: &str) -> Option<u64> {
    let at = s.rfind("0x")?;
    let digits: String = s[at + 2..].chars().take_while(char::is_ascii_hexdigit).collect();
    u64::from_str_radix(&digits, 16).ok()
}

fn dump(t: &Tools, prog: &Path, dir: &Path, args: &[&str]) -> String {
    let mut a: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
    a.push(prog.display().to_string());
    let o = tool_run(&t.dwarfdump, &a, dir, Duration::from_mins(1));
    assert!(o.success(), "llvm-dwarfdump {args:?} {}: {}", prog.display(), o.stderr_text());
    o.stdout_text()
}

/// `(name, low_pc, high_pc)` of every `DW_TAG_subprogram` with a
/// `DW_AT_low_pc`, from `llvm-dwarfdump --debug-info`.
fn subprograms(info: &str) -> Vec<(String, u64, Option<u64>)> {
    let mut out = Vec::new();
    let mut cur: Option<(String, Option<u64>, Option<u64>)> = None;
    let flush = |c: &mut Option<(String, Option<u64>, Option<u64>)>, out: &mut Vec<_>| {
        if let Some((n, Some(lo), hi)) = c.take() {
            out.push((n, lo, hi));
        }
    };
    for line in info.lines() {
        let l = line.trim_start();
        if l.starts_with("0x") && l.contains(": ") && l.contains("DW_TAG_") || l == "NULL" {
            flush(&mut cur, &mut out);
            if l.ends_with("DW_TAG_subprogram") {
                cur = Some((String::new(), None, None));
            }
            continue;
        }
        let Some((n, lo, hi)) = cur.as_mut() else { continue };
        let attr = l.split_whitespace().next().unwrap_or("");
        // llvm-dwarfdump prints the tombstone as `dead code`.
        // A `.debug_addr` entry (`DW_FORM_addrx`) holds DEAD_PAIRED.
        let value = || {
            if l.contains("dead code") {
                Some(DEAD)
            } else {
                last_hex(l).map(|v| if v == DEAD_PAIRED { DEAD } else { v })
            }
        };
        match attr {
            "DW_AT_low_pc" => *lo = value(),
            "DW_AT_high_pc" => *hi = last_hex(l),
            "DW_AT_linkage_name"
            | "DW_AT_name"
            | "DW_AT_specification"
            | "DW_AT_abstract_origin"
                if n.is_empty() =>
            {
                if let Some(q) = l.split('"').nth(1) {
                    q.clone_into(n);
                }
            }
            _ => {}
        }
    }
    flush(&mut cur, &mut out);
    out
}

/// The address facts of one output's DWARF: the tombstones it holds and
/// what lies outside the sections it should lie in.
#[derive(Debug, Default)]
struct DebugAddrs {
    /// Names of subprograms whose `DW_AT_low_pc` is the tombstone.
    dead_subprograms: Vec<String>,
    /// Tombstoned entries by section.
    dead: BTreeMap<&'static str, usize>,
    /// Addresses that are neither a tombstone nor where they belong.
    wrong: Vec<String>,
}

/// Reads the address-holding debug sections of `prog`.
fn debug_addrs(tools: &Tools, pair: &Pair, prog: &Path, img: &Elf) -> DebugAddrs {
    let mut found = DebugAddrs::default();
    let dir = &pair.dir;
    let info = dump(tools, prog, dir, &["--debug-info"]);
    let subs = subprograms(&info);
    let live: BTreeSet<&str> = subs.iter().filter(|s| s.1 != DEAD).map(|s| s.0.as_str()).collect();
    for (name, lo, hi) in &subs {
        if *lo == DEAD {
            found.dead_subprograms.push(name.clone());
            *found.dead.entry(".debug_info").or_default() += 1;
            // Under --gc-sections a collected function has no copy at all.
            if !pair.gc && !live.contains(name.as_str()) {
                found.wrong.push(format!("tombstoned subprogram {name} has no kept copy"));
            }
        } else if !img.in_exec(*lo, hi.unwrap_or(*lo)) {
            found.wrong.push(format!("subprogram {name} [{lo:#x}, {hi:x?}) outside its section"));
        }
    }
    // .debug_aranges: `[start, end)` tuples (I-47, I-112: all ones and
    // the length 0).
    for l in dump(tools, prog, dir, &["--debug-aranges"]).lines() {
        let Some(r) = l.strip_prefix('[') else { continue };
        let Some((a, b)) = r.trim_end_matches(')').split_once(", ") else { continue };
        let (a, b) = (last_hex(a).unwrap_or(0), last_hex(b).unwrap_or(0));
        if a == DEAD && b == DEAD {
            *found.dead.entry(".debug_aranges").or_default() += 1;
        } else if !img.in_exec(a, b) {
            found.wrong.push(format!(".debug_aranges [{a:#x}, {b:#x})"));
        }
    }
    // .debug_ranges (DWARF 4): `offset begin end`, 1 for both ends of a
    // dead entry (I-47's first row). The entries may be offsets from the
    // CU's base (Clang), so where they lie is judged on the resolved
    // lists below.
    for l in dump(tools, prog, dir, &["--debug-ranges"]).lines() {
        let w: Vec<&str> = l.split_whitespace().collect();
        if w.len() == 3 && w[0].len() == 8 && w[1] == "0000000000000001" && w[2] == w[1] {
            *found.dead.entry(".debug_ranges").or_default() += 1;
        }
    }
    // Every `DW_AT_ranges` list as llvm-dwarfdump resolves it (from
    // .debug_ranges or .debug_rnglists, with the CU's base): each range
    // empty, starting at the tombstone (a dead entry, whose end wraps),
    // or in code.
    let mut in_ranges = false;
    for l in info.lines() {
        let l = l.trim();
        if l.starts_with("DW_AT_ranges") {
            in_ranges = true;
            continue;
        }
        if !in_ranges {
            continue;
        }
        let Some(r) = l.strip_prefix('[') else {
            in_ranges = false;
            continue;
        };
        if l.ends_with("))") {
            in_ranges = false;
        }
        let Some((a, b)) = r.split_once(", ") else { continue };
        let (a, b) = (last_hex(a).unwrap_or(0), last_hex(b).unwrap_or(0));
        if a != b && a != DEAD && a != DEAD_PAIRED && !img.in_exec(a, b) {
            found.wrong.push(format!("DW_AT_ranges [{a:#x}, {b:#x})"));
        }
    }
    // .debug_rnglists (DWARF 5): the address operands (GCC's
    // `DW_RLE_start_length`/`start_end`/`base_address`); Clang's indexed
    // forms are judged in .debug_addr.
    for l in dump(tools, prog, dir, &["--debug-rnglists", "--verbose"]).lines() {
        let Some((kind, ops)) = l.split_once("]:") else { continue };
        let ops = ops.split("=>").next().unwrap_or("");
        let vals: Vec<u64> = ops.split(',').filter_map(last_hex).collect();
        let kind = kind.rsplit('[').next().unwrap_or("").trim();
        let first = vals.first().copied().unwrap_or(0);
        match kind {
            "DW_RLE_start_length" | "DW_RLE_start_end" | "DW_RLE_base_address" => {
                if first == DEAD_PAIRED {
                    *found.dead.entry(".debug_rnglists").or_default() += 1;
                } else {
                    let end = match kind {
                        "DW_RLE_start_length" => first + vals.get(1).copied().unwrap_or(0),
                        "DW_RLE_start_end" => vals.get(1).copied().unwrap_or(first),
                        _ => first,
                    };
                    if !img.in_exec(first, end) {
                        found.wrong.push(format!(".debug_rnglists {}", l.trim()));
                    }
                }
            }
            _ => {}
        }
    }
    // .debug_addr: every entry an allocated address or the tombstone.
    let addr = dump(tools, prog, dir, &["--debug-addr"]);
    for l in addr.lines().filter(|l| l.starts_with("0x") && l.len() == 18) {
        let a = last_hex(l).unwrap_or(0);
        if a == DEAD_PAIRED {
            *found.dead.entry(".debug_addr").or_default() += 1;
        } else if !img.in_alloc(a) {
            found.wrong.push(format!(".debug_addr {l}"));
        }
    }
    // .debug_line: every `DW_LNE_set_address` in code or the tombstone.
    for l in dump(tools, prog, dir, &["--debug-line", "--verbose"]).lines() {
        if !l.contains("DW_LNE_set_address") {
            continue;
        }
        let a = last_hex(l).unwrap_or(0);
        if a == DEAD {
            *found.dead.entry(".debug_line").or_default() += 1;
        } else if img.exec_section(a).is_none() {
            found.wrong.push(format!(".debug_line {}", l.trim()));
        }
    }
    found
}

/// The complaints of `llvm-dwarfdump --verify`, normalized, or `None`
/// when it did not finish in time.
fn verify(t: &Tools, prog: &Path, dir: &Path, e: &Elf) -> Option<BTreeSet<String>> {
    let args = vec!["--verify".to_owned(), prog.display().to_string()];
    let o = tool_run(&t.dwarfdump, &args, dir, Duration::from_secs(20));
    if o.timed_out() {
        return None;
    }
    let all = normalize(&text(&o), prog, dir, e);
    Some(
        all.lines()
            .filter(|l| l.starts_with("error:") || l.starts_with("warning:"))
            .filter(|l| !l.contains("Aggregated error counts"))
            .map(str::to_owned)
            .collect(),
    )
}

/// What gdb says of an output's symbol tables without running it.
struct Symtabs {
    /// `maint check-symtabs`'s findings.
    check: Vec<String>,
    /// gdb's complaints while it reads the debug information (`set
    /// complaints`), normalized.
    complaints: BTreeSet<String>,
    /// `info functions -n` as `(file:line, signature)`: functions with
    /// debug information. The non-debugging symbols are left out: they
    /// are every library symbol and say nothing of the DWARF.
    functions: BTreeSet<(String, String)>,
}

fn gdb_symtabs(t: &Tools, prog: &Path, dir: &Path, e: &Elf) -> Symtabs {
    let args: Vec<String> = [
        "-batch",
        "-nx",
        "-iex",
        "set debuginfod enabled off",
        "-iex",
        "set complaints 10000",
        "-ex",
        "set width 0",
        "-ex",
        "maint check-symtabs",
        "-ex",
        "echo @@functions\\n",
        "-ex",
        "info functions -n",
    ]
    .map(String::from)
    .into_iter()
    .chain([prog.display().to_string()])
    .collect();
    let o = tool_run(&t.gdb, &args, dir, Duration::from_mins(1));
    let all = normalize(&text(&o), prog, dir, e);
    let complaint = |l: &str| l.starts_with("During symbol reading");
    let complaints = all.lines().filter(|l| complaint(l)).map(str::to_owned).collect();
    let (check, functions) = all.split_once("@@functions\n").unwrap_or((&all, ""));
    let check =
        check.lines().filter(|l| !l.is_empty() && !complaint(l)).map(str::to_owned).collect();
    let mut file = String::new();
    let mut set = BTreeSet::new();
    for l in functions.lines() {
        if let Some(f) = l.strip_prefix("File ") {
            f.trim_end_matches(':').clone_into(&mut file);
        } else if let Some((n, sig)) = l.split_once(":\t")
            && n.chars().all(|c| c.is_ascii_digit())
        {
            set.insert((format!("{file}:{n}"), sig.to_owned()));
        }
    }
    Symtabs { check, complaints, functions: set }
}

/// gdb's complaint when a line sequence starts at the tombstone: gdb 16
/// recognizes all ones as a dead address and drops the sequence, which
/// is I-47's purpose (the message says 0 for both).
const LINE_TOMBSTONE: &str = "During symbol reading: .debug_line address at offset 0x? is 0";

/// gdb's complaint at an optimized call site (`DW_AT_call_target` /
/// `DW_AT_call_origin`) whose target is its own CU's DIE of a discarded
/// COMDAT copy: that DIE's `low_pc` is the tombstone (I-47), which gdb
/// drops as I-47 intends, so the edge has no address. GNU ld's redirect
/// gives it the kept copy's. Effect: gdb cannot take entry values
/// (`x@entry`) through such an edge, at GCC and Clang `-O2` only.
const CALL_TARGET_DEAD: &str =
    "During symbol reading: DW_AT_call_target target DIE has invalid low pc";

/// gdb's complaint when a range list entry starts at [`DEAD_PAIRED`]:
/// gdb drops the entry (it has no section at 0), which is I-47's purpose.
/// lld's and mold's outputs give it too.
const RNGLISTS_ZERO: &str =
    "During symbol reading: .debug_rnglists entry has start address of zero";

/// gdb's complaint when a range list wraps, as all ones in a
/// `DW_RLE_start_length` entry did before WP-027e: gdb then drops the
/// CU's whole list. Never on mortar's output.
const RNGLISTS_INVERTED: &str =
    "During symbol reading: Invalid .debug_rnglists data (inverted range)";

/// The static checks on both outputs of each configuration.
fn dwarf(name: &str, driver: Driver, p: &Program) {
    each(name, driver, p, |label, _, t, pair| {
        let (eo, et) = (elf(&pair.ours), elf(&pair.theirs));
        verify_against(label, t, pair, &eo, &et);
        symtabs_against(label, t, pair, (&eo, &et));
        addresses(label, t, pair, p, driver == Driver::Gxx, (&eo, &et));
    });
}

/// The incremental lane's static checks: `llvm-dwarfdump --verify`,
/// gdb's symbol tables and complaints, and the subprogram addresses of
/// mortar's incremental-mode output, held to its plain output's.
// spec: incr.md IN-50, IN-51b (the neutral units: verify-clean)
fn dwarf_incr(name: &str, driver: Driver, p: &Program) {
    each_incr(name, driver, p, |label, _, t, pair| {
        let (eo, et) = (elf(&pair.ours), elf(&pair.theirs));
        verify_against(label, t, pair, &eo, &et);
        symtabs_against(label, t, pair, (&eo, &et));
        // Every address in range or a tombstone, and the same tombstones
        // as the plain link's (both are mortar's, I-47).
        let ours = debug_addrs(t, pair, &pair.ours, &eo);
        let theirs = debug_addrs(t, pair, &pair.theirs, &et);
        assert!(ours.wrong.is_empty(), "{label}: the incremental output: {:#?}", ours.wrong);
        assert_eq!(ours.dead, theirs.dead, "{label}: tombstones");
        assert_eq!(
            ours.dead_subprograms, theirs.dead_subprograms,
            "{label}: tombstoned subprograms"
        );
    });
}

/// The static checks under `--gc-sections` (WP-031, `spec/image.md`
/// I-47): `llvm-dwarfdump --verify` and gdb's complaints as without it;
/// every address in mortar's debug sections is in its section or the
/// tombstone (GNU ld writes 0 for a collected function, `E14 Q13`, so its
/// output is not held to that); the unused functions' subprograms are
/// tombstoned, and no address 0 claims code.
fn dwarf_gc(name: &str, driver: Driver, p: &Program) {
    each_gc(name, driver, p, true, |label, _, t, pair| {
        let (eo, et) = (elf(&pair.ours), elf(&pair.theirs));
        verify_against(label, t, pair, &eo, &et);
        symtabs_against(label, t, pair, (&eo, &et));
        let ours = debug_addrs(t, pair, &pair.ours, &eo);
        assert!(ours.wrong.is_empty(), "{label}: mortar's output: {:#?}", ours.wrong);
        if p.sources.first().is_some_and(|s| Path::new(s.name).extension() != Some("f90".as_ref()))
        {
            let gone = ours.dead_subprograms.iter().filter(|n| n.contains("gc_unused")).count();
            assert!(gone >= 1, "{label}: tombstoned subprograms {:?}", ours.dead_subprograms);
            assert!(!eo.values.contains_key("gc_unused"), "{label}: gc_unused is in the output");
        }
    });
}

/// `llvm-dwarfdump --verify`: mortar's complaints are a subset of GNU
/// ld's output's (GNU ld's is not clean everywhere: `.debug_str_offsets`
/// errors for Clang DWARF 5, `spec/image.md` I-47's E16 table; and GCC's
/// objects carry complaints of their own, such as "Call site nested entry
/// within inlined subroutine", which both outputs inherit).
///
/// llvm-dwarfdump 19 and 22 do not finish `--verify` (20 s limit) on the
/// outputs of GCC 14's `-O2` programs here, C and C++, DWARF 4 and 5; a
/// lone GCC `-O2` C++ object with a `try`/`catch` already hangs it,
/// before any link. When GNU ld's output times out the check is skipped
/// with a note (4 of the 20 configurations).
// spec: VALIDATION.md L6 (`llvm-dwarfdump --verify` on every -g output)
fn verify_against(label: &str, t: &Tools, pair: &Pair, eo: &Elf, et: &Elf) {
    let Some(theirs) = verify(t, &pair.theirs, &pair.dir, et) else {
        eprintln!("note: {label}: llvm-dwarfdump --verify does not finish on GNU ld's output");
        return;
    };
    let ours = verify(t, &pair.ours, &pair.dir, eo)
        .unwrap_or_else(|| panic!("{label}: llvm-dwarfdump --verify timed out on mortar's output"));
    let extra: Vec<&String> = ours.difference(&theirs).collect();
    assert!(
        extra.is_empty(),
        "{label}: llvm-dwarfdump --verify complains of mortar's output only:\n{extra:#?}"
    );
}

/// gdb's `maint check-symtabs` is silent on mortar's output; gdb's
/// complaints while reading mortar's debug information are GNU ld's
/// output's, but for [`LINE_TOMBSTONE`], [`CALL_TARGET_DEAD`] and
/// [`RNGLISTS_ZERO`] (I-47), and never [`RNGLISTS_INVERTED`]; and `info functions -n`
/// lists the same functions as on GNU ld's output. GNU ld's lists a
/// COMDAT function once per object that defined a copy (the redirect,
/// `spec/image.md` I-47); mortar's once, under the kept copy's file: the
/// signature sets are equal and mortar's entries are a subset of GNU's.
// spec: VALIDATION.md L6; image.md I-47 (tombstones)
fn symtabs_against(label: &str, t: &Tools, pair: &Pair, (eo, et): (&Elf, &Elf)) {
    let ours = gdb_symtabs(t, &pair.ours, &pair.dir, eo);
    let theirs = gdb_symtabs(t, &pair.theirs, &pair.dir, et);
    let check: Vec<&String> =
        ours.check.iter().filter(|l| !l.contains("Error disabling address space")).collect();
    assert!(check.is_empty(), "{label}: maint check-symtabs: {check:#?}");
    let extra: Vec<&String> = ours
        .complaints
        .difference(&theirs.complaints)
        .filter(|c| !c.starts_with(LINE_TOMBSTONE) && !c.starts_with(CALL_TARGET_DEAD))
        .filter(|c| !c.starts_with(RNGLISTS_ZERO))
        .collect();
    assert!(extra.is_empty(), "{label}: gdb complains of mortar's output only: {extra:#?}");
    assert!(
        !ours.complaints.iter().any(|c| c.starts_with(RNGLISTS_INVERTED)),
        "{label}: gdb rejects a range list of mortar's output: {:#?}",
        ours.complaints
    );
    let (fo, ft) = (&ours.functions, &theirs.functions);
    assert!(!fo.is_empty(), "{label}: gdb lists no function with debug information");
    let sigs = |s: &BTreeSet<(String, String)>| -> BTreeSet<String> {
        s.iter().map(|(_, sig)| sig.clone()).collect()
    };
    assert_eq!(sigs(fo), sigs(ft), "{label}: info functions -n: the functions differ");
    let extra: Vec<_> = fo.difference(ft).collect();
    assert!(extra.is_empty(), "{label}: info functions -n lists, for mortar only: {extra:#?}");
}

/// Subprogram addresses and tombstones (`spec/image.md` I-45, I-47,
/// I-112): on both outputs every `DW_AT_low_pc` of a kept subprogram lies
/// in an executable output section, with its `DW_AT_high_pc`, and every
/// address in `.debug_aranges`, `.debug_ranges`, `.debug_rnglists`,
/// `.debug_addr` and `.debug_line` is in its section or a tombstone. On
/// mortar's output, a C++ program's discarded copies of `twice<int>` are
/// tombstoned in each of the sections the compiler wrote (all ones, or 0
/// where a DWARF 5 entry pairs the address with a length). GNU ld
/// redirects the copies (I-47), so its output has no tombstone to count.
// spec: image.md I-45, I-47, I-112 (tombstones); VALIDATION.md L6
fn addresses(label: &str, t: &Tools, pair: &Pair, p: &Program, gcc: bool, (eo, et): (&Elf, &Elf)) {
    let ours = debug_addrs(t, pair, &pair.ours, eo);
    let theirs = debug_addrs(t, pair, &pair.theirs, et);
    assert!(ours.wrong.is_empty(), "{label}: mortar's output: {:#?}", ours.wrong);
    assert!(theirs.wrong.is_empty(), "{label}: GNU ld's output (the check): {:#?}", theirs.wrong);
    if p.comdat.is_none() {
        assert!(ours.dead.is_empty(), "{label}: tombstones without COMDAT: {:?}", ours.dead);
        return;
    }
    // Three objects define twice<int>; two copies are discarded.
    let twice = ours.dead_subprograms.iter().filter(|n| n.contains("twice")).count();
    assert_eq!(twice, 2, "{label}: tombstoned subprograms {:?}", ours.dead_subprograms);
    for (section, n) in &ours.dead {
        assert!(*n >= 1, "{label}: {section}");
    }
    // The compilers' choices: GCC writes .debug_aranges, and .debug_ranges
    // (DWARF 4) or .debug_rnglists (DWARF 5) for the CU; Clang DWARF 5
    // indexes .debug_addr. Every CU's line program has a sequence for the
    // COMDAT section.
    assert_eq!(ours.dead.get(".debug_line"), Some(&2), "{label}: {:?}", ours.dead);
    let dwarf5 = label.contains("d5");
    if gcc {
        assert_eq!(ours.dead.get(".debug_aranges"), Some(&2), "{label}: {:?}", ours.dead);
        let ranges = if dwarf5 { ".debug_rnglists" } else { ".debug_ranges" };
        assert_eq!(ours.dead.get(ranges), Some(&2), "{label}: {:?}", ours.dead);
    } else if dwarf5 {
        assert_eq!(ours.dead.get(".debug_addr"), Some(&2), "{label}: {:?}", ours.dead);
    }
    assert!(
        theirs.dead.is_empty(),
        "{label}: GNU ld's output has tombstones (I-47 says it redirects): {:?}",
        theirs.dead
    );
}

// ---------------------------------------------------------------------
// The tests.

// spec: VALIDATION.md L6
#[test]
fn sessions_c_gcc() {
    sessions("dbg-c-gcc", Driver::Gcc, &C);
}

// spec: VALIDATION.md L6
#[test]
fn sessions_c_clang() {
    sessions("dbg-c-clang", Driver::Clang, &C);
}

// spec: VALIDATION.md L6; image.md I-47
#[test]
fn sessions_cxx_gxx() {
    sessions("dbg-cxx-gxx", Driver::Gxx, &CXX);
}

// spec: VALIDATION.md L6; image.md I-47
#[test]
fn sessions_cxx_clangxx() {
    sessions("dbg-cxx-clangxx", Driver::Clangxx, &CXX);
}

// spec: VALIDATION.md L6
#[test]
fn sessions_fortran_gfortran() {
    sessions("dbg-f-gfortran", Driver::Gfortran, &FORTRAN);
}

// spec: VALIDATION.md L6; image.md I-45, I-47, I-112
#[test]
fn dwarf_c_gcc() {
    dwarf("dwarf-c-gcc", Driver::Gcc, &C);
}

// spec: VALIDATION.md L6; image.md I-45, I-47, I-112
#[test]
fn dwarf_c_clang() {
    dwarf("dwarf-c-clang", Driver::Clang, &C);
}

// spec: VALIDATION.md L6; image.md I-45, I-47, I-112
#[test]
fn dwarf_cxx_gxx() {
    dwarf("dwarf-cxx-gxx", Driver::Gxx, &CXX);
}

// spec: VALIDATION.md L6; image.md I-45, I-47, I-112
#[test]
fn dwarf_cxx_clangxx() {
    dwarf("dwarf-cxx-clangxx", Driver::Clangxx, &CXX);
}

// spec: VALIDATION.md L6; image.md I-45, I-47, I-112
#[test]
fn dwarf_fortran_gfortran() {
    dwarf("dwarf-f-gfortran", Driver::Gfortran, &FORTRAN);
}

// The --gc-sections variants (WP-031): the same sessions against GNU ld
// with --gc-sections, and the static checks.

// spec: VALIDATION.md L6; image.md I-6 to I-10, I-47
#[test]
fn gc_sessions_c_gcc() {
    sessions_gc("gc-dbg-c-gcc", Driver::Gcc, &C, true);
}

// spec: VALIDATION.md L6; image.md I-6 to I-10, I-47
#[test]
fn gc_sessions_cxx_gxx() {
    sessions_gc("gc-dbg-cxx-gxx", Driver::Gxx, &CXX, true);
}

// spec: VALIDATION.md L6; image.md I-6 to I-10, I-47
#[test]
fn gc_sessions_cxx_clangxx() {
    sessions_gc("gc-dbg-cxx-clangxx", Driver::Clangxx, &CXX, true);
}

// spec: VALIDATION.md L6; image.md I-6 to I-10, I-47
#[test]
fn gc_dwarf_c_clang() {
    dwarf_gc("gc-dwarf-c-clang", Driver::Clang, &C);
}

// spec: VALIDATION.md L6; image.md I-6 to I-10, I-47
#[test]
fn gc_dwarf_cxx_gxx() {
    dwarf_gc("gc-dwarf-cxx-gxx", Driver::Gxx, &CXX);
}

// spec: VALIDATION.md L6; image.md I-6 to I-10, I-47
#[test]
fn gc_dwarf_cxx_clangxx() {
    dwarf_gc("gc-dwarf-cxx-clangxx", Driver::Clangxx, &CXX);
}

// spec: VALIDATION.md L6; image.md I-6 to I-10, I-47
#[test]
fn gc_dwarf_fortran_gfortran() {
    dwarf_gc("gc-dwarf-f-gfortran", Driver::Gfortran, &FORTRAN);
}

// ---------------------------------------------------------------------
// The incremental lane (WP-062): mortar's incremental-mode first link
// against its plain link.

#[test]
fn incr_sessions_c_gcc() {
    sessions_incr("incr-dbg-c-gcc", Driver::Gcc, &C);
}

#[test]
fn incr_sessions_cxx_gxx() {
    sessions_incr("incr-dbg-cxx-gxx", Driver::Gxx, &CXX);
}

#[test]
fn incr_sessions_cxx_clangxx() {
    sessions_incr("incr-dbg-cxx-clangxx", Driver::Clangxx, &CXX);
}

#[test]
fn incr_dwarf_c_gcc() {
    dwarf_incr("incr-dwarf-c-gcc", Driver::Gcc, &C);
}

#[test]
fn incr_dwarf_cxx_gxx() {
    dwarf_incr("incr-dwarf-cxx-gxx", Driver::Gxx, &CXX);
}

#[test]
fn incr_dwarf_cxx_clangxx() {
    dwarf_incr("incr-dwarf-cxx-clangxx", Driver::Clangxx, &CXX);
}

#[test]
fn incr_dwarf_fortran_gfortran() {
    dwarf_incr("incr-dwarf-f-gfortran", Driver::Gfortran, &FORTRAN);
}

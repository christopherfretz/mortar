//! G-1 on BYTES (WP-228 B3; `spec/incr.md` IN-50..IN-57, IN-51b, IN-78):
//! the debug placement's decisions (`mortar_incr::update::debug`) written
//! into an output and read by debuggers, against the FULL link of the same
//! new inputs.
//!
//! **The test-only writer** (never in a shipped path). For an edit of one
//! file `b.c` that leaves its allocated bytes alone (a debug-only edit:
//! line shifts, a renamed local, an added type), from:
//! - `P`: the incremental-mode full link of the old inputs, its capture's
//!   rows (`MemoryState`, the real format-5 producer) and its layout `L0`;
//! - `F`: the incremental-mode full link of the NEW inputs (the judge) and
//!   its capture (the changed file's `SECTIONS` rows: its `ChangedFile`);
//!
//! it runs the update's debug steps on the real rows ([`debug::strings`]
//! over the capture's `DEBUG_STRS` rows (WP-247), or, in one test, a state
//! without the table; `place` on the delta
//! `layout(debug: true)` gives for a debug-only edit: the stored placement,
//! the changed file's old debug rows in `killed`), then writes the image of
//! the NEW inputs with the update's placement (IN-78's "full link with the
//! update's placement": `L0` with the changed file's debug sections where
//! `place` put them, the holes of `DebugPlan::chunks`, the merged strings
//! at `DebugStrings::output_offset` and its appended tail), so that the
//! changed file's debug sections are relocated by the full link's own
//! code. The update's output `U` is then a COPY of `P` with exactly what an
//! update writes: each placed contribution's bytes, every hole of a touched
//! debug chunk (IN-51b's forms, `mortar_image::neutral`), the appended
//! strings, `.debug_aranges` from [`debug::aranges`] (checked equal to the
//! image's own, full-link order), and the section header table. Every
//! other byte of `U` is `P`'s; the test checks that the image with the
//! update's placement agrees with `P` on every allocated byte (the edit is
//! debug-only) and on every unchanged file's debug contribution.
//!
//! Judged: gdb and lldb (AArch64: run, breakpoints by line in the changed
//! and an unchanged file, backtrace, locals, a global, `ptype`; x86-64:
//! the same questions without running, this host being AArch64),
//! `llvm-dwarfdump --verify`, `readelf --debug-dump` (no warning `F` does
//! not have, beyond IN-51b's accepted ones). `MORTAR_WP228_KEEP=<dir>`
//! keeps each `U`, `F` and their maps there for `cargo xtask plumb`.
//!
//! Several changed files are taken (`Edit::more`, in link order), C, C++
//! or Fortran (`f.f90`, GCC). A file the new link no longer has (IN-52) is
//! NOT written on bytes: removing it is not a debug-only edit (its code,
//! `.eh_frame` records and symbols leave the allocated image: the image of
//! the old layout without it fails the layout's rules, `eh_records` out
//! of range, B4), so its bytes need the join's allocated update; IN-52 is
//! judged by the decision test `removed_file_contributions_neutral`, by
//! the B3-7 refusal here (`dwarf4_split_…`, with a removed file) and by
//! the emulation (`place.py`) through the verifier.

mod support;

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mortar::front::{Env, front};
use mortar::incremental::{Rows5Sink, State5};
use mortar::timing::Timer;
use mortar_core::column::Column;
use mortar_core::incr::{LayoutDelta, StoredLayout};
use mortar_core::inputs::Reader;
use mortar_core::layout::{HoleKind, Layout, MergeMap, Place};
use mortar_core::par::Pool;
use mortar_core::{ChunkId, FileId, SectionId};
use mortar_diag::{Collector, Policy};
use mortar_incr::format::{SectionRow, Str};
use mortar_incr::state5::LayoutOf;
use mortar_incr::update::debug::{self, DebugPlan, Taken};
use mortar_incr::update::rows::name_hash;
use mortar_incr::update::testing::MemoryState;
use mortar_incr::update::view::{Change, ChangedFile, ChangedFiles, OdrRows};
use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, run_with, tool, tools_required};
use support::{Source, build_by, with_output};

// ---- the programs ------------------------------------------------------------

/// `a.c` (never edited), with [`BULK`]'s type before it: enough debug
/// bytes that the debug sections' tail reserves (`size/32`, IN-33) hold a
/// moved contribution and the appended strings.
const A_C: &str = r#"#include <stdio.h>
struct pt { int x, y; };
int g_count = 3;
int area(struct pt *p);
int helper(int v) {
    int local_a = v * 2;
    return local_a + g_count; /*A*/
}
int main(int argc, char **argv) {
    struct pt p = { argc, 4 };
    int r = area(&p);
    printf("%d %d %s\n", r, helper(r), argv[0] ? "" : "x");
    return 0;
}
"#;

/// The type that makes `a.c`'s debug information large (1500 members).
fn bulk() -> String {
    let mut t = String::from("struct bulk_of_a {\n");
    for i in 0..1500 {
        use std::fmt::Write as _;
        writeln!(t, "    long a_member_field_number_{i:04};").unwrap();
    }
    t + "};\n"
}

fn a_c() -> String {
    bulk() + A_C
}

/// `b.c`, before every edit: `area` and a small type of its own.
const B_OLD: &str = r"struct pt { int x, y; };
struct small_b { long s1, s2; };
static int scale = 5;
int area(struct pt *p) {
    int w = p->x + scale;
    int h = p->y;
    return w * h; /*B*/
}
";

/// One edit that leaves the allocated bytes alone: `b` (and the other
/// files of `more`) changed, or removed.
#[derive(Debug, Clone, Copy)]
struct Edit {
    name: &'static str,
    /// The old `b.c`.
    b_old: &'static str,
    /// The new `b.c`.
    b: &'static str,
    /// The placement the edit's `.debug_info` must take, if the test pins it.
    info: Option<Taken>,
    /// More files after `b` on the link line: (stem, old text, new text or
    /// `None` for a file the new link no longer has, IN-52), and the
    /// placement its `.debug_info` must take if pinned.
    more: &'static [(&'static str, &'static str, Option<&'static str>, Option<Taken>)],
    /// Extra compiler flags.
    flags: &'static [&'static str],
    /// Extra gdb questions (asked without running).
    gdb: &'static [&'static str],
    /// The languages it runs in (C, C++).
    langs: &'static [bool],
    /// The placement `b`'s `.debug_addr` must take, if the test pins it.
    addr: Option<Taken>,
}

const C_ONLY: &[bool] = &[false];

const LINES: Edit = Edit {
    name: "lines",
    b_old: B_OLD,
    more: &[],
    flags: &[],
    gdb: &[],
    langs: &[false, true],
    addr: None,
    b: r"struct pt { int x, y; };
struct small_b { long s1, s2; };
/* three
   more
   lines */
static int scale = 5;
int area(struct pt *p) {
    int w = p->x + scale;
    int h = p->y;
    return w * h; /*B*/
}
",
    info: Some(Taken::InPlace),
};

const RENAME: Edit = Edit {
    name: "rename",
    b_old: B_OLD,
    more: &[],
    flags: &[],
    gdb: &[],
    langs: &[false, true],
    addr: None,
    b: r"struct pt { int x, y; };
struct small_b { long s1, s2; };
static int scale = 5;
int area(struct pt *p) {
    int width_renamed_for_wp228 = p->x + scale;
    int h = p->y;
    return width_renamed_for_wp228 * h; /*B*/
}
",
    info: Some(Taken::InPlace),
};

const GROW: Edit = Edit {
    name: "grow",
    b_old: B_OLD,
    more: &[],
    flags: &[],
    gdb: &[],
    langs: &[false, true],
    addr: None,
    b: r"struct pt { int x, y; };
struct small_b { long s1, s2; };
struct grown_type_of_wp228 {
    long m01, m02, m03, m04, m05, m06, m07, m08;
};
static int scale = 5;
int area(struct pt *p) {
    int w = p->x + scale;
    int h = p->y;
    return w * h; /*B*/
}
",
    info: Some(Taken::Tail),
};

/// `c.c` of [`REUSE`]: smaller than `b`'s `.debug_info`, so that grown a
/// little it fits `b`'s old allotment.
const C_OLD: &str = r"struct small_c { long c1, c2; };
int c_value(int v) {
    int t = v + 1;
    return t; /*C*/
}
";

/// IN-50 step 2 (best fit into a dead range): `b` grows to the tail, its old
/// allotment dead (R-3); `c`, later in the update, grows past its own
/// slack and takes `b`'s old range.
const REUSE: Edit = Edit {
    name: "reuse",
    b_old: B_OLD,
    b: GROW.b,
    info: Some(Taken::Tail),
    more: &[(
        "c",
        C_OLD,
        Some(
            r"struct small_c { long c1, c2; };
struct c_grown_for_wp228_reuse { long r1, r2; };
int c_value(int v) {
    int t = v + 1;
    return t; /*C*/
}
",
        ),
        Some(Taken::DeadRange),
    )],
    flags: &[],
    gdb: &["ptype struct c_grown_for_wp228_reuse", "info scope c_value"],
    langs: &[false, true],
    addr: None,
};

/// R-5 on a reached path (B3-1): GCC `-g3` macro groups (`wm4.*`,
/// `DW_MACRO_import`); `b` changed, `a` the keeper of the shared groups,
/// `b` alone keeps the group of its own header.
const MACROS: Edit = Edit {
    name: "macros",
    b_old: B_MACROS_OLD,
    b: r"#include <limits.h>
#define B_SCALE_WP228 5
struct pt { int x, y; };
struct small_b { long s1, s2; };
static int scale = B_SCALE_WP228;
/* moved down */
int area(struct pt *p) {
    int w = p->x + scale;
    int h = p->y;
    return w * h; /*B*/
}
",
    info: None,
    more: &[],
    flags: &["-g3"],
    gdb: &[
        "info macro -at b.c:9 B_SCALE_WP228",
        "macro expand B_SCALE_WP228 + 1",
        "info macro -at b.c:9 INT_MAX",
        "info macro -at a.c:1510 EOF",
    ],
    langs: &[false],
    addr: None,
};

/// The old `b.c` of [`MACROS`].
const B_MACROS_OLD: &str = r"#include <limits.h>
#define B_SCALE_WP228 5
struct pt { int x, y; };
struct small_b { long s1, s2; };
static int scale = B_SCALE_WP228;
int area(struct pt *p) {
    int w = p->x + scale;
    int h = p->y;
    return w * h; /*B*/
}
";

/// The old `b.c` of [`SPLIT5`]: `split_extra_wp228` without debug
/// information under Clang (`nodebug`); `split_base_wp228` makes the unit
/// span several sections in both versions (`DW_AT_ranges`, so that the
/// edit adds no `.debug_rnglists` section).
const B_SPLIT_OLD: &str = r"struct pt { int x, y; };
struct small_b { long s1, s2; };
#ifdef __clang__
#define WP228_NODEBUG __attribute__((nodebug))
#else
#define WP228_NODEBUG
#endif
static int scale = 5;
int area(struct pt *p) {
    int w = p->x + scale;
    int h = p->y;
    return w * h; /*B*/
}
int split_base_wp228(int v) {
    return v - 1;
}
WP228_NODEBUG int split_extra_wp228(int v) {
    return v + 7;
}
";

/// B3-7: DWARF 5 split DWARF (`-gsplit-dwarf -gdwarf-5`: a
/// `DW_UT_skeleton` unit, `.debug_addr` with its header) through the
/// update. The edit adds `.debug_addr` entries and no allocated byte
/// (the writer's premise; a real added function moves code, which is not
/// G-1's): under Clang `split_extra_wp228` gains its debug information
/// (`nodebug` dropped; `-ffunction-sections` gives its `DW_AT_low_pc` an
/// entry of its own), under GCC (no `nodebug`) two labels in it (each a
/// `DW_TAG_label` with an address entry). `.debug_addr` goes to the tail,
/// its old range a dead DWARF 5 table; gdb and lldb read the `.dwo` files
/// through the updated skeletons.
const SPLIT5: Edit = Edit {
    name: "split5",
    b_old: B_SPLIT_OLD,
    b: r"struct pt { int x, y; };
struct small_b { long s1, s2; };
#define WP228_NODEBUG
static int scale = 5;
int area(struct pt *p) {
    int w = p->x + scale;
    int h = p->y;
    return w * h; /*B*/
}
int split_base_wp228(int v) {
    return v - 1;
}
WP228_NODEBUG int split_extra_wp228(int v) {
#ifndef __clang__
wp228_one: ;
wp228_two: ;
#endif
    return v + 7;
}
",
    info: None,
    more: &[],
    flags: &["-gsplit-dwarf", "-ffunction-sections"],
    gdb: &[
        "info scope split_extra_wp228",
        "info line split_extra_wp228",
        "info address split_extra_wp228",
    ],
    langs: &[false],
    addr: Some(Taken::Tail),
};

/// B3-7: the GNU split DWARF of DWARF 4 (`DW_AT_GNU_addr_base`, a
/// headerless `.debug_addr`): `b` changed in its lines.
const SPLIT4: Edit = Edit { name: "split4", flags: &["-gsplit-dwarf"], ..LINES };

/// B3-7: the same objects, `c` removed (IN-52).
const SPLIT4_GONE: Edit = Edit {
    name: "split4-gone",
    more: &[("c", C_OLD, None, None)],
    flags: &["-gsplit-dwarf"],
    ..LINES
};

/// The stem of [`FORTRAN`]'s Fortran file (`f.f90`, compiled by the GCC
/// driver, which runs gfortran's `f951`).
const FORTRAN_STEM: &str = "f";

/// The source extension of `stem`.
fn src_ext(cfg: Cfg, stem: &str) -> &'static str {
    if stem == FORTRAN_STEM { "f90" } else { cfg.ext() }
}

/// The old `f.f90` of [`FORTRAN`]: a C-callable function, no runtime call
/// (the C driver's link line has no `libgfortran`).
const F_OLD: &str = "integer(c_int) function f_value(v) bind(c, name='f_value')
  use iso_c_binding
  implicit none
  integer(c_int), value :: v
  integer :: t
  t = v + 1
  f_value = t
end function f_value
";

/// IN-50 and IN-56 on a gfortran unit (WP-228 B4 item 6): `f.f90`
/// changed by a line shift and a renamed local (lines in place, the new
/// name appended); `b` unchanged in its bytes.
const FORTRAN: Edit = Edit {
    name: "fortran",
    b_old: B_OLD,
    b: B_OLD,
    info: Some(Taken::InPlace),
    more: &[(
        FORTRAN_STEM,
        F_OLD,
        Some(
            "! three
! more
! lines
integer(c_int) function f_value(v) bind(c, name='f_value')
  use iso_c_binding
  implicit none
  integer(c_int), value :: v
  integer :: t_renamed_for_wp228
  t_renamed_for_wp228 = v + 1
  f_value = t_renamed_for_wp228
end function f_value
",
        ),
        None,
    )],
    flags: &[],
    gdb: &["info scope f_value", "info line f_value", "info address f_value", "list f_value"],
    langs: &[false],
    addr: None,
};

/// What C++ adds to every source: COMDAT code shared by `a` and `b`
/// (an inline function, a template instance), so that `b`'s units name
/// dropped copies.
fn cxx_tail(stem: &str) -> String {
    format!(
        "namespace wp228 {{ template <class T> struct Box {{ T v; T get() const {{ return v; }} }};\n\
         inline int twice(int v) {{ return v * 2; }} }}\n\
         int use_box_{stem}(long v) {{ wp228::Box<long> b{{v}}; return int(b.get()) + wp228::twice(1); }}\n"
    )
}

/// The compiler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cc {
    Gcc,
    Clang,
    /// kiln (front end) and hod (code generator), C++ only.
    Kiln,
}

/// A compiler configuration.
#[derive(Debug, Clone, Copy)]
struct Cfg {
    arch: Arch,
    cc: Cc,
    cxx: bool,
    dwarf: u8,
}

/// The workspace's kiln, as the gate names it.
fn kiln() -> Option<PathBuf> {
    ["MASON_KILN", "MORTAR_KILN"]
        .iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .find(|p| p.is_file())
}

impl Cfg {
    fn label(self) -> String {
        let cc = match self.cc {
            Cc::Gcc => "gcc",
            Cc::Clang => "clang",
            Cc::Kiln => "kiln",
        };
        let pp = if self.cxx && self.cc != Cc::Kiln { "++" } else { "" };
        format!("{}-{cc}{pp}-dwarf{}", self.arch.name(), self.dwarf)
    }

    fn ext(self) -> &'static str {
        if self.cxx { "cc" } else { "c" }
    }

    /// The GNU driver whose link line the test takes, and its flags.
    fn driver(self, extra: &[&str]) -> (String, Vec<String>) {
        let mut f = Vec::new();
        let cc = match (self.cc, self.cxx) {
            (Cc::Clang, cxx) => {
                f.extend(self.arch.clang_args());
                if cxx { "clang++" } else { "clang" }.to_owned()
            }
            (_, true) => self.arch.gnu("g++"),
            (_, false) => self.arch.gnu("gcc"),
        };
        f.extend(
            ["-g", &format!("-gdwarf-{}", self.dwarf), "-O0", "-fno-eliminate-unused-debug-types"]
                .map(String::from),
        );
        f.extend(extra.iter().map(|s| (*s).to_owned()));
        (cc, f)
    }

    /// The compiler of the test's objects and its flags.
    fn compiler(self, extra: &[&str]) -> Option<(PathBuf, Vec<String>)> {
        if self.cc == Cc::Kiln {
            let mut f = vec![format!("--target={}", self.arch.triple())];
            f.extend(["-g", &format!("-gdwarf-{}", self.dwarf), "-O0"].map(String::from));
            f.extend(extra.iter().map(|s| (*s).to_owned()));
            return kiln().map(|k| (k, f));
        }
        let (cc, f) = self.driver(extra);
        tool(&cc).map(|t| (t, f))
    }
}

fn configs(langs: &[bool]) -> Vec<Cfg> {
    let mut v = Vec::new();
    for arch in [Arch::Aarch64, Arch::X86_64] {
        for &cxx in langs {
            for cc in [Cc::Gcc, Cc::Clang] {
                for dwarf in [4, 5] {
                    v.push(Cfg { arch, cc, cxx, dwarf });
                }
            }
        }
    }
    v
}

// ---- ELF ------------------------------------------------------------------------

/// A section header of a 64-bit little-endian ELF file.
#[derive(Debug, Clone)]
struct Shdr {
    name: String,
    sh_type: u32,
    flags: u64,
    offset: u64,
    size: u64,
    /// Where its `sh_size` is in the file.
    at: usize,
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(b[at..at + 2].try_into().unwrap())
}
fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

fn shdrs(b: &[u8]) -> Vec<Shdr> {
    let (off, n, strndx) = (u64_at(b, 0x28) as usize, u16_at(b, 0x3c), u16_at(b, 0x3e));
    let raw = |i: usize| off + i * 64;
    let stroff = u64_at(b, raw(strndx as usize) + 24) as usize;
    (0..n as usize)
        .map(|i| {
            let h = raw(i);
            let n0 = stroff + u32_at(b, h) as usize;
            let len = b[n0..].iter().position(|&c| c == 0).unwrap();
            Shdr {
                name: String::from_utf8_lossy(&b[n0..n0 + len]).into_owned(),
                sh_type: u32_at(b, h + 4),
                flags: u64_at(b, h + 8),
                offset: u64_at(b, h + 24),
                size: u64_at(b, h + 32),
                at: h + 32,
            }
        })
        .collect()
}

fn section<'b>(b: &'b [u8], name: &str) -> Option<&'b [u8]> {
    let s = shdrs(b).into_iter().find(|s| s.name == name)?;
    Some(&b[s.offset as usize..(s.offset + s.size) as usize])
}

// ---- the links ----------------------------------------------------------------

struct Fixture {
    cfg: Cfg,
    edit: Edit,
    scratch: Scratch,
    /// The driver's link line (`-o out`), objects `a.c.o`, `b.c.o`, ...
    args: Vec<String>,
}

fn compile(cfg: Cfg, edit: Edit, dir: &Path, src: &str, obj: &str) {
    let (cc, flags) = cfg.compiler(edit.flags).expect("the compiler");
    let mut c = Command::new(cc);
    c.current_dir(dir).args(&flags).args(["-c", src, "-o", obj]);
    let o = run_with(&mut c, b"", Duration::from_mins(2));
    assert!(o.success(), "{}: {}", cfg.label(), o.stderr_text());
}

impl Fixture {
    /// The stems of the files the edit changes or removes, in link order.
    fn stems(edit: Edit) -> Vec<&'static str> {
        std::iter::once("b").chain(edit.more.iter().map(|m| m.0)).collect()
    }

    /// The text of `stem` in the language of `cfg`.
    fn text(cfg: Cfg, stem: &str, body: &str) -> String {
        if cfg.cxx && stem != FORTRAN_STEM {
            format!("{body}{}", cxx_tail(stem))
        } else {
            body.to_owned()
        }
    }

    fn new(cfg: Cfg, edit: Edit) -> Option<Self> {
        cfg.compiler(edit.flags)?;
        let (cc, flags) = cfg.driver(&[]);
        let scratch = Scratch::new(&format!("wp228-b3-{}-{}", cfg.label(), edit.name));
        let flags: Vec<&str> = flags.iter().map(String::as_str).collect();
        let ext = cfg.ext();
        let mut sources =
            vec![Source { name: Box::leak(format!("a.{ext}").into_boxed_str()), text: "" }];
        for stem in Self::stems(edit) {
            sources.push(Source {
                name: Box::leak(format!("{stem}.{}", src_ext(cfg, stem)).into_boxed_str()),
                text: "",
            });
        }
        let built = build_by(&scratch, &cc, &flags, &sources)?;
        // Every object compiled alike (relative names, the scratch as the
        // directory): the link line's objects are replaced.
        let a = format!("a.{ext}");
        let split = edit.flags.contains(&"-gsplit-dwarf");
        let mut a_text = a_c();
        if split {
            // `.debug_addr`'s tail reserve (`size/32`, IN-33) must hold
            // `b`'s grown table: 320 functions, an address entry each.
            for k in 0..320 {
                use std::fmt::Write as _;
                let _ = writeln!(a_text, "int wp228_fill_{k}(int v) {{ return v + {k}; }}");
            }
        }
        scratch.write(&a, Self::text(cfg, "a", &a_text));
        compile(cfg, edit, scratch.path(), &a, &format!("{a}.o"));
        let mut versions = vec![("b", edit.b_old, Some(edit.b))];
        versions.extend(edit.more.iter().map(|m| (m.0, m.1, m.2)));
        // Split DWARF: both versions compiled as `<stem>.o`, so that the
        // skeleton names the same `<stem>.dwo` (as a rebuild of one object
        // does); `<stem>.dwo` is the new version's, the one the debuggers
        // read through `F` and the update.
        let version = |src: &str, stem: &str, v: u8| {
            if split {
                compile(cfg, edit, scratch.path(), src, &format!("{stem}.o"));
                let to = scratch.join(format!("{stem}{v}.o"));
                std::fs::rename(scratch.join(format!("{stem}.o")), to).unwrap();
            } else {
                compile(cfg, edit, scratch.path(), src, &format!("{stem}{v}.o"));
            }
        };
        for (stem, old, new) in versions {
            let src = format!("{stem}.{}", src_ext(cfg, stem));
            scratch.write(&src, Self::text(cfg, stem, old));
            version(&src, stem, 1);
            std::fs::remove_file(scratch.join(format!("{src}.o"))).unwrap();
            if let Some(new) = new {
                scratch.write(&src, Self::text(cfg, stem, new));
                version(&src, stem, 2);
            }
        }
        Some(Self { cfg, edit, scratch, args: built.args })
    }

    /// Whether `stem` is in the link of version `v`.
    fn present(&self, stem: &str, v: u8) -> bool {
        v == 1 || stem == "b" || self.edit.more.iter().any(|m| m.0 == stem && m.2.is_some())
    }

    fn label(&self) -> String {
        format!("{} {}", self.cfg.label(), self.edit.name)
    }

    /// The link line of version `v` (1 old, 2 new) to `out`, incremental mode.
    fn line(&self, v: u8, out: &str) -> Vec<OsString> {
        let args: Vec<String> = self
            .args
            .iter()
            .filter_map(|a| {
                for stem in Self::stems(self.edit) {
                    if a.ends_with(&format!("/{stem}.{}.o", src_ext(self.cfg, stem))) {
                        let o = self.scratch.join(format!("{stem}{v}.o"));
                        return self.present(stem, v).then(|| o.display().to_string());
                    }
                }
                // No build id: an update re-seals it over the whole output
                // (not G-1's; the writer copies `P`'s, which the verifier's
                // B1 would report on every output).
                (!a.starts_with("--build-id")).then(|| a.clone())
            })
            .collect();
        // A file the new link no longer has is the LAST input of both
        // links, so that no other file's ids move (the writer keeps the
        // old layout's per-file and per-symbol data).
        let (mut args, last): (Vec<String>, Vec<String>) = args.into_iter().partition(|a| {
            !self.edit.more.iter().any(|m| m.2.is_none() && a.ends_with(&format!("/{}1.o", m.0)))
        });
        args.extend(last);
        let mut a = with_output(&args, &self.scratch.join(out));
        a.extend(["--incremental=full", "--incremental-state"].iter().map(OsString::from));
        a.push(self.scratch.join(format!("{out}.mortar-state")).into_os_string());
        a.push("--mortar-map".into());
        a.push(self.scratch.join(format!("{out}.map")).into_os_string());
        a
    }

    /// A full link of version `v` to `out` through the command line: its
    /// capture's rows.
    fn full(&self, v: u8, out: &str) -> MemoryState {
        let sink: Rows5Sink = std::sync::Arc::default();
        let inv = mortar::invoke_state5(
            "mortar",
            &self.line(v, out),
            self.scratch.path(),
            State5 {
                path: self.scratch.join(format!("{out}.state5")),
                collide: false,
                rows: Some(sink.clone()),
            },
        );
        assert!(
            matches!(inv.outcome, mortar::Outcome::Linked { .. }),
            "{}: {:?}",
            self.label(),
            inv.outcome
        );
        sink.lock().unwrap().take().expect("rows")
    }
}

// ---- the update's debug steps ---------------------------------------------------

/// A section's name and bytes in a link's inputs.
fn sec<'s>(
    inputs: &mortar_core::inputs::Inputs<'s>,
    res: &mortar_core::resolution::Resolution,
    id: SectionId,
) -> (Vec<u8>, &'s [u8]) {
    let (file, idx) = res.sh_index(id).unwrap();
    let Reader::Object(obj) = &inputs.files[file].reader else { panic!("not an object") };
    let s = obj.section(idx).unwrap();
    (s.name().unwrap().to_vec(), s.data().unwrap_or(&[]))
}

/// What the old link gives the update: its layout, and per (input section
/// name, fragment bytes) the fragment's offset in its merged chunk.
struct Old {
    layout: Layout,
    frags: HashMap<(Vec<u8>, Vec<u8>), u64>,
    base: Column<FileId, mortar_core::id::IdRange<SectionId>>,
    /// Each file's path, for an object named on the link line.
    paths: Vec<Option<Vec<u8>>>,
}

/// Each file's path, for an object named on the link line.
fn paths_of(inputs: &mortar_core::inputs::Inputs<'_>) -> Vec<Option<Vec<u8>>> {
    let mut v = vec![None; inputs.files.len() as usize];
    for o in &inputs.link.opened {
        if let mortar_core::inputs::OpenedKind::Object(id) = o.kind {
            v[id.0 as usize] = Some(o.path.to_vec());
        }
    }
    v
}

fn ends(p: Option<&Vec<u8>>, s: &str) -> bool {
    p.is_some_and(|p| p.ends_with(s.as_bytes()))
}

fn old_of(fx: &Fixture) -> Old {
    let store = support::store();
    let parsed = mortar_cli::parse(&fx.line(1, "prev-front"));
    assert!(!parsed.failed());
    let diag = Collector::new(Policy::default());
    let env =
        Env { cwd: fx.scratch.path(), pool: Pool::new(2), replay: None, slack: true, state5: true };
    let f = front(parsed.plan, env, &store, &diag, &mut Timer::new(false)).expect("front");
    let mut frags = HashMap::new();
    for (id, fr) in f.merge.split.frags.iter() {
        let Some(off) = f.layout.merge.offset(id) else { continue };
        let (name, data) = sec(&f.inputs, &f.resolution, fr.section);
        let bytes = data[fr.offset as usize..(fr.offset + fr.size) as usize].to_vec();
        frags.entry((name, bytes)).or_insert(off);
    }
    Old {
        layout: f.layout.clone(),
        frags,
        base: f.resolution.section_base.clone(),
        paths: paths_of(&f.inputs),
    }
}

/// The capture's `DEBUG_STRS` rows (WP-247) as a hashed lookup, the
/// lowest offset per key as [`MemoryState`]'s own `debug_str` (which scans
/// every row: too slow for the measurement's timings).
fn debug_strs_of(rows: &MemoryState) -> HashMap<(u8, u64), u32> {
    let mut m: HashMap<(u8, u64), u32> = HashMap::new();
    for r in &rows.debug_strs {
        let e = m.entry((r.out, r.hash)).or_insert(r.offset);
        *e = (*e).min(r.offset);
    }
    m
}

/// The changed file `ob` (`nb` in the new link) from the new capture's
/// rows, into `cf`.
fn changed_of(
    old: &MemoryState,
    new: &MemoryState,
    ob: FileId,
    nb: FileId,
    first: SectionId,
    cf: &mut ChangedFiles,
) {
    let base = old.strings.len() as u32;
    let intern = |s: Str, cf: &mut ChangedFiles| -> Str {
        if s.len == 0 {
            return Str::default();
        }
        let bytes = &new.strings[s.off as usize..(s.off + s.len) as usize];
        let off = base + cf.strings.len() as u32;
        cf.strings.extend_from_slice(bytes);
        Str { off, len: s.len }
    };
    let mut sections = Vec::new();
    for r in new.sections.iter().filter(|r| r.file == nb.0) {
        let name = intern(r.name, cf);
        let anchor = if r.anchor_kind == 1 { intern(r.anchor, cf) } else { r.anchor };
        sections.push(SectionRow {
            file: ob.0,
            name,
            anchor,
            place_chunk: mortar_incr::format::NONE,
            place_offset: 0,
            ..*r
        });
    }
    cf.files.push(file_row(old, ob, Change::Changed, first, sections));
}

fn file_row(
    old: &MemoryState,
    file: FileId,
    change: Change,
    first_section: SectionId,
    sections: Vec<SectionRow>,
) -> ChangedFile {
    ChangedFile {
        file,
        change,
        row: old.files[file.0 as usize].clone(),
        first_section,
        sections,
        keys: Vec::new(),
        first_global: 0,
        mentions: Vec::new(),
        values: Vec::new(),
        index: Vec::new(),
        edges: Vec::new(),
        meta_units: 0,
        debug: true,
        versioned: 0,
        odr: OdrRows::default(),
    }
}

/// What the update decided, and the image of the new inputs with its
/// placement.
struct Update {
    plan: DebugPlan,
    /// The image of the new inputs with the update's placement.
    image: Vec<u8>,
    /// The machine map of that image (mortar's format, `spec/plumb.md`
    /// §3.2): the update's placement, its `dead` and `hole` records; the
    /// map `cargo xtask plumb` judges the update's output by.
    map: Vec<u8>,
}

/// The update of `fx` (old link `old`, its rows `rows`, previous output
/// `prev`; new rows `new`): `debug::strings` over the capture's
/// `DEBUG_STRS` rows (or, `with_rows` false, a state without the table),
/// `place`, and the image with that placement.
#[allow(
    clippy::too_many_lines,
    clippy::many_single_char_names,
    reason = "the update's steps in order"
)]
fn update(
    fx: &Fixture,
    old: &Old,
    rows: &MemoryState,
    new: &MemoryState,
    prev: &[u8],
    with_rows: bool,
) -> Result<Update, String> {
    let store = support::store();
    let parsed = mortar_cli::parse(&fx.line(2, "upd-image"));
    assert!(!parsed.failed());
    let args: Vec<Vec<u8>> = parsed.args.iter().map(|a| a.as_encoded_bytes().to_vec()).collect();
    let diag = Collector::new(Policy::default());
    let out: RefCell<Option<Result<DebugPlan, String>>> = RefCell::new(None);
    let hook = |cx: &mortar::front::Replay<'_, '_>| {
        let link = &cx.link;
        let r = (|| -> Result<(Layout, DebugPlan), String> {
            // The files: the new link's are the old one's without the
            // removed ones, in order; the changed ones are `<stem>2.o`.
            let paths = paths_of(link.inputs);
            let stems = Fixture::stems(fx.edit);
            let find = |ps: &[Option<Vec<u8>>], tail: &str| {
                (0..ps.len() as u32).map(FileId).find(|f| ends(ps[f.0 as usize].as_ref(), tail))
            };
            let mut removed = Vec::new();
            for s in stems.iter().filter(|s| !fx.present(s, 2)) {
                removed.push(find(&old.paths, &format!("/{s}1.o")).ok_or("no removed file")?);
            }
            let map: Vec<FileId> =
                (0..old.paths.len() as u32).map(FileId).filter(|f| !removed.contains(f)).collect();
            if map.len() != paths.len() {
                return Err("the new link's files are not the old ones less the removed".into());
            }
            let mut ch: Vec<(FileId, FileId)> = Vec::new();
            for s in stems.iter().filter(|s| fx.present(s, 2)) {
                let nb = find(&paths, &format!("/{s}2.o")).ok_or("no changed file")?;
                let ob = map[nb.0 as usize];
                if !ends(old.paths[ob.0 as usize].as_ref(), &format!("/{s}1.o")) {
                    return Err(format!("{s}: the file map"));
                }
                if link.resolution.section_base[nb].len() != old.base[ob].len() {
                    return Err(format!("the edit changed {s}'s section count"));
                }
                ch.push((nb, ob));
            }
            let view = rows.view();
            let lo = LayoutOf { view: &view };
            let stored = StoredLayout::new(&lo);
            let mut changed = ChangedFiles::default();
            let mut firsts: HashMap<FileId, SectionId> = HashMap::new();
            let mut first = SectionId(rows.sections.len() as u32);
            for &(nb, ob) in &ch {
                changed_of(rows, new, ob, nb, first, &mut changed);
                firsts.insert(nb, first);
                first = SectionId(first.0 + old.base[ob].len());
            }
            for &ob in &removed {
                changed.files.push(file_row(rows, ob, Change::Gone, first, Vec::new()));
            }
            // (1) strings.
            let mut inputs = Vec::new();
            for &(nb, ob) in &ch {
                let r = link.resolution.section_base[nb];
                for k in 0..r.len() {
                    let (name, data) = sec(link.inputs, link.resolution, r.nth(k).unwrap());
                    if let Some(class) = debug::str_class(&name) {
                        inputs.push((ob, class, data));
                    }
                }
            }
            // The capture's real `DEBUG_STRS` rows (WP-247) through the
            // update's own lookup, or a state without the table.
            let none = |_: u8, _: u64| Ok(None);
            let strings = if with_rows {
                debug::strings(&view, &stored, &inputs, prev)
            } else {
                debug::strings_with(&none, &stored, &inputs, prev)
            }
            .map_err(|e| {
                let keys: std::collections::HashSet<(u8, u64)> =
                    rows.debug_strs.iter().map(|r| (r.out, r.hash)).collect();
                let miss: Vec<String> = inputs
                    .iter()
                    .flat_map(|(_, c, d)| d.split(|&x| x == 0).map(move |w| (*c, w)))
                    .filter(|(c, w)| !w.is_empty() && !keys.contains(&(*c, name_hash(w, b""))))
                    .map(|(c, w)| format!("{c}:{}", String::from_utf8_lossy(w)))
                    .collect();
                format!("strings: {e:?}; not in the rows: {miss:?}; rows {}", keys.len())
            })?;
            // (2) what layout(debug: true) gives a debug-only edit.
            let olds: Vec<FileId> = ch.iter().map(|c| c.1).chain(removed.iter().copied()).collect();
            let mut d = LayoutDelta {
                placement: old.layout.placement.clone(),
                files: olds.clone(),
                ..LayoutDelta::default()
            };
            for &ob in &olds {
                for k in 0..old.base[ob].len() {
                    let id = old.base[ob].nth(k).unwrap();
                    let r = &rows.sections[id.0 as usize];
                    let name = view.string(r.name).map_err(|e| e.name().to_owned())?;
                    if r.sh_flags & 2 == 0
                        && name.starts_with(b".debug_")
                        && d.placement.place[id].get().is_some()
                    {
                        d.killed.push(id);
                    }
                }
            }
            // (3) place.
            let plan = debug::place(&view, &stored, &changed, strings, &mut d)
                .map_err(|e| format!("place: {} ({e:?})", e.name()))?;
            // (4) the layout of the new link with the update's placement.
            let mut l = old.layout.clone();
            if !removed.is_empty() {
                // The removed files' locals leave `.symtab` (the update's
                // symbol table, not G-1's: the writer keeps `P`'s).
                let locals = &old.layout.slots.symtab_locals;
                let mut at = locals.iter().next().map_or(0, |(_, s)| s.start);
                let mut spans = Vec::new();
                for &o in &map {
                    let len = locals[o].len;
                    spans.push(mortar_core::id::Span { start: at, len });
                    at += len;
                }
                l.slots.symtab_locals = Column::from_vec(spans).map_err(|_| "symtab_locals")?;
            }
            l.placement.chunks.clone_from(&d.placement.chunks);
            l.placement.holes.clone_from(&d.placement.holes);
            for (chunk, at, bytes) in plan.strings.appended.values() {
                l.placement.chunks[*chunk].size = at + bytes.len() as u64;
            }
            let n = link.resolution.section_base.iter().map(|(_, r)| r.end.0).max().unwrap_or(0);
            let mut place = Column::filled(n, Place::NONE);
            for (f, r) in link.resolution.section_base.iter() {
                let o = map[f.0 as usize];
                for k in 0..r.len() {
                    let id = r.nth(k).unwrap();
                    let oid = old.base[o].nth(k).unwrap();
                    place[id] = if let Some(first) = firsts.get(&f) {
                        let (name, _) = sec(link.inputs, link.resolution, id);
                        let flags = rows.sections[oid.0 as usize].sh_flags;
                        let placed_by_place = !matches!(
                            debug::form(&name).map(|f| f.policy),
                            Some(debug::Policy::Strings | debug::Policy::Whole)
                        );
                        if flags & 2 == 0 && name.starts_with(b".debug_") && placed_by_place {
                            d.placement
                                .place
                                .get(SectionId(first.0 + k))
                                .copied()
                                .unwrap_or(Place::NONE)
                        } else {
                            old.layout.placement.place[oid]
                        }
                    } else {
                        old.layout.placement.place[oid]
                    };
                }
            }
            // `.debug_aranges` (IN-54): the input sets in the old order,
            // laid end to end with their new sizes (the full link's order).
            let mut ar: Vec<(u64, SectionId, u64)> = Vec::new();
            let mut ar_chunk = None;
            for (_, r) in link.resolution.section_base.iter() {
                for k in 0..r.len() {
                    let id = r.nth(k).unwrap();
                    let (name, data) = sec(link.inputs, link.resolution, id);
                    if name == b".debug_aranges"
                        && let Some((c, o)) = place[id].get()
                    {
                        ar_chunk = Some(c);
                        ar.push((o, id, data.len() as u64));
                    }
                }
            }
            ar.sort_unstable();
            if let Some(c) = ar_chunk {
                let mut at = ar.first().map_or(0, |a| a.0);
                for &(_, id, size) in &ar {
                    place[id] = Place::at(c, at);
                    at += size;
                }
                l.placement.chunks[c].size = at;
            }
            l.placement.place = place;
            let mut frag_off = Vec::new();
            for (_, fr) in link.split.frags.iter() {
                let (name, data) = sec(link.inputs, link.resolution, fr.section);
                let bytes = data[fr.offset as usize..(fr.offset + fr.size) as usize].to_vec();
                let file = link.resolution.file_of(fr.section);
                let in_b = file.is_some_and(|f| firsts.contains_key(&f));
                let off = match debug::str_class(&name) {
                    Some(class) if in_b => plan
                        .strings
                        .output_offset(map[file.unwrap().0 as usize], class, u64::from(fr.offset))
                        .ok_or("a string of a changed file without an output offset")?,
                    _ => match old.frags.get(&(name.clone(), bytes)) {
                        Some(&o) => o,
                        None if in_b => {
                            return Err(format!(
                                "a new fragment of a changed file in {}",
                                String::from_utf8_lossy(&name)
                            ));
                        }
                        None => MergeMap::NONE,
                    },
                };
                frag_off.push(off);
            }
            l.merge = MergeMap { frag_off: Column::from_vec(frag_off).map_err(|_| "frags")? };
            Ok((l, plan))
        })();
        match r {
            Ok((l, plan)) => {
                *out.borrow_mut() = Some(Ok(plan));
                Ok(l)
            }
            Err(e) => {
                *out.borrow_mut() = Some(Err(e.clone()));
                Err(e)
            }
        }
    };
    let env = Env {
        cwd: fx.scratch.path(),
        pool: Pool::new(2),
        replay: Some(&hook as &mortar::front::ReplayHook<'_>),
        slack: true,
        state5: false,
    };
    let f = front(parsed.plan, env, &store, &diag, &mut Timer::new(false));
    let plan = out.borrow_mut().take().ok_or("the replay did not run")??;
    let mut f = f.ok_or_else(|| format!("front: {:?}", diag.finish()))?;
    // The line records of the map (`spec/plumb.md` §3.2a), as a link sets them.
    f.args = args;
    let path = fx.scratch.join("upd-image");
    mortar::image::write(&f, &store, &path, &|_| {}, &mut |_, _| {})
        .map_err(|e| format!("image: {:?}", e.diagnostics))?;
    let image = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    let map = mortar::image::map_of(&f, &store).map_err(|e| format!("map: {e:?}"))?;
    let map = mortar_image::map::render(&map, &f.pool);
    Ok(Update { plan, image, map })
}

/// The update's output: a copy of `prev` with what an update writes
/// (module doc). Checks the image agrees with `prev` where the update
/// writes nothing.
#[allow(clippy::too_many_lines, reason = "what an update writes, in order")]
fn write_update(label: &str, prev: &[u8], u: &Update, rows: &MemoryState) -> Vec<u8> {
    let img = &u.image;
    // The symbol tables and what follows them may differ (a removed
    // file's `STT_FILE` symbol: the update's symbol table, not G-1's);
    // every other section is where `P` has it.
    let tables = |n: &str| matches!(n, ".symtab" | ".strtab" | ".shstrtab");
    let mut out = prev.to_vec();
    let view = rows.view();
    let lo = LayoutOf { view: &view };
    let stored = StoredLayout::new(&lo);
    let chunks: BTreeMap<ChunkId, mortar_core::layout::Chunk> =
        stored.chunks().unwrap().into_iter().collect();
    let copy = |at: u64, n: u64, out: &mut Vec<u8>| {
        let (a, e) = (at as usize, (at + n) as usize);
        out[a..e].copy_from_slice(&img[a..e]);
    };
    for c in &u.plan.contributions {
        copy(chunks[&c.chunk].offset + c.allot.offset, c.size, &mut out);
    }
    for (chunk, holes) in &u.plan.chunks {
        for h in holes.iter().filter(|h| matches!(h.kind, HoleKind::Debug | HoleKind::DeadDebug)) {
            copy(chunks[chunk].offset + h.offset, h.size, &mut out);
        }
    }
    for (chunk, at, bytes) in u.plan.strings.appended.values() {
        let s = (chunks[chunk].offset + at) as usize;
        out[s..s + bytes.len()].copy_from_slice(bytes);
        // With rows, every appended string is one no unchanged file has:
        // its fragment is the changed file's, and the image wrote it at the
        // same place. (Without rows a string shared with an unchanged file
        // is appended too, while the image's one fragment per content
        // names the old copy: both copies are the same bytes.)
        if u.plan.strings.hits > 0 {
            assert_eq!(&img[s..s + bytes.len()], &bytes[..], "{label}: appended strings");
        }
    }
    // `.debug_aranges` (IN-54): `aranges` over P's section; the changed
    // file's new sets are the image's sets naming its new `.debug_info`.
    if let (Some(pa), Some(ia)) = (section(prev, ".debug_aranges"), section(img, ".debug_aranges"))
    {
        let mut replaced = Vec::new();
        for &(file, os, oe) in &u.plan.aranges {
            // The file's new sets: the image's sets naming its new
            // `.debug_info` (none for a removed file).
            let info = u
                .plan
                .contributions
                .iter()
                .find(|c| c.file == file && chunk_name(rows, c.chunk) == b".debug_info");
            let mut sets = Vec::new();
            if let Some(c) = info {
                let (s, e) = (c.allot.offset, c.allot.offset + c.size);
                let mut at = 0;
                while at + 10 <= ia.len() {
                    let len = u32_at(ia, at) as usize;
                    if len == 0 {
                        break;
                    }
                    let off = u64::from(u32_at(ia, at + 6));
                    if (s..e).contains(&off) {
                        sets.extend_from_slice(&ia[at..at + 4 + len]);
                    }
                    at += 4 + len;
                }
            }
            replaced.push((os, oe, sets));
        }
        replaced.sort_by_key(|r| r.0);
        let new = debug::aranges(pa, &replaced).unwrap();
        assert_eq!(new, ia, "{label}: IN-54's rewrite equals the full link's order");
        let sh = shdrs(prev).into_iter().find(|s| s.name == ".debug_aranges").unwrap();
        out[sh.offset as usize..sh.offset as usize + new.len()].copy_from_slice(&new);
    }
    // The section header table: sizes of the grown debug sections.
    let (ps, is) = (shdrs(prev), shdrs(img));
    assert_eq!(ps.len(), is.len(), "{label}: section count");
    for (p, i) in ps.iter().zip(&is) {
        assert_eq!(p.name, i.name, "{label}: section headers");
        if tables(&p.name) {
            continue;
        }
        assert_eq!(p.offset, i.offset, "{label}: {}'s offset", p.name);
        if p.size != i.size {
            assert!(i.name.starts_with(".debug_"), "{label}: {} changed size", i.name);
            out[p.at..p.at + 8].copy_from_slice(&i.size.to_le_bytes());
        }
    }
    // The symbol table (U-5's, emulated): with no file removed it has the
    // image's size and place, and the image's is the update's (the changed
    // file's locals in its debug sections, Clang's `$d` on AArch64, at
    // their new places: WP-228 record §7.2).
    for name in [".symtab", ".strtab"] {
        let (p, i) = (ps.iter().find(|s| s.name == name), is.iter().find(|s| s.name == name));
        if let (Some(p), Some(i)) = (p, i)
            && (p.offset, p.size) == (i.offset, i.size)
        {
            copy(p.offset, p.size, &mut out);
        }
    }
    // Where the update writes nothing, the image of the update's
    // placement holds P's bytes: every allocated byte (a debug-only edit;
    // the build id excepted) and every debug byte outside the update's
    // ranges but the merged strings' (P's prefix stays).
    for s in &is {
        let (a, e) = (s.offset as usize, (s.offset + s.size) as usize);
        if s.size == 0
            || s.sh_type == 8
            || e > img.len()
            || tables(&s.name)
            || s.name == ".debug_str"
            || s.name == ".debug_line_str"
            || s.name.starts_with(".note")
            || s.name == ".symtab"
            || s.name == ".strtab"
            || s.name == ".shstrtab"
            || s.flags & 2 == 0 && !s.name.starts_with(".debug_")
        {
            continue;
        }
        if let Some(i) = (a..e).find(|&i| out[i] != img[i]) {
            panic!("{label}: {} differs from the image at file offset {i:#x}", s.name);
        }
    }
    out
}

/// The output section name of a stored chunk.
fn chunk_name(rows: &MemoryState, chunk: ChunkId) -> Vec<u8> {
    let view = rows.view();
    let lo = LayoutOf { view: &view };
    let stored = StoredLayout::new(&lo);
    let names: BTreeMap<_, _> = stored.outsecs().unwrap().into_iter().collect();
    let c = stored.chunks().unwrap().into_iter().find(|(id, _)| *id == chunk).unwrap().1;
    c.kind.out().and_then(|o| names.get(&o).cloned()).unwrap_or_default()
}

// ---- the judges -------------------------------------------------------------------

fn tool_text(exe: &Path, args: &[String], dir: &Path) -> String {
    let mut c = Command::new(exe);
    c.current_dir(dir).args(args);
    let o = run_with(&mut c, b"", Duration::from_mins(3));
    assert!(!o.timed_out(), "{} timed out", exe.display());
    format!("{}{}", o.stdout_text(), o.stderr_text())
}

fn line_of(src: &str, mark: &str) -> usize {
    src.lines().position(|l| l.contains(mark)).unwrap() + 1
}

/// gdb's transcript of the questions on `prog`.
fn gdb(cfg: Cfg, edit: Edit, prog: &Path, dir: &Path) -> String {
    let (bl, al) = (line_of(edit.b, "/*B*/"), line_of(&a_c(), "/*A*/"));
    let x = cfg.ext();
    let mut ex: Vec<String> =
        ["set pagination off", "set width 0", "set confirm off"].map(String::from).to_vec();
    ex.push(format!("break b.{x}:{bl}"));
    ex.push(format!("break a.{x}:{al}"));
    ex.push(format!("info line b.{x}:{bl}"));
    ex.extend(edit.gdb.iter().map(|q| q.replace(".c:", &format!(".{x}:"))));
    ex.push("info line helper".into());
    ex.push("ptype struct pt".into());
    ex.push("ptype struct small_b".into());
    ex.push("info scope area".into());
    ex.push("info address g_count".into());
    ex.push("list area".into());
    if cfg.arch.is_host() {
        ex.extend(
            [
                "run > /dev/null 2>&1",
                "bt",
                "info locals",
                "print *p",
                "print scale",
                "print g_count",
                "next",
                "continue",
                "bt",
                "print local_a",
                "continue",
            ]
            .map(String::from),
        );
    }
    let mut args: Vec<String> =
        ["-batch", "-nx", "-iex", "set debuginfod enabled off"].map(String::from).to_vec();
    for e in ex {
        args.push("-ex".into());
        args.push(e);
    }
    args.push(prog.display().to_string());
    let exe = tool(cfg.arch.gdb()).unwrap();
    tool_text(&exe, &args, dir)
}

fn lldb(cfg: Cfg, edit: Edit, prog: &Path, dir: &Path) -> String {
    let (bl, al) = (line_of(edit.b, "/*B*/"), line_of(&a_c(), "/*A*/"));
    let x = cfg.ext();
    let mut o: Vec<String> = vec!["settings set target.disable-aslr false".into()];
    o.push(format!("breakpoint set -f b.{x} -l {bl}"));
    o.push(format!("breakpoint set -f a.{x} -l {al}"));
    o.push("image lookup -n area".into());
    o.push("image lookup -t pt".into());
    o.push("image lookup -t small_b".into());
    if cfg.arch.is_host() {
        o.extend(
            [
                "process launch -o /dev/null -e /dev/null",
                "bt",
                "frame variable",
                "target variable g_count",
                "continue",
                "bt",
                "frame variable local_a",
                "continue",
            ]
            .map(String::from),
        );
    }
    let mut args: Vec<String> = vec!["-b".into()];
    for x in o {
        args.push("-o".into());
        args.push(x);
    }
    args.push(prog.display().to_string());
    let exe = tool("lldb").unwrap();
    tool_text(&exe, &args, dir)
}

/// A transcript with the program's name, process ids and stack
/// addresses masked (the container refuses to disable ASLR), and lldb's
/// DIE offsets.
fn norm(t: &str, prog: &Path) -> String {
    let name = prog.file_name().unwrap().to_string_lossy().into_owned();
    let full = prog.display().to_string();
    let t = t.replace(&full, "PROG").replace(&name, "PROG");
    let mut out = String::new();
    for l in t.lines() {
        let pid = l.contains("rocess ") || l.contains("[Inferior");
        let mut rest = l;
        while !rest.is_empty() {
            if let Some(h) = rest.strip_prefix("0x") {
                let n = h.chars().take_while(char::is_ascii_hexdigit).count();
                let w = &h[..n];
                let die = out.ends_with("id = {");
                // A stack address: `0xffff...` (AArch64) or `0x7ff...`
                // (x86-64), which ASLR moves on a native run (the hosted
                // x86-64 lane; gdb cannot disable it in a container).
                let top = w.trim_start_matches('0');
                let stack = (top.starts_with("ff") || top.starts_with("7ff")) && n >= 10;
                // A DIE's offset (lldb's `id`) is a position, not a meaning:
                // an update moves units (IN-50).
                out.push_str(if stack {
                    "0xSTACK"
                } else if die {
                    "0xDIE"
                } else {
                    &rest[..2 + n]
                });
                rest = &h[n..];
            } else if pid && rest.starts_with(|c: char| c.is_ascii_digit()) {
                let n = rest.chars().take_while(char::is_ascii_digit).count();
                out.push('N');
                rest = &rest[n..];
            } else {
                let c = rest.chars().next().unwrap();
                out.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
        out.push('\n');
    }
    out
}

/// readelf's warnings on `prog`, offsets masked, one each.
fn readelf_warnings(cfg: Cfg, prog: &Path, dir: &Path) -> std::collections::BTreeSet<String> {
    let exe = tool(&cfg.arch.gnu("readelf")).or_else(|| tool("readelf")).unwrap();
    let t = tool_text(&exe, &["--debug-dump".into(), "-W".into(), prog.display().to_string()], dir);
    t.lines()
        .filter(|l| l.contains("readelf: Warning") || l.contains("readelf: Error"))
        .map(|l| l.chars().map(|c| if c.is_ascii_hexdigit() { '#' } else { c }).collect())
        .collect()
}

/// IN-51b's accepted readelf differences (record §A.3's causes), and
/// B3-7's: a dead DWARF 5 `.debug_addr` table no unit names (IN-51 as
/// amended) is read by readelf as the next table's "header". `w` is
/// masked as [`readelf_warnings`] masks (every hex digit `#`, letters
/// `a`-`f` and `A`-`F` too), so the patterns are masked alike.
fn accepted(w: &str) -> bool {
    let mask = |p: &str| -> String {
        p.chars().map(|c| if c.is_ascii_hexdigit() { '#' } else { c }).collect()
    };
    ["hole", "unused bytes", "Corrupt .debug_addr", "There are 0 unused bytes"]
        .iter()
        .any(|p| w.contains(&mask(p)))
}

fn verify(prog: &Path, dir: &Path) -> (bool, String) {
    let exe = tool("llvm-dwarfdump").unwrap();
    let mut c = Command::new(exe);
    c.current_dir(dir).args(["--verify".as_ref(), prog.as_os_str()]);
    let o = run_with(&mut c, b"", Duration::from_mins(3));
    (
        o.success(),
        o.stdout_text()
            .lines()
            .filter(|l| l.contains("error"))
            .take(5)
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// The judges: `u` (the update's output) against `f` (the full link).
fn judge(label: &str, cfg: Cfg, edit: Edit, dir: &Path, u: &Path, f: &Path) {
    let (gu, gf) = (gdb(cfg, edit, u, dir), gdb(cfg, edit, f, dir));
    let (gu, gf) = (norm(&gu, u), norm(&gf, f));
    assert!(gf.contains("Breakpoint 1 at"), "{label}: gdb on the full link: {gf}");
    assert_eq!(gu, gf, "{label}: gdb reads the update's output differently");
    if edit.flags.contains(&"-gsplit-dwarf") {
        // The `.dwo` was read through the skeleton: the parameter of the
        // function whose debug information the edit added is known.
        assert!(gu.contains("Symbol v is"), "{label}: gdb did not read b.dwo: {gu}");
        assert!(!gu.contains("Could not find DWO"), "{label}: {gu}");
    }
    let (lu, lf) = (lldb(cfg, edit, u, dir), lldb(cfg, edit, f, dir));
    let (lu, lf) = (norm(&lu, u), norm(&lf, f));
    assert!(lf.contains("Breakpoint 1:"), "{label}: lldb on the full link: {lf}");
    assert_eq!(lu, lf, "{label}: lldb reads the update's output differently");
    // `--verify`: the update's output has no error the full link has not
    // (the full link of C++ on x86-64 with dropped COMDAT copies has one:
    // overlapping `DW_AT_ranges` at the tombstone, recorded in WP-228 B3).
    let (vf, ef) = verify(f, dir);
    let (vu, eu) = verify(u, dir);
    let fl: std::collections::BTreeSet<&str> = ef.lines().collect();
    let new: Vec<&str> = eu.lines().filter(|l| !fl.contains(l)).collect();
    assert!(vu || new.is_empty(), "{label}: --verify on the update's output: {new:?}");
    if !vf {
        eprintln!("[wp228-b3] {label}: --verify fails on the FULL link too (same errors): {ef:?}");
    }
    let (wu, wf) = (readelf_warnings(cfg, u, dir), readelf_warnings(cfg, f, dir));
    let new: Vec<&String> = wu.iter().filter(|w| !wf.contains(*w) && !accepted(w)).collect();
    assert!(new.is_empty(), "{label}: readelf warns anew: {new:?}");
    let acc = wu.iter().filter(|w| !wf.contains(*w)).count();
    eprintln!("[wp228-b4] {label}: gdb SAME, lldb SAME, verify ok, readelf: {acc} accepted new");
}

// ---- the scenarios ----------------------------------------------------------------

/// One edit on every configuration: P, F, the update through the
/// capture's real `DEBUG_STRS` rows, judged.
fn scenario(edit: Edit) {
    scenario_on(edit, &configs(edit.langs));
}

fn scenario_on(edit: Edit, cfgs: &[Cfg]) {
    scenario_with(edit, cfgs, true);
}

/// `with_rows` false: a state without the `DEBUG_STRS` table (every
/// string a miss, appended, IN-56; past the tail reserve IN-57's named
/// fall-back): never a wrong string.
fn scenario_with(edit: Edit, cfgs: &[Cfg], with_rows: bool) {
    if !tools_required() && tool("lldb").is_none() {
        return;
    }
    let keep = std::env::var_os("MORTAR_WP228_KEEP").map(PathBuf::from);
    let tag = if with_rows { "rows" } else { "no-rows" };
    for &cfg in cfgs {
        let Some(fx) = Fixture::new(cfg, edit) else {
            assert!(!tools_required(), "{}: a compiler is missing", cfg.label());
            continue;
        };
        let label = fx.label();
        let rows = fx.full(1, "prev");
        let prev = std::fs::read(fx.scratch.join("prev")).unwrap();
        let new = fx.full(2, "full");
        let old = old_of(&fx);
        assert!(!rows.debug_strs.is_empty(), "{label}: the capture wrote no DEBUG_STRS rows");
        {
            let l = format!("{label} {tag}");
            let u = match update(&fx, &old, &rows, &new, &prev, with_rows) {
                Ok(u) => u,
                // Without rows every string is appended (IN-56): past the
                // merged section's tail reserve (`size/32`, small in a small
                // program) it is IN-57's named fall-back.
                Err(e) if !with_rows && e.starts_with("strings: DebugCompaction") => {
                    eprintln!(
                        "[wp228-b4] {l}: debug-compaction (the string tail): {}",
                        &e[..e.find(';').unwrap_or(e.len())]
                    );
                    continue;
                }
                Err(e) => panic!("{l}: {e}"),
            };
            let s = &u.plan.strings;
            eprintln!(
                "[wp228-b4] {l}: {} contributions {:?}, comdat in place {}, strings hits {} \
                 misses {} appended {:?}",
                u.plan.contributions.len(),
                u.plan
                    .contributions
                    .iter()
                    .map(|c| {
                        let n = String::from_utf8_lossy(&chunk_name(&rows, c.chunk)).into_owned();
                        format!("{}:{n}:{:?}:{}/{}", c.file.0, c.taken, c.size, c.allot.size)
                    })
                    .collect::<Vec<_>>(),
                u.plan.comdat_in_place.len(),
                s.hits,
                s.misses,
                s.appended.values().map(|a| a.2.len()).collect::<Vec<_>>()
            );
            // The pinned placements, per changed file in link order.
            let infos: Vec<Taken> = u
                .plan
                .contributions
                .iter()
                .filter(|c| chunk_name(&rows, c.chunk) == b".debug_info")
                .map(|c| c.taken)
                .collect();
            let pins: Vec<Option<Taken>> = std::iter::once(edit.info)
                .chain(edit.more.iter().filter(|m| m.2.is_some()).map(|m| m.3))
                .collect();
            for (k, (want, got)) in pins.iter().zip(&infos).enumerate() {
                if let Some(want) = want {
                    assert_eq!(got, want, "{l}: changed file {k}'s .debug_info placement");
                }
            }
            if let Some(want) = edit.addr {
                let got = u
                    .plan
                    .contributions
                    .iter()
                    .find(|c| chunk_name(&rows, c.chunk) == b".debug_addr")
                    .map(|c| c.taken);
                assert_eq!(got, Some(want), "{l}: b's .debug_addr placement");
            }
            if with_rows {
                assert!(s.hits > 0, "{l}: no string found through the rows");
            } else {
                assert_eq!(s.hits, 0, "{l}");
            }
            let out = write_update(&l, &prev, &u, &rows);
            let up = fx.scratch.join(format!("upd-{tag}"));
            std::fs::write(&up, &out).unwrap();
            std::fs::set_permissions(&up, std::os::unix::fs::PermissionsExt::from_mode(0o755))
                .unwrap();
            // Kept before the judges, so that a failed one is kept too.
            if let Some(k) = &keep {
                std::fs::write(fx.scratch.join(format!("upd-{tag}.map")), &u.map).unwrap();
                keep_for_plumb(&k.join(format!("{}-{}-{tag}", cfg.label(), edit.name)), &fx);
            }
            judge(&l, cfg, edit, fx.scratch.path(), &up, &fx.scratch.join("full"));
        }
    }
}

/// `MORTAR_WP228_KEEP`: the scratch's files into `d` (the update's output
/// `upd-<tag>` and its map `upd-<tag>.map`, which is the image's: the
/// update's placement and its `dead`/`hole` records; `P` and `F` with
/// their maps; the objects, `.dwo` files and sources; no state), the maps'
/// paths rewritten from the scratch to `d`, for `cargo xtask plumb
/// <d>/upd-<tag> --map <d>/upd-<tag>.map --map-format mortar`.
fn keep_for_plumb(d: &Path, fx: &Fixture) {
    std::fs::create_dir_all(d).unwrap();
    let from = fx.scratch.path().display().to_string();
    let to = d.display().to_string();
    for e in std::fs::read_dir(fx.scratch.path()).unwrap() {
        let e = e.unwrap();
        let name = e.file_name().to_string_lossy().into_owned();
        if !e.file_type().unwrap().is_file() || name.contains("state") {
            continue;
        }
        if Path::new(&name).extension().is_some_and(|e| e == "map") {
            let t = std::fs::read_to_string(e.path()).unwrap();
            std::fs::write(d.join(&name), t.replace(&from, &to)).unwrap();
        } else {
            std::fs::copy(e.path(), d.join(&name)).unwrap();
        }
    }
}

// spec: incr.md IN-50 step 1 (in place), IN-51b (the slack after it in
// neutral form); a line-shift edit: `.debug_line` and `.debug_info`
// rewritten in their allotments.
#[test]
fn in_place_line_edit_reads_as_the_full_link() {
    scenario(LINES);
}

// spec: incr.md IN-56, R-2 (a renamed local's string appended at the
// merged section's tail; every other string found through the rows and
// verified, or, without rows, appended).
#[test]
fn appended_string_reads_as_the_full_link() {
    scenario(RENAME);
}

// spec: incr.md IN-50 step 3 (the tail, with its class), R-3 (the old
// allotment a dead range in neutral form), IN-54 (`.debug_aranges`
// rewritten whole, full-link order).
#[test]
fn tail_and_dead_range_read_as_the_full_link() {
    scenario(GROW);
}

// spec: incr.md IN-50 step 2 (best fit into a dead range), R-3: `b` grows
// to the tail and its old allotment is dead; `c`, changed in the same
// update, grows past its slack and takes `b`'s old range.
#[test]
fn dead_range_reuse_reads_as_the_full_link() {
    scenario(REUSE);
}

// spec: WP-228 R-5 on image.md I-47a (B3-1): GCC `-g3` macro groups; the
// changed file's imports of groups `a` keeps name `a`'s copies, the group
// only `b` keeps stays in place (R-5 a); gdb `info macro`, `macro expand`.
#[test]
fn macro_group_import_reads_as_the_full_link() {
    let cfgs: Vec<Cfg> = configs(C_ONLY).into_iter().filter(|c| c.cc == Cc::Gcc).collect();
    scenario_on(MACROS, &cfgs);
}

// spec: incr.md IN-50, IN-56 on kiln/hod objects (C++, DWARF 5), both
// targets: the line edit and the growth to the tail.
#[test]
fn kiln_objects_read_as_the_full_link() {
    if kiln().is_none() {
        assert!(!tools_required() || std::env::var_os("MASON_KILN").is_none(), "kiln");
        return;
    }
    let cfgs: Vec<Cfg> = [Arch::Aarch64, Arch::X86_64]
        .into_iter()
        .map(|arch| Cfg { arch, cc: Cc::Kiln, cxx: true, dwarf: 5 })
        .collect();
    scenario_on(LINES, &cfgs);
    // kiln's allotments differ: the growth's placement is not pinned.
    scenario_on(Edit { info: None, ..GROW }, &cfgs);
}

// spec: incr.md IN-56 (WP-228 B4 item 2): a state without the
// `DEBUG_STRS` table: every string appended, or IN-57's named fall-back
// past the tail reserve; never a wrong string (judged as every update).
#[test]
fn state_without_debug_strs_appends_or_falls_back() {
    scenario_with(RENAME, &configs(C_ONLY), false);
}

/// DWARF 5 configurations of GCC and Clang, C, both targets.
fn dwarf5_gcc_clang() -> Vec<Cfg> {
    configs(C_ONLY).into_iter().filter(|c| c.dwarf == 5).collect()
}

// spec: WP-228 B3-7 (WP-229 section 8 item 3a): DWARF 5 split DWARF goes
// through the update; `.debug_addr` to the tail, its old range a dead
// DWARF 5 table (IN-51 as amended: a dead table with a header need not be
// named); gdb and lldb read the `.dwo` files through the new skeletons.
#[test]
fn split5_skeleton_edit_reads_as_the_full_link() {
    scenario_on(SPLIT5, &dwarf5_gcc_clang());
}

// spec: WP-228 B3-7, B3-4 (G-1c): a changed or removed file with GNU split
// DWARF of DWARF 4 (a headerless `.debug_addr` a dead range must keep
// named, which `Hole` cannot say yet) is `debug-unit-kind`, before the
// update writes anything.
#[test]
fn dwarf4_split_changed_file_is_debug_unit_kind() {
    let cfgs: Vec<Cfg> = configs(C_ONLY).into_iter().filter(|c| c.dwarf == 4).collect();
    for edit in [SPLIT4, SPLIT4_GONE] {
        for &cfg in &cfgs {
            let Some(fx) = Fixture::new(cfg, edit) else {
                assert!(!tools_required(), "{}: a compiler is missing", cfg.label());
                continue;
            };
            let label = fx.label();
            let rows = fx.full(1, "prev");
            let prev = std::fs::read(fx.scratch.join("prev")).unwrap();
            let new = fx.full(2, "full");
            let old = old_of(&fx);
            match update(&fx, &old, &rows, &new, &prev, true) {
                Err(e) if e.starts_with("place: debug-unit-kind") && e.contains("G-1c") => {
                    eprintln!("[wp228-b4] {label}: {e}");
                }
                Err(e) => panic!("{label}: not debug-unit-kind: {e}"),
                Ok(_) => panic!("{label}: a DWARF 4 split file went through the update"),
            }
        }
    }
}

// spec: incr.md IN-50, IN-56 on gfortran objects (WP-228 B4 item 6):
// a changed Fortran file among C files (GCC, DWARF 4 and 5, both
// targets).
#[test]
fn fortran_unit_edit_reads_as_the_full_link() {
    let cfgs: Vec<Cfg> = configs(C_ONLY).into_iter().filter(|c| c.cc == Cc::Gcc).collect();
    scenario_on(FORTRAN, &cfgs);
}

// ---- item 4: the update's debug steps at scale ------------------------------------

/// The median of `n` runs of `f`, after one warm-up.
fn median_of<T>(n: usize, mut f: impl FnMut() -> T) -> (Duration, T) {
    let mut last = f();
    let mut ts = Vec::new();
    for _ in 0..n {
        let t = std::time::Instant::now();
        last = f();
        ts.push(t.elapsed());
    }
    ts.sort_unstable();
    (ts[ts.len() / 2], last)
}

// WP-228 B3 item 4: `strings`, `place` and `aranges` on the state of a
// real `-g` link (the response file `MORTAR_WP228_RSP`, run in
// `MORTAR_WP228_CWD`), for a one-file change of the file with the largest,
// the median and the smallest `.debug_info` (its own bytes again: a body
// edit that keeps every size). Prints wall times (median of 5) and what
// each step reads.
#[test]
#[ignore = "a measurement: MORTAR_WP228_RSP and MORTAR_WP228_CWD"]
#[allow(
    clippy::too_many_lines,
    clippy::many_single_char_names,
    reason = "one measurement in order"
)]
fn measure_debug_steps_at_scale() {
    let (Some(rsp), Some(cwd)) =
        (std::env::var_os("MORTAR_WP228_RSP"), std::env::var_os("MORTAR_WP228_CWD"))
    else {
        return;
    };
    let cwd = PathBuf::from(cwd);
    let scratch = Scratch::new("wp228-b3-measure");
    let words: Vec<String> = std::fs::read_to_string(&rsp)
        .unwrap()
        .lines()
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect();
    let out = scratch.join("prog");
    let mut line = with_output(&words, &out);
    line.extend(["--incremental=full", "--incremental-state"].iter().map(OsString::from));
    line.push(scratch.join("prog.mortar-state").into_os_string());
    let sink: Rows5Sink = std::sync::Arc::default();
    let t = std::time::Instant::now();
    let inv = mortar::invoke_state5(
        "mortar",
        &line,
        &cwd,
        State5 { path: scratch.join("prog.state5"), collide: false, rows: Some(sink.clone()) },
    );
    assert!(matches!(inv.outcome, mortar::Outcome::Linked { .. }), "{:?}", inv.outcome);
    eprintln!("[wp228-b3-measure] full link (in-process, debug build) {:?}", t.elapsed());
    let rows = sink.lock().unwrap().take().expect("rows");
    let prev = std::fs::read(&out).unwrap();
    let store = support::store();
    let parsed = mortar_cli::parse(&line);
    assert!(!parsed.failed());
    let diag = Collector::new(Policy::default());
    let env = Env { cwd: &cwd, pool: Pool::new(2), replay: None, slack: true, state5: true };
    let f = front(parsed.plan, env, &store, &diag, &mut Timer::new(false)).expect("front");
    let strs = debug_strs_of(&rows);
    let view = rows.view();
    let lo = LayoutOf { view: &view };
    let stored = StoredLayout::new(&lo);
    let mut by: Vec<(u64, FileId)> = Vec::new();
    for (file, r) in f.resolution.section_base.iter() {
        let s: u64 = (0..r.len())
            .map(|k| r.nth(k).unwrap())
            .filter(|&id| sec(&f.inputs, &f.resolution, id).0 == b".debug_info")
            .map(|id| rows.sections[id.0 as usize].size)
            .sum();
        if s > 0 {
            by.push((s, file));
        }
    }
    by.sort_unstable();
    let debug_chunks: Vec<ChunkId> = stored
        .chunks()
        .unwrap()
        .into_iter()
        .filter(|(id, _)| chunk_name(&rows, *id).starts_with(b".debug_"))
        .map(|c| c.0)
        .collect();
    let pa = section(&prev, ".debug_aranges").unwrap_or(&[]).to_vec();
    eprintln!(
        "[wp228-b3-measure] files with debug {}; rows: sections {}, DEBUG_STRS {}; \
         output {} B; .debug_aranges {} B; debug chunks {}",
        by.len(),
        rows.sections.len(),
        strs.len(),
        prev.len(),
        pa.len(),
        debug_chunks.len()
    );
    for (tag, &(size, file)) in
        [("largest", by.last().unwrap()), ("median", &by[by.len() / 2]), ("smallest", &by[0])]
    {
        let r = f.resolution.section_base[file];
        let first = SectionId(rows.sections.len() as u32);
        let mut changed = ChangedFiles::default();
        changed_of(&rows, &rows, file, file, first, &mut changed);
        let mut inputs = Vec::new();
        for k in 0..r.len() {
            let (name, data) = sec(&f.inputs, &f.resolution, r.nth(k).unwrap());
            if let Some(class) = debug::str_class(&name) {
                inputs.push((file, class, data));
            }
        }
        let in_bytes: usize = inputs.iter().map(|i| i.2.len()).sum();
        let lookup = |class: u8, hash: u64| Ok(strs.get(&(class, hash)).copied());
        let (t_rows, s_rows) =
            median_of(5, || debug::strings_with(&lookup, &stored, &inputs, &prev).unwrap());
        let none = |_: u8, _: u64| Ok(None);
        let (t_none, s_none) = median_of(5, || debug::strings_with(&none, &stored, &inputs, &prev));
        let mut killed = Vec::new();
        for k in 0..r.len() {
            let id = r.nth(k).unwrap();
            let row = &rows.sections[id.0 as usize];
            if row.sh_flags & 2 == 0
                && sec(&f.inputs, &f.resolution, id).0.starts_with(b".debug_")
                && f.layout.placement.place[id].get().is_some()
            {
                killed.push(id);
            }
        }
        // `place` on a fresh delta each run (the clone outside the time).
        let mut ts = Vec::new();
        let mut plan = None;
        let mut holes = 0;
        for _ in 0..6 {
            let mut d = LayoutDelta {
                placement: f.layout.placement.clone(),
                files: vec![file],
                killed: killed.clone(),
                ..LayoutDelta::default()
            };
            let s = s_rows.clone();
            let t = std::time::Instant::now();
            let p = debug::place(&view, &stored, &changed, s, &mut d);
            ts.push(t.elapsed());
            plan = Some(p);
            holes = debug_chunks
                .iter()
                .map(|&c| d.holes_of(&stored, c).map_or(0, |h| h.len()))
                .sum::<usize>();
        }
        ts.remove(0);
        ts.sort_unstable();
        let plan = plan.unwrap();
        let (t_ar, ar_len) = match &plan {
            Ok(p) if !pa.is_empty() && !p.aranges.is_empty() => {
                let (_, os, oe) = p.aranges[0];
                // The file's own sets again (a body edit keeps them).
                let mut sets = Vec::new();
                let mut at = 0;
                while at + 10 <= pa.len() {
                    let len = u32_at(&pa, at) as usize;
                    if len == 0 {
                        break;
                    }
                    if (os..oe).contains(&u64::from(u32_at(&pa, at + 6))) {
                        sets.extend_from_slice(&pa[at..at + 4 + len]);
                    }
                    at += 4 + len;
                }
                let (t, v) = median_of(5, || debug::aranges(&pa, &[(os, oe, sets.clone())]));
                (Some(t), v.map_or(0, |v| v.len()))
            }
            _ => (None, 0),
        };
        eprintln!(
            "[wp228-b3-measure] {tag}: .debug_info {size} B; its strings {in_bytes} B; \
             strings(rows) {t_rows:?} hits {} misses {} appended {} B; \
             strings(no rows) {t_none:?} {}; place {:?} -> {}; holes rows read {holes}; \
             aranges {t_ar:?} writes {ar_len} B",
            s_rows.hits,
            s_rows.misses,
            s_rows.appended.values().map(|a| a.2.len()).sum::<usize>(),
            match &s_none {
                Ok(s) =>
                    format!("appended {} B", s.appended.values().map(|a| a.2.len()).sum::<usize>()),
                Err(e) => format!("{} ({e:?})", e.name()),
            },
            ts[ts.len() / 2],
            match &plan {
                Ok(p) =>
                    format!("{:?}", p.contributions.iter().map(|c| c.taken).collect::<Vec<_>>()),
                Err(e) => format!("{} ({e:?})", e.name()),
            },
        );
    }
    drop(f);
    std::fs::remove_file(&out).unwrap();
}

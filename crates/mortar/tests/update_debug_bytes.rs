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
//! it runs the update's debug steps on the real rows ([`debug::strings`]'
//! core over `DEBUG_STRS` rows emulated from `P`'s merge, as WP-247's
//! capture will store them, or over none; `place` on the delta
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
        t.push_str(&format!("    long a_member_field_number_{i:04};\n"));
    }
    t + "};\n"
}

fn a_c() -> String {
    bulk() + A_C
}

/// `b.c`, before every edit: `area` and a small type of its own.
const B_OLD: &str = r#"struct pt { int x, y; };
struct small_b { long s1, s2; };
static int scale = 5;
int area(struct pt *p) {
    int w = p->x + scale;
    int h = p->y;
    return w * h; /*B*/
}
"#;

/// One edit of `b.c` that leaves its allocated bytes alone.
#[derive(Debug, Clone, Copy)]
struct Edit {
    name: &'static str,
    /// The new `b.c`.
    b: &'static str,
    /// The placement the edit's `.debug_info` must take, if the test pins it.
    info: Option<Taken>,
}

const LINES: Edit = Edit {
    name: "lines",
    b: r#"struct pt { int x, y; };
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
"#,
    info: Some(Taken::InPlace),
};

const RENAME: Edit = Edit {
    name: "rename",
    b: r#"struct pt { int x, y; };
struct small_b { long s1, s2; };
static int scale = 5;
int area(struct pt *p) {
    int width_renamed_for_wp228 = p->x + scale;
    int h = p->y;
    return width_renamed_for_wp228 * h; /*B*/
}
"#,
    info: Some(Taken::InPlace),
};

const GROW: Edit = Edit {
    name: "grow",
    b: r#"struct pt { int x, y; };
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
"#,
    info: Some(Taken::Tail),
};

/// A compiler configuration.
#[derive(Debug, Clone, Copy)]
struct Cfg {
    arch: Arch,
    clang: bool,
    dwarf: u8,
}

impl Cfg {
    fn label(self) -> String {
        format!(
            "{}-{}-dwarf{}",
            self.arch.name(),
            if self.clang { "clang" } else { "gcc" },
            self.dwarf
        )
    }

    /// The driver's name and its leading flags.
    fn driver(self) -> (String, Vec<String>) {
        let mut f = Vec::new();
        let cc = if self.clang {
            f.extend(self.arch.clang_args());
            "clang".to_owned()
        } else {
            self.arch.gnu("gcc")
        };
        f.extend(
            ["-g", &format!("-gdwarf-{}", self.dwarf), "-O0", "-fno-eliminate-unused-debug-types"]
                .map(String::from),
        );
        (cc, f)
    }
}

fn configs() -> Vec<Cfg> {
    let mut v = Vec::new();
    for arch in [Arch::Aarch64, Arch::X86_64] {
        for clang in [false, true] {
            for dwarf in [4, 5] {
                v.push(Cfg { arch, clang, dwarf });
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
    /// The driver's link line (`-o out`), objects `a.c.o` and `b.c.o`.
    args: Vec<String>,
}

fn compile(cfg: Cfg, dir: &Path, src: &str, obj: &str) {
    let (cc, flags) = cfg.driver();
    let cc = tool(&cc).unwrap();
    let mut c = Command::new(cc);
    c.current_dir(dir).args(&flags).args(["-c", src, "-o", obj]);
    let o = run_with(&mut c, b"", Duration::from_mins(2));
    assert!(o.success(), "{}: {}", cfg.label(), o.stderr_text());
}

impl Fixture {
    fn new(cfg: Cfg, edit: Edit) -> Option<Self> {
        let (cc, flags) = cfg.driver();
        let scratch = Scratch::new(&format!("wp228-b3-{}-{}", cfg.label(), edit.name));
        let flags: Vec<&str> = flags.iter().map(String::as_str).collect();
        let sources =
            [Source { name: "a.c", text: "" }, Source { name: "b.c", text: B_OLD }];
        let built = build_by(&scratch, &cc, &flags, &sources)?;
        scratch.write("a.c", a_c());
        // Every object compiled alike (relative names, the scratch as the
        // directory): the link line's objects are replaced.
        compile(cfg, scratch.path(), "a.c", "a.c.o");
        compile(cfg, scratch.path(), "b.c", "b1.o");
        std::fs::remove_file(scratch.join("b.c.o")).unwrap();
        scratch.write("b.c", edit.b);
        compile(cfg, scratch.path(), "b.c", "b2.o");
        Some(Self { cfg, edit, scratch, args: built.args })
    }

    fn label(&self) -> String {
        format!("{} {}", self.cfg.label(), self.edit.name)
    }

    /// The link line of version `v` (1 old, 2 new) to `out`, incremental mode.
    fn line(&self, v: u8, out: &str) -> Vec<OsString> {
        let args: Vec<String> = self
            .args
            .iter()
            .map(|a| if a.ends_with("b.c.o") { self.scratch.join(format!("b{v}.o")).display().to_string() } else { a.clone() })
            .collect();
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
}

fn old_of(fx: &Fixture) -> Old {
    let store = support::store();
    let parsed = mortar_cli::parse(&fx.line(1, "prev-front"));
    assert!(!parsed.failed());
    let diag = Collector::new(Policy::default());
    let env = Env { cwd: fx.scratch.path(), pool: Pool::new(2), replay: None, slack: true, state5: true };
    let f = front(parsed.plan, env, &store, &diag, &mut Timer::new(false)).expect("front");
    let mut frags = HashMap::new();
    for (id, fr) in f.merge.split.frags.iter() {
        let Some(off) = f.layout.merge.offset(id) else { continue };
        let (name, data) = sec(&f.inputs, &f.resolution, fr.section);
        let bytes = data[fr.offset as usize..(fr.offset + fr.size) as usize].to_vec();
        frags.entry((name, bytes)).or_insert(off);
    }
    Old { layout: f.layout.clone(), frags, base: f.resolution.section_base.clone() }
}

/// The `DEBUG_STRS` rows WP-247's capture will store for `P` (every live
/// debug string fragment's hash and offset), as a lookup.
fn emulated_debug_strs(old: &Old) -> HashMap<(u8, u64), u32> {
    let mut m = HashMap::new();
    for ((name, bytes), &off) in &old.frags {
        let Some(class) = debug::str_class(name) else { continue };
        let s = bytes.strip_suffix(&[0]).unwrap_or(bytes);
        m.entry((class, name_hash(s, b""))).or_insert(u32::try_from(off).unwrap());
    }
    m
}

/// The changed file `b` from the new capture's rows.
fn changed_of(old: &MemoryState, new: &MemoryState, b: FileId, first: SectionId) -> ChangedFiles {
    let base = old.strings.len() as u32;
    let mut cf = ChangedFiles::default();
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
    for r in new.sections.iter().filter(|r| r.file == b.0) {
        let name = intern(r.name, &mut cf);
        let anchor = if r.anchor_kind == 1 { intern(r.anchor, &mut cf) } else { r.anchor };
        sections.push(SectionRow {
            name,
            anchor,
            place_chunk: mortar_incr::format::NONE,
            place_offset: 0,
            ..*r
        });
    }
    cf.files.push(ChangedFile {
        file: b,
        change: Change::Changed,
        row: old.files[b.0 as usize].clone(),
        first_section: first,
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
    });
    cf
}

/// What the update decided, and the image of the new inputs with its
/// placement.
struct Update {
    plan: DebugPlan,
    /// The image of the new inputs with the update's placement.
    image: Vec<u8>,
}

/// The update of `fx` (old link `old`, its rows `rows`, previous output
/// `prev`; new rows `new`): `debug::strings`' core over `strs` (emulated
/// `DEBUG_STRS`, or none), `place`, and the image with that placement.
#[allow(clippy::too_many_lines)]
fn update(
    fx: &Fixture,
    old: &Old,
    rows: &MemoryState,
    new: &MemoryState,
    prev: &[u8],
    strs: Option<&HashMap<(u8, u64), u32>>,
) -> Result<Update, String> {
    let store = support::store();
    let parsed = mortar_cli::parse(&fx.line(2, "upd-image"));
    assert!(!parsed.failed());
    let diag = Collector::new(Policy::default());
    let out: RefCell<Option<Result<DebugPlan, String>>> = RefCell::new(None);
    let hook = |link: &mortar_incr::capture::Link<'_, '_>, _computed: &Layout| {
        let r = (|| -> Result<(Layout, DebugPlan), String> {
            // The changed file: the one whose path is `b2.o`.
            let b = (0..link.inputs.files.len() as u32)
                .map(FileId)
                .find(|&f| {
                    link.inputs.link.opened.iter().any(|o| {
                        matches!(o.kind, mortar_core::inputs::OpenedKind::Object(id) if id == f)
                            && o.path.ends_with(b"b2.o")
                    })
                })
                .ok_or("no b2.o")?;
            let nb = link.resolution.section_base[b];
            let ob = old.base[b];
            if nb.len() != ob.len() {
                return Err("the edit changed b's section count".into());
            }
            let view = rows.view();
            let lo = LayoutOf { view: &view };
            let stored = StoredLayout::new(&lo);
            let first = SectionId(rows.sections.len() as u32);
            let changed = changed_of(rows, new, b, first);
            // (1) strings.
            let mut inputs = Vec::new();
            for k in 0..nb.len() {
                let (name, data) = sec(link.inputs, link.resolution, nb.nth(k).unwrap());
                if let Some(class) = debug::str_class(&name) {
                    inputs.push((b, class, data));
                }
            }
            let empty = HashMap::new();
            let strs = strs.unwrap_or(&empty);
            let lookup = |class: u8, hash: u64| Ok(strs.get(&(class, hash)).copied());
            let strings = debug::strings_with(&lookup, &stored, &inputs, prev)
                .map_err(|e| {
                    let miss: Vec<String> = inputs
                        .iter()
                        .flat_map(|(_, c, d)| d.split(|&x| x == 0).map(move |w| (*c, w)))
                        .filter(|(c, w)| !w.is_empty() && !strs.contains_key(&(*c, name_hash(w, b""))))
                        .map(|(c, w)| format!("{c}:{}", String::from_utf8_lossy(w)))
                        .collect();
                    format!("strings: {e:?}; not in the rows: {miss:?}; rows {}", strs.len())
                })?;
            // (2) what layout(debug: true) gives a debug-only edit.
            let mut d = LayoutDelta::default();
            d.placement = old.layout.placement.clone();
            d.files = vec![b];
            for k in 0..ob.len() {
                let id = ob.nth(k).unwrap();
                let r = &rows.sections[id.0 as usize];
                let name = view.string(r.name).map_err(|e| e.name().to_owned())?;
                if r.sh_flags & 2 == 0 && name.starts_with(b".debug_") && d.placement.place[id].get().is_some() {
                    d.killed.push(id);
                }
            }
            // (3) place.
            let plan = debug::place(&view, &stored, &changed, strings, &mut d)
                .map_err(|e| format!("place: {} ({e:?})", e.name()))?;
            // (4) the layout of the new link with the update's placement.
            let mut l = old.layout.clone();
            l.placement.chunks = d.placement.chunks.clone();
            l.placement.holes = d.placement.holes.clone();
            for (chunk, at, bytes) in plan.strings.appended.values() {
                l.placement.chunks[*chunk].size = at + bytes.len() as u64;
            }
            let n = link.resolution.section_base.iter().map(|(_, r)| r.end.0).max().unwrap_or(0);
            let mut place = Column::filled(n, Place::NONE);
            for (f, r) in link.resolution.section_base.iter() {
                for k in 0..r.len() {
                    let id = r.nth(k).unwrap();
                    let oid = old.base[f].nth(k).unwrap();
                    place[id] = if f == b {
                        let (name, _) = sec(link.inputs, link.resolution, id);
                        let flags = rows.sections[oid.0 as usize].sh_flags;
                        let placed_by_place = !matches!(
                            debug::form(&name).map(|f| f.policy),
                            Some(debug::Policy::Strings | debug::Policy::Whole)
                        );
                        if flags & 2 == 0 && name.starts_with(b".debug_") && placed_by_place {
                            d.placement.place.get(SectionId(first.0 + k)).copied().unwrap_or(Place::NONE)
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
                    if name == b".debug_aranges" && let Some((c, o)) = place[id].get() {
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
                let in_b = link.resolution.file_of(fr.section) == Some(b);
                let off = match debug::str_class(&name) {
                    Some(class) if in_b => plan
                        .strings
                        .output_offset(b, class, u64::from(fr.offset))
                        .ok_or("a string of b without an output offset")?,
                    _ => match old.frags.get(&(name.clone(), bytes)) {
                        Some(&o) => o,
                        None if in_b => return Err(format!("a new fragment of b in {}", String::from_utf8_lossy(&name))),
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
        replay: Some(&hook as &mortar::front::Replay<'_>),
        slack: true,
        state5: false,
    };
    let f = front(parsed.plan, env, &store, &diag, &mut Timer::new(false));
    let plan = out.borrow_mut().take().ok_or("the replay did not run")??;
    let mut f = f.ok_or_else(|| format!("front: {:?}", diag.finish()))?;
    let path = fx.scratch.join("upd-image");
    mortar::image::write(&mut f, &store, &path, &|_| {}, &mut |_, _| {})
        .map_err(|e| format!("image: {:?}", e.diagnostics))?;
    let image = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    Ok(Update { plan, image })
}

/// The update's output: a copy of `prev` with what an update writes
/// (module doc). Checks the image agrees with `prev` where the update
/// writes nothing.
fn write_update(label: &str, prev: &[u8], u: &Update, rows: &MemoryState) -> Vec<u8> {
    let img = &u.image;
    assert_eq!(img.len(), prev.len(), "{label}: the image with the update's placement has P's size");
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
    if let (Some(pa), Some(ia)) = (section(prev, ".debug_aranges"), section(img, ".debug_aranges")) {
        let info = u.plan.contributions.iter().find(|c| chunk_name(rows, c.chunk) == b".debug_info");
        let mut replaced = Vec::new();
        if let Some(c) = info {
            let (s, e) = (c.allot.offset, c.allot.offset + c.size);
            let mut sets = Vec::new();
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
            let (_, os, oe) = u.plan.aranges[0];
            replaced.push((os, oe, sets));
        }
        let new = debug::aranges(pa, &replaced).unwrap();
        assert_eq!(new, ia, "{label}: IN-54's rewrite equals the full link's order");
        let sh = shdrs(prev).into_iter().find(|s| s.name == ".debug_aranges").unwrap();
        out[sh.offset as usize..sh.offset as usize + new.len()].copy_from_slice(&new);
    }
    // The section header table: sizes of the grown debug sections.
    let (ps, is) = (shdrs(prev), shdrs(img));
    assert_eq!(ps.len(), is.len(), "{label}: section count");
    for (p, i) in ps.iter().zip(&is) {
        assert_eq!((&p.name, p.offset), (&i.name, i.offset), "{label}: section headers");
        if p.size != i.size {
            assert!(i.name.starts_with(".debug_"), "{label}: {} changed size", i.name);
            out[p.at..p.at + 8].copy_from_slice(&i.size.to_le_bytes());
        }
    }
    // Where the update writes nothing, the image of the update's
    // placement holds P's bytes: every allocated byte (a debug-only edit;
    // the build id excepted) and every debug byte outside the update's
    // ranges but the merged strings' (P's prefix stays).
    for s in &is {
        let (a, e) = (s.offset as usize, (s.offset + s.size) as usize);
        if s.size == 0 || s.sh_type == 8 || e > img.len() || s.name == ".debug_str" || s.name == ".debug_line_str"
            || s.name.starts_with(".note") || s.name == ".symtab" || s.name == ".strtab" || s.name == ".shstrtab"
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
fn gdb(cfg: Cfg, prog: &Path, dir: &Path, b: &str) -> String {
    let (bl, al) = (line_of(b, "/*B*/"), line_of(&a_c(), "/*A*/"));
    let mut ex: Vec<String> = ["set pagination off", "set width 0", "set confirm off"].map(String::from).to_vec();
    ex.push(format!("break b.c:{bl}"));
    ex.push(format!("break a.c:{al}"));
    ex.push(format!("info line b.c:{bl}"));
    ex.push("info line helper".into());
    ex.push("ptype struct pt".into());
    ex.push("ptype struct small_b".into());
    ex.push("info scope area".into());
    ex.push("info address g_count".into());
    ex.push("list area".into());
    if cfg.arch.is_host() {
        ex.extend(["run > /dev/null 2>&1", "bt", "info locals", "print *p", "print scale", "print g_count", "next", "continue", "bt", "print local_a", "continue"].map(String::from));
    }
    let mut args: Vec<String> = ["-batch", "-nx", "-iex", "set debuginfod enabled off"].map(String::from).to_vec();
    for e in ex {
        args.push("-ex".into());
        args.push(e);
    }
    args.push(prog.display().to_string());
    let exe = tool(cfg.arch.gdb()).unwrap();
    tool_text(&exe, &args, dir)
}

fn lldb(cfg: Cfg, prog: &Path, dir: &Path, b: &str) -> String {
    let (bl, al) = (line_of(b, "/*B*/"), line_of(&a_c(), "/*A*/"));
    let mut o: Vec<String> = vec!["settings set target.disable-aslr false".into()];
    o.push(format!("breakpoint set -f b.c -l {bl}"));
    o.push(format!("breakpoint set -f a.c -l {al}"));
    o.push("image lookup -n area".into());
    o.push("image lookup -t pt".into());
    o.push("image lookup -t small_b".into());
    if cfg.arch.is_host() {
        o.extend(["process launch -o /dev/null -e /dev/null", "bt", "frame variable", "target variable g_count", "continue", "bt", "frame variable local_a", "continue"].map(String::from));
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
                let stack = w.trim_start_matches('0').starts_with("ff") && n >= 10;
                // A DIE's offset (lldb's `id`) is a position, not a meaning:
                // an update moves units (IN-50).
                out.push_str(if stack { "0xSTACK" } else if die { "0xDIE" } else { &rest[..2 + n] });
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

/// IN-51b's accepted readelf differences (record §A.3's causes).
fn accepted(w: &str) -> bool {
    ["hole", "unused bytes", "Corrupt .debug_addr", "There are ### unused bytes"].iter().any(|p| w.contains(p))
}

fn verify(prog: &Path, dir: &Path) -> (bool, String) {
    let exe = tool("llvm-dwarfdump").unwrap();
    let mut c = Command::new(exe);
    c.current_dir(dir).args(["--verify".as_ref(), prog.as_os_str()]);
    let o = run_with(&mut c, b"", Duration::from_mins(3));
    (o.success(), o.stdout_text().lines().filter(|l| l.contains("error")).take(5).collect::<Vec<_>>().join("\n"))
}

/// The judges: `u` (the update's output) against `f` (the full link).
fn judge(label: &str, cfg: Cfg, dir: &Path, u: &Path, f: &Path, b: &str) {
    let (gu, gf) = (gdb(cfg, u, dir, b), gdb(cfg, f, dir, b));
    let (gu, gf) = (norm(&gu, u), norm(&gf, f));
    assert!(gf.contains("Breakpoint 1 at"), "{label}: gdb on the full link: {gf}");
    assert_eq!(gu, gf, "{label}: gdb reads the update's output differently");
    let (lu, lf) = (lldb(cfg, u, dir, b), lldb(cfg, f, dir, b));
    let (lu, lf) = (norm(&lu, u), norm(&lf, f));
    assert!(lf.contains("Breakpoint 1:"), "{label}: lldb on the full link: {lf}");
    assert_eq!(lu, lf, "{label}: lldb reads the update's output differently");
    let (vf, ef) = verify(f, dir);
    assert!(vf, "{label}: --verify on the full link: {ef}");
    let (vu, eu) = verify(u, dir);
    assert!(vu, "{label}: --verify on the update's output: {eu}");
    let (wu, wf) = (readelf_warnings(cfg, u, dir), readelf_warnings(cfg, f, dir));
    let new: Vec<&String> = wu.iter().filter(|w| !wf.contains(*w) && !accepted(w)).collect();
    assert!(new.is_empty(), "{label}: readelf warns anew: {new:?}");
    let acc = wu.iter().filter(|w| !wf.contains(*w)).count();
    eprintln!("[wp228-b3] {label}: gdb SAME, lldb SAME, verify ok, readelf: {acc} accepted new");
}

// ---- the scenarios ----------------------------------------------------------------

/// One edit on every configuration: P, F, the update with and without
/// emulated `DEBUG_STRS` rows, each judged.
fn scenario(edit: Edit) {
    if !tools_required() && tool("lldb").is_none() {
        return;
    }
    let keep = std::env::var_os("MORTAR_WP228_KEEP").map(PathBuf::from);
    for cfg in configs() {
        let Some(fx) = Fixture::new(cfg, edit) else {
            assert!(!tools_required(), "{}: a compiler is missing", cfg.label());
            continue;
        };
        let label = fx.label();
        let rows = fx.full(1, "prev");
        let prev = std::fs::read(fx.scratch.join("prev")).unwrap();
        let new = fx.full(2, "full");
        let old = old_of(&fx);
        let strs = emulated_debug_strs(&old);
        for (with_rows, tag) in [(true, "rows"), (false, "no-rows")] {
            let l = format!("{label} {tag}");
            let u = match update(&fx, &old, &rows, &new, &prev, with_rows.then_some(&strs)) {
                Ok(u) => u,
                // Without rows every string is appended (IN-56): past the
                // merged section's tail reserve (`size/32`, small in a small
                // program) it is IN-57's named fall-back.
                Err(e) if !with_rows && e.starts_with("strings: DebugCompaction") => {
                    eprintln!("[wp228-b3] {l}: debug-compaction (the string tail): {}", &e[..e.find(';').unwrap_or(e.len())]);
                    continue;
                }
                Err(e) => panic!("{l}: {e}"),
            };
            let info = u.plan.contributions.iter().find(|c| chunk_name(&rows, c.chunk) == b".debug_info");
            if let (Some(want), Some(c)) = (edit.info, info) {
                assert_eq!(c.taken, want, "{l}: .debug_info's placement");
            }
            let s = &u.plan.strings;
            eprintln!(
                "[wp228-b3] {l}: {} contributions {:?}, strings hits {} misses {} appended {:?}",
                u.plan.contributions.len(),
                u.plan.contributions.iter().map(|c| c.taken).collect::<Vec<_>>(),
                s.hits,
                s.misses,
                s.appended.values().map(|a| a.2.len()).collect::<Vec<_>>()
            );
            if with_rows {
                assert!(s.hits > 0, "{l}: no string found through the rows");
            } else {
                assert_eq!(s.hits, 0, "{l}");
            }
            let out = write_update(&l, &prev, &u, &rows);
            let up = fx.scratch.join(format!("upd-{tag}"));
            std::fs::write(&up, &out).unwrap();
            std::fs::set_permissions(&up, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
            judge(&l, cfg, fx.scratch.path(), &up, &fx.scratch.join("full"), edit.b);
            if let Some(k) = &keep {
                let d = k.join(format!("{}-{}-{tag}", cfg.label(), edit.name));
                std::fs::create_dir_all(&d).unwrap();
                std::fs::copy(&up, d.join("upd")).unwrap();
                std::fs::copy(fx.scratch.join("full"), d.join("full")).unwrap();
                std::fs::copy(fx.scratch.join("full.map"), d.join("full.map")).unwrap();
                for src in ["a.c", "b.c"] {
                    std::fs::copy(fx.scratch.join(src), d.join(src)).unwrap();
                }
            }
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

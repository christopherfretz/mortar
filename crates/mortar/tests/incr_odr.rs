//! W1 and W2 in an incremental update read only rows (`spec/incr.md`
//! IN-71d, IN-18e; WP-220): the comparator's input of every key of a real
//! full link is built twice, from the link (`mortar_odr::foreign`) and
//! from the rows its capture stores (`COPIES`, `MENTIONS` by position,
//! `SECTIONS`, `FILES`, `FILE_INDEX`, `SYMBOLS`, `RESOLVED`, `KEY_REFS`,
//! `GROUPS`), and the two are compared field by field, then the
//! diagnostics each gives. GCC, Clang and kiln objects, on AArch64 and
//! x86-64.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use mortar::front::{Env, Front, front};
use mortar::timing::Timer;
use mortar_core::par::Pool;
use mortar_core::{FileId, SymbolId};
use mortar_diag::{Collector, Diagnostic, FileRef, Key, Policy};
use mortar_incr::capture::{Keys, Link, capture};
use mortar_incr::format::Strings;
use mortar_incr::update::odr::{
    KeyInput, copy_row, mentions_by_position, stored_display, stored_key,
};
use mortar_incr::update::testing::MemoryState;
use mortar_input::store::{Store, StorePolicy};
use mortar_odr::foreign::{
    self, Definition, Finding, Names, Subjects, compare_copies, link_names, resolved_of,
};
use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, run, tool, tools_required};

/// `_start`; `w` a NON-weak data copy (8 bytes) in group `w`; `i` an
/// `STT_GNU_IFUNC` copy; `z` a size-0 copy; `t` an `STT_TLS` copy; `k`
/// twice in this one file, in groups `two1` (8 bytes) and `two2` (4; the
/// second entry is renamed `kk` -> `k` by objcopy: one key in two groups
/// of one file); `a_abs` a strong absolute `STT_OBJECT`; a reference to
/// `m_sym` (an archive member's).
const A: &str = "\t.text\n\t.globl _start\n\t.type _start, %function\n_start:\tret\n\
    \t.size _start, .-_start\n\t.data\n\t.quad m_sym\n\
    \t.section .data.w,\"awG\",%progbits,w,comdat\n\t.globl w\n\t.type w, %object\n\
    \t.size w, 8\nw:\t.quad 1\n\
    \t.section .text.i,\"axG\",%progbits,i,comdat\n\t.weak i\n\
    \t.type i, %gnu_indirect_function\ni:\tret\n\t.size i, .-i\n\
    \t.section .bss.z,\"awG\",%nobits,z,comdat\n\t.weak z\n\t.type z, %object\n\
    \t.size z, 0\nz:\n\
    \t.section .tdata.t,\"awTG\",%progbits,t,comdat\n\t.weak t\n\t.type t, %tls_object\n\
    \t.size t, 4\nt:\t.long 1\n\
    \t.section .data.two1,\"awG\",%progbits,two1,comdat\n\t.weak k\n\t.type k, %object\n\
    \t.size k, 8\nk:\t.quad 1\n\
    \t.section .data.two2,\"awG\",%progbits,two2,comdat\n\t.weak kk\n\t.type kk, %object\n\
    \t.size kk, 4\nkk:\t.long 2\n\
    \t.globl a_abs\n\t.type a_abs, %object\n\t.set a_abs, 0x10\n\
    \t.section .note.GNU-stack,\"\",%progbits\n";

/// A WEAK copy of `w` of 4 bytes (W1 against `a.o`'s 8), `i` as data
/// (W2 against the IFUNC), `k` in `two1` again, `a_abs` a function copy
/// (W2 against the absolute object, the resolved definition first).
const B: &str = "\t.section .data.w,\"awG\",%progbits,w,comdat\n\t.weak w\n\t.type w, %object\n\
    \t.size w, 4\nw:\t.long 1\n\
    \t.section .data.i,\"awG\",%progbits,i,comdat\n\t.weak i\n\t.type i, %object\n\
    \t.size i, 8\ni:\t.quad 0\n\
    \t.section .data.two1,\"awG\",%progbits,two1,comdat\n\t.weak k\n\t.type k, %object\n\
    \t.size k, 8\nk:\t.quad 1\n\
    \t.section .text.a_abs,\"axG\",%progbits,a_abs,comdat\n\t.weak a_abs\n\
    \t.type a_abs, %function\na_abs:\tret\n\t.size a_abs, .-a_abs\n\
    \t.section .note.GNU-stack,\"\",%progbits\n";

/// An archive member (`lib.a(m.o)`, its display): `m_sym`, and a copy of
/// `w`.
const M: &str = "\t.data\n\t.globl m_sym\n\t.type m_sym, %object\n\t.size m_sym, 8\n\
    m_sym:\t.quad 0\n\
    \t.section .data.w,\"awG\",%progbits,w,comdat\n\t.weak w\n\t.type w, %object\n\
    \t.size w, 8\nw:\t.quad 1\n\t.section .note.GNU-stack,\"\",%progbits\n";

/// C++ COMDAT copies as each producer writes them: an inline variable
/// (`int` here, `long` in [`C2`]: W1), a `thread_local` one, a variable
/// template, inline functions, a static inline member.
const C1: &str = "inline int v = 1;\ninline thread_local int tl = 2;\n\
    template <class T> T tv = T(3);\ninline int f() { return v + tl + tv<int>; }\n\
    struct S { static inline long s = 4; int g() { return int(s); } };\n\
    int use1() { S x; return f() + x.g(); }\n";
const C2: &str = "inline long v = 1;\ninline thread_local int tl = 2;\n\
    template <class T> T tv = T(3);\ninline int f() { return int(v) + tl + tv<int>; }\n\
    struct S { static inline long s = 4; int g() { return int(s); } };\n\
    int use2() { S x; return f() + x.g(); }\n";

/// The workspace's kiln build, if any.
fn kiln() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("MORTAR_KILN").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .map(|a| a.join(".build/target-kiln/release/kiln++"))
        .find(|p| p.is_file())
}

/// A producer: its C++ compiler and the driver that assembles `.s`.
struct Producer {
    name: &'static str,
    cxx: PathBuf,
    cxx_args: Vec<String>,
    asm: PathBuf,
    asm_args: Vec<String>,
}

fn producers(arch: Arch) -> Vec<Producer> {
    let mut out = Vec::new();
    let gcc = arch.tool("gcc");
    if let (Some(cxx), Some(asm)) = (arch.tool("g++"), gcc.clone()) {
        out.push(Producer { name: "gcc", cxx, cxx_args: Vec::new(), asm, asm_args: Vec::new() });
    }
    if let (Some(cxx), Some(asm)) = (tool("clang++"), tool("clang")) {
        let args = arch.clang_args();
        out.push(Producer { name: "clang", cxx, cxx_args: args.clone(), asm, asm_args: args });
    }
    if let (Some(cxx), Some(asm)) = (kiln(), gcc) {
        let target = vec![format!("--target={}", arch.triple())];
        out.push(Producer { name: "kiln", cxx, cxx_args: target, asm, asm_args: Vec::new() });
    }
    out
}

fn must(program: &Path, args: &[OsString], cwd: &Path) {
    let o = run(program, args);
    assert!(
        o.success(),
        "{} {args:?} in {}: {}",
        program.display(),
        cwd.display(),
        o.stderr_text()
    );
}

/// The fixture's objects compiled by `p` in `scratch`; the link line.
fn build(scratch: &Scratch, arch: Arch, p: &Producer) -> Option<Vec<OsString>> {
    let objcopy = arch.tool("objcopy")?;
    let ar = arch.tool("ar")?;
    let dir = scratch.path();
    let obj = |n: &str| scratch.join(n).into_os_string();
    let compile = |exe: &Path, pre: &[String], src: PathBuf, out: &str| {
        let mut args: Vec<OsString> = pre.iter().map(OsString::from).collect();
        args.extend(["-O0".into(), "-c".into(), src.into_os_string(), "-o".into(), obj(out)]);
        must(exe, &args, dir);
    };
    compile(&p.asm, &p.asm_args, scratch.write("a.s", A), "a0.o");
    compile(&p.asm, &p.asm_args, scratch.write("b.s", B), "b.o");
    compile(&p.asm, &p.asm_args, scratch.write("m.s", M), "m.o");
    compile(&p.cxx, &p.cxx_args, scratch.write("c1.cc", C1), "c1.o");
    compile(&p.cxx, &p.cxx_args, scratch.write("c2.cc", C2), "c2.o");
    must(&objcopy, &["--redefine-sym".into(), "kk=k".into(), obj("a0.o"), obj("a.o")], dir);
    must(&ar, &["rcs".into(), obj("lib.a"), obj("m.o")], dir);
    let mut args: Vec<OsString> = vec!["-static".into()];
    args.extend(["a.o", "b.o", "c1.o", "c2.o", "lib.a"].map(obj));
    args.extend(["-o".into(), obj("out")]);
    Some(args)
}

fn front_of<'s>(args: &[OsString], cwd: &Path, store: &'s Store) -> Front<'s> {
    let parsed = mortar_cli::parse(args);
    assert!(!parsed.failed(), "{:?}", parsed.diagnostics);
    let diag = Collector::new(Policy::default());
    let f = front(
        parsed.plan,
        Env { cwd, pool: Pool::new(2), replay: None, slack: true, state5: false },
        store,
        &diag,
        &mut Timer::new(false),
    );
    let report = diag.finish();
    match f {
        Some(f) if !report.failed() => f,
        _ => panic!("{}", report.render(&mortar_diag::Style::plain("mortar"))),
    }
}

/// The rows a full link's state holds for W1 and W2 (format 5's shapes):
/// format 4's capture as `MemoryState::from_format4` gives it, with
/// `MENTIONS` by position and `FILE_INDEX.first_global` (WP-215 D7) and
/// `COPIES` (WP-220), each by the capture's own producer.
fn rows_of(f: &Front<'_>, defs: &[Definition]) -> MemoryState {
    let link = Link {
        inputs: &f.inputs,
        resolution: &f.resolution,
        assignment: &f.assignment,
        split: &f.merge.split,
    };
    let keys = Keys::for_capture(&link, &f.pool).expect("keys");
    let mut strings = Strings::default();
    let mut state =
        capture(&link, &keys, &f.layout, &mut strings, &|_| Some([0; 16]), f.arch.get())
            .expect("capture");
    state.strings = strings.into_bytes();
    let mut m = MemoryState::from_format4(&state);
    let row_of = |m: &MemoryState, id: SymbolId| -> u32 {
        let k = f.inputs.symbols[id];
        let found = m.view().find_key(k.name, k.version.unwrap_or_default()).expect("find_key");
        found.unwrap_or_else(|| panic!("no SYMBOLS row for {}", String::from_utf8_lossy(k.name))).0
    };
    let mut mentions = Vec::new();
    for n in 0..f.inputs.files.len() as usize {
        let file = FileId(n as u32);
        let Some((first, rows)) = mentions_by_position(&f.inputs, &f.resolution, file) else {
            continue;
        };
        if let Some(fi) = m.file_index.get_mut(n) {
            fi.first_global = first;
        }
        for mut r in rows {
            r.symbol = row_of(&m, SymbolId(r.symbol));
            mentions.push((file, r));
        }
    }
    m.mentions = mentions;
    m.copies = defs.iter().map(|d| copy_row(d, row_of(&m, d.key))).collect();
    m
}

/// A copy as both sides give it: place, size, type, group signature,
/// whether its file keeps the group.
type Copy = (u32, u32, u64, u8, Vec<u8>, bool);

fn render(
    display: &dyn Fn(FileId) -> Option<FileRef>,
    found: Vec<(usize, Finding)>,
) -> Vec<(Key, String, Vec<FileId>, usize)> {
    let mut out: Vec<(Key, String, Vec<FileId>, usize)> = found
        .into_iter()
        .map(|(w, finding)| {
            let (key, d, s): (Key, Diagnostic, Subjects) = foreign::diagnostic(display, finding);
            (key, format!("{d:?}"), s.files, w)
        })
        .collect();
    out.sort();
    out
}

/// Copies' places by group signature.
type Grouped = Vec<(Vec<u8>, Vec<(u32, u32)>)>;

/// What the fixture must have exercised, per link.
#[derive(Debug, Default)]
#[allow(clippy::struct_excessive_bools, reason = "one flag per case of the fixture")]
struct Seen {
    weak_and_strong: bool,
    ifunc: bool,
    tls: bool,
    size0: bool,
    two_groups: bool,
    absolute: bool,
    member: bool,
    w1: usize,
    w2: usize,
}

fn compare(label: &str, f: &Front<'_>) -> Seen {
    let pool = Pool::new(2);
    let defs = foreign::copies(&f.inputs, &f.resolution, &pool);
    let rows_state = rows_of(f, &defs);
    let view = rows_state.view();
    let rank = |file: u32| Ok(view.file(FileId(file))?.rank);
    let names = link_names(&f.inputs, &f.resolution);
    let display_link =
        |file: FileId| f.inputs.files.get(file).map(mortar_core::inputs::File::display);
    let display_rows = |file: FileId| stored_display(&view, file).ok();
    let mut seen = Seen::default();
    for run in defs.chunk_by(|a, b| a.key == b.key) {
        let key = run[0].key;
        let sym = f.inputs.symbols[key];
        let row =
            view.find_key(sym.name, sym.version.unwrap_or_default()).expect("find").expect("row");
        let input: KeyInput = stored_key(&view, row, &rank).expect("stored_key");
        let what = format!("{label}: {}", names.symbol(key));
        // The name.
        assert_eq!(input.name, names.symbol(key), "{what}: name");
        // Each copy, field by field.
        let ours: Vec<Copy> = run
            .iter()
            .map(|d| {
                let sig = f.inputs.comdats[d.group].name.to_vec();
                let kept = names.kept(d.group, d.file);
                (d.file.0, d.index.0, d.size, d.sym_type, sig, kept)
            })
            .collect();
        let stored: Vec<Copy> = input
            .copies
            .iter()
            .map(|c| {
                let g = &input.groups[c.group as usize];
                let kept = g.keeper == Some(FileId(c.row.file));
                (c.row.file, c.row.index, c.row.size, c.row.typ, g.signature.clone(), kept)
            })
            .collect();
        let mut want = ours.clone();
        let mut got = stored.clone();
        want.sort();
        got.sort();
        assert_eq!(want, got, "{what}: copies");
        // One group number per signature (the link's are per signature).
        for one in run {
            for two in run {
                let same_sig = f.inputs.comdats[one.group].name == f.inputs.comdats[two.group].name;
                assert_eq!(one.group == two.group, same_sig, "{what}: groups by signature");
            }
        }
        // The order within each group: the link's (file, index) is load
        // order; the rows' (current rank, index).
        let order = |v: &[Copy]| {
            let mut by: Grouped = Vec::new();
            for c in v {
                match by.iter_mut().find(|g| g.0 == c.4) {
                    Some(g) => g.1.push((c.0, c.1)),
                    None => by.push((c.4.clone(), vec![(c.0, c.1)])),
                }
            }
            by.sort();
            by
        };
        assert_eq!(order(&ours), order(&stored), "{what}: order within a group");
        // The resolved definition.
        let (outside, resolved) = resolved_of(&f.inputs, &f.resolution, key, run);
        assert_eq!((input.outside, input.resolved), (outside, resolved), "{what}: resolved");
        // Every file a copy or the resolved definition names: its display.
        for file in run.iter().map(|d| d.file).chain(resolved.map(|r| r.at.file)) {
            assert_eq!(display_rows(file), display_link(file), "{what}: display of {file:?}");
            seen.member |= display_link(file).is_some_and(|d| d.member().is_some());
        }
        // The diagnostics each side's input gives.
        let full = compare_copies(key, run, outside, || resolved, &names, true, true);
        let rows = input.compare(true, true);
        let full = render(&display_link, full);
        let rows = render(&display_rows, rows);
        assert_eq!(full, rows, "{what}: diagnostics");
        seen.w1 += full.iter().filter(|d| d.3 == 0).count();
        seen.w2 += full.iter().filter(|d| d.3 == 1).count();
        // What the fixture exercised.
        let name = sym.name;
        seen.weak_and_strong |= name == b"w" && run.len() >= 3;
        seen.ifunc |= run.iter().any(|d| d.sym_type == mortar_elf::consts::STT_GNU_IFUNC);
        seen.tls |= run.iter().any(|d| d.sym_type == mortar_elf::consts::STT_TLS);
        seen.size0 |= run.iter().any(|d| d.size == 0);
        seen.two_groups |=
            run.iter().any(|x| run.iter().any(|y| x.file == y.file && x.group != y.group));
        seen.absolute |= name == b"a_abs" && resolved.is_some();
    }
    seen
}

// spec: incr.md IN-71d, IN-18e (WP-220: W1/W2's input derivable from rows)
#[test]
fn w1_w2_input_from_rows_equals_the_links() {
    let mut linked = 0;
    for arch in [Arch::Aarch64, Arch::X86_64] {
        let list = producers(arch);
        // GCC and Clang are required where tools are; kiln is taken where
        // the workspace has a build of it (as the census does): the hosted
        // `test` step has none, kiln is required by the mason step.
        for needed in ["gcc", "clang"] {
            if !list.iter().any(|p| p.name == needed) {
                assert!(!tools_required(), "{arch:?}: {needed} is required");
            }
        }
        for p in &list {
            let label = format!("{}-{}", arch.triple(), p.name);
            let scratch = Scratch::new(&format!("incr-odr-{label}"));
            let Some(args) = build(&scratch, arch, p) else {
                assert!(!tools_required(), "{label}: objcopy and ar are required");
                continue;
            };
            let store = Store::new(StorePolicy::from_env());
            let f = front_of(&args, scratch.path(), &store);
            let seen = compare(&label, &f);
            eprintln!("{label}: {seen:?}");
            assert!(
                seen.weak_and_strong
                    && seen.ifunc
                    && seen.tls
                    && seen.size0
                    && seen.two_groups
                    && seen.absolute
                    && seen.member,
                "{label}: the fixture did not exercise every case: {seen:?}"
            );
            // W1: `w` (a.o, b.o), `v` (c1.o, c2.o); W2: `i`, `a_abs`.
            assert!(seen.w1 >= 2 && seen.w2 >= 2, "{label}: {seen:?}");
            linked += 1;
        }
    }
    eprintln!("{linked} links compared");
}

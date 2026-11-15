//! `spec/resolve.md` §4.3, §15.1 `s4_3_system_lines_match_gnu`: real link
//! lines against the reference linkers.
//!
//! Each line is compiled with its target's GCC (AArch64, x86-64, i686;
//! native on its own host, a cross compiler elsewhere), its linker
//! argument vector is taken
//! from `gcc -###`, and the inputs are read into the test model in
//! command-line order. Then:
//!
//! - the members mortar extracts equal `ld.bfd -t -t`'s (and are shown
//!   against `ld.lld --trace` and the members in `mold -Map`);
//! - for every key mortar defines from a file, the definer (file and
//!   section) equals the one `ld.bfd -Map` and `mold -Map` place the
//!   symbol in, outside COMDAT groups for GNU ld (whose keeper is the
//!   first loaded, ruling C) and everywhere for mold (whose keeper is
//!   mortar's);
//! - the simulation of GNU ld (§4.6) loads exactly `ld.bfd`'s members.
//!
//! Each line prints one table row (`--nocapture` shows them).

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use mortar_core::par::Pool;
use mortar_core::resolution::FileLive;
use mortar_core::{FileId, OptId};
use mortar_elf::{Machine, SymbolSection};
use mortar_resolve::options::{Entry, Options, Output};
use mortar_resolve::view::{FileKind, View};
use mortar_resolve::{Def, Resolved, resolve};
use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, tool};
use support::{Model, canon, exec, member_label, ok, parse_trace};

/// A target of the lines: its tools are its architecture's (by their
/// names on this host, [`Arch::gnu`]).
#[derive(Clone, Copy)]
struct Target {
    machine: Machine,
    arch: Arch,
    tag: &'static str,
}

const A64: Target = Target { machine: Machine::AArch64, arch: Arch::Aarch64, tag: "" };
const X64: Target = Target { machine: Machine::X86_64, arch: Arch::X86_64, tag: "64" };
const X86: Target = Target { machine: Machine::I386, arch: Arch::I386, tag: "32" };

const HELLO: &str = "#include <stdio.h>\nint main(void){printf(\"hi %d\\n\", 3);return 0;}\n";
const CXX: &str = r#"#include <iostream>
#include <stdexcept>
#include <string>
static int f(int x) { if (x > 2) throw std::runtime_error("big"); return x; }
int main(int argc, char**) {
  try { std::cout << f(argc + 5) << std::endl; }
  catch (const std::exception& e) { std::cout << std::string(e.what()) << '\n'; }
  return 0;
}
"#;
const MTH: &str = r#"#include <math.h>
#include <pthread.h>
#include <dlfcn.h>
#include <stdio.h>
static void* run(void* p) { return p; }
int main(int argc, char** argv) {
  pthread_t t; pthread_create(&t, 0, run, 0); pthread_join(t, 0);
  void* h = argc > 5 ? dlopen(argv[1], RTLD_NOW) : 0;
  printf("%f %p\n", sin((double)argc) + sqrt((double)argc), h);
  return 0;
}
"#;
const FORT_MAIN: &str =
    "program m\n  common /blk/ a, b\n  real a, b\n  print *, a + b\nend program m\n";
const FORT_BD: &str =
    "block data bd\n  common /blk/ a, b\n  real a, b\n  data a, b /1.0, 2.0/\nend block data bd\n";
const TPL_H: &str = r"#pragma once
template <class T> struct Box { T v; T get() const; static int count; };
template <class T> T Box<T>::get() const { return v + T(count); }
template <class T> int Box<T>::count = 1;
template <class T> inline T twice(T x) { return x + x; }
";
const TPL_A: &str =
    "#include \"tpl.h\"\nint ua(int x) { Box<int> b{x}; return b.get() + twice(x); }\n";
const TPL_B: &str = "#include \"tpl.h\"\nint ub(long x) { Box<long> b{x}; Box<int> c{1}; return int(b.get()) + c.get() + twice(2); }\n";
const TPL_MAIN: &str =
    "int ua(int); int ub(long);\nint main(int c, char**) { return ua(c) + ub(c); }\n";

const TWICE_MAIN: &str = "int u(int*);\nint main(void){int x=0;return u(&x);}\n";
// With -O1 GCC calls libgcc's out-of-line atomics (`__aarch64_cas4_*`).
const TWICE_U: &str = "int u(int*p){int e=0;return __atomic_compare_exchange_n(p,&e,1,0,5,5);}\n";

/// One link line: a driver, its flags, sources compiled into the named
/// objects, and archives of sources.
struct Line {
    name: String,
    target: Target,
    driver: &'static str,
    flags: &'static [&'static str],
    main: Vec<(&'static str, &'static str)>,
    archives: Vec<(&'static str, Vec<(&'static str, &'static str)>)>,
    headers: Vec<(&'static str, &'static str)>,
}

fn line(name: &str, target: Target) -> Line {
    let (driver, flags, main, archives, headers): (_, &'static [&'static str], _, _, _) = match name
    {
        "chello" => ("gcc", &["-static"], vec![("h.c", HELLO)], vec![], vec![]),
        "spie" => ("gcc", &["-static-pie"], vec![("h.c", HELLO)], vec![], vec![]),
        "mth" => {
            ("gcc", &["-static", "-lm", "-lpthread", "-ldl"], vec![("m.c", MTH)], vec![], vec![])
        }
        "cxx" => ("g++", &["-static"], vec![("x.cc", CXX)], vec![], vec![]),
        "fort" => (
            "gfortran",
            &["-static"],
            vec![("m.f90", FORT_MAIN)],
            vec![("libbd.a", vec![("bd.f90", FORT_BD)])],
            vec![],
        ),
        "tpl" => (
            "g++",
            &["-static"],
            vec![("main.cc", TPL_MAIN)],
            vec![("libta.a", vec![("ta.cc", TPL_A)]), ("libtb.a", vec![("tb.cc", TPL_B)])],
            vec![("tpl.h", TPL_H)],
        ),
        "twice" => (
            "gcc",
            &["-static"],
            vec![("t.c", TWICE_MAIN)],
            vec![("libu.a", vec![("u.c", TWICE_U)])],
            vec![],
        ),
        other => panic!("no line {other}"),
    };
    Line { name: format!("{name}{}", target.tag), target, driver, flags, main, archives, headers }
}

fn tool_of(name: &str) -> Option<PathBuf> {
    tool(name)
}

/// Splits `gcc -###` output's command line: arguments in double quotes
/// with backslash escapes, or bare.
fn split_quoted(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        let mut arg = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() {
                break;
            }
            chars.next();
            if c == '"' {
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => arg.extend(chars.next()),
                        c => arg.push(c),
                    }
                }
            } else {
                arg.push(c);
            }
        }
        out.push(arg);
    }
    out
}

/// An input of the line, in command-line order.
#[derive(Debug, Clone)]
enum Input {
    Object(String),
    Archive { path: String, whole: bool, group: Option<u32> },
}

/// The linker arguments of a collect2 line without the plugin options
/// and the output, and the inputs they name.
fn walk(args: &[String], dir: &Path) -> (Vec<String>, Vec<Input>, Vec<String>) {
    let mut kept = Vec::new();
    let mut inputs = Vec::new();
    let mut undefined = Vec::new();
    let mut lib_dirs: Vec<String> = Vec::new();
    let mut whole = false;
    let mut group: Option<u32> = None;
    let mut groups = 0u32;
    let mut i = 0;
    let with_arg =
        ["-m", "-z", "-dynamic-linker", "-e", "-u", "-T", "-y", "-soname", "-rpath", "-L"];
    while i < args.len() {
        let a = args[i].as_str();
        if a == "-plugin" || a == "-o" {
            i += 2;
            continue;
        }
        if a.starts_with("-plugin-opt") {
            i += 1;
            continue;
        }
        kept.push(a.to_owned());
        if with_arg.contains(&a) {
            let v = args[i + 1].clone();
            kept.push(v.clone());
            match a {
                "-L" => lib_dirs.push(v),
                "-u" => undefined.push(v),
                _ => {}
            }
            i += 2;
            continue;
        }
        i += 1;
        if let Some(d) = a.strip_prefix("-L") {
            lib_dirs.push(d.to_owned());
        } else if a == "--start-group" || a == "-(" {
            group = Some(groups);
            groups += 1;
        } else if a == "--end-group" || a == "-)" {
            group = None;
        } else if a == "--whole-archive" {
            whole = true;
        } else if a == "--no-whole-archive" {
            whole = false;
        } else if let Some(u) = a.strip_prefix("--undefined=") {
            undefined.push(u.to_owned());
        } else if let Some(l) = a.strip_prefix("-l") {
            let file = l.strip_prefix(':').map_or_else(|| format!("lib{l}.a"), str::to_owned);
            let path = lib_dirs
                .iter()
                .map(|d| format!("{d}/{file}"))
                .find(|p| Path::new(p).is_file())
                .unwrap_or_else(|| panic!("-l{l} not found in {lib_dirs:?}"));
            add_file(&path, whole, &mut group, &mut groups, &mut inputs);
        } else if !a.starts_with('-') {
            let path = if Path::new(a).is_absolute() {
                a.to_owned()
            } else {
                dir.join(a).display().to_string()
            };
            add_file(&path, whole, &mut group, &mut groups, &mut inputs);
        }
    }
    (kept, inputs, undefined)
}

fn add_file(
    path: &str,
    whole: bool,
    group: &mut Option<u32>,
    groups: &mut u32,
    inputs: &mut Vec<Input>,
) {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    if bytes.starts_with(b"!<arch>\n") {
        inputs.push(Input::Archive { path: path.to_owned(), whole, group: *group });
    } else if bytes.starts_with(b"\x7fELF") {
        inputs.push(Input::Object(path.to_owned()));
    } else {
        // A linker script: GROUP ( ... ), INPUT ( ... ), AS_NEEDED ( ... ).
        let text = String::from_utf8_lossy(&bytes);
        let text = strip_comments(&text);
        let mut words = text
            .split(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == ',')
            .filter(|w| !w.is_empty());
        while let Some(w) = words.next() {
            match w {
                "GROUP" => {
                    let outer = group.is_some();
                    if !outer {
                        *group = Some(*groups);
                        *groups += 1;
                    }
                    for f in words.by_ref() {
                        if f == "AS_NEEDED" {
                            continue;
                        }
                        if f.starts_with('/') {
                            add_file(f, whole, group, groups, inputs);
                        } else {
                            break;
                        }
                    }
                    if !outer {
                        *group = None;
                    }
                }
                "INPUT" | "AS_NEEDED" => {}
                f if f.starts_with('/') => add_file(f, whole, group, groups, inputs),
                _ => {}
            }
        }
    }
}

fn strip_comments(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("/*") {
        out.push_str(&rest[..at]);
        rest = rest[at..].find("*/").map_or("", |end| &rest[at + end + 2..]);
    }
    out.push_str(rest);
    out
}

/// A symbol's place in a map file: the input file's label and the input
/// section's name.
type Placed = BTreeMap<String, Vec<(String, String)>>;

/// The files an input section line names in a map, normalised.
fn file_label(text: &str) -> String {
    if let Some(open) = text.rfind('(')
        && text.ends_with(')')
    {
        member_label(&text[..open], &text[open + 1..text.len() - 1])
    } else {
        canon(text)
    }
}

/// `ld.bfd -Map`: input section lines and the symbols after them.
fn parse_bfd_map(text: &str) -> (Placed, BTreeSet<String>) {
    let mut placed = Placed::new();
    let mut members = BTreeSet::new();
    let mut current: Option<(String, String)> = None;
    let mut pending: Option<String> = None;
    let body = text.split("Linker script and memory map").nth(1).unwrap_or("");
    for raw in body.lines() {
        let words: Vec<&str> = raw.split_whitespace().collect();
        let indented = raw.starts_with(' ');
        if let Some(name) = pending.take()
            && words.len() == 3
            && words[0].starts_with("0x")
        {
            current = Some((file_label(words[2]), name));
            continue;
        }
        if raw.starts_with(' ')
            && !raw.starts_with("  ")
            && !words.is_empty()
            && !words[0].starts_with('*')
        {
            match words.len() {
                1 => {
                    pending = Some(words[0].to_owned());
                    current = None;
                    continue;
                }
                4 if words[1].starts_with("0x") && words[2].starts_with("0x") => {
                    current = Some((file_label(words[3]), words[0].to_owned()));
                    continue;
                }
                _ => {}
            }
        }
        if indented && words.len() == 2 && words[0].starts_with("0x") {
            if let Some(c) = &current {
                placed.entry(words[1].to_owned()).or_default().push(c.clone());
            }
            continue;
        }
        current = None;
    }
    for places in placed.values() {
        for (f, _) in places {
            if f.ends_with(')') {
                members.insert(f.clone());
            }
        }
    }
    (placed, members)
}

/// `mold -Map`: columns `VMA Size Align Out In Symbol`.
fn parse_mold_map(text: &str) -> (Placed, BTreeSet<String>) {
    let mut placed = Placed::new();
    let mut members = BTreeSet::new();
    let mut lines = text.lines();
    let header = lines.next().unwrap_or("");
    let in_col = header.find(" In ").map_or(usize::MAX, |p| p + 1);
    let sym_col = header.find("Symbol").unwrap_or(usize::MAX);
    let mut current: Option<(String, String)> = None;
    for raw in lines {
        let words: Vec<&str> = raw.split_whitespace().collect();
        if words.len() < 4 {
            continue;
        }
        let col = raw.len() - raw[raw.find(words[3]).unwrap_or(0)..].len();
        let last = words[3..].join(" ");
        if col >= sym_col {
            if let Some(c) = &current {
                placed.entry(last).or_default().push(c.clone());
            }
        } else if col >= in_col {
            current = last.rfind(":(").map(|at| {
                let file = file_label(&last[..at]);
                if file.ends_with(')') {
                    members.insert(file.clone());
                }
                (file, last[at + 2..last.len() - 1].to_owned())
            });
        } else {
            current = None;
        }
    }
    (placed, members)
}

struct Built {
    dir: Scratch,
    args: Vec<String>,
    inputs: Vec<Input>,
    undefined: Vec<String>,
}

fn build(l: &Line) -> Option<Built> {
    let driver = l.target.arch.tool(l.driver)?;
    let ar = l.target.arch.tool("ar")?;
    let dir = Scratch::new(&format!("resolve-diff-{}", l.name));
    for (name, text) in &l.headers {
        dir.write(name, text);
    }
    let compile = |src: &str, text: &str| -> String {
        let path = dir.write(src, text);
        let obj = format!("{}.o", path.display());
        let level = if l.name.starts_with("tpl") { "-O0" } else { "-O1" };
        let out = exec(
            &driver,
            &["-c".into(), level.into(), path.display().to_string(), "-o".into(), obj.clone()],
            dir.path(),
        );
        ok(&out, &format!("compiling {src}"));
        obj
    };
    let mut objs: Vec<String> = l.main.iter().map(|(s, t)| compile(s, t)).collect();
    for (lib, members) in &l.archives {
        let mobjs: Vec<String> = members.iter().map(|(s, t)| compile(s, t)).collect();
        let path = dir.join(lib).display().to_string();
        let mut args = vec!["rcs".to_owned(), path.clone()];
        args.extend(mobjs);
        ok(&exec(&ar, &args, dir.path()), "ar");
        objs.push(path);
    }
    let mut args: Vec<String> = l.flags.iter().map(|s| (*s).to_owned()).collect();
    // Flags like -lm go after the objects.
    let (libs, flags): (Vec<String>, Vec<String>) =
        args.drain(..).partition(|a| a.starts_with("-l"));
    let mut all = flags;
    all.push("-###".into());
    all.extend(objs);
    all.extend(libs);
    all.push("-o".into());
    all.push(dir.join("a.out").display().to_string());
    let out = exec(&driver, &all, dir.path());
    let text = out.stderr_text();
    let cmd = text
        .lines()
        .find(|l| l.contains("collect2"))
        .unwrap_or_else(|| panic!("no collect2 line:\n{text}"));
    let words = split_quoted(cmd);
    let (args, inputs, undefined) = walk(&words[1..], dir.path());
    Some(Built { dir, args, inputs, undefined })
}

/// The model of a line's inputs.
fn model(target: Target, inputs: &[Input]) -> Model {
    let mut m = Model::new(target.machine);
    for input in inputs {
        match input {
            Input::Object(path) => {
                let bytes = std::fs::read(path).unwrap();
                m.load_object(&bytes, &canon(path), FileKind::Object).unwrap();
            }
            Input::Archive { path, whole, group } => {
                let label = canon(path);
                let files = m.load_archive(Path::new(path), *whole, *group).unwrap();
                for f in files {
                    m.files[f.0 as usize].label.clone_from(&label);
                }
            }
        }
    }
    m
}

fn mortar(m: &mut Model, static_pie: bool, undefined: &[String], warn_backrefs: bool) -> Resolved {
    let start = m.key("_start");
    let undefined = undefined
        .iter()
        .enumerate()
        .map(|(i, u)| mortar_resolve::options::Named {
            key: OptId::some(m.key(u)),
            name: u.as_bytes().into(),
            arg: i as u32 + 1,
        })
        .collect();
    let opts = Options {
        entry: Entry::Default(OptId::some(start)),
        undefined,
        warn_backrefs,
        output: Output { pie: static_pie, dynamic: false },
        ..Options::default()
    };
    resolve(&*m, &opts, &Pool::new(4))
}

/// Stage B on the same line: `Inputs` (as `mortar_input` would build
/// it), the view over it, the `Resolution` table with its checks, and the
/// diagnostics (none may be an error).
fn over_inputs(
    target: Target,
    inputs: &[Input],
    static_pie: bool,
    undefined: &[String],
) -> (mortar_core::Inputs<'static>, Resolved) {
    use support::inputs::Item;
    let items: Vec<Item> = inputs
        .iter()
        .map(|i| match i {
            Input::Object(p) => Item::Object(PathBuf::from(p)),
            Input::Archive { path, whole, group } => {
                Item::Archive { path: PathBuf::from(path), whole: *whole, group: *group }
            }
        })
        .collect();
    let inputs = support::inputs::build(target.machine, &items).unwrap();
    let pool = Pool::new(4);
    let view = mortar_resolve::InputsView::new(&inputs, &pool);
    assert!(view.faults().is_empty(), "{:?}", view.faults());
    let key = |n: &str| view.key_of(n.as_bytes()).map_or(OptId::NONE, OptId::some);
    let opts = Options {
        entry: Entry::Default(key("_start")),
        undefined: undefined
            .iter()
            .enumerate()
            .map(|(i, u)| mortar_resolve::options::Named {
                key: key(u),
                name: u.as_bytes().into(),
                arg: i as u32 + 1,
            })
            .collect(),
        warn_backrefs: true,
        output: Output { pie: static_pie, dynamic: false },
        ..Options::default()
    };
    let r = resolve(&view, &opts, &pool);
    let table = mortar_resolve::tables::resolution(&view, &r, &opts).unwrap();
    table.check().unwrap();
    table.check_against(&inputs).unwrap();
    let diags = mortar_resolve::tables::diagnostics(&inputs, &view, &opts, &r);
    assert!(diags.iter().all(|(_, d)| d.severity() == mortar_diag::Severity::Warning), "{diags:?}");
    drop(view);
    (inputs, r)
}

/// Whether the entry's definition is in a COMDAT group.
fn in_group(m: &Model, e: mortar_core::SymEntry) -> bool {
    let Some(mention) = m.mention(e.file, e.index) else { return false };
    let SymbolSection::Section(s) = mention.place else { return false };
    m.files[e.file.0 as usize].groups.groups.iter().any(|g| {
        g.flags == mortar_elf::consts::GRP_COMDAT
            && m.files[e.file.0 as usize].groups.members_of(g).contains(&s.0)
    })
}

/// Mortar's definers against a map: keys compared and those that differ;
/// with `skip_groups`, the keys defined in COMDAT groups are counted
/// apart (compared, differing).
fn definers(
    m: &Model,
    r: &Resolved,
    map: &Placed,
    skip_groups: bool,
) -> (usize, Vec<String>, (usize, usize)) {
    let mut compared = 0;
    let mut bad = Vec::new();
    let mut group_keys = (0, 0);
    for (k, d) in r.def.iter().enumerate() {
        let Some(e) = d.entry() else { continue };
        let name = m.name(mortar_core::SymbolId(k as u32));
        if name.contains('@') {
            continue;
        }
        let Some(mention) = m.mention(e.file, e.index) else { continue };
        let section = match mention.place {
            SymbolSection::Section(s) => {
                String::from_utf8_lossy(&m.files[e.file.0 as usize].sections[s.0 as usize].name)
                    .into_owned()
            }
            SymbolSection::Common => "COMMON".to_owned(),
            _ => continue,
        };
        let Some(places) = map.get(&name) else { continue };
        let [(file, sec)] = places.as_slice() else { continue };
        let ours = m.label(e.file);
        let sec_ok = sec == &section
            || (section == "COMMON" && (sec == ".bss" || sec.starts_with("COMMON")));
        if skip_groups && in_group(m, e) {
            group_keys.0 += 1;
            group_keys.1 += usize::from(file != &ours || !sec_ok);
            continue;
        }
        compared += 1;
        if file != &ours || !sec_ok {
            bad.push(format!("{name}: mortar {ours}:{section}, map {file}:{sec}"));
        }
    }
    (compared, bad, group_keys)
}

fn check_line(name: &str, target: Target) {
    let l = line(name, target);
    let Some(built) = build(&l) else { return };
    let Some(bfd) = target.arch.tool("ld.bfd") else { return };
    let Some(lld) = tool_of("ld.lld") else { return };
    let Some(mold) = tool_of("mold") else { return };
    let dir = built.dir.path().to_path_buf();
    let link = |program: &Path, extra: &[&str], out: &str| {
        let mut args = built.args.clone();
        args.extend(extra.iter().map(|s| (*s).to_owned()));
        args.push("-o".into());
        args.push(dir.join(out).display().to_string());
        let o = exec(program, &args, &dir);
        ok(&o, &format!("{} on {}", program.display(), l.name));
        o
    };
    let bfd_map = dir.join("bfd.map").display().to_string();
    let mold_map = dir.join("mold.map").display().to_string();
    let bfd_out = link(&bfd, &["-t", "-t", "--no-demangle", &format!("-Map={bfd_map}")], "out.bfd");
    let lld_out = link(&lld, &["--trace"], "out.lld");
    link(&mold, &["--no-demangle", &format!("-Map={mold_map}")], "out.mold");
    let bfd_members = parse_trace(&bfd_out.stdout_text());
    let lld_members = parse_trace(&lld_out.stdout_text());
    let (bfd_placed, _) = parse_bfd_map(&std::fs::read_to_string(&bfd_map).unwrap());
    let (mold_placed, mold_members) = parse_mold_map(&std::fs::read_to_string(&mold_map).unwrap());

    let mut m = model(target, &built.inputs);
    let static_pie = l.flags.contains(&"-static-pie");
    let r = mortar(&mut m, static_pie, &built.undefined, true);
    let tables = over_inputs(target, &built.inputs, static_pie, &built.undefined);
    if let Err(e) = support::inputs::same(&m, &r, &tables.0, &tables.1) {
        panic!("{}: the Inputs view differs from the model: {e}", l.name);
    }
    let ours: BTreeSet<String> = (0..m.files() as usize)
        .filter(|&f| matches!(r.file_live[f], FileLive::Extracted(_)))
        .map(|f| m.label(FileId(f as u32)))
        .collect();
    let sim = r.gnu.as_ref().unwrap();
    let sim_members: BTreeSet<String> = sim.loaded.iter().map(|&f| m.label(f)).collect();
    let (bc, bbad, bgroup) = definers(&m, &r, &bfd_placed, true);
    let (mc, mbad, _) = definers(&m, &r, &mold_placed, false);
    let errors: Vec<&str> = r
        .problems
        .iter()
        .filter(|p| p.severity() == mortar_resolve::Severity::Error)
        .map(mortar_resolve::Problem::id)
        .collect();
    let warnings: BTreeSet<&str> = r.problems.iter().map(mortar_resolve::Problem::id).collect();
    println!(
        "| {} | {} | {} | {} | {} | {} | {} | {} | {}/{} | {}/{} | {}/{} | {:?} |",
        l.name,
        bfd_members.len(),
        lld_members.len(),
        mold_members.len(),
        ours.len(),
        if ours == bfd_members { "equal" } else { "DIFF" },
        sim_members.len(),
        if sim_members == bfd_members { "equal" } else { "DIFF" },
        bbad.len(),
        bc,
        bgroup.1,
        bgroup.0,
        mbad.len(),
        mc,
        warnings,
    );
    let only_ours: Vec<&String> = ours.difference(&bfd_members).collect();
    let only_bfd: Vec<&String> = bfd_members.difference(&ours).collect();
    assert!(
        only_ours.is_empty() && only_bfd.is_empty(),
        "{}: mortar only {only_ours:?}, bfd only {only_bfd:?}",
        l.name
    );
    let sim_only: Vec<&String> = sim_members.symmetric_difference(&bfd_members).collect();
    assert!(sim_only.is_empty(), "{}: simulation differs from bfd: {sim_only:?}", l.name);
    assert!(errors.is_empty(), "{}: {:?}", l.name, r.problems);
    // The line's own archives are extracted (the Fortran BLOCK DATA by
    // its COMMON block alone, §4.5).
    for (lib, _) in &l.archives {
        assert!(
            ours.iter().any(|f| f.contains(&format!("/{lib}("))),
            "{}: nothing from {lib}",
            l.name
        );
    }
    assert!(bbad.is_empty(), "{}: definers differ from ld.bfd: {bbad:#?}", l.name);
    assert!(mbad.is_empty(), "{}: definers differ from mold: {mbad:#?}", l.name);
    assert!(bc > 100 && mc > 100, "{}: too few definers compared ({bc}, {mc})", l.name);
    let _ = Def::Undefined { weak: false };
}

macro_rules! lines {
    ($($test:ident: $name:literal $target:ident;)*) => {
        $(
            #[test]
            fn $test() {
                check_line($name, $target);
            }
        )*
    };
}

lines! {
    s4_3_system_lines_match_gnu_chello: "chello" A64;
    s4_3_system_lines_match_gnu_spie: "spie" A64;
    s4_3_system_lines_match_gnu_mth: "mth" A64;
    s4_3_system_lines_match_gnu_cxx: "cxx" A64;
    s4_3_system_lines_match_gnu_fort: "fort" A64;
    s4_3_system_lines_match_gnu_tpl: "tpl" A64;
    s4_3_system_lines_match_gnu_chello64: "chello" X64;
    s4_3_system_lines_match_gnu_spie64: "spie" X64;
    s4_3_system_lines_match_gnu_mth64: "mth" X64;
    s4_3_system_lines_match_gnu_cxx64: "cxx" X64;
    s4_3_system_lines_match_gnu_fort64: "fort" X64;
    s4_3_system_lines_match_gnu_tpl64: "tpl" X64;
    s4_3_system_lines_match_gnu_chello32: "chello" X86;
    s4_3_system_lines_match_gnu_spie32: "spie" X86;
    s4_3_system_lines_match_gnu_mth32: "mth" X86;
    s4_3_system_lines_match_gnu_cxx32: "cxx" X86;
    s4_3_system_lines_match_gnu_tpl32: "tpl" X86;
}

/// §15.1 `s12_same_tables_under_seeds_and_threads` over the system
/// archives: the C++ line on each target, every thread count of
/// `TEST_THREADS` with ten seeds, equal to one thread; the closure equals
/// the sequential worklist and the simulation the full visit.
fn same_tables(target: Target) {
    let l = line("cxx", target);
    let Some(built) = build(&l) else { return };
    let mut m = model(target, &built.inputs);
    let start = m.key("_start");
    let opts = Options {
        entry: Entry::Default(OptId::some(start)),
        warn_backrefs: true,
        warn_common: true,
        ..Options::default()
    };
    let reference = resolve(&m, &opts, &Pool::sequential());
    for threads in mortar_core::par::TEST_THREADS {
        for i in 0..10u64 {
            let pool = Pool::sequential()
                .with_threads(threads)
                .with_seed(Some(0x9e37_79b9_7f4a_7c15_u64.wrapping_mul(i + 1)));
            assert!(
                resolve(&m, &opts, &pool) == reference,
                "{}: {threads} threads, seed {i}",
                l.name
            );
        }
    }
    let roots: Vec<(mortar_core::SymbolId, u32)> =
        opts.roots().iter().map(|r| (r.key, r.arg)).collect();
    assert_eq!(
        mortar_resolve::live::oracle(&m, &reference.targets, &reference.redirect, &roots),
        reference.file_live
    );
    assert_eq!(mortar_resolve::gnu::naive(&m, &opts, &reference), *reference.gnu.as_ref().unwrap());
}

#[test]
fn s12_same_tables_under_seeds_and_threads_system() {
    same_tables(A64);
    same_tables(X64);
    same_tables(X86);
}

/// §4.6 archive-choice, ruling 1: the same archive named twice is one
/// file, not two candidates. `libu.a` moved after `libgcc.a` needs an
/// atomic helper from it (a backreference); naming `libgcc.a` again after
/// `libu.a` gives GNU ld a later definer in the same member, and no
/// archive-choice. A copy of `libgcc.a` under another path is a second
/// file, and the warning fires.
#[test]
fn s4_6_libgcc_twice_is_no_choice() {
    let l = line("twice", A64);
    let Some(mut built) = build(&l) else { return };
    let at = built
        .inputs
        .iter()
        .position(|i| matches!(i, Input::Archive { path, .. } if path.ends_with("/libu.a")))
        .unwrap();
    let libu = built.inputs.remove(at);
    let libgcc = built
        .inputs
        .iter()
        .find(|i| matches!(i, Input::Archive { path, .. } if path.ends_with("/libgcc.a")))
        .unwrap()
        .clone();
    built.inputs.push(libu);
    let choices = |inputs: &[Input]| -> (usize, usize) {
        let mut m = model(A64, inputs);
        let a = mortar(&mut m, false, &built.undefined, true);
        let (_, b) = over_inputs(A64, inputs, false, &built.undefined);
        let count = |r: &Resolved| r.problems.iter().filter(|p| p.id() == "archive-choice").count();
        let backrefs = b.problems.iter().filter(|p| p.id() == "archive-backref").count();
        assert_eq!(count(&a), count(&b));
        (count(&b), backrefs)
    };
    // Without a second mention, the backreference is reported.
    let (choice, backref) = choices(&built.inputs);
    assert_eq!(choice, 0);
    assert!(backref > 0, "the line has no backreference to libgcc.a");
    let mut twice = built.inputs.clone();
    twice.push(libgcc.clone());
    assert_eq!(choices(&twice).0, 0, "libgcc.a twice");
    let Input::Archive { path, whole, group } = libgcc else { unreachable!() };
    let copy = built.dir.join("libgcc-copy.a");
    std::fs::copy(&path, &copy).unwrap();
    let mut other = built.inputs.clone();
    other.push(Input::Archive { path: copy.display().to_string(), whole, group });
    assert!(choices(&other).0 > 0, "a copy of libgcc.a is another file");
}

/// §1.1, §4.2, §11: the whole phase from a `Plan` (`resolve_inputs`) on
/// the C line. The table passes its checks (no `internal-error`), there
/// is no diagnostic, and `-e main` picks `main`. With the program object
/// named twice, `main` is a `duplicate-symbol` error.
#[test]
fn s1_1_resolve_inputs_from_a_plan() {
    use mortar_core::plan::{OutputKind, Plan};
    use support::inputs::Item;
    let l = line("chello", A64);
    let Some(built) = build(&l) else { return };
    let items = |inputs: &[Input]| -> Vec<Item> {
        inputs
            .iter()
            .map(|i| match i {
                Input::Object(p) => Item::Object(PathBuf::from(p)),
                Input::Archive { path, whole, group } => {
                    Item::Archive { path: PathBuf::from(path), whole: *whole, group: *group }
                }
            })
            .collect()
    };
    let pool = Pool::new(4);
    let plan =
        Plan { kind: OutputKind::StaticExecutable, entry: Some("main".into()), ..Plan::default() };
    let inputs = support::inputs::build(A64.machine, &items(&built.inputs)).unwrap();
    let out = mortar_resolve::resolve_inputs(&inputs, &plan, &pool);
    assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
    let main = inputs.symbols.iter().find(|(_, k)| k.name == b"main").unwrap().0;
    assert_eq!(out.entry, mortar_resolve::EntryPoint::Symbol(main));

    let mut twice = built.inputs.clone();
    let obj = twice.iter().find(|i| matches!(i, Input::Object(p) if p.ends_with("h.c.o"))).unwrap();
    twice.insert(0, obj.clone());
    let inputs = support::inputs::build(A64.machine, &items(&twice)).unwrap();
    let out = mortar_resolve::resolve_inputs(&inputs, &plan, &pool);
    let ids: Vec<&str> = out.diagnostics.iter().map(|(_, d)| d.id()).collect();
    assert_eq!(ids, ["duplicate-symbol"], "{:?}", out.diagnostics);
    assert_eq!(out.diagnostics[0].1.severity(), mortar_diag::Severity::Error);
}

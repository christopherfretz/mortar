//! WP-262: a full link's capture writes `SYMTAB_KEYS` (`spec/incr.md`
//! IN-18e, IN-49a): per `SYMBOLS` row, its key's index in the output's
//! `.symtab` and its name's `.strtab` offset, or `NONE` for a key with no
//! global entry.
//!
//! `stored_keys_name_the_output_globals`: on a static C++ link (the
//! `support` program: libstdc++, libm, libc), `-g`, `--gc-sections`, in
//! incremental mode, with `x3.cpp`'s hidden global `wp262_hidden` (a
//! local entry, S-77) and its reference to the linker-defined `_end`, on
//! AArch64 and x86-64: the OUTPUT's `.symtab` and `.strtab`, read with
//! `mortar_elf`, against every stored row.
//! - A global entry with the row's name exists exactly when the row is not
//!   `NONE` (a name two keys share, `foo@V1` and `foo@V2`, counts for
//!   both only through the rows that name its entries);
//! - its index is that entry's, and `strtab` the entry's `st_name`;
//! - every entry of the global area `[sh_info, entries)` is named by one
//!   row; a local row (and `wp262_hidden`'s) is `NONE`.
//!
//! `version_7_state_is_refused`: the state's header with version 7 (WP-261's
//! format, which has no `SYMTAB_KEYS`) is refused `state-version`, by the
//! reader and end to end.

mod support;

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use mortar::incremental::State5;
use mortar_incr::update::rows::sym_kind;
use mortar_incr::update::testing::MemoryState;
use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, tools_required};
use support::{Source, build_by, with_output};

/// `.symtab`'s `NONE` row.
const NONE: u32 = u32::MAX;
const STB_LOCAL: u8 = 0;

/// The `support` program's two units and a third: a hidden global, a
/// reference to `_end`, both kept by a constructor under `--gc-sections`.
fn sources() -> Vec<Source> {
    let mut v = support::CXX_PROGRAM.to_vec();
    v.push(Source {
        name: "x3.cpp",
        text: r#"extern "C" {
__attribute__((visibility("hidden"))) int wp262_hidden = 7;
extern char _end[];
long wp262_sink;
int wp262_v1() { return 1; }
int wp262_v2() { return 2; }
}
__asm__(".symver wp262_v1, wp262_ver@V1");
__asm__(".symver wp262_v2, wp262_ver@@V2");
__attribute__((constructor)) static void wp262_init() {
    wp262_sink = wp262_hidden + reinterpret_cast<long>(_end) + wp262_v1() + wp262_v2();
}
"#,
    });
    v
}

struct Line {
    _scratch: Scratch,
    label: &'static str,
    dir: PathBuf,
    args: Vec<String>,
}

fn lines() -> Vec<Line> {
    let mut v = Vec::new();
    let srcs = sources();
    for (label, arch) in [("aarch64", Arch::Aarch64), ("x86-64", Arch::X86_64)] {
        let cc = arch.gnu("g++");
        let s = Scratch::new(&format!("wp262-{label}"));
        let Some(b) = build_by(&s, &cc, &["-O1", "-g"], &srcs) else {
            assert!(!tools_required(), "{cc} is required");
            continue;
        };
        let mut args = b.args;
        args.push("--gc-sections".into());
        v.push(Line { _scratch: s, label, dir: b.dir, args });
    }
    v
}

fn incremental_args(line: &Line, mode: &str) -> Vec<OsString> {
    let mut args = with_output(&line.args, &line.dir.join("out"));
    args.extend([mode, "--incremental-state", "out.mortar-state"].map(OsString::from));
    args
}

/// The link's format-5 state, decoded whole (a test's read), and its bytes.
fn state(line: &Line) -> (MemoryState, Vec<u8>) {
    let path = line.dir.join("out.state5");
    let args = incremental_args(line, "--incremental=full");
    let inv = mortar::invoke_state5(
        "mortar",
        &args,
        &line.dir,
        State5 { path: path.clone(), collide: false, rows: None },
    );
    let ok = matches!(inv.outcome, mortar::Outcome::Linked { .. });
    assert!(ok, "{}: the link failed: {:?}\n{}", line.label, inv.outcome, inv.incremental);
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|_| panic!("{}: no state: {}", line.label, inv.incremental));
    (mortar_incr::state5::read::decode(&bytes).expect("the state decodes"), bytes)
}

fn text(m: &MemoryState, s: mortar_incr::format::Str) -> &[u8] {
    m.strings.get(s.off as usize..(s.off + s.len) as usize).unwrap_or_default()
}

/// The output's `.symtab`: `sh_info`, the entry count, and per global
/// entry `(index, name, st_name)`; the local entries' names.
type Globals = (u32, u32, Vec<(u32, Vec<u8>, u32)>, Vec<Vec<u8>>);

fn output_globals(label: &str, out: &Path) -> Globals {
    let bytes = std::fs::read(out).expect("the output");
    let h = mortar_elf::FileHeader::parse(&bytes).expect("an ELF header");
    let t = mortar_elf::section::SectionTable::parse(&bytes, &h).expect("section headers");
    let syms = t
        .find_symbol_table(mortar_elf::consts::SHT_SYMTAB)
        .expect("a symbol table")
        .expect("a .symtab");
    let first = syms.first_global().0;
    let (mut globals, mut locals) = (Vec::new(), Vec::new());
    for (i, s) in syms.iter() {
        let name = syms.name(&s).expect("a name in .strtab").to_vec();
        if i.0 < first {
            locals.push(name);
        } else {
            assert_ne!(s.binding(), STB_LOCAL, "{label}: a local entry {i:?} past sh_info");
            globals.push((i.0, name, s.st_name));
        }
    }
    (first, syms.len(), globals, locals)
}

/// A `.symtab` name before its version (`foo@V1`, `foo@@V2`: `foo`).
fn base(name: &[u8]) -> &[u8] {
    name.iter().position(|&c| c == b'@').map_or(name, |i| &name[..i])
}

/// What [`judge`] counted.
struct Judged {
    sh_info: u32,
    entries: u32,
    globals: usize,
    stored: usize,
    none_global: usize,
    local_rows: usize,
    /// The global entries' names by index.
    names: HashMap<u32, Vec<u8>>,
    locals: Vec<Vec<u8>>,
}

impl Judged {
    fn line(&self, rows: usize) -> String {
        format!(
            ".symtab {} entries, sh_info {}, {} globals; SYMBOLS {rows} rows: {} with an entry, \
             {} global rows NONE, {} local rows",
            self.entries,
            self.sh_info,
            self.globals,
            self.stored,
            self.none_global,
            self.local_rows
        )
    }
}

/// The oracle: `m`'s `SYMTAB_KEYS` against `out`'s `.symtab` and `.strtab`.
fn judge(label: &str, m: &MemoryState, out: &Path) -> Judged {
    let (sh_info, entries, globals, locals) = output_globals(label, out);
    assert_eq!(m.symtab_keys.len(), m.symbols.len(), "{label}: one SYMTAB_KEYS row per key");
    // Per global entry: its name and st_name, and the row naming it.
    let mut by_index: HashMap<u32, (&[u8], u32)> = HashMap::new();
    let mut by_name: HashMap<&[u8], Vec<u32>> = HashMap::new();
    for (i, name, st_name) in &globals {
        by_index.insert(*i, (name.as_slice(), *st_name));
        by_name.entry(base(name)).or_default().push(*i);
    }
    let mut row_at: HashMap<u32, usize> = HashMap::new();
    let (mut stored, mut none_global, mut local_rows) = (0, 0, 0);
    for (r, (sym, k)) in m.symbols.iter().zip(&m.symtab_keys).enumerate() {
        let name = text(m, sym.name);
        let shown = String::from_utf8_lossy(name);
        let global = matches!(sym.kind, sym_kind::GLOBAL | sym_kind::VERSIONED);
        if !global {
            local_rows += 1;
            assert_eq!(k.index, NONE, "{label}: row {r} {shown} (kind {}) has an entry", sym.kind);
            continue;
        }
        if k.index == NONE {
            none_global += 1;
            continue;
        }
        stored += 1;
        let Some(&(ename, st_name)) = by_index.get(&k.index) else {
            panic!(
                "{label}: row {r} {shown}: index {} is no global entry [{sh_info}, {entries})",
                k.index
            )
        };
        // S-75 (WP-150): a definition its object writes with a version
        // keeps that spelling: `foo@V1` for the key `foo@V1`, `foo@@V2`
        // for the key `foo` of a default version.
        let version = text(m, sym.version);
        let spelled = if version.is_empty() {
            ename == name || (base(ename) == name && ename[name.len()..].starts_with(b"@@"))
        } else {
            ename == [name, &b"@"[..], version].concat()
        };
        assert!(
            spelled,
            "{label}: row {r} {shown}: entry {} is {}",
            k.index,
            String::from_utf8_lossy(ename)
        );
        assert_eq!(k.strtab, st_name, "{label}: row {r} {shown}: strtab against st_name");
        if let Some(other) = row_at.insert(k.index, r) {
            panic!("{label}: rows {other} and {r} name entry {}", k.index);
        }
    }
    // Every global entry is named; a NONE global row's name has no
    // entry left unnamed (`=>` of the equivalence).
    for (i, name, _) in &globals {
        assert!(
            row_at.contains_key(i),
            "{label}: global entry {i} {} is named by no row",
            String::from_utf8_lossy(name)
        );
    }
    for (r, (sym, k)) in m.symbols.iter().zip(&m.symtab_keys).enumerate() {
        if matches!(sym.kind, sym_kind::GLOBAL | sym_kind::VERSIONED) && k.index == NONE {
            let name = text(m, sym.name);
            if let Some(e) = by_name.get(name) {
                assert!(
                    e.iter().all(|i| row_at.get(i).is_some_and(|&o| o != r)),
                    "{label}: row {r} {} is NONE and its name has an entry",
                    String::from_utf8_lossy(name)
                );
            }
        }
    }
    assert_eq!(stored, globals.len(), "{label}: rows with an entry against global entries");
    let names = globals.iter().map(|(i, n, _)| (*i, n.clone())).collect();
    Judged {
        sh_info,
        entries,
        globals: globals.len(),
        stored,
        none_global,
        local_rows,
        names,
        locals,
    }
}

#[test]
fn stored_keys_name_the_output_globals() {
    let mut ran = 0;
    for line in lines() {
        let label = line.label;
        let (m, _) = state(&line);
        let j = judge(label, &m, &line.dir.join("out"));
        // The two keys of x3.cpp.
        let row_of = |n: &[u8]| {
            m.symbols
                .iter()
                .position(|s| s.kind != sym_kind::LOCAL && text(&m, s.name) == n)
                .unwrap_or_else(|| panic!("{label}: no SYMBOLS row {}", String::from_utf8_lossy(n)))
        };
        let hidden = row_of(b"wp262_hidden");
        assert_eq!(m.symtab_keys[hidden].index, NONE, "{label}: the hidden global's row");
        assert!(j.locals.iter().any(|n| n == b"wp262_hidden"), "{label}: wp262_hidden is a local");
        let end = row_of(b"_end");
        let e = m.symtab_keys[end];
        assert!(j.names.get(&e.index).is_some_and(|x| x == b"_end"), "{label}: _end's row {e:?}");
        // `.symver`: the key `wp262_ver@V1` and the default `wp262_ver`
        // (`wp262_ver@@V2`), each with its entry.
        let ver: Vec<(Vec<u8>, u32)> = m
            .symbols
            .iter()
            .zip(&m.symtab_keys)
            .filter(|(s, _)| text(&m, s.name) == b"wp262_ver")
            .map(|(s, k)| (text(&m, s.version).to_vec(), k.index))
            .collect();
        let spelled: Vec<&[u8]> =
            ver.iter().map(|(_, i)| j.names.get(i).map_or(&b"NONE"[..], Vec::as_slice)).collect();
        assert_eq!(ver.len(), 2, "{label}: wp262_ver's rows {ver:?}");
        assert!(
            spelled.contains(&&b"wp262_ver@V1"[..]) && spelled.contains(&&b"wp262_ver@@V2"[..]),
            "{label}: wp262_ver's entries {spelled:?}"
        );
        let sink = m.symtab_keys[row_of(b"wp262_sink")];
        assert_ne!(sink.index, NONE, "{label}: wp262_sink's row");
        eprintln!(
            "[wp262] {label}: {}; _end at {}, wp262_hidden NONE",
            j.line(m.symbols.len()),
            e.index
        );
        ran += 1;
    }
    assert!(ran == 2 || !tools_required(), "{ran} of 2 links");
}

#[test]
fn version_7_state_is_refused() {
    let mut ran = 0;
    for line in lines() {
        let label = line.label;
        let (_, mut bytes) = state(&line);
        assert_eq!(&bytes[..8], b"MORTARSR", "{label}: format 5's magic");
        assert_eq!(
            bytes[8..12],
            mortar_incr::state5::VERSION.to_le_bytes(),
            "{label}: the state's version"
        );
        bytes[8..12].copy_from_slice(&7u32.to_le_bytes());
        let e = mortar_incr::state5::read::decode(&bytes).expect_err("a version-7 state decodes");
        let shown = format!("{e:?}");
        assert!(
            shown.contains("Version") && shown.contains("format version 7"),
            "{label}: {shown}"
        );
        // End to end: the link's own state at version 7, then an update.
        let at = line.dir.join("out.mortar-state");
        let mut disk = std::fs::read(&at).expect("the state");
        assert_eq!(&disk[..8], b"MORTARSR", "{label}: the written state's magic");
        disk[8..12].copy_from_slice(&7u32.to_le_bytes());
        std::fs::write(&at, disk).expect("the state");
        let args = incremental_args(&line, "--incremental=update");
        let second = mortar::invoke_in("mortar", &args, &line.dir);
        let report = format!("{:?}\n{}", second.outcome, second.incremental);
        assert!(report.contains("state-version"), "{label}: {report}");
        eprintln!("[wp262] {label}: a version-7 state: state-version");
        ran += 1;
    }
    assert!(ran == 2 || !tools_required(), "{ran} of 2 links");
}

// WP-262 task 5 (a check, not a gate): an update's state against its
// output. With `MORTAR_WP262_W=<dir>` (an `incr-update` run kept with
// `--keep`: its `w` directory, after a hop that was an update in place),
// `prog.mortar-state` against `prog`.
#[test]
#[ignore = "reads a kept incr-update run named by MORTAR_WP262_W"]
fn kept_update_state_names_its_output_globals() {
    let w = PathBuf::from(std::env::var_os("MORTAR_WP262_W").expect("MORTAR_WP262_W"));
    let bytes = std::fs::read(w.join("prog.mortar-state")).expect("the state");
    let m = mortar_incr::state5::read::decode(&bytes).expect("the state decodes");
    let j = judge("update", &m, &w.join("prog"));
    eprintln!("[wp262] {}: {}", w.display(), j.line(m.symbols.len()));
}

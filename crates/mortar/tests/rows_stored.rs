//! WP-255c (`stored_rows_value_every_global`): for EVERY global key of a
//! link, three ways equal: `rows::value` over the full link's
//! `FrontRows`, `rows::value` over the format-5 state that link captured
//! (`StoredRows`, `crates/mortar/src/update/stored_rows.rs`), and the
//! output's `.symtab` value. A key the stored source cannot value must be
//! one of the record's named fall-backs (`spec/landed/incr/WP-255c.md`:
//! a key defined in a `SHF_MERGE` section without a place, §9.3, or in an
//! input `.eh_frame`), and then the stored side answers
//! `update-not-implemented (image)`.
//!
//! The fixtures (AArch64 and x86-64): U-6's three assembly files; a C
//! link with TLS, an IFUNC and a merged string named by a global and
//! referenced through a global pointer; a C++ link whose inline function
//! is a COMDAT group in two files (one copy dropped, I-47a's case); a
//! kiln/hod link when `MASON_KILN` and `MASON_HOD` are set.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::process::Command;
use std::time::Duration;

use mortar::front::{Env, front};
use mortar::timing::Timer;
use mortar::update::stored_rows::StoredRows;
use mortar_arch::SlotKind;
use mortar_core::FileId;
use mortar_core::demands::GlobalFlags;
use mortar_core::incr::{IncrReason, StoredLayout, Unimplemented};
use mortar_core::par::Pool;
use mortar_core::resolution::{Def, InternalDef, SectionKind};
use mortar_core::{SymKey, SymbolId};
use mortar_diag::{Collector, Policy};
use mortar_image::rows::{self, FrontRows, Rows};
use mortar_incr::format::Str;
use mortar_incr::format::def_kind::SECTION;
use mortar_incr::update::rows::MentionRow;
use mortar_incr::update::testing::MemoryState;
use mortar_incr::update::view::StateView;
use mortar_input::store::{Store, StorePolicy};
use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, run_with, tools_required};

const ARCHES: [Arch; 2] = [Arch::Aarch64, Arch::X86_64];

/// How a source is compiled.
#[derive(Clone, Copy)]
enum Lang {
    Asm,
    C,
    Cxx,
    Kiln,
}

/// A fixture's sources: name, language, text.
type Files = Vec<(&'static str, Lang, String)>;

/// U-6's fixture (`tests/update_image.rs`): `_start` with a GOT load and
/// a call, data and merged strings, a function with CFI.
fn u6(arch: Arch) -> Files {
    let (got, call, ty) = match arch {
        Arch::X86_64 => ("\tmovq d@GOTPCREL(%rip), %rax\n\tmovq (%rax), %rdi\n", "\tcall f\n", "@"),
        _ => ("\tadrp x1, :got:d\n\tldr x1, [x1, :got_lo12:d]\n\tldr x0, [x1]\n", "\tbl f\n", "%"),
    };
    let ret = "\tret\n";
    vec![
        (
            "a",
            Lang::Asm,
            format!(
                "\t.text\n\t.globl _start\n\t.type _start, {ty}function\n_start:\n\t.cfi_startproc\n\
                 {got}{call}{ret}\t.cfi_endproc\n\t.section .note.GNU-stack,\"\",{ty}progbits\n"
            ),
        ),
        (
            "b",
            Lang::Asm,
            format!(
                "\t.data\n\t.globl d\nd:\t.quad 7\n\
                 \t.section .rodata.str1.1,\"aMS\",{ty}progbits,1\n\t.globl gstr\ngstr:\t.asciz \"hello\"\n\
                 \t.asciz \"world\"\n\
                 \t.section .rodata.cst8,\"aM\",{ty}progbits,8\n\t.p2align 3\n\t.quad 0x1122334455667788\n\
                 \t.section .note.GNU-stack,\"\",{ty}progbits\n"
            ),
        ),
        (
            "c",
            Lang::Asm,
            format!(
                "\t.text\n\t.globl f\n\t.type f, {ty}function\nf:\n\t.cfi_startproc\n{ret}\
                 \t.cfi_endproc\n\t.section .note.GNU-stack,\"\",{ty}progbits\n"
            ),
        ),
    ]
}

/// The C fixture: TLS, an IFUNC, a merged string named by a global (in
/// top-level assembly: C gives a string literal no global name) and
/// referenced through a global pointer.
fn c_fixture(arch: Arch) -> Files {
    let ty = if arch == Arch::X86_64 { "@" } else { "%" };
    vec![
        (
            "main",
            Lang::C,
            "extern __thread int tv;\nextern const char gmsg[];\nextern const char *gptr;\n\
             int pick(void);\nint g_counter = 5;\n\
             void _start(void) { g_counter += tv + pick() + gptr[0] + gmsg[1]; for (;;) {} }\n"
                .to_owned(),
        ),
        (
            "lib",
            Lang::C,
            format!(
                "__thread int tv = 3;\n__thread int tz;\n\
                 static int pick_a(void) {{ return 1; }}\n\
                 static int (*resolve_pick(void))(void) {{ return pick_a; }}\n\
                 int pick(void) __attribute__((ifunc(\"resolve_pick\")));\n\
                 __asm__(\".section .rodata.str1.1,\\\"aMS\\\",{ty}progbits,1\\n\
                 .globl gmsg\\ngmsg: .asciz \\\"merged\\\"\\n.text\\n\");\n\
                 const char *gptr = \"literal\";\nlong g_bss;\n"
            ),
        ),
    ]
}

/// The C++ fixture: `inline` `h` and a template's instance are COMDAT
/// groups in both files; the second copy is dropped.
fn cxx_fixture() -> Files {
    let common = "inline int h(int x) { return x * 3; }\n\
                  template <class T> struct S { static int v; static int get() { return v + 1; } };\n\
                  template <class T> int S<T>::v = 4;\n";
    vec![
        (
            "one",
            Lang::Cxx,
            format!(
                "{common}int two();\nextern \"C\" void _start() {{ volatile int r = h(2) + S<int>::get() \
                 + two(); (void)r; for (;;) {{}} }}\n"
            ),
        ),
        ("two", Lang::Cxx, format!("{common}int two() {{ return h(5) + S<int>::get(); }}\n")),
    ]
}

/// The kiln/hod fixture, when both are given.
fn kiln_fixture() -> Files {
    vec![
        (
            "k1",
            Lang::Kiln,
            "inline int h(int x) { return x * 3; }\nint gk = 9;\nint two();\n\
             extern \"C\" void _start() { volatile int r = h(gk) + two(); (void)r; for (;;) {} }\n"
                .to_owned(),
        ),
        (
            "k2",
            Lang::Kiln,
            "inline int h(int x) { return x * 3; }\nint two() { return h(5); }\n".to_owned(),
        ),
    ]
}

fn compile(arch: Arch, dir: &Scratch, name: &str, lang: Lang, text: &str) -> Option<String> {
    let (ext, mut cmd) = match lang {
        Lang::Asm => ("s", Command::new(arch.tool("gcc")?)),
        Lang::C => ("c", Command::new(arch.tool("gcc")?)),
        Lang::Cxx => ("cpp", Command::new(arch.tool("g++")?)),
        Lang::Kiln => {
            let kiln = std::env::var_os("MASON_KILN")?;
            let hod = std::env::var_os("MASON_HOD")?;
            let mut c = Command::new(kiln);
            c.env("KILN_HOD", hod)
                .arg("-fkiln-backend=hod")
                .arg(format!("--target={}", arch.triple()));
            ("cpp", c)
        }
    };
    let src = dir.write(&format!("{name}.{ext}"), text);
    let obj = format!("{name}.o");
    if !matches!(lang, Lang::Kiln) {
        // -O0: `h` and `S<int>::get` stay out-of-line COMDAT copies.
        let opt = if matches!(lang, Lang::Cxx) { "-O0" } else { "-O1" };
        cmd.args([opt, "-fno-asynchronous-unwind-tables", "-fno-exceptions"]);
    }
    if matches!(lang, Lang::Cxx) {
        cmd.arg("-fno-rtti");
    }
    cmd.args([
        "-g".as_ref(),
        "-c".as_ref(),
        src.as_os_str(),
        "-o".as_ref(),
        dir.join(&obj).as_os_str(),
    ]);
    let o = run_with(&mut cmd, b"", Duration::from_mins(2));
    assert!(o.success(), "{name}: {}", o.stderr_text());
    Some(obj)
}

/// `.symtab`'s global and weak definitions of the output: name to
/// `(st_value, st_type)`.
fn symtab(bytes: &[u8]) -> HashMap<Vec<u8>, (u64, u8)> {
    // ELF64 little-endian (both targets): read by hand, the verifier's
    // reader is not the linker's to use.
    let u16_at = |o: usize| u64::from(u16::from_le_bytes(bytes[o..o + 2].try_into().unwrap()));
    let u32_at = |o: usize| u64::from(u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap()));
    let u64_at = |o: usize| u64::from_le_bytes(bytes[o..o + 8].try_into().unwrap());
    let (shoff, shnum) = (u64_at(0x28) as usize, u16_at(0x3c) as usize);
    let sh = |i: usize| shoff + i * 64;
    let symtab = (0..shnum).find(|&i| u32_at(sh(i) + 4) == 2).expect("a .symtab");
    let (off, size) = (u64_at(sh(symtab) + 24) as usize, u64_at(sh(symtab) + 32) as usize);
    let strtab = u64_at(sh(u32_at(sh(symtab) + 40) as usize) + 24) as usize;
    let mut out = HashMap::new();
    for e in (off..off + size).step_by(24) {
        let info = bytes[e + 4];
        let bind = info >> 4;
        if (bind == 1 || bind == 2) && u16_at(e + 6) != 0 {
            let n = strtab + u32_at(e) as usize;
            let end = n + bytes[n..].iter().position(|&b| b == 0).unwrap();
            out.insert(bytes[n..end].to_vec(), (u64_at(e + 8), info & 0xf));
        }
    }
    out
}

/// What one fixture compared.
#[derive(Debug, Default)]
struct Seen {
    keys: u32,
    equal: u32,
    symtab: u32,
    fallbacks: u32,
    /// Slot and PLT answers compared (every key, every slot kind).
    slots: u32,
    /// The scale run (not strict): the fall-backs by reason and the
    /// definer's section kind.
    reasons: BTreeMap<String, u32>,
    /// The scale run: keys whose `SYMBOLS` row is not their `SymbolId`.
    renumbered: u32,
    /// The scale run: `StoredRows::new`, and `value_of` over every key,
    /// microseconds, each run.
    new_us: Vec<u128>,
    value_us: Vec<u128>,
}

/// How [`compare`] runs.
#[derive(Clone, Copy, Default)]
struct Opts {
    /// A fall-back that is not a named one is tallied, not an error.
    tally: bool,
    /// Time `StoredRows::new` and `value_of` over every key (5 runs).
    timing: bool,
}

/// One fixture's incremental-mode full link: its directory, arguments,
/// output and format-5 state.
struct Linked {
    label: String,
    dir: Scratch,
    args: Vec<OsString>,
    out: Vec<u8>,
    state: Vec<u8>,
}

/// The incremental-mode full link of `files`; `None` when a tool is
/// missing.
fn link(arch: Arch, label: &str, files: &Files) -> Option<Linked> {
    link_with(arch, label, files, &[])
}

/// [`link`] with `extra` options (WP-257a: `--gc-sections`).
fn link_with(arch: Arch, label: &str, files: &Files, extra: &[&str]) -> Option<Linked> {
    let dir = Scratch::new(&format!("wp255c-{label}-{arch:?}"));
    let mut objs = Vec::new();
    for (name, lang, text) in files {
        objs.push(compile(arch, &dir, name, *lang, text)?);
    }
    let mut args: Vec<OsString> = objs.iter().map(OsString::from).collect();
    args.extend(["-static", "-e", "_start", "-o", "out"].iter().map(OsString::from));
    args.extend(extra.iter().map(OsString::from));
    let mut inc = args.clone();
    inc.push("--incremental".into());
    let inv = mortar::invoke_in("mortar", &inc, dir.path());
    assert!(matches!(inv.outcome, mortar::Outcome::Linked { .. }), "{label}: {:?}", inv.outcome);
    let out = std::fs::read(dir.join("out")).unwrap();
    std::fs::remove_file(dir.join("out")).unwrap();
    let state = std::fs::read(dir.join("out.mortar-state")).unwrap();
    assert_eq!(state[..8], mortar_incr::state5::MAGIC, "{label}: a format-5 state");
    Some(Linked { label: format!("{label} {arch:?}"), dir, args, out, state })
}

/// The slot kinds compared per key (`TlsLd` is the output's, compared once).
const SLOT_KINDS: [SlotKind; 5] = [
    SlotKind::Addr,
    SlotKind::TpOff { negated: false },
    SlotKind::TlsGd,
    SlotKind::TlsDesc,
    SlotKind::PltGot,
];

/// A look at the stored rows right after they open (the failure test).
type Probe<'p> = &'p dyn Fn(&StoredRows<'_, '_>);

/// The three-way comparison of `linked`'s link against the format-5 state
/// `state` (the link's own, or a mutated copy): `Err` names the first key
/// on which the stored side disagrees, or the open's refusal.
fn compare(
    linked: &Linked,
    state: Vec<u8>,
    probe: Option<Probe<'_>>,
    opts: Opts,
) -> Result<Seen, String> {
    let label = &linked.label;
    let view = mortar_incr::state5::read::open_bytes(state).map_err(|e| format!("open: {e:?}"))?;
    let lo = mortar_incr::state5::read::LayoutOf { view: &view };
    let stored_layout = StoredLayout::new(&lo);
    let table = symtab(&linked.out);
    // The same link in this process (incremental mode: slack, format-5
    // data), for `FrontRows`.
    let parsed = mortar_cli::parse(&linked.args);
    assert!(!parsed.failed());
    let store = Store::new(StorePolicy::from_env());
    let diag = Collector::new(Policy::default());
    let env =
        Env { cwd: linked.dir.path(), pool: Pool::new(2), replay: None, slack: true, state5: true };
    let f = front(parsed.plan, env, &store, &diag, &mut Timer::new(false))
        .unwrap_or_else(|| panic!("front: {:?}", diag.finish()));
    // The `.got` plan's two inputs that are not rows (the record's
    // finding F-1): the scan's output flags with `_GLOBAL_OFFSET_TABLE_`'s
    // `GOT_BASE` (as `front::got_base_defined`), and the PLT variant.
    let got_table = f.resolution.def.iter().any(|(s, d)| {
        matches!(d, Def::Internal(InternalDef::Provide { conditional: false }))
            && f.inputs.symbols.get(s).is_some_and(|k| k.name == b"_GLOBAL_OFFSET_TABLE_")
    });
    let mut got_output = f.scanned.demands.output;
    if got_table {
        got_output |= GlobalFlags::GOT_BASE;
    }
    let variant = f.synth.got.variant();
    mortar::image::with_link(&f, &store, &|_| {}, |l| {
        let front_rows = FrontRows::new(*l);
        let stored = StoredRows::new(&view, &stored_layout, l.arch, got_output, variant)
            .map_err(|e| format!("{label}: open (StoredRows::new): {e:?}"))?;
        let mut seen = Seen::default();
        if opts.timing {
            let n = l.inputs.symbols.len();
            time_rows(
                &|| StoredRows::new(&view, &stored_layout, l.arch, got_output, variant),
                n,
                &mut seen,
            )
            .map_err(|e| format!("{label}: open: {e:?}"))?;
        }
        let tls = front_rows.tls().map_or(0, |t| t.start);
        if let Some(p) = probe {
            p(&stored);
        }
        let any = SymKey::global(SymbolId(0));
        let (fr, st) = (
            rows::slot(&front_rows, any, SlotKind::TlsLd),
            rows::slot(&stored, any, SlotKind::TlsLd),
        );
        if fr != st {
            return Err(format!("{label}: the TLS module slot: front {fr:x?}, stored {st:x?}"));
        }
        let by_name = if opts.tally { globals_by_name(&view)? } else { HashMap::new() };
        for i in 0..l.inputs.symbols.len() {
            let key = SymKey::global(SymbolId(i));
            let name = front_rows.name(key).to_vec();
            let shown = String::from_utf8_lossy(&name).into_owned();
            // The scale run matches keys by name (the state's numbering is
            // not the in-process front's there: see the record).
            let sid = if opts.tally {
                let Some(r) = stored_id(&by_name, &name, i, &mut seen) else { continue };
                r
            } else if stored.name(key) == &name[..] {
                i
            } else {
                let other = String::from_utf8_lossy(stored.name(key)).into_owned();
                return Err(format!("{label}: key {i} ({shown}) names another key: {other}"));
            };
            let skey = SymKey::global(SymbolId(sid));
            seen.keys += 1;
            same_rows(
                &front_rows,
                &stored,
                (SymbolId(i), SymbolId(sid)),
                &format!("{label}: {shown}"),
                &mut seen,
            )?;
            let want = rows::sym(&front_rows, key);
            match stored.value_of(skey) {
                Ok(got) if got == want => seen.equal += 1,
                Ok(got) => {
                    return Err(format!("{label}: {shown}: front {want:x?}, stored {got:x?}"));
                }
                Err(e) => {
                    fallback(&front_rows, key, &e, opts, &mut seen)
                        .map_err(|()| format!("{label}: {shown} has no stored value: {e:?}"))?;
                    continue;
                }
            }
            let Some(&(v, typ)) = table.get(&name) else { continue };
            if !want.live || typ == 10 {
                // An IFUNC's `.symtab` value is its resolver; `S` is its
                // IPLT entry.
                continue;
            }
            // A TLS symbol's `.symtab` value is its offset in the block.
            let s = if typ == 6 { want.s - tls } else { want.s };
            if s != v {
                return Err(format!("{label}: .symtab value of {shown}: {v:#x}, rows {s:#x}"));
            }
            seen.symtab += 1;
        }
        Ok(seen)
    })
    .unwrap()
}

/// A key the stored side could not value: a named fall-back only (a key
/// defined in a merge section, §9.3, or an input `.eh_frame`), or in the
/// scale run any reason, tallied.
fn fallback(
    front: &FrontRows<'_, '_>,
    key: SymKey,
    e: &IncrReason,
    opts: Opts,
    seen: &mut Seen,
) -> Result<(), ()> {
    let def_kind = rows::def_entry(front, key)
        .and_then(|d| front.entry(d))
        .and_then(|d| d.place?.ok())
        .and_then(|s| front.section_kind(s));
    let named = matches!(def_kind, Some(SectionKind::Merge | SectionKind::EhFrame))
        && *e == IncrReason::UpdateNotImplemented(Some(Unimplemented::Image));
    if opts.tally {
        *seen.reasons.entry(format!("{e:?} / {def_kind:?}")).or_default() += 1;
    } else if !named {
        return Err(());
    }
    seen.fallbacks += 1;
    Ok(())
}

/// The state's `GLOBAL` rows by name (`None`: two rows of one name).
fn globals_by_name(view: &StateView<'_>) -> Result<HashMap<Vec<u8>, Option<u32>>, String> {
    let mut by_name = HashMap::new();
    for r in 0..u32::try_from(view.count(mortar_incr::format::kind::SYMBOLS)).unwrap() {
        let row = view.symbol(SymbolId(r)).map_err(|e| format!("SYMBOLS: {e:?}"))?;
        if row.kind == mortar_incr::format::sym_kind::GLOBAL {
            let n = view.string(row.name).map_err(|e| format!("STRINGS: {e:?}"))?;
            by_name.entry(n).and_modify(|v| *v = None).or_insert(Some(r));
        }
    }
    Ok(by_name)
}

/// The scale run matches keys by name (the state's numbering is not the
/// in-process front's there: see the record): front key `i`'s row, or
/// `None` with the reason tallied.
fn stored_id(
    by_name: &HashMap<Vec<u8>, Option<u32>>,
    name: &[u8],
    i: u32,
    seen: &mut Seen,
) -> Option<u32> {
    match by_name.get(name) {
        Some(&Some(r)) => {
            if r != i {
                seen.renumbered += 1;
            }
            Some(r)
        }
        found => {
            let why = if found.is_some() { "two SYMBOLS rows" } else { "no SYMBOLS row" };
            let shown = String::from_utf8_lossy(name);
            *seen.reasons.entry(format!("{why} (e.g. {shown})")).or_default() += 1;
            None
        }
    }
}

/// Beside the value: one key's slot addresses, PLT entry and definition
/// row (its entry included) agree; `at` names the key.
fn same_rows(
    front: &FrontRows<'_, '_>,
    stored: &StoredRows<'_, '_>,
    (fid, sid): (SymbolId, SymbolId),
    at: &str,
    seen: &mut Seen,
) -> Result<(), String> {
    let (fkey, skey) = (SymKey::global(fid), SymKey::global(sid));
    for kind in SLOT_KINDS {
        let (fr, st) = (rows::slot(front, fkey, kind), rows::slot(stored, skey, kind));
        if fr != st {
            return Err(format!("{at}: {kind:?} slot: front {fr:x?}, stored {st:x?}"));
        }
        seen.slots += 1;
    }
    let (fr, st) = (rows::plt(front, fkey), rows::plt(stored, skey));
    if fr != st {
        return Err(format!("{at}: PLT entry: front {fr:x?}, stored {st:x?}"));
    }
    // The definition row, its entry included (WP-255c3: the value
    // alone does not see a definer's `MENTIONS` rows reordered,
    // the derived index stays consistent inside the state).
    let (fr, st) = (front.def(fid), stored.def(sid));
    if fr != st && !(st.is_none() && stored.failure().is_some()) {
        return Err(format!("{at}: def: front {fr:?}, stored {st:?}"));
    }
    Ok(())
}

/// `StoredRows::new` (by `open`) and `value_of` over the `n` keys, 5 runs
/// each, into `seen`.
fn time_rows<'v, 's>(
    open: &dyn Fn() -> Result<StoredRows<'v, 's>, IncrReason>,
    n: u32,
    seen: &mut Seen,
) -> Result<(), IncrReason> {
    for _ in 0..5 {
        let t = std::time::Instant::now();
        let again = open()?;
        seen.new_us.push(t.elapsed().as_micros());
        let t = std::time::Instant::now();
        for i in 0..n {
            std::hint::black_box(again.value_of(SymKey::global(SymbolId(i))).ok());
        }
        seen.value_us.push(t.elapsed().as_micros());
    }
    Ok(())
}

/// The three-way comparison on one fixture's own state; `None` when a
/// tool is missing.
fn check(arch: Arch, label: &str, files: &Files) -> Option<Seen> {
    let linked = link(arch, label, files)?;
    let seen = compare(&linked, linked.state.clone(), None, Opts::default())
        .unwrap_or_else(|e| panic!("{e}"));
    eprintln!("[wp255c] {}: {seen:?}", linked.label);
    Some(seen)
}

#[test]
fn stored_rows_value_every_global() {
    let mut ran = 0;
    for arch in ARCHES {
        let fixtures: Vec<(&str, Files)> = vec![
            ("u6", u6(arch)),
            ("c-tls-ifunc-merge", c_fixture(arch)),
            ("cxx-comdat", cxx_fixture()),
        ];
        for (label, files) in &fixtures {
            match check(arch, label, files) {
                Some(seen) => {
                    assert!(seen.equal > 0 && seen.symtab > 0, "{label} {arch:?}: {seen:?}");
                    ran += 1;
                }
                None => assert!(!tools_required(), "{label} {arch:?}: a tool is missing"),
            }
        }
        if std::env::var_os("MASON_KILN").is_some() && std::env::var_os("MASON_HOD").is_some() {
            let seen = check(arch, "kiln", &kiln_fixture()).expect("kiln and hod");
            assert!(seen.equal > 0 && seen.symtab > 0, "kiln {arch:?}: {seen:?}");
        }
    }
    eprintln!("[wp255c] {ran} fixtures compared");
}

// ---- WP-255c3: the evidence that a stale stored datum is SEEN ----------------

/// The mutation fixture: `_start` loads `d` and `e` through the GOT (two
/// `SLOTS` rows of one list), reads `_end` (a `DEFINED` row) and calls
/// `f` (in another file's `.text`, after `_start`'s); `d` and `e` are
/// the two globals of one file (adjacent `MENTIONS` rows).
fn mutation_fixture(arch: Arch) -> Files {
    let (ty, body) = match arch {
        Arch::X86_64 => (
            "@",
            "\tmovq d@GOTPCREL(%rip), %rax\n\tmovq e@GOTPCREL(%rip), %rbx\n\
             \tleaq _end(%rip), %rcx\n\tcall f\n\tret\n",
        ),
        _ => (
            "%",
            "\tadrp x1, :got:d\n\tldr x1, [x1, :got_lo12:d]\n\
             \tadrp x2, :got:e\n\tldr x2, [x2, :got_lo12:e]\n\
             \tadrp x3, _end\n\tadd x3, x3, :lo12:_end\n\tbl f\n\tret\n",
        ),
    };
    let stack = format!("\t.section .note.GNU-stack,\"\",{ty}progbits\n");
    vec![
        (
            "a",
            Lang::Asm,
            format!(
                "\t.text\n\t.globl _start\n\t.type _start, {ty}function\n_start:\n{body}{stack}"
            ),
        ),
        (
            "b",
            Lang::Asm,
            format!("\t.data\n\t.globl d\nd:\t.quad 7\n\t.globl e\ne:\t.quad 8\n{stack}"),
        ),
        (
            "c",
            Lang::Asm,
            format!("\t.text\n\t.globl f\n\t.type f, {ty}function\nf:\n\tnop\n\tret\n{stack}"),
        ),
    ]
}

/// The `SYMBOLS` row named `name`.
fn key_of(m: &MemoryState, name: &[u8]) -> usize {
    let at = |s: Str| &m.strings[s.off as usize..(s.off + s.len) as usize];
    m.symbols.iter().position(|r| at(r.name) == name).unwrap_or_else(|| {
        panic!("no key {}", String::from_utf8_lossy(name));
    })
}

/// On a COPY of each target's captured state (written by the state's own
/// writer into the test's directory), the mutation `mutate`; the
/// three-way comparison over the copy must fail on a named key, or the
/// open must refuse it by name. The unmutated copy compares equal first
/// (the writer's round trip is not the mutation). Returns the failures.
fn sees_stale(x: &str, mutate: fn(&mut MemoryState) -> Result<(), String>) -> Vec<String> {
    let mut seen = Vec::new();
    for arch in ARCHES {
        let Some(linked) = link(arch, &format!("stale-{x}"), &mutation_fixture(arch)) else {
            assert!(!tools_required(), "{x} {arch:?}: a tool is missing");
            continue;
        };
        let mut m = mortar_incr::state5::read::decode(&linked.state).unwrap();
        let control = mortar_incr::state5::write::encode(&m).unwrap();
        let ok = compare(&linked, control, None, Opts::default())
            .unwrap_or_else(|e| panic!("control: {e}"));
        assert!(ok.equal > 0, "{x} {arch:?}: {ok:?}");
        mutate(&mut m).unwrap_or_else(|e| panic!("{x} {arch:?}: {e}"));
        let copy = linked.dir.join("copy.mortar-state");
        let bytes = mortar_incr::state5::write::encode(&m)
            .unwrap_or_else(|e| panic!("{x} {arch:?}: the writer refuses the mutation: {e:?}"));
        std::fs::write(&copy, bytes).unwrap();
        let r = compare(&linked, std::fs::read(&copy).unwrap(), None, Opts::default());
        std::fs::remove_file(&copy).unwrap();
        eprintln!("[wp255c3] stale {x} {arch:?}: {r:?}");
        match r {
            Ok(s) => panic!("{x} {arch:?}: a stale datum NOTHING sees: {s:?}"),
            Err(e) => seen.push(e),
        }
    }
    seen
}

#[test]
fn stored_rows_sees_a_stale_resolved_value() {
    for e in sees_stale("resolved-value", |m| {
        let d = key_of(m, b"d");
        m.resolved[d].value += 8;
        Ok(())
    }) {
        assert!(e.contains(": d: front"), "{e}");
    }
}

#[test]
fn stored_rows_sees_a_stale_section_place() {
    for e in sees_stale("section-place", |m| {
        let s = m.resolved[key_of(m, b"f")].section as usize;
        m.sections[s].place_offset += 0x10;
        Ok(())
    }) {
        assert!(e.contains(": f: front"), "{e}");
    }
}

#[test]
fn stored_rows_sees_a_stale_chunk_addr() {
    for e in sees_stale("chunk-addr", |m| {
        let s = m.resolved[key_of(m, b"d")].section as usize;
        let c = m.sections[s].place_chunk as usize;
        m.chunks[c].addr += 0x10;
        Ok(())
    }) {
        assert!(e.contains(": d: front") || e.contains(": e: front"), "{e}");
    }
}

#[test]
fn stored_rows_sees_a_stale_slot_index() {
    for e in sees_stale("slot-index", |m| {
        // Two rows of one list swapped: each key's index is the other's.
        let (d, e) = (key_of(m, b"d") as u32, key_of(m, b"e") as u32);
        let i = m.slots.iter().position(|r| r.symbol == d).ok_or("no slot of d")?;
        let j = m.slots.iter().position(|r| r.symbol == e).ok_or("no slot of e")?;
        if m.slots[i].kind != m.slots[j].kind {
            return Err("d's and e's slots in two lists".into());
        }
        m.slots.swap(i, j);
        Ok(())
    }) {
        assert!(e.contains("Addr slot"), "{e}");
    }
}

#[test]
fn stored_rows_sees_a_stale_defined_value() {
    for e in sees_stale("defined-value", |m| {
        let k = key_of(m, b"_end") as u32;
        m.defined.iter_mut().find(|r| r.symbol == k).ok_or("no DEFINED row of _end")?.value += 8;
        Ok(())
    }) {
        assert!(e.contains(": _end: front"), "{e}");
    }
}

#[test]
fn stored_rows_sees_a_stale_definer_mention() {
    for e in sees_stale("definer-mention", |m| {
        // The definer's two `MENTIONS` rows (`d`'s, `e`'s) swapped: the
        // index derived from the row's position names the other entry.
        let (d, e) = (key_of(m, b"d") as u32, key_of(m, b"e") as u32);
        let def = |k: u32| move |r: &(FileId, MentionRow)| r.1.symbol == k && r.1.kind == SECTION;
        let i = m.mentions.iter().position(def(d)).ok_or("no definer mention of d")?;
        let j = m.mentions.iter().position(def(e)).ok_or("no definer mention of e")?;
        if m.mentions[i].0 != m.mentions[j].0 {
            return Err("d and e in two files".into());
        }
        m.mentions.swap(i, j);
        Ok(())
    }) {
        assert!(e.contains(": d: def: front") || e.contains(": e: def: front"), "{e}");
    }
}

/// Ruling 2 ("After 255c"): one corrupt row; `value_of` gives the reason,
/// never a value, and `failure()` holds it.
#[test]
fn stored_rows_value_of_gives_the_reason() {
    for arch in ARCHES {
        let Some(linked) = link(arch, "failure", &mutation_fixture(arch)) else {
            assert!(!tools_required(), "{arch:?}: a tool is missing");
            continue;
        };
        let mut m = mortar_incr::state5::read::decode(&linked.state).unwrap();
        let d = key_of(&m, b"d");
        m.resolved[d].kind = 0xee;
        let copy = linked.dir.join("copy.mortar-state");
        std::fs::write(&copy, mortar_incr::state5::write::encode(&m).unwrap()).unwrap();
        let want = IncrReason::StateCorrupt("WP-255c stored rows: RESOLVED: kind".into());
        let probe = |s: &StoredRows<'_, '_>| {
            let key = SymKey::global(SymbolId(d as u32));
            assert_eq!(s.value_of(key), Err(want.clone()), "{arch:?}");
            assert_eq!(s.failure(), Some(want.clone()), "{arch:?}");
            // A sound key after it: its own value, the reason cleared.
            let e = SymKey::global(SymbolId(key_of(&m, b"e") as u32));
            assert!(s.value_of(e).is_ok(), "{arch:?}");
            assert_eq!(s.failure(), None, "{arch:?}");
        };
        let r = compare(&linked, std::fs::read(&copy).unwrap(), Some(&probe), Opts::default());
        std::fs::remove_file(&copy).unwrap();
        let e = r.expect_err("the corrupt key compared");
        assert!(e.contains(": d has no stored value: StateCorrupt"), "{e}");
    }
}

// ---- WP-255c3: scale (protoc, a Fortran link) ------------------------------

/// The median of `v`.
fn median(v: &[u128]) -> u128 {
    let mut v = v.to_vec();
    v.sort_unstable();
    v.get(v.len() / 2).copied().unwrap_or(0)
}

/// The three-way comparison on a bench workload's link (run by hand):
/// `WP255C3_WORKLOADS` names workload directories of the bench's cache
/// (`<dir>/response.txt`, inputs under `<dir>/build`), colon-separated.
/// The cache is only READ: the link runs in this test's own directory,
/// the relative inputs made absolute. Prints the keys compared, the
/// fall-backs by reason, the disagreement if any, and the median of 5
/// of `StoredRows::new` and of `value_of` over every key.
#[test]
#[ignore = "by hand: needs the bench's prepared workloads"]
fn stored_rows_scale() {
    let list = std::env::var("WP255C3_WORKLOADS").expect("WP255C3_WORKLOADS");
    for w in list.split(':') {
        let w = std::path::Path::new(w);
        let build = w.join("build");
        let label = w.file_name().unwrap().to_string_lossy().into_owned();
        let dir = Scratch::new(&format!("wp255c3-scale-{label}"));
        let text = std::fs::read_to_string(w.join("response.txt")).unwrap();
        let mut args: Vec<OsString> = Vec::new();
        let mut lines = text.lines().peekable();
        while let Some(a) = lines.next() {
            if a == "-o" {
                lines.next();
                args.extend(["-o".into(), "out".into()]);
            } else if a.starts_with("--dependency-file") {
            } else if !a.starts_with('-') && !a.starts_with('/') && build.join(a).exists() {
                args.push(build.join(a).into_os_string());
            } else {
                args.push(a.into());
            }
        }
        let mut inc = args.clone();
        inc.push("--incremental".into());
        let t = std::time::Instant::now();
        let inv = mortar::invoke_in("mortar", &inc, dir.path());
        let link_ms = t.elapsed().as_millis();
        assert!(
            matches!(inv.outcome, mortar::Outcome::Linked { .. }),
            "{label}: {:?}",
            inv.outcome
        );
        let out = std::fs::read(dir.join("out")).unwrap();
        std::fs::remove_file(dir.join("out")).unwrap();
        let state = std::fs::read(dir.join("out.mortar-state")).unwrap();
        std::fs::remove_file(dir.join("out.mortar-state")).unwrap();
        let (out_len, state_len) = (out.len(), state.len());
        let linked = Linked { label: label.clone(), dir, args, out, state };
        let opts = Opts { tally: true, timing: true };
        match compare(&linked, linked.state.clone(), None, opts) {
            Ok(seen) => {
                let n = u128::from(seen.keys.max(1));
                eprintln!(
                    "[wp255c3-scale] {label}: link {link_ms} ms, out {out_len} B, state {state_len} B; \
                     keys {}, renumbered {}, equal {}, .symtab {}, slots {}; fall-backs {:?}; \
                     StoredRows::new median {} us {:?}; value_of all keys median {} us {:?} \
                     ({} ns a key)",
                    seen.keys,
                    seen.renumbered,
                    seen.equal,
                    seen.symtab,
                    seen.slots,
                    seen.reasons,
                    median(&seen.new_us),
                    seen.new_us,
                    median(&seen.value_us),
                    seen.value_us,
                    median(&seen.value_us) * 1000 / n,
                );
            }
            Err(e) => panic!("[wp255c3-scale] {label}: DISAGREEMENT: {e}"),
        }
    }
}

// ---- WP-257a: the state stores what a section IS --------------------------------

/// What one link's sections compare to (WP-257a): counts by class.
#[derive(Debug, Default)]
struct SecSeen {
    sections: usize,
    split: usize,
    merge_kept_collected: usize,
    comdat_losers: usize,
    placeless_with_place: usize,
    large: usize,
}

/// For EVERY section of `linked`'s link, matched by identity (its file by
/// path, its index in the file, its name), the front's answers against
/// the state `state`'s: `Rows::section_kind`, `Rows::section_place`,
/// the stored triple `(kind, kept, out)` against resolve's row and
/// `Assignment::out` (an output section matched by name, and its
/// `OUTSECS` row checked to be its `OutSecId`), and `phase::Link::section`
/// against the triple. `Err` names the first disagreement.
fn sections_agree(linked: &Linked, state: Vec<u8>) -> Result<SecSeen, String> {
    use mortar_incr::format::kind as table;
    use mortar_incr::update::rows::FileTable;
    use mortar_merge::phase::{Link as _, Tables};
    let label = &linked.label;
    let view = mortar_incr::state5::read::open_bytes(state).map_err(|e| format!("open: {e:?}"))?;
    let lo = mortar_incr::state5::read::LayoutOf { view: &view };
    let stored_layout = StoredLayout::new(&lo);
    let parsed = mortar_cli::parse(&linked.args);
    assert!(!parsed.failed());
    let store = Store::new(StorePolicy::from_env());
    let diag = Collector::new(Policy::default());
    let env =
        Env { cwd: linked.dir.path(), pool: Pool::new(2), replay: None, slack: true, state5: true };
    let f = front(parsed.plan, env, &store, &diag, &mut Timer::new(false))
        .unwrap_or_else(|| panic!("front: {:?}", diag.finish()));
    let got_table = f.resolution.def.iter().any(|(s, d)| {
        matches!(d, Def::Internal(InternalDef::Provide { conditional: false }))
            && f.inputs.symbols.get(s).is_some_and(|k| k.name == b"_GLOBAL_OFFSET_TABLE_")
    });
    let mut got_output = f.scanned.demands.output;
    if got_table {
        got_output |= GlobalFlags::GOT_BASE;
    }
    let variant = f.synth.got.variant();
    // The state's files by path (identity; a path twice is refused).
    let mut by_path: HashMap<Vec<u8>, u32> = HashMap::new();
    for r in 0..u32::try_from(view.count(table::FILES)).unwrap() {
        let row = view.file(FileId(r)).map_err(|e| format!("FILES: {e:?}"))?;
        let path = view.string(row.path).map_err(|e| format!("STRINGS: {e:?}"))?;
        if by_path.insert(path, r).is_some() {
            return Err(format!("{label}: two FILES rows of one path"));
        }
    }
    let outsecs = view.outsecs().map_err(|e| format!("OUTSECS: {e:?}"))?;
    let outsec_name = |row: u32| -> Result<Vec<u8>, String> {
        let o = outsecs.get(row as usize).ok_or_else(|| format!("OUTSECS {row}"))?;
        view.string(o.name).map_err(|e| format!("STRINGS: {e:?}"))
    };
    mortar::image::with_link(&f, &store, &|_| {}, |l| {
        let front_rows = FrontRows::new(*l);
        let stored = StoredRows::new(&view, &stored_layout, l.arch, got_output, variant)
            .map_err(|e| format!("{label}: open (StoredRows::new): {e:?}"))?;
        let tables =
            Tables { inputs: l.inputs, resolution: l.resolution, assignment: l.assignment };
        let mut seen = SecSeen::default();
        for (fid, file) in l.inputs.files.iter() {
            let mortar_core::inputs::Reader::Object(object) = file.reader else { continue };
            let Some(&sfile) = by_path.get(file.path) else {
                return Err(format!(
                    "{label}: {} has no FILES row",
                    String::from_utf8_lossy(file.path)
                ));
            };
            let range = l.resolution.section_base[fid];
            let first =
                view.file_index(FileId(sfile)).map_err(|e| format!("FILE_INDEX: {e:?}"))?.ranges
                    [FileTable::Sections as usize];
            if first.count != range.len() {
                return Err(format!(
                    "{label}: file {sfile}: {} stored sections, {} in the front",
                    first.count,
                    range.len()
                ));
            }
            for i in 0..range.len() {
                let id = mortar_core::SectionId(range.start.0 + i);
                let sid = mortar_core::SectionId(first.first + i);
                let at = format!("{label}: {} section {i}", String::from_utf8_lossy(file.path));
                // identity: the stored row's name is the section's
                let row = view.section(sid).map_err(|e| format!("{at}: SECTIONS: {e:?}"))?;
                let name = view.string(row.name).map_err(|e| format!("STRINGS: {e:?}"))?;
                let index = mortar_elf::SectionIndex(i);
                let fname = object.section(index).ok().and_then(|s| s.name().ok());
                if i != 0 && fname.is_some_and(|n| n != &name[..]) {
                    return Err(format!(
                        "{at}: stored row names {}",
                        String::from_utf8_lossy(&name)
                    ));
                }
                let rrow = l.resolution.sections[id];
                let fout = l.assignment.out(id);
                let (fk, fp) = (front_rows.section_kind(id), front_rows.section_place(id));
                let (sk, sp) = (stored.section_kind(sid), stored.section_place(sid));
                if let Some(e) = stored.failure() {
                    return Err(format!("{at}: a stored read failed: {e:?}"));
                }
                if fk != sk || fp != sp {
                    return Err(format!("{at}: kind/place: front {fk:?}/{fp}, stored {sk:?}/{sp}"));
                }
                let Some((kind, kept, out)) = stored.section_fact(sid) else {
                    return Err(format!("{at}: no stored facts: {:?}", stored.failure()));
                };
                if kind != rrow.kind || kept != rrow.is_kept() {
                    return Err(format!("{at}: stored {kind:?}/{kept}, resolve {rrow:?}"));
                }
                match (fout, out) {
                    (None, None) => {}
                    (Some(a), Some(b)) => {
                        let fname = l.assignment.outsecs.get(a).map(|o| o.name.to_vec());
                        if b != a || fname.as_deref() != Some(&outsec_name(b.0)?[..]) {
                            return Err(format!(
                                "{at}: output section: front {a:?} {fname:?}, stored {b:?}"
                            ));
                        }
                    }
                    (a, b) => {
                        return Err(format!("{at}: output section: front {a:?}, stored {b:?}"));
                    }
                }
                let split = tables.section(fid, index);
                let want = (kept && out.is_some()).then(|| (kind, out));
                if split.map(|s| (s.kind, Some(s.out))) != want {
                    return Err(format!("{at}: Link::section {split:?}, stored {want:?}"));
                }
                seen.sections += 1;
                seen.split += usize::from(split.is_some());
                let collected = kept && !l.liveness.is_live(id);
                seen.merge_kept_collected += usize::from(kind == SectionKind::Merge && collected);
                seen.comdat_losers += usize::from(
                    rrow.state
                        == mortar_core::resolution::SectionState::Dropped(
                            mortar_core::resolution::DropReason::Comdat,
                        ),
                );
                seen.placeless_with_place += usize::from(sp && Rows::place(&stored, sid).is_none());
                seen.large +=
                    usize::from(object.section(index).is_ok_and(|s| s.flags() & 0x1000_0000 != 0));
            }
        }
        if let Some(e) = stored.failure() {
            return Err(format!("{label}: a stored read failed: {e:?}"));
        }
        Ok(seen)
    })
    .unwrap()
}

/// x86-64's large-model sections (`SHF_X86_64_LARGE`): `.ldata`, `.lbss`.
fn large_fixture() -> Files {
    vec![(
        "a",
        Lang::Asm,
        "\t.text\n\t.globl _start\n\t.type _start, @function\n_start:\n\
             \tmovabsq $ld, %rax\n\tmovabsq $lb, %rbx\n\tret\n\
             \t.section .ldata,\"awl\",@progbits\n\t.globl ld\nld:\t.quad 1\n\
             \t.section .lbss,\"awl\",@nobits\n\t.globl lb\nlb:\t.zero 16\n\
             \t.section .note.GNU-stack,\"\",@progbits\n"
            .to_owned(),
    )]
}

/// A merge section nothing references, in a section of its own: kept by
/// resolve, collected by `--gc-sections` (the split sees it first).
fn gc_fixture(arch: Arch) -> Files {
    let ty = if arch == Arch::X86_64 { "@" } else { "%" };
    let mut files = u6(arch);
    files.push((
        "dead",
        Lang::Asm,
        format!(
            "\t.section .rodata.str1.1,\"aMS\",{ty}progbits,1\n\t.asciz \"never read\"\n\
             \t.section .text.unused,\"ax\",{ty}progbits\nunused:\tret\n\
             \t.section .note.GNU-stack,\"\",{ty}progbits\n"
        ),
    ));
    files
}

fn sections_check(arch: Arch, label: &str, files: &Files, extra: &[&str]) -> Option<SecSeen> {
    let linked = link_with(arch, label, files, extra)?;
    let seen = sections_agree(&linked, linked.state.clone()).unwrap_or_else(|e| panic!("{e}"));
    eprintln!("[wp257a] {}{extra:?}: {seen:?}", linked.label);
    assert!(seen.sections > 0 && seen.split > 0, "{label} {arch:?}: {seen:?}");
    Some(seen)
}

#[test]
fn stored_section_facts_equal_the_front() {
    let mut ran = 0;
    for arch in ARCHES {
        let mut cases: Vec<(&str, Files, &[&str])> = vec![
            ("s-u6", u6(arch), &[]),
            ("s-c", c_fixture(arch), &[]),
            ("s-cxx", cxx_fixture(), &[]),
            ("s-cxx-gc", cxx_fixture(), &["--gc-sections"]),
            ("s-gc", gc_fixture(arch), &["--gc-sections"]),
        ];
        if arch == Arch::X86_64 {
            cases.push(("s-large", large_fixture(), &[]));
        }
        if std::env::var_os("MASON_KILN").is_some() && std::env::var_os("MASON_HOD").is_some() {
            cases.push(("s-kiln", kiln_fixture(), &[]));
        }
        for (label, files, extra) in &cases {
            let Some(seen) = sections_check(arch, label, files, extra) else {
                assert!(!tools_required(), "{label} {arch:?}: a tool is missing");
                continue;
            };
            ran += 1;
            match *label {
                "s-cxx" => assert!(seen.comdat_losers > 0, "{label} {arch:?}: {seen:?}"),
                "s-gc" => assert!(seen.merge_kept_collected > 0, "{label} {arch:?}: {seen:?}"),
                "s-u6" => assert!(seen.placeless_with_place > 0, "{label} {arch:?}: {seen:?}"),
                "s-large" => assert!(seen.large > 0, "{label} {arch:?}: {seen:?}"),
                _ => {}
            }
        }
    }
    eprintln!("[wp257a] {ran} links compared");
}

/// One field of the first row that satisfies `pick` changed by `mutate`
/// (a valid value), on both targets: the comparison FAILS.
fn sees_stale_section(
    x: &str,
    pick: fn(&mortar_incr::format::SectionRow) -> bool,
    mutate: fn(&mut mortar_incr::format::SectionRow, u16),
) -> Vec<String> {
    let mut seen = Vec::new();
    for arch in ARCHES {
        let Some(linked) =
            link_with(arch, &format!("stale-sec-{x}"), &gc_fixture(arch), &["--gc-sections"])
        else {
            assert!(!tools_required(), "{x} {arch:?}: a tool is missing");
            continue;
        };
        let mut m = mortar_incr::state5::read::decode(&linked.state).unwrap();
        let control = mortar_incr::state5::write::encode(&m).unwrap();
        sections_agree(&linked, control).unwrap_or_else(|e| panic!("control: {e}"));
        let outsecs = u16::try_from(m.outsecs.len()).unwrap();
        let row =
            m.sections.iter().position(pick).unwrap_or_else(|| panic!("{x}: no row to mutate"));
        mutate(&mut m.sections[row], outsecs);
        let bytes = mortar_incr::state5::write::encode(&m)
            .unwrap_or_else(|e| panic!("{x} {arch:?}: the writer refuses the mutation: {e:?}"));
        let r = sections_agree(&linked, bytes);
        eprintln!("[wp257a] stale {x} {arch:?}: {r:?}");
        match r {
            Ok(s) => panic!("{x} {arch:?}: a stale section fact NOTHING sees: {s:?}"),
            Err(e) => seen.push(e),
        }
    }
    seen
}

fn is_merge_placeless(r: &mortar_incr::format::SectionRow) -> bool {
    r.kind == SectionKind::Merge as u8 && r.place_chunk == mortar_incr::format::NONE && r.out != 0
}

#[test]
fn stored_section_facts_see_a_stale_kind() {
    let seen =
        sees_stale_section("kind", is_merge_placeless, |r, _| r.kind = SectionKind::Regular as u8);
    assert!(
        seen.iter().all(|e| e.contains("kind/place") || e.contains("stored Regular")),
        "{seen:?}"
    );
}

#[test]
fn stored_section_facts_see_a_stale_kept() {
    let seen = sees_stale_section("kept", is_merge_placeless, |r, _| r.bits = 0);
    assert!(!seen.is_empty() || !tools_required());
}

#[test]
fn stored_section_facts_see_a_stale_collected() {
    let seen = sees_stale_section(
        "collected",
        |r| is_merge_placeless(r) && r.bits == mortar_incr::format::section_bits::KEPT,
        |r, _| r.bits |= mortar_incr::format::section_bits::COLLECTED,
    );
    assert!(seen.iter().all(|e| e.contains("kind/place")), "{seen:?}");
}

#[test]
fn stored_section_facts_see_a_stale_out() {
    let seen = sees_stale_section("out", is_merge_placeless, |r, n| r.out = r.out % n + 1);
    assert!(seen.iter().all(|e| e.contains("output section")), "{seen:?}");
}

#[test]
fn stored_section_facts_refuse_an_unknown_code() {
    for arch in ARCHES {
        let Some(linked) = link_with(arch, "bad-sec-code", &u6(arch), &[]) else {
            assert!(!tools_required(), "{arch:?}: a tool is missing");
            continue;
        };
        for (what, mutate) in [
            ("kind 14", (|r| r.kind = 14) as fn(&mut mortar_incr::format::SectionRow)),
            ("bit 4", |r| r.bits |= 4),
            ("collected, not kept", |r| r.bits = mortar_incr::format::section_bits::COLLECTED),
            ("out past OUTSECS", |r| r.out = u16::MAX),
        ] {
            let mut m = mortar_incr::state5::read::decode(&linked.state).unwrap();
            let row = m.sections.iter().position(is_merge_placeless).expect("a merge row");
            mutate(&mut m.sections[row]);
            let r = mortar_incr::state5::write::encode(&m)
                .map_err(|e| format!("{e:?}"))
                .and_then(|bytes| sections_agree(&linked, bytes));
            eprintln!("[wp257a] {what} {arch:?}: {r:?}");
            let e = r.expect_err(what);
            assert!(e.contains("state-corrupt") || e.contains("Corrupt"), "{what} {arch:?}: {e}");
        }
    }
}

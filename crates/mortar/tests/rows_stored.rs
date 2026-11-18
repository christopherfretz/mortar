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

use std::collections::HashMap;
use std::ffi::OsString;
use std::process::Command;
use std::time::Duration;

use mortar::front::{Env, front};
use mortar::timing::Timer;
use mortar::update::stored_rows::StoredRows;
use mortar_core::demands::GlobalFlags;
use mortar_core::incr::{IncrReason, StoredLayout, Unimplemented};
use mortar_core::par::Pool;
use mortar_core::resolution::{Def, InternalDef, SectionKind};
use mortar_core::{SymKey, SymbolId};
use mortar_diag::{Collector, Policy};
use mortar_image::rows::{FrontRows, Rows};
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
}

/// The three-way comparison on one fixture; `None` when a tool is
/// missing.
fn check(arch: Arch, label: &str, files: &Files) -> Option<Seen> {
    let dir = Scratch::new(&format!("wp255c-{label}-{arch:?}"));
    let mut objs = Vec::new();
    for (name, lang, text) in files {
        objs.push(compile(arch, &dir, name, *lang, text)?);
    }
    let mut args: Vec<OsString> = objs.iter().map(OsString::from).collect();
    args.extend(["-static", "-e", "_start", "-o", "out"].iter().map(OsString::from));
    // The incremental-mode full link: the output and its format-5 state.
    let mut inc = args.clone();
    inc.push("--incremental".into());
    let inv = mortar::invoke_in("mortar", &inc, dir.path());
    assert!(matches!(inv.outcome, mortar::Outcome::Linked { .. }), "{label}: {:?}", inv.outcome);
    let out = std::fs::read(dir.join("out")).unwrap();
    let state = std::fs::read(dir.join("out.mortar-state")).unwrap();
    assert_eq!(state[..8], mortar_incr::state5::MAGIC, "{label}: a format-5 state");
    let view = mortar_incr::state5::read::open_bytes(state).unwrap();
    let lo = mortar_incr::state5::read::LayoutOf { view: &view };
    let stored_layout = StoredLayout::new(&lo);
    let table = symtab(&out);
    // The same link in this process (incremental mode: slack, format-5
    // data), for `FrontRows`.
    let parsed = mortar_cli::parse(&args);
    assert!(!parsed.failed());
    let store = Store::new(StorePolicy::from_env());
    let diag = Collector::new(Policy::default());
    let env = Env { cwd: dir.path(), pool: Pool::new(2), replay: None, slack: true, state5: true };
    let f = front(parsed.plan, env, &store, &diag, &mut Timer::new(false))
        .unwrap_or_else(|| panic!("front: {:?}", diag.finish()));
    // The `.got` plan's two inputs that are not rows (the record's
    // finding): the scan's output flags with `_GLOBAL_OFFSET_TABLE_`'s
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
    let seen = mortar::image::with_link(&f, &store, &|_| {}, |l| {
        let front_rows = FrontRows::new(*l);
        let stored = StoredRows::new(&view, &stored_layout, l.arch, got_output, variant)
            .unwrap_or_else(|e| panic!("{label}: StoredRows: {e:?}"));
        let mut seen = Seen::default();
        let tls = front_rows.tls().map_or(0, |t| t.start);
        for i in 0..l.inputs.symbols.len() {
            let key = SymKey::global(SymbolId(i));
            let name = front_rows.name(key).to_vec();
            assert_eq!(stored.name(key), &name[..], "{label}: key {i} names another key");
            seen.keys += 1;
            let want = mortar_image::rows::sym(&front_rows, key);
            match stored.value_of(key) {
                Ok(got) => {
                    assert_eq!(got, want, "{label}: {}", String::from_utf8_lossy(&name));
                    seen.equal += 1;
                }
                Err(e) => {
                    // A named fall-back only: a key defined in a merge
                    // section (§9.3) or an input `.eh_frame`.
                    let def_kind = mortar_image::rows::def_entry(&front_rows, key)
                        .and_then(|e| front_rows.entry(e))
                        .and_then(|d| d.place?.ok())
                        .and_then(|s| front_rows.section_kind(s));
                    assert!(
                        matches!(def_kind, Some(SectionKind::Merge | SectionKind::EhFrame)),
                        "{label}: {} has no stored value: {e:?}",
                        String::from_utf8_lossy(&name)
                    );
                    assert_eq!(e, IncrReason::UpdateNotImplemented(Some(Unimplemented::Image)));
                    seen.fallbacks += 1;
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
            assert_eq!(s, v, "{label}: .symtab value of {}", String::from_utf8_lossy(&name));
            seen.symtab += 1;
        }
        seen
    })
    .unwrap();
    eprintln!("[wp255c] {label} {arch:?}: {seen:?}");
    std::fs::remove_file(dir.join("out")).unwrap();
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

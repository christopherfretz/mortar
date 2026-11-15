//! `mortar::image` on real inputs, through the real front
//! (`mortar::front::front`; WP-027c moved these tests here from
//! `mortar_image/tests/link.rs`, whose test front restated the driver): a
//! static AArch64 program written, run, read back by readelf,
//! llvm-readelf and eu-elflint, and checked by `mortar_plumb` with
//! mortar's map; the stores and thread counts against each other; an
//! input changed or truncated before the image's recheck; I-110; the
//! comparison with GNU ld.

mod support;

use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mortar::front::Front;
use mortar_core::FileId;
use mortar_core::par::Pool;
use mortar_diag::Style;
use mortar_image::fill::Failed;
use mortar_image::link::Written;
use mortar_input::store::{Store, StorePolicy};
use mortar_testutil::{Scratch, tool, tools_required};

/// A front and the store it reads.
struct Linked {
    f: Front<'static>,
    store: &'static Store,
}

fn try_front(
    args: &[&str],
    cwd: &Path,
    threads: usize,
    policy: StorePolicy,
) -> Result<Linked, String> {
    // A test's store lives as long as the test binary: one per link.
    let store: &'static Store = Box::leak(Box::new(Store::new(policy)));
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    match support::front_of(&args, cwd, store, Pool::new(threads)) {
        Ok(f) => Ok(Linked { f, store }),
        Err(r) => Err(r.render(&Style::plain("mortar"))),
    }
}

fn front(args: &[&str], cwd: &Path, threads: usize, policy: StorePolicy) -> Linked {
    try_front(args, cwd, threads, policy).unwrap_or_else(|t| panic!("the front: {t}"))
}

fn write_with(l: &Linked, out: &Path, before: &(dyn Fn(FileId) + Sync)) -> Result<Written, Failed> {
    mortar::image::write(&l.f, l.store, out, before, &mut |_, _| {})
}

fn write(l: &Linked, out: &Path) -> Result<Written, Failed> {
    write_with(l, out, &|_| {})
}

/// The map of the link, rendered, at `out.map`.
fn write_map(l: &Linked, out: &Path) -> std::path::PathBuf {
    let map = mortar::image::map_of(&l.f, l.store).expect("the map");
    let path = out.with_extension("map");
    std::fs::write(&path, mortar_image::map::render(&map, &l.f.pool)).unwrap();
    path
}

const A_S: &str = r#"
    .section .rodata.str1.1,"aMS",@progbits,1
msg:
    .asciz "hello\n"
other:
    .asciz "lo\n"

    .text
    .globl _start
    .type _start, %function
_start:
    .cfi_startproc
    // A static program applies its own IRELATIVE records (glibc's
    // `apply_irel`): __rela_iplt_start..__rela_iplt_end.
    adrp x19, __rela_iplt_start
    add x19, x19, :lo12:__rela_iplt_start
    adrp x20, __rela_iplt_end
    add x20, x20, :lo12:__rela_iplt_end
1:
    cmp x19, x20
    b.hs 2f
    ldr x21, [x19]
    ldr x9, [x19, #16]
    blr x9
    str x0, [x21]
    add x19, x19, #24
    b 1b
2:
    mov x0, #1
    adrp x1, msg
    add x1, x1, :lo12:msg
    mov x2, #6
    mov x8, #64
    svc #0
    adrp x3, ptr
    ldr x3, [x3, :lo12:ptr]
    ldr w0, [x3]
    adrp x4, :got:val
    ldr x4, [x4, :got_lo12:val]
    ldr w5, [x4]
    add w0, w0, w5
    mov w22, w0
    bl pick
    add w0, w22, w0
    bl helper
    mov x8, #93
    svc #0
    .cfi_endproc
    .size _start, .-_start

    .data
    .p2align 3
ptr:
    .quad forty
forty:
    .word 40
"#;

/// An IFUNC: `pick`'s resolver returns `pick_zero`, which returns 0.
const C_S: &str = r"
    .text
    .globl pick
    .type pick, %gnu_indirect_function
pick:
    adrp x0, pick_zero
    add x0, x0, :lo12:pick_zero
    ret
    .size pick, .-pick

    .type pick_zero, %function
pick_zero:
    mov w0, #0
    ret
    .size pick_zero, .-pick_zero
";

const B_S: &str = r#"
    .text
    .globl helper
    .type helper, %function
helper:
    .cfi_startproc
    adrp x9, tail
    add x9, x9, :lo12:tail
    ret
    .cfi_endproc
    .size helper, .-helper

    .section .rodata.str1.1,"aMS",@progbits,1
tail:
    .asciz "lo\n"

    .data
    .globl val
    .type val, %object
    .p2align 2
val:
    .word 2
    .size val, 4
"#;

/// The objects of the program in `dir`, or `None` without the tools.
/// With `big`, also `big.o`: 4000 functions compiled with `-g`, whose
/// `.rela.debug_info` and `.text` pass 64 KiB (the staged store copies
/// them, input.md §11.3), and which keeps its file open (§11.4).
fn objects(dir: &Scratch, big: bool) -> Option<()> {
    let Some(cc) = tool("gcc") else {
        assert!(!tools_required(), "gcc is required");
        return None;
    };
    if std::env::consts::ARCH != "aarch64" {
        return None;
    }
    let mut units = vec![("a.o", "a.s"), ("b.o", "b.s"), ("c.o", "c.s")];
    std::fs::write(dir.join("a.s"), A_S).unwrap();
    std::fs::write(dir.join("b.s"), B_S).unwrap();
    std::fs::write(dir.join("c.s"), C_S).unwrap();
    if big {
        let mut src = String::new();
        for i in 0..4000 {
            writeln!(src, "const char *s{i}(int x) {{ return x ? \"string {i}\" : 0; }}").unwrap();
        }
        std::fs::write(dir.join("big.c"), src).unwrap();
        units.push(("big.o", "big.c"));
    }
    for (out, src) in units {
        let o = mortar_testutil::run_with(
            std::process::Command::new(&cc)
                .args(["-c", "-g", "-O1", "-o", out, src])
                .current_dir(dir.path()),
            b"",
            Duration::from_mins(2),
        );
        assert!(o.success(), "gcc: {}", o.stderr_text());
    }
    Some(())
}

const ARGS: &[&str] = &[
    "-maarch64linux",
    "-static",
    "--build-id=sha1",
    "--eh-frame-hdr",
    "-o",
    "out",
    "a.o",
    "b.o",
    "c.o",
];

fn with_big() -> Vec<&'static str> {
    let mut a = ARGS.to_vec();
    a.push("big.o");
    a
}

fn check_tool(name: &str, args: &[&str], file: &Path) -> Option<String> {
    let t = tool(name)?;
    let mut a: Vec<OsString> = args.iter().map(Into::into).collect();
    a.push(file.into());
    let out = mortar_testutil::run(t, a);
    assert!(out.success(), "{name}: {}{}", out.stdout_text(), out.stderr_text());
    Some(out.stdout_text())
}

/// Runs `out`: it prints `hello` and exits 42.
fn runs(out: &Path) {
    let run = mortar_testutil::run_with(
        &mut std::process::Command::new(out),
        b"",
        Duration::from_secs(20),
    );
    assert_eq!(run.stdout_text(), "hello\n");
    assert_eq!(run.status.and_then(|s| s.code()), Some(42), "{}", run.stderr_text());
}

/// eu-elflint, with `support::plumb::ELFLINT_KNOWN` set aside.
fn elflint(out: &Path) {
    support::plumb::elflint(out);
}

/// `mortar_plumb` (`spec/plumb.md` §2) on the written image, its inputs
/// and mortar's map: no finding.
fn plumb(l: &Linked, out: &Path, dir: &Path) {
    let map = write_map(l, out);
    support::plumb::clean(out, &map, dir);
}

#[test]
fn i40_static_program_runs_and_reads_back() {
    let dir = Scratch::new("image-link");
    if objects(&dir, false).is_none() {
        return;
    }
    let f = front(ARGS, dir.path(), 4, StorePolicy::Resident);
    let out = dir.join("out");
    let w = write(&f, &out).unwrap_or_else(|e| panic!("image: {:?}", e.diagnostics));
    assert_eq!(w.build_id.as_ref().map(Vec::len), Some(20));
    runs(&out);
    check_tool("readelf", &["-W", "-a"], &out);
    check_tool("readelf", &["--debug-dump=frames"], &out);
    check_tool("llvm-readelf", &["-a"], &out);
    elflint(&out);
    plumb(&f, &out, dir.path());
}

/// I-81, I-75: the staged store and the resident one give the same
/// bytes, at every thread count, with `-g` inputs whose relocation
/// sections the staged store copies.
#[test]
fn i81_staged_equals_resident_at_every_thread_count() {
    let dir = Scratch::new("image-link-staged");
    if objects(&dir, true).is_none() {
        return;
    }
    let args = with_big();
    let mut first: Option<Vec<u8>> = None;
    for (policy, threads) in [
        (StorePolicy::Resident, 1),
        (StorePolicy::Staged, 1),
        (StorePolicy::Staged, 2),
        (StorePolicy::Resident, 4),
        (StorePolicy::Staged, 8),
    ] {
        let f = front(&args, dir.path(), threads, policy);
        let out = dir.join("out");
        write(&f, &out).unwrap_or_else(|e| panic!("image: {:?}", e.diagnostics));
        let bytes = std::fs::read(&out).unwrap();
        match &first {
            None => {
                runs(&out);
                elflint(&out);
                plumb(&f, &out, dir.path());
                first = Some(bytes);
            }
            Some(b) => assert!(*b == bytes, "{policy:?} at {threads} threads differs"),
        }
    }
}

/// I-65, input.md §11.5: an input that changes before the image's end
/// fails the link with `input-changed`, and no output is left.
#[test]
fn i65_input_changed_at_recheck_leaves_no_output() {
    let dir = Scratch::new("image-link-changed");
    if objects(&dir, true).is_none() {
        return;
    }
    let f = front(&with_big(), dir.path(), 2, StorePolicy::Staged);
    let out = dir.join("out");
    let big = dir.join("big.o");
    let changed = AtomicBool::new(false);
    let e = write_with(&f, &out, &|_| {
        if changed.swap(true, Ordering::SeqCst) {
            return;
        }
        // A newer write of the same bytes: another mtime and ctime.
        let bytes = std::fs::read(&big).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&big, bytes).unwrap();
    })
    .expect_err("a changed input fails the link");
    assert!(e.diagnostics.iter().any(|(_, d)| d.id() == "input-changed"), "{:?}", e.diagnostics);
    assert!(!out.exists(), "no output file after a failed link");
}

/// I-65, input.md §11.5: an input truncated between load and fill (here,
/// at the recheck) fails the link with `input-changed`, and no output is
/// left.
#[test]
fn i65_input_truncated_at_recheck_leaves_no_output() {
    let dir = Scratch::new("image-link-truncated");
    if objects(&dir, true).is_none() {
        return;
    }
    let f = front(&with_big(), dir.path(), 2, StorePolicy::Staged);
    let out = dir.join("out");
    let big = dir.join("big.o");
    let done = AtomicBool::new(false);
    let e = write_with(&f, &out, &|_| {
        if done.swap(true, Ordering::SeqCst) {
            return;
        }
        let len = std::fs::metadata(&big).unwrap().len();
        std::thread::sleep(Duration::from_millis(20));
        std::fs::OpenOptions::new().write(true).open(&big).unwrap().set_len(len / 2).unwrap();
    })
    .expect_err("a truncated input fails the link");
    assert!(e.diagnostics.iter().any(|(_, d)| d.id() == "input-changed"), "{:?}", e.diagnostics);
    assert!(!out.exists(), "no output file after a failed link");
}

/// I-110: a key referenced only from a non-alloc section (`.debug_info`
/// naming an undefined global) fails the link with `undefined-symbol`,
/// the same under the staged and the resident store.
#[test]
fn i110_undefined_from_debug_only_is_refused_by_both_stores() {
    let dir = Scratch::new("image-link-i110");
    if objects(&dir, false).is_none() {
        return;
    }
    std::fs::write(
        dir.join("d.s"),
        "    .section .debug_info,\"\",@progbits\n    .xword nowhere_defined\n",
    )
    .unwrap();
    let o = mortar_testutil::run_with(
        std::process::Command::new(tool("gcc").unwrap())
            .args(["-c", "-o", "d.o", "d.s"])
            .current_dir(dir.path()),
        b"",
        Duration::from_mins(1),
    );
    assert!(o.success(), "gcc: {}", o.stderr_text());
    let mut args = ARGS.to_vec();
    args.push("d.o");
    let mut seen = None;
    for policy in [StorePolicy::Resident, StorePolicy::Staged] {
        let Err(text) = try_front(&args, dir.path(), 2, policy).map(|_| ()) else {
            panic!("{policy:?}: the link is accepted");
        };
        assert!(text.contains("undefined symbol: nowhere_defined"), "{text}");
        match &seen {
            None => seen = Some(text),
            Some(t) => assert_eq!(*t, text, "the stores' diagnostics differ"),
        }
    }
}

// ---- the comparison with GNU ld ---------------------------------------------

/// `readelf -sW`'s `.symtab` rows: (value, size, type, bind, vis, ndx, name).
fn symbols(file: &Path) -> Vec<(u64, u64, String, String, String, String, String)> {
    let text = check_tool("readelf", &["-sW"], file).unwrap_or_default();
    let mut rows = Vec::new();
    let mut in_symtab = false;
    for l in text.lines() {
        if l.starts_with("Symbol table") {
            in_symtab = l.contains("'.symtab'");
            continue;
        }
        // Under `EI_OSABI` 0 readelf names `STT_GNU_IFUNC` by number.
        let l = l.replace("<OS specific>: 10", "IFUNC");
        let t: Vec<&str> = l.split_whitespace().collect();
        if !in_symtab || t.len() < 7 || !t[0].ends_with(':') || t[0] == "Num:" {
            continue;
        }
        let name = t.get(7).copied().unwrap_or("").to_owned();
        rows.push((
            u64::from_str_radix(t[1], 16).unwrap(),
            t[2].parse().unwrap_or(0),
            t[3].to_owned(),
            t[4].to_owned(),
            t[5].to_owned(),
            t[6].to_owned(),
            name,
        ));
    }
    rows
}

/// `readelf -SW`'s section names by index.
fn section_names(file: &Path) -> std::collections::BTreeMap<String, String> {
    let text = check_tool("readelf", &["-SW"], file).unwrap_or_default();
    let mut out = std::collections::BTreeMap::new();
    for l in text.lines() {
        let Some(rest) = l.trim_start().strip_prefix('[') else { continue };
        let Some((n, tail)) = rest.split_once(']') else { continue };
        let Ok(n) = n.trim().parse::<u32>() else { continue };
        out.insert(n.to_string(), tail.split_whitespace().next().unwrap_or("").to_owned());
    }
    out
}

/// The input symbols of `.symtab` as a multiset: (name, type, bind, vis,
/// size, output section), without what the linkers make differently by
/// design (`spec/synth.md` S-75..S-78: section symbols, `STT_FILE`, and
/// GNU ld's own linker-defined symbols, which mortar defines only when
/// referenced, layout §13).
fn symtab_multiset(file: &Path) -> Vec<(String, String, String, String, u64, String)> {
    const GNU_OWN: &[&str] = &[
        "__bss_start",
        "__bss_start__",
        "_bss_end__",
        "__bss_end__",
        "__end__",
        "_edata",
        "_end",
        "__GNU_EH_FRAME_HDR",
        "_GLOBAL_OFFSET_TABLE_",
    ];
    let names = section_names(file);
    let mut v: Vec<_> = symbols(file)
        .into_iter()
        .filter(|s| s.2 != "SECTION" && s.2 != "FILE" && !s.6.is_empty())
        .filter(|s| !GNU_OWN.contains(&s.6.as_str()))
        .map(|s| {
            let sec = names.get(&s.5).cloned().unwrap_or(s.5.clone());
            // resolve.md §8.1: `__rela_iplt_*` are local and hidden; GNU
            // ld writes them local with default visibility.
            let vis = if s.6.starts_with("__rela_iplt_") && s.4 == "HIDDEN" {
                "DEFAULT".to_owned()
            } else {
                s.4
            };
            (s.6, s.2, s.3, vis, s.1, sec)
        })
        .collect();
    v.sort();
    v
}

/// `readelf --debug-dump=frames` with addresses replaced by the function
/// they are in (`name+off`), the eh section's own address dropped, and
/// mortar's terminator (S-49) removed.
fn frames(file: &Path) -> Vec<String> {
    let syms: Vec<(u64, u64, String)> =
        symbols(file).into_iter().filter(|s| s.2 == "FUNC").map(|s| (s.0, s.1, s.6)).collect();
    let at = |a: u64| -> String {
        syms.iter()
            .find(|s| s.0 <= a && a < s.0 + s.1)
            .or_else(|| syms.iter().find(|s| a == s.0 + s.1))
            .map_or_else(|| format!("{a:x}?"), |s| format!("{}+{:x}", s.2, a - s.0))
    };
    let hex = |t: &str| u64::from_str_radix(t, 16).unwrap_or(0);
    let text = check_tool("readelf", &["--debug-dump=frames"], file).unwrap_or_default();
    let mut out = Vec::new();
    for l in text.lines() {
        let l = l.trim();
        if l.is_empty() || l.contains("ZERO terminator") || l.starts_with("Start of frame section")
        {
            continue;
        }
        if let Some((head, pc)) = l.split_once(" pc=") {
            let (a, b) = pc.split_once("..").unwrap();
            out.push(format!("{head} pc={}..{}", at(hex(a)), at(hex(b))));
        } else if let Some(i) = l.find("(offset: ") {
            let rest = &l[i + 9..];
            let (a, tail) = rest.split_once(')').unwrap();
            out.push(format!("entry {} {tail}", at(hex(a.trim_start_matches("0x")))));
        } else {
            out.push(l.to_owned());
        }
    }
    out
}

/// readelf's ELF header lines that do not depend on the layout
/// (`EI_OSABI` is checked with the identification bytes).
fn header(file: &Path) -> Vec<String> {
    let text = check_tool("readelf", &["-hW"], file).unwrap_or_default();
    text.lines()
        .map(str::trim)
        .filter(|l| {
            !["Entry point", "Start of", "Number of", "Section header string", "Magic", "OS/ABI"]
                .iter()
                .any(|p| l.starts_with(p))
        })
        .map(str::to_owned)
        .collect()
}

fn entry(file: &Path) -> u64 {
    let text = check_tool("readelf", &["-hW"], file).unwrap_or_default();
    let l = text.lines().find(|l| l.contains("Entry point address")).unwrap();
    u64::from_str_radix(l.rsplit("0x").next().unwrap().trim(), 16).unwrap()
}

/// The program against GNU ld's link of the same objects: the ELF header
/// field by field (`e_entry` as "the address of `_start`"), `.symtab` as
/// a multiset, and `.eh_frame` and `.eh_frame_hdr` decoded.
#[test]
fn i40_compares_with_gnu_ld() {
    let dir = Scratch::new("image-link-gnu");
    if objects(&dir, false).is_none() {
        return;
    }
    let Some(ld) = tool("ld.bfd") else {
        assert!(!tools_required(), "ld.bfd is required");
        return;
    };
    if tool("readelf").is_none() {
        assert!(!tools_required(), "readelf is required");
        return;
    }
    let o = mortar_testutil::run_with(
        std::process::Command::new(&ld)
            .args([
                "-static",
                "--build-id=sha1",
                "--eh-frame-hdr",
                "-o",
                "ref",
                "a.o",
                "b.o",
                "c.o",
            ])
            .current_dir(dir.path()),
        b"",
        Duration::from_mins(1),
    );
    assert!(o.success(), "ld: {}", o.stderr_text());
    let f = front(ARGS, dir.path(), 4, StorePolicy::Staged);
    let out = dir.join("out");
    write(&f, &out).unwrap_or_else(|e| panic!("image: {:?}", e.diagnostics));
    let reference = dir.join("ref");
    runs(&reference);
    assert_eq!(header(&out), header(&reference));
    let ident = |p: &Path| std::fs::read(p).unwrap()[..16].to_vec();
    let (ours, theirs) = (ident(&out), ident(&reference));
    // a64.md Q11 as ruled: ELFOSABI_GNU with an IFUNC symbol (`pick`), as
    // GNU ld writes.
    assert_eq!(ours[7], 3, "EI_OSABI");
    assert_eq!(ours, theirs);
    let start = |p: &Path| symbols(p).into_iter().find(|s| s.6 == "_start").unwrap().0;
    assert_eq!(entry(&out), start(&out));
    assert_eq!(entry(&reference), start(&reference));
    assert_eq!(symtab_multiset(&out), symtab_multiset(&reference));
    assert_eq!(frames(&out), frames(&reference));
}

/// I-40, layout L-103: with no entry symbol, `e_entry` is 0 (the gABI's
/// "no entry point") and layout's warning says so. The one object has a
/// `.note.gnu.property` (BTI and PAC, `-mbranch-protection=standard`),
/// which the output keeps (S-94) and plumb checks (P1, P2). With no
/// writable data there is no RW `PT_LOAD` (layout L-159: the object's
/// empty `.data` and `.bss` have no chunk), and no empty one.
#[test]
fn i40_no_entry_writes_zero() {
    let dir = Scratch::new("image-link-noentry");
    if objects(&dir, false).is_none() {
        return;
    }
    std::fs::write(dir.join("p.c"), "int v(int x) { return x + 1; }\n").unwrap();
    let o = mortar_testutil::run_with(
        std::process::Command::new(tool("gcc").unwrap())
            .args(["-c", "-O1", "-mbranch-protection=standard", "-o", "p.o", "p.c"])
            .current_dir(dir.path()),
        b"",
        Duration::from_mins(1),
    );
    assert!(o.success(), "gcc: {}", o.stderr_text());
    let f = front(
        &["-maarch64linux", "-static", "-o", "out", "p.o"],
        dir.path(),
        2,
        StorePolicy::Staged,
    );
    assert!(
        f.f.placed.warnings.iter().any(|w| w.diagnostic().id() == "entry-not-found"),
        "{:?}",
        f.f.placed.warnings
    );
    assert!(f.f.synth.property.exists(), "the property note is kept");
    let out = dir.join("out");
    write(&f, &out).unwrap_or_else(|e| panic!("image: {:?}", e.diagnostics));
    let bytes = std::fs::read(&out).unwrap();
    assert_eq!(u64::from_le_bytes(bytes[24..32].try_into().unwrap()), 0);
    let notes = check_tool("readelf", &["-nW"], &out).unwrap_or_default();
    assert!(notes.is_empty() || notes.contains("BTI, PAC"), "{notes}");
    let segs = check_tool("readelf", &["-lW"], &out).unwrap_or_default();
    assert!(segs.is_empty() || segs.contains("GNU_PROPERTY"), "{segs}");
    let loads: Vec<&str> = segs.lines().filter(|l| l.trim_start().starts_with("LOAD")).collect();
    assert!(
        segs.is_empty()
            || loads.iter().all(|l| {
                let t: Vec<&str> = l.split_whitespace().collect();
                !l.contains(" RW ") && t.get(5).is_some_and(|m| *m != "0x000000")
            }),
        "no RW and no empty PT_LOAD: {segs}"
    );
    elflint(&out);
    plumb(&f, &out, dir.path());
}

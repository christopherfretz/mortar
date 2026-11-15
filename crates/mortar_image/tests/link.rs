//! `link::write` on real inputs (WP-027b scope 5): a static AArch64
//! program through the test front (`support::front`, main's phases up to
//! `place`) and the image phase, then run, read back by readelf,
//! llvm-readelf and eu-elflint, and checked by `mortar_plumb`.

mod support;

use std::fmt::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mortar_input::store::StorePolicy;
use mortar_testutil::{Scratch, tool, tools_required};
use support::front::{front, write, write_with};

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
    let mut units = vec![("a.o", "a.s"), ("b.o", "b.s")];
    std::fs::write(dir.join("a.s"), A_S).unwrap();
    std::fs::write(dir.join("b.s"), B_S).unwrap();
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

const ARGS: &[&str] =
    &["-maarch64linux", "-static", "--build-id=sha1", "--eh-frame-hdr", "-o", "out", "a.o", "b.o"];

fn with_big() -> Vec<&'static str> {
    let mut a = ARGS.to_vec();
    a.push("big.o");
    a
}

fn check_tool(name: &str, args: &[&str], file: &Path) -> Option<String> {
    let t = tool(name)?;
    let mut a: Vec<std::ffi::OsString> = args.iter().map(Into::into).collect();
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

/// eu-elflint's findings, but for `.relro_padding`'s name: elflint takes
/// every `.rel*` name for a relocation section (lld's output has the same
/// finding; layout L-64 names the chunk).
fn elflint(out: &Path) {
    let Some(t) = tool("eu-elflint") else { return };
    let o = mortar_testutil::run(t, [std::ffi::OsStr::new("--gnu-ld"), out.as_os_str()]);
    let text = o.stdout_text();
    let rest: Vec<&str> = text.lines().filter(|l| !l.contains("'.relro_padding'")).collect();
    assert!(rest.is_empty(), "eu-elflint: {text}{}", o.stderr_text());
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

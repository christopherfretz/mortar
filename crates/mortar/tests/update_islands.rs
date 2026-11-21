//! WP-273 (`spec/landed/incr/WP-271.md` "WP-273"): an AArch64 update whose
//! edit moves an ADRP onto a page's `0xff8` (erratum 843419, `spec/a64.md`
//! §11) re-plans the patch islands at the update's own addresses: the new
//! site gets a patch appended to its part's island (fixed, R-1), the
//! program returns 42 through it, and the map has the patch.
//!
//! The island has capacity because an executable output section (`.after`)
//! follows `.text` (R-4: the last chunk of a `PT_LOAD` has none, L-9).

mod support;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mortar_testutil::target::Arch;
use mortar_testutil::{Outcome, Scratch, run_exec_with, run_with, tool, tools_required};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

/// `_start` calls `f` and exits with what it returns; `val` (42) is more
/// than 1 MiB past `f` (a 2 MiB `.after`), so the ADR fix is out of reach
/// and a site takes a patch.
const A: &str = "    .text\n    .globl _start\n_start:\n    bl f\n    mov x8, #93\n    svc #0\n\
    \n    .section .after,\"ax\",@progbits\n    .globl after\nafter:\n    ret\n    .skip 0x200000\n\
    \n    .data\n    .balign 8\n    .globl val\nval:\n    .xword 42\n\
    \t.section .note.GNU-stack,\"\",%progbits\n";

/// `f` with its ADRP at page offset `skip` (`0xff0`: no site; `0xff8`: a
/// site, `F` at `Q + 8`), its section the same size in both versions.
fn c(skip: u32) -> String {
    let tail = if skip == 0xff0 { "    nop\n    nop\n" } else { "" };
    format!(
        "    .section .text.f,\"ax\",@progbits\n    .balign 4096\n    .skip {skip:#x}\n\
         \n    .globl f\n    .type f, %function\nf:\n    adrp x0, val\n    ldr x2, [sp]\n\
         \n    ldr x0, [x0, #:lo12:val]\n    ret\n{tail}\
         \t.section .note.GNU-stack,\"\",%progbits\n"
    )
}

fn run_in(dir: &Path, exe: &Path, args: &[OsString], env: &[(&str, &OsString)]) -> Outcome {
    let mut c = Command::new(exe);
    c.args(args).current_dir(dir).env_remove("MORTAR_SCHED_SEED").env_remove("SOURCE_DATE_EPOCH");
    for (k, v) in env {
        c.env(k, v);
    }
    run_with(&mut c, b"", Duration::from_mins(2))
}

fn assemble(gcc: &Path, dir: &Path, name: &str, text: &str) -> PathBuf {
    let src = dir.join(format!("{name}.s"));
    std::fs::write(&src, text).expect("source");
    let obj = dir.join(format!("{name}.o"));
    let o = run_in(dir, gcc, &["-c".into(), src.into(), "-o".into(), obj.clone().into()], &[]);
    assert!(o.success(), "gcc: {}", o.stderr_text());
    obj
}

/// The exit status of `out`, run natively or under qemu.
fn status(dir: &Path, out: &Path) -> Option<i32> {
    let runme = dir.join("runme");
    std::fs::copy(out, &runme).expect("copy");
    let (exe, args) = Arch::Aarch64.runner(&runme);
    let o = run_exec_with(
        || {
            let mut c = Command::new("sh");
            c.arg("-c").arg("ulimit -c 0; exec \"$0\" \"$@\"").arg(&exe).args(&args);
            c.current_dir(dir);
            c
        },
        b"",
        Duration::from_mins(1),
    );
    o.status.and_then(|s| s.code())
}

/// The map's erratum patch lines (`chunk patch __erratum_843419_veneer
/// <addr> <size>`).
fn patches(map: &Path) -> Vec<u64> {
    std::fs::read_to_string(map)
        .expect("the map")
        .lines()
        .filter_map(|l| l.strip_prefix("chunk patch __erratum_843419_veneer "))
        .map(|l| u64::from_str_radix(l.split(' ').next().unwrap(), 16).unwrap())
        .collect()
}

/// The 4-byte word at address `addr` of `out` (through `.text`'s header).
fn word(out: &Path, addr: u64) -> u32 {
    let sections =
        mortar_testutil::run(tool("readelf").expect("readelf"), ["-SW".as_ref(), out.as_os_str()]);
    let line =
        sections.stdout_text().lines().find(|l| l.contains(" .text ")).expect(".text").to_owned();
    let fields: Vec<&str> = line.split(']').nth(1).unwrap().split_whitespace().collect();
    let (text_addr, text_off) =
        (u64::from_str_radix(fields[2], 16).unwrap(), u64::from_str_radix(fields[3], 16).unwrap());
    let image = std::fs::read(out).unwrap();
    let at = (text_off + (addr - text_addr)) as usize;
    u32::from_le_bytes(image[at..at + 4].try_into().unwrap())
}

// spec: WP-271 §4 "Pipeline test" (WP-273): an update that moves `f`'s
// ADRP onto 0xff8 runs, returns 42, and its map has the patch; the
// re-plan appended it (the findings line), the lane's full-scan compare is
// equal, and the verifier finds nothing in the update's output.
#[test]
fn an_update_that_moves_an_adrp_onto_0xff8_gets_a_patch() {
    let Some(gcc) = Arch::Aarch64.tool("gcc") else {
        assert!(!tools_required(), "{} is required", Arch::Aarch64.gnu("gcc"));
        return;
    };
    let scratch = Scratch::new("wp273-island");
    let dir = scratch.path().to_path_buf();
    let a = assemble(&gcc, &dir, "a", A);
    let c0 = assemble(&gcc, &dir, "c0", &c(0xff0));
    let c1 = assemble(&gcc, &dir, "c1", &c(0xff8));
    let obj = dir.join("c.o");
    let out = dir.join("out");
    let map = dir.join("out.map");
    let findings = dir.join("findings.txt");
    let fpath = findings.clone().into_os_string();
    let link = |mode: &str, report: Option<&Path>| {
        let mut args: Vec<OsString> = vec![
            "-static".into(),
            "--fix-cortex-a53-843419".into(),
            a.clone().into(),
            obj.clone().into(),
            "-o".into(),
            out.clone().into(),
            format!("--mortar-map={}", map.display()).into(),
            mode.into(),
        ];
        if let Some(r) = report {
            args.push(format!("--incremental-report={}", r.display()).into());
        }
        let o = run_in(&dir, Path::new(MORTAR), &args, &[("MORTAR_UPDATE_FINDINGS", &fpath)]);
        assert!(o.success(), "mortar {mode}: {}", o.stderr_text());
    };
    std::fs::copy(&c0, &obj).expect("c.o");
    link("--incremental=full", None);
    assert_eq!(status(&dir, &out), Some(42), "the first link");
    assert!(patches(&map).is_empty(), "the first link has no site");
    // The edit, as a new inode.
    std::fs::remove_file(&obj).expect("c.o");
    std::fs::copy(&c1, &obj).expect("c.o");
    std::fs::write(&findings, b"").expect("findings");
    let rep = dir.join("rep.txt");
    link("--incremental=update", Some(&rep));
    let report = std::fs::read_to_string(&rep).expect("the report");
    let lines = std::fs::read_to_string(&findings).expect("the findings");
    assert!(report.starts_with("incremental: update"), "{report}\n{lines}");
    let replan = lines.lines().find(|l| l.starts_with("WP-273 replan: ")).unwrap_or_else(|| {
        panic!("no re-plan line: {lines}");
    });
    assert!(replan.contains(" 1 appended, "), "{replan}");
    assert!(replan.ends_with("full-scan compare: equal"), "{replan}");
    assert!(!lines.contains("finding"), "{lines}");
    assert_eq!(status(&dir, &out), Some(42), "the load through the patch");
    let p = patches(&map);
    assert_eq!(p.len(), 1, "the update's map has the patch");
    // a64.md §11.2: the patch holds `W4'` (`ldr x0, [x0, #lo12]`) and a
    // `b`; the site's `F` (two words after the ADRP) branches to it.
    assert_eq!(word(&out, p[0]) & 0xffc0_03ff, 0xf940_0000, "W4' is the load");
    assert_eq!(word(&out, p[0] + 4) >> 26, 0b00_0101, "then a b");
    support::plumb::clean(&out, &map, &dir);
}

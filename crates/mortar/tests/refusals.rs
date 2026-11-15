//! The refusals and warnings a real link line reaches, through the `mortar`
//! executable, with their exact text (`spec/diag.md` §4, §5).

mod support;

use std::ffi::OsString;
use std::process::Command;

use mortar_testutil::{DEFAULT_TIMEOUT, Scratch, run_with};
use support::{Built, C_PROGRAM, Driver, build};

const MORTAR: &str = env!("CARGO_BIN_EXE_mortar");

/// Runs mortar on `built`'s link line with `extra` appended and `env` set;
/// returns (status, stderr).
fn link(built: &Built, extra: &[&str], env: &[(&str, &str)]) -> (Option<i32>, String) {
    let mut args: Vec<OsString> = built.args.iter().map(OsString::from).collect();
    args.extend(extra.iter().map(OsString::from));
    let mut cmd = Command::new(MORTAR);
    cmd.args(&args)
        .current_dir(&built.dir)
        .env_remove("MORTAR_SCHED_SEED")
        .envs(env.iter().copied());
    let o = run_with(&mut cmd, &[], DEFAULT_TIMEOUT);
    (o.status.and_then(|s| s.code()), o.stderr_text())
}

fn c_program(label: &str) -> Option<(Scratch, Built)> {
    let scratch = Scratch::new(label);
    let built = build(&scratch, Driver::Gcc, &["-O2"], C_PROGRAM)?;
    Some((scratch, built))
}

// spec: image.md I-5 (`--gc-sections` is refused in M1, by name, once
// the command line says it is on; nothing is written)
#[test]
fn gc_sections_is_refused_by_name() {
    let Some((_s, built)) = c_program("refuse-gc") else { return };
    let (status, stderr) = link(&built, &["--gc-sections"], &[]);
    assert_eq!(status, Some(1));
    assert_eq!(stderr, "mortar: error: unsupported option: --gc-sections\n");
    assert!(!built.dir.join("out").exists());
}

// spec: layout.md L-103, resolve.md §5.5 (an undefined entry is one
// warning, resolve's, and the link goes on with `e_entry = 0`)
#[test]
fn an_undefined_entry_is_one_warning() {
    let Some((_s, built)) = c_program("refuse-entry") else { return };
    let (status, stderr) = link(&built, &["-e", "no_such_entry"], &[]);
    assert_eq!(status, Some(0), "a warning does not fail the link");
    assert_eq!(stderr, "mortar: warning: cannot find entry symbol no_such_entry\n");
    let out = std::fs::read(built.dir.join("out")).expect("the output");
    assert_eq!(&out[24..32], &[0u8; 8], "e_entry");
}

// spec: model.md §6 (a malformed MORTAR_SCHED_SEED is an error at
// mortar_core::seed::SEED_KEY, never ignored; nothing is linked)
#[test]
fn a_bad_schedule_seed_is_reported() {
    let Some((_s, built)) = c_program("refuse-seed") else { return };
    for bad in ["bogus", "-1", "18446744073709551616", ""] {
        let (status, stderr) = link(&built, &[], &[("MORTAR_SCHED_SEED", bad)]);
        assert_eq!(status, Some(1), "{bad:?}");
        assert_eq!(
            stderr,
            format!(
                "mortar: error: MORTAR_SCHED_SEED: invalid value '{bad}'\n\
                 >>> expected a decimal integer below 2^64\n"
            ),
            "{bad:?}"
        );
    }
    let (status, stderr) = link(&built, &[], &[("MORTAR_SCHED_SEED", "12345")]);
    assert_eq!((status, stderr.as_str()), (Some(0), ""));
}

// spec: docs/ROADMAP.md (M1 links AArch64): an x86-64 or i386 link is
// refused by name after the input phase, and objects of two machines are
// input.md's `incompatible-file`
#[test]
fn another_machine_is_refused_by_name() {
    let Some(clang) = mortar_testutil::tool("clang") else { return };
    let scratch = Scratch::new("refuse-machine");
    let src = scratch.write("f.c", "int f(void) { return 1; }\n");
    let mut objects = Vec::new();
    for (target, obj) in [
        ("x86_64-linux-gnu", "x86.o"),
        ("i386-linux-gnu", "i386.o"),
        ("aarch64-linux-gnu", "a64.o"),
    ] {
        let o = mortar_testutil::run(
            &clang,
            [
                OsString::from(format!("--target={target}")),
                "-c".into(),
                src.clone().into_os_string(),
                "-o".into(),
                scratch.join(obj).into_os_string(),
            ],
        );
        if !o.success() {
            return; // no cross target in this clang
        }
        objects.push(obj);
    }
    let run = |args: &[&str]| {
        let mut cmd = Command::new(MORTAR);
        cmd.args(args).current_dir(scratch.path()).env_remove("MORTAR_SCHED_SEED");
        let o = run_with(&mut cmd, &[], DEFAULT_TIMEOUT);
        (o.status.and_then(|s| s.code()), o.stderr_text())
    };
    for (obj, name) in [("x86.o", "x86-64"), ("i386.o", "i386")] {
        assert_eq!(
            run(&["-e", "f", obj, "-o", "out"]),
            (
                Some(1),
                format!(
                    "mortar: error: unsupported feature: linking for {name}\n\
                     >>> M1 links AArch64 (docs/ROADMAP.md)\n"
                )
            )
        );
    }
    assert_eq!(
        run(&["-e", "f", "a64.o", "x86.o", "-o", "out"]),
        (
            Some(1),
            "mortar: error: x86.o: incompatible file: elf64-littleaarch64 is expected but got elf64-x86-64\n\
             >>> the target was set by a64.o\n"
                .to_owned()
        )
    );
}

// spec: image.md §11 (`--time-phases`: one line per phase run, then the
// total, the image's 100 and 105 among them), cli.md §3.10 (`--threads`)
#[test]
fn time_phases_lists_the_front() {
    let Some((_s, built)) = c_program("time-phases") else { return };
    let (status, stderr) = link(&built, &["--threads=3", "--time-phases"], &[]);
    assert_eq!(status, Some(0));
    let (report, phases) =
        stderr.split_at(stderr.find("mortar: time-phases:").unwrap_or_else(|| panic!("{stderr}")));
    assert_eq!(report, "");
    let rows: Vec<&str> = phases
        .lines()
        .map(|l| {
            let l = l.strip_prefix("mortar: time-phases: ").unwrap_or_else(|| panic!("{l}"));
            let (name, rest) = l.split_once(": ").unwrap_or_else(|| panic!("{l}"));
            assert!(
                rest.starts_with("wall ") && rest.contains(" ms, cpu ") && rest.contains(" MiB"),
                "{l}"
            );
            name
        })
        .collect();
    assert_eq!(
        rows,
        [
            "10 command line",
            "20 input",
            "30 resolve",
            "45 assignment",
            "60 split",
            "50 liveness",
            "70 scan",
            "80 synthetic",
            "90 layout",
            "100 image",
            "105 output file",
            "total"
        ]
    );
    let (_, quiet) = link(&built, &["--threads=1"], &[]);
    assert!(!quiet.contains("time-phases"), "{quiet}");
}

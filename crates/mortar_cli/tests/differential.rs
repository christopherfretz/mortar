//! Accept or reject, against GNU ld (`spec/cli.md` §7): each command line
//! is given to `ld.bfd` (built for the target) and to [`parse`]. GNU ld
//! accepts when it exits 0; mortar when the parse has no error. Where
//! mortar disagrees on purpose the case says so and why (§7.2), and the
//! test checks that the disagreement is still the one recorded.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use mortar_cli::parse;
use mortar_testutil::{DEFAULT_TIMEOUT, Scratch, run_with, tool};

/// What GNU ld does with a case, against mortar.
#[derive(Clone, Copy)]
enum Expect {
    /// Both accept.
    Accept,
    /// Both reject.
    Reject,
    /// GNU ld accepts, mortar rejects: the reason (§7.2).
    OnlyGnu(&'static str),
    /// mortar accepts, GNU ld rejects: the reason (§7.2).
    OnlyMortar(&'static str),
}

use Expect::{Accept, OnlyGnu, OnlyMortar, Reject};

/// Options appended after `-o out obj.o`, for every target.
#[rustfmt::skip]
const COMMON: &[(&[&str], Expect)] = &[
    // §2.2: names, one or two dashes; l/m/o after one dash.
    (&["-shared"], Accept), (&["--shared"], Accept), (&["-pie"], Accept),
    (&["--pic-executable"], Accept), (&["-Bstatic"], Accept), (&["--Bstatic"], Accept),
    (&["-dc"], Accept), (&["-d"], Accept), (&["--omagic"], Accept), (&["-omagic"], Accept),
    (&["-v"], Accept), (&["--version"], Accept), (&["--help"], Accept), (&["-help"], Accept),
    // §2.3: values.
    (&["-e", "_start"], Accept), (&["-e_start"], Accept), (&["--entry=_start"], Accept),
    (&["-Ttext=0x10000"], Accept), (&["-Ttext", "0x10000"], Accept),
    (&["--build-id"], Accept), (&["--build-id=sha1"], Accept), (&["--build-id=0xabcd"], Accept),
    (&["--hash-style=gnu"], Accept), (&["--hash-style=md5"], Reject),
    (&["--shared=x"], Reject), (&["--no-as-needed=x"], Reject),
    (&["-o"], Reject), (&["--hash-style"], Reject), (&["-m"], Reject), (&["-z"], Reject),
    (&["-e"], Reject), (&["-T"], Reject),
    // §2.4: abbreviations.
    (&["-Td", "0x20000"], Accept), (&["-entr=_start"], Accept), (&["-eh"], Accept),
    (&["-sh"], Accept), (&["--gc-sec"], Accept), (&["--no-as"], Accept),
    (&["-j", "/dev/null"], Accept),
    (&["-Tte", "0"], Reject), (&["-em"], Reject), (&["-st"], Reject), (&["-sta"], Reject),
    (&["-rp", "x"], Reject), (&["-Bsym"], Reject), (&["-e=main"], Reject), (&["-T=x"], Reject),
    (&["-u=x"], Reject), (&["--rel"], Reject), (&["--libr"], Reject), (&["--noinhibit"], Reject),
    (&["--frobnicate"], Reject), (&["-k"], Reject), (&["-B"], Reject),
    (&["--a=x"], OnlyGnu("GNU ld takes some ambiguous abbreviations (here --a=x); mortar takes none")),
    (&["--f=x"], OnlyGnu("GNU ld takes some ambiguous abbreviations (here --f=x); mortar takes none")),
    (&["--b=sha1"], Accept),
    // §2.5: `--` and grouping.
    (&["--"], OnlyGnu("mortar rejects '--' (spec/cli.md §2.5)")),
    (&["-sx"], OnlyGnu("mortar does not read grouped short options (spec/cli.md §2.5)")),
    // §2.7: -z.
    (&["-z", "now"], Accept), (&["-znow"], Accept), (&["-z", "max-page-size=4096"], Accept),
    (&["-zmax-page-size=0x1000"], Accept), (&["-z", "relro", "-z", "lazy"], Accept),
    (&["-z", "frob"], OnlyGnu("GNU ld warns about an unknown -z keyword and goes on; mortar rejects it")),
    (&["-z", "now=1"], OnlyGnu("GNU ld ignores a value given to a -z keyword that takes none; mortar rejects it")),
    (&["-z", "max-page-size"], OnlyGnu("GNU ld ignores a valued -z keyword without its value; mortar rejects it")),
    (&["-z", "max-page-size=x"], OnlyMortar("mortar keeps -z sizes as text; the phase that uses them checks them (follow-up)")),
    // §3.3: regions.
    (&["--push-state"], Accept), (&["--start-group"], Accept), (&["--pop-state"], Reject),
    (&["--end-group"], Reject), (&["-)"], Reject), (&["--push-state", "--pop-state"], Accept),
    (&["-(", "-("], Accept), (&["-(", "-)", "-)"], Reject),
    // §3.2: kinds.
    (&["-r", "-shared"], Reject), (&["-shared", "-r"], Reject), (&["-pie", "-r"], Reject),
    (&["-static", "-shared"], Accept), (&["-shared", "-pie"], Accept),
    // Options GNU ld does not have.
    (&["--threads"], OnlyMortar("mortar reads lld's and mold's options too (refusing those it does not carry out)")),
    (&["--error-limit=5"], OnlyMortar("mortar reads lld's and mold's options too")),
    (&["-G"], OnlyGnu("GNU ld reads -G without a size as -shared; mortar requires the size")),
];

/// Cases for one target's ld.bfd.
#[rustfmt::skip]
const AARCH64: &[(&[&str], Expect)] = &[
    (&["-m", "aarch64linux"], Accept), (&["-maarch64elf"], Accept),
    (&["--fix-cortex-a53-843419"], Accept), (&["--fix-cortex-a53-843419=adr"], Accept),
    (&["-z", "force-bti"], Accept), (&["-z", "gcs=always"], Accept),
    (&["--ld-generated-unwind-info"], OnlyMortar("without -m mortar does not know the target, so another target's option is checked only when -m names one (spec/cli.md §2.9)")),
    (&["-m", "aarch64linux", "--ld-generated-unwind-info"], Reject),
    (&["-m", "elf_x86_64"], OnlyMortar("one mortar links for every target; one ld.bfd links for its own")),
];

#[rustfmt::skip]
const X86_64: &[(&[&str], Expect)] = &[
    (&["-m", "elf_x86_64"], Accept), (&["-melf_x86_64"], Accept),
    (&["--ld-generated-unwind-info"], Accept), (&["-z", "lam-u48"], Accept), (&["-z", "ibt"], Accept),
    (&["-m", "elf_x86_64", "--fix-cortex-a53-843419"], Reject),
    (&["-m", "elf_x86_64", "-z", "force-bti"], OnlyGnu("GNU ld warns about another target's -z keyword and goes on; mortar rejects it")),
    (&["-m", "elf32_x86_64"], Reject),
];

#[rustfmt::skip]
const I386: &[(&[&str], Expect)] = &[
    (&["-m", "elf_i386"], Accept), (&["-z", "ibt"], Accept), (&["--ld-generated-unwind-info"], Accept),
    (&["-m", "elf_i386", "-z", "lam-u48"], OnlyGnu("GNU ld warns about a -z keyword of another emulation and goes on; mortar rejects it")),
];

fn object(scratch: &Scratch, cc: &str) -> Option<std::path::PathBuf> {
    let cc = tool(cc)?;
    let src = scratch.write("s.c", "void _start(void) {}\n");
    let obj = scratch.join("s.o");
    let outcome = run_with(
        Command::new(cc).args(["-c", "-fPIC", "-O1"]).arg(&src).arg("-o").arg(&obj),
        &[],
        DEFAULT_TIMEOUT,
    );
    assert!(outcome.success(), "{}", outcome.stderr_text());
    Some(obj)
}

fn compare(ld: &str, cc: &str, cases: &[(&[&str], Expect)]) {
    let Some(ld) = tool(ld) else { return };
    let scratch = Scratch::new("differential");
    let Some(obj) = object(&scratch, cc) else { return };
    let out = scratch.join("out");
    let mut wrong = Vec::new();
    for (case, expect) in COMMON.iter().chain(cases) {
        let mut line: Vec<OsString> = vec!["-o".into(), out.clone().into(), obj.clone().into()];
        line.extend(case.iter().map(OsString::from));
        let gnu = run_with(
            Command::new(&ld).args(&line).current_dir(scratch.path()),
            &[],
            DEFAULT_TIMEOUT,
        )
        .success();
        let mortar = !parse(&line).failed();
        let (want_gnu, want_mortar, why) = match expect {
            Accept => (true, true, "both accept"),
            Reject => (false, false, "both reject"),
            OnlyGnu(why) => (true, false, *why),
            OnlyMortar(why) => (false, true, *why),
        };
        if (gnu, mortar) != (want_gnu, want_mortar) {
            wrong.push(format!(
                "{case:?}: GNU ld accepts: {gnu}, mortar accepts: {mortar}; recorded: {why}"
            ));
        }
        // GNU ld may write files named by options (`-omagic`).
        let _ = std::fs::remove_file(scratch.join("magic"));
        let _ = std::fs::remove_file(Path::new(&out));
    }
    assert!(wrong.is_empty(), "accepts or rejects differently:\n{}", wrong.join("\n"));
}

// spec: cli.md §7 (AArch64)
#[test]
fn against_ld_bfd_aarch64() {
    compare("aarch64-linux-gnu-ld.bfd", "gcc", AARCH64);
}

// spec: cli.md §7 (x86-64)
#[test]
fn against_ld_bfd_x86_64() {
    compare("x86_64-linux-gnu-ld.bfd", "x86_64-linux-gnu-gcc", X86_64);
}

// spec: cli.md §7 (i386)
#[test]
fn against_ld_bfd_i386() {
    compare("i686-linux-gnu-ld.bfd", "i686-linux-gnu-gcc", I386);
}

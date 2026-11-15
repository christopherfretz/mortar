//! The grammar, table-driven (`spec/cli.md` §2, §3).

use std::ffi::OsString;

use mortar_cli::{
    BuildId, Color, DynamicLinker, Emulation, Exit, Input, InputFormat, InputSpec, OutputKind,
    Parsed, Target, Threads, VersionStyle, check_target, parse,
};
use mortar_diag::Style;

fn args(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

fn p(list: &[&str]) -> Parsed {
    parse(&args(list))
}

/// The diagnostics, rendered, in key order.
fn errors(parsed: &Parsed) -> Vec<String> {
    let mut d = parsed.diagnostics.clone();
    d.sort_by_key(|a| a.0);
    d.iter().map(|(_, d)| d.render(&Style::plain("ld"))).collect()
}

/// The uses, as `spelling` or `spelling=value`.
fn uses(parsed: &Parsed) -> Vec<String> {
    parsed
        .uses
        .iter()
        .map(|u| match &u.value {
            Some(v) => format!("{}={}", u.spelling, v.to_string_lossy()),
            None => u.spelling.clone(),
        })
        .collect()
}

fn paths(parsed: &Parsed) -> Vec<String> {
    parsed
        .plan
        .inputs
        .iter()
        .map(|i| match &i.spec {
            InputSpec::Path(p) => p.to_string_lossy().into_owned(),
            InputSpec::Library(l) => format!("-l{}", l.to_string_lossy()),
            InputSpec::LibraryFile(l) => format!("-l:{}", l.to_string_lossy()),
            InputSpec::JustSymbols(l) => format!("-R{}", l.to_string_lossy()),
        })
        .collect()
}

/// Accepted forms: (command line, the uses it makes, the inputs).
/// `a.o` is appended to each line.
#[rustfmt::skip]
const ACCEPTED: &[(&[&str], &[&str], &[&str])] = &[
    // spec: cli.md §2.2 (exact names; one or two dashes)
    (&["-o", "out"], &["-o=out"], &[]),
    (&["-oout"], &["-o=out"], &[]),
    (&["--output=out"], &["--output=out"], &[]),
    (&["--output", "out"], &["--output=out"], &[]),
    (&["-shared"], &["-shared"], &[]),
    (&["--shared"], &["--shared"], &[]),
    (&["-Bstatic"], &["-Bstatic"], &[]),
    (&["--Bstatic"], &["--Bstatic"], &[]),
    (&["-dc"], &["-dc"], &[]),
    (&["-d"], &["-d"], &[]),
    (&["--omagic"], &["--omagic"], &[]),
    (&["-j", "syms.o"], &["-just-symbols=syms.o"], &["-Rsyms.o"]),
    // spec: cli.md §2.2 (one dash before l, m, o: the short option)
    (&["-omagic"], &["-o=magic"], &[]),
    (&["-output", "x"], &["-o=utput"], &["x"]),
    (&["-library=c"], &["-l=ibrary=c"], &["-library=c"]),
    (&["-ld-generated-unwind-info"], &["-l=d-generated-unwind-info"], &["-ld-generated-unwind-info"]),
    // spec: cli.md §2.3 (values: joined, `=`, next argument; optional ones
    // only with `=`)
    (&["-e", "main"], &["-e=main"], &[]),
    (&["-emain"], &["-e=main"], &[]),
    (&["--entry=main"], &["--entry=main"], &[]),
    (&["-Ttext=0x1000"], &["-Ttext=0x1000"], &[]),
    (&["-Ttext", "0x1000"], &["-Ttext=0x1000"], &[]),
    (&["--build-id"], &["--build-id"], &[]),
    (&["--build-id=sha1"], &["--build-id=sha1"], &[]),
    (&["--build-id", "sha1"], &["--build-id"], &["sha1"]),
    (&["--threads"], &["--threads"], &[]),
    (&["--threads=4"], &["--threads=4"], &[]),
    (&["--threads", "4"], &["--threads"], &["4"]),
    (&["--color-diagnostics", "never"], &["--color-diagnostics"], &["never"]),
    (&["-o", "-mfoo"], &["-o=-mfoo"], &[]),
    (&["--lto-O2"], &["--lto-O=2"], &[]),
    // spec: cli.md §2.4 (unique abbreviations of GNU ld's long names)
    (&["-Td", "0x10"], &["-Tdata=0x10"], &[]),
    (&["-entr=main"], &["-entry=main"], &[]),
    (&["-eh"], &["-eh-frame-hdr"], &[]),
    (&["-sh"], &["-shared"], &[]),
    (&["--gc-sec"], &["--gc-sections"], &[]),
    (&["--no-as"], &["--no-as-needed"], &[]),
    // spec: cli.md §2.7 (-z)
    (&["-z", "now"], &["-z now"], &[]),
    (&["-znow"], &["-z now"], &[]),
    (&["-z", "max-page-size=4096"], &["-z max-page-size=4096"], &[]),
    (&["-zmax-page-size=0x1000"], &["-z max-page-size=0x1000"], &[]),
    (&["-z", "bti-report"], &["-z bti-report"], &[]),
    (&["-z", "bti-report=error"], &["-z bti-report=error"], &[]),
    // spec: cli.md §2.8 (-m)
    (&["-m", "elf_x86_64"], &["-m=elf_x86_64"], &[]),
    (&["-melf_i386"], &["-m=elf_i386"], &[]),
    (&["-m", "aarch64elf", "-m", "aarch64linux"], &["-m=aarch64elf", "-m=aarch64linux"], &[]),
    // spec: cli.md §3.3 (inputs)
    (&["-lc"], &["-l=c"], &["-lc"]),
    (&["-l", ":libc.a"], &["-l=:libc.a"], &["-l:libc.a"]),
    (&["--library=m"], &["--library=m"], &["-lm"]),
    (&["-R", "syms.o"], &["-R=syms.o"], &["-Rsyms.o"]),
    (&["-"], &[], &["-"]),
];

// spec: cli.md §2.2, §2.3, §2.4, §2.7, §2.8, §3.3
#[test]
fn accepted_forms() {
    for (line, want_uses, want_inputs) in ACCEPTED {
        let mut full: Vec<&str> = line.to_vec();
        full.push("a.o");
        let parsed = p(&full);
        assert_eq!(errors(&parsed), Vec::<String>::new(), "{line:?}");
        assert_eq!(uses(&parsed), *want_uses, "{line:?}");
        let mut inputs: Vec<String> = want_inputs.iter().map(|s| (*s).to_owned()).collect();
        inputs.push("a.o".to_owned());
        assert_eq!(paths(&parsed), inputs, "{line:?}");
    }
}

/// Rejected forms: (command line, the report). `a.o` is appended.
#[rustfmt::skip]
const REJECTED: &[(&[&str], &str)] = &[
    // spec: cli.md §2.4 (ambiguous abbreviations; no short fallback)
    (&["-Tte", "0"], "ld: error: unknown argument '-Tte'\n>>> ambiguous abbreviation: it could be -Ttext, -Ttext-segment (spec/cli.md §2.4)\n"),
    (&["-em"], "ld: error: unknown argument '-em'\n>>> ambiguous abbreviation: it could be -embedded-relocs, -emit-relocs (spec/cli.md §2.4)\n"),
    (&["-rp", "x"], "ld: error: unknown argument '-rp'\n>>> ambiguous abbreviation: it could be -rpath, -rpath-link (spec/cli.md §2.4)\n"),
    (&["-Bsym"], "ld: error: unknown argument '-Bsym'\n>>> ambiguous abbreviation: it could be -Bsymbolic, -Bsymbolic-functions (spec/cli.md §2.4)\n"),
    (&["--rel"], "ld: error: unknown argument '--rel'\n>>> ambiguous abbreviation: it could be --relax, --relocatable (spec/cli.md §2.4)\n"),
    (&["--libr"], "ld: error: unknown argument '--libr'\n>>> ambiguous abbreviation: it could be --library, --library-path (spec/cli.md §2.4)\n"),
    (&["--noinhibit"], "ld: error: unknown argument '--noinhibit'\n>>> ambiguous abbreviation: it could be --noinhibit-exec, --noinhibit_exec (spec/cli.md §2.4)\n"),
    // spec: cli.md §2.5 (`--` and grouped short options)
    (&["--"], "ld: error: unknown argument '--'\n>>> GNU ld reads every argument after '--' as an input; mortar does not accept '--' (spec/cli.md §2.5)\n"),
    (&["-sx"], "ld: error: unknown argument '-sx'\n>>> -s takes no value, and mortar does not read grouped short options (spec/cli.md §2.5)\n"),
    (&["--frobnicate"], "ld: error: unknown argument '--frobnicate'\n"),
    (&["-k"], "ld: error: unknown argument '-k'\n"),
    // spec: cli.md §2.3 (values)
    (&["--shared=x"], "ld: error: unknown argument '--shared=x'\n>>> --shared takes no value (spec/cli.md §2.3)\n"),
    (&["--hash-style=md5"], "ld: error: --hash-style: invalid value 'md5'\n>>> expected one of: sysv, gnu, both, none\n"),
    (&["--error-limit=many"], "ld: error: --error-limit: invalid value 'many'\n>>> expected a decimal number\n"),
    (&["--build-id=0xZZ"], "ld: error: --build-id: invalid value '0xZZ'\n>>> expected an even number of hexadecimal digits\n"),
    (&["--build-id=0xabc"], "ld: error: --build-id: invalid value '0xabc'\n>>> expected an even number of hexadecimal digits\n"),
    (&["--build-id=0x"], "ld: error: --build-id: invalid value '0x'\n>>> expected an even number of hexadecimal digits\n"),
    (&["--build-id=0XAB"], "ld: error: --build-id: invalid value '0XAB'\n>>> expected one of: none, md5, sha1, sha256, fast, uuid, 0xHEX\n"),
    // spec: cli.md §2.7 (-z)
    (&["-z", "frob"], "ld: error: unknown argument '-z frob'\n>>> GNU ld, lld and mold warn and go on; mortar does not ignore a -z keyword (spec/cli.md §2.7)\n"),
    (&["-z", "now,relro"], "ld: error: unknown argument '-z now,relro'\n>>> GNU ld, lld and mold warn and go on; mortar does not ignore a -z keyword (spec/cli.md §2.7)\n"),
    (&["-z", "now=1"], "ld: error: unknown argument '-z now=1'\n>>> -z now takes no value (spec/cli.md §2.7)\n"),
    (&["-z", "max-page-size"], "ld: error: -z max-page-size: missing argument\n"),
    (&["-z", "gcs=sometimes"], "ld: error: -z gcs: invalid value 'sometimes'\n>>> expected one of: always, never, implicit\n"),
    // spec: cli.md §2.8 (an emulation mortar does not link for)
    (&["-m", "elf32_x86_64"], "ld: error: unsupported emulation: elf32_x86_64\n>>> mortar links for aarch64linux, aarch64elf, elf_x86_64, elf_i386\n"),
    (&["-map-whole-files"], "ld: error: unsupported emulation: ap-whole-files\n>>> mortar links for aarch64linux, aarch64elf, elf_x86_64, elf_i386\n"),
    // spec: cli.md §3.3 (unbalanced regions)
    (&["--pop-state"], "ld: error: --pop-state without a matching --push-state\n"),
    (&["--end-group"], "ld: error: --end-group without a matching --start-group\n"),
    (&["-)"], "ld: error: -) without a matching --start-group\n"),
    (&["--end-lib"], "ld: error: --end-lib without a matching --start-lib\n"),
    (&["--start-lib", "--start-lib"], "ld: error: --start-lib without a matching --end-lib\n"),
    // spec: cli.md §3.2 (-r with -shared or -pie)
    (&["-r", "-shared"], "ld: error: -r and -shared cannot be used together\n"),
    (&["-pie", "-r"], "ld: error: -r and -pie cannot be used together\n"),
];

// spec: cli.md §2.3, §2.4, §2.5, §2.7, §2.8, §3.2, §3.3
#[test]
fn rejected_forms() {
    for (line, want) in REJECTED {
        let mut full: Vec<&str> = line.to_vec();
        full.push("a.o");
        let parsed = p(&full);
        assert!(parsed.failed(), "{line:?}");
        assert_eq!(errors(&parsed).concat(), *want, "{line:?}");
    }
}

// spec: cli.md §2.3 (a value missing at the end of the line)
#[test]
fn missing_values() {
    for (line, want) in [
        (&["a.o", "-o"][..], "ld: error: -o: missing argument\n"),
        (&["a.o", "--output"], "ld: error: --output: missing argument\n"),
        (&["a.o", "--hash-style"], "ld: error: --hash-style: missing argument\n"),
        (&["a.o", "-m"], "ld: error: -m: missing argument\n"),
        (&["a.o", "-z"], "ld: error: -z: missing argument\n"),
        (&["a.o", "-Td"], "ld: error: -Tdata: missing argument\n"),
    ] {
        assert_eq!(errors(&p(line)).concat(), want, "{line:?}");
    }
}

// spec: cli.md §2.10 (errors are keyed by argument: they report in
// argument order whatever their kind)
#[test]
fn errors_are_in_argument_order() {
    let parsed = p(&["--b0gus", "-z", "frob", "a.o", "--hash-style=x", "--frob"]);
    let report: Vec<String> =
        errors(&parsed).iter().map(|e| e.lines().next().unwrap_or_default().to_owned()).collect();
    assert_eq!(
        report,
        [
            "ld: error: unknown argument '--b0gus'",
            "ld: error: unknown argument '-z frob'",
            "ld: error: --hash-style: invalid value 'x'",
            "ld: error: unknown argument '--frob'",
        ]
    );
}

fn state_of(parsed: &Parsed, name: &str) -> Input {
    parsed
        .plan
        .inputs
        .iter()
        .find(|i| match &i.spec {
            InputSpec::Path(p) => p == name,
            InputSpec::Library(l) | InputSpec::LibraryFile(l) | InputSpec::JustSymbols(l) => {
                *l == *name
            }
        })
        .cloned()
        .unwrap_or_else(|| panic!("no input {name}"))
}

// spec: cli.md §3.3 (positional state)
#[test]
fn positional_state() {
    let parsed = p(&[
        "a.o",
        "--as-needed",
        "-lc",
        "--no-as-needed",
        "b.o",
        "--push-state",
        "--whole-archive",
        "-Bstatic",
        "w.a",
        "--pop-state",
        "after.a",
        "--start-group",
        "g1.a",
        "-(",
        "g2.a",
        "-)",
        "g3.a",
        "--end-group",
        "out.a",
        "--start-lib",
        "l1.o",
        "--end-lib",
        "-b",
        "binary",
        "d.bin",
        "-b",
        "default",
        "e.o",
        "--copy-dt-needed-entries",
        "f.so",
    ]);
    assert_eq!(errors(&parsed), Vec::<String>::new());
    let a = state_of(&parsed, "a.o").state;
    assert!(!a.as_needed && !a.whole_archive && !a.link_static && a.group.is_none());
    assert!(state_of(&parsed, "c").state.as_needed);
    assert!(!state_of(&parsed, "b.o").state.as_needed);
    let w = state_of(&parsed, "w.a").state;
    assert!(w.whole_archive && w.link_static);
    let after = state_of(&parsed, "after.a").state;
    assert!(!after.whole_archive && !after.link_static, "--pop-state restores");
    assert_eq!(state_of(&parsed, "g1.a").state.group, Some(1));
    assert_eq!(state_of(&parsed, "g2.a").state.group, Some(1), "nested: the outer group");
    assert_eq!(state_of(&parsed, "g3.a").state.group, Some(1));
    assert_eq!(state_of(&parsed, "out.a").state.group, None);
    assert_eq!(state_of(&parsed, "l1.o").state.lib, Some(1));
    assert_eq!(state_of(&parsed, "d.bin").state.format, InputFormat::Binary);
    assert_eq!(state_of(&parsed, "e.o").state.format, InputFormat::Detect);
    assert!(state_of(&parsed, "f.so").state.copy_dt_needed);
    // Inputs keep their argument indexes.
    assert_eq!(state_of(&parsed, "a.o").arg, 0);
    assert_eq!(state_of(&parsed, "c").arg, 2);
}

// spec: cli.md §3.3 (--push-state does not save -b, the group or the lib
// region; unterminated regions are accepted)
#[test]
fn push_state_saves_four_switches_only() {
    let parsed = p(&[
        "--push-state",
        "-b",
        "binary",
        "--start-group",
        "x.a",
        "--pop-state",
        "y.a",
        "--push-state",
        "--start-lib",
        "z.o",
    ]);
    assert_eq!(errors(&parsed), Vec::<String>::new());
    let y = state_of(&parsed, "y.a").state;
    assert_eq!(y.format, InputFormat::Binary);
    assert_eq!(y.group, Some(1));
    assert_eq!(state_of(&parsed, "z.o").state.lib, Some(1));
}

// spec: cli.md §3.2 (the output kind: the last of -pie, -shared, -no-pie;
// -r; static when no program interpreter is named)
#[test]
fn output_kind() {
    const DL: [&str; 2] = ["-dynamic-linker", "/lib/ld.so"];
    for (line, dynamic, want) in [
        (&[][..], true, OutputKind::Executable),
        (&[][..], false, OutputKind::StaticExecutable),
        (&["-no-pie"], true, OutputKind::Executable),
        (&["-pie"], true, OutputKind::PieExecutable),
        (&["-pie"], false, OutputKind::StaticPie),
        (&["-shared", "-pie"], true, OutputKind::PieExecutable),
        (&["-pie", "-shared"], true, OutputKind::Shared),
        (&["-pie", "-shared"], false, OutputKind::Shared),
        (&["-pie", "-no-pie"], true, OutputKind::Executable),
        (&["-r"], false, OutputKind::Relocatable),
        (&["-r", "-no-pie"], false, OutputKind::Relocatable),
        (&["-static"], false, OutputKind::StaticExecutable),
        (&["-static", "-pie", "--no-dynamic-linker", "-z", "text"], false, OutputKind::StaticPie),
        (&["-static", "-shared"], false, OutputKind::Shared),
        (&["-Bstatic"], true, OutputKind::Executable),
        (&["--no-dynamic-linker"], true, OutputKind::StaticExecutable),
        (&["-Bstatic", "-lfoo", "-Bdynamic"], false, OutputKind::StaticExecutable),
    ] {
        let mut full: Vec<&str> = if dynamic { DL.to_vec() } else { Vec::new() };
        full.extend(line);
        full.push("a.o");
        let parsed = p(&full);
        assert_eq!(errors(&parsed), Vec::<String>::new(), "{full:?}");
        assert_eq!(parsed.plan.kind, want, "{full:?}");
    }
}

// spec: cli.md §3.6-§3.10 (typed fields)
#[test]
fn typed_fields() {
    let parsed = p(&[
        "a.o",
        "-o",
        "out",
        "-e",
        "start",
        "--sysroot=/sys",
        "-L",
        "=/usr/lib",
        "-L$SYSROOT/lib",
        "-Lplain",
        "-nostdlib",
        "-soname",
        "libx.so.1",
        "-rpath",
        "/r1",
        "-rpath=/r2",
        "-dynamic-linker",
        "/lib/ld.so",
        "--hash-style=both",
        "-z",
        "now",
        "-z",
        "relro",
        "-z",
        "lazy",
        "--gc-sections",
        "--build-id=0xabcd",
        "--error-limit=7",
        "--fatal-warnings",
        "-w",
        "--color-diagnostics=never",
        "--thread-count",
        "3",
        "-z",
        "max-page-size=65536",
        "--no-undefined",
        "-plugin",
        "/p.so",
        "-plugin-opt=-fresolution=r",
    ]);
    assert_eq!(errors(&parsed), Vec::<String>::new());
    let plan = &parsed.plan;
    assert_eq!(plan.output.as_deref(), Some("out".as_ref()));
    assert_eq!(plan.entry.as_deref(), Some("start".as_ref()));
    assert_eq!(plan.sysroot.as_deref(), Some("/sys".as_ref()));
    let lpaths: Vec<(String, bool)> = plan
        .library_paths
        .iter()
        .map(|l| (l.path.to_string_lossy().into_owned(), l.in_sysroot))
        .collect();
    assert_eq!(
        lpaths,
        [("/usr/lib".to_owned(), true), ("/lib".to_owned(), true), ("plain".to_owned(), false)]
    );
    assert!(plan.nostdlib);
    assert_eq!(plan.dynamic.soname.as_deref(), Some("libx.so.1".as_ref()));
    assert_eq!(plan.dynamic.rpath, args(&["/r1", "/r2"]));
    assert_eq!(plan.dynamic.dynamic_linker, Some(DynamicLinker::Path("/lib/ld.so".into())));
    assert_eq!(plan.z.now, Some(false), "the last of -z now and -z lazy wins");
    assert_eq!(plan.z.relro, Some(true));
    assert_eq!(plan.z.defs, Some(true));
    assert_eq!(plan.z.max_page_size.as_deref(), Some("65536".as_ref()));
    assert!(plan.shaping.gc_sections);
    assert_eq!(plan.shaping.build_id, Some(BuildId::Hex("0xabcd".to_owned())));
    assert_eq!(plan.diagnostics.error_limit, Some(7));
    assert!(plan.diagnostics.fatal_warnings && plan.diagnostics.no_warnings);
    assert_eq!(plan.diagnostics.color, Color::Never);
    assert_eq!(plan.bookkeeping.threads, Some(Threads::Count(3)));
    assert_eq!(plan.plugin.path.as_deref(), Some("/p.so".as_ref()));
    assert_eq!(plan.plugin.options, args(&["-fresolution=r"]));
}

// spec: cli.md §3.3, §3.6, §3.9 (the fields of spec/input.md §17 and spec/resolve.md §18:
// -rpath-link as written, --[no-]allow-shlib-undefined, --[no-]warn-backrefs, --[no-]warn-common)
#[test]
fn input_and_resolve_fields() {
    let plan = p(&["a.o"]).plan;
    assert_eq!(plan.dynamic.allow_shlib_undefined, None);
    assert!(!plan.diagnostics.warn_backrefs && !plan.diagnostics.warn_common);
    let parsed = p(&[
        "a.o",
        "-rpath-link",
        "/a:/b",
        "-rpath-link=/c",
        "--allow-shlib-undefined",
        "--warn-backrefs",
        "--warn-common",
    ]);
    assert_eq!(errors(&parsed), Vec::<String>::new());
    let plan = &parsed.plan;
    assert_eq!(plan.dynamic.rpath_link, args(&["/a:/b", "/c"]), "split by mortar_input, not here");
    assert_eq!(plan.dynamic.allow_shlib_undefined, Some(true));
    assert!(plan.diagnostics.warn_backrefs && plan.diagnostics.warn_common);
    let off = p(&[
        "a.o",
        "--warn-backrefs",
        "--no-warn-backrefs",
        "--warn-common",
        "--no-warn-common",
        "--allow-shlib-undefined",
        "--no-allow-shlib-undefined",
    ])
    .plan;
    assert_eq!(off.dynamic.allow_shlib_undefined, Some(false), "the last wins");
    assert!(!off.diagnostics.warn_backrefs && !off.diagnostics.warn_common);
}

// spec: cli.md §2.10 (the grammar's diagnostics have phase 10, spec/model.md §8; refusals
// come after the errors of their argument)
#[test]
fn keys_have_the_command_line_phase() {
    let parsed = p(&["--no-such-option", "a.o", "--no-fortran-common"]);
    assert!(!parsed.diagnostics.is_empty());
    for (key, _) in parsed.diagnostics.iter().cloned().chain(parsed.refusals()) {
        assert_eq!(key.phase, mortar_cli::PHASE, "{key:?}");
    }
    assert_eq!(mortar_cli::PHASE, 10);
    let refused: Vec<String> =
        parsed.refusals().iter().map(|(_, d)| d.render(&Style::plain("ld"))).collect();
    assert!(refused.iter().any(|r| r.contains("--no-fortran-common")), "{refused:?}");
}

// spec: cli.md §2.9 (options of one target)
#[test]
fn options_of_one_target() {
    let wrong = p(&["-m", "elf_x86_64", "--fix-cortex-a53-843419", "a.o"]);
    assert_eq!(
        errors(&wrong).concat(),
        "ld: error: unknown argument '--fix-cortex-a53-843419'\n\
         >>> --fix-cortex-a53-843419 is an option of AArch64 only; this link is for x86-64\n"
    );
    let right = p(&["-maarch64linux", "--fix-cortex-a53-843419", "a.o"]);
    assert_eq!(errors(&right), Vec::<String>::new());
    // The emulation can come after the option; the last -m decides.
    let later = p(&["-z", "lam-u48", "a.o", "-m", "elf_x86_64", "-m", "elf_i386"]);
    assert_eq!(
        errors(&later).concat(),
        "ld: error: unknown argument '-z lam-u48'\n\
         >>> -z lam-u48 is an option of x86-64 only; this link is for i386\n"
    );
    for (option, ok) in [
        ("-z ibt", [false, true, true]),
        ("-z mark-plt", [false, true, false]),
        ("-z force-bti", [true, false, false]),
        ("--ld-generated-unwind-info", [false, true, true]),
    ] {
        for (target, ok) in Target::ALL.into_iter().zip(ok) {
            let emulation = target.emulations()[0].name();
            let mut line = vec!["-m", emulation];
            line.extend(option.split(' '));
            line.push("a.o");
            assert_eq!(!p(&line).failed(), ok, "{option} for {}", target.name());
        }
    }
    // Without -m the options are recorded and checked later.
    let unknown = p(&["--fix-cortex-a53-843419", "-z", "lam-u48", "a.o"]);
    assert_eq!(errors(&unknown), Vec::<String>::new());
    assert_eq!(unknown.plan.target_options.len(), 2);
    assert_eq!(check_target(&unknown.plan, Target::Aarch64).len(), 1);
    assert_eq!(check_target(&unknown.plan, Target::X86_64).len(), 1);
    assert_eq!(check_target(&unknown.plan, Target::I386).len(), 2);
}

// spec: cli.md §2.9 (the CET rows of `spec/x64.md` §11, R-10): read on
// every target and carried in the plan (`plan.other`, with the value, and
// `plan.target_options`); refused by name on x86-64 until `mortar_x64`
// honours them; on AArch64 an option of x86 only (`unknown-option`);
// `-z force-ibt`, lld's spelling, has no target and is refused on each.
#[test]
fn r10_cet_rows() {
    let line = [
        "-z",
        "ibt",
        "-z",
        "shstk",
        "-z",
        "ibtplt",
        "-z",
        "cet-report=warning",
        "-z",
        "force-ibt",
        "a.o",
    ];
    let parsed = p(&line);
    assert_eq!(errors(&parsed), Vec::<String>::new());
    let other: Vec<(String, Option<String>)> = parsed
        .plan
        .other
        .iter()
        .map(|o| (o.name.clone(), o.value.as_ref().map(|v| v.to_string_lossy().into_owned())))
        .collect();
    assert_eq!(
        other,
        [
            ("-z ibt".to_owned(), None),
            ("-z shstk".to_owned(), None),
            ("-z ibtplt".to_owned(), None),
            ("-z cet-report".to_owned(), Some("warning".to_owned())),
            ("-z force-ibt".to_owned(), None),
        ]
    );
    let targeted: Vec<&str> =
        parsed.plan.target_options.iter().map(|o| o.option.as_str()).collect();
    assert_eq!(targeted, ["-z ibt", "-z shstk", "-z ibtplt", "-z cet-report"]);
    assert!(parsed.plan.target_options.iter().all(|o| o.targets == [Target::X86_64, Target::I386]));
    // x86-64: no target error, each refused by name.
    assert!(check_target(&parsed.plan, Target::X86_64).is_empty());
    let refused: Vec<String> =
        parsed.refusals().iter().map(|(_, d)| d.render(&Style::plain("ld"))).collect();
    assert_eq!(
        refused,
        [
            "ld: error: unsupported option: -z ibt\n",
            "ld: error: unsupported option: -z shstk\n",
            "ld: error: unsupported option: -z ibtplt\n",
            "ld: error: unsupported option: -z cet-report\n",
            "ld: error: unsupported option: -z force-ibt\n",
        ]
    );
    // AArch64: the four x86 rows are unknown options there.
    let a64: Vec<String> = check_target(&parsed.plan, Target::Aarch64)
        .iter()
        .map(|(_, d)| d.render(&Style::plain("ld")))
        .collect();
    assert_eq!(a64.len(), 4);
    assert_eq!(
        a64[3],
        "ld: error: unknown argument '-z cet-report'\n\
         >>> -z cet-report is an option of x86-64 and i386 only; this link is for AArch64\n"
    );
    // The value is a choice; the default (no option) is `none`.
    assert!(p(&["-z", "cet-report=loud", "a.o"]).failed());
    assert!(p(&["-z", "cet-report", "a.o"]).failed());
    for level in ["none", "warning", "error"] {
        assert!(!p(&["-z", &format!("cet-report={level}"), "a.o"]).failed(), "{level}");
    }
}

// spec: synth.md S-70, `docs/ROADMAP.md` M1/M2 (R-9): until the
// static-pie rows are implemented, `gcc -static-pie`'s line is a static
// pie in the plan and refused by name (`-pie` is a refused row).
#[test]
fn r9_static_pie_is_refused_by_name() {
    let parsed = p(&["-static", "-pie", "--no-dynamic-linker", "a.o"]);
    assert_eq!(errors(&parsed), Vec::<String>::new());
    assert_eq!(parsed.plan.kind, OutputKind::StaticPie);
    let refused: Vec<&str> = parsed.refused_uses().iter().map(|u| u.spelling.as_str()).collect();
    assert!(refused.contains(&"-pie"), "{refused:?}");
}

// spec: cli.md §2.8 (each emulation names its target)
#[test]
fn each_emulation() {
    for e in Emulation::ALL {
        let parsed = p(&["-m", e.name(), "a.o"]);
        assert_eq!(errors(&parsed), Vec::<String>::new());
        assert_eq!(parsed.plan.emulation, Some(e));
        assert!(e.target().emulations().contains(&e));
    }
}

// spec: cli.md §4.4 (the refused options used: first use per row, in
// argument order, as written)
#[test]
fn refused_uses_in_argument_order() {
    let parsed = p(&[
        "a.o",
        "--fatal-warnings",
        "-o",
        "x",
        "-Td",
        "0",
        "-o",
        "y",
        "--push-state",
        "-z",
        "now",
        "-EL",
        "--error-limit=3",
        "--output=z",
        "-plugin",
        "p.so",
    ]);
    let refused: Vec<&str> = parsed.refused_uses().iter().map(|u| u.spelling.as_str()).collect();
    assert_eq!(refused, ["-Tdata", "--push-state", "-z now"]);
    let rendered: Vec<String> =
        parsed.refusals().iter().map(|(_, d)| d.render(&Style::plain("ld"))).collect();
    assert_eq!(rendered[0], "ld: error: unsupported option: -Tdata\n");
}

// spec: cli.md §3.3 (`--sysroot`: the last one, either form; an empty
// value is no sysroot)
#[test]
fn sysroot_is_the_last_and_empty_is_none() {
    assert_eq!(p(&["a.o", "--sysroot=/x", "--sysroot", "/y"]).plan.sysroot, Some("/y".into()));
    assert_eq!(p(&["a.o", "--sysroot=/x", "--sysroot="]).plan.sysroot, None);
    assert!(p(&["a.o", "--sysroot=/"]).refusals().is_empty());
}

// spec: cli.md §4.4 (a refused use that a later use of the same
// last-wins field overrides is not refused; a partial row's refused
// values are refused by value, each use)
#[test]
fn overridden_and_partial_uses() {
    let refused = |args: &[&str]| -> Vec<String> {
        p(args).refusals().iter().map(|(_, d)| d.render(&Style::plain("ld"))).collect()
    };
    assert_eq!(refused(&["a.o", "--gc-sections", "--no-gc-sections"]), Vec::<String>::new());
    assert_eq!(
        refused(&["a.o", "--no-gc-sections", "--gc-sections"]),
        ["ld: error: unsupported option: --gc-sections\n"]
    );
    assert_eq!(refused(&["a.o", "--build-id=md5", "--build-id"]), Vec::<String>::new());
    assert_eq!(refused(&["a.o", "--build-id=md5", "--no-build-id"]), Vec::<String>::new());
    let md5 = refused(&["a.o", "--build-id", "--build-id=md5"]);
    assert_eq!(md5.len(), 1);
    assert!(md5[0].starts_with("ld: error: unsupported option value: --build-id=md5\n>>> "));
    assert_eq!(
        refused(&["a.o", "--fix-cortex-a53-843419=adr", "--fix-cortex-a53-843419=adrp"]).len(),
        2,
        "not last-wins: both are refused"
    );
    assert_eq!(
        refused(&["a.o", "--build-id=sha1", "--fix-cortex-a53-843419=full"]),
        Vec::<String>::new()
    );
}

// spec: cli.md §5 (requests: --version and --help stop the parse; -v and
// -V go on; no inputs is an error except with -v)
#[test]
fn requests() {
    let v = p(&["--version", "--frobnicate"]);
    assert_eq!(v.plan.exit, Some(Exit::Version));
    assert_eq!(errors(&v), Vec::<String>::new());
    assert_eq!(p(&["-help", "x.o"]).plan.exit, Some(Exit::Help));
    let before = p(&["--frobnicate", "--help"]);
    assert_eq!(before.plan.exit, Some(Exit::Help));
    assert!(before.failed());
    let small_v = p(&["-v"]);
    assert_eq!(small_v.plan.print_version, Some(VersionStyle::Version));
    assert_eq!(errors(&small_v), Vec::<String>::new());
    let both = p(&["-V", "-v", "a.o"]);
    assert_eq!(both.plan.print_version, Some(VersionStyle::Emulations));
    assert_eq!(errors(&p(&[])).concat(), "ld: error: no input files\n");
    assert_eq!(errors(&p(&["-o", "x"])).concat(), "ld: error: no input files\n");
    assert_eq!(errors(&p(&["-T", "script.ld"])), Vec::<String>::new());
}

// spec: cli.md §2.1 (arguments are bytes; values keep them)
#[cfg(unix)]
#[test]
fn non_utf8_arguments() {
    use std::os::unix::ffi::OsStringExt;
    let bad = OsString::from_vec(b"caf\xe9.o".to_vec());
    let out = OsString::from_vec(b"-o\xff".to_vec());
    let parsed = parse(&[bad.clone(), out]);
    assert_eq!(errors(&parsed), Vec::<String>::new());
    assert_eq!(parsed.plan.inputs[0].spec, InputSpec::Path(bad));
    assert_eq!(parsed.plan.output, Some(OsString::from_vec(vec![0xff])));
    let unknown = parse(&[OsString::from_vec(b"--\xff".to_vec()), "a.o".into()]);
    assert_eq!(errors(&unknown).concat(), "ld: error: unknown argument '--\u{fffd}'\n");
}

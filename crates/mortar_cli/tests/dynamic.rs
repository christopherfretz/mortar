//! The dynamic rows of the command line (`spec/dyn.md` §11.2 R-C1, §4,
//! §5, §7 D-30, D-31; `spec/cli.md` §3.2, §3.3, §3.6, §3.7; WP-042): what
//! each option of a dynamic link line puts in the `Plan`, and which are
//! refused by name, today and once the M2 join lifts the dynamic guard.

use std::ffi::OsString;

use mortar_cli::{DynamicLinker, HashStyle, InputSpec, OutputKind, Parsed, SymbolRole, Use, parse};
use mortar_diag::Style;

fn p(list: &[&str]) -> Parsed {
    let args: Vec<OsString> = list.iter().map(OsString::from).collect();
    parse(&args)
}

/// The grammar's errors, rendered.
fn errors(parsed: &Parsed) -> Vec<String> {
    parsed.diagnostics.iter().map(|(_, d)| d.render(&Style::plain("ld"))).collect()
}

/// A parse that must have no grammar error.
fn ok(list: &[&str]) -> Parsed {
    let parsed = p(list);
    assert_eq!(errors(&parsed), Vec::<String>::new(), "{list:?}");
    parsed
}

/// The uses as written (`spelling` or `spelling=value`).
fn written(uses: &[&Use]) -> Vec<String> {
    uses.iter()
        .map(|u| match &u.value {
            Some(v) => format!("{}={}", u.spelling, v.to_string_lossy()),
            None => u.spelling.clone(),
        })
        .collect()
}

/// The refusals of a line, rendered (the command line's report).
fn refusals(parsed: &Parsed) -> Vec<String> {
    parsed.refusals().iter().map(|(_, d)| d.render(&Style::plain("ld"))).collect()
}

/// Each input by its name, with its `as_needed` and `link_static` state.
fn states(parsed: &Parsed) -> Vec<(String, bool, bool)> {
    parsed
        .plan
        .inputs
        .iter()
        .map(|i| {
            let name = match &i.spec {
                InputSpec::Path(p) => p.to_string_lossy().into_owned(),
                InputSpec::Library(l) => format!("-l{}", l.to_string_lossy()),
                InputSpec::LibraryFile(f) => format!("-l:{}", f.to_string_lossy()),
                InputSpec::JustSymbols(f) => format!("-R{}", f.to_string_lossy()),
            };
            (name, i.state.as_needed, i.state.link_static)
        })
        .collect()
}

fn st(name: &str, as_needed: bool, link_static: bool) -> (String, bool, bool) {
    (name.to_owned(), as_needed, link_static)
}

// spec: dyn.md D-1, D-31; cli.md §3.2 (the kind: -pie, -no-pie and the
// interpreter, every spelling; the last of each wins)
#[test]
fn kind_and_interpreter() {
    const L: &str = "/lib/ld-linux-aarch64.so.1";
    for (line, kind, interp) in [
        (&["-pie", "-dynamic-linker", L][..], OutputKind::PieExecutable, Some(L)),
        (&["-dynamic-linker", L, "-pie"], OutputKind::PieExecutable, Some(L)),
        (&["--pic-executable", "-I", L], OutputKind::PieExecutable, Some(L)),
        (&["-pie", &format!("--dynamic-linker={L}")], OutputKind::PieExecutable, Some(L)),
        (&["-pie", &format!("-I{L}")], OutputKind::PieExecutable, Some(L)),
        (&["-dynamic-linker", L], OutputKind::Executable, Some(L)),
        (&["-pie", "-no-pie", "-dynamic-linker", L], OutputKind::Executable, Some(L)),
        (&["--no-pic-executable", "-dynamic-linker", L], OutputKind::Executable, Some(L)),
        (&["-no-pie", "-pie", "-dynamic-linker", L], OutputKind::PieExecutable, Some(L)),
        (&["-pie"], OutputKind::StaticPie, None),
        (&["-static", "-pie", "--no-dynamic-linker", "-z", "text"], OutputKind::StaticPie, None),
        (&["-dynamic-linker", L, "--no-dynamic-linker"], OutputKind::StaticExecutable, None),
        (&["--no-dynamic-linker", "-dynamic-linker", L], OutputKind::Executable, Some(L)),
        (
            &["-pie", "--no-dynamic-linker", "-dynamic-linker", L],
            OutputKind::PieExecutable,
            Some(L),
        ),
        (&["-no-pie"], OutputKind::StaticExecutable, None),
        (&["-dynamic-linker", "/a", "-dynamic-linker", L], OutputKind::Executable, Some(L)),
    ] {
        let mut full = line.to_vec();
        full.push("m.o");
        let parsed = ok(&full);
        assert_eq!(parsed.plan.kind, kind, "{full:?}");
        let want = interp.map(|i| DynamicLinker::Path(i.into()));
        match &parsed.plan.dynamic.dynamic_linker {
            Some(DynamicLinker::None) => assert!(interp.is_none(), "{full:?}"),
            other => assert_eq!(other, &want, "{full:?}"),
        }
    }
}

// spec: dyn.md §5 D-11, D-19a ("a mention's as-needed state is the one in
// force where it is named; --push-state/--pop-state restore it"); cli.md
// §3.3. The lines are the census drivers' (dyn.md §2.1) and D-19a's.
#[test]
fn as_needed_is_positional() {
    // gcc's default line: a global --as-needed, and libgcc_s inside a
    // push-state pair.
    let parsed = ok(&[
        "--as-needed",
        "Scrt1.o",
        "m.o",
        "-lgcc",
        "--push-state",
        "--as-needed",
        "-lgcc_s",
        "--pop-state",
        "-lc",
        "-lgcc",
        "--push-state",
        "--as-needed",
        "-lgcc_s",
        "--pop-state",
        "crtn.o",
    ]);
    assert_eq!(
        states(&parsed),
        [
            st("Scrt1.o", true, false),
            st("m.o", true, false),
            st("-lgcc", true, false),
            st("-lgcc_s", true, false),
            st("-lc", true, false),
            st("-lgcc", true, false),
            st("-lgcc_s", true, false),
            st("crtn.o", true, false),
        ]
    );
    // clang's: --as-needed and --no-as-needed around libgcc_s, nothing
    // global.
    let parsed = ok(&[
        "m.o",
        "-lgcc",
        "--as-needed",
        "-lgcc_s",
        "--no-as-needed",
        "-lc",
        "-lgcc",
        "--as-needed",
        "-lgcc_s",
        "--no-as-needed",
    ]);
    assert_eq!(
        states(&parsed),
        [
            st("m.o", false, false),
            st("-lgcc", false, false),
            st("-lgcc_s", true, false),
            st("-lc", false, false),
            st("-lgcc", false, false),
            st("-lgcc_s", true, false),
        ]
    );
    // --pop-state restores the state saved, whatever came between: a
    // push without as-needed, then --as-needed inside it.
    let parsed = ok(&[
        "--push-state",
        "--as-needed",
        "-la",
        "--push-state",
        "--no-as-needed",
        "-lb",
        "--pop-state",
        "-lc",
        "--pop-state",
        "-ld",
    ]);
    assert_eq!(
        states(&parsed),
        [
            st("-la", true, false),
            st("-lb", false, false),
            st("-lc", true, false),
            st("-ld", false, false)
        ]
    );
    // D-19a's example: `na --as-needed nx --no-as-needed nb nx nc`.
    let parsed = ok(&["-lna", "--as-needed", "-lnx", "--no-as-needed", "-lnb", "-lnx", "-lnc"]);
    assert_eq!(
        states(&parsed),
        [
            st("-lna", false, false),
            st("-lnx", true, false),
            st("-lnb", false, false),
            st("-lnx", false, false),
            st("-lnc", false, false),
        ]
    );
    // Every input carries the state, objects and `-l:` included, and a
    // group does not change it.
    let parsed =
        ok(&["--as-needed", "--start-group", "-l:libx.so.1", "o.o", "--end-group", "y.so"]);
    assert_eq!(
        states(&parsed),
        [st("-l:libx.so.1", true, false), st("o.o", true, false), st("y.so", true, false)]
    );
}

// spec: dyn.md §2.1 (-static-libstdc++: `-Bstatic -lstdc++ -Bdynamic`),
// D-19; cli.md §3.3 (-Bstatic and its spellings set link_static,
// -Bdynamic and its spellings clear it; --push-state saves it)
#[test]
fn bstatic_and_bdynamic_are_positional() {
    let parsed = ok(&[
        "m.o",
        "-Bstatic",
        "-lstdc++",
        "-Bdynamic",
        "-lm",
        "-static",
        "-la",
        "-dy",
        "-lb",
        "-dn",
        "-lc",
        "-call_shared",
        "-ld",
        "-non_shared",
        "--push-state",
        "-Bdynamic",
        "--as-needed",
        "-le",
        "--pop-state",
        "-lf",
    ]);
    assert_eq!(
        states(&parsed),
        [
            st("m.o", false, false),
            st("-lstdc++", false, true),
            st("-lm", false, false),
            st("-la", false, true),
            st("-lb", false, false),
            st("-lc", false, true),
            st("-ld", false, false),
            st("-le", true, false),
            st("-lf", false, true),
        ]
    );
    // Neither decides the kind (cli.md §3.2).
    assert_eq!(parsed.plan.kind, OutputKind::StaticExecutable);
}

// spec: dyn.md D-21 (-E, --export-dynamic-symbol), cli.md §3.5, §3.6
#[test]
fn exports() {
    for (line, on) in [
        (&["-E"][..], true),
        (&["--export-dynamic"], true),
        (&["-export-dynamic"], true),
        (&["-E", "--no-export-dynamic"], false),
        (&["--no-export-dynamic", "-E"], true),
        (&[], false),
    ] {
        let mut full = line.to_vec();
        full.push("m.o");
        assert_eq!(ok(&full).plan.dynamic.export_dynamic, on, "{full:?}");
    }
    let parsed = ok(&[
        "--export-dynamic-symbol",
        "foo",
        "--export-dynamic-symbol=bar*",
        "-export-dynamic-symbol=baz",
        "m.o",
    ]);
    let syms: Vec<(SymbolRole, String)> = parsed
        .plan
        .symbols
        .iter()
        .map(|s| (s.role, s.value.to_string_lossy().into_owned()))
        .collect();
    assert_eq!(
        syms,
        [
            (SymbolRole::ExportDynamicSymbol, "foo".to_owned()),
            (SymbolRole::ExportDynamicSymbol, "bar*".to_owned()),
            (SymbolRole::ExportDynamicSymbol, "baz".to_owned()),
        ]
    );
}

// spec: dyn.md D-30 row 2 (-rpath in order, as written; DT_RUNPATH by
// default, DT_RPATH under --disable-new-dtags, the last of the two wins)
#[test]
fn rpath_and_dtags() {
    let parsed =
        ok(&["-rpath", "/a", "-rpath=$ORIGIN/../lib", "--rpath", "/a:/c", "-rpath", "/a", "m.o"]);
    let d = &parsed.plan.dynamic;
    let rpath: Vec<String> = d.rpath.iter().map(|r| r.to_string_lossy().into_owned()).collect();
    assert_eq!(rpath, ["/a", "$ORIGIN/../lib", "/a:/c", "/a"], "every -rpath, as written");
    assert_eq!(d.new_dtags, None);
    assert!(d.runpath(), "--enable-new-dtags is the default");
    for (line, runpath) in [
        (&["--disable-new-dtags"][..], false),
        (&["--enable-new-dtags"], true),
        (&["--enable-new-dtags", "--disable-new-dtags"], false),
        (&["--disable-new-dtags", "--enable-new-dtags"], true),
    ] {
        let mut full = line.to_vec();
        full.push("m.o");
        assert_eq!(ok(&full).plan.dynamic.runpath(), runpath, "{full:?}");
    }
}

// spec: dyn.md D-30 rows 3-4 (-init, -fini: the last wins; without them
// _init and _fini)
#[test]
fn init_and_fini() {
    let parsed = ok(&["m.o"]);
    assert_eq!(parsed.plan.dynamic.init, None);
    assert_eq!(parsed.plan.dynamic.init_symbol(), "_init");
    assert_eq!(parsed.plan.dynamic.fini_symbol(), "_fini");
    let parsed = ok(&["-init", "a", "-init=myinit", "--fini", "f1", "-fini=myfini", "m.o"]);
    let d = &parsed.plan.dynamic;
    assert_eq!(d.init.as_deref(), Some("myinit".as_ref()));
    assert_eq!(d.fini.as_deref(), Some("myfini".as_ref()));
    assert_eq!(d.init_symbol(), "myinit");
    assert_eq!(d.fini_symbol(), "myfini");
    assert!(parsed.plan.other.is_empty(), "{:?}", parsed.plan.other);
}

// spec: dyn.md D-33, D-47, D-49 (-z now, lazy, relro, norelro, text: the
// last of a pair wins)
#[test]
fn z_keywords() {
    let parsed = ok(&["-z", "now", "-z", "norelro", "-ztext", "m.o"]);
    let z = &parsed.plan.z;
    assert_eq!((z.now, z.relro, z.text), (Some(true), Some(false), Some(true)));
    let parsed = ok(&["-z", "now", "-z", "lazy", "-z", "norelro", "-z", "relro", "m.o"]);
    let z = &parsed.plan.z;
    assert_eq!((z.now, z.relro, z.text), (Some(false), Some(true), None));
}

// spec: dyn.md D-20, R-C1 (--hash-style: gnu only; sysv, both and none are
// refused as a value, by name)
#[test]
fn hash_style_is_gnu_only() {
    let parsed = ok(&["--hash-style=gnu", "m.o"]);
    assert_eq!(parsed.plan.dynamic.hash_style, Some(HashStyle::Gnu));
    assert!(parsed.refused_uses().is_empty());
    for style in ["sysv", "both", "none"] {
        let parsed = ok(&["--hash-style", style, "m.o"]);
        assert_eq!(written(&parsed.refused_uses()), [format!("--hash-style={style}")]);
        let r = refusals(&parsed);
        assert_eq!(r.len(), 1);
        assert!(
            r[0].starts_with(&format!("ld: error: unsupported option value: --hash-style={style}")),
            "{r:?}"
        );
        assert!(r[0].contains("mortar writes =gnu"), "{r:?}");
    }
    // A refused value is refused wherever it stands.
    let parsed = ok(&["--hash-style=sysv", "--hash-style=gnu", "m.o"]);
    assert_eq!(written(&parsed.refused_uses()), ["--hash-style=sysv"]);
}

// spec: dyn.md R-C1 (what M2 keeps refused, each by its name, today and at
// the join), D-15, D-25, D-47; WP-040 §6
#[test]
fn m2_refusals_keep_their_names() {
    for (line, name) in [
        (&["-z", "notext"][..], "-z notext"),
        (&["-z", "textoff"], "-z textoff"),
        (&["-z", "nocopyreloc"], "-z nocopyreloc"),
        (&["--spare-dynamic-tags=5"], "--spare-dynamic-tags"),
        (&["--copy-dt-needed-entries"], "--copy-dt-needed-entries"),
        (&["--add-needed"], "--add-needed"),
        // `--dynamic-list` links since WP-128 (D-112; resolve.md §9.6).
        (&["--export-dynamic-symbol-list", "l.txt"], "--export-dynamic-symbol-list"),
        (&["-m", "elf_x86_64", "-z", "dynamic-undefined-weak"], "-z dynamic-undefined-weak"),
        (&["-m", "elf_x86_64", "-z", "nodynamic-undefined-weak"], "-z nodynamic-undefined-weak"),
        (&["-m", "aarch64linux", "-z", "force-bti"], "-z force-bti"),
        (&["-m", "aarch64linux", "-z", "pac-plt"], "-z pac-plt"),
        (&["-m", "elf_x86_64", "-z", "ibtplt"], "-z ibtplt"),
        (&["-m", "elf_x86_64", "-z", "ibt"], "-z ibt"),
        // `-shared` links since WP-123; `-soname` outside it stays refused.
        (&["-soname", "libx.so.1"], "-soname"),
        (&["--pack-dyn-relocs=relr"], "--pack-dyn-relocs"),
        (&["-z", "pack-relative-relocs"], "-z pack-relative-relocs"),
        (&["-Bgroup"], "-Bgroup"),
    ] {
        let mut full = line.to_vec();
        full.push("m.o");
        let parsed = ok(&full);
        let today: Vec<String> = parsed.refused_uses().iter().map(|u| u.spelling.clone()).collect();
        assert_eq!(today, [name], "{full:?}");
        let r = refusals(&parsed);
        assert!(r[0].starts_with(&format!("ld: error: unsupported option: {name}")), "{r:?}");
    }
    // The default of D-15 is accepted.
    let parsed = ok(&["--no-copy-dt-needed-entries", "--no-add-needed", "m.o"]);
    assert!(parsed.refused_uses().is_empty());
    assert!(!parsed.plan.inputs[0].state.copy_dt_needed);
}

// spec: WP-040 step 0e, dyn.md R-C1 (since the M2 join, WP-053, no row of
// a dynamic line refuses, the gates included)
#[test]
fn no_row_of_a_dynamic_line_refuses() {
    let line = [
        "--build-id",
        "--eh-frame-hdr",
        "--hash-style=gnu",
        "--as-needed",
        "-dynamic-linker",
        "/lib/ld-linux-aarch64.so.1",
        "-X",
        "-EL",
        "-maarch64linux",
        "--fix-cortex-a53-843419",
        "-pie",
        "-o",
        "out",
        "Scrt1.o",
        "m.o",
        "-Bstatic",
        "-lstdc++",
        "-Bdynamic",
        "-export-dynamic",
        "-rpath",
        "/x",
        "-z",
        "now",
        "-z",
        "relro",
        "-init",
        "i",
        "-fini",
        "f",
        "--export-dynamic-symbol",
        "s",
        "-z",
        "text",
        "--disable-new-dtags",
        "-lc",
    ];
    let parsed = ok(&line);
    assert_eq!(written(&parsed.refused_uses()), Vec::<String>::new());
}

//! The front's layouts against GNU ld, lld and mold (WP-027a).

mod support;

use mortar_core::par::Pool;
use mortar_testutil::Scratch;
use support::*;

fn explore(label: &str, driver: Driver, flags: &[&str], program: &[Source]) {
    let scratch = Scratch::new(label);
    let Some(built) = build(&scratch, driver, flags, program) else { return };
    let refs = references(&built);
    let store = store();
    let args = with_output(&built.args, &built.dir.join("out.mortar"));
    let f = match front_of(&args, &built.dir, &store, Pool::new(4)) {
        Ok(f) => f,
        Err(r) => panic!("{label}: {}", r.render(&mortar_diag::Style::plain("mortar"))),
    };
    let ours = summarize_front(&f);
    if let Some(dir) = std::env::var_os("DUMP_DIR") {
        let dir = std::path::PathBuf::from(dir).join(label);
        std::fs::create_dir_all(&dir).expect("dump dir");
        std::fs::write(dir.join("mortar.places"), places(&f)).expect("dump");
        let syms: Vec<String> =
            symtab_entries(&f).into_iter().map(|(t, b, x, n)| format!("{t} {b} {x} {n}")).collect();
        std::fs::write(dir.join("mortar.symtab"), syms.join("\n")).expect("dump");
        std::fs::write(dir.join("args"), built.args.join("\n")).expect("dump");
        for e in std::fs::read_dir(&built.dir).expect("read scratch").flatten() {
            let _ = std::fs::copy(e.path(), dir.join(e.file_name()));
        }
    }
    let mut rows: Vec<(&str, &Summary)> = vec![("mortar", &ours)];
    rows.extend(refs.iter().map(|(n, s)| (*n, s)));
    let sl = &f.layout.slots;
    let p = &f.layout.placement;
    let merged: Vec<String> = f
        .order
        .chunks
        .iter()
        .map(|c| {
            format!("{}={:#x}", String::from_utf8_lossy(f.assignment.outsecs[c.out].name), c.size)
        })
        .collect();
    eprintln!("merged chunks: {merged:?}");
    eprintln!(
        "== {label}\n{}slots: got {} plt {} iplt {} tls_desc {} tls_gd {} tls_ie {} tls_ld {}; patches {:?}; rounds {}",
        table(&rows),
        sl.got.len(),
        sl.plt.len(),
        sl.iplt.len(),
        sl.tls_desc.len(),
        sl.tls_gd.len(),
        sl.tls_ie.len(),
        sl.tls_ld,
        p.patches.iter().map(|x| format!("{:?}", x.kind)).collect::<Vec<_>>(),
        f.placed.rounds,
    );
}

#[test]
#[ignore = "exploration"]
fn explore_all() {
    if let Ok(only) = std::env::var("EXPLORE") {
        let all: &[(&str, Driver, &[&str], &[Source])] = &[
            ("gcc-c-O0", Driver::Gcc, &["-O0", "-g"], C_PROGRAM),
            ("gxx-O0", Driver::Gxx, &["-O0", "-g"], CXX_PROGRAM),
            ("gfortran-O0", Driver::Gfortran, &["-O0", "-g"], FORTRAN_PROGRAM),
        ];
        for (l, d, fl, p) in all {
            if *l == only {
                explore(l, *d, fl, p);
            }
        }
        return;
    }
    explore("gcc-c-O0", Driver::Gcc, &["-O0", "-g"], C_PROGRAM);
    explore("gcc-c-O2", Driver::Gcc, &["-O2"], C_PROGRAM);
    explore("gxx-O0", Driver::Gxx, &["-O0", "-g"], CXX_PROGRAM);
    explore("gxx-O2", Driver::Gxx, &["-O2"], CXX_PROGRAM);
    explore("clang-c-O0", Driver::Clang, &["-O0", "-g"], C_PROGRAM);
    explore("clangxx-O2", Driver::Clangxx, &["-O2"], CXX_PROGRAM);
    explore("gfortran-O0", Driver::Gfortran, &["-O0", "-g"], FORTRAN_PROGRAM);
}

/// The allocated output sections of these programs, in L-60's order.
const ALLOC_ORDER: &[&str] = &[
    ".note.gnu.property",
    ".note.gnu.build-id",
    ".note.ABI-tag",
    // The cross C runtime's crt1.o (libc6-dev-<arch>-cross) has it; the
    // native one has not.
    ".note.package",
    ".rela.plt",
    ".rodata",
    ".stapsdt.base",
    ".gcc_except_table",
    ".eh_frame_hdr",
    ".eh_frame",
    ".init",
    ".plt",
    ".text",
    ".fini",
    ".tdata",
    ".tbss",
    ".init_array",
    ".fini_array",
    ".data.rel.ro",
    ".got",
    ".pad.relro",
    ".got.plt",
    ".data",
    ".tm_clone_table",
    ".bss",
];

/// Sections whose size is a function of the input set only (not of
/// order, merging or synthesis): equal to every reference that has them.
const SAME_SIZE: &[&str] = &[
    ".note.gnu.build-id",
    ".note.ABI-tag",
    ".note.package",
    ".gcc_except_table",
    ".stapsdt.base",
    ".tdata",
    ".init_array",
    ".fini_array",
    ".note.stapsdt",
    ".debug_info",
    ".debug_abbrev",
    ".debug_aranges",
    ".debug_line",
    ".debug_rnglists",
    ".debug_loclists",
];

/// The front's layout of one program against GNU ld, lld and mold: each
/// comparison below is one the record (spec/landed/pipeline/WP-027a.md)
/// explains; a difference outside them fails.
#[allow(
    clippy::too_many_lines,
    reason = "one assertion per compared property, in the record's order"
)]
fn compare(label: &str, driver: Driver, flags: &[&str], program: &[Source]) {
    let scratch = Scratch::new(label);
    let Some(built) = build(&scratch, driver, flags, program) else { return };
    let refs = references(&built);
    let get = |n: &str| refs.iter().find(|r| r.0 == n).map(|r| &r.1);
    let (Some(bfd), Some(lld), Some(mold)) = (get("bfd"), get("lld"), get("mold")) else { return };
    let store = store();
    let args = with_output(&built.args, &built.dir.join("out.mortar"));
    let f = match front_of(&args, &built.dir, &store, Pool::new(4)) {
        Ok(f) => f,
        Err(r) => panic!("{label}: {}", r.render(&mortar_diag::Style::plain("mortar"))),
    };
    let ours = summarize_front(&f);
    if label == "cmp-big" {
        // The object's `.text` and `.rela.debug_info` exceed 64 KiB.
        let big = |name: &[u8]| f.headers.iter().any(|(_, h)| h.name == name && h.size > 0x1_0000);
        assert!(big(b".text") && big(b".rela.debug_info"), "{label}");
    }
    let names = |s: &Summary| s.sections.iter().map(|x| x.name.clone()).collect::<Vec<_>>();
    let ctx = format!(
        "{label}\n{}",
        table(&[("mortar", &ours), ("bfd", bfd), ("lld", lld), ("mold", mold)])
    );

    // The set: GNU's, plus `.pad.relro` (layout L-64, L-156; lld and
    // mold write the same padding as `.relro_padding`). An output
    // section whose live members are all empty has no chunk (L-159), so
    // an empty `.tm_clone_table` is dropped as GNU drops it (lld and
    // mold keep it).
    let mut want: Vec<String> = names(bfd);
    want.push(".pad.relro".into());
    let mut got = names(&ours);
    want.sort();
    got.sort();
    assert_eq!(got, want, "{ctx}");

    // The order of the allocated ones: L-60's.
    let alloc: Vec<&str> = ours
        .sections
        .iter()
        .filter(|s| s.flags & SHF_ALLOC != 0)
        .map(|s| s.name.as_str())
        .collect();
    let expected: Vec<&str> = ALLOC_ORDER.iter().copied().filter(|n| alloc.contains(n)).collect();
    assert_eq!(alloc, expected, "{ctx}");
    // Non-allocated: rank 9 in first-appearance order, then L-63's.
    let tail: Vec<&str> = ours.sections.iter().rev().take(3).map(|s| s.name.as_str()).collect();
    assert_eq!(tail, [".shstrtab", ".strtab", ".symtab"], "{ctx}");

    // Sizes that depend on the input set only.
    for name in SAME_SIZE {
        let Some(ours) = ours.section(name) else { continue };
        for (r, s) in [("bfd", bfd), ("lld", lld), ("mold", mold)] {
            if let Some(x) = s.section(name) {
                assert_eq!(ours.size, x.size, "{name} against {r}\n{ctx}");
            }
        }
    }
    // `.eh_frame`: CIE de-duplication and dead-FDE removal as lld and
    // mold (S-50..S-56); GNU keeps a terminator and pads differently.
    for (r, s) in [("lld", lld), ("mold", mold)] {
        assert_eq!(
            ours.section(".eh_frame").map(|x| x.size),
            s.section(".eh_frame").map(|x| x.size),
            "{r}\n{ctx}"
        );
    }
    for (r, s) in [("bfd", bfd), ("lld", lld), ("mold", mold)] {
        assert_eq!((ours.cies, ours.fdes), (s.cies, s.fdes), "CIEs, FDEs against {r}\n{ctx}");
        assert_eq!(ours.irelative, s.irelative, "IRELATIVE against {r}\n{ctx}");
    }
    // `.eh_frame_hdr` only with `--eh-frame-hdr` (Clang passes it to a
    // static link, GCC does not); then one entry per live FDE.
    assert_eq!(ours.hdr_fdes, bfd.hdr_fdes, "{ctx}");
    // `.init`, `.fini`: crti/crtn's prologue and epilogue, as GNU and lld.
    for name in [".init", ".fini"] {
        for s in [bfd, lld] {
            assert_eq!(
                ours.section(name).map(|x| x.size),
                s.section(name).map(|x| x.size),
                "{name}\n{ctx}"
            );
        }
    }
    // IFUNC: GNU's names and shapes (S-67..S-69): `.plt` 32-byte IPLT
    // entries (lld's `.iplt`), `.rela.plt` (lld's `.rela.dyn`), and on
    // AArch64 `.got.plt` one word per entry (lld's; GNU reserves three).
    let size = |s: &Summary, n: &str| s.section(n).map_or(0, |x| x.size);
    assert_eq!(size(&ours, ".plt"), size(bfd, ".plt"), "{ctx}");
    assert_eq!(size(&ours, ".plt"), size(lld, ".iplt"), "{ctx}");
    assert_eq!(size(&ours, ".rela.plt"), size(bfd, ".rela.plt"), "{ctx}");
    assert_eq!(size(&ours, ".rela.plt"), size(lld, ".rela.dyn"), "{ctx}");
    assert_eq!(size(&ours, ".got.plt"), size(lld, ".got.plt"), "{ctx}");
    // `.got`: no header word (no `_GLOBAL_OFFSET_TABLE_` reference, S-65),
    // the `Addr` slots — lld's count, since neither relaxes a GOT load in
    // a static link (a64.md R5 is M2; lld 19 relaxes none here) — then
    // the IE slots, which lld relaxes to LE and mortar keeps at M1
    // (a64.md R3 is M2, synth.md S-72).
    let sl = &f.layout.slots;
    assert_eq!(size(&ours, ".got"), 8 * (sl.got.len() + sl.tls_ie.len()) as u64, "{ctx}");
    assert_eq!(8 * sl.got.len() as u64, size(lld, ".got"), "{ctx}");
    assert!(sl.tls_gd.is_empty() && sl.tls_desc.is_empty() && !sl.tls_ld, "{ctx}");

    // `.symtab`: lld's entries (S-74's order is lld's), less lld's
    // section symbols (S-79: none), its `STB_LOCAL` undefined entries
    // (hidden undefined keys are absent, S-75), its locals in
    // `.gnu.warning*` (consumed, layout L-33), and mapping symbols of
    // `.eh_frame` records S-50 removes.
    let shndx_of = |s: &Summary, n: &str| {
        s.sections.iter().position(|x| x.name == n).map(|i| (i + 1).to_string())
    };
    let warnings: Vec<String> = lld
        .sections
        .iter()
        .enumerate()
        .filter(|(_, x)| x.name.starts_with(".gnu.warning"))
        .map(|(i, _)| (i + 1).to_string())
        .collect();
    let eh = shndx_of(lld, ".eh_frame");
    let mut theirs: Vec<&str> = Vec::new();
    let mut eh_mapping = 0usize;
    for (typ, bind, ndx, name) in &lld.entries {
        if typ == "SECTION" || (bind == "LOCAL" && ndx == "UND") || warnings.contains(ndx) {
            continue;
        }
        if Some(ndx) == eh.as_ref() && name.starts_with('$') {
            eh_mapping += 1;
            continue;
        }
        theirs.push(name);
    }
    let entries = symtab_entries(&f);
    let mut mine: Vec<&str> = Vec::new();
    let mut my_eh_mapping = 0usize;
    for (_, _, shndx, name) in &entries {
        if *shndx == 2 && name.starts_with('$') {
            my_eh_mapping += 1;
            continue;
        }
        mine.push(name);
    }
    theirs.sort_unstable();
    mine.sort_unstable();
    assert!(mine == theirs, "mortar − lld: {}\n{ctx}", multiset_diff(&mine, &theirs));
    // lld keeps mapping symbols of removed `.eh_frame` records that
    // mortar drops (S-76): at most 4 with the native runtime. With
    // Debian's cross AArch64 runtime (an x86-64 host; told by the
    // runtime's path, as in tests/gc.rs) lld keeps 13 more. Suspected:
    // COMDAT keeper differences (resolve.md §6 ruling C) leave different
    // input `.eh_frame` sections without records (S-50, S-76); not
    // proven. The looser bound leans on the same records being kept,
    // asserted again here: `.eh_frame`'s size and the CIE and FDE counts
    // are lld's.
    let cross = built.args.iter().any(|a| a.contains("/gcc-cross/"));
    let gap = eh_mapping.checked_sub(my_eh_mapping);
    eprintln!("{label}: .eh_frame mapping symbols: mortar {my_eh_mapping}, lld {eh_mapping}");
    if cross {
        assert_eq!(
            ours.section(".eh_frame").map(|x| x.size),
            lld.section(".eh_frame").map(|x| x.size),
            "{ctx}"
        );
        assert_eq!((ours.cies, ours.fdes), (lld.cies, lld.fdes), "{ctx}");
    }
    let bound = if cross { 16 } else { 4 };
    assert!(
        gap.is_some_and(|g| g <= bound),
        "{my_eh_mapping} {eh_mapping} (cross runtime: {cross})\n{ctx}"
    );
    assert_eq!(ours.symtab_entries, entries.len() as u64 + 1, "{ctx}");

    // Segments: L-65..L-67, and GNU's base (L-68).
    let segs: Vec<String> =
        ours.segments.iter().map(|g| format!("{}:{}", g.p_type, g.flags)).collect();
    let mut want = vec!["LOAD:R", "LOAD:RE", "LOAD:RW", "LOAD:RW", "NOTE:R", "TLS:R"];
    if ours.hdr_fdes.is_some() {
        want.push("GNU_EH_FRAME:R");
    }
    want.extend(["GNU_STACK:RW", "GNU_RELRO:R"]);
    assert_eq!(segs, want, "{ctx}");
    let base = |s: &Summary| s.segments.iter().find(|g| g.p_type == "LOAD").map(|g| g.vaddr);
    assert_eq!(base(&ours), base(bfd), "{ctx}");
    assert_eq!(base(&ours), Some(0x40_0000), "{ctx}");
    // The entry is `_start`'s address.
    assert_eq!(Some(ours.entry), global_addr(&f, "_start"), "{ctx}");
}

// spec: layout.md §8-§13, synth.md §3-§7 (the front's layout of real
// static programs against the three references; VALIDATION.md L4)
#[test]
fn gcc_c_against_references() {
    compare("cmp-gcc-c-O0", Driver::Gcc, &["-O0", "-g"], C_PROGRAM);
    compare("cmp-gcc-c-O2", Driver::Gcc, &["-O2"], C_PROGRAM);
}

#[test]
fn gxx_against_references() {
    compare("cmp-gxx-O0", Driver::Gxx, &["-O0", "-g"], CXX_PROGRAM);
    compare("cmp-gxx-O2", Driver::Gxx, &["-O2"], CXX_PROGRAM);
}

#[test]
fn clang_against_references() {
    compare("cmp-clang-c-O0", Driver::Clang, &["-O0", "-g"], C_PROGRAM);
    compare("cmp-clangxx-O2", Driver::Clangxx, &["-O2"], CXX_PROGRAM);
}

#[test]
fn gfortran_against_references() {
    compare("cmp-gfortran-O0", Driver::Gfortran, &["-O0", "-g"], FORTRAN_PROGRAM);
}

/// A C program whose object has `.text` and `.rela.debug_info` over the
/// staged store's 64 KiB (`spec/landed/input/WP-021b.md`: neither is
/// resident), so the scan, the erratum words (`Sites::code`) and the
/// rest read them through `copy_to`.
fn big_program() -> &'static [Source] {
    use std::fmt::Write;
    let mut text = String::from("#include <stdio.h>\n");
    for i in 0..2000 {
        let _ = writeln!(text, "int f{i}(int x) {{ int y{i} = x * {i}; return y{i} + {i}; }}");
    }
    text.push_str("int main(int c, char **v) { int s = 0;\n");
    for i in (0..2000).step_by(50) {
        let _ = writeln!(text, "  s += f{i}(c);");
    }
    text.push_str("  printf(\"%d\\n\", s); return 0; }\n");
    Box::leak(Box::new([Source { name: "big.c", text: Box::leak(text.into_boxed_str()) }]))
}

// spec: input.md §11.1 and WP-021b (non-resident bytes are read, never
// taken as empty), layout.md §14 (the erratum scan reads every code word)
#[test]
fn sections_over_the_staged_limits() {
    compare("cmp-big", Driver::Gcc, &["-O0", "-g"], big_program());
}

// spec: model.md §6 (the result does not depend on the schedule: thread
// count and MORTAR_SCHED_SEED), docs/ARCHITECTURE.md (deterministic
// output): the same link under every test schedule gives the same
// `Layout`, sections, symbol table and report.
#[test]
fn the_front_does_not_depend_on_the_schedule() {
    for (label, driver, flags, program) in [
        ("det-gxx-O2", Driver::Gxx, &["-O2"][..], CXX_PROGRAM),
        ("det-big", Driver::Gcc, &["-O0", "-g"][..], big_program()),
    ] {
        let scratch = Scratch::new(label);
        let Some(built) = build(&scratch, driver, flags, program) else { return };
        let store = store();
        let args = with_output(&built.args, &built.dir.join("out.mortar"));
        mortar_core::par::check_schedules(|pool| {
            let parsed = mortar_cli::parse(&args);
            let diag = mortar_diag::Collector::new(mortar_diag::Policy::default());
            let f = mortar::front::front(
                parsed.plan,
                mortar::front::Env { cwd: &built.dir, pool: *pool, replay: None, slack: false },
                &store,
                &diag,
                &mut mortar::timing::Timer::new(false),
            );
            let report = diag.finish().render(&mortar_diag::Style::plain("mortar"));
            let f = f.unwrap_or_else(|| panic!("{label}: {report}"));
            (f.layout.clone(), f.sections(), symtab_entries(&f), report)
        });
    }
}

// spec: cli.md §3 (the rows WP-027a made Supported do what their row
// says), synth.md S-84 (-s: no symbol table), S-78 (-x/-X), S-57
// (.eh_frame_hdr only with --eh-frame-hdr), S-91 (build-id sizes),
// layout.md L-102/L-103 (-e; an undefined entry is none), image.md I-5
// (--no-gc-sections is accepted)
#[test]
fn the_supported_rows_shape_the_front() {
    let scratch = Scratch::new("rows");
    let Some(built) = build(&scratch, Driver::Gcc, &["-O2", "-g"], C_PROGRAM) else { return };
    let store = store();
    let base = with_output(&built.args, &built.dir.join("out.mortar"));
    let run = |extra: &[&str]| {
        let mut args = base.clone();
        args.extend(extra.iter().map(std::ffi::OsString::from));
        front_of(&args, &built.dir, &store, Pool::new(2)).unwrap_or_else(|r| {
            panic!("{extra:?}: {}", r.render(&mortar_diag::Style::plain("mortar")))
        })
    };
    let names = |f: &mortar::front::Front<'_>| -> Vec<String> {
        f.sections().iter().map(|r| String::from_utf8_lossy(&r.name).into_owned()).collect()
    };
    let has = |f: &mortar::front::Front<'_>, n: &str| names(f).iter().any(|x| x == n);
    let locals =
        |f: &mortar::front::Front<'_>| symtab_entries(f).iter().filter(|e| e.1 == 0).count();

    let plain = run(&[]);
    assert!(has(&plain, ".symtab") && has(&plain, ".strtab") && has(&plain, ".debug_info"));
    assert!(!has(&plain, ".eh_frame_hdr"), "gcc's static line has no --eh-frame-hdr");
    assert_eq!(
        plain.synth.build_id.as_ref().map(mortar_synth::BuildIdPlan::desc_size),
        Some(20),
        "bare --build-id"
    );
    assert_eq!(plain.layout.placement.entry, global_addr(&plain, "_start"));

    let s = run(&["-s"]);
    assert!(s.synth.symtab.is_none() && !has(&s, ".symtab") && !has(&s, ".strtab"));
    assert!(!has(&s, ".debug_info"), "-s strips debugging sections too");
    let big_s = run(&["-S"]);
    assert!(
        has(&big_s, ".symtab") && !names(&big_s).iter().any(|n| n.starts_with(".debug")),
        "{:?}",
        names(&big_s)
    );

    // gcc's line has -X; a later -x drops every local but the file and
    // section ones the table keeps (S-78), a later --discard-none none.
    let x = run(&["-x"]);
    assert!(locals(&x) < locals(&plain), "{} {}", locals(&x), locals(&plain));
    // Under -x the locals are exactly lld's: the demoted hidden globals
    // and the internal file's (S-77), no FILE, mapping or `.L` symbol;
    // lld's LOCAL UND entries (S-75) and section symbols (S-79) are absent.
    if let Some(lld) = reference_with(&built, "ld.lld", &["-x"]) {
        let mut theirs: Vec<&str> = lld
            .entries
            .iter()
            .filter(|(t, b, n, _)| b == "LOCAL" && n != "UND" && t != "SECTION")
            .map(|e| e.3.as_str())
            .collect();
        let entries = symtab_entries(&x);
        let mut mine: Vec<&str> =
            entries.iter().filter(|e| e.1 == 0).map(|e| e.3.as_str()).collect();
        theirs.sort_unstable();
        mine.sort_unstable();
        assert!(mine == theirs, "-x: mortar − lld: {}", multiset_diff(&mine, &theirs));
    }

    let hdr = run(&["--eh-frame-hdr"]);
    assert!(hdr.synth.hdr.is_some() && has(&hdr, ".eh_frame_hdr"));
    let no_hdr = run(&["--eh-frame-hdr", "--no-eh-frame-hdr"]);
    assert!(no_hdr.synth.hdr.is_none() && !has(&no_hdr, ".eh_frame_hdr"));

    assert_eq!(
        run(&["--build-id=sha256"])
            .synth
            .build_id
            .as_ref()
            .map(mortar_synth::BuildIdPlan::desc_size),
        Some(32)
    );
    assert_eq!(
        run(&["--build-id=0x0102ab"])
            .synth
            .build_id
            .as_ref()
            .map(mortar_synth::BuildIdPlan::desc_size),
        Some(3)
    );
    assert!(run(&["--build-id=none"]).synth.build_id.is_none());

    assert_eq!(names(&run(&["--no-gc-sections"])), names(&plain));

    // R-C3: `-z memory-seal` adds S-95's MEMORY_SEAL property (one 8-byte
    // property, pr_type 3, no data, padded to 8); the last of the pair wins.
    let size = |f: &mortar::front::Front<'_>| f.synth.property.size();
    let sealed = run(&["-z", "memory-seal"]);
    assert_eq!(size(&sealed), if size(&plain) == 0 { 16 + 8 } else { size(&plain) + 8 });
    assert_eq!(size(&run(&["-z", "memory-seal", "-z", "nomemory-seal"])), size(&plain));
    let none = run(&["--discard-none"]);
    assert!(locals(&none) >= locals(&plain), "--discard-none keeps the -X locals too");
    assert!(
        symtab_entries(&none).iter().any(|e| e.1 == 0 && e.3.starts_with(".L"))
            || locals(&none) == locals(&plain)
    );

    let e = run(&["-e", "main"]);
    assert_eq!(e.layout.placement.entry, global_addr(&e, "main"));
    assert!(e.layout.placement.entry.is_some());
    assert_eq!(run(&["-e", "no_such_entry"]).layout.placement.entry, None, "L-103: e_entry = 0");
}

// spec: synth.md S-6 (a `Merge` section that is writable, code, TLS,
// relocated or of an odd shape is placed whole, as `Regular`; resolve
// demotes it, so phase 60 never cuts it), S-5 (the others are split)
#[test]
fn s6_unsafe_merge_sections_are_placed_whole() {
    const SOURCES: &[Source] = &[
        Source {
            name: "m.s",
            text: "\t.section .data.m,\"awM\",@progbits,4\n\t.word 7\n\t.word 7\n\
                   \t.section .rodata.r,\"aM\",@progbits,8\n\t.xword 0\n\t.xword msg\n\
                   \t.section .rodata.k,\"aM\",@progbits,4\n\t.word 9\n\t.word 9\n\
                   \t.section .rodata.s,\"aMS\",@progbits,1\nmsg:\t.asciz \"s6\"\n",
        },
        Source { name: "main.c", text: "int main(void) { return 0; }\n" },
    ];
    let scratch = Scratch::new("s6");
    let Some(built) = build(&scratch, Driver::Gcc, &["-O2"], SOURCES) else { return };
    let store = store();
    let args = with_output(&built.args, &built.dir.join("out.mortar"));
    let f = front_of(&args, &built.dir, &store, Pool::new(2))
        .unwrap_or_else(|r| panic!("{}", r.render(&mortar_diag::Style::plain("mortar"))));
    let placed = places(&f);
    let whole = |name: &str| placed.lines().any(|l| l.split(' ').nth(2) == Some(name));
    assert!(whole(".data.m"), "writable: placed whole\n{placed}");
    assert!(whole(".rodata.r"), "relocated: placed whole\n{placed}");
    assert!(!whole(".rodata.k") && !whole(".rodata.s"), "the others are split\n{placed}");
    let kind = |name: &str| {
        f.resolution
            .sections
            .iter()
            .find(|(sid, _)| f.headers[*sid].name == name.as_bytes())
            .map(|(_, row)| row.kind)
    };
    assert_eq!(kind(".data.m"), Some(mortar_core::resolution::SectionKind::Regular));
    assert_eq!(kind(".rodata.k"), Some(mortar_core::resolution::SectionKind::Merge));
}

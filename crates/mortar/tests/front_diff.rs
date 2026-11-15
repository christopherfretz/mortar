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
        .map(|c| format!("{}={:#x}", String::from_utf8_lossy(f.assignment.outsecs[c.out].name), c.size))
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
    ".relro_padding",
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
    let names = |s: &Summary| s.sections.iter().map(|x| x.name.clone()).collect::<Vec<_>>();
    let ctx = format!("{label}\n{}", table(&[("mortar", &ours), ("bfd", bfd), ("lld", lld), ("mold", mold)]));

    // The set: GNU's, plus `.relro_padding` (layout L-64, as lld and
    // mold) and an empty `.tm_clone_table` (L-43: an output section with
    // a live member has a chunk; GNU drops empty ones, lld and mold keep
    // it).
    let mut want: Vec<String> = names(bfd);
    want.push(".relro_padding".into());
    if lld.section(".tm_clone_table").is_some() && !want.iter().any(|n| n == ".tm_clone_table") {
        want.push(".tm_clone_table".into());
    }
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
        assert_eq!(ours.section(".eh_frame").map(|x| x.size), s.section(".eh_frame").map(|x| x.size), "{r}\n{ctx}");
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
            assert_eq!(ours.section(name).map(|x| x.size), s.section(name).map(|x| x.size), "{name}\n{ctx}");
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
    assert!(my_eh_mapping <= eh_mapping && eh_mapping - my_eh_mapping <= 4, "{my_eh_mapping} {eh_mapping}\n{ctx}");
    assert_eq!(ours.symtab_entries, entries.len() as u64 + 1, "{ctx}");

    // Segments: L-65..L-67, and GNU's base (L-68).
    let segs: Vec<String> = ours.segments.iter().map(|g| format!("{}:{}", g.p_type, g.flags)).collect();
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

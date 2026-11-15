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

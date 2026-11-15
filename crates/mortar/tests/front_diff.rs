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
    let mut rows: Vec<(&str, &Summary)> = vec![("mortar", &ours)];
    rows.extend(refs.iter().map(|(n, s)| (*n, s)));
    eprintln!("== {label}\n{}", table(&rows));
}

#[test]
#[ignore = "exploration"]
fn explore_all() {
    explore("gcc-c-O0", Driver::Gcc, &["-O0", "-g"], C_PROGRAM);
    explore("gcc-c-O2", Driver::Gcc, &["-O2"], C_PROGRAM);
    explore("gxx-O0", Driver::Gxx, &["-O0", "-g"], CXX_PROGRAM);
    explore("gxx-O2", Driver::Gxx, &["-O2"], CXX_PROGRAM);
    explore("clang-c-O0", Driver::Clang, &["-O0", "-g"], C_PROGRAM);
    explore("clangxx-O2", Driver::Clangxx, &["-O2"], CXX_PROGRAM);
    explore("gfortran-O0", Driver::Gfortran, &["-O0", "-g"], FORTRAN_PROGRAM);
}

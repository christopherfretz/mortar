mod common;

#[test]
fn smoke_gxx() {
    let Some(gxx) = common::need("g++") else { return };
    let s = mortar_testutil::Scratch::new("smoke");
    s.write("m.cc", "int main(){return 0;}\n");
    let o = common::tool_run(&gxx, &["-c".into(), "m.cc".into()], s.path());
    assert!(o.success());
    let args = common::driver_link_args(&gxx, &["m.o"], s.path()).unwrap();
    let out = common::run_closure(s.path(), &args);
    eprintln!("{:#?}", common::report(&out));
    eprintln!("{:#?}", common::opened(&out));
    eprintln!("{:#?}", common::names(&out).len());
    eprintln!("waves {}", out.waves);
    panic!();
}

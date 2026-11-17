//! Stage R (`spec/plumb.md` §14): the verifier of `-r` outputs.

use mortar_plumb::{Ctx, Format, HostFiles, Input};

fn probe_one(dir: &str, case: &str, linker: &str, format: Format, ins: &[&str]) {
    let out = std::fs::read(format!("{dir}/{case}.{linker}.o")).unwrap();
    let text = std::fs::read_to_string(format!("{dir}/{case}.{linker}.map")).unwrap();
    let map = mortar_plumb::map::parse(format, &text).unwrap();
    let files: Vec<(String, Vec<u8>)> =
        map.paths().into_iter().map(|p| (p.to_string(), std::fs::read(format!("{dir}/{p}")).unwrap())).collect();
    let inputs: Vec<Input<'_>> = files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
    let mut line: Vec<String> = vec!["-r".into()];
    line.extend(ins.iter().map(|s| s.to_string()));
    let ctx = Ctx { line: Some(line), files: &HostFiles };
    let r = mortar_plumb::verify_with(&out, &inputs, &map, &ctx);
    println!("==== {case}.{linker}: {} findings", r.findings.len());
    for f in &r.findings {
        println!("  {} {} {:?}", f.rule.id(), f.message, f.site.as_ref().map(|s| (&s.file, &s.section, s.offset)));
    }
    println!("{}", r.summary());
}

#[test]
#[ignore = "a probe over RL5_PROBE's samples"]
fn probe() {
    let Ok(dir) = std::env::var("RL5_PROBE") else { return };
    for (case, ins) in [("c", ["c1.o", "c2.o"]), ("x", ["x1.o", "x2.o"])] {
        for (l, f) in [("bfd", Format::Gnu), ("lld", Format::Lld), ("mold", Format::Mold)] {
            probe_one(&dir, case, l, f, &ins);
        }
    }
}

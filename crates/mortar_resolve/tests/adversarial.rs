//! The review's adversarial cases (`spec/landed/resolve/WP-022-spec.md`:
//! t1, t1s, t2, t2b, t10, t12, E1, E2, R2-R8, R12, R14), re-created from
//! their sources with the native GCC on AArch64, read as real objects and
//! archives. Each case checks mortar's outcome as the record states it
//! (members, definitions, diagnostics), and that the simulation of GNU ld
//! (§4.6) loads the members `ld.bfd -t -t` loads (§15.1
//! `s4_6_simulation_matches_bfd`).

mod support;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use mortar_core::par::Pool;
use mortar_core::resolution::FileLive;
use mortar_core::{FileId, OptId};
use mortar_elf::Machine;
use mortar_resolve::options::{Entry, Named, Options};
use mortar_resolve::view::{FileKind, View};
use mortar_resolve::{Def, Resolved, resolve};
use mortar_testutil::Scratch;
use support::{Model, canon, exec, ok, parse_trace};

const SOURCES: &[(&str, &str)] = &[
    ("mk.c", "int K(void); int main(void){return K();}"),
    ("mkw.c", "__attribute__((weak)) int K(void); int main(void){return K ? K() : 7;}"),
    ("ks.c", "int K(void){return 2;}"),
    ("kw.c", "__attribute__((weak)) int K(void){return 1;}"),
    ("t1s.c", "int K(void){return 3;}"),
    ("uk.c", "int K(void); int user2(void){return K();}"),
    ("m0.c", "int main(void){return 0;}"),
    ("mc.c", "int v; int main(void){return v;}"),
    ("mv.c", "extern int v; int main(void){return v;}"),
    ("lv.c", "int v = 9;"),
    ("lvc.c", "int v; int other(void){return 3;}"),
    ("lvf.c", "int v(void){return 9;}"),
    ("wv.c", "__attribute__((weak)) int v = 4;"),
    ("fv.c", "int v(void){return 4;}"),
    ("m2.c", "int b(void); int main(void){return b();}"),
    ("pa.c", "int pick(void){return 1;}"),
    ("pb.c", "int pick(void); int b(void){return pick();}"),
    ("pc.c", "int pick(void){return 3;}"),
    // E1
    ("a.c", "int pick(void){return 1;}"),
    ("c.c", "int pick(void){return 2;}"),
    ("b.c", "int pick(void); int user(void){return pick();}"),
    ("a2.c", "int dup = 7; int pick(void){return 11;}"),
    ("main.c", "int user(void); int dup = 5; int main(void){return user();}"),
    // E2
    ("m1.c", "int K(void){return 1;}"),
    ("m2k.c", "int K(void){return 2;} int J(void){return 100;}"),
    ("n.c", "int K(void); int U(void){return K()*10;}"),
    ("main1.c", "int U(void); int J(void); int main(void){return U()+J();}"),
    // t10
    ("m10.c", "int g(void); int hx(void); int main(void){return g()+hx();}"),
    // R12
    (
        "mv2.c",
        "__asm__(\".symver foo_v2, foo@V2\"); int foo_v2(void); int main(void){return foo_v2();}",
    ),
    ("mf.c", "int foo(void); int main(void){return foo();}"),
    ("dloc.c", "__asm__(\".symver foo_impl, foo@@V2\"); int foo_impl(void){return 9;}"),
    ("dv.c", "__asm__(\".symver foo_impl, foo@V2\"); int foo_impl(void){return 4;}"),
];

const ASM: &[(&str, &str)] = &[
    ("start.s", ".text\n.globl _start\n_start: bl main\n mov x8, #93\n svc #0\n"),
    (
        "st.s",
        ".text\n.globl _start\n_start: bl main\n mov x8, #93\n svc #0\n.globl marker\nmarker: ret\n",
    ),
    (
        "gl.s",
        ".section .text.g,\"axG\",%progbits,g,comdat\n.globl g\n.type g,%function\ng: mov w0, #40\n ret\n",
    ),
    (
        "gh.s",
        ".section .text.g,\"axG\",%progbits,g,comdat\n.globl g\n.type g,%function\ng: mov w0, #40\n ret\n\
         .globl hx\n.type hx,%function\nhx: mov w0, #100\n ret\n",
    ),
    (
        "mt.s",
        ".text\n.globl main\nmain: mrs x0, tpidr_el0\n add x0, x0, #:tprel_hi12:x\n \
         add x0, x0, #:tprel_lo12_nc:x\n ret\n",
    ),
    ("mn.s", ".data\n.quad x\n"),
    ("m.s", ".text\n.globl main\nmain: ret\n.data\n.quad x\n"),
    ("mo.s", ".text\n.globl main\nmain: ret\n.type x,%object\n.data\n.quad x\n"),
    ("t.s", ".section .tdata,\"awT\",%progbits\n.globl x\n.type x,%object\nx: .word 1\n"),
    ("hz.s", ".text\n.globl main\nmain: ret\n.hidden zz\n.globl zz\n"),
];

/// Archives: name, members (objects), `ar` flags.
const ARCHIVES: &[(&str, &[&str], &str)] = &[
    ("libt1k.a", &["ks.o"], "rcs"),
    ("libKS.a", &["ks.o"], "rcs"),
    ("libKW.a", &["kw.o"], "rcs"),
    ("libLV.a", &["lv.o"], "rcs"),
    ("libLC.a", &["lvc.o"], "rcs"),
    ("libLF.a", &["lvf.o"], "rcs"),
    ("libPA.a", &["pa.o"], "rcs"),
    ("libPCB.a", &["pc.o", "pb.o"], "rcs"),
    ("libPAB.a", &["pa.o", "pb.o"], "rcs"),
    ("libPC.a", &["pc.o"], "rcs"),
    ("libst.a", &["st.o"], "rcs"),
    ("libG.a", &["gl.o", "gh.o"], "rcs"),
    ("libnoidx.a", &["ks.o"], "rcS"),
    ("libA.a", &["a.o"], "rcs"),
    ("libB.a", &["b.o"], "rcs"),
    ("libC.a", &["c.o"], "rcs"),
    ("libABC.a", &["a.o", "b.o", "c.o"], "rcs"),
    ("libBAC.a", &["b.o", "a.o", "c.o"], "rcs"),
    ("libA2.a", &["a2.o"], "rcs"),
    ("libX.a", &["m1.o"], "rcs"),
    ("libY.a", &["n.o", "m2k.o"], "rcs"),
    ("lib1.a", &["m1.o"], "rcs"),
    ("lib2.a", &["m2k.o", "n.o"], "rcs"),
    ("lib2r.a", &["n.o", "m2k.o"], "rcs"),
];

struct Corpus {
    dir: Scratch,
    bfd: PathBuf,
}

fn corpus() -> Option<Corpus> {
    // The sources are AArch64's: its toolchain on every host.
    let a64 = mortar_testutil::target::Arch::Aarch64;
    let gcc = a64.tool("gcc")?;
    let ar = a64.tool("ar")?;
    let bfd = a64.tool("ld.bfd")?;
    let dir = Scratch::new("resolve-adv");
    for (name, text) in SOURCES.iter().chain(ASM) {
        let src = dir.write(name, text);
        let obj = dir.join(format!("{}.o", &name[..name.len() - 2])).display().to_string();
        let out = exec(
            &gcc,
            &[
                "-c".into(),
                "-O1".into(),
                "-fcommon".into(),
                src.display().to_string(),
                "-o".into(),
                obj,
            ],
            dir.path(),
        );
        ok(&out, name);
    }
    for (name, members, flags) in ARCHIVES {
        let mut args = vec![(*flags).to_owned(), dir.join(name).display().to_string()];
        args.extend(members.iter().map(|m| dir.join(m).display().to_string()));
        ok(&exec(&ar, &args, dir.path()), name);
    }
    Some(Corpus { dir, bfd })
}

/// The outcome of one case.
struct Run {
    m: Model,
    r: Resolved,
    members: BTreeSet<String>,
    sim: BTreeSet<String>,
    bfd: Option<BTreeSet<String>>,
}

fn run(c: &Corpus, line: &[&str]) -> Run {
    let dir = c.dir.path();
    let mut m = Model::new(Machine::AArch64);
    let mut whole = false;
    let mut group = None;
    let mut groups = 0;
    let mut undefined = Vec::new();
    let mut args: Vec<String> = vec!["-static".into(), "-t".into(), "-t".into()];
    // t12 brings its own _start.
    if !line.contains(&"libst.a") {
        let start = dir.join("start.o");
        args.push(start.display().to_string());
        m.load_object(
            &std::fs::read(&start).unwrap(),
            &canon(&start.display().to_string()),
            FileKind::Object,
        )
        .unwrap();
    }
    let mut it = line.iter();
    while let Some(&a) = it.next() {
        let path = dir.join(a);
        match a {
            "--whole-archive" => whole = true,
            "--no-whole-archive" => whole = false,
            "--start-group" => {
                group = Some(groups);
                groups += 1;
            }
            "--end-group" => group = None,
            "-u" => {
                let name = *it.next().unwrap();
                undefined.push(name);
                args.push("-u".into());
                args.push(name.into());
                continue;
            }
            _ if Path::new(a).extension().is_some_and(|e| e == "a") => {
                let label = canon(&path.display().to_string());
                for f in m.load_archive(&path, whole, group).unwrap() {
                    m.files[f.0 as usize].label.clone_from(&label);
                }
            }
            _ => {
                let label = canon(&path.display().to_string());
                let f = m
                    .load_object(&std::fs::read(&path).unwrap(), &label, FileKind::Object)
                    .unwrap();
                if let Some(g) = group {
                    m.set_group(f, g);
                }
            }
        }
        args.push(if a.starts_with('-') { a.to_owned() } else { path.display().to_string() });
    }
    let undefined: Vec<Named> = undefined
        .iter()
        .enumerate()
        .map(|(i, u)| Named {
            key: OptId::some(m.key(u)),
            name: u.as_bytes().into(),
            arg: i as u32 + 1,
        })
        .collect();
    let opts = Options {
        entry: Entry::Default(OptId::some(m.key("_start"))),
        undefined,
        warn_backrefs: true,
        ..Options::default()
    };
    let r = mortar_core::par::check_schedules(|pool: &Pool| resolve(&m, &opts, pool));
    let members = (0..m.files() as usize)
        .filter(|&f| {
            matches!(r.file_live[f], FileLive::Extracted(_))
                || m.files[f].info.kind == FileKind::WholeMember
        })
        .map(|f| short(&m.label(FileId(f as u32))))
        .collect();
    // ld.bfd -t -t lists --whole-archive members too.
    let sim = r
        .gnu
        .as_ref()
        .unwrap()
        .loaded
        .iter()
        .copied()
        .chain(
            (0..m.files())
                .map(FileId)
                .filter(|&f| m.files[f.0 as usize].info.kind == FileKind::WholeMember),
        )
        .map(|f| short(&m.label(f)))
        .collect();
    args.push("-o".into());
    args.push(dir.join("out").display().to_string());
    let out = exec(&c.bfd, &args, dir);
    let bfd =
        out.success().then(|| parse_trace(&out.stdout_text()).iter().map(|s| short(s)).collect());
    Run { m, r, members, sim, bfd }
}

/// `lib.a(member)` without the directory.
fn short(label: &str) -> String {
    Path::new(label.split('(').next().unwrap_or(label)).file_name().map_or_else(
        || label.to_owned(),
        |f| format!("{}({}", f.to_string_lossy(), label.split_once('(').map_or("", |x| x.1)),
    )
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

fn ids(r: &Resolved) -> Vec<&'static str> {
    r.problems.iter().map(mortar_resolve::Problem::id).collect()
}

fn def_file(run: &Run, name: &str) -> Option<String> {
    let k = run.m.try_id(name)?;
    run.r.def[k.0 as usize].entry().map(|e| short(&run.m.label(e.file)))
}

/// One case: the line, mortar's members, the diagnostics' ids, and
/// whether the simulation must equal `ld.bfd` (when it links).
fn case(c: &Corpus, name: &str, line: &[&str], members: &[&str], diags: &[&str]) -> Run {
    let run = run(c, line);
    let got_ids = ids(&run.r);
    println!(
        "| {name} | {} | mortar {:?} | simulation {:?} | bfd {:?} | {:?} |",
        line.join(" "),
        run.members,
        run.sim,
        run.bfd,
        got_ids
    );
    assert_eq!(run.members, set(members), "{name}: members");
    let mut want: Vec<&str> = diags.to_vec();
    want.sort_unstable();
    let mut got = got_ids.clone();
    got.sort_unstable();
    assert_eq!(got, want, "{name}: diagnostics {:?}", run.r.problems);
    if let Some(bfd) = &run.bfd {
        assert_eq!(&run.sim, bfd, "{name}: simulation vs ld.bfd -t -t");
    }
    run
}

#[test]
fn s4_6_simulation_matches_bfd() {
    let Some(c) = corpus() else { return };
    // t1 / R1: a weak definition after the archive does not block.
    let t1 = case(&c, "t1", &["mk.o", "libt1k.a", "kw.o"], &["libt1k.a(ks.o)"], &[]);
    assert_eq!(def_file(&t1, "K").as_deref(), Some("libt1k.a(ks.o)"));
    case(&c, "t1s", &["mk.o", "libt1k.a", "t1s.o"], &["libt1k.a(ks.o)"], &["duplicate-symbol"]);
    // t2 / t2b (finding 3): GNU ld takes c.o; mortar a.o.
    let t2 = case(
        &c,
        "t2",
        &["m2.o", "libPA.a", "libPCB.a"],
        &["libPA.a(pa.o)", "libPCB.a(pb.o)"],
        &["archive-choice"],
    );
    assert_eq!(t2.sim, set(&["libPCB.a(pb.o)", "libPCB.a(pc.o)"]));
    case(&c, "t2b", &["m2.o", "libPAB.a", "libPC.a"], &["libPAB.a(pa.o)", "libPAB.a(pb.o)"], &[]);
    // t10: keeper gl.o; hx only in the dropped copy.
    let t10 = case(&c, "t10", &["m10.o", "libG.a"], &["libG.a(gl.o)", "libG.a(gh.o)"], &[]);
    let hx = t10.m.id("hx");
    assert_eq!(t10.r.def[hx.0 as usize], Def::Undefined { weak: false });
    assert_eq!(t10.r.comdat_mismatch.len(), 1);
    assert_eq!(short(&t10.m.label(t10.r.comdat_mismatch[0].keeper)), "libG.a(gl.o)");
    // t12 / R9: the default _start is a root.
    let t12 = case(&c, "t12", &["m0.o", "libst.a"], &["libst.a(st.o)"], &[]);
    let _ = t12;
    // E1.
    for (n, line) in [
        ("E1 o1", &["main.o", "libA.a", "libB.a", "libC.a"][..]),
        ("E1 o2", &["main.o", "--start-group", "libA.a", "libB.a", "libC.a", "--end-group"][..]),
    ] {
        let r = case(&c, n, line, &["libA.a(a.o)", "libB.a(b.o)"], &["archive-choice"]);
        assert_eq!(def_file(&r, "pick").as_deref(), Some("libA.a(a.o)"));
    }
    case(
        &c,
        "E1 o3",
        &["main.o", "libABC.a"],
        &["libABC.a(a.o)", "libABC.a(b.o)"],
        &["archive-choice"],
    );
    case(&c, "E1 o5", &["main.o", "libBAC.a"], &["libBAC.a(a.o)", "libBAC.a(b.o)"], &[]);
    case(
        &c,
        "E1 o4",
        &["main.o", "libA2.a", "libB.a", "libC.a"],
        &["libA2.a(a2.o)", "libB.a(b.o)"],
        &["duplicate-symbol", "archive-choice"],
    );
    // E2: mortar reports the duplicate K on all three lines, as mold.
    for (n, a, b, members) in [
        ("E2 libX libY", "libX.a", "libY.a", ["libX.a(m1.o)", "libY.a(m2k.o)", "libY.a(n.o)"]),
        ("E2 lib1 lib2", "lib1.a", "lib2.a", ["lib1.a(m1.o)", "lib2.a(m2k.o)", "lib2.a(n.o)"]),
        ("E2 lib1 lib2r", "lib1.a", "lib2r.a", ["lib1.a(m1.o)", "lib2r.a(m2k.o)", "lib2r.a(n.o)"]),
    ] {
        let r = run(&c, &["main1.o", a, b]);
        println!(
            "| {n} | mortar {:?} | simulation {:?} | bfd {:?} | {:?} |",
            r.members,
            r.sim,
            r.bfd,
            ids(&r.r)
        );
        assert_eq!(r.members, set(&members), "{n}");
        assert!(ids(&r.r).contains(&"duplicate-symbol"), "{n}");
        if let Some(bfd) = &r.bfd {
            assert_eq!(&r.sim, bfd, "{n}");
        }
    }
    // R2: GNU ld fails; mortar extracts ks.o and warns archive-backref.
    case(&c, "R2", &["mkw.o", "libKS.a", "uk.o"], &["libKS.a(ks.o)"], &["archive-backref"]);
    // R3: the weak member is first.
    let r3 = case(&c, "R3", &["mk.o", "libKW.a", "libKS.a"], &["libKW.a(kw.o)"], &[]);
    assert_eq!(def_file(&r3, "K").as_deref(), Some("libKW.a(kw.o)"));
    case(&c, "R3b", &["mk.o", "libKW.a", "ks.o"], &["libKW.a(kw.o)"], &[]);
    // R4.
    let r4 = case(
        &c,
        "R4",
        &["mk.o", "libKS.a", "--whole-archive", "libKW.a", "--no-whole-archive"],
        &["libKS.a(ks.o)", "libKW.a(kw.o)"],
        &[],
    );
    assert_eq!(def_file(&r4, "K").as_deref(), Some("libKS.a(ks.o)"));
    case(&c, "R4b", &["-u", "K", "m0.o", "libKS.a"], &["libKS.a(ks.o)"], &[]);
    // R5.
    case(&c, "R5", &["mv.o", "libLC.a"], &["libLC.a(lvc.o)"], &[]);
    let r5 =
        case(&c, "R5b", &["mv.o", "libLC.a", "libLV.a"], &["libLC.a(lvc.o)", "libLV.a(lv.o)"], &[]);
    assert_eq!(def_file(&r5, "v").as_deref(), Some("libLV.a(lv.o)"));
    // R6.
    case(&c, "R6 function", &["mc.o", "libLF.a"], &[], &[]);
    case(&c, "R6 data", &["mc.o", "libLV.a"], &["libLV.a(lv.o)"], &[]);
    case(&c, "R6 weak first", &["wv.o", "mc.o", "libLV.a"], &["libLV.a(lv.o)"], &[]);
    case(&c, "R6 weak last", &["mc.o", "libLV.a", "wv.o"], &["libLV.a(lv.o)"], &[]);
    case(&c, "R6 strong function", &["fv.o", "mc.o", "libLV.a"], &[], &[]);
    // R7: TLS mismatches, an error in each direction.
    case(&c, "R7", &["mt.o", "mn.o"], &[], &["tls-mismatch"]);
    case(&c, "R7 notype", &["m.o", "t.o"], &[], &["tls-mismatch"]);
    case(&c, "R7 object", &["mo.o", "t.o"], &[], &["tls-mismatch"]);
    // R8: mortar reads the members' tables, not the index.
    case(&c, "R8", &["mk.o", "libnoidx.a"], &["libnoidx.a(ks.o)"], &[]);
    // R12.
    let r12 = case(&c, "R12 mv2 dloc", &["mv2.o", "dloc.o"], &[], &[]);
    assert!(def_file(&r12, "foo@V2").is_some());
    let r12 = case(&c, "R12 mv2 dv", &["mv2.o", "dv.o"], &[], &[]);
    assert!(def_file(&r12, "foo@V2").is_some());
    let r12 = case(&c, "R12 mf dloc", &["mf.o", "dloc.o"], &[], &[]);
    assert!(def_file(&r12, "foo").is_some());
    let r12 = case(&c, "R12 mf dv", &["mf.o", "dv.o"], &[], &[]);
    assert!(def_file(&r12, "foo").is_none());
    // R14: an unused hidden undefined key is no diagnostic.
    case(&c, "R14", &["hz.o"], &[], &[]);
}

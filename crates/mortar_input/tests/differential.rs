//! `spec/input.md` §16, the differential: the files GNU ld's `--trace`
//! reports against the files the walk opens (`Inputs.link.opened`), on
//! the link lines of the compiler drivers.
//!
//! Lines: fresh `-###` captures of gcc, g++, gfortran and clang (native,
//! and the x86-64 and i686 cross compilers; dynamic, `-no-pie`, static,
//! static PIE, shared), plus, when `MORTAR_INPUT_REVIEW_ARGV` names a
//! directory of `*.argv` files (one argument per line, `ARGV0=` first,
//! `----` last; the reviewer's captures), those lines, with their work
//! directory recreated in a scratch directory.
//!
//! Comparison. GNU ld prints each file it opens, an archive as one line
//! (its members are not printed), in the order it opens them; a group
//! rescan prints the group's files again. The two lists are aligned
//! greedily in order: a `--trace` line equal to the next file the walk
//! opened is *equal*; a line naming a file printed before is a *rescan*
//! (not an open: the walk opens each mention once, §10.1); anything else,
//! on either side, is *different*. Spellings are compared exactly (§4.5:
//! the found path is the candidate as spelled). Also compared: the
//! `skipping incompatible` warnings (§4.4) as multisets, with their order
//! against the skipped probes (§4.6; one search's warnings share a key,
//! §12.2, so the report orders them by text), and each `DT_NEEDED`
//! entry of GNU ld's output against the sonames of the walk's shared
//! objects (§8.2, §8.3).
#![allow(clippy::many_single_char_names, reason = "short names for fixtures and tallies")]

mod common;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use mortar_input::walk::ProbeOutcome;
use mortar_testutil::Scratch;

struct Line {
    name: String,
    args: Vec<OsString>,
}

#[derive(Default)]
struct Tally {
    lines: usize,
    tried: usize,
    equal: usize,
    rescans: usize,
    different: Vec<String>,
    skips_equal: usize,
    needed_equal: usize,
}

fn emulation(args: &[OsString]) -> Option<String> {
    let mut it = args.iter().map(|a| a.to_string_lossy().into_owned());
    while let Some(a) = it.next() {
        if a == "-m" {
            return it.next();
        }
        if let Some(e) = a.strip_prefix("-m")
            && e.starts_with("elf") | e.starts_with("aarch64")
        {
            return Some(e.to_owned());
        }
    }
    None
}

fn bfd_for(emulation: &str) -> &'static str {
    match emulation {
        "elf_x86_64" => "x86_64-linux-gnu-ld.bfd",
        "elf_i386" => "i686-linux-gnu-ld.bfd",
        _ => "ld.bfd",
    }
}

/// The line without `-o X` and `--trace`, which the comparison adds.
fn strip_output(args: Vec<OsString>) -> Vec<OsString> {
    let mut out = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if a == "-o" {
            it.next();
        } else if a != "--trace" && a != "-t" {
            out.push(a);
        }
    }
    out
}

#[allow(clippy::too_many_lines, reason = "the comparisons in order")]
fn compare(w: &Path, line: &Line, t: &mut Tally) {
    let em = emulation(&line.args).unwrap_or_else(|| "aarch64linux".into());
    let Some(ld) = common::need(bfd_for(&em)) else { return };
    let mut a = line.args.clone();
    a.extend([
        "--trace".into(),
        "-o".into(),
        w.join(format!("out.{}", line.name)).into_os_string(),
    ]);
    let o = common::tool_run(&ld, &a, w);
    let stdout = o.stdout_text();
    let stderr = o.stderr_text();
    let bfd: Vec<String> = stdout
        .lines()
        .map(|l| l.strip_prefix("trace: ").unwrap_or(l).trim().to_owned())
        .filter(|l| !l.is_empty())
        .collect();
    let out = common::run(w, &line.args);
    let ours = common::opened(&out);
    t.lines += 1;
    let mut j = 0;
    let mut seen: Vec<&str> = Vec::new();
    for b in &bfd {
        t.tried += 1;
        if j < ours.len() && &ours[j] == b {
            t.equal += 1;
            j += 1;
        } else if seen.contains(&b.as_str()) {
            t.rescans += 1;
        } else {
            t.different.push(format!(
                "{}: GNU ld opened {b}; the walk's next is {:?}",
                line.name,
                ours.get(j)
            ));
        }
        seen.push(b);
    }
    for o in &ours[j..] {
        t.tried += 1;
        t.different.push(format!("{}: the walk opened {o}, GNU ld did not", line.name));
    }
    // §4.4: the skip warnings.
    let bfd_skips: Vec<String> = stderr
        .lines()
        .filter_map(|l| l.find("skipping incompatible ").map(|i| l[i..].trim().to_owned()))
        .collect();
    let mut our_skips: Vec<String> = out
        .problems
        .iter()
        .filter(|p| p.id() == "skipped-incompatible")
        .map(mortar_input::problem::Problem::message)
        .collect();
    // The order: the warnings of one search share a key (§12.2) and sort
    // by text, so the search order is read from the probes (§4.6).
    let probe_order: Vec<String> = out
        .link
        .probes
        .iter()
        .filter(|p| matches!(p.outcome, ProbeOutcome::Skipped(_)))
        .map(|p| String::from_utf8_lossy(&p.path).into_owned())
        .collect();
    let bfd_order: Vec<String> = bfd_skips
        .iter()
        .map(|l| {
            l["skipping incompatible ".len()..].split(" when searching").next().unwrap().to_owned()
        })
        .collect();
    let mut bfd_sorted = bfd_skips.clone();
    bfd_sorted.sort();
    our_skips.sort();
    if bfd_sorted == our_skips && bfd_order == probe_order {
        t.skips_equal += bfd_skips.len();
    } else {
        t.different.push(format!(
            "{}: skips:\n    GNU ld:\n      {}\n    the walk:\n      {}\n    probes: {probe_order:?}",
            line.name,
            bfd_skips.join("\n      "),
            our_skips.join("\n      ")
        ));
    }
    let errors: Vec<String> = out
        .problems
        .iter()
        .filter(|p| p.is_error())
        .map(mortar_input::problem::Problem::render)
        .collect();
    if !errors.is_empty() {
        t.different.push(format!("{}: the walk failed: {errors:?}", line.name));
    }
    // §8.2/§8.3: every DT_NEEDED GNU ld wrote is a soname of the walk.
    let output = w.join(format!("out.{}", line.name));
    if o.success()
        && let Some(readelf) = common::need("readelf")
    {
        let d = common::tool_run(&readelf, &["-dW".into(), output.clone().into_os_string()], w)
            .stdout_text();
        let sonames: Vec<String> = out
            .files
            .iter()
            .filter_map(|f| f.soname.as_ref().map(|s| String::from_utf8_lossy(&s.0).into_owned()))
            .collect();
        for l in d.lines().filter(|l| l.contains("(NEEDED)")) {
            let n = l.split('[').nth(1).and_then(|r| r.split(']').next()).unwrap_or_default();
            if sonames.iter().any(|s| s == n) {
                t.needed_equal += 1;
            } else {
                t.different.push(format!(
                    "{}: DT_NEEDED {n} is no soname of the walk ({sonames:?})",
                    line.name
                ));
            }
        }
    }
    let _ = std::fs::remove_file(output);
}

fn compile(
    driver: &str,
    extra: &[&str],
    dir: &Path,
    src: (&str, &str),
    out: &str,
    flags: &[&str],
) -> bool {
    let Some(d) = common::need(driver) else { return false };
    std::fs::write(dir.join(src.0), src.1).unwrap();
    let mut a: Vec<OsString> = extra.iter().map(OsString::from).collect();
    a.extend(flags.iter().map(OsString::from));
    a.extend(["-c".into(), src.0.into(), "-o".into(), out.into()]);
    let o = common::tool_run(&d, &a, dir);
    assert!(o.success(), "{driver} {out}: {}", o.stderr_text());
    true
}

const C_MAIN: &str = "int main(void) { return 0; }\n";
const C_MATH: &str =
    "#include <math.h>\nvolatile double x = 2;\nint main(void) { return (int)sin(x); }\n";
const CXX: &str = "#include <iostream>\nint main() { std::cout << \"hi\\n\"; }\n";
const F90: &str = "program p\nprint *, 'hi'\nend program\n";
const PIC: &str = "int lib_f(int x) { return x + 1; }\n";

/// One driver family: its compilers, its `-###` drivers, its objects.
struct Family {
    tag: &'static str,
    cc: &'static str,
    cxx: &'static str,
    fc: Option<&'static str>,
    clang: &'static [&'static str],
}

const FAMILIES: &[Family] = &[
    Family { tag: "a64", cc: "gcc", cxx: "g++", fc: Some("gfortran"), clang: &[] },
    Family {
        tag: "x86",
        cc: "x86_64-linux-gnu-gcc",
        cxx: "x86_64-linux-gnu-g++",
        fc: Some("x86_64-linux-gnu-gfortran"),
        clang: &["--target=x86_64-linux-gnu"],
    },
    Family {
        tag: "i686",
        cc: "i686-linux-gnu-gcc",
        cxx: "i686-linux-gnu-g++",
        fc: None,
        clang: &["--target=i686-linux-gnu"],
    },
];

fn fresh_lines(w: &Path) -> Vec<Line> {
    let mut lines = Vec::new();
    for f in FAMILIES {
        let t = f.tag;
        let (m, mm, c, pic) =
            (format!("{t}m.o"), format!("{t}mm.o"), format!("{t}c.o"), format!("{t}pic.o"));
        if !compile(f.cc, &[], w, (&format!("{t}m.c"), C_MAIN), &m, &[])
            || !compile(f.cc, &[], w, (&format!("{t}mm.c"), C_MATH), &mm, &[])
            || !compile(f.cxx, &[], w, (&format!("{t}c.cc"), CXX), &c, &[])
            || !compile(f.cc, &[], w, (&format!("{t}pic.c"), PIC), &pic, &["-fPIC"])
        {
            continue;
        }
        let fo = format!("{t}f.o");
        let fortran =
            f.fc.is_some_and(|fc| compile(fc, &[], w, (&format!("{t}f.f90"), F90), &fo, &[]));
        let modes: &[(&str, &[&str])] = &[
            ("dyn", &[]),
            ("nopie", &["-no-pie"]),
            ("static", &["-static"]),
            ("spie", &["-static-pie"]),
        ];
        let mut drivers: Vec<(String, PathBuf, Vec<&str>, String)> = Vec::new();
        let cc = common::need(f.cc).unwrap();
        drivers.push(("gcc".into(), cc.clone(), vec![], mm.clone()));
        drivers.push(("gxx".into(), common::need(f.cxx).unwrap(), vec![], c.clone()));
        if fortran {
            drivers.push((
                "gfortran".into(),
                common::need(f.fc.unwrap()).unwrap(),
                vec![],
                fo.clone(),
            ));
        }
        if let Some(clang) = common::need("clang") {
            drivers.push(("clang".into(), clang.clone(), f.clang.to_vec(), mm.clone()));
            if let Some(clangxx) = common::need("clang++") {
                drivers.push(("clangxx".into(), clangxx, f.clang.to_vec(), c.clone()));
            }
        }
        for (dname, d, extra, obj) in &drivers {
            for (mode, flags) in modes {
                let mut a: Vec<&str> = extra.clone();
                a.extend_from_slice(flags);
                a.extend([obj.as_str(), "-lm", "-o", "out"]);
                if let Some(args) = common::driver_link_args(d, &a, w) {
                    lines.push(Line {
                        name: format!("{t}-{dname}-{mode}"),
                        args: strip_output(args),
                    });
                }
            }
            let mut a: Vec<&str> = extra.clone();
            a.extend(["-shared", pic.as_str(), "-o", "out.so"]);
            if let Some(args) = common::driver_link_args(d, &a, w) {
                lines.push(Line { name: format!("{t}-{dname}-shared"), args: strip_output(args) });
            }
        }
        // The forms the brief names: -nostdlib (no SEARCH_DIR, §4.2),
        // /dev/null as an input (§2.5).
        let a = ["-nostdlib", m.as_str(), "/dev/null", "-lc", "-o", "out"];
        if let Some(args) = common::driver_link_args(&cc, &a, w) {
            lines.push(Line { name: format!("{t}-gcc-nostdlib"), args: strip_output(args) });
        }
        // Multiarch: another target's library directory first, so its
        // libraries are skipped (§4.4).
        if t != "a64" {
            let a = ["-L/usr/lib/aarch64-linux-gnu", mm.as_str(), "-lm", "-o", "out"];
            if let Some(args) = common::driver_link_args(&cc, &a, w) {
                lines.push(Line { name: format!("{t}-gcc-multiarch"), args: strip_output(args) });
            }
        }
    }
    lines
}

/// The reviewer's lines, their `/tmp` work directory replaced by `w`.
fn review_lines(dir: &Path, w: &Path) -> Vec<Line> {
    let root = "/tmp/rvin-wp021-78867";
    let mut names: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "argv"))
        .collect();
    names.sort();
    let mut lines = Vec::new();
    for p in names {
        let name = p.file_stem().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(&p).unwrap();
        let args: Vec<OsString> = text
            .lines()
            .skip(1)
            .take_while(|l| *l != "----")
            .map(|l| {
                let l = l
                    .replace(&format!("{root}/w"), &w.to_string_lossy())
                    .replace(root, &w.to_string_lossy());
                // The one object the reviewer compiled through a temporary.
                OsString::from(if l == "/tmp/ccZajPJA.o" { "x86c.o".to_owned() } else { l })
            })
            .collect();
        if args.iter().any(|a| a.to_string_lossy().starts_with("@/tmp/")) {
            eprintln!("note: {name}: its response file is gone; not replayed");
            continue;
        }
        lines.push(Line { name: format!("review-{name}"), args: strip_output(args) });
    }
    lines
}

/// The reviewer's work directory: its objects and `lib/libz9.a`.
fn review_work(w: &Path) -> bool {
    for d in ["bin", "bi", "bx", "lib"] {
        std::fs::create_dir_all(w.join(d)).unwrap();
    }
    let ok = compile("gcc", &[], w, ("rm.c", C_MAIN), "m.o", &[])
        && compile("gcc", &[], w, ("rmm.c", C_MATH), "mm.o", &[])
        && compile("g++", &[], w, ("rc.cc", CXX), "c.o", &[])
        && compile("gfortran", &[], w, ("rf.f90", F90), "f.o", &[])
        && compile("gcc", &[], w, ("rpic.c", PIC), "mpic.o", &["-fPIC"])
        && compile("i686-linux-gnu-gcc", &[], w, ("rimm.c", C_MATH), "imm.o", &[])
        && compile("x86_64-linux-gnu-gcc", &[], w, ("rxmm.c", C_MATH), "xmm.o", &[])
        && compile("x86_64-linux-gnu-g++", &[], w, ("rxc.cc", CXX), "x86c.o", &[])
        && compile("gcc", &[], w, ("z9.c", "int z9(void) { return 9; }\n"), "lib/z9.o", &[]);
    ok && common::ar(&w.join("lib"), "libz9.a", &["z9.o"], false).is_some()
}

fn report(what: &str, t: &Tally) {
    eprintln!(
        "{what}: {} lines; trace lines tried {}, equal {}, rescans {}, different {}; skip warnings equal {}; DT_NEEDED equal {}",
        t.lines,
        t.tried,
        t.equal,
        t.rescans,
        t.different.len(),
        t.skips_equal,
        t.needed_equal
    );
    for d in &t.different {
        eprintln!("  {d}");
    }
}

#[test]
fn s16_differential_fresh_captures() {
    let s = Scratch::new("diff");
    if !common::have(&["native", "x86", "i686"]) {
        return;
    }
    let lines = fresh_lines(s.path());
    let mut t = Tally::default();
    for l in &lines {
        compare(s.path(), l, &mut t);
    }
    report("fresh captures", &t);
    assert!(t.lines >= 40, "{} lines", t.lines);
    assert!(t.skips_equal > 0, "the multiarch lines skip");
    assert!(t.different.is_empty(), "{} differences", t.different.len());
}

#[test]
fn s16_differential_review_lines() {
    let Some(dir) = std::env::var_os("MORTAR_INPUT_REVIEW_ARGV") else {
        eprintln!("note: MORTAR_INPUT_REVIEW_ARGV not set; skipped");
        return;
    };
    let s = Scratch::new("diffreview");
    if !common::have(&["native", "x86", "i686"]) || !review_work(s.path()) {
        return;
    }
    let lines = review_lines(Path::new(&dir), s.path());
    let mut t = Tally::default();
    for l in &lines {
        compare(s.path(), l, &mut t);
    }
    report("review lines", &t);
    assert!(t.different.is_empty(), "{} differences", t.different.len());
}

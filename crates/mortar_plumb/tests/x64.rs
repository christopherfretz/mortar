//! The x86-64 corpus (`spec/plumb.md` §9.1, §9.5): C, C++ and Fortran
//! from GCC and Clang, not PIC, PIE and PIC objects, `-fno-plt`, the TLS
//! dialects, and the `spec/x64.md` §14.2 cases W1, W4, W8, W9 and W10,
//! each linked static by GNU ld, lld 22 and mold with a map. Every link
//! must be judged with no finding but the anomalies it expects (§10.2),
//! and every expected anomaly or violation must be found: the defects of
//! the references are caught, never "ok".

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use common::{Link, Linker, known_anomaly};
use mortar_plumb::Report;
use mortar_testutil::{Scratch, tool};

const RELAX_S: &str = include_str!("x64/relax.s");
const BITFIELD_S: &str = include_str!("x64/bitfield.s");
const GD_C: &str = include_str!("x64/gd.c");
const F2_S: &str = include_str!("x64/f2.s");
const LG_C: &str = include_str!("x64/lg.c");
const SD_C: &str = include_str!("x64/sd.c");
const HELLO_CC: &str = include_str!("x64/hello.cc");
const TLS_C: &str = include_str!("x64/tls.c");
const TLS_MAIN_C: &str = include_str!("x64/tls_main.c");
const IPLT_IBT_S: &str = include_str!("x64/iplt-ibt.s");
const HELLO_F90: &str = include_str!("x64/hello.f90");
const HELLO_C: &str = "#include <stdio.h>\n#include <string.h>\n\
    int main(int c, char **v) { printf(\"%zu\\n\", strlen(v[0]) + c); return 0; }\n";

struct Tools {
    gcc: PathBuf,
    gxx: PathBuf,
    gfortran: PathBuf,
    clang: PathBuf,
    asm: PathBuf,
    ld: [PathBuf; 3],
}

fn tools() -> Option<Tools> {
    Some(Tools {
        gcc: tool("x86_64-linux-gnu-gcc")?,
        gxx: tool("x86_64-linux-gnu-g++")?,
        gfortran: tool("x86_64-linux-gnu-gfortran")?,
        clang: tool("clang-22")?,
        asm: tool("x86_64-linux-gnu-as")?,
        ld: [tool("x86_64-linux-gnu-ld.bfd")?, tool("ld.lld-22")?, tool("mold")?],
    })
}

fn run_in(dir: &Path, program: &Path, args: &[&str]) -> bool {
    let mut c = std::process::Command::new(program);
    c.current_dir(dir).args(args);
    mortar_testutil::run_with(&mut c, b"", mortar_testutil::DEFAULT_TIMEOUT).success()
}

fn must(dir: &Path, program: &Path, args: &[&str]) {
    assert!(
        run_in(dir, program, args),
        "{} {} failed in {}",
        program.display(),
        args.join(" "),
        dir.display()
    );
}

/// One case: what to link and, per linker, what the verifier must say.
struct Case {
    name: &'static str,
    driver: fn(&Tools) -> &Path,
    args: &'static [&'static str],
    /// Per linker (bfd, lld, mold): `None` when it refuses the link, else
    /// the anomalies and rules the report must contain beyond the
    /// run-wide defaults.
    expect: [Option<&'static [&'static str]>; 3],
}

fn gcc(t: &Tools) -> &Path {
    &t.gcc
}
fn gxx(t: &Tools) -> &Path {
    &t.gxx
}
fn gf(t: &Tools) -> &Path {
    &t.gfortran
}

const CLEAN: Option<&[&str]> = Some(&[]);

fn cases() -> Vec<Case> {
    let nostd: &[&str] = &["-static", "-nostdlib"];
    vec![
        // §5.9: every form, per linker. lld writes W4 over the large GD.
        Case { name: "relax", driver: gcc, args: nostd, expect: [CLEAN, Some(&["A12"]), CLEAN] },
        Case { name: "c-gcc", driver: gcc, args: &["-static", "c-gcc.o"], expect: [CLEAN; 3] },
        Case { name: "c-gcc-pie", driver: gcc, args: &["-static", "c-pie.o"], expect: [CLEAN; 3] },
        Case {
            name: "c-gcc-noplt",
            driver: gcc,
            args: &["-static", "c-noplt.o"],
            expect: [CLEAN; 3],
        },
        Case { name: "c-clang", driver: gcc, args: &["-static", "c-clang.o"], expect: [CLEAN; 3] },
        Case {
            name: "c-clang-pic",
            driver: gcc,
            args: &["-static", "c-clang-pic.o"],
            expect: [CLEAN; 3],
        },
        Case { name: "cxx", driver: gxx, args: &["-static", "cxx.o"], expect: [CLEAN; 3] },
        Case { name: "cxx-O0", driver: gxx, args: &["-static", "cxx0.o"], expect: [CLEAN; 3] },
        Case { name: "fortran", driver: gf, args: &["-static", "f.o"], expect: [CLEAN; 3] },
        Case {
            name: "tls-gd-ld",
            driver: gcc,
            args: &["-static", "tls-pic.o", "tls_main.o"],
            expect: [CLEAN; 3],
        },
        Case {
            name: "tls-desc",
            driver: gcc,
            args: &["-static", "tls-desc.o", "tls_main.o"],
            expect: [CLEAN; 3],
        },
        Case {
            name: "tls-ie",
            driver: gcc,
            args: &["-static", "tls-nopic.o", "tls_main.o"],
            expect: [CLEAN; 3],
        },
        // W1: GNU ld truncates; lld and mold refuse. A violation (R2).
        Case {
            name: "w1",
            driver: gcc,
            args: &["-static", "-nostdlib", "bf.o"],
            expect: [Some(&["R2"]), None, None],
        },
        // W4: lld's GD rewriting of the large model.
        Case {
            name: "w4",
            driver: gcc,
            args: &["-static", "gd.o"],
            expect: [CLEAN, Some(&["A12"]), CLEAN],
        },
        // W6: mold's IBT IPLT entries have no endbr64 (F5).
        Case {
            name: "w6",
            driver: gcc,
            args: &["-static", "-nostdlib", "ibt.o"],
            expect: [CLEAN, CLEAN, Some(&["A13", "A17"])],
        },
        // W8: lld and mold trap in `.init` padding.
        Case {
            name: "w8",
            driver: gcc,
            args: &["-static", "c-gcc.o", "f2.o"],
            expect: [CLEAN, Some(&["A16"]), Some(&["A16"])],
        },
        // W9: lld makes the large common absolute; mold refuses.
        Case {
            name: "w9",
            driver: gcc,
            args: &["-static", "lg.o"],
            expect: [CLEAN, Some(&["A14"]), None],
        },
        // W10: GNU ld and mold write template addresses in .debug_addr.
        Case {
            name: "w10",
            driver: gcc,
            args: &["-static", "sd.o"],
            expect: [Some(&["A15"]), CLEAN, Some(&["A15"])],
        },
    ]
}

struct Corpus {
    _scratch: Scratch,
    links: Vec<(Link, &'static [&'static str], bool)>,
}

/// The run-wide defaults (`tests/link/anomalies`): mold's on every link
/// with the C runtime (A1, A2, A3, A9, A13; A3 and A9 without), GNU ld's
/// A10 with the C runtime.
fn defaults(linker: Linker, crt: bool) -> &'static [&'static str] {
    match (linker, crt) {
        (Linker::Mold, true) => &["A1", "A2", "A3", "A9", "A13"],
        (Linker::Mold, false) => &["A3", "A9"],
        (Linker::Bfd, true) => &["A10"],
        _ => &[],
    }
}

fn corpus() -> Option<&'static Corpus> {
    static C: OnceLock<Option<Corpus>> = OnceLock::new();
    C.get_or_init(build).as_ref()
}

#[allow(clippy::too_many_lines)] // the objects, then the links
fn build() -> Option<Corpus> {
    let t = tools()?;
    let scratch = Scratch::new("plumb-x64");
    let dir = scratch.path().to_path_buf();
    for (n, text) in [
        ("relax.s", RELAX_S),
        ("bitfield.s", BITFIELD_S),
        ("gd.c", GD_C),
        ("f2.s", F2_S),
        ("lg.c", LG_C),
        ("sd.c", SD_C),
        ("hello.cc", HELLO_CC),
        ("tls.c", TLS_C),
        ("tls_main.c", TLS_MAIN_C),
        ("hello.f90", HELLO_F90),
        ("iplt-ibt.s", IPLT_IBT_S),
        ("hello.c", HELLO_C),
    ] {
        std::fs::write(dir.join(n), text).unwrap();
    }
    let (cc, cxx, as_) = (&t.gcc, &t.gxx, &t.asm);
    must(&dir, as_, &["--defsym", "GDL=1", "--defsym", "LDL=1", "relax.s", "-o", "relax.o"]);
    must(&dir, as_, &["--defsym", "GDL=1", "relax.s", "-o", "relax-nold.o"]);
    must(&dir, as_, &["--defsym", "m1=-32769", "--defsym", "m2=-129", "bitfield.s", "-o", "bf.o"]);
    must(&dir, as_, &["f2.s", "-o", "f2.o"]);
    must(&dir, as_, &["iplt-ibt.s", "-o", "ibt.o"]);
    must(&dir, cc, &["-O2", "-c", "hello.c", "-o", "c-gcc.o"]);
    must(&dir, cc, &["-O2", "-fPIE", "-c", "hello.c", "-o", "c-pie.o"]);
    must(&dir, cc, &["-O2", "-fPIC", "-fno-plt", "-c", "hello.c", "-o", "c-noplt.o"]);
    let clang = |pic: &str, out: &str| {
        must(
            &dir,
            &t.clang,
            &["--target=x86_64-linux-gnu", "-O2", pic, "-c", "hello.c", "-o", out],
        );
    };
    clang("-fno-pic", "c-clang.o");
    clang("-fPIC", "c-clang-pic.o");
    must(&dir, cxx, &["-O2", "-c", "hello.cc", "-o", "cxx.o"]);
    must(&dir, cxx, &["-O0", "-g", "-c", "hello.cc", "-o", "cxx0.o"]);
    must(&dir, &t.gfortran, &["-O2", "-c", "hello.f90", "-o", "f.o"]);
    must(&dir, cc, &["-O2", "-fPIC", "-c", "tls.c", "-o", "tls-pic.o"]);
    must(&dir, cc, &["-O2", "-fPIC", "-mtls-dialect=gnu2", "-c", "tls.c", "-o", "tls-desc.o"]);
    must(&dir, cc, &["-O2", "-g", "-c", "tls.c", "-o", "tls-nopic.o"]);
    must(&dir, cc, &["-O2", "-c", "tls_main.c", "-o", "tls_main.o"]);
    must(&dir, cc, &["-O2", "-fPIC", "-mcmodel=large", "-c", "gd.c", "-o", "gd.o"]);
    must(&dir, cc, &["-O2", "-mcmodel=medium", "-fcommon", "-c", "lg.c", "-o", "lg.o"]);
    must(&dir, cc, &["-g", "-O2", "-gsplit-dwarf", "-c", "sd.c", "-o", "sd.o"]);
    // Each linker as `ld` in a directory of its own, for the driver's -B.
    for (l, path) in Linker::ALL.iter().zip(&t.ld) {
        let bin = dir.join(format!("ld-{}", l.name()));
        std::fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink(path, bin.join("ld")).unwrap();
    }
    let mut links = Vec::new();
    for case in cases() {
        for (k, &linker) in Linker::ALL.iter().enumerate() {
            let out = format!("{}.{}", case.name, linker.name());
            let map = format!("{out}.map");
            let bin = dir.join(format!("ld-{}", linker.name())).display().to_string();
            let mut args: Vec<String> = vec![format!("-B{bin}/")];
            args.extend(case.args.iter().map(|s| (*s).to_string()));
            if case.name == "relax" {
                args.push(if linker == Linker::Lld { "relax-nold.o" } else { "relax.o" }.into());
            }
            args.extend(["-o".into(), out.clone(), format!("-Wl,-Map,{map}")]);
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            let linked = run_in(&dir, (case.driver)(&t), &refs);
            let Some(expect) = case.expect[k] else {
                assert!(
                    !linked,
                    "{}/{}: the link was expected to be refused",
                    case.name,
                    linker.name()
                );
                continue;
            };
            assert!(linked, "{}/{}: the link failed", case.name, linker.name());
            let crt = !case.args.contains(&"-nostdlib");
            links.push((
                Link {
                    case: case.name,
                    linker,
                    dir: dir.clone(),
                    output: dir.join(&out),
                    map: dir.join(&map),
                },
                expect,
                crt,
            ));
        }
    }
    Some(Corpus { _scratch: scratch, links })
}

/// The anomaly ids of `r`'s findings, and the rule ids of the rest.
fn verdict(link: &Link, r: &Report) -> (BTreeSet<String>, BTreeSet<String>) {
    let (mut anomalies, mut rules) = (BTreeSet::new(), BTreeSet::new());
    for f in &r.findings {
        match known_anomaly(link, f) {
            Some(a) => anomalies.insert(mortar_plumb::anomaly::id(a).to_string()),
            None => rules.insert(f.rule.id()),
        };
    }
    (anomalies, rules)
}

// spec/plumb.md §9.1, §10.2; spec/x64.md §14.2.
#[test]
fn x64_corpus_is_judged_with_only_its_expected_findings() {
    let Some(c) = corpus() else { return };
    let mut failures = Vec::new();
    for (link, expect, crt) in &c.links {
        let r = link.verify();
        let (anomalies, rules) = verdict(link, &r);
        let allowed: BTreeSet<String> =
            expect.iter().chain(defaults(link.linker, *crt)).map(|s| (*s).to_string()).collect();
        let found: BTreeSet<String> = anomalies.iter().chain(&rules).cloned().collect();
        let missing: Vec<&&str> = expect.iter().filter(|e| !found.contains(**e)).collect();
        let extra: Vec<&String> = found.iter().filter(|f| !allowed.contains(*f)).collect();
        if !missing.is_empty() || !extra.is_empty() {
            failures.push(format!(
                "{}: missing {missing:?}, unexpected {extra:?}\n{}",
                link.label(),
                r.findings
                    .iter()
                    .filter(|f| extra.iter().any(|e| **e == f.rule.id()))
                    .take(5)
                    .map(|f| format!("  {} {}", f.rule.id(), f.message))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
        // The table judges the x86-64 records: most are verified.
        assert!(r.coverage.records > 0, "{}: no records", link.label());
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// spec/plumb.md §5.9: the relax case exercises every rewriting family.
#[test]
fn x64_relax_case_accepts_every_rewriting_family() {
    let Some(c) = corpus() else { return };
    for (link, ..) in c.links.iter().filter(|(l, ..)| l.case == "relax") {
        let r = link.verify();
        let s = r.summary();
        for family in ["tlsie", "tlsdesc", "tlsgd", "tlsld", "got-to"] {
            // mold keeps GD (a GOT pair) and IE `add`; its LD is its own form.
            if link.linker == Linker::Mold && family == "tlsgd" {
                continue;
            }
            assert!(s.contains(family), "{}: no {family} rewriting accepted:\n{s}", link.label());
        }
    }
}

// ---- mutations (spec/plumb.md §9.2, x86-64) ----

fn le64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

/// The file offset of an address in a `PT_LOAD`'s file image.
fn file_offset(b: &[u8], addr: u64) -> Option<usize> {
    let phoff = le64(b, 32) as usize;
    let n = usize::from(u16::from_le_bytes([b[56], b[57]]));
    (0..n).map(|i| phoff + i * 56).find_map(|o| {
        let (ty, off, va, fsz) = (le32(b, o), le64(b, o + 8), le64(b, o + 16), le64(b, o + 32));
        (ty == 1 && va <= addr && addr < va + fsz).then(|| (off + (addr - va)) as usize)
    })
}

/// A link, its inputs and map, and whether a mutant is found.
struct Ctx<'c> {
    link: &'c Link,
    output: Vec<u8>,
    files: Vec<(String, Vec<u8>)>,
    map: mortar_plumb::Map,
}

impl<'c> Ctx<'c> {
    fn of(link: &'c Link) -> Self {
        let map = link.parse_map();
        let files = link.inputs(&map);
        Ctx { link, output: link.output_bytes(), files, map }
    }
    fn report(&self, output: &[u8]) -> Report {
        common::verify_with(output, &self.files, &self.map)
    }
    /// A finding that is not a recorded anomaly.
    fn found(&self, output: &[u8]) -> bool {
        !common::unexpected(self.link, &self.report(output)).is_empty()
    }
}

fn link_of<'c>(c: &'c Corpus, case: &str, linker: Linker) -> Option<&'c Link> {
    c.links.iter().map(|(l, ..)| l).find(|l| l.case == case && l.linker == linker)
}

/// The relocated places of `relax.o`: (address, field width, type name),
/// and for the GOT and TLS types the instruction bytes before the field.
fn relax_places(ctx: &Ctx) -> Vec<(u64, usize, &'static str)> {
    let mut out = Vec::new();
    for (path, bytes) in &ctx.files {
        if !path.starts_with("relax") {
            continue;
        }
        let fi = ctx.map.files.iter().position(|f| &f.path == path).unwrap();
        let obj = mortar_elf::Object::parse(bytes).unwrap();
        for s in obj.sections() {
            if s.sh_type() != mortar_elf::consts::SHT_RELA {
                continue;
            }
            let target = obj.section(s.info_section().unwrap()).unwrap();
            if target.flags() & mortar_elf::consts::SHF_ALLOC == 0 {
                continue;
            }
            let name = String::from_utf8_lossy(target.name().unwrap()).into_owned();
            let Some(pl) = ctx.map.placed.iter().find(|p| {
                p.file as usize == fi
                    && p.section == mortar_plumb::map::SectionRef::Name(name.clone())
            }) else {
                continue;
            };
            for r in s.rela().unwrap() {
                let Some(def) = mortar_plumb::x64::def(r.r_type) else { continue };
                let w = usize::from(def.field.width());
                if w > 0 {
                    out.push((pl.addr + r.r_offset, w, def.name));
                }
            }
        }
    }
    out
}

// spec/plumb.md §9.2 (a) for x86-64: a flipped bit in every relocated field
// of the relax case (lowest and highest bit), per linker; and a flipped
// bit in the opcode byte before every GOT and TLS field, which a
// rewriting may have changed (§5.9).
#[test]
fn x64_flipped_fields_and_opcodes_are_found() {
    let Some(c) = corpus() else { return };
    let (mut total, mut found, mut op_total, mut op_found, mut skipped) = (0, 0, 0, 0, 0);
    let mut missed = Vec::new();
    for linker in Linker::ALL {
        let link = link_of(c, "relax", linker).unwrap();
        let ctx = Ctx::of(link);
        // The places of a recorded anomaly's sequence (A12: lld's W4 over
        // the 22-byte GD) are not a clean starting point.
        let base: Vec<u64> = ctx
            .report(&ctx.output)
            .findings
            .iter()
            .filter(|f| f.rule == mortar_plumb::Rule::Field)
            .filter_map(|f| f.addr)
            .collect();
        for (p, w, name) in relax_places(&ctx) {
            if base.iter().any(|&a| a <= p && p < a + 22) {
                skipped += 1;
                continue;
            }
            let off = file_offset(&ctx.output, p).unwrap();
            for bit in [0, 8 * w - 1] {
                let mut m = ctx.output.clone();
                m[off + bit / 8] ^= 1 << (bit % 8);
                total += 1;
                if ctx.found(&m) {
                    found += 1;
                } else {
                    missed.push(format!("{} {name} at {p:#x} bit {bit}", link.label()));
                }
            }
            if name.contains("GOT") || name.contains("TLS") {
                let mut m = ctx.output.clone();
                m[off - 2] ^= 0x01;
                op_total += 1;
                if ctx.found(&m) {
                    op_found += 1;
                } else {
                    missed.push(format!("{} {name} at {p:#x}: opcode byte", link.label()));
                }
            }
        }
    }
    eprintln!(
        "§9.2 (a) x86-64 field bit flips: {found}/{total}; opcode bytes: {op_found}/{op_total} ({skipped} places of anomalies skipped)"
    );
    assert!(missed.is_empty(), "not found:\n{}", missed.join("\n"));
}

/// The section header table: (name, `sh_addr`, `sh_offset`, `sh_size`).
fn sections(b: &[u8]) -> Vec<(String, u64, u64, u64)> {
    let h = mortar_elf::FileHeader::parse(b).unwrap();
    let t = mortar_elf::SectionTable::parse(b, &h).unwrap();
    t.iter()
        .map(|s| {
            let sh = s.header();
            let n = String::from_utf8_lossy(s.name().unwrap_or(b"")).into_owned();
            (n, sh.sh_addr, sh.sh_offset, sh.sh_size)
        })
        .collect()
}

// spec/plumb.md §9.2 for the rules WP-028c adds: S19 (an `int3` in GNU
// ld's `.init` padding), Y3 for a common (lc made absolute, as lld's
// W9), P1 (an x86 property bit), S11 (a 4-aligned note under p_align 8
// in a GNU ld output), the TLS block (PT_TLS memsz).
#[test]
#[allow(clippy::many_single_char_names)]
fn x64_new_rules_are_enforced() {
    let Some(c) = corpus() else { return };
    let mut results = Vec::new();
    // S19: GNU ld's NOP padding in `.init`, one byte made int3.
    let ctx = Ctx::of(link_of(c, "w8", Linker::Bfd).unwrap());
    let init = sections(&ctx.output).into_iter().find(|s| s.0 == ".init").unwrap();
    assert!(!ctx.found(&ctx.output), "w8/bfd is not clean");
    let mut hit = 0;
    let mut tried = 0;
    for k in 0..init.3 as usize {
        let at = init.2 as usize + k;
        // A byte of the padding: a NOP byte after crti's 18 bytes.
        if k < 18 || ctx.output[at] != 0x66 && ctx.output[at] != 0x0f && ctx.output[at] != 0x2e {
            continue;
        }
        let mut m = ctx.output.clone();
        m[at] = 0xcc;
        tried += 1;
        hit += usize::from(ctx.report(&m).with_rule("S19").next().is_some());
        break;
    }
    results.push(("S19 .init padding made int3", hit, tried));
    // Y3: the large common made absolute.
    let ctx = Ctx::of(link_of(c, "w9", Linker::Bfd).unwrap());
    // X-54: the large sections' placement is named as not judged.
    let mut named = ctx.report(&ctx.output).coverage.placement_not_judged;
    named.sort();
    assert_eq!(named, [".lbss", ".ldata", ".lrodata"], "large sections named not judged");
    let st = sections(&ctx.output).into_iter().find(|s| s.0 == ".symtab").unwrap();
    let strtab = sections(&ctx.output).into_iter().find(|s| s.0 == ".strtab").unwrap();
    let mut hit = 0;
    for i in 0..(st.3 / 24) as usize {
        let e = st.2 as usize + i * 24;
        let name_at = strtab.2 as usize + le32(&ctx.output, e) as usize;
        if ctx.output[name_at..].starts_with(b"lc\0") {
            let mut m = ctx.output.clone();
            m[e + 6..e + 8].copy_from_slice(&0xfff1u16.to_le_bytes());
            hit = usize::from(
                ctx.report(&m).findings.iter().any(|f| f.message.starts_with("common symbol")),
            );
        }
    }
    results.push(("Y3 common made absolute", hit, 1));
    // P1: the ISA_1_NEEDED bit of a GNU ld C output flipped.
    let ctx = Ctx::of(link_of(c, "c-gcc", Linker::Bfd).unwrap());
    let note = sections(&ctx.output).into_iter().find(|s| s.0 == ".note.gnu.property").unwrap();
    let mut m = ctx.output.clone();
    // namesz, descsz, type, "GNU\0", then (pr_type, pr_datasz, data).
    let d = note.2 as usize + 16;
    assert_eq!(le32(&m, d), 0xc000_8002, "the first property is ISA_1_NEEDED");
    m[d + 8] ^= 0x02;
    results.push((
        "P1 x86 property bit",
        usize::from(ctx.report(&m).with_rule("P1").next().is_some()),
        1,
    ));
    // F5: an IBT IPLT entry's endbr64 made NOPs; the IFUNC word changed.
    // (mold's entries have no endbr64 to remove: W6, A17.)
    for (linker, word, what) in
        [(Linker::Bfd, true, "F5 IBT IPLT, GNU ld"), (Linker::Lld, true, "F5 IBT IPLT, lld")]
    {
        let ctx = Ctx::of(link_of(c, "w6", linker).unwrap());
        let b = &ctx.output;
        let secs = sections(b);
        let mut hit = 0;
        let mut tried = 0;
        if let Some(p) = secs.iter().find(|s| s.0 == ".plt" || s.0 == ".iplt") {
            let (at, size) = (p.2 as usize, p.3 as usize);
            if let Some(k) = (0..size.saturating_sub(4))
                .find(|&k| b[at + k..at + k + 4] == [0xf3, 0x0f, 0x1e, 0xfa])
            {
                let mut m = b.clone();
                m[at + k..at + k + 4].copy_from_slice(&[0x0f, 0x1f, 0x40, 0x00]);
                tried += 1;
                hit += usize::from(ctx.report(&m).with_rule("F5").next().is_some());
            }
        }
        if word {
            let g = secs.iter().find(|s| s.0 == ".got.plt").unwrap();
            let mut m = b.clone();
            let o = g.2 as usize + g.3 as usize - 8;
            m[o] ^= 0x10;
            tried += 1;
            hit += usize::from(ctx.report(&m).with_rule("F5").next().is_some());
        }
        results.push((what, hit, tried));
    }
    // TLS: PT_TLS memsz grown by its alignment moves every TP offset.
    let ctx = Ctx::of(link_of(c, "tls-ie", Linker::Bfd).unwrap());
    let b = &ctx.output;
    let phoff = le64(b, 32) as usize;
    let n = usize::from(u16::from_le_bytes([b[56], b[57]]));
    let tls = (0..n).map(|i| phoff + i * 56).find(|&o| le32(b, o) == 7).unwrap();
    let mut m = b.clone();
    let memsz = le64(&m, tls + 40) + le64(&m, tls + 48);
    m[tls + 40..tls + 48].copy_from_slice(&memsz.to_le_bytes());
    results.push(("TLS block memsz grown", usize::from(ctx.found(&m)), 1));
    for (what, hit, of) in &results {
        eprintln!("§9.2 x86-64 {what}: {hit}/{of} found");
    }
    let missed: Vec<_> = results.iter().filter(|(_, h, o)| h != o).collect();
    assert!(missed.is_empty(), "not found: {missed:?}");
}

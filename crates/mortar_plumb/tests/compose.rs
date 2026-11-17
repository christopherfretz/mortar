//! P-P1 (`spec/plumb.md` §14.5; RL-6, WP-186): the composition of a `-r`
//! output and its final link, on WP-154's D.3 components.
//!
//! Each component is linked `-r` by GNU ld and by lld; each line is then
//! final-linked twice by GNU ld, lld and mortar (when its binary is
//! built: the lane's smoke tests always run it): directly (`D`) and
//! through the `-r` output (`V`, the component's inputs replaced at the
//! place of the first). The finals are libc-free (`main.c` defines `rec`
//! and `_start`), so every input is the test's own. P-P1 must find
//! nothing on them; the mutations of §9.2 (t) (21), one per item, one
//! just outside each tolerance and one per exclusion must be named.

// d, v, r, e, k: the direct link, the link through -r, the -r output,
// the editor, a symbol index, throughout.
#![allow(clippy::many_single_char_names)]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use mortar_plumb::compose::{Link, Partial, compose};
use mortar_plumb::{Format, Input, Map, Report};
use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, run_with, tool};

#[path = "relocatable/ed.rs"]
mod ed;
use ed::Ed;

const C1: &str = include_str!("relocatable/c1.c");
const C2: &str = include_str!("relocatable/c2.c");
const C3: &str = include_str!("relocatable/c3.c");
const C4: &str = include_str!("relocatable/c4.c");
const REC_H: &str = include_str!("relocatable/rec.h");

/// The final link's own object: libc-free, it references every global of
/// both components.
const MAIN: &str = r"
__attribute__((weak)) void rec(int v) { static volatile int last; last = v; }
extern int g_data, common_x[];
extern __thread int tv;
const char *msg1(void); const char *msg2(void);
int wk(void); int wk_only(void); int bump(void); int call_undef_weak(void);
int use_hid(void); int ifn(void);
int k1_value(void); int k2_value(void);
int __wrap_hid(void) { return 0; }
int get_tv(void) { return tv; }
__attribute__((used)) void *const keep[] = {
  &g_data, common_x, (void *)msg1, (void *)msg2, (void *)wk, (void *)wk_only,
  (void *)bump, (void *)call_undef_weak, (void *)use_hid, (void *)ifn,
  (void *)k1_value, (void *)k2_value, (void *)get_tv,
};
void _start(void) { for (;;) ; }
";

/// The C++ component without libstdc++: an inline function with a static
/// and a template instantiation, COMDAT in both objects.
const K1: &str = r#"
inline int shared_inline(int x) { static int k = x; return k + x; }
template <class T> T twice(T v) { return v + v; }
extern "C" int k1_value() { return shared_inline(1) + twice(2); }
"#;
const K2: &str = r#"
inline int shared_inline(int x) { static int k = x; return k + x; }
template <class T> T twice(T v) { return v + v; }
extern "C" int k2_value() { return shared_inline(5) + twice(10); }
"#;

/// A member that defines `rec`: the `-r` extracts it, the direct link
/// does not (`main.o` defines `rec` first): exclusion (a).
const RECDEF: &str = "void rec(int v) { (void)v; }\n";

const D3: &[&str] = &["-O1", "-g", "-fcommon", "-ffunction-sections", "-fdata-sections"];
const CXX: &[&str] = &[
    "-O1",
    "-g",
    "-ffunction-sections",
    "-fdata-sections",
    "-fno-exceptions",
    "-fno-rtti",
    "-fno-threadsafe-statics",
];

struct Tools {
    arch: Arch,
    gcc: PathBuf,
    gxx: PathBuf,
    ar: PathBuf,
    bfd: PathBuf,
    lld: PathBuf,
    mortar: Option<PathBuf>,
}

/// mortar's binary next to the test's (`target/<profile>/mortar`), when
/// the workspace built it.
fn mortar_bin() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let p = exe.parent()?.parent()?.join("mortar");
    p.is_file().then_some(p)
}

impl Tools {
    fn find(arch: Arch) -> Option<Tools> {
        Some(Tools {
            arch,
            gcc: arch.tool("gcc")?,
            gxx: arch.tool("g++")?,
            ar: arch.tool("ar").or_else(|| tool("ar"))?,
            bfd: arch.tool("ld.bfd")?,
            lld: tool("ld.lld")?,
            mortar: mortar_bin(),
        })
    }

    fn emulation(&self) -> &'static str {
        match self.arch {
            Arch::Aarch64 => "aarch64linux",
            Arch::X86_64 => "elf_x86_64",
            Arch::I386 => "elf_i386",
        }
    }
}

/// A linker of a link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum L {
    Bfd,
    Lld,
    Mortar,
}

impl L {
    fn name(self) -> &'static str {
        match self {
            L::Bfd => "bfd",
            L::Lld => "lld",
            L::Mortar => "mortar",
        }
    }
    fn format(self) -> Format {
        match self {
            L::Bfd => Format::Gnu,
            L::Lld => Format::Lld,
            L::Mortar => Format::Mortar,
        }
    }
}

fn run_ok(dir: &Path, program: &Path, args: &[String]) -> Result<(), String> {
    let mut c = std::process::Command::new(program);
    c.args(args).current_dir(dir);
    let o = run_with(&mut c, b"", std::time::Duration::from_mins(2));
    if o.success() {
        Ok(())
    } else {
        Err(format!("{} {}: {}", program.display(), args.join(" "), o.stderr_text()))
    }
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| (*x).to_string()).collect()
}

fn build(t: &Tools, dir: &Path) -> Result<(), String> {
    std::fs::write(dir.join("rec.h"), REC_H).map_err(|e| e.to_string())?;
    let srcs: [(&str, &str, bool, &[&str]); 8] = [
        ("c1.c", C1, false, D3),
        ("c2.c", C2, false, D3),
        ("c3.c", C3, false, D3),
        ("c4.c", C4, false, D3),
        ("main.c", MAIN, false, &["-O1", "-g", "-fno-stack-protector"]),
        ("k1.cc", K1, true, CXX),
        ("k2.cc", K2, true, CXX),
        ("recdef.c", RECDEF, false, &["-O1"]),
    ];
    for (name, text, cxx, flags) in srcs {
        std::fs::write(dir.join(name), text).map_err(|e| e.to_string())?;
        let obj = format!("{}.o", name.split('.').next().unwrap_or(name));
        let mut a = s(flags);
        a.extend(["-c".into(), name.into(), "-o".into(), obj]);
        run_ok(dir, if cxx { &t.gxx } else { &t.gcc }, &a)?;
    }
    run_ok(dir, &t.ar, &s(&["rcs", "librec.a", "recdef.o"]))
}

/// One link's words (after the program), its output and map paths.
fn words(t: &Tools, l: L, out: &str, opts: &[&str], inputs: &[&str]) -> Vec<String> {
    let mut w = Vec::new();
    if l != L::Bfd {
        w.extend(["-m".to_string(), t.emulation().to_string()]);
    }
    w.extend(s(opts));
    w.extend(["-o".to_string(), out.to_string()]);
    w.push(if l == L::Mortar {
        format!("--mortar-map={out}.map")
    } else {
        format!("-Map={out}.map")
    });
    w.extend(s(inputs));
    w
}

fn link(t: &Tools, l: L, dir: &Path, w: &[String]) -> Result<(), String> {
    let prog = match l {
        L::Bfd => t.bfd.clone(),
        L::Lld => t.lld.clone(),
        L::Mortar => t.mortar.clone().ok_or("mortar is not built")?,
    };
    run_ok(dir, &prog, w)
}

/// A link read back: its output, inputs, map and line.
struct Read {
    out: Vec<u8>,
    files: Vec<(String, Vec<u8>)>,
    map: Map,
    line: Vec<String>,
    format: Format,
}

fn read(dir: &Path, l: L, out: &str, line: Vec<String>) -> Read {
    read_as(dir, l.format(), out, line)
}

fn read_as(dir: &Path, format: Format, out: &str, line: Vec<String>) -> Read {
    let bytes = std::fs::read(dir.join(out)).unwrap();
    let text = std::fs::read_to_string(dir.join(format!("{out}.map"))).unwrap();
    let map = mortar_plumb::map::parse(format, &text)
        .unwrap_or_else(|e| panic!("{out}: map line {}: {}", e.line, e.message));
    let files = map
        .paths()
        .into_iter()
        .filter_map(|p| std::fs::read(dir.join(p)).ok().map(|b| (p.to_string(), b)))
        .collect();
    Read { out: bytes, files, map, line, format }
}

fn inputs(r: &Read) -> Vec<Input<'_>> {
    r.files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect()
}

/// P-P1 on `D`, `V` and the `-r` link, with `V`'s output and map text
/// optionally replaced (a mutation).
fn p1(d: &Read, v: &Read, r: &Read, rpath: &str) -> Report {
    let (di, vi, ri) = (inputs(d), inputs(v), inputs(r));
    let line = |x: &Read| (x.format != Format::Mortar).then(|| x.line.clone());
    let dl = Link { output: &d.out, src: &di, map: &d.map, line: line(d) };
    let vl = Link { output: &v.out, src: &vi, map: &v.map, line: line(v) };
    let rl = Partial {
        link: Link { output: &r.out, src: &ri, map: &r.map, line: Some(r.line.clone()) },
        path: rpath,
    };
    compose(&dl, &vl, &rl)
}

fn p1_findings(rep: &Report) -> Vec<String> {
    rep.findings
        .iter()
        .filter(|f| f.message.starts_with("P-P1"))
        .map(|f| f.message.clone())
        .collect()
}

/// A composition: the component, its inputs, the -r output's name.
struct Comp {
    name: &'static str,
    inputs: &'static [&'static str],
}

/// D.3's components (§14.9): the C component, `cifunc`, `cctors`, and
/// the C++ component (here without libstdc++).
const COMPS: [Comp; 4] = [
    Comp { name: "compc", inputs: &["c1.o", "c2.o"] },
    Comp { name: "cifunc", inputs: &["c4.o"] },
    Comp { name: "cctors", inputs: &["c3.o"] },
    Comp { name: "compx", inputs: &["k1.o", "k2.o"] },
];
const LINE: [&str; 7] = ["main.o", "c1.o", "c2.o", "c3.o", "c4.o", "k1.o", "k2.o"];
/// mortar refuses `.ctors` sections by name (`unsupported feature:
/// .ctors section`): its line has no `c3.o`, and `cctors` is not linked.
const LINE_MORTAR: [&str; 6] = ["main.o", "c1.o", "c2.o", "c4.o", "k1.o", "k2.o"];
const FINAL: &[&str] = &["-e", "_start"];

fn line_of(l: L) -> &'static [&'static str] {
    if l == L::Mortar { &LINE_MORTAR } else { &LINE }
}

/// `line` with the component's inputs replaced at the first's place.
fn via_line(line: &[&'static str], c: &Comp, r: &str) -> Vec<&'static str> {
    let mut v: Vec<&'static str> = Vec::new();
    for &w in line {
        if c.inputs.first() == Some(&w) {
            v.push(Box::leak(r.to_string().into_boxed_str()));
        } else if !c.inputs.contains(&w) {
            v.push(w);
        }
    }
    v
}

/// The `-r` of a component by `rl`: its read link.
fn partial(
    t: &Tools,
    rl: L,
    dir: &Path,
    c: &Comp,
    extra: &[&str],
    tag: &str,
) -> Result<(String, Read), String> {
    let out = format!("{}.{}{tag}.o", c.name, rl.name());
    let mut ins: Vec<&str> = c.inputs.to_vec();
    ins.extend(extra);
    let w = words(t, rl, &out, &["-r"], &ins);
    link(t, rl, dir, &w)?;
    let r = read(dir, rl, &out, w);
    Ok((out, r))
}

/// A final link: its read link.
fn final_link(
    t: &Tools,
    l: L,
    dir: &Path,
    out: &str,
    opts: &[&str],
    ins: &[&str],
) -> Result<Read, String> {
    let mut o: Vec<&str> = FINAL.to_vec();
    o.extend(opts);
    let w = words(t, l, out, &o, ins);
    link(t, l, dir, &w)?;
    Ok(read(dir, l, out, w))
}

fn tmp(dir: &Path, name: &str, b: &[u8]) {
    std::fs::write(dir.join(name), b).unwrap();
}

/// The pass cells: every component, `-r` by GNU ld and lld, finals by
/// GNU ld, lld and mortar.
fn passes(t: &Tools, dir: &Path, log: &mut String) -> Vec<String> {
    let mut bad = Vec::new();
    let finals: Vec<L> = [L::Bfd, L::Lld, L::Mortar]
        .into_iter()
        .filter(|l| *l != L::Mortar || t.mortar.is_some())
        .collect();
    if t.mortar.is_none() {
        let _ = writeln!(log, "{:?}: mortar finals skipped: mortar is not built", t.arch);
    }
    for c in &COMPS {
        for rl in [L::Bfd, L::Lld] {
            let (rpath, r) = match partial(t, rl, dir, c, &[], "") {
                Ok(x) => x,
                Err(e) => {
                    bad.push(format!("{:?} {} -r {}: {e}", t.arch, c.name, rl.name()));
                    continue;
                }
            };
            for &fl in &finals {
                if fl == L::Mortar && c.name == "cctors" {
                    let _ = writeln!(
                        log,
                        "{:?} cctors: no mortar final (.ctors is refused by name)",
                        t.arch
                    );
                    continue;
                }
                let dout = format!("d.{}", fl.name());
                let vout = format!("v.{}.{}.{}", c.name, rl.name(), fl.name());
                let d = final_link(t, fl, dir, &dout, &[], line_of(fl));
                let v = final_link(t, fl, dir, &vout, &[], &via_line(line_of(fl), c, &rpath));
                let (d, v) = match (d, v) {
                    (Ok(d), Ok(v)) => (d, v),
                    (d, v) => {
                        for e in [d.err(), v.err()].into_iter().flatten() {
                            let _ = writeln!(
                                log,
                                "{:?} {} {}->{}: link refused: {e}",
                                t.arch,
                                c.name,
                                rl.name(),
                                fl.name()
                            );
                            if fl != L::Mortar {
                                bad.push(e);
                            }
                        }
                        continue;
                    }
                };
                let rep = p1(&d, &v, &r, &rpath);
                let cov = rep.coverage.composition.clone().unwrap_or_default();
                let f = p1_findings(&rep);
                let _ = writeln!(
                    log,
                    "{:?} {} -r {} final {}: judged {}, {} findings; {}",
                    t.arch,
                    c.name,
                    rl.name(),
                    fl.name(),
                    cov.judged,
                    f.len(),
                    rep.summary()
                        .lines()
                        .filter(|l| l.starts_with("composition"))
                        .collect::<Vec<_>>()
                        .join("; ")
                );
                for m in &f {
                    let _ = writeln!(log, "    {m}");
                }
                // WP-203 (the supervisor's ruling, 2026-11-17): mortar's
                // final link makes lld's `-r` `COMMON` section (A120) an
                // orphan output section; GNU ld and lld put it in .bss.
                // P-P1 names it, exactly, and nothing else on that cell;
                // when WP-203 lands this expectation fails: delete it.
                let wp203 = rl == L::Lld && fl == L::Mortar && c.name == "compc";
                let f: Vec<String> = if wp203 {
                    let (named, rest): (Vec<String>, Vec<String>) = f
                        .into_iter()
                        .partition(|m| m.contains(", COMMON (type 0x8, flags 0x3) through -r"));
                    if named.len() != 1 {
                        bad.push(format!("{:?} {} -r lld final mortar: WP-203: input section COMMON is no longer an orphan in mortar's final link? (the expected P-P1 finding is not shown)", t.arch, c.name));
                    }
                    rest
                } else {
                    f
                };
                if cov.judged != 1 || !f.is_empty() {
                    bad.push(format!(
                        "{:?} {} -r {} final {}: {f:?}",
                        t.arch,
                        c.name,
                        rl.name(),
                        fl.name()
                    ));
                }
            }
        }
    }
    bad
}

/// A mutation of `V`: a name, the item or tolerance it is for, the edit
/// of `V`'s output and map text, and what P-P1 must say (a substring of
/// one finding; "" for none).
struct Mutant {
    name: &'static str,
    edit: fn(&mut Ed, &mut String) -> bool,
    expect: &'static str,
}

fn sec_named(e: &Ed, name: &str) -> Option<usize> {
    e.find(name)
}

/// (21) a `.init_array` pointer's word in `V` increased by 8: item 3.
fn m21_word(e: &mut Ed, _: &mut String) -> bool {
    let Some(i) = sec_named(e, ".init_array") else { return false };
    let sh = e.sh(i);
    let w = if e.c64 { 8 } else { 4 };
    let at = sh.offset as usize;
    let x = e.rd(at, w);
    e.wr(at, w, x.wrapping_add(8));
    true
}

/// (21) a global's `.symtab` value moved by 4: item 4.
fn m21_sym(e: &mut Ed, _: &mut String) -> bool {
    let Some(k) = e.sym_by_name("g_data") else { return false };
    let mut y = e.sym(k);
    y.value += 4;
    e.set_sym(k, &y);
    true
}

/// The address token after section `name`'s line of input `file` in a
/// GNU map: its byte range.
fn map_addr(text: &str, name: &str, file: &str) -> Option<(usize, usize)> {
    let mut from = 0;
    while let Some(k) = text[from..].find(name) {
        let at = from + k;
        from = at + name.len();
        let after = &text[from..];
        if !after.starts_with(|c: char| c.is_whitespace()) || !text[..at].ends_with(' ') {
            continue;
        }
        let a = after.find("0x")?;
        let end = after[a..].find(|c: char| c.is_whitespace())? + a;
        let rest = &after[end..];
        let line_end = rest.find('\n').unwrap_or(rest.len());
        if rest[..line_end].contains(file) {
            return Some((from + a, from + end));
        }
    }
    None
}

/// (21) two pieces of `V`'s `.text` swapped, their bytes and their map
/// places: item 2.
fn m21_swap(e: &mut Ed, map: &mut String) -> bool {
    let (Some(a), Some(b)) = (e.sym_by_name("hid"), e.sym_by_name("wk_only")) else { return false };
    let (ya, yb) = (e.sym(a), e.sym(b));
    if ya.size != yb.size || ya.size == 0 {
        return false;
    }
    let Some(ti) = sec_named(e, ".text") else { return false };
    let sh = e.sh(ti);
    let off = |v: u64| (v - sh.addr + sh.offset) as usize;
    let n = ya.size as usize;
    let (pa, pb) = (off(ya.value), off(yb.value));
    let ba = e.b[pa..pa + n].to_vec();
    let bb = e.b[pb..pb + n].to_vec();
    e.b[pa..pa + n].copy_from_slice(&bb);
    e.b[pb..pb + n].copy_from_slice(&ba);
    let (Some(ra), Some(rb)) =
        (map_addr(map, ".text.hid", "compc"), map_addr(map, ".text.wk_only", "compc"))
    else {
        return false;
    };
    let (sa, sb) = (map[ra.0..ra.1].to_string(), map[rb.0..rb.1].to_string());
    if sa.len() != sb.len() {
        return false;
    }
    map.replace_range(ra.0..ra.1, &sb);
    map.replace_range(rb.0..rb.1, &sa);
    true
}

/// Item 1: `.comment`'s `SHF_MERGE` cleared.
fn m_item1(e: &mut Ed, _: &mut String) -> bool {
    let Some(i) = sec_named(e, ".comment") else { return false };
    let mut sh = e.sh(i);
    sh.flags ^= 0x10;
    e.set_sh(i, &sh);
    true
}

/// Item 4: a global's size.
fn m_item4_size(e: &mut Ed, _: &mut String) -> bool {
    let Some(k) = e.sym_by_name("wk_only") else { return false };
    let mut y = e.sym(k);
    y.size += 1;
    e.set_sym(k, &y);
    true
}

/// Just outside (t1): a byte of a component piece no record covers.
fn m_t1(e: &mut Ed, _: &mut String) -> bool {
    let Some(k) = e.sym_by_name("wk_only") else { return false };
    let y = e.sym(k);
    let Some(ti) = sec_named(e, ".text") else { return false };
    let sh = e.sh(ti);
    let at = (y.value - sh.addr + sh.offset) as usize;
    e.b[at] ^= 0x01;
    true
}

/// Inside (t2): a local's value moved; nothing to find.
fn m_t2_in(e: &mut Ed, _: &mut String) -> bool {
    let Some(k) = (1..e.nsyms()).find(|&k| e.sym(k).bind() == 0 && e.sym(k).ty() == 1) else {
        return false;
    };
    let mut y = e.sym(k);
    y.value += 4;
    e.set_sym(k, &y);
    true
}

/// Just outside (t2): a global made weak.
fn m_t2_out(e: &mut Ed, _: &mut String) -> bool {
    let Some(k) = e.sym_by_name("g_data") else { return false };
    let mut y = e.sym(k);
    y.info = (2 << 4) | (y.info & 0xf);
    e.set_sym(k, &y);
    true
}

/// Inside (t3): the build-id descriptor's last byte; nothing to find.
fn m_t3_in(e: &mut Ed, _: &mut String) -> bool {
    let Some(i) = sec_named(e, ".note.gnu.build-id") else { return false };
    let sh = e.sh(i);
    let at = (sh.offset + sh.size - 1) as usize;
    e.b[at] ^= 0xff;
    true
}

/// Just outside (t3): the build-id note's section made writable.
fn m_t3_out(e: &mut Ed, _: &mut String) -> bool {
    let Some(i) = sec_named(e, ".note.gnu.build-id") else { return false };
    let mut sh = e.sh(i);
    sh.flags |= 1;
    e.set_sh(i, &sh);
    true
}

const MUTANTS: &[Mutant] = &[
    Mutant { name: "(21) .init_array word +8: item 3", edit: m21_word, expect: "resolves to" },
    Mutant { name: "(21) g_data value +4: item 4", edit: m21_sym, expect: "symbol g_data:" },
    Mutant {
        name: "(21) .text pieces swapped: item 2",
        edit: m21_swap,
        expect: "in another order through -r",
    },
    Mutant {
        name: "item 1: .comment flags",
        edit: m_item1,
        expect: "the output sections differ at",
    },
    Mutant { name: "item 4: wk_only size +1", edit: m_item4_size, expect: "symbol wk_only: size" },
    Mutant {
        name: "(t1) outside: a byte no record covers",
        edit: m_t1,
        expect: "a byte no relocation covers",
    },
    Mutant { name: "(t2) inside: a local's value", edit: m_t2_in, expect: "" },
    Mutant {
        name: "(t2) outside: g_data made weak",
        edit: m_t2_out,
        expect: "symbol g_data: size",
    },
];

const BUILD_ID_MUTANTS: &[Mutant] = &[
    Mutant { name: "(t3) inside: the build-id descriptor", edit: m_t3_in, expect: "" },
    Mutant {
        name: "(t3) outside: the note's flags",
        edit: m_t3_out,
        expect: "the output sections differ at",
    },
];

/// What a mutant runs on: `D`, `V` (and `V`'s output name), the `-r`.
struct Base<'a> {
    d: &'a Read,
    v: &'a Read,
    vout: &'a str,
    r: &'a Read,
    rpath: &'a str,
}

/// Runs mutants on `V`; returns (applied, caught) and the misses. A
/// mutant is caught when P-P1 adds a finding with its text to the
/// unmutated links' (none, for the pass cells); one for no finding when
/// it adds none.
fn mutate(
    dir: &Path,
    b: &Base<'_>,
    ms: &[Mutant],
    log: &mut String,
) -> (usize, usize, Vec<String>) {
    let base = p1_findings(&p1(b.d, b.v, b.r, b.rpath));
    let (mut applied, mut caught, mut bad) = (0, 0, Vec::new());
    for m in ms {
        let mut e = Ed::new(&b.v.out);
        let mut text = std::fs::read_to_string(dir.join(format!("{}.map", b.vout))).unwrap();
        if !(m.edit)(&mut e, &mut text) {
            let _ = writeln!(log, "    not applied: {}", m.name);
            bad.push(format!("not applied: {}", m.name));
            continue;
        }
        applied += 1;
        let name = format!("{}.mut", b.vout);
        tmp(dir, &name, &e.b);
        tmp(dir, &format!("{name}.map"), text.as_bytes());
        let mv = read_as(dir, b.v.format, &name, b.v.line.clone());
        let f: Vec<String> = p1_findings(&p1(b.d, &mv, b.r, b.rpath))
            .into_iter()
            .filter(|x| !base.contains(x))
            .collect();
        let ok =
            if m.expect.is_empty() { f.is_empty() } else { f.iter().any(|x| x.contains(m.expect)) };
        let shown = f.iter().find(|x| m.expect.is_empty() || x.contains(m.expect)).or(f.first());
        let _ = writeln!(
            log,
            "    {}: {} ({} new findings{})",
            m.name,
            if ok { "caught" } else { "MISSED" },
            f.len(),
            shown.map(|x| format!(": {x}")).unwrap_or_default()
        );
        if ok {
            caught += 1;
        } else {
            bad.push(format!("{}: {f:?}", m.name));
        }
    }
    (applied, caught, bad)
}

/// The mutants, on GNU ld's `-r` of the C component and GNU ld's finals.
fn mutants(t: &Tools, dir: &Path, log: &mut String) -> Vec<String> {
    let c = &COMPS[0];
    let mut bad = Vec::new();
    let Ok((rpath, r)) = partial(t, L::Bfd, dir, c, &[], "") else {
        return vec![format!("{:?}: bfd -r", t.arch)];
    };
    for (opts, ms, tag) in
        [(&[][..], MUTANTS, ""), (&["--build-id=sha1"][..], BUILD_ID_MUTANTS, ".bid")]
    {
        let d = final_link(t, L::Bfd, dir, &format!("md{tag}"), opts, &LINE);
        let vout = format!("mv{tag}");
        let v = final_link(t, L::Bfd, dir, &vout, opts, &via_line(&LINE, c, &rpath));
        let (Ok(d), Ok(v)) = (d, v) else {
            bad.push(format!("{:?}: mutant finals{tag}", t.arch));
            continue;
        };
        let base = p1(&d, &v, &r, &rpath);
        let _ = writeln!(
            log,
            "{:?} mutants{tag}: base {} findings; {}",
            t.arch,
            p1_findings(&base).len(),
            base.summary()
                .lines()
                .filter(|l| l.starts_with("composition tol"))
                .collect::<Vec<_>>()
                .join("")
        );
        if !p1_findings(&base).is_empty() {
            bad.push(format!("{:?} mutant base{tag}: {:?}", t.arch, p1_findings(&base)));
        }
        if tag == ".bid" && !base.summary().contains("(t3)") {
            bad.push(format!("{:?}: the build-id links do not use (t3)", t.arch));
        }
        // GNU ld's finals through GNU ld's or lld's `-r` keep the same
        // locals and `.strtab`: (t2) is needed by lld's finals (the pass
        // cells, checked in `p_p1`) and by the mutant `(t2) inside`.
        let (a, k, mut b) =
            mutate(dir, &Base { d: &d, v: &v, vout: &vout, r: &r, rpath: &rpath }, ms, log);
        let _ = writeln!(log, "{:?} mutants{tag}: {a} applied, {k} caught", t.arch);
        bad.append(&mut b);
    }
    bad
}

/// The exclusions (a)-(d), each named.
fn exclusions(t: &Tools, dir: &Path, log: &mut String) -> Vec<String> {
    let mut bad = Vec::new();
    let c = &COMPS[0];
    let mut want = |letter: char, rep: &Report, bad: &mut Vec<String>| {
        let tag = format!("not judged: relocatable composition ({letter})");
        let ok = rep.summary().contains(&tag) && p1_findings(rep).is_empty();
        let _ = writeln!(
            log,
            "{:?} exclusion ({letter}): {}",
            t.arch,
            if ok { "named" } else { "MISSED" }
        );
        if !ok {
            bad.push(format!("{:?} exclusion ({letter}): {}", t.arch, rep.summary()));
        }
    };
    // (a) and (d): the -r extracts `rec` from librec.a; L does not.
    if let Ok((rpath, r)) = partial(t, L::Bfd, dir, c, &["librec.a"], ".ar") {
        let mut dl: Vec<&str> = LINE.to_vec();
        dl.push("librec.a");
        let mut vl = via_line(&LINE, c, &rpath);
        vl.push("librec.a");
        for (letter, opts, tag) in [('a', &[][..], "a"), ('d', &["--exclude-libs=ALL"][..], "d")] {
            let d = final_link(t, L::Bfd, dir, &format!("xd{tag}"), opts, &dl);
            let v = final_link(t, L::Bfd, dir, &format!("xv{tag}"), opts, &vl);
            match (d, v) {
                (Ok(d), Ok(v)) => want(letter, &p1(&d, &v, &r, &rpath), &mut bad),
                (d, v) => bad.push(format!("({letter}) links: {:?} {:?}", d.err(), v.err())),
            }
        }
    } else {
        bad.push("(a) -r".into());
    }
    // (b): --gc-sections, and the -r merges pieces of two files by name.
    if let Ok((rpath, r)) = partial(t, L::Bfd, dir, c, &[], "") {
        let d = final_link(t, L::Bfd, dir, "xdb", &["--gc-sections"], &LINE);
        let v = final_link(t, L::Bfd, dir, "xvb", &["--gc-sections"], &via_line(&LINE, c, &rpath));
        match (d, v) {
            (Ok(d), Ok(v)) => want('b', &p1(&d, &v, &r, &rpath), &mut bad),
            (d, v) => bad.push(format!("(b) links: {:?} {:?}", d.err(), v.err())),
        }
    }
    // (c): --wrap=hid; the component defines hid (c1) and references it (c2).
    if let Ok((rpath, r)) = partial(t, L::Bfd, dir, c, &[], "") {
        let d = final_link(t, L::Bfd, dir, "xdc", &["--wrap=hid"], &LINE);
        let v = final_link(t, L::Bfd, dir, "xvc", &["--wrap=hid"], &via_line(&LINE, c, &rpath));
        match (d, v) {
            (Ok(d), Ok(v)) => want('c', &p1(&d, &v, &r, &rpath), &mut bad),
            (d, v) => bad.push(format!("(c) links: {:?} {:?}", d.err(), v.err())),
        }
    }
    bad
}

fn judge(arch: Arch) {
    let Some(t) = Tools::find(arch) else {
        eprintln!("skipped: the {arch:?} tools are not installed");
        return;
    };
    let scratch = Scratch::new(&format!("plumb-compose-{}", arch.name()));
    let dir = scratch.path();
    build(&t, dir).unwrap_or_else(|e| panic!("{arch:?}: {e}"));
    let mut log = String::new();
    let mut bad = passes(&t, dir, &mut log);
    for tol in ["(t1)", "(t2)"] {
        if !log.lines().any(|l| l.contains("composition tolerances") && l.contains(tol)) {
            bad.push(format!("{arch:?}: no pass cell uses {tol}"));
        }
    }
    bad.extend(mutants(&t, dir, &mut log));
    bad.extend(exclusions(&t, dir, &mut log));
    eprintln!("{log}");
    assert!(bad.is_empty(), "{arch:?}:\n{}", bad.join("\n"));
}

#[test]
fn p_p1_aarch64() {
    judge(Arch::Aarch64);
}

#[test]
fn p_p1_x86_64() {
    judge(Arch::X86_64);
}

#[test]
fn p_p1_i386() {
    judge(Arch::I386);
}

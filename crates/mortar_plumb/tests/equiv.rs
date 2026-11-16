//! The equivalence of an update and a full link (`spec/plumb.md` §6.9;
//! `spec/incr.md` IN-2, IN-62): hand-built pairs, each ACCEPTED or
//! REJECTED by a named P-E finding.
//!
//! `X` is modelled as in `tests/dead.rs`: a GNU ld link with one more
//! input (`pad.o`, code and data) whose file leaves the map; its ranges
//! become `hole` lines holding their fill, so every later unit sits at
//! another address. `F` is the GNU ld link of the same inputs without it
//! (a plain map: any map is an `F`). R13's two cases first: `X` against
//! `F` (other addresses, the same order) ACCEPTED; `X` against the link
//! of `main.o b.o a.o` (the `myset` array in another order) REJECTED by
//! P-E5.

#![allow(clippy::many_single_char_names)]

use std::path::Path;

use mortar_plumb::map::{Chunk, ChunkKind, DeadRange, Map, Placed, SectionRef};
use mortar_plumb::{Format, Input};
use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, run_with};

fn run(dir: &Path, program: &Path, args: &[&str]) {
    let mut c = std::process::Command::new(program);
    c.current_dir(dir).args(args);
    let out = run_with(&mut c, b"", mortar_testutil::DEFAULT_TIMEOUT);
    assert!(
        out.success(),
        "{} {} failed: {}",
        program.display(),
        args.join(" "),
        out.stderr_text()
    );
}

/// The sources: `main.s` calls `afun` and `bfun` and points at `avar`,
/// `bvar` and `bfun`; `a.s` and `b.s` each add a function, a variable and
/// a `myset` entry (a `__start_`-reachable, packed array); `pad.s` is the
/// dead version of a file.
fn sources(arch: Arch, extra_main: &str, extra_a: &str) -> Vec<(&'static str, String)> {
    let (word, sec, ty) = match arch {
        Arch::Aarch64 => (".quad", "%progbits", "%function"),
        Arch::X86_64 => (".quad", "@progbits", "@function"),
        Arch::I386 => (".long", "@progbits", "@function"),
    };
    let (call, ret, fill) = match arch {
        Arch::Aarch64 => ("bl", "\tret\n", ".fill 8,4,0"),
        _ => ("call", "\tret\n", ".fill 32,1,0xcc"),
    };
    let spin = if arch == Arch::Aarch64 { "\tb .\n" } else { "\thlt\n" };
    let main = format!(
        "\t.text\n\t.globl _start\n\t.type _start,{ty}\n_start:\n\t{call} afun\n\t{call} bfun\n{extra_main}{spin}\
         \t.section .init,\"ax\",{sec}\n\t.balign 4\n\tnop\n\
         \t.data\n\t.globl ptrs\nptrs:\n\t{word} avar\n\t{word} bvar+4\n\t{word} bfun\n\t{word} __start_myset\n"
    );
    let unit = |n: &str, extra: &str| {
        format!(
            "\t.text\n\t.globl {n}fun\n\t.type {n}fun,{ty}\n{n}fun:\n{ret}{extra}\t.size {n}fun, .-{n}fun\n\
             \t.data\n\t.balign 8\n\t.globl {n}var\n\t.type {n}var,%object\n{n}var:\t.fill 8,1,7\n\t.size {n}var, 8\n\
             \t.section myset,\"aw\",{sec}\n\t.balign {a}\n\t{word} {n}fun\n",
            a = if arch == Arch::I386 { 4 } else { 8 }
        )
    };
    let pad = format!("\t.text\n\t{fill}\n\t.data\n\t.fill 24,1,0\n");
    vec![("main", main), ("a", unit("a", extra_a)), ("b", unit("b", "")), ("pad", pad)]
}

/// A link and its mortar map.
#[derive(Clone)]
struct Link {
    output: Vec<u8>,
    map: Map,
    secs: Vec<(String, u64, u64, u64)>,
}

impl Link {
    fn sec(&self, name: &str) -> (u64, u64, u64) {
        let s = self.secs.iter().find(|s| s.0 == name).unwrap_or_else(|| panic!("no {name}"));
        (s.1, s.2, s.3)
    }
    /// The file offset of address `a`.
    fn off(&self, a: u64) -> usize {
        let s = self
            .secs
            .iter()
            .find(|s| s.1 != 0 && s.1 <= a && a < s.1 + s.2)
            .unwrap_or_else(|| panic!("{a:#x} is in no section"));
        (s.3 + (a - s.1)) as usize
    }
    fn placed(&self, path: &str, name: &str) -> (u64, u64) {
        let fi = self.map.files.iter().position(|f| f.path == path).unwrap() as u32;
        let p = self
            .map
            .placed
            .iter()
            .find(|p| p.file == fi && p.section == SectionRef::Name(name.into()))
            .unwrap_or_else(|| panic!("{path} {name}"));
        (p.addr, p.size)
    }
    fn symbol(&self, name: &str) -> u64 {
        let h = mortar_elf::FileHeader::parse(&self.output).unwrap();
        let t = mortar_elf::SectionTable::parse(&self.output, &h).unwrap();
        let s = t.find_symbol_table(mortar_elf::consts::SHT_SYMTAB).unwrap().unwrap();
        s.iter().find(|(_, y)| s.name(y).ok() == Some(name.as_bytes())).unwrap().1.st_value
    }
}

fn sections(bytes: &[u8]) -> Vec<(String, u64, u64, u64)> {
    let h = mortar_elf::FileHeader::parse(bytes).unwrap();
    let t = mortar_elf::SectionTable::parse(bytes, &h).unwrap();
    t.iter()
        .map(|s| {
            let n = String::from_utf8_lossy(s.name().unwrap_or(b"")).into_owned();
            (n, s.header().sh_addr, s.header().sh_size, s.header().sh_offset)
        })
        .collect()
}

/// Removes `path` from the map with every record of it.
fn drop_file(map: &mut Map, path: &str) -> Vec<Placed> {
    let d = map.files.iter().position(|f| f.path == path).unwrap() as u32;
    map.files.remove(d as usize);
    let (gone, kept): (Vec<Placed>, Vec<Placed>) = map.placed.drain(..).partition(|p| p.file == d);
    map.placed = kept;
    let re = |f: &mut u32| {
        if *f > d {
            *f -= 1;
        }
    };
    map.placed.iter_mut().for_each(|p| re(&mut p.file));
    map.discarded.retain(|p| p.file != d);
    map.discarded.iter_mut().for_each(|p| re(&mut p.file));
    map.nonalloc.retain(|p| p.file != d);
    map.nonalloc.iter_mut().for_each(|p| re(&mut p.file));
    gone
}

/// One pair's directory: the objects, and links of them.
struct Pair {
    arch: Arch,
    dir: Scratch,
    files: Vec<(String, Vec<u8>)>,
}

fn assemble(
    arch: Arch,
    label: &str,
    extra_main: &str,
    extra_a: &str,
    asm_args: &[&str],
) -> Option<Pair> {
    let asm = arch.tool("as")?;
    arch.tool("ld.bfd")?;
    let dir = Scratch::new(&format!("equiv-{label}-{}", arch.triple()));
    let mut files = Vec::new();
    for (name, text) in sources(arch, extra_main, extra_a) {
        std::fs::write(dir.join(format!("{name}.s")), text).unwrap();
        let o = format!("{name}.o");
        let mut args = vec![format!("{name}.s"), "-o".into(), o.clone()];
        args.extend(asm_args.iter().map(|s| (*s).to_string()));
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        run(dir.path(), &asm, &args);
        files.push((o.clone(), std::fs::read(dir.join(&o)).unwrap()));
    }
    Some(Pair { arch, dir, files })
}

impl Pair {
    /// GNU ld's link of `objs`, its map in mortar's form.
    fn link(&self, objs: &[&str], extra: &[&str]) -> Link {
        let ld = self.arch.tool("ld.bfd").unwrap();
        let mut args = vec!["-x", "-e", "_start", "-o", "out", "-Map=out.map", "-z", "noexecstack"];
        args.extend(extra);
        args.extend(objs);
        run(self.dir.path(), &ld, &args);
        let output = std::fs::read(self.dir.join("out")).unwrap();
        let text = std::fs::read_to_string(self.dir.join("out.map")).unwrap();
        let mut map = mortar_plumb::map::parse(Format::Gnu, &text).unwrap();
        map.format = Format::Mortar;
        // GNU ld lists its own sections (`.iplt`, `.got`, ...) under the
        // first file: in mortar's map they are synthetic chunks.
        let names: Vec<Vec<String>> = map
            .files
            .iter()
            .map(|f| {
                let b = std::fs::read(self.dir.join(&f.path)).unwrap_or_default();
                if b.starts_with(b"\x7fELF") {
                    sections(&b).into_iter().map(|s| s.0).collect()
                } else {
                    Vec::new()
                }
            })
            .collect();
        let mut chunks = Vec::new();
        map.placed.retain(|q| match &q.section {
            SectionRef::Name(n) if !names[q.file as usize].contains(n) => {
                if q.size > 0 {
                    chunks.push(Chunk {
                        kind: ChunkKind::Synthetic,
                        name: n.clone(),
                        addr: Some(q.addr),
                        size: q.size,
                        target: None,
                        line: 0,
                    });
                }
                false
            }
            _ => true,
        });
        map.chunks.extend(chunks);
        let secs = sections(&output);
        Link { output, map, secs }
    }

    /// `X`: the link with `pad.o` after `main.o`, `pad.o` out of the map,
    /// its ranges holes holding their fill.
    fn update(&self, extra: &[&str]) -> Link {
        let mut x = self.link(&["main.o", "pad.o", "a.o", "b.o"], extra);
        x.map.incremental = true;
        for p in drop_file(&mut x.map, "pad.o").into_iter().filter(|p| p.size > 0) {
            let (name, addr, _, _) = x
                .secs
                .iter()
                .find(|s| s.1 != 0 && s.1 <= p.addr && p.addr + p.size <= s.1 + s.2)
                .unwrap()
                .clone();
            let fill = if name == ".text" && self.arch != Arch::Aarch64 { 0xcc } else { 0 };
            let o = x.off(p.addr);
            x.output[o..o + p.size as usize].fill(fill);
            x.map.dead.push(DeadRange {
                section: name,
                offset: p.addr - addr,
                size: p.size,
                kind: None,
                line: 0,
            });
        }
        x
    }

    /// GNU ld's map places `.eh_frame` per file; mortar's per record (a
    /// piece each, §3.2). Splits each placement by its input's records
    /// (GNU ld keeps them in order when one file has them, as here).
    fn eh_pieces(&self, mut l: Link) -> Link {
        let mut add = Vec::new();
        l.map.placed.retain(|q| {
            if q.section != SectionRef::Name(".eh_frame".into()) || q.size == 0 {
                return true;
            }
            let path = &l.map.files[q.file as usize].path;
            let b = &self.files.iter().find(|f| &f.0 == path).unwrap().1;
            let (_, _, size, off) = sections(b).into_iter().find(|s| s.0 == ".eh_frame").unwrap();
            let eh = &b[off as usize..(off + size) as usize];
            let mut o = 0usize;
            while o + 4 <= eh.len() {
                let n = u32::from_le_bytes(eh[o..o + 4].try_into().unwrap()) as usize;
                if n == 0 {
                    break;
                }
                add.push(Placed {
                    input_offset: Some(o as u64),
                    addr: q.addr + o as u64,
                    size: n as u64 + 4,
                    ..q.clone()
                });
                o += n + 4;
            }
            false
        });
        l.map.placed.extend(add);
        l
    }

    /// `equiv(X, F)` through the maps' text form: the P-E findings
    /// (`"<id> <message>"`) and each side's own finding count.
    fn equiv(&self, x: &Link, f: &Link) -> (Vec<String>, usize, usize) {
        let text = |m: &Map| {
            if m.format != Format::Mortar {
                return m.clone();
            }
            mortar_plumb::map::parse(Format::Mortar, &mortar_plumb::map::mortar::write(m)).unwrap()
        };
        let (xm, fm) = (text(&x.map), text(&f.map));
        let inputs: Vec<Input<'_>> =
            self.files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
        let e = mortar_plumb::equiv::check(&x.output, &xm, &f.output, &fm, &inputs);
        let pe = e.findings.iter().map(|g| format!("{} {}", g.rule.id(), g.message)).collect();
        let side = |r: &mortar_plumb::Report| {
            for g in &r.findings {
                eprintln!("side finding: {} {}", g.rule.id(), g.message);
            }
            r.findings.len()
        };
        (pe, side(&e.x), side(&e.f))
    }
}

fn archs() -> [Arch; 3] {
    [Arch::Aarch64, Arch::X86_64, Arch::I386]
}

fn has(found: &[String], id: &str, needle: &str) -> bool {
    found.iter().any(|f| f.starts_with(&format!("{id} ")) && f.contains(needle))
}

fn word(arch: Arch) -> usize {
    if arch == Arch::I386 { 4 } else { 8 }
}

// ---------------------------------------------------------------------
// R13's two cases.

// R13 case 1: the update (other addresses, the same order) is ACCEPTED,
// on both sides clean; every unit of a.o and b.o moved.
#[test]
fn r13_relink_at_other_addresses_is_accepted() {
    for arch in archs() {
        let Some(p) = assemble(arch, "r13", "", "", &[]) else { return };
        let x = p.update(&[]);
        let f = p.link(&["main.o", "a.o", "b.o"], &[]);
        assert_ne!(x.placed("a.o", ".text").0, f.placed("a.o", ".text").0, "{arch:?}: a.o moved");
        let (pe, xs, fs) = p.equiv(&x, &f);
        assert!(pe.is_empty() && xs == 0 && fs == 0, "{arch:?}: {pe:#?} (X {xs}, F {fs})");
        // F's own update against itself: equivalent too.
        let (pe, _, _) = p.equiv(&x, &x);
        assert!(pe.is_empty(), "{arch:?}: X against itself {pe:#?}");
    }
}

// R13 case 2: against `main.o b.o a.o`, whose `myset` holds b's entry
// first: REJECTED by item 5 (packed order), though both links verify.
#[test]
fn r13_packed_section_in_another_order_is_rejected() {
    for arch in archs() {
        let Some(p) = assemble(arch, "r13b", "", "", &[]) else { return };
        let x = p.update(&[]);
        let f = p.link(&["main.o", "b.o", "a.o"], &[]);
        let (pe, xs, fs) = p.equiv(&x, &f);
        assert!(xs == 0 && fs == 0, "{arch:?}: both sides verify");
        assert!(has(&pe, "P-E5", "packed section myset"), "{arch:?}: {pe:#?}");
    }
}

// ---------------------------------------------------------------------
// One mutant per item.

// The one-byte control: a code byte no relocation covers, flipped in X's
// `bfun`: REJECTED (P-E4; X's own S12 says it too).
#[test]
fn p_e4_one_flipped_code_byte_is_rejected() {
    for arch in archs() {
        let Some(p) = assemble(arch, "byte", "", "", &[]) else { return };
        let mut x = p.update(&[]);
        let f = p.link(&["main.o", "a.o", "b.o"], &[]);
        let at = x.off(x.symbol("bfun"));
        x.output[at] ^= 0x10;
        let (pe, _, _) = p.equiv(&x, &f);
        assert!(has(&pe, "P-E4", "a byte no relocation covers differs"), "{arch:?}: {pe:#?}");
    }
}

// Item 1: a section's flags and a segment's flags differ.
#[test]
fn p_e1_sections_and_segments() {
    for arch in archs() {
        let Some(p) = assemble(arch, "e1", "", "", &[]) else { return };
        let x = p.update(&[]);
        let f = p.link(&["main.o", "a.o", "b.o"], &[]);
        // `.data`'s sh_flags gain SHF_EXECINSTR.
        let h = mortar_elf::FileHeader::parse(&x.output).unwrap();
        let idx = x.secs.iter().position(|s| s.0 == ".data").unwrap();
        let w = word(arch);
        let at = h.e_shoff as usize + idx * usize::from(h.e_shentsize) + 8;
        let mut m = x.clone();
        m.output[at] |= 4;
        let (pe, _, _) = p.equiv(&m, &f);
        assert!(has(&pe, "P-E1", "section"), "{arch:?}: {pe:#?}");
        // The first PT_LOAD's p_flags gain PF_W.
        let flags = h.e_phoff as usize + if w == 8 { 4 } else { 24 };
        let mut m = x.clone();
        m.output[flags] |= 2;
        let (pe, _, _) = p.equiv(&m, &f);
        assert!(has(&pe, "P-E1", "program header 0"), "{arch:?}: {pe:#?}");
        // e_entry moved one instruction on.
        let mut m = x.clone();
        let e = 24;
        let v = u64::from_le_bytes(m.output[e..e + 8].try_into().unwrap());
        let n = if w == 8 {
            (v + 4).to_le_bytes().to_vec()
        } else {
            ((v as u32) + 4).to_le_bytes().to_vec()
        };
        m.output[e..e + n.len()].copy_from_slice(&n);
        let (pe, _, _) = p.equiv(&m, &f);
        assert!(has(&pe, "P-E1", "e_entry"), "{arch:?}: {pe:#?}");
    }
}

// Item 2: a unit placed in F only (X's map lost b.o's `.data`, its
// range declared a hole), and a piece placed twice.
#[test]
fn p_e2_live_units() {
    for arch in archs() {
        let Some(p) = assemble(arch, "e2", "", "", &[]) else { return };
        let x = p.update(&[]);
        let f = p.link(&["main.o", "a.o", "b.o"], &[]);
        let mut m = x.clone();
        let fi = m.map.files.iter().position(|q| q.path == "b.o").unwrap() as u32;
        m.map.placed.retain(|q| !(q.file == fi && q.section == SectionRef::Name(".data".into())));
        let (pe, _, _) = p.equiv(&m, &f);
        assert!(
            has(&pe, "P-E2", "b.o()@None:.data is placed in the full link, not in the update"),
            "{arch:?}: {pe:#?}"
        );
        // F has an extra unit X lacks, the other way round.
        let (pe, _, _) = p.equiv(&x, &{
            let mut g = f.clone();
            g.map
                .placed
                .retain(|q| !(q.file == fi && q.section == SectionRef::Name(".data".into())));
            g
        });
        assert!(
            has(&pe, "P-E2", "placed in the update, not in the full link"),
            "{arch:?}: {pe:#?}"
        );
    }
}

// Item 3: a global naming another definition (bvar's value made avar's,
// both placed: Y1 alone would accept a value equal to ANY placed
// definition of the name; P-E3 asks for the same one), and a global in
// X's `.symtab` only.
#[test]
fn p_e3_symbols() {
    for arch in archs() {
        let Some(p) = assemble(arch, "e3", "", "", &[]) else { return };
        let x = p.update(&[]);
        let f = p.link(&["main.o", "a.o", "b.o"], &[]);
        let h = mortar_elf::FileHeader::parse(&x.output).unwrap();
        let t = mortar_elf::SectionTable::parse(&x.output, &h).unwrap();
        let s = t.find_symbol_table(mortar_elf::consts::SHT_SYMTAB).unwrap().unwrap();
        let (sym_off, ent) = {
            let st = x.secs.iter().find(|q| q.0 == ".symtab").unwrap();
            (st.3 as usize, if word(arch) == 8 { 24 } else { 16 })
        };
        let idx = s.iter().find(|(_, y)| s.name(y).ok() == Some(b"bvar".as_slice())).unwrap().0.0
            as usize;
        let val = sym_off + idx * ent + if ent == 24 { 8 } else { 4 };
        let mut m = x.clone();
        let av = x.symbol("bfun");
        let n =
            if ent == 24 { av.to_le_bytes().to_vec() } else { (av as u32).to_le_bytes().to_vec() };
        m.output[val..val + n.len()].copy_from_slice(&n);
        let (pe, _, _) = p.equiv(&m, &f);
        assert!(
            has(&pe, "P-E3", "the global 'bvar' names another definition"),
            "{arch:?}: {pe:#?}"
        );
        // A global renamed in X (`bvar` -> `bvaz`): in one link only.
        let name_at = {
            let strtab = x.secs.iter().find(|q| q.0 == ".strtab").unwrap().3 as usize;
            let st_name =
                u32::from_le_bytes(x.output[sym_off + idx * ent..][..4].try_into().unwrap());
            strtab + st_name as usize + 3
        };
        let mut m = x.clone();
        m.output[name_at] = b'z';
        let (pe, _, _) = p.equiv(&m, &f);
        assert!(has(&pe, "P-E3", "'bvaz' is only in the update"), "{arch:?}: {pe:#?}");
        assert!(has(&pe, "P-E3", "'bvar' is only in the full link"), "{arch:?}: {pe:#?}");
    }
}

// Item 4: a relocated field whose value names another place in X (its
// record judged on X's side: the side's finding), and a word S12 never
// sees: the `.init` piece's byte.
#[test]
fn p_e4_relocated_values() {
    for arch in archs() {
        let Some(p) = assemble(arch, "e4", "", "", &[]) else { return };
        let x = p.update(&[]);
        let f = p.link(&["main.o", "a.o", "b.o"], &[]);
        // ptrs[1] = bvar+4 made bvar+5 in X: X's R1, and a record verified
        // in F but not in X (P-E4 1).
        let (ptrs, _) = x.placed("main.o", ".data");
        let mut m = x.clone();
        let o = m.off(ptrs + word(arch) as u64);
        m.output[o] = m.output[o].wrapping_add(1);
        let (pe, xs, _) = p.equiv(&m, &f);
        assert!(xs > 0, "{arch:?}: X's own finding");
        let _ = pe;
        // The `.init` byte.
        let (init, _) = x.placed("main.o", ".init");
        let mut m = x.clone();
        let o = m.off(init);
        m.output[o] ^= 1;
        let (pe, _, _) = p.equiv(&m, &f);
        assert!(has(&pe, "P-E4", "a byte no relocation covers differs"), "{arch:?}: {pe:#?}");
    }
}

// Item 5: a GOT-free program has no GOT; the tables here: a note's
// descriptor (not a build id) changed in X.
#[test]
fn p_e5_notes() {
    for arch in archs() {
        let Some(p) = assemble(arch, "e5", "", "", &[]) else { return };
        let x = p.update(&["--build-id=sha1"]);
        let f = p.link(&["main.o", "a.o", "b.o"], &["--build-id=sha1"]);
        // The build ids differ (other bytes): accepted.
        let (pe, _, _) = p.equiv(&x, &f);
        assert!(!pe.iter().any(|g| g.contains("notes")), "{arch:?}: {pe:#?}");
        // The note's type changed: rejected.
        let (a, _, _) = x.sec(".note.gnu.build-id");
        let mut m = x.clone();
        let o = m.off(a) + 8;
        m.output[o] = 4;
        let (pe, _, _) = p.equiv(&m, &f);
        assert!(has(&pe, "P-E5", "the notes of .note.gnu.build-id differ"), "{arch:?}: {pe:#?}");
    }
}

// Item 6: a non-zero byte between sections in X's file.
#[test]
fn p_e6_stray_file_bytes() {
    for arch in archs() {
        let Some(p) = assemble(arch, "e6", "", "", &[]) else { return };
        let x = p.update(&[]);
        let f = p.link(&["main.o", "a.o", "b.o"], &[]);
        // The last byte before `.text`'s file image, when it is padding.
        let (_, _, to) = x.sec(".text");
        let mut used: Vec<(u64, u64)> =
            x.secs.iter().filter(|s| s.0 != ".bss").map(|s| (s.3, s.3 + s.2)).collect();
        used.sort_unstable();
        let gap = (0..to).rev().find(|o| !used.iter().any(|(a, b)| a <= o && o < b) && *o > 0x100);
        let Some(gap) = gap else { continue };
        let mut m = x.clone();
        m.output[gap as usize] = 0x5a;
        let (pe, _, _) = p.equiv(&m, &f);
        assert!(has(&pe, "P-E6", "are not zero"), "{arch:?}: {pe:#?}");
    }
}

// ---------------------------------------------------------------------
// Item 4's relaxations.

// AArch64: `ADRP`+`ADD` in F, `NOP`+`ADR` in X (W1): ACCEPTED; the same
// with the `ADR` naming the next word: REJECTED (X's own finding).
#[test]
fn p_e4_aarch64_adrp_add_to_adr_may_differ() {
    let arch = Arch::Aarch64;
    let seq = "\tadrp x0, avar\n\tadd x0, x0, :lo12:avar\n";
    let Some(p) = assemble(arch, "w1", seq, "", &[]) else { return };
    let x0 = p.update(&[]);
    let f = p.link(&["main.o", "a.o", "b.o"], &[]);
    let site = x0.symbol("_start") + 8;
    let target = x0.symbol("avar");
    let adr = |pc: u64, t: u64| -> u32 {
        let d = (t as i64 - pc as i64) as u32;
        0x1000_0000 | ((d & 3) << 29) | (((d >> 2) & 0x7ffff) << 5)
    };
    let mut x = x0.clone();
    let o = x.off(site);
    x.output[o..o + 4].copy_from_slice(&0xd503_201fu32.to_le_bytes());
    x.output[o + 4..o + 8].copy_from_slice(&adr(site + 4, target).to_le_bytes());
    let (pe, xs, _) = p.equiv(&x, &f);
    assert!(pe.is_empty() && xs == 0, "W1 in X only: {pe:#?} (X {xs})");
    let mut m = x.clone();
    m.output[o + 4..o + 8].copy_from_slice(&adr(site + 4, target + 4).to_le_bytes());
    let (_, xs, _) = p.equiv(&m, &f);
    assert!(xs > 0, "a wrong ADR is X's finding");
}

// x86-64: `GOTPCRELX` relaxed to `mov $S` (X4) in F and to `lea` (X1) in
// X, in an ET_EXEC output: REJECTED (IN-2: not distance-dependent in a
// non-PIC link). The same pair taken as ET_DYN is the rule's other side,
// `may_differ` (unit-tested in `src/equiv.rs`).
#[test]
fn p_e4_x86_64_gotpcrelx_must_not_differ_in_exec() {
    let arch = Arch::X86_64;
    let seq = "\tmovq avar@GOTPCREL(%rip), %rax\n";
    let Some(p) = assemble(arch, "x1", seq, "", &[]) else { return };
    let x0 = p.update(&[]);
    // F's form, as GNU ld wrote it (X4, `mov $S`): a rewriting GNU ld
    // writes and mortar does not (§5.9), so F's map stays GNU ld's (any
    // map is an F; the command line takes mortar maps only).
    let mut f = p.link(&["main.o", "a.o", "b.o"], &[]);
    f.map = mortar_plumb::map::parse(
        Format::Gnu,
        &std::fs::read_to_string(p.dir.join("out.map")).unwrap(),
    )
    .unwrap();
    let site = x0.symbol("_start") + 10;
    let o = x0.off(site);
    let target = x0.symbol("avar");
    let mut x = x0.clone();
    // lea target(%rip), %rax: 48 8d 05 disp32.
    let disp = (target as i64 - (site + 7) as i64) as i32;
    x.output[o..o + 3].copy_from_slice(&[0x48, 0x8d, 0x05]);
    x.output[o + 3..o + 7].copy_from_slice(&disp.to_le_bytes());
    let (pe, xs, _) = p.equiv(&x, &f);
    assert_eq!(xs, 0, "X's lea is accepted on its own side");
    assert!(has(&pe, "P-E4", "got-to-lea"), "{pe:#?}");
}

// ---------------------------------------------------------------------
// More mutants, one per clause (the mutation run over the check, §9.5).

/// A `.symtab` entry's file offset: `(offset of the entry, entry size)`.
fn sym_entry(x: &Link, name: &str) -> (usize, usize) {
    let h = mortar_elf::FileHeader::parse(&x.output).unwrap();
    let t = mortar_elf::SectionTable::parse(&x.output, &h).unwrap();
    let s = t.find_symbol_table(mortar_elf::consts::SHT_SYMTAB).unwrap().unwrap();
    let ent = if h.class == mortar_elf::Class::Elf64 { 24 } else { 16 };
    let idx = s.iter().find(|(_, y)| s.name(y).ok() == Some(name.as_bytes())).unwrap().0.0 as usize;
    (x.sec(".symtab").2 as usize + idx * ent, ent)
}

// e_flags, a unit's size, a synthetic chunk, a global made local.
#[test]
fn p_e1_to_p_e3_clauses() {
    for arch in archs() {
        let Some(p) = assemble(arch, "cl", "", "", &[]) else { return };
        let x = p.update(&[]);
        let f = p.link(&["main.o", "a.o", "b.o"], &[]);
        let mut m = x.clone();
        let at = if word(arch) == 8 { 48 } else { 36 };
        m.output[at] ^= 0x40;
        let (pe, _, _) = p.equiv(&m, &f);
        assert!(has(&pe, "P-E1", "file headers differ"), "{arch:?}: {pe:#?}");
        let mut m = x.clone();
        let fi = m.map.files.iter().position(|q| q.path == "b.o").unwrap() as u32;
        for q in m
            .map
            .placed
            .iter_mut()
            .filter(|q| q.file == fi && q.section == SectionRef::Name(".data".into()))
        {
            q.size += 8;
        }
        let (pe, _, _) = p.equiv(&m, &f);
        assert!(has(&pe, "P-E2", "has size"), "{arch:?}: {pe:#?}");
        let mut m = x.clone();
        m.map.chunks.push(Chunk {
            kind: ChunkKind::Synthetic,
            name: ".extra".into(),
            addr: None,
            size: 0,
            target: None,
            line: 0,
        });
        let (pe, _, _) = p.equiv(&m, &f);
        assert!(has(&pe, "P-E2", "chunk '.extra'"), "{arch:?}: {pe:#?}");
        // `bvar`'s binding made STB_LOCAL in X.
        let (o, ent) = sym_entry(&x, "bvar");
        let info = o + if ent == 24 { 4 } else { 12 };
        let mut m = x.clone();
        m.output[info] &= 0x0f;
        let (pe, _, _) = p.equiv(&m, &f);
        assert!(has(&pe, "P-E3", "local symbols 'bvar'"), "{arch:?}: {pe:#?}");
    }
}

// AArch64: an initial-exec load of an undefined weak TLS symbol (glibc's
// `setlocale.o` pattern, question 8). GNU ld's entry (TPREL with S = 0)
// is made mortar's 0 (a64.md §7, `T = A`) on both sides, as the maps are
// mortar's: judged on each side, the pair is ACCEPTED; X's entry made 8
// is X's R4, and the pair is REJECTED.
#[test]
fn p_e4_undefined_weak_tls_initial_exec() {
    let arch = Arch::Aarch64;
    let seq = "\t.section .tbss,\"awT\",%nobits\n\t.space 8\n\t.text\n\t.weak tv\n\tadrp x0, :gottprel:tv\n\tldr x0, [x0, :gottprel_lo12:tv]\n";
    let Some(p) = assemble(arch, "wtls", seq, "", &[]) else { return };
    let as_mortar = |l: &Link| {
        let (g, size, _) = l.sec(".got");
        let mut m = l.clone();
        let o = (0..size / 8)
            .map(|k| m.off(g + 8 * k))
            .find(|&o| m.output[o + 7] == 0xff)
            .expect("GNU ld's GOT entry holding a negative TPREL");
        m.output[o..o + 8].fill(0);
        (m, o)
    };
    let (x, o) = as_mortar(&p.update(&[]));
    let (f, _) = as_mortar(&p.link(&["main.o", "a.o", "b.o"], &[]));
    let (pe, xs, fs) = p.equiv(&x, &f);
    assert!(pe.is_empty() && xs == 0 && fs == 0, "{pe:#?}");
    let mut m = x.clone();
    m.output[o] = 8;
    let (_, xs, _) = p.equiv(&m, &f);
    assert!(xs > 0, "X's mutated TPREL entry passes");
}

// A record neither side checks (AArch64: a GOT page with no LDR to pair
// it, `got-unpaired`): REJECTED by P-E4 1 (the value is not seen; the
// conservative reading, §6.9).
#[test]
fn p_e4_a_record_neither_side_checks_is_rejected() {
    let arch = Arch::Aarch64;
    let seq = "\tadrp x0, :got:avar\n";
    let Some(p) = assemble(arch, "nc", seq, "", &[]) else { return };
    let x = p.update(&[]);
    let f = p.link(&["main.o", "a.o", "b.o"], &[]);
    let (pe, _, _) = p.equiv(&x, &f);
    assert!(has(&pe, "P-E4", "not checked"), "{pe:#?}");
}

// x86-64: a GOT kept (`addq S@GOTPCREL`, not relaxable): the update's
// GOT word aimed at another place is REJECTED by P-E5.
#[test]
fn p_e5_got_words() {
    let arch = Arch::X86_64;
    let seq = "\taddq avar@GOTPCREL(%rip), %rax\n";
    let Some(p) = assemble(arch, "got", seq, "", &["-mrelax-relocations=no"]) else { return };
    let x = p.update(&[]);
    let f = p.link(&["main.o", "a.o", "b.o"], &[]);
    let (pe, xs, fs) = p.equiv(&x, &f);
    assert!(pe.is_empty() && xs == 0 && fs == 0, "{pe:#?}");
    let (g, _, _) = x.sec(".got");
    let mut m = x.clone();
    let o = m.off(g);
    let v = u64::from_le_bytes(m.output[o..o + 8].try_into().unwrap()) + 8;
    m.output[o..o + 8].copy_from_slice(&v.to_le_bytes());
    let (pe, _, _) = p.equiv(&m, &f);
    assert!(has(&pe, "P-E5", "the .got word"), "{pe:#?}");
}

// A non-allocated section of units: `.myinfo` in a.o. X's map without
// the unit's last two bytes (its size made 2) leaves them in no unit:
// REJECTED by P-E6 (and P-E2: the size).
#[test]
fn p_e6_non_alloc_tiling() {
    for arch in archs() {
        let sec = if arch == Arch::Aarch64 { "%progbits" } else { "@progbits" };
        let extra = format!("\t.section .myinfo,\"\",{sec}\n\t.long 0x11223344\n\t.text\n");
        let Some(p) = assemble(arch, "na", "", &extra, &[]) else { return };
        // GNU ld's map gives the non-allocated piece without its offset:
        // mortar's `nplace` (with it) replaces it on both sides.
        let nplace = |l: &mut Link, size: u64| {
            let fi = l.map.files.iter().position(|q| q.path == "a.o").unwrap() as u32;
            let myinfo = SectionRef::Name(".myinfo".into());
            l.map.placed.retain(|q| !(q.file == fi && q.section == myinfo));
            l.map.nonalloc.retain(|q| !(q.file == fi && q.section == myinfo));
            l.map.nonalloc.push(mortar_plumb::map::NonAlloc {
                file: fi,
                section: myinfo,
                out: ".myinfo".into(),
                offset: Some(0),
                size,
                line: 0,
            });
        };
        let mut x = p.update(&[]);
        let mut f = p.link(&["main.o", "a.o", "b.o"], &[]);
        nplace(&mut x, 4);
        nplace(&mut f, 4);
        let (pe, xs, _) = p.equiv(&x, &f);
        assert!(pe.is_empty() && xs == 0, "{arch:?}: {pe:#?}");
        let mut m = x.clone();
        nplace(&mut m, 2);
        let (pe, _, _) = p.equiv(&m, &f);
        assert!(
            has(&pe, "P-E6", ".myinfo, which no unit holds") || has(&pe, "P-E6", "in no unit"),
            "{arch:?}: {pe:#?}"
        );
    }
}

// A PIE pair (GNU ld `-pie --no-dynamic-linker --export-dynamic`): its
// dynamic tables. ACCEPTED as built; REJECTED (P-E5, P-E3) with a
// `.rela.dyn` addend, a `.dynamic` value, a `.dynstr` byte and a
// `.dynsym` entry changed in X.
#[test]
fn p_e5_dynamic_tables() {
    for arch in [Arch::Aarch64, Arch::X86_64] {
        let Some(p) = assemble(arch, "pie", "", "", &[]) else { return };
        let flags = ["-pie", "--no-dynamic-linker", "--export-dynamic", "-z", "norelro"];
        let mut x = p.update(&flags);
        // The linker's own sections, chunks in X's map (it claims coverage).
        let mut chunks = Vec::new();
        for (n, a, z, _) in &x.secs {
            let covered = x.map.placed.iter().any(|q| *a <= q.addr && q.addr < a + z)
                || x.map.chunks.iter().any(|c| c.addr.is_some_and(|ca| *a <= ca && ca < a + z));
            if *a != 0 && *z > 0 && !covered {
                chunks.push(Chunk {
                    kind: ChunkKind::Synthetic,
                    name: n.clone(),
                    addr: Some(*a),
                    size: *z,
                    target: None,
                    line: 0,
                });
            }
        }
        x.map.chunks.extend(chunks);
        let f = p.link(&["main.o", "a.o", "b.o"], &flags);
        let (pe, _, _) = p.equiv(&x, &f);
        assert!(pe.is_empty(), "{arch:?}: {pe:#?}");
        // The first .rela.dyn record's addend + 8.
        let (r, _, _) = x.sec(".rela.dyn");
        let mut m = x.clone();
        let o = m.off(r) + 16;
        let v = u64::from_le_bytes(m.output[o..o + 8].try_into().unwrap()) + 8;
        m.output[o..o + 8].copy_from_slice(&v.to_le_bytes());
        let (pe, _, _) = p.equiv(&m, &f);
        assert!(has(&pe, "P-E5", "the .rela.dyn record"), "{arch:?}: {pe:#?}");
        // DT_SYMENT (11) made 0x20.
        let (d, z, _) = x.sec(".dynamic");
        let mut m = x.clone();
        let o = m.off(d);
        let k = (0..z as usize / 16).find(|i| m.output[o + i * 16] == 11).expect("DT_SYMENT");
        m.output[o + k * 16 + 8] = 0x20;
        let (pe, _, _) = p.equiv(&m, &f);
        assert!(has(&pe, "P-E5", ".dynamic entry"), "{arch:?}: {pe:#?}");
        // A `.dynstr` byte (the last name's last character).
        let (s, z, _) = x.sec(".dynstr");
        let mut m = x.clone();
        let o = m.off(s) + z as usize - 2;
        m.output[o] ^= 1;
        let (pe, _, _) = p.equiv(&m, &f);
        assert!(has(&pe, "P-E5", ".dynstr differs"), "{arch:?}: {pe:#?}");
        // The last `.dynsym` entry's size + 1, its type (st_info), and its
        // name (st_name one byte on, the same string's tail).
        let (y, z, _) = x.sec(".dynsym");
        let last = x.off(y) + z as usize - 24;
        for (at, delta) in [(16, 1), (4, 1), (0, 1)] {
            let mut m = x.clone();
            let o = last + at;
            m.output[o] = m.output[o].wrapping_add(delta);
            let (pe, _, _) = p.equiv(&m, &f);
            assert!(has(&pe, "P-E3", ".dynsym entry"), "{arch:?} +{at}: {pe:#?}");
        }
    }
}

// `.eh_frame_hdr` (GNU ld's `--eh-frame-hdr`, an FDE in a.o): the pair
// is ACCEPTED; X's first table entry's location made another FDE's (+4)
// is REJECTED by P-E5. AArch64 only: on x86 GNU ld rewrites the records
// it writes a header for (a.o's 0x30 bytes become 0x2c), so its map
// cannot be split into mortar's per-record pieces.
#[test]
fn p_e5_eh_frame_hdr() {
    let arch = Arch::Aarch64;
    {
        let cfi = "\t.cfi_startproc\n\tnop\n\t.cfi_endproc\n";
        let Some(p) = assemble(arch, "ehh", "", cfi, &[]) else { return };
        let flags = ["--eh-frame-hdr", "-z", "norelro"];
        let x = p.eh_pieces(p.update(&flags));
        let f = p.eh_pieces(p.link(&["main.o", "a.o", "b.o"], &flags));
        let (pe, xs, fs) = p.equiv(&x, &f);
        assert!(pe.is_empty() && xs == 0 && fs == 0, "{arch:?}: {pe:#?}");
        let (h, z, _) = x.sec(".eh_frame_hdr");
        assert!(z >= 20, "{arch:?}: a table entry");
        let mut m = x.clone();
        let o = m.off(h) + 12;
        m.output[o] = m.output[o].wrapping_add(4);
        let (pe, _, _) = p.equiv(&m, &f);
        assert!(has(&pe, "P-E5", ".eh_frame_hdr"), "{arch:?}: {pe:#?}");
    }
}

// §9.4 for `--equiv`: truncations and byte flips of X's output, X's and
// F's map text cut short, the sides swapped, a foreign map (another
// program's) as F: judged without a panic; a truncated X is never clean.
#[test]
fn hostile_pairs_never_panic() {
    for arch in archs() {
        let Some(p) = assemble(arch, "hostile", "", "", &[]) else { return };
        let x = p.update(&[]);
        let f = p.link(&["main.o", "a.o", "b.o"], &[]);
        let inputs: Vec<Input<'_>> =
            p.files.iter().map(|(q, b)| Input { path: q, bytes: b }).collect();
        let xt = mortar_plumb::map::mortar::write(&x.map);
        let ft = mortar_plumb::map::mortar::write(&f.map);
        let xm = mortar_plumb::map::parse(Format::Mortar, &xt).unwrap();
        let fm = mortar_plumb::map::parse(Format::Mortar, &ft).unwrap();
        let n = x.output.len();
        for k in (0..n).step_by((n / 300).max(1)) {
            let e = mortar_plumb::equiv::check(&x.output[..k], &xm, &f.output, &fm, &inputs);
            assert!(!e.clean(), "{arch:?}: X cut at {k} is clean");
            let mut b = x.output.clone();
            b[k] ^= 0xff;
            let _ = mortar_plumb::equiv::check(&b, &xm, &f.output, &fm, &inputs);
            let _ = mortar_plumb::equiv::check(&x.output, &xm, &b, &fm, &inputs);
        }
        for text in [&xt, &ft] {
            for k in (0..text.len()).step_by((text.len() / 100).max(1)) {
                let Some(cut) = text.get(..k) else { continue };
                if let Ok(m) = mortar_plumb::map::parse(Format::Mortar, cut) {
                    let _ = mortar_plumb::equiv::check(&x.output, &m, &f.output, &fm, &inputs);
                    let _ = mortar_plumb::equiv::check(&x.output, &xm, &f.output, &m, &inputs);
                }
            }
        }
        // Swapped and foreign: F's output with X's map, X's with F's.
        let e = mortar_plumb::equiv::check(&f.output, &xm, &x.output, &fm, &inputs);
        assert!(!e.clean(), "{arch:?}: swapped outputs are clean");
        let other = p.link(&["main.o", "a.o", "b.o", "pad.o"], &[]);
        let om =
            mortar_plumb::map::parse(Format::Mortar, &mortar_plumb::map::mortar::write(&other.map))
                .unwrap();
        let e = mortar_plumb::equiv::check(&x.output, &xm, &other.output, &om, &inputs);
        assert!(!e.clean(), "{arch:?}: a foreign F is clean");
        let e = mortar_plumb::equiv::check(&x.output, &xm, &[], &fm, &inputs);
        assert!(!e.clean(), "{arch:?}: an empty F is clean");
    }
}

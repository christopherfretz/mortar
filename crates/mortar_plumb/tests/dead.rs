//! Holes and dead ranges (`spec/plumb.md` §6.8; `spec/incr.md` IN-3a,
//! IN-60): hand-built incremental outputs, each REJECTED by a named
//! finding or ACCEPTED.
//!
//! An output of an incremental link is modelled from a full link by GNU
//! ld: one input file (`dead.o`) plays the dead version of a file. Its
//! file and placements leave the map (converted to mortar's format) and
//! each range it occupied becomes a `hole` or `dead` line, its bytes
//! rewritten to what IN-3 and IN-51/IN-51b say an update leaves there.
//! That baseline must be judged with no P-I finding; each mutation of it
//! must be rejected by the finding it names, beyond the baseline's.

#![allow(clippy::many_single_char_names)] // test models: a, b, d, m

use std::fmt::Write as _;
use std::path::Path;

use mortar_plumb::map::{Chunk, ChunkKind, DeadKind, DeadRange, Map, Placed, SectionRef};
use mortar_plumb::{Format, Input};
use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, run_with};

// ---------------------------------------------------------------------
// The program.

/// The sources per architecture: `main.s` (live code with a reference
/// one past `arr`'s end, one 8 bytes before `blk_`, a label at `arr`'s
/// end, an `.init` piece and a `__start_`-reachable array), `arr.s`,
/// `dead.s` (code, data and an FDE: the dead version of a file) and
/// `blk.s`.
fn sources(arch: Arch) -> [(&'static str, String); 4] {
    let (call, word, fill, live_ret, ty) = match arch {
        Arch::Aarch64 => ("\tbl live\n\tb .\n", ".quad", ".fill 8,4,0", "\tret\n", "%function"),
        Arch::X86_64 => {
            ("\tcall live\n\thlt\n", ".quad", ".fill 32,1,0xcc", "\tret\n", "@function")
        }
        Arch::I386 => ("\tcall live\n\thlt\n", ".long", ".fill 32,1,0xcc", "\tret\n", "@function"),
    };
    let sec = if arch == Arch::Aarch64 { "%progbits" } else { "@progbits" };
    let main = format!(
        "\t.text\n\t.globl _start\n_start:\n\t.cfi_startproc\n{call}\t.cfi_endproc\n\
         \t.section .text.live,\"ax\",{sec}\n\t.globl live\n\t.type live,{ty}\nlive:\n\
         \t.cfi_startproc\n{live_ret}\t.cfi_endproc\n\t.size live, .-live\n\
         \t.section .init,\"ax\",{sec}\n\t.balign 4\n\tnop\n\
         \t.section my_array,\"aw\",{sec}\n\t{word} live\n\
         \t.section .tdata,\"awT\",{sec}\n\t.balign 8\n\t.fill 8,1,2\n\
         \t.section .rodata.str1.1,\"aMS\",{sec},1\n\t.asciz \"live\"\n\
         \t.data\n\t.globl ptrs\nptrs:\n\t{word} arr+40\n\t{word} blk_-8\n\t{word} arr_end\n"
    );
    let arr =
        "\t.data\n\t.balign 8\n\t.globl arr\narr:\t.fill 40,1,1\n\t.globl arr_end\narr_end:\n"
            .to_string();
    let dead = format!(
        "\t.text\n\t.cfi_startproc\n\t{fill}\n\t.cfi_endproc\n\t.data\n\t.fill 16,1,0\n\
                 \t.section .tdata,\"awT\",{sec}\n\t.balign 8\n\t.fill 8,1,3\n\
                 \t.section .rodata.str1.1,\"aMS\",{sec},1\n\t.asciz \"deadstr\"\n"
    );
    let blk = format!("\t.data\n\t.globl blk_\nblk_:\t{word} 7\n");
    [("main", main), ("arr", arr), ("dead", dead), ("blk", blk)]
}

fn run(dir: &Path, program: &Path, args: &[&str]) {
    let mut c = std::process::Command::new(program);
    c.current_dir(dir).args(args);
    let out = run_with(&mut c, b"", mortar_testutil::DEFAULT_TIMEOUT);
    assert!(
        out.success(),
        "{} {} failed in {}: {}",
        program.display(),
        args.join(" "),
        dir.display(),
        out.stderr_text()
    );
}

/// An output section: name, address, size, file offset, flags.
#[derive(Debug, Clone)]
struct Sec {
    name: String,
    addr: u64,
    size: u64,
    offset: u64,
}

fn sections(bytes: &[u8]) -> Vec<Sec> {
    let h = mortar_elf::FileHeader::parse(bytes).unwrap();
    let t = mortar_elf::SectionTable::parse(bytes, &h).unwrap();
    t.iter()
        .map(|s| Sec {
            name: String::from_utf8_lossy(s.name().unwrap_or(b"")).into_owned(),
            addr: s.header().sh_addr,
            size: s.header().sh_size,
            offset: s.header().sh_offset,
        })
        .collect()
}

fn section_sizes(bytes: &[u8]) -> Vec<(String, u64)> {
    sections(bytes).into_iter().map(|s| (s.name, s.size)).collect()
}

/// A hand-built incremental output and its map.
#[derive(Clone)]
struct Built {
    arch: Arch,
    output: Vec<u8>,
    map: Map,
    files: Vec<(String, Vec<u8>)>,
    secs: Vec<Sec>,
}

impl Built {
    fn sec(&self, name: &str) -> &Sec {
        self.secs.iter().find(|s| s.name == name).unwrap_or_else(|| panic!("no {name}"))
    }
    /// The file offset of address `a` of section `name`.
    fn at(&self, name: &str, a: u64) -> usize {
        let s = self.sec(name);
        (s.offset + (a - s.addr)) as usize
    }
    fn patch(&mut self, name: &str, a: u64, bytes: &[u8]) {
        let o = self.at(name, a);
        self.output[o..o + bytes.len()].copy_from_slice(bytes);
    }
    fn word(&self) -> usize {
        if self.arch == Arch::I386 { 4 } else { 8 }
    }
    fn value(&self, name: &str, a: u64) -> u64 {
        let o = self.at(name, a);
        self.output[o..o + self.word()].iter().rev().fold(0, |v, &b| v << 8 | u64::from(b))
    }
    /// The output section holding `[a, a + size)`.
    fn owner(&self, a: u64, size: u64) -> Option<&Sec> {
        self.secs
            .iter()
            .filter(|s| s.addr != 0 && s.addr <= a && a + size <= s.addr + s.size)
            .min_by_key(|s| s.size)
    }
    fn hole(&mut self, section: &str, a: u64, size: u64) {
        let s = self.sec(section).clone();
        self.map.dead.push(DeadRange {
            section: s.name,
            offset: a - s.addr,
            size,
            kind: None,
            line: 0,
        });
    }
    fn dead(&mut self, section: &str, offset: u64, size: u64, kind: DeadKind) {
        self.map.dead.push(DeadRange {
            section: section.into(),
            offset,
            size,
            kind: Some(kind),
            line: 0,
        });
    }
    /// Removes the placement of input section `name` of file `path`: its
    /// address and size.
    fn unplace(&mut self, path: &str, name: &str) -> (u64, u64) {
        let fi = self.map.files.iter().position(|f| f.path == path).unwrap() as u32;
        let i = self
            .map
            .placed
            .iter()
            .position(|p| p.file == fi && p.section == SectionRef::Name(name.into()))
            .unwrap_or_else(|| panic!("{path} {name} not placed"));
        let p = self.map.placed.remove(i);
        (p.addr, p.size)
    }
    fn placed(&self, path: &str, name: &str) -> (u64, u64) {
        let fi = self.map.files.iter().position(|f| f.path == path).unwrap() as u32;
        let p = self
            .map
            .placed
            .iter()
            .find(|p| p.file == fi && p.section == SectionRef::Name(name.into()))
            .unwrap();
        (p.addr, p.size)
    }
    /// The P-I findings, `"<id> <message>"`, through the map's text form.
    fn judge(&self) -> Vec<String> {
        let text = mortar_plumb::map::mortar::write(&self.map);
        let map = mortar_plumb::map::parse(Format::Mortar, &text).unwrap();
        let inputs: Vec<Input<'_>> =
            self.files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
        let r = mortar_plumb::verify(&self.output, &inputs, &map);
        r.findings
            .iter()
            .filter(|f| f.rule.id().starts_with("P-I"))
            .map(|f| format!("{} {}", f.rule.id(), f.message))
            .collect()
    }
}

/// Removes file `path` from the map with every record of it: the file
/// leaves the link, as the dead version of an input does. Its
/// placements are returned.
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
    map.folds.retain(|p| p.file != d && p.into_file != d);
    map.folds.iter_mut().for_each(|p| {
        re(&mut p.file);
        re(&mut p.into_file);
    });
    map.nonalloc.retain(|p| p.file != d);
    map.nonalloc.iter_mut().for_each(|p| re(&mut p.file));
    gone
}

/// IN-34a's padding CIE of `len` bytes.
fn padding_cie(arch: Arch, len: usize) -> Vec<u8> {
    let ra = match arch {
        Arch::Aarch64 => 30,
        Arch::X86_64 => 16,
        Arch::I386 => 8,
    };
    let mut v = ((len - 4) as u32).to_le_bytes().to_vec();
    v.extend_from_slice(&[0, 0, 0, 0, 1, 0, 1, 0x7c, ra]);
    v.resize(len, 0);
    v
}

/// The fill of code (IN-3).
fn code_fill(arch: Arch) -> u8 {
    if arch == Arch::Aarch64 { 0 } else { 0xcc }
}

fn tool(arch: Arch, name: &str) -> Option<std::path::PathBuf> {
    arch.tool(name)
}

/// Links the program for `arch` and models the incremental output: the
/// baseline.
fn build(arch: Arch) -> Option<(Scratch, Built)> {
    build_with(arch, false)
}

/// [`build`]; with `empty`, `arr.o` also has `.data.empty`, an input
/// section of size 0 with the global `empty_lbl` (8-aligned, after `arr`:
/// GNU ld places it at `arr`'s end, which is the start of the hole
/// `dead.o`'s `.data` leaves), and `main.o` a word referring to it.
fn build_with(arch: Arch, empty: bool) -> Option<(Scratch, Built)> {
    let asm = tool(arch, "as")?;
    let ld = tool(arch, "ld.bfd")?;
    let dir = Scratch::new(&format!("dead-{}{}", arch.triple(), if empty { "-empty" } else { "" }));
    let mut objs = Vec::new();
    let mut texts = sources(arch);
    if empty {
        let (word, sec) = match arch {
            Arch::Aarch64 => (".quad", "%progbits"),
            Arch::X86_64 => (".quad", "@progbits"),
            Arch::I386 => (".long", "@progbits"),
        };
        write!(texts[0].1, "\t.data\n\t{word} empty_lbl\n").unwrap();
        write!(
            texts[1].1,
            "\t.section .data.empty,\"aw\",{sec}\n\t.balign 8\n\t.globl empty_lbl\nempty_lbl:\n"
        )
        .unwrap();
    }
    for (name, text) in texts {
        std::fs::write(dir.join(format!("{name}.s")), text).unwrap();
        run(dir.path(), &asm, &[&format!("{name}.s"), "-o", &format!("{name}.o")]);
        objs.push(format!("{name}.o"));
    }
    // The dead version's symbols are not in an update's `.symtab`: `-x`
    // drops the local ones (AArch64's mapping symbols).
    let mut args = vec!["-x", "-e", "_start", "-o", "out", "-Map=out.map", "-z", "noexecstack"];
    args.extend(objs.iter().map(String::as_str));
    run(dir.path(), &ld, &args);
    let output = std::fs::read(dir.join("out")).unwrap();
    let text = std::fs::read_to_string(dir.join("out.map")).unwrap();
    let mut map = mortar_plumb::map::parse(Format::Gnu, &text).unwrap();
    let files: Vec<(String, Vec<u8>)> = map
        .paths()
        .into_iter()
        .filter(|p| *p != "dead.o")
        .filter_map(|p| std::fs::read(dir.join(p)).ok().map(|b| (p.to_string(), b)))
        .collect();
    map.format = Format::Mortar;
    map.incremental = true;
    let gone = drop_file(&mut map, "dead.o");
    let secs = sections(&output);
    let mut b = Built { arch, output, map, files, secs };
    // `.eh_frame` as mortar lists it: linker content (chunks) around the
    // hole the dead FDE leaves.
    let eh = b.sec(".eh_frame").clone();
    b.map.placed.retain(|p| p.section != SectionRef::Name(".eh_frame".into()));
    for p in gone.iter().filter(|p| p.size > 0) {
        // GNU ld merges the dead string into main.o's block: no range.
        let Some(owner) = b.owner(p.addr, p.size).map(|s| s.name.clone()) else { continue };
        if owner == ".eh_frame" {
            assert!(p.size >= 16, "the dead FDE is {} bytes", p.size);
            b.patch(".eh_frame", p.addr, &padding_cie(arch, p.size as usize));
            for (lo, hi) in [(eh.addr, p.addr), (p.addr + p.size, eh.addr + eh.size)] {
                if hi > lo {
                    b.map.chunks.push(Chunk {
                        kind: ChunkKind::Synthetic,
                        name: ".eh_frame".into(),
                        addr: Some(lo),
                        size: hi - lo,
                        target: None,
                        line: 0,
                    });
                }
            }
        } else {
            // What an update leaves in a hole: the fill (IN-3).
            let f = if owner == ".text" { code_fill(arch) } else { 0 };
            b.patch(&owner, p.addr, &vec![f; p.size as usize]);
        }
        b.hole(&owner, p.addr, p.size);
    }
    // GNU ld merged dead.o's "deadstr" after main.o's "live": the dead
    // fragment is a `strings` range (IN-3a: dead fragments are dead
    // ranges).
    let ro = b.sec(".rodata").size;
    b.dead(".rodata", ro - 8, 8, DeadKind::Strings);
    Some((dir, b))
}

fn archs() -> [Arch; 3] {
    [Arch::Aarch64, Arch::X86_64, Arch::I386]
}

/// The findings a mutation adds to the baseline's.
fn added(base: &[String], m: &Built) -> Vec<String> {
    m.judge().into_iter().filter(|f| !base.contains(f)).collect()
}

fn expect(arch: Arch, case: &str, base: &[String], m: &Built, id: &str, needle: &str) {
    let got = added(base, m);
    assert!(
        got.iter().any(|f| f.starts_with(&format!("{id} ")) && f.contains(needle)),
        "{arch:?} {case}: no {id} finding with '{needle}': {got:#?}"
    );
}

// ---------------------------------------------------------------------
// The baseline and the two positive N3 cases.

// IN-60 coverage, IN-3 fills and IN-34a's padding CIE: the modelled
// output is ACCEPTED. It holds both of the re-review's N3 references:
// `arr+40` (one past the end of the 40-byte `arr`, the first byte of the
// `.data` hole), a label `arr_end` there, and `blk_-8` (gfortran's bias,
// inside the hole before `blk_`): judged by what they name, not where
// they point.
#[test]
fn in60_baseline_and_n3_references_are_accepted() {
    for arch in archs() {
        let Some((_dir, b)) = build(arch) else { return };
        assert!(b.judge().is_empty(), "{arch:?}: {:#?}", b.judge());
        let hole = b.map.dead.iter().find(|d| d.section == ".data").unwrap().clone();
        let data = b.sec(".data").addr;
        let (lo, hi) = (data + hole.offset, data + hole.offset + hole.size);
        let (ptrs, _) = b.placed("main.o", ".data");
        let w = b.word() as u64;
        assert_eq!(b.value(".data", ptrs), lo, "{arch:?}: arr+40 is the hole's first byte");
        let bias = b.value(".data", ptrs + w);
        assert!(lo <= bias && bias < hi, "{arch:?}: blk_-8 ({bias:#x}) lies in the hole");
        assert_eq!(
            b.value(".data", ptrs + 2 * w),
            lo,
            "{arch:?}: arr_end is the hole's first byte"
        );
        // Without `incremental` and its lines the map claims nothing: the
        // checks do not run, and a plain map pays nothing.
        let mut plain = b.clone();
        plain.map.dead.clear();
        plain.map.incremental = false;
        assert!(plain.judge().is_empty(), "{arch:?}");
    }
}

// The N3 references alone, each REJECTED once it names the range: a
// relocation whose symbol's section is placed inside a hole.
#[test]
fn in3a_live_relocation_naming_a_hole_is_rejected() {
    for arch in archs() {
        let Some((_dir, b)) = build(arch) else { return };
        let base = b.judge();
        let mut m = b.clone();
        let (a, size) = m.placed("main.o", ".text.live");
        let fill = vec![code_fill(arch); size as usize];
        m.patch(".text", a, &fill);
        m.hole(".text", a, size);
        expect(arch, "live reloc into hole", &base, &m, "P-I3", "a relocation");
        expect(arch, "live reloc into hole", &base, &m, "P-I1", "covered twice");
        // The same section left out of the map: its symbol is in the hole.
        let mut m = b.clone();
        let (a, size) = m.unplace("main.o", ".text.live");
        m.patch(".text", a, &fill);
        m.hole(".text", a, size);
        expect(arch, "symbol in hole", &base, &m, "P-I3", ".symtab symbol 'live'");
        // e_entry aimed into the code hole.
        let mut m = b.clone();
        let hole = m.map.dead.iter().find(|d| d.section == ".text").unwrap().clone();
        let entry = m.sec(".text").addr + hole.offset + 4;
        let w = m.word();
        // e_entry is at offset 24 in both classes.
        m.output[24..24 + w].copy_from_slice(&entry.to_le_bytes()[..w]);
        expect(arch, "e_entry in hole", &base, &m, "P-I3", "e_entry");
    }
}

// P-I3 and an input section of size 0 (WP-062's finding 9, libgfortran's
// `.bss.empty_string.N` at their region's end): it covers no byte, so it
// is in a range only strictly inside it. At the hole's start (GNU ld's
// place: `arr`'s end) and at its end it names the boundary (N3), and the
// relocation naming it is ACCEPTED; placed strictly inside the hole it is
// REJECTED (a section with bytes there is `in3a_live_relocation_…`'s).
#[test]
fn in3a_size_zero_section_at_a_range_boundary_is_accepted() {
    for arch in archs() {
        let Some((_dir, b)) = build_with(arch, true) else { return };
        let base = b.judge();
        assert!(base.is_empty(), "{arch:?}: {base:#?}");
        let hole = b.map.dead.iter().find(|d| d.section == ".data").unwrap().clone();
        let lo = b.sec(".data").addr + hole.offset;
        let (at, size) = b.placed("arr.o", ".data.empty");
        assert_eq!((at, size), (lo, 0), "{arch:?}: .data.empty is at the hole's start");
        let fi = b.map.files.iter().position(|f| f.path == "arr.o").unwrap() as u32;
        let moved = |to: u64| {
            let mut m = b.clone();
            let p = m
                .map
                .placed
                .iter_mut()
                .find(|p| p.file == fi && p.section == SectionRef::Name(".data.empty".into()))
                .unwrap();
            p.addr = to;
            m
        };
        let end = moved(lo + hole.size);
        assert!(added(&base, &end).is_empty(), "{arch:?}: at the end: {:#?}", added(&base, &end));
        let inside = moved(lo + 8);
        expect(arch, "size 0 inside", &base, &inside, "P-I3", "names 'empty_lbl'");
    }
}

// ---------------------------------------------------------------------
// Fills (IN-3a check 3) and coverage (IN-60).

#[test]
fn in3_stale_bytes_in_holes_are_rejected() {
    for arch in archs() {
        let Some((_dir, b)) = build(arch) else { return };
        let base = b.judge();
        for (sec, byte) in [(".text", 0x90u8), (".data", 0x5a)] {
            let mut m = b.clone();
            let hole = m.map.dead.iter().find(|d| d.section == sec).unwrap().clone();
            let a = m.sec(sec).addr + hole.offset + hole.size - 1;
            m.patch(sec, a, &[byte]);
            expect(arch, "stale byte", &base, &m, "P-I5", "is not the fill");
        }
    }
}

#[test]
fn in60_unlisted_hole_and_bad_lines_are_rejected() {
    for arch in archs() {
        let Some((_dir, b)) = build(arch) else { return };
        let base = b.judge();
        // An unlisted hole: covered by nothing.
        let mut m = b.clone();
        m.map.dead.retain(|d| d.section != ".data");
        expect(arch, "unlisted hole", &base, &m, "P-I1", "covered by no line");
        // Overlapping lines.
        let mut m = b.clone();
        let d = m.map.dead.iter().find(|d| d.section == ".data").unwrap().clone();
        m.map.dead.push(DeadRange { offset: d.offset + 4, size: 4, ..d.clone() });
        expect(arch, "overlap", &base, &m, "P-I2", "overlaps");
        // Outside its section.
        let mut m = b.clone();
        let size = m.sec(".data").size;
        m.map.dead.push(DeadRange { offset: size, size: 8, ..d.clone() });
        expect(arch, "outside", &base, &m, "P-I2", "outside");
        // A kind the section does not have.
        let mut m = b.clone();
        let t = m.map.dead.iter().position(|x| x.section == ".text").unwrap();
        m.map.dead[t].kind = Some(DeadKind::Cie);
        expect(arch, "kind", &base, &m, "P-I2", "'cie' range");
        // A section the map names twice or not at all.
        let mut m = b.clone();
        m.map.dead.push(DeadRange { section: ".nowhere".into(), ..d.clone() });
        expect(arch, "no section", &base, &m, "P-I2", "names 0 output sections");
        // `strings` and `neutral` in a data section, a hole in a
        // non-allocated one.
        for (kind, needle) in
            [(DeadKind::Strings, "'strings' range"), (DeadKind::Neutral, "'neutral' range")]
        {
            let mut m = b.clone();
            let i = m.map.dead.iter().position(|x| x.section == ".data").unwrap();
            m.map.dead[i].kind = Some(kind);
            expect(arch, "kind in .data", &base, &m, "P-I2", needle);
        }
        let mut m = b.clone();
        m.map.dead.push(DeadRange { section: ".shstrtab".into(), offset: 0, size: 1, ..d });
        expect(arch, "non-alloc hole", &base, &m, "P-I2", "allocated section only");
        // A `.tdata` hole not at the section's end (L-143): main.o's word,
        // with dead.o's after it.
        let mut m = b.clone();
        let (a, size) = m.unplace("main.o", ".tdata");
        m.patch(".tdata", a, &vec![0; size as usize]);
        m.hole(".tdata", a, size);
        expect(arch, "tls hole", &base, &m, "P-I2", "thread-local");
        // The dead string a `strings` range that does not end with a NUL.
        let mut m = b.clone();
        let s = m.sec(".rodata").clone();
        m.patch(".rodata", s.addr + s.size - 1, b"x");
        expect(arch, "strings NUL", &base, &m, "P-I6", "NUL");
    }
}

// layout.md L-142, incr.md IN-30: packed sections have no hole.
#[test]
fn in30_holes_in_packed_sections_are_rejected() {
    for arch in archs() {
        let Some((_dir, b)) = build(arch) else { return };
        let base = b.judge();
        // A hole in `.init`, where the dead piece was.
        let mut m = b.clone();
        let (a, size) = m.unplace("main.o", ".init");
        m.patch(".init", a, &vec![code_fill(arch); size as usize]);
        m.hole(".init", a, size);
        expect(arch, "hole in .init", &base, &m, "P-I2", "packed section");
        // A zero slot in the `__start_my_array` array.
        let mut m = b.clone();
        let (a, size) = m.unplace("main.o", "my_array");
        m.patch("my_array", a, &vec![0; size as usize]);
        m.hole("my_array", a, size);
        expect(arch, "zero slot", &base, &m, "P-I2", "packed section");
    }
}

// incr.md IN-34a: an `.eh_frame` hole is one padding CIE of at least 16
// bytes.
#[test]
fn in34a_eh_frame_holes_are_padding_cies() {
    for arch in archs() {
        let Some((_dir, b)) = build(arch) else { return };
        let base = b.judge();
        let hole = b.map.dead.iter().find(|d| d.section == ".eh_frame").unwrap().clone();
        let a = b.sec(".eh_frame").addr + hole.offset;
        // A 12-byte CIE and then a zero word: a terminator for the
        // walkers, in the middle of the section.
        let mut m = b.clone();
        let mut bytes = padding_cie(arch, 12);
        bytes.resize(hole.size as usize, 0);
        m.patch(".eh_frame", a, &bytes);
        expect(arch, "12-byte CIE", &base, &m, "P-I5", "padding CIE");
        // A hole of 12 bytes (the rest linker content).
        let mut m = b.clone();
        let i = m.map.dead.iter().position(|d| d.section == ".eh_frame").unwrap();
        m.map.dead[i].size = 12;
        m.patch(".eh_frame", a, &padding_cie(arch, 12));
        m.map.chunks.push(Chunk {
            kind: ChunkKind::Synthetic,
            name: ".eh_frame".into(),
            addr: Some(a + 12),
            size: hole.size - 12,
            target: None,
            line: 0,
        });
        expect(arch, "12-byte hole", &base, &m, "P-I5", "at least 16 bytes");
        // A `cie` range of something else.
        let mut m = b.clone();
        let i = m.map.dead.iter().position(|d| d.section == ".eh_frame").unwrap();
        m.map.dead[i].kind = Some(DeadKind::Cie);
        let mut not_cie = ((hole.size - 4) as u32).to_le_bytes().to_vec();
        not_cie.extend_from_slice(&0x10u32.to_le_bytes());
        m.patch(".eh_frame", a, &not_cie);
        expect(arch, "cie kind", &base, &m, "P-I6", "not a CIE");
        // A live FDE whose CIE pointer names the padding CIE of the hole.
        let mut m = b.clone();
        let eh = m.sec(".eh_frame").clone();
        let o = eh.offset as usize;
        let cie_len = u64::from(u32::from_le_bytes(m.output[o..o + 4].try_into().unwrap()));
        let field = eh.addr + 4 + cie_len + 4;
        let ptr = (field.wrapping_sub(a)) as u32;
        m.patch(".eh_frame", field, &ptr.to_le_bytes());
        expect(arch, "eh CIE pointer", &base, &m, "P-I3", "CIE");
    }
}

// ---------------------------------------------------------------------
// The debug sections (IN-3a checks 1 and 2, IN-51, IN-51b, IN-53).

/// ULEB128 of `v` in exactly `w` bytes.
fn uleb_fixed(v: u64, w: usize) -> Option<Vec<u8>> {
    if w < 10 && v >> (7 * w) != 0 {
        return None;
    }
    Some((0..w).map(|i| ((v >> (7 * i)) & 0x7f) as u8 | if i + 1 < w { 0x80 } else { 0 }).collect())
}

/// IN-51's block unit of `len` bytes (its `unit_length` + 4), naming
/// the abbreviation table at `abbrev`.
fn neutral_unit(version: u16, len: usize, abbrev: u32) -> Vec<u8> {
    let mut v = ((len - 4) as u32).to_le_bytes().to_vec();
    v.extend_from_slice(&version.to_le_bytes());
    if version >= 5 {
        v.extend_from_slice(&[1, 8]);
        v.extend_from_slice(&abbrev.to_le_bytes());
    } else {
        v.extend_from_slice(&abbrev.to_le_bytes());
        v.push(8);
    }
    v.push(1);
    let rest = len - v.len();
    let (w, n) = (1..=5).find_map(|w| uleb_fixed((rest - w) as u64, w).map(|n| (w, n))).unwrap();
    v.extend_from_slice(&n);
    v.resize(v.len() + rest - w, 0);
    assert_eq!(v.len(), len);
    v
}

/// IN-51b's neutral line table of `len` bytes.
fn neutral_line(version: u16, len: usize) -> Vec<u8> {
    let mut v = ((len - 4) as u32).to_le_bytes().to_vec();
    v.extend_from_slice(&version.to_le_bytes());
    let header: &[u8] = if version >= 5 {
        &[1, 1, 1, 0xfb, 14, 7, 0, 1, 1, 1, 1, 0, 1, 1, 8, 0, 1, 1, 8, 0]
    } else {
        &[1, 1, 1, 0xfb, 14, 7, 0, 1, 1, 1, 1, 0, 0, 0]
    };
    if version >= 5 {
        v.extend_from_slice(&[8, 0]);
    }
    v.extend_from_slice(&(header.len() as u32).to_le_bytes());
    v.extend_from_slice(header);
    assert!(v.len() <= len, "a neutral line table of {len} bytes");
    v.resize(len, 6);
    v
}

/// A debug program: `main.c` live, `dead.c` the dead file, linked by GNU
/// ld with `dead.o` last so that its contributions end each debug
/// section. The baseline neutralizes them: `.debug_info` units in block
/// form, `.debug_abbrev` IN-51's table and zeros, `.debug_line` the
/// neutral table, `.debug_frame` left as written with its address
/// tombstoned (IN-53), and the dead code a hole.
struct Debug {
    b: Built,
    /// `(section, offset, size)` of each dead contribution.
    dead: Vec<(String, u64, u64)>,
    /// The output before neutralization.
    raw: Vec<u8>,
    /// main.o's `.debug_line` size.
    main_line: u64,
}

fn build_debug(arch: Arch, dwarf: u16) -> Option<(Scratch, Debug)> {
    let gcc = tool(arch, "gcc")?;
    let ld = tool(arch, "ld.bfd")?;
    let objcopy = tool(arch, "objcopy")?;
    let dir = Scratch::new(&format!("dead-dwarf-{}-{dwarf}", arch.triple()));
    std::fs::write(dir.join("main.c"), "int g = 5;\nint main(void) { return g; }\n").unwrap();
    std::fs::write(
        dir.join("dead.c"),
        "__attribute__((used)) static int deadf(int x) { return x * 3 + 1; }\n",
    )
    .unwrap();
    let dv = format!("-gdwarf-{dwarf}");
    let mut flags = vec![
        "-c",
        "-O0",
        dv.as_str(),
        "-fno-pic",
        "-fno-pie",
        "-fno-asynchronous-unwind-tables",
        "-fno-unwind-tables",
    ];
    if arch != Arch::Aarch64 {
        flags.push("-fcf-protection=none");
    }
    for f in ["main", "dead"] {
        let mut a = flags.clone();
        let (src, obj) = (format!("{f}.c"), format!("{f}.o"));
        a.extend([src.as_str(), "-o", obj.as_str()]);
        run(dir.path(), &gcc, &a);
    }
    // The dead file's arange set would name its unit (IN-54: rewritten
    // whole by an update); the model leaves it out.
    run(dir.path(), &objcopy, &["--remove-section=.debug_aranges", "dead.o"]);
    run(
        dir.path(),
        &ld,
        &["-x", "-e", "main", "-o", "out", "-Map=out.map", "-z", "noexecstack", "main.o", "dead.o"],
    );
    let output = std::fs::read(dir.join("out")).unwrap();
    let text = std::fs::read_to_string(dir.join("out.map")).unwrap();
    let mut map = mortar_plumb::map::parse(Format::Gnu, &text).unwrap();
    let dead_o = std::fs::read(dir.join("dead.o")).unwrap();
    let main_o = std::fs::read(dir.join("main.o")).unwrap();
    let files: Vec<(String, Vec<u8>)> = vec![("main.o".into(), main_o.clone())];
    map.format = Format::Mortar;
    map.incremental = true;
    let gone = drop_file(&mut map, "dead.o");
    let secs = sections(&output);
    let raw = output.clone();
    let mut b = Built { arch, output, map, files, secs };
    for p in gone.iter().filter(|p| p.size > 0 && p.addr != 0) {
        let Some(owner) = b.owner(p.addr, p.size).map(|s| s.name.clone()) else { continue };
        b.patch(&owner, p.addr, &vec![code_fill(arch); p.size as usize]);
        b.hole(&owner, p.addr, p.size);
    }
    let in_sizes = section_sizes(&dead_o);
    let out_sizes = section_sizes(&b.output);
    let size_of = |v: &[(String, u64)], n: &str| v.iter().find(|s| s.0 == n).map(|s| s.1);
    let mut dead = Vec::new();
    for name in [".debug_info", ".debug_abbrev", ".debug_line", ".debug_frame"] {
        let (Some(i), Some(o)) = (size_of(&in_sizes, name), size_of(&out_sizes, name)) else {
            continue;
        };
        dead.push((name.to_string(), o - i, i));
    }
    let main_line = size_of(&section_sizes(&main_o), ".debug_line").unwrap();
    let mut d = Debug { b, dead, raw, main_line };
    let get = |d: &Debug, n: &str| d.dead.iter().find(|x| x.0 == n).cloned().unwrap();
    let (_, ab, ab_size) = get(&d, ".debug_abbrev");
    let (_, info, info_size) = get(&d, ".debug_info");
    let (_, line, line_size) = get(&d, ".debug_line");
    let mut abbrev = vec![1u8, 0x11, 0, 0x80, 0x40, 9, 0, 0];
    abbrev.resize(ab_size as usize, 0);
    d.patch_nonalloc(".debug_abbrev", ab, &abbrev);
    d.patch_nonalloc(".debug_info", info, &neutral_unit(dwarf, info_size as usize, ab as u32));
    d.patch_nonalloc(".debug_line", line, &neutral_line(dwarf, line_size as usize));
    d.b.dead(".debug_abbrev", ab, ab_size, DeadKind::Neutral);
    d.b.dead(".debug_info", info, info_size, DeadKind::Neutral);
    d.b.dead(".debug_line", line, line_size, DeadKind::Neutral);
    if let Some((_, fr, fr_size)) = d.dead.iter().find(|x| x.0 == ".debug_frame").cloned() {
        d.tombstone_frame(fr, fr_size);
        d.b.dead(".debug_frame", fr, fr_size, DeadKind::Contribution);
    }
    Some((dir, d))
}

impl Debug {
    fn file_offset(&self, sec: &str, off: u64) -> usize {
        (self.b.sec(sec).offset + off) as usize
    }
    fn patch_nonalloc(&mut self, sec: &str, off: u64, bytes: &[u8]) {
        let o = self.file_offset(sec, off);
        self.b.output[o..o + bytes.len()].copy_from_slice(bytes);
    }
    /// The FDEs of the dead `.debug_frame` contribution get I-47's
    /// tombstone (all ones) as their initial location.
    fn tombstone_frame(&mut self, off: u64, size: u64) {
        let w = self.b.word();
        let base = self.file_offset(".debug_frame", off);
        let mut p = 0usize;
        while p + 8 <= size as usize {
            let o = base + p;
            let len = u32::from_le_bytes(self.b.output[o..o + 4].try_into().unwrap()) as usize;
            let id = u32::from_le_bytes(self.b.output[o + 4..o + 8].try_into().unwrap());
            if id != 0xffff_ffff {
                self.b.output[o + 8..o + 8 + w].fill(0xff);
            }
            p += 4 + len;
        }
    }
}

fn debug_archs() -> [Arch; 3] {
    [Arch::Aarch64, Arch::X86_64, Arch::I386]
}

// IN-51, IN-51b, IN-53: neutral units, the neutral line table, IN-51's
// abbreviation table and a tombstoned `.debug_frame` contribution are
// ACCEPTED; DWARF 4 and 5.
#[test]
fn in51_neutral_debug_ranges_are_accepted() {
    for arch in debug_archs() {
        for dwarf in [4, 5] {
            let Some((_dir, d)) = build_debug(arch, dwarf) else { return };
            assert!(
                d.dead.iter().any(|x| x.0 == ".debug_frame"),
                "{arch:?}: no .debug_frame to model"
            );
            assert!(d.b.judge().is_empty(), "{arch:?} DWARF {dwarf}: {:#?}", d.b.judge());
        }
    }
}

#[test]
fn in3a_debug_mutations_are_rejected() {
    for arch in debug_archs() {
        for dwarf in [4, 5] {
            let Some((_dir, d)) = build_debug(arch, dwarf) else { return };
            let base = d.b.judge();
            let case = |c: &str| format!("{c} (DWARF {dwarf})");
            // A live unit's DW_AT_stmt_list aimed at a dead .debug_line
            // range: main.o's own table declared dead and neutralized.
            let mut m = Debug {
                b: d.b.clone(),
                dead: d.dead.clone(),
                raw: d.raw.clone(),
                main_line: d.main_line,
            };
            let n = m.main_line;
            m.patch_nonalloc(".debug_line", 0, &neutral_line(dwarf, n as usize));
            m.b.dead(".debug_line", 0, n, DeadKind::Neutral);
            expect(arch, &case("stmt_list"), &base, &m.b, "P-I4", "attribute 0x10");
            // The dead unit not neutralized.
            let mut m = d.b.clone();
            let (_, info, size) = d.dead.iter().find(|x| x.0 == ".debug_info").cloned().unwrap();
            let o = d.file_offset(".debug_info", info);
            m.output[o..o + size as usize].copy_from_slice(&d.raw[o..o + size as usize]);
            expect(arch, &case("unit not neutral"), &base, &m, "P-I6", "");
            // A stray byte in the dead abbreviation zeros.
            let mut m = d.b.clone();
            let (_, ab, size) = d.dead.iter().find(|x| x.0 == ".debug_abbrev").cloned().unwrap();
            let o = d.file_offset(".debug_abbrev", ab + size - 1);
            m.output[o] = 0x2e;
            expect(arch, &case("abbrev"), &base, &m, "P-I6", "neutral .debug_abbrev");
            // A neutral line table with a program byte other than 0x06.
            let mut m = d.b.clone();
            let (_, line, size) = d.dead.iter().find(|x| x.0 == ".debug_line").cloned().unwrap();
            let o = d.file_offset(".debug_line", line + size - 1);
            m.output[o] = 0x01;
            expect(arch, &case("line"), &base, &m, "P-I6", "DW_LNS_negate_stmt");
            // The dead `.debug_frame` FDE with its address left.
            if let Some((_, fr, size)) = d.dead.iter().find(|x| x.0 == ".debug_frame").cloned() {
                let mut m = d.b.clone();
                let o = d.file_offset(".debug_frame", fr);
                m.output[o..o + size as usize].copy_from_slice(&d.raw[o..o + size as usize]);
                expect(arch, &case("frame"), &base, &m, "P-I6", "tombstone");
            }
            // `.debug_info` takes no `contribution` (IN-53's list).
            let mut m = d.b.clone();
            let i = m.map.dead.iter().position(|x| x.section == ".debug_info").unwrap();
            m.map.dead[i].kind = Some(DeadKind::Contribution);
            expect(arch, &case("contribution kind"), &base, &m, "P-I2", "'contribution' range");
            // main.o's FDE aimed at the dead contribution's CIE.
            if let Some((_, fr, _)) = d.dead.iter().find(|x| x.0 == ".debug_frame").cloned() {
                let mut m = d.b.clone();
                let o = d.file_offset(".debug_frame", 0);
                let cie_len = u32::from_le_bytes(m.output[o..o + 4].try_into().unwrap()) as usize;
                let f = o + 4 + cie_len + 4;
                m.output[f..f + 4].copy_from_slice(&(fr as u32).to_le_bytes());
                expect(arch, &case("frame CIE pointer"), &base, &m, "P-I4", "CIE");
            }
            // ELFCLASS32 position-independent: a dead `.debug_line` left as
            // written (IN-53: neutral there).
            if arch == Arch::I386 {
                let mut m = d.b.clone();
                let (_, line, size) =
                    d.dead.iter().find(|x| x.0 == ".debug_line").cloned().unwrap();
                let o = d.file_offset(".debug_line", line);
                m.output[o..o + size as usize].copy_from_slice(&d.raw[o..o + size as usize]);
                let i = m.map.dead.iter().position(|x| x.section == ".debug_line").unwrap();
                m.map.dead[i].kind = Some(DeadKind::Contribution);
                m.output[16..18].copy_from_slice(&3u16.to_le_bytes());
                expect(arch, &case("pie"), &base, &m, "P-I6", "position-independent");
            }
            // `.debug_aranges` has no dead range (IN-54).
            let mut m = d.b.clone();
            m.dead(".debug_aranges", 0, 4, DeadKind::Neutral);
            expect(arch, &case("aranges"), &base, &m, "P-I2", "IN-54");
            // A `strings` range of `.debug_str` not ending with a NUL.
            let mut m = d.b.clone();
            let size = m.sec(".debug_str").size;
            let o = d.file_offset(".debug_str", size - 2);
            m.output[o] = b'x';
            m.dead(".debug_str", size - 2, 1, DeadKind::Strings);
            expect(arch, &case("strings"), &base, &m, "P-I6", "NUL");
        }
    }
}

// The hostile map: lines no output has, of every shape, judged without a
// panic.
#[test]
fn hostile_dead_lines_never_panic() {
    let Some((_dir, b)) = build(Arch::X86_64) else { return };
    let names = [".text", ".data", ".eh_frame", ".init", ".symtab", ".shstrtab", "", "my_array"];
    let kinds = [
        None,
        Some(DeadKind::Neutral),
        Some(DeadKind::Contribution),
        Some(DeadKind::Strings),
        Some(DeadKind::Cie),
    ];
    let nums = [0u64, 1, 3, 4, 12, 16, 0x7fff_ffff, u64::MAX / 2, u64::MAX];
    let mut n = 0;
    for name in names {
        for kind in kinds {
            for &off in &nums {
                for &size in &nums {
                    let mut m = b.clone();
                    m.map.dead.push(DeadRange {
                        section: name.into(),
                        offset: off,
                        size,
                        kind,
                        line: 0,
                    });
                    let _ = m.judge();
                    n += 1;
                }
            }
        }
    }
    assert!(n > 1000);
}

// ---------------------------------------------------------------------
// A position-independent output (WP-092: WP-091's first missing model).
// The dynamic tables IN-3a check 1 reads (the GOT's words, `.rela.dyn`'s
// records and their targets, `.eh_frame_hdr`) and IN-3's fill of an
// allocated relocation section exist only there.

/// The PIE program: `main.s` loads `live` through the GOT and holds the
/// three N3 words (each a `RELATIVE` record in a PIE), `dead.s` the dead
/// version of a file with code and two words that point at it (two
/// `RELATIVE` records that become the `.rela.dyn` hole).
fn pie_sources(arch: Arch) -> [(&'static str, String); 4] {
    let (got, ty, sec) = match arch {
        Arch::Aarch64 => (
            "\tadrp x0, :got:live\n\tldr x0, [x0, :got_lo12:live]\n\tbl live\n\tb .\n",
            "%function",
            "%progbits",
        ),
        _ => ("\taddq live@GOTPCREL(%rip), %rax\n\tcall live\n\thlt\n", "@function", "@progbits"),
    };
    let ret = "\tret\n";
    let main = format!(
        "\t.text\n\t.globl _start\n_start:\n\t.cfi_startproc\n{got}\t.cfi_endproc\n\
         \t.section .text.live,\"ax\",{sec}\n\t.globl live\n\t.type live,{ty}\nlive:\n\
         \t.cfi_startproc\n{ret}\t.cfi_endproc\n\t.size live, .-live\n\
         \t.data\n\t.globl ptrs\nptrs:\n\t.quad arr+40\n\t.quad blk_-8\n\t.quad arr_end\n"
    );
    let arr =
        "\t.data\n\t.balign 8\n\t.globl arr\narr:\t.fill 40,1,1\n\t.globl arr_end\narr_end:\n"
            .to_string();
    let fill = if arch == Arch::Aarch64 { ".fill 8,4,0" } else { ".fill 32,1,0xcc" };
    let dead = format!("\t.text\ndead_code:\n\t{fill}\n\t.data\n\t.quad dead_code\n\t.quad dead_code+4\n");
    let blk = "\t.data\n\t.globl blk_\nblk_:\t.quad 7\n".to_string();
    [("main", main), ("arr", arr), ("dead", dead), ("blk", blk)]
}

/// The `.rela.dyn` records: `(address, r_offset, type, symbol, addend)`.
fn rela_dyn(b: &Built) -> Vec<(u64, u64, u32, u32, u64)> {
    let s = b.sec(".rela.dyn").clone();
    let rd = |o: usize| u64::from_le_bytes(b.output[o..o + 8].try_into().unwrap());
    (0..s.size / 24)
        .map(|k| {
            let o = (s.offset + 24 * k) as usize;
            let info = rd(o + 8);
            (s.addr + 24 * k, rd(o), info as u32, (info >> 32) as u32, rd(o + 16))
        })
        .collect()
}

/// `R_*_RELATIVE` of the 64-bit machines.
fn relative(arch: Arch) -> u64 {
    if arch == Arch::Aarch64 { 1027 } else { 8 }
}

/// Links the PIE program with GNU ld (`-pie --eh-frame-hdr`) and models
/// the update that removed `dead.o`: its code and data are holes, its two
/// `RELATIVE` records (inside `DT_RELACOUNT`'s prefix) a `.rela.dyn` hole
/// of `RELATIVE` records aimed at one scratch word (IN-3, dyn.md §14).
fn build_pie(arch: Arch) -> Option<(Scratch, Built)> {
    let asm = tool(arch, "as")?;
    let ld = tool(arch, "ld.bfd")?;
    let dir = Scratch::new(&format!("dead-pie-{}", arch.triple()));
    let mut objs = Vec::new();
    for (name, text) in pie_sources(arch) {
        std::fs::write(dir.join(format!("{name}.s")), text).unwrap();
        let (s, o) = (format!("{name}.s"), format!("{name}.o"));
        let mut a = vec![s.as_str(), "-o", o.as_str()];
        if arch != Arch::Aarch64 {
            // GOTPCREL in an `add`, which GNU ld keeps: the GOT word stays.
            a.push("-mrelax-relocations=no");
        }
        run(dir.path(), &asm, &a);
        objs.push(o);
    }
    let mut args = vec![
        "-pie",
        "--no-dynamic-linker",
        "--eh-frame-hdr",
        "-z",
        "combreloc",
        "-x",
        "-e",
        "_start",
        "-o",
        "out",
        "-Map=out.map",
        "-z",
        "noexecstack",
    ];
    args.extend(objs.iter().map(String::as_str));
    run(dir.path(), &ld, &args);
    let output = std::fs::read(dir.join("out")).unwrap();
    let text = std::fs::read_to_string(dir.join("out.map")).unwrap();
    let mut map = mortar_plumb::map::parse(Format::Gnu, &text).unwrap();
    let files: Vec<(String, Vec<u8>)> = map
        .paths()
        .into_iter()
        .filter(|p| *p != "dead.o")
        .filter_map(|p| std::fs::read(dir.join(p)).ok().map(|b| (p.to_string(), b)))
        .collect();
    map.format = Format::Mortar;
    map.incremental = true;
    let gone = drop_file(&mut map, "dead.o");
    let secs = sections(&output);
    let mut b = Built { arch, output, map, files, secs };
    let mut data_hole = None;
    for p in gone.iter().filter(|p| p.size > 0) {
        let Some(owner) = b.owner(p.addr, p.size).map(|s| s.name.clone()) else { continue };
        let f = if owner == ".text" { code_fill(arch) } else { 0 };
        b.patch(&owner, p.addr, &vec![f; p.size as usize]);
        b.hole(&owner, p.addr, p.size);
        if owner == ".data" {
            data_hole = Some((p.addr, p.size));
        }
    }
    let (lo, size) = data_hole.expect("dead.o's .data");
    // The dead words' records: RELATIVE at the scratch word (the first
    // word of the data hole), addend 0, and a hole line over them.
    let dead: Vec<_> =
        rela_dyn(&b).into_iter().filter(|r| r.1 >= lo && r.1 < lo + size).collect();
    assert_eq!(dead.len(), 2, "{arch:?}: dead.o's two RELATIVE records: {dead:x?}");
    for r in &dead {
        let mut rec = lo.to_le_bytes().to_vec();
        rec.extend_from_slice(&relative(arch).to_le_bytes());
        rec.extend_from_slice(&0u64.to_le_bytes());
        b.patch(".rela.dyn", r.0, &rec);
    }
    assert_eq!(dead[1].0, dead[0].0 + 24, "{arch:?}: the dead records are adjacent");
    b.hole(".rela.dyn", dead[0].0, 48);
    // The tables GNU ld's map does not list, as mortar lists them: linker
    // content (chunks) around the holes.
    b.map.placed.retain(|p| p.section != SectionRef::Name(".eh_frame".into()));
    for name in [
        ".gnu.hash",
        ".dynsym",
        ".dynstr",
        ".rela.dyn",
        ".eh_frame_hdr",
        ".eh_frame",
        ".dynamic",
        ".got",
        ".got.plt",
    ] {
        let s = b.sec(name).clone();
        let mut at = s.addr;
        let mut holes: Vec<(u64, u64)> = b
            .map
            .dead
            .iter()
            .filter(|d| d.section == name)
            .map(|d| (s.addr + d.offset, d.size))
            .collect();
        holes.push((s.addr + s.size, 0));
        holes.sort_unstable();
        for (h, size) in holes {
            if h > at {
                b.map.chunks.push(Chunk {
                    kind: ChunkKind::Synthetic,
                    name: name.into(),
                    addr: Some(at),
                    size: h - at,
                    target: None,
                    line: 0,
                });
            }
            at = h + size;
        }
    }
    Some((dir, b))
}

fn pie_archs() -> [Arch; 2] {
    [Arch::Aarch64, Arch::X86_64]
}

// The PIE model is ACCEPTED: the `.rela.dyn` hole's RELATIVE records aim
// at one scratch word, and `ptrs`' `RELATIVE` record for `arr+40` targets
// the data hole's first byte, which is the end of the live `arr` (N3:
// a table word at a live item's end names that item). Each table
// reference into a range is REJECTED (P-I3), and so is each wrong fill
// of the relocation hole (P-I5).
#[test]
fn in3a_pie_tables_and_relocation_fill() {
    for arch in pie_archs() {
        let Some((_dir, b)) = build_pie(arch) else { return };
        let base = b.judge();
        assert!(base.is_empty(), "{arch:?}: {base:#?}");
        let text_hole = b.map.dead.iter().find(|d| d.section == ".text").unwrap().clone();
        let in_text = b.sec(".text").addr + text_hole.offset + 4;
        let data_hole = b.map.dead.iter().find(|d| d.section == ".data").unwrap().clone();
        let in_data = b.sec(".data").addr + data_hole.offset;
        let (ptrs, _) = b.placed("main.o", ".data");
        let recs = rela_dyn(&b);
        let live = recs.iter().find(|r| r.1 == ptrs).copied().expect("ptrs' RELATIVE record");
        assert_eq!(live.4, in_data, "{arch:?}: arr+40's record targets the hole's first byte");
        // A GOT word aimed into the code hole.
        let got = b.sec(".got").clone();
        let slot = recs
            .iter()
            .find(|r| r.1 >= got.addr && r.1 < got.addr + got.size)
            .copied()
            .expect("the GOT slot's RELATIVE record");
        let mut m = b.clone();
        m.patch(".got", slot.1, &in_text.to_le_bytes());
        expect(arch, "got word", &base, &m, "P-I3", ".got word");
        // A live RELATIVE record's target in the code hole.
        let mut m = b.clone();
        m.patch(".rela.dyn", slot.0 + 16, &in_text.to_le_bytes());
        expect(arch, "relative target", &base, &m, "P-I3", "targets");
        // A live record that writes into the data hole.
        let mut m = b.clone();
        m.patch(".rela.dyn", live.0, &(in_data + 8).to_le_bytes());
        expect(arch, "record writes", &base, &m, "P-I3", "writes at");
        // An `.eh_frame_hdr` entry whose location is in the code hole.
        let mut m = b.clone();
        let hdr = m.sec(".eh_frame_hdr").clone();
        let entry = hdr.addr + 12;
        let rel = in_text.wrapping_sub(hdr.addr) as u32;
        m.patch(".eh_frame_hdr", entry, &rel.to_le_bytes());
        expect(arch, "eh_frame_hdr", &base, &m, "P-I3", ".eh_frame_hdr entry");
        // The hole's records: one not RELATIVE, one NONE inside the
        // prefix, two aimed at different words.
        let hole = b.map.dead.iter().find(|d| d.section == ".rela.dyn").unwrap().clone();
        let at = b.sec(".rela.dyn").addr + hole.offset;
        let mut m = b.clone();
        let abs = if arch == Arch::Aarch64 { 257u64 } else { 1 };
        m.patch(".rela.dyn", at + 8, &abs.to_le_bytes());
        expect(arch, "hole record type", &base, &m, "P-I5", "is not RELATIVE");
        let mut m = b.clone();
        m.patch(".rela.dyn", at + 24 + 8, &0u64.to_le_bytes());
        expect(arch, "NONE in prefix", &base, &m, "P-I5", "inside DT_RELACOUNT's prefix");
        let mut m = b.clone();
        m.patch(".rela.dyn", at + 24, &(in_data + 8).to_le_bytes());
        expect(arch, "two words", &base, &m, "P-I5", "more than one word");
    }
}

// ---------------------------------------------------------------------
// Clang's DWARF 5 (WP-092: WP-091's second missing model): strings named
// through `DW_FORM_strx` (the unit's `.debug_str_offsets` table) and
// through the v5 line header (`DW_FORM_line_strp` into
// `.debug_line_str`), each the only reference to its string.

/// `main.c`, compiled by clang `-gdwarf-5` and linked by GNU ld, as an
/// incremental map that claims coverage with no range: "only.h" is named
/// only by the line header's file table, "helper" only through a
/// `.debug_str_offsets` entry.
fn build_clang5(arch: Arch) -> Option<(Scratch, Built)> {
    let clang = mortar_testutil::tool("clang-22")?;
    let ld = tool(arch, "ld.bfd")?;
    let dir = Scratch::new(&format!("dead-clang5-{}", arch.triple()));
    std::fs::write(dir.join("only.h"), "static inline int helper(int x) { return x + 1; }\n")
        .unwrap();
    std::fs::write(
        dir.join("main.c"),
        "#include \"only.h\"\nint g = 5;\nint main(void) { return helper(g); }\n",
    )
    .unwrap();
    let target = format!("--target={}", arch.triple());
    let mut flags = vec![target.as_str(), "-c", "-O0", "-g", "-gdwarf-5", "-fno-pic"];
    flags.extend(["-fno-asynchronous-unwind-tables", "main.c", "-o", "main.o"]);
    run(dir.path(), &clang, &flags);
    run(
        dir.path(),
        &ld,
        &["-x", "-e", "main", "-o", "out", "-Map=out.map", "-z", "noexecstack", "main.o"],
    );
    let output = std::fs::read(dir.join("out")).unwrap();
    let text = std::fs::read_to_string(dir.join("out.map")).unwrap();
    let mut map = mortar_plumb::map::parse(Format::Gnu, &text).unwrap();
    map.format = Format::Mortar;
    map.incremental = true;
    let files = vec![("main.o".to_string(), std::fs::read(dir.join("main.o")).unwrap())];
    let secs = sections(&output);
    Some((dir, Built { arch, output, map, files, secs }))
}

/// The offset of string `s` (with its NUL) in non-allocated section
/// `name`.
fn string_at(b: &Built, name: &str, s: &str) -> u64 {
    let sec = b.sec(name);
    let data = &b.output[sec.offset as usize..(sec.offset + sec.size) as usize];
    let needle: Vec<u8> = s.bytes().chain([0]).collect();
    let at = data.windows(needle.len()).position(|w| w == needle.as_slice());
    at.unwrap_or_else(|| panic!("no '{s}' in {name}")) as u64
}

// IN-3a check 1 for the DWARF 5 indirections (P-I4): a dead `strings`
// range under a string that only a live line header names, or only a
// live `.debug_str_offsets` entry, is REJECTED; the model is ACCEPTED.
#[test]
fn in3a_dwarf5_line_strings_and_str_offsets() {
    for arch in debug_archs() {
        let Some((_dir, b)) = build_clang5(arch) else { return };
        let base = b.judge();
        assert!(base.is_empty(), "{arch:?}: {base:#?}");
        let mut m = b.clone();
        let at = string_at(&m, ".debug_line_str", "only.h");
        m.dead(".debug_line_str", at, 7, DeadKind::Strings);
        expect(arch, "line header string", &base, &m, "P-I4", "live .debug_line table");
        let mut m = b.clone();
        let at = string_at(&m, ".debug_str", "helper");
        m.dead(".debug_str", at, 7, DeadKind::Strings);
        expect(arch, "str_offsets entry", &base, &m, "P-I4", "live .debug_str_offsets entry");
    }
}

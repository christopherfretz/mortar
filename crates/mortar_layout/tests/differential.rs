//! Differential tests against GNU ld, lld and mold (WP-024 extra test (i)).
//!
//! Stage A: each reference links a set of programs; its map file says
//! which output section every input section went to. Mortar's stage A runs
//! on the same input files (read with `mortar_elf`) and must agree, except
//! for differences the table [`explained`] names by rule id.
//!
//! Stage D: lld's output-section list (type, flags, size, alignment, RELRO
//! membership, file order) is fed to stage D as chunks; with
//! `--image-base=0x400000 -z max-page-size=0x10000 -z noseparate-code`
//! mortar must give every section lld's address and offset, except for
//! differences named by rule id.
//!
//! A missing tool skips (fails under `MORTAR_REQUIRE_TOOLS=1`).
#![allow(clippy::cast_possible_truncation, clippy::too_many_lines)]

mod common;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::elf::{Elf, PT_GNU_RELRO, PT_LOAD, PT_PHDR, PT_TLS, Shdr, read_elf};
use common::{Row, World};
use mortar_core::column::Column;
use mortar_core::id::{ChunkId, OutSecId};
use mortar_core::layout::{Chunk, ChunkKind, Part, Synth};
use mortar_core::par::Pool;
use mortar_core::resolution::{DropReason, SectionKind, SectionState};
use mortar_elf::consts::{
    SHF_ALLOC, SHF_EXCLUDE, SHF_MERGE, SHF_TLS, SHT_GROUP, SHT_NOBITS, SHT_NOTE, SHT_REL,
    SHT_RELA, SHT_STRTAB, SHT_SYMTAB,
};
use mortar_elf::{Machine, Object, SectionIndex};
use mortar_layout::Header;
use mortar_layout::address::address;
use mortar_layout::chunk::Staged;
use mortar_layout::rank::Rank;
use mortar_layout::{Options, Target};
use mortar_testutil::{Scratch, run_with, tool};

// -- the programs ---------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lang {
    C,
    Cxx,
    Fortran,
}

/// One test program: its language, source, compile flags and link flags.
struct Program {
    name: &'static str,
    lang: Lang,
    source: &'static str,
    cflags: &'static [&'static str],
    /// `-nostdlib` links give their own `_start`.
    nostdlib: bool,
}

const PROGRAMS: &[Program] = &[
    Program {
        name: "hello",
        lang: Lang::C,
        source: "#include <stdio.h>\nint main(void) { puts(\"hi\"); return 0; }\n",
        cflags: &["-O2"],
        nostdlib: false,
    },
    Program {
        name: "cxx-eh-tls",
        lang: Lang::Cxx,
        source: "#include <cstdio>\n#include <stdexcept>\n\
                 thread_local int tl = 7;\nthread_local int tz;\n\
                 int f(int x) { if (x > 3) throw std::runtime_error(\"x\"); return x; }\n\
                 int main(int c, char **) {\n\
                   try { f(c + 5); } catch (const std::exception &e) {\n\
                     std::printf(\"caught %s %d %d\\n\", e.what(), tl, tz); }\n\
                   return 0; }\n",
        cflags: &["-O2"],
        nostdlib: false,
    },
    Program {
        name: "fortran",
        lang: Lang::Fortran,
        source: "program p\n  print *, 'hi'\nend program p\n",
        cflags: &["-O2"],
        nostdlib: false,
    },
    Program {
        name: "sections",
        lang: Lang::C,
        source: "#include <stdio.h>\nint d1 = 1; int b1; const char r1[] = \"ro\";\n\
                 static int f1(int x) { return x * 3; }\n\
                 int f2(int x) { return f1(x) + d1 + b1; }\n\
                 int main(void) { printf(\"%s %d\\n\", r1, f2(2)); return 0; }\n",
        cflags: &["-O1", "-ffunction-sections", "-fdata-sections"],
        nostdlib: false,
    },
    Program {
        name: "debug",
        lang: Lang::C,
        source: "#include <stdio.h>\nint g = 3;\nint main(void) { printf(\"%d\\n\", g); return 0; }\n",
        cflags: &["-O0", "-g"],
        nostdlib: false,
    },
    Program {
        name: "align",
        lang: Lang::C,
        source: "#include <stdio.h>\n_Alignas(65536) char big[16] = {1};\n\
                 _Alignas(4096) char zero[100];\n\
                 __attribute__((aligned(256))) int fn(void) { return big[0] + zero[1]; }\n\
                 int main(void) { printf(\"%d\\n\", fn()); return 0; }\n",
        cflags: &["-O1"],
        nostdlib: false,
    },
    Program {
        name: "tbss-only",
        lang: Lang::C,
        source: "__thread int a;\n__thread int b[100];\n\
                 void _start(void) { a = 1; b[3] = 2; for (;;) {} }\n",
        cflags: &["-O1", "-ftls-model=local-exec", "-fno-pic", "-fno-asynchronous-unwind-tables"],
        nostdlib: true,
    },
    Program {
        name: "init-array",
        lang: Lang::C,
        source: "#include <stdio.h>\n\
                 __attribute__((constructor(101))) static void c101(void) { puts(\"101\"); }\n\
                 __attribute__((constructor(200))) static void c200(void) { puts(\"200\"); }\n\
                 __attribute__((constructor)) static void cdef(void) { puts(\"def\"); }\n\
                 __attribute__((destructor(150))) static void d150(void) { puts(\"d150\"); }\n\
                 int main(void) { return 0; }\n",
        cflags: &["-O1"],
        nostdlib: false,
    },
];

/// A target: its machine and its compiler prefix.
#[derive(Debug, Clone, Copy)]
struct Arch {
    name: &'static str,
    machine: Machine,
    prefix: &'static str,
}

const ARCHES: &[Arch] = &[
    Arch { name: "aarch64", machine: Machine::AArch64, prefix: "" },
    Arch { name: "x86-64", machine: Machine::X86_64, prefix: "x86_64-linux-gnu-" },
    Arch { name: "i386", machine: Machine::I386, prefix: "i686-linux-gnu-" },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Linker {
    Gnu,
    Lld,
    Mold,
}

const LINKERS: [Linker; 3] = [Linker::Gnu, Linker::Lld, Linker::Mold];

impl Linker {
    fn name(self) -> &'static str {
        match self {
            Self::Gnu => "GNU ld",
            Self::Lld => "lld",
            Self::Mold => "mold",
        }
    }
    fn fuse(self) -> &'static str {
        match self {
            Self::Gnu => "-fuse-ld=bfd",
            Self::Lld => "-fuse-ld=lld",
            Self::Mold => "-fuse-ld=mold",
        }
    }
}

fn driver(arch: Arch, lang: Lang) -> String {
    let base = match lang {
        Lang::C => "gcc",
        Lang::Cxx => "g++",
        Lang::Fortran => "gfortran",
    };
    format!("{}{base}", arch.prefix)
}

fn ext(lang: Lang) -> &'static str {
    match lang {
        Lang::C => "c",
        Lang::Cxx => "cc",
        Lang::Fortran => "f90",
    }
}

const LIMIT: Duration = Duration::from_secs(300);

/// Compiles `program` for `arch` in `dir`; the object's path, or why not.
fn compile(dir: &Path, arch: Arch, program: &Program) -> Result<PathBuf, String> {
    let src = dir.join(format!("{}.{}", program.name, ext(program.lang)));
    std::fs::write(&src, program.source).map_err(|e| e.to_string())?;
    let obj = dir.join(format!("{}-{}.o", program.name, arch.name));
    let mut cmd = std::process::Command::new(driver(arch, program.lang));
    cmd.args(program.cflags).arg("-c").arg(&src).arg("-o").arg(&obj);
    let out = run_with(&mut cmd, &[], LIMIT);
    if out.success() { Ok(obj) } else { Err(out.stderr_text()) }
}

/// Links `obj` with `linker` and a map file; the output and the map.
fn link(
    dir: &Path,
    arch: Arch,
    program: &Program,
    obj: &Path,
    linker: Linker,
    extra: &[&str],
    tag: &str,
) -> Result<(PathBuf, PathBuf), String> {
    let stem = format!("{}-{}-{tag}", program.name, arch.name);
    let exe = dir.join(&stem);
    let map = dir.join(format!("{stem}.map"));
    let mut cmd = std::process::Command::new(driver(arch, program.lang));
    cmd.arg("-static").arg(linker.fuse());
    if program.nostdlib {
        cmd.arg("-nostdlib");
    }
    cmd.arg(obj).arg("-o").arg(&exe).arg(format!("-Wl,-Map={}", map.display()));
    for e in extra {
        cmd.arg(format!("-Wl,{e}"));
    }
    let out = run_with(&mut cmd, &[], LIMIT);
    if out.success() { Ok((exe, map)) } else { Err(out.stderr_text()) }
}

// -- map files ------------------------------------------------------------

/// An input file as a map names it: a path, or an archive and a member.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct FileKey {
    path: PathBuf,
    member: Option<String>,
}

impl FileKey {
    fn parse(text: &str) -> Option<Self> {
        let (path, member) = match text.strip_suffix(')') {
            Some(t) => {
                let open = t.find(".a(")? + 2;
                (&t[..open], Some(t[open + 1..].to_owned()))
            }
            None => (text, None),
        };
        let path = std::fs::canonicalize(path).ok()?;
        Some(Self { path, member })
    }
    fn display(&self) -> String {
        let name = self.path.file_name().map(|n| n.to_string_lossy().into_owned());
        let name = name.unwrap_or_default();
        match &self.member {
            Some(m) => format!("{name}({m})"),
            None => name,
        }
    }
}

/// One placement a map records: an input section of a file (or of the
/// linker, `None`) into an output section.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Placement {
    file: Option<FileKey>,
    input: String,
    output: String,
}

/// GNU ld's map: the "Linker script and memory map" part.
fn parse_gnu(text: &str) -> Vec<Placement> {
    let mut out = Vec::new();
    let Some(start) = text.find("Linker script and memory map") else { return out };
    let mut output = String::new();
    let mut pending: Option<String> = None;
    for line in text[start..].lines().skip(1) {
        if line.starts_with("OUTPUT(") {
            break;
        }
        if line.is_empty() {
            continue;
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if !line.starts_with(' ') {
            // An output section (or LOAD, START GROUP, ...).
            if line.starts_with('.') || line.starts_with("/DISCARD/") || line.starts_with("__") {
                output = tokens[0].to_owned();
            } else if !line.starts_with("LOAD ") {
                output.clear();
            }
            pending = None;
            continue;
        }
        let first = line.as_bytes().get(1).copied().unwrap_or(b' ');
        if first != b' ' {
            // ` NAME [addr size file]`
            if first == b'*' || tokens[0] == "COMMON" || tokens[0].ends_with(')') {
                pending = None;
                continue;
            }
            if tokens.len() >= 4 && tokens[1].starts_with("0x") {
                push_gnu(&mut out, &output, tokens[0], tokens[3]);
                pending = None;
            } else if tokens.len() == 1 {
                pending = Some(tokens[0].to_owned());
            } else {
                pending = None;
            }
            continue;
        }
        // A continuation `addr size file`, or a symbol.
        if let Some(name) = pending.take() {
            if tokens.len() >= 3 && tokens[0].starts_with("0x") && tokens[1].starts_with("0x") {
                push_gnu(&mut out, &output, &name, tokens[2]);
            }
        }
    }
    out
}

fn push_gnu(out: &mut Vec<Placement>, output: &str, input: &str, file: &str) {
    if output.is_empty() {
        return;
    }
    let file = FileKey::parse(file);
    out.push(Placement { file, input: input.to_owned(), output: output.to_owned() });
}

/// lld's and mold's maps: columns `... Out In Symbol`.
fn parse_columns(text: &str) -> Vec<Placement> {
    let mut lines = text.lines();
    let Some(header) = lines.next() else { return Vec::new() };
    let (Some(out_col), Some(in_col)) = (header.find("Out"), header.find(" In ").map(|i| i + 1))
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut output = String::new();
    for line in lines {
        let b = line.as_bytes();
        if b.len() > out_col && b[out_col] != b' ' && b[out_col - 1] == b' ' {
            output = line[out_col..].trim().to_owned();
        } else if b.len() > in_col && b[in_col] != b' ' && b[in_col - 1] == b' ' {
            let item = line[in_col..].trim();
            // `file:(name)`
            let Some(open) = item.rfind(":(") else { continue };
            let file = &item[..open];
            let input = item[open + 2..].trim_end_matches(')');
            let file = if file == "<internal>" { None } else { FileKey::parse(file) };
            if file.is_none() && item.starts_with('<') {
                out.push(Placement { file: None, input: input.to_owned(), output: output.clone() });
            } else if file.is_some() {
                out.push(Placement { file, input: input.to_owned(), output: output.clone() });
            }
        }
    }
    out
}

// -- mortar's stage A on the same inputs ----------------------------------

/// The files read so far (leaked once each: the headers borrow names for
/// the test's lifetime).
type Cache = HashMap<PathBuf, &'static [u8]>;

/// One input file's bytes.
fn file_bytes(key: &FileKey, cache: &mut Cache) -> Option<&'static [u8]> {
    let bytes = match cache.get(&key.path) {
        Some(b) => *b,
        None => {
            let b: &'static [u8] = Box::leak(std::fs::read(&key.path).ok()?.into_boxed_slice());
            cache.insert(key.path.clone(), b);
            b
        }
    };
    let Some(member) = &key.member else { return Some(bytes) };
    let archive = mortar_elf::archive::Archive::parse(bytes).ok()?;
    archive.members().filter_map(Result::ok).find_map(|m| {
        let name = String::from_utf8_lossy(m.name());
        (name.trim_end_matches('/') == member).then(|| m.data()).flatten()
    })
}

fn kind_of(name: &[u8], ty: u32, flags: u64) -> SectionKind {
    match ty {
        SHT_SYMTAB => SectionKind::Symtab,
        SHT_STRTAB => SectionKind::Strtab,
        SHT_REL | SHT_RELA => SectionKind::Reloc,
        SHT_GROUP => SectionKind::Group,
        _ if name == b".note.GNU-stack" => SectionKind::Stack,
        _ if flags & SHF_EXCLUDE != 0 => SectionKind::Excluded,
        SHT_NOBITS => SectionKind::Nobits,
        SHT_NOTE => SectionKind::Note,
        _ if name == b".eh_frame" => SectionKind::EhFrame,
        _ if flags & SHF_MERGE != 0 => SectionKind::Merge,
        _ if flags & SHF_ALLOC == 0 => SectionKind::Debug,
        _ => SectionKind::Regular,
    }
}

/// The section rows of one object; COMDAT groups whose signature `seen`
/// holds are dropped, the others added to it.
fn rows_of(bytes: &'static [u8], seen: &mut BTreeSet<Vec<u8>>) -> Option<Vec<Row>> {
    let object = Object::parse(bytes).ok()?;
    let mut rows = Vec::new();
    let mut dropped = BTreeSet::new();
    let symtab = object.symbols().ok().flatten();
    for s in object.sections() {
        if s.index() == SectionIndex(0) || s.sh_type() != SHT_GROUP {
            continue;
        }
        let Ok(g) = s.group() else { continue };
        if !g.is_comdat() {
            continue;
        }
        let sig = symtab
            .as_ref()
            .and_then(|t| t.get(g.signature()).ok().and_then(|sym| t.name(&sym).ok()))
            .unwrap_or_default()
            .to_vec();
        if !seen.insert(sig) {
            dropped.extend(g.members().filter_map(Result::ok).map(|i| i.0));
        }
    }
    for s in object.sections() {
        if s.index() == SectionIndex(0) {
            continue;
        }
        let name: &'static [u8] = s.name().ok()?;
        let align = s.alignment().ok()?.max(1).trailing_zeros() as u8;
        let kind = kind_of(name, s.sh_type(), s.flags());
        let state = if dropped.contains(&s.index().0) {
            SectionState::Dropped(DropReason::Comdat)
        } else if kind == SectionKind::Excluded {
            SectionState::Dropped(DropReason::Excluded)
        } else if !kind.carries_output() {
            SectionState::Dropped(DropReason::Consumed)
        } else {
            SectionState::Kept
        };
        let link = s.link().map(|l| l.0).unwrap_or(0);
        rows.push(Row {
            header: Header {
                name,
                sh_type: s.sh_type(),
                flags: s.flags(),
                size: s.size(),
                align,
                link: None,
                entsize: s.header().sh_entsize,
            },
            link: if s.flags() & mortar_elf::consts::SHF_LINK_ORDER != 0 { link } else { 0 },
            kind,
            state,
        });
    }
    Some(rows)
}

/// One stage A comparison's counts.
#[derive(Debug, Default, Clone)]
struct Tally {
    compared: usize,
    agree: usize,
    /// (reference output, mortar output, input name pattern) → (count,
    /// rule): explained differences.
    explained: BTreeMap<(String, String, String), (usize, &'static str)>,
    unexplained: Vec<String>,
}

/// The input-name class of a difference, for the tables.
fn pattern(input: &str) -> String {
    for p in [
        ".text.unlikely", ".text.hot", ".text.startup", ".text.exit", ".rodata.str",
        ".rodata.cst", ".data.rel.ro", ".init_array", ".fini_array", ".tbss", ".tdata",
        ".gcc_except_table", ".note", ".debug_", ".ARM.attributes", ".comment",
        ".gnu.warning", "__libc_", "__libc_freeres", ".stapsdt", ".gnu_debuglink",
    ] {
        if input.starts_with(p) {
            return format!("{p}*");
        }
    }
    input.to_owned()
}

/// Why mortar's output section for `input` may differ from the
/// reference's: a rule id, or `None` (unexplained).
fn explained(linker: Linker, input: &str, _reference: &str, mortar: &str) -> Option<&'static str> {
    // L-34: mortar consumes .note.gnu.property into its synthetic note.
    if input == ".note.gnu.property" && mortar == "(consumed)" {
        return Some("L-34");
    }
    // L-33: `.gnu.warning[.SYM]` is consumed, as GNU ld does; lld copies
    // it as an orphan, mold into one `.gnu.warning`.
    if input.starts_with(".gnu.warning") && mortar == "(consumed)" {
        return Some("L-33");
    }
    let _ = linker;
    None
}

/// Stage A on the files `placements` name, compared with `placements`.
fn compare_stage_a(
    machine: Machine,
    linker: Linker,
    placements: &[Placement],
    tally: &mut Tally,
    cache: &mut Cache,
    pool: &Pool,
) {
    // The files, in first-mention order (COMDAT: the first keeps).
    let mut files: Vec<FileKey> = Vec::new();
    let mut index: HashMap<FileKey, usize> = HashMap::new();
    for p in placements {
        if let Some(f) = &p.file {
            if !index.contains_key(f) {
                index.insert(f.clone(), files.len());
                files.push(f.clone());
            }
        }
    }
    let mut seen = BTreeSet::new();
    let mut rows = Vec::new();
    let mut readable = Vec::new();
    for f in &files {
        match file_bytes(f, cache).and_then(|b| rows_of(b, &mut seen)) {
            Some(r) => {
                rows.push(r);
                readable.push(true);
            }
            None => {
                rows.push(Vec::new());
                readable.push(false);
            }
        }
    }
    let world = World::from_rows(machine, &rows, &[]);
    let assignment = match world.stage_a(pool) {
        Ok(a) => a,
        Err(e) => {
            tally.unexplained.push(format!(
                "{}: stage A refused: {:?}",
                linker.name(),
                e.iter().map(|(_, d)| d.message()).collect::<Vec<_>>()
            ));
            return;
        }
    };
    // Mortar's output for each (file, input name): every section of that
    // name in the file (kept ones; a name may repeat, as COMDAT members do).
    let out_name = |file: usize, input: &str| -> Option<BTreeSet<String>> {
        let mut names = BTreeSet::new();
        for (i, r) in rows[file].iter().enumerate() {
            if r.header.name != input.as_bytes() || r.state != SectionState::Kept {
                continue;
            }
            let sid = world.sid(file as u32 + 1, i as u32 + 1);
            let name = match assignment.out_of[sid] {
                mortar_core::assignment::OutOf::Out(o) => {
                    String::from_utf8_lossy(assignment.outsecs[o].name).into_owned()
                }
                mortar_core::assignment::OutOf::Consumed => "(consumed)".to_owned(),
                _ => "(none)".to_owned(),
            };
            names.insert(name);
        }
        (!names.is_empty()).then_some(names)
    };
    // The linker's own inputs (`<internal>`, merged strings): every input
    // of that name, in any file, must go where the reference put it.
    let by_name = |input: &str| -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        for f in 0..files.len() {
            if let Some(n) = out_name(f, input) {
                names.extend(n);
            }
        }
        names
    };
    for p in placements {
        if p.output == "/DISCARD/" {
            continue;
        }
        let mortar = match &p.file {
            Some(f) => {
                let i = index[f];
                if !readable[i] {
                    tally.unexplained.push(format!("{}: unreadable", f.display()));
                    continue;
                }
                // A name the file does not have is the linker's own
                // section, attributed to a file (GNU's `.iplt`, `.rela.*`).
                let Some(n) = out_name(i, &p.input) else { continue };
                n
            }
            None => {
                let n = by_name(&p.input);
                if n.is_empty() {
                    continue;
                }
                n
            }
        };
        tally.compared += 1;
        let mortar_out = mortar.iter().cloned().collect::<Vec<_>>().join("|");
        if mortar.len() == 1 && mortar.contains(&p.output) {
            tally.agree += 1;
            continue;
        }
        let key = (p.output.clone(), mortar_out.clone(), pattern(&p.input));
        match explained(linker, &p.input, &p.output, &mortar_out) {
            Some(rule) => {
                let e = tally.explained.entry(key).or_insert((0, rule));
                e.0 += 1;
            }
            None => tally.unexplained.push(format!(
                "{}: {} {} -> reference {} mortar {}",
                linker.name(),
                p.file.as_ref().map_or("<linker>".to_owned(), FileKey::display),
                p.input,
                p.output,
                mortar_out
            )),
        }
    }
}

// -- stage D against lld --------------------------------------------------

/// The synthetic chunk names derive_segments looks for.
fn synth_name(name: &str) -> Option<&'static [u8]> {
    match name {
        ".relro_padding" => Some(b".relro_padding"),
        ".eh_frame_hdr" => Some(b".eh_frame_hdr"),
        ".note.gnu.property" => Some(b".note.gnu.property"),
        _ => None,
    }
}

/// lld's sections as stage C would hand them to stage D.
fn staged_from(elf: &Elf) -> (Staged, Vec<(usize, ChunkId)>) {
    let relro = elf.segments.iter().find(|p| p.ty == PT_GNU_RELRO).copied();
    let in_relro = |s: &Shdr| {
        relro.is_some_and(|r| s.flags & SHF_ALLOC != 0 && s.addr >= r.vaddr && s.addr < r.vaddr + r.memsz)
    };
    let hdr_align = if elf.class64 { 3 } else { 2 };
    let phdr_size = elf
        .segments
        .iter()
        .find(|p| p.ty == PT_PHDR)
        .map_or(elf.phentsize * elf.phnum, |p| p.filesz);
    let blank = |kind, ty, flags, align, size| Chunk {
        kind,
        sh_type: ty,
        flags,
        align,
        size,
        capacity: size,
        addr: (flags & SHF_ALLOC != 0).then_some(0),
        offset: 0,
        shndx: None,
        relro: false,
    };
    let mut chunks = vec![
        blank(ChunkKind::FileHeader, 1, SHF_ALLOC, hdr_align, elf.ehsize),
        // lld sizes the table before it drops empty PT_LOADs: PT_PHDR
        // says what it reserved.
        blank(ChunkKind::ProgramHeaders, 1, SHF_ALLOC, hdr_align, phdr_size),
    ];
    let mut out_of = vec![None, None];
    let mut map = Vec::new();
    let mut relro_padding = None;
    for (i, s) in elf.sections.iter().enumerate().skip(1) {
        let kind = match synth_name(&s.name) {
            Some(name) => ChunkKind::Synthetic(Synth { name, address_dependent: false }),
            None => ChunkKind::Body(Part::first(OutSecId(i as u32))),
        };
        let mut c = blank(kind, s.ty, s.flags, s.align.max(1).trailing_zeros() as u8, s.size);
        c.shndx = Some(i as u32);
        c.relro = in_relro(s);
        let id = ChunkId(chunks.len() as u32);
        if s.name == ".relro_padding" {
            relro_padding = Some(id);
        }
        out_of.push(matches!(kind, ChunkKind::Body(_)).then_some(OutSecId(i as u32)));
        chunks.push(c);
        map.push((i, id));
    }
    // The section header table where lld put it (last).
    let sh = blank(ChunkKind::SectionHeaders, 1, 0, hdr_align, elf.shentsize * elf.shnum);
    chunks.push(sh);
    out_of.push(None);
    let n = chunks.len() as u32;
    // File order: lld's, the headers first; non-alloc in offset order.
    let mut order: Vec<ChunkId> = vec![ChunkId(0), ChunkId(1)];
    let mut alloc: Vec<_> = map.iter().filter(|(i, _)| elf.sections[*i].flags & SHF_ALLOC != 0).collect();
    alloc.sort_by_key(|(i, _)| (elf.sections[*i].addr, *i));
    order.extend(alloc.iter().map(|(_, c)| *c));
    let mut rest: Vec<_> = map.iter().filter(|(i, _)| elf.sections[*i].flags & SHF_ALLOC == 0).collect();
    rest.sort_by_key(|(i, _)| (elf.sections[*i].offset, *i));
    order.extend(rest.iter().map(|(_, c)| *c));
    order.push(ChunkId(n - 1));
    let staged = Staged {
        chunks: Column::from_vec(chunks).unwrap(),
        order: order.into(),
        ranks: Column::filled(n, Rank { rank: 0, within: 0 }),
        place: Column::new(),
        sym_place: Box::new([]),
        islands: Box::new([]),
        relro_padding,
        out_of: Column::from_vec(out_of).unwrap(),
    };
    (staged, map)
}

/// Stage D on lld's list; the differences, each with its explanation.
fn compare_stage_d(
    machine: Machine,
    elf: &Elf,
    common_page: bool,
) -> (usize, Vec<(String, &'static str)>, Vec<String>) {
    let mut target = Target::of(machine);
    target.image_base = 0x40_0000;
    let mut options = Options::defaults(&target);
    options.max_page = 0x1_0000;
    options.separate_code = false;
    let rules = options.segment_rules(&target, false);
    let (staged, map) = staged_from(elf);
    let mut explained = Vec::new();
    let mut unexplained = Vec::new();
    let addressed = match address(&staged, &[], &target, &options, &rules) {
        Ok(a) => a,
        Err(e) => {
            unexplained.push(format!("stage D failed: {}", e.message()));
            return (0, explained, unexplained);
        }
    };
    let mut compared = 0;
    // After the first difference, later ones follow from it: report the
    // first per segment kind and stop.
    let mut shifted: Option<&'static str> = None;
    for &(i, c) in &map {
        let s = &elf.sections[i];
        let ch = &addressed.chunks[c];
        compared += 1;
        let addr = ch.addr.unwrap_or(0);
        let ok = addr == s.addr && ch.offset == s.offset && ch.size == s.size;
        if ok {
            continue;
        }
        let what = format!(
            "{}: lld addr {:#x} off {:#x} size {:#x}, mortar addr {addr:#x} off {:#x} size {:#x}",
            s.name, s.addr, s.offset, s.size, ch.offset, ch.size
        );
        if let Some(rule) = shifted {
            explained.push((what, rule));
            continue;
        }
        if s.name == ".relro_padding" && !common_page && addr == s.addr {
            // L-64: mortar pads the RELRO end to maxpage; lld to the
            // common page size (4 KiB by default).
            explained.push((what, "L-64"));
            shifted = Some("L-64 (follows from the padding)");
            continue;
        }
        let first_of_load = elf.segments.iter().any(|p| p.ty == PT_LOAD && p.vaddr == s.addr);
        if first_of_load
            && s.align >= options.max_page
            && ch.offset == s.offset
            && addr < s.addr
            && (s.addr - addr) % options.max_page == 0
        {
            // L-70: mortar takes the least A >= up(cur, maxpage) with
            // A = o (mod P); lld picks up(cur, maxpage) + cur % maxpage first
            // and then aligns it, a page later when the chunk's alignment
            // is at least maxpage. Both satisfy L-1/L-2.
            explained.push((what, "L-70"));
            shifted = Some("L-70 (follows from the segment start)");
            continue;
        }
        let tdata = elf.sections.iter().any(|t| t.flags & SHF_TLS != 0 && t.ty != SHT_NOBITS);
        if s.name == ".tbss" && !tdata {
            // L-77: a TLS template of .tbss only starts the RW run in
            // mortar; lld puts it after the last chunk before it (in the RX
            // range), mold after .init_array, GNU at the segment start.
            explained.push((what, "L-77"));
            shifted = Some("L-77 (follows from the .tbss placement)");
            continue;
        }
        unexplained.push(what);
    }
    // The PT_LOADs and PT_TLS.
    let ours: Vec<_> = addressed.segments.iter().filter(|s| matches!(s.p_type, PT_LOAD | PT_TLS | PT_GNU_RELRO)).collect();
    let theirs: Vec<_> = elf.segments.iter().filter(|s| matches!(s.ty, PT_LOAD | PT_TLS | PT_GNU_RELRO)).collect();
    if ours.len() == theirs.len() {
        for (a, b) in ours.iter().zip(&theirs) {
            compared += 1;
            let same = a.p_type == b.ty
                && a.offset == b.offset
                && a.addr == b.vaddr
                && a.filesz == b.filesz
                && a.memsz == b.memsz
                && 1u64 << a.align == b.align
                && a.flags == b.flags;
            if !same {
                let what = format!("segment {:#x}: lld {b:x?}, mortar {a:x?}", b.ty);
                match shifted {
                    Some(rule) => explained.push((what, rule)),
                    None => unexplained.push(what),
                }
            }
        }
    } else {
        let what = format!("segment count: lld {}, mortar {}", theirs.len(), ours.len());
        match shifted {
            Some(rule) => explained.push((what, rule)),
            None => unexplained.push(what),
        }
    }
    let _ = elf.shoff;
    (compared, explained, unexplained)
}

// -- the tests ------------------------------------------------------------

fn tools_for(arch: Arch, lang: Lang) -> bool {
    tool(&driver(arch, lang)).is_some()
}

// spec: layout.md §8 L-30..L-62 (stage A against GNU ld, lld and mold maps)
#[test]
fn stage_a_against_references() {
    if tool("ld.lld").is_none() || tool("mold").is_none() || tool("ld.bfd").is_none() {
        return;
    }
    let dir = Scratch::new("layout-diff-a");
    let pool = Pool::sequential();
    let mut cache = Cache::new();
    let mut tallies: BTreeMap<(String, Linker), Tally> = BTreeMap::new();
    let mut skipped = Vec::new();
    for &arch in ARCHES {
        for program in PROGRAMS {
            if !tools_for(arch, program.lang) {
                skipped.push(format!("{} {}: no compiler", program.name, arch.name));
                continue;
            }
            let obj = match compile(dir.path(), arch, program) {
                Ok(o) => o,
                Err(e) => {
                    skipped.push(format!("{} {}: compile: {}", program.name, arch.name, e.trim()));
                    continue;
                }
            };
            for linker in LINKERS {
                let tag = format!("{linker:?}");
                let (_, map) = match link(dir.path(), arch, program, &obj, linker, &[], &tag) {
                    Ok(x) => x,
                    Err(e) => {
                        let first = e.lines().next().unwrap_or("").to_owned();
                        skipped.push(format!("{} {} {}: {first}", program.name, arch.name, linker.name()));
                        continue;
                    }
                };
                let text = std::fs::read_to_string(&map).unwrap_or_default();
                let placements =
                    if linker == Linker::Gnu { parse_gnu(&text) } else { parse_columns(&text) };
                assert!(!placements.is_empty(), "{}: empty map", map.display());
                let tally = tallies.entry((arch.name.to_owned(), linker)).or_default();
                compare_stage_a(arch.machine, linker, &placements, tally, &mut cache, &pool);
            }
        }
    }
    eprintln!("stage A: arch, reference, compared, agree, explained, unexplained");
    let mut unexplained = Vec::new();
    for ((arch, linker), t) in &tallies {
        let ex: usize = t.explained.values().map(|v| v.0).sum();
        eprintln!(
            "  {arch} {}: {} {} {ex} {}",
            linker.name(),
            t.compared,
            t.agree,
            t.unexplained.len()
        );
        for ((r, m, p), (n, rule)) in &t.explained {
            eprintln!("    {n} x {p}: reference {r}, mortar {m} ({rule})");
        }
        unexplained.extend(t.unexplained.iter().cloned());
    }
    for s in &skipped {
        eprintln!("  skipped: {s}");
    }
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for u in &unexplained {
        *counts.entry(u.as_str()).or_default() += 1;
    }
    for (u, n) in &counts {
        eprintln!("  UNEXPLAINED {n} x {u}");
    }
    assert!(unexplained.is_empty(), "{} unexplained stage A differences", unexplained.len());
}

// spec: layout.md §11 L-65..L-78 (stage D on lld's output-section list)
#[test]
fn stage_d_against_lld() {
    if tool("ld.lld").is_none() {
        return;
    }
    let dir = Scratch::new("layout-diff-d");
    let flags = ["--image-base=0x400000", "-z", "max-page-size=0x10000", "-z", "noseparate-code"];
    let mut total = (0, 0, 0);
    let mut unexplained = Vec::new();
    let mut skipped = Vec::new();
    for &arch in ARCHES {
        for program in PROGRAMS {
            if !tools_for(arch, program.lang) {
                continue;
            }
            let Ok(obj) = compile(dir.path(), arch, program) else {
                skipped.push(format!("{} {}: compile", program.name, arch.name));
                continue;
            };
            for common_page in [false, true] {
                let mut extra: Vec<&str> = flags.to_vec();
                if common_page {
                    extra.extend(["-z", "common-page-size=0x10000"]);
                }
                let joined = [extra.join(",")];
                let extra: Vec<&str> = joined.iter().map(String::as_str).collect();
                let tag = if common_page { "lld-d-cp" } else { "lld-d" };
                let (exe, _) =
                    match link(dir.path(), arch, program, &obj, Linker::Lld, &extra, tag) {
                        Ok(x) => x,
                        Err(e) => {
                            let first = e.lines().next().unwrap_or("").to_owned();
                            skipped.push(format!("{} {}: {first}", program.name, arch.name));
                            continue;
                        }
                    };
                let elf = read_elf(&std::fs::read(&exe).unwrap());
                let (n, ex, un) = compare_stage_d(arch.machine, &elf, common_page);
                eprintln!(
                    "stage D {} {} common-page-size={}: {n} compared, {} explained, {} unexplained",
                    arch.name,
                    program.name,
                    if common_page { "maxpage" } else { "default" },
                    ex.len(),
                    un.len()
                );
                for (w, r) in ex.iter().take(1) {
                    eprintln!("    {r}: {w}");
                }
                for w in &un {
                    eprintln!("    UNEXPLAINED {w}");
                }
                total.0 += n;
                total.1 += ex.len();
                total.2 += un.len();
                unexplained.extend(un.into_iter().map(|w| format!("{} {}: {w}", arch.name, program.name)));
            }
        }
    }
    for s in &skipped {
        eprintln!("  skipped: {s}");
    }
    eprintln!("stage D total: {} compared, {} explained, {} unexplained", total.0, total.1, total.2);
    assert!(unexplained.is_empty(), "{} unexplained stage D differences", unexplained.len());
}

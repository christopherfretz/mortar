//! Test support: phase 60 over bare objects (a `Link` without the
//! pipeline, for scans of whole libraries), relocation classes for the x86
//! targets (no `mortar_arch` implementation exists for them yet), and
//! `readelf` frame counts.
#![allow(dead_code)] // each test binary uses a part

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mortar_core::inputs::RelocForm;
use mortar_core::resolution::SectionKind;
use mortar_core::{FileId, OutSecId, SectionId, SymbolId};
use mortar_elf::consts::{
    SHF_EXCLUDE, SHF_MERGE, SHT_GROUP, SHT_PROGBITS, SHT_REL, SHT_RELA, SHT_STRTAB, SHT_SYMTAB,
};
use mortar_elf::{Class, Machine, Object, Section, SectionIndex, SymbolIndex, SymbolSection};
use mortar_merge::What;
use mortar_merge::eh::{RelocClass, Target};
use mortar_merge::phase::{ArchRelocs, Link, LinkSection, RelocInfo, Relocs};

/// `SHT_X86_64_UNWIND`.
const SHT_X86_64_UNWIND: u32 = 0x7000_0001;

/// One section's place in [`Objects`].
#[derive(Clone, Copy)]
struct Row {
    kind: Option<SectionKind>,
    out: OutSecId,
    reloc: SectionIndex,
}

/// A link of bare objects: every section kept, output sections by name
/// prefix, globals resolved by name among the objects (first definition
/// wins), the rest undefined.
pub struct Objects {
    pub names: Vec<String>,
    data: Vec<&'static [u8]>,
    base: Vec<u32>,
    rows: Vec<Vec<Row>>,
    globals: Vec<Vec<Target>>,
    pub machine: Machine,
    class: Class,
}

fn out_of(name: &[u8]) -> OutSecId {
    let prefixes: [&[u8]; 6] =
        [b".rodata", b".debug_str", b".debug_line_str", b".comment", b".eh_frame", b".data"];
    OutSecId(prefixes.iter().position(|p| name.starts_with(p)).unwrap_or(6) as u32)
}

/// The kind resolve.md §10 gives (the rows phase 60 cares about).
fn kind(machine: Machine, s: &Section<'_>) -> Option<SectionKind> {
    let h = s.header();
    if s.index().0 == 0
        || matches!(h.sh_type, SHT_REL | SHT_RELA | SHT_SYMTAB | SHT_STRTAB | SHT_GROUP)
        || h.sh_flags & SHF_EXCLUDE != 0
    {
        return None;
    }
    let name = s.name().unwrap_or_default();
    if name == b".eh_frame" || (machine == Machine::X86_64 && h.sh_type == SHT_X86_64_UNWIND) {
        Some(SectionKind::EhFrame)
    } else if h.sh_flags & SHF_MERGE != 0 && h.sh_type == SHT_PROGBITS && h.sh_entsize > 0 {
        Some(SectionKind::Merge)
    } else {
        Some(SectionKind::Regular)
    }
}

impl Objects {
    /// `files`: (name, bytes) of relocatable objects of one machine.
    pub fn new(files: Vec<(String, Vec<u8>)>) -> Self {
        let mut names = Vec::new();
        let mut data: Vec<&'static [u8]> = Vec::new();
        for (n, b) in files {
            names.push(n);
            data.push(Box::leak(b.into_boxed_slice()));
        }
        let objects: Vec<Object<'static>> =
            data.iter().map(|d| Object::parse(d).expect("an object")).collect();
        let machine = objects.first().map_or(Machine::X86_64, Object::machine);
        let class = objects.first().map_or(Class::Elf64, Object::class);
        let mut base = Vec::new();
        let mut next = 0u32;
        let mut rows = Vec::new();
        for o in &objects {
            base.push(next);
            next += o.section_count();
            let mut r: Vec<Row> = o
                .sections()
                .map(|s| Row {
                    kind: kind(machine, &s),
                    out: out_of(s.name().unwrap_or_default()),
                    reloc: SectionIndex(0),
                })
                .collect();
            for s in o.sections() {
                if matches!(s.sh_type(), SHT_REL | SHT_RELA) {
                    let t = s.header().sh_info as usize;
                    if let Some(row) = r.get_mut(t) {
                        row.reloc = s.index();
                    }
                }
            }
            rows.push(r);
        }
        // Globals by name: the first definition, else undefined.
        let mut defs: HashMap<&[u8], Target> = HashMap::new();
        let mut ids: HashMap<&[u8], SymbolId> = HashMap::new();
        for (f, o) in objects.iter().enumerate() {
            let Some(t) = o.symbols().unwrap() else { continue };
            for (i, sym) in t.globals() {
                let name = t.name(&sym).unwrap();
                let n = ids.len() as u32;
                ids.entry(name).or_insert(SymbolId(n));
                if let Ok(SymbolSection::Section(s)) = t.section_of(i, &sym) {
                    defs.entry(name).or_insert(Target::Section {
                        section: SectionId(base[f] + s.0),
                        value: sym.st_value,
                    });
                }
            }
        }
        let globals = objects
            .iter()
            .map(|o| {
                let Some(t) = o.symbols().unwrap() else { return Vec::new() };
                t.globals()
                    .map(|(_, sym)| {
                        let name = t.name(&sym).unwrap();
                        defs.get(name).copied().unwrap_or(Target::Undefined {
                            symbol: ids[name],
                            weak: sym.st_info >> 4 == 2,
                        })
                    })
                    .collect()
            })
            .collect();
        Self { names, data, base, rows, globals, machine, class }
    }

    pub fn object(&self, f: usize) -> Object<'static> {
        Object::parse(self.data[f]).unwrap()
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// The file a section id is in.
    pub fn file_of(&self, s: SectionId) -> usize {
        self.base.partition_point(|&b| b <= s.0) - 1
    }

    /// The id of section `index` of file `f`.
    pub fn id(&self, f: usize, index: u32) -> SectionId {
        SectionId(self.base[f] + index)
    }
}

impl Link<'static> for Objects {
    fn files(&self) -> u32 {
        self.data.len() as u32
    }

    fn object(&self, file: FileId) -> Option<Object<'static>> {
        Some(Object::parse(self.data[file.0 as usize]).unwrap())
    }

    fn section_id(&self, file: FileId, index: SectionIndex) -> Option<SectionId> {
        let f = file.0 as usize;
        (index.0 < self.rows[f].len() as u32).then(|| SectionId(self.base[f] + index.0))
    }

    fn section(&self, file: FileId, index: SectionIndex) -> Option<LinkSection> {
        let f = file.0 as usize;
        let row = self.rows[f].get(index.0 as usize)?;
        Some(LinkSection {
            id: SectionId(self.base[f] + index.0),
            kind: row.kind?,
            out: row.out,
            reloc: row.reloc,
        })
    }

    fn global(&self, file: FileId, index: SymbolIndex, first: u32) -> Result<Target, What> {
        Ok(self.globals[file.0 as usize][(index.0 - first) as usize])
    }

    fn bytes(&self, _file: FileId, s: &Section<'static>) -> Result<Cow<'static, [u8]>, What> {
        s.data().map(Cow::Borrowed).map_err(|e| What::Malformed(e.to_string()))
    }

    fn class(&self) -> Class {
        self.class
    }

    fn form(&self) -> RelocForm {
        if self.machine == Machine::I386 { RelocForm::Rel } else { RelocForm::Rela }
    }
}

/// x86-64 and i386 relocation classes, for `.eh_frame` (`spec/arch.md`
/// §3.9: `Pc`, `AbsWord` and `AbsNarrow`, `None`; everything else is
/// `Other`).
pub struct X86(pub Machine);

impl Relocs for X86 {
    fn classify(&self, r_type: u32, _bytes: &[u8], _offset: u64) -> Result<RelocInfo, What> {
        let (name, class, width) = match (self.0, r_type) {
            (Machine::X86_64, 0) => ("R_X86_64_NONE", RelocClass::None, 0),
            (Machine::X86_64, 1) => ("R_X86_64_64", RelocClass::Abs, 8),
            (Machine::X86_64, 2) => ("R_X86_64_PC32", RelocClass::Pc, 4),
            (Machine::X86_64, 10) => ("R_X86_64_32", RelocClass::Abs, 4),
            (Machine::X86_64, 11) => ("R_X86_64_32S", RelocClass::Abs, 4),
            (Machine::X86_64, 24) => ("R_X86_64_PC64", RelocClass::Pc, 8),
            (Machine::X86_64, _) => ("R_X86_64_other", RelocClass::Other, 4),
            (_, 0) => ("R_386_NONE", RelocClass::None, 0),
            (_, 1) => ("R_386_32", RelocClass::Abs, 4),
            (_, 2) => ("R_386_PC32", RelocClass::Pc, 4),
            (_, _) => ("R_386_other", RelocClass::Other, 4),
        };
        Ok(RelocInfo { name, class, width })
    }

    fn addend(&self, r_type: u32, bytes: &[u8], offset: u64) -> i64 {
        let w = if self.0 == Machine::X86_64 && matches!(r_type, 1 | 24) { 8 } else { 4 };
        let at = offset as usize;
        let Some(f) = bytes.get(at..at + w) else { return 0 };
        let mut v = [0u8; 8];
        v[..w].copy_from_slice(f);
        if w == 4 {
            i64::from(i32::from_le_bytes(v[..4].try_into().unwrap()))
        } else {
            i64::from_le_bytes(v)
        }
    }
}

static A64: mortar_a64::A64 = mortar_a64::A64::M1;

/// The relocation classes of `machine`.
pub fn relocs(machine: Machine) -> Box<dyn Relocs> {
    match machine {
        Machine::AArch64 => Box::new(ArchRelocs(&A64)),
        m => Box::new(X86(m)),
    }
}

/// The ELF members of an archive, in order, as `(name, bytes)`.
pub fn members(path: &Path) -> Vec<(String, Vec<u8>)> {
    let bytes = std::fs::read(path).unwrap();
    let ar = mortar_elf::archive::Archive::parse(&bytes).unwrap();
    ar.members()
        .filter_map(|m| {
            let m = m.ok()?;
            let data = m.data()?;
            data.starts_with(b"\x7fELF")
                .then(|| (String::from_utf8_lossy(m.name()).into_owned(), data.to_vec()))
        })
        .collect()
}

/// The library directories of each target: (machine, crt and libgcc
/// directory, libc directory).
pub fn lib_dirs() -> Vec<(Machine, PathBuf, PathBuf)> {
    let gcc = |d: &str| -> Option<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(d).ok()?.flatten().map(|e| e.path()).collect();
        v.sort();
        v.into_iter().rev().find(|p| p.join("crtbegin.o").exists())
    };
    let mut out = Vec::new();
    for (m, g, c) in [
        (Machine::AArch64, "/usr/lib/gcc/aarch64-linux-gnu", "/usr/lib/aarch64-linux-gnu"),
        (Machine::X86_64, "/usr/lib/gcc-cross/x86_64-linux-gnu", "/usr/x86_64-linux-gnu/lib"),
        (Machine::I386, "/usr/lib/gcc-cross/i686-linux-gnu", "/usr/i686-linux-gnu/lib"),
    ] {
        if let Some(g) = gcc(g) {
            if Path::new(c).join("libc.a").exists() {
                out.push((m, g, PathBuf::from(c)));
            }
        }
    }
    out
}

/// Per file of `readelf --debug-dump=frames` on `path` (an object or an
/// archive): (CIEs, FDEs, terminators), in file order.
pub fn readelf_frames(readelf: &Path, path: &Path) -> Vec<(usize, usize, usize)> {
    let o = mortar_testutil::run(
        readelf,
        [std::ffi::OsStr::new("--debug-dump=frames"), path.as_os_str()],
    );
    let text = o.stdout_text();
    let mut out: Vec<(usize, usize, usize)> = Vec::new();
    let archive = text.contains("\nFile: ") || text.starts_with("File: ");
    if !archive {
        out.push((0, 0, 0));
    }
    for line in text.lines() {
        if line.starts_with("File: ") {
            out.push((0, 0, 0));
            continue;
        }
        let Some(last) = out.last_mut() else { continue };
        let mut words = line.split_whitespace();
        let (Some(_off), Some(second)) = (words.next(), words.next()) else { continue };
        if second == "ZERO" {
            last.2 += 1;
            continue;
        }
        match words.nth(1) {
            Some("CIE") => last.0 += 1,
            Some("FDE") => last.1 += 1,
            _ => {}
        }
    }
    out
}

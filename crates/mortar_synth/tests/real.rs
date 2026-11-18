//! `.eh_frame` on real objects (WP-025 deliverable 2): GCC C++ with
//! exceptions and COMDAT, the crt files, assembly with CFI before the
//! first instruction, and an `ld -r` output, parsed by `mortar_merge`'s
//! phase 60 (over a [`Link`] of bare objects), planned and written by
//! [`EhPlan`] over [`MergeEh`], then read back by `readelf
//! --debug-dump=frames` and `llvm-dwarfdump --eh-frame`.

mod common;

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use common::{collector, text};
use mortar_core::inputs::RelocForm;
use mortar_core::par::Pool;
use mortar_core::resolution::SectionKind;
use mortar_core::{FileId, OutSecId, SectionId, SymbolId};
use mortar_elf::consts::{SHF_GROUP, SHT_GROUP, SHT_REL, SHT_RELA, SHT_STRTAB, SHT_SYMTAB};
use mortar_elf::{Class, Object, Section, SectionIndex, SymbolIndex, SymbolSection};
use mortar_merge::What;
use mortar_merge::eh::{Code, EhFrames, Target};
use mortar_merge::phase::{ArchRelocs, Link, LinkSection};
use mortar_synth::eh::{EhPlan, MergeEh};
use mortar_synth::out::Window;
use mortar_testutil::{Scratch, run, tool};

fn sid(file: u32, index: u32) -> SectionId {
    SectionId(file * 10_000 + index)
}

/// A link of bare objects (file 0 is the internal file, with no object):
/// every section kept; globals resolved by name, first definition wins;
/// the members of a COMDAT group an earlier file already gave are dead
/// code (by section name: the test's groups have one member each).
struct Objs {
    /// The objects' bytes, kept for the whole test (`Link<'static>`).
    data: Vec<&'static [u8]>,
    /// Per global name, its definition.
    defs: BTreeMap<Vec<u8>, Target>,
    ids: BTreeMap<Vec<u8>, SymbolId>,
    dead: Vec<SectionId>,
}

impl Objs {
    fn new(data: Vec<Vec<u8>>) -> Self {
        let data = data.into_iter().map(|d| &*Box::leak(d.into_boxed_slice())).collect();
        let mut o = Self { data, defs: BTreeMap::new(), ids: BTreeMap::new(), dead: Vec::new() };
        let mut groups: BTreeMap<Vec<u8>, u32> = BTreeMap::new();
        for f in 1..=o.data.len() as u32 {
            let obj = Object::parse(o.data[f as usize - 1]).unwrap();
            for s in obj.sections() {
                if s.flags() & SHF_GROUP != 0 {
                    let name = s.name().unwrap().to_vec();
                    if *groups.entry(name).or_insert(f) != f {
                        o.dead.push(sid(f, s.index().0));
                    }
                }
            }
            let Some(t) = obj.symbols().unwrap() else { continue };
            for (i, sym) in t.globals() {
                let name = t.name(&sym).unwrap().to_vec();
                let n = o.ids.len() as u32;
                o.ids.entry(name.clone()).or_insert(SymbolId(n));
                if let Ok(SymbolSection::Section(s)) = t.section_of(i, &sym) {
                    o.defs
                        .entry(name)
                        .or_insert(Target::Section { section: sid(f, s.0), value: sym.st_value });
                }
            }
        }
        o
    }
}

impl Link<'static> for Objs {
    fn files(&self) -> u32 {
        self.data.len() as u32 + 1
    }
    fn object(&self, file: FileId) -> Option<Object<'static>> {
        let d = self.data.get((file.0 as usize).checked_sub(1)?)?;
        Some(Object::parse(d).unwrap())
    }
    fn section_id(&self, file: FileId, index: SectionIndex) -> Option<SectionId> {
        Some(sid(file.0, index.0))
    }
    fn section(&self, file: FileId, index: SectionIndex) -> Option<LinkSection> {
        let obj = Object::parse(self.data[file.0 as usize - 1]).unwrap();
        let s = obj.section(index).ok()?;
        if index.0 == 0
            || matches!(s.sh_type(), SHT_REL | SHT_RELA | SHT_SYMTAB | SHT_STRTAB | SHT_GROUP)
        {
            return None;
        }
        let kind = if s.name().unwrap() == b".eh_frame" {
            SectionKind::EhFrame
        } else {
            SectionKind::Regular
        };
        let reloc = obj
            .sections()
            .find(|r| r.sh_type() == SHT_RELA && r.header().sh_info == index.0)
            .map_or(SectionIndex(0), |r| r.index());
        Some(LinkSection { id: sid(file.0, index.0), kind, out: OutSecId(0), reloc })
    }
    fn global(&self, file: FileId, index: SymbolIndex, _first: u32) -> Result<Target, What> {
        let obj = Object::parse(self.data[file.0 as usize - 1]).unwrap();
        let t = obj.symbols().unwrap().unwrap();
        let sym = t.get(index).unwrap();
        let name = t.name(&sym).unwrap();
        Ok(self
            .defs
            .get(name)
            .copied()
            .unwrap_or(Target::Undefined { symbol: self.ids[name], weak: sym.st_info >> 4 == 2 }))
    }
    fn bytes(&self, _: FileId, s: &Section<'static>) -> Result<Cow<'static, [u8]>, What> {
        s.data().map(Cow::Borrowed).map_err(|e| What::Malformed(e.to_string()))
    }
    fn class(&self) -> Class {
        Class::Elf64
    }
    fn form(&self) -> RelocForm {
        RelocForm::Rela
    }
}

/// Input FDEs whose code is live.
fn live_fdes(frames: &EhFrames, dead: &[SectionId]) -> usize {
    let live = |c: &Code| matches!(c, Code::Section { section, .. } if !dead.contains(section));
    frames.records.iter().filter_map(|r| r.fde()).filter(|f| live(&f.code)).count()
}

/// A relocatable ELF64 AArch64 file holding one `.eh_frame`.
fn wrap(eh: &[u8]) -> Vec<u8> {
    let shstr = b"\0.eh_frame\0.shstrtab\0";
    let eh_off = 64u64;
    let str_off = eh_off + eh.len() as u64;
    let sh_off = (str_off + shstr.len() as u64).next_multiple_of(8);
    let mut f = Vec::new();
    f.extend_from_slice(b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0");
    f.extend_from_slice(&1u16.to_le_bytes()); // ET_REL
    f.extend_from_slice(&183u16.to_le_bytes()); // EM_AARCH64
    f.extend_from_slice(&1u32.to_le_bytes());
    f.extend_from_slice(&0u64.to_le_bytes());
    f.extend_from_slice(&0u64.to_le_bytes());
    f.extend_from_slice(&sh_off.to_le_bytes());
    f.extend_from_slice(&0u32.to_le_bytes());
    for v in [64u16, 0, 0, 64, 3, 2] {
        f.extend_from_slice(&v.to_le_bytes());
    }
    f.extend_from_slice(eh);
    f.extend_from_slice(shstr);
    f.resize(sh_off as usize, 0);
    let sh = |f: &mut Vec<u8>, name: u32, ty: u32, flags: u64, off: u64, size: u64, align: u64| {
        f.extend_from_slice(&name.to_le_bytes());
        f.extend_from_slice(&ty.to_le_bytes());
        for v in [flags, 0, off, size] {
            f.extend_from_slice(&v.to_le_bytes());
        }
        f.extend_from_slice(&0u32.to_le_bytes());
        f.extend_from_slice(&0u32.to_le_bytes());
        f.extend_from_slice(&align.to_le_bytes());
        f.extend_from_slice(&0u64.to_le_bytes());
    };
    sh(&mut f, 0, 0, 0, 0, 0, 0);
    sh(&mut f, 1, 1, 2, eh_off, eh.len() as u64, 8);
    sh(&mut f, 11, 3, 0, str_off, shstr.len() as u64, 1);
    f
}

const CXX: &str = r#"
#include <stdexcept>
inline int shared(int x) { if (x < 0) throw std::runtime_error("neg"); return x * 2; }
int ONE(int x) { try { return shared(x); } catch (const std::exception &) { return -1; } }
"#;

const CFI: &str = "\t.text\n\t.globl NAME\n\t.type NAME, %function\nNAME:\n\t.cfi_startproc\n\t.cfi_undefined x30\n\tnop\n\tret\n\t.cfi_endproc\n";

fn compile(dir: &Scratch, cxx: &Path, gcc: &Path) -> Vec<PathBuf> {
    let mut objs = Vec::new();
    for name in ["one", "two"] {
        let src = dir.write(&format!("{name}.cc"), CXX.replace("ONE", name));
        let obj = dir.join(format!("{name}.o"));
        let out = run(cxx, ["-O0", "-c", src.to_str().unwrap(), "-o", obj.to_str().unwrap()]);
        assert!(out.success(), "{}", out.stderr_text());
        objs.push(obj);
    }
    for name in ["g1", "g2"] {
        let src = dir.write(&format!("{name}.s"), CFI.replace("NAME", name));
        let obj = dir.join(format!("{name}.o"));
        let out = run(gcc, ["-c", src.to_str().unwrap(), "-o", obj.to_str().unwrap()]);
        assert!(out.success(), "{}", out.stderr_text());
        objs.push(obj);
    }
    objs
}

fn crt(gcc: &Path, name: &str) -> PathBuf {
    let out = run(gcc, [format!("-print-file-name={name}")]);
    PathBuf::from(out.stdout_text().trim())
}

/// `(CIEs, FDEs)` as readelf sees them; panics on a warning.
fn readelf_counts(readelf: &Path, file: &Path) -> (usize, usize) {
    let out = run(readelf, ["--debug-dump=frames", file.to_str().unwrap()]);
    assert!(out.success(), "{}", out.stderr_text());
    assert!(!out.stderr_text().contains("Warning"), "{}", out.stderr_text());
    let t = out.stdout_text();
    (t.matches(" CIE\n").count(), t.matches(" FDE cie=").count())
}

fn dwarfdump_counts(dd: &Path, file: &Path) -> (usize, usize) {
    let out = run(dd, ["--eh-frame", file.to_str().unwrap()]);
    assert!(out.success(), "{}", out.stderr_text());
    assert!(
        !out.stderr_text().contains("error") && !out.stderr_text().contains("warning"),
        "{}",
        out.stderr_text()
    );
    let t = out.stdout_text();
    (t.matches(" CIE\n").count(), t.matches(" FDE cie=").count())
}

fn check(
    label: &str,
    inputs: &[PathBuf],
    readelf: &Path,
    dd: &Path,
    dir: &Scratch,
) -> (usize, usize) {
    let files: Vec<Vec<u8>> = inputs.iter().map(|p| std::fs::read(p).unwrap()).collect();
    let link = Objs::new(files);
    let pool = Pool::new(4);
    let a64 = mortar_a64::A64::M1;
    let phase = mortar_merge::phase::run(&pool, &link, &ArchRelocs(&a64))
        .unwrap_or_else(|p| panic!("{label}: phase 60: {p:?}"));
    let src = MergeEh::new(&phase.eh, &link).unwrap();
    let diag = collector();
    let plan = EhPlan::new(&src, |s| !link.dead.contains(&s), &pool, &diag);
    let report = diag.finish();
    assert!(!report.failed(), "{label}: {}", text(&report));
    let plan = plan.unwrap();
    let live = live_fdes(&phase.eh, &link.dead);
    assert_eq!(plan.fde_count() as usize, live, "{label}: S-41");
    let mut out = vec![0; plan.size() as usize];
    plan.write(&src, &mut Window::new(0, &mut out)).unwrap();
    let file = dir.write(&format!("{label}.eh.o"), wrap(&out));
    let (cies, fdes) = readelf_counts(readelf, &file);
    assert_eq!(fdes, live, "{label}: readelf FDEs");
    assert_eq!((cies, fdes), dwarfdump_counts(dd, &file), "{label}: llvm-dwarfdump agrees");
    // S-43: no two output CIEs are equal with equal relocations, and
    // every input CIE shape used by a live FDE is present.
    let in_cies = phase.eh.records.iter().filter(|r| r.cie().is_some()).count();
    assert!(cies <= in_cies && cies >= 1, "{label}: {cies} CIEs from {in_cies}");
    (cies, fdes)
}

#[test]
fn eh_frame_of_real_objects_reads_back() {
    // AArch64 objects on every host: its gcc, g++ and GNU ld, plain or
    // cross.
    let a64 = mortar_testutil::target::Arch::Aarch64;
    let (Some(gcc), Some(cxx), Some(readelf), Some(dd), Some(ld)) = (
        a64.tool("gcc"),
        a64.tool("g++"),
        tool("readelf"),
        tool("llvm-dwarfdump"),
        a64.tool("ld.bfd"),
    ) else {
        return;
    };
    let dir = Scratch::new("synth-real");
    let objs = compile(&dir, &cxx, &gcc);
    // C++ with exceptions and COMDAT (`shared` in both).
    let (c, f) = check("cxx", &objs[..2], &readelf, &dd, &dir);
    eprintln!("cxx: {c} CIEs, {f} FDEs");
    let link = Objs::new(vec![std::fs::read(&objs[0]).unwrap(), std::fs::read(&objs[1]).unwrap()]);
    assert!(!link.dead.is_empty(), "the second copy of `shared` is a discarded COMDAT member");
    // Assembly with CFI before the first instruction, twice: one CIE
    // survives (review finding 4).
    let (c, f) = check("cfi", &objs[2..], &readelf, &dd, &dir);
    assert_eq!((c, f), (1, 2), "byte-identical CIEs merge");
    // The crt files around the C++ objects.
    let mut all: Vec<PathBuf> =
        ["crt1.o", "crti.o", "crtbeginT.o"].iter().map(|n| crt(&gcc, n)).collect();
    all.extend_from_slice(&objs);
    all.extend(["crtend.o", "crtn.o"].iter().map(|n| crt(&gcc, n)));
    let (c, f) = check("crt", &all, &readelf, &dd, &dir);
    eprintln!("crt: {c} CIEs, {f} FDEs");
    // An `ld -r` output of the C++ objects.
    let r = dir.join("r.o");
    let out = run(
        &ld,
        ["-r", "-o", r.to_str().unwrap(), objs[0].to_str().unwrap(), objs[1].to_str().unwrap()],
    );
    assert!(out.success(), "{}", out.stderr_text());
    let (c, f) = check("ld-r", std::slice::from_ref(&r), &readelf, &dd, &dir);
    eprintln!("ld -r: {c} CIEs, {f} FDEs");
}

// ---- WP-243: the per-file selection (`spec/incr.md` IN-34a) -----------------

/// A link of real objects for the per-file selection: file 0 is the
/// internal file; `.eh_frame` relocations are resolved by name over every
/// file (first definition wins), and a COMDAT group's members are dead in
/// every file but the first with its signature (resolve.md §6.1, by load
/// order). `only` makes a view of one file's objects with the same
/// resolution and liveness: what an update's front gives the join for a
/// changed file.
struct Corpus {
    data: Vec<&'static [u8]>,
    defs: std::collections::HashMap<Vec<u8>, Target>,
    ids: std::collections::HashMap<Vec<u8>, SymbolId>,
    dead: std::collections::HashSet<SectionId>,
    /// Per file (index `file - 1`): a section's `SHT_RELA` section.
    rela: Vec<std::collections::HashMap<u32, u32>>,
}

/// Section ids of a corpus: `file << 22 | index`.
fn csid(file: u32, index: u32) -> SectionId {
    assert!(file < 1 << 10 && index < 1 << 22, "corpus id space");
    SectionId(file << 22 | index)
}

impl Corpus {
    fn new(data: Vec<Vec<u8>>) -> Self {
        let data: Vec<&'static [u8]> =
            data.into_iter().map(|d| &*Box::leak(d.into_boxed_slice())).collect();
        let mut c = Self {
            data,
            defs: std::collections::HashMap::new(),
            ids: std::collections::HashMap::new(),
            dead: std::collections::HashSet::new(),
            rela: Vec::new(),
        };
        let mut groups: std::collections::HashSet<Vec<u8>> = std::collections::HashSet::new();
        for f in 1..=c.data.len() as u32 {
            let obj = Object::parse(c.data[f as usize - 1]).unwrap();
            let t = obj.symbols().unwrap();
            let mut rela = std::collections::HashMap::new();
            for s in obj.sections() {
                if s.sh_type() == SHT_RELA {
                    rela.insert(s.header().sh_info, s.index().0);
                }
                if s.sh_type() == SHT_GROUP {
                    let g = s.group().unwrap();
                    if !g.is_comdat() {
                        continue;
                    }
                    let t = t.as_ref().unwrap();
                    let sym = t.get(g.signature()).unwrap();
                    let name = t.name(&sym).unwrap().to_vec();
                    if !groups.insert(name) {
                        for m in g.members() {
                            c.dead.insert(csid(f, m.unwrap().0));
                        }
                    }
                }
            }
            c.rela.push(rela);
            let Some(t) = t else { continue };
            for (i, sym) in t.globals() {
                let name = t.name(&sym).unwrap().to_vec();
                let n = c.ids.len() as u32;
                c.ids.entry(name.clone()).or_insert(SymbolId(n));
                if let Ok(SymbolSection::Section(s)) = t.section_of(i, &sym) {
                    c.defs
                        .entry(name)
                        .or_insert(Target::Section { section: csid(f, s.0), value: sym.st_value });
                }
            }
        }
        c
    }

    fn live(&self, s: SectionId) -> bool {
        !self.dead.contains(&s)
    }
}

/// [`Corpus`] whole, or one file of it.
struct View<'a> {
    c: &'a Corpus,
    only: Option<u32>,
}

impl Link<'static> for View<'_> {
    fn files(&self) -> u32 {
        self.c.data.len() as u32 + 1
    }
    fn object(&self, file: FileId) -> Option<Object<'static>> {
        if self.only.is_some_and(|f| f != file.0) {
            return None;
        }
        let d = self.c.data.get((file.0 as usize).checked_sub(1)?)?;
        Some(Object::parse(d).unwrap())
    }
    fn section_id(&self, file: FileId, index: SectionIndex) -> Option<SectionId> {
        Some(csid(file.0, index.0))
    }
    fn section(&self, file: FileId, index: SectionIndex) -> Option<LinkSection> {
        let obj = Object::parse(self.c.data[file.0 as usize - 1]).unwrap();
        let s = obj.section(index).ok()?;
        if index.0 == 0
            || matches!(s.sh_type(), SHT_REL | SHT_RELA | SHT_SYMTAB | SHT_STRTAB | SHT_GROUP)
        {
            return None;
        }
        let kind = if s.name().unwrap() == b".eh_frame" {
            SectionKind::EhFrame
        } else {
            SectionKind::Regular
        };
        let reloc = self.c.rela[file.0 as usize - 1].get(&index.0).map_or(0, |&r| r);
        Some(LinkSection {
            id: csid(file.0, index.0),
            kind,
            out: OutSecId(0),
            reloc: SectionIndex(reloc),
        })
    }
    fn global(&self, file: FileId, index: SymbolIndex, _first: u32) -> Result<Target, What> {
        let obj = Object::parse(self.c.data[file.0 as usize - 1]).unwrap();
        let t = obj.symbols().unwrap().unwrap();
        let sym = t.get(index).unwrap();
        let name = t.name(&sym).unwrap();
        Ok(self
            .c
            .defs
            .get(name)
            .copied()
            .unwrap_or(Target::Undefined { symbol: self.c.ids[name], weak: sym.st_info >> 4 == 2 }))
    }
    fn bytes(&self, _: FileId, s: &Section<'static>) -> Result<Cow<'static, [u8]>, What> {
        s.data().map(Cow::Borrowed).map_err(|e| What::Malformed(e.to_string()))
    }
    fn class(&self) -> Class {
        Class::Elf64
    }
    fn form(&self) -> RelocForm {
        RelocForm::Rela
    }
}

/// The objects of `paths`: each an object, or an archive whose members
/// are taken in order.
fn objects_of(paths: &[PathBuf]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for p in paths {
        let bytes = std::fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        if bytes.starts_with(b"!<arch>\n") {
            let a = mortar_elf::archive::Archive::parse(&bytes).unwrap();
            for m in a.members() {
                let m = m.unwrap();
                if let Some(d) = m.data()
                    && d.starts_with(b"\x7fELF")
                {
                    out.push(d.to_vec());
                }
            }
        } else {
            out.push(bytes);
        }
    }
    out
}

/// IN-34a's per-file selection checked over `files`: for every file with
/// an `.eh_frame`, `EhPlan::select_in` with `per_file` over that file
/// alone (its own phase 60 parse, as an update's front gives it) equals
/// the full link's `select_in` with `per_file` restricted to the file.
/// Returns (files with an `.eh_frame`, records kept).
fn per_file_selection_equal(label: &str, files: Vec<Vec<u8>>) -> (usize, usize) {
    let machine = Object::parse(&files[0]).unwrap().machine();
    let a64 = mortar_a64::A64::M1;
    let x64 = mortar_x64::X64::M1;
    let arch: &dyn mortar_arch::Arch = match machine {
        mortar_elf::Machine::AArch64 => &a64,
        mortar_elf::Machine::X86_64 => &x64,
        mortar_elf::Machine::I386 => panic!("{label}: 64-bit targets only"),
    };
    let corpus = Corpus::new(files);
    let pool = Pool::new(2);
    let whole = View { c: &corpus, only: None };
    let phase = mortar_merge::phase::run(&pool, &whole, &ArchRelocs(arch))
        .unwrap_or_else(|p| panic!("{label}: phase 60: {p:?}"));
    let src = MergeEh::new(&phase.eh, &whole).unwrap();
    let diag = collector();
    let full = EhPlan::select_in(&src, |s| corpus.live(s), true, &pool, &diag).unwrap();
    let mut by_file: BTreeMap<u32, Vec<mortar_core::layout::EhRecord>> = BTreeMap::new();
    for r in &full {
        by_file.entry(r.section.0 >> 22).or_default().push(*r);
    }
    let with_eh: std::collections::BTreeSet<u32> =
        phase.eh.sections.iter().map(|s| s.section.0 >> 22).collect();
    for &f in &with_eh {
        let one = View { c: &corpus, only: Some(f) };
        let p1 = mortar_merge::phase::run(&pool, &one, &ArchRelocs(arch))
            .unwrap_or_else(|p| panic!("{label}: file {f}: phase 60: {p:?}"));
        let s1 = MergeEh::new(&p1.eh, &one).unwrap();
        let alone = EhPlan::select_in(&s1, |s| corpus.live(s), true, &pool, &diag).unwrap();
        let restricted = by_file.get(&f).map_or(&[][..], Vec::as_slice);
        assert_eq!(alone, restricted, "{label}: file {f}: per-file selection differs");
    }
    let report = diag.finish();
    assert!(!report.failed(), "{label}: {}", text(&report));
    (with_eh.len(), full.len())
}

// spec: incr.md IN-34a (S-44's de-duplication per file: the full link's
// selection restricted to a file is the file's own selection), WP-239 §6
// (the first test of the eh package). Always over the C++ objects and crt
// files of `compile` per 64-bit target with tools; over more objects and
// archives when `MORTAR_EH_SELECT_SETS` names them (`;` between sets, `,`
// between the paths of one set; protoc's in WP-243's record).
#[test]
fn select_in_per_file_equals_restricted_full_selection() {
    let mut sets: Vec<(String, Vec<PathBuf>)> = Vec::new();
    let mut dirs = Vec::new();
    for arch in [mortar_testutil::target::Arch::Aarch64, mortar_testutil::target::Arch::X86_64] {
        let (Some(gcc), Some(cxx)) = (arch.tool("gcc"), arch.tool("g++")) else { continue };
        let sub = Scratch::new(&format!("synth-select-{arch:?}"));
        let mut objs = Vec::new();
        for name in ["one", "two"] {
            let src = sub.write(&format!("{name}.cc"), CXX.replace("ONE", name));
            let obj = sub.join(format!("{name}.o"));
            let out = run(&cxx, ["-O0", "-c", src.to_str().unwrap(), "-o", obj.to_str().unwrap()]);
            assert!(out.success(), "{}", out.stderr_text());
            objs.push(obj);
        }
        let mut all: Vec<PathBuf> =
            ["crt1.o", "crti.o", "crtbeginT.o"].iter().map(|n| crt(&gcc, n)).collect();
        all.extend(objs);
        all.extend(["crtend.o", "crtn.o"].iter().map(|n| crt(&gcc, n)));
        if all.iter().all(|p| p.is_file()) {
            sets.push((format!("{arch:?} gcc"), all));
        }
        dirs.push(sub);
    }
    if let Ok(v) = std::env::var("MORTAR_EH_SELECT_SETS") {
        for (k, set) in v.split(';').filter(|s| !s.is_empty()).enumerate() {
            sets.push((format!("set {k}"), set.split(',').map(PathBuf::from).collect()));
        }
    }
    for (label, paths) in sets {
        let (files, records) = per_file_selection_equal(&label, objects_of(&paths));
        eprintln!("{label}: {files} files with .eh_frame, {records} records: equal");
    }
}

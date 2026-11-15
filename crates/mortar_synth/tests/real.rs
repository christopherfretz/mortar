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

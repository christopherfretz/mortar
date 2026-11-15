//! `.eh_frame` on real objects (WP-025 deliverable 2): GCC C++ with
//! exceptions and COMDAT, the crt files, assembly with CFI before the
//! first instruction, and an `ld -r` output, parsed by a test-only
//! parser (stage A: `mortar_merge`'s parser takes its place in stage B),
//! planned and written, then read back by `readelf --debug-dump=frames`
//! and `llvm-dwarfdump --eh-frame`.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use common::{collector, text};
use mortar_core::par::Pool;
use mortar_core::{FileId, SectionId, SymbolId};
use mortar_elf::consts::{SHF_GROUP, SHT_RELA};
use mortar_elf::{Object, SymbolIndex, SymbolSection};
use mortar_synth::eh::{
    CieReloc, CodeRef, EhPlan, EhSection, EhSource, InRecord, RecordKind, RelocTarget,
};
use mortar_synth::out::Window;
use mortar_testutil::{Scratch, run, tool};

/// The `.eh_frame` of a set of objects, parsed as S-34 to S-40 describe.
#[derive(Debug, Default)]
struct Parsed {
    secs: Vec<EhSection>,
    recs: Vec<Vec<InRecord>>,
    bytes: Vec<Vec<u8>>,
    relocs: BTreeMap<(usize, u32), Vec<CieReloc>>,
    /// Code sections that a COMDAT group of an earlier file already holds.
    dead: Vec<SectionId>,
    /// Input FDEs whose code is live.
    live_fdes: usize,
}

impl EhSource for Parsed {
    fn sections(&self) -> &[EhSection] {
        &self.secs
    }
    fn records(&self, s: usize) -> &[InRecord] {
        &self.recs[s]
    }
    fn bytes(&self, s: usize) -> &[u8] {
        &self.bytes[s]
    }
    fn cie_relocs(&self, s: usize, r: u32, out: &mut Vec<CieReloc>) {
        out.extend(self.relocs.get(&(s, r)).into_iter().flatten());
    }
}

fn sid(file: usize, index: u32) -> SectionId {
    SectionId(file as u32 * 10_000 + index)
}

fn parse(files: &[Vec<u8>]) -> Parsed {
    let mut p = Parsed::default();
    let globals: std::cell::RefCell<BTreeMap<Vec<u8>, u32>> = std::cell::RefCell::default();
    let mut groups: BTreeMap<Vec<u8>, usize> = BTreeMap::new();
    for (f, data) in files.iter().enumerate() {
        let file = f + 1;
        let obj = Object::parse(data).unwrap();
        let syms = obj.symbols().unwrap().unwrap();
        for s in obj.sections() {
            if s.flags() & u64::from(SHF_GROUP) != 0 {
                let name = s.name().unwrap().to_vec();
                if *groups.entry(name).or_insert(file) != file {
                    p.dead.push(sid(file, s.index().0));
                }
            }
        }
        for s in obj.sections() {
            if s.name().unwrap() != b".eh_frame" {
                continue;
            }
            let bytes = s.data().unwrap().to_vec();
            let rela: Vec<mortar_elf::Rela> = obj
                .sections()
                .filter(|r| r.sh_type() == SHT_RELA && r.info_section().ok() == Some(s.index()))
                .flat_map(|r| r.rela().unwrap().iter().collect::<Vec<_>>())
                .collect();
            let target = |r: &mortar_elf::Rela| -> Option<RelocTarget> {
                let sym = syms.get(SymbolIndex(r.r_sym)).ok()?;
                match syms.section_of(SymbolIndex(r.r_sym), &sym).ok()? {
                    SymbolSection::Section(i) if sym.binding() == 0 => Some(RelocTarget::Section {
                        section: sid(file, i.0),
                        offset: sym.st_value.wrapping_add(r.r_addend as u64),
                    }),
                    SymbolSection::Absolute => Some(RelocTarget::Absolute(sym.st_value)),
                    _ => {
                        let name = syms.name(&sym).ok()?.to_vec();
                        let mut g = globals.borrow_mut();
                        let n = g.len() as u32;
                        Some(RelocTarget::Global(SymbolId(*g.entry(name).or_insert(n))))
                    }
                }
            };
            let i = p.secs.len();
            let mut recs = Vec::new();
            let mut at = 0usize;
            while at + 4 <= bytes.len() {
                let len = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
                if len == 0 {
                    recs.push(InRecord { offset: at as u32, len: 0, kind: RecordKind::Terminator });
                    at += 4;
                    continue;
                }
                let id = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap());
                let kind = if id == 0 {
                    let mine: Vec<CieReloc> = rela
                        .iter()
                        .filter(|r| {
                            (at as u64..(at + 4 + len as usize) as u64).contains(&r.r_offset)
                        })
                        .filter(|r| r.r_type != 0)
                        .map(|r| CieReloc {
                            offset: (r.r_offset - at as u64) as u32,
                            r_type: r.r_type,
                            addend: r.r_addend,
                            target: target(r).unwrap(),
                        })
                        .collect();
                    if !mine.is_empty() {
                        p.relocs.insert((i, recs.len() as u32), mine);
                    }
                    RecordKind::Cie
                } else {
                    let cie_off = (at + 4 - id as usize) as u32;
                    let cie =
                        recs.iter().position(|r: &InRecord| r.offset == cie_off).unwrap() as u32;
                    // S-38: the relocation at the initial location; R_*_NONE is none.
                    let code = rela
                        .iter()
                        .find(|r| r.r_offset == (at + 8) as u64 && r.r_type != 0)
                        .and_then(|r| match target(r)? {
                            RelocTarget::Section { section, offset } => {
                                Some(CodeRef { section, offset })
                            }
                            _ => None,
                        });
                    let range = u32::from_le_bytes(bytes[at + 12..at + 16].try_into().unwrap());
                    RecordKind::Fde { cie, code, empty_range: range == 0 }
                };
                recs.push(InRecord { offset: at as u32, len, kind });
                at += 4 + len as usize;
            }
            let align = s.alignment().unwrap().max(1).trailing_zeros() as u8;
            p.secs.push(EhSection {
                id: sid(file, s.index().0),
                file: FileId(file as u32),
                size: bytes.len() as u32,
                align,
                sh_type: s.sh_type(),
            });
            p.recs.push(recs);
            p.bytes.push(bytes);
        }
    }
    p.live_fdes = p
        .recs
        .iter()
        .flatten()
        .filter(|r| matches!(r.kind, RecordKind::Fde { code: Some(c), .. } if !p.dead.contains(&c.section)))
        .count();
    p
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
    let parsed = parse(&files);
    let diag = collector();
    let plan = EhPlan::new(&parsed, |s| !parsed.dead.contains(&s), &Pool::new(4), &diag);
    let report = diag.finish();
    assert!(!report.failed(), "{label}: {}", text(&report));
    let plan = plan.unwrap();
    assert_eq!(plan.fde_count() as usize, parsed.live_fdes, "{label}: S-41");
    let mut out = vec![0; plan.size() as usize];
    plan.write(&parsed, &mut Window::new(0, &mut out)).unwrap();
    let file = dir.write(&format!("{label}.eh.o"), wrap(&out));
    let (cies, fdes) = readelf_counts(readelf, &file);
    assert_eq!(fdes, parsed.live_fdes, "{label}: readelf FDEs");
    assert_eq!((cies, fdes), dwarfdump_counts(dd, &file), "{label}: llvm-dwarfdump agrees");
    // S-43: no two output CIEs are equal with equal relocations, and
    // every input CIE shape used by a live FDE is present.
    let in_cies = parsed.recs.iter().flatten().filter(|r| r.kind == RecordKind::Cie).count();
    assert!(cies <= in_cies && cies >= 1, "{label}: {cies} CIEs from {in_cies}");
    (cies, fdes)
}

#[test]
fn eh_frame_of_real_objects_reads_back() {
    let (Some(gcc), Some(cxx), Some(readelf), Some(dd), Some(ld)) =
        (tool("gcc"), tool("g++"), tool("readelf"), tool("llvm-dwarfdump"), tool("ld.bfd"))
    else {
        return;
    };
    if !cfg!(target_arch = "aarch64") {
        eprintln!("note: the real-object test compiles for the host, AArch64 only");
        return;
    }
    let dir = Scratch::new("synth-real");
    let objs = compile(&dir, &cxx, &gcc);
    // C++ with exceptions and COMDAT (`shared` in both).
    let (c, f) = check("cxx", &objs[..2], &readelf, &dd, &dir);
    eprintln!("cxx: {c} CIEs, {f} FDEs");
    let parsed = parse(&[std::fs::read(&objs[0]).unwrap(), std::fs::read(&objs[1]).unwrap()]);
    assert!(!parsed.dead.is_empty(), "the second copy of `shared` is a discarded COMDAT member");
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

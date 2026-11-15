//! The census (brief scope 4; x64.md §17): every record of every object
//! of the review's corpus classified and scanned by `mortar_x64`, in a
//! static and a static-pie link, with no unnamed refusal.
//!
//! The objects are the review's `corpus.sh` matrix over `t1.c`, `t2.cc`
//! and `t3.f90` (compiler × `-fno-pic`/`-fPIE`/`-fPIC` × optimization ×
//! 12 code-model / PLT / TLS-dialect / CET variants). By default the
//! matrix is cut to `-O2` and the default `clang` (72 configurations);
//! `MORTAR_X64_CENSUS=full` builds the whole of it (`-O0`…`-Os`, GCC,
//! clang 19 and 22: the review's 1261 objects). Kiln objects (`kiln++`
//! with hod at `-O0`, LLVM at `-O2`) join when a kiln build is found
//! (`MORTAR_KILN`, `MORTAR_HOD`, or the workspace's `.build/target-*`).
//!
//! Each symbol of an object stands for its link-time resolution in a
//! static link: defined and undefined non-weak symbols are `Local` (a
//! static link defines them, or refuses an undefined one before scan),
//! undefined weak ones `weak0`, `SHN_ABS` ones `Abs`. The outcome of
//! every record is tallied by type: accepted, or the named refusal or
//! incompatibility; an unknown type (a hex name), a field outside its
//! section, overlapping records or an `Internal` error fails the test.

#![allow(clippy::too_many_lines)]

mod elf;
mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use elf::{Image, SHF_ALLOC, SHF_TLS, SHN_ABS, SHT_RELA, STB_WEAK, STT_TLS, u64_at};
use mortar_arch::{
    Arch, DemandSink, OutputKind, PlaceFacts, RefName, Reloc, RelocAt, ScanCx, ScanError, Site,
    TargetKind,
};
use mortar_core::{FileId, SectionId};
use mortar_testutil::{Scratch, run, tool};
use mortar_x64::X64;
use support::{Sink, Tables, all_code, key};

const EXTRAS: [&str; 12] = [
    "",
    "-mcmodel=medium",
    "-mcmodel=large",
    "-fno-plt",
    "-mtls-dialect=gnu2",
    "-fno-plt -Wa,-mrelax-relocations=no",
    "-mcmodel=large -mtls-dialect=gnu2",
    "-fcf-protection=full -mno-red-zone",
    "-ftls-model=local-dynamic",
    "-ftls-model=initial-exec",
    "-mcmodel=medium -fno-plt -mtls-dialect=gnu2",
    "-Wa,-mrelax-relocations=no",
];
const PICS: [&str; 3] = ["-fno-pic", "-fPIE", "-fPIC"];

struct Job {
    exe: PathBuf,
    args: Vec<String>,
    obj: PathBuf,
}

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name)
}

/// The workspace's kiln and hod builds, if any.
fn kiln() -> Option<(PathBuf, PathBuf)> {
    let env = |v: &str| std::env::var_os(v).map(PathBuf::from).filter(|p| p.is_file());
    let found = |rel: &str| {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .map(|a| a.join(".build").join(rel))
            .find(|p| p.is_file())
    };
    let k = env("MORTAR_KILN").or_else(|| found("target-kiln/release/kiln++"))?;
    let h = env("MORTAR_HOD").or_else(|| found("target-hod/release/hod"))?;
    Some((k, h))
}

fn jobs(dir: &Scratch, full: bool) -> Vec<Job> {
    let gcc = tool("x86_64-linux-gnu-gcc");
    let gxx = tool("x86_64-linux-gnu-g++");
    let gfortran = tool("x86_64-linux-gnu-gfortran");
    let (Some(gcc), Some(gxx), Some(gfortran)) = (gcc, gxx, gfortran) else { return Vec::new() };
    let mut clangs = Vec::new();
    if full {
        clangs.extend(["clang-19", "clang-22"].into_iter().filter_map(tool));
    } else {
        clangs.extend(tool("clang"));
    }
    let opts: &[&str] = if full { &["-O0", "-O1", "-O2", "-O3", "-Os"] } else { &["-O2"] };
    let mut out = Vec::new();
    let mut n = 0;
    let mut add = |exe: &Path, pre: &[&str], flags: &[&str], src: &str, tag: &str| {
        n += 1;
        let obj = dir.join(format!("{tag}-{n}.o"));
        let mut args: Vec<String> = pre.iter().map(|s| (*s).to_string()).collect();
        args.extend(flags.iter().filter(|f| !f.is_empty()).map(|s| (*s).to_string()));
        args.extend([
            "-c".into(),
            data(src).display().to_string(),
            "-o".into(),
            obj.display().to_string(),
        ]);
        out.push(Job { exe: exe.to_path_buf(), args, obj });
    };
    let jdir = format!("-J{}", dir.path().display());
    for cc in std::iter::once(None).chain(clangs.iter().map(Some)) {
        for pic in PICS {
            for opt in opts {
                for extra in EXTRAS {
                    let mut flags = vec![pic, *opt];
                    flags.extend(extra.split(' '));
                    match cc {
                        None => {
                            add(&gcc, &[], &flags, "t1.c", "gcc-t1");
                            add(&gxx, &[], &flags, "t2.cc", "gcc-t2");
                            add(&gfortran, &["-fopenmp", &jdir], &flags, "t3.f90", "gcc-t3");
                        }
                        Some(c) => {
                            let t = ["--target=x86_64-linux-gnu"];
                            add(c, &t, &flags, "t1.c", "clang-t1");
                            add(c, &t, &flags, "t2.cc", "clang-t2");
                        }
                    }
                }
            }
        }
    }
    if let Some((k, h)) = kiln() {
        let hod = format!("--hod={}", h.display());
        for pic in PICS {
            for src in ["t2.cc", "q.cc"] {
                let t = ["--target=x86_64-linux-gnu"];
                add(&k, &t, &[pic, "-O0", "-fkiln-backend=hod", &hod], src, "kiln-hod");
                add(&k, &t, &[pic, "-O2"], src, "kiln-llvm");
            }
        }
    }
    out
}

#[derive(Default)]
struct Census {
    objects: u32,
    records: u64,
    /// `(type, output, outcome) → count`.
    outcomes: BTreeMap<(u32, &'static str, String), u64>,
    unnamed: Vec<String>,
}

fn census_object(obj: &Image, what: &str, c: &mut Census) {
    let syms = obj.symbols();
    let mut tables = Tables::default();
    for (i, s) in syms.iter().enumerate() {
        let k = key(u32::try_from(i).unwrap());
        let in_tls = obj.sections.get(usize::from(s.shndx)).is_some_and(|x| x.flags & SHF_TLS != 0);
        let kind = match s.shndx {
            0 if s.info >> 4 == STB_WEAK => TargetKind::Weak0,
            SHN_ABS => TargetKind::Abs,
            _ => TargetKind::Local,
        };
        let v = if kind == TargetKind::Weak0 { 0 } else { 0x40_0000 + s.value };
        if s.info & 0xF == STT_TLS || in_tls {
            tables.tls_sym(k, if kind == TargetKind::Weak0 { 0 } else { s.value }, kind);
        } else {
            tables.sym(k, v, kind);
        }
        let f = tables.facts.get_mut(&k).unwrap();
        f.sym_type = s.info & 0xF;
        tables.names.insert(k, s.name.as_bytes().to_vec());
    }
    c.objects += 1;
    for rs in obj.sections.iter().filter(|s| s.kind == SHT_RELA && s.info != 0) {
        let sec = &obj.sections[rs.info as usize];
        let relocs: Vec<Reloc> = obj
            .data(rs)
            .chunks_exact(24)
            .map(|e| {
                let info = u64_at(e, 8);
                Reloc {
                    r_offset: u64_at(e, 0),
                    key: key((info >> 32) as u32),
                    r_type: (info & 0xFFFF_FFFF) as u32,
                    addend: u64_at(e, 16) as i64,
                }
            })
            .collect();
        let place = PlaceFacts { alloc: sec.flags & SHF_ALLOC != 0, writable: sec.flags & 1 != 0 };
        let input = obj.data(sec);
        for (output, oname) in [(OutputKind::Static, "static"), (OutputKind::Pie, "static-pie")] {
            if output == OutputKind::Static {
                c.records += relocs.len() as u64;
            }
            let proof = all_code(input.len());
            let cx = ScanCx { tables: &tables, output, place, name: sec.name.as_bytes() };
            let mut sink = Sink::default();
            let mut refused: Vec<Option<String>> = vec![None; relocs.len()];
            for (i, r) in relocs.iter().enumerate() {
                let site = Site { bytes: input, offset: r.r_offset };
                let h = match X64::M1.classify(r.r_type, site) {
                    Ok(h) => h,
                    Err(refusal) => {
                        if matches!(refusal.name, RefName::Hex(_)) {
                            c.unnamed.push(format!("{what} {}: {} unknown", sec.name, r.r_type));
                        }
                        refused[i] = Some(format!("refused: {}", refusal.name));
                        continue;
                    }
                };
                let at = RelocAt {
                    file: FileId(1),
                    section: SectionId(rs.info),
                    index: u32::try_from(i).unwrap(),
                    r_offset: r.r_offset,
                    key: r.key,
                    r_type: r.r_type,
                    addend: Some(r.addend),
                    howto: &h,
                    site,
                    records: &relocs.as_slice(),
                    proof: &proof,
                    leads: true,
                };
                X64::M1.scan(&cx, &at, &mut DemandSink::new(&mut sink));
            }
            for (i, r) in relocs.iter().enumerate() {
                let outcome = refused[i].take().unwrap_or_else(|| {
                    match sink.errors.iter().find(|(k, _)| *k as usize == i) {
                        None => "accepted".to_string(),
                        Some((_, ScanError::Refused(r))) => format!("refused: {}", r.name),
                        Some((_, ScanError::Incompatible(x))) => format!("incompatible: {x:?}"),
                        Some((_, e)) => {
                            c.unnamed.push(format!(
                                "{what} {}+{:#x}: {} {e:?}",
                                sec.name, r.r_offset, r.r_type
                            ));
                            format!("{e:?}")
                        }
                    }
                });
                *c.outcomes.entry((r.r_type, oname, outcome)).or_default() += 1;
            }
        }
    }
}

#[test]
fn census_every_record_scans_or_is_refused_by_name() {
    let full = std::env::var_os("MORTAR_X64_CENSUS").is_some_and(|v| v == "full");
    let dir = Scratch::new("x64-census");
    let jobs = jobs(&dir, full);
    if jobs.is_empty() {
        return;
    }
    // A few compilers at a time (each `run` holds a jobserver token).
    let next = Mutex::new(0usize);
    let built = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..4 {
            s.spawn(|| {
                loop {
                    let i = {
                        let mut n = next.lock().unwrap();
                        *n += 1;
                        *n - 1
                    };
                    let Some(j) = jobs.get(i) else { break };
                    let o = run(&j.exe, &j.args);
                    // A configuration a compiler rejects (clang has no
                    // `-mrelax-relocations` for its integrated assembler
                    // in some versions) is skipped, as corpus.sh does.
                    if o.success() {
                        built.lock().unwrap().push(j.obj.clone());
                    }
                }
            });
        }
    });
    let mut built = built.into_inner().unwrap();
    built.sort();
    let mut c = Census::default();
    for o in &built {
        let img = Image::read(o);
        census_object(&img, &o.file_name().unwrap().to_string_lossy(), &mut c);
    }
    eprintln!(
        "census: {} objects of {} configurations, {} records",
        c.objects,
        jobs.len(),
        c.records
    );
    let mut by_type: BTreeMap<u32, Vec<(&str, &str, u64)>> = BTreeMap::new();
    for ((t, o, what), n) in &c.outcomes {
        by_type.entry(*t).or_default().push((o, what, *n));
    }
    for (t, rows) in &by_type {
        let name = mortar_elf::reloc::x86_64::name(*t).unwrap_or("?");
        for (o, what, n) in rows {
            eprintln!("  {t:<3} {name:<26} {o:<10} {what:<40} {n}");
        }
    }
    assert!(c.unnamed.is_empty(), "unnamed refusals:\n{}", c.unnamed.join("\n"));
    assert!(c.objects >= 150 || full, "{} objects", c.objects);
}

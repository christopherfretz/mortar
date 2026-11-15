//! The census (x86.md §2, §2.2 "Form match over the review corpora"):
//! every record of every object of three sets classified and scanned by
//! `mortar_x86`, in a static and a static-pie link, with no unnamed
//! refusal and no unmatched GOT or TLS record.
//!
//! The sets are x86.md's:
//! - **review**: the reviewer's `corpus.sh` over `t1.c`, `t2.cc` and
//!   `t3.c` (GCC 14, Clang 19 and 22 × `-fno-pic`/`-fPIE`/`-fPIC`/`-fpic`
//!   × `-O0`…`-Os` × 19 option sets, plus its GCC and Clang 22 tails:
//!   2 744 configurations, 2 736 objects);
//! - **census**: x86.md's own `corpus.sh` (E1) over `c1.c`, `c2.cc`,
//!   `c3.c` and `f1.f90` (compiler × 3 PIC modes × 5 optimizations × 19
//!   option sets; Fortran from GCC only: 2 850 objects);
//! - **system**: every `.o`, and every ELF32 member of every `.a`, under
//!   `/usr/i686-linux-gnu/lib` and `/usr/lib/gcc-cross/i686-linux-gnu/14`
//!   (4 811 objects).
//!
//! The sources are copies in `tests/data/census/`. By default both
//! matrices are cut to `-O0` and `-O2` and to GCC and Clang 22 (the
//! gate, about 860 objects);
//! `MORTAR_X86_CENSUS=full` builds both whole. The system set is scanned
//! in both modes; a missing directory or compiler skips its part (fails
//! under `MORTAR_REQUIRE_TOOLS=1`).
//!
//! Each symbol stands for its link-time resolution in a static link:
//! defined and undefined non-weak symbols are `Local`, undefined weak ones
//! `Weak0`, `SHN_ABS` ones `Abs`, defined `STT_GNU_IFUNC` ones `Ifunc`;
//! `STT_TLS` symbols (and symbols of `SHF_TLS` sections) are TLS. A
//! section symbol resolves to its section. Records are REL: the implicit
//! addend is read by `mortar_x86` from the section bytes passed as the
//! scan input.
//!
//! The outcome of every record is tallied by output and type: accepted,
//! or the named refusal or incompatibility. The test fails on an unnamed
//! refusal (a hex name), a field outside its section, overlapping
//! records, an `Internal` error, and an unmatched GOT or TLS record (a GD,
//! LD or descriptor record refused as an unrecognized sequence, or a
//! `GOT32`/`GOT32X` whose instruction does not match, X86-13).

#![allow(clippy::too_many_lines)]

mod elf;
mod support;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use elf::{
    Image, SHF_ALLOC, SHF_EXECINSTR, SHF_TLS, SHF_WRITE, SHN_ABS, SHN_UNDEF, SHT_REL, SHT_RELA,
    STB_WEAK, STT_GNU_IFUNC, STT_SECTION, STT_TLS, Shdr, is_elf32, u16_at,
};
use mortar_arch::{
    Arch, DemandSink, Incompatible, OutputKind, PlaceFacts, RefName, Reloc, RelocAt, ScanCx,
    ScanError, Site, TargetKind,
};
use mortar_core::{FileId, SectionId};
use mortar_testutil::{Scratch, run, tool, tools_required};
use mortar_x86::forms::{GOT32, GOT32X, TLS_DESC_CALL, TLS_GD, TLS_GOTDESC, TLS_LDM};
use mortar_x86::{UNRECOGNIZED_DESC, UNRECOGNIZED_GD, UNRECOGNIZED_LD, X86};
use support::{Sink, Tables, all_code, key};

/// The reviewer's option sets (`.build/review-x86-adv/corpus.sh`).
const REVIEW_EXTRAS: [&str; 19] = [
    "",
    "-fno-plt",
    "-mtls-dialect=gnu2",
    "-mtls-dialect=gnu2 -fno-plt",
    "-ftls-model=global-dynamic",
    "-ftls-model=local-dynamic",
    "-ftls-model=initial-exec",
    "-ftls-model=local-exec",
    "-ftls-model=global-dynamic -fno-plt",
    "-ftls-model=local-dynamic -fno-plt",
    "-mregparm=3",
    "-fomit-frame-pointer -march=i386",
    "-march=pentium4 -mstackrealign",
    "-fcf-protection",
    "-fstack-protector-all",
    "-pg",
    "-Wa,-mrelax-relocations=no",
    "-g -gsplit-dwarf",
    "-ftls-model=local-dynamic -mtls-dialect=gnu2",
];

/// x86.md's census option sets (E1 `corpus.sh`).
const CENSUS_EXTRAS: [&str; 19] = [
    "",
    "-fno-plt",
    "-mtls-dialect=gnu2",
    "-ftls-model=global-dynamic",
    "-ftls-model=local-dynamic",
    "-ftls-model=initial-exec",
    "-ftls-model=local-exec",
    "-fcf-protection=full",
    "-march=pentium4 -msse2 -mfpmath=sse",
    "-march=i686",
    "-fomit-frame-pointer -mregparm=3",
    "-mstackrealign",
    "-g -gsplit-dwarf",
    "-g -gdwarf-4",
    "-g -gdwarf-5 -ffunction-sections -fdata-sections",
    "-Wa,-mrelax-relocations=no",
    "-fno-plt -Wa,-mrelax-relocations=no",
    "-fno-plt -mtls-dialect=gnu2 -ftls-model=local-dynamic",
    "-fcommon",
];

const OPTS: [&str; 5] = ["-O0", "-O1", "-O2", "-O3", "-Os"];
const COMPILERS: [&str; 3] = ["gcc", "clang-19", "clang-22"];
const SYSTEM_DIRS: [&str; 2] = ["/usr/i686-linux-gnu/lib", "/usr/lib/gcc-cross/i686-linux-gnu/14"];

/// One configuration of a corpus script: compiler label, flags, source.
struct Entry {
    cc: &'static str,
    flags: String,
    src: &'static str,
}

struct Job {
    set: usize,
    what: String,
    exe: PathBuf,
    args: Vec<String>,
    obj: PathBuf,
}

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/census").join(name)
}

fn entry(cc: &'static str, parts: &[&str], src: &'static str) -> Entry {
    let flags = parts.iter().filter(|p| !p.is_empty()).copied().collect::<Vec<_>>().join(" ");
    Entry { cc, flags, src }
}

/// The reviewer's `gen` (`review-x86-adv/corpus.sh`), in its order.
fn review_entries() -> Vec<Entry> {
    let mut out = Vec::new();
    for cc in COMPILERS {
        for pic in ["-fno-pic", "-fPIE", "-fPIC", "-fpic"] {
            for opt in OPTS {
                for ex in REVIEW_EXTRAS {
                    for s in ["t1.c", "t2.cc", "t3.c"] {
                        if s == "t2.cc" && opt != "-O2" && opt != "-O0" {
                            continue;
                        }
                        if s == "t2.cc" && ex == "-mregparm=3" {
                            continue;
                        }
                        out.push(entry(cc, &[pic, opt, ex], s));
                    }
                }
            }
        }
    }
    for pic in ["-fno-pic", "-fPIC"] {
        for opt in ["-O0", "-O2"] {
            for ex in ["-fsplit-stack", "-pg -mfentry", "-mno-tls-direct-seg-refs"] {
                for s in ["t1.c", "t3.c"] {
                    out.push(entry("gcc", &[pic, opt, ex], s));
                }
            }
        }
    }
    for pic in ["-fno-pic", "-fPIC"] {
        for s in ["t1.c", "t3.c"] {
            out.push(entry("clang-22", &[pic, "-Oz"], s));
            out.push(entry(
                "clang-22",
                &[pic, "-O2 -fno-plt -ftls-model=global-dynamic -mrelax-relocations=no"],
                s,
            ));
        }
    }
    out
}

/// x86.md's E1 `corpus.sh`, in its order (Fortran from GCC only).
fn census_entries() -> Vec<Entry> {
    let mut out = Vec::new();
    for cc in COMPILERS {
        for pic in ["-fno-pic", "-fPIE", "-fPIC"] {
            for opt in OPTS {
                for ex in CENSUS_EXTRAS {
                    for s in ["c1.c", "c3.c", "c2.cc"] {
                        out.push(entry(cc, &[pic, opt, ex], s));
                    }
                    if cc == "gcc" {
                        out.push(entry(cc, &[pic, opt, ex], "f1.f90"));
                    }
                }
            }
        }
    }
    out
}

/// The default cut: `-O0` and `-O2` (the TLS and GOT sequences differ
/// between them), GCC and Clang 22.
fn in_cut(e: &Entry) -> bool {
    e.cc != "clang-19" && e.flags.split(' ').any(|f| f == "-O0" || f == "-O2")
}

/// The compilers, found once (a missing one is a skip of its jobs).
struct Tools {
    found: BTreeMap<&'static str, PathBuf>,
}

impl Tools {
    fn find() -> Self {
        let mut found = BTreeMap::new();
        for name in [
            "i686-linux-gnu-gcc",
            "i686-linux-gnu-g++",
            "x86_64-linux-gnu-gfortran",
            "clang-19",
            "clang-22",
            "clang++-19",
            "clang++-22",
        ] {
            if let Some(p) = tool(name) {
                found.insert(name, p);
            }
        }
        Self { found }
    }

    fn get(&self, name: &str) -> Option<PathBuf> {
        self.found.get(name).cloned()
    }
}

/// The jobs of one set. `cxx_driver` is how the script compiles `.cc`
/// with Clang: `clang++-N` (review) or `clang-N -x c++` (census).
fn jobs_of(
    set: usize,
    entries: &[Entry],
    full: bool,
    tools: &Tools,
    dir: &Scratch,
    jobs: &mut Vec<Job>,
) {
    for (n, e) in entries.iter().enumerate().filter(|(_, e)| full || in_cut(e)) {
        let cc_src = Path::new(e.src).extension().is_some_and(|x| x == "cc");
        let mut pre: Vec<String> = Vec::new();
        let mut post: Vec<String> = Vec::new();
        let exe = match (e.cc, e.src) {
            ("gcc", "f1.f90") => {
                pre.push("-m32".into());
                let m = dir.join(format!("mod{set}-{n}"));
                std::fs::create_dir_all(&m).unwrap();
                post.extend(["-fopenmp".to_string(), "-J".into(), m.display().to_string()]);
                tools.get("x86_64-linux-gnu-gfortran")
            }
            ("gcc", _) if cc_src => tools.get("i686-linux-gnu-g++"),
            ("gcc", _) => tools.get("i686-linux-gnu-gcc"),
            (clang, _) => {
                pre.push("--target=i686-linux-gnu".into());
                if cc_src && set == 0 {
                    tools.get(&clang.replace("clang", "clang++"))
                } else {
                    if cc_src {
                        pre.extend(["-x".to_string(), "c++".into()]);
                    }
                    tools.get(clang)
                }
            }
        };
        let Some(exe) = exe else { continue };
        let obj = dir.join(format!("s{set}-{n}.o"));
        let mut args = pre;
        args.extend(e.flags.split(' ').filter(|f| !f.is_empty()).map(str::to_string));
        args.extend(post);
        args.extend([
            "-c".into(),
            data(e.src).display().to_string(),
            "-o".into(),
            obj.display().to_string(),
        ]);
        let what = format!("{} {} {}", e.cc, e.flags, e.src);
        jobs.push(Job { set, what, exe, args, obj });
    }
}

/// The `.o` and `.a` files under `dir`, recursively, sorted.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x == "o" || x == "a") {
            out.push(p);
        }
    }
}

/// The members of an `ar` archive with their (long) names; the symbol
/// table and the long-name table are skipped.
fn members(bytes: &[u8]) -> Vec<(String, &[u8])> {
    let mut out = Vec::new();
    if !bytes.starts_with(b"!<arch>\n") {
        return out;
    }
    let mut longnames: &[u8] = &[];
    let mut o = 8;
    while o + 60 <= bytes.len() {
        let h = &bytes[o..o + 60];
        let name = String::from_utf8_lossy(&h[..16]).trim_end().to_string();
        let size: usize = String::from_utf8_lossy(&h[48..58]).trim().parse().unwrap();
        let body = &bytes[o + 60..o + 60 + size];
        o += 60 + size + (size & 1);
        if name == "//" {
            longnames = body;
            continue;
        }
        if name == "/" || name == "/SYM64/" {
            continue;
        }
        let name = match name.strip_prefix('/').and_then(|n| n.parse::<usize>().ok()) {
            Some(i) => {
                let s = &longnames[i..];
                let end = s.iter().position(|&b| b == b'\n').unwrap_or(s.len());
                String::from_utf8_lossy(&s[..end]).into_owned()
            }
            None => name,
        };
        out.push((name.trim_end_matches('/').to_string(), body));
    }
    out
}

/// An i386 relocatable object (`ELFCLASS32`, `EM_386`, `ET_REL`).
fn is_i386_rel(bytes: &[u8]) -> bool {
    is_elf32(bytes) && bytes.len() >= 52 && u16_at(bytes, 18) == 3 && u16_at(bytes, 16) == 1
}

#[derive(Default)]
struct Census {
    objects: u64,
    records: u64,
    rela_sections: u64,
    /// `type → records`.
    by_type: BTreeMap<u32, u64>,
    /// `(output, type, outcome) → count`.
    outcomes: BTreeMap<(&'static str, u32, String), u64>,
    /// Unnamed refusals, `Outside`, `Overlap`, `Internal`.
    unnamed: Vec<String>,
    /// Unmatched GOT and TLS records (X86-13, X86-31…).
    unmatched: Vec<String>,
}

/// The 16 bytes around `off` in `bytes`, as hex.
fn around(bytes: &[u8], off: u64) -> String {
    let o = usize::try_from(off).unwrap_or(usize::MAX).min(bytes.len());
    let lo = o.saturating_sub(8);
    let hi = (o + 8).min(bytes.len());
    let mut s = format!("[{lo:#x}..{hi:#x}]");
    for (i, b) in bytes[lo..hi].iter().enumerate() {
        let mark = if lo + i == o { "|" } else { " " };
        let _ = write!(s, "{mark}{b:02x}");
    }
    s
}

fn is_unmatched(r_type: u32, e: &ScanError) -> bool {
    match e {
        ScanError::Refused(r) => {
            matches!(r_type, TLS_GD | TLS_LDM | TLS_GOTDESC | TLS_DESC_CALL)
                && matches!(r.name, RefName::Named(n)
                    if n == UNRECOGNIZED_GD || n == UNRECOGNIZED_LD || n == UNRECOGNIZED_DESC)
        }
        ScanError::Incompatible(Incompatible::InstructionMismatch) => {
            matches!(r_type, GOT32 | GOT32X)
        }
        _ => false,
    }
}

fn census_object(obj: &Image, what: &str, c: &mut Census) {
    let syms = obj.symbols();
    let mut tables = Tables::default();
    let tls_sec = |shndx: u16| {
        obj.sections.get(usize::from(shndx)).is_some_and(|x: &Shdr| x.flags & SHF_TLS != 0)
    };
    for (i, s) in syms.iter().enumerate() {
        let k = key(u32::try_from(i).unwrap());
        let ty = s.info & 0xF;
        let weak = s.info >> 4 == STB_WEAK;
        let kind = match s.shndx {
            SHN_UNDEF if weak => TargetKind::Weak0,
            SHN_ABS => TargetKind::Abs,
            SHN_UNDEF => TargetKind::Local,
            _ if ty == STT_GNU_IFUNC => TargetKind::Ifunc,
            _ => TargetKind::Local,
        };
        // A section symbol is its section: each section has its own base.
        let base = 0x40_0000 + 0x10_0000 * u64::from(s.shndx);
        let v = match kind {
            TargetKind::Weak0 => 0,
            TargetKind::Abs => s.value,
            _ if ty == STT_SECTION => base,
            _ => base + s.value,
        };
        if ty == STT_TLS || tls_sec(s.shndx) {
            let tv = if kind == TargetKind::Weak0 { 0 } else { s.value };
            tables.tls_sym(k, tv, kind);
        } else {
            tables.sym(k, v, kind);
        }
        tables.facts.get_mut(&k).unwrap().sym_type = ty;
        let name = if ty == STT_SECTION {
            obj.sections.get(usize::from(s.shndx)).map_or(String::new(), |x| x.name.clone())
        } else {
            s.name.clone()
        };
        tables.names.insert(k, name.into_bytes());
    }
    c.objects += 1;
    c.rela_sections +=
        obj.sections.iter().filter(|s| s.kind == SHT_RELA && s.size != 0).count() as u64;
    for rs in obj.sections.iter().filter(|s| s.kind == SHT_REL && s.info != 0) {
        let sec = &obj.sections[rs.info as usize];
        let relocs: Vec<Reloc> = obj
            .rels(rs)
            .into_iter()
            .map(|r| Reloc { r_offset: r.offset, key: key(r.sym), r_type: r.r_type, addend: 0 })
            .collect();
        let place = PlaceFacts {
            alloc: sec.flags & SHF_ALLOC != 0,
            writable: sec.flags & SHF_WRITE != 0,
            exec: Some(sec.flags & SHF_EXECINSTR != 0),
        };
        let input = obj.data(sec);
        let proof = all_code(input.len());
        c.records += relocs.len() as u64;
        for r in &relocs {
            *c.by_type.entry(r.r_type).or_default() += 1;
        }
        for (output, oname) in [(OutputKind::Static, "static"), (OutputKind::Pie, "static-pie")] {
            let cx = ScanCx { tables: &tables, output, place, name: sec.name.as_bytes() };
            let mut sink = Sink::default();
            let mut refused: Vec<Option<String>> = vec![None; relocs.len()];
            for (i, r) in relocs.iter().enumerate() {
                let site = Site { bytes: input, offset: r.r_offset };
                let h = match X86::M1.classify(r.r_type, site) {
                    Ok(h) => h,
                    Err(refusal) => {
                        if matches!(refusal.name, RefName::Hex(_)) {
                            c.unnamed.push(format!(
                                "{what} {}+{:#x}: type {} unnamed ({oname})",
                                sec.name, r.r_offset, r.r_type
                            ));
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
                    addend: None,
                    howto: &h,
                    site,
                    records: &relocs.as_slice(),
                    proof: &proof,
                    leads: true,
                };
                X86::M1.scan(&cx, &at, &mut DemandSink::new(&mut sink));
            }
            let mut first: BTreeMap<usize, String> = BTreeMap::new();
            for (i, e) in &sink.errors {
                let i = *i as usize;
                let r = &relocs[i];
                let at = || {
                    format!(
                        "{what} {}+{:#x} type {} ({}) {oname}: {e:?}\n    bytes {}",
                        sec.name,
                        r.r_offset,
                        r.r_type,
                        mortar_elf::reloc::i386::name(r.r_type).unwrap_or("?"),
                        around(input, r.r_offset)
                    )
                };
                let outcome = match e {
                    ScanError::Refused(x) => {
                        if matches!(x.name, RefName::Hex(_)) {
                            c.unnamed.push(at());
                        }
                        format!("refused: {}", x.name)
                    }
                    ScanError::Incompatible(x) => format!("incompatible: {x:?}"),
                    _ => {
                        c.unnamed.push(at());
                        format!("{e:?}")
                    }
                };
                if is_unmatched(r.r_type, e) {
                    c.unmatched.push(at());
                }
                first.entry(i).or_insert(outcome);
            }
            for (i, r) in relocs.iter().enumerate() {
                let outcome = refused[i]
                    .take()
                    .or_else(|| first.remove(&i))
                    .unwrap_or_else(|| "accepted".to_string());
                *c.outcomes.entry((oname, r.r_type, outcome)).or_default() += 1;
            }
        }
    }
}

fn report(name: &str, extra: &str, c: &Census) {
    eprintln!(
        "== {name}: {} objects{extra}, {} records, {} SHT_RELA sections",
        c.objects, c.records, c.rela_sections
    );
    let types: Vec<String> = c
        .by_type
        .iter()
        .map(|(t, n)| format!("{}={n}", mortar_elf::reloc::i386::name(*t).unwrap_or("?")))
        .collect();
    eprintln!("   records: {}", types.join(" "));
    for ((o, t, what), n) in &c.outcomes {
        let tn = mortar_elf::reloc::i386::name(*t).unwrap_or("?");
        eprintln!("   {o:<10} {t:<3} {tn:<20} {what:<56} {n}");
    }
    eprintln!("   unnamed {}, unmatched {}", c.unnamed.len(), c.unmatched.len());
}

#[test]
fn census_every_record_scans_or_is_refused_by_name() {
    let full = std::env::var_os("MORTAR_X86_CENSUS").is_some_and(|v| v == "full");
    let started = Instant::now();
    let dir = Scratch::new("x86-census");
    let tools = Tools::find();
    let mut jobs = Vec::new();
    jobs_of(0, &review_entries(), full, &tools, &dir, &mut jobs);
    jobs_of(1, &census_entries(), full, &tools, &dir, &mut jobs);
    let configs =
        [jobs.iter().filter(|j| j.set == 0).count(), jobs.iter().filter(|j| j.set == 1).count()];

    // A few compilers at a time (each `run` holds a jobserver token).
    let next = Mutex::new(0usize);
    let built = Mutex::new(Vec::new());
    let failed = Mutex::new(Vec::new());
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
                    // A configuration a compiler rejects is skipped, as
                    // the scripts do (the reviewer's lists 8).
                    if run(&j.exe, &j.args).success() {
                        built.lock().unwrap().push(i);
                    } else {
                        failed.lock().unwrap().push(i);
                    }
                }
            });
        }
    });
    let mut built = built.into_inner().unwrap();
    built.sort_unstable();
    let mut failed = failed.into_inner().unwrap();
    failed.sort_unstable();
    let compiled = started.elapsed();

    let mut sets: [Census; 3] = Default::default();
    for &i in &built {
        let j = &jobs[i];
        let img = Image::read(&j.obj);
        census_object(&img, &j.what, &mut sets[j.set]);
    }

    let mut files = Vec::new();
    for d in SYSTEM_DIRS {
        if Path::new(d).is_dir() {
            walk(Path::new(d), &mut files);
        } else {
            assert!(!tools_required(), "{d} not found (MORTAR_REQUIRE_TOOLS=1)");
            eprintln!("note: {d} not found; skipping its part of the system set");
        }
    }
    for f in &files {
        let bytes = std::fs::read(f).unwrap();
        if bytes.starts_with(b"!<arch>\n") {
            for (m, body) in members(&bytes) {
                if is_i386_rel(body) {
                    let img = Image::parse(body.to_vec());
                    census_object(&img, &format!("{}({m})", f.display()), &mut sets[2]);
                }
            }
        } else if is_i386_rel(&bytes) {
            census_object(&Image::parse(bytes), &f.display().to_string(), &mut sets[2]);
        }
    }

    eprintln!(
        "census ({}): compiled in {:.1?}, total {:.1?}",
        if full { "full" } else { "cut" },
        compiled,
        started.elapsed()
    );
    for (s, name) in [(0, "review"), (1, "census")] {
        let fails = failed.iter().filter(|&&i| jobs[i].set == s).count();
        report(
            name,
            &format!(" of {} configurations ({fails} failed to compile)", configs[s]),
            &sets[s],
        );
        for &i in failed.iter().filter(|&&i| jobs[i].set == s) {
            eprintln!("   compile failed: {}", jobs[i].what);
        }
    }
    report("system", &format!(" from {} files", files.len()), &sets[2]);

    let unnamed: Vec<&String> = sets.iter().flat_map(|c| &c.unnamed).collect();
    let unmatched: Vec<&String> = sets.iter().flat_map(|c| &c.unmatched).collect();
    let show = |v: &[&String]| v.iter().take(50).map(|s| s.as_str()).collect::<Vec<_>>().join("\n");
    assert!(unnamed.is_empty(), "{} unnamed refusals:\n{}", unnamed.len(), show(&unnamed));
    assert!(unmatched.is_empty(), "{} unmatched records:\n{}", unmatched.len(), show(&unmatched));
    if !jobs.is_empty() {
        assert!(sets[0].objects + sets[1].objects >= 500 || full, "too few corpus objects");
    }
}

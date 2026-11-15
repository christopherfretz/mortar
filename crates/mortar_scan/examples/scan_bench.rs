//! The scan (`spec/image.md` §4) on a bench workload and on synthetic
//! relocations: per thread count, the scan's wall time and throughput
//! (relocation records per second), the peak RSS, the machine's load,
//! and the scan's statistics (units, par calls).
//!
//! `cargo run --release -p mortar_scan --example scan_bench -- workload
//! DIR [THREADS...]`: `DIR` is a workload (`DIR/response.txt`, one
//! argument per line, run in `DIR/build`; the LTO plugin's options are
//! removed).
//!
//! `... -- synth N SCRATCH [THREADS...]`: N relocation records in 64
//! generated AArch64 objects written to `SCRATCH` (created; removed at
//! the end), linked `-static`.
//!
//! `SCAN_STORE=resident` loads the inputs with the resident store; the
//! default is the staged store (`spec/input.md` §11.1).
//!
//! Thread counts default to 1 2 4 8. Each runs in its own process, so its
//! peak RSS is its own; each process runs the phases before the scan
//! once and the scan `RUNS` times (default 5), reporting the median and
//! minimum.

#[path = "../tests/support/mod.rs"]
mod support;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use mortar_core::par::Pool;

fn status(key: &str) -> String {
    std::fs::read_to_string("/proc/self/status")
        .unwrap_or_default()
        .lines()
        .find(|l| l.starts_with(key))
        .map(|l| l[key.len()..].trim().to_owned())
        .unwrap_or_default()
}

fn loadavg() -> String {
    std::fs::read_to_string("/proc/loadavg")
        .unwrap_or_default()
        .split_whitespace()
        .take(3)
        .collect::<Vec<_>>()
        .join(" ")
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

/// The plugin's options removed (mortar has no plugin interface).
fn strip_plugin(words: impl IntoIterator<Item = String>) -> Vec<OsString> {
    let mut out = Vec::new();
    let mut it = words.into_iter();
    while let Some(w) = it.next() {
        if w == "-plugin" {
            it.next();
        } else if !w.starts_with("-plugin-opt=") && !w.is_empty() {
            out.push(OsString::from(w));
        }
    }
    out
}

fn child(cwd: &Path, args: &[OsString], threads: usize, runs: usize) {
    let pool = Pool::new(threads);
    let load_before = loadavg();
    let t = Instant::now();
    let policy = match std::env::var("SCAN_STORE").as_deref() {
        Ok("resident") => mortar_input::store::StorePolicy::Resident,
        _ => mortar_input::store::StorePolicy::Staged,
    };
    let Some(l) = support::link_in(cwd, args, &pool, policy) else {
        eprintln!("threads {threads}: the inputs do not load");
        std::process::exit(1);
    };
    let before = t.elapsed();
    let mut times = Vec::new();
    let mut stats = l.scanned.stats;
    let mut diags = l.scanned.diagnostics.len();
    for _ in 0..runs {
        let t = Instant::now();
        let s = l.rescan(&pool);
        times.push(t.elapsed());
        stats = s.stats;
        diags = s.diagnostics.len();
    }
    times.sort();
    if threads == 1 || std::env::var_os("SCAN_IDS").is_some() {
        let mut ids = std::collections::BTreeMap::<&str, usize>::new();
        for (_, d) in l.scanned.diagnostics.iter().chain(&l.before) {
            *ids.entry(d.id()).or_default() += 1;
        }
        println!("diagnostic ids (scan and before): {ids:?}");
        if let Some((k, d)) = l.scanned.diagnostics.iter().min_by_key(|x| x.0) {
            print!(
                "first: {}",
                support::render(&[(*k, d.clone())], mortar_diag::Policy::default())
            );
        }
    }
    let median = times[runs / 2];
    #[allow(clippy::cast_precision_loss, reason = "a rate")]
    let per_s = stats.records as f64 / median.as_secs_f64();
    println!(
        "{policy:?} threads {threads}: scan median {:.1} ms (min {:.1}); {:.1} M records/s; \
         records {}, sections {}, units {}, par calls {}, rewalked {}, events {}, diagnostics {diags} \
         (phases before: {:.0} ms, {} before-diagnostics); VmHWM {}; load {load_before} -> {}",
        ms(median),
        ms(times[0]),
        per_s / 1e6,
        stats.records,
        stats.sections,
        stats.units,
        stats.par_calls,
        stats.rewalked,
        stats.events,
        ms(before),
        l.before.len(),
        status("VmHWM:"),
        loadavg(),
    );
}

// ---- The synthetic objects --------------------------------------------

const FILES: usize = 64;
/// Globals each file defines.
const GLOBALS: usize = 4096;

fn put16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn put32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn put64(v: &mut Vec<u8>, x: u64) {
    v.extend_from_slice(&x.to_le_bytes());
}

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 =
            self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
}

/// An AArch64 relocatable object of `records` relocations on `.text`, in
/// groups of four instructions: `bl` (CALL26) to a global of another
/// file, `adrp`/`add` (`ADR_PREL_PG_HI21`, `ADD_ABS_LO12_NC`) to a local,
/// `adrp`/`ldr` (`ADR_GOT_PAGE`, `LD64_GOT_LO12_NC`) to a global. It defines
/// `GLOBALS` globals (and `_start` in file 0).
#[allow(clippy::too_many_lines, reason = "one ELF object, header to section table")]
fn object(file: usize, records: usize) -> Vec<u8> {
    let mut r = Lcg(file as u64 + 1);
    let records = records / 5 * 5;
    let insns = records; // one record per instruction
    let mut text = Vec::with_capacity(insns * 4);
    let mut rela = Vec::with_capacity(records * 24);
    // Symbols: 0 null, 1 section .text, 2 `$x`, 3 local `l`, then globals:
    // GLOBALS defined, then the undefined ones named by other files.
    let first_global = 4u32;
    let undef_base = first_global + GLOBALS as u32 + u32::from(file == 0);
    let mut undef: Vec<(usize, usize)> = Vec::new();
    let mut undef_index = std::collections::HashMap::new();
    let mut global = |r: &mut Lcg| {
        let f = (file + 1 + (r.next() as usize % (FILES - 1))) % FILES;
        let g = r.next() as usize % GLOBALS;
        *undef_index.entry((f, g)).or_insert_with(|| {
            undef.push((f, g));
            undef_base + undef.len() as u32 - 1
        })
    };
    let mut rel = |off: usize, sym: u32, ty: u32| {
        put64(&mut rela, off as u64);
        put64(&mut rela, u64::from(sym) << 32 | u64::from(ty));
        put64(&mut rela, 0);
    };
    for i in 0..insns / 5 {
        let o = i * 20;
        put32(&mut text, 0x9400_0000); // bl
        put32(&mut text, 0x9000_0000); // adrp x0
        put32(&mut text, 0x9100_0000); // add x0, x0, #0
        put32(&mut text, 0x9000_0001); // adrp x1
        put32(&mut text, 0xf940_0021); // ldr x1, [x1]
        let s = global(&mut r);
        rel(o, s, 283); // CALL26
        rel(o + 4, 3, 275); // ADR_PREL_PG_HI21
        rel(o + 8, 3, 277); // ADD_ABS_LO12_NC
        let s = global(&mut r);
        rel(o + 12, s, 311); // ADR_GOT_PAGE
        rel(o + 16, s, 312); // LD64_GOT_LO12_NC
    }
    if text.is_empty() {
        put32(&mut text, 0xd65f_03c0); // ret
    }
    // String table and symbols.
    let mut strtab = vec![0u8];
    let mut name = |s: &str| {
        let at = strtab.len() as u32;
        strtab.extend_from_slice(s.as_bytes());
        strtab.push(0);
        at
    };
    let mut symtab = vec![0u8; 24];
    let sym = |v: &mut Vec<u8>, name: u32, info: u8, shndx: u16, value: u64| {
        put32(v, name);
        v.push(info);
        v.push(0);
        put16(v, shndx);
        put64(v, value);
        put64(v, 0);
    };
    let len = text.len() as u64;
    sym(&mut symtab, 0, 3, 1, 0); // STT_SECTION .text
    let x = name("$x");
    sym(&mut symtab, x, 0, 1, 0);
    let l = name("l");
    sym(&mut symtab, l, 0, 1, 0);
    for g in 0..GLOBALS {
        let n = name(&format!("g{file}_{g}"));
        sym(&mut symtab, n, 0x12, 1, (g as u64 * 4) % len); // GLOBAL FUNC
    }
    if file == 0 {
        let n = name("_start");
        sym(&mut symtab, n, 0x12, 1, 0);
    }
    for &(f, g) in &undef {
        let n = name(&format!("g{f}_{g}"));
        sym(&mut symtab, n, 0x10, 0, 0); // GLOBAL NOTYPE UND
    }
    let shstr = b"\0.text\0.rela.text\0.symtab\0.strtab\0.shstrtab\0";
    // Layout: header, .text, .rela.text, .symtab, .strtab, .shstrtab,
    // section headers.
    let mut out = vec![0u8; 64];
    let at = |out: &mut Vec<u8>, data: &[u8], align: usize| {
        while !out.len().is_multiple_of(align) {
            out.push(0);
        }
        let off = out.len();
        out.extend_from_slice(data);
        off as u64
    };
    let text_off = at(&mut out, &text, 4);
    let rela_off = at(&mut out, &rela, 8);
    let sym_off = at(&mut out, &symtab, 8);
    let str_off = at(&mut out, &strtab, 1);
    let shstr_off = at(&mut out, shstr, 1);
    while !out.len().is_multiple_of(8) {
        out.push(0);
    }
    let shoff = out.len() as u64;
    let sh = |out: &mut Vec<u8>, n, ty, flags, off, size, link, info, align, ent| {
        put32(out, n);
        put32(out, ty);
        put64(out, flags);
        put64(out, 0);
        put64(out, off);
        put64(out, size);
        put32(out, link);
        put32(out, info);
        put64(out, align);
        put64(out, ent);
    };
    sh(&mut out, 0, 0, 0, 0, 0, 0, 0, 0, 0);
    sh(&mut out, 1, 1, 6, text_off, len, 0, 0, 4, 0);
    sh(&mut out, 7, 4, 0x40, rela_off, rela.len() as u64, 3, 1, 8, 24);
    sh(&mut out, 18, 2, 0, sym_off, symtab.len() as u64, 4, first_global, 8, 24);
    sh(&mut out, 26, 3, 0, str_off, strtab.len() as u64, 0, 0, 1, 0);
    sh(&mut out, 34, 3, 0, shstr_off, shstr.len() as u64, 0, 0, 1, 0);
    // The ELF header.
    let mut h = Vec::with_capacity(64);
    h.extend_from_slice(b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0");
    put16(&mut h, 1); // ET_REL
    put16(&mut h, 183); // EM_AARCH64
    put32(&mut h, 1);
    put64(&mut h, 0);
    put64(&mut h, 0);
    put64(&mut h, shoff);
    put32(&mut h, 0);
    put16(&mut h, 64);
    put16(&mut h, 0);
    put16(&mut h, 0);
    put16(&mut h, 64);
    put16(&mut h, 6);
    put16(&mut h, 5);
    out[..64].copy_from_slice(&h);
    out
}

fn synth(n: usize, scratch: &Path) -> Vec<OsString> {
    std::fs::create_dir_all(scratch).expect("scratch");
    let mut args: Vec<OsString> =
        ["-static", "-e", "_start", "-o", "out"].iter().map(OsString::from).collect();
    let per = n / FILES;
    for f in 0..FILES {
        let p = scratch.join(format!("s{f}.o"));
        std::fs::write(&p, object(f, per)).expect("write");
        args.push(p.into_os_string());
    }
    args
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let runs: usize = std::env::var("RUNS").ok().and_then(|r| r.parse().ok()).unwrap_or(5);
    if args.first().map(String::as_str) == Some("--child") {
        // --child CWD THREADS LINK-ARGS...
        let link: Vec<OsString> = args[3..].iter().map(OsString::from).collect();
        child(Path::new(&args[1]), &link, args[2].parse().expect("threads"), runs);
        return;
    }
    let usage = || -> ! {
        eprintln!("usage: scan_bench workload DIR [THREADS...] | synth N SCRATCH [THREADS...]");
        std::process::exit(2);
    };
    let (cwd, link, rest, cleanup): (PathBuf, Vec<OsString>, &[String], Option<PathBuf>) =
        match args.first().map(String::as_str) {
            Some("workload") if args.len() > 1 => {
                let dir = PathBuf::from(&args[1]);
                let text = std::fs::read_to_string(dir.join("response.txt")).expect("response.txt");
                let link = strip_plugin(text.lines().map(str::to_owned));
                (dir.join("build"), link, &args[2..], None)
            }
            Some("synth") if args.len() > 2 => {
                let n: usize = args[1].parse().expect("N");
                let scratch = PathBuf::from(&args[2]);
                let t = Instant::now();
                let link = synth(n, &scratch);
                println!(
                    "synthetic: {n} records in {FILES} objects, written in {:.0} ms",
                    ms(t.elapsed())
                );
                (scratch.clone(), link, &args[3..], Some(scratch))
            }
            _ => usage(),
        };
    let threads: Vec<usize> = if rest.is_empty() {
        vec![1, 2, 4, 8]
    } else {
        rest.iter().map(|t| t.parse().expect("threads")).collect()
    };
    let exe = std::env::current_exe().expect("the example's path");
    println!("cwd {}; {runs} scans per thread count", cwd.display());
    for t in threads {
        let status = Command::new(&exe)
            .arg("--child")
            .arg(&cwd)
            .arg(t.to_string())
            .args(&link)
            .status()
            .expect("the child");
        assert!(status.success(), "the {t}-thread child failed");
    }
    if let Some(dir) = cleanup {
        for f in 0..FILES {
            let _ = std::fs::remove_file(dir.join(format!("s{f}.o")));
        }
        let _ = std::fs::remove_dir(&dir);
    }
}

//! WP-048's oracle: x86-64 dynamic executables against GNU ld 2.44 and
//! lld 22, without mortar linking anything (the dynamic output is refused
//! until the join).
//!
//! A shared library (GNU ld) defines TLS and ordinary functions and data.
//! One translation unit (`T`) reaching them with every TLS model is
//! compiled over the census matrix of the brief: GCC 14, Clang 19 and 22;
//! `-fPIC`/`-fPIE`; `-mtls-dialect=gnu`/`gnu2`; `-O0`/`-O2`; with and
//! without `-fno-plt`; `-mcmodel=small` (plus `-fno-pic` objects for the
//! copy and canonical-PLT cells of a non-PIE). Each object is linked by
//! both references as a PIE and as a non-PIE, lazily, and with `-z now`
//! at `-O2`. Then, for each link:
//!
//! - **scan**: mortar's `scan` of every record, `exec` or `pie`, must not
//!   refuse (the census proof of X-26/X-28: every form the compilers emit
//!   is matched), and the demand it raises on each shared key must be what
//!   the reference made (`JUMP_SLOT` ⇔ PLT, `GLOB_DAT` ⇔ GOT, `COPY` ⇔
//!   COPY, an undefined `.dynsym` entry with a value ⇔ canonical PLT,
//!   `TPOFF64` ⇔ `GOT_TPREL`, `DTPMOD64` ⇔ `TLS_GD`);
//! - **apply**: every record of the object's code is applied by mortar
//!   with `S`, `G`, `L`, the GOT base and the TLS template read from the
//!   reference's image, and the code compared byte for byte with the
//!   reference's. A differing record is **documented** only for D1 (an IE
//!   `add` relaxed to LE: mortar `add $T`, both references `lea`), else a
//!   **mismatch**; there must be none;
//! - **PLT**: PLT0 and every lazy entry (§10.3, D-49) are rebuilt by
//!   `synth::plt0`/`synth::plt_entry` at the reference's addresses and
//!   compared byte for byte.
#![allow(clippy::many_single_char_names, clippy::too_many_lines)]

mod elf;
mod support;

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use elf::{Image, SHF_EXECINSTR, SHF_TLS, STT_SECTION, STT_TLS, Shdr, Sym, u32_at, u64_at};
use mortar_arch::{
    Arch, DemandFlags, DemandSink, FillCx, OutputKind, PlaceFacts, Reloc, RelocAt, ScanCx,
    SectionOut, Site, SlotKind, TargetKind,
};
use mortar_core::{FileId, SectionId};
use mortar_testutil::{Scratch, run, tool};
use mortar_x64::{X64, synth};
use support::{Sink, Tables, all_code, key};

const LIB: &str = r"
__thread int tlsx = 3;
__thread int tlsy = 4;
int ldata = 5;
int lf(int x) { return x + 1; }
int lh(int x) { return x + 2; }
int lg(int x) { return x + 3; }
";

/// Every TLS model against a shared key and against the executable's own
/// TLS, a call, a data read and a function's address.
const T: &str = r#"
extern __thread int tlsx;
extern __thread int tlsy __attribute__((tls_model("initial-exec")));
__thread int mine = 7;
static __thread int st;
static __thread int st2 = 2;
__thread int mine_ie __attribute__((tls_model("initial-exec"))) = 9;
extern int ldata;
extern int lf(int), lh(int), lg(int);
int t_gd(void) { return tlsx; }
int *t_gd_addr(void) { return &tlsx; }
int t_ie(void) { return tlsy; }
int *t_ie_addr(void) { return &tlsy; }
int t_ie_exe(void) { return mine_ie; }
int *t_ie_exe_addr(void) { return &mine_ie; }
int t_gd_exe(void) { return mine; }
int t_ld(void) { return st++ + st2; }
int t_call(int x) { return lf(x) + lh(x); }
int t_data(void) { return ldata; }
void *t_fp(void) { return (void *)lg; }
int main(void) { return t_gd() + *t_gd_addr() + t_ie() + *t_ie_addr() + t_ie_exe() + *t_ie_exe_addr() + t_gd_exe() + t_ld() + t_call(1) + t_data() + (t_fp() != 0); }
"#;

const R_GLOB_DAT: u32 = 6;
const R_JUMP_SLOT: u32 = 7;
const R_RELATIVE: u32 = 8;
const R_COPY: u32 = 5;
const R_DTPMOD64: u32 = 16;
const R_TPOFF64: u32 = 18;
const SHT_DYNSYM: u32 = 11;

struct Rela {
    off: u64,
    ty: u32,
    sym: u32,
    addend: i64,
}

fn relas(img: &Image, s: &Shdr) -> Vec<Rela> {
    img.data(s)
        .chunks_exact(24)
        .map(|e| {
            let info = u64_at(e, 8);
            Rela {
                off: u64_at(e, 0),
                ty: (info & 0xffff_ffff) as u32,
                sym: (info >> 32) as u32,
                addend: u64_at(e, 16) as i64,
            }
        })
        .collect()
}

fn syms_in(img: &Image, kind: u32) -> Vec<Sym> {
    let Some(st) = img.sections.iter().find(|s| s.kind == kind) else { return Vec::new() };
    let names = img.data(&img.sections[st.link as usize]);
    img.data(st)
        .chunks_exact(24)
        .map(|e| {
            let n = &names[u32_at(e, 0) as usize..];
            Sym {
                name: String::from_utf8_lossy(&n[..n.iter().position(|&b| b == 0).unwrap_or(0)])
                    .into_owned(),
                info: e[4],
                shndx: elf::u16_at(e, 6),
                value: u64_at(e, 8),
                size: u64_at(e, 16),
            }
        })
        .collect()
}

// ---------------------------------------------------------------- the matrix

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cc {
    Gcc,
    Clang19,
    Clang22,
}

#[derive(Clone, Debug)]
struct Obj {
    cc: Cc,
    flags: Vec<&'static str>,
    nopic: bool,
    o2: bool,
}

impl Obj {
    fn name(&self) -> String {
        format!("{:?} {}", self.cc, self.flags.join(" "))
    }
}

fn matrix() -> Vec<Obj> {
    let mut v = Vec::new();
    for cc in [Cc::Gcc, Cc::Clang19, Cc::Clang22] {
        for pic in ["-fPIC", "-fPIE"] {
            for dialect in ["-mtls-dialect=gnu", "-mtls-dialect=gnu2"] {
                for opt in ["-O0", "-O2"] {
                    for plt in [false, true] {
                        let mut flags = vec![pic, dialect, opt, "-mcmodel=small"];
                        if plt {
                            flags.push("-fno-plt");
                        }
                        v.push(Obj { cc, flags, nopic: false, o2: opt == "-O2" });
                    }
                }
            }
        }
        v.push(Obj { cc, flags: vec!["-fno-pic", "-O2"], nopic: true, o2: true });
    }
    v
}

struct Tools {
    gcc: PathBuf,
    clang19: PathBuf,
    clang22: PathBuf,
    lld_dir: PathBuf,
}

fn tools() -> Option<Tools> {
    tool("x86_64-linux-gnu-ld.bfd")?;
    let lld = tool("ld.lld")?;
    let lld_dir = [PathBuf::from("/usr/lib/llvm-22/bin")]
        .into_iter()
        .find(|d| d.join("ld.lld").is_file())
        .unwrap_or_else(|| lld.parent().unwrap().to_path_buf());
    Some(Tools {
        gcc: tool("x86_64-linux-gnu-gcc")?,
        clang19: tool("clang-19")?,
        clang22: tool("clang-22")?,
        lld_dir,
    })
}

fn must(what: &str, o: &mortar_testutil::Outcome) {
    assert!(o.success(), "{what} failed: {}", o.stderr_text());
}

// ---------------------------------------------------------------- one link

#[derive(Default)]
struct Report {
    links: u32,
    /// `(class, linker)` → [ok, documented, mismatch].
    records: BTreeMap<(String, &'static str), [u32; 3]>,
    plt: [u32; 2],
    demand: [u32; 2],
    failures: Vec<String>,
}

/// The transition one record exercises.
fn class(ty: u32, kind: TargetKind) -> String {
    let shared = matches!(kind, TargetKind::Shlib { .. });
    let s = match (ty, shared) {
        (19, true) => "GD→IE",
        (19, false) => "GD→LE",
        (20, _) => "LD→LE",
        (21, _) => "DTPOFF32",
        (22, true) => "IE kept",
        (22, false) => "IE→LE",
        (23, _) => "LE",
        (34, true) => "TLSDESC→IE",
        (34, false) => "TLSDESC→LE",
        (35, _) => "TLSDESC_CALL",
        (4, true) => "PLT32 shared",
        (4, false) => "PLT32 local",
        (2, true) => "PC32 shared",
        (9 | 41 | 42, true) => "GOTPCREL shared",
        (9 | 41 | 42, false) => "GOTPCREL local",
        (10 | 11, true) => "32/32S shared",
        (1, _) => "64",
        _ => return format!("type {ty}"),
    };
    s.to_string()
}

struct Layout {
    img: Image,
    out_syms: HashMap<String, Sym>,
    out_locals: HashMap<String, Sym>,
    dynsyms: Vec<Sym>,
    rela: Vec<Rela>,
    rela_plt: Vec<Rela>,
    /// The entry address of each GOT word a PLT entry jumps through.
    entries: HashMap<u64, u64>,
    got_base: u64,
}

impl Layout {
    fn new(img: Image) -> Self {
        let mut out_syms = HashMap::new();
        let mut out_locals = HashMap::new();
        for s in img.symbols() {
            if s.shndx == 0 || s.name.is_empty() {
                continue;
            }
            if s.info >> 4 == 0 {
                out_locals.insert(s.name.clone(), s);
            } else {
                out_syms.insert(s.name.clone(), s);
            }
        }
        let dynsyms = syms_in(&img, SHT_DYNSYM);
        let mut rela = Vec::new();
        let mut rela_plt = Vec::new();
        for s in img.sections.iter().filter(|s| s.kind == elf::SHT_RELA) {
            if s.name == ".rela.plt" {
                rela_plt = relas(&img, s);
            } else {
                rela.extend(relas(&img, s));
            }
        }
        let mut entries = HashMap::new();
        for s in img.sections.iter().filter(|s| s.name.starts_with(".plt")) {
            let d = img.data(s);
            let step = if s.name == ".plt.got" { 8 } else { 16 };
            for o in (0..d.len()).step_by(step) {
                if d[o..].starts_with(&[0xff, 0x25]) {
                    let rel = i64::from(u32_at(d, o + 2) as i32);
                    let at = s.addr + o as u64;
                    entries.insert((at + 6).wrapping_add(rel as u64), at);
                }
            }
        }
        let got_base = out_locals
            .get("_GLOBAL_OFFSET_TABLE_")
            .or_else(|| out_syms.get("_GLOBAL_OFFSET_TABLE_"))
            .map_or(0, |s| s.value);
        Self { img, out_syms, out_locals, dynsyms, rela, rela_plt, entries, got_base }
    }

    fn dyn_name(&self, i: u32) -> &str {
        self.dynsyms.get(i as usize).map_or("", |s| s.name.as_str())
    }

    fn rec(&self, ty: u32, name: &str) -> Option<u64> {
        self.rela
            .iter()
            .find(|r| r.ty == ty && r.sym != 0 && self.dyn_name(r.sym) == name)
            .map(|r| r.off)
    }

    fn jump_slot(&self, name: &str) -> Option<u64> {
        self.rela_plt
            .iter()
            .find(|r| r.ty == R_JUMP_SLOT && self.dyn_name(r.sym) == name)
            .map(|r| r.off)
    }

    fn got_word_of(&self, value: u64, rel: Option<i64>) -> Option<u64> {
        if let Some(a) = rel
            && let Some(r) = self.rela.iter().find(|r| r.sym == 0 && r.addend == a)
        {
            return Some(r.off);
        }
        self.img.got_words().get(&value).and_then(|v| v.first().copied())
    }
}

fn and(a: DemandFlags, b: DemandFlags) -> DemandFlags {
    DemandFlags::from_bits(a.bits() & b.bits()).unwrap()
}

/// What the reference made for one shared key, as demand flags.
fn ref_demand(l: &Layout, name: &str) -> DemandFlags {
    let mut f = DemandFlags::empty();
    if l.jump_slot(name).is_some() {
        f |= DemandFlags::PLT;
    }
    if l.rec(R_GLOB_DAT, name).is_some() {
        f |= DemandFlags::GOT;
    }
    if l.rec(R_COPY, name).is_some() {
        f |= DemandFlags::COPY;
    }
    if l.rec(R_TPOFF64, name).is_some() {
        f |= DemandFlags::GOT_TPREL;
    }
    if l.rec(R_DTPMOD64, name).is_some() {
        f |= DemandFlags::TLS_GD;
    }
    if l.dynsyms.iter().any(|s| s.name == name && s.shndx == 0 && s.value != 0) {
        f |= DemandFlags::CANON_PLT;
    }
    f
}

#[allow(clippy::too_many_arguments)]
fn compare_link(
    rep: &Mutex<Report>,
    what: &str,
    linker: &'static str,
    obj: &Image,
    l: &Layout,
    output: OutputKind,
) {
    let arch = X64::M2;
    let osyms = obj.symbols();
    let tls = l.img.tls.expect("PT_TLS");
    let tls_end = tls.start + tls.size.next_multiple_of(tls.align.max(1));
    let mut t = Tables::default();
    let mut kinds: HashMap<u32, TargetKind> = HashMap::new();
    let mut masked: Vec<u32> = Vec::new();
    // Each code section's output address: by a function defined in it.
    let mut bases: HashMap<usize, u64> = HashMap::new();
    for s in &osyms {
        if s.info & 0xf == 2
            && s.shndx != 0
            && s.info >> 4 == 1
            && let Some(o) = l.out_syms.get(&s.name)
        {
            bases.insert(usize::from(s.shndx), o.value - s.value);
        }
    }
    let mut local = Vec::new();
    for (i, s) in osyms.iter().enumerate() {
        let i = i as u32;
        let k = key(i);
        let tlsy = s.info & 0xf == STT_TLS;
        let (value, kind) = if s.info & 0xf == STT_SECTION {
            let sh = &obj.sections[usize::from(s.shndx)];
            let out = if sh.flags & SHF_TLS != 0 {
                l.img.sections.iter().find(|o| o.name == sh.name).map(|o| o.addr)
            } else {
                bases.get(&usize::from(s.shndx)).copied()
            };
            if out.is_none() {
                masked.push(i);
            }
            if sh.flags & SHF_TLS != 0 {
                t.tls_sym(k, out.unwrap_or(0), TargetKind::Local);
            } else {
                t.sym(k, out.unwrap_or(0), TargetKind::Local);
            }
            kinds.insert(i, TargetKind::Local);
            continue;
        } else if s.shndx != 0 {
            let o =
                if s.info >> 4 == 0 { l.out_locals.get(&s.name) } else { l.out_syms.get(&s.name) };
            // A TLS symbol's value in an executable is its offset in PT_TLS.
            if let Some(o) = o {
                (if tlsy { tls.start + o.value } else { o.value }, TargetKind::Local)
            } else {
                masked.push(i);
                (0, TargetKind::Local)
            }
        } else if s.name.is_empty() {
            (0, TargetKind::Local)
        } else if let Some(c) = l.rec(R_COPY, &s.name) {
            t.flags.insert(k, DemandFlags::COPY);
            (c, TargetKind::Shlib { func: false })
        } else if let Some(d) = l.dynsyms.iter().find(|d| d.name == s.name && d.shndx == 0) {
            let func = d.info & 0xf == 2;
            if d.value != 0 {
                t.flags.insert(k, DemandFlags::CANON_PLT);
            }
            (d.value, TargetKind::Shlib { func })
        } else if s.name == "__tls_get_addr" {
            (0, TargetKind::Shlib { func: true })
        } else if let Some(o) = l.out_syms.get(&s.name) {
            (if tlsy { tls.start + o.value } else { o.value }, TargetKind::Local)
        } else {
            masked.push(i);
            (0, TargetKind::Local)
        };
        if tlsy {
            t.tls_sym(k, value, kind);
        } else {
            t.sym(k, value, kind);
        }
        kinds.insert(i, kind);
        t.names.insert(k, s.name.clone().into_bytes());
        if matches!(kind, TargetKind::Shlib { .. }) {
            let name = s.name.as_str();
            if let Some(g) = l.rec(R_GLOB_DAT, name) {
                t.slots.insert((k, SlotKind::Addr), g);
                if let Some(&e) = l.entries.get(&g) {
                    t.plts.insert(k, e);
                }
            }
            if let Some(g) = l.rec(R_TPOFF64, name) {
                t.slots.insert((k, SlotKind::TpOff { negated: false }), g);
            }
            if let Some(g) = l.rec(R_DTPMOD64, name) {
                t.slots.insert((k, SlotKind::TlsGd), g);
            }
            if let Some(w) = l.jump_slot(name)
                && let Some(&e) = l.entries.get(&w)
            {
                t.plts.insert(k, e);
            }
            let f = t.flags.entry(k).or_default();
            *f |= and(ref_demand(l, name), DemandFlags::PLT | DemandFlags::GOT);
        } else if s.shndx != 0 {
            local.push((k, value, tlsy));
        }
    }
    for (k, value, tlsy) in local {
        if tlsy {
            let tp = value.wrapping_sub(tls_end);
            if let Some(g) = l.got_word_of(tp, Some(value.wrapping_sub(tls.start) as i64)) {
                t.slots.insert((k, SlotKind::TpOff { negated: false }), g);
            }
        } else if let Some(g) = l.got_word_of(value, Some(value as i64)) {
            t.slots.insert((k, SlotKind::Addr), g);
        }
    }
    let _ = R_RELATIVE;

    let mut rep = rep.lock().unwrap();
    rep.links += 1;
    let mut raised: HashMap<u32, DemandFlags> = HashMap::new();
    for (si, sh) in obj.sections.iter().enumerate() {
        if sh.flags & SHF_EXECINSTR == 0 || sh.size == 0 {
            continue;
        }
        let Some(rs) =
            obj.sections.iter().find(|r| r.kind == elf::SHT_RELA && r.info as usize == si)
        else {
            continue;
        };
        let Some(&base) = bases.get(&si) else {
            rep.failures.push(format!("{what} {linker}: {} not located", sh.name));
            continue;
        };
        let input = obj.data(sh).to_vec();
        let relocs: Vec<Reloc> = relas(obj, rs)
            .iter()
            .map(|r| Reloc { r_offset: r.off, key: key(r.sym), r_type: r.ty, addend: r.addend })
            .collect();
        let proof = all_code(input.len());
        let name = sh.name.as_bytes();
        // scan
        let mut sink = Sink::default();
        for (i, r) in relocs.iter().enumerate() {
            let h = match arch.classify(r.r_type, Site { bytes: &input, offset: r.r_offset }) {
                Ok(h) => h,
                Err(e) => {
                    rep.failures.push(format!("{what} {linker}: classify {} {e:?}", r.r_type));
                    continue;
                }
            };
            let at = RelocAt {
                file: FileId(1),
                section: SectionId(3),
                index: i as u32,
                r_offset: r.r_offset,
                key: r.key,
                r_type: r.r_type,
                addend: Some(r.addend),
                howto: &h,
                site: Site { bytes: &input, offset: r.r_offset },
                records: &relocs,
                proof: &proof,
                leads: true,
            };
            let place = PlaceFacts { alloc: true, writable: false, exec: Some(true) };
            let cx = ScanCx { tables: &t, output, place, name };
            arch.scan(&cx, &at, &mut DemandSink::new(&mut sink));
        }
        for (rec, e) in &sink.errors {
            let r = &relocs[*rec as usize];
            rep.failures.push(format!(
                "{what} {linker} {output:?}: scan refused type {} at {:#x} ({}): {e:?}; bytes {:02x?}",
                r.r_type,
                r.r_offset,
                osyms[r.key.as_global().unwrap().0 as usize].name,
                &input[r.r_offset.saturating_sub(4) as usize..(r.r_offset as usize + 12).min(input.len())]
            ));
        }
        for (k, f) in &sink.raised {
            *raised.entry(k.as_global().unwrap().0).or_default() |= *f;
        }
        // apply
        let mut mine = input.clone();
        let cx = FillCx { tables: &t, output, got_base: Some(l.got_base), tls: Some(tls) };
        let mut bad: Vec<usize> = Vec::new();
        for (i, r) in relocs.iter().enumerate() {
            let Ok(h) = arch.classify(r.r_type, Site { bytes: &input, offset: r.r_offset }) else {
                continue;
            };
            let at = RelocAt {
                file: FileId(1),
                section: SectionId(3),
                index: i as u32,
                r_offset: r.r_offset,
                key: r.key,
                r_type: r.r_type,
                addend: Some(r.addend),
                howto: &h,
                site: Site { bytes: &input, offset: r.r_offset },
                records: &relocs,
                proof: &proof,
                leads: true,
            };
            let mut out = SectionOut::whole(&mut mine, base, true, name).with_exec(true);
            if let Err(e) = arch.apply(&cx, &at, &mut out) {
                rep.failures.push(format!(
                    "{what} {linker} {output:?}: apply type {} at {:#x} ({}): {e:?}",
                    r.r_type,
                    r.r_offset,
                    osyms[r.key.as_global().unwrap().0 as usize].name
                ));
                bad.push(i);
            }
        }
        let rsec = l
            .img
            .sections
            .iter()
            .find(|s| {
                s.flags & SHF_EXECINSTR != 0 && s.addr <= base && base < s.addr + s.size as u64
            })
            .expect("output section");
        let o = rsec.offset + (base - rsec.addr) as usize;
        let mut reference = l.img.bytes[o..o + input.len()].to_vec();
        for (i, r) in relocs.iter().enumerate() {
            if masked.contains(&r.key.as_global().unwrap().0) {
                let a = r.r_offset as usize;
                let n = if r.r_type == 1 { 8 } else { 4 };
                reference[a..a + n].copy_from_slice(&mine[a..a + n]);
                let _ = i;
            }
        }
        // Attribute each differing byte to the nearest record at or after
        // it (a sequence's bytes precede its fields), else before it.
        let mut diff: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
        for j in 0..input.len() {
            if mine[j] != reference[j] {
                let at = relocs
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| {
                        (r.r_offset as usize) + 4 > j && (r.r_offset as usize) <= j + 16
                    })
                    .min_by_key(|(_, r)| (r.r_offset as usize + 4).abs_diff(j))
                    .map_or(usize::MAX, |(i, _)| i);
                diff.insert(at);
            }
        }
        for (i, r) in relocs.iter().enumerate() {
            let kind =
                kinds.get(&r.key.as_global().unwrap().0).copied().unwrap_or(TargetKind::Local);
            let c = class(r.r_type, kind);
            let a = r.r_offset as usize;
            let slot = if !diff.contains(&i) && !bad.contains(&i) {
                0
            } else if r.r_type == 22
                && kind == TargetKind::Local
                && input[a - 2] == 0x03
                && mine[a - 2] == 0x81
                && reference[a - 2] == 0x8d
            {
                1
            } else {
                let w = a.saturating_sub(4)..(a + 12).min(input.len());
                rep.failures.push(format!(
                    "{what} {linker} {output:?}: {c} type {} at {:#x} ({}): input {:02x?} mine {:02x?} ref {:02x?}",
                    r.r_type,
                    r.r_offset,
                    osyms[r.key.as_global().unwrap().0 as usize].name,
                    &input[w.clone()],
                    &mine[w.clone()],
                    &reference[w]
                ));
                2
            };
            rep.records.entry((c, linker)).or_default()[slot] += 1;
        }
        if diff.contains(&usize::MAX) {
            rep.failures
                .push(format!("{what} {linker} {output:?}: a difference outside every record"));
        }
    }
    // The demand on each shared key against the reference's records.
    for (i, f) in raised {
        let s = &osyms[i as usize];
        if !matches!(kinds.get(&i), Some(TargetKind::Shlib { .. })) || s.name == "__tls_get_addr" {
            continue;
        }
        let want = ref_demand(l, &s.name);
        let mask = DemandFlags::PLT
            | DemandFlags::GOT
            | DemandFlags::COPY
            | DemandFlags::CANON_PLT
            | DemandFlags::GOT_TPREL
            | DemandFlags::TLS_GD;
        let mut got = and(f, mask);
        // A function with a GOT use and a call: GNU ld's `.plt.got` entry
        // (the GOT slot, no `JUMP_SLOT`) — mortar's PLT entry.
        if linker == "bfd"
            && got.contains(DemandFlags::GOT | DemandFlags::PLT)
            && !want.contains(DemandFlags::PLT)
        {
            got = and(got, DemandFlags::from_bits(!DemandFlags::PLT.bits() & mask.bits()).unwrap());
        }
        // A canonical PLT entry is a PLT entry (its `JUMP_SLOT`, D-44).
        if got.contains(DemandFlags::CANON_PLT) {
            got |= DemandFlags::PLT;
        }
        if got == want {
            rep.demand[0] += 1;
        } else {
            rep.demand[1] += 1;
            rep.failures.push(format!(
                "{what} {linker} {output:?}: `{}` mortar {got:?}, reference {want:?}",
                s.name
            ));
        }
    }
    // PLT0 and the lazy entries.
    if let Some(plt) = l.img.sections.iter().find(|s| s.name == ".plt") {
        let d = l.img.data(plt);
        let mut check = |at: u64, mine: [u8; 16], what2: &str| {
            let o = (at - plt.addr) as usize;
            if d[o..o + 16] == mine {
                rep.plt[0] += 1;
            } else {
                rep.plt[1] += 1;
                rep.failures.push(format!(
                    "{what} {linker}: {what2} mine {mine:02x?} ref {:02x?}",
                    &d[o..o + 16]
                ));
            }
        };
        check(plt.addr, synth::plt0(plt.addr, l.got_base).unwrap(), "PLT0");
        for (n, r) in l.rela_plt.iter().enumerate() {
            let Some(&e) = l.entries.get(&r.off) else { continue };
            if e < plt.addr || e >= plt.addr + plt.size as u64 {
                continue;
            }
            let b = synth::plt_entry(e, r.off, n as u32, plt.addr).unwrap();
            check(e, b, &format!("entry {n}"));
        }
    }
}

#[test]
fn x64_dynamic_against_gnu_ld_and_lld() {
    let Some(t) = tools() else { return };
    let dir = Scratch::new("x64-dyndiff");
    let lib_c = dir.write("lib.c", LIB);
    let lib_o = dir.join("lib.o");
    must(
        "lib.c",
        &run(
            &t.gcc,
            [
                "-fPIC",
                "-O1",
                "-c",
                &lib_c.display().to_string(),
                "-o",
                &lib_o.display().to_string(),
            ],
        ),
    );
    let so = dir.join("libL.so");
    must(
        "libL.so",
        &run(
            &t.gcc,
            [
                "-shared",
                "-fuse-ld=bfd",
                "-Wl,-soname,libL.so",
                &lib_o.display().to_string(),
                "-o",
                &so.display().to_string(),
            ],
        ),
    );
    let src = dir.write("t.c", T);
    let objs = matrix();
    let rep = Mutex::new(Report::default());
    let next = Mutex::new(0usize);
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                loop {
                    let i = {
                        let mut n = next.lock().unwrap();
                        let i = *n;
                        *n += 1;
                        i
                    };
                    let Some(ob) = objs.get(i) else { break };
                    one_object(&t, &dir, &src, ob, i, &rep);
                }
            });
        }
    });
    let rep = rep.into_inner().unwrap();
    let mut s = String::new();
    let _ = writeln!(
        s,
        "links {}; PLT words ok {} mismatch {}; shared-key demands ok {} mismatch {}",
        rep.links, rep.plt[0], rep.plt[1], rep.demand[0], rep.demand[1]
    );
    let _ = writeln!(s, "{:<16} {:<4} {:>6} {:>6} {:>6}", "record", "ld", "ok", "doc", "bad");
    for ((c, l), [a, b, x]) in &rep.records {
        let _ = writeln!(s, "{c:<16} {l:<4} {a:>6} {b:>6} {x:>6}");
    }
    eprintln!("{s}");
    for f in rep.failures.iter().take(400) {
        eprintln!("FAIL {f}");
    }
    assert!(rep.failures.is_empty(), "{} failures", rep.failures.len());
    // Every transition of D-48 and every cell met, by both references.
    for c in [
        "GD→IE",
        "GD→LE",
        "LD→LE",
        "IE kept",
        "IE→LE",
        "TLSDESC→IE",
        "TLSDESC→LE",
        "LE",
        "DTPOFF32",
        "PLT32 shared",
        "PC32 shared",
        "GOTPCREL shared",
        "GOTPCREL local",
        "32/32S shared",
    ] {
        for l in ["bfd", "lld"] {
            let n = rep.records.get(&(c.to_string(), l)).map_or(0, |v| v[0]);
            assert!(n > 0, "{c} never compared with {l}");
        }
    }
    assert!(rep.plt[0] > 0);
    assert!(rep.links >= 300, "links {}", rep.links);
}

fn one_object(t: &Tools, dir: &Scratch, src: &Path, ob: &Obj, i: usize, rep: &Mutex<Report>) {
    let obj_path = dir.join(format!("t{i}.o"));
    let mut args: Vec<String> = Vec::new();
    let cc = match ob.cc {
        Cc::Gcc => &t.gcc,
        Cc::Clang19 => &t.clang19,
        Cc::Clang22 => &t.clang22,
    };
    if ob.cc != Cc::Gcc {
        args.push("--target=x86_64-linux-gnu".into());
    }
    args.push("-c".into());
    args.extend(ob.flags.iter().map(|s| (*s).to_string()));
    args.extend([src.display().to_string(), "-o".into(), obj_path.display().to_string()]);
    must(&format!("compile {}", ob.name()), &run(cc, &args));
    let obj = Image::read(&obj_path);
    for linker in ["bfd", "lld"] {
        for pie in [false, true] {
            if pie && ob.nopic {
                continue;
            }
            for now in [false, true] {
                if now && !ob.o2 {
                    continue;
                }
                let out = dir.join(format!("e{i}-{linker}-{pie}-{now}"));
                let mut a: Vec<String> = vec![if pie { "-pie" } else { "-no-pie" }.into()];
                if linker == "lld" {
                    a.extend(["-fuse-ld=lld".into(), format!("-B{}/", t.lld_dir.display())]);
                } else {
                    a.push("-fuse-ld=bfd".into());
                }
                if now {
                    a.push("-Wl,-z,now".into());
                }
                a.extend([
                    "-Wl,--build-id=none".into(),
                    obj_path.display().to_string(),
                    format!("-L{}", dir.path().display()),
                    "-lL".into(),
                    "-o".into(),
                    out.display().to_string(),
                ]);
                let what = format!("[{}{}]", ob.name(), if now { " -z now" } else { "" });
                let o = run(&t.gcc, &a);
                if !o.success() {
                    rep.lock().unwrap().failures.push(format!(
                        "{what} {linker} pie={pie}: link failed {}",
                        o.stderr_text()
                    ));
                    continue;
                }
                let l = Layout::new(Image::read(&out));
                let kind = if pie { OutputKind::Pie } else { OutputKind::Exec };
                compare_link(rep, &what, linker, &obj, &l, kind);
                let _ = std::fs::remove_file(&out);
            }
        }
    }
}

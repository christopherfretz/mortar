//! WP-045's oracle for `.dynamic` (`spec/dyn.md` D-30; the brief's
//! "`review-m2-adv/dyntags/tags.py` against GNU ld's output on the same
//! inputs"): GNU ld links the dyntags program (`.preinit_array`, a
//! constructor, `printf`, `memcpy`) for AArch64 and x86-64 under each
//! condition of the experiment; mortar's plan is built from the facts of
//! that output that D-30's conditions read (which sections exist, counts
//! of records, the command line), its values resolved against that
//! output's addresses and a fixture `DynStr` (its `.dynstr` offsets, until
//! WP-M2-DYNSYM lands); and mortar's `.dynamic` is compared with GNU ld's
//! entry by entry: tag, order and value.
//!
//! The differences D-30 states, and only those, are allowed:
//! - **empty `.rela.dyn`**: GNU ld writes `DT_RELA`/`DT_RELASZ`/`DT_RELAENT`
//!   for an empty one; mortar omits them (D-30 rows 21-23, mold's choice);
//! - **`DT_NULL` count**: mortar ends with six; GNU ld with the
//!   `--spare-dynamic-tags` default (5) writes 5 or 6. The references are
//!   linked with `--spare-dynamic-tags=6` so that mortar's table fits in
//!   theirs, which changes only their `DT_NULL` count.
//!
//! Then mortar's bytes replace GNU ld's `.dynamic` in a copy of the output
//! (the rest zero: `DT_NULL`s), and that image is checked by
//! `llvm-readelf -d` (no warning, the same listing as GNU ld's up to the
//! `DT_NULL`s), by `eu-elflint --gnu-ld` (the same verdict as GNU ld's
//! output), and run (natively on its own host, else under `qemu-<arch>`;
//! i386, M4 for dynamic output, checks the ELF32/REL rows): the
//! loader reads mortar's table.

#![allow(clippy::many_single_char_names)]

use std::path::{Path, PathBuf};

use mortar_arch::{DynTag, RelForm, TagWhen};
use mortar_arch::{Facts, GotBase};
use mortar_core::DynStr;
use mortar_core::id::SymbolId;
use mortar_elf::Class;
use mortar_elf::consts::{DT_AARCH64_BTI_PLT, DT_AARCH64_VARIANT_PCS, DT_NULL};
use mortar_image::dynamic::{DynamicAddrs, DynamicChunk, GotTable, dynamic_table, reserved_word};
use mortar_synth::chunks::SynthKind;
use mortar_synth::dynamic::{
    ArraySection, DynKind, DynamicInputs, DynamicPlan, RunpathTag, init_key,
};
use mortar_synth::image::{SizeCx, SynthChunk};
use mortar_testutil::{Scratch, run, target, tool, tools_required};

const PROGRAM: &str = r#"#include <stdio.h>
#include <string.h>
static void pre(void) { }
__attribute__((section(".preinit_array"), used)) static void (*p)(void) = pre;
__attribute__((constructor)) static void ctor(void) { }
void myinit(void) { }
void myfini(void) { }
int main(int c, char **v) { char b[8]; memcpy(b, v[0], 1); printf("%d\n", b[0] != 0); return 0; }
"#;

/// AArch64's tags as `spec/a64.md` §10.3 and D-30 rows 24-25 give them
/// (`Arch::dyn_tags` is WP-M2-A64's; step 0's default is empty).
const A64_TAGS: &[DynTag] = &[
    DynTag { tag: DT_AARCH64_BTI_PLT, value: 0, when: TagWhen::BtiProperty },
    DynTag { tag: DT_AARCH64_VARIANT_PCS, value: 0, when: TagWhen::VariantPcsJumpSlot },
];

/// A condition of the experiment: its name and the driver's flags.
struct Variant {
    name: &'static str,
    flags: &'static [&'static str],
    now: bool,
    runpath: Option<(RunpathTag, &'static [u8])>,
    bti: bool,
    /// `-init=`, `-fini=`: the symbol named.
    init: Option<&'static [u8]>,
    fini: Option<&'static [u8]>,
}

const fn v(name: &'static str, flags: &'static [&'static str]) -> Variant {
    Variant { name, flags, now: false, runpath: None, bti: false, init: None, fini: None }
}

const VARIANTS: &[Variant] = &[
    v("pie", &[]),
    v("nopie", &["-no-pie"]),
    Variant { now: true, ..v("now", &["-Wl,-z,now"]) },
    Variant {
        runpath: Some((RunpathTag::Runpath, b"/x:$ORIGIN/../lib")),
        ..v("rpath", &["-Wl,-rpath,/x", "-Wl,-rpath,$ORIGIN/../lib"])
    },
    Variant {
        runpath: Some((RunpathTag::Rpath, b"/x")),
        ..v("rpath-old", &["-Wl,-rpath,/x", "-Wl,--disable-new-dtags"])
    },
    Variant { now: true, ..v("nopie-now", &["-no-pie", "-Wl,-z,now"]) },
    Variant { bti: true, ..v("bti", &["-mbranch-protection=standard"]) },
    v("relro-off", &["-Wl,-z,norelro"]),
    v("static-pie", &["-static-pie"]),
    Variant {
        init: Some(b"myinit"),
        fini: Some(b"myfini"),
        ..v("init-fini", &["-Wl,-init=myinit", "-Wl,-fini=myfini"])
    },
    // An undefined `-init` symbol: no DT_INIT at all (not `_init`'s).
    Variant { init: Some(b"nosuch"), ..v("init-undefined", &["-Wl,-init=nosuch"]) },
    Variant { now: true, ..v("static-pie-now", &["-static-pie", "-Wl,-z,now"]) },
];

// ---- a small ELF64 reader, independent of mortar's -------------------------

struct Sec {
    name: Vec<u8>,
    sh_type: u32,
    addr: u64,
    offset: u64,
    size: u64,
    link: u32,
    info: u32,
}

struct Elf {
    bytes: Vec<u8>,
    /// ELFCLASS64.
    wide: bool,
    e_type: u16,
    machine: u16,
    secs: Vec<Sec>,
    interp: bool,
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(b[at..at + 2].try_into().unwrap())
}
fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}
/// A word of the class: 8 bytes, or 4 zero-extended.
fn word_at(b: &[u8], at: usize, wide: bool) -> u64 {
    if wide { u64_at(b, at) } else { u64::from(u32_at(b, at)) }
}

impl Elf {
    fn read(path: &Path) -> Self {
        let b = std::fs::read(path).unwrap();
        assert_eq!(&b[..4], b"\x7fELF", "{}", path.display());
        let wide = b[4] == 2;
        let w = |at: usize| word_at(&b, at, wide);
        // e_phoff, e_shoff, e_phnum, e_shnum, e_shstrndx; phdr and shdr sizes.
        let (phoff, shoff, phnum, shnum, shstrndx, phsz, shsz) = if wide {
            (w(32), w(40), u16_at(&b, 56), u16_at(&b, 60), u16_at(&b, 62), 56, 64)
        } else {
            (w(28), w(32), u16_at(&b, 44), u16_at(&b, 48), u16_at(&b, 50), 32, 40)
        };
        let (phoff, shoff) = (phoff as usize, shoff as usize);
        let interp = (0..phnum as usize).any(|i| u32_at(&b, phoff + phsz * i) == 3);
        let raw: Vec<(u32, u32, u64, u64, u64, u32, u32)> = (0..shnum as usize)
            .map(|i| {
                let h = shoff + shsz * i;
                if wide {
                    let r = (w(h + 16), w(h + 24), w(h + 32));
                    (
                        u32_at(&b, h),
                        u32_at(&b, h + 4),
                        r.0,
                        r.1,
                        r.2,
                        u32_at(&b, h + 40),
                        u32_at(&b, h + 44),
                    )
                } else {
                    let r = (w(h + 12), w(h + 16), w(h + 20));
                    (
                        u32_at(&b, h),
                        u32_at(&b, h + 4),
                        r.0,
                        r.1,
                        r.2,
                        u32_at(&b, h + 24),
                        u32_at(&b, h + 28),
                    )
                }
            })
            .collect();
        let strs = raw[shstrndx as usize].3 as usize;
        let secs = raw
            .iter()
            .map(|&(name, sh_type, addr, offset, size, link, info)| {
                let at = strs + name as usize;
                let end = at + b[at..].iter().position(|&c| c == 0).unwrap();
                Sec { name: b[at..end].to_vec(), sh_type, addr, offset, size, link, info }
            })
            .collect();
        Self { e_type: u16_at(&b, 16), machine: u16_at(&b, 18), secs, interp, wide, bytes: b }
    }

    fn word(&self, at: usize) -> u64 {
        word_at(&self.bytes, at, self.wide)
    }

    fn sec(&self, name: &[u8]) -> Option<&Sec> {
        self.secs.iter().find(|s| s.name == name)
    }

    /// The dynamic relocation section's name (`.rel.dyn` on i386).
    fn rel(&self, plt: bool) -> &'static [u8] {
        match (self.wide, plt) {
            (true, false) => b".rela.dyn",
            (true, true) => b".rela.plt",
            (false, false) => b".rel.dyn",
            (false, true) => b".rel.plt",
        }
    }

    /// `.dynamic`'s entries up to and including the first `DT_NULL`.
    fn dynamic(&self) -> Vec<(i64, u64)> {
        let d = self.sec(b".dynamic").unwrap();
        let ent = if self.wide { 16 } else { 8 };
        let mut out = Vec::new();
        for k in 0..(d.size / ent) as usize {
            let at = d.offset as usize + ent as usize * k;
            let tag = if self.wide {
                u64_at(&self.bytes, at) as i64
            } else {
                i64::from(u32_at(&self.bytes, at) as i32)
            };
            let e = (tag, self.word(at + ent as usize / 2));
            out.push(e);
            if e.0 == DT_NULL {
                break;
            }
        }
        out
    }

    /// `.symtab`'s value of a defined `name`.
    fn symbol(&self, name: &[u8]) -> Option<u64> {
        let st = self.secs.iter().find(|s| s.sh_type == 2)?;
        let strs = &self.secs[st.link as usize];
        // (entsize, st_shndx, st_value) offsets.
        let (ent, shndx_at, value_at) = if self.wide { (24, 6, 8) } else { (16, 14, 4) };
        (0..(st.size / ent) as usize).find_map(|i| {
            let at = st.offset as usize + ent as usize * i;
            let n = strs.offset as usize + u32_at(&self.bytes, at) as usize;
            let end = n + self.bytes[n..].iter().position(|&c| c == 0).unwrap();
            let shndx = u16_at(&self.bytes, at + shndx_at);
            (&self.bytes[n..end] == name && shndx != 0).then(|| self.word(at + value_at))
        })
    }

    /// The `.dynstr` offset of a whole string `s` (NUL before and after).
    fn dynstr_offset(&self, s: &[u8]) -> Option<u32> {
        let d = self.sec(b".dynstr")?;
        let t = &self.bytes[d.offset as usize..(d.offset + d.size) as usize];
        let mut needle = vec![0];
        needle.extend_from_slice(s);
        needle.push(0);
        t.windows(needle.len()).position(|w| w == needle).map(|p| p as u32 + 1)
    }

    /// The number of `RELATIVE` records of `.rela.dyn` (`.rel.dyn`).
    fn relative(&self) -> u64 {
        // (type, entsize, r_info offset, type mask)
        let (rel, ent, info, mask) = match self.machine {
            183 => (1027, 24, 8, 0xffff_ffff),
            62 => (8, 24, 8, 0xffff_ffff),
            3 => (8, 8, 4, 0xff),
            m => panic!("machine {m}"),
        };
        self.sec(self.rel(false)).map_or(0, |s| {
            (0..(s.size / ent) as usize)
                .filter(|&i| self.word(s.offset as usize + ent as usize * i + info) & mask == rel)
                .count() as u64
        })
    }
}

/// The addresses of GNU ld's output, as the image would answer them.
struct GnuAddrs<'a> {
    elf: &'a Elf,
}

impl DynamicAddrs for GnuAddrs<'_> {
    fn chunk(&self, kind: SynthKind) -> Option<(u64, u64)> {
        // Under `-z now` GNU ld merges `.got.plt` into `.got`, first
        // (D-49, a stated difference: mortar keeps the chunk); its words
        // then start `.got`.
        let s = match kind {
            SynthKind::GotPlt => self.elf.sec(b".got.plt").or_else(|| self.elf.sec(b".got")),
            _ => self.elf.sec(kind.name(!self.elf.wide)),
        };
        s.map(|s| (s.addr, s.size))
    }
    fn section(&self, section: ArraySection) -> Option<(u64, u64)> {
        self.elf.sec(section.name()).map(|s| (s.addr, s.size))
    }
    fn symbol(&self, key: SymbolId) -> Option<u64> {
        self.elf.symbol(SYMBOLS.get(key.0 as usize)?)
    }
}

/// A target: the architecture whose tools build it and whose runner runs
/// it (natively on its own host, else under `qemu-<arch> -L <sysroot>`).
struct Target {
    arch: target::Arch,
}

/// AArch64 and x86-64 (M2), and i386 for the ELF32/REL rows (D-30's
/// `DT_REL*`; i386 dynamic output is M4, the writer is checked now).
const TARGETS: &[Target] = &[
    Target { arch: target::Arch::Aarch64 },
    Target { arch: target::Arch::X86_64 },
    Target { arch: target::Arch::I386 },
];

struct Tools {
    readelf: PathBuf,
    elflint: Option<PathBuf>,
}

fn tools() -> Option<Tools> {
    Some(Tools { readelf: tool("llvm-readelf")?, elflint: tool("eu-elflint") })
}

/// The fixture keys: `SymbolId(k)` is `SYMBOLS[k]`.
const SYMBOLS: &[&[u8]] = &[b"", b"_init", b"_fini", b"myinit", b"myfini", b"nosuch"];

/// The key of `name` when GNU ld's output defines it.
fn key(elf: &Elf, name: &[u8]) -> Option<SymbolId> {
    let k = SYMBOLS.iter().position(|&s| s == name).unwrap();
    elf.symbol(name).map(|_| SymbolId(k as u32))
}

/// mortar's `.dynamic` for GNU ld's output `elf` of variant `var`.
fn mortar_dynamic(elf: &Elf, var: &Variant) -> (DynamicPlan, Vec<u8>) {
    let kind = match (elf.e_type, elf.interp) {
        (2, true) => DynKind::Exec,
        (3, true) => DynKind::Pie,
        (3, false) => DynKind::StaticPie,
        other => panic!("{}: e_type/interp {other:?}", var.name),
    };
    let nonempty = |n: &[u8]| elf.sec(n).is_some_and(|s| s.size > 0);
    // The fixture DynStr: the needed names' offsets as GNU ld placed them
    // (D-24's order is WP-M2-DYNSYM's; the count is the needed set).
    let gnu = elf.dynamic();
    let needed: Box<[u32]> =
        gnu.iter().filter(|e| e.0 == mortar_elf::consts::DT_NEEDED).map(|e| e.1 as u32).collect();
    let dynstr = DynStr {
        symbols: Box::new([]),
        needed: needed.clone(),
        versions: Box::new([]),
        runpath: var.runpath.map(|(_, s)| elf.dynstr_offset(s).expect("the run path string")),
        size: elf.sec(b".dynstr").unwrap().size as u32,
    };
    let inputs = DynamicInputs {
        kind,
        class: if elf.wide { Class::Elf64 } else { Class::Elf32 },
        rel_form: if elf.wide { RelForm::Rela } else { RelForm::Rel },
        needed: needed.len() as u32,
        runpath: var.runpath.map(|(t, _)| t),
        init: init_key(var.init.map(|n| key(elf, n)), key(elf, b"_init")),
        fini: init_key(var.fini.map(|n| key(elf, n)), key(elf, b"_fini")),
        preinit_array: elf.sec(b".preinit_array").is_some(),
        init_array: elf.sec(b".init_array").is_some(),
        fini_array: elf.sec(b".fini_array").is_some(),
        // `.got.plt` exists for some PLT or IPLT entry; GNU ld's `-z now`
        // keeps its words at the start of `.got` (D-49).
        got_plt: elf.sec(b".got.plt").is_some() || (var.now && nonempty(elf.rel(true))),
        rela_plt: nonempty(elf.rel(true)),
        rela_dyn: nonempty(elf.rel(false)),
        relative: elf.relative(),
        verneed: elf.sec(b".gnu.version_r").map_or(0, |s| s.info),
        bind_now: var.now,
        arch_tags: if elf.machine == 183 { A64_TAGS } else { &[] },
        bti_property: var.bti,
        variant_pcs_jump_slot: false,
    };
    let plan = DynamicPlan::new(&inputs).unwrap_or_else(|e| panic!("{}: {e:?}", var.name));
    let addrs = GnuAddrs { elf };
    let chunk = DynamicChunk { plan: &plan, dynstr: &dynstr, addrs: &addrs };
    let size = chunk.size(&SizeCx::new());
    let mut out = vec![0xee; size as usize];
    let arch = mortar_a64::A64::M1;
    let pool = mortar_core::par::Pool::sequential();
    let output = mortar_diag::FileRef::new("out");
    let cx = mortar_image::synth::no_answers(&arch, &pool, &output);
    // Two windows, so the split path is the one checked.
    let cut = (size / 2 + 3) as usize;
    let (a, b) = out.split_at_mut(cut);
    chunk.write(&cx, 0..cut as u64, a).unwrap();
    chunk.write(&cx, cut as u64..size, b).unwrap();
    (plan, out)
}

fn entries(bytes: &[u8], wide: bool) -> Vec<(i64, u64)> {
    if wide {
        bytes.chunks(16).map(|c| (u64_at(c, 0) as i64, u64_at(c, 8))).collect()
    } else {
        bytes.chunks(8).map(|c| (i64::from(u32_at(c, 0) as i32), u64::from(u32_at(c, 4)))).collect()
    }
}

/// GNU ld's entries after D-30's stated differences: its `DT_NULL`s and an
/// empty `.rela.dyn`'s three tags removed.
fn expected(elf: &Elf) -> Vec<(i64, u64)> {
    let empty_rela = elf.sec(elf.rel(false)).is_some_and(|s| s.size == 0);
    // DT_RELA..DT_RELAENT, DT_REL..DT_RELENT.
    let rows = if elf.wide { 7..=9 } else { 17..=19 };
    elf.dynamic()
        .into_iter()
        .filter(|e| e.0 != DT_NULL)
        .filter(|e| !(empty_rela && rows.contains(&e.0)))
        .collect()
}

/// Appends `l` and a newline.
fn lines(mut s: String, l: &str) -> String {
    s.push_str(l);
    s.push('\n');
    s
}

fn readelf_d(t: &Tools, path: &Path) -> (String, String) {
    let o = run(&t.readelf, ["-d".as_ref(), path.as_os_str()]);
    assert!(o.success(), "llvm-readelf -d {}: {}", path.display(), o.stderr_text());
    // The listing without the NULL rows and the header line (its count).
    let text: String = o
        .stdout_text()
        .lines()
        .filter(|l| !l.contains("(NULL)") && !l.starts_with("Dynamic section"))
        .fold(String::new(), lines);
    (text, o.stderr_text())
}

fn elflint(t: &Tools, path: &Path) -> Option<String> {
    let lint = t.elflint.as_ref()?;
    let o = run(lint, ["--gnu-ld".as_ref(), "--quiet".as_ref(), path.as_os_str()]);
    Some(format!("{:?} {}", o.success(), o.stdout_text().replace(path.to_str()?, "OUT")))
}

#[test]
fn dynamic_equals_gnu_ld_per_condition() {
    let Some(t) = tools() else {
        assert!(!tools_required(), "llvm-readelf is required");
        return;
    };
    let dir = Scratch::new("wp045-dyn");
    let src = dir.write("t.c", PROGRAM);
    let mut checked = 0;
    let mut failures = Vec::new();
    for target in TARGETS {
        let arch = target.arch.name();
        let Some(cc) = target.arch.tool("gcc") else {
            assert!(!tools_required(), "the {arch} compiler is required");
            continue;
        };
        for var in VARIANTS {
            if arch != "aarch64" && var.bti {
                continue;
            }
            let name = format!("{arch}.{}", var.name);
            let gnu = dir.join(format!("{name}.gnu"));
            let mut args: Vec<&str> = vec!["-O2", "-fuse-ld=bfd", "-Wl,--spare-dynamic-tags=6"];
            args.extend_from_slice(var.flags);
            let (s, g) = (src.to_str().unwrap(), gnu.to_str().unwrap());
            args.extend_from_slice(&[s, "-o", g]);
            let o = run(&cc, &args);
            assert!(o.success(), "{name}: {}", o.stderr_text());
            let elf = Elf::read(&gnu);
            let (plan, bytes) = mortar_dynamic(&elf, var);
            let ours = entries(&bytes, elf.wide);
            let n = plan.entries().len();
            // Row 32: six DT_NULLs.
            assert!(ours[n..].iter().all(|&e| e == (0, 0)) && ours.len() == n + 6, "{name}");
            let want = expected(&elf);
            if ours[..n] != want[..] {
                failures.push(format!("{name}:\n  mortar {:x?}\n  gnu    {want:x?}", &ours[..n]));
                continue;
            }
            // The splice: mortar's table in GNU ld's `.dynamic`.
            let d = elf.sec(b".dynamic").unwrap();
            assert!(bytes.len() as u64 <= d.size, "{name}: {} > {}", bytes.len(), d.size);
            let mut img = elf.bytes.clone();
            let at = d.offset as usize;
            img[at..at + d.size as usize].fill(0);
            img[at..at + bytes.len()].copy_from_slice(&bytes);
            let spliced = dir.join(format!("{name}.mortar"));
            std::fs::write(&spliced, &img).unwrap();
            let (gnu_list, _) = readelf_d(&t, &gnu);
            let (our_list, warnings) = readelf_d(&t, &spliced);
            assert!(warnings.is_empty(), "{name}: llvm-readelf: {warnings}");
            // GNU's listing less an empty `.rela.dyn`'s rows.
            let gnu_list: String = if elf.sec(elf.rel(false)).is_some_and(|s| s.size == 0) {
                gnu_list
                    .lines()
                    .filter(|l| !l.contains("(REL") || l.contains("COUNT"))
                    .fold(String::new(), lines)
            } else {
                gnu_list
            };
            assert_eq!(our_list, gnu_list, "{name}: llvm-readelf -d");
            if let (Some(a), Some(b)) = (elflint(&t, &gnu), elflint(&t, &spliced)) {
                assert_eq!(b, a, "{name}: eu-elflint --gnu-ld");
            }
            // Run it: the loader (or glibc's static-pie start-up) reads it.
            run_program(target, &name, &spliced);
            if let Some(why) = got_header(&elf, var) {
                failures.push(format!("{name}: D-32: {why}"));
            }
            checked += 1;
        }
    }
    assert!(
        failures.is_empty(),
        "{} of the conditions differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
    eprintln!("dynamic_equals_gnu_ld_per_condition: {checked} outputs equal GNU ld's and run");
}

/// D-32 against GNU ld: the word [`reserved_word`] puts `_DYNAMIC` in
/// holds `.dynamic`'s address in GNU ld's output, and the other reserved
/// `.got.plt` words are 0. Under AArch64 `-z now` GNU ld merges `.got.plt`
/// in front of `.got`'s header word, which is then not at a section start:
/// those outputs are skipped. Returns the disagreement.
fn got_header(elf: &Elf, var: &Variant) -> Option<String> {
    let dynamic = elf.sec(b".dynamic")?.addr;
    let facts = match elf.machine {
        183 => mortar_a64::FACTS,
        _ => Facts { got_base: GotBase::GotPltStart, ..mortar_a64::FACTS },
    };
    let word = if elf.wide { 8 } else { 4 };
    let addrs = GnuAddrs { elf };
    for table in [GotTable::Got, GotTable::GotPlt] {
        let (sec, reserved) = match table {
            GotTable::Got if var.now => continue,
            GotTable::Got => (elf.sec(b".got"), facts.got_header),
            GotTable::GotPlt => {
                let at = addrs.chunk(SynthKind::GotPlt)?.0;
                (elf.secs.iter().find(|s| s.addr == at && s.size > 0), 3)
            }
        };
        let Some(sec) = sec else { continue };
        for k in 0..reserved {
            if dynamic_table(&facts) != table && k == 0 && table == GotTable::Got {
                continue;
            }
            let at = sec.offset as usize + usize::from(k) * word;
            let gnu = elf.word(at);
            let ours = reserved_word(&facts, table, k, dynamic);
            if gnu != ours {
                let n = String::from_utf8_lossy(&sec.name);
                return Some(format!("{n}[{k}]: GNU ld {gnu:#x}, mortar {ours:#x}"));
            }
        }
    }
    None
}

fn run_program(target: &Target, name: &str, exe: &Path) {
    let o = run("chmod", ["+x".as_ref(), exe.as_os_str()]);
    assert!(o.success());
    let script = "ulimit -c 0; exec \"$@\"";
    let (program, lead) = target.arch.runner(exe);
    let mut args: Vec<&std::ffi::OsStr> =
        vec!["-c".as_ref(), script.as_ref(), "sh".as_ref(), program.as_os_str()];
    args.extend(lead.iter().map(std::ffi::OsString::as_os_str));
    // The program was written by this process: it can be busy for a
    // moment (`run_exec_with`).
    let o = mortar_testutil::run_exec_with(
        || {
            let mut c = std::process::Command::new("sh");
            c.args(&args);
            c
        },
        &[],
        std::time::Duration::from_mins(1),
    );
    assert!(o.success() && o.stdout_text() == "1\n", "{name}: {:?} {}", o.status, o.stderr_text());
}

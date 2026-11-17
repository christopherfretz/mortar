//! WP-118's oracle for a shared object's `.dynamic` (`spec/dyn.md`
//! D-100, D-102, D-103, D-116), as `dynamic_gnu.rs` is for executables:
//! GNU ld links a small library (`-shared -fPIC`, a constructor, `puts`
//! through the PLT, a preemptible variable through the GOT) for AArch64
//! and x86-64 under each condition of the `-shared` line (`-soname`,
//! `-Bsymbolic`, `-Bsymbolic-functions`, `-z now`, a run path, `-init`/
//! `-fini`, `-z norelro`); mortar's plan (`DynKind::Shared`) is built from
//! the facts of that output D-30 and D-102 read, its values resolved
//! against that output's addresses, the soname's `.dynstr` offset
//! included; and the entries are compared with GNU ld's: tag, order and
//! value. The differences are `dynamic_gnu.rs`'s (an empty `.rela.dyn`'s
//! three tags; `--spare-dynamic-tags=6` for the `DT_NULL`s).
//!
//! Then mortar's bytes replace GNU ld's `.dynamic` in a copy of the
//! library, which is checked by `llvm-readelf -d` (the same listing),
//! `eu-elflint --gnu-ld` (the same verdict) and loaded: a host program
//! `dlopen`s it and calls into it (natively, or under `qemu-<arch>`).

#![allow(clippy::many_single_char_names)]

use std::path::{Path, PathBuf};

use mortar_arch::{DynTag, RelForm, TagWhen};
use mortar_core::DynStr;
use mortar_core::id::SymbolId;
use mortar_elf::Class;
use mortar_elf::consts::{DT_AARCH64_BTI_PLT, DT_AARCH64_VARIANT_PCS, DT_NULL};
use mortar_image::dynamic::{DynamicAddrs, DynamicChunk, fill_records};
use mortar_image::map::{DynRecords, OutputKind};
use mortar_synth::chunks::SynthKind;
use mortar_synth::dynamic::{
    ArraySection, DynKind, DynamicInputs, DynamicPlan, RunpathTag, SharedDyn, shared_init_key,
};
use mortar_synth::image::{SizeCx, SynthChunk};
use mortar_testutil::{Scratch, run, target, tool, tools_required};

const LIBRARY: &str = r"#include <stdio.h>
int counter = 3;
__attribute__((constructor)) static void ctor(void) { counter += 1; }
void myinit(void) { }
void myfini(void) { }
int get(void) { return counter; }
int call_get(void) { return get() + (puts != 0); }
";

/// Loads the library named by its argument and prints `call_get()`.
const HOST: &str = r#"#include <dlfcn.h>
#include <stdio.h>
int main(int c, char **v) {
    void *h = dlopen(v[1], RTLD_NOW);
    if (!h) { printf("%s\n", dlerror()); return 1; }
    int (*f)(void) = (int (*)(void))dlsym(h, "call_get");
    if (!f) { printf("%s\n", dlerror()); return 1; }
    printf("%d\n", f());
    return dlclose(h);
}
"#;

const A64_TAGS: &[DynTag] = &[
    DynTag { tag: DT_AARCH64_BTI_PLT, value: 0, when: TagWhen::BtiProperty },
    DynTag { tag: DT_AARCH64_VARIANT_PCS, value: 0, when: TagWhen::VariantPcsJumpSlot },
];

/// A condition of the `-shared` line: its name, the driver's flags, and
/// the facts mortar's plan takes from the command line.
struct Variant {
    name: &'static str,
    flags: &'static [&'static str],
    now: bool,
    soname: Option<&'static [u8]>,
    /// `SymbolicLine::writes_symbolic_tags()` (D-112).
    symbolic: bool,
    runpath: Option<(RunpathTag, &'static [u8])>,
    init: Option<&'static [u8]>,
    fini: Option<&'static [u8]>,
}

const fn v(name: &'static str, flags: &'static [&'static str]) -> Variant {
    Variant {
        name,
        flags,
        now: false,
        soname: None,
        symbolic: false,
        runpath: None,
        init: None,
        fini: None,
    }
}

const VARIANTS: &[Variant] = &[
    v("plain", &[]),
    Variant { soname: Some(b"libt.so.1"), ..v("soname", &["-Wl,-soname,libt.so.1"]) },
    Variant { symbolic: true, ..v("symbolic", &["-Wl,-Bsymbolic"]) },
    // D-112: `-Bsymbolic-functions` writes no tag.
    v("symbolic-functions", &["-Wl,-Bsymbolic-functions"]),
    Variant { now: true, ..v("now", &["-Wl,-z,now"]) },
    Variant {
        now: true,
        symbolic: true,
        soname: Some(b"libt.so.1"),
        ..v("all-flags", &["-Wl,-z,now", "-Wl,-Bsymbolic", "-Wl,-soname,libt.so.1"])
    },
    Variant {
        soname: Some(b"libt.so.1"),
        runpath: Some((RunpathTag::Runpath, b"/x:$ORIGIN/../lib")),
        ..v(
            "rpath-soname",
            &["-Wl,-soname,libt.so.1", "-Wl,-rpath,/x", "-Wl,-rpath,$ORIGIN/../lib"],
        )
    },
    Variant {
        init: Some(b"myinit"),
        fini: Some(b"myfini"),
        ..v("init-fini", &["-Wl,-init=myinit", "-Wl,-fini=myfini"])
    },
    v("relro-off", &["-Wl,-z,norelro"]),
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

/// The addresses of GNU ld's output, as the image would answer them, and
/// its soname's `.dynstr` offset.
struct GnuAddrs<'a> {
    elf: &'a Elf,
    soname: Option<u32>,
}

impl DynamicAddrs for GnuAddrs<'_> {
    fn chunk(&self, kind: SynthKind) -> Option<(u64, u64)> {
        // Under `-z now` GNU ld merges `.got.plt` into `.got` (D-49).
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
    fn soname(&self) -> Option<u32> {
        self.soname
    }
}

const TARGETS: &[target::Arch] = &[target::Arch::Aarch64, target::Arch::X86_64];

struct Tools {
    readelf: PathBuf,
    elflint: Option<PathBuf>,
}

fn tools() -> Option<Tools> {
    Some(Tools { readelf: tool("llvm-readelf")?, elflint: tool("eu-elflint") })
}

/// The fixture keys: `SymbolId(k)` is `SYMBOLS[k]`.
const SYMBOLS: &[&[u8]] = &[b"", b"_init", b"_fini", b"myinit", b"myfini"];

/// The key of `name` when GNU ld's output defines it.
fn key(elf: &Elf, name: &[u8]) -> Option<SymbolId> {
    let k = SYMBOLS.iter().position(|&s| s == name).unwrap();
    elf.symbol(name).map(|_| SymbolId(k as u32))
}

/// mortar's `.dynamic` for GNU ld's library `elf` of variant `var`.
fn mortar_dynamic(elf: &Elf, var: &Variant) -> (DynamicPlan, Vec<u8>) {
    assert_eq!((elf.e_type, elf.interp), (3, false), "{}: a library", var.name);
    let kind = DynKind::Shared(SharedDyn {
        soname: var.soname.is_some(),
        symbolic: var.symbolic,
        static_tls: false,
    });
    let nonempty = |n: &[u8]| elf.sec(n).is_some_and(|s| s.size > 0);
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
    let init = |named: Option<&'static [u8]>, underscore: &[u8]| {
        shared_init_key(named.map(|n| (n, key(elf, n))), key(elf, underscore)).unwrap()
    };
    let inputs = DynamicInputs {
        kind,
        class: if elf.wide { Class::Elf64 } else { Class::Elf32 },
        rel_form: if elf.wide { RelForm::Rela } else { RelForm::Rel },
        needed: needed.len() as u32,
        runpath: var.runpath.map(|(t, _)| t),
        init: init(var.init, b"_init"),
        fini: init(var.fini, b"_fini"),
        preinit_array: elf.sec(b".preinit_array").is_some(),
        init_array: elf.sec(b".init_array").is_some(),
        fini_array: elf.sec(b".fini_array").is_some(),
        // `.got.plt` exists (its words): under `-z now` GNU ld merges it
        // into `.got` (D-49), and x86-64's keeps it, with its PLT header,
        // when every call goes through `.plt.got` (no `JUMP_SLOT`).
        got_plt: elf.sec(b".got.plt").is_some()
            || (var.now && (nonempty(elf.rel(true)) || elf.sec(b".plt").is_some())),
        rela_plt: nonempty(elf.rel(true)),
        rela_dyn: nonempty(elf.rel(false)),
        relative: elf.relative(),
        verneed: elf.sec(b".gnu.version_r").map_or(0, |s| s.info),
        bind_now: var.now,
        arch_tags: if elf.machine == 183 { A64_TAGS } else { &[] },
        bti_property: false,
        variant_pcs_jump_slot: false,
    };
    let plan = DynamicPlan::new(&inputs).unwrap_or_else(|e| panic!("{}: {e:?}", var.name));
    let soname = var.soname.map(|s| elf.dynstr_offset(s).expect("the soname string"));
    let addrs = GnuAddrs { elf, soname };
    let chunk = DynamicChunk { plan: &plan, dynstr: &dynstr, addrs: &addrs };
    let size = chunk.size(&SizeCx::new());
    let mut out = vec![0xee; size as usize];
    let arch = mortar_a64::A64::M1;
    let pool = mortar_core::par::Pool::sequential();
    let output = mortar_diag::FileRef::new("out");
    let cx = mortar_image::synth::no_answers(&arch, &pool, &output);
    let cut = (size / 2 + 3) as usize;
    let (a, b) = out.split_at_mut(cut);
    chunk.write(&cx, 0..cut as u64, a).unwrap();
    chunk.write(&cx, cut as u64..size, b).unwrap();
    (plan, out)
}

fn entries(bytes: &[u8]) -> Vec<(i64, u64)> {
    bytes.chunks(16).map(|c| (u64_at(c, 0) as i64, u64_at(c, 8))).collect()
}

/// GNU ld's entries less its `DT_NULL`s and an empty `.rela.dyn`'s tags.
fn expected(elf: &Elf) -> Vec<(i64, u64)> {
    let empty_rela = elf.sec(elf.rel(false)).is_some_and(|s| s.size == 0);
    elf.dynamic()
        .into_iter()
        .filter(|e| e.0 != DT_NULL)
        .filter(|e| !(empty_rela && (7..=9).contains(&e.0)))
        .collect()
}

fn lines(mut s: String, l: &str) -> String {
    s.push_str(l);
    s.push('\n');
    s
}

fn readelf_d(t: &Tools, path: &Path) -> (String, String) {
    let o = run(&t.readelf, ["-d".as_ref(), path.as_os_str()]);
    assert!(o.success(), "llvm-readelf -d {}: {}", path.display(), o.stderr_text());
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

// spec: dyn.md D-100 (no DT_DEBUG), D-102 (DT_SONAME, DT_SYMBOLIC, init/fini),
// D-103 (DF_BIND_NOW, DF_1_NOW, no DF_1_PIE), D-116 (the soname's string)
#[test]
fn shared_dynamic_equals_gnu_ld_per_condition() {
    let Some(t) = tools() else {
        assert!(!tools_required(), "llvm-readelf is required");
        return;
    };
    let dir = Scratch::new("wp118-dyn");
    let src = dir.write("t.c", LIBRARY);
    let host_src = dir.write("host.c", HOST);
    let mut checked = 0;
    let mut failures = Vec::new();
    for &arch in TARGETS {
        let an = arch.name();
        let Some(cc) = arch.tool("gcc") else {
            assert!(!tools_required(), "the {an} compiler is required");
            continue;
        };
        let host = dir.join(format!("{an}.host"));
        let o = run(&cc, [host_src.as_os_str(), "-o".as_ref(), host.as_os_str()]);
        assert!(o.success(), "{an} host: {}", o.stderr_text());
        for var in VARIANTS {
            let name = format!("{an}.{}", var.name);
            let gnu = dir.join(format!("{name}.gnu.so"));
            let mut args: Vec<&str> =
                vec!["-O2", "-fPIC", "-shared", "-fuse-ld=bfd", "-Wl,--spare-dynamic-tags=6"];
            args.extend_from_slice(var.flags);
            let (s, g) = (src.to_str().unwrap(), gnu.to_str().unwrap());
            args.extend_from_slice(&[s, "-o", g]);
            let o = run(&cc, &args);
            assert!(o.success(), "{name}: {}", o.stderr_text());
            let elf = Elf::read(&gnu);
            let (plan, bytes) = mortar_dynamic(&elf, var);
            let ours = entries(&bytes);
            let n = plan.entries().len();
            assert!(ours[n..].iter().all(|&e| e == (0, 0)) && ours.len() == n + 6, "{name}");
            let want = expected(&elf);
            if ours[..n] != want[..] {
                failures.push(format!("{name}:\n  mortar {:x?}\n  gnu    {want:x?}", &ours[..n]));
                continue;
            }
            let d = elf.sec(b".dynamic").unwrap();
            assert!(bytes.len() as u64 <= d.size, "{name}: {} > {}", bytes.len(), d.size);
            let mut img = elf.bytes.clone();
            let at = d.offset as usize;
            img[at..at + d.size as usize].fill(0);
            img[at..at + bytes.len()].copy_from_slice(&bytes);
            let spliced = dir.join(format!("{name}.mortar.so"));
            std::fs::write(&spliced, &img).unwrap();
            let (gnu_list, _) = readelf_d(&t, &gnu);
            let (our_list, warnings) = readelf_d(&t, &spliced);
            assert!(warnings.is_empty(), "{name}: llvm-readelf: {warnings}");
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
            load(arch, &name, &host, &spliced);
            checked += 1;
        }
    }
    assert!(
        failures.is_empty(),
        "{} of the conditions differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
    eprintln!(
        "shared_dynamic_equals_gnu_ld_per_condition: {checked} libraries equal GNU ld's and load"
    );
}

/// `dlopen`s `lib` from `host` and checks `call_get()`: the constructor
/// ran (3 + 1) and `puts` was bound (+ 1).
fn load(arch: target::Arch, name: &str, host: &Path, lib: &Path) {
    let (program, mut lead) = arch.runner(host);
    lead.push(lib.as_os_str().to_owned());
    let script = "ulimit -c 0; exec \"$@\"";
    let mut args: Vec<std::ffi::OsString> =
        vec!["-c".into(), script.into(), "sh".into(), program.into_os_string()];
    args.extend(lead);
    let o = mortar_testutil::run_exec_with(
        || {
            let mut c = std::process::Command::new("sh");
            c.args(&args);
            c
        },
        &[],
        std::time::Duration::from_mins(1),
    );
    assert!(
        o.success() && o.stdout_text() == "5\n",
        "{name}: {:?} {} {}",
        o.status,
        o.stdout_text(),
        o.stderr_text()
    );
}

// spec: plumb.md §3.2: a `Shared` kind's map record is `output shared`,
// with no interpreter; the line records are the caller's (`link.rs`).
#[test]
fn shared_map_records() {
    let mut r = DynRecords::default();
    let kind = DynKind::Shared(SharedDyn { soname: true, symbolic: false, static_tls: false });
    fill_records(&mut r, kind, None, Some((RunpathTag::Runpath, b"/x")), true, true);
    assert_eq!(r.kind, Some(OutputKind::Shared));
    assert_eq!(r.interp, None);
    assert_eq!((r.now, r.relro), (Some(true), Some(true)));
    assert!(r.shared.is_none());
}

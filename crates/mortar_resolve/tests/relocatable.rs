//! Resolution at `-r` (`spec/resolve.md` §11, RV-R1..RV-R10; WP-182,
//! RL-2) on real objects: each line is parsed (`mortar_cli::parse`),
//! loaded (`mortar_input::load`) and resolved by
//! `mortar_resolve::relocatable::resolve`. The objects are assembled by
//! the host's GNU as from machine-neutral sources (data and sections, no
//! instructions), so every rule here holds on the three targets.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use mortar_core::par::Pool;
use mortar_core::resolution::{
    Def, DropReason, FileLive, RefFlags, Resolution, SectionKind, SectionRow, SectionState, Verdict,
};
use mortar_core::{FileId, Inputs, SymbolId};
use mortar_diag::Style;
use mortar_elf::consts::{STB_GLOBAL, STB_WEAK, STV_DEFAULT, STV_HIDDEN, STV_PROTECTED};
use mortar_resolve::Outcome;
use mortar_testutil::{Scratch, tool};

/// GNU as for the host's machine.
fn assembler() -> Option<PathBuf> {
    tool(if cfg!(target_arch = "aarch64") { "as" } else { "x86_64-linux-gnu-as" })
}

fn assemble(scratch: &Scratch, name: &str, src: &str) -> Option<PathBuf> {
    let asm = assembler()?;
    let s = scratch.write(&format!("{name}.s"), src);
    let o = scratch.join(format!("{name}.o"));
    let out = Command::new(asm).arg(&s).arg("-o").arg(&o).output().unwrap();
    assert!(out.status.success(), "{name}: {}", String::from_utf8_lossy(&out.stderr));
    Some(o)
}

fn archive(scratch: &Scratch, name: &str, members: &[&Path]) -> Option<PathBuf> {
    let ar = tool(if cfg!(target_arch = "aarch64") { "ar" } else { "x86_64-linux-gnu-ar" })?;
    let a = scratch.join(name);
    let status = Command::new(ar).arg("rcs").arg(&a).args(members).status().unwrap();
    assert!(status.success());
    Some(a)
}

/// Sets the `sh_type` of the section named `name` of an ELF64
/// little-endian object.
fn retype(path: &Path, name: &[u8], sh_type: u32) {
    let mut bytes = std::fs::read(path).unwrap();
    let u16_at = |b: &[u8], o: usize| usize::from(u16::from_le_bytes([b[o], b[o + 1]]));
    let u32_at = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize;
    let u64_at = |b: &[u8], o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap()) as usize;
    let (shoff, shentsize, shnum, shstrndx) =
        (u64_at(&bytes, 0x28), u16_at(&bytes, 0x3a), u16_at(&bytes, 0x3c), u16_at(&bytes, 0x3e));
    let strtab = u64_at(&bytes, shoff + shstrndx * shentsize + 0x18);
    let mut changed = 0;
    for i in 0..shnum {
        let sh = shoff + i * shentsize;
        let at = strtab + u32_at(&bytes, sh);
        if bytes[at..].starts_with(name) && bytes[at + name.len()] == 0 {
            bytes[sh + 4..sh + 8].copy_from_slice(&sh_type.to_le_bytes());
            changed += 1;
        }
    }
    assert_eq!(changed, 1, "one section {}", String::from_utf8_lossy(name));
    std::fs::write(path, bytes).unwrap();
}

/// What a `-r` line gives: the load's and resolution's errors rendered,
/// or the result to look at.
struct Linked<'s> {
    plan: mortar_core::plan::Plan,
    inputs: Inputs<'s>,
    outcome: Outcome,
}

impl Linked<'_> {
    fn t(&self) -> &Resolution {
        &self.outcome.resolution
    }

    fn key(&self, name: &str) -> SymbolId {
        self.inputs
            .symbols
            .iter()
            .find(|(_, k)| k.name == name.as_bytes() && k.version.is_none())
            .map_or_else(|| panic!("no key {name}"), |(id, _)| id)
    }

    fn def(&self, name: &str) -> Def {
        *self.t().def.get(self.key(name)).unwrap()
    }

    fn props(&self, name: &str) -> mortar_core::resolution::SymProps {
        *self.t().props.get(self.key(name)).unwrap()
    }

    /// The file whose path ends with `label` (a member: `lib.a(m.o)`).
    fn file(&self, label: &str) -> FileId {
        self.inputs
            .files
            .iter()
            .find(|(_, f)| {
                let r = f.display();
                let text = match r.member() {
                    Some(m) => format!("{}({m})", r.path()),
                    None => r.path().to_owned(),
                };
                // A thin archive's member is named by its path.
                text.ends_with(label) || text.ends_with(&format!("{label})"))
            })
            .map_or_else(|| panic!("no file {label}"), |(id, _)| id)
    }

    fn live(&self, label: &str) -> bool {
        *self.t().file_live.get(self.file(label)).unwrap() != FileLive::Dead
    }

    /// The row of the section named `name` of the file `label`.
    fn section(&self, label: &str, name: &str) -> SectionRow {
        let file = self.file(label);
        let mortar_core::inputs::Reader::Object(obj) = self.inputs.files[file].reader else {
            panic!("{label} is not an object")
        };
        let index = (0..obj.section_count())
            .find(|&i| {
                obj.section(mortar_elf::SectionIndex(i)).and_then(|s| s.name()).ok()
                    == Some(name.as_bytes())
            })
            .unwrap_or_else(|| panic!("{label} has no {name}"));
        let base = self.t().section_base.get(file).unwrap();
        *self.t().sections.get(mortar_core::SectionId(base.start.0 + index)).unwrap()
    }

    fn errors(&self) -> Vec<String> {
        self.outcome
            .diagnostics
            .iter()
            .filter(|(_, d)| d.severity() == mortar_diag::Severity::Error)
            .map(|(_, d)| d.render(&Style::plain("ld")))
            .collect()
    }

    fn warnings(&self) -> Vec<String> {
        self.outcome
            .diagnostics
            .iter()
            .filter(|(_, d)| d.severity() != mortar_diag::Severity::Error)
            .map(|(_, d)| d.render(&Style::plain("ld")))
            .collect()
    }
}

/// The rendered errors of a line that got through (a function, so it is
/// general over the lifetime).
fn errors_of(l: &Linked<'_>) -> Vec<String> {
    l.errors()
}

/// Parses, loads and resolves `-r LINE` in `dir`, and hands the result
/// to `check`; `Err` holds the rendered errors that stopped it.
fn link_r<T>(
    dir: &Path,
    line: &[&str],
    check: impl FnOnce(&Linked<'_>) -> T,
) -> Result<T, Vec<String>> {
    let mut args = vec![std::ffi::OsString::from("-r")];
    args.extend(line.iter().map(std::ffi::OsString::from));
    let parsed = mortar_cli::parse(&args);
    let render = |d: &mortar_diag::Diagnostic| d.render(&Style::plain("ld"));
    let mut errors: Vec<String> = parsed.diagnostics.iter().map(|(_, d)| render(d)).collect();
    errors.extend(parsed.refusals_landed(&["RL-7"]).iter().map(|(_, d)| render(d)));
    if !errors.is_empty() {
        return Err(errors);
    }
    let plan = parsed.plan.clone();
    assert_eq!(plan.kind, mortar_core::plan::OutputKind::Relocatable);
    let store = mortar_input::store::Store::new(mortar_input::store::StorePolicy::Resident);
    let pool = Pool::new(2);
    let diag = mortar_diag::Collector::new(mortar_diag::Policy::default());
    let env = mortar_input::load::LoadEnv { cwd: dir.to_path_buf(), pool: Pool::new(2) };
    let loaded = mortar_input::load::load(&plan, &env, &store, &diag);
    let load_errors: Vec<String> = diag
        .finish()
        .diagnostics()
        .iter()
        .filter(|d| d.severity() == mortar_diag::Severity::Error)
        .map(render)
        .collect();
    let Some(inputs) = loaded else { return Err(load_errors) };
    if !load_errors.is_empty() {
        return Err(load_errors);
    }
    match mortar_resolve::relocatable::resolve(&inputs, &plan, &pool) {
        Ok(outcome) => Ok(check(&Linked { plan, inputs, outcome })),
        Err(e) => Err(e.iter().map(|(_, d)| render(d)).collect()),
    }
}

/// `.quad SYM` references from a data section; `.globl` definitions.
fn refs(names: &[&str]) -> String {
    let mut s = String::from(".data\n");
    for n in names {
        let _ = writeln!(s, ".quad {n}");
    }
    s
}

fn defs(names: &[&str]) -> String {
    let mut s = String::from(".data\n");
    for n in names {
        let _ = writeln!(s, ".globl {n}\n.type {n},%object\n{n}: .quad 0\n.size {n},8");
    }
    s
}

macro_rules! scratch {
    ($label:literal) => {{
        let scratch = Scratch::new($label);
        if assembler().is_none() {
            return;
        }
        scratch
    }};
}

// spec: resolve.md RV-R1 (no verdict), RV-R3 (undefined keys kept)
#[test]
fn r_no_verdict() {
    let s = scratch!("r-no-verdict");
    assemble(&s, "a", &format!("{}.weak w\n.quad w\n", refs(&["und"]))).unwrap();
    link_r(s.path(), &["a.o", "-o", "out.o"], |l| {
        assert!(l.errors().is_empty(), "{:?}", l.errors());
        assert_eq!(l.def("und"), Def::Undefined { weak: false });
        assert_eq!(l.def("w"), Def::Undefined { weak: true });
        for (name, binding) in [("und", STB_GLOBAL), ("w", STB_WEAK)] {
            let p = l.props(name);
            assert_eq!(p.verdict, Verdict::Bound, "{name}");
            assert_eq!(p.binding, binding, "{name}");
            assert!(p.refs.contains(RefFlags::FROM_OBJECT), "{name}: written (S14)");
        }
        // RV-R1: every key's verdict is `Bound`.
        assert!(l.t().props.iter().all(|(_, p)| p.verdict == Verdict::Bound));
    })
    .unwrap();
}

// spec: resolve.md RV-R3 (`-u SYM` written), cli.md C-R2b
// (`--require-defined`), RV-R2 (`-e` is not a root at `-r`)
#[test]
fn r_u_written() {
    let s = scratch!("r-u-written");
    assemble(&s, "a", &defs(&["here"])).unwrap();
    assemble(&s, "r", &refs(&["yy"])).unwrap();
    // `-u` of a name no input mentions: no key, listed for the writer.
    link_r(s.path(), &["-u", "zz", "-u", "yy", "-u", "zz", "a.o", "r.o", "-o", "out.o"], |l| {
        assert!(l.errors().is_empty(), "{:?}", l.errors());
        assert!(!l.inputs.symbols.iter().any(|(_, k)| k.name == b"zz"));
        let listed = mortar_resolve::relocatable::undefined_without_key(&l.plan, &l.inputs);
        assert_eq!(listed, vec![Box::<[u8]>::from(&b"zz"[..])]);
        // `-u` of a key an input references: an undefined global.
        assert_eq!(l.def("yy"), Def::Undefined { weak: false });
        assert!(l.props("yy").refs.contains(RefFlags::FROM_OBJECT));
        assert_eq!(l.props("yy").binding, STB_GLOBAL);
    })
    .unwrap();
    // `-u` of a weak reference's key: written, global (GNU ld).
    assemble(&s, "w", ".data\n.weak ww\n.quad ww\n").unwrap();
    link_r(s.path(), &["-u", "ww", "a.o", "w.o", "-o", "out.o"], |l| {
        assert!(l.errors().is_empty(), "{:?}", l.errors());
        assert!(l.props("ww").refs.contains(RefFlags::FROM_OBJECT));
    })
    .unwrap();
    // `-u` of a defined key changes nothing.
    link_r(s.path(), &["-u", "here", "a.o", "-o", "out.o"], |l| {
        assert!(matches!(l.def("here"), Def::Defined(_)));
    })
    .unwrap();
    // `-e`: nothing written for it (lld, mold; RV-R2, RV-R6 is `-u`'s).
    link_r(s.path(), &["-e", "zz", "a.o", "-o", "out.o"], |l| {
        assert!(l.errors().is_empty(), "{:?}", l.errors());
        if let Some((id, _)) = l.inputs.symbols.iter().find(|(_, k)| k.name == b"zz") {
            assert!(!l.t().props.get(id).unwrap().refs.contains(RefFlags::FROM_OBJECT));
        }
    })
    .unwrap();
    // `--require-defined` of a key nothing defines: refused, as GNU ld
    // does at `-r` (measured: "required symbol `zz' not defined").
    link_r(s.path(), &["--require-defined=zz", "a.o", "-o", "out.o"], |l| {
        assert_eq!(l.errors().len(), 1, "{:?}", l.errors());
        assert!(l.errors()[0].contains("zz"), "{:?}", l.errors());
    })
    .unwrap();
}

// spec: resolve.md RV-R2 (archives, as in a final link)
#[test]
fn r_archives() {
    let s = scratch!("r-archives");
    let m = assemble(&s, "keep", &defs(&["keep_me"])).unwrap();
    let n = assemble(&s, "other", &defs(&["other"])).unwrap();
    archive(&s, "libo.a", &[&m, &n]).unwrap();
    assemble(&s, "o2", &refs(&["keep_me"])).unwrap();
    assemble(&s, "plain", &defs(&["plain"])).unwrap();
    // r-archive-alone: nothing extracted.
    link_r(s.path(), &["plain.o", "libo.a", "-o", "out.o"], |l| {
        assert!(!l.live("libo.a(keep.o)") && !l.live("libo.a(other.o)"));
    })
    .unwrap();
    // r-archive-u
    link_r(s.path(), &["-u", "keep_me", "plain.o", "libo.a", "-o", "out.o"], |l| {
        assert!(l.live("libo.a(keep.o)") && !l.live("libo.a(other.o)"));
    })
    .unwrap();
    // r-archive-after-ref
    link_r(s.path(), &["o2.o", "libo.a", "-o", "out.o"], |l| {
        assert!(l.live("libo.a(keep.o)") && !l.live("libo.a(other.o)"));
        assert!(matches!(l.def("keep_me"), Def::Defined(_)));
    })
    .unwrap();
    // r-thin-whole-archive, r-start-group
    let ar = tool(if cfg!(target_arch = "aarch64") { "ar" } else { "x86_64-linux-gnu-ar" });
    let Some(ar) = ar else { return };
    let thin = s.join("libthin.a");
    assert!(Command::new(ar).arg("rcsT").arg(&thin).arg(&m).arg(&n).status().unwrap().success());
    link_r(
        s.path(),
        &["plain.o", "--whole-archive", "libthin.a", "--no-whole-archive", "-o", "out.o"],
        |l| assert!(l.live("keep.o") && l.live("other.o")),
    )
    .unwrap();
    assemble(&s, "back", &format!("{}{}", defs(&["back"]), refs(&["other"]))).unwrap();
    archive(&s, "libb.a", &[&s.join("back.o")]).unwrap();
    assemble(&s, "needs_back", &refs(&["back"])).unwrap();
    link_r(
        s.path(),
        &["needs_back.o", "--start-group", "libb.a", "libo.a", "--end-group", "-o", "out.o"],
        |l| assert!(l.live("libb.a(back.o)") && l.live("libo.a(other.o)")),
    )
    .unwrap();
}

// spec: resolve.md RV-R4 (duplicates)
#[test]
fn r_duplicates() {
    let s = scratch!("r-duplicates");
    assemble(&s, "a", &defs(&["dup"])).unwrap();
    assemble(&s, "b", &defs(&["dup"])).unwrap();
    assemble(&s, "w", ".data\n.weak dup\ndup: .quad 1\n").unwrap();
    link_r(s.path(), &["a.o", "b.o", "-o", "out.o"], |l| {
        assert_eq!(l.errors().len(), 1, "{:?}", l.errors());
        assert!(l.errors()[0].contains("duplicate symbol"), "{:?}", l.errors());
    })
    .unwrap();
    link_r(s.path(), &["--allow-multiple-definition", "a.o", "b.o", "-o", "out.o"], |l| {
        assert!(l.errors().is_empty(), "{:?}", l.errors());
    })
    .unwrap();
    // r-weak-strong: the strong one, global.
    link_r(s.path(), &["w.o", "a.o", "-o", "out.o"], |l| {
        assert!(l.errors().is_empty(), "{:?}", l.errors());
        let Def::Defined(e) = l.def("dup") else { panic!("{:?}", l.def("dup")) };
        assert_eq!(e.file, l.file("a.o"));
        assert_eq!(l.props("dup").binding, STB_GLOBAL);
    })
    .unwrap();
}

// spec: resolve.md RV-R5 (commons kept, the maxima)
#[test]
fn r_commons() {
    let s = scratch!("r-commons");
    assemble(&s, "a", ".comm c,8,8\n").unwrap();
    assemble(&s, "b", ".comm c,32,16\n").unwrap();
    link_r(s.path(), &["a.o", "b.o", "-o", "out.o"], |l| {
        assert!(l.errors().is_empty(), "{:?}", l.errors());
        assert!(matches!(l.def("c"), Def::Common(_)), "{:?}", l.def("c"));
        let (_, shape) = l.t().common.iter().find(|(k, _)| *k == l.key("c")).unwrap();
        // `align` is log2: 16.
        assert_eq!((shape.size, shape.align), (32, 4));
        assert_eq!(l.props("c").binding, STB_GLOBAL);
    })
    .unwrap();
}

// spec: resolve.md RV-R6 (visibility merged, hidden stays global)
#[test]
fn r_hidden_stays_global() {
    let s = scratch!("r-hidden");
    let src = format!(
        "{}.hidden h\n.protected p\n.weak wk\n.hidden wk\nwk: .quad 0\n",
        defs(&["h", "p"])
    );
    assemble(&s, "a", &src).unwrap();
    assemble(&s, "b", &format!("{}.hidden r\n", refs(&["r", "h"]))).unwrap();
    link_r(s.path(), &["a.o", "b.o", "-o", "out.o"], |l| {
        assert!(l.errors().is_empty(), "{:?}", l.errors());
        for (name, visibility, binding) in [
            ("h", STV_HIDDEN, STB_GLOBAL),
            ("p", STV_PROTECTED, STB_GLOBAL),
            ("wk", STV_HIDDEN, STB_WEAK),
            ("r", STV_HIDDEN, STB_GLOBAL),
        ] {
            let p = l.props(name);
            assert_eq!((p.visibility, p.binding), (visibility, binding), "{name}");
        }
        assert_eq!(l.props("h").visibility, STV_HIDDEN);
        let _ = STV_DEFAULT;
    })
    .unwrap();
}

// spec: resolve.md RV-R8 (no linker-defined symbol)
#[test]
fn r_no_linker_symbols() {
    let s = scratch!("r-no-linker-symbols");
    let names = ["_end", "__start_mysec", "__stop_mysec", "__bss_start", "_GLOBAL_OFFSET_TABLE_"];
    let src = format!("{}.section mysec,\"aw\",%progbits\n.quad 0\n", refs(&names));
    assemble(&s, "a", &src).unwrap();
    // An object may define a name §8.2 forces in a final link.
    let forced: Vec<&[u8]> = mortar_resolve::linker::forced(if cfg!(target_arch = "aarch64") {
        mortar_elf::Machine::AArch64
    } else {
        mortar_elf::Machine::X86_64
    })
    .filter(|f| !names.iter().any(|n| n.as_bytes() == *f))
    .collect();
    let forced = String::from_utf8(forced[0].to_vec()).unwrap();
    assemble(&s, "f", &defs(&[forced.as_str()])).unwrap();
    link_r(s.path(), &["a.o", "f.o", "-o", "out.o"], |l| {
        assert!(l.errors().is_empty(), "{:?}", l.errors());
        assert!(l.warnings().is_empty(), "{:?}", l.warnings());
        for name in names {
            assert_eq!(l.def(name), Def::Undefined { weak: false }, "{name}");
            assert!(l.props(name).refs.contains(RefFlags::FROM_OBJECT), "{name}");
        }
        assert!(matches!(l.def(&forced), Def::Defined(_)), "{forced}");
    })
    .unwrap();
}

// spec: cli.md C-R5 (a shared object input; no input of a known machine)
#[test]
fn r_inputs_refused() {
    let s = scratch!("r-inputs");
    assemble(&s, "a", &defs(&["x"])).unwrap();
    let ld = tool(if cfg!(target_arch = "aarch64") { "ld" } else { "x86_64-linux-gnu-ld" });
    if let Some(ld) = ld {
        let so = s.join("libx.so");
        assert!(
            Command::new(ld)
                .args(["-shared", "-o"])
                .arg(&so)
                .arg(s.join("a.o"))
                .status()
                .unwrap()
                .success()
        );
        let e = link_r(s.path(), &["a.o", "libx.so", "-o", "out.o"], |_| ()).unwrap_err();
        assert_eq!(e.len(), 1, "{e:?}");
        assert!(e[0].contains("unsupported feature: shared object input with -r"), "{e:?}");
        assert!(e[0].contains("libx.so"), "{e:?}");
    }
    // An archive with no member: no object to take the machine from. The
    // input phase refuses it first, as in a final link (`-m or at least
    // one .o file required`); `resolve` refuses it by C-R5's name if it
    // ever gets that far.
    archive(&s, "libempty.a", &[]).unwrap_or_default();
    if s.join("libempty.a").exists() {
        let e = link_r(s.path(), &["libempty.a", "-o", "out.o"], |_| ()).unwrap_err();
        assert!(
            e.iter().any(|e| e.contains(
                "unsupported feature: relocatable output with no input of a known machine"
            ) || e.contains("-m or at least one .o file required")),
            "{e:?}"
        );
    }
}

// spec: resolve.md RV-R9 (a fat GCC LTO object, a slim one, CREL)
#[test]
fn r_lto_and_crel_refused() {
    let s = scratch!("r-lto");
    assemble(
        &s,
        "fat",
        &format!("{}.section .gnu.lto_main.0,\"e\",%progbits\n.byte 1\n", defs(&["f"])),
    )
    .unwrap();
    assemble(
        &s,
        "slim",
        ".section .gnu.lto_main.0,\"e\",%progbits\n.byte 1\n.comm __gnu_lto_slim,1,1\n",
    )
    .unwrap();
    link_r(s.path(), &["fat.o", "-o", "out.o"], |l| {
        assert_eq!(l.errors().len(), 1, "{:?}", l.errors());
        assert!(
            l.errors()[0].contains("unsupported feature: GCC LTO sections with -r"),
            "{:?}",
            l.errors()
        );
        assert!(l.errors()[0].contains("fat.o"), "{:?}", l.errors());
    })
    .unwrap();
    // A slim one: the input phase's `GCC LTO object`, as in a final link.
    let e = match link_r(s.path(), &["slim.o", "-o", "out.o"], errors_of) {
        Ok(e) | Err(e) => e,
    };
    assert!(e.iter().any(|e| e.contains("GCC LTO object")), "{e:?}");
    assert!(!e.iter().any(|e| e.contains("with -r")), "{e:?}");
    let crel = assemble(
        &s,
        "crel",
        &format!("{}.section .crel.data,\"\",%progbits\n.byte 0\n", defs(&["c"])),
    )
    .unwrap();
    retype(&crel, b".crel.data", 0x4000_0014);
    let e = match link_r(s.path(), &["crel.o", "-o", "out.o"], errors_of) {
        Ok(e) | Err(e) => e,
    };
    assert!(e.iter().any(|e| e.contains("unsupported section type")), "{e:?}");
}

// spec: resolve.md RV-R7 (the election among the `-r`'s inputs; the other
// copies dropped; groups with flag 0 kept)
#[test]
fn r_comdat() {
    let s = scratch!("r-comdat");
    let group = |sig: &str, comdat: bool| {
        format!(
            ".section .data.{sig},\"awG\",%progbits,{sig}{}\n.globl {sig}\n{sig}: .quad 0\n\
             .section .ex.{sig},\"eG\",%progbits,{sig}{}\n.byte 1\n",
            if comdat { ",comdat" } else { "" },
            if comdat { ",comdat" } else { "" },
        )
    };
    assemble(&s, "a", &format!("{}{}", group("f", true), group("g", false))).unwrap();
    assemble(&s, "b", &format!("{}{}", group("f", true), group("g", false))).unwrap();
    link_r(s.path(), &["a.o", "b.o", "-o", "out.o", "--allow-multiple-definition"], |l| {
        assert!(l.errors().is_empty(), "{:?}", l.errors());
        // The first copy is kept, its `SHF_EXCLUDE` member too (S13).
        assert_eq!(l.section("a.o", ".data.f").state, SectionState::Kept);
        assert_eq!(l.section("a.o", ".ex.f").state, SectionState::Kept);
        assert_eq!(l.section("a.o", ".ex.f").kind, SectionKind::Regular);
        assert_eq!(l.section("b.o", ".data.f").state, SectionState::Dropped(DropReason::Comdat));
        assert_eq!(l.section("b.o", ".ex.f").state, SectionState::Dropped(DropReason::Comdat));
        // r-group-flag0-kept: never deduplicated.
        for file in ["a.o", "b.o"] {
            assert_eq!(l.section(file, ".data.g").state, SectionState::Kept, "{file}");
            assert_eq!(l.section(file, ".ex.g").state, SectionState::Kept, "{file}");
        }
    })
    .unwrap();
}

// spec: layout.md L-R1 (what a `-r` copies and drops), rl-sizes.md S13
// (the classification at `-r`); resolve.md §10 (`.gnu.warning*`)
#[test]
fn r_classification() {
    let s = scratch!("r-classification");
    let src = format!(
        "{}{}.section .note.GNU-stack,\"\",%progbits\n\
         .section .ex,\"e\",%progbits\n.byte 1\n\
         .section .exn,\"e\",%nobits\n.zero 4\n\
         .section .gnu.warning.foo,\"\",%progbits\n.asciz \"foo is bad\"\n\
         .section .gnu.warning,\"\",%progbits\n.asciz \"this file is bad\"\n\
         .section .note.gnu.build-id,\"a\",%note\n.long 4,4,3\n.asciz \"GNU\"\n.long 1\n\
         .section .llvm_addrsig,\"e\",%progbits\n.byte 0\n\
         .section .ARM.attributes,\"\",%progbits\n.byte 0x41\n\
         .section .deplibs,\"MS\",%progbits,1\n.asciz \"m\"\n",
        defs(&["foo"]),
        refs(&["foo"]),
    );
    let o = assemble(&s, "a", &src).unwrap();
    retype(&o, b".llvm_addrsig", 0x6fff_4c03);
    retype(&o, b".deplibs", 0x6fff_4c04);
    let aarch64 = cfg!(target_arch = "aarch64");
    if aarch64 {
        retype(&o, b".ARM.attributes", 0x7000_0003);
    }
    assemble(&s, "u", &refs(&["foo"])).unwrap();
    link_r(s.path(), &["a.o", "u.o", "-o", "out.o"], |l| {
        assert!(l.errors().is_empty(), "{:?}", l.errors());
        // The final link warns for `.gnu.warning*`; a `-r` carries them.
        assert!(l.warnings().is_empty(), "{:?}", l.warnings());
        assert!(l.outcome.gnu_warning_syms.is_empty());
        for (name, kind) in [
            (".note.GNU-stack", SectionKind::Regular),
            (".ex", SectionKind::Regular),
            (".exn", SectionKind::Nobits),
            (".gnu.warning.foo", SectionKind::Regular),
            (".gnu.warning", SectionKind::Regular),
            (".note.gnu.build-id", SectionKind::Note),
            (".ARM.attributes", SectionKind::Regular),
            (".deplibs", SectionKind::Regular),
        ] {
            let row = l.section("a.o", name);
            assert_eq!((row.kind, row.state), (kind, SectionState::Kept), "{name}");
        }
        let addrsig = l.section("a.o", ".llvm_addrsig");
        assert_eq!(addrsig.state, SectionState::Dropped(DropReason::Consumed));
        // No row of a live file reaches layout as `Stack`, or as
        // `Excluded` and kept (S13's internal error).
        for (_, row) in l.t().sections.iter() {
            assert!(row.kind != SectionKind::Stack, "{row:?}");
            assert!(!(row.kind == SectionKind::Excluded && row.is_kept()), "{row:?}");
        }
    })
    .unwrap();
}

// spec: resolve.md RV-R6 (versioned names; the key table, WP-182's
// question to the supervisor)
#[test]
fn r_symver_names() {
    let s = scratch!("r-symver");
    let src = ".data\n.globl v1\nv1: .quad 1\n.globl v2\nv2: .quad 2\n\
               .symver v1,vfoo@V1\n.symver v2,vfoo@@V2\n";
    assemble(&s, "v", src).unwrap();
    assemble(&s, "u", &refs(&["vfoo"])).unwrap();
    link_r(s.path(), &["v.o", "u.o", "-o", "out.o"], |l| {
        assert!(l.errors().is_empty(), "{:?}", l.errors());
        // `foo@V1` is a key of its own; `foo@@V2` and the plain `vfoo`
        // reference share the key `vfoo` (spec/model.md §2.3), which the
        // `@@` definition defines (lld's form at `-r`; record §G).
        let v1 = l
            .inputs
            .symbols
            .iter()
            .find(|(_, k)| k.name == b"vfoo" && k.version == Some(b"V1".as_slice()))
            .map(|(id, _)| id)
            .expect("the row vfoo@V1");
        assert!(matches!(*l.t().def.get(v1).unwrap(), Def::Defined(_)));
        let Def::Defined(e) = l.def("vfoo") else { panic!("{:?}", l.def("vfoo")) };
        assert_eq!(e.file, l.file("v.o"));
    })
    .unwrap();
}

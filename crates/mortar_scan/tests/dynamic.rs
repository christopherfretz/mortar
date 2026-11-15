//! `spec/dyn.md` D-18, D-25, D-40-D-49 (WP-046): the scan of an `Exec`
//! or `Pie` with `.dynamic` on AArch64 objects compiled by the system's
//! GCC against shared objects GNU ld builds, and the records the GOT,
//! PLT and copy plans make of its demands, against the dynamic
//! relocations GNU ld and lld write for the same link (`readelf -rW`).
//!
//! The link stops at the scan (the dynamic output is the join's, WP-053):
//! [`dyn_link`] runs the phases before it for real, binds each undefined
//! key a loaded shared object defines to that definition (resolution's
//! `Def::Shared`, which resolution does not make yet: D-11's binding is
//! WP-043's) and scans with [`DynA64`] (D-25's AArch64 table and D-49's
//! lazy target, WP-047's hooks).

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use mortar_arch::{OutputKind, PltVariant, TargetKind};
use mortar_core::assignment::Assignment;
use mortar_core::demands::DemandFlags;
use mortar_core::inputs::{Inputs, Reader};
use mortar_core::layout::Slots;
use mortar_core::par::Pool;
use mortar_core::resolution::Def;
use mortar_core::split::Split;
use mortar_core::{Column, FileId, SymEntry, SymKey, SymbolId};
use mortar_diag::{Collector, Policy};
use mortar_elf::SymbolIndex;
use mortar_elf::consts::{STB_WEAK, STT_FUNC, STT_GNU_IFUNC};
use mortar_input::load::{LoadEnv, load};
use mortar_input::store::{Store, StorePolicy};
use mortar_live::{LiveInputs, LiveOptions, liveness};
use mortar_resolve::Outcome;
use mortar_scan::{ScanInputs, ScanOptions, Scanned, scan};
use mortar_synth::dynsym::DynsymPlan;
use mortar_synth::{DynGotCx, GotPlan, InputCopyDefs, KeyKinds, copy_plan};
use mortar_testutil::Scratch;
use support::dynarch::DynA64;
use support::{A64Proof, compile, mismatches, need, render, run_in};

/// Every dynamic link names its interpreter (input §8.4).
const DYNAMIC_LINKER: [&str; 2] = ["-dynamic-linker", "/lib/ld-linux-aarch64.so.1"];

/// A link up to the scan, with shared definitions bound.
struct DynLinked {
    inputs: Inputs<'static>,
    outcome: Outcome,
    scanned: Scanned,
    output: OutputKind,
}

/// The phases before the scan on `args` (a linker command line), every
/// undefined key a loaded shared object defines bound to it, and the scan
/// of a dynamic `output`.
fn dyn_link(cwd: &Path, args: &[&str], output: OutputKind) -> DynLinked {
    let args: Vec<OsString> = DYNAMIC_LINKER.iter().chain(args).map(OsString::from).collect();
    let plan = mortar_cli::parse(&args).plan;
    let pool = Pool::new(4);
    let store: &'static Store = Box::leak(Box::new(Store::new(StorePolicy::Staged)));
    let diag = Collector::new(Policy::default());
    let env = LoadEnv { cwd: cwd.to_path_buf(), pool };
    let inputs = load(&plan, &env, store, &diag).unwrap_or_else(|| {
        panic!("load: {}", diag.finish().render(&mortar_diag::Style::plain("ld")))
    });
    let mut outcome = mortar_resolve::resolve_inputs(&inputs, &plan, &pool);
    bind_shared(&inputs, &mut outcome);
    let split = Split::default();
    let assignment = Assignment::empty(outcome.resolution.sections.len());
    let live = LiveInputs {
        inputs: &inputs,
        resolution: &outcome.resolution,
        assignment: &assignment,
        split: &split,
        roots: &outcome.roots,
        arch: None,
    };
    let options = LiveOptions {
        gc_sections: plan.shaping.gc_sections,
        keep_exported: plan.shaping.gc_keep_exported,
    };
    let liveness = liveness(&live, options, &pool).expect("liveness");
    let mismatch = mismatches(&outcome);
    let t = ScanInputs {
        inputs: &inputs,
        resolution: &outcome.resolution,
        liveness: &liveness,
        assignment: None,
        comdat_mismatch: &mismatch,
        gnu_warnings: &outcome.gnu_warning_syms,
    };
    let opts = ScanOptions { output, dynamic: true, sections: None, proof: &A64Proof };
    let scanned = scan(&DynA64::default(), &t, &opts, &pool);
    // The same scan on one thread.
    let one = scan(&DynA64::default(), &t, &opts, &Pool::sequential());
    assert_eq!(support::Linked::picture(&one), support::Linked::picture(&scanned));
    DynLinked { inputs, outcome, scanned, output }
}

/// D-11's binding as far as these tests need it: an undefined key that
/// a loaded shared object's `.dynsym` defines is that definition (the
/// first such object in command-line order).
fn bind_shared(inputs: &Inputs<'_>, outcome: &mut Outcome) {
    for (f, row) in inputs.files.iter() {
        let Reader::Shared(so) = row.reader else { continue };
        let Ok(Some(syms)) = so.symbols() else { continue };
        let first = syms.first_global().0;
        for (i, &id) in inputs.sym_map_of(f).iter().enumerate() {
            let index = SymbolIndex(first + i as u32);
            let Ok(sym) = syms.get(index) else { continue };
            if sym.st_shndx == 0 {
                continue;
            }
            if let Some(d) = outcome.resolution.def.get_mut(id)
                && matches!(d, Def::Undefined { .. })
            {
                *d = Def::Shared(SymEntry { file: f, index });
            }
        }
    }
}

impl DynLinked {
    fn name(&self, id: SymbolId) -> String {
        String::from_utf8_lossy(self.inputs.symbols[id].name).into_owned()
    }

    fn ids(&self) -> Vec<&'static str> {
        let mut v: Vec<_> = self.scanned.diagnostics.iter().map(|(k, d)| (*k, d.id())).collect();
        v.sort();
        v.into_iter().map(|x| x.1).collect()
    }

    fn rendered(&self) -> String {
        render(&self.scanned.diagnostics, Policy::default())
    }

    fn id_of(&self, name: &str) -> SymbolId {
        (0..self.inputs.symbols.len())
            .map(SymbolId)
            .find(|&id| self.inputs.symbols[id].name == name.as_bytes())
            .unwrap_or_else(|| panic!("no key {name}"))
    }

    fn weak_dyn(&self) -> BTreeSet<String> {
        let d = &self.scanned.demands;
        (0..self.inputs.symbols.len())
            .map(SymbolId)
            .filter(|&id| d.weak_dyn.contains(id))
            .map(|id| self.name(id))
            .collect()
    }

    fn flags(&self, name: &str) -> DemandFlags {
        self.scanned.demands.of(SymKey::global(self.id_of(name)))
    }

    /// The target kind of a key as the scan saw it.
    fn kind(&self, key: SymKey) -> TargetKind {
        let Some(id) = key.as_global() else { return TargetKind::Local };
        match self.outcome.resolution.def[id] {
            Def::Shared(e) => {
                let Reader::Shared(so) = self.inputs.files[e.file].reader else {
                    return TargetKind::Local;
                };
                let t = so.symbols().ok().flatten().and_then(|s| s.get(e.index).ok());
                let func = t.is_some_and(|s| matches!(s.sym_type(), STT_FUNC | STT_GNU_IFUNC));
                TargetKind::Shlib { func }
            }
            Def::Undefined { .. } if self.scanned.demands.weak_dyn.contains(id) => {
                TargetKind::WeakDyn
            }
            Def::Undefined { weak: true } => TargetKind::Weak0,
            Def::Undefined { weak: false } => TargetKind::Undefined,
            _ => TargetKind::Local,
        }
    }

    /// The dynamic records mortar's plans make of the scan's demands, as
    /// `readelf -rW` names them: per type, the count and the symbol names
    /// (a `RELATIVE` has none).
    fn records(&self) -> Records {
        let d = &self.scanned.demands;
        let arch = DynA64::default();
        let list = |f: DemandFlags| -> Box<[SymKey]> { d.keys_with(f).collect() };
        let slots = Slots {
            got: list(DemandFlags::GOT),
            plt: list(DemandFlags::PLT | DemandFlags::CANON_PLT),
            iplt: list(DemandFlags::IPLT),
            tls_desc: list(DemandFlags::TLS_DESC),
            tls_gd: list(DemandFlags::TLS_GD),
            tls_ie: list(DemandFlags::GOT_TPREL),
            tls_ld: d.output.contains(mortar_core::demands::GlobalFlags::TLS_LD),
            symtab_locals: Column::filled(
                self.outcome.resolution.file_live.len(),
                mortar_core::Span::default(),
            ),
            ..Slots::default()
        };
        // Every global key, in id order: the indices only name the keys.
        let order: Box<[SymbolId]> = (1..self.inputs.symbols.len()).map(SymbolId).collect();
        let dynsym = DynsymPlan::new(order, 0, 1).expect("dynsym");
        let diag = Collector::new(Policy::default());
        let kinds = Kinds(self);
        let cx = DynGotCx { pie: self.output == OutputKind::Pie, bind_now: false };
        let got = GotPlan::new_dynamic(
            &arch,
            &slots,
            d.output,
            PltVariant::Plain,
            cx,
            &kinds,
            &dynsym,
            &diag,
        )
        .unwrap_or_else(|_| panic!("got plan: {}", render_collector(diag)));
        let mut r = Records::default();
        let name_of = |key: Option<SymKey>| -> Option<String> {
            key.and_then(SymKey::as_global).map(|id| self.name(id))
        };
        for g in got.got_records() {
            r.add(type_name(g.r_type), name_of(g.key).filter(|_| g.r_sym != 0));
        }
        for &k in &slots.plt {
            if matches!(self.kind(k), TargetKind::Shlib { .. } | TargetKind::WeakDyn) {
                r.add("JUMP_SLOT", name_of(Some(k)));
            }
        }
        for _ in &slots.iplt {
            r.add("IRELATIVE", None);
        }
        for (_, c) in &d.dyn_relocs {
            use mortar_core::demands::DynKind;
            for _ in 0..c.get(DynKind::Relative) {
                r.add("RELATIVE", None);
            }
            for _ in 0..c.get(DynKind::Symbolic) {
                r.add("ABS64", Some("?".into()));
            }
            for _ in 0..c.get(DynKind::Tls) {
                r.add("TLS", Some("?".into()));
            }
        }
        let copies = copy_plan(
            d,
            &InputCopyDefs { inputs: &self.inputs, resolution: &self.outcome.resolution },
        )
        .expect("copy plan");
        for c in &copies.copies {
            r.add("COPY", Some(self.name(c.alias)));
        }
        r
    }
}

fn render_collector(c: Collector) -> String {
    c.finish().render(&mortar_diag::Style::plain("ld"))
}

struct Kinds<'a>(&'a DynLinked);

impl KeyKinds for Kinds<'_> {
    fn kind(&self, key: SymKey) -> TargetKind {
        self.0.kind(key)
    }
}

/// Per `readelf -rW` type (without `R_AARCH64_`): the count and the
/// symbol names.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Records(BTreeMap<String, (usize, BTreeSet<String>)>);

impl Records {
    fn add(&mut self, ty: &str, sym: Option<String>) {
        let e = self.0.entry(ty.to_owned()).or_default();
        e.0 += 1;
        if let Some(s) = sym {
            e.1.insert(s);
        }
    }

    fn count(&self, ty: &str) -> usize {
        self.0.get(ty).map_or(0, |e| e.0)
    }

    fn names(&self, ty: &str) -> BTreeSet<String> {
        self.0.get(ty).map(|e| e.1.clone()).unwrap_or_default()
    }
}

/// AArch64's dynamic types, by `readelf`'s names.
fn type_name(r_type: u32) -> &'static str {
    match r_type {
        257 => "ABS64",
        1024 => "COPY",
        1025 => "GLOB_DAT",
        1026 => "JUMP_SLOT",
        1027 => "RELATIVE",
        1028 => "TLS_DTPMOD64",
        1029 => "TLS_DTPREL64",
        1030 => "TLS_TPREL64",
        1031 => "TLSDESC",
        1032 => "IRELATIVE",
        _ => "?",
    }
}

/// `readelf -rW out`'s dynamic records.
fn readelf_records(cwd: &Path, out: &Path) -> Records {
    let readelf = need("readelf").expect("readelf");
    let r = run_in(&readelf, &["-rW".into(), out.as_os_str().to_owned()], cwd);
    assert!(r.success(), "readelf: {}", r.stderr_text());
    let mut recs = Records::default();
    for line in r.stdout_text().lines() {
        let w: Vec<&str> = line.split_whitespace().collect();
        if w.len() < 3 || !w[2].starts_with("R_AARCH64_") {
            continue;
        }
        let ty = match &w[2]["R_AARCH64_".len()..] {
            "TLS_TPREL" => "TLS_TPREL64",
            "TLS_DTPREL" => "TLS_DTPREL64",
            "TLS_DTPMOD" => "TLS_DTPMOD64",
            t => t,
        }
        .to_owned();
        let sym = (w.len() >= 5 && w[4] != "+" && ty != "RELATIVE").then(|| w[4].to_owned());
        recs.add(&ty, sym);
    }
    recs
}

/// Links `args` with each reference linker (`ld.bfd`, `ld.lld`) to
/// `out.<ld>`: the records of each (`None`: the linker failed, its
/// stderr).
fn references(
    cwd: &Path,
    args: &[&str],
    out: &str,
) -> Vec<(&'static str, Result<Records, String>)> {
    let mut v = Vec::new();
    for ld in ["ld.bfd", "ld.lld"] {
        let Some(tool) = need(ld) else { continue };
        let o = cwd.join(format!("{out}.{ld}"));
        let mut a: Vec<OsString> = DYNAMIC_LINKER.iter().chain(args).map(OsString::from).collect();
        a.extend(["-o".into(), o.clone().into_os_string()]);
        let r = run_in(&tool, &a, cwd);
        v.push((ld, if r.success() { Ok(readelf_records(cwd, &o)) } else { Err(r.stderr_text()) }));
    }
    v
}

/// A shared object built by GCC and GNU ld from `src` (`-fPIC`, no
/// startfiles).
fn shlib(dir: &Path, name: &str, src: &str) -> Option<PathBuf> {
    let gcc = need("gcc")?;
    let s = dir.join(format!("{name}.c"));
    std::fs::write(&s, src).unwrap();
    let o = dir.join(name);
    let args: Vec<OsString> = ["-shared", "-fPIC", "-O1", "-nostdlib", "-fuse-ld=bfd", "-o"]
        .iter()
        .map(OsString::from)
        .chain([o.clone().into_os_string(), s.into_os_string()])
        .collect();
    let r = run_in(&gcc, &args, dir);
    assert!(r.success(), "shlib {name}: {}", r.stderr_text());
    Some(o)
}

const LIB: &str = r#"
int lib_data = 100;
extern int lib_alias __attribute__((alias("lib_data")));
const int lib_ro[4] = {1, 2, 3, 4};
int lib_zero[0];
__attribute__((visibility("protected"))) int lib_prot = 3;
__thread int lib_tls = 5;
int lib_fn(int x) { return x + 1; }
"#;

/// Compiles `src` (C, `-O1`, `flags`) into `dir/name`.
fn cc(dir: &Path, name: &str, src: &str, flags: &[&str]) -> Option<PathBuf> {
    let mut f = vec!["-O1"];
    f.extend_from_slice(flags);
    compile(dir, "gcc", name, "c", src, &f)
}

/// The setup of the shared object and the tools; `None` without them.
fn setup(label: &str) -> Option<Scratch> {
    need("gcc")?;
    need("readelf")?;
    let dir = Scratch::new(label);
    shlib(dir.path(), "libL.so", LIB)?;
    Some(dir)
}

// ---------------------------------------------------------------------
// Oracle: -fPIE PIE against a shared object, mortar's records per type
// against GNU ld's and lld's.

const PIE_MAIN: &str = r"
extern int lib_data, lib_fn(int);
extern __thread int lib_tls;
static int here = 7;
int *p_here = &here;
int *p_lib = &lib_data;
int *get(void) { return &lib_data; }
int call(void) { return lib_fn(2) + lib_tls; }
";

#[test]
fn d46_pie_records_against_references() {
    let Some(dir) = setup("dyn-oracle-pie") else { return };
    cc(dir.path(), "m.o", PIE_MAIN, &["-fPIE"]).unwrap();
    let l = dyn_link(dir.path(), &["-pie", "m.o", "libL.so", "-o", "out"], OutputKind::Pie);
    assert!(l.scanned.diagnostics.is_empty(), "{}", l.rendered());
    let m = l.records();
    eprintln!("mortar: {m:?}");
    for (ld, r) in references(dir.path(), &["-pie", "m.o", "libL.so"], "out") {
        let r = r.unwrap_or_else(|e| panic!("{ld}: {e}"));
        eprintln!("{ld}: {r:?}");
        for ty in ["GLOB_DAT", "JUMP_SLOT", "TLS_TPREL64", "COPY"] {
            assert_eq!(m.names(ty), r.names(ty), "{ld}: {ty} symbols");
            assert_eq!(m.count(ty), r.count(ty), "{ld}: {ty} count");
        }
        assert_eq!(m.count("ABS64"), r.count("ABS64"), "{ld}: ABS64 count");
        assert_eq!(m.count("RELATIVE"), r.count("RELATIVE"), "{ld}: RELATIVE count");
    }
}

// ---------------------------------------------------------------------
// D-43 / D-42: a `-fno-pic` read of shared data in a PIE and an `Exec` is
// a copy, `.dynbss` or (read-only in the library) `.data.rel.ro`; aliases
// share one allocation.

const NOPIC_DATA: &str = r"
extern int lib_data, lib_alias;
extern const int lib_ro[4];
int get(void) { return lib_data + lib_alias + lib_ro[2]; }
";

/// The section each reference puts `sym`'s copy in, and its size.
fn copy_place(cwd: &Path, out: &Path, sym: &str) -> (String, u64) {
    let readelf = need("readelf").unwrap();
    let syms = run_in(&readelf, &["-sW".into(), out.as_os_str().to_owned()], cwd).stdout_text();
    let secs = run_in(&readelf, &["-SW".into(), out.as_os_str().to_owned()], cwd).stdout_text();
    let (value, size) = syms
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>())
        .find(|w| w.len() >= 8 && w[7] == sym && w[6] != "UND")
        .map_or_else(
            || panic!("{sym} not defined in {}", out.display()),
            |w| (u64::from_str_radix(w[1], 16).unwrap(), w[2].parse::<u64>().unwrap()),
        );
    let sec = secs
        .lines()
        .find_map(|l| {
            let l = l.split_once(']')?.1;
            let w: Vec<&str> = l.split_whitespace().collect();
            let addr = u64::from_str_radix(w.get(2)?, 16).ok()?;
            let size = u64::from_str_radix(w.get(4)?, 16).ok()?;
            (addr != 0 && (addr..addr + size).contains(&value)).then(|| w[0].to_owned())
        })
        .unwrap_or_default();
    (sec, size)
}

#[test]
fn d42_d43_copies_against_references() {
    let Some(dir) = setup("dyn-oracle-copy") else { return };
    cc(dir.path(), "n.o", NOPIC_DATA, &["-fno-pic"]).unwrap();
    for (flag, output) in [("-pie", OutputKind::Pie), ("-no-pie", OutputKind::Exec)] {
        let l = dyn_link(dir.path(), &[flag, "n.o", "libL.so", "-o", "out"], output);
        assert!(l.scanned.diagnostics.is_empty(), "{flag}: {}", l.rendered());
        for k in ["lib_data", "lib_alias", "lib_ro"] {
            assert!(l.flags(k).contains(DemandFlags::COPY), "{flag}: {k} {:?}", l.flags(k));
        }
        let copies = copy_plan(
            &l.scanned.demands,
            &InputCopyDefs { inputs: &l.inputs, resolution: &l.outcome.resolution },
        )
        .unwrap();
        // lib_data and lib_alias: one allocation (D-42), the alias the least id.
        assert_eq!(copies.copies.len(), 2, "{flag}: {copies:?}");
        let alias = l.id_of("lib_data").min(l.id_of("lib_alias"));
        let m = l.records();
        eprintln!("{flag} mortar: {m:?} {copies:?}");
        let mut ours: BTreeMap<String, (String, u64)> = BTreeMap::new();
        for (i, c) in copies.copies.iter().enumerate() {
            let place = match c.place {
                mortar_core::layout::CopyPlace::DynBss => ".dynbss",
                mortar_core::layout::CopyPlace::RelRo => ".data.rel.ro",
            };
            ours.insert(l.name(c.alias), (place.to_owned(), copies.size[i]));
        }
        assert!(ours.contains_key(&l.name(alias)));
        assert_eq!(ours["lib_ro"], (".data.rel.ro".to_owned(), 16), "{flag}");
        assert_eq!(ours[&l.name(alias)].1, 4, "{flag}");
        for (ld, r) in references(dir.path(), &[flag, "n.o", "libL.so"], "out") {
            // GNU ld refuses the AArch64 `-fno-pic` PIE (D-43's finding).
            if ld == "ld.bfd" && flag == "-pie" {
                let e = r.expect_err("GNU ld copies in an AArch64 PIE");
                assert!(e.contains("can not be used when making a shared object"), "{e}");
                continue;
            }
            let r = r.unwrap_or_else(|e| panic!("{ld} {flag}: {e}"));
            eprintln!("{ld} {flag}: {r:?}");
            if ld == "ld.bfd" {
                // GNU ld writes one COPY per key, aliases included, at
                // one address; lld and mortar one per allocation (D-42).
                assert!(r.names("COPY").is_superset(&m.names("COPY")), "{ld} {flag}");
                assert_eq!(r.count("COPY"), 3, "{ld} {flag}");
            } else {
                assert_eq!(m.names("COPY"), r.names("COPY"), "{ld} {flag}: COPY symbols");
                assert_eq!(m.count("COPY"), r.count("COPY"), "{ld} {flag}: COPY count");
            }
            let out = dir.join(format!("out.{ld}"));
            for sym in ["lib_ro", "lib_data"] {
                let (sec, size) = copy_place(dir.path(), &out, sym);
                eprintln!("{ld} {flag}: {sym} in {sec}, size {size}");
                assert_eq!(size, if sym == "lib_ro" { 16 } else { 4 }, "{ld} {flag}: {sym} size");
                if sym == "lib_ro" && ld == "ld.lld" {
                    assert_eq!(sec, ".bss.rel.ro", "{ld} {flag}: {sym}");
                }
                if sym == "lib_data" {
                    assert!(sec == ".dynbss" || sec == ".bss", "{ld} {flag}: {sym} in {sec}");
                }
            }
        }
    }
}

// ---------------------------------------------------------------------
// Refusals by name (diag.md's texts).

/// Links in a PIE and an `Exec`; the refusal ids of each.
fn refusals(dir: &Path, objs: &[&str]) -> (DynLinked, DynLinked) {
    let mut a = vec!["-pie"];
    a.extend_from_slice(objs);
    a.extend(["libL.so", "-o", "out"]);
    let pie = dyn_link(dir, &a, OutputKind::Pie);
    a[0] = "-no-pie";
    let exec = dyn_link(dir, &a, OutputKind::Exec);
    (pie, exec)
}

// D-44: a `-fno-pic` address of a shared function is a canonical PLT in
// an `Exec` and `canonical-plt-in-pie` in a PIE; GNU ld refuses the PIE
// too.
#[test]
fn d44_canonical_plt_in_pie() {
    let Some(dir) = setup("dyn-canon") else { return };
    let src = "extern int lib_fn(int);\nvoid *addr(void) { return (void *)&lib_fn; }\n";
    cc(dir.path(), "c.o", src, &["-fno-pic"]).unwrap();
    let (pie, exec) = refusals(dir.path(), &["c.o"]);
    // One per record (the ADRP and its ADD), as GNU ld reports.
    assert_eq!(pie.ids(), ["canonical-plt-in-pie", "canonical-plt-in-pie"], "{}", pie.rendered());
    assert!(
        pie.rendered().contains(
            "relocation R_AARCH64_ADR_PREL_PG_HI21 against symbol `lib_fn' cannot be used in a position-independent executable; recompile with -fPIE, or link with -no-pie"
        ),
        "{}",
        pie.rendered()
    );
    assert!(exec.scanned.diagnostics.is_empty(), "{}", exec.rendered());
    assert!(exec.flags("lib_fn").contains(DemandFlags::CANON_PLT));
    let m = exec.records();
    for (ld, r) in references(dir.path(), &["-no-pie", "c.o", "libL.so"], "out") {
        let r = r.unwrap_or_else(|e| panic!("{ld}: {e}"));
        assert_eq!(m.names("JUMP_SLOT"), r.names("JUMP_SLOT"), "{ld}");
    }
    let bfd = references(dir.path(), &["-pie", "c.o", "libL.so"], "pie");
    eprintln!("{bfd:?}");
    assert!(bfd[0].1.is_err(), "GNU ld links the canonical PLT in a PIE");
}

// D-47: an address-sized word in a read-only place of a dynamic output is
// `text-relocation` (never `DT_TEXTREL`).
#[test]
fn d47_text_relocation() {
    let Some(dir) = setup("dyn-textrel") else { return };
    let src = "static int x;\nint *const p = &x;\n__attribute__((section(\".rodata.q\"))) int *const q = &x;\n";
    cc(dir.path(), "t.o", src, &["-fno-pic"]).unwrap();
    let (pie, exec) = refusals(dir.path(), &["t.o"]);
    eprintln!("{}", pie.rendered());
    assert!(
        !pie.ids().is_empty() && pie.ids().iter().all(|&i| i == "text-relocation"),
        "{}",
        pie.rendered()
    );
    assert!(pie.rendered().contains(
        "relocation R_AARCH64_ABS64 against symbol `x' can not be used when making a PIE object; recompile with -fPIE"
    ) || pie.rendered().contains("can not be used when making a PIE object"), "{}", pie.rendered());
    // An absolute word against a local in an `Exec` needs no record.
    assert!(exec.scanned.diagnostics.is_empty(), "{}", exec.rendered());
    // A static PIE keeps M1's behaviour.
    let mut st = dyn_link_static_pie(dir.path(), &["-pie", "t.o", "-o", "out"]);
    st.sort_unstable();
    eprintln!("static pie: {st:?}");
    assert!(!st.contains(&"text-relocation"), "{st:?}");
}

/// The scan of a static PIE (`dynamic: false`): the ids.
fn dyn_link_static_pie(cwd: &Path, args: &[&str]) -> Vec<&'static str> {
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    let pool = Pool::sequential();
    let l = support::link(cwd, &args, &pool).expect("load");
    let mismatch = mismatches(&l.outcome);
    let t = ScanInputs {
        inputs: &l.inputs,
        resolution: &l.outcome.resolution,
        liveness: &l.liveness,
        assignment: None,
        comdat_mismatch: &mismatch,
        gnu_warnings: &l.outcome.gnu_warning_syms,
    };
    let opts =
        ScanOptions { output: OutputKind::Pie, dynamic: false, sections: None, proof: &A64Proof };
    let s = scan(&DynA64::default(), &t, &opts, &pool);
    s.diagnostics.iter().map(|(_, d)| d.id()).collect()
}

// D-18 / D-42: a copy of a protected or a zero-size definition is refused
// by name, once per key.
#[test]
fn d18_d42_copy_refusals() {
    let Some(dir) = setup("dyn-copyref") else { return };
    let src = "extern int lib_prot, lib_zero[];\nint get(void) { return lib_prot + lib_zero[0] + lib_prot; }\n";
    cc(dir.path(), "r.o", src, &["-fno-pic"]).unwrap();
    let (pie, exec) = refusals(dir.path(), &["r.o"]);
    for l in [&pie, &exec] {
        let mut ids = l.ids();
        ids.sort_unstable();
        assert_eq!(
            ids,
            ["copy-relocation-against-protected", "copy-relocation-of-zero-size"],
            "{}",
            l.rendered()
        );
        let text = l.rendered();
        assert!(
            text.contains("copy relocation against non-copyable protected symbol `lib_prot' in"),
            "{text}"
        );
        assert!(text.contains("cannot create a copy relocation for symbol lib_zero"), "{text}");
    }
    for (ld, r) in references(dir.path(), &["-no-pie", "r.o", "libL.so"], "out") {
        eprintln!("{ld}: {r:?}");
    }
}

// ---------------------------------------------------------------------
// D-25: the weak classes (the `weak-class-table` case's rows, AArch64).

const WEAK_ROWS: [(&str, &str, &[&str]); 7] = [
    (
        "got",
        "extern void k_got(void) __attribute__((weak));\nvoid *row_got(void) { return (void *)&k_got; }\n",
        &["-fPIE"],
    ),
    (
        "word",
        "extern void k_word(void) __attribute__((weak));\nvoid (*row_word)(void) = k_word;\n",
        &["-fPIE"],
    ),
    (
        "relro",
        "extern void k_relro(void) __attribute__((weak));\nvoid (*const row_relro)(void) = k_relro;\n",
        &["-fPIE"],
    ),
    (
        "branch",
        "extern void k_branch(void) __attribute__((weak));\nvoid row_branch(void) { k_branch(); }\n",
        &["-fPIE"],
    ),
    (
        "gotcall",
        "extern void k_gotcall(void) __attribute__((weak));\nint row_gotcall(void) { if (k_gotcall) { k_gotcall(); return 1; } return 0; }\n",
        &["-fPIE"],
    ),
    (
        "rodata",
        "extern void k_rodata(void) __attribute__((weak));\nvoid (*const row_rodata)(void) = k_rodata;\n",
        &["-fno-pic"],
    ),
    (
        "text",
        "extern void k_text(void) __attribute__((weak));\nvoid *row_text(void) { return (void *)&k_text; }\n",
        &["-fno-pic", "-mcmodel=large"],
    ),
];

#[test]
fn d25_weak_classes() {
    let Some(dir) = setup("dyn-weak") else { return };
    for (row, src, flags) in WEAK_ROWS {
        cc(dir.path(), &format!("{row}.o"), src, flags).unwrap();
    }
    // libL.so: lld makes an undefined weak key dynamic only in a link
    // with a shared object (as every driver's link with libc is).
    let fillable = ["got.o", "word.o", "relro.o", "branch.o", "gotcall.o", "libL.so"];
    let mut a = vec!["-pie"];
    a.extend(fillable);
    a.extend(["-o", "out"]);
    let pie = dyn_link(dir.path(), &a, OutputKind::Pie);
    assert!(pie.scanned.diagnostics.is_empty(), "{}", pie.rendered());
    let want: BTreeSet<String> =
        ["k_got", "k_word", "k_relro", "k_gotcall"].iter().map(|s| (*s).to_owned()).collect();
    assert_eq!(pie.weak_dyn(), want);
    let m = pie.records();
    eprintln!("mortar pie: {m:?}");
    assert_eq!(
        m.names("GLOB_DAT"),
        ["k_got", "k_gotcall"].iter().map(|s| (*s).to_owned()).collect()
    );
    assert_eq!(m.names("JUMP_SLOT"), ["k_gotcall"].iter().map(|s| (*s).to_owned()).collect());
    assert_eq!(m.count("ABS64"), 2);
    // lld agrees on every row but the branch-only key (§13.1: GNU ld and
    // lld make it dynamic with a JUMP_SLOT; mortar Weak0).
    let refs = references(dir.path(), &a[..a.len() - 2], "out");
    for (ld, r) in &refs {
        let r = r.as_ref().unwrap_or_else(|e| panic!("{ld}: {e}"));
        eprintln!("{ld} pie: {r:?}");
        if *ld == "ld.lld" {
            assert_eq!(m.names("GLOB_DAT"), r.names("GLOB_DAT"));
            assert_eq!(m.count("ABS64"), r.count("ABS64"));
            let mut jl = r.names("JUMP_SLOT");
            jl.remove("k_branch");
            assert_eq!(m.names("JUMP_SLOT"), jl);
        }
    }
    // Exec: the unfillable rows are Weak0, no refusal.
    let mut a = vec!["-no-pie"];
    a.extend(fillable);
    a.extend(["rodata.o", "text.o", "-o", "out"]);
    let exec = dyn_link(dir.path(), &a, OutputKind::Exec);
    assert!(exec.scanned.diagnostics.is_empty(), "{}", exec.rendered());
    assert_eq!(exec.weak_dyn(), want);
    // PIE with an unfillable use: refused by name, once per key.
    for row in ["rodata", "text"] {
        let o = format!("{row}.o");
        let pie = dyn_link(dir.path(), &["-pie", "got.o", &o, &o, "-o", "out"], OutputKind::Pie);
        let text = pie.rendered();
        assert_eq!(pie.ids(), ["weak-undefined-pc-in-pie"], "{row}: {text}");
        assert!(text.contains(&format!("against undefined weak symbol `k_{row}' cannot be used in a position-independent executable; recompile with -fPIE")), "{text}");
        assert!(!pie.weak_dyn().contains(&format!("k_{row}")));
    }
    // A thread-local undefined weak key: refused in both.
    let tls =
        "extern __thread int k_tls __attribute__((weak));\nint *row_tls(void) { return &k_tls; }\n";
    cc(dir.path(), "tls.o", tls, &["-fPIE"]).unwrap();
    for (flag, out) in [("-pie", OutputKind::Pie), ("-no-pie", OutputKind::Exec)] {
        let l = dyn_link(dir.path(), &[flag, "tls.o", "-o", "out"], out);
        assert!(l.rendered().contains(mortar_scan::weak::TLS_WEAK), "{flag}: {}", l.rendered());
        assert_eq!(l.scanned.diagnostics.len(), 1, "{flag}: {}", l.rendered());
    }
    // Supervisor ruling 1: an AArch64 -no-pie ADRP/ADD to a Weak0 key
    // follows a64 §7's table: no refusal, no demand.
    // (GCC reaches a weak variable through the GOT even at -fno-pic: the
    // pair is written by hand.)
    let adrp = "\t.weak k_adrp\n\t.text\n\t.globl row_adrp\nrow_adrp:\n\tadrp x0, k_adrp\n\tadd x0, x0, :lo12:k_adrp\n\tret\n";
    support::assemble(dir.path(), "adrp.o", adrp).unwrap();
    let l = dyn_link(dir.path(), &["-no-pie", "adrp.o", "-o", "out"], OutputKind::Exec);
    assert!(l.scanned.diagnostics.is_empty(), "{}", l.rendered());
    assert!(l.flags("k_adrp").is_empty(), "{:?}", l.flags("k_adrp"));
    assert!(l.weak_dyn().is_empty());
    let _ = STB_WEAK;
}

// ---------------------------------------------------------------------
// The `mixed-mode` case (tests/link/dyn/reloc): its refused PIE, which
// refusal comes first.

/// The `#--- name` files of a link-test case.
fn case_files(case: &str) -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/link/dyn/reloc").join(case);
    let text = std::fs::read_to_string(root).unwrap();
    let mut out: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("#--- ") {
            out.push((name.trim().to_owned(), String::new()));
        } else if let Some((_, body)) = out.last_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    out
}

#[test]
fn mixed_mode_refusal_order() {
    let Some(gcc) = need("gcc") else { return };
    let dir = Scratch::new("dyn-mixed");
    for (name, body) in case_files("mixed-mode.test") {
        dir.write(&name, body);
    }
    let c = |args: &[&str]| {
        let a: Vec<OsString> = args.iter().map(OsString::from).collect();
        let r = run_in(&gcc, &a, dir.path());
        assert!(r.success(), "{args:?}: {}", r.stderr_text());
    };
    c(&["-c", "-O2", "-fPIC", "lib.c", "-o", "lib.o"]);
    c(&["-shared", "-fuse-ld=bfd", "lib.o", "-Wl,-soname,libmx.so", "-o", "libmx.so"]);
    c(&["-c", "-O2", "-fPIE", "main.c", "-o", "main.pie.o"]);
    c(&["-c", "-O2", "-fno-pic", "-DTUNAME=tu_a", "tu.c", "-o", "a.nopic.o"]);
    c(&["-c", "-O2", "-fPIC", "-DTUNAME=tu_b", "tu.c", "-o", "b.pic.o"]);
    c(&["-c", "-O2", "-fno-plt", "-DTUNAME=tu_c", "tu.c", "-o", "c.noplt.o"]);
    // libc for puts, stdout, strlen: the one the driver links.
    let libc = run_in(&gcc, &["-print-file-name=libc.so.6".into()], dir.path()).stdout_text();
    let libc = libc.trim().to_owned();
    let libc = if Path::new(&libc).is_absolute() {
        libc
    } else {
        "/lib/aarch64-linux-gnu/libc.so.6".to_owned()
    };
    let args =
        ["-pie", "main.pie.o", "a.nopic.o", "b.pic.o", "c.noplt.o", "libmx.so", &libc, "-o", "out"];
    let l = dyn_link(dir.path(), &args, OutputKind::Pie);
    let text = l.rendered();
    eprintln!("mixed-mode PIE with the -fno-pic unit:\n{text}");
    let ids = l.ids();
    assert!(!ids.is_empty());
    assert_eq!(ids[0], "canonical-plt-in-pie", "{text}");
    assert!(text.lines().next().unwrap_or_default().contains("cannot be used in a position-independent executable; recompile with -fPIE, or link with -no-pie"), "{text}");
    let _ = FileId(0);
}

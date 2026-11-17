//! `spec/dyn.md` D-102, D-103, D-111, D-116, D-120 (WP-118): the
//! synthetic tables of a shared object on AArch64, from hand-made tables:
//! `.dynamic`'s rows, `.dynstr`'s soname, `.dynsym`'s set and forms
//! (against GNU ld's for the same library), and the GOT's records.

mod common;

#[path = "../../mortar_scan/tests/support/dynarch.rs"]
#[allow(dead_code, reason = "a fixture shared with the scan's tests")]
mod dynarch;

use std::collections::BTreeMap;

use common::{collector, text};
use dynarch::DynA64;
use mortar_arch::{DynRole, DynTag, PltVariant, RelForm, TagWhen, TargetKind, TlsBlock};
use mortar_core::demands::GlobalFlags;
use mortar_core::id::SymEntry;
use mortar_core::idset::IdSet;
use mortar_core::par::Pool;
use mortar_core::resolution::{Def, RefFlags};
use mortar_core::{Demands, DynStr, FileId, Resolution, Slots, SymKey, SymbolId, VerIndex};
use mortar_elf::consts::{
    DF_1_NOW, DF_1_PIE, DF_BIND_NOW, DF_SYMBOLIC, DT_AARCH64_BTI_PLT, DT_AARCH64_VARIANT_PCS,
    DT_DEBUG, DT_FINI, DT_FLAGS, DT_FLAGS_1, DT_GNU_HASH, DT_INIT, DT_NEEDED, DT_RUNPATH,
    DT_SONAME, DT_SYMBOLIC, STB_GLOBAL, STB_WEAK, STT_FUNC, STT_GNU_IFUNC, STT_NOTYPE, STT_OBJECT,
    STT_TLS, STV_PROTECTED,
};
use mortar_elf::{Class, Machine, SymbolIndex};
use mortar_synth::SynthKind as K;
use mortar_synth::dynamic::{
    DynKind, DynValue, DynamicInputs, DynamicPlan, RunpathTag, SharedDyn, shared_init_key,
};
use mortar_synth::dynstr::{DynNames, DynstrChunk, dynstr_plan, dynstr_plan_shared};
use mortar_synth::dynsym::{
    DynAddrs, DynSymSource, DynsymChunk, DynsymInputs, DynsymPlan, Form, ProgramDef, SharedDef,
    chunk_sizes, dynsym_plan_shared,
};
use mortar_synth::got::{GotAddrs, SlotValues};
use mortar_synth::out::Window;
use mortar_synth::{GotPlan, KeyKinds};
use mortar_testutil::{Scratch, run, tool};

// ---- D-102, D-103: .dynamic ---------------------------------------------------

const A64_TAGS: &[DynTag] = &[
    DynTag { tag: DT_AARCH64_BTI_PLT, value: 0, when: TagWhen::BtiProperty },
    DynTag { tag: DT_AARCH64_VARIANT_PCS, value: 0, when: TagWhen::VariantPcsJumpSlot },
];

fn inputs(kind: DynKind) -> DynamicInputs {
    DynamicInputs {
        kind,
        class: Class::Elf64,
        rel_form: RelForm::Rela,
        needed: 0,
        runpath: None,
        init: None,
        fini: None,
        preinit_array: false,
        init_array: false,
        fini_array: false,
        got_plt: false,
        rela_plt: false,
        rela_dyn: false,
        relative: 0,
        verneed: 0,
        bind_now: false,
        arch_tags: A64_TAGS,
        bti_property: false,
        variant_pcs_jump_slot: false,
    }
}

fn shared(soname: bool, symbolic: bool) -> DynKind {
    DynKind::Shared(SharedDyn { soname, symbolic })
}

fn tags(i: &DynamicInputs) -> Vec<i64> {
    DynamicPlan::new(i).unwrap().entries().iter().map(|e| e.tag).collect()
}

fn value(i: &DynamicInputs, tag: i64) -> Option<DynValue> {
    DynamicPlan::new(i).unwrap().entries().iter().find(|e| e.tag == tag).map(|e| e.value)
}

// D-102: SONAME then SYMBOLIC right after the DT_NEEDEDs, before the run
// path (GNU ld's order, record §F); no DT_DEBUG; DF_SYMBOLIC with
// DT_SYMBOLIC; D-103: `-z now` gives DF_BIND_NOW and DF_1_NOW, never
// DF_1_PIE.
#[test]
fn d102_d103_rows_in_order() {
    let mut i = inputs(shared(true, true));
    i.needed = 2;
    i.runpath = Some(RunpathTag::Runpath);
    i.init = Some(SymbolId(3));
    i.fini = Some(SymbolId(4));
    i.bind_now = true;
    let t = tags(&i);
    assert_eq!(
        t[..7],
        [DT_NEEDED, DT_NEEDED, DT_SONAME, DT_SYMBOLIC, DT_RUNPATH, DT_INIT, DT_FINI],
        "{t:x?}"
    );
    assert!(!t.contains(&DT_DEBUG));
    assert_eq!(value(&i, DT_SONAME), Some(DynValue::SonameName));
    assert_eq!(value(&i, DT_SYMBOLIC), Some(DynValue::Const(0)));
    assert_eq!(value(&i, DT_FLAGS), Some(DynValue::Const(DF_SYMBOLIC | DF_BIND_NOW)));
    assert_eq!(value(&i, DT_FLAGS_1), Some(DynValue::Const(DF_1_NOW)));
    // FLAGS before FLAGS_1, both after the hash and string rows.
    let at = |tag| t.iter().position(|&x| x == tag).unwrap();
    assert!(at(DT_GNU_HASH) < at(DT_FLAGS) && at(DT_FLAGS) + 1 == at(DT_FLAGS_1));
}

// D-102, D-103: each fact alone; the plain library has neither flags
// entry; `-Bsymbolic-functions` (symbolic false) writes no tag.
#[test]
fn d102_d103_each_fact_alone() {
    let plain = inputs(shared(false, false));
    let t = tags(&plain);
    for tag in [DT_SONAME, DT_SYMBOLIC, DT_FLAGS, DT_FLAGS_1, DT_DEBUG] {
        assert!(!t.contains(&tag), "{tag:#x} in {t:x?}");
    }
    let s = inputs(shared(true, false));
    assert_eq!(tags(&s)[0], DT_SONAME);
    assert!(!tags(&s).contains(&DT_SYMBOLIC));
    let sym = inputs(shared(false, true));
    assert_eq!(tags(&sym)[0], DT_SYMBOLIC);
    assert_eq!(value(&sym, DT_FLAGS), Some(DynValue::Const(DF_SYMBOLIC)));
    assert!(!tags(&sym).contains(&DT_FLAGS_1));
    let mut now = inputs(shared(false, false));
    now.bind_now = true;
    assert_eq!(value(&now, DT_FLAGS), Some(DynValue::Const(DF_BIND_NOW)));
    assert_eq!(value(&now, DT_FLAGS_1), Some(DynValue::Const(DF_1_NOW)));
    // Unchanged for the other kinds: a PIE keeps DT_DEBUG and DF_1_PIE.
    let pie = inputs(DynKind::Pie);
    assert!(tags(&pie).contains(&DT_DEBUG));
    assert_eq!(value(&pie, DT_FLAGS_1), Some(DynValue::Const(DF_1_PIE)));
    assert!(DynKind::Pie.position_independent());
    assert!(shared(false, false).position_independent());
    assert!(!DynKind::Exec.position_independent());
}

// D-102 rows 5-6: `.preinit_array` in a shared object is refused by name.
#[test]
fn d102_preinit_array_refused() {
    let mut i = inputs(shared(false, false));
    i.preinit_array = true;
    let e = DynamicPlan::new(&i).unwrap_err();
    let d = format!("{:?}", e.into_diagnostic());
    assert!(d.contains(".preinit_array in a shared object"), "{d}");
    // A PIE's is written as before.
    let mut p = inputs(DynKind::Pie);
    p.preinit_array = true;
    assert!(DynamicPlan::new(&p).is_ok());
}

// D-102 rows 3-4: `-init=SYM` naming an undefined symbol is refused,
// `undefined-init-symbol`; a defined one wins over `_init`; without the
// option `_init` when defined, nothing when not.
#[test]
fn d102_init_key() {
    let (k, u) = (SymbolId(5), SymbolId(9));
    assert_eq!(shared_init_key(Some((b"my_init", Some(k))), Some(u)).unwrap(), Some(k));
    assert_eq!(shared_init_key(None, Some(u)).unwrap(), Some(u));
    assert_eq!(shared_init_key(None, None).unwrap(), None);
    let e = shared_init_key(Some((b"my_init", None)), Some(u)).unwrap_err();
    let d = format!("{:?}", e.into_diagnostic());
    assert!(d.contains("UndefinedInitSymbol") && d.contains("my_init"), "{d}");
}

// ---- the fixture of .dynstr and .dynsym ----------------------------------------

/// What a key is.
#[derive(Debug, Clone, Copy)]
enum Spec {
    /// A definition here: `st_info`, `st_other`, size; exported or not
    /// (hidden), preemptible or not (protected).
    Def { info: u8, other: u8, size: u64, exported: bool, preemptible: bool },
    /// Undefined: weak or strong; referenced from a live object or not.
    Undef { weak: bool, live: bool },
    /// Defined in needed file 1 and referenced (an import).
    Import { info: u8 },
}

struct Fx {
    names: Vec<Vec<u8>>,
    specs: Vec<Spec>,
    res: Resolution,
    demands: Demands,
    slots: Slots,
}

impl Fx {
    fn new(keys: &[(&str, Spec)]) -> Self {
        let n = keys.len() as u32;
        let mut res = Resolution::empty(2, n, 0);
        res.imported = IdSet::with_len(n);
        res.exported = IdSet::with_len(n);
        res.preemptible = IdSet::with_len(n);
        let mut demands = Demands::empty(n);
        demands.weak_dyn = IdSet::with_len(n);
        let mut needed = Vec::new();
        for (i, (_, spec)) in keys.iter().enumerate() {
            let k = SymbolId(i as u32);
            let (def, ref_flags) = match *spec {
                Spec::Def { exported, preemptible, .. } => {
                    if exported {
                        res.exported.insert(k);
                    }
                    if preemptible {
                        res.preemptible.insert(k);
                    }
                    let e = SymEntry { file: FileId(0), index: SymbolIndex(i as u32 + 1) };
                    (Def::Defined(e), RefFlags::STRONG)
                }
                Spec::Undef { weak, live } => {
                    let mut r = if weak { RefFlags::empty() } else { RefFlags::STRONG };
                    if live {
                        r |= RefFlags::FROM_OBJECT;
                    }
                    (Def::Undefined { weak }, r)
                }
                Spec::Import { .. } => {
                    res.imported.insert(k);
                    needed = vec![FileId(1)];
                    let e = SymEntry { file: FileId(1), index: SymbolIndex(i as u32 + 1) };
                    (Def::Shared(e), RefFlags::STRONG | RefFlags::FROM_OBJECT)
                }
            };
            *res.def.get_mut(k).unwrap() = def;
            res.props.get_mut(k).unwrap().refs.insert(ref_flags);
        }
        let slots = Slots { needed: needed.into(), ..Slots::default() };
        Self {
            names: keys.iter().map(|r| r.0.as_bytes().to_vec()).collect(),
            specs: keys.iter().map(|r| r.1).collect(),
            res,
            demands,
            slots,
        }
    }

    fn plan(&self) -> Result<DynsymPlan, String> {
        let diag = collector();
        let p = dynsym_plan_shared(
            &self.res,
            &self.demands,
            &self.slots,
            self,
            &Pool::sequential(),
            &diag,
        );
        let r = diag.finish();
        p.map_err(|_| text(&r))
    }

    fn inputs(&self) -> DynsymInputs<'_> {
        DynsymInputs {
            resolution: &self.res,
            demands: &self.demands,
            slots: &self.slots,
            src: self,
            shared: true,
        }
    }
}

impl DynNames for Fx {
    fn symbol(&self, key: SymbolId) -> &[u8] {
        &self.names[key.0 as usize]
    }
    fn needed(&self, _file: FileId) -> &[u8] {
        b"libdep.so"
    }
    fn version(&self, _file: FileId, _version: VerIndex) -> &[u8] {
        unreachable!("no versions here")
    }
}

impl DynSymSource for Fx {
    fn shared(&self, entry: SymEntry) -> Option<SharedDef> {
        match self.specs.get(entry.index.0 as usize - 1)? {
            Spec::Import { info } => {
                Some(SharedDef { info: *info, other: 0, value: 0x10, size: 0 })
            }
            _ => None,
        }
    }
    fn program(&self, key: SymbolId) -> Option<ProgramDef<'_>> {
        match self.specs.get(key.0 as usize)? {
            Spec::Def { info, other, size, .. } => {
                Some(ProgramDef { info: *info, other: *other, size: *size, version: None })
            }
            _ => None,
        }
    }
    fn bound_version(&self, _entry: SymEntry) -> Option<VerIndex> {
        None
    }
}

impl DynAddrs for Fx {
    fn defined(&self, key: SymbolId) -> Option<(u32, u64)> {
        matches!(self.specs[key.0 as usize], Spec::Def { .. })
            .then_some((7, 0x1000 + 0x10 * u64::from(key.0)))
    }
    fn plt(&self, _key: SymbolId) -> Option<u64> {
        None
    }
    fn iplt(&self, _key: SymbolId) -> Option<(u32, u64)> {
        None
    }
    fn copy(&self, _alloc: u32) -> Option<(u32, u64)> {
        None
    }
}

fn cstr(t: &[u8], at: u32) -> &[u8] {
    let s = &t[at as usize..];
    &s[..s.iter().position(|&b| b == 0).unwrap()]
}

/// One `.dynsym` entry, as a comparison sees it: name, bind, type,
/// visibility, defined.
type Row = (String, u8, u8, u8, bool);

/// The library of the GNU ld comparison and its keys in mortar's tables.
const LIB_C: &str = "\
int counter = 3;
int *const ptr = &counter;
__attribute__((weak)) int wdef(void) { return 1; }
__attribute__((visibility(\"hidden\"))) int hid(void) { return 2; }
__attribute__((visibility(\"protected\"))) int prot(void) { return 3; }
extern int imported(int);
extern void weakf(void) __attribute__((weak));
int get(void) { return counter + imported(1) + (weakf ? 1 : 0) + hid() + prot() + wdef(); }
";

fn d(info: u8, other: u8, size: u64, exported: bool, preemptible: bool) -> Spec {
    Spec::Def { info, other, size, exported, preemptible }
}

fn lib_keys(sizes: &BTreeMap<String, u64>) -> Vec<(&'static str, Spec)> {
    let sz = |n: &str| sizes.get(n).copied().unwrap_or(4);
    let g = STB_GLOBAL << 4;
    vec![
        ("counter", d(g | STT_OBJECT, 0, sz("counter"), true, true)),
        ("ptr", d(g | STT_OBJECT, 0, sz("ptr"), true, true)),
        ("wdef", d(STB_WEAK << 4 | STT_FUNC, 0, sz("wdef"), true, true)),
        // Hidden: not exported, no entry.
        ("hid", d(g | STT_FUNC, 2, 4, false, false)),
        ("prot", d(g | STT_FUNC, STV_PROTECTED, sz("prot"), true, false)),
        ("imported", Spec::Undef { weak: false, live: true }),
        ("weakf", Spec::Undef { weak: true, live: true }),
        // Referenced by no live object: no entry (D-111).
        ("unused", Spec::Undef { weak: false, live: false }),
        ("get", d(g | STT_FUNC, 0, sz("get"), true, true)),
    ]
}

/// mortar's `.dynsym` of `fx`, written, as rows by name.
fn our_rows(fx: &Fx) -> Vec<Row> {
    let plan = fx.plan().unwrap();
    let (dynstr, soname) = dynstr_plan_shared(&plan, &[], Some(b"libx.so"), &[], None, fx);
    let sizes = chunk_sizes(&plan, &[], &dynstr, Class::Elf64).unwrap();
    let size = |k| sizes.iter().find(|r| r.0 == k).map_or(0, |r| r.1 as usize);
    let mut sym = vec![0xee; size(K::Dynsym)];
    DynsymChunk {
        plan: &plan,
        inputs: fx.inputs(),
        dynstr: &dynstr,
        addrs: fx,
        machine: Machine::AArch64,
    }
    .write_window(&mut Window::new(0, &mut sym))
    .unwrap();
    let mut strs = vec![0xee; size(K::Dynstr)];
    DynstrChunk {
        plan: &plan,
        needed: &[],
        verneed: &[],
        rpath: None,
        soname: soname.map(|o| (&b"libx.so"[..], o)),
        dynstr: &dynstr,
        names: fx,
    }
    .write_all(&mut strs)
    .unwrap();
    let mut rows: Vec<Row> = sym
        .chunks_exact(24)
        .skip(1)
        .map(|e| {
            let name = u32::from_le_bytes(e[0..4].try_into().unwrap());
            let shndx = u16::from_le_bytes(e[6..8].try_into().unwrap());
            let name = String::from_utf8(cstr(&strs, name).to_vec()).unwrap();
            (name, e[4] >> 4, e[4] & 0xf, e[5] & 3, shndx != 0)
        })
        .collect();
    rows.sort();
    rows
}

// D-111: the set and forms of a library's `.dynsym` are GNU ld's for the
// same library: the exports (weak, protected and preemptible alike), a
// strong undefined reference `GLOBAL NOTYPE UND`, a weak one `WEAK NOTYPE
// UND`; no hidden symbol, no undefined key no live object references.
// GNU ld's section symbols are left out (D-110 item 4: mortar writes
// none). Sizes come from GNU ld's output (the fixture's input), the rest
// from mortar's tables.
#[test]
fn d111_dynsym_set_is_gnu_lds() {
    let Some(cc) = tool("gcc") else { return };
    let scratch = Scratch::new("wp118-dynsym");
    let src = scratch.write("lib.c", LIB_C);
    let lib = scratch.join("lib.so");
    let o = run(
        &cc,
        [
            "-O1".as_ref(),
            "-fPIC".as_ref(),
            "-shared".as_ref(),
            "-nostdlib".as_ref(),
            "-fuse-ld=bfd".as_ref(),
            src.as_os_str(),
            "-o".as_ref(),
            lib.as_os_str(),
        ],
    );
    assert!(o.success(), "{}", o.stderr_text());
    let Some(readelf) = tool("readelf") else { return };
    let o = run(&readelf, ["--dyn-syms".as_ref(), "-W".as_ref(), lib.as_os_str()]);
    assert!(o.success(), "{}", o.stderr_text());
    let mut sizes = BTreeMap::new();
    let mut gnu: Vec<Row> = Vec::new();
    for line in o.stdout_text().lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 8
            || f[0].strip_suffix(':').is_none_or(|n| n.parse::<u32>().is_err())
            || f[0] == "0:"
            || f[3] == "SECTION"
        {
            continue;
        }
        let bind = match f[4] {
            "GLOBAL" => STB_GLOBAL,
            "WEAK" => STB_WEAK,
            b => panic!("binding {b}"),
        };
        let typ = match f[3] {
            "FUNC" => STT_FUNC,
            "OBJECT" => STT_OBJECT,
            "NOTYPE" => STT_NOTYPE,
            t => panic!("type {t}"),
        };
        let vis = match f[5] {
            "DEFAULT" => 0,
            "PROTECTED" => STV_PROTECTED,
            v => panic!("visibility {v}"),
        };
        let size = f[2].parse::<u64>().unwrap();
        sizes.insert(f[7].to_owned(), size);
        gnu.push((f[7].to_owned(), bind, typ, vis, f[6] != "UND"));
    }
    gnu.sort();
    let fx = Fx::new(&lib_keys(&sizes));
    assert_eq!(our_rows(&fx), gnu);
    eprintln!("GNU ld: {} .dynsym entries, the same set and forms", gnu.len());
}

// D-111: the forms by key, without a reference tool; the undefined ones
// are unhashed (before the hashed exports, D-22).
#[test]
fn d111_forms() {
    let fx = Fx::new(&lib_keys(&BTreeMap::new()));
    let plan = fx.plan().unwrap();
    let names: Vec<&[u8]> = plan.order().iter().map(|&k| fx.symbol(k)).collect();
    assert_eq!(names[..2], [&b"imported"[..], b"weakf"]);
    let i = fx.inputs();
    let form = |n: u32| i.form(SymbolId(n)).unwrap();
    assert_eq!(form(5), Form::Undefined);
    assert_eq!(form(6), Form::WeakDyn);
    assert_eq!(form(0), Form::Export);
    assert!(!Form::Undefined.hashed());
    assert!(!names.contains(&&b"hid"[..]) && !names.contains(&&b"unused"[..]));
}

// D-111 with an import: a key bound to a needed library's definition is
// `Import`, written `UND` with the definition's type.
#[test]
fn d111_import() {
    let g = STB_GLOBAL << 4;
    let fx = Fx::new(&[
        ("dep_fn", Spec::Import { info: g | STT_FUNC }),
        ("mine", d(g | STT_FUNC, 0, 8, true, true)),
    ]);
    let i = fx.inputs();
    assert_eq!(i.form(SymbolId(0)).unwrap(), Form::Import);
    let rows = our_rows(&fx);
    assert_eq!(
        rows,
        [
            ("dep_fn".into(), STB_GLOBAL, STT_FUNC, 0, false),
            ("mine".into(), STB_GLOBAL, STT_FUNC, 0, true)
        ]
    );
}

// D-121 (until WP-124): an exported IFUNC or TLS definition is refused by
// name.
#[test]
fn d111_ifunc_and_tls_exports_refused() {
    let g = STB_GLOBAL << 4;
    for (typ, what) in
        [(STT_GNU_IFUNC, "exported IFUNC definition sel"), (STT_TLS, "exported TLS definition sel")]
    {
        let fx = Fx::new(&[("sel", d(g | typ, 0, 8, true, true))]);
        let e = fx.plan().unwrap_err();
        assert!(e.contains(what) && e.contains("in a shared object"), "{e}");
    }
}

// ---- D-116: .dynstr ------------------------------------------------------------

// D-116: the soname goes after the needed names (before the version names
// and the run path); a soname equal to an earlier string shares it;
// without one the table is `dynstr_plan`'s.
#[test]
fn d116_soname_after_needed() {
    let fx = Fx::new(&[
        ("dep_fn", Spec::Import { info: STB_GLOBAL << 4 | STT_FUNC }),
        ("mine", d(STB_GLOBAL << 4 | STT_FUNC, 0, 8, true, true)),
    ]);
    let plan = fx.plan().unwrap();
    let needed = [FileId(1)];
    let (t, so) = dynstr_plan_shared(&plan, &needed, Some(b"libx.so.1"), &[], Some(b"/r"), &fx);
    // "\0dep_fn\0mine\0libdep.so\0libx.so.1\0/r\0" (the order of `plan`).
    let syms: usize = plan.order().iter().map(|&k| fx.symbol(k).len() + 1).sum();
    assert_eq!(t.needed[..], [1 + syms as u32]);
    assert_eq!(so, Some(t.needed[0] + 10));
    assert_eq!(t.runpath, so.map(|o| o + 10));
    let mut out = vec![0xee; t.size as usize];
    DynstrChunk {
        plan: &plan,
        needed: &needed,
        verneed: &[],
        rpath: Some(b"/r"),
        soname: Some((b"libx.so.1", so.unwrap())),
        dynstr: &t,
        names: &fx,
    }
    .write_all(&mut out)
    .unwrap();
    assert_eq!(cstr(&out, so.unwrap()), b"libx.so.1");
    assert_eq!(cstr(&out, t.needed[0]), b"libdep.so");
    assert!(!out.contains(&0xee));
    // A soname equal to a needed name shares its offset; the size is the
    // table's without it.
    let (same, same_at) = dynstr_plan_shared(&plan, &needed, Some(b"libdep.so"), &[], None, &fx);
    assert_eq!(same_at, Some(same.needed[0]));
    let without: DynStr = dynstr_plan(&plan, &needed, &[], None, &fx);
    assert_eq!(same, without);
    assert_eq!(dynstr_plan_shared(&plan, &needed, None, &[], None, &fx), (without, None));
}

// ---- D-120: the GOT --------------------------------------------------------------

fn kk(n: u32) -> SymKey {
    SymKey::global(SymbolId(n))
}

/// 1: preemptible data here; 2: local; 3: shared data; 4: weak-dynamic;
/// 5: a shared function; 6: a preemptible function here.
struct Kinds;

impl KeyKinds for Kinds {
    fn kind(&self, key: SymKey) -> TargetKind {
        match key.as_global().map(|i| i.0) {
            Some(1 | 6) => TargetKind::Preemptible,
            Some(3) => TargetKind::Shlib { func: false },
            Some(4) => TargetKind::WeakDyn,
            Some(5) => TargetKind::Shlib { func: true },
            _ => TargetKind::Local,
        }
    }
}

struct Values;

impl SlotValues for Values {
    fn address(&self, key: SymKey) -> Option<u64> {
        BTreeMap::from([(kk(1), 0x2_0000u64), (kk(2), 0x2_0100), (kk(6), 0x1_0400)])
            .get(&key)
            .copied()
    }
    fn tls(&self) -> Option<TlsBlock> {
        None
    }
    fn dynamic(&self) -> Option<u64> {
        Some(0x3_0000)
    }
}

fn got_slots() -> Slots {
    Slots {
        got: [kk(1), kk(2), kk(3), kk(4)].into(),
        plt: [kk(5), kk(6)].into(),
        ..Slots::default()
    }
}

fn got_plan(slots: &Slots, bind_now: bool) -> Result<GotPlan, String> {
    // `.dynsym` holds 1, 3, 4, 5, 6 as entries 1..=5.
    let dynsym = DynsymPlan::new([1, 3, 4, 5, 6].map(SymbolId).into(), 0, 1).unwrap();
    let diag = collector();
    let p = GotPlan::new_shared(
        &DynA64::default(),
        slots,
        GlobalFlags::empty(),
        PltVariant::Plain,
        bind_now,
        &Kinds,
        &dynsym,
        &diag,
    );
    let r = diag.finish();
    p.map_err(|_| text(&r))
}

fn words(b: &[u8]) -> Vec<u64> {
    b.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect()
}

const AT: GotAddrs = GotAddrs { got: 0x4_0000, got_plt: 0x4_1000, plt: 0x1_0000 };

// D-120: a preemptible definition's GOT slot is a GLOB_DAT against its
// `.dynsym` entry whose place holds 0 (D-46), as a shared or weak one's;
// a local slot a RELATIVE; each PLT entry a JUMP_SLOT, the preemptible
// function's too.
#[test]
fn d120_got_records() {
    let p = got_plan(&got_slots(), false).unwrap();
    let recs: Vec<(u64, DynRole, u32, u32)> =
        p.got_records().iter().map(|r| (r.offset, r.role, r.r_type, r.r_sym)).collect();
    assert_eq!(
        recs,
        [
            (16, DynRole::Relative, 1027, 0),
            (8, DynRole::GlobDat, 1025, 1),
            (24, DynRole::GlobDat, 1025, 2),
            (32, DynRole::GlobDat, 1025, 3),
        ]
    );
    assert_eq!(p.got_relative(), 1);
    let mut got = vec![0xffu8; p.got_size() as usize];
    p.write_got(&AT, &Values, &mut Window::new(0, &mut got)).unwrap();
    assert_eq!(words(&got), [0x3_0000, 0, 0x2_0100, 0, 0]);
    let mut rp = vec![0u8; p.rela_plt_size() as usize];
    p.write_rela_plt(&AT, &Values, &mut Window::new(0, &mut rp)).unwrap();
    let rows: Vec<(u64, u64)> = rp
        .chunks_exact(24)
        .map(|c| {
            let w = words(c);
            (w[0], w[1])
        })
        .collect();
    // R_AARCH64_JUMP_SLOT (0x402) against entries 4 and 5.
    assert_eq!(rows, [(AT.got_plt + 24, 4 << 32 | 0x402), (AT.got_plt + 32, 5 << 32 | 0x402)]);
    // `-z now` gives the same records.
    let now = got_plan(&got_slots(), true).unwrap();
    assert_eq!(now.got_records(), p.got_records());
    assert_eq!(now.rela_plt_size(), p.rela_plt_size());
}

// D-121, D-122 (until WP-124): TLS slots and IPLT entries in a shared
// object are refused by name.
#[test]
fn d120_tls_and_iplt_refused() {
    let tls = Slots { tls_ie: [kk(1)].into(), ..got_slots() };
    let e = got_plan(&tls, false).unwrap_err();
    assert!(e.contains("thread-local storage in a shared object"), "{e}");
    let iplt = Slots { iplt: [kk(2)].into(), ..got_slots() };
    let e = got_plan(&iplt, false).unwrap_err();
    assert!(e.contains("IFUNC (IPLT entries) in a shared object"), "{e}");
}

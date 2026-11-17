//! WP-M2-DYNSYM's oracles (`spec/dyn.md` D-21 to D-27): `.dynsym`,
//! `.gnu.hash`, `.dynstr`, `.gnu.version` and `.gnu.version_r` from
//! fixture tables, against
//!
//! - D-21's set, D-22's order and D-23's fields, row by row;
//! - a Rust port of glibc's `.gnu.hash` lookup (`dl-lookup.c`
//!   `do_lookup_x`: Bloom word, bucket, chain), over generated name sets
//!   of sizes 0, 1, 2, powers of two ± 1 and 10^5;
//! - the reviewer's independent model of D-22/D-26
//!   (`tests/dynsym-models/gnuhash_model.py`, copied from
//!   `review-m2-adv/gnuhash`), run on a section-only ELF of our bytes;
//! - lld's outputs, where D-26 claims equality: the same `.dynsym` keys
//!   give lld's order and lld's `.gnu.hash` byte for byte;
//! - the reviewer's model of D-27 (`tests/dynsym-models/verneed_model.py`)
//!   on our `.gnu.version_r` for the imports of real C++ links.

mod common;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use common::{collector, text};
use mortar_core::demands::DemandFlags;
use mortar_core::id::SymEntry;
use mortar_core::idset::IdSet;
use mortar_core::par::{Pool, check_schedules};
use mortar_core::resolution::{Def, InternalDef, RefFlags};
use mortar_core::{
    CopyAlloc, CopyPlace, Demands, DynStr, FileId, Resolution, Slots, SymbolId, VerIndex,
};
use mortar_elf::SymbolIndex;
use mortar_elf::consts::{
    SHN_ABS, STB_GLOBAL, STB_GNU_UNIQUE, STB_WEAK, STO_AARCH64_VARIANT_PCS, STT_FUNC,
    STT_GNU_IFUNC, STT_NOTYPE, STT_OBJECT, STT_TLS, STV_HIDDEN, STV_PROTECTED,
};
use mortar_elf::{Class, Machine};
use mortar_synth::SynthKind as K;
use mortar_synth::dynstr::{DynNames, DynstrChunk, dynstr_plan};
use mortar_synth::dynsym::{
    DynAddrs, DynSymSource, DynsymChunk, DynsymInputs, DynsymPlan, Form, ProgramDef, SharedDef,
    chunk_sizes, dynsym_plan, gnu_symbols,
};
use mortar_synth::gnuhash::{GnuHashShape, gnu_hash, hashes, write_table};
use mortar_synth::out::Window;
use mortar_synth::version::{Verneed, VerneedChunk, VersymChunk, elf_hash, verneed_plan};
use mortar_testutil::{Scratch, run, tool};

// ---- the fixture -------------------------------------------------------------

/// What a key is in the fixture.
#[derive(Debug, Clone)]
enum Spec {
    /// A shared definition in `file` (`st_info`, `st_other`, value, size),
    /// referenced from an object (`imported`) or not (a copy alias).
    Shared { file: u32, def: SharedDef, referenced: bool, strong: bool, version: u16 },
    /// A program definition, exported or not.
    Program { def: ProgramDef<'static>, exported: bool, internal: bool },
    /// Undefined; `weak_dyn` puts it in `Demands.weak_dyn`.
    Undef { weak: bool, weak_dyn: bool },
}

#[derive(Default)]
struct Fx {
    names: Vec<Vec<u8>>,
    specs: Vec<Spec>,
    flags: BTreeMap<u32, DemandFlags>,
    copies: Vec<CopyAlloc>,
    needed: Vec<(u32, Vec<u8>)>,
    vernames: BTreeMap<(u32, u16), Vec<u8>>,
    // addresses
    defined: BTreeMap<u32, (u32, u64)>,
    plt: BTreeMap<u32, u64>,
    iplt: BTreeMap<u32, (u32, u64)>,
    copy_at: BTreeMap<u32, (u32, u64)>,
}

/// The built tables, a source and an address oracle over them.
struct Built {
    fx: Fx,
    res: Resolution,
    demands: Demands,
    slots: Slots,
}

fn sh(_file: u32, info: u8, value: u64, size: u64) -> SharedDef {
    SharedDef { info, other: 0, value, size }
}

fn pd(info: u8, size: u64) -> ProgramDef<'static> {
    ProgramDef { info, other: 0, size, version: None }
}

impl Fx {
    fn key(&mut self, name: &str, spec: Spec) -> SymbolId {
        self.names.push(name.as_bytes().to_vec());
        self.specs.push(spec);
        SymbolId(self.names.len() as u32 - 1)
    }

    /// A referenced shared definition (an import).
    fn import(&mut self, name: &str, file: u32, def: SharedDef, strong: bool) -> SymbolId {
        self.key(name, Spec::Shared { file, def, referenced: true, strong, version: 0 })
    }

    fn export(&mut self, name: &str, def: ProgramDef<'static>, at: (u32, u64)) -> SymbolId {
        let k = self.key(name, Spec::Program { def, exported: true, internal: false });
        self.defined.insert(k.0, at);
        k
    }

    fn flag(&mut self, k: SymbolId, f: DemandFlags) {
        self.flags.entry(k.0).or_default().insert(f);
    }

    fn entry(k: SymbolId) -> SymEntry {
        SymEntry { file: FileId(0), index: SymbolIndex(k.0 + 1) }
    }

    fn build(self) -> Built {
        let n = self.names.len() as u32;
        let files = 1 + self.needed.iter().map(|r| r.0).max().unwrap_or(0).max(
            self.specs
                .iter()
                .filter_map(|s| match s {
                    Spec::Shared { file, .. } => Some(*file),
                    _ => None,
                })
                .max()
                .unwrap_or(0),
        );
        let mut res = Resolution::empty(files, n, 0);
        res.imported = IdSet::with_len(n);
        res.exported = IdSet::with_len(n);
        let mut demands = Demands::empty(n);
        demands.weak_dyn = IdSet::with_len(n);
        for (i, s) in self.specs.iter().enumerate() {
            let k = SymbolId(i as u32);
            let (def, strong) = match s {
                Spec::Shared { file, referenced, strong, .. } => {
                    if *referenced {
                        res.imported.insert(k);
                    }
                    let e = SymEntry { file: FileId(*file), index: SymbolIndex(i as u32 + 1) };
                    (Def::Shared(e), *strong)
                }
                Spec::Program { exported, internal, .. } => {
                    if *exported {
                        res.exported.insert(k);
                    }
                    let def = if *internal {
                        Def::Internal(InternalDef::Provide { conditional: false })
                    } else {
                        Def::Defined(Fx::entry(k))
                    };
                    (def, true)
                }
                Spec::Undef { weak, weak_dyn } => {
                    if *weak_dyn {
                        demands.weak_dyn.insert(k);
                    }
                    (Def::Undefined { weak: *weak }, !*weak)
                }
            };
            *res.def.get_mut(k).unwrap() = def;
            if strong {
                res.props.get_mut(k).unwrap().refs.insert(RefFlags::STRONG);
            }
        }
        for (&k, &f) in &self.flags {
            *demands.global.get_mut(SymbolId(k)).unwrap() = f;
        }
        let mut copies = self.copies.clone();
        copies.sort();
        let mut copy_of = Vec::new();
        for (i, s) in self.specs.iter().enumerate() {
            if let Spec::Shared { file, def, referenced: true, .. } = s
                && let Ok(at) =
                    copies.binary_search_by_key(&(FileId(*file), def.value), CopyAlloc::key)
            {
                copy_of.push((SymbolId(i as u32), at as u32));
            }
        }
        let slots = Slots {
            needed: self.needed.iter().map(|r| FileId(r.0)).collect(),
            copies: copies.into(),
            copy_of: copy_of.into(),
            ..Slots::default()
        };
        Built { fx: self, res, demands, slots }
    }
}

impl DynNames for Built {
    fn symbol(&self, key: SymbolId) -> &[u8] {
        &self.fx.names[key.0 as usize]
    }
    fn needed(&self, file: FileId) -> &[u8] {
        &self.fx.needed.iter().find(|r| r.0 == file.0).unwrap().1
    }
    fn version(&self, file: FileId, version: VerIndex) -> &[u8] {
        &self.fx.vernames[&(file.0, version.0)]
    }
}

impl DynSymSource for Built {
    fn shared(&self, entry: SymEntry) -> Option<SharedDef> {
        match self.fx.specs.get(entry.index.0 as usize - 1)? {
            Spec::Shared { def, .. } => Some(*def),
            _ => None,
        }
    }
    fn program(&self, key: SymbolId) -> Option<ProgramDef<'_>> {
        match self.fx.specs.get(key.0 as usize)? {
            Spec::Program { def, .. } => Some(*def),
            _ => None,
        }
    }
    fn bound_version(&self, entry: SymEntry) -> Option<VerIndex> {
        match self.fx.specs.get(entry.index.0 as usize - 1)? {
            Spec::Shared { version, .. } => Some(VerIndex(*version)),
            _ => None,
        }
    }
}

impl DynAddrs for Built {
    fn defined(&self, key: SymbolId) -> Option<(u32, u64)> {
        self.fx.defined.get(&key.0).copied()
    }
    fn plt(&self, key: SymbolId) -> Option<u64> {
        self.fx.plt.get(&key.0).copied()
    }
    fn iplt(&self, key: SymbolId) -> Option<(u32, u64)> {
        self.fx.iplt.get(&key.0).copied()
    }
    fn copy(&self, alloc: u32) -> Option<(u32, u64)> {
        self.fx.copy_at.get(&alloc).copied()
    }
}

/// Everything this package writes for one link.
#[derive(Debug, PartialEq, Eq)]
struct Out {
    plan: DynsymPlan,
    verneed: Box<Verneed>,
    dynstr: DynStr,
    dynsym: Vec<u8>,
    gnu_hash: Vec<u8>,
    strtab: Vec<u8>,
    versym: Vec<u8>,
    verneed_bytes: Vec<u8>,
}

impl Built {
    fn inputs(&self) -> DynsymInputs<'_> {
        DynsymInputs {
            resolution: &self.res,
            demands: &self.demands,
            slots: &self.slots,
            src: self,
            shared: false,
        }
    }

    fn plan(&self, pool: &Pool) -> Result<DynsymPlan, String> {
        let diag = collector();
        let p = dynsym_plan(&self.res, &self.demands, &self.slots, self, pool, &diag);
        let r = diag.finish();
        p.map_err(|_| text(&r))
    }

    fn link(&self, machine: Machine, pool: &Pool) -> Result<Out, String> {
        let plan = self.plan(pool)?;
        let diag = collector();
        let needed: Vec<FileId> = self.slots.needed.to_vec();
        let verneed = verneed_plan(&plan, &self.inputs(), &needed, &diag)
            .map_err(|_| text(&diag.finish()))?;
        let dynstr = dynstr_plan(&plan, &needed, &verneed, Some(b"/opt/lib"), self);
        let class = machine.class();
        let sizes = chunk_sizes(&plan, &verneed, &dynstr, class)
            .map_err(|e| format!("{:?}", e.into_diagnostic()))?;
        let size = |k| sizes.iter().find(|r| r.0 == k).map_or(0, |r| r.1 as usize);
        let chunk = DynsymChunk {
            plan: &plan,
            inputs: self.inputs(),
            dynstr: &dynstr,
            addrs: self,
            machine,
        };
        let mut dynsym = vec![0xee; size(K::Dynsym)];
        chunk.write_window(&mut Window::new(0, &mut dynsym)).map_err(|e| format!("{e:?}"))?;
        let shape = GnuHashShape::of(&plan, class);
        let mut gnu = vec![0xee; size(K::GnuHash)];
        write_table(&shape, &hashes(&plan, self, pool), &mut gnu).map_err(|e| format!("{e:?}"))?;
        let mut strtab = vec![0xee; size(K::Dynstr)];
        DynstrChunk {
            plan: &plan,
            needed: &needed,
            verneed: &verneed,
            rpath: Some(b"/opt/lib"),
            soname: None,
            dynstr: &dynstr,
            names: self,
        }
        .write_all(&mut strtab)
        .map_err(|e| format!("{e:?}"))?;
        let mut versym = vec![0xee; size(K::GnuVersion)];
        VersymChunk { plan: &plan, inputs: self.inputs(), verneed: &verneed, class }
            .write_window(&mut Window::new(0, &mut versym))
            .map_err(|e| format!("{e:?}"))?;
        let mut vn = vec![0xee; size(K::GnuVersionR)];
        VerneedChunk { needed: &needed, verneed: &verneed, dynstr: &dynstr, names: self, class }
            .write_all(&mut vn)
            .map_err(|e| format!("{e:?}"))?;
        Ok(Out { plan, verneed, dynstr, dynsym, gnu_hash: gnu, strtab, versym, verneed_bytes: vn })
    }
}

/// One `.dynsym` entry, decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Sym {
    name: u32,
    info: u8,
    other: u8,
    shndx: u16,
    value: u64,
    size: u64,
}

fn syms(dynsym: &[u8]) -> Vec<Sym> {
    dynsym
        .chunks(24)
        .map(|e| Sym {
            name: u32::from_le_bytes(e[0..4].try_into().unwrap()),
            info: e[4],
            other: e[5],
            shndx: u16::from_le_bytes(e[6..8].try_into().unwrap()),
            value: u64::from_le_bytes(e[8..16].try_into().unwrap()),
            size: u64::from_le_bytes(e[16..24].try_into().unwrap()),
        })
        .collect()
}

fn cstr(t: &[u8], at: u32) -> &[u8] {
    let s = &t[at as usize..];
    &s[..s.iter().position(|&b| b == 0).unwrap()]
}

// ---- glibc's lookup ----------------------------------------------------------

/// glibc's `.gnu.hash` lookup (`elf/dl-lookup.c` `do_lookup_x`, the
/// `l_gnu_bitmask` path, ELF64): the Bloom test on word `(h / 64) &
/// (maskwords - 1)` with bits `h & 63` and `(h >> shift) & 63`, then the
/// bucket `h % nbuckets` and its chain, comparing `h` without bit 0 and
/// stopping after a word with bit 0 set; `check_match` then rejects a
/// name mismatch and an entry with value 0 that is not `STT_TLS`.
fn glibc_lookup(gh: &[u8], dynsym: &[u8], dynstr: &[u8], name: &[u8]) -> Option<u32> {
    let w32 = |at: usize| u32::from_le_bytes(gh[at..at + 4].try_into().unwrap());
    let (nbuckets, symoffset, maskwords, shift) = (w32(0), w32(4), w32(8), w32(12));
    assert!(maskwords.is_power_of_two(), "glibc requires a power-of-two bitmask");
    let bloom_at = 16usize;
    let buckets_at = bloom_at + 8 * maskwords as usize;
    let chain_at = buckets_at + 4 * nbuckets as usize;
    let h = gnu_hash(name);
    let word_at = bloom_at + 8 * ((h / 64) & (maskwords - 1)) as usize;
    let word = u64::from_le_bytes(gh[word_at..word_at + 8].try_into().unwrap());
    if (word >> (h & 63)) & (word >> ((h >> shift) & 63)) & 1 == 0 {
        return None;
    }
    let bucket = w32(buckets_at + 4 * (h % nbuckets) as usize);
    if bucket == 0 {
        return None;
    }
    let mut i = bucket;
    loop {
        let h2 = w32(chain_at + 4 * (i - symoffset) as usize);
        if (h2 ^ h) >> 1 == 0 {
            let e = syms(&dynsym[24 * i as usize..24 * (i as usize + 1)])[0];
            let typ = e.info & 0xf;
            if cstr(dynstr, e.name) == name && !(e.value == 0 && typ != STT_TLS) {
                return Some(i);
            }
        }
        if h2 & 1 != 0 {
            return None;
        }
        i += 1;
    }
}

// ---- D-21, D-22, D-23 --------------------------------------------------------

/// A program that has one key of every row of D-23, with names chosen to
/// spread over buckets.
fn every_row() -> (Built, BTreeMap<&'static str, SymbolId>) {
    let mut fx = Fx::default();
    let mut ids = BTreeMap::new();
    fx.needed = vec![(1, b"libc.so.6".to_vec()), (2, b"libm.so.6".to_vec())];
    fx.vernames.insert((1, 3), b"GLIBC_2.17".to_vec());
    fx.vernames.insert((1, 5), b"GLIBC_2.34".to_vec());
    fx.vernames.insert((2, 2), b"GLIBC_2.29".to_vec());
    // 0: a local program definition: never in .dynsym.
    fx.key(
        "helper",
        Spec::Program { def: pd(STB_GLOBAL << 4 | STT_FUNC, 4), exported: false, internal: false },
    );
    let puts = fx.import("puts", 1, sh(1, STB_GLOBAL << 4 | STT_FUNC, 0x1000, 8), true);
    ids.insert("puts", puts);
    let wimp = fx.import("__cxa_finalize", 1, sh(1, STB_WEAK << 4 | STT_FUNC, 0x1100, 8), false);
    ids.insert("__cxa_finalize", wimp);
    let ifn = fx.import("memcpy", 1, sh(1, STB_GLOBAL << 4 | STT_GNU_IFUNC, 0x1200, 8), true);
    ids.insert("memcpy", ifn);
    let tls = fx.import("errno_tls", 1, sh(1, STB_GLOBAL << 4 | STT_TLS, 0x10, 4), true);
    ids.insert("errno_tls", tls);
    let canon = fx.import(
        "exp",
        2,
        SharedDef {
            info: STB_GLOBAL << 4 | STT_FUNC,
            other: STO_AARCH64_VARIANT_PCS,
            value: 0x500,
            size: 16,
        },
        true,
    );
    fx.flag(canon, DemandFlags::CANON_PLT);
    fx.plt.insert(canon.0, 0x40_0400);
    ids.insert("exp", canon);
    // environ and two aliases in libc at 0x9000: only `environ` is referenced.
    let environ = fx.import("environ", 1, sh(1, STB_WEAK << 4 | STT_OBJECT, 0x9000, 8), true);
    let env2 = fx.key(
        "__environ",
        Spec::Shared {
            file: 1,
            def: sh(1, STB_GLOBAL << 4 | STT_OBJECT, 0x9000, 8),
            referenced: false,
            strong: false,
            version: 0,
        },
    );
    let env3 = fx.key(
        "_environ",
        Spec::Shared {
            file: 1,
            def: sh(1, STB_GNU_UNIQUE << 4 | STT_OBJECT, 0x9000, 8),
            referenced: false,
            strong: false,
            version: 0,
        },
    );
    // Another libc datum at another address: not an alias.
    fx.key(
        "stdin",
        Spec::Shared {
            file: 1,
            def: sh(1, STB_GLOBAL << 4 | STT_OBJECT, 0x9100, 8),
            referenced: false,
            strong: false,
            version: 0,
        },
    );
    fx.copies.push(CopyAlloc {
        file: FileId(1),
        value: 0x9000,
        place: CopyPlace::DynBss,
        align: 8,
        alias: environ,
    });
    fx.copy_at.insert(0, (24, 0x42_0000));
    ids.insert("environ", environ);
    ids.insert("__environ", env2);
    ids.insert("_environ", env3);
    let main = fx.export(
        "main",
        ProgramDef {
            info: STB_GLOBAL << 4 | STT_FUNC,
            other: STV_PROTECTED,
            size: 40,
            version: None,
        },
        (13, 0x40_1000),
    );
    ids.insert("main", main);
    let uniq = fx.export("_ZGVZ1fvE1x", pd(STB_GNU_UNIQUE << 4 | STT_OBJECT, 8), (25, 0x43_0000));
    ids.insert("_ZGVZ1fvE1x", uniq);
    let ifx = fx.export("myifunc", pd(STB_GLOBAL << 4 | STT_GNU_IFUNC, 12), (13, 0x40_2000));
    fx.flag(ifx, DemandFlags::IPLT);
    fx.iplt.insert(ifx.0, (14, 0x40_0500));
    ids.insert("myifunc", ifx);
    let abs =
        fx.export("abs_sym", pd(STB_GLOBAL << 4 | STT_NOTYPE, 0), (u32::from(SHN_ABS), 0x1234));
    ids.insert("abs_sym", abs);
    let hook = fx.key("hook", Spec::Undef { weak: true, weak_dyn: true });
    ids.insert("hook", hook);
    // A weak undefined that is Weak0: not in .dynsym.
    fx.key("__gmon_start__", Spec::Undef { weak: true, weak_dyn: false });
    // An EXPORT demand on a shared datum not otherwise imported is an import.
    (fx.build(), ids)
}

// dyn.md D-21, D-22: the set and the order: unhashed undefined entries by
// SymbolId, then the hashed ones by (bucket, SymbolId).
#[test]
fn set_and_order() {
    let (b, ids) = every_row();
    let plan = b.plan(&Pool::sequential()).unwrap();
    let name = |k: SymbolId| String::from_utf8(b.symbol(k).to_vec()).unwrap();
    let order: Vec<String> = plan.order().iter().map(|&k| name(k)).collect();
    let unhashed = &order[..plan.unhashed() as usize];
    assert_eq!(unhashed, ["puts", "__cxa_finalize", "memcpy", "errno_tls", "hook"]);
    let hashed = plan.hashed();
    assert_eq!(hashed.len(), 8);
    assert_eq!(plan.nbuckets(), 2);
    let key = |k: &SymbolId| (gnu_hash(b.symbol(*k)) % 2, *k);
    assert!(hashed.windows(2).all(|w| key(&w[0]) < key(&w[1])), "{order:?}");
    for n in
        ["exp", "environ", "__environ", "_environ", "main", "_ZGVZ1fvE1x", "myifunc", "abs_sym"]
    {
        assert!(hashed.contains(&ids[n]), "{n}");
    }
    for (i, &k) in plan.order().iter().enumerate() {
        assert_eq!(plan.index(k), Some(i as u32 + 1));
    }
    let inputs = b.inputs();
    assert_eq!(inputs.form(ids["environ"]).unwrap(), Form::Copy(0));
    assert_eq!(inputs.form(ids["_environ"]).unwrap(), Form::Copy(0));
    assert_eq!(inputs.form(ids["exp"]).unwrap(), Form::CanonPlt);
    assert_eq!(inputs.form(ids["hook"]).unwrap(), Form::WeakDyn);
    // EI_OSABI (D-23): the exported unique definition.
    assert!(gnu_symbols(&plan, &inputs));
}

// dyn.md D-23: one case per row, on AArch64 and x86-64.
#[test]
fn fields_per_row() {
    for machine in [Machine::AArch64, Machine::X86_64] {
        let (b, ids) = every_row();
        let out = b.link(machine, &Pool::sequential()).unwrap();
        let s = syms(&out.dynsym);
        assert_eq!(s[0], Sym { name: 0, info: 0, other: 0, shndx: 0, value: 0, size: 0 });
        let at = |n: &str| s[out.plan.index(ids[n]).unwrap() as usize];
        let row = |n: &str| {
            let e = at(n);
            assert_eq!(cstr(&out.strtab, e.name), n.as_bytes());
            (e.info, e.other, e.shndx, e.value, e.size)
        };
        let glob = STB_GLOBAL << 4;
        let weak = STB_WEAK << 4;
        assert_eq!(row("puts"), (glob | STT_FUNC, 0, 0, 0, 0));
        assert_eq!(row("__cxa_finalize"), (weak | STT_FUNC, 0, 0, 0, 0));
        assert_eq!(row("memcpy"), (glob | STT_FUNC, 0, 0, 0, 0), "IFUNC import is STT_FUNC");
        assert_eq!(row("errno_tls"), (glob | STT_TLS, 0, 0, 0, 0));
        let pcs = if machine == Machine::AArch64 { STO_AARCH64_VARIANT_PCS } else { 0 };
        assert_eq!(row("exp"), (glob | STT_FUNC, pcs, 0, 0x40_0400, 0), "canonical PLT");
        assert_eq!(row("environ"), (weak | STT_OBJECT, 0, 24, 0x42_0000, 8));
        assert_eq!(row("__environ"), (glob | STT_OBJECT, 0, 24, 0x42_0000, 8));
        assert_eq!(row("_environ"), (glob | STT_OBJECT, 0, 24, 0x42_0000, 8), "unique copy");
        assert_eq!(row("main"), (glob | STT_FUNC, STV_PROTECTED, 13, 0x40_1000, 40));
        assert_eq!(row("_ZGVZ1fvE1x"), (STB_GNU_UNIQUE << 4 | STT_OBJECT, 0, 25, 0x43_0000, 8));
        assert_eq!(row("myifunc"), (glob | STT_FUNC, 0, 14, 0x40_0500, 12), "IFUNC export");
        assert_eq!(row("abs_sym"), (glob | STT_NOTYPE, 0, SHN_ABS, 0x1234, 0));
        assert_eq!(row("hook"), (weak | STT_NOTYPE, 0, 0, 0, 0));
        // No versions: no .gnu.version, no .gnu.version_r.
        assert!(out.versym.is_empty() && out.verneed_bytes.is_empty());
        // glibc finds every hashed name and none of the unhashed ones.
        for &k in out.plan.hashed() {
            let i = glibc_lookup(&out.gnu_hash, &out.dynsym, &out.strtab, b.symbol(k));
            assert_eq!(i, out.plan.index(k), "{:?}", String::from_utf8_lossy(b.symbol(k)));
        }
        for n in ["puts", "hook", "helper", "stdin", "absent"] {
            assert_eq!(glibc_lookup(&out.gnu_hash, &out.dynsym, &out.strtab, n.as_bytes()), None);
        }
    }
}

// dyn.md D-21, D-23, D-27, WP-040 §6: the refusals and the broken tables.
#[test]
fn refusals_by_name() {
    let one = |f: &dyn Fn(&mut Fx)| {
        let mut fx = Fx { needed: vec![(1, b"libc.so.6".to_vec())], ..Fx::default() };
        f(&mut fx);
        fx.build()
    };
    let versioned = one(&|fx| {
        let def = ProgramDef {
            info: STB_GLOBAL << 4 | STT_FUNC,
            other: 0,
            size: 4,
            version: Some(b"V1"),
        };
        fx.export("foo", def, (13, 0x1000));
    });
    let e = versioned.plan(&Pool::sequential()).unwrap_err();
    assert!(e.contains("versioned definition foo@@V1 cannot be exported"), "{e}");
    let hidden = one(&|fx| {
        let def = ProgramDef {
            info: STB_GLOBAL << 4 | STT_FUNC,
            other: STV_HIDDEN,
            size: 4,
            version: None,
        };
        fx.export("h", def, (13, 0x1000));
    });
    assert!(hidden.plan(&Pool::sequential()).unwrap_err().contains("internal"));
    let ifunc = one(&|fx| {
        fx.export("f", pd(STB_GLOBAL << 4 | STT_GNU_IFUNC, 4), (13, 0x1000));
    });
    let e = ifunc.plan(&Pool::sequential()).unwrap_err();
    assert!(e.contains("exported IFUNC f that no relocation names"), "{e}");
    let dynamic = one(&|fx| {
        fx.export("_DYNAMIC", pd(STB_GLOBAL << 4 | STT_OBJECT, 0), (20, 0x1000));
    });
    assert!(dynamic.plan(&Pool::sequential()).unwrap_err().contains("never"));
    let strong = one(&|fx| {
        let k = fx.key("u", Spec::Undef { weak: false, weak_dyn: false });
        fx.flag(k, DemandFlags::EXPORT);
    });
    assert!(strong.plan(&Pool::sequential()).unwrap_err().contains("not a WeakDyn key"));
    let shared_weak_dyn = one(&|fx| {
        let k = fx.import("puts", 1, sh(1, STB_GLOBAL << 4 | STT_FUNC, 0x10, 0), true);
        fx.specs[k.0 as usize] = Spec::Shared {
            file: 1,
            def: sh(1, 0x12, 0x10, 0),
            referenced: true,
            strong: true,
            version: 0,
        };
    });
    // (a Shared key that is also WeakDyn)
    let mut b = shared_weak_dyn;
    b.demands.weak_dyn.insert(SymbolId(0));
    assert!(b.plan(&Pool::sequential()).unwrap_err().contains("WeakDyn and has a shared"));
    // A versioned import bound in an object that is not needed.
    let unneeded = one(&|fx| {
        fx.key(
            "f",
            Spec::Shared {
                file: 3,
                def: sh(3, 0x12, 0x10, 0),
                referenced: true,
                strong: true,
                version: 2,
            },
        );
        fx.vernames.insert((3, 2), b"V2".to_vec());
    });
    let e = unneeded.link(Machine::AArch64, &Pool::sequential()).unwrap_err();
    assert!(
        e.contains("versioned reference to f bound in a shared object that is not needed"),
        "{e}"
    );
    // i386 dynamic output is M4.
    let (b, _) = every_row();
    let e = b.link(Machine::I386, &Pool::sequential()).unwrap_err();
    assert!(e.contains("ELFCLASS32"), "{e}");
}

// dyn.md D-22 (`--threads=1` vs many), spec/model.md determinism: every
// byte is a function of the tables, at any schedule; and of SymbolIds,
// not of the order the sets were filled in.
#[test]
fn deterministic() {
    let (b, _) = every_row();
    let reference = b.link(Machine::AArch64, &Pool::sequential()).unwrap();
    check_schedules(|pool| b.link(Machine::AArch64, pool).unwrap());
    // The sets built in reverse key order give the same tables.
    let (mut r, _) = every_row();
    let n = r.res.def.len();
    r.res.imported =
        IdSet::from_ids(n, b.res.imported.iter().collect::<Vec<_>>().into_iter().rev()).unwrap();
    r.res.exported =
        IdSet::from_ids(n, b.res.exported.iter().collect::<Vec<_>>().into_iter().rev()).unwrap();
    assert_eq!(r.link(Machine::AArch64, &Pool::new(8)).unwrap(), reference);
}

// dyn.md D-24: `.dynstr`'s offsets are a function of the plan; two
// constructions of the same tables (names supplied by different
// objects, the lists walked in another order of construction) give the
// same bytes, with each distinct string once and no tail merging.
#[test]
fn dynstr_is_a_function_of_the_plan() {
    let (b, _) = every_row();
    let out = b.link(Machine::X86_64, &Pool::sequential()).unwrap();
    // Distinct strings once: `environ` and `__environ` are both present,
    // the second not merged into the first's tail.
    let t = &out.strtab;
    assert_eq!(t[0], 0);
    let count = |s: &[u8]| t.split(|&c| c == 0).filter(|w| *w == s).count();
    assert_eq!(count(b"environ"), 1);
    assert_eq!(count(b"__environ"), 1);
    assert_eq!((count(b"libc.so.6"), count(b"libm.so.6")), (1, 1), "DT_NEEDED names once");
    let total: usize =
        1 + t[1..].split(|&c| c == 0).filter(|w| !w.is_empty()).map(|w| w.len() + 1).sum::<usize>();
    assert_eq!(total, t.len());
    // A second construction: the same names in a fresh map.
    let names: BTreeMap<SymbolId, Vec<u8>> =
        out.plan.order().iter().rev().map(|&k| (k, b.symbol(k).to_vec())).collect();
    let needed: Vec<FileId> = b.slots.needed.to_vec();
    let c = Copy(names, b"libc.so.6".to_vec());
    let d1 = dynstr_plan(&out.plan, &needed[..1], &[], Some(b"/opt/lib"), &c);
    let d2 = dynstr_plan(&out.plan, &needed[..1], &[], Some(b"/opt/lib"), &b);
    assert_eq!(d1, d2);
}

/// The names of a second construction (a fresh map).
struct Copy(BTreeMap<SymbolId, Vec<u8>>, Vec<u8>);
impl DynNames for Copy {
    fn symbol(&self, k: SymbolId) -> &[u8] {
        &self.0[&k]
    }
    fn needed(&self, _: FileId) -> &[u8] {
        &self.1
    }
    fn version(&self, _: FileId, _: VerIndex) -> &[u8] {
        b""
    }
}

// ---- D-26: glibc's lookup over generated sets ------------------------------

/// A generator of distinct names: a 64-bit LCG, names of 1 to 40 bytes
/// from C identifier characters, some mangled-looking.
struct Gen(u64);

impl Gen {
    fn next(&mut self) -> u64 {
        self.0 =
            self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 17
    }

    fn name(&mut self, i: usize) -> String {
        const CH: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_";
        let len = 1 + (self.next() % 24) as usize;
        let mut s = String::from(if self.next().is_multiple_of(3) { "_ZN" } else { "" });
        for _ in 0..len {
            s.push(CH[(self.next() % CH.len() as u64) as usize] as char);
        }
        format!("{s}{i}")
    }
}

/// `n` exported functions and `n / 7` imports with generated names.
fn generated(n: usize, seed: u64) -> (Built, Vec<String>) {
    let mut g = Gen(seed);
    let mut fx = Fx { needed: vec![(1, b"libgen.so".to_vec())], ..Fx::default() };
    for i in 0..n / 7 {
        let name = format!("imp_{}", g.name(i));
        fx.import(&name, 1, sh(1, STB_GLOBAL << 4 | STT_FUNC, 0x10 * i as u64, 4), true);
    }
    for i in 0..n {
        let name = g.name(i);
        fx.export(&name, pd(STB_GLOBAL << 4 | STT_FUNC, 4), (13, 0x1000 + 16 * i as u64));
    }
    let absent = (0..200).map(|i| format!("absent{}", g.name(i))).collect();
    (fx.build(), absent)
}

fn sizes_under_test() -> Vec<usize> {
    let mut v = vec![0, 1, 2, 3];
    for k in 2..=16 {
        v.extend([(1usize << k) - 1, 1 << k, (1 << k) + 1]);
    }
    v.push(100_000);
    v.sort_unstable();
    v.dedup();
    v
}

// dyn.md D-26 (`gnu-hash-sizes`): glibc's lookup finds every exported
// name at its index and rejects the imports (value 0) and absent names,
// for sizes 0, 1, 2, 3, 2^k - 1, 2^k, 2^k + 1 (k = 2..16) and 10^5.
#[test]
fn glibc_finds_every_name() {
    let pool = Pool::new(8);
    for n in sizes_under_test() {
        let (b, absent) = generated(n, 0x5eed ^ n as u64);
        let out = b.link(Machine::X86_64, &pool).unwrap();
        let shape = GnuHashShape::of(&out.plan, Class::Elf64);
        assert_eq!(shape.n as usize, n);
        assert_eq!(shape.nbuckets, (n as u32 / 4).max(1));
        assert_eq!(out.gnu_hash.len() as u64, shape.size());
        for (i, &k) in out.plan.order().iter().enumerate() {
            let found = glibc_lookup(&out.gnu_hash, &out.dynsym, &out.strtab, b.symbol(k));
            let hashed = i >= out.plan.unhashed() as usize;
            assert_eq!(
                found,
                hashed.then_some(i as u32 + 1),
                "n={n} {:?}",
                b.fx.names[k.0 as usize]
            );
        }
        for a in &absent {
            assert_eq!(glibc_lookup(&out.gnu_hash, &out.dynsym, &out.strtab, a.as_bytes()), None);
        }
    }
}

// ---- D-26 against the reviewer's model and lld -----------------------------

/// The directory of the copied reviewer models.
fn models() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/dynsym-models")
}

/// A minimal ELF64 little-endian file (addresses equal file offsets)
/// holding `secs` = (name, type, link, info, entsize, bytes), plus
/// `.shstrtab`; two program headers, a `PT_LOAD` of the whole file and a
/// `PT_DYNAMIC` of `.dynamic` when there is one (readelf's `-d` reads the
/// segment, not the section).
/// A section for [`mini_elf`]: name, type, link, info, entsize, bytes.
type Sec<'a> = (&'a str, u32, u32, u32, u64, Vec<u8>);

fn mini_elf(machine: u16, secs: &[Sec<'_>]) -> Vec<u8> {
    let mut shstr = vec![0u8];
    let mut names = Vec::new();
    for s in secs.iter().map(|s| s.0).chain([".shstrtab"]) {
        names.push(shstr.len() as u32);
        shstr.extend_from_slice(s.as_bytes());
        shstr.push(0);
    }
    let mut body = vec![0u8; 64 + 2 * 56];
    let mut offs = Vec::new();
    for s in secs {
        while !body.len().is_multiple_of(8) {
            body.push(0);
        }
        offs.push(body.len() as u64);
        body.extend_from_slice(&s.5);
    }
    offs.push(body.len() as u64);
    body.extend_from_slice(&shstr);
    while !body.len().is_multiple_of(8) {
        body.push(0);
    }
    let shoff = body.len() as u64;
    let shnum = secs.len() as u16 + 2;
    let mut sh = vec![0u8; 64];
    let mut push = |name: u32,
                    typ: u32,
                    flags: u64,
                    off: u64,
                    size: u64,
                    link: u32,
                    info: u32,
                    align: u64,
                    ent: u64| {
        sh.extend_from_slice(&name.to_le_bytes());
        sh.extend_from_slice(&typ.to_le_bytes());
        sh.extend_from_slice(&flags.to_le_bytes());
        sh.extend_from_slice(&off.to_le_bytes()); // addr = offset
        sh.extend_from_slice(&off.to_le_bytes());
        sh.extend_from_slice(&size.to_le_bytes());
        sh.extend_from_slice(&link.to_le_bytes());
        sh.extend_from_slice(&info.to_le_bytes());
        sh.extend_from_slice(&align.to_le_bytes());
        sh.extend_from_slice(&ent.to_le_bytes());
    };
    for (i, s) in secs.iter().enumerate() {
        push(names[i], s.1, 2, offs[i], s.5.len() as u64, s.2, s.3, 8, s.4);
    }
    push(names[secs.len()], 3, 0, offs[secs.len()], shstr.len() as u64, 0, 0, 1, 0);
    body.extend_from_slice(&sh);
    let file_size = body.len() as u64;
    let mut ph = Vec::new();
    let mut phdr = |typ: u32, off: u64, size: u64| {
        ph.extend_from_slice(&typ.to_le_bytes());
        ph.extend_from_slice(&4u32.to_le_bytes()); // PF_R
        for v in [off, off, off, size, size, 8] {
            ph.extend_from_slice(&v.to_le_bytes());
        }
    };
    phdr(1, 0, file_size);
    match secs.iter().position(|s| s.0 == ".dynamic") {
        Some(i) => phdr(2, offs[i], secs[i].5.len() as u64),
        None => phdr(0, 0, 0),
    }
    body[64..64 + 2 * 56].copy_from_slice(&ph);
    let h = &mut body[..64];
    h[..16].copy_from_slice(&[0x7f, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    h[16..18].copy_from_slice(&3u16.to_le_bytes()); // ET_DYN
    h[18..20].copy_from_slice(&machine.to_le_bytes());
    h[20..24].copy_from_slice(&1u32.to_le_bytes());
    h[32..40].copy_from_slice(&64u64.to_le_bytes()); // e_phoff
    h[40..48].copy_from_slice(&shoff.to_le_bytes());
    h[52..54].copy_from_slice(&64u16.to_le_bytes());
    h[54..56].copy_from_slice(&56u16.to_le_bytes());
    h[56..58].copy_from_slice(&2u16.to_le_bytes());
    h[58..60].copy_from_slice(&64u16.to_le_bytes());
    h[60..62].copy_from_slice(&shnum.to_le_bytes());
    h[62..64].copy_from_slice(&(shnum - 1).to_le_bytes());
    body
}

/// Our sections of `out` as a section-only ELF: `.dynstr` 1, `.dynsym` 2,
/// `.gnu.hash` 3, and the version sections when present, then
/// `.dynamic` with the `DT_NEEDED` entries.
fn as_elf(out: &Out, machine: Machine) -> Vec<u8> {
    use mortar_elf::consts::{
        SHT_DYNAMIC, SHT_DYNSYM, SHT_GNU_HASH, SHT_GNU_VERNEED, SHT_GNU_VERSYM, SHT_STRTAB,
    };
    let mut secs = vec![
        (".dynstr", SHT_STRTAB, 0, 0, 0, out.strtab.clone()),
        (".dynsym", SHT_DYNSYM, 1, 1, 24, out.dynsym.clone()),
        (".gnu.hash", SHT_GNU_HASH, 2, 0, 0, out.gnu_hash.clone()),
    ];
    if !out.versym.is_empty() {
        secs.push((".gnu.version", SHT_GNU_VERSYM, 2, 0, 2, out.versym.clone()));
        let n = out.verneed.len() as u32;
        secs.push((".gnu.version_r", SHT_GNU_VERNEED, 1, n, 0, out.verneed_bytes.clone()));
    }
    let mut dynamic = Vec::new();
    let mut tag = |t: i64, v: u64| {
        dynamic.extend_from_slice(&t.to_le_bytes());
        dynamic.extend_from_slice(&v.to_le_bytes());
    };
    for &o in &out.dynstr.needed {
        tag(1, u64::from(o));
    }
    // DT_STRTAB (its address, here its offset, patched below), DT_STRSZ.
    tag(5, 0);
    tag(10, out.strtab.len() as u64);
    tag(0, 0);
    secs.push((".dynamic", SHT_DYNAMIC, 1, 0, 16, dynamic));
    let em = match machine {
        Machine::AArch64 => 183,
        _ => 62,
    };
    let mut elf = mini_elf(em, &secs);
    // Patch DT_STRTAB with .dynstr's offset (section 1).
    let shoff = u64::from_le_bytes(elf[40..48].try_into().unwrap()) as usize;
    let str_off = u64::from_le_bytes(elf[shoff + 64 + 24..shoff + 64 + 32].try_into().unwrap());
    let dyn_sec = shoff + 64 * secs.len();
    let dyn_off = u64::from_le_bytes(elf[dyn_sec + 24..dyn_sec + 32].try_into().unwrap()) as usize;
    let at = dyn_off + 16 * out.dynstr.needed.len() + 8;
    elf[at..at + 8].copy_from_slice(&str_off.to_le_bytes());
    elf
}

// dyn.md D-22, D-26 against the reviewer's independent model (written
// from the rule text; its own ELF reader): every table EQUAL, the hashed
// entries the tail, sorted by bucket. run.sh's sizes.
#[test]
fn reviewer_model_agrees() {
    let Some(python) = tool("python3") else { return };
    let scratch = Scratch::new("wp044-model");
    let mut files = Vec::new();
    for n in [
        0usize, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 15, 16, 17, 21, 31, 32, 33, 43, 64, 85, 100,
        171, 341, 1000, 5000,
    ] {
        let (b, _) = generated(n, n as u64);
        let out = b.link(Machine::AArch64, &Pool::sequential()).unwrap();
        files.push(scratch.write(&format!("n{n}.elf"), as_elf(&out, Machine::AArch64)));
    }
    let mut args = vec![models().join("gnuhash_model.py").into_os_string()];
    args.extend(files.iter().map(|f| f.clone().into_os_string()));
    let o = run(&python, std::iter::once("-I".into()).chain(args));
    assert!(o.success(), "{}", o.stderr_text());
    let lines: Vec<String> = o.stdout_text().lines().map(str::to_owned).collect();
    assert_eq!(lines.len(), files.len(), "{lines:?}");
    for l in &lines {
        assert!(l.contains("tail=True sorted=True EQUAL"), "{l}");
    }
    eprintln!("gnuhash model: {} of {} tables EQUAL", lines.len(), files.len());
}

// ---- real links -----------------------------------------------------------

/// A section of an ELF64 file by name: (offset, size, link, entsize).
struct Elf64<'a> {
    b: &'a [u8],
    secs: Vec<(String, u32, u64, u64, u32)>,
}

impl<'a> Elf64<'a> {
    fn new(b: &'a [u8]) -> Self {
        let r16 = |at: usize| u16::from_le_bytes(b[at..at + 2].try_into().unwrap());
        let r32 = |at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
        let r64 = |at: usize| u64::from_le_bytes(b[at..at + 8].try_into().unwrap());
        let (shoff, shnum, shstrndx) = (r64(40) as usize, r16(60) as usize, r16(62) as usize);
        let hdr = |i: usize| shoff + 64 * i;
        let stroff = r64(hdr(shstrndx) + 24) as usize;
        let secs = (0..shnum)
            .map(|i| {
                let h = hdr(i);
                let name = cstr(&b[stroff..], r32(h)).to_vec();
                (
                    String::from_utf8(name).unwrap(),
                    r32(h + 4),
                    r64(h + 24),
                    r64(h + 32),
                    r32(h + 40),
                )
            })
            .collect();
        Self { b, secs }
    }

    fn sec(&self, name: &str) -> Option<&'a [u8]> {
        let s = self.secs.iter().find(|s| s.0 == name)?;
        Some(&self.b[s.2 as usize..(s.2 + s.3) as usize])
    }

    fn linked(&self, name: &str) -> &'a [u8] {
        let s = self.secs.iter().find(|s| s.0 == name).unwrap();
        let l = &self.secs[s.4 as usize];
        &self.b[l.2 as usize..(l.2 + l.3) as usize]
    }
}

/// D-26's claim (lld's choice): lld's `.dynsym` keys, given `SymbolId`s in
/// lld's order, plan to lld's order and to lld's `.gnu.hash` byte for
/// byte, and D-23's rows reproduce lld's entries (`st_name` aside: lld's
/// `.dynstr` is its own).
#[test]
fn lld_gives_the_same_table() {
    let cases: [(&str, Machine); 2] =
        [("gcc", Machine::AArch64), ("x86_64-linux-gnu-gcc", Machine::X86_64)];
    let ns = [0usize, 1, 2, 3, 5, 10, 17, 43, 100, 341, 1000];
    let scratch = Scratch::new("wp044-lld");
    let mut agree = 0;
    for (cc, machine) in cases {
        let Some(cc) = tool(cc) else { continue };
        if tool("ld.lld").is_none() {
            return;
        }
        for n in ns {
            let mut c = String::from("#include <stdio.h>\n");
            for i in 1..=n {
                writeln!(c, "int f{i}(void){{return {i};}}").unwrap();
            }
            // An address of puts taken from non-PIC code: a canonical PLT.
            c += "int (*pp)(const char*) = puts;\nint main(void){puts(\"x\");return 0;}\n";
            let src = scratch.write(&format!("e{n}.c"), c);
            let exe = scratch.join(format!("{}.{n}.out", machine.name()));
            let o = run(
                &cc,
                [
                    "-O0".as_ref(),
                    "-fno-pie".as_ref(),
                    "-no-pie".as_ref(),
                    "-fuse-ld=lld".as_ref(),
                    "-rdynamic".as_ref(),
                    src.as_os_str(),
                    "-o".as_ref(),
                    exe.as_os_str(),
                ],
            );
            assert!(o.success(), "{}", o.stderr_text());
            let bytes = std::fs::read(&exe).unwrap();
            let e = Elf64::new(&bytes);
            let ref_sym = syms(e.sec(".dynsym").unwrap());
            let ref_str = e.linked(".dynsym");
            let ref_gh = e.sec(".gnu.hash").unwrap();
            let mut fx = Fx { needed: vec![(1, b"libc.so.6".to_vec())], ..Fx::default() };
            for s in &ref_sym[1..] {
                let name = String::from_utf8(cstr(ref_str, s.name).to_vec()).unwrap();
                let (bind, typ) = (s.info >> 4, s.info & 0xf);
                if s.shndx != 0 {
                    let def =
                        ProgramDef { info: s.info, other: s.other, size: s.size, version: None };
                    fx.export(&name, def, (u32::from(s.shndx), s.value));
                } else if s.value != 0 {
                    let k = fx.import(
                        &name,
                        1,
                        sh(1, STB_GLOBAL << 4 | typ, 0x10, 0),
                        bind == STB_GLOBAL,
                    );
                    fx.flag(k, DemandFlags::CANON_PLT);
                    fx.plt.insert(k.0, s.value);
                } else if typ == STT_NOTYPE && bind == STB_WEAK {
                    fx.key(&name, Spec::Undef { weak: true, weak_dyn: true });
                } else {
                    fx.import(&name, 1, sh(1, STB_GLOBAL << 4 | typ, 0x10, 0), bind == STB_GLOBAL);
                }
            }
            let b = fx.build();
            let out = b.link(machine, &Pool::sequential()).unwrap();
            let in_order: Vec<u32> = out.plan.order().iter().map(|k| k.0).collect();
            assert_eq!(
                in_order,
                (0..ref_sym.len() as u32 - 1).collect::<Vec<_>>(),
                "{machine:?} n={n}: lld's order"
            );
            assert_eq!(out.gnu_hash, ref_gh, "{machine:?} n={n}: .gnu.hash");
            let ours = syms(&out.dynsym);
            for (i, (a, r)) in ours.iter().zip(&ref_sym).enumerate() {
                assert_eq!(
                    Sym { name: 0, ..*a },
                    Sym { name: 0, ..*r },
                    "{machine:?} n={n} entry {i}"
                );
                assert_eq!(cstr(&out.strtab, a.name), cstr(ref_str, r.name));
            }
            agree += 1;
        }
    }
    eprintln!("lld: {agree} links, .dynsym order and .gnu.hash equal");
}

/// The `Verdef` names of a shared object by index (`vd_ndx`), its
/// `.gnu.version_d` read directly.
fn verdefs(lib: &[u8]) -> BTreeMap<u16, Vec<u8>> {
    let e = Elf64::new(lib);
    let mut out = BTreeMap::new();
    let (Some(vd), strs) = (e.sec(".gnu.version_d"), e.linked(".gnu.version_d")) else {
        return out;
    };
    let mut at = 0usize;
    loop {
        let r16 = |o: usize| u16::from_le_bytes(vd[o..o + 2].try_into().unwrap());
        let r32 = |o: usize| u32::from_le_bytes(vd[o..o + 4].try_into().unwrap());
        let ndx = r16(at + 4);
        let aux = at + r32(at + 12) as usize;
        out.insert(ndx, cstr(strs, r32(aux)).to_vec());
        let next = r32(at + 16) as usize;
        if next == 0 {
            break;
        }
        at += next;
    }
    out
}

// dyn.md D-27 against the reviewer's model (`verneed/model.py`, which
// reads our sections with readelf and the libraries' verdefs): for the
// imports of lld's links of `t.cc` (libstdc++, libm, libgcc_s, libc),
// with lld's DT_NEEDED order and each import's version read from lld's
// output, our `.gnu.version_r` is in needed order, its `Vernaux` in the
// library's verdef order, `vna_other` 2.. and flags 0; and our
// `.gnu.version` names, per entry, the version lld's names.
#[test]
#[allow(clippy::too_many_lines)] // lld's link, its tables read back, the fixture, then the checks
fn verneed_model_agrees() {
    let Some(python) = tool("python3") else { return };
    // An AArch64 program: the AArch64 g++ and C runtime by the host's
    // convention (mortar_testutil::target).
    let a64 = mortar_testutil::target::Arch::Aarch64;
    let Some(cxx) = a64.tool("g++") else { return };
    if tool("ld.lld").is_none() || tool("readelf").is_none() {
        return;
    }
    let dirs: Vec<String> = if a64.is_host() {
        vec!["/lib/aarch64-linux-gnu".into(), "/usr/lib/aarch64-linux-gnu".into()]
    } else {
        vec![a64.sysroot().join("lib").display().to_string()]
    };
    let scratch = Scratch::new("wp044-verneed");
    let exe = scratch.join("t.lld");
    let src = models().join("t.cc");
    let o = run(
        &cxx,
        [
            "-O1".as_ref(),
            "-fuse-ld=lld".as_ref(),
            src.as_os_str(),
            "-o".as_ref(),
            exe.as_os_str(),
            "-lm".as_ref(),
        ],
    );
    assert!(o.success(), "{}", o.stderr_text());
    let bytes = std::fs::read(&exe).unwrap();
    let e = Elf64::new(&bytes);
    let dynstr = e.linked(".dynsym");
    let ref_sym = syms(e.sec(".dynsym").unwrap());
    let versym: Vec<u16> = e
        .sec(".gnu.version")
        .unwrap()
        .chunks(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    // lld's needs: vna_other -> (file name, version name).
    let vr = e.sec(".gnu.version_r").unwrap();
    let mut need_of: BTreeMap<u16, (Vec<u8>, Vec<u8>)> = BTreeMap::new();
    let mut at = 0usize;
    let r16 = |o: usize| u16::from_le_bytes(vr[o..o + 2].try_into().unwrap());
    let r32 = |o: usize| u32::from_le_bytes(vr[o..o + 4].try_into().unwrap());
    loop {
        let (cnt, file, aux, next) = (r16(at + 2), r32(at + 4), r32(at + 8), r32(at + 12));
        let mut a = at + aux as usize;
        for _ in 0..cnt {
            need_of.insert(
                r16(a + 6),
                (cstr(dynstr, file).to_vec(), cstr(dynstr, r32(a + 8)).to_vec()),
            );
            a += r32(a + 12) as usize;
        }
        if next == 0 {
            break;
        }
        at += next as usize;
    }
    // DT_NEEDED in lld's order.
    let dynamic = e.sec(".dynamic").unwrap();
    let needed: Vec<Vec<u8>> = dynamic
        .chunks(16)
        .filter(|d| i64::from_le_bytes(d[0..8].try_into().unwrap()) == 1)
        .map(|d| cstr(dynstr, u64::from_le_bytes(d[8..16].try_into().unwrap()) as u32).to_vec())
        .collect();
    let find = |so: &[u8]| {
        dirs.iter()
            .map(|d| Path::new(d).join(String::from_utf8_lossy(so).as_ref()))
            .find(|p| p.exists())
            .unwrap()
    };
    let mut fx = Fx::default();
    let mut verdef_of = BTreeMap::new();
    for (i, so) in needed.iter().enumerate() {
        let file = i as u32 + 1;
        fx.needed.push((file, so.clone()));
        let defs = verdefs(&std::fs::read(find(so)).unwrap());
        for (&ndx, name) in &defs {
            fx.vernames.insert((file, ndx), name.clone());
        }
        verdef_of.insert(so.clone(), (file, defs));
    }
    let mut expect = Vec::new();
    for (i, s) in ref_sym.iter().enumerate().skip(1) {
        let name = String::from_utf8(cstr(dynstr, s.name).to_vec()).unwrap();
        let v = versym[i] & 0x7fff;
        let (file, version) = match need_of.get(&v) {
            Some((so, ver)) => {
                let (file, defs) = &verdef_of[so];
                let ndx = defs.iter().find(|d| d.1 == ver).map(|d| *d.0).unwrap();
                (*file, ndx)
            }
            None => (1, 0),
        };
        expect.push(need_of.get(&v).map(|r| r.1.clone()));
        if s.shndx != 0 {
            // A copy or an export: as an export (unversioned) unless lld
            // gives it a version (a copy), then a shared definition.
            if v >= 2 {
                let k = fx.key(
                    &name,
                    Spec::Shared {
                        file,
                        def: sh(file, s.info, 0x10 * i as u64, s.size),
                        referenced: true,
                        strong: true,
                        version,
                    },
                );
                fx.copies.push(CopyAlloc {
                    file: FileId(file),
                    value: 0x10 * i as u64,
                    place: CopyPlace::DynBss,
                    align: 8,
                    alias: k,
                });
                fx.copy_at.insert(fx.copies.len() as u32 - 1, (u32::from(s.shndx), s.value));
            } else {
                let def = ProgramDef { info: s.info, other: s.other, size: s.size, version: None };
                fx.export(&name, def, (u32::from(s.shndx), s.value));
            }
        } else {
            let typ = s.info & 0xf;
            if typ == STT_NOTYPE && s.info >> 4 == STB_WEAK && v < 2 {
                fx.key(&name, Spec::Undef { weak: true, weak_dyn: true });
            } else {
                let k = fx.key(
                    &name,
                    Spec::Shared {
                        file,
                        def: sh(file, s.info, 0x10 * i as u64, 0),
                        referenced: true,
                        strong: s.info >> 4 == STB_GLOBAL,
                        version,
                    },
                );
                if s.value != 0 {
                    fx.flag(k, DemandFlags::CANON_PLT);
                    fx.plt.insert(k.0, s.value);
                }
            }
        }
    }
    // Copies: sorted allocations renumber; rebuild the address map.
    let mut allocs: Vec<(CopyAlloc, (u32, u64))> =
        fx.copies.iter().copied().zip(fx.copy_at.values().copied()).collect();
    allocs.sort_by_key(|r| r.0);
    fx.copy_at = allocs.iter().enumerate().map(|(i, r)| (i as u32, r.1)).collect();
    let b = fx.build();
    let out = b.link(Machine::AArch64, &Pool::sequential()).unwrap();
    assert!(!out.verneed.is_empty());
    // Our .gnu.version names, per key, the version lld's names.
    let ours: Vec<u16> = out.versym.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let mut name_of_other: BTreeMap<u16, Vec<u8>> = BTreeMap::new();
    let mut other = 2;
    for ((f, vs), _) in out.verneed.iter().zip(&out.dynstr.versions) {
        for v in vs {
            name_of_other.insert(other, b.version(*f, *v).to_vec());
            other += 1;
        }
    }
    for (i, &k) in out.plan.order().iter().enumerate() {
        let got = name_of_other.get(&ours[i + 1]).cloned();
        assert_eq!(got, expect[k.0 as usize], "{:?}", String::from_utf8_lossy(b.symbol(k)));
    }
    // vna_hash is the ELF hash of each name.
    let path = scratch.write("ours.elf", as_elf(&out, Machine::AArch64));
    let mut args: Vec<std::ffi::OsString> =
        vec!["-I".into(), models().join("verneed_model.py").into(), path.clone().into()];
    args.extend(dirs.iter().map(Into::into));
    let o = run(&python, args);
    assert!(o.success(), "{}", o.stderr_text());
    let line = o.stdout_text();
    let dyn_text = run(tool("readelf").unwrap(), ["-dW".as_ref(), path.as_os_str()]).stdout_text();
    eprintln!("verneed model: {line}");
    assert!(line.contains("verneed-in-needed-order=True vernaux-in-verdef-order=True vna_other=2..=True flags=none"), "{line}\n{dyn_text}");
    // lld's .gnu.version_r, decoded: the same needs in the same order,
    // the same versions (hash, flags, name) in the same order. Not byte
    // for byte: lld writes every Verneed before the Vernaux and numbers
    // vna_other by first use; D-27 writes each Verneed's Vernaux after it
    // and numbers in output order (the version per entry is checked
    // above, by name).
    assert_eq!(decode_verneed(&out.verneed_bytes, &out.strtab), decode_verneed(vr, dynstr));
    eprintln!("verneed: .gnu.version_r's needs and versions equal lld's");
}

/// A `.gnu.version_r`, decoded: per `Verneed` its file name and, per
/// `Vernaux`, (`vna_hash`, `vna_flags`, name).
/// A decoded `.gnu.version_r`: (file, [(hash, flags, name)]).
type Needs = Vec<(Vec<u8>, Vec<(u32, u16, Vec<u8>)>)>;

fn decode_verneed(b: &[u8], strs: &[u8]) -> Needs {
    let r16 = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
    let r32 = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let mut out = Vec::new();
    let mut at = 0usize;
    loop {
        let mut a = at + r32(at + 8) as usize;
        let mut aux = Vec::new();
        for _ in 0..r16(at + 2) {
            assert_eq!(elf_hash(cstr(strs, r32(a + 8))), r32(a));
            aux.push((r32(a), r16(a + 4), cstr(strs, r32(a + 8)).to_vec()));
            a += r32(a + 12) as usize;
        }
        out.push((cstr(strs, r32(at + 4)).to_vec(), aux));
        let next = r32(at + 12) as usize;
        if next == 0 {
            return out;
        }
        at += next;
    }
}

// The cost of `dynsym_plan` and the fill (the hand-back's number), on
// 10^5 exports and 14 285 imports, at 1 and 8 threads: run with
// `cargo test --release -p mortar_synth --test dynsym -- --ignored cost`.
#[test]
#[ignore = "a measurement, not a check"]
fn cost() {
    let (b, _) = generated(100_000, 7);
    for threads in [1, 8] {
        let pool = if threads == 1 { Pool::sequential() } else { Pool::new(threads) };
        let mut best = (f64::MAX, f64::MAX);
        for _ in 0..5 {
            let t0 = std::time::Instant::now();
            let plan = b.plan(&pool).unwrap();
            let t1 = std::time::Instant::now();
            b.link(Machine::X86_64, &pool).unwrap();
            let all = t1.elapsed().as_secs_f64() * 1e3;
            let p = (t1 - t0).as_secs_f64() * 1e3;
            best = (best.0.min(p), best.1.min(all));
            drop(plan);
        }
        eprintln!(
            "threads={threads}: dynsym_plan {:.2} ms; plan + all five fills {:.2} ms",
            best.0, best.1
        );
    }
}

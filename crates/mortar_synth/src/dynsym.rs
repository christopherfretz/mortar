//! `.dynsym` (`spec/dyn.md` D-21 to D-23, D-26; `spec/synth.md` S-101):
//! which keys, in which order, the `.dynsym` index of each, and the
//! entries' bytes. Owned by WP-M2-DYNSYM.
//!
//! `mortar_resolve` produces only the sets (`Resolution::{imported,
//! exported}`, D-21 items 1 and 2), the scan the `WeakDyn` keys
//! (`Demands::weak_dyn`, item 4) and the `EXPORT` demands, and `Slots` the
//! copy allocations (item 3); this plan is the one place that orders them.
//! The relocation writers take `r_sym` and D-40's sort key from
//! [`DynsymPlan::index`].
//!
//! **Plan** ([`dynsym_plan`], before layout): the D-21 set, each key's
//! [`Form`] (D-23's row), the unhashed/hashed split and D-22's order with
//! D-26's `nbuckets`. Nothing in it depends on an address.
//!
//! **Write** ([`DynsymChunk`], image): any range of entries in O(range ×
//! log), each entry recomputed from the tables (the form) and an address
//! oracle ([`DynAddrs`]); divisible (S-4).
//!
//! **A shared object** (`spec/dyn.md` D-111; [`dynsym_plan_shared`],
//! [`DynsymInputs::shared`]): the export set (`Resolution::exported`,
//! preemptible or bound alike), the imports, and every undefined key a
//! live relocatable object references and no collected section alone
//! names (the scan's `Demands::weak_dyn` in a shared object): a strong
//! one is [`Form::Undefined`] (`STB_GLOBAL STT_NOTYPE SHN_UNDEF`, D-111
//! item 2), a weak one [`Form::WeakDyn`] (item 3). No copy alias and no
//! canonical PLT entry exists (D-122): one is an internal error. An
//! exported TLS definition is written as `.symtab` writes it; an exported
//! preemptible IFUNC `STT_GNU_IFUNC` at its resolver (D-121).

use std::ops::Range;

use mortar_core::demands::DemandFlags;
use mortar_core::id::{SymEntry, SymKey, SymbolId};
use mortar_core::par::Pool;
use mortar_core::resolution::{Def, RefFlags};
use mortar_core::{Demands, DynStr, Resolution, Slots, VerIndex};
use mortar_diag::{Collector, Diagnostic, Kind};
use mortar_elf::consts::{
    SHN_ABS, SHN_LORESERVE, SHN_UNDEF, STB_GLOBAL, STB_GNU_UNIQUE, STB_WEAK,
    STO_AARCH64_VARIANT_PCS, STT_FUNC, STT_GNU_IFUNC, STT_NOTYPE, STT_TLS, STV_HIDDEN,
    STV_INTERNAL, STV_MASK,
};
use mortar_elf::{Class, Machine};

use crate::dynstr::DynNames;
use crate::error::{Failed, SynthError, feature, internal, key};
use crate::gnuhash::{gnu_hash, nbuckets_for};
use crate::image::{Division, FillCx, SizeCx, SynthChunk};
use crate::out::Window;

/// The `.dynsym` of an output (D-21, D-22, D-26): the entries after the
/// null one, the unhashed ones first. Empty (no `.dynsym`) in a static
/// link.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DynsymPlan {
    order: Box<[SymbolId]>,
    unhashed: u32,
    nbuckets: u32,
    /// `(key, index)` sorted by key.
    index: Box<[(SymbolId, u32)]>,
}

impl DynsymPlan {
    /// The plan of `order` (the entries after the null one, D-22's order),
    /// whose first `unhashed` entries are unhashed, with `nbuckets`
    /// `.gnu.hash` buckets (D-26). `None` when a key is given twice or
    /// `unhashed` exceeds the entries. Also the fixture constructor.
    #[must_use]
    pub fn new(order: Box<[SymbolId]>, unhashed: u32, nbuckets: u32) -> Option<Self> {
        if unhashed as usize > order.len() {
            return None;
        }
        let mut index: Vec<(SymbolId, u32)> =
            order.iter().enumerate().map(|(i, &k)| (k, i as u32 + 1)).collect();
        index.sort_unstable();
        if index.windows(2).any(|w| w[0].0 == w[1].0) {
            return None;
        }
        Some(Self { order, unhashed, nbuckets, index: index.into() })
    }

    /// The `.dynsym` index of `key` (1 for the first entry after the null
    /// one), if it is an entry.
    #[must_use]
    pub fn index(&self, key: SymbolId) -> Option<u32> {
        self.index.binary_search_by_key(&key, |row| row.0).ok().map(|at| self.index[at].1)
    }

    /// The entries after the null one, in `.dynsym` order (`Slots.dynsym`).
    #[must_use]
    pub fn order(&self) -> &[SymbolId] {
        &self.order
    }

    /// How many of [`Self::order`]'s entries are unhashed (D-22).
    #[must_use]
    pub fn unhashed(&self) -> u32 {
        self.unhashed
    }

    /// The hashed entries (D-22), in `.dynsym` order: the tail of
    /// [`Self::order`].
    #[must_use]
    pub fn hashed(&self) -> &[SymbolId] {
        &self.order[self.unhashed as usize..]
    }

    /// `.gnu.hash`'s `nbuckets` (D-26).
    #[must_use]
    pub fn nbuckets(&self) -> u32 {
        self.nbuckets
    }

    /// Whether the plan has no entry besides the null one.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// The keys with an index, in key order (for the checks and the map).
    pub fn keys(&self) -> impl Iterator<Item = SymbolId> + '_ {
        self.index.iter().map(|row| row.0)
    }

    /// The `.dynsym` entry count, the null entry included (`sh_size /
    /// sh_entsize`).
    #[must_use]
    pub fn count(&self) -> u32 {
        self.order.len() as u32 + 1
    }
}

// ---- the inputs --------------------------------------------------------------

/// The `st_info`, `st_other`, `st_value` and `st_size` of the entry a
/// shared object defines a key with (`Resolution::bind`, D-11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SharedDef {
    pub info: u8,
    pub other: u8,
    pub value: u64,
    pub size: u64,
}

/// A program definition as `.symtab` writes it (`spec/synth.md` S-81,
/// S-121): the output `st_info` (binding and type: a common symbol is
/// `STT_OBJECT`), the merged `st_other`, the size, and the version its
/// definition is written with (`foo@@V` in a relocatable object), if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProgramDef<'a> {
    pub info: u8,
    pub other: u8,
    pub size: u64,
    pub version: Option<&'a [u8]>,
}

/// What `.dynsym` and its companions read of the inputs besides the
/// tables: names (through [`DynNames`]), the shared definitions' entry
/// fields and versions, and the program definitions' `.symtab` facts.
pub trait DynSymSource: DynNames + Sync {
    /// The fields of a shared object's `.dynsym` entry `entry` (a
    /// `Def::Shared` definition). `None` only for an entry that does not
    /// exist (an internal error).
    fn shared(&self, entry: SymEntry) -> Option<SharedDef>;
    /// The `.symtab` facts of a key the program defines (`Defined`,
    /// `Common`, `Internal`). `None` only for a key the program does not
    /// define.
    fn program(&self, key: SymbolId) -> Option<ProgramDef<'_>>;

    /// Whether key `key` is a non-default `foo@V` (its version in its
    /// name; `ProgramDef::version` of a `foo@@V` definition comes from
    /// the defining entry). Only for messages.
    fn non_default_version(&self, key: SymbolId) -> bool {
        let _ = key;
        false
    }
    /// The version of the shared definition `entry` (its `versym` index,
    /// the hidden bit cleared, D-27, resolve §9.2): `None` or an index
    /// below 2 for an unversioned definition.
    fn bound_version(&self, entry: SymEntry) -> Option<VerIndex>;
    /// Whether the key's definition has a place (`spec/resolve.md` §10.1
    /// P-1): `false` only for a key defined in an input section without
    /// one (dropped, consumed, collected), whose export is omitted, never
    /// an entry without a value (WP-136). `true` for every other key.
    fn has_place(&self, _key: SymbolId) -> bool {
        true
    }
}

/// The address facts of the entries (the image phase's answers).
pub trait DynAddrs: Sync {
    /// An exported key's output section index and value, exactly as
    /// `.symtab` writes them (`SHN_ABS` for an absolute value, the TLS
    /// offset for a TLS symbol; the index unreduced).
    fn defined(&self, key: SymbolId) -> Option<(u32, u64)>;
    /// The address of the PLT entry of a key with a `CANON_PLT` demand.
    fn plt(&self, key: SymbolId) -> Option<u64>;
    /// The output section index of `.plt` and the address of the IPLT
    /// entry of a key with an `IPLT` demand.
    fn iplt(&self, key: SymbolId) -> Option<(u32, u64)>;
    /// The output section index and address of copy allocation `alloc`
    /// (its position in `Slots::copies`).
    fn copy(&self, alloc: u32) -> Option<(u32, u64)>;
}

/// The tables a `.dynsym` entry is a function of.
#[derive(Clone, Copy)]
pub struct DynsymInputs<'a> {
    pub resolution: &'a Resolution,
    pub demands: &'a Demands,
    pub slots: &'a Slots,
    pub src: &'a dyn DynSymSource,
    /// The output is a shared object (`spec/dyn.md` D-111): an undefined
    /// key is an entry of its own (strong: [`Form::Undefined`]; weak:
    /// [`Form::WeakDyn`] whether or not the scan named it), and copies
    /// and canonical PLT entries do not exist.
    pub shared: bool,
}

impl std::fmt::Debug for DynsymInputs<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DynsymInputs").finish_non_exhaustive()
    }
}

/// The row of D-23's table an entry takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// A shared definition referenced through the GOT or a PLT: undefined,
    /// value 0 (unhashed).
    Import,
    /// A shared function with a `CANON_PLT` demand: `SHN_UNDEF`, value the
    /// PLT entry (hashed).
    CanonPlt,
    /// A shared definition at copy allocation `.0` (its position in
    /// `Slots::copies`), each alias: defined at the copy (hashed).
    Copy(u32),
    /// A program definition (hashed).
    Export,
    /// A `WeakDyn` key (D-25): `WEAK UND` (unhashed). In a shared object
    /// every weak undefined key a live object references (D-111 item 3).
    WeakDyn,
    /// A shared object's strong undefined key (D-111 item 2): `STB_GLOBAL
    /// STT_NOTYPE SHN_UNDEF` (unhashed), left to the loader.
    Undefined,
}

impl Form {
    /// Whether the entry is hashed (D-22: defined, or undefined with a
    /// non-zero value).
    #[must_use]
    pub fn hashed(self) -> bool {
        !matches!(self, Self::Import | Self::WeakDyn | Self::Undefined)
    }
}

/// The names D-21 never exports (`_DYNAMIC`, `_GLOBAL_OFFSET_TABLE_`;
/// `__dso_handle` is hidden, so the visibility rule covers it).
const NEVER: [&[u8]; 3] = [b"_DYNAMIC", b"_GLOBAL_OFFSET_TABLE_", b"__dso_handle"];

/// The refusal of an ELF32 dynamic output (i386 dynamic links are M4,
/// `spec/landed/dyn/WP-040.md` §6).
///
/// # Errors
///
/// `refused` (feature) for [`Class::Elf32`].
pub fn elf64(class: Class) -> Result<(), SynthError> {
    match class {
        Class::Elf64 => Ok(()),
        Class::Elf32 => {
            Err(feature("dynamic output for ELFCLASS32 (i386 dynamic links are M4)").into())
        }
    }
}

impl DynsymInputs<'_> {
    /// The copy allocation at `(file, value)`, if any.
    fn copy_at(&self, file: mortar_core::FileId, value: u64) -> Option<u32> {
        let copies = &self.slots.copies;
        copies
            .binary_search_by_key(&(file, value), mortar_core::CopyAlloc::key)
            .ok()
            .map(|at| at as u32)
    }

    /// Whether `key`, defined with `info`, is a preemptible IFUNC of a
    /// shared object (`spec/dyn.md` D-112, D-121).
    fn preemptible_ifunc(&self, key: SymbolId, info: u8) -> bool {
        self.shared && info & 0xf == STT_GNU_IFUNC && self.resolution.preemptible.contains(key)
    }

    /// The D-23 row of key `k`, and the checks of D-21 and D-23 on it.
    ///
    /// # Errors
    ///
    /// A refusal by name (`versioned-export`, an exported IFUNC with no
    /// IPLT entry), or `internal-error` when the tables contradict each
    /// other (an undefined key that is not `WeakDyn`, an export the
    /// program does not define or that is hidden).
    #[allow(clippy::too_many_lines, reason = "one arm per definition kind (D-23, D-111)")]
    pub fn form(&self, key: SymbolId) -> Result<Form, SynthError> {
        let res = self.resolution;
        let name = || String::from_utf8_lossy(self.src.symbol(key)).into_owned();
        let flags = self.demands.of(SymKey::global(key));
        let weak_dyn = self.demands.weak_dyn.contains(key);
        match res.def.get(key).copied() {
            None => Err(internal(format!("a .dynsym key {} out of range", key.0))),
            Some(Def::Shared(e)) => {
                if weak_dyn {
                    return Err(internal(format!(
                        "{} is WeakDyn and has a shared definition (spec/dyn.md D-25)",
                        name()
                    )));
                }
                let def = self.src.shared(e).ok_or_else(|| {
                    internal::<SynthError>(format!("the shared definition of {} is absent", name()))
                })?;
                if self.shared {
                    // D-122: no copy and no canonical PLT in a shared object.
                    if flags.intersects(DemandFlags::COPY | DemandFlags::CANON_PLT)
                        || !self.slots.copy_of.is_empty()
                    {
                        return Err(internal(format!(
                            "a copy or canonical PLT entry of {} in a shared object \
                             (spec/dyn.md D-122)",
                            name()
                        )));
                    }
                    return Ok(Form::Import);
                }
                let by_value = self.copy_at(e.file, def.value);
                let copy_of = &self.slots.copy_of;
                let by_key =
                    copy_of.binary_search_by_key(&key, |row| row.0).ok().map(|at| copy_of[at].1);
                match (by_key, by_value) {
                    (Some(x), Some(y)) if x == y => {}
                    (Some(_), _) => {
                        return Err(internal(format!(
                            "the copy allocation of {} is not at its definition (spec/dyn.md D-42)",
                            name()
                        )));
                    }
                    _ => {}
                }
                if let Some(a) = by_value {
                    let typ = def.info & 0xf;
                    if matches!(typ, STT_FUNC | STT_GNU_IFUNC | STT_TLS) {
                        return Err(internal(format!(
                            "a copy allocation of {} whose type is {typ} (spec/dyn.md D-42)",
                            name()
                        )));
                    }
                    return Ok(Form::Copy(a));
                }
                if flags.contains(DemandFlags::CANON_PLT) {
                    Ok(Form::CanonPlt)
                } else {
                    Ok(Form::Import)
                }
            }
            Some(Def::Defined(_) | Def::Common(_) | Def::Internal(_)) => {
                if weak_dyn {
                    return Err(internal(format!(
                        "{} is WeakDyn and the program defines it (spec/dyn.md D-25)",
                        name()
                    )));
                }
                let p = self.src.program(key).ok_or_else(|| {
                    internal::<SynthError>(format!("the definition of {} is absent", name()))
                })?;
                let vis = p.other & STV_MASK;
                if vis == STV_HIDDEN || vis == STV_INTERNAL || NEVER.contains(&self.src.symbol(key))
                {
                    return Err(internal(format!(
                        "{} is exported, which spec/dyn.md D-21 never does",
                        name()
                    )));
                }
                // D-115 (WP-125): a shared object's versioned export has
                // its version node (resolution refused it otherwise, resolve
                // §9.7 rule 12); an executable's is refused (D-27).
                if let Some(version) = p.version {
                    let at = if self.src.non_default_version(key) { "@" } else { "@@" };
                    let v = self.resolution.props.get(key).map_or(0, |p| p.version);
                    if !self.shared {
                        return Err(Diagnostic::error(Kind::VersionedExport {
                            name: name(),
                            version: format!("{at}{}", String::from_utf8_lossy(version)),
                        })
                        .into());
                    }
                    if v == 0 {
                        return Err(internal(format!(
                            "{}{at}{} is exported without its version (spec/dyn.md D-115)",
                            name(),
                            String::from_utf8_lossy(version)
                        )));
                    }
                }
                // D-121: a preemptible IFUNC of a shared object is exported
                // `STT_GNU_IFUNC` at its resolver and has no IPLT entry.
                if p.info & 0xf == STT_GNU_IFUNC
                    && !self.preemptible_ifunc(key, p.info)
                    && !flags.contains(DemandFlags::IPLT)
                {
                    return Err(feature(format!(
                        "exported IFUNC {} that no relocation names (it has no IPLT entry, \
                         spec/dyn.md D-23)",
                        name()
                    ))
                    .into());
                }
                Ok(Form::Export)
            }
            Some(Def::Undefined { weak }) if self.shared => {
                // D-111 items 2 and 3: the scan's set (`Demands::weak_dyn`
                // in a shared object) makes the entry.
                if !weak_dyn {
                    return Err(internal(format!(
                        "undefined {} in .dynsym has no live reference (spec/dyn.md D-111)",
                        name()
                    )));
                }
                Ok(if weak { Form::WeakDyn } else { Form::Undefined })
            }
            Some(Def::Undefined { weak }) => {
                if weak && weak_dyn {
                    Ok(Form::WeakDyn)
                } else {
                    Err(internal(format!(
                        "undefined {} in .dynsym is not a WeakDyn key (spec/dyn.md D-21, D-25)",
                        name()
                    )))
                }
            }
        }
    }

    /// The bytes of entry `k` (24, ELF64) given its form (D-23) and its
    /// `st_name`.
    ///
    /// # Errors
    ///
    /// `internal-error` when an address is missing, a refusal when the
    /// section index needs an extension `.dynsym` cannot have.
    pub fn entry(
        &self,
        k: SymbolId,
        st_name: u32,
        machine: Machine,
        addrs: &dyn DynAddrs,
    ) -> Result<[u8; 24], SynthError> {
        let res = self.resolution;
        let name = || String::from_utf8_lossy(self.src.symbol(k)).into_owned();
        let missing = |what: &str| internal::<SynthError>(format!("{} has no {what}", name()));
        let shared = |e| self.src.shared(e).ok_or_else(|| missing("shared definition"));
        let (info, other, shndx, value, size) = match self.form(k)? {
            form @ (Form::Import | Form::CanonPlt) => {
                let Some(Def::Shared(e)) = res.def.get(k).copied() else {
                    return Err(missing("shared definition"));
                };
                let d = shared(e)?;
                let strong = res.props.get(k).is_some_and(|p| p.refs.contains(RefFlags::STRONG));
                let bind = if strong { STB_GLOBAL } else { STB_WEAK };
                // D-23 (both import rows): the definition's
                // `STO_AARCH64_VARIANT_PCS` is kept; glibc reads it on the
                // program's own `.dynsym` entry of a `JUMP_SLOT` to bind
                // it at load under `DT_AARCH64_VARIANT_PCS` (D-30 row 25).
                let pcs =
                    if machine == Machine::AArch64 { d.other & STO_AARCH64_VARIANT_PCS } else { 0 };
                if form == Form::CanonPlt {
                    let value = addrs.plt(k).ok_or_else(|| missing("PLT entry"))?;
                    (bind << 4 | STT_FUNC, pcs, u32::from(SHN_UNDEF), value, 0)
                } else {
                    let typ = match d.info & 0xf {
                        STT_FUNC | STT_GNU_IFUNC => STT_FUNC,
                        t => t,
                    };
                    (bind << 4 | typ, pcs, u32::from(SHN_UNDEF), 0, 0)
                }
            }
            Form::Copy(a) => {
                let Some(Def::Shared(e)) = res.def.get(k).copied() else {
                    return Err(missing("shared definition"));
                };
                let d = shared(e)?;
                let bind = if d.info >> 4 == STB_WEAK { STB_WEAK } else { STB_GLOBAL };
                let (shndx, value) = addrs.copy(a).ok_or_else(|| missing("copy allocation"))?;
                (bind << 4 | (d.info & 0xf), 0, shndx, value, d.size)
            }
            Form::Export => {
                let p = self.src.program(k).ok_or_else(|| missing("definition"))?;
                let (typ, (shndx, value)) = if self.preemptible_ifunc(k, p.info) {
                    // D-121: the loader resolves every use (its records).
                    (STT_GNU_IFUNC, addrs.defined(k).ok_or_else(|| missing("value"))?)
                } else if p.info & 0xf == STT_GNU_IFUNC {
                    (STT_FUNC, addrs.iplt(k).ok_or_else(|| missing("IPLT entry"))?)
                } else {
                    (p.info & 0xf, addrs.defined(k).ok_or_else(|| missing("value"))?)
                };
                let pcs = if machine == Machine::AArch64 { STO_AARCH64_VARIANT_PCS } else { 0 };
                (p.info & 0xf0 | typ, p.other & (STV_MASK | pcs), shndx, value, p.size)
            }
            // D-23's `WeakDyn` row: `STO_AARCH64_VARIANT_PCS` when a
            // reference has it (resolve §3.7), so that a `JUMP_SLOT` to a
            // vector-PCS function a library supplies is bound at load
            // (D-30 row 25; GNU ld and lld). D-111 item 2's strong
            // undefined entry is the same with `STB_GLOBAL`.
            form @ (Form::WeakDyn | Form::Undefined) => {
                let pcs = machine == Machine::AArch64
                    && res.props.get(k).is_some_and(|p| p.refs.contains(RefFlags::VARIANT_PCS));
                let other = if pcs { STO_AARCH64_VARIANT_PCS } else { 0 };
                let bind = if form == Form::WeakDyn { STB_WEAK } else { STB_GLOBAL };
                (bind << 4 | STT_NOTYPE, other, u32::from(SHN_UNDEF), 0, 0)
            }
        };
        if shndx >= u32::from(SHN_LORESERVE) && shndx != u32::from(SHN_ABS) {
            return Err(feature(format!(
                ".dynsym entry {} in output section {shndx} (an extended section index)",
                name()
            ))
            .into());
        }
        let mut buf = [0u8; 24];
        buf[0..4].copy_from_slice(&st_name.to_le_bytes());
        buf[4] = info;
        buf[5] = other;
        buf[6..8].copy_from_slice(&(shndx as u16).to_le_bytes());
        buf[8..16].copy_from_slice(&value.to_le_bytes());
        buf[16..24].copy_from_slice(&size.to_le_bytes());
        Ok(buf)
    }
}

/// D-21's set in D-22's order with D-26's `nbuckets`, from the
/// resolution's sets, the `EXPORT` demands, the scan's `WeakDyn` keys and
/// the copy allocations of `slots` (with every alias at each allocation's
/// address, found through `src`).
///
/// The order: the unhashed entries (imports without `CANON_PLT`, `WeakDyn`
/// keys) by `SymbolId`, then the hashed ones (exports, copy aliases,
/// canonical PLTs) by (`gnu_hash(name) mod nbuckets`, `SymbolId`), with
/// `nbuckets = max(1, ⌊hashed/4⌋)`. A function of the tables: the same at
/// any thread count.
///
/// Each key's D-23 row is checked ([`DynsymInputs::form`]); a key that
/// fails is reported under `(80, 0, SymbolId)`.
///
/// # Errors
///
/// [`Failed`] after emitting each failing key's diagnostic.
pub fn dynsym_plan(
    resolution: &Resolution,
    demands: &Demands,
    slots: &Slots,
    src: &dyn DynSymSource,
    pool: &Pool,
    diag: &Collector,
) -> Result<DynsymPlan, Failed> {
    let t = DynsymInputs { resolution, demands, slots, src, shared: false };
    plan_of(&t, pool, diag)
}

/// D-111: the `.dynsym` of a shared object, as [`dynsym_plan`] with
/// [`DynsymInputs::shared`]: the exports (`Resolution::exported`), the
/// imports, the `EXPORT` demands, and the undefined keys of
/// `Demands::weak_dyn`, which in a shared object is the scan's D-111 set
/// (a live relocatable object references the key and not only from
/// sections `--gc-sections` collected; strong: [`Form::Undefined`], weak:
/// [`Form::WeakDyn`]). The order is
/// D-22's (unhashed by `SymbolId`, then hashed by bucket and `SymbolId`),
/// `.gnu.hash`'s geometry D-26's: the same functions as an executable's.
///
/// # Errors
///
/// [`Failed`] after emitting each failing key's diagnostic.
pub fn dynsym_plan_shared(
    resolution: &Resolution,
    demands: &Demands,
    slots: &Slots,
    src: &dyn DynSymSource,
    pool: &Pool,
    diag: &Collector,
) -> Result<DynsymPlan, Failed> {
    let t = DynsymInputs { resolution, demands, slots, src, shared: true };
    plan_of(&t, pool, diag)
}

/// The plan of [`dynsym_plan`] and [`dynsym_plan_shared`].
fn plan_of(t: &DynsymInputs<'_>, pool: &Pool, diag: &Collector) -> Result<DynsymPlan, Failed> {
    let (resolution, demands, slots, src) = (t.resolution, t.demands, t.slots, t.src);
    let mut keys: Vec<SymbolId> = resolution
        .imported
        .iter()
        .chain(resolution.exported.iter())
        .chain(demands.weak_dyn.iter())
        .chain(demands.keys_with(DemandFlags::EXPORT).filter_map(SymKey::as_global))
        .chain(slots.copy_of.iter().map(|row| row.0))
        .collect();
    pool.unit_cost(20).sort_by(&mut keys, Ord::cmp);
    keys.dedup();
    // P-1 (WP-136): an export of a definition without a place is omitted,
    // never an error; a reference to it the scan refused already.
    keys.retain(|&k| src.has_place(k));
    if !slots.copies.is_empty() {
        let aliases = copy_aliases(t, &keys, pool);
        keys.extend(aliases);
        pool.unit_cost(20).sort_by(&mut keys, Ord::cmp);
    }
    if let Err(d) = one_entry_per_definition(t, &keys) {
        diag.emit(key(0), d.into_diagnostic());
        return Err(Failed);
    }
    let forms: Vec<Result<Form, SynthError>> =
        pool.unit_cost(100).map(keys.len(), |i| t.form(keys[i]));
    let mut failed = false;
    let mut unhashed = Vec::new();
    let mut hashed = Vec::new();
    for (&k, f) in keys.iter().zip(forms) {
        match f {
            Ok(f) if f.hashed() => hashed.push(k),
            Ok(_) => unhashed.push(k),
            Err(d) => {
                diag.emit(key(u64::from(k.0)), d.into_diagnostic());
                failed = true;
            }
        }
    }
    if failed {
        return Err(Failed);
    }
    let nbuckets = nbuckets_for(hashed.len() as u32);
    let mut sorted: Vec<(u32, SymbolId)> = pool
        .unit_cost(40)
        .map(hashed.len(), |i| (gnu_hash(src.symbol(hashed[i])) % nbuckets, hashed[i]));
    pool.unit_cost(20).sort_by(&mut sorted, Ord::cmp);
    let n_unhashed = unhashed.len() as u32;
    let order: Vec<SymbolId> =
        unhashed.into_iter().chain(sorted.into_iter().map(|row| row.1)).collect();
    // `keys` was deduplicated, so `new` cannot fail.
    DynsymPlan::new(order.into(), n_unhashed, nbuckets).ok_or(Failed)
}

/// Every key whose shared definition lies at a copy allocation's
/// `(file, value)` (D-21 item 3; resolve §7.3), whether or not it is
/// referenced, in `SymbolId` order: one key per shared definition (a
/// `.dynsym` entry is a name and a version, so `foo` and the row `foo@V`
/// that a default-version definition `foo@@V` also binds, resolve §9.2,
/// are one entry). A definition that a key of `present` (the set so far,
/// sorted) already names adds nothing; otherwise its least key is taken.
fn copy_aliases(t: &DynsymInputs<'_>, present: &[SymbolId], pool: &Pool) -> Vec<SymbolId> {
    const BLOCK: usize = 4096;
    let defs = t.resolution.def.as_slice();
    let copies = &t.slots.copies;
    let blocks = defs.len().div_ceil(BLOCK);
    let found: Vec<Vec<(SymEntry, SymbolId)>> = pool.unit_cost(20_000).map(blocks, |b| {
        let mut out = Vec::new();
        for (i, def) in defs.iter().enumerate().skip(b * BLOCK).take(BLOCK) {
            let Def::Shared(e) = *def else { continue };
            let start = copies.partition_point(|c| c.file < e.file);
            if copies.get(start).is_none_or(|c| c.file != e.file) {
                continue;
            }
            if let Some(d) = t.src.shared(e)
                && t.copy_at(e.file, d.value).is_some()
            {
                out.push((e, SymbolId(i as u32)));
            }
        }
        out
    });
    let mut named: Vec<SymEntry> = present
        .iter()
        .filter_map(|&k| match t.resolution.def.get(k) {
            Some(Def::Shared(e)) => Some(*e),
            _ => None,
        })
        .collect();
    named.sort_unstable();
    let mut found: Vec<(SymEntry, SymbolId)> = found.into_iter().flatten().collect();
    // By (definition, key): the first of each definition is its least key.
    found.sort_unstable();
    found.dedup_by_key(|row| row.0);
    found.retain(|row| named.binary_search(&row.0).is_err());
    found.into_iter().map(|row| row.1).collect()
}

/// D-21/D-22: no two hashed entries for one shared definition. Two keys
/// that resolve to one shared definition and that the program both
/// references (`foo` and a `.symver` reference `foo@V` that `foo@@V`
/// binds, resolve §9.2) would be two entries of one name and version:
/// a copy or a canonical PLT twice, which the loader's lookup finds only
/// once (the other key's address is then not the one the program
/// compares). Refused by name; two unhashed imports of one definition
/// are two `UND` entries the loader binds alike and are kept.
fn one_entry_per_definition(t: &DynsymInputs<'_>, keys: &[SymbolId]) -> Result<(), SynthError> {
    let mut shared: Vec<(SymEntry, SymbolId)> = keys
        .iter()
        .filter_map(|&k| match t.resolution.def.get(k) {
            Some(Def::Shared(e)) => Some((*e, k)),
            _ => None,
        })
        .collect();
    shared.sort_unstable();
    for w in shared.windows(2) {
        if w[0].0 != w[1].0 {
            continue;
        }
        let hashed = |k| t.form(k).is_ok_and(Form::hashed);
        if hashed(w[0].1) || hashed(w[1].1) {
            return Err(feature(format!(
                "references to the shared definition of {} both by its plain name and by \
                 its explicit version (`.symver`), where a defined .dynsym entry is needed \
                 (a copy relocation or a canonical PLT entry, spec/dyn.md D-21)",
                String::from_utf8_lossy(t.src.symbol(w[0].1))
            ))
            .into());
        }
    }
    Ok(())
}

/// Whether `.dynsym` holds an `STT_GNU_IFUNC` or `STB_GNU_UNIQUE` symbol
/// (D-23's `EI_OSABI` rule, with `.symtab`'s and the `IRELATIVE` records'):
/// only an exported unique definition can, since D-23 writes an IFUNC as
/// `STT_FUNC` and a copy or import as `STB_GLOBAL`/`STB_WEAK`.
#[must_use]
pub fn gnu_symbols(plan: &DynsymPlan, t: &DynsymInputs<'_>) -> bool {
    plan.hashed().iter().any(|&k| {
        matches!(t.resolution.def.get(k), Some(Def::Defined(_) | Def::Common(_) | Def::Internal(_)))
            && t.src.program(k).is_some_and(|p| p.info >> 4 == STB_GNU_UNIQUE)
    })
}

/// `.dynsym`'s size (S-108): `entsize × (1 + entries)`.
#[must_use]
pub fn dynsym_size(plan: &DynsymPlan, class: Class) -> u64 {
    let entsize = match class {
        Class::Elf64 => 24,
        Class::Elf32 => 16,
    };
    entsize * u64::from(plan.count())
}

/// The sizes (S-108) of the chunks this package plans, for an output of
/// `class`: `.gnu.hash`, `.dynsym`, `.dynstr`, and `.gnu.version` with
/// `.gnu.version_r` when some entry is versioned (D-27).
///
/// # Errors
///
/// A refusal for an ELF32 output (i386 dynamic links are M4), or a
/// `.dynstr` of 4 GiB or more.
pub fn chunk_sizes(
    plan: &DynsymPlan,
    verneed: &crate::version::Verneed,
    dynstr: &DynStr,
    class: Class,
) -> Result<Vec<(crate::chunks::SynthKind, u64)>, SynthError> {
    chunk_sizes_versioned(plan, verneed, None, dynstr, class)
}

/// [`chunk_sizes`] for an output that may define versions (D-115):
/// `.gnu.version_d` with `.gnu.version` when `verdef` has entries.
///
/// # Errors
///
/// As [`chunk_sizes`].
pub fn chunk_sizes_versioned(
    plan: &DynsymPlan,
    verneed: &crate::version::Verneed,
    verdef: Option<&crate::version::VerdefPlan>,
    dynstr: &DynStr,
    class: Class,
) -> Result<Vec<(crate::chunks::SynthKind, u64)>, SynthError> {
    use crate::chunks::SynthKind as K;
    let verdefs = verdef.map_or(0, crate::version::VerdefPlan::count);
    elf64(class)?;
    if dynstr.size == u32::MAX {
        return Err(feature(".dynstr of 4 GiB or more").into());
    }
    let mut v = vec![
        (K::GnuHash, crate::gnuhash::GnuHashShape::of(plan, class).size()),
        (K::Dynsym, dynsym_size(plan, class)),
        (K::Dynstr, u64::from(dynstr.size)),
    ];
    if !verneed.is_empty() || verdefs != 0 {
        v.push((K::GnuVersion, crate::version::versym_size_with(plan, verneed, verdefs)));
    }
    if let Some(d) = verdef.filter(|d| d.count() != 0) {
        v.push((K::GnuVersionD, d.size()));
    }
    if !verneed.is_empty() {
        v.push((K::GnuVersionR, crate::version::verneed_size(verneed)));
    }
    Ok(v)
}

/// The `.dynsym` chunk (D-23): divisible, each entry a function of its
/// key's form, its `.dynstr` offset and the address oracle.
#[derive(Clone, Copy)]
pub struct DynsymChunk<'a> {
    pub plan: &'a DynsymPlan,
    pub inputs: DynsymInputs<'a>,
    pub dynstr: &'a DynStr,
    pub addrs: &'a dyn DynAddrs,
    pub machine: Machine,
}

impl std::fmt::Debug for DynsymChunk<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DynsymChunk").field("plan", self.plan).finish_non_exhaustive()
    }
}

impl DynsymChunk<'_> {
    /// The size (S-108).
    #[must_use]
    pub fn size(&self) -> u64 {
        dynsym_size(self.plan, self.machine.class())
    }

    /// Fills the window's range of `.dynsym`.
    ///
    /// # Errors
    ///
    /// As [`DynsymInputs::entry`]; a refusal for an ELF32 output.
    pub fn write_window(&self, out: &mut Window<'_>) -> Result<(), SynthError> {
        elf64(self.machine.class())?;
        let order = self.plan.order();
        if self.dynstr.symbols.len() != order.len() {
            return Err(internal(".dynstr's symbol offsets do not match .dynsym"));
        }
        let first = out.start() / 24;
        let last = out.end().div_ceil(24).min(u64::from(self.plan.count()));
        for i in first..last {
            let at = i * 24;
            if i == 0 {
                out.zero(0, 24);
                continue;
            }
            let k = order[i as usize - 1];
            let st_name = self.dynstr.symbols[i as usize - 1];
            let bytes = self.inputs.entry(k, st_name, self.machine, self.addrs)?;
            out.put(at, &bytes);
        }
        Ok(())
    }
}

impl SynthChunk for DynsymChunk<'_> {
    fn size(&self, _: &SizeCx<'_>) -> u64 {
        DynsymChunk::size(self)
    }

    fn division(&self) -> Division {
        Division::Divisible
    }

    fn write(&self, _: &FillCx<'_>, range: Range<u64>, out: &mut [u8]) -> Result<(), SynthError> {
        check_range(&range, out, DynsymChunk::size(self))?;
        self.write_window(&mut Window::new(range.start, out))
    }
}

/// `range` lies in a chunk of `size` bytes and `out` is its length.
pub(crate) fn check_range(range: &Range<u64>, out: &[u8], size: u64) -> Result<(), SynthError> {
    if range.start > range.end || range.end > size || out.len() as u64 != range.end - range.start {
        return Err(internal(format!(
            "a synthetic chunk of {size} bytes written at {}..{} into {} bytes",
            range.start,
            range.end,
            out.len()
        )));
    }
    Ok(())
}

/// A whole chunk: `fill` writes all of it into a buffer of `size` bytes
/// and the range is copied out (no buffer when the range is the chunk).
pub(crate) fn whole_range(
    range: &Range<u64>,
    out: &mut [u8],
    size: u64,
    fill: impl FnOnce(&mut [u8]) -> Result<(), SynthError>,
) -> Result<(), SynthError> {
    check_range(range, out, size)?;
    if range.start == 0 && range.end == size {
        return fill(out);
    }
    let mut buf = vec![0u8; size as usize];
    fill(&mut buf)?;
    out.copy_from_slice(&buf[range.start as usize..range.end as usize]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mortar_core::id::Id;

    // dyn.md D-22: the index of each entry counts from 1 (0 is the null
    // entry); a fixture plan rejects a key given twice.
    #[test]
    fn a_fixture_plan_and_its_index() {
        let p = DynsymPlan::new(vec![SymbolId(7), SymbolId(2), SymbolId(5)].into(), 1, 1).unwrap();
        assert_eq!(
            (p.index(SymbolId(7)), p.index(SymbolId(5)), p.index(SymbolId(3))),
            (Some(1), Some(3), None)
        );
        assert_eq!(p.keys().map(Id::index).collect::<Vec<_>>(), [2, 5, 7]);
        assert_eq!(p.hashed(), [SymbolId(2), SymbolId(5)]);
        assert_eq!(p.count(), 4);
        assert!(DynsymPlan::new(vec![SymbolId(1), SymbolId(1)].into(), 0, 1).is_none());
        assert!(DynsymPlan::new(vec![SymbolId(1)].into(), 2, 1).is_none());
    }
}

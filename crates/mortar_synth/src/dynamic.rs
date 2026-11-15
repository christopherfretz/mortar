//! `.dynamic` (`spec/dyn.md` D-30, D-1, D-2; `spec/synth.md` S-105,
//! S-108).
//!
//! **Plan** ([`DynamicPlan::new`], phase 80): the tag list as a pure
//! function of [`DynamicInputs`], facts the plan's callers know before any
//! address (which chunks and output sections exist and whether they are
//! empty, counts from `Slots`, the command line). Each D-30 row is present
//! exactly under its condition and in D-30's order (GNU ld's), then six
//! `DT_NULL`s (row 32): the terminator and five spare entries. The size,
//! `(entries + 6) × (16, ELF32 8)`, is therefore known before layout.
//!
//! **Write** ([`DynamicPlan::write`], image): any byte range, each entry's
//! value from a resolver of [`DynValue`]s. A value is a constant fixed
//! here or a reference the image answers once addresses are known: the
//! address or size of a chunk or an output section, a symbol's address, a
//! `.dynstr` offset from `Slots.dynstr` (D-24; this module never builds a
//! string). `mortar_image::dynamic` is that resolver.
//!
//! The rows, with what each reads (D-30's numbering):
//!
//! | # | tag | present when ([`DynamicInputs`]) | value ([`DynValue`]) |
//! |---|---|---|---|
//! | 1 | `DT_NEEDED` × n | `needed` (`Slots.needed.len()`) | `NeededName(i)` |
//! | 2 | `DT_RUNPATH` / `DT_RPATH` | `runpath` | `RunpathName` |
//! | 3-4 | `DT_INIT`, `DT_FINI` | `init`, `fini` | `Symbol` |
//! | 5-10 | the three arrays and their sizes | `preinit_array`, `init_array`, `fini_array` | `SectionAddr`, `SectionSize` |
//! | 11 | `DT_GNU_HASH` | always | `ChunkAddr(GnuHash)` |
//! | 12-15 | `DT_STRTAB`, `DT_SYMTAB`, `DT_STRSZ`, `DT_SYMENT` | always | `ChunkAddr(Dynstr)`, `ChunkAddr(Dynsym)`, `DynstrSize`, 24 (16) |
//! | 16 | `DT_DEBUG` | always | 0 |
//! | 17 | `DT_PLTGOT` | `got_plt` | `ChunkAddr(GotPlt)` |
//! | 18-20 | `DT_PLTRELSZ`, `DT_PLTREL`, `DT_JMPREL` | `rela_plt` | `ChunkSize(RelaPlt)`, `DT_RELA` (`DT_REL`), `ChunkAddr(RelaPlt)` |
//! | 21-23 | `DT_RELA`, `DT_RELASZ`, `DT_RELAENT` (REL: `DT_REL*`) | `rela_dyn` | `ChunkAddr(RelaDyn)`, `ChunkSize(RelaDyn)`, entsize |
//! | 24-25 | the architecture's tags (`Arch::dyn_tags`) | each one's `TagWhen` | its value |
//! | 26 | `DT_FLAGS` | `bind_now` | `DF_BIND_NOW` |
//! | 27 | `DT_FLAGS_1` | `Pie`, `StaticPie` or `bind_now` | `DF_1_PIE`, `DF_1_NOW` |
//! | 28-30 | `DT_VERNEED`, `DT_VERNEEDNUM`, `DT_VERSYM` | `verneed` | `ChunkAddr(GnuVersionR)`, the count, `ChunkAddr(GnuVersion)` |
//! | 31 | `DT_RELACOUNT` (`DT_RELCOUNT`) | `relative` | the count |
//! | 32 | `DT_NULL` × 6 | always | 0 |
//!
//! `StaticPie` (D-1, D-2) takes the same rows under the same conditions:
//! with no shared input there is no `DT_NEEDED`, version or `JUMP_SLOT`,
//! and `DT_FLAGS_1` holds `DF_1_PIE`.

use mortar_arch::{DynTag, RelForm, TagWhen};
use mortar_core::id::SymbolId;
use mortar_elf::Class;
use mortar_elf::consts::{
    DF_1_NOW, DF_1_PIE, DF_BIND_NOW, DT_DEBUG, DT_FINI, DT_FINI_ARRAY, DT_FINI_ARRAYSZ, DT_FLAGS,
    DT_FLAGS_1, DT_GNU_HASH, DT_INIT, DT_INIT_ARRAY, DT_INIT_ARRAYSZ, DT_JMPREL, DT_NEEDED,
    DT_PLTGOT, DT_PLTREL, DT_PLTRELSZ, DT_PREINIT_ARRAY, DT_PREINIT_ARRAYSZ, DT_REL, DT_RELA,
    DT_RELACOUNT, DT_RELAENT, DT_RELASZ, DT_RELCOUNT, DT_RELENT, DT_RELSZ, DT_RPATH, DT_RUNPATH,
    DT_STRSZ, DT_STRTAB, DT_SYMENT, DT_SYMTAB, DT_VERNEED, DT_VERNEEDNUM, DT_VERSYM,
};

use crate::chunks::SynthKind;
use crate::error::{SynthError, internal};
use crate::out::Window;

/// The number of `DT_NULL` entries that end `.dynamic` (D-30 row 32): the
/// terminator and five spares.
pub const NULLS: u64 = 6;

/// The output kinds that have a `.dynamic` (D-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DynKind {
    /// `-dynamic-linker` without `-pie`.
    Exec,
    /// `-pie -dynamic-linker`.
    Pie,
    /// `-pie` without an interpreter (D-2).
    StaticPie,
}

/// Which tag names the run path (D-30 row 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunpathTag {
    /// `DT_RUNPATH`: `--enable-new-dtags`, the default (cli §3.6).
    Runpath,
    /// `DT_RPATH`: `--disable-new-dtags`.
    Rpath,
}

impl RunpathTag {
    /// The tag for the command line's `new_dtags` (`None`: the default,
    /// `DT_RUNPATH`; D-30 row 2, `spec/dyn.md` §3.1).
    #[must_use]
    pub fn of(new_dtags: Option<bool>) -> Self {
        if new_dtags == Some(false) { Self::Rpath } else { Self::Runpath }
    }

    /// The `DT_*` value.
    #[must_use]
    pub fn tag(self) -> i64 {
        match self {
            Self::Runpath => DT_RUNPATH,
            Self::Rpath => DT_RPATH,
        }
    }
}

/// The run path's string (D-30 row 2): the `-rpath`/`-R` directories in
/// argument order (input §4.8) joined by `:`, each as written (`$ORIGIN`
/// kept, no deduplication: GNU ld 2.44 writes `-rpath /x -rpath /y:/x
/// -rpath /x` as `/x:/y:/x`, measured by WP-045). `None` for no
/// directory. This is the string `dynstr_plan` places (D-24) and the map's
/// `runpath`/`rpath` record holds.
#[must_use]
pub fn join_rpath<P: AsRef<[u8]>>(paths: &[P]) -> Option<Vec<u8>> {
    let (first, rest) = paths.split_first()?;
    let mut s = first.as_ref().to_vec();
    for p in rest {
        s.push(b':');
        s.extend_from_slice(p.as_ref());
    }
    Some(s)
}

/// An output section whose address and size `.dynamic` names (D-30 rows
/// 5-10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ArraySection {
    PreinitArray,
    InitArray,
    FiniArray,
}

impl ArraySection {
    /// The output section's name.
    #[must_use]
    pub fn name(self) -> &'static [u8] {
        match self {
            Self::PreinitArray => b".preinit_array",
            Self::InitArray => b".init_array",
            Self::FiniArray => b".fini_array",
        }
    }
}

/// What `.dynamic`'s plan is a function of: facts known before addresses
/// (D-30: "each is present exactly under its condition, which depends on
/// no address"). The join builds it from the plan, `Slots`, the chunk
/// list and the architecture.
#[allow(clippy::struct_excessive_bools, reason = "one field per condition of D-30")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DynamicInputs {
    pub kind: DynKind,
    pub class: Class,
    /// REL (i386) or RELA (`Facts.rel_form`).
    pub rel_form: RelForm,
    /// `Slots.needed.len()` (row 1, D-19a order).
    pub needed: u32,
    /// Some `-rpath`/`-R` was given (row 2), and its tag.
    pub runpath: Option<RunpathTag>,
    /// Row 3: `-init=SYM` when it names a defined symbol, else `_init`
    /// when it is defined (crti); `None` for neither.
    pub init: Option<SymbolId>,
    /// Row 4: `-fini=SYM`, else `_fini`, as [`Self::init`].
    pub fini: Option<SymbolId>,
    /// Rows 5-10: the output section exists.
    pub preinit_array: bool,
    pub init_array: bool,
    pub fini_array: bool,
    /// Row 17: `.got.plt` exists (some PLT or IPLT entry).
    pub got_plt: bool,
    /// Rows 18-20: `.rela.plt` (`.rel.plt`) is not empty.
    pub rela_plt: bool,
    /// Rows 21-23: `.rela.dyn` (`.rel.dyn`) is not empty.
    pub rela_dyn: bool,
    /// Row 31: the number of `RELATIVE` records (they lead `.rela.dyn`,
    /// D-40); 0 for none.
    pub relative: u64,
    /// Rows 28-30: the number of `Verneed` entries (`Slots.verneed.len()`);
    /// 0 without `.gnu.version_r`.
    pub verneed: u32,
    /// `-z now` (`OutputCx.bind_now`): rows 26-27.
    pub bind_now: bool,
    /// Rows 24-25: `Arch::dyn_tags` for the output's `OutputCx`.
    pub arch_tags: &'static [DynTag],
    /// [`TagWhen::BtiProperty`]: the output has the BTI property
    /// (a64 §10.3).
    pub bti_property: bool,
    /// [`TagWhen::VariantPcsJumpSlot`]: some `JUMP_SLOT` symbol is
    /// `STO_AARCH64_VARIANT_PCS` (a64 §10.3).
    pub variant_pcs_jump_slot: bool,
}

/// An entry's value, as the plan knows it: a constant, or a reference the
/// image resolves once addresses are known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DynValue {
    /// Fixed by the plan.
    Const(u64),
    /// The `.dynstr` offset of needed object `i`'s name
    /// (`Slots.dynstr.needed[i]`).
    NeededName(u32),
    /// The `.dynstr` offset of the run path (`Slots.dynstr.runpath`).
    RunpathName,
    /// `.dynstr`'s size (`Slots.dynstr.size`).
    DynstrSize,
    /// A synthetic chunk's address.
    ChunkAddr(SynthKind),
    /// A synthetic chunk's size.
    ChunkSize(SynthKind),
    /// An output section's address.
    SectionAddr(ArraySection),
    /// An output section's size (layout L-86).
    SectionSize(ArraySection),
    /// A symbol's address.
    Symbol(SymbolId),
}

/// One `.dynamic` entry before addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DynEntry {
    pub tag: i64,
    pub value: DynValue,
}

/// `.dynamic`'s plan: the entries before the `DT_NULL`s, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DynamicPlan {
    class: Class,
    entries: Box<[DynEntry]>,
}

/// The entry size of a relocation record form.
fn rel_entsize(class: Class, rel: RelForm) -> u64 {
    match (rel, class) {
        (RelForm::Rel, Class::Elf32) => 8,
        (RelForm::Rel, Class::Elf64) => 16,
        (RelForm::Rela, Class::Elf32) => 12,
        (RelForm::Rela, Class::Elf64) => 24,
    }
}

impl DynamicPlan {
    /// D-30's rows for `inputs`.
    ///
    /// # Errors
    ///
    /// `internal-error` when the inputs contradict each other (a fact the
    /// tables guarantee): a `StaticPie` with a needed object or a version
    /// need (input §8.4 refuses its shared inputs); more `Verneed`s than
    /// needed objects (D-27); `RELATIVE` records in an empty `.rela.dyn`;
    /// a `.rela.plt` without `.got.plt`.
    pub fn new(inputs: &DynamicInputs) -> Result<Self, SynthError> {
        let i = inputs;
        if i.kind == DynKind::StaticPie && (i.needed != 0 || i.verneed != 0) {
            return Err(internal("a static PIE with DT_NEEDED or version needs"));
        }
        if i.verneed > i.needed {
            return Err(internal(".gnu.version_r names more objects than DT_NEEDED"));
        }
        if i.relative != 0 && !i.rela_dyn {
            return Err(internal("RELATIVE records counted in an empty .rela.dyn"));
        }
        if i.rela_plt && !i.got_plt {
            return Err(internal(".rela.plt without .got.plt"));
        }
        let rel = i.rel_form == RelForm::Rel;
        let mut e: Vec<DynEntry> = Vec::with_capacity(40 + i.needed as usize);
        let mut put = |tag: i64, value: DynValue| e.push(DynEntry { tag, value });
        // Row 1.
        for n in 0..i.needed {
            put(DT_NEEDED, DynValue::NeededName(n));
        }
        // Row 2.
        if let Some(t) = i.runpath {
            put(t.tag(), DynValue::RunpathName);
        }
        // Rows 3-4.
        if let Some(s) = i.init {
            put(DT_INIT, DynValue::Symbol(s));
        }
        if let Some(s) = i.fini {
            put(DT_FINI, DynValue::Symbol(s));
        }
        // Rows 5-10.
        for (present, sec, at, size) in [
            (i.preinit_array, ArraySection::PreinitArray, DT_PREINIT_ARRAY, DT_PREINIT_ARRAYSZ),
            (i.init_array, ArraySection::InitArray, DT_INIT_ARRAY, DT_INIT_ARRAYSZ),
            (i.fini_array, ArraySection::FiniArray, DT_FINI_ARRAY, DT_FINI_ARRAYSZ),
        ] {
            if present {
                put(at, DynValue::SectionAddr(sec));
                put(size, DynValue::SectionSize(sec));
            }
        }
        // Rows 11-16.
        put(DT_GNU_HASH, DynValue::ChunkAddr(SynthKind::GnuHash));
        put(DT_STRTAB, DynValue::ChunkAddr(SynthKind::Dynstr));
        put(DT_SYMTAB, DynValue::ChunkAddr(SynthKind::Dynsym));
        put(DT_STRSZ, DynValue::DynstrSize);
        put(DT_SYMENT, DynValue::Const(if i.class == Class::Elf64 { 24 } else { 16 }));
        put(DT_DEBUG, DynValue::Const(0));
        // Row 17.
        if i.got_plt {
            put(DT_PLTGOT, DynValue::ChunkAddr(SynthKind::GotPlt));
        }
        // Rows 18-20.
        if i.rela_plt {
            put(DT_PLTRELSZ, DynValue::ChunkSize(SynthKind::RelaPlt));
            put(DT_PLTREL, DynValue::Const(if rel { DT_REL } else { DT_RELA } as u64));
            put(DT_JMPREL, DynValue::ChunkAddr(SynthKind::RelaPlt));
        }
        // Rows 21-23.
        if i.rela_dyn {
            let (at, size, ent) =
                if rel { (DT_REL, DT_RELSZ, DT_RELENT) } else { (DT_RELA, DT_RELASZ, DT_RELAENT) };
            put(at, DynValue::ChunkAddr(SynthKind::RelaDyn));
            put(size, DynValue::ChunkSize(SynthKind::RelaDyn));
            put(ent, DynValue::Const(rel_entsize(i.class, i.rel_form)));
        }
        // Rows 24-25: the architecture's, in its order.
        for t in i.arch_tags {
            let present = match t.when {
                TagWhen::BtiProperty => i.bti_property,
                TagWhen::VariantPcsJumpSlot => i.variant_pcs_jump_slot,
            };
            if present {
                put(t.tag, DynValue::Const(t.value));
            }
        }
        // Rows 26-27.
        if i.bind_now {
            put(DT_FLAGS, DynValue::Const(DF_BIND_NOW));
        }
        let pie = matches!(i.kind, DynKind::Pie | DynKind::StaticPie);
        let flags_1 = if pie { DF_1_PIE } else { 0 } | if i.bind_now { DF_1_NOW } else { 0 };
        if flags_1 != 0 {
            put(DT_FLAGS_1, DynValue::Const(flags_1));
        }
        // Rows 28-30.
        if i.verneed != 0 {
            put(DT_VERNEED, DynValue::ChunkAddr(SynthKind::GnuVersionR));
            put(DT_VERNEEDNUM, DynValue::Const(u64::from(i.verneed)));
            put(DT_VERSYM, DynValue::ChunkAddr(SynthKind::GnuVersion));
        }
        // Row 31.
        if i.relative != 0 {
            put(if rel { DT_RELCOUNT } else { DT_RELACOUNT }, DynValue::Const(i.relative));
        }
        Ok(Self { class: i.class, entries: e.into() })
    }

    /// The entries before the `DT_NULL`s, in order.
    #[must_use]
    pub fn entries(&self) -> &[DynEntry] {
        &self.entries
    }

    /// The size of one entry: 16, ELF32 8.
    #[must_use]
    pub fn entsize(&self) -> u64 {
        if self.class == Class::Elf64 { 16 } else { 8 }
    }

    /// The size (S-108): `(entries + 6) × entsize`.
    #[must_use]
    pub fn size(&self) -> u64 {
        (self.entries.len() as u64 + NULLS) * self.entsize()
    }

    /// Writes the window's range of `.dynamic`: the entries meeting it,
    /// each value from `value`, and the `DT_NULL`s. O(range).
    ///
    /// # Errors
    ///
    /// Whatever `value` returns; `internal-error` for an ELF32 value that
    /// does not fit 32 bits.
    pub fn write(
        &self,
        value: &dyn Fn(DynValue) -> Result<u64, SynthError>,
        w: &mut Window<'_>,
    ) -> Result<(), SynthError> {
        let ent = self.entsize();
        let half = (ent / 2) as u8;
        let first = w.start() / ent;
        let last = w.end().div_ceil(ent).min(self.entries.len() as u64);
        for k in first..last {
            let e = self.entries[k as usize];
            let v = match e.value {
                DynValue::Const(c) => c,
                other => value(other)?,
            };
            if half == 4 && v > u64::from(u32::MAX) {
                return Err(internal(format!(".dynamic value {v:#x} of tag {:#x}", e.tag)));
            }
            let at = k * ent;
            // An ELF32 tag is an `Elf32_Sword`: the low 32 bits.
            w.word(at, half, e.tag as u64);
            w.word(at + u64::from(half), half, v);
        }
        let nulls = self.entries.len() as u64 * ent;
        w.zero(nulls, NULLS * ent);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mortar_elf::consts::{DT_AARCH64_BTI_PLT, DT_AARCH64_VARIANT_PCS};

    const A64_TAGS: &[DynTag] = &[
        DynTag { tag: DT_AARCH64_BTI_PLT, value: 0, when: TagWhen::BtiProperty },
        DynTag { tag: DT_AARCH64_VARIANT_PCS, value: 0, when: TagWhen::VariantPcsJumpSlot },
    ];

    fn base(kind: DynKind) -> DynamicInputs {
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

    fn tags(i: &DynamicInputs) -> Vec<i64> {
        DynamicPlan::new(i).unwrap().entries().iter().map(|e| e.tag).collect()
    }

    // D-30: the rows that are always present, alone, in an `Exec`.
    #[test]
    fn the_minimum() {
        let p = DynamicPlan::new(&base(DynKind::Exec)).unwrap();
        let t: Vec<i64> = p.entries().iter().map(|e| e.tag).collect();
        assert_eq!(t, [DT_GNU_HASH, DT_STRTAB, DT_SYMTAB, DT_STRSZ, DT_SYMENT, DT_DEBUG]);
        assert_eq!(p.size(), (6 + 6) * 16);
        assert_eq!(p.entries()[4].value, DynValue::Const(24));
        assert_eq!(p.entries()[5].value, DynValue::Const(0));
    }

    // D-30: every row at once, in the table's order.
    #[test]
    fn every_row_in_order() {
        let mut i = base(DynKind::Pie);
        i.needed = 2;
        i.runpath = Some(RunpathTag::Runpath);
        i.init = Some(SymbolId(3));
        i.fini = Some(SymbolId(4));
        i.preinit_array = true;
        i.init_array = true;
        i.fini_array = true;
        i.got_plt = true;
        i.rela_plt = true;
        i.rela_dyn = true;
        i.relative = 7;
        i.verneed = 1;
        i.bind_now = true;
        i.bti_property = true;
        i.variant_pcs_jump_slot = true;
        let p = DynamicPlan::new(&i).unwrap();
        let t: Vec<i64> = p.entries().iter().map(|e| e.tag).collect();
        assert_eq!(
            t,
            [
                DT_NEEDED,
                DT_NEEDED,
                DT_RUNPATH,
                DT_INIT,
                DT_FINI,
                DT_PREINIT_ARRAY,
                DT_PREINIT_ARRAYSZ,
                DT_INIT_ARRAY,
                DT_INIT_ARRAYSZ,
                DT_FINI_ARRAY,
                DT_FINI_ARRAYSZ,
                DT_GNU_HASH,
                DT_STRTAB,
                DT_SYMTAB,
                DT_STRSZ,
                DT_SYMENT,
                DT_DEBUG,
                DT_PLTGOT,
                DT_PLTRELSZ,
                DT_PLTREL,
                DT_JMPREL,
                DT_RELA,
                DT_RELASZ,
                DT_RELAENT,
                DT_AARCH64_BTI_PLT,
                DT_AARCH64_VARIANT_PCS,
                DT_FLAGS,
                DT_FLAGS_1,
                DT_VERNEED,
                DT_VERNEEDNUM,
                DT_VERSYM,
                DT_RELACOUNT,
            ]
        );
        let v = |tag| p.entries().iter().find(|e| e.tag == tag).unwrap().value;
        assert_eq!(v(DT_NEEDED), DynValue::NeededName(0));
        assert_eq!(p.entries()[1].value, DynValue::NeededName(1));
        assert_eq!(v(DT_FLAGS), DynValue::Const(DF_BIND_NOW));
        assert_eq!(v(DT_FLAGS_1), DynValue::Const(DF_1_PIE | DF_1_NOW));
        assert_eq!(v(DT_PLTREL), DynValue::Const(DT_RELA as u64));
        assert_eq!(v(DT_RELAENT), DynValue::Const(24));
        assert_eq!(v(DT_VERNEEDNUM), DynValue::Const(1));
        assert_eq!(v(DT_RELACOUNT), DynValue::Const(7));
        assert_eq!(v(DT_INIT), DynValue::Symbol(SymbolId(3)));
        assert_eq!(v(DT_INIT_ARRAYSZ), DynValue::SectionSize(ArraySection::InitArray));
        assert_eq!(p.size(), (32 + 6) * 16);
    }

    // D-30 rows 26-27 and D-1's `DT_FLAGS_1` row, per kind and `-z now`.
    #[test]
    fn flags_per_kind() {
        let flags = |kind, now| {
            let mut i = base(kind);
            i.bind_now = now;
            let p = DynamicPlan::new(&i).unwrap();
            let f = |tag| p.entries().iter().find(|e| e.tag == tag).map(|e| e.value);
            (f(DT_FLAGS), f(DT_FLAGS_1))
        };
        let c = |x| Some(DynValue::Const(x));
        assert_eq!(flags(DynKind::Exec, false), (None, None));
        assert_eq!(flags(DynKind::Exec, true), (c(DF_BIND_NOW), c(DF_1_NOW)));
        assert_eq!(flags(DynKind::Pie, false), (None, c(DF_1_PIE)));
        assert_eq!(flags(DynKind::Pie, true), (c(DF_BIND_NOW), c(DF_1_PIE | DF_1_NOW)));
        assert_eq!(flags(DynKind::StaticPie, false), (None, c(DF_1_PIE)));
        assert_eq!(flags(DynKind::StaticPie, true), (c(DF_BIND_NOW), c(DF_1_PIE | DF_1_NOW)));
    }

    // D-30 rows 2, 18-23, 31 on i386 (REL, ELF32) and `--disable-new-dtags`.
    #[test]
    fn rel_and_rpath() {
        let mut i = base(DynKind::Pie);
        i.class = Class::Elf32;
        i.rel_form = RelForm::Rel;
        i.arch_tags = &[];
        i.runpath = Some(RunpathTag::of(Some(false)));
        i.got_plt = true;
        i.rela_plt = true;
        i.rela_dyn = true;
        i.relative = 1;
        let p = DynamicPlan::new(&i).unwrap();
        let t: Vec<i64> = p.entries().iter().map(|e| e.tag).collect();
        assert_eq!(
            t,
            [
                DT_RPATH,
                DT_GNU_HASH,
                DT_STRTAB,
                DT_SYMTAB,
                DT_STRSZ,
                DT_SYMENT,
                DT_DEBUG,
                DT_PLTGOT,
                DT_PLTRELSZ,
                DT_PLTREL,
                DT_JMPREL,
                DT_REL,
                DT_RELSZ,
                DT_RELENT,
                DT_FLAGS_1,
                DT_RELCOUNT,
            ]
        );
        let v = |tag| p.entries().iter().find(|e| e.tag == tag).unwrap().value;
        assert_eq!(v(DT_SYMENT), DynValue::Const(16));
        assert_eq!(v(DT_RELENT), DynValue::Const(8));
        assert_eq!(v(DT_PLTREL), DynValue::Const(DT_REL as u64));
        assert_eq!(p.entsize(), 8);
        assert_eq!(p.size(), (16 + 6) * 8);
        assert_eq!(RunpathTag::of(None), RunpathTag::Runpath);
        assert_eq!(RunpathTag::of(Some(true)), RunpathTag::Runpath);
    }

    // D-30 rows 24-25: each architecture tag under its own condition.
    #[test]
    fn arch_tags_per_condition() {
        for (bti, vpcs) in [(false, false), (true, false), (false, true), (true, true)] {
            let mut i = base(DynKind::Exec);
            i.bti_property = bti;
            i.variant_pcs_jump_slot = vpcs;
            let t = tags(&i);
            assert_eq!(t.contains(&DT_AARCH64_BTI_PLT), bti);
            assert_eq!(t.contains(&DT_AARCH64_VARIANT_PCS), vpcs);
        }
        let mut i = base(DynKind::Exec);
        i.arch_tags = &[];
        i.bti_property = true;
        i.variant_pcs_jump_slot = true;
        assert_eq!(tags(&i).len(), 6);
    }

    // Each row's condition alone adds exactly its rows.
    #[test]
    fn each_condition_alone() {
        let n = |i: &DynamicInputs| tags(i).len();
        let b = n(&base(DynKind::Exec));
        let one = |f: &dyn Fn(&mut DynamicInputs)| {
            let mut i = base(DynKind::Exec);
            f(&mut i);
            n(&i) - b
        };
        assert_eq!(one(&|i| i.needed = 3), 3);
        assert_eq!(one(&|i| i.runpath = Some(RunpathTag::Runpath)), 1);
        assert_eq!(one(&|i| i.init = Some(SymbolId(1))), 1);
        assert_eq!(one(&|i| i.fini = Some(SymbolId(1))), 1);
        assert_eq!(one(&|i| i.preinit_array = true), 2);
        assert_eq!(one(&|i| i.init_array = true), 2);
        assert_eq!(one(&|i| i.fini_array = true), 2);
        assert_eq!(one(&|i| i.got_plt = true), 1);
        assert_eq!(
            one(&|i| {
                i.got_plt = true;
                i.rela_plt = true;
            }),
            4
        );
        assert_eq!(one(&|i| i.rela_dyn = true), 3);
        assert_eq!(
            one(&|i| {
                i.rela_dyn = true;
                i.relative = 2;
            }),
            4
        );
        assert_eq!(
            one(&|i| {
                i.needed = 1;
                i.verneed = 1;
            }),
            4
        );
        assert_eq!(one(&|i| i.bind_now = true), 2);
    }

    // Contradictory inputs are internal errors, not outputs.
    #[test]
    fn contradictions() {
        let bad = |f: &dyn Fn(&mut DynamicInputs)| {
            let mut i = base(DynKind::StaticPie);
            f(&mut i);
            DynamicPlan::new(&i).is_err()
        };
        assert!(bad(&|i| i.needed = 1));
        assert!(bad(&|i| {
            i.kind = DynKind::Exec;
            i.verneed = 1;
        }));
        assert!(bad(&|i| i.relative = 1));
        assert!(bad(&|i| i.rela_plt = true));
        assert!(!bad(&|_| {}));
    }

    // The write: any split of the chunk gives the same bytes (I-103);
    // values come from the resolver; the six DT_NULLs are zero.
    #[test]
    fn write_any_split() {
        for class in [Class::Elf64, Class::Elf32] {
            let mut i = base(DynKind::Pie);
            i.class = class;
            i.needed = 1;
            i.rela_dyn = true;
            i.relative = 3;
            let p = DynamicPlan::new(&i).unwrap();
            let value = |v: DynValue| -> Result<u64, SynthError> {
                Ok(match v {
                    DynValue::NeededName(0) => 1,
                    DynValue::DynstrSize => 0x55,
                    DynValue::ChunkAddr(k) => 0x1000 + u64::from(k.number()),
                    DynValue::ChunkSize(k) => 0x10 * u64::from(k.number()),
                    _ => return Err(internal("unexpected")),
                })
            };
            let size = p.size() as usize;
            let mut whole = vec![0xaa; size];
            p.write(&value, &mut Window::new(0, &mut whole)).unwrap();
            for cut in [1, 3, 8, 13, 16, 17, size - 1] {
                let mut head = vec![0xbb; cut];
                let mut tail = vec![0xcc; size - cut];
                p.write(&value, &mut Window::new(0, &mut head)).unwrap();
                p.write(&value, &mut Window::new(cut as u64, &mut tail)).unwrap();
                head.extend_from_slice(&tail);
                assert_eq!(head, whole, "{class:?} cut at {cut}");
            }
            let ent = p.entsize() as usize;
            let count = p.entries().len();
            assert!(whole[count * ent..].iter().all(|&byte| byte == 0));
            let word = |at: usize| -> u64 {
                let half = ent / 2;
                let mut bytes = [0u8; 8];
                bytes[..half].copy_from_slice(&whole[at..at + half]);
                u64::from_le_bytes(bytes)
            };
            assert_eq!(word(0), DT_NEEDED as u64);
            assert_eq!(word(ent / 2), 1);
            let flags1 = p.entries().iter().position(|e| e.tag == DT_FLAGS_1).unwrap();
            assert_eq!(word(flags1 * ent) as u32, DT_FLAGS_1 as u32);
            assert_eq!(word(flags1 * ent + ent / 2), DF_1_PIE);
        }
    }

    // D-30 row 2: joined by `:` in order, as written, no deduplication.
    #[test]
    fn the_run_path_string() {
        let none: [&[u8]; 0] = [];
        assert_eq!(join_rpath(&none), None);
        assert_eq!(join_rpath(&[b"/x"]).unwrap(), b"/x");
        let p: [&[u8]; 3] = [b"/x", b"/y:$ORIGIN/../lib", b"/x"];
        assert_eq!(join_rpath(&p).unwrap(), b"/x:/y:$ORIGIN/../lib:/x");
    }

    // ELF32: a value over 32 bits is an internal error, not a truncation.
    #[test]
    fn elf32_overflow() {
        let mut i = base(DynKind::Exec);
        i.class = Class::Elf32;
        let p = DynamicPlan::new(&i).unwrap();
        let mut buf = vec![0; p.size() as usize];
        let r = p.write(&|_| Ok(1 << 32), &mut Window::new(0, &mut buf));
        assert!(r.is_err());
    }
}

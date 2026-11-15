//! Copy allocations (`spec/dyn.md` D-42): one per `(shared FileId,
//! st_value)` that a `COPY` demand reaches, its size, alignment and chunk,
//! and the key its `COPY` record names.
//!
//! The plan ([`copy_plan`]) runs before addresses, from `Demands` and the
//! shared definitions ([`CopyDefs`]; [`InputCopyDefs`] reads them from the
//! loaded shared objects). It gives `Slots.copies`, `Slots.copy_of` and,
//! per chunk, each allocation's offset and the chunk's size (the order
//! inside a chunk is by the least `SymbolId` among the allocation's keys).
//! The scan refused every demand D-18 and D-42 refuse (protected, size 0;
//! a TLS definition never reaches here, arch.md §4.3's TLS rule): meeting
//! one here is an internal error.

use std::collections::BTreeMap;

use mortar_core::demands::{DemandFlags, Demands};
use mortar_core::inputs::{Inputs, Reader};
use mortar_core::layout::{CopyAlloc, CopyPlace};
use mortar_core::resolution::{Def, Resolution};
use mortar_core::{FileId, SymKey, SymbolId};
use mortar_elf::consts::{PF_W, PT_GNU_RELRO, PT_LOAD, STT_FUNC, STT_GNU_IFUNC, STT_TLS, STV_PROTECTED};
use mortar_elf::{SectionIndex, SymbolIndex};

use crate::error::{SynthError, internal};

/// What D-42 reads of the shared definition of a key with a `COPY`
/// demand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyDef {
    pub file: FileId,
    pub value: u64,
    pub size: u64,
    /// `sh_addralign` of the defining section; `None` when the shared
    /// object has no section headers (the alignment is then `st_value`'s).
    pub section_align: Option<u64>,
    /// The defining address lies in a `PT_GNU_RELRO` or a non-writable
    /// `PT_LOAD` of the shared object (lld's `isReadOnly`).
    pub read_only: bool,
    pub sym_type: u8,
    pub visibility: u8,
}

/// The shared definitions of the keys with a `COPY` demand.
pub trait CopyDefs {
    /// `key`'s shared definition; `None` when it has none (an internal
    /// error for a key with a `COPY` demand).
    fn copy_def(&self, key: SymbolId) -> Option<CopyDef>;
}

/// [`CopyDefs`] over the loaded inputs and the resolution
/// (`Def::Shared`).
#[derive(Clone, Copy)]
pub struct InputCopyDefs<'a, 's> {
    pub inputs: &'a Inputs<'s>,
    pub resolution: &'a Resolution,
}

impl std::fmt::Debug for InputCopyDefs<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InputCopyDefs").finish_non_exhaustive()
    }
}

impl CopyDefs for InputCopyDefs<'_, '_> {
    fn copy_def(&self, key: SymbolId) -> Option<CopyDef> {
        let Def::Shared(e) = *self.resolution.def.get(key)? else { return None };
        let Reader::Shared(so) = self.inputs.files.get(e.file)?.reader else { return None };
        let sym = so.symbols().ok()??.get(SymbolIndex(e.index.0)).ok()?;
        let value = sym.st_value;
        let sections = so.section_table();
        let section_align = (!sections.is_empty())
            .then(|| sections.get(SectionIndex(u32::from(sym.st_shndx))).ok()?.alignment().ok())
            .flatten();
        let phdrs = so.program_headers();
        let in_relro = phdrs.iter().any(|p| p.p_type() == PT_GNU_RELRO && p.contains(value));
        let ro_load = phdrs
            .load_containing(value)
            .is_some_and(|p| p.p_type() == PT_LOAD && p.flags() & PF_W == 0);
        Some(CopyDef {
            file: e.file,
            value,
            size: sym.st_size,
            section_align,
            read_only: in_relro || ro_load,
            sym_type: sym.sym_type(),
            visibility: sym.visibility(),
        })
    }
}

/// The plan of D-42.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CopyPlan {
    /// `Slots.copies`: sorted by `(file, value)`.
    pub copies: Box<[CopyAlloc]>,
    /// `Slots.copy_of`: every key with a `COPY` demand, sorted by key.
    pub copy_of: Box<[(SymbolId, u32)]>,
    /// Per allocation (as `copies`): its size, and its offset in its chunk.
    pub size: Box<[u64]>,
    pub offset: Box<[u64]>,
    /// The size and alignment of `.dynbss` and of `.bss.rel.ro`.
    pub dynbss: (u64, u64),
    pub relro: (u64, u64),
}

impl CopyPlan {
    /// The chunk-local order of `place`'s allocations (by the least
    /// `SymbolId` among each one's keys, D-42), as indices of `copies`.
    #[must_use]
    pub fn order(&self, place: CopyPlace) -> Vec<u32> {
        let mut v: Vec<u32> = (0..self.copies.len() as u32)
            .filter(|&i| self.copies[i as usize].place == place)
            .collect();
        v.sort_unstable_by_key(|&i| self.copies[i as usize].alias);
        v
    }
}

/// D-42's alignment: `min(sh_addralign, 2^ctz(st_value))`, a power of
/// two (`st_value` 0 and a missing section: the other term; both: 1).
#[must_use]
pub fn copy_align(section_align: Option<u64>, value: u64) -> u64 {
    let of_value = (value != 0).then(|| 1u64 << value.trailing_zeros());
    let of_section = section_align.map(|a| if a.is_power_of_two() { a } else { 1 });
    match (of_section, of_value) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) | (None, Some(a)) => a,
        (None, None) => 1,
    }
}

/// One `(file, value)` being planned: size, alignment, read-only, keys.
type Group = (u64, u64, bool, Vec<SymbolId>);

/// D-42's plan from the keys with a `COPY` demand.
pub fn copy_plan(demands: &Demands, defs: &dyn CopyDefs) -> Result<CopyPlan, SynthError> {
    // (file, value) -> (size, align, read_only, keys).
    let mut groups: BTreeMap<(FileId, u64), Group> = BTreeMap::new();
    for key in demands.keys_with(DemandFlags::COPY).filter_map(SymKey::as_global) {
        let d = defs.copy_def(key).ok_or_else(|| {
            internal::<SynthError>(format!("symbol {}: a COPY demand without a shared definition", key.0))
        })?;
        let refused = d.visibility == STV_PROTECTED
            || d.size == 0
            || matches!(d.sym_type, STT_TLS | STT_FUNC | STT_GNU_IFUNC);
        if refused {
            return Err(internal(format!(
                "symbol {}: a COPY demand the scan should have refused (spec/dyn.md D-18, D-42)",
                key.0
            )));
        }
        let g = groups.entry((d.file, d.value)).or_insert((0, u64::MAX, false, Vec::new()));
        g.0 = g.0.max(d.size);
        g.1 = g.1.min(copy_align(d.section_align, d.value));
        g.2 |= d.read_only;
        g.3.push(key);
    }
    let mut copies = Vec::with_capacity(groups.len());
    let mut size = Vec::with_capacity(groups.len());
    let mut copy_of = Vec::new();
    for (i, ((file, value), (sz, align, ro, keys))) in groups.into_iter().enumerate() {
        let alias = keys.iter().copied().min().unwrap_or(SymbolId(0));
        let place = if ro { CopyPlace::RelRo } else { CopyPlace::DynBss };
        copies.push(CopyAlloc { file, value, place, align, alias });
        size.push(sz);
        copy_of.extend(keys.into_iter().map(|k| (k, i as u32)));
    }
    copy_of.sort_unstable();
    let mut plan = CopyPlan {
        copies: copies.into(),
        copy_of: copy_of.into(),
        size: size.into(),
        offset: Box::default(),
        dynbss: (0, 1),
        relro: (0, 1),
    };
    let mut offset = vec![0u64; plan.copies.len()];
    for place in [CopyPlace::DynBss, CopyPlace::RelRo] {
        let (mut at, mut align) = (0u64, 1u64);
        for i in plan.order(place) {
            let c = &plan.copies[i as usize];
            at = at.next_multiple_of(c.align);
            offset[i as usize] = at;
            at += plan.size[i as usize];
            align = align.max(c.align);
        }
        match place {
            CopyPlace::DynBss => plan.dynbss = (at, align),
            CopyPlace::RelRo => plan.relro = (at, align),
        }
    }
    plan.offset = offset.into();
    Ok(plan)
}

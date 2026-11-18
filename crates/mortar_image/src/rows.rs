//! The rows the image's kernels read (WP-255b; `spec/landed/incr/WP-255.md`
//! §3 C', §9.5): a small read-only trait [`Rows`], the ONE formula for a
//! symbol's value over it ([`value`]), and the fill tables over it
//! ([`RowTables`]). The full link implements `Rows` over its `Link`
//! ([`FrontRows`]); an update implements it over the state (255c), and
//! both call the same kernels. Monomorphized: no `dyn Rows`.
//!
//! The trait's methods are the leaf reads of the formula: a datum each
//! (a chunk, a section's place, kind or flags, a symbol's definition, a
//! merge piece, a slot's index), never a second formula. What is a
//! formula (an address from a chunk and an offset, `S` of a key, a slot's
//! address, a piece's value) is a function of this module, generic over
//! `R`.

use std::ops::Deref;

use mortar_arch::{
    DemandFlags, FillTables, ProofRun, SlotKind, SymFacts, SymNames, SymValue, TargetKind, TlsBlock,
};
use mortar_core::inputs::Reader;
use mortar_core::layout::{Addresses, AllocKey, Chunk, ChunkKind, SlotIndex};
use mortar_core::place::NoPlace;
use mortar_core::resolution::{Def, SectionKind};
use mortar_core::{
    ChunkId, Column, FileId, FragId, SectionId, SymEntry, SymKey, SymKind, SymbolId,
};
use mortar_diag::Diagnostic;
use mortar_elf::{SectionIndex, SymbolSection, SymbolTable};
use mortar_merge::What;
use mortar_merge::refs::{self, PieceRef};
use mortar_synth::chunks::SynthKind;
use mortar_synth::got::{GotAddrs, GotPlan, Region};
use mortar_synth::{EhValue, EhWho, SynthError};

use crate::fill::{SplitPlace, Tables};
use crate::layout_plan;
use crate::link::Link;
use crate::plan::{Builder, InputUnit, RecRows, RelaTable, RelocSection};

const SHF_ALLOC: u64 = 0x2;
const SHF_EXECINSTR: u64 = 0x4;
const STT_SECTION: u8 = 3;

fn internal(what: impl Into<String>) -> Diagnostic {
    Diagnostic::error(mortar_diag::Kind::InternalError { what: what.into() })
}

/// The value of a definition (`spec/image.md` I-124; `spec/resolve.md`
/// §10.1 P-1; `spec/model.md` §4.5's invariant). There is deliberately no
/// accessor to an `Option` or a number: every consumer says what it does
/// with each case, so "no place" cannot be defaulted to 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Valued {
    /// Its address (for a non-alloc section, its offset, I-117; for
    /// `SHN_ABS`, `st_value`; for a weak undefined key, 0, resolve §5.4).
    At(u64),
    /// Not defined in this output: a strong undefined key (the scan
    /// refused its references, or a shared object supplies it at run
    /// time) or a shared definition without a copy. The static `S` of
    /// such a key is 0 (`spec/arch.md` §4.3).
    Import,
    /// Defined in a section without a place, by the named rule.
    NoPlace(NoPlace),
    /// A defined key the layout gave no place although its section has
    /// one: an internal fault, never a number.
    Missing,
}

/// What fill reads of a value (I-124): the address, if any, and whether
/// the target is live. `NoPlace` and `Missing` are never live, so fill
/// writes P-1's value for the referring section or stops; only `Import`
/// (and a split target, whose caller asks `split_target`) is live
/// without an address of its own, its `S` 0 by `spec/arch.md` §4.3.
pub(crate) fn fill_parts(value: Valued) -> (Option<u64>, bool) {
    match value {
        Valued::At(v) => (Some(v), true),
        Valued::Import => (None, true),
        Valued::NoPlace(_) | Valued::Missing => (None, false),
    }
}

/// One output section as its header gives it (L-86).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OutSection {
    pub addr: u64,
    pub offset: u64,
    pub size: u64,
}

impl OutSection {
    fn addr_or_offset(&self, alloc: bool) -> u64 {
        if alloc { self.addr } else { self.offset }
    }
}

/// The output sections by index (L-86): the first chunk's address and
/// offset, the size to the end of the last chunk's `size`.
#[must_use]
pub fn out_sections(chunks: &Column<ChunkId, Chunk>) -> Vec<OutSection> {
    let mut rows: Vec<Option<OutSection>> = Vec::new();
    let mut ends: Vec<u64> = Vec::new();
    for (_, c) in chunks.iter() {
        let Some(n) = c.shndx else { continue };
        let n = n as usize;
        if rows.len() <= n {
            rows.resize(n + 1, None);
            ends.resize(n + 1, 0);
        }
        let start = c.addr.unwrap_or(c.offset);
        let end = start + c.size;
        match &mut rows[n] {
            Some(s) => {
                let first = s.addr_or_offset(c.addr.is_some());
                if start < first {
                    s.addr = c.addr.unwrap_or(0);
                    s.offset = c.offset;
                }
                ends[n] = ends[n].max(end);
                let first = s.addr_or_offset(c.addr.is_some());
                s.size = ends[n] - first;
            }
            slot @ None => {
                *slot =
                    Some(OutSection { addr: c.addr.unwrap_or(0), offset: c.offset, size: c.size });
                ends[n] = end;
            }
        }
    }
    rows.into_iter().map(Option::unwrap_or_default).collect()
}

/// A symbol entry's definition as its input symbol table gives it:
/// `st_value`, `st_size`, the symbol's type, and where it is defined
/// (`Ok`: the input section; `Err`: `SHN_ABS` with its value; `None`: no
/// section, or a section the resolution does not hold).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryDef {
    pub value: u64,
    pub size: u64,
    pub sym_type: u8,
    pub place: Option<Result<SectionId, u64>>,
}

/// The leaf reads of the image's kernels (WP-255 §9.5). Each method is a
/// datum; the formulas over them are this module's generic functions.
pub trait Rows: Sync {
    // ---- the placement ----
    /// Every chunk (the placement's column).
    fn chunks(&self) -> &Column<ChunkId, Chunk>;
    /// Output section `shndx`'s header values (L-86).
    fn out_section(&self, shndx: u32) -> Option<OutSection>;
    // ---- input sections ----
    /// `(chunk, offset)` of a placed section.
    fn place(&self, s: SectionId) -> Option<(ChunkId, u64)>;
    /// The input header's `sh_size`.
    fn section_size(&self, s: SectionId) -> Option<u64>;
    /// The input header's `sh_flags` (0 when there is none).
    fn section_flags(&self, s: SectionId) -> u64;
    /// The resolution's kind of the section.
    fn section_kind(&self, s: SectionId) -> Option<SectionKind>;
    /// P-1: whether the section has a place (`resolve.md` §10.1).
    fn section_place(&self, s: SectionId) -> Result<(), NoPlace>;
    /// The flags of the OUTPUT section `s` is assigned to.
    fn out_flags(&self, s: SectionId) -> Option<u64>;
    /// I-47a: the keeper's section `k` that a dropped COMDAT copy's
    /// non-alloc section `d` is read in (every condition but `k`'s value).
    fn kept_copy(&self, d: SectionId) -> Option<SectionId>;
    // ---- merge pieces (S-13..S-22) and `.eh_frame` (S-50) ----
    /// S-19, S-22: the piece of a named symbol at `v` in `Merge` section
    /// `s`; `None` outside it (or without its size).
    fn named_piece(&self, s: SectionId, v: u64) -> Option<PieceRef>;
    /// The tables' own row of a `Merge` section (`SplitPlace::row`).
    fn split_row(&self, s: SectionId) -> Option<u32>;
    /// S-17, S-18: the piece of a section symbol of `s` plus `r`; `None`
    /// without the section's size.
    fn section_piece(&self, s: SectionId, row: u32, r: i64) -> Option<Result<PieceRef, What>>;
    /// Whether the fragment survived the mark (I-7).
    fn frag_live(&self, f: FragId) -> bool;
    /// The fragment's merged chunk and offset in it.
    fn frag_place(&self, f: FragId) -> Option<(ChunkId, u64)>;
    /// The fragment's offset in its merged chunk.
    fn frag_offset(&self, f: FragId) -> Option<u64>;
    /// The merged chunk of `Merge` section `s`'s output section.
    fn merged_chunk(&self, s: SectionId) -> Option<ChunkId>;
    /// S-18's refusal of `r` outside `Merge` section `s`.
    fn outside_merge(&self, s: SectionId, r: i64, w: &What) -> Diagnostic;
    /// S-50: the value of `r` in input `.eh_frame` `s`; `None` without
    /// the `.eh_frame` plan.
    fn eh_value(&self, s: SectionId, r: i64, who: EhWho) -> Option<Result<EhValue, SynthError>>;
    /// The output `.eh_frame`'s section index and address.
    fn eh_frame(&self) -> Option<(u32, u64)>;
    // ---- symbols ----
    /// A global's resolved definition (`None`: no row).
    fn def(&self, id: SymbolId) -> Option<Def>;
    /// An entry's definition in its input symbol table.
    fn entry(&self, e: SymEntry) -> Option<EntryDef>;
    /// [`def_value`] of a global (a full link computes it once per
    /// symbol, in parallel).
    fn global(&self, id: SymbolId) -> Valued;
    /// A common's or a copy's allocation: `(chunk, offset)`.
    fn alloc_place(&self, key: AllocKey) -> Option<(ChunkId, u64)>;
    /// A shared definition's copy allocation, by position.
    fn copy_of(&self, id: SymbolId) -> Option<u32>;
    /// A linker-defined symbol's value.
    fn defined(&self, id: SymbolId) -> Option<u64>;
    /// What the scan knows of a key (`spec/arch.md` §3.4).
    fn facts(&self, key: SymKey) -> SymFacts;
    /// [`Rows::facts`] at a record whose symbol is `mention` (I-14).
    fn facts_at(&self, key: SymKey, mention: Option<SymEntry>) -> SymFacts;
    /// The key's name.
    fn name(&self, key: SymKey) -> &[u8];
    // ---- slots ----
    /// The demands on a key.
    fn demand(&self, key: SymKey) -> DemandFlags;
    /// The GOT/PLT plan (the slots' offsets).
    fn got(&self) -> &GotPlan;
    /// The addresses of `.got`, `.got.plt`, `.plt`.
    fn got_addrs(&self) -> GotAddrs;
    /// The key's index in the slot table of `kind` (`TlsLd`: 0 when the
    /// module slot exists).
    fn slot_index(&self, key: SymKey, kind: SlotKind) -> Option<u32>;
    /// The patch a branch at `off` of `s` is redirected to: its
    /// `(chunk, offset)`.
    fn redirect(&self, s: SectionId, off: u64) -> Option<(ChunkId, u64)>;
    /// The TLS block.
    fn tls(&self) -> Option<TlsBlock>;
}

// ---- the formulas ----------------------------------------------------------

/// The section index and the address base of chunk `c` plus `offset`:
/// its address, or for a non-alloc chunk its offset from its section's
/// start.
pub fn chunk_at<R: Rows>(r: &R, c: ChunkId, offset: u64) -> Option<(u32, u64)> {
    let chunk = r.chunks().get(c)?;
    let shndx = match chunk.kind {
        ChunkKind::FileHeader | ChunkKind::ProgramHeaders => 1,
        _ => chunk.shndx?,
    };
    let base = if let Some(a) = chunk.addr {
        a
    } else {
        let sec = r.out_section(shndx)?;
        chunk.offset.checked_sub(sec.offset)?
    };
    Some((shndx, base.checked_add(offset)?))
}

/// A live fragment's value (model §4.5): its address, or in a
/// non-allocated merged chunk its offset from its section's start
/// (`.debug_str`, the gABI's `sh_addr` 0).
pub fn frag_value<R: Rows>(r: &R, frag: FragId) -> Option<u64> {
    let (chunk, offset) = r.frag_place(frag)?;
    Some(chunk_at(r, chunk, offset)?.1)
}

/// The value of a named symbol at `v` in section `s` (I-124): no value
/// at all when the section has no place (P-1).
pub fn value_in<R: Rows>(r: &R, s: SectionId, v: u64) -> Valued {
    if let Err(why) = r.section_place(s) {
        return Valued::NoPlace(why);
    }
    addr_in_placed(r, s, v).map_or(Valued::Missing, Valued::At)
}

/// The output address of a named symbol at `v` in section `s`, which has
/// a place (model §4.5; S-19/S-22 for a split `Merge` section; S-50 for
/// an `.eh_frame`).
pub fn addr_in_placed<R: Rows>(r: &R, s: SectionId, v: u64) -> Option<u64> {
    match r.section_kind(s)? {
        SectionKind::Merge => {
            let p = r.named_piece(s, v)?;
            frag_value(r, p.frag)?.checked_add(p.within)
        }
        SectionKind::EhFrame => {
            let at = i64::try_from(v).ok()?;
            match r.eh_value(s, at, EhWho::Symbol { droppable: false })?.ok()? {
                EhValue::Offset(o) => r.eh_frame()?.1.checked_add(o),
                EhValue::Dropped => None,
            }
        }
        _ => {
            let (c, at) = r.place(s)?;
            Some(chunk_at(r, c, at.checked_add(v)?)?.1)
        }
    }
}

/// Whether a named symbol at `v` in `Merge` section `s` names a collected
/// piece (I-7): its references are then to a dead target.
pub fn named_piece_dead<R: Rows>(r: &R, s: SectionId, v: u64) -> bool {
    r.named_piece(s, v).is_some_and(|p| !r.frag_live(p.frag))
}

/// The value of entry `e`'s definition (I-124).
pub fn entry_value<R: Rows>(r: &R, e: SymEntry) -> Valued {
    match r.entry(e) {
        Some(EntryDef { place: Some(Err(abs)), .. }) => Valued::At(abs),
        Some(EntryDef { place: Some(Ok(s)), value, .. }) => value_in(r, s, value),
        _ => Valued::Missing,
    }
}

/// A common's or a copy's address.
pub fn alloc_addr<R: Rows>(r: &R, key: AllocKey) -> Option<u64> {
    let (c, offset) = r.alloc_place(key)?;
    r.chunks().get(c)?.addr?.checked_add(offset)
}

/// The value of global `id`'s definition (model §4.5 without `redirect`;
/// I-124): weak undefined is `At(0)` (resolve §5.4), strong undefined
/// `Import`.
pub fn def_value<R: Rows>(r: &R, id: SymbolId) -> Valued {
    let found = |v: Option<u64>| v.map_or(Valued::Missing, Valued::At);
    let Some(def) = r.def(id) else { return Valued::Import };
    match def {
        Def::Undefined { weak: true } => Valued::At(0),
        Def::Undefined { weak: false } => Valued::Import,
        Def::Defined(e) => entry_value(r, e),
        Def::Common(_) => found(alloc_addr(r, AllocKey::Common(id))),
        Def::Shared(_) => match r.copy_of(id) {
            Some(at) => found(alloc_addr(r, AllocKey::Copy(at))),
            None => Valued::Import,
        },
        Def::Internal(_) => found(r.defined(id)),
    }
}

/// The entry defining `key` in a section (a local's own, a global's
/// regular definition).
pub fn def_entry<R: Rows>(r: &R, key: SymKey) -> Option<SymEntry> {
    match key.kind() {
        SymKind::Local(e) => Some(e),
        SymKind::Global(id) => match r.def(id)? {
            Def::Defined(e) => Some(e),
            _ => None,
        },
    }
}

/// The address a slot holds for `key` (S-66): its definition's, the
/// resolver's for an IFUNC; `None` for an undefined key, and for one
/// without a place (P-1: the scan refused every reference to it, so no
/// slot exists; a slot that did would be the caller's internal error,
/// never 0).
pub fn address<R: Rows>(r: &R, key: SymKey) -> Option<u64> {
    let v = match key.kind() {
        SymKind::Local(e) => entry_value(r, e),
        SymKind::Global(id) => {
            if matches!(r.def(id)?, Def::Undefined { .. }) {
                return None;
            }
            r.global(id)
        }
    };
    match v {
        Valued::At(a) => Some(a),
        Valued::Import | Valued::NoPlace(_) | Valued::Missing => None,
    }
}

/// I-47a: the value of a local symbol at `v` in section `d`, read in the
/// keeper's copy [`Rows::kept_copy`]; `None` otherwise (I-47's tombstone).
pub fn kept_copy_value<R: Rows>(r: &R, d: SectionId, v: u64) -> Option<Valued> {
    let k = r.kept_copy(d)?;
    match value_in(r, k, v) {
        Valued::At(a) => Some(Valued::At(a)),
        Valued::Import | Valued::NoPlace(_) | Valued::Missing => None,
    }
}

/// `spec/dyn.md` D-44, D-49: a key's PLT entry (dynamic) or IPLT one.
pub fn plt<R: Rows>(r: &R, key: SymKey) -> Option<u64> {
    r.got().plt_address(&r.got_addrs(), key)
}

/// The address of `key`'s slot of `kind`.
pub fn slot<R: Rows>(r: &R, key: SymKey, kind: SlotKind) -> Option<u64> {
    let i = r.slot_index(key, kind)?;
    let a = r.got_addrs();
    let got = r.got();
    Some(match kind {
        SlotKind::Addr => a.got + got.got_offset(Region::Addr, i),
        SlotKind::TpOff { .. } => a.got + got.got_offset(Region::TpOff, i),
        SlotKind::TlsGd => a.got + got.got_offset(Region::TlsGd, i),
        SlotKind::TlsDesc => a.got + got.got_offset(Region::TlsDesc, i),
        SlotKind::TlsLd => a.got + got.got_offset(Region::TlsLd, 0),
        SlotKind::PltGot => a.got_plt + got.iplt_word(i),
    })
}

/// THE value of a key (`spec/arch.md` §3.6 `FillTables::sym`, I-124),
/// with the key's facts (a definition's) given; with `split` (the caller
/// asks `split_target`), `s` is left 0 when the target is in a split
/// section, and the flag says so (`Tables::record_sym`).
pub fn value<R: Rows>(r: &R, key: SymKey, split: bool, facts: SymFacts) -> (SymValue, bool) {
    let read = def_entry(r, key).and_then(|e| r.entry(e));
    let size = read.map_or(0, |d| d.size);
    let place: Option<Result<(SectionId, EntryDef), u64>> =
        read.and_then(|d| d.place.map(|p| p.map(|s| (s, d))));
    // `split_target`'s cases: a section symbol of a `Merge` section, any
    // key defined in an `.eh_frame`.
    let in_split = |s: SectionId, d: &EntryDef| match r.section_kind(s) {
        Some(SectionKind::Merge) => d.sym_type == STT_SECTION,
        Some(SectionKind::EhFrame) => true,
        _ => false,
    };
    let is_split = split && matches!(&place, Some(Ok((s, d))) if in_split(*s, d));
    // A named symbol in a collected merge piece (I-7) is dead, as one in
    // a collected section is.
    let piece_dead = |place: &Option<Result<(SectionId, EntryDef), u64>>| {
        matches!(place, Some(Ok((s, d)))
            if d.sym_type != STT_SECTION
                && r.section_kind(*s) == Some(SectionKind::Merge)
                && named_piece_dead(r, *s, d.value))
    };
    // I-124: the value is typed; a key without a place (P-1), or whose
    // place the layout lacks, is not live, whatever `facts` says, so fill
    // writes the referring section's P-1 value (the tombstone, 0 in
    // `.gcc_except_table`) or stops: never `S = 0`.
    let value = match key.kind() {
        SymKind::Local(_) => {
            if facts.dead {
                // I-47a: a non-alloc member of a dropped COMDAT copy is
                // read in the keeper's copy; otherwise I-47.
                match &place {
                    Some(Ok((d, e))) => kept_copy_value(r, *d, e.value).unwrap_or(Valued::Missing),
                    _ => Valued::Missing,
                }
            } else if piece_dead(&place) {
                Valued::Missing
            } else if is_split {
                Valued::Import
            } else {
                match place {
                    Some(Err(abs)) => Valued::At(abs),
                    Some(Ok((s, d))) => value_in(r, s, d.value),
                    // `r_sym` 0, or a symbol of no section: S is 0 (the
                    // gABI's undefined symbol 0).
                    None => Valued::Import,
                }
            }
        }
        SymKind::Global(id) => {
            if piece_dead(&place) {
                Valued::Missing
            } else {
                r.global(id)
            }
        }
    };
    let (s, live) = fill_parts(value);
    // A PLT entry is `S` only where it is the symbol's address for the
    // whole program (`spec/arch.md` §3.6, WP-113): the canonical entry of
    // a `CANON_PLT` key (`spec/dyn.md` D-44), and, S-71, the IPLT entry of
    // an IFUNC defined in this output. An ordinary PLT entry (a
    // `JUMP_SLOT`'s, a library IFUNC's) is a stand-in for calls only:
    // such a key's `S` is its copy, or 0 (arch.md §4.3).
    let entry = if r.demand(key).contains(DemandFlags::CANON_PLT) {
        plt(r, key)
    } else if matches!(facts.kind, TargetKind::Shlib { .. } | TargetKind::WeakDyn) {
        None
    } else {
        r.got().iplt_address(&r.got_addrs(), key)
    };
    // `S` of a live key without an address of its own is 0 only for
    // `Import` (arch.md §4.3) and a split target (its caller's); a key
    // that is not live carries no `S` fill reads.
    let s = match (entry, s) {
        (Some(e), _) if live => e,
        (_, Some(v)) => v,
        _ => 0,
    };
    (SymValue { s, kind: facts.kind, size, live, tls: facts.tls }, is_split)
}

/// [`value`] of `key` with its own facts: `FillTables::sym`.
pub fn sym<R: Rows>(r: &R, key: SymKey) -> SymValue {
    value(r, key, false, r.facts(key)).0
}

// ---- the fill tables over rows ----------------------------------------------

/// The fill tables (`FillTables`, `fill::Tables`) over a row source: the
/// full link's are `RowTables<FrontRows>` ([`crate::link::LinkTables`]).
pub struct RowTables<R: Rows> {
    pub(crate) rows: R,
}

impl<R: Rows> RowTables<R> {
    /// The tables over `rows`.
    pub const fn from_rows(rows: R) -> Self {
        Self { rows }
    }

    /// The rows.
    pub const fn rows(&self) -> &R {
        &self.rows
    }
}

impl<R: Rows> Deref for RowTables<R> {
    type Target = R;
    fn deref(&self) -> &R {
        &self.rows
    }
}

impl<R: Rows> std::fmt::Debug for RowTables<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RowTables").field("got", &self.rows.got_addrs()).finish_non_exhaustive()
    }
}

impl<R: Rows> SymNames for RowTables<R> {
    fn name(&self, key: SymKey) -> &[u8] {
        self.rows.name(key)
    }
}

impl<R: Rows> FillTables for RowTables<R> {
    fn sym(&self, key: SymKey) -> SymValue {
        sym(&self.rows, key)
    }

    fn slot(&self, key: SymKey, kind: SlotKind) -> Option<u64> {
        slot(&self.rows, key, kind)
    }

    fn plt(&self, key: SymKey) -> Option<u64> {
        plt(&self.rows, key)
    }

    fn flags(&self, key: SymKey) -> DemandFlags {
        self.rows.demand(key)
    }

    fn redirect(&self, section: SectionId, offset: u64) -> Option<u64> {
        let (c, off) = self.rows.redirect(section, offset)?;
        self.rows.chunks().get(c)?.addr?.checked_add(off)
    }
}

impl<R: Rows> Tables for RowTables<R> {
    fn record_sym(&self, key: SymKey) -> (SymValue, bool) {
        value(&self.rows, key, true, self.rows.facts(key))
    }

    fn scan_facts(&self, key: SymKey, mention: Option<SymEntry>) -> Option<SymFacts> {
        Some(self.rows.facts_at(key, mention))
    }

    /// One read of the scan's facts where the mention cannot change them:
    /// a mention gives only an undefined key its type, and an undefined
    /// key's kind is `Weak0`, `WeakDyn` or `Undefined` (I-19); otherwise
    /// the definition's facts are those at the mention.
    fn record_sym_at(
        &self,
        key: SymKey,
        mention: Option<SymEntry>,
    ) -> (SymValue, bool, Option<SymFacts>) {
        let r = &self.rows;
        let at = r.facts_at(key, mention);
        let undefined =
            matches!(at.kind, TargetKind::Weak0 | TargetKind::WeakDyn | TargetKind::Undefined);
        let def = if mention.is_some() && undefined && matches!(key.kind(), SymKind::Global(_)) {
            r.facts(key)
        } else {
            debug_assert_eq!(at, r.facts(key), "facts at a mention");
            at
        };
        let (value, split) = value(r, key, true, def);
        (value, split, Some(at))
    }

    /// D-25, D-46: the OUTPUT section's `SHF_WRITE` (the loader maps the
    /// output section; RELRO sections have it), as the scan's place.
    fn section_writable(&self, section: SectionId) -> Option<bool> {
        let flags = match self.rows.out_flags(section) {
            Some(f) => f,
            None => self.rows.section_flags(section),
        };
        Some(flags & mortar_elf::consts::SHF_WRITE != 0)
    }

    fn target_alloc(&self, key: SymKey) -> bool {
        let r = &self.rows;
        match def_entry(r, key).and_then(|e| r.entry(e)).and_then(|d| d.place) {
            Some(Ok(s)) => r.section_flags(s) & SHF_ALLOC != 0,
            Some(Err(_)) => false,
            // Commons, linker-defined symbols: in alloc chunks.
            None => true,
        }
    }

    fn split_target(&self, key: SymKey, addend: i64) -> Option<Result<Option<u64>, Diagnostic>> {
        let place = self.split_place(key)?;
        self.split_at(&place, key, addend)
    }

    fn split_place(&self, key: SymKey) -> Option<SplitPlace> {
        let r = &self.rows;
        let d = r.entry(def_entry(r, key)?)?;
        let Ok(s) = d.place? else { return None };
        let section_sym = d.sym_type == STT_SECTION;
        let merge = match r.section_kind(s)? {
            SectionKind::Merge if section_sym => true,
            SectionKind::EhFrame => false,
            _ => return None,
        };
        let row = if merge { r.split_row(s).unwrap_or(u32::MAX) } else { u32::MAX };
        let chunk_base = merge.then(|| Some(chunk_at(r, r.merged_chunk(s)?, 0)?.1)).flatten();
        Some(SplitPlace { section: s, merge, value: d.value, section_sym, row, chunk_base })
    }

    fn split_at(
        &self,
        place: &SplitPlace,
        _key: SymKey,
        addend: i64,
    ) -> Option<Result<Option<u64>, Diagnostic>> {
        let r = &self.rows;
        let &SplitPlace { section: s, merge, value, section_sym, row, chunk_base } = place;
        if merge {
            let at = (value as i64).wrapping_add(addend);
            let found = r.section_piece(s, row, at)?;
            // S-13: the piece's fragment is in the section's output
            // section's merged chunk.
            let frag_value = |frag| match chunk_base {
                Some(base) => {
                    let v = r.frag_offset(frag).and_then(|o| base.checked_add(o));
                    debug_assert_eq!(v, frag_value(r, frag), "S-13");
                    v
                }
                None => frag_value(r, frag),
            };
            return Some(match found {
                Ok(p) if !r.frag_live(p.frag) => Ok(None),
                Ok(p) => frag_value(p.frag)
                    .and_then(|a| a.checked_add(p.within))
                    .map(Some)
                    .ok_or_else(|| internal("fill: a live merge piece has no address")),
                Err(w) => Err(r.outside_merge(s, at, &w)),
            });
        }
        let at = if section_sym { (value as i64).wrapping_add(addend) } else { value as i64 };
        let Some(v) = r.eh_value(s, at, EhWho::Reloc) else {
            return Some(Err(internal("fill: a reference into .eh_frame without its plan")));
        };
        let Some((_, eh)) = r.eh_frame() else {
            return Some(Err(internal("fill: a reference into .eh_frame without the chunk")));
        };
        let tail = if section_sym { 0 } else { addend };
        Some(match v {
            Ok(EhValue::Offset(o)) => Ok(Some(eh.wrapping_add(o).wrapping_add(tail as u64))),
            Ok(EhValue::Dropped) => Err(internal("fill: a relocation to a dropped symbol")),
            Err(e) => Err(e.into_diagnostic()),
        })
    }
}

// ---- the full link's rows ---------------------------------------------------

/// [`Rows`] over the full link's tables (its `Link`), with what the image
/// derives from them once per link: the input symbol tables, the slot
/// indexes, the chunks' addresses, the output sections, the merge piece
/// index and every global's value.
pub struct FrontRows<'a, 's> {
    pub(crate) link: Link<'a, 's>,
    pub(crate) addrs: Addresses<'a, 's>,
    /// Per file: its symbol table, when it is an object with one.
    pub(crate) tables: Box<[Option<SymbolTable<'s>>]>,
    /// Per global symbol: its definition's value (model §4.5, without
    /// `redirect`: the record keys are already redirected, as the scan
    /// decoded them; I-124).
    pub(crate) global: Box<[Valued]>,
    pub(crate) got: SlotIndex,
    pub(crate) tls_ie: SlotIndex,
    pub(crate) tls_gd: SlotIndex,
    pub(crate) tls_desc: SlotIndex,
    pub(crate) iplt: SlotIndex,
    pub(crate) got_addrs: GotAddrs,
    /// `.eh_frame`'s section index and address.
    pub(crate) eh_frame: Option<(u32, u64)>,
    /// `_DYNAMIC`, `.dynamic`'s address (`spec/dyn.md` D-32); `None` in a
    /// static link.
    pub(crate) dynamic: Option<u64>,
    /// Per section index (0 is the null section).
    pub(crate) sections: Box<[OutSection]>,
    /// References into `Merge` sections by offset (I-119).
    pub(crate) pieces: refs::PieceIndex,
}

impl std::fmt::Debug for FrontRows<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrontRows").field("got", &self.got_addrs).finish_non_exhaustive()
    }
}

impl<'a, 's> FrontRows<'a, 's> {
    /// The rows of `link`.
    #[must_use]
    pub fn new(link: Link<'a, 's>) -> Self {
        let addrs = Addresses::new(link.resolution, link.assignment, link.split, link.layout);
        let tables: Box<[Option<SymbolTable<'s>>]> = link
            .inputs
            .files
            .iter()
            .map(|(_, f)| match &f.reader {
                Reader::Object(o) => o.symbols().ok().flatten(),
                _ => None,
            })
            .collect();
        let slots = &link.layout.slots;
        let pl = &link.layout.placement;
        let chunk_addr = |kind: SynthKind| {
            link.synth
                .chunks
                .iter()
                .find(|c| c.0 == kind)
                .and_then(|c| pl.chunks.get(c.1))
                .and_then(|c| c.addr)
        };
        let got_addrs = GotAddrs {
            got: chunk_addr(SynthKind::Got).unwrap_or(0),
            got_plt: chunk_addr(SynthKind::GotPlt).unwrap_or(0),
            plt: chunk_addr(SynthKind::Plt).unwrap_or(0),
        };
        let eh_frame = link
            .synth
            .chunks
            .iter()
            .find(|c| c.0 == SynthKind::EhFrame)
            .and_then(|c| pl.chunks.get(c.1))
            .and_then(|c| Some((c.shndx?, c.addr?)));
        let mut t = Self {
            link,
            addrs,
            tables,
            global: Box::new([]),
            got: SlotIndex::new(&slots.got),
            tls_ie: SlotIndex::new(&slots.tls_ie),
            tls_gd: SlotIndex::new(&slots.tls_gd),
            tls_desc: SlotIndex::new(&slots.tls_desc),
            iplt: SlotIndex::new(&slots.iplt),
            got_addrs,
            eh_frame,
            dynamic: link.synth.dynamic.and_then(|_| chunk_addr(SynthKind::Dynamic)),
            sections: out_sections(&pl.chunks).into(),
            pieces: refs::PieceIndex::default(),
        };
        t.pieces = refs::PieceIndex::new(link.split, |s| t.section_size(s));
        let n = link.inputs.symbols.len() as usize;
        let global = link.pool.unit_cost(16).map(n, |i| def_value(&t, SymbolId(i as u32)));
        t.global = global.into();
        t
    }

    pub(crate) fn sym_of(&self, e: SymEntry) -> Option<(SymbolTable<'s>, mortar_elf::raw::Sym)> {
        let t = (*self.tables.get(e.file.0 as usize)?)?;
        let s = t.get(e.index).ok()?;
        Some((t, s))
    }

    /// The input section entry `e` is defined in, with its value; `Err`
    /// with the absolute value for `SHN_ABS`.
    pub(crate) fn entry_place(
        &self,
        e: SymEntry,
    ) -> Option<Result<(SectionId, mortar_elf::raw::Sym), u64>> {
        let (t, s) = self.sym_of(e)?;
        match t.section_of(e.index, &s).ok()? {
            SymbolSection::Absolute => Some(Err(s.st_value)),
            SymbolSection::Section(index) => {
                Some(Ok((self.link.resolution.section_id(e.file, index)?, s)))
            }
            _ => None,
        }
    }
}

impl Rows for FrontRows<'_, '_> {
    fn chunks(&self) -> &Column<ChunkId, Chunk> {
        &self.link.layout.placement.chunks
    }

    fn out_section(&self, shndx: u32) -> Option<OutSection> {
        self.sections.get(shndx as usize).copied()
    }

    fn place(&self, s: SectionId) -> Option<(ChunkId, u64)> {
        self.link.layout.placement.place.get(s)?.get()
    }

    fn section_size(&self, s: SectionId) -> Option<u64> {
        let (file, index) = self.link.resolution.sh_index(s)?;
        let Reader::Object(o) = &self.link.inputs.files.get(file)?.reader else { return None };
        Some(o.section(index).ok()?.size())
    }

    fn section_flags(&self, s: SectionId) -> u64 {
        let Some((file, index)) = self.link.resolution.sh_index(s) else { return 0 };
        let Some(Reader::Object(o)) = self.link.inputs.files.get(file).map(|f| &f.reader) else {
            return 0;
        };
        o.section(index).map_or(0, |s| s.flags())
    }

    fn section_kind(&self, s: SectionId) -> Option<SectionKind> {
        self.link.resolution.sections.get(s).map(|r| r.kind)
    }

    fn section_place(&self, s: SectionId) -> Result<(), NoPlace> {
        let l = &self.link;
        mortar_core::place::section_place(l.resolution, Some(l.assignment), l.liveness, s)
    }

    fn out_flags(&self, s: SectionId) -> Option<u64> {
        let a = self.link.assignment;
        a.out(s).and_then(|o| a.outsecs.get(o)).map(|out| out.flags)
    }

    /// I-47a: `d` has no `SHF_ALLOC`, is not live only because its COMDAT
    /// group copy is dropped (`spec/resolve.md` §6.2), and the group's
    /// keeper has, in its copy, exactly one section `k` of `d`'s name, `k`
    /// has no `SHF_ALLOC`, `d`'s `sh_size`, and is live. Section names
    /// and sizes only: no DWARF is parsed, no bytes are compared. Only a
    /// dead key's path asks, and an alloc `d` (a dropped copy's code)
    /// costs one flag test. `d` and `k` are of a kind placed as a whole (a
    /// `Merge` or `.eh_frame` member is not redirected: its value is not
    /// `k`'s address plus an offset). The referrer's half of the condition
    /// (live, non-alloc, debug-named) is fill's (`reloc.rs`, `own_value`).
    fn kept_copy(&self, d: SectionId) -> Option<SectionId> {
        use mortar_core::resolution::{DropReason, SectionState};
        let r = self.link.resolution;
        let row = r.sections.get(d)?;
        if row.state != SectionState::Dropped(DropReason::Comdat) || !row.kind.is_placed() {
            return None;
        }
        let shdr = |s: SectionId| {
            let (file, index) = r.sh_index(s)?;
            let Reader::Object(o) = &self.link.inputs.files.get(file)?.reader else { return None };
            let sec = o.section(index).ok()?;
            Some((sec.name().ok()?, sec.flags(), sec.size()))
        };
        let (name, flags, size) = shdr(d)?;
        if flags & SHF_ALLOC != 0 {
            return None;
        }
        let group = row.comdat.get()?;
        let keeper = r.comdat.get(group)?.get()?;
        let mut found = None;
        for k in r.section_base.get(keeper)?.iter() {
            if r.sections.get(k)?.comdat.get() != Some(group) {
                continue;
            }
            let Some(kh) = shdr(k) else { continue };
            if kh.0 == name {
                if found.is_some() {
                    return None;
                }
                found = Some((k, kh));
            }
        }
        let (k, (_, k_flags, k_size)) = found?;
        let k_row = r.sections.get(k)?;
        if k_flags & SHF_ALLOC != 0 || k_size != size || !k_row.is_kept() || !k_row.kind.is_placed()
        {
            return None;
        }
        Some(k)
    }

    fn named_piece(&self, s: SectionId, v: u64) -> Option<PieceRef> {
        if let Some(p) = self.pieces.named_symbol(self.link.split, s, v) {
            p.ok()
        } else {
            let size = self.section_size(s)?;
            refs::named_symbol(self.link.split, s, size, v).ok()
        }
    }

    fn split_row(&self, s: SectionId) -> Option<u32> {
        self.pieces.row_of(s)
    }

    fn section_piece(&self, s: SectionId, row: u32, r: i64) -> Option<Result<PieceRef, What>> {
        if let Some(found) = self.pieces.section_symbol_in(self.link.split, row, r) {
            Some(found)
        } else {
            let size = self.section_size(s)?;
            Some(refs::section_symbol(self.link.split, s, size, r))
        }
    }

    fn frag_live(&self, frag: FragId) -> bool {
        self.link.liveness.frag_live.get(frag).copied().unwrap_or(true)
    }

    fn frag_place(&self, frag: FragId) -> Option<(ChunkId, u64)> {
        let row = self.link.split.frags.get(frag)?;
        let chunk = self.addrs.merged_chunk(self.link.assignment.out(row.section)?)?;
        let offset = self.link.layout.merge.offset(frag)?;
        Some((chunk, offset))
    }

    fn frag_offset(&self, frag: FragId) -> Option<u64> {
        self.link.layout.merge.offset(frag)
    }

    fn merged_chunk(&self, s: SectionId) -> Option<ChunkId> {
        self.addrs.merged_chunk(self.link.assignment.out(s)?)
    }

    /// `spec/synth.md` S-18: the refusal of a section symbol plus addend
    /// `r` outside split `Merge` section `s` (`r < 0` or `r ≥ sh_size`;
    /// one past the last piece included, as lld: the references disagree
    /// on its address, WP-148), naming the target: the section and its
    /// file (`defined in`), `r` and the section's size. The caller adds
    /// the referring place (fill's record, `.rela.dyn`'s planner).
    fn outside_merge(&self, s: SectionId, r: i64, w: &What) -> Diagnostic {
        let d = Diagnostic::error(mortar_diag::Kind::Malformed { what: w.message() });
        let Some((file, index)) = self.link.resolution.sh_index(s) else { return d };
        let name = match self.link.inputs.files.get(file).map(|f| &f.reader) {
            Some(Reader::Object(o)) => o
                .section(index)
                .ok()
                .and_then(|h| h.name().ok())
                .map(|n| String::from_utf8_lossy(n).into_owned())
                .unwrap_or_default(),
            _ => String::new(),
        };
        let size = self.section_size(s).unwrap_or(0);
        let target = if r < 0 {
            format!("{name}-{:#x}", r.unsigned_abs())
        } else {
            format!("{name}+{r:#x}")
        };
        let path = layout_plan::file_ref(self.link.inputs, file);
        d.with(mortar_diag::Relation::DefinedIn, mortar_diag::Place::symbol(path, name)).note(
            format!(
                "the reference is to {target} (the section symbol plus addend); the mergeable \
                 section holds {size:#x} bytes, so no piece contains it (synth.md S-18)"
            ),
        )
    }

    fn eh_value(&self, s: SectionId, r: i64, who: EhWho) -> Option<Result<EhValue, SynthError>> {
        let (plan, src) = self.link.eh?;
        Some(plan.value(src, s, r, who))
    }

    fn eh_frame(&self) -> Option<(u32, u64)> {
        self.eh_frame
    }

    fn def(&self, id: SymbolId) -> Option<Def> {
        self.link.assignment.def(self.link.resolution, id)
    }

    fn entry(&self, e: SymEntry) -> Option<EntryDef> {
        let (t, sym) = self.sym_of(e)?;
        let place = match t.section_of(e.index, &sym).ok() {
            Some(SymbolSection::Absolute) => Some(Err(sym.st_value)),
            Some(SymbolSection::Section(index)) => {
                self.link.resolution.section_id(e.file, index).map(Ok)
            }
            _ => None,
        };
        Some(EntryDef { value: sym.st_value, size: sym.st_size, sym_type: sym.sym_type(), place })
    }

    fn global(&self, id: SymbolId) -> Valued {
        self.global.get(id.0 as usize).copied().unwrap_or(Valued::Missing)
    }

    fn alloc_place(&self, key: AllocKey) -> Option<(ChunkId, u64)> {
        let row = self.link.layout.placement.sym_place_of(key)?;
        Some((row.chunk, row.offset))
    }

    fn copy_of(&self, id: SymbolId) -> Option<u32> {
        let copies = &self.link.layout.slots.copy_of;
        copies.binary_search_by_key(&id, |row| row.0).ok().map(|at| copies[at].1)
    }

    fn defined(&self, id: SymbolId) -> Option<u64> {
        self.link.layout.placement.defined_of(id)
    }

    fn facts(&self, key: SymKey) -> SymFacts {
        self.link.facts.facts(key)
    }

    fn facts_at(&self, key: SymKey, mention: Option<SymEntry>) -> SymFacts {
        self.link.facts.facts_at(key, mention)
    }

    fn name(&self, key: SymKey) -> &[u8] {
        self.link.facts.name(key)
    }

    fn demand(&self, key: SymKey) -> DemandFlags {
        self.link.demands.of(key)
    }

    fn got(&self) -> &GotPlan {
        self.link.synth.got
    }

    fn got_addrs(&self) -> GotAddrs {
        self.got_addrs
    }

    fn slot_index(&self, key: SymKey, kind: SlotKind) -> Option<u32> {
        match kind {
            SlotKind::Addr => self.got.find(key),
            SlotKind::TpOff { .. } => self.tls_ie.find(key),
            SlotKind::TlsGd => self.tls_gd.find(key),
            SlotKind::TlsDesc => self.tls_desc.find(key),
            SlotKind::TlsLd => self.link.layout.slots.tls_ld.then_some(0),
            SlotKind::PltGot => self.iplt.find(key),
        }
    }

    fn redirect(&self, s: SectionId, off: u64) -> Option<(ChunkId, u64)> {
        let patch = self.link.layout.placement.redirect_of(s, off)?;
        Some((patch.chunk, patch.offset))
    }

    fn tls(&self) -> Option<TlsBlock> {
        self.link.layout.placement.tls.map(|t| TlsBlock {
            start: t.start,
            size: t.size,
            align: 1u64 << t.align,
        })
    }
}

// ---- the plan's rows ---------------------------------------------------------

/// What [`row_for`] reads of one input section from its file's front: its
/// header (`sh_size`, `sh_flags`, file offset, name) and its relocation
/// table, when it has one.
#[derive(Debug, Clone, Copy)]
pub struct SectionFront<'f> {
    pub file: FileId,
    pub index: SectionIndex,
    pub size: u64,
    pub flags: u64,
    /// The section's offset in its file.
    pub src: u64,
    pub name: &'f [u8],
    pub rela: Option<RelaTable>,
}

/// The plan row of placed input section `s` at `(chunk, offset)` (I-27;
/// WP-255 §9.1 `Input`): its unit, its relocation section with the
/// section's proof runs, its file range. The caller has checked that the
/// chunk is an in-file `Body` chunk and that the section has file bytes.
///
/// # Errors
///
/// `internal-error` for a chunk the rows do not hold.
pub fn row_for<R: Rows>(
    r: &R,
    s: SectionId,
    place: (ChunkId, u64),
    front: &SectionFront<'_>,
    proof: &dyn Fn(FileId, SectionIndex, u64) -> Vec<ProofRun>,
    b: &mut Builder,
) -> Result<(), Diagnostic> {
    let (c, off) = place;
    let chunk = r
        .chunks()
        .get(c)
        .ok_or_else(|| internal(format!("invalid fill plan: section {} in no chunk", s.0)))?;
    let size = front.size;
    let name = b.name(front.name);
    let alloc = front.flags & SHF_ALLOC != 0;
    let exec = front.flags & SHF_EXECINSTR != 0;
    let relocs = front.rela.map(|t| {
        let rows = RecRows::Rela(t);
        let proof = proof(front.file, front.index, size).into_boxed_slice();
        b.relocs(RelocSection { section: s, rows, proof })
    });
    let addr = if alloc { chunk.addr.map_or(0, |a| a + off) } else { 0 };
    let unit = InputUnit {
        file: front.file,
        index: front.index.0,
        section: s,
        src: front.src,
        start: 0,
        section_size: size,
        addr,
        alloc,
        exec,
        name,
        relocs,
    };
    b.input(chunk.offset + off, size, unit);
    Ok(())
}

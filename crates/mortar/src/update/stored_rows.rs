//! WP-255c: [`mortar_image::rows::Rows`] over the STORED state (format 5):
//! the image's formulas (`rows::value` first) fed by a `StateView` and a
//! `StoredLayout` instead of the full link's `Link`, so ONE implementation
//! of every kernel gives a key's value from stored rows (never a second
//! writer of the same bytes). The IDENTITY case: the stored placement as it
//! is (the layout delta is a later milestone).
//!
//! The source of every method is `spec/landed/incr/WP-255c.md`'s table.
//! The rule of every read: a row that fails to read, a row out of range, a
//! kind this file does not know, or a datum the state does not hold (the
//! file's own front, 255d; §9.3's merge globals) makes the method answer
//! `None` (or its trait's "absent") as `FrontRows` does, AND records the
//! first such reason; [`StoredRows::value_of`] returns that reason instead
//! of a value. Never a default value presented as a datum.
//!
//! It lives in the driver crate, which sees both `mortar_image` and
//! `mortar_incr` (ruling "After 255b" item 5): no new dependency edge.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use mortar_arch::{Arch, DemandFlags, PltVariant, SlotKind, SymFacts, SymValue, TlsBlock};
use mortar_core::demands::GlobalFlags;
use mortar_core::incr::{IncrReason, StoredLayout, Unimplemented};
use mortar_core::layout::{AllocKey, Chunk, ChunkKind, Placement, Slots};
use mortar_core::place::NoPlace;
use mortar_core::resolution::{Def, InternalDef, SectionKind};
use mortar_core::{
    ChunkId, Column, FileId, FragId, SectionId, SymEntry, SymKey, SymKind, SymbolId,
};
use mortar_diag::{Collector, Diagnostic, Policy};
use mortar_elf::SymbolIndex;
use mortar_elf::consts::{SHF_ALLOC, SHF_MERGE, SHT_NOBITS, SHT_NOTE, STB_WEAK};
use mortar_image::rows::{EntryDef, OutSection, Rows, Valued, out_sections};
use mortar_incr::format::{NONE, SectionRow, def_kind};
use mortar_incr::update::rows::MentionRow;
use mortar_incr::update::view::StateView;
use mortar_merge::What;
use mortar_merge::refs::PieceRef;
use mortar_synth::got::{GotAddrs, GotPlan};
use mortar_synth::{EhValue, EhWho, SynthError};

/// Keys per page of the name cache (two levels: a page is allocated when
/// one of its keys is first named, so the cache is bounded by the keys
/// read, plus one pointer per page).
const NAME_PAGE: usize = 4096;

/// The facts a read that failed answers (an undefined key's, packed 7),
/// always with the failure recorded: never read as a datum.
const UNKNOWN_FACTS: SymFacts = match SymFacts::unpack(7) {
    Some(f) => f,
    None => panic!("7 is a packed undefined key's facts"),
};

/// One page of the name cache: a key's `SYMBOLS.name` bytes, once read.
type NamePage = Box<[OnceLock<Box<[u8]>>]>;

/// One file's `MENTIONS` range and its `FILE_INDEX.first_global`.
type Mentions = Arc<(u32, Vec<MentionRow>)>;

/// [`Rows`] over a stored state, the identity placement.
pub struct StoredRows<'a, 's> {
    view: &'a StateView<'s>,
    /// The whole-class tables of the placement (`CHUNKS`, `SYM_PLACE`,
    /// `DEFINED`, `PATCHES`, `REDIRECTS`, `LAYOUT`'s TLS), in the
    /// `Placement` the full link's lookups read (`defined_of`,
    /// `sym_place_of`, `redirect_of`: one implementation). Its `place`
    /// column is empty: a section's place is its `SECTIONS` row.
    placement: Placement,
    sections: Box<[OutSection]>,
    got: GotPlan,
    got_addrs: GotAddrs,
    eh_frame: Option<(u32, u64)>,
    /// `SLOTS`: `(list, key)` to the index in its list.
    slots: HashMap<(mortar_core::layout::SlotKind, SymKey), u32>,
    tls_ld: bool,
    mentions: Mutex<HashMap<FileId, Mentions>>,
    names: Box<[OnceLock<NamePage>]>,
    /// The first reason a read could not answer (see the module doc).
    failed: Mutex<Option<IncrReason>>,
}

impl std::fmt::Debug for StoredRows<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredRows").field("got", &self.got_addrs).finish_non_exhaustive()
    }
}

fn corrupt(what: &str) -> IncrReason {
    IncrReason::StateCorrupt(format!("WP-255c stored rows: {what}"))
}

/// A datum the state does not hold: the file's own front (255d), or
/// §9.3's merge globals (class (d)).
fn front_only() -> IncrReason {
    IncrReason::UpdateNotImplemented(Some(Unimplemented::Image))
}

/// The synthetic chunk named `name`.
fn synth<'c>(chunks: &'c Column<ChunkId, Chunk>, name: &[u8]) -> Option<&'c Chunk> {
    chunks
        .iter()
        .map(|(_, c)| c)
        .find(|c| matches!(c.kind, ChunkKind::Synthetic(s) if s.name == name))
}

impl<'a, 's> StoredRows<'a, 's> {
    /// The rows of the state `view` with its layout `stored`, for a link
    /// of `arch` whose `.got` plan was made with `got_output` (the scan's
    /// output flags, `GOT_BASE` for `_GLOBAL_OFFSET_TABLE_` included) and
    /// PLT `variant` (the merged properties): neither is a stored row, so
    /// the caller gives them, and the plan they give is checked against
    /// the stored `.got`, `.got.plt` and `.plt` sizes.
    ///
    /// # Errors
    /// `state-corrupt` (a table that fails, a plan that disagrees with the
    /// stored chunks); `update-not-implemented (image)` for an output with
    /// `.dynamic` (its plan is `dynlink`'s, not built here).
    pub fn new(
        view: &'a StateView<'s>,
        stored: &StoredLayout<'_>,
        arch: &dyn Arch,
        got_output: GlobalFlags,
        variant: PltVariant,
    ) -> Result<Self, IncrReason> {
        let mut placement = Placement::default();
        let mut chunks = stored.chunks()?;
        chunks.sort_by_key(|c| c.0);
        for (at, (id, c)) in chunks.into_iter().enumerate() {
            if id.0 as usize != at {
                return Err(corrupt("CHUNKS: ids not dense"));
            }
            placement.chunks.push(c).map_err(|_| corrupt("CHUNKS: too many"))?;
        }
        let mut sym_place = stored.sym_place()?;
        sym_place.sort_by_key(|p| p.key);
        placement.sym_place = sym_place.into();
        let mut defined = stored.defined()?;
        defined.sort_by_key(|d| d.symbol);
        placement.defined = defined.into();
        placement.patches = stored.patches()?.into();
        let mut redirects = stored.redirects()?;
        redirects.sort_by_key(|r| (r.section, r.offset));
        placement.redirects = redirects.into();
        placement.tls = stored.tls_and_entry()?.0;
        let ch = &placement.chunks;
        if synth(ch, b".dynamic").is_some() {
            return Err(front_only());
        }
        let addr = |n: &[u8]| synth(ch, n).and_then(|c| c.addr).unwrap_or(0);
        let got_addrs =
            GotAddrs { got: addr(b".got"), got_plt: addr(b".got.plt"), plt: addr(b".plt") };
        let eh_frame = synth(ch, b".eh_frame").and_then(|c| Some((c.shndx?, c.addr?)));
        let tls_ld = view.layout()?.bits & mortar_incr::format::layout_bits::TLS_LD != 0;
        let mut lists = Slots { tls_ld, ..Slots::default() };
        let mut slots = HashMap::new();
        {
            use mortar_core::layout::SlotKind as L;
            let (mut got, mut plt, mut iplt, mut desc, mut gd, mut ie) =
                (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
            for s in stored.slots()? {
                let list = match s.kind {
                    L::Got => &mut got,
                    L::Plt => &mut plt,
                    L::Iplt => &mut iplt,
                    L::TlsDesc => &mut desc,
                    L::TlsGd => &mut gd,
                    L::TlsIe => &mut ie,
                };
                if s.index as usize != list.len() {
                    return Err(corrupt("SLOTS: index"));
                }
                list.push(s.key);
                if slots.insert((s.kind, s.key), s.index).is_some() {
                    return Err(corrupt("SLOTS: a key twice in a list"));
                }
            }
            lists.got = got.into();
            lists.plt = plt.into();
            lists.iplt = iplt.into();
            lists.tls_desc = desc.into();
            lists.tls_gd = gd.into();
            lists.tls_ie = ie.into();
        }
        let diag = Collector::new(Policy::default());
        let got = GotPlan::new(arch, &lists, got_output, variant, &diag)
            .map_err(|_| corrupt("SLOTS: no .got plan"))?;
        let size = |n: &[u8]| synth(ch, n).map_or(0, |c| c.size);
        if got.got_size() != size(b".got")
            || got.got_plt_size() != size(b".got.plt")
            || got.plt_size() != size(b".plt")
        {
            return Err(corrupt("SLOTS: the plan disagrees with the stored .got/.got.plt/.plt"));
        }
        let sections = out_sections(&placement.chunks).into();
        let keys = view.count(mortar_incr::format::kind::SYMBOLS) as usize;
        let names = (0..keys.div_ceil(NAME_PAGE)).map(|_| OnceLock::new()).collect();
        Ok(Self {
            view,
            placement,
            sections,
            got,
            got_addrs,
            eh_frame,
            slots,
            tls_ld,
            mentions: Mutex::new(HashMap::new()),
            names,
            failed: Mutex::new(None),
        })
    }

    /// The first reason a read could not answer, if any.
    #[must_use]
    pub fn failure(&self) -> Option<IncrReason> {
        self.failed.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// `rows::value` of `key` with its stored facts, or the reason a read
    /// of it could not answer (the reasons of earlier reads are cleared).
    ///
    /// # Errors
    /// The first reason recorded while valuing `key`.
    pub fn value_of(&self, key: SymKey) -> Result<SymValue, IncrReason> {
        *self.failed.lock().unwrap_or_else(PoisonError::into_inner) = None;
        let v = mortar_image::rows::sym(self, key);
        self.failure().map_or(Ok(v), Err)
    }

    fn fail(&self, why: IncrReason) {
        let mut f = self.failed.lock().unwrap_or_else(PoisonError::into_inner);
        if f.is_none() {
            *f = Some(why);
        }
    }

    /// `r`, or `None` with its error recorded.
    fn ok<T>(&self, r: Result<T, IncrReason>) -> Option<T> {
        r.map_err(|e| self.fail(e)).ok()
    }

    fn section_row(&self, s: SectionId) -> Option<SectionRow> {
        self.ok(self.view.section(s))
    }

    fn mentions(&self, file: FileId) -> Option<Mentions> {
        if let Some(m) = self.mentions.lock().unwrap_or_else(PoisonError::into_inner).get(&file) {
            return Some(m.clone());
        }
        let first = self.ok(self.view.file_index(file))?.first_global;
        let rows = self.ok(self.view.mentions_of(file))?;
        let m: Mentions = Arc::new((first, rows));
        self.mentions.lock().unwrap_or_else(PoisonError::into_inner).insert(file, m.clone());
        Some(m)
    }

    /// The entry of `file` that defines `id` with definition kind `kind`
    /// in `section` (`NONE` for none): the one `MENTIONS` row that says so.
    fn definer_entry(
        &self,
        file: FileId,
        id: SymbolId,
        kind: u8,
        section: u32,
    ) -> Option<SymEntry> {
        let m = self.mentions(file)?;
        let mut found =
            m.1.iter()
                .enumerate()
                .filter(|(_, r)| r.symbol == id.0 && r.kind == kind && r.section == section);
        let Some((k, _)) = found.next() else {
            self.fail(corrupt("MENTIONS: no definer row of a RESOLVED key"));
            return None;
        };
        if found.next().is_some() {
            // Two definitions of one key in one file: which one won is
            // not stored (the file's own front).
            self.fail(front_only());
            return None;
        }
        let index = m.0.checked_add(u32::try_from(k).ok()?)?;
        Some(SymEntry { file, index: SymbolIndex(index) })
    }

    /// The definer file of an absolute key: its one `KEY_REFS` row of
    /// definition kind `ABSOLUTE` and the resolved binding.
    fn absolute_file(&self, id: SymbolId, bind: u8) -> Option<FileId> {
        let refs = self.ok(self.view.key_refs(id))?;
        let mut found =
            refs.iter().filter(|r| !r.tomb && r.def == def_kind::ABSOLUTE && r.bind == bind);
        let first = found.next();
        if found.next().is_some() || first.is_none() {
            self.fail(front_only());
            return None;
        }
        Some(FileId(first?.file))
    }
}

impl Rows for StoredRows<'_, '_> {
    fn chunks(&self) -> &Column<ChunkId, Chunk> {
        &self.placement.chunks
    }

    fn out_section(&self, shndx: u32) -> Option<OutSection> {
        self.sections.get(shndx as usize).copied()
    }

    fn place(&self, s: SectionId) -> Option<(ChunkId, u64)> {
        let row = self.section_row(s)?;
        if row.place_chunk == NONE {
            return None;
        }
        if self.placement.chunks.get(ChunkId(row.place_chunk)).is_none() {
            self.fail(corrupt("SECTIONS: place_chunk out of range"));
            return None;
        }
        Some((ChunkId(row.place_chunk), row.place_offset))
    }

    fn section_size(&self, s: SectionId) -> Option<u64> {
        Some(self.section_row(s)?.size)
    }

    fn section_flags(&self, s: SectionId) -> u64 {
        self.section_row(s).map_or(0, |r| r.sh_flags)
    }

    fn section_kind(&self, s: SectionId) -> Option<SectionKind> {
        let row = self.section_row(s)?;
        let name = self.ok(self.view.string(row.name))?;
        Some(if name == b".eh_frame" {
            SectionKind::EhFrame
        } else if row.sh_flags & SHF_MERGE != 0 && row.place_chunk == NONE {
            // §9.3: a split section has no place (a dead one neither: the
            // row cannot tell them apart; both fall back in `named_piece`).
            SectionKind::Merge
        } else if row.sh_type == SHT_NOBITS {
            SectionKind::Nobits
        } else if row.sh_type == SHT_NOTE {
            SectionKind::Note
        } else if row.sh_flags & SHF_ALLOC == 0 && name.starts_with(b".debug") {
            SectionKind::Debug
        } else {
            SectionKind::Regular
        })
    }

    fn section_place(&self, s: SectionId) -> Result<(), NoPlace> {
        // The state stores a place for a kept, assigned, live section
        // only; which P-1 reason a placeless one has is not stored. `value`
        // reads only whether there is a place (`fill_parts`).
        match self.section_row(s) {
            Some(r) if r.place_chunk != NONE => Ok(()),
            _ => Err(NoPlace::Collected),
        }
    }

    fn out_flags(&self, s: SectionId) -> Option<u64> {
        let (c, _) = self.place(s)?;
        Some(self.placement.chunks.get(c)?.flags)
    }

    fn kept_copy(&self, _d: SectionId) -> Option<SectionId> {
        self.fail(front_only());
        None
    }

    fn named_piece(&self, _s: SectionId, _v: u64) -> Option<PieceRef> {
        // §9.3 class (d): the definer's section split by `mortar_merge`,
        // then `FRAGS`: needs the section's bytes (255d's front).
        self.fail(front_only());
        None
    }

    fn split_row(&self, _s: SectionId) -> Option<u32> {
        self.fail(front_only());
        None
    }

    fn section_piece(&self, _s: SectionId, _row: u32, _r: i64) -> Option<Result<PieceRef, What>> {
        self.fail(front_only());
        None
    }

    fn frag_live(&self, _f: FragId) -> bool {
        // Only reached through `named_piece`/`section_piece`, which fall
        // back first.
        self.fail(front_only());
        true
    }

    fn frag_place(&self, _f: FragId) -> Option<(ChunkId, u64)> {
        self.fail(front_only());
        None
    }

    fn frag_offset(&self, _f: FragId) -> Option<u64> {
        self.fail(front_only());
        None
    }

    fn merged_chunk(&self, _s: SectionId) -> Option<ChunkId> {
        self.fail(front_only());
        None
    }

    fn outside_merge(&self, s: SectionId, r: i64, w: &What) -> Diagnostic {
        let d = Diagnostic::error(mortar_diag::Kind::Malformed { what: w.message() });
        let Some(row) = self.section_row(s) else { return d };
        let name = self
            .ok(self.view.string(row.name))
            .map(|n| String::from_utf8_lossy(&n).into_owned())
            .unwrap_or_default();
        let target = if r < 0 {
            format!("{name}-{:#x}", r.unsigned_abs())
        } else {
            format!("{name}+{r:#x}")
        };
        d.note(format!(
            "the reference is to {target} (the section symbol plus addend); the mergeable \
             section holds {:#x} bytes, so no piece contains it (synth.md S-18)",
            row.size
        ))
    }

    fn eh_value(&self, _s: SectionId, _r: i64, _who: EhWho) -> Option<Result<EhValue, SynthError>> {
        self.fail(front_only());
        None
    }

    fn eh_frame(&self) -> Option<(u32, u64)> {
        self.eh_frame
    }

    fn def(&self, id: SymbolId) -> Option<Def> {
        let r = self.ok(self.view.resolved(id))?;
        Some(match r.kind {
            def_kind::UNDEFINED => Def::Undefined { weak: r.bind == STB_WEAK },
            def_kind::SECTION => {
                let file = FileId(self.section_row(SectionId(r.section))?.file);
                Def::Defined(self.definer_entry(file, id, r.kind, r.section)?)
            }
            def_kind::ABSOLUTE => {
                let file = self.absolute_file(id, r.bind)?;
                Def::Defined(self.definer_entry(file, id, r.kind, NONE)?)
            }
            def_kind::COMMON => {
                let Some(c) = self.ok(self.view.common(id))? else {
                    self.fail(corrupt("COMMONS: a common key without a common"));
                    return None;
                };
                Def::Common(self.definer_entry(FileId(c.file), id, r.kind, NONE)?)
            }
            // The payload (`Assign`, `Alias`, `Provide`) is not stored;
            // every formula of `rows.rs` reads an internal key's value
            // from `DEFINED` alone (`def_value`, `address`).
            def_kind::INTERNAL => Def::Internal(InternalDef::Provide { conditional: false }),
            def_kind::SHARED => {
                self.fail(front_only());
                return None;
            }
            _ => {
                self.fail(corrupt("RESOLVED: kind"));
                return None;
            }
        })
    }

    fn entry(&self, e: SymEntry) -> Option<EntryDef> {
        let m = self.mentions(e.file)?;
        let Some(k) = e.index.0.checked_sub(m.0) else {
            // A local: the file's own front.
            self.fail(front_only());
            return None;
        };
        let Some(row) = m.1.get(k as usize) else {
            self.fail(corrupt("MENTIONS: an entry out of range"));
            return None;
        };
        let r = self.ok(self.view.resolved(SymbolId(row.symbol)))?;
        let defines = row.kind == r.kind
            && row.section == r.section
            && matches!(r.kind, def_kind::SECTION | def_kind::ABSOLUTE | def_kind::COMMON);
        if !defines {
            // An entry that is not its key's resolved definition: the
            // stored row is the winner's, not this entry's.
            self.fail(front_only());
            return None;
        }
        let place = match r.kind {
            def_kind::SECTION => Some(Ok(SectionId(r.section))),
            def_kind::ABSOLUTE => Some(Err(r.value)),
            _ => None,
        };
        Some(EntryDef { value: r.value, size: r.size, sym_type: r.typ, place })
    }

    fn global(&self, id: SymbolId) -> Valued {
        mortar_image::rows::def_value(self, id)
    }

    fn alloc_place(&self, key: AllocKey) -> Option<(ChunkId, u64)> {
        let row = self.placement.sym_place_of(key)?;
        Some((row.chunk, row.offset))
    }

    fn copy_of(&self, _id: SymbolId) -> Option<u32> {
        // A static output has no copies (`new` refuses `.dynamic`).
        None
    }

    fn defined(&self, id: SymbolId) -> Option<u64> {
        let v = self.placement.defined_of(id);
        if v.is_none() {
            self.fail(corrupt("DEFINED: no row of an internal key"));
        }
        v
    }

    fn facts(&self, key: SymKey) -> SymFacts {
        let SymKind::Global(id) = key.kind() else {
            self.fail(front_only());
            return UNKNOWN_FACTS;
        };
        let Some(r) = self.ok(self.view.resolved(id)) else { return UNKNOWN_FACTS };
        SymFacts::unpack(r.facts).unwrap_or_else(|| {
            self.fail(corrupt("RESOLVED: facts"));
            UNKNOWN_FACTS
        })
    }

    fn facts_at(&self, key: SymKey, mention: Option<SymEntry>) -> SymFacts {
        if mention.is_some() {
            self.fail(front_only());
        }
        self.facts(key)
    }

    fn name(&self, key: SymKey) -> &[u8] {
        let SymKind::Global(id) = key.kind() else {
            self.fail(front_only());
            return b"";
        };
        let i = id.0 as usize;
        let Some(page) = self.names.get(i / NAME_PAGE) else {
            self.fail(corrupt("SYMBOLS: a key out of range"));
            return b"";
        };
        let page = page.get_or_init(|| (0..NAME_PAGE).map(|_| OnceLock::new()).collect());
        page[i % NAME_PAGE].get_or_init(|| {
            self.ok(self.view.symbol(id))
                .and_then(|s| self.ok(self.view.string(s.name)))
                .unwrap_or_default()
                .into()
        })
    }

    fn demand(&self, key: SymKey) -> DemandFlags {
        let SymKind::Global(id) = key.kind() else {
            self.fail(front_only());
            return DemandFlags::empty();
        };
        let Some(refs) = self.ok(self.view.key_refs(id)) else { return DemandFlags::empty() };
        let bits = refs.iter().filter(|r| !r.tomb).fold(0u16, |a, r| a | r.demand);
        DemandFlags::from_bits(bits).unwrap_or_else(|| {
            self.fail(corrupt("KEY_REFS: demand"));
            DemandFlags::empty()
        })
    }

    fn got(&self) -> &GotPlan {
        &self.got
    }

    fn got_addrs(&self) -> GotAddrs {
        self.got_addrs
    }

    fn slot_index(&self, key: SymKey, kind: SlotKind) -> Option<u32> {
        use mortar_core::layout::SlotKind as L;
        let list = match kind {
            SlotKind::Addr => L::Got,
            SlotKind::TpOff { .. } => L::TlsIe,
            SlotKind::TlsGd => L::TlsGd,
            SlotKind::TlsDesc => L::TlsDesc,
            SlotKind::TlsLd => return self.tls_ld.then_some(0),
            SlotKind::PltGot => L::Iplt,
        };
        self.slots.get(&(list, key)).copied()
    }

    fn redirect(&self, s: SectionId, off: u64) -> Option<(ChunkId, u64)> {
        let patch = self.placement.redirect_of(s, off)?;
        Some((patch.chunk, patch.offset))
    }

    fn tls(&self) -> Option<TlsBlock> {
        self.placement.tls.map(|t| TlsBlock {
            start: t.start,
            size: t.size,
            align: 1u64 << t.align,
        })
    }
}

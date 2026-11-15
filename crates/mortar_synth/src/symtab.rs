//! `.symtab`, `.strtab` and `.symtab_shndx` (`spec/synth.md` §7.1 to
//! §7.3).
//!
//! **Plan** ([`SymtabPlan::new`], phase 80; again after layout's fixed
//! point when patches exist, S-2): which symbols are written (S-75 to
//! S-78), in which order (S-74, S-75), the counts per file
//! (`Slots.symtab_locals`, S-88) and the sizes. No row per symbol is
//! stored (S-89): the plan keeps per-file counts and a checkpoint every
//! [`CHECKPOINT`] entries, and the write step recomputes the keep rule,
//! which is a pure function of the tables.
//!
//! **Write** (image): any range of each table ([`SymtabPlan::write_symtab`],
//! [`SymtabPlan::write_strtab`], [`SymtabPlan::write_shndx`]) in
//! O(range + [`CHECKPOINT`] + log entries), from the addresses a
//! [`SymAddrs`] oracle gives (S-81, S-82, S-122 to S-124).

use std::ops::Range;

use mortar_core::column::Column;
use mortar_core::layout::{Patch, PatchKind, ThunkTarget};
use mortar_core::par::Pool;
use mortar_core::plan::Discard;
use mortar_core::split::Split;
use mortar_core::{FileId, Liveness, Resolution, SectionId, Span, SymKey, SymbolId};
use mortar_diag::Collector;
use mortar_elf::Class;
use mortar_elf::consts::{
    SHN_ABS, SHN_LORESERVE, SHN_UNDEF, SHN_XINDEX, STB_LOCAL, STT_FILE, STT_FUNC, STT_NOTYPE,
    STT_OBJECT, STT_SECTION, STT_TLS, STV_HIDDEN, STV_INTERNAL,
};

use crate::eh::{EhPlan, EhSource, EhValue, Referrer};
use crate::error::{SynthError, Failed, fail, internal, key};
use crate::out::Window;

/// Entries between two checkpoints of a block (the `B` of the write cost).
pub const CHECKPOINT: u32 = 1024;

/// The fixed table of S-86 after the leading zero byte: `$x`, `$d`,
/// `__erratum_843419_veneer`.
const FIXED: &[u8] = b"\0$x\0$d\0__erratum_843419_veneer\0";
/// `.strtab`'s first region byte.
const REGION: u64 = FIXED.len() as u64;
/// An AArch64 Cortex-A53 843419 patch's name (S-80).
pub const ERRATUM_843419: &[u8] = b"__erratum_843419_veneer";
/// The bytes of an 843419 patch: two instructions (`spec/a64.md` §11).
pub const ERRATUM_843419_SIZE: u64 = 8;

fn fixed_name(name: &[u8]) -> Option<u32> {
    match name {
        b"" => Some(0),
        b"$x" => Some(1),
        b"$d" => Some(4),
        ERRATUM_843419 => Some(7),
        _ => None,
    }
}

/// The bytes a name takes in a region (S-86): none for the empty name and
/// the fixed table's names.
fn region_len(name: &[u8]) -> u64 {
    if fixed_name(name).is_some() { 0 } else { name.len() as u64 + 1 }
}

/// An AArch64 mapping symbol (AAELF64 "Mapping symbols"): `$x`, `$d`,
/// `$x.<any>`, `$d.<any>`.
#[must_use]
pub fn is_mapping(name: &[u8]) -> bool {
    matches!(name, [b'$', b'x' | b'd'] | [b'$', b'x' | b'd', b'.', ..])
}

/// What a section is for the keep rule of S-76.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecKind {
    /// Placed whole.
    Plain,
    /// Split into merge pieces (S-22).
    Merge,
    /// `.eh_frame` (S-50).
    EhFrame,
}

/// What the keep rule asks of sections.
pub trait Sections: Sync {
    /// Live after `--gc-sections` (model §3.6).
    fn live(&self, section: SectionId) -> bool;
    fn kind(&self, section: SectionId) -> SecKind;
    /// For a `Merge` section: whether the piece holding `offset` is in a
    /// live fragment (S-22).
    fn merge_live(&self, section: SectionId, offset: u64) -> bool;
}

/// [`Sections`] over the core tables.
#[derive(Debug, Clone, Copy)]
pub struct CoreSections<'a> {
    pub resolution: &'a Resolution,
    pub liveness: &'a Liveness,
    pub split: &'a Split,
}

impl Sections for CoreSections<'_> {
    fn live(&self, section: SectionId) -> bool {
        self.liveness.is_live(section)
    }

    fn kind(&self, section: SectionId) -> SecKind {
        use mortar_core::resolution::SectionKind as K;
        match self.resolution.sections.get(section).map(|row| row.kind) {
            Some(K::Merge) => SecKind::Merge,
            Some(K::EhFrame) => SecKind::EhFrame,
            _ => SecKind::Plain,
        }
    }

    fn merge_live(&self, section: SectionId, offset: u64) -> bool {
        let Ok(offset) = u32::try_from(offset) else { return false };
        self.split.merge_piece_at(section, offset).is_some_and(|at| {
            let frag = self.split.piece_frag[at.piece as usize];
            self.liveness.frag_live.get(frag).copied().unwrap_or(false)
        })
    }
}

/// Where a symbol is defined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymDef {
    /// `st_value` bytes into an input section (S-81).
    Section { section: SectionId, value: u64 },
    /// `SHN_ABS`, value unchanged.
    Absolute(u64),
    /// A common symbol: its `.bss` allocation (layout L-88).
    Common,
    /// Linker-defined (layout §13): layout's value.
    Internal,
    /// Undefined (weak; or shared in a dynamic output).
    Undefined,
}

/// A local entry of an input symbol table (`st_index ≥ 1`, before
/// `first_global`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalIn<'a> {
    pub name: &'a [u8],
    pub info: u8,
    pub other: u8,
    pub size: u64,
    pub def: SymDef,
    /// A live relocation refers to it through this symbol (S-50 rule 4:
    /// an unreferenced local inside a removed record is dropped).
    pub referenced: bool,
}

/// A global key's facts after resolution (resolve.md §3.7): the chosen
/// definition's type and size, its binding, the visibility merged over
/// every non-local entry (S-121).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlobalIn<'a> {
    pub name: &'a [u8],
    /// `STT_*` of the chosen definition.
    pub typ: u8,
    /// `STB_*` of the chosen definition (`STB_WEAK` when every
    /// definition is weak).
    pub binding: u8,
    /// The merged `st_other` (its low bits the most constraining
    /// visibility).
    pub other: u8,
    pub size: u64,
    pub def: SymDef,
    /// The defining file (`FileId::INTERNAL` for linker-defined keys).
    pub file: FileId,
    /// A live relocation refers to the key (an undefined key without one
    /// is absent, resolve.md §3.7).
    pub referenced: bool,
}

/// The symbols, as resolution leaves them.
pub trait SymSource: Sync {
    /// The number of files (`FileId`s are `0..files()`).
    fn files(&self) -> u32;
    /// The number of local entries of `file` after the null entry.
    fn locals(&self, file: FileId) -> u32;
    /// Local `i` of `file` (its `st_index` is `i + 1`).
    fn local(&self, file: FileId, i: u32) -> LocalIn<'_>;
    /// The number of global keys.
    fn globals(&self) -> u32;
    /// Key `id`, or `None` when it has no `.symtab` entry in any case (a
    /// withdrawn conditional linker symbol, a key no live file mentions).
    fn global(&self, id: SymbolId) -> Option<GlobalIn<'_>>;
    /// The name of a key, for thunk names (S-80).
    fn key_name(&self, key: SymKey) -> &[u8];
}

/// A linker-defined symbol's value (layout §13.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InternalValue {
    Absolute(u64),
    /// In the output section with this index.
    At { shndx: u32, addr: u64 },
}

/// The addresses the write step needs (the image phase's answers).
pub trait SymAddrs: Sync {
    /// The output section index and address of `value` bytes into input
    /// section `section` (model §4.5; a `Merge` section by S-19, a folded
    /// one at its target's place). Never asked for an `.eh_frame`.
    fn section(&self, section: SectionId, value: u64) -> Option<(u32, u64)>;
    /// The output `.eh_frame`'s section index and address.
    fn eh_frame(&self) -> Option<(u32, u64)>;
    /// A common symbol's allocation.
    fn common(&self, id: SymbolId) -> Option<(u32, u64)>;
    /// A linker-defined symbol's value; `None` only if layout failed to
    /// define it.
    fn internal(&self, id: SymbolId) -> Option<InternalValue>;
    /// The section index and address of patch `index` of
    /// `Placement.patches`.
    fn patch(&self, index: usize) -> Option<(u32, u64)>;
    /// `PT_TLS`'s `p_vaddr` (S-81).
    fn tls_start(&self) -> Option<u64>;
    /// The address and size of output section `shndx` (S-122's check).
    fn section_range(&self, shndx: u32) -> Option<(u64, u64)>;
}

/// The inputs of the keep rule, shared by plan and write.
#[derive(Clone, Copy)]
pub struct SymCtx<'a> {
    pub syms: &'a dyn SymSource,
    pub sections: &'a dyn Sections,
    /// The `.eh_frame` plan and its source, for symbols in `.eh_frame`.
    pub eh: Option<(&'a EhPlan, &'a dyn EhSource)>,
    /// `plan.shaping.discard`; `None` is `-X` (S-78).
    pub discard: Option<Discard>,
}

impl std::fmt::Debug for SymCtx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SymCtx").field("discard", &self.discard).finish_non_exhaustive()
    }
}

/// Patch symbols' facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatchFacts {
    /// `arch.thunk()`'s size.
    pub thunk_size: u64,
    /// AArch64: a `$x` per patch (S-80).
    pub mapping: bool,
}

/// One output symbol, before addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Where {
    Section { section: SectionId, value: u64 },
    /// `.eh_frame` at this output offset.
    Eh(u64),
    Absolute(u64),
    Common(SymbolId),
    Internal(SymbolId),
    Undefined,
    Patch(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Row<'a> {
    name: std::borrow::Cow<'a, [u8]>,
    info: u8,
    other: u8,
    size: u64,
    at: Where,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seg {
    Null,
    /// Rows `demoted[rows]` (the internal file's).
    Linker,
    Patches,
    Locals(FileId),
    Demoted(FileId),
    Globals,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SegRow {
    seg: Seg,
    entry: u32,
    len: u32,
    str_off: u64,
    /// For `Linker` and `Demoted`: the rows of `demoted`.
    rows: Range<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Check {
    entry: u32,
    seg: u32,
    /// Input local index, `SymbolId`, or row.
    cursor: u32,
    str_off: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PatchSym {
    Name(usize),
    Mapping(usize),
}

/// The plan of §7.1 to §7.3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymtabPlan {
    class: Class,
    facts: PatchFacts,
    segs: Box<[SegRow]>,
    checks: Box<[Check]>,
    /// Demoted globals by `(file, SymbolId)`; the internal file's first.
    demoted: Box<[(FileId, SymbolId)]>,
    /// Per demoted row: its first region byte.
    demoted_str: Box<[u64]>,
    patches: Box<[Patch]>,
    patch_syms: Box<[PatchSym]>,
    patch_str: Box<[u64]>,
    entries: u32,
    locals: u32,
    strtab: u64,
    symtab_locals: Column<FileId, Span>,
}

enum Class3 {
    Skip,
    Global,
    Demoted(FileId),
}

fn thunk_name(syms: &dyn SymSource, target: ThunkTarget) -> Vec<u8> {
    let mut v = b"__".to_vec();
    match target {
        ThunkTarget::Sym { key, addend } => {
            v.extend_from_slice(syms.key_name(key));
            if addend > 0 {
                v.extend_from_slice(format!("+{addend:#x}").as_bytes());
            } else if addend < 0 {
                v.extend_from_slice(format!("-{:#x}", addend.unsigned_abs()).as_bytes());
            }
        }
        ThunkTarget::Plt(key) => {
            v.extend_from_slice(syms.key_name(key));
            v.extend_from_slice(b"@plt");
        }
    }
    v.extend_from_slice(b"_veneer");
    v
}

impl<'a> SymCtx<'a> {
    /// S-50, S-22 and liveness for a definition in `section` at `value`:
    /// the place, or `None` when the symbol is not written.
    fn place(
        &self,
        section: SectionId,
        value: u64,
        who: Referrer,
    ) -> Result<Option<Where>, SynthError> {
        if !self.sections.live(section) {
            return Ok(None);
        }
        match self.sections.kind(section) {
            SecKind::Plain => Ok(Some(Where::Section { section, value })),
            SecKind::Merge => Ok(self
                .sections
                .merge_live(section, value)
                .then_some(Where::Section { section, value })),
            SecKind::EhFrame => {
                let Some((eh, src)) = self.eh else {
                    return Err(internal("a symbol in .eh_frame without an .eh_frame plan"));
                };
                let r = i64::try_from(value).unwrap_or(i64::MAX);
                Ok(match eh.value(src, section, r, who)? {
                    EhValue::Offset(o) => Some(Where::Eh(o)),
                    EhValue::Dropped => None,
                })
            }
        }
    }

    /// S-76, S-78: local `i` of `file` as written, or `None`.
    fn local(&self, file: FileId, i: u32) -> Result<Option<Row<'a>>, SynthError> {
        let syms: &'a dyn SymSource = self.syms;
        let l = syms.local(file, i);
        let discard = self.discard.unwrap_or(Discard::Locals);
        let typ = l.info & 0xf;
        if discard == Discard::All || typ == STT_SECTION {
            return Ok(None);
        }
        if discard == Discard::Locals && l.name.starts_with(b".L") && typ != STT_FILE {
            return Ok(None);
        }
        let at = match l.def {
            SymDef::Absolute(v) => Some(Where::Absolute(v)),
            SymDef::Section { section, value } => {
                let who = if is_mapping(l.name) {
                    Referrer::Mapping
                } else {
                    Referrer::Symbol { local_unreferenced: !l.referenced }
                };
                self.place(section, value, who)?
            }
            // A local is never common, linker-defined or undefined in a
            // valid object; such an entry names nothing.
            SymDef::Common | SymDef::Internal | SymDef::Undefined => None,
        };
        Ok(at.map(|at| Row { name: l.name.into(), info: l.info, other: l.other, size: l.size, at }))
    }

    fn classify(&self, id: SymbolId) -> Result<(Class3, Option<Row<'a>>), SynthError> {
        let syms: &'a dyn SymSource = self.syms;
        let Some(g) = syms.global(id) else { return Ok((Class3::Skip, None)) };
        let vis = g.other & 3;
        let at = match g.def {
            SymDef::Section { section, value } => {
                self.place(section, value, Referrer::Symbol { local_unreferenced: false })?
            }
            SymDef::Absolute(v) => Some(Where::Absolute(v)),
            SymDef::Common => Some(Where::Common(id)),
            SymDef::Internal => Some(Where::Internal(id)),
            SymDef::Undefined => g.referenced.then_some(Where::Undefined),
        };
        let Some(at) = at else { return Ok((Class3::Skip, None)) };
        // S-81: a common symbol is `STT_OBJECT` in `.bss`.
        let typ = if g.def == SymDef::Common { STT_OBJECT } else { g.typ };
        // S-77, S-121: hidden and internal definitions are locals.
        let local = (vis == STV_HIDDEN || vis == STV_INTERNAL) && g.def != SymDef::Undefined;
        let binding = if local { STB_LOCAL } else { g.binding };
        let row = Row { name: g.name.into(), info: binding << 4 | typ, other: g.other, size: g.size, at };
        Ok((if local { Class3::Demoted(g.file) } else { Class3::Global }, Some(row)))
    }
}

/// A walk over the entries from a checkpoint.
struct Walk<'p, 'c> {
    plan: &'p SymtabPlan,
    cx: &'p SymCtx<'c>,
    seg: usize,
    cursor: u32,
    entry: u32,
    str_off: u64,
}

impl<'c> Walk<'_, 'c> {
    /// The next entry: its index, its `st_name`, and the row.
    fn next(&mut self) -> Result<Option<(u32, u32, Row<'c>)>, SynthError> {
        loop {
            let Some(seg) = self.plan.segs.get(self.seg) else { return Ok(None) };
            let row = match seg.seg {
                Seg::Null => (self.cursor == 0).then(|| {
                    self.cursor = 1;
                    Row { name: (&b""[..]).into(), info: 0, other: 0, size: 0, at: Where::Undefined }
                }),
                Seg::Linker | Seg::Demoted(_) => {
                    let r = seg.rows.start + self.cursor;
                    if r < seg.rows.end {
                        self.cursor += 1;
                        let id = self.plan.demoted[r as usize].1;
                        match self.cx.classify(id)? {
                            (_, Some(row)) => Some(row),
                            (_, None) => return Err(internal("a demoted symbol lost its place")),
                        }
                    } else {
                        None
                    }
                }
                Seg::Patches => {
                    let r = self.cursor as usize;
                    if r < self.plan.patch_syms.len() {
                        self.cursor += 1;
                        Some(self.plan.patch_row(self.cx, r))
                    } else {
                        None
                    }
                }
                Seg::Locals(file) => {
                    let n = self.cx.syms.locals(file);
                    let mut found = None;
                    while self.cursor < n {
                        let i = self.cursor;
                        self.cursor += 1;
                        if let Some(row) = self.cx.local(file, i)? {
                            found = Some(row);
                            break;
                        }
                    }
                    found
                }
                Seg::Globals => {
                    let n = self.cx.syms.globals();
                    let mut found = None;
                    while self.cursor < n {
                        let id = SymbolId(self.cursor);
                        self.cursor += 1;
                        if let (Class3::Global, Some(row)) = self.cx.classify(id)? {
                            found = Some(row);
                            break;
                        }
                    }
                    found
                }
            };
            if let Some(row) = row {
                let entry = self.entry;
                self.entry += 1;
                let st_name = fixed_name(&row.name).unwrap_or_else(|| {
                    let at = self.str_off;
                    self.str_off += region_len(&row.name);
                    at as u32
                });
                return Ok(Some((entry, st_name, row)));
            }
            self.seg += 1;
            self.cursor = 0;
            if let Some(next) = self.plan.segs.get(self.seg) {
                debug_assert_eq!(next.entry, self.entry, "segment entries are contiguous");
                debug_assert_eq!(next.str_off, self.str_off, "segment strings are contiguous");
            }
        }
    }
}

/// The per-file result of the counting pass.
#[derive(Debug, Default)]
struct FileCount {
    kept: u32,
    bytes: u64,
    /// `(kept so far, input cursor, bytes so far)` every `CHECKPOINT`.
    checks: Vec<(u32, u32, u64)>,
}

impl SymtabPlan {
    /// The plan of S-73 to S-88 (`-s` has none: the caller does not call
    /// this, S-84). `patches` is `Placement.patches` (empty before
    /// layout's fixed point).
    pub fn new(
        cx: &SymCtx<'_>,
        class: Class,
        patches: &[Patch],
        facts: PatchFacts,
        pool: &Pool,
        diag: &Collector,
    ) -> Result<Self, Failed> {
        let n_files = cx.syms.files();
        let (gblocks, mut demoted, files) = count(cx, pool, diag)?;
        // S-88: a counting sort of the demoted by defining file; the
        // order within a file is `SymbolId` (stable).
        pool.sort_by(&mut demoted, |a, b| a.0.cmp(&b.0));
        if demoted.iter().any(|d| d.0.0 >= n_files) {
            return fail(diag, key(0), internal("a demoted symbol's file is not a file"));
        }
        let (patch_syms, patch_bytes) = patch_syms(cx.syms, patches, facts);
        // The segments, in output order (S-74, S-75).
        let mut segs = Vec::new();
        let mut checks = Vec::new();
        let mut demoted_str = vec![0u64; demoted.len()];
        let mut entry = 0u32;
        let mut str_off = REGION;
        let push = |segs: &mut Vec<SegRow>, seg, len: u32, bytes: u64, rows: Range<u32>, entry: &mut u32, str_off: &mut u64| {
            segs.push(SegRow { seg, entry: *entry, len, str_off: *str_off, rows });
            *entry += len;
            *str_off += bytes;
        };
        let mut symtab_locals = vec![Span::default(); n_files as usize];
        push(&mut segs, Seg::Null, 1, 0, 0..0, &mut entry, &mut str_off);
        let file_rows = |f: u32| {
            let lo = demoted.partition_point(|d| d.0.0 < f) as u32;
            let hi = demoted.partition_point(|d| d.0.0 <= f) as u32;
            lo..hi
        };
        let demoted_rows = |rows: Range<u32>, str_off: u64, out: &mut Vec<u64>| -> u64 {
            let mut o = str_off;
            for r in rows {
                out[r as usize] = o;
                o += demoted[r as usize].2;
            }
            o - str_off
        };
        // The internal file's block.
        let block_start = entry;
        let rows = file_rows(0);
        let bytes = demoted_rows(rows.clone(), str_off, &mut demoted_str);
        push(&mut segs, Seg::Linker, rows.len() as u32, bytes, rows, &mut entry, &mut str_off);
        let mut patch_str = Vec::with_capacity(patch_syms.len());
        let mut o = str_off;
        for b in &patch_bytes {
            patch_str.push(o);
            o += b;
        }
        push(&mut segs, Seg::Patches, patch_syms.len() as u32, o - str_off, 0..0, &mut entry, &mut str_off);
        if let Some(s) = symtab_locals.first_mut() {
            *s = Span { start: block_start, len: entry - block_start };
        }
        for (f, count) in files.iter().enumerate().skip(1) {
            let file = FileId(f as u32);
            let block_start = entry;
            let seg = segs.len() as u32;
            checks.push(Check { entry, seg, cursor: 0, str_off });
            for &(kept, cursor, bytes) in &count.checks {
                checks.push(Check { entry: entry + kept, seg, cursor, str_off: str_off + bytes });
            }
            push(&mut segs, Seg::Locals(file), count.kept, count.bytes, 0..0, &mut entry, &mut str_off);
            let rows = file_rows(f as u32);
            let bytes = demoted_rows(rows.clone(), str_off, &mut demoted_str);
            push(&mut segs, Seg::Demoted(file), rows.len() as u32, bytes, rows, &mut entry, &mut str_off);
            symtab_locals[f] = Span { start: block_start, len: entry - block_start };
        }
        let locals = entry;
        let gseg = segs.len() as u32;
        let (mut gentry, mut gstr) = (entry, str_off);
        for (b, &(k, bytes)) in gblocks.iter().enumerate() {
            checks.push(Check { entry: gentry, seg: gseg, cursor: b as u32 * CHECKPOINT, str_off: gstr });
            gentry += k;
            gstr += bytes;
        }
        push(&mut segs, Seg::Globals, gentry - entry, gstr - str_off, 0..0, &mut entry, &mut str_off);
        let Ok(symtab_locals) = Column::from_vec(symtab_locals) else {
            return fail(diag, key(0), internal("too many files"));
        };
        Ok(Self {
            class,
            facts,
            segs: segs.into(),
            checks: checks.into(),
            demoted: demoted.into_iter().map(|d| (d.0, d.1)).collect(),
            demoted_str: demoted_str.into(),
            patches: patches.into(),
            patch_syms: patch_syms.into(),
            patch_str: patch_str.into(),
            entries: entry,
            locals,
            strtab: str_off,
            symtab_locals,
        })
    }

    fn patch_row<'c>(&self, cx: &SymCtx<'c>, r: usize) -> Row<'c> {
        let syms: &'c dyn SymSource = cx.syms;
        match self.patch_syms[r] {
            PatchSym::Mapping(p) => Row {
                name: (&b"$x"[..]).into(),
                info: STB_LOCAL << 4 | STT_NOTYPE,
                other: 0,
                size: 0,
                at: Where::Patch(p),
            },
            PatchSym::Name(p) => {
                let (name, size): (std::borrow::Cow<'c, [u8]>, u64) = match self.patches[p].kind {
                    PatchKind::Thunk(t) => (thunk_name(syms, t).into(), self.facts.thunk_size),
                    PatchKind::Erratum { .. } => (ERRATUM_843419.into(), ERRATUM_843419_SIZE),
                };
                Row { name, info: STB_LOCAL << 4 | STT_FUNC, other: 0, size, at: Where::Patch(p) }
            }
        }
    }

    /// The number of `.symtab` entries, the null entry included.
    #[must_use]
    pub fn entries(&self) -> u32 {
        self.entries
    }

    /// `sh_info` (S-73): the local entries, the null entry included.
    #[must_use]
    pub fn sh_info(&self) -> u32 {
        self.locals
    }

    /// `Slots.symtab_locals` (S-88): per file its `(first local, count)`,
    /// demoted globals included; file 0 is the internal block.
    #[must_use]
    pub fn symtab_locals(&self) -> &Column<FileId, Span> {
        &self.symtab_locals
    }

    fn entsize(&self) -> u64 {
        match self.class {
            Class::Elf64 => 24,
            Class::Elf32 => 16,
        }
    }

    /// `.symtab`'s size (S-108).
    #[must_use]
    pub fn symtab_size(&self) -> u64 {
        self.entsize() * u64::from(self.entries)
    }

    /// `.strtab`'s size (S-108).
    #[must_use]
    pub fn strtab_size(&self) -> u64 {
        self.strtab
    }

    /// `.symtab_shndx`'s size when it exists (S-85).
    #[must_use]
    pub fn shndx_size(&self) -> u64 {
        4 * u64::from(self.entries)
    }

    /// Whether `.symtab_shndx` exists for an output with `sections`
    /// section headers (S-85): when some section index is at least
    /// `SHN_LORESERVE`. (S-85 asks for it exactly when some entry needs
    /// it; this is the bound known before content, see the landed
    /// record.)
    #[must_use]
    pub fn needs_shndx(sections: u32) -> bool {
        sections > u32::from(SHN_LORESERVE)
    }

    /// A walk from the last checkpoint at or before entry `target` (by
    /// `by_str`: at or before string offset `target`).
    fn walk_from<'p, 'c>(&'p self, cx: &'p SymCtx<'c>, target: u64, by_str: bool) -> Walk<'p, 'c> {
        let seg_at = |pred: &dyn Fn(&SegRow) -> bool| self.segs.partition_point(pred).saturating_sub(1);
        let (seg, entry, str_off) = if by_str {
            let s = seg_at(&|s: &SegRow| s.str_off <= target);
            (s, self.segs[s].entry, self.segs[s].str_off)
        } else {
            let s = seg_at(&|s: &SegRow| u64::from(s.entry) <= target);
            (s, self.segs[s].entry, self.segs[s].str_off)
        };
        let mut w = Walk { plan: self, cx, seg, cursor: 0, entry, str_off };
        let row = &self.segs[seg];
        // Rows with their string offsets stored: a direct lookup.
        let stored: Option<&[u64]> = match row.seg {
            Seg::Linker | Seg::Demoted(_) => self.demoted_str.get(row.rows.start as usize..row.rows.end as usize),
            Seg::Patches => Some(&self.patch_str),
            _ => None,
        };
        if let Some(offs) = stored {
            let k = if by_str {
                offs.partition_point(|&o| o <= target).saturating_sub(1)
            } else {
                usize::try_from(target - u64::from(row.entry)).unwrap_or(usize::MAX)
            };
            if let Some(&o) = offs.get(k) {
                w.cursor = k as u32;
                w.entry = row.entry + k as u32;
                w.str_off = o;
            }
            return w;
        }
        let c = self.checks.partition_point(|c| {
            if by_str { c.str_off <= target } else { u64::from(c.entry) <= target }
        });
        if let Some(c) = c.checked_sub(1).map(|c| self.checks[c])
            && c.seg as usize == seg
        {
            w.cursor = c.cursor;
            w.entry = c.entry;
            w.str_off = c.str_off;
        }
        w
    }

    /// The output section index and value of a row (S-81, S-82, S-122 to
    /// S-124), as `(shndx, value)` with `shndx` unreduced.
    fn resolve(addrs: &dyn SymAddrs, row: &Row<'_>) -> Result<(u32, u64), SynthError> {
        let name = || String::from_utf8_lossy(&row.name).into_owned();
        let (shndx, addr) = match row.at {
            Where::Absolute(v) => return Ok((u32::from(SHN_ABS), v)),
            Where::Undefined => return Ok((u32::from(SHN_UNDEF), 0)),
            Where::Section { section, value } => addrs
                .section(section, value)
                .ok_or_else(|| internal::<SynthError>(format!("symbol {}: section {} has no place", name(), section.0)))?,
            Where::Eh(o) => {
                let (s, a) = addrs.eh_frame().ok_or_else(|| internal::<SynthError>(".eh_frame has no place"))?;
                (s, a + o)
            }
            Where::Common(id) => addrs
                .common(id)
                .ok_or_else(|| internal::<SynthError>(format!("common symbol {} has no allocation", name())))?,
            Where::Internal(id) => match addrs.internal(id) {
                Some(InternalValue::Absolute(v)) => return Ok((u32::from(SHN_ABS), v)),
                Some(InternalValue::At { shndx, addr }) => (shndx, addr),
                // S-124 (anomaly A11): a defined linker symbol has a value.
                None => {
                    return Err(internal::<SynthError>(format!(
                        "linker-defined symbol {} has no value (spec/synth.md S-124)",
                        name()
                    )));
                }
            },
            Where::Patch(p) => {
                addrs.patch(p).ok_or_else(|| internal::<SynthError>(format!("patch {p} has no place")))?
            }
        };
        // S-122 (anomaly A10): the index names the section holding the
        // address, its end included.
        let held = addrs
            .section_range(shndx)
            .is_some_and(|(start, size)| start <= addr && addr <= start.saturating_add(size));
        if !held {
            return Err(internal::<SynthError>(format!(
                "symbol {}: value {addr:#x} is outside section {shndx} (spec/synth.md S-122)",
                name()
            )));
        }
        // S-81: a TLS symbol's value is its offset in the template.
        if row.info & 0xf == STT_TLS {
            let start = addrs.tls_start().ok_or_else(|| internal::<SynthError>("a TLS symbol without PT_TLS"))?;
            return Ok((shndx, addr.wrapping_sub(start)));
        }
        Ok((shndx, addr))
    }

    /// Fills the window's range of `.symtab` (S-73, S-81, S-82).
    pub fn write_symtab(
        &self,
        cx: &SymCtx<'_>,
        addrs: &dyn SymAddrs,
        out: &mut Window<'_>,
    ) -> Result<(), SynthError> {
        let e = self.entsize();
        let mut walk = self.walk_from(cx, out.start() / e, false);
        while let Some((entry, st_name, row)) = walk.next()? {
            let at = u64::from(entry) * e;
            if at >= out.end() {
                break;
            }
            if at + e <= out.start() {
                continue;
            }
            let (shndx, value) =
                if entry == 0 { (0, 0) } else { Self::resolve(addrs, &row)? };
            let shndx16 = if shndx >= u32::from(SHN_LORESERVE)
                && shndx != u32::from(SHN_ABS)
            {
                SHN_XINDEX
            } else {
                shndx as u16
            };
            let mut buf = [0u8; 24];
            match self.class {
                Class::Elf64 => {
                    buf[0..4].copy_from_slice(&st_name.to_le_bytes());
                    buf[4] = row.info;
                    buf[5] = row.other;
                    buf[6..8].copy_from_slice(&shndx16.to_le_bytes());
                    buf[8..16].copy_from_slice(&value.to_le_bytes());
                    buf[16..24].copy_from_slice(&row.size.to_le_bytes());
                }
                Class::Elf32 => {
                    buf[0..4].copy_from_slice(&st_name.to_le_bytes());
                    buf[4..8].copy_from_slice(&(value as u32).to_le_bytes());
                    buf[8..12].copy_from_slice(&(row.size as u32).to_le_bytes());
                    buf[12] = row.info;
                    buf[13] = row.other;
                    buf[14..16].copy_from_slice(&shndx16.to_le_bytes());
                }
            }
            out.put(at, &buf[..e as usize]);
        }
        Ok(())
    }

    /// Fills the window's range of `.symtab_shndx` (S-85).
    pub fn write_shndx(
        &self,
        cx: &SymCtx<'_>,
        addrs: &dyn SymAddrs,
        out: &mut Window<'_>,
    ) -> Result<(), SynthError> {
        let mut walk = self.walk_from(cx, out.start() / 4, false);
        while let Some((entry, _, row)) = walk.next()? {
            let at = u64::from(entry) * 4;
            if at >= out.end() {
                break;
            }
            if at + 4 <= out.start() {
                continue;
            }
            let shndx = if entry == 0 { 0 } else { Self::resolve(addrs, &row)?.0 };
            let real = if shndx >= u32::from(SHN_LORESERVE) && shndx != u32::from(SHN_ABS) {
                shndx
            } else {
                0
            };
            out.word(at, 4, u64::from(real));
        }
        Ok(())
    }

    /// Fills the window's range of `.strtab` (S-86).
    pub fn write_strtab(&self, cx: &SymCtx<'_>, out: &mut Window<'_>) -> Result<(), SynthError> {
        out.put(0, FIXED);
        if out.end() <= REGION {
            return Ok(());
        }
        let mut walk = self.walk_from(cx, out.start().max(REGION), true);
        loop {
            let before = walk.str_off;
            if before >= out.end() {
                break;
            }
            let Some((_, _, row)) = walk.next()? else { break };
            if fixed_name(&row.name).is_some() {
                continue;
            }
            out.put(before, &row.name);
            out.zero(before + row.name.len() as u64, 1);
        }
        Ok(())
    }
}

/// Global blocks' `(kept, name bytes)`; demoted `(file, id, name bytes)`.
type Counted = (Vec<(u32, u64)>, Vec<(FileId, SymbolId, u64)>, Vec<FileCount>);
type GBlock = Result<(u32, u64, Vec<(FileId, SymbolId, u64)>), (SymbolId, SynthError)>;

/// The counting pass (S-75 to S-78): per block of [`CHECKPOINT`] global
/// ids the kept ones' count and name bytes and the demoted ones; per
/// file its kept locals with a checkpoint every [`CHECKPOINT`] inputs.
fn count(cx: &SymCtx<'_>, pool: &Pool, diag: &Collector) -> Result<Counted, Failed> {
    let n_globals = cx.syms.globals();
    let n_files = cx.syms.files();
    let blocks = n_globals.div_ceil(CHECKPOINT) as usize;
    let per_block: Vec<GBlock> = pool.map(blocks, |b| {
        let start = b as u32 * CHECKPOINT;
        let end = (start + CHECKPOINT).min(n_globals);
        let (mut kept, mut bytes, mut demoted) = (0, 0, Vec::new());
        for id in (start..end).map(SymbolId) {
            match cx.classify(id).map_err(|d| (id, d))? {
                (Class3::Global, Some(row)) => {
                    kept += 1;
                    bytes += region_len(&row.name);
                }
                (Class3::Demoted(f), Some(row)) => demoted.push((f, id, region_len(&row.name))),
                _ => {}
            }
        }
        Ok((kept, bytes, demoted))
    });
    let mut gblocks = Vec::with_capacity(blocks);
    let mut demoted = Vec::new();
    let mut failed = false;
    for b in per_block {
        match b {
            Ok((k, bytes, d)) => {
                gblocks.push((k, bytes));
                demoted.extend(d);
            }
            Err((id, d)) => {
                diag.emit(key(u64::from(id.0)), d.into_diagnostic());
                failed = true;
            }
        }
    }
    // Locals per file.
    let per_file: Vec<Result<FileCount, (FileId, u32, SynthError)>> =
        pool.map(n_files as usize, |f| {
            let file = FileId(f as u32);
            let mut c = FileCount::default();
            for i in 0..cx.syms.locals(file) {
                // A checkpoint every CHECKPOINT input locals (I-104).
                if i % CHECKPOINT == 0 && i > 0 {
                    c.checks.push((c.kept, i, c.bytes));
                }
                if let Some(row) = cx.local(file, i).map_err(|d| (file, i, d))? {
                    c.kept += 1;
                    c.bytes += region_len(&row.name);
                }
            }
            Ok(c)
        });
    let mut files = Vec::with_capacity(n_files as usize);
    for r in per_file {
        match r {
            Ok(c) => files.push(c),
            Err((file, i, d)) => {
                diag.emit(key(u64::from(file.0) << 32 | u64::from(i + 1)), d.into_diagnostic());
                failed = true;
                files.push(FileCount::default());
            }
        }
    }
    if failed {
        return Err(Failed);
    }
    Ok((gblocks, demoted, files))
}

/// S-80: per patch its `$x` (AArch64) and its name, and their bytes.
fn patch_syms(syms: &dyn SymSource, patches: &[Patch], facts: PatchFacts) -> (Vec<PatchSym>, Vec<u64>) {
    let mut patch_syms = Vec::new();
    let mut patch_bytes = Vec::new();
    for (p, patch) in patches.iter().enumerate() {
        if facts.mapping {
            patch_syms.push(PatchSym::Mapping(p));
            patch_bytes.push(0);
        }
        patch_syms.push(PatchSym::Name(p));
        patch_bytes.push(match patch.kind {
            PatchKind::Thunk(t) => region_len(&thunk_name(syms, t)),
            PatchKind::Erratum { .. } => 0,
        });
    }
    (patch_syms, patch_bytes)
}

/// The fixed table of S-86, for tests.
#[must_use]
pub fn fixed_table() -> &'static [u8] {
    FIXED
}

#[allow(dead_code)]
fn _demoted_str(plan: &SymtabPlan) -> &[u64] {
    &plan.demoted_str
}

#[allow(dead_code)]
fn _patch_str(plan: &SymtabPlan) -> &[u64] {
    &plan.patch_str
}

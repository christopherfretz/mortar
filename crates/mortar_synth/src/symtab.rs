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
use mortar_core::incr::{SymtabBlock, SymtabTables};
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

use crate::eh::{EhPlan, EhSource, EhValue, EhWho};
use crate::error::{Failed, SynthError, fail, internal, key};
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

pub(crate) fn fixed_name(name: &[u8]) -> Option<u32> {
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
pub(crate) fn region_len(name: &[u8]) -> u64 {
    if fixed_name(name).is_some() { 0 } else { name.len() as u64 + 1 }
}

/// S-88 with no `.symtab` (`--strip-all`): every span of a layout's
/// `Slots.symtab_locals` is empty.
///
/// # Errors
///
/// The first file with a span, by name.
pub fn check_no_spans(spans: &Column<FileId, Span>) -> Result<(), SynthError> {
    match spans.as_slice().iter().position(|s| s.len != 0) {
        None => Ok(()),
        Some(f) => Err(internal(format!(
            "SYMTAB_LOCALS: file {f} has a span and the output no .symtab (S-88)"
        ))),
    }
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
    At {
        shndx: u32,
        addr: u64,
    },
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
    /// `plan.shaping.discard`; `None`: neither `-x`, `-X` nor
    /// `--discard-none` (S-78's default).
    pub discard: Option<Discard>,
}

impl std::fmt::Debug for SymCtx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SymCtx").field("discard", &self.discard).finish_non_exhaustive()
    }
}

/// Patch symbols' facts, and the form of the tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatchFacts {
    /// `arch.thunk()`'s size.
    pub thunk_size: u64,
    /// `arch.patch(Erratum::E843419)`'s size (S-80's `st_size`).
    pub erratum_843419_size: u64,
    /// AArch64: a `$x` per patch (S-80).
    pub mapping: bool,
    /// An incremental-mode output (`spec/incr.md` IN-49a, S-88, S-112):
    /// each block followed by fillers up to its capacity, a local tail
    /// reserve, a `.strtab` region per block with slack. `false` in a
    /// plain link (S-73 to S-88 unchanged, no filler, no slack).
    pub room: bool,
}

impl PatchFacts {
    /// The facts of `arch` (`spec/arch.md` §3.6, §3.7), for a plain link.
    #[must_use]
    pub fn of(arch: &dyn mortar_arch::Arch) -> Self {
        let size = |s: Option<&mortar_arch::ThunkShape>| s.map_or(0, |s| u64::from(s.size));
        Self {
            thunk_size: size(arch.thunk()),
            erratum_843419_size: size(arch.patch(mortar_core::layout::Erratum::E843419)),
            mapping: arch.facts().machine == mortar_elf::Machine::AArch64,
            room: false,
        }
    }
}

/// IN-49a's room after a block of `count` entries and `bytes` of names:
/// `(fillers, .strtab slack)`. A block with no entry is no block (S-74):
/// no room.
#[must_use]
pub fn block_room(count: u32, bytes: u64) -> (u32, u64) {
    if count == 0 {
        (0, 0)
    } else {
        (BLOCK_MIN.max(count / BLOCK_DIV), REGION_MIN.max(bytes / REGION_DIV))
    }
}

/// IN-49a's local tail reserve after the last block, for `locals` local
/// entries (fillers and the null entry not counted): max(64, locals/32).
#[must_use]
pub fn tail_room(locals: u32) -> u32 {
    TAIL_MIN.max(locals / TAIL_DIV)
}

/// IN-49a's file slack after `.symtab` or `.strtab` of `size` bytes:
/// max(4 KiB, size/16) (`spec/layout.md` L-72: the layout's tail, the same
/// numbers as `mortar_layout::slack::Slack::tail`).
#[must_use]
pub fn file_slack(size: u64) -> u64 {
    FILE_SLACK_MIN.max(size / FILE_SLACK_DIV)
}

/// IN-49a's constants: a block's fillers max(2, count/8), a region's
/// slack max(16, len/8), the tail reserve max(64, locals/32), the file
/// slack max(4 KiB, size/16).
pub const BLOCK_MIN: u32 = 2;
pub const BLOCK_DIV: u32 = 8;
pub const REGION_MIN: u64 = 16;
pub const REGION_DIV: u64 = 8;
pub const TAIL_MIN: u32 = 64;
pub const TAIL_DIV: u32 = 32;
pub const FILE_SLACK_MIN: u64 = 4096;
pub const FILE_SLACK_DIV: u64 = 16;

/// A filler's `st_info` (IN-49a: `STB_LOCAL`, `STT_SECTION`).
pub const FILLER_INFO: u8 = STT_SECTION;
/// A filler's `st_shndx` (IN-49a: section 1).
pub const FILLER_SHNDX: u16 = 1;

/// One output symbol, before addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Where {
    Section {
        section: SectionId,
        value: u64,
    },
    /// `.eh_frame` at this output offset.
    Eh(u64),
    Absolute(u64),
    Common(SymbolId),
    Internal(SymbolId),
    Undefined,
    Patch(usize),
    /// An IN-49a filler: section 1 at its address.
    Filler,
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
    /// IN-49a (incremental mode only): `len` fillers after the block
    /// before it (or, the last one, the local tail reserve), and the
    /// `.strtab` slack of that block's region (the bytes to the next
    /// segment's `str_off`).
    Fill,
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

/// A file's entry ranges (in entries) and their string ranges (in bytes)
/// in `.symtab` and `.strtab` ([`SymtabPlan::file_entries`], WP-094).
pub type FileEntries = (Vec<Range<u32>>, Vec<Range<u64>>);

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
    /// Some row is `STT_GNU_IFUNC` or `STB_GNU_UNIQUE`.
    gnu_symbols: bool,
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
        who: EhWho,
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
        let typ = l.info & 0xf;
        if self.discard == Some(Discard::All) || typ == STT_SECTION {
            return Ok(None);
        }
        if l.name.starts_with(b".L") && typ != STT_FILE {
            // S-78: `-X` drops every `.L` local; the default only those in
            // a split merge section (GNU's `discard_sec_merge`, lld's
            // default policy).
            let in_merge = matches!(l.def, SymDef::Section { section, .. }
                if self.sections.kind(section) == SecKind::Merge);
            match self.discard {
                Some(Discard::Locals) => return Ok(None),
                None if in_merge => return Ok(None),
                _ => {}
            }
        }
        let at = match l.def {
            SymDef::Absolute(v) => Some(Where::Absolute(v)),
            SymDef::Section { section, value } => {
                let who = if is_mapping(l.name) {
                    EhWho::Mapping
                } else {
                    EhWho::Symbol { droppable: !l.referenced }
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
                self.place(section, value, EhWho::Symbol { droppable: false })?
            }
            SymDef::Absolute(v) => Some(Where::Absolute(v)),
            SymDef::Common => Some(Where::Common(id)),
            SymDef::Internal => Some(Where::Internal(id)),
            SymDef::Undefined => g.referenced.then_some(Where::Undefined),
        };
        let Some(at) = at else { return Ok((Class3::Skip, None)) };
        // S-81: a common symbol is `STT_OBJECT` in `.bss`.
        let typ = if g.def == SymDef::Common { STT_OBJECT } else { g.typ };
        // S-77, S-121: hidden and internal definitions are locals, and so
        // are the definitions resolution writes `STB_LOCAL` (a version
        // script's or `--exclude-libs`' local keys, `spec/dyn.md` D-114,
        // WP-150), with their own visibility.
        let local = (vis == STV_HIDDEN || vis == STV_INTERNAL || g.binding == STB_LOCAL)
            && g.def != SymDef::Undefined;
        let binding = if local { STB_LOCAL } else { g.binding };
        let row =
            Row { name: g.name.into(), info: binding << 4 | typ, other: g.other, size: g.size, at };
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
                    Row {
                        name: (&b""[..]).into(),
                        info: 0,
                        other: 0,
                        size: 0,
                        at: Where::Undefined,
                    }
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
                Seg::Fill => (self.cursor < seg.len).then(|| {
                    self.cursor += 1;
                    Row {
                        name: (&b""[..]).into(),
                        info: FILLER_INFO,
                        other: 0,
                        size: 0,
                        at: Where::Filler,
                    }
                }),
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
            let filled = seg.seg == Seg::Fill;
            self.seg += 1;
            self.cursor = 0;
            if let Some(next) = self.plan.segs.get(self.seg) {
                debug_assert_eq!(next.entry, self.entry, "segment entries are contiguous");
                if filled {
                    // IN-49a: the region's slack, which no entry names.
                    debug_assert!(next.str_off >= self.str_off, "a region's slack is not negative");
                    self.str_off = next.str_off;
                }
                debug_assert_eq!(next.str_off, self.str_off, "segment strings are contiguous");
            }
        }
    }
}

/// The segments and checkpoints being laid out, and the next entry and
/// string offset.
struct Segs {
    list: Vec<SegRow>,
    checks: Vec<Check>,
    entry: u32,
    str_off: u64,
}

impl Segs {
    fn push(&mut self, seg: Seg, len: u32, bytes: u64, rows: Range<u32>) {
        self.list.push(SegRow { seg, entry: self.entry, len, str_off: self.str_off, rows });
        self.entry += len;
        self.str_off += bytes;
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
        let (gblocks, mut demoted, files, gnu_symbols) = count(cx, pool, diag)?;
        // S-88: a counting sort of the demoted by defining file; the
        // order within a file is `SymbolId` (stable).
        pool.unit_cost(15).sort_by(&mut demoted, |a, b| a.0.cmp(&b.0));
        if demoted.iter().any(|d| d.0.0 >= n_files) {
            return fail(diag, key(0), internal("a demoted symbol's file is not a file"));
        }
        let (patch_syms, patch_bytes) = patch_syms(cx.syms, patches, facts);
        // The segments, in output order (S-74, S-75).
        let mut b = Segs { list: Vec::new(), checks: Vec::new(), entry: 0, str_off: REGION };
        let mut demoted_str = vec![0u64; demoted.len()];
        let mut symtab_locals = vec![Span::default(); n_files as usize];
        b.push(Seg::Null, 1, 0, 0..0);
        let file_rows = |f: u32| {
            let lo = demoted.partition_point(|d| d.0.0 < f) as u32;
            let hi = demoted.partition_point(|d| d.0.0 <= f) as u32;
            lo..hi
        };
        // Each demoted row's first region byte; the rows' bytes.
        let demoted_rows = |rows: Range<u32>, str_off: u64, out: &mut Vec<u64>| -> u64 {
            let mut o = str_off;
            for r in rows {
                out[r as usize] = o;
                o += demoted[r as usize].2;
            }
            o - str_off
        };
        // The internal file's block.
        let rows = file_rows(0);
        let bytes = demoted_rows(rows.clone(), b.str_off, &mut demoted_str);
        b.push(Seg::Linker, rows.len() as u32, bytes, rows);
        let mut patch_str = Vec::with_capacity(patch_syms.len());
        let mut o = b.str_off;
        for bytes in &patch_bytes {
            patch_str.push(o);
            o += bytes;
        }
        b.push(Seg::Patches, patch_syms.len() as u32, o - b.str_off, 0..0);
        symtab_locals[0] = Span { start: 1, len: b.entry - 1 };
        // IN-49a: each block's room, the tail reserve after the last.
        // A block's span is its capacity (S-88 in incremental mode: the
        // spans follow IN-49a's capacities, so they stay contiguous).
        let room = |b: &mut Segs, start: u32, str_start: u64| -> u32 {
            if !facts.room {
                return 0;
            }
            let (fill, slack) = block_room(b.entry - start, b.str_off - str_start);
            b.push(Seg::Fill, fill, slack, 0..0);
            fill
        };
        symtab_locals[0].len += room(&mut b, 1, REGION);
        let mut block_entries = b.entry - 1;
        for (f, count) in files.iter().enumerate().skip(1) {
            let block_start = b.entry;
            let block_str = b.str_off;
            let seg = b.list.len() as u32;
            b.checks.push(Check { entry: b.entry, seg, cursor: 0, str_off: b.str_off });
            for &(kept, cursor, bytes) in &count.checks {
                let c = Check { entry: b.entry + kept, seg, cursor, str_off: b.str_off + bytes };
                b.checks.push(c);
            }
            b.push(Seg::Locals(FileId(f as u32)), count.kept, count.bytes, 0..0);
            let rows = file_rows(f as u32);
            let bytes = demoted_rows(rows.clone(), b.str_off, &mut demoted_str);
            b.push(Seg::Demoted(FileId(f as u32)), rows.len() as u32, bytes, rows);
            symtab_locals[f] = Span { start: block_start, len: b.entry - block_start };
            block_entries += b.entry - block_start;
            symtab_locals[f].len += room(&mut b, block_start, block_str);
        }
        if facts.room {
            b.push(Seg::Fill, tail_room(block_entries), 0, 0..0);
        }
        let locals = b.entry;
        let gseg = b.list.len() as u32;
        let (mut gentry, mut gstr) = (b.entry, b.str_off);
        for (k, &(kept, bytes)) in gblocks.iter().enumerate() {
            let cursor = k as u32 * CHECKPOINT;
            b.checks.push(Check { entry: gentry, seg: gseg, cursor, str_off: gstr });
            gentry += kept;
            gstr += bytes;
        }
        b.push(Seg::Globals, gentry - b.entry, gstr - b.str_off, 0..0);
        let Segs { list: segs, checks, entry, str_off } = b;
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
            gnu_symbols,
        })
    }

    /// This plan with `patches` in place of its own: what
    /// [`SymtabPlan::new`] gives for them with the same `cx`, without
    /// counting the symbols again. The patches' rows are one segment
    /// after the internal file's (S-74), so everything after it moves by
    /// the difference in rows and string bytes.
    #[must_use]
    pub fn with_patches(&self, syms: &dyn SymSource, patches: &[Patch]) -> Self {
        let (patch_syms, patch_bytes) = patch_syms(syms, patches, self.facts);
        let Some(at) = self.segs.iter().position(|s| s.seg == Seg::Patches) else {
            return self.clone();
        };
        let old = &self.segs[at];
        let old_bytes = self.segs.get(at + 1).map_or(self.strtab, |n| n.str_off) - old.str_off;
        let len = patch_syms.len() as u32;
        let bytes: u64 = patch_bytes.iter().sum();
        if self.facts.room {
            return self.relaid(at, len, bytes, patch_syms, &patch_bytes, patches);
        }
        // Entries and bytes after the segment, in wrapping arithmetic:
        // each shifted value is the old one plus the difference.
        let d_entry = len.wrapping_sub(old.len);
        let d_str = bytes.wrapping_sub(old_bytes);
        let mut patch_str = Vec::with_capacity(patch_bytes.len());
        let mut o = old.str_off;
        for b in &patch_bytes {
            patch_str.push(o);
            o += b;
        }
        let mut segs = self.segs.to_vec();
        segs[at].len = len;
        for s in &mut segs[at + 1..] {
            s.entry = s.entry.wrapping_add(d_entry);
            s.str_off = s.str_off.wrapping_add(d_str);
        }
        // The checkpoints are all of segments after it (files 1.., the
        // globals).
        let checks: Box<[Check]> = self
            .checks
            .iter()
            .map(|c| Check {
                entry: c.entry.wrapping_add(d_entry),
                str_off: c.str_off.wrapping_add(d_str),
                ..*c
            })
            .collect();
        // The internal file's demoted rows come first and before it.
        let linker = segs.iter().find(|s| s.seg == Seg::Linker).map_or(0, |s| s.rows.end as usize);
        let demoted_str: Box<[u64]> = self
            .demoted_str
            .iter()
            .enumerate()
            .map(|(r, &o)| if r < linker { o } else { o.wrapping_add(d_str) })
            .collect();
        // File 0's block holds the patches; each later block moves.
        let locals: Vec<Span> = self
            .symtab_locals
            .as_slice()
            .iter()
            .enumerate()
            .map(|(f, s)| {
                if f == 0 {
                    Span { start: s.start, len: s.len.wrapping_add(d_entry) }
                } else {
                    Span { start: s.start.wrapping_add(d_entry), len: s.len }
                }
            })
            .collect();
        Self {
            class: self.class,
            facts: self.facts,
            segs: segs.into(),
            checks,
            demoted: self.demoted.clone(),
            demoted_str,
            patches: patches.into(),
            patch_syms: patch_syms.into(),
            patch_str: patch_str.into(),
            entries: self.entries.wrapping_add(d_entry),
            locals: self.locals.wrapping_add(d_entry),
            strtab: self.strtab.wrapping_add(d_str),
            symtab_locals: Column::from_vec(locals).unwrap_or_default(),
            gnu_symbols: self.gnu_symbols,
        }
    }

    /// [`SymtabPlan::with_patches`] in an incremental-mode output
    /// (IN-49a): the patch segment `at` gets `len` rows and `bytes` name
    /// bytes, so the internal block's room and the tail reserve change
    /// too; every segment is placed again from its rows and bytes, and
    /// each checkpoint and stored string offset moves with its segment.
    fn relaid(
        &self,
        at: usize,
        len: u32,
        bytes: u64,
        patch_syms: Vec<PatchSym>,
        patch_bytes: &[u64],
        patches: &[Patch],
    ) -> Self {
        let n = self.segs.len();
        let end = |i: usize| self.segs.get(i + 1).map_or(self.strtab, |s| s.str_off);
        let mut sizes: Vec<(u32, u64)> =
            (0..n).map(|i| (self.segs[i].len, end(i) - self.segs[i].str_off)).collect();
        sizes[at] = (len, bytes);
        // The room of each block, then the tail reserve (the last fill).
        let last_fill = self.segs.iter().rposition(|s| s.seg == Seg::Fill);
        let (mut block, mut total) = ((0u32, 0u64), 0u32);
        for (i, (s, size)) in self.segs.iter().zip(sizes.iter_mut()).enumerate() {
            match s.seg {
                Seg::Null | Seg::Globals => block = (0, 0),
                Seg::Fill if Some(i) == last_fill => *size = (tail_room(total), 0),
                Seg::Fill => {
                    *size = block_room(block.0, block.1);
                    total += block.0;
                    block = (0, 0);
                }
                _ => block = (block.0 + size.0, block.1 + size.1),
            }
        }
        let mut segs = self.segs.to_vec();
        let mut delta = Vec::with_capacity(n);
        let (mut entry, mut str_off) = (0u32, REGION);
        for (s, &(l, b)) in segs.iter_mut().zip(&sizes) {
            delta.push((entry.wrapping_sub(s.entry), str_off.wrapping_sub(s.str_off)));
            s.entry = entry;
            s.str_off = str_off;
            s.len = l;
            entry += l;
            str_off += b;
        }
        let checks: Box<[Check]> = self
            .checks
            .iter()
            .map(|c| {
                let (de, ds) = delta[c.seg as usize];
                Check { entry: c.entry.wrapping_add(de), str_off: c.str_off.wrapping_add(ds), ..*c }
            })
            .collect();
        let mut demoted_str = self.demoted_str.to_vec();
        for (s, &(_, ds)) in segs.iter().zip(&delta) {
            for r in s.rows.clone() {
                demoted_str[r as usize] = demoted_str[r as usize].wrapping_add(ds);
            }
        }
        let mut patch_str = Vec::with_capacity(patch_bytes.len());
        let mut o = segs[at].str_off;
        for b in patch_bytes {
            patch_str.push(o);
            o += b;
        }
        let mut locals = self.symtab_locals.as_slice().to_vec();
        let mut globals_at = entry;
        for s in &segs {
            match s.seg {
                Seg::Linker => locals[0] = Span { start: s.entry, len: s.len },
                Seg::Patches => locals[0].len += s.len,
                Seg::Locals(f) => locals[f.0 as usize] = Span { start: s.entry, len: s.len },
                Seg::Demoted(f) => locals[f.0 as usize].len += s.len,
                Seg::Globals => globals_at = s.entry,
                Seg::Null | Seg::Fill => {}
            }
        }
        // The spans are the capacities (S-88 in incremental mode): each
        // block's fill is its file's, the tail reserve is no file's.
        let last_fill = segs.iter().rposition(|s| s.seg == Seg::Fill);
        let mut owner = None;
        for (i, s) in segs.iter().enumerate() {
            match s.seg {
                Seg::Linker | Seg::Patches => owner = Some(0),
                Seg::Locals(f) | Seg::Demoted(f) => owner = Some(f.0 as usize),
                Seg::Fill if Some(i) != last_fill => {
                    if let Some(f) = owner.take() {
                        locals[f].len += s.len;
                    }
                }
                _ => owner = None,
            }
        }
        Self {
            class: self.class,
            facts: self.facts,
            segs: segs.into(),
            checks,
            demoted: self.demoted.clone(),
            demoted_str: demoted_str.into(),
            patches: patches.into(),
            patch_syms: patch_syms.into(),
            patch_str: patch_str.into(),
            entries: entry,
            locals: globals_at,
            strtab: str_off,
            symtab_locals: Column::from_vec(locals).unwrap_or_default(),
            gnu_symbols: self.gnu_symbols,
        }
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
                    PatchKind::Erratum { .. } => {
                        (ERRATUM_843419.into(), self.facts.erratum_843419_size)
                    }
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
    /// In an incremental-mode output the count is the block's capacity
    /// (IN-49a: its entries and its fillers), so the spans stay
    /// contiguous; the tail reserve is no file's.
    #[must_use]
    pub fn symtab_locals(&self) -> &Column<FileId, Span> {
        &self.symtab_locals
    }

    /// Whether the tables have IN-49a's form (an incremental-mode output).
    #[must_use]
    pub fn room(&self) -> bool {
        self.facts.room
    }

    /// IN-49a's blocks of an incremental-mode output, in `.symtab` order
    /// (the internal file's first, as `FileId(0)`; a file with no entry
    /// has no block, S-74): `(first, count, capacity)` and the `.strtab`
    /// region `(offset, bytes used, bytes held)`. What the state's
    /// `SYMTAB` rows (IN-18e) and the map's `symblock` records
    /// (`spec/plumb.md` §3.2) say. `None` in a plain link.
    #[must_use]
    pub fn incr_blocks(&self) -> Option<Vec<(FileId, SymtabBlock)>> {
        if !self.facts.room {
            return None;
        }
        let end = |i: usize| self.segs.get(i + 1).map_or(self.strtab, |s| s.str_off);
        let mut out = Vec::new();
        // The open block: its file, first entry, first byte, entries, bytes.
        let mut open: Option<(FileId, u32, u64, u32, u64)> = None;
        let last_fill = self.segs.iter().rposition(|s| s.seg == Seg::Fill);
        for (i, s) in self.segs.iter().enumerate() {
            let file = match s.seg {
                Seg::Linker | Seg::Patches => FileId(0),
                Seg::Locals(f) | Seg::Demoted(f) => f,
                Seg::Fill if Some(i) != last_fill => {
                    if let Some((file, first, str_at, count, bytes)) = open.take()
                        && count > 0
                    {
                        let slack = end(i) - s.str_off;
                        out.push((
                            file,
                            SymtabBlock {
                                first,
                                count,
                                capacity: count + s.len,
                                strtab_offset: str_at,
                                strtab_len: bytes,
                                strtab_capacity: bytes + slack,
                            },
                        ));
                    }
                    continue;
                }
                Seg::Null | Seg::Fill | Seg::Globals => continue,
            };
            let b = open.get_or_insert((file, s.entry, s.str_off, 0, 0));
            b.3 += s.len;
            b.4 += end(i) - s.str_off;
        }
        Some(out)
    }

    /// The global area of an incremental-mode output (IN-49a: dense, from
    /// `sh_info` to the end; `SYMTAB`'s row of file 0): its first entry,
    /// its entries (capacity the same), its names' bytes. `None` in a
    /// plain link.
    #[must_use]
    pub fn incr_global_area(&self) -> Option<SymtabBlock> {
        if !self.facts.room {
            return None;
        }
        let g = self.segs.iter().find(|s| s.seg == Seg::Globals)?;
        let bytes = self.strtab - g.str_off;
        Some(SymtabBlock {
            first: g.entry,
            count: g.len,
            capacity: g.len,
            strtab_offset: g.str_off,
            strtab_len: bytes,
            strtab_capacity: bytes,
        })
    }

    /// The sizes of an incremental-mode output's tables (IN-49a; the
    /// state's `META.symtab`): `sh_info`, the local tail reserve, each
    /// table's `sh_size` and its file slack ([`file_slack`], the layout's
    /// L-72 tail). `None` in a plain link.
    #[must_use]
    pub fn incr_tables(&self) -> Option<SymtabTables> {
        if !self.facts.room {
            return None;
        }
        let tail = self.segs.iter().rfind(|s| s.seg == Seg::Fill)?;
        let symtab_size = self.symtab_size();
        Some(SymtabTables {
            sh_info: self.locals,
            tail_first: tail.entry,
            tail_capacity: tail.len,
            symtab_size,
            symtab_slack: file_slack(symtab_size),
            strtab_size: self.strtab,
            strtab_slack: file_slack(self.strtab),
        })
    }

    /// S-88: whether a layout's `Slots.symtab_locals` are this plan's
    /// spans. Format version 1 (`spec/incr.md` IN-18a) admits no other
    /// value: each file's block starts where the one before ends, from
    /// entry 1, and holds its kept locals and demoted globals, so the
    /// spans follow from the plan; an update (IN-49a) reads the previous
    /// link's from its state. (Q8's per-file ranges with holes would admit
    /// others.)
    ///
    /// # Errors
    ///
    /// The first file whose span differs, by name.
    pub fn check_spans(&self, spans: &Column<FileId, Span>) -> Result<(), SynthError> {
        if spans.len() != self.symtab_locals.len() {
            return Err(internal("SYMTAB_LOCALS: not one span per file (S-88)"));
        }
        let mine = self.symtab_locals.as_slice().iter();
        match spans.as_slice().iter().zip(mine).position(|(a, b)| a != b) {
            None => Ok(()),
            Some(f) => {
                let (a, b) = (spans.as_slice()[f], self.symtab_locals.as_slice()[f]);
                Err(internal(format!(
                    "SYMTAB_LOCALS: file {f}'s span is entries {}+{}, its block {}+{} (S-88)",
                    a.start, a.len, b.start, b.len
                )))
            }
        }
    }

    fn entsize(&self) -> u64 {
        match self.class {
            Class::Elf64 => 24,
            Class::Elf32 => 16,
        }
    }

    /// Whether `.symtab` holds an `STT_GNU_IFUNC` or `STB_GNU_UNIQUE`
    /// symbol (the output's `EI_OSABI`, image.md I-40).
    #[must_use]
    pub fn gnu_symbols(&self) -> bool {
        self.gnu_symbols
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
        let seg_at =
            |pred: &dyn Fn(&SegRow) -> bool| self.segs.partition_point(pred).saturating_sub(1);
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
            Seg::Linker | Seg::Demoted(_) => {
                self.demoted_str.get(row.rows.start as usize..row.rows.end as usize)
            }
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
        // A linker-defined value in the file header or the program
        // headers (layout §13.1: `__ehdr_start`, `__executable_start`, the
        // absent arrays' base) takes index 1, below that section's start
        // (S-122's header case; GNU, lld and mold write the same).
        let mut in_headers = false;
        let (shndx, addr) = match row.at {
            Where::Absolute(v) => return Ok((u32::from(SHN_ABS), v)),
            Where::Undefined => return Ok((u32::from(SHN_UNDEF), 0)),
            Where::Section { section, value } => {
                addrs.section(section, value).ok_or_else(|| {
                    internal::<SynthError>(format!(
                        "symbol {}: section {} has no place",
                        name(),
                        section.0
                    ))
                })?
            }
            Where::Eh(o) => {
                let (s, a) = addrs
                    .eh_frame()
                    .ok_or_else(|| internal::<SynthError>(".eh_frame has no place"))?;
                (s, a + o)
            }
            Where::Common(id) => addrs.common(id).ok_or_else(|| {
                internal::<SynthError>(format!("common symbol {} has no allocation", name()))
            })?,
            Where::Internal(id) => match addrs.internal(id) {
                Some(InternalValue::Absolute(v)) => return Ok((u32::from(SHN_ABS), v)),
                Some(InternalValue::At { shndx, addr }) => {
                    in_headers = shndx == 1;
                    (shndx, addr)
                }
                // S-124 (anomaly A11): a defined linker symbol has a value.
                None => {
                    return Err(internal::<SynthError>(format!(
                        "linker-defined symbol {} has no value (spec/synth.md S-124)",
                        name()
                    )));
                }
            },
            Where::Patch(p) => addrs
                .patch(p)
                .ok_or_else(|| internal::<SynthError>(format!("patch {p} has no place")))?,
            // IN-49a: section 1 and its `sh_addr`.
            Where::Filler => {
                let (addr, _) = addrs.section_range(u32::from(FILLER_SHNDX)).ok_or_else(|| {
                    internal::<SynthError>("an IN-49a filler without a section 1")
                })?;
                return Ok((u32::from(FILLER_SHNDX), addr));
            }
        };
        // S-122 (anomaly A10): the index names the section holding the
        // address, its end included.
        let held = addrs.section_range(shndx).is_some_and(|(start, size)| {
            (start <= addr || in_headers) && addr <= start.saturating_add(size)
        });
        if !held {
            return Err(internal::<SynthError>(format!(
                "symbol {}: value {addr:#x} is outside section {shndx} (spec/synth.md S-122)",
                name()
            )));
        }
        // S-81: a TLS symbol's value is its offset in the template.
        if row.info & 0xf == STT_TLS {
            let start = addrs
                .tls_start()
                .ok_or_else(|| internal::<SynthError>("a TLS symbol without PT_TLS"))?;
            return Ok((shndx, addr.wrapping_sub(start)));
        }
        Ok((shndx, addr))
    }

    /// WP-094 (b) prototype: the entries and string bytes a change of
    /// `file` can change, as `(entry ranges, .strtab byte ranges)`: its
    /// block (its kept locals and demoted globals, S-88) and its strings,
    /// and the entry and name of each kept global in `defined` (the keys
    /// `file` defines, by `SymbolId`). A global's place is found from its
    /// block's checkpoint, at most [`CHECKPOINT`] keys walked.
    ///
    /// # Errors
    ///
    /// The walk's (a row without a place).
    pub fn file_entries(
        &self,
        cx: &SymCtx<'_>,
        file: FileId,
        defined: &[SymbolId],
    ) -> Result<FileEntries, SynthError> {
        let mut entries = Vec::new();
        let mut strings = Vec::new();
        let str_end = |i: usize| self.segs.get(i + 1).map_or(self.strtab, |n| n.str_off);
        let mut mine = false;
        for (i, seg) in self.segs.iter().enumerate() {
            // IN-49a: the fillers and the region's slack after its block
            // are the file's too.
            let filled = mine && seg.seg == Seg::Fill;
            mine = matches!(seg.seg, Seg::Locals(f) | Seg::Demoted(f) if f == file);
            if mine || filled {
                entries.push(seg.entry..seg.entry + seg.len);
                strings.push(seg.str_off..str_end(i));
            }
        }
        let Some(gseg) = self.segs.iter().position(|s| s.seg == Seg::Globals) else {
            return Ok((entries, strings));
        };
        let mut ids = defined.to_vec();
        ids.sort_unstable();
        ids.dedup();
        for id in ids {
            let c = self.checks.partition_point(|c| {
                (c.seg as usize) < gseg || (c.seg as usize == gseg && c.cursor <= id.0)
            });
            let Some(c) =
                c.checked_sub(1).map(|c| self.checks[c]).filter(|c| c.seg as usize == gseg)
            else {
                continue;
            };
            let mut w = Walk {
                plan: self,
                cx,
                seg: gseg,
                cursor: c.cursor,
                entry: c.entry,
                str_off: c.str_off,
            };
            loop {
                let before = w.str_off;
                let Some((entry, _, row)) = w.next()? else { break };
                if w.seg != gseg || w.cursor > id.0 + 1 {
                    break;
                }
                if w.cursor == id.0 + 1 {
                    entries.push(entry..entry + 1);
                    if fixed_name(&row.name).is_none() {
                        strings.push(before..before + region_len(&row.name));
                    }
                    break;
                }
            }
        }
        Ok((entries, strings))
    }

    /// The bytes of one `.symtab` entry.
    #[must_use]
    pub fn entry_size(&self) -> u64 {
        self.entsize()
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
            let (shndx, value) = if entry == 0 { (0, 0) } else { Self::resolve(addrs, &row)? };
            let shndx16 = if shndx >= u32::from(SHN_LORESERVE) && shndx != u32::from(SHN_ABS) {
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
        if self.facts.room {
            // IN-49a: the regions' slack is zeros (no entry names it).
            out.zero(out.start(), out.end() - out.start());
        }
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
            let Some((_, st_name, row)) = walk.next()? else { break };
            if fixed_name(&row.name).is_some() {
                continue;
            }
            // The entry's own offset: past a block's fillers the walk
            // skips the region's slack (IN-49a) before naming it.
            let at = if self.facts.room { u64::from(st_name) } else { before };
            out.put(at, &row.name);
            out.zero(at + row.name.len() as u64, 1);
        }
        Ok(())
    }
}

/// Global blocks' `(kept, name bytes)`; demoted `(file, id, name bytes)`.
type Counted = (Vec<(u32, u64)>, Vec<(FileId, SymbolId, u64)>, Vec<FileCount>, bool);

/// Whether a row is a GNU extension of the gABI that `EI_OSABI` must
/// announce (`STT_GNU_IFUNC`, `STB_GNU_UNIQUE`; image.md I-40's OS/ABI).
fn is_gnu(info: u8) -> bool {
    info & 0xf == 10 || info >> 4 == 10
}
type GBlock = Result<(u32, u64, Vec<(FileId, SymbolId, u64)>), (SymbolId, SynthError)>;

/// The counting pass (S-75 to S-78): per block of [`CHECKPOINT`] global
/// ids the kept ones' count and name bytes and the demoted ones; per
/// file its kept locals with a checkpoint every [`CHECKPOINT`] inputs.
fn count(cx: &SymCtx<'_>, pool: &Pool, diag: &Collector) -> Result<Counted, Failed> {
    let n_globals = cx.syms.globals();
    let n_files = cx.syms.files();
    let blocks = n_globals.div_ceil(CHECKPOINT) as usize;
    let gnu = std::sync::atomic::AtomicBool::new(false);
    let per_block: Vec<GBlock> = pool.unit_cost(25000).map(blocks, |b| {
        let start = b as u32 * CHECKPOINT;
        let end = (start + CHECKPOINT).min(n_globals);
        let (mut kept, mut bytes, mut demoted) = (0, 0, Vec::new());
        for id in (start..end).map(SymbolId) {
            match cx.classify(id).map_err(|d| (id, d))? {
                (Class3::Global, Some(row)) => {
                    kept += 1;
                    bytes += region_len(&row.name);
                    if is_gnu(row.info) {
                        gnu.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                (Class3::Demoted(f), Some(row)) => {
                    if is_gnu(row.info) {
                        gnu.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    demoted.push((f, id, region_len(&row.name)));
                }
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
        pool.unit_cost(700).map(n_files as usize, |f| {
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
                    if is_gnu(row.info) {
                        gnu.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
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
    Ok((gblocks, demoted, files, gnu.into_inner()))
}

/// S-80: per patch its `$x` (AArch64) and its name, and their bytes.
fn patch_syms(
    syms: &dyn SymSource,
    patches: &[Patch],
    facts: PatchFacts,
) -> (Vec<PatchSym>, Vec<u64>) {
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

/// A global's output section index and value exactly as `.symtab`
/// writes them (S-81, S-82, S-122 to S-124; the index unreduced): what
/// `.dynsym` writes for a program definition (`spec/dyn.md` D-23).
/// `None` for a global `.symtab` has no entry for.
///
/// # Errors
///
/// As `.symtab`'s write: `internal-error` for a value without a place.
pub fn global_value(
    cx: &SymCtx<'_>,
    addrs: &dyn SymAddrs,
    id: SymbolId,
) -> Result<Option<(u32, u64)>, SynthError> {
    match cx.classify(id)? {
        (_, Some(row)) => SymtabPlan::resolve(addrs, &row).map(Some),
        (_, None) => Ok(None),
    }
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

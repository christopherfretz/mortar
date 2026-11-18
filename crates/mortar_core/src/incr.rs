//! The incremental update's plain-data types that cross crates
//! (`spec/incr.md` §5.10; WP-213, package U-1 of the cut,
//! `spec/landed/incr/WP-211.md` §C). `mortar_layout`, `mortar_synth` and
//! `mortar_image` do not depend on `mortar_incr`, so what they take from or
//! give to the update lives here. No algorithm: every behaviour is the
//! package's that the type's text names.
//!
//! **The update's id space** (WP-213; every package codes against it). In an
//! update every [`FileId`], [`SymbolId`], [`SectionId`], [`ChunkId`] and
//! [`OutSecId`] at a seam is a **row of the state** (`spec/incr.md` IN-18e):
//! a `FILES`, `SYMBOLS`, `SECTIONS`, `CHUNKS` or `OUTSECS` row. An id at or
//! past the table's row count (as the state was opened, before the patch)
//! is **new to the update**, numbered in this order and nowhere else:
//! - a new `FileId` (an appeared file, or a member newly given a row) is
//!   `FILES`' count plus its position in `ChangedFiles`' list of new files
//!   (`mortar_incr::update::view`); U-2 appends their `FILES` rows in that
//!   order, so the id is the row the patched state gives it;
//! - a new `SymbolId` (a key no `SYMBOLS` row names) is `SYMBOLS`' count
//!   plus its position in `ResolveDelta`'s list of new keys
//!   (`mortar_incr::update::resolve`, U-3); U-2 appends in that order;
//! - a new `SectionId` (every section of a changed, appeared or newly
//!   extracted file as its new bytes give it: the old version's rows are
//!   that file's **old** sections, gone for the update) is `SECTIONS`'
//!   count plus its position in `ChangedFiles`' list of new sections (files
//!   in that list's order, sections in input order). U-2's patch writes
//!   them into the file's range (IN-18e: rewritten in place when the count
//!   fits its capacity, else a new range in the append area) and maps each
//!   new id to the row it gets; no other package renumbers;
//! - `ChunkId` and `OutSecId` have no new ids: an update that would need
//!   one ends `new-output-section` (IN-72 item 10).
//!
//! The section identity of `spec/model.md` §7.1 (anchor, name, ordinal)
//! matches a new section to its old row ([`SectionChange::old`]); a region
//! is keyed by its file and `(O, c)`: the [`OutSecId`] and the `.text`
//! category of `spec/layout.md` L-53 (`mortar_layout::order::text_category`,
//! 0 for a section that is not code).

use std::fmt;

use crate::demands::DemandFlags;
use crate::layout::{Chunk, Defined, Hole, Patch, Place, Placement, Redirect, Segment};
use crate::layout::{SlotKind, SymPlace, Tls};
use crate::{ChunkId, FileId, OutSecId, SectionId, SymKey, SymbolId};

// ---- the reasons ------------------------------------------------------------

/// The word `update-not-implemented` carries (`spec/incr.md` §7): which part
/// of the update is missing. Each word is printed in parentheses after the
/// reason (`update-not-implemented (debug)`). Each entry point returns its
/// word until its package fills it; the join (U-10) leaves only `Debug`
/// and `Meta`, which the G packages delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Unimplemented {
    /// The state format 5 (U-2: `state5::open`, `patch`, `compact`).
    State5,
    /// Delta resolution, extraction, liveness (U-3).
    Resolve,
    /// The refill set (U-3: `update::refill::refill_set`).
    Refill,
    /// The delta layout (U-4).
    Layout,
    /// The per-file symbol tables (U-5).
    Symtab,
    /// Refill and image (U-6).
    Image,
    /// The writes and the crash protocol (U-7).
    Write,
    /// `.mason.meta` units under `--keep-mason-meta` in a changed file
    /// (IN-71d; until G-1's contributions cover it). U-8's entry point
    /// gives it while it is empty.
    Meta,
    /// A changed, appeared or newly extracted file with debug sections or
    /// a split-DWARF skeleton (IN-70; until G-3 joins G-1 and G-2).
    Debug,
    /// A stored `link`-subject warning the update cannot raise again
    /// within O(change) (IN-75a as amended by WP-212, F-13): it is never
    /// repeated from the stored row; the update falls back.
    Diags,
    /// A symbol-table entry named `foo@V` or `foo@@V` in the stored link
    /// (`META.versioned` not 0) or in a changed file's new bytes
    /// (`ChangedFile::versioned` not 0): IN-71's affected keys do not
    /// cover the `also` key a `foo@@V` definition defines (resolve.md
    /// §2; the old version of a file's `@@` is not stored). WP-215 (U-3);
    /// [later] the version of a `@@` definition stored.
    Versioned,
    /// A link under `--gc-sections` (`OptionKeys::gc_sections`): IN-29's
    /// stored graph and IN-71a's walk are not built (WP-215 seam 3, G3);
    /// decided from the option alone, on every update. [later] WP-238.
    Live,
    /// A winning common that appears, goes, becomes a definition (or a
    /// definition a common), or changes size or alignment (IN-72 item 11
    /// as ruled for v1, WP-216 R-2): commons stay in the commons chunk
    /// (layout.md L-88) in every mode and an update does not place one.
    /// Decided by the join (U-10) from `ResolveDelta::commons` against the
    /// stored `COMMONS` rows, before the layout is called; a winning
    /// mention that only moves to another file is no fall-back. [later]
    /// item 11's placing rule.
    Common,
    /// A `.eh_frame` range the update would move holds a CIE whose FDE
    /// pointer, LSDA or personality encoding is not `0x1b` (pcrel sdata4)
    /// or `0x9b` (indirect pcrel sdata4) (IN-34a as ruled by WP-239 S4-3):
    /// its fields cannot be adjusted from the previous output's bytes.
    /// Decided by the plan, which reads every range it will move, before
    /// W1 (nothing is written).
    Eh,
    /// A fact of a key's resolution that the scan reads (the one value
    /// `mortar_arch::SymFacts`, through `ScanTables::sym`) changes, and the
    /// key has a referrer in an unchanged file (`KEY_REFS`): that file's
    /// stored `KEY_REFS.demand` may be stale (IN-72 item 7 as ruled by
    /// WP-239 R-4). Decided by the join, comparing the value before and
    /// after.
    Slots,
}

impl Unimplemented {
    /// Every word, in `spec/incr.md` §7's order.
    pub const ALL: [Self; 15] = [
        Self::State5,
        Self::Resolve,
        Self::Refill,
        Self::Layout,
        Self::Symtab,
        Self::Image,
        Self::Write,
        Self::Meta,
        Self::Debug,
        Self::Diags,
        Self::Versioned,
        Self::Live,
        Self::Common,
        Self::Eh,
        Self::Slots,
    ];

    /// The word (`spec/incr.md` §7).
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::State5 => "state5",
            Self::Resolve => "resolve",
            Self::Refill => "refill",
            Self::Layout => "layout",
            Self::Symtab => "symtab",
            Self::Image => "image",
            Self::Write => "write",
            Self::Meta => "meta",
            Self::Debug => "debug",
            Self::Diags => "diags",
            Self::Versioned => "versioned",
            Self::Live => "live",
            Self::Common => "common",
            Self::Eh => "eh",
            Self::Slots => "slots",
        }
    }
}

/// Why an incremental-mode link is a full link (`spec/incr.md` §7): one
/// variant per row of §7's table, in its order, and nothing else (a write's
/// own failure is not a reason, IN-77e). `Display` prints the reason's name
/// alone ([`IncrReason::name`]); the detail is data ([`IncrReason::detail`])
/// and [`IncrReason::report`] gives the words the report line and
/// `incremental-update-failed` print (`name`, or `name: detail`, or
/// `update-not-implemented (word)`), byte for byte what the floor printed
/// before this type (WP-213).
///
/// Every variant but `UpdateNotImplemented` carries a detail string, empty
/// when there is nothing to add; a package that names a reason fills it
/// with what a user needs (the key, the section, the capacity), never with
/// text another package parses.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IncrReason {
    /// `no-state`: no state at the state path (the first link).
    NoState(String),
    /// `state-corrupt`: the state fails validation (IN-18, IN-18c).
    StateCorrupt(String),
    /// `state-version`: the state's format or mortar version differs.
    StateVersion(String),
    /// `options-changed`: the normalized plan differs (IN-18b), or a
    /// changed path that is no file of the link (a linker script).
    OptionsChanged(String),
    /// `output-changed`: the output is missing or not the one the state
    /// describes (IN-21, IN-77a), or W4 found another file at `P`.
    OutputChanged(String),
    /// `output-torn`: the state's "updating" flag is set (IN-46, IN-77).
    OutputTorn(String),
    /// `state-busy`: the state's path named another inode after the lock,
    /// 8 times in a row (IN-77b).
    StateBusy(String),
    /// `lock-unavailable`: `flock` is not enforced on the state's file
    /// system (IN-77b, image.md I-59).
    LockUnavailable(String),
    /// `small-output`: the output is below `mortar_incr::SMALL_OUTPUT`.
    SmallOutput(String),
    /// `update-not-implemented`, with the missing part's word, or none:
    /// the stage-A floor (`mortar::incremental`, every check passed and no
    /// update exists), which prints the bare name.
    UpdateNotImplemented(Option<Unimplemented>),
    /// `extraction-changed`: IN-71b's bound.
    ExtractionChanged(String),
    /// `closure-iterated`: the stored link's closure ran more than one
    /// iteration (`META.iterations`), or the full link of the update's
    /// inputs would (resolve.md §4.1; IN-71b "Iterations"; WP-215).
    ClosureIterated(String),
    /// `inputs-reordered`: two unchanged inputs' relative order changed.
    InputsReordered(String),
    /// `too-much-changed`: IN-16's threshold.
    TooMuchChanged(String),
    /// `reserve-exhausted`: a capacity of IN-32, IN-34 is exhausted.
    ReserveExhausted(String),
    /// `island-exhausted`: a thunk or patch does not fit its island
    /// (IN-72 item 9).
    IslandExhausted(String),
    /// `tls-layout-changed` (IN-34).
    TlsLayoutChanged(String),
    /// `copy-size-changed` (IN-34, IN-40).
    CopySizeChanged(String),
    /// `link-order-moved` (IN-72 item 4).
    LinkOrderMoved(String),
    /// `symbol-kind-changed` (IN-26, IN-71).
    SymbolKindChanged(String),
    /// `symtab-exhausted` (IN-49a).
    SymtabExhausted(String),
    /// `new-output-section` (IN-72 item 10).
    NewOutputSection(String),
    /// `property-changed` (IN-72 item 10).
    PropertyChanged(String),
    /// `dynamic-changed` (IN-42).
    DynamicChanged(String),
    /// `debug-compaction` (IN-57).
    DebugCompaction(String),
    /// `debug-name-index` (IN-55).
    DebugNameIndex(String),
    /// `debug-unit-kind` (IN-55a).
    DebugUnitKind(String),
    /// `debug-comdat-keeper`: the keeper of a debug section group (a type
    /// unit, a `.debug_macro` group) is a changed file whose new version
    /// drops the group, or the kept copy must move (WP-228 R-5).
    DebugComdatKeeper(String),
    /// `debug-growth`: a growth step in an output that has debug sections
    /// (WP-228 R-8; v1: the refill of every debug byte is a later package).
    DebugGrowth(String),
    /// `input-changed`: a file the update read is not the file it judged
    /// (IN-28a condition 2, IN-75).
    InputChanged(String),
    /// `state-collision`: the capture of this full link found two different
    /// fragments with one `FRAGS` key `(out, class, hash)` (IN-18e, WP-239
    /// R-2): no state is written and the previous one is removed; the link
    /// itself is not affected.
    StateCollision(String),
    /// `state-limit`: the capture of this full link has a value a stored
    /// column cannot hold (WP-239: a `.eh_frame` chunk past `u32` bytes,
    /// for `FILES`' eh columns): no state is written, as `state-collision`.
    StateLimit(String),
    /// `layout-changed`: the test path's only (WP-064a); never an update's.
    LayoutChanged(String),
    /// `resolution-changed`: the test path's only (WP-064a).
    ResolutionChanged(String),
}

impl IncrReason {
    /// Every reason's name, in `spec/incr.md` §7's order (the spec test
    /// compares it with the table).
    pub const NAMES: [&'static str; 34] = [
        "no-state",
        "state-corrupt",
        "state-version",
        "options-changed",
        "output-changed",
        "output-torn",
        "state-busy",
        "lock-unavailable",
        "small-output",
        "update-not-implemented",
        "extraction-changed",
        "closure-iterated",
        "inputs-reordered",
        "too-much-changed",
        "reserve-exhausted",
        "island-exhausted",
        "tls-layout-changed",
        "copy-size-changed",
        "link-order-moved",
        "symbol-kind-changed",
        "symtab-exhausted",
        "new-output-section",
        "property-changed",
        "dynamic-changed",
        "debug-compaction",
        "debug-name-index",
        "debug-unit-kind",
        "debug-comdat-keeper",
        "debug-growth",
        "input-changed",
        "state-collision",
        "state-limit",
        "layout-changed",
        "resolution-changed",
    ];

    /// The reason's name (`spec/incr.md` §7).
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::NoState(_) => "no-state",
            Self::StateCorrupt(_) => "state-corrupt",
            Self::StateVersion(_) => "state-version",
            Self::OptionsChanged(_) => "options-changed",
            Self::OutputChanged(_) => "output-changed",
            Self::OutputTorn(_) => "output-torn",
            Self::StateBusy(_) => "state-busy",
            Self::LockUnavailable(_) => "lock-unavailable",
            Self::SmallOutput(_) => "small-output",
            Self::UpdateNotImplemented(_) => "update-not-implemented",
            Self::ExtractionChanged(_) => "extraction-changed",
            Self::ClosureIterated(_) => "closure-iterated",
            Self::InputsReordered(_) => "inputs-reordered",
            Self::TooMuchChanged(_) => "too-much-changed",
            Self::ReserveExhausted(_) => "reserve-exhausted",
            Self::IslandExhausted(_) => "island-exhausted",
            Self::TlsLayoutChanged(_) => "tls-layout-changed",
            Self::CopySizeChanged(_) => "copy-size-changed",
            Self::LinkOrderMoved(_) => "link-order-moved",
            Self::SymbolKindChanged(_) => "symbol-kind-changed",
            Self::SymtabExhausted(_) => "symtab-exhausted",
            Self::NewOutputSection(_) => "new-output-section",
            Self::PropertyChanged(_) => "property-changed",
            Self::DynamicChanged(_) => "dynamic-changed",
            Self::DebugCompaction(_) => "debug-compaction",
            Self::DebugNameIndex(_) => "debug-name-index",
            Self::DebugUnitKind(_) => "debug-unit-kind",
            Self::DebugComdatKeeper(_) => "debug-comdat-keeper",
            Self::DebugGrowth(_) => "debug-growth",
            Self::InputChanged(_) => "input-changed",
            Self::StateCollision(_) => "state-collision",
            Self::StateLimit(_) => "state-limit",
            Self::LayoutChanged(_) => "layout-changed",
            Self::ResolutionChanged(_) => "resolution-changed",
        }
    }

    /// The detail: the string a variant carries, or the word of
    /// `update-not-implemented` (empty for the floor's bare name).
    #[must_use]
    pub fn detail(&self) -> &str {
        match self {
            Self::UpdateNotImplemented(w) => w.map_or("", Unimplemented::word),
            Self::NoState(d)
            | Self::StateCorrupt(d)
            | Self::StateVersion(d)
            | Self::OptionsChanged(d)
            | Self::OutputChanged(d)
            | Self::OutputTorn(d)
            | Self::StateBusy(d)
            | Self::LockUnavailable(d)
            | Self::SmallOutput(d)
            | Self::ExtractionChanged(d)
            | Self::ClosureIterated(d)
            | Self::InputsReordered(d)
            | Self::TooMuchChanged(d)
            | Self::ReserveExhausted(d)
            | Self::IslandExhausted(d)
            | Self::TlsLayoutChanged(d)
            | Self::CopySizeChanged(d)
            | Self::LinkOrderMoved(d)
            | Self::SymbolKindChanged(d)
            | Self::SymtabExhausted(d)
            | Self::NewOutputSection(d)
            | Self::PropertyChanged(d)
            | Self::DynamicChanged(d)
            | Self::DebugCompaction(d)
            | Self::DebugNameIndex(d)
            | Self::DebugUnitKind(d)
            | Self::DebugComdatKeeper(d)
            | Self::DebugGrowth(d)
            | Self::InputChanged(d)
            | Self::StateCollision(d)
            | Self::StateLimit(d)
            | Self::LayoutChanged(d)
            | Self::ResolutionChanged(d) => d,
        }
    }

    /// The reason named `name` with `detail` (`update-not-implemented`:
    /// `detail` is its word, or empty), or `None` for a name §7 does not
    /// have. For the places that name a reason by its `&'static str`
    /// today (`mortar_incr::delta`'s fall-backs).
    #[must_use]
    pub fn from_name(name: &str, detail: impl Into<String>) -> Option<Self> {
        let d = detail.into();
        Some(match name {
            "no-state" => Self::NoState(d),
            "state-corrupt" => Self::StateCorrupt(d),
            "state-version" => Self::StateVersion(d),
            "options-changed" => Self::OptionsChanged(d),
            "output-changed" => Self::OutputChanged(d),
            "output-torn" => Self::OutputTorn(d),
            "state-busy" => Self::StateBusy(d),
            "lock-unavailable" => Self::LockUnavailable(d),
            "small-output" => Self::SmallOutput(d),
            "update-not-implemented" => {
                if d.is_empty() {
                    Self::UpdateNotImplemented(None)
                } else {
                    let w = Unimplemented::ALL.into_iter().find(|w| w.word() == d)?;
                    Self::UpdateNotImplemented(Some(w))
                }
            }
            "extraction-changed" => Self::ExtractionChanged(d),
            "closure-iterated" => Self::ClosureIterated(d),
            "inputs-reordered" => Self::InputsReordered(d),
            "too-much-changed" => Self::TooMuchChanged(d),
            "reserve-exhausted" => Self::ReserveExhausted(d),
            "island-exhausted" => Self::IslandExhausted(d),
            "tls-layout-changed" => Self::TlsLayoutChanged(d),
            "copy-size-changed" => Self::CopySizeChanged(d),
            "link-order-moved" => Self::LinkOrderMoved(d),
            "symbol-kind-changed" => Self::SymbolKindChanged(d),
            "symtab-exhausted" => Self::SymtabExhausted(d),
            "new-output-section" => Self::NewOutputSection(d),
            "property-changed" => Self::PropertyChanged(d),
            "dynamic-changed" => Self::DynamicChanged(d),
            "debug-compaction" => Self::DebugCompaction(d),
            "debug-name-index" => Self::DebugNameIndex(d),
            "debug-unit-kind" => Self::DebugUnitKind(d),
            "debug-comdat-keeper" => Self::DebugComdatKeeper(d),
            "debug-growth" => Self::DebugGrowth(d),
            "input-changed" => Self::InputChanged(d),
            "state-collision" => Self::StateCollision(d),
            "state-limit" => Self::StateLimit(d),
            "layout-changed" => Self::LayoutChanged(d),
            "resolution-changed" => Self::ResolutionChanged(d),
            _ => return None,
        })
    }

    /// The words of the report line `incremental: full (<report>)` and of
    /// `incremental-update-failed`: the name alone when the detail is
    /// empty; `update-not-implemented (word)`; else `name: detail`.
    #[must_use]
    pub fn report(&self) -> String {
        match self {
            Self::UpdateNotImplemented(Some(w)) => format!("{} ({})", self.name(), w.word()),
            _ if self.detail().is_empty() => self.name().to_owned(),
            _ => format!("{}: {}", self.name(), self.detail()),
        }
    }
}

impl fmt::Display for IncrReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

// ---- the option-given keys (passed in, never stored) -------------------------

/// What the update reads of the plan (WP-215 D3-D5 and seam 3;
/// `spec/incr.md` IN-71, IN-71a, IN-71b, IN-71c): the keys a link's
/// options name, resolve.md §4.2's roots, `--wrap` (§7.1) and `--defsym`
/// (§7.2), **by name**, and the one switch `--gc-sections`. Never stored: an update has the full
/// link's options (else `options-changed`, IN-18b), so the join (U-10)
/// builds this from the plan and the scripts' directives exactly as the
/// full link's resolution reads them (`mortar_resolve::tables::options`
/// and its `defsyms`: the names of `Options::entry`, `undefined`,
/// `require_defined`, `wrap`, `defsym`), and U-3's entry points
/// (`extract`, `resolve`, `live`) take it by reference and map each name
/// to its key with `StateRows::find_key`. A name with no key is no root
/// (as `Options::roots`: a root whose key the link has no row for is
/// none). Written by the join; read by U-3.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OptionKeys {
    /// The entry root (§4.2): `-e SYM` or the last script `ENTRY(SYM)`,
    /// else `_start`; `None` for an output with no entry (`-shared`,
    /// `-r`). Its `arg` is 0 (the plan keeps `-e`'s value, not its index).
    pub entry: Option<OptionKey>,
    /// `-u SYM` in command-line order, then each script `EXTERN(SYM)`
    /// (its `arg` is `mortar_resolve::options::EXTERN_ARG` plus the
    /// directive's ordinal).
    pub undefined: Vec<OptionKey>,
    /// `--require-defined=SYM`, in command-line order.
    pub require_defined: Vec<OptionKey>,
    /// `--wrap=SYM`'s `SYM`, in the plan's order (`Plan::wraps`); several
    /// of one symbol are one (§7.1).
    pub wrap: Vec<Box<[u8]>>,
    /// `--defsym=SYM=EXPR`: the last option of each `SYM`, in
    /// command-line order of those (§7.2).
    pub defsym: Vec<OptionDefsym>,
    /// `--gc-sections` (`Plan::gc_sections`). Under it every update is
    /// `update-not-implemented (live)` (WP-215 seam 3; [later] WP-238).
    pub gc_sections: bool,
    /// `-init SYM` (`Plan::dynamic.init`), only when given: under
    /// `--gc-sections` its definition is a root (resolve.md §4.7;
    /// `mortar_resolve::Outcome::roots`); never a root of extraction.
    pub init: Option<OptionKey>,
    /// `-fini SYM` (`Plan::dynamic.fini`), as `init`.
    pub fini: Option<OptionKey>,
    /// `--gc-keep-exported` (`Plan::shaping.gc_keep_exported`). v1:
    /// `update-not-implemented (live)` (WP-238 R-5).
    pub gc_keep_exported: bool,
}

/// A key an option names (§4.2), by name, with the option's argument
/// index (`Named::arg`: the `seq` of a root's pulling reference, §4.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionKey {
    /// The option's operand as written. Its key is found as
    /// `mortar_resolve::tables::InputsView::key_of` finds it: split at
    /// the first `@`; `foo@V` is `(foo, V)`, `foo@@V` is `foo` looked up
    /// with no version, no `@` the name with no version. `foo@V` is reachable under D8
    /// only when a shared object defines `foo@V` (`META.versioned` counts
    /// relocatable objects and members only).
    pub name: Box<[u8]>,
    pub arg: u32,
}

/// One `--defsym` (§7.2): the key it defines and its expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionDefsym {
    /// The defined key's name.
    pub name: Box<[u8]>,
    pub value: DefsymOperand,
    /// The option's argument index (the root's `arg` of its operand).
    pub arg: u32,
}

/// A `--defsym` expression by name (`mortar_resolve::options::DefsymValue`
/// with names for keys).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DefsymOperand {
    /// An absolute number.
    Number(u64),
    /// An alias of the named key.
    Alias(Box<[u8]>),
    /// The named key's address plus the addend.
    Offset(Box<[u8]>, i64),
}

impl DefsymOperand {
    /// The key the expression names, if any (a root, §7.2).
    #[must_use]
    pub fn base(&self) -> Option<&[u8]> {
        match self {
            Self::Number(_) => None,
            Self::Alias(b) | Self::Offset(b, _) => Some(b),
        }
    }
}

/// One stored liveness row (IN-29; WP-238 S1) in the link's id space,
/// made by `mortar_live::stored_edges`; the capture and the join turn it
/// into an `EdgeRow`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LiveEdge {
    /// `mortar_incr::update::rows::edge_kind`.
    pub kind: u8,
    pub source: SectionId,
    pub target: LiveTarget,
    /// As `EdgeRow::offset` (F8 applied).
    pub offset: u32,
}

/// A [`LiveEdge`]'s target, by `edge_kind::target`'s class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LiveTarget {
    Key(SymbolId),
    Section(SectionId),
    /// `ROOT`, `NAMED`, `SPLIT`.
    None,
    /// `REFUSE`'s code.
    Code(u32),
}

impl OptionKeys {
    /// The root names with their `arg`, in `mortar_resolve::Options::roots`'
    /// order: the entry, `-u` and `EXTERN`, `--require-defined`, then each
    /// `--defsym` operand's base.
    #[must_use]
    pub fn roots(&self) -> Vec<(&[u8], u32)> {
        let mut out: Vec<(&[u8], u32)> = Vec::new();
        out.extend(self.entry.iter().map(|k| (&*k.name, k.arg)));
        for k in self.undefined.iter().chain(&self.require_defined) {
            out.push((&k.name, k.arg));
        }
        for d in &self.defsym {
            if let Some(b) = d.value.base() {
                out.push((b, d.arg));
            }
        }
        out
    }

    /// The keys the liveness walk roots (WP-238 F1): [`Self::roots`]
    /// then `-init` and `-fini`, the order of
    /// `mortar_resolve::Outcome::roots`. Not `roots()`: `-init`/`-fini`
    /// are never roots of extraction (§4.2).
    #[must_use]
    pub fn live_roots(&self) -> Vec<(&[u8], u32)> {
        let mut out = self.roots();
        out.extend([&self.init, &self.fini].into_iter().flatten().map(|k| (&*k.name, k.arg)));
        out
    }

    /// The `--defsym` keys' names (the keys IN-71c makes affected).
    pub fn defsym_names(&self) -> impl Iterator<Item = &[u8]> {
        self.defsym.iter().map(|d| &*d.name)
    }

    /// The name an **undefined** mention of the unversioned key `name`
    /// binds to (§7.1; `mortar_resolve::Options::redirect` by name): for
    /// each `--wrap=S` in order, `S` binds to `__wrap_S` and `__real_S`
    /// to `S`, the last such option deciding; `None` when no option names
    /// it (the mention binds to its own key). A versioned key is never
    /// redirected (`--wrap` names the unversioned key).
    #[must_use]
    pub fn redirect(&self, name: &[u8]) -> Option<Vec<u8>> {
        let mut to = None;
        for s in &self.wrap {
            if name == &**s {
                to = Some([b"__wrap_".as_slice(), s].concat());
            } else if name.strip_prefix(b"__real_".as_slice()) == Some(&**s) {
                to = Some(s.to_vec());
            }
        }
        to
    }

    /// Whether any `--wrap` option names `name` as `S`, `__wrap_S` or
    /// `__real_S` (a key whose binding a `--wrap` decides).
    #[must_use]
    pub fn wrapped(&self, name: &[u8]) -> bool {
        self.wrap.iter().any(|s| {
            name == &**s
                || name.strip_prefix(b"__wrap_".as_slice()) == Some(&**s)
                || name.strip_prefix(b"__real_".as_slice()) == Some(&**s)
        })
    }
}

// ---- the stored layout (what U-4 and U-5 read) ------------------------------

/// A section row as the delta layout reads it (`SECTIONS`, IN-18e;
/// IN-72): its identity (`spec/model.md` §7.1), header, and place. Read by
/// U-4 (keep/place phases), U-5 (its file's symbols' sections); written by
/// U-2's reader from the row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSection {
    /// The `SECTIONS` row.
    pub id: SectionId,
    /// The `FILES` row of its file.
    pub file: FileId,
    /// Its index in its file's section header table.
    pub index: u32,
    /// The anchor's kind (`format::SectionRow::anchor_kind`: 0 none, 1
    /// COMDAT signature, 2 global, 3 local, as `capture.rs` writes it).
    pub anchor_kind: u8,
    /// The anchor's bytes.
    pub anchor: Vec<u8>,
    /// The section's name.
    pub name: Vec<u8>,
    /// Among the file's sections with the same anchor and name.
    pub ordinal: u32,
    pub sh_type: u32,
    pub sh_flags: u64,
    /// Bytes.
    pub size: u64,
    /// Bytes (a power of two; 1 for none).
    pub align: u64,
    /// Where it is: `(chunk, offset from the chunk's start)`, or
    /// [`Place::NONE`] when the link placed it nowhere (dead, discarded,
    /// merged: a merged section's pieces are `FRAGS`).
    pub place: Place,
}

/// A region (IN-30, IN-72 item 2): a run of one file's sections in one
/// `(O, c)` of a hole-tolerant output section, with its slack. A file has
/// one per `(O, c)` from its first link and one more per overflow, kept
/// until the compaction. Key: `(file, out, category, ordinal)`. Written by
/// the first link's capture and by U-4 (`LayoutDelta::regions_opened`);
/// read by U-4 (phase 2 (ii)), U-6 (the map's `region` line, WP-211
/// §R.12), H-1 (`P_U`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StoredRegion {
    pub file: FileId,
    pub out: OutSecId,
    /// The `.text` category (layout.md L-53); 0 when not code.
    pub category: u8,
    /// 0 for the first link's region, then the overflow regions in the
    /// order they were opened (IN-72 item 2 fills them in this order).
    pub ordinal: u32,
    /// The body chunk it is in.
    pub chunk: ChunkId,
    /// Bytes from the chunk's start.
    pub offset: u64,
    /// Bytes: its sections, their padding and its slack (IN-33).
    pub capacity: u64,
}

/// A merged-section fragment (`FRAGS`, IN-72 item 6): identity, offset,
/// reference count. Key: `(out, class, hash)` (IN-18e as ruled by WP-239
/// S4-1: key-keyed, so a fragment is found by one binary search,
/// [`LayoutRows::frags_of`]). Only fragments of split sections whose name
/// does not start with `.debug_` have rows (WP-239 R-1). Two different
/// fragments with one key refuse the state at the capture
/// (`state-collision`), never the link. Read and written by U-4; read by
/// U-6 (a fragment whose count rises from 0 is rewritten).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StoredFrag {
    pub out: OutSecId,
    /// The merge class word ([`MergeClass::word`]).
    pub class: u32,
    /// L128 of the content.
    pub hash: [u8; 16],
    /// Bytes from the merged chunk's start.
    pub offset: u64,
    /// Its bytes (WP-242 C, seam 6): a function of its identity, so never
    /// changed for a key; a dead string fragment's hole is this long.
    pub size: u32,
    /// The live files with a `FRAG_USE` row naming it (IN-18e
    /// `FRAGS.refs`, counted per FILE); 0: a dead range.
    pub refs: u32,
}

/// One `FRAG_USE` row of a file (IN-18e, kind 56; file-keyed, 28 bytes;
/// WP-239 S4-2): a fragment the file's live split pieces name, once per
/// fragment, by the fragment's key (never a `FRAGS` row number). No row for
/// a split section named `.debug_*` (R-1). Written by the capture and U-2
/// (a changed file's range from its `MergedPiece`s); read by U-4 (item 6:
/// the old version's fragments, each decrementing `FRAGS.refs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StoredFragUse {
    pub file: FileId,
    pub out: OutSecId,
    /// The merge class word ([`MergeClass::word`]).
    pub class: u32,
    pub hash: [u8; 16],
}

/// A file's `.eh_frame` range in the stored output (IN-34a as ruled by
/// WP-239 S4-3): four `u32` columns of its `FILES` row (`fdes`: WP-214
/// seam 6, from WP-243). Record sizes are not stored: a changed file's come
/// from its new bytes (S-47), an unchanged range is moved by walking the
/// previous output's bytes. Written by the capture (the incremental-mode
/// full link gives each file's records a capacity of size + max(16,
/// ceil4(size/8))) and U-2 (from `LayoutDelta::eh`, with `fdes` counted
/// by the join from the file's [`EhFile::records`]: the records that are
/// not `cie`); read by U-4 (item 5), U-6 (the move).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StoredEhRange {
    /// Bytes from the `.eh_frame` chunk's start.
    pub offset: u32,
    /// Bytes: the records and the pad.
    pub capacity: u32,
    /// Kept records in the range.
    pub records: u32,
    /// The FDEs among `records` (WP-243, seam 6): written wherever
    /// `records` is. With it U-4 sizes `.eh_frame_hdr` without reading
    /// unchanged ranges: the stored chunk's FDE count, minus the changed
    /// files' stored `fdes`, plus their new ones (IN-34a).
    pub fdes: u32,
}

/// One `KEY_REFS` row as the delta layout reads it (IN-18e; WP-239 S4-4):
/// the file mentions `key`, and `demand` is the OR of the `DemandFlags`
/// the scan raised on `key` over the file's live units (counted per FILE;
/// empty for a mention no relocation uses). `INDEX` is never the demand.
/// Written by the capture and U-2 (from the scan's per-file demand, S4-5);
/// read by U-4 (item 7: what the old version demanded).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StoredKeyRef {
    pub key: SymbolId,
    pub file: FileId,
    pub demand: DemandFlags,
}

/// A GOT, PLT, IPLT or TLS slot (`SLOTS`, IN-28b, IN-72 item 7): the
/// `index`-th slot of its kind's list. Read and written by U-4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StoredSlot {
    pub kind: SlotKind,
    /// The key (a global's `SYMBOLS` row, or a local by `(FILES row,
    /// symbol-table index)`: IN-28b's section identity is the caller's).
    pub key: SymKey,
    /// Its position in its kind's list.
    pub index: u32,
    /// The live files whose `KEY_REFS.demand` needs this slot (WP-239
    /// S4-4; counted per FILE); a slot whose count falls to 0 is retired.
    /// A local's slot (IN-28b) is its file's alone: 1.
    pub refs: u32,
}

/// One file's `.symtab` block and `.strtab` region (`SYMTAB` table 43,
/// IN-49a), or (for file 0's row) the global area. Entries and bytes;
/// read and written by U-5; U-2 stores it; the map's `symblock` line
/// (WP-211 §R.12, U-6).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct SymtabBlock {
    /// The block's first `.symtab` index.
    pub first: u32,
    /// Its entries (the file's locals, S-74), fillers not counted.
    pub count: u32,
    /// Entries it can hold: `count + max(2, count/8)` when written.
    pub capacity: u32,
    /// Its `.strtab` region: offset in `.strtab`, bytes used, bytes held.
    pub strtab_offset: u64,
    pub strtab_len: u64,
    pub strtab_capacity: u64,
}

/// The sizes of `.symtab` and `.strtab` and their file slack (IN-49a):
/// read by U-5.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct SymtabTables {
    /// `sh_info`: entries before the global area, fillers included.
    pub sh_info: u32,
    /// The first index of the local tail reserve and its entries.
    pub tail_first: u32,
    pub tail_capacity: u32,
    /// `.symtab`'s `sh_size` and the bytes after it in no section.
    pub symtab_size: u64,
    pub symtab_slack: u64,
    /// `.strtab`'s `sh_size` and its slack.
    pub strtab_size: u64,
    pub strtab_slack: u64,
}

/// What a [`LayoutRows`] accessor its implementation does not give yet
/// returns (WP-216 seam 4): a named plain link, never a guess.
fn not_given() -> IncrReason {
    IncrReason::UpdateNotImplemented(Some(Unimplemented::Layout))
}

/// The read side of a stored layout (WP-211 §C `StoredLayout`): the
/// `Placement` rows an update reads, each by the key the update has, never
/// a whole read of a table that is not "whole" in IN-18e (IN-18c: an update
/// reads only the blocks it touches). Every call can fail with
/// `state-corrupt` (a block that fails its hash, IN-18c). Implemented by U-2
/// over a `StateView` (`mortar_incr::state5::read::LayoutOf`), by
/// [`MemoryLayout`] for tests and for H-1's `P_U` (read from a map).
///
/// Cost classes: *whole* reads a whole small table (IN-18e class "whole",
/// under 2 MB together on g10x); *range* reads one file's range or one key's
/// rows; nothing here is O(state).
pub trait LayoutRows: Sync {
    /// `CHUNKS` (whole): every chunk, by id.
    ///
    /// # Errors
    /// `state-corrupt`.
    fn chunks(&self) -> Result<Vec<(ChunkId, Chunk)>, IncrReason>;
    /// `ORDER` (whole): every chunk once, in file order.
    ///
    /// # Errors
    /// `state-corrupt`.
    fn order(&self) -> Result<Vec<ChunkId>, IncrReason>;
    /// `SEGMENTS` (whole).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn segments(&self) -> Result<Vec<Segment>, IncrReason>;
    /// `OUTSECS` (whole): each output section's name.
    ///
    /// # Errors
    /// `state-corrupt`.
    fn outsecs(&self) -> Result<Vec<(OutSecId, Vec<u8>)>, IncrReason>;
    /// A file's `SECTIONS` range (range, by `FILE_INDEX`).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn sections_of(&self, file: FileId) -> Result<Vec<StoredSection>, IncrReason>;
    /// A file's regions, every `(O, c)`, in `(out, category, ordinal)`
    /// order (range). See WP-213's finding on where they are stored.
    ///
    /// # Errors
    /// `state-corrupt`.
    fn regions_of(&self, file: FileId) -> Result<Vec<StoredRegion>, IncrReason>;
    /// A chunk's holes, by offset (key-keyed `HOLES`, two binary searches).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn holes_in(&self, chunk: ChunkId) -> Result<Vec<Hole>, IncrReason>;
    /// The `FRAGS` row of one key `(out, class, hash)`, or `None` (key-keyed:
    /// one binary search). Never a whole read of an output section's
    /// fragments.
    ///
    /// # Errors
    /// `state-corrupt`; `update-not-implemented (layout)` until the
    /// implementation gives it.
    fn frags_of(
        &self,
        out: OutSecId,
        class: u32,
        hash: [u8; 16],
    ) -> Result<Option<StoredFrag>, IncrReason> {
        let _ = (out, class, hash);
        Err(not_given())
    }
    /// A file's `FRAG_USE` rows (range), by key.
    ///
    /// # Errors
    /// `state-corrupt`; `update-not-implemented (layout)` until given.
    fn frag_uses_of(&self, file: FileId) -> Result<Vec<StoredFragUse>, IncrReason> {
        let _ = file;
        Err(not_given())
    }
    /// A file's `.eh_frame` range (its `FILES` row), or `None` when it has
    /// no kept record.
    ///
    /// # Errors
    /// `state-corrupt`; `update-not-implemented (layout)` until given.
    fn eh_range_of(&self, file: FileId) -> Result<Option<StoredEhRange>, IncrReason> {
        let _ = file;
        Err(not_given())
    }
    /// Every file that has a `.eh_frame` range (`records` > 0; not `GONE`),
    /// with its CURRENT `FILES.rank` and its range, in `rank` order (WP-243,
    /// seam 6). Called only by the whole rewrite of IN-34a (overflow, an
    /// appearing file): it reads `FILES` whole, O(files), which IN-18c
    /// names; the rewrite it serves is O(records) anyway.
    ///
    /// # Errors
    /// `state-corrupt`; `update-not-implemented (layout)` until given.
    fn eh_ranges(&self) -> Result<Vec<(FileId, u64, StoredEhRange)>, IncrReason> {
        Err(not_given())
    }
    /// A file's `KEY_REFS` rows, by key: the keys of its `MENTIONS` range,
    /// each row found by `(key, file)` (one binary search a key; no
    /// global keys of other files).
    ///
    /// # Errors
    /// `state-corrupt`; `update-not-implemented (layout)` until given.
    fn key_refs_of(&self, file: FileId) -> Result<Vec<StoredKeyRef>, IncrReason> {
        let _ = file;
        Err(not_given())
    }
    /// One `SECTIONS` row by id (the file's range holds it, IN-18e;
    /// `FILE_INDEX` is not needed: the row is addressed by id), or `None`
    /// for an id at or past the table's row count (new to the update).
    /// [`LayoutDelta::place_of`] reads it.
    ///
    /// # Errors
    /// `state-corrupt`; `update-not-implemented (layout)` until given.
    fn section(&self, id: SectionId) -> Result<Option<StoredSection>, IncrReason> {
        let _ = id;
        Err(not_given())
    }
    /// `SLOTS` (whole).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn slots(&self) -> Result<Vec<StoredSlot>, IncrReason>;
    /// `PATCHES` (whole): thunks and erratum patches; an island is a
    /// `ChunkKind::Patches` chunk with its capacity.
    ///
    /// # Errors
    /// `state-corrupt`.
    fn patches(&self) -> Result<Vec<Patch>, IncrReason>;
    /// `REDIRECTS` (whole).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn redirects(&self) -> Result<Vec<Redirect>, IncrReason>;
    /// `SYM_PLACE` (whole): commons (in the commons chunk in every mode,
    /// IN-72 item 11 as ruled for v1), copies.
    ///
    /// # Errors
    /// `state-corrupt`.
    fn sym_place(&self) -> Result<Vec<SymPlace>, IncrReason>;
    /// `DEFINED` (whole).
    ///
    /// # Errors
    /// `state-corrupt`.
    fn defined(&self) -> Result<Vec<Defined>, IncrReason>;
    /// `LAYOUT` (whole): the TLS template and the entry.
    ///
    /// # Errors
    /// `state-corrupt`.
    fn tls_and_entry(&self) -> Result<(Option<Tls>, Option<u64>), IncrReason>;
    /// A file's `.symtab` block (`SYMTAB`, range); `None` when it has none.
    ///
    /// # Errors
    /// `state-corrupt`.
    fn symtab_block(&self, file: FileId) -> Result<Option<SymtabBlock>, IncrReason>;
    /// The global area (file 0's `SYMTAB` row) and the tables' sizes.
    ///
    /// # Errors
    /// `state-corrupt`.
    fn symtab_tables(&self) -> Result<(SymtabBlock, SymtabTables), IncrReason>;
}

/// A stored layout (WP-211 §C): a borrowed [`LayoutRows`]. U-4's and U-5's
/// entry points take it; the rows are read through `Deref`.
#[derive(Clone, Copy)]
pub struct StoredLayout<'a> {
    rows: &'a dyn LayoutRows,
}

impl<'a> StoredLayout<'a> {
    /// The layout `rows` give.
    #[must_use]
    pub fn new(rows: &'a dyn LayoutRows) -> Self {
        Self { rows }
    }
}

impl<'a> std::ops::Deref for StoredLayout<'a> {
    type Target = dyn LayoutRows + 'a;
    fn deref(&self) -> &Self::Target {
        self.rows
    }
}

impl fmt::Debug for StoredLayout<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoredLayout").finish_non_exhaustive()
    }
}

/// [`LayoutRows`] in memory: hand-made rows for the packages' tests, and
/// H-1's `P_U` read from an update's map (IN-78). Plain public fields;
/// every accessor filters or copies them (no hashing, never fails; every
/// accessor is given; [`LayoutRows::section`] finds a row of `sections` by
/// its `id`). No link builds one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryLayout {
    pub chunks: Vec<Chunk>,
    pub order: Vec<ChunkId>,
    pub segments: Vec<Segment>,
    pub outsecs: Vec<Vec<u8>>,
    pub sections: Vec<StoredSection>,
    pub regions: Vec<StoredRegion>,
    pub holes: Vec<Hole>,
    pub frags: Vec<StoredFrag>,
    pub frag_uses: Vec<StoredFragUse>,
    /// `(file, range)`.
    pub eh_ranges: Vec<(FileId, StoredEhRange)>,
    /// `(file, FILES.rank)` for [`LayoutRows::eh_ranges`]; a file not
    /// listed has the first link's rank, `FileId << 20`.
    pub ranks: Vec<(FileId, u64)>,
    pub key_refs: Vec<StoredKeyRef>,
    pub slots: Vec<StoredSlot>,
    pub patches: Vec<Patch>,
    pub redirects: Vec<Redirect>,
    pub sym_place: Vec<SymPlace>,
    pub defined: Vec<Defined>,
    pub tls: Option<Tls>,
    pub entry: Option<u64>,
    /// `(file, block)`.
    pub symtab_blocks: Vec<(FileId, SymtabBlock)>,
    pub global_area: SymtabBlock,
    pub symtab_tables: SymtabTables,
}

impl LayoutRows for MemoryLayout {
    fn chunks(&self) -> Result<Vec<(ChunkId, Chunk)>, IncrReason> {
        Ok(self.chunks.iter().enumerate().map(|(i, c)| (ChunkId(i as u32), *c)).collect())
    }
    fn order(&self) -> Result<Vec<ChunkId>, IncrReason> {
        Ok(self.order.clone())
    }
    fn segments(&self) -> Result<Vec<Segment>, IncrReason> {
        Ok(self.segments.clone())
    }
    fn outsecs(&self) -> Result<Vec<(OutSecId, Vec<u8>)>, IncrReason> {
        Ok(self.outsecs.iter().enumerate().map(|(i, n)| (OutSecId(i as u32), n.clone())).collect())
    }
    fn sections_of(&self, file: FileId) -> Result<Vec<StoredSection>, IncrReason> {
        Ok(self.sections.iter().filter(|s| s.file == file).cloned().collect())
    }
    fn regions_of(&self, file: FileId) -> Result<Vec<StoredRegion>, IncrReason> {
        let mut v: Vec<StoredRegion> =
            self.regions.iter().filter(|r| r.file == file).copied().collect();
        v.sort_by_key(|r| (r.out, r.category, r.ordinal));
        Ok(v)
    }
    fn holes_in(&self, chunk: ChunkId) -> Result<Vec<Hole>, IncrReason> {
        let mut v: Vec<Hole> = self.holes.iter().filter(|h| h.chunk == chunk).copied().collect();
        v.sort_by_key(|h| h.offset);
        Ok(v)
    }
    fn frags_of(
        &self,
        out: OutSecId,
        class: u32,
        hash: [u8; 16],
    ) -> Result<Option<StoredFrag>, IncrReason> {
        Ok(self.frags.iter().find(|f| (f.out, f.class, f.hash) == (out, class, hash)).copied())
    }
    fn frag_uses_of(&self, file: FileId) -> Result<Vec<StoredFragUse>, IncrReason> {
        let mut v: Vec<StoredFragUse> =
            self.frag_uses.iter().filter(|r| r.file == file).copied().collect();
        v.sort();
        Ok(v)
    }
    fn eh_range_of(&self, file: FileId) -> Result<Option<StoredEhRange>, IncrReason> {
        Ok(self.eh_ranges.iter().find(|(f, _)| *f == file).map(|(_, r)| *r))
    }
    fn eh_ranges(&self) -> Result<Vec<(FileId, u64, StoredEhRange)>, IncrReason> {
        let rank = |f: FileId| {
            self.ranks.iter().find(|(g, _)| *g == f).map_or(u64::from(f.0) << 20, |(_, r)| *r)
        };
        let mut v: Vec<(FileId, u64, StoredEhRange)> = self
            .eh_ranges
            .iter()
            .filter(|(_, r)| r.records > 0)
            .map(|(f, r)| (*f, rank(*f), *r))
            .collect();
        v.sort_by_key(|e| (e.1, e.0));
        Ok(v)
    }
    fn key_refs_of(&self, file: FileId) -> Result<Vec<StoredKeyRef>, IncrReason> {
        let mut v: Vec<StoredKeyRef> =
            self.key_refs.iter().filter(|r| r.file == file).copied().collect();
        v.sort();
        Ok(v)
    }
    fn section(&self, id: SectionId) -> Result<Option<StoredSection>, IncrReason> {
        Ok(self.sections.iter().find(|s| s.id == id).cloned())
    }
    fn slots(&self) -> Result<Vec<StoredSlot>, IncrReason> {
        Ok(self.slots.clone())
    }
    fn patches(&self) -> Result<Vec<Patch>, IncrReason> {
        Ok(self.patches.clone())
    }
    fn redirects(&self) -> Result<Vec<Redirect>, IncrReason> {
        Ok(self.redirects.clone())
    }
    fn sym_place(&self) -> Result<Vec<SymPlace>, IncrReason> {
        Ok(self.sym_place.clone())
    }
    fn defined(&self) -> Result<Vec<Defined>, IncrReason> {
        Ok(self.defined.clone())
    }
    fn tls_and_entry(&self) -> Result<(Option<Tls>, Option<u64>), IncrReason> {
        Ok((self.tls, self.entry))
    }
    fn symtab_block(&self, file: FileId) -> Result<Option<SymtabBlock>, IncrReason> {
        Ok(self.symtab_blocks.iter().find(|(f, _)| *f == file).map(|(_, b)| *b))
    }
    fn symtab_tables(&self) -> Result<(SymtabBlock, SymtabTables), IncrReason> {
        Ok((self.global_area, self.symtab_tables))
    }
}

// ---- what U-4 takes and gives -----------------------------------------------

/// One section of a changed or drawn-in file as the update's front gives
/// it (IN-72 item 2, 2a), the input of the delta layout (U-4). Written by
/// the join (U-10) from the parsed file and U-3's liveness and election;
/// read by U-4. Key: `id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionChange<'a> {
    /// Its new id (the module text's id space).
    pub id: SectionId,
    /// Its file's `FILES` row (a new id for an appeared file).
    pub file: FileId,
    /// Its index in the file's section header table.
    pub index: u32,
    /// Its identity (`spec/model.md` §7.1): anchor and name, then
    /// `ordinal` among the file's sections with the same key.
    pub key: crate::identity::SectionKey<'a>,
    pub ordinal: u32,
    /// The `SECTIONS` row of its previous version matched by that
    /// identity, or `None` (new to the file, or its file appeared).
    pub old: Option<SectionId>,
    pub sh_type: u32,
    pub sh_flags: u64,
    /// Bytes.
    pub size: u64,
    /// Bytes (a power of two).
    pub align: u64,
    /// Live after IN-71a (always `true` without `--gc-sections`).
    pub live: bool,
    /// Kept by IN-71's COMDAT election (`true` outside a group).
    pub kept: bool,
}

/// The merge class of a piece (IN-72 item 6): what, with the output
/// section and the bytes, makes a fragment's identity in the full link
/// (`mortar_merge::merge::MergeMeta` less `out`; S-6..S-10). Two pieces
/// share a fragment only when out, class and bytes are equal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MergeClass {
    /// `SHF_STRINGS`.
    pub strings: bool,
    /// `sh_entsize`.
    pub entsize: u32,
    /// log2 of the alignment.
    pub align: u8,
}

impl MergeClass {
    /// The class word `FRAGS` and `FRAG_USE` store (IN-18e, WP-239 S4-1):
    /// `strings << 31 | align << 24 | entsize` (`entsize` is at most 2^16,
    /// S-6; `align` below 64).
    #[must_use]
    pub const fn word(self) -> u32 {
        ((self.strings as u32) << 31) | ((self.align as u32 & 0x7f) << 24) | self.entsize
    }
}

/// One merged piece of a changed or drawn-in file (IN-72 item 6) as the
/// full link's split gives it (`mortar_merge::cut::placed_whole`, S-6, and
/// `mortar_merge::cut::cut`, S-7..S-10; the hash as the capture's
/// `frag_hash`, L128 by `mortar_digest::l128`). Written by the join (U-10)
/// from the parsed file; read by U-4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MergedPiece {
    /// The input section (the update's id space).
    pub section: SectionId,
    /// Bytes from the input section's start.
    pub offset: u64,
    /// Bytes.
    pub size: u64,
    /// The output section the section is assigned to.
    pub out: OutSecId,
    pub class: MergeClass,
    /// L128 of the piece's bytes.
    pub hash: [u8; 16],
}

/// A changed or drawn-in file's merged pieces (IN-72 item 6), every piece
/// of every live, kept `SHF_MERGE` section, in `(section, offset)` order.
/// A gone file, or a changed file whose new version has none, is given
/// with no pieces (its old version's are its `FRAG_USE` rows).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilePieces {
    pub file: FileId,
    pub pieces: Vec<MergedPiece>,
}

/// One kept `.eh_frame` record of a changed or drawn-in file (IN-34a), as
/// the full link's selection gives it: `mortar_merge::eh::parse` (the
/// records), `mortar_synth::eh::EhPlan::select_in` with `per_file` true
/// (S-46..S-49: the file's own CIE deduplication, which is the full
/// link's restricted to the file in incremental mode), the size
/// `mortar_merge::eh::Record::out_size` (S-47).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EhRecordIn {
    /// The input `.eh_frame` section (the update's id space).
    pub section: SectionId,
    /// The piece's index among that section's `.eh_frame` pieces (the
    /// `EH_RECORDS.piece` the patch stores).
    pub piece: u32,
    /// Its output bytes (S-47).
    pub size: u64,
    /// A CIE (else an FDE).
    pub cie: bool,
}

/// A changed, drawn-in or gone file's `.eh_frame` records (IN-34a): the
/// kept records in S-46 order (empty for a gone file or a file whose new
/// version has none; the old version's range is [`StoredEhRange`]), and
/// the file's CURRENT rank (S-46's order is the
/// load order: the stored `FILES.rank` composed with the update's ranks
/// as `mortar_incr::update::rows::keeper`'s caller composes it; never the
/// `FILES` row number).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EhFile {
    pub file: FileId,
    pub rank: u64,
    pub records: Vec<EhRecordIn>,
}

/// One symbol a changed or drawn-in file's live relocations demand a slot
/// for (IN-72 item 7; IN-28b): the key (a global's `SYMBOLS` row in the
/// update's id space, or a local by `(file, index)`) and its flags (the
/// slot ones: GOT, PLT, IPLT, TLS; the layout ignores the others).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SlotDemand {
    pub key: SymKey,
    pub flags: DemandFlags,
}

/// A changed, drawn-in or gone file's slot demand (IN-72 item 7), sorted
/// by key, one row per key (the flags or-ed over the file's live units:
/// counted per FILE). One producer (WP-239 S4-5): `run_unit` generic over
/// its global sink, the plain link's sink today's atomic OR, the
/// incremental-mode full link's and the join's a recording one whose
/// per-file OR is this list and `KEY_REFS.demand`. The scan reads a
/// target's resolution only through `mortar_arch::SymFacts`
/// (`ScanTables::sym`): the join compares that value before and after for
/// the keys unchanged files reference (`update-not-implemented (slots)`).
/// A gone file is given with none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDemands {
    pub file: FileId,
    pub demands: Vec<SlotDemand>,
}

/// What the delta layout takes besides the stored layout (WP-216 seam 4):
/// the sections (items 1-4, 8-10), and per file the `.eh_frame` records
/// (item 5), merged pieces (item 6) and slot demand (item 7). Every list is
/// sorted by its key (`changes` by id, the others by file) and names only
/// files the update changed, drew in or lost.
#[derive(Debug, Clone, Copy, Default)]
pub struct LayoutInput<'a> {
    pub changes: &'a [SectionChange<'a>],
    pub eh: &'a [EhFile],
    pub pieces: &'a [FilePieces],
    pub demands: &'a [FileDemands],
}

/// A growth step (IN-72 item 3): an output section grown into its
/// segment's reserve, and what moved. Written by U-4; read by U-6 (both
/// header tables rewritten, every moved byte dirty), U-3 (the moved
/// sections' referrers), U-5 (remapped `.symtab` values).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GrowthStep {
    /// The output section that grew.
    pub out: OutSecId,
    /// Bytes added to its tail; every later output section of its
    /// `PT_LOAD` moves by this much (in address, and in file offset unless
    /// `NOBITS`).
    pub step: u64,
}

/// One file's `.eh_frame` range in the update's output (IN-34a): where
/// its records start, the bytes it holds, and each record's offset (bytes
/// from the `.eh_frame` chunk's start, in the order of the file's
/// [`EhFile::records`]). Written by U-4; read by U-6 (the bytes, the
/// padding CIE, `.eh_frame_hdr`), U-2 (the patch).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EhRange {
    pub file: FileId,
    /// Bytes from the chunk's start.
    pub offset: u64,
    /// Bytes: its records and its pad. The pad is 0; or a hole of at
    /// least 16 bytes in [`LayoutDelta::holes`], `HoleKind::EhPad` (a
    /// padding CIE in the bytes); or 4, 8 or 12 bytes, which in the
    /// placement DATA are a short `HoleKind::EhPad` entry (never at offset
    /// 0), as the full link's `eh_pads` reads them, and in the BYTES and
    /// the map are the last record's padding (`DW_CFA_nop`), never a
    /// `hole` line or a padding CIE (IN-34a; WP-243, seam 6).
    pub capacity: u64,
    pub records: Vec<u64>,
}

/// The `.eh_frame` the update writes (IN-72 item 5): the ranges it
/// rewrites, or (`whole`) the section rewritten whole with every range.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EhLayout {
    /// The `.eh_frame` chunk.
    pub chunk: ChunkId,
    /// Every file's range is in `ranges` (else only the rewritten ones).
    pub whole: bool,
    /// Sorted by offset.
    pub ranges: Vec<EhRange>,
}

/// Where a section is after an update ([`LayoutDelta::place_of`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlaceNow {
    /// Placed by this update (a section of a file of
    /// [`LayoutDelta::files`]).
    Here(Place),
    /// A section of a file of [`LayoutDelta::files`] the update does not
    /// place (dead, not kept, merged, an old version's row, a gone file's).
    NotPlaced,
    /// A section of no file of [`LayoutDelta::files`]: its stored place
    /// (which may be [`Place::NONE`]: the stored link placed it nowhere).
    Stored(Place),
}

/// The delta layout's result (WP-211 §C `LayoutDelta`; IN-72): the update's
/// own `Placement` (`P_U`, IN-78) and what changed against the stored one.
/// Written by U-4; read by U-3 (`recompute`, `refill_set`), U-5, U-6, U-2
/// (the patch's decision rows), the join.
///
/// The placement is SPARSE (WP-216 R-3): a section's place is read through
/// [`LayoutDelta::place_of`] and a chunk's holes through
/// [`LayoutDelta::holes_of`], never by indexing `placement.place` or
/// `placement.holes`. An ADDRESS is always the chunk's address in
/// `placement.chunks` (moved by the growth steps) plus the place's offset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LayoutDelta {
    /// The update's placement, of the full link's shape: the whole tables
    /// (chunks grown or moved by the growth steps, the segments derived
    /// again, the TLS template and the entry) complete; `place` indexed by
    /// the update's id space and meaningful only for the sections of
    /// `files`; `holes` only the holes of `hole_chunks`; `sized` empty (no
    /// row `LayoutRows` gives). `defined`: the stored `DEFINED`, whole and
    /// unchanged (chunk-relative values follow their chunks); a row whose
    /// value is a chunk's END is not moved by the layout when the update
    /// grows that chunk (seam 6, from WP-242).
    pub placement: Placement,
    /// The files whose sections this update places (changed, appeared,
    /// drawn-in, newly extracted, gone), sorted.
    pub files: Vec<FileId>,
    /// The chunks whose holes `placement.holes` replaces, sorted; a
    /// touched chunk with no hole left is listed.
    pub hole_chunks: Vec<ChunkId>,
    /// Sections whose place `(chunk, offset)` changed, in any file (IN-27's
    /// moved sections; their files' `SECTIONS` rows are rewritten, IN-78),
    /// sorted. A section moved only by a growth step keeps its place and
    /// is not here (its chunk is in `placement.chunks` and the step in
    /// `growth_steps`; WP-216 F-5).
    pub moved: Vec<SectionId>,
    /// Sections placed before and not now (dead, no longer kept, gone),
    /// sorted.
    pub killed: Vec<SectionId>,
    /// Sections not placed before and placed now (revived, newly kept),
    /// sorted.
    pub revived: Vec<SectionId>,
    /// Holes and dead ranges this update made (IN-3, IN-3a), sorted by
    /// `(chunk, offset)`.
    pub holes: Vec<Hole>,
    /// Regions opened by this update's overflow (IN-72 item 2 phase 3).
    pub regions_opened: Vec<StoredRegion>,
    /// Growth steps, in address order (IN-72 item 3).
    pub growth_steps: Vec<GrowthStep>,
    /// The `.eh_frame` rewritten (IN-72 item 5); `None` when untouched.
    pub eh: Option<EhLayout>,
    /// Fragments whose `refs` or offset changed, appended or revived
    /// (IN-72 item 6), by identity.
    pub frags: Vec<StoredFrag>,
    /// Every `SLOTS` row the update writes (IN-72 item 7), in four lists
    /// (WP-242 B, seam 6). Each is sorted by `(kind, index)` and holds the
    /// row as it is AFTER the update; a `(kind, index)` is in at most one
    /// of them; a stored row in none is unchanged. U-2 writes `SLOTS`
    /// from the stored rows with these replacing theirs; U-6 writes the
    /// bytes of `slots_appended` and `slots_revived`, and 0 over
    /// `slots_dead`.
    ///
    /// Slots appended: an index at or past its kind's stored count.
    pub slots_appended: Vec<StoredSlot>,
    /// Slots retired: a stored row whose `refs` was above 0 and is now 0;
    /// written 0, a [`crate::layout::HoleKind::DeadSlot`] hole in
    /// `holes` (IN-72 item 7, S-113).
    pub slots_dead: Vec<StoredSlot>,
    /// Slots revived: a stored row whose `refs` was 0 and is now above 0
    /// (the same key, its old index); written again as an appended slot
    /// is, its `DeadSlot` hole gone.
    pub slots_revived: Vec<StoredSlot>,
    /// Slots recounted: a stored row whose `refs` was above 0 and is now a
    /// different count above 0; no byte changes, the row only.
    pub slots_recounted: Vec<StoredSlot>,
}

impl LayoutDelta {
    /// Where `id` (the update's id space) is after the update (WP-216
    /// R-3): the one reader of `placement.place`. A section of a file of
    /// `files` (a new id always is one) is [`PlaceNow::Here`] or
    /// [`PlaceNow::NotPlaced`]; any other is [`PlaceNow::Stored`] with its
    /// stored row's place. The address is the chunk's new address (in
    /// `placement.chunks`) plus the place's offset, whichever variant.
    ///
    /// # Errors
    /// What [`LayoutRows::section`] returns.
    pub fn place_of(
        &self,
        stored: &StoredLayout<'_>,
        id: SectionId,
    ) -> Result<PlaceNow, IncrReason> {
        if let Some(row) = stored.section(id)?
            && self.files.binary_search(&row.file).is_err()
        {
            return Ok(PlaceNow::Stored(row.place));
        }
        Ok(match self.placement.place.get(id) {
            Some(p) if *p != Place::NONE => PlaceNow::Here(*p),
            _ => PlaceNow::NotPlaced,
        })
    }

    /// A chunk's holes after the update, by offset (WP-216 R-3): the one
    /// reader of `placement.holes`. A chunk of `hole_chunks` has this
    /// update's (maybe none); any other has its stored ones.
    ///
    /// # Errors
    /// What [`LayoutRows::holes_in`] returns.
    pub fn holes_of(
        &self,
        stored: &StoredLayout<'_>,
        chunk: ChunkId,
    ) -> Result<Vec<Hole>, IncrReason> {
        if self.hole_chunks.binary_search(&chunk).is_err() {
            return stored.holes_in(chunk);
        }
        Ok(self.placement.holes.iter().filter(|h| h.chunk == chunk).copied().collect())
    }
}

// ---- what U-5 gives ---------------------------------------------------------

/// One `.symtab` entry an update writes (IN-49a), by index. A filler is
/// an entry of the filler's form (`name` 0, `info` 0x03, `shndx` 1, `value`
/// section 1's address); the plan lists fillers as ranges instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SymtabEntry {
    /// `.symtab` index.
    pub index: u32,
    /// `st_name`: offset in `.strtab`.
    pub name: u32,
    pub info: u8,
    pub other: u8,
    /// The output section index (`SHN_XINDEX` and `.symtab_shndx` when it
    /// does not fit 16 bits, the writer's).
    pub shndx: u32,
    pub value: u64,
    pub size: u64,
}

/// The update's `.symtab`/`.strtab` plan (WP-211 §C `SymtabPlan`; IN-49a):
/// what U-5 decides and U-6 writes. Not `mortar_synth::SymtabPlan` (the
/// full link's plan). Written by U-5; read by U-6 (the bytes and the map's
/// `hole .symtab`/`.strtab` lines), U-2 (the `SYMTAB` and `SYMTAB_KEYS`
/// rows of the patch).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SymtabPlan {
    /// Every rewritten or moved block, by file: its new place.
    pub blocks: Vec<(FileId, SymtabBlock)>,
    /// The global area after the update.
    pub global_area: SymtabBlock,
    /// The tables' sizes after the update (`sh_info`, `sh_size`, slack).
    pub tables: SymtabTables,
    /// Entries to write, by index.
    pub entries: Vec<SymtabEntry>,
    /// Index ranges `[start, end)` to write as fillers (a block's unused
    /// capacity, a moved block's old place).
    pub fillers: Vec<(u32, u32)>,
    /// `.strtab` bytes to write: `(offset, bytes)` (names, each with its
    /// terminating zero).
    pub strtab: Vec<(u64, Vec<u8>)>,
    /// `.strtab` dead ranges `(offset, size)` (old names, IN-49a: the map's
    /// `dead .strtab <off> <size> strings`).
    pub strtab_dead: Vec<(u64, u64)>,
    /// Each global whose `.symtab` index or `.strtab` offset changed:
    /// `(key, index, strtab offset)` (`SYMTAB_KEYS` rows).
    pub globals: Vec<(SymbolId, u32, u32)>,
    /// Entries U-6 copies from the previous output, `(from, to)` indices
    /// (the global area's last entry taking a removed one's index; IN-49a:
    /// read from the output, not the state).
    pub moves: Vec<(u32, u32)>,
    /// Index ranges `[start, end)` of unchanged files' blocks whose
    /// `st_value`s U-6 remaps through `LayoutDelta::growth_steps`, the rest
    /// of each entry as read (IN-49a).
    pub remap: Vec<(u32, u32)>,
}

// ---- what U-3 gives U-6 -----------------------------------------------------

/// A unit to refill (IN-27, IN-74): a section of the update's id space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RefillUnit {
    pub section: SectionId,
    /// `mortar_incr::delta::reason` bits: why (the report's words).
    pub reasons: u32,
}

/// A synthetic or packed chunk rewritten whole (IN-72 items 4, 5, 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RefillChunk {
    pub chunk: ChunkId,
    /// `mortar_incr::delta::reason` bits.
    pub reasons: u32,
}

/// The refill set of an update (WP-211 §C `RefillSet`: `delta::Delta`'s
/// shape, IN-74). Moved here from `mortar_incr::update::refill` (WP-213):
/// U-6's `mortar_image::update::refill` takes it and `mortar_image` does not
/// depend on `mortar_incr`. Written by U-3; read by U-6, the bench rows
/// (B-1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefillSet {
    /// Units refilled, sorted by section.
    pub units: Vec<RefillUnit>,
    /// Chunks rewritten whole, sorted.
    pub chunks: Vec<RefillChunk>,
    /// Bytes of the units, and of the output's allocated sections (IN-16's
    /// bound is their ratio).
    pub unit_bytes: u64,
    pub alloc_bytes: u64,
    /// Definitions of changed and drawn-in files whose address moved (the
    /// bench row of IN-74).
    pub moved_definitions: u64,
    /// Files with a refilled unit (the bench row of IN-74).
    pub referrer_files: u64,
}

// ---- what U-6 gives U-7 -----------------------------------------------------

/// A range of the output the update writes (WP-211 §C; IN-75: the dirty
/// ranges cover every byte that differs from the previous output). Bytes
/// from the file's start. Written by U-6; read by U-7 (W3/C1), H-1.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DirtyRange {
    pub offset: u64,
    pub len: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: incr.md §7 (one name per reason; the report words of today)
    #[test]
    fn names_round_trip_and_report_words() {
        for name in IncrReason::NAMES {
            let r = IncrReason::from_name(name, "").expect(name);
            assert_eq!(r.name(), name);
            assert_eq!(r.to_string(), name);
            assert_eq!(r.report(), name);
        }
        let r = IncrReason::StateCorrupt("table 7".into());
        assert_eq!(r.report(), "state-corrupt: table 7");
        assert_eq!(r.to_string(), "state-corrupt");
        let r = IncrReason::UpdateNotImplemented(Some(Unimplemented::Debug));
        assert_eq!(r.report(), "update-not-implemented (debug)");
        assert_eq!(r.detail(), "debug");
        for w in Unimplemented::ALL {
            let r = IncrReason::from_name("update-not-implemented", w.word()).unwrap();
            assert_eq!(r, IncrReason::UpdateNotImplemented(Some(w)));
        }
        assert!(IncrReason::from_name("update-not-implemented", "nonsense").is_none());
        assert!(IncrReason::from_name("full requested", "").is_none());
    }

    // spec: resolve.md §7.1 (`--wrap` by name: `S` to `__wrap_S`,
    // `__real_S` to `S`, the last option deciding), §4.2 (the roots' order)
    #[test]
    fn option_keys_redirect_and_roots() {
        let k = |n: &str, arg| OptionKey { name: n.as_bytes().into(), arg };
        let o = OptionKeys {
            entry: Some(k("_start", 0)),
            undefined: vec![k("u", 3)],
            require_defined: vec![k("r", 5)],
            wrap: vec![b"foo".as_slice().into(), b"__real_foo".as_slice().into()],
            defsym: vec![
                OptionDefsym {
                    name: b"d".as_slice().into(),
                    value: DefsymOperand::Number(1),
                    arg: 7,
                },
                OptionDefsym {
                    name: b"e".as_slice().into(),
                    value: DefsymOperand::Offset(b"base".as_slice().into(), 4),
                    arg: 8,
                },
            ],
            gc_sections: false,
            init: Some(k("myinit", 0)),
            fini: Some(k("myfini", 0)),
            gc_keep_exported: false,
        };
        assert_eq!(o.redirect(b"foo"), Some(b"__wrap_foo".to_vec()));
        // `__real_foo` is `S` of the second option, which decides.
        assert_eq!(o.redirect(b"__real_foo"), Some(b"__wrap___real_foo".to_vec()));
        assert_eq!(o.redirect(b"__real___real_foo"), Some(b"__real_foo".to_vec()));
        assert_eq!(o.redirect(b"bar"), None);
        assert!(o.wrapped(b"__wrap_foo") && !o.wrapped(b"bar"));
        let roots: Vec<(&[u8], u32)> = o.roots();
        let want: Vec<(&[u8], u32)> = vec![(b"_start", 0), (b"u", 3), (b"r", 5), (b"base", 8)];
        assert_eq!(roots, want);
        // WP-238 F1, P0-6: the walk's roots are `roots()` then `-init`,
        // `-fini` (never roots of extraction: `roots()` lacks them)
        let live: Vec<(&[u8], u32)> = o.live_roots();
        let mut want_live = want.clone();
        want_live.extend([(b"myinit".as_slice(), 0), (b"myfini".as_slice(), 0)]);
        assert_eq!(live, want_live);
        let only_fini = OptionKeys { init: None, ..o.clone() };
        assert_eq!(only_fini.live_roots().last(), Some(&(b"myfini".as_slice(), 0)));
        assert_eq!(only_fini.live_roots().len(), want.len() + 1);
        assert_eq!(o.defsym_names().collect::<Vec<_>>(), [b"d".as_slice(), b"e"]);
    }

    // spec: incr.md §7 (the type has exactly the table's reasons, in its
    // order, and the update-not-implemented row names every detail word)
    #[test]
    fn names_are_the_spec_table() {
        let text =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../spec/incr.md"))
                .expect("spec/incr.md");
        let start = text.find("\n## 7. Fall-backs").expect("§7");
        let end = start + text[start..].find("\n## 8.").expect("§8");
        let rows: Vec<&str> = text[start..end]
            .lines()
            .filter_map(|l| l.strip_prefix("| `"))
            .filter_map(|l| l.split('`').next())
            .collect();
        assert_eq!(rows, IncrReason::NAMES);
        let row = text[start..end]
            .lines()
            .find(|l| l.starts_with("| `update-not-implemented`"))
            .expect("the row");
        for w in Unimplemented::ALL {
            assert!(row.contains(&format!("`{}`", w.word())), "{}", w.word());
        }
    }

    // spec: incr.md IN-72 (WP-216 R-3: the one reader of a sparse
    // placement), IN-18c (an accessor not given is a named plain link)
    #[test]
    fn place_of_and_holes_of_read_the_sparse_placement() {
        use crate::layout::HoleKind;
        let sec = |id: u32, file: u32, place: Place| StoredSection {
            id: SectionId(id),
            file: FileId(file),
            index: id,
            anchor_kind: 0,
            anchor: Vec::new(),
            name: b".text".to_vec(),
            ordinal: 0,
            sh_type: 1,
            sh_flags: 6,
            size: 16,
            align: 16,
            place,
        };
        let hole = |chunk: u32, offset: u64| Hole {
            chunk: ChunkId(chunk),
            offset,
            size: 16,
            kind: HoleKind::Fill,
        };
        let rows = MemoryLayout {
            sections: vec![
                sec(0, 1, Place::at(ChunkId(0), 0)),
                sec(1, 2, Place::at(ChunkId(0), 16)),
                sec(2, 3, Place::NONE),
            ],
            holes: vec![hole(0, 32), hole(1, 0)],
            ..MemoryLayout::default()
        };
        let stored = StoredLayout::new(&rows);
        // File 2 changed: its old row 1 is gone, its new section is id 3.
        let mut d = LayoutDelta { files: vec![FileId(2)], ..LayoutDelta::default() };
        d.placement.place = crate::Column::from_vec(vec![
            Place::NONE,
            Place::NONE,
            Place::NONE,
            Place::at(ChunkId(0), 48),
        ])
        .unwrap();
        d.hole_chunks = vec![ChunkId(0)];
        d.placement.holes = vec![hole(0, 16)].into();
        let at = |id| d.place_of(&stored, SectionId(id)).unwrap();
        assert_eq!(at(0), PlaceNow::Stored(Place::at(ChunkId(0), 0)));
        assert_eq!(at(1), PlaceNow::NotPlaced);
        assert_eq!(at(2), PlaceNow::Stored(Place::NONE));
        assert_eq!(at(3), PlaceNow::Here(Place::at(ChunkId(0), 48)));
        assert_eq!(at(4), PlaceNow::NotPlaced);
        assert_eq!(d.holes_of(&stored, ChunkId(0)).unwrap(), [hole(0, 16)]);
        assert_eq!(d.holes_of(&stored, ChunkId(1)).unwrap(), [hole(1, 0)]);
        // A touched chunk with no hole left is listed, and has none.
        d.hole_chunks = vec![ChunkId(0), ChunkId(1)];
        assert_eq!(d.holes_of(&stored, ChunkId(1)).unwrap(), []);
    }

    // spec: incr.md IN-18c (an accessor its implementation does not give
    // is a named plain link; WP-216 seam 4)
    #[test]
    fn accessors_not_given_are_a_named_plain_link() {
        let d = LayoutDelta { files: vec![FileId(2)], ..LayoutDelta::default() };
        let bare = StoredLayout::new(&Bare);
        let no = Some(IncrReason::UpdateNotImplemented(Some(Unimplemented::Layout)));
        assert_eq!(d.place_of(&bare, SectionId(0)).err(), no);
        assert_eq!(bare.frags_of(OutSecId(0), 0, [0; 16]).err(), no);
        assert_eq!(bare.frag_uses_of(FileId(0)).err(), no);
        assert_eq!(bare.eh_range_of(FileId(0)).err(), no);
        assert_eq!(bare.key_refs_of(FileId(0)).err(), no);
        assert_eq!(bare.eh_ranges().err(), no);
    }

    // spec: incr.md IN-34a (WP-243, seam 6: every file with a range, by
    // its current rank; `fdes` beside `records`)
    #[test]
    fn eh_ranges_by_current_rank() {
        let r = |offset, records, fdes| StoredEhRange { offset, capacity: 64, records, fdes };
        let rows = MemoryLayout {
            eh_ranges: vec![
                (FileId(1), r(0, 3, 2)),
                (FileId(2), r(64, 0, 0)),
                (FileId(3), r(128, 2, 1)),
                (FileId(6), r(192, 1, 1)),
            ],
            // file 6 appeared between files 1 and 3
            ranks: vec![(FileId(6), (1 << 20) + (1 << 19))],
            ..MemoryLayout::default()
        };
        let v = StoredLayout::new(&rows).eh_ranges().unwrap();
        let files: Vec<(u32, u64)> = v.iter().map(|e| (e.0.0, e.1)).collect();
        assert_eq!(files, [(1, 1 << 20), (6, (1 << 20) + (1 << 19)), (3, 3 << 20)]);
        assert_eq!(v[0].2.fdes, 2);
    }

    /// An implementation that gives none of the seam-4 accessors.
    struct Bare;
    impl LayoutRows for Bare {
        fn chunks(&self) -> Result<Vec<(ChunkId, Chunk)>, IncrReason> {
            Ok(Vec::new())
        }
        fn order(&self) -> Result<Vec<ChunkId>, IncrReason> {
            Ok(Vec::new())
        }
        fn segments(&self) -> Result<Vec<Segment>, IncrReason> {
            Ok(Vec::new())
        }
        fn outsecs(&self) -> Result<Vec<(OutSecId, Vec<u8>)>, IncrReason> {
            Ok(Vec::new())
        }
        fn sections_of(&self, _: FileId) -> Result<Vec<StoredSection>, IncrReason> {
            Ok(Vec::new())
        }
        fn regions_of(&self, _: FileId) -> Result<Vec<StoredRegion>, IncrReason> {
            Ok(Vec::new())
        }
        fn holes_in(&self, _: ChunkId) -> Result<Vec<Hole>, IncrReason> {
            Ok(Vec::new())
        }
        fn slots(&self) -> Result<Vec<StoredSlot>, IncrReason> {
            Ok(Vec::new())
        }
        fn patches(&self) -> Result<Vec<Patch>, IncrReason> {
            Ok(Vec::new())
        }
        fn redirects(&self) -> Result<Vec<Redirect>, IncrReason> {
            Ok(Vec::new())
        }
        fn sym_place(&self) -> Result<Vec<SymPlace>, IncrReason> {
            Ok(Vec::new())
        }
        fn defined(&self) -> Result<Vec<Defined>, IncrReason> {
            Ok(Vec::new())
        }
        fn tls_and_entry(&self) -> Result<(Option<Tls>, Option<u64>), IncrReason> {
            Ok((None, None))
        }
        fn symtab_block(&self, _: FileId) -> Result<Option<SymtabBlock>, IncrReason> {
            Ok(None)
        }
        fn symtab_tables(&self) -> Result<(SymtabBlock, SymtabTables), IncrReason> {
            Ok((SymtabBlock::default(), SymtabTables::default()))
        }
    }
}

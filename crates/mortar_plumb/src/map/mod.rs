//! The placement map: where each input section went (`spec/plumb.md` §3).
//!
//! [`Map`] is the common form every reader produces: mortar's own format
//! ([`mortar`], §3.2) and the map files of GNU ld ([`gnu`], §3.4), lld
//! ([`lld`], §3.5) and mold ([`mold`], §3.6). A foreign map names sections
//! by name, not by index; [`SectionRef`] carries whichever the map gives,
//! and the verifier resolves it against the input files (§3.3).

pub mod gnu;
pub mod lld;
pub mod mold;
pub mod mortar;

use std::fmt;

/// The map formats `parse` reads (§3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Format {
    Mortar,
    Gnu,
    Lld,
    Mold,
}

impl Format {
    /// Every format, in the order of §3.1.
    pub const ALL: [Format; 4] = [Format::Mortar, Format::Gnu, Format::Lld, Format::Mold];

    /// The name `xtask plumb --map-format` takes.
    pub const fn name(self) -> &'static str {
        match self {
            Format::Mortar => "mortar",
            Format::Gnu => "gnu",
            Format::Lld => "lld",
            Format::Mold => "mold",
        }
    }

    /// The format named `name`.
    pub fn from_name(name: &str) -> Option<Format> {
        Format::ALL.into_iter().find(|f| f.name() == name)
    }
}

/// An input file as a map names it: a path, or a member of the archive at
/// a path (§3.3).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MapFile {
    /// The path as the map spells it.
    pub path: String,
    /// The archive member's name, for a member.
    pub member: Option<String>,
    /// The offset of the member's header in the archive, when the map says
    /// it (mortar's format does; the foreign maps do not).
    pub member_offset: Option<u64>,
}

impl MapFile {
    /// A file that is not an archive member.
    pub fn plain(path: impl Into<String>) -> Self {
        MapFile { path: path.into(), member: None, member_offset: None }
    }

    /// A member of an archive.
    pub fn member(path: impl Into<String>, member: impl Into<String>) -> Self {
        MapFile { path: path.into(), member: Some(member.into()), member_offset: None }
    }
}

impl fmt::Display for MapFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.member {
            Some(m) => write!(f, "{}({m})", self.path),
            None => f.write_str(&self.path),
        }
    }
}

/// Which section of a file a record means.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SectionRef {
    /// By index in the file's section header table (mortar's format).
    Index(u32),
    /// By name (the foreign maps): resolved by §3.3.
    Name(String),
}

/// A placed input section, or a placed piece of one (§3.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    /// Index into [`Map::files`].
    pub file: u32,
    pub section: SectionRef,
    /// For a piece: the offset in the input section where the piece starts.
    /// `None` for a whole section.
    pub input_offset: Option<u64>,
    /// The output address of the section's (or piece's) first byte.
    pub addr: u64,
    /// The size in the output.
    pub size: u64,
    /// The map's line, for messages (1-based; 0 when not from text).
    pub line: u32,
}

/// A non-alloc input section in mortar's map (§3.2, `nplace` and
/// `merged`): placed whole at `offset` of output section `out`, or
/// (`offset` `None`) merged into it, its pieces not listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonAlloc {
    pub file: u32,
    pub section: SectionRef,
    pub out: String,
    pub offset: Option<u64>,
    pub size: u64,
    pub line: u32,
}

/// Why a section is not in the output, as far as the map says (§3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DiscardReason {
    /// Garbage-collected (`--gc-sections`).
    Gc,
    /// A member of a COMDAT group another file's copy of which was kept.
    Comdat,
    /// Discarded by a linker script (`/DISCARD/`).
    Script,
    /// The map does not say why (GNU ld's "Discarded input sections").
    Unspecified,
}

impl DiscardReason {
    pub const fn name(self) -> &'static str {
        match self {
            DiscardReason::Gc => "gc",
            DiscardReason::Comdat => "comdat",
            DiscardReason::Script => "script",
            DiscardReason::Unspecified => "unspecified",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        [Self::Gc, Self::Comdat, Self::Script, Self::Unspecified]
            .into_iter()
            .find(|r| r.name() == name)
    }
}

/// An input section the map says is not in the output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discarded {
    pub file: u32,
    pub section: SectionRef,
    pub reason: DiscardReason,
    pub line: u32,
}

/// An input section folded into another (identical code folding): it has
/// the place of `into` (§3.2, read from stage 1 on).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fold {
    pub file: u32,
    pub section: SectionRef,
    pub into_file: u32,
    pub into_section: SectionRef,
    pub line: u32,
}

/// What kind of linker-made content a chunk is (§3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ChunkKind {
    /// A range-extension thunk (veneer).
    Thunk,
    /// An erratum patch (AArch64 843419).
    Patch,
    /// A synthetic section: GOT, PLT, headers, merged strings, notes...
    Synthetic,
    /// The common symbols of a file (GNU ld's `COMMON`).
    Common,
}

impl ChunkKind {
    pub const fn name(self) -> &'static str {
        match self {
            ChunkKind::Thunk => "thunk",
            ChunkKind::Patch => "patch",
            ChunkKind::Synthetic => "synthetic",
            ChunkKind::Common => "common",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        [Self::Thunk, Self::Patch, Self::Synthetic, Self::Common]
            .into_iter()
            .find(|k| k.name() == name)
    }
}

/// Linker-made content at an address (§3.1). Stage 1 reads chunks but does
/// not rely on them: veneers and patches are proven from the output's
/// bytes (§5.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub kind: ChunkKind,
    pub name: String,
    /// `None` for a chunk without an address (non-alloc).
    pub addr: Option<u64>,
    pub size: u64,
    /// For a thunk: the symbol it reaches and the addend.
    pub target: Option<(String, i64)>,
    pub line: u32,
}

/// A common symbol's allocation as mortar's map states it (§3.2, the
/// allocation records; WP-096): judged against the output by M7 (§3.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommonAlloc {
    pub name: String,
    pub addr: u64,
    pub size: u64,
    /// The alignment it was placed at: a power of two.
    pub align: u64,
    pub line: u32,
}

/// A linker-defined symbol and its value (§3.2; not read by stage 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Defined {
    pub name: String,
    pub value: u64,
    pub line: u32,
}

/// The output kind a map records (§3.2 `output`; `spec/dyn.md` D-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OutputKind {
    Static,
    Exec,
    Pie,
    StaticPie,
    /// `-shared` (`spec/dyn.md` D-100; §12.5, WP-121).
    Shared,
}

impl OutputKind {
    pub const ALL: [OutputKind; 5] = [
        OutputKind::Static,
        OutputKind::Exec,
        OutputKind::Pie,
        OutputKind::StaticPie,
        OutputKind::Shared,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            OutputKind::Static => "static",
            OutputKind::Exec => "exec",
            OutputKind::Pie => "pie",
            OutputKind::StaticPie => "static-pie",
            OutputKind::Shared => "shared",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }
}

/// Which tag carries the run path (§3.2 `runpath`/`rpath`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RunpathTag {
    /// `DT_RUNPATH` (`--enable-new-dtags`, the default).
    Runpath,
    /// `DT_RPATH` (`--disable-new-dtags`).
    Rpath,
}

/// A copy allocation as the map states it: an order hint (P-D3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyHint {
    pub name: String,
    pub addr: u64,
    pub size: u64,
    /// The alignment the copy was placed at (§3.2's allocation records,
    /// judged by M8): a power of two.
    pub align: u64,
}

/// The dynamic records of a map (§3.2, `spec/plumb.md` P-D3): facts of the
/// command line (the words themselves are [`Map::args`], in every map), and **order hints** (`needed`, `copy`, `export`, `plt`,
/// `iplt`) the verifier uses only to name what it compares, never as the
/// expected value. Every field is optional in the text: a record that is
/// absent leaves its default.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DynRecords {
    /// `output`.
    pub kind: Option<OutputKind>,
    /// `interp`: the `-dynamic-linker` path.
    pub interp: Option<String>,
    /// `runpath` or `rpath`: the joined `-rpath` list.
    pub runpath: Option<(RunpathTag, String)>,
    /// `bind now` (`-z now`) or `bind lazy`.
    pub now: Option<bool>,
    /// `relro on` or `relro off` (`-z norelro`).
    pub relro: Option<bool>,
    /// `needed`: the `DT_NEEDED` names, in the order written.
    pub needed: Vec<String>,
    /// `copy`: the copy allocations, by the name of the record's alias.
    pub copies: Vec<CopyHint>,
    /// `export`: the exported names.
    pub exports: Vec<String>,
    /// `plt`: the keys of the PLT entries, in PLT order.
    pub plt: Vec<String>,
    /// `iplt`: the keys of the IPLT entries, in IPLT order.
    pub iplt: Vec<String>,
    /// The shared-object line records (`symbolic`, `dynamic-list`,
    /// `version-script`; §3.2, WP-121): `Some` when the map has any.
    pub shared: Option<SharedRecords>,
}

/// The line's symbolic form as `spec/dyn.md` D-112 resolves it (§3.2's
/// `symbolic` record).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Symbolic {
    /// No `-Bsymbolic` form, or `-Bno-symbolic` last.
    None,
    /// `-Bsymbolic` last.
    All,
    /// `-Bsymbolic-functions` last.
    Functions,
}

impl Symbolic {
    pub const ALL: [Symbolic; 3] = [Symbolic::None, Symbolic::All, Symbolic::Functions];

    pub const fn name(self) -> &'static str {
        match self {
            Symbolic::None => "none",
            Symbolic::All => "all",
            Symbolic::Functions => "functions",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }
}

/// The shared-object line records of a `Shared` map (§3.2, read by
/// `crate::maprec`): facts of the line, recomputed from its words.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SharedRecords {
    /// `symbolic`: exactly one in a `Shared` map.
    pub symbolic: Option<Symbolic>,
    /// `dynamic-list`: the files, in line order, as the line names them.
    pub dynamic_lists: Vec<String>,
    /// `version-script`: the files, in line order, as the line names them.
    pub version_scripts: Vec<String>,
}

/// What a `dead` range of mortar's map is (§3.2; `spec/incr.md` IN-3a,
/// IN-60). The word decides which grammar the verifier holds the bytes
/// to (§6.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DeadKind {
    /// A remainder, an allotment's slack or a dead `.debug_info` unit in
    /// its section's neutral form (IN-51, IN-51b).
    Neutral,
    /// A whole dead debug contribution left as its compiler wrote it,
    /// its addresses tombstoned (IN-53).
    Contribution,
    /// Dead fragments of a merged string section (`SHF_MERGE|SHF_STRINGS`,
    /// L-147, S-110): they keep their bytes.
    Strings,
    /// One or more whole CIEs of `.eh_frame` that no FDE uses.
    Cie,
}

impl DeadKind {
    pub const ALL: [DeadKind; 4] =
        [DeadKind::Neutral, DeadKind::Contribution, DeadKind::Strings, DeadKind::Cie];

    pub const fn name(self) -> &'static str {
        match self {
            DeadKind::Neutral => "neutral",
            DeadKind::Contribution => "contribution",
            DeadKind::Strings => "strings",
            DeadKind::Cie => "cie",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }
}

/// A `hole` or `dead` line of mortar's map (§3.2): bytes `[offset, +size)`
/// of the output section named `section` that a full link of the current
/// inputs would not write. `kind` is `None` for a hole (an allocated
/// range that holds its section's fill, IN-3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeadRange {
    pub section: String,
    pub offset: u64,
    pub size: u64,
    pub kind: Option<DeadKind>,
    pub line: u32,
}

/// The common form of a placement map (§3.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Map {
    pub format: Format,
    /// Every file the map names, in the order the map first names it.
    pub files: Vec<MapFile>,
    pub placed: Vec<Placed>,
    pub discarded: Vec<Discarded>,
    pub folds: Vec<Fold>,
    pub chunks: Vec<Chunk>,
    /// mortar's `common` records (§3.2, the allocation records).
    pub commons: Vec<CommonAlloc>,
    pub defined: Vec<Defined>,
    /// mortar's non-alloc records (§3.2).
    pub nonalloc: Vec<NonAlloc>,
    /// Whether the link had `--fix-cortex-a53-843419`, when the caller
    /// knows (no map format records it); `None` infers it from the fixes
    /// found (§5.10).
    pub fix_843419: Option<bool>,
    /// mortar's dynamic records (§3.2; P-D3), when the map has any.
    pub dynamic: Option<DynRecords>,
    /// mortar's `arg` records (§3.2, §3.2a): the link's command line, one
    /// word per record, in order, response files expanded, the program
    /// name left out. A word is bytes (`spec/cli.md` §2.1), not text.
    pub args: Vec<Vec<u8>>,
    /// mortar's `hole` and `dead` lines (§3.2, `spec/incr.md` IN-60), in
    /// map order.
    pub dead: Vec<DeadRange>,
    /// mortar's `incremental` record: the link was made in incremental
    /// mode, and the map claims IN-60's coverage even with no `hole` or
    /// `dead` line.
    pub incremental: bool,
}

impl Map {
    /// An empty map of a format.
    pub fn new(format: Format) -> Self {
        Map {
            format,
            files: Vec::new(),
            placed: Vec::new(),
            discarded: Vec::new(),
            folds: Vec::new(),
            chunks: Vec::new(),
            commons: Vec::new(),
            defined: Vec::new(),
            nonalloc: Vec::new(),
            fix_843419: None,
            dynamic: None,
            args: Vec::new(),
            dead: Vec::new(),
            incremental: false,
        }
    }

    /// Whether the map claims IN-60's coverage (§6.8): mortar's format
    /// with an `incremental` record or any `hole` or `dead` line. Only
    /// such a map pays for the dead-range checks.
    pub fn claims_coverage(&self) -> bool {
        self.format == Format::Mortar && (self.incremental || !self.dead.is_empty())
    }

    /// The index of `file`, adding it when it is new. Linear in the number
    /// of distinct files only through [`FileInterner`]; readers use that.
    pub fn file_index(&self, file: &MapFile) -> Option<u32> {
        self.files.iter().position(|f| f == file).map(|i| i as u32)
    }

    /// The distinct paths the map names, in first-named order: the files
    /// `xtask plumb --inputs-from` loads.
    pub fn paths(&self) -> Vec<&str> {
        let mut seen = std::collections::BTreeSet::new();
        let mut out = Vec::new();
        for f in &self.files {
            if seen.insert(f.path.as_str()) {
                out.push(f.path.as_str());
            }
        }
        out
    }
}

/// Builds [`Map::files`] without a linear search per line.
#[derive(Debug, Default)]
pub(crate) struct FileInterner {
    index: crate::hasher::HashMap<MapFile, u32>,
}

impl FileInterner {
    pub(crate) fn intern(&mut self, map: &mut Map, file: MapFile) -> u32 {
        if let Some(&i) = self.index.get(&file) {
            return i;
        }
        let i = map.files.len() as u32;
        map.files.push(file.clone());
        self.index.insert(file, i);
        i
    }
}

/// A map that cannot be read: the line and what is wrong (§3.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapError {
    /// 1-based; 0 for the file as a whole.
    pub line: u32,
    pub message: String,
}

impl MapError {
    pub(crate) fn new(line: u32, message: impl Into<String>) -> Self {
        MapError { line, message: message.into() }
    }
}

impl fmt::Display for MapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 {
            f.write_str(&self.message)
        } else {
            write!(f, "line {}: {}", self.line, self.message)
        }
    }
}

impl std::error::Error for MapError {}

/// Reads a map of `format` (§3.1).
pub fn parse(format: Format, text: &str) -> Result<Map, MapError> {
    match format {
        Format::Mortar => mortar::parse(text),
        Format::Gnu => gnu::parse(text),
        Format::Lld => lld::parse(text),
        Format::Mold => mold::parse(text),
    }
}

/// Splits a foreign map's file spelling `path(member)` (§3.3): the member
/// is the text between the last `(` and a final `)`. A thin archive's
/// member is a path and may have `/` in it (`libt.a(lib/one.o)`). A
/// spelling without that shape is a plain path.
pub(crate) fn split_member(spec: &str) -> MapFile {
    if let Some(stripped) = spec.strip_suffix(')')
        && let Some(open) = stripped.rfind('(')
        && open > 0
    {
        return MapFile::member(&spec[..open], &stripped[open + 1..]);
    }
    MapFile::plain(spec)
}

/// Parses an unsigned hexadecimal number with an optional `0x`.
pub(crate) fn hex(text: &str) -> Option<u64> {
    let digits = text.strip_prefix("0x").unwrap_or(text);
    if digits.is_empty() || digits.len() > 16 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u64::from_str_radix(digits, 16).ok()
}

/// Parses an unsigned decimal number.
pub(crate) fn dec(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Splits lld's and mold's `file:(section)` input spelling (§3.5): at the
/// last `:(`, with a final `)`.
pub(crate) fn split_input(text: &str) -> Option<(&str, &str)> {
    let inner = text.strip_suffix(')')?;
    let at = inner.rfind(":(")?;
    Some((&inner[..at], &inner[at + 2..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec/plumb.md §3.3: archive member spelling.
    #[test]
    fn s3_3_member_spelling_splits_at_the_last_paren() {
        assert_eq!(
            split_member("/usr/lib/libc.a(ioputs.o)"),
            MapFile::member("/usr/lib/libc.a", "ioputs.o")
        );
        assert_eq!(split_member("hello.o"), MapFile::plain("hello.o"));
        assert_eq!(split_member("/a(b)/lib.a(m.o)"), MapFile::member("/a(b)/lib.a", "m.o"));
        assert_eq!(split_member("/a(b)/x.o"), MapFile::plain("/a(b)/x.o"));
        assert_eq!(split_member("(odd)"), MapFile::plain("(odd)"));
        assert_eq!(split_member("libt.a(lib/one.o)"), MapFile::member("libt.a", "lib/one.o"));
    }

    // §3.5: the input spelling of lld and mold.
    #[test]
    fn s3_5_input_spelling_splits_at_the_last_colon_paren() {
        assert_eq!(split_input("a.o:(.text)"), Some(("a.o", ".text")));
        assert_eq!(split_input("l.a(m.o):(.text.x)"), Some(("l.a(m.o)", ".text.x")));
        assert_eq!(split_input("<internal>:(.got)"), Some(("<internal>", ".got")));
        assert_eq!(split_input("a.o"), None);
    }

    #[test]
    fn numbers() {
        assert_eq!(hex("0x10"), Some(16));
        assert_eq!(hex("ff"), Some(255));
        assert_eq!(hex("0x"), None);
        assert_eq!(hex("0x10000000000000000"), None);
        assert_eq!(dec("12"), Some(12));
        assert_eq!(dec("-1"), None);
    }
}

//! The map file (§9.2, I-77..I-79): `spec/plumb.md` §3.2's `mortar-map 2`.

use std::fmt::Write as _;

use std::os::unix::ffi::OsStrExt;

use mortar_core::par::Pool;
use mortar_core::plan::{Plan, Symbolic};

/// One `file` record (I-78): the found path, and the member name and
/// member header offset of an archive member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapFile {
    pub path: Vec<u8>,
    pub member: Option<(Vec<u8>, u64)>,
}

/// Why a section is not in the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiscardWhy {
    Gc,
    Comdat,
    Script,
    Unspecified,
}

impl DiscardWhy {
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Gc => "gc",
            Self::Comdat => "comdat",
            Self::Script => "script",
            Self::Unspecified => "unspecified",
        }
    }
}

/// A placed record of one file: `place` or `piece`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Placed {
    Place { section: u32, addr: u64, size: u64 },
    Piece { section: u32, input_offset: u64, addr: u64, size: u64 },
}

/// A non-alloc record of one file (R-9, `spec/plumb.md` §3.2): `nplace`,
/// a section placed whole at `offset` of the non-alloc output section
/// `out`; `merged`, an `SHF_MERGE` non-alloc section whose pieces are in
/// `out` and are not listed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NonAlloc {
    Place { section: u32, out: Vec<u8>, offset: u64, size: u64 },
    Merged { section: u32, out: Vec<u8> },
}

/// `chunk`'s kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChunkKind {
    Thunk,
    Patch,
    Synthetic,
    Common,
}

impl ChunkKind {
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Thunk => "thunk",
            Self::Patch => "patch",
            Self::Synthetic => "synthetic",
            Self::Common => "common",
        }
    }
}

/// A `chunk` record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapChunk {
    pub kind: ChunkKind,
    pub name: Vec<u8>,
    pub addr: Option<u64>,
    pub size: u64,
    pub target: Option<(Vec<u8>, i64)>,
}

/// A `common` record (`spec/plumb.md` §3.2, the allocation records): a
/// common symbol's allocation and the alignment it was placed at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapCommon {
    pub name: Vec<u8>,
    pub addr: u64,
    pub size: u64,
    /// A power of two.
    pub align: u64,
}

/// A `fold` record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fold {
    pub file: u32,
    pub section: u32,
    pub into_file: u32,
    pub into_section: u32,
}

/// The records of a map, by group, each in output order (I-78). File ids
/// are map ids: indexes into `files`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Map {
    pub files: Vec<MapFile>,
    /// Per map file, its `place` and `piece` records.
    pub placed: Vec<Vec<Placed>>,
    /// Per map file, its `nplace` and `merged` records (R-9).
    pub nonalloc: Vec<Vec<NonAlloc>>,
    /// Per map file, its `discard` records.
    pub discards: Vec<Vec<(u32, DiscardWhy)>>,
    pub folds: Vec<Fold>,
    pub chunks: Vec<MapChunk>,
    /// The `common` records, in `SymbolId` order (layout L-88).
    pub commons: Vec<MapCommon>,
    pub defines: Vec<(Vec<u8>, u64)>,
    /// The `incremental` record (`spec/plumb.md` §3.2): the link was made
    /// in incremental mode, so the map claims IN-60's coverage, with or
    /// without `hole`/`dead` lines. `false` in a plain link.
    pub incremental: bool,
    /// The `hole` and `dead` records (`spec/plumb.md` §3.2, `spec/incr.md`
    /// IN-60), by output section in header order, then offset: empty in
    /// a plain link.
    pub dead: Vec<DeadLine>,
    /// The dynamic records (`spec/plumb.md` §3.2, `spec/dyn.md` P-D3);
    /// `None` for a static link, which then writes none of them.
    pub dynamic: Option<DynRecords>,
    /// The `arg` records, in every map (`spec/plumb.md` §3.2a, image.md
    /// I-78): the command line's words in order, response files
    /// expanded, the program name excluded.
    pub args: Vec<Vec<u8>>,
    /// The records of a `-r` link (`spec/plumb.md` §14.2): `None` in every
    /// other map, which then writes none of them. A `-r` map has no
    /// `placed`, `nonalloc`, `folds`, `chunks`, `commons`, `defines`, no
    /// incremental record, and of `dynamic` only `kind`
    /// (`Some(OutputKind::Relocatable)`): nothing at `-r` has an address.
    /// RL-4 fills it (`crate::relocatable`, WP-181).
    pub relocatable: Option<RelocatableRecords>,
    /// The decision records (`spec/image.md` I-78a, WP-218): `Some` only
    /// in an incremental-mode map.
    pub decisions: Option<Decisions>,
}

/// An `rplace` record (`spec/plumb.md` §14.2): input section `section`
/// of its map file is the piece at `offset` of the output section whose
/// header index is `out`, with `size` its input size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RPlace {
    pub section: u32,
    pub out: u32,
    pub offset: u64,
    pub size: u64,
}

/// An `rframe` record (`spec/plumb.md` §14.2, `spec/layout.md` L-R6's
/// record map): the record (CIE, FDE or zero terminator) at
/// `input_offset` of input `.eh_frame` `section` of its map file is at
/// `output_offset` of the output `.eh_frame`, or removed (`None`, `-`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RFrame {
    pub section: u32,
    pub input_offset: u64,
    pub output_offset: Option<u64>,
}

/// An `rgroup` record (a **hint**, `spec/plumb.md` §14.2): the output
/// `SHT_GROUP` at header index `out` is made from input group section
/// `section` of map file `file` (the keeper).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RGroup {
    pub out: u32,
    pub file: u32,
    pub section: u32,
}

/// An `rcommon` record (`spec/plumb.md` §14.2, RV-R5, L-R4): a common
/// allocated under `-d`, at `offset` of output section `out` (a header
/// index), with the alignment it was placed at (a power of two).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RCommon {
    pub name: Vec<u8>,
    pub out: u32,
    pub offset: u64,
    pub size: u64,
    pub align: u64,
}

/// The records of a `-r` map (`spec/plumb.md` §14.2), by group, each in
/// output order. File ids are map ids, as in [`Map`]; `discard` records
/// are [`Map::discards`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RelocatableRecords {
    /// Per map file, its `rplace` records.
    pub rplace: Vec<Vec<RPlace>>,
    /// Per map file, its `rframe` records.
    pub rframe: Vec<Vec<RFrame>>,
    /// In output header order.
    pub rgroup: Vec<RGroup>,
    /// In `SymbolId` order (L-R4).
    pub rcommon: Vec<RCommon>,
}

/// The kind of a `dead` record (`spec/plumb.md` §3.2 and §6.8,
/// `spec/incr.md` IN-3a): `neutral` an IN-51/IN-51b neutral form of a
/// debug section, `contribution` a whole dead debug contribution as its
/// compiler wrote it (IN-53), `strings` dead fragments of a merged string
/// section (L-147), `cie` unused CIEs in `.eh_frame`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeadKind {
    Neutral,
    Contribution,
    Strings,
    Cie,
}

impl DeadKind {
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Neutral => "neutral",
            Self::Contribution => "contribution",
            Self::Strings => "strings",
            Self::Cie => "cie",
        }
    }
}

/// A `hole` record (`kind` `None`) or a `dead` record: a range of output
/// section `out` at `offset` from its start.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeadLine {
    pub out: Vec<u8>,
    pub offset: u64,
    pub size: u64,
    pub kind: Option<DeadKind>,
}

/// The `<part>` of a `capacity` record (`spec/image.md` I-78a): `b<k>`
/// the body part `k`, `p<k>` the island after part `k`, `m` the merged
/// chunk, `s` a synthetic chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CapPart {
    Body(u32),
    Island(u32),
    Merged,
    Synthetic,
}

/// A `region` record (I-78a): a file's region in a hole-tolerant output
/// section (IN-30), `cat` L-53's ordinal 1-6 in `.text`, 0 elsewhere.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MapRegion {
    pub file: u32,
    pub out: Vec<u8>,
    pub cat: u8,
    pub addr: u64,
    pub capacity: u64,
}

/// A `capacity` record (I-78a): a chunk of output section `out`, its
/// position `ordinal` among `out`'s chunks in `ChunkId` order.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MapCapacity {
    pub out: Vec<u8>,
    pub ordinal: u32,
    pub part: CapPart,
    /// `None`: a non-allocated chunk (`-`).
    pub addr: Option<u64>,
    pub size: u64,
    pub capacity: u64,
}

/// An `order` record (I-78a): `out`'s chunks' ordinals in file order.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MapOrder {
    pub out: Vec<u8>,
    pub ordinals: Vec<u32>,
}

/// A `slot` record's kind (I-78a), in the records' order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MapSlotKind {
    Got,
    GotTprel,
    TlsGd,
    TlsDesc,
    TlsLd,
    Plt,
    Iplt,
}

impl MapSlotKind {
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Got => "got",
            Self::GotTprel => "got-tprel",
            Self::TlsGd => "tls-gd",
            Self::TlsDesc => "tls-desc",
            Self::TlsLd => "tls-ld",
            Self::Plt => "plt",
            Self::Iplt => "iplt",
        }
    }
}

/// A `slot` record's key (I-78a): a global by name (`name@version` as the
/// `plt` records name it), a local by (map file id, symbol-table index),
/// the module slot (`tls-ld`) without one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MapSlotKey {
    Global(Vec<u8>),
    Local { file: u32, index: u32 },
    Module,
}

/// A `slot` record (I-78a): a slot and its address (a GOT slot's first
/// word, a PLT or IPLT entry's start).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MapSlot {
    pub kind: MapSlotKind,
    pub key: MapSlotKey,
    pub addr: u64,
}

/// A `redirect` record (I-78a): the site (map file, section index,
/// offset) and the address of the patch it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MapRedirect {
    pub file: u32,
    pub section: u32,
    pub offset: u64,
    pub addr: u64,
}

/// A `frag` record (I-78a): a live fragment of a merged output section,
/// its class `<flags>,<entsize>,<align>`, its L128 and its offset from
/// the output section's start.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MapFrag {
    pub out: Vec<u8>,
    pub flags: u64,
    pub entsize: u64,
    pub align: u64,
    pub hash: [u8; 16],
    pub offset: u64,
}

/// A `symblock` record (I-78a, IN-49a): a `.symtab` local block; `file`
/// `None` for the internal file (`-`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MapSymblock {
    pub file: Option<u32>,
    pub first: u32,
    pub count: u32,
    pub capacity: u32,
    pub strtab_off: u64,
    pub strtab_cap: u64,
}

/// The decision records of an incremental-mode map (`spec/image.md`
/// I-78a; `spec/plumb.md` §3.2), each group in I-78a's order, and the
/// `<file>` field of each `common` record (by index into
/// [`Map::commons`]). Written by U-6 (WP-218) only in a map that says
/// `incremental`; `None` in every plain map, which then does not change
/// by one byte.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Decisions {
    pub regions: Vec<MapRegion>,
    pub capacities: Vec<MapCapacity>,
    pub orders: Vec<MapOrder>,
    pub slots: Vec<MapSlot>,
    pub redirects: Vec<MapRedirect>,
    pub frags: Vec<MapFrag>,
    pub symblocks: Vec<MapSymblock>,
    /// Per `common` record, its winning file's map id (`spec/plumb.md`
    /// §3.2: the fifth field, required once the map has a decision
    /// record).
    pub common_files: Vec<Option<u32>>,
}

/// The output kind of the `output` record (`spec/dyn.md` D-1; `Shared`,
/// M4: `spec/plumb.md` §3.2, planned by WP-116).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutputKind {
    Static,
    Exec,
    Pie,
    StaticPie,
    /// A shared object (`-shared`). No link writes it before the front
    /// accepts `-shared` (`spec/cli.md` §4.2, WP-117).
    Shared,
    /// A relocatable object (`-r`, `spec/plumb.md` §14.2's `output
    /// relocatable`). No link writes it before RL-7 lifts the refusal of
    /// `-r` (WP-181).
    Relocatable,
}

impl OutputKind {
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Static => "static",
            Self::Exec => "exec",
            Self::Pie => "pie",
            Self::StaticPie => "static-pie",
            Self::Shared => "shared",
            Self::Relocatable => "relocatable",
        }
    }
}

/// The tag of the run path: `runpath` (`DT_RUNPATH`) or `rpath`
/// (`DT_RPATH`, `--disable-new-dtags`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunpathTag {
    Runpath,
    Rpath,
}

/// The records of a `Shared` link's line (`spec/plumb.md` §3.2, the line
/// records of a shared object; M4, planned by WP-116, read by WP-121):
/// D-112's form of the line, then each `--dynamic-list` and each
/// `--version-script` file as the line names it, in order. Facts of the
/// line: the verifier recomputes each from the `arg` words and reports a
/// record that disagrees.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SharedLine {
    /// `symbolic none|all|functions`: the last of `-Bno-symbolic`,
    /// `-Bsymbolic` and `-Bsymbolic-functions` (`spec/dyn.md` D-112).
    pub symbolic: Symbolic,
    /// `dynamic-list <path>`, one per `--dynamic-list`, the path as given.
    pub dynamic_lists: Vec<Vec<u8>>,
    /// `version-script <path>`, one per `--version-script`, the path as
    /// given.
    pub version_scripts: Vec<Vec<u8>>,
}

impl SharedLine {
    /// The records of `plan`'s line (`Plan::symbolic`, the list and script
    /// files); `None` for a line whose last `-Bsymbolic` form D-112
    /// refuses, which never links.
    #[must_use]
    pub fn of(plan: &Plan) -> Option<Self> {
        let line = plan.symbolic()?;
        let path = |s: &mortar_core::plan::ScriptArg| s.path.as_bytes().to_vec();
        Some(Self {
            symbolic: line.form,
            dynamic_lists: plan.dynamic_lists().map(path).collect(),
            version_scripts: plan.version_scripts().map(path).collect(),
        })
    }
}

/// The token of a `symbolic` record.
#[must_use]
pub const fn symbolic_token(form: Symbolic) -> &'static str {
    match form {
        Symbolic::None => "none",
        Symbolic::All => "all",
        Symbolic::Functions => "functions",
    }
}

/// A `copy` record: the name of the alias the `COPY` record names, the
/// allocation's address and size, and the alignment it was placed at
/// (`spec/dyn.md` D-42; a power of two).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyHint {
    pub name: Vec<u8>,
    pub addr: u64,
    pub size: u64,
    pub align: u64,
}

/// The dynamic records (`spec/plumb.md` §3.2; P-D3): facts of the command
/// line (`output`, `interp`, `runpath`/`rpath`, `bind`, `relro`, `arg`),
/// then order hints the verifier never takes as the expected value
/// (`needed`, `copy`, `export`, `plt`, `iplt`). Each package that makes
/// the fact fills its field; a field left at its default writes no record.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DynRecords {
    pub kind: Option<OutputKind>,
    /// The `-dynamic-linker` path.
    pub interp: Option<Vec<u8>>,
    /// The `-rpath` list joined by `:`, and its tag.
    pub runpath: Option<(RunpathTag, Vec<u8>)>,
    /// `Some(true)` under `-z now`.
    pub now: Option<bool>,
    /// `Some(false)` under `-z norelro`.
    pub relro: Option<bool>,
    /// The line records of a shared object (`symbolic`, `dynamic-list`,
    /// `version-script`): `Some` exactly when `kind` is `Shared`; `None`
    /// writes none of them, so the map of every other kind is unchanged.
    pub shared: Option<SharedLine>,
    /// The `DT_NEEDED` names, in the order written.
    pub needed: Vec<Vec<u8>>,
    pub copies: Vec<CopyHint>,
    /// The exported names, in `.dynsym` order.
    pub exports: Vec<Vec<u8>>,
    /// The PLT entries' keys in PLT order (`name` or `name@version`).
    pub plt: Vec<Vec<u8>>,
    /// The IPLT entries' keys in IPLT order.
    pub iplt: Vec<Vec<u8>>,
}

/// A string token (`spec/plumb.md` §3.2): `!`..`~` except `%` stand for
/// themselves, every other byte is `%XX`, the empty string a lone `%`.
pub fn token(s: &[u8], out: &mut String) {
    if s.is_empty() {
        out.push('%');
        return;
    }
    for &b in s {
        if (b'!'..=b'~').contains(&b) && b != b'%' {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
}

// ---- the decision records of an incremental-mode link (WP-218) -----------

/// The map ids of `link`'s files: the live objects other than the
/// internal file, in `FileId` order (`LinkTables::map`'s `file` records).
fn map_ids(link: &crate::link::Link<'_, '_>) -> Vec<Option<u32>> {
    use mortar_core::FileId;
    use mortar_core::inputs::Reader;
    let mut ids = vec![None; link.inputs.files.len() as usize];
    let mut n = 0u32;
    for (file, row) in link.inputs.files.iter() {
        if file == FileId::INTERNAL
            || !link.resolution.file_live.get(file).is_some_and(|x| x.is_live())
        {
            continue;
        }
        let Reader::Object(_) = row.reader else { continue };
        ids[file.0 as usize] = Some(n);
        n += 1;
    }
    ids
}

/// An input section's header fields: name, flags, size, entsize, file
/// range.
type InputHeader<'s> = (&'s [u8], u64, u64, u64, Option<std::ops::Range<usize>>);

/// The input section header of `s`.
fn input_header<'s>(
    link: &crate::link::Link<'_, 's>,
    s: mortar_core::SectionId,
) -> Option<InputHeader<'s>> {
    use mortar_core::inputs::Reader;
    let (file, index) = link.resolution.sh_index(s)?;
    let Reader::Object(o) = &link.inputs.files.get(file)?.reader else { return None };
    let sec = o.section(index).ok()?;
    Some((
        sec.name().ok()?,
        sec.flags(),
        sec.size(),
        sec.header().sh_entsize,
        sec.file_range().ok().flatten(),
    ))
}

/// `spec/layout.md` L-53's category of an input `.text` section, as its
/// ordinal 1-6 (I-78a's `<cat>`).
fn text_cat(name: &[u8]) -> u8 {
    const CATS: [&[u8]; 4] = [b".text.unlikely", b".text.exit", b".text.startup", b".text.hot"];
    let under = |p: &[u8]| {
        name == p || (name.len() > p.len() && name.starts_with(p) && name[p.len()] == b'.')
    };
    if let Some(i) = CATS.iter().position(|p| under(p)) {
        return i as u8 + 1;
    }
    if name.starts_with(b".text.sorted.") { 5 } else { 6 }
}

/// The decision records of `t`'s link (`spec/image.md` I-78a), for an
/// incremental-mode map: what a full link and an update both state of
/// their `Placement` (the update's is `P_U`), read from the image's
/// tables only. `symblock` is the `.symtab` plan's (IN-49a), which only an
/// update has today ([`update_map`]): a full link writes none.
#[must_use]
#[allow(clippy::too_many_lines)]
pub(crate) fn decisions_of(t: &crate::link::LinkTables<'_, '_>) -> Decisions {
    use mortar_core::layout::{AllocKey, ChunkKind, HoleKind};
    use mortar_core::resolution::Def;
    use mortar_core::SymKind;
    use mortar_elf::consts::{SHF_EXECINSTR, SHF_MERGE, SHF_STRINGS};
    use mortar_synth::got::Region;
    let l = t.link();
    let pl = &l.layout.placement;
    let ids = map_ids(l);
    let id_of = |f: mortar_core::FileId| ids.get(f.0 as usize).copied().flatten();
    let pos = |c: &mortar_core::layout::Chunk| c.addr.unwrap_or(c.offset);
    let header = |c: &mortar_core::layout::Chunk| {
        matches!(
            c.kind,
            ChunkKind::FileHeader | ChunkKind::ProgramHeaders | ChunkKind::SectionHeaders
        )
    };
    // Per section header index: its name and start (as `dead_lines`).
    let mut starts: std::collections::BTreeMap<u32, (u64, &[u8])> =
        std::collections::BTreeMap::new();
    for (_, c) in pl.chunks.iter() {
        if header(c) {
            continue;
        }
        let (Some(i), Some(name)) = (c.shndx, c.name(l.assignment)) else { continue };
        let e = starts.entry(i).or_insert((u64::MAX, name));
        e.0 = e.0.min(pos(c));
    }
    let mut d = Decisions::default();
    // `capacity` and `order`: per output section in header order, its
    // chunks by ordinal (ChunkId order), then in file order.
    let mut ordinal = vec![u32::MAX; pl.chunks.len() as usize];
    let mut count: std::collections::BTreeMap<u32, u32> = std::collections::BTreeMap::new();
    let mut caps: Vec<(u32, MapCapacity)> = Vec::new();
    for (id, c) in pl.chunks.iter() {
        if header(c) {
            continue;
        }
        let Some(i) = c.shndx else { continue };
        let Some(&(_, name)) = starts.get(&i) else { continue };
        let part = match c.kind {
            ChunkKind::Body(p) => CapPart::Body(p.part),
            ChunkKind::Patches(p) => CapPart::Island(p.part),
            ChunkKind::Merged(_) => CapPart::Merged,
            ChunkKind::Synthetic(_) => CapPart::Synthetic,
            _ => continue,
        };
        let n = count.entry(i).or_insert(0);
        ordinal[id.0 as usize] = *n;
        caps.push((
            i,
            MapCapacity {
                out: name.to_vec(),
                ordinal: *n,
                part,
                addr: c.addr,
                size: c.size,
                capacity: c.capacity,
            },
        ));
        *n += 1;
    }
    caps.sort_by_key(|(i, c)| (*i, c.ordinal));
    d.capacities = caps.into_iter().map(|(_, c)| c).collect();
    let mut orders: std::collections::BTreeMap<u32, Vec<u32>> = std::collections::BTreeMap::new();
    for &id in &pl.order {
        let Some(c) = pl.chunks.get(id) else { continue };
        let (Some(i), Some(&n)) = (c.shndx, ordinal.get(id.0 as usize)) else { continue };
        if n != u32::MAX {
            orders.entry(i).or_default().push(n);
        }
    }
    d.orders = orders
        .into_iter()
        .filter_map(|(i, ordinals)| Some(MapOrder { out: starts.get(&i)?.1.to_vec(), ordinals }))
        .collect();
    // `region`: per body chunk of a hole-tolerant output section (one with
    // a fill hole: IN-30's region slack is at least 64 bytes), each run of
    // members of one (file, category) and the fill hole after it.
    let mut fill_holes: std::collections::BTreeMap<u32, Vec<(u64, u64)>> =
        std::collections::BTreeMap::new();
    for h in &pl.holes {
        if h.kind == HoleKind::Fill
            && matches!(pl.chunks.get(h.chunk).map(|c| c.kind), Some(ChunkKind::Body(_)))
        {
            fill_holes.entry(h.chunk.0).or_default().push((h.offset, h.size));
        }
    }
    if !fill_holes.is_empty() {
        let mut members: std::collections::BTreeMap<u32, Vec<(u64, u64, u32, u8)>> =
            std::collections::BTreeMap::new();
        for (sid, p) in pl.place.iter() {
            let Some((c, at)) = p.get() else { continue };
            if !fill_holes.contains_key(&c.0) || !l.liveness.is_live(sid) {
                continue;
            }
            let Some((file, _)) = l.resolution.sh_index(sid) else { continue };
            let Some(id) = id_of(file) else { continue };
            let Some((name, flags, size, _, _)) = input_header(l, sid) else { continue };
            let chunk = &pl.chunks[c];
            let text = chunk.name(l.assignment) == Some(b".text".as_slice());
            let cat = if text && flags & SHF_EXECINSTR != 0 { text_cat(name) } else { 0 };
            members.entry(c.0).or_default().push((at, at + size, id, cat));
        }
        let mut regions: Vec<(u32, u32, u8, u64, MapRegion)> = Vec::new();
        for (c, mut ms) in members {
            ms.sort_unstable();
            let chunk = &pl.chunks[mortar_core::ChunkId(c)];
            let (Some(i), Some(&(_, name))) =
                (chunk.shndx, chunk.shndx.and_then(|i| starts.get(&i)))
            else {
                continue;
            };
            let holes = &fill_holes[&c];
            let base = pos(chunk);
            let mut k = 0;
            while k < ms.len() {
                let (start, mut end, file, cat) = ms[k];
                let mut j = k + 1;
                while j < ms.len() && (ms[j].2, ms[j].3) == (file, cat) {
                    end = end.max(ms[j].1);
                    j += 1;
                }
                if let Some(&(o, sz)) = holes.iter().find(|h| h.0 == end) {
                    end = o + sz;
                }
                let addr = chunk.addr.map_or(base + start, |a| a + start);
                regions.push((
                    file,
                    i,
                    cat,
                    addr,
                    MapRegion { file, out: name.to_vec(), cat, addr, capacity: end - start },
                ));
                k = j;
            }
        }
        regions.sort_by_key(|r| (r.0, r.1, r.2, r.3));
        d.regions = regions.into_iter().map(|r| r.4).collect();
    }
    // `slot`: by kind, then index (the slot plan's order).
    let got = t.got_addrs();
    let plan = l.synth.got;
    let slots = &l.layout.slots;
    let key_of = |k: mortar_core::SymKey| match k.kind() {
        SymKind::Local(e) => id_of(e.file).map(|file| MapSlotKey::Local { file, index: e.index.0 }),
        SymKind::Global(id) => {
            let mut n = l.facts.name(k).to_vec();
            if let Some(dp) = l.synth.dynamic
                && let Some(Def::Shared(e)) = l.resolution.def.get(id).copied()
                && let Some(v) = dp.src.bound_version(e).filter(|v| v.0 >= 2)
            {
                n.push(b'@');
                n.extend_from_slice(dp.src.version(e.file, v));
            }
            Some(MapSlotKey::Global(n))
        }
    };
    let mut out: Vec<MapSlot> = Vec::new();
    let push = |out: &mut Vec<MapSlot>,
                kind: MapSlotKind,
                keys: &[mortar_core::SymKey],
                addr: &dyn Fn(u32) -> Option<u64>| {
        for (n, &k) in keys.iter().enumerate() {
            if let (Some(key), Some(addr)) = (key_of(k), addr(n as u32)) {
                out.push(MapSlot { kind, key, addr });
            }
        }
    };
    let at = |r: Region| move |n: u32| Some(got.got + plan.got_offset(r, n));
    push(&mut out, MapSlotKind::Got, &slots.got, &at(Region::Addr));
    push(&mut out, MapSlotKind::GotTprel, &slots.tls_ie, &at(Region::TpOff));
    push(&mut out, MapSlotKind::TlsGd, &slots.tls_gd, &at(Region::TlsGd));
    push(&mut out, MapSlotKind::TlsDesc, &slots.tls_desc, &at(Region::TlsDesc));
    if slots.tls_ld {
        out.push(MapSlot {
            kind: MapSlotKind::TlsLd,
            key: MapSlotKey::Module,
            addr: got.got + plan.got_offset(Region::TlsLd, 0),
        });
    }
    let plt = |n: u32| plan.plt_address(&got, slots.plt[n as usize]);
    push(&mut out, MapSlotKind::Plt, &slots.plt, &plt);
    push(&mut out, MapSlotKind::Iplt, &slots.iplt, &|n| Some(got.plt + plan.iplt_entry(n)));
    d.slots = out;
    // `redirect`: in the placement's order.
    for r in &pl.redirects {
        let Some((file, index)) = l.resolution.sh_index(r.section) else { continue };
        let Some(file) = id_of(file) else { continue };
        let Some(p) = pl.patches.get(r.patch as usize) else { continue };
        let Some(addr) = pl.chunks.get(p.chunk).and_then(|c| c.addr?.checked_add(p.offset)) else {
            continue;
        };
        d.redirects.push(MapRedirect { file, section: index.0, offset: r.offset, addr });
    }
    // `frag`: the live fragments of merged output sections not named
    // `.debug_*` (`FRAGS`, WP-239 R-1), by output section, class word,
    // hash, `FragId`.
    let merged: std::collections::BTreeMap<u32, &mortar_core::layout::Chunk> = pl
        .chunks
        .iter()
        .filter_map(|(_, c)| match c.kind {
            ChunkKind::Merged(o) => Some((o.0, c)),
            _ => None,
        })
        .collect();
    let mut live: Vec<(u32, u32, [u8; 16], u32, MapFrag)> = Vec::new();
    let mut buf = Vec::new();
    for (frag, &off) in l.layout.merge.frag_off.iter() {
        if off == mortar_core::layout::MergeMap::NONE {
            continue;
        }
        let Some(f) = l.split.frags.get(frag) else { continue };
        let Some(o) = l.assignment.out(f.section) else { continue };
        let Some(c) = merged.get(&o.0) else { continue };
        let (Some(i), Some(name)) = (c.shndx, c.name(l.assignment)) else { continue };
        if name.starts_with(b".debug_") {
            continue;
        }
        let Some((_, flags, _, entsize, Some(range))) = input_header(l, f.section) else {
            continue;
        };
        let Some((file, _)) = l.resolution.sh_index(f.section) else { continue };
        let lo = range.start as u64 + u64::from(f.offset);
        let bytes = lo..lo + u64::from(f.size);
        let hash = if let Some(b) = l.inputs.resident(file, bytes.clone()) {
            mortar_digest::l128(b)
        } else {
            buf.clear();
            buf.resize(f.size as usize, 0);
            if l.inputs.copy_to(file, bytes, &mut buf).is_err() {
                continue;
            }
            mortar_digest::l128(&buf)
        };
        let strings = flags & SHF_STRINGS != 0;
        let class = mortar_core::incr::MergeClass {
            strings,
            entsize: u32::try_from(entsize).unwrap_or(u32::MAX),
            align: f.align,
        };
        let start = starts.get(&i).map_or(0, |s| s.0);
        let offset = pos(c) - start + off;
        live.push((
            i,
            class.word(),
            hash,
            frag.0,
            MapFrag {
                out: name.to_vec(),
                flags: SHF_MERGE | if strings { SHF_STRINGS } else { 0 },
                entsize,
                align: 1u64 << f.align.min(63),
                hash,
                offset,
            },
        ));
    }
    live.sort_by_key(|a| (a.0, a.1, a.2, a.3));
    d.frags = live.into_iter().map(|f| f.4).collect();
    // `common`'s `<file>`: the winning common's file, in the `common`
    // records' order (`LinkTables::map`'s filter).
    for r in &pl.sym_place {
        let AllocKey::Common(id) = r.key else { continue };
        if l.inputs.symbols.get(id).is_none() || t.chunk_at(r.chunk, r.offset).is_none() {
            continue;
        }
        let file = match l.assignment.def(l.resolution, id) {
            Some(Def::Common(e)) => id_of(e.file),
            _ => None,
        };
        d.common_files.push(file);
    }
    d
}

/// The map of an update (WP-211 §C; `spec/image.md` I-78a; WP-218, U-6):
/// the full link's records of `link` (the update's output `P_U`: its
/// `Placement` is the materialized one), `incremental`, the decision
/// records ([`decisions_of`]), each `common`'s `<file>`, and of the
/// `.symtab` plan (IN-49a): the `symblock` of every block it places, the
/// file slack `hole .symtab`/`hole .strtab`, and the `.strtab` dead names
/// (`dead .strtab <off> <size> strings`). The map is written whole.
///
/// Not stated: the `symblock` of a block the plan does not place (an
/// unchanged file's: its place is the stored `SYMTAB` row, which this
/// function is not given; WP-218's record, finding R1).
#[must_use]
pub fn update_map(
    link: &crate::link::Link<'_, '_>,
    layout: &mortar_core::incr::LayoutDelta,
    symtab: &mortar_core::incr::SymtabPlan,
) -> Map {
    let _ = layout;
    let mut m = crate::link::map_of(link);
    m.incremental = true;
    if m.decisions.is_none() {
        m.decisions = Some(decisions_of(&crate::link::LinkTables::new(*link)));
    }
    let ids = map_ids(link);
    let pl = &link.layout.placement;
    let shndx_of = |name: &[u8]| {
        pl.chunks
            .iter()
            .find(|(_, c)| c.name(link.assignment) == Some(name))
            .and_then(|(_, c)| c.shndx)
    };
    let t = &symtab.tables;
    if shndx_of(b".symtab").is_some() {
        let symtab_dead = |out: &[u8], offset, size| DeadLine {
            out: out.to_vec(),
            offset,
            size,
            kind: None,
        };
        m.dead.retain(|d| d.out != b".symtab" && d.out != b".strtab");
        if t.symtab_slack > 0 {
            m.dead.push(symtab_dead(b".symtab", t.symtab_size, t.symtab_slack));
        }
        if t.strtab_slack > 0 {
            m.dead.push(symtab_dead(b".strtab", t.strtab_size, t.strtab_slack));
        }
        for &(offset, size) in &symtab.strtab_dead {
            m.dead.push(DeadLine {
                out: b".strtab".to_vec(),
                offset,
                size,
                kind: Some(DeadKind::Strings),
            });
        }
        let mut order: Vec<(Vec<u8>, Option<u32>)> = Vec::new();
        let mut key = |out: &[u8]| -> u32 {
            if let Some((_, k)) = order.iter().find(|(n, _)| n == out) {
                return k.unwrap_or(u32::MAX);
            }
            let k = shndx_of(out);
            order.push((out.to_vec(), k));
            k.unwrap_or(u32::MAX)
        };
        let mut keyed: Vec<(u32, u64, DeadLine)> =
            m.dead.drain(..).map(|d| (key(&d.out), d.offset, d)).collect();
        keyed.sort_by_key(|k| (k.0, k.1));
        m.dead = keyed.into_iter().map(|k| k.2).collect();
    }
    if let Some(d) = &mut m.decisions {
        let mut blocks: Vec<MapSymblock> = symtab
            .blocks
            .iter()
            .filter_map(|&(file, b)| {
                let file = if file == mortar_core::FileId::INTERNAL {
                    None
                } else {
                    Some(ids.get(file.0 as usize).copied().flatten()?)
                };
                Some(MapSymblock {
                    file,
                    first: b.first,
                    count: b.count,
                    capacity: b.capacity,
                    strtab_off: b.strtab_offset,
                    strtab_cap: b.strtab_capacity,
                })
            })
            .collect();
        blocks.sort_by_key(|b| b.first);
        d.symblocks = blocks;
    }
    m
}

/// The text of `map` (I-79): groups formatted in parallel per file and
/// concatenated in the format's group order.
///
/// A `-r` map's records go in `spec/plumb.md` §14.2's writer order:
/// `rplace` where `place` goes, then `rframe`, `rgroup`, `discard`,
/// `rcommon`, `output`, `arg`.
#[must_use]
pub fn render(map: &Map, pool: &Pool) -> Vec<u8> {
    let mut out = String::from("mortar-map 2\n");
    for (id, f) in map.files.iter().enumerate() {
        let _ = write!(out, "file {id} ");
        token(&f.path, &mut out);
        if let Some((member, off)) = &f.member {
            out.push(' ');
            token(member, &mut out);
            let _ = write!(out, " {off:x}");
        }
        out.push('\n');
    }
    let placed = pool.map(map.placed.len(), |id| {
        let mut s = String::new();
        for p in &map.placed[id] {
            let _ = match *p {
                Placed::Place { section, addr, size } => {
                    writeln!(s, "place {id} {section} {addr:x} {size:x}")
                }
                Placed::Piece { section, input_offset, addr, size } => {
                    writeln!(s, "piece {id} {section} {input_offset:x} {addr:x} {size:x}")
                }
            };
        }
        s
    });
    let nonalloc = pool.map(map.nonalloc.len(), |id| {
        let mut s = String::new();
        for r in &map.nonalloc[id] {
            match r {
                NonAlloc::Place { section, out, offset, size } => {
                    let _ = write!(s, "nplace {id} {section} ");
                    token(out, &mut s);
                    let _ = writeln!(s, " {offset:x} {size:x}");
                }
                NonAlloc::Merged { section, out } => {
                    let _ = write!(s, "merged {id} {section} ");
                    token(out, &mut s);
                    s.push('\n');
                }
            }
        }
        s
    });
    let discards = pool.map(map.discards.len(), |id| {
        let mut s = String::new();
        for (section, why) in &map.discards[id] {
            let _ = writeln!(s, "discard {id} {section} {}", why.token());
        }
        s
    });
    out.extend(placed);
    // A `-r` map has no `nplace`/`merged` record (§14.2).
    if let Some(r) = &map.relocatable {
        render_rplace(r, pool, &mut out);
        render_rframe_rgroup(r, &mut out);
    }
    out.extend(nonalloc);
    out.extend(discards);
    if let Some(r) = &map.relocatable {
        render_rcommon(r, &mut out);
    }
    for f in &map.folds {
        let _ = writeln!(out, "fold {} {} {} {}", f.file, f.section, f.into_file, f.into_section);
    }
    for c in &map.chunks {
        let _ = write!(out, "chunk {} ", c.kind.token());
        token(&c.name, &mut out);
        match c.addr {
            Some(a) => {
                let _ = write!(out, " {a:x} {:x}", c.size);
            }
            None => {
                let _ = write!(out, " - {:x}", c.size);
            }
        }
        if let Some((t, addend)) = &c.target {
            out.push(' ');
            token(t, &mut out);
            if *addend < 0 {
                let _ = write!(out, " -{:x}", addend.unsigned_abs());
            } else {
                let _ = write!(out, " {addend:x}");
            }
        }
        out.push('\n');
    }
    render_commons(map, &mut out);
    for (name, value) in &map.defines {
        out.push_str("define ");
        token(name, &mut out);
        let _ = writeln!(out, " {value:x}");
    }
    render_dead(map, &mut out);
    if let Some(d) = &map.decisions {
        render_decisions(d, &mut out);
    }
    render_dynamic(map.dynamic.as_ref(), &map.args, &mut out);
    out.push_str("end\n");
    out.into_bytes()
}

/// The `common` records; with decision records, each with its `<file>`
/// (`spec/plumb.md` §3.2).
fn render_commons(map: &Map, out: &mut String) {
    let common_files = map.decisions.as_ref().map_or(&[][..], |d| &d.common_files[..]);
    for (i, c) in map.commons.iter().enumerate() {
        out.push_str("common ");
        token(&c.name, out);
        let _ = write!(out, " {:x} {:x} {:x}", c.addr, c.size, c.align);
        if let Some(Some(f)) = common_files.get(i) {
            let _ = write!(out, " {f}");
        }
        out.push('\n');
    }
}

/// The `rplace` records of a `-r` map, per file in parallel.
fn render_rplace(r: &RelocatableRecords, pool: &Pool, out: &mut String) {
    out.extend(pool.map(r.rplace.len(), |id| {
        let mut s = String::new();
        for p in &r.rplace[id] {
            let _ = writeln!(s, "rplace {id} {} {} {:x} {:x}", p.section, p.out, p.offset, p.size);
        }
        s
    }));
}

/// The `rcommon` records of a `-r` map.
fn render_rcommon(r: &RelocatableRecords, out: &mut String) {
    for c in &r.rcommon {
        out.push_str("rcommon ");
        token(&c.name, out);
        let _ = writeln!(out, " {} {:x} {:x} {:x}", c.out, c.offset, c.size, c.align);
    }
}

/// The `rframe` and `rgroup` records of a `-r` map.
fn render_rframe_rgroup(r: &RelocatableRecords, out: &mut String) {
    for (id, frames) in r.rframe.iter().enumerate() {
        for f in frames {
            let _ = write!(out, "rframe {id} {} {:x} ", f.section, f.input_offset);
            let _ = match f.output_offset {
                Some(o) => writeln!(out, "{o:x}"),
                None => writeln!(out, "-"),
            };
        }
    }
    for g in &r.rgroup {
        let _ = writeln!(out, "rgroup {} {} {}", g.out, g.file, g.section);
    }
}

/// The `incremental`, `hole` and `dead` records (`spec/plumb.md` §3.2).
fn render_dead(map: &Map, out: &mut String) {
    if map.incremental {
        out.push_str("incremental\n");
    }
    for d in &map.dead {
        out.push_str(if d.kind.is_some() { "dead " } else { "hole " });
        token(&d.out, out);
        let _ = write!(out, " {:x} {:x}", d.offset, d.size);
        if let Some(k) = d.kind {
            out.push(' ');
            out.push_str(k.token());
        }
        out.push('\n');
    }
}

/// The decision records (`spec/plumb.md` §3.2, `spec/image.md` I-78a), in
/// `region`, `capacity`, `order`, `slot`, `redirect`, `frag`, `symblock`
/// order, each group in its vector's order.
fn render_decisions(d: &Decisions, out: &mut String) {
    for r in &d.regions {
        let _ = write!(out, "region {} ", r.file);
        token(&r.out, out);
        let _ = writeln!(out, " {} {:x} {:x}", r.cat, r.addr, r.capacity);
    }
    for c in &d.capacities {
        out.push_str("capacity ");
        token(&c.out, out);
        let _ = write!(out, " {} ", c.ordinal);
        let _ = match c.part {
            CapPart::Body(k) => write!(out, "b{k}"),
            CapPart::Island(k) => write!(out, "p{k}"),
            CapPart::Merged => write!(out, "m"),
            CapPart::Synthetic => write!(out, "s"),
        };
        let _ = match c.addr {
            Some(a) => write!(out, " {a:x}"),
            None => write!(out, " -"),
        };
        let _ = writeln!(out, " {:x} {:x}", c.size, c.capacity);
    }
    for o in &d.orders {
        out.push_str("order ");
        token(&o.out, out);
        for n in &o.ordinals {
            let _ = write!(out, " {n}");
        }
        out.push('\n');
    }
    for s in &d.slots {
        let _ = write!(out, "slot {}", s.kind.token());
        match &s.key {
            MapSlotKey::Global(name) => {
                out.push(' ');
                token(name, out);
            }
            MapSlotKey::Local { file, index } => {
                let _ = write!(out, " {file} {index}");
            }
            MapSlotKey::Module => {}
        }
        let _ = writeln!(out, " {:x}", s.addr);
    }
    for r in &d.redirects {
        let _ = writeln!(out, "redirect {} {} {:x} {:x}", r.file, r.section, r.offset, r.addr);
    }
    for f in &d.frags {
        out.push_str("frag ");
        token(&f.out, out);
        let _ = write!(out, " {:x},{:x},{:x} ", f.flags, f.entsize, f.align);
        for b in f.hash {
            let _ = write!(out, "{b:02x}");
        }
        let _ = writeln!(out, " {:x}", f.offset);
    }
    for b in &d.symblocks {
        out.push_str("symblock ");
        let _ = match b.file {
            Some(f) => write!(out, "{f}"),
            None => write!(out, "-"),
        };
        let _ = writeln!(
            out,
            " {} {} {} {:x} {:x}",
            b.first, b.count, b.capacity, b.strtab_off, b.strtab_cap
        );
    }
}

/// The dynamic records, in the format's fixed order (`spec/plumb.md` §3.2),
/// with the `arg` records at their place: a static map has those only.
fn render_dynamic(d: Option<&DynRecords>, args: &[Vec<u8>], out: &mut String) {
    let empty = DynRecords::default();
    let d = d.unwrap_or(&empty);
    let named = |out: &mut String, record: &str, name: &[u8]| {
        out.push_str(record);
        out.push(' ');
        token(name, out);
        out.push('\n');
    };
    if let Some(k) = d.kind {
        let _ = writeln!(out, "output {}", k.token());
    }
    if let Some(i) = &d.interp {
        named(out, "interp", i);
    }
    if let Some((tag, paths)) = &d.runpath {
        let record = match tag {
            RunpathTag::Runpath => "runpath",
            RunpathTag::Rpath => "rpath",
        };
        named(out, record, paths);
    }
    if let Some(now) = d.now {
        out.push_str(if now { "bind now\n" } else { "bind lazy\n" });
    }
    if let Some(relro) = d.relro {
        out.push_str(if relro { "relro on\n" } else { "relro off\n" });
    }
    if let Some(line) = &d.shared {
        let _ = writeln!(out, "symbolic {}", symbolic_token(line.symbolic));
        for l in &line.dynamic_lists {
            named(out, "dynamic-list", l);
        }
        for v in &line.version_scripts {
            named(out, "version-script", v);
        }
    }
    for a in args {
        named(out, "arg", a);
    }
    for n in &d.needed {
        named(out, "needed", n);
    }
    for c in &d.copies {
        out.push_str("copy ");
        token(&c.name, out);
        let _ = writeln!(out, " {:x} {:x} {:x}", c.addr, c.size, c.align);
    }
    for e in &d.exports {
        named(out, "export", e);
    }
    for p in &d.plt {
        named(out, "plt", p);
    }
    for p in &d.iplt {
        named(out, "iplt", p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mortar_core::plan::{Bsymbolic, ScriptArg, ScriptRole};

    fn render_dyn(d: DynRecords) -> String {
        let map = Map { dynamic: Some(d), args: vec![b"-shared".to_vec()], ..Map::default() };
        String::from_utf8(render(&map, &Pool::new(1))).unwrap()
    }

    // spec: plumb.md §3.2 (the line records of a shared object, planned by
    // WP-116): `output shared`, then `symbolic`, `dynamic-list` and
    // `version-script` after `relro` and before `arg`, paths as tokens.
    #[test]
    fn s3_2_shared_line_records() {
        let d = DynRecords {
            kind: Some(OutputKind::Shared),
            now: Some(false),
            relro: Some(true),
            shared: Some(SharedLine {
                symbolic: Symbolic::Functions,
                dynamic_lists: vec![b"a.list".to_vec()],
                version_scripts: vec![b"v 1.map".to_vec(), b"v2.map".to_vec()],
            }),
            ..DynRecords::default()
        };
        assert_eq!(
            render_dyn(d),
            "mortar-map 2\noutput shared\nbind lazy\nrelro on\nsymbolic functions\n\
             dynamic-list a.list\nversion-script v%201.map\nversion-script v2.map\n\
             arg -shared\nend\n"
        );
    }

    // spec: plumb.md §3.2: without `shared` no line record is written, so
    // the map of every other kind is byte-identical to its map before them.
    #[test]
    fn s3_2_no_shared_line_records_outside_shared() {
        let d = DynRecords {
            kind: Some(OutputKind::Pie),
            now: Some(true),
            relro: Some(true),
            ..DynRecords::default()
        };
        assert_eq!(
            render_dyn(d),
            "mortar-map 2\noutput pie\nbind now\nrelro on\narg -shared\nend\n"
        );
    }

    // spec: dyn.md D-112 (the last form wins, `--dynamic-list` beside it),
    // cli.md §3.6; the refused forms have no record.
    #[test]
    fn d112_shared_line_of_a_plan() {
        let mut plan = Plan::default();
        assert_eq!(SharedLine::of(&plan), Some(SharedLine::default()));
        plan.dynamic.bsymbolic = Some(Bsymbolic::All);
        plan.scripts = vec![
            ScriptArg { arg: 1, role: ScriptRole::VersionScript, path: "v.map".into() },
            ScriptArg { arg: 2, role: ScriptRole::DynamicList, path: "d.list".into() },
            ScriptArg { arg: 3, role: ScriptRole::ExportDynamicSymbolList, path: "e".into() },
            ScriptArg { arg: 4, role: ScriptRole::VersionScript, path: "w.map".into() },
        ];
        let line = SharedLine::of(&plan).unwrap();
        assert_eq!(line.symbolic, Symbolic::All);
        assert_eq!(line.dynamic_lists, [b"d.list".to_vec()]);
        assert_eq!(line.version_scripts, [b"v.map".to_vec(), b"w.map".to_vec()]);
        plan.dynamic.bsymbolic = Some(Bsymbolic::NonWeakFunctions);
        assert_eq!(SharedLine::of(&plan), None);
    }

    // spec: plumb.md §14.2 (the map at `-r`, WP-181's writer): `rplace`
    // where `place` goes, then `rframe`, `rgroup`, `discard`, `rcommon`,
    // `output relocatable`, `arg`; offsets in hex, header indexes decimal.
    #[test]
    fn s14_2_relocatable_records_in_writer_order() {
        let map = Map {
            files: vec![MapFile { path: b"a.o".to_vec(), member: None }],
            discards: vec![vec![(7, DiscardWhy::Comdat)]],
            dynamic: Some(DynRecords {
                kind: Some(OutputKind::Relocatable),
                ..DynRecords::default()
            }),
            args: vec![b"-r".to_vec()],
            relocatable: Some(RelocatableRecords {
                rplace: vec![vec![RPlace { section: 1, out: 12, offset: 0x10, size: 0x20 }]],
                rframe: vec![vec![
                    RFrame { section: 4, input_offset: 0, output_offset: Some(0x18) },
                    RFrame { section: 4, input_offset: 0x18, output_offset: None },
                ]],
                rgroup: vec![RGroup { out: 2, file: 0, section: 3 }],
                rcommon: vec![RCommon {
                    name: b"c".to_vec(),
                    out: 13,
                    offset: 0x8,
                    size: 0x4,
                    align: 0x4,
                }],
            }),
            ..Map::default()
        };
        assert_eq!(
            String::from_utf8(render(&map, &Pool::new(1))).unwrap(),
            "mortar-map 2\nfile 0 a.o\nrplace 0 1 12 10 20\nrframe 0 4 0 18\nrframe 0 4 18 -\n\
             rgroup 2 0 3\ndiscard 0 7 comdat\nrcommon c 13 8 4 4\noutput relocatable\n\
             arg -r\nend\n"
        );
        assert_eq!(OutputKind::Relocatable.token(), "relocatable");
    }

    fn decisions_map() -> Map {
        Map {
            files: vec![MapFile { path: b"a.o".to_vec(), member: None }],
            commons: vec![MapCommon { name: b"c".to_vec(), addr: 0x40, size: 4, align: 4 }],
            incremental: true,
            dead: vec![DeadLine { out: b".text".to_vec(), offset: 0x10, size: 0x40, kind: None }],
            dynamic: Some(DynRecords { kind: Some(OutputKind::Static), ..DynRecords::default() }),
            decisions: Some(Decisions {
                regions: vec![MapRegion {
                    file: 0,
                    out: b".text".to_vec(),
                    cat: 6,
                    addr: 0x401000,
                    capacity: 0x50,
                }],
                capacities: vec![
                    MapCapacity {
                        out: b".text".to_vec(),
                        ordinal: 0,
                        part: CapPart::Body(0),
                        addr: Some(0x401000),
                        size: 0x50,
                        capacity: 0x1050,
                    },
                    MapCapacity {
                        out: b".text".to_vec(),
                        ordinal: 1,
                        part: CapPart::Island(0),
                        addr: Some(0x402050),
                        size: 0,
                        capacity: 0,
                    },
                    MapCapacity {
                        out: b".comment".to_vec(),
                        ordinal: 0,
                        part: CapPart::Merged,
                        addr: None,
                        size: 0x20,
                        capacity: 0x20,
                    },
                    MapCapacity {
                        out: b".got".to_vec(),
                        ordinal: 0,
                        part: CapPart::Synthetic,
                        addr: Some(0x403000),
                        size: 8,
                        capacity: 8,
                    },
                ],
                orders: vec![MapOrder { out: b".text".to_vec(), ordinals: vec![0, 1] }],
                slots: vec![
                    MapSlot {
                        kind: MapSlotKind::Got,
                        key: MapSlotKey::Global(b"g".to_vec()),
                        addr: 0x403000,
                    },
                    MapSlot {
                        kind: MapSlotKind::GotTprel,
                        key: MapSlotKey::Local { file: 0, index: 12 },
                        addr: 0x403008,
                    },
                    MapSlot { kind: MapSlotKind::TlsLd, key: MapSlotKey::Module, addr: 0x403010 },
                    MapSlot {
                        kind: MapSlotKind::Iplt,
                        key: MapSlotKey::Global(b"f@V 1".to_vec()),
                        addr: 0x404000,
                    },
                ],
                redirects: vec![MapRedirect { file: 0, section: 3, offset: 0x18, addr: 0x402050 }],
                frags: vec![MapFrag {
                    out: b".rodata".to_vec(),
                    flags: 0x30,
                    entsize: 1,
                    align: 1,
                    hash: [0xab; 16],
                    offset: 0x7,
                }],
                symblocks: vec![
                    MapSymblock {
                        file: None,
                        first: 1,
                        count: 2,
                        capacity: 6,
                        strtab_off: 1,
                        strtab_cap: 0x10,
                    },
                    MapSymblock {
                        file: Some(0),
                        first: 7,
                        count: 3,
                        capacity: 7,
                        strtab_off: 0x11,
                        strtab_cap: 0x20,
                    },
                ],
                common_files: vec![Some(0)],
            }),
            args: vec![b"-o".to_vec()],
            ..Map::default()
        }
    }

    // spec: plumb.md §3.2 (the decision records), image.md I-78a: after
    // `incremental`/`hole`/`dead` and before the dynamic records, in
    // `region`, `capacity`, `order`, `slot`, `redirect`, `frag`, `symblock`
    // order; `common` with its `<file>`; numbers hex except the decimal
    // fields; strings as tokens; the hash 32 lower-case digits.
    #[test]
    fn i78a_decision_records_grammar_and_order() {
        let text = String::from_utf8(render(&decisions_map(), &Pool::new(1))).unwrap();
        assert_eq!(
            text,
            "mortar-map 2\nfile 0 a.o\ncommon c 40 4 4 0\nincremental\nhole .text 10 40\n\
             region 0 .text 6 401000 50\n\
             capacity .text 0 b0 401000 50 1050\n\
             capacity .text 1 p0 402050 0 0\n\
             capacity .comment 0 m - 20 20\n\
             capacity .got 0 s 403000 8 8\n\
             order .text 0 1\n\
             slot got g 403000\n\
             slot got-tprel 0 12 403008\n\
             slot tls-ld 403010\n\
             slot iplt f@V%201 404000\n\
             redirect 0 3 18 402050\n\
             frag .rodata 30,1,1 abababababababababababababababab 7\n\
             symblock - 1 2 6 1 10\n\
             symblock 0 7 3 7 11 20\n\
             output static\narg -o\nend\n"
        );
    }

    // spec: image.md I-78a "A plain-mode map has none, and its bytes do not
    // change": without decisions, `common` keeps four fields and no record
    // is added.
    #[test]
    fn i78a_plain_map_unchanged() {
        let mut m = decisions_map();
        m.decisions = None;
        m.incremental = false;
        m.dead.clear();
        assert_eq!(
            String::from_utf8(render(&m, &Pool::new(1))).unwrap(),
            "mortar-map 2\nfile 0 a.o\ncommon c 40 4 4\noutput static\narg -o\nend\n"
        );
    }

    // spec: L-53's categories as I-78a's ordinal 1-6.
    #[test]
    fn l53_text_category_ordinals() {
        assert_eq!(text_cat(b".text.unlikely"), 1);
        assert_eq!(text_cat(b".text.unlikely.f"), 1);
        assert_eq!(text_cat(b".text.unlikelyx"), 6);
        assert_eq!(text_cat(b".text.exit"), 2);
        assert_eq!(text_cat(b".text.startup.main"), 3);
        assert_eq!(text_cat(b".text.hot"), 4);
        assert_eq!(text_cat(b".text.sorted.0001"), 5);
        assert_eq!(text_cat(b".text"), 6);
        assert_eq!(text_cat(b".text.f"), 6);
    }
}

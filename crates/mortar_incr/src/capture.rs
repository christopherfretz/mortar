//! A link's tables in identity terms (`spec/model.md` §7.1) and back:
//! [`capture`] writes the rows of a state from one link's tables;
//! [`replay`] translates a state's decision tables to another link's ids
//! (IN-61; the update of WP-064 will translate the same way).
//!
//! The parts of an identity read from input bytes (a section's anchor, a
//! local symbol's name) come from each object's symbol table, read here
//! once per file ([`Keys::new`]).

use mortar_core::hash::HashMap;

use mortar_core::assignment::Assignment;
use mortar_core::column::Column;
use mortar_core::id::{
    ChunkId, FileId, FragId, Id, OutSecId, SectionId, SymKey, SymKind, SymbolId,
};
use mortar_core::identity::{
    Anchor, ChunkIdentity, ChunkTag, FileIdentity, Identities, LocalIdentity, Lookup, Ordinals,
    SectionIdentity, SectionKey, SymbolIdentity, chunk_id, chunk_ordinals, outsec_id,
    outsec_ordinals, symbol_id, symbol_lookup,
};
use mortar_core::inputs::{Inputs, Reader};
use mortar_core::layout::{
    AllocKey, Chunk, ChunkKind, DefValue, Defined, EhRecord, Erratum, Hole, HoleKind, Layout,
    MergeMap, Patch, PatchKind, Place, Placement, Redirect, Segment, Sized, SlotKind, Slots,
    SymPlace, ThunkTarget, Tls,
};
use mortar_core::par::Pool;
use mortar_core::resolution::Resolution;
use mortar_core::split::Split;
use mortar_elf::consts::{STT_FILE, STT_SECTION};
use mortar_elf::{SectionIndex, SymbolSection};

use crate::format::{
    ChunkRow, DefinedRow, EhRow, FileRow, FragRow, HoleRow, NONE, OutSecRow, PatchRow, ProbeRow,
    RedirectRow, SectionRow, SegmentRow, SizeRow, SlotRow, SpanRow, State, Str, Strings,
    SymPlaceRow, SymbolRow, chunk_bits, hole_kind, layout_bits, patch_kind, sym_kind,
};

/// The tables of one link that identities are computed from.
#[derive(Debug, Clone, Copy)]
pub struct Link<'a, 's> {
    pub inputs: &'a Inputs<'s>,
    pub resolution: &'a Resolution,
    pub assignment: &'a Assignment<'s>,
    pub split: &'a Split,
}

/// One link's identities and the ways back (§7.1).
#[derive(Debug)]
pub struct Keys<'s> {
    pub identities: Identities<'s>,
    pub outsecs: Ordinals<&'s [u8]>,
    pub symbols: Lookup<SymbolId>,
    /// Whether every file was read and `symbols` holds every key: false
    /// for [`Keys::partial`] (WP-094).
    pub complete: bool,
}

/// The slot lists in `SLOTS`' kind order (IN-18a).
const SLOT_LISTS: [SlotKind; 6] = [
    SlotKind::Got,
    SlotKind::Plt,
    SlotKind::Iplt,
    SlotKind::TlsDesc,
    SlotKind::TlsGd,
    SlotKind::TlsIe,
];

fn slot_list(slots: &Slots, kind: SlotKind) -> &[SymKey] {
    match kind {
        SlotKind::Got => &slots.got,
        SlotKind::Plt => &slots.plt,
        SlotKind::Iplt => &slots.iplt,
        SlotKind::TlsDesc => &slots.tls_desc,
        SlotKind::TlsGd => &slots.tls_gd,
        SlotKind::TlsIe => &slots.tls_ie,
    }
}

fn slot_list_mut(slots: &mut Slots, kind: SlotKind) -> &mut Box<[SymKey]> {
    match kind {
        SlotKind::Got => &mut slots.got,
        SlotKind::Plt => &mut slots.plt,
        SlotKind::Iplt => &mut slots.iplt,
        SlotKind::TlsDesc => &mut slots.tls_desc,
        SlotKind::TlsGd => &mut slots.tls_gd,
        SlotKind::TlsIe => &mut slots.tls_ie,
    }
}

const TAGS: [ChunkTag; 9] = [
    ChunkTag::FileHeader,
    ChunkTag::ProgramHeaders,
    ChunkTag::SectionHeaders,
    ChunkTag::Body,
    ChunkTag::Merged,
    ChunkTag::Patches,
    ChunkTag::Synthetic,
    ChunkTag::Group,
    ChunkTag::Relocs,
];

fn hole_byte(kind: HoleKind) -> u8 {
    match kind {
        HoleKind::Fill => hole_kind::FILL,
        HoleKind::EhPad => hole_kind::EH_PAD,
        HoleKind::Debug => hole_kind::DEBUG,
        HoleKind::AbbrevTable => hole_kind::ABBREV_TABLE,
        HoleKind::DeadDebug => hole_kind::DEAD_DEBUG,
        HoleKind::DeadFragment => hole_kind::DEAD_FRAGMENT,
        HoleKind::DeadCie => hole_kind::DEAD_CIE,
    }
}

fn hole_of(b: u8) -> Result<HoleKind, String> {
    Ok(match b {
        hole_kind::FILL => HoleKind::Fill,
        hole_kind::EH_PAD => HoleKind::EhPad,
        hole_kind::DEBUG => HoleKind::Debug,
        hole_kind::ABBREV_TABLE => HoleKind::AbbrevTable,
        hole_kind::DEAD_DEBUG => HoleKind::DeadDebug,
        hole_kind::DEAD_FRAGMENT => HoleKind::DeadFragment,
        hole_kind::DEAD_CIE => HoleKind::DeadCie,
        _ => return Err(format!("hole kind {b}")),
    })
}

fn tag_byte(tag: ChunkTag) -> u8 {
    TAGS.iter().position(|&t| t == tag).unwrap_or(0) as u8
}

impl<'s> Keys<'s> {
    /// Reads every object's anchors and local names (§7.1).
    pub fn new(link: &Link<'_, 's>) -> Result<Self, String> {
        Self::build(link, &|_| true, true, None)
    }

    /// WP-094 (c): the keys [`capture`] needs, every file's read in
    /// parallel on `pool`, without the global lookup (a replay's).
    pub fn for_capture(link: &Link<'_, 's>, pool: &Pool) -> Result<Self, String> {
        Self::build(link, &|_| true, false, Some(pool))
    }

    /// WP-094: the anchors and local names of the files `read` selects
    /// only, and `symbols` over the keys those files mention: what the
    /// replay by position needs ([`replay_by_position`]). The other files'
    /// sections and locals have no keys here.
    pub fn partial(
        link: &Link<'_, 's>,
        read: &(dyn Fn(FileId) -> bool + Sync),
    ) -> Result<Self, String> {
        Self::build(link, read, false, None)
    }

    /// One object's section keys and local names.
    fn file_keys(
        link: &Link<'_, 's>,
        file: FileId,
    ) -> Result<(Ordinals<SectionKey<'s>>, Ordinals<&'s [u8]>), String> {
        let inputs = link.inputs;
        let f = &inputs.files[file];
        let Reader::Object(object) = f.reader else {
            return Ok((Ordinals::new(Vec::new()), Ordinals::new(Vec::new())));
        };
        let bad = |what: &str| format!("{}: {what}", String::from_utf8_lossy(f.path));
        let count = object.section_count() as usize;
        let mut global: Vec<Option<&'s [u8]>> = vec![None; count];
        let mut local: Vec<Option<&'s [u8]>> = vec![None; count];
        let mut names = Vec::new();
        if let Some(symbols) = object.symbols().map_err(|_| bad("symbol table"))? {
            let first_global = symbols.first_global().0;
            for (index, sym) in symbols.iter() {
                let name = symbols.name(&sym).map_err(|_| bad("symbol name"))?;
                if index.0 < first_global {
                    names.push(name);
                }
                let kind = sym.st_info & 0xf;
                if name.is_empty() || kind == STT_SECTION || kind == STT_FILE || sym.st_value != 0 {
                    continue;
                }
                let Ok(SymbolSection::Section(at)) = symbols.section_of(index, &sym) else {
                    continue;
                };
                let slot = if index.0 < first_global { &mut local } else { &mut global };
                if let Some(s @ None) = slot.get_mut(at.0 as usize) {
                    *s = Some(name);
                }
            }
        }
        let mut keys = Vec::with_capacity(count);
        for i in 0..count {
            let name = match object.section(SectionIndex(i as u32)) {
                Ok(s) if i > 0 => s.name().map_err(|_| bad("section name"))?,
                _ => &[][..],
            };
            let comdat = link
                .resolution
                .section_id(file, SectionIndex(i as u32))
                .and_then(|id| link.resolution.sections.get(id))
                .and_then(|row| row.comdat.get())
                .and_then(|c| inputs.comdats.get(c))
                .map(|c| c.name);
            let anchor = Anchor::choose(comdat, global[i], local[i]);
            keys.push(SectionKey { anchor, name });
        }
        Ok((Ordinals::new(keys), Ordinals::new(names)))
    }

    fn build(
        link: &Link<'_, 's>,
        read: &(dyn Fn(FileId) -> bool + Sync),
        complete: bool,
        pool: Option<&Pool>,
    ) -> Result<Self, String> {
        let inputs = link.inputs;
        let one = |i: usize| {
            let file = FileId(i as u32);
            if read(file) {
                Self::file_keys(link, file)
            } else {
                Ok((Ordinals::new(Vec::new()), Ordinals::new(Vec::new())))
            }
        };
        let n = inputs.files.len() as usize;
        let rows: Vec<_> = match pool {
            Some(pool) => pool.map(n, one),
            None => (0..n).map(one).collect(),
        };
        let mut sections = Vec::with_capacity(n);
        let mut locals = Vec::with_capacity(n);
        for r in rows {
            let (s, l) = r?;
            sections.push(s);
            locals.push(l);
        }
        let sections = Column::from_vec(sections).map_err(|_| "too many files".to_owned())?;
        let locals = Column::from_vec(locals).map_err(|_| "too many files".to_owned())?;
        let symbols = if complete {
            symbol_lookup(&inputs.symbols)
        } else if pool.is_some() {
            Lookup::new(std::iter::empty(), |s: SymbolId| s)
        } else {
            let mut ids: Vec<SymbolId> = Vec::new();
            for (file, _) in inputs.files.iter() {
                if read(file) {
                    ids.extend_from_slice(inputs.sym_map_of(file));
                }
            }
            ids.sort_unstable();
            ids.dedup();
            let k = &inputs.symbols;
            Lookup::new(ids.into_iter(), |s| (k[s].name, k[s].version))
        };
        Ok(Self {
            identities: Identities::new(inputs.file_identities(), sections, locals),
            outsecs: outsec_ordinals(link.assignment),
            symbols,
            complete,
        })
    }
}

/// The content hash of a fragment's bytes (`L128`, IN-18a `FRAGS`).
/// `buf` holds the bytes of a fragment not resident in the store; it is
/// reused from one fragment to the next.
fn frag_hash(link: &Link<'_, '_>, frag: FragId, buf: &mut Vec<u8>) -> Option<[u8; 16]> {
    let f = link.split.frags.get(frag)?;
    let (file, index) = link.resolution.sh_index(f.section)?;
    let Reader::Object(object) = link.inputs.files.get(file)?.reader else { return None };
    let range = object.section(index).ok()?.file_range().ok()??;
    let start = range.start as u64 + u64::from(f.offset);
    let range = start..start + u64::from(f.size);
    if let Some(bytes) = link.inputs.resident(file, range.clone()) {
        return Some(mortar_digest::l128(bytes));
    }
    buf.clear();
    buf.resize(f.size as usize, 0);
    link.inputs.copy_to(file, range, buf).ok()?;
    Some(mortar_digest::l128(buf))
}

/// A section's header as `SECTIONS` stores it: `sh_type`, `sh_flags`,
/// `sh_size`, alignment and file offset (0 for `SHT_NOBITS`); zeros for a
/// section of a file that is not an object.
pub(crate) fn header_fields(
    inputs: &Inputs<'_>,
    file: FileId,
    index: SectionIndex,
) -> (u32, u64, u64, u64, u64) {
    if let Some(f) = inputs.files.get(file)
        && let Reader::Object(object) = f.reader
        && let Ok(sec) = object.section(index)
    {
        (
            sec.sh_type(),
            sec.flags(),
            sec.size(),
            sec.alignment().unwrap_or(0),
            sec.file_range().ok().flatten().map_or(0, |r| r.start as u64),
        )
    } else {
        (0, 0, 0, 0, 0)
    }
}

/// What the dry run needs of the new link beyond its state
/// ([`crate::delta::Extra`]): each section's index in its file, and the
/// content hash of every fragment of its merged sections that have an
/// output section.
#[must_use]
pub fn extra(link: &Link<'_, '_>) -> crate::delta::Extra {
    let res = link.resolution;
    let shndx =
        res.sections.iter().map(|(id, _)| res.sh_index(id).map_or(0, |(_, i)| i.0)).collect();
    let mut hashes: Vec<Option<Option<[u8; 16]>>> = vec![None; link.split.frags.len() as usize];
    let mut frags = Vec::new();
    let mut buf = Vec::new();
    for row in &link.split.merge_of {
        let Some(out) = link.assignment.out(row.section) else { continue };
        let mut seen = Vec::new();
        for p in row.pieces.slots() {
            let Some(&frag) = link.split.piece_frag.get(p) else { continue };
            let Some(slot) = hashes.get_mut(frag.0 as usize) else { continue };
            let h = *slot.get_or_insert_with(|| frag_hash(link, frag, &mut buf));
            // A fragment whose bytes cannot be read is no fragment the
            // delta can find: the section counts as moved (an unknown hash).
            let h = h.unwrap_or([0xff; 16]);
            seen.push(h);
        }
        seen.sort_unstable();
        seen.dedup();
        frags.extend(seen.into_iter().map(|h| (row.section.0, out.0, h)));
    }
    let mut eh_sections = Vec::with_capacity(link.split.eh_of.len());
    let mut eh_cie = Vec::with_capacity(link.split.eh_piece.len());
    for row in &link.split.eh_of {
        eh_sections.push((row.section.0, eh_cie.len() as u32));
        eh_cie.extend(
            link.split.eh_piece.get(row.pieces.slots()).into_iter().flatten().map(|p| p.cie),
        );
    }
    crate::delta::Extra { shndx, frags, eh_sections, eh_cie }
}

fn anchor_row(strings: &mut Strings, anchor: Anchor<'_>) -> (u8, Str) {
    match anchor {
        Anchor::None => (0, Str::default()),
        Anchor::Comdat(n) => (1, strings.add(n)),
        Anchor::Global(n) => (2, strings.add(n)),
        Anchor::Local(n) => (3, strings.add(n)),
    }
}

pub(crate) struct Symbols<'k, 's> {
    keys: &'k Keys<'s>,
    inputs: &'k Inputs<'s>,
    rows: Vec<SymbolRow>,
    index: HashMap<SymKey, u32>,
    /// Each global's first mention (file, entry from `first_global`), made
    /// on the first global row (WP-094).
    first: Vec<(u32, u32)>,
    /// Each global's row by `SymbolId` (`NONE`: none yet), made on the
    /// first global row: the lookup without hashing (WP-094 (c)).
    global_rows: Vec<u32>,
}

/// Each global's first mention: the file and its entry among the file's
/// non-local entries; `(NONE, 0)` for a key no file mentions.
fn first_mentions(inputs: &Inputs<'_>) -> Vec<(u32, u32)> {
    let mut first = vec![(NONE, 0); inputs.symbols.len() as usize];
    for (file, _) in inputs.files.iter() {
        for (i, id) in inputs.sym_map_of(file).iter().enumerate() {
            if let Some(slot @ (NONE, _)) = first.get_mut(id.0 as usize) {
                *slot = (file.0, i as u32);
            }
        }
    }
    first
}

impl Symbols<'_, '_> {
    pub(crate) fn row(&mut self, strings: &mut Strings, key: SymKey) -> Result<u32, String> {
        if let SymKind::Global(id) = key.kind() {
            if self.global_rows.is_empty() {
                self.global_rows = vec![NONE; self.inputs.symbols.len() as usize];
            }
            match self.global_rows.get(id.0 as usize) {
                Some(&NONE) => {}
                Some(&row) => return Ok(row),
                None => return Err("a global out of range".into()),
            }
        } else if let Some(&row) = self.index.get(&key) {
            return Ok(row);
        }
        let row = match key.kind() {
            SymKind::Global(id) => {
                let k = self.inputs.symbols.get(id).ok_or("a global out of range")?;
                if self.first.is_empty() {
                    self.first = first_mentions(self.inputs);
                }
                let (at_file, at_index) =
                    self.first.get(id.0 as usize).copied().unwrap_or((NONE, 0));
                SymbolRow {
                    at_file,
                    at_index,
                    kind: if k.version.is_some() { sym_kind::VERSIONED } else { sym_kind::GLOBAL },
                    file: NONE,
                    ordinal: 0,
                    name: strings.add(k.name),
                    version: k.version.map_or(Str::default(), |v| strings.add(v)),
                }
            }
            SymKind::Local(sym) => {
                let l = self.keys.identities.local(sym).ok_or("a local out of range")?;
                SymbolRow {
                    at_file: sym.file.0,
                    at_index: sym.index.0,
                    kind: sym_kind::LOCAL,
                    file: sym.file.0,
                    ordinal: l.ordinal,
                    name: strings.add(l.name),
                    version: Str::default(),
                }
            }
        };
        let at = self.rows.len() as u32;
        self.rows.push(row);
        match key.kind() {
            SymKind::Global(id) => self.global_rows[id.0 as usize] = at,
            SymKind::Local(_) => {
                self.index.insert(key, at);
            }
        }
        Ok(at)
    }
}

/// The rows of a state that a link's tables give: `FILES`, `PROBES`,
/// `OUTSECS`, `SYMBOLS`, `SECTIONS` and the decision tables. `file_hash`
/// is each file's content hash (`None`: none could be computed). The
/// caller fills `META`, `STAT`, `LEAVES` and the pool. An `Err` names why
/// no state can be written (dynamic tables, `--icf`'s folds).
pub fn capture(
    link: &Link<'_, '_>,
    keys: &Keys<'_>,
    layout: &Layout,
    strings: &mut Strings,
    file_hash: &dyn Fn(FileId) -> Option<[u8; 16]>,
    arch: &dyn mortar_arch::Arch,
) -> Result<State, String> {
    let pool = Pool::sequential();
    capture_timed(link, keys, layout, strings, file_hash, arch, &pool, &mut Laps::new())
}

/// [`capture`], with its parts timed into `laps` (WP-094).
#[allow(clippy::too_many_lines, clippy::too_many_arguments)] // one block per table, in IN-18a's order
pub fn capture_timed(
    link: &Link<'_, '_>,
    keys: &Keys<'_>,
    layout: &Layout,
    strings: &mut Strings,
    file_hash: &dyn Fn(FileId) -> Option<[u8; 16]>,
    arch: &dyn mortar_arch::Arch,
    pool: &Pool,
    laps: &mut Laps,
) -> Result<State, String> {
    let mut clock = Clock { laps, at: std::time::Instant::now() };
    let slots = &layout.slots;
    let p = &layout.placement;
    if layout.folds.folded.is_some()
        || !slots.dynsym.is_empty()
        || !slots.dyn_relocs.is_empty()
        || !slots.needed.is_empty()
        || !slots.verneed.is_empty()
        || !slots.copies.is_empty()
        || !slots.copy_of.is_empty()
    {
        return Err("the layout has folds or dynamic tables, which format 1 does not store".into());
    }
    let inputs = link.inputs;
    let mut s = State::default();
    for (id, f) in inputs.files.iter() {
        let identity = keys.identities.file(id).ok_or("a file without identity")?;
        let (member_name, member_ordinal) = identity.member.unwrap_or_default();
        let (base, len) = f.bytes.map_or((0, 0), |b| (b.base, b.len));
        s.files.push(FileRow {
            // IN-28a: a full link captures every file's position rows
            // from the bytes it hashes, in this one write.
            flags: crate::format::file_flags::POSITIONAL,
            kind: match f.reader {
                Reader::Internal => 0,
                Reader::Object(_) => 1,
                Reader::Shared(_) => 2,
            },
            member: u8::from(identity.member.is_some()),
            path: strings.add(identity.path),
            mention: identity.mention,
            member_name: strings.add(member_name),
            member_ordinal,
            base,
            len,
            hash: if f.bytes.is_some() { file_hash(id).ok_or("a file's hash")? } else { [0; 16] },
        });
    }
    clock.lap("capture files");
    for probe in inputs.probe_checks().ok_or("a probe failed")? {
        s.probes.push(ProbeRow { path: strings.add(probe.path), present: u8::from(probe.present) });
    }
    for id in link.assignment.outsecs.ids().iter() {
        let o =
            mortar_core::identity::outsec_identity(&keys.outsecs, id).ok_or("an output section")?;
        s.outsecs.push(OutSecRow { name: strings.add(o.name), ordinal: o.ordinal });
    }
    let chunk_ord = chunk_ordinals(p, link.assignment);
    clock.lap("capture probes, outsecs");
    for (id, row) in link.resolution.sections.iter() {
        let _ = row;
        let si =
            keys.identities.section(link.resolution, id).ok_or("a section without identity")?;
        let (file, index) = link.resolution.sh_index(id).ok_or("a section without index")?;
        let (anchor_kind, anchor) = anchor_row(strings, si.key.anchor);
        let mut row = SectionRow {
            file: file.0,
            anchor_kind,
            anchor,
            name: strings.add(si.key.name),
            ordinal: si.ordinal,
            place_chunk: NONE,
            ..SectionRow::default()
        };
        (row.sh_type, row.sh_flags, row.size, row.align, row.offset) =
            header_fields(inputs, file, index);
        if let Some((chunk, offset)) = p.place.get(id).and_then(|pl| pl.get()) {
            row.place_chunk = chunk.0;
            row.place_offset = offset;
        }
        s.sections.push(row);
    }
    clock.lap("capture sections");
    for (id, c) in p.chunks.iter() {
        let (kind, name) = chunk_ord.key(id.0).ok_or("a chunk")?;
        let ordinal = chunk_ord.ordinal(id.0).ok_or("a chunk")?;
        let (out, part, dep) = match c.kind {
            ChunkKind::Body(pt) | ChunkKind::Patches(pt) => (pt.out.0, pt.part, false),
            ChunkKind::Merged(o) | ChunkKind::Group(o) | ChunkKind::Relocs(o) => (o.0, 0, false),
            ChunkKind::Synthetic(sy) => (NONE, 0, sy.address_dependent),
            _ => (NONE, 0, false),
        };
        let mut bits = 0;
        if c.addr.is_some() {
            bits |= chunk_bits::HAS_ADDR;
        }
        if c.shndx.is_some() {
            bits |= chunk_bits::HAS_SHNDX;
        }
        if c.relro {
            bits |= chunk_bits::RELRO;
        }
        if dep {
            bits |= chunk_bits::ADDRESS_DEPENDENT;
        }
        s.chunks.push(ChunkRow {
            tag: tag_byte(kind),
            bits,
            align: c.align,
            name: name.map_or(Str::default(), |n| strings.add(n)),
            ordinal,
            out,
            part,
            sh_type: c.sh_type,
            sh_flags: c.flags,
            size: c.size,
            capacity: c.capacity,
            addr: c.addr.unwrap_or(0),
            offset: c.offset,
            shndx: c.shndx.unwrap_or(0),
        });
    }
    s.order = p.order.iter().map(|c| c.0).collect();
    s.segments = p
        .segments
        .as_slice()
        .iter()
        .map(|g| SegmentRow {
            p_type: g.p_type,
            p_flags: g.flags,
            offset: g.offset,
            addr: g.addr,
            filesz: g.filesz,
            memsz: g.memsz,
            align: u64::from(g.align),
        })
        .collect();
    clock.lap("capture chunks, order, segments");
    // FRAGS: per (output section, hash), the live fragments' offsets;
    // FRAG_AT: each live fragment by where it was read, naming its FRAGS
    // row (IN-18a4).
    let mut live: Vec<(u32, [u8; 16], u32, u64)> = Vec::new();
    let mut buf = Vec::new();
    for (frag, &off) in layout.merge.frag_off.iter() {
        if off == MergeMap::NONE {
            continue;
        }
        let f = link.split.frags.get(frag).ok_or("a fragment")?;
        let out = link.assignment.out(f.section).ok_or("a live fragment without output section")?;
        let hash = frag_hash(link, frag, &mut buf).ok_or("a fragment's bytes")?;
        live.push((out.0, hash, frag.0, off));
    }
    live.sort_unstable();
    let mut at_row = |frag: u32, row: usize| -> Result<(), String> {
        let f = link.split.frags.get(FragId::from_index(frag)).ok_or("a fragment")?;
        // `SECTIONS` has a row per `SectionId`, in order.
        s.frag_at.push(crate::format::FragAtRow {
            section: f.section.0,
            offset: f.offset,
            frag: row as u64,
        });
        Ok(())
    };
    let mut frags = Vec::new();
    let mut i = 0;
    while i < live.len() {
        let mut j = i + 1;
        while j < live.len() && (live[j].0, live[j].1) == (live[i].0, live[i].1) {
            j += 1;
        }
        let group = &live[i..j];
        if group.iter().all(|g| g.3 == group[0].3) {
            for g in group {
                at_row(g.2, frags.len())?;
            }
            frags.push(FragRow {
                out: group[0].0,
                ordinal: NONE,
                hash: group[0].1,
                offset: group[0].3,
            });
        } else {
            // The ranks are by `FragId`, as the replay by identity ranks.
            for (n, g) in group.iter().enumerate() {
                at_row(g.2, frags.len())?;
                frags.push(FragRow { out: g.0, ordinal: n as u32, hash: g.1, offset: g.3 });
            }
        }
        i = j;
    }
    s.frags = frags;
    s.frag_at.sort_unstable();
    clock.lap("capture frags");
    let mut syms = Symbols {
        keys,
        inputs,
        rows: Vec::new(),
        index: HashMap::default(),
        first: Vec::new(),
        global_rows: Vec::new(),
    };
    for (k, kind) in SLOT_LISTS.iter().enumerate() {
        for &key in slot_list(slots, *kind) {
            s.slots.push(SlotRow { kind: k as u8, symbol: syms.row(strings, key)? });
        }
    }
    s.eh_records =
        slots.eh_records.iter().map(|e| EhRow { section: e.section.0, piece: e.piece }).collect();
    s.symtab_locals = slots
        .symtab_locals
        .as_slice()
        .iter()
        .map(|sp| SpanRow { start: sp.start, len: sp.len })
        .collect();
    s.sizes = slots.sizes.iter().map(|&(c, size)| SizeRow { chunk: c.0, size }).collect();
    for sp in &p.sym_place {
        let AllocKey::Common(id) = sp.key else {
            return Err("a copy allocation, which format 1 does not store".into());
        };
        s.sym_place.push(SymPlaceRow {
            kind: 0,
            symbol: syms.row(strings, SymKey::global(id))?,
            chunk: sp.chunk.0,
            offset: sp.offset,
            size: sp.size,
            align: sp.align,
        });
    }
    for d in &p.defined {
        let symbol = syms.row(strings, SymKey::global(d.symbol))?;
        s.defined.push(match d.value {
            DefValue::Absolute(v) => DefinedRow { symbol, kind: 0, chunk: NONE, value: v },
            DefValue::At { chunk, offset } => {
                DefinedRow { symbol, kind: 1, chunk: chunk.0, value: offset }
            }
        });
    }
    for patch in &p.patches {
        let mut row = PatchRow {
            chunk: patch.chunk.0,
            offset: patch.offset,
            symbol: NONE,
            section: NONE,
            ..PatchRow::default()
        };
        match patch.kind {
            PatchKind::Thunk(ThunkTarget::Sym { key, addend }) => {
                row.kind = patch_kind::THUNK;
                row.symbol = syms.row(strings, key)?;
                row.value = addend as u64;
            }
            PatchKind::Thunk(ThunkTarget::Plt(key)) => {
                row.kind = patch_kind::THUNK_PLT;
                row.symbol = syms.row(strings, key)?;
            }
            PatchKind::Erratum { erratum, section, offset } => {
                if erratum != Erratum::E843419 {
                    return Err("an erratum format 1 does not name".into());
                }
                row.kind = patch_kind::E843419;
                row.section = section.0;
                row.value = offset;
            }
        }
        s.patches.push(row);
    }
    s.redirects = p
        .redirects
        .iter()
        .map(|r| RedirectRow { section: r.section.0, patch: r.patch, offset: r.offset })
        .collect();
    s.sized = p.sized.iter().map(|z| SizeRow { chunk: z.chunk.0, size: z.size }).collect();
    s.holes = p
        .holes
        .iter()
        .map(|h| HoleRow {
            chunk: h.chunk.0,
            kind: hole_byte(h.kind),
            offset: h.offset,
            size: h.size,
        })
        .collect();
    let mut bits = 0;
    if let Some(tls) = p.tls {
        bits |= layout_bits::TLS;
        s.layout.tls_start = tls.start;
        s.layout.tls_size = tls.size;
        s.layout.tls_align = tls.align;
    }
    if let Some(entry) = p.entry {
        bits |= layout_bits::ENTRY;
        s.layout.entry = entry;
    }
    if slots.tls_ld {
        bits |= layout_bits::TLS_LD;
    }
    s.layout.bits = bits;
    clock.lap("capture slots to holes");
    let mut sub = Laps::new();
    crate::tables::fill(link, &mut syms, strings, &mut s, arch, pool, &mut sub)?;
    clock.lap("capture delta tables");
    clock.laps.extend(sub);
    s.symbols = syms.rows;
    if !strings.fits() {
        return Err("the string pool exceeds 4 GiB".into());
    }
    Ok(s)
}

// ---- replay (IN-61) ------------------------------------------------------------

/// The present link's id of each state row of a table.
struct Map {
    files: Vec<Option<FileId>>,
    sections: Vec<Option<SectionId>>,
    chunks: Vec<ChunkId>,
    symbols: Vec<Option<SymKey>>,
    outsecs: Vec<Option<OutSecId>>,
}

fn file_identity<'a>(s: &'a State, row: &FileRow) -> FileIdentity<'a> {
    FileIdentity {
        path: s.str(row.path),
        mention: row.mention,
        member: (row.member == 1).then(|| (s.str(row.member_name), row.member_ordinal)),
    }
}

fn anchor_of(s: &State, kind: u8, name: Str) -> Anchor<'_> {
    match kind {
        1 => Anchor::Comdat(s.str(name)),
        2 => Anchor::Global(s.str(name)),
        3 => Anchor::Local(s.str(name)),
        _ => Anchor::None,
    }
}

/// WP-094: the parts of a replay, timed (name, ms).
pub type Laps = Vec<(&'static str, f64)>;

struct Clock<'l> {
    laps: &'l mut Laps,
    at: std::time::Instant,
}

impl Clock<'_> {
    fn lap(&mut self, name: &'static str) {
        let now = std::time::Instant::now();
        self.laps.push((name, (now - self.at).as_secs_f64() * 1e3));
        self.at = now;
    }
}

/// IN-28a: the positional set of an update, decided from `FILES`, the
/// new link's files and what change detection found, before any identity
/// is built ([`positional_set`]).
#[derive(Debug, Clone)]
pub struct Positional {
    /// Per `FILES` row: the present file it maps to by file identity.
    files: Vec<Option<FileId>>,
    /// Per `FILES` row: in the set.
    set: Vec<bool>,
    /// Per `FILES` row: its first `SECTIONS` row and its row count.
    first: Vec<(u32, u32)>,
    /// Per present `FileId`: its `FILES` row, `NONE` if none.
    row_of: Vec<u32>,
}

impl Positional {
    /// Whether the present file `f` is in the set.
    #[must_use]
    pub fn covers(&self, f: FileId) -> bool {
        let row = self.row_of.get(f.0 as usize).copied().unwrap_or(NONE);
        row != NONE && self.set.get(row as usize).copied().unwrap_or(false)
    }

    /// The number of `FILES` rows in the set.
    #[must_use]
    pub fn len(&self) -> usize {
        self.set.iter().filter(|&&b| b).count()
    }

    /// Whether the set is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The `FILES` row of the present file `f`, if any.
    #[must_use]
    pub fn row_of(&self, f: FileId) -> Option<u32> {
        self.row_of.get(f.0 as usize).copied().filter(|&r| r != NONE)
    }

    /// The `SECTIONS` range (first row, count) of `FILES` row `row`.
    #[must_use]
    pub fn sections_of(&self, row: u32) -> Option<(u32, u32)> {
        self.first.get(row as usize).copied().filter(|&(f, _)| f != NONE)
    }

    /// The present file of a `FILES` row in the set.
    fn file(&self, row: u32) -> Option<FileId> {
        if *self.set.get(row as usize)? {
            self.files.get(row as usize).copied().flatten()
        } else {
            None
        }
    }
}

/// IN-28a's positional set: a `FILES` row is in it iff (1) its
/// `POSITIONAL` bit is set; (2) the file it maps to by file identity is
/// `unchanged` (IN-22, as the caller judged it; the caller also owes that
/// the update uses no other bytes of it); (3) the new entry's kind, base
/// and length equal the row's; (4) its `SECTIONS` rows are one contiguous
/// range of exactly the file's section count (zero only for a file with
/// none), and (WP-064a) each row's stored header (`sh_type`, `sh_flags`,
/// size, alignment, file offset) is that of the section at its position
/// in the new link's tables. Decided from the state and the new link's
/// file and section tables only: no identity is built and no input byte
/// beyond the parsed headers is read.
pub fn positional_set(
    link: &Link<'_, '_>,
    s: &State,
    unchanged: &dyn Fn(FileId) -> bool,
) -> Positional {
    let inputs = link.inputs;
    let identities = inputs.file_identities();
    let index = Lookup::new(identities.ids().iter(), |f| identities[f]);
    let files: Vec<Option<FileId>> =
        s.files.iter().map(|row| index.find(&file_identity(s, row), |f| identities[f])).collect();
    let mut row_of = vec![NONE; inputs.files.len() as usize];
    let mut set = vec![false; s.files.len()];
    for (r, f) in files.iter().enumerate() {
        let Some(f) = *f else { continue };
        row_of[f.slot()] = r as u32;
        let row = &s.files[r];
        let new = &inputs.files[f];
        let kind = match new.reader {
            Reader::Internal => 0,
            Reader::Object(_) => 1,
            Reader::Shared(_) => 2,
        };
        let (base, len) = new.bytes.map_or((0, 0), |b| (b.base, b.len));
        set[r] = row.flags & crate::format::file_flags::POSITIONAL != 0
            && unchanged(f)
            && row.kind == kind
            && (row.base, row.len) == (base, len);
    }
    // (4): one contiguous range per file, of the file's section count.
    let mut first = vec![(NONE, 0u32); s.files.len()];
    let mut split = vec![false; s.files.len()];
    let mut prev = NONE;
    for (i, x) in s.sections.iter().enumerate() {
        let Some(e) = first.get_mut(x.file as usize) else { continue };
        if e.0 == NONE {
            *e = (i as u32, 0);
        } else if prev != x.file {
            split[x.file as usize] = true;
        }
        e.1 += 1;
        prev = x.file;
    }
    for (r, in_set) in set.iter_mut().enumerate() {
        if !*in_set {
            continue;
        }
        let Some(f) = files[r] else { continue };
        let (start, count) = first[r];
        let n = link.resolution.section_base[f].len();
        if split[r] || count != n {
            *in_set = false;
            continue;
        }
        if count == 0 {
            continue;
        }
        // Each row describes the section at its position (the stale and
        // the swapped row of the review's H1 differ in offset or size).
        *in_set = (0..count).all(|k| {
            let x = &s.sections[(start + k) as usize];
            header_fields(inputs, f, SectionIndex(k))
                == (x.sh_type, x.sh_flags, x.size, x.align, x.offset)
        });
    }
    Positional { files, set, first, row_of }
}

/// What a replay translated, for the writer that recaptures a file's
/// position rows (IN-28a, the writer's duty): the present id of each
/// `SECTIONS` and `SYMBOLS` row, and the `FRAGS` row each fragment
/// outside the positional set was found at.
#[derive(Debug, Clone, Default)]
pub struct Translation {
    /// Per `FILES` row: its present file.
    pub files: Vec<Option<FileId>>,
    /// Per `SECTIONS` row: its present section.
    pub sections: Vec<Option<SectionId>>,
    /// Per `SYMBOLS` row: its present key.
    pub symbols: Vec<Option<SymKey>>,
    /// Per `OUTSECS` row: its present output section.
    pub outsecs: Vec<Option<OutSecId>>,
    /// The `SYMBOLS` rows translated by identity because their file (a
    /// local's) or first mention (a global's) is outside the set; every
    /// row when the replay is by identity.
    pub by_identity: Vec<u32>,
    /// Each live fragment found by its hash (outside the set, or every
    /// fragment when the replay by identity decided), and its `FRAGS` row.
    pub frag_rows: Vec<(FragId, u32)>,
}

/// IN-28a, the writer's duty: a writer that writes `file`'s new hash
/// into its `FILES` row `row` recaptures the file's position rows from
/// the bytes it hashed (the new link's tables, through the replay's
/// translation `tr`), in the same write, or clears the row's
/// `POSITIONAL` bit. Recaptured: its `SECTIONS` rows' headers (when the
/// rows already name its sections in index order), the `at_index` of its
/// locals and of the globals it first mentions, and its `FRAG_AT` rows
/// (each fragment with the `FRAGS` row the replay found it at). Cleared
/// otherwise (a section permutation: the rows stay in identity order, and
/// the next update translates the file by identity). The cost is that of
/// the rows translated by identity. `true`: recaptured.
pub fn recapture(
    s: &mut State,
    link: &Link<'_, '_>,
    pos: &Positional,
    tr: &Translation,
    file: FileId,
    row: u32,
) -> bool {
    let done = recapture_rows(s, link, pos, tr, file, row).is_some();
    if let Some(r) = s.files.get_mut(row as usize) {
        if done {
            r.flags |= crate::format::file_flags::POSITIONAL;
        } else {
            r.flags &= !crate::format::file_flags::POSITIONAL;
        }
    }
    done
}

fn recapture_rows(
    s: &mut State,
    link: &Link<'_, '_>,
    pos: &Positional,
    tr: &Translation,
    file: FileId,
    row: u32,
) -> Option<()> {
    let inputs = link.inputs;
    let res = link.resolution;
    let (first, count) = pos.sections_of(row).unwrap_or((0, 0));
    if count != res.section_base.get(file)?.len() {
        return None;
    }
    // Every row is decided first; the state changes only when all are.
    for k in 0..count {
        let id = tr.sections.get((first + k) as usize).copied().flatten()?;
        if res.sh_index(id) != Some((file, SectionIndex(k))) {
            return None;
        }
    }
    let headers: Vec<_> =
        (0..count).map(|k| header_fields(inputs, file, SectionIndex(k))).collect();
    let mut first_at: HashMap<SymbolId, u32> = HashMap::default();
    for (i, &id) in inputs.sym_map_of(file).iter().enumerate() {
        first_at.entry(id).or_insert(i as u32);
    }
    let mut at_index = Vec::new();
    for &r in &tr.by_identity {
        let y = s.symbols.get(r as usize)?;
        let key = tr.symbols.get(r as usize).copied().flatten();
        if y.kind == sym_kind::LOCAL {
            if y.file != row {
                continue;
            }
            match key?.kind() {
                SymKind::Local(e) if e.file == file => at_index.push((r, e.index.0)),
                _ => return None,
            }
        } else if y.at_file == row {
            at_index.push((r, *first_at.get(&key?.as_global()?)?));
        }
    }
    let mut frag_at = Vec::new();
    for &(frag, fr) in &tr.frag_rows {
        let f = link.split.frags.get(frag)?;
        if let Some((x, index)) = res.sh_index(f.section)
            && x == file
        {
            frag_at.push(crate::format::FragAtRow {
                section: first + index.0,
                offset: f.offset,
                frag: u64::from(fr),
            });
        }
    }
    frag_at.sort_unstable();
    for (k, h) in headers.into_iter().enumerate() {
        let x = &mut s.sections[first as usize + k];
        (x.sh_type, x.sh_flags, x.size, x.align, x.offset) = h;
    }
    for (r, i) in at_index {
        s.symbols[r as usize].at_index = i;
    }
    let lo = s.frag_at.partition_point(|r| r.section < first);
    let hi = s.frag_at.partition_point(|r| r.section < first + count);
    s.frag_at.splice(lo..hi, frag_at);
    Some(())
}

#[allow(clippy::too_many_lines)] // one block per table
fn map(
    link: &Link<'_, '_>,
    keys: &Keys<'_>,
    s: &State,
    placement: &Placement,
    clock: &mut Clock<'_>,
    pos: Option<&Positional>,
) -> Result<(Map, Vec<u32>), String> {
    let inputs = link.inputs;
    let files: Vec<Option<FileId>> = match pos {
        Some(p) => p.files.clone(),
        None => s.files.iter().map(|f| keys.identities.file_id(&file_identity(s, f))).collect(),
    };
    clock.lap("map files");
    let sections = s
        .sections
        .iter()
        .enumerate()
        .map(|(i, x)| {
            if let Some(p) = pos
                && let Some(f) = p.file(x.file)
            {
                let index = i as u32 - p.first[x.file as usize].0;
                return link.resolution.section_id(f, SectionIndex(index));
            }
            let file = s.files.get(x.file as usize)?;
            let identity = SectionIdentity {
                file: file_identity(s, file),
                key: SectionKey {
                    anchor: anchor_of(s, x.anchor_kind, x.anchor),
                    name: s.str(x.name),
                },
                ordinal: x.ordinal,
            };
            keys.identities.section_id(link.resolution, &identity)
        })
        .collect();
    clock.lap("map sections");
    let outsecs: Vec<Option<OutSecId>> = s
        .outsecs
        .iter()
        .map(|o| {
            outsec_id(
                &keys.outsecs,
                &mortar_core::identity::OutSecIdentity { name: s.str(o.name), ordinal: o.ordinal },
            )
        })
        .collect();
    let ord = chunk_ordinals(placement, link.assignment);
    let mut chunks = Vec::with_capacity(s.chunks.len());
    for (i, c) in s.chunks.iter().enumerate() {
        let tag = *TAGS.get(c.tag as usize).ok_or("a chunk tag")?;
        let name = (tag == ChunkTag::Synthetic || c.out != NONE).then(|| s.str(c.name));
        let identity = ChunkIdentity { kind: tag, name, ordinal: c.ordinal };
        let id = chunk_id(&ord, &identity)
            .ok_or_else(|| format!("CHUNKS row {i}: no such chunk in this link"))?;
        let present = placement.chunks.get(id).ok_or("a chunk")?;
        let (out, part, dep) = match present.kind {
            ChunkKind::Body(pt) | ChunkKind::Patches(pt) => (Some(pt.out), pt.part, false),
            ChunkKind::Merged(o) | ChunkKind::Group(o) | ChunkKind::Relocs(o) => {
                (Some(o), 0, false)
            }
            ChunkKind::Synthetic(sy) => (None, 0, sy.address_dependent),
            _ => (None, 0, false),
        };
        let want_out =
            if c.out == NONE { None } else { outsecs.get(c.out as usize).copied().flatten() };
        if out != want_out || part != c.part || dep != (c.bits & chunk_bits::ADDRESS_DEPENDENT != 0)
        {
            return Err(format!("CHUNKS row {i}: not this link's chunk of that identity"));
        }
        chunks.push(id);
    }
    clock.lap("map outsecs and chunks");
    // A key the partial lookup does not hold (a key the changed files no
    // longer mention): the whole lookup, made once, on the first miss.
    let mut whole: Option<Lookup<SymbolId>> = None;
    let mut misses = 0u32;
    let mut symbols = Vec::with_capacity(s.symbols.len());
    let mut by_identity = Vec::new();
    // IN-28a's local bound: each positional file's count of locals
    // (`sh_info`), read from its parsed symbol table header once.
    let mut locals: Vec<u32> = Vec::new();
    for (row, y) in s.symbols.iter().enumerate() {
        let key = match y.kind {
            sym_kind::LOCAL => {
                if let Some(p) = pos
                    && let Some(f) = p.file(y.file)
                {
                    if locals.is_empty() {
                        locals = vec![NONE; inputs.files.len() as usize];
                    }
                    let n = &mut locals[f.slot()];
                    if *n == NONE {
                        *n = match inputs.files[f].reader {
                            Reader::Object(o) => {
                                o.symbols().ok().flatten().map_or(0, |t| t.first_global().0)
                            }
                            _ => 0,
                        };
                    }
                    if y.at_index >= *n {
                        return Err(format!(
                            "state-corrupt: SYMBOLS row {row}: a local's position {} is not below its file's {} locals",
                            y.at_index, *n
                        ));
                    }
                    symbols.push(Some(SymKey::local(mortar_core::id::SymEntry {
                        file: f,
                        index: mortar_elf::SymbolIndex(y.at_index),
                    })));
                    continue;
                }
                by_identity.push(row as u32);
                let file = s.files.get(y.file as usize);
                file.and_then(|file| {
                    let l = LocalIdentity {
                        file: file_identity(s, file),
                        name: s.str(y.name),
                        ordinal: y.ordinal,
                    };
                    keys.identities.local_sym(&l).map(SymKey::local)
                })
            }
            k => {
                let version = (k == sym_kind::VERSIONED).then(|| s.str(y.version));
                let (name, version) = (s.str(y.name), version);
                if let Some(p) = pos
                    && let Some(f) = p.file(y.at_file)
                    && let Some(&id) = inputs.sym_map_of(f).get(y.at_index as usize)
                    && inputs
                        .symbols
                        .get(id)
                        .is_some_and(|k| k.name == name && k.version == version)
                {
                    symbols.push(Some(SymKey::global(id)));
                    continue;
                }
                by_identity.push(row as u32);
                let g = SymbolIdentity { name, version };
                let found = symbol_id(&inputs.symbols, &keys.symbols, &g);
                if found.is_none() && !keys.complete {
                    misses += 1;
                    let all = whole.get_or_insert_with(|| symbol_lookup(&inputs.symbols));
                    symbol_id(&inputs.symbols, all, &g)
                } else {
                    found
                }
                .map(SymKey::global)
            }
        };
        symbols.push(key);
    }
    let _ = misses;
    clock.lap(if whole.is_some() { "map symbols (whole lookup)" } else { "map symbols" });
    Ok((Map { files, sections, chunks, symbols, outsecs }, by_identity))
}

/// The merged map by identity (IN-18a `FRAGS`): every live fragment
/// hashed, and ranked among those of its identity.
/// The `FRAGS` rows of each (output section, hash), with their row index.
type FragGroups<'s> = HashMap<(OutSecId, [u8; 16]), Vec<(u32, &'s FragRow)>>;

fn frags_by_identity(
    link: &Link<'_, '_>,
    s: &State,
    m: &Map,
    frag_off: &mut Column<FragId, u64>,
    found: &mut Vec<(FragId, u32)>,
) -> Result<(), String> {
    let mut by_identity: FragGroups<'_> = HashMap::default();
    for (i, f) in s.frags.iter().enumerate() {
        let out = get(&m.outsecs, f.out, "OUTSECS")?;
        by_identity.entry((out, f.hash)).or_default().push((i as u32, f));
    }
    let mut buf = Vec::new();
    let mut rank: HashMap<(OutSecId, [u8; 16]), u32> = HashMap::default();
    for (i, off) in frag_off.as_mut_slice().iter_mut().enumerate() {
        let frag = FragId::from_index(i as u32);
        if *off == MergeMap::NONE {
            continue;
        }
        let f = link.split.frags.get(frag).ok_or("a fragment")?;
        let out = link.assignment.out(f.section).ok_or("a fragment's output section")?;
        let hash = frag_hash(link, frag, &mut buf).ok_or("a fragment's bytes")?;
        let rows = by_identity.get(&(out, hash)).ok_or("a live fragment has no FRAGS row")?;
        let n = rank.entry((out, hash)).or_insert(0);
        let (at, row) = if rows.len() == 1 && rows[0].1.ordinal == NONE {
            rows[0]
        } else {
            rows.iter()
                .find(|r| r.1.ordinal == *n)
                .copied()
                .ok_or("a live fragment's ordinal has no row")?
        };
        *n += 1;
        *off = row.offset;
        found.push((frag, at));
    }
    Ok(())
}

fn get<T: Copy>(v: &[Option<T>], i: u32, what: &str) -> Result<T, String> {
    v.get(i as usize).copied().flatten().ok_or_else(|| format!("{what} row {i} does not translate"))
}

/// The decision tables of a state, translated to this link's ids (IN-61):
/// what the link writes in place of its own `computed` layout. Every row
/// must translate and every chunk of this link must be named once; a
/// failure is named. `Folds` and the dynamic tables are `computed`'s
/// (format 1 stores none).
pub fn replay(
    link: &Link<'_, '_>,
    keys: &Keys<'_>,
    s: &State,
    computed: &Layout,
) -> Result<Layout, String> {
    replay_timed(link, keys, s, computed, &mut Laps::new())
}

/// [`replay`], with its parts timed into `laps` (WP-094).
pub fn replay_timed(
    link: &Link<'_, '_>,
    keys: &Keys<'_>,
    s: &State,
    computed: &Layout,
    laps: &mut Laps,
) -> Result<Layout, String> {
    replay_inner(link, keys, s, computed, laps, None).map(|r| r.0)
}

/// [`replay_timed`] with what it translated (for a writer's recapture).
pub fn replay_translated(
    link: &Link<'_, '_>,
    keys: &Keys<'_>,
    s: &State,
    computed: &Layout,
    laps: &mut Laps,
) -> Result<(Layout, Translation), String> {
    replay_inner(link, keys, s, computed, laps, None)
}

/// IN-28a: [`replay`] with the rows of the files in the positional set
/// `pos` ([`positional_set`]) translated by position (section index,
/// symbol index, fragment position) and every other row by identity.
/// `keys` must hold every file outside the set ([`Keys::partial`] with
/// `!pos.covers(f)`). An accepted position names the row identity would
/// find: a local's index is below its file's local count, a global's key
/// has the row's name and version, and a fragment's `FRAGS` row has no
/// ordinal and the fragment's output section (else every fragment is
/// found by identity).
pub fn replay_by_position(
    link: &Link<'_, '_>,
    keys: &Keys<'_>,
    s: &State,
    computed: &Layout,
    pos: &Positional,
    laps: &mut Laps,
) -> Result<(Layout, Translation), String> {
    replay_inner(link, keys, s, computed, laps, Some(pos))
}

#[allow(clippy::too_many_lines, clippy::many_single_char_names)] // one block per table
fn replay_inner(
    link: &Link<'_, '_>,
    keys: &Keys<'_>,
    s: &State,
    computed: &Layout,
    laps: &mut Laps,
    pos: Option<&Positional>,
) -> Result<(Layout, Translation), String> {
    let mut clock = Clock { laps, at: std::time::Instant::now() };
    let cp = &computed.placement;
    let (m, by_identity) = map(link, keys, s, cp, &mut clock, pos)?;
    let mut chunks: Vec<Option<Chunk>> = vec![None; cp.chunks.len() as usize];
    for (row, (c, &id)) in s.chunks.iter().zip(&m.chunks).enumerate() {
        let slot = &mut chunks[id.slot()];
        if slot.is_some() {
            return Err(format!("CHUNKS row {row}: a chunk named twice"));
        }
        *slot = Some(Chunk {
            kind: cp.chunks[id].kind,
            sh_type: c.sh_type,
            flags: c.sh_flags,
            align: c.align,
            size: c.size,
            capacity: c.capacity,
            addr: (c.bits & chunk_bits::HAS_ADDR != 0).then_some(c.addr),
            offset: c.offset,
            shndx: (c.bits & chunk_bits::HAS_SHNDX != 0).then_some(c.shndx),
            relro: c.bits & chunk_bits::RELRO != 0,
        });
    }
    let chunks: Vec<Chunk> =
        chunks.into_iter().collect::<Option<_>>().ok_or("a chunk of this link has no row")?;
    let chunk = |i: u32| m.chunks.get(i as usize).copied().ok_or_else(|| format!("chunk row {i}"));
    let mut place = vec![Place::NONE; cp.place.len() as usize];
    for (i, x) in s.sections.iter().enumerate() {
        if x.place_chunk == NONE {
            continue;
        }
        let id = get(&m.sections, i as u32, "SECTIONS")?;
        place[id.slot()] = Place::at(chunk(x.place_chunk)?, x.place_offset);
    }
    let mut sym_place = Vec::with_capacity(s.sym_place.len());
    for r in &s.sym_place {
        let key = get(&m.symbols, r.symbol, "SYMBOLS")?;
        let id = key.as_global().ok_or("a common allocation of a local")?;
        sym_place.push(SymPlace {
            key: AllocKey::Common(id),
            chunk: chunk(r.chunk)?,
            offset: r.offset,
            size: r.size,
            align: r.align,
        });
    }
    sym_place.sort_by_key(|r| r.key);
    let mut defined = Vec::with_capacity(s.defined.len());
    for d in &s.defined {
        let symbol =
            get(&m.symbols, d.symbol, "SYMBOLS")?.as_global().ok_or("a definition of a local")?;
        let value = if d.kind == 0 {
            DefValue::Absolute(d.value)
        } else {
            DefValue::At { chunk: chunk(d.chunk)?, offset: d.value }
        };
        defined.push(Defined { symbol, value });
    }
    defined.sort_by_key(|d| d.symbol);
    let mut patches = Vec::with_capacity(s.patches.len());
    for (i, r) in s.patches.iter().enumerate() {
        let kind = match r.kind {
            patch_kind::THUNK => PatchKind::Thunk(ThunkTarget::Sym {
                key: get(&m.symbols, r.symbol, "SYMBOLS")?,
                addend: r.value as i64,
            }),
            patch_kind::THUNK_PLT => {
                PatchKind::Thunk(ThunkTarget::Plt(get(&m.symbols, r.symbol, "SYMBOLS")?))
            }
            _ => PatchKind::Erratum {
                erratum: Erratum::E843419,
                section: get(&m.sections, r.section, "SECTIONS")?,
                offset: r.value,
            },
        };
        patches.push((Patch { chunk: chunk(r.chunk)?, offset: r.offset, kind }, i));
    }
    patches.sort_by_key(|(p, _)| (p.chunk, p.offset));
    let mut moved = vec![0u32; patches.len()];
    for (new, (_, old)) in patches.iter().enumerate() {
        moved[*old] = new as u32;
    }
    let mut redirects = Vec::with_capacity(s.redirects.len());
    for r in &s.redirects {
        let patch = *moved.get(r.patch as usize).ok_or("a redirect's patch")?;
        redirects.push(Redirect {
            section: get(&m.sections, r.section, "SECTIONS")?,
            offset: r.offset,
            patch,
        });
    }
    redirects.sort_by_key(|r| (r.section, r.offset));
    let mut sized = Vec::with_capacity(s.sized.len());
    for z in &s.sized {
        sized.push(Sized { chunk: chunk(z.chunk)?, size: z.size });
    }
    let mut holes = Vec::with_capacity(s.holes.len());
    for h in &s.holes {
        holes.push(Hole {
            chunk: chunk(h.chunk)?,
            offset: h.offset,
            size: h.size,
            kind: hole_of(h.kind)?,
        });
    }
    // The state's order is by row; the replay's by the present chunk ids.
    holes.sort_unstable();
    let l = &s.layout;
    let placement = Placement {
        chunks: Column::from_vec(chunks).map_err(|_| "chunks")?,
        order: s.order.iter().map(|&c| chunk(c)).collect::<Result<_, _>>()?,
        segments: Column::from_vec(
            s.segments
                .iter()
                .map(|g| {
                    Ok(Segment {
                        p_type: g.p_type,
                        flags: g.p_flags,
                        offset: g.offset,
                        addr: g.addr,
                        filesz: g.filesz,
                        memsz: g.memsz,
                        align: u8::try_from(g.align).map_err(|_| "a segment's alignment")?,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
        )
        .map_err(|_| "segments")?,
        place: Column::from_vec(place).map_err(|_| "sections")?,
        sym_place: sym_place.into(),
        defined: defined.into(),
        patches: patches.into_iter().map(|(p, _)| p).collect(),
        redirects: redirects.into(),
        sized: sized.into(),
        tls: (l.bits & layout_bits::TLS != 0).then_some(Tls {
            start: l.tls_start,
            size: l.tls_size,
            align: l.tls_align,
        }),
        entry: (l.bits & layout_bits::ENTRY != 0).then_some(l.entry),
        holes: holes.into(),
    };
    clock.lap("placement");
    // The merged map: this link's live fragments take their identity's
    // stored offset; its dead ones stay dead (IN-18a `FRAGS`). By
    // position (IN-28a): a fragment of a file in the set, found in
    // `FRAG_AT` by (section, offset), takes the offset of the `FRAGS` row
    // that row names when it has no ordinal and the fragment's output
    // section; the others are hashed and found by identity. Any other
    // answer sends every fragment to the replay by identity.
    let mut frag_off = computed.merge.frag_off.clone();
    let mut frag_rows: Vec<(FragId, u32)> = Vec::new();
    let mut done = false;
    if let Some(p) = pos {
        let mut out_rows = vec![NONE; link.assignment.outsecs.len() as usize];
        for (r, o) in m.outsecs.iter().enumerate() {
            if let Some(o) = o
                && let Some(x) = out_rows.get_mut(o.0 as usize)
            {
                *x = r as u32;
            }
        }
        let mut buf = Vec::new();
        let mut ok = true;
        // A fragment is one content of a merge class, numbered by its
        // first occurrence (`spec/model.md` §3.5): when its first
        // occurrence is in a file of the set and another occurrence is in
        // a file outside it, the outside file's references mean its bytes,
        // so the fragment is hashed, not taken by position (the review's
        // H2: a forged FRAG_AT row otherwise gives the changed file's
        // reference another string). O(the pieces of the files outside
        // the set).
        let mut outside = vec![false; link.split.frags.len() as usize];
        for r in &*link.split.merge_of {
            let covered = link
                .resolution
                .sh_index(r.section)
                .and_then(|(f, _)| p.row_of.get(f.0 as usize).copied())
                .is_some_and(|row| row != NONE && p.set[row as usize]);
            if covered {
                continue;
            }
            let (start, len) = (r.pieces.start as usize, r.pieces.len as usize);
            for fr in link.split.piece_frag.get(start..start + len).unwrap_or(&[]) {
                if let Some(x) = outside.get_mut(fr.index() as usize) {
                    *x = true;
                }
            }
        }
        // The FRAGS rows taken by position, with their fragment.
        let mut by_position: Vec<(u32, FragId)> = Vec::new();
        for (i, off) in frag_off.as_mut_slice().iter_mut().enumerate() {
            let frag = FragId::from_index(i as u32);
            if *off == MergeMap::NONE {
                continue;
            }
            let f = link.split.frags.get(frag).ok_or("a fragment")?;
            let (file, index) =
                link.resolution.sh_index(f.section).ok_or("a fragment's section")?;
            let out = link.assignment.out(f.section).ok_or("a fragment's output section")?;
            let out_row = out_rows.get(out.0 as usize).copied().unwrap_or(NONE);
            let row = p.row_of.get(file.0 as usize).copied().unwrap_or(NONE);
            if row != NONE && p.set[row as usize] && !outside[i] {
                let section = p.first[row as usize].0 + index.0;
                let want = (section, f.offset);
                let at = s.frag_at.partition_point(|r| (r.section, r.offset) < want);
                if let Some(r) = s.frag_at.get(at)
                    && (r.section, r.offset) == want
                {
                    let fr = u32::try_from(r.frag).map_err(|_| "a FRAG_AT row's FRAGS row")?;
                    match s.frags.get(fr as usize) {
                        Some(x) if x.ordinal == NONE && x.out == out_row && out_row != NONE => {
                            *off = x.offset;
                            by_position.push((fr, frag));
                            continue;
                        }
                        // An identity with ordinals (or a row of another
                        // output section): ranked by identity.
                        _ => {
                            ok = false;
                            break;
                        }
                    }
                }
            }
            if out_row == NONE {
                return Err("a fragment's output section has no OUTSECS row".into());
            }
            let hash = frag_hash(link, frag, &mut buf).ok_or("a fragment's bytes")?;
            let want = (out_row, hash);
            let at = s.frags.partition_point(|r| (r.out, r.hash) < want);
            match s.frags.get(at) {
                Some(r) if (r.out, r.hash) == want && r.ordinal == NONE => {
                    *off = r.offset;
                    frag_rows.push((frag, at as u32));
                }
                // An identity with ordinals ranks the fragments of every
                // file: the replay by identity decides.
                _ => {
                    ok = false;
                    break;
                }
            }
        }
        // A FRAGS row that a hashed fragment also took holds that
        // fragment's bytes: a fragment that took it by position must have
        // them too (one hash each; none for a position-exact state, whose
        // rows name the row identity finds; the review's H2 swapped two).
        if ok && !frag_rows.is_empty() && !by_position.is_empty() {
            let mut hashed: Vec<u32> = frag_rows.iter().map(|r| r.1).collect();
            hashed.sort_unstable();
            for &(fr, frag) in &by_position {
                if hashed.binary_search(&fr).is_ok() {
                    let hash = frag_hash(link, frag, &mut buf).ok_or("a fragment's bytes")?;
                    if s.frags.get(fr as usize).is_none_or(|x| x.hash != hash) {
                        ok = false;
                        break;
                    }
                }
            }
        }
        done = ok;
        if !ok {
            frag_off = computed.merge.frag_off.clone();
            frag_rows.clear();
        }
    }
    if !done {
        frags_by_identity(link, s, &m, &mut frag_off, &mut frag_rows)?;
    }
    clock.lap("fragments");
    let mut slots = computed.slots.clone();
    for (k, kind) in SLOT_LISTS.iter().enumerate() {
        let list: Result<Vec<SymKey>, String> = s
            .slots
            .iter()
            .filter(|r| r.kind as usize == k)
            .map(|r| get(&m.symbols, r.symbol, "SYMBOLS"))
            .collect();
        *slot_list_mut(&mut slots, *kind) = list?.into();
    }
    slots.tls_ld = l.bits & layout_bits::TLS_LD != 0;
    slots.eh_records = s
        .eh_records
        .iter()
        .map(|e| Ok(EhRecord { section: get(&m.sections, e.section, "SECTIONS")?, piece: e.piece }))
        .collect::<Result<_, String>>()?;
    // The image's `.eh_frame` plan is made from this list (S-49), and its
    // symbol table plan is held to these spans (S-88); a list or a span
    // the rules do not admit fails there by name (WP-086). No table (a
    // state without `.symtab`): every span is empty.
    if s.symtab_locals.is_empty() {
        slots.symtab_locals =
            Column::filled(link.inputs.files.len(), mortar_core::id::Span::default());
    } else {
        let mut spans = vec![mortar_core::id::Span::default(); link.inputs.files.len() as usize];
        for (i, sp) in s.symtab_locals.iter().enumerate() {
            let file = get(&m.files, i as u32, "FILES")?;
            spans[file.slot()] = mortar_core::id::Span { start: sp.start, len: sp.len };
        }
        slots.symtab_locals = Column::from_vec(spans).map_err(|_| "files")?;
    }
    slots.sizes =
        s.sizes.iter().map(|z| Ok((chunk(z.chunk)?, z.size))).collect::<Result<_, String>>()?;
    clock.lap("slots");
    let layout =
        Layout { folds: computed.folds.clone(), merge: MergeMap { frag_off }, slots, placement };
    let Map { files, sections, symbols, outsecs, .. } = m;
    Ok((layout, Translation { files, sections, symbols, outsecs, by_identity, frag_rows }))
}

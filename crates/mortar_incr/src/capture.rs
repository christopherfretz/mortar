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
    RedirectRow, SectionFacts, SectionRow, SegmentRow, SizeRow, SlotRow, SpanRow, State, Str,
    Strings, SymPlaceRow, SymbolRow, chunk_bits, hole_kind, layout_bits, patch_kind, sym_kind,
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
pub(crate) const SLOT_LISTS: [SlotKind; 6] = [
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

pub(crate) const TAGS: [ChunkTag; 9] = [
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

pub(crate) fn hole_byte(kind: HoleKind) -> u8 {
    match kind {
        HoleKind::Fill => hole_kind::FILL,
        HoleKind::EhPad => hole_kind::EH_PAD,
        HoleKind::Debug => hole_kind::DEBUG,
        HoleKind::AbbrevTable => hole_kind::ABBREV_TABLE,
        HoleKind::DeadDebug => hole_kind::DEAD_DEBUG,
        HoleKind::DeadFragment => hole_kind::DEAD_FRAGMENT,
        HoleKind::DeadCie => hole_kind::DEAD_CIE,
        HoleKind::DeadSlot => hole_kind::DEAD_SLOT,
    }
}

pub(crate) fn hole_of(b: u8) -> Result<HoleKind, String> {
    Ok(match b {
        hole_kind::FILL => HoleKind::Fill,
        hole_kind::EH_PAD => HoleKind::EhPad,
        hole_kind::DEBUG => HoleKind::Debug,
        hole_kind::ABBREV_TABLE => HoleKind::AbbrevTable,
        hole_kind::DEAD_DEBUG => HoleKind::DeadDebug,
        hole_kind::DEAD_FRAGMENT => HoleKind::DeadFragment,
        hole_kind::DEAD_CIE => HoleKind::DeadCie,
        hole_kind::DEAD_SLOT => HoleKind::DeadSlot,
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
        let comdat = |i: SectionIndex| {
            link.resolution
                .section_id(file, i)
                .and_then(|id| link.resolution.sections.get(id))
                .and_then(|row| row.comdat.get())
                .and_then(|c| inputs.comdats.get(c))
                .map(|c| c.name)
        };
        let (keys, names) = section_keys(&object, &comdat)
            .map_err(|what| format!("{}: {what}", String::from_utf8_lossy(f.path)))?;
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

/// §7.1 for one object: each section's key (anchor, then name), section
/// 0 included, and the names of its local symbol-table entries in order.
/// The anchor is `comdat(i)` (the section's COMDAT signature, as the
/// caller knows it), else the first named non-local, else local, symbol
/// defined at offset 0 of the section. The ONE producer of the anchor:
/// the capture (`Keys::file_keys`) and the update's per-file front
/// (`mortar::update::file_front`) both call it.
///
/// # Errors
/// The part of the object that does not read (`symbol table`, `symbol
/// name`, `section name`).
pub fn section_keys<'s>(
    object: &mortar_elf::Object<'s>,
    comdat: &dyn Fn(SectionIndex) -> Option<&'s [u8]>,
) -> Result<(Vec<SectionKey<'s>>, Vec<&'s [u8]>), &'static str> {
    let count = object.section_count() as usize;
    let mut global: Vec<Option<&'s [u8]>> = vec![None; count];
    let mut local: Vec<Option<&'s [u8]>> = vec![None; count];
    let mut names = Vec::new();
    if let Some(symbols) = object.symbols().map_err(|_| "symbol table")? {
        let first_global = symbols.first_global().0;
        for (index, sym) in symbols.iter() {
            let name = symbols.name(&sym).map_err(|_| "symbol name")?;
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
            Ok(s) if i > 0 => s.name().map_err(|_| "section name")?,
            _ => &[][..],
        };
        let anchor = Anchor::choose(comdat(SectionIndex(i as u32)), global[i], local[i]);
        keys.push(SectionKey { anchor, name });
    }
    Ok((keys, names))
}

/// The content hash of a fragment's bytes (`L128`, IN-18a `FRAGS`).
/// `buf` holds the bytes of a fragment not resident in the store; it is
/// reused from one fragment to the next.
pub(crate) fn frag_hash(link: &Link<'_, '_>, frag: FragId, buf: &mut Vec<u8>) -> Option<[u8; 16]> {
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
                    option: false,
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
                    option: false,
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
#[allow(clippy::too_many_arguments)] // capture's, and the laps
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
    capture_in(link, keys, layout, strings, file_hash, arch, pool, laps, None).map(|x| x.0)
}

/// [`capture_timed`], and the rows of format 5 that format 4 does not
/// hold, from `extra` (WP-244): the format-4 state is unchanged; the
/// caller completes it (`META`, `STAT`, ...) and gives both to
/// [`state5_bytes`].
///
/// # Errors
/// As [`capture_timed`]; and the format-5 capture's refusals (a demand
/// on a key the file does not mention, a fragment without class).
#[allow(clippy::too_many_arguments)] // capture's, the laps and the format-5 data
pub fn capture5_timed(
    link: &Link<'_, '_>,
    keys: &Keys<'_>,
    layout: &Layout,
    strings: &mut Strings,
    file_hash: &dyn Fn(FileId) -> Option<[u8; 16]>,
    arch: &dyn mortar_arch::Arch,
    pool: &Pool,
    laps: &mut Laps,
    extra: &Link5<'_>,
) -> Result<(State, Rows5), String> {
    let (s, rows) =
        capture_in(link, keys, layout, strings, file_hash, arch, pool, laps, Some(extra))?;
    Ok((s, rows.ok_or("no format-5 rows")?))
}

#[allow(clippy::too_many_lines, clippy::too_many_arguments)] // one block per table, in IN-18a's order
fn capture_in(
    link: &Link<'_, '_>,
    keys: &Keys<'_>,
    layout: &Layout,
    strings: &mut Strings,
    file_hash: &dyn Fn(FileId) -> Option<[u8; 16]>,
    arch: &dyn mortar_arch::Arch,
    pool: &Pool,
    laps: &mut Laps,
    extra: Option<&Link5<'_>>,
) -> Result<(State, Option<Rows5>), String> {
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
    // WP-257a: format 5's section facts (they need `Liveness`, which only
    // `Link5` carries; format 4's rows keep those bytes 0).
    let mut sec_facts = Vec::with_capacity(if extra.is_some() { s.sections.capacity() } else { 0 });
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
        if let Some(x) = extra {
            sec_facts.push(section_facts_of(link, x.liveness, id)?);
        }
        s.sections.push(row);
    }
    clock.lap("capture sections");
    if let Some(x) = extra
        && x.members.len() != p.chunks.len() as usize
    {
        return Err(format!(
            "CHUNKS.members: {} counts for {} chunks",
            x.members.len(),
            p.chunks.len()
        ));
    }
    for (id, c) in p.chunks.iter() {
        let (kind, name) = chunk_ord.key(id.0).ok_or("a chunk")?;
        let ordinal = chunk_ord.ordinal(id.0).ok_or("a chunk")?;
        let (out, part, dep) = match c.kind {
            ChunkKind::Body(pt) | ChunkKind::Patches(pt) => (pt.out.0, pt.part, false),
            ChunkKind::Merged(o) | ChunkKind::Relocs(o) => (o.0, 0, false),
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
            members: extra.map_or(0, |x| x.members[id.0 as usize]),
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
    // Format 5's FRAGS read the same hashes (one per live fragment).
    let live_frags = if extra.is_some() { live } else { Vec::new() };
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
    let rows5 = match extra {
        Some(extra) => {
            let mut r =
                rows5(link, keys, &mut syms, strings, &s, layout, extra, &live_frags, pool)?;
            r.sections = sec_facts;
            clock.lap("capture format 5");
            Some(r)
        }
        None => None,
    };
    s.symbols = syms.rows;
    if !strings.fits() {
        return Err("the string pool exceeds 4 GiB".into());
    }
    Ok((s, rows5))
}

// ---- format 5 (WP-244) ---------------------------------------------------------

/// One file's `.eh_frame` range in the output (`spec/incr.md` IN-34a,
/// IN-18e `FILES`' eh columns; the full link's `EhPlan::file_runs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EhFile {
    pub file: FileId,
    /// Bytes from the `.eh_frame` chunk's start.
    pub offset: u64,
    /// The records' bytes and the pad after them.
    pub capacity: u64,
    /// Kept records (CIEs and FDEs).
    pub records: u32,
    /// The FDEs among `records` (seam 6's `FILES.eh_fdes`).
    pub fdes: u32,
    /// Every kept CIE's pointer encodings are ones an update moves by a
    /// delta (FDE and LSDA `0x1b`, personality `0x9b`; WP-239 §3).
    pub movable: bool,
}

/// What a format-5 capture reads of the link beyond a format-4 one
/// (WP-244), each from the producer the update's front also calls.
#[derive(Clone, Copy)]
pub struct Link5<'a> {
    pub liveness: &'a mortar_core::Liveness,
    /// Per `Split::merge_of` row: its pieces' class
    /// (`mortar_merge::phase::Phase::meta`, less `out`).
    pub classes: &'a [mortar_core::incr::MergeClass],
    /// The files' `.eh_frame` ranges, or why the link has none to store.
    pub eh: Result<&'a [EhFile], &'a str>,
    /// Per file, the scan's demand per key (`mortar_scan::scan_recording`),
    /// sorted by file.
    pub demands: &'a [mortar_core::incr::FileDemands],
    /// Per `SymbolId`: `SymFacts::pack(Walker::key_facts(id))`.
    pub facts: &'a [u32],
    /// The demand bits that make a slot of each kind (the full link's
    /// slot lists, `mortar::front`).
    pub slot_demand: fn(SlotKind) -> mortar_core::demands::DemandFlags,
    /// `COPIES` is written (every `--odr` mode but `off`, WP-220).
    pub copies: bool,
    /// Per object file that takes part in L-84: its `.note.GNU-stack` as
    /// a `rows::file_stack` byte (`mortar_layout::stack::file_note`), by
    /// file (seam 6's `FILES.stack`).
    pub stack: &'a [(FileId, u8)],
    /// Per live object with properties: its own after S-126, as
    /// `rows::encode_properties` writes them
    /// (`mortar_synth::notes::object_properties`), by file (seam 6's
    /// `FILES.properties`).
    pub properties: &'a [(FileId, Vec<u8>)],
    /// `mortar_core::plan::Plan::option_keys` (seam 6's `SYMBOLS.option`).
    pub option_keys: &'a [Vec<u8>],
    /// IN-29's rows (`mortar_live::stored_edges`), sorted by source;
    /// `None` without `--gc-sections` (no `EDGES`, `META.live_hash`
    /// zero). WP-238 seam 9: the field only; Part 2 writes the rows and
    /// the hash.
    pub edges: Option<&'a [mortar_core::incr::LiveEdge]>,
    /// `CHUNKS.members` (VERSION 12, WP-255g): per `ChunkId` of the
    /// layout, the number of sections the full link's plan gives an
    /// `Input` row (`mortar_image::layout_plan::member_counts`); one entry
    /// per chunk, or the capture fails.
    pub members: &'a [u32],
    /// `SYMTAB_KEYS` (IN-18e, IN-49a; WP-262): the output's global area
    /// (`SymtabPlan::incr_global_area`) and per `SymbolId` its entry and
    /// `st_name` (`SymtabPlan::incr_global_keys`, `(NONE, 0)`: no global
    /// entry); `None` with no `.symtab` (`-s`), every row `NONE`.
    /// Then (WP-263) `SYMTAB`'s rows and `META.symtab`: the plan's blocks
    /// in `.symtab` order (`SymtabPlan::incr_blocks`, the internal file's
    /// as `FileId(0)`) and the tables' sizes (`SymtabPlan::incr_tables`).
    #[allow(clippy::type_complexity)] // the four answers of one plan, together or none
    pub symtab: Option<(
        mortar_core::incr::SymtabBlock,
        &'a [(u32, u32)],
        &'a [(FileId, mortar_core::incr::SymtabBlock)],
        mortar_core::incr::SymtabTables,
    )>,
}

impl std::fmt::Debug for Link5<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Link5").field("demands", &self.demands.len()).finish_non_exhaustive()
    }
}

/// One file's format-5 columns beyond format 4's ([`Rows5`]).
#[derive(Debug, Clone, Default)]
struct FileCols {
    /// `FILES.flags` to add (`LAZY`, `EH_FIXED`).
    flags: u8,
    /// `FILES`' eh columns: offset, capacity, records, FDEs.
    eh: [u32; 4],
    /// `FILE_INDEX.first_global`.
    first_global: u32,
    /// `FILES.stack` (seam 6).
    stack: u8,
    /// `FILES.properties`' bytes (seam 6; empty: none).
    properties: Vec<u8>,
    /// `FILES.round` (IN-71b): resolve's `Resolution::file_round`.
    round: u32,
}

/// The rows of format 5 that format 4 does not hold, captured beside a
/// format-4 state (its `SYMBOLS`, `SECTIONS`, `FILES` and `OUTSECS` rows
/// as references); [`state5_bytes`] makes the state of both.
#[derive(Debug, Clone, Default)]
pub struct Rows5 {
    /// Per file: its columns beyond format 4's.
    files: Vec<FileCols>,
    /// `Plan::option_keys` (seam 6): the rows they name get the bit.
    option_keys: Vec<Vec<u8>>,
    mentions: Vec<(FileId, crate::update::rows::MentionRow)>,
    key_refs: Vec<crate::update::rows::KeyRefRow>,
    frags: Vec<crate::update::rows::FragRow>,
    frag_uses: Vec<(FileId, crate::update::rows::FragUseRow)>,
    /// Parallel to format 4's `SLOTS`.
    slot_refs: Vec<u32>,
    /// Per `SYMBOLS` row: `(at_file, at_index, facts)`; a local's is its
    /// format-4 row's.
    symbols: Vec<Option<(u32, u32, u32)>>,
    groups: Vec<crate::update::rows::GroupRow>,
    commons: Vec<crate::update::rows::CommonRow>,
    copies: Vec<crate::update::rows::CopyRow>,
    /// `EDGES` (WP-238 S1), by the source's file.
    edges: Vec<(FileId, crate::update::rows::EdgeRow)>,
    versioned: u32,
    /// `META.iterations` (IN-71b "Iterations", WP-261): resolve's
    /// `Resolution::iterations`, at least 1.
    iterations: u32,
    /// `DEBUG_STRS` (WP-247, [`debug_str_rows`]).
    debug_strs: Vec<crate::update::rows::DebugStrRow>,
    /// `SYMTAB_KEYS` (WP-262): per `SYMBOLS` row, its key's global entry
    /// (`NONE`: none), and the global area it is checked against
    /// ([`symtab_keys_check`]).
    symtab_keys: Vec<crate::update::rows::SymtabKeyRow>,
    symtab_area: Option<mortar_core::incr::SymtabBlock>,
    /// `SYMTAB` (WP-263): the plan's blocks in `.symtab` order, the
    /// internal file's (`FileId(0)`) among them, checked with the area
    /// and the sizes ([`symtab_rows_check`]); `META.symtab`.
    symtab_blocks: Vec<(FileId, mortar_core::incr::SymtabBlock)>,
    symtab_tables: Option<mortar_core::incr::SymtabTables>,
    /// `META.gnu_warning_orphans` (WP-263, 255d5's F-7): resolve's
    /// `Resolution::gnu_warning_orphans`.
    gnu_warning_orphans: u32,
    /// Counts for the experiments (`incremental: state5 ...` lines).
    pub notes: Vec<String>,
    /// WP-257a: per `SECTIONS` row, what the section IS.
    sections: Vec<SectionFacts>,
    /// `DIAGS` (WP-265, [`diag_rows`]), in the capture's ids, their keys
    /// by name until [`state5_rows`] resolves them.
    diags: Vec<NamedDiag>,
}

impl Rows5 {
    /// `DIAGS`' rows of the link's `warnings` (the collector's, read before
    /// its report is finished) and of `gnu_warnings`, as [`diag_rows`]
    /// makes them; [`state5_rows`] stores them, their keys the rows of the
    /// state it makes (the options' rows included).
    ///
    /// # Errors
    /// [`diag_rows`]'.
    pub fn capture_diags(
        &mut self,
        inputs: &Inputs<'_>,
        gnu_warnings: &[(SymbolId, FileId, SectionIndex)],
        warnings: &[(mortar_diag::Key, mortar_diag::Diagnostic)],
    ) -> Result<(), String> {
        self.diags = diag_rows_named(inputs, gnu_warnings, warnings)?;
        Ok(())
    }
}

/// WP-257a: what section `id` IS, from resolve's row, `liveness` and the
/// assignment: the producers' own data (`Assignment::out` for every
/// section, kept or not: not its chunk's output section, which differs,
/// e.g. `.tm_clone_table` placed in a `.data` chunk).
///
/// # Errors
/// A section without a resolve row.
pub fn section_facts_of(
    link: &Link<'_, '_>,
    liveness: &mortar_core::Liveness,
    id: SectionId,
) -> Result<SectionFacts, String> {
    let row = link.resolution.sections.get(id).ok_or("a section without a resolve row")?;
    let kept = row.is_kept();
    Ok(SectionFacts {
        kind: row.kind,
        kept,
        collected: kept && !liveness.is_live(id),
        out: link.assignment.out(id).map(|o| o.0),
    })
}

/// `row` with `facts` written into it (WP-257a's encoding:
/// [`crate::format::section_facts`]).
///
/// # Errors
/// `state-limit`: an output section's `OUTSECS` row past the 16-bit field.
pub fn put_section_facts(row: &mut SectionRow, facts: &SectionFacts) -> Result<(), String> {
    let (kind, bits, out) = facts.encode().ok_or_else(|| {
        mortar_core::incr::IncrReason::StateLimit(format!(
            "output section {:?} is past SECTIONS' 16-bit output section column",
            facts.out
        ))
        .report()
    })?;
    (row.kind, row.bits, row.out) = (kind, bits, out);
    Ok(())
}

/// `FILES.flags`' bit for a file whose `.eh_frame` range holds a CIE
/// with a pointer encoding an update cannot move by a delta (WP-244:
/// `update-not-implemented (eh)` before the first write). PROPOSED
/// (`update::rows::file_flags::EH_FIXED`).
pub const EH_FIXED: u8 = crate::update::rows::file_flags::EH_FIXED;

/// The `FILES.flags` bits that are a function of a file's bytes alone
/// (WP-247 items d and k): `EH_FIXED` ([`eh_fixed`] over each of its
/// `.eh_frame` sections) and `DEBUG_EXPORTS` ([`debug_exports`]); 0 for
/// a file that is not an object. The capture writes them for every
/// file, archive members not extracted included; an update writes them
/// for a changed, appeared or newly extracted file's row (the join
/// calls this on the new bytes: `StatePatch.files_set`/`files_append`
/// take the row's flags from here and `LAZY`/`POSITIONAL`/`GONE` from
/// the update's own decisions).
#[must_use]
pub fn object_flags(inputs: &Inputs<'_>, file: FileId) -> u8 {
    let Some(f) = inputs.files.get(file) else { return 0 };
    let Reader::Object(object) = f.reader else { return 0 };
    let class64 = object.class() == mortar_elf::raw::Class::Elf64;
    let mut buf = Vec::new();
    object_flags_by(&object, &mut |range| {
        let bytes = if let Some(b) = inputs.resident(file, range.clone()) {
            b
        } else {
            buf.clear();
            buf.resize((range.end - range.start) as usize, 0);
            if inputs.copy_to(file, range, &mut buf).is_err() {
                return true;
            }
            &buf[..]
        };
        eh_fixed(bytes, class64)
    })
}

/// WP-267 P-1: [`object_flags`] of ONE object from its own bytes `bytes`
/// (the file's, offsets in the object; no `Inputs`): the same producer
/// (`object_flags_by`), an `.eh_frame` range past `bytes` counting as
/// one that cannot be read (`EH_FIXED`), as [`object_flags`] counts one
/// it cannot copy.
#[must_use]
pub fn object_flags_of(object: &mortar_elf::Object<'_>, bytes: &[u8]) -> u8 {
    let class64 = object.class() == mortar_elf::raw::Class::Elf64;
    object_flags_by(object, &mut |range| {
        let (Ok(a), Ok(b)) = (usize::try_from(range.start), usize::try_from(range.end)) else {
            return true;
        };
        bytes.get(a..b).is_none_or(|e| eh_fixed(e, class64))
    })
}

/// [`object_flags`]' body over `object`, `fixed` answering for each of
/// its `.eh_frame` sections' file range whether it is `EH_FIXED`.
fn object_flags_by(
    object: &mortar_elf::Object<'_>,
    fixed: &mut dyn FnMut(std::ops::Range<u64>) -> bool,
) -> u8 {
    use crate::update::rows::file_flags;
    let mut flags = 0;
    if object.symbols().ok().flatten().is_some_and(|t| debug_exports(object, &t)) {
        flags |= file_flags::DEBUG_EXPORTS;
    }
    for sec in object.sections() {
        if sec.name().ok() != Some(&b".eh_frame"[..]) {
            continue;
        }
        let Ok(Some(range)) = sec.file_range() else { continue }; // NOBITS: no records
        if fixed(range.start as u64..range.end as u64) {
            flags |= file_flags::EH_FIXED;
        }
    }
    flags
}

/// Whether the `.eh_frame` bytes `eh` hold a CIE whose FDE (`R`), LSDA
/// (`L`) or personality (`P`) pointer encoding is other than `0x1b`,
/// `0x1b`, `0x9b` (`file_flags::EH_FIXED`): an absent `R` is `absptr`
/// (other); an absent or `omit` `L`/`P` gives none. A record the walk
/// cannot read is such a CIE (a 64-bit length, a length past the bytes, a
/// CIE without a `z` augmentation but with letters, an unknown letter).
/// Walks the records by their length words to a zero length or the end.
#[must_use]
pub fn eh_fixed(eh: &[u8], class64: bool) -> bool {
    fn uleb(b: &[u8], at: &mut usize) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *b.get(*at)?;
            *at += 1;
            v |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }
    // the pointer's size by its encoding's format (low 4 bits)
    let size = |enc: u8| match enc & 0x0f {
        0x0 => Some(if class64 { 8 } else { 4 }),
        0x2 | 0xa => Some(2),
        0x3 | 0xb => Some(4),
        0x4 | 0xc => Some(8),
        _ => None, // LEB pointers: not in a CIE's `P` that an update moves
    };
    // One CIE's body after its id: `Some(true)` fixed, `Some(false)` not.
    let cie = |b: &[u8]| -> Option<bool> {
        let mut at = 0;
        let version = *b.get(at)?;
        at += 1;
        let aug_end = at + b.get(at..)?.iter().position(|&c| c == 0)?;
        let aug = &b[at..aug_end];
        at = aug_end + 1;
        if version == 4 {
            at += 2; // address and segment selector sizes
        }
        uleb(b, &mut at)?; // code alignment
        uleb(b, &mut at)?; // data alignment (SLEB: the same bytes)
        if version == 1 {
            at += 1;
        } else {
            uleb(b, &mut at)?;
        }
        let (mut fde, mut lsda, mut personality) = (0u8, None, None);
        if let Some((&b'z', letters)) = aug.split_first() {
            let len = usize::try_from(uleb(b, &mut at)?).ok()?;
            let end = at.checked_add(len)?;
            for &c in letters {
                match c {
                    b'R' => {
                        fde = *b.get(at)?;
                        at += 1;
                    }
                    b'L' => {
                        lsda = Some(*b.get(at)?).filter(|&e| e != 0xff);
                        at += 1;
                    }
                    b'P' => {
                        let enc = *b.get(at)?;
                        at += 1;
                        if enc != 0xff {
                            personality = Some(enc);
                            at += size(enc)?;
                        }
                    }
                    b'S' | b'B' | b'G' => {}
                    _ => return None,
                }
                if at > end {
                    return None;
                }
            }
        } else if !aug.is_empty() {
            return None;
        }
        Some(
            fde != 0x1b
                || lsda.is_some_and(|e| e != 0x1b)
                || personality.is_some_and(|e| e != 0x9b),
        )
    };
    let mut at = 0usize;
    while at + 4 <= eh.len() {
        let len = u32::from_le_bytes([eh[at], eh[at + 1], eh[at + 2], eh[at + 3]]);
        if len == 0 {
            break;
        }
        if len == u32::MAX {
            return true;
        }
        let Some(end) = (at + 4).checked_add(len as usize).filter(|&e| e <= eh.len() && len >= 4)
        else {
            return true;
        };
        let id = u32::from_le_bytes([eh[at + 4], eh[at + 5], eh[at + 6], eh[at + 7]]);
        if id == 0 && cie(&eh[at + 8..end]) != Some(false) {
            return true;
        }
        at = end;
    }
    false
}

/// Whether `object` defines a GLOBAL, WEAK or `GNU_UNIQUE` symbol in a
/// non-allocated section named `.debug_*` or `.zdebug_*`
/// (`file_flags::DEBUG_EXPORTS`; WP-229 ruling 3).
#[must_use]
pub fn debug_exports(
    object: &mortar_elf::object::Object<'_>,
    table: &mortar_elf::symbol::SymbolTable<'_>,
) -> bool {
    use mortar_elf::consts::{SHF_ALLOC, STB_GLOBAL, STB_GNU_UNIQUE, STB_WEAK};
    (table.first_global().0..table.len()).any(|i| {
        let i = mortar_elf::symbol::SymbolIndex(i);
        let Ok(sym) = table.get(i) else { return false };
        if !matches!(sym.binding(), STB_GLOBAL | STB_WEAK | STB_GNU_UNIQUE) {
            return false;
        }
        let Ok(SymbolSection::Section(index)) = table.section_of(i, &sym) else { return false };
        object.section(index).is_ok_and(|s| {
            s.flags() & SHF_ALLOC == 0
                && s.name().is_ok_and(|n| n.starts_with(b".debug") || n.starts_with(b".zdebug"))
        })
    })
}

/// `FILES.round` of `file` (IN-71b, WP-261): resolve's round of an
/// extracted lazy file, which is at least 1; 0 for every other file. A
/// round that breaks this (an extracted lazy file of round 0, another
/// file of a round) is the producer's defect: no state is written.
fn file_round(res: &Resolution, file: FileId) -> Result<u32, String> {
    use mortar_core::resolution::FileLive;
    let round = res.file_round.get(file.0 as usize).copied().unwrap_or(0);
    let extracted = matches!(res.file_live.get(file), Some(FileLive::Extracted(_)));
    if extracted == (round == 0) {
        return Err(format!(
            "file {}: FILES.round {round} for a file {} (IN-71b)",
            file.0,
            if extracted { "extracted from an archive" } else { "not extracted" }
        ));
    }
    Ok(round)
}

/// `SYMTAB_KEYS`' write-time check (IN-18e, IN-49a; WP-262), on every
/// capture: with the output's global area `[first, first + count)`, every
/// entry of it is named by exactly one row, every row not `NONE` names an
/// entry of it, and the rows' `strtab` are where S-86 lays the area's
/// names: in entry order from the area's first byte, each at least its
/// key's name and its zero past the one before (the entry's name is the
/// key's, or the key's spelled with its version, `foo@V1`, `foo@@V2`:
/// S-75, WP-150), the last ending inside the area; the fixed table's
/// names at their own offsets. The exact lengths are the symbol table
/// plan's (`SymtabPlan::incr_global_keys` checks its walk against its
/// counting pass). Without a `.symtab` (`-s`) every row is `NONE`. A row
/// that breaks this is the producer's defect: no state is written.
fn symtab_keys_check(
    m: &crate::update::testing::MemoryState,
    area: Option<mortar_core::incr::SymtabBlock>,
) -> Result<(), String> {
    let err = |what: String| Err(format!("SYMTAB_KEYS: {what} (IN-49a)"));
    let Some(area) = area else {
        return match m.symtab_keys.iter().position(|r| r.index != NONE) {
            None => Ok(()),
            Some(r) => err(format!("row {r} has an entry and the output has no .symtab")),
        };
    };
    // Per entry of the area, the row that names it.
    let mut by_entry = vec![NONE; area.count as usize];
    for (r, k) in m.symtab_keys.iter().enumerate() {
        if k.index == NONE {
            continue;
        }
        let Some(slot) = k.index.checked_sub(area.first).and_then(|i| by_entry.get_mut(i as usize))
        else {
            return err(format!(
                "row {r}'s index {} is outside the global area [{}, {})",
                k.index,
                area.first,
                u64::from(area.first) + u64::from(area.count)
            ));
        };
        if *slot != NONE {
            return err(format!("rows {} and {r} have one index {}", *slot, k.index));
        }
        *slot = r as u32;
    }
    let end = area.strtab_offset + area.strtab_len;
    // The first byte the next region name may start at, and whether a
    // region name came before (the first starts AT the area's first byte).
    let (mut at, mut first) = (area.strtab_offset, true);
    for (i, &r) in by_entry.iter().enumerate() {
        let entry = u64::from(area.first) + i as u64;
        if r == NONE {
            return err(format!("entry {entry} of the global area is named by no row"));
        }
        let (k, sym) = (m.symtab_keys[r as usize], &m.symbols[r as usize]);
        let name = m.strings.get(sym.name.off as usize..(sym.name.off + sym.name.len) as usize);
        let Some(name) = name else { return err(format!("row {r}'s name is outside STRINGS")) };
        // S-86's fixed table: `$x`, `$d`, `__erratum_843419_veneer`.
        let fixed = match name {
            b"" => Some(0),
            b"$x" => Some(1),
            b"$d" => Some(4),
            b"__erratum_843419_veneer" => Some(7),
            _ => None,
        };
        let s = u64::from(k.strtab);
        let placed = match fixed {
            Some(f) => s == f,
            None if first => s == at,
            None => s >= at,
        };
        if !placed {
            return err(format!(
                "row {r} (entry {entry}): strtab {s} where the area's names put {}{at}",
                if first || fixed.is_some() { "" } else { "at least " }
            ));
        }
        if fixed.is_none() {
            at = s + name.len() as u64 + 1;
            first = false;
            if at > end {
                return err(format!(
                    "row {r} (entry {entry}): its name ends past the area's {end}"
                ));
            }
        }
    }
    Ok(())
}

/// `SYMTAB` and `META.symtab`'s write-time check (IN-18e, IN-49a;
/// WP-263), on every capture. With a `.symtab`: the plan's blocks are in
/// `FILES` order (the internal file's first, as file 0), each of a file
/// with a `FILES` row that is not `LAZY`, holding at least one entry and
/// at most its capacity, and they tile `[1, tail_first)`; the local tail
/// reserve ends at `sh_info`, where the global area starts; the area is
/// dense (capacity = count) and ends `.symtab`'s entries; the `.strtab`
/// regions are each used at most to their capacity and tile from the
/// first block's region to the area's names, which end `.strtab`;
/// `META.symtab`'s sizes are the `.symtab` and `.strtab` chunks' sizes,
/// and `SYMTAB`'s rows are, in file 0's range, the area then the
/// internal file's block (when it has an entry; WP-264), then the blocks
/// of the input files. Without one (`-s`): no block, no row, `META`
/// zero, no such chunk. A break is the producer's defect: no state is
/// written.
#[allow(clippy::too_many_lines)] // one test per property, each with its words
fn symtab_rows_check(
    m: &crate::update::testing::MemoryState,
    blocks: &[(FileId, mortar_core::incr::SymtabBlock)],
    area: Option<mortar_core::incr::SymtabBlock>,
    tables: Option<mortar_core::incr::SymtabTables>,
) -> Result<(), String> {
    use crate::update::rows::file_flags::LAZY;
    let err = |what: String| Err(format!("SYMTAB: {what} (IN-49a)"));
    let chunk = |name: &[u8]| {
        m.chunks.iter().find(|c| {
            m.strings.get(c.name.off as usize..(c.name.off + c.name.len) as usize) == Some(name)
        })
    };
    let (Some(area), Some(t)) = (area, tables) else {
        if !blocks.is_empty() || !m.symtab.is_empty() {
            return err("rows for an output without .symtab".into());
        }
        if m.meta.symtab != mortar_core::incr::SymtabTables::default() {
            return err("META.symtab is not zero for an output without .symtab".into());
        }
        if chunk(b".symtab").is_some() {
            return err("a .symtab chunk with no table sizes".into());
        }
        return Ok(());
    };
    let mut at = 1u32;
    let mut str_at: Option<u64> = None;
    let mut prev: Option<FileId> = None;
    for &(file, b) in blocks {
        let who = format!("file {}'s block", file.0);
        if prev.is_some_and(|p| p >= file) {
            return err(format!(
                "{who} after file {}'s: not in FILES order",
                prev.map_or(0, |p| p.0)
            ));
        }
        prev = Some(file);
        match m.files.get(file.0 as usize) {
            None => return err(format!("{who}: no FILES row")),
            Some(f) if file.0 != 0 && f.flags & LAZY != 0 => {
                return err(format!("{who}: the file is LAZY"));
            }
            Some(_) => {}
        }
        if b.first != at {
            return err(format!(
                "{who} starts at entry {} where the one before ends at {at}",
                b.first
            ));
        }
        if b.count == 0 || b.count > b.capacity {
            return err(format!("{who} holds {} entries of capacity {}", b.count, b.capacity));
        }
        if b.strtab_len > b.strtab_capacity {
            return err(format!(
                "{who}'s .strtab region uses {} of {} bytes",
                b.strtab_len, b.strtab_capacity
            ));
        }
        if str_at.is_some_and(|s| s != b.strtab_offset) {
            return err(format!(
                "{who}'s .strtab region starts at {} where the one before ends at {}",
                b.strtab_offset,
                str_at.unwrap_or_default()
            ));
        }
        at = b
            .first
            .checked_add(b.capacity)
            .ok_or_else(|| format!("SYMTAB: {who} ends past 2^32 entries"))?;
        str_at = Some(b.strtab_offset + b.strtab_capacity);
    }
    if at != t.tail_first {
        return err(format!(
            "the blocks end at entry {at}, the tail reserve starts at {}",
            t.tail_first
        ));
    }
    if u64::from(t.tail_first) + u64::from(t.tail_capacity) != u64::from(t.sh_info) {
        return err(format!(
            "the tail reserve [{}, +{}) does not end at sh_info {}",
            t.tail_first, t.tail_capacity, t.sh_info
        ));
    }
    if area.first != t.sh_info || area.count != area.capacity {
        return err(format!(
            "the global area starts at {} (sh_info {}), holds {} of {}",
            area.first, t.sh_info, area.count, area.capacity
        ));
    }
    if str_at.is_some_and(|s| s != area.strtab_offset) {
        return err(format!(
            "the blocks' .strtab regions end at {}, the global names start at {}",
            str_at.unwrap_or_default(),
            area.strtab_offset
        ));
    }
    if area.strtab_len != area.strtab_capacity
        || area.strtab_offset + area.strtab_len != t.strtab_size
    {
        return err(format!(
            "the global names [{}, +{}) (held {}) do not end .strtab's {}",
            area.strtab_offset, area.strtab_len, area.strtab_capacity, t.strtab_size
        ));
    }
    let entsize = if m.meta.class == mortar_elf::consts::ELFCLASS64 { 24 } else { 16 };
    let entries = u64::from(area.first) + u64::from(area.count);
    if entries * entsize != t.symtab_size {
        return err(format!(
            "{entries} entries of {entsize} bytes where META.symtab's size is {}",
            t.symtab_size
        ));
    }
    for (name, size) in [(&b".symtab"[..], t.symtab_size), (&b".strtab"[..], t.strtab_size)] {
        let shown = String::from_utf8_lossy(name);
        match chunk(name) {
            None => return err(format!("no {shown} chunk")),
            Some(c) if c.size != size => {
                return err(format!(
                    "{shown}'s chunk is {} bytes, META.symtab says {size}",
                    c.size
                ));
            }
            Some(_) => {}
        }
    }
    // File 0's range: the area, then the internal block (none without
    // an entry); then every input file's block, in FILES order.
    let mut want = vec![(FileId(0), area)];
    want.extend(blocks.iter().copied());
    if m.symtab != want {
        return err("the rows are not file 0's global area and internal block, then the input \
                    files' blocks"
            .into());
    }
    Ok(())
}

/// `REGIONS` (IN-18e, IN-30, IN-72 item 2; WP-264): the full link's
/// regions, from its placement as the state stores it (`SECTIONS`'
/// places, `CHUNKS`, `HOLES`), so the delta layout reads exactly the
/// rows it judges against.
///
/// The full link defines a region in `mortar_layout::chunk` `plan_parts`
/// (`chunk.rs` ~348-410) with `close` (~325-345): in an allocated
/// hole-tolerant output section (`slack::class`, IN-30), a run of members
/// with one key, `(file, category)` (`region_key`, ~471: the `.text`
/// category of an `SHF_EXECINSTR` member, else 0), from the first
/// member's aligned offset to the end of the slack `close` adds as ONE
/// `HoleKind::Fill` hole after the last member (`Slack::region`, clipped
/// to the part's limit; no hole when clipped to 0). Here: per body chunk
/// of such a section, its placed sections whose own output section
/// (`SECTIONS.out`) is the chunk's (L-159's empty neighbours are not its
/// members), by offset; a run ends where the key changes or a `Fill` hole
/// starts; its capacity runs to that hole's end. Ordinals per (file,
/// output section, category) in address order (a run cut by a part limit
/// is two regions).
///
/// # Errors
/// A row that names a missing chunk, output section or string.
fn region_rows(
    m: &crate::update::testing::MemoryState,
) -> Result<Vec<(FileId, crate::update::rows::RegionRow)>, String> {
    use crate::format::hole_kind;
    use crate::update::rows::RegionRow;
    use mortar_layout::slack::{Class, class};
    let body = tag_byte(ChunkTag::Body);
    let text = |s: Str| {
        m.strings
            .get(s.off as usize..(s.off as usize).saturating_add(s.len as usize))
            .ok_or_else(|| "REGIONS: a string past STRINGS".to_owned())
    };
    let tolerant = |c: &ChunkRow| -> Result<bool, String> {
        if c.tag != body || c.sh_flags & mortar_elf::consts::SHF_ALLOC == 0 {
            return Ok(false);
        }
        let o = m.outsecs.get(c.out as usize).ok_or("REGIONS: a body chunk's OUTSECS row")?;
        Ok(class(text(o.name)?, c.sh_flags) == Class::Tolerant)
    };
    let mut members: HashMap<u32, Vec<(u64, u64, usize)>> = HashMap::default();
    for (i, s) in m.sections.iter().enumerate() {
        if s.place_chunk == NONE {
            continue;
        }
        let c = m.chunks.get(s.place_chunk as usize).ok_or("REGIONS: a section's chunk")?;
        if !tolerant(c)? || u32::from(s.out) != c.out + 1 {
            continue;
        }
        let end = s.place_offset.checked_add(s.size).ok_or("REGIONS: a section's end")?;
        members.entry(s.place_chunk).or_default().push((s.place_offset, end, i));
    }
    let mut fills: HashMap<u32, Vec<(u64, u64)>> = HashMap::default();
    for h in m.holes.iter().filter(|h| h.kind == hole_kind::FILL && !h.tomb) {
        fills.entry(h.chunk).or_default().push((h.offset, h.offset.saturating_add(h.size)));
    }
    // (file, out, category, address, row) before the ordinals.
    let mut found: Vec<(u32, u32, u8, u64, RegionRow)> = Vec::new();
    let mut chunks: Vec<u32> = members.keys().copied().collect();
    chunks.sort_unstable();
    for ci in chunks {
        let c = &m.chunks[ci as usize];
        let mut list = members.remove(&ci).unwrap_or_default();
        list.sort_unstable();
        let holes = fills.get(&ci).map_or(&[][..], |v| &v[..]);
        // The first `Fill` hole at or after `at`.
        let next_hole = |at: u64| holes.get(holes.partition_point(|h| h.0 < at)).copied();
        let slack_at = |end: u64| next_hole(end).filter(|h| h.0 == end).map(|h| h.1);
        let mut run: Option<(u32, u8, u64, u64)> = None;
        let close = |r: (u32, u8, u64, u64), found: &mut Vec<_>| {
            let (file, category, start, end) = r;
            let end = slack_at(end).unwrap_or(end);
            found.push((
                file,
                c.out,
                category,
                c.addr.saturating_add(start),
                RegionRow {
                    out: c.out,
                    category,
                    ordinal: 0,
                    chunk: ci,
                    offset: start,
                    capacity: end - start,
                },
            ));
        };
        for (off, end, i) in list {
            let s = &m.sections[i];
            let cat = if s.sh_flags & mortar_elf::consts::SHF_EXECINSTR != 0 {
                mortar_layout::order::text_category(text(s.name)?)
            } else {
                0
            };
            // A run ends at a key change or at a `Fill` hole between its
            // end and this member (a zero-size member at its end is its).
            if let Some(r) = run
                && (r.0 != s.file || r.1 != cat || next_hole(r.3).is_some_and(|h| h.0 < off))
            {
                close(r, &mut found);
                run = None;
            }
            match &mut run {
                Some(r) => r.3 = r.3.max(end),
                None => run = Some((s.file, cat, off, end)),
            }
        }
        if let Some(r) = run {
            close(r, &mut found);
        }
    }
    found.sort_unstable_by_key(|x| (x.0, x.1, x.2, x.3));
    let mut out = Vec::with_capacity(found.len());
    let mut prev: Option<(u32, u32, u8)> = None;
    let mut ordinal = 0u32;
    for (file, o, cat, _, mut row) in found {
        ordinal = if prev == Some((file, o, cat)) { ordinal + 1 } else { 0 };
        prev = Some((file, o, cat));
        row.ordinal = ordinal;
        out.push((FileId(file), row));
    }
    Ok(out)
}

/// The write-time check of `REGIONS` (WP-264): per body chunk of an
/// allocated hole-tolerant output section, its regions lie inside the
/// chunk and do not overlap; they tile its placed sections (each section
/// in a region of its own file and category, each region starting at its
/// first section); every `Fill` hole of the chunk is one region's slack,
/// from its last section's end to its own, or the padding before a
/// region, from the previous region's end to its start (WP-255g, S2-1:
/// the delta's pad once the region's first member no longer explains it,
/// and `overflow`'s pad before a new region); a region without slack ends at its
/// last section and is the chunk's last; a file's ordinals per (output
/// section, category) are 0, 1, ... in address order. A break is `Err`:
/// no state is written.
#[allow(clippy::too_many_lines)] // one check per property, each with its words
fn regions_check(m: &crate::update::testing::MemoryState) -> Result<(), String> {
    // Per chunk: (offset, end, file, category, sections, first, last end).
    type Reg = (u64, u64, u32, u8, u32, u64, u64);
    use crate::format::hole_kind;
    use mortar_layout::slack::{Class, class};
    let err = |what: String| Err(format!("REGIONS: {what} (IN-30, IN-72 item 2)"));
    let body = tag_byte(ChunkTag::Body);
    let text =
        |s: Str| m.strings.get(s.off as usize..(s.off as usize).saturating_add(s.len as usize));
    let tolerant = |c: &ChunkRow| {
        c.tag == body
            && c.sh_flags & mortar_elf::consts::SHF_ALLOC != 0
            && m.outsecs
                .get(c.out as usize)
                .and_then(|o| text(o.name))
                .is_some_and(|n| class(n, c.sh_flags) == Class::Tolerant)
    };
    let mut by_chunk: HashMap<u32, Vec<Reg>> = HashMap::default();
    let mut ordinals: HashMap<(u32, u32, u8), Vec<(u64, u32)>> = HashMap::default();
    for &(file, r) in &m.regions {
        let who = format!("file {}'s region {} of output section {}", file.0, r.ordinal, r.out);
        let Some(c) = m.chunks.get(r.chunk as usize) else {
            return err(format!("{who}: no chunk {}", r.chunk));
        };
        if !tolerant(c) || c.out != r.out {
            return err(format!("{who}: chunk {} is not a hole-tolerant body of it", r.chunk));
        }
        let end = r.offset.checked_add(r.capacity);
        if end.is_none_or(|e| e > c.size) {
            return err(format!(
                "{who}: [{:#x}, +{:#x}) is outside chunk {} ({:#x} bytes)",
                r.offset, r.capacity, r.chunk, c.size
            ));
        }
        by_chunk.entry(r.chunk).or_default().push((
            r.offset,
            r.offset + r.capacity,
            file.0,
            r.category,
            0,
            u64::MAX,
            0,
        ));
        ordinals
            .entry((file.0, r.out, r.category))
            .or_default()
            .push((c.addr.saturating_add(r.offset), r.ordinal));
    }
    for v in by_chunk.values_mut() {
        v.sort_unstable();
        for w in v.windows(2) {
            if w[0].1 > w[1].0 {
                return err(format!(
                    "regions [{:#x}, {:#x}) (file {}) and [{:#x}, {:#x}) (file {}) overlap",
                    w[0].0, w[0].1, w[0].2, w[1].0, w[1].1, w[1].2
                ));
            }
        }
    }
    for (key, mut v) in ordinals {
        v.sort_unstable();
        if v.iter().enumerate().any(|(k, x)| x.1 as usize != k) {
            return err(format!(
                "file {}'s regions of output section {} category {} have ordinals {:?}",
                key.0,
                key.1,
                key.2,
                v.iter().map(|x| x.1).collect::<Vec<_>>()
            ));
        }
    }
    for s in &m.sections {
        if s.place_chunk == NONE {
            continue;
        }
        let Some(c) = m.chunks.get(s.place_chunk as usize) else {
            return err(format!("a section of file {} in no chunk", s.file));
        };
        if !tolerant(c) || u32::from(s.out) != c.out + 1 {
            continue;
        }
        let cat = if s.sh_flags & mortar_elf::consts::SHF_EXECINSTR != 0 {
            text(s.name).map_or(0, mortar_layout::order::text_category)
        } else {
            0
        };
        let (off, end) = (s.place_offset, s.place_offset.saturating_add(s.size));
        let name = || text(s.name).map(String::from_utf8_lossy).unwrap_or_default();
        let Some(regs) = by_chunk.get_mut(&s.place_chunk) else {
            return err(format!(
                "section {} of file {} is in chunk {}, which has no region",
                name(),
                s.file,
                s.place_chunk
            ));
        };
        let at = regs.partition_point(|r| r.0 <= off);
        // The region holding it: the last one starting at or before it, or
        // (a section of size 0 at a boundary) the one before that.
        let hit = (at.saturating_sub(2)..at).rev().find(|&k| {
            let r = &regs[k];
            r.2 == s.file && r.3 == cat && r.0 <= off && end <= r.1
        });
        let Some(k) = hit else {
            return err(format!(
                "section {} of file {} at [{off:#x}, {end:#x}) of chunk {} is in no region \
                 of its file and category {cat}",
                name(),
                s.file,
                s.place_chunk
            ));
        };
        let r = &mut regs[k];
        r.4 += 1;
        r.5 = r.5.min(off);
        r.6 = r.6.max(end);
    }
    let mut fills: HashMap<u32, Vec<(u64, u64)>> = HashMap::default();
    for h in m.holes.iter().filter(|h| h.kind == hole_kind::FILL && !h.tomb) {
        if m.chunks.get(h.chunk as usize).is_some_and(&tolerant) {
            fills.entry(h.chunk).or_default().push((h.offset, h.offset.saturating_add(h.size)));
        }
    }
    for (ci, regs) in &by_chunk {
        let holes = fills.remove(ci).unwrap_or_default();
        let mut used = vec![false; holes.len()];
        for (k, r) in regs.iter().enumerate() {
            let who = format!("file {}'s region [{:#x}, {:#x}) of chunk {ci}", r.2, r.0, r.1);
            if r.4 == 0 {
                return err(format!("{who} holds no section"));
            }
            if r.5 != r.0 {
                return err(format!("{who} starts before its first section ({:#x})", r.5));
            }
            match holes.iter().position(|h| h.0 == r.6 && h.1 == r.1) {
                Some(h) => used[h] = true,
                None if r.6 == r.1 && k + 1 == regs.len() => {}
                None => {
                    return err(format!(
                        "{who}: its sections end at {:#x} and no Fill hole is its slack",
                        r.6
                    ));
                }
            }
        }
        // S2-1 (WP-255g): a pad, `[previous region's end, region start)`.
        for (k, r) in regs.iter().enumerate().skip(1) {
            if let Some(h) = holes.iter().position(|h| h.0 == regs[k - 1].1 && h.1 == r.0) {
                used[h] = true;
            }
        }
        if let Some(h) = used.iter().position(|u| !u) {
            return err(format!(
                "the Fill hole [{:#x}, {:#x}) of chunk {ci} is no region's slack",
                holes[h].0, holes[h].1
            ));
        }
    }
    if let Some((ci, h)) = fills.iter().next() {
        return err(format!("chunk {ci} has {} Fill holes and no region", h.len()));
    }
    Ok(())
}

/// One file's part of [`rows5`], with keys as `SymbolId`s.
#[derive(Default)]
struct File5<'s> {
    first_global: u32,
    /// [`object_flags`] (WP-247).
    flags: u8,
    versioned: u32,
    lazy: bool,
    /// Positional (IN-18e "`MENTIONS` by position"); a live object's only.
    mentions: Vec<crate::format::MentionRow>,
    /// `(key, def, bind, typ, flags)`, one per key, the first mention.
    refs: Vec<(u32, u8, u8, u8, u8)>,
    /// `(key, size, align)`, the maximum of each per key.
    commons: Vec<(u32, u64, u64)>,
    /// `(key, symbol index, size, type)`.
    copies: Vec<(u32, u32, u64, u8)>,
    /// The COMDAT signatures the file has a copy of.
    groups: Vec<&'s [u8]>,
}

/// One file's part of [`rows5`] (its symbol table's rows: `MENTIONS`,
/// `KEY_REFS`' refs, `COMMONS`, `COPIES`, `GROUPS`), the per-file task of
/// the format-5 capture (WP-225 G-2b: moved out of `rows5` as it was).
#[allow(clippy::too_many_lines)]
fn file5<'s>(
    inputs: &'s Inputs<'_>,
    res: &Resolution,
    copies: bool,
    file: FileId,
) -> Result<File5<'s>, String> {
    use crate::format::def_kind;
    use crate::update::rows::key_ref_flags;
    use mortar_core::resolution::FileLive;
    let live_file = |f: FileId| res.file_live.get(f).is_some_and(|l| *l != FileLive::Dead);
    let f = &inputs.files[file];
    let Reader::Object(object) = f.reader else { return Ok(File5::default()) };
    let bad = || format!("{}: symbol table", String::from_utf8_lossy(f.path));
    let live = live_file(file);
    let Some(table) = object.symbols().map_err(|_| bad())? else {
        // WP-261 (F-7): an object without a symbol table (x86-64's
        // libstdc++ `tinfo2.o`, `compatibility-*.o`, libm's
        // `w_*_compat.o`) has no mention, so no edge reaches it: an
        // archive member of that kind is never extracted and its row is
        // `LAZY` like any member not extracted (it was stored live).
        return Ok(File5 { lazy: !live, flags: object_flags(inputs, file), ..File5::default() });
    };
    let first = table.first_global().0;
    let mut acc = File5 { first_global: first, lazy: !live, ..File5::default() };
    acc.flags = object_flags(inputs, file);
    let globals = table.len().saturating_sub(first) as usize;
    if inputs.sym_map_of(file).len() < globals {
        return Err(format!(
            "{}: the symbol table is not the one the input phase read",
            String::from_utf8_lossy(f.path)
        ));
    }
    let rows = crate::tables::file_mentions(inputs, res, file, &table);
    let mut seen: mortar_core::hash::HashSet<u32> = mortar_core::hash::HashSet::default();
    let mut commons: HashMap<u32, usize> = HashMap::default();
    for (k, m) in rows.iter().enumerate() {
        let index = first + k as u32;
        let sym = table.get(mortar_elf::symbol::SymbolIndex(index)).map_err(|_| bad())?;
        let name = table.name(&sym).map_err(|_| bad())?;
        if name.contains(&b'@') {
            acc.versioned += 1;
        }
        let in_section = matches!(
            table.section_of(mortar_elf::symbol::SymbolIndex(index), &sym),
            Ok(SymbolSection::Section(_))
        );
        let mut def = m.kind;
        if !live && in_section {
            // A member not extracted has no section ids: its
            // definition in a section is one (IN-71b).
            def = def_kind::SECTION;
        }
        let mut flags = m.other & key_ref_flags::VISIBILITY;
        if def == def_kind::SECTION {
            flags |= key_ref_flags::IN_SECTION;
        }
        if !live {
            if def == def_kind::UNDEFINED {
                continue;
            }
            flags |= key_ref_flags::LAZY;
        }
        if seen.insert(m.symbol) {
            acc.refs.push((m.symbol, def, m.bind, m.typ, flags));
        }
        if !live {
            continue;
        }
        if m.kind == def_kind::COMMON {
            let align = sym.st_value.max(1);
            if let Some(&at) = commons.get(&m.symbol) {
                let c = &mut acc.commons[at];
                c.1 = c.1.max(sym.st_size);
                c.2 = c.2.max(align);
            } else {
                commons.insert(m.symbol, acc.commons.len());
                acc.commons.push((m.symbol, sym.st_size, align));
            }
        }
        if copies
            && m.kind == def_kind::SECTION
            && m.bind != 0
            && res.sections.get(SectionId(m.section)).is_some_and(|r| r.comdat.get().is_some())
        {
            acc.copies.push((m.symbol, index, sym.st_size, m.typ));
        }
    }
    if live {
        acc.mentions = rows;
        let range = res.section_base[file];
        let mut groups: Vec<&[u8]> = Vec::new();
        for sid in range.start.0..range.end.0 {
            let Some(row) = res.sections.get(SectionId(sid)) else { continue };
            let Some(c) = row.comdat.get().and_then(|c| inputs.comdats.get(c)) else {
                continue;
            };
            groups.push(c.name);
        }
        groups.sort_unstable();
        groups.dedup();
        acc.groups = groups;
    }
    Ok(acc)
}

#[allow(clippy::too_many_lines, clippy::too_many_arguments)] // one block per table
fn rows5(
    link: &Link<'_, '_>,
    keys: &Keys<'_>,
    syms: &mut Symbols<'_, '_>,
    strings: &mut Strings,
    four: &State,
    layout: &Layout,
    extra: &Link5<'_>,
    live_frags: &[(u32, [u8; 16], u32, u64)],
    pool: &Pool,
) -> Result<Rows5, String> {
    use crate::format::def_kind;
    use crate::update::rows::{
        CommonRow, CopyRow, FragRow, FragUseRow, GroupRow, KeyRefRow, MentionRow, key_ref_flags,
    };
    use mortar_core::resolution::FileLive;
    let inputs = link.inputs;
    let res = link.resolution;
    let n_files = inputs.files.len() as usize;
    let mut out = Rows5::default();
    let live_file = |f: FileId| res.file_live.get(f).is_some_and(|l| *l != FileLive::Dead);
    // ---- the files' symbol tables: MENTIONS, KEY_REFS, COMMONS, COPIES,
    // GROUPS, `versioned`, `first_global` (one task per file).
    let per_file = pool.map(n_files, |n| file5(inputs, res, extra.copies, FileId(n as u32)));
    let mut files5 = Vec::with_capacity(n_files);
    for f in per_file {
        files5.push(f?);
    }
    // ---- demand per (file, key), and SLOTS.refs (WP-239 S4-4, S4-5).
    let mut demand: HashMap<(u32, u32), u16> = HashMap::default();
    let mut slot_files: HashMap<u32, [u32; 6]> = HashMap::default();
    for fd in extra.demands {
        if !live_file(fd.file) {
            return Err(format!("file {} has demand but is not live", fd.file.0));
        }
        for d in &fd.demands {
            let SymKind::Global(id) = d.key.kind() else { continue };
            *demand.entry((fd.file.0, id.0)).or_default() |= d.flags.bits();
            for (k, kind) in SLOT_LISTS.iter().enumerate() {
                if d.flags.intersects((extra.slot_demand)(*kind)) {
                    slot_files.entry(id.0).or_default()[k] += 1;
                }
            }
        }
    }
    // ---- KEY_REFS (IN-18e: MENTIONS transposed, the LAZY members'
    // definitions, file 0's linker-defined keys), MENTIONS, COMMONS,
    // COPIES, GROUPS.
    let mut used = 0usize;
    let mut lazy_rows = 0usize;
    let mut lazy_files = 0usize;
    for (n, f) in files5.iter().enumerate() {
        let file = n as u32;
        if f.lazy && !f.refs.is_empty() {
            lazy_files += 1;
        }
        for &(id, def, bind, typ, flags) in &f.refs {
            let d = demand.get(&(file, id)).copied().unwrap_or(0);
            if d != 0 {
                used += 1;
            }
            if f.lazy {
                lazy_rows += 1;
            }
            out.key_refs.push(KeyRefRow {
                key: syms.row(strings, SymKey::global(SymbolId(id)))?,
                file,
                def,
                bind,
                typ,
                flags,
                demand: d,
                tomb: false,
            });
        }
        for m in &f.mentions {
            let row = MentionRow {
                symbol: syms.row(strings, SymKey::global(SymbolId(m.symbol)))?,
                kind: m.kind,
                bind: m.bind,
                typ: m.typ,
                other: m.other,
                section: m.section,
            };
            out.mentions.push((FileId(file), row));
        }
        for &(id, size, align) in &f.commons {
            let key = syms.row(strings, SymKey::global(SymbolId(id)))?;
            out.commons.push(CommonRow { key, file, size, align, tomb: false });
        }
        for &(id, index, size, typ) in &f.copies {
            let key = syms.row(strings, SymKey::global(SymbolId(id)))?;
            out.copies.push(CopyRow { key, file, index, size, typ, tomb: false });
        }
        for g in &f.groups {
            out.groups.push(GroupRow { signature: strings.add(g), file, tomb: false });
        }
    }
    if used != demand.len() {
        // A demand on a key the file has no row of: a reference the scan
        // redirected (`--wrap`), which no stored row carries.
        return Err("a file's demand names a key it does not mention (--wrap)".into());
    }
    let mut internal = 0usize;
    for (id, &def) in res.def.iter() {
        if let mortar_core::resolution::Def::Internal(_) = def {
            let other = res.props.get(id).map_or(0, |p| p.visibility);
            out.key_refs.push(KeyRefRow {
                key: syms.row(strings, SymKey::global(id))?,
                file: 0,
                def: def_kind::INTERNAL,
                flags: other & key_ref_flags::VISIBILITY,
                ..KeyRefRow::default()
            });
            internal += 1;
        }
    }
    out.key_refs.sort_by_key(|r| (r.key, r.file));
    out.key_refs.dedup_by_key(|r| (r.key, r.file));
    // ---- SLOTS.refs: parallel to format 4's rows (`SLOT_LISTS` order).
    let mut zero = 0usize;
    for (k, kind) in SLOT_LISTS.iter().enumerate() {
        for &key in slot_list(&layout.slots, *kind) {
            let count = match key.kind() {
                SymKind::Local(_) => 1,
                SymKind::Global(id) => slot_files.get(&id.0).map_or(0, |c| c[k]),
            };
            if count == 0 {
                zero += 1;
            }
            out.slot_refs.push(count);
        }
    }
    if out.slot_refs.len() != four.slots.len() {
        return Err("SLOTS: the lists changed under the capture".into());
    }
    // ---- SYMBOLS' first LIVE mentioner (IN-71, WP-214 F-12) and
    // RESOLVED.facts (R-4a).
    let mut first = vec![(NONE, 0u32); inputs.symbols.len() as usize];
    for (file, _) in inputs.files.iter() {
        if !live_file(file) {
            continue;
        }
        for (i, id) in inputs.sym_map_of(file).iter().enumerate() {
            if let Some(slot @ (NONE, _)) = first.get_mut(id.0 as usize) {
                *slot = (file.0, i as u32);
            }
        }
    }
    // ---- EDGES (WP-238 S1): the producer's rows, in state ids (a
    // `SECTIONS` row is its `SectionId` at the capture; a key its
    // `SYMBOLS` row), each checked against its kind's target class.
    let before = syms.rows.len();
    let mut refuse = 0usize;
    for e in extra.edges.unwrap_or_default() {
        use crate::update::rows::{EdgeRow, edge_kind};
        use mortar_core::incr::LiveTarget;
        let bad = || format!("EDGES: a row of kind {} with the wrong target", e.kind);
        let target = match (edge_kind::target(e.kind), e.target) {
            (Some(edge_kind::Target::Key), LiveTarget::Key(k)) => {
                syms.row(strings, SymKey::global(k))?
            }
            (Some(edge_kind::Target::Section), LiveTarget::Section(t)) => {
                res.sections.get(t).ok_or("EDGES: a target section out of range")?;
                t.0
            }
            (Some(edge_kind::Target::Zero), LiveTarget::None) => 0,
            (Some(edge_kind::Target::Code), LiveTarget::Code(c)) => c,
            _ => return Err(bad()),
        };
        let (file, _) = res.sh_index(e.source).ok_or("EDGES: a source section without index")?;
        let row = EdgeRow { kind: e.kind, source: e.source.0, target, offset: e.offset };
        if !crate::state5::edge_row_ok(&row) {
            return Err(bad());
        }
        if e.kind == edge_kind::REFUSE {
            refuse += 1;
        }
        out.edges.push((file, row));
    }
    if syms.rows.len() != before {
        return Err("EDGES: a key without a format-4 row".into());
    }
    out.edges.sort_unstable_by_key(|&(f, r)| (f.0, r));
    out.edges.dedup();
    let n_rows = syms.rows.len();
    out.symbols = vec![None; n_rows];
    // SYMTAB_KEYS (WP-262): the symbol table plan's own walk, by row.
    let none = crate::update::rows::SymtabKeyRow { index: NONE, strtab: 0 };
    out.symtab_keys = vec![none; n_rows];
    let entries = extra.symtab.map(|(area, entries, blocks, tables)| {
        out.symtab_area = Some(area);
        out.symtab_blocks = blocks.to_vec();
        out.symtab_tables = Some(tables);
        entries
    });
    if entries.is_some_and(|e| e.len() != first.len()) {
        return Err("SYMTAB_KEYS: the symbol table's keys are not this link's".into());
    }
    for (i, &(at_file, at_index)) in first.iter().enumerate() {
        let id = SymbolId(i as u32);
        let row = syms.row(strings, SymKey::global(id))? as usize;
        let facts = extra.facts.get(i).copied().ok_or("RESOLVED.facts: a key without facts")?;
        if let Some(x) = out.symbols.get_mut(row) {
            *x = Some((at_file, at_index, facts));
        }
        if let Some(&(index, strtab)) = entries.and_then(|e| e.get(i))
            && index != NONE
            && let Some(x) = out.symtab_keys.get_mut(row)
        {
            *x = crate::update::rows::SymtabKeyRow { index, strtab };
        }
    }
    if syms.rows.len() != n_rows {
        return Err("SYMBOLS: a key without a format-4 row".into());
    }
    // ---- FRAGS (out, class, hash) and FRAG_USE (WP-239 S4-1, S4-2; R-1:
    // no `.debug_*` row).
    let outsec_debug: Vec<bool> = link
        .assignment
        .outsecs
        .ids()
        .iter()
        .map(|id| {
            mortar_core::identity::outsec_identity(&keys.outsecs, id)
                .is_some_and(|o| o.name.starts_with(b".debug_"))
        })
        .collect();
    let debug = |out: u32| outsec_debug.get(out as usize).copied().unwrap_or(false);
    let split = link.split;
    let n_frags = split.frags.len() as usize;
    let mut class = vec![NONE; n_frags];
    for (row, m) in split.merge_of.iter().enumerate() {
        let c = extra.classes.get(row).ok_or("a merged section without class")?.word();
        for p in m.pieces.slots() {
            if let Some(&frag) = split.piece_frag.get(p)
                && let Some(x) = class.get_mut(frag.0 as usize)
            {
                *x = c;
            }
        }
    }
    let mut hash: Vec<Option<[u8; 16]>> = vec![None; n_frags];
    for &(o, h, frag, offset) in live_frags {
        if debug(o) {
            continue;
        }
        hash[frag as usize] = Some(h);
        let c = class[frag as usize];
        if c == NONE {
            return Err("a live fragment without class".into());
        }
        // FRAGS.size (seam 6): the fragment's bytes, a function of its
        // identity (`Frag::size`, the bytes `frag_hash` read).
        let size = split.frags.get(FragId::from_index(frag)).ok_or("a fragment")?.size;
        out.frags.push(FragRow { out: o, class: c, hash: h, offset, size, refs: 0, tomb: false });
    }
    out.frags.sort_by_key(|r| (r.out, r.class, r.hash, r.offset));
    out.frags.dedup_by_key(|r| (r.out, r.class, r.hash, r.offset));
    for (row, m) in split.merge_of.iter().enumerate() {
        let Some(osec) = link.assignment.out(m.section) else { continue };
        if debug(osec.0) || !extra.liveness.is_live(m.section) {
            continue;
        }
        let (file, _) = res.sh_index(m.section).ok_or("a merged section without index")?;
        let c = extra.classes.get(row).ok_or("a merged section without class")?.word();
        for p in m.pieces.slots() {
            let Some(&frag) = split.piece_frag.get(p) else { continue };
            let Some(Some(h)) = hash.get(frag.0 as usize) else { continue };
            let r = FragUseRow { file: file.0, out: osec.0, class: c, hash: *h };
            out.frag_uses.push((file, r));
        }
    }
    out.frag_uses.sort_unstable_by_key(|(f, r)| (f.0, *r));
    out.frag_uses.dedup();
    // ---- FILES: LAZY, the eh columns, `first_global`.
    let eh = extra.eh.map_err(|why| format!("no .eh_frame ranges: {why}"))?;
    out.files = files5
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let file = FileId(i as u32);
            let round = file_round(res, file)?;
            Ok(FileCols {
                flags: f.flags | if f.lazy { crate::update::rows::file_flags::LAZY } else { 0 },
                first_global: f.first_global,
                round,
                ..FileCols::default()
            })
        })
        .collect::<Result<_, String>>()?;
    for e in eh {
        let [offset, capacity, records] =
            crate::state5::eh_columns(e.offset, e.capacity, u64::from(e.records))
                .map_err(|r| r.report())?;
        let row = out.files.get_mut(e.file.0 as usize).ok_or("an .eh_frame range's file")?;
        row.eh = [offset, capacity, records, e.fdes];
        // `!movable` (a KEPT CIE's encoding) implies the bytes' bit: or-ed
        // as a guard, never a second definition
        if !e.movable {
            row.flags |= EH_FIXED;
        }
    }
    // ---- FILES.stack, FILES.properties (seam 6): 0 and empty for a LAZY
    // row (it takes no part until extracted).
    for &(file, b) in extra.stack {
        let row = out.files.get_mut(file.0 as usize).ok_or("a stack note's file")?;
        if row.flags & crate::update::rows::file_flags::LAZY == 0 {
            row.stack = b;
        }
    }
    for (file, bytes) in extra.properties {
        let row = out.files.get_mut(file.0 as usize).ok_or("a property note's file")?;
        if row.flags & crate::update::rows::file_flags::LAZY == 0 {
            row.properties.clone_from(bytes);
        }
    }
    let fixed = out.files.iter().filter(|f| f.flags & EH_FIXED != 0).count();
    let exports =
        out.files.iter().filter(|f| f.flags & crate::update::rows::file_flags::DEBUG_EXPORTS != 0);
    let exports = exports.count();
    out.option_keys = extra.option_keys.to_vec();
    out.versioned = files5.iter().map(|f| f.versioned).sum();
    if res.iterations == 0 {
        return Err("META.iterations 0: no closure ran (IN-71b)".into());
    }
    out.iterations = res.iterations;
    out.gnu_warning_orphans = res.gnu_warning_orphans;
    let debug_note;
    (out.debug_strs, debug_note) = debug_str_rows(link, keys, layout, live_frags)?;
    out.notes = vec![
        format!(
            "KEY_REFS {} (lazy {lazy_rows} of {lazy_files} files, linker-defined {internal}), demand on {used}",
            out.key_refs.len()
        ),
        format!("MENTIONS {} (positional)", out.mentions.len()),
        format!("FRAGS {} FRAG_USE {}", out.frags.len(), out.frag_uses.len()),
        format!("SLOTS {} (refs 0: {zero})", out.slot_refs.len()),
        format!(
            "GROUPS {} COMMONS {} COPIES {}",
            out.groups.len(),
            out.commons.len(),
            out.copies.len()
        ),
        format!(
            "eh files {} (fixed encodings {fixed}), versioned {}, debug exports {exports}",
            eh.len(),
            out.versioned
        ),
        debug_note,
    ];
    if extra.edges.is_some() {
        out.notes.push(format!("EDGES {} (REFUSE {refuse})", out.edges.len()));
    }
    Ok(out)
}

/// `DEBUG_STRS` of a full link (WP-247; WP-228 R-2, seam 8; incremental
/// mode only, a plain link never comes here): per distinct string of the
/// output's `.debug_str` (class 0) and `.debug_line_str` (class 1), its
/// key `name_hash(bytes, b"")` (the string's bytes and its NUL, hashed)
/// and its offset in the OUTPUT section; sorted by key, of two strings of
/// one key the LOWER offset kept; no row at or past 2^32 (an accelerator:
/// such a string is a miss, appended by an update). The strings are the
/// section's merged fragments (the link's `MergeMap`, the fragments
/// `live_frags` lists), so the rows are exactly the strings the image
/// wrote; the merged chunk is the section (`frag_off` is from its start).
/// A class whose output section holds anything but its merged chunk (an
/// input `.debug_str` without `SHF_MERGE`, read whole) gets NO rows: every
/// string a miss, never a wrong offset (the note says so). Also returns
/// the note for the `incremental: state5` lines.
///
/// # Errors
/// A fragment whose bytes cannot be read.
fn debug_str_rows(
    link: &Link<'_, '_>,
    keys: &Keys<'_>,
    layout: &Layout,
    live_frags: &[(u32, [u8; 16], u32, u64)],
) -> Result<(Vec<crate::update::rows::DebugStrRow>, String), String> {
    use crate::update::rows::{DebugStrRow, debug_str_class, name_hash};
    // WP-247's measurement (WP-228 R-2.4: the capture with and without
    // the rows, one binary): a `testhooks` build with
    // `MORTAR_INCR_NO_DEBUG_STRS` set writes none.
    #[cfg(feature = "testhooks")]
    if std::env::var_os("MORTAR_INCR_NO_DEBUG_STRS").is_some() {
        return Ok((Vec::new(), "DEBUG_STRS off (MORTAR_INCR_NO_DEBUG_STRS)".to_owned()));
    }
    let mut classes: Vec<(u32, u8)> = Vec::new();
    for id in link.assignment.outsecs.ids().iter() {
        let Some(o) = mortar_core::identity::outsec_identity(&keys.outsecs, id) else { continue };
        let class = match o.name {
            b".debug_str" => debug_str_class::STR,
            b".debug_line_str" => debug_str_class::LINE_STR,
            _ => continue,
        };
        classes.push((id.0, class));
    }
    let mut rows = Vec::new();
    let mut notes = Vec::new();
    let mut buf = Vec::new();
    for (out, class) in classes {
        // The merged chunk, and whether its section header holds another.
        let shndx = layout
            .placement
            .chunks
            .iter()
            .find(|(_, c)| matches!(c.kind, ChunkKind::Merged(o) if o.0 == out))
            .and_then(|(_, c)| c.shndx);
        let alone = shndx.is_some_and(|x| {
            layout.placement.chunks.iter().filter(|(_, c)| c.shndx == Some(x)).count() == 1
        });
        if !alone {
            notes.push(format!("class {class}: not one merged chunk, no rows"));
            continue;
        }
        let at = live_frags.partition_point(|f| f.0 < out);
        let mut n = 0usize;
        let mut far = 0usize;
        for &(o, _, frag, off) in &live_frags[at..] {
            if o != out {
                break;
            }
            let Ok(offset) = u32::try_from(off) else {
                far += 1;
                continue;
            };
            let bytes = frag_bytes(link, FragId::from_index(frag), &mut buf)
                .ok_or("a debug string fragment's bytes")?;
            let string = bytes.strip_suffix(&[0]).unwrap_or(bytes);
            rows.push(DebugStrRow { out: class, hash: name_hash(string, b""), offset });
            n += 1;
        }
        notes.push(format!("class {class}: {n} strings, {far} at or past 2^32"));
    }
    let rows = crate::state5::compact::debug_strs_whole(&rows);
    let note = format!("DEBUG_STRS {} ({})", rows.len(), notes.join("; "));
    Ok((rows, note))
}

/// A fragment's bytes (`frag_hash`'s read): `buf` holds them.
fn frag_bytes<'b>(link: &Link<'_, '_>, frag: FragId, buf: &'b mut Vec<u8>) -> Option<&'b [u8]> {
    let f = link.split.frags.get(frag)?;
    let (file, index) = link.resolution.sh_index(f.section)?;
    let Reader::Object(object) = link.inputs.files.get(file)?.reader else { return None };
    let range = object.section(index).ok()?.file_range().ok()??;
    let start = range.start as u64 + u64::from(f.offset);
    let range = start..start + u64::from(f.size);
    buf.clear();
    if let Some(bytes) = link.inputs.resident(file, range.clone()) {
        buf.extend_from_slice(bytes);
    } else {
        buf.resize(f.size as usize, 0);
        link.inputs.copy_to(file, range, buf).ok()?;
    }
    Some(buf)
}

/// The format-5 state of a full link (WP-244): `state`, the link's
/// format-4 capture completed and bound, with `rows`, captured beside it
/// by [`capture5_timed`]; checked (`state5::check_capture`), normalized
/// and encoded: `encode(normalize(capture))`. `collide` is the test
/// injection of `testhook::state5_collide` (the first `FRAGS` row again at
/// another offset).
///
/// # Errors
/// The report of `state-collision` or `state-limit`, or why the rows do
/// not make a state.
pub fn state5_bytes(state: &State, rows: Rows5, collide: bool) -> Result<Vec<u8>, String> {
    state5_encode(&state5_rows(state, rows, collide)?)
}

/// [`state5_bytes`]' rows: `state` and `rows` as one format-5 state,
/// checked (`check_capture`) and normalized (`compact::normalize`), not
/// yet encoded. Row-level tests read seam 6's columns here (format 5's
/// bytes do not hold them until U-2 encodes them).
///
/// # Errors
/// As [`state5_bytes`], but the encoding's.
#[allow(clippy::too_many_lines)]
pub fn state5_rows(
    state: &State,
    rows: Rows5,
    collide: bool,
) -> Result<crate::update::testing::MemoryState, String> {
    use crate::update::rows::sym_kind;
    use crate::update::testing::MemoryState;
    let mut m = MemoryState::from_format4(state);
    if rows.files.len() != m.files.len()
        || rows.symbols.len() != m.symbols.len()
        || rows.sections.len() != m.sections.len()
    {
        return Err("the format-5 rows are not this state's".into());
    }
    for (row, facts) in m.sections.iter_mut().zip(&rows.sections) {
        put_section_facts(row, facts)?;
    }
    // FILES.properties: each distinct value interned once.
    let mut interned: HashMap<Vec<u8>, Str> = HashMap::default();
    for (i, (f, c)) in m.files.iter_mut().zip(rows.files).enumerate() {
        f.flags |= c.flags;
        [f.eh_offset, f.eh_capacity, f.eh_records, f.eh_fdes] = c.eh;
        f.stack = c.stack;
        f.round = c.round;
        if !c.properties.is_empty() {
            f.properties = if let Some(&v) = interned.get(&c.properties) {
                v
            } else {
                let v = Str {
                    off: u32::try_from(m.strings.len()).map_err(|_| "STRINGS over 4 GiB")?,
                    len: u32::try_from(c.properties.len()).map_err(|_| "a property list")?,
                };
                m.strings.extend_from_slice(&c.properties);
                interned.insert(c.properties, v);
                v
            };
        }
        if let Some(x) = m.file_index.get_mut(i) {
            x.first_global = c.first_global;
        }
    }
    m.mentions = rows.mentions;
    m.key_refs = rows.key_refs;
    m.frags = rows.frags;
    m.frag_uses = rows.frag_uses;
    m.groups = rows.groups;
    m.commons = rows.commons;
    m.copies = rows.copies;
    m.edges = rows.edges;
    m.meta.versioned = rows.versioned;
    m.meta.iterations = rows.iterations;
    m.debug_strs = rows.debug_strs;
    if rows.symtab_keys.len() != m.symbols.len() {
        return Err("SYMTAB_KEYS: the rows are not this state's".into());
    }
    m.symtab_keys = rows.symtab_keys;
    symtab_keys_check(&m, rows.symtab_area)?;
    // SYMTAB and META.symtab (WP-263): each file's block; file 0's range
    // the global area, then the internal file's block when it has one
    // (IN-18e as WP-263 F-1 proposed it; WP-264).
    m.meta.gnu_warning_orphans = rows.gnu_warning_orphans;
    if let (Some(area), Some(tables)) = (rows.symtab_area, rows.symtab_tables) {
        m.meta.symtab = tables;
        m.symtab = vec![(FileId(0), area)];
        m.symtab.extend(rows.symtab_blocks.iter().copied());
    }
    symtab_rows_check(&m, &rows.symtab_blocks, rows.symtab_area, rows.symtab_tables)?;
    // REGIONS (WP-264): the full link's regions, from its stored placement.
    m.regions = region_rows(&m)?;
    regions_check(&m)?;
    if rows.slot_refs.len() != m.slots.len() {
        return Err("SLOTS: the rows are not this state's".into());
    }
    for (r, refs) in m.slots.iter_mut().zip(rows.slot_refs) {
        r.refs = refs;
    }
    for (i, x) in rows.symbols.into_iter().enumerate() {
        let Some((at_file, at_index, facts)) = x else { continue };
        m.symbols[i].at_file = at_file;
        m.symbols[i].at_index = at_index;
        if let Some(r) = m.resolved.get_mut(i) {
            r.facts = facts;
        }
    }
    // SYMBOLS.option (seam 6): the rows `Plan::option_keys` names, matched
    // as `compact::live_keys` matches an option name (split at the first
    // `@`; `foo@@V` is `foo`).
    if !rows.option_keys.is_empty() {
        let mut named: mortar_core::hash::HashSet<(&[u8], &[u8])> =
            mortar_core::hash::HashSet::default();
        for b in &rows.option_keys {
            named.insert(option_key_of(b));
        }
        let text = |x: Str| m.strings.get(x.off as usize..(x.off + x.len) as usize);
        let mut set = Vec::new();
        let mut found: mortar_core::hash::HashSet<(&[u8], &[u8])> =
            mortar_core::hash::HashSet::default();
        for (i, r) in m.symbols.iter().enumerate() {
            if !matches!(r.kind, sym_kind::GLOBAL | sym_kind::VERSIONED) {
                continue;
            }
            if let (Some(n), Some(v)) = (text(r.name), text(r.version))
                && named.contains(&(n, v))
            {
                set.push(i);
                found.insert((n, v));
            }
        }
        // WP-247 item b: a name an option roots (entry, `-u`, `EXTERN`,
        // `--require-defined`, `-init`, `-fini`) that no file mentions has
        // no key in the link: its row is added here, unmentioned, with the
        // bit (IN-71b: never dead while the option names it), in the
        // options' order; the compaction sorts it among the unmentioned.
        let mut missing: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        for b in &rows.option_keys {
            let key = option_key_of(b);
            if !found.contains(&key) && !missing.iter().any(|(n, v)| (&n[..], &v[..]) == key) {
                missing.push((key.0.to_vec(), key.1.to_vec()));
            }
        }
        for i in set {
            m.symbols[i].option = true;
        }
        for (name, version) in missing {
            let mut add = |b: &[u8]| -> Result<Str, String> {
                let off = u32::try_from(m.strings.len()).map_err(|_| "STRINGS over 4 GiB")?;
                m.strings.extend_from_slice(b);
                Ok(Str { off, len: u32::try_from(b.len()).map_err(|_| "an option's name")? })
            };
            let (name, version) = (add(&name)?, add(&version)?);
            m.symbols.push(SymbolRow {
                kind: if version.len == 0 { sym_kind::GLOBAL } else { sym_kind::VERSIONED },
                file: NONE,
                name,
                version,
                at_file: NONE,
                option: true,
                ..SymbolRow::default()
            });
        }
    }
    // DIAGS (WP-265): the link's warnings, checked by `diag_rows`; their
    // keys are rows of this state, the options' rows included.
    if !rows.diags.is_empty() {
        let text = |x: Str| m.strings.get(x.off as usize..(x.off + x.len) as usize).unwrap_or(&[]);
        let resolved = resolve_diag_keys(
            rows.diags,
            m.symbols.iter().map(|r| (r.kind, text(r.name), text(r.version))),
        )?;
        m.diags = resolved;
    }
    // The rows added for the options' names are no key of the link: none
    // has a `.symtab` entry.
    m.symtab_keys
        .resize(m.symbols.len(), crate::update::rows::SymtabKeyRow { index: NONE, strtab: 0 });
    if collide && let Some(&r) = m.frags.first() {
        m.frags.push(crate::update::rows::FragRow { offset: r.offset + 1, ..r });
    }
    crate::state5::check_capture(&m).map_err(|r| r.report())?;
    crate::state5::compact::normalize(&m).map_err(|e| e.to_string())
}

/// An option's operand as its key (`OptionKey::name`'s rule, as
/// `compact::live_keys` matches it): split at the first `@`; `foo@@V` is
/// `foo` with no version.
fn option_key_of(b: &[u8]) -> (&[u8], &[u8]) {
    match b.iter().position(|&c| c == b'@') {
        None => (b, &[][..]),
        Some(i) if b.get(i + 1) == Some(&b'@') => (&b[..i], &[][..]),
        Some(i) => (&b[..i], &b[i + 1..]),
    }
}

/// The names whose `SYMBOLS` rows carry the option bit (seam 6's
/// `SYMBOLS.option`, WP-247 item b): `Plan::option_keys` (`--wrap`'s
/// three names, `--defsym`'s symbol and the symbol its expression names)
/// then the names [`OptionKeys::live_roots`] lists that are not among
/// them (the entry, `-u` and `EXTERN`, `--require-defined`, `--defsym`'s
/// operand, `-init`, `-fini`). The front passes it to the capture
/// (`Incr5.option_keys`).
///
/// [`OptionKeys::live_roots`]: mortar_core::incr::OptionKeys::live_roots
#[must_use]
pub fn option_names(plan_keys: Vec<Vec<u8>>, keys: &mortar_core::incr::OptionKeys) -> Vec<Vec<u8>> {
    let mut out = plan_keys;
    for (name, _) in keys.live_roots() {
        if !out.iter().any(|n| n == name) {
            out.push(name.to_vec());
        }
    }
    out
}

/// [`state5_rows`]' bytes (`state5::write::encode`).
///
/// # Errors
/// The encoding's (a table past its limits).
pub fn state5_encode(m: &crate::update::testing::MemoryState) -> Result<Vec<u8>, String> {
    crate::state5::write::encode(m).map_err(|e| e.to_string())
}

// ---- DIAGS (IN-75a; WP-265) ----------------------------------------------------

/// One warning's row before its key is a `SYMBOLS` row: the name it is
/// looked up by, and the rest of [`crate::update::diags::row`]'s inputs.
struct PendingDiag {
    key: mortar_diag::Key,
    d: mortar_diag::Diagnostic,
    /// The key subject by (name, version), when the code has one.
    name: Option<(Vec<u8>, Vec<u8>)>,
    /// Subjects other than the key (files, the link).
    subjects: Vec<crate::update::diags::Subject>,
    refs: Vec<crate::update::diags::DiagRef>,
    /// A dormant row (WP-221 §2.4): made by `diags::dormant`.
    dormant: Option<FileId>,
}

/// `DIAGS`' rows of a full link (IN-75a; WP-265): one per warning of
/// `warnings` (the collector's, `mortar_diag::Collector::warnings`, in
/// report order), then one dormant row per warned key of `gnu_warnings`
/// (`Front::gnu_warnings`: (key, holder, section), by key) that no warning
/// names, in the capture's ids (a `FILES` row is its `FileId`, a key its
/// format-4 `SYMBOLS` row in `four`; `compact::normalize` maps both).
///
/// Subjects (diag.md's subjects column, `mortar_diag::subject::subjects`):
/// - a reference-site warning (`gnu-warning` of a key, `undefined-symbol`;
///   phase 70): its key; for `gnu-warning` also the holder of the key's
///   first `.gnu.warning.<key>` section (the one the scan's text is
///   from); its references the listed `referenced by` places, each the
///   relocation the scan numbers (section index × 2^32 + record), its
///   count the diagnostic's own (more than three: a capped row, which an
///   update that touches it falls back on, `diags::Site::unchanged`);
/// - any other warning: every file its place and context lines name, its
///   key (the kind's symbol) when the code's column has one, the link when
///   the column has it.
///
/// The check (write time): every row reads back (`diags::diagnostic`) to
/// its warning's model, and renders to its bytes; no two rows share
/// `DIAGS`' key (phase, unit, seq, code, text).
///
/// # Errors
/// A warning whose subjects are not known here, named (its code and why):
/// the link writes no state (the next link is a full one, `no-state`).
pub fn diag_rows(
    inputs: &Inputs<'_>,
    gnu_warnings: &[(SymbolId, FileId, SectionIndex)],
    four: &State,
    warnings: &[(mortar_diag::Key, mortar_diag::Diagnostic)],
) -> Result<Vec<crate::update::diags::DiagRow>, String> {
    let named = diag_rows_named(inputs, gnu_warnings, warnings)?;
    resolve_diag_keys(
        named,
        four.symbols.iter().map(|r| (r.kind, four.str(r.name), four.str(r.version))),
    )
}

/// A `DIAGS` row whose key subject is still a name ([`diag_rows_named`]):
/// the row holds `Subject::Key(SymbolId(NONE))` for it until
/// [`resolve_diag_keys`] puts the key's `SYMBOLS` row.
type NamedDiag = (crate::update::diags::DiagRow, Option<(Vec<u8>, Vec<u8>)>);

/// [`diag_rows`]' rows, each key by its name (`state5_rows` resolves them
/// after it adds the rows of the options' unmentioned names: an
/// `entry-not-found` warning's key is one).
#[allow(clippy::too_many_lines, reason = "each code's subjects, in one place")]
fn diag_rows_named(
    inputs: &Inputs<'_>,
    gnu_warnings: &[(SymbolId, FileId, SectionIndex)],
    warnings: &[(mortar_diag::Key, mortar_diag::Diagnostic)],
) -> Result<Vec<NamedDiag>, String> {
    use crate::update::diags::{DiagRef, Subject, reference_site};
    use mortar_diag::subject::{SubjectKind, subjects};
    use mortar_diag::{Context, Diagnostic, FileRef, Kind, Place, Relation, Style};
    // The scan's key of a warned key is its FIRST `.gnu.warning.<key>`
    // section of a live file (`mortar_scan::report::gnu_warning`).
    let mut holders: Vec<(SymbolId, FileId, SectionIndex)> = Vec::new();
    for &w in gnu_warnings {
        if holders.last().is_none_or(|h| h.0 != w.0) {
            holders.push(w);
        }
    }
    if warnings.is_empty() && holders.is_empty() {
        return Ok(Vec::new());
    }
    let refuse = |code: &str, why: &str| format!("DIAGS: a warning {code}: {why} (IN-75a)");
    let object = |f: FileId| match inputs.files.get(f).map(|x| &x.reader) {
        Some(Reader::Object(o)) => Some(*o),
        _ => None,
    };
    let text_of = |w: &(SymbolId, FileId, SectionIndex)| -> Option<String> {
        let data = object(w.1)?.section(w.2).and_then(|s| s.data()).ok()?;
        let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
        Some(String::from_utf8_lossy(&data[..end]).into_owned())
    };
    // Each display a file of the link has, once (two files of one display:
    // `None`, refused when a warning names it).
    let mut displays: std::collections::BTreeMap<FileRef, Option<FileId>> =
        std::collections::BTreeMap::new();
    for (id, f) in inputs.files.iter() {
        displays.entry(f.display()).and_modify(|e| *e = None).or_insert(Some(id));
    }
    let file_of = |code: &str, r: &FileRef| -> Result<FileId, String> {
        displays.get(r).copied().flatten().ok_or_else(|| {
            refuse(code, &format!("it names {r:?}, which is not one file of the link"))
        })
    };
    // The relocations of section `index` of `file` (`SHT_RELA` or `SHT_REL`
    // whose `sh_info` names it): each record's (r_offset, symbol name).
    let relocs = |file: FileId, index: u32| -> Option<Vec<(u64, Vec<u8>)>> {
        let o = object(file)?;
        let symtab = o.symbols().ok()??;
        let mut out = None;
        for s in o.sections() {
            let t = s.sh_type();
            if !matches!(t, mortar_elf::consts::SHT_RELA | mortar_elf::consts::SHT_REL)
                || s.info_section().ok()?.0 != index
            {
                continue;
            }
            if out.is_some() {
                return None;
            }
            let name = |r_sym: u32| -> Option<Vec<u8>> {
                let sym = symtab.get(mortar_elf::SymbolIndex(r_sym)).ok()?;
                Some(symtab.name(&sym).ok()?.to_vec())
            };
            let mut v = Vec::new();
            if t == mortar_elf::consts::SHT_RELA {
                for r in &s.rela().ok()? {
                    v.push((r.r_offset, name(r.r_sym)?));
                }
            } else {
                for r in &s.rel().ok()? {
                    v.push((r.r_offset, name(r.r_sym)?));
                }
            }
            out = Some(v);
        }
        out
    };
    let mut pending: Vec<PendingDiag> = Vec::with_capacity(warnings.len() + holders.len());
    let mut warned: Vec<SymbolId> = Vec::new();
    for (key, d) in warnings {
        let code = d.id();
        let column = subjects(code);
        if column.is_empty() {
            return Err(refuse(code, "its code has no subjects in diag.md's table"));
        }
        let site = reference_site(code) && key.phase == 70;
        let mut p = PendingDiag {
            key: *key,
            d: d.clone(),
            name: None,
            subjects: Vec::new(),
            refs: Vec::new(),
            dormant: None,
        };
        if site {
            // The first reference is the key's (`undefined_key`).
            let first_file = FileId(key.unit);
            let (first_index, first_record) = ((key.seq >> 32) as u32, key.seq as u32);
            let first = relocs(first_file, first_index)
                .and_then(|v| v.get(first_record as usize).cloned())
                .ok_or_else(|| refuse(code, "its first reference is not a relocation"))?;
            let name: Vec<u8> = if code == "gnu-warning" {
                let Kind::GnuWarning { text } = d.kind() else {
                    return Err(refuse(code, "not a gnu-warning kind"));
                };
                let hit: Vec<&(SymbolId, FileId, SectionIndex)> = holders
                    .iter()
                    .filter(|h| inputs.symbols.get(h.0).is_some_and(|s| s.name == &first.1[..]))
                    .collect();
                let [h] = hit[..] else {
                    return Err(refuse(code, "its key is not one warned key"));
                };
                if text_of(h).as_deref() != Some(text.as_str()) {
                    return Err(refuse(code, "its text is not its holder's"));
                }
                let k = inputs.symbols.get(h.0).ok_or_else(|| refuse(code, "a key"))?;
                if k.version.is_some() {
                    return Err(refuse(code, "a versioned key"));
                }
                warned.push(h.0);
                p.subjects.push(Subject::File(h.1));
                first.1.clone()
            } else {
                let n = d.kind().symbol().ok_or_else(|| refuse(code, "no symbol"))?;
                if n.as_bytes() != &first.1[..] {
                    return Err(refuse(code, "its first reference is not to its key"));
                }
                n.as_bytes().to_vec()
            };
            if d.place().is_some() {
                return Err(refuse(code, "a reference-site warning with a place"));
            }
            for c in d.context() {
                let (file, section, offset) = match c {
                    Context::At(
                        Relation::ReferencedBy,
                        Place::Section { file, section, offset },
                    ) => (file, section, *offset),
                    Context::Note(_) => continue,
                    Context::At(..) => {
                        return Err(refuse(code, "a context line not a reference"));
                    }
                };
                let f = file_of(code, file)?;
                let o = object(f).ok_or_else(|| refuse(code, "a reference not in an object"))?;
                let mut found: Vec<DiagRef> = Vec::new();
                for s in o.sections() {
                    if s.name().ok() != Some(section.as_bytes()) {
                        continue;
                    }
                    let index = s.index().0;
                    for (n, r) in relocs(f, index).unwrap_or_default().iter().enumerate() {
                        if r.0 == offset && r.1 == name {
                            found.push(DiagRef {
                                file: f,
                                seq: (u64::from(index) << 32) + n as u64,
                                offset,
                            });
                        }
                    }
                }
                let [r] = found[..] else {
                    return Err(refuse(code, "a reference that is not one relocation"));
                };
                p.refs.push(r);
            }
            if p.refs.first().map(|r| (r.file.0, r.seq)) != Some((key.unit, key.seq)) {
                return Err(refuse(code, "its first listed reference is not its key's"));
            }
            p.name = Some((name, Vec::new()));
        } else {
            let places = d.place().into_iter().chain(d.context().iter().filter_map(|c| match c {
                Context::At(_, p) => Some(p),
                Context::Note(_) => None,
            }));
            for place in places {
                let (Place::File(file)
                | Place::Section { file, .. }
                | Place::Symbol { file, .. }
                | Place::Record { file, .. }) = place
                else {
                    continue;
                };
                p.subjects.push(Subject::File(file_of(code, file)?));
            }
            if column.contains(&SubjectKind::Key) {
                // An ODR record finding (odr.md §9.1; `mortar_odr::records`,
                // its seq tagged): its key is an ODR key (`Subject::OdrKey`,
                // PROVISIONAL until U-8 numbers them): not stored here.
                if code.starts_with("odr-") && key.seq >> 44 != 0 {
                    return Err(refuse(code, "an ODR record finding (its ODR key: U-8)"));
                }
                match d.kind().symbol() {
                    Some(n) => p.name = Some((n.as_bytes().to_vec(), Vec::new())),
                    // `gnu-warning` of a file (`.gnu.warning`, no key).
                    None if code == "gnu-warning" => {}
                    None => return Err(refuse(code, "its key is not in its kind")),
                }
            }
            if column.contains(&SubjectKind::Link) {
                p.subjects.push(Subject::Link);
            }
        }
        pending.push(p);
    }
    for h in &holders {
        if warned.contains(&h.0) {
            continue;
        }
        let k = inputs.symbols.get(h.0).ok_or("DIAGS: a warned key out of range")?;
        let text = text_of(h).ok_or("DIAGS: a .gnu.warning section that does not read")?;
        if k.version.is_some() {
            return Err("DIAGS: a versioned warned key (IN-75a)".into());
        }
        pending.push(PendingDiag {
            key: mortar_diag::Key { phase: 70, unit: h.1.0, seq: u64::MAX },
            d: Diagnostic::warning(Kind::GnuWarning { text }),
            name: Some((k.name.to_vec(), Vec::new())),
            subjects: Vec::new(),
            refs: Vec::new(),
            dormant: Some(h.1),
        });
    }
    let plain = Style::plain("");
    let mut out = Vec::with_capacity(pending.len());
    for p in pending {
        let code = p.d.id();
        let key = p.name.as_ref().map(|_| SymbolId(NONE));
        let row = if let (Some(holder), Some(k)) = (p.dormant, key) {
            crate::update::diags::dormant(&p.d, k, holder)
        } else {
            let mut subjects = p.subjects;
            subjects.extend(key.map(Subject::Key));
            crate::update::diags::row(p.key, &p.d, subjects, p.refs)
        }
        .ok_or_else(|| refuse(code, "it has no row (diags::row)"))?;
        // The check: the row reads back to the warning, by bytes.
        let back = crate::update::diags::diagnostic(&row)
            .ok_or_else(|| refuse(code, "its row does not read back"))?;
        if back != p.d || back.render(&plain) != p.d.render(&plain) {
            return Err(refuse(code, "its row reads back to another warning"));
        }
        out.push((row, p.name));
    }
    let mut seen: Vec<(u16, u32, u64, &str, &str)> = out
        .iter()
        .map(|(r, _)| (r.key.phase, r.key.unit, r.key.seq, &r.code[..], &r.text[..]))
        .collect();
    seen.sort_unstable();
    if seen.windows(2).any(|w| w[0] == w[1]) {
        return Err("DIAGS: two warnings of one key, code and text (IN-75a)".into());
    }
    Ok(out)
}

/// [`diag_rows_named`]' rows with each key subject its `SYMBOLS` row:
/// `symbols` gives each row's (kind, name, version); one pass for every
/// name wanted.
///
/// # Errors
/// A name with no global row, or with two (named).
fn resolve_diag_keys<'b>(
    named: Vec<NamedDiag>,
    symbols: impl Iterator<Item = (u8, &'b [u8], &'b [u8])>,
) -> Result<Vec<crate::update::diags::DiagRow>, String> {
    use crate::update::diags::Subject;
    let mut rows_of: HashMap<(Vec<u8>, Vec<u8>), Option<u32>> = HashMap::default();
    for (_, n) in &named {
        if let Some(n) = n {
            rows_of.insert(n.clone(), None);
        }
    }
    if !rows_of.is_empty() {
        for (i, (kind, name, version)) in symbols.enumerate() {
            if !matches!(kind, sym_kind::GLOBAL | sym_kind::VERSIONED) {
                continue;
            }
            if let Some(slot) = rows_of.get_mut(&(name.to_vec(), version.to_vec())) {
                if slot.is_some() {
                    return Err("DIAGS: two SYMBOLS rows of one key".into());
                }
                *slot = Some(i as u32);
            }
        }
    }
    let mut out = Vec::with_capacity(named.len());
    for (mut row, name) in named {
        if let Some(n) = name {
            let k = rows_of.get(&n).copied().flatten().ok_or_else(|| {
                format!(
                    "DIAGS: a warning {}: its key {:?} has no SYMBOLS row (IN-75a)",
                    row.code,
                    String::from_utf8_lossy(&n.0)
                )
            })?;
            for x in &mut row.subjects {
                if *x == Subject::Key(SymbolId(NONE)) {
                    *x = Subject::Key(SymbolId(k));
                }
            }
            row.subjects.sort_unstable();
            row.subjects.dedup();
        }
        out.push(row);
    }
    Ok(out)
}

// ---- replay (IN-61) ------------------------------------------------------------

/// The present link's id of each state row of a table.
pub(crate) struct Map {
    files: Vec<Option<FileId>>,
    sections: Vec<Option<SectionId>>,
    pub(crate) chunks: Vec<ChunkId>,
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

pub(crate) struct Clock<'l> {
    pub(crate) laps: &'l mut Laps,
    pub(crate) at: std::time::Instant,
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
pub(crate) fn map(
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
            ChunkKind::Merged(o) | ChunkKind::Relocs(o) => (Some(o), 0, false),
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
        // A link state is written by a loadable link only (`-r` is never
        // incremental, `spec/incr.md` IN-12).
        kind: mortar_core::PlacementKind::Loadable,
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

// ---- WP-225 G-2: the capture's per-file rows, restricted to a list of files --

/// One file's rows as a full link's capture makes them (WP-225 G-2, the
/// update's U4 producers): the same private makers [`capture`] and the
/// format-5 rows call, for the files asked only. Ids are the LINK's
/// (`SectionId`, `SymbolId`); the join translates them to the update's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRows {
    pub file: FileId,
    /// Its first section's id (`Resolution::section_base`): `sections[k]`
    /// is `SectionId(first_section + k)`.
    pub first_section: u32,
    /// Its `SECTIONS` rows, as [`capture`] writes them (placement from
    /// `layout`; `Str`s in the caller's `strings`).
    pub sections: Vec<SectionRow>,
    /// Its symbol table's first non-local entry (`sh_info`; 0 without a
    /// symbol table).
    pub first_global: u32,
    /// `MENTIONS` by position: the `k`-th is entry `first_global + k`
    /// (`tables::file_mentions`, `symbol` the link's `SymbolId`, `section`
    /// its `SectionId` or `NONE`). Empty for a file the link did not keep
    /// (a member not extracted), as in the capture.
    pub mentions: Vec<crate::format::MentionRow>,
    /// `(st_value, st_size)` of each mention, position for position.
    pub values: Vec<(u64, u64)>,
    /// Its non-local entries whose name holds an `@` (`META.versioned`'s
    /// count, as the format-5 capture counts it).
    pub versioned: u32,
    /// The format-5 capture's own per-file rows (`file5`, ruling G-2b:
    /// one producer), keys as the link's `SymbolId`s: `KEY_REFS`'
    /// `(key, def, bind, typ, flags)` one per key, `COMMONS`'
    /// `(key, size, align)`, `COPIES`' `(key, symbol index, size, type)`,
    /// and the COMDAT signatures it has a copy of (`GROUPS`), sorted.
    pub refs: Vec<(u32, u8, u8, u8, u8)>,
    pub commons: Vec<(u32, u64, u64)>,
    pub copies: Vec<(u32, u32, u64, u8)>,
    pub groups: Vec<Vec<u8>>,
    /// WP-225 M2b-1: its `INDEX` rows (`tables::index_file`, the
    /// capture's own maker), the LINK's ids (`unit` a `SectionId`, a
    /// key target a `SymbolId`). Empty from [`file_rows`]; filled by
    /// [`file_rows_index_edges`].
    pub index: Vec<crate::format::IndexRow>,
    /// WP-225 M2b-1: its sections' IN-29 rows under `--gc-sections`
    /// (`mortar_live::stored_edges`' rows whose source is one of its
    /// sections, in their order), the LINK's ids. Empty from
    /// [`file_rows`] and without `--gc-sections`; filled by
    /// [`file_rows_index_edges`].
    pub edges: Vec<mortar_core::incr::LiveEdge>,
}

/// WP-225 M2b-1: [`FileRows::index`] and [`FileRows::edges`] of `rows`
/// (made by [`file_rows`]): `INDEX` by `tables::index_file`, the
/// capture's own maker, over each live object's symbol table; `EDGES` the
/// slice of `edges` (`mortar_live::stored_edges`' rows, sorted by
/// source; `None` without `--gc-sections`) whose source is one of the
/// file's sections. O(those files' sections and relocations).
///
/// # Errors
/// As `index_file`, or a symbol table that cannot be read.
pub fn file_rows_index_edges(
    link: &Link<'_, '_>,
    arch: &dyn mortar_arch::Arch,
    edges: Option<&[mortar_core::incr::LiveEdge]>,
    rows: &mut [FileRows],
) -> Result<(), String> {
    use mortar_core::resolution::FileLive;
    let inputs = link.inputs;
    let res = link.resolution;
    for r in rows.iter_mut() {
        let f = inputs.files.get(r.file).ok_or("a file of the list is not the link's")?;
        let live = res.file_live.get(r.file).is_some_and(|l| *l != FileLive::Dead);
        if live && let Reader::Object(object) = f.reader {
            let bad = || format!("{}: symbol table", String::from_utf8_lossy(f.path));
            if let Some(table) = object.symbols().map_err(|_| bad())? {
                let mut index = Vec::new();
                crate::tables::index_file(inputs, link, r.file, &object, &table, arch, &mut index)?;
                r.index = index;
            }
        }
        if let Some(all) = edges {
            let range = res.section_base.get(r.file).copied().ok_or("a file without sections")?;
            let lo = all.partition_point(|e| e.source.0 < range.start.0);
            let hi = all.partition_point(|e| e.source.0 < range.end.0);
            r.edges = all[lo..hi].to_vec();
        }
    }
    Ok(())
}

/// [`FileRows`] for each of `files`, in that order: O(those files'
/// sections and symbols), no pass over the link's other files. `copies`
/// is `Link5::copies` (`COPIES` written: every `--odr` mode but `off`).
///
/// Not produced here: `INDEX` (`tables::index_file`, `pub(crate)` by
/// ruling G-2a, called by the U4 adapter next), `demand` and `facts` (the
/// scan's, whole-link: path (A)'s front in M2, ruling G-2c).
///
/// # Errors
/// A file or section without identity, or a symbol table that is not
/// the one the input phase read.
pub fn file_rows(
    link: &Link<'_, '_>,
    liveness: &mortar_core::Liveness,
    keys: &Keys<'_>,
    layout: &Layout,
    strings: &mut Strings,
    files: &[FileId],
    copies: bool,
) -> Result<Vec<FileRows>, String> {
    use mortar_core::resolution::FileLive;
    let inputs = link.inputs;
    let res = link.resolution;
    let p = &layout.placement;
    let mut out = Vec::with_capacity(files.len());
    for &file in files {
        let f = inputs.files.get(file).ok_or("a file of the list is not the link's")?;
        let range = res.section_base.get(file).copied().ok_or("a file without sections' base")?;
        let mut rows = FileRows {
            file,
            first_section: range.start.0,
            sections: Vec::new(),
            first_global: 0,
            mentions: Vec::new(),
            values: Vec::new(),
            versioned: 0,
            refs: Vec::new(),
            commons: Vec::new(),
            copies: Vec::new(),
            groups: Vec::new(),
            index: Vec::new(),
            edges: Vec::new(),
        };
        // KEY_REFS, COMMONS, COPIES, GROUPS: the capture's per-file task
        // (`copies`: `Link5::copies`, the link's `--odr` mode).
        let five = file5(inputs, res, copies, file)?;
        rows.refs = five.refs;
        rows.commons = five.commons;
        rows.copies = five.copies;
        rows.groups = five.groups.iter().map(|g| g.to_vec()).collect();
        // SECTIONS: `capture`'s row per section.
        for sid in range.start.0..range.end.0 {
            let id = SectionId(sid);
            let si = keys.identities.section(res, id).ok_or("a section without identity")?;
            let (owner, index) = res.sh_index(id).ok_or("a section without index")?;
            let (anchor_kind, anchor) = anchor_row(strings, si.key.anchor);
            let mut row = SectionRow {
                file: owner.0,
                anchor_kind,
                anchor,
                name: strings.add(si.key.name),
                ordinal: si.ordinal,
                place_chunk: NONE,
                ..SectionRow::default()
            };
            (row.sh_type, row.sh_flags, row.size, row.align, row.offset) =
                header_fields(inputs, owner, index);
            if let Some((chunk, offset)) = p.place.get(id).and_then(|pl| pl.get()) {
                row.place_chunk = chunk.0;
                row.place_offset = offset;
            }
            put_section_facts(&mut row, &section_facts_of(link, liveness, id)?)?;
            rows.sections.push(row);
        }
        // MENTIONS, their values, `first_global`, `versioned`: the format-5
        // capture's per-file reading.
        if let Reader::Object(object) = f.reader {
            let bad = || format!("{}: symbol table", String::from_utf8_lossy(f.path));
            if let Some(table) = object.symbols().map_err(|_| bad())? {
                let first = table.first_global().0;
                rows.first_global = first;
                let globals = table.len().saturating_sub(first) as usize;
                if inputs.sym_map_of(file).len() < globals {
                    return Err(format!(
                        "{}: the symbol table is not the one the input phase read",
                        String::from_utf8_lossy(f.path)
                    ));
                }
                for k in 0..globals {
                    let sym = table
                        .get(mortar_elf::symbol::SymbolIndex(first + k as u32))
                        .map_err(|_| bad())?;
                    if table.name(&sym).map_err(|_| bad())?.contains(&b'@') {
                        rows.versioned += 1;
                    }
                }
                let live = res.file_live.get(file).is_some_and(|l| *l != FileLive::Dead);
                if live {
                    rows.mentions = crate::tables::file_mentions(inputs, res, file, &table);
                    for k in 0..rows.mentions.len() {
                        let sym = table
                            .get(mortar_elf::symbol::SymbolIndex(first + k as u32))
                            .map_err(|_| bad())?;
                        rows.values.push((sym.st_value, sym.st_size));
                    }
                }
            }
        }
        out.push(rows);
    }
    Ok(out)
}

// ---- WP-267 P-1: the per-object producers (no `Inputs`, no `Resolution`) --

/// WP-267 P-1 (M-3): [`file_rows`]' `MENTIONS` of ONE live object without
/// the link (`tables::file_mentions`' body): the `k`-th non-local entry's
/// key is `keys[k]` (the caller's numbering, by name), a definition in
/// section index `i < sections` names `first_section + i`.
#[must_use]
pub fn file_mentions_of(
    file: FileId,
    table: &mortar_elf::symbol::SymbolTable<'_>,
    keys: &[u32],
    first_section: u32,
    sections: u32,
) -> Vec<crate::format::MentionRow> {
    crate::tables::file_mentions_of(file, table, keys, first_section, sections)
}

/// WP-267 P-1 (M-3): `file5`'s `COPIES` rows of ONE live object without the
/// link: per mention (`mentions`, [`file_mentions_of`]'s, position `k` is
/// entry `first_global + k` of `table`) a non-local definition in a
/// section `comdat` says is a COMDAT group's member, as `(key, symbol
/// index, size, type)`; none when `copies` is false (`--odr=off`).
/// `file5`'s other per-file rows (`KEY_REFS`, `COMMONS`, `GROUPS`) are no
/// `ChangedFile` field and are not made here.
///
/// # Errors
/// A symbol table entry that cannot be read.
pub fn file5_of(
    table: &mortar_elf::symbol::SymbolTable<'_>,
    mentions: &[crate::format::MentionRow],
    comdat: &dyn Fn(u32) -> bool,
    copies: bool,
) -> Result<Vec<(u32, u32, u64, u8)>, String> {
    use crate::format::def_kind;
    let mut out = Vec::new();
    if !copies {
        return Ok(out);
    }
    let first = table.first_global().0;
    for (k, m) in mentions.iter().enumerate() {
        if m.kind == def_kind::SECTION && m.bind != 0 && comdat(m.section) {
            let index = first + k as u32;
            let sym = table
                .get(mortar_elf::symbol::SymbolIndex(index))
                .map_err(|_| format!("symbol {index}: not readable"))?;
            out.push((m.symbol, index, sym.st_size, m.typ));
        }
    }
    Ok(out)
}

/// WP-267 P-1b (M-4): [`FileRows::index`] of ONE live object without the
/// link (`tables::index_file_of`, the body `index_file` shares): `section`
/// answers a section index's `(id, kind, kept, relocation section)` (the
/// update's post-resolve facts), `key(k)` the key of the `k`-th non-local
/// entry (the caller's numbering), `path` names the file in an error. The
/// rows are in `index_file`'s order (relocation order per unit).
///
/// # Errors
/// A section or relocation section that cannot be read.
#[allow(clippy::type_complexity, reason = "the facts' tuple, as `index_file` reads them")]
pub fn index_file_of(
    object: &mortar_elf::Object<'_>,
    table: &mortar_elf::symbol::SymbolTable<'_>,
    arch: &dyn mortar_arch::Arch,
    section: &dyn Fn(
        SectionIndex,
    )
        -> Option<(u32, mortar_core::resolution::SectionKind, bool, SectionIndex)>,
    key: &dyn Fn(usize) -> Option<u32>,
    path: &str,
) -> Result<Vec<crate::format::IndexRow>, String> {
    let mut out = Vec::new();
    crate::tables::index_file_of(object, table, arch, section, key, path, &mut out)?;
    Ok(out)
}

/// WP-267 P-1: `SECTIONS`' anchor columns of `anchor` (the capture's own
/// encoding, `anchor_row`), its name into `strings`.
pub fn anchor_row_of(strings: &mut Strings, anchor: Anchor<'_>) -> (u8, Str) {
    anchor_row(strings, anchor)
}

#[cfg(test)]
mod wp262 {
    //! `SYMTAB_KEYS`' write-time check ([`symtab_keys_check`]): each way
    //! a row can break it is refused.
    use super::{NONE, symtab_keys_check};
    use crate::format::Str;
    use crate::update::rows::{SymbolRow, SymtabKeyRow};
    use crate::update::testing::MemoryState;
    use mortar_core::incr::SymtabBlock;

    /// Keys `ab`, `c`, `d` (no entry); the area [10, 12) whose names
    /// start at byte 40: `ab` at 40, `c` at 43.
    fn good() -> (MemoryState, SymtabBlock) {
        let mut m = MemoryState { strings: b"abcd".to_vec(), ..MemoryState::default() };
        for (off, len) in [(0, 2), (2, 1), (3, 1)] {
            m.symbols.push(SymbolRow { name: Str { off, len }, ..SymbolRow::default() });
        }
        m.symtab_keys = vec![
            SymtabKeyRow { index: 10, strtab: 40 },
            SymtabKeyRow { index: 11, strtab: 43 },
            SymtabKeyRow { index: NONE, strtab: 0 },
        ];
        let area = SymtabBlock {
            first: 10,
            count: 2,
            capacity: 2,
            strtab_offset: 40,
            strtab_len: 5,
            strtab_capacity: 5,
        };
        (m, area)
    }

    /// A way to break [`good`]'s table.
    type Break = fn(&mut MemoryState, &mut SymtabBlock);

    #[test]
    fn a_good_table_passes_and_each_break_is_refused() {
        let (m, area) = good();
        assert_eq!(symtab_keys_check(&m, Some(area)), Ok(()));
        let cases: [(&str, Break); 8] = [
            ("outside", |m, _| m.symtab_keys[0].index = 12),
            ("one index", |m, _| m.symtab_keys[1] = SymtabKeyRow { index: 10, strtab: 40 }),
            ("named by no row", |m, _| m.symtab_keys[1] = SymtabKeyRow { index: NONE, strtab: 0 }),
            ("strtab", |m, _| m.symtab_keys[1].strtab = 42),
            ("ends past", |m, _| m.symtab_keys.swap(0, 1)),
            ("ends past", |_, a| a.strtab_len = 4),
            ("at least", |m, _| m.symtab_keys[1].strtab = 41),
            ("named by no row", |_, a| a.count = 3),
        ];
        for (what, break_it) in cases {
            let (mut m, mut a) = good();
            break_it(&mut m, &mut a);
            let e = symtab_keys_check(&m, Some(a)).expect_err(what);
            assert!(e.contains(what), "{what}: {e}");
        }
        // No `.symtab`: every row NONE.
        let (mut m, _) = good();
        assert!(symtab_keys_check(&m, None).is_err());
        m.symtab_keys.iter_mut().for_each(|k| *k = SymtabKeyRow { index: NONE, strtab: 0 });
        assert_eq!(symtab_keys_check(&m, None), Ok(()));
    }
}

#[cfg(test)]
mod wp263 {
    //! `SYMTAB` and `META.symtab`'s write-time check
    //! ([`symtab_rows_check`]): each way a row can break it is refused.
    use super::symtab_rows_check;
    use crate::format::{ChunkRow, Str};
    use crate::update::rows::{FileRow, file_flags};
    use crate::update::testing::MemoryState;
    use mortar_core::FileId;
    use mortar_core::incr::{SymtabBlock, SymtabTables};

    fn block(first: u32, count: u32, capacity: u32, str: (u64, u64, u64)) -> SymtabBlock {
        SymtabBlock {
            first,
            count,
            capacity,
            strtab_offset: str.0,
            strtab_len: str.1,
            strtab_capacity: str.2,
        }
    }

    struct Case {
        m: MemoryState,
        blocks: Vec<(FileId, SymtabBlock)>,
        area: SymtabBlock,
        tables: SymtabTables,
    }

    /// Three files: the internal block [1, 5), file 1's [5, 10), file 2's
    /// [10, 13); the tail [13, 77); the global area [77, 83) of 24-byte
    /// entries; `.strtab`'s regions from 31, its names [105, 145).
    fn good() -> Case {
        let mut m = MemoryState { strings: b".symtab.strtab".to_vec(), ..MemoryState::default() };
        m.meta.class = mortar_elf::consts::ELFCLASS64;
        m.files = vec![FileRow::default(); 3];
        let chunk = |off, size| ChunkRow { name: Str { off, len: 7 }, size, ..ChunkRow::default() };
        m.chunks = vec![chunk(0, 83 * 24), chunk(7, 145)];
        let blocks = vec![
            (FileId(0), block(1, 2, 4, (31, 10, 26))),
            (FileId(1), block(5, 3, 5, (57, 12, 28))),
            (FileId(2), block(10, 1, 3, (85, 4, 20))),
        ];
        let area = block(77, 6, 6, (105, 40, 40));
        let tables = SymtabTables {
            sh_info: 77,
            tail_first: 13,
            tail_capacity: 64,
            symtab_size: 83 * 24,
            symtab_slack: 4096,
            strtab_size: 145,
            strtab_slack: 4096,
        };
        m.meta.symtab = tables;
        m.symtab = vec![(FileId(0), area), blocks[0], blocks[1], blocks[2]];
        Case { m, blocks, area, tables }
    }

    /// A way to break [`good`]'s rows.
    type Break = fn(&mut Case);

    fn check(c: &Case) -> Result<(), String> {
        symtab_rows_check(&c.m, &c.blocks, Some(c.area), Some(c.tables))
    }

    #[test]
    fn good_rows_pass_and_each_break_is_refused() {
        assert_eq!(check(&good()), Ok(()));
        let cases: [(&str, Break); 19] = [
            ("not in FILES order", |c| (c.blocks[1].0, c.blocks[2].0) = (FileId(2), FileId(1))),
            ("no FILES row", |c| c.blocks[2].0 = FileId(3)),
            ("is LAZY", |c| c.m.files[2].flags |= file_flags::LAZY),
            ("starts at entry 11", |c| c.blocks[2].1.first = 11),
            ("holds 6 entries of capacity 5", |c| c.blocks[1].1.count = 6),
            ("holds 0 entries", |c| c.blocks[2].1.count = 0),
            ("uses 29 of 28", |c| c.blocks[1].1.strtab_len = 29),
            ("region starts at 86", |c| c.blocks[2].1.strtab_offset = 86),
            ("the tail reserve starts at 14", |c| c.tables.tail_first = 14),
            ("does not end at sh_info", |c| c.tables.tail_capacity = 63),
            ("the global area starts at 78", |c| c.area.first = 78),
            ("holds 6 of 7", |c| c.area.capacity = 7),
            ("the global names start at 106", |c| c.area.strtab_offset = 106),
            ("do not end .strtab", |c| c.tables.strtab_size = 146),
            ("entries of 24 bytes", |c| c.tables.symtab_size = 83 * 24 + 1),
            (".symtab's chunk is 1990 bytes", |c| c.m.chunks[0].size = 1990),
            ("not file 0's global area and internal block", |c| {
                c.m.symtab.pop();
            }),
            // WP-264: the internal block is file 0's second row.
            ("not file 0's global area and internal block", |c| {
                c.m.symtab.remove(1);
            }),
            ("not file 0's global area and internal block", |c| c.m.symtab.swap(0, 1)),
        ];
        for (what, break_it) in cases {
            let mut c = good();
            break_it(&mut c);
            let e = check(&c).expect_err(what);
            assert!(e.contains(what), "{what}: {e}");
        }
        let mut c = good();
        c.m.chunks.remove(1);
        assert!(check(&c).expect_err("no .strtab chunk").contains("no .strtab chunk"));
        // No `.symtab`: no block, no row, META zero, no chunk.
        let mut c = good();
        assert!(symtab_rows_check(&c.m, &c.blocks, None, None).is_err());
        c.m.symtab.clear();
        c.m.meta.symtab = SymtabTables::default();
        assert!(symtab_rows_check(&c.m, &[], None, None).expect_err("chunk").contains("chunk"));
        c.m.chunks.clear();
        assert_eq!(symtab_rows_check(&c.m, &[], None, None), Ok(()));
    }
}

#[cfg(test)]
mod wp264 {
    //! `REGIONS`' producer ([`region_rows`]) and its write-time check
    //! ([`regions_check`]) on a hand-made placement: two files' runs in
    //! one `.text` body chunk, each followed by its slack.
    use super::{region_rows, regions_check, tag_byte};
    use crate::format::{ChunkRow, OutSecRow, SectionRow, Str, hole_kind};
    use crate::update::rows::{HoleRow, RegionRow};
    use crate::update::testing::MemoryState;
    use mortar_core::FileId;
    use mortar_core::identity::ChunkTag;
    use mortar_elf::consts::{SHF_ALLOC, SHF_EXECINSTR};

    fn sec(m: &mut MemoryState, file: u32, name: &str, offset: u64, size: u64) {
        let off = m.strings.len() as u32;
        m.strings.extend_from_slice(name.as_bytes());
        m.sections.push(SectionRow {
            file,
            out: 1,
            name: Str { off, len: name.len() as u32 },
            sh_flags: SHF_ALLOC | SHF_EXECINSTR,
            size,
            align: 8,
            place_chunk: 0,
            place_offset: offset,
            ..SectionRow::default()
        });
    }

    /// File 1: `.text` [0, 0x10) and `.text.b` [0x10, 0x18), slack to
    /// 0x58; file 2: `.text` [0x60, 0x80), slack to 0xc0; a `.text.hot`
    /// of file 2 [0xc0, 0xc8), category 1, the chunk's last region with
    /// no slack (cut at a part limit).
    fn placed() -> MemoryState {
        let mut m = MemoryState::default();
        m.strings.extend_from_slice(b".text");
        m.outsecs.push(OutSecRow { name: Str { off: 0, len: 5 }, ordinal: 0 });
        m.chunks.push(ChunkRow {
            tag: tag_byte(ChunkTag::Body),
            sh_flags: SHF_ALLOC | SHF_EXECINSTR,
            size: 0xc8,
            capacity: 0x1000,
            addr: 0x40_0000,
            ..ChunkRow::default()
        });
        sec(&mut m, 1, ".text", 0, 0x10);
        sec(&mut m, 1, ".text.b", 0x10, 0x8);
        sec(&mut m, 2, ".text", 0x60, 0x20);
        sec(&mut m, 2, ".text.hot", 0xc0, 0x8);
        for (offset, size) in [(0x18, 0x40), (0x80, 0x40)] {
            m.holes.push(HoleRow { chunk: 0, kind: hole_kind::FILL, offset, size, tomb: false });
        }
        m
    }

    fn region(category: u8, ordinal: u32, offset: u64, capacity: u64) -> RegionRow {
        RegionRow { out: 0, category, ordinal, chunk: 0, offset, capacity }
    }

    #[test]
    fn the_runs_are_the_regions_and_each_break_is_refused() {
        let mut m = placed();
        let text = mortar_layout::order::text_category(b".text");
        let hot = mortar_layout::order::text_category(b".text.hot");
        assert_ne!(hot, text, "a category of its own");
        assert!(hot < text, "the order below");
        let rows = region_rows(&m).expect("the rows");
        assert_eq!(
            rows,
            vec![
                (FileId(1), region(text, 0, 0, 0x58)),
                (FileId(2), region(hot, 0, 0xc0, 0x8)),
                (FileId(2), region(text, 0, 0x60, 0x60)),
            ]
        );
        m.regions = rows.clone();
        assert_eq!(regions_check(&m), Ok(()));
        let broken = |f: &dyn Fn(&mut MemoryState), words: &str| {
            let mut b = m.clone();
            f(&mut b);
            let e = regions_check(&b).expect_err(words);
            assert!(e.contains(words), "{words}: {e}");
        };
        broken(&|b| b.regions[0].1.capacity = 0x61, "overlap");
        broken(&|b| b.regions[1].1.capacity = 0x9, "outside chunk");
        broken(&|b| b.regions[2].0 = FileId(3), "in no region of its file");
        broken(&|b| b.regions[0].1.ordinal = 1, "ordinals");
        broken(&|b| b.regions[0].1.offset = 0x8, "in no region of its file");
        broken(&|b| b.regions[2].1.capacity = 0x50, "no Fill hole is its slack");
        broken(
            &|b| {
                b.regions.remove(0);
            },
            "in no region",
        );
        broken(&|b| b.regions.push((FileId(3), region(text, 0, 0x58, 0x8))), "holds no section");
        broken(
            &|b| {
                b.regions[0].1.offset = 0x0;
                b.sections[0].place_offset = 0x4;
                b.sections[0].size = 0xc;
            },
            "starts before its first section",
        );
        broken(
            &|b| {
                b.holes.push(HoleRow {
                    chunk: 0,
                    kind: hole_kind::FILL,
                    offset: 0x58,
                    size: 4,
                    tomb: false,
                });
            },
            "is no region's slack",
        );
        // WP-255g S2-1: the padding from one region's end to the next
        // region's start as a `Fill` hole is a pad, not a break.
        let mut pad = m.clone();
        pad.holes.push(HoleRow {
            chunk: 0,
            kind: hole_kind::FILL,
            offset: 0x58,
            size: 8,
            tomb: false,
        });
        assert_eq!(regions_check(&pad), Ok(()), "a pad between two regions");
        broken(&|b| b.regions[0].1.chunk = 1, "no chunk 1");
        broken(&|b| b.chunks[0].tag = tag_byte(ChunkTag::Merged), "not a hole-tolerant body");
        // A member of another output section (L-159's empty neighbour) is
        // no member of the chunk's regions.
        let mut n = m.clone();
        sec(&mut n, 3, ".tm_clone_table", 0x58, 0);
        n.sections.last_mut().expect("added").out = 2;
        assert_eq!(region_rows(&n).expect("the rows"), rows);
        n.regions = rows;
        assert_eq!(regions_check(&n), Ok(()));
    }
}

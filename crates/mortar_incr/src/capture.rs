//! A link's tables in identity terms (`spec/model.md` §7.1) and back:
//! [`capture`] writes the rows of a state from one link's tables;
//! [`replay`] translates a state's decision tables to another link's ids
//! (IN-61; the update of WP-064 will translate the same way).
//!
//! The parts of an identity read from input bytes (a section's anchor, a
//! local symbol's name) come from each object's symbol table, read here
//! once per file ([`Keys::new`]).

use std::collections::HashMap;

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
    AllocKey, Chunk, ChunkKind, DefValue, Defined, EhRecord, Erratum, Layout, MergeMap, Patch,
    PatchKind, Place, Placement, Redirect, Segment, Sized, SlotKind, Slots, SymPlace, ThunkTarget,
    Tls,
};
use mortar_core::resolution::Resolution;
use mortar_core::split::Split;
use mortar_elf::consts::{STT_FILE, STT_SECTION};
use mortar_elf::{SectionIndex, SymbolSection};

use crate::format::{
    ChunkRow, DefinedRow, EhRow, FileRow, FragRow, NONE, OutSecRow, PatchRow, ProbeRow,
    RedirectRow, SectionRow, SegmentRow, SizeRow, SlotRow, SpanRow, State, Str, Strings,
    SymPlaceRow, SymbolRow, chunk_bits, layout_bits, patch_kind, sym_kind,
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

fn tag_byte(tag: ChunkTag) -> u8 {
    TAGS.iter().position(|&t| t == tag).unwrap_or(0) as u8
}

impl<'s> Keys<'s> {
    /// Reads every object's anchors and local names (§7.1).
    pub fn new(link: &Link<'_, 's>) -> Result<Self, String> {
        let inputs = link.inputs;
        let mut sections = Vec::with_capacity(inputs.files.len() as usize);
        let mut locals = Vec::with_capacity(inputs.files.len() as usize);
        for (file, f) in inputs.files.iter() {
            let Reader::Object(object) = f.reader else {
                sections.push(Ordinals::new(Vec::new()));
                locals.push(Ordinals::new(Vec::new()));
                continue;
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
                    if name.is_empty()
                        || kind == STT_SECTION
                        || kind == STT_FILE
                        || sym.st_value != 0
                    {
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
            sections.push(Ordinals::new(keys));
            locals.push(Ordinals::new(names));
        }
        let sections = Column::from_vec(sections).map_err(|_| "too many files".to_owned())?;
        let locals = Column::from_vec(locals).map_err(|_| "too many files".to_owned())?;
        Ok(Self {
            identities: Identities::new(inputs.file_identities(), sections, locals),
            outsecs: outsec_ordinals(link.assignment),
            symbols: symbol_lookup(&inputs.symbols),
        })
    }
}

/// The content hash of a fragment's bytes (`L128`, IN-18a `FRAGS`).
fn frag_hash(link: &Link<'_, '_>, frag: FragId) -> Option<[u8; 16]> {
    let f = link.split.frags.get(frag)?;
    let (file, index) = link.resolution.sh_index(f.section)?;
    let Reader::Object(object) = link.inputs.files.get(file)?.reader else { return None };
    let range = object.section(index).ok()?.file_range().ok()??;
    let start = range.start as u64 + u64::from(f.offset);
    let range = start..start + u64::from(f.size);
    if let Some(bytes) = link.inputs.resident(file, range.clone()) {
        return Some(mortar_digest::l128(bytes));
    }
    let mut buf = vec![0u8; f.size as usize];
    link.inputs.copy_to(file, range, &mut buf).ok()?;
    Some(mortar_digest::l128(&buf))
}

fn anchor_row(strings: &mut Strings, anchor: Anchor<'_>) -> (u8, Str) {
    match anchor {
        Anchor::None => (0, Str::default()),
        Anchor::Comdat(n) => (1, strings.add(n)),
        Anchor::Global(n) => (2, strings.add(n)),
        Anchor::Local(n) => (3, strings.add(n)),
    }
}

struct Symbols<'k, 's> {
    keys: &'k Keys<'s>,
    inputs: &'k Inputs<'s>,
    rows: Vec<SymbolRow>,
    index: HashMap<SymKey, u32>,
}

impl Symbols<'_, '_> {
    fn row(&mut self, strings: &mut Strings, key: SymKey) -> Result<u32, String> {
        if let Some(&row) = self.index.get(&key) {
            return Ok(row);
        }
        let row = match key.kind() {
            SymKind::Global(id) => {
                let k = self.inputs.symbols.get(id).ok_or("a global out of range")?;
                SymbolRow {
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
        self.index.insert(key, at);
        Ok(at)
    }
}

/// The rows of a state that a link's tables give: `FILES`, `PROBES`,
/// `OUTSECS`, `SYMBOLS`, `SECTIONS` and the decision tables. `file_hash`
/// is each file's content hash (`None`: none could be computed). The
/// caller fills `META`, `STAT`, `LEAVES` and the pool. An `Err` names why
/// no state can be written (dynamic tables, `--icf`'s folds).
#[allow(clippy::too_many_lines)] // one block per table, in IN-18a's order
pub fn capture(
    link: &Link<'_, '_>,
    keys: &Keys<'_>,
    layout: &Layout,
    strings: &mut Strings,
    file_hash: &dyn Fn(FileId) -> Option<[u8; 16]>,
) -> Result<State, String> {
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
    for probe in inputs.probe_checks().ok_or("a probe failed")? {
        s.probes.push(ProbeRow { path: strings.add(probe.path), present: u8::from(probe.present) });
    }
    for id in link.assignment.outsecs.ids().iter() {
        let o =
            mortar_core::identity::outsec_identity(&keys.outsecs, id).ok_or("an output section")?;
        s.outsecs.push(OutSecRow { name: strings.add(o.name), ordinal: o.ordinal });
    }
    let chunk_ord = chunk_ordinals(p, link.assignment);
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
        if let Reader::Object(object) = inputs.files[file].reader
            && let Ok(sec) = object.section(index)
        {
            row.sh_type = sec.sh_type();
            row.sh_flags = sec.flags();
            row.size = sec.size();
            row.align = sec.alignment().unwrap_or(0);
            row.offset = sec.file_range().ok().flatten().map_or(0, |r| r.start as u64);
        }
        if let Some((chunk, offset)) = p.place.get(id).and_then(|pl| pl.get()) {
            row.place_chunk = chunk.0;
            row.place_offset = offset;
        }
        s.sections.push(row);
    }
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
    // FRAGS: per (output section, hash), the live fragments' offsets.
    let mut live: Vec<(u32, [u8; 16], u32, u64)> = Vec::new();
    for (frag, &off) in layout.merge.frag_off.iter() {
        if off == MergeMap::NONE {
            continue;
        }
        let f = link.split.frags.get(frag).ok_or("a fragment")?;
        let out = link.assignment.out(f.section).ok_or("a live fragment without output section")?;
        let hash = frag_hash(link, frag).ok_or("a fragment's bytes")?;
        live.push((out.0, hash, frag.0, off));
    }
    live.sort_unstable();
    let mut i = 0;
    while i < live.len() {
        let mut j = i + 1;
        while j < live.len() && (live[j].0, live[j].1) == (live[i].0, live[i].1) {
            j += 1;
        }
        let group = &live[i..j];
        if group.iter().all(|g| g.3 == group[0].3) {
            s.frags.push(FragRow {
                out: group[0].0,
                ordinal: NONE,
                hash: group[0].1,
                offset: group[0].3,
            });
        } else {
            for (n, g) in group.iter().enumerate() {
                s.frags.push(FragRow { out: g.0, ordinal: n as u32, hash: g.1, offset: g.3 });
            }
        }
        i = j;
    }
    let mut syms = Symbols { keys, inputs, rows: Vec::new(), index: HashMap::new() };
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

fn map(
    link: &Link<'_, '_>,
    keys: &Keys<'_>,
    s: &State,
    placement: &Placement,
) -> Result<Map, String> {
    let files: Vec<Option<FileId>> =
        s.files.iter().map(|f| keys.identities.file_id(&file_identity(s, f))).collect();
    let sections = s
        .sections
        .iter()
        .map(|x| {
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
    let symbols = s
        .symbols
        .iter()
        .map(|y| match y.kind {
            sym_kind::LOCAL => {
                let file = s.files.get(y.file as usize)?;
                let l = LocalIdentity {
                    file: file_identity(s, file),
                    name: s.str(y.name),
                    ordinal: y.ordinal,
                };
                keys.identities.local_sym(&l).map(SymKey::local)
            }
            k => {
                let version = (k == sym_kind::VERSIONED).then(|| s.str(y.version));
                let g = SymbolIdentity { name: s.str(y.name), version };
                symbol_id(&link.inputs.symbols, &keys.symbols, &g).map(SymKey::global)
            }
        })
        .collect();
    Ok(Map { files, sections, chunks, symbols, outsecs })
}

fn get<T: Copy>(v: &[Option<T>], i: u32, what: &str) -> Result<T, String> {
    v.get(i as usize).copied().flatten().ok_or_else(|| format!("{what} row {i} does not translate"))
}

/// The decision tables of a state, translated to this link's ids (IN-61):
/// what the link writes in place of its own `computed` layout. Every row
/// must translate and every chunk of this link must be named once; a
/// failure is named. `Folds` and the dynamic tables are `computed`'s
/// (format 1 stores none).
#[allow(clippy::too_many_lines, clippy::many_single_char_names)] // one block per table
pub fn replay(
    link: &Link<'_, '_>,
    keys: &Keys<'_>,
    s: &State,
    computed: &Layout,
) -> Result<Layout, String> {
    let cp = &computed.placement;
    let m = map(link, keys, s, cp)?;
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
    // The erratum fixes (the branch at each site and its patch's words)
    // are words the front computes from its own placement and the image
    // writes as they are (`Front::erratum_fixes`): a stored erratum patch
    // or redirect that is not the computed one would be a layout the
    // image does not follow, so it is refused (a finding of the WP-061
    // review, F8).
    let errata = |patches: &mut dyn Iterator<Item = &Patch>, redirects: &[Redirect]| {
        let all: Vec<&Patch> = patches.collect();
        let mut v: Vec<(SectionId, u64, ChunkId, u64)> = redirects
            .iter()
            .filter_map(|r| {
                let p = all.get(r.patch as usize)?;
                matches!(p.kind, PatchKind::Erratum { .. })
                    .then_some((r.section, r.offset, p.chunk, p.offset))
            })
            .collect();
        v.sort_unstable();
        let mut sites: Vec<(SectionId, u64, ChunkId, u64)> = all
            .iter()
            .filter_map(|p| match p.kind {
                PatchKind::Erratum { section, offset, .. } => {
                    Some((section, offset, p.chunk, p.offset))
                }
                PatchKind::Thunk(_) => None,
            })
            .collect();
        sites.sort_unstable();
        (v, sites)
    };
    if errata(&mut patches.iter().map(|(p, _)| p), &redirects)
        != errata(&mut cp.patches.iter(), &cp.redirects)
    {
        return Err("an erratum patch or its redirect is not the one the image writes".to_owned());
    }
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
    };
    // The merged map: this link's live fragments take their identity's
    // stored offset; its dead ones stay dead (IN-18a `FRAGS`).
    let mut by_identity: HashMap<(OutSecId, [u8; 16]), Vec<&FragRow>> = HashMap::new();
    for f in &s.frags {
        let out = get(&m.outsecs, f.out, "OUTSECS")?;
        by_identity.entry((out, f.hash)).or_default().push(f);
    }
    let mut frag_off = computed.merge.frag_off.clone();
    let mut rank: HashMap<(OutSecId, [u8; 16]), u32> = HashMap::new();
    for (i, off) in frag_off.as_mut_slice().iter_mut().enumerate() {
        let frag = FragId::from_index(i as u32);
        if *off == MergeMap::NONE {
            continue;
        }
        let f = link.split.frags.get(frag).ok_or("a fragment")?;
        let out = link.assignment.out(f.section).ok_or("a fragment's output section")?;
        let hash = frag_hash(link, frag).ok_or("a fragment's bytes")?;
        let rows = by_identity.get(&(out, hash)).ok_or("a live fragment has no FRAGS row")?;
        let n = rank.entry((out, hash)).or_insert(0);
        let row = if rows.len() == 1 && rows[0].ordinal == NONE {
            rows[0]
        } else {
            rows.iter()
                .find(|r| r.ordinal == *n)
                .copied()
                .ok_or("a live fragment's ordinal has no row")?
        };
        *n += 1;
        *off = row.offset;
    }
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
    // The image writes `.eh_frame` from the front's plan, not from this
    // list (a finding of WP-061): a stored list that is not the plan's
    // would be a layout the image does not follow, so it is refused.
    if slots.eh_records != computed.slots.eh_records {
        return Err("EH_RECORDS is not the list of records the image writes".to_owned());
    }
    // Likewise `.symtab`'s local spans come from the front's symbol table
    // plan; the stored spans must be its spans (no table: every span is
    // empty).
    if s.symtab_locals.is_empty() {
        if computed.slots.symtab_locals.iter().any(|(_, sp)| sp.len != 0) {
            return Err("SYMTAB_LOCALS is empty but the symbol table has locals".to_owned());
        }
        slots.symtab_locals = computed.slots.symtab_locals.clone();
    } else {
        let mut spans = vec![mortar_core::id::Span::default(); link.inputs.files.len() as usize];
        for (i, sp) in s.symtab_locals.iter().enumerate() {
            let file = get(&m.files, i as u32, "FILES")?;
            spans[file.slot()] = mortar_core::id::Span { start: sp.start, len: sp.len };
        }
        slots.symtab_locals = Column::from_vec(spans).map_err(|_| "files")?;
        if slots.symtab_locals != computed.slots.symtab_locals {
            return Err("SYMTAB_LOCALS is not the symbol table's spans".to_owned());
        }
    }
    slots.sizes =
        s.sizes.iter().map(|z| Ok((chunk(z.chunk)?, z.size))).collect::<Result<_, String>>()?;
    Ok(Layout { folds: computed.folds.clone(), merge: MergeMap { frag_off }, slots, placement })
}

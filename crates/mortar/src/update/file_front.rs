//! WP-255d: the per-file front (`spec/landed/incr/WP-255.md` §4, shape C';
//! the phase table in `spec/landed/incr/WP-255d.md`).
//!
//! [`front_of`] opens ONE file of the state (an archive member at its
//! stored `base`), checks the hash of the bytes it read against
//! `FILES.hash` before anything is taken from them (IN-24; §7's
//! `FILES.base` row: a difference is `input-changed`), parses it with the
//! input phase's own reader (`mortar_input::facts::analyse`), and gives:
//!
//! - its section rows with the STORED ids (the file's `SECTIONS` range of
//!   `FILE_INDEX`), the header fields the capture reads
//!   (`capture::header_fields`' five), the §7.1 key (anchor, name,
//!   ordinal), the stored place, and the relocation table of each placed
//!   section ([`FileFront::section_front`]: what `mortar_image::rows::row_for`
//!   reads);
//! - its non-local symbol-table entries mapped to stored keys by
//!   `StateRows::find_key` (name and version, `SymName::parse`'s split);
//!   locals have no key.
//!
//! [`FrontCache`] keys the fronts by `FileId`: each file is opened once, and
//! [`FrontCache::opened`] lists what was opened (IN-75's recheck covers
//! exactly those).
//!
//! Ids: an UNCHANGED file (a referrer, `Ids::Stored`) takes every id from
//! the state. A CHANGED file (`Ids::Update`) takes the `first_section` the
//! join gives it (`ChangedFile::first_section`); its keys are `find_key`'s
//! answer or `None` (a new key: U-3 allocates it, `resolve.rs` `new_keys`).
//! This module allocates nothing.
//!
//! Not here (WP-255d's next milestones): the split of merge sections and
//! `.eh_frame`, and the scan with stored facts (the seams of the table).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mortar_core::id::{ChunkId, FileId, SectionId, SymbolId};
use mortar_core::identity::Anchor;
use mortar_core::incr::{IncrReason, Unimplemented};
use mortar_core::intern::SymName;
use mortar_core::par::Pool;
use mortar_elf::consts::{
    GRP_COMDAT, SHF_LINK_ORDER, SHT_GROUP, SHT_REL, SHT_RELA,
};
use mortar_elf::{Object, SectionIndex};
use mortar_image::plan::RelaTable;
use mortar_image::rows::SectionFront;
use mortar_incr::capture;
use mortar_incr::format::NONE;
use mortar_incr::update::rows::{FileRow, FileTable, file_flags};
use mortar_incr::update::view::StateView;
use mortar_input::facts::{Kind, analyse, effective_signature};
use mortar_input::store::{ReadJob, Store, content_hash};

/// Which id space the file's rows take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ids {
    /// An unchanged file: every id is the state's.
    Stored,
    /// A changed file: its sections are `first_section + k` (the join's
    /// allocation, `ChangedFile::first_section`).
    Update { first_section: SectionId },
}

/// One section of a [`FileFront`] (section index `k` of its file).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrontSection<'s> {
    /// Its id: the state's `SECTIONS` row (`Ids::Stored`) or the update's.
    pub id: SectionId,
    pub index: SectionIndex,
    pub sh_type: u32,
    pub flags: u64,
    pub size: u64,
    pub align: u64,
    /// Its offset in its file (0 without file bytes), as
    /// `capture::header_fields` gives it.
    pub offset: u64,
    pub name: &'s [u8],
    /// §7.1's anchor and the ordinal among the file's sections of one key.
    pub anchor: Anchor<'s>,
    pub ordinal: u32,
    /// Its place `(chunk, offset)` (`SECTIONS.place_*`); `None` when not
    /// placed, and always for `Ids::Update` (U-4 places them).
    pub place: Option<(ChunkId, u64)>,
    /// The relocation section that applies to it, for a placed section.
    pub rela: Option<RelaTable>,
}

/// A non-local symbol-table entry `first_global + k` and its key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrontKey<'s> {
    pub name: &'s [u8],
    /// `None` when unversioned (`foo` and `foo@@V`).
    pub version: Option<&'s [u8]>,
    /// `find_key`'s answer: the state's key, or `None` (a new key).
    pub key: Option<SymbolId>,
    pub st_value: u64,
    pub st_size: u64,
}

/// What [`front_of`] gives of one file.
#[derive(Debug, Clone)]
pub struct FileFront<'s> {
    pub file: FileId,
    /// Its stored `FILES` row (path, member, `base`, `len`, `hash`).
    pub row: FileRow,
    pub object: Object<'s>,
    /// Its bytes (`base .. base + len` of the file it was read from).
    pub bytes: &'s [u8],
    /// One per section index, index 0 included (as `SECTIONS` holds them);
    /// none for a file the state holds no `SECTIONS` rows of (`Ids::Stored`).
    pub sections: Vec<FrontSection<'s>>,
    /// The symbol table's first non-local entry (`sh_info`; 0 without one).
    pub first_global: u32,
    pub keys: Vec<FrontKey<'s>>,
    /// Non-local entries whose name holds an `@` (`META.versioned`'s count).
    pub versioned: u32,
}

impl FileFront<'_> {
    /// What `mortar_image::rows::row_for` reads of section index `k`, with
    /// the file's id `file` (the image's).
    #[must_use]
    pub fn section_front(&self, k: usize) -> Option<SectionFront<'_>> {
        let s = self.sections.get(k)?;
        Some(SectionFront {
            file: self.file,
            index: s.index,
            size: s.size,
            flags: s.flags,
            src: s.offset,
            name: s.name,
            rela: s.rela,
        })
    }
}

/// The fronts an update opened, by `FileId`: each file once.
#[derive(Debug)]
pub struct FrontCache<'s> {
    store: &'s Store,
    pool: Pool,
    cwd: PathBuf,
    fronts: BTreeMap<FileId, FileFront<'s>>,
    opened: Vec<FileId>,
}

impl<'s> FrontCache<'s> {
    /// An empty cache reading through `store` (the update's: IN-75 rechecks
    /// what it holds), paths relative to `cwd`.
    #[must_use]
    pub fn new(store: &'s Store, pool: Pool, cwd: &Path) -> Self {
        Self { store, pool, cwd: cwd.to_path_buf(), fronts: BTreeMap::new(), opened: Vec::new() }
    }

    /// The files opened, in the order they were.
    #[must_use]
    pub fn opened(&self) -> &[FileId] {
        &self.opened
    }

    /// The front of `file`, if it was opened.
    #[must_use]
    pub fn get(&self, file: FileId) -> Option<&FileFront<'s>> {
        self.fronts.get(&file)
    }
}

fn corrupt(what: impl Into<String>) -> IncrReason {
    IncrReason::StateCorrupt(what.into())
}

fn changed(path: &[u8], what: &str) -> IncrReason {
    IncrReason::InputChanged(format!("{}: {what}", String::from_utf8_lossy(path)))
}

/// The front of `file` (opened once; later calls take the cache's).
///
/// # Errors
/// `input-changed` when the bytes read are not the ones `FILES.hash`
/// names (or no longer an object), `state-corrupt` for rows that do not
/// fit the file; `update-not-implemented (resolve)` for a changed file
/// (`Ids::Update`) with an `SHF_LINK_ORDER` section outside the groups
/// linked to a COMDAT member (resolve.md §6.5: the keeper decides).
pub fn front_of<'c, 's>(
    cache: &'c mut FrontCache<'s>,
    view: &StateView<'_>,
    file: FileId,
    ids: Ids,
) -> Result<&'c FileFront<'s>, IncrReason> {
    if !cache.fronts.contains_key(&file) {
        let front = open(cache, view, file, ids)?;
        cache.opened.push(file);
        cache.fronts.insert(file, front);
    }
    cache.fronts.get(&file).ok_or_else(|| corrupt("the cache lost a front"))
}

/// The file's bytes as the store holds them, `base .. base + len`.
fn read<'s>(cache: &FrontCache<'s>, path: &[u8], row: &FileRow) -> Result<&'s [u8], IncrReason> {
    let store = cache.store;
    let p = cache.cwd.join(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(path));
    let stat = store.stat(&p);
    let job = ReadJob { path: p, stat };
    let blob = store
        .read_many(&cache.pool, std::slice::from_ref(&job))
        .pop()
        .ok_or_else(|| changed(path, "not read"))?
        .map_err(|e| changed(path, &format!("not read: {e:?}")))?;
    let end = row.base.checked_add(row.len).ok_or_else(|| corrupt("a FILES range overflows"))?;
    if end > store.len(blob) {
        return Err(changed(path, "shorter than its FILES range"));
    }
    if let Some(whole) = store.whole(blob) {
        let (b, e) = (usize::try_from(row.base), usize::try_from(end));
        let (Ok(b), Ok(e)) = (b, e) else { return Err(corrupt("a FILES range")) };
        return Ok(&whole[b..e]);
    }
    let len = usize::try_from(row.len).map_err(|_| corrupt("a FILES length"))?;
    let mut buf = vec![0u8; len];
    store
        .copy_to(blob, row.base..end, &mut buf)
        .map_err(|e| changed(path, &format!("not read: {e:?}")))?;
    Ok(store.keep(&buf))
}

#[allow(clippy::too_many_lines, reason = "the rows of one file in order")]
fn open<'s>(
    cache: &FrontCache<'s>,
    view: &StateView<'_>,
    file: FileId,
    ids: Ids,
) -> Result<FileFront<'s>, IncrReason> {
    let row = view.file(file)?;
    let path = view.string(row.path)?;
    if row.flags & file_flags::GONE != 0 {
        return Err(corrupt(format!("file {} is gone", file.0)));
    }
    let bytes = read(cache, &path, &row)?;
    // IN-24 / §7: the hash of exactly the bytes used, before any is used.
    if crate::incremental::hash_bytes(content_hash(bytes)) != row.hash {
        return Err(changed(&path, "its bytes are not the ones the state holds"));
    }
    let facts = analyse(bytes, row.member != 0);
    let Kind::Object(parsed) = facts.kind else {
        return Err(changed(&path, "no longer a relocatable object"));
    };
    let object = parsed.object;
    let bad = |what: &str| changed(&path, what);
    let count = object.section_count();
    let table = object.symbols().map_err(|_| bad("symbol table"))?;

    // The ids and the stored rows (an unchanged file).
    let (first_section, stored) = match ids {
        Ids::Stored => {
            let index = view.file_index(file)?;
            let range = index.ranges[FileTable::Sections as usize];
            let stored = view.sections_of(file)?;
            // A file without `SECTIONS` rows (a member not extracted; on
            // x86-64 also one member without the `LAZY` flag, see the
            // record): the state gives it no section ids (an extraction is
            // a changed file's, `Ids::Update`). Its keys are still read.
            if stored.is_empty() && range.count == 0 {
                (SectionId(range.first), Some(stored))
            } else if stored.len() != count as usize || range.count != count {
                return Err(corrupt(format!(
                    "file {}: {} SECTIONS rows for {count} sections",
                    file.0,
                    stored.len()
                )));
            } else {
                (SectionId(range.first), Some(stored))
            }
        }
        Ids::Update { first_section } => (first_section, None),
    };

    // The non-local entries and their keys.
    let len = count as usize;
    let mut keys = Vec::new();
    let mut first_global = 0;
    let mut versioned = 0;
    if let Some(symtab) = &table {
        first_global = symtab.first_global().0;
        for (index, sym) in symtab.iter() {
            let name = symtab.name(&sym).map_err(|_| bad("symbol name"))?;
            if index.0 >= first_global {
                if name.contains(&b'@') {
                    versioned += 1;
                }
                let k = SymName::parse(name);
                let key = view.find_key(k.name, k.version.unwrap_or_default())?;
                keys.push(FrontKey {
                    name: k.name,
                    version: k.version,
                    key,
                    st_value: sym.st_value,
                    st_size: sym.st_size,
                });
            }
        }
    }

    // Each section's COMDAT group (resolve.md §6: a member of a group whose
    // flags are exactly `GRP_COMDAT`; only for a live file, as resolve
    // fills `SectionRow::comdat` for live objects alone).
    // A changed or newly extracted file (`Ids::Update`) is live in the
    // link the update makes, whatever its old row's `LAZY`.
    let live = matches!(ids, Ids::Update { .. }) || row.flags & file_flags::LAZY == 0;
    let mut comdat: Vec<Option<&'s [u8]>> = vec![None; len];
    let mut rela: Vec<Option<SectionIndex>> = vec![None; len];
    for i in 1..count {
        let s = object.section(SectionIndex(i)).map_err(|_| bad("section header"))?;
        match s.sh_type() {
            SHT_GROUP if live => {
                let g = s.group().map_err(|_| bad("group"))?;
                if g.flags() != GRP_COMDAT {
                    continue;
                }
                let sig = effective_signature(&object, table.as_ref(), &s)
                    .map_err(|_| bad("group signature"))?;
                for m in g.members() {
                    let m = m.map_err(|_| bad("group member"))?;
                    if let Some(c @ None) = comdat.get_mut(m.0 as usize) {
                        *c = Some(sig);
                    }
                }
            }
            SHT_RELA | SHT_REL => {
                let t = s.info_section().map_err(|_| bad("relocation target"))?;
                if let Some(r @ None) = rela.get_mut(t.0 as usize).filter(|_| t.0 != 0) {
                    *r = Some(SectionIndex(i));
                }
            }
            _ => {}
        }
    }

    // Each section's COMDAT signature as resolve's row gives it.
    // resolve.md §6.5: an `SHF_LINK_ORDER` section outside the groups
    // whose linked section is dropped takes that section's group. Whether
    // it is dropped is the keeper's (whole-program): an unchanged file's
    // stored row says.
    // A changed file has no stored row: the keeper decides (whole-link,
    // U-3's), so such a section is a named fall-back, never a guessed anchor.
    let mut sig = comdat.clone();
    for i in 1..count {
        let s = object.section(SectionIndex(i)).map_err(|_| bad("section header"))?;
        let link_order = s.flags() & SHF_LINK_ORDER != 0 && comdat[i as usize].is_none();
        let linked = if link_order {
            s.link().ok().and_then(|l| comdat.get(l.0 as usize).copied().flatten())
        } else {
            None
        };
        if linked.is_some() && stored.is_none() {
            return Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Resolve)));
        }
        if let (Some(l), Some(st)) = (linked, &stored)
            && let Some(r) = st.get(i as usize)
            && r.anchor_kind == 1
            && view.string(r.anchor)? == l
        {
            sig[i as usize] = Some(l);
        }
    }
    // §7.1: the anchor, by the capture's own producer.
    let (section_keys, _) =
        capture::section_keys(&object, &|i| sig.get(i.0 as usize).copied().flatten())
            .map_err(bad)?;

    let mut sections = Vec::with_capacity(len);
    let mut seen: BTreeMap<(Anchor<'s>, &'s [u8]), u32> = BTreeMap::new();
    let with_ids = if stored.as_ref().is_some_and(Vec::is_empty) { 0 } else { count };
    for i in 0..with_ids {
        let s = object.section(SectionIndex(i)).map_err(|_| bad("section header"))?;
        let name = if i > 0 { s.name().map_err(|_| bad("section name"))? } else { &[][..] };
        let id = SectionId(first_section.0 + i);
        let anchor = section_keys[i as usize].anchor;
        let ordinal = seen.entry((anchor, name)).or_insert(0);
        let this = *ordinal;
        *ordinal += 1;
        let place = stored.as_ref().and_then(|st| {
            let r = &st[i as usize];
            (r.place_chunk != NONE).then_some((ChunkId(r.place_chunk), r.place_offset))
        });
        let table = match (place, rela[i as usize]) {
            (Some(_), Some(r)) => Some(rela_table(&object, file, r).map_err(|e| bad(&e))?),
            _ => None,
        };
        // `capture::header_fields`' five, section 0 included.
        let offset = s.file_range().ok().flatten().map_or(0, |r| r.start as u64);
        sections.push(FrontSection {
            id,
            index: SectionIndex(i),
            sh_type: s.sh_type(),
            flags: s.flags(),
            size: s.size(),
            align: s.alignment().unwrap_or(0),
            offset,
            name,
            anchor,
            ordinal: this,
            place,
            rela: table,
        });
    }
    Ok(FileFront { file, row, object, bytes, sections, first_global, keys, versioned })
}

/// The table of relocation section `rel` (`layout_plan.rs` `table`'s
/// reading, over the file's own object).
fn rela_table(object: &Object<'_>, file: FileId, rel: SectionIndex) -> Result<RelaTable, String> {
    let sec = object.section(rel).map_err(|e| format!("relocation section {}: {e:?}", rel.0))?;
    let rel_form = sec.sh_type() == SHT_REL;
    let class = object.class();
    let entsize = RelaTable { file, offset: 0, count: 0, class, rel: rel_form }.entsize();
    let range = sec
        .file_range()
        .map_err(|e| format!("relocation section {}: {e:?}", rel.0))?
        .unwrap_or(0..0);
    let len = (range.end - range.start) as u64;
    if !len.is_multiple_of(entsize) {
        return Err(format!("relocation section {} is not whole records", rel.0));
    }
    let count = u32::try_from(len / entsize)
        .map_err(|_| format!("relocation section {} too large", rel.0))?;
    Ok(RelaTable { file, offset: range.start as u64, count, class, rel: rel_form })
}

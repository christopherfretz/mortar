//! Format version 2's delta tables (WP-063, `spec/incr.md` IN-18a):
//! `MENTIONS` and `RESOLVED` (IN-26's columns: what every live object
//! defines or references, and how each key, COMDAT group and file was
//! resolved) and `INDEX` (IN-25: per allocated or `.eh_frame` unit, the
//! targets its relocations name).
//!
//! The index is read from the relocations the store keeps resident
//! (allocated sections and `.eh_frame`, `spec/input.md` §11.1), copied
//! when they are not. A record whose type the architecture cannot
//! classify is recorded as a slot use as well as a reference: the
//! conservative answer (it may only add refills).

use mortar_core::hash::HashMap;
use mortar_core::par::Pool;

use mortar_arch::{Arch, DemandClass, Site};
use mortar_core::FileId;
use mortar_core::id::{SectionId, SymKey, SymbolId};
use mortar_core::inputs::{Inputs, Reader};
use mortar_core::resolution::{Def, FileLive, SectionKind};
use mortar_elf::SymbolSection;
use mortar_elf::consts::{SHF_ALLOC, SHF_LINK_ORDER, SHT_REL, SHT_RELA};
use mortar_elf::symbol::SymbolIndex;
use mortar_elf::symbol::SymbolTable;
use mortar_elf::{Object, Rel, Rela, SectionIndex, Table};

use crate::capture::{Link, Symbols};
use crate::format::{
    IndexRow, MentionRow, NONE, ResolvedRow, State, Strings, def_kind, index_kind,
};

/// Whether a relocation of this class uses a slot or the GOT's base: its
/// unit is refilled when the slot tables move (IN-25's non-relocation
/// targets).
fn uses_slot(class: DemandClass) -> bool {
    matches!(
        class,
        DemandClass::Got
            | DemandClass::GotRel
            | DemandClass::GotBaseRef
            | DemandClass::TlsGd
            | DemandClass::TlsLd
            | DemandClass::TlsIe
            | DemandClass::TlsDesc
    )
}

fn sym_kind(section: Result<SymbolSection, mortar_elf::Error>) -> u8 {
    match section {
        Ok(SymbolSection::Undefined) => def_kind::UNDEFINED,
        Ok(SymbolSection::Section(_)) => def_kind::SECTION,
        Ok(SymbolSection::Common | SymbolSection::Reserved(_)) => def_kind::COMMON,
        Ok(SymbolSection::Absolute) | Err(_) => def_kind::ABSOLUTE,
    }
}

/// Fills `MENTIONS`, `RESOLVED` and `INDEX` of `s`.
#[allow(clippy::too_many_lines)] // one block per table
pub(crate) fn fill(
    link: &Link<'_, '_>,
    syms: &mut Symbols<'_, '_>,
    strings: &mut Strings,
    s: &mut State,
    arch: &dyn Arch,
    pool: &Pool,
) -> Result<(), String> {
    let inputs = link.inputs;
    let res = link.resolution;
    let mut tables: Vec<Option<(Object<'_>, SymbolTable<'_>)>> = Vec::new();
    let mut live_files = Vec::new();
    for (file, f) in inputs.files.iter() {
        let live = res.file_live.get(file).is_some_and(|l| *l != FileLive::Dead);
        let mut entry = None;
        if live {
            live_files.push(file);
            if let Reader::Object(object) = f.reader {
                let bad = || format!("{}: symbol table", String::from_utf8_lossy(f.path));
                if let Some(t) = object.symbols().map_err(|_| bad())? {
                    entry = Some((object, t));
                }
            }
        }
        tables.push(entry);
    }
    let section_row = |file: FileId, sec: Result<SymbolSection, mortar_elf::Error>| -> u32 {
        match sec {
            Ok(SymbolSection::Section(i)) => res.section_id(file, i).map_or(NONE, |id| id.0),
            _ => NONE,
        }
    };
    // MENTIONS and INDEX are read per file in parallel with each global
    // as its `SymbolId`; the ids become rows below, in file order, so the
    // rows are numbered as a serial pass numbers them (WP-094).
    let per_file = pool.map(tables.len(), |n| -> Result<_, String> {
        let Some((object, table)) = &tables[n] else { return Ok((Vec::new(), Vec::new())) };
        let file = FileId(n as u32);
        let mut mentions = Vec::new();
        let first = table.first_global().0;
        let map = inputs.sym_map_of(file);
        for (index, sym) in table.iter() {
            if index.0 < first {
                continue;
            }
            let Some(&id) = map.get((index.0 - first) as usize) else { continue };
            let section = table.section_of(index, &sym);
            let kind = sym_kind(section);
            let row = MentionRow {
                file: file.0,
                symbol: id.0,
                kind,
                bind: sym.st_info >> 4,
                typ: sym.st_info & 0xf,
                other: sym.st_other,
                section: if kind == def_kind::SECTION { section_row(file, section) } else { NONE },
            };
            // A definition in a section without an id (one the reader
            // does not keep) is recorded as absolute: still a definition.
            let row = if row.kind == def_kind::SECTION && row.section == NONE {
                MentionRow { kind: def_kind::ABSOLUTE, ..row }
            } else {
                row
            };
            mentions.push(row);
        }
        let mut index = Vec::new();
        index_file(inputs, link, file, object, table, arch, &mut index)?;
        Ok((mentions, index))
    });
    let mut index = Vec::new();
    for part in per_file {
        let (mentions, rows) = part?;
        for mut m in mentions {
            m.symbol = syms.row(strings, SymKey::global(SymbolId(m.symbol)))?;
            s.mentions.push(m);
        }
        index.push(rows);
    }
    s.mentions.sort_by_key(|m| (m.file, m.symbol));
    s.mentions.dedup_by_key(|m| (m.file, m.symbol));
    // RESOLVED: every key, then every kept group, then every live file.
    let mut keys = Vec::new();
    let commons: HashMap<SymbolId, u64> =
        link.resolution.common.iter().rev().map(|&(k, shape)| (k, shape.size)).collect();
    for (id, &def) in res.def.iter() {
        let def = link.assignment.def(res, id).unwrap_or(def);
        keys.push(resolved_row(link, &tables, syms, strings, id, def, &section_row, &commons)?);
    }
    keys.sort_by_key(|r| r.symbol);
    s.resolved = keys;
    let mut groups = Vec::new();
    for (cid, keeper) in res.comdat.iter() {
        let (Some(file), Some(c)) = (keeper.get(), inputs.comdats.get(cid)) else { continue };
        groups.push((c.name, file.0));
    }
    groups.sort_unstable();
    groups.dedup_by_key(|g| g.0);
    for (name, file) in groups {
        s.resolved.push(ResolvedRow {
            kind: def_kind::COMDAT,
            symbol: NONE,
            file,
            section: NONE,
            name: strings.add(name),
            ..ResolvedRow::default()
        });
    }
    for file in live_files {
        s.resolved.push(ResolvedRow {
            kind: def_kind::LIVE,
            symbol: NONE,
            file: file.0,
            section: NONE,
            ..ResolvedRow::default()
        });
    }
    // INDEX: a GLOBAL or SLOT row's target is a symbol row.
    for rows in index {
        for mut r in rows {
            if r.kind == index_kind::GLOBAL || r.kind == index_kind::SLOT {
                r.target = syms.row(strings, SymKey::global(SymbolId(r.target)))?;
            }
            s.index.push(r);
        }
    }
    s.index.sort_unstable();
    s.index.dedup();
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn resolved_row(
    link: &Link<'_, '_>,
    tables: &[Option<(Object<'_>, SymbolTable<'_>)>],
    syms: &mut Symbols<'_, '_>,
    strings: &mut Strings,
    id: SymbolId,
    def: Def,
    section_row: &dyn Fn(FileId, Result<SymbolSection, mortar_elf::Error>) -> u32,
    commons: &HashMap<SymbolId, u64>,
) -> Result<ResolvedRow, String> {
    let symbol = syms.row(strings, SymKey::global(id))?;
    let mut row = ResolvedRow { symbol, file: NONE, section: NONE, ..ResolvedRow::default() };
    let props = link.resolution.props.get(id);
    match def {
        Def::Undefined { weak } => {
            row.kind = def_kind::UNDEFINED;
            row.bind = if weak { 2 } else { 1 };
            row.other = props.map_or(0, |p| p.visibility);
        }
        Def::Internal(_) => {
            row.kind = def_kind::INTERNAL;
            row.other = props.map_or(0, |p| p.visibility);
        }
        Def::Shared(e) => {
            row.kind = def_kind::SHARED;
            row.file = e.file.0;
            row.value = u64::from(e.index.0);
        }
        Def::Defined(e) | Def::Common(e) => {
            row.file = e.file.0;
            let entry = tables.get(e.file.0 as usize).and_then(Option::as_ref);
            let Some((_, table)) = entry else {
                // Not an object's symbol table: conservatively absolute
                // with the entry's index as its value (it compares).
                row.kind = def_kind::ABSOLUTE;
                row.value = u64::from(e.index.0);
                return Ok(row);
            };
            let sym = table.get(e.index).map_err(|_| "a definition's symbol")?;
            let section = table.section_of(e.index, &sym);
            row.kind =
                if matches!(def, Def::Common(_)) { def_kind::COMMON } else { sym_kind(section) };
            row.typ = sym.st_info & 0xf;
            row.bind = sym.st_info >> 4;
            row.other = sym.st_other;
            row.value = sym.st_value;
            row.size = sym.st_size;
            if row.kind == def_kind::SECTION {
                row.section = section_row(e.file, section);
                if row.section == NONE {
                    row.kind = def_kind::ABSOLUTE;
                }
            }
            if row.kind == def_kind::COMMON
                && let Some(&size) = commons.get(&id)
            {
                row.size = size;
            }
        }
    }
    Ok(row)
}

/// The records of one relocation section, either form.
enum Records<'b> {
    Rela(Table<'b, Rela>),
    Rel(Table<'b, Rel>),
}

impl Records<'_> {
    fn len(&self) -> usize {
        match self {
            Self::Rela(t) => t.len(),
            Self::Rel(t) => t.len(),
        }
    }
    /// `(r_offset, r_sym, r_type)`.
    fn get(&self, i: usize) -> Option<(u64, u32, u32)> {
        match self {
            Self::Rela(t) => t.get(i).map(|r| (r.r_offset, r.r_sym, r.r_type)),
            Self::Rel(t) => t.get(i).map(|r| (r.r_offset, r.r_sym, r.r_type)),
        }
    }
}

fn records<'b>(
    inputs: &Inputs<'_>,
    file: FileId,
    object: &Object<'b>,
    rel: SectionIndex,
    buf: &'b mut Vec<u8>,
) -> Result<Records<'b>, String> {
    let bad = |e: mortar_elf::Error| e.to_string();
    let s = object.section(rel).map_err(bad)?;
    let rela = match s.sh_type() {
        SHT_RELA => true,
        SHT_REL => false,
        t => return Err(format!("section {} applies relocations with type {t:#x}", rel.0)),
    };
    let resident = if rela {
        s.rela().map(|r| Records::Rela(r.records()))
    } else {
        s.rel().map(|r| Records::Rel(r.records()))
    };
    match resident {
        Ok(t) => Ok(t),
        Err(mortar_elf::Error::NotResident { offset, size }) => {
            let len = usize::try_from(size).map_err(|_| "relocations too large")?;
            buf.clear();
            buf.resize(len, 0);
            inputs
                .copy_to(file, offset..offset + size, buf)
                .map_err(|_| "relocations could not be read".to_owned())?;
            let buf: &'b Vec<u8> = buf;
            if rela {
                Ok(Records::Rela(s.rela_in(buf).map_err(bad)?.records()))
            } else {
                Ok(Records::Rel(s.rel_in(buf).map_err(bad)?.records()))
            }
        }
        Err(e) => Err(bad(e)),
    }
}

#[allow(clippy::too_many_arguments)]
fn index_file(
    inputs: &Inputs<'_>,
    link: &Link<'_, '_>,
    file: FileId,
    object: &Object<'_>,
    table: &SymbolTable<'_>,
    arch: &dyn Arch,
    out: &mut Vec<IndexRow>,
) -> Result<(), String> {
    let mut buf = Vec::new();
    let mut data = Vec::new();
    let (buf, data) = (&mut buf, &mut data);
    let res = link.resolution;
    let path = || String::from_utf8_lossy(inputs.files[file].path).into_owned();
    let first = table.first_global().0;
    let map = inputs.sym_map_of(file);
    for i in 1..object.section_count() {
        let index = SectionIndex(i);
        let Some(id) = res.section_id(file, index) else { continue };
        let Some(row) = res.sections.get(id) else { continue };
        if !row.is_kept() {
            continue;
        }
        let sec = object.section(index).map_err(|e| format!("{}: {e}", path()))?;
        let eh = row.kind == SectionKind::EhFrame;
        if sec.flags() & SHF_ALLOC == 0 && !eh {
            continue;
        }
        let unit = id.0;
        if sec.flags() & SHF_LINK_ORDER != 0
            && let Ok(to) = sec.link()
            && let Some(target) = res.section_id(file, to)
        {
            out.push(IndexRow { unit, kind: index_kind::LINK_ORDER, target: target.0 });
        }
        if row.reloc.0 == 0 {
            continue;
        }
        // The section's bytes, which `classify` reads at each record.
        let bytes: &[u8] = match sec.data() {
            Ok(d) => d,
            Err(mortar_elf::Error::NotResident { offset, size }) => {
                data.clear();
                data.resize(usize::try_from(size).map_err(|_| "section too large")?, 0);
                let ok = inputs.copy_to(file, offset..offset + size, data).is_ok();
                if ok { data } else { &[] }
            }
            Err(_) => &[],
        };
        let records = records(inputs, file, object, row.reloc, buf)
            .map_err(|e| format!("{}: {e}", path()))?;
        for r in 0..records.len() {
            let Some((r_offset, r_sym, r_type)) = records.get(r) else { continue };
            let slot = arch
                .classify(r_type, Site { bytes, offset: r_offset })
                .map_or(true, |h| uses_slot(h.class));
            if r_sym == 0 {
                if slot {
                    out.push(IndexRow { unit, kind: index_kind::LOCAL_SLOT, target: unit });
                }
                continue;
            }
            if r_sym >= first {
                let Some(&id) = map.get((r_sym - first) as usize) else { continue };
                let to = res.redirect.get(id).copied().unwrap_or(id);
                let key = if to == id {
                    id
                } else {
                    let undefined = table.get(SymbolIndex(r_sym)).ok().is_some_and(|y| {
                        table.section_of(SymbolIndex(r_sym), &y).ok()
                            == Some(SymbolSection::Undefined)
                    });
                    if undefined { to } else { id }
                };
                // The key's id: `fill` makes it a symbol row.
                let target = key.0;
                out.push(IndexRow { unit, kind: index_kind::GLOBAL, target });
                if slot {
                    out.push(IndexRow { unit, kind: index_kind::SLOT, target });
                }
                continue;
            }
            let Ok(y) = table.get(SymbolIndex(r_sym)) else { continue };
            let target = match table.section_of(SymbolIndex(r_sym), &y) {
                Ok(SymbolSection::Section(at)) => res.section_id(file, at),
                _ => None,
            };
            let Some(target) = target else {
                if slot {
                    out.push(IndexRow { unit, kind: index_kind::LOCAL_SLOT, target: unit });
                }
                continue;
            };
            if slot {
                out.push(IndexRow { unit, kind: index_kind::LOCAL_SLOT, target: target.0 });
            }
            if target == SectionId(unit) {
                continue;
            }
            let into_eh = res.sections.get(target).is_some_and(|t| t.kind == SectionKind::EhFrame);
            let kind = if into_eh { index_kind::EH_FRAME } else { index_kind::SECTION };
            out.push(IndexRow { unit, kind, target: target.0 });
        }
    }
    Ok(())
}

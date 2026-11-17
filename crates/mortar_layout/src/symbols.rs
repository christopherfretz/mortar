//! Stage F (`spec/layout.md` §13): the values of linker-defined symbols
//! (L-90 to L-99, L-105) and the entry point (L-102 to L-104).
//!
//! Every value is a `DefValue::At{chunk, offset}` (L-91): the chunk ids
//! and offsets are stage C's, so the table is a function of the chunk
//! order and sizes; only the entry, an address, reads stage D.

use mortar_core::assignment::Assignment;
use mortar_core::column::Column;
use mortar_core::id::{ChunkId, OutSecId, SectionId, SymEntry, SymbolId};
use mortar_core::inputs::SymbolKey;
use mortar_core::layout::{
    AllocKey, Chunk, ChunkKind, DefValue, Defined, EntryDef, Place, SymPlace,
};
use mortar_core::resolution::{Def, InternalDef, Resolution, SectionKind};
use mortar_elf::Machine;
use mortar_elf::consts::{SHF_EXECINSTR, SHT_NOBITS};

use crate::diag::{LayoutDiag, entry_not_found, feature, internal, text};
use crate::rank::Rank;
use crate::target::{EntryName, Options, Target, parse_number};

/// What stage F reads.
#[derive(Debug, Clone, Copy)]
pub struct SymbolInputs<'t, 'n> {
    pub resolution: &'t Resolution,
    pub assignment: &'t Assignment<'n>,
    pub symbols: &'t Column<SymbolId, SymbolKey<'n>>,
    /// Stage D's chunks (addresses are read only for the entry).
    pub chunks: &'t Column<ChunkId, Chunk>,
    pub order: &'t [ChunkId],
    pub ranks: &'t Column<ChunkId, Rank>,
    pub out_of: &'t Column<ChunkId, Option<OutSecId>>,
    pub place: &'t Column<SectionId, Place>,
    pub sym_place: &'t [SymPlace],
}

/// The result of stage F.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbols {
    /// Sorted by symbol (model §4.4 rule 5).
    pub defined: Box<[Defined]>,
    pub entry: Option<u64>,
    /// `entry-not-found` (L-103).
    pub warnings: Vec<LayoutDiag>,
}

/// The first and last chunk (in file order) of each output section or
/// unjoined synthetic chunk, by name.
struct Bounds<'n> {
    rows: Vec<(&'n [u8], ChunkId, ChunkId)>,
}

impl<'n> Bounds<'n> {
    fn new(cx: &SymbolInputs<'_, 'n>) -> Self {
        let mut rows: Vec<(&'n [u8], ChunkId, ChunkId)> = Vec::new();
        let mut index: Vec<Option<usize>> = vec![None; cx.assignment.outsecs.len() as usize];
        for &c in cx.order {
            let chunk = &cx.chunks[c];
            if !chunk.is_alloc() && chunk.kind != ChunkKind::FileHeader {
                continue;
            }
            match cx.out_of.get(c).copied().flatten() {
                Some(o) => {
                    if let Some(i) = index[o.0 as usize] {
                        rows[i].2 = c;
                    } else {
                        index[o.0 as usize] = Some(rows.len());
                        rows.push((cx.assignment.outsecs[o].name, c, c));
                    }
                }
                None => {
                    if let ChunkKind::Synthetic(s) = chunk.kind {
                        rows.push((s.name, c, c));
                    }
                }
            }
        }
        Self { rows }
    }

    fn find(&self, name: &[u8]) -> Option<(ChunkId, ChunkId)> {
        self.rows.iter().find(|r| r.0 == name).map(|r| (r.1, r.2))
    }
}

/// A chunk's start and end.
fn start(c: ChunkId) -> DefValue {
    DefValue::At { chunk: c, offset: 0 }
}

fn end(chunks: &Column<ChunkId, Chunk>, c: ChunkId) -> DefValue {
    DefValue::At { chunk: c, offset: chunks[c].size }
}

/// The values every table row is built from.
struct Table<'t, 'n> {
    cx: SymbolInputs<'t, 'n>,
    bounds: Bounds<'n>,
    machine: Machine,
    base: DefValue,
}

impl Table<'_, '_> {
    fn chunks(&self) -> &Column<ChunkId, Chunk> {
        self.cx.chunks
    }

    fn alloc_order(&self) -> impl DoubleEndedIterator<Item = ChunkId> + '_ {
        self.cx.order.iter().copied().filter(|&c| self.cx.chunks[c].is_alloc())
    }

    fn section_start(&self, name: &[u8]) -> Option<DefValue> {
        self.bounds.find(name).map(|(first, _)| start(first))
    }

    fn section_end(&self, name: &[u8]) -> Option<DefValue> {
        self.bounds.find(name).map(|(_, last)| end(self.chunks(), last))
    }

    /// `edata`: the end of the last allocated chunk with file content
    /// (L-95), the x86-64 large sections aside (L-36a: they follow
    /// `.bss`; GNU ld's `_edata` is before `.bss` too); the base when
    /// there is none.
    fn edata(&self) -> DefValue {
        self.alloc_order()
            .rfind(|&c| self.chunks()[c].in_file() && !self.chunks()[c].is_large())
            .map_or(self.base, |c| end(self.chunks(), c))
    }

    /// `end`: the end of the last allocated chunk in memory, `.tbss` aside.
    fn end(&self) -> DefValue {
        self.alloc_order()
            .rfind(|&c| self.chunks()[c].takes_memory())
            .map_or(self.base, |c| end(self.chunks(), c))
    }

    fn bss_start(&self) -> DefValue {
        self.alloc_order()
            .find(|&c| {
                let ch = &self.chunks()[c];
                self.cx.ranks[c].rank == 8
                    && ch.sh_type == SHT_NOBITS
                    && !ch.is_tls()
                    && !ch.is_large()
            })
            .map_or_else(|| self.edata(), start)
    }

    /// `etext`: the end of the last exec chunk, `.ltext` aside (L-36a).
    fn etext(&self) -> DefValue {
        self.alloc_order()
            .rfind(|&c| self.chunks()[c].flags & SHF_EXECINSTR != 0 && !self.chunks()[c].is_large())
            .map_or(self.base, |c| end(self.chunks(), c))
    }

    fn array(&self, name: &[u8], at_end: bool) -> DefValue {
        let v = if at_end { self.section_end(name) } else { self.section_start(name) };
        v.unwrap_or(self.base)
    }

    fn iplt(&self, at_end: bool) -> DefValue {
        let name: &[u8] = if self.machine == Machine::I386 { b".rel.plt" } else { b".rela.plt" };
        if let Some(v) = if at_end { self.section_end(name) } else { self.section_start(name) } {
            return v;
        }
        // Absent: both the start of rank 4, or the base.
        self.alloc_order().find(|&c| self.cx.ranks[c].rank == 4).map_or(self.base, start)
    }

    /// The value of one §13.1 name (`Ok(None)`: not a name of the table).
    fn value(&self, name: &[u8]) -> Result<Option<DefValue>, LayoutDiag> {
        let aarch64 = self.machine == Machine::AArch64;
        let rela_iplt: (&[u8], &[u8]) = if self.machine == Machine::I386 {
            (b"__rel_iplt_start", b"__rel_iplt_end")
        } else {
            (b"__rela_iplt_start", b"__rela_iplt_end")
        };
        let v = match name {
            b"__ehdr_start" | b"__executable_start" => self.base,
            b"etext" | b"_etext" | b"__etext" => self.etext(),
            b"edata" | b"_edata" => self.edata(),
            b"__bss_start" => self.bss_start(),
            b"end" | b"_end" => self.end(),
            b"__bss_start__" if aarch64 => self.bss_start(),
            b"__bss_end__" | b"_bss_end__" | b"__end__" if aarch64 => self.end(),
            b"__data_start" if aarch64 => {
                self.section_start(b".data").unwrap_or_else(|| self.edata())
            }
            b"__tdata_start" => self
                .section_start(b".tdata")
                .ok_or_else(|| feature("__tdata_start without .tdata"))?,
            b"__preinit_array_start" => self.array(b".preinit_array", false),
            b"__preinit_array_end" => self.array(b".preinit_array", true),
            b"__init_array_start" => self.array(b".init_array", false),
            b"__init_array_end" => self.array(b".init_array", true),
            b"__fini_array_start" => self.array(b".fini_array", false),
            b"__fini_array_end" => self.array(b".fini_array", true),
            n if n == rela_iplt.0 => self.iplt(false),
            n if n == rela_iplt.1 => self.iplt(true),
            b"_GLOBAL_OFFSET_TABLE_" => {
                let got: &[u8] = if aarch64 { b".got" } else { b".got.plt" };
                self.section_start(got)
                    .ok_or_else(|| internal("_GLOBAL_OFFSET_TABLE_ with no GOT"))?
            }
            b"__GNU_EH_FRAME_HDR" => self
                .section_start(b".eh_frame_hdr")
                .ok_or_else(|| internal("__GNU_EH_FRAME_HDR confirmed with no .eh_frame_hdr"))?,
            // §13.1: M2, the start of `.dynamic` (spec/dyn.md D-32).
            b"_DYNAMIC" => self
                .section_start(b".dynamic")
                .ok_or_else(|| internal("_DYNAMIC defined with no .dynamic"))?,
            b"_TLS_MODULE_BASE_" => {
                // L-98: tls.start, in .tdata's index or .tbss's.
                let first = self.alloc_order().find(|&c| self.chunks()[c].is_tls());
                first.map(start).ok_or_else(|| feature("_TLS_MODULE_BASE_ without TLS"))?
            }
            n => {
                if let Some(sec) = n.strip_prefix(b"__start_") {
                    self.section_start(sec)
                        .ok_or_else(|| internal("__start_ confirmed with no output section"))?
                } else if let Some(sec) = n.strip_prefix(b"__stop_") {
                    self.section_end(sec)
                        .ok_or_else(|| internal("__stop_ confirmed with no output section"))?
                } else {
                    return Ok(None);
                }
            }
        };
        Ok(Some(v))
    }

    /// The value of a symbol defined in an input section, as a chunk
    /// offset.
    fn in_section(&self, entry: SymEntry, def: EntryDef) -> Result<DefValue, LayoutDiag> {
        match def {
            EntryDef::Absolute(v) => Ok(DefValue::Absolute(v)),
            EntryDef::Section(index, value) => {
                let res = self.cx.resolution;
                let s = res
                    .section_id(entry.file, index)
                    .ok_or_else(|| internal("a symbol in a section that does not exist"))?;
                if res.sections[s].kind == SectionKind::Merge {
                    return Err(feature("entry or alias in a merged section"));
                }
                let (chunk, off) = self.cx.place[s]
                    .get()
                    .ok_or_else(|| internal("a defined symbol in a section with no place"))?;
                Ok(DefValue::At { chunk, offset: off + value })
            }
        }
    }

    fn common(&self, s: SymbolId) -> Option<DefValue> {
        self.cx
            .sym_place
            .iter()
            .find(|p| p.key == AllocKey::Common(s))
            .map(|p| DefValue::At { chunk: p.chunk, offset: p.offset })
    }

    fn name(&self, s: SymbolId) -> &[u8] {
        self.cx.symbols.get(s).map_or(&b""[..], |k| k.name)
    }

    /// The value of key `s` through its definition, `Ok(None)` when it is
    /// undefined.
    fn symbol(
        &self,
        s: SymbolId,
        entry_def: &impl Fn(SymEntry) -> Option<EntryDef>,
        depth: u32,
    ) -> Result<Option<DefValue>, LayoutDiag> {
        let res = self.cx.resolution;
        let Some(def) = self.cx.assignment.def(res, s) else {
            return Err(internal("a symbol id out of range"));
        };
        match def {
            Def::Undefined { .. } => Ok(None),
            Def::Defined(e) => {
                let d =
                    entry_def(e).ok_or_else(|| internal("a defined entry that cannot be read"))?;
                self.in_section(e, d).map(Some)
            }
            Def::Common(_) => {
                self.common(s).map(Some).ok_or_else(|| internal("a common with no allocation"))
            }
            Def::Shared(_) => Err(feature("entry or alias in a shared object")),
            Def::Internal(InternalDef::Provide { .. }) => {
                let name = self.name(s);
                self.value(name)?.map(Some).ok_or_else(|| {
                    internal(format!("linker-defined symbol {} is not in the table", text(name)))
                })
            }
            Def::Internal(InternalDef::Alias(to)) => {
                if depth > 64 {
                    return Err(internal("a cycle of --defsym aliases"));
                }
                self.symbol(to, entry_def, depth + 1)
            }
            Def::Internal(InternalDef::Assign(_)) => Err(feature("--defsym expression")),
        }
    }

    fn addr(&self, v: DefValue) -> Option<u64> {
        match v {
            DefValue::Absolute(a) => Some(a),
            DefValue::At { chunk, offset } => self.cx.chunks.get(chunk)?.addr?.checked_add(offset),
        }
    }
}

/// Stage F.
///
/// `entry_def` reads a defined entry (`Inputs::entry_def`).
///
/// # Errors
///
/// The refusals of L-98, L-104 and L-105, an alias of a symbol mortar
/// cannot express as a chunk offset, and `internal-error` for a table
/// earlier stages cannot have made.
pub fn symbols(
    cx: SymbolInputs<'_, '_>,
    target: &Target,
    options: &Options,
    entry_def: impl Fn(SymEntry) -> Option<EntryDef>,
) -> Result<Symbols, LayoutDiag> {
    let header = cx
        .order
        .iter()
        .copied()
        .find(|&c| cx.chunks[c].kind == ChunkKind::FileHeader)
        .ok_or_else(|| internal("no file header chunk"))?;
    let table =
        Table { cx, bounds: Bounds::new(&cx), machine: target.machine, base: start(header) };
    let mut defined = Vec::new();
    for (s, def) in cx.resolution.def.iter() {
        if !matches!(def, Def::Internal(_)) || cx.assignment.is_withdrawn(s) {
            continue;
        }
        let value = table
            .symbol(s, &entry_def, 0)?
            .ok_or_else(|| feature("--defsym alias of an undefined symbol"))?;
        defined.push(Defined { symbol: s, value });
    }
    let (entry, warnings) = entry(&table, options, &entry_def, &defined)?;
    Ok(Symbols { defined: defined.into(), entry, warnings })
}

/// L-102 to L-104.
fn entry(
    table: &Table<'_, '_>,
    options: &Options,
    entry_def: &impl Fn(SymEntry) -> Option<EntryDef>,
    defined: &[Defined],
) -> Result<(Option<u64>, Vec<LayoutDiag>), LayoutDiag> {
    let cx = table.cx;
    // `spec/dyn.md` D-101: a shared object's default `_start` gives 0
    // silently when undefined; `-e`'s symbol keeps L-103's warning.
    let (name, warn) = match &options.entry {
        EntryName::None => return Ok((None, Vec::new())),
        EntryName::Address(a) => return in_code(table, *a).map(|a| (Some(a), Vec::new())),
        EntryName::Symbol(name) => (name, true),
        EntryName::IfDefined(name) => (name, false),
    };
    let key = cx
        .symbols
        .iter()
        .find(|(_, k)| k.name == name.as_slice() && k.version.is_none())
        .map(|(s, _)| s);
    let value = match key {
        // The entry names its symbol, not an undefined mention: `--wrap`
        // does not redirect it (`spec/resolve.md` §7.1: GNU ld and mold
        // take `SYM` itself; WP-153). A `--defsym` alias is its `def`.
        Some(s) => match cx.assignment.def(cx.resolution, s) {
            Some(Def::Internal(_)) => {
                defined.binary_search_by_key(&s, |d| d.symbol).ok().map(|i| defined[i].value)
            }
            _ => table.symbol(s, entry_def, 0)?,
        },
        None => None,
    };
    let Some(value) = value else {
        // L-102: a number that is not a defined symbol is an address.
        if let Some(a) = std::str::from_utf8(name).ok().and_then(parse_number) {
            return in_code(table, a).map(|a| (Some(a), Vec::new()));
        }
        return Ok((None, if warn { vec![entry_not_found(name)] } else { Vec::new() }));
    };
    let addr = table.addr(value).ok_or_else(|| internal("the entry symbol has no address"))?;
    in_code(table, addr).map(|a| (Some(a), Vec::new()))
}

/// L-104: the entry lies in an exec chunk.
/// An entry from the input (a symbol or `-e` number) outside code is
/// refused; mortar computes no entry of its own.
fn in_code(table: &Table<'_, '_>, addr: u64) -> Result<u64, LayoutDiag> {
    let inside = table.cx.order.iter().map(|&c| &table.cx.chunks[c]).any(|c| {
        c.is_alloc()
            && c.flags & SHF_EXECINSTR != 0
            && c.addr.is_some_and(|a| addr >= a && addr - a < c.size)
    });
    if inside { Ok(addr) } else { Err(feature("entry point outside code")) }
}

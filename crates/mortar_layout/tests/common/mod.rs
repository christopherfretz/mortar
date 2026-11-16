//! A synthetic link: files of section headers, keys and their
//! definitions, run through every stage, and checked by `Layout::check`.
#![allow(dead_code)]

pub mod elf;

use mortar_core::assignment::Assignment;
use mortar_core::check::{CheckContext, Violation};
use mortar_core::column::Column;
use mortar_core::demands::Demands;
use mortar_core::id::OptId;
use mortar_core::id::{FileId, IdRange, SectionId, SymEntry, SymbolId};
use mortar_core::inputs::SymbolKey;
use mortar_core::layout::{EntryDef, Folds, Layout, MergeMap, Slots};
use mortar_core::liveness::Liveness;
use mortar_core::par::Pool;
use mortar_core::resolution::{Def, FileLive, Resolution, SectionKind, SectionRow, SectionState};
use mortar_core::split::Split;
use mortar_elf::consts::{SHF_ALLOC, SHT_NOBITS, SHT_NOTE};
use mortar_elf::{Machine, SectionIndex, SymbolIndex};
use mortar_layout::assign::assign;
use mortar_layout::chunk::{Common, SynthChunk};
use mortar_layout::headers::HeaderShapes;
use mortar_layout::stack::StackNote;
use mortar_layout::{
    Header, Headers, LayoutDiag, Link, Options, Placed, RangeExtension, Target, place,
};

/// One input section.
#[derive(Debug, Clone, Copy)]
pub struct Sec {
    pub name: &'static [u8],
    pub ty: u32,
    pub flags: u64,
    pub size: u64,
    /// log2.
    pub align: u8,
    /// `sh_link`, an index in the same file.
    pub link: u32,
}

pub const fn sec(name: &'static [u8], ty: u32, flags: u64, size: u64, align: u8) -> Sec {
    Sec { name, ty, flags, size, align, link: 0 }
}

/// One section of [`World::from_rows`].
#[derive(Debug, Clone, Copy)]
pub struct Row {
    pub header: Header<'static>,
    /// `sh_link` as an index in the same file (0: none).
    pub link: u32,
    pub kind: SectionKind,
    pub state: SectionState,
}

/// The link.
#[derive(Debug, Clone)]
pub struct World {
    pub headers: Headers<'static>,
    pub resolution: Resolution,
    pub symbols: Column<SymbolId, SymbolKey<'static>>,
    pub liveness: Liveness,
    pub folds: Folds,
    pub split: Split,
    pub merge: MergeMap,
    pub commons: Vec<Common>,
    pub synthetic: Vec<SynthChunk>,
    pub stack_notes: Vec<(String, StackNote)>,
    pub target: Target,
    pub options: Options,
    /// What `Inputs::entry_def` would read.
    pub entries: Vec<(SymEntry, EntryDef)>,
}

fn kind_of(s: &Sec) -> SectionKind {
    if s.name == b".note.GNU-stack" {
        SectionKind::Stack
    } else if s.ty == SHT_NOBITS {
        SectionKind::Nobits
    } else if s.ty == SHT_NOTE {
        SectionKind::Note
    } else if s.flags & SHF_ALLOC == 0 {
        SectionKind::Debug
    } else {
        SectionKind::Regular
    }
}

impl World {
    /// Files 1.. hold `files` (file 0 is the internal file); every section
    /// is kept and live.
    pub fn new(machine: Machine, files: &[Vec<Sec>], symbols: &[(&'static [u8], Def)]) -> Self {
        let rows: Vec<Vec<Row>> = files
            .iter()
            .map(|f| {
                f.iter()
                    .map(|s| Row {
                        header: Header {
                            name: s.name,
                            sh_type: s.ty,
                            flags: s.flags,
                            size: s.size,
                            align: s.align,
                            link: None,
                            entsize: 0,
                        },
                        link: s.link,
                        kind: kind_of(s),
                        state: SectionState::Kept,
                    })
                    .collect()
            })
            .collect();
        Self::from_rows(machine, &rows, symbols)
    }

    /// [`World::new`] from section rows (index 1.. of each file; `link`
    /// in the row, not the header, as an index in the same file).
    pub fn from_rows(
        machine: Machine,
        files: &[Vec<Row>],
        symbols: &[(&'static [u8], Def)],
    ) -> Self {
        let mut headers = Vec::new();
        let mut rows = Vec::new();
        // File 0, the internal file, has no sections.
        let mut base = vec![IdRange::empty(SectionId(0))];
        let mut next = 0u32;
        for f in files {
            let start = next;
            headers.push(Header::EMPTY);
            rows.push(SectionRow {
                kind: SectionKind::Null,
                state: SectionState::Kept,
                reloc: SectionIndex(0),
                comdat: OptId::NONE,
            });
            next += 1;
            for r in f {
                let mut h = r.header;
                h.link = (r.link != 0).then_some(SectionId(start + r.link));
                headers.push(h);
                rows.push(SectionRow {
                    kind: r.kind,
                    state: r.state,
                    reloc: SectionIndex(0),
                    comdat: OptId::NONE,
                });
                next += 1;
            }
            base.push(IdRange::new(SectionId(start), SectionId(next)));
        }
        let nfiles = files.len() as u32 + 1;
        let mut resolution = Resolution::empty(nfiles, symbols.len() as u32, 0);
        resolution.file_live = Column::filled(nfiles, FileLive::Always);
        resolution.section_base = Column::from_vec(base).unwrap();
        resolution.sections = Column::from_vec(rows).unwrap();
        for (i, (_, def)) in symbols.iter().enumerate() {
            resolution.def[SymbolId(i as u32)] = *def;
        }
        let keys = symbols.iter().map(|&(name, _)| SymbolKey { name, version: None, hash: 0 });
        let n = headers.len() as u32;
        let target = Target::of(machine);
        let options = Options::defaults(&target);
        Self {
            headers: Column::from_vec(headers).unwrap(),
            resolution,
            symbols: Column::from_vec(keys.collect()).unwrap(),
            liveness: Liveness { live: Column::filled(n, true), frag_live: Column::new() },
            folds: Folds::default(),
            split: Split::default(),
            merge: MergeMap::default(),
            commons: Vec::new(),
            synthetic: Vec::new(),
            stack_notes: Vec::new(),
            target,
            options,
            entries: Vec::new(),
        }
    }

    /// The section id of index `index` of file `file` (1-based files).
    pub fn sid(&self, file: u32, index: u32) -> SectionId {
        self.resolution.section_id(FileId(file), SectionIndex(index)).unwrap()
    }

    /// Defines key `symbol` at `value` in section `index` of `file`.
    pub fn define(&mut self, symbol: u32, file: u32, index: u32, value: u64) {
        let entry = SymEntry { file: FileId(file), index: SymbolIndex(symbol + 1) };
        self.resolution.def[SymbolId(symbol)] = Def::Defined(entry);
        self.entries.retain(|(e, _)| *e != entry);
        self.entries.push((entry, EntryDef::Section(SectionIndex(index), value)));
    }

    pub fn stage_a(
        &self,
        pool: &Pool,
    ) -> Result<Assignment<'static>, Vec<(SectionId, LayoutDiag)>> {
        assign(&self.headers, &self.resolution, &self.symbols, &self.target, &self.options, pool)
            .map(|a| a.assignment)
    }

    pub fn link(&self) -> Link<'_, 'static> {
        Link {
            headers: &self.headers,
            resolution: &self.resolution,
            symbols: &self.symbols,
            liveness: &self.liveness,
            folds: &self.folds,
            split: &self.split,
            merge: &self.merge,
            commons: &self.commons,
            synthetic: &self.synthetic,
            synth_holes: &[],
            stack_notes: &self.stack_notes,
            target: &self.target,
            options: &self.options,
        }
    }

    /// Every stage, with the range extension `range`.
    pub fn run_with(
        &self,
        range: Option<&RangeExtension<'_>>,
        pool: &Pool,
    ) -> Result<(Assignment<'static>, Placed), Vec<LayoutDiag>> {
        let mut a =
            self.stage_a(pool).map_err(|v| v.into_iter().map(|(_, d)| d).collect::<Vec<_>>())?;
        let entries = self.entries.clone();
        let placed = place(
            &self.link(),
            &mut a,
            range,
            move |e| entries.iter().find(|(x, _)| *x == e).map(|(_, d)| *d),
            pool,
        )?;
        Ok((a, placed))
    }

    pub fn run(&self, pool: &Pool) -> Result<(Assignment<'static>, Placed), Vec<LayoutDiag>> {
        self.run_with(None, pool)
    }

    /// `Layout::check` of a result.
    pub fn check(
        &self,
        assignment: &Assignment<'static>,
        placed: &Placed,
    ) -> Result<(), Violation> {
        let layout = Layout {
            folds: self.folds.clone(),
            merge: self.merge.clone(),
            slots: Slots {
                symtab_locals: Column::filled(
                    self.resolution.file_live.len(),
                    mortar_core::id::Span::default(),
                ),
                ..Slots::default()
            },
            placement: placed.placement.clone(),
        };
        let demands = Demands::empty(self.symbols.len());
        let shapes = HeaderShapes(&self.headers);
        let cx = CheckContext {
            class: self.target.class,
            page_size: self.options.max_page,
            needs_entry: placed.placement.entry.is_some(),
            segment_rules: placed.rules,
            resolution: &self.resolution,
            assignment,
            split: &self.split,
            liveness: &self.liveness,
            demands: &demands,
            shapes: &shapes,
        };
        layout.validate(&cx)
    }
}

/// Prints a placement (debugging aid).
pub fn dump(placed: &Placed) {
    let p = &placed.placement;
    for &c in &p.order {
        let ch = &p.chunks[c];
        eprintln!(
            "{:?} addr={:?} off={:#x} size={:#x} align={} shndx={:?}",
            ch.kind,
            ch.addr.map(|a| format!("{a:#x}")),
            ch.offset,
            ch.size,
            ch.align,
            ch.shndx
        );
    }
    for (_, s) in p.segments.iter() {
        eprintln!(
            "type={:#x} flags={} off={:#x} va={:#x} fs={:#x} ms={:#x} al={}",
            s.p_type, s.flags, s.offset, s.addr, s.filesz, s.memsz, s.align
        );
    }
}

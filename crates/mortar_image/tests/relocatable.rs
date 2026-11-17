//! WP-184 (RL-4): the image of a `-r` link (`spec/image.md` §17, I-R1 to
//! I-R6, I-R8, I-R11, I-R12; `spec/arch.md` A-R1, A-R4).
//!
//! RL-2's resolution and RL-3's layout are written at the same time as
//! this package, so the tables here are HAND-MADE by a small model of
//! the two (`World`): every section of a live object kept, the keys
//! resolved strong > common > weak > undefined, one output section per
//! (name, type, flags) in the order of first occurrence, the pieces at
//! their alignment in input order, `.eh_frame` as the synthetic chunk of
//! the rulings (S1, S12), offsets in chunk order. The sizes of the
//! bodies, `.eh_frame`, the relocation sections are the model's own; the
//! sizes of the symbol and string tables and the notes come from
//! `relocatable::sizes` (the rulings' single source to be). The objects
//! are compiled from the design's C corpus
//! (`tests/relocatable-design/exp/corpus`), read back with `readelf` and
//! `mortar_elf`, and linked finally by GNU ld and lld into programs that
//! run and print what the direct link prints.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mortar_a64::A64;
use mortar_arch::Arch;
use mortar_core::assignment::{OutOf, OutSec};
use mortar_core::column::Column;
use mortar_core::inputs::{Inputs, Reader};
use mortar_core::layout::{Chunk, ChunkKind, Part, Place, Synth};
use mortar_core::par::Pool;
use mortar_core::plan::Plan;
use mortar_core::resolution::{
    CommonShape, Def, DropReason, FileLive, RefFlags, SectionKind, SectionRow, SectionState,
};
use mortar_core::split::{EhPiece, SplitRow};
use mortar_core::{
    Assignment, ChunkId, FileId, IdRange, Layout, Liveness, OptId, OutSecId, Placement,
    PlacementKind, RelocatableLayout, Resolution, SectionId, Span, Split, SymEntry, SymbolId,
};
use mortar_diag::{Collector, Policy};
use mortar_elf::consts::{
    ET_REL, SHF_ALLOC, SHF_EXCLUDE, SHN_COMMON, SHN_UNDEF, SHT_GROUP, SHT_NOBITS, SHT_NOTE,
    SHT_NULL, SHT_REL, SHT_RELA, SHT_STRTAB, SHT_SYMTAB, SHT_SYMTAB_SHNDX, STB_GLOBAL, STB_WEAK,
};
use mortar_elf::{Object, SectionIndex, SymbolSection};
use mortar_image::output::OutputOptions;
use mortar_image::relocatable::{RelocatableLink, map_of, sizes, write};
use mortar_input::load::{LoadEnv, load};
use mortar_input::store::{Store, StorePolicy};
use mortar_testutil::target::Arch as TArch;
use mortar_testutil::{Scratch, named_skip, run_with, tools_required};

const CORPUS: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/relocatable-design/exp/corpus");

// ---- the hand-made tables ----------------------------------------------------

/// The `-r` tables of a set of objects, made by the model above.
struct World {
    arch: &'static dyn Arch,
    plan: Plan,
    inputs: &'static Inputs<'static>,
    resolution: Resolution,
    liveness: Liveness,
    split: Split,
    layout: RelocatableLayout<'static>,
}

fn obj(inputs: &Inputs<'static>, f: FileId) -> Option<Object<'static>> {
    match inputs.files.get(f)?.reader {
        Reader::Object(o) => Some(o),
        _ => None,
    }
}

fn arch_of(emulation: &str) -> &'static dyn Arch {
    match emulation {
        "-maarch64linux" => &A64::M1,
        "-melf_x86_64" => &mortar_x64::X64::M1,
        "-melf_i386" => &mortar_x86::X86::M1,
        _ => unreachable!(),
    }
}

fn plan_of(dir: &Path, words: &[&str]) -> Plan {
    let args: Vec<OsString> = words.iter().map(OsString::from).collect();
    let _ = dir;
    mortar_cli::parse(&args).plan
}

fn load_inputs(dir: &Path, plan: &Plan) -> &'static Inputs<'static> {
    let store: &'static Store = Box::leak(Box::new(Store::new(StorePolicy::Resident)));
    let diag = Collector::new(Policy::default());
    let env = LoadEnv { cwd: dir.to_path_buf(), pool: Pool::new(4) };
    let inputs = load(plan, &env, store, &diag).expect("the inputs load");
    Box::leak(Box::new(inputs))
}

/// The model's classification of an input section (RL-2's at `-r`, S13).
fn classify(name: &[u8], sh_type: u32, flags: u64, index: u32) -> (SectionKind, SectionState) {
    let kept = SectionState::Kept;
    if index == 0 || sh_type == SHT_NULL {
        return (SectionKind::Null, kept);
    }
    match sh_type {
        SHT_SYMTAB | SHT_SYMTAB_SHNDX => return (SectionKind::Symtab, kept),
        SHT_STRTAB => return (SectionKind::Strtab, kept),
        SHT_RELA | SHT_REL => return (SectionKind::Reloc, kept),
        SHT_GROUP => return (SectionKind::Group, kept),
        // `.llvm_addrsig` (L-R1: dropped at `-r`).
        0x6fff_4c03 => return (SectionKind::Excluded, SectionState::Dropped(DropReason::Excluded)),
        _ => {}
    }
    if name == b".eh_frame" {
        return (SectionKind::EhFrame, kept);
    }
    if sh_type == SHT_NOTE {
        return (SectionKind::Note, kept);
    }
    if sh_type == SHT_NOBITS {
        return (SectionKind::Nobits, kept);
    }
    if flags & SHF_ALLOC == 0 && flags & SHF_EXCLUDE == 0 {
        return (SectionKind::Debug, kept);
    }
    (SectionKind::Regular, kept)
}

fn rank(def: Def, weak_def: bool) -> u8 {
    match def {
        Def::Defined(_) if !weak_def => 3,
        Def::Common(_) => 2,
        Def::Defined(_) => 1,
        _ => 0,
    }
}

fn log2(a: u64) -> u8 {
    a.max(1).next_power_of_two().trailing_zeros() as u8
}

fn align_up(x: u64, a: u64) -> u64 {
    if a <= 1 { x } else { x.div_ceil(a) * a }
}

impl World {
    /// The tables of the objects of the command line `words` in `dir`.
    #[allow(clippy::too_many_lines)]
    fn new(dir: &Path, words: &[&str]) -> Self {
        let plan = plan_of(dir, words);
        let arch = arch_of(words[0]);
        let inputs = load_inputs(dir, &plan);
        let files = inputs.files.len();
        let keys = inputs.symbols.len();
        let mut res = Resolution::empty(files, keys, inputs.comdats.len());
        let mut rows: Vec<SectionRow> = Vec::new();
        let mut weak_def = vec![false; keys as usize];
        let mut undef_weak = vec![true; keys as usize];
        let mut commons: Vec<(SymbolId, CommonShape)> = Vec::new();
        for (f, _) in inputs.files.iter() {
            let start = SectionId(rows.len() as u32);
            let Some(o) = obj(inputs, f) else {
                res.file_live[f] = FileLive::Always;
                res.section_base[f] = IdRange::empty(start);
                continue;
            };
            res.file_live[f] = FileLive::Always;
            let base = rows.len();
            for s in o.sections() {
                let h = *s.header();
                let (kind, state) =
                    classify(s.name().unwrap_or(b""), h.sh_type, h.sh_flags, s.index().0);
                rows.push(SectionRow { kind, state, reloc: SectionIndex(0), comdat: OptId::NONE });
            }
            for s in o.sections() {
                let h = s.header();
                if (h.sh_type == SHT_RELA || h.sh_type == SHT_REL) && h.sh_info != 0 {
                    rows[base + h.sh_info as usize].reloc = s.index();
                }
            }
            res.section_base[f] = IdRange::new(start, SectionId(rows.len() as u32));
            // The keys.
            let Some(t) = o.symbols().unwrap() else { continue };
            let map = inputs.sym_map_of(f);
            for (index, s) in t.globals() {
                let id = map[(index.0 - t.first_global().0) as usize];
                let k = id.0 as usize;
                let props = &mut res.props[id];
                props.refs |= RefFlags::FROM_OBJECT;
                let vis = s.st_other & 3;
                if vis != 0 && (props.visibility == 0 || vis < props.visibility) {
                    props.visibility = vis;
                }
                let entry = SymEntry { file: f, index };
                let new = match s.st_shndx {
                    SHN_UNDEF => {
                        if s.binding() != STB_WEAK {
                            undef_weak[k] = false;
                            props.refs |= RefFlags::STRONG;
                        }
                        continue;
                    }
                    SHN_COMMON => {
                        let shape =
                            CommonShape { size: s.st_size, align: log2(s.st_value), large: false };
                        match commons.iter_mut().find(|c| c.0 == id) {
                            Some(c) => {
                                c.1.size = c.1.size.max(shape.size);
                                c.1.align = c.1.align.max(shape.align);
                            }
                            None => commons.push((id, shape)),
                        }
                        Def::Common(entry)
                    }
                    _ => Def::Defined(entry),
                };
                let is_weak = s.binding() == STB_WEAK;
                if rank(new, is_weak) > rank(res.def[id], weak_def[k]) {
                    res.def[id] = new;
                    weak_def[k] = is_weak;
                    res.props[id].binding = if is_weak { STB_WEAK } else { STB_GLOBAL };
                }
            }
        }
        for (id, _) in inputs.symbols.iter() {
            if let Def::Undefined { .. } = res.def[id]
                && res.props[id].refs.contains(RefFlags::FROM_OBJECT)
            {
                res.def[id] = Def::Undefined { weak: undef_weak[id.0 as usize] };
            }
        }
        commons.retain(|c| matches!(res.def[c.0], Def::Common(_)));
        commons.sort_by_key(|c| c.0);
        res.common = commons.into_boxed_slice();
        let nsec = rows.len() as u32;
        res.sections = Column::from_vec(rows).unwrap();
        let mut live = Liveness::empty(nsec, 0);
        for (s, r) in res.sections.iter() {
            live.live[s] = r.is_kept();
        }
        let split = eh_split(inputs, &res);
        let mut w = Self {
            arch,
            plan,
            inputs,
            resolution: res,
            liveness: live,
            split,
            layout: RelocatableLayout::default(),
        };
        w.layout = w.lay_out();
        w
    }

    fn sections_of(&self, f: FileId) -> impl Iterator<Item = (SectionId, SectionIndex)> + '_ {
        let r = self.resolution.section_base[f];
        (r.start.0..r.end.0).map(move |s| (SectionId(s), SectionIndex(s - r.start.0)))
    }

    /// The model's RL-3: L-R1's output sections, the chunks, their sizes
    /// and offsets.
    #[allow(clippy::too_many_lines)]
    fn lay_out(&self) -> RelocatableLayout<'static> {
        let inputs = self.inputs;
        let nsec = self.resolution.sections.len();
        let mut asg = Assignment::empty(nsec);
        // (name, type, flags) -> out; pieces per out.
        let mut keys: Vec<(&'static [u8], u32, u64)> = Vec::new();
        let mut pieces: Vec<Vec<SectionId>> = Vec::new();
        let mut eh: Option<usize> = None;
        for (f, _) in inputs.files.iter() {
            let Some(o) = obj(inputs, f) else { continue };
            for (s, i) in self.sections_of(f) {
                let row = self.resolution.sections[s];
                if !row.is_kept() || !row.kind.carries_output() {
                    continue;
                }
                let sec = o.section(i).unwrap();
                let name = sec.name().unwrap();
                if name == b".note.gnu.property" {
                    asg.out_of[s] = OutOf::Consumed;
                    continue;
                }
                let h = sec.header();
                let key = (name, h.sh_type, h.sh_flags);
                let o = if let Some(o) = keys.iter().position(|k| *k == key) {
                    o
                } else {
                    keys.push(key);
                    pieces.push(Vec::new());
                    keys.len() - 1
                };
                if row.kind == SectionKind::EhFrame {
                    eh = Some(o);
                }
                pieces[o].push(s);
                asg.out_of[s] = OutOf::Out(OutSecId(o as u32));
            }
        }
        for &(name, sh_type, flags) in &keys {
            asg.outsecs
                .push(OutSec {
                    name,
                    sh_type,
                    flags,
                    merge: None,
                    keep: false,
                    fill: None,
                    constraint: None,
                    script_position: None,
                })
                .unwrap();
        }
        let is64 = self.arch.facts().class == mortar_elf::Class::Elf64;
        let rela = self.arch.facts().rel_form == mortar_arch::RelForm::Rela;
        let mut chunks: Vec<Chunk> = Vec::new();
        let mut place = Column::filled(nsec, Place::NONE);
        let chunk = |kind, sh_type, flags, align, size| Chunk {
            kind,
            sh_type,
            flags,
            align,
            size,
            capacity: size,
            addr: None,
            offset: 0,
            shndx: None,
            relro: false,
        };
        let synth = |name: &'static [u8]| Synth { name, address_dependent: false };
        let hdr = |s: SectionId| {
            let (f, i) = self.resolution.sh_index(s).unwrap();
            *obj(inputs, f).unwrap().section(i).unwrap().header()
        };
        chunks.push(chunk(ChunkKind::FileHeader, 0, 0, 3, if is64 { 64 } else { 52 }));
        // The notes (S16: the join's sizes from mortar_synth).
        let (has_property, has_build_id) = self.notes();
        if has_build_id {
            chunks.push(chunk(
                ChunkKind::Synthetic(synth(b".note.gnu.build-id")),
                SHT_NOTE,
                SHF_ALLOC,
                2,
                0,
            ));
        }
        if has_property {
            chunks.push(chunk(
                ChunkKind::Synthetic(synth(b".note.gnu.property")),
                SHT_NOTE,
                SHF_ALLOC,
                if is64 { 3 } else { 2 },
                0,
            ));
        }
        let rel_size = |s: SectionId| -> u64 {
            let r = self.resolution.sections[s].reloc;
            if r.0 == 0 {
                return 0;
            }
            let (f, _) = self.resolution.sh_index(s).unwrap();
            let h = *obj(inputs, f).unwrap().section(r).unwrap().header();
            let n = match (is64, h.sh_type == SHT_RELA) {
                (true, true) => 24,
                (true, false) => 16,
                (false, true) => 12,
                (false, false) => 8,
            };
            h.sh_size / n
        };
        let entsize = if rela {
            if is64 { 24 } else { 12 }
        } else if is64 {
            16
        } else {
            8
        };
        for (o, list) in pieces.iter().enumerate() {
            let out = OutSecId(o as u32);
            let (_, sh_type, flags) = keys[o];
            let align = list.iter().map(|&s| log2(hdr(s).sh_addralign)).max().unwrap_or(0);
            let id = ChunkId(chunks.len() as u32);
            let size = if Some(o) == eh {
                chunks.push(chunk(
                    ChunkKind::Synthetic(synth(b".eh_frame")),
                    sh_type,
                    flags,
                    align,
                    0,
                ));
                list.iter().map(|&s| hdr(s).sh_size).sum()
            } else {
                chunks.push(chunk(ChunkKind::Body(Part::first(out)), sh_type, flags, align, 0));
                let mut at = 0;
                for &s in list {
                    let h = hdr(s);
                    at = align_up(at, h.sh_addralign);
                    place[s] = Place::at(id, at);
                    at += h.sh_size;
                }
                at
            };
            chunks[id.0 as usize].size = size;
            chunks[id.0 as usize].capacity = size;
            let records: u64 = list.iter().map(|&s| rel_size(s)).sum();
            if records > 0 {
                let n = records * entsize;
                chunks.push(chunk(
                    ChunkKind::Relocs(out),
                    if rela { SHT_RELA } else { SHT_REL },
                    0x40 | (flags & 0x200),
                    if is64 { 3 } else { 2 },
                    n,
                ));
            }
        }
        // L-R3: one SHT_GROUP per kept input group (no copy is dropped
        // here: one object per signature), its size by S8.
        for (f, _) in inputs.files.iter() {
            let Some(ob) = obj(inputs, f) else { continue };
            for (g, i) in self.sections_of(f) {
                if self.resolution.sections[g].kind != SectionKind::Group {
                    continue;
                }
                let data = ob.section(i).unwrap().data().unwrap();
                let mut n = 1u64;
                for w in data.chunks_exact(4).skip(1) {
                    let m = self
                        .resolution
                        .section_id(f, SectionIndex(u32::from_le_bytes(w.try_into().unwrap())))
                        .unwrap();
                    if self.resolution.sections[m].kind == SectionKind::Reloc {
                        continue;
                    }
                    let Some(o) = asg.out_of[m].out() else { continue };
                    n += 1;
                    if chunks.iter().any(|c| c.kind == ChunkKind::Relocs(o)) {
                        n += 1;
                    }
                }
                chunks.push(chunk(ChunkKind::Group(g), SHT_GROUP, 0, 2, 4 * n));
            }
        }
        chunks.push(chunk(
            ChunkKind::Synthetic(synth(b".symtab")),
            SHT_SYMTAB,
            0,
            if is64 { 3 } else { 2 },
            0,
        ));
        chunks.push(chunk(ChunkKind::Synthetic(synth(b".strtab")), SHT_STRTAB, 0, 0, 0));
        chunks.push(chunk(ChunkKind::Synthetic(synth(b".shstrtab")), SHT_STRTAB, 0, 0, 0));
        chunks.push(chunk(ChunkKind::SectionHeaders, 0, 0, if is64 { 3 } else { 2 }, 0));
        if self.probe(&chunks, &asg, &place).symtab_shndx {
            let at = chunks.len() - 3;
            chunks.insert(
                at,
                chunk(ChunkKind::Synthetic(synth(b".symtab_shndx")), SHT_SYMTAB_SHNDX, 0, 2, 0),
            );
        }
        // The image's sizes of the tables and notes, and the headers.
        let sz = self.probe(&chunks, &asg, &place);
        let shnum = chunks.len() as u64 - 1; // the null header in, the file header out
        let mut off = 0u64;
        let mut ix = 1u32;
        for (c, (_, size)) in chunks.iter_mut().zip(sz.chunks) {
            match c.kind {
                ChunkKind::Synthetic(s)
                    if [
                        &b".symtab"[..],
                        b".symtab_shndx",
                        b".strtab",
                        b".shstrtab",
                        b".note.gnu.build-id",
                        b".note.gnu.property",
                    ]
                    .contains(&s.name) =>
                {
                    c.size = size;
                }
                ChunkKind::SectionHeaders => c.size = shnum * if is64 { 64 } else { 40 },
                _ => {}
            }
            c.capacity = c.size;
            if !matches!(c.kind, ChunkKind::FileHeader | ChunkKind::SectionHeaders) {
                c.shndx = Some(ix);
                ix += 1;
            }
            c.offset = align_up(off, 1 << c.align);
            if c.sh_type != SHT_NOBITS {
                off = c.offset + c.size;
            }
        }
        let n = chunks.len() as u32;
        RelocatableLayout {
            assignment: asg,
            layout: Layout {
                placement: Placement {
                    kind: PlacementKind::Relocatable,
                    chunks: Column::from_vec(chunks).unwrap(),
                    order: (0..n).map(ChunkId).collect(),
                    place,
                    ..Placement::default()
                },
                ..Layout::default()
            },
            eh_removed: Box::new([]),
        }
    }

    /// Whether the property note and the build-id note exist (the
    /// image's plans; they read no layout).
    fn notes(&self) -> (bool, bool) {
        let pool = Pool::new(2);
        let layout = RelocatableLayout::default();
        let link = self.link_with(&pool, &layout);
        (
            mortar_image::relocatable::property_plan(&link).expect("property plan").is_some(),
            mortar_image::relocatable::build_id_plan(&link).expect("build-id plan").is_some(),
        )
    }

    /// `sizes` over provisional chunks (the header indexes assigned).
    fn probe(
        &self,
        chunks: &[Chunk],
        asg: &Assignment<'static>,
        place: &Column<SectionId, Place>,
    ) -> mortar_image::relocatable::Sizes {
        let mut chunks = chunks.to_vec();
        let mut ix = 1;
        for c in &mut chunks {
            if !matches!(c.kind, ChunkKind::FileHeader | ChunkKind::SectionHeaders) {
                c.shndx = Some(ix);
                ix += 1;
            }
        }
        let n = chunks.len() as u32;
        let layout = RelocatableLayout {
            assignment: asg.clone(),
            layout: Layout {
                placement: Placement {
                    kind: PlacementKind::Relocatable,
                    chunks: Column::from_vec(chunks).unwrap(),
                    order: (0..n).map(ChunkId).collect(),
                    place: place.clone(),
                    ..Placement::default()
                },
                ..Layout::default()
            },
            eh_removed: Box::new([]),
        };
        let pool = Pool::new(2);
        let link = self.link_with(&pool, &layout);
        sizes(&link).unwrap_or_else(|f| panic!("sizes: {:?}", f.diagnostics))
    }

    fn link_with<'a>(
        &'a self,
        pool: &'a Pool,
        layout: &'a RelocatableLayout<'static>,
    ) -> RelocatableLink<'a, 'static> {
        static RECHECK: fn(FileId) -> Result<(), mortar_core::inputs::CopyError> = |_| Ok(());
        RelocatableLink {
            arch: self.arch,
            pool,
            plan: &self.plan,
            inputs: self.inputs,
            resolution: &self.resolution,
            liveness: &self.liveness,
            split: &self.split,
            layout,
            recheck: &RECHECK,
            build_id: self.plan.shaping.build_id.as_ref().and_then(mortar_image::link::tree_style),
            args: &[],
        }
    }

    fn link<'a>(&'a self, pool: &'a Pool) -> RelocatableLink<'a, 'static> {
        self.link_with(pool, &self.layout)
    }
}

/// `Split`'s `.eh_frame` rows (S-48: CIEs and FDEs; a zero terminator is
/// no piece).
fn eh_split(inputs: &Inputs<'static>, res: &Resolution) -> Split {
    let mut eh_of = Vec::new();
    let mut eh_off = Vec::new();
    let mut eh_piece = Vec::new();
    for (s, row) in res.sections.iter() {
        if row.kind != SectionKind::EhFrame {
            continue;
        }
        let (f, i) = res.sh_index(s).unwrap();
        let sec = obj(inputs, f).unwrap().section(i).unwrap();
        let b = sec.data().unwrap();
        let start = eh_off.len() as u32;
        let mut cies: Vec<(u32, u32)> = Vec::new();
        let mut pos = 0usize;
        while pos + 4 <= b.len() {
            let len = u32::from_le_bytes(b[pos..pos + 4].try_into().unwrap()) as usize;
            if len == 0 {
                pos += 4;
                continue;
            }
            let id = u32::from_le_bytes(b[pos + 4..pos + 8].try_into().unwrap());
            let ordinal = eh_off.len() as u32 - start;
            if id == 0 {
                cies.push((pos as u32, ordinal));
                eh_piece.push(EhPiece::CIE);
            } else {
                let cie = (pos + 4 - id as usize) as u32;
                let c = cies.iter().find(|c| c.0 == cie).expect("the FDE's CIE").1;
                eh_piece.push(EhPiece { cie: c, code: OptId::NONE });
            }
            eh_off.push(pos as u32);
            pos += 4 + len;
        }
        eh_of.push(SplitRow {
            section: s,
            pieces: Span { start, len: eh_off.len() as u32 - start },
        });
    }
    Split {
        eh_of: eh_of.into_boxed_slice(),
        eh_off: eh_off.into_boxed_slice(),
        eh_piece: eh_piece.into_boxed_slice(),
        ..Split::default()
    }
}

// ---- tools -------------------------------------------------------------------

/// `tool` for `arch`, or a named skip (an assertion when the gate requires
/// the tools).
fn need(arch: TArch, tool: &str, test: &str) -> Option<PathBuf> {
    let t = arch.tool(tool);
    if t.is_none() {
        assert!(!tools_required(), "{test}: {} is required", arch.gnu(tool));
        named_skip(test, &format!("{} is not installed", arch.gnu(tool)));
    }
    t
}

fn sh(dir: &Path, program: &Path, args: &[&str]) -> mortar_testutil::Outcome {
    run_with(Command::new(program).args(args).current_dir(dir), b"", Duration::from_mins(3))
}

fn ok(dir: &Path, program: &Path, args: &[&str]) -> String {
    let o = sh(dir, program, args);
    assert!(o.success(), "{} {args:?}: {}{}", program.display(), o.stdout_text(), o.stderr_text());
    o.stdout_text()
}

fn copy_corpus(dir: &Scratch) {
    for f in ["c1.c", "c2.c", "mainc.c", "rec.h"] {
        std::fs::copy(Path::new(CORPUS).join(f), dir.join(f)).unwrap();
    }
}

fn opts() -> OutputOptions {
    OutputOptions { mode: 0o666, ..OutputOptions::executable(Vec::new()) }
}

/// Writes the `-r` output of `w` to `dir/name` with `threads` threads.
fn write_r(w: &World, dir: &Path, name: &str, threads: usize) -> Vec<u8> {
    let pool = Pool::new(threads);
    let link = w.link(&pool);
    let path = dir.join(name);
    if let Err(f) = write(&link, &path, &opts()) {
        panic!("the -r write failed: {:#?}", f.diagnostics);
    }
    std::fs::read(&path).unwrap()
}

/// `readelf -a -W` with no complaint.
fn readelf_clean(dir: &Path, name: &str) -> String {
    let readelf = mortar_testutil::tool("readelf").expect("readelf");
    let o = sh(dir, &readelf, &["-a", "-W", name]);
    assert!(o.success(), "readelf: {}", o.stderr_text());
    assert!(o.stderr_text().trim().is_empty(), "readelf complains: {}", o.stderr_text());
    o.stdout_text()
}

// ---- the corpus, AArch64 -----------------------------------------------------------

fn corpus_world(dir: &Scratch, cc: &Path, extra: &[&str]) -> World {
    copy_corpus(dir);
    for c in ["c1", "c2", "mainc"] {
        ok(dir.path(), cc, &["-O1", "-fcommon", "-c", &format!("{c}.c"), "-o", &format!("{c}.o")]);
    }
    let mut words = vec!["-maarch64linux", "c1.o", "c2.o"];
    words.extend_from_slice(extra);
    World::new(dir.path(), &words)
}

#[test]
fn r_corpus_reads_back_and_links_with_gnu_ld_and_lld() {
    const T: &str = "relocatable::r_corpus_reads_back_and_links_with_gnu_ld_and_lld";
    let Some(cc) = need(TArch::Aarch64, "gcc", T) else { return };
    let dir = Scratch::new("rl4-corpus");
    let w = corpus_world(&dir, &cc, &[]);
    let bytes = write_r(&w, dir.path(), "r.o", 4);
    let text = readelf_clean(dir.path(), "r.o");
    // mortar_elf reads it back.
    let o = Object::parse(&bytes).expect("mortar_elf parses the -r output");
    assert_eq!(o.header().e_type, ET_REL);
    assert_eq!(o.header().e_phnum, 0);
    assert_eq!(o.header().e_entry, 0);
    let symtab_ix = o.sections().find(|s| s.sh_type() == SHT_SYMTAB).unwrap().index().0;
    for s in o.sections().filter(|s| s.sh_type() == SHT_RELA) {
        let h = s.header();
        assert_eq!(h.sh_link, symtab_ix, "{:?}", s.name());
        assert_eq!(h.sh_entsize, 24);
        let target = o.section(SectionIndex(h.sh_info)).unwrap();
        let mut want = b".rela".to_vec();
        want.extend_from_slice(target.name().unwrap());
        assert_eq!(s.name().unwrap(), &want[..]);
        // Every record names a symbol of the table.
        for r in s.rela().unwrap() {
            assert!(r.r_offset < target.size().max(1), "{r:?}");
        }
    }
    let t = o.symbols().unwrap().unwrap();
    let mut seen = std::collections::BTreeMap::new();
    for (i, s) in t.globals() {
        seen.insert(t.name(&s).unwrap().to_vec(), (i, s, t.section_of(i, &s).unwrap()));
    }
    for (_, s) in t.locals().skip(1) {
        assert!(s.binding() == 0, "a global among the locals");
    }
    let get = |n: &str| seen.get(n.as_bytes()).unwrap_or_else(|| panic!("{n} missing:\n{text}"));
    assert_eq!(get("common_x").2, SymbolSection::Common);
    assert_eq!(get("common_x").1.st_size, 8, "RV-R5: the larger common");
    assert_eq!(get("undef_weak").2, SymbolSection::Undefined);
    assert_eq!(get("undef_weak").1.binding(), STB_WEAK);
    assert_eq!(get("rec").2, SymbolSection::Undefined);
    assert_eq!(get("wk_only").1.binding(), STB_WEAK);
    assert_eq!(get("wk").1.binding(), STB_GLOBAL, "c2's strong wk");
    assert_eq!(get("hid").1.st_other & 3, 2, "RV-R6: hidden stays hidden");
    assert!(matches!(get("g_data").2, SymbolSection::Section(_)));
    // The final links run and print what the direct link prints.
    let host = TArch::Aarch64.is_host();
    ok(dir.path(), &cc, &["mainc.o", "c1.o", "c2.o", "-o", "direct"]);
    let mut outs = Vec::new();
    for ld in ["bfd", "lld"] {
        let exe = format!("via-{ld}");
        let o = sh(dir.path(), &cc, &[&format!("-fuse-ld={ld}"), "mainc.o", "r.o", "-o", &exe]);
        if !o.success() && ld == "lld" && mortar_testutil::tool("ld.lld").is_none() {
            assert!(!tools_required(), "ld.lld is required");
            named_skip(&format!("{T} (lld)"), "ld.lld is not installed");
            continue;
        }
        assert!(o.success(), "{ld}: {}", o.stderr_text());
        outs.push(exe);
    }
    if !host {
        named_skip(&format!("{T} (run)"), "the AArch64 programs run on an AArch64 host only");
        return;
    }
    let want = ok(dir.path(), &dir.join("direct"), &[]);
    assert!(want.contains("g_data=5 wk=2 wk_only=11"), "{want}");
    for exe in outs {
        assert_eq!(ok(dir.path(), &dir.join(&exe), &[]), want, "{exe}");
    }
}

#[test]
fn r_output_is_deterministic_across_threads() {
    const T: &str = "relocatable::r_output_is_deterministic_across_threads";
    let Some(cc) = need(TArch::Aarch64, "gcc", T) else { return };
    let dir = Scratch::new("rl4-det");
    let w = corpus_world(&dir, &cc, &["--build-id"]);
    let a = write_r(&w, dir.path(), "a.o", 1);
    let b = write_r(&w, dir.path(), "b.o", 8);
    let c = write_r(&w, dir.path(), "c.o", 8);
    assert!(a == b && b == c, "I-R11: -j1 and -j8 differ");
    let pool1 = Pool::new(1);
    let pool8 = Pool::new(8);
    let m1 = map_of(&w.link(&pool1)).unwrap();
    let m8 = map_of(&w.link(&pool8)).unwrap();
    assert_eq!(format!("{m1:?}"), format!("{m8:?}"));
}

#[test]
fn r_build_id_note_follows_the_option() {
    const T: &str = "relocatable::r_build_id_note_follows_the_option";
    let Some(cc) = need(TArch::Aarch64, "gcc", T) else { return };
    let dir = Scratch::new("rl4-buildid");
    let note = |bytes: &[u8]| -> Option<Vec<u8>> {
        let o = Object::parse(bytes).unwrap();
        let s = o.sections().find(|s| s.name().unwrap() == b".note.gnu.build-id")?;
        Some(s.data().unwrap().to_vec())
    };
    let plain = corpus_world(&dir, &cc, &[]);
    assert!(
        note(&write_r(&plain, dir.path(), "none.o", 2)).is_none(),
        "I-R8: no note without --build-id"
    );
    let sha = World::new(dir.path(), &["-maarch64linux", "c1.o", "c2.o", "--build-id=sha1"]);
    let n = note(&write_r(&sha, dir.path(), "sha.o", 2)).expect("a note with --build-id=sha1");
    assert_eq!(n.len(), 36);
    assert_eq!(&n[12..16], b"GNU\0");
    assert!(n[16..].iter().any(|&b| b != 0), "the id is hashed");
    let hex = World::new(dir.path(), &["-maarch64linux", "c1.o", "c2.o", "--build-id=0xdeadbeef"]);
    let n = note(&write_r(&hex, dir.path(), "hex.o", 2)).expect("a note with 0xHEX");
    assert_eq!(&n[16..20], &[0xde, 0xad, 0xbe, 0xef]);
    let fast = World::new(dir.path(), &["-maarch64linux", "c1.o", "c2.o", "--build-id"]);
    let a = write_r(&fast, dir.path(), "fast1.o", 1);
    let b = write_r(&fast, dir.path(), "fast2.o", 8);
    assert_eq!(a, b);
    readelf_clean(dir.path(), "fast1.o");
    // The final link drops the input's note and links.
    ok(dir.path(), &cc, &["mainc.o", "fast1.o", "-o", "p"]);
}

#[test]
fn r_size_mismatch_is_an_internal_error_naming_the_chunk() {
    const T: &str = "relocatable::r_size_mismatch_is_an_internal_error_naming_the_chunk";
    let Some(cc) = need(TArch::Aarch64, "gcc", T) else { return };
    let dir = Scratch::new("rl4-size");
    let mut w = corpus_world(&dir, &cc, &[]);
    let pl = &mut w.layout.layout.placement;
    let (id, c) = pl
        .chunks
        .iter()
        .find(|(_, c)| matches!(c.kind, ChunkKind::Body(_)))
        .map(|(i, c)| (i, *c))
        .unwrap();
    pl.chunks[id].size = c.size + 8;
    let pool = Pool::new(2);
    let link = w.link(&pool);
    let name = String::from_utf8_lossy(w.layout.assignment.outsecs[c.kind.out().unwrap()].name)
        .into_owned();
    let err = write(&link, &dir.join("bad.o"), &opts()).expect_err("a size mismatch is refused");
    let text = format!("{:?}", err.diagnostics);
    assert!(text.contains("InternalError"), "{text}");
    assert!(text.contains(&name), "names the chunk: {text}");
    assert!(
        text.contains(&format!("{:#x}", c.size + 8)) && text.contains(&format!("{:#x}", c.size)),
        "{text}"
    );
    assert!(!dir.join("bad.o").exists(), "no output");
}

#[test]
fn r_removed_eh_records_are_refused_by_name() {
    const T: &str = "relocatable::r_removed_eh_records_are_refused_by_name";
    let Some(cc) = need(TArch::Aarch64, "gcc", T) else { return };
    let dir = Scratch::new("rl4-ehrm");
    let mut w = corpus_world(&dir, &cc, &[]);
    let s = w.split.eh_of[0].section;
    let off = u64::from(w.split.eh_off[w.split.eh_of[0].pieces.start as usize + 1]);
    w.layout.eh_removed = Box::new([(s, off)]);
    let pool = Pool::new(2);
    let err = write(&w.link(&pool), &dir.join("x.o"), &opts()).expect_err("refused");
    let text = format!("{:?}", err.diagnostics);
    assert!(text.contains("removed .eh_frame records at -r") && text.contains("RL-8"), "{text}");
}

/// A-R4: an input relocation section of the other type is refused by name.
#[test]
fn r_other_relocation_type_is_refused_by_name() {
    const T: &str = "relocatable::r_other_relocation_type_is_refused_by_name";
    let Some(cc) = need(TArch::Aarch64, "gcc", T) else { return };
    let dir = Scratch::new("rl4-ar4");
    copy_corpus(&dir);
    ok(dir.path(), &cc, &["-O1", "-c", "c2.c", "-o", "c2.o"]);
    // Make `.rela.text` an `SHT_REL` (records of 16 bytes on ELF64).
    let mut b = std::fs::read(dir.join("c2.o")).unwrap();
    let o = Object::parse(&b).unwrap();
    let shoff = o.header().e_shoff as usize;
    let ix = o.sections().find(|s| s.name().unwrap() == b".rela.text").unwrap().index().0 as usize;
    let at = shoff + ix * 64 + 4;
    b[at..at + 4].copy_from_slice(&SHT_REL.to_le_bytes());
    std::fs::write(dir.join("c2.o"), &b).unwrap();
    let w = World::new(dir.path(), &["-maarch64linux", "c2.o"]);
    let pool = Pool::new(2);
    let err = write(&w.link(&pool), &dir.join("x.o"), &opts()).expect_err("refused");
    let text = format!("{:?}", err.diagnostics);
    assert!(text.contains("SHT_REL") && text.contains("SectionType"), "{text}");
}

/// I-R5 on REL (i386): the new addend of a section-symbol record goes
/// through RL-9's `relrel::write`, whose refusal reaches the user by name.
#[test]
fn r_i386_rel_addend_reaches_relrel() {
    const T: &str = "relocatable::r_i386_rel_addend_reaches_relrel";
    let Some(cc) = need(TArch::I386, "gcc", T) else { return };
    let dir = Scratch::new("rl4-rel");
    copy_corpus(&dir);
    for c in ["c1", "c2"] {
        ok(dir.path(), &cc, &["-O1", "-fcommon", "-c", &format!("{c}.c"), "-o", &format!("{c}.o")]);
    }
    let w = World::new(dir.path(), &["-melf_i386", "c1.o", "c2.o"]);
    let pool = Pool::new(2);
    let err = write(&w.link(&pool), &dir.join("x.o"), &opts()).expect_err("RL-9 refuses");
    let text = format!("{:?}", err.diagnostics);
    assert!(text.contains("A-R2") && text.contains("RL-9"), "{text}");
    assert!(text.contains("c2.o"), "names the file: {text}");
}

/// The x86-64 corpus: GCC writes `.note.gnu.property` into every object
/// (S10), and the records are RELA.
#[test]
fn r_x86_64_corpus_links_with_gnu_ld() {
    const T: &str = "relocatable::r_x86_64_corpus_links_with_gnu_ld";
    let Some(cc) = need(TArch::X86_64, "gcc", T) else { return };
    let dir = Scratch::new("rl4-x64");
    copy_corpus(&dir);
    for c in ["c1", "c2", "mainc"] {
        ok(
            dir.path(),
            &cc,
            &[
                "-O1",
                "-fcommon",
                "-fcf-protection",
                "-c",
                &format!("{c}.c"),
                "-o",
                &format!("{c}.o"),
            ],
        );
    }
    let w = World::new(dir.path(), &["-melf_x86_64", "c1.o", "c2.o"]);
    let bytes = write_r(&w, dir.path(), "r.o", 4);
    readelf_clean(dir.path(), "r.o");
    let o = Object::parse(&bytes).unwrap();
    assert!(
        o.sections().any(|s| s.name().unwrap() == b".note.gnu.property"),
        "S10: the merged note"
    );
    ok(dir.path(), &cc, &["mainc.o", "r.o", "-o", "p"]);
    if TArch::X86_64.is_host() {
        ok(dir.path(), &cc, &["mainc.o", "c1.o", "c2.o", "-o", "d"]);
        assert_eq!(ok(dir.path(), &dir.join("p"), &[]), ok(dir.path(), &dir.join("d"), &[]));
    }
}

/// I-R4's `.symtab_shndx`: 65,300 output sections, symbols past
/// `SHN_LORESERVE`; the output reads back and GNU ld and lld link it.
#[test]
#[allow(clippy::many_single_char_names)]
fn r_xindex_past_shn_loreserve() {
    const T: &str = "relocatable::r_xindex_past_shn_loreserve";
    let Some(cc) = need(TArch::Aarch64, "gcc", T) else { return };
    let dir = Scratch::new("rl4-xindex");
    let mut s = String::new();
    for i in 0..65_300 {
        use std::fmt::Write as _;
        writeln!(s, ".section .data.s{i},\"aw\"\ns{i}: .quad s{i}").unwrap();
    }
    s.push_str(".globl high\nhigh: .quad 0\n");
    std::fs::write(dir.join("big.s"), s).unwrap();
    ok(dir.path(), &cc, &["-c", "big.s", "-o", "big.o"]);
    let w = World::new(dir.path(), &["-maarch64linux", "big.o"]);
    assert!(
        w.layout
            .layout
            .placement
            .chunks
            .iter()
            .any(|(_, c)| matches!(c.kind, ChunkKind::Synthetic(s) if s.name == b".symtab_shndx"))
    );
    let bytes = write_r(&w, dir.path(), "r.o", 8);
    let o = Object::parse(&bytes).unwrap();
    assert_eq!(o.header().e_shnum, 0, "the gABI escape");
    let t = o.symbols().unwrap().unwrap();
    let high = t.globals().find(|(_, s)| t.name(s).unwrap() == b"high").unwrap();
    match t.section_of(high.0, &high.1).unwrap() {
        SymbolSection::Section(i) => {
            assert!(i.0 >= 0xff00, "{i:?}");
            assert_eq!(o.section(i).unwrap().name().unwrap(), b".data.s65299");
        }
        other => panic!("{other:?}"),
    }
    let readelf = mortar_testutil::tool("readelf").expect("readelf");
    let r = sh(dir.path(), &readelf, &["-h", "-S", "-s", "-W", "r.o"]);
    assert!(r.success() && r.stderr_text().trim().is_empty(), "{}", r.stderr_text());
    for ld in ["ld.bfd", "ld.lld"] {
        let Some(l) = mortar_testutil::tool(ld) else {
            assert!(!tools_required(), "{ld} is required");
            named_skip(&format!("{T} ({ld})"), &format!("{ld} is not installed"));
            continue;
        };
        let mut args = vec!["-e", "high", "r.o", "-o", "x"];
        if ld == "ld.bfd" {
            args.insert(0, "-maarch64linux");
        }
        let o = sh(dir.path(), &l, &args);
        assert!(o.success(), "{ld}: {}", o.stderr_text());
    }
}

/// A-R1/A-R5: at `-r` no relocation is a branch site (no veneer, no
/// range check).
#[test]
fn branch_candidate_is_none_at_r() {
    use mortar_arch::need::branch_candidate;
    use mortar_arch::{OutputKind, Reloc, RelocAt, Site, SymNames};
    use mortar_core::SymKey;
    struct Names;
    impl SymNames for Names {
        fn name(&self, _: SymKey) -> &[u8] {
            b"f"
        }
    }
    let code = [0u8; 4];
    let howto = A64::M1.classify(283, Site { bytes: &code, offset: 0 }).unwrap();
    let records: &[Reloc] = &[];
    let at = RelocAt {
        file: FileId(1),
        section: SectionId(1),
        index: 0,
        r_offset: 0,
        key: SymKey::global(SymbolId(0)),
        r_type: 283,
        addend: Some(0),
        howto: &howto,
        site: Site { bytes: &code, offset: 0 },
        records: &records,
        proof: &[],
        leads: true,
    };
    assert!(branch_candidate(&A64::M1, OutputKind::Exec, &at, &Names, false).is_some());
    assert!(branch_candidate(&A64::M1, OutputKind::Relocatable, &at, &Names, false).is_none());
}

/// The map (I-R12): every placed section an `rplace`, every `.eh_frame`
/// record an `rframe`, the files, `output relocatable`.
#[test]
fn r_map_records() {
    const T: &str = "relocatable::r_map_records";
    let Some(cc) = need(TArch::Aarch64, "gcc", T) else { return };
    let dir = Scratch::new("rl4-map");
    let w = corpus_world(&dir, &cc, &[]);
    let pool = Pool::new(2);
    let m = map_of(&w.link(&pool)).unwrap();
    assert_eq!(m.files.len(), 2);
    let r = m.relocatable.as_ref().unwrap();
    let placed = w.layout.layout.placement.place.iter().filter(|(_, p)| p.get().is_some()).count();
    assert_eq!(r.rplace.iter().map(Vec::len).sum::<usize>(), placed);
    let pieces = w.split.eh_off.len();
    let frames = r.rframe.iter().map(Vec::len).sum::<usize>();
    assert!(frames >= pieces, "{frames} rframe records, {pieces} pieces");
    assert!(r.rframe.iter().flatten().all(|f| f.output_offset.is_some()));
    assert_eq!(m.dynamic.as_ref().unwrap().kind, Some(mortar_image::map::OutputKind::Relocatable));
}

/// L-R3 / S8: a COMDAT group's output `SHT_GROUP` (its members and their
/// relocation sections), linked finally by GNU ld and lld.
#[test]
fn r_comdat_group_links() {
    const T: &str = "relocatable::r_comdat_group_links";
    let Some(cc) = need(TArch::Aarch64, "gcc", T) else { return };
    let dir = Scratch::new("rl4-group");
    std::fs::write(
        dir.join("g.s"),
        concat!(
            ".section .text.gsig,\"axG\",@progbits,gsig,comdat\n",
            ".globl gsig\n.type gsig,%function\ngsig: mov w0, #42\nret\n",
            ".section .data.gsig,\"awG\",@progbits,gsig,comdat\n",
            ".globl gdat\n.p2align 3\ngdat: .quad gsig\n",
            ".text\n.globl use_g\n.type use_g,%function\nuse_g: b gsig\n",
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("m.c"),
        "int gsig(void); int use_g(void); extern int (*gdat)(void);\n\
         int main(void) { return gsig() == 42 && use_g() == 42 && gdat == gsig ? 0 : 1; }\n",
    )
    .unwrap();
    ok(dir.path(), &cc, &["-c", "g.s", "-o", "g.o"]);
    ok(dir.path(), &cc, &["-O1", "-c", "m.c", "-o", "m.o"]);
    let w = World::new(dir.path(), &["-maarch64linux", "g.o"]);
    let bytes = write_r(&w, dir.path(), "r.o", 4);
    let text = readelf_clean(dir.path(), "r.o");
    assert!(
        text.contains("COMDAT group section [") && text.contains("[gsig] contains 3 sections"),
        "{text}"
    );
    let o = Object::parse(&bytes).unwrap();
    let g = o.sections().find(|s| s.sh_type() == SHT_GROUP).unwrap();
    let t = o.symbols().unwrap().unwrap();
    let sig = t.get(mortar_elf::SymbolIndex(g.header().sh_info)).unwrap();
    assert_eq!(t.name(&sig).unwrap(), b"gsig");
    for ld in ["bfd", "lld"] {
        let exe = format!("p-{ld}");
        ok(dir.path(), &cc, &[&format!("-fuse-ld={ld}"), "m.o", "r.o", "-o", &exe]);
        if TArch::Aarch64.is_host() {
            ok(dir.path(), &dir.join(&exe), &[]);
        }
    }
}

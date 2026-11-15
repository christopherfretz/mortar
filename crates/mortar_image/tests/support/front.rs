//! A test-only front: the phases of WP-027a's `crates/mortar/src/front.rs`
//! on main's crates (input, resolve, assignment, split, liveness, scan,
//! the synthetic plans, `place`), without range extension (no thunks, no
//! erratum 843419 patches), so that `mortar_image::link::write` runs on
//! real inputs before the driver joins them (WP-027c).
//!
//! The adapters the driver owns (`CoreSyms`, `LinkSections`, the proof of
//! mapping symbols, the scan's facts per key) are restated here in their
//! simplest form for a static link.

use std::ffi::OsString;
use std::path::Path;

use mortar_a64::A64;
use mortar_a64::mapping::{MapSym, Mapping, mapping_kind, proof_runs};
use mortar_arch::{Arch, OutputCx, OutputKind, ProofRun, SymFacts, SymNames, TargetKind};
use mortar_core::assignment::Assignment;
use mortar_core::column::Column;
use mortar_core::demands::DemandFlags;
use mortar_core::inputs::{CopyError, Errno, Inputs, Reader};
use mortar_core::layout::{ChunkKind, Folds, Layout, Slots};
use mortar_core::par::Pool;
use mortar_core::plan::{self, Plan, Strip};
use mortar_core::resolution::{Def, InternalDef, RefFlags, Resolution, SectionKind};
use mortar_core::{ChunkId, FileId, Liveness, SectionId, SymEntry, SymKey, SymKind, SymbolId};
use mortar_diag::{Collector, Policy};
use mortar_elf::consts::{
    SHF_EXECINSTR, SHT_NOTE, STB_GLOBAL, STB_WEAK, STT_GNU_IFUNC, STT_NOTYPE, STT_OBJECT, STT_TLS,
    STV_HIDDEN, STV_INTERNAL,
};
use mortar_elf::{Class, Sym, SymbolIndex, SymbolSection, SymbolTable};
use mortar_image::fill::Failed;
use mortar_image::link::{self, KeyFacts, LinkOptions, Written};
use mortar_image::output::OutputOptions;
use mortar_input::load::{LoadEnv, load};
use mortar_input::store::{ReadError, Store, StorePolicy};
use mortar_layout::chunk::{Common, SynthChunk, commons};
use mortar_layout::{Headers, Options, Placed, Target};
use mortar_live::{LiveInputs, LiveOptions, liveness};
use mortar_merge::order::{Order, offsets};
use mortar_merge::phase::{ArchRelocs, Phase, Tables};
use mortar_scan::{CodeProof, Mark, ScanInputs, ScanOptions, Scanned, scan};
use mortar_synth::SymSource;
use mortar_synth::notes::ObjectNotes;
use mortar_synth::symtab::{
    CoreSections, GlobalIn, LocalIn, PatchFacts, SecKind, Sections, SymCtx, SymDef,
};
use mortar_synth::{
    BuildIdPlan, EhPlan, GotPlan, HdrPlan, MergeEh, PropertyPlan, ShstrtabPlan, SymtabPlan,
    SynthKind, chunk_ids,
};

pub const FIRST_SYNTHETIC: ChunkId = ChunkId(3);

/// A link up to its `Layout` (WP-027a's `Front`, its fields by value).
pub struct Front {
    pub plan: Plan,
    pub pool: Pool,
    pub arch: A64,
    pub store: &'static Store,
    /// Where the command line's paths are relative to.
    pub cwd: std::path::PathBuf,
    pub inputs: Inputs<'static>,
    pub resolution: Resolution,
    pub headers: Headers<'static>,
    pub assignment: Assignment<'static>,
    pub merge: Phase,
    pub order: Order,
    pub liveness: Liveness,
    pub scanned: Scanned,
    pub eh: EhPlan,
    pub hdr: Option<HdrPlan>,
    pub got: GotPlan,
    pub symtab: Option<SymtabPlan>,
    pub shstrtab: ShstrtabPlan,
    pub build_id: Option<BuildIdPlan>,
    pub property: PropertyPlan,
    pub chunks: Vec<(SynthKind, ChunkId)>,
    pub commons: Vec<Common>,
    pub layout: Layout,
    pub placed: Placed,
}

/// AArch64's mapping symbols, for the scan.
#[derive(Debug, Clone, Copy, Default)]
pub struct A64Proof;

impl CodeProof for A64Proof {
    fn mark(&self, name: &[u8], st_info: u8) -> Option<bool> {
        mapping_kind(name, st_info).map(|m| m == Mapping::Code)
    }

    fn runs(&self, marks: &mut [Mark], len: u64, exec: bool, out: &mut Vec<ProofRun>) {
        let mut syms: Vec<MapSym> = marks
            .iter()
            .map(|m| MapSym {
                index: m.index,
                value: m.value,
                kind: if m.code { Mapping::Code } else { Mapping::Data },
            })
            .collect();
        proof_runs(&mut syms, len, exec, 0, 0, out);
    }
}

fn output_kind(kind: plan::OutputKind) -> OutputKind {
    match kind {
        plan::OutputKind::StaticExecutable => OutputKind::Static,
        plan::OutputKind::Executable => OutputKind::Exec,
        plan::OutputKind::PieExecutable | plan::OutputKind::StaticPie => OutputKind::Pie,
        plan::OutputKind::Shared => OutputKind::Shared,
        plan::OutputKind::Relocatable => OutputKind::Relocatable,
    }
}

/// Runs the front on a command line (`args`, relative to `cwd`), panicking
/// with the rendered diagnostics when a phase fails.
pub fn front(args: &[&str], cwd: &Path, threads: usize, policy: StorePolicy) -> Front {
    let diag = Collector::new(Policy::default());
    match front_in(args, cwd, threads, policy, &diag) {
        Some(f) => f,
        None => panic!("{}", diag.finish().render(&mortar_diag::Style::plain("mortar"))),
    }
}

#[allow(clippy::too_many_lines)]
fn front_in(
    args: &[&str],
    cwd: &Path,
    threads: usize,
    policy: StorePolicy,
    diag: &Collector,
) -> Option<Front> {
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    let plan = mortar_cli::parse(&args).plan;
    let pool = Pool::new(threads);
    let store: &'static Store = Box::leak(Box::new(Store::new(policy)));
    let env = LoadEnv { cwd: cwd.to_path_buf(), pool };
    let inputs = load(&plan, &env, store, diag)?;
    let inputs: Inputs<'static> = inputs;
    let outcome = mortar_resolve::resolve_inputs(&inputs, &plan, &pool);
    for (k, d) in outcome.diagnostics.iter().cloned() {
        diag.emit(k, d);
    }
    if diag.has_errors() {
        return None;
    }
    let comdat: Vec<mortar_scan::Mismatch> = outcome
        .comdat_mismatch
        .iter()
        .map(|m| mortar_scan::Mismatch {
            key: m.key,
            group: m.group,
            keeper: m.keeper,
            dropped: m.dropped,
        })
        .collect();
    let gnu_warnings = outcome.gnu_warning_syms.clone();
    let resolution = outcome.resolution;
    let target = Target::of(inputs.target.machine);
    let options = Options::from_plan(&plan, &target).expect("layout options");
    let arch = A64::new(mortar_arch::Status::M1, options.fix_843419);
    let headers =
        mortar_layout::headers::gather(&inputs, &resolution, &pool).expect("section headers");
    let assignment = mortar_layout::assign::assign(
        &headers,
        &resolution,
        &inputs.symbols,
        &target,
        &options,
        &pool,
    )
    .unwrap_or_else(|_| panic!("assignment"))
    .assignment;
    let tables = Tables { inputs: &inputs, resolution: &resolution, assignment: &assignment };
    let merge = mortar_merge::phase::run(&pool, &tables, &ArchRelocs(&arch))
        .unwrap_or_else(|p| panic!("split: {:?}", mortar_merge::phase::diagnostics(&inputs, &p)));
    let live = LiveInputs {
        inputs: &inputs,
        resolution: &resolution,
        assignment: &assignment,
        split: &merge.split,
    };
    let liveness = liveness(&live, LiveOptions { gc_sections: false }, &pool)
        .unwrap_or_else(|(_, d)| panic!("liveness: {d:?}"));
    let order = offsets(&pool, &merge.split, &merge.meta, &liveness.frag_live);
    let output = output_kind(plan.kind);
    let scan_inputs = ScanInputs {
        inputs: &inputs,
        resolution: &resolution,
        liveness: &liveness,
        assignment: Some(&assignment),
        comdat_mismatch: &comdat,
        gnu_warnings: &gnu_warnings,
    };
    let scan_options = ScanOptions { output, dynamic: false, proof: &A64Proof };
    let scanned = scan(&arch, &scan_inputs, &scan_options, &pool);
    for (k, d) in scanned.diagnostics.iter().cloned() {
        diag.emit(k, d);
    }
    if diag.has_errors() {
        return None;
    }

    // 80.
    let base_slots = slots_of(&scanned.demands, &resolution);
    let merge_eh = MergeEh::new(&merge.eh, &tables).expect("eh source");
    let eh = EhPlan::new(&merge_eh, |s: SectionId| liveness.is_live(s), &pool, diag).ok()?;
    let hdr = HdrPlan::new(&merge_eh, &eh, plan.shaping.eh_frame_hdr.unwrap_or(false), &pool, diag)
        .ok()?;
    let property = property_plan(&inputs, &resolution, &headers, arch, diag)?;
    let feature_1_and = property.value_u32(mortar_a64::FEATURE_1_AND);
    let variant = arch
        .plt_variant(&OutputCx { output, feature_1_and, pac_plt: false, force_bti: false })
        .unwrap_or_else(|_| panic!("plt variant"));
    let got = GotPlan::new(&arch, &base_slots, scanned.demands.output, variant, diag).ok()?;
    let build_id = BuildIdPlan::new(plan.shaping.build_id.as_ref(), diag).ok()?;
    let symtab = {
        let core =
            CoreSections { resolution: &resolution, liveness: &liveness, split: &merge.split };
        let sections = LinkSections { core, assignment: &assignment };
        let syms = CoreSyms::new(&inputs, &resolution, &assignment);
        let cx = SymCtx {
            syms: &syms,
            sections: &sections,
            eh: Some((&eh, &merge_eh)),
            discard: plan.shaping.discard,
        };
        if plan.shaping.strip == Some(Strip::All) {
            None
        } else {
            Some(SymtabPlan::new(&cx, Class::Elf64, &[], PatchFacts::of(&arch), &pool, diag).ok()?)
        }
    };

    // 90.
    let folds = Folds::default();
    let commons = commons(&resolution, Some(&inputs));
    let stack_notes = mortar_layout::stack::notes(&inputs, &resolution);
    let mut names: Vec<&[u8]> = estimated_names(&assignment, &liveness, &resolution);
    let mut shndx = SymtabPlan::needs_shndx(names.len() as u32 + 2);
    let mut attempt = 0;
    let (assignment_after, placed, layout, chunks, shstrtab) = loop {
        attempt += 1;
        let shstrtab = ShstrtabPlan::new(&names);
        let (synthetic, present) = synth_chunks(
            &property,
            build_id.as_ref(),
            &got,
            hdr.as_ref(),
            &eh,
            symtab.as_ref(),
            &shstrtab,
            shndx,
        );
        let sizes = chunk_ids(FIRST_SYNTHETIC, &present).expect("chunk ids");
        let chunks: Vec<(SynthKind, ChunkId)> =
            present.iter().map(|p| p.0).zip(sizes.iter().map(|s| s.0)).collect();
        let mut slots = base_slots.clone();
        slots.sizes = sizes;
        slots.eh_records = eh.records().into();
        if let Some(s) = &symtab {
            slots.symtab_locals = s.symtab_locals().clone();
        }
        let link = mortar_layout::Link {
            headers: &headers,
            resolution: &resolution,
            symbols: &inputs.symbols,
            liveness: &liveness,
            folds: &folds,
            split: &merge.split,
            merge: &order.map,
            commons: &commons,
            synthetic: &synthetic,
            stack_notes: &stack_notes,
            target: &target,
            options: &options,
        };
        let mut a = assignment.clone();
        let placed = mortar_layout::place(&link, &mut a, None, |e| inputs.entry_def(e), &pool)
            .unwrap_or_else(|e| {
                let d: Vec<String> =
                    e.into_iter().map(|d| format!("{:?}", d.diagnostic())).collect();
                panic!("layout: {d:?}")
            });
        let exact = placed_names(&placed, &a);
        let exact_shndx = SymtabPlan::needs_shndx(exact.len() as u32 + 1);
        let again = ShstrtabPlan::new(&exact).size() != shstrtab.size() || exact_shndx != shndx;
        if again {
            assert!(attempt < 3, "the non-allocated sizes do not settle");
            names = exact;
            shndx = exact_shndx;
            continue;
        }
        let layout = Layout {
            folds: folds.clone(),
            merge: order.map.clone(),
            slots,
            placement: placed.placement.clone(),
        };
        break (a, placed, layout, chunks, shstrtab);
    };
    for &(kind, id) in &chunks {
        assert!(
            matches!(placed.placement.chunks.get(id).map(|c| c.kind),
                Some(ChunkKind::Synthetic(s)) if s.name == kind.name(false)),
            "chunk {id:?} is not {kind:?}"
        );
    }
    drop(merge_eh);
    Some(Front {
        plan,
        pool,
        arch,
        store,
        cwd: cwd.to_path_buf(),
        inputs,
        resolution,
        headers,
        assignment: assignment_after,
        merge,
        order,
        liveness,
        scanned,
        eh,
        hdr,
        got,
        symtab,
        shstrtab,
        build_id,
        property,
        chunks,
        commons,
        layout,
        placed,
    })
}

/// Writes `front`'s image to `out` with `mortar_image::link::write`.
///
/// # Errors
///
/// The image phase's diagnostics.
pub fn write(front: &Front, out: &Path) -> Result<Written, Failed> {
    write_with(front, out, &|_| {})
}

/// [`write`], calling `between(file)` before each input's recheck (a
/// test changes a file there).
///
/// # Errors
///
/// The image phase's diagnostics.
pub fn write_with(
    f: &Front,
    out: &Path,
    before_recheck: &(dyn Fn(FileId) + Sync),
) -> Result<Written, Failed> {
    with_link(f, before_recheck, |link| {
        let ids = f
            .inputs
            .files
            .iter()
            .filter_map(|(_, r)| {
                let p = std::str::from_utf8(r.path).ok()?;
                mortar_image::output::identity(&f.cwd.join(p))
            })
            .collect();
        let opts = OutputOptions::executable(ids);
        link::write(link, out, &opts, &mut |_, _| {})
    })
}

/// Calls `go` with the image phase's view of `f`.
fn with_link<T>(
    f: &Front,
    before_recheck: &(dyn Fn(FileId) + Sync),
    go: impl FnOnce(&link::Link<'_, 'static>) -> T,
) -> T {
    let tables = Tables { inputs: &f.inputs, resolution: &f.resolution, assignment: &f.assignment };
    let merge_eh = MergeEh::new(&f.merge.eh, &tables).expect("eh source");
    let core =
        CoreSections { resolution: &f.resolution, liveness: &f.liveness, split: &f.merge.split };
    let sections = LinkSections { core, assignment: &f.assignment };
    let syms = CoreSyms::new(&f.inputs, &f.resolution, &f.assignment);
    let cx = SymCtx {
        syms: &syms,
        sections: &sections,
        eh: Some((&f.eh, &merge_eh)),
        discard: f.plan.shaping.discard,
    };
    let facts = Facts { syms: &syms, liveness: &f.liveness };
    let proof = |file: FileId, index: mortar_elf::SectionIndex, len: u64| -> Vec<ProofRun> {
        proof_of(&f.inputs, file, index, len)
    };
    let recheck = |file: FileId| -> Result<(), CopyError> {
        before_recheck(file);
        // The store's handle of the file, by its inode (the model's
        // `Blob` has no public way back: a driver follow-up).
        let Some((dev, ino)) = std::str::from_utf8(f.inputs.files[file].path)
            .ok()
            .and_then(|p| mortar_image::output::identity(&f.cwd.join(p)))
        else {
            return Ok(());
        };
        let Some(blob) = f.store.blob_of_inode(dev, ino) else { return Ok(()) };
        f.store.recheck(blob).map_err(|e| match e {
            ReadError::Changed => CopyError::Changed,
            ReadError::Os { errno, .. } => CopyError::Read(Errno(errno.unwrap_or(0))),
            _ => CopyError::Read(Errno(0)),
        })
    };
    let output = output_kind(f.plan.kind);
    let link = link::Link {
        arch: &f.arch,
        pool: &f.pool,
        inputs: &f.inputs,
        resolution: &f.resolution,
        assignment: &f.assignment,
        liveness: &f.liveness,
        split: &f.merge.split,
        order: &f.order,
        records: &f.scanned.order,
        demands: &f.scanned.demands,
        layout: &f.layout,
        eh: Some((&f.eh, &merge_eh)),
        synth: link::SynthPlans {
            chunks: &f.chunks,
            hdr: f.hdr.as_ref(),
            got: &f.got,
            symtab: f.symtab.as_ref().map(|s| (s, cx)),
            shstrtab: &f.shstrtab,
            build_id: f.build_id.as_ref(),
            property: &f.property,
        },
        facts: &facts,
        proof: &proof,
        recheck: &recheck,
        options: LinkOptions {
            output,
            separate_code: f.plan.z.separate_code.unwrap_or(false),
            build_id: f.plan.shaping.build_id.as_ref().and_then(link::tree_style),
        },
    };
    go(&link)
}

/// The map of `f`'s link (`link::map_of`).
pub fn map(f: &Front) -> mortar_image::map::Map {
    with_link(f, &|_| {}, |l| link::map_of(l))
}

/// The proof runs of one input section, from its mapping symbols.
fn proof_of(
    inputs: &Inputs<'_>,
    file: FileId,
    index: mortar_elf::SectionIndex,
    len: u64,
) -> Vec<ProofRun> {
    let mut out = Vec::new();
    let Reader::Object(o) = inputs.files[file].reader else { return out };
    let exec = o.section(index).is_ok_and(|s| s.header().sh_flags & SHF_EXECINSTR != 0);
    let mut syms = Vec::new();
    if let Ok(Some(t)) = o.symbols() {
        for i in 1..t.first_global().0 {
            let Ok(s) = t.get(SymbolIndex(i)) else { continue };
            if !matches!(t.section_of(SymbolIndex(i), &s), Ok(SymbolSection::Section(x)) if x == index)
            {
                continue;
            }
            let Some(kind) = mapping_kind(t.name(&s).unwrap_or_default(), s.st_info) else {
                continue;
            };
            syms.push(MapSym { index: i, value: s.st_value, kind });
        }
    }
    proof_runs(&mut syms, len, exec, 0, 0, &mut out);
    out
}

/// The scan's facts of a key, for a static link (`mortar_scan` view.rs
/// `facts`, without shared objects).
struct Facts<'a, 'b> {
    syms: &'a CoreSyms<'b, 'static>,
    liveness: &'a Liveness,
}

impl SymNames for Facts<'_, '_> {
    fn name(&self, key: SymKey) -> &[u8] {
        self.syms.key_name(key)
    }
}

impl KeyFacts for Facts<'_, '_> {
    #[allow(clippy::many_single_char_names)]
    fn facts(&self, key: SymKey) -> SymFacts {
        let c = self.syms;
        let of = |kind, s: Option<Sym>| SymFacts {
            kind,
            sym_type: s.map_or(0, |s| s.sym_type()),
            st_other: s.map_or(0, |s| s.st_other),
            tls: s.is_some_and(|s| s.sym_type() == STT_TLS),
            dead: false,
            comdat_only: false,
            gnu_warning: false,
        };
        match key.kind() {
            SymKind::Local(e) => {
                let Some((s, t)) = c.sym(e) else { return of(TargetKind::Abs, None) };
                let place = t.section_of(e.index, &s);
                let kind = match place {
                    Ok(SymbolSection::Section(_)) if s.sym_type() == STT_GNU_IFUNC => {
                        TargetKind::Ifunc
                    }
                    Ok(SymbolSection::Section(_)) => TargetKind::Local,
                    _ => TargetKind::Abs,
                };
                let mut f = of(kind, Some(s));
                if let Ok(SymbolSection::Section(i)) = place
                    && e.index.0 != 0
                    && let Some(sid) = c.resolution.section_id(e.file, i)
                {
                    f.dead = !self.liveness.is_live(sid);
                }
                f
            }
            SymKind::Global(id) => {
                let def = c
                    .assignment
                    .def(c.resolution, id)
                    .or_else(|| c.resolution.def.get(id).copied());
                match def {
                    Some(Def::Defined(e)) => {
                        let s = c.sym(e).map(|x| x.0);
                        let abs = c.sym(e).is_some_and(|(s, t)| {
                            matches!(t.section_of(e.index, &s), Ok(SymbolSection::Absolute))
                        });
                        let kind = match s.map(|s| s.sym_type()) {
                            _ if abs => TargetKind::Abs,
                            Some(STT_GNU_IFUNC) => TargetKind::Ifunc,
                            _ => TargetKind::Local,
                        };
                        of(kind, s)
                    }
                    Some(Def::Common(e)) => of(TargetKind::Local, c.sym(e).map(|x| x.0)),
                    Some(Def::Internal(InternalDef::Provide { .. })) => of(TargetKind::Local, None),
                    Some(Def::Internal(_)) => of(TargetKind::Abs, None),
                    Some(Def::Undefined { weak: true }) => of(TargetKind::Weak0, None),
                    _ => of(TargetKind::Undefined, None),
                }
            }
        }
    }
}

/// `Slots` from `Demands` (S-64).
fn slots_of(d: &mortar_core::Demands, resolution: &Resolution) -> Slots {
    let list = |f: DemandFlags| -> Box<[SymKey]> { d.keys_with(f).collect() };
    Slots {
        got: list(DemandFlags::GOT),
        plt: list(DemandFlags::PLT | DemandFlags::CANON_PLT),
        iplt: list(DemandFlags::IPLT),
        tls_desc: list(DemandFlags::TLS_DESC),
        tls_gd: list(DemandFlags::TLS_GD),
        tls_ie: list(DemandFlags::GOT_TPREL),
        tls_ld: d.output.contains(mortar_core::demands::GlobalFlags::TLS_LD),
        symtab_locals: Column::filled(resolution.file_live.len(), mortar_core::Span::default()),
        ..Slots::default()
    }
}

fn property_plan(
    inputs: &Inputs<'_>,
    resolution: &Resolution,
    headers: &Headers<'_>,
    arch: A64,
    diag: &Collector,
) -> Option<PropertyPlan> {
    let mut paths: Vec<String> = Vec::new();
    let mut per: Vec<Vec<(SectionId, &[u8], u64)>> = Vec::new();
    for (file, row) in inputs.files.iter() {
        let Reader::Object(obj) = row.reader else { continue };
        if !resolution.file_live.get(file).is_some_and(|l| l.is_live()) {
            continue;
        }
        let range = resolution.section_base[file];
        let mut secs = Vec::new();
        for i in 1..range.len() {
            let sid = SectionId(range.start.0 + i);
            let h = &headers[sid];
            let r = &resolution.sections[sid];
            if h.name != b".note.gnu.property" || h.sh_type != SHT_NOTE || !r.is_kept() {
                continue;
            }
            let data = obj
                .section(mortar_elf::SectionIndex(i))
                .and_then(|s| s.data())
                .expect("a resident note");
            secs.push((sid, data, 1u64 << h.align));
        }
        let f = row.display();
        paths.push(match f.member() {
            Some(m) => format!("{}({m})", f.path()),
            None => f.path().to_owned(),
        });
        per.push(secs);
    }
    let objects: Vec<ObjectNotes<'_>> =
        paths.iter().zip(&per).map(|(p, s)| ObjectNotes { path: p, sections: s }).collect();
    let processor = |t: u32| arch.property_merge(t);
    PropertyPlan::new(&objects, Class::Elf64, &processor, false, diag).ok()
}

#[allow(clippy::too_many_arguments)]
fn synth_chunks(
    property: &PropertyPlan,
    build_id: Option<&BuildIdPlan>,
    got: &GotPlan,
    hdr: Option<&HdrPlan>,
    eh: &EhPlan,
    symtab: Option<&SymtabPlan>,
    shstrtab: &ShstrtabPlan,
    shndx: bool,
) -> (Vec<SynthChunk>, Vec<(SynthKind, u64)>) {
    let mut present: Vec<(SynthKind, u64)> = Vec::new();
    if property.exists() {
        present.push((SynthKind::NoteGnuProperty, property.size()));
    }
    if let Some(b) = build_id {
        present.push((SynthKind::NoteGnuBuildId, b.size()));
    }
    if got.rela_plt_size() > 0 {
        present.push((SynthKind::RelaPlt, got.rela_plt_size()));
    }
    if let Some(h) = hdr {
        present.push((SynthKind::EhFrameHdr, h.size()));
    }
    if eh.exists() {
        present.push((SynthKind::EhFrame, eh.size()));
    }
    if got.plt_size() > 0 {
        present.push((SynthKind::Plt, got.plt_size()));
    }
    if got.got_size() > 0 {
        present.push((SynthKind::Got, got.got_size()));
    }
    if got.got_plt_size() > 0 {
        present.push((SynthKind::GotPlt, got.got_plt_size()));
    }
    if let Some(s) = symtab {
        present.push((SynthKind::Symtab, s.symtab_size()));
        if shndx {
            present.push((SynthKind::SymtabShndx, s.shndx_size()));
        }
        present.push((SynthKind::Strtab, s.strtab_size()));
    }
    present.push((SynthKind::Shstrtab, shstrtab.size()));
    present.sort_by_key(|c| c.0);
    let chunks = present
        .iter()
        .map(|&(kind, size)| {
            let h = kind.header(Class::Elf64, false).expect("an M1 chunk");
            let (sh_type, align) = match kind {
                SynthKind::EhFrame => (eh.sh_type(), eh.align()),
                _ => (h.sh_type, h.align.trailing_zeros() as u8),
            };
            SynthChunk { synth: kind.synth(false), sh_type, flags: h.flags, align, size }
        })
        .collect();
    (chunks, present)
}

fn estimated_names<'n>(
    a: &Assignment<'n>,
    liveness: &Liveness,
    resolution: &Resolution,
) -> Vec<&'n [u8]> {
    let mut used = vec![false; a.outsecs.len() as usize];
    for (sid, out) in a.out_of.iter() {
        if let Some(o) = out.out()
            && liveness.is_live(sid)
            && resolution.sections[sid].kind != SectionKind::EhFrame
        {
            used[o.0 as usize] = true;
        }
    }
    let mut names: Vec<&[u8]> = a
        .outsecs
        .iter()
        .filter(|(id, o)| used[id.0 as usize] || o.keep)
        .map(|(_, o)| o.name)
        .collect();
    for k in [
        SynthKind::NoteGnuProperty,
        SynthKind::NoteGnuBuildId,
        SynthKind::EhFrame,
        SynthKind::Symtab,
        SynthKind::Strtab,
        SynthKind::Shstrtab,
    ] {
        names.push(k.name(false));
    }
    names
}

fn placed_names<'n>(placed: &Placed, a: &Assignment<'n>) -> Vec<&'n [u8]> {
    let mut rows: Vec<(u32, &'n [u8])> = Vec::new();
    for (_, c) in placed.placement.chunks.iter() {
        let Some(shndx) = c.shndx else { continue };
        let name: &'n [u8] = match c.kind {
            ChunkKind::Synthetic(s) => s.name,
            k => match k.part().map(|p| p.out).or(match k {
                ChunkKind::Merged(o) => Some(o),
                _ => None,
            }) {
                Some(o) => a.outsecs[o].name,
                None => continue,
            },
        };
        rows.push((shndx, name));
    }
    rows.sort_by_key(|r| r.0);
    rows.dedup_by_key(|r| r.0);
    rows.into_iter().map(|r| r.1).collect()
}

// ---- WP-027a's `syms.rs`, restated ------------------------------------------

/// `SymSource` over `Inputs`, `Resolution` and `Assignment`.
pub struct CoreSyms<'a, 's> {
    inputs: &'a Inputs<'s>,
    resolution: &'a Resolution,
    assignment: &'a Assignment<'a>,
    tables: Vec<Option<SymbolTable<'s>>>,
}

impl<'a, 's> CoreSyms<'a, 's> {
    pub fn new(
        inputs: &'a Inputs<'s>,
        resolution: &'a Resolution,
        assignment: &'a Assignment<'a>,
    ) -> Self {
        let tables = inputs
            .files
            .iter()
            .map(|(file, row)| {
                let live = resolution.file_live.get(file).is_some_and(|l| l.is_live());
                match row.reader {
                    Reader::Object(o) if live => o.symbols().ok().flatten(),
                    _ => None,
                }
            })
            .collect();
        Self { inputs, resolution, assignment, tables }
    }

    fn table(&self, file: FileId) -> Option<&SymbolTable<'s>> {
        self.tables.get(file.0 as usize)?.as_ref()
    }

    fn sym(&self, e: SymEntry) -> Option<(Sym, &SymbolTable<'s>)> {
        let t = self.table(e.file)?;
        Some((t.get(e.index).ok()?, t))
    }

    fn def_of(&self, e: SymEntry, s: &Sym, t: &SymbolTable<'s>) -> SymDef {
        match t.section_of(e.index, s) {
            Ok(SymbolSection::Section(index)) => {
                self.resolution.section_id(e.file, index).map_or(SymDef::Undefined, |section| {
                    SymDef::Section { section, value: s.st_value }
                })
            }
            Ok(SymbolSection::Absolute) => SymDef::Absolute(s.st_value),
            Ok(SymbolSection::Common) => SymDef::Common,
            _ => SymDef::Undefined,
        }
    }
}

impl SymSource for CoreSyms<'_, '_> {
    fn files(&self) -> u32 {
        self.tables.len() as u32
    }

    fn locals(&self, file: FileId) -> u32 {
        self.table(file).map_or(0, |t| t.first_global().0.saturating_sub(1))
    }

    fn local(&self, file: FileId, i: u32) -> LocalIn<'_> {
        let e = SymEntry { file, index: SymbolIndex(i + 1) };
        let Some((s, t)) = self.sym(e) else {
            return LocalIn {
                name: b"",
                info: 0,
                other: 0,
                size: 0,
                def: SymDef::Undefined,
                referenced: false,
            };
        };
        LocalIn {
            name: t.name(&s).unwrap_or_default(),
            info: s.st_info,
            other: s.st_other,
            size: s.st_size,
            def: self.def_of(e, &s, t),
            referenced: false,
        }
    }

    fn globals(&self) -> u32 {
        self.inputs.symbols.len()
    }

    fn global(&self, id: SymbolId) -> Option<GlobalIn<'_>> {
        if self.assignment.is_withdrawn(id) {
            return None;
        }
        let key = self.inputs.symbols.get(id)?;
        let def = self
            .assignment
            .def(self.resolution, id)
            .or_else(|| self.resolution.def.get(id).copied())?;
        let props = self.resolution.props.get(id).copied().unwrap_or_default();
        let referenced = props.refs.contains(RefFlags::FROM_OBJECT);
        let other = props.visibility & 3;
        let row = |typ, size, def, file, binding| GlobalIn {
            name: key.name,
            typ,
            binding,
            other,
            size,
            def,
            file,
            referenced,
        };
        Some(match def {
            Def::Defined(e) => {
                let (s, t) = self.sym(e)?;
                row(s.sym_type(), s.st_size, self.def_of(e, &s, t), e.file, props.binding)
            }
            Def::Common(e) => {
                let size = self.resolution.common_of(id).map_or(0, |c| c.size);
                row(STT_OBJECT, size, SymDef::Common, e.file, props.binding)
            }
            Def::Internal(_) => row(STT_NOTYPE, 0, SymDef::Internal, FileId::INTERNAL, STB_GLOBAL),
            Def::Undefined { .. } if matches!(other, STV_INTERNAL | STV_HIDDEN) => return None,
            Def::Undefined { weak } if referenced => {
                let binding = if weak { STB_WEAK } else { STB_GLOBAL };
                row(STT_NOTYPE, 0, SymDef::Undefined, FileId::INTERNAL, binding)
            }
            Def::Undefined { .. } | Def::Shared(_) => return None,
        })
    }

    fn key_name(&self, key: SymKey) -> &[u8] {
        match key.kind() {
            SymKind::Global(id) => self.inputs.symbols.get(id).map_or(&[][..], |k| k.name),
            SymKind::Local(e) => self.sym(e).and_then(|(s, t)| t.name(&s).ok()).unwrap_or_default(),
        }
    }
}

/// `CoreSections` where a section with no output section is not live.
#[derive(Clone, Copy)]
pub struct LinkSections<'a> {
    pub core: CoreSections<'a>,
    pub assignment: &'a Assignment<'a>,
}

impl Sections for LinkSections<'_> {
    fn live(&self, section: SectionId) -> bool {
        self.core.live(section) && self.assignment.out(section).is_some()
    }

    fn kind(&self, section: SectionId) -> SecKind {
        self.core.kind(section)
    }

    fn merge_live(&self, section: SectionId, offset: u64) -> bool {
        self.core.merge_live(section, offset)
    }
}

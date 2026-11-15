//! The front of the pipeline (WP-027a): from a parsed command line to the
//! final `Layout`, with every table the image phase reads.
//!
//! The order is the model's (`spec/model.md` §8, `docs/ARCHITECTURE.md`):
//!
//! 1. input (20): `mortar_input::load`;
//! 2. resolve (30): `mortar_resolve::resolve_inputs`;
//! 3. assignment (45): `mortar_layout::headers::gather`, then stage A
//!    (`mortar_layout::assign::assign`);
//! 4. split and merge (60): `mortar_merge::phase::run` over the
//!    assignment;
//! 5. liveness (50): `mortar_live::liveness`, then the fragment order of
//!    the live pieces (`mortar_merge::order::offsets`, the `MergeMap`);
//! 6. scan (70): `mortar_scan::scan`;
//! 7. synthetic sections (80): the plans of `mortar_synth`, as sizes, and
//!    `Slots`;
//! 8. layout (90): `mortar_layout::place`, with the range extension
//!    (thunks, erratum 843419) through [`crate::sites::LinkSites`]; then
//!    the sizes known only after it (`.shstrtab` after stage C, the
//!    symbol tables after the fixed point when patches exist, S-90 and
//!    layout L-121 by synth.md R-L5), and `Layout::validate`.
//!
//! Every phase reports into one collector; a phase that reports an error
//! stops the link there (`spec/diag.md` §7), so a link with errors has no
//! `Layout`.

use std::path::Path;

use mortar_a64::A64;
use mortar_arch::{Arch, OutputCx, OutputKind};
use mortar_core::assignment::Assignment;
use mortar_core::check::CheckContext;
use mortar_core::column::Column;
use mortar_core::demands::DemandFlags;
use mortar_core::id::{ChunkId, FileId, SectionId, SymKey};
use mortar_core::inputs::{Inputs, Reader};
use mortar_core::layout::{ChunkKind, Folds, Layout, Slots};
use mortar_core::par::Pool;
use mortar_core::plan::{self, Plan, Strip};
use mortar_core::resolution::{Resolution, SectionKind};
use mortar_core::{Liveness, SymbolId};
use mortar_diag::{Collector, Diagnostic, Key, Kind, Place};
use mortar_elf::consts::SHT_NOTE;
use mortar_elf::{Class, Machine, SectionIndex};
use mortar_input::load::{LoadEnv, load};
use mortar_input::store::Store;
use mortar_layout::chunk::{Common, SynthChunk, commons};
use mortar_layout::fixed::RangeExtension;
use mortar_layout::headers::{HeaderShapes, gather};
use mortar_layout::{Headers, LayoutDiag, Options, Placed, Target};
use mortar_live::{LiveInputs, LiveOptions, liveness};
use mortar_merge::order::{MergedChunk, offsets};
use mortar_merge::phase::{ArchRelocs, Phase, Tables};
use mortar_scan::{Mismatch, ScanInputs, ScanOptions, Scanned, Walker, scan};
use mortar_synth::notes::ObjectNotes;
use mortar_synth::symtab::{CoreSections, PatchFacts, SymCtx};
use mortar_synth::{
    BuildIdPlan, EhPlan, GotPlan, HdrPlan, MergeEh, PropertyPlan, ShstrtabPlan, SymtabPlan,
    SynthKind, chunk_ids,
};

use crate::proof::A64Proof;
use crate::sites::{LinkSites, SiteInputs, SynthIds};
use crate::syms::CoreSyms;
use crate::timing::Timer;

/// The first `ChunkId` after the file header, the program headers and
/// the section headers (model §4.4, layout's stage C creation order).
pub const FIRST_SYNTHETIC: ChunkId = ChunkId(3);

/// The plans of the synthetic chunks (phase 80) and the chunk each one
/// has.
#[derive(Debug, Clone)]
pub struct SynthPlans {
    /// `.eh_frame_hdr`: with `--eh-frame-hdr` and some live FDE (S-57).
    pub hdr: Option<HdrPlan>,
    pub got: GotPlan,
    /// `None` under `-s` (S-84).
    pub symtab: Option<SymtabPlan>,
    pub shstrtab: ShstrtabPlan,
    pub build_id: Option<BuildIdPlan>,
    pub property: PropertyPlan,
    /// The synthetic chunks that exist, in S-3 order, with their ids
    /// (`Link::synthetic` has them in this order).
    pub chunks: Vec<(SynthKind, ChunkId)>,
}

impl SynthPlans {
    /// The chunk of `kind`, if it exists.
    #[must_use]
    pub fn chunk(&self, kind: SynthKind) -> Option<ChunkId> {
        self.chunks.iter().find(|c| c.0 == kind).map(|c| c.1)
    }
}

/// A link up to its final `Layout`: what the image phase (WP-027b) reads.
#[derive(Debug)]
pub struct Front<'s> {
    pub plan: Plan,
    pub pool: Pool,
    pub arch: A64,
    pub target: Target,
    pub options: Options,
    pub inputs: Inputs<'s>,
    pub resolution: Resolution,
    pub comdat_mismatch: Vec<Mismatch>,
    pub gnu_warnings: Vec<(SymbolId, FileId, SectionIndex)>,
    pub headers: Headers<'s>,
    /// After layout's `finish` (stage A, then L-50's flags).
    pub assignment: Assignment<'s>,
    /// Phase 60: `Split`, the pieces' identity, `.eh_frame` parsed.
    pub merge: Phase,
    /// The merged chunk of each output section with live fragments.
    pub merged: Box<[MergedChunk]>,
    pub liveness: Liveness,
    pub scanned: Scanned,
    /// `.eh_frame` (S-50..S-56): its plan, apart from the others because
    /// the symbol tables' plan reads it.
    pub eh: EhPlan,
    pub synth: SynthPlans,
    pub commons: Vec<Common>,
    pub layout: Layout,
    /// What `place` returned besides the placement (the segment rules
    /// `Layout::validate` used, the rounds of the fixed point).
    pub placed: Placed,
}

/// Where a link runs.
#[derive(Debug, Clone, Copy)]
pub struct Env<'e> {
    pub cwd: &'e Path,
    pub pool: Pool,
}

/// The scan's output kind for a plan's.
#[must_use]
pub fn output_kind(kind: plan::OutputKind) -> OutputKind {
    match kind {
        plan::OutputKind::StaticExecutable => OutputKind::Static,
        plan::OutputKind::Executable => OutputKind::Exec,
        plan::OutputKind::PieExecutable | plan::OutputKind::StaticPie => OutputKind::Pie,
        plan::OutputKind::Shared => OutputKind::Shared,
        plan::OutputKind::Relocatable => OutputKind::Relocatable,
    }
}

/// Resolution's `ComdatMismatch` rows as the scan takes them.
fn mismatches(out: &mortar_resolve::Outcome) -> Vec<Mismatch> {
    out.comdat_mismatch
        .iter()
        .map(|m| Mismatch { key: m.key, group: m.group, keeper: m.keeper, dropped: m.dropped })
        .collect()
}

/// A layout diagnostic as the collector takes it: its own key, else
/// `fallback` (`spec/model.md` §8: `(90, 0, ChunkId)`, or a count); its
/// input section as the place.
fn layout_diag(inputs: &Inputs<'_>, res: &Resolution, d: LayoutDiag, fallback: Key) -> (Key, Diagnostic) {
    let key = d.key().unwrap_or(fallback);
    let at = d.at();
    let mut diag = d.into_diagnostic();
    if let Some((section, offset)) = at
        && let Some((file, index)) = res.sh_index(section)
        && let Some(f) = inputs.files.get(file)
    {
        let name = match f.reader {
            Reader::Object(o) => o
                .section(index)
                .ok()
                .and_then(|s| s.name().ok())
                .map(|n| String::from_utf8_lossy(n).into_owned())
                .unwrap_or_default(),
            _ => String::new(),
        };
        diag = diag.at(Place::section(f.display(), name, offset));
    }
    (key, diag)
}

fn internal(what: impl Into<String>) -> Diagnostic {
    Diagnostic::error(Kind::InternalError { what: what.into() })
}

/// Runs the front of the pipeline on `plan`. Every diagnostic goes to
/// `diag`; `None` when one is an error (no `Layout` is made).
#[allow(clippy::too_many_lines, reason = "the phases in order, each a few lines")]
pub fn front<'s>(
    plan: Plan,
    env: Env<'_>,
    store: &'s Store,
    diag: &Collector,
    timer: &mut Timer,
) -> Option<Front<'s>> {
    let pool = env.pool;
    let stop = |diag: &Collector| diag.has_errors();

    // 20: input.
    let load_env = LoadEnv { cwd: env.cwd.to_path_buf(), pool };
    let inputs = load(&plan, &load_env, store, diag)?;
    timer.phase(20, "input");
    let machine = inputs.target.machine;
    if machine != Machine::AArch64 {
        diag.emit(
            Key::new(90, 0, 0),
            Diagnostic::refused(mortar_diag::RefusalClass::Feature, format!("linking for {machine:?}"))
                .note("M1 links AArch64 (docs/ROADMAP.md)"),
        );
        return None;
    }

    // 30: resolve.
    let outcome = mortar_resolve::resolve_inputs(&inputs, &plan, &pool);
    for (k, d) in outcome.diagnostics.iter().cloned() {
        diag.emit(k, d);
    }
    timer.phase(30, "resolve");
    if stop(diag) {
        return None;
    }
    let comdat_mismatch = mismatches(&outcome);
    let gnu_warnings = outcome.gnu_warning_syms.clone();
    let resolution = outcome.resolution;

    // 45: assignment (layout stage A).
    let target = Target::of(machine);
    let options = match Options::from_plan(&plan, &target) {
        Ok(o) => o,
        Err(d) => {
            let (k, d) = layout_diag(&inputs, &resolution, d, Key::new(90, 0, 0));
            diag.emit(k, d);
            return None;
        }
    };
    let arch = A64::new(mortar_arch::Status::M1, options.fix_843419);
    let headers = match gather(&inputs, &resolution, &pool) {
        Ok(h) => h,
        Err(d) => {
            let (k, d) = layout_diag(&inputs, &resolution, d, Key::new(45, 0, 0));
            diag.emit(k, d);
            return None;
        }
    };
    let assignment =
        match mortar_layout::assign::assign(&headers, &resolution, &inputs.symbols, &target, &options, &pool) {
            Ok(a) => a.assignment,
            Err(errors) => {
                for (s, d) in errors {
                    let (file, index) = resolution.sh_index(s).unwrap_or((FileId(0), SectionIndex(0)));
                    let fallback = Key::new(45, file.0, u64::from(index.0));
                    let (k, d) = layout_diag(&inputs, &resolution, d, fallback);
                    diag.emit(k, d);
                }
                return None;
            }
        };
    timer.phase(45, "assignment");

    // 60: split and merge.
    let tables = Tables { inputs: &inputs, resolution: &resolution, assignment: &assignment };
    let merge = match mortar_merge::phase::run(&pool, &tables, &ArchRelocs(&arch)) {
        Ok(p) => p,
        Err(problems) => {
            for (k, d) in mortar_merge::phase::diagnostics(&inputs, &problems) {
                diag.emit(k, d);
            }
            return None;
        }
    };
    timer.phase(60, "split");

    // 50: liveness, then the fragments' order.
    let live = LiveInputs {
        inputs: &inputs,
        resolution: &resolution,
        assignment: &assignment,
        split: &merge.split,
    };
    let liveness = match liveness(&live, LiveOptions { gc_sections: plan.shaping.gc_sections }, &pool) {
        Ok(l) => l,
        Err((k, d)) => {
            diag.emit(k, d);
            return None;
        }
    };
    let order = offsets(&pool, &merge.split, &merge.meta, &liveness.frag_live);
    timer.phase(50, "liveness");

    // 70: scan.
    let output = output_kind(plan.kind);
    let scan_inputs = ScanInputs {
        inputs: &inputs,
        resolution: &resolution,
        liveness: &liveness,
        assignment: Some(&assignment),
        comdat_mismatch: &comdat_mismatch,
        gnu_warnings: &gnu_warnings,
    };
    let scan_options = ScanOptions { output, dynamic: false, proof: &A64Proof };
    let scanned = scan(&arch, &scan_inputs, &scan_options, &pool);
    for (k, d) in scanned.diagnostics.iter().cloned() {
        diag.emit(k, d);
    }
    timer.phase(70, "scan");
    if stop(diag) {
        return None;
    }

    // 80: the synthetic chunks' plans, as sizes.
    let slots = slots_of(&scanned.demands, &resolution);
    let merge_eh = match MergeEh::new(&merge.eh, &tables) {
        Ok(m) => m,
        Err(e) => {
            diag.emit(mortar_synth::error::key(0), e.into_diagnostic());
            return None;
        }
    };
    let eh = EhPlan::new(&merge_eh, |s: SectionId| liveness.is_live(s), &pool, diag).ok()?;
    let eh_frame_hdr = plan.shaping.eh_frame_hdr.unwrap_or(false);
    let hdr = HdrPlan::new(&merge_eh, &eh, eh_frame_hdr, &pool, diag).ok()?;
    let property = property_plan(&inputs, &resolution, &headers, &arch, diag).ok()?;
    let feature_1_and = property.value_u32(mortar_a64::FEATURE_1_AND);
    let variant = arch
        .plt_variant(&OutputCx { output, feature_1_and, pac_plt: false, force_bti: false })
        .map_err(|r| diag.emit(mortar_synth::error::key(0), r.diagnostic()))
        .ok()?;
    let got = GotPlan::new(&arch, &slots, scanned.demands.output, variant, diag).ok()?;
    let build_id = BuildIdPlan::new(plan.shaping.build_id.as_ref(), diag).ok()?;
    let sections = CoreSections { resolution: &resolution, liveness: &liveness, split: &merge.split };
    let syms = CoreSyms::new(&inputs, &resolution, &assignment);
    let strip_all = plan.shaping.strip == Some(Strip::All);
    let sym_cx = SymCtx {
        syms: &syms,
        sections: &sections,
        eh: Some((&eh, &merge_eh)),
        discard: plan.shaping.discard,
    };
    let facts = PatchFacts::of(&arch);
    let symtab = if strip_all {
        None
    } else {
        Some(SymtabPlan::new(&sym_cx, Class::Elf64, &[], facts, &pool, diag).ok()?)
    };
    timer.phase(80, "synthetic");

    // 90: layout, to the fixed point.
    let folds = Folds::default();
    let commons = commons(&resolution, Some(&inputs));
    let stack_notes = mortar_layout::stack::notes(&inputs, &resolution);
    let names: Vec<&[u8]> = estimated_names(&assignment, &liveness, &resolution);
    let mut shstrtab = ShstrtabPlan::new(&names);
    let mut plans = SynthPlans {
        hdr,
        got,
        symtab,
        shstrtab: shstrtab.clone(),
        build_id,
        property,
        chunks: Vec::new(),
    };
    let walker = Walker::new(&arch, &scan_inputs, scan_options);
    let mut attempt = 0;
    let (placement_assignment, placed, layout) = loop {
        attempt += 1;
        plans.shstrtab = shstrtab.clone();
        let (synthetic, present) = synth_chunks(&plans, &eh);
        let Some(sizes) = chunk_ids(FIRST_SYNTHETIC, &present) else {
            diag.emit(Key::new(80, 0, 0), internal("a synthetic chunk given twice"));
            return None;
        };
        plans.chunks = present
            .iter()
            .map(|p| p.0)
            .zip(sizes.iter().map(|s| s.0))
            .collect();
        let mut slots = slots.clone();
        slots.sizes = sizes;
        slots.eh_records = eh.records().into();
        if let Some(s) = &plans.symtab {
            slots.symtab_locals = s.symtab_locals().clone();
        } else {
            slots.symtab_locals = Column::filled(inputs.files.len(), Default::default());
        }
        let ids = SynthIds {
            got: plans.chunk(SynthKind::Got),
            got_plt: plans.chunk(SynthKind::GotPlt),
            plt: plans.chunk(SynthKind::Plt),
        };
        let site_inputs = SiteInputs {
            inputs: &inputs,
            resolution: &resolution,
            assignment: &assignment,
            headers: &headers,
            liveness: &liveness,
            split: &merge.split,
            merge: &order.map,
            demands: &scanned.demands,
            order: &scanned.order,
            slots: &slots,
            got: &plans.got,
            ids,
        };
        let sites = match LinkSites::new(site_inputs, &arch, &walker, &pool) {
            Ok(s) => s,
            Err((k, d)) => {
                diag.emit(k, d);
                return None;
            }
        };
        let range = RangeExtension { arch: &arch, sites: &sites };
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
        let placed = match mortar_layout::place(&link, &mut a, Some(&range), |e| inputs.entry_def(e), &pool) {
            Ok(p) => p,
            Err(errors) => {
                for (n, d) in errors.into_iter().enumerate() {
                    let (k, d) = layout_diag(&inputs, &resolution, d, Key::new(90, 0, n as u64));
                    diag.emit(k, d);
                }
                return None;
            }
        };
        // The sizes known only now (S-90 after stage C; R-L5 after the
        // fixed point): equal, or one more run with them (they are
        // non-allocated and last, so they move no address).
        let exact = ShstrtabPlan::new(&placed_names(&placed, &a));
        let mut again = exact.size() != shstrtab.size();
        shstrtab = exact;
        if !placed.placement.patches.is_empty()
            && let Some(old) = &plans.symtab
        {
            let new = SymtabPlan::new(&sym_cx, Class::Elf64, &placed.placement.patches, facts, &pool, diag).ok()?;
            again |= new.symtab_size() != old.symtab_size()
                || new.strtab_size() != old.strtab_size()
                || new.shndx_size() != old.shndx_size();
            plans.symtab = Some(new);
        }
        if again && attempt < 3 {
            continue;
        }
        if again {
            diag.emit(Key::new(90, 0, 0), internal("the non-allocated sizes do not settle"));
            return None;
        }
        let layout = Layout {
            folds: folds.clone(),
            merge: order.map.clone(),
            slots,
            placement: placed.placement.clone(),
        };
        break (a, placed, layout);
    };
    let mut seq = 0u64;
    for w in placed.warnings.iter().cloned() {
        let (k, d) = layout_diag(&inputs, &resolution, w, Key::new(90, 0, u64::MAX - 1000 + seq));
        seq += 1;
        diag.emit(k, d);
    }
    // Every synthetic chunk has the id its plan was given.
    for &(kind, id) in &plans.chunks {
        let ok = matches!(
            placed.placement.chunks.get(id).map(|c| c.kind),
            Some(ChunkKind::Synthetic(s)) if s.name == kind.name(false)
        );
        if !ok {
            diag.emit(Key::new(90, 0, u64::from(id.0)), internal("a synthetic chunk's id is not its plan's"));
            return None;
        }
    }
    let demands = &scanned.demands;
    let shapes = HeaderShapes(&headers);
    let cx = CheckContext {
        class: target.class,
        page_size: options.max_page,
        needs_entry: placed.placement.entry.is_some(),
        segment_rules: placed.rules,
        resolution: &resolution,
        assignment: &placement_assignment,
        split: &merge.split,
        liveness: &liveness,
        demands,
        shapes: &shapes,
    };
    if let Err(v) = layout.validate(&cx) {
        diag.emit(Key::new(90, 0, u64::MAX), internal(format!("the layout breaks its rules: {v:?}")));
        return None;
    }
    timer.phase(90, "layout");
    drop(walker);
    drop(merge_eh);
    Some(Front {
        plan,
        pool,
        arch,
        target,
        options,
        inputs,
        resolution,
        comdat_mismatch,
        gnu_warnings,
        headers,
        assignment: placement_assignment,
        merged: order.chunks.clone(),
        merge,
        liveness,
        scanned,
        eh,
        synth: plans,
        commons,
        layout,
        placed,
    })
}

/// `Slots` from `Demands` (S-64): each kind's keys in `SymKey` order.
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
        symtab_locals: Column::filled(resolution.file_live.len(), Default::default()),
        ..Slots::default()
    }
}

/// S-94: the property notes of every live object (those without one take
/// part as "every property absent", S-95).
fn property_plan(
    inputs: &Inputs<'_>,
    resolution: &Resolution,
    headers: &Headers<'_>,
    arch: &A64,
    diag: &Collector,
) -> Result<PropertyPlan, mortar_synth::Failed> {
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
            if let Ok(s) = obj.section(SectionIndex(i))
                && let Ok(data) = s.data()
            {
                secs.push((sid, data, 1u64 << h.align));
            }
        }
        let f = row.display();
        paths.push(match f.member() {
            Some(m) => format!("{}({m})", f.path()),
            None => f.path().to_owned(),
        });
        per.push(secs);
    }
    let objects: Vec<ObjectNotes<'_>> = paths
        .iter()
        .zip(&per)
        .map(|(p, s)| ObjectNotes { path: p, sections: s })
        .collect();
    let processor = |t: u32| arch.property_merge(t);
    // `-z memory-seal` is refused by the command line in M1 (R-C3).
    PropertyPlan::new(&objects, Class::Elf64, &processor, false, diag)
}

/// The synthetic chunks that exist, in S-3 order, as layout takes them,
/// and their sizes.
fn synth_chunks(p: &SynthPlans, eh: &EhPlan) -> (Vec<SynthChunk>, Vec<(SynthKind, u64)>) {
    let mut present: Vec<(SynthKind, u64)> = Vec::new();
    if p.property.exists() {
        present.push((SynthKind::NoteGnuProperty, p.property.size()));
    }
    if let Some(b) = &p.build_id {
        present.push((SynthKind::NoteGnuBuildId, b.size()));
    }
    if p.got.rela_plt_size() > 0 {
        present.push((SynthKind::RelaPlt, p.got.rela_plt_size()));
    }
    if let Some(h) = &p.hdr {
        present.push((SynthKind::EhFrameHdr, h.size()));
    }
    if eh.exists() {
        present.push((SynthKind::EhFrame, eh.size()));
    }
    if p.got.plt_size() > 0 {
        present.push((SynthKind::Plt, p.got.plt_size()));
    }
    if p.got.got_size() > 0 {
        present.push((SynthKind::Got, p.got.got_size()));
    }
    if p.got.got_plt_size() > 0 {
        present.push((SynthKind::GotPlt, p.got.got_plt_size()));
    }
    if let Some(s) = &p.symtab {
        present.push((SynthKind::Symtab, s.symtab_size()));
        if s.shndx_size() > 0 {
            present.push((SynthKind::SymtabShndx, s.shndx_size()));
        }
        present.push((SynthKind::Strtab, s.strtab_size()));
    }
    present.push((SynthKind::Shstrtab, p.shstrtab.size()));
    present.sort_by_key(|c| c.0);
    let chunks = present
        .iter()
        .map(|&(kind, size)| {
            let h = kind.header(Class::Elf64, false).unwrap_or_else(|| unreachable!("an M1 chunk"));
            let (sh_type, align) = match kind {
                SynthKind::EhFrame => (eh.sh_type(), eh.align()),
                _ => (h.sh_type, h.align.trailing_zeros() as u8),
            };
            SynthChunk { synth: kind.synth(false), sh_type, flags: h.flags, align, size }
        })
        .collect();
    (chunks, present)
}

/// The output section names before layout (S-90's set): every output
/// section with a live member, then the synthetic chunks' (added by
/// [`synth_chunks`]' kinds through `placed_names` once placed). An
/// estimate; [`front`] runs layout again in the rare case it differs.
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
    let mut names: Vec<&[u8]> =
        a.outsecs.iter().filter(|(id, o)| used[id.0 as usize] || o.keep).map(|(_, o)| o.name).collect();
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

/// The output section names of a placement, in section index order.
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

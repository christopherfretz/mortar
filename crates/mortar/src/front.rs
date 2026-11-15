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
use mortar_elf::{Class, SectionIndex};
use mortar_input::load::{LoadEnv, load_steps};
use mortar_input::store::Store;
use mortar_layout::chunk::{Common, SynthChunk, commons};
use mortar_layout::fixed::RangeExtension;
use mortar_layout::headers::{HeaderShapes, gather};
use mortar_layout::{Headers, LayoutDiag, Options, Placed, Target};
use mortar_live::{LiveInputs, LiveOptions, liveness};
use mortar_merge::order::{Order, offsets};
use mortar_merge::phase::{ArchRelocs, Phase, Tables};
use mortar_scan::{Mismatch, ScanInputs, ScanOptions, Scanned, Walker, scan};
use mortar_synth::notes::ObjectNotes;
use mortar_synth::symtab::{CoreSections, PatchFacts, SymCtx};
use mortar_synth::{
    BuildIdPlan, EhPlan, GotPlan, HdrPlan, MergeEh, PropertyPlan, ShstrtabPlan, SymtabPlan,
    SynthKind, chunk_ids,
};

use crate::arch::LinkArch;
use crate::sites::{LinkSites, SiteInputs, SynthIds};
use crate::syms::{CoreSyms, LinkSections};
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
    pub arch: LinkArch,
    pub target: Target,
    pub options: Options,
    pub inputs: Inputs<'s>,
    pub resolution: Resolution,
    pub comdat_mismatch: Vec<Mismatch>,
    pub gnu_warnings: Vec<(SymbolId, FileId, SectionIndex)>,
    /// D-12's closure-only keys (`Outcome.closure_only`).
    pub closure_only: Vec<(SymbolId, FileId)>,
    pub headers: Headers<'s>,
    /// After layout's `finish` (stage A, then L-50's flags).
    pub assignment: Assignment<'s>,
    /// Phase 60: `Split`, the pieces' identity, `.eh_frame` parsed.
    pub merge: Phase,
    /// Phase 50's fragment order (S-24): `order.map` is `layout.merge`;
    /// `order.chunks` the merged chunk of each output section with live
    /// fragments; `order.order` their fragments in offset order.
    pub order: Order,
    pub liveness: Liveness,
    pub scanned: Scanned,
    /// `.eh_frame` (S-50..S-56): its plan, apart from the others because
    /// the symbol tables' plan reads it.
    pub eh: EhPlan,
    pub synth: SynthPlans,
    /// The dynamic tables' plans; `None` in a static link.
    pub dyn_tables: Option<crate::dynamic::DynTables>,
    /// The command line's words, response files expanded (the map's `arg`
    /// records, `spec/plumb.md` §3.2); set by the driver after `front`.
    pub args: Vec<Vec<u8>>,
    pub commons: Vec<Common>,
    pub layout: Layout,
    /// What `place` returned besides the placement (the segment rules
    /// `Layout::validate` used, the rounds of the fixed point).
    pub placed: Placed,
    /// The erratum fixes (`spec/a64.md` §11.2): words written over fill's
    /// bytes, by address, sorted (`LinkSites::erratum_fixes`).
    pub erratum_fixes: Vec<(u64, [u8; 4])>,
}

/// One output section of a [`Front`]'s layout, as its section header will
/// show it (the chunks that share a section index, `spec/layout.md` §4.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutRow {
    pub shndx: u32,
    pub name: Vec<u8>,
    pub sh_type: u32,
    pub flags: u64,
    /// `None`: not allocated.
    pub addr: Option<u64>,
    /// From the first chunk's start to the last one's end.
    pub size: u64,
    /// In bytes.
    pub align: u64,
}

impl Front<'_> {
    /// Calls `f` with the symbol tables' context (`spec/synth.md` §7.1)
    /// as phase 80 built it: what `SymtabPlan::write_symtab` and
    /// `write_strtab` read.
    ///
    /// # Errors
    ///
    /// `MergeEh::new`'s, which phase 80 already passed on these tables.
    pub fn with_sym_cx<T>(
        &self,
        f: impl FnOnce(&SymCtx<'_>) -> T,
    ) -> Result<T, mortar_synth::error::SynthError> {
        let tables = Tables {
            inputs: &self.inputs,
            resolution: &self.resolution,
            assignment: &self.assignment,
        };
        let merge_eh = MergeEh::new(&self.merge.eh, &tables)?;
        let core = CoreSections {
            resolution: &self.resolution,
            liveness: &self.liveness,
            split: &self.merge.split,
        };
        let sections = LinkSections { core, assignment: &self.assignment };
        let syms = CoreSyms::new(&self.inputs, &self.resolution, &self.assignment)
            .dynamic(self.dyn_tables.is_some());
        let cx = SymCtx {
            syms: &syms,
            sections: &sections,
            eh: Some((&self.eh, &merge_eh)),
            discard: self.plan.shaping.discard,
        };
        Ok(f(&cx))
    }

    /// The output sections, in section index order.
    #[must_use]
    pub fn sections(&self) -> Vec<OutRow> {
        let mut rows: Vec<OutRow> = Vec::new();
        let mut span: Vec<(u64, u64)> = Vec::new();
        let mut chunks: Vec<_> = self
            .layout
            .placement
            .chunks
            .iter()
            .filter_map(|(_, c)| c.shndx.map(|n| (n, *c)))
            .collect();
        chunks.sort_by_key(|(n, c)| (*n, c.addr.unwrap_or(c.offset), c.offset));
        for (shndx, c) in chunks {
            let start = c.addr.unwrap_or(c.offset);
            let end = start + c.size;
            if let (Some(last), Some(sp)) = (rows.last_mut(), span.last_mut())
                && last.shndx == shndx
            {
                sp.1 = sp.1.max(end);
                last.size = sp.1 - sp.0;
                last.align = last.align.max(1 << c.align);
                continue;
            }
            let name: Vec<u8> = match c.kind {
                ChunkKind::Synthetic(s) => mortar_layout::rank::home(s.name).to_vec(),
                k => k.out().map_or_else(Vec::new, |o| self.assignment.outsecs[o].name.to_vec()),
            };
            rows.push(OutRow {
                shndx,
                name,
                sh_type: c.sh_type,
                flags: c.flags,
                addr: c.addr,
                size: c.size,
                align: 1 << c.align,
            });
            span.push((start, end));
        }
        rows
    }
}

/// The resident input bytes that repay one more worker after the input
/// phase (WP-032h: at default threads under load 10, hello-c's 6.9 MB
/// link went 25.7 -> 23.5 ms on 2 workers; 2, 4 and 8 MB per worker were
/// equal on hello-cxx (15 MB) and a 68 MB link).
const BYTES_PER_WORKER: u64 = 4 << 20;

/// The pool the phases after input run on: without `--threads`, at most
/// one worker per [`BYTES_PER_WORKER`] resident input bytes, so a small
/// link does not pay for workers it cannot use. The count is a function
/// of the inputs, never of a clock; an explicit `--threads` (or a
/// scheduling seed, which exists to exercise schedules) keeps the pool.
fn capped(pool: Pool, plan: &Plan, resident: u64) -> Pool {
    if plan.bookkeeping.threads.is_some() || pool.seed().is_some() {
        return pool;
    }
    let cap = usize::try_from(resident.div_ceil(BYTES_PER_WORKER)).unwrap_or(usize::MAX).max(1);
    pool.with_threads(pool.threads().min(cap))
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
fn layout_diag(
    inputs: &Inputs<'_>,
    res: &Resolution,
    d: LayoutDiag,
    fallback: Key,
) -> (Key, Diagnostic) {
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
    let time_steps = mortar_core::wp032b_steps::enabled();
    let mut steps = mortar_input::steps::Steps::default();
    let inputs = load_steps(&plan, &load_env, store, diag, &mut steps);
    if time_steps {
        print_steps("input", &steps.times, &steps.counts);
    }
    let inputs = inputs?;
    timer.phase(20, "input");
    let pool = capped(pool, &plan, store.stats().resident);
    let machine = inputs.target.machine;
    // 30: resolve.
    let outcome = mortar_resolve::resolve_inputs(&inputs, &plan, &pool);
    if time_steps {
        print_steps("resolve", &outcome.steps.times, &outcome.steps.counts);
    }
    for (k, d) in outcome.diagnostics.iter().cloned() {
        diag.emit(k, d);
    }
    timer.phase(30, "resolve");
    if stop(diag) {
        return None;
    }
    // 40: the one-definition rule (`spec/odr.md` §8.1, O-41); nothing is
    // read under `--odr=off` (O-42).
    let odr = mortar_odr::check(&inputs, &outcome.resolution, &plan.diagnostics.odr, &pool);
    for (k, d) in odr.diagnostics {
        diag.emit(k, d);
    }
    if let Some(line) = odr.stats {
        timer.line(line);
    }
    // resolve reports an undefined entry symbol (resolve.md §5.5, key
    // (30,0,argument)); layout finds the same and says so again (L-103):
    // one diagnostic, resolve's.
    let entry_warned = outcome.diagnostics.iter().any(|(_, d)| d.id() == "entry-not-found");
    let comdat_mismatch = mismatches(&outcome);
    let gnu_warnings = outcome.gnu_warning_syms.clone();
    let closure_only = outcome.closure_only;
    let roots = outcome.roots;
    let needed = outcome.needed;
    let resolution = outcome.resolution;

    // 45: assignment (layout stage A). A dynamic output on an
    // architecture whose join has not landed is refused here, by name.
    let status = match crate::dynamic::arch_status(&plan, machine) {
        Ok(s) => s,
        Err(d) => {
            diag.emit(Key::new(45, 0, 0), d);
            return None;
        }
    };
    let target = Target::of(machine);
    let options = match Options::from_plan(&plan, &target) {
        Ok(o) => o,
        Err(d) => {
            let (k, d) = layout_diag(&inputs, &resolution, d, Key::new(90, 0, 0));
            diag.emit(k, d);
            return None;
        }
    };
    // A dynamic output (`Exec`, `Pie`; WP-053): shared objects, a
    // dynamic symbol table, the scan's dynamic rules.
    let dynamic = status == mortar_arch::Status::M2;
    // An output with `.dynamic`: the dynamic ones and a `StaticPie`
    // (`spec/dyn.md` D-1, D-2: `.dynamic` for glibc's self-relocation,
    // nothing a loader reads; its scan, its architecture tables and its
    // symbols are a static link's, WP-052).
    let has_dynamic = dynamic || plan.kind == plan::OutputKind::StaticPie;
    let Some(link_arch) = LinkArch::of(machine, options.fix_843419, status) else {
        diag.emit(Key::new(45, 0, 0), internal(format!("no architecture for {}", machine.name())));
        return None;
    };
    let arch = link_arch.get();
    // S-68: an output whose relocations are REL (i386) names `.rel.plt`.
    let rel = arch.facts().rel_form == mortar_arch::RelForm::Rel;
    let headers = match gather(&inputs, &resolution, &pool) {
        Ok(h) => h,
        Err(d) => {
            let (k, d) = layout_diag(&inputs, &resolution, d, Key::new(45, 0, 0));
            diag.emit(k, d);
            return None;
        }
    };
    let assignment = match mortar_layout::assign::assign(
        &headers,
        &resolution,
        &inputs.symbols,
        &target,
        &options,
        &pool,
    ) {
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
    let merge = match mortar_merge::phase::run(&pool, &tables, &ArchRelocs(arch)) {
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
        roots: &roots,
        arch: Some(arch),
    };
    let live_options = LiveOptions {
        gc_sections: plan.shaping.gc_sections,
        keep_exported: plan.shaping.gc_keep_exported,
    };
    let liveness = match liveness(&live, live_options, &pool) {
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
        closure_only: &closure_only,
    };
    // D-25: a writable place, from the section mapping.
    let out_writable = crate::dynlink::OutWritable(&assignment);
    let sections: Option<&dyn mortar_scan::weak::SectionMap> =
        if dynamic { Some(&out_writable) } else { None };
    let scan_options = ScanOptions { output, dynamic, sections, proof: link_arch.proof() };
    let mut scanned = scan(arch, &scan_inputs, &scan_options, &pool);
    // The branch candidates are the sites' input only: dropped once the
    // sites are final, before the image.
    let branch_candidates = std::mem::take(&mut scanned.branches);
    for (k, d) in scanned.diagnostics.iter().cloned() {
        diag.emit(k, d);
    }
    let deferred = Deferred {
        arch,
        inputs: &scan_inputs,
        options: scan_options,
        order: &scanned.order,
        weak_dyn: &scanned.demands.weak_dyn,
        pool: &pool,
        diag,
        armed: true,
    };
    timer.phase(70, "scan");
    if stop(diag) {
        return None;
    }

    // 80: the synthetic chunks' plans, as sizes.
    let mut steps = mortar_core::wp032b_steps::Steps::new("synthetic");
    let mut slots = slots_of(&scanned.demands, &resolution);
    let merge_eh = match MergeEh::new(&merge.eh, &tables) {
        Ok(m) => m,
        Err(e) => {
            diag.emit(mortar_synth::error::key(0), e.into_diagnostic());
            return None;
        }
    };
    steps.step("merge eh", 0, "");
    let eh = EhPlan::new(&merge_eh, |s: SectionId| liveness.is_live(s), &pool, diag).ok()?;
    steps.step("eh plan", 0, "");
    let eh_frame_hdr = plan.shaping.eh_frame_hdr.unwrap_or(false);
    let hdr = HdrPlan::new(&merge_eh, &eh, eh_frame_hdr, &pool, diag).ok()?;
    steps.step("hdr plan", 0, "");
    let memory_seal = plan.z.memory_seal == Some(true);
    let property = property_plan(&inputs, &resolution, &headers, arch, memory_seal, diag).ok()?;
    let feature_1_and = property.value_u32(link_arch.feature_1().0);
    // D-49: `-z now` binds every PLT entry at load (always `false` in a
    // static link).
    let bind_now = has_dynamic && plan.z.now == Some(true);
    let variant = arch
        .plt_variant(&OutputCx {
            output,
            feature_1_and,
            pac_plt: false,
            force_bti: false,
            bind_now,
        })
        .map_err(|r| diag.emit(mortar_synth::error::key(0), r.diagnostic()))
        .ok()?;
    let got_output = got_base_defined(&inputs, &resolution, scanned.demands.output);
    let walker = Walker::new(arch, &scan_inputs, scan_options)
        .with_weak_dyn(scanned.demands.weak_dyn.clone());
    let (got, dyn_tables) = if has_dynamic {
        let src = crate::dynlink::DynSource::new(&inputs, &resolution, &assignment, &plan);
        let kinds = crate::dynlink::ScanKinds(&walker);
        let t = crate::dynlink::PlanIn {
            plan: &plan,
            arch,
            inputs: &inputs,
            resolution: &resolution,
            assignment: &assignment,
            demands: &scanned.demands,
            liveness: &liveness,
            headers: &headers,
            src: &src,
            kinds: &kinds,
            needed: &needed,
            variant,
            output_cx: OutputCx {
                output,
                feature_1_and,
                pac_plt: false,
                force_bti: false,
                bind_now,
            },
            got_output,
            relro: options.relro,
            pool: &pool,
        };
        let (got, tables) = crate::dynlink::plan(&t, &mut slots, diag)?;
        (got, Some(tables))
    } else {
        (GotPlan::new(arch, &slots, got_output, variant, diag).ok()?, None)
    };
    let build_id = BuildIdPlan::new(plan.shaping.build_id.as_ref(), diag).ok()?;
    steps.step("property got build-id", 0, "");
    let core = CoreSections { resolution: &resolution, liveness: &liveness, split: &merge.split };
    let sections = LinkSections { core, assignment: &assignment };
    let syms = CoreSyms::new(&inputs, &resolution, &assignment).dynamic(dynamic);
    let strip_all = plan.shaping.strip == Some(Strip::All);
    let sym_cx = SymCtx {
        syms: &syms,
        sections: &sections,
        eh: Some((&eh, &merge_eh)),
        discard: plan.shaping.discard,
    };
    let facts = PatchFacts::of(arch);
    let symtab = if strip_all {
        None
    } else {
        Some(SymtabPlan::new(&sym_cx, target.class, &[], facts, &pool, diag).ok()?)
    };
    steps.step("symtab plan", 0, "");
    timer.phase(80, "synthetic");
    let mut steps = mortar_core::wp032b_steps::Steps::new("layout");

    // 90: layout, to the fixed point.
    let folds = Folds::default();
    let commons = commons(&resolution, Some(&inputs));
    let stack_notes = mortar_layout::stack::notes(&inputs, &resolution);
    let mut plans = SynthPlans {
        hdr,
        got,
        symtab,
        shstrtab: ShstrtabPlan::new(&[]),
        build_id,
        property,
        chunks: Vec::new(),
    };
    // The section names, estimated from the output sections with live
    // input and the synthetic chunks that exist (their kinds do not depend
    // on `.shstrtab`'s size), so that one placement usually suffices; the
    // exact names are checked after it.
    // The copy allocations as layout places them (D-42, D-60).
    let copy_rows = match &dyn_tables {
        Some(d) => match mortar_layout::chunk::copy_rows(&slots, |i, _| d.copy.size[i as usize]) {
            Ok(r) => r,
            Err(e) => {
                let (k, e) = layout_diag(&inputs, &resolution, e, Key::new(80, 0, 0));
                diag.emit(k, e);
                return None;
            }
        },
        None => Vec::new(),
    };
    let dyn_chunks = match dyn_chunks(dyn_tables.as_ref(), &copy_rows) {
        Ok(c) => c,
        Err(e) => {
            let (k, e) = layout_diag(&inputs, &resolution, e, Key::new(80, 0, 0));
            diag.emit(k, e);
            return None;
        }
    };
    let kinds: Vec<SynthKind> = synth_chunks(&plans, &dyn_chunks, &eh, false, target.class, rel)
        .1
        .iter()
        .map(|p| p.0)
        .collect();
    let names: Vec<&[u8]> =
        estimated_names(&assignment, &liveness, &resolution, &kinds, options.relro, rel);
    let mut shstrtab = ShstrtabPlan::new(&names);
    // S-85: `.symtab_shndx` exists when some section index reaches
    // `SHN_LORESERVE`; the count of section headers (with the null one)
    // is estimated, then checked after the placement.
    let mut shndx = SymtabPlan::needs_shndx(names.len() as u32 + 2);
    let mut attempt = 0;
    // The sites read the slots' key lists only, which every attempt shares
    // (`slots`, before the sizes are set); they are found once per set of
    // synthetic chunk ids. The placement of the attempt before is the next
    // one's start (`mortar_layout::place_again`), and the symbol tables'
    // plan is made once per patch table (a function of it).
    let demand_slots = &slots;
    let mut kept_sites: Option<(SynthIds, LinkSites<'_, '_>)> = None;
    let mut prev: Option<Placed> = None;
    let mut planned_for: Option<Box<[mortar_core::layout::Patch]>> = None;
    steps.step("prepare", 0, "");
    let (placement_assignment, placed, layout, erratum_fixes) = loop {
        attempt += 1;
        plans.shstrtab = shstrtab.clone();
        let (synthetic, present) = synth_chunks(&plans, &dyn_chunks, &eh, shndx, target.class, rel);
        let Some(sizes) = chunk_ids(FIRST_SYNTHETIC, &present) else {
            diag.emit(Key::new(80, 0, 0), internal("a synthetic chunk given twice"));
            return None;
        };
        plans.chunks = present.iter().map(|p| p.0).zip(sizes.iter().map(|s| s.0)).collect();
        let mut slots = slots.clone();
        slots.sizes = sizes;
        slots.eh_records = eh.records().into();
        if let Some(s) = &plans.symtab {
            slots.symtab_locals = s.symtab_locals().clone();
        } else {
            slots.symtab_locals = Column::filled(inputs.files.len(), mortar_core::Span::default());
        }
        let ids = SynthIds {
            got: plans.chunk(SynthKind::Got),
            got_plt: plans.chunk(SynthKind::GotPlt),
            plt: plans.chunk(SynthKind::Plt),
        };
        let site_inputs = SiteInputs {
            resolution: &resolution,
            assignment: &assignment,
            headers: &headers,
            liveness: &liveness,
            split: &merge.split,
            merge: &order.map,
            demands: &scanned.demands,
            order: &scanned.order,
            slots: demand_slots,
            got: &plans.got,
            ids,
        };
        if kept_sites.as_ref().is_none_or(|(i, _)| *i != ids) {
            match LinkSites::new(site_inputs, &branch_candidates, arch, &walker, &pool) {
                Ok(s) => kept_sites = Some((ids, s)),
                Err((k, d)) => {
                    diag.emit(k, d);
                    return None;
                }
            }
        }
        let Some((_, link_sites)) = &kept_sites else { return None };
        steps.step("sites", 0, "");
        let range = RangeExtension { arch, sites: link_sites };
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
        steps.step("assignment clone", 0, "");
        let placed = match mortar_layout::run::place_with_copies(
            &link,
            &copy_rows,
            &mut a,
            Some(&range),
            |e| inputs.entry_def(e),
            &pool,
            prev.as_ref(),
        ) {
            Ok(p) => p,
            Err(errors) => {
                for (n, d) in errors.into_iter().enumerate() {
                    let (k, d) = layout_diag(&inputs, &resolution, d, Key::new(90, 0, n as u64));
                    diag.emit(k, d);
                }
                return None;
            }
        };
        steps.step("place", u64::from(placed.rounds), "round");
        // The sizes known only now (S-90 after stage C; R-L5 after the
        // fixed point): equal, or one more run with them (they are
        // non-allocated and last, so they move no address).
        let exact_names = placed_names(&placed, &a);
        let exact = ShstrtabPlan::new(&exact_names);
        let mut again = exact.size() != shstrtab.size();
        shstrtab = exact;
        // The headers: the null one and one per output section.
        let exact_shndx = SymtabPlan::needs_shndx(exact_names.len() as u32 + 1);
        again |= exact_shndx != shndx;
        shndx = exact_shndx;
        if !placed.placement.patches.is_empty()
            && planned_for.as_ref() != Some(&placed.placement.patches)
            && let Some(old) = &plans.symtab
        {
            let new = old.with_patches(sym_cx.syms, &placed.placement.patches);
            #[cfg(debug_assertions)]
            {
                let full = SymtabPlan::new(
                    &sym_cx,
                    target.class,
                    &placed.placement.patches,
                    facts,
                    &pool,
                    diag,
                )
                .ok()?;
                assert_eq!(new, full, "SymtabPlan::with_patches");
            }
            again |= new.symtab_size() != old.symtab_size()
                || new.strtab_size() != old.strtab_size()
                || (shndx && new.shndx_size() != old.shndx_size());
            plans.symtab = Some(new);
            planned_for = Some(placed.placement.patches.clone());
            steps.step("symtab replan", placed.placement.patches.len() as u64, "patch");
        }
        if again && attempt < 3 {
            prev = Some(placed);
            continue;
        }
        if again {
            diag.emit(Key::new(90, 0, 0), internal("the non-allocated sizes do not settle"));
            return None;
        }
        // a64.md §11.2: the fixes, from the words fill writes at the
        // final addresses (while the sites are at hand).
        let fixes = match link_sites.erratum_fixes(&placed.placement, &pool) {
            Ok(f) => f,
            Err(d) => {
                let (k, d) = layout_diag(&inputs, &resolution, d, Key::new(90, 0, u64::MAX - 1));
                diag.emit(k, d);
                return None;
            }
        };
        steps.step("erratum fixes", 0, "");
        // The exact names, in section index order (S-90: `sh_name` of
        // section `n` is the plan's `n`-th name); their size is the one
        // placed.
        plans.shstrtab = shstrtab.clone();
        let layout = Layout {
            folds: folds.clone(),
            merge: order.map.clone(),
            slots,
            placement: placed.placement.clone(),
        };
        steps.step("layout clone", 0, "");
        break (a, placed, layout, fixes);
    };
    drop(branch_candidates);
    steps.step("drop sites", 0, "");
    // X86-45, X86-38: an ELFCLASS32 TLS block of 2^31 bytes or more would
    // put a TP offset outside s(32); refused by name, as L-73 refuses an
    // end beyond 2^32.
    if target.class == Class::Elf32
        && let Some((_, tls)) =
            placed.placement.segments.iter().find(|(_, g)| g.p_type == mortar_elf::consts::PT_TLS)
        && tls.memsz >= 1 << 31
    {
        let d = Diagnostic::error(Kind::OutputTooLarge {
            what: "PT_TLS".to_owned(),
            limit: format!("{:#x}", 1u64 << 31),
        });
        diag.emit(Key::new(90, 0, 0), d);
        return None;
    }
    for (seq, w) in (0u64..).zip(placed.warnings.iter().cloned()) {
        let (k, d) = layout_diag(&inputs, &resolution, w, Key::new(90, 0, u64::MAX - 1000 + seq));
        if entry_warned && d.id() == "entry-not-found" {
            continue;
        }
        diag.emit(k, d);
    }
    // Every synthetic chunk has the id its plan was given.
    for &(kind, id) in &plans.chunks {
        let ok = matches!(
            placed.placement.chunks.get(id).map(|c| c.kind),
            Some(ChunkKind::Synthetic(s)) if s.name == kind.name(rel)
        );
        if !ok {
            diag.emit(
                Key::new(90, 0, u64::from(id.0)),
                internal("a synthetic chunk's id is not its plan's"),
            );
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
    steps.step("checks", 0, "");
    let validated = layout.validate(&cx);
    steps.step("validate", 0, "");
    if let Err(v) = validated {
        diag.emit(
            Key::new(90, 0, u64::MAX),
            internal(format!("the layout breaks its rules: {v:?}")),
        );
        return None;
    }
    timer.phase(90, "layout");
    deferred.disarm();
    drop(kept_sites);
    drop(walker);
    drop(merge_eh);
    Some(Front {
        plan,
        pool,
        arch: link_arch,
        target,
        options,
        inputs,
        resolution,
        comdat_mismatch,
        gnu_warnings,
        closure_only,
        headers,
        assignment: placement_assignment,
        merge,
        order,
        liveness,
        scanned,
        eh,
        synth: plans,
        dyn_tables,
        args: Vec::new(),
        commons,
        layout,
        placed,
        erratum_fixes,
    })
}

/// `MORTAR_STEPS` for the phases that collect their sub-steps (input and
/// resolve): their times and work counts on standard error, in the format
/// of `mortar_core::wp032b_steps` (the bench's unit costs; nothing in the
/// output depends on them).
fn print_steps(
    phase: &str,
    times: &[(&'static str, std::time::Duration)],
    counts: &[(&'static str, u64)],
) {
    use std::fmt::Write as _;
    let mut s = String::new();
    for (name, d) in times {
        let _ = writeln!(s, "mortar: steps: {phase} {name}: {:.2} ms", d.as_secs_f64() * 1e3);
    }
    for (name, n) in counts {
        let _ = writeln!(s, "mortar: steps: {phase} count {name}: {n}");
    }
    eprint!("{s}");
}

/// WP-032c (`spec/image.md` I-14): a link that fails after the scan and
/// before fill gets the scan's diagnostics of the sections the scan left
/// to fill's checks, which fill would have found (dropped with `armed`).
struct Deferred<'g, 'a, 's> {
    arch: &'a dyn Arch,
    inputs: &'g ScanInputs<'a, 's>,
    options: ScanOptions<'a>,
    order: &'g mortar_scan::RecIndex,
    weak_dyn: &'g mortar_core::IdSet<SymbolId>,
    pool: &'g Pool,
    diag: &'g Collector,
    armed: bool,
}

impl Deferred<'_, '_, '_> {
    /// The front has succeeded: fill checks the sections.
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for Deferred<'_, '_, '_> {
    fn drop(&mut self) {
        if !self.armed || self.order.deferred().is_empty() {
            return;
        }
        let walker =
            Walker::new(self.arch, self.inputs, self.options).with_weak_dyn(self.weak_dyn.clone());
        for (k, d) in walker.deferred_report(self.order.deferred(), self.pool) {
            self.diag.emit(k, d);
        }
    }
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
        symtab_locals: Column::filled(resolution.file_live.len(), mortar_core::Span::default()),
        ..Slots::default()
    }
}

/// S-94: the property notes of every live object (those without one take
/// part as "every property absent", S-95).
fn property_plan(
    inputs: &Inputs<'_>,
    resolution: &Resolution,
    headers: &Headers<'_>,
    arch: &dyn Arch,
    memory_seal: bool,
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
            // A note is resident under every store (`spec/landed/input/
            // WP-021b.md`); a note that cannot be read fails the link,
            // never "no properties".
            let read = obj.section(SectionIndex(i)).and_then(|s| s.data());
            match read {
                Ok(data) => secs.push((sid, data, 1u64 << h.align)),
                Err(e) => {
                    let what = match e {
                        mortar_elf::Error::NotResident { .. } => {
                            Diagnostic::error(Kind::InternalError {
                                what: "a .note.gnu.property section is not resident".into(),
                            })
                        }
                        e => Diagnostic::error(Kind::Malformed { what: e.to_string() }),
                    };
                    let place = Place::section(row.display(), ".note.gnu.property", 0);
                    diag.emit(mortar_synth::error::record_key(sid, 0), what.at(place));
                    return Err(mortar_synth::Failed);
                }
            }
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
    // `-z memory-seal` (R-C3): the last of it and `-z nomemory-seal` wins.
    PropertyPlan::new(&objects, arch.facts().class, &processor, memory_seal, diag)
}

/// The output demands the GOT is planned from: the scan's, with
/// `GOT_BASE` when `_GLOBAL_OFFSET_TABLE_` is defined by the linker
/// (layout L-101 values it at the GOT header). A symbol table can name
/// the symbol with no record referring to it (GNU as writes it beside
/// i386's `@ntpoff`, `@gotntpoff`, ...): the definition still has its
/// section. One pass over the keys, testing the definition first.
fn got_base_defined(
    inputs: &Inputs<'_>,
    resolution: &Resolution,
    output: mortar_core::demands::GlobalFlags,
) -> mortar_core::demands::GlobalFlags {
    use mortar_core::demands::GlobalFlags;
    use mortar_core::resolution::{Def, InternalDef};
    if output.contains(GlobalFlags::GOT_BASE) {
        return output;
    }
    let defined = resolution.def.iter().any(|(s, d)| {
        matches!(d, Def::Internal(InternalDef::Provide { conditional: false }))
            && inputs.symbols.get(s).is_some_and(|k| k.name == b"_GLOBAL_OFFSET_TABLE_")
    });
    if defined { output | GlobalFlags::GOT_BASE } else { output }
}

/// A dynamic output's synthetic chunk: its kind and size, and for a copy
/// chunk (`.dynbss`, `.bss.rel.ro`, whose header no plan fixes, D-42) its
/// alignment (log2).
type DynChunk = (SynthKind, u64, Option<u8>);

/// The synthetic chunks of an output with `.dynamic` (S-3, D-30 to D-42):
/// none in a static link.
fn dyn_chunks(
    d: Option<&crate::dynamic::DynTables>,
    rows: &[mortar_layout::chunk::CopyRow],
) -> Result<Vec<DynChunk>, LayoutDiag> {
    let Some(d) = d else { return Ok(Vec::new()) };
    // D-1: no `.interp` in a `StaticPie`.
    let mut v: Vec<DynChunk> =
        d.interp.iter().map(|i| (SynthKind::Interp, i.size(), None)).collect();
    v.extend(d.sizes.iter().map(|&(k, size)| (k, size, None)));
    if d.rela.size() > 0 {
        v.push((SynthKind::RelaDyn, d.rela.size(), None));
    }
    v.push((SynthKind::Dynamic, d.dynamic.size(), None));
    for (kind, place) in [
        (SynthKind::DynBss, mortar_core::CopyPlace::DynBss),
        (SynthKind::BssRelRo, mortar_core::CopyPlace::RelRo),
    ] {
        let (_, size, align) = mortar_layout::chunk::copy_extent(rows, place)?;
        if size > 0 {
            v.push((kind, size, Some(align)));
        }
    }
    Ok(v)
}

/// The synthetic chunks that exist, in S-3 order, as layout takes them,
/// and their sizes.
fn synth_chunks(
    p: &SynthPlans,
    dynamic: &[DynChunk],
    eh: &EhPlan,
    shndx: bool,
    class: Class,
    rel: bool,
) -> (Vec<SynthChunk>, Vec<(SynthKind, u64)>) {
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
        if shndx {
            present.push((SynthKind::SymtabShndx, s.shndx_size()));
        }
        present.push((SynthKind::Strtab, s.strtab_size()));
    }
    present.push((SynthKind::Shstrtab, p.shstrtab.size()));
    // S-100: mortar's `.comment` piece, after the inputs' strings.
    present.push((SynthKind::Comment, mortar_synth::COMMENT.len() as u64));
    present.extend(dynamic.iter().map(|d| (d.0, d.1)));
    present.sort_by_key(|c| c.0);
    let copy_align = |kind: SynthKind| dynamic.iter().find(|d| d.0 == kind).and_then(|d| d.2);
    let chunks = present
        .iter()
        .map(|&(kind, size)| {
            let synth = kind.synth(rel);
            // D-42: a copy chunk is NOBITS, writable, aligned as its
            // allocations.
            if let Some(align) = copy_align(kind) {
                let flags = mortar_elf::consts::SHF_ALLOC | mortar_elf::consts::SHF_WRITE;
                let sh_type = mortar_elf::consts::SHT_NOBITS;
                return SynthChunk { synth, sh_type, flags, align, size };
            }
            let h =
                kind.header(class, rel).unwrap_or_else(|| unreachable!("a chunk with a header"));
            let (sh_type, align) = match kind {
                SynthKind::EhFrame => (eh.sh_type(), eh.align()),
                _ => (h.sh_type, h.align.trailing_zeros() as u8),
            };
            SynthChunk { synth, sh_type, flags: h.flags, align, size }
        })
        .collect();
    (chunks, present)
}

/// The output section names before layout (S-90's set): every output
/// section with a live member, the synthetic chunks that exist, and
/// layout's `.relro_padding`. An estimate; [`front`] runs layout again in
/// the rare case it differs from the placement's names.
fn estimated_names<'n>(
    a: &Assignment<'n>,
    liveness: &Liveness,
    resolution: &Resolution,
    synthetic: &[SynthKind],
    relro: bool,
    rel: bool,
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
    for k in synthetic {
        // `.dynbss` is a chunk of `.bss` (D-42, D-60).
        names.push(mortar_layout::rank::home(k.name(rel)));
    }
    // layout's `.relro_padding` (L-64), when RELRO has content: a static
    // executable's `.got` always does.
    if relro {
        names.push(mortar_core::layout::Synth::RELRO_PADDING.name);
    }
    // A name shared by an input output section and a synthetic chunk that
    // joins it (`.eh_frame`, `.got`) is one section.
    names.sort_unstable();
    names.dedup();
    names
}

/// The output section names of a placement, in section index order.
fn placed_names<'n>(placed: &Placed, a: &Assignment<'n>) -> Vec<&'n [u8]> {
    let mut rows: Vec<(u32, &'n [u8])> = Vec::new();
    for (_, c) in placed.placement.chunks.iter() {
        let Some(shndx) = c.shndx else { continue };
        let name: &'n [u8] = match c.kind {
            ChunkKind::Synthetic(s) => mortar_layout::rank::home(s.name),
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

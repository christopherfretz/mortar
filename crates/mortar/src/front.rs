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
use mortar_core::layout::{ChunkKind, Folds, Layout, Placement, Slots};
use mortar_core::par::Pool;
use mortar_core::plan::{self, Plan, Strip};
use mortar_core::resolution::{Resolution, SectionKind};
use mortar_core::{Liveness, SymbolId};
use mortar_diag::{Collector, Diagnostic, Key, Kind, Place};
use mortar_elf::consts::SHT_NOTE;
use mortar_elf::{Class, SectionIndex};
use mortar_input::load::{LoadEnv, load_steps_with};
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
    /// `.mason.meta` kept for tools (`spec/meta.md` MT-32): with
    /// `--keep-mason-meta` and some live unit.
    pub mason_meta: Option<mortar_synth::MasonMetaPlan>,
    /// `.gdb_index` (`spec/synth.md` §17): with `--gdb-index` and some
    /// unit in the output's `.debug_info`.
    pub gdb_index: Option<mortar_synth::GdbIndexPlan>,
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
    /// Keep the build-id leaves for the link state (`spec/incr.md` IN-21);
    /// set by the driver in incremental mode only.
    pub keep_leaves: bool,
    pub commons: Vec<Common>,
    pub layout: Layout,
    /// What `place` returned besides the placement (the segment rules
    /// `Layout::validate` used, the rounds of the fixed point); its
    /// `placement` is empty, moved into `layout`.
    pub placed: Placed,
    /// The erratum fixes (`spec/a64.md` §11.2): words written over fill's
    /// bytes, by address, sorted (`LinkSites::erratum_fixes`).
    pub erratum_fixes: Vec<mortar_image::fill::Fixup>,
    /// What a format-5 capture reads beyond format 4's (WP-244), each
    /// from its one producer; `None` unless [`Env::state5`].
    pub incr5: Option<Incr5>,
}

/// The data of a format-5 capture that the link computes on its way
/// (WP-244; `spec/incr.md` IN-18e, WP-239 S4-1..S4-5, R-4a): made only
/// in an incremental-mode link with [`Env::state5`].
#[derive(Debug, Clone)]
pub struct Incr5 {
    /// Per file, the OR of the scan's demands per key over its live
    /// units (`mortar_scan::scan_recording`).
    pub demands: Vec<mortar_core::incr::FileDemands>,
    /// Per `SymbolId`: `SymFacts::pack(Walker::key_facts(id))`.
    pub facts: Vec<u32>,
    /// Each file's `.eh_frame` range in the final plan
    /// (`EhPlan::file_runs`), or why there is none to store.
    pub eh: Result<Vec<mortar_synth::eh::EhRun>, String>,
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
        let syms = CoreSyms::new(&self.inputs, &self.resolution, &self.assignment);
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
#[derive(Clone, Copy)]
pub struct Env<'e> {
    pub cwd: &'e Path,
    pub pool: Pool,
    /// The replay hook of `spec/incr.md` IN-61 (tests only; `None` in every
    /// link the command line runs): given the link's tables and its
    /// computed layout, the decision tables to write instead, put in place
    /// before `Layout::validate`.
    pub replay: Option<&'e Replay<'e>>,
    /// An incremental-mode link (`spec/incr.md` IN-30 to IN-34a, IN-50):
    /// layout takes IN-33's slack and `.eh_frame` its per-file CIEs.
    /// `false` in a plain link, which pays nothing for it (L-87).
    pub slack: bool,
    /// With `slack`: the link also computes what a format-5 capture
    /// reads ([`Incr5`]: per-file demand, facts) and gives each file's
    /// `.eh_frame` records a range with `Slack::eh_pad` bytes after them
    /// (WP-244). The join (U-10) makes it the incremental-mode default;
    /// until then only `mortar_incr::testhook::state5_path` and the
    /// tests set it.
    pub state5: bool,
}

/// The replay hook's function (IN-61).
pub type Replay<'e> =
    dyn Fn(&mortar_incr::capture::Link<'_, '_>, &Layout) -> Result<Layout, String> + 'e;

impl std::fmt::Debug for Env<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Env")
            .field("cwd", &self.cwd)
            .field("pool", &self.pool)
            .field("replay", &self.replay.is_some())
            .field("slack", &self.slack)
            .field("state5", &self.state5)
            .finish()
    }
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

/// The line's `--version-script` files read (relative to `cwd`), parsed
/// as one script and compiled (`mortar_resolve::version`); `Ok(None)`
/// without one or outside a `Shared` output (the command line refuses
/// the option there, `spec/dyn.md` D-114). `Err(())` (diagnostics
/// emitted) when a file cannot be read or the script is refused.
fn version_script(
    plan: &Plan,
    cwd: &Path,
    diag: &Collector,
) -> Result<Option<std::sync::Arc<mortar_resolve::version::VersionScript>>, ()> {
    if plan.kind != plan::OutputKind::Shared || plan.version_scripts().next().is_none() {
        return Ok(None);
    }
    let texts = read_scripts(plan.version_scripts(), cwd, diag)?;
    let files: Vec<mortar_resolve::version::SourceFile<'_>> = texts
        .iter()
        .map(|(path, bytes)| mortar_resolve::version::SourceFile { path, bytes })
        .collect();
    loaded(mortar_resolve::version::VersionScript::load(&files), diag)
}

/// The line's `--dynamic-list` files as one list (`spec/dyn.md` D-112,
/// `spec/resolve.md` §9.6; WP-128), in every output kind that accepts
/// the option.
fn dynamic_list(
    plan: &Plan,
    cwd: &Path,
    diag: &Collector,
) -> Result<Option<std::sync::Arc<mortar_resolve::version::DynamicList>>, ()> {
    if plan.dynamic_lists().next().is_none() {
        return Ok(None);
    }
    let texts = read_scripts(plan.dynamic_lists(), cwd, diag)?;
    let files: Vec<mortar_resolve::version::SourceFile<'_>> = texts
        .iter()
        .map(|(path, bytes)| mortar_resolve::version::SourceFile { path, bytes })
        .collect();
    loaded(mortar_resolve::version::DynamicList::load(&files), diag)
}

/// Reads each of `scripts`' files: `(path as shown, bytes)`, or
/// `cannot-open-file` for each that does not open.
fn read_scripts<'p>(
    scripts: impl Iterator<Item = &'p plan::ScriptArg>,
    cwd: &Path,
    diag: &Collector,
) -> Result<Vec<(String, Vec<u8>)>, ()> {
    let mut texts: Vec<(String, Vec<u8>)> = Vec::new();
    let mut failed = false;
    for s in scripts {
        let shown = s.path.to_string_lossy().into_owned();
        match std::fs::read(cwd.join(&s.path)) {
            Ok(bytes) => texts.push((shown, bytes)),
            Err(e) => {
                failed = true;
                diag.emit(
                    Key::new(20, 0, u64::from(s.arg)),
                    Diagnostic::error(Kind::CannotOpenFile {
                        path: shown,
                        reason: mortar_diag::os_reason(&e),
                    }),
                );
            }
        }
    }
    if failed { Err(()) } else { Ok(texts) }
}

/// A compiled script, or its diagnostics emitted.
fn loaded<T>(
    got: Result<T, Vec<Diagnostic>>,
    diag: &Collector,
) -> Result<Option<std::sync::Arc<T>>, ()> {
    match got {
        Ok(v) => Ok(Some(std::sync::Arc::new(v))),
        Err(ds) => {
            for (i, d) in ds.into_iter().enumerate() {
                diag.emit(Key::new(30, 0, i as u64), d);
            }
            Err(())
        }
    }
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
    let embedded = crate::odr_runtime::embedded(&plan);
    let inputs = load_steps_with(&plan, &load_env, store, diag, &mut steps, embedded);
    if time_steps {
        print_steps("input", &steps.times, &steps.counts);
    }
    let inputs = inputs?;
    timer.phase(20, "input");
    // `--odr-runtime` on a machine the runtime is not linked for.
    if let Some((k, d)) = crate::odr_runtime::refuse_target(&plan, inputs.target.machine) {
        diag.emit(k, d);
        return None;
    }
    let pool = capped(pool, &plan, store.stats().resident);
    let machine = inputs.target.machine;
    // 30: the version script (`spec/script.md` §9; resolve.md §9.7,
    // dyn.md D-114; WP-125), then resolve.
    let versions = version_script(&plan, env.cwd, diag).ok()?;
    let dynamic_list = dynamic_list(&plan, env.cwd, diag).ok()?;
    let outcome =
        mortar_resolve::resolve_inputs_with(&inputs, &plan, &pool, versions.clone(), dynamic_list);
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
    // A dynamic output's scope (O-24: its `DT_NEEDED`) and kind: an
    // executable, or a shared object (O-60, WP-129).
    let odr_output = match plan.kind {
        plan::OutputKind::Executable | plan::OutputKind::PieExecutable => {
            Some(mortar_odr::Output { needed: &outcome.needed, executable: true })
        }
        plan::OutputKind::Shared => {
            Some(mortar_odr::Output { needed: &outcome.needed, executable: false })
        }
        _ => None,
    };
    let odr =
        mortar_odr::check(&inputs, &outcome.resolution, &plan.diagnostics.odr, odr_output, &pool);
    // `spec/dyn.md` D-117 with hod's `reloc-model` row (`spec/meta.md`
    // MT-26; WP-129): in every `--odr` mode, a refusal, not an ODR check.
    if plan.kind == plan::OutputKind::Shared {
        for (k, d) in mortar_odr::reloc_model::check(&inputs, &outcome.resolution) {
            diag.emit(k, d);
        }
    }
    let odr_table = odr.table;
    let odr_full = odr.full;
    // meta.md MT-32 (WP-076): the live inputs' units, kept for tools.
    let kept_meta = plan
        .diagnostics
        .odr
        .keep_meta
        .then(|| mortar_odr::kept_units(&inputs, &outcome.resolution, &pool))
        .filter(|b| !b.is_empty())
        .map(mortar_synth::MasonMetaPlan::new);
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
        Ok(mut o) => {
            if env.slack {
                o.slack = mortar_layout::slack::Slack::INCREMENTAL;
            }
            o
        }
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
    let mut order = offsets(&pool, &merge.split, &merge.meta, &liveness.frag_live);
    timer.phase(50, "liveness");

    // 70: scan. WP-177: the owners of `.eh_frame` and
    // `.gcc_except_table` fields, asked only about references to dead
    // targets there.
    let eh_owners = crate::eh_owners::Owners::new(&merge.eh, &merge.eh_relocs, &liveness);
    let output = output_kind(plan.kind);
    let scan_inputs = ScanInputs {
        inputs: &inputs,
        resolution: &resolution,
        liveness: &liveness,
        assignment: Some(&assignment),
        comdat_mismatch: &comdat_mismatch,
        gnu_warnings: &gnu_warnings,
        closure_only: &closure_only,
        eh: Some(&eh_owners),
    };
    // D-25: a writable place, from the section mapping.
    let out_writable = crate::dynlink::OutWritable(&assignment);
    let sections: Option<&dyn mortar_scan::weak::SectionMap> =
        if dynamic { Some(&out_writable) } else { None };
    let scan_options = ScanOptions { output, dynamic, sections, proof: link_arch.proof() };
    // WP-244: the per-file demand of a format-5 capture (the same walk,
    // a recording sink; a plain link takes `scan`, unchanged).
    let state5 = env.slack && env.state5;
    let (mut scanned, file_demands) = if state5 {
        let (s, d) = mortar_scan::scan_recording(arch, &scan_inputs, &scan_options, &pool);
        (s, Some(d))
    } else {
        (scan(arch, &scan_inputs, &scan_options, &pool), None)
    };
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
    // S-49: the decision (`Slots.eh_records`), then the plan of that list:
    // the one the image writes unless a replay's list differs (rebuilt
    // after the layout, WP-086).
    // spec/incr.md IN-34a: CIEs de-duplicated per file in incremental mode.
    let live = |s: SectionId| liveness.is_live(s);
    let eh_list = EhPlan::select_in(&merge_eh, live, env.slack, &pool, diag).ok()?;
    let mut eh = EhPlan::from_records(&merge_eh, &eh_list, &pool, diag).ok()?;
    // WP-244 (IN-34a, IN-72 item 5): each file's records a range of
    // `size + Slack::eh_pad(size)` bytes, the pad a padding CIE.
    let mut eh_runs: Result<(), String> = Ok(());
    if state5 {
        match eh.file_runs(&merge_eh) {
            Ok(runs) => {
                let pad = |size| mortar_layout::slack::Slack::INCREMENTAL.eh_pad(size);
                match EhPlan::run_pads(&runs, pad) {
                    Some(pads) if !pads.is_empty() => {
                        let ra = padding_ra(machine);
                        eh = EhPlan::from_records_padded(
                            &merge_eh, &eh_list, &pads, ra, &pool, diag,
                        )
                        .ok()?;
                    }
                    Some(_) => {}
                    None => eh_runs = Err("the .eh_frame runs are not contiguous".to_owned()),
                }
            }
            Err(file) => {
                eh_runs = Err(format!("file {} has two runs of .eh_frame records", file.0));
            }
        }
    }
    drop(eh_list);
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
    // WP-239 R-4a: each key's facts, packed, as `RESOLVED.facts` stores
    // them (the walker's per-key facts, the one producer).
    let facts5 = if state5 {
        let n = inputs.symbols.len() as usize;
        pool.map(n, |i| walker.key_facts(SymbolId(i as u32)).pack())
    } else {
        Vec::new()
    };
    let (got, dyn_tables, placeless) = if has_dynamic {
        let src =
            crate::dynlink::DynSource::new(&inputs, &resolution, &assignment, &liveness, &plan);
        // resolve.md §10.1 P-1 (WP-136): the exports `.dynsym` omits, for
        // the layout's check of its entries.
        let placeless = crate::dynlink::placeless_exports(&src, &resolution, &scanned.demands);
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
            versions: versions.as_deref(),
        };
        let (got, mut tables) = crate::dynlink::plan(&t, &mut slots, diag)?;
        // O-50 to O-52: the table, once `.dynsym`'s order is known.
        // WP-198: and the runtime's note after them (`spec/odr.md` §10.2).
        if let Some(rows) = &odr_table {
            let runtime = crate::odr_runtime::mode_word(plan.diagnostics.odr.runtime).zip(
                crate::odr_runtime::runtime_id(machine, plan.kind == plan::OutputKind::Shared),
            );
            tables.note_mason =
                Some(note_mason(&inputs, rows, odr_full.as_ref(), runtime, &slots.dynsym));
        }
        (got, Some(tables), placeless)
    } else {
        (GotPlan::new(arch, &slots, got_output, variant, diag).ok()?, None, Vec::new())
    };
    let build_id = BuildIdPlan::new(plan.shaping.build_id.as_ref(), diag).ok()?;
    steps.step("property got build-id", 0, "");
    let core = CoreSections { resolution: &resolution, liveness: &liveness, split: &merge.split };
    let sections = LinkSections { core, assignment: &assignment };
    let syms = CoreSyms::new(&inputs, &resolution, &assignment);
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
    // spec/synth.md S-G1 (WP-097): `.gdb_index` only with `--gdb-index`;
    // without it nothing is read (S-109).
    let gdb_index = if plan.shaping.gdb_index == Some(true) {
        let in_output = |s: SectionId| {
            liveness.is_live(s)
                && resolution
                    .sections
                    .get(s)
                    .is_some_and(mortar_core::resolution::SectionRow::is_kept)
                && assignment.out(s).is_some()
        };
        let consumed = |s: SectionId| {
            resolution.sections.get(s).is_some_and(|r| {
                r.state
                    == mortar_core::resolution::SectionState::Dropped(
                        mortar_core::resolution::DropReason::Consumed,
                    )
            })
        };
        let src = mortar_synth::gdbread::Source {
            link: &tables,
            relocs: &ArchRelocs(arch),
            in_output: &in_output,
            consumed: &consumed,
            word: target.class.word_size() as u8,
        };
        match mortar_synth::GdbIndexPlan::new(&src, &pool) {
            Ok(p) => p,
            Err(e) => {
                diag.emit(mortar_synth::error::key(0), gdb_index_diag(&inputs, e));
                return None;
            }
        }
    } else {
        None
    };
    steps.step("gdb index plan", 0, "");
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
        mason_meta: kept_meta,
        gdb_index,
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
    // meta.md MT-32 (WP-076): an input section named `.mason.meta` that
    // is not a container is copied (MT-6) and would join the kept units'
    // section, which a reader would then find malformed: refused by name.
    if plans.mason_meta.is_some()
        && copied_section(&assignment, &liveness, SynthKind::MasonMeta.name(false))
    {
        diag.emit(
            Key::new(80, 0, 1),
            Diagnostic::refused(
                mortar_diag::RefusalClass::Feature,
                "--keep-mason-meta with an input section named .mason.meta that is not a \
                 container (it would be copied into the kept section)",
            )
            .note("meta.md MT-6, MT-32: the section is copied as any other; its warning names it"),
        );
        return None;
    }
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
    let (placement_assignment, placed, layout, computed_fixes) = loop {
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
            output,
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
            synth_holes: &eh_holes(&eh),
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
        let exact_names = placed_names(&placed.placement, &a);
        let exact = ShstrtabPlan::new(&exact_names);
        let mut again = exact.size() != shstrtab.size();
        shstrtab = exact;
        // The headers: the null one and one per output section.
        let exact_shndx = SymtabPlan::needs_shndx(exact_names.len() as u32 + 1);
        again |= exact_shndx != shndx;
        shndx = exact_shndx;
        if planned_for.as_deref().unwrap_or(&[]) != &placed.placement.patches[..]
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
        // a64.md §11.2: the erratum fixes at the computed placement, while
        // it is the only copy held (made again below from a replay's layout:
        // made after the layout is, they cost a plain link's peak 1.3 MiB
        // on cxx-medium-g-static, WP-086).
        let fixes = match &kept_sites {
            Some((_, s)) => s.erratum_fixes(&placed.placement, &pool),
            None => Ok(Vec::new()),
        };
        steps.step("erratum fixes", 0, "");
        // The exact names, in section index order (S-90: `sh_name` of
        // section `n` is the plan's `n`-th name); their size is the one
        // placed.
        plans.shstrtab = shstrtab.clone();
        // The placement moves into the layout: one copy is held, not two
        // (`Front.placed` keeps the rest; its `placement` is left empty).
        let mut placed = placed;
        let placement = std::mem::take(&mut placed.placement);
        let layout = Layout { folds: folds.clone(), merge: order.map.clone(), slots, placement };
        steps.step("layout clone", 0, "");
        break (a, placed, layout, fixes);
    };
    // X86-45, X86-38: an ELFCLASS32 TLS block of 2^31 bytes or more would
    // put a TP offset outside s(32); refused by name, as L-73 refuses an
    // end beyond 2^32.
    if target.class == Class::Elf32
        && let Some((_, tls)) =
            layout.placement.segments.iter().find(|(_, g)| g.p_type == mortar_elf::consts::PT_TLS)
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
            layout.placement.chunks.get(id).map(|c| c.kind),
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
        needs_entry: layout.placement.entry.is_some(),
        segment_rules: placed.rules,
        resolution: &resolution,
        assignment: &placement_assignment,
        split: &merge.split,
        liveness: &liveness,
        demands,
        shapes: &shapes,
        shared: plan.kind == plan::OutputKind::Shared,
        placeless: &placeless,
    };
    steps.step("checks", 0, "");
    // IN-61: the replay's decision tables in place of the computed ones.
    let replayed = env.replay.is_some();
    let layout = match env.replay {
        None => layout,
        Some(replay) => {
            let link = mortar_incr::capture::Link {
                inputs: &inputs,
                resolution: &resolution,
                assignment: &placement_assignment,
                split: &merge.split,
            };
            match replay(&link, &layout) {
                Ok(l) => l,
                Err(e) => {
                    diag.emit(Key::new(90, 0, u64::MAX), internal(format!("replay: {e}")));
                    return None;
                }
            }
        }
    };
    // The sites read the slots' lists and the merged map: a layout whose
    // are not theirs needs them made again for the erratum fixes, and so
    // the branch candidates; else those go now, before the checks, and the
    // sites too when no replay's placement needs fixes of its own.
    let sites_stale = !arch.errata().is_empty()
        && (!same_slot_lists(&layout.slots, demand_slots) || layout.merge != order.map);
    let branch_candidates = if sites_stale { branch_candidates } else { Vec::new() };
    let kept_sites = if replayed { kept_sites } else { None };
    let validated = layout.validate(&cx);
    steps.step("validate", 0, "");
    if let Err(v) = validated {
        diag.emit(
            Key::new(90, 0, u64::MAX),
            internal(format!("the layout breaks its rules: {v:?}")),
        );
        return None;
    }
    // WP-086 (`spec/image.md` I-2): the plans the image writes from, as
    // functions of the final layout. Built on the one path a plain link
    // takes, except where that would cost it a second build: there the
    // plan is compared with the layout's table and rebuilt when they
    // differ (only a replay's tables can).
    //
    // `.eh_frame`, `.eh_frame_hdr` and `.symtab` (S-49, S-76: the symbol
    // table reads the `.eh_frame` plan), compare-then-rebuild: a second
    // symbol table plan is a cost.
    let pads = eh_pads(&layout);
    if layout.slots.eh_records[..] != *eh.records() || pads[..] != *eh.pads() {
        let ra = padding_ra(machine);
        let records = &layout.slots.eh_records;
        eh = EhPlan::from_records_padded(&merge_eh, records, &pads, ra, &pool, diag).ok()?;
        plans.hdr = HdrPlan::new(&merge_eh, &eh, eh_frame_hdr, &pool, diag).ok()?;
        if plans.symtab.is_some() {
            let cx = SymCtx {
                syms: &syms,
                sections: &sections,
                eh: Some((&eh, &merge_eh)),
                discard: plan.shaping.discard,
            };
            let patches = &layout.placement.patches;
            plans.symtab =
                Some(SymtabPlan::new(&cx, target.class, patches, facts, &pool, diag).ok()?);
            planned_for = Some(patches.clone());
        }
        steps.step("eh plan from the layout", 0, "");
    }
    // `.symtab`'s patch rows (S-74), from the final placement: the
    // layout loop's replan step.
    if planned_for.as_deref().unwrap_or(&[]) != &layout.placement.patches[..]
        && let Some(old) = &plans.symtab
    {
        plans.symtab = Some(old.with_patches(&syms, &layout.placement.patches));
    }
    // S-88: the layout's spans are the plan's (format version 1 admits no
    // other value; IN-49a).
    let spans = match &plans.symtab {
        Some(s) => s.check_spans(&layout.slots.symtab_locals),
        None => mortar_synth::symtab::check_no_spans(&layout.slots.symtab_locals),
    };
    if let Err(e) = spans {
        diag.emit(Key::new(90, 0, u64::MAX), e.into_diagnostic());
        return None;
    }
    // `.shstrtab` (S-90): the names of the final placement's chunks, in
    // section index order. One path.
    plans.shstrtab = ShstrtabPlan::new(&placed_names(&layout.placement, &placement_assignment));
    // `.got` (S-64) of a static link: the final layout's slot lists. One
    // path. (An output with `.dynamic`, a `StaticPie` among them, has
    // `dynlink`'s, WP-066.)
    let final_got = if has_dynamic {
        None
    } else {
        Some(GotPlan::new(arch, &layout.slots, got_output, variant, diag).ok()?)
    };
    // a64.md §11.2: the erratum fixes, from the words fill writes at the
    // final layout's addresses. A plain link's layout is the computed one
    // and so are its fixes (above); a replay's are made again from its
    // placement. The sites read the slots' lists and the merged map, so
    // they are made again too when the layout's are not theirs
    // (compare-then-rebuild: a second walk of the code is a cost).
    let fixes = if !replayed {
        computed_fixes
    } else if sites_stale {
        let ids = kept_sites.as_ref().map(|(i, _)| *i).unwrap_or_default();
        let site_inputs = SiteInputs {
            resolution: &resolution,
            assignment: &assignment,
            headers: &headers,
            liveness: &liveness,
            split: &merge.split,
            merge: &layout.merge,
            demands: &scanned.demands,
            order: &scanned.order,
            slots: &layout.slots,
            got: final_got.as_ref().unwrap_or(&plans.got),
            ids,
            output,
        };
        match LinkSites::new(site_inputs, &branch_candidates, arch, &walker, &pool) {
            Ok(s) => s.erratum_fixes(&layout.placement, &pool),
            Err((k, d)) => {
                diag.emit(k, d);
                return None;
            }
        }
    } else {
        match &kept_sites {
            Some((_, s)) => s.erratum_fixes(&layout.placement, &pool),
            None => Ok(Vec::new()),
        }
    };
    if replayed {
        steps.step("erratum fixes from the layout", 0, "");
    }
    let erratum_fixes = match fixes {
        Ok(f) => f,
        Err(d) => {
            let (k, d) = layout_diag(&inputs, &resolution, d, Key::new(90, 0, u64::MAX - 1));
            diag.emit(k, d);
            return None;
        }
    };
    drop(kept_sites);
    drop(branch_candidates);
    if let Some(g) = final_got {
        plans.got = g;
    }
    // The merged chunks' order (S-24) follows `layout.merge`,
    // compare-then-rebuild: a second sort of the fragments is a cost.
    if layout.merge != order.map {
        order.follow(layout.merge.clone(), &pool);
    }
    // Every plan's size is its chunk's (`Slots.sizes`, from the plans the
    // layout was made with).
    let (_, present) = synth_chunks(&plans, &dyn_chunks, &eh, shndx, target.class, rel);
    if chunk_ids(FIRST_SYNTHETIC, &present).as_deref() != Some(&layout.slots.sizes[..]) {
        diag.emit(
            Key::new(90, 0, u64::MAX),
            internal("a synthetic plan made from the layout's tables is not its chunk's size"),
        );
        return None;
    }
    steps.step("plans from the layout", 0, "");
    timer.phase(90, "layout");
    deferred.disarm();
    drop(walker);
    let incr5 = file_demands.map(|demands| Incr5 {
        demands,
        facts: facts5,
        eh: eh_runs.and_then(|()| {
            eh.file_runs(&merge_eh).map_err(|f| format!("file {} has two .eh_frame runs", f.0))
        }),
    });
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
        keep_leaves: false,
        commons,
        layout,
        placed,
        erratum_fixes,
        incr5,
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
    mortar_diag::stream::to_stderr(&s);
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

/// Whether two `Slots` have the same slot lists (what `GotPlan` and the
/// sites read).
fn same_slot_lists(a: &Slots, b: &Slots) -> bool {
    a.got == b.got
        && a.plt == b.plt
        && a.iplt == b.iplt
        && a.tls_desc == b.tls_desc
        && a.tls_gd == b.tls_gd
        && a.tls_ie == b.tls_ie
        && a.tls_ld == b.tls_ld
}

/// `Slots` from `Demands` (S-64): each kind's keys in `SymKey` order.
/// The demand bits that make a slot of `kind`: the one mapping of the
/// slot lists ([`slots_of`]) and of format 5's `SLOTS.refs` (WP-244).
#[must_use]
pub fn slot_demand(kind: mortar_core::layout::SlotKind) -> DemandFlags {
    use mortar_core::layout::SlotKind;
    match kind {
        SlotKind::Got => DemandFlags::GOT,
        SlotKind::Plt => DemandFlags::PLT | DemandFlags::CANON_PLT,
        SlotKind::Iplt => DemandFlags::IPLT,
        SlotKind::TlsDesc => DemandFlags::TLS_DESC,
        SlotKind::TlsGd => DemandFlags::TLS_GD,
        SlotKind::TlsIe => DemandFlags::GOT_TPREL,
    }
}

fn slots_of(d: &mortar_core::Demands, resolution: &Resolution) -> Slots {
    use mortar_core::layout::SlotKind;
    let list = |k: SlotKind| -> Box<[SymKey]> { d.keys_with(slot_demand(k)).collect() };
    Slots {
        got: list(SlotKind::Got),
        plt: list(SlotKind::Plt),
        iplt: list(SlotKind::Iplt),
        tls_desc: list(SlotKind::TlsDesc),
        tls_gd: list(SlotKind::TlsGd),
        tls_ie: list(SlotKind::TlsIe),
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

/// The `.note.mason` table (`spec/odr.md` O-50 to O-52): each row's
/// `.dynsym` index is its symbol's place in `dynsym` plus one (entry 0 is
/// the null symbol). A row whose symbol is not in `.dynsym` has no index
/// and is not written (O-50's rows are exported, so in `.dynsym`, D-21).
/// With `=full` its second note follows the first in the same section
/// (O-52, WP-076); both are a multiple of 8 bytes long. With
/// `--odr-runtime`, the runtime's note is last (WP-198; 40 bytes).
fn note_mason(
    inputs: &Inputs<'_>,
    rows: &[mortar_odr::TableRow],
    full: Option<&mortar_odr::FullRows>,
    runtime: Option<(u32, u32)>,
    dynsym: &[SymbolId],
) -> mortar_synth::NoteMasonPlan {
    let mut index: Vec<(SymbolId, u32)> =
        dynsym.iter().enumerate().map(|(i, &s)| (s, i as u32 + 1)).collect();
    index.sort_unstable();
    let table: Vec<mortar_odr::NoteRow> = rows
        .iter()
        .filter_map(|r| {
            let at = index.binary_search_by_key(&r.sym, |e| e.0).ok()?;
            let name = inputs.symbols.get(r.sym)?.name;
            Some(r.row(name, index[at].1))
        })
        .collect();
    let mut bytes = mortar_odr::table_note(&table);
    if let Some(f) = full {
        bytes.extend(mortar_odr::full_note(&f.definitions, &f.types));
    }
    // WP-198: `--odr-runtime`'s note (mode, runtime id), 40 bytes.
    if let Some((mode, id)) = runtime {
        bytes.extend(mortar_odr::runtime_note(mode, id));
    }
    mortar_synth::NoteMasonPlan::new(bytes)
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
    if let Some(n) = &d.note_mason {
        v.push((SynthKind::NoteMason, n.size(), None));
    }
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

/// The padding of `eh` as holes of the `.eh_frame` chunk (`spec/incr.md`
/// IN-34a).
fn eh_holes(eh: &EhPlan) -> Vec<mortar_layout::chunk::SynthHole> {
    let synth = SynthKind::EhFrame.synth(false);
    eh.pads()
        .iter()
        .map(|&(offset, size)| mortar_layout::chunk::SynthHole {
            synth,
            offset,
            size,
            kind: mortar_core::layout::HoleKind::EhPad,
        })
        .collect()
}

/// The `.eh_frame` chunk's holes in `layout` (IN-34a), as `EhPlan` pads.
fn eh_pads(layout: &Layout) -> Vec<(u64, u64)> {
    let synth = SynthKind::EhFrame.synth(false);
    let p = &layout.placement;
    p.holes
        .iter()
        .filter(|h| matches!(p.chunks[h.chunk].kind, ChunkKind::Synthetic(s) if s == synth))
        .map(|h| (h.offset, h.size))
        .collect()
}

/// A padding CIE's return-address register (IN-34a).
fn padding_ra(machine: mortar_elf::Machine) -> u8 {
    match machine {
        mortar_elf::Machine::AArch64 => 30,
        mortar_elf::Machine::X86_64 => 16,
        mortar_elf::Machine::I386 => 8,
    }
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
    if let Some(m) = &p.mason_meta {
        present.push((SynthKind::MasonMeta, m.size()));
    }
    if let Some(g) = &p.gdb_index {
        present.push((SynthKind::GdbIndex, g.size()));
    }
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
/// layout's `.pad.relro`. An estimate; [`front`] runs layout again in
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
    // layout's `.pad.relro` (L-64), when RELRO has content: a static
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

/// Whether an output section named `name` has a live input section.
fn copied_section(a: &Assignment<'_>, liveness: &Liveness, name: &[u8]) -> bool {
    a.out_of.iter().any(|(sid, out)| {
        out.out().is_some_and(|o| a.outsecs[o].name == name) && liveness.is_live(sid)
    })
}

/// The diagnostic of a `.gdb_index` reader's error (`spec/synth.md`
/// §17.6): a refusal by name at the file (and section), or the input
/// error.
fn gdb_index_diag(inputs: &Inputs<'_>, e: mortar_synth::gdbread::ReadError) -> Diagnostic {
    use mortar_synth::gdbread::ReadError;
    let file = &inputs.files[e.file()];
    let section_name = |index: SectionIndex| match file.reader {
        Reader::Object(o) => o
            .section(index)
            .ok()
            .and_then(|s| s.name().ok())
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .unwrap_or_default(),
        _ => String::new(),
    };
    match e {
        ReadError::Refused { name, at, note, .. } => {
            let place = match at {
                Some((index, offset)) => {
                    Place::section(file.display(), section_name(index), offset)
                }
                None => Place::File(file.display()),
            };
            Diagnostic::refused(mortar_diag::RefusalClass::Feature, name).at(place).note(note)
        }
        ReadError::Input { file: id, section, what } => {
            let p =
                mortar_merge::Problem { file: id, section: section.0, piece: 0, offset: 0, what };
            p.diagnostic(file.display(), &section_name(section))
        }
    }
}

/// The output section names of a placement, in section index order.
fn placed_names<'n>(placement: &Placement, a: &Assignment<'n>) -> Vec<&'n [u8]> {
    let mut rows: Vec<(u32, &'n [u8])> = Vec::new();
    for (_, c) in placement.chunks.iter() {
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

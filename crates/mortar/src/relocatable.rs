//! The `-r` link (`spec/model.md` §4.6 MD-R1, `spec/layout.md` §24,
//! `spec/image.md` §17): the join of RL-2 (resolution), RL-3 (layout) and
//! RL-4 (the image), WP-187 (RL-7), in record `spec/landed/core/WP-181.md`
//! §D's order:
//!
//! 1. the inputs (phase 20), as a final link loads them;
//! 2. `mortar_resolve::relocatable::resolve` (phase 30), then the ODR
//!    check over the `-r`'s units (`spec/meta.md` MT-30a);
//! 3. MD-R1's liveness: every section of a live file that is not dropped;
//! 4. the `.eh_frame` split (phase 60; L-R5 splits nothing else);
//! 5. the two notes' sizes from `mortar_synth`'s plans (`rl-sizes` S16);
//! 6. `mortar_layout::relocatable::layout` (phase 90), `Layout::check`
//!    (L-R10) and its two added rules with the facts of the inputs;
//! 7. the maps, then `mortar_image::relocatable::write` (phase 100).
//!
//! A final link never comes here: `crate::invoke_with` dispatches on
//! `plan.kind` once, so it pays one comparison.
//!
//! The cut of RL-7 (record `spec/landed/layout/WP-154.md` §F) is C on
//! AArch64. What a later package lifts stays refused by name here:
//! kept section groups (RL-8) and the x86 targets (RL-9).

use std::borrow::Cow;
use std::path::Path;

use mortar_core::assignment::Assignment;
use mortar_core::check::CheckContext;
use mortar_core::check::relocatable::{validate_groups, validate_relocs};
use mortar_core::demands::Demands;
use mortar_core::id::{FileId, OutSecId, SectionId};
use mortar_core::inputs::RelocForm;
use mortar_core::inputs::{CopyError, Inputs};
use mortar_core::layout::{ChunkKind, SegmentRules};
use mortar_core::par::Pool;
use mortar_core::plan::Plan;
use mortar_core::resolution::{FileLive, Resolution, SectionKind};
use mortar_core::split::Split;
use mortar_core::{Liveness, RelocatableLayout};
use mortar_diag::{Collector, Diagnostic, Key, Place};
use mortar_elf::{Class, Machine, Object, Section, SectionIndex, SymbolIndex};
use mortar_image::relocatable::RelocatableLink;
use mortar_input::load::{LoadEnv, load_steps_with};
use mortar_input::store::Store;
use mortar_layout::headers::{HeaderShapes, Headers, gather};
use mortar_layout::relocatable::NoteSizes;
use mortar_layout::{Target, relocatable::reloc_entry_size};
use mortar_merge::eh::Target as RelocTarget;
use mortar_merge::phase::{ArchRelocs, Link, LinkSection, Tables};
use mortar_merge::problem::What;

use crate::arch::LinkArch;
use crate::front::Env;
use crate::timing::Timer;

/// The tables of a `-r` link, before the image (MD-R1).
pub struct RFront<'s> {
    pub plan: Plan,
    pub pool: Pool,
    pub arch: LinkArch,
    pub inputs: Inputs<'s>,
    pub resolution: Resolution,
    pub liveness: Liveness,
    pub split: Split,
    pub headers: Headers<'s>,
    pub layout: RelocatableLayout<'s>,
    /// RV-R3's names with no key (`undefined_without_key`).
    pub undefined: Vec<Box<[u8]>>,
    /// The command line's words, for the map's `arg` records.
    pub args: Vec<Vec<u8>>,
}

impl std::fmt::Debug for RFront<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RFront").finish_non_exhaustive()
    }
}

fn internal(what: impl Into<String>) -> Diagnostic {
    Diagnostic::error(mortar_diag::Kind::InternalError { what: what.into() })
}

/// Phase 60's view of a `-r` link: only the live `.eh_frame` inputs are
/// split (L-R5 merges nothing; L-R6 keeps `.eh_frame` record by record),
/// and they have no output section yet (the layout, which needs the
/// split, makes it): `out` is a placeholder phase 60 does not read for
/// `.eh_frame`.
struct EhOnly<'t, 's> {
    tables: Tables<'t, 's>,
}

impl<'s> Link<'s> for EhOnly<'_, 's> {
    fn files(&self) -> u32 {
        self.tables.files()
    }

    fn object(&self, file: FileId) -> Option<Object<'s>> {
        self.tables.object(file)
    }

    fn section_id(&self, file: FileId, index: SectionIndex) -> Option<SectionId> {
        self.tables.section_id(file, index)
    }

    fn section(&self, file: FileId, index: SectionIndex) -> Option<LinkSection> {
        let id = self.tables.resolution.section_id(file, index)?;
        let row = self.tables.resolution.sections[id];
        (row.is_kept() && row.kind == SectionKind::EhFrame).then_some(LinkSection {
            id,
            kind: row.kind,
            out: OutSecId(0),
            reloc: row.reloc,
        })
    }

    fn global(
        &self,
        file: FileId,
        index: SymbolIndex,
        first_global: u32,
    ) -> Result<RelocTarget, What> {
        self.tables.global(file, index, first_global)
    }

    fn bytes(&self, file: FileId, s: &Section<'s>) -> Result<Cow<'s, [u8]>, What> {
        self.tables.bytes(file, s)
    }

    fn class(&self) -> Class {
        self.tables.class()
    }

    fn form(&self) -> RelocForm {
        self.tables.form()
    }
}

/// A diagnostic at the first `SHT_GROUP` section of a file in the link,
/// when the inputs have one: writing section groups at `-r` is RL-8's
/// (L-R2, L-R3). The group section's own row is never `Kept` (a final
/// link consumes it), so the test is the file's liveness: any group of a
/// live file is refused, also a COMDAT copy that loses (RL-8 decides
/// which copies are written).
fn kept_group(inputs: &Inputs<'_>, resolution: &Resolution) -> Option<(Key, Diagnostic)> {
    let (file, index) = resolution.sections.iter().find_map(|(s, r)| {
        if r.kind != SectionKind::Group {
            return None;
        }
        let (file, index) = resolution.sh_index(s)?;
        (!matches!(resolution.file_live[file], FileLive::Dead)).then_some((file, index))
    })?;
    let f = &inputs.files[file];
    let mut d = mortar_diag::relocatable::refused_section_groups();
    if let mortar_core::inputs::Reader::Object(o) = f.reader
        && let Ok(sec) = o.section(index)
        && let Ok(name) = sec.name()
    {
        d = d.at(Place::section(f.display(), String::from_utf8_lossy(name), 0));
    }
    Some((Key::new(45, file.0, u64::from(index.0)), d))
}

/// The output records per output section (`spec/image.md` I-R5), from
/// the inputs' relocation section headers: L-R10's relocation rule's
/// facts. Every record of a kept piece's relocation section is written
/// (no `.eh_frame` record is removed before RL-8).
fn records(
    resolution: &Resolution,
    headers: &Headers<'_>,
    layout: &RelocatableLayout<'_>,
    class: Class,
) -> Result<Vec<u64>, Box<Diagnostic>> {
    let mut out = vec![0u64; layout.assignment.outsecs.len() as usize];
    let size = reloc_entry_size(class);
    for (s, row) in resolution.sections.iter() {
        let Some(o) = layout.assignment.out(s) else { continue };
        if row.reloc.0 == 0 || !row.is_kept() {
            continue;
        }
        let (file, _) =
            resolution.sh_index(s).ok_or_else(|| Box::new(internal("a section with no file")))?;
        let r = resolution
            .section_id(file, row.reloc)
            .ok_or_else(|| Box::new(internal("a relocation section with no id")))?;
        let bytes = headers.get(r).map_or(0, |h| h.size);
        if !bytes.is_multiple_of(size) {
            return Err(Box::new(internal(
                "a relocation section is not a whole number of records",
            )));
        }
        out[o.0 as usize] += bytes / size;
    }
    Ok(out)
}

/// The front of a `-r` link: everything up to the checked layout. Every
/// diagnostic goes to `diag`; `None` when one is an error.
#[allow(clippy::too_many_lines, reason = "WP-181 §D's steps in order")]
pub fn front<'s>(
    plan: Plan,
    env: Env<'_>,
    store: &'s Store,
    diag: &Collector,
    timer: &mut Timer,
) -> Option<RFront<'s>> {
    let pool = env.pool;
    // 20: input. `--odr-runtime` is a no-op at `-r` (C-R2b): no runtime
    // object is added (`odr_runtime::embedded` is `None`).
    let load_env = LoadEnv { cwd: env.cwd.to_path_buf(), pool };
    let mut steps = mortar_input::steps::Steps::default();
    let embedded = crate::odr_runtime::embedded(&plan);
    if embedded.is_some() {
        diag.emit(Key::new(20, 0, 0), internal("the ODR runtime at -r"));
        return None;
    }
    let inputs = load_steps_with(&plan, &load_env, store, diag, &mut steps, None)?;
    timer.phase(20, "input");
    let pool = crate::front::capped(pool, &plan, store.stats().resident);
    let machine = inputs.target.machine;
    // RL-9: REL targets and x86-64's parts (record WP-154 §F).
    if machine != Machine::AArch64 {
        diag.emit(
            Key::new(20, u32::MAX, 1),
            mortar_diag::relocatable::refused_target_with_r(machine.name()),
        );
        return None;
    }
    // 30: resolve (RV-R1..RV-R10).
    let outcome = match mortar_resolve::relocatable::resolve(&inputs, &plan, &pool) {
        Ok(o) => o,
        Err(ds) => {
            for (k, d) in ds {
                diag.emit(k, d);
            }
            return None;
        }
    };
    for (k, d) in outcome.diagnostics.iter().cloned() {
        diag.emit(k, d);
    }
    timer.phase(30, "resolve");
    if diag.has_errors() {
        return None;
    }
    // 40: the one-definition rule over the `-r`'s units (MT-30a), as in a
    // static link.
    let odr = mortar_odr::check(&inputs, &outcome.resolution, &plan.diagnostics.odr, None, &pool);
    for (k, d) in odr.diagnostics {
        diag.emit(k, d);
    }
    if let Some(line) = odr.stats {
        timer.line(line);
    }
    let resolution = outcome.resolution;
    if let Some((k, d)) = kept_group(&inputs, &resolution) {
        diag.emit(k, d);
    }
    if diag.has_errors() {
        return None;
    }
    let Some(arch) = LinkArch::of(machine, false, mortar_arch::Status::M1) else {
        diag.emit(Key::new(45, 0, 0), internal(format!("no architecture for {}", machine.name())));
        return None;
    };
    // 50: MD-R1's liveness: every kept section (a dead file has none).
    let n = resolution.sections.len();
    let mut liveness = Liveness::empty(n, 0);
    for (s, row) in resolution.sections.iter() {
        liveness.live[s] = row.is_kept();
    }
    // 60: the `.eh_frame` split.
    let empty = Assignment::empty(n);
    let eh =
        EhOnly { tables: Tables { inputs: &inputs, resolution: &resolution, assignment: &empty } };
    let split = match mortar_merge::phase::run(&pool, &eh, &ArchRelocs(arch.get())) {
        Ok(p) => p.split,
        Err(problems) => {
            for (k, d) in mortar_merge::phase::diagnostics(&inputs, &problems) {
                diag.emit(k, d);
            }
            return None;
        }
    };
    timer.phase(60, "split");
    // The notes' sizes, from the plans the image writes (S16).
    let undefined = mortar_resolve::relocatable::undefined_without_key(&plan, &inputs);
    let notes = {
        let provisional = RelocatableLayout::default();
        let recheck = |_: FileId| Ok(());
        let link = RelocatableLink {
            arch: arch.get(),
            pool: &pool,
            plan: &plan,
            inputs: &inputs,
            resolution: &resolution,
            liveness: &liveness,
            split: &split,
            layout: &provisional,
            recheck: &recheck,
            build_id: None,
            args: &[],
            undefined: &undefined,
        };
        let property = match mortar_image::relocatable::property_plan_with(&link, diag) {
            Ok(p) => p,
            Err(f) => {
                if !diag.has_errors() {
                    for (k, d) in f.diagnostics {
                        diag.emit(k, d);
                    }
                }
                return None;
            }
        };
        let build_id = match mortar_image::relocatable::build_id_plan(&link) {
            Ok(p) => p,
            Err(f) => {
                for (k, d) in f.diagnostics {
                    diag.emit(k, d);
                }
                return None;
            }
        };
        NoteSizes {
            property: property.as_ref().map(mortar_synth::PropertyPlan::size),
            build_id: build_id.as_ref().map(mortar_synth::notes::BuildIdPlan::size),
        }
    };
    // 90: layout (L-R1..L-R10).
    let target = Target::of(machine);
    let layout = match mortar_layout::relocatable::layout(
        &inputs,
        &resolution,
        &liveness,
        &split,
        &target,
        &plan,
        notes,
        &undefined,
        &pool,
    ) {
        Ok(l) => l,
        Err(errors) => {
            for (i, d) in errors.into_iter().enumerate() {
                let (k, d) =
                    crate::front::layout_diag(&inputs, &resolution, d, Key::new(90, 0, i as u64));
                diag.emit(k, d);
            }
            return None;
        }
    };
    // L-R6: removing a record is RL-8's (the image refuses it too; here it
    // comes before L-R10's record counts, which assume none).
    if !layout.eh_removed.is_empty() {
        diag.emit(Key::new(90, 0, 0), mortar_diag::relocatable::refused_eh_records_removed());
        return None;
    }
    let headers = match gather(&inputs, &resolution, &pool) {
        Ok(h) => h,
        Err(d) => {
            let (k, d) = crate::front::layout_diag(&inputs, &resolution, d, Key::new(90, 0, 0));
            diag.emit(k, d);
            return None;
        }
    };
    let records = match records(&resolution, &headers, &layout, target.class) {
        Ok(r) => r,
        Err(d) => {
            diag.emit(Key::new(90, 0, u64::MAX), *d);
            return None;
        }
    };
    let demands = Demands::empty(inputs.symbols.len());
    let shapes = HeaderShapes(&headers);
    let cx = CheckContext {
        class: target.class,
        page_size: target.max_page,
        needs_entry: false,
        segment_rules: SegmentRules {
            class: target.class,
            max_page: target.max_page,
            stack: 6,
            pie: false,
        },
        resolution: &resolution,
        assignment: &layout.assignment,
        split: &split,
        liveness: &liveness,
        demands: &demands,
        shapes: &shapes,
        shared: false,
        placeless: &[],
    };
    // L-R10: the readings, then the group rule (no kept group: refused
    // above) and the relocation rule with the inputs' facts.
    let checked = layout
        .layout
        .validate(&cx)
        .and_then(|()| validate_groups(&layout.layout, &cx, &[]))
        .and_then(|()| validate_relocs(&layout.layout, &cx, &records));
    if let Err(v) = checked {
        diag.emit(
            Key::new(90, 0, u64::MAX),
            internal(format!("the -r layout breaks its rules: {v:?}")),
        );
        return None;
    }
    timer.phase(90, "layout");
    Some(RFront {
        plan,
        pool,
        arch,
        inputs,
        resolution,
        liveness,
        split,
        headers,
        layout,
        undefined,
        args: Vec::new(),
    })
}

/// What [`write`] gives the caller: the text map (`-Map`, `-M`), the
/// machine map (`--mortar-map`), each empty when not asked for, and the
/// output written.
#[derive(Debug)]
pub struct Written {
    pub text: Vec<u8>,
    pub machine: Vec<u8>,
    pub written: mortar_image::link::Written,
}

/// The image phase of a `-r` link (100, 105): the maps (from the tables
/// before fill, I-79), then the output file at `path`, mode `0o666`
/// (I-59). `None` (diagnostics emitted) when it fails.
pub fn write(
    f: &RFront<'_>,
    store: &Store,
    path: &Path,
    diag: &Collector,
    timer: &mut Timer,
) -> Option<Written> {
    // One recheck per blob (an archive's members share their archive's),
    // asked of the file that holds it first (input.md §11.5).
    let mut first_of_blob: Vec<bool> = vec![false; f.inputs.files.len() as usize];
    let mut seen = std::collections::BTreeSet::new();
    for (file, row) in f.inputs.files.iter() {
        if let Some(b) = row.bytes
            && seen.insert(b.blob)
        {
            first_of_blob[file.0 as usize] = true;
        }
    }
    let recheck = |file: FileId| -> Result<(), CopyError> {
        if !first_of_blob.get(file.0 as usize).copied().unwrap_or(false) {
            return Ok(());
        }
        let Some(bytes) = f.inputs.files[file].bytes else { return Ok(()) };
        let Some(blob) = store.blob_of_model(bytes.blob) else { return Ok(()) };
        store.recheck(blob).map_err(|e| crate::image::copy_error(&e))
    };
    let link = RelocatableLink {
        arch: f.arch.get(),
        pool: &f.pool,
        plan: &f.plan,
        inputs: &f.inputs,
        resolution: &f.resolution,
        liveness: &f.liveness,
        split: &f.split,
        layout: &f.layout,
        recheck: &recheck,
        build_id: f.plan.shaping.build_id.as_ref().and_then(mortar_image::link::tree_style),
        args: &f.args,
        undefined: &f.undefined,
    };
    let b = &f.plan.bookkeeping;
    let mut out = Written {
        text: Vec::new(),
        machine: Vec::new(),
        written: mortar_image::link::Written {
            build_id: None,
            leaves: None,
            sealed: None,
            file_size: 0,
            file: None,
            rows: 0,
            jobs: 1,
        },
    };
    let failed = |ds: Vec<(Key, Diagnostic)>| {
        for (k, d) in ds {
            diag.emit(k, d);
        }
    };
    if b.map.is_some() || b.print_map {
        let output = f.plan.output.as_deref().unwrap_or_else(|| std::ffi::OsStr::new("a.out"));
        out.text = mortar_image::textmap::render(&text_map(f, output.as_encoded_bytes()), &f.pool);
    }
    if b.mortar_map.is_some() {
        match mortar_image::relocatable::map_of(&link) {
            Ok(m) => out.machine = mortar_image::map::render(&m, &f.pool),
            Err(e) => {
                failed(e.diagnostics);
                return None;
            }
        }
    }
    let opts = mortar_image::output::OutputOptions {
        mode: 0o666,
        ..mortar_image::output::OutputOptions::executable(store.inodes())
    };
    match mortar_image::relocatable::write(&link, path, &opts) {
        Ok(w) => {
            timer.phase(100, "image");
            out.written = w;
            Some(out)
        }
        Err(e) => {
            failed(e.diagnostics);
            None
        }
    }
}

/// RL-3's sizes against RL-4's (WP-187, brief item 3): every chunk's size
/// in `f`'s layout (computed by `mortar_layout::relocatable`'s pure sizes:
/// `strtab_size`, `shstrtab_size`, the symbol counts, `relocs`, the group
/// and `.eh_frame` sizes) beside the size `mortar_image::relocatable::sizes`
/// fills, and the `.symtab_shndx` decision of each. One line per
/// disagreement, `<chunk>: layout <n>, image <m>`; empty when they agree.
/// [`write`] refuses a disagreement as an internal error; this names it.
///
/// # Errors
///
/// The image cannot read the tables (its diagnostics' texts).
pub fn size_disagreements(f: &RFront<'_>) -> Result<Vec<String>, Vec<String>> {
    let recheck = |_: FileId| -> Result<(), CopyError> { Ok(()) };
    let link = RelocatableLink {
        arch: f.arch.get(),
        pool: &f.pool,
        plan: &f.plan,
        inputs: &f.inputs,
        resolution: &f.resolution,
        liveness: &f.liveness,
        split: &f.split,
        layout: &f.layout,
        recheck: &recheck,
        build_id: f.plan.shaping.build_id.as_ref().and_then(mortar_image::link::tree_style),
        args: &f.args,
        undefined: &f.undefined,
    };
    let sizes = mortar_image::relocatable::sizes(&link)
        .map_err(|e| e.diagnostics.iter().map(|(_, d)| format!("{d:?}")).collect::<Vec<_>>())?;
    let pl = &f.layout.layout.placement;
    let outsecs = &f.layout.assignment.outsecs;
    let mut out = Vec::new();
    let mut shndx = false;
    for &(c, image) in &sizes.chunks {
        let ch = &pl.chunks[c];
        let name = match ch.kind {
            ChunkKind::Body(p) => String::from_utf8_lossy(outsecs[p.out].name).into_owned(),
            ChunkKind::Relocs(o) => format!(".rela{}", String::from_utf8_lossy(outsecs[o].name)),
            ChunkKind::Group(g) => format!("group {g:?}"),
            ChunkKind::Synthetic(s) => {
                shndx |= s.name == b".symtab_shndx";
                String::from_utf8_lossy(s.name).into_owned()
            }
            other => format!("{other:?}"),
        };
        if ch.size != image {
            out.push(format!("{name}: layout {}, image {image}", ch.size));
        }
    }
    if sizes.chunks.len() != pl.chunks.len() as usize {
        out.push(format!("chunks: layout {}, image {}", pl.chunks.len(), sizes.chunks.len()));
    }
    if shndx != sizes.symtab_shndx {
        out.push(format!(".symtab_shndx: layout {shndx}, image {}", sizes.symtab_shndx));
    }
    Ok(out)
}

/// `-Map` at `-r` (I-R12): mortar's own map as in any link, without
/// addresses: each output section at 0 with its pieces at their offsets.
fn text_map(f: &RFront<'_>, output: &[u8]) -> mortar_image::textmap::TextMap {
    use mortar_image::textmap::{Item, OutSection, TextMap, target_name};
    let class64 = f.inputs.target.class == Class::Elf64;
    let machine = match f.inputs.target.machine {
        Machine::AArch64 => 183,
        Machine::X86_64 => 62,
        Machine::I386 => 3,
    };
    let mut loads: Vec<Vec<u8>> = Vec::new();
    for (file, row) in f.inputs.files.iter() {
        if !f.resolution.file_live.get(file).is_some_and(|l| l.is_live()) {
            continue;
        }
        let p = row.path.to_vec();
        if loads.last() != Some(&p) {
            loads.push(p);
        }
    }
    let pl = &f.layout.layout.placement;
    let mut by_chunk: Vec<Vec<(u64, SectionId)>> = vec![Vec::new(); pl.chunks.len() as usize];
    for (s, place) in pl.place.iter() {
        if let Some((c, off)) = place.get() {
            by_chunk[c.0 as usize].push((off, s));
        }
    }
    let outsecs = &f.layout.assignment.outsecs;
    let mut sections = Vec::new();
    for &c in &pl.order {
        let ch = &pl.chunks[c];
        let name: Vec<u8> = match ch.kind {
            ChunkKind::Body(p) => outsecs[p.out].name.to_vec(),
            ChunkKind::Relocs(o) => [&b".rela"[..], outsecs[o].name].concat(),
            ChunkKind::Group(_) => b".group".to_vec(),
            ChunkKind::Synthetic(s) => s.name.to_vec(),
            _ => continue,
        };
        let mut pieces = std::mem::take(&mut by_chunk[c.0 as usize]);
        pieces.sort_unstable();
        let items = pieces
            .into_iter()
            .filter_map(|(off, s)| {
                let (file, _) = f.resolution.sh_index(s)?;
                let fr = f.inputs.files[file].display();
                let file = match fr.member() {
                    Some(m) => format!("{}({m})", fr.path()),
                    None => fr.path().to_owned(),
                };
                Some(Item::Input {
                    name: f.headers[s].name.to_vec(),
                    addr: off,
                    size: f.headers[s].size,
                    file: file.into_bytes(),
                })
            })
            .collect();
        sections.push(OutSection { name, addr: 0, size: ch.size, items });
    }
    TextMap {
        digits: if class64 { 16 } else { 8 },
        archive: Vec::new(),
        discarded: Vec::new(),
        loads,
        sections,
        output: output.to_vec(),
        target: target_name(machine, class64),
    }
}

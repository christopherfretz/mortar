//! The join of the front and the image phase (WP-027c): `Front` as
//! `mortar_image::link::Link` (the eight items of
//! `spec/landed/image/WP-027b.md` "What WP-027a must provide"), then
//! `link::write` (phases 100 and 105) and the map (`-Map`, image.md
//! I-77..I-79).
//!
//! Every answer the image needs that the front's tables do not hold is
//! taken from the phase that decided it, never restated:
//! - the target kind of each key: the scan's own view (`Walker::facts`);
//! - the instruction proof of a section: the scan's `CodeProof` runs
//!   (`Walker::proof`);
//! - the recheck of an input (input.md §11.5): the store's blob of the
//!   file's `FileBytes` (`Store::blob_of_model`), once per blob;
//! - the symbol tables' context: [`Front::with_sym_cx`]'s parts.

use std::path::{Path, PathBuf};

use mortar_arch::{ProofRun, SymFacts, SymNames};
use mortar_core::FileId;
use mortar_core::id::SymKey;
use mortar_core::inputs::{CopyError, Errno};
use mortar_elf::SectionIndex;
use mortar_elf::consts::SHF_EXECINSTR;
use mortar_image::fill::Failed;
use mortar_image::link::{self, KeyFacts, LinkOptions, Written};
use mortar_image::map::Map;
use mortar_image::output::OutputOptions;
use mortar_input::store::{ReadError, Store};
use mortar_merge::phase::Tables;
use mortar_scan::{ScanInputs, ScanOptions, Walker};
use mortar_synth::MergeEh;
use mortar_synth::symtab::{CoreSections, SymCtx};

use crate::front::{Front, output_kind};
use crate::syms::{CoreSyms, LinkSections};

/// The scan's answers per key, for fill (`KeyFacts` over `Walker`).
struct WalkFacts<'w, 'a, 's>(&'w Walker<'a, 's>);

impl SymNames for WalkFacts<'_, '_, '_> {
    fn name(&self, key: SymKey) -> &[u8] {
        self.0.names().name(key)
    }
}

impl KeyFacts for WalkFacts<'_, '_, '_> {
    fn facts(&self, key: SymKey) -> SymFacts {
        self.0.facts(key)
    }

    // WP-032c (image.md I-14): fill's checks of the sections the scan
    // left to it.
    fn facts_at(&self, key: SymKey, mention: Option<mortar_core::SymEntry>) -> SymFacts {
        self.0.facts_at(key, mention)
    }

    fn deferred_report(
        &self,
        sections: &[mortar_core::SectionId],
        pool: &mortar_core::par::Pool,
    ) -> Vec<(mortar_diag::Key, mortar_diag::Diagnostic)> {
        self.0.deferred_report(sections, pool)
    }

    // WP-053 (spec/dyn.md D-40): `.rela.dyn`'s records, as the scan's
    // walk counted them.
    fn walk_records(
        &self,
        file: FileId,
        sid: mortar_core::SectionId,
        index: u32,
        rel: u32,
        leads: bool,
        visit: &mut dyn FnMut(&mortar_arch::ScanCx<'_>, &mortar_arch::RelocAt<'_>),
    ) -> Result<(), mortar_diag::Diagnostic> {
        self.0
            .walk(file, sid, index, rel, 0..u64::MAX, leads, &mut |v| visit(&v.cx(), &v.at))
            .map_err(|(_, d)| d)
    }
}

/// The output path: `-o`'s value or `a.out` (image.md I-56), relative to
/// `cwd`.
#[must_use]
pub fn output_path(front: &Front<'_>, cwd: &Path) -> PathBuf {
    let p = front.plan.output.as_deref().map_or_else(|| PathBuf::from("a.out"), PathBuf::from);
    cwd.join(p)
}

/// `CopyError` of the store's recheck (`ReadError`).
fn copy_error(e: &ReadError) -> CopyError {
    match e {
        ReadError::Changed => CopyError::Changed,
        ReadError::Os { errno, .. } => CopyError::Read(Errno(errno.unwrap_or(0))),
        _ => CopyError::Read(Errno(0)),
    }
}

/// Calls `go` with the image phase's view of `front` (`link::Link`).
/// `before_recheck(file)` runs before each input's recheck (a test changes
/// a file there; the driver passes a no-op).
///
/// # Errors
///
/// `MergeEh::new`'s, which phase 80 already passed on these tables.
#[allow(
    clippy::too_many_lines,
    reason = "the plans of every chunk, static and dynamic, in one place"
)]
pub fn with_link<T>(
    front: &Front<'_>,
    store: &Store,
    before_recheck: &(dyn Fn(FileId) + Sync),
    go: impl FnOnce(&link::Link<'_, '_>) -> T,
) -> Result<T, mortar_synth::error::SynthError> {
    let f = front;
    let tables = Tables { inputs: &f.inputs, resolution: &f.resolution, assignment: &f.assignment };
    let merge_eh = MergeEh::new(&f.merge.eh, &tables)?;
    let core =
        CoreSections { resolution: &f.resolution, liveness: &f.liveness, split: &f.merge.split };
    let sections = LinkSections { core, assignment: &f.assignment };
    let syms =
        CoreSyms::new(&f.inputs, &f.resolution, &f.assignment).dynamic(f.dyn_tables.is_some());
    let cx = SymCtx {
        syms: &syms,
        sections: &sections,
        eh: Some((&f.eh, &merge_eh)),
        discard: f.plan.shaping.discard,
    };
    let output = output_kind(f.plan.kind);
    let scan_inputs = ScanInputs {
        inputs: &f.inputs,
        resolution: &f.resolution,
        liveness: &f.liveness,
        assignment: Some(&f.assignment),
        comdat_mismatch: &f.comdat_mismatch,
        gnu_warnings: &f.gnu_warnings,
        closure_only: &f.closure_only,
    };
    // As the scan's options (front.rs): a dynamic output's walk sees the
    // output sections' writability and the scan's `WeakDyn` keys.
    let dyn_tables = f.dyn_tables.as_ref();
    let out_writable = crate::dynlink::OutWritable(&f.assignment);
    let scan_options = ScanOptions {
        output,
        dynamic: dyn_tables.is_some(),
        sections: dyn_tables.map(|_| &out_writable as &dyn mortar_scan::weak::SectionMap),
        proof: f.arch.proof(),
    };
    let mut walker = Walker::new(f.arch.get(), &scan_inputs, scan_options);
    if dyn_tables.is_some() {
        walker = walker.with_weak_dyn(f.scanned.demands.weak_dyn.clone());
    }
    let dyn_src = dyn_tables
        .map(|_| crate::dynlink::DynSource::new(&f.inputs, &f.resolution, &f.assignment, &f.plan));
    let facts = WalkFacts(&walker);
    let proof = |file: FileId, index: SectionIndex, len: u64| -> Vec<ProofRun> {
        let exec = f
            .resolution
            .section_id(file, index)
            .is_some_and(|sid| f.headers[sid].flags & SHF_EXECINSTR != 0);
        walker.proof(file, index.0, len, exec)
    };
    // One recheck per blob (an archive's members share their archive's),
    // asked of the file that holds it first.
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
        before_recheck(file);
        if !first_of_blob.get(file.0 as usize).copied().unwrap_or(false) {
            return Ok(());
        }
        let Some(bytes) = f.inputs.files[file].bytes else { return Ok(()) };
        let Some(blob) = store.blob_of_model(bytes.blob) else { return Ok(()) };
        store.recheck(blob).map_err(|e| copy_error(&e))
    };
    let s = &f.synth;
    let link = link::Link {
        arch: f.arch.get(),
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
            chunks: &s.chunks,
            hdr: s.hdr.as_ref(),
            got: &s.got,
            symtab: s.symtab.as_ref().map(|p| (p, cx)),
            shstrtab: &s.shstrtab,
            build_id: s.build_id.as_ref(),
            property: &s.property,
            mason_meta: s.mason_meta.as_ref(),
            dynamic: match (dyn_tables, &dyn_src) {
                (Some(d), Some(src)) => Some(link::DynPlans {
                    kind: d.kind,
                    dynsym: &d.dynsym,
                    src,
                    rela: &d.rela,
                    dynamic: &d.dynamic,
                    interp: d.interp.as_ref(),
                    runpath: d.runpath.as_ref().map(|(t, p)| (*t, p.as_slice())),
                    copy_size: &d.copy.size,
                    bind_now: d.bind_now,
                    relro: d.relro,
                    note_mason: d.note_mason.as_ref(),
                    syms: cx,
                    args: &f.args,
                }),
                _ => None,
            },
        },
        facts: &facts,
        proof: &proof,
        recheck: &recheck,
        options: LinkOptions {
            output,
            separate_code: f.options.separate_code,
            build_id: f.plan.shaping.build_id.as_ref().and_then(link::tree_style),
            bti: {
                let (ty, bit) = f.arch.feature_1();
                s.property.value_u32(ty) & bit != 0
            },
            keep_leaves: f.keep_leaves,
        },
        fixups: &f.erratum_fixes,
    };
    Ok(go(&link))
}

/// The map of `front`'s link (`link::map_of`, image.md I-78).
///
/// # Errors
///
/// [`with_link`]'s.
pub fn map_of(front: &Front<'_>, store: &Store) -> Result<Map, mortar_synth::error::SynthError> {
    with_link(front, store, &|_| {}, link::map_of)
}

/// The text map of `front`'s link (`link::text_map_of`, image.md I-121),
/// for the output `output` names.
///
/// # Errors
///
/// [`with_link`]'s.
pub fn text_map_of(
    front: &Front<'_>,
    store: &Store,
    output: &[u8],
) -> Result<mortar_image::textmap::TextMap, mortar_synth::error::SynthError> {
    use mortar_core::inputs::DirectiveKind;
    use mortar_core::plan::SymbolRole;
    // The roots by argument index, as resolve names them (§4.2): the
    // entry is index 0.
    let plan = &front.plan;
    let script_entry =
        front.inputs.link.directives.iter().rev().find(|d| d.kind == DirectiveKind::Entry);
    let entry = match (&plan.entry, script_entry) {
        (Some(e), _) => e.as_encoded_bytes().to_vec(),
        (None, Some(d)) => d.value.to_vec(),
        (None, None) => b"_start".to_vec(),
    };
    let mut roots: Vec<(u32, Vec<u8>)> = vec![(0, entry)];
    roots.extend(
        plan.symbols
            .iter()
            .filter(|s| matches!(s.role, SymbolRole::Undefined | SymbolRole::RequireDefined))
            .map(|s| (s.arg, s.value.as_encoded_bytes().to_vec())),
    );
    with_link(front, store, &|_| {}, |l: &link::Link<'_, '_>| link::text_map_of(l, output, &roots))
}

/// The output options of `front`'s link: the mode of an executable, and
/// the identities of every input the store read (I-62, I-66, I-113).
#[must_use]
pub fn output_options(store: &Store) -> OutputOptions {
    OutputOptions::executable(store.inodes())
}

/// Writes `front`'s output file at `path` (`link::write`), with
/// `before_recheck` as [`with_link`]'s and `on_phase` as `link::write`'s.
///
/// # Errors
///
/// The image phase's diagnostics.
pub fn write(
    front: &Front<'_>,
    store: &Store,
    path: &Path,
    before_recheck: &(dyn Fn(FileId) + Sync),
    on_phase: &mut dyn FnMut(u16, &str),
) -> Result<Written, Failed> {
    let opts = output_options(store);
    match with_link(front, store, before_recheck, |l| link::write(l, path, &opts, on_phase)) {
        Ok(w) => w,
        Err(e) => {
            Err(Failed { diagnostics: vec![(mortar_synth::error::key(0), e.into_diagnostic())] })
        }
    }
}

/// The test-only update path's image phase (`link::refill`): the jobs of
/// the dirty `windows` of `front`'s output, filled into memory.
///
/// # Errors
///
/// The image phase's diagnostics.
pub fn refill(
    front: &Front<'_>,
    store: &Store,
    path: &Path,
    windows: &[u64],
) -> Result<link::Refilled, Failed> {
    match with_link(front, store, &|_| {}, |l| link::refill(l, path, windows)) {
        Ok(r) => r,
        Err(e) => {
            Err(Failed { diagnostics: vec![(mortar_synth::error::key(0), e.into_diagnostic())] })
        }
    }
}

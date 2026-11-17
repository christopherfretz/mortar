//! The tables of an output with `.dynamic` (WP-053, the M2 join): what
//! phase 80 plans from (`spec/dyn.md` D-21 to D-49, D-30), over the
//! front's tables. A static link builds none of this.
//!
//! - [`OutWritable`]: D-25's writable place (the scan's `SectionMap`),
//!   from the output section an input section maps to;
//! - [`ScanKinds`]: the scan's target kind of each key (`KeyKinds`);
//! - [`DynSource`]: `.dynsym`'s names, shared definitions, program
//!   definitions and versions (`DynSymSource`, `DynNames`), read from
//!   the inputs;
//! - [`plan`]: the phase-80 plans of the dynamic chunks, in D-21's,
//!   D-27's, D-24's, D-46's and D-30's order, and the `Slots` fields
//!   they fix.

use mortar_arch::{Arch, OutputCx, PltVariant, TargetKind};
use mortar_core::assignment::Assignment;
use mortar_core::demands::{Demands, GlobalFlags};
use mortar_core::inputs::{Inputs, Origin, Reader};
use mortar_core::layout::Slots;
use mortar_core::liveness::Liveness;
use mortar_core::par::Pool;
use mortar_core::plan::{DynamicLinker, InputSpec, OutputKind, Plan};
use mortar_core::resolution::{Def, RefFlags, Resolution};
use mortar_core::{FileId, SectionId, SymEntry, SymKey, SymbolId, VerIndex};
use mortar_diag::{Collector, Diagnostic, RefusalClass};
use mortar_elf::consts::{SHF_WRITE, STO_AARCH64_VARIANT_PCS, STT_OBJECT};
use mortar_elf::{SymbolIndex, SymbolTable, Version, Versions};
use mortar_scan::Walker;
use mortar_scan::weak::SectionMap;
use mortar_synth::copy::CopyPlan;
use mortar_synth::dynamic::{
    DynKind, DynamicInputs, RunpathTag, SharedDyn, init_key, join_rpath, shared_init_key,
};
use mortar_synth::dynstr::DynNames;
use mortar_synth::dynsym::{DynSymSource, DynsymInputs, ProgramDef, SharedDef};
use mortar_synth::symtab::SymDef;
use mortar_synth::version::VerdefPlan;
use mortar_synth::{
    DynGotCx, DynamicPlan, GotPlan, InputCopyDefs, InterpPlan, KeyKinds, RelaPlan, SymSource,
    SynthKind,
};

use crate::syms::CoreSyms;

/// D-25: a place is writable when the output section its input section
/// maps to has `SHF_WRITE` (RELRO sections have it).
#[derive(Debug, Clone, Copy)]
pub struct OutWritable<'a, 'n>(pub &'a Assignment<'n>);

impl SectionMap for OutWritable<'_, '_> {
    fn writable(&self, section: SectionId) -> bool {
        let a = self.0;
        a.out(section).and_then(|o| a.outsecs.get(o)).is_some_and(|o| o.flags & SHF_WRITE != 0)
    }
}

/// The scan's target kind of each key (`Walker::facts`).
#[derive(Debug)]
pub struct ScanKinds<'w, 'a, 's>(pub &'w Walker<'a, 's>);

impl KeyKinds for ScanKinds<'_, '_, '_> {
    fn kind(&self, key: SymKey) -> TargetKind {
        self.0.facts(key).kind
    }
}

/// A loaded shared object as `.dynsym` reads it.
struct Shared<'s> {
    table: Option<SymbolTable<'s>>,
    versions: Option<Versions<'s>>,
    /// Its `DT_NEEDED` name (D-19a, input §8.3).
    needed: &'s [u8],
}

/// [`DynSymSource`] over the inputs.
pub struct DynSource<'a, 's> {
    inputs: &'a Inputs<'s>,
    syms: CoreSyms<'a, 's>,
    /// For P-1's place of a definition (`has_place`).
    resolution: &'a Resolution,
    assignment: &'a Assignment<'a>,
    liveness: &'a Liveness,
    /// Per file: the shared object's tables.
    shared: Vec<Option<Shared<'s>>>,
}

impl std::fmt::Debug for DynSource<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DynSource").finish_non_exhaustive()
    }
}

/// The last component of `path`.
fn file_name(path: &[u8]) -> &[u8] {
    path.rsplit(|&b| b == b'/').next().unwrap_or(path)
}

impl<'a, 's> DynSource<'a, 's> {
    #[must_use]
    pub fn new(
        inputs: &'a Inputs<'s>,
        resolution: &'a Resolution,
        assignment: &'a Assignment<'a>,
        liveness: &'a Liveness,
        plan: &Plan,
    ) -> Self {
        let shared = inputs
            .files
            .iter()
            .map(|(_, row)| {
                let Reader::Shared(so) = row.reader else { return None };
                let soname = so.dynamic().ok().flatten().and_then(|d| d.soname().ok().flatten());
                // input §8.3: the soname; else a path as written, or the
                // file name of a library found by a search.
                let needed = soname.unwrap_or_else(|| match row.origin {
                    Origin::Input { input, entry: None, .. }
                        if matches!(
                            plan.inputs.get(input as usize).map(|i| &i.spec),
                            Some(InputSpec::Path(_))
                        ) =>
                    {
                        row.path
                    }
                    _ => file_name(row.path),
                });
                Some(Shared {
                    table: so.symbols().ok().flatten(),
                    versions: so.versions().ok(),
                    needed,
                })
            })
            .collect();
        Self {
            inputs,
            syms: CoreSyms::new(inputs, resolution, assignment),
            resolution,
            assignment,
            liveness,
            shared,
        }
    }

    fn shared_of(&self, file: FileId) -> Option<&Shared<'s>> {
        self.shared.get(file.0 as usize)?.as_ref()
    }
}

impl DynNames for DynSource<'_, '_> {
    fn symbol(&self, key: SymbolId) -> &[u8] {
        self.inputs.symbols.get(key).map_or(&[][..], |k| k.name)
    }

    fn needed(&self, file: FileId) -> &[u8] {
        self.shared_of(file).map_or(&[][..], |s| s.needed)
    }

    fn version(&self, file: FileId, version: VerIndex) -> &[u8] {
        let Some(v) = self.shared_of(file).and_then(|s| s.versions.as_ref()) else {
            return &[];
        };
        match v.version(SymbolIndex(0), version.0) {
            Ok(Version::Defined(d)) => d.name().unwrap_or_default(),
            _ => &[],
        }
    }
}

impl DynSymSource for DynSource<'_, '_> {
    fn shared(&self, entry: SymEntry) -> Option<SharedDef> {
        let s = self.shared_of(entry.file)?.table.as_ref()?.get(entry.index).ok()?;
        Some(SharedDef { info: s.st_info, other: s.st_other, value: s.st_value, size: s.st_size })
    }

    fn program(&self, key: SymbolId) -> Option<ProgramDef<'_>> {
        let g = self.syms.global(key)?;
        if g.def == SymDef::Undefined {
            return None;
        }
        // S-81: a common symbol is `STT_OBJECT`.
        let typ = if g.def == SymDef::Common { STT_OBJECT } else { g.typ };
        // `foo@V` is in the key's name; `foo@@V` only in the defining entry.
        let version = self
            .inputs
            .symbols
            .get(key)
            .and_then(|k| k.version)
            .or_else(|| self.syms.default_version(key));
        // D-23's export row: the merged visibility, and the defining
        // entry's `STO_AARCH64_VARIANT_PCS` (kept on AArch64 by `entry`).
        let other = g.other | self.syms.defined_other_flags(key);
        Some(ProgramDef { info: g.binding << 4 | typ, other, size: g.size, version })
    }

    fn has_place(&self, key: SymbolId) -> bool {
        match self.syms.global(key).map(|g| g.def) {
            Some(SymDef::Section { section, .. }) => mortar_core::place::section_place(
                self.resolution,
                Some(self.assignment),
                self.liveness,
                section,
            )
            .is_ok(),
            _ => true,
        }
    }

    fn non_default_version(&self, key: SymbolId) -> bool {
        self.inputs.symbols.get(key).is_some_and(|k| k.version.is_some())
    }

    fn bound_version(&self, entry: SymEntry) -> Option<VerIndex> {
        let v = self.shared_of(entry.file)?.versions.as_ref()?.versym()?;
        v.get(entry.index.0 as usize).map(|s| VerIndex(s.index()))
    }
}

/// The exported keys (`Resolution.exported`, the `EXPORT` demands) whose
/// definition has no place (`spec/resolve.md` §10.1 P-1, WP-136), sorted:
/// `.dynsym` omits them (`mortar_synth::dynsym`, by the same
/// [`DynSymSource::has_place`]), and the layout's check does not require
/// them.
#[must_use]
pub fn placeless_exports(
    src: &DynSource<'_, '_>,
    resolution: &Resolution,
    demands: &Demands,
) -> Vec<SymbolId> {
    let mut v: Vec<SymbolId> = resolution
        .exported
        .iter()
        .chain(
            demands
                .keys_with(mortar_core::demands::DemandFlags::EXPORT)
                .filter_map(SymKey::as_global),
        )
        .filter(|&k| !src.has_place(k))
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// The plans of an output with `.dynamic` (`Front.dyn_tables`).
#[derive(Debug, Clone)]
pub struct DynTables {
    pub kind: DynKind,
    /// D-42 (`Slots.copies`, `copy_of` and the sizes).
    pub copy: CopyPlan,
    /// D-21, D-22, D-26.
    pub dynsym: mortar_synth::DynsymPlan,
    /// D-40.
    pub rela: RelaPlan,
    /// D-30.
    pub dynamic: DynamicPlan,
    /// D-31; `None` in a `StaticPie` (D-1).
    pub interp: Option<InterpPlan>,
    /// The run path's string (D-30 row 2), and its tag.
    pub runpath: Option<(RunpathTag, Vec<u8>)>,
    pub bind_now: bool,
    pub relro: bool,
    /// The sizes of `.gnu.hash`, `.dynsym`, `.dynstr` and, when some entry
    /// is versioned, `.gnu.version` and `.gnu.version_r` (S-108).
    pub sizes: Vec<(SynthKind, u64)>,
    /// `.note.mason` (`spec/odr.md` O-50 to O-52), with `--odr-table`;
    /// filled by the front once `.dynsym`'s order is known.
    pub note_mason: Option<mortar_synth::NoteMasonPlan>,
    /// A shared object's facts (`spec/dyn.md` D-102, D-116; plumb.md
    /// §3.2): `Some` exactly when `kind` is `DynKind::Shared`.
    pub shared: Option<SharedTables>,
}

/// What a shared object's image needs besides an executable's plans
/// (`mortar_image::link::SharedPlans`, record `spec/landed/dyn/WP-118.md`
/// §4 item 8).
#[derive(Debug, Clone)]
pub struct SharedTables {
    /// The `-soname` string and its `.dynstr` offset (D-102, D-116).
    pub soname: Option<(Vec<u8>, u32)>,
    /// The map's shared-object line records (`map::SharedLine::of`).
    pub line: mortar_image::map::SharedLine,
    /// D-115: the version definitions, their `.dynstr` offsets set
    /// (empty without a named version node).
    pub verdef: VerdefPlan,
}

/// What [`plan`] reads.
pub struct PlanIn<'a, 's> {
    pub plan: &'a Plan,
    pub arch: &'a dyn Arch,
    pub inputs: &'a Inputs<'s>,
    pub resolution: &'a Resolution,
    pub assignment: &'a Assignment<'s>,
    pub demands: &'a Demands,
    pub liveness: &'a Liveness,
    pub headers: &'a mortar_layout::Headers<'s>,
    pub src: &'a DynSource<'a, 's>,
    pub kinds: &'a dyn KeyKinds,
    pub needed: &'a [FileId],
    pub variant: PltVariant,
    pub output_cx: OutputCx,
    pub got_output: GlobalFlags,
    pub relro: bool,
    pub pool: &'a Pool,
    /// The line's version script (`spec/dyn.md` D-114, D-115; WP-125):
    /// its nodes are a shared object's `.gnu.version_d`.
    pub versions: Option<&'a mortar_resolve::version::VersionScript>,
}

impl std::fmt::Debug for PlanIn<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlanIn").finish_non_exhaustive()
    }
}

fn refused(what: &str) -> Diagnostic {
    Diagnostic::refused(RefusalClass::Feature, what)
}

/// The output's interpreter (D-31): `-dynamic-linker`'s path.
#[allow(clippy::result_large_err, reason = "the phase's diagnostic, made once")]
fn interp(plan: &Plan) -> Result<InterpPlan, Diagnostic> {
    use std::os::unix::ffi::OsStrExt as _;
    match &plan.dynamic.dynamic_linker {
        Some(DynamicLinker::Path(p)) => {
            InterpPlan::new(p.as_bytes()).map_err(mortar_synth::SynthError::into_diagnostic)
        }
        _ => Err(refused("a dynamic executable that names no interpreter (-dynamic-linker)")),
    }
}

/// The keys of `DT_INIT` and `DT_FINI` (D-30 rows 3-4, [`init_key`]):
/// over the RESOLVED symbols, an undefined one naming nothing.
fn init_fini(t: &PlanIn<'_, '_>) -> (Option<SymbolId>, Option<SymbolId>) {
    use std::os::unix::ffi::OsStrExt as _;
    let d = &t.plan.dynamic;
    let names: [&[u8]; 2] = [d.init_symbol().as_bytes(), d.fini_symbol().as_bytes()];
    let mut found = [None, None];
    for (id, k) in t.inputs.symbols.iter() {
        for (i, n) in names.iter().enumerate() {
            if k.name == *n && k.version.is_none() {
                let defined = matches!(
                    t.assignment.def(t.resolution, id),
                    Some(Def::Defined(_) | Def::Common(_) | Def::Internal(_))
                );
                if defined {
                    found[i] = Some(id);
                }
            }
        }
    }
    let init = init_key(d.init.as_ref().map(|_| found[0]), found[0]);
    let fini = init_key(d.fini.as_ref().map(|_| found[1]), found[1]);
    (init, fini)
}

/// The keys of `DT_INIT` and `DT_FINI` of a shared object (D-102,
/// [`shared_init_key`]): a symbol `-init`/`-fini` names must be defined
/// (`undefined-init-symbol`); without the option, `_init`/`_fini` when
/// defined. An explicit name must also be code: a function (or a
/// `NOTYPE` label) in an executable input section; a data object, TLS or
/// IFUNC symbol is refused by name (WP-128: GNU ld, lld and mold write a
/// data object's address and the loader calls it, crashing; the verifier's
/// P-D22 wants "defined, live code").
#[allow(clippy::result_large_err, reason = "the phase's diagnostic, made once")]
fn shared_init_fini(
    t: &PlanIn<'_, '_>,
) -> Result<(Option<SymbolId>, Option<SymbolId>), Diagnostic> {
    use std::os::unix::ffi::OsStrExt as _;
    let d = &t.plan.dynamic;
    let (init, fini) = init_fini(t);
    // With the option: the named symbol (its key when defined); without
    // it: `_init`/`_fini` when defined.
    let init = match &d.init {
        Some(n) => shared_init_key(Some((n.as_bytes(), init)), None),
        None => shared_init_key(None, init),
    }
    .map_err(mortar_synth::SynthError::into_diagnostic)?;
    let fini = match &d.fini {
        Some(n) => shared_init_key(Some((n.as_bytes(), fini)), None),
        None => shared_init_key(None, fini),
    }
    .map_err(mortar_synth::SynthError::into_diagnostic)?;
    for (option, name, key) in [("-init", &d.init, init), ("-fini", &d.fini, fini)] {
        if let (Some(name), Some(key)) = (name, key)
            && !is_code(t, key)
        {
            return Err(refused(&format!(
                "{option} naming {}, which is not a function (dyn.md D-102 rows 3-4)",
                name.to_string_lossy()
            )));
        }
    }
    Ok((init, fini))
}

/// Whether `key`'s definition is code: `STT_FUNC` or `STT_NOTYPE` in an
/// input section with `SHF_EXECINSTR`.
fn is_code(t: &PlanIn<'_, '_>, key: SymbolId) -> bool {
    use mortar_elf::consts::{SHF_EXECINSTR, STT_FUNC, STT_NOTYPE};
    use mortar_synth::symtab::SymDef;
    let Some(g) = t.src.syms.global(key) else { return false };
    let SymDef::Section { section, .. } = g.def else { return false };
    matches!(g.typ, STT_FUNC | STT_NOTYPE) && t.headers[section].flags & SHF_EXECINSTR != 0
}

/// Whether output sections `.preinit_array`, `.init_array`, `.fini_array`
/// have live content (D-30 rows 5-10).
fn arrays(t: &PlanIn<'_, '_>) -> [bool; 3] {
    let names: [&[u8]; 3] = [b".preinit_array", b".init_array", b".fini_array"];
    let mut has = [false; 3];
    for (sid, out) in t.assignment.out_of.iter() {
        let Some(o) = out.out() else { continue };
        let Some(os) = t.assignment.outsecs.get(o) else { continue };
        if let Some(i) = names.iter().position(|n| *n == os.name)
            && t.liveness.is_live(sid)
            && t.headers[sid].size > 0
        {
            has[i] = true;
        }
    }
    has
}

/// D-115's plan of a shared object: the base (`soname`, else the
/// output's basename), then the script's named nodes in order with their
/// parents; empty without one.
fn verdef_plan(t: &PlanIn<'_, '_>, soname: Option<&[u8]>) -> VerdefPlan {
    let Some(script) = t.versions.filter(|s| !s.is_anonymous()) else {
        return VerdefPlan::default();
    };
    let output = t.plan.output.as_deref().unwrap_or("a.out".as_ref());
    let basename = std::path::Path::new(output).file_name().unwrap_or(output);
    let base = soname.unwrap_or(basename.as_encoded_bytes());
    let nodes: Vec<(&[u8], &[usize])> = script
        .nodes()
        .iter()
        .map(|n| (n.name.as_deref().unwrap_or_default(), &*n.parents))
        .collect();
    VerdefPlan::new(base, &nodes)
}

/// D-103's `-z` flags of a `Shared` line (WP-128): each keyword lands in
/// [`Plan::other`] as `-z <keyword>`; repeating one changes nothing (no
/// reference has a `no` form). The front refuses them in the other kinds.
fn z_flags(plan: &Plan) -> mortar_synth::dynamic::ZFlags {
    let has = |name: &str| plan.other.iter().any(|o| o.name == name);
    mortar_synth::dynamic::ZFlags {
        nodelete: has("-z nodelete"),
        nodlopen: has("-z nodlopen"),
        initfirst: has("-z initfirst"),
        interpose: has("-z interpose"),
        origin: has("-z origin"),
    }
}

/// The `.dynamic` kind of the plan's output and its `.interp` plan (none
/// in a `StaticPie`, D-1), or the refusal reported through `emit`.
fn kind_and_interp(
    plan: &Plan,
    emit: &dyn Fn(Diagnostic),
) -> Option<(DynKind, Option<InterpPlan>)> {
    let kind = match plan.kind {
        OutputKind::Executable => DynKind::Exec,
        OutputKind::PieExecutable => DynKind::Pie,
        OutputKind::StaticPie => DynKind::StaticPie,
        // D-102: `DT_SONAME` with `-soname`; `DT_SYMBOLIC`/`DF_SYMBOLIC`
        // for a last `-Bsymbolic` without `--dynamic-list` (D-112).
        OutputKind::Shared => DynKind::Shared(SharedDyn {
            soname: plan.dynamic.soname.is_some(),
            symbolic: plan.symbolic().is_some_and(mortar_cli::SymbolicLine::writes_symbolic_tags),
            // Set from the scan's demands by `plan` (D-119).
            static_tls: false,
            // Set from the version script by `plan` (D-115).
            verdef: 0,
            z: z_flags(plan),
        }),
        _ => {
            emit(refused("this output kind with .dynamic"));
            return None;
        }
    };
    // D-1: a `StaticPie`'s `.dynsym` is the null entry only, whatever the
    // line exports: `--export-dynamic` and `--export-dynamic-symbol` are
    // accepted and have no effect there (GNU ld, lld and mold export, and
    // nothing can bind the exports: WP-056; judged by P-D14, WP-088).
    // D-1: a `StaticPie` names no interpreter and has no `.interp`.
    let interp = match kind {
        // D-100: a shared object has no `.interp`, `-dynamic-linker` or
        // not (GNU ld and lld write none; record WP-117 §5).
        DynKind::StaticPie | DynKind::Shared(_) => None,
        DynKind::Exec | DynKind::Pie => Some(interp(plan).map_err(emit).ok()?),
    };
    Some((kind, interp))
}

/// Phase 80 for an output with `.dynamic`: the copy, `.dynsym`, version,
/// `.dynstr`, GOT, `.rela.dyn`, `.dynamic` and `.interp` plans, with the
/// `Slots` fields they fix (`needed`, `copies`, `copy_of`, `dynsym`,
/// `verneed`, `dynstr`). Each failure is reported into `diag`.
#[allow(
    clippy::too_many_lines,
    reason = "one step per plan, the shared object's beside the executable's"
)]
pub fn plan(
    t: &PlanIn<'_, '_>,
    slots: &mut Slots,
    diag: &Collector,
) -> Option<(GotPlan, DynTables)> {
    let key = mortar_synth::error::key(0);
    let emit = |d: Diagnostic| {
        diag.emit(key, d);
    };
    let facts = t.arch.facts();
    let (mut kind, interp) = kind_and_interp(t.plan, &emit)?;
    // D-103, D-119: `DF_STATIC_TLS` when some reference is initial-exec.
    if let DynKind::Shared(s) = &mut kind {
        s.static_tls = t.demands.output.contains(GlobalFlags::STATIC_TLS);
    }
    let shared = matches!(kind, DynKind::Shared(_));
    // D-42: the copy allocations.
    let defs = InputCopyDefs { inputs: t.inputs, resolution: t.resolution };
    let copy =
        mortar_synth::copy_plan(t.demands, &defs).map_err(|e| emit(e.into_diagnostic())).ok()?;
    slots.needed = t.needed.into();
    slots.copies.clone_from(&copy.copies);
    slots.copy_of.clone_from(&copy.copy_of);
    // D-21, D-22, D-26; in a shared object D-111's set.
    let dynsym = if shared {
        mortar_synth::dynsym::dynsym_plan_shared(
            t.resolution,
            t.demands,
            slots,
            t.src,
            t.pool,
            diag,
        )
        .ok()?
    } else {
        mortar_synth::dynsym_plan(t.resolution, t.demands, slots, t.src, t.pool, diag).ok()?
    };
    slots.dynsym = dynsym.order().into();
    slots.dynsym_unhashed = dynsym.unhashed();
    slots.gnu_hash_buckets = dynsym.nbuckets();
    // D-27.
    let inputs =
        DynsymInputs { resolution: t.resolution, demands: t.demands, slots, src: t.src, shared };
    let verneed = mortar_synth::version::verneed_plan(&dynsym, &inputs, t.needed, diag).ok()?;
    // D-24.
    let rpath =
        join_rpath(&t.plan.dynamic.rpath.iter().map(|p| p.as_encoded_bytes()).collect::<Vec<_>>());
    // D-116: a shared object's soname after the needed names.
    let soname_bytes = t.plan.dynamic.soname.as_ref().map(|s| s.as_encoded_bytes().to_vec());
    // D-115: the version definitions of a shared object, the base named
    // by the soname, else the output's basename.
    let mut verdef =
        if shared { verdef_plan(t, soname_bytes.as_deref()) } else { VerdefPlan::default() };
    let (dynstr, soname_off) = if shared {
        mortar_synth::dynstr::dynstr_plan_versioned(
            &dynsym,
            t.needed,
            soname_bytes.as_deref(),
            Some(&mut verdef),
            &verneed,
            rpath.as_deref(),
            t.src,
        )
    } else {
        (mortar_synth::dynstr_plan(&dynsym, t.needed, &verneed, rpath.as_deref(), t.src), None)
    };
    if let DynKind::Shared(s) = &mut kind {
        s.verdef = verdef.count();
    }
    let sizes = mortar_synth::dynsym::chunk_sizes_versioned(
        &dynsym,
        &verneed,
        Some(&verdef),
        &dynstr,
        facts.class,
    )
    .map_err(|e| emit(e.into_diagnostic()))
    .ok()?;
    slots.verneed = verneed.iter().cloned().collect();
    slots.dynstr = dynstr;
    // D-46, D-48, D-41; D-120 in a shared object.
    let got = if shared {
        GotPlan::new_shared(
            t.arch,
            slots,
            t.got_output,
            t.variant,
            t.output_cx.bind_now,
            t.kinds,
            &dynsym,
            diag,
        )
        .ok()?
    } else {
        let cx = DynGotCx {
            pie: matches!(kind, DynKind::Pie | DynKind::StaticPie),
            bind_now: t.output_cx.bind_now,
            static_pie: kind == DynKind::StaticPie,
        };
        GotPlan::new_dynamic(t.arch, slots, t.got_output, t.variant, cx, t.kinds, &dynsym, diag)
            .ok()?
    };
    // D-40.
    let rela = RelaPlan::new(facts.class, facts.rel_form, t.demands, &got, slots.copies.len())
        .map_err(|e| emit(e.into_diagnostic()))
        .ok()?;
    // The sections with dynamic records, by section (their `RELATIVE`
    // records are ordered by address at write time, D-40).
    slots.dyn_relocs = rela.sections().collect();
    // D-30; D-102 in a shared object.
    let (init, fini) = if shared { shared_init_fini(t).map_err(emit).ok()? } else { init_fini(t) };
    let [preinit_array, init_array, fini_array] = arrays(t);
    let runpath = rpath.map(|r| (RunpathTag::of(t.plan.dynamic.new_dtags), r));
    let variant_pcs_jump_slot = slots.plt.iter().any(|&k| {
        let Some(id) = k.as_global() else { return false };
        match t.resolution.def.get(id) {
            Some(Def::Shared(e)) => {
                t.src.shared(*e).is_some_and(|d| d.other & STO_AARCH64_VARIANT_PCS != 0)
            }
            // A `WeakDyn` key's entry has the bit of its references (D-23).
            Some(Def::Undefined { .. }) => {
                t.resolution.props.get(id).is_some_and(|p| p.refs.contains(RefFlags::VARIANT_PCS))
            }
            _ => false,
        }
    });
    let inputs = DynamicInputs {
        kind,
        class: facts.class,
        rel_form: facts.rel_form,
        needed: t.needed.len() as u32,
        runpath: runpath.as_ref().map(|r| r.0),
        init,
        fini,
        preinit_array,
        init_array,
        fini_array,
        got_plt: got.got_plt_size() > 0,
        rela_plt: got.rela_plt_size() > 0,
        rela_dyn: rela.size() > 0,
        relative: rela.relative_count(),
        verneed: slots.verneed.len() as u32,
        bind_now: t.output_cx.bind_now,
        arch_tags: t.arch.dyn_tags(&t.output_cx),
        bti_property: t.output_cx.feature_1_and
            & mortar_elf::consts::GNU_PROPERTY_AARCH64_FEATURE_1_BTI
            != 0,
        variant_pcs_jump_slot,
    };
    let dynamic = DynamicPlan::new(&inputs).map_err(|e| emit(e.into_diagnostic())).ok()?;
    let shared = if shared {
        let Some(line) = mortar_image::map::SharedLine::of(t.plan) else {
            // `spec/cli.md` §4.2 refuses the `-Bsymbolic-non-weak*` forms
            // before the link.
            emit(Diagnostic::error(mortar_diag::Kind::InternalError {
                what: "a shared object's line with a refused -Bsymbolic form".into(),
            }));
            return None;
        };
        let soname = match (soname_bytes, soname_off) {
            (Some(b), Some(o)) => Some((b, o)),
            (None, None) => None,
            _ => {
                emit(Diagnostic::error(mortar_diag::Kind::InternalError {
                    what: "the soname and its .dynstr offset disagree".into(),
                }));
                return None;
            }
        };
        Some(SharedTables { soname, line, verdef })
    } else {
        None
    };
    let tables = DynTables {
        kind,
        copy,
        dynsym,
        rela,
        dynamic,
        interp,
        runpath,
        bind_now: t.output_cx.bind_now,
        relro: t.relro,
        sizes,
        note_mason: None,
        shared,
    };
    Some((got, tables))
}

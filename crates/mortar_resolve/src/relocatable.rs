//! Resolution at `-r` (`spec/resolve.md` §11). Owner: **RL-2** (front and
//! resolve at `-r`): RV-R1 to RV-R6, RV-R8, RV-R9, and RV-R7's election
//! and dropped copies (no `.eh_frame` work). Then RL-8 (RV-R7 in full,
//! `--force-group-allocation`) and RL-11 (RV-R5's `-d`, RV-R10's
//! `--defsym`), each after the one before has merged.
//!
//! A partial link asks resolution fewer questions than a final link
//! (§11): the same stage A runs (`crate::resolve::resolve_steps`: the
//! closure, `def`, the COMDAT keepers, the sections), and this module
//! then takes back what a final link adds to it:
//! - the linker-defined symbols (RV-R8): a key §8 would define stays
//!   undefined, and §8.2's "redefined" diagnostics are not raised;
//! - the verdicts (RV-R1): every key's is `Bound`, nothing is undefined;
//! - the demotion of hidden keys (RV-R6): a key's binding is its
//!   definition's (or its references', RV-R3);
//! - the sections a final link consumes and `spec/layout.md` L-R1 copies
//!   (`rl-sizes.md` S13): `.note.GNU-stack`, `SHF_EXCLUDE` sections,
//!   `.gnu.warning*`, an input's `.note.gnu.build-id`, `.deplibs`, the
//!   AArch64 build attributes get a placed kind and `Kept`;
//!   `.llvm_addrsig` and the call-graph profile stay dropped;
//! - the `.gnu.warning` diagnostics: the sections are carried to the
//!   final link, which warns (lld's and mold's form);
//!
//! and adds the refusals of `spec/cli.md` C-R5 and RV-R9 (a shared
//! object as input, no input of a known machine, a fat GCC LTO object).

use mortar_core::Inputs;
use mortar_core::inputs::DirectiveKind;
use mortar_core::inputs::Reader;
use mortar_core::par::Pool;
use mortar_core::plan::{OutputKind, Plan, SymbolRole};
use mortar_core::resolution::{DropReason, Resolution, SectionKind, SectionState};
use mortar_core::{FileId, OptId, SymbolId};
use mortar_diag::relocatable::{
    GCC_LTO_SECTIONS, NO_INPUT_OF_A_KNOWN_MACHINE, NO_INPUT_OF_A_KNOWN_MACHINE_NOTE,
    SHARED_OBJECT_INPUT,
};
use mortar_diag::{Diagnostic, Key, Kind, Place, RefusalClass};
use mortar_elf::consts::{
    SHF_ALLOC, SHF_EXCLUDE, SHF_LINK_ORDER, SHT_LLVM_ADDRSIG, SHT_LLVM_CALL_GRAPH_PROFILE,
    SHT_NOBITS, SHT_NOTE, SHT_PROGBITS,
};
use mortar_elf::{Machine, SectionIndex};

use crate::problem::{PHASE, Problem, What};
use crate::resolve::{Def, Resolved, flags, resolve_steps};
use crate::sections::{self, SectionRefusal};
use crate::steps::Steps;
use crate::tables::{diagnostics, options, resolution};
use crate::view::{SectionInfo, View};
use crate::{InputsView, Outcome};

/// The resolution of a `-r` link (`Plan.kind == Relocatable`): the table
/// without verdicts (RV-R1), the undefined and common keys kept (RV-R3,
/// RV-R5), hidden keys global (RV-R6). RL-7's driver calls it in place of
/// `resolve_inputs_with` at `-r`.
///
/// # Errors
///
/// The diagnostics that stop the link before a table exists, keyed in
/// phase 30: a shared object among the inputs (`unsupported feature:
/// shared object input with -r`, at each), and a line with no `-m` and no
/// relocatable object to take the machine from (`spec/cli.md` C-R5).
pub fn resolve(
    inputs: &Inputs<'_>,
    plan: &Plan,
    pool: &Pool,
) -> Result<Outcome, Vec<(Key, Diagnostic)>> {
    debug_assert_eq!(plan.kind, OutputKind::Relocatable);
    refuse_inputs(inputs, plan)?;
    let mut steps = Steps::default();
    let view = InputsView::new_steps(inputs, pool, &mut steps);
    let opts = options(plan, inputs, &view);
    steps.step("options");
    let mut r = resolve_steps(&view, &opts, pool, &mut steps);
    let mut extra = Vec::new();
    take_back(&view, &opts, &mut r);
    sections_at_r(inputs, &view, &mut r, &mut extra);
    steps.step("relocatable");
    let mut diagnostics = diagnostics(inputs, &view, &opts, &r);
    diagnostics.extend(extra);
    diagnostics.sort_by_key(|(k, _)| *k);
    steps.step("diagnostics");
    let internal = |what: String| {
        (Key::new(u16::from(PHASE), u32::MAX, 0), Diagnostic::error(Kind::InternalError { what }))
    };
    let resolution = match resolution(&view, &r, &opts) {
        Ok(mut t) => {
            bind_at_r(&r, &mut t);
            if let Err(e) = t.check().and_then(|()| t.check_against(inputs)) {
                diagnostics.push(internal(e.to_string()));
            }
            t
        }
        Err(e) => {
            diagnostics.push(internal(format!("{e:?}")));
            Resolution::empty(inputs.files.len(), inputs.symbols.len(), inputs.comdats.len())
        }
    };
    steps.step("table");
    steps.count("files", u64::from(inputs.files.len()));
    steps.count("live", r.file_live.iter().filter(|l| l.is_live()).count() as u64);
    steps.count("keys", u64::from(inputs.symbols.len()));
    steps.count("sections", r.sections.len() as u64);
    steps.count("rounds", u64::from(r.rounds));
    // The roots of §4.2 (`-u`, `--require-defined`): `--gc-sections` is
    // refused at `-r` (C-R4), so nothing reads them for collection.
    let roots: Vec<SymbolId> = opts.roots().iter().map(|root| root.key).collect();
    drop(view);
    Ok(Outcome {
        resolution,
        diagnostics,
        comdat_mismatch: r.comdat_mismatch,
        gnu_warning_syms: Vec::new(),
        retained: r.retained,
        entry: r.entry,
        roots,
        needed: Vec::new(),
        closure_only: Vec::new(),
        steps,
    })
}

/// RV-R3's `-u SYM` (and a script's `EXTERN (SYM)`) that no input
/// mentions, in argument order, once each. The input phase interns no
/// option's name, so such a `SYM` has no key and no row in the table:
/// the writer (RL-4) writes each as a `STB_GLOBAL` `SHN_UNDEF`
/// `STT_NOTYPE` entry of default visibility after the keys' entries (GNU
/// ld writes it; measured, record WP-182 §B). A `-u` of a key the table
/// has is in the table (`FROM_OBJECT`, S14) and is not listed here.
#[must_use]
pub fn undefined_without_key(plan: &Plan, inputs: &Inputs<'_>) -> Vec<Box<[u8]>> {
    let mut names: Vec<&[u8]> = plan
        .symbols
        .iter()
        .filter(|s| s.role == SymbolRole::Undefined)
        .map(|s| s.value.as_encoded_bytes())
        .chain(
            inputs
                .link
                .directives
                .iter()
                .filter(|d| d.kind == DirectiveKind::Extern)
                .map(|d| d.value),
        )
        .collect();
    let mut seen: Vec<&[u8]> = Vec::with_capacity(names.len());
    names.retain(|n| {
        let fresh = !seen.contains(n);
        seen.push(n);
        fresh
    });
    // As `InputsView::key_of` splits an option's name.
    let split = |name: &'_ [u8]| -> (Vec<u8>, Option<Vec<u8>>) {
        match name.iter().position(|&b| b == b'@') {
            Some(at) if !name[at..].starts_with(b"@@") => {
                (name[..at].to_vec(), Some(name[at + 1..].to_vec()))
            }
            Some(at) => (name[..at].to_vec(), None),
            None => (name.to_vec(), None),
        }
    };
    let wanted: Vec<(Vec<u8>, Option<Vec<u8>>)> = names.iter().map(|n| split(n)).collect();
    let mut found = vec![false; names.len()];
    for (_, key) in inputs.symbols.iter() {
        for (i, (name, version)) in wanted.iter().enumerate() {
            if key.name == name.as_slice() && key.version == version.as_deref() {
                found[i] = true;
            }
        }
    }
    names.iter().zip(found).filter(|(_, f)| !f).map(|(n, _)| Box::from(*n)).collect()
}

/// `spec/cli.md` C-R5: a shared object, named or found by `-l`, is
/// refused at each (GNU ld and lld: `attempted static link of dynamic
/// object`; mold drops it silently); with no `-m`, a line without a
/// relocatable object (an archive's members count) has no machine to
/// write (GNU ld writes its default emulation's, lld and mold refuse).
fn refuse_inputs(inputs: &Inputs<'_>, plan: &Plan) -> Result<(), Vec<(Key, Diagnostic)>> {
    let mut out = Vec::new();
    let mut object = false;
    for (id, file) in inputs.files.iter() {
        match file.reader {
            Reader::Shared(_) => out.push((
                Key::new(u16::from(PHASE), id.0, 0),
                Diagnostic::refused(RefusalClass::Feature, SHARED_OBJECT_INPUT)
                    .at(Place::File(file.display())),
            )),
            Reader::Object(_) => object = true,
            Reader::Internal => {}
        }
    }
    if !object && plan.emulation.is_none() {
        out.push((
            Key::new(u16::from(PHASE), 0, 0),
            Diagnostic::refused(RefusalClass::Feature, NO_INPUT_OF_A_KNOWN_MACHINE)
                .note(NO_INPUT_OF_A_KNOWN_MACHINE_NOTE),
        ));
    }
    if out.is_empty() { Ok(()) } else { Err(out) }
}

/// What a final link adds to stage A and a `-r` does not (RV-R3, RV-R8,
/// the `.gnu.warning` diagnostics).
fn take_back<V: View>(view: &V, opts: &crate::Options, r: &mut Resolved) {
    // RV-R8: no linker-defined symbol; the key stays undefined, weak when
    // every reference is (RV-R3, the rule of `ref_strong`).
    for (def, props) in r.def.iter_mut().zip(&r.props) {
        if matches!(def, Def::Provide { .. }) {
            *def = Def::Undefined { weak: props.flags & flags::REF_STRONG == 0 };
        }
    }
    // §8.2's names may be defined by an object at `-r`; `.gnu.warning*`
    // sections are carried, and the final link warns.
    r.problems.retain(|p| {
        !matches!(p.what, What::LinkerSymbolRedefined { .. } | What::GnuWarning { .. })
    });
    r.gnu_warning_syms.clear();
    // RV-R3: `-u SYM` with no definition is written as an undefined
    // global, as GNU ld does: it is a reference of the output
    // (`FROM_OBJECT`, rl-sizes.md S14). `--require-defined` of an
    // undefined key is refused (`required-symbol`, measured: GNU ld
    // refuses it at `-r` too); `-e` is not a root at `-r` (RV-R2).
    for named in &opts.undefined {
        if let Some(k) = named.key.get()
            && r.def[k.0 as usize].is_undefined()
        {
            r.props[k.0 as usize].flags |= flags::OBJ_REF;
        }
    }
    let _ = view;
}

/// The kind a section a final link consumes takes at `-r` (`spec/layout.md`
/// L-R1; rl-sizes.md S13): `Ok(None)` when it stays dropped
/// (`.llvm_addrsig`, the call-graph profile), else the placed kind by its
/// type and flags, or the refusal a final link gives a kept section of
/// that shape.
fn kind_at_r(
    machine: Machine,
    index: u32,
    s: &SectionInfo<'_>,
) -> Result<Option<SectionKind>, SectionRefusal> {
    if matches!(s.sh_type, SHT_LLVM_ADDRSIG | SHT_LLVM_CALL_GRAPH_PROFILE)
        || s.name.starts_with(b".gnu.lto_")
    {
        return Ok(None);
    }
    let plain = SectionInfo { flags: s.flags & !SHF_EXCLUDE, ..*s };
    if s.name.starts_with(b".gnu.linkonce.") {
        return Err(SectionRefusal::Linkonce);
    }
    if s.name.starts_with(b".zdebug") {
        return Err(SectionRefusal::Zdebug);
    }
    if s.flags & mortar_elf::consts::SHF_COMPRESSED != 0 {
        return Err(SectionRefusal::Compressed);
    }
    let kind = match sections::classify(machine, index, &plain)? {
        // `.note.GNU-stack`, an input build id, `.gnu.warning*`,
        // `.deplibs`, AArch64's attributes: by type and flags.
        (SectionKind::Stack | SectionKind::Excluded, _) => match plain.sh_type {
            SHT_NOTE => SectionKind::Note,
            SHT_NOBITS => SectionKind::Nobits,
            SHT_PROGBITS if plain.flags & SHF_ALLOC == 0 && sections::is_debug(&plain) => {
                SectionKind::Debug
            }
            _ => SectionKind::Regular,
        },
        // An `SHF_EXCLUDE` section of an ordinary type: placed whole.
        (SectionKind::Merge, _) => SectionKind::Regular,
        (kind, _) => kind,
    };
    Ok(Some(kind))
}

/// S13 over every live object's rows, and RV-R9's fat GCC LTO object.
fn sections_at_r(
    inputs: &Inputs<'_>,
    view: &InputsView<'_, '_>,
    r: &mut Resolved,
    extra: &mut Vec<(Key, Diagnostic)>,
) {
    let machine = view.machine();
    let slim: Vec<FileId> = r
        .problems
        .iter()
        .filter_map(|p| match p.what {
            What::LtoSlim { file } => Some(file),
            _ => None,
        })
        .collect();
    for f in 0..r.section_base.len() {
        let file = FileId(f as u32);
        let base = r.section_base[f];
        let (start, end) = (base.start.0 as usize, base.end.0 as usize);
        if start == end {
            continue;
        }
        let mut lto = false;
        let mut changed = false;
        for i in 0..(end - start) {
            let Some(s) = view.section(file, SectionIndex(i as u32)) else { continue };
            lto |= s.name.starts_with(b".gnu.lto_");
            let row = &mut r.sections[start + i];
            if !matches!(row.kind, SectionKind::Stack | SectionKind::Excluded) {
                continue;
            }
            let dropped = row.comdat.get().is_some_and(|c| {
                r.keepers.keeper.get(c.0 as usize).copied() != Some(OptId::some(file))
            });
            match kind_at_r(machine, i as u32, &s) {
                Ok(None) => row.state = SectionState::Dropped(DropReason::Consumed),
                Ok(Some(kind)) => {
                    row.kind = kind;
                    row.state = if dropped {
                        SectionState::Dropped(DropReason::Comdat)
                    } else {
                        SectionState::Kept
                    };
                    changed = true;
                }
                Err(_) if dropped => {
                    row.kind = SectionKind::Regular;
                    row.state = SectionState::Dropped(DropReason::Comdat);
                }
                Err(why) => {
                    r.problems.push(Problem {
                        key: crate::problem::DiagKey::of(file, i as u64),
                        what: What::Section { file, section: SectionIndex(i as u32), why },
                    });
                }
            }
        }
        // §6.5: an `SHF_LINK_ORDER` section now kept whose linked section
        // is a dropped copy is dropped with it.
        if changed {
            for i in 0..(end - start) {
                let Some(s) = view.section(file, SectionIndex(i as u32)) else { continue };
                let link = s.link as usize;
                if s.flags & SHF_LINK_ORDER != 0
                    && r.sections[start + i].is_kept()
                    && link != 0
                    && link < end - start
                    && r.sections[start + link].state == SectionState::Dropped(DropReason::Comdat)
                {
                    r.sections[start + i].state = SectionState::Dropped(DropReason::Comdat);
                }
            }
        }
        // RV-R9: a fat GCC LTO object (code and `.gnu.lto_*`); a slim one
        // is refused as in a final link (§1.2).
        if lto && !slim.contains(&file) {
            extra.push((
                Key::new(u16::from(PHASE), file.0, 0),
                Diagnostic::refused(RefusalClass::Feature, GCC_LTO_SECTIONS)
                    .at(Place::File(inputs.files[file].display())),
            ));
        }
    }
    r.problems.sort_by_key(|p| p.key);
}

/// RV-R1, RV-R6: no verdict (every key's is `Bound`), and the binding of
/// the definition (or of the references, RV-R3) whatever the visibility.
fn bind_at_r(r: &Resolved, t: &mut Resolution) {
    for (k, props) in t.props.as_mut_slice().iter_mut().enumerate() {
        props.verdict = mortar_core::resolution::Verdict::Bound;
        if let Some(binding) = crate::verdict::binding_at_r(r.def[k]) {
            props.binding = binding;
        }
    }
}

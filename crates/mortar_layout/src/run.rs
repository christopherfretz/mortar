//! The stages after liveness, in order (§7): `finish`, B, C, E (with D
//! inside), F; their result as a `Placement`.

use mortar_core::assignment::Assignment;
use mortar_core::column::Column;
use mortar_core::id::{OutSecId, SymEntry, SymbolId};
use mortar_core::inputs::SymbolKey;
use mortar_core::layout::{EntryDef, Folds, MergeMap, Placement, SegmentRules};
use mortar_core::liveness::Liveness;
use mortar_core::par::Pool;
use mortar_core::resolution::Resolution;
use mortar_core::split::Split;

use crate::assign::finish;
use crate::chunk::{ChunkInputs, Common, Staged, SynthChunk, chunk, start_stop_outputs};
use crate::diag::LayoutDiag;
use crate::fixed::{RangeExtension, fixed_point};
use crate::headers::Headers;
use crate::order::order;
use crate::rank::{Rank, rank_of};
use crate::stack::{StackNote, executable_stack};
use crate::symbols::{SymbolInputs, symbols};
use crate::target::{Options, Target};

/// The tables the stages after liveness read.
#[derive(Debug, Clone, Copy)]
pub struct Link<'t, 'n> {
    pub headers: &'t Headers<'n>,
    pub resolution: &'t Resolution,
    pub symbols: &'t Column<SymbolId, SymbolKey<'n>>,
    pub liveness: &'t Liveness,
    pub folds: &'t Folds,
    pub split: &'t Split,
    pub merge: &'t MergeMap,
    pub commons: &'t [Common],
    pub synthetic: &'t [SynthChunk],
    /// [`crate::stack::notes`].
    pub stack_notes: &'t [(String, StackNote)],
    pub target: &'t Target,
    pub options: &'t Options,
}

/// What [`place`] produces.
#[derive(Debug, Clone)]
pub struct Placed {
    pub placement: Placement,
    /// The rules `derive_segments` was run with (`CheckContext`).
    pub rules: SegmentRules,
    /// Stage C's output, for callers that look inside (tests, plumb).
    pub staged: Staged,
    /// `executable-stack`, `entry-not-found`.
    pub warnings: Vec<LayoutDiag>,
    pub rounds: u32,
}

/// The ranks of the output sections (L-60).
#[must_use]
pub fn out_ranks(assignment: &Assignment<'_>) -> Column<OutSecId, Rank> {
    let rows =
        assignment.outsecs.iter().map(|(_, o)| rank_of(o.name, o.sh_type, o.flags)).collect();
    Column::from_vec(rows).unwrap_or_default()
}

/// `finish`, then stages B to F.
///
/// # Errors
///
/// Every error of the stages, the first stage that fails giving them.
pub fn place(
    link: &Link<'_, '_>,
    assignment: &mut Assignment<'_>,
    range: Option<&RangeExtension<'_>>,
    entry_def: impl Fn(SymEntry) -> Option<EntryDef>,
    pool: &Pool,
) -> Result<Placed, Vec<LayoutDiag>> {
    let (target, options) = (link.target, link.options);
    let mut steps = mortar_core::wp032b_steps::Steps::new("place");
    let info = finish(assignment, link.headers, link.liveness, target)?;
    steps.step("finish", 0, "");
    let ranks = out_ranks(assignment);
    let ordered =
        order(assignment, link.headers, link.resolution, link.liveness, link.folds, &ranks, pool)?;
    steps.step("order", 0, "");
    let keep_empty = start_stop_outputs(assignment, link.resolution, link.symbols);
    let mut warnings = Vec::new();
    let (exec, warn) = executable_stack(link.stack_notes, options.execstack);
    warnings.extend(warn);
    let rules = options.segment_rules(target, exec);
    let inputs = ChunkInputs {
        assignment,
        info: &info,
        headers: link.headers,
        ordered: &ordered,
        folds: link.folds,
        split: link.split,
        merge: link.merge,
        liveness: link.liveness,
        commons: link.commons,
        synthetic: link.synthetic,
        keep_empty: &keep_empty,
        relro: options.relro,
    };
    let staged = chunk(&inputs, target, &rules, pool).map_err(|e| vec![e])?;
    steps.step("chunk", staged.chunks.len() as u64, "chunk");
    let fixed = fixed_point(&staged, target, options, &rules, range, pool).map_err(|e| vec![e])?;
    steps.step("fixed point", u64::from(fixed.rounds), "round");
    let cx = SymbolInputs {
        resolution: link.resolution,
        assignment,
        symbols: link.symbols,
        chunks: &fixed.addressed.chunks,
        order: &staged.order,
        ranks: &staged.ranks,
        out_of: &staged.out_of,
        place: &staged.place,
        sym_place: &staged.sym_place,
    };
    let syms = symbols(cx, target, options, entry_def).map_err(|e| vec![e])?;
    warnings.extend(syms.warnings);
    steps.step("symbols", 0, "");
    let placement = Placement {
        chunks: fixed.addressed.chunks,
        order: staged.order.clone(),
        segments: Column::from_vec(fixed.addressed.segments).unwrap_or_default(),
        place: staged.place.clone(),
        sym_place: staged.sym_place.clone(),
        defined: syms.defined,
        patches: fixed.patches,
        redirects: fixed.redirects,
        sized: fixed.sized,
        tls: fixed.addressed.tls,
        entry: syms.entry,
    };
    steps.step("placement", 0, "");
    Ok(Placed { placement, rules, staged, warnings, rounds: fixed.rounds })
}

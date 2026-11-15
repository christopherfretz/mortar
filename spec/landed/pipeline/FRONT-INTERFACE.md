# The front's interface (WP-027a → WP-027b/c)

What `mortar::front::front` returns: one value, `Front<'s>`, holding the final
`Layout` and every table the image phase (100) reads. Source of truth:
`crates/mortar/src/front.rs`. This file is kept current with it; a change is
listed at the end.

## The call

```rust
// crates/mortar/src/front.rs
pub struct Env<'e> { pub cwd: &'e Path, pub pool: Pool }

pub fn front<'s>(
    plan: Plan,                 // mortar_core::plan::Plan (the parsed command line), moved in
    env: Env<'_>,
    store: &'s Store,           // mortar_input::store::Store; the caller owns it
    diag: &Collector,           // mortar_diag::Collector: every phase's diagnostics
    timer: &mut Timer,          // crate::timing::Timer: --time-phases rows
) -> Option<Front<'s>>;         // None ⇔ an error was emitted into `diag`
```

- The caller (`lib.rs` `invoke_in`) owns the `Store`, the `Collector` and the
  `Timer`, and makes the `Pool` (`--threads`, `MORTAR_SCHED_SEED`). The store
  is `StorePolicy::from_env()` — Staged by default; `MORTAR_STORE=resident`
  switches. Image reads non-resident bytes through `copy_to` like every
  other reader (WP-021b), never assuming `Object::data` is resident.
- The image gets the same `&Collector` and `&mut Timer` from the caller (they
  are not inside `Front`): `Timer::phase(100, "image")`, `Timer::phase(110,
  "output")`, then `lib.rs` prints `Timer::finish()`. Timer wraps
  `mortar_image::phases::PhaseTimer` (image.md §11).
- Today `invoke_in` ends a successful front with the refusal
  `unsupported feature: writing the output file` at key (100,0,0); WP-027b/c
  replace that one `if front.is_some()` block (`crates/mortar/src/lib.rs`) with
  the image call.

## `Front<'s>`

`'s` is the store's lifetime: every borrowed name or byte slice points into
the `Store` (through `Inputs<'s>`). Everything else is owned. `Front` is not
`Clone`; image takes `&Front<'s>`.

| field | type | produced by | notes |
|---|---|---|---|
| `plan` | `mortar_core::plan::Plan` | 10 (cli) | owned; options the image reads (`shaping.build_id`, `strip`, `discard`, output path) |
| `pool` | `mortar_core::par::Pool` | caller | `Copy`; the link's threads and schedule seed |
| `arch` | `mortar_a64::A64` | 45 | `A64::new(Status::M1, options.fix_843419)`; use as `&dyn mortar_arch::Arch` |
| `target` | `mortar_layout::Target` | 45 | `Target::of(machine)`: class, page sizes |
| `options` | `mortar_layout::Options` | 45 | `Options::from_plan(&plan, &target)` |
| `inputs` | `mortar_core::inputs::Inputs<'s>` | 20 (`mortar_input::load`) | files, readers, symbol keys, `store: StoreRef<'s>` |
| `resolution` | `mortar_core::resolution::Resolution` | 30 (`mortar_resolve::resolve_inputs`) | `Outcome.resolution` |
| `comdat_mismatch` | `Vec<mortar_scan::Mismatch>` | 30 | scan's/fill's input |
| `gnu_warnings` | `Vec<(SymbolId, FileId, SectionIndex)>` | 30 | `.gnu.warning` symbols |
| `headers` | `mortar_layout::Headers<'s>` = `Column<SectionId, Header<'s>>` | 45 (`headers::gather`) | names borrow the store |
| `assignment` | `mortar_core::assignment::Assignment<'s>` | 45 + 90 | the assignment *after* `place` (stage A, L-50's flags, synthetic rows) — the one `Layout::validate` checked |
| `merge` | `mortar_merge::phase::Phase` | 60 (`phase::run`) | `split: Split`, `meta`, `eh: EhFrames`, `eh_relocs`, `whole` (S-6) |
| `order` | `mortar_merge::order::Order` | 50 (`order::offsets`) | `map` (== `layout.merge`), `chunks: Box<[MergedChunk]>` (one per output section with live fragments, `OutSecId` order), `order: Box<[FragId]>` (each chunk's fragments in offset order) |
| `liveness` | `mortar_core::Liveness` | 50 (`mortar_live::liveness`) | |
| `scanned` | `mortar_scan::Scanned` | 70 (`mortar_scan::scan`) | `demands: Demands`, `order: RecIndex` (moving to `mortar_core`, re-exported by `mortar_scan` — WP-027b), `diagnostics` (already emitted), `stats` |
| `eh` | `mortar_synth::EhPlan` | 80 | `.eh_frame` plan; `MergeEh::new(&front.merge.eh, &Tables{..})` rebuilds the borrowed view cheaply when writing |
| `synth` | `crate::front::SynthPlans` | 80 (+90) | below |
| `commons` | `Vec<mortar_layout::chunk::Common>` | 90 | the commons layout placed |
| `layout` | `mortar_core::layout::Layout` | 90 | `folds`, `merge`, `slots` (with `sizes`, `eh_records`, `symtab_locals`), `placement` — validated (`Layout::validate`) |
| `placed` | `mortar_layout::Placed` | 90 | `rules: SegmentRules` (for `CheckContext`), `staged`, `warnings` (already emitted), `rounds`; `placed.placement == layout.placement` |

```rust
pub struct SynthPlans {
    pub hdr: Option<HdrPlan>,        // .eh_frame_hdr: --eh-frame-hdr and a live FDE (S-57)
    pub got: GotPlan,                // .got, .got.plt, .iplt (Plt), .rela.iplt (RelaPlt)
    pub symtab: Option<SymtabPlan>,  // None under -s (S-84); built with the final patches (R-L5)
    pub shstrtab: ShstrtabPlan,      // the exact names of the placement (S-90)
    pub build_id: Option<BuildIdPlan>,
    pub property: PropertyPlan,      // .note.gnu.property (S-94)
    pub chunks: Vec<(SynthKind, ChunkId)>, // the synthetic chunks that exist, S-3 order
}
impl SynthPlans { pub fn chunk(&self, kind: SynthKind) -> Option<ChunkId>; }
pub const FIRST_SYNTHETIC: ChunkId = ChunkId(3); // after file header, phdrs, shdrs
```

Every `(kind, id)` in `synth.chunks` is checked against the placement before
`front` returns (the chunk at `id` is `ChunkKind::Synthetic` with that
kind's name).

Helpers on `Front`: `fn sections(&self) -> Vec<OutRow>` (the output sections
in index order: `shndx, name, sh_type, flags, addr, size, align`), used by
the differential tests.

## What `Front` does not carry (by design)

- the `Collector`, the `Timer`, the `Store` — the caller's (above);
- the scan's `View`/`Walker` and `CoreSyms` (`crates/mortar/src/syms.rs`, the
  `SymSource` the symtab plan reads) — borrowed views, rebuilt from the fields
  above when needed (`Walker::new(&arch, &ScanInputs{..}, ScanOptions{..})`;
  `CoreSyms::new(&inputs, &resolution, &assignment)`, crate-private today: say
  if image needs it public);
- `.comment` (S-100) — not yet a chunk (an internal-file `SectionId` is
  missing; see the record).

## Changes after first publication

(none yet)

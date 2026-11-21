# WIP WP-267 P-1b: INDEX and EDGES of the changed files without front 1

Brief: `/Volumes/AgentWork/mortar-helper/briefs/wp-267-p1b.md`. Branch
`wp/267-p1b`, worktree wp-156, base b941ff4b (`int/batch-25`).

## Step 1: the field list

Oracle: `update/changed.rs` `changed_files` (front 1): `index` from
`tables::index_file(inputs, link, ..)` through `file_rows_index_edges`,
`edges` the slice of front 1's `mortar_live::stored_edges(.., None, ..)`
(`front.rs` ~:677) whose source is one of the file's sections, both moved
into the update's ids by `translate` (keys by NAME: a position in
`ChangedFile::keys`; sections `first_section + k`).

Sources, P-1b's: **file** = the changed file's own bytes (`FileFront`,
`Ids::Update`); **post** = P-1's post-resolve facts
(`file_split::changed_facts(ff, view, plan, delta.keepers)`: kind, kept,
out, reloc; resolve's and layout's per-object bodies) and `split_of` over
them (the merge phase's own `units`/`cut`/`eh::parse`); **delta** = the
update's own `ResolveDelta` (`key_ids`, `resolved`); **OUTSECS** = the
stored output section rows (the name of `out`).

### INDEX (12 bytes: unit, kind, pad, target)

| column / input | front 1 | P-1b | source |
|---|---|---|---|
| which units: kept, `SHF_ALLOC` or kind `EhFrame` | `res.sections[id]` | `changed_facts` kept, kind; header flags | post + file |
| unit | `res.section_id(file, i)` | `first_section + i` | file |
| the relocation section of a unit | `res.sections[id].reloc` | `changed_facts` reloc (resolve's `reloc_fate` body) | post |
| kind 0/2 (global; slot by `arch.classify`) | record + `arch` | the same body | file |
| target of kinds 0/2 (a key) | `sym_map` + `redirect` (link `SymbolId`), then `translate`'s key position | the entry's key by (name, version) in `cf.keys` (P-1's numbering); `redirect` is `--wrap` only, refused by P-1 | file |
| kind 1/3 (local section; 3 when the target is `EhFrame`) | `res.section_id`, `res.sections[t].kind` | `first_section + i`, `changed_facts` kind | file + post |
| kind 5 (local slot) | record + `arch` | the same body | file |
| kind 4 (link order: `sh_link`) | header + `res.section_id` | the same body | file |

No column needs a whole-link pass. Shared body: `tables::index_body`,
called by `index_file` (whole link) and `index_file_of` (one object).

### EDGES (13 bytes: kind, source, target, offset), `--gc-sections` only

`stored` reads, for the rows whose SOURCE is one of the file's sections:

| datum | front 1 (`gc.rs`) | P-1b (narrow tables of ONE file) | source |
|---|---|---|---|
| kept (rows only of kept sources; F8 target kept) | `r.sections[s].is_kept()` | `changed_facts` kept | post |
| `ALLOC` bit | header | header | file |
| `ROOT` bit: `assignment.keep` | `assign`'s `keep_by_rule(h, out name)` | the same rule over the header and the `OUTSECS` name of `changed_facts`' out (**duplicated**: `keep_by_rule` is private to `mortar_layout`; ruling asked) | file + post + OUTSECS |
| `ROOT` bit: by type, `EhFrame`, no output, `startup_output(out name)`, `startup_input(name)`, link order | `file_facts` | `file_facts` itself over the narrow tables | file + post + OUTSECS |
| grouped (by type not a root) | `section_groups` (own file's groups, kept members) | the same body | file + post |
| `EDGES` bit | kind, `is_debug_name` | the same body | post + file |
| `MERGE` bit (`split.merge_of`) | the merge phase over the link | `split_of`'s pieces (the phase's `units`/`cut`) | post |
| `FDES` bit, frames (`by_code`, `eh_edges`) | `split.eh_of/eh_piece` of the link | `split_of`'s `eh.split_rows()` (an FDE's code outside the file: dropped, see below) | post |
| `LINKED`, link-order rows | own file | the same body | file |
| record targets (`record_to`): key | `sym_map` + `redirect` | narrow `sym_map` = the entry's position in `cf.keys` | file |
| record targets: section, offset (`piece_offset`, REL implicit addend) | own file + `arch` | the same body | file |
| row kind (`kind_of`: `START_STOP` by the key's name) | `inputs.symbols[k].name` | `cf.keys[p].name` | file |
| `ROOT`/`NAMED`/`SPLIT` rows | `file_rows` | the same body | as above |
| group ring | `group_of` over the file's groups | the same body (groups never span files) | file + post |
| `REFUSE` 1 (S5, `fde-global`) | own `.eh_frame` | the same body | file + post |
| `REFUSE` 2 (F11, `def-dropped`): keys whose definition is in a section of the file that is not kept | `assignment.def`/`r.def` over EVERY key | narrow `def` of the file's keys from `delta.resolved` (kind `SECTION`, definer the file; the entry found by its section); the F11 loop itself over the file's keys | delta + post |

No column stops. One case is not computed narrowly and is not needed:
an UNCHANGED file's FDE whose initial location names, through a
non-local symbol, a section of the changed file gives front 1 a `Frame`
row with that section as source. That FDE carries S5's `fde-global`
`REFUSE` row in the unchanged file's stored `EDGES`, and the update's
walk refuses on ANY file's `REFUSE` row (`update/live.rs` `walk`): such
an update never takes the rows. The shadow reports it as a finding if
it ever differs.

Id spaces: the narrow tables are a one-file link (`FileId(0)`, sections
`0..count`, keys `0..cf.keys.len()` = positions); rows translated back
as `translate` does (sections `first_section + k`, keys positions).

## Done
- step 1 (this table), c30dc278; steps 2-4, e0d062f7; P-1's test filter
  ff54eab9; step 5 tests e226e866; clippy fixes, spec section "P-1b" in
  `spec/landed/incr/WP-267.md` (lane counts and cost there).
- step 6: lane 636 PASS 0 FAIL, P-1b 348 equal / 0 finding / 0 refused;
  protoc `changed_edges_of` 0.065 ms; plain link unchanged.

## Not done
- step 7 (the full gate): the coordinator runs it at the merge.

## Ruling asked
- `keep_by_rule` is copied from `mortar_layout::assign` (private there):
  make it `pub` in `mortar_layout` (not mine) and call it.

## How to resume
Read this file and `git log --oneline b941ff4b..`; scratch is
`/Volumes/AgentWork/mortar-helper/.build/target-wp-156/p1b` (`t.sh <test>`,
`lane.sh`, `clock/clock.sh`, `clip.sh`).

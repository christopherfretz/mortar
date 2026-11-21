# WP-267 adversarial review (design, before code)

Reviewed: `spec/landed/incr/WP-267.md` at edfde095 (branch
`review/267-front1`), the rulings `briefs/wp-267-rulings.md`, the
hand-back `briefs/handbacks/wp-267.md`. The linker crates were read. I did
not read `crates/mortar_plumb`. The experiments were lane runs
(`cargo xtask incr-update`, with `MORTAR_UPDATE_FINDINGS` set) on
existing scenarios and on three throwaway scenarios. Copies of those are
in `.build/target-wp-143/r267/scenarios/`, and I removed them from
`tests/`. No crate code was changed. Phase A is not built, so no claim
below says that phase A produces a wrong byte. Each "broke" item is a
missing or circular guard, shown by file:line.

## 1. Circular judges

**Broke (R-A1). Phase A turns a binding judge into a shadow only: the
identity check. The narrow refill then rests on identity' alone.**
Ruling 3 says "No judge becomes shadow-only in phase A". WP-267 §2.1 row
"identity check" says "gone; identity' decides … shadow". Row 5 of §1
lists that check as a J (judge). Today `reference_of` (`mod.rs:417-433`)
compares the canonical form of a full front's capture with the stored
state, row for row (`identity`, `mod.rs:2224-2231`). This is the only
binding guard that says the narrow refill is enough. The narrow refill
covers the non-body chunks and the changed files' sections
(`mod.rs:392-399`), so the unchanged files' bytes are not rewritten. In
phase A an identity hop has these binding guards:
- front 2's `Layout::validate` on the replayed stored layout
  (`front.rs:1248`). It is structural: places, slots and commons against
  front 2's own resolution, liveness and demands (`check.rs:940-1001,
  1788-1811`). It is not about the bytes of unchanged sections.
- placed-as-computed. It checks set membership only, and validate already
  does that (below).
- U5 and R-4a against front 2's capture. Their domain is the update's own
  delta (next item).
None of them sees an unchanged referrer whose relocation value changed
because a key's resolution changed. If identity' misses one row of
§2.3's list, the result is a stale unchanged section in the output. The
state is still correct, because it is front 2's capture. So the state
says new and the image says old, which is the history's defect class.
Only the lane catches it (plumb stage 1, `run`). At a developer's desk
the shadow is off, because it needs `MORTAR_UPDATE_FINDINGS`.

The fix needs no front 1, and phase A already pays for its inputs. On an
identity hop front 2 runs with the stored placement and captures, so
"canon(front 2's capture) == canon(stored) outside STAT, LEAVES and the
changed FILES rows" is exactly today's check over a front that phase A
keeps. If it fails after identity' said identity, the fall-back is cheap
and correct: front 2's layout passed validate, so refill every chunk from
front 2. No second front is needed. Cost: the identity lap (156 ms) on
identity hops only. Alternative: always refill every chunk from front 2
(about 183 ms) and keep identity' for the state alone. Proposed rule
text for WP-267 §2.1: "identity check: binding in phase A, over front
2's capture; a disagreement with identity' is a FINDING and the hop
refills every chunk".

**Broke (R-A2). The U5 and R-4a judges take their domain from the
update.** `u5_judged` compares only `delta.resolved`, the keys the
update's own `resolve` calls affected (`changed.rs:503`, loop
`for (id, r) in &delta.resolved`). `facts_into` compares only `defined`,
the update's list (`mod.rs:2053`). A key that the update leaves out of
`affected` is never compared, so in that respect the judge reads its own
input. The values it does compare are independent of U4-U7: `st_value`,
`size`, `typ`, `bind`, `other`, and the definer's section by
(file, anchor, name, ordinal), all from front 2's own resolution
(`changed.rs:522-536`). Moving these judges to front 2 does not change
this. Front 1 had the same domain, and the identity check covered it
(R-A1). Proposed: the binding identity check of R-A1 is what closes this
domain. If R-A1 is not taken, a second check is needed: every key whose
front-2 `RESOLVED` row differs from the stored row is in
`delta.affected`. That costs O(state), which is the identity lap again.

**Held (inputs independent of U4-U7).** Front 2 builds its resolution,
liveness, demands, merge split and computed layout from the command line
and the inputs alone. The replay hook takes only the layout, after
`computed` exists (`front.rs:1220-1238`). `Layout::validate` reads front
2's `cx` (`front.rs:1196-1210`), not the update's. So placed-as-computed,
the placement half of `judge` and validate are true judges. The capture
of front 2 is not independent in its layout columns (place, holes, frag
offsets, slots, eh ranges, `.symtab` values): those are the replayed `lu`
by construction. IN-78 allows that, but no judge may compare those
columns with the update's own placement and call it agreement.

**Finding (R-A3). Placed-as-computed adds nothing to what front 2 already
checks.** `Rule::Places` (`check.rs:940-949`) already fails "a live
section has no place" and "a section that needs no place has one"
against front 2's liveness and kept flags, on the replayed layout
(`front.rs:1248`). That is §2.2's set equality, and it already binds
today. §2.2 should say so, or name what it checks beyond `Rule::Places`.
It is not a new guard to count in §6 R-3.

**Finding (R-A4). Materialize copies tables from `computed`.** These are
`merge.frag_off` as its base, `symtab_locals`, `sized` and `folds`
(`materialize.rs:702, 924-926, 1120-1144, 1105`). The output of a
materialized hop is therefore partly front 2's own answer, not the
update's. In phase A the hook makes `computed` front 2's (row 19). That
is correct, but those tables in `lu` agree with `computed` trivially. P-2's
`hook_materialize_equals_front_ones` and placed-as-computed must not
count them as evidence about the update.

## 2. U4 from the changed file alone (P-1)

**Held: `--wrap`.** The update refuses it by name. Reproducer:
`r267/scenarios/r267_wrap_caller_edit` (caller changed, `--wrap=foo`).
All hops give `update-not-implemented (scan) wrap: the link has --wrap`,
from `file_scan.rs:201-202`, on gcc-a64 and gcc-x64, and pass.
**Gap in the inventory:** WP-267 §1 does not list that refusal. P-1 must
keep it. Without it, 7d ("a function of the file's symbol table alone")
is false: the key a mention names depends on `--wrap`, as
`resolve.rs:614-617` applies it.

**Held as designed.** Archive extraction: M-1 refuses a changed archive.
The COMDAT keeper: 7b's keepers come from `resolve` (`resolve.rs:376-390`).
Commons and weak against strong: `resolve` re-elects them from the stored
rows. I did not construct an edit that reaches a from-the-file answer,
because P-1 is not written.

**Partly refuted, M-3:** `mortar_layout::stack::file_note(object)`
already exists per object (`stack.rs:63-72`). Only `takes_part`
(`file_live`) is whole-link, and a changed object is live unless it is an
archive member. `file_mentions` (`tables.rs:198-206`) reads
`inputs.sym_map_of(file)`, the whole link's key interning, which also
carries `--wrap`. `file5` (`capture.rs:1834-1935`) reads
`res.sections[..].comdat` and `inputs.comdats` for the file's own
sections. Both are per-object once the keys come by name (and `--wrap`
is refused). So M-3 is small, as stated.

## 3. Identity' (M-2)

**Not built, so no byte check is possible.** As evidence of which rows
must be in identity's predicate, I recorded the tables in which the full
link's capture differs from the stored state on today's non-identity hops
with "0 moved, 0 growth steps, 0 flips". These are 16 scenarios, gcc on
a64 and x64, 128 hops, all PASS (`r267/findings-same.txt`). The sets
seen were {32,33,45}, {32,34,45}, {1,8,10,11,21,56}, {1,32,43,45,50},
{7}, {11}, {23}, {11,23}, and larger. §2.3's list covers each set
through the changed files' rows, the affected keys, the FRAGS and
`.symtab` counts. **Held on the corpus.** One requirement must be stated
in §2.3: "no new FRAGS" does not cover a FRAGS row that changes without a
new row appearing (set {11} alone). That is harmless to bytes only
because the merged chunk is refilled and `layout.merge` is replayed.
§2.3 should name that reason.

**Broke by construction (R-A1), restated for line 3.** Identity' and
U5's judge both take their keys from `ResolveDelta.affected`
(`resolve.rs`, the doc of `affected`). One omission there therefore
makes identity' say identity and also leaves U5 silent. There is no
independent second answer.

## 4. Liveness without front 1 (M-5)

**Held: an unchanged file's root reaching a changed file.**
`r267/scenarios/r267_gc_init_array_unchanged`: u.o's constructor calls
c.o's `c_helper`, and the edit renames its section. Under
`--gc-sections`, the x64 updates are taken (gcc and kiln, both
directions) and pass. The a64 hops fall back by name.

**Broke (lane, not linker): a START_STOP revival.**
`r267/scenarios/r267_gc_start_stop_root`: the edit adds c.o's reference
to `__start_myreg`/`__stop_myreg`, which revives u.o's `myreg` sections.
The update falls back (`M2b-4: front 1's refill or capture: (image)`).
exit, verify, equiv and run all pass. But the lane's oracle FAILs on all
8 runs: `oracle: FAIL MISS output section myreg differs past its last
unit (8 bytes; new section) and only its body chunk is named`. This is a
harness defect (the dry-run oracle on a fall-back hop that creates an
output section), not a mislink. No scenario under `tests/incr-update`
puts START_STOP under `--gc-sections`.

**M-5 confirmed, larger than stated.** `stored_edges` calls
`gc::stored`, which calls `prepare(t, ..)` (`gc.rs:601-607`). `prepare`
runs `file_facts` for EVERY file (`gc.rs:343-345`) and `eh_edges` over
every `.eh_frame` row (`gc.rs:407-409`). The F11 pass loops over every
key of the link (`gc.rs:682-691`, `t.inputs.symbols.len()`,
`t.assignment.def`). The group ring needs `group_of` over the kept
members (`gc.rs:657-678`), and keeping is the COMDAT keeper, a
whole-link answer. A narrow `LiveInputs` therefore needs three things:
(a) the changed files' facts, eh rows and keys only; (b) the update's own
keeper and kept flags, from P-1's 7b, so P-1b depends on P-1, which §4
denies; (c) `LiveTarget::Key` ids in the state's id space. Fix §4: P-1b
"depends on P-1 (7b's keepers)".

## 5. The unconfirmed reads

- **M-3:** see §2. Confirmed small. The stack note is not missing.
- **M-5:** see §4. Confirmed. P-1b depends on P-1.
- **M-7, confirmed: the data exists when the hook runs. The hook's type
  does not carry it.** Front 2 has `merge` (with `.meta`), `merge_eh` and
  the first `eh` plan in scope before the replay. `eh` is built at
  `front.rs:765-787` with the incremental pads and is not rebuilt until
  after the hook (`front.rs:1270-1286`, only when
  `layout.slots.eh_records` differ). `eh.file_runs(&merge_eh)` at the hook
  therefore gives what front 1's `Incr5::eh` gives (`front.rs:1389-1391`).
  In a link without replay the rebuild never happens, and both rest on
  front 2's own liveness (`select_in`, `front.rs:763-764`). The `Replay`
  type is `dyn Fn(&Link, &Layout)` (`front.rs:318-320`) and is shared with
  IN-61's test hooks, so M-7 changes a signature that U-1 owns. One more
  case: `eh_runs` can be `Err` (two runs, not contiguous;
  `front.rs:782-786`). It must reach the hook as an error, by name.

## 6. (d0) (P-4)

What `incr_state compare S' G` misses, by construction:
- **Stale decision rows on every non-identity hop.** P-4 excludes
  SECTIONS.place, CHUNKS, HOLES, FRAGS offsets, SLOTS positions and eh
  ranges, because the update's placement may differ from G's fresh one.
  Three of the four historical defects were in rows like these, which
  only a capture of a full link with the update's placement (IN-78, (d)
  proper) can judge.
- **"Exact on identity hops" fails on any identity hop whose base was an
  update.** After a moved or grown hop, the stored placement is not G's
  fresh placement. (d0) then needs the same exclusions, and so misses the
  same rows. The lane already tracks `base_was_update` (43 of 128 hops in
  my run). P-4 must key exactness on it.
- **`FILES.round`:** `canon` writes it as 0 (`canon.rs:17, 84`). Today
  only F-10 judges it (`changed.rs:446-451`), and in phase B F-10 is a
  shadow. A stale round is invisible to (d0).
- **Fields that `canon` zeroes or ignores:** `META.compactions`,
  `build_id_len` (`canon.rs:80-82`), STAT, LEAVES.
- Rows the update writes that G does not have (none in phase A, where the
  state is front 2's capture) are seen only as set differences
  (`canon.rs:224-231`). Duplicates collapse there.

## Not tried

- Any build of phase A (identity', P-1, the narrow `LiveInputs`); so no
  byte claim about phase A.
- An i386 run (excluded by ruling).
- An FDE or LSDA flip under gc with a changed file. `gc_eh_owner_dies`
  already exists, and I did not run it.
- Common alignment raised in a changed file while another file's common
  wins on size. By reading, `Rule::Presence` (`check.rs:1802-1811`)
  checks `row.align >= c.align` against front 2's resolution, so a stale
  `sym_place` is refused.
- Symbol versions; linker scripts (M-6).

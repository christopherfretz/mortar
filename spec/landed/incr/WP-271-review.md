# WP-271 review: patch islands at the update's addresses (ADVERSARIAL)

Branch `review/271-islands`, at a1ff56ac (mortar 1bfce906 plus WP-271's
record). I wrote no code. The evidence is in
`.build/target-wp-145/r271/` (`x1/`: the sources, `run.py`, the logs, the
maps `x0.map`..`x2.map` and the findings). Mortar was built release with
`--features testhooks` from this head. I did not read the verifier's code.

**What blocks the design as written: R-1 and R-2.** Both are in the
"one round, nothing moves" claim and in the record's risk table, and both
need a rule change before code. R-3 to R-6 are amendments.

## Findings

| # | case | the bytes or the lines | rule it breaks | fix direction |
|---|---|---|---|---|
| R-1 | **A growth step moves islands, and the record handles L-118 only for an island that was empty.** IN-72 item 3 sets `step = round_up(need + max(4 KiB, size/16), A)`, where `A` is the largest alignment of the output sections that follow in the `PT_LOAD`. RX is not RELRO, so `A` is not the page size. A growth step of any exec output section before `.text` moves `.text`'s body and its island by `step`, which need not be a multiple of 4096. On AArch64 static that section is `.plt` (IPLT, packed; IN-72 item 4: "past it, a growth step") or `.init`. F-1's bound refuses this today (`moves_code`, materialize.rs:1214-1236: an exec `Body`/`Patches` chunk whose address changed). The record removes that bound. | protoc (`w271/island-bytes.txt`): `.text` has `Al 64`, and its island starts at `b28f40` (page offset 0xF40). `.plt` is `capacity .plt 0 s 666090 50 5a`. A step of `0x1000 + 0xc0` (need in (0x80, 0xc0], i.e. 9-12 more IPLT entries) puts the island's start at page offset 0x000. Every step whose rounded need ≡ 0xc0..0xc8 (mod 4096) does the same: about 1 step in 64 for this island. *Argued from the spec text and protoc's map; not reproduced as an update, because F-1 refuses first.* | L-118 / a64 §11.1 item 2: a non-empty synthetic code chunk at page offset 0xFFC-0x008 is verifier C5. It also voids §11.1 item 2's argument that no input `W1` runs into a chunk: the body's last `W1`/`W2` and the island's first `W4'` (a load/store, unsigned immediate) can make a site. Re-padding the moved island moves every patch in it, which contradicts "nothing moves". | Pick one: (i) keep F-1's bound for the growth-step case only (an exec chunk moved by a growth step: `update-not-implemented (layout)` by name); or (ii) on AArch64 with the option, round a growth step that moves exec chunks up to 4096. That is c1's idea applied to growth steps only. It costs at most 4 KiB per step, keeps every moved window's site set and every L-118 offset, and spares the O(moved) re-scan. The record's §2 cost paragraph ("a growth step that moves the rest of a part ...") must name the choice. |
| R-2 | **Risk 10's "caught by" is wrong: (d0) never compares `PATCHES` or `REDIRECTS` on a hop that appends.** | `tests/l10/judge.py:697`: `D0_PLACEMENT_TABLES = ('CHUNKS', 'ORDER', 'HOLES', 'PATCHES', 'REDIRECTS', ...)` is excluded on every non-identity hop (`d0_outside_placement`, judge.py:812-833). A hop that appends or holes a patch is never an identity hop, because G's fresh placement puts patches in key order at fresh addresses. `spec/harness.md:1959-1962`: "`P_U` is not read ... so no decision row is compared with the update's placement". The verifier finds redirects from the IMAGE (plumb.md §5.10 item 2: "a `B` to a patch that §5.3's W8 conditions accept ... is a redirect"), not from the state. So a state row that disagrees with the image the update wrote passes every check of that hop. | The history the brief names: a stored datum trusted after it went stale. The NEXT update builds on that row in two ways. It reuses a key's patch "if one exists" (§2 a step 3). It rewrites only the patches "whose redirect's section is refilled" (step 4). | Before the code lands, add the check the record assumes: on every update hop, the state's `PATCHES`/`REDIRECTS`/island `SIZED`/`HOLES` rows equal the update's own map (`chunk patch`, `redirect`, `capacity`, `hole` lines). That is `state_equals_capture_of_its_placement` for these tables only. It needs no G. Then rewrite §5 row 10's "caught by". |
| R-3 | **Fixes outside the dirty ranges are dropped silently.** The design is correct only if its window set is complete, and nothing in the image path notices when it is not. | In an update the front computes every fix over all exec words at `P_U` (front.rs:1340-1384: `erratum_fixes(&layout.placement)` on the replay). The refill fills only `want` (mortar_image/src/update.rs:203-283: units, files, moved, revived, holes, synthetic chunks, headers; never a fixup's address). `fill` applies only the fixups inside the range it fills (fill.rs:530-531, `partition_point` and `take_while(f.0 < hi)`). A fix at a word whose section is not refilled (an ADR at an unrefilled `W1`; a `b patch` at an unrefilled `F`) is therefore dropped, and no internal error is raised. Today it is unreachable only because of a layout guard (see "held", item 2). The phase-A full-scan compare in the record compares SITE sets, so it would not see this: both scans agree on the site, and only the write is lost. | IN-75's invariant ("the dirty ranges cover every byte in which the update's output differs") is enforced for the ranges the update names. A fix the front computed outside them leaves the previous image's word in place without a diagnostic. On a missed `W1` that is an unfixed 843419 site: wrong only on Cortex-A53, and caught only by C1 in the lane. | Make it an invariant in the image, not a hope in the planner. In `refill`, every fixup (`Link::fixups`) whose word lies outside `want` must equal the previous output's word there; otherwise `internal-error` (or `update-not-implemented (image)` by name). It costs O(fixups) reads of 4 bytes. Also make the phase-A lane compare compare the FIX sets (address, word) of the full scan with those the dirty scan writes. |
| R-4 | **The record's lane scenarios cannot append: a nolibc `.text` island has capacity 0.** | `x1/x0.map` (nolibc, `--fix-cortex-a53-843419 --incremental=full`): `capacity .text 0 b0 420120 f30 f30` and `capacity .text 1 p0 421050 0 0`. `.text` is the last chunk of the RX `PT_LOAD`, so L-87/IN-72 item 3 give it no tail, and the tail is where the island's capacity comes from (record §1.1). With glibc, `.fini` follows `.text` and the island gets the tail (protoc: `4c2f4`). | §4's `e843419_site_appears`, `_target_moves`, `_neighbour_window` and `_changed_file_patch_hole` as "nolibc `.S`" would all be `island-exhausted` (or ADR-only), not the appends they claim to prove. | Give those scenarios an exec output section after `.text` (an input `.fini` contribution, or glibc-static). Keep one nolibc scenario as the capacity-0 `island-exhausted` case. Each scenario should also check that its hop is `update`, not a fall-back (an `expect`-style line). |
| R-5 | **The island cap `C` is not the island's capacity.** On a large `.text` the island holds `.text`'s whole tail, `max(4 KiB, size/16)`, which exceeds `C = 4 MiB` once `.text` is over 64 MiB. That is the operator's target shape. | Arithmetic from IN-33 and L-112. A patch appended past offset `C` of the island is outside L-112's bound (`S + 16 + 3 + C − 12 < 2^27 − 4`). §2 a step 3 says "L-112 guarantees it". The full link refuses an island over `C`; the update has no such cap. | State it in the replan: an append is taken only if the island's end stays ≤ `C` AND the exact `s(28)` test of both branches passes; otherwise `island-exhausted`. Add a unit test at `C`. |
| R-6 | **The bytes of an unused patch: spec, image and update disagree.** a64.md §11.2 says an unused patch is written "as if it were used". The image writes it as zeros: `link.rs:509-532` zero-fills islands, and erratum words come only from `fixups`, which `erratum_fixes` builds from redirects only (sites.rs:417-425). In an update that drops a redirect (§2 a step 4: "the old patch stays unused"), the island is not in the refill set: IN-27's row covers a patch "added, moved or removed", not a redirect dropped. So the update keeps the old `W4'`/`b F+4` words, where a full link of `P_U` writes zeros. | Code lines above; not run. | IN-78/IN-75: the update's bytes must be those of the full link of its placement. The difference is harmless to run (nothing branches there, and C6/§11.3 item 6 allow unused patches), but every byte-compare oracle on such a hop will report it. | Pick one meaning (zeros, i.e. `UDF #0`, which IN-3 calls the trap fill, is the simpler) and write it in a64.md §11.2. Then make a dropped redirect dirty the patch's 8 bytes (one more line in §2 a step 4 and in IN-27's patch row). |

Smaller wording points, with no reproducer:

- §2 a step 3 ("the chunk's start moves forward by the L-118 padding ...
  inside its capacity") must say that the capacity's END is fixed, so the
  capacity shrinks by the padding. With protoc's numbers, a start moved by
  16 with the capacity kept would overlap `.fini` (b28f40 + 4c2f4 =
  b75234 = `.fini`).
- `check.rs:1664` requires `placement.patches` sorted by (chunk, offset),
  and `REDIRECTS.patch` is a row index (check.rs:1711). So an "append" to
  island k is an insertion in the vector whenever a later island has
  patches, and the "hole" of step 4 removes a row. Both renumber every
  later redirect and shift the S-80 symbols, which are in `PATCHES` order.
  The record says the tables are "written whole", so this is allowed, but
  the cost line O(P_touched) is really O(all patches and redirects).

## What held

1. **One round, for an append or a hole in an island that does not move.**
   An island's words cannot form a site:
   - its only ADRP is a thunk's, followed by `add` (a64 §11.1 item 1);
   - a patch is `W4'` (a load/store, never ADRP) then `b`;
   - a `Fill` hole is zero (`UDF #0`) (link.rs:509-532; IN-3).

   `.text`'s `sh_size` and the section header table are re-derived from
   the placement and written whenever they differ
   (mortar_image/src/update.rs:262-283, `headers`; IN-75). `.symtab` is
   non-allocated. None of these moves an address.
2. **The dirty-window set across files.** A window cannot span two files'
   regions in an incremental-mode output:
   - every region has at least 64 bytes of slack (IN-33), written as trap
     fill;
   - the delta refuses a touched region that ends at the next region with
     no hole: delta.rs:1437, "I, kept: a touched region with bytes
     followed by another".

   Shown in `x1/`: P (`p.o`) grown into its 0x8c region by 4, 0x18, 0x34
   or 0x38 bytes is an update. Grown to exactly 0x8c, so that it would
   abut Q's region at 0x421004, it is `update-not-implemented (layout)`
   (`x1/findings-fullcap-hop1.txt`). That is exactly the case where R-3's
   dropped fix would bite: `W1` in an unrefilled P, `F` in Q.

   So every window lies inside one file (refilled whole, IN-27 row 1) or
   inside sections that a growth step moved (refilled, IN-27's moved row).
   The record's neighbour row is therefore only reachable at chunk
   boundaries. It is not wrong. The invariant it rests on should be named
   in IN-72 item 9, so that lifting delta.rs:1437 later re-opens R-3
   knowingly.

   A run moved by exactly a page is refilled (moved), so `Page(V)`
   changes are judged. `.init`/`.fini` islands have capacity 3 and 0
   (protoc), so any site there needing a patch is `island-exhausted`; an
   ADR still works.
3. **The judge's words.** In phase A the fixes are recomputed from fill's
   words at `P_U`, never from the previous image:
   - `LinkSites::code_words` → `section_code` fills from the inputs
     (sites.rs:312-391);
   - front.rs:1340-1384 runs `erratum_fixes` again on a replay.

   The image's old `b patch` or ADR never reaches the judge on that path.
   Phase B (C') was not examined: it does not exist yet. R-3 is the guard
   it needs.
4. **`check.rs` admits a `Fill` hole inside a `Patches` chunk.** `holes()`
   (check.rs:1042-1066) checks only the bounds, the order and overlap with
   places, whatever the chunk kind. `islands()` (check.rs:1731-1770) sorts
   the keys itself.
5. **Key order (L-115 scoped to a full link).** No judge relies on it:
   - `check.rs` checks address order and one key per island;
   - plumb.md P-E3 item 4 lets `thunk`/`patch` chunks differ between maps;
   - §5.10 finds redirects from the image;
   - S-80 orders symbols by `PATCHES` order, which the placement records.

   The scoping is safe.

## What I did NOT try

- **Item 6, the 15 hops.** I did not find what refuses after `sized()`:
  that needs a code change to pass F-1. What I can say:
  - the 9 w8/w10/w11 hops are nolibc with island capacity 0 and no
    option, so after F-1 they depend only on non-erratum paths;
  - the 6 glibc hops reach front 2's replay, where `erratum_fixes` runs
    over all of `.text` at `P_U`. A new site there needing a patch has no
    redirect and is an internal error today (sites.rs:474-482), so the
    replan must run before it.
- R-1 as a real update: F-1's bound refuses first, and building a
  `.plt`/`.init` growth step at a chosen page offset was over my time box.
- R-6 as bytes: not linked.
- Thunks: no link with an RX span over 128 MiB (the record did not try
  one either).
- Any phase-B design (C'), the lane (`cargo xtask incr-update`), and the
  verifier on my links (`cargo xtask plumb`). My links were judged by
  map lines and update reports only.
- A window that crosses two exec OUTPUT sections (`.text` → `.fini`
  directly in a plain link): §11.1 scans per output section. That is
  outside WP-271 and pre-existing.

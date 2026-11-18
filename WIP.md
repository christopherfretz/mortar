# WIP: WP-216 (U-4) the delta layout — delete in the final commit

## Plan
1. `crates/mortar_layout/src/delta.rs`: fill `layout` (private helpers only).
   Read whole tables, infer `SegmentRules` (re-derive must reproduce the
   stored segments, else `state-corrupt`), classify each change (ignore /
   region / fixed / refused-by-name), keep / place / overflow per
   (file, O, c), overflow regions in FileId order at O's last chunk, growth
   steps per PT_LOAD run, holes = every free range of a touched region,
   segments re-derived, moved/killed/revived by identity.
2. `crates/mortar_layout/tests/delta.rs`: World-harness first links
   (slack on) captured into `MemoryLayout`; overlay the delta and run
   `Layout::check` on the edited World; a counting `LayoutRows` wrapper.
3. M.1 fixture (strutil objects) as committed text + extraction script.
4. Gates (background script), `spec/landed/incr/WP-216.md`, hand-back.

## Done
- (see git log on wp/216-incr-delta-layout)

## Resume
- Read this file, `git log --oneline -10`, then continue the first unchecked
  step above. Ruling items so far: items 5/6 (no pieces/ranges in
  `SectionChange`), item 11 (no SymbolId; commons after .bss inputs in the
  full link), sparse `placement.place`/`holes` vs IN-18c, no slot demand
  input, regions opened in FileId order (no rank), the category tail is O's
  tail, AArch64 tail lives in the last island.

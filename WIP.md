# WP-032i — work in progress (deleted at the end)

Scratch: `/tmp/mortar-wp032i-3693169` (`ident.sh <bin> <tag>`: 78 links;
`bench.py`: interleaved runs). Base binary = 2297dac; identity
`bd9642e9c9bd2b8d` (combined line hash of ident-base.txt).

## Done
- Item 1 scan side: `need::branch_candidate`, `Scanned.branches`
  (`BranchRec`), collected in `driver::walk_unit` for live code.
- Item 1 driver side: `LinkSites::new` reads `SiteInputs.branches`; the old
  walk kept under `cfg(debug_assertions)` as a check.

## Next
- Identity of item 1; measure; commit (scan side and driver side apart).
- Item 2: profile erratum needs alone.

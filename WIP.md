# WP-032b work in progress (deleted in the final commit)

## Done (commits on wp/032b-perf-scan)
- e9f6566 steps timers (`MORTAR_STEPS=1`); 9fe02ad scan units from headers;
  ff95a4d facts once / overlaps skip; c92d0d3 a64 classify table;
  9fa820a merge tagged table + counting-sort column; 1c6f16f marks index;
  e220887 eh reloc cursor + CIE binary search; c5178bb merge main 78dd0f9.

## Next
- final profile / e2e vs mold, lld (running in /tmp/mortar-wp032b-294675)
- record spec/landed/perf/WP-032b.md; gate; delete WIP.md; rm /tmp dir.

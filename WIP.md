# WIP — WP-244 (format-5 capture from a real link)

Branch `wp/244-incr-capture`, base 01f70788. Scratch:
`/Volumes/AgentWork/mortar-helper/.build/target-wp-143/wp244/`.

## Done
- Items 1-6 and tests (commits a9745afb, 23011441 seam 6, 6bbef55b).
- F-1 fix: normalize renumbers RESOLVED.section (3ec9cca7).
- Experiments run: protoc + cxx-large-g (wl/*/), M.1 + slot + kiln/hod
  series on both targets (series.log, stab.log, facts.log, kh.log).
- Record `spec/landed/incr/WP-244.md` drafted (cost line pending).

## Next
1. plaincost.sh results (wl/cxx-medium-static-cost/medians.txt) into the
   record.
2. `gate.sh` (fmt, clippy, quick, linktest, census, static-identical);
   commit the record, delete this file.

## Resume
`gate.sh` writes `gate/summary.txt`, final line "GATE DONE".

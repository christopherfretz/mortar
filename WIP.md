# WIP: WP-187 (RL-7), the -r join

Brief: `briefs/wp-187.md` + `briefs/rl-common.md` in the context repo.
Scratch: `.build/target-wp-154/rl7/` (cargo.sh = cargo with env + load wait;
lane.sh, census.sh, bb.sh = gates in the background; compose.py = P-P1 by
hand over a kept lane run; tools.py/tools2.sh = tool warnings vs GNU ld;
m1/m2/m3 = measurements: stack note, .gnu.warning, symver).

## Done
- Driver, refusals lifted, records' items, lane tests, texts (commits
  3645fc53, 5589e53b and the next).
- Lane run-715813: 63 PASS, 0 FAIL, 1 XFAIL (WP-203). Stage R: no finding.
- P-P1 by hand (compose.py): all clean but r-kernel-style(-m) via [mortar]:
  36 findings, merged-string "" dedup (a verifier question, INTERIM).
- Tools: no warning on mortar's -r output that GNU ld's does not get.
- BusyBox gcc + clang-22: -r by all four, finals by GNU ld and mortar run;
  busybox --list 401 applets.
- Census 1: C++ AArch64 -r lines hit an internal error (group check before
  the refusal): fixed (kept_group by file liveness), test
  r_section_groups_refused; Mason container key named (RL-12).

## Next
1. Census 2 (t4.log): confirm the split; commit.
2. INTERIM hand-back to the supervisor (verifier findings list).
3. After resume: final gates (ci --quick, whole lane, census,
   static-identical --update), record §E/§F, delete WIP.md, hand-back.

## Resume
Read this file, `git log`, then continue at the first Next item.

# WIP: WP-040r (spec requests of M2)

Done: items 1-3 (every `dyn.md` §11 request applied, §15 rulings written,
consistency pass); record `spec/landed/dyn/WP-040r.md`.

Done also: the citation check of `dyn.md` (about 120 citations fine;
14 fixed in the follow-up commit; D-48's TLSDESC bullet vs x64 X-28 left
as a question). main merged (WP-030c).

Half-done: item 4 edits in x86.md, arch.md, layout.md (uncommitted at
this commit).

Learned, not yet written into a spec (questions for the supervisor):
the new diagnostic kinds are not in `diag.md`;
thread-local weak undefined in a PIE (D-25 vs a64 §7); x64 X-26's
unmatched GD against an output-defined key; `PltVariant::Now` vs BTI;
the Q1 ruling vs D-11 computed after `def`; D-1 "[M4 end to end]" vs D-2
"static-pie gate M2".

Next: run `MORTAR_REQUIRE_TOOLS=1 cargo xtask ci --quick` in the
background; wait for the supervisor's "WP-030c on main", `git merge
main`, then item 4 (x86 X86-24, §15.1 rows, synth S-59, WP-030c R-12 and
R-15) as a separate last commit that also deletes this file.

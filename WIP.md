# WIP: WP-040r (spec requests of M2)

Done: items 1-3 (every `dyn.md` §11 request applied, §15 rulings written,
consistency pass); record `spec/landed/dyn/WP-040r.md`.

Half-done: the citation check of `dyn.md` (every external rule id it
cites) runs in a read-only helper; its fixes, if any, go in a follow-up
commit.

Learned, not yet written into a spec (questions for the supervisor):
D-46 vs D-49 (RELA `JUMP_SLOT` word); D-49's i386 lazy target is
entry + 6, not PLT0; the new diagnostic kinds are not in `diag.md`;
thread-local weak undefined in a PIE (D-25 vs a64 §7); x64 X-26's
unmatched GD against an output-defined key; `PltVariant::Now` vs BTI;
the Q1 ruling vs D-11 computed after `def`; D-1 "[M4 end to end]" vs D-2
"static-pie gate M2".

Next: run `MORTAR_REQUIRE_TOOLS=1 cargo xtask ci --quick` in the
background; wait for the supervisor's "WP-030c on main", `git merge
main`, then item 4 (x86 X86-24, §15.1 rows, synth S-59, WP-030c R-12 and
R-15) as a separate last commit that also deletes this file.

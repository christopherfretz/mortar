# WIP WP-042 (M2-CLI)

Done: Plan.dynamic.{init,fini} + helpers (plan.rs); Lands::Init/Fini; today's row
changes (--hash-style partial gnu-only, -z notext refused, -no-pie supported,
--no-copy-dt-needed-entries noop); AT_JOIN table + Row::status_at_join +
Parsed::refused_uses_at_join; options.md join section; tests table.rs, dynamic.rs.

Next: record spec/landed/dyn/WP-042.md; merge main once WP-051 lands (xfails tagged WP-042); gate.
audit edits in crates/mortar/tests/options.rs (hash-style Check, -z notext out,
--no-copy-dt-needed-entries Same, -no-pie Check); spec/cli.md row-status text and counts;
record spec/landed/dyn/WP-042.md; gate.

Learned: GNU ld and mold drop an exact repeat of an -rpath entry, lld keeps it
(D-30 row 2 finding). GNU ld/mold write nothing for -Bsymbolic in a PIE; lld DF_SYMBOLIC.

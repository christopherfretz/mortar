# WP-143 work in progress (deleted in the final commit)

Done: step 0 (reloc.rs), M10 (fate.rs), empty sections (locate.rs M4,
Target::Empty), Y3 missing/empty/placeless (symbols.rs), spec/plumb.md
amendments, tests/placeless.rs + fixtures, landed record draft.

Now: gates — full linktest (running), then ci --quick with
MORTAR_REQUIRE_TOOLS=1, then census. Then fill "Gates" in
spec/landed/plumb/WP-143.md, delete this file, hand back.

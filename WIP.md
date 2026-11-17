# WP-143 work in progress (deleted in the final commit)

Root cause (confirmed on the reproducers): a global reference whose only
input definitions are in sections without a place falls through
plumb.md §4.4 steps 1-4 to `symbol-unresolved`, a not-checked count, so
R5 (§5.7) is never reached for a global. Step 2 filters to placed
definitions; nothing looks at the unplaced ones.

Plan:
1. reloc.rs resolve_any: a name all of whose input definitions are in
   sections without a place resolves to that section (R5 / tombstone).
2. fate.rs: the reasons a section may lack a place (mortar's map); M10.
3. Empty sections: a foreign row of an empty mergeable section is a
   place; M4 for empty sections; an empty merged section's candidates.
4. Y3: a global of a placed or empty section missing from .symtab.
5. Tombstone only where the absence has a reason (mortar's map).
6. Sweep for siblings; tests; spec; landed record; gates.

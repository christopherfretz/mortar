# WIP: WP-048 (M2-X64)

Done:
- `X64 { max: Status }` (`M1`, `M2`, `new`); M2 hooks: `write_plt` PLT0/entries, `lazy_target`
  (entry + 6), `dyn_tags` (none), `weak_use` (X-57 + branch-opcode test).
- scan: `dynamic_cell` (D-43 copy in pie, D-44 canonical-plt-in-pie, X-57 PC32 branch vs weakdyn
  → PLT, D-47 text-relocation for shlib/weakdyn); TLS: GD→IE, TLSDESC→IE, unmatched GD kept (TLS_GD).
- apply: N16/N22/N1, unmatched GD field, G5/G6 in exec, PC32→PLT for weakdyn, D-46 place 0.

Next:
- tests/dynamic.rs (unit: cells, weak_use, PLT bytes, TLS bytes).
- tests/dyndiff.rs: PLT + TLS byte oracle vs GNU ld and lld; census of TLS forms.
- record spec/landed/x64/WP-048.md; gate.

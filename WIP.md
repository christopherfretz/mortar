# WIP — WP-050 (verifier stage 3)

## Done
- Step 0d (03d2154, on main as d3bde0d): map dynamic records.
- Stage 3: dynout, line, dynmodel, loader, dynverify, dyncheck, dynmutate;
  xtask plumb `--line FILE`, `--mutate` runs stage 3 (`mutate::run_with`).
- tests/dyn.rs: 20 cases (hello, E1, W1-W13, H1, DUP, F2, PD2, PD12), pass.
- Mutations (16/class, 5 E1 outputs; script /tmp/mortar-wp050-182487/mut.sh,
  result mut5.out): every dyn survivor classified (dead PLT entries, x86-64
  GLOB_DAT addend, versym 0->1, order swaps masked by baseline H1).
- Cost measured (release; cost.sh/cost.out in scratch).

## Next
- Record spec/landed/plumb/WP-050.md; gate `MORTAR_REQUIRE_TOOLS=1 cargo
  xtask ci --quick` (background); delete WIP.md; rm -rf /tmp/mortar-wp050-182487.

# WP-166 WIP (verifier: linker-defined exports, D-110 item 2 / D-21 item 2)

Done: shared.rs SEVEN + D-110 item 2 export derivation; dynverify file0
keeps the seven (exec); dynmodel Key.linker; dyncheck pd7 -E/list clause
and converse (Exec/Pie only); reloc internal_value (segment-end keys);
anomaly A57 (mold), A58/A59/A60 (GNU ld), spie needs DF_1_PIE.
Fixtures tests/ldef (gen.sh), tests/ldef.rs survey (all clean / named).

Next: mutations in tests/ldef.rs; lane test headers (linker-defined-export
xfail -> anomaly(mold): A57; exec test xfail(bfd,aarch64) gone, add
anomaly(bfd): A60 for prog-e0); new lane test tests/link/dyn/dynsym/
linker-defined-def-exec (api_end()==_end); spec/plumb.md; record; gates.
Unrelated finding: Y5 __bss_start false on outputs without .data (RELRO pad).

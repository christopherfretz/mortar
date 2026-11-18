# WIP: WP-228 (G-1), debug information in an incremental update

Step 1 (design with experiments) in progress; step 2 waits for the
supervisor's ruling on the record `spec/landed/incr/WP-228.md` "Step 1".

Done:
- item 1's survey: `tests/incr-design/wp228/survey.sh` (8 compiler-targets x
  DWARF 4/5 x 6 flag sets), `elfdbg.py` (ELF/DWARF reader).
- `place.py` (hand-made neutralization of a dead file's contributions, the
  edited map for plumb), `emulate.sh` (scenarios S1-S5 x variants, checks:
  readelf, llvm-dwarfdump --verify, gdb, lldb, xtask plumb),
  `measure.py` + `protoc.sh` (item 5).

Next: run emulate.sh on all AArch64 configs, x86-64 static; protoc.sh;
measure.py --list on cxx-large-g (build/large-inputs.txt); write the record.

Resume: scratch `/Volumes/AgentWork/mortar-helper/.build/target-wp-154/wp228`
(bin/ld -> release mortar; xtask at ../debug/xtask). Run scripts in the
background with a load wait (< 12).

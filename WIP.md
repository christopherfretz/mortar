# WIP — WP-028e (plumb i386)

Scratch dir: /tmp/mortar-wp028e-405572 (remove by explicit path at the end).
Logs: /Volumes/AgentWork/mortar-helper/.build/wp028e-logs/

## Done
- Item 0: `src/x86op.rs` (ModRm, disp32_reg, alu, imm_form, CALL/JMP_DISP32,
  ENDBR64, le); x64rec/x64/ifunc use it. x86-64 tests unchanged (all green).

## Next
- Item 1: src/x86.rs table, src/x86rec.rs, Form::AbsA, word-size slot_ok.

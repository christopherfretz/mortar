# WIP: WP-223 (H-1), the judge of an incremental update

Deleted in the final commit.

- Ruling (supervisor, 2026-11-17): a64 + x64 required; i386 only where the
  same scenario files run for free; i386-only failures parked in one line.
- Done: oracle patches (WP-067 §7.2); judge_step's IN-79 path; duty.py;
  selftest units; lane.py; 2 scenarios; 9 subjects; testhooks build (33.6 MB
  binary, 161 MB dir).
- Next: elflint/flip/wrong_local subjects; remaining scenarios; xtask
  incr_update.rs; harness.md + IN-78 sentence; record; gates.
- Probe finding: plumb verify catches every corruption elflint catches that
  was tried (OSABI change: neither).

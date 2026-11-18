# WP-200 rework (supervisor's rulings Q1-Q4)
Done: demoted mentions never edges (live.rs, resolve.rs), OBJ_STRONG not set
by a demoted mention; spec §4.1 procedure, §6.2, dyn.md D-11; unit tests;
strong/unique-demoted now refusal .test files.
Next: measure tests/link/resolve/comdat-mismatch-strong-demoted-needed.test
(draft) on 4 linkers; timing (release base vs after, protoc static, 7 runs
alternating, scratch .build/target-wp-140/wp200/bench); record update; gate.
Resume: .build/target-wp-140/wp200/gate.sh in the background.

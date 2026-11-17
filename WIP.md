# WP-118 work in progress (deleted in the final commit)

- Done (uncommitted -> committing): DynKind::Shared(SharedDyn) + D-102/D-103
  rows; shared_init_key; dynstr_plan_shared (soname after needed);
  dynsym_plan_shared + Form::Undefined; GotPlan::new_shared (TLS/IPLT
  refused), Preemptible GLOB_DAT + zero slot; zeroed_place + reloc.rs
  Preemptible; DynPlans.shared (SharedPlans), map `output shared` + line;
  layout target.rs Shared = PIE, EntryName::IfDefined (D-101).
- Measured: GNU ld/lld/mold RELRO end at 64 KiB: 0x20000 / 0x21000 /
  0x30000 (scratch .build/target-wp-118/measure).
- Next: tests shared_*.rs (synth, layout, image GNU-ld comparison),
  record spec/landed/dyn/WP-118.md, merge origin/main, gate.

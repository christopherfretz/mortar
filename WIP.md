# WP-032h WIP

Done:
- store wave A: descriptor table grown before a batch (no RCU grace period
  per growth) + pages of fresh read buffers touched before the read
  (`crates/mortar_input/src/store.rs` `make_room`, `prefault`). Identity
  set (54 links) identical to b6cf648.

Measured, not written: hello-cxx "members as symbol tables" upper bound
~1.15 MB resident (2.1 MB if small members were split too) of 15.3 MB.

Next: gc mark profile (crates/mortar_live/src/gc.rs), then the thread cap.

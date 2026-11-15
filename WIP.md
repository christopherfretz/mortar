# WP-032h WIP

Done:
- store wave A: descriptor table grown before a batch (no RCU grace period
  per growth) + pages of fresh read buffers touched before the read
  (`crates/mortar_input/src/store.rs` `make_room`, `prefault`). Identity
  set (54 links) identical to b6cf648.

Measured, not written: hello-cxx "members as symbol tables" upper bound
~1.15 MB resident (2.1 MB if small members were split too) of 15.3 MB.

- gc mark (`crates/mortar_live/src/gc.rs`): workers continue on what they
  mark (budget `CONTINUE`, unlimited inline; 20 rounds -> 3 on protoc);
  facts bits skip the FDE/link-order/group/merge lookups; load before
  swap; `unit_cost` on every region. A failed mark is re-run breadth
  first so the reported diagnostic is main's. Liveness phase, protoc
  --gc-sections: 1 thread 19.8 -> 15.7 ms, 8 threads 10.9 -> 9.8 ms.
  I-8 amendment to propose in the landed record.

Next: the thread cap (driver), then small costs, then the record.

# WP-151 WIP

State: code A+B done and committed; e2e tests in odr_records.rs pass;
spec odr.md edited (O-11, O-15, O-19, O-23, O-44, §8.1 item 4, §9.1,
§12.6); diag.md relation rows done.

Next:
- odr link tests: update texts that changed (kiln-k4-*, kiln-mixed,
  k4-details*); log .build/target-wp-116/wp151/lt-odr.log
- cost: .build/target-wp-116/wp151/cost (gen.py; named vs anchored,
  strict/warn/off, /usr/bin/time)
- clippy, gate, lane (background, uptime <= 12)
- record spec/landed/meta/WP-151.md; delete this file in final commit

# WIP WP-077 (deleted in the final commit)
Done: runtime (tests/hook/mason_odr_rt.cpp), run.sh/lib.sh matrix, exp/ (fail-the-load, audit);
aarch64 hod matrix all pass (T2-dlopen, R3m-lazy, T2-initial-lazy named exclusions);
byte-identical rebuilds; a68 == hod-next bytes on aarch64/x86-64.
Running: chain1 (aarch64 gcc, x86_64 hod, i686 hod), then chain2 (cost aarch64, AUD-4 x15,
i686 under -cpu pentium2,-mmx). Logs in .build/target-wp-077/scratch/.
Next: spec/landed/meta/WP-077.md, the §10.3 proposal in it, gate (cargo check, cargo test -p xtask).

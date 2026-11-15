# WP-012 handover notes (work in progress; delete when WP-012.md is written)

State: four modules written under `xtask/src/bench/` and NEVER COMPILED
(no `mod bench` in `xtask/src/lib.rs` yet, no fmt, no clippy, no test run).
Nothing else of the scope exists. No workload was built; no cache exists.

## Written (uncompiled)

- `response.rs`: capture record, wrapper script, response file write/parse,
  depfile parse, glob.
- `record.rs`: results/baseline file (`# mortar-bench 1`), `Machine`, FNV,
  UTC date. Test helpers `ok`/`not_ok`/`info`/`sample` are `pub(crate)` and
  used by `stats.rs` tests.
- `stats.rs`: `median`, `exceeds`, `calibration_differs`, `judge`,
  `Common::of`, `Check`.
- `synth.rs`: generator (`Request{shape, params}.source(i)`), 5 shapes.
  `the_generators_output_is_pinned` has PLACEHOLDER hashes (all zero): run
  it once and paste the real ones.

## Planned modules, not started

`manifest.rs` (parse `bench/workloads/*.wl`), `prepare.rs` (build, capture,
census, stamp), `measure.rs` (linkers, rounds, calibration), `report.rs`
(table + `docs/status/bench.md`), `bench.rs` (options, orchestration),
`main.rs` dispatch, the `spec/harness.md` "bench" section, `WP-012.md`,
`spec/README.md` status cell, `bench/workloads/`, `bench/baselines/`.

## Decisions (verified by hand on this host where marked V)

Capture
- Recording wrapper, not `--reproduce`: bfd has no `--reproduce`, and lld's
  and mold's rewrite the command line. The wrapper is a `/bin/sh` script
  named `ld` in a directory put in **`COMPILER_PATH`** (V: gcc and clang
  both find it; unlike `-B` it adds no `-L` to the line). It writes
  cwd + argv NUL-separated to `mktemp <dir>/link.XXXXXXXX`, then execs the
  real `ld.bfd` (cmake/cargo kinds, whose builds need the output) or exits 0
  (kinds where the harness runs the link step itself).
- Compile objects first, then link: a one-step driver link names `/tmp/cc*.o`.
- gcc's line has `-plugin .../liblto_plugin.so -plugin-opt=-fresolution=/tmp/ccXXXX.res`
  (V). All three references replay it although the `.res` file is gone (V).
  Set `TMPDIR=<workload>/tmp` while capturing. The random name would change
  the workload hash on every re-prepare: hash arguments with the workload
  directory replaced by `$W` and anything after `$W/tmp/` replaced by `*`.
  Flag for WP-008: ARCHITECTURE says the plugin interface is refused by
  name, which as written refuses every gcc-driven link.
- A captured argument starting with `@` is refused (not expanded by us).
- Select the link by glob on the basename of its `-o` (`capture = protoc*`);
  exactly one record must match.

Replay
- `cd <cwd from stamp> && <linker> @<workload>/response.txt -o <scratch>`:
  the last `-o` wins in bfd, lld and mold (V). One thread: append
  `--threads=1` for lld, mold (V) and mortar; bfd rejects it (V) and has one
  configuration, recorded as threads `1`.
- Response file: one argument per line; non-plain arguments wholly in double
  quotes with `\` before `\` and `"` — the subset bfd, lld and mold agree on
  (V; mold does not join a quoted part to an unquoted one).

Census (which files a link reads; also the admission self-check)
- Replay once with `ld.bfd ... --dependency-file=<f>` (V: bfd, lld and mold
  all write make format; bfd repeats files and keeps `..`: canonicalize and
  dedupe). Count `.o` / `.a` / `.so*` / other, sum bytes, FNV-1a the
  normalized args and file contents → `input_hash`. A workload a reference
  cannot replay is not admitted.

Workloads (`bench/workloads/*.wl`, hod's manifest syntax: `[name]`, `key = value`)
- `kind = sources`: hello-c, hello-c-static, hello-cxx, hello-cxx-static with
  gcc/g++ (`-pie`, `-static`), sources under `bench/workloads/src/`.
- `kind = synthetic`: `shape`, `small|medium|large = <a> <b>`; workload name
  gets the size suffix (`synthetic-objects-medium`); `--size` takes a list,
  default medium. Compile in batches (`clang -c a.c b.c …` in the build
  directory) on ~8 worker threads, each child through `process::run`. Sizes
  were NOT yet measured; intended medium ≈ objects 1000×200, sections
  4×25000 (two sections per function), comdat 300×500 with `-g`, strings
  200×5000, archives 100×40; large ≈ 4x (sections large per object > 0xff00
  sections on purpose).
- `kind = cmake`: cxx-medium = protobuf 3.21.12 `protoc` (last release
  without abseil; self-contained; ~200 TUs; link is main.o + libprotoc.a +
  libprotobuf.a). URL
  `https://github.com/protocolbuffers/protobuf/releases/download/v21.12/protobuf-cpp-3.21.12.tar.gz`,
  sha256 `4eab9b524aa5913c6fffb20b2a8abf5ef7f95a80bc0701f3a6dbb4c607f73460`
  (V: fetched, hashed). Fetch with `curl`, check with `sha256sum`, both
  through `process::run`. Generator "Unix Makefiles" and `make <target>`:
  make 4.4.1 is a jobserver client and inherits `MAKEFLAGS` from
  `process::run` (ninja 1.12.1 here is not a client); without a pool pass
  `-j4` (hod does the same). Two workloads: `-O0 -g` and `-O0`. Rejected:
  ninja/re2/fmt+spdlog (too few objects), mold (configure-heavy, slow),
  abseil (no program), rocksdb/LLVM (too large for the disk/time budget).
- `kind = cargo` (rust-self): `cargo rustc -p mortar --bin mortar -- -C save-temps`
  via `run_step`, with `CARGO_TARGET_DIR` and `TMPDIR` inside the workload
  directory (so rustc's `symbols.o` survives) and `COMPILER_PATH` = wrapper;
  dev profile (the many-rlib line). Untested.
- Calibration: a fixed-size synthetic `objects` workload, linked by the
  first of lld, mold, bfd at default threads, 5 runs before and after.
- Extension points to mark, not implement: LLVM-scale C++, kiln+hod objects,
  a libc++ test link, incremental cases.

Cache: `$CARGO_TARGET_DIR/../bench-work` (shared by worktrees; guard with a
pid lock file), `workloads/<name>/{build,capture,tmp,response.txt,stamp}`,
stamp key = FNV of manifest section + driver version lines + source bytes
(+ `synth::GENERATOR`).

Measurement
- One measured child at a time, one token each (`run_measured`). Do NOT hold
  extra tokens for the linker's threads: `scripts/jobserver.sh refill` counts
  one token per child and would mint tokens for the deficit. Document it
  (as hod does) and rely on calibration, spread and the recorded load.
- Per workload: one warm-up round (discarded; decides refused/failed), then
  `--runs` rounds (default 5), each round every configuration once in a
  fixed order. Reported figure: median wall, median RSS; min/max kept.
- Status: `ok` (exit 0 and a non-empty output), `refused` (mortar only: exit
  status non-zero, a diagnostic, no output left), `failed` (everything
  else). Refused/failed rows have NO figures (`record.rs` enforces it).
- Address-space limit for a measured link is untested: lld/mold died
  silently under small `ulimit -v` in WP-003; try 16 GiB.

Gate (`stats.rs`): time slower = median > baseline median × drift × 1.05
and by > 2 ms; memory = > ×1.02 and by > 1024 KiB; drift = calibration now /
baseline, at least 1, time not judged above 1.25; spread (median over min)
above 5% on either side, or a noisy run/baseline (calibration before vs
after > 10%), makes a slower time NOISY, not REGRESSED; memory is always
judged. Exit status as hod: 1 regressed/lost, 3 error or noisy, 2 usage.
`--update-baseline` should require `--reason` (AGENTS.md review gate), with
a `.log` beside `bench/baselines/<machine-id>.tsv`.

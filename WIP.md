# WIP — WP-087 (written paths)

Done: registry (`mortar_cli::table::WRITES`/`READS`, test), response files
recorded (`Parsed::response_files`), kind `written-file-clash`, the rule in
`crates/mortar/src/written.rs` called from `lib.rs` after the front;
incremental's own check replaced by `Session::written`; image: I-58 follows
symbolic links, descriptor aliases (`/dev/stdout`) written in place,
streams for maps, `remove_stale_output` spares `/dev/stdout`.

Next: link tests (crates/mortar/tests/written.rs), incremental test
assertion, specs (cli.md §2.6.4 §3.14, image.md I-58a I-79a I-66,
model.md phase 95, incr.md IN-17), strace counts, bench, record
spec/landed/cli/WP-087.md, gate.

Scratch: /tmp/mortar-wp087-3067244 (measure.py, before.tsv, after.tsv,
protoc/, strace-x/).

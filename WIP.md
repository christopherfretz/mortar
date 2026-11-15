# WIP: WP-044 (M2-DYNSYM)

Done: plan + chunk writers in `crates/mortar_synth/src/{dynsym,gnuhash,dynstr,version}.rs`
(compile, clippy clean). Signature change: `dynsym_plan` takes `src: &dyn DynSymSource`,
`pool`, `diag` and returns `Result` (names are needed for D-22's buckets).

Next: integration tests `crates/mortar_synth/tests/dynsym.rs` (fixtures, glibc lookup port,
model.py agreement via a mini ELF, lld equality), then the record `spec/landed/dyn/WP-044.md`.

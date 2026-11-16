# WP-112 work in progress (deleted in the final commit)

Count: 81 of 121 accepted rows unspelled at the start (explicit rule,
`cargo xtask spelled`; WP-100 counted 60 of 114 with drivers' implicit
words); 0 now.

Done: spelled.rs (+ `cargo xtask spelled`), `check cmp`, link `exit: info`
/ `print:`, harness reads a test's own `--mortar-map`; all tests written;
reference-difference rows added to spec/harness.md.

Findings:
- F1 (mortar): `-v`/`-V` print nothing on a line with inputs
  (crates/mortar/src/lib.rs:200 prints only when inputs empty).
  input/print-version.test xfail(mortar).
- V1 (verifier): line.rs:186 knows `-dy`/`-call_shared` with one dash only.
- V2 (verifier): P-D6 rejects lld's DF_SYMBOLIC under -Bsymbolic.
- V3 (verifier): S2 (structure.rs:674) holds mortar i386 p_align 0x1000 /
  base 0x08048000 regardless of -z max-page-size; layout/max-page-size.test
  xfail(mortar,i386).
- V4 (verifier): R6 reads SHF_COMPRESSED debug sections raw (GNU ld/lld
  `--compress-debug-sections=zlib` outputs fail).
- V5 (verifier): passes mold's push-state output: PT_INTERP + JUMP_SLOT,
  no PT_DYNAMIC (crashes). mold mislink, dyn/shlib/push-state skip(mold).
- I1 note: GNU ld's LTO link map names /tmp ltrans objects.

Next: xtask unit tests; static-identical --update (aarch64); gate;
merge main when told; hosted CI; record spec/landed/harness/WP-112.md.

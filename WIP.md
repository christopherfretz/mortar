# WIP — WP-055 (x86-64 dynamic join)

Done:
- guard lifted for x86-64 Exec/Pie (`crates/mortar/src/dynamic.rs`), `X64::new(status)` in `arch.rs`.
- FIXED a wrong output: PLT0 jumped through `.got` (generic `GotPlan::write_plt` passed `.got`'s
  start as `SynthCx.got_base`; x86-64's GOT base is `.got.plt`, X-36). `crates/mortar_synth/src/got.rs`
  `write_plt`; test `mortar_synth/tests/dyngot.rs` `x64_plt0_reads_got_plt_words`.
  The verifier does not decode PLT0 nor the entries' `jmp PLT0` (request).
- xfails: 67 x86-64 lines removed (61 WP-055, 6 WP-056 that pass); weak-class-table retagged
  (X-52 large sections staging refusal), gdb/lldb retagged (harness skips under qemu).

Next: E-corpus matrix, C++/Fortran hello, item-5 checks, static-identical, gate, record.

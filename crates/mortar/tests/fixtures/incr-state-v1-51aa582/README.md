# Link states written before WP-086

Format version 1 states (`spec/incr.md` IN-18a) written by mortar at
main 51aa582 (WP-061 stage A merged), before WP-086 made the image read
`EH_RECORDS` and `SYMTAB_LOCALS`. The test
`a_state_written_before_wp086_replays_identically`
(`crates/mortar/tests/incremental.rs`) reads each state with today's
reader, replays it (IN-61) and requires the bytes of `out`; a plain link
of the same line must give them too.

How they were made (AArch64 host, 2026-11-15), in each directory:

- `a64/`: `gcc -c a.s -o a.o`, `gcc -c b.s -o b.o` (GCC 14.2), then
  `mortar a.o b.o -static --eh-frame-hdr --incremental -o out` with the
  `mortar` binary built from 51aa582 (the WP-086 branch's first commit
  changes tests only). Two `.eh_frame` sections, so S-44 removes `b.o`'s
  CIE; `out` exits 42.
- `x86_64/`: `x86_64-linux-gnu-gcc -c a.s -o a.o`, then
  `mortar a.o -static --eh-frame-hdr --incremental -o out`.

`a.s` is the test's `ASM` / `ASM_X86_64` text as of that commit. The
objects are committed so that no assembler's version changes the input.

Re-cut by WP-083 (the RELRO padding renamed `.pad.relro`, layout L-156):
a state names its chunks, so a state that says `.relro_padding` no
longer replays (`replay: CHUNKS row 10: no such chunk in this link`, an
internal error; nothing is written). `out` and `out.mortar-state` were
made again as above, with the 51aa582 tree whose one change is that
name (`Synth::RELRO_PADDING` in `crates/mortar_core/src/layout.rs`);
the objects are the same. Each new `out` differs from the old only by
the section's name (`.shstrtab` and what follows from it: the record's
comparator, `spec/landed/dyn/WP-083.md`).

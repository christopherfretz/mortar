# WP-030b WIP (deleted in the final commit)

## Done
- Step 0 (interface), commits 0987420 + b834262: `RelocAt.addend: Option<i64>`
  (+ `RelocAt::rela_addend`), `SectionOut::input` with `with_resident`/
  `with_margins`; mechanical edits in a64/x64/scan/image and tests; arch.md
  R-1 (App. B), R-2 (§3.3), R-3 (§3.1), R-13 (§3.3, §3.4, §3.6). Gated:
  ci --quick fmt/clippy/test ok at b834262; linktest ok on rerun (first run
  failed only because crates/mortar_x86 existed without a Cargo.toml).
- Stage A source: crates/mortar_x86/src/{table,forms,relax,apply,synth,lib}.rs
  compile, clippy clean. tests/support/mod.rs (REL Case::word, set_addend, rel).

- Tests written and passing: table.rs (9), x86.rs (24), relax.rs (21),
  clip.rs (4; 21 types, 11 634 cases), w.rs (14, W1-W16 minus W12 = driver),
  elf/mod.rs (ELF32 reader). examples/apply_bench.rs: x86 scan 98 M/s,
  apply 72 M/s vs x64 104 / 88 M/s (same host, load 12.8/12 CPUs).
- x86.md §6.3 correction committed (0x<n> per diag.md §4.2).

## Half-done
- tests/census.rs and tests/differential.rs (+ sites/mod.rs, data/) are
  being written by two opus subagents (do not touch those files).

## Learned, not yet written (findings for the record / hand-back)
- image.md I-111's i386 values (R-3 first half) are image.md's: not edited.
- ScanCx has no SHF_EXECINSTR: X86-13 step 1 (GOT32 outside code) is refused
  at fill only (FieldError::Instruction); stage B: PlaceFacts.exec.
- X86-36 asks `relocation-incompatible`/InstructionMismatch with a note; the
  arch interface has no note: refused feature with x64's names (as x64).
- X86-1 names `input-flags`, which diag.md lacks: refused feature E_FLAGS.
- X86-12 test "SIZE32 −1 refused" needs u(32) before reduction (§3.1 says
  reduce first): implemented as checked before reduction (like X86-38).
- T1 (X86-37) and L1/L2 (X86-35) state no `A` condition: mortar requires
  lead A = 0 (and partner A per form), else refused by name.
- D2/L2 partner ModRM rm' = 100 not matched (SIB where the field must be).
- PC16/PC8 against an IFUNC in static-pie refused like PC32 (spec names PC32).
- §6.3 "named by number (R_386_<n>)": RefName::Hex per diag.md §4.2.
- Relaxed GOT32X still demands its GDAT slot (harmless; kind-1 could drop it).
- Brief lists TLS_LE_32 under LE; x86.md X86-14 refuses it: kept refused.

- X86-2 (RELA refused) lives in mortar_scan/src/view.rs:542 (stage B).
- TLS_TO_IE unreachable in M1 (no non-local TLS kind in static links).

## Next
- Review + commit census/differential; record spec/landed/x86/WP-030b.md;
  final gate (MORTAR_REQUIRE_TOOLS=1 cargo xtask ci --quick) in background;
  delete WIP.md; rm /tmp/mortar-wp030b-576295 by explicit path; hand-back.

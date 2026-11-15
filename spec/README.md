# Specifications

One contract per crate or cross-cutting topic. A spec is normative: the code
does what it says, and the tests named in it check that. The aim is that any
crate could be re-implemented from its spec and its tests alone.

A linker inherits most of its behavior from documents that already exist
and from what GNU ld does where they are silent. A spec here cites the
former and writes down the latter: "as GNU ld" is not a specification.

| file | covers | status |
|---|---|---|
| `model.md` | `mortar_core`: ids, the tables phases exchange, determinism rules | normative (WP-002); §3.7, §4.3 and `patches` of §4.4 confirmed with `arch.md` (WP-013) |
| `elf.md` | `mortar_elf`: readers for objects and shared objects; writer primitives | §1-§4 landed (WP-004); §6 landed (WP-006) |
| `archive.md` | `mortar_elf::archive`: regular and thin archives, GNU and BSD names, the GNU, 64-bit and BSD symbol indexes | implemented (WP-005) |
| `diag.md` | `mortar_diag`: diagnostics, the vocabulary of refusals, demangling | normative (WP-007); demangling planned |
| `cli.md` | `mortar_cli`: the command line and the status of every option | normative (WP-008) |
| `script.md` | `mortar_script`: linker scripts, version scripts, dynamic lists | §1-§7 normative (WP-009, stage 1: lexer, implicit-script commands, recognition); SECTIONS, expressions, version scripts planned |
| `input.md` | `mortar_input`: finding and loading files | accepted for M1 (WP-021 stage 1; one independent review, dispositions in `landed/input/WP-021-spec.md`); its §17 requests to other specs are applied by WP-020d |
| `resolve.md` | `mortar_resolve`: symbol resolution | accepted for M1 (WP-022 stage 1; one independent review with an executable model of the spec, dispositions in `landed/resolve/WP-022-spec.md`); its §18 requests to other specs are applied by WP-020d |
| `arch.md` | `mortar_arch`: the `Arch` trait | accepted for M1 (WP-013; two independent review passes, dispositions in `landed/arch/WP-013.md`); its requested `model.md` changes M-1..M-8 are applied by WP-020c |
| `a64.md` | AArch64: relocation table, PLT/GOT/TLS/thunk shapes, erratum 843419 | accepted for M1 (WP-013; reviewed with `arch.md`) |
| `x64.md`, `x86.md` | the relocation tables, PLT/GOT/TLS/thunk shapes per architecture | planned (WP-029, WP-030) |
| `layout.md` | `mortar_layout`: output sections, segments, addresses | accepted for M1 (WP-024 stage 1; one independent review, dispositions in `landed/layout/WP-024-spec.md`); its §20 requests to other specs are applied by the WP-024 code package |
| `synth.md` | `mortar_merge`, `mortar_synth`: split and merge, synthetic sections | draft (WP-025 stage 1) |
| `image.md` | `mortar_image`: image synthesis, the writer, debug sections | planned (WP-026) |
| `plumb.md` | `mortar_plumb`: the verifier and the map-file format | stage 1: static non-PIE AArch64 (WP-011); TLS, IFUNC, `.eh_frame` stage 2 |
| `meta.md` | `mortar_meta`: the Mason metadata container (shared with kiln, hod, tuck) | planned (M6) |
| `odr.md` | `mortar_odr`: one-definition-rule checks | planned (M6) |
| `incr.md` | `mortar_incr`: link state and incremental links | planned (M7) |
| `harness.md` | `xtask`: the gate, link tests, differential runs, bench, jobserver | "ci", "Jobserver", "Limits" (WP-003); "Link tests" (WP-010); "bench" (WP-012); "Hosted CI" (WP-014) |

## Conventions

- A spec states behavior, not history. What each work package did is
  recorded in `landed/<spec>/WP-NNN.md`, one file per WP, so that parallel
  branches never edit the same file.
- External formats are cited by their stable names: the ELF gABI and its
  section numbers, the processor supplements (AAELF64, the SysV x86-64
  psABI), the TLS ABI documents, DWARF 5 section numbers, the Itanium C++
  ABI, the LSB for `.eh_frame`, GNU ld's manual by section name.
- Every "must" has a test. The test names the spec section.
- Where the reference linkers differ from each other, the spec records the
  difference and which behavior mortar takes, and why.

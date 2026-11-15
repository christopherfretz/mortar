# Specifications

One contract per crate or cross-cutting topic. A spec is normative: the code
does what it says, and the tests named in it check that. The aim is that any
crate could be re-implemented from its spec and its tests alone.

A linker inherits most of its behavior from documents that already exist
and from what GNU ld does where they are silent. A spec here cites the
former and writes down the latter: "as GNU ld" is not a specification.

| file | covers | status |
|---|---|---|
| `model.md` | `mortar_core`: ids, the tables phases exchange, determinism rules | draft (WP-002); frozen after review against WP-004 and WP-008 |
| `elf.md` | `mortar_elf`: readers for objects and shared objects; writer primitives | §1-§4 landed (WP-004); §6 planned (WP-006) |
| `archive.md` | `mortar_elf::archive`: regular and thin archives, GNU and BSD names, the GNU, 64-bit and BSD symbol indexes | implemented (WP-005) |
| `diag.md` | `mortar_diag`: diagnostics, the vocabulary of refusals, demangling | normative (WP-007); demangling planned |
| `cli.md` | `mortar_cli`: the command line and the status of every option | planned (WP-008) |
| `script.md` | `mortar_script`: linker scripts, version scripts, dynamic lists | §1-§7 normative (WP-009, stage 1: lexer, implicit-script commands, recognition); SECTIONS, expressions, version scripts planned |
| `input.md` | `mortar_input`: finding and loading files | planned (WP-021) |
| `resolve.md` | `mortar_resolve`: symbol resolution | planned (WP-022) |
| `arch.md` | `mortar_arch`: the `Arch` trait | draft (WP-013) |
| `a64.md` | AArch64: relocation table, PLT/GOT/TLS/thunk shapes, erratum 843419 | draft (WP-013) |
| `x64.md`, `x86.md` | the relocation tables, PLT/GOT/TLS/thunk shapes per architecture | planned (WP-029, WP-030) |
| `layout.md` | `mortar_layout`: output sections, segments, addresses | planned (WP-024) |
| `synth.md` | `mortar_synth`: synthetic sections | planned (WP-025) |
| `image.md` | `mortar_image`: image synthesis, the writer, debug sections | planned (WP-026) |
| `plumb.md` | `mortar_plumb`: the verifier and the map-file format | planned (WP-011) |
| `meta.md` | `mortar_meta`: the Mason metadata container (shared with kiln, hod, tuck) | planned (M6) |
| `odr.md` | `mortar_odr`: one-definition-rule checks | planned (M6) |
| `incr.md` | `mortar_incr`: link state and incremental links | planned (M7) |
| `harness.md` | `xtask`: the gate, link tests, differential runs, bench, jobserver | "ci", "Jobserver", "Limits" (WP-003); "Link tests" (WP-010); bench planned (WP-012) |

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

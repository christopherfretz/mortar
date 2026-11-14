# Specifications

One contract per crate or cross-cutting topic. A spec is normative: the code
does what it says, and the tests named in it check that. The aim is that any
crate could be re-implemented from its spec and its tests alone.

A linker inherits most of its behavior from documents that already exist
and from what GNU ld does where they are silent. A spec here cites the
former and writes down the latter: "as GNU ld" is not a specification.

| file | covers | status |
|---|---|---|
| `model.md` | `mortar_core`: ids, the tables phases exchange, determinism rules | planned (WP-002) |
| `elf.md` | `mortar_elf`: readers for objects, archives, shared objects; writer primitives | planned (WP-004, WP-005, WP-006) |
| `diag.md` | `mortar_diag`: diagnostics, the vocabulary of refusals, demangling | planned (WP-007) |
| `cli.md` | `mortar_cli`: the command line and the status of every option | planned (WP-008) |
| `script.md` | `mortar_script`: linker scripts, version scripts, dynamic lists | planned (WP-009) |
| `input.md` | `mortar_input`: finding and loading files | planned (WP-021) |
| `resolve.md` | `mortar_resolve`: symbol resolution | planned (WP-022) |
| `arch.md` | `mortar_arch`: the `Arch` trait | planned (WP-013) |
| `a64.md`, `x64.md` | the relocation tables, PLT/GOT/TLS/thunk shapes per architecture | planned (WP-013, WP-029) |
| `layout.md` | `mortar_layout`: output sections, segments, addresses | planned (WP-024) |
| `synth.md` | `mortar_synth`: synthetic sections | planned (WP-025) |
| `image.md` | `mortar_image`: image synthesis, the writer, debug sections | planned (WP-026) |
| `plumb.md` | `mortar_plumb`: the verifier and the map-file format | planned (WP-011) |
| `meta.md` | `mortar_meta`: the Mason metadata container (shared with kiln, hod, tuck) | planned (M6) |
| `odr.md` | `mortar_odr`: one-definition-rule checks | planned (M6) |
| `incr.md` | `mortar_incr`: link state and incremental links | planned (M7) |
| `harness.md` | `xtask`: the gate, link tests, differential runs, bench, jobserver | planned (WP-003, WP-010, WP-012) |

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

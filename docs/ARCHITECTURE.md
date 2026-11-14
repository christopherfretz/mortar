# mortar architecture

mortar turns relocatable objects, archives and shared objects into an
executable or a shared object. It is the linker of the Mason toolchain
([kiln](https://github.com/christopherfretz/kiln) lowers C++ to IR,
[hod](https://github.com/christopherfretz/hod) turns the IR into objects)
and a drop-in for GNU ld on Linux for objects from GCC, Clang and gfortran.

This document is the map. Contracts live in `spec/`, the plan in
`docs/ROADMAP.md`, how correctness and speed are established in
`docs/VALIDATION.md`, and the reasoning behind the larger decisions in
`docs/design/`.

## Principles

1. **Correct, or loudly refused.** mortar either links a construct
   correctly or stops with an error that names it: an option, a relocation
   type, a section type or flag, a linker-script command. It never guesses
   and never silently ignores what it does not understand; an option that
   is accepted without effect is listed as such in `spec/cli.md` with the
   reason that doing nothing is right. Nothing double-checks a linker, so a
   file mortar writes is a file mortar vouches for.
2. **Checked twice.** `mortar_plumb` is an independent verifier: given the
   inputs and a finished output it recomputes, from the specification and
   without the linker's code, what every relocated field must hold, and
   checks the structural rules of the output. It runs on every link the
   test harness makes, on the output of the reference linkers (which tests
   the verifier), and on demand in production (`--verify`).
3. **Speed and memory are tested properties.** Wall-clock time and peak
   resident memory of a link are measured against recorded baselines and
   against mold, lld and GNU ld on the same inputs; a regression fails like
   a test.
4. **Safe Rust end to end.** Every crate is `#![forbid(unsafe_code)]`.
   `unsafe` is admitted only for a measured need (file mapping is the
   expected first candidate), inside a small separately reviewed crate,
   with the measurement and the decision recorded in `docs/design/`.
5. **No third-party dependencies in the shipped crates** unless approved in
   a work package and recorded here. (None so far.)
6. **Deterministic.** The output depends only on the inputs, their order
   and the options: not on thread count, scheduling, hash iteration order,
   file modification times or the clock. Every parallel phase has a
   specified sequential meaning and the tests compare `--threads=1` with
   many.
7. **Layout is data; the image is a function of it.** Deciding where
   everything goes (`Layout`) is separate from producing bytes. The image
   is a pure function of the inputs, the resolution and the layout. This is
   what makes incremental linking checkable (an incremental link only
   computes a different layout cheaply and writes the bytes that changed)
   and what the verifier leans on.
8. **Specs over folklore.** Each crate has a contract in `spec/` with
   behavioral tests tied to it. Re-implementing a crate from its spec and
   its tests must be possible. Linker behavior that exists only as "what
   GNU ld does" is written down before it is implemented.
9. **Additive metadata.** What the Mason toolchain adds to objects (the
   one-definition-rule records, incremental hints, debugger context) is
   never required to link: objects of other toolchains link as they do with
   any linker, with fewer guarantees, and Mason objects link with other
   linkers, which ignore the additions.
10. **Debuggable output is part of "correct".** Debug information that gdb
    and lldb consume, unwind tables the C++ runtime and profilers use,
    symbol tables binutils reads.

## Scope

In scope, in the order the roadmap takes them: little-endian ELF for
`aarch64-unknown-linux-gnu` and `x86_64-unknown-linux-gnu` (ELF64) and
`i686-unknown-linux-gnu` (ELF32, `SHT_REL` relocations); executables (PIE,
non-PIE, static, static-PIE), shared objects, relocatable output (`-r`);
glibc first, musl checked; GNU ld's command line, linker scripts, version
scripts and dynamic lists.

kiln and hod target the two 64-bit architectures; 32-bit x86 objects come
from GCC and Clang. Because a 32-bit target is in scope, nothing in the
shared crates assumes the ELF class: readers present both classes through
one widened view, the tables hold addresses and sizes as 64-bit values
with the class a property of the link, and the writers of headers, symbol
tables, dynamic tables and relocation records are generic over the class
and over REL or RELA.

Out of scope until a milestone says otherwise: Mach-O, COFF, big-endian
ELF, the x32 ABI, other architectures (32-bit ARM included), link-time
optimization (the plugin interface is refused by name; `-flto` objects are
a later decision).

## Pipeline

```
argv ─────── mortar_cli ────► Plan            options, inputs in order, scripts
                                │
             mortar_input ──────┤             find, read and parse files in
                                │             parallel: objects, archives,
                                ▼             shared objects, scripts
                              Inputs          immutable, index-addressed
                                │
             mortar_resolve ────┤             symbols, archive members to a
                                │             fixed point, COMDAT, versions
                                ▼
                            Resolution        which definition wins, what is live
                                │
             mortar_odr ────────┤             one-definition-rule checks (Mason
                                │             metadata; weaker checks without)
             mortar_live ───────┤             --gc-sections, identical code folding
             mortar_merge ──────┤             mergeable strings and constants
             mortar_arch::scan ─┤             per relocation: GOT, PLT, TLS, copy,
                                │             dynamic relocation demands
             mortar_synth ──────┤             synthetic sections, sized
             mortar_layout ─────┤             output sections, segments, addresses;
                                ▼             range thunks to a fixed point
                              Layout          where every byte goes
                                │
             mortar_image ──────┤             copy, relocate, fill synthetic
                                ▼             sections: parallel, pure
                              output file  (+ map file, link state for mortar_incr)
```

Three properties of this shape matter more than the boxes:

- **Phases exchange plain data.** `Plan`, `Inputs`, `Resolution` and
  `Layout` are tables addressed by small integer ids (`FileId`,
  `SectionId`, `SymbolId`, `ChunkId`), not object graphs. A phase reads the
  tables before it and writes its own. That is what lets phases be built,
  tested and re-implemented separately, what makes parallelism safe without
  `unsafe`, and what makes the tables storable for incremental links.
- **Parallel inside a phase, sequential between phases.** Work inside a
  phase is split by file, by section or by output range over scoped
  threads; each split has a deterministic merge.
- **The same code serves full and incremental links.** An incremental link
  loads the previous tables, recomputes the rows the changed inputs touch,
  and asks `mortar_image` for the dirty ranges only (`mortar_incr`).

## Crates

Crates are created by the work package that first fills them; the list is
the plan, and `spec/model.md` fixes the tables they exchange before
parallel work on them starts.

| crate | role | depends on |
|---|---|---|
| `mortar_elf` | ELF, both classes: constants, bounds-checked zero-copy readers for relocatable objects, shared objects and archives (GNU, thin, BSD), writer primitives. No policy. | - |
| `mortar_diag` | The diagnostic model, rendering, symbol demangling (Itanium C++, Rust v0), the "refused by name" vocabulary. | - |
| `mortar_core` | The shared vocabulary: ids, the `Plan`/`Inputs`/`Resolution`/`Layout` tables, interning, the parallel helpers with their deterministic merges. No algorithms. | `mortar_elf`, `mortar_diag` |
| `mortar_cli` | GNU ld's command line to a `Plan`: the option table with a status per option, response files, emulation names. | `mortar_core` |
| `mortar_script` | Linker scripts (including the implicit ones: `libc.so` is a text file), version scripts, dynamic lists: text to a checked AST. | `mortar_diag` |
| `mortar_input` | Search paths and sysroot, file reading, format sniffing, parallel parsing into `Inputs`. Owns how bytes get into memory. | `mortar_core`, `mortar_script` |
| `mortar_resolve` | Symbol resolution: binding and visibility rules, archive extraction, COMDAT, common symbols, versions, `--wrap`, `--defsym`. | `mortar_core` |
| `mortar_live` | Section liveness (`--gc-sections`), identical code folding. | `mortar_core` |
| `mortar_merge` | `SHF_MERGE` sections: string and constant deduplication. | `mortar_core` |
| `mortar_arch` | The `Arch` trait: classify, scan, apply and relax relocations; PLT, GOT and thunk shapes; TLS sequences; unwind encodings. | `mortar_core` |
| `mortar_a64`, `mortar_x64`, `mortar_x86` | The architectures: AArch64, x86-64, 32-bit x86. | `mortar_arch` |
| `mortar_synth` | Synthetic sections: `.dynamic`, `.dynsym`, hash tables, version sections, GOT and PLT, dynamic relocations (RELA, RELR), `.eh_frame` and `.eh_frame_hdr`, notes, `.symtab`, debug indexes. | `mortar_core`, `mortar_arch` |
| `mortar_layout` | Output sections from the default rules or a script, their order, segments, addresses, thunk insertion: `Layout`. | `mortar_core`, `mortar_arch`, `mortar_script` |
| `mortar_image` | Image synthesis and the file writer; debug section handling (tombstones, compression). | `mortar_core`, `mortar_arch`, `mortar_synth` |
| `mortar_meta` | The Mason metadata container: reader and writer of the format shared with kiln, hod and tuck (`spec/meta.md`). | `mortar_elf` |
| `mortar_odr` | One-definition-rule checks at static link time; the tables and the load-time hook for dynamic checks. | `mortar_core`, `mortar_meta` |
| `mortar_incr` | The link state, change detection, delta layout, patching. | everything above |
| `mortar_plumb` | The independent verifier. Depends on `mortar_elf` and `mortar_diag` only, by rule: it shares no algorithm with the linker. | `mortar_elf`, `mortar_diag` |
| `mortar` | The facade (`mortar::link`) and the `mortar` executable (also invoked as `ld.mortar` or `ld`). | all |
| `mortar_testutil`, `xtask` | Test support and the harness: link tests, differential runs against the reference linkers, external suites, benchmarks, the jobserver client. | - |

## How mortar is reached

Compiler drivers run the linker; mortar is one when its executable is
named `ld` or `ld.mortar` in a directory given with `-B`, or with clang's
`--ld-path=`. kiln links through `clang++` today and passes `-B`,
`-fuse-ld=` and `-Wl,` through, so mortar needs nothing from kiln to be
used. A `-fuse-ld=mortar` spelling in kiln's driver, and kiln running
mortar without a driver in between, are requests for the kiln project
(`docs/ROADMAP.md`, "Cross-project").

## The larger designs

Each of these gets a design document in `docs/design/` before its work
packages are cut. The positions taken here are the constraints the first
milestones must not violate.

**Input and output bytes.** `mortar_input` is the only crate that knows how
a file's bytes reach memory, and `mortar_image` the only one that knows how
they reach the output. Version one reads files into owned buffers and
writes ranges with positioned writes, both safe and parallel. Mapping files
is the expected first request for `unsafe`; it is decided by measurement
against the benchmark set, not in advance.

**The verifier.** `mortar_plumb` takes the inputs, the output and the map
of where each input section went (mortar's link state, or the map file of
another linker), and recomputes each relocation from the processor
supplement's formula and the output's own symbol table. It also checks
the output as a loader and a debugger will read it: segments against
sections, the dynamic table against what it points to, hash tables against
the symbols, `.eh_frame_hdr` against `.eh_frame`. It is written from the
specs by someone who has not read the linker's relocation code.

**Incremental linking.** The state of a link (the tables above, plus an
index from each symbol to the fields that refer to it) is kept next to the
output. A later link compares inputs by content hash (kiln and hod can
supply hashes per section), re-resolves what the changed inputs define and
reference, and places changed sections into the slack the previous layout
left, or moves them and patches every referring field through the index.
Whenever a change is outside what the delta rules cover, mortar links from
scratch and says why. Two invariants are tested on every incremental link
in the harness: the verifier accepts the output, and a full link *given the
same layout* produces the same bytes (principle 7).

**One-definition rule.** kiln records, per entity with linkage that may be
defined in several translation units, a hash of the definition and where
it came from; hod carries the records into the object in a section other
linkers discard (`spec/meta.md`). mortar compares them when it resolves
symbols and COMDAT groups: definitions that differ are an error naming
both locations, instead of one being chosen silently. For shared objects
the records of exported entities are kept in a loadable table, and a hook
run when the object is loaded compares it with the tables of the objects
already present. Without metadata mortar still reports what the objects
themselves reveal (sizes, alignments and types of same-named symbols;
COMDAT groups whose members differ in shape).

**Debug information.** Version one treats `.debug_*` as data: merge
strings, relocate, tombstone references to discarded sections, compress on
request. Indexes (`.gdb_index`, `.debug_names`) and anything tuck needs
come through the same synthetic-section mechanism, later.

**Performance.** The designs that make the fast linkers fast are assumed
from the start: everything indexed, nothing parsed twice, strings interned
once in a concurrent table, relocations applied while copying, output
written in parallel. Phase times and peak memory are reported by
`--time-phases` from the first working link, so that the data exists
before the optimization milestone.

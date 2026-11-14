# mortar

**A linker for the edit-compile-debug loop of modern C++, in safe Rust.**

mortar is the linker of the Mason toolchain:
[kiln](https://github.com/christopherfretz/kiln) is the C++26 front end,
[hod](https://github.com/christopherfretz/hod) the debug-build code
generator, mortar the linker. It also links objects from GCC, Clang and
gfortran, and takes GNU ld's command line.

Goals, in order:

- **Correct.** Nothing double-checks a linker, so mortar checks itself: an
  independent verifier recomputes every relocation of a finished link. A
  construct mortar does not implement is refused by name, never guessed at.
- **Fast and lean.** Wall-clock time and peak memory are measured by the
  validation harness against recorded baselines and against mold, lld and
  GNU ld, and gated like tests.
- **Incremental.** Replace a symbol, an object or an archive and mortar
  brings the executable up to date by patching it, with the guarantee that
  the result is the one a full link with the same layout produces.
- **Safer than the language requires.** With metadata from kiln, mortar
  diagnoses violations of the one-definition rule at static link time and
  when shared objects are loaded.
- **Safe Rust end to end**, with output that gdb, lldb and the usual binary
  tools consume.

Status: bootstrapping. Nothing here is usable yet. The map is
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), the plan
[docs/ROADMAP.md](docs/ROADMAP.md), how correctness and speed are
established [docs/VALIDATION.md](docs/VALIDATION.md).

## Building

```
source scripts/env.sh
cargo xtask ci --quick
```

## License

Dual-licensed under MIT ([LICENSE-MIT](LICENSE-MIT)) or Apache-2.0
([LICENSE-APACHE](LICENSE-APACHE)), at your option.

# The Mason lane

Objects made by kiln (the C++ front end) and hod (its code generator),
linked by mortar and run, beside the same objects linked by GNU ld, lld
and mold. `cargo xtask mason` (`xtask/src/mason.rs`) is the harness;
`spec/landed/mason/WP-033.md` is the record of the first runs.

kiln and hod are not built by this repository. The harness finds their
release builds in the build root (`<root>/target-kiln/release/kiln++`,
`<root>/target-hod/release/hod`, the root being the parent of this
checkout's target directory) or through `MASON_KILN` / `MASON_HOD`.
Without them every command says `skipped` and exits 0;
`MASON_REQUIRE_TOOLS=1` makes a missing build an error. kiln is always
run with `KILN_BACKEND=hod KILN_HOD_STRICT=1`, and every object job must
be hod's (kiln's statistics log): a decline that fell back to `llc` fails
the cell. The lane runs natively on AArch64.

## Programs

`cargo xtask mason [programs] [--filter <s>]... [-j <n>] [--keep] [--census] [--mortar <path>]`

Each directory of `programs/` is one program. Its sources are named by
producer: `*.kiln.cpp` (kiln + hod), `*.gcc.cpp` (g++), `*.clang.cpp`
(clang++ 22), `*.gfortran.f90`. An optional `link` file names the driver
whose `-static -###` gives the link line (`kiln`, the default, asks
kiln++ for its clang++; `g++`) and extra link arguments after the
objects (`g++ -lgfortran`).

| program | what it holds |
|---|---|
| hello | iostream |
| eh | an exception thrown in one TU and caught in another (exit 11) |
| tls | `thread_local` across TUs and threads |
| rtti | virtual dispatch, diamond virtual inheritance, `typeid`, `dynamic_cast` (exit 8) |
| templates | the same instantiations in three TUs (COMDAT) |
| ctors | `init_priority`, `constructor(N)`, static ctor/dtor order, `atexit` |
| inline | inline variables and static members across TUs |
| mixed-kiln-main | kiln `main` over GCC and Clang libraries; exceptions both ways |
| mixed-gcc-main | GCC `main` over a kiln library; exceptions both ways |
| mixed-clang | Clang `main` -> GCC -> kiln; an exception through all three |
| fortran | kiln C++ calling gfortran (`bind(C)`) and back |

Every program runs in each variant: libstdc++ with `-O0`, `-g`,
`-ffunction-sections -fdata-sections`, `-fno-pic`; and, for programs of
kiln and Clang sources only, when `setup` has made the libc++ tree,
libc++ 22 static as shipped (`libcxx`) and with `.ARM.attributes` and
`.deplibs` stripped (`libcxx-stripped`, with `-g`). Each cell links the
same objects with GNU ld (the reference), lld, mold (when present) and
mortar, runs every output, and compares standard output, standard error
and the exit status with GNU ld's. A cell holds when mortar's program runs
as GNU ld's and the verifier finds nothing on mortar's output, or, in `libcxx`, when
mortar refuses with `unsupported section type: SHT_AARCH64_ATTRIBUTES`;
mortar linking the shipped archives fails the cell (XPASS) until the
expectation in `programs.rs` (`LIBCXX_REFUSAL`) is updated. `--census`
also compiles the kiln sources with g++ and clang++ and prints the census
of the three producers' objects.

The run took 41 s at `-j 4` (mortar already built, a loaded shared
machine). It is not in `ci --quick`: it needs kiln and hod builds, and
its time is close to the one-minute budget.

## Census

`cargo xtask mason census [--group <name> <file>...]...` counts, per group
of objects or archives: section classes (a name with its symbol part as
`*`, type, flags), relocation types per target section class, symbol
kinds, COMDAT groups, `.eh_frame` CIE shapes, notes and GNU properties;
then the keys some group has and another does not.

## Setup

`cargo xtask mason setup` makes, under `<root>/mason-lane`
(`MASON_LANE_DIR`), each step skipped when done: Debian's
`libc++-22-dev`, `libc++abi-22-dev`, `libunwind-22-dev`
(`apt-get download`, nothing installed); their headers and static
archives without the packages' symlinks (`libcxx-22-static`); the
archives with `.ARM.attributes` and `.deplibs` removed
(`libcxx-22-stripped`); libc++'s tests at kiln's tag, by kiln's
`scripts/fetch-libcxx-tests.sh`; a copy of kiln's tree at the submodule's
commit (`git archive`) whose `.toolchain/libcxx-22-root` points at the
static tree, and kiln's `xtask` built from it. Network: the Debian mirror
and GitHub. About 230 MB.

## libc++

`cargo xtask mason libcxx [--mode <m>]... [--backend hod|llvm] [--sample vector|wp033] [--shard K/N] [-j <n>] [--plumb] [--report <file>] [<dir>...]`

kiln's own runner (`kiln/spec/libcxx.md`) runs the tests, once per mode,
with `lane/kiln++` as its compiler. Compiles go to kiln++ unchanged. The
link, except in mode `dyn`, is made fully static (`-static`; the runner's
`-Wl,-Bstatic`/`-Bdynamic`/`--as-needed` dropped) and uses the `ld` of
`lane/ld/<linker>/` (`-B`): `bfd`, `lld`, `mold`, `mortar`;
`mortar-stripped` is mortar on `libcxx-22-stripped`. `dyn` is kiln's
procedure unchanged (static libc++, dynamic glibc, the host's `ld`).
mortar's `ld` writes a map, logs each link, and with `--plumb` runs
`xtask mason verify` on the output. The report: kiln's summary per mode;
for each mortar mode the tests the reference (`bfd`, else `dyn`) passes
and it does not, of which WRONG (mortar linked and the test then failed);
refusals by diagnostic; the verifier's results; the back end of every
object job. The command fails when a WRONG test exists.

Samples: `vector` (`std/containers/sequences/vector`, 83 tests);
`wp033` (`std/algorithms`, `strings`, `input.output`,
`language.support`, `thread`, `localization`, shard 1/4).

## Reproducers

`repro/` holds the reduced cases of what the lane found; each says how
to run it and what mortar should do.

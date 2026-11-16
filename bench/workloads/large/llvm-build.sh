#!/bin/sh
# Build LLVM/clang 19.1.7 for the bench's large workloads (spec/harness.md
# "bench", "Large workloads"; WP-081). Step 1 of 2: the compile. Step 2,
# `llvm-capture.py`, records the final links and copies their inputs into
# the bench cache; after it the build tree is deleted.
#
# usage: llvm-build.sh <work> <config> <jobs>
#   <work>    the bench cache (.build/bench-work)
#   <config>  g      RelWithDebInfo (-O2 -g)
#             split  the same with -gsplit-dwarf (LLVM_USE_SPLIT_DWARF)
#   <jobs>    ninja's -j: at most 6 on the shared host (ninja 1.12 is not a
#             jobserver client); run it under `nice -n 10`
#
# The build tree is <work>/workloads/cxx-large-<config>/build. Archives are
# THIN while building (half the disk); the capture writes regular ones.
set -eu
WORK=$1
CONFIG=$2
JOBS=$3
SHA=82401fea7b79d0078043f7598b835284d6650a75b93e64b6f761ea7b63097501
URL=https://github.com/llvm/llvm-project/releases/download/llvmorg-19.1.7/llvm-project-19.1.7.src.tar.xz
TARBALL=$WORK/downloads/llvm-project-19.1.7.src.tar.xz
SRC=$WORK/src/82401fea7b79d007
case $CONFIG in
  g) EXTRA="" ;;
  split) EXTRA="-DLLVM_USE_SPLIT_DWARF=ON" ;;
  *) echo "llvm-build.sh: config is g or split" >&2; exit 2 ;;
esac
mkdir -p "$WORK/downloads" "$SRC"
if ! echo "$SHA  $TARBALL" | sha256sum -c --status 2>/dev/null; then
  curl -fsSL --retry 3 -o "$TARBALL.part" "$URL"
  echo "$SHA  $TARBALL.part" | sha256sum -c --status
  mv "$TARBALL.part" "$TARBALL"
fi
if [ ! -e "$SRC/.unpacked" ]; then
  # Five test inputs (symlinks under clang/test/Driver/Inputs) cannot be
  # created on the workspace's file system; they matter to no test run here.
  (cd "$SRC" && tar -xJf "$TARBALL" --no-same-owner \
      llvm-project-19.1.7.src/llvm llvm-project-19.1.7.src/clang \
      llvm-project-19.1.7.src/cmake llvm-project-19.1.7.src/third-party || true)
  test -f "$SRC/llvm-project-19.1.7.src/llvm/CMakeLists.txt"
  touch "$SRC/.unpacked"
fi
B=$WORK/workloads/cxx-large-$CONFIG/build
mkdir -p "$B"
export LC_ALL=C
# Static tools (the operator's configuration), the container's clang 19,
# the host target only, no optional system libraries (none is static here).
# The default linker (GNU ld): LLVM's configuration chooses the link flags
# by the linker it finds, and the captured lines must be GNU ld's.
cmake -S "$SRC/llvm-project-19.1.7.src/llvm" -B "$B" -G Ninja \
  -DCMAKE_BUILD_TYPE=RelWithDebInfo \
  -DCMAKE_C_COMPILER=clang -DCMAKE_CXX_COMPILER=clang++ \
  -DLLVM_ENABLE_PROJECTS=clang \
  -DLLVM_TARGETS_TO_BUILD=AArch64 \
  -DLLVM_ENABLE_ASSERTIONS=OFF \
  -DCMAKE_EXE_LINKER_FLAGS=-static \
  -DLLVM_USE_LINKER= -DLLVM_PARALLEL_LINK_JOBS=1 \
  "-DCMAKE_C_ARCHIVE_CREATE=<CMAKE_AR> DqcT <TARGET> <LINK_FLAGS> <OBJECTS>" \
  "-DCMAKE_CXX_ARCHIVE_CREATE=<CMAKE_AR> DqcT <TARGET> <LINK_FLAGS> <OBJECTS>" \
  -DLLVM_ENABLE_ZLIB=OFF -DLLVM_ENABLE_ZSTD=OFF -DLLVM_ENABLE_LIBXML2=OFF \
  -DLLVM_ENABLE_TERMINFO=OFF -DLLVM_ENABLE_LIBEDIT=OFF -DLLVM_ENABLE_LIBPFM=OFF \
  -DLLVM_ENABLE_BINDINGS=OFF -DLLVM_INCLUDE_BENCHMARKS=OFF \
  -DLLVM_INCLUDE_EXAMPLES=OFF -DCLANG_ENABLE_ARCMT=ON \
  $EXTRA
# The small tools the tests need, linked by the build; of the three large
# links only the inputs (the capture records the links themselves).
ninja -C "$B" -j "$JOBS" FileCheck not count split-file llvm-config clang-resource-headers
ninja -C "$B" -t inputs bin/clang-19 bin/opt bin/llvm-lto2 | grep -E '[.](o|a)$' > "$B/large-inputs.txt"
xargs ninja -C "$B" -j "$JOBS" < "$B/large-inputs.txt"
echo "llvm-build.sh: done"

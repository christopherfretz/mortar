#!/bin/bash
# tools.sh WORK MORTAR: part 3 of the census area (AArch64 native).
# (1) libtool-shaped -shared lines, written by hand from what libtool 2.4
#     documents/emits (libtool is NOT installed: every line here is
#     UNVERIFIED-as-libtool), linked by GNU ld, lld 22 and mold, run.
# (2) a tiny CMake project with BUILD_SHARED_LIBS=ON: the link lines it
#     produces (verbose make output + a recording ld wrapper), linked by
#     GNU ld (default), then CMAKE_LINKER_TYPE=LLD and MOLD, run.
set -u
ulimit -c 0
WORK=$(realpath -m "$1"); MORTAR=$2
HERE=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$WORK"; cd "$WORK" || exit 1
# gcc finds ld.lld in its -B directories first: one directory per lld version
mkdir -p "$WORK/lld22bin" "$WORK/lld19bin" "$WORK/tmp"; ln -s /usr/bin/ld.lld-22 "$WORK/lld22bin/ld.lld"; ln -s /usr/bin/ld.lld-19 "$WORK/lld19bin/ld.lld"
export TMPDIR=$WORK/tmp
say() { echo "=== $*"; }
WR=$WORK/wrap/aarch64; mkdir -p "$WR"; cp "$HERE/wrap_ld.sh" "$WR/ld"; chmod +x "$WR/ld"
lflag() { case $1 in bfd) echo "-B$WR/";; lld) echo "-B$WORK/lld22bin/ -fuse-ld=lld";; lld19) echo "-B$WORK/lld19bin/ -fuse-ld=lld";; mold) echo "-fuse-ld=mold";; mortar) echo "-B$WORK/mortarbin/";; esac; }
mkdir -p mortarbin; ln -s "$MORTAR" mortarbin/ld
LT=$HERE/src/lt
CRTI=$(gcc -print-file-name=crti.o); CRTN=$(gcc -print-file-name=crtn.o)
CRTBS=$(gcc -print-file-name=crtbeginS.o); CRTES=$(gcc -print-file-name=crtendS.o)
GDIR=$(dirname "$CRTBS")
# libtool's postdeps/predep_objects for g++, as `g++ -v -shared` reports them on this host
say "g++ -shared -v link line (what libtool's configure parses into predep_objects/postdeps)"
echo 'int x;' > conftest.cpp; g++ -fPIC -c conftest.cpp -o conftest.o
g++ -shared -v conftest.o -o conftest.so 2>&1 | grep collect2 | sed 's/ -plugin[^ ]*//g' | tr ' ' '\n' | grep -E '\.o$|^-l|^-L' | tr '\n' ' '; echo

say "compile (libtool: -fPIC -DPIC into .libs/)"
for s in foo.c conv.c dep.c; do gcc -O2 -fPIC -DPIC -c "$LT/$s" -o "$WORK/${s%.c}.o"; done
g++ -O2 -fPIC -DPIC -c "$LT/bar.cpp" -o "$WORK/bar.o"
ar cr libconv.a conv.o; ranlib libconv.a

for k in bfd lld lld19 mold mortar; do
  D=$WORK/lt-$k/.libs; mkdir -p "$D"
  say "libtool-shaped lines, linker $k (UNVERIFIED-as-libtool)"
  export CENSUS_ARGS_DIR=; [ $k = bfd ] && { mkdir -p "$WORK/lt-lines"; export CENSUS_ARGS_DIR=$WORK/lt-lines; }
  # L1: C library, -version-info 1:0:0
  gcc -shared -fPIC -DPIC $(lflag $k) "$WORK/foo.o" -O2 -Wl,-soname -Wl,libfoo.so.1 \
      -Wl,-version-script -Wl,"$LT/foo.ver" -o "$D/libfoo.so.1.0.0"; echo "L1 libfoo (C, soname, -version-script from -export-symbols) rc=$?"
  ln -sf libfoo.so.1.0.0 "$D/libfoo.so.1"; ln -sf libfoo.so.1.0.0 "$D/libfoo.so"
  # L2: C++ library, libtool's -nostdlib form with the predeps/postdeps spelled out
  g++ -fPIC -DPIC -shared -nostdlib $(lflag $k) "$CRTI" "$CRTBS" "$WORK/bar.o" -L"$GDIR" \
      -L/usr/lib/aarch64-linux-gnu -lstdc++ -lm -lc -lgcc_s "$CRTES" "$CRTN" -O2 \
      -Wl,-soname -Wl,libbar.so.1 -o "$D/libbar.so.1.0.0"; echo "L2 libbar (C++, -nostdlib crti crtbeginS ... -lstdc++ -lm -lc -lgcc_s crtendS crtn) rc=$?"
  ln -sf libbar.so.1.0.0 "$D/libbar.so.1"; ln -sf libbar.so.1.0.0 "$D/libbar.so"
  # L3: a library made only of a convenience archive
  gcc -shared -fPIC -DPIC $(lflag $k) -Wl,--whole-archive "$WORK/libconv.a" -Wl,--no-whole-archive -O2 \
      -Wl,-soname -Wl,libwa.so.1 -o "$D/libwa.so.1.0.0"; echo "L3 libwa (--whole-archive convenience archive) rc=$?"
  ln -sf libwa.so.1.0.0 "$D/libwa.so.1"; ln -sf libwa.so.1.0.0 "$D/libwa.so"
  # L4: a library depending on another libtool library, with the install rpath
  gcc -shared -fPIC -DPIC $(lflag $k) "$WORK/dep.o" -Wl,-rpath -Wl,/usr/local/lib -L"$D" -lfoo -O2 \
      -Wl,-soname -Wl,libdep.so.1 -o "$D/libdep.so.1.0.0"; echo "L4 libdep (-rpath -L.libs -lfoo) rc=$?"
  ln -sf libdep.so.1.0.0 "$D/libdep.so.1"; ln -sf libdep.so.1.0.0 "$D/libdep.so"
  unset CENSUS_ARGS_DIR
  [ $k = mortar ] && continue
  for l in foo bar wa dep; do
    f=$D/lib$l.so.1.0.0; [ -e "$f" ] || continue
    echo "$k lib$l: $(readelf -dW "$f" | awk '/NEEDED|SONAME|RUNPATH|RPATH|FLAGS/{print $2 $NF}' | tr '\n' ' ') verdefs: $(readelf -VW "$f" | sed -n '/Version definition/,/Version needs/p' | grep -c 'Name:') init_array: $(readelf -dW "$f" | grep -c INIT_ARRAY) exported: $(readelf --dyn-syms -W "$f" | awk '$7!="UND" && $5=="GLOBAL"{print $8}' | grep -v '^_Z.*St' | sort | tr '\n' ' ' | cut -c1-200)"
  done
  g++ -o "$WORK/lt-$k/host" "$LT/host.cpp" -L"$D" -lfoo -lbar -lwa -ldep; echo "host (GNU ld) rc=$?"
  (LD_LIBRARY_PATH=$D "$WORK/lt-$k/host") 2>&1 | sed "s/^/$k host: /"
done
say "libtool-shaped ld lines as GNU ld received them (recording wrapper)"
for f in "$WORK"/lt-lines/line*.args; do echo "--- $(basename "$f")"; tail -n +2 "$f" | grep -v -E '^-plugin|^-plugin-opt' | tr '\n' ' '; echo; done

# ---------------------------------------------------------------- CMake
CM=$HERE/src/cm
for k in default LLD MOLD; do
  B=$WORK/cmake-$k; mkdir -p "$B/lines"
  say "CMake BUILD_SHARED_LIBS=ON, CMAKE_LINKER_TYPE=$k"
  extra=(); [ $k != default ] && extra=(-DCMAKE_LINKER_TYPE=$k)
  wflags=(); [ $k = default ] && wflags=(-DCMAKE_SHARED_LINKER_FLAGS=-B$WR/ -DCMAKE_EXE_LINKER_FLAGS=-B$WR/)
  (cd "$B" && cmake -S "$CM" -B . -G "Unix Makefiles" -DBUILD_SHARED_LIBS=ON -DCMAKE_VERBOSE_MAKEFILE=ON "${extra[@]}" "${wflags[@]}" > cmake-configure.log 2>&1); echo "configure rc=$?"
  (cd "$B" && CENSUS_ARGS_DIR=$B/lines make -j1 > build.log 2>&1); echo "build rc=$?"
  echo "-- driver link lines (verbose make):"
  grep -E '^/usr/bin/(c\+\+|cc|gcc|g\+\+)\b.* -o ' "$B/build.log" | grep -v ' -c ' | sed "s|$B|BUILD|g; s|$WR|WRAP|g"
  echo "-- link.txt files:"; for t in $(cd "$B" && find . -name link.txt | sort); do echo "$t: $(sed "s|$B|BUILD|g; s|$WR|WRAP|g" "$B/$t")"; done
  if [ $k = default ]; then
    echo "-- ld lines (recording wrapper):"
    for f in "$B"/lines/line*.args; do echo "--- $(basename "$f")"; tail -n +2 "$f" | grep -v -E '^-plugin|^-plugin-opt' | sed "s|$B|BUILD|g" | tr '\n' ' '; echo; done
  fi
  for f in libcl.so.1.2.3 libcxl.so app; do [ -e "$B/$f" ] && echo "$k $f: $(readelf -dW "$B/$f" | awk '/NEEDED|SONAME|RUNPATH|RPATH/{print $2 $NF}' | tr '\n' ' ')"; done
  (cd /; "$B/app") 2>&1 | sed "s/^/$k app: /"
done
say "mortar on the CMake project (CMAKE_SHARED_LINKER_FLAGS=-B<dir with ld -> mortar>)"
B=$WORK/cmake-mortar; mkdir -p "$B"
(cd "$B" && cmake -S "$CM" -B . -G "Unix Makefiles" -DBUILD_SHARED_LIBS=ON -DCMAKE_VERBOSE_MAKEFILE=ON -DCMAKE_SHARED_LINKER_FLAGS=-B$WORK/mortarbin/ > cmake-configure.log 2>&1; make -j1 > build.log 2>&1); echo "build rc=$?"
grep -E 'error' "$B/build.log" | head -8
say "trim"
for k in default LLD MOLD mortar; do find "$WORK/cmake-$k/CMakeFiles" -name '*.o' -delete 2>/dev/null; done
du -sh "$WORK"

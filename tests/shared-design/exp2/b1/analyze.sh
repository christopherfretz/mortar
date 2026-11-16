#!/bin/bash
# analyze.sh WORK: run relscan.py over the B-1 corpus (corpus.sh WORK first),
# the author's census objects, and the system archives; one summary per group
# into WORK/b1-analysis/<group>.txt, the hits into <group>.hits.
set -u
ulimit -c 0
WORK=$(realpath -m "$1"); HERE=$(cd "$(dirname "$0")" && pwd)
C=$WORK/b1corpus/obj; A=$WORK/b1-analysis; mkdir -p "$A"; cd "$A" || exit 1
CEN=/Volumes/AgentWork/mortar-helper/.build/target-wp-110/work/census
run() { local g=$1; shift; python3 -I "$HERE/relscan.py" --hits --label "$g" "$@" > "$g.all"; grep -v '^HIT' "$g.all" > "$g.txt"; grep '^HIT' "$g.all" > "$g.hits"; find . -maxdepth 1 -name "$g.all" -delete; }
for comp in gcc-a64 gcc-x64 clang-a64 clang-x64 gcc-i386; do
  for mode in pic picnsi picvih pie; do
    ls $C/$comp.$mode.*.o >/dev/null 2>&1 || continue
    run "$comp.$mode" $C/$comp.$mode.*.o
  done
done
# the author's census: every -fPIC object (the 21 census library objects, the
# Lua modules, zlib's -g -O2 -fPIC objects, the libtool/CMake objects)
run census-pic $CEN/p1/obj/obj.*.pic.o $CEN/p2/lua/lib*.o $CEN/p2/zlib/objs/*.o $CEN/p3/*.o $CEN/p3/libconv.a
run census-fpie $CEN/p1/obj/obj.*.nopic.o $CEN/p2/lua/obj/*.o
# what -static-libgcc / -static-libstdc++ / -static-libgfortran put into a library
run sysrt-a64 /usr/lib/gcc/aarch64-linux-gnu/14/{libgcc.a,libgcc_eh.a,libstdc++.a,libsupc++.a,libgfortran.a,libatomic.a,libgomp.a}
run sysrt-x64 /usr/lib/gcc-cross/x86_64-linux-gnu/14/{libgcc.a,libgcc_eh.a,libstdc++.a,libsupc++.a,libgfortran.a,libatomic.a,libgomp.a}
run syslapack-pic-a64 /usr/lib/aarch64-linux-gnu/liblapack_pic.a
# distribution static libraries built the default way (GCC's default -fPIE)
run sysstatic-a64 /usr/lib/aarch64-linux-gnu/{libc.a,libm-2.41.a,libblas.a,liblapack.a,libcrypt.a,libresolv.a}
run sysstatic-x64 /usr/x86_64-linux-gnu/lib/{libc.a,libm-2.41.a}
# rustc's standard library (the rlibs a cdylib links -Bstatic; workspace toolchain)
RL=/Volumes/AgentWork/mortar-helper/.toolchain/rustup/toolchains/1.95.0-aarch64-unknown-linux-gnu/lib/rustlib/aarch64-unknown-linux-gnu/lib
run rust-std-a64 $RL/libstd-*.rlib $RL/libcore-*.rlib $RL/liballoc-*.rlib $RL/libcompiler_builtins-*.rlib
# libtool twins (libtool-twins.sh WORK first): expat and pcre2 built by their own make
if ls $WORK/twins/gcc-a64.pic.*.o >/dev/null 2>&1; then
  run twins-pic $WORK/twins/gcc-a64.pic.*.o; run twins-pie $WORK/twins/gcc-a64.pie.*.o
  python3 -I "$HERE/groundtruth.py" $WORK/twins gcc-a64 > gt.twins.txt
fi
# objects the rule refuses that the references link (no symbol-named use)
python3 -I "$HERE/onlyrule.py" $C/gcc-a64.pie.*.o /usr/lib/gcc/aarch64-linux-gnu/14/libgomp.a > onlyrule.txt
echo "analysis done: $(ls *.txt | wc -l) groups"

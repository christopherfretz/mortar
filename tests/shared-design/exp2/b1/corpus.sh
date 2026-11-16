#!/bin/bash
# corpus.sh WORK: compile the B-1 corpus (lua 5.4.7 core, zlib 1.3.1, googletest
# 1.15.2, the census library, libstdc++-style C++ and the boundary cases) with
# GCC 14 and Clang 22 for AArch64 and x86-64 (GCC for i386, for the record), in
# the modes -fPIC, -fPIC -fno-semantic-interposition, -fPIC
# -fvisibility-inlines-hidden (C++), -fPIE; -O2 -g throughout.  4 jobs at most.
set -u
ulimit -c 0
WORK=$(realpath -m "$1"); HERE=$(cd "$(dirname "$0")" && pwd)
DL=/Volumes/AgentWork/mortar-helper/.build/real-builds/downloads
CEN=/Volumes/AgentWork/mortar-helper/.build/target-wp-110/exp/census/src
mkdir -p "$WORK/src" "$WORK/obj"; cd "$WORK" || exit 1
for a in zlib-1.3.1 lua-5.4.7 googletest-1.15.2; do mkdir -p src/$a; tar -xzf $DL/$a.tar.gz -C src/$a; done
Z=$WORK/src/zlib-1.3.1/zlib-1.3.1; L=$WORK/src/lua-5.4.7/lua-5.4.7/src
GT=$WORK/src/googletest-1.15.2/googletest-1.15.2/googletest
CSRC=""
for s in adler32 crc32 deflate infback inffast inflate inftrees trees zutil compress uncompr gzclose gzlib gzread gzwrite; do CSRC="$CSRC zlib:$Z/$s.c"; done
for f in $L/*.c; do b=$(basename $f .c); [ $b = lua ] || [ $b = luac ] || CSRC="$CSRC lua:$f"; done
CSRC="$CSRC census:$CEN/lib.c case:$HERE/src/boundary.c case:$HERE/src/anchorgap.c"
XSRC="gtest:$GT/src/gtest-all.cc gtest:$GT/src/gtest_main.cc census:$CEN/lib.cpp case:$HERE/src/vague.cc"
: > jobs.txt
emit() { # comp mode lang tag:file
  local comp=$1 mode=$2 lang=$3 tf=$4 tag=${4%%:*} f=${4#*:}
  local cc flags out
  case $comp in
    gcc-a64) cc=$([ $lang = c ] && echo gcc || echo g++);;
    gcc-x64) cc=$([ $lang = c ] && echo x86_64-linux-gnu-gcc || echo x86_64-linux-gnu-g++);;
    gcc-i386) cc=i686-linux-gnu-gcc;;
    clang-a64) cc=$([ $lang = c ] && echo clang-22 || echo clang++-22);;
    clang-x64) cc="$([ $lang = c ] && echo clang-22 || echo clang++-22) --target=x86_64-linux-gnu";;
  esac
  case $mode in pic) flags=-fPIC;; picnsi) flags="-fPIC -fno-semantic-interposition";; picvih) flags="-fPIC -fvisibility-inlines-hidden";; pie) flags=-fPIE;; esac
  inc="-I$Z -I$L -I$GT -I$GT/include -D_LARGEFILE64_SOURCE=1 -DHAVE_HIDDEN -DLUA_COMPAT_5_3 -DLUA_USE_LINUX -DGTEST_HAS_PTHREAD=1"
  std=$([ $lang = c ] && echo -std=gnu99 || echo -std=c++17)
  [ $tag = case ] && [ $lang = c ] && std=-std=gnu11
  out=obj/$comp.$mode.$tag.$(basename $f).o
  echo "$cc -O2 -g $flags $std $inc -c $f -o $out" >> jobs.txt
}
for comp in gcc-a64 gcc-x64 clang-a64 clang-x64; do
  for mode in pic picnsi pie; do for tf in $CSRC; do emit $comp $mode c $tf; done; done
  for mode in pic picnsi picvih pie; do for tf in $XSRC; do emit $comp $mode cxx $tf; done; done
done
for mode in pic pie; do for tf in $CSRC; do emit gcc-i386 $mode c $tf; done; done
echo "jobs $(wc -l < jobs.txt)"
xargs -P 4 -I{} sh -c '{} 2>>compile.err || echo "FAIL {}" >> fails.txt' < jobs.txt
echo "objects $(ls obj | wc -l) fails $(cat fails.txt 2>/dev/null | wc -l)"
find "$WORK/src" -mindepth 1 -delete
echo "corpus done"

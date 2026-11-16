#!/bin/bash
# libtool-twins.sh WORK: expat 2.6.4 and pcre2 10.44 built by their own
# configure + make (libtool compiles every source twice: -fPIC -DPIC into
# .libs/ and, for the static library, the compiler default, GCC's -fPIE);
# the twin objects are copied into WORK/twins/ as gcc-a64.{pic,pie}.<pkg>.<f>.o
# for groundtruth.py.  AArch64, GCC 14, -O2 (the packages' default -g -O2).
set -u
ulimit -c 0
WORK=$(realpath -m "$1"); DL=/Volumes/AgentWork/mortar-helper/.build/real-builds/downloads
mkdir -p "$WORK/twsrc" "$WORK/twins"; cd "$WORK/twsrc" || exit 1
export TMPDIR=$WORK/twsrc
mkdir -p expat pcre2
tar -xJf $DL/expat-2.6.4.tar.xz -C expat; tar -xjf $DL/pcre2-10.44.tar.bz2 -C pcre2
for p in expat/expat-2.6.4 pcre2/pcre2-10.44; do
  ( cd $p && CC=gcc ./configure --enable-shared --enable-static --without-docbook >/dev/null 2>&1 && make -j4 >/dev/null 2>&1 ) || echo "BUILD FAIL $p"
done
n=0
for pic in $(find . -path '*/.libs/*.o'); do
  d=$(dirname $(dirname $pic)); b=$(basename $pic); pie=$d/$b
  [ -f "$pie" ] || continue
  pkg=$(echo $pic | cut -d/ -f2)
  cp $pic "$WORK/twins/gcc-a64.pic.$pkg.$b"; cp $pie "$WORK/twins/gcc-a64.pie.$pkg.$b"; n=$((n+1))
done
echo "twin pairs $n"
find "$WORK/twsrc" -mindepth 1 -delete

#!/bin/sh
# The linked clang at work (WP-081, spec/landed/harness/WP-081.md "Correct
# first"): compiles the programs of this directory and a sample of LLVM's
# own sources with CLANG, links the programs (the driver's default GNU ld),
# runs them, and writes into OUT one line per object (its SHA-256) and per
# program (its exit status and the SHA-256 of its standard output). Two
# clangs linked from the same inputs must write the same OUT, byte for
# byte: the objects of a compiler are a pure function of its inputs.
#
# usage: programs.sh CLANG OUT [SRC BUILD COUNT]
#   CLANG   a clang 19 binary in <tree>/build/bin (its resource headers are
#           found beside it, in ../lib/clang/19)
#   OUT     a new directory
#   SRC BUILD COUNT   also compile the first COUNT sources of
#           llvm/lib/Support, llvm/lib/IR and clang/lib/Sema (in name
#           order) of the tree at -O2 -g
set -eu
CLANG=$1
OUT=$2
HERE=$(cd "$(dirname "$0")/programs" && pwd)
mkdir "$OUT"
cd "$OUT"
# The objects' DW_AT_comp_dir is not the directory they are made in.
dc=-fdebug-compilation-dir=/wp-081
for p in sieve.c templates.cc containers.cc exceptions.cc; do
  stem=${p%.*}
  case $p in
    *.c) mode=gcc; std=-std=c17 ;;
    *) mode=g++; std=-std=c++17 ;;
  esac
  "$CLANG" --driver-mode=$mode $std -O2 -g $dc -c "$HERE/$p" -o "$stem.o"
  "$CLANG" --driver-mode=$mode $std -O2 -c "$HERE/$p" -o "$stem.nog.o"
  "$CLANG" --driver-mode=$mode "$stem.o" -o "$stem" -pthread
  "$CLANG" --driver-mode=$mode "$stem.nog.o" -o "$stem.static" -static -pthread
  set +e
  ./"$stem" > "$stem.stdout"
  status=$?
  ./"$stem.static" > "$stem.static.stdout"
  sstatus=$?
  set -e
  echo "program $stem exit $status stdout $(sha256sum < "$stem.stdout" | cut -c1-16) static exit $sstatus stdout $(sha256sum < "$stem.static.stdout" | cut -c1-16)" >> summary
  echo "object $stem.o $(sha256sum < "$stem.o" | cut -c1-16)" >> summary
  echo "object $stem.nog.o $(sha256sum < "$stem.nog.o" | cut -c1-16)" >> summary
done
if [ $# -ge 5 ]; then
  SRC=$3
  BUILD=$4
  COUNT=$5
  inc="-I$SRC/llvm/include -I$BUILD/include -I$SRC/clang/include -I$BUILD/tools/clang/include"
  for dir in llvm/lib/Support llvm/lib/IR clang/lib/Sema; do
    ls "$SRC/$dir" | grep '[.]cpp$' | sort | head -n "$COUNT" | while read -r f; do
      o=$(echo "$dir/$f" | tr / _).o
      "$CLANG" --driver-mode=g++ -std=c++17 -O2 -g -fno-exceptions -fno-rtti $dc $inc \
        -c "$SRC/$dir/$f" -o "$o"
      echo "object $o $(sha256sum < "$o" | cut -c1-16)" >> summary
    done
  done
fi
cat summary

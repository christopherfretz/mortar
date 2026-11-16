#!/bin/bash
# WP-110 area `dyn`: shared-object output (-shared) measured against GNU ld 2.44, lld 22/19, mold 2.37.1
# on aarch64 (native), x86-64 and i386 (qemu-user).
# Usage: run.sh WORK   (WORK must not exist; it is created; every log lands in WORK/logs)
# Takes about 15-20 minutes, serially (one compiler or linker at a time).
set -u
if [ $# -ne 1 ]; then echo "usage: $0 WORK" >&2; exit 2; fi
WORK=$1
if [ -e "$WORK" ]; then echo "$WORK exists: give a fresh path" >&2; exit 2; fi
mkdir -p "$WORK/logs"
WORK=$(cd "$WORK" && pwd)
HERE=$(cd "$(dirname "$0")" && pwd)
# Several experiments crash on purpose (wrong outputs, patched headers): no core files, and run from WORK
# so that nothing is ever written to the caller's directory.
ulimit -c 0
cd "$WORK" || exit 2
{
  date -u
  uname -a
  for t in gcc x86_64-linux-gnu-gcc i686-linux-gnu-gcc clang-22; do echo "== $t"; $t --version | head -1; done
  echo "== ld.bfd"; ld.bfd --version | head -1
  echo "== x86_64-linux-gnu-ld"; x86_64-linux-gnu-ld --version | head -1
  echo "== i686-linux-gnu-ld"; i686-linux-gnu-ld --version | head -1
  echo "== ld.lld-22"; ld.lld-22 --version
  echo "== ld.lld-19"; ld.lld-19 --version
  echo "== mold"; mold --version
  echo "== qemu-x86_64"; qemu-x86_64 --version | head -1
  echo "== qemu-i386"; qemu-i386 --version | head -1
  echo "== readelf"; readelf --version | head -1
  echo "== llvm-objdump-22"; llvm-objdump-22 --version | sed -n 2p
  echo "== glibc"; ldd --version | head -1
  echo "linkers are chosen through the gcc driver: GNU ld = the driver default;"
  echo "lld/mold = -B WORK/bin/<name>/ holding a symlink 'ld' to /usr/bin/ld.lld-22, ld.lld-19, mold."
  echo "x86 compilers are the Debian cross gcc (sysroots /usr/x86_64-linux-gnu, /usr/i686-linux-gnu);"
  echo "x86 programs run with qemu-x86_64 -L /usr/x86_64-linux-gnu, qemu-i386 -L /usr/i686-linux-gnu."
} > "$WORK/logs/versions.log" 2>&1
for p in partA partB partC partD partE partF; do
  echo "[$(date -u +%T)] $p"
  python3 -I "$HERE/$p.py" "$WORK" > "$WORK/logs/$p.stdout" 2> "$WORK/logs/$p.stderr"
  echo "  rc=$?"
done
echo "logs in $WORK/logs: A.log B.log C.log D.log E.log F.log (+ every output's readelf dump beside it)"

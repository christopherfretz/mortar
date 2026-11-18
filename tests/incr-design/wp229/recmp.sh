#!/bin/bash
# Re-compares skel.sh's transcripts with the gdbstub port masked (x64).
# recmp.sh WORK: run from skel.sh's WORK directory.
cd "$1" || exit 1
m() { sed 's/localhost:[0-9]*/localhost:PORT/' "$1"; }
for d in a64-* x64-*; do
  for tag in dwo dwp; do for t in gdb lldb; do
    for p in "plain full" "full emul" "full emuldrop"; do set -- $p
      if cmp -s <(m $d/$1.$tag.$t) <(m $d/$2.$tag.$t); then r=SAME; else r="DIFF $(diff <(m $d/$1.$tag.$t) <(m $d/$2.$tag.$t) | grep -c '^[<>]')"; fi
      echo "$d $tag $t $2 vs $1: $r"
    done
  done; done
  for t in gdb lldb; do
    if cmp -s <(m $d/full.dwo.$t) <(m $d/full.dwp.$t); then r=SAME; else r="DIFF $(diff <(m $d/full.dwo.$t) <(m $d/full.dwp.$t) | grep -c '^[<>]')"; fi
    echo "$d dwp-vs-dwo $t full: $r"
  done
done

#!/bin/bash
# WP-258, the coordinator's first finding: llvm-dwarfdump --verify does not
# terminate on GCC -O2 -g objects. src/loop.cc (a DW_TAG_call_site, DWARF 4:
# DW_TAG_GNU_call_site, whose parent is a DW_TAG_lexical_block) compiled by GCC 14
# and Clang 19, DWARF 4 and 5, AArch64 and x86-64; --verify (llvm-dwarfdump-19 and
# -22, `timeout 30`) on the OBJECT, and on its static link with src/g.cc by
# mortar, GNU ld, lld 19, lld 22 and mold; then retag.py's probe: every
# DW_TAG_lexical_block abbreviation retagged DW_TAG_inlined_subroutine (0x0b ->
# 0x1d) in each output. With LINKS_OUT (links.sh's directory), the same for
# its <cfg>-o2 outputs of a.cc + b.cc.
#   verifyloop.sh OUT [MORTAR] [LINKS_OUT]   OUT/verifyloop.tsv; last line VERIFYLOOP DONE
ulimit -c 0
HERE="$(cd "$(dirname "$0")" && pwd)"
[ -e "$1" ] && { echo "OUT must not exist"; echo "VERIFYLOOP DONE (failed)"; exit 1; }
OUT="$1"; MORTAR="${2:-/Volumes/AgentWork/mortar-helper/.build/target-wp-143/release/mortar}"; LO="$3"
mkdir -p "$OUT"; OUT="$(cd "$OUT" && pwd)"
[ -x "$MORTAR" ] || { echo "no mortar"; echo "VERIFYLOOP DONE (failed)"; exit 1; }
BM="$OUT/bin-mortar"; L22="$OUT/bin-lld22"; mkdir -p "$BM" "$L22"
for n in ld x86_64-linux-gnu-ld aarch64-linux-gnu-ld; do ln -sf "$MORTAR" "$BM/$n"; done
ln -sf /usr/bin/ld.lld-22 "$L22/ld.lld"
vf() { # file label -> one tsv row per dwarfdump version
  for dd in 19 22; do
    local t0 rc n; t0=$(date +%s)
    timeout 30 llvm-dwarfdump-$dd --verify "$1" > "$1.v$dd" 2>&1; rc=$?
    if [ $rc = 124 ]; then n="TIMEOUT(30s)"; else n="rc=$rc first: $(grep -m1 '^error' "$1.v$dd" | cut -c1-90)"; fi
    printf '%s\tdwarfdump-%s\t%s (%ss)\n' "$2" $dd "$n" $(( $(date +%s) - t0 )) >> "$OUT/rows.$3"
  done
}
one() { # tgt cc v
  local tgt=$1 cc=$2 v=$3 D F W
  case $tgt-$cc in
   a64-gcc) D=g++-14; F=;; x64-gcc) D=x86_64-linux-gnu-g++-14; F=;;
   a64-clang) D=clang++-19; F=--target=aarch64-linux-gnu;; x64-clang) D=clang++-19; F=--target=x86_64-linux-gnu;;
  esac
  local cfg=$tgt-$cc-dw$v; W="$OUT/$cfg"; mkdir -p "$W"
  $D $F -O2 -g -gdwarf-$v -c "$HERE/src/loop.cc" -o "$W/loop.o"
  $D $F -O2 -c "$HERE/src/g.cc" -o "$W/g.o"
  echo "$cfg: $(llvm-dwarfdump-22 --debug-info "$W/loop.o" | grep -B3 -i 'call_site$' | grep -o 'DW_TAG_[A-Za-z_]*' | tr '\n' ' ')" >> "$OUT/rows.$cfg"
  vf "$W/loop.o" "$cfg object loop.o" $cfg
  local L="$F -static $W/loop.o $W/g.o"
  $D -B"$BM/" $L -o "$W/mortar" > /dev/null 2>&1
  $D $L -o "$W/gnu" > /dev/null 2>&1
  $D -fuse-ld=lld $L -o "$W/lld19" > /dev/null 2>&1
  $D -B"$L22/" -fuse-ld=lld $L -o "$W/lld22" > /dev/null 2>&1
  $D -fuse-ld=mold $L -o "$W/mold" > /dev/null 2>&1
  for x in mortar gnu lld19 lld22 mold; do
    [ -f "$W/$x" ] || { echo "$cfg $x: no output" >> "$OUT/rows.$cfg"; continue; }
    vf "$W/$x" "$cfg $x" $cfg
  done
  python3 -I "$HERE/retag.py" "$W/mortar" "$W/mortar-retag" 0x0b 0x1d > /dev/null
  vf "$W/mortar-retag" "$cfg mortar, lexical_block retagged inlined_subroutine" $cfg
}
for tgt in a64 x64; do for cc in gcc clang; do ( one $tgt $cc 4; one $tgt $cc 5 ) & done; done
wait
if [ -n "$LO" ]; then
  for c in a64-gcc-dw4-o2 a64-gcc-dw5-o2 x64-gcc-dw4-o2 x64-gcc-dw5-o2; do
    [ -f "$LO/$c/mortar" ] || continue
    ( for x in mortar gnu; do
        python3 -I "$HERE/retag.py" "$LO/$c/$x" "$OUT/$c-$x-retag" 0x0b 0x1d > /dev/null
        vf "$OUT/$c-$x-retag" "links $c $x, lexical_block retagged" links-$c
      done ) &
  done
  wait
fi
cat "$OUT"/rows.* > "$OUT/verifyloop.tsv"
echo "VERIFYLOOP DONE"

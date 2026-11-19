#!/bin/bash
# WP-258 item 1: the full (plain) link of src/a.cc + src/b.cc (b's copies of
# wp258::twice and wp258::Box<long>::get are dropped) by mortar, GNU ld, lld 19,
# lld 22 and mold (gold is not installed), for g++ 14 and clang++ 19, DWARF 4 and
# 5, AArch64 and x86-64, -static; 'eq' as written (the two functions have
# equal sizes on AArch64, 0x18), 'ne' with -DWP258_NE (twice grows: sizes differ),
# 'o2': -O2 -DWP258_O2 (twice noinline: .debug_loc/.debug_loclists refer to it). Per output: llvm-dwarfdump-22 --verify's
# error count and its overlapping-range lines, and fields.py's bytes for each of
# b.o's references into its dropped COMDAT code. 'ones-rng' (DWARF 5): mortar's
# output patched to all ones in .debug_rnglists/.debug_addr/.debug_loclists.
#   links.sh OUT [MORTAR]   OUT/verify.tsv OUT/fields.tsv, OUT/<cfg>/ (objects, outputs)
# Last line: LINKS DONE.
ulimit -c 0
HERE="$(cd "$(dirname "$0")" && pwd)"
[ -e "$1" ] && { echo "OUT must not exist (no stale outputs)"; echo "LINKS DONE (failed)"; exit 1; }
OUT="$1"; MORTAR="${2:-/Volumes/AgentWork/mortar-helper/.build/target-wp-143/release/mortar}"
mkdir -p "$OUT"; OUT="$(cd "$OUT" && pwd)"
BM="$OUT/bin-mortar"; L22="$OUT/bin-lld22"
mkdir -p "$BM" "$L22"
for n in ld x86_64-linux-gnu-ld aarch64-linux-gnu-ld; do ln -sf "$MORTAR" "$BM/$n"; done
ln -sf /usr/bin/ld.lld-22 "$L22/ld.lld"
[ -x "$MORTAR" ] || { echo "no mortar at $MORTAR"; echo "LINKS DONE (failed)"; exit 1; }
: > "$OUT/verify.tsv"; : > "$OUT/fields.tsv"
for tgt in a64 x64; do
 for cc in gcc clang; do
  case $tgt-$cc in
   a64-gcc) D=g++-14; F=;;
   x64-gcc) D=x86_64-linux-gnu-g++-14; F=;;
   a64-clang) D=clang++-19; F=--target=aarch64-linux-gnu;;
   x64-clang) D=clang++-19; F=--target=x86_64-linux-gnu;;
  esac
  for v in 4 5; do
  for sz in eq ne o2; do
   case $sz in eq) S=-O0;; ne) S="-O0 -DWP258_NE";; o2) S="-O2 -DWP258_O2";; esac
   cfg=$tgt-$cc-dw$v-$sz; W="$OUT/$cfg"; mkdir -p "$W"
   for f in a b; do
     $D $F -g -gdwarf-$v $S -c "$HERE/src/$f.cc" -o "$W/$f.o" || echo "$cfg compile $f failed"
   done
   L="$F -static $W/a.o $W/b.o"
   $D -B"$BM/" $F -Wl,--version 2>/dev/null | grep -q '^mortar' || { echo "$cfg: -B does not reach mortar"; continue; }
   $D -B"$BM/" $L -o "$W/mortar" > "$W/mortar.err" 2>&1 || echo "$cfg mortar failed: $(head -2 "$W/mortar.err")"
   $D $L -o "$W/gnu" > /dev/null 2>&1 || echo "$cfg gnu failed"
   $D -fuse-ld=lld $L -o "$W/lld19" > /dev/null 2>&1 || echo "$cfg lld19 failed"
   $D -B"$L22/" -fuse-ld=lld $L -o "$W/lld22" > /dev/null 2>&1 || echo "$cfg lld22 failed"
   $D -fuse-ld=mold $L -o "$W/mold" > /dev/null 2>&1 || echo "$cfg mold failed"
   # the probe of the alternative: mortar's output with the dropped copies'
   # .debug_rnglists/.debug_addr/.debug_loclists fields set to all ones
   [ $v = 5 ] && [ -f "$W/mortar" ] && python3 -I "$HERE/fields.py" "$W/mortar" "$W/b.o" \
     --patch "$W/ones-rng" .debug_rnglists,.debug_addr,.debug_loclists ones > /dev/null && chmod 755 "$W/ones-rng"
   for x in mortar gnu lld19 lld22 mold ones-rng; do
     [ -f "$W/$x" ] || continue
     t0=$(date +%s)
     timeout 120 llvm-dwarfdump-22 --verify "$W/$x" > "$W/$x.verify" 2>&1; rc=$?
     t1=$(date +%s)
     if [ $rc = 124 ] || [ $rc = 137 ]; then n="TIMEOUT(120s)"
     else n=$(grep '^error:' "$W/$x.verify" | grep -vc 'Aggregated error counts\|occurred [0-9]* time'); fi
     n="$n ($((t1 - t0))s)"
     ov=$(grep 'overlapping ranges' "$W/$x.verify" | sed 's/.*attribute: //' | tr '\n' ' ')
     printf '%s\t%s\terrors=%s\t%s\n' "$cfg" "$x" "$n" "$ov" >> "$OUT/verify.tsv"
     python3 -I "$HERE/fields.py" "$W/$x" "$W/b.o" | sed "s/^/$cfg\t$x\t/" >> "$OUT/fields.tsv"
   done
  done
  done
 done
done
echo "LINKS DONE"

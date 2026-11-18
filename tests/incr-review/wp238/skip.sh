#!/bin/bash
# skip.sh PROBE W -- WP-238 review, item 3 (record §4's skip rule).
# K1: c.s's `.rodata.m` holds two strings, `msg` at 0 and `msg2` at 6;
#     v0 plain ("a"), v1 SHF_MERGE|SHF_STRINGS ("aMS",1). Same section
#     index, same values, no rows of its own; m.o (unchanged) names msg2.
#     §4's conditions 1-5 hold (rows equal, no definition moved, the
#     section index and value of msg2 equal); v1's live pieces are NEW
#     (v0's section had none), so "pieces taken from the stored marks"
#     has nothing to take.
# K2: c.c's `w` changes binding weak -> global and type object -> func
#     style size change, same (file, index); m.o names w.
# Last line FINAL.
PROBE=$1; W=$2
source "$(cd "$(dirname "$0")" && pwd)/lib.sh"
for arch in a64 x64; do
  if [ $arch = a64 ]; then CC=gcc; else CC=x86_64-linux-gnu-gcc; fi
  for v in 0 1; do
    d=$W/k1-$arch/v$v; mkdir -p "$d"
    if [ $v = 0 ]; then fl='"a",%progbits'; else fl='"aMS",%progbits,1'; fi
    printf '  .section .rodata.m,%s\n  .globl msg\n  .globl msg2\nmsg:\n  .asciz "hello"\nmsg2:\n  .asciz "world"\n' "$fl" > "$d/c.s"
    printf 'extern const char msg2[];\nint main(void){ return msg2[1]; }\n' > "$d/m.c"
    (cd "$d" && $CC -c c.s) || echo "compile failed"
    if [ $v = 0 ]; then (cd "$d" && $CC -O1 -c m.c); else cp "$W/k1-$arch/v0/m.o" "$d/"; fi
    eval "r$v=\$(probe_link $d $CC -static -Wl,--gc-sections m.o c.o -o prog)"
    rr=$(eval echo \$r$v)
    sid=$(awk -F'\t' '$1=="S" && $5==".rodata.m"{print $2}' "$rr")
    echo "k1 $arch v$v: $(sec_live "$rr" .rodata.m) split-pieces(off:frag:live) $(awk -F'\t' -v s="$sid" 'NR==FNR{if($1=="L")l[$2]=1; next} $1=="P" && $2==s {printf "%s:%s:%d ", $3, $4, ($4 in l)}' "$rr" "$rr") msg2: $(awk -F'\t' '$1=="K" && $3=="msg2"{print $4}' "$rr")"
    rm -f "$d/prog"
  done
  echo "k1 $arch fwd:"; emu "$r0" "$r1" --roots _start
  echo "k1 $arch back:"; emu "$r1" "$r0" --roots _start
  for v in 0 1; do
    d=$W/k2-$arch/v$v; mkdir -p "$d"
    if [ $v = 0 ]; then printf '__attribute__((weak)) int w[2] = {1, 2};\nint other = 5;\n' > "$d/c.c"
    else printf 'int w[4] = {1, 2, 3, 4};\nint other = 5;\n' > "$d/c.c"; fi
    printf 'extern int w[];\nint main(void){ return w[1]; }\n' > "$d/m.c"
    (cd "$d" && $CC -O1 -fdata-sections -fno-common -c c.c) || echo "compile failed"
    if [ $v = 0 ]; then (cd "$d" && $CC -O1 -c m.c); else cp "$W/k2-$arch/v0/m.o" "$d/"; fi
    eval "r$v=\$(probe_link $d $CC -static -Wl,--gc-sections m.o c.o -o prog)"
    rm -f "$d/prog"
  done
  echo "k2 $arch fwd:"; emu "$r0" "$r1" --roots _start
  echo "k2 $arch back:"; emu "$r1" "$r0" --roots _start
done
echo "FINAL skip"

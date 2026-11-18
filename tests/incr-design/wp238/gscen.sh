#!/bin/bash
# gscen.sh PROBE WORK -- WP-238 step 1, experiments (a), (c), (d) on the
# edge kinds gen200 lacks: the review's g scenario (spec/incr.md IN-29, the
# L10 case `gc-edge-kinds`): u.c, unchanged, has a registry section `ureg`
# reached only through m's __start_ureg/__stop_ureg (startstop), keepf()
# and dropf() each with an alloc SHF_LINK_ORDER section nothing references
# (.meta_k, .meta_d: linkorder), and a COMDAT group gsig of .text.gfun
# (called) and .rodata.gaux (referenced by nothing: group), and a non-alloc
# section .nanote whose record names .rodata.nref by a local label
# (nonalloc_section). m0.c calls
# keepf and gfun; m1.c also dropf, so .text.dropf and .meta_d of the
# UNCHANGED u.o flip. Per (target, compiler, version): a full link with
# the probe (replay and drops), the program's output; then emulate.py
# v0 -> v1 and v1 -> v0. Last line FINAL.
ulimit -c 0
P=$1; W=$2
mkdir -p "$W/bin"; ln -sf "$P" "$W/bin/ld"
here=$(cd "$(dirname "$0")" && pwd)
for arch in a64 x64; do
  if [ $arch = a64 ]; then gf='mov w0, #7\n ret'; else gf='movl $7, %eax\n ret'; fi
  S=$W/src-$arch; mkdir -p "$S"
  cat > "$S/u.c" <<E3
__attribute__((section("ureg"), used)) const int ureg_a = 11;
__attribute__((section("ureg"), used)) const int ureg_b = 31;
int keepf(int x){ asm volatile(".pushsection .meta_k,\"ao\",%progbits,keepf\n.long 0x6b6b6b6b\n.popsection"); return x * 3; }
int dropf(int x){ asm volatile(".pushsection .meta_d,\"ao\",%progbits,dropf\n.long 0x64646464\n.popsection"); return x * 5; }
asm(".pushsection .text.gfun,\"axG\",%progbits,gsig,comdat\n.globl gfun\n.type gfun,%function\n.p2align 2\ngfun:\n $gf\n.size gfun,.-gfun\n.popsection");
asm(".pushsection .rodata.nref,\"a\",%progbits\n.Lnref:\n.long 0x6e6e6e6e\n.popsection\n.pushsection .nanote,\"\",%progbits\n.quad .Lnref\n.popsection");
asm(".pushsection .rodata.gaux,\"aG\",%progbits,gsig,comdat\n.long 0x67676767\n.popsection");
E3
  printf '#include <stdio.h>\nextern const int __start_ureg[], __stop_ureg[];\nint keepf(int); int gfun(void);\nint main(void){ int s = 0; for (const int *p = __start_ureg; p < __stop_ureg; p++) s += *p; printf("%%d %%d %%d\\n", s, keepf(2), gfun()); return 0; }\n' > "$S/m0.c"
  printf '#include <stdio.h>\nextern const int __start_ureg[], __stop_ureg[];\nint keepf(int); int dropf(int); int gfun(void);\nint main(void){ int s = 0; for (const int *p = __start_ureg; p < __stop_ureg; p++) s += *p; printf("%%d %%d %%d %%d\\n", s, keepf(2), gfun(), dropf(2)); return 0; }\n' > "$S/m1.c"
  for cc in gcc clang; do
    if [ $arch = a64 ]; then
      if [ $cc = gcc ]; then CC="gcc"; else CC="clang"; fi; RUN=""
    else
      if [ $cc = gcc ]; then CC="x86_64-linux-gnu-gcc"; else CC="clang --target=x86_64-linux-gnu"; fi; RUN="qemu-x86_64"
    fi
    for opt in O0 O2; do
      cfg=$arch-$cc-$opt
      for v in 0 1; do
        d=$W/$cfg/v$v; mkdir -p "$d/dump"
        $CC -$opt -g -ffunction-sections -c "$S/u.c" -o "$d/u.o" || { echo "$cfg v$v: compile u failed"; continue; }
        $CC -$opt -g -ffunction-sections -c "$S/m$v.c" -o "$d/m.o" || { echo "$cfg v$v: compile m failed"; continue; }
        (cd "$d" && MORTAR_WP238=$d/dump MORTAR_WP238_ROWS=1 $CC -static -B"$W/bin" -Wl,--gc-sections m.o u.o -o prog > link.txt 2>&1) \
          || { echo "$cfg v$v: link failed: $(head -c 300 "$d/link.txt")"; continue; }
        out=$(cd "$d" && $RUN ./prog 2>&1)
        sum=$(ls "$d"/dump/*.sum | head -1)
        echo "$cfg v$v: run '$out'; $(grep '^replay' "$sum"); $(grep -E '^kind (startstop|linkorder|group|nonalloc_section) ' "$sum" | awk '{printf "%s %s ", $2, $6}')"
        grep -E '^drop (startstop|linkorder|group|nonalloc_section)' "$sum" | sed "s/^/  $cfg v$v /"
        rm -f "$d/prog"
      done
      r0=$(ls "$W/$cfg"/v0/dump/*.rows 2>/dev/null | head -1); r1=$(ls "$W/$cfg"/v1/dump/*.rows 2>/dev/null | head -1)
      if [ -n "$r0" ] && [ -n "$r1" ]; then
        echo "EMU $cfg fwd: $(python3 -I "$here/emulate.py" "$r0" "$r1" | tr '\n' ' ' | cut -c1-400)"
        echo "EMU $cfg back: $(python3 -I "$here/emulate.py" "$r1" "$r0" | tr '\n' ' ' | cut -c1-400)"
      fi
    done
  done
done
echo "FINAL gscen"

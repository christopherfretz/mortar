#!/bin/bash
# newscen.sh PROBE W -- WP-238 step 2 Part 0: F6's inputs and the review's
# open questions, both 64-bit targets, each as v0/v1 (v1 edits ONE file),
# single links and updates both ways through oracle_design.py.
#  f6c  `-u onlya`, onlya defined only in a.o's COMDAT copy of group grp,
#       which loses to b.o's copy (b.o first); v1 edits m.c.
#  f6x  `-u exk`, exk defined only in e.o's SHF_EXCLUDE section; v1 edits m.c.
#  f6xr as f6x but m.o's .data names exk through a relocation (the link
#       may refuse: shown as it is).
#  scr  an implicit input script `x = 1;` (an assignment).
#  ext  an implicit input script `EXTERN(kfun)`; kfun only in k.o's
#       .text.kfun; v1 edits k.c.
#  ring a group rg of .text.r, .debug_r (non-alloc, debug) and .rodata.r,
#       with -Wl,--strip-debug; m.o calls rfun; v1 edits m.c.
#  dss  --defsym=__start_myx=0x1000 with a kept section myx named by
#       __start_myx and __stop_myx from m.o; v1 edits m.c.
#  ifr  -fini=myfini --require-defined=rd; v1 edits b.c.
PROBE=$1; W=$2
source @LIB@
two() { # two NAME CC LINKARGS... (objects already in $W/NAME-$arch/v0 v1)
  local n=$1 cc=$2; shift 2
  local d0=$W/$n-$arch/v0 d1=$W/$n-$arch/v1
  local r0 r1
  r0=$(probe_link "$d0" "$cc" "$@" -o prog); r1=$(probe_link "$d1" "$cc" "$@" -o prog)
  if [ -z "$r0" ] || [ -z "$r1" ]; then echo "$n $arch: LINK FAILED: $(head -c 400 "$d0/link.txt" | tr '\n' ' ')"; return; fi
  echo "$n $arch:"; emu "$r0" "$r1"
  echo "$n $arch back:"; echo "  ORACLE update:     $(python3 -I "$design/oracle_design.py" "$r1" "$r0" | tail -1 | cut -c1-700)"
}
for arch in a64 x64; do
  if [ $arch = a64 ]; then CC=gcc; RET=ret; else CC=x86_64-linux-gnu-gcc; RET=ret; fi
  for v in 0 1; do
    # f6c
    d=$W/f6c-$arch/v$v; mkdir -p "$d"
    printf 'int gfun(void);\nint main(void){ return gfun() + %d; }\n' $v > "$d/m.c"
    printf '  .section .text.g,"axG",%%progbits,grp,comdat\n  .globl gfun\n  .type gfun,%%function\ngfun:\n  %s\n' $RET > "$d/b.s"
    printf '  .section .text.g,"axG",%%progbits,grp,comdat\n  .globl gfun\n  .globl onlya\n  .type gfun,%%function\ngfun:\nonlya:\n  %s\n  .section .text.a2,"ax",%%progbits\n  .globl a2\na2:\n  %s\n' $RET $RET > "$d/a.s"
    (cd "$d" && $CC -O1 -c m.c b.s a.s) || echo "f6c compile failed"
    # f6x / f6xr
    d=$W/f6x-$arch/v$v; mkdir -p "$d"
    printf 'int main(void){ return %d; }\n' $v > "$d/m.c"
    printf '  .section .disc,"ae",%%progbits\n  .globl exk\nexk:\n  .long 7\n  .section .text.e,"ax",%%progbits\n  .globl efun\nefun:\n  %s\n' $RET > "$d/e.s"
    (cd "$d" && $CC -O1 -c m.c e.s) || echo "f6x compile failed"
    d=$W/f6xr-$arch/v$v; mkdir -p "$d"
    printf 'extern int exk;\nint *p = &exk;\nint main(void){ return %d + (p != 0); }\n' $v > "$d/m.c"
    cp "$W/f6x-$arch/v0/e.s" "$d/e.s"
    (cd "$d" && $CC -O1 -c m.c e.s) || echo "f6xr compile failed"
    # scr
    d=$W/scr-$arch/v$v; mkdir -p "$d"
    printf 'int main(void){ return %d; }\n' $v > "$d/m.c"
    printf 'x = 1;\n' > "$d/s.ld"
    (cd "$d" && $CC -O1 -c m.c) || echo "scr compile failed"
    # ext
    d=$W/ext-$arch/v$v; mkdir -p "$d"
    printf 'int main(void){ return 0; }\n' > "$d/m.c"
    printf 'int kfun(void){ return 3%d; }\nint kother(void){ return 1; }\n' $v > "$d/k.c"
    printf 'EXTERN(kfun)\n' > "$d/s.ld"
    (cd "$d" && $CC -O1 -ffunction-sections -c m.c k.c) || echo "ext compile failed"
    # ring
    d=$W/ring-$arch/v$v; mkdir -p "$d"
    printf 'int rfun(void);\nint main(void){ return rfun() + %d; }\n' $v > "$d/m.c"
    cat > "$d/r.s" <<EOF
  .section .text.r,"axG",%progbits,rg,comdat
  .globl rfun
  .type rfun,%function
rfun:
  $RET
  .section .debug_r,"G",%progbits,rg,comdat
  .long 1
  .section .rodata.r,"aG",%progbits,rg,comdat
  .long 2
  .section .text.other,"ax",%progbits
  .globl other
other:
  $RET
EOF
    (cd "$d" && $CC -O1 -g -c m.c r.s) || echo "ring compile failed"
    # dss
    d=$W/dss-$arch/v$v; mkdir -p "$d"
    printf 'extern char __start_myx[], __stop_myx[];\n__attribute__((section("myx"),used)) int myx_a = 1;\nint main(void){ return (int)(__stop_myx - __start_myx) + %d; }\n' $v > "$d/m.c"
    (cd "$d" && $CC -O1 -c m.c) || echo "dss compile failed"
    # ifr
    d=$W/ifr-$arch/v$v; mkdir -p "$d"
    printf 'int main(void){ return 0; }\n' > "$d/a.c"
    printf 'void myfini(void){ }\nint rd(void){ return 4%d; }\nint other(void){ return 1; }\n' $v > "$d/b.c"
    (cd "$d" && $CC -O1 -ffunction-sections -c a.c b.c) || echo "ifr compile failed"
    if [ $v = 1 ]; then
      for n in f6c:b.o f6c:a.o f6x:e.o f6xr:e.o scr:m.o ext:m.o ring:r.o dss:m.o ifr:a.o; do
        cp "$W/${n%%:*}-$arch/v0/${n#*:}" "$W/${n%%:*}-$arch/v1/"
      done
    fi
  done
  two f6c $CC -static -Wl,--gc-sections -Wl,-u,onlya m.o b.o a.o
  two f6x $CC -static -Wl,--gc-sections -Wl,-u,exk m.o e.o
  two f6xr $CC -static -Wl,--gc-sections m.o e.o
  two scr $CC -static -Wl,--gc-sections m.o s.ld
  two ext $CC -static -Wl,--gc-sections m.o k.o s.ld
  two ring $CC -static -Wl,--gc-sections -Wl,--strip-debug m.o r.o
  two dss $CC -static -Wl,--gc-sections -Wl,--defsym=__start_myx=0x1000 m.o
  two ifr $CC -static -Wl,--gc-sections -Wl,-fini=myfini -Wl,--require-defined=rd a.o b.o
done
echo "FINAL newscen"

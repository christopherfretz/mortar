#!/bin/bash
# roots.sh PROBE W -- WP-238 review, items 1 and 2: roots and key readers
# the design does not carry. Per target (a64 native, x64 cross):
#  s1 `-Wl,-init=myinit` in a static --gc-sections link: myinit's section
#     is live only through the -init root (resolve's `roots`, not
#     `OptionKeys`); the update changes b.o (myinit's file).
#  s2 a non-alloc, non-debug section `.myinfo` of u.o names __start_ureg:
#     `ureg` is live only through that row (kind nonalloc_key, which
#     IN-29 resolves "as reloc": an Internal key gives nothing); the
#     update changes u.o.
# Last line FINAL.
PROBE=$1; W=$2
source "$(cd "$(dirname "$0")" && pwd)/lib.sh"
for arch in a64 x64; do
  if [ $arch = a64 ]; then CC=gcc; else CC=x86_64-linux-gnu-gcc; fi
  # s1
  for v in 0 1; do
    d=$W/s1-$arch/v$v; mkdir -p "$d"
    printf 'int main(void){ return 0; }\n' > "$d/a.c"
    printf 'int myinit(void){ return 4%d; }\nint other(void){ return 1; }\n' $v > "$d/b.c"
    (cd "$d" && $CC -O1 -g -ffunction-sections -c a.c b.c) || echo "s1 compile failed"
    # same object for a.o in both versions: copy v0's
    [ $v = 1 ] && cp "$W/s1-$arch/v0/a.o" "$d/a.o"
    eval "r$v=\$(probe_link $d $CC -static -Wl,--gc-sections -Wl,-init=myinit a.o b.o -o prog)"
    echo "s1 $arch v$v: $(sec_live "$(eval echo \$r$v)" .text.myinit)"
    rm -f "$d/prog"
  done
  echo "s1 $arch fwd (b.o changed):"; emu "$r0" "$r1" --roots _start
  echo "s1 $arch back:"; emu "$r1" "$r0" --roots _start
  # s2
  for v in 0 1; do
    d=$W/s2-$arch/v$v; mkdir -p "$d"
    printf 'int main(void){ return 0; }\n' > "$d/a.c"
    cat > "$d/u.c" <<EOF
__attribute__((section("ureg"), used)) const int ureg_a = 1$v;
int helper(void){ return $v; }
asm(".pushsection .myinfo,\"\",%progbits\n.quad __start_ureg\n.quad __stop_ureg\n.popsection");
EOF
    (cd "$d" && $CC -O1 -g -ffunction-sections -c a.c u.c) || echo "s2 compile failed"
    [ $v = 1 ] && cp "$W/s2-$arch/v0/a.o" "$d/a.o"
    eval "r$v=\$(probe_link $d $CC -static -Wl,--gc-sections a.o u.o -o prog)"
    rr=$(eval echo \$r$v)
    echo "s2 $arch v$v: $(sec_live "$rr" ureg) $(sec_live "$rr" .myinfo)"
    echo "  rows naming __start_ureg: $(awk -F'\t' '$1=="K" && $3=="__start_ureg"{k=$2} $1=="E" && $4=="K" && $5==k {print "src="$2" kind="$3}' "$rr" | tr '\n' ' ')"
    rm -f "$d/prog"
  done
  echo "s2 $arch fwd (u.o changed):"; emu "$r0" "$r1" --roots _start
  echo "s2 $arch back:"; emu "$r1" "$r0" --roots _start
done
echo "FINAL roots"

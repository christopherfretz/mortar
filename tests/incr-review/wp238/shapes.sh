#!/bin/bash
# shapes.sh PROBE W [only] -- WP-238 review, item 5: link shapes outside
# the author's corpus, each in two versions (v1 edits ONE file; the other
# objects are copied from v0 so their bytes are equal). Per shape and
# target: the probe's replay of both full links (sections/fragments
# differ, expected 0) and both emulations of the update v0 -> v1 and
# v1 -> v0. Last line FINAL.
PROBE=$1; W=$2; ONLY=${3:-}
source "$(cd "$(dirname "$0")" && pwd)/lib.sh"
KILN=${MASON_KILN:-/Volumes/AgentWork/mortar-helper/.build/target-kiln-next/release/kiln++}
HOD=${MASON_HOD:-/Volumes/AgentWork/mortar-helper/.build/target-hod-a68/release/hod}

# pair NAME CC ROOTS LINKARGS... : v0/ and v1/ under $W/NAME hold the
# objects; links both with the probe, then emulates both ways.
pair() {
  local name=${L:-}$1 dir=$1 cc=$2 roots=$3; shift 3
  local d0=$W/$dir/v0 d1=$W/$dir/v1 r0 r1
  r0=$(probe_link "$d0" "$cc" "$@" -o prog); r1=$(probe_link "$d1" "$cc" "$@" -o prog)
  rm -f "$d0/prog" "$d1/prog"
  if [ -z "$r0" ] || [ -z "$r1" ]; then echo "$name: LINK FAILED"; return; fi
  echo "$name: v0 $(grep -h '^replay' "$d0"/dump/*.sum) | v1 $(grep -h '^replay' "$d1"/dump/*.sum)"
  echo "$name: kinds v0 $(grep -h '^kind' "$d0"/dump/*.sum | awk '$6>0{printf "%s=%s ", $2, $6}')"
  echo "$name fwd:"; emu "$r0" "$r1" --roots "$roots"
  echo "$name back:"; emu "$r1" "$r0" --roots "$roots"
}
want() { [ -z "$ONLY" ] || [[ ",$ONLY," == *",$1,"* ]]; }

for arch in a64 x64; do
  if [ $arch = a64 ]; then CC=gcc; CXX=g++; FC=gfortran; CL="clang"; CLX="clang++"; KT=""
  else CC=x86_64-linux-gnu-gcc; CXX=x86_64-linux-gnu-g++; FC=x86_64-linux-gnu-gfortran
    CL="clang --target=x86_64-linux-gnu"; CLX="clang++ --target=x86_64-linux-gnu"; KT="--target=x86_64-linux-gnu"; fi

  # 1. C++ exceptions, -O2 -g -ffunction-sections -fdata-sections, gcc and clang;
  #    static, dynamic and PIE. y.cpp edited (a new throw site and call).
  if want cxx; then
  for comp in gcc clang; do
    if [ $comp = gcc ]; then X=$CXX; else X=$CLX; fi
    for v in 0 1; do
      d=$W/cxx-$comp-$arch/v$v; mkdir -p "$d"
      cat > "$d/x.cpp" <<'EOF'
#include <stdexcept>
#include <string>
struct Base { virtual ~Base() {} virtual int f() const { return 1; } };
struct D : Base { int f() const override { return 2; } };
template <class T> T twice(T t) { return t + t; }
int y(int);
int unused_x(int a) { return twice(a) * 7; }
int main(int argc, char **) {
  try { D d; const Base &b = d; return y(argc) + b.f() + twice(argc); }
  catch (const std::exception &e) { return (int)std::string(e.what()).size(); }
}
EOF
      if [ $v = 0 ]; then
        printf '#include <stdexcept>\ntemplate <class T> T twice(T t) { return t + t; }\nint y(int a) { if (a > 5) throw std::runtime_error("big"); return twice(a); }\nint unused_y(int a) { throw std::logic_error("never"); }\n' > "$d/y.cpp"
      else
        printf '#include <stdexcept>\n#include <vector>\ntemplate <class T> T twice(T t) { return t + t; }\nint y(int a) { std::vector<int> v(a); if (a > 5) throw std::out_of_range("big"); try { return twice(a) + (int)v.at(0); } catch (...) { return -1; } }\nint unused_y(int a) { throw std::logic_error("never"); }\n' > "$d/y.cpp"
      fi
      (cd "$d" && $X -O2 -g -ffunction-sections -fdata-sections -c y.cpp) || echo "compile failed"
      if [ $v = 0 ]; then (cd "$d" && $X -O2 -g -ffunction-sections -fdata-sections -c x.cpp); else cp "$W/cxx-$comp-$arch/v0/x.o" "$d/"; fi
    done
    L=static: ; pair cxx-$comp-$arch "$X" _start -static -Wl,--gc-sections x.o y.o
    L=dynamic: ; pair cxx-$comp-$arch "$X" _start -Wl,--gc-sections x.o y.o
    L=pie: ; pair cxx-$comp-$arch "$X" _start -pie -Wl,--gc-sections x.o y.o
  done
  L=
  fi

  # 2. -fno-asynchronous-unwind-tables mixed with unwind tables (C, gcc);
  #    -fno-exceptions -fno-unwind-tables on one file.
  if want nouw; then
  for v in 0 1; do
    d=$W/nouw-$arch/v$v; mkdir -p "$d"
    printf 'int g(int);\nint f(int a){ return g(a) + 1; }\nint dead_f(int a){ return a * 9; }\nint main(int c, char **v){ return f(c); }\n' > "$d/a.c"
    printf 'int g(int a){ return a * %d; }\nint dead_g(int a){ return a - 3; }\n' $((v + 2)) > "$d/b.c"
    (cd "$d" && $CC -O2 -g -ffunction-sections -fno-asynchronous-unwind-tables -fno-unwind-tables -c b.c) || echo "compile failed"
    if [ $v = 0 ]; then (cd "$d" && $CC -O2 -g -ffunction-sections -c a.c); else cp "$W/nouw-$arch/v0/a.o" "$d/"; fi
  done
  pair nouw-$arch "$CC" _start -static -Wl,--gc-sections a.o b.o
  fi

  # 3. Fortran with COMMON blocks (gfortran, -O2 -g), main in C? no: a
  #    Fortran program; s.f edited (a new COMMON member use).
  if want fort; then
  for v in 0 1; do
    d=$W/fort-$arch/v$v; mkdir -p "$d"
    printf '      program p\n      common /blk/ a, b\n      real a, b\n      a = 1.0\n      call s\n      print *, a, b\n      end\n' > "$d/p.f"
    if [ $v = 0 ]; then
      printf '      subroutine s\n      common /blk/ a, b\n      real a, b\n      b = a + 1.0\n      end\n      subroutine t\n      common /cc/ z\n      z = 2.0\n      end\n' > "$d/s.f"
    else
      printf '      subroutine s\n      common /blk/ a, b\n      common /cc/ z\n      real a, b\n      b = a + z\n      end\n      subroutine t\n      common /cc/ z\n      z = 2.0\n      end\n' > "$d/s.f"
    fi
    (cd "$d" && $FC -O2 -g -ffunction-sections -fdata-sections -c s.f) || echo "compile failed"
    if [ $v = 0 ]; then (cd "$d" && $FC -O2 -g -ffunction-sections -fdata-sections -c p.f); else cp "$W/fort-$arch/v0/p.o" "$d/"; fi
  done
  pair fort-$arch "$FC" _start -static -Wl,--gc-sections p.o s.o
  fi

  # 4. Hand-written assembly: SHF_GNU_RETAIN, a SHF_LINK_ORDER chain
  #    (.lo2 -> .lo1 -> .text.k), mergeable strings referenced with
  #    addends into the middle of a string (section symbol + addend, and a
  #    local label + addend), a non-alloc section naming a piece. u.s
  #    unchanged; m.c edited (calls k or not).
  if want asm; then
  for v in 0 1; do
    d=$W/asm-$arch/v$v; mkdir -p "$d"
    if [ $arch = a64 ]; then RET=ret; else RET=ret; fi
    cat > "$d/u.s" <<EOF
  .section .text.k,"ax",%progbits
  .globl k
  .type k,%function
k:
  $RET
  .size k,.-k
  .section .lo1,"ao",%progbits,.text.k
  .long 0x6c6f3131
  .section .lo2,"ao",%progbits,.lo1
  .long 0x6c6f3232
  .section .text.r,"axR",%progbits
rfun:
  $RET
  .section .rodata.str1.1,"aMS",%progbits,1
.LS0:
  .asciz "alpha-one"
.LS1:
  .asciz "beta-two"
.LS2:
  .asciz "gamma-three"
.LS3:
  .asciz "delta-four"
  .section .data.k,"aw",%progbits
  .globl kdata
kdata:
  .quad .rodata.str1.1+12
  .quad .LS2+3
  .section .data.r,"awR",%progbits
  .quad .LS3+2
  .section .nonalloc_info,"",%progbits
  .quad .LS0+4
EOF
    if [ $v = 0 ]; then printf 'int main(void){ return 0; }\n' > "$d/m.c"
    else printf 'void k(void); extern const char *kdata[];\nint main(void){ k(); return kdata[0][0]; }\n' > "$d/m.c"; fi
    (cd "$d" && $CC -O1 -c m.c) || echo "compile failed"
    if [ $v = 0 ]; then (cd "$d" && $CC -c u.s); else cp "$W/asm-$arch/v0/u.o" "$d/"; fi
  done
  pair asm-$arch "$CC" _start -static -Wl,--gc-sections m.o u.o
  fi

  # 5. An archive member extracted only through a reference from a section
  #    gc removes (v0); v1's main.o makes that section live.
  if want arch; then
  for v in 0 1; do
    d=$W/ar-$arch/v$v; mkdir -p "$d"
    printf 'int f1(void){ return 11; }\nint f1b(void){ return 12; }\n' > "$d/m1.c"
    printf 'int f2(void){ return 22; }\n' > "$d/m2.c"
    if [ $v = 0 ]; then printf 'int f1(void);\nint deadref(void){ return f1(); }\nint main(void){ return 0; }\n' > "$d/main.c"
    else printf 'int f1(void);\nint deadref(void){ return f1(); }\nint main(void){ return deadref(); }\n' > "$d/main.c"; fi
    (cd "$d" && $CC -O1 -g -ffunction-sections -c main.c) || echo "compile failed"
    if [ $v = 0 ]; then (cd "$d" && $CC -O1 -g -ffunction-sections -c m1.c m2.c && ar rcs lib.a m1.o m2.o)
    else cp "$W/ar-$arch/v0/lib.a" "$d/"; fi
  done
  pair ar-$arch "$CC" _start -static -Wl,--gc-sections main.o lib.a
  fi

  # 6. kiln/hod objects (.mason.meta), -O0 -g; k2.cpp edited.
  if want kiln; then
  for v in 0 1; do
    d=$W/kiln-$arch/v$v; mkdir -p "$d"
    printf 'struct S { int v; int get() const { return v; } };\ninline int shared_inl(int a) { return a + 3; }\nint k2(int);\nint unused_k1(int a) { return shared_inl(a); }\nint main(int c, char **) { S s{c}; return k2(s.get()) + shared_inl(c); }\n' > "$d/k1.cpp"
    printf 'inline int shared_inl(int a) { return a + 3; }\nint k2(int a) { return shared_inl(a) * %d; }\nint unused_k2(int a) { return a; }\n' $((v + 2)) > "$d/k2.cpp"
    (cd "$d" && KILN_BACKEND=hod KILN_HOD=$HOD KILN_HOD_STRICT=1 $KILN $KT -O0 -g -c k2.cpp -o k2.o) || echo "kiln compile failed"
    if [ $v = 0 ]; then (cd "$d" && KILN_BACKEND=hod KILN_HOD=$HOD KILN_HOD_STRICT=1 $KILN $KT -O0 -g -c k1.cpp -o k1.o) || echo "kiln compile failed"
    else cp "$W/kiln-$arch/v0/k1.o" "$d/"; fi
  done
  pair kiln-$arch "$CXX" _start -static -Wl,--gc-sections k1.o k2.o
  fi
done

# 7. i386 (free: cross gcc and libc are installed): SHT_REL, mergeable
#    strings through a section symbol with an implicit addend.
if want i386; then
  for v in 0 1; do
    d=$W/i386/v$v; mkdir -p "$d"
    cat > "$d/u.s" <<'EOF'
  .section .rodata.str1.1,"aMS",@progbits,1
.LS0:
  .asciz "alpha-one"
.LS1:
  .asciz "beta-two"
.LS2:
  .asciz "gamma-three"
  .section .data.k,"aw",@progbits
  .globl kdata
kdata:
  .long .rodata.str1.1+12
  .long .LS2+3
EOF
    if [ $v = 0 ]; then printf 'int main(void){ return 0; }\n' > "$d/m.c"
    else printf 'extern const char *kdata[];\nint main(void){ return kdata[0][0]; }\n' > "$d/m.c"; fi
    (cd "$d" && i686-linux-gnu-gcc -O1 -c m.c) || echo "compile failed"
    if [ $v = 0 ]; then (cd "$d" && i686-linux-gnu-gcc -c u.s); else cp "$W/i386/v0/u.o" "$d/"; fi
  done
  pair i386 i686-linux-gnu-gcc _start -static -Wl,--gc-sections m.o u.o
fi
echo "FINAL shapes"

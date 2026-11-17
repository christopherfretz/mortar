#!/bin/bash
# review/equiv.sh WORK: more candidates against V-R1 (link(r(A,B)) == link(A,B)),
# component A,B contiguous on the direct line. GNU ld's -r; finals by GNU ld and mortar.
set -u; ulimit -c 0
W=$(realpath -m "$1"); mkdir -p "$W"; cd "$W" || exit 1
M=/Volumes/AgentWork/mortar-helper/.build/target-wp-154/mortar-3ad9867a; mkdir -p mbin; ln -sf $M mbin/ld
run() { # NAME FLAGS BEFORE COMPONENT AFTER: direct = BEFORE COMPONENT AFTER, via = BEFORE r(COMPONENT) AFTER
  local n=$1 fl=$2 pre=$3 r=$4 rest=$5 out=""
  ld.bfd -r -o r.$n.o $r
  for L in bfd mortar; do B=""; [ $L = mortar ] && B="-B$W/mbin/"
    for how in direct via; do objs="$pre $r"; [ $how = via ] && objs="$pre r.$n.o"
      if gcc $B $fl $objs $rest -o $n.$L.$how > $n.$L.$how.err 2>&1; then
        case $fl in *-shared*) res="dynsym: $(readelf --dyn-syms -W $n.$L.$how | awk '$7!="UND" && $5!="LOCAL" && $8 ~ /^(lib_fn|v|api)$/ {print $8}' | sort | tr '\n' ' ')";; *) res=$(./$n.$L.$how 2>&1 | tr '\n' ' ');; esac
      else res="LINK FAILS: $(grep -v collect2 $n.$L.$how.err | grep -m1 -E 'error|undefined|multiple'  | cut -c1-90)"; fi; out="$out | $L $how: $res"; done; done
  echo "$n$out"; }
# 1. --wrap: B calls foo, A defines it. Direct: B's reference is undefined, so wrapped.
printf 'int foo(void){return 1;}\n' > wa.c; printf 'int foo(void);\nint callfoo(void){return foo();}\n' > wb.c
printf '#include <stdio.h>\nint callfoo(void); int __real_foo(void);\nint __wrap_foo(void){return 100+__real_foo();}\nint main(void){printf("callfoo=%%d\\n", callfoo());return 0;}\n' > wm.c
for f in wa wb wm; do gcc -O1 -fno-inline -c $f.c -o $f.o; done
run wrap "-Wl,--wrap=foo" "" "wa.o wb.o" "wm.o"
# 2. --exclude-libs ALL in a -shared final link: an archive member on the -r line.
printf 'int lib_fn(void){return 7;}\n' > l.c; printf 'int lib_fn(void);\nint api(void){return lib_fn();}\n' > la.c
gcc -O1 -fPIC -c l.c -o l.o; gcc -O1 -fPIC -c la.c -o la.o; rm -f libl.a; ar rcs libl.a l.o
run exclibs "-shared -nostartfiles -Wl,--exclude-libs,ALL" "" "la.o libl.a" ""
# 3. COMDAT keeper, copies with different bytes (an ODR violation), component contiguous.
printf 'inline int v(void){return V;}\nint k%s(void){return v();}\n' 1 > k1.c; printf 'inline int v(void){return V;}\nint k%s(void){return v();}\n' 2 > k2.c
g++ -x c++ -O0 -DV=1 -c k1.c -o k1.o; g++ -x c++ -O0 -DV=2 -c k2.c -o k2.o; g++ -x c++ -O0 -DV=3 -c k1.c -o k3.o -Dk1=k3
printf '#include <stdio.h>\nint k1(void); int k2(void);\nint main(void){printf("v=%%d,%%d\\n", k1(), k2());return 0;}\n' > km.cc; g++ -O0 -c km.cc -o km.o
run keeper-in "" "km.o" "k1.o k2.o" "-lstdc++"
run keeper-out-first "" "km.o k3.o" "k1.o k2.o" "-lstdc++"
# 4. .init_array order: outside unprioritized c, component a (none), b (prio 200) and d (none).
for x in a c d; do printf '#include <stdio.h>\n__attribute__((constructor)) static void i(void){printf("%s ");}\n' $x > i$x.c; done
printf '#include <stdio.h>\n__attribute__((constructor(200))) static void i(void){printf("b200 ");}\n' > ib.c
printf 'int main(void){return 0;}\n' > im.c; for x in a b c d m; do gcc -O1 -c i$x.c -o i$x.o; done
run initorder "" "im.o ic.o" "ia.o ib.o id.o" ""
# 5. common alignment and size: component commons (4 align 4; 8 align 32), outside common 16 align 4.
printf 'int cx;\n' > c1.c; printf 'long cx __attribute__((aligned(32)));\nlong pad1;\n' > c2.c; printf 'int cx[4];\n' > c3.c
printf '#include <stdio.h>\nextern char cx[];\nint main(void){printf("cx%%%%32=%%d\\n", (int)((unsigned long)cx %% 32));return 0;}\n' > cm.c
for x in 1 2 3; do gcc -O1 -fcommon -c c$x.c -o c$x.o; done; gcc -O1 -c cm.c -o cm.o
run common "" "cm.o" "c1.o c2.o" "c3.o"
# 6. visibility merging: component references v hidden, the definition outside; -shared.
printf 'extern int v __attribute__((visibility("hidden")));\nint api(void){return v;}\n' > v1.c; printf 'int v = 3;\n' > v2.c
gcc -O1 -fPIC -c v1.c -o v1.o; gcc -O1 -fPIC -c v2.c -o v2.o
run visibility "-shared -nostartfiles" "" "v1.o" "v2.o"
echo "equiv done"

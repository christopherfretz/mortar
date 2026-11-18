#!/bin/bash
# ar-flips.sh: archive member flips of ordinary one-line edits in a static
# program (glibc libc.a, libstdc++.a, libm.a, libgcc*.a), from two full links
# (GNU ld -t lists every extracted member; mortar's map as a second opinion).
# Prints per edit: members extracted before/after, flipped in/out, and the
# defined globals of the flipped members (KEY_REFS/MENTIONS rows they carry).
ulimit -c 0
D=/Volumes/AgentWork/mortar-helper/.build/review-wp207/ar
cd $D || exit 1
cat > c0.c <<'X'
#include <stdio.h>
int main(void){ puts("hi"); return 0; }
X
cat > c1.c <<'X'
#include <stdio.h>
int main(int c, char**v){ printf("%f\n", c*1.5); return 0; }
X
cat > c2.c <<'X'
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
int main(int c, char**v){ char b[64]; time_t t=time(0); strftime(b,64,"%c",localtime(&t)); puts(b); return 0; }
X
cat > x0.cc <<'X'
#include <cstdio>
int main(){ std::puts("hi"); return 0; }
X
cat > x1.cc <<'X'
#include <iostream>
int main(){ std::cout << "hi" << std::endl; return 0; }
X
cat > x2.cc <<'X'
#include <thread>
#include <cstdio>
int main(){ std::thread t([]{ std::puts("hi"); }); t.join(); return 0; }
X
cat > x3.cc <<'X'
#include <string>
#include <cstdio>
int main(int c, char**){ std::string s = std::to_string(c*2.5); std::puts(s.c_str()); return 0; }
X
cat > x4.cc <<'X'
#include <regex>
#include <cstdio>
int main(int c, char**v){ std::regex r("a+b"); std::puts(std::regex_match(v[0], r) ? "y" : "n"); return 0; }
X
for f in c0 c1 c2; do gcc -O1 -c $f.c -o $f.o; gcc -static $f.o -o $f.g -Wl,-t,-t > $f.t 2>&1; done
for f in x0 x1 x2 x3 x4; do g++ -O1 -c $f.cc -o $f.o; g++ -static $f.o -o $f.g -Wl,-t,-t > $f.t 2>&1; done
mem() { sed -n 's/^(\(.*\.a\))\(.*\)$/\1(\2)/p' $1 | sort -u; }
for p in "c0 c1" "c0 c2" "x0 x1" "x0 x2" "x0 x3" "x0 x4" "x1 x2"; do
  set -- $p
  mem $1.t > $1.m; mem $2.t > $2.m
  in=$(comm -13 $1.m $2.m | wc -l); out=$(comm -23 $1.m $2.m | wc -l)
  # defined globals of the flipped members
  rows=0
  for m in $(comm -3 $1.m $2.m | tr -d '\t'); do
    a=${m%%(*}; o=${m#*(}; o=${o%)}
    n=$(nm -g --defined-only "$a" 2>/dev/null | awk -v o="$o:" '$0==o{f=1;next} /^$/{f=0} /:$/{f=0} f&&NF==3{c++} END{print c+0}')
    rows=$((rows+n))
  done
  echo "$1 -> $2: members $(wc -l < $1.m) -> $(wc -l < $2.m), in $in, out $out, defined globals of flipped members $rows; run: $(./$2.g x >/dev/null 2>&1; echo $?)"
done

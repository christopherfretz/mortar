#!/bin/bash
# gnu-warning.sh: a warning whose source is an UNCHANGED archive member
# (glibc's .gnu.warning.mktemp in libc.a) and whose existence is decided by
# a CHANGED file's reference (IN-75a's "subject"). Full links in incremental
# mode, v1 calls mktemp, v2 does not, v3 calls it again.
ulimit -c 0
R=/Volumes/AgentWork/mortar-helper/.build/review-in72
cd /Volumes/AgentWork/mortar-helper/.build/review-wp207/gw || exit 1
printf '#include <stdlib.h>\n#include <stdio.h>\nint main(void){ char t[]="/tmp/xXXXXXX"; puts(mktemp(t)?"a":"b"); return 0; }\n' > v1.c
printf '#include <stdio.h>\nint main(void){ puts("a"); return 0; }\n' > v2.c
for v in v1 v2; do gcc -O1 -c $v.c -o $v.o; done
for v in v1 v2 v1; do cp $v.o c.o; rm -f out out.mortar-state
  gcc -B$R/bin/ -static -o out c.o -Wl,--incremental=full > $v.log 2>&1; echo "mortar $v rc=$? stderr: $(grep -v '^incremental:' $v.log | head -3 | tr '\n' '|')"
  gcc -fuse-ld=bfd -static -o outg c.o > g$v.log 2>&1; echo "  GNU ld $v: $(head -2 g$v.log | tr '\n' '|')"
done

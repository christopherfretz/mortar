#!/bin/bash
# review/zkw.sh WORK: every -z keyword GNU ld 2.44 lists in --help, at -r, per
# target (aarch64 native, x86_64 and i686 cross): rc, and whether the output is
# byte-identical to the same -r without it. Inputs: a C object with its notes
# (gcc: .note.GNU-stack, and on x86 .note.gnu.property when -fcf-protection).
set -u; ulimit -c 0
H=$(cd "$(dirname "$0")" && pwd); W=$(realpath -m "$1"); mkdir -p "$W"; cd "$W" || exit 1
printf 'int a(int x){return x+1;}\nint g_data=5;\n' > a.c; printf 'int b(int x){return x*2;}\n' > b.c
expand() { # one keyword pattern of --help -> concrete forms
  case $1 in
    *=SIZE) echo "${1%=SIZE}=0x10000";; start-stop-visibility=V) echo "start-stop-visibility=hidden";;
    *'[=none|warning|error]') echo "${1%%\[*} ${1%%\[*}=error";;
    gcs=*) echo "gcs=always gcs=never";; gcs-report-dynamic=*) echo "gcs-report-dynamic=error";;
    cet-report=*) echo "cet-report=error";; isa-level-report=*) echo "isa-level-report=all";;
    lam-*-report=*|lam-report=*) echo "${1%%=*}=error";; call-nop=*) echo "call-nop=suffix-nop";;
    x86-64-*) echo "x86-64-baseline x86-64-v3";; *) echo "$1";; esac; }
for T in aarch64 x86_64 i686; do
  case $T in aarch64) CC=gcc LD=ld.bfd;; x86_64) CC=x86_64-linux-gnu-gcc LD=x86_64-linux-gnu-ld;; i686) CC=i686-linux-gnu-gcc LD=i686-linux-gnu-ld;; esac
  $CC -O1 -c a.c -o a.$T.o; $CC -O1 -c b.c -o b.$T.o; $LD -r -o base.$T.o a.$T.o b.$T.o
  for k in $($LD --help | grep -E '^\s+-z ' | sed -E 's/^\s+-z ([^ ]+).*/\1/' | sort -u); do for f in $(expand "$k"); do
    o=z.$T.$(echo "$f" | tr -c 'A-Za-z0-9\n' _).o
    $LD -r -z "$f" -o "$o" a.$T.o b.$T.o > err 2>&1; rc=$?
    if [ $rc != 0 ]; then r="error: $(head -1 err | sed 's/^.*ld: //')"; elif cmp -s "$o" base.$T.o; then r="no-op (identical)"; else r="CHANGES: $(python3 -I "$H"/secdiff.py base.$T.o "$o")"; fi
    [ -s err ] && [ $rc = 0 ] && r="$r; warns: $(head -1 err | sed 's/^.*ld: //' | cut -c1-80)"
    printf '%-7s -z %-32s %s\n' $T "$f" "$r"; done; done; done
echo "zkw done"

#!/bin/sh
# A gdb and an lldb session on a linked clang (WP-081, "Correct first"):
# a breakpoint in a template instantiation of clang's template
# instantiator, the backtrace, a local, an argument, a type. Two clangs
# linked from the same inputs must give the same normalized transcripts
# (addresses written `0x?`, the binary's path written `CLANG`). The
# container may not disable address-space randomisation, so values derived
# from addresses (a hash of a pointer, a source location of a heap buffer)
# can differ from run to run of the same binary; compare against a second
# run of the control. Each
# session's wall time and peak RSS (GNU time) are in OUT/*.time.
#
# usage: debugger.sh CLANG OUT
set -u
CLANG=$1
OUT=$2
HERE=$(cd "$(dirname "$0")/programs" && pwd)
mkdir -p "$OUT"
BP='clang::TreeTransform<(anonymous namespace)::TemplateInstantiator>::TransformType'
cat > "$OUT/gdb.cmd" <<EOF
set pagination off
set width 0
set print frame-arguments scalars
break $BP
run
bt 6
info args
frame 1
info locals
ptype clang::QualType
print sizeof(clang::Sema)
kill
quit
EOF
cat > "$OUT/lldb.cmd" <<EOF
settings set target.disable-aslr false
breakpoint set -n "$BP"
run
thread backtrace -c 6
frame variable
up
frame variable
type lookup clang::QualType
expression sizeof(clang::Sema)
kill
quit
EOF
ARGS="-cc1 -fsyntax-only -std=c++17 -x c++ $HERE/templates.cc -I/usr/include/c++/14 -I/usr/include/aarch64-linux-gnu/c++/14 -I/usr/include/aarch64-linux-gnu -I/usr/include"
norm() {
  sed -e "s|$CLANG|CLANG|g" -e "s|$OUT|OUT|g" -e 's/0x[0-9a-f]\{4,\}/0x?/g' -e 's/process [0-9]*/process N/g' \
      -e 's/Process [0-9]*/Process N/g' -e 's/\[LWP [0-9]*\]/[LWP N]/g' -e 's/ pid [0-9]*/ pid N/g' \
      -e 's/thread #1, name = .[^,]*,/thread #1,/'
}
/usr/bin/time -f "gdb wall %e s, peak %M KiB" -o "$OUT/gdb.time" timeout 600 gdb -q -batch -x "$OUT/gdb.cmd" --args "$CLANG" $ARGS > "$OUT/gdb.raw" 2>&1
norm < "$OUT/gdb.raw" > "$OUT/gdb.txt"
/usr/bin/time -f "lldb wall %e s, peak %M KiB" -o "$OUT/lldb.time" timeout 600 lldb -b -s "$OUT/lldb.cmd" -- "$CLANG" $ARGS > "$OUT/lldb.raw" 2>&1
norm < "$OUT/lldb.raw" > "$OUT/lldb.txt"
wc -l "$OUT/gdb.txt" "$OUT/lldb.txt"
cat "$OUT/gdb.time" "$OUT/lldb.time"

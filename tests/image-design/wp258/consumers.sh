#!/bin/bash
# WP-258 item 4: what the consumers make of each linker's output from links.sh.
#   consumers.sh LINKS_OUT     LINKS_OUT/consumers.tsv, LINKS_OUT/<cfg>/<linker>.{gdb,lldb,sym}
# Static sessions (no program runs; both targets): gdb 16 (gdb-multiarch for
# x86-64) `break wp258::twice` (one location or two), `info line`, `info scope`,
# `info line *KEPT`, complaints on; lldb 19 `breakpoint set -n`, `image lookup -v -n`,
# `image lookup -v -a KEPT`; llvm-symbolizer-22 on KEPT (wp258::twice's address
# in the output's symbol table, the kept copy) and KEPT+4. On AArch64 (the host)
# gdb also runs the program: `break wp258::twice`, run, bt, continue, bt.
# Last line: CONSUMERS DONE.
ulimit -c 0
OUT="$(cd "$1" && pwd)"
: > "$OUT/consumers.tsv"
TW=_ZN5wp2585twiceEi
for cfg in a64-gcc-dw4-ne a64-gcc-dw5-eq a64-gcc-dw5-ne a64-clang-dw4-ne a64-clang-dw5-ne \
           x64-gcc-dw4-eq x64-gcc-dw5-eq x64-clang-dw4-eq x64-clang-dw5-eq \
           a64-gcc-dw5-o2 a64-clang-dw5-o2 x64-gcc-dw4-o2 x64-gcc-dw5-o2 x64-clang-dw5-o2; do
 W="$OUT/$cfg"
 case $cfg in a64*) G=gdb;; x64*) G=gdb-multiarch;; esac
 for x in mortar gnu lld19 lld22 mold ones-rng; do
  B="$W/$x"; [ -f "$B" ] || continue
  K=$(nm "$B" | awk -v s=$TW '$3==s{print "0x"$1}')
  K4=$(printf '0x%x' $((K + 4)))
  timeout 120 $G -nx -q -batch -ex 'set pagination off' -ex 'set debuginfod enabled off' \
    -ex 'set complaints 100' -ex "file $B" -ex 'break wp258::twice' -ex 'info line wp258::twice' \
    -ex 'info scope wp258::twice' -ex "info line *$K" -ex "info symbol $K" > "$B.gdb" 2>&1
  if [ "${cfg%%-*}" = a64 ]; then
    timeout 120 gdb -nx -q -batch -ex 'set pagination off' -ex 'set debuginfod enabled off' \
      -ex 'break wp258::twice' -ex run -ex 'bt 2' -ex continue -ex 'bt 2' -ex kill "$B" > "$B.run" 2>&1
  fi
  timeout 120 lldb-19 --no-lldbinit -b -o 'breakpoint set -n wp258::twice' \
    -o 'image lookup -v -n wp258::twice' -o "image lookup -v -a $K" "$B" > "$B.lldb" 2>&1
  { timeout 120 llvm-symbolizer-22 --obj="$B" "$K"; timeout 120 llvm-symbolizer-22 --obj="$B" "$K4"; } > "$B.sym" 2>&1
  gl=$(grep -m1 '^Breakpoint 1 at' "$B.gdb" | sed 's/^Breakpoint 1 at //; s/0x[0-9a-f]*/A/g')
  gc=$(grep -c 'start address of zero\|inverted range\|Invalid .debug_rnglists\|not in symtab' "$B.gdb")
  il=$(grep -m1 '^Line [0-9]* of\|^No line number' "$B.gdb" | sed 's/0x[0-9a-f]*/A/g; s/<[^>]*>/<f>/g')
  ik=$(grep '^Line [0-9]* of\|^No line number' "$B.gdb" | sed -n 2p | sed 's/0x[0-9a-f]*/A/g; s/<[^>]*>/<f>/g')
  ls=$(grep -m1 '^Breakpoint 1:' "$B.lldb" | sed 's/^Breakpoint 1: //; s/0x[0-9a-f]*/A/g')
  lf=$(grep -c '^ *Function: ' "$B.lldb")
  sy=$(tr '\n' ' ' < "$B.sym" | sed 's/  */ /g')
  hits=; if [ -f "$B.run" ]; then hits="run: $(grep -c '^Breakpoint 1, ' "$B.run") stops, callers $(grep -o '^#1 .* in [a-z_]*' "$B.run" | awk '{print $NF}' | tr '\n' ,)"; fi
  printf '%s\t%s\tgdb break: %s\tgdb complaints=%s\tgdb info line: %s\tgdb info line *kept: %s\tlldb break: %s\tlldb Function lines=%s\tsymbolizer: %s\t%s\n' \
    "$cfg" "$x" "$gl" "$gc" "$il" "$ik" "$ls" "$lf" "$sy" "$hits" >> "$OUT/consumers.tsv"
 done
done
echo "CONSUMERS DONE"

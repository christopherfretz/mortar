#!/bin/bash
# usedset.sh WORK: re-links every shlib experiment (WORK from `run.sh WORK all`, already run)
# with GNU ld behind a recording wrapper (argv, -t -t trace, rc, DT_NEEDED, .dynsym), adds
# the default hello lines (gcc/g++/gfortran, aarch64 native and x86-64 cross) and a few
# extra experiments (X*), then runs usedset.py: the model of dyn.md §5's sets vs GNU ld.
# Output: WORK/usedset.txt.  About 2 minutes.
# Ported for WP-043 (crates/mortar_resolve/tests/shlib_oracle.rs) from
# .build/review-m2/shlib/usedset.sh: NORUN=1 skips running the outputs and
# NOMODEL=1 skips usedset.py (the oracle replays the records with mortar).
set -u
W=$(cd "${1:?usage: usedset.sh WORK}" && pwd)
HERE=$(cd "$(dirname "$0")" && pwd)
LINKERS=trace

needed() { readelf -dW "$1" 2>/dev/null | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p' | paste -sd, -; }
LOG=/dev/null; log() { :; }; sec() { :; }; so() { :; }; obj() { :; }

mkwrap() { # ARCH REAL-LD
  mkdir -p "$W/bin/$1-trace"
  cat > "$W/bin/$1-trace/ld" <<EOF
#!/bin/bash
o=""; prev=""; for a in "\$@"; do [ "\$prev" = -o ] && o=\$a; prev=\$a; done
id=\$(basename "\$o"); id=\${id%.trace}; M=\$MODEL_DIR/\$id
printf '%s\n' "\$PWD" > "\$M.args"; printf '%s\n' "\$@" >> "\$M.args"
env | grep -E '^(LD_LIBRARY_PATH|LD_RUN_PATH)=' | paste -sd' ' - > "\$M.env"
$2 "\$@" -t -t > "\$M.trace" 2> "\$M.err"; rc=\$?; echo \$rc > "\$M.rc"
[ \$rc = 0 ] && { readelf -dW "\$o" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p' | paste -sd, - > "\$M.needed"; readelf --dyn-syms -W "\$o" > "\$M.dynsyms.all"; readelf -rW "\$o" | grep _COPY > "\$M.copyrel"; }
exit \$rc
EOF
  chmod +x "$W/bin/$1-trace/ld"
}
mkwrap aarch64 /usr/bin/ld.bfd
mkwrap x86_64 /usr/bin/x86_64-linux-gnu-ld.bfd

runit() {
  [ "${NORUN:-0}" = 1 ] && { echo "run skipped"; return; }
  if [ "$ARCH" = x86_64 ]; then timeout 20 qemu-x86_64 -L /usr/x86_64-linux-gnu -E LD_LIBRARY_PATH="$LLP" "$1" 2>&1
  else (cd "$X" && timeout 20 env LD_LIBRARY_PATH="$LLP" "$1" 2>&1); fi
  echo "rc=$?"
}
lk() { # ID LINKER RUN -- args ; SYM in $SYM
  local id=$1; shift 4
  local out=$X/out/$id.trace
  ( cd "$X" && $CC -B"$W/bin/$ARCH-trace/" -o "$out" "$@" ) > /dev/null 2>&1
  echo "${SYM:-}" > "$MODEL_DIR/$id.sym"
  [ -n "${SYM:-}" ] && [ -f "$MODEL_DIR/$id.dynsyms.all" ] && awk -v n="$SYM" '{s=$8; sub(/@.*/,"",s); if (s==n) print}' "$MODEL_DIR/$id.dynsyms.all" > "$MODEL_DIR/$id.dynsym"
  [ -f "$out" ] && runit "$out" | tail -2 | paste -sd' ' - > "$MODEL_DIR/$id.run"
}
all() { local id=$1; shift 3; SYM= lk "$id" trace 1 -- "$@"; }
allsym() { local id=$1 s=$2; shift 4; SYM=$s lk "$id" trace 1 -- "$@"; }

source "$HERE/experiments.sh"

extras() {
  local S=$X/xtra; mkdir -p $S
  echo 'int leaf_fn(void); int mid2_fn(void){return 100+leaf_fn();}' > $S/xmid2.c
  echo 'int mid2_fn(void); int top2_fn(void){return 1000+mid2_fn();}' > $S/xtop2.c
  printf '#include <stdio.h>\nint top2_fn(void); int main(void){printf("%%d\\n",top2_fn());return 0;}\n' > $S/m_top2.c
  ( cd $X && $CC -B$W/bin/$ARCH-bfd/ -fPIC -shared -Wl,-soname,libmid2.so -o xtra/libmid2.so xtra/xmid2.c &&
    $CC -B$W/bin/$ARCH-bfd/ -fPIC -shared -Wl,-soname,libtop2.so -o xtra/libtop2.so xtra/xtop2.c -Lxtra -lmid2 &&
    $CC -O1 -c -o m_top2.o xtra/m_top2.c )
  # X1: a used (as-needed, reached from a needed object) command-line object with an unsatisfiable reference
  all X1-used-AN-undef 1 -- $AN m_umid.o middir/libumid.so leafdir/libuleaf.so
  # X2: under-linked libtopu references mid_fn; libmid is reached from needed libtop (LP/L: needed; G: used)
  all X2-underlinked-but-reached 1 -- $NA m_top.o topdir/libtopu.so topdir/libtop.so $AN middir/libmid.so leafdir/libleaf.so
  all X2b-same-AN-lib-first 1 -- $AN middir/libmid.so leafdir/libleaf.so $NA m_top.o topdir/libtopu.so topdir/libtop.so
  # X3: a USED object (libmid2, reached from needed libtop2) under-links leaf_fn: U -> N interplay
  all X3-used-underlinks 1 -- $AN m_top2.o xtra/libtop2.so xtra/libmid2.so leafdir/libleaf.so
  all X3b-used-underlinks-NA-mid 1 -- $AN m_top2.o xtra/libtop2.so $NA xtra/libmid2.so $AN leafdir/libleaf.so
  # X3c: GNU ld forced to write the output the N-u-U variant predicts for X3 (libleaf needed, libmid2 only used): does it run?
  all X3c-X3-output-forced 1 -- $AN m_top2.o xtra/libtop2.so xtra/libmid2.so $NA leafdir/libleaf.so
}

hellos() {
  local H=$X/hello; mkdir -p $H
  printf '#include <stdio.h>\nint main(void){puts("hi");return 0;}\n' > $H/h.c
  printf '#include <iostream>\n#include <string>\nint main(){std::string s="hi"; try { throw 1; } catch (int) {} std::cout<<s<<std::endl;}\n' > $H/h.cc
  printf 'program h\n  print *, "hi"\nend program h\n' > $H/h.f90
  local P=; [ $ARCH = x86_64 ] && P=x86_64-linux-gnu-
  ( cd $H && ${P}gcc -c h.c -o hc.o && ${P}g++ -c h.cc -o hcc.o && ${P}gfortran -c h.f90 -o hf.o )
  local d
  for d in "gcc hc.o" "g++ hcc.o" "gfortran hf.o" "gcc-nopie hc.o" "g++-nopie hcc.o"; do
    set -- $d; local drv=$1 o=$2 extra=
    case $drv in *-nopie) drv=${drv%-nopie}; extra=-no-pie;; esac
    ( cd $X && ${P}$drv -B"$W/bin/$ARCH-trace/" $extra -o "$X/out/hello-$1.trace" hello/$o ) > /dev/null 2>&1
    echo > "$MODEL_DIR/hello-$1.sym"
    [ -f "$X/out/hello-$1.trace" ] && runit "$X/out/hello-$1.trace" | tail -2 | paste -sd' ' - > "$MODEL_DIR/hello-$1.run"
  done
}

for ARCH in aarch64 x86_64; do
  X=$W/$ARCH; [ -d $X ] || { echo "no $X: run run.sh first"; exit 1; }
  if [ $ARCH = aarch64 ]; then CC=gcc; else CC=x86_64-linux-gnu-gcc; fi
  LLP=$X/d:$X/leafdir:$X/middir:$X/topdir:$X/q:$X/xtra
  MODEL_DIR=$X/model; export MODEL_DIR; mkdir -p $MODEL_DIR
  cd $X
  experiments
  extras
  hellos
done
[ "${NOMODEL:-0}" = 1 ] && exit 0
python3 -I "$HERE/usedset.py" "$W" > "$W/usedset.txt" 2>&1
tail -5 "$W/usedset.txt"; echo "full report: $W/usedset.txt"

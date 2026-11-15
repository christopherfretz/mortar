#!/bin/bash
# shlib: input-side semantics of shared objects in a dynamic executable link.
# Usage: run.sh WORK [aarch64|x86_64|all]   (WORK must not exist or be empty)
# Logs: WORK/<arch>/log.txt  (one block per experiment, per linker)
# Ported for WP-043 (crates/mortar_resolve/tests/shlib_oracle.rs) from
# .build/review-m2/shlib/run.sh: LINKERS and NORUN may be set (the oracle
# builds with LINKERS= (empty) NORUN=1: the inputs only; usedset.sh records).
set -u
W=${1:?usage: run.sh WORK [arch]}
WHICH=${2:-all}
mkdir -p "$W"; W=$(cd "$W" && pwd)
HERE=$(cd "$(dirname "$0")" && pwd)
LINKERS=${LINKERS-"bfd lld22 lld19 mold"}
NORUN=${NORUN:-0}

source "$HERE/tools.sh"

mkdir -p "$W/bin"
mk_bin() { mkdir -p "$W/bin/$1"; ln -sf "$2" "$W/bin/$1/ld"; }
mk_bin aarch64-bfd "$(bfd_of aarch64)"
mk_bin x86_64-bfd "$(bfd_of x86_64)"
for a in aarch64 x86_64; do
  mk_bin $a-lld22 /usr/bin/ld.lld-22; mk_bin $a-lld19 /usr/bin/ld.lld-19; mk_bin $a-mold /usr/bin/mold
done
{
  for t in "$(pfx aarch64)gcc" "$(pfx x86_64)gcc" "$(pfx aarch64)ld.bfd" "$(pfx x86_64)ld.bfd" ld.lld-22 ld.lld-19 mold readelf; do
    echo "$t: $($t --version 2>&1 | head -1)"; done
} > "$W/versions.txt"

log() { echo "$*" >> "$LOG"; }
sec() { echo >> "$LOG"; echo "######## $* ########" >> "$LOG"; }

# needed OUT -> comma list of DT_NEEDED
needed() { readelf -dW "$1" 2>/dev/null | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p' | paste -sd, -; }
# dsym OUT NAME -> .dynsym rows whose name (before @) is NAME
dsym() { readelf --dyn-syms -W "$1" 2>/dev/null | awk -v n="$2" '{s=$8; sub(/@.*/,"",s); if (s==n) print "    dynsym:",$4,$5,$6,$7,$8}'; }
runit() { # OUT
  [ "$NORUN" = 1 ] && { echo "run skipped"; return; }
  local o
  if [ -n "$(pfx $ARCH)" ]; then
    o=$(timeout 20 qemu-$ARCH -L /usr/$(triple $ARCH) -E LD_LIBRARY_PATH="$LLP" "$1" 2>&1); else
    o=$(cd "$X" && timeout 20 env LD_LIBRARY_PATH="$LLP" "$1" 2>&1); fi
  local rc=$?
  echo "run rc=$rc out=$(echo "$o" | head -3 | paste -sd'|' -)"
}
# lk ID LINKER [RUN=0|1] -- gcc args...   (output always a fresh path)
lk() {
  local id=$1 l=$2 r=$3; shift 4
  local out=$X/out/$id.$l
  rm -f "$out"
  ( cd "$X" && $CC -B"$W/bin/$ARCH-$l/" -o "$out" "$@" ) > "$out.err" 2>&1
  local rc=$?
  local line="[$ARCH $id $l] link rc=$rc"
  if [ $rc = 0 ]; then line="$line NEEDED=$(needed "$out")"; [ "$r" = 1 ] && line="$line $(runit "$out")"; fi
  log "$line"
  sed 's/^/    msg: /' "$out.err" | sed "s#$X/##g" | head -12 >> "$LOG"
}
# all ID RUN -- args : every linker
all() { local id=$1 r=$2; shift 3; for l in $LINKERS; do lk "$id" "$l" "$r" -- "$@"; done; }
# allsym ID SYM RUN -- args : every linker + dynsym rows of SYM
allsym() { local id=$1 s=$2 r=$3; shift 4; for l in $LINKERS; do lk "$id" "$l" "$r" -- "$@"; [ -f "$X/out/$id.$l" ] && dsym "$X/out/$id.$l" "$s" >> "$LOG"; done; }

so() { # so OUTPATH SONAME|- SRC [extra args]
  local out=$1 sn=$2 src=$3; shift 3
  local snarg=""; [ "$sn" != - ] && snarg="-Wl,-soname,$sn"
  mkdir -p "$(dirname "$out")"
  ( cd "$X" && $CC -B"$W/bin/$ARCH-bfd/" -fPIC -shared -O1 $snarg -o "$out" "$src" "$@" ) || echo "BUILD FAILED $out" >> "$LOG"
}
obj() { ( cd "$X" && $CC -O1 -c -o "$1" "${@:2}" ) || echo "BUILD FAILED $1" >> "$LOG"; }

write_sources() {
  local S=$X/src; mkdir -p "$S"; cd "$S"
  echo 'int a_fn(void){return 1;} int a_var=11;' > liba.c
  echo 'int a_fn(void){return 2;}' > liba2.c
  echo 'int b_fn(void){return 20;}' > libb.c
  echo 'int ns_fn(void){return 30;}' > libns.c
  echo 'int leaf_fn(void){return 3;}' > libleaf.c
  echo 'int leaf_fn(void); int mid_fn(void){return 100+leaf_fn();}' > libmid.c
  echo 'int mid_fn(void); int top_fn(void){return 1000+mid_fn();}' > libtop.c
  echo 'int hook(void); int hmid_fn(void){return hook();}' > libhmid.c
  echo 'int hmid_fn(void); int htop_fn(void){return hmid_fn();}' > libhtop.c
  echo 'int hook(void); int callhook(void){return hook();}' > libhook.c
  echo 'int nobody(void); int x_fn(void){return 5;} int x_call(void){return nobody();}' > libx.c
  echo 'int nobody_w(void) __attribute__((weak)); int xw_fn(void){return nobody_w?nobody_w():6;}' > libxw.c
  echo 'int nobody(void); int uleaf_fn(void){return 4;} int uleaf_call(void){return nobody();}' > libuleaf.c
  echo 'int uleaf_fn(void); int umid_fn(void){return uleaf_fn();}' > libumid.c
  cat > libver.c <<'EOF'
int foo_v1(void){return 1;}
int foo_v2(void){return 2;}
__asm__(".symver foo_v1,foo@VERS_1");
__asm__(".symver foo_v2,foo@@VERS_2");
int hid_v1(void){return 7;}
__asm__(".symver hid_v1,hid@VERS_1");
int v1only(void){return 8;}
int foo(void);
int call_foo(void){return foo();}
EOF
  printf 'VERS_1 { global: v1only; foo; hid; local: *; };\nVERS_2 { global: call_foo; foo; } VERS_1;\n' > libver.map
  printf 'VERS_1 { global: v1only; local: *; };\nVERS_2 { global: call_foo; } VERS_1;\n' > libver_loc.map
  echo '__attribute__((weak)) int wd(void){return 7;}' > libweak.c
  echo 'int cv = 5; int get_cv(void){return cv;}' > libcom.c
  cat > libprot.c <<'EOF'
__attribute__((visibility("protected"))) int pv = 9;
__attribute__((visibility("protected"))) int pf(void){return 9;}
int get_pv(void){return pv;}
void *addr_pf(void){return (void*)pf;}
EOF
  echo 'int vf(void){return 12;}' > libvis.c
  cat > libuniq.c <<'EOF'
__asm__(".globl uq\n.section .data.uq,\"aw\"\n.p2align 2\n.type uq, %gnu_unique_object\n.size uq,4\nuq: .long 5\n.text");
int get_uq(void){extern int uq; return uq;}
EOF
  echo 'static int impl(void){return 33;} static void *res(void){return (void*)impl;} int ifn(void) __attribute__((ifunc("res"))); void *addr_ifn(void){return (void*)ifn;}' > libifunc.c
  echo 'int dup_fn(void){return 1;}' > libdup1.c
  echo 'int dup_fn(void){return 2;} int dup_extra(void){return 3;}' > libdup2.c
  echo 'int q_fn(void){return 1;}' > libq_so.c
  echo 'int q_fn(void){return 2;}' > libq_a.c
  echo 'int q_fn(void){return 3;}' > libq_a3.c
  echo 'int a_fn(void){return 50;}' > arc_a.c
  echo 'int a_fn(void){return 1;} int call_a(void){return a_fn();}' > libref.c

  P='#include <stdio.h>'
  printf '%s\nint a_fn(void); int main(void){printf("%%d\\n",a_fn());return 0;}\n' "$P" > m_a.c
  printf '%s\nint a_fn(void) __attribute__((weak)); int main(void){printf("%%d\\n",a_fn?a_fn():0);return 0;}\n' "$P" > m_wa.c
  printf '%s\nint a_fn(void){return 100;} int main(void){printf("%%d\\n",a_fn());return 0;}\n' "$P" > m_def.c
  printf '%s\nint a_fn(void); int unused_fn(void){return a_fn();} int main(void){puts("0");return 0;}\n' "$P" > m_gc.c
  printf '%s\nint main(void){puts("hi");return 0;}\n' "$P" > m_hello.c
  printf '%s\nint foo(void); int main(void){printf("%%d\\n",foo());return 0;}\n' "$P" > m_v.c
  printf '%s\nint foo_old(void); __asm__(".symver foo_old,foo@VERS_1"); int main(void){printf("%%d\\n",foo_old());return 0;}\n' "$P" > m_v1.c
  printf '%s\nint foo_x(void); __asm__(".symver foo_x,foo@VERS_3"); int main(void){printf("%%d\\n",foo_x());return 0;}\n' "$P" > m_v3.c
  printf '%s\nint hid(void); int main(void){printf("%%d\\n",hid());return 0;}\n' "$P" > m_hid.c
  printf '%s\nint foo(void){return 99;} int call_foo(void); int main(void){printf("%%d %%d\\n",foo(),call_foo());return 0;}\n' "$P" > m_vdef.c
  printf '%s\nint top_fn(void); int main(void){printf("%%d\\n",top_fn());return 0;}\n' "$P" > m_top.c
  printf '%s\nint mid_fn(void); int main(void){printf("%%d\\n",mid_fn());return 0;}\n' "$P" > m_mid.c
  printf '%s\nint hook(void){return 42;} int htop_fn(void); int main(void){printf("%%d\\n",htop_fn());return 0;}\n' "$P" > m_htop.c
  printf '%s\nint hook(void){return 42;} int callhook(void); int main(void){printf("%%d\\n",callhook());return 0;}\n' "$P" > m_hook.c
  printf '%s\nint x_fn(void); int main(void){printf("%%d\\n",x_fn());return 0;}\n' "$P" > m_x.c
  printf '%s\nint xw_fn(void); int main(void){printf("%%d\\n",xw_fn());return 0;}\n' "$P" > m_xw.c
  printf '%s\nint umid_fn(void); int main(void){printf("%%d\\n",umid_fn());return 0;}\n' "$P" > m_umid.c
  printf '%s\nint cv; int get_cv(void); int main(void){printf("%%d %%d\\n",cv,get_cv());return 0;}\n' "$P" > m_cv.c
  printf '%s\nextern int pv; int get_pv(void); int main(void){printf("%%d %%d\\n",pv,get_pv());return 0;}\n' "$P" > m_pv.c
  printf '%s\nint pf(void); void *addr_pf(void); int main(void){printf("%%d %%d\\n",pf(),(void*)pf==addr_pf());return 0;}\n' "$P" > m_pf.c
  printf '%s\nint vf(void); int main(void){printf("%%d\\n",vf());return 0;}\n' "$P" > m_vf.c
  printf '%s\nextern int uq; int get_uq(void); int main(void){uq=6; printf("%%d %%d\\n",uq,get_uq());return 0;}\n' "$P" > m_uq.c
  printf '%s\nint ifn(void); void *addr_ifn(void); int main(void){printf("%%d %%d\\n",ifn(),(void*)ifn==addr_ifn());return 0;}\n' "$P" > m_if.c
  printf '%s\nint dup_fn(void); int main(void){printf("%%d\\n",dup_fn());return 0;}\n' "$P" > m_dup.c
  printf '%s\nint dup_extra(void); int main(void){printf("%%d\\n",dup_extra());return 0;}\n' "$P" > m_dupx.c
  printf '%s\nint q_fn(void); int main(void){printf("%%d\\n",q_fn());return 0;}\n' "$P" > m_q.c
  printf '%s\nint ns_fn(void); int main(void){printf("%%d\\n",ns_fn());return 0;}\n' "$P" > m_ns.c
  printf '%s\nint a_fn(void); int b_fn(void); int ns_fn(void); int top_fn(void);\nint main(void){printf("%%d\\n",a_fn()+b_fn()+ns_fn()+top_fn());return 0;}\n' "$P" > m_all.c
  printf '%s\nint wd(void); int main(void){printf("%%d\\n",wd());return 0;}\n' "$P" > m_wd.c
  printf '%s\nint wd(void){return 70;} int main(void){printf("%%d\\n",wd());return 0;}\n' "$P" > m_wddef.c
  printf '%s\nint a_fn(void){return 100;} int call_a(void); int main(void){printf("%%d %%d\\n",a_fn(),call_a());return 0;}\n' "$P" > m_def2.c
  printf '%s\n__attribute__((weak)) int a_fn(void){return 100;} int call_a(void); int main(void){printf("%%d %%d\\n",a_fn(),call_a());return 0;}\n' "$P" > m_wdef2.c
  printf '%s\nextern void *__tls_get_addr(void *); void *(*volatile p)(void *) = __tls_get_addr; int main(void){puts("tga");return 0;}\n' "$P" > m_tga.c
  printf '%s\nextern int _dl_find_object(void *, void *); int (*volatile p)(void *, void *) = _dl_find_object; int main(void){puts("dfo");return 0;}\n' "$P" > m_dfo.c
  printf '%s\nextern __thread int tv; int main(void){printf("%%d\\n",tv);return 0;}\n' "$P" > m_tls.c
  echo '__thread int tv = 4;' > libtls.c
  cd "$X"
}

build_libs() {
  local S=src D=$X/d
  so $D/liba.so liba.so $S/liba.c
  so $D/liba2.so liba2.so $S/liba2.c
  so $D/libb.so libb.so.1 $S/libb.c
  so $D/libns.so - $S/libns.c
  so $X/leafdir/libleaf.so libleaf.so $S/libleaf.c
  so $X/middir/libmid.so libmid.so $S/libmid.c -L$X/leafdir -lleaf
  so $X/topdir/libtop.so libtop.so $S/libtop.c -L$X/middir -lmid -Wl,-rpath-link,$X/leafdir
  so $X/topdir/libtopu.so libtopu.so $S/libtop.c          # underlinked: no DT_NEEDED libmid
  so $X/middir/libhmid.so libhmid.so $S/libhmid.c
  so $X/topdir/libhtop.so libhtop.so $S/libhtop.c -L$X/middir -lhmid
  so $D/libhook.so libhook.so $S/libhook.c
  so $D/libx.so libx.so $S/libx.c
  so $D/libxw.so libxw.so $S/libxw.c
  so $X/leafdir/libuleaf.so libuleaf.so $S/libuleaf.c
  so $X/middir/libumid.so libumid.so $S/libumid.c -L$X/leafdir -luleaf
  so $D/libver.so libver.so $S/libver.c -Wl,--version-script,$S/libver.map
  so $D/libver_loc.so libver_loc.so $S/libver.c -Wl,--version-script,$S/libver_loc.map
  so $D/libweak.so libweak.so $S/libweak.c
  so $D/libcom.so libcom.so $S/libcom.c
  so $D/libprot.so libprot.so $S/libprot.c
  so $D/libvis.so libvis.so $S/libvis.c
  python3 -I "$HERE/patch_vis.py" $D/libvis.so vf 2 >> "$LOG" 2>&1   # vf -> STV_HIDDEN in .dynsym
  so $D/libuniq.so libuniq.so $S/libuniq.c
  so $D/libifunc.so libifunc.so $S/libifunc.c
  so $D/libdup1.so libdup.so.1 $S/libdup1.c
  so $D/libdup2.so libdup.so.1 $S/libdup2.c
  so $D/libref.so libref.so $S/libref.c
  ln -sfn libb.so $D/libb.so.1; ln -sfn libdup1.so $D/libdup.so.1   # run-time names
  so $D/libtls.so libtls.so $S/libtls.c
  so $X/q/libq.so libq.so $S/libq_so.c
  obj $X/q/q_a.o $S/libq_a.c; ar rcs $X/q/libq.a $X/q/q_a.o
  mkdir -p $X/qa; obj $X/qa/q_a3.o $S/libq_a3.c; ar rcs $X/qa/libq.a $X/qa/q_a3.o
  mkdir -p $X/arc; obj $X/arc/arc_a.o $S/arc_a.c; ar rcs $X/arc/libarc.a $X/arc/arc_a.o
  # scripts
  echo "GROUP ( $D/libns.so $D/libb.so AS_NEEDED ( $D/liba.so $D/liba2.so $D/libhook.so ) $X/topdir/libtop.so )" > $D/libgrp.so
  echo "INPUT ( $D/liba.so -lns )" > $D/libinp.so
  ln -sfn $D/nonexistent.so $D/libdang.so
  : > $D/libempty.so
  echo "hello world" > $D/libjunk.so
  printf '\177ELF\002\001\001\000garbagegarbage' > $D/libbad.so
  # objects
  for m in a wa def hello v v1 v3 hid vdef top mid htop hook x xw umid pv pf vf uq if dup dupx q ns all wd wddef def2 wdef2 tga dfo tls; do
    obj $X/m_$m.o src/m_$m.c; done
  obj $X/m_cv.o src/m_cv.c -fcommon
  obj $X/m_gc.o src/m_gc.c -ffunction-sections
  for f in $(find $D $X/leafdir $X/middir $X/topdir -name '*.so' -type f | sort); do
    log "lib $(echo $f | sed "s#$X/##") SONAME=$(readelf -dW $f 2>/dev/null | sed -n 's/.*(SONAME).*\[\(.*\)\]/\1/p') NEEDED=$(needed $f)"; done
}

source "$HERE/experiments.sh"

for ARCH in aarch64 x86_64; do
  [ "$WHICH" != all ] && [ "$WHICH" != $ARCH ] && continue
  X=$W/$ARCH; mkdir -p $X/out $X/d; LOG=$X/log.txt; : > $LOG
  CC=$(pfx $ARCH)gcc
  LLP=$X/d:$X/leafdir:$X/middir:$X/topdir:$X/q
  write_sources
  build_libs
  experiments
  echo "done $ARCH: $LOG"
done

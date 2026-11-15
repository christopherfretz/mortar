#!/bin/bash
# needed_order_inputs.sh CC: the inputs of dynamic/needed_order.sh (WP-040,
# review finding F5; .build/review-m2/dynamic/needed_order.sh), built in the
# current directory with the driver CC. Ported for WP-043's oracle
# (tests/shlib_oracle.rs); the cases are in that file.
set -eu
CC=$1
mkdir -p d1 d2
mklib() { # name [extra source] [extra link args]
  printf '#include <stdio.h>\n__attribute__((constructor)) static void ctor(void){ printf("%s "); fflush(stdout); }\nint f_%s(void){return 1;}\n%s\n' \
    "$1" "$1" "${2:-}" > "$1.c"
  $CC -O1 -fPIC -shared "$1.c" -o "lib$1.so" -Wl,-soname,"lib$1.so" ${3:-}
}
for n in na nb nc nd ne nf nx ny; do mklib $n; done
# nu: under-linked, references f_ny without a DT_NEEDED on libny.so
mklib nu 'int f_ny(void); int g_nu(void){return f_ny();}'
# nv: properly linked, DT_NEEDED libny.so, references f_ny
mklib nv 'int f_ny(void); int g_nv(void){return f_ny();}' "-L. -lny"
# nw: under-linked, references f_nx (a library that main never references)
mklib nw 'int f_nx(void); int g_nw(void){return f_nx();}'
# two more files with soname libnc.so in d1/ and d2/ (ctor prints nc1/nc2)
for k in 1 2; do
  printf '#include <stdio.h>\n__attribute__((constructor)) static void ctor(void){ printf("nc%s "); fflush(stdout); }\nint f_nc(void){return 1;}\n' $k > d$k/nc.c
  $CC -O1 -fPIC -shared d$k/nc.c -o d$k/libnc.so -Wl,-soname,libnc.so
done
# a static libnc.a (ctor prints nc.a)
printf '#include <stdio.h>\n__attribute__((constructor)) static void ctor(void){ printf("nc.a "); fflush(stdout); }\nint f_nc(void){return 1;}\n' > nc_static.c
$CC -O1 -fPIE -c nc_static.c -o nc_static.o; rm -f libnc.a; ar rcs libnc.a nc_static.o
# main objects: mainX.o calls the listed functions
mkmain() { # obj funcs...
  local o=$1; shift; local decl="" body="0"
  for f in "$@"; do decl="$decl int $f(void);"; body="$body+$f()"; done
  printf '#include <stdio.h>\n%s\nint main(void){ int r=%s; printf("\\n"); return r==%d?0:1; }\n' "$decl" "$body" $# > "${o%.o}.c"
  $CC -O1 -fPIE -c "${o%.o}.c" -o "$o"
}
mkmain main.o  f_na f_nb f_nc f_nd f_ne f_nf
mkmain mainu.o f_na f_nb f_nc g_nu
mkmain mainv.o f_na f_nb g_nv
mkmain mainw.o f_na f_nb g_nw
mkmain mainy.o f_na f_nb f_ny
mkmain main0.o
mkmain main_abd.o f_na f_nb f_nd
mkmain main_z.o f_na f_nz
# main_wk.o references f_nc only weakly (plus f_na f_nb)
printf '#include <stdio.h>\nint f_na(void),f_nb(void); extern int f_nc(void) __attribute__((weak));\nint main(void){ int r=f_na()+f_nb()+(f_nc?f_nc():0); printf("\\n"); return r>=2?0:1; }\n' > main_wk.c
$CC -O1 -fPIE -c main_wk.c -o main_wk.o
# usec.o: an object whose only job is to reference f_nc
echo 'int f_nc(void); int use_c(void){return f_nc();}' > usec.c; $CC -O1 -fPIE -c usec.c -o usec.o
# libnz.so: NO soname
printf '#include <stdio.h>\n__attribute__((constructor)) static void ctor(void){ printf("nz "); fflush(stdout); }\nint f_nz(void){return 1;}\n' > nz.c
$CC -O1 -fPIC -shared nz.c -o libnz.so
# linker scripts
echo 'GROUP ( libna.so libnb.so )'                          > s_grp_ab.so
echo 'INPUT ( libnc.so libnd.so )'                          > s_inp_cd.so
echo 'GROUP ( libnd.so AS_NEEDED ( libnc.so libnx.so ) )'   > s_grp_d_asn_cx.so
echo 'INPUT ( AS_NEEDED ( libnc.so ) )'                     > s_inp_asn_c.so
echo 'INPUT ( s_inp_cd.so libnb.so )'                       > s_nest.so
echo 'INPUT ( libne.so s_grp_d_asn_cx.so libnf.so )'        > s_nest2.so
echo 'INPUT ( -lnc )'                                       > s_inp_lc.so
echo 'GROUP ( AS_NEEDED ( libnc.so ) usec.o )'              > s_grp_rescan.so

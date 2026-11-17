#!/bin/sh
# Regenerates WP-161's fixtures of tests/shared.rs (`p_d*_mortar_option_tail`):
# mortar's outputs of the option tail of -shared on AArch64 and x86-64:
#   libz   -shared, the five D-103 -z flags, -Bsymbolic, -soname, -init/-fini
#   libex  -shared with an archive member --exclude-libs ALL excludes
#   libdl  -shared -Bsymbolic --dynamic-list dl.list
#   e-plain, e-ex  -pie -E without and with --exclude-libs ALL
#   d-plain        -pie, no export option
# The outputs are checked in.
#   sh gen.sh <mortar>
# Run from this directory. First made with the mortar of WP-161's base
# (int/batch-9, 71e376e).
set -e
mortar=$1
for a in a64 x64; do
  case $a in
    a64) cc=aarch64-linux-gnu-gcc; ld=/lib/ld-linux-aarch64.so.1;;
    x64) cc=x86_64-linux-gnu-gcc; ld=/lib64/ld-linux-x86-64.so.2;;
  esac
  "$cc" -O1 -fPIC -mtls-dialect=desc -c opt.c -o opt-$a.o 2>/dev/null ||
    "$cc" -O1 -fPIC -mtls-dialect=gnu2 -c opt.c -o opt-$a.o
  "$cc" -O1 -fPIC -c xfoo.c -o xfoo-$a.o
  "$cc" -O1 -fPIE -c xmain.c -o xmain-$a.o
  "$cc" -O1 -fPIC -c xdd.c -o xdd-$a.o
  ar rc libxfoo-$a.a xfoo-$a.o
  "$mortar" -shared -o libxdd-$a.so -soname libxdd.so xdd-$a.o
  "$mortar" -shared -o libz-$a.so opt-$a.o libxfoo-$a.a -z nodelete -z nodlopen \
    -z initfirst -z interpose -z origin -Bsymbolic -soname libz.so \
    -init opt_init -fini opt_fini --mortar-map=libz-$a.map
  "$mortar" -shared -o libex-$a.so opt-$a.o libxfoo-$a.a --exclude-libs ALL \
    --mortar-map=libex-$a.map
  "$mortar" -shared -o libdl-$a.so opt-$a.o libxfoo-$a.a -Bsymbolic --dynamic-list dl.list \
    --mortar-map=libdl-$a.map
  for e in e-plain e-ex d-plain; do
    case $e in e-plain) x=-E;; e-ex) x="-E --exclude-libs ALL";; d-plain) x=;; esac
    # shellcheck disable=SC2086
    "$mortar" -pie -dynamic-linker $ld -e main -o $e-$a xmain-$a.o libxfoo-$a.a \
      libxdd-$a.so $x --mortar-map=$e-$a.map
  done
done

#!/bin/bash
# census/run.sh WORK: the ld line each compiler driver writes for -r and its
# variants (A of WP-154). Each line is captured by a -B wrapper that execs GNU ld.
set -u
WORK=$(realpath -m "$1"); HERE=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$WORK/tmp"; cd "$WORK" || exit 1; export TMPDIR=$WORK/tmp
for a in aarch64 x86_64 i686; do mkdir -p w/$a; for n in ld ld.bfd $a-linux-gnu-ld $a-linux-gnu-ld.bfd; do ln -sf "$HERE/wrap_ld.sh" w/$a/$n; done; done
drivers() {
  echo "gcc aarch64 gcc"; echo "g++ aarch64 g++"; echo "gfortran aarch64 gfortran"
  echo "x86_64-linux-gnu-gcc x86_64 gcc"; echo "x86_64-linux-gnu-g++ x86_64 g++"; echo "x86_64-linux-gnu-gfortran x86_64 gfortran"
  echo "i686-linux-gnu-gcc i686 gcc"; echo "i686-linux-gnu-g++ i686 g++"; echo "i686-linux-gnu-gfortran i686 gfortran"
  for v in 19 22; do for a in aarch64 x86_64 i686; do echo "clang-$v $a clang"; echo "clang++-$v $a clang++"; done; done
}
VARIANTS=("-r" "-r -nostdlib" "-nostdlib -Wl,-r" "-Wl,-r" "-r -static" "-r -pie" "-r -no-pie" "-r -shared" "-r -g" "-r -Wl,--gc-sections" "-r -s" "-r -static-pie" "-r -pthread" "-r -flto" "-no-pie -Wl,-r" "-no-pie -nostdlib -Wl,-r" "-r -Wl,--gc-sections,-e,a" "-r -fuse-ld=lld" "-r -fuse-ld=mold")
norm() { sed -e "s#$WORK#W#g" -e 's#/usr/lib/gcc[^ ]*/\([^/ ]*\.o\)#GCC/\1#g' -e 's#/usr/[^ ]*lib[^ ]*/\(crt[^/ ]*\.o\)#SYS/\1#g' -e 's#-plugin-opt=-fresolution=[^ ]*#-plugin-opt=-fresolution=R#' -e 's# -L[^ ]*##g' -e 's#-plugin [^ ]*liblto_plugin.so#-plugin P#' -e 's#-plugin-opt=[^ ]*lto-wrapper#-plugin-opt=LW#' -e 's#tmp/cc[A-Za-z0-9]*\.o#tmp/X.o#g' ; }
drivers | while read -r drv arch kind; do
  t=""; case $drv in clang*) t="--target=$arch-linux-gnu";; esac
  src=$HERE/src/a.c; [ "$kind" = gfortran ] && src=$HERE/src/f.f90
  $drv $t -c "$src" -o obj.o 2>/dev/null || { echo "$drv $arch: cannot compile"; continue; }
  for v in "${VARIANTS[@]}"; do
    out=out.o; : > line.args
    msg=$(CENSUS_ARGS=$WORK/line.args $drv $t -B "$WORK/w/$arch/" $v obj.o -o "$out" 2>&1); rc=$?
    line="(ld not run)"; [ -s line.args ] && line=$(tr '\n' ' ' < line.args | norm)
    typ=$( [ -f "$out" ] && readelf -h "$out" 2>/dev/null | awk '/Type:/{print $2}')
    printf '%s|%s|%s|rc=%s type=%s|%s|%s\n' "$drv" "$arch" "$v" "$rc" "${typ:-none}" "$line" "$(echo "$msg" | head -2 | tr '\n' ' ' | cut -c1-200)"
    [ -f out.o ] && mv out.o last-out.o
  done
done > lines.txt
echo "census: $(wc -l < lines.txt) lines" 

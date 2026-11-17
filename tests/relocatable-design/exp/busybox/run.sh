#!/bin/bash
# busybox/run.sh WORK: BusyBox 1.36.1's two -r lines (Kbuild: LD = $(CC)
# -nostdlib; `$(LD) -nostdlib $(ld_flags) -r -o applets/built-in.o applets.o`),
# built with gcc and clang-22; applets/built-in.o redone by GNU ld, lld 22,
# mold and mortar through the same driver line; then the final link
# (trylink's line, replayed from the log) by GNU ld and by mortar, and the
# program run.
set -u
ulimit -c 0
WORK=$(realpath -m "$1"); HERE=$(cd "$(dirname "$0")" && pwd)
MORTAR=${2:-/Volumes/AgentWork/mortar-helper/.build/target-wp-154/mortar-3ad9867a}
mkdir -p "$WORK"; cd "$WORK" || exit 1
mkdir -p bin/bfd bin/lld bin/mold bin/mortar
ln -sf /usr/bin/ld.bfd bin/bfd/ld; ln -sf /usr/bin/ld.lld-22 bin/lld/ld; ln -sf /usr/bin/mold bin/mold/ld; ln -sf "$MORTAR" bin/mortar/ld
for cc in gcc clang-22; do
  [ -f "$WORK/$cc/src/busybox-1.36.1/busybox_unstripped" ] || "$HERE/build.sh" "$WORK/$cc" "$cc" > "$WORK/$cc.build.out" 2>&1
  S=$WORK/$cc/src/busybox-1.36.1; cd "$S" || continue
  echo "== $cc: the -r line: $(awk 'BEGIN{RS=""} /\n-r\n/ {last=$0} END{print last}' "$WORK/$cc/ld.log" | tail -n +2 | tr '\n' ' ' | sed -e "s#$WORK#W#g" -e 's# -L[^ ]*##g')"
  # the final ld line (trylink's last), as an argument file
  awk 'BEGIN{RS=""} /\nbusybox_unstripped\n/ {last=$0} END{print last}' "$WORK/$cc/ld.log" | tail -n +2 | awk 'NF' > "$WORK/$cc.final.args"
  sed -i -e '/^-Map$/,+1d' -e '/^--verbose$/d' "$WORK/$cc.final.args"
  for k in bfd lld mold mortar; do
    msg=$($cc -B"$WORK/bin/$k/" -nostdlib -r -o "applets/built-in.$k.o" applets/applets.o 2>&1); rc=$?
    echo "-r by $k: rc=$rc $(echo "$msg" | head -2 | tr '\n' ' ' | cut -c1-200)"
    [ $rc = 0 ] || continue
    readelf -SW "applets/built-in.$k.o" | awk '/^ *\[ *[0-9]/{sub(/^ *\[ *[0-9]+\] */,""); printf "%s(%s,%s) ", $1, $2, $7}' ; echo
    for fin in bfd mortar; do
      sed "s#^applets/built-in.o\$#applets/built-in.$k.o#; s#^busybox_unstripped\$#bb.$k.$fin#" "$WORK/$cc.final.args" > "$WORK/$cc.$k.$fin.args"
      case $fin in bfd) L=/usr/bin/ld.bfd;; mortar) L=$MORTAR;; esac
      msg=$($L @"$WORK/$cc.$k.$fin.args" 2>&1); rc=$?
      if [ $rc != 0 ]; then echo "   final $fin: LINK FAILED rc=$rc $(echo "$msg" | head -3 | tr '\n' ' ' | cut -c1-300)"; continue; fi
      mkdir -p "run.$k.$fin"; ln -f "bb.$k.$fin" "run.$k.$fin/busybox"; B=$PWD/run.$k.$fin/busybox
      out=$("$B" echo ok 2>&1; "$B" sh -c 'echo $((6*7)); printf "%s\n" abc | '"$B"' sha256sum | '"$B"' cut -c1-16' 2>&1; "$B" ls applets/applets.o 2>&1; "$B" awk 'BEGIN{print length("mortar")}' 2>&1)
      echo "   final $fin: $(echo "$out" | tr '\n' '|')"
    done
  done
  cd "$WORK"
done

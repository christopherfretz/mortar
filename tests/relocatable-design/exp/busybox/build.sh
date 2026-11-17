#!/bin/bash
# busybox/build.sh WORK CC: build BusyBox 1.36.1 (defconfig, CONFIG_TC off) with
# CC (gcc or clang-22) through a -B wrapper that logs every ld line in WORK/ld.log.
set -u
WORK=$(realpath -m "$1"); CCX=$2; HERE=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$WORK/src" "$WORK/w" "$WORK/tmp"; export TMPDIR=$WORK/tmp
ln -sf "$HERE/wrap_log.sh" "$WORK/w/ld"
cd "$WORK/src" && tar xjf /Volumes/AgentWork/mortar-helper/.build/real-builds/downloads/busybox-1.36.1.tar.bz2 && cd busybox-1.36.1 || exit 1
export LDLOG=$WORK/ld.log; : > "$LDLOG"
make defconfig > ../defconfig.log 2>&1
sed -i -e 's/^CONFIG_TC=y/# CONFIG_TC is not set/' .config
yes '' | make oldconfig > ../oldconfig.log 2>&1
make -j4 CC="$CCX -B$WORK/w/" HOSTCC=gcc V=1 > ../make.log 2>&1; echo "make rc=$?"
ls -la busybox_unstripped applets/built-in.o 2>&1
echo "ld lines: $(grep -c '^CWD' "$LDLOG"); -r lines: $(awk 'BEGIN{RS=""} /\n-r\n/' "$LDLOG" | grep -c '^CWD')"

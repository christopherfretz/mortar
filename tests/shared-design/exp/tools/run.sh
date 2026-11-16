#!/bin/bash
# WP-110 area `tools`: debuggers, tools, dlopen lifecycles, interposition,
# and mortar as the host's linker, on shared objects made by GNU ld, lld 22,
# lld 19 and mold.  Usage: run.sh WORK [parts]   (WORK must not exist)
# parts default to "a b c d e"; each writes WORK/<part>/ and its log.
set -u
W=$1; shift
PARTS=${*:-a c d b e}
S=$(cd "$(dirname "$0")" && pwd)
ulimit -c 0
if [ ! -d "$W/bin" ]; then
    [ -e "$W" ] && { echo "WORK $W exists; give a fresh path"; exit 1; }
    mkdir -p "$W/bin/l22" "$W/bin/l19" "$W/bin/mortar"
    ln -s /usr/bin/ld.lld-22 "$W/bin/l22/ld.lld"
    ln -s /usr/bin/ld.lld-19 "$W/bin/l19/ld.lld"
    ln -s /Volumes/AgentWork/mortar-helper/.build/target-wp-110/release/mortar "$W/bin/mortar/ld"
fi
W=$(cd "$W" && pwd); cd "$W"
{
    for t in gcc g++ gfortran x86_64-linux-gnu-gcc x86_64-linux-gnu-g++ x86_64-linux-gnu-gfortran \
             ld.bfd x86_64-linux-gnu-ld.bfd ld.lld-22 ld.lld-19 mold gdb gdb-multiarch lldb-19 \
             eu-elflint readelf objdump nm qemu-x86_64; do
        printf '%s: ' $t; $t --version 2>&1 | head -1
    done
    printf 'mortar: '; /Volumes/AgentWork/mortar-helper/.build/target-wp-110/release/mortar --version
    printf 'glibc: '; /lib/ld-linux-aarch64.so.1 --version | head -1
} > "$W/versions.txt" 2>&1
for p in $PARTS; do
    echo "=== part $p"
    bash "$S/part_$p.sh" "$W" > "$W/part_$p.log" 2>&1
    echo "    log: $W/part_$p.log"
done

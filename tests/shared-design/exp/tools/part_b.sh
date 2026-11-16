#!/bin/bash
# Part B: ldd, eu-elflint --gnu-ld, readelf -a warnings, nm -D, objdump -T,
# ld.so --verify on every library parts A, C and D built (run it after them).
# Usage: part_b.sh WORK
W=$1; S=$(cd "$(dirname "$0")" && pwd); . "$S/common.sh"
B=$W/b; [ -e "$B" ] && { echo "$B exists: use a fresh WORK"; exit 1; }; mkdir -p "$B"
L=$B/log.txt; : > "$L"
for a in $ARCHS; do
    if [ $a = aa ]; then RE=readelf; OD=objdump; NM=nm; else RE=readelf; OD=x86_64-linux-gnu-objdump; NM=x86_64-linux-gnu-nm; fi
    for k in $LINKERS; do
        o=$B/$a.$k; : > $o.elflint; : > $o.ldd; : > $o.readelf; : > $o.verify; : > $o.syms
        for part in a c d; do
            d=$W/$part/$a/$k; [ -d $d ] || continue
            for f in $(cd $d && find . -name '*.so' | sort); do
                lib=$d/${f#./}; tag="$part/${f#./}"
                # eu-elflint (it reads ELF of any machine)
                r=$(eu-elflint --gnu-ld $lib 2>&1); echo "== $tag rc=$?" >> $o.elflint
                [ -n "$r" ] && echo "$r" | sed "s#$W#W#g" >> $o.elflint
                # readelf -a: warnings and errors only
                echo "== $tag" >> $o.readelf
                $RE -a -W $lib 2>&1 >/dev/null | sed "s#$W#W#g" >> $o.readelf
                # ld.so --verify (exit 0 = dynamic object, 1 = not dynamic, 2 = not ELF/bad)
                if [ $a = aa ]; then /lib/ld-linux-aarch64.so.1 --verify $lib > /dev/null 2>&1; v=$?
                else qemu-x86_64 -L $X64ROOT $X64ROOT/lib/ld-linux-x86-64.so.2 --verify $lib > /dev/null 2>&1; v=$?; fi
                echo "$tag verify-rc=$v" >> $o.verify
                # ldd (library dependencies resolved against the same directory)
                echo "== $tag" >> $o.ldd
                if [ $a = aa ]; then LD_LIBRARY_PATH=$d ldd $lib 2>&1
                else qemu-x86_64 -L $X64ROOT -E LD_LIBRARY_PATH=$d $X64ROOT/lib/ld-linux-x86-64.so.2 --list $lib 2>&1; fi \
                    | sed -E "s#$W#W#g; s/\(0x[0-9a-f]+\)/(ADDR)/" >> $o.ldd
                echo "rc=${PIPESTATUS[0]}" >> $o.ldd
                # dynamic symbol tables: names, types, binding, section kind
                echo "== $tag" >> $o.syms
                $NM -D --defined-only $lib 2>&1 | awk '{print $2, $3}' | sort >> $o.syms
                n1=$($NM -D $lib 2>/dev/null | wc -l); n2=$($OD -T $lib 2>/dev/null | grep -c -E '^[0-9a-f]{8,}')
                echo "nm-D=$n1 objdump-T=$n2" >> $o.syms
            done
            # host executables (GNU ld) of the part
            for h in $(cd $d && find . -maxdepth 1 -type f -perm -u+x ! -name '*.so' | sort); do
                echo "== host $part/${h#./}" >> $o.ldd
                if [ $a = aa ]; then LD_LIBRARY_PATH=$d ldd $d/${h#./} 2>&1
                else qemu-x86_64 -L $X64ROOT -E LD_LIBRARY_PATH=$d $X64ROOT/lib/ld-linux-x86-64.so.2 --list $d/${h#./} 2>&1; fi \
                    | sed -E "s#$W#W#g; s/\(0x[0-9a-f]+\)/(ADDR)/" >> $o.ldd
                echo "rc=${PIPESTATUS[0]}" >> $o.ldd
            done
        done
    done
done
# controls for ld.so --verify exit codes
{
  /lib/ld-linux-aarch64.so.1 --verify /lib/aarch64-linux-gnu/libc.so.6 >/dev/null 2>&1; echo "aa libc.so.6 rc=$?"
  /lib/ld-linux-aarch64.so.1 --verify $W/a/aa/bfd/host >/dev/null 2>&1; echo "aa GNU ld PIE host rc=$?"
  /lib/ld-linux-aarch64.so.1 --verify $S/run.sh >/dev/null 2>&1; echo "aa a shell script rc=$?"
  /lib/ld-linux-aarch64.so.1 --verify $W/a/aa/bfd/host.c.o.missing >/dev/null 2>&1; echo "aa a missing file rc=$?"
  qemu-x86_64 -L $X64ROOT $X64ROOT/lib/ld-linux-x86-64.so.2 --verify $X64ROOT/lib/libc.so.6 >/dev/null 2>&1; echo "x64 libc.so.6 rc=$?"
  qemu-x86_64 -L $X64ROOT $X64ROOT/lib/ld-linux-x86-64.so.2 --verify $W/a/x64/bfd/host >/dev/null 2>&1; echo "x64 GNU ld PIE host rc=$?"
} > $B/verify-controls.txt
python3 -I "$S/check_b.py" "$B" >> "$L"
cat "$L"

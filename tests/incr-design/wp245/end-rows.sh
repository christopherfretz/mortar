#!/bin/bash
# WP-245 step 4 on bytes: the real object pair whose edit grows `.bss`
# (file a.c: `buf` 64 B -> 64 KiB) linked in full by mortar
# (`--incremental=full`, static) for both 64-bit targets: `_end` and
# `_edata` are the ends of their chunks (the end of `.bss`, of the last
# chunk with file bytes) in both versions, so an update that grows `.bss`
# must move `_end` by the growth (`end_rows_follow_a_grown_chunk` in
# crates/mortar_layout/tests/delta.rs shows the delta layout on the same
# shape). Usage: end-rows.sh <mortar binary> <scratch dir>.
set -eu
mortar=$1 out=$2
mkdir -p "$out/bin"
ln -sf "$mortar" "$out/bin/ld"
cat > "$out/main.c" <<'C'
extern char buf[], _end[], _edata[];
int main(void) { return buf[0] + (_end > _edata); }
C
for t in aarch64 x86_64; do
  cc=$t-linux-gnu-gcc
  for v in 64 65536; do
    d=$out/$t-$v; mkdir -p "$d"
    printf 'char buf[%s];\nint pad_a = 1;\n' "$v" > "$d/a.c"
    $cc -O0 -c "$d/a.c" -o "$d/a.o"
    $cc -O0 -c "$out/main.c" -o "$d/main.o"
    $cc -static -B"$out/bin/" -Wl,--incremental=full "$d/main.o" "$d/a.o" -o "$d/prog"
    end=$(nm "$d/prog" | awk '$3=="_end"{print $1}')
    edata=$(nm "$d/prog" | awk '$3=="_edata"{print $1}')
    bss=$(readelf -SW "$d/prog" | awk '$2==".bss"{print $4, $6}')
    set -- $bss
    bss_end=$(printf '%x' $((16#$1 + 16#$2)))
    echo "$t buf=$v: _end=$end .bss=[$1 +$2) end=$bss_end _edata=$edata"
  done
done
echo "end-rows: DONE"

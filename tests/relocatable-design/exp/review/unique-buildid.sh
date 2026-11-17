#!/bin/bash
# review/unique-buildid.sh WORK: (1) --unique[=PAT] at -r by each reference, on two
# inputs with same-named sections (.text, .data, .text.helper), and the final
# --gc-sections link of each output (the x06 case 2 of the review); (2) the bytes
# a -r --build-id note hashes, per reference and style.
set -u; ulimit -c 0
W=$(realpath -m "$1"); mkdir -p "$W"; cd "$W" || exit 1
M=/Volumes/AgentWork/mortar-helper/.build/target-wp-154/mortar-3ad9867a; mkdir -p mbin; ln -sf $M mbin/ld
cat > g1.c <<'C'
static int helper(int x) { return x + 1; }
int d1 = 1;
int f1(int x) { return helper(x); }
C
cat > g2.c <<'C'
extern int missing_function(int);
static int helper(int x) { return missing_function(x); }
int d2 = 2;
int f2(int x) { return helper(x); }
C
printf 'int f1(int);\nint main(void){return f1(41) - 42;}\n' > gm.c
for f in g1 g2 gm; do gcc -O0 -ffunction-sections -fdata-sections -c $f.c -o $f.o; done
summ() { echo "$(readelf -SW $1 | grep -cE '\] \.text') .text*, $(readelf -SW $1 | grep -cE '\] \.data') .data*: $(readelf -SW $1 | grep -E '\] \.(text|data)' | awk '{print $2}' | tr '\n' ' ')| UND f1/f2/d1: $(readelf -sW $1 | awk '$7=="UND" && $8 ~ /^(f1|f2|d1|d2)$/' | wc -l)"; }
for v in "" "--unique" "--unique=.text.helper" "--unique=.text*"; do for L in bfd lld mold; do
  o=u.$L.$(echo "x$v" | tr -dc 'a-z').o
  case $L in bfd) c="ld.bfd -r";; lld) c="ld.lld -r";; mold) c="mold -r";; esac
  if $c $v -o $o g1.o g2.o > err 2>&1; then
    g=$(gcc -Wl,--gc-sections gm.o $o -o e.$o > /dev/null 2>&1 && ./e.$o && echo "runs" || echo "fails"); gm=$(gcc -B$W/mbin/ -Wl,--gc-sections gm.o $o -o em.$o > /dev/null 2>&1 && ./em.$o && echo "runs" || echo "fails")
    echo "$L ${v:-(none)}: $(summ $o); final --gc-sections: GNU ld $g, mortar $gm"
  else echo "$L ${v:-(none)}: error: $(head -1 err)"; fi; done; done
echo "-- the same, with -ffunction-sections off (both inputs' functions in .text)"
for f in g1 g2; do gcc -O0 -c $f.c -o n$f.o; done
for L in bfd lld; do case $L in bfd) c="ld.bfd -r";; lld) c="ld.lld -r";; esac; $c --unique -o n.$L.o ng1.o ng2.o 2>&1 | head -1; echo "$L --unique: $(summ n.$L.o); final --gc-sections: GNU ld $(gcc -Wl,--gc-sections gm.o n.$L.o -o en.$L > /dev/null 2>&1 && ./en.$L && echo runs || echo fails), mortar $(gcc -B$W/mbin/ -Wl,--gc-sections gm.o n.$L.o -o enm.$L > /dev/null 2>&1 && ./enm.$L && echo runs || echo fails)"; done
echo "-- build-id at -r"
for v in "--build-id" "--build-id=sha1" "--build-id=md5" "--build-id=uuid" "--build-id=0x0102" "--build-id=fast"; do for L in bfd lld mold; do
  o=b.$L.$(echo "x$v" | tr -dc 'a-z0-9').o
  case $L in bfd) c="ld.bfd -r";; lld) c="ld.lld -r";; mold) c="mold -r";; esac
  if $c $v -o $o g1.o g2.o > err 2>&1; then echo "$L $v: $(python3 -I - $o <<'PY'
import sys, hashlib, struct
b = bytearray(open(sys.argv[1], 'rb').read())
shoff, = struct.unpack_from('<Q', b, 40); shentsize, shnum, shstrndx = struct.unpack_from('<HHH', b, 58)
def sh(i): return struct.unpack_from('<IIQQQQIIQQ', b, shoff + i * shentsize)
names = sh(shstrndx)
def nm(i): o = names[4] + sh(i)[0]; return bytes(b[o:b.index(0, o)]).decode()
for i in range(shnum):
    if nm(i) == '.note.gnu.build-id':
        off, size = sh(i)[4], sh(i)[5]; namesz, descsz, typ = struct.unpack_from('<III', b, off)
        d = off + 12 + ((namesz + 3) & ~3); desc = bytes(b[d:d + descsz])
        z = bytearray(b); z[d:d + descsz] = bytes(descsz)
        whole = {n: getattr(hashlib, n)(z).digest()[:descsz] for n in ('sha1', 'md5')}
        tree = hashlib.sha1(b''.join(hashlib.sha1(z[k:k + (1 << 20)]).digest() for k in range(0, len(z), 1 << 20))).digest()[:descsz]
        how = [n + ' of the file with the desc zeroed' for n, v in whole.items() if v == desc] + (['SHA-1 tree of 1 MiB chunks'] if tree == desc else [])
        print('%d bytes, %s' % (descsz, ', '.join(how) or 'not a whole-file hash'))
        break
else: print('no note')
PY
)"; else echo "$L $v: error: $(head -1 err)"; fi; done; done
echo "-- the output section names of GNU ld 2.44's -r script per emulation (bare --unique splits every other name)"
for LD in ld.bfd x86_64-linux-gnu-ld i686-linux-gnu-ld; do n=$($LD --verbose -r | sed -n '/^======/,/^======/p' | grep -oE '^\s*\.[A-Za-z0-9_.]+\s+0?\s*:' | tr -d ' \t:0' | sort -u); echo "$LD ($(echo "$n" | wc -l) names): $(echo $n)"; done
echo "unique-buildid done"

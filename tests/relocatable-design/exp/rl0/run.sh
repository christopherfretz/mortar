#!/bin/bash
# rl0/run.sh WORK: WP-180's measurements (the verifier's undecided cells),
# each case partially linked by GNU ld 2.44, lld 22 and mold 2.37.1. The log
# is tests/relocatable-design/work/rl0.log. Cases:
#  1 maps        what -Map says at -r (placement of input sections)
#  2 rel386      i386 REL: section-symbol, local and global records, fields
#  3 groups      SHT_GROUP: flags, flag word, members, SHF_GROUP on .rela
#  4 empty       empty input sections: output sections and STT_SECTION symbols
#  5 noneonly    a relocation section that holds only R_*_NONE records
#  6 property    .note.gnu.property merged or not (AArch64, x86-64)
#  7 retain      SHF_GNU_RETAIN and plain pieces of one name
#  8 dropdebug   debug and .eh_frame records into a dropped COMDAT copy
#  9 linkorder   SHF_LINK_ORDER pieces linked to different sections
# 10 entsize     SHF_MERGE pieces of one name with different sh_entsize
# 11 nobits      PROGBITS and NOBITS pieces of one name
# 12 symsec      STT_SECTION values; the section of a symbol in a merged piece
# 13 cgprofile   clang's .llvm.call-graph-profile and its SHT_REL of NONE records
# 14 dupmaps     -Map with output sections of one name (the same file twice)
set -u; ulimit -c 0; HERE=$(cd "$(dirname "$0")" && pwd)
WORK=$(realpath -m "$1"); mkdir -p "$WORK"; cd "$WORK" || exit 1
R() { # R ARCH REF OUT ARGS...
  local a=$1 k=$2 o=$3; shift 3
  case $k in
    bfd) case $a in aarch64) ld.bfd -r -o "$o" "$@";; x86_64) x86_64-linux-gnu-ld.bfd -r -o "$o" "$@";; i386) i686-linux-gnu-ld.bfd -r -o "$o" "$@";; esac;;
    lld) case $a in aarch64) ld.lld-22 -r -o "$o" "$@";; x86_64) ld.lld-22 -m elf_x86_64 -r -o "$o" "$@";; i386) ld.lld-22 -m elf_i386 -r -o "$o" "$@";; esac;;
    mold) case $a in aarch64) mold -r -o "$o" "$@";; x86_64) mold -m elf_x86_64 -r -o "$o" "$@";; i386) mold -m elf_i386 -r -o "$o" "$@";; esac;;
  esac
}
CC() { local a=$1; shift; case $a in aarch64) gcc "$@";; x86_64) x86_64-linux-gnu-gcc "$@";; i386) i686-linux-gnu-gcc "$@";; esac; }
RE() { case $1 in aarch64) shift; readelf "$@";; *) shift; readelf "$@";; esac; }
each() { # each CASE ARCH DESCRIBE INPUTS...: link by each reference, describe
  local c=$1 a=$2 d=$3; shift 3
  for k in bfd lld mold; do
    local o=$c.$a.$k.o msg rc
    msg=$(R $a $k $o "$@" 2>&1); rc=$?
    echo "--- $c $a $k rc=$rc ${msg:+[diag: $(echo "$msg" | head -2 | tr '\n' ' ' | cut -c1-200)]}"
    [ $rc = 0 ] && [ -f $o ] && $d $o
  done
}
secs() { readelf -SW "$1" | awk '/^  \[/ && !/Nr\]/' | sed 's/^ *//'; }
grp()  { readelf -gW "$1" | sed '/^$/d'; readelf -SW "$1" | awk '/ GROUP /'; }
rels() { readelf -rW "$1" | awk 'NF && !/^ Offset/'; }
ssym() { readelf -sW "$1" | awk 'NR>3 && $4=="SECTION"'; }

echo "== 1 maps"
cat > m1.c <<'X'
static int s1(int x) { return x + 1; }
int g1(int x) { return s1(x) * 2; }
int d1 = 5; const char *str1 = "hello";
X
cat > m2.c <<'X'
int g1(int);
int g2(int x) { return g1(x) + 7; }
int d2 = 9; const char *str2 = "hello"; int b2;
X
CC aarch64 -O1 -c m1.c -o m1.o; CC aarch64 -O1 -c m2.c -o m2.o
for k in bfd lld mold; do
  R aarch64 $k maps.$k.o -Map=maps.$k.map m1.o m2.o; echo "--- maps $k rc=$?"
  case $k in
    bfd) awk '/^Linker script and memory map/{p=1} p && /^ ?\.(text|data|rodata|bss)/' maps.$k.map | head -14;;
    *) grep -E '\.(text|data|rodata|bss)' maps.$k.map | head -14;;
  esac
done

echo "== 2 rel386"
cat > r1.s <<'X'
  .text
  nop
  nop
  nop
  .data
  .long 1
  .byte 7
X
cat > r2.s <<'X'
  .text
  .globl f2
f2:
  call gsym
  call lfun
lfun:
  ret
  .data
  .balign 4
  .long 2
lab:
  .long 3
  .long lab
  .long .text+1
  .long gsym+4
  .long lab-.
  .word lab
X
CC i386 -c r1.s -o r1.o; CC i386 -c r2.s -o r2.o
echo "inputs:"; rels r2.o; readelf -x .data r2.o | sed -n '3,$p'; readelf -SW r1.o r2.o | grep -E ' \.(data|text) '
each rel386 i386 'eval rels' r1.o r2.o
for k in bfd lld mold; do echo "data $k:"; readelf -x .data rel386.i386.$k.o 2>/dev/null | sed -n '3,$p'; objdump -d rel386.i386.$k.o 2>/dev/null | grep -E 'call' ; done

echo "== 3 groups"
cat > g1.cc <<'X'
inline int shared_inline(int x) { return x * 3; }
int use1(int x) { return shared_inline(x); }
X
cat > g2.cc <<'X'
inline int shared_inline(int x) { return x * 3; }
int use2(int x) { return shared_inline(x) + 1; }
X
cat > g3.s <<'X'
  .section .text.plain,"axG",%progbits,plaingrp
pg:
  bl use1
  ret
X
CC aarch64 -O1 -c g1.cc -o g1.o; CC aarch64 -O1 -c g2.cc -o g2.o; CC aarch64 -c g3.s -o g3.o
echo "input g3:"; grp g3.o
each groups aarch64 grp g1.o g2.o g3.o g3.o
for k in bfd lld mold; do echo "rela flags $k:"; readelf -SW groups.aarch64.$k.o 2>/dev/null | grep -E 'rela\.text\.(plain|_Z)|text\._Z|text\.plain'; echo "signature symbols $k:"; readelf -sW groups.aarch64.$k.o 2>/dev/null | grep -E 'plaingrp|shared_inline'; done
for k in bfd lld mold; do echo "group words $k:"; [ -f groups.aarch64.$k.o ] && python3 -I "$HERE/grpwords.py" groups.aarch64.$k.o; done

echo "== 4 empty"
cat > e1.c <<'X'
int e1(void) { return 1; }
X
cat > e2.s <<'X'
  .section .data.empty,"aw",%progbits
  .section .bss.empty,"aw",%nobits
  .text
  ret
X
CC aarch64 -O1 -c e1.c -o e1.o; CC aarch64 -c e2.s -o e2.o
echo "input e1:"; secs e1.o
each empty aarch64 'eval secs' e1.o e2.o
for k in bfd lld mold; do echo "STT_SECTION $k:"; ssym empty.aarch64.$k.o 2>/dev/null; done

echo "== 5 noneonly"
cat > n1.s <<'X'
  .text
  .globl n1
n1:
  ret
  .reloc 0, R_AARCH64_NONE, n1
  .reloc 0, R_AARCH64_NONE, other
X
cat > n2.s <<'X'
  .text
  .globl n2
n2:
  ret
X
CC aarch64 -c n1.s -o n1.o; CC aarch64 -c n2.s -o n2.o
echo "input n1:"; rels n1.o
each noneonly aarch64 'eval secs' n1.o n2.o
for k in bfd lld mold; do echo "rels $k:"; rels noneonly.aarch64.$k.o 2>/dev/null; readelf -sW noneonly.aarch64.$k.o 2>/dev/null | grep -w other; done

echo "== 6 property"
cat > p.c <<'X'
int PNAME(int x) { return x + 1; }
X
CC aarch64 -O1 -mbranch-protection=standard -DPNAME=pa -c p.c -o pa.o
CC aarch64 -O1 -mbranch-protection=standard -DPNAME=pb -c p.c -o pb.o
CC aarch64 -O1 -mbranch-protection=pac-ret -DPNAME=pc -c p.c -o pc.o
CC aarch64 -O1 -mbranch-protection=none -DPNAME=pd -c p.c -o pd.o
for f in pa pc pd; do echo "input $f: $(readelf -nW $f.o | grep -E 'Properties|feature' | tr -s ' ' | tr '\n' ' ')"; done
for set in "pa.o pb.o" "pa.o pc.o" "pa.o pd.o" "pd.o pa.o"; do
  for k in bfd lld mold; do
    R aarch64 $k prop.o $set; echo "aarch64 [$set] $k rc=$?: $(readelf -SW prop.o | grep -c 'note.gnu.property') section(s); $(readelf -nW prop.o | grep -E 'Properties|feature' | tr -s ' ' | tr '\n' ' ')"; rm -f prop.o
  done
done
CC x86_64 -O1 -fcf-protection=full -DPNAME=xa -c p.c -o xa.o
CC x86_64 -O1 -fcf-protection=full -DPNAME=xb -c p.c -o xb.o
CC x86_64 -O1 -fcf-protection=none -DPNAME=xc -c p.c -o xc.o
CC x86_64 -O1 -fcf-protection=none -Wa,-mx86-used-note=yes -DPNAME=xd -c p.c -o xd.o
for f in xa xc xd; do echo "input $f: $(readelf -nW $f.o | grep -E 'Properties|x86' | tr -s ' ' | tr '\n' ' ')"; done
for set in "xa.o xb.o" "xa.o xc.o" "xa.o xd.o" "xd.o xc.o"; do
  for k in bfd lld mold; do
    R x86_64 $k prop.o $set; echo "x86_64 [$set] $k rc=$?: $(readelf -SW prop.o | grep -c 'note.gnu.property') section(s); $(readelf -nW prop.o | grep -E 'Properties|x86' | tr -s ' ' | tr '\n' ' ')"; rm -f prop.o
  done
done

echo "== 7 retain"
cat > k1.s <<'X'
  .section .text.k,"axR",%progbits
  .globl k1
k1:
  ret
X
cat > k2.s <<'X'
  .section .text.k,"ax",%progbits
  .globl k2
k2:
  ret
X
CC aarch64 -c k1.s -o k1.o; CC aarch64 -c k2.s -o k2.o
each retain aarch64 'eval secs' k1.o k2.o

echo "== 8 dropdebug"
CC aarch64 -O0 -g -c g1.cc -o d1.o; CC aarch64 -O0 -g -c g2.cc -o d2.o
echo "input d2 records into its own copy (debug_info):"
readelf -rW d2.o | awk '/Relocation section .\.rela\.debug_info/{p=1;next} /Relocation section/{p=0} p && /shared_inline/' | head -4
for k in bfd lld mold; do
  R aarch64 $k dd.$k.o d1.o d2.o; echo "--- dropdebug $k rc=$?"
  echo " .rela.debug_info by type and symbol (dropped copy's records):"
  readelf -rW dd.$k.o | awk '/Relocation section .\.rela\.debug_info/{p=1;next} /Relocation section/{p=0} p && NF>=3 {print $3, ($3 ~ /NONE/ ? "-" : $5)}' | sort | uniq -c | sort -rn
  echo " .rela.eh_frame:"; readelf -rW dd.$k.o | awk '/Relocation section .\.rela\.eh_frame/{p=1;next} /Relocation section/{p=0} p && NF>=3 {print "  ", $1, $3, $5, $6, $7}'
  echo " .eh_frame size: $(readelf -SW dd.$k.o | sed 's/^ *\[ *[0-9]*\] //' | awk '$1==".eh_frame" {print $5}') (inputs: $(readelf -SW d1.o | sed 's/^ *\[ *[0-9]*\] //' | awk '$1==".eh_frame" {print $5}') + $(readelf -SW d2.o | sed 's/^ *\[ *[0-9]*\] //' | awk '$1==".eh_frame" {print $5}'))"
done
echo " lld: the field under each NONE record of .rela.debug_info (offset: 4 bytes of .debug_info there)"
python3 -I - dd.lld.o <<'PY'
import subprocess, sys, struct
f = sys.argv[1]; b = open(f, 'rb').read()
out = subprocess.run(['readelf', '-SW', f], capture_output=True, text=True).stdout
off = None
for l in out.splitlines():
    if '] .debug_info ' in l:
        p = l.split(']')[1].split(); off = int(p[3], 16)
rl = subprocess.run(['readelf', '-rW', f], capture_output=True, text=True).stdout
p = False; n = 0
for l in rl.splitlines():
    if l.startswith('Relocation section'):
        p = "'.rela.debug_info'" in l; continue
    if p and 'R_AARCH64_NONE' in l and n < 6:
        o = int(l.split()[0], 16); print('  %#x: %s' % (o, b[off + o: off + o + 8].hex())); n += 1
PY

echo "== 9 linkorder"
cat > l.c <<'X'
int LNAME(int x) { return x + 2; }
X
CC aarch64 -O1 -ffunction-sections -fpatchable-function-entry=2 -DLNAME=lf -c l.c -o l1.o
CC aarch64 -O1 -ffunction-sections -fpatchable-function-entry=2 -DLNAME=lg -c l.c -o l2.o
echo "input l1:"; secs l1.o | grep -E 'patchable|text'
each linkorder aarch64 'eval secs' l1.o l2.o

echo "== 10 entsize"
cat > z1.s <<'X'
  .section .rodata.cst,"aM",%progbits,4
  .word 1
X
cat > z2.s <<'X'
  .section .rodata.cst,"aM",%progbits,8
  .xword 2
X
CC aarch64 -c z1.s -o z1.o; CC aarch64 -c z2.s -o z2.o
each entsize aarch64 'eval secs' z1.o z2.o

echo "== 11 nobits"
cat > b1.s <<'X'
  .section .foo,"aw",%progbits
  .word 1
X
cat > b2.s <<'X'
  .section .foo,"aw",%nobits
  .zero 8
X
CC aarch64 -c b1.s -o b1.o; CC aarch64 -c b2.s -o b2.o
each nobits aarch64 'eval secs' b1.o b2.o

echo "== 12 symsec"
for k in bfd lld mold; do echo "--- STT_SECTION values in maps.$k.o"; ssym maps.$k.o | awk '{print $2, $7}' | tr '\n' ' '; echo; done
echo "== 13 cgprofile"
cat > cg.s <<'X'
  .text
  .globl cga, cgb
cga: ret
cgb: ret
  .cg_profile cga, cgb, 10
X
clang --target=aarch64-linux-gnu -c cg.s -o cg.o
echo "input:"; secs cg.o | grep -E 'call-graph|REL'; rels cg.o
for k in bfd lld mold; do
  R aarch64 $k cgprofile.$k.o cg.o; echo "--- cgprofile $k rc=$?"
  secs cgprofile.$k.o | grep -E 'call-graph|REL'; rels cgprofile.$k.o | grep -A3 call-graph
done

echo "== 14 dupmaps"
cat > dm.s <<'X'
  .section .text.r,"axR",%progbits
  ret
  .section .text.r,"ax",%progbits
  ret
X
CC aarch64 -c dm.s -o dm.o
for k in bfd lld mold; do
  R aarch64 $k dupmaps.$k.o -Map=dupmaps.$k.map dm.o; echo "--- dupmaps $k rc=$?"
  secs dupmaps.$k.o | grep text.r
  grep -n 'text\.r' dupmaps.$k.map | head -8
done
echo "rl0 done"

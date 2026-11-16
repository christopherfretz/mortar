#!/bin/bash
# escape/run.sh: the B-1 opt-in escape candidate (export the bound definition
# STV_PROTECTED): what the references and glibc 2.41 do when an executable
# reaches a protected data definition, per host kind (-no-pie: COPY; -pie)
# and with/without GNU_PROPERTY_1_NEEDED_INDIRECT_EXTERN_ACCESS (W41, x86-64:
# -mno-direct-extern-access).  a64 native, x86-64 under qemu.
ulimit -c 0; cd "$(dirname "$0")"; W=work.run; [ -d $W ] && find $W -mindepth 1 -delete; mkdir -p $W; cd $W  # fresh outputs (mold writes in place)
R=/Volumes/AgentWork/mortar-helper/.build/review-110-adv
for arch in a64 x64; do
 if [ $arch = a64 ]; then CC=gcc; RUN=""; else CC=x86_64-linux-gnu-gcc; RUN="qemu-x86_64 -L /usr/x86_64-linux-gnu"; fi
 for prop in none iea; do
  [ $prop = iea ] && [ $arch = a64 ] && continue
  PF=""; [ $prop = iea ] && PF="-mno-direct-extern-access"
  $CC -O2 -fPIE $PF -c ../lib.c -o lib.$arch.$prop.o
  for hk in nopie pie; do
   if [ $hk = nopie ]; then HF="-fno-pie"; HL="-no-pie"; else HF="-fPIE"; HL="-pie"; fi
   $CC -O2 $HF -c ../host.c -o host.$arch.$hk.o
   for ld in bfd lld mold; do
    case $ld in bfd) L="-fuse-ld=bfd";; lld) L="-B$R/lld22 -fuse-ld=lld";; mold) L="-fuse-ld=mold";; esac
    t=$arch.$prop.$hk.$ld; so=lib$t.so
    if ! $CC $L -shared lib.$arch.$prop.o -o $so 2>$t.soerr; then echo "$t: -shared FAIL: $(grep -m1 -i error $t.soerr)"; continue; fi
    if ! $CC $L $HL host.$arch.$hk.o ./$so -o host.$t -Wl,-rpath,'$ORIGIN' 2>$t.err; then echo "$t: exe link FAIL: $(grep -m1 -iE 'error|protected' $t.err)"; continue; fi
    cr=$(readelf -rW host.$t | grep -c COPY)
    echo "$t: copies=$cr warn=$(grep -m1 -i protected $t.err | cut -c1-80) run: $($RUN ./host.$t 2>&1 | head -1)"
   done
  done
 done
done

#!/bin/bash
# meta/run.sh WORK: kiln++ units through each reference's -r, then mortar's
# final link under --odr=strict, direct vs through the -r output.
ulimit -c 0
H=$(cd "$(dirname "$0")" && pwd); W=$1; mkdir -p "$W"; cd "$W" || exit 1
K=/Volumes/AgentWork/mortar-helper/.build/target-kiln-next/release/kiln++
M=/Volumes/AgentWork/mortar-helper/.build/target-wp-154/mortar-3ad9867a
mkdir -p mbin; ln -sf "$M" mbin/ld
for f in m1 m2 m3 mm; do $K -O1 -c -fmason-metadata=detail -o $f.o "$H/$f.cc" || echo "kiln++ $f failed"; done
python3 "$H/meta.py" m1.o m2.o m3.o
for L in bfd lld mold; do
  case $L in bfd) LD=ld.bfd;; lld) LD=ld.lld-22;; mold) LD=mold;; esac
  $LD -r -o r.$L.o m1.o m2.o m3.o || echo "$L -r failed"
  python3 "$H/meta.py" r.$L.o
done
link() { out=$1; shift; g++ -static -B mbin/ -Wl,--odr=strict -o $out "$@" > $out.log 2>&1; echo "  rc=$? $(grep -c 'odr-' $out.log) odr lines; $(grep -o 'odr-[a-z-]*' $out.log | sort | uniq -c | tr '\n' ' ')"; }
echo "direct:"; link d mm.o m1.o m2.o m3.o
for L in bfd lld mold; do echo "via $L -r:"; link v.$L mm.o r.$L.o; done
diff <(sed 's/r\.[a-z]*\.o/X/g; s/m[123]\.o/X/g' d.log) <(sed 's/r\.[a-z]*\.o/X/g; s/m[123]\.o/X/g' v.bfd.log) > /dev/null && echo "bfd diagnostics equal modulo file names" || { echo "bfd diagnostics differ:"; head -20 d.log; echo ---; head -20 v.bfd.log; }
# the legal case: m4.cc is m2.cc with m1.cc's S; no finding, the program runs
$K -O1 -c -fmason-metadata=detail -o m4.o "$H/m4.cc"
for L in bfd lld mold; do case $L in bfd) LD=ld.bfd;; lld) LD=ld.lld-22;; mold) LD=mold;; esac
  $LD -r -o ok.$L.o m1.o m4.o m3.o; g++ -static -B mbin/ -Wl,--odr=strict -o ok.$L mm.o ok.$L.o > ok.$L.log 2>&1
  echo "legal via $L -r: rc=$? run: $(./ok.$L) diagnostics: $(wc -l < ok.$L.log)"; done
# hod's objects (kiln -fkiln-backend=hod, the workspace's hod release build)
export PATH=/Volumes/AgentWork/mortar-helper/.build/target-hod/release:$PATH
$K -O0 -fkiln-backend=hod -c -fmason-metadata=detail -o h1.o "$H/m1.cc" && $K -O0 -fkiln-backend=hod -c -fmason-metadata=detail -o h2.o "$H/m2.cc" && {
  python3 "$H/meta.py" h1.o h2.o; ld.bfd -r -o hr.o h1.o h2.o; python3 "$H/meta.py" hr.o
  g++ -static -B mbin/ -Wl,--odr=strict -o hv mm.o hr.o m3.o > hv.log 2>&1; echo "hod via bfd -r: rc=$? $(head -1 hv.log)"; }
echo "meta done"

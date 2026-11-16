#!/bin/bash
# Version-script corner cases: '#' comments; a node matching nothing (is .gnu.version_d written?);
# two --version-script files; .symver foo@V1 vs local: in node V1 vs in another node.
ulimit -c 0; cd "$(dirname "$0")"; W=work.run; [ -d $W ] && find $W -mindepth 1 -delete; mkdir -p $W; cd $W  # fresh outputs (mold writes in place)
R=/Volumes/AgentWork/mortar-helper/.build/review-110-adv
gcc -fPIC -c ../lib.c -o lib.o; gcc -fPIC -c ../lib2.c -o lib2.o
printf '# a libxml2-style comment\nV1 {\n  global:\n# section\n    foo; bar;\n  local: *;\n};\n' > hash.map
printf 'V1 { global: nosuch; local: *; };\n' > empty.map
printf 'V1 { global: foo; };\n' > a.map; printf 'V2 { global: bar; } V1;\n' > b.map
printf 'V1 { global: bar; local: *; };\n' > sv_v1local.map
printf 'V1 { global: bar; }; V2 { local: *; } V1;\n' > sv_v2local.map
printf 'V1 { global: bar; }; V2 { global: foo; local: *; } V1;\n' > sv_v2local_foo.map
for ld in bfd lld mold; do
 case $ld in bfd) L="-fuse-ld=bfd";; lld) L="-B$R/lld22 -fuse-ld=lld";; mold) L="-fuse-ld=mold";; esac
 t() { n=$1; o=$2; shift 2; if gcc $L -shared -nostdlib $o "$@" -o $n.$ld.so 2>$n.$ld.err; then
   echo "$ld $n: verdefs=[$(readelf -VW $n.$ld.so | awk '/Name:/ && /Flags/ {print $NF}' | tr '\n' ' ')] dynsyms=[$(readelf --dyn-syms -W $n.$ld.so | awk 'NR>3 && $7!="UND" {print $8}' | sort | tr '\n' ' ')] $(sed -n 1p $n.$ld.err | cut -c1-100)"
   else echo "$ld $n: ERROR $(head -1 $n.$ld.err | cut -c1-140)"; fi; }
 t hash lib2.o -Wl,--version-script=hash.map
 t empty lib2.o -Wl,--version-script=empty.map
 t two lib2.o -Wl,--version-script=a.map -Wl,--version-script=b.map
 t sv_v1local lib.o -Wl,--version-script=sv_v1local.map
 t sv_v2local lib.o -Wl,--version-script=sv_v2local.map
 t sv_v2local_foo lib.o -Wl,--version-script=sv_v2local_foo.map
done
printf 'V1 { global: foo; # inline comment\n bar; local: *; };\n' > inline.map
printf 'V2 { global: bar; } V1;\nV1 { global: foo; local: *; };\n' > fwdparent.map
printf 'V1 { global: foo; local: *; };\nV2 { global: bar; } V9;\n' > noparent.map
for ld in bfd lld mold; do
 case $ld in bfd) L="-fuse-ld=bfd";; lld) L="-B$R/lld22 -fuse-ld=lld";; mold) L="-fuse-ld=mold";; esac
 for n in inline fwdparent noparent; do if gcc $L -shared -nostdlib lib2.o -Wl,--version-script=$n.map -o $n.$ld.so 2>$n.$ld.err; then
   echo "$ld $n: verdefs=[$(readelf -VW $n.$ld.so | awk '/Name:/ && /Flags/ {print $NF}' | tr '\n' ' ')] dynsyms=[$(readelf --dyn-syms -W $n.$ld.so | awk 'NR>3 && $7!="UND" {print $8}' | sort | tr '\n' ' ')] $(sed -n 1p $n.$ld.err | cut -c1-100)"
   else echo "$ld $n: ERROR $(head -1 $n.$ld.err | cut -c1-140)"; fi; done
done
# WP-110 second pass additions: node-scoped hiding of a .symver definition
# (sv_* above), with the default version foo@@V2 too; a local: of the
# version's own node by exact name.
printf 'V1 { global: bar; local: foo; };\nV2 { global: foo; } V1;\n' > sv_v1exact.map
printf 'V1 { global: bar; };\nV2 { global: foo; local: *; } V1;\n' > sv_v2star.map
for ld in bfd lld mold; do
 case $ld in bfd) L="-fuse-ld=bfd";; lld) L="-B$R/lld22 -fuse-ld=lld";; mold) L="-fuse-ld=mold";; esac
 for n in sv_v1exact sv_v2star; do if gcc $L -shared -nostdlib lib.o -Wl,--version-script=$n.map -o $n.$ld.so 2>$n.$ld.err; then
   echo "$ld $n: dynsyms=[$(readelf --dyn-syms -W $n.$ld.so | awk 'NR>3 && $7!="UND" {print $8}' | sort | tr '\n' ' ')] $(sed -n 1p $n.$ld.err | cut -c1-100)"
   else echo "$ld $n: ERROR $(head -1 $n.$ld.err | cut -c1-140)"; fi; done
done
# a node name used twice; an anonymous node with a named one
printf 'V1 { global: foo; };\nV1 { global: bar; };\n' > dupnode.map
printf '{ global: foo; };\nV1 { global: bar; };\n' > anon_named.map
printf 'V1 { global: foo; };\n{ global: bar; };\n' > named_anon.map
for ld in bfd lld mold; do
 case $ld in bfd) L="-fuse-ld=bfd";; lld) L="-B$R/lld22 -fuse-ld=lld";; mold) L="-fuse-ld=mold";; esac
 for n in dupnode anon_named named_anon; do if gcc $L -shared -nostdlib lib2.o -Wl,--version-script=$n.map -o $n.$ld.so 2>$n.$ld.err; then
   echo "$ld $n: dynsyms=[$(readelf --dyn-syms -W $n.$ld.so | awk 'NR>3 && $7!="UND" {print $8}' | sort | tr '\n' ' ')] $(sed -n 1p $n.$ld.err | cut -c1-100)"
   else echo "$ld $n: ERROR $(head -1 $n.$ld.err | cut -c1-140)"; fi; done
done

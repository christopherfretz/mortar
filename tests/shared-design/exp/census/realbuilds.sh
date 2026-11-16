#!/bin/bash
# realbuilds.sh WORK MORTAR: part 2 of the census area.
# Rebuild zlib 1.3.1's libz.so, lua 5.4.7's test C modules and googletest
# 1.15.2's libgtest_dll.so from the downloaded sources, link each shared
# object with GNU ld, lld 22 and mold (by the gcc/g++ driver), and run the
# projects' own programs against each.  AArch64 (native) only.
# Every host program is linked by GNU ld (the default driver line).
set -u
ulimit -c 0   # gtest death tests abort: no core files in WORK
WORK=$(realpath -m "$1"); MORTAR=$2
HERE=$(cd "$(dirname "$0")" && pwd)
DL=/Volumes/AgentWork/mortar-helper/.build/real-builds/downloads
mkdir -p "$WORK"; cd "$WORK" || exit 1
# gcc finds ld.lld in its -B directories first: one directory per lld version
mkdir -p "$WORK/lld22bin" "$WORK/lld19bin" "$WORK/tmp"; ln -s /usr/bin/ld.lld-22 "$WORK/lld22bin/ld.lld"; ln -s /usr/bin/ld.lld-19 "$WORK/lld19bin/ld.lld"
export TMPDIR=$WORK/tmp
LINKERS="bfd lld lld19 mold"
lflag() { case $1 in bfd) echo "-fuse-ld=bfd";; lld) echo "-B$WORK/lld22bin/ -fuse-ld=lld";; lld19) echo "-B$WORK/lld19bin/ -fuse-ld=lld";; mold) echo "-fuse-ld=mold";; esac; }
say() { echo "=== $*"; }
{ gcc --version | head -1; ld.bfd --version | head -1; ld.lld-22 --version; ld.lld-19 --version; mold --version; readelf --version | head -1; } > versions.log 2>&1

# --- fetch sources: each archive into its own new empty directory
for a in zlib-1.3.1.tar.gz lua-5.4.7.tar.gz lua-5.4.7-tests.tar.gz googletest-1.15.2.tar.gz; do
  d=src/${a%%.tar.gz}; mkdir -p "$d"; tar -xzf "$DL/$a" -C "$d"
done
Z=$WORK/src/zlib-1.3.1/zlib-1.3.1
L=$WORK/src/lua-5.4.7/lua-5.4.7
LT=$WORK/src/lua-5.4.7-tests/lua-5.4.7-tests
GT=$WORK/src/googletest-1.15.2/googletest-1.15.2/googletest

dynfacts() { # file -> log of the facts compared across linkers
  local f=$1
  echo "--- readelf -d $f"; readelf -dW "$f" | sed 1,2d
  echo "--- readelf -V $f"; readelf -VW "$f" | sed -n '/Version definition/,$p' | sed 's/^ *0x[0-9a-f]*: *//; s/Addr: 0x[0-9a-f]*  Offset: 0x[0-9a-f]*  Link: [0-9]* (.*)//'
}
dynsyms() { # file -> sorted "BIND TYPE VIS DEF|UND name" with value dropped ([VARIANT_PCS] etc. skipped)
  # readelf 2.44 appends " (N)" (the verneed index) to undefined versioned names and may insert
  # "[VARIANT_PCS]"-style tokens after the visibility: strip both, then Ndx is $7 and the name $8
  readelf --dyn-syms -W "$1" | sed -E 's/ \([0-9]+\)$//; s/ \[[A-Z_]+[^]]*\]//g; s/<OS specific>: 10/UNIQUE(osabi-SYSV)/' | awk 'NR>3 && $1 ~ /:$/ && NF>=8 {nd=($7=="UND")?"UND":"DEF"; print $5, $4, $6, nd, $8}' | sort -k5
}

# ======================================================== (a) zlib
say "zlib: compile (flags reproduced by hand from the survey's configure log, results/zlib.gcc.control/log)"
ZO=$WORK/zlib/objs; mkdir -p "$ZO"
ZSRC="adler32 crc32 deflate infback inffast inflate inftrees trees zutil compress uncompr gzclose gzlib gzread gzwrite"
for s in $ZSRC; do
  gcc -g -O2 -fPIC -D_LARGEFILE64_SOURCE=1 -DHAVE_HIDDEN -DPIC -I"$Z" -c -o "$ZO/$s.o" "$Z/$s.c" || echo "COMPILE FAIL $s"
done
gcc -g -O2 -D_LARGEFILE64_SOURCE=1 -DHAVE_HIDDEN -I"$Z" -c -o "$WORK/zlib/example.o" "$Z/test/example.c"
gcc -g -O2 -D_LARGEFILE64_SOURCE=1 -DHAVE_HIDDEN -I"$Z" -D_FILE_OFFSET_BITS=64 -c -o "$WORK/zlib/example64.o" "$Z/test/example.c"
gcc -g -O2 -D_LARGEFILE64_SOURCE=1 -DHAVE_HIDDEN -I"$Z" -c -o "$WORK/zlib/minigzip.o" "$Z/test/minigzip.c"
cp "$Z/zlib.map" "$WORK/zlib/zlib.map"
say "zlib.map (whole)"; cat "$WORK/zlib/zlib.map"
say "zlib.map constructs"; python3 -I "$HERE/vscript.py" "$WORK/zlib/zlib.map"
ZOBJS=$(for s in $ZSRC; do printf '%s ' "$ZO/$s.o"; done)
for k in $LINKERS; do
  D=$WORK/zlib/$k; mkdir -p "$D"
  say "zlib link $k"
  # the Makefile line: $(LDSHARED) $(SFLAGS) -o $@ $(PIC_OBJS) $(LDSHAREDLIBC) $(LDFLAGS)
  # LDSHARED = gcc -shared -Wl,-soname,libz.so.1,--version-script,zlib.map ; LDSHAREDLIBC = -lc
  gcc -shared -Wl,-soname,libz.so.1,--version-script,"$WORK/zlib/zlib.map" $(lflag $k) -g -O2 -fPIC \
      -D_LARGEFILE64_SOURCE=1 -DHAVE_HIDDEN -o "$D/libz.so.1.3.1" $ZOBJS -lc; echo "link rc=$?"
  ln -s libz.so.1.3.1 "$D/libz.so.1"; ln -s libz.so.1.3.1 "$D/libz.so"
  dynfacts "$D/libz.so.1.3.1" > "$D/facts.log"; dynsyms "$D/libz.so.1.3.1" > "$D/dynsyms.txt"
  readelf -sW "$D/libz.so.1.3.1" | awk '$8 ~ /^_/ && $5!="LOCAL" {print "symtab non-local _*:", $5, $6, $7, $8}' > "$D/underscore.txt"
  readelf -lW "$D/libz.so.1.3.1" | grep -E '^\s+[A-Z_]+ ' > "$D/phdrs.txt"
  eu-elflint --gnu-ld "$D/libz.so.1.3.1" > "$D/elflint.txt" 2>&1; echo "elflint rc=$? $(head -3 "$D/elflint.txt" | tr '\n' ' ')"
  for e in example example64 minigzip; do
    gcc -g -O2 -o "$D/$e" "$WORK/zlib/$e.o" -L"$D" -lz; echo "$e (GNU ld) link rc=$?"
  done
  readelf -VW "$D/example64" | sed -n '/Version needs/,$p' | grep -E 'Name: ZLIB|File: libz' > "$D/example64.verneed"
done
say "zlib configure's shared-library probe (configure lines 438-446: ztest.c = extern int getchar(); int hello() {return getchar();}), per linker"
printf 'extern int getchar();\nint hello() {return getchar();}\n' > "$WORK/zlib/ztest.c"
for cc in gcc clang-22; do
  $cc -O2 -fPIC -w -c "$WORK/zlib/ztest.c" -o "$WORK/zlib/ztest.$cc.o"
  for k in $LINKERS; do
    if [ $cc = gcc ]; then lf=$(lflag $k); else case $k in bfd) lf="-fuse-ld=bfd";; lld) lf="-fuse-ld=lld --ld-path=/usr/bin/ld.lld-22";; lld19) lf="-fuse-ld=lld --ld-path=/usr/bin/ld.lld-19";; mold) lf="-fuse-ld=mold";; esac; fi
    msg=$($cc -shared -Wl,-soname,libz.so.1,--version-script,"$WORK/zlib/zlib.map" $lf -O2 -fPIC -o "$WORK/zlib/ztest.$cc.$k.so" "$WORK/zlib/ztest.$cc.o" 2>&1); rc=$?
    echo "probe $cc $k rc=$rc :: $(echo "$msg" | head -3 | tr '\n' '|' | cut -c1-300) ($(echo "$msg" | grep -c .) diagnostic lines)"
  done
  for k in lld mold; do
    if [ $cc = gcc ]; then lf=$(lflag $k); else case $k in lld) lf="-fuse-ld=lld --ld-path=/usr/bin/ld.lld-22";; mold) lf="-fuse-ld=mold";; esac; fi
    msg=$($cc -shared -Wl,-soname,libz.so.1,--version-script,"$WORK/zlib/zlib.map" -Wl,--undefined-version $lf -o "$WORK/zlib/ztest.$cc.$k.uv.so" "$WORK/zlib/ztest.$cc.o" 2>&1); echo "probe $cc $k -Wl,--undefined-version rc=$? :: $(echo "$msg" | head -2 | tr '\n' '|' | cut -c1-200)"
  done
done
msg=$(gcc -shared -Wl,-soname,libz.so.1,--version-script,"$WORK/zlib/zlib.map" -Wl,--no-undefined-version -fuse-ld=bfd -o "$WORK/zlib/ztest.bfd.nuv.so" "$WORK/zlib/ztest.gcc.o" 2>&1); echo "probe gcc bfd -Wl,--no-undefined-version rc=$? :: $(echo "$msg" | head -2 | tr '\n' '|' | cut -c1-200)"
say "zlib dynsym sets compared (name, bind, type, vis, def/und); version suffix included"
for k in $LINKERS; do echo "$k: $(wc -l < "$WORK/zlib/$k/dynsyms.txt") dynsyms, $(grep -c ' DEF ' "$WORK/zlib/$k/dynsyms.txt") defined"; done
diff "$WORK/zlib/bfd/dynsyms.txt" "$WORK/zlib/lld/dynsyms.txt" > zlib-bfd-lld.diff; echo "bfd vs lld diff rc=$?"; cat zlib-bfd-lld.diff
diff "$WORK/zlib/bfd/dynsyms.txt" "$WORK/zlib/lld19/dynsyms.txt" > zlib-bfd-lld19.diff; echo "bfd vs lld19 diff rc=$?"; cat zlib-bfd-lld19.diff
diff "$WORK/zlib/bfd/dynsyms.txt" "$WORK/zlib/mold/dynsyms.txt" > zlib-bfd-mold.diff; echo "bfd vs mold diff rc=$?"; cat zlib-bfd-mold.diff
for k in $LINKERS; do say "zlib facts $k"; cat "$WORK/zlib/$k/facts.log"; cat "$WORK/zlib/$k/underscore.txt"; cat "$WORK/zlib/$k/phdrs.txt"; echo "--- example64 verneed"; cat "$WORK/zlib/$k/example64.verneed"; done
say "zlib dynsym versions per linker (unversioned vs versioned counts)"
for k in $LINKERS; do awk -v k=$k '$4=="DEF"{ if ($5 ~ /@/) v++; else u++ } END {print k": versioned", v+0, "unversioned", u+0}' "$WORK/zlib/$k/dynsyms.txt"; done
say "zlib run matrix: example built against <lib>, run with LD_LIBRARY_PATH=<dir>"
for b in $LINKERS; do for r in $LINKERS; do for e in example example64; do
  out=$(cd "$WORK/zlib" && LD_LIBRARY_PATH=$WORK/zlib/$r "$WORK/zlib/$b/$e" "$WORK/zlib/$b.$r.$e.gz" 2>&1); rc=$?
  echo "built-vs=$b run-with=$r $e rc=$rc $([ $rc = 0 ] && echo OK || echo FAIL) :: $(echo "$out" | tr '\n' '|' | cut -c1-90)…$(echo "$out" | tail -1)"
done
  rt=$(echo hello world | LD_LIBRARY_PATH=$WORK/zlib/$r "$WORK/zlib/$b/minigzip" | LD_LIBRARY_PATH=$WORK/zlib/$r "$WORK/zlib/$b/minigzip" -d)
  echo "built-vs=$b run-with=$r minigzip|minigzip -d: '$rt' $([ "$rt" = 'hello world' ] && echo OK || echo FAIL)"
done; done
system=/lib/aarch64-linux-gnu/libz.so.1
if [ -e $system ]; then say "system libz for comparison: $(readlink -f $system)"; dynsym_sys=$(dynsyms $system | awk '$4=="DEF"' | wc -l); echo "defined dynsyms: $dynsym_sys"; dynsyms $system | awk '$4=="DEF" && $5 !~ /@/' | head; fi

# ======================================================== (b) lua
say "lua: core by hand from src/Makefile (CC=gcc -std=gnu99, CFLAGS=-O2 -Wall -Wextra -DLUA_COMPAT_5_3 -DLUA_USE_LINUX; linux-noreadline SYSLIBS=-Wl,-E -ldl)"
LO=$WORK/lua/obj; mkdir -p "$LO"
CORE="lapi lcode lctype ldebug ldo ldump lfunc lgc llex lmem lobject lopcodes lparser lstate lstring ltable ltm lundump lvm lzio lauxlib lbaselib lcorolib ldblib liolib lmathlib loadlib loslib lstrlib ltablib lutf8lib linit"
for s in $CORE lua; do gcc -std=gnu99 -O2 -Wall -Wextra -DLUA_COMPAT_5_3 -DLUA_USE_LINUX -c -o "$LO/$s.o" "$L/src/$s.c" 2>>lua-compile.log || echo "COMPILE FAIL $s"; done
ar rcu "$WORK/lua/liblua.a" $(for s in $CORE; do printf '%s ' "$LO/$s.o"; done) 2>/dev/null; ranlib "$WORK/lua/liblua.a"
gcc -std=gnu99 -o "$WORK/lua/lua" "$LO/lua.o" "$WORK/lua/liblua.a" -lm -Wl,-E -ldl; echo "lua (GNU ld, -Wl,-E) rc=$?"
echo "lua exports: $(readelf --dyn-syms -W "$WORK/lua/lua" | awk '$7!="UND" && $8 ~ /^luaL?_/' | wc -l) defined lua_*/luaL_* in .dynsym"
say "lua test modules: compile once (-Wall -std=gnu99 -O2 -I<lua src> -fPIC, as libs/makefile), link per linker"
MODS="lib1 lib11 lib2 lib21 lib22"
for m in $MODS; do gcc -Wall -std=gnu99 -O2 -I"$L/src" -fPIC -c -o "$WORK/lua/$m.o" "$LT/libs/$m.c" || echo "COMPILE FAIL $m"; done
for k in $LINKERS; do
  T=$WORK/lua/tests-$k; mkdir -p "$T"; cp -r "$LT/." "$T/"
  for m in $MODS; do
    out=$m.so; [ $m = lib22 ] && out=lib2-v2.so
    gcc -shared $(lflag $k) -o "$T/libs/$out" "$WORK/lua/$m.o"; echo "$k $out link rc=$?"
  done
  say "lua $k: undefined lua symbols / NEEDED per module"
  for f in lib1 lib11 lib2 lib21 lib2-v2; do
    und=$(readelf --dyn-syms -W "$T/libs/$f.so" | awk '$7=="UND" && $8 ~ /^(lua|lib1_|luaopen)/ {print $8}' | sort | tr '\n' ' ')
    nd=$(readelf -dW "$T/libs/$f.so" | awk '/NEEDED/{print $5}' | tr '\n' ' ')
    ex=$(readelf --dyn-syms -W "$T/libs/$f.so" | awk '$7!="UND" && $5=="GLOBAL" {print $8}' | sort | tr '\n' ' ')
    echo "$k $f.so: undefined: $und | needed: $nd | defined globals: $ex"
  done
  say "lua $k: attrib.lua (cwd = a fresh copy of the test directory)"
  (cd "$T" && timeout 120 "$WORK/lua/lua" attrib.lua) > "$WORK/lua/attrib-$k.log" 2>&1; echo "attrib.lua rc=$?"
  grep -n -E 'cannot load|OK|^\+|error|assert' "$WORK/lua/attrib-$k.log" | head -8
  (cd "$T" && timeout 60 "$WORK/lua/lua" "$HERE/src/luamods.lua") 2>&1; echo "luamods.lua rc=$?"
done

# ======================================================== (c) googletest
say "googletest: compile (CMake's cxx_default flags for gtest_dll: -Wall -Wshadow -Wundef -Wno-error=dangling-else -DGTEST_HAS_PTHREAD=1 -fexceptions -fPIC; -DGTEST_CREATE_SHARED_LIBRARY=1)"
GO=$WORK/gtest; mkdir -p "$GO"
GFLAGS="-I$GT/include -I$GT -Wall -Wshadow -Wundef -Wno-error=dangling-else -DGTEST_HAS_PTHREAD=1 -fexceptions"
for s in gtest-all gtest_main; do g++ $GFLAGS -fPIC -DGTEST_CREATE_SHARED_LIBRARY=1 -c -o "$GO/$s.o" "$GT/src/$s.cc" || echo "COMPILE FAIL $s"; done
g++ $GFLAGS -DGTEST_LINKED_AS_SHARED_LIBRARY=1 -c -o "$GO/gtest_all_test.o" "$GT/test/gtest_all_test.cc" || echo "COMPILE FAIL gtest_all_test"
g++ $GFLAGS -DGTEST_LINKED_AS_SHARED_LIBRARY=1 -c -o "$GO/crossthrow.o" "$HERE/src/crossthrow.cc" || echo "COMPILE FAIL crossthrow"
for k in $LINKERS; do
  D=$GO/$k; mkdir -p "$D"
  # CMake's line: c++ -fPIC -shared -Wl,--dependency-file=... -Wl,-soname,libgtest_dll.so -o libgtest_dll.so objs
  g++ -fPIC -shared $(lflag $k) -Wl,--dependency-file="$D/link.d" -Wl,-soname,libgtest_dll.so -o "$D/libgtest_dll.so" "$GO/gtest-all.o" "$GO/gtest_main.o"
  echo "$k libgtest_dll.so link rc=$? (dependency file: $(wc -l < "$D/link.d" 2>/dev/null || echo none) lines)"
  readelf -dW "$D/libgtest_dll.so" | sed 1,2d > "$D/dyn.txt"
  echo "$k: $(grep -E 'NEEDED|SONAME|FLAGS' "$D/dyn.txt" | awk '{print $2, $NF}' | tr '\n' ' ')"
  echo "$k: dynsyms $(dynsyms "$D/libgtest_dll.so" | wc -l) (defined $(dynsyms "$D/libgtest_dll.so" | grep -c ' DEF ')); main exported: $(dynsyms "$D/libgtest_dll.so" | awk '$5=="main"' | wc -l); size $(stat -c %s "$D/libgtest_dll.so")"
  dynsyms "$D/libgtest_dll.so" > "$D/dynsyms.txt"
  echo "$k: e_ident OS/ABI: $(readelf -hW "$D/libgtest_dll.so" | awk -F: '/OS\/ABI/{gsub(/^ +/,"",$2); print $2}'); STB_GNU_UNIQUE dynsyms: $(awk '$1 ~ /^UNIQUE/' "$D/dynsyms.txt" | wc -l)"
  g++ -o "$D/gtest_dll_test" "$GO/gtest_all_test.o" -L"$D" -lgtest_dll -Wl,-rpath,"$D"; echo "gtest_dll_test (GNU ld) rc=$?"
  g++ -o "$D/crossthrow" "$GO/crossthrow.o" -L"$D" -lgtest_dll -Wl,-rpath,"$D"; echo "crossthrow (GNU ld) rc=$?"
  (cd "$D" && timeout 300 ./gtest_dll_test) > "$D/gtest_dll_test.log" 2>&1; echo "$k gtest_dll_test rc=$? :: $(grep -E '^\[  PASSED  \]|^\[  FAILED  \]|tests? ran|FAILED TESTS' "$D/gtest_dll_test.log" | tr '\n' ' ')"
  (cd "$D" && timeout 60 ./crossthrow) > "$D/crossthrow.log" 2>&1; echo "$k crossthrow rc=$? :: $(grep -E 'OK|FAIL|PASSED' "$D/crossthrow.log" | tr '\n' ' ')"
done
for k in lld lld19 mold; do diff <(awk '{print $5}' "$GO/bfd/dynsyms.txt") <(awk '{print $5}' "$GO/$k/dynsyms.txt") > "gtest-bfd-$k.diff"; echo "gtest dynsym names bfd vs $k: diff rc=$? ($(grep -c '^[<>]' "gtest-bfd-$k.diff") lines)"; head -10 "gtest-bfd-$k.diff"; done
say "gtest LD_LIBRARY_PATH swap: test built against bfd's lib, run with each lib (rpath first: use a copy without rpath)"
g++ -o "$GO/crossthrow.norpath" "$GO/crossthrow.o" -L"$GO/bfd" -lgtest_dll
for r in $LINKERS; do (cd "$GO" && LD_LIBRARY_PATH=$GO/$r timeout 60 ./crossthrow.norpath) > "$GO/swap-$r.log" 2>&1; echo "swap run-with=$r rc=$? :: $(grep -E 'OK|FAIL|PASSED' "$GO/swap-$r.log" | tr '\n' ' ')"; done

# ======================================================== mortar on the real lines
say "mortar on the real -shared lines (gcc -B<dir with ld -> mortar>)"
MB=$WORK/mortarbin; mkdir -p "$MB"; ln -s "$MORTAR" "$MB/ld"
gcc -B"$MB/" -shared -Wl,-soname,libz.so.1,--version-script,"$WORK/zlib/zlib.map" -o "$WORK/mortar-libz.so" $ZOBJS -lc; echo "rc=$?"
gcc -B"$MB/" -shared -o "$WORK/mortar-lib1.so" "$WORK/lua/lib1.o"; echo "rc=$?"
g++ -B"$MB/" -fPIC -shared -Wl,--dependency-file="$WORK/mortar-link.d" -Wl,-soname,libgtest_dll.so -o "$WORK/mortar-gtest.so" "$GO/gtest-all.o" "$GO/gtest_main.o"; echo "rc=$?"
gcc -B"$MB/" -std=gnu99 -o "$WORK/mortar-lua" "$LO/lua.o" "$WORK/lua/liblua.a" -lm -Wl,-E -ldl; echo "mortar lua interpreter (executable, -Wl,-E) rc=$?"
[ -x "$WORK/mortar-lua" ] && { (cd "$WORK/lua/tests-bfd" && timeout 120 "$WORK/mortar-lua" attrib.lua) > "$WORK/lua/attrib-mortar-lua.log" 2>&1; echo "attrib.lua under mortar-linked lua with GNU ld modules rc=$?"; }

# ======================================================== trim the trees
say "trim: delete extracted sources and big objects (literal-path find -delete)"
find "$WORK/src" -mindepth 1 -delete
find "$WORK/gtest" \( -name '*.o' -o -name gtest_dll_test -o -name 'core*' \) -type f -delete
du -sh "$WORK"

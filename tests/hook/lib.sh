# WP-077: the matrix's build helpers (sourced by run.sh and cost.sh).
# Needs: T (aarch64 | x86_64 | i686), W (an empty work directory), H (this
# directory), MORTAR, XTASK. RT_FLAVOR = hod (default) | gcc picks the runtime
# object every module gets.
set -u
ulimit -c 0
KILN=${KILN:-/Volumes/AgentWork/mortar-helper/.build/target-kiln-next/release/kiln++}
HOD=${HOD:-/Volumes/AgentWork/mortar-helper/.build/target-hod-next/release/hod}
HOD386=${HOD386:-/Volumes/AgentWork/mortar-helper/.build/target-hod-a68/release/hod}
CLANG22=${CLANG22:-clang++-22}
RT_FLAVOR=${RT_FLAVOR:-hod}
case $T in
  aarch64) CC=gcc; CXX=g++; RUN=(); KT=() ;;
  x86_64) CC=x86_64-linux-gnu-gcc; CXX=x86_64-linux-gnu-g++; RUN=(qemu-x86_64 -L /usr/x86_64-linux-gnu); KT=(--target=x86_64-linux-gnu) ;;
  i686) CC=i686-linux-gnu-gcc; CXX=i686-linux-gnu-g++; RUN=(qemu-i386 -L /usr/i686-linux-gnu); KT=() ;;
  *) echo "lib.sh: T=$T?"; exit 2 ;;
esac
mkdir -p "$W/wrap"
printf '#!/bin/sh\nexec %s "$@"\n' "$MORTAR" > "$W/wrap/ld"
chmod +x "$W/wrap/ld"
LDM=(-B"$W/wrap/")   # gcc/g++ drive mortar as their ld

# the runtime: hod's object (kiln++ on aarch64 and x86-64, clang 22 IR on i386) and GCC -O2's
build_rt() {
  local s=$H/mason_odr_rt.cpp f
  for f in "" -DMASON_RT_EXE; do
    local o=rt${f:+exe}
    if [ "$T" = i686 ]; then
      $CLANG22 --target=i686-linux-gnu -O0 -fPIC -fno-exceptions -fno-rtti $f -S -emit-llvm -o "$W/$o-hod.ll" "$s" || return 1
      "$HOD386" -O0 -filetype=obj -mtriple=i686-linux-gnu -relocation-model=pic --strict "$W/$o-hod.ll" -o "$W/$o-hod.o" || return 1
    else
      KILN_BACKEND=hod KILN_HOD=$HOD KILN_HOD_STRICT=1 "$KILN" "${KT[@]}" -c -O0 -fPIC -fno-exceptions -fno-rtti $f "$s" -o "$W/$o-hod.o" || return 1
    fi
    $CXX -O2 -fPIC -fno-exceptions -fno-rtti $f -c "$s" -o "$W/$o-gcc.o" || return 1
  done
  # the runtime note mortar would write (proposed NT_MASON_ODR_RT): {version 1, mode, flags, 0}
  local m
  for m in 1:report 2:fatal; do
    printf '#include <stdint.h>\n__attribute__((section(".note.mason.rt"), aligned(4), used)) static const struct { uint32_t namesz, descsz, type; char name[12]; uint32_t d[4]; } n = { 10, 16, 0x4d4f4403u, "MasonMeta", {1, %s, 0, 0} };\n' "${m%%:*}" > "$W/note-${m#*:}.c"
    $CC -c "$W/note-${m#*:}.c" -o "$W/note-${m#*:}.o" || return 1
  done
  RT=$W/rt-$RT_FLAVOR.o
  RTEXE=$W/rtexe-$RT_FLAVOR.o
}

hash_of() { # a 16-byte kind 2 hash for body tag N
  printf '%016x%016x' $(( ($1 * 2654435761 + 40503) & 0x7fffffffffffffff )) $(( $1 + 4096 ))
}

# meta OBJ_IN OBJ_OUT "symbol=body ..." [system]: add a test-producer unit with kind 2 rows
meta() {
  local in=$1 out=$2 rows=$3 fl=${4:-1} d=$2.meta a
  echo 'summary name=lang value=c++' > "$d"
  for a in $rows; do echo "definition anchor=${a%%=*} hash=$(hash_of ${a#*=}) class=1 flags=$fl" >> "$d"; done
  "$XTASK" meta write "$d" --into "$in" -o "$out"
}

# the plugin source: fmtlite::version() of body N, entry_NAME and plugin_entry
plug_src() { # plug_src NAME N
  printf 'namespace fmtlite { inline int version() { return %d; } }\nextern "C" int entry_%s() { return fmtlite::version(); }\nextern "C" int plugin_entry() { return fmtlite::version(); }\n' "$2" "$1" > "$W/$1.cpp"
  $CXX -O0 -fPIC -c "$W/$1.cpp" -o "$W/$1.0.o"
}

# plug NAME N KIND [link flags...]: KIND = rt (mortar, runtime, table, -z now: what
# the proposed option does), rtlazy (the same with -z lazy), table (mortar, table
# only), plain (GCC/GNU ld, nothing)
plug() {
  local n=$1 b=$2 k=$3; shift 3
  plug_src "$n" "$b" || return 1
  case $k in
    plain) $CXX -O0 -fPIC -shared "$W/$n.0.o" "$@" -o "$W/$n.so" ;;
    table) meta "$W/$n.0.o" "$W/$n.o" "_ZN7fmtlite7versionEv=$b" && \
           $CXX "${LDM[@]}" -shared "$W/$n.o" -Wl,--odr-table=exported "$@" -o "$W/$n.so" ;;
    rt|rtlazy) local z=now; [ "$k" = rtlazy ] && z=lazy
           meta "$W/$n.0.o" "$W/$n.o" "_ZN7fmtlite7versionEv=$b" && \
           $CXX "${LDM[@]}" -shared "$W/$n.o" "$RT" "$W/note-report.o" -Wl,--odr-table=exported -Wl,-z,$z "$@" -o "$W/$n.so" ;;
  esac
}

# xrun ENV... -- PROG ARGS: run natively with env, or under qemu with -E
xrun() {
  local envs=()
  while [ "$1" != -- ]; do envs+=("$1"); shift; done
  shift
  if [ ${#RUN[@]} -eq 0 ]; then env "${envs[@]}" "$@"
  else local e qe=(); for e in "${envs[@]}"; do qe+=(-E "$e"); done; "${RUN[@]}" "${qe[@]}" "$@"; fi
}

# r LABEL ENV... -- PROG ARGS: run, print the output indented, the exit status, and
# the counts the matrix reads
r() {
  local label=$1; shift
  local envs=()
  while [ "$1" != -- ]; do envs+=("$1"); shift; done
  shift
  local out st
  if [ ${#RUN[@]} -eq 0 ]; then
    out=$(cd "$W" && env "${envs[@]}" "$@" 2>&1); st=$?
  else
    local e qe=()
    for e in "${envs[@]}"; do qe+=(-E "$e"); done
    out=$(cd "$W" && "${RUN[@]}" "${qe[@]}" "$@" 2>&1); st=$?
  fi
  printf '%s\n' "$out" | sed 's/^/    /'
  local rep warn
  rep=$(printf '%s\n' "$out" | grep -c ': odr-violation-at-load: [^0-9]')
  warn=$(printf '%s\n' "$out" | grep -c 'mason: warning: odr-\(hook\|interposed\)')
  LAST="reports=$rep other-warnings=$warn exit=$st"
  echo "    [$label] $LAST"
}

# verdict ID REQUIRED: REQUIRED = none | report | report-or-exclusion; reads $LAST
verdict() {
  local id=$1 req=$2 rep ok
  rep=$(echo "$LAST" | sed 's/reports=\([0-9]*\).*/\1/')
  case $req in
    none) [ "$rep" = 0 ] && ok=pass || ok=FAIL ;;
    report) [ "$rep" -gt 0 ] && ok=pass || ok=FAIL ;;
  esac
  echo "MATRIX $T $RT_FLAVOR $id required=$req observed: $LAST => $ok"
}

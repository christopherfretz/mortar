#!/bin/bash
# protoc.sh WORK MORTAR [gcc|clang]: WP-229 item 4, gdb's start-up without the GNU
# name index. protoc 3.21.12 (the bench's source) built static,
# `-O0 -g -gsplit-dwarf`, by GCC 14 and Clang 19 (R14's build.sh), then
# linked by mortar from cmake's own link line twice: plain (the index
# kept) and `--incremental=full` (the index left out, IN-55 (A)); a third
# copy with `gdb-add-index` (a .gdb_index) for reference. Per output: its
# size, skeleton count, the GNU index bytes; gdb start-up timed (`gdb
# -batch -nx -ex 'break main' -ex 'info line main'`, -readnow off) one
# cold run right after the link and 7 warm runs, interleaved; lldb the
# same (`breakpoint set -n main`). The last line is `protoc: done`.
set -u; ulimit -c 0
W="$1"; M="$2"; ONLY="${3:-gcc clang}"
S=/Volumes/AgentWork/mortar-helper/.build/bench-work/src/4eab9b524aa5913c/protobuf-3.21.12
mkdir -p "$W/bin"; ln -sf "$M" "$W/bin/ld"
waitload() { while [ "$(cut -d. -f1 /proc/loadavg)" -ge 12 ]; do sleep 60; done; }
t() { local s e; s=$(date +%s.%N); "$@" > /dev/null 2>&1; e=$(date +%s.%N); awk -v s="$s" -v e="$e" 'BEGIN { printf "%.3f\n", e - s }'; }
for c in $ONLY; do
  case $c in gcc) CC=/usr/bin/gcc CXX=/usr/bin/g++;; clang) CC=/usr/bin/clang-19 CXX=/usr/bin/clang++-19;; esac
  B="$W/build-$c"; mkdir -p "$B"
  if [ ! -x "$B/protoc" ]; then
    waitload
    t0=$(date +%s)
    (cd "$B" && cmake -G "Unix Makefiles" -DCMAKE_BUILD_TYPE=None -DCMAKE_C_COMPILER=$CC -DCMAKE_CXX_COMPILER=$CXX \
       "-DCMAKE_C_FLAGS=-O0 -g -gsplit-dwarf" "-DCMAKE_CXX_FLAGS=-O0 -g -gsplit-dwarf" -DCMAKE_EXE_LINKER_FLAGS=-static \
       -Dprotobuf_BUILD_SHARED_LIBS=OFF -Dprotobuf_BUILD_TESTS=OFF -Dprotobuf_BUILD_CONFORMANCE=OFF -Dprotobuf_BUILD_EXAMPLES=OFF \
       -Dprotobuf_BUILD_LIBPROTOC=OFF -Dprotobuf_WITH_ZLIB=OFF "$S" > "$W/cmake-$c.log" 2>&1 && \
     nice -n 10 make -j3 protoc > "$W/make-$c.log" 2>&1) || { echo "protoc: $c build FAILED"; continue; }
    echo "protoc: $c built in $(( $(date +%s) - t0 )) s"
  fi
  link=$(sed -n 1p "$B/CMakeFiles/protoc.dir/link.txt")
  for v in plain incr; do
    case $v in plain) x="";; incr) x="-Wl,--incremental=full";; esac
    out="$W/protoc-$c-$v"
    (cd "$B" && eval "${link/ -o protoc / -o $out } -B$W/bin $x") > "$W/link-$c-$v.log" 2>&1 \
      || { echo "protoc: $c $v link FAILED: $(head -2 "$W/link-$c-$v.log")"; continue; }
    sk=$(llvm-dwarfdump-22 --debug-info "$out" | grep -c 'DW_TAG_skeleton_unit\|DW_AT_GNU_dwo_name')
    gi=$(readelf -SW "$out" | awk '/debug_gnu_pub/ {s += strtonum("0x" $6)} END {print s + 0}')
    echo "protoc: $c $v size=$(stat -c %s "$out") skeleton-units=$sk gnu-index-bytes=$gi runs: $("$out" --version 2>&1 | head -1)"
    echo "protoc: $c $v gdb-cold=$(cd "$B" && t gdb -batch -nx -ex 'break main' -ex 'info line main' "$out") s"
  done
  cp "$W/protoc-$c-incr" "$W/protoc-$c-gdbidx"
  (cd "$B" && gdb-add-index "$W/protoc-$c-gdbidx") > /dev/null 2>&1 || echo "protoc: $c gdb-add-index FAILED"
  echo "protoc: $c gdbidx size=$(stat -c %s "$W/protoc-$c-gdbidx")"
  for i in 1 2 3 4 5 6 7; do
    waitload
    line="load=$(cut -d' ' -f1 /proc/loadavg)"
    for v in plain incr gdbidx; do
      line="$line $v-gdb=$(cd "$B" && t gdb -batch -nx -ex 'break main' -ex 'info line main' "$W/protoc-$c-$v")"
    done
    for v in plain incr; do
      line="$line $v-gdbfn=$(cd "$B" && t gdb -batch -nx -ex 'info functions DebugString$' "$W/protoc-$c-$v")"
      line="$line $v-lldb=$(cd "$B" && t lldb -b -o 'breakpoint set -n main' "$W/protoc-$c-$v")"
    done
    echo "protoc: $c warm $i $line"
  done
  # the same answers with and without the index
  for v in plain incr; do
    (cd "$B" && gdb -batch -nx -ex 'break main' -ex 'run --version' -ex bt -ex 'info source' \
       -ex 'ptype google::protobuf::FieldDescriptor::Type' -ex 'info functions ^google::protobuf::DescriptorPool::generated_pool' \
       -ex 'info line google::protobuf::FileDescriptor::DebugString' "$W/protoc-$c-$v" 2>&1 \
       | sed -e 's/0x[0-9a-f]*/ADDR/g' -e 's/process [0-9]*/process N/' -e "s#$W/protoc-$c-$v#X#g" > "$W/gdb-$c-$v.txt")
  done
  if cmp -s "$W/gdb-$c-plain.txt" "$W/gdb-$c-incr.txt"; then echo "protoc: $c gdb transcript plain vs incr: SAME ($(wc -l < "$W/gdb-$c-plain.txt") lines)"; else echo "protoc: $c gdb transcript plain vs incr: DIFF"; fi
done
echo "protoc: done"

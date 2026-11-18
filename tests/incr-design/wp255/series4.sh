#!/bin/bash
# WP-255: the full links the cost model is judged against (the CLI,
# --time-phases, interleaved), and the refill on the large workload
# (clang -g, static, one member of libLLVMSupport.a edited): the outputs
# (1.1 GB each, two at a time in the test) deleted by literal name.
ulimit -c 0
S=/Volumes/AgentWork/mortar-helper/.build/target-wp-140/wp255
B=/Volumes/AgentWork/mortar-helper/.build/target-wp-140/release
R=/Volumes/AgentWork/mortar-helper/mortar/.worktrees/wp-140/tests/incr-design/wp255/readset.sh
quiet() { while awk -v l="$(cut -d' ' -f1 /proc/loadavg)" 'BEGIN{exit !(l>12)}'; do sleep 20; done; }
one() { # tag workload
  W=/Volumes/AgentWork/mortar-helper/.build/bench-work/workloads/$2
  out=$S/full-$2; mkdir -p $out
  cwd=$(awk '$1=="cwd"{print $2}' "$W/stamp")
  awk -v o="$out/out" 'prev=="-o"{print o; prev=""; next} /^--dependency-file=/{next} {print; prev=$0}' "$W/response.txt" > "$out/response.txt"
  quiet
  l=$(cut -d' ' -f1 /proc/loadavg)
  ( cd "$cwd" && /usr/bin/time -f "%e %M" -o "$out/t.txt" "$B/mortar" @"$out/response.txt" --time-phases ) > "$out/$1.log" 2>&1
  echo "$1 $(cat $out/t.txt) load=$l" >> $out/runs.txt
  cd $out && rm -f out
}
for i in 1 2 3 4 5; do one r$i cxx-medium-static; one r$i cxx-medium-g-static; done
for i in 1 2 3; do one r$i cxx-large-g; done
WP255_CLIPS=1 $R cxx-large-g lib/libLLVMSupport.a StringRef.cpp.o L1c time 3
$R cxx-large-g lib/libLLVMSupport.a StringRef.cpp.o L1 time 3
echo "series4 done"

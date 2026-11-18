#!/bin/bash
# WP-255: E2, the refill filling its `want` ranges as clips (WP255_CLIPS,
# the throwaway refill_ranges) against whole windows: bytes judged, read
# set and time; and the state primitives (state.sh's test).
R=/Volumes/AgentWork/mortar-helper/mortar/.worktrees/wp-140/tests/incr-design/wp255/readset.sh
$R cxx-medium-static libprotobuf.a strutil.cc.o p1 probe 1
$R cxx-medium-g-static libprotobuf.a strutil.cc.o g1 probe 1
export WP255_CLIPS=1
$R cxx-medium-static libprotobuf.a strutil.cc.o p1c probe 1
$R cxx-medium-g-static libprotobuf.a strutil.cc.o g1c probe 1
for i in 1 2 3; do
  WP255_CLIPS=1 $R cxx-medium-static libprotobuf.a strutil.cc.o p1ct$i time 5
  env -u WP255_CLIPS $R cxx-medium-static libprotobuf.a strutil.cc.o p1t$i time 5
  WP255_CLIPS=1 $R cxx-medium-g-static libprotobuf.a strutil.cc.o g1ct$i time 5
  env -u WP255_CLIPS $R cxx-medium-g-static libprotobuf.a strutil.cc.o g1t$i time 5
done
unset WP255_CLIPS
echo "series2 done"

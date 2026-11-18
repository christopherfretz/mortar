#!/bin/bash
# WP-255: the read-set probe and the timing runs, protoc without and with -g.
R=/Volumes/AgentWork/mortar-helper/mortar/.worktrees/wp-140/tests/incr-design/wp255/readset.sh
$R cxx-medium-static libprotobuf.a strutil.cc.o p0 probe 1
$R cxx-medium-g-static libprotobuf.a strutil.cc.o g0 probe 1
$R cxx-medium-static libprotobuf.a strutil.cc.o p0t time 7
$R cxx-medium-g-static libprotobuf.a strutil.cc.o g0t time 7
echo "series1 done"

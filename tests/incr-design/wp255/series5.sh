#!/bin/bash
# WP-255: series4's large-workload refill lines again, after member_of
# learned to require the archive (two members named StringRef.cpp.o).
ulimit -c 0
R=/Volumes/AgentWork/mortar-helper/mortar/.worktrees/wp-140/tests/incr-design/wp255/readset.sh
WP255_CLIPS=1 $R cxx-large-g lib/libLLVMSupport.a StringRef.cpp.o L1c time 3
[ -n "$ONLY_CLIPS" ] || $R cxx-large-g lib/libLLVMSupport.a StringRef.cpp.o L1 time 3
echo "series5 done"
